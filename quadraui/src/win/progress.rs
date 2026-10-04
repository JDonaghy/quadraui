//! Direct2D / DirectWrite rasteriser for [`crate::ProgressBar`] (issue
//! #29).
//!
//! Painting moved to the shared
//! [`crate::primitives::progress::native_surface_paint::paint`] (#1085,
//! `PaintSurface` Phase 4 slice 8/8) — see that fn's doc for the one
//! named divergence this backend was the source of: this module's
//! pre-#1085 `draw_progress` took no `theme` parameter at all and called
//! `Theme::default()` internally on every paint, so a Win-GUI host
//! running any theme other than the default painted every progress bar
//! in the wrong colours.
//!
//! [`super::backend::WinBackend::draw_progress`] (the sanctioned,
//! `Backend`-trait entry point, and the one every in-tree call site
//! already uses) no longer goes through a free function at all — it
//! calls the shared `paint` directly with `self.current_theme`, the
//! same live theme every other Win-GUI rasteriser already uses, which
//! is where the fix actually lands. The deprecated `draw_progress` free
//! function (which, matching its pre-#1085 self, always hardcoded
//! `Theme::default()` internally) was removed in issue #1109 (zero uses
//! in coord-tui's `main` and vimcode's `develop`). This module now only
//! carries [`win_progress_layout`] (still real, backend-specific pure
//! geometry — no painting involved).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod progress;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

#[cfg(test)]
use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::layout_metrics::pixel_progress_layout;
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute a [`ProgressBar`]'s layout without painting — the twin of
/// `crate::Backend::draw_progress` (the removed `draw_progress` free
/// function shim this backed — see module doc). Both call
/// [`ProgressBar::layout`] with the identical cancel-affordance width,
/// so a no-paint hit-test call always agrees with what the last paint
/// drew. Shares that width with `gtk_progress_layout` /
/// `mac_progress_layout` via [`pixel_progress_layout`] (issue #1079).
pub fn win_progress_layout(rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
    pixel_progress_layout(bar, rect.x, rect.y, rect.width, rect.height)
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

    /// Paint `bar` via the shared
    /// [`crate::primitives::progress::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s
    /// headless target — the same adapter the now-removed
    /// `draw_progress` shim used (issue #1109), exercised here directly.
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        bar: &ProgressBar,
        theme: &Theme,
    ) -> ProgressBarLayout {
        surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::progress::native_surface_paint::paint(
                    bar, &mut raw, theme, rect,
                );
            })
            .map(|_| win_progress_layout(rect, bar))
            .expect("paint progress")
    }

    /// Determinate mode: the fill's painted colour lands at its own
    /// bounds, and a click past the filled fraction (but inside the
    /// track) still resolves to `Body`.
    #[test]
    fn determinate_fill_paints_and_hit_tests() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar(Some(0.5), false);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let theme = Theme::default();
        let layout = paint(&surface, &dwrite, rect, &bar, &theme);

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

    /// Regression for #1085's one named divergence: `draw_progress` used
    /// to hardcode `Theme::default()` internally, ignoring whatever theme
    /// the host was actually running. A non-default `accent_bg` must now
    /// show up in the painted fill.
    #[test]
    fn paints_the_caller_supplied_theme_not_a_hardcoded_default() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar(Some(0.5), false);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let custom_accent = crate::types::Color::rgb(12, 200, 250);
        assert_ne!(
            custom_accent,
            Theme::default().accent_bg,
            "fixture must actually differ from the default theme to be a real regression guard",
        );
        let theme = Theme {
            accent_bg: custom_accent,
            ..Theme::default()
        };

        paint(&surface, &dwrite, rect, &bar, &theme);

        let fill_px = surface.pixel_at(2, H / 2);
        assert_eq!(
            (fill_px.r, fill_px.g, fill_px.b),
            (custom_accent.r, custom_accent.g, custom_accent.b),
            "draw_progress must paint the caller's theme, not Theme::default()",
        );
    }

    /// Indeterminate mode animates via `frame_idx`: two different frame
    /// indices paint the pulse at different positions.
    #[test]
    fn indeterminate_mode_animates_via_frame_idx() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let mut bar0 = bar(None, false);
        bar0.frame_idx = 0;
        let surface0 = HeadlessSurface::new(W, H).expect("create surface");
        paint(&surface0, &dwrite, rect, &bar0, &theme);

        let mut bar1 = bar(None, false);
        bar1.frame_idx = 5;
        let surface1 = HeadlessSurface::new(W, H).expect("create surface");
        paint(&surface1, &dwrite, rect, &bar1, &theme);

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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar(Some(1.0), true);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let layout = paint(&surface, &dwrite, rect, &bar, &theme);

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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let painted = paint(&surface, &dwrite, rect, &bar, &theme);
        let no_paint = win_progress_layout(rect, &bar);

        assert_eq!(painted, no_paint);
    }
}
