//! The pixel painter for [`Minimap`], written against [`PaintSurface`]
//! only.
//!
//! Back to front: the strip's background, the editor viewport's
//! translucent `accent_bg` band, then — clipped to the strip — one row per
//! [`MinimapLayout::visible_lines`] entry. A row paints each non-blank
//! character column, up to [`COLUMN_CAPACITY`], in the colour of the
//! [`MinimapSpan`] covering it, one [`MinimapScale::cell_w_px`] apart:
//!
//! - **Atlas glyphs** when the surface can rasterise text offscreen
//!   ([`PaintSurface::surface_glyph_alpha_sheet`]): every printable ASCII
//!   character is shaped once, large, into a sample sheet, downsampled to
//!   a device-pixel tile ([`MinimapCharAtlas`]) and blitted per column
//!   with [`PaintSurface::surface_blit_alpha`]. The atlas is cached per
//!   thread ([`super::with_atlas_cache`]), keyed on the sheet and the
//!   tile size, so painting never shapes text at the row pitch.
//! - **Column blocks** otherwise: one solid `cell_w`-wide block per
//!   column, the full row tall — the indent and gap silhouette VS Code's
//!   `renderCharacters: false` mode shows.

use super::{
    color_at_column, with_atlas_cache, Minimap, MinimapCharAtlas, MinimapLayout, MinimapScale,
    MinimapSpan, SpanCursor, VisibleMinimapLine, ATLAS_FIRST_CHAR, ATLAS_LAST_CHAR,
    COLUMN_CAPACITY,
};
use crate::paint_surface::PaintSurface;
use crate::primitive::Size;
use crate::theme::Theme;
use crate::{Color, FontRole, Rect};

/// One glyph's slot in the sample sheet the atlas is downsampled from, in
/// pixels: VS Code's own sample cell (10 wide, 16 tall).
pub(crate) const SAMPLE_CELL: Size = Size {
    width: 10.0,
    height: 16.0,
};

/// How a row paints its characters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Glyphs {
    /// Atlas glyph tiles at `dpi_scale` device pixels per logical pixel,
    /// when the surface supplies a sample sheet; column blocks otherwise.
    Atlas { dpi_scale: f32 },
    /// Column blocks only.
    #[cfg_attr(
        not(any(feature = "gtk", all(feature = "macos", target_os = "macos"))),
        allow(dead_code)
    )]
    Blocks,
}

/// Every character the atlas samples, in atlas order.
fn atlas_chars() -> String {
    (ATLAS_FIRST_CHAR..=ATLAS_LAST_CHAR)
        .filter_map(char::from_u32)
        .collect()
}

/// Paint `minimap` at `layout` onto `surface`, one `scale` cell per
/// character. See the module docs.
pub(crate) fn paint(
    minimap: &Minimap,
    layout: &MinimapLayout,
    surface: &mut dyn PaintSurface,
    theme: &Theme,
    scale: MinimapScale,
    glyphs: Glyphs,
) {
    if layout.visible_lines.is_empty() {
        return;
    }
    let bounds = layout.bounds;
    surface.surface_fill_rect(bounds, theme.background);
    let hl = layout.viewport_highlight;
    if hl.height > 0.0 {
        surface.surface_fill_rect_alpha(hl, theme.accent_bg, 0.25);
    }

    surface.surface_push_clip(bounds);
    let cell_w = scale.cell_w_px() as f32;
    let sheet = match glyphs {
        Glyphs::Atlas { dpi_scale } => surface
            .surface_glyph_alpha_sheet(&atlas_chars(), FontRole::Editor, SAMPLE_CELL)
            .map(|sheet| (sheet, dpi_scale.max(1.0))),
        Glyphs::Blocks => None,
    };
    match sheet {
        Some((sheet, dpi_scale)) => {
            let tile_w = (cell_w * dpi_scale).round().max(1.0) as usize;
            let tile_h = (scale.row_pitch_px() as f32 * dpi_scale).round().max(1.0) as usize;
            let key = sheet_key(&sheet, tile_w, tile_h);
            with_atlas_cache(|cache| {
                let atlas = cache.get_or_build(&key, dpi_scale, || {
                    MinimapCharAtlas::from_alpha_sheet(
                        &sheet.alpha,
                        sheet.width as usize,
                        sheet.height as usize,
                        SAMPLE_CELL.width as usize,
                        tile_w,
                        tile_h,
                    )
                });
                let tile_size = Size {
                    width: atlas.tile_w() as f32 / dpi_scale,
                    height: atlas.tile_h() as f32 / dpi_scale,
                };
                for_each_row(minimap, layout, |vline, text, spans| {
                    for_each_column(text, spans, theme, |col, ch, color| {
                        surface.surface_blit_alpha(
                            Rect::new(
                                vline.bounds.x + col as f32 * cell_w,
                                vline.bounds.y,
                                tile_size.width,
                                tile_size.height,
                            ),
                            atlas.tile(ch),
                            atlas.tile_w() as u32,
                            atlas.tile_h() as u32,
                            color,
                        );
                    });
                });
            });
        }
        None => for_each_row(minimap, layout, |vline, text, spans| {
            for_each_column(text, spans, theme, |col, _, color| {
                surface.surface_fill_rect(
                    Rect::new(
                        vline.bounds.x + col as f32 * cell_w,
                        vline.bounds.y,
                        cell_w,
                        vline.bounds.height,
                    ),
                    color,
                );
            });
        }),
    }
    surface.surface_pop_clip();
}

/// The atlas cache key for `sheet` at a `tile_w` by `tile_h` tile: a hash
/// of the sheet's pixels, so a different font yields a different key.
fn sheet_key(sheet: &crate::AlphaSheet, tile_w: usize, tile_h: usize) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (sheet.width, sheet.height, &sheet.alpha).hash(&mut hasher);
    format!("{:016x}#{tile_w}x{tile_h}", hasher.finish())
}

/// Call `paint_row` for each visible row with its first line's text and
/// that row's spans, walking `syntax_spans` once for the whole strip.
fn for_each_row(
    minimap: &Minimap,
    layout: &MinimapLayout,
    mut paint_row: impl FnMut(&VisibleMinimapLine, &str, &[MinimapSpan]),
) {
    let mut spans = SpanCursor::new(&minimap.syntax_spans);
    for vline in &layout.visible_lines {
        let Some(line) = minimap.lines.get(vline.start_line_idx) else {
            continue;
        };
        let row_spans = spans.row_spans(vline.start_line_idx);
        paint_row(vline, &line.text, row_spans);
    }
}

/// Call `paint_col` for each non-blank character column of `text`, up to
/// [`COLUMN_CAPACITY`], with the colour of the span covering it.
fn for_each_column(
    text: &str,
    row_spans: &[MinimapSpan],
    theme: &Theme,
    mut paint_col: impl FnMut(usize, char, Color),
) {
    for (col, ch) in text.chars().enumerate().take(COLUMN_CAPACITY) {
        if ch.is_whitespace() {
            continue;
        }
        paint_col(col, ch, color_at_column(row_spans, col, theme.foreground));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ImagePaintResult;
    use crate::primitives::minimap::{MinimapLine, MinimapScale};
    use crate::{Image, Point, Viewport, WidgetId};

    #[derive(Debug, Clone, PartialEq)]
    enum Op {
        Fill(Rect, Color),
        FillAlpha(Rect, Color, f32),
        Blit(Rect, u32, u32, Color),
        Clip(Rect),
        Unclip,
    }

    /// Records every verb; offers a sample sheet when `sheet` is `Some`.
    struct Recorder {
        ops: Vec<Op>,
        sheet: Option<crate::AlphaSheet>,
        sheet_requests: usize,
    }

    impl Recorder {
        fn new(sheet: Option<crate::AlphaSheet>) -> Self {
            Self {
                ops: Vec::new(),
                sheet,
                sheet_requests: 0,
            }
        }
    }

    impl PaintSurface for Recorder {
        fn surface_begin_frame(&mut self, _viewport: Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> Viewport {
            Viewport::new(100.0, 100.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            16.0
        }
        fn surface_char_width(&self) -> f32 {
            8.0
        }
        fn surface_measure_text(&self, text: &str) -> (f32, f32) {
            (text.len() as f32 * 8.0, 16.0)
        }
        fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
            self.ops.push(Op::Fill(rect, color));
        }
        fn surface_fill_rect_alpha(&mut self, rect: Rect, color: Color, alpha: f32) {
            self.ops.push(Op::FillAlpha(rect, color, alpha));
        }
        fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
            self.ops.push(Op::Fill(rect, color));
        }
        fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _w: f32) {}
        fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
        fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _w: f32) {}
        fn surface_push_clip(&mut self, rect: Rect) {
            self.ops.push(Op::Clip(rect));
        }
        fn surface_pop_clip(&mut self) {
            self.ops.push(Op::Unclip);
        }
        fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
            ImagePaintResult::Unsupported
        }
        fn surface_blit_alpha(
            &mut self,
            dest: Rect,
            _alpha: &[u8],
            width: u32,
            height: u32,
            color: Color,
        ) {
            self.ops.push(Op::Blit(dest, width, height, color));
        }
        fn surface_glyph_alpha_sheet(
            &mut self,
            chars: &str,
            _role: FontRole,
            cell: Size,
        ) -> Option<crate::AlphaSheet> {
            self.sheet_requests += 1;
            assert_eq!(chars.chars().count(), super::super::ATLAS_CHAR_COUNT);
            assert_eq!(cell, SAMPLE_CELL);
            self.sheet.clone()
        }
    }

    fn minimap(lines: &[&str], scale: MinimapScale) -> Minimap {
        Minimap {
            id: WidgetId::new("mm"),
            lines: lines
                .iter()
                .enumerate()
                .map(|(i, t)| MinimapLine {
                    text: t.to_string(),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: vec![MinimapSpan {
                line_idx: 0,
                start_col: 0,
                end_col: 2,
                color: Color::rgb(200, 10, 10),
            }],
            visible_row_start: 0,
            visible_row_count: 1,
            total_buffer_lines: lines.len(),
            scale,
            sizing: None,
        }
    }

    fn sample_sheet(value: u8) -> crate::AlphaSheet {
        let w = SAMPLE_CELL.width as u32 * super::super::ATLAS_CHAR_COUNT as u32;
        let h = SAMPLE_CELL.height as u32;
        crate::AlphaSheet::new(w, h, vec![value; (w * h) as usize])
    }

    #[test]
    fn blocks_paint_one_block_per_non_blank_column_in_its_span_colour() {
        let mm = minimap(&["fn  x", "y"], MinimapScale::One);
        let layout = mm.pixel_layout(Rect::new(10.0, 20.0, 50.0, 40.0), mm.scale, None);
        let mut surface = Recorder::new(None);
        let theme = Theme::default();
        paint(&mm, &layout, &mut surface, &theme, mm.scale, Glyphs::Blocks);

        let blocks: Vec<(f32, Color)> = surface
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Fill(r, c) if r.width == 1.0 => Some((r.x, *c)),
                _ => None,
            })
            .collect();
        let red = Color::rgb(200, 10, 10);
        assert_eq!(
            blocks,
            vec![
                (10.0, red),
                (11.0, red),
                (14.0, theme.foreground),
                (10.0, theme.foreground),
            ]
        );
        assert_eq!(surface.ops[0], Op::Fill(layout.bounds, theme.background));
        assert!(surface.ops.contains(&Op::Clip(layout.bounds)));
        assert_eq!(surface.ops.last(), Some(&Op::Unclip));
    }

    #[test]
    fn a_surface_without_a_sheet_falls_back_to_blocks() {
        let mm = minimap(&["ab"], MinimapScale::Two);
        let layout = mm.pixel_layout(Rect::new(0.0, 0.0, 50.0, 40.0), mm.scale, None);
        let mut surface = Recorder::new(None);
        paint(
            &mm,
            &layout,
            &mut surface,
            &Theme::default(),
            mm.scale,
            Glyphs::Atlas { dpi_scale: 1.0 },
        );
        assert_eq!(surface.sheet_requests, 1);
        assert!(surface.ops.iter().all(|op| !matches!(op, Op::Blit(..))));
        assert!(surface
            .ops
            .iter()
            .any(|op| matches!(op, Op::Fill(r, _) if r.width == 2.0 && r.x == 2.0)));
    }

    #[test]
    fn atlas_glyphs_blit_a_device_pixel_tile_per_column_at_the_cell_size() {
        let mm = minimap(&["a b"], MinimapScale::Two);
        let layout = mm.pixel_layout(Rect::new(4.0, 0.0, 50.0, 40.0), mm.scale, None);
        let mut surface = Recorder::new(Some(sample_sheet(200)));
        paint(
            &mm,
            &layout,
            &mut surface,
            &Theme::default(),
            mm.scale,
            Glyphs::Atlas { dpi_scale: 2.0 },
        );
        let blits: Vec<(Rect, u32, u32)> = surface
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Blit(r, w, h, _) => Some((*r, *w, *h)),
                _ => None,
            })
            .collect();
        assert_eq!(
            blits,
            vec![
                (Rect::new(4.0, 0.0, 2.0, 4.0), 4, 8),
                (Rect::new(8.0, 0.0, 2.0, 4.0), 4, 8),
            ]
        );
    }

    #[test]
    fn the_viewport_band_is_a_translucent_accent_fill() {
        let mut mm = minimap(&["a"; 40], MinimapScale::One);
        mm.visible_row_start = 10;
        mm.visible_row_count = 5;
        let layout = mm.pixel_layout(Rect::new(0.0, 0.0, 50.0, 200.0), mm.scale, None);
        let mut surface = Recorder::new(None);
        let theme = Theme::default();
        paint(&mm, &layout, &mut surface, &theme, mm.scale, Glyphs::Blocks);
        assert!(surface.ops.contains(&Op::FillAlpha(
            layout.viewport_highlight,
            theme.accent_bg,
            0.25
        )));
    }

    #[test]
    fn an_empty_minimap_paints_nothing() {
        let mm = minimap(&[], MinimapScale::One);
        let layout = mm.pixel_layout(Rect::new(0.0, 0.0, 50.0, 40.0), mm.scale, None);
        let mut surface = Recorder::new(None);
        paint(
            &mm,
            &layout,
            &mut surface,
            &Theme::default(),
            mm.scale,
            Glyphs::Blocks,
        );
        assert!(surface.ops.is_empty());
    }

    #[test]
    fn the_default_blit_fills_one_rect_per_run_of_equal_coverage() {
        struct Fills(Vec<(Rect, f32)>);
        impl PaintSurface for Fills {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(1.0, 1.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                1.0
            }
            fn surface_char_width(&self) -> f32 {
                1.0
            }
            fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
                (0.0, 0.0)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rect_alpha(&mut self, rect: Rect, _color: Color, alpha: f32) {
                self.0.push((rect, alpha));
            }
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _r: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _w: f32) {}
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(&mut self, _a: Point, _b: Point, _color: Color, _w: f32) {}
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }
        let mut s = Fills(Vec::new());
        // Two rows of four pixels, stretched over an 8x4 rect.
        s.surface_blit_alpha(
            Rect::new(10.0, 20.0, 8.0, 4.0),
            &[0, 255, 255, 51, 0, 0, 0, 0],
            4,
            2,
            Color::rgb(1, 2, 3),
        );
        assert_eq!(
            s.0,
            vec![
                (Rect::new(12.0, 20.0, 4.0, 2.0), 1.0),
                (Rect::new(16.0, 20.0, 2.0, 2.0), 0.2),
            ]
        );
    }
}
