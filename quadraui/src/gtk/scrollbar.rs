//! GTK rasteriser for [`crate::Scrollbar`].
//!
//! Painting moved to the shared
//! [`crate::primitives::scrollbar::native_surface_paint::paint`] (#811,
//! `NativeSurface` Phase 2d) — see that fn's doc for the one named
//! divergence (quadraui#791) re-verified (already fixed) while unifying
//! `gtk::draw_scrollbar`, `macos::scrollbar::draw_scrollbar` and
//! `win::scrollbar::draw_scrollbar` into one implementation. This module
//! now only carries [`RawScrollbarSurface`] and the deprecated
//! [`draw_scrollbar`] compatibility shim over it, mirroring
//! `gtk::form::RawFormSurface` (#808).

use gtk4::cairo::Context;

use crate::native_surface::NativeSurface;
use crate::primitives::scrollbar::Scrollbar;
use crate::theme::Theme;

/// Minimal [`NativeSurface`] adapter over a bare Cairo context, used by
/// the deprecated [`draw_scrollbar`] shim below and by any free-function
/// rasteriser (`gtk::data_table`, `gtk::list`) that paints an embedded
/// scrollbar from only a `cr: &Context` — not a live `GtkBackend`. A
/// scrollbar's paint calls exactly one verb (`surface_fill_rect`), so
/// every other method is `unreachable!()`. Mirrors
/// `gtk::form::RawFormSurface`'s identical pattern (#808).
pub(crate) struct RawScrollbarSurface<'a> {
    pub(crate) cr: &'a Context,
}

impl NativeSurface for RawScrollbarSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawScrollbarSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawScrollbarSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawScrollbarSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawScrollbarSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawScrollbarSurface has no backend char width")
    }

    fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
        unreachable!("RawScrollbarSurface has no text measurement")
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // `set_source_rgba`, not `set_source` — a scrollbar's entire
        // visual identity is a translucent overlay (see this module's
        // doc / quadraui#791), so the fill must honour `color.a`.
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
        unreachable!("Scrollbar::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, _rect: crate::Rect, _text: &str, _color: crate::Color) {
        unreachable!("Scrollbar::paint never draws text")
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("Scrollbar::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, _rect: crate::Rect) {
        unreachable!("Scrollbar::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("Scrollbar::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("Scrollbar::paint never draws an image")
    }
}

/// Deprecated free-function shim (#811, CLAUDE.md rule 8): reproduces
/// the pre-#811 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_scrollbar` reference rather than going
/// through [`crate::Backend::draw_scrollbar`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_scrollbar` instead — this free function is a compatibility shim over the shared #811 implementation"
)]
pub fn draw_scrollbar(cr: &Context, scrollbar: &Scrollbar, theme: &Theme) {
    let mut surface = RawScrollbarSurface { cr };
    crate::primitives::scrollbar::native_surface_paint::paint(scrollbar, &mut surface, theme);
}
