//! GTK rasteriser for [`crate::ToastStack`].
//!
//! Paints toast notification boxes stacked in a viewport corner.
//! Each toast is a rounded box with title, optional body, severity
//! tint, dismiss `×`, and optional action button label.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::set_source;
use crate::primitives::toast::{
    ToastMeasure, ToastSeverity, ToastStack, ToastStackLayout, VisibleToast,
};
use crate::theme::Theme;
use crate::types::Color;

const GTK_TOAST_WIDTH_PX: f32 = 320.0;
const GTK_TOAST_MARGIN_PX: f32 = 12.0;
const GTK_TOAST_GAP_PX: f32 = 8.0;
const GTK_DISMISS_WIDTH_PX: f32 = 28.0;
const GTK_ACTION_PADDING_PX: f32 = 16.0;
const GTK_TOAST_PADDING_PX: f64 = 8.0;

fn severity_bg(severity: ToastSeverity, theme: &Theme) -> Color {
    match severity {
        ToastSeverity::Info => theme.surface_bg,
        ToastSeverity::Success => Color::rgb(30, 80, 30),
        ToastSeverity::Warning => Color::rgb(100, 80, 20),
        ToastSeverity::Error => theme.error_fg,
    }
}

/// Compute the GTK pixel-unit layout for a [`ToastStack`] without painting.
pub fn gtk_toast_stack_layout(
    stack: &ToastStack,
    pango_layout: &pango::Layout,
    viewport_width: f32,
    viewport_height: f32,
    line_height: f64,
) -> ToastStackLayout {
    stack.layout(
        viewport_width,
        viewport_height,
        GTK_TOAST_MARGIN_PX,
        GTK_TOAST_GAP_PX,
        |i| {
            let toast = &stack.toasts[i];
            let h = if toast.body.is_empty() {
                line_height as f32 + GTK_TOAST_PADDING_PX as f32 * 2.0
            } else {
                line_height as f32 * 2.0 + GTK_TOAST_PADDING_PX as f32 * 2.0
            };
            let action_w = toast
                .action
                .as_ref()
                .map(|a| {
                    pango_layout.set_text(&a.label);
                    pango_layout.set_attributes(None);
                    pango_layout.pixel_size().0 as f32 + GTK_ACTION_PADDING_PX
                })
                .unwrap_or(0.0);
            ToastMeasure {
                width: GTK_TOAST_WIDTH_PX.min(viewport_width - GTK_TOAST_MARGIN_PX * 2.0),
                height: h,
                dismiss_width: GTK_DISMISS_WIDTH_PX,
                action_width: action_w,
            }
        },
    )
}

/// Draw a [`ToastStack`] overlay onto `cr`. Returns the layout for
/// host click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_toast_stack(
    cr: &Context,
    pango_layout: &pango::Layout,
    viewport_width: f64,
    viewport_height: f64,
    stack: &ToastStack,
    theme: &Theme,
    line_height: f64,
) -> ToastStackLayout {
    let layout = gtk_toast_stack_layout(
        stack,
        pango_layout,
        viewport_width as f32,
        viewport_height as f32,
        line_height,
    );

    for vt in &layout.visible_toasts {
        let toast = &stack.toasts[vt.toast_idx];
        paint_toast(cr, pango_layout, vt, toast, theme, line_height);
    }

    layout
}

fn paint_toast(
    cr: &Context,
    pango_layout: &pango::Layout,
    vt: &VisibleToast,
    toast: &crate::primitives::toast::ToastItem,
    theme: &Theme,
    _line_height: f64,
) {
    let bg_color = toast
        .accent
        .unwrap_or_else(|| severity_bg(toast.severity, theme));

    // Background rect.
    set_source(cr, bg_color);
    cr.rectangle(
        vt.bounds.x as f64,
        vt.bounds.y as f64,
        vt.bounds.width as f64,
        vt.bounds.height as f64,
    );
    cr.fill().ok();

    // Title text.
    pango_layout.set_text(&toast.title);
    pango_layout.set_attributes(None);
    set_source(cr, theme.foreground);
    cr.move_to(
        vt.bounds.x as f64 + GTK_TOAST_PADDING_PX,
        vt.bounds.y as f64 + GTK_TOAST_PADDING_PX,
    );
    pcfn::show_layout(cr, pango_layout);

    // Body text (second line).
    if !toast.body.is_empty() {
        let title_h = pango_layout.pixel_size().1 as f64;
        pango_layout.set_text(&toast.body);
        set_source(cr, theme.foreground);
        cr.move_to(
            vt.bounds.x as f64 + GTK_TOAST_PADDING_PX,
            vt.bounds.y as f64 + GTK_TOAST_PADDING_PX + title_h,
        );
        pcfn::show_layout(cr, pango_layout);
    }

    // Dismiss ×.
    if let Some(db) = vt.dismiss_bounds {
        pango_layout.set_text("×");
        set_source(cr, theme.foreground);
        let text_w = pango_layout.pixel_size().0 as f64;
        cr.move_to(
            db.x as f64 + (db.width as f64 - text_w) / 2.0,
            vt.bounds.y as f64 + GTK_TOAST_PADDING_PX,
        );
        pcfn::show_layout(cr, pango_layout);
    }

    // Action button label.
    if let Some(ab) = vt.action_bounds {
        if let Some(ref action) = toast.action {
            pango_layout.set_text(&action.label);
            set_source(cr, theme.accent_fg);
            let text_w = pango_layout.pixel_size().0 as f64;
            cr.move_to(
                ab.x as f64 + (ab.width as f64 - text_w) / 2.0,
                vt.bounds.y as f64 + GTK_TOAST_PADDING_PX,
            );
            pcfn::show_layout(cr, pango_layout);
        }
    }
}
