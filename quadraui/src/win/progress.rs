//! Direct2D / DirectWrite rasteriser for [`crate::ProgressBar`].
//!
//! Painting is delegated to the shared
//! [`crate::primitives::progress::native_surface_paint::paint`] function — see that
//! fn's doc for details. The `Backend`-trait entry point
//! [`super::backend::WinBackend::draw_progress`] calls the shared `paint` directly
//! with `self.current_theme`, ensuring the live theme is used for all painting.
//! This module now only carries [`win_progress_layout`], which provides backend-specific
//! geometry calculations without performing painting.
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
/// `mac_progress_layout` via [`pixel_progress_layout`].
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
    /// `draw_progress` shim used, exercised here directly.
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
                    bar,
                    &mut raw,
                    theme,
                    rect,
                    &crate::style::Style::default(),
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

    /// The painted fill must respect the supplied theme, not a hardcoded default.
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

    /// `win_progress_layout` must produce byte-identical layout to the painted result.
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
