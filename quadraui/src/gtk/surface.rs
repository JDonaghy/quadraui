//! The one shared [`NativeSurface`] adapter over a bare Cairo context,
//! consolidating the 10 private per-file `Raw*Surface` copies that #811's
//! Phase 2d slices each left behind (issue #1072). Every one of those
//! copies differed from its neighbours only in struct name, `unreachable!`
//! panic strings, and — in two respects called out below — genuine
//! per-primitive behaviour this module preserves exactly rather than
//! silently picking one:
//!
//! 1. **`layout` presence.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!    [`crate::Split`] and [`crate::SplitTree`] paint with `cr` alone (no
//!    text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//!    [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//!    [`crate::ToastStack`] and [`crate::Panel`] paint text too and need a
//!    live `pango::Layout`. [`CairoSurface::layout`] is `Option` so one
//!    struct covers both — [`Self::layout_or_panic`] panics only if a text
//!    verb is reached with `layout: None`, exactly mirroring what the old
//!    per-file `unreachable!("... has no text measurement")` arms already
//!    guaranteed (those primitives' `paint` fns never call a text verb).
//! 2. **Fill translucency.** `gtk::scrollbar`, `gtk::drop_overlay`,
//!    `gtk::status_bar`, `gtk::diff_view`, `gtk::toast` and `gtk::panel`'s
//!    old adapters filled with `set_source_rgba` (honouring `color.a` —
//!    the quadraui#791 fix); `gtk::form`, `gtk::sidebar_panel`,
//!    `gtk::split` and `gtk::split_tree`'s old adapters filled with
//!    `set_source` (opaque-only — `gtk::split`'s own doc calls this out
//!    as a **deliberate, documented divergence** from the live
//!    `GtkBackend::surface_fill_rect` path, kept so the deprecated
//!    `draw_split`/`draw_split_tree` shims reproduce their pre-migration
//!    behaviour byte-for-byte for any external caller still holding a
//!    direct reference). [`CairoSurface::translucent_fill`] carries that
//!    choice per call site instead of baking it into the type.
//!
//! Every other verb (`surface_stroke_rect`, `surface_draw_text_run`,
//! `surface_draw_line`, `surface_push_clip`/`surface_pop_clip`,
//! `surface_draw_image`) was already byte-identical across every adapter
//! that implemented it for real (the rest `unreachable!()`d because their
//! primitive's `paint` never calls it) — this module implements all of
//! them unconditionally; the primitives that never call a given verb
//! simply never reach that code, so this is not a behaviour change.
//! `surface_draw_text_run_styled`/`surface_measure_text_styled` (bold-aware
//! Pango `AttrList`) were previously overridden only by
//! `gtk::status_bar`'s adapter (every other adapter took the trait
//! default, which drops `bold`); implementing them here unconditionally
//! is likewise inert for every primitive besides `StatusBar` — no other
//! `native_surface_paint::paint` calls either styled verb (see
//! `crate::native_surface::NativeSurface`'s own doc for the cross-backend
//! survey; only `primitives::terminal` and `primitives::status_bar` call
//! them, and `terminal` paints through a live `GtkBackend`, never through
//! this adapter).
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly rather than `unreachable!()` — `gtk::form`'s old adapter
//! already did this (matching `GtkBackend::draw_image`'s own contract for
//! an unsupported decode path); no primitive that uses this adapter ever
//! paints an image, so this is a strictly safer default than the panic
//! the other nine copies carried, not a behaviour change for any live
//! call site.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::backend::ImagePaintResult;
use crate::native_surface::NativeSurface;
use crate::{Color, Image, Point, Rect, Viewport};

/// See the module doc for the two fields that carry genuine
/// per-call-site behaviour ([`Self::layout`], [`Self::translucent_fill`]);
/// every other [`NativeSurface`] verb below is one shared implementation.
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

impl NativeSurface for CairoSurface<'_> {
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
