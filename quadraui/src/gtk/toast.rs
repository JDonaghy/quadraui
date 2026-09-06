//! GTK rasteriser for [`crate::ToastStack`].
//!
//! Paints toast notification boxes stacked in a viewport corner.
//! Each toast is a rounded box with title, optional body, severity
//! tint, dismiss `×`, and optional action button label.

use gtk4::cairo::Context;
use gtk4::pango;

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
///
/// `(origin_x, origin_y)` is baked into the returned bounds (absolute
/// window coordinates, matching `gtk_menu_bar_layout` / `gtk_panel_layout`)
/// — hosts call `layout.hit_test(x, y)` with raw click coordinates, no
/// localisation needed.
#[allow(clippy::too_many_arguments)]
pub fn gtk_toast_stack_layout(
    stack: &ToastStack,
    pango_layout: &pango::Layout,
    origin_x: f32,
    origin_y: f32,
    viewport_width: f32,
    viewport_height: f32,
    line_height: f64,
) -> ToastStackLayout {
    stack.layout(
        origin_x,
        origin_y,
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
    origin_x: f64,
    origin_y: f64,
    viewport_width: f64,
    viewport_height: f64,
    stack: &ToastStack,
    theme: &Theme,
    line_height: f64,
) -> ToastStackLayout {
    let layout = gtk_toast_stack_layout(
        stack,
        pango_layout,
        origin_x as f32,
        origin_y as f32,
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
    super::painted_text::show_layout(cr, pango_layout);

    // Body text (second line).
    if !toast.body.is_empty() {
        let title_h = pango_layout.pixel_size().1 as f64;
        pango_layout.set_text(&toast.body);
        set_source(cr, theme.foreground);
        cr.move_to(
            vt.bounds.x as f64 + GTK_TOAST_PADDING_PX,
            vt.bounds.y as f64 + GTK_TOAST_PADDING_PX + title_h,
        );
        super::painted_text::show_layout(cr, pango_layout);
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
        super::painted_text::show_layout(cr, pango_layout);
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
            super::painted_text::show_layout(cr, pango_layout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toast::{ToastCorner, ToastHit, ToastItem, ToastSeverity, ToastStack};
    use crate::types::WidgetId;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    // Fixed overlay size, independent of the test's origin — this file
    // had no test module at all before quadraui#494 (the reviewed gap:
    // `gtk_toast_stack_layout`/`draw_toast_stack` never received the
    // overlay's origin, so a non-zero-origin toast stack — e.g.
    // coord-tui's `main_content_bounds`, which sits right of a sidebar —
    // painted at the wrong screen position and hit-tested against the
    // wrong coordinates).
    const VIEW_W: f64 = 340.0;
    const VIEW_H: f64 = 200.0;
    const LINE_HEIGHT: f64 = 16.0;
    const BOX_COLOR: Color = Color::rgb(10, 20, 30);

    /// A toast with a distinct `accent` fill (overrides the severity
    /// tint) so its painted box is trivially distinguishable from the
    /// white canvas background by colour, without scanning for glyphs.
    fn colored_toast(id: &str, title: &str) -> ToastItem {
        ToastItem {
            id: WidgetId::new(id),
            title: title.into(),
            body: String::new(),
            severity: ToastSeverity::Info,
            action: None,
            accent: Some(BOX_COLOR),
        }
    }

    fn stack_br(toasts: Vec<ToastItem>) -> ToastStack {
        ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts,
        }
    }

    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    /// Paint→click round trip at `(origin_x, origin_y)`: paints a single
    /// toast through [`draw_toast_stack`], confirms the box's fill
    /// colour lands at the origin-shifted *absolute* position — not the
    /// viewport-local one `gtk_toast_stack_layout` used to compute
    /// internally before shifting — and that `hit_test` resolves clicks
    /// at that same absolute position through Dismiss and Body.
    ///
    /// Canvas grows with the origin (`VIEW_W`/`VIEW_H` stay fixed) so a
    /// dropped-origin regression shows up as a shifted absolute paint
    /// position rather than being masked by a shrinking viewport.
    fn paint_and_click_round_trip_at(origin_x: f64, origin_y: f64) {
        let canvas_w = (origin_x + VIEW_W).ceil() as i32;
        let canvas_h = (origin_y + VIEW_H).ceil() as i32;
        let mut surface =
            ImageSurface::create(Format::ARgb32, canvas_w, canvas_h).expect("create ImageSurface");
        let stack = stack_br(vec![colored_toast("t1", "Hello")]);

        let layout = {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_toast_stack(
                &cr,
                &pango_layout,
                origin_x,
                origin_y,
                VIEW_W,
                VIEW_H,
                &stack,
                &Theme::default(),
                LINE_HEIGHT,
            )
        };
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        assert_eq!(layout.visible_toasts.len(), 1);
        let vt = &layout.visible_toasts[0];

        // Probe near the box's own bottom-left corner — inset, away
        // from glyphs/dismiss — must be the toast's fill colour at the
        // *absolute* bounds `draw_toast_stack` painted into.
        let probe_x = (vt.bounds.x + 2.0) as i32;
        let probe_y = (vt.bounds.y + vt.bounds.height - 2.0) as i32;
        assert_eq!(
            pixel(&data, stride, probe_x, probe_y),
            (BOX_COLOR.r, BOX_COLOR.g, BOX_COLOR.b),
            "toast box should be painted at its own absolute bounds \
             (origin=({origin_x}, {origin_y}), bounds={:?})",
            vt.bounds,
        );

        // Round trip: absolute clicks at the dismiss and body positions
        // resolve through hit_test.
        let db = vt.dismiss_bounds.expect("dismiss bounds present");
        let hit = layout.hit_test(db.x + db.width * 0.5, db.y + db.height * 0.5);
        assert_eq!(hit, ToastHit::Dismiss(WidgetId::new("t1")));

        let body_hit = layout.hit_test(vt.bounds.x + 5.0, vt.bounds.y + vt.bounds.height * 0.5);
        assert_eq!(body_hit, ToastHit::Body(WidgetId::new("t1")));
    }

    #[test]
    fn paint_and_click_round_trip() {
        paint_and_click_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends"): before this fix `gtk_toast_stack_layout` had no
    /// origin parameter at all, so `draw_toast_stack` could only ever
    /// paint correctly when the overlay's own rect started at `(0, 0)`.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        paint_and_click_round_trip_at(7.0, 13.0);
    }
}
