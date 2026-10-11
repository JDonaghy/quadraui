//! The shared [`PaintSurface`] adapter over a bare Cairo context.
//! Every primitive that uses this adapter is listed below, grouped by
//! two characteristics that vary per-primitive:
//!
//! 1. **`layout` presence.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!    [`crate::Split`] and [`crate::SplitTree`] paint with `cr` alone (no
//!    text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//!    [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//!    [`crate::ToastOverlay`] and [`crate::Panel`] paint text too and need a
//!    live `pango::Layout`. [`CairoSurface::layout`] is `Option` so one
//!    struct covers both — [`Self::layout_or_panic`] panics only if a text
//!    verb is reached with `layout: None`.
//! 2. **Fill translucency.** [`crate::Scrollbar`], `gtk::drop_overlay`,
//!    `gtk::status_bar`, `gtk::diff_view`, `gtk::toast` and `gtk::panel`
//!    fill with `set_source_rgba` (honouring `color.a`); [`crate::Form`],
//!    `gtk::sidebar_panel`, [`crate::Split`] and [`crate::SplitTree`] fill
//!    with `set_source` (opaque-only). [`CairoSurface::translucent_fill`]
//!    carries that choice per call site.
//!
//! Every other verb (`surface_stroke_rect`, `surface_draw_text_run`,
//! `surface_draw_line`, `surface_push_clip`/`surface_pop_clip`,
//! `surface_draw_image`) is implemented unconditionally; primitives that
//! never call a given verb simply never reach that code.
//! `surface_draw_text_run_styled`/`surface_measure_text_styled` (bold-aware
//! Pango `AttrList`) are implemented here unconditionally; only `StatusBar`
//! uses them, making this inert for other primitives.
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly; no primitive that uses this adapter ever paints an image.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::backend::ImagePaintResult;
use crate::paint_surface::PaintSurface;
use crate::{Color, Image, Point, Rect, Viewport};

/// See the module doc for the two fields that carry genuine
/// per-call-site behaviour ([`Self::layout`], [`Self::translucent_fill`]);
/// every other [`PaintSurface`] verb below is one shared implementation.
pub(crate) struct CairoSurface<'a> {
    pub(crate) cr: &'a Context,
    /// `None` for primitives that never paint text through this adapter
    /// (`Scrollbar`, `DropOverlay`, `Split`, `SplitTree`) — see module doc
    /// point 1.
    pub(crate) layout: Option<&'a pango::Layout>,
    /// `true` selects `set_source_rgba` (honours `color.a`); `false`
    /// selects `set_source` (opaque-only) — see module doc point 2.
    pub(crate) translucent_fill: bool,
}

impl<'a> CairoSurface<'a> {
    /// Returns the live layout or panics — only reachable if a text verb
    /// is called on an adapter constructed with `layout: None`, which no
    /// current primitive does (see module doc point 1).
    fn layout_or_panic(&self) -> &'a pango::Layout {
        self.layout
            .expect("CairoSurface: text verb called without a pango::Layout")
    }
}

impl PaintSurface for CairoSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: Viewport) {
        unreachable!("CairoSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("CairoSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> Viewport {
        unreachable!("CairoSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("CairoSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("CairoSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let layout = self.layout_or_panic();
        layout.set_text(text);
        layout.set_attributes(None);
        let (w, h) = layout.pixel_size();
        (w as f32, h as f32)
    }

    fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
        let layout = self.layout_or_panic();
        layout.set_text(text);
        if bold {
            let attrs = pango::AttrList::new();
            attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
            layout.set_attributes(Some(&attrs));
        } else {
            layout.set_attributes(None);
        }
        let (w, h) = layout.pixel_size();
        layout.set_attributes(None);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        if self.translucent_fill {
            super::set_source_rgba(self.cr, color);
        } else {
            super::set_source(self.cr, color);
        }
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.fill().ok();
    }

    /// Same clamp/fill recipe as `GtkBackend`'s own override,
    /// honouring [`Self::translucent_fill`] the same way
    /// [`Self::surface_fill_rect`] above does.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        if self.translucent_fill {
            super::set_source_rgba(self.cr, color);
        } else {
            super::set_source(self.cr, color);
        }
        let r = (radius as f64)
            .min(rect.width as f64 / 2.0)
            .min(rect.height as f64 / 2.0)
            .max(0.0);
        super::rounded_rect_path(
            self.cr,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            r,
        );
        self.cr.fill().ok();
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        super::set_source(self.cr, color);
        self.cr.set_line_width(stroke_width as f64);
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.stroke().ok();
    }

    /// Same clamp/stroke recipe as `GtkBackend`'s own override.
    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    ) {
        super::set_source(self.cr, color);
        self.cr.set_line_width(stroke_width as f64);
        let r = (radius as f64)
            .min(rect.width as f64 / 2.0)
            .min(rect.height as f64 / 2.0)
            .max(0.0);
        super::rounded_rect_path(
            self.cr,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            r,
        );
        self.cr.stroke().ok();
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let layout = self.layout_or_panic();
        layout.set_text(text);
        layout.set_attributes(None);
        super::set_source(self.cr, color);
        self.cr.move_to(rect.x as f64, rect.y as f64);
        super::painted_text::show_layout(self.cr, layout);
    }

    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let layout = self.layout_or_panic();
        layout.set_text(text);
        let attrs = pango::AttrList::new();
        if bold {
            attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
        }
        if italic {
            attrs.insert(pango::AttrInt::new_style(pango::Style::Italic));
        }
        if underline {
            attrs.insert(pango::AttrInt::new_underline(pango::Underline::Single));
        }
        layout.set_attributes(Some(&attrs));
        super::set_source(self.cr, color);
        if (scale_x - 1.0).abs() > f32::EPSILON {
            self.cr.save().ok();
            self.cr.translate(rect.x as f64, rect.y as f64);
            self.cr.scale(scale_x as f64, 1.0);
            self.cr.move_to(0.0, 0.0);
            super::painted_text::show_layout(self.cr, layout);
            self.cr.restore().ok();
        } else {
            self.cr.move_to(rect.x as f64, rect.y as f64);
            super::painted_text::show_layout(self.cr, layout);
        }
        layout.set_attributes(None);
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
        super::set_source(self.cr, color);
        self.cr.set_line_width(stroke_width as f64);
        self.cr.move_to(from.x as f64, from.y as f64);
        self.cr.line_to(to.x as f64, to.y as f64);
        self.cr.stroke().ok();
    }

    fn surface_push_clip(&mut self, rect: Rect) {
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

    fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
        ImagePaintResult::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    fn probe_pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    /// `CairoSurface::surface_fill_rounded_rect` must reach the
    /// same real rounded-corner Cairo path `GtkBackend`'s own override
    /// does (`gtk_backend_paint_surface_fill_rounded_rect_clips_the_corners`'s
    /// twin, at the adapter level) — proven by a corner staying
    /// untouched while the box's centre paints solid.
    #[test]
    fn cairo_surface_fill_rounded_rect_clips_the_corners() {
        let mut surface =
            ImageSurface::create(Format::ARgb32, 40, 40).expect("create ImageSurface");
        let red = Color::rgb(200, 20, 20);

        {
            let cr = Context::new(&surface).expect("Context::new");
            let mut adapter = CairoSurface {
                cr: &cr,
                layout: None,
                translucent_fill: true,
            };
            adapter.surface_fill_rounded_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 15.0, red);
        }

        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        assert_eq!(
            probe_pixel(&data, stride, 20, 20),
            (red.r, red.g, red.b),
            "surface_fill_rounded_rect must paint the solid color at the box's centre"
        );
        assert_eq!(
            probe_pixel(&data, stride, 1, 1),
            (0, 0, 0),
            "a corner pixel well inside a radius-15 fillet on a 40x40 box must stay \
             untouched"
        );
    }

    /// `CairoSurface::surface_stroke_rounded_rect` must reach the
    /// same real rounded-corner Cairo path `GtkBackend`'s own override
    /// does — the adapter-level twin of
    /// `gtk_backend_paint_surface_stroke_rounded_rect_clips_the_corners`.
    #[test]
    fn cairo_surface_stroke_rounded_rect_clips_the_corners() {
        let mut surface =
            ImageSurface::create(Format::ARgb32, 40, 40).expect("create ImageSurface");
        let blue = Color::rgb(60, 120, 220);

        {
            let cr = Context::new(&surface).expect("Context::new");
            let mut adapter = CairoSurface {
                cr: &cr,
                layout: None,
                translucent_fill: true,
            };
            adapter.surface_stroke_rounded_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 15.0, blue, 4.0);
        }

        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        assert_eq!(
            probe_pixel(&data, stride, 20, 1),
            (blue.r, blue.g, blue.b),
            "the straight middle of the top edge must land the stroke colour"
        );
        assert_eq!(
            probe_pixel(&data, stride, 1, 1),
            (0, 0, 0),
            "a corner pixel well inside a radius-15 fillet on a 40x40 box must stay \
             untouched"
        );
        assert_eq!(
            probe_pixel(&data, stride, 20, 20),
            (0, 0, 0),
            "the box's centre, far from any stroke, must stay untouched"
        );
    }
}
