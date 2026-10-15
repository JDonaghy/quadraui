//! macOS rasteriser for [`crate::RadioGroup`].
//!
//! Painting is handled by the shared
//! [`crate::primitives::radio_group::native_surface_paint::paint`], same as
//! [`crate::macos::progress`]'s `ProgressBar`. This module provides
//! [`mac_radio_group_layout`], the backend-specific geometry calculation.

use crate::primitives::radio_group::{
    RadioGroup, RadioGroupLayout, RadioOption, RadioOptionMeasure,
};

/// Compute the macOS pixel-unit layout for a [`RadioGroup`] without
/// painting. The per-option measure is nominal (`w` × `row_height`),
/// mirroring [`crate::gtk::backend::GtkBackend::radio_group_layout`] —
/// there is no native text-metrics surface to measure against yet.
pub fn mac_radio_group_layout(
    group: &RadioGroup,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    row_height: f64,
) -> RadioGroupLayout {
    let bounds = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    group.layout(bounds, |_option: &RadioOption| RadioOptionMeasure {
        width: w as f32,
        height: row_height as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Point, Rect as QRect, Viewport};
    use crate::primitives::radio_group::{RadioGroupEvent, RadioGroupHit};
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use crate::Backend;
    use core_text::font::CTFont;

    const W: u32 = 120;
    const H: u32 = 40;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn mk_group() -> RadioGroup {
        RadioGroup::new(
            WidgetId::new("settings:theme"),
            vec![
                RadioOption::new(WidgetId::new("settings:theme:light"), "Light"),
                RadioOption::new(WidgetId::new("settings:theme:dark"), "Dark"),
            ],
        )
    }

    /// Paint via the real `Backend::draw_radio_group` path — routes
    /// through the shared `native_surface_paint::paint`.
    fn paint_via_backend(group: &RadioGroup) -> (BitmapSurface, RadioGroupLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_radio_group(QRect::new(0.0, 0.0, W as f32, H as f32), group);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// `native_surface_paint::paint` draws the marker as a filled
    /// circle inscribed in the `item.height`-sized square at the row's
    /// leading edge (`item.width` is the whole row, used for
    /// hit-testing, not the marker). Sample the circle's *center*, not
    /// a fixed offset — near the circle's own edge the anti-aliased
    /// blend with the background diverges from the exact theme color.
    fn marker_center(marker: QRect) -> (u32, u32) {
        let marker_size = marker.height.min(marker.width);
        (
            (marker.x + marker_size / 2.0) as u32,
            (marker.y + marker_size / 2.0) as u32,
        )
    }

    #[test]
    fn selected_option_marker_uses_accent_fg() {
        let mut group = mk_group();
        group.selected = Some(0);
        let (surface, layout) = paint_via_backend(&group);
        let theme = Theme::default();
        let (x, y) = marker_center(layout.items[0]);
        let (r, g, b, _) = surface.pixel(x, y);
        assert_eq!(
            (r, g, b),
            (theme.accent_fg.r, theme.accent_fg.g, theme.accent_fg.b)
        );
    }

    /// An unselected marker is only *stroked* (an outline, not a
    /// filled disc — see `native_surface_paint::paint`), so no single
    /// fixed point is reliably on the 1px line; scan the marker's
    /// bounding box for a pixel that is an exact, unblended match.
    fn marker_contains_color(
        surface: &BitmapSurface,
        marker: QRect,
        color: crate::types::Color,
    ) -> bool {
        let marker_size = marker.height.min(marker.width) as u32;
        let (x0, y0) = (marker.x as u32, marker.y as u32);
        (0..marker_size).any(|dy| {
            (0..marker_size).any(|dx| {
                let (r, g, b, _) = surface.pixel(x0 + dx, y0 + dy);
                (r, g, b) == (color.r, color.g, color.b)
            })
        })
    }

    #[test]
    fn disabled_option_marker_uses_muted_fg() {
        let mut group = mk_group();
        group.options[1].disabled = true;
        let (surface, layout) = paint_via_backend(&group);
        let theme = Theme::default();
        assert!(marker_contains_color(
            &surface,
            layout.items[1],
            theme.muted_fg
        ));
        assert!(!marker_contains_color(
            &surface,
            layout.items[1],
            theme.accent_fg
        ));
    }

    /// Regression guard at a non-zero origin — LESSONS.md "Layout
    /// helpers must return coords in the same frame across backends".
    fn hit_test_round_trip_at(origin_x: f64, origin_y: f64) {
        let layout = mac_radio_group_layout(&mk_group(), origin_x, origin_y, 40.0, 16.0, 8.0);
        let first = layout.hit_test(Point::new(origin_x as f32 + 4.0, origin_y as f32 + 2.0));
        assert_eq!(first, RadioGroupHit::Option(0));
        let second = layout.hit_test(Point::new(origin_x as f32 + 4.0, origin_y as f32 + 9.0));
        assert_eq!(second, RadioGroupHit::Option(1));
        let outside = layout.hit_test(Point::new(origin_x as f32 + 400.0, origin_y as f32));
        assert_eq!(outside, RadioGroupHit::Outside);
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
    fn click_selects_option_after_painting() {
        let group = mk_group();
        let (_surface, layout) = paint_via_backend(&group);
        assert_eq!(
            layout.hit_test(Point::new(2.0, 2.0)),
            RadioGroupHit::Option(0)
        );
        assert_eq!(
            group.select(1),
            Some(RadioGroupEvent::SelectionChanged { index: 1 })
        );
    }

    #[test]
    fn keyboard_moves_cursor_when_focused() {
        let mut group = mk_group();
        group.focused = true;
        let event = group.handle_key(
            &crate::event::Key::Named(crate::event::NamedKey::Down),
            &crate::types::Modifiers::default(),
        );
        assert_eq!(event, Some(RadioGroupEvent::SelectionChanged { index: 0 }));
    }
}
