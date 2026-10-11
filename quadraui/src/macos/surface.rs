//! The one shared [`PaintSurface`] adapter over a bare `CGContextRef`.
//! Every adapter fills through `super::backend::ns_fill_rect`, which honours
//! `color.a` with a real alpha blend. This module varies only in one axis:
//!
//! - **`font` presence.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!   [`crate::Split`] and [`crate::SplitTree`] paint with `ctx` alone (no
//!   text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//!   [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//!   [`crate::ToastOverlay`] and [`crate::Panel`] paint text too and need a
//!   live `&CTFont`. [`CgSurface::font`] is `Option` so one struct covers
//!   both — [`Self::font_or_panic`] panics only if a text verb is reached
//!   with `font: None`, which never happens in practice.
//!
//! Every other verb (`surface_stroke_rect`, `surface_draw_text_run`,
//! `surface_draw_line`, `surface_push_clip`/`surface_pop_clip`,
//! `surface_draw_image`) is implemented unconditionally; primitives that
//! never call a given verb simply never reach that code.
//! `surface_measure_text_styled`/`surface_draw_text_run_styled` (bold-aware)
//! take the trait's default everywhere on macOS — macOS ignores a segment's
//! own `bold` weight, unlike GTK/Win.
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly, matching `MacBackend::draw_image`'s contract (no decoder
//! wired up yet); no primitive using this adapter paints an image.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::backend::ImagePaintResult;
use crate::paint_surface::PaintSurface;
use crate::{Color, Image, Point, Rect, Viewport};

use super::text::{draw_text, measure_text};

/// See the module doc for [`Self::font`], the one field that carries
/// genuine per-call-site behaviour; every other [`PaintSurface`] verb
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

impl PaintSurface for CgSurface<'_> {
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

    /// `ns_fill_rounded_rect`'s twin of [`Self::surface_fill_rect`]
    /// above — same SAFETY contract.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_fill_rounded_rect(self.ctx, rect, radius, color) };
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_stroke_rect(self.ctx, rect, color, stroke_width as f64) };
    }

    /// `ns_stroke_rounded_rect`'s twin of [`Self::surface_stroke_rect`]
    /// above — same SAFETY contract.
    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    ) {
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            super::backend::ns_stroke_rounded_rect(
                self.ctx,
                rect,
                radius,
                color,
                stroke_width as f64,
            )
        };
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
        // yet.
        ImagePaintResult::Unsupported
    }
}
