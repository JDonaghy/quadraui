//! Shared vertical row-stacking layout math for `TreeView` and `ListView`.
//!
//! Both primitives lay out a flat sequence of rows top-to-bottom from a
//! `scroll_offset` (optionally overridden by a pixel-precise `scroll_px`),
//! clip the last visible row to the viewport, and expose a vertical
//! `Scrollbar` whose thumb tracks the same scroll position. This module
//! factors that shared arithmetic out of `tree.rs` / `list.rs` so a future
//! row-stacking primitive gets it for free, and so the two existing ones
//! can never drift out of sync on the underlying math.
//!
//! What's *not* here: hit-region construction (`TreeView` splits a row
//! into `Chevron`/`Row` zones; `ListView` doesn't), border/title insets
//! (`ListView`-only), and the public `*Layout`/`*Hit` types themselves —
//! those stay primitive-specific because their shapes genuinely differ.

use crate::event::Rect;
use crate::primitives::scrollbar::{resolve_scroll_px, Scrollbar};
use crate::types::WidgetId;

/// Resolved position of one row in a [`RowStackLayout`]: an index into
/// the caller's own row collection, its full (viewport-clipped) bounds,
/// and whatever per-row extra data the caller's `measure` closure
/// produced alongside the height (e.g. `TreeView`'s chevron boundary;
/// `()` for `ListView`, which has none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleRow<E> {
    pub idx: usize,
    pub bounds: Rect,
    pub extra: E,
}

/// Result of [`layout_row_stack`]: the rows that ended up at least
/// partially visible, plus the scroll resolution both `TreeView::layout`
/// and `ListView::layout` report back on their own `*Layout` structs.
#[derive(Debug, Clone, PartialEq)]
pub struct RowStackLayout<E> {
    /// Visible rows, top to bottom.
    pub visible: Vec<VisibleRow<E>>,
    /// Scroll offset actually used. Clamped to `[0, total_rows)` so the
    /// caller never iterates past the end of its row slice.
    pub resolved_scroll_offset: usize,
    /// Sub-row pixel remainder when `scroll_px` is in effect — see
    /// [`resolve_scroll_px`]. `0.0` when `scroll_px` is `None`.
    pub scroll_sub_px: f32,
}

/// Lay out `total_rows` rows stacked vertically from `y_start`, stopping
/// at `y_end`, each positioned at `x` with width `width` and a height
/// from `measure(i) -> (height, extra)`.
///
/// Resolves `scroll_offset` vs. `scroll_px` exactly as
/// [`TreeView::layout_with_scroll_px`](crate::primitives::tree::TreeView::layout_with_scroll_px)
/// and
/// [`ListView::layout_with_scroll_px`](crate::primitives::list::ListView::layout_with_scroll_px)
/// did inline before this extraction: `scroll_px` wins when set and
/// usable (via [`resolve_scroll_px`]), using `measure(0).0` as the
/// uniform row-height basis for the pixel conversion — read only when
/// `scroll_px` is actually in play, since `measure` may do real
/// text-measurement work on a GUI backend.
///
/// The last visible row's `bounds.height` is clipped to whatever fits
/// inside `y_end`; the loop still advances `y` by the row's *unclipped*
/// height, so a clipped row is always the last one pushed (the next
/// iteration's `y >= y_end` check breaks immediately).
#[allow(clippy::too_many_arguments)]
pub fn layout_row_stack<F, E>(
    scroll_offset: usize,
    scroll_px: Option<f32>,
    total_rows: usize,
    x: f32,
    y_start: f32,
    y_end: f32,
    width: f32,
    mut measure: F,
) -> RowStackLayout<E>
where
    F: FnMut(usize) -> (f32, E),
{
    let row_height_basis = if scroll_px.is_none() || total_rows == 0 {
        1.0
    } else {
        measure(0).0
    };
    let (resolved_scroll_offset, scroll_sub_px) =
        resolve_scroll_px(scroll_offset, scroll_px, total_rows, row_height_basis);

    let mut visible = Vec::new();
    let mut y = y_start - scroll_sub_px;
    for i in resolved_scroll_offset..total_rows {
        if y >= y_end {
            break;
        }
        let (row_height, extra) = measure(i);
        let remaining = y_end - y;
        let height = row_height.min(remaining).max(0.0);
        if height <= 0.0 {
            break;
        }
        visible.push(VisibleRow {
            idx: i,
            bounds: Rect::new(x, y, width, height),
            extra,
        });
        y += row_height;
    }

    RowStackLayout {
        visible,
        resolved_scroll_offset,
        scroll_sub_px,
    }
}

/// Shared vertical-scrollbar thumb geometry: resolve `scroll_offset` /
/// `scroll_px` against `total_rows`, convert to a continuous row
/// position, and build the [`Scrollbar::vertical`] thumb — the tail end
/// both `TreeView::vscrollbar_with_scroll_px` and
/// `ListView::vscrollbar_with_scroll_px` perform once their own
/// track-geometry (border insets, title row) is resolved.
///
/// `visible` is how many whole rows fit in the track; `row_height` also
/// serves as the minimum thumb length, matching both callers.
pub fn vscrollbar_thumb(
    id: impl Into<WidgetId>,
    track: Rect,
    scroll_offset: usize,
    scroll_px: Option<f32>,
    total_rows: usize,
    row_height: f32,
    visible: f32,
) -> Scrollbar {
    let (resolved_scroll_offset, scroll_sub_px) =
        resolve_scroll_px(scroll_offset, scroll_px, total_rows, row_height);
    let scroll_rows = resolved_scroll_offset as f32 + scroll_sub_px / row_height;
    Scrollbar::vertical(
        id,
        track,
        scroll_rows,
        total_rows as f32,
        visible,
        row_height,
    )
}

/// Per-frame paint-time state shared by `TreeView` and `ListView`:
/// hover/press row tracking plus a pixel-precise scroll position, none
/// of which live on either primitive's declarative descriptor itself —
/// both are consumer-constructed via full struct literals
/// (`PRIMITIVE_RULES.md` rule 8), so paint-only state that a required
/// field would make breaking lives here instead.
///
/// [`crate::primitives::tree::TreePaintState`] and
/// [`crate::primitives::list::ListPaintState`] are type aliases for this
/// struct — the two were field-identical before this extraction, so
/// aliasing them is not a behaviour change.
///
/// `#[non_exhaustive]`: construct via [`Default::default`] plus the
/// `with_*` builders, so a future field here is additive.
///
/// `scroll_px: None` paints exactly as each primitive's `layout` /
/// `vscrollbar` already do (whole-row from `scroll_offset`); `Some(px)`
/// wins and is resolved via [`resolve_scroll_px`] into a `scroll_offset`
/// plus a sub-row remainder — see [`layout_row_stack`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RowPaintState {
    pub hovered_idx: Option<usize>,
    pub pressed_idx: Option<usize>,
    pub scroll_px: Option<f32>,
}

impl RowPaintState {
    pub fn with_hovered_idx(mut self, idx: Option<usize>) -> Self {
        self.hovered_idx = idx;
        self
    }

    pub fn with_pressed_idx(mut self, idx: Option<usize>) -> Self {
        self.pressed_idx = idx;
        self
    }

    pub fn with_scroll_px(mut self, px: Option<f32>) -> Self {
        self.scroll_px = px;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_row_stack_fills_the_viewport_from_scroll_offset() {
        let layout = layout_row_stack(2, None, 10, 0.0, 0.0, 5.0, 20.0, |_| (1.0, ()));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.scroll_sub_px, 0.0);
        let idxs: Vec<usize> = layout.visible.iter().map(|r| r.idx).collect();
        assert_eq!(idxs, vec![2, 3, 4, 5, 6]);
        assert_eq!(layout.visible[0].bounds, Rect::new(0.0, 0.0, 20.0, 1.0));
    }

    #[test]
    fn layout_row_stack_clips_the_last_row_to_the_viewport() {
        let layout = layout_row_stack(0, None, 3, 0.0, 0.0, 2.5, 20.0, |_| (1.0, ()));
        assert_eq!(layout.visible.len(), 3);
        assert_eq!(layout.visible[2].bounds.height, 0.5);
    }

    #[test]
    fn layout_row_stack_scroll_px_shifts_rows_by_the_sub_row_remainder() {
        let layout = layout_row_stack(0, Some(5.0), 10, 0.0, 0.0, 30.0, 20.0, |_| (2.0, ()));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.scroll_sub_px, 1.0);
        assert_eq!(layout.visible[0].bounds.y, -1.0);
    }

    #[test]
    fn layout_row_stack_carries_per_row_extra_data() {
        let layout = layout_row_stack(0, None, 3, 0.0, 0.0, 10.0, 20.0, |i| (1.0, i * 2));
        let extras: Vec<usize> = layout.visible.iter().map(|r| r.extra).collect();
        assert_eq!(extras, vec![0, 2, 4]);
    }

    #[test]
    fn layout_row_stack_empty_rows_is_empty() {
        let layout = layout_row_stack(0, None, 0, 0.0, 0.0, 10.0, 20.0, |_| (1.0, ()));
        assert!(layout.visible.is_empty());
        assert_eq!(layout.resolved_scroll_offset, 0);
    }

    #[test]
    fn vscrollbar_thumb_scroll_px_wins_over_scroll_offset() {
        let track = Rect::new(0.0, 0.0, 1.0, 10.0);
        let via_px = vscrollbar_thumb("rs", track, 0, Some(5.0), 20, 1.0, 10.0);
        let via_offset = vscrollbar_thumb("rs", track, 5, None, 20, 1.0, 10.0);
        assert_eq!(via_px.thumb_start, via_offset.thumb_start);
    }

    #[test]
    fn row_paint_state_builders_set_fields() {
        let state = RowPaintState::default()
            .with_hovered_idx(Some(1))
            .with_pressed_idx(Some(2))
            .with_scroll_px(Some(3.0));
        assert_eq!(state.hovered_idx, Some(1));
        assert_eq!(state.pressed_idx, Some(2));
        assert_eq!(state.scroll_px, Some(3.0));
    }
}
