//! Direct2D / DirectWrite rasteriser for [`crate::ToastOverlay`] (issue #29).
//!
//! Painting moved to the shared
//! [`crate::primitives::toast::native_surface_paint::paint`] (#861,
//! `NativeSurface` Phase 2d slice 4/9) — see that fn's doc for the named
//! divergences found while unifying `gtk::toast::draw_toast_stack`,
//! `macos::toast::draw_toast_stack` and `win::toast::draw_toast_stack`
//! into one implementation:
//!
//! - This module's `draw_toast_stack` never took a `theme: &Theme`
//!   parameter — every call painted with `Theme::default()`, unlike its
//!   `gtk`/`macos` counterparts. The shared `paint` requires a live theme
//!   (matching those two), so [`WinBackend::draw_toast_stack`] now passes
//!   `&self.current_theme` — fixing the quadraui#789-class gap this
//!   module was missed by.
//! - This module's dismiss `×`/action label were drawn flush against
//!   their sub-region's left edge (`DWrite`'s default alignment); the
//!   shared `paint` centres them horizontally, matching what `gtk`/`macos`
//!   already did.
//!
//! This module now only carries [`win_toast_stack_layout`] (pure layout,
//! still needed by `WinBackend::toast_stack_layout` for no-paint
//! hit-test queries); the deprecated `draw_toast_stack` compatibility
//! shim over the shared [`super::surface::D2dSurface`] adapter was
//! removed in issue #1109 (zero uses in coord-tui's `main` and
//! vimcode's `develop`).
//!
//! [`win_toast_stack_layout`] is pure geometry generic over
//! [`crate::primitives::layout_metrics::TextMeasure`] — no Direct2D/
//! DirectWrite type in its signature — so it compiles and runs
//! everywhere, including a plain `cargo test --features win` on Linux.
//! `super::mod`'s `mod toast;` is no longer whole-module gated; see
//! `backend.rs`'s module docs.

#[cfg(all(test, target_os = "windows"))]
use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::layout_metrics::{pixel_toast_stack_layout, TextMeasure};
use crate::primitives::toast::{ToastOverlay, ToastStackLayout};
#[cfg(all(test, target_os = "windows"))]
use crate::theme::Theme;

/// Compute a [`ToastOverlay`]'s layout without painting — the measurer twin
/// of the shared paint's internal layout computation. Both use the
/// identical per-toast measurer shape, so a no-paint hit-test call always
/// agrees with what the last paint drew. Pure geometry over [`TextMeasure`]
/// (issue #1078) — `measure` may be a live `&DWrite` (when painting) or
/// [`super::backend`]'s nominal measurer (no surface yet); independent of
/// [`crate::native_surface::NativeSurface::surface_measure_text`] — same
/// "no-paint layout stays put" posture as `WinBackend::status_bar_layout`
/// (#860). Shares its geometry with `gtk_toast_stack_layout` /
/// `mac_toast_stack_layout` via [`pixel_toast_stack_layout`] (issue
/// #1079).
pub fn win_toast_stack_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    stack: &ToastOverlay,
    line_height: f32,
) -> ToastStackLayout {
    pixel_toast_stack_layout(
        stack,
        measure,
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        line_height,
    )
}

// #1078: every test below paints through a real `DWrite`/`HeadlessSurface`
// — gated the same way the whole module used to be.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::primitives::toast::{Toast, ToastButton, ToastCorner, ToastHit, ToastSeverity};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 400;
    const H: u32 = 300;
    const LINE_HEIGHT: f32 = 16.0;

    /// A toast with a distinct `accent` fill (overrides the severity
    /// tint) so its painted box is trivially distinguishable from the
    /// cleared canvas by colour, without scanning for glyphs — same
    /// technique `gtk::toast::tests::colored_toast` uses.
    const BOX_COLOR: Color = Color::rgb(10, 20, 30);

    fn colored_toast(id: &str, title: &str) -> Toast {
        Toast {
            id: WidgetId::new(id),
            title: title.into(),
            body: "Details here".into(),
            severity: ToastSeverity::Info,
            actions: vec![ToastButton {
                id: WidgetId::new(format!("{id}:act")),
                label: "Undo".into(),
                primary: false,
            }],
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

    /// Paint↔click round trip: the toast box's painted fill colour lands
    /// at its own bounds, and `hit_test` resolves clicks on dismiss,
    /// action, and body to the matching `ToastHit`. Exercises the shared
    /// paint through [`super::super::surface::D2dSurface`] directly —
    /// the same adapter the now-removed `draw_toast_stack` shim used
    /// (issue #1109).
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let stack = stack_br(vec![colored_toast("t1", "Saved")]);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let layout = surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(&dwrite),
                };
                crate::primitives::toast::native_surface_paint::paint(
                    &stack,
                    &mut raw,
                    &theme,
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    LINE_HEIGHT,
                );
            })
            .map(|_| win_toast_stack_layout(&dwrite, rect, &stack, LINE_HEIGHT))
            .expect("paint toast stack");

        assert_eq!(layout.visible_toasts.len(), 1);
        let vt = &layout.visible_toasts[0];

        // Probe near the box's own bottom-left corner — inset, away
        // from glyphs/dismiss/action.
        let probe = surface.pixel_at(
            (vt.bounds.x + 2.0) as u32,
            (vt.bounds.y + vt.bounds.height - 2.0) as u32,
        );
        assert_eq!(
            (probe.r, probe.g, probe.b),
            (BOX_COLOR.r, BOX_COLOR.g, BOX_COLOR.b)
        );

        let db = vt.dismiss_bounds.expect("dismiss bounds present");
        let dismiss_hit = layout.hit_test(db.x + db.width / 2.0, db.y + db.height / 2.0);
        assert_eq!(dismiss_hit, ToastHit::Dismiss(WidgetId::new("t1")));

        let ab = *vt.action_rects.first().expect("action bounds present");
        let action_hit = layout.hit_test(ab.x + ab.width / 2.0, ab.y + ab.height / 2.0);
        assert_eq!(action_hit, ToastHit::Action(WidgetId::new("t1:act")));

        let body_hit = layout.hit_test(vt.bounds.x + 2.0, vt.bounds.y + vt.bounds.height - 2.0);
        assert_eq!(body_hit, ToastHit::Body(WidgetId::new("t1")));
    }

    /// `win_toast_stack_layout` (no-paint) must produce byte-identical
    /// layout to what the shared paint used to lay out — same stack,
    /// same rect, same line height.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let stack = stack_br(vec![colored_toast("t1", "Saved")]);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let painted = surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(&dwrite),
                };
                crate::primitives::toast::native_surface_paint::paint(
                    &stack,
                    &mut raw,
                    &theme,
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    LINE_HEIGHT,
                );
            })
            .map(|_| win_toast_stack_layout(&dwrite, rect, &stack, LINE_HEIGHT))
            .expect("paint");
        let no_paint = win_toast_stack_layout(&dwrite, rect, &stack, LINE_HEIGHT);

        assert_eq!(painted, no_paint);
    }

    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends"): `rect`'s own `(x, y)` must be honoured as the
    /// overlay's absolute origin, not silently treated as `(0, 0)`.
    #[test]
    fn paint_and_hit_test_round_trip_at_nonzero_origin() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let stack = stack_br(vec![colored_toast("t1", "Saved")]);
        let rect = Rect::new(20.0, 30.0, W as f32, H as f32);
        let theme = Theme::default();

        let surface = HeadlessSurface::new(W + 20, H + 30).expect("create surface");
        let layout = surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(&dwrite),
                };
                crate::primitives::toast::native_surface_paint::paint(
                    &stack,
                    &mut raw,
                    &theme,
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    LINE_HEIGHT,
                );
            })
            .map(|_| win_toast_stack_layout(&dwrite, rect, &stack, LINE_HEIGHT))
            .expect("paint toast stack");

        let vt = &layout.visible_toasts[0];
        assert!(
            vt.bounds.x >= rect.x && vt.bounds.y >= rect.y,
            "toast bounds should be shifted into the overlay's absolute frame"
        );

        let probe = surface.pixel_at(
            (vt.bounds.x + 2.0) as u32,
            (vt.bounds.y + vt.bounds.height - 2.0) as u32,
        );
        assert_eq!(
            (probe.r, probe.g, probe.b),
            (BOX_COLOR.r, BOX_COLOR.g, BOX_COLOR.b)
        );
    }
}
