//! `ScrollView`: a generic scrollable region for arbitrary child content.
//! Shared logic only — scroll-offset state, wheel/keyboard handling, and
//! scrollbar geometry; no backend painter lives here (see
//! `docs/PRIMITIVE_RULES.md`'s cross-backend commitment — a rasteriser
//! for the actual clipped paint is a later, backend-scoped issue).
//!
//! Every other scrollable primitive in this crate (`List`, `TreeView`,
//! `DataTable`, `MessageList`) owns its own row-indexed scroll state
//! because it also owns row layout. `ScrollView` is for everything
//! else: a host that wants to scroll content it lays out itself (a
//! form section, a help panel, a canvas) without reimplementing offset
//! clamping, wheel/keyboard deltas, and thumb geometry. It is additive
//! — none of the existing primitives change.
//!
//! # Clipping
//!
//! `ScrollView` computes *what* is visible, not how to paint only that:
//! [`ScrollView::visible_range`] returns the `[start, end)` window of
//! content units currently inside [`ScrollView::viewport`], and
//! [`ScrollView::is_visible`] answers "does this child's extent overlap
//! that window" for one child at a time. A host skips painting (TUI) or
//! pushes a clip rect before painting (GTK/win) based on those answers;
//! `ScrollView` itself never touches a [`crate::backend::Backend`].
//!
//! # Units and the v0.2 `f32` offset migration
//!
//! [`ScrollView::scroll_offset`] and [`ScrollView::content_len`] are
//! already `f32`, in whatever unit the host's content is measured in —
//! rows, wrapped lines, or raw pixels. A host with discrete,
//! row-indexed content (the common case) keeps the value whole
//! (`+1.0`/`-1.0` steps) and rounds it to a `usize` index at the point
//! it indexes its own content, the same split [`crate::ThumbDrag`]'s
//! own doc comment already documents for exactly this reason: one
//! struct serving both discrete and continuous callers without
//! duplicating it. A host with continuous (pixel) content uses
//! fractional values directly. Because the field is `f32` from the
//! start, the v0.2 migration to crate-wide continuous scroll offsets
//! needs no follow-up API change here — `ScrollView` already is the
//! post-migration shape; only the discrete primitives that still use
//! `usize` today (`List`, `TreeView`, `DataTable`) have a migration
//! path to walk.
//!
//! # Axis
//!
//! A `ScrollView` scrolls along one [`ScrollAxis`] at a time —
//! vertical by default, or horizontal via [`ScrollView::horizontal`].
//! A host that needs both independently composes two `ScrollView`s
//! (one per axis), the same way a bidirectional scroll area is built
//! from a horizontal and a vertical [`Scrollbar`] elsewhere in this
//! crate.

use crate::event::{Key, NamedKey, Rect, ScrollDelta};
use crate::primitives::scrollbar::{fit_thumb, ScrollAxis, Scrollbar};
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative scroll-offset state for a generic scrollable region.
///
/// # Examples
///
/// ```
/// use quadraui::{Rect, ScrollView, ScrollViewEvent, WidgetId};
///
/// let viewport = Rect::new(0.0, 0.0, 20.0, 10.0);
/// let mut view = ScrollView::new(WidgetId::new("panel:help"), viewport, 50.0);
///
/// // 50 rows of content, 10 visible: max offset is 40.
/// assert_eq!(view.max_offset(), 40.0);
///
/// let event = view.scroll_by(5.0).unwrap();
/// assert_eq!(event, ScrollViewEvent::Scrolled { offset: 5.0 });
/// view.scroll_offset = 5.0;
///
/// let (start, end) = view.visible_range();
/// assert_eq!((start, end), (5.0, 15.0));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrollView {
    pub id: WidgetId,
    #[serde(default = "ScrollAxis::vertical_default")]
    pub axis: ScrollAxis,
    /// The clip region, in surface-native units. Content outside this
    /// rect (after subtracting [`Self::scroll_offset`]) is not visible.
    pub viewport: Rect,
    /// Total scrollable extent of the child content, in the host's own
    /// units (rows, wrapped lines, pixels — see the module doc).
    pub content_len: f32,
    /// First visible content unit. Always kept in `[0, max_offset()]`
    /// by every mutating method on this type; a host that writes the
    /// field directly (e.g. restoring saved scroll position) should
    /// clamp it itself, or call [`Self::clamp_offset`] afterwards.
    #[serde(default)]
    pub scroll_offset: f32,
    /// Whether this view currently holds keyboard focus. Gates
    /// [`Self::handle_key`] the same way every other standalone
    /// primitive's `focused` flag gates its own `handle_key` — wheel
    /// scrolling via [`Self::handle_wheel`] is ungated, since wheel
    /// events route to the widget under the cursor, not the focused
    /// one (see [`crate::UiEvent::Scroll`]'s own doc).
    #[serde(default)]
    pub focused: bool,
}

/// Semantic event produced by [`ScrollView::handle_wheel`],
/// [`ScrollView::handle_key`], or [`ScrollView::scroll_by`].
///
/// Like [`crate::ButtonEvent`] and friends, `ScrollView`'s own methods
/// take `&self` and return this instead of mutating — the host applies
/// `offset` back onto [`ScrollView::scroll_offset`] itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ScrollViewEvent {
    /// The resolved new offset, already clamped to `[0, max_offset()]`.
    Scrolled { offset: f32 },
}

impl ScrollAxis {
    /// `serde(default)` helper — [`ScrollAxis`] has no `Default` impl of
    /// its own (it has no obviously-more-correct axis), so
    /// [`ScrollView`] picks vertical, the common case for a scrollable
    /// panel.
    fn vertical_default() -> Self {
        ScrollAxis::Vertical
    }
}

impl ScrollView {
    /// Build a vertical `ScrollView` (the common case — a panel that
    /// scrolls top-to-bottom). `scroll_offset` starts at `0.0`.
    pub fn new(id: impl Into<WidgetId>, viewport: Rect, content_len: f32) -> Self {
        Self {
            id: id.into(),
            axis: ScrollAxis::Vertical,
            viewport,
            content_len,
            scroll_offset: 0.0,
            focused: false,
        }
    }

    /// Build a vertical `ScrollView`. Equivalent to [`Self::new`].
    pub fn vertical(id: impl Into<WidgetId>, viewport: Rect, content_len: f32) -> Self {
        Self::new(id, viewport, content_len)
    }

    /// Build a horizontal `ScrollView` — content scrolls left-to-right
    /// instead of top-to-bottom.
    pub fn horizontal(id: impl Into<WidgetId>, viewport: Rect, content_len: f32) -> Self {
        Self {
            axis: ScrollAxis::Horizontal,
            ..Self::new(id, viewport, content_len)
        }
    }

    /// The viewport's extent along [`Self::axis`] — `viewport.height`
    /// for vertical, `viewport.width` for horizontal.
    pub fn visible_len(&self) -> f32 {
        match self.axis {
            ScrollAxis::Vertical => self.viewport.height,
            ScrollAxis::Horizontal => self.viewport.width,
        }
    }

    /// The largest reachable [`Self::scroll_offset`]: `content_len -
    /// visible_len()`, floored at `0.0` when all content already fits.
    pub fn max_offset(&self) -> f32 {
        (self.content_len - self.visible_len()).max(0.0)
    }

    /// [`Self::scroll_offset`] clamped into `[0, max_offset()]`.
    pub fn clamped_offset(&self) -> f32 {
        self.scroll_offset.clamp(0.0, self.max_offset())
    }

    /// Pin [`Self::scroll_offset`] into `[0, max_offset()]` in place —
    /// for a host that just wrote the field directly (e.g. restoring a
    /// saved scroll position, or after [`Self::content_len`] shrank).
    pub fn clamp_offset(&mut self) {
        self.scroll_offset = self.clamped_offset();
    }

    /// The `[start, end)` window of content units currently visible,
    /// already clamped into `[0, content_len]`. Empty content
    /// (`content_len <= 0.0`) returns `(0.0, 0.0)`.
    pub fn visible_range(&self) -> (f32, f32) {
        if self.content_len <= 0.0 {
            return (0.0, 0.0);
        }
        let start = self.clamped_offset();
        let end = (start + self.visible_len()).min(self.content_len);
        (start, end)
    }

    /// Whether a child occupying `[child_start, child_start +
    /// child_len)` overlaps [`Self::visible_range`] — the host's
    /// per-child "should I paint this" check. A zero-length child
    /// (`child_len <= 0.0`) is never visible.
    pub fn is_visible(&self, child_start: f32, child_len: f32) -> bool {
        if child_len <= 0.0 {
            return false;
        }
        let (start, end) = self.visible_range();
        child_start < end && (child_start + child_len) > start
    }

    /// Resolve a scroll by `delta` content units (positive = forward,
    /// i.e. down for vertical / right for horizontal), clamped to
    /// `[0, max_offset()]`. Returns `None` when the resolved offset
    /// equals the current one (nothing to apply) — e.g. already at an
    /// end and scrolling further that way.
    pub fn scroll_by(&self, delta: f32) -> Option<ScrollViewEvent> {
        let offset = (self.scroll_offset + delta).clamp(0.0, self.max_offset());
        if offset == self.scroll_offset {
            None
        } else {
            Some(ScrollViewEvent::Scrolled { offset })
        }
    }

    /// Mouse-wheel handling. `delta` is the raw [`ScrollDelta`] from
    /// [`crate::UiEvent::Scroll`]; `step` is how many content units one
    /// wheel "notch" moves (host-chosen — typically one row/line, or a
    /// fixed pixel count for continuous content). Always consults
    /// `delta.y` for a vertical view and `delta.x` for a horizontal
    /// one, regardless of [`Self::focused`] — wheel routes by cursor
    /// position, resolved by the host before calling this.
    pub fn handle_wheel(&self, delta: ScrollDelta, step: f32) -> Option<ScrollViewEvent> {
        let notches = match self.axis {
            ScrollAxis::Vertical => delta.y,
            ScrollAxis::Horizontal => delta.x,
        };
        self.scroll_by(notches * step)
    }

    /// Keyboard handling: `Up`/`Down` (or `Left`/`Right` for a
    /// horizontal view) move by `step`; `PageUp`/`PageDown` move by one
    /// [`Self::visible_len`]; `Home`/`End` jump to `0.0` /
    /// [`Self::max_offset`]. Returns `None` when unfocused, when `key`
    /// isn't one of those, or when the resolved offset doesn't change.
    pub fn handle_key(
        &self,
        key: &Key,
        _modifiers: &Modifiers,
        step: f32,
    ) -> Option<ScrollViewEvent> {
        if !self.focused {
            return None;
        }
        let (forward, backward) = match self.axis {
            ScrollAxis::Vertical => (NamedKey::Down, NamedKey::Up),
            ScrollAxis::Horizontal => (NamedKey::Right, NamedKey::Left),
        };
        match key {
            Key::Named(k) if *k == forward => self.scroll_by(step),
            Key::Named(k) if *k == backward => self.scroll_by(-step),
            Key::Named(NamedKey::PageDown) => self.scroll_by(self.visible_len()),
            Key::Named(NamedKey::PageUp) => self.scroll_by(-self.visible_len()),
            Key::Named(NamedKey::Home) => {
                if self.scroll_offset == 0.0 {
                    None
                } else {
                    Some(ScrollViewEvent::Scrolled { offset: 0.0 })
                }
            }
            Key::Named(NamedKey::End) => {
                let offset = self.max_offset();
                if offset == self.scroll_offset {
                    None
                } else {
                    Some(ScrollViewEvent::Scrolled { offset })
                }
            }
            _ => None,
        }
    }

    /// Scrollbar geometry for the current offset, via the crate's one
    /// canonical thumb-fitting formula ([`fit_thumb`]). Returns `None`
    /// when there is nothing to scroll (`max_offset() <= 0.0`), mirroring
    /// every other primitive's `*scrollbar()` accessor (e.g.
    /// [`crate::ListView::vscrollbar`]).
    pub fn scrollbar(&self, track: Rect, min_thumb_len: f32) -> Option<Scrollbar> {
        let track_len = match self.axis {
            ScrollAxis::Vertical => track.height,
            ScrollAxis::Horizontal => track.width,
        };
        let (thumb_start, thumb_len) = fit_thumb(
            self.clamped_offset(),
            self.content_len,
            self.visible_len(),
            track_len,
            min_thumb_len,
        );
        if thumb_len <= 0.0 {
            return None;
        }
        Some(Scrollbar {
            id: self.id.clone(),
            axis: self.axis,
            track,
            thumb_start,
            thumb_len,
            hovered: false,
            dragging: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> ScrollView {
        ScrollView::new(
            WidgetId::new("test:scroll"),
            Rect::new(0.0, 0.0, 20.0, 10.0),
            50.0,
        )
    }

    #[test]
    fn max_offset_is_content_minus_visible() {
        assert_eq!(view().max_offset(), 40.0);
    }

    #[test]
    fn max_offset_floors_at_zero_when_content_fits() {
        let v = ScrollView::new(WidgetId::new("x"), Rect::new(0.0, 0.0, 20.0, 10.0), 5.0);
        assert_eq!(v.max_offset(), 0.0);
    }

    #[test]
    fn scroll_by_clamps_to_zero_and_max() {
        let mut v = view();
        // Already at the start: scrolling further backward is a no-op.
        assert_eq!(v.scroll_by(-10.0), None);

        v.scroll_offset = 5.0;
        assert_eq!(
            v.scroll_by(-10.0).unwrap(),
            ScrollViewEvent::Scrolled { offset: 0.0 }
        );
        assert_eq!(
            v.scroll_by(1000.0).unwrap(),
            ScrollViewEvent::Scrolled { offset: 40.0 }
        );
    }

    #[test]
    fn scroll_by_zero_effective_delta_returns_none() {
        let v = view();
        assert_eq!(v.scroll_by(-5.0), None);
    }

    #[test]
    fn visible_range_tracks_offset() {
        let mut v = view();
        assert_eq!(v.visible_range(), (0.0, 10.0));
        v.scroll_offset = 20.0;
        assert_eq!(v.visible_range(), (20.0, 30.0));
        v.scroll_offset = 45.0; // beyond max_offset (40.0)
        assert_eq!(v.visible_range(), (40.0, 50.0));
    }

    #[test]
    fn visible_range_empty_content_is_zero_zero() {
        let v = ScrollView::new(WidgetId::new("x"), Rect::new(0.0, 0.0, 20.0, 10.0), 0.0);
        assert_eq!(v.visible_range(), (0.0, 0.0));
    }

    #[test]
    fn is_visible_detects_overlap_with_the_visible_window() {
        let mut v = view();
        v.scroll_offset = 10.0; // visible [10, 20)
        assert!(v.is_visible(5.0, 10.0)); // overlaps [5, 15)
        assert!(v.is_visible(15.0, 20.0)); // overlaps [15, 35)
        assert!(!v.is_visible(0.0, 5.0)); // [0, 5) is entirely before
        assert!(!v.is_visible(20.0, 5.0)); // [20, 25) is entirely after
        assert!(!v.is_visible(10.0, 0.0)); // zero-length child
    }

    #[test]
    fn handle_wheel_scrolls_vertical_by_delta_y_times_step() {
        let v = view();
        let event = v.handle_wheel(ScrollDelta::new(0.0, 2.0), 3.0);
        assert_eq!(event, Some(ScrollViewEvent::Scrolled { offset: 6.0 }));
    }

    #[test]
    fn handle_wheel_ignores_focus() {
        let v = view(); // focused == false
        assert!(v.handle_wheel(ScrollDelta::new(0.0, 1.0), 1.0).is_some());
    }

    #[test]
    fn handle_wheel_on_horizontal_view_uses_delta_x() {
        let v = ScrollView::horizontal(WidgetId::new("x"), Rect::new(0.0, 0.0, 20.0, 10.0), 50.0);
        let event = v.handle_wheel(ScrollDelta::new(1.0, 5.0), 2.0);
        assert_eq!(event, Some(ScrollViewEvent::Scrolled { offset: 2.0 }));
    }

    #[test]
    fn handle_key_requires_focus() {
        let v = view();
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default(), 1.0),
            None
        );
    }

    #[test]
    fn handle_key_up_down_move_by_step_when_focused() {
        let mut v = view();
        v.focused = true;
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default(), 2.0),
            Some(ScrollViewEvent::Scrolled { offset: 2.0 })
        );
        v.scroll_offset = 10.0;
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default(), 2.0),
            Some(ScrollViewEvent::Scrolled { offset: 8.0 })
        );
    }

    #[test]
    fn handle_key_page_up_down_move_by_visible_len() {
        let mut v = view();
        v.focused = true;
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::PageDown), &Modifiers::default(), 1.0),
            Some(ScrollViewEvent::Scrolled { offset: 10.0 })
        );
        v.scroll_offset = 20.0;
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::PageUp), &Modifiers::default(), 1.0),
            Some(ScrollViewEvent::Scrolled { offset: 10.0 })
        );
    }

    #[test]
    fn handle_key_home_end_jump_to_bounds() {
        let mut v = view();
        v.focused = true;
        v.scroll_offset = 20.0;
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::Home), &Modifiers::default(), 1.0),
            Some(ScrollViewEvent::Scrolled { offset: 0.0 })
        );
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::End), &Modifiers::default(), 1.0),
            Some(ScrollViewEvent::Scrolled { offset: 40.0 })
        );
    }

    #[test]
    fn handle_key_home_already_at_start_returns_none() {
        let mut v = view();
        v.focused = true;
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::Home), &Modifiers::default(), 1.0),
            None
        );
    }

    #[test]
    fn handle_key_horizontal_view_uses_left_right() {
        let mut v =
            ScrollView::horizontal(WidgetId::new("x"), Rect::new(0.0, 0.0, 20.0, 10.0), 50.0);
        v.focused = true;
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::Right), &Modifiers::default(), 3.0),
            Some(ScrollViewEvent::Scrolled { offset: 3.0 })
        );
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default(), 3.0),
            None
        );
    }

    #[test]
    fn handle_key_ignores_unrelated_keys() {
        let mut v = view();
        v.focused = true;
        assert_eq!(
            v.handle_key(&Key::Named(NamedKey::Tab), &Modifiers::default(), 1.0),
            None
        );
    }

    #[test]
    fn scrollbar_none_when_content_fits_viewport() {
        let v = ScrollView::new(WidgetId::new("x"), Rect::new(0.0, 0.0, 20.0, 10.0), 5.0);
        assert_eq!(v.scrollbar(Rect::new(19.0, 0.0, 1.0, 10.0), 1.0), None);
    }

    #[test]
    fn scrollbar_thumb_shrinks_as_offset_advances() {
        let mut v = view();
        let track = Rect::new(19.0, 0.0, 1.0, 10.0);
        let at_top = v.scrollbar(track, 1.0).unwrap();
        assert_eq!(at_top.thumb_start, 0.0);

        v.scroll_offset = v.max_offset();
        let at_bottom = v.scrollbar(track, 1.0).unwrap();
        assert!(at_bottom.thumb_start > at_top.thumb_start);
    }

    #[test]
    fn clamp_offset_pins_a_directly_written_offset() {
        let mut v = view();
        v.scroll_offset = 999.0;
        v.clamp_offset();
        assert_eq!(v.scroll_offset, 40.0);
    }
}
