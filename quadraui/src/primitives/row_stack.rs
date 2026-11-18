//! Shared vertical row-stacking layout math for `TreeView`, `ListView`,
//! `Palette`, and `MessageList`, plus a shared icon/detail row-content
//! painter for `ListView` and `Palette` (`row_content_paint`, below).
//!
//! Each row-stacking primitive lays out a flat sequence of rows
//! top-to-bottom from a `scroll_offset` (optionally overridden by a
//! pixel-precise `scroll_px`), clips the last visible row to the
//! viewport, and (where it has one) exposes a vertical `Scrollbar` whose
//! thumb tracks the same scroll position. This module is the single home
//! for that shared arithmetic, so `tree.rs`, `list.rs`, `palette.rs`, and
//! `message_list.rs` cannot drift out of sync on it and a future
//! row-stacking primitive gets it for free.
//!
//! What's *not* here: hit-region construction (`TreeView` splits a row
//! into `Chevron`/`Row` zones; the others don't), border/title insets
//! (`ListView`-only), and the public `*Layout`/`*Hit` types themselves —
//! those stay primitive-specific because their shapes genuinely differ.
//! `MessageList::layout` uses [`layout_row_stack`] but its rasterisers do
//! not yet paint from it — see `MessageListLayout`'s own doc.

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

/// [`layout_row_stack`]'s stacking walk, driven by a row *count* from an
/// already-resolved scroll position instead of by a `y_end` bound:
/// `count` rows starting at `scroll_offset`, stacked from `y_start` with
/// the first shifted up by `scroll_sub_px`, each keeping its full
/// measured height.
///
/// `DataTable`'s rasterisers stack their body rows through this rather
/// than through [`layout_row_stack`], because they differ from the
/// `y_end`-bounded primitives on both of that function's remaining
/// behaviours:
///
/// * **Scroll resolution.** `DataTable` resolves its own offset via
///   [`DataTable::effective_scroll_offset`](crate::primitives::data_table::DataTable::effective_scroll_offset),
///   which reads a stale `scroll_offset` raw on the `scroll_px: None`
///   path where [`resolve_scroll_px`] would clamp it into
///   `[0, total_rows)` — a stale offset must leave an empty visible
///   window, not snap to the last row.
/// * **Row count.** `DataTable::layout` derives `visible_rows` from the
///   *inset-reduced* body height, which a `y_end` walk over the painted
///   `rect` does not reproduce; and its rows paint at full height under
///   the painter's own body-band clip, never height-clipped individually.
pub fn stack_rows_from<F, E>(
    scroll_offset: usize,
    scroll_sub_px: f32,
    count: usize,
    x: f32,
    y_start: f32,
    width: f32,
    mut measure: F,
) -> impl Iterator<Item = VisibleRow<E>>
where
    F: FnMut(usize) -> (f32, E),
{
    let mut y = y_start - scroll_sub_px;
    (scroll_offset..scroll_offset + count).map(move |idx| {
        let (height, extra) = measure(idx);
        let bounds = Rect::new(x, y, width, height);
        y += height;
        VisibleRow { idx, bounds, extra }
    })
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
    /// Tint every other row with [`crate::theme::Theme::row_alt_bg`]
    /// ("zebra striping"). `false` (the default) paints with no stripe
    /// at all.
    ///
    /// How a stripe interacts with the selection/hover highlight is a
    /// per-consumer painting choice, not shared by this type: `ListView`
    /// (see [`crate::primitives::list::native_surface_paint`]'s row loop)
    /// and `DataTable`'s TUI rasteriser both *replace* the stripe on the
    /// selected/hovered row — the row carrying either highlight is never
    /// striped. `DataTable`'s GUI painter instead fills the stripe first
    /// and alpha-blends the selection/hover tint on top of it, so a
    /// striped row keeps showing through a hover tint underneath it.
    ///
    /// [`crate::primitives::list::ListPaintState`] and
    /// [`crate::primitives::data_table::DataTablePaintState`] both read
    /// this field — [`crate::primitives::tree::TreePaintState`] shares
    /// the same underlying type but no `TreeView` rasteriser paints a
    /// stripe, so the field is simply always `false` there.
    pub striped: bool,
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

    pub fn with_striped(mut self, striped: bool) -> Self {
        self.striped = striped;
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
    use crate::backend::ImagePaintResult;
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::primitives::image::Image;
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

    /// Interaction-state adornments [`paint_tile_icon_label`] paints on
    /// top of an `IconGrid` tile's icon/label — selection fill, hover
    /// tint, and the focus ring, plus the colours/geometry each one
    /// paints with. Grouped into one struct rather than more positional
    /// `bool`/`Color` arguments: a future adornment (e.g. a drag-over
    /// highlight) becomes one new field here instead of another
    /// positional argument every call site must thread through blind.
    pub(crate) struct TileHighlight {
        /// Fills `tile` with [`Self::selected_bg`] — `IconGrid`'s
        /// selection highlight is a filled tile, unlike a row's
        /// selection highlight which the row's own background already
        /// covers before this helper runs.
        pub selected: bool,
        /// Fills `tile` with [`Self::hover_bg`] — skipped when
        /// [`Self::selected`] is also set, so a hovered-and-selected
        /// tile shows only the (more prominent) selection fill.
        pub hovered: bool,
        /// Strokes a focus ring around `tile` in
        /// [`Self::focus_ring_color`] when [`Self::selected`] is also
        /// set — `IconGrid` has no separate "this tile has keyboard
        /// focus but isn't the selection" state (same single-selection
        /// model [`crate::primitives::icon_grid::IconGrid::has_focus`]'s
        /// own doc describes), so the ring always coincides with the
        /// selected tile's fill, reinforcing it as the grid's live
        /// keyboard target rather than replacing the fill.
        pub focused: bool,
        pub selected_bg: Color,
        pub hover_bg: Color,
        pub focus_ring_color: Color,
        /// Corner radius for both the selection/hover fill and the
        /// focus ring, clamped to half of `tile`'s shorter side the same
        /// way [`PaintSurface::surface_fill_rounded_rect`]'s own doc
        /// describes.
        pub radius: f32,
        pub ring_width: f32,
    }

    /// Paint one `IconGrid` tile's icon/thumbnail (centred, upper band)
    /// and label (centred, word-wrapped to at most 2 lines then
    /// ellipsized, lower band) within `tile` — the centred/stacked
    /// sibling of [`paint_row_icon`]'s left-aligned, single-line layout.
    /// `ListView`/`Palette` paint a row's icon beside its text on one
    /// line; `IconGrid` stacks icon above label across two bands of the
    /// same tile, so this is a distinct shape, not a parameterisation of
    /// [`paint_row_icon`].
    ///
    /// `image`, when `Some`, paints in place of `icon` — scaled to fit
    /// the icon band via [`Image::layout`] (aspect preserved, the
    /// default [`crate::primitives::image::ImageFit::Contain`]) — and
    /// falls back to `icon` when [`PaintSurface::surface_draw_image`]
    /// reports [`ImagePaintResult::Unsupported`] (e.g. macOS's own
    /// "no `NSImage` decoder wired up yet" gap — see
    /// [`PaintSurface::surface_draw_image`]'s doc), so a backend that
    /// cannot yet decode the thumbnail still shows the icon glyph rather
    /// than a blank band.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_tile_icon_label(
        surface: &mut dyn PaintSurface,
        tile: Rect,
        icon: Option<&Icon>,
        image: Option<&Image>,
        nerd_fonts_enabled: bool,
        label: &str,
        fg: Color,
        highlight: TileHighlight,
    ) {
        if tile.width <= 0.0 || tile.height <= 0.0 {
            return;
        }
        let radius = highlight
            .radius
            .min(tile.width / 2.0)
            .min(tile.height / 2.0)
            .max(0.0);
        if highlight.selected {
            surface.surface_fill_rounded_rect(tile, radius, highlight.selected_bg);
        } else if highlight.hovered {
            surface.surface_fill_rounded_rect(tile, radius, highlight.hover_bg);
        }
        if highlight.selected && highlight.focused {
            surface.surface_stroke_rounded_rect(
                tile,
                radius,
                highlight.focus_ring_color,
                highlight.ring_width,
            );
        }

        let icon_band_h = (tile.height * 0.6).max(1.0).min(tile.height);
        let icon_band = Rect::new(tile.x, tile.y, tile.width, icon_band_h);
        let painted_image = image.is_some_and(|image| {
            let fit = image.layout(icon_band);
            surface.surface_draw_image(fit.bounds, image) == ImagePaintResult::Painted
        });
        if !painted_image {
            if let Some(icon) = icon {
                let glyph = if nerd_fonts_enabled {
                    icon.glyph.as_str()
                } else {
                    icon.fallback.as_str()
                };
                let (iw, ih) = surface.surface_measure_text(glyph);
                let ix = tile.x + ((tile.width - iw) / 2.0).max(0.0);
                let iy = tile.y + ((icon_band_h - ih) / 2.0).max(0.0);
                surface.surface_draw_icon_glyph(
                    Rect::new(ix, iy, iw, ih),
                    glyph,
                    icon.color.unwrap_or(fg),
                );
            }
        }

        let label_y = tile.y + icon_band_h;
        let label_h = (tile.height - icon_band_h).max(0.0);
        if label_h <= 0.0 || label.is_empty() {
            return;
        }
        let line_h = surface.surface_measure_text(label).1.max(1.0);
        // At most 2 lines, and never more than `label_h` actually has
        // room for — a short tile still gets the single-line truncation
        // behaviour this replaced, rather than painting a second line
        // past the tile's own bottom edge.
        let max_lines = ((label_h / line_h).floor() as usize).clamp(1, 2);
        let max_w = (tile.width - 4.0).max(0.0);
        let lines = crate::primitives::toast::wrap_text_lines(label, max_w, max_lines, &|s| {
            surface.surface_measure_text(s).0
        });
        if lines.is_empty() {
            return;
        }
        let total_h = line_h * lines.len() as f32;
        let mut ly = label_y + ((label_h - total_h) / 2.0).max(0.0);
        for line in &lines {
            if !line.is_empty() {
                let (lw, _) = surface.surface_measure_text(line);
                let lx = tile.x + ((tile.width - lw) / 2.0).max(0.0);
                surface.surface_draw_text_run(Rect::new(lx, ly, lw, line_h), line, fg);
            }
            ly += line_h;
        }
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
        /// `surface_draw_text_run`/`surface_fill_rect` call so a test
        /// can assert whether [`paint_row_detail`]/[`paint_tile_icon_label`]
        /// actually painted.
        #[derive(Default)]
        struct RecordingSurface {
            text_runs: Vec<(Rect, String, Color)>,
            fills: Vec<(Rect, Color)>,
            rounded_fills: Vec<(Rect, f32, Color)>,
            rounded_strokes: Vec<(Rect, f32, Color, f32)>,
            images: Vec<Rect>,
            /// What [`PaintSurface::surface_draw_image`] reports — a test
            /// that wants to exercise the "image painted" path sets this
            /// to [`ImagePaintResult::Painted`] before calling
            /// `paint_tile_icon_label`.
            draw_image_result: ImagePaintResult,
        }

        impl Default for ImagePaintResult {
            fn default() -> Self {
                ImagePaintResult::Unsupported
            }
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
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
                self.rounded_fills.push((rect, radius, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                radius: f32,
                color: Color,
                stroke_width: f32,
            ) {
                self.rounded_strokes
                    .push((rect, radius, color, stroke_width));
            }
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
            fn surface_draw_image(&mut self, rect: Rect, _image: &Image) -> ImagePaintResult {
                self.images.push(rect);
                self.draw_image_result
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

        // ── paint_tile_icon_label ────────────────────────────────────

        /// A plain, unselected/unhovered/unfocused [`TileHighlight`] —
        /// the baseline most tests start from and tweak one field of.
        fn plain_highlight() -> TileHighlight {
            TileHighlight {
                selected: false,
                hovered: false,
                focused: false,
                selected_bg: Color::rgb(9, 9, 9),
                hover_bg: Color::rgb(5, 5, 5),
                focus_ring_color: Color::rgb(2, 2, 2),
                radius: 4.0,
                ring_width: 2.0,
            }
        }

        #[test]
        fn paint_tile_icon_label_paints_glyph_then_label() {
            let mut surface = RecordingSurface::default();
            let icon = Icon::new("GLYPH", "F");
            let tile = Rect::new(0.0, 0.0, 80.0, 40.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                Some(&icon),
                None,
                true,
                "a.txt",
                Color::rgb(1, 2, 3),
                plain_highlight(),
            );
            assert_eq!(surface.text_runs.len(), 2, "icon glyph plus label");
            assert_eq!(surface.text_runs[0].1, "GLYPH");
            assert_eq!(surface.text_runs[1].1, "a.txt");
            assert_eq!(surface.text_runs[1].2, Color::rgb(1, 2, 3));
            assert!(
                surface.fills.is_empty() && surface.rounded_fills.is_empty(),
                "an unselected, unhovered tile must not fill its background"
            );
        }

        #[test]
        fn paint_tile_icon_label_without_an_icon_paints_only_the_label() {
            let mut surface = RecordingSurface::default();
            let tile = Rect::new(0.0, 0.0, 80.0, 40.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                None,
                None,
                false,
                "a.txt",
                Color::rgb(1, 2, 3),
                plain_highlight(),
            );
            assert_eq!(surface.text_runs.len(), 1);
            assert_eq!(surface.text_runs[0].1, "a.txt");
        }

        #[test]
        fn paint_tile_icon_label_fills_the_whole_tile_rounded_when_selected() {
            let mut surface = RecordingSurface::default();
            let tile = Rect::new(0.0, 0.0, 80.0, 40.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                None,
                None,
                false,
                "a.txt",
                Color::rgb(1, 2, 3),
                TileHighlight {
                    selected: true,
                    ..plain_highlight()
                },
            );
            assert_eq!(
                surface.rounded_fills,
                vec![(tile, 4.0, Color::rgb(9, 9, 9))]
            );
            assert!(surface.fills.is_empty(), "selection fill must be rounded");
        }

        #[test]
        fn paint_tile_icon_label_tints_a_hovered_unselected_tile() {
            let mut surface = RecordingSurface::default();
            let tile = Rect::new(0.0, 0.0, 80.0, 40.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                None,
                None,
                false,
                "a.txt",
                Color::rgb(1, 2, 3),
                TileHighlight {
                    hovered: true,
                    ..plain_highlight()
                },
            );
            assert_eq!(
                surface.rounded_fills,
                vec![(tile, 4.0, Color::rgb(5, 5, 5))]
            );
        }

        #[test]
        fn paint_tile_icon_label_prefers_the_selection_fill_over_hover() {
            let mut surface = RecordingSurface::default();
            let tile = Rect::new(0.0, 0.0, 80.0, 40.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                None,
                None,
                false,
                "a.txt",
                Color::rgb(1, 2, 3),
                TileHighlight {
                    selected: true,
                    hovered: true,
                    ..plain_highlight()
                },
            );
            assert_eq!(
                surface.rounded_fills,
                vec![(tile, 4.0, Color::rgb(9, 9, 9))],
                "a hovered-and-selected tile shows only the selection fill"
            );
        }

        #[test]
        fn paint_tile_icon_label_strokes_a_focus_ring_only_when_selected_and_focused() {
            let mut surface = RecordingSurface::default();
            let tile = Rect::new(0.0, 0.0, 80.0, 40.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                None,
                None,
                false,
                "a.txt",
                Color::rgb(1, 2, 3),
                TileHighlight {
                    selected: true,
                    focused: true,
                    ..plain_highlight()
                },
            );
            assert_eq!(
                surface.rounded_strokes,
                vec![(tile, 4.0, Color::rgb(2, 2, 2), 2.0)]
            );

            let mut unfocused = RecordingSurface::default();
            paint_tile_icon_label(
                &mut unfocused,
                tile,
                None,
                None,
                false,
                "a.txt",
                Color::rgb(1, 2, 3),
                TileHighlight {
                    selected: true,
                    focused: false,
                    ..plain_highlight()
                },
            );
            assert!(
                unfocused.rounded_strokes.is_empty(),
                "selected but unfocused must not paint a focus ring"
            );
        }

        #[test]
        fn paint_tile_icon_label_truncates_an_overlong_label() {
            let mut surface = RecordingSurface::default();
            let tile = Rect::new(0.0, 0.0, 40.0, 40.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                None,
                None,
                false,
                &"x".repeat(50),
                Color::rgb(1, 2, 3),
                plain_highlight(),
            );
            let (_, text, _) = &surface.text_runs[0];
            assert!(text.ends_with('…'));
            assert!(text.len() < 50);
        }

        #[test]
        fn paint_tile_icon_label_wraps_a_long_label_across_two_lines() {
            let mut surface = RecordingSurface::default();
            // Tall enough tile for a 2-line label band (icon band
            // 0.6 * 80 == 48, label band 32 == exactly 2 * the 16-unit
            // line height the mock measures every string at).
            let tile = Rect::new(0.0, 0.0, 40.0, 80.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                None,
                None,
                false,
                "one two",
                Color::rgb(1, 2, 3),
                plain_highlight(),
            );
            assert_eq!(
                surface.text_runs.len(),
                2,
                "a tall-enough tile wraps rather than truncating onto one line"
            );
            assert_eq!(surface.text_runs[0].1, "one");
            assert_eq!(surface.text_runs[1].1, "two");
        }

        #[test]
        fn paint_tile_icon_label_paints_a_thumbnail_scaled_to_fit_in_place_of_the_icon() {
            let mut surface = RecordingSurface::default();
            surface.draw_image_result = ImagePaintResult::Painted;
            let icon = Icon::new("GLYPH", "F");
            let image = Image {
                id: crate::WidgetId::new("thumb"),
                source: crate::ImageSource::Bytes(vec![]),
                intrinsic_size: Some((100, 50)),
                fit: crate::ImageFit::Contain,
                fallback_text: String::new(),
            };
            let tile = Rect::new(0.0, 0.0, 80.0, 40.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                Some(&icon),
                Some(&image),
                true,
                "a.txt",
                Color::rgb(1, 2, 3),
                plain_highlight(),
            );
            assert_eq!(
                surface.images.len(),
                1,
                "the thumbnail must be painted once"
            );
            // Contain-fit of a 100x50 image into the 80x24 icon band
            // (80 * 0.6) is width-limited: 80 wide, 40 tall — still
            // taller than the 24-unit band, so height-limited instead:
            // scale = 24 / 50 == 0.48, width == 48.
            assert_eq!(surface.images[0].height, 24.0);
            assert!(
                surface.text_runs.iter().all(|(_, t, _)| t != "GLYPH"),
                "the icon glyph must not paint once a thumbnail painted"
            );
        }

        #[test]
        fn paint_tile_icon_label_falls_back_to_the_icon_when_the_thumbnail_is_unsupported() {
            let mut surface = RecordingSurface::default();
            // Default `draw_image_result` is `Unsupported`.
            let icon = Icon::new("GLYPH", "F");
            let image = Image {
                id: crate::WidgetId::new("thumb"),
                source: crate::ImageSource::Bytes(vec![]),
                intrinsic_size: Some((100, 50)),
                fit: crate::ImageFit::Contain,
                fallback_text: String::new(),
            };
            let tile = Rect::new(0.0, 0.0, 80.0, 40.0);
            paint_tile_icon_label(
                &mut surface,
                tile,
                Some(&icon),
                Some(&image),
                true,
                "a.txt",
                Color::rgb(1, 2, 3),
                plain_highlight(),
            );
            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == "GLYPH"),
                "an unsupported thumbnail must fall back to the icon glyph"
            );
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
    fn stack_rows_from_emits_exactly_count_rows_at_full_height() {
        let rows: Vec<VisibleRow<()>> =
            stack_rows_from(2, 0.0, 3, 1.0, 10.0, 20.0, |_| (4.0, ())).collect();
        let idxs: Vec<usize> = rows.iter().map(|r| r.idx).collect();
        assert_eq!(idxs, vec![2, 3, 4]);
        assert_eq!(rows[0].bounds, Rect::new(1.0, 10.0, 20.0, 4.0));
        assert_eq!(rows[2].bounds, Rect::new(1.0, 18.0, 20.0, 4.0));
    }

    #[test]
    fn stack_rows_from_shifts_the_whole_stack_up_by_the_sub_row_remainder() {
        let rows: Vec<VisibleRow<()>> =
            stack_rows_from(0, 1.5, 2, 0.0, 10.0, 5.0, |_| (4.0, ())).collect();
        assert_eq!(rows[0].bounds.y, 8.5);
        assert_eq!(rows[1].bounds.y, 12.5);
    }

    #[test]
    fn stack_rows_from_does_not_clamp_a_stale_scroll_offset() {
        // `layout_row_stack` would clamp 99 into `[0, 4)`; this walk
        // leaves it where the caller put it, which is what `DataTable`
        // needs on its `scroll_px: None` path.
        let rows: Vec<VisibleRow<()>> =
            stack_rows_from(99, 0.0, 1, 0.0, 0.0, 1.0, |_| (1.0, ())).collect();
        assert_eq!(rows[0].idx, 99);
    }

    #[test]
    fn stack_rows_from_zero_count_is_empty() {
        let rows: Vec<VisibleRow<()>> =
            stack_rows_from(0, 0.0, 0, 0.0, 0.0, 1.0, |_| (1.0, ())).collect();
        assert!(rows.is_empty());
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
