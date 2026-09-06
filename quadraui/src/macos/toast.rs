//! macOS rasteriser for [`crate::ToastStack`].
//!
//! Painting moved to the shared
//! [`crate::primitives::toast::native_surface_paint::paint`] (#861,
//! `NativeSurface` Phase 2d slice 4/9) — see that fn's doc for the named
//! divergences (Win never took a live theme; Win's dismiss/action text
//! wasn't centred in its sub-region) found while unifying
//! `gtk::toast::draw_toast_stack`, `macos::toast::draw_toast_stack` and
//! `win::toast::draw_toast_stack` into one implementation. This module
//! now only carries [`mac_toast_stack_layout`] (pure layout, still needed
//! by `MacBackend::toast_stack_layout` for no-paint hit-test queries),
//! [`RawMacToastSurface`], and the deprecated [`draw_toast_stack`]
//! compatibility shim over it, mirroring `macos::form::RawFormSurface`
//! (#808).
//!
//! ## Scope omissions (follow-up)
//!
//! - **Rounded corners** — boxes are straight rectangles for now. CG
//!   path API for rounded rects deferred with other corner work
//!   (search-box border in command_center, close-button hover bg in
//!   tab_bar).

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::toast::{ToastMeasure, ToastStack, ToastStackLayout};
use crate::theme::Theme;
use crate::types::Color;

const TOAST_WIDTH_PX: f32 = 320.0;
const TOAST_MARGIN_PX: f32 = 12.0;
const TOAST_GAP_PX: f32 = 8.0;
const DISMISS_WIDTH_PX: f32 = 28.0;
const ACTION_PADDING_PX: f32 = 16.0;
const TOAST_PADDING_PX: f64 = 8.0;

/// Compute the macOS pixel-unit layout for a [`ToastStack`].
///
/// `(origin_x, origin_y)` is baked into the returned bounds (absolute
/// screen coordinates, matching `mac_menu_bar_layout` / `mac_panel_layout`)
/// — hosts call `layout.hit_test(x, y)` with raw click coordinates, no
/// localisation needed.
///
/// Still its own Core Text-based measurer, independent of the shared
/// paint's internal layout computation (which measures via
/// [`crate::native_surface::NativeSurface::surface_measure_text`]) — same
/// "no-paint layout stays put" posture as `MacBackend::status_bar_layout`
/// (#860).
#[allow(clippy::too_many_arguments)]
pub fn mac_toast_stack_layout(
    stack: &ToastStack,
    font: &CTFont,
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

/// Minimal [`crate::native_surface::NativeSurface`] adapter over a raw
/// `(CGContextRef, &CTFont)` pair, used only by the deprecated
/// [`draw_toast_stack`] shim below — mirrors `macos::form::RawFormSurface`'s
/// identical pattern (#808), scoped to the three verbs a toast's paint
/// actually uses (fill, plain text run, measure).
pub(crate) struct RawMacToastSurface<'a> {
    pub(crate) ctx: CGContextRef,
    pub(crate) font: &'a CTFont,
}

impl crate::native_surface::NativeSurface for RawMacToastSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawMacToastSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawMacToastSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawMacToastSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawMacToastSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawMacToastSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let (w, h) = measure_text(self.font, text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's
        // paint pass — see this struct's construction site.
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
    }

    fn surface_stroke_rect(&mut self, _rect: crate::Rect, _color: Color, _stroke_width: f32) {
        unreachable!("ToastStack::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: Color) {
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            draw_text(
                self.ctx,
                self.font,
                text,
                rect.x as f64,
                rect.y as f64,
                super::backend::ns_color_to_cg(color),
            );
        }
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: Color,
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
/// direct `quadraui::macos::draw_toast_stack` reference rather than going
/// through [`crate::Backend::draw_toast_stack`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_toast_stack` instead — this free function is a compatibility shim over the shared #861 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_toast_stack(
    ctx: CGContextRef,
    font: &CTFont,
    origin_x: f64,
    origin_y: f64,
    viewport_width: f64,
    viewport_height: f64,
    stack: &ToastStack,
    theme: &Theme,
    line_height: f64,
) -> ToastStackLayout {
    let mut surface = RawMacToastSurface { ctx, font };
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
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::toast::{ToastAction, ToastCorner, ToastHit, ToastItem, ToastSeverity};
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
        paint_via_backend_at(stack, 0.0, 0.0)
    }

    /// Like [`paint_via_backend`] but paints the toast overlay anchored
    /// at an arbitrary `(origin_x, origin_y)` instead of always `(0, 0)`.
    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends"): `mac_toast_stack_layout` bakes the origin straight
    /// into the returned bounds (absolute frame, matching
    /// `mac_menu_bar_layout` / `mac_panel_layout`), so `hit_test` must
    /// keep resolving against raw (unshifted) click coordinates at any
    /// origin, not just `(0, 0)`.
    fn paint_via_backend_at(
        stack: &ToastStack,
        origin_x: f32,
        origin_y: f32,
    ) -> (BitmapSurface, ToastStackLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_toast_stack(
                QRect::new(origin_x, origin_y, W as f32 - origin_x, H as f32 - origin_y),
                stack,
            );
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
        // Error severity's fallback tint is `theme.error_fg` — see
        // `primitives::toast::native_surface_paint::severity_bg`, the
        // one shared copy of this formula post-#861.
        let expected = theme.error_fg;
        assert_eq!((r, g, b), (expected.r, expected.g, expected.b));
    }

    /// Shared body for `hit_test_dismiss_vs_body` — see
    /// [`paint_via_backend_at`] for the quadraui#494 non-zero-origin
    /// rationale.
    fn hit_test_dismiss_vs_body_at(origin_x: f32, origin_y: f32) {
        let stack = sample_stack();
        let (_surface, layout) = paint_via_backend_at(&stack, origin_x, origin_y);
        let t = &layout.visible_toasts[0];
        let db = t.dismiss_bounds.expect("dismiss bounds present");
        let hit = layout.hit_test(db.x + db.width * 0.5, db.y + db.height * 0.5);
        assert!(matches!(hit, ToastHit::Dismiss(_)), "hit was {:?}", hit);

        // Body click: left side of toast, well before dismiss/action.
        let hit = layout.hit_test(t.bounds.x + 10.0, t.bounds.y + t.bounds.height * 0.5);
        assert!(matches!(hit, ToastHit::Body(_)));
    }

    #[test]
    fn hit_test_dismiss_vs_body() {
        hit_test_dismiss_vs_body_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (quadraui#494): same round trip,
    /// painted at a shifted overlay origin.
    #[test]
    fn hit_test_dismiss_vs_body_at_nonzero_origin() {
        hit_test_dismiss_vs_body_at(7.0, 13.0);
    }

    /// Shared body for `action_button_reserves_action_bounds` — see
    /// [`paint_via_backend_at`] for the quadraui#494 non-zero-origin
    /// rationale.
    fn action_button_reserves_action_bounds_at(origin_x: f32, origin_y: f32) {
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
        let (_surface, layout) = paint_via_backend_at(&stack, origin_x, origin_y);
        let t = &layout.visible_toasts[0];
        let ab = t.action_bounds.expect("action bounds present");
        // Hit-test the action returns Action.
        let hit = layout.hit_test(ab.x + ab.width * 0.5, ab.y + ab.height * 0.5);
        assert!(matches!(hit, ToastHit::Action(_)), "hit was {:?}", hit);
    }

    #[test]
    fn action_button_reserves_action_bounds() {
        action_button_reserves_action_bounds_at(0.0, 0.0);
    }

    #[test]
    fn action_button_reserves_action_bounds_at_nonzero_origin() {
        action_button_reserves_action_bounds_at(7.0, 13.0);
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
