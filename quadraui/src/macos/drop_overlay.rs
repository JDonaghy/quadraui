//! macOS (Core Graphics) rasteriser for [`crate::DropOverlay`].
//!
//! Painting moved to the shared
//! [`crate::primitives::drop_zone::native_surface_paint::paint`] (#865,
//! `NativeSurface` Phase 2d slice 8/9) — see that fn's doc for the one
//! named divergence (Windows's pre-migration CPU-premixed highlight,
//! unlike GTK/macOS's real alpha blend) found and reported while
//! unifying `gtk::draw_drop_overlay`, `macos::drop_overlay::
//! draw_drop_overlay` and `win::drop_overlay::draw_drop_overlay` into
//! one implementation. This module now only carries
//! [`RawDropOverlaySurface`] and the deprecated [`draw_drop_overlay`]
//! compatibility shim over it, mirroring `macos::scrollbar::
//! RawScrollbarSurface` (#811 slice 1/9), plus the driver-tier tests
//! below (unchanged — they already painted through
//! [`crate::Backend::draw_drop_overlay`], so they exercise the new
//! shared path without modification).
//!
//! `DropOverlay::ghost_position` is not rendered — neither GTK nor TUI
//! paints a ghost label either, so this is parity, not a macOS gap.

use core_graphics::sys::CGContextRef;

use crate::native_surface::NativeSurface;
use crate::primitives::drop_zone::DropOverlay;
use crate::theme::Theme;

/// Minimal [`NativeSurface`] adapter over a bare `CGContextRef`, used
/// only by the deprecated [`draw_drop_overlay`] shim below — a drop
/// overlay's paint calls exactly one verb (`surface_fill_rect`), so
/// every other method is `unreachable!()`. Mirrors `macos::scrollbar::
/// RawScrollbarSurface`'s identical pattern (#811).
pub(crate) struct RawDropOverlaySurface {
    pub(crate) ctx: CGContextRef,
}

impl NativeSurface for RawDropOverlaySurface {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawDropOverlaySurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawDropOverlaySurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawDropOverlaySurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawDropOverlaySurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawDropOverlaySurface has no backend char width")
    }

    fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
        unreachable!("RawDropOverlaySurface has no text measurement")
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's paint
        // pass — see this struct's construction site. `ns_fill_rect`
        // already honours `color.a` with a real alpha blend.
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
    }

    fn surface_stroke_rect(
        &mut self,
        _rect: crate::Rect,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("DropOverlay::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, _rect: crate::Rect, _text: &str, _color: crate::Color) {
        unreachable!("DropOverlay::paint never draws text")
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("DropOverlay::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, _rect: crate::Rect) {
        unreachable!("DropOverlay::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("DropOverlay::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("DropOverlay::paint never draws an image")
    }
}

/// Deprecated free-function shim (#865, CLAUDE.md rule 8): reproduces
/// the pre-#865 signature exactly for any external caller that held a
/// direct `quadraui::macos::draw_drop_overlay` reference rather than
/// going through [`crate::Backend::draw_drop_overlay`] — the sanctioned
/// entry point, and the one every in-tree call site already uses, which
/// is why this shim has no in-repo caller left to trip the
/// `-D warnings`-denied `deprecated` lint.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call (typical: the frame-scope pointer stashed on [`super::MacBackend`]).
/// Calling with a freed or null pointer is UB.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_drop_overlay` instead — this free function is a compatibility shim over the shared #865 implementation"
)]
pub unsafe fn draw_drop_overlay(ctx: CGContextRef, overlay: &DropOverlay, theme: &Theme) {
    let mut surface = RawDropOverlaySurface { ctx };
    crate::primitives::drop_zone::native_surface_paint::paint(overlay, &mut surface, theme);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn paint_via_backend(overlay: &DropOverlay) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        // Opaque black base so the 15% tint composites to a predictable,
        // non-white value.
        surface.fill(0.0, 0.0, 0.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_drop_overlay(overlay);
        });
        backend.end_frame();
        surface
    }

    /// The impl this replaced had an empty body — the whole frame stayed
    /// exactly as the caller left it (quadraui#484 §4).
    #[test]
    fn highlight_tints_the_target_rect() {
        let overlay = DropOverlay {
            highlight: Some(QRect::new(20.0, 20.0, 60.0, 40.0)),
            insertion_bar: None,
            ghost_position: None,
        };
        let surface = paint_via_backend(&overlay);
        let theme = Theme::default();

        let (r, g, b, _) = surface.pixel(50, 40);
        assert_ne!(
            (r, g, b),
            (0, 0, 0),
            "the highlight must actually tint the surface, not no-op",
        );
        // 15% of accent over black — each channel lands near 0.15 * accent.
        let expect = |c: u8| (c as f64 * DropOverlay::HIGHLIGHT_ALPHA as f64).round() as i32;
        for (got, want, name) in [
            (r as i32, expect(theme.accent_fg.r), "r"),
            (g as i32, expect(theme.accent_fg.g), "g"),
            (b as i32, expect(theme.accent_fg.b), "b"),
        ] {
            assert!(
                (got - want).abs() <= 2,
                "{name} channel {got} should be within 2 of the composited {want}",
            );
        }

        // Outside the highlight rect: untouched.
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(5, 5);
                (r, g, b)
            },
            (0, 0, 0),
            "nothing should paint outside the highlight rect",
        );
    }

    #[test]
    fn insertion_bar_paints_solid_accent() {
        let overlay = DropOverlay {
            highlight: None,
            insertion_bar: Some(QRect::new(100.0, 10.0, 2.0, 80.0)),
            ghost_position: None,
        };
        let surface = paint_via_backend(&overlay);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(100, 50);
        assert_eq!(
            (r, g, b),
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
            insertion_bar: Some(QRect::new(140.0, 33.0, 0.0, 40.0)),
            ghost_position: None,
        };
        let surface = paint_via_backend(&overlay);
        let theme = Theme::default();
        let accent = (theme.accent_fg.r, theme.accent_fg.g, theme.accent_fg.b);

        // Zero-width bars are widened to DropOverlay::MIN_BAR_THICKNESS, matching GTK.
        let (r, g, b, _) = surface.pixel(140, 50);
        assert_eq!((r, g, b), accent, "bar should paint at its own x");
        let (r, g, b, _) = surface.pixel(141, 50);
        assert_eq!((r, g, b), accent, "zero-width bar widens to 2 points");

        // Above the bar's y and left of its x: untouched.
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(140, 20);
                (r, g, b)
            },
            (0, 0, 0),
            "nothing should paint above the bar",
        );
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(130, 50);
                (r, g, b)
            },
            (0, 0, 0),
            "nothing should paint left of the bar",
        );
    }

    #[test]
    fn empty_overlay_paints_nothing() {
        let overlay = DropOverlay {
            highlight: None,
            insertion_bar: None,
            ghost_position: None,
        };
        let surface = paint_via_backend(&overlay);
        assert!(
            surface
                .bytes()
                .chunks_exact(4)
                .all(|p| (p[0], p[1], p[2]) == (0, 0, 0)),
            "an overlay with no geometry must leave the frame untouched",
        );
    }
}
