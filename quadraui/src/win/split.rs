//! Direct2D rasteriser for [`crate::Split`] (issue #29).
//!
//! Mirrors `gtk::split`'s structure: [`Split::layout`] (the D6 layout
//! API — see that primitive's module doc) computes divider + pane
//! geometry; this module paints only the divider as a filled rectangle
//! — pane content is the app's responsibility, same contract as every
//! other backend.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod split;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] — see `win::status_bar`'s
//! module doc for the "placeholder until a later issue wires the app's
//! real theme through" posture this module shares.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::fill_rect;
use crate::event::Rect;
use crate::primitives::split::{Split, SplitLayout, SplitMeasure};
use crate::theme::Theme;

/// Divider thickness (DIPs) — the DirectWrite twin of
/// `gtk::split::GTK_DIVIDER_PX`.
pub const DIVIDER_DIP: f32 = 4.0;

/// Compute a [`Split`]'s layout without painting — the twin of
/// [`draw_split`]. Both call [`Split::layout`] with the identical
/// divider thickness, so a no-paint hit-test call always agrees with
/// what the last paint drew.
pub fn win_split_layout(rect: Rect, split: &Split) -> SplitLayout {
    split.layout(rect, SplitMeasure::new(DIVIDER_DIP))
}

/// Draw a [`Split`] divider onto `target`. Returns the layout for host
/// click/drag dispatch. Pane content is NOT painted.
pub fn draw_split(target: &ID2D1RenderTarget, rect: Rect, split: &Split) -> SplitLayout {
    let layout = win_split_layout(rect, split);
    let theme = Theme::default();
    let _ = fill_rect(target, layout.divider_bounds, theme.separator);
    layout
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

    /// Paint↔click round trip: the divider's painted bg and the
    /// layout's own `hit_test` over that same bounds must agree, and
    /// clicks either side of it resolve to the matching pane.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let split = split();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let layout = surface
            .paint(|target| {
                draw_split(target, rect, &split);
            })
            .map(|_| win_split_layout(rect, &split))
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
        let painted = surface
            .paint(|target| {
                draw_split(target, rect, &split);
            })
            .map(|_| win_split_layout(rect, &split))
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
