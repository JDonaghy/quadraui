//! GTK rasteriser for [`crate::DropOverlay`].
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
//! compatibility shim over it, mirroring `gtk::scrollbar::
//! RawScrollbarSurface` (#811 slice 1/9).

use gtk4::cairo::Context;

use crate::native_surface::NativeSurface;
use crate::primitives::drop_zone::DropOverlay;
use crate::theme::Theme;

/// Minimal [`NativeSurface`] adapter over a bare Cairo context, used
/// only by the deprecated [`draw_drop_overlay`] shim below — a drop
/// overlay's paint calls exactly one verb (`surface_fill_rect`), so
/// every other method is `unreachable!()`. Mirrors `gtk::scrollbar::
/// RawScrollbarSurface`'s identical pattern (#811).
pub(crate) struct RawDropOverlaySurface<'a> {
    pub(crate) cr: &'a Context,
}

impl NativeSurface for RawDropOverlaySurface<'_> {
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
        // `set_source_rgba`, not `set_source` — the highlight rect is a
        // translucent overlay (see this module's doc), so the fill must
        // honour `color.a`.
        super::set_source_rgba(self.cr, color);
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.fill().ok();
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
/// direct `quadraui::gtk::draw_drop_overlay` reference rather than going
/// through [`crate::Backend::draw_drop_overlay`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_drop_overlay` instead — this free function is a compatibility shim over the shared #865 implementation"
)]
pub fn draw_drop_overlay(cr: &Context, overlay: &DropOverlay, theme: &Theme) {
    let mut surface = RawDropOverlaySurface { cr };
    crate::primitives::drop_zone::native_surface_paint::paint(overlay, &mut surface, theme);
}
