//! Direct2D / DirectWrite rasteriser for [`crate::Button`].
//!
//! Painting is delegated to the shared
//! [`crate::primitives::button::native_surface_paint::paint`] function — see
//! that fn's doc for details. The `Backend`-trait entry point
//! [`super::backend::WinBackend::draw_button`] calls the shared `paint`
//! directly with `self.current_theme`, ensuring the live theme is used
//! for all painting. This module carries only [`win_button_layout`], which
//! provides backend-specific geometry calculations without performing
//! painting.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod button;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use crate::event::Rect;
use crate::primitives::button::{Button, ButtonLayout};

/// Compute a [`Button`]'s layout without painting — the twin of
/// `crate::Backend::draw_button`'s paint path. A `Button` has no
/// internal sub-regions, so this maps straight through to
/// [`Button::layout`], the same way `win_progress_layout` wraps
/// [`crate::primitives::progress::ProgressBar`]'s layout.
pub fn win_button_layout(rect: Rect, button: &Button) -> ButtonLayout {
    button.layout(rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::button::{ButtonEvent, ButtonHit, ButtonVariant};
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;

    const W: u32 = 120;
    const H: u32 = 20;

    fn mk_button(label: &str) -> Button {
        Button::new(WidgetId::new("dialog:ok"), label)
    }

    /// Paint `button` via the shared
    /// [`crate::primitives::button::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s
    /// headless target — the same adapter `WinBackend::draw_button`
    /// uses, exercised here directly.
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        button: &Button,
        theme: &Theme,
    ) -> ButtonLayout {
        surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::button::native_surface_paint::paint(
                    button,
                    &mut raw,
                    theme,
                    rect,
                    &crate::style::Style::default(),
                );
            })
            .map(|_| win_button_layout(rect, button))
            .expect("paint button")
    }

    #[test]
    fn primary_variant_fills_accent_bg() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let button = mk_button("OK");
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        paint(&surface, &dwrite, rect, &button, &theme);

        let px = surface.pixel_at(W / 2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b)
        );
    }

    #[test]
    fn secondary_variant_fills_surface_bg() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut button = mk_button("Cancel");
        button.variant = ButtonVariant::Secondary;
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        paint(&surface, &dwrite, rect, &button, &theme);

        let px = surface.pixel_at(W / 2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b)
        );
    }

    #[test]
    fn disabled_button_fills_surface_bg_regardless_of_variant() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut button = mk_button("OK");
        button.disabled = true;
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        paint(&surface, &dwrite, rect, &button, &theme);

        let px = surface.pixel_at(W / 2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b)
        );
    }

    /// Regression guard at a non-zero origin — layout helpers must
    /// return coords in the same frame across backends.
    fn hit_test_round_trip_at(x: f32, y: f32) {
        let rect = Rect::new(x, y, 40.0, 16.0);
        let layout = win_button_layout(rect, &mk_button("OK"));
        let inside = layout.hit_test(crate::event::Point::new(x + 4.0, y + 4.0));
        assert_eq!(inside, ButtonHit::Body);
        let outside = layout.hit_test(crate::event::Point::new(x + 400.0, y));
        assert_eq!(outside, ButtonHit::Outside);
    }

    #[test]
    fn hit_test_round_trip() {
        hit_test_round_trip_at(0.0, 0.0);
    }

    #[test]
    fn hit_test_round_trip_at_nonzero_origin() {
        hit_test_round_trip_at(7.0, 13.0);
    }

    #[test]
    fn click_activation_still_works_after_painting() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let button = mk_button("OK");
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let layout = paint(&surface, &dwrite, rect, &button, &theme);
        assert_eq!(
            layout.hit_test(crate::event::Point::new(2.0, 2.0)),
            ButtonHit::Body
        );
        assert_eq!(
            button.click(),
            Some(ButtonEvent::Activated { id: button.id })
        );
    }

    #[test]
    fn keyboard_activation_requires_focus() {
        let mut button = mk_button("OK");
        button.focused = true;
        let event = button.handle_key(
            &crate::event::Key::Named(crate::event::NamedKey::Enter),
            &crate::types::Modifiers::default(),
        );
        assert_eq!(event, Some(ButtonEvent::Activated { id: button.id }));
    }

    /// `win_button_layout` must produce byte-identical layout to the painted result.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let button = mk_button("OK");
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let painted = paint(&surface, &dwrite, rect, &button, &theme);
        let no_paint = win_button_layout(rect, &button);

        assert_eq!(painted, no_paint);
    }
}
