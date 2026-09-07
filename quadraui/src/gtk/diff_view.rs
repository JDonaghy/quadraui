//! GTK rasteriser for [`crate::primitives::diff_view::DiffView`].
//!
//! Painting moved to the shared
//! [`crate::primitives::diff_view::native_surface_paint::paint`] (#866,
//! `NativeSurface` Phase 2d slice 9/9) — see that fn's doc for the two
//! named divergences (row/header text vertical alignment; header-label
//! ellipsize vs. hard-clip) found while unifying
//! `gtk::diff_view::draw_diff_view`, `macos::diff_view::draw_diff_view`
//! and `win::diff_view::draw_diff_view` into one implementation. This
//! module now only carries [`RawGtkDiffViewSurface`] and the deprecated
//! [`draw_diff_view`] compatibility shim over it, mirroring
//! `gtk::status_bar::RawGtkStatusBarSurface` (#860).

use gtk4::cairo::Context;
use gtk4::pango;

use crate::native_surface::NativeSurface;
use crate::primitives::diff_view::{DiffView, DiffViewLayout};
use crate::theme::Theme;

/// Minimal [`NativeSurface`] adapter over a bare Cairo context + Pango
/// layout, used only by the deprecated [`draw_diff_view`] shim below —
/// mirrors `gtk::status_bar::RawGtkStatusBarSurface`'s identical pattern
/// (#860), extended with clip push/pop, which this primitive's paint
/// actually uses.
struct RawGtkDiffViewSurface<'a> {
    cr: &'a Context,
    pango_layout: &'a pango::Layout,
}

impl NativeSurface for RawGtkDiffViewSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawGtkDiffViewSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawGtkDiffViewSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawGtkDiffViewSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawGtkDiffViewSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawGtkDiffViewSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.pango_layout.set_text(text);
        self.pango_layout.set_attributes(None);
        let (w, h) = self.pango_layout.pixel_size();
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // `set_source_rgba`, not `set_source` — see that fn's doc
        // (issue #811) for why a translucent fill must honour `color.a`
        // here to match macOS/Windows's `NativeSurface::surface_fill_rect`.
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
        unreachable!("DiffView::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        self.pango_layout.set_text(text);
        self.pango_layout.set_attributes(None);
        super::set_source(self.cr, color);
        self.cr.move_to(rect.x as f64, rect.y as f64);
        super::painted_text::show_layout(self.cr, self.pango_layout);
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("DiffView::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, rect: crate::Rect) {
        self.cr.save().ok();
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.clip();
    }

    fn surface_pop_clip(&mut self) {
        self.cr.restore().ok();
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("DiffView::paint never draws an image")
    }
}

/// Deprecated free-function shim (#866, CLAUDE.md rule 8): reproduces
/// the pre-#866 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_diff_view` reference rather than going
/// through [`crate::Backend::draw_diff_view`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_diff_view` instead — this free function is a compatibility shim over the shared #866 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub fn draw_diff_view(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    view: &DiffView,
    theme: &Theme,
    line_height: f64,
) -> DiffViewLayout {
    let mut surface = RawGtkDiffViewSurface {
        cr,
        pango_layout: layout,
    };
    crate::primitives::diff_view::native_surface_paint::paint(
        view,
        &mut surface,
        theme,
        crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32),
        line_height as f32,
    )
}
