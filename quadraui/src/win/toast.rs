//! Direct2D / DirectWrite rasteriser for [`crate::ToastStack`] (issue #29).
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
//! hit-test queries), [`RawWinToastSurface`], and the deprecated
//! [`draw_toast_stack`] compatibility shim over it, mirroring
//! `win::status_bar`'s identical #860 shape.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod toast;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::native_surface::NativeSurface;
use crate::primitives::toast::{ToastMeasure, ToastStack, ToastStackLayout};
use crate::theme::Theme;

const TOAST_WIDTH_DIP: f32 = 320.0;
const TOAST_MARGIN_DIP: f32 = 12.0;
const TOAST_GAP_DIP: f32 = 8.0;
const DISMISS_WIDTH_DIP: f32 = 28.0;
const ACTION_PADDING_DIP: f32 = 16.0;
const TOAST_PADDING_DIP: f32 = 8.0;

/// Compute a [`ToastStack`]'s layout without painting — the DirectWrite
/// measurer twin of the shared paint's internal layout computation. Both
/// use the identical per-toast measurer shape, so a no-paint hit-test
/// call always agrees with what the last paint drew. Still its own
/// DirectWrite-based measurer, independent of
/// [`crate::native_surface::NativeSurface::surface_measure_text`] — same
/// "no-paint layout stays put" posture as `WinBackend::status_bar_layout`
/// (#860).
pub fn win_toast_stack_layout(
    dwrite: &DWrite,
    rect: Rect,
    stack: &ToastStack,
    line_height: f32,
) -> ToastStackLayout {
    stack.layout(
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        TOAST_MARGIN_DIP,
        TOAST_GAP_DIP,
        |i| {
            let toast = &stack.toasts[i];
            let h = if toast.body.is_empty() {
                line_height + TOAST_PADDING_DIP * 2.0
            } else {
                line_height * 2.0 + TOAST_PADDING_DIP * 2.0
            };
            let action_w = toast
                .action
                .as_ref()
                .map(|a| {
                    let (w, _) = dwrite.measure_text(&a.label).unwrap_or((0.0, 0.0));
                    w + ACTION_PADDING_DIP
                })
                .unwrap_or(0.0);
            ToastMeasure {
                width: TOAST_WIDTH_DIP.min((rect.width - TOAST_MARGIN_DIP * 2.0).max(0.0)),
                height: h,
                dismiss_width: DISMISS_WIDTH_DIP,
                action_width: action_w,
            }
        },
    )
}

/// Minimal [`NativeSurface`] adapter over a raw `(&ID2D1RenderTarget,
/// &DWrite)` pair, used only by the deprecated [`draw_toast_stack`] shim
/// below — mirrors `win::form::RawFormSurface`'s identical pattern
/// (#808), scoped to the three verbs a toast's paint actually uses
/// (fill, plain text run, measure).
pub(crate) struct RawWinToastSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
    pub(crate) dwrite: &'a DWrite,
}

impl NativeSurface for RawWinToastSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawWinToastSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawWinToastSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawWinToastSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawWinToastSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawWinToastSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.dwrite.measure_text(text).unwrap_or((0.0, 0.0))
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: crate::Color) {
        let _ = fill_rect(self.target, rect, color);
    }

    fn surface_stroke_rect(&mut self, _rect: Rect, _color: crate::Color, _stroke_width: f32) {
        unreachable!("ToastStack::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: crate::Color) {
        let _ = self.dwrite.draw_text(self.target, text, rect, color);
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

    fn surface_push_clip(&mut self, _rect: Rect) {
        unreachable!("ToastStack::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("ToastStack::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("ToastStack::paint never draws an image")
    }
}

/// Deprecated free-function shim (#861, CLAUDE.md rule 8): reproduces
/// the pre-#861 signature exactly for any external caller that held a
/// direct `quadraui::win::draw_toast_stack` reference rather than going
/// through [`crate::Backend::draw_toast_stack`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
///
/// Unlike the pre-#861 version, this now requires a `theme: &Theme`
/// argument — the shared `paint` always takes one (matching `gtk`/
/// `macos`) — see this module's doc for why the pre-#861 signature's
/// `Theme::default()` was itself the bug being fixed here, not a shape
/// worth preserving byte-for-byte in the shim.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_toast_stack` instead — this free function is a compatibility shim over the shared #861 implementation"
)]
pub fn draw_toast_stack(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    stack: &ToastStack,
    theme: &Theme,
    line_height: f32,
) -> ToastStackLayout {
    let mut surface = RawWinToastSurface { target, dwrite };
    crate::primitives::toast::native_surface_paint::paint(
        stack,
        &mut surface,
        theme,
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        line_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toast::{ToastAction, ToastCorner, ToastHit, ToastItem, ToastSeverity};
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

    fn colored_toast(id: &str, title: &str) -> ToastItem {
        ToastItem {
            id: WidgetId::new(id),
            title: title.into(),
            body: "Details here".into(),
            severity: ToastSeverity::Info,
            action: Some(ToastAction {
                id: WidgetId::new(format!("{id}:act")),
                label: "Undo".into(),
            }),
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

    /// Paint↔click round trip: the toast box's painted fill colour lands
    /// at its own bounds, and `hit_test` resolves clicks on dismiss,
    /// action, and body to the matching `ToastHit`. Exercises the shared
    /// paint through [`RawWinToastSurface`] directly rather than the
    /// deprecated [`draw_toast_stack`] shim, so this test doesn't trip
    /// the `-D warnings`-denied `deprecated` lint (CLAUDE.md rule 3;
    /// mirrors `win::status_bar`'s identical #860 test-migration note).
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let stack = stack_br(vec![colored_toast("t1", "Saved")]);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let layout = surface
            .paint(|target| {
                let mut raw = RawWinToastSurface {
                    target,
                    dwrite: &dwrite,
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

        let ab = vt.action_bounds.expect("action bounds present");
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let painted = surface
            .paint(|target| {
                let mut raw = RawWinToastSurface {
                    target,
                    dwrite: &dwrite,
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let stack = stack_br(vec![colored_toast("t1", "Saved")]);
        let rect = Rect::new(20.0, 30.0, W as f32, H as f32);
        let theme = Theme::default();

        let surface = HeadlessSurface::new(W + 20, H + 30).expect("create surface");
        let layout = surface
            .paint(|target| {
                let mut raw = RawWinToastSurface {
                    target,
                    dwrite: &dwrite,
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
