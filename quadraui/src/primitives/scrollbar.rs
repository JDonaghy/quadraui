//! `Scrollbar` primitive: a thin track-and-thumb indicator showing the
//! visible window's position within a larger scrollable region.
//!
//! Used by editor viewports (vertical line scroll, horizontal column
//! scroll), tab-group viewports, and any panel with overflowing content.
//! The primitive is intentionally data-only: it carries pre-computed
//! `thumb_start` + `thumb_len` positions along the track, in the same
//! units as the rasteriser's surface (cells for TUI, pixels for GTK).
//!
//! ## Math
//!
//! [`fit_thumb`] is the canonical thumb-sizing/positioning formula —
//! every scrollbar geometry type (`Scrollbar`, `PaletteScrollbar`,
//! `PopupScrollbar`, `TerminalScrollbar`, `dispatch::SurfaceScrollbar`)
//! computes its thumb through it, directly or via a thin per-primitive
//! adapter (e.g. `multi_section_view::compute_thumb_bounds` converts a
//! row-indexed scroll position into `fit_thumb`'s continuous units, then
//! `multi_section_view::quantize_thumb`
//! snaps the result to whole cells for TUI).
//!
//! A caller may choose its own `min_thumb_len` value (cells vs. pixels,
//! 1 vs. 8, etc.) — that is a per-surface *policy knob* `fit_thumb`
//! takes as a parameter.
//!
//! ## Idle fade
//!
//! [`idle_fade_opacity`] is the overlay-scrollbar fade curve: full
//! opacity for [`SCROLLBAR_FADE_HOLD`] after the last hover/scroll/drag,
//! then a linear fade to `0.0` over [`SCROLLBAR_FADE_OUT`], built on
//! [`crate::transition::Transition`] so it shares its deterministic,
//! headless-clock-friendly contract (pass a fixed [`std::time::Instant`]
//! captured once at test setup, advance `now` by adding a
//! [`std::time::Duration`]). The function itself only answers "how
//! opaque should the bar be right now" — it does not track *when* a
//! widget was last interacted with; that timestamp is per-widget,
//! backend-driven state (mouse-move/scroll/drag events differ by
//! backend) a host feeds in. [`native_surface_paint::paint_with_opacity`]
//! is the render-side half of the seam: the same track/thumb fill
//! [`native_surface_paint::paint`] already does, scaled by an opacity
//! multiplier a caller computed via [`idle_fade_opacity`].

use crate::event::Rect;
use crate::transition::Transition;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// How long an overlay scrollbar stays fully visible after the last
/// hover/scroll/drag interaction, before [`idle_fade_opacity`] starts
/// fading it out — VS Code holds the bar at full opacity for a beat
/// rather than snapping it invisible the instant activity stops.
pub const SCROLLBAR_FADE_HOLD: Duration = Duration::from_millis(400);

/// How long the fade-out itself takes once [`SCROLLBAR_FADE_HOLD`] elapses.
pub const SCROLLBAR_FADE_OUT: Duration = Duration::from_millis(800);

/// Opacity multiplier (`0.0..=1.0`) for an overlay scrollbar that was
/// last interacted with (hovered, scrolled, or dragged) at
/// `last_interaction`, sampled at `now`.
///
/// Full opacity for [`SCROLLBAR_FADE_HOLD`] after `last_interaction`, then
/// a linear ramp down to `0.0` over the following [`SCROLLBAR_FADE_OUT`] —
/// a thin wrapper around [`Transition::progress_at`] run in reverse
/// (`1.0 - progress`). `now` at or before `last_interaction` returns
/// `1.0` rather than going negative: [`Instant::duration_since`] itself
/// saturates to a zero [`Duration`] when `now` precedes `last_interaction`,
/// which already falls inside the hold window below, so no separate
/// early-return is needed for that case.
///
/// Tests build a deterministic headless clock the same way
/// [`Transition`]'s own doc describes: capture `Instant::now()` once,
/// then advance `now` by adding a [`Duration`] rather than sleeping.
pub fn idle_fade_opacity(last_interaction: Instant, now: Instant) -> f32 {
    if now.duration_since(last_interaction) <= SCROLLBAR_FADE_HOLD {
        return 1.0;
    }
    let fade = Transition::starting_at(last_interaction + SCROLLBAR_FADE_HOLD, SCROLLBAR_FADE_OUT);
    1.0 - fade.progress_at(now)
}

/// Orientation of a scrollbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ScrollAxis {
    Vertical,
    Horizontal,
}

/// Declarative description of a scrollbar.
///
/// Coordinate units in `track`, `thumb_start`, and `thumb_len` are
/// surface-native (TUI cells, GTK pixels). `thumb_start` is an offset
/// from the track's leading edge along `axis`; `thumb_len` is the
/// thumb's length along `axis`.
///
/// # Examples
///
/// ```
/// use quadraui::{PressZone, Rect, Scrollbar};
///
/// let track = Rect::new(79.0, 0.0, 1.0, 24.0);
/// // 100 total lines, 24 visible, scrolled to line 10.
/// let bar = Scrollbar::vertical("scrollbar:editor", track, 10.0, 100.0, 24.0, 2.0);
///
/// assert_eq!(bar.press_zone(0.0), PressZone::Before);
/// // thumb_len = (24.0 / 100.0) * 24.0 = 5.76, so travel is the track
/// // length minus that, not a tautological "it's non-negative" check.
/// assert_eq!(bar.travel(), track.height - bar.thumb_len);
/// ```
///
/// Not `#[non_exhaustive]`: the oracle acceptance suite constructs this
/// struct with an exhaustive literal, and that suite is sealed, so no
/// in-tree caller can migrate off the literal to keep pace with a future
/// field addition. [`Default`] is implemented regardless — it is
/// additive and doesn't require the exhaustiveness trade-off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Scrollbar {
    pub id: WidgetId,
    pub axis: ScrollAxis,
    pub track: Rect,
    pub thumb_start: f32,
    pub thumb_len: f32,
    /// Cursor is hovering over the scrollbar — rasterisers may brighten
    /// the thumb. Default `false`.
    #[serde(default)]
    pub hovered: bool,
    /// User is actively dragging the thumb — rasterisers may apply an
    /// even brighter / fully opaque highlight. Default `false`.
    #[serde(default)]
    pub dragging: bool,
}

impl Scrollbar {
    /// Build a vertical scrollbar with thumb geometry computed via
    /// [`fit_thumb`].
    pub fn vertical(
        id: impl Into<WidgetId>,
        track: Rect,
        scroll: f32,
        total: f32,
        visible: f32,
        min_thumb_len: f32,
    ) -> Self {
        let (thumb_start, thumb_len) =
            fit_thumb(scroll, total, visible, track.height, min_thumb_len);
        Self {
            id: id.into(),
            axis: ScrollAxis::Vertical,
            track,
            thumb_start,
            thumb_len,
            hovered: false,
            dragging: false,
        }
    }

    /// Build a horizontal scrollbar with thumb geometry computed via
    /// [`fit_thumb`].
    pub fn horizontal(
        id: impl Into<WidgetId>,
        track: Rect,
        scroll: f32,
        total: f32,
        visible: f32,
        min_thumb_len: f32,
    ) -> Self {
        let (thumb_start, thumb_len) =
            fit_thumb(scroll, total, visible, track.width, min_thumb_len);
        Self {
            id: id.into(),
            axis: ScrollAxis::Horizontal,
            track,
            thumb_start,
            thumb_len,
            hovered: false,
            dragging: false,
        }
    }
}

impl Default for Scrollbar {
    /// A vertical scrollbar with an empty track and no thumb. See
    /// [`Scrollbar::vertical`] / [`Scrollbar::horizontal`] for a
    /// scrollbar with real thumb geometry.
    fn default() -> Self {
        Self {
            id: WidgetId::new(""),
            axis: ScrollAxis::Vertical,
            track: Rect::default(),
            thumb_start: 0.0,
            thumb_len: 0.0,
            hovered: false,
            dragging: false,
        }
    }
}

/// Canonical scrollbar thumb-fitting math.
///
/// Sizes the thumb proportional to `visible / total`, clamped to
/// `[min_thumb_len, track_len]`. Positions the thumb so it travels the
/// remaining `track_len - thumb_len` linearly with `scroll` over its
/// available range `(total - visible)`. When `total <= visible` the
/// scrollbar has no work to do; both outputs are zero.
///
/// All values are in the same surface units. Backends that need cell-
/// precise rounding (e.g. TUI) typically `.floor()` / `.ceil()` the
/// returned values themselves.
pub fn fit_thumb(
    scroll: f32,
    total: f32,
    visible: f32,
    track_len: f32,
    min_thumb_len: f32,
) -> (f32, f32) {
    if total <= 0.0 || track_len <= 0.0 || visible <= 0.0 || total <= visible {
        return (0.0, 0.0);
    }
    let raw_len = (visible / total) * track_len;
    let thumb_len = raw_len.max(min_thumb_len).min(track_len);
    let scroll_range = (total - visible).max(1.0);
    let travel = (track_len - thumb_len).max(0.0);
    let thumb_start = (scroll / scroll_range).clamp(0.0, 1.0) * travel;
    (thumb_start, thumb_len)
}

/// Where a click landed relative to a [`Scrollbar`]'s painted thumb,
/// along its axis. Returned by [`Scrollbar::press_zone`] so callers can
/// route a scrollbar click into "start a thumb drag" (on the thumb) vs.
/// "page before/after" (on the bare track) without re-deriving
/// `thumb_top`/`thumb_bottom` arithmetic themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressZone {
    /// Before the thumb (page up / left).
    Before,
    /// On the thumb itself — start a drag.
    Thumb,
    /// After the thumb (page down / right).
    After,
}

impl Scrollbar {
    /// Classify a click at `pos` — the coordinate along this scrollbar's
    /// [`ScrollAxis`] (`y` for [`ScrollAxis::Vertical`], `x` for
    /// [`ScrollAxis::Horizontal`]), in the same units as [`Self::track`] —
    /// against the painted thumb.
    pub fn press_zone(&self, pos: f32) -> PressZone {
        let thumb_start = match self.axis {
            ScrollAxis::Vertical => self.track.y + self.thumb_start,
            ScrollAxis::Horizontal => self.track.x + self.thumb_start,
        };
        let thumb_end = thumb_start + self.thumb_len;
        if pos < thumb_start {
            PressZone::Before
        } else if pos < thumb_end {
            PressZone::Thumb
        } else {
            PressZone::After
        }
    }

    /// How far the thumb can travel along the track: `track_len -
    /// thumb_len`, clamped to `>= 0.0`. `track_len` is `track.height`
    /// for a vertical scrollbar, `track.width` for horizontal.
    ///
    /// Feeds [`ThumbDrag::begin`]'s `travel` parameter.
    pub fn travel(&self) -> f32 {
        let track_len = match self.axis {
            ScrollAxis::Vertical => self.track.height,
            ScrollAxis::Horizontal => self.track.width,
        };
        (track_len - self.thumb_len).max(0.0)
    }
}

/// State for an in-progress scrollbar thumb drag.
///
/// Shared by every scroll-owning controller. Offsets are `f32` so this
/// type serves both discrete, row-indexed scroll state (`usize`, round
/// [`Self::offset_at`]'s result) and continuous pixel-based scroll state
/// without duplicating the struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThumbDrag {
    origin_pos: f32,
    origin_offset: f32,
    travel: f32,
    max_offset: f32,
}

impl ThumbDrag {
    /// Start tracking a drag from a thumb click.
    ///
    /// - `origin_pos` — cursor coordinate (along the scroll axis) at
    ///   mouse-down.
    /// - `origin_offset` — the scroll offset at mouse-down.
    /// - `travel` — track pixels/cells the thumb can move over, e.g.
    ///   [`Scrollbar::travel`].
    /// - `max_offset` — the largest reachable scroll offset.
    pub fn begin(origin_pos: f32, origin_offset: f32, travel: f32, max_offset: f32) -> Self {
        Self {
            origin_pos,
            origin_offset,
            travel,
            max_offset,
        }
    }

    /// Resolve the scroll offset for cursor coordinate `pos`, clamped
    /// to `[0, max_offset]`.
    ///
    /// Returns `None` when the drag has no usable range (`travel <=
    /// 0.0` or `max_offset <= 0.0`) — callers should treat that as "no
    /// change" (e.g. return their own `Ignored` event variant).
    ///
    /// The result is not rounded: continuous (pixel) callers use it
    /// directly, discrete (row-indexed) callers round it to `usize`
    /// themselves. Clamping before rounding gives the same integer
    /// result as the reverse order, since `max_offset` is always an
    /// exact integer value for discrete callers.
    pub fn offset_at(&self, pos: f32) -> Option<f32> {
        if self.travel <= 0.0 || self.max_offset <= 0.0 {
            return None;
        }
        let delta = pos - self.origin_pos;
        let scaled = delta / self.travel * self.max_offset;
        Some((self.origin_offset + scaled).clamp(0.0, self.max_offset))
    }

    /// The `max_offset` this drag was started with. Useful for callers
    /// that need to compare a resolved [`Self::offset_at`] result
    /// against the drag's own range after the drag reference has gone
    /// out of scope (e.g. to recompute a "stuck to end" flag).
    pub fn max_offset(&self) -> f32 {
        self.max_offset
    }
}

/// Round a continuous `f32` scroll offset to the whole row a
/// row-indexing surface starts painting from.
///
/// Scroll offsets are stored as `f32` in the primitives' native units so
/// a trackpad or high-resolution wheel can move content by a fraction of
/// a row. Surfaces that can only begin a frame on a whole row — the TUI
/// rasterisers, and any list painter that slices `rows[start..]` — call
/// this once, at the paint boundary, and keep the `f32` field as the
/// source of truth.
///
/// Rounds half away from zero (`2.5 → 3`); negative, `NaN` and infinite
/// offsets resolve to `0`.
pub fn scroll_rows(scroll_offset: f32) -> usize {
    if !scroll_offset.is_finite() || scroll_offset <= 0.0 {
        0
    } else {
        scroll_offset.round() as usize
    }
}

/// Floor a continuous pixel offset to the whole row a `scroll_px` paint
/// path starts painting from.
///
/// This is the same `(px / row_height).floor()` [`resolve_scroll_px`]
/// uses to pick its starting row, extracted so every caller that derives
/// a row index from a pixel offset (a controller accumulating
/// `scroll_px`, a paint path resolving it) agrees on the same rounding
/// direction. Non-finite, non-positive `px`, or a non-finite/non-positive
/// `row_height`, resolves to `0`.
pub fn scroll_rows_floor(px: f32, row_height: f32) -> usize {
    if !row_height.is_finite() || row_height <= 0.0 {
        return 0;
    }
    let row = (px / row_height).floor();
    if row.is_finite() && row > 0.0 {
        row as usize
    } else {
        0
    }
}

/// Clamp `current + delta` into `[0, max]`.
///
/// The core arithmetic behind every `scroll_by` / `page_scroll` operation.
/// `max` is typically `content_len.saturating_sub(viewport_rows)`.
pub fn scroll_by_clamped(current: usize, delta: isize, max: usize) -> usize {
    let cur = current as isize;
    let max = max as isize;
    (cur + delta).clamp(0, max) as usize
}

/// Clamp a (possibly stale) `scroll_offset` into a valid starting index
/// for `content_len` items.
///
/// Returns `0` when there is nothing to scroll to (`content_len == 0`),
/// otherwise `scroll_offset.min(content_len - 1)` — pinning the start so
/// the last item is always reachable instead of scrolling past the end
/// into an empty view.
///
/// Callers that render a fixed number of rows per frame (rather than
/// filling a variable-height viewport item-by-item) usually want
/// [`visible_window`] instead, which layers the end-of-window clamp on
/// top of this.
pub fn clamp_scroll_offset(scroll_offset: usize, content_len: usize) -> usize {
    if content_len == 0 {
        0
    } else {
        scroll_offset.min(content_len - 1)
    }
}

/// Resolve a widget's `(scroll_offset, scroll_px)` pair into a whole
/// starting-row index plus the sub-row pixel remainder a GUI painter
/// uses to clip the first visible row.
///
/// `scroll_px` wins over `scroll_offset` when it is `Some` and usable
/// (finite, non-negative, and `row_height` itself finite and positive);
/// otherwise this is exactly [`clamp_scroll_offset`] with a `0.0`
/// remainder, so a caller that never has a `scroll_px` to pass (`None`)
/// sees no behaviour change at all.
///
/// `row_height` is the caller's uniform row height in its native unit
/// (`1.0` on TUI, `line_height` on pixel backends). Content with
/// per-row-varying heights (`measure_item` returning different values)
/// should pass a representative row height — the same approximation
/// `vscrollbar`/`hscrollbar` already make for thumb sizing — so the
/// conversion is exact for uniform-height content and approximate
/// otherwise.
///
/// The returned `sub_row_px` is `0.0` whenever `px` pushes the floored
/// row index itself past the content's last reachable offset (nothing
/// left to partially scroll past) — e.g. `px` far beyond
/// `content_len * row_height`. It is **not** always `0.0` just because
/// the resolved row happens to equal `content_len - 1`;
/// `resolve_scroll_px(0, Some(4.5), 5, 1.0)` still returns `(4, 0.5)`,
/// since `4` is reachable on its own (no clamping occurred) and `0.5`
/// pixels remain unaccounted for. A TUI rasteriser that only reads the
/// row index and never this remainder gets the pre-existing whole-row
/// behaviour "for free" either way: `scroll_px` degrades to
/// row-quantised there, rather than breaking.
pub fn resolve_scroll_px(
    scroll_offset: usize,
    scroll_px: Option<f32>,
    content_len: usize,
    row_height: f32,
) -> (usize, f32) {
    let px = match scroll_px {
        Some(px) if px.is_finite() && px >= 0.0 && row_height.is_finite() && row_height > 0.0 => px,
        _ => return (clamp_scroll_offset(scroll_offset, content_len), 0.0),
    };
    let row_idx = scroll_rows_floor(px, row_height);
    let clamped = clamp_scroll_offset(row_idx, content_len);
    if clamped == row_idx {
        (clamped, (px - clamped as f32 * row_height).max(0.0))
    } else {
        (clamped, 0.0)
    }
}

/// Continuous counterpart of [`scroll_by_clamped`]: clamp
/// `current + delta` into `[0.0, max]` without quantising to whole rows.
///
/// Controllers that consume a [`ScrollDelta`](crate::ScrollDelta)
/// magnitude use this so a sub-row trackpad gesture accumulates instead
/// of being rounded away on every event. Non-finite inputs resolve to
/// `0.0`.
pub fn scroll_by_clamped_f32(current: f32, delta: f32, max: f32) -> f32 {
    let max = if max.is_finite() { max.max(0.0) } else { 0.0 };
    let next = current + delta;
    if next.is_finite() {
        next.clamp(0.0, max)
    } else {
        0.0
    }
}

/// Continuous counterpart of [`clamp_scroll_offset`]: clamp a (possibly
/// stale) `f32` `scroll_offset` into `[0.0, content_len - 1]`.
///
/// Returns `0.0` when there is nothing to scroll to (`content_len == 0`).
/// Sub-row precision is preserved — use [`scroll_rows`] at the paint
/// boundary to get the row index.
pub fn clamp_scroll_offset_f32(scroll_offset: f32, content_len: usize) -> f32 {
    if content_len == 0 {
        0.0
    } else {
        scroll_by_clamped_f32(scroll_offset, 0.0, (content_len - 1) as f32)
    }
}

/// Convert the signed `y` component of a wheel/trackpad
/// [`ScrollDelta`](crate::ScrollDelta) into a scroll-offset delta in the
/// surface's own units, preserving the gesture's magnitude.
///
/// `units_per_notch` is how far one notch of a classic mouse wheel
/// (`delta.y == ±1.0`) should move the content: `1.0` for a one-row list,
/// `3.0` for a three-rows-per-notch transcript, `line_height` for a
/// pixel-space panel.
///
/// The sign is flipped because positive `delta.y` means "scroll the
/// content up", which *decreases* a scroll offset. Non-finite inputs
/// resolve to `0.0`.
pub fn wheel_delta(delta_y: f32, units_per_notch: f32) -> f32 {
    if delta_y.is_finite() && units_per_notch.is_finite() {
        -delta_y * units_per_notch
    } else {
        0.0
    }
}

/// Whole-row counterpart of [`wheel_delta`], for controllers whose scroll
/// offset is a row index rather than a continuous position.
///
/// Rounds the magnitude half away from zero, but never rounds a non-zero
/// gesture down to nothing: a fractional trackpad delta still moves one
/// row, so slow scrolling is not silently swallowed. A zero (or
/// non-finite) delta returns `0`.
pub fn wheel_row_delta(delta_y: f32, rows_per_notch: f32) -> isize {
    let rows = wheel_delta(delta_y, rows_per_notch);
    if rows == 0.0 {
        return 0;
    }
    let magnitude = rows.abs().round().max(1.0) as isize;
    if rows < 0.0 {
        -magnitude
    } else {
        magnitude
    }
}

/// Visible window `[start, end)` into a `content_len`-item list, given a
/// (possibly stale) `scroll_offset` and how many rows fit in the
/// viewport (`viewport_rows` — typically `(body_len / row_step).floor()`,
/// computed by the caller since the step size is surface- and
/// primitive-specific).
///
/// `start` is [`clamp_scroll_offset`]; `end` is `start + viewport_rows`
/// clamped to `content_len`. Returns `(0, 0)` when `content_len == 0`.
pub fn visible_window(
    scroll_offset: usize,
    content_len: usize,
    viewport_rows: usize,
) -> (usize, usize) {
    if content_len == 0 {
        return (0, 0);
    }
    let start = clamp_scroll_offset(scroll_offset, content_len);
    let end = (start + viewport_rows).min(content_len);
    (start, end)
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────
//
// `Scrollbar` reaches the frame type-erased as
// [`crate::frame::Surface::Widget`]: `layout` resolves the painted track
// and thumb into a [`ScrollbarLayout`], `paint` runs the shared pixel
// painter in [`native_surface_paint`], `paint_cells` runs the cell
// painter in [`cell_paint`], and `handle` owns the three gestures a bar
// answers — thumb drag, track page, wheel.
//
// All three resolve to a *fraction of the thumb's travel*
// ([`UiEvent::ScrollbarMoved`]), not an item index: the descriptor is
// geometry-only (track, `thumb_start`, `thumb_len`) and deliberately
// carries no content length or viewport size, so the host that owns that
// extent maps the fraction onto its own scroll state.

/// Fraction of one viewport a single wheel notch moves the thumb.
///
/// A viewport, in track units, *is* the thumb's own length
/// ([`Scrollbar::thumb_len`] is sized `visible / total` of the track by
/// [`fit_thumb`]), so a notch is expressible without knowing the content
/// extent. The quarter-viewport step is the policy knob
/// [`wheel_delta`]'s `units_per_notch` parameter takes elsewhere, fixed
/// here because a bar has nothing else to scale it against.
pub const SCROLLBAR_WHEEL_VIEWPORTS_PER_NOTCH: f32 = 0.25;

/// Fully-resolved [`Scrollbar`] geometry: the painted track plus the
/// thumb rect inside it, both absolute.
///
/// Built by [`crate::primitive::Primitive::layout`] from the rect the
/// host painted the bar into, so the thumb a
/// [`Self::press_zone`] classifies against is exactly the thumb the
/// painters drew.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarLayout {
    /// Which coordinate axis the thumb travels along.
    pub axis: ScrollAxis,
    /// The whole track's bounds.
    pub track: Rect,
    /// The thumb's bounds inside [`Self::track`].
    pub thumb: Rect,
}

impl ScrollbarLayout {
    /// Resolve `sb`'s geometry for a track painted at `track`.
    ///
    /// `sb.thumb_start` / `sb.thumb_len` are already in the track's own
    /// units, measured from its leading edge, so this only has to place
    /// them along `axis`.
    pub fn resolve(sb: &Scrollbar, track: Rect) -> Self {
        let thumb = match sb.axis {
            ScrollAxis::Vertical => {
                Rect::new(track.x, track.y + sb.thumb_start, track.width, sb.thumb_len)
            }
            ScrollAxis::Horizontal => Rect::new(
                track.x + sb.thumb_start,
                track.y,
                sb.thumb_len,
                track.height,
            ),
        };
        Self {
            axis: sb.axis,
            track,
            thumb,
        }
    }

    /// The track's extent along [`Self::axis`].
    pub fn track_len(&self) -> f32 {
        match self.axis {
            ScrollAxis::Vertical => self.track.height,
            ScrollAxis::Horizontal => self.track.width,
        }
    }

    /// The thumb's extent along [`Self::axis`].
    pub fn thumb_len(&self) -> f32 {
        match self.axis {
            ScrollAxis::Vertical => self.thumb.height,
            ScrollAxis::Horizontal => self.thumb.width,
        }
    }

    /// The thumb's leading edge in **widget-local** units — the same
    /// space [`crate::interactive::RoutedEvent::Pointer`]'s `local`
    /// field uses, so [`Self::press_zone`] needs no absolute
    /// coordinates.
    pub fn local_thumb_start(&self) -> f32 {
        match self.axis {
            ScrollAxis::Vertical => self.thumb.y - self.track.y,
            ScrollAxis::Horizontal => self.thumb.x - self.track.x,
        }
    }

    /// How far the thumb can travel: `track_len - thumb_len`, floored at
    /// `0.0`.
    pub fn travel(&self) -> f32 {
        (self.track_len() - self.thumb_len()).max(0.0)
    }

    /// The component of a widget-local point that lies along
    /// [`Self::axis`].
    pub fn axis_pos(&self, local: crate::Point) -> f32 {
        match self.axis {
            ScrollAxis::Vertical => local.y,
            ScrollAxis::Horizontal => local.x,
        }
    }

    /// Classify a widget-local point against the painted thumb — the
    /// [`Scrollbar::press_zone`] of a resolved layout.
    ///
    /// Both the zone and the thumb come from the same resolved layout,
    /// so a press can never be classified against a thumb other than
    /// the one the painters drew.
    pub fn press_zone(&self, local: crate::Point) -> PressZone {
        let thumb_start = self.local_thumb_start();
        let pos = self.axis_pos(local);
        if pos < thumb_start {
            PressZone::Before
        } else if pos < thumb_start + self.thumb_len() {
            PressZone::Thumb
        } else {
            PressZone::After
        }
    }

    /// Where a thumb leading edge of `thumb_start` (in track units)
    /// sits as a fraction of [`Self::travel`], clamped to `0.0..=1.0`.
    /// A bar with no travel is fully scrolled by definition and reports
    /// `0.0`.
    pub fn fraction_at(&self, thumb_start: f32) -> f32 {
        let travel = self.travel();
        if travel <= 0.0 || !thumb_start.is_finite() {
            return 0.0;
        }
        (thumb_start / travel).clamp(0.0, 1.0)
    }
}

/// [`Scrollbar`]'s interaction state — the hover and in-flight-drag
/// bookkeeping a renderer derives, kept here rather than on the
/// geometry-only descriptor.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScrollbarState {
    /// Whether the pointer is inside the track, so a hover transition
    /// asks for exactly one redraw instead of one per pointer event.
    pub hovered: bool,
    /// The live thumb drag, started by a press on
    /// [`PressZone::Thumb`] and ended on button-up.
    pub drag: Option<ThumbDrag>,
}

/// The semantic event a [`Scrollbar`] gesture resolves to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollbarEvent {
    /// The thumb's leading edge moved to `fraction` of its travel —
    /// becomes [`UiEvent::ScrollbarMoved`].
    Moved { fraction: f32 },
}

impl crate::interactive::WidgetAction for ScrollbarEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            ScrollbarEvent::Moved { fraction } => crate::UiEvent::ScrollbarMoved {
                widget: id,
                fraction,
            },
        }
    }
}

impl Scrollbar {
    /// Resolve a requested thumb leading edge (in track units) to a
    /// [`ScrollbarEvent::Moved`] fraction, clamped to the track's
    /// travel.
    ///
    /// Nothing about the move is remembered: the host owns the scroll
    /// offset the fraction maps onto, and the next gesture reads the
    /// thumb position back off the rebuilt descriptor, so a bar driven
    /// by the keyboard or by its region's own scrolling can never
    /// disagree with the position it was painted at.
    fn move_thumb(layout: &ScrollbarLayout, thumb_start: f32) -> crate::Handled<ScrollbarEvent> {
        let clamped = thumb_start.clamp(0.0, layout.travel());
        crate::Handled::Event(ScrollbarEvent::Moved {
            fraction: layout.fraction_at(clamped),
        })
    }

    /// Record the pointer entering or leaving the track, asking for a
    /// redraw only when that actually changed.
    fn set_hovered(
        state: &mut ScrollbarState,
        hovered: bool,
        cx: &mut crate::interactive::EventCx,
    ) {
        if state.hovered != hovered {
            state.hovered = hovered;
            cx.request_redraw();
        }
    }
}

impl crate::primitive::Primitive for Scrollbar {
    type Layout = ScrollbarLayout;
    type Event = PressZone;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Scrollbar(self.clone())
    }

    /// As thin as the track it was given on the cross axis, stretching
    /// along its own: a scrollbar's length is its container's, and its
    /// thickness is the gutter the host reserved.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let thickness = match ctx.units() {
            crate::primitive::Units::Cells => 1.0,
            _ => self.track_thickness().max(ctx.char_width),
        };
        match self.axis {
            ScrollAxis::Vertical => crate::NaturalSize::new(Some(thickness), None),
            ScrollAxis::Horizontal => crate::NaturalSize::new(None, Some(thickness)),
        }
    }

    /// `rect` is the track — every host paints a bar into the gutter it
    /// reserved for it, which is what [`Scrollbar::track`] names.
    fn layout(&self, _ctx: &crate::primitive::LayoutCtx, rect: Rect) -> ScrollbarLayout {
        ScrollbarLayout::resolve(self, rect)
    }

    /// The shared pixel painter, against the track `layout` resolved, at
    /// the surface's overlay idle-fade opacity
    /// ([`crate::PaintSurface::surface_overlay_scrollbar_opacity`]): an
    /// idle bar is hidden until the user scrolls. A bar under the
    /// pointer or being dragged paints at full opacity.
    fn paint(
        &self,
        layout: &ScrollbarLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let engaged = self.hovered || self.dragging || env.is_hovered(&self.id);
            let opacity = if engaged {
                1.0
            } else {
                surface.surface_overlay_scrollbar_opacity()
            };
            native_surface_paint::paint_with_opacity(
                &self.on_track(layout.track),
                surface,
                theme,
                opacity,
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme);
    }

    /// [`cell_paint::paint`] over `cells`.
    ///
    /// Paints against [`crate::Theme::background`] — the cell background
    /// a vertical bar in a reserved gutter sits on. A host that needs
    /// the bar to blend into a differently-coloured row calls
    /// [`crate::tui::draw_scrollbar`] with its own `cell_bg` instead.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ScrollbarLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        cell_paint::paint(layout, cells, theme, theme.background);
    }
}

impl Scrollbar {
    /// The gutter thickness this bar paints across — `track.width` for a
    /// vertical bar, `track.height` for a horizontal one.
    fn track_thickness(&self) -> f32 {
        match self.axis {
            ScrollAxis::Vertical => self.track.width,
            ScrollAxis::Horizontal => self.track.height,
        }
    }

    /// This bar with its track replaced by `track` — what the shared
    /// pixel painter, which reads geometry off the descriptor rather
    /// than off a layout, is handed so it paints exactly the track
    /// [`crate::primitive::Primitive::layout`] resolved.
    ///
    /// Gated with [`native_surface_paint`], its only caller: the cell
    /// painter takes a [`ScrollbarLayout`] directly and needs no such
    /// rebind.
    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    fn on_track(&self, track: Rect) -> Self {
        Self {
            track,
            ..self.clone()
        }
    }
}

impl crate::interactive::Interactive for Scrollbar {
    type State = ScrollbarState;
    type Action = ScrollbarEvent;

    /// A scrollbar is chrome, reachable with the pointer and the wheel
    /// but never a keyboard tab stop — the scrollable region it drives
    /// is the focus stop, and that region's own arrow/page keys scroll
    /// it.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::None
    }

    /// - **Press on the thumb** captures the pointer and opens a
    ///   [`ThumbDrag`] whose offsets are the thumb's own leading edge in
    ///   track units, so a following move resolves straight to a
    ///   fraction of [`ScrollbarLayout::travel`].
    /// - **Press on the bare track** pages by one viewport — the
    ///   thumb's own length — toward the press.
    /// - **Move** while a drag is live resolves the new thumb position;
    ///   with no drag it only tracks hover.
    /// - **Up** releases the capture.
    /// - **Wheel** moves by
    ///   [`SCROLLBAR_WHEEL_VIEWPORTS_PER_NOTCH`] of a viewport per
    ///   notch.
    ///
    /// Every gesture that is not a live drag measures from
    /// [`ScrollbarLayout::local_thumb_start`] — the thumb `layout`
    /// resolved for *this* event — never from a position a previous
    /// gesture resolved. The host owns the scroll offset the emitted
    /// fraction maps onto and rebuilds the descriptor before the next
    /// event, so measuring off the layout is what keeps a bar in step
    /// with a region scrolled by the keyboard or by the host itself.
    fn handle(
        &self,
        state: &mut ScrollbarState,
        event: &crate::interactive::RoutedEvent,
        layout: &ScrollbarLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<ScrollbarEvent> {
        use crate::interactive::{PointerKind, RoutedEvent};
        use crate::Handled;

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Enter,
                ..
            } => {
                Self::set_hovered(state, true, cx);
                Handled::Consumed
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Leave,
                ..
            } => {
                Self::set_hovered(state, false, cx);
                Handled::Consumed
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => {
                let pos = layout.axis_pos(*local);
                let thumb_start = layout.local_thumb_start();
                match layout.press_zone(*local) {
                    PressZone::Thumb => {
                        cx.capture_pointer();
                        state.drag = Some(ThumbDrag::begin(
                            pos,
                            thumb_start,
                            layout.travel(),
                            layout.travel(),
                        ));
                        Handled::Consumed
                    }
                    PressZone::Before => Self::move_thumb(layout, thumb_start - layout.thumb_len()),
                    PressZone::After => Self::move_thumb(layout, thumb_start + layout.thumb_len()),
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } => match state.drag {
                Some(drag) => match drag.offset_at(layout.axis_pos(*local)) {
                    Some(thumb_start) => Self::move_thumb(layout, thumb_start),
                    None => Handled::Consumed,
                },
                None => {
                    Self::set_hovered(state, true, cx);
                    Handled::Ignored
                }
            },
            RoutedEvent::Pointer {
                kind: PointerKind::Up(crate::MouseButton::Left),
                ..
            } => {
                if state.drag.take().is_some() {
                    cx.release_pointer();
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            RoutedEvent::Wheel { dy, .. } => {
                let step = wheel_delta(
                    *dy,
                    layout.thumb_len() * SCROLLBAR_WHEEL_VIEWPORTS_PER_NOTCH,
                );
                if step == 0.0 {
                    return Handled::Ignored;
                }
                Self::move_thumb(layout, layout.local_thumb_start() + step)
            }
            _ => Handled::Ignored,
        }
    }
}

/// The cell-grid painter for [`Scrollbar`], written against
/// [`crate::CellSurface`].
///
/// A vertical bar fills one column with full-cell glyphs (`█` thumb, `░`
/// track). A horizontal bar fills one row with half-block glyphs (`▄`
/// thumb, `▁` track) so it sits at the bottom of its row without
/// obscuring the content above.
///
/// `cell_bg` is the background behind both glyphs: a vertical bar in a
/// reserved gutter passes [`crate::Theme::background`]; a horizontal bar
/// at the bottom of a window with a coloured active background passes
/// that background so the row blends into the area above.
///
/// [`Scrollbar::hovered`] / [`Scrollbar::dragging`] are not reflected
/// here — a terminal has no alpha to brighten the thumb with, and the
/// active state reads from the surrounding chrome instead.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{ScrollAxis, ScrollbarLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;

    /// An opaque single-cell style — every glyph this painter writes
    /// sets both colours and no modifier.
    fn solid(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint the track and thumb `layout` resolved onto `surface`.
    ///
    /// Cell coordinates come from `layout.track` (treated as
    /// cell-precise integers via `.round()`); the thumb's extent is
    /// floored at one whole cell so a thumb shorter than a cell still
    /// shows.
    pub(crate) fn paint(
        layout: &ScrollbarLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        cell_bg: Color,
    ) {
        let track_style = solid(theme.scrollbar_track, cell_bg);
        let thumb_style = solid(theme.scrollbar_thumb, cell_bg);

        let track = layout.track;
        let thumb_start = layout.local_thumb_start().floor() as u16;
        let thumb_size = layout.thumb_len().ceil().max(1.0) as u16;

        match layout.axis {
            ScrollAxis::Vertical => {
                let track_h = track.height.round() as u16;
                if track_h == 0 || track.width <= 0.0 {
                    return;
                }
                let x = track.x.round() as u16;
                let y0 = track.y.round() as u16;
                for dy in 0..track_h {
                    let in_thumb = dy >= thumb_start && dy < thumb_start.saturating_add(thumb_size);
                    let (ch, style) = if in_thumb {
                        ('█', thumb_style)
                    } else {
                        ('░', track_style)
                    };
                    surface.put_char(x, y0 + dy, ch, style);
                }
            }
            ScrollAxis::Horizontal => {
                let track_w = track.width.round() as u16;
                if track_w == 0 || track.height <= 0.0 {
                    return;
                }
                let x0 = track.x.round() as u16;
                let y = track.y.round() as u16;
                for dx in 0..track_w {
                    let in_thumb = dx >= thumb_start && dx < thumb_start.saturating_add(thumb_size);
                    let (ch, style) = if in_thumb {
                        ('▄', thumb_style)
                    } else {
                        ('▁', track_style)
                    };
                    surface.put_char(x0 + dx, y, ch, style);
                }
            }
        }
    }
}

// This is the one shared implementation of scrollbar painting, written
// against [`crate::paint_surface::PaintSurface`] instead of any one
// backend's drawing API.
//
// Track and thumb must carry real alpha (`Color::a < 255`), never a
// fully-opaque colour pre-blended against `theme.background`.
//
// `#[allow(dead_code)]`: only called when a real pixel backend is
// compiled in, exercised by each backend's own `Backend::draw_scrollbar`
// call site plus tests.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ScrollAxis, Scrollbar};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::Rect;

    /// Paint a [`Scrollbar`] onto `surface`: a translucent track with a
    /// brighter, higher-contrast translucent thumb on top (pill-shaped,
    /// rounded to half its shorter side), both bumping alpha on
    /// hover/drag. See this module's doc for the one known divergence
    /// re-verified (already fixed) while unifying three
    /// per-backend copies into this one.
    ///
    /// Equivalent to [`paint_with_opacity`] at `opacity = 1.0` — callers
    /// that don't yet track an idle-fade timestamp (see
    /// [`crate::primitives::scrollbar::idle_fade_opacity`]) keep calling
    /// this.
    pub(crate) fn paint(scrollbar: &Scrollbar, surface: &mut dyn PaintSurface, theme: &Theme) {
        paint_with_opacity(scrollbar, surface, theme, 1.0);
    }

    /// Like [`paint`], but scales every fill's alpha by `opacity`
    /// (`0.0..=1.0`) on top of the hover/drag weighting — the render-side
    /// half of the overlay idle-fade seam
    /// [`crate::primitives::scrollbar::idle_fade_opacity`] computes.
    /// Any `opacity` below `1.0 / 255.0` paints nothing at all, the same
    /// early return as a zero-size track: below that threshold even the
    /// track's own (lowest-weighted, `>= 0.20`) alpha already rounds to
    /// `0` in [`crate::types::Color::with_alpha`], so an idle-faded bar
    /// this close to invisible costs no draw calls.
    pub(crate) fn paint_with_opacity(
        scrollbar: &Scrollbar,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        opacity: f32,
    ) {
        let track = scrollbar.track;
        if track.width <= 0.0 || track.height <= 0.0 || opacity < 1.0 / 255.0 {
            return;
        }
        let opacity = opacity.min(1.0) as f64;

        let track_alpha = if scrollbar.hovered || scrollbar.dragging {
            0.35
        } else {
            0.20
        };
        // Thumb alpha floors at 0.65 even at rest — below that the thumb
        // reads as barely narrower than the track instead of a distinct
        // control, at the visual weight VS Code's default theme carries.
        let thumb_alpha = if scrollbar.dragging {
            0.90
        } else if scrollbar.hovered {
            0.80
        } else {
            0.65
        };

        surface.surface_fill_rect(
            track,
            theme.scrollbar_track.with_alpha(track_alpha * opacity),
        );

        let thumb_rect = match scrollbar.axis {
            ScrollAxis::Vertical => Rect::new(
                track.x,
                track.y + scrollbar.thumb_start,
                track.width,
                scrollbar.thumb_len,
            ),
            ScrollAxis::Horizontal => Rect::new(
                track.x + scrollbar.thumb_start,
                track.y,
                scrollbar.thumb_len,
                track.height,
            ),
        };
        let thumb_radius = thumb_rect.width.min(thumb_rect.height) / 2.0;
        surface.surface_fill_rounded_rect(
            thumb_rect,
            thumb_radius,
            theme.scrollbar_thumb.with_alpha(thumb_alpha * opacity),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every `surface_fill_rect` / `surface_fill_rounded_rect`
        /// call — mirrors `primitives::chart`'s `RecordingSurface` test
        /// double, scoped to just the verbs this primitive uses, so this
        /// test runs on any host without Cairo/Core Graphics/Direct2D.
        ///
        /// `fills` holds every fill regardless of verb, `(rect, color)`,
        /// in call order — `paint`'s track-then-thumb order means index 1
        /// is always the thumb. `rounded_radii` records just the radius
        /// passed to `surface_fill_rounded_rect` calls, in the same
        /// relative order, so a test can assert the thumb painted with a
        /// real corner radius without caring about its exact pixel value.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            rounded_radii: Vec<f32>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
                (0.0, 0.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
                self.fills.push((rect, color));
                self.rounded_radii.push(radius);
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn vertical_bar(scroll: f32, total: f32, visible: f32) -> Scrollbar {
            Scrollbar::vertical(
                WidgetId::new("sb"),
                Rect::new(0.0, 0.0, 8.0, 200.0),
                scroll,
                total,
                visible,
                20.0,
            )
        }

        /// Track and thumb must carry real alpha (`Color::a < 255`), not a
        /// fully-opaque colour pre-blended against `theme.background`.
        #[test]
        fn track_and_thumb_paint_with_real_alpha_not_opaque() {
            let sb = vertical_bar(0.0, 200.0, 50.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&sb, &mut surface, &theme);

            assert_eq!(
                surface.fills.len(),
                2,
                "expected exactly a track fill and a thumb fill"
            );
            for (_, color) in &surface.fills {
                assert!(
                    color.a < 255,
                    "scrollbar fill must carry real alpha, got opaque a={}",
                    color.a
                );
            }
        }

        /// The thumb must read as a distinct control against its track even
        /// at rest, not only on hover/drag — regression coverage for the
        /// too-translucent-to-read-as-14px-wide complaint: resting alpha
        /// floors at 0.65 (`166/255`) and the thumb paints with a real
        /// corner radius rather than a sharp-cornered `surface_fill_rect`.
        #[test]
        fn resting_thumb_has_high_contrast_rounded_fill() {
            let sb = vertical_bar(0.0, 200.0, 50.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&sb, &mut surface, &theme);

            let (thumb_rect, thumb_color) = surface.fills[1];
            assert!(
                thumb_color.a >= 166,
                "resting thumb alpha should floor at ~0.65, got a={}",
                thumb_color.a
            );
            assert_eq!(
                surface.rounded_radii.len(),
                1,
                "thumb should paint via surface_fill_rounded_rect, not a sharp-cornered fill"
            );
            assert!(
                surface.rounded_radii[0] > 0.0,
                "thumb corner radius should be positive"
            );
            assert!(
                surface.rounded_radii[0] <= thumb_rect.width.min(thumb_rect.height) / 2.0 + 0.01
            );
        }

        #[test]
        fn dragging_increases_thumb_alpha() {
            let mut sb = vertical_bar(0.0, 200.0, 50.0);
            let theme = Theme::default();

            let mut normal = RecordingSurface::default();
            paint(&sb, &mut normal, &theme);
            let normal_thumb_alpha = normal.fills[1].1.a;

            sb.dragging = true;
            let mut dragging = RecordingSurface::default();
            paint(&sb, &mut dragging, &theme);
            let dragging_thumb_alpha = dragging.fills[1].1.a;

            assert!(
                dragging_thumb_alpha > normal_thumb_alpha,
                "dragging should raise thumb alpha: normal={normal_thumb_alpha}, dragging={dragging_thumb_alpha}"
            );
        }

        #[test]
        fn zero_size_track_paints_nothing() {
            let sb = Scrollbar::vertical(
                WidgetId::new("sb"),
                Rect::new(0.0, 0.0, 0.0, 0.0),
                0.0,
                200.0,
                50.0,
                20.0,
            );
            let mut surface = RecordingSurface::default();
            paint(&sb, &mut surface, &Theme::default());
            assert!(surface.fills.is_empty());
        }

        #[test]
        fn horizontal_thumb_rect_uses_width_axis() {
            let track = Rect::new(0.0, 50.0, 100.0, 8.0);
            let sb = Scrollbar::horizontal(WidgetId::new("h"), track, 0.0, 200.0, 40.0, 10.0);
            let mut surface = RecordingSurface::default();
            paint(&sb, &mut surface, &Theme::default());
            let (thumb_rect, _) = surface.fills[1];
            assert_eq!(thumb_rect.y, track.y);
            assert_eq!(thumb_rect.height, track.height);
            assert!(thumb_rect.width > 0.0 && thumb_rect.width <= track.width);
        }

        /// Regression lock for the VS Code overlay-scrollbar convention:
        /// the thumb paints *inside* the track column (same x/width for a
        /// vertical bar), not as a separate column beside it — the track
        /// itself paints at exactly the width the caller gave it (e.g.
        /// `Style::scrollbar_width`'s 10px default), so no extra width is
        /// added for the thumb.
        #[test]
        fn thumb_rect_is_contained_within_track_rect_at_the_given_track_width() {
            let track = Rect::new(0.0, 0.0, 10.0, 200.0);
            let sb = Scrollbar::vertical(WidgetId::new("sb"), track, 50.0, 200.0, 50.0, 20.0);
            let mut surface = RecordingSurface::default();
            paint(&sb, &mut surface, &Theme::default());

            let (track_rect, _) = surface.fills[0];
            let (thumb_rect, _) = surface.fills[1];

            assert_eq!(
                track_rect.width, 10.0,
                "track paints at exactly the given width, not a wider column"
            );
            assert!(
                thumb_rect.x >= track_rect.x
                    && thumb_rect.x + thumb_rect.width <= track_rect.x + track_rect.width,
                "thumb must sit inside the track horizontally, not beside it: \
                 track={track_rect:?}, thumb={thumb_rect:?}"
            );
            assert!(
                thumb_rect.y >= track_rect.y
                    && thumb_rect.y + thumb_rect.height <= track_rect.y + track_rect.height,
                "thumb must sit inside the track vertically: track={track_rect:?}, thumb={thumb_rect:?}"
            );
        }

        #[test]
        fn zero_opacity_paints_nothing() {
            let sb = vertical_bar(0.0, 200.0, 50.0);
            let mut surface = RecordingSurface::default();
            paint_with_opacity(&sb, &mut surface, &Theme::default(), 0.0);
            assert!(
                surface.fills.is_empty(),
                "a fully faded-out scrollbar should paint no fills"
            );
        }

        #[test]
        fn partial_opacity_scales_both_fills_alpha_down() {
            let sb = vertical_bar(0.0, 200.0, 50.0);
            let theme = Theme::default();

            let mut full = RecordingSurface::default();
            paint_with_opacity(&sb, &mut full, &theme, 1.0);

            let mut half = RecordingSurface::default();
            paint_with_opacity(&sb, &mut half, &theme, 0.5);

            assert_eq!(full.fills.len(), half.fills.len());
            for ((_, full_color), (_, half_color)) in full.fills.iter().zip(half.fills.iter()) {
                assert!(
                    half_color.a < full_color.a,
                    "half opacity should paint a dimmer fill: full a={}, half a={}",
                    full_color.a,
                    half_color.a
                );
            }
        }

        #[test]
        fn paint_is_full_opacity_paint_with_opacity() {
            let sb = vertical_bar(0.0, 200.0, 50.0);
            let theme = Theme::default();

            let mut via_paint = RecordingSurface::default();
            paint(&sb, &mut via_paint, &theme);

            let mut via_opacity = RecordingSurface::default();
            paint_with_opacity(&sb, &mut via_opacity, &theme, 1.0);

            assert_eq!(via_paint.fills, via_opacity.fills);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_thumb_zero_range_returns_zero() {
        assert_eq!(fit_thumb(0.0, 0.0, 100.0, 200.0, 1.0), (0.0, 0.0));
        assert_eq!(fit_thumb(0.0, 100.0, 100.0, 200.0, 1.0), (0.0, 0.0));
        assert_eq!(fit_thumb(0.0, 100.0, 200.0, 200.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn fit_thumb_proportional_size() {
        // 20% of total visible → 20% of track length.
        let (start, len) = fit_thumb(0.0, 100.0, 20.0, 200.0, 1.0);
        assert_eq!(start, 0.0);
        assert!((len - 40.0).abs() < 0.01);
    }

    #[test]
    fn fit_thumb_min_length_applied() {
        let (_start, len) = fit_thumb(0.0, 1000.0, 1.0, 100.0, 10.0);
        assert!(len >= 10.0);
    }

    #[test]
    fn fit_thumb_clamped_to_track() {
        let (_start, len) = fit_thumb(0.0, 100.0, 50.0, 30.0, 100.0);
        assert!(len <= 30.0);
    }

    #[test]
    fn fit_thumb_full_scroll_aligns_to_track_end() {
        // scroll = total - visible should put thumb at the end.
        let (start, len) = fit_thumb(80.0, 100.0, 20.0, 200.0, 1.0);
        assert!((start + len - 200.0).abs() < 0.01);
    }

    #[test]
    fn fit_thumb_clamps_overscroll() {
        // scroll past max should still place thumb at track end.
        let (start, len) = fit_thumb(500.0, 100.0, 20.0, 200.0, 1.0);
        assert!((start + len - 200.0).abs() < 0.01);
    }

    #[test]
    fn fit_thumb_horizontal_uses_width_units() {
        let (start, len) = fit_thumb(50.0, 200.0, 100.0, 400.0, 1.0);
        assert!((len - 200.0).abs() < 0.01);
        assert!(start >= 0.0 && start + len <= 400.0);
    }

    #[test]
    fn vertical_constructor_uses_track_height() {
        let track = Rect::new(10.0, 20.0, 8.0, 100.0);
        let sb = Scrollbar::vertical("v", track, 0.0, 200.0, 50.0, 5.0);
        assert!(matches!(sb.axis, ScrollAxis::Vertical));
        assert_eq!(sb.track.height, 100.0);
        assert!((sb.thumb_len - 25.0).abs() < 0.01);
    }

    #[test]
    fn horizontal_constructor_uses_track_width() {
        let track = Rect::new(0.0, 0.0, 100.0, 4.0);
        let sb = Scrollbar::horizontal("h", track, 0.0, 200.0, 100.0, 5.0);
        assert!(matches!(sb.axis, ScrollAxis::Horizontal));
        assert_eq!(sb.track.width, 100.0);
        assert!((sb.thumb_len - 50.0).abs() < 0.01);
    }

    #[test]
    fn clamp_scroll_offset_empty_content_returns_zero() {
        assert_eq!(clamp_scroll_offset(0, 0), 0);
        assert_eq!(clamp_scroll_offset(50, 0), 0);
    }

    #[test]
    fn clamp_scroll_offset_in_range_is_unchanged() {
        assert_eq!(clamp_scroll_offset(0, 10), 0);
        assert_eq!(clamp_scroll_offset(5, 10), 5);
        assert_eq!(clamp_scroll_offset(9, 10), 9);
    }

    #[test]
    fn clamp_scroll_offset_pins_to_last_item() {
        // Scrolling past the end still resolves to the last valid index
        // rather than an empty view.
        assert_eq!(clamp_scroll_offset(10, 10), 9);
        assert_eq!(clamp_scroll_offset(1000, 10), 9);
    }

    #[test]
    fn clamp_scroll_offset_single_item() {
        assert_eq!(clamp_scroll_offset(0, 1), 0);
        assert_eq!(clamp_scroll_offset(50, 1), 0);
    }

    #[test]
    fn resolve_scroll_px_none_falls_back_to_scroll_offset() {
        assert_eq!(resolve_scroll_px(3, None, 10, 22.0), (3, 0.0));
    }

    #[test]
    fn resolve_scroll_px_sub_row_remainder_on_a_pixel_surface() {
        // 1.5 rows deep into 22px rows: row 1, 11px leftover for a GUI
        // painter to clip off the top of the first visible row.
        let (row, sub) = resolve_scroll_px(0, Some(33.0), 50, 22.0);
        assert_eq!(row, 1);
        assert!((sub - 11.0).abs() < 0.001);
    }

    #[test]
    fn resolve_scroll_px_quantises_to_whole_rows_on_tui() {
        // row_height == 1.0 cell: fractional pixels still resolve to a
        // whole row index with the TUI rasteriser never reading `sub`.
        let (row, sub) = resolve_scroll_px(0, Some(3.7), 50, 1.0);
        assert_eq!(row, 3);
        assert!((sub - 0.7).abs() < 0.001);
    }

    #[test]
    fn resolve_scroll_px_clamped_at_end_has_no_remainder() {
        // Scrolled past the last reachable row: clamp like
        // `clamp_scroll_offset`, with no partial row left to show.
        let (row, sub) = resolve_scroll_px(0, Some(10_000.0), 5, 22.0);
        assert_eq!(row, 4);
        assert_eq!(sub, 0.0);
    }

    #[test]
    fn resolve_scroll_px_landing_exactly_on_the_last_row_still_has_a_remainder() {
        // `px` resolves to row 4 (the content's last row) on its own,
        // with no clamping involved — unlike the test above, this is
        // *not* "scrolled past the end", so the 0.5-row leftover must
        // still be reported rather than zeroed out.
        let (row, sub) = resolve_scroll_px(0, Some(4.5), 5, 1.0);
        assert_eq!(row, 4);
        assert!((sub - 0.5).abs() < 0.001);
    }

    #[test]
    fn resolve_scroll_px_non_finite_or_non_positive_row_height_falls_back() {
        assert_eq!(resolve_scroll_px(2, Some(50.0), 10, 0.0), (2, 0.0));
        assert_eq!(resolve_scroll_px(2, Some(50.0), 10, -1.0), (2, 0.0));
        assert_eq!(resolve_scroll_px(2, Some(f32::NAN), 10, 22.0), (2, 0.0));
        assert_eq!(resolve_scroll_px(2, Some(-5.0), 10, 22.0), (2, 0.0));
    }

    #[test]
    fn visible_window_empty_content_is_zero_zero() {
        assert_eq!(visible_window(0, 0, 5), (0, 0));
        assert_eq!(visible_window(50, 0, 5), (0, 0));
    }

    #[test]
    fn visible_window_fits_entirely_in_viewport() {
        // 5 items, 10-row viewport — the whole list is visible from 0.
        assert_eq!(visible_window(0, 5, 10), (0, 5));
    }

    #[test]
    fn visible_window_slices_a_middle_page() {
        assert_eq!(visible_window(3, 20, 5), (3, 8));
    }

    #[test]
    fn visible_window_clamps_end_to_content_len() {
        assert_eq!(visible_window(18, 20, 5), (18, 20));
    }

    #[test]
    fn visible_window_overscroll_still_shows_last_item() {
        // Scrolled well past the end: pins to the last item rather than
        // returning an empty window.
        assert_eq!(visible_window(1000, 20, 5), (19, 20));
    }

    #[test]
    fn visible_window_zero_viewport_rows_is_empty() {
        assert_eq!(visible_window(3, 20, 0), (3, 3));
    }

    // ── press_zone / travel ─────────────────────────────────────────

    #[test]
    fn press_zone_before_on_after_thumb() {
        // track 0..200, thumb spans [50, 90).
        let sb = Scrollbar::vertical("v", Rect::new(0.0, 0.0, 8.0, 200.0), 40.0, 200.0, 40.0, 1.0);
        assert_eq!(sb.thumb_start, 40.0);
        assert!((sb.thumb_len - 40.0).abs() < 0.01);
        assert_eq!(sb.press_zone(10.0), PressZone::Before);
        assert_eq!(sb.press_zone(sb.thumb_start), PressZone::Thumb);
        assert_eq!(
            sb.press_zone(sb.thumb_start + sb.thumb_len - 1.0),
            PressZone::Thumb
        );
        assert_eq!(
            sb.press_zone(sb.thumb_start + sb.thumb_len),
            PressZone::After
        );
    }

    #[test]
    fn press_zone_uses_x_axis_for_horizontal() {
        let track = Rect::new(0.0, 50.0, 100.0, 8.0);
        let sb = Scrollbar::horizontal("h", track, 0.0, 200.0, 40.0, 10.0);
        assert_eq!(sb.press_zone(0.0), PressZone::Thumb);
        assert_eq!(
            sb.press_zone(track.x + sb.thumb_len + 1.0),
            PressZone::After
        );
    }

    #[test]
    fn travel_is_track_len_minus_thumb_len() {
        let sb = Scrollbar::vertical("v", Rect::new(0.0, 0.0, 8.0, 200.0), 0.0, 200.0, 40.0, 1.0);
        assert!((sb.travel() - (200.0 - sb.thumb_len)).abs() < 0.01);
    }

    #[test]
    fn travel_never_negative() {
        // min_thumb_len larger than track clamps thumb_len to track_len.
        let sb = Scrollbar::vertical(
            "v",
            Rect::new(0.0, 0.0, 8.0, 50.0),
            0.0,
            200.0,
            40.0,
            1000.0,
        );
        assert_eq!(sb.travel(), 0.0);
    }

    // ── ThumbDrag ────────────────────────────────────────────────────

    #[test]
    fn thumb_drag_no_movement_returns_origin_offset() {
        let drag = ThumbDrag::begin(100.0, 5.0, 80.0, 20.0);
        assert_eq!(drag.offset_at(100.0), Some(5.0));
    }

    #[test]
    fn thumb_drag_moves_proportionally() {
        // travel=80 maps to max_offset=20 units, so moving 40 (half the
        // travel) should move the offset by 10 (half of max_offset).
        let drag = ThumbDrag::begin(0.0, 0.0, 80.0, 20.0);
        assert_eq!(drag.offset_at(40.0), Some(10.0));
    }

    #[test]
    fn thumb_drag_clamps_to_range() {
        let drag = ThumbDrag::begin(0.0, 0.0, 80.0, 20.0);
        assert_eq!(drag.offset_at(-1000.0), Some(0.0));
        assert_eq!(drag.offset_at(1000.0), Some(20.0));
    }

    #[test]
    fn thumb_drag_zero_travel_or_max_offset_is_none() {
        assert_eq!(ThumbDrag::begin(0.0, 0.0, 0.0, 20.0).offset_at(50.0), None);
        assert_eq!(ThumbDrag::begin(0.0, 0.0, 80.0, 0.0).offset_at(50.0), None);
    }

    // ── scroll_by_clamped ────────────────────────────────────────────

    #[test]
    fn scroll_by_clamped_moves_within_range() {
        assert_eq!(scroll_by_clamped(5, 3, 20), 8);
        assert_eq!(scroll_by_clamped(5, -3, 20), 2);
    }

    #[test]
    fn scroll_by_clamped_clamps_to_bounds() {
        assert_eq!(scroll_by_clamped(5, -100, 20), 0);
        assert_eq!(scroll_by_clamped(5, 100, 20), 20);
    }

    // ── scroll_rows ───────────────────────────────────────────

    #[test]
    fn scroll_rows_rounds_sub_row_offsets_to_the_nearest_row() {
        assert_eq!(scroll_rows(0.0), 0);
        assert_eq!(scroll_rows(0.4), 0);
        assert_eq!(scroll_rows(0.5), 1);
        assert_eq!(scroll_rows(2.49), 2);
        assert_eq!(scroll_rows(2.5), 3);
        assert_eq!(scroll_rows(7.0), 7);
    }

    #[test]
    fn scroll_rows_floors_degenerate_offsets_to_zero() {
        assert_eq!(scroll_rows(-3.0), 0);
        assert_eq!(scroll_rows(f32::NAN), 0);
        assert_eq!(scroll_rows(f32::INFINITY), 0);
        assert_eq!(scroll_rows(f32::NEG_INFINITY), 0);
    }

    // ── f32 clamping ─────────────────────────────────────────

    #[test]
    fn scroll_by_clamped_f32_preserves_sub_row_precision() {
        assert_eq!(scroll_by_clamped_f32(1.25, 0.5, 20.0), 1.75);
        assert_eq!(scroll_by_clamped_f32(1.25, -0.5, 20.0), 0.75);
    }

    #[test]
    fn scroll_by_clamped_f32_clamps_to_bounds() {
        assert_eq!(scroll_by_clamped_f32(5.0, -100.0, 20.0), 0.0);
        assert_eq!(scroll_by_clamped_f32(5.0, 100.0, 20.0), 20.0);
        assert_eq!(scroll_by_clamped_f32(5.0, 1.0, -4.0), 0.0);
        assert_eq!(scroll_by_clamped_f32(5.0, f32::NAN, 20.0), 0.0);
    }

    #[test]
    fn clamp_scroll_offset_f32_pins_the_last_item_reachable() {
        assert_eq!(clamp_scroll_offset_f32(2.5, 0), 0.0);
        assert_eq!(clamp_scroll_offset_f32(2.5, 10), 2.5);
        assert_eq!(clamp_scroll_offset_f32(40.0, 10), 9.0);
        assert_eq!(clamp_scroll_offset_f32(-1.5, 10), 0.0);
    }

    // ── wheel deltas ─────────────────────────────────────────

    #[test]
    fn wheel_delta_flips_sign_and_scales_by_units_per_notch() {
        assert_eq!(wheel_delta(1.0, 1.0), -1.0);
        assert_eq!(wheel_delta(-1.0, 1.0), 1.0);
        assert_eq!(wheel_delta(-2.5, 3.0), 7.5);
        assert_eq!(wheel_delta(0.25, 16.0), -4.0);
    }

    #[test]
    fn wheel_delta_resolves_non_finite_input_to_zero() {
        assert_eq!(wheel_delta(f32::NAN, 1.0), 0.0);
        assert_eq!(wheel_delta(f32::INFINITY, 1.0), 0.0);
        assert_eq!(wheel_delta(1.0, f32::NAN), 0.0);
    }

    #[test]
    fn wheel_row_delta_carries_the_gesture_magnitude_not_just_its_sign() {
        assert_eq!(wheel_row_delta(1.0, 1.0), -1);
        assert_eq!(wheel_row_delta(-1.0, 1.0), 1);
        assert_eq!(wheel_row_delta(-4.0, 1.0), 4);
        assert_eq!(wheel_row_delta(5.0, 1.0), -5);
        assert_eq!(wheel_row_delta(-2.0, 3.0), 6);
    }

    #[test]
    fn wheel_row_delta_never_swallows_a_sub_row_gesture() {
        assert_eq!(wheel_row_delta(0.1, 1.0), -1);
        assert_eq!(wheel_row_delta(-0.1, 1.0), 1);
        assert_eq!(wheel_row_delta(0.0, 1.0), 0);
        assert_eq!(wheel_row_delta(f32::NAN, 1.0), 0);
    }

    // ── idle_fade_opacity ────────────────────────────────────────────
    //
    // Every test builds its own headless clock the way `Transition`'s
    // own doc describes: capture `Instant::now()` once, then advance
    // `now` by adding a `Duration` — never `std::thread::sleep`.

    #[test]
    fn idle_fade_opacity_is_full_during_the_hold_window() {
        let last = Instant::now();
        assert_eq!(idle_fade_opacity(last, last), 1.0);
        assert_eq!(idle_fade_opacity(last, last + SCROLLBAR_FADE_HOLD), 1.0);
    }

    #[test]
    fn idle_fade_opacity_ramps_down_linearly_after_the_hold_window() {
        let last = Instant::now();
        let mid = last + SCROLLBAR_FADE_HOLD + SCROLLBAR_FADE_OUT / 2;
        let opacity = idle_fade_opacity(last, mid);
        assert!(
            (opacity - 0.5).abs() < 1e-3,
            "expected ~0.5 opacity halfway through the fade, got {opacity}"
        );
    }

    #[test]
    fn idle_fade_opacity_reaches_zero_after_the_full_idle_interval() {
        let last = Instant::now();
        let after = last + SCROLLBAR_FADE_HOLD + SCROLLBAR_FADE_OUT;
        assert_eq!(idle_fade_opacity(last, after), 0.0);
        // Staying idle even longer doesn't go negative.
        assert_eq!(
            idle_fade_opacity(last, after + Duration::from_secs(10)),
            0.0
        );
    }

    #[test]
    fn idle_fade_opacity_clamps_to_full_when_now_precedes_last_interaction() {
        let last = Instant::now() + Duration::from_secs(1);
        assert_eq!(idle_fade_opacity(last, Instant::now()), 1.0);
    }
}

/// Direct-`Interactive::handle` coverage for [`Scrollbar`] — the dispatch
/// shape an out-of-process binding host takes when it calls
/// `DynWidget::handle` itself, rather than through a backend's routing
/// engine. The runtime leg of the same behaviour lives in
/// `tests/widget_contract_scrollbar.rs`; both must resolve the same
/// gesture to the same [`crate::UiEvent::ScrollbarMoved`] fraction.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::event::Point;
    use crate::interactive::{
        EventCx, EventHost, Interactive, PointerKind, RoutedEvent, SubTarget, WidgetAction,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::{Handled, MouseButton, PointerShape};

    /// Minimal [`EventHost`] mock — the same shape
    /// `crate::primitives::tab_bar`'s own module tests use, plus a
    /// capture log, since a thumb drag's whole contract is "capture on
    /// press, release on button-up".
    #[derive(Debug, Default)]
    struct MockHost {
        captures: Vec<WidgetId>,
        releases: usize,
        emitted: Vec<crate::UiEvent>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}
        fn capture_pointer(&mut self, id: &WidgetId) {
            self.captures.push(*id);
        }
        fn release_pointer(&mut self) {
            self.releases += 1;
        }
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, event: crate::UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    /// A vertical bar on a 1×20 cell track showing a quarter of its
    /// content from the top: `thumb_len` 5, `thumb_start` 0, so travel
    /// is 15 cells.
    fn vertical_bar() -> Scrollbar {
        Scrollbar::vertical(
            "sb:test",
            Rect::new(79.0, 3.0, 1.0, 20.0),
            0.0,
            80.0,
            20.0,
            1.0,
        )
    }

    /// [`vertical_bar`] repainted with its thumb `thumb_start` track
    /// cells down — what a host rebuilds the descriptor as once it has
    /// applied a [`ScrollbarEvent::Moved`], or scrolled the region by
    /// any other means.
    fn vertical_bar_at(thumb_start: f32) -> Scrollbar {
        Scrollbar {
            thumb_start,
            ..vertical_bar()
        }
    }

    fn layout_of(sb: &Scrollbar) -> ScrollbarLayout {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        Primitive::layout(sb, &ctx, sb.track)
    }

    /// Route one event and report both the resolved action and the
    /// host's capture bookkeeping.
    fn route(
        sb: &Scrollbar,
        state: &mut ScrollbarState,
        layout: &ScrollbarLayout,
        event: RoutedEvent,
    ) -> (Handled<ScrollbarEvent>, MockHost) {
        let mut host = MockHost::default();
        let handled = {
            let mut cx = EventCx::new(sb.id, &mut host);
            Interactive::handle(sb, state, &event, layout, &mut cx)
        };
        (handled, host)
    }

    fn down_at(y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(0.0, y),
            mods: Default::default(),
        }
    }

    fn move_to(y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind: PointerKind::Move,
            sub: None,
            local: Point::new(0.0, y),
            mods: Default::default(),
        }
    }

    fn up_at(y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind: PointerKind::Up(MouseButton::Left),
            sub: None,
            local: Point::new(0.0, y),
            mods: Default::default(),
        }
    }

    fn fraction(handled: &Handled<ScrollbarEvent>) -> f32 {
        match handled {
            Handled::Event(ScrollbarEvent::Moved { fraction }) => *fraction,
            other => panic!("expected a Moved action, got {other:?}"),
        }
    }

    #[test]
    fn layout_places_the_thumb_inside_the_track_it_was_given() {
        let sb = vertical_bar();
        let layout = layout_of(&sb);
        assert_eq!(layout.track, sb.track);
        assert_eq!(layout.thumb.x, sb.track.x);
        assert_eq!(layout.thumb.y, sb.track.y + sb.thumb_start);
        assert_eq!(layout.thumb.height, sb.thumb_len);
        assert_eq!(layout.travel(), 20.0 - sb.thumb_len);
    }

    #[test]
    fn press_zone_classifies_local_coordinates_against_the_painted_thumb() {
        let layout = layout_of(&vertical_bar());
        // The thumb sits at the top of the track, so local y below its
        // length is on it and anything past that is after it.
        assert_eq!(layout.press_zone(Point::new(0.0, 0.0)), PressZone::Thumb);
        assert_eq!(layout.press_zone(Point::new(0.0, 19.0)), PressZone::After);
    }

    #[test]
    fn pressing_the_thumb_captures_the_pointer_without_moving_it() {
        let sb = vertical_bar();
        let layout = layout_of(&sb);
        let mut state = ScrollbarState::default();
        let (handled, host) = route(&sb, &mut state, &layout, down_at(1.0));
        // `Consumed` rather than an `Event`: a press on the thumb opens
        // the drag without scrolling anything yet.
        assert!(matches!(handled, Handled::Consumed), "{handled:?}");
        assert_eq!(host.captures, vec![sb.id]);
        assert!(state.drag.is_some());
    }

    #[test]
    fn dragging_the_thumb_to_the_end_of_the_track_resolves_a_full_fraction() {
        let sb = vertical_bar();
        let layout = layout_of(&sb);
        let mut state = ScrollbarState::default();
        let _ = route(&sb, &mut state, &layout, down_at(0.0));
        let (handled, _) = route(&sb, &mut state, &layout, move_to(layout.travel()));
        assert_eq!(fraction(&handled), 1.0);

        // Dragging back past the start clamps rather than going negative.
        let (handled, _) = route(&sb, &mut state, &layout, move_to(-40.0));
        assert_eq!(fraction(&handled), 0.0);
    }

    #[test]
    fn releasing_ends_the_drag_and_the_capture() {
        let sb = vertical_bar();
        let layout = layout_of(&sb);
        let mut state = ScrollbarState::default();
        let _ = route(&sb, &mut state, &layout, down_at(0.0));
        let (handled, host) = route(&sb, &mut state, &layout, up_at(5.0));
        assert!(matches!(handled, Handled::Consumed), "{handled:?}");
        assert_eq!(host.releases, 1);
        assert!(state.drag.is_none());

        // A second release has no drag to end and is left to the host.
        let (handled, host) = route(&sb, &mut state, &layout, up_at(5.0));
        assert!(matches!(handled, Handled::Ignored), "{handled:?}");
        assert_eq!(host.releases, 0);
    }

    #[test]
    fn a_move_with_no_live_drag_only_tracks_hover() {
        let sb = vertical_bar();
        let layout = layout_of(&sb);
        let mut state = ScrollbarState::default();
        let (handled, _) = route(&sb, &mut state, &layout, move_to(10.0));
        assert!(matches!(handled, Handled::Ignored), "{handled:?}");
        assert!(state.hovered);
    }

    #[test]
    fn pressing_the_bare_track_pages_by_one_viewport_toward_the_press() {
        let sb = vertical_bar();
        let layout = layout_of(&sb);
        let travel = layout.travel();
        let page = layout.thumb_len();

        let mut state = ScrollbarState::default();
        let (handled, host) = route(&sb, &mut state, &layout, down_at(19.0));
        assert_eq!(fraction(&handled), page / travel);
        assert!(
            host.captures.is_empty(),
            "a track page is not a drag — no capture"
        );

        // A second page accumulates from the thumb the host repainted
        // after applying the first.
        let sb = vertical_bar_at(page);
        let layout = layout_of(&sb);
        let (handled, _) = route(&sb, &mut state, &layout, down_at(19.0));
        assert_eq!(fraction(&handled), 2.0 * page / travel);

        // Paging back from there returns to where the first page left
        // the thumb.
        let sb = vertical_bar_at(2.0 * page);
        let layout = layout_of(&sb);
        let (handled, _) = route(&sb, &mut state, &layout, down_at(0.0));
        assert_eq!(fraction(&handled), page / travel);
    }

    /// The descriptor — not anything the widget remembers — decides
    /// where the thumb is. A host that scrolls the region by some other
    /// means (its arrow/page keys, a jump-to-match) rebuilds the bar
    /// with a new `thumb_start`, and the next gesture must measure from
    /// *that*, or a press just above the painted thumb pages the wrong
    /// way and a thumb grab jumps.
    #[test]
    fn a_scroll_between_two_gestures_is_measured_from_the_repainted_thumb() {
        let sb = vertical_bar();
        let layout = layout_of(&sb);
        let travel = layout.travel();
        let page = layout.thumb_len();
        let mut state = ScrollbarState::default();

        // Gesture one: a wheel notch from the top of the track.
        let notch = page * SCROLLBAR_WHEEL_VIEWPORTS_PER_NOTCH;
        let (handled, _) = route(
            &sb,
            &mut state,
            &layout,
            RoutedEvent::Wheel {
                dx: 0.0,
                dy: -1.0,
                local: Point::new(0.0, 5.0),
                mods: Default::default(),
            },
        );
        assert_eq!(fraction(&handled), notch / travel);

        // The host now scrolls the region to the bottom on its own, so
        // the thumb is repainted at the end of its travel.
        let sb = vertical_bar_at(travel);
        let layout = layout_of(&sb);
        assert_eq!(layout.local_thumb_start(), travel);

        // A press above the painted thumb pages *back*, against the
        // repainted position — not forward against the wheel notch.
        let (handled, _) = route(&sb, &mut state, &layout, down_at(0.0));
        assert_eq!(fraction(&handled), (travel - page) / travel);

        // And a press on the painted thumb grabs it there: dragging one
        // page back from the grab point lands exactly one page back.
        let (handled, host) = route(&sb, &mut state, &layout, down_at(travel + 1.0));
        assert!(matches!(handled, Handled::Consumed), "{handled:?}");
        assert_eq!(host.captures, vec![sb.id]);
        let (handled, _) = route(&sb, &mut state, &layout, move_to(travel + 1.0 - page));
        assert_eq!(fraction(&handled), (travel - page) / travel);
    }

    #[test]
    fn a_wheel_notch_moves_a_fraction_of_a_viewport_and_flips_sign() {
        let sb = vertical_bar();
        let layout = layout_of(&sb);
        let travel = layout.travel();
        let notch = layout.thumb_len() * SCROLLBAR_WHEEL_VIEWPORTS_PER_NOTCH;

        // Positive dy means "scroll the content up", which at the top of
        // the track has nowhere to go and clamps to 0.
        let mut state = ScrollbarState::default();
        let (handled, _) = route(
            &sb,
            &mut state,
            &layout,
            RoutedEvent::Wheel {
                dx: 0.0,
                dy: 1.0,
                local: Point::new(0.0, 5.0),
                mods: Default::default(),
            },
        );
        assert_eq!(fraction(&handled), 0.0);

        let mut state = ScrollbarState::default();
        let (handled, _) = route(
            &sb,
            &mut state,
            &layout,
            RoutedEvent::Wheel {
                dx: 0.0,
                dy: -1.0,
                local: Point::new(0.0, 5.0),
                mods: Default::default(),
            },
        );
        assert_eq!(fraction(&handled), notch / travel);
    }

    #[test]
    fn a_zero_delta_wheel_gesture_is_left_to_the_host() {
        let sb = vertical_bar();
        let layout = layout_of(&sb);
        let mut state = ScrollbarState::default();
        let (handled, _) = route(
            &sb,
            &mut state,
            &layout,
            RoutedEvent::Wheel {
                dx: 0.0,
                dy: 0.0,
                local: Point::new(0.0, 5.0),
                mods: Default::default(),
            },
        );
        assert!(matches!(handled, Handled::Ignored), "{handled:?}");
    }

    #[test]
    fn a_bar_with_no_travel_reports_no_movement() {
        // Content shorter than the viewport: `fit_thumb` zeroes both
        // thumb values, so there is nothing to drag.
        let sb = Scrollbar::vertical(
            "sb:short",
            Rect::new(0.0, 0.0, 1.0, 20.0),
            0.0,
            5.0,
            20.0,
            1.0,
        );
        let layout = layout_of(&sb);
        assert_eq!(layout.travel(), 20.0);
        assert_eq!(layout.fraction_at(0.0), 0.0);

        let mut state = ScrollbarState::default();
        // Every local y is "after" a zero-length thumb, so this pages —
        // by a zero-length viewport, which resolves to no movement.
        let (handled, _) = route(&sb, &mut state, &layout, down_at(10.0));
        assert_eq!(fraction(&handled), 0.0);
    }

    #[test]
    fn a_moved_action_becomes_a_scrollbar_moved_ui_event() {
        let id = WidgetId::new("sb:test");
        let event = ScrollbarEvent::Moved { fraction: 0.5 }.into_ui_event(id);
        assert_eq!(
            event,
            crate::UiEvent::ScrollbarMoved {
                widget: id,
                fraction: 0.5
            }
        );
    }

    /// A horizontal bar classifies and drags along `x`, not `y` — the
    /// axis is the only thing that differs from the vertical case.
    #[test]
    fn a_horizontal_bar_resolves_gestures_along_its_own_axis() {
        let sb = Scrollbar::horizontal(
            "sb:h",
            Rect::new(0.0, 23.0, 40.0, 1.0),
            0.0,
            160.0,
            40.0,
            1.0,
        );
        let layout = layout_of(&sb);
        assert_eq!(layout.thumb.y, sb.track.y);
        assert_eq!(layout.thumb.width, sb.thumb_len);
        assert_eq!(
            layout.press_zone(Point::new(39.0, 0.0)),
            PressZone::After,
            "a press past the thumb's right edge pages forward"
        );

        let mut state = ScrollbarState::default();
        let _ = route(
            &sb,
            &mut state,
            &layout,
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(1.0, 0.0),
                mods: Default::default(),
            },
        );
        let (handled, _) = route(
            &sb,
            &mut state,
            &layout,
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                sub: None,
                local: Point::new(1.0 + layout.travel(), 0.0),
                mods: Default::default(),
            },
        );
        assert_eq!(fraction(&handled), 1.0);
    }
}
