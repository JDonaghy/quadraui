//! macOS rasteriser for [`crate::ToastStack`].
//!
//! Paints toast notification boxes stacked in a viewport corner. Each
//! toast is a rect with title, optional body, severity tint, dismiss
//! `×`, and optional action button label.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Rounded corners** — boxes are straight rectangles for now. CG
//!   path API for rounded rects deferred with other corner work
//!   (search-box border in command_center, close-button hover bg in
//!   tab_bar).

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::toast::{
    ToastItem, ToastMeasure, ToastSeverity, ToastStack, ToastStackLayout, VisibleToast,
};
use crate::theme::Theme;
use crate::types::Color;

const TOAST_WIDTH_PX: f32 = 320.0;
const TOAST_MARGIN_PX: f32 = 12.0;
const TOAST_GAP_PX: f32 = 8.0;
const DISMISS_WIDTH_PX: f32 = 28.0;
const ACTION_PADDING_PX: f32 = 16.0;
const TOAST_PADDING_PX: f64 = 8.0;

fn severity_bg(severity: ToastSeverity, theme: &Theme) -> Color {
    match severity {
        ToastSeverity::Info => theme.surface_bg,
        ToastSeverity::Success => Color::rgb(30, 80, 30),
        ToastSeverity::Warning => Color::rgb(100, 80, 20),
        ToastSeverity::Error => theme.error_fg,
    }
}

/// Compute the macOS pixel-unit layout for a [`ToastStack`].
pub fn mac_toast_stack_layout(
    stack: &ToastStack,
    font: &CTFont,
    viewport_width: f32,
    viewport_height: f32,
    line_height: f64,
) -> ToastStackLayout {
    stack.layout(
        viewport_width,
        viewport_height,
        TOAST_MARGIN_PX,
        TOAST_GAP_PX,
        |i| {
            let toast = &stack.toasts[i];
            let h = if toast.body.is_empty() {
                line_height as f32 + TOAST_PADDING_PX as f32 * 2.0
            } else {
                line_height as f32 * 2.0 + TOAST_PADDING_PX as f32 * 2.0
            };
            let action_w = toast
                .action
                .as_ref()
                .map(|a| {
                    let (tw, _) = measure_text(font, &a.label);
                    tw as f32 + ACTION_PADDING_PX
                })
                .unwrap_or(0.0);
            ToastMeasure {
                width: TOAST_WIDTH_PX.min(viewport_width - TOAST_MARGIN_PX * 2.0),
                height: h,
                dismiss_width: DISMISS_WIDTH_PX,
                action_width: action_w,
            }
        },
    )
}

/// Draw a [`ToastStack`] overlay onto `ctx`. Returns the layout for
/// host click dispatch.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_toast_stack(
    ctx: CGContextRef,
    font: &CTFont,
    viewport_width: f64,
    viewport_height: f64,
    stack: &ToastStack,
    theme: &Theme,
    line_height: f64,
) -> ToastStackLayout {
    let layout = mac_toast_stack_layout(
        stack,
        font,
        viewport_width as f32,
        viewport_height as f32,
        line_height,
    );

    for vt in &layout.visible_toasts {
        let toast = &stack.toasts[vt.toast_idx];
        paint_toast(ctx, font, vt, toast, theme);
    }

    layout
}

unsafe fn paint_toast(
    ctx: CGContextRef,
    font: &CTFont,
    vt: &VisibleToast,
    toast: &ToastItem,
    theme: &Theme,
) {
    let bg_color = toast
        .accent
        .unwrap_or_else(|| severity_bg(toast.severity, theme));

    fill_rect(
        ctx,
        vt.bounds.x as f64,
        vt.bounds.y as f64,
        vt.bounds.width as f64,
        vt.bounds.height as f64,
        bg_color,
    );

    // Title text.
    draw_text(
        ctx,
        font,
        &toast.title,
        vt.bounds.x as f64 + TOAST_PADDING_PX,
        vt.bounds.y as f64 + TOAST_PADDING_PX,
        color_to_cg(theme.foreground),
    );

    // Body text (second line).
    if !toast.body.is_empty() {
        let (_, title_h) = measure_text(font, &toast.title);
        draw_text(
            ctx,
            font,
            &toast.body,
            vt.bounds.x as f64 + TOAST_PADDING_PX,
            vt.bounds.y as f64 + TOAST_PADDING_PX + title_h,
            color_to_cg(theme.foreground),
        );
    }

    // Dismiss × — centred inside dismiss_bounds.
    if let Some(db) = vt.dismiss_bounds {
        let (tw, _) = measure_text(font, "×");
        draw_text(
            ctx,
            font,
            "×",
            db.x as f64 + (db.width as f64 - tw) / 2.0,
            vt.bounds.y as f64 + TOAST_PADDING_PX,
            color_to_cg(theme.foreground),
        );
    }

    // Action button label.
    if let (Some(ab), Some(ref action)) = (vt.action_bounds, &toast.action) {
        let (tw, _) = measure_text(font, &action.label);
        draw_text(
            ctx,
            font,
            &action.label,
            ab.x as f64 + (ab.width as f64 - tw) / 2.0,
            vt.bounds.y as f64 + TOAST_PADDING_PX,
            color_to_cg(theme.accent_fg),
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

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::toast::{ToastAction, ToastCorner, ToastHit};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 400;
    const H: u32 = 240;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn toast(id: &str, title: &str, severity: ToastSeverity) -> ToastItem {
        ToastItem {
            id: WidgetId::new(id),
            title: title.into(),
            body: String::new(),
            severity,
            action: None,
            accent: None,
        }
    }

    fn sample_stack() -> ToastStack {
        ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![
                toast("t1", "Saved", ToastSeverity::Success),
                toast("t2", "Error", ToastSeverity::Error),
            ],
        }
    }

    fn paint_via_backend(stack: &ToastStack) -> (BitmapSurface, ToastStackLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_toast_stack(QRect::new(0.0, 0.0, W as f32, H as f32), stack);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn bottom_right_corner_positions_toasts_at_bottom_right() {
        let stack = sample_stack();
        let (_surface, layout) = paint_via_backend(&stack);
        assert_eq!(layout.visible_toasts.len(), 2);
        // First-visible toast is newest (t2 = idx 1), nearest the
        // corner (bottom-right).
        let first = &layout.visible_toasts[0];
        // Right edge near W - margin.
        assert!(
            (first.bounds.x + first.bounds.width - (W as f32 - TOAST_MARGIN_PX)).abs() < 1.0,
            "toast right edge should be near viewport right: got x={}, width={}",
            first.bounds.x,
            first.bounds.width,
        );
        // Bottom edge near H - margin.
        assert!(
            (first.bounds.y + first.bounds.height - (H as f32 - TOAST_MARGIN_PX)).abs() < 1.0,
            "toast bottom edge should be near viewport bottom",
        );
    }

    #[test]
    fn severity_tint_painted() {
        let stack = ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![toast("err", "Boom", ToastSeverity::Error)],
        };
        let (surface, layout) = paint_via_backend(&stack);
        let theme = Theme::default();
        let t = &layout.visible_toasts[0];
        // Probe inside the box, away from glyphs (right of the title
        // and above the dismiss).
        let px = (t.bounds.x + t.bounds.width - DISMISS_WIDTH_PX - 4.0) as u32;
        let py = (t.bounds.y + t.bounds.height - 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        let expected = severity_bg(ToastSeverity::Error, &theme);
        assert_eq!((r, g, b), (expected.r, expected.g, expected.b));
    }

    #[test]
    fn hit_test_dismiss_vs_body() {
        let stack = sample_stack();
        let (_surface, layout) = paint_via_backend(&stack);
        let t = &layout.visible_toasts[0];
        let db = t.dismiss_bounds.expect("dismiss bounds present");
        let hit = layout.hit_test(db.x + db.width * 0.5, db.y + db.height * 0.5);
        assert!(matches!(hit, ToastHit::Dismiss(_)), "hit was {:?}", hit);

        // Body click: left side of toast, well before dismiss/action.
        let hit = layout.hit_test(t.bounds.x + 10.0, t.bounds.y + t.bounds.height * 0.5);
        assert!(matches!(hit, ToastHit::Body(_)));
    }

    #[test]
    fn action_button_reserves_action_bounds() {
        let stack = ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![ToastItem {
                action: Some(ToastAction {
                    id: WidgetId::new("undo"),
                    label: "Undo".into(),
                }),
                ..toast("t", "Did the thing", ToastSeverity::Info)
            }],
        };
        let (_surface, layout) = paint_via_backend(&stack);
        let t = &layout.visible_toasts[0];
        let ab = t.action_bounds.expect("action bounds present");
        // Hit-test the action returns Action.
        let hit = layout.hit_test(ab.x + ab.width * 0.5, ab.y + ab.height * 0.5);
        assert!(matches!(hit, ToastHit::Action(_)), "hit was {:?}", hit);
    }

    #[test]
    fn empty_stack_no_visible_toasts() {
        let stack = ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![],
        };
        let (_surface, layout) = paint_via_backend(&stack);
        assert!(layout.visible_toasts.is_empty());
    }
}
