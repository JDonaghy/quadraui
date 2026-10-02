//! macOS rasteriser for [`crate::Split`].
//!
//! Painting moved to the shared
//! [`crate::primitives::split::native_surface_paint::paint`] (#864,
//! `NativeSurface` Phase 2d slice 7/9, child of #811) — see that fn's
//! module doc for a reported divergence between this module and GTK's:
//! macOS's `CGContextSetRGBFillColor` (via `ns_fill_rect`) always
//! honoured a translucent `theme.separator`, while pre-migration GTK
//! did not — this module's behaviour is unchanged by the migration.
//! This module now carries [`mac_split_layout`]; the deprecated
//! `draw_split` compatibility shim over the shared
//! [`super::surface::CgSurface`] adapter was removed in issue #1109
//! (zero uses in coord-tui's `main` and vimcode's `develop`).

use crate::event::Rect as QRect;
use crate::primitives::layout_metrics::pixel_split_layout;
use crate::primitives::split::{Split, SplitLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute the macOS pixel-unit layout for a [`Split`] without painting.
/// Shares its divider thickness with `gtk_split_layout` /
/// `win_split_layout` via [`pixel_split_layout`] (issue #1079).
pub fn mac_split_layout(split: &Split, x: f64, y: f64, w: f64, h: f64) -> SplitLayout {
    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    pixel_split_layout(split, bounds)
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::split::{SplitDirection, SplitHit};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn sample_split(direction: SplitDirection) -> Split {
        Split {
            id: WidgetId::new("split"),
            direction,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    fn paint_via_backend(split: &Split) -> (BitmapSurface, SplitLayout) {
        let surface = BitmapSurface::new(W, H);
        // Fill with a known background so the divider's separator
        // colour reads against a known starting state.
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_split(QRect::new(0.0, 0.0, W as f32, H as f32), split);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn horizontal_divider_paints_separator() {
        let split = sample_split(SplitDirection::Horizontal);
        let (surface, layout) = paint_via_backend(&split);
        let theme = Theme::default();
        // Probe inside the divider rect.
        let d = layout.divider_bounds;
        let px = (d.x + d.width / 2.0) as u32;
        let py = (d.y + d.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
        );
    }

    #[test]
    fn vertical_divider_paints_separator() {
        let split = sample_split(SplitDirection::Vertical);
        let (surface, layout) = paint_via_backend(&split);
        let theme = Theme::default();
        let d = layout.divider_bounds;
        let px = (d.x + d.width / 2.0) as u32;
        let py = (d.y + d.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
        );
    }

    #[test]
    fn pane_areas_left_unpainted_outside_divider() {
        // Background fill stays white outside the divider band — split
        // paints chrome only.
        let split = sample_split(SplitDirection::Horizontal);
        let (surface, _layout) = paint_via_backend(&split);
        // Probe near the right edge of the first pane (well left of
        // the divider at W/2).
        let (r, g, b, _) = surface.pixel(10, H / 2);
        assert_eq!((r, g, b), (255, 255, 255), "left pane should be unpainted");
    }

    /// Shared body for the divider/first-pane/second-pane hit_test
    /// round trip, run at both the origin and a non-zero origin
    /// (quadraui#494 / LESSONS.md "Layout helpers must return coords
    /// in the same frame across backends"). `mac_split_layout` bakes
    /// `bounds.x`/`bounds.y` into `first_bounds`/`divider_bounds`/
    /// `second_bounds` (absolute frame, matching the GTK/TUI twins) —
    /// call it directly (pure fn, no paint needed) and prove clicks at
    /// the resulting absolute positions still resolve through
    /// `hit_test` for all three regions.
    fn hit_test_resolves_divider_and_panes_at(origin_x: f64, origin_y: f64) {
        let split = sample_split(SplitDirection::Horizontal);
        let layout = mac_split_layout(&split, origin_x, origin_y, W as f64, H as f64);

        let d = layout.divider_bounds;
        let hit = layout.hit_test(d.x + d.width / 2.0, d.y + d.height / 2.0);
        assert!(
            matches!(hit, SplitHit::Divider(_)),
            "divider hit was {:?}",
            hit
        );

        // First pane: well left of the divider, relative to origin.
        let hit = layout.hit_test(origin_x as f32 + 10.0, origin_y as f32 + 10.0);
        assert!(
            matches!(hit, SplitHit::FirstPane(_)),
            "first pane hit was {:?}",
            hit
        );

        // Second pane: well right of the divider, relative to origin.
        let hit = layout.hit_test(origin_x as f32 + W as f32 - 10.0, origin_y as f32 + 10.0);
        assert!(
            matches!(hit, SplitHit::SecondPane(_)),
            "second pane hit was {:?}",
            hit
        );
    }

    #[test]
    fn hit_test_resolves_divider_and_panes() {
        hit_test_resolves_divider_and_panes_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (quadraui#494).
    #[test]
    fn hit_test_resolves_divider_and_panes_at_nonzero_origin() {
        hit_test_resolves_divider_and_panes_at(7.0, 13.0);
    }

    #[test]
    fn min_size_clamps_ratio() {
        // Force ratio = 0.1 but require first_min = W*0.4; resolved
        // ratio should bump up so first pane >= first_min.
        let split = Split {
            id: WidgetId::new("split"),
            direction: SplitDirection::Horizontal,
            ratio: 0.1,
            first_min: (W as f32) * 0.4,
            second_min: 0.0,
        };
        let (_surface, layout) = paint_via_backend(&split);
        assert!(
            layout.resolved_ratio > 0.3,
            "resolved_ratio {} should be clamped above 0.3 to honour first_min",
            layout.resolved_ratio,
        );
    }
}
