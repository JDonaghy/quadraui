//! macOS rasteriser for [`crate::ToastOverlay`].
//!
//! Painting moved to the shared
//! [`crate::primitives::toast::native_surface_paint::paint`] (#861,
//! `PaintSurface` Phase 2d slice 4/9) — see that fn's doc for the named
//! divergences (Win never took a live theme; Win's dismiss/action text
//! wasn't centred in its sub-region) found while unifying
//! `gtk::toast::draw_toast_stack`, `macos::toast::draw_toast_stack` and
//! `win::toast::draw_toast_stack` into one implementation. This module
//! now only carries [`mac_toast_stack_layout`] (pure layout, still needed
//! by `MacBackend::toast_stack_layout` for no-paint hit-test queries);
//! the deprecated `draw_toast_stack` compatibility shim over the shared
//! [`super::surface::CgSurface`] adapter was removed in issue #1109
//! (zero uses in coord-tui's `main` and vimcode's `develop`).
//!
//! ## Scope omissions (follow-up)
//!
//! - **Rounded corners** — boxes are straight rectangles for now. CG
//!   path API for rounded rects deferred with other corner work
//!   (search-box border in command_center, close-button hover bg in
//!   tab_bar).

use core_text::font::CTFont;

use crate::primitives::layout_metrics::pixel_toast_stack_layout;
use crate::primitives::toast::{ToastOverlay, ToastStackLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute the macOS pixel-unit layout for a [`ToastOverlay`].
///
/// `(origin_x, origin_y)` is baked into the returned bounds (absolute
/// screen coordinates, matching `mac_menu_bar_layout` / `mac_panel_layout`)
/// — hosts call `layout.hit_test(x, y)` with raw click coordinates, no
/// localisation needed.
///
/// Still its own Core Text-based measurer, independent of the shared
/// paint's internal layout computation (which measures via
/// [`crate::paint_surface::PaintSurface::surface_measure_text`]) — same
/// "no-paint layout stays put" posture as `MacBackend::status_bar_layout`
/// (#860). Shares its geometry with `gtk_toast_stack_layout` /
/// `win_toast_stack_layout` via [`pixel_toast_stack_layout`] (issue
/// #1079) — `font: &CTFont` is passed straight through as `&dyn
/// TextMeasure` (`CTFont` implements it directly, see `macos::text`'s
/// module doc).
#[allow(clippy::too_many_arguments)]
pub fn mac_toast_stack_layout(
    stack: &ToastOverlay,
    font: &CTFont,
    origin_x: f32,
    origin_y: f32,
    viewport_width: f32,
    viewport_height: f32,
    line_height: f64,
) -> ToastStackLayout {
    pixel_toast_stack_layout(
        stack,
        font,
        origin_x,
        origin_y,
        viewport_width,
        viewport_height,
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
    use crate::primitives::layout_metrics::pixel;
    use crate::primitives::toast::{Toast, ToastButton, ToastCorner, ToastHit, ToastSeverity};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 400;
    const H: u32 = 240;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn toast(id: &str, title: &str, severity: ToastSeverity) -> Toast {
        Toast {
            id: WidgetId::new(id),
            title: title.into(),
            body: String::new(),
            severity,
            actions: Vec::new(),
            accent: None,
        }
    }

    fn sample_stack() -> ToastOverlay {
        ToastOverlay {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![
                toast("t1", "Saved", ToastSeverity::Success),
                toast("t2", "Error", ToastSeverity::Error),
            ],
            focus: None,
        }
    }

    fn paint_via_backend(stack: &ToastOverlay) -> (BitmapSurface, ToastStackLayout) {
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
        stack: &ToastOverlay,
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
            let l = b.draw_toast_overlay(
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
            (first.bounds.x + first.bounds.width - (W as f32 - pixel::TOAST_MARGIN)).abs() < 1.0,
            "toast right edge should be near viewport right: got x={}, width={}",
            first.bounds.x,
            first.bounds.width,
        );
        // Bottom edge near H - margin.
        assert!(
            (first.bounds.y + first.bounds.height - (H as f32 - pixel::TOAST_MARGIN)).abs() < 1.0,
            "toast bottom edge should be near viewport bottom",
        );
    }

    #[test]
    fn severity_tint_painted() {
        let stack = ToastOverlay {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![toast("err", "Boom", ToastSeverity::Error)],
            focus: None,
        };
        let (surface, layout) = paint_via_backend(&stack);
        let theme = Theme::default();
        let t = &layout.visible_toasts[0];
        // Probe inside the box, away from glyphs (right of the title
        // and above the dismiss).
        let px = (t.bounds.x + t.bounds.width - pixel::TOAST_DISMISS_WIDTH - 4.0) as u32;
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
        let stack = ToastOverlay {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![Toast {
                actions: vec![ToastButton {
                    id: WidgetId::new("undo"),
                    label: "Undo".into(),
                    primary: false,
                }],
                ..toast("t", "Did the thing", ToastSeverity::Info)
            }],
            focus: None,
        };
        let (_surface, layout) = paint_via_backend_at(&stack, origin_x, origin_y);
        let t = &layout.visible_toasts[0];
        let ab = *t.action_rects.first().expect("action bounds present");
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
        let stack = ToastOverlay {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![],
            focus: None,
        };
        let (_surface, layout) = paint_via_backend(&stack);
        assert!(layout.visible_toasts.is_empty());
    }
}
