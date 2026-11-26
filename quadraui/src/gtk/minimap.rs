//! GTK entry points for [`Minimap`] — thin delegates onto the widget's
//! own pixel painter, `crate::primitives::minimap::paint_pixels`, which
//! documents the paint passes. These functions only adapt a bare Cairo
//! context and the frame's Pango layout into the
//! [`super::surface::CairoSurface`] that painter takes; the glyph atlas is
//! sampled in that layout's font family.

use gtk4::cairo::Context;
use gtk4::pango;

use super::surface::CairoSurface;
use crate::event::Rect as QRect;
use crate::primitives::minimap::paint_pixels::{self, Glyphs};
use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapScale};
use crate::theme::Theme;

/// The pixel layout of `minimap` in `(x, y, w, h)` without painting, at
/// [`MinimapScale::One`].
pub fn gtk_minimap_layout(minimap: &Minimap, x: f64, y: f64, w: f64, h: f64) -> MinimapLayout {
    gtk_minimap_layout_scaled(minimap, x, y, w, h, MinimapScale::One)
}

/// [`gtk_minimap_layout`] at an explicit [`MinimapScale`] — see
/// [`Minimap::pixel_layout`].
pub(crate) fn gtk_minimap_layout_scaled(
    minimap: &Minimap,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    scale: MinimapScale,
) -> MinimapLayout {
    minimap.pixel_layout(
        QRect::new(x as f32, y as f32, w as f32, h as f32),
        scale,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn paint(
    cr: &Context,
    pango_layout: &pango::Layout,
    rect: (f64, f64, f64, f64),
    minimap: &Minimap,
    theme: &Theme,
    scale: MinimapScale,
    glyphs: Glyphs,
) -> MinimapLayout {
    let (x, y, w, h) = rect;
    let layout = gtk_minimap_layout_scaled(minimap, x, y, w, h, scale);
    let mut surface = CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: true,
    };
    paint_pixels::paint(minimap, &layout, &mut surface, theme, scale, glyphs);
    layout
}

/// Draw `minimap` onto `cr` at [`MinimapScale::One`] as column blocks,
/// without the glyph atlas. Returns the layout for host click dispatch
/// (`layout.hit_test(x, y)`).
#[allow(clippy::too_many_arguments)]
pub fn draw_minimap(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    paint(
        cr,
        pango_layout,
        (x, y, w, h),
        minimap,
        theme,
        MinimapScale::One,
        Glyphs::Blocks,
    )
}

/// Draw `minimap` at `mm_scale` with glyph-atlas character shapes, each
/// tile `dpi_scale` device pixels per logical pixel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_minimap_cached(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    minimap: &Minimap,
    theme: &Theme,
    dpi_scale: f64,
    mm_scale: MinimapScale,
) -> MinimapLayout {
    paint(
        cr,
        pango_layout,
        (x, y, w, h),
        minimap,
        theme,
        mm_scale,
        Glyphs::Atlas {
            dpi_scale: dpi_scale as f32,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::minimap::{MinimapHit, MinimapLine, COLUMN_CAPACITY, ROW_PITCH_PX};
    use crate::types::{Color, WidgetId};
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

    fn minimap_from(lines: Vec<&str>, total_buffer_lines: usize) -> Minimap {
        Minimap {
            id: WidgetId::new("mm"),
            lines: lines
                .into_iter()
                .enumerate()
                .map(|(i, t)| MinimapLine {
                    text: t.into(),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: 0,
            total_buffer_lines,
            scale: MinimapScale::One,
            sizing: None,
        }
    }

    fn surface_and_layout() -> (CairoContext, pango::Layout) {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let layout = pangocairo::functions::create_layout(&cr);
        (cr, layout)
    }

    // ── fixed pitch, not file-length-dependent ───────────────────────

    #[test]
    fn row_pitch_is_independent_of_the_strip_height() {
        // The same buffer painted into strips of two very different
        // heights must resolve to the *same* row pitch under
        // [`MinimapSizing::FixedPitch`]. See `Minimap::layout`'s module docs.
        let lines: Vec<&str> = vec!["fn main() {}"; 8];
        let mm = minimap_from(lines, 8);

        let tall = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 800.0);
        let short = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 16.0);

        let tall_px = tall.visible_lines[0].bounds.height as f64;
        let short_px = short.visible_lines[0].bounds.height as f64;
        assert_eq!(tall_px, ROW_PITCH_PX);
        assert_eq!(short_px, ROW_PITCH_PX);
    }

    #[test]
    fn row_pitch_is_independent_of_the_file_length() {
        // The other half of the same guarantee: a 3-line file and a
        // 10,000-line file painted into the *same* strip must also
        // resolve to the same row pitch.
        let short_mm = minimap_from(vec!["fn main() {}"; 3], 3);
        let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
        let long_mm = minimap_from(long_refs, 10_000);

        let short_layout = gtk_minimap_layout(&short_mm, 0.0, 0.0, 40.0, 400.0);
        let long_layout = gtk_minimap_layout(&long_mm, 0.0, 0.0, 40.0, 400.0);

        assert_eq!(
            short_layout.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
        assert_eq!(
            long_layout.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
    }

    #[test]
    fn short_file_in_a_tall_strip_does_not_stretch() {
        // A 3-line file at the fixed pitch occupies 6px of a 200px
        // strip -- it must not stretch to fill it.
        let mm = minimap_from(vec!["//! Placeholder module"; 3], 3);
        let layout = gtk_minimap_layout(&mm, 0.0, 0.0, 200.0, 200.0);
        assert_eq!(layout.visible_lines.len(), 3);
        let last = layout.visible_lines.last().unwrap();
        assert!(
            (last.bounds.y + last.bounds.height) < 200.0,
            "a short file must not stretch its rows to fill the whole strip"
        );
    }

    // ── legibility floor ─────────────────────────────────────────────
    //
    // `is_legible`/`render_mode`'s own threshold behaviour is covered by
    // `primitives::minimap::tests::legibility_floor_switches_render_mode_on_both_sides`
    // — this module keeps only the paint-level regression that a
    // real Cairo/Pango pass actually respects that threshold.

    #[test]
    fn default_fixed_pitch_stays_below_the_floor_and_never_shapes_text() {
        // ROW_PITCH_PX (2px) is below LEGIBILITY_FLOOR_PX (4px), so the
        // default GTK minimap always lands in ColumnBlocks.
        let mm = minimap_from(vec!["fn main() {}"; 4], 4);
        let (cr, layout) = surface_and_layout();

        let _ = draw_minimap(&cr, &layout, 0.0, 0.0, 40.0, 4.0, &mm, &Theme::default());
        assert_eq!(layout.text(), "");

        let _ = draw_minimap(&cr, &layout, 0.0, 0.0, 40.0, 400.0, &mm, &Theme::default());
        assert_eq!(
            layout.text(),
            "",
            "the fixed pitch stays below the legibility floor regardless of strip height"
        );
    }

    // ── glyph atlas ─────────────────────────────────────────────────────

    /// Paint `mm` through [`draw_minimap_cached`] into a fresh surface and
    /// return the raw pixel bytes.
    fn paint_cached_pixels(mm: &Minimap, w: i32, h: i32) -> Vec<u8> {
        paint_cached_pixels_at_scale(mm, w, h, MinimapScale::One, &Theme::default())
    }

    /// [`paint_cached_pixels`], but at an explicit [`MinimapScale`] and
    /// [`Theme`].
    fn paint_cached_pixels_at_scale(
        mm: &Minimap,
        w: i32,
        h: i32,
        mm_scale: MinimapScale,
        theme: &Theme,
    ) -> Vec<u8> {
        let mut surface = ImageSurface::create(Format::ARgb32, w, h).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_minimap_cached(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                w as f64,
                h as f64,
                mm,
                theme,
                1.0,
                mm_scale,
            );
        }
        surface.flush();
        let pixels = surface.data().expect("surface data").to_vec();
        pixels
    }

    #[test]
    fn atlas_mode_differentiates_content_column_blocks_would_make_identical() {
        // Two rows of equal length, both entirely non-blank, must paint
        // *different* alpha patterns under real character shapes — solid
        // columns would paint identically for both.
        let code_line = "fn main() {";
        let comment_line = "///////////";
        assert_eq!(
            code_line.chars().count(),
            comment_line.chars().count(),
            "both lines must be the same length for this to be a fair comparison"
        );

        let code_pixels = paint_cached_pixels(&minimap_from(vec![code_line], 1), 40, 4);
        let comment_pixels = paint_cached_pixels(&minimap_from(vec![comment_line], 1), 40, 4);

        assert_ne!(
            code_pixels, comment_pixels,
            "Characters-mode (atlas) rows for different same-length, all-non-blank \
             content must paint different pixels"
        );
    }

    #[test]
    fn the_glyph_sample_sheet_does_not_pollute_the_text_run_sink() {
        // The sample sheet shapes each ASCII glyph into a private,
        // throwaway surface (never real screen content), but the
        // text-run sink is **thread-local**, not scoped to any
        // particular `Context` — the recording wrapper must never be
        // called here, else every atlas cache miss during a test would
        // append bogus bounds to the shared sink.
        let previous = crate::testing::install_text_run_sink();
        let chars: String = (' '..='~').collect();
        let _ = super::super::surface::glyph_alpha_sheet(
            "sans-serif",
            &chars,
            paint_pixels::SAMPLE_CELL,
        );
        let recorded = crate::testing::take_text_run_sink(previous);
        assert!(
            recorded.is_empty(),
            "the glyph sample sheet must never record into the text-run sink, got {recorded:?}"
        );
    }

    #[test]
    fn atlas_mode_paints_something_for_a_non_blank_line() {
        // A baseline sanity check alongside the differentiation test
        // above: a non-blank line must actually paint ink, not silently
        // no-op into an all-background surface (which would make the
        // "differentiates content" test above meaningless if both inputs
        // painted nothing).
        let bg = ImageSurface::create(Format::ARgb32, 40, 4)
            .and_then(|mut s| {
                let cr = CairoContext::new(&s)?;
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.paint()?;
                drop(cr); // release the surface borrow before `s.data()`
                s.flush();
                Ok(s.data().expect("surface data").to_vec())
            })
            .expect("blank surface");

        let painted = paint_cached_pixels(&minimap_from(vec!["fn main() {"], 1), 40, 4);
        assert_ne!(
            painted, bg,
            "a non-blank line must paint something other than a uniform background"
        );
    }

    /// At [`MinimapScale::Two`] (a 2px wide x 4px tall cell), a row of
    /// real code paints *shapes* — non-uniform pixel values across a
    /// character's own cell, not solid, uniformly-coloured blocks — and
    /// blank (whitespace) columns stay fully background throughout their cells.
    #[test]
    fn scale_two_atlas_mode_paints_a_shape_and_leaves_blank_columns_empty() {
        let line = "fn main()";
        let mm = minimap_from(vec![line], 1);
        let cell_w = MinimapScale::Two.cell_w_px() as i32;
        let row_h = MinimapScale::Two.row_pitch_px() as i32;
        let w = line.chars().count() as i32 * cell_w;

        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };

        let pixels = paint_cached_pixels_at_scale(&mm, w, row_h, MinimapScale::Two, &theme);
        let stride = ImageSurface::create(Format::ARgb32, w, row_h)
            .expect("create surface")
            .stride() as usize;
        let bg = (theme.background.r, theme.background.g, theme.background.b);

        // The space between "fn" and "main" is column 2 -- its whole
        // `cell_w`-wide, `row_h`-tall cell must stay fully background.
        let space_col = 2i32;
        for dx in 0..cell_w {
            for y in 0..row_h {
                let x = space_col * cell_w + dx;
                assert_eq!(
                    pixel(&pixels, stride, x, y),
                    bg,
                    "a blank column must stay fully background at ({x}, {y})"
                );
            }
        }

        // The first character's own cell ('f', column 0) must show a
        // *shape* — at least two pixels inside the cell with different
        // colours — rather than a solid, uniformly-filled block.
        let mut cell_pixels = Vec::new();
        for dx in 0..cell_w {
            for y in 0..row_h {
                cell_pixels.push(pixel(&pixels, stride, dx, y));
            }
        }
        assert!(
            cell_pixels.windows(2).any(|w| w[0] != w[1]),
            "expected non-uniform alpha (a glyph shape) across the 'f' cell, got {cell_pixels:?}"
        );
    }

    /// The thread's atlas cache is keyed on the tile size as well as the
    /// sample sheet, so a [`MinimapScale`] change rebuilds the atlas rather
    /// than serving the previous scale's smaller tile.
    #[test]
    fn atlas_cache_key_includes_minimap_scale_so_a_scale_change_rebuilds() {
        let mm = minimap_from(vec!["fn main() {}"], 1);
        let theme = Theme::default();
        let bg = (theme.background.r, theme.background.g, theme.background.b);
        let w = 40;
        let two_h = MinimapScale::Two.row_pitch_px() as i32;

        // Paint once at `One` (a 1x2 cell) through the shared cache...
        let surface_one =
            ImageSurface::create(Format::ARgb32, w, MinimapScale::One.row_pitch_px() as i32)
                .expect("create surface");
        {
            let cr = CairoContext::new(&surface_one).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_minimap_cached(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                w as f64,
                MinimapScale::One.row_pitch_px(),
                &mm,
                &theme,
                1.0,
                MinimapScale::One,
            );
        }

        // ...then at `Two` (a 2x4 cell) through the *same* cache instance.
        let mut surface_two =
            ImageSurface::create(Format::ARgb32, w, two_h).expect("create surface");
        {
            let cr = CairoContext::new(&surface_two).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_minimap_cached(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                w as f64,
                two_h as f64,
                &mm,
                &theme,
                1.0,
                MinimapScale::Two,
            );
        }
        surface_two.flush();
        let stride = surface_two.stride() as usize;
        let data = surface_two.data().expect("surface data");

        // If the cache had wrongly reused `One`'s 1x2 tile for this
        // `Two` paint, the blit would only ever touch the tile's own
        // (much smaller) device-pixel footprint, leaving the bottom of
        // this 4px-tall row untouched (still background). A correctly
        // rebuilt 2x4 tile paints something below row 2.
        let mut painted_below_row_2 = false;
        for y in 2..two_h {
            for x in 0..w {
                if pixel(&data, stride, x, y) != bg {
                    painted_below_row_2 = true;
                }
            }
        }
        assert!(
            painted_below_row_2,
            "MinimapScale::Two's own 4px-tall cell must paint below row 2 -- a stale \
             1x2 tile carried over from MinimapScale::One would not"
        );
    }

    #[test]
    fn column_blocks_skip_indentation_and_internal_gaps() {
        // 4-space indent, "x", 2-space internal gap, "y": the walk must
        // paint no block over the indent or the gap -- only over the two
        // non-blank columns, preserving the indent silhouette.
        let mm = minimap_from(vec!["    x  y"], 1);
        let theme = Theme {
            background: Color::rgb(10, 10, 10),
            foreground: Color::rgb(200, 200, 200),
            ..Theme::default()
        };

        let mut surface = ImageSurface::create(Format::ARgb32, 20, 4).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_minimap(&cr, &pango_layout, 0.0, 0.0, 20.0, 4.0, &mm, &theme);
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let bg = (theme.background.r, theme.background.g, theme.background.b);
        let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);

        for col in 0..4 {
            assert_eq!(
                pixel(&data, stride, col, 0),
                bg,
                "indent column {col} must stay blank"
            );
        }
        assert_eq!(
            pixel(&data, stride, 4, 0),
            fg,
            "'x' column must paint a block"
        );
        for col in 5..7 {
            assert_eq!(
                pixel(&data, stride, col, 0),
                bg,
                "gap column {col} must stay blank"
            );
        }
        assert_eq!(
            pixel(&data, stride, 7, 0),
            fg,
            "'y' column must paint a block"
        );
    }

    #[test]
    fn column_blocks_stop_at_the_column_capacity() {
        let long_line = "x".repeat(10_000);
        let mm = minimap_from(vec![&long_line], 1);
        let theme = Theme {
            background: Color::rgb(1, 1, 1),
            foreground: Color::rgb(250, 250, 250),
            ..Theme::default()
        };
        let strip_w = (COLUMN_CAPACITY + 50) as i32;

        let mut surface = ImageSurface::create(Format::ARgb32, strip_w, 4).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_minimap(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                strip_w as f64,
                4.0,
                &mm,
                &theme,
            );
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);
        let bg = (theme.background.r, theme.background.g, theme.background.b);
        assert_eq!(
            pixel(&data, stride, (COLUMN_CAPACITY - 1) as i32, 0),
            fg,
            "the last in-capacity column must still paint"
        );
        assert_eq!(
            pixel(&data, stride, COLUMN_CAPACITY as i32, 0),
            bg,
            "the walk must stop at COLUMN_CAPACITY and paint no further"
        );
    }

    // `color_at_column`/`SpanCursor`'s own behaviour is covered by
    // `primitives::minimap::tests::color_at_column_*` /
    // `span_cursor_matches_a_full_linear_scan_per_row` — nothing
    // GTK-specific left to re-test here.

    // ── paint/click round trip ────────────────────────────────────────

    /// Shared body for `paint_and_click_round_trip_returns_seek_for_the_clicked_fraction`
    /// — `minimap_layout` returns coordinates in absolute frame
    /// (see `docs/PRIMITIVE_RULES.md`), so `hit_test` must be called with
    /// coordinates shifted by the same `x`/`y` origin the strip was painted at.
    fn paint_and_click_round_trip_at(x: f64, y: f64) {
        let mm = minimap_from(vec!["x"; 8], 8);
        let (cr, pango_layout) = surface_and_layout();
        let layout = draw_minimap(
            &cr,
            &pango_layout,
            x,
            y,
            40.0,
            100.0,
            &mm,
            &Theme::default(),
        );
        assert_eq!(
            layout.hit_test((x + 20.0) as f32, (y + 50.0) as f32),
            MinimapHit::Seek { fraction: 0.5 }
        );
        assert_eq!(
            layout.hit_test((x + 20.0) as f32, y as f32),
            MinimapHit::Seek { fraction: 0.0 }
        );
    }

    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction() {
        paint_and_click_round_trip_at(0.0, 0.0);
    }

    /// Regression guard: `(x, y) = (0, 0)` is exactly the case where a
    /// LOCAL/ABSOLUTE mixup in `gtk_minimap_layout` would be invisible.
    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction_at_nonzero_origin() {
        paint_and_click_round_trip_at(7.0, 13.0);
    }

    #[test]
    fn empty_minimap_is_a_no_op() {
        let mm = minimap_from(vec![], 0);
        let (cr, pango_layout) = surface_and_layout();
        let layout = draw_minimap(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            40.0,
            100.0,
            &mm,
            &Theme::default(),
        );
        assert!(layout.visible_lines.is_empty());
    }

    // ── strip clipping ──────────────────────────────────────────────────

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    ///
    /// Cairo's `ARgb32` stores each pixel as four bytes in native
    /// (little-endian) byte order: [B, G, R, A].
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    #[test]
    fn wide_line_blocks_never_paint_right_of_bounds() {
        // Without `cr.clip()` and Pango layout width, a line wider than
        // the strip paints across whatever sat beside it. Paint a
        // pathologically long line into a strip embedded in a wider white
        // canvas and assert nothing right of the strip is touched.
        const STRIP_W: i32 = 40;
        const CANVAS_W: i32 = 200;
        const H: i32 = 40;

        let long_line = "x".repeat(500);
        let mm = minimap_from(vec![&long_line], 1);

        let mut surface =
            ImageSurface::create(Format::ARgb32, CANVAS_W, H).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0); // white sentinel
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let theme = Theme {
                background: Color::rgb(20, 20, 20),
                foreground: Color::rgb(255, 255, 255),
                ..Theme::default()
            };
            // The fixed pitch (ROW_PITCH_PX, 2px) is below the
            // legibility floor, so this exercises the ColumnBlocks
            // branch -- the column-capacity walk on its own already
            // bounds how far right it paints, but the clip is still the
            // hard guarantee this test checks.
            draw_minimap(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                STRIP_W as f64,
                H as f64,
                &mm,
                &theme,
            );
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        for y in 0..H {
            for x in STRIP_W..CANVAS_W {
                assert_eq!(
                    pixel(&data, stride, x, y),
                    (255, 255, 255),
                    "pixel ({x},{y}) is right of bounds.x + bounds.width and must be untouched"
                );
            }
        }
    }
}
