//! Direct2D rasteriser for [`crate::Split`] (issue #29).
//!
//! Painting moved to the shared
//! [`crate::primitives::split::native_surface_paint::paint`] (#864,
//! `NativeSurface` Phase 2d slice 7/9, child of #811) — see that fn's
//! module doc for a reported divergence between this module and GTK's:
//! Windows's `ID2D1SolidColorBrush` (via `super::text::fill_rect`)
//! always honoured a translucent `theme.separator`, while pre-migration
//! GTK did not — this module's behaviour is unchanged by the migration.
//! This module now carries [`win_split_layout`]; the deprecated
//! `draw_split` compatibility shim over the shared
//! [`super::surface::D2dSurface`] adapter was removed in issue #1109
//! (zero uses in coord-tui's `main` and vimcode's `develop`).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod split;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend::draw_split` passes `&self.current_theme` to the shared
//! [`crate::primitives::split::native_surface_paint::paint`] (issue
//! #1261) — the same live theme `Backend::set_theme` writes, the same
//! shape `win::status_bar`'s module doc documents.

#[cfg(test)]
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::event::Rect;
use crate::primitives::layout_metrics::pixel_split_layout;
use crate::primitives::split::{Split, SplitLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute a [`Split`]'s layout without painting — the twin of
/// `crate::Backend::draw_split` (the removed `draw_split` free function
/// shim this backed — see module doc). Both call [`Split::layout`] with
/// the identical divider thickness, so a no-paint hit-test call always
/// agrees with what the last paint drew. Shares that thickness with
/// `gtk_split_layout` / `mac_split_layout` via
/// [`crate::primitives::layout_metrics::pixel::DIVIDER`] (issue #1079).
pub fn win_split_layout(rect: Rect, split: &Split) -> SplitLayout {
    pixel_split_layout(split, rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split::{SplitDirection, SplitHit};
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 100;

    fn split() -> Split {
        Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    /// Paint `layout` via the shared
    /// [`crate::primitives::split::native_surface_paint::paint`] through
    /// a [`super::super::surface::D2dSurface`] over `target` — the same
    /// adapter the now-removed `draw_split` shim used (issue #1109),
    /// exercised here directly.
    fn paint(target: &ID2D1RenderTarget, layout: &SplitLayout) {
        let mut raw = super::super::surface::D2dSurface {
            target,
            dwrite: None,
        };
        crate::primitives::split::native_surface_paint::paint(layout, &mut raw, &Theme::default());
    }

    /// Paint↔click round trip: the divider's painted bg and the
    /// layout's own `hit_test` over that same bounds must agree, and
    /// clicks either side of it resolve to the matching pane.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let split = split();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let layout = win_split_layout(rect, &split);
        surface
            .paint(|target| {
                paint(target, &layout);
            })
            .expect("paint split");

        let theme = Theme::default();
        let div = layout.divider_bounds;
        let div_px = surface.pixel_at(
            (div.x + div.width / 2.0) as u32,
            (div.y + div.height / 2.0) as u32,
        );
        assert_eq!(
            (div_px.r, div_px.g, div_px.b),
            (theme.separator.r, theme.separator.g, theme.separator.b)
        );

        let divider_hit = layout.hit_test(div.x + div.width / 2.0, H as f32 / 2.0);
        assert_eq!(divider_hit, SplitHit::Divider(WidgetId::new("s")));

        let first_hit = layout.hit_test(1.0, H as f32 / 2.0);
        assert_eq!(first_hit, SplitHit::FirstPane(WidgetId::new("s")));

        let second_hit = layout.hit_test(W as f32 - 1.0, H as f32 / 2.0);
        assert_eq!(second_hit, SplitHit::SecondPane(WidgetId::new("s")));
    }

    /// `win_split_layout` (no-paint) must produce byte-identical layout
    /// to what `draw_split` used to paint — same split, same rect.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let split = split();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let painted = win_split_layout(rect, &split);
        surface
            .paint(|target| {
                paint(target, &painted);
            })
            .expect("paint");
        let no_paint = win_split_layout(rect, &split);

        assert_eq!(painted, no_paint);
    }

    #[test]
    fn vertical_direction_stacks_panes() {
        let mut split = split();
        split.direction = SplitDirection::Vertical;
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let layout = win_split_layout(rect, &split);

        assert!(layout.first_bounds.y < layout.divider_bounds.y);
        assert!(layout.divider_bounds.y < layout.second_bounds.y);
    }
}
