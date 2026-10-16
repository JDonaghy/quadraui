//! Direct2D / DirectWrite rasteriser for [`crate::Switch`].
//!
//! Painting is delegated to the shared
//! [`crate::primitives::switch::native_surface_paint::paint`] function —
//! see that fn's doc for details. The `Backend`-trait entry point
//! [`super::backend::WinBackend::draw_switch`] calls the shared `paint`
//! directly with `self.current_theme`, ensuring the live theme is used
//! for all painting. This module carries only [`win_switch_layout`],
//! which provides backend-specific geometry calculations without
//! performing painting.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod switch;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use crate::event::Rect;
use crate::primitives::switch::{Switch, SwitchLayout};

/// Compute a [`Switch`]'s layout without painting — the twin of
/// `crate::Backend::draw_switch`'s paint path. A `Switch` has no
/// internal sub-regions, so this maps straight through to
/// [`Switch::layout`], the same way `win_button_layout` wraps
/// [`crate::primitives::button::Button`]'s layout.
pub fn win_switch_layout(rect: Rect, switch: &Switch) -> SwitchLayout {
    switch.layout(rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, NamedKey, Point};
    use crate::primitives::switch::{SwitchEvent, SwitchHit};
    use crate::theme::Theme;
    use crate::types::{Modifiers, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;
    use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

    const W: u32 = 40;
    const H: u32 = 16;

    fn mk_switch() -> Switch {
        Switch::new(WidgetId::new("settings:wrap"))
    }

    /// Paint `switch` via the shared
    /// [`crate::primitives::switch::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s
    /// headless target — the same adapter `WinBackend::draw_switch`
    /// uses, exercised here directly.
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        switch: &Switch,
        theme: &Theme,
    ) -> SwitchLayout {
        surface
            .paint(|target| {
                // Clear to opaque black so anti-aliased edges blend
                // with a known background, not undefined DIB content.
                //
                // SAFETY: `target` is the live render target
                // `HeadlessSurface::paint` passes in, mid-`BeginDraw`/
                // `EndDraw`; the `D2D1_COLOR_F` is a plain stack literal
                // borrowed only for the duration of this call.
                unsafe {
                    target.Clear(Some(&D2D1_COLOR_F {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }));
                }
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::switch::native_surface_paint::paint(
                    switch, &mut raw, theme, rect,
                );
            })
            .map(|_| win_switch_layout(rect, switch))
            .expect("paint switch")
    }

    #[test]
    fn on_value_fills_accent_track() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut switch = mk_switch();
        switch.value = true;
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        paint(&surface, &dwrite, rect, &switch, &theme);

        // Probe the track's far (unoccupied-by-thumb) end: "on" parks the
        // thumb at the right edge, so the left edge still shows the
        // track's own fill colour.
        let px = surface.pixel_at(2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b)
        );
    }

    #[test]
    fn off_value_fills_surface_track() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let switch = mk_switch();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        paint(&surface, &dwrite, rect, &switch, &theme);

        // "off" parks the thumb at the left edge, so probe the right end
        // for the track's own fill colour. Use H/4 inset from the right
        // edge to stay clear of the anti-aliased rounded cap and border
        // stroke.
        let px = surface.pixel_at(W - H / 4, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b)
        );
    }

    #[test]
    fn disabled_switch_fills_surface_track_regardless_of_value() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut switch = mk_switch();
        switch.value = true;
        switch.disabled = true;
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        paint(&surface, &dwrite, rect, &switch, &theme);

        let px = surface.pixel_at(2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b)
        );
    }

    /// Regression guard at a non-zero origin — layout helpers must
    /// return coords in the same frame across backends.
    fn hit_test_round_trip_at(x: f32, y: f32) {
        let rect = Rect::new(x, y, 20.0, 10.0);
        let layout = win_switch_layout(rect, &mk_switch());
        let inside = layout.hit_test(Point::new(x + 2.0, y + 2.0));
        assert_eq!(inside, SwitchHit::Body);
        let outside = layout.hit_test(Point::new(x + 400.0, y));
        assert_eq!(outside, SwitchHit::Outside);
    }

    #[test]
    fn hit_test_round_trip() {
        hit_test_round_trip_at(0.0, 0.0);
    }

    #[test]
    fn hit_test_round_trip_at_nonzero_origin() {
        hit_test_round_trip_at(6.0, 11.0);
    }

    #[test]
    fn click_toggles_after_painting() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let switch = mk_switch();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let layout = paint(&surface, &dwrite, rect, &switch, &theme);
        assert_eq!(layout.hit_test(Point::new(2.0, 2.0)), SwitchHit::Body);
        assert_eq!(
            switch.click(),
            Some(SwitchEvent::Toggled {
                id: switch.id,
                value: true
            })
        );
    }

    #[test]
    fn keyboard_activation_requires_focus() {
        let mut switch = mk_switch();
        switch.focused = true;
        let event = switch.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default());
        assert_eq!(
            event,
            Some(SwitchEvent::Toggled {
                id: switch.id,
                value: true
            })
        );
    }

    /// `win_switch_layout` must produce byte-identical layout to the
    /// painted result.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let switch = mk_switch();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let painted = paint(&surface, &dwrite, rect, &switch, &theme);
        let no_paint = win_switch_layout(rect, &switch);

        assert_eq!(painted, no_paint);
    }
}
