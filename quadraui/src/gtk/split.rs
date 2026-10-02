//! GTK rasteriser for [`crate::Split`].
//!
//! Painting moved to the shared
//! [`crate::primitives::split::native_surface_paint::paint`] (#864,
//! `NativeSurface` Phase 2d slice 7/9, child of #811) — see that fn's
//! module doc for a **reported divergence**: the old free-function
//! `draw_split` painted the divider opaque-only (`set_source`), while
//! the live `Backend::draw_split` path now honours `theme.separator`'s
//! alpha via `GtkBackend::surface_fill_rect` (`gtk::set_source_rgba`,
//! inherited from the #811 slice 1 scrollbar fix). This module now
//! carries [`gtk_split_layout`]; the deprecated `draw_split`
//! compatibility shim over the shared [`super::surface::CairoSurface`]
//! adapter was removed in issue #1109 (zero uses in coord-tui's `main`
//! and vimcode's `develop`).

use crate::event::Rect;
use crate::primitives::layout_metrics::pixel_split_layout;
use crate::primitives::split::{Split, SplitLayout};

/// Compute the GTK pixel-unit layout for a [`Split`] without painting.
/// Shares its divider thickness with `mac_split_layout` /
/// `win_split_layout` via [`pixel_split_layout`] (issue #1079).
pub fn gtk_split_layout(split: &Split, x: f64, y: f64, w: f64, h: f64) -> SplitLayout {
    let bounds = Rect::new(x as f32, y as f32, w as f32, h as f32);
    pixel_split_layout(split, bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split::{Split, SplitDirection, SplitHit};
    use crate::types::WidgetId;

    fn round_trip_at(x: f64, y: f64) {
        let split = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let layout = gtk_split_layout(&split, x, y, 100.0, 40.0);

        // `split_layout` is documented **ABSOLUTE** (issue #505):
        // `first_bounds` must start exactly at the origin the split was
        // laid out at, not at (0, 0).
        assert_eq!(layout.first_bounds.x as f64, x);
        assert_eq!(layout.first_bounds.y as f64, y);

        // A click inside the first pane's own (absolute) bounds must
        // resolve back to it without any further coordinate shift.
        let cx = layout.first_bounds.x + layout.first_bounds.width / 2.0;
        let cy = layout.first_bounds.y + layout.first_bounds.height / 2.0;
        assert_eq!(
            layout.hit_test(cx, cy),
            SplitHit::FirstPane(split.id.clone())
        );

        let dx = layout.divider_bounds.x + layout.divider_bounds.width / 2.0;
        let dy = layout.divider_bounds.y + layout.divider_bounds.height / 2.0;
        assert_eq!(layout.hit_test(dx, dy), SplitHit::Divider(split.id));
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends"): `(x, y) = (0, 0)` is exactly the case where a
    /// LOCAL/ABSOLUTE mixup in `gtk_split_layout` would be invisible.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
