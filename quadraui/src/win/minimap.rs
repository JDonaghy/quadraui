//! Direct2D entry points for [`Minimap`] — thin delegates onto the
//! widget's own pixel painter, `crate::primitives::minimap::paint_pixels`,
//! which documents the paint passes. These functions only adapt a render
//! target and [`DWrite`] handle into the [`super::surface::D2dSurface`]
//! that painter takes. That surface rasterises no glyph sample sheet, so
//! every row paints as column blocks.
//!
//! Only compiled on `target_os = "windows"`, like the rest of the
//! Direct2D rasterisers.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::surface::D2dSurface;
use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::minimap::paint_pixels::{self, Glyphs};
use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapScale};
use crate::theme::Theme;

/// The DIP layout of `minimap` in `rect` without painting, at
/// [`MinimapScale::One`]. `rect.x`/`rect.y` are baked into every bound.
pub fn win_minimap_layout(minimap: &Minimap, rect: Rect) -> MinimapLayout {
    win_minimap_layout_scaled(minimap, rect, MinimapScale::One)
}

/// [`win_minimap_layout`] at an explicit [`MinimapScale`] — see
/// [`Minimap::pixel_layout`].
pub(crate) fn win_minimap_layout_scaled(
    minimap: &Minimap,
    rect: Rect,
    scale: MinimapScale,
) -> MinimapLayout {
    minimap.pixel_layout(rect, scale, None)
}

/// Draw `minimap` into `rect` (DIPs, target-relative) on `target` at
/// [`MinimapScale::One`]. Returns the layout for host click dispatch
/// (`layout.hit_test(x, y)`).
pub fn draw_minimap(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    draw_minimap_scaled(target, dwrite, rect, minimap, theme, MinimapScale::One)
}

/// [`draw_minimap`] at an explicit [`MinimapScale`].
pub(crate) fn draw_minimap_scaled(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
    scale: MinimapScale,
) -> MinimapLayout {
    let layout = win_minimap_layout_scaled(minimap, rect, scale);
    let mut surface = D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    paint_pixels::paint(
        minimap,
        &layout,
        &mut surface,
        theme,
        scale,
        Glyphs::Atlas { dpi_scale: 1.0 },
    );
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::minimap::{MinimapHit, MinimapLine, ROW_PITCH_PX};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

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

    /// C0 smoke: `draw_minimap` must actually paint pixels and return a
    /// click-routable layout. The default fixed pitch stays below the legibility
    /// floor, so this exercises `ColumnBlocks`, not `DrawText`.
    #[test]
    fn draw_minimap_paints_column_blocks_and_returns_layout() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_minimap(target, &dwrite, rect, &mm, &theme);
            })
            .map(|_| win_minimap_layout(&mm, rect))
            .expect("paint minimap");

        assert!(!layout.visible_lines.is_empty());

        let mut painted_any = false;
        for x in 0..W as u32 {
            for y in 0..(ROW_PITCH_PX as u32 * 8).max(8) {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected draw_minimap to paint visible blocks");
    }

    /// Paint↔click round trip at a non-zero origin — regression guard for
    /// LOCAL/ABSOLUTE mixup, mirrored from `win::board`/`win::diff_view`'s tests.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        let origin_x = 7.0_f32;
        let origin_y = 13.0_f32;
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let mm = minimap_from(vec!["x"; 8], 8);
        let rect = Rect::new(origin_x, origin_y, 40.0, 100.0);

        let layout = surface
            .paint(|target| {
                draw_minimap(target, &dwrite, rect, &mm, &theme);
            })
            .map(|_| win_minimap_layout(&mm, rect))
            .expect("paint minimap");

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
    /// painted — same contract every other `win::` rasteriser's
    /// `no_paint_layout_matches_paint_layout` test proves.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_minimap(target, &dwrite, rect, &mm, &Theme::default());
            })
            .map(|_| win_minimap_layout(&mm, rect))
            .expect("paint");
        let no_paint = win_minimap_layout(&mm, rect);
        assert_eq!(painted, no_paint);
    }

    /// Zero-size rect is a no-op — mirrors every other `win::` rasteriser's
    /// same guard.
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        surface
            .paint(|target| {
                draw_minimap(target, &dwrite, rect, &mm, &theme);
            })
            .expect("paint minimap");

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width minimap should paint nothing at all",
        );
    }

    /// The fixed pitch stays independent of both the strip height and the
    /// file length — same parity guarantee `gtk::minimap`'s tests pin, now
    /// proven on Win-GUI's own `win_minimap_layout`.
    #[test]
    fn row_pitch_is_independent_of_strip_height_and_file_length() {
        let short = minimap_from(vec!["fn main() {}"; 3], 3);
        let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
        let long = minimap_from(long_refs, 10_000);

        let tall = win_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 800.0));
        let short_strip = win_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 16.0));
        let long_layout = win_minimap_layout(&long, Rect::new(0.0, 0.0, 40.0, 400.0));

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

    /// At [`MinimapScale::Two`] each non-blank column paints a 2-DIP-wide
    /// block, and a blank column stays unpainted.
    #[test]
    fn scale_two_paints_two_dip_column_blocks() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let mm = minimap_from(vec!["fn main() {}"], 1);
        let rect = Rect::new(0.0, 0.0, W, H);
        surface
            .paint(|target| {
                draw_minimap_scaled(target, &dwrite, rect, &mm, &theme, MinimapScale::Two);
            })
            .expect("paint minimap");
        for x in 0..4u32 {
            let px = surface.pixel_at(x, 1);
            assert_eq!((px.r, px.g, px.b), (0, 0, 0), "'fn' column at x={x}");
        }
        for x in 4..6u32 {
            let px = surface.pixel_at(x, 1);
            assert_eq!((px.r, px.g, px.b), (255, 255, 255), "blank column at x={x}");
        }
    }
}
