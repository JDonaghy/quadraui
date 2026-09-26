//! Direct2D rasteriser for [`crate::primitives::drop_zone::DropOverlay`]
//! (#726).
//!
//! Painting moved to the shared
//! [`crate::primitives::drop_zone::native_surface_paint::paint`] (#865,
//! `NativeSurface` Phase 2d slice 8/9) — see that fn's doc for the one
//! named divergence this migration found and reported (rather than
//! silently resolving): this module's pre-migration `draw_drop_overlay`
//! CPU-premixed the highlight tint against `theme.background` via
//! `super::text::blend` (the render target here is created with
//! `D2D1_ALPHA_MODE_IGNORE`/`UNKNOWN`), which is a genuine behavioural
//! difference from GTK's `cr.set_source_rgba` and macOS's
//! `CGContextSetRGBFillColor`, both of which alpha-blend the tint
//! against whatever is *actually* underneath. Unlike `win::scrollbar`
//! (#791 already fixed its equivalent premix before the #811 slice 1/9
//! migration started), this module's premix was **not** already fixed —
//! `super::text::fill_rect`, the verb `NativeSurface::surface_fill_rect`
//! calls on this backend, has honoured `color.a` with a real
//! `ID2D1SolidColorBrush` alpha blend since #791, so routing this
//! primitive's paint through the shared `surface_fill_rect` verb (same
//! as every other backend) *corrects* Windows's highlight to a real
//! alpha blend instead of reproducing the old premix. The insertion bar
//! was already opaque on every backend, so it fills unchanged.
//!
//! This module now only carries the deprecated [`draw_drop_overlay`]
//! compatibility shim over the shared [`super::surface::D2dSurface`]
//! adapter (#1072 — consolidated from this module's own private
//! `RawDropOverlaySurface`).
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

use crate::primitives::drop_zone::DropOverlay;
use crate::theme::Theme;

/// Deprecated free-function shim (#865, CLAUDE.md rule 8): reproduces
/// the pre-#865 signature exactly for any external caller that held a
/// direct `quadraui::win::draw_drop_overlay` reference rather than going
/// through [`crate::Backend::draw_drop_overlay`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the
/// `-D warnings`-denied `deprecated` lint.
///
/// Note this shim's behaviour differs from the pre-#865 free function it
/// replaces: the highlight now alpha-blends against whatever is already
/// on `target` instead of premixing against `theme.background` — see
/// the module doc.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_drop_overlay` instead — this free function is a compatibility shim over the shared #865 implementation"
)]
pub fn draw_drop_overlay(target: &ID2D1RenderTarget, overlay: &DropOverlay, theme: &Theme) {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: None,
    };
    crate::primitives::drop_zone::native_surface_paint::paint(overlay, &mut surface, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 120;

    /// Paint `overlay` via the shared
    /// [`crate::primitives::drop_zone::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s headless
    /// target — the same adapter the deprecated [`draw_drop_overlay`]
    /// shim uses, exercised here directly so these tests don't trip the
    /// `-D warnings`-denied `deprecated` lint (CLAUDE.md rule 3; mirrors
    /// `win::scrollbar`'s identical test-migration note).
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
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: None,
                };
                crate::primitives::drop_zone::native_surface_paint::paint(
                    overlay,
                    &mut raw,
                    &Theme::default(),
                );
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

    /// Regression pinning this migration's found-and-fixed divergence
    /// (see module doc): the highlight must alpha-blend against the
    /// *real* background beneath it, not premix against
    /// `theme.background` regardless of what's actually painted there.
    #[test]
    fn highlight_blends_with_real_background_not_theme_background() {
        let overlay = DropOverlay {
            highlight: Some(Rect::new(20.0, 20.0, 60.0, 40.0)),
            insertion_bar: None,
            ghost_position: None,
        };
        let theme = Theme::default();
        let real_bg = crate::types::Color::rgb(200, 30, 30);
        assert_ne!(
            (real_bg.r, real_bg.g, real_bg.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "test fixture must differ from theme.background to be a meaningful probe",
        );

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        surface
            .fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), real_bg)
            .expect("fill bg");
        surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: None,
                };
                crate::primitives::drop_zone::native_surface_paint::paint(
                    &overlay, &mut raw, &theme,
                );
            })
            .expect("paint drop overlay");

        let mix = |b: u8, o: u8, alpha: f32| -> u8 {
            (b as f32 * (1.0 - alpha) + o as f32 * alpha).round() as u8
        };
        let alpha = DropOverlay::HIGHLIGHT_ALPHA;
        let expected_r = mix(real_bg.r, theme.accent_fg.r, alpha);
        let expected_g = mix(real_bg.g, theme.accent_fg.g, alpha);
        let expected_b = mix(real_bg.b, theme.accent_fg.b, alpha);
        let old_halo_r = mix(theme.background.r, theme.accent_fg.r, alpha);

        let c = surface.pixel_at(50, 40);
        assert_eq!(
            (c.r, c.g, c.b),
            (expected_r, expected_g, expected_b),
            "highlight should alpha-blend with the real background beneath it",
        );
        assert_ne!(
            c.r, old_halo_r,
            "highlight must not premix against theme.background regardless of what's painted underneath",
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
