//! macOS rasteriser for [`crate::Tooltip`].
//!
//! Mirrors [`crate::gtk::tooltip::draw_tooltip`]: bordered rectangle
//! at the tooltip's resolved bounds, then plain `text` or per-row
//! `styled_lines`.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::theme::Theme;
use crate::types::Color;

/// Draw a [`Tooltip`] at its resolved layout position.
///
/// `padding_x` is the horizontal padding from the left border to the
/// start of text — consumers typically pass `char_width`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_tooltip(
    ctx: CGContextRef,
    font: &CTFont,
    tooltip: &Tooltip,
    tooltip_layout: &TooltipLayout,
    line_height: f64,
    padding_x: f64,
    theme: &Theme,
) {
    let bounds = tooltip_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }

    let bg = tooltip.bg.unwrap_or(theme.hover_bg);
    let fg = tooltip.fg.unwrap_or(theme.hover_fg);
    let border = theme.hover_border;

    fill_rect(
        ctx,
        bounds.x as f64,
        bounds.y as f64,
        bounds.width as f64,
        bounds.height as f64,
        bg,
    );

    stroke_rect(
        ctx,
        bounds.x as f64,
        bounds.y as f64,
        bounds.width as f64,
        bounds.height as f64,
        border,
        1.0,
    );

    let text_x = bounds.x as f64 + padding_x;
    let text_top = bounds.y as f64 + 2.0;

    if let Some(ref styled_lines) = tooltip.styled_lines {
        for (i, styled) in styled_lines.iter().enumerate() {
            let row_y = text_top + i as f64 * line_height;
            if row_y + line_height > bounds.y as f64 + bounds.height as f64 {
                break;
            }
            let mut x_off = text_x;
            for span in &styled.spans {
                let span_fg = span.fg.unwrap_or(fg);
                draw_text(ctx, font, &span.text, x_off, row_y, color_to_cg(span_fg));
                let (sw, _) = measure_text(font, &span.text);
                x_off += sw;
            }
        }
        return;
    }

    for (i, text_line) in tooltip.text.lines().enumerate() {
        let row_y = text_top + i as f64 * line_height;
        if row_y + line_height > bounds.y as f64 + bounds.height as f64 {
            break;
        }
        draw_text(ctx, font, text_line, text_x, row_y, color_to_cg(fg));
    }
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

unsafe fn stroke_rect(
    ctx: CGContextRef,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    c: Color,
    line_width: f64,
) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextStrokeRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetLineWidth(c: CGContextRef, w: core_graphics::base::CGFloat);
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextStrokeRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::tooltip::{ResolvedPlacement, Tooltip, TooltipLayout, TooltipPlacement};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 60;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_tooltip() -> Tooltip {
        Tooltip {
            id: WidgetId::new("tip"),
            text: "Hover hint".into(),
            styled_lines: None,
            placement: TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        }
    }

    fn sample_layout() -> TooltipLayout {
        TooltipLayout {
            bounds: QRect::new(10.0, 10.0, 120.0, 24.0),
            resolved_placement: ResolvedPlacement::Bottom,
        }
    }

    fn paint(tip: &Tooltip, layout: &TooltipLayout) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_tooltip(tip, layout);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn tooltip_paints_hover_bg() {
        let tip = sample_tooltip();
        let layout = sample_layout();
        let surface = paint(&tip, &layout);
        let theme = Theme::default();
        // Probe near right edge of bounds — glyph-free zone.
        let bx = layout.bounds.x as u32;
        let by = layout.bounds.y as u32;
        let bw = layout.bounds.width as u32;
        let bh = layout.bounds.height as u32;
        let (r, g, b, _) = surface.pixel(bx + bw - 4, by + bh / 2);
        assert_eq!(
            (r, g, b),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
        );
    }

    #[test]
    fn tooltip_border_paints_at_edge() {
        let tip = sample_tooltip();
        let layout = sample_layout();
        let surface = paint(&tip, &layout);
        let theme = Theme::default();
        // The 1pt border stroke centres on the rect's top edge, so
        // the edge pixel is anti-aliased ~50/50 between border ink
        // and tooltip bg. Verify the edge pixel differs from the
        // pure bg fill (probed at +2 px below the top edge, well
        // inside the bg region).
        // Probe near the right edge, away from "Hover hint" glyphs.
        let bx = layout.bounds.x as u32;
        let by = layout.bounds.y as u32;
        let bw = layout.bounds.width as u32;
        let (edge_r, edge_g, edge_b, _) = surface.pixel(bx + bw - 4, by);
        let (inner_r, inner_g, inner_b, _) = surface.pixel(bx + bw - 4, by + 4);
        assert_eq!(
            (inner_r, inner_g, inner_b),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
            "inner pixel should be pure bg",
        );
        assert_ne!(
            (edge_r, edge_g, edge_b),
            (inner_r, inner_g, inner_b),
            "edge pixel should differ from bg (border ink present)",
        );
    }

    #[test]
    fn tooltip_with_custom_bg_overrides_theme() {
        let mut tip = sample_tooltip();
        tip.bg = Some(crate::types::Color::rgb(50, 100, 150));
        let layout = sample_layout();
        let surface = paint(&tip, &layout);
        let bx = layout.bounds.x as u32;
        let by = layout.bounds.y as u32;
        let bw = layout.bounds.width as u32;
        let bh = layout.bounds.height as u32;
        let (r, g, b, _) = surface.pixel(bx + bw - 4, by + bh / 2);
        assert_eq!((r, g, b), (50, 100, 150));
    }

    #[test]
    fn empty_bounds_no_op() {
        let tip = sample_tooltip();
        let layout = TooltipLayout {
            bounds: QRect::new(10.0, 10.0, 0.0, 0.0),
            resolved_placement: ResolvedPlacement::Bottom,
        };
        let surface = paint(&tip, &layout);
        // Surface stays all-zero.
        let (r, g, b, _) = surface.pixel(10, 10);
        assert_eq!((r, g, b), (0, 0, 0));
    }
}
