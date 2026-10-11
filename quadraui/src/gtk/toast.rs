//! GTK rasteriser for [`crate::ToastOverlay`].
//!
//! Painting moved to the shared [`crate::primitives::toast::native_surface_paint::paint`].
//! This module now only carries [`gtk_toast_stack_layout`] (pure layout, still needed
//! by `GtkDriver`/downstream callers for no-paint hit-test queries).

use gtk4::pango;

use crate::primitives::layout_metrics::pixel_toast_stack_layout;
use crate::primitives::toast::{ToastOverlay, ToastStackLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute the GTK pixel-unit layout for a [`ToastOverlay`] without painting.
///
/// `(origin_x, origin_y)` is baked into the returned bounds (absolute
/// window coordinates, matching `gtk_menu_bar_layout` / `gtk_panel_layout`)
/// — hosts call `layout.hit_test(x, y)` with raw click coordinates, no
/// localisation needed.
///
/// Uses pango-based text measurement. Shares its geometry with
/// `mac_toast_stack_layout` / `win_toast_stack_layout` via
/// [`pixel_toast_stack_layout`]; `pango_layout`'s `set_attributes(None)`
/// reset happens once up front for efficiency.
#[allow(clippy::too_many_arguments)]
pub fn gtk_toast_stack_layout(
    stack: &ToastOverlay,
    pango_layout: &pango::Layout,
    origin_x: f32,
    origin_y: f32,
    viewport_width: f32,
    viewport_height: f32,
    line_height: f64,
) -> ToastStackLayout {
    pango_layout.set_attributes(None);
    let measure = super::toolbar::PangoMeasure {
        pango_layout: Some(pango_layout),
        char_width: 0.0,
    };
    pixel_toast_stack_layout(
        stack,
        &measure,
        origin_x,
        origin_y,
        viewport_width,
        viewport_height,
        line_height as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toast::{Toast, ToastCorner, ToastHit, ToastOverlay, ToastSeverity};
    use crate::types::{Color, WidgetId};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    // Fixed overlay size, independent of the test's origin.
    const VIEW_W: f64 = 340.0;
    const VIEW_H: f64 = 200.0;
    const LINE_HEIGHT: f64 = 16.0;
    const BOX_COLOR: Color = Color::rgb(10, 20, 30);

    /// A toast with a distinct `accent` fill (overrides the severity
    /// tint) so its painted box is trivially distinguishable from the
    /// white canvas background by colour, without scanning for glyphs.
    fn colored_toast(id: &str, title: &str) -> Toast {
        Toast {
            id: WidgetId::new(id),
            title: title.into(),
            body: String::new(),
            severity: ToastSeverity::Info,
            actions: Vec::new(),
            accent: Some(BOX_COLOR),
        }
    }

    fn stack_br(toasts: Vec<Toast>) -> ToastOverlay {
        ToastOverlay {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts,
            focus: None,
        }
    }

    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    /// Paint→click round trip at `(origin_x, origin_y)`: paints a single
    /// toast through [`super::super::surface::CairoSurface`] and the shared
    /// `primitives::toast::native_surface_paint::paint`, confirms the box's
    /// fill colour lands at the origin-shifted *absolute* position, and that
    /// `hit_test` resolves clicks at that same absolute position through
    /// Dismiss and Body.
    ///
    /// Canvas grows with the origin (`VIEW_W`/`VIEW_H` stay fixed) so an
    /// origin regression would show as a shifted absolute paint position.
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
            let mut raw = crate::gtk::surface::CairoSurface {
                cr: &cr,
                layout: Some(&pango_layout),
                translucent_fill: true,
            };
            crate::primitives::toast::native_surface_paint::paint(
                &stack,
                &mut raw,
                &Theme::default(),
                &crate::style::Style::default(),
                origin_x as f32,
                origin_y as f32,
                VIEW_W as f32,
                VIEW_H as f32,
                LINE_HEIGHT as f32,
            )
        };
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        assert_eq!(layout.visible_toasts.len(), 1);
        let vt = &layout.visible_toasts[0];

        // Probe near the box's own left edge, at mid-height — inset,
        // away from glyphs/dismiss *and* away from the rounded corners
        // (a near-corner probe would land outside the fillet, seeing the
        // backdrop instead of the box) — must be the toast's fill colour
        // at the *absolute* bounds `draw_toast_stack` painted into.
        let probe_x = (vt.bounds.x + 2.0) as i32;
        let probe_y = (vt.bounds.y + vt.bounds.height / 2.0) as i32;
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

    /// Regression guard: `gtk_toast_stack_layout` must return coordinates in
    /// the same frame (absolute window coordinates) regardless of the overlay's
    /// starting position.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        paint_and_click_round_trip_at(7.0, 13.0);
    }
}
