//! Direct2D / DirectWrite rasteriser for [`crate::RadioGroup`].
//!
//! Painting is delegated to the shared
//! [`crate::primitives::radio_group::native_surface_paint::paint`] function —
//! see that fn's doc for details. The `Backend`-trait entry point
//! [`super::backend::WinBackend::draw_radio_group`] calls the shared
//! `paint` directly with `self.current_theme` and `self.current_line_height`
//! as the per-option row height, ensuring the live theme is used for
//! all painting. This module carries only [`win_radio_group_layout`],
//! which provides backend-specific geometry calculations without
//! performing painting.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod radio_group;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use crate::event::Rect;
use crate::primitives::radio_group::{
    RadioGroup, RadioGroupLayout, RadioOption, RadioOptionMeasure,
};

/// Compute a [`RadioGroup`]'s layout without painting — the twin of
/// `crate::Backend::draw_radio_group`'s paint path. Both stack each
/// option's row at the same `row_height` (the chrome line height), so a
/// no-paint hit-test call always agrees with what the last paint drew.
pub fn win_radio_group_layout(rect: Rect, group: &RadioGroup, row_height: f32) -> RadioGroupLayout {
    group.layout(rect, |_option: &RadioOption| RadioOptionMeasure {
        width: rect.width,
        height: row_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::event::{Key, NamedKey};
    use crate::primitives::radio_group::{RadioGroupEvent, RadioGroupHit};
    use crate::theme::Theme;
    use crate::types::{Modifiers, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;

    const W: u32 = 120;
    const H: u32 = 60;
    const ROW_HEIGHT: f32 = 20.0;

    fn options() -> Vec<RadioOption> {
        vec![
            RadioOption::new(WidgetId::new("theme:light"), "Light"),
            RadioOption::new(WidgetId::new("theme:dark"), "Dark"),
            RadioOption::new(WidgetId::new("theme:auto"), "Auto"),
        ]
    }

    fn mk_group() -> RadioGroup {
        RadioGroup::new(WidgetId::new("settings:theme"), options())
    }

    /// Paint `group` via the shared
    /// [`crate::primitives::radio_group::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s
    /// headless target — the same adapter `WinBackend::draw_radio_group`
    /// uses, exercised here directly.
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        group: &RadioGroup,
        theme: &Theme,
    ) -> RadioGroupLayout {
        surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::radio_group::native_surface_paint::paint(
                    group, &mut raw, theme, rect, ROW_HEIGHT,
                );
            })
            .map(|_| win_radio_group_layout(rect, group, ROW_HEIGHT))
            .expect("paint radio group")
    }

    /// Scan the marker's square region (top-left `item.height`-sized
    /// box of each row) for any pixel matching `color`, the same probe
    /// idiom `macos::radio_group`'s tests use since the exact marker
    /// shape (filled disc vs. stroked ring) isn't itself under test.
    fn marker_contains_color(
        surface: &HeadlessSurface,
        item: Rect,
        color: crate::types::Color,
    ) -> bool {
        let x0 = item.x.round() as u32;
        let y0 = item.y.round() as u32;
        let size = item.height.min(item.width).max(0.0).round() as u32;
        for dy in 0..size {
            for dx in 0..size {
                let px = surface.pixel_at(x0 + dx, y0 + dy);
                if (px.r, px.g, px.b) == (color.r, color.g, color.b) {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn selected_marker_paints_accent_colour() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut group = mk_group();
        group.selected = Some(1);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let layout = paint(&surface, &dwrite, rect, &group, &theme);

        let item = layout.items[1];
        assert!(marker_contains_color(&surface, item, theme.accent_fg));
    }

    #[test]
    fn disabled_option_marker_paints_muted_colour() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut group = mk_group();
        group.options[0].disabled = true;
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let layout = paint(&surface, &dwrite, rect, &group, &theme);

        let item = layout.items[0];
        assert!(marker_contains_color(&surface, item, theme.muted_fg));
    }

    /// Regression guard at a non-zero origin — layout helpers must
    /// return coords in the same frame across backends.
    fn hit_test_round_trip_at(x: f32, y: f32) {
        let rect = Rect::new(x, y, 40.0, 60.0);
        let group = mk_group();
        let layout = win_radio_group_layout(rect, &group, ROW_HEIGHT);
        assert_eq!(
            layout.hit_test(Point::new(x + 1.0, y + 1.0)),
            RadioGroupHit::Option(0)
        );
        assert_eq!(
            layout.hit_test(Point::new(x + 1.0, y + ROW_HEIGHT + 1.0)),
            RadioGroupHit::Option(1)
        );
        assert_eq!(
            layout.hit_test(Point::new(x + 1000.0, y)),
            RadioGroupHit::Outside
        );
    }

    #[test]
    fn hit_test_round_trip() {
        hit_test_round_trip_at(0.0, 0.0);
    }

    #[test]
    fn hit_test_round_trip_at_nonzero_origin() {
        hit_test_round_trip_at(5.0, 9.0);
    }

    #[test]
    fn click_selects_option_after_painting() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let group = mk_group();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let layout = paint(&surface, &dwrite, rect, &group, &theme);
        let hit = layout.hit_test(Point::new(2.0, ROW_HEIGHT + 2.0));
        assert_eq!(hit, RadioGroupHit::Option(1));
        assert_eq!(
            group.select(1),
            Some(RadioGroupEvent::SelectionChanged { index: 1 })
        );
    }

    #[test]
    fn keyboard_moves_cursor_when_focused() {
        let mut group = mk_group();
        group.focused = true;
        let event = group.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default());
        assert_eq!(event, Some(RadioGroupEvent::SelectionChanged { index: 0 }));
    }

    /// `win_radio_group_layout` must produce byte-identical layout to
    /// the painted result.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let group = mk_group();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let painted = paint(&surface, &dwrite, rect, &group, &theme);
        let no_paint = win_radio_group_layout(rect, &group, ROW_HEIGHT);

        assert_eq!(painted, no_paint);
    }
}
