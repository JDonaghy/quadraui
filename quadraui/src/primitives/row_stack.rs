//! Shared vertical row-stacking layout math for `TreeView` and `ListView`.
//!
//! Both primitives lay out a flat sequence of rows top-to-bottom from a
//! `scroll_offset` (optionally overridden by a pixel-precise `scroll_px`),
//! clip the last visible row to the viewport, and expose a vertical
//! `Scrollbar` whose thumb tracks the same scroll position. This module
//! is the single home for that shared arithmetic, so `tree.rs` and
//! `list.rs` cannot drift out of sync on it and a future row-stacking
//! primitive gets it for free.
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
    /// Scroll offset actually used. Clamped to `[0, total_rows)` (or `0`
    /// when `total_rows == 0`) so the caller never iterates past the end
    /// of its row slice.
    pub resolved_scroll_offset: usize,
    /// Sub-row pixel remainder when `scroll_px` is in effect — see
    /// [`resolve_scroll_px`]. `0.0` when `scroll_px` is `None`.
    pub scroll_sub_px: f32,
}

/// Lay out `total_rows` rows stacked vertically from `y_start`, stopping
/// at `y_end`, each positioned at `x` with width `width` and a height
/// from `measure(i) -> (height, extra)`.
///
/// Scroll resolution for both
/// [`TreeView::layout_with_scroll_px`](crate::primitives::tree::TreeView::layout_with_scroll_px)
/// and
/// [`ListView::layout_with_scroll_px`](crate::primitives::list::ListView::layout_with_scroll_px):
/// `scroll_px` wins when set and usable (via [`resolve_scroll_px`]),
/// using `measure(0).0` as the uniform row-height basis for the pixel
/// conversion — read only when `scroll_px` is actually in play, since
/// `measure` may do real text-measurement work on a GUI backend.
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
///
/// `row_height <= 0.0` is treated as "no sub-row remainder to apply"
/// rather than dividing by it: both callers already guard `row_height >
/// 0.0` before reaching here, but this keeps the function itself free
/// of a `NaN` result for any other caller.
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
    let scroll_rows = resolved_scroll_offset as f32
        + if row_height > 0.0 {
            scroll_sub_px / row_height
        } else {
            0.0
        };
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
/// struct: both primitives track exactly these three fields.
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

// ── Shared icon + detail row-content painting ───────────────────────────
//
// `ListView::native_surface_paint` and `Palette::native_surface_paint`
// each painted a leading icon glyph and a right-aligned "detail" span
// with their own, independently-maintained copy of the same
// measure/position arithmetic. The two below are the shared
// implementation; each primitive's main-text treatment (`ListView`'s
// per-span styling + ellipsis truncation vs. `Palette`'s match-position
// highlighting) stays primitive-specific — those genuinely differ, not
// just by accident of two authors.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod row_content_paint {
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::types::{Color, Icon, StyledText};

    /// Paint a row's optional leading icon glyph (`ListItem::icon` /
    /// `PaletteItem::icon`) at `cursor_x`, nerd-font glyph vs. ASCII
    /// fallback per `nerd_fonts_enabled`, and return the cursor position
    /// advanced past it (plus a 6-unit trailing gap) — unchanged
    /// (`cursor_x` as given) when there is no icon.
    ///
    /// `draw` is the actual `PaintSurface` verb to paint with — callers
    /// keep their own choice: `ListView` paints the glyph via
    /// `surface_draw_text_run` (plain text in the row's decoration
    /// colour); `Palette` paints it via `surface_draw_icon_glyph` (the
    /// dedicated icon-font path). `fg` is whatever colour that call
    /// should use — `ListView` passes its per-row `decoration_fg`;
    /// `Palette` passes the flat `theme.surface_fg` (its items carry no
    /// decoration).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_row_icon(
        surface: &mut dyn PaintSurface,
        icon: Option<&Icon>,
        nerd_fonts_enabled: bool,
        fg: Color,
        row_y: f32,
        row_h: f32,
        cursor_x: f32,
        draw: impl FnOnce(&mut dyn PaintSurface, Rect, &str, Color),
    ) -> f32 {
        let Some(icon) = icon else {
            return cursor_x;
        };
        let glyph = if nerd_fonts_enabled {
            icon.glyph.as_str()
        } else {
            icon.fallback.as_str()
        };
        let (iw, ih) = surface.surface_measure_text(glyph);
        draw(
            surface,
            Rect::new(cursor_x, row_y + (row_h - ih) / 2.0, iw, ih),
            glyph,
            fg,
        );
        cursor_x + iw + 6.0
    }

    /// A row's optional right-aligned "detail" text (`ListItem::detail` /
    /// `PaletteItem::detail`), measured ahead of painting the row's main
    /// content so the caller knows how much trailing width to reserve.
    pub(crate) struct RowDetail {
        pub text: String,
        pub width: f32,
    }

    /// Measure `detail`, if present, via [`RowDetail`].
    pub(crate) fn measure_row_detail(
        surface: &mut dyn PaintSurface,
        detail: Option<&StyledText>,
    ) -> Option<RowDetail> {
        detail.map(|d| {
            let text: String = d.spans.iter().map(|s| s.text.as_str()).collect();
            let (width, _) = surface.surface_measure_text(&text);
            RowDetail { text, width }
        })
    }

    /// Width to reserve right of a row's main content for `detail` —
    /// its measured width plus an 8-unit gap, or `0.0` when there is no
    /// detail. Shared so `ListView`/`Palette` can't drift on how much
    /// room a detail column needs ahead of painting the main text.
    pub(crate) fn row_detail_reserve(detail: &Option<RowDetail>) -> f32 {
        detail.as_ref().map(|d| d.width + 8.0).unwrap_or(0.0)
    }

    /// Paint `detail` right-aligned within `row_rect`, `right_margin`
    /// units in from the row's right edge, skipped when there isn't room
    /// past `cursor_x` (the main content's own trailing edge).
    ///
    /// `right_margin` is caller-supplied rather than a shared constant:
    /// `ListView` uses `4.0`, `Palette` uses `8.0` — a pre-existing,
    /// intentional difference between the two that this extraction
    /// preserves rather than silently unifies.
    pub(crate) fn paint_row_detail(
        surface: &mut dyn PaintSurface,
        detail: &Option<RowDetail>,
        row_rect: Rect,
        right_margin: f32,
        cursor_x: f32,
        fg: Color,
    ) {
        let Some(detail) = detail else {
            return;
        };
        let dx = row_rect.x + row_rect.width - detail.width - right_margin;
        if dx <= cursor_x {
            return;
        }
        let (_, dh) = surface.surface_measure_text(&detail.text);
        surface.surface_draw_text_run(
            Rect::new(
                dx,
                row_rect.y + (row_rect.height - dh) / 2.0,
                detail.width,
                dh,
            ),
            &detail.text,
            fg,
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::types::StyledText;
        use crate::{Icon, Image, Point, Viewport};

        /// Minimal `PaintSurface` double: 8-unit-wide chars, 16-unit
        /// lines — matches the mocks every other `native_surface_paint`
        /// test module in this crate uses. Records every
        /// `surface_draw_text_run` call so a test can assert whether
        /// [`paint_row_detail`] actually painted.
        #[derive(Default)]
        struct RecordingSurface {
            text_runs: Vec<(Rect, String, Color)>,
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
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: Point,
                _to: Point,
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

        #[test]
        fn paint_row_icon_without_an_icon_returns_cursor_unchanged() {
            let mut surface = RecordingSurface::default();
            let cursor_x = paint_row_icon(
                &mut surface,
                None,
                false,
                Color::rgb(0, 0, 0),
                0.0,
                16.0,
                10.0,
                |_, _, _, _| panic!("draw must not be called without an icon"),
            );
            assert_eq!(cursor_x, 10.0);
        }

        #[test]
        fn paint_row_icon_advances_past_the_glyph_plus_a_6_unit_gap() {
            let mut surface = RecordingSurface::default();
            let icon = Icon {
                glyph: "\u{f15b}".to_string(),
                fallback: "F".to_string(),
                color: None,
            };
            let mut drawn: Option<(Rect, String, Color)> = None;
            let cursor_x = paint_row_icon(
                &mut surface,
                Some(&icon),
                false,
                Color::rgb(10, 20, 30),
                0.0,
                16.0,
                10.0,
                |_, rect, glyph, fg| drawn = Some((rect, glyph.to_string(), fg)),
            );
            // ASCII fallback ("F") is one char wide == 8.0 units.
            assert_eq!(cursor_x, 10.0 + 8.0 + 6.0);
            let (rect, glyph, fg) = drawn.expect("draw must be called when an icon is present");
            assert_eq!(glyph, "F");
            assert_eq!(fg, Color::rgb(10, 20, 30));
            assert_eq!(rect.x, 10.0);
        }

        #[test]
        fn paint_row_icon_nerd_fonts_enabled_uses_the_glyph_not_the_fallback() {
            let mut surface = RecordingSurface::default();
            let icon = Icon {
                glyph: "XY".to_string(),
                fallback: "F".to_string(),
                color: None,
            };
            let mut drawn_glyph = String::new();
            paint_row_icon(
                &mut surface,
                Some(&icon),
                true,
                Color::rgb(0, 0, 0),
                0.0,
                16.0,
                0.0,
                |_, _, glyph, _| drawn_glyph = glyph.to_string(),
            );
            assert_eq!(drawn_glyph, "XY");
        }

        #[test]
        fn row_detail_reserve_is_zero_without_a_detail() {
            assert_eq!(row_detail_reserve(&None), 0.0);
        }

        #[test]
        fn measure_and_reserve_detail_adds_an_8_unit_gap() {
            let mut surface = RecordingSurface::default();
            let detail = measure_row_detail(&mut surface, Some(&StyledText::plain("abc")));
            assert_eq!(row_detail_reserve(&detail), 3.0 * 8.0 + 8.0);
        }

        #[test]
        fn paint_row_detail_skipped_when_main_content_runs_past_it() {
            let mut surface = RecordingSurface::default();
            let detail = measure_row_detail(&mut surface, Some(&StyledText::plain("abc")));
            let row_rect = Rect::new(0.0, 0.0, 100.0, 16.0);
            // `cursor_x` (100.0) is already past where `dx` would land, so
            // the detail must not paint at all.
            paint_row_detail(
                &mut surface,
                &detail,
                row_rect,
                4.0,
                100.0,
                Color::rgb(1, 1, 1),
            );
            assert!(surface.text_runs.is_empty());
        }

        #[test]
        fn paint_row_detail_paints_right_aligned_within_right_margin_when_there_is_room() {
            let mut surface = RecordingSurface::default();
            let detail = measure_row_detail(&mut surface, Some(&StyledText::plain("abc")));
            let row_rect = Rect::new(0.0, 0.0, 100.0, 16.0);
            paint_row_detail(
                &mut surface,
                &detail,
                row_rect,
                4.0,
                0.0,
                Color::rgb(9, 9, 9),
            );
            assert_eq!(surface.text_runs.len(), 1);
            let (rect, text, fg) = &surface.text_runs[0];
            assert_eq!(text, "abc");
            assert_eq!(*fg, Color::rgb(9, 9, 9));
            // dx = row_rect.x + row_rect.width - detail.width - right_margin
            //    = 0 + 100 - 24 - 4 = 72.
            assert_eq!(rect.x, 72.0);
        }
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
        // `scroll_offset` conflicts with `scroll_px` here (9 vs. the 5
        // rows `scroll_px` resolves to); the thumb matching the `None`
        // case below proves `scroll_px` actually wins rather than the
        // two calls merely agreeing by construction.
        let via_px = vscrollbar_thumb("rs", track, 9, Some(5.0), 20, 1.0, 10.0);
        let via_offset = vscrollbar_thumb("rs", track, 5, None, 20, 1.0, 10.0);
        assert_eq!(via_px.thumb_start, via_offset.thumb_start);
    }

    #[test]
    fn vscrollbar_thumb_zero_row_height_does_not_produce_nan() {
        let track = Rect::new(0.0, 0.0, 1.0, 10.0);
        let thumb = vscrollbar_thumb("rs", track, 3, Some(5.0), 20, 0.0, 10.0);
        assert!(!thumb.thumb_start.is_nan());
        assert!(!thumb.thumb_len.is_nan());
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
