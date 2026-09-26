//! The one shared [`NativeSurface`] adapter over a bare `CGContextRef`,
//! consolidating the 10 private per-file `Raw*Surface` copies that #811's
//! Phase 2d slices each left behind (issue #1072). Every one of those
//! copies differed from its neighbours only in struct name and
//! `unreachable!` panic strings — unlike the GTK side of the same issue,
//! macOS has **no fill-translucency divergence to preserve**: all 10 old
//! adapters filled through `super::backend::ns_fill_rect`, which already
//! honours `color.a` with a real alpha blend (the quadraui#791 fix,
//! predating this issue — several of the old adapters' doc comments call
//! this out explicitly as *not* sharing GTK's opaque-fill bug). So this
//! module needs only one axis of variation:
//!
//! - **`font` presence.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!   [`crate::Split`] and [`crate::SplitTree`] paint with `ctx` alone (no
//!   text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//!   [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//!   [`crate::ToastStack`] and [`crate::Panel`] paint text too and need a
//!   live `&CTFont`. [`CgSurface::font`] is `Option` so one struct covers
//!   both — [`Self::font_or_panic`] panics only if a text verb is reached
//!   with `font: None`, exactly mirroring what the old per-file
//!   `unreachable!("... has no text measurement")` arms already
//!   guaranteed (those primitives' `paint` fns never call a text verb).
//!
//! Every other verb (`surface_stroke_rect`, `surface_draw_text_run`,
//! `surface_draw_line`, `surface_push_clip`/`surface_pop_clip`,
//! `surface_draw_image`) was already byte-identical across every adapter
//! that implemented it for real (the rest `unreachable!()`d because their
//! primitive's `paint` never calls it) — this module implements all of
//! them unconditionally; the primitives that never call a given verb
//! simply never reach that code, so this is not a behaviour change.
//! `surface_measure_text_styled`/`surface_draw_text_run_styled` (bold-aware)
//! take the trait's default everywhere on macOS — matching every one of
//! the 10 old adapters, none of which overrode either (see
//! `macos::status_bar`'s old adapter doc, "Bold segments": macOS ignores
//! a segment's own `bold` weight, unlike GTK/Win).
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly, matching `MacBackend::draw_image`'s own contract (no
//! `NSImage` decoder wired up yet, #802) and `macos::form`'s old adapter
//! (the only one of the 10 that didn't `unreachable!()` here); no
//! primitive that uses this adapter ever paints an image, so this is a
//! strictly safer default than the panic the other nine copies carried,
//! not a behaviour change for any live call site.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::backend::ImagePaintResult;
use crate::native_surface::NativeSurface;
use crate::{Color, Image, Point, Rect, Viewport};

use super::text::{draw_text, measure_text};

/// See the module doc for [`Self::font`], the one field that carries
/// genuine per-call-site behaviour; every other [`NativeSurface`] verb
/// below is one shared implementation.
pub(crate) struct CgSurface<'a> {
    pub(crate) ctx: CGContextRef,
    /// `None` for primitives that never paint text through this adapter
    /// (`Scrollbar`, `DropOverlay`, `Split`, `SplitTree`) — see module
    /// doc.
    pub(crate) font: Option<&'a CTFont>,
}

impl<'a> CgSurface<'a> {
    /// Returns the live font or panics — only reachable if a text verb
    /// is called on an adapter constructed with `font: None`, which no
    /// current primitive does (see module doc).
    fn font_or_panic(&self) -> &'a CTFont {
        self.font
            .expect("CgSurface: text verb called without a CTFont")
    }
}

impl NativeSurface for CgSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: Viewport) {
        unreachable!("CgSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("CgSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> Viewport {
        unreachable!("CgSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("CgSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("CgSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let (w, h) = measure_text(self.font_or_panic(), text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's paint
        // pass — see this struct's construction sites. `ns_fill_rect`
        // already honours `color.a` with a real alpha blend (see module
        // doc — there is no opaque-only variant to pick between here,
        // unlike `gtk::surface::CairoSurface`).
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
    }

    /// #1073: `ns_fill_rounded_rect`'s twin of [`Self::surface_fill_rect`]
    /// above — same SAFETY contract.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_fill_rounded_rect(self.ctx, rect, radius, color) };
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_stroke_rect(self.ctx, rect, color, stroke_width as f64) };
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let font = self.font_or_panic();
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            draw_text(
                self.ctx,
                font,
                text,
                rect.x as f64,
                rect.y as f64,
                super::backend::ns_color_to_cg(color),
            );
        }
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            super::backend::ns_draw_line(
                self.ctx,
                from.x as f64,
                from.y as f64,
                to.x as f64,
                to.y as f64,
                color,
                stroke_width as f64,
            );
        }
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_push_clip(self.ctx, rect) };
    }

    fn surface_pop_clip(&mut self) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_pop_clip(self.ctx) };
    }

    fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
        // Matches `MacBackend::draw_image`: no `NSImage` decoder wired up
        // yet (#802).
        ImagePaintResult::Unsupported
    }
}
