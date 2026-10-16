//! macOS rasteriser for [`crate::Button`].
//!
//! Painting is handled by the shared
//! [`crate::primitives::button::native_surface_paint::paint`], same as
//! [`crate::macos::progress`]'s `ProgressBar`. This module provides
//! [`mac_button_layout`], the backend-specific geometry calculation.

use crate::primitives::button::{Button, ButtonLayout};

/// Compute the macOS pixel-unit layout for a [`Button`] without
/// painting. A `Button` has no internal sub-regions, so this maps
/// straight through to [`Button::layout`].
pub fn mac_button_layout(button: &Button, x: f64, y: f64, w: f64, h: f64) -> ButtonLayout {
    button.layout(crate::event::Rect::new(
        x as f32, y as f32, w as f32, h as f32,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Point, Rect as QRect, Viewport};
    use crate::primitives::button::{ButtonEvent, ButtonHit, ButtonVariant};
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use crate::Backend;
    use core_text::font::CTFont;

    const W: u32 = 120;
    const H: u32 = 20;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn mk_button(label: &str) -> Button {
        Button::new(WidgetId::new("dialog:ok"), label)
    }

    /// Paint via the real `Backend::draw_button` path — routes through
    /// the shared `native_surface_paint::paint`.
    fn paint_via_backend(button: &Button) -> (BitmapSurface, ButtonLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_button(QRect::new(0.0, 0.0, W as f32, H as f32), button);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn primary_variant_fills_accent_bg() {
        let button = mk_button("OK");
        let (surface, _layout) = paint_via_backend(&button);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(W / 2, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b),
        );
    }

    #[test]
    fn secondary_variant_fills_surface_bg() {
        let mut button = mk_button("Cancel");
        button.variant = ButtonVariant::Secondary;
        let (surface, _layout) = paint_via_backend(&button);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(W / 2, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
        );
    }

    #[test]
    fn disabled_button_fills_surface_bg_regardless_of_variant() {
        let mut button = mk_button("OK");
        button.disabled = true;
        let (surface, _layout) = paint_via_backend(&button);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(W / 2, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
        );
    }

    /// Regression guard at a non-zero origin — LESSONS.md "Layout
    /// helpers must return coords in the same frame across backends".
    fn hit_test_round_trip_at(origin_x: f64, origin_y: f64) {
        let layout = mac_button_layout(&mk_button("OK"), origin_x, origin_y, 40.0, 16.0);
        let inside = layout.hit_test(Point::new(origin_x as f32 + 4.0, origin_y as f32 + 4.0));
        assert_eq!(inside, ButtonHit::Body);
        let outside = layout.hit_test(Point::new(origin_x as f32 + 400.0, origin_y as f32));
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
        let button = mk_button("OK");
        let (_surface, layout) = paint_via_backend(&button);
        assert_eq!(layout.hit_test(Point::new(2.0, 2.0)), ButtonHit::Body);
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
}
