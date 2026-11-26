//! macOS entry points for [`Minimap`] — thin delegates onto the
//! widget's own pixel painter, `crate::primitives::minimap::paint_pixels`,
//! which documents the paint passes. These functions only adapt a bare
//! `CGContextRef` and a [`CTFont`] into the
//! [`super::surface::CgSurface`] that painter takes; the glyph atlas is
//! sampled in that font's family.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::surface::CgSurface;
use crate::event::Rect;
use crate::primitives::minimap::paint_pixels::{self, Glyphs};
use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapScale};
use crate::theme::Theme;

/// The pixel layout of `minimap` in `rect` without painting, at
/// [`MinimapScale::One`]. `rect.x`/`rect.y` are baked into every bound.
pub fn mac_minimap_layout(minimap: &Minimap, rect: Rect) -> MinimapLayout {
    mac_minimap_layout_scaled(minimap, rect, MinimapScale::One)
}

/// [`mac_minimap_layout`] at an explicit [`MinimapScale`] — see
/// [`Minimap::pixel_layout`].
pub(crate) fn mac_minimap_layout_scaled(
    minimap: &Minimap,
    rect: Rect,
    scale: MinimapScale,
) -> MinimapLayout {
    minimap.pixel_layout(rect, scale, None)
}

/// # Safety
///
/// `ctx` must be a valid `CGContextRef` for the duration of the call.
unsafe fn paint(
    ctx: CGContextRef,
    font: &CTFont,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
    scale: MinimapScale,
    glyphs: Glyphs,
) -> MinimapLayout {
    let layout = mac_minimap_layout_scaled(minimap, rect, scale);
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }
    let mut surface = CgSurface {
        ctx,
        font: Some(font),
    };
    paint_pixels::paint(minimap, &layout, &mut surface, theme, scale, glyphs);
    layout
}

/// Draw `minimap` into `rect` at [`MinimapScale::One`] as column blocks,
/// without the glyph atlas. Returns the layout for host click dispatch
/// (`layout.hit_test(x, y)`).
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of this
/// call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
pub unsafe fn draw_minimap(
    ctx: CGContextRef,
    font: &CTFont,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    paint(
        ctx,
        font,
        rect,
        minimap,
        theme,
        MinimapScale::One,
        Glyphs::Blocks,
    )
}

/// Draw `minimap` at `scale` with glyph-atlas character shapes sampled
/// from `font`'s family, each tile `dpi_scale` device pixels per point.
///
/// # Safety
///
/// Same contract as [`draw_minimap`].
pub(crate) unsafe fn draw_minimap_cached(
    ctx: CGContextRef,
    font: &CTFont,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
    scale: MinimapScale,
    dpi_scale: f64,
) -> MinimapLayout {
    paint(
        ctx,
        font,
        rect,
        minimap,
        theme,
        scale,
        Glyphs::Atlas {
            dpi_scale: dpi_scale as f32,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macos::headless::BitmapSurface;
    use crate::macos::text::make_font;
    use crate::primitives::minimap::{MinimapHit, MinimapLine, ROW_PITCH_PX};
    use crate::types::{Color, WidgetId};

    const W: f32 = 200.0;
    const H: f32 = 200.0;

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
            scale: Default::default(),
            sizing: None,
        }
    }

    fn test_font() -> CTFont {
        make_font("Menlo", 10.0).expect("Menlo should be installed on every macOS host")
    }

    /// C0 smoke: `draw_minimap` must actually paint pixels + return a
    /// click-routable layout. The default fixed pitch stays below the
    /// legibility floor, so this exercises `ColumnBlocks`, not glyph shaping —
    /// the "non-background pixel actually painted" check every other `macos::`
    /// C0 smoke uses stands in for "text_ok" here.
    #[test]
    fn draw_minimap_paints_column_blocks_and_returns_layout() {
        let surface = BitmapSurface::new(W as u32, H as u32);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let font = test_font();
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = unsafe { draw_minimap(surface.context_ptr(), &font, rect, &mm, &theme) };

        assert!(!layout.visible_lines.is_empty());

        let mut painted_any = false;
        for x in 0..W as u32 {
            for y in 0..(ROW_PITCH_PX as u32 * 8).max(8) {
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected draw_minimap to paint visible blocks");
    }

    /// Paint↔click round trip at a non-zero origin — tests the invariant
    /// that LOCAL coordinates are used consistently, mirrored from
    /// `macos::board`/`win::minimap`'s own nonzero-origin tests.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        let origin_x = 7.0_f32;
        let origin_y = 13.0_f32;
        let surface = BitmapSurface::new(W as u32, H as u32);
        let font = test_font();
        let theme = Theme::default();
        let mm = minimap_from(vec!["x"; 8], 8);
        let rect = Rect::new(origin_x, origin_y, 40.0, 100.0);

        let layout = unsafe { draw_minimap(surface.context_ptr(), &font, rect, &mm, &theme) };

        assert_eq!(
            layout.hit_test(origin_x + 20.0, origin_y + 50.0),
            MinimapHit::Seek { fraction: 0.5 }
        );
        assert_eq!(
            layout.hit_test(origin_x + 20.0, origin_y),
            MinimapHit::Seek { fraction: 0.0 }
        );
    }

    /// No-paint layout must agree byte-for-byte with what `draw_minimap`
    /// painted — same contract every other backend's
    /// `no_paint_layout_matches_paint_layout` test proves.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, W, H);
        let font = test_font();
        let surface = BitmapSurface::new(W as u32, H as u32);

        let painted =
            unsafe { draw_minimap(surface.context_ptr(), &font, rect, &mm, &Theme::default()) };
        let no_paint = mac_minimap_layout(&mm, rect);
        assert_eq!(painted, no_paint);
    }

    /// Zero-size rect is a no-op — mirrors every other backend's same
    /// guard.
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let surface = BitmapSurface::new(W as u32, H as u32);
        let font = test_font();
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface.fill(1.0, 1.0, 1.0, 1.0);

        unsafe {
            draw_minimap(surface.context_ptr(), &font, rect, &mm, &theme);
        }

        let (r, g, b, _) = surface.pixel(1, 1);
        assert_eq!(
            (r, g, b),
            (255, 255, 255),
            "a zero-width minimap should paint nothing at all",
        );
    }

    /// The fixed pitch stays independent of both the strip height and the
    /// file length — same parity guarantee `gtk::minimap`'s/`win::minimap`'s
    /// `row_pitch_is_independent_of_the_strip_height` /
    /// `_of_the_file_length` tests pin, now proven on macOS's own
    /// `mac_minimap_layout`.
    #[test]
    fn row_pitch_is_independent_of_strip_height_and_file_length() {
        let short = minimap_from(vec!["fn main() {}"; 3], 3);
        let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
        let long = minimap_from(long_refs, 10_000);

        let tall = mac_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 800.0));
        let short_strip = mac_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 16.0));
        let long_layout = mac_minimap_layout(&long, Rect::new(0.0, 0.0, 40.0, 400.0));

        assert_eq!(tall.visible_lines[0].bounds.height as f64, ROW_PITCH_PX);
        assert_eq!(
            short_strip.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
        assert_eq!(
            long_layout.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
    }

    // ── glyph atlas ──────────────────────────────────────────────────────

    /// At [`MinimapScale::Two`] the atlas keeps every row's ink inside its
    /// own row band. Row 1 here is blank, so *any* ink in its band can
    /// only be row 0's glyph bleeding past its own row.
    #[test]
    fn atlas_mode_keeps_each_row_within_its_own_band() {
        let code_line = "fn main()";
        let mm = minimap_from(vec![code_line, ""], 2);
        let scale = MinimapScale::Two;
        let cell_w = scale.cell_w_px() as u32;
        let row_h = scale.row_pitch_px() as u32;
        let w = (code_line.chars().count() as u32) * cell_w;
        let h = row_h * 2;

        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };

        let surface = BitmapSurface::new(w, h);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let font = test_font();
        let rect = Rect::new(0.0, 0.0, w as f32, h as f32);

        let layout = unsafe {
            draw_minimap_cached(surface.context_ptr(), &font, rect, &mm, &theme, scale, 1.0)
        };
        assert_eq!(
            layout.visible_lines.len(),
            2,
            "expected exactly 2 visible rows"
        );

        // Row 1 (blank) must stay fully background across its whole band
        // — any ink there is row 0's glyph tiles bleeding past their own row.
        for y in row_h..h {
            for x in 0..w {
                let (r, g, b, _) = surface.pixel(x, y);
                assert_eq!(
                    (r, g, b),
                    (255, 255, 255),
                    "row 1 (blank) must stay fully background at ({x}, {y}) -- \
                     any ink here means row 0 bled past its own row band"
                );
            }
        }

        // Sanity: row 0 must have actually painted *something*, or the
        // containment check above would be vacuous.
        let mut painted_row0 = false;
        for y in 0..row_h {
            for x in 0..w {
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) != (255, 255, 255) {
                    painted_row0 = true;
                }
            }
        }
        assert!(painted_row0, "expected row 0 to paint something");
    }

    /// At [`MinimapScale::Two`], a row of real code paints *shapes* —
    /// non-uniform pixel values across a character's own cell, not a
    /// solid, uniformly-coloured block — mirroring
    /// `gtk::minimap::scale_two_atlas_mode_paints_a_shape_and_leaves_blank_columns_empty`.
    #[test]
    fn atlas_mode_paints_a_shape_not_a_solid_block() {
        let line = "fn main()";
        let mm = minimap_from(vec![line], 1);
        let scale = MinimapScale::Two;
        let cell_w = scale.cell_w_px() as u32;
        let row_h = scale.row_pitch_px() as u32;
        let w = line.chars().count() as u32 * cell_w;

        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };

        let surface = BitmapSurface::new(w, row_h);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let font = test_font();
        let rect = Rect::new(0.0, 0.0, w as f32, row_h as f32);

        unsafe {
            draw_minimap_cached(surface.context_ptr(), &font, rect, &mm, &theme, scale, 1.0);
        }

        // The first character's own cell ('f', column 0) must show a
        // shape -- at least two pixels inside the cell with different
        // colours -- rather than a solid, uniformly-filled block.
        let mut cell_pixels = Vec::new();
        for dx in 0..cell_w {
            for y in 0..row_h {
                cell_pixels.push(surface.pixel(dx, y));
            }
        }
        let distinct: std::collections::HashSet<_> = cell_pixels.iter().collect();
        assert!(
            distinct.len() > 1,
            "expected the 'f' cell to show a shape (multiple distinct pixel values), got {cell_pixels:?}"
        );
    }

    /// The sample sheet shapes each ASCII glyph
    /// into is a private, throwaway surface (never real screen content), but
    /// `crate::testing`'s text-run sink is a **thread-local**, not scoped to
    /// any particular `CGContextRef` — so if this function ever stopped
    /// pausing/discarding recording around its `draw_text` calls, every atlas
    /// cache miss during a `MacDriver`-based test (guaranteed on a fresh
    /// backend's first paint) would silently append all `ATLAS_CHAR_COUNT`
    /// sample glyphs' bogus bounds to the shared sink that backs
    /// `MacDriver::find`/`screen_contains`.
    #[test]
    fn the_glyph_sample_sheet_does_not_pollute_the_text_run_sink() {
        let previous = crate::testing::install_text_run_sink();
        let chars: String = (' '..='~').collect();
        let _ =
            crate::macos::surface::glyph_alpha_sheet("Menlo", &chars, paint_pixels::SAMPLE_CELL);
        let recorded = crate::testing::take_text_run_sink(previous);
        assert!(
            recorded.is_empty(),
            "the glyph sample sheet must never record into the text-run sink, got {recorded:?}"
        );
    }
}
