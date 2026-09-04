//! Direct2D rasteriser for [`crate::primitives::drop_zone::DropOverlay`]
//! (#726).
//!
//! Port of [`crate::gtk::drop_overlay::draw_drop_overlay`] /
//! [`crate::macos::drop_overlay::draw_drop_overlay`]:
//!
//! - Highlight: tinted rect in `theme.accent_fg` at
//!   [`DropOverlay::HIGHLIGHT_ALPHA`].
//! - Insertion bar: solid rect in `theme.accent_fg`, at least
//!   [`DropOverlay::MIN_BAR_THICKNESS`] wide/tall.
//!
//! Unlike the GTK/macOS twins (real alpha-blended fills straight onto
//! whatever is already on screen), this render target is created with
//! `D2D1_ALPHA_MODE_IGNORE`/`UNKNOWN` (see `super::text::blend`'s doc),
//! so the highlight tint is premixed against `theme.background` on the
//! CPU before filling — the same posture `super::scrollbar` uses for its
//! track/thumb. The insertion bar is opaque, so it fills directly with
//! no premix.
//!
//! `DropOverlay::ghost_position` is not rendered — neither GTK, macOS
//! nor TUI paints a ghost label either, so this is parity, not a
//! Win-GUI gap.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod drop_overlay;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{blend, fill_rect};
use crate::event::Rect;
use crate::primitives::drop_zone::DropOverlay;
use crate::theme::Theme;

/// Paint `overlay` onto `target`.
pub fn draw_drop_overlay(target: &ID2D1RenderTarget, overlay: &DropOverlay, theme: &Theme) {
    if let Some(h) = overlay.highlight {
        if h.width > 0.0 && h.height > 0.0 {
            let tint = blend(
                theme.background,
                theme.accent_fg,
                DropOverlay::HIGHLIGHT_ALPHA,
            );
            let _ = fill_rect(target, h, tint);
        }
    }

    if let Some(bar) = overlay.insertion_bar {
        if bar.height > 0.0 {
            let widened = Rect::new(
                bar.x,
                bar.y,
                bar.width.max(DropOverlay::MIN_BAR_THICKNESS),
                bar.height,
            );
            let _ = fill_rect(target, widened, theme.accent_fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 120;

    fn paint(overlay: &DropOverlay) -> HeadlessSurface {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        // Fill with a known background so a blended highlight tint is
        // deterministic to probe.
        surface
            .fill_rect(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                Theme::default().background,
            )
            .expect("fill bg");
        surface
            .paint(|target| {
                draw_drop_overlay(target, overlay, &Theme::default());
            })
            .expect("paint drop overlay");
        surface
    }

    /// The impl this replaced was a genuine `todo!()` — every tab drag
    /// panicked (quadraui#726).
    #[test]
    fn highlight_tints_the_target_rect() {
        let overlay = DropOverlay {
            highlight: Some(Rect::new(20.0, 20.0, 60.0, 40.0)),
            insertion_bar: None,
            ghost_position: None,
        };
        let surface = paint(&overlay);
        let theme = Theme::default();

        let c = surface.pixel_at(50, 40);
        assert_ne!(
            (c.r, c.g, c.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "the highlight must actually tint the surface, not no-op",
        );

        // Outside the highlight rect: untouched.
        let outside = surface.pixel_at(5, 5);
        assert_eq!(
            (outside.r, outside.g, outside.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "nothing should paint outside the highlight rect",
        );
    }

    #[test]
    fn insertion_bar_paints_solid_accent() {
        let overlay = DropOverlay {
            highlight: None,
            insertion_bar: Some(Rect::new(100.0, 10.0, 2.0, 80.0)),
            ghost_position: None,
        };
        let surface = paint(&overlay);
        let theme = Theme::default();
        let c = surface.pixel_at(100, 50);
        assert_eq!(
            (c.r, c.g, c.b),
            (theme.accent_fg.r, theme.accent_fg.g, theme.accent_fg.b),
            "the insertion bar is opaque accent",
        );
    }

    /// Non-zero-origin guard: the overlay carries absolute rects, so a
    /// bar at x=140 must paint at x=140 and nowhere else.
    #[test]
    fn insertion_bar_honours_a_nonzero_origin() {
        let overlay = DropOverlay {
            highlight: None,
            insertion_bar: Some(Rect::new(140.0, 33.0, 0.0, 40.0)),
            ghost_position: None,
        };
        let surface = paint(&overlay);
        let theme = Theme::default();
        let accent = (theme.accent_fg.r, theme.accent_fg.g, theme.accent_fg.b);

        // Zero-width bars are widened to DropOverlay::MIN_BAR_THICKNESS,
        // matching GTK/macOS.
        let left = surface.pixel_at(140, 50);
        assert_eq!(
            (left.r, left.g, left.b),
            accent,
            "bar should paint at its own x"
        );
        let right = surface.pixel_at(141, 50);
        assert_eq!(
            (right.r, right.g, right.b),
            accent,
            "zero-width bar widens to 2 DIPs"
        );

        // Above the bar's y: untouched.
        let above = surface.pixel_at(140, 20);
        assert_eq!(
            (above.r, above.g, above.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "nothing should paint above the bar",
        );
    }

    #[test]
    fn empty_overlay_is_a_no_op() {
        let overlay = DropOverlay {
            highlight: None,
            insertion_bar: None,
            ghost_position: None,
        };
        let surface = paint(&overlay);
        let theme = Theme::default();
        let c = surface.pixel_at(100, 60);
        assert_eq!(
            (c.r, c.g, c.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "an overlay with no geometry must leave the frame untouched",
        );
    }
}
