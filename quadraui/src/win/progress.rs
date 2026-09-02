//! Direct2D / DirectWrite rasteriser for [`crate::ProgressBar`] (issue
//! #29).
//!
//! Mirrors `gtk::progress`'s structure: [`ProgressBar::layout`] (the D6
//! layout API — see that primitive's module doc) computes fill/cancel
//! geometry; this module paints the track, the fill (determinate) or a
//! sliding pulse driven by `frame_idx` (indeterminate — `value.is_none()`),
//! the label, and the cancel glyph.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod progress;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] — see `win::status_bar`'s
//! module doc for the "placeholder until a later issue wires the app's
//! real theme through" posture this module shares.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::progress::{ProgressBar, ProgressBarLayout, ProgressBarMeasure};
use crate::theme::Theme;

/// Width (DIPs) reserved for the cancel affordance — the DirectWrite
/// twin of `gtk::progress::GTK_CANCEL_WIDTH_PX`.
pub const CANCEL_WIDTH_DIP: f32 = 28.0;
/// Width (DIPs) of the sliding indeterminate pulse.
const PULSE_WIDTH_DIP: f32 = 40.0;

/// Compute a [`ProgressBar`]'s layout without painting — the twin of
/// [`draw_progress`]. Both call [`ProgressBar::layout`] with the
/// identical cancel-affordance width, so a no-paint hit-test call
/// always agrees with what the last paint drew.
pub fn win_progress_layout(rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
    let cancel_width = if bar.cancellable {
        CANCEL_WIDTH_DIP
    } else {
        0.0
    };
    bar.layout(
        rect.x,
        rect.y,
        ProgressBarMeasure {
            width: rect.width,
            height: rect.height,
            cancel_width,
        },
    )
}

/// Draw a [`ProgressBar`] onto `target`. Returns the layout for host
/// click dispatch.
pub fn draw_progress(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &ProgressBar,
) -> ProgressBarLayout {
    let layout = win_progress_layout(rect, bar);
    let theme = Theme::default();

    // Track background.
    let _ = fill_rect(target, rect, theme.surface_bg);

    let fill_color = bar.accent.unwrap_or(theme.accent_bg);
    if let Some(fb) = layout.fill_bounds {
        let _ = fill_rect(target, fb, fill_color);
    } else {
        // Indeterminate: a sliding pulse driven by `frame_idx`, same
        // formula as `gtk::progress::draw_progress`.
        let bar_w = if bar.cancellable {
            (rect.width - CANCEL_WIDTH_DIP).max(0.0)
        } else {
            rect.width
        };
        if bar_w > 0.0 {
            let pulse_w = PULSE_WIDTH_DIP.min(bar_w);
            let pos = (bar.frame_idx as f32 * 4.0) % bar_w;
            let w = pulse_w.min(bar_w - pos);
            if w > 0.0 {
                let pulse_rect = Rect::new(rect.x + pos, rect.y, w, rect.height);
                let _ = fill_rect(target, pulse_rect, fill_color);
            }
        }
    }

    if !bar.label.is_empty() {
        let label_rect = Rect::new(
            rect.x + 4.0,
            rect.y,
            (rect.width - 4.0).max(0.0),
            rect.height,
        );
        let _ = dwrite.draw_text(target, &bar.label, label_rect, theme.foreground);
    }

    if let Some(cb) = layout.cancel_bounds {
        let (glyph_w, _) = dwrite.measure_text("\u{d7}").unwrap_or((0.0, 0.0));
        let glyph_x = cb.x + ((cb.width - glyph_w) / 2.0).max(0.0);
        let glyph_rect = Rect::new(
            glyph_x,
            cb.y,
            (cb.x + cb.width - glyph_x).max(1.0),
            cb.height,
        );
        let _ = dwrite.draw_text(target, "\u{d7}", glyph_rect, theme.foreground);
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::progress::ProgressBarHit;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 20;

    fn bar(value: Option<f32>, cancellable: bool) -> ProgressBar {
        ProgressBar {
            id: WidgetId::new("p"),
            label: String::new(),
            value,
            frame_idx: 0,
            cancellable,
            accent: None,
        }
    }

    /// Determinate mode: the fill's painted colour lands at its own
    /// bounds, and a click past the filled fraction (but inside the
    /// track) still resolves to `Body`.
    #[test]
    fn determinate_fill_paints_and_hit_tests() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar(Some(0.5), false);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let layout = surface
            .paint(|target| {
                draw_progress(target, &dwrite, rect, &bar);
            })
            .map(|_| win_progress_layout(rect, &bar))
            .expect("paint progress");

        let theme = Theme::default();
        let fb = layout.fill_bounds.expect("determinate fill present");
        assert!((fb.width - W as f32 * 0.5).abs() < 0.01);

        let fill_px = surface.pixel_at(2, H / 2);
        assert_eq!(
            (fill_px.r, fill_px.g, fill_px.b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b)
        );

        // Past the fill, still on the track: paints the track colour,
        // not the fill, and hit-tests to `Body` (no cancel affordance).
        let track_px = surface.pixel_at(W - 2, H / 2);
        assert_ne!(
            (track_px.r, track_px.g, track_px.b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b)
        );
        let hit = layout.hit_test(W as f32 - 2.0, H as f32 / 2.0);
        assert_eq!(hit, ProgressBarHit::Body(WidgetId::new("p")));
    }

    /// Indeterminate mode animates via `frame_idx`: two different frame
    /// indices paint the pulse at different positions.
    #[test]
    fn indeterminate_mode_animates_via_frame_idx() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let mut bar0 = bar(None, false);
        bar0.frame_idx = 0;
        let surface0 = HeadlessSurface::new(W, H).expect("create surface");
        surface0
            .paint(|target| {
                draw_progress(target, &dwrite, rect, &bar0);
            })
            .expect("paint frame 0");

        let mut bar1 = bar(None, false);
        bar1.frame_idx = 5;
        let surface1 = HeadlessSurface::new(W, H).expect("create surface");
        surface1
            .paint(|target| {
                draw_progress(target, &dwrite, rect, &bar1);
            })
            .expect("paint frame 5");

        // The pulse starts at x=0 on frame 0 (lit) and has moved past
        // x=0 by frame 5 (frame 0's leading pixel should no longer be
        // lit — it now shows the plain track colour).
        let px0 = surface0.pixel_at(1, H / 2);
        let px1 = surface1.pixel_at(1, H / 2);
        assert_ne!(
            (px0.r, px0.g, px0.b),
            (px1.r, px1.g, px1.b),
            "advancing frame_idx should move the indeterminate pulse"
        );
    }

    /// Cancellable bars reserve a trailing cancel affordance that
    /// hit-tests to `Cancel`, distinct from the bar body.
    #[test]
    fn cancellable_bar_hit_tests_cancel_affordance() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let bar = bar(Some(1.0), true);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let layout = surface
            .paint(|target| {
                draw_progress(target, &dwrite, rect, &bar);
            })
            .map(|_| win_progress_layout(rect, &bar))
            .expect("paint progress");

        let cb = layout.cancel_bounds.expect("cancel bounds present");
        let hit = layout.hit_test(cb.x + cb.width / 2.0, cb.y + cb.height / 2.0);
        assert_eq!(hit, ProgressBarHit::Cancel(WidgetId::new("p")));
    }

    /// `win_progress_layout` (no-paint) must produce byte-identical
    /// layout to what `draw_progress` used to paint.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let bar = bar(Some(0.3), true);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let painted = surface
            .paint(|target| {
                draw_progress(target, &dwrite, rect, &bar);
            })
            .map(|_| win_progress_layout(rect, &bar))
            .expect("paint");
        let no_paint = win_progress_layout(rect, &bar);

        assert_eq!(painted, no_paint);
    }
}
