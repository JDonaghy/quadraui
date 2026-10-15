//! macOS rasteriser for [`crate::Switch`].
//!
//! Painting is handled by the shared
//! [`crate::primitives::switch::native_surface_paint::paint`], same as
//! [`crate::macos::progress`]'s `ProgressBar`. This module provides
//! [`mac_switch_layout`], the backend-specific geometry calculation.

use crate::primitives::switch::{Switch, SwitchLayout};

/// Compute the macOS pixel-unit layout for a [`Switch`] without
/// painting. A `Switch` has no internal sub-regions, so this maps
/// straight through to [`Switch::layout`].
pub fn mac_switch_layout(switch: &Switch, x: f64, y: f64, w: f64, h: f64) -> SwitchLayout {
    switch.layout(crate::event::Rect::new(
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
    use crate::primitives::switch::{SwitchEvent, SwitchHit};
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use crate::Backend;
    use core_text::font::CTFont;

    const W: u32 = 32;
    const H: u32 = 16;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn mk_switch() -> Switch {
        Switch::new(WidgetId::new("settings:wrap"))
    }

    /// Paint via the real `Backend::draw_switch` path — routes through
    /// the shared `native_surface_paint::paint`.
    fn paint_via_backend(switch: &Switch) -> (BitmapSurface, SwitchLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_switch(QRect::new(0.0, 0.0, W as f32, H as f32), switch);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// `native_surface_paint::paint` draws the track as a stadium
    /// (rounded rect, radius `height / 2`) and the thumb as a filled
    /// circle at the "on"/"off" end. When `value` is `true` the thumb
    /// sits on the trailing (right) half, so the leading end's rounded
    /// cap is bare track — sample there, offset from its own center so
    /// the anti-aliased edge blend doesn't shift the color.
    fn track_sample_point(bounds: QRect) -> (u32, u32) {
        (
            (bounds.x + bounds.height / 4.0) as u32,
            (bounds.y + bounds.height / 2.0) as u32,
        )
    }

    #[test]
    fn on_value_fills_accent_bg() {
        let mut switch = mk_switch();
        switch.value = true;
        let (surface, layout) = paint_via_backend(&switch);
        let theme = Theme::default();
        let (x, y) = track_sample_point(layout.bounds);
        let (r, g, b, _) = surface.pixel(x, y);
        assert_eq!(
            (r, g, b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b)
        );
    }

    #[test]
    fn disabled_switch_dims_regardless_of_value() {
        let mut switch = mk_switch();
        switch.value = true;
        switch.disabled = true;
        let (surface, layout) = paint_via_backend(&switch);
        let theme = Theme::default();
        let (x, y) = track_sample_point(layout.bounds);
        let (r, g, b, _) = surface.pixel(x, y);
        assert_ne!(
            (r, g, b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b)
        );
    }

    /// Regression guard at a non-zero origin — LESSONS.md "Layout
    /// helpers must return coords in the same frame across backends".
    fn hit_test_round_trip_at(origin_x: f64, origin_y: f64) {
        let layout = mac_switch_layout(&mk_switch(), origin_x, origin_y, 8.0, 4.0);
        let inside = layout.hit_test(Point::new(origin_x as f32 + 1.0, origin_y as f32 + 1.0));
        assert_eq!(inside, SwitchHit::Body);
        let outside = layout.hit_test(Point::new(origin_x as f32 + 400.0, origin_y as f32));
        assert_eq!(outside, SwitchHit::Outside);
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
    fn click_toggles_value_after_painting() {
        let switch = mk_switch();
        let (_surface, layout) = paint_via_backend(&switch);
        assert_eq!(layout.hit_test(Point::new(2.0, 2.0)), SwitchHit::Body);
        assert_eq!(
            switch.click(),
            Some(SwitchEvent::Toggled {
                id: switch.id.clone(),
                value: true
            })
        );
    }

    #[test]
    fn keyboard_activation_requires_focus() {
        let mut switch = mk_switch();
        switch.focused = true;
        let event = switch.handle_key(
            &crate::event::Key::Named(crate::event::NamedKey::Enter),
            &crate::types::Modifiers::default(),
        );
        assert_eq!(
            event,
            Some(SwitchEvent::Toggled {
                id: switch.id.clone(),
                value: true
            })
        );
    }
}
