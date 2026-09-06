//! Direct2D / DirectWrite rasteriser for [`crate::ToastStack`] (issue #29).
//!
//! Mirrors `gtk::toast`'s structure: [`ToastStack::layout`] (the D6
//! layout API — see that primitive's module doc) computes per-toast
//! stacking + sub-region geometry (dismiss / action / body); this
//! module measures (title/body/action-label widths via DirectWrite) and
//! paints (box fill, title/body text, dismiss glyph, action label).
//!
//! `rect` doubles as both the overlay's absolute origin (`rect.x`,
//! `rect.y`) and its viewport size (`rect.width`, `rect.height`) —
//! matching every other Win-GUI rasteriser's single-`Rect` `Backend`
//! trait signature (`crate::Backend::draw_toast_stack`) and
//! `gtk_toast_stack_layout`'s origin-aware convention (quadraui#494).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod toast;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] — see `win::status_bar`'s
//! module doc for the "placeholder until a later issue wires the app's
//! real theme through" posture this module shares.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::toast::{
    ToastItem, ToastMeasure, ToastSeverity, ToastStack, ToastStackLayout, VisibleToast,
};
use crate::theme::Theme;
use crate::types::Color;

const TOAST_WIDTH_DIP: f32 = 320.0;
const TOAST_MARGIN_DIP: f32 = 12.0;
const TOAST_GAP_DIP: f32 = 8.0;
const DISMISS_WIDTH_DIP: f32 = 28.0;
const ACTION_PADDING_DIP: f32 = 16.0;
const TOAST_PADDING_DIP: f32 = 8.0;

fn severity_bg(severity: ToastSeverity, theme: &Theme) -> Color {
    match severity {
        ToastSeverity::Info => theme.surface_bg,
        ToastSeverity::Success => Color::rgb(30, 80, 30),
        ToastSeverity::Warning => Color::rgb(100, 80, 20),
        ToastSeverity::Error => theme.error_fg,
    }
}

/// Compute a [`ToastStack`]'s layout without painting — the DirectWrite
/// measurer twin of [`draw_toast_stack`]. Both call [`ToastStack::layout`]
/// with the identical per-toast measurer, so a no-paint hit-test call
/// always agrees with what the last paint drew.
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

/// Draw a [`ToastStack`] overlay onto `target`. Returns the layout for
/// host click dispatch.
pub fn draw_toast_stack(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    stack: &ToastStack,
    line_height: f32,
) -> ToastStackLayout {
    let layout = win_toast_stack_layout(dwrite, rect, stack, line_height);

    for vt in &layout.visible_toasts {
        let toast = &stack.toasts[vt.toast_idx];
        paint_toast(target, dwrite, vt, toast, line_height);
    }

    layout
}

fn paint_toast(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    vt: &VisibleToast,
    toast: &ToastItem,
    line_height: f32,
) {
    let theme = Theme::default();
    let bg = toast
        .accent
        .unwrap_or_else(|| severity_bg(toast.severity, &theme));
    let _ = fill_rect(target, vt.bounds, bg);

    let title_rect = Rect::new(
        vt.bounds.x + TOAST_PADDING_DIP,
        vt.bounds.y + TOAST_PADDING_DIP,
        (vt.bounds.width - TOAST_PADDING_DIP * 2.0).max(0.0),
        line_height,
    );
    let _ = dwrite.draw_text(target, &toast.title, title_rect, theme.foreground);

    if !toast.body.is_empty() {
        let body_rect = Rect::new(
            title_rect.x,
            title_rect.y + line_height,
            title_rect.width,
            line_height,
        );
        let _ = dwrite.draw_text(target, &toast.body, body_rect, theme.foreground);
    }

    if let Some(db) = vt.dismiss_bounds {
        let _ = dwrite.draw_text(target, "\u{d7}", db, theme.foreground);
    }

    if let Some(ab) = vt.action_bounds {
        if let Some(ref action) = toast.action {
            let _ = dwrite.draw_text(target, &action.label, ab, theme.accent_fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toast::{ToastAction, ToastCorner, ToastHit};
    use crate::types::WidgetId;
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
    /// action, and body to the matching `ToastHit`.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let stack = stack_br(vec![colored_toast("t1", "Saved")]);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let layout = surface
            .paint(|target| {
                draw_toast_stack(target, &dwrite, rect, &stack, LINE_HEIGHT);
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
    /// layout to what `draw_toast_stack` used to paint — same stack,
    /// same rect, same line height.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let stack = stack_br(vec![colored_toast("t1", "Saved")]);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let painted = surface
            .paint(|target| {
                draw_toast_stack(target, &dwrite, rect, &stack, LINE_HEIGHT);
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

        let surface = HeadlessSurface::new(W + 20, H + 30).expect("create surface");
        let layout = surface
            .paint(|target| {
                draw_toast_stack(target, &dwrite, rect, &stack, LINE_HEIGHT);
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
