//! macOS rasteriser for [`crate::Completions`].
//!
//! Mirrors [`crate::gtk::completions::draw_completions`]: bordered
//! popup, per-item rows with the selected row highlighted in
//! `completion_selected_bg`.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::draw_text;
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::theme::Theme;
use crate::types::Color;

/// Paint a [`Completions`] popup at its resolved layout.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_completions(
    ctx: CGContextRef,
    font: &CTFont,
    completions: &Completions,
    layout: &CompletionsLayout,
    theme: &Theme,
) {
    let bounds = layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    fill_rect(ctx, bx, by, bw, bh, theme.completion_bg);
    stroke_rect(ctx, bx, by, bw, bh, theme.completion_border, 1.0);

    for vis in &layout.visible_items {
        let Some(item) = completions.items.get(vis.item_idx) else {
            continue;
        };
        let item_x = vis.bounds.x as f64;
        let item_y = vis.bounds.y as f64;
        let item_w = vis.bounds.width as f64;
        let item_h = vis.bounds.height as f64;

        if vis.item_idx == completions.selected_idx {
            fill_rect(
                ctx,
                item_x,
                item_y,
                item_w,
                item_h,
                theme.completion_selected_bg,
            );
        }

        let label = item
            .label
            .spans
            .first()
            .map(|s| s.text.as_str())
            .unwrap_or("");
        let display = format!(" {label}");
        draw_text(
            ctx,
            font,
            &display,
            item_x,
            item_y,
            color_to_cg(theme.completion_fg),
        );
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
    use crate::primitives::completions::{
        CompletionItem, CompletionItemMeasure, CompletionKind, CompletionsHit,
    };
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample() -> Completions {
        Completions {
            id: WidgetId::new("comp"),
            items: vec![
                CompletionItem {
                    label: StyledText::plain("len"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Method,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("clone"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Method,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("map"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Method,
                    icon: None,
                },
            ],
            selected_idx: 1,
            scroll_offset: 0,
            has_focus: true,
        }
    }

    fn layout_for(c: &Completions) -> CompletionsLayout {
        c.layout(
            20.0,
            20.0,
            16.0,
            QRect::new(0.0, 0.0, W as f32, H as f32),
            120.0,
            80.0,
            |_| CompletionItemMeasure::new(16.0),
        )
    }

    fn paint_via_backend(c: &Completions, l: &CompletionsLayout) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_completions(c, l);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn popup_paints_completion_bg() {
        let c = sample();
        let l = layout_for(&c);
        let surface = paint_via_backend(&c, &l);
        let theme = Theme::default();
        // Probe near bottom-right of the popup interior.
        let px = (l.bounds.x + l.bounds.width - 4.0) as u32;
        let py = (l.bounds.y + l.bounds.height - 4.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.completion_bg.r,
                theme.completion_bg.g,
                theme.completion_bg.b
            ),
        );
    }

    #[test]
    fn selected_item_paints_completion_selected_bg() {
        let c = sample();
        let l = layout_for(&c);
        let surface = paint_via_backend(&c, &l);
        let theme = Theme::default();
        // selected_idx = 1 — find the visible item with idx 1.
        let item = l
            .visible_items
            .iter()
            .find(|v| v.item_idx == 1)
            .expect("selected item visible");
        let px = (item.bounds.x + item.bounds.width - 4.0) as u32;
        let py = (item.bounds.y + item.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.completion_selected_bg.r,
                theme.completion_selected_bg.g,
                theme.completion_selected_bg.b
            ),
        );
    }

    #[test]
    fn hit_test_resolves_visible_items() {
        let c = sample();
        let l = layout_for(&c);
        for vis in &l.visible_items {
            let cx = vis.bounds.x + vis.bounds.width * 0.5;
            let cy = vis.bounds.y + vis.bounds.height * 0.5;
            assert_eq!(l.hit_test(cx, cy), CompletionsHit::Item(vis.item_idx));
        }
    }
}
