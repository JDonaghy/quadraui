//! macOS rasteriser for [`crate::Tooltip`].
//!
//! Mirrors [`crate::gtk::tooltip::draw_tooltip`]: a filled background
//! rectangle at the tooltip's resolved bounds, then border chrome per
//! `tooltip.border` (#541 — [`crate::TooltipBorder`]):
//!
//! - [`TooltipBorder::Full`] (the default) strokes a full 4-sided box —
//!   this backend has always done this, unconditionally, before #541
//!   gave it a name (it was one of the three backends the original issue
//!   flagged as "not verified in detail" beyond a `fill_rect` call —
//!   confirmed here to be a `stroke_rect`, same as GTK). An optional
//!   `tooltip.title` is centred over the top edge, punched through the
//!   stroke with a background-coloured backing rectangle so it reads as
//!   embedded in the border, mirroring the TUI and GTK rasterisers.
//! - [`TooltipBorder::Sides`] strokes two vertical lines at the left and
//!   right edges only, no top/bottom. No title (no top rule).
//! - [`TooltipBorder::None`] strokes nothing.
//!
//! Then draws either the plain `text` or per-row `styled_lines`.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::tooltip::{Tooltip, TooltipBorder, TooltipLayout};
use crate::theme::Theme;
use crate::types::Color;

/// Draw a [`Tooltip`] at its resolved layout position.
///
/// `padding_x` is the horizontal padding from the left border to the
/// start of text — consumers typically pass `char_width`. Halved when
/// `tooltip.border` is [`TooltipBorder::None`], since there is no border
/// column to clear first — mirrors the TUI/GTK rasterisers.
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

    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    fill_rect(ctx, bx, by, bw, bh, bg);

    // Content normally starts 2pt below the top edge; a title pushes
    // that down further, since its real font height (title_h) is
    // typically much taller than the 1pt border line it's centred on —
    // without this, a title would visually collide with the first
    // content row instead of sitting in its own space above it, the way
    // the TUI rasteriser's dedicated title row never overlaps content.
    let mut text_top = by + 2.0;

    match tooltip.border {
        TooltipBorder::Full => {
            stroke_rect(ctx, bx, by, bw, bh, border, 1.0);

            if let Some(title) = tooltip
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                let (title_w, title_h) = measure_text(font, title);
                let pad = 4.0;
                let title_x = bx + ((bw - title_w) / 2.0).max(0.0);
                let title_y = by - title_h / 2.0;

                // Punch a background-coloured gap through the border
                // stroke so the title reads as embedded in the top rule,
                // not a content row sitting on top of it — mirrors the
                // GTK rasteriser.
                fill_rect(
                    ctx,
                    title_x - pad,
                    title_y,
                    title_w + pad * 2.0,
                    title_h,
                    bg,
                );
                draw_text(ctx, font, title, title_x, title_y, color_to_cg(fg));

                text_top = text_top.max(title_y + title_h + 2.0);
            }
        }
        TooltipBorder::Sides => {
            stroke_line(ctx, bx, by, bx, by + bh, border, 1.0);
            stroke_line(ctx, bx + bw, by, bx + bw, by + bh, border, 1.0);
        }
        TooltipBorder::None => {}
    }

    let text_padding_x = if matches!(tooltip.border, TooltipBorder::None) {
        padding_x / 2.0
    } else {
        padding_x
    };
    let text_x = bx + text_padding_x;

    if let Some(ref styled_lines) = tooltip.styled_lines {
        for (i, styled) in styled_lines.iter().enumerate() {
            let row_y = text_top + i as f64 * line_height;
            if row_y + line_height > by + bh {
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
        if row_y + line_height > by + bh {
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

/// Stroke a single line segment — used by [`TooltipBorder::Sides`] to
/// paint the left/right edges without the top/bottom rules `stroke_rect`
/// would also draw.
unsafe fn stroke_line(
    ctx: CGContextRef,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    c: Color,
    line_width: f64,
) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    CGContextMoveToPoint(ctx, x0, y0);
    CGContextAddLineToPoint(ctx, x1, y1);
    CGContextStrokePath(ctx);
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
    fn CGContextMoveToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    fn CGContextAddLineToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    fn CGContextStrokePath(c: CGContextRef);
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
        sample_tooltip_with(TooltipBorder::default(), None)
    }

    fn sample_tooltip_with(border: TooltipBorder, title: Option<&str>) -> Tooltip {
        Tooltip {
            id: WidgetId::new("tip"),
            text: "Hover hint".into(),
            styled_lines: None,
            placement: TooltipPlacement::Bottom,
            border,
            title: title.map(str::to_string),
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

    // ── #541: explicit border vocabulary — same coverage as
    // `gtk::tooltip::tests`, since ask 4 was to confirm this backend
    // wasn't just a bare `fill_rect` call (it turned out to already be a
    // `stroke_rect`, matching GTK) and to bring it up to the same
    // vocabulary once confirmed.

    /// A pure-background reference pixel: bottom-right corner, a few
    /// pixels in from both edges — clear of any border stroke (which
    /// hugs the very edge) and of `sample_tooltip`'s short, top-aligned
    /// "Hover hint" body text.
    fn bg_reference(surface: &BitmapSurface, bounds: QRect) -> (u8, u8, u8) {
        let (r, g, b, _) = surface.pixel(
            bounds.x as u32 + bounds.width as u32 - 4,
            bounds.y as u32 + bounds.height as u32 - 4,
        );
        (r, g, b)
    }

    #[test]
    fn sides_border_only_strokes_left_and_right() {
        let tip = sample_tooltip_with(TooltipBorder::Sides, None);
        let layout = sample_layout();
        let surface = paint(&tip, &layout);
        let b = layout.bounds;
        let (bx, by, bw, bh) = (b.x as u32, b.y as u32, b.width as u32, b.height as u32);

        let inner = bg_reference(&surface, b);
        let (top_r, top_g, top_b, _) = surface.pixel(bx + bw / 2, by);
        let (bottom_r, bottom_g, bottom_b, _) = surface.pixel(bx + bw / 2, by + bh - 1);
        let (left_r, left_g, left_b, _) = surface.pixel(bx, by + bh / 2);
        let (right_r, right_g, right_b, _) = surface.pixel(bx + bw - 1, by + bh / 2);

        assert_eq!(
            (top_r, top_g, top_b),
            inner,
            "TooltipBorder::Sides must not stroke the top edge — no top/bottom rule, \
             regardless of box height"
        );
        assert_eq!(
            (bottom_r, bottom_g, bottom_b),
            inner,
            "TooltipBorder::Sides must not stroke the bottom edge"
        );
        assert_ne!(
            (left_r, left_g, left_b),
            inner,
            "TooltipBorder::Sides must still stroke the left edge"
        );
        assert_ne!(
            (right_r, right_g, right_b),
            inner,
            "TooltipBorder::Sides must still stroke the right edge"
        );
    }

    #[test]
    fn none_border_strokes_nothing() {
        let tip = sample_tooltip_with(TooltipBorder::None, None);
        let layout = sample_layout();
        let surface = paint(&tip, &layout);
        let b = layout.bounds;
        let (bx, by, bw, bh) = (b.x as u32, b.y as u32, b.width as u32, b.height as u32);

        let inner = bg_reference(&surface, b);
        for (name, (r, g, bl, _)) in [
            ("top", surface.pixel(bx + bw / 2, by)),
            ("bottom", surface.pixel(bx + bw / 2, by + bh - 1)),
            ("left", surface.pixel(bx, by + bh / 2)),
            ("right", surface.pixel(bx + bw - 1, by + bh / 2)),
        ] {
            assert_eq!(
                (r, g, bl),
                inner,
                "TooltipBorder::None must not stroke {name} — no chrome at all"
            );
        }
    }

    /// #541 ask 2: a title punches a background-coloured gap through the
    /// top stroke so it reads as embedded in the border, not a content
    /// row. Scans the top edge rather than probing one fixed x — the
    /// title is horizontally centred, so a single dead-centre sample can
    /// land on a glyph's own ink instead of the cleared pad around it
    /// (same reasoning as the GTK rasteriser's equivalent test).
    #[test]
    fn full_border_title_punches_a_gap_but_leaves_the_rest_of_the_top_edge_stroked() {
        let tip = sample_tooltip_with(TooltipBorder::Full, Some("Hi"));
        let layout = sample_layout();
        let surface = paint(&tip, &layout);
        let b = layout.bounds;
        let (bx, by, bw) = (b.x as u32, b.y as u32, b.width as u32);

        let inner = bg_reference(&surface, b);
        let margin = 3u32;
        let interior: Vec<(u8, u8, u8)> = (bx + margin..bx + bw - margin)
            .map(|x| {
                let (r, g, bl, _) = surface.pixel(x, by);
                (r, g, bl)
            })
            .collect();

        assert!(
            interior.contains(&inner),
            "the title's short label + padding should punch at least one background-\
             coloured pixel into the top edge somewhere in its interior (inner={inner:?}, \
             top-edge interior samples={interior:?})"
        );

        let (corner_r, corner_g, corner_b, _) = surface.pixel(bx + 1, by);
        assert_ne!(
            (corner_r, corner_g, corner_b),
            inner,
            "the top edge right at the corner should still show the plain border stroke"
        );
    }

    #[test]
    fn title_is_ignored_when_border_is_sides_or_none() {
        for border in [TooltipBorder::Sides, TooltipBorder::None] {
            let with_title = sample_tooltip_with(border, Some("Ignored"));
            let without_title = sample_tooltip_with(border, None);
            let layout = sample_layout();
            let with_surface = paint(&with_title, &layout);
            let without_surface = paint(&without_title, &layout);
            let b = layout.bounds;
            let (bx, by, bw) = (b.x as u32, b.y as u32, b.width as u32);
            let top_with = with_surface.pixel(bx + bw / 2, by);
            let top_without = without_surface.pixel(bx + bw / 2, by);
            assert_eq!(
                top_with, top_without,
                "{border:?}: setting `title` must not change what's painted at the top edge"
            );
        }
    }
}
