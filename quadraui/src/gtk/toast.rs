//! GTK rasteriser for [`crate::ToastStack`].
//!
//! Painting moved to the shared
//! [`crate::primitives::toast::native_surface_paint::paint`] (#861,
//! `NativeSurface` Phase 2d slice 4/9) — see that fn's doc for the named
//! divergences (Win never took a live theme; Win's dismiss/action text
//! wasn't centred in its sub-region) found while unifying
//! `gtk::draw_toast_stack`, `macos::toast::draw_toast_stack` and
//! `win::toast::draw_toast_stack` into one implementation. This module
//! now only carries [`gtk_toast_stack_layout`] (pure layout, still needed
//! by `GtkDriver`/downstream callers for no-paint hit-test queries),
//! [`RawGtkToastSurface`], and the deprecated [`draw_toast_stack`]
//! compatibility shim over it, mirroring `gtk::status_bar`'s identical
//! #860 shape.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::native_surface::NativeSurface;
use crate::primitives::toast::{ToastMeasure, ToastStack, ToastStackLayout};
use crate::theme::Theme;

const GTK_TOAST_WIDTH_PX: f32 = 320.0;
const GTK_TOAST_MARGIN_PX: f32 = 12.0;
const GTK_TOAST_GAP_PX: f32 = 8.0;
const GTK_DISMISS_WIDTH_PX: f32 = 28.0;
const GTK_ACTION_PADDING_PX: f32 = 16.0;
const GTK_TOAST_PADDING_PX: f64 = 8.0;

/// Compute the GTK pixel-unit layout for a [`ToastStack`] without painting.
///
/// `(origin_x, origin_y)` is baked into the returned bounds (absolute
/// window coordinates, matching `gtk_menu_bar_layout` / `gtk_panel_layout`)
/// — hosts call `layout.hit_test(x, y)` with raw click coordinates, no
/// localisation needed.
///
/// Still its own pango-based measurer, independent of the shared paint's
/// internal layout computation (which measures via
/// [`NativeSurface::surface_measure_text`]) — same "no-paint layout stays
/// put" posture as `GtkBackend::status_bar_layout` (#860).
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

/// Minimal [`NativeSurface`] adapter over a bare Cairo context + Pango
/// layout, used only by the deprecated [`draw_toast_stack`] shim below —
/// mirrors `gtk::status_bar::RawGtkStatusBarSurface`'s identical pattern
/// (#860), scoped to the three verbs a toast's paint actually uses
/// (fill, plain text run, measure).
struct RawGtkToastSurface<'a> {
    cr: &'a Context,
    pango_layout: &'a pango::Layout,
}

impl NativeSurface for RawGtkToastSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawGtkToastSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawGtkToastSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawGtkToastSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawGtkToastSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawGtkToastSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.pango_layout.set_text(text);
        self.pango_layout.set_attributes(None);
        let (w, h) = self.pango_layout.pixel_size();
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        super::set_source_rgba(self.cr, color);
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.fill().ok();
    }

    fn surface_stroke_rect(
        &mut self,
        _rect: crate::Rect,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("ToastStack::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        self.pango_layout.set_text(text);
        self.pango_layout.set_attributes(None);
        super::set_source(self.cr, color);
        self.cr.move_to(rect.x as f64, rect.y as f64);
        super::painted_text::show_layout(self.cr, self.pango_layout);
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("ToastStack::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, _rect: crate::Rect) {
        unreachable!("ToastStack::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("ToastStack::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("ToastStack::paint never draws an image")
    }
}

/// Deprecated free-function shim (#861, CLAUDE.md rule 8): reproduces
/// the pre-#861 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_toast_stack` reference rather than going
/// through [`crate::Backend::draw_toast_stack`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_toast_stack` instead — this free function is a compatibility shim over the shared #861 implementation"
)]
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
    let mut surface = RawGtkToastSurface { cr, pango_layout };
    crate::primitives::toast::native_surface_paint::paint(
        stack,
        &mut surface,
        theme,
        origin_x as f32,
        origin_y as f32,
        viewport_width as f32,
        viewport_height as f32,
        line_height as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toast::{ToastCorner, ToastHit, ToastItem, ToastSeverity, ToastStack};
    use crate::types::{Color, WidgetId};
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
    /// toast through [`RawGtkToastSurface`] and the shared
    /// `primitives::toast::native_surface_paint::paint` directly (rather
    /// than the deprecated [`draw_toast_stack`] shim, so this test
    /// doesn't trip the `-D warnings`-denied `deprecated` lint — mirrors
    /// `gtk::status_bar`'s identical #860 test-migration note), confirms
    /// the box's fill colour lands at the origin-shifted *absolute*
    /// position — not the viewport-local one `gtk_toast_stack_layout`
    /// used to compute internally before shifting — and that `hit_test`
    /// resolves clicks at that same absolute position through Dismiss
    /// and Body.
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
            let mut raw = RawGtkToastSurface {
                cr: &cr,
                pango_layout: &pango_layout,
            };
            crate::primitives::toast::native_surface_paint::paint(
                &stack,
                &mut raw,
                &Theme::default(),
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
