//! The one shared [`PaintSurface`] adapter over a bare
//! `&ID2D1RenderTarget`.
//!
//! This adapter varies on one axis: **`dwrite` presence.**
//! [`crate::Scrollbar`], [`crate::DropOverlay`],
//! [`crate::Split`] and [`crate::SplitTree`] paint with `target` alone
//! (no text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//! [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//! [`crate::ToastOverlay`] and [`crate::Panel`] paint text too and need a
//! live [`DWrite`]. [`D2dSurface::dwrite`] is `Option` so one struct
//! covers both — [`Self::dwrite_or_panic`] panics only if a text verb
//! is reached with `dwrite: None`, exactly mirroring what the primitives'
//! `paint` fns guarantee: they never call a text verb without text measurement.
//!
//! All other verbs (`surface_stroke_rect`, `surface_draw_text_run`,
//! `surface_draw_line`, `surface_push_clip`/`surface_pop_clip`,
//! `surface_draw_image`) are implemented unconditionally; primitives that
//! never call a given verb simply never reach that code.
//!
//! `surface_measure_text_styled`/`surface_draw_text_run_styled` handle bold
//! and `scale_x` scaling (needed by `win::multi_section_view`'s embedded
//! Terminal section body); at `scale_x == 1.0` the scaling branch is skipped.
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly; no primitive that uses this adapter ever paints an image.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::backend::ImagePaintResult;
use crate::paint_surface::PaintSurface;
use crate::{Color, Image, Point, Rect, Viewport};

use super::text::{
    draw_line, fill_rect, fill_rounded_rect, pop_clip, push_clip, stroke_rect, stroke_rounded_rect,
    with_horizontal_scale, DWrite,
};

/// See the module doc for [`Self::dwrite`], the one field that carries
/// genuine per-call-site behaviour; every other [`PaintSurface`] verb
/// below is one shared implementation.
pub(crate) struct D2dSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
    /// `None` for primitives that never paint text through this adapter
    /// (`Scrollbar`, `DropOverlay`, `Split`, `SplitTree`) — see module
    /// doc.
    pub(crate) dwrite: Option<&'a DWrite>,
}

impl<'a> D2dSurface<'a> {
    /// Returns the live `DWrite` handle or panics — only reachable if a
    /// text verb is called on an adapter constructed with
    /// `dwrite: None`, which no current primitive does (see module doc).
    fn dwrite_or_panic(&self) -> &'a DWrite {
        self.dwrite
            .expect("D2dSurface: text verb called without a DWrite instance")
    }
}

impl PaintSurface for D2dSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: Viewport) {
        unreachable!("D2dSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("D2dSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> Viewport {
        unreachable!("D2dSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("D2dSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("D2dSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.dwrite_or_panic()
            .measure_text(text)
            .unwrap_or((0.0, 0.0))
    }

    fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
        self.dwrite_or_panic()
            .measure_text_styled(text, bold)
            .unwrap_or((0.0, 0.0))
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        let _ = fill_rect(self.target, rect, color);
    }

    /// `super::text::fill_rounded_rect`'s `ID2D1RenderTarget`
    /// twin of [`Self::surface_fill_rect`] above.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        let _ = fill_rounded_rect(self.target, rect, radius, color);
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        let _ = stroke_rect(self.target, rect, color, stroke_width);
    }

    /// `super::text::stroke_rounded_rect`'s `ID2D1RenderTarget`
    /// twin of [`Self::surface_stroke_rect`] above.
    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    ) {
        let _ = stroke_rounded_rect(self.target, rect, radius, color, stroke_width);
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let _ = self
            .dwrite_or_panic()
            .draw_text(self.target, text, rect, color);
    }

    /// See module doc: this is `win::form`'s pre-existing bold +
    /// `scale_x` implementation, generalised to every call site. At
    /// `scale_x == 1.0` it reproduces `win::status_bar`'s narrower
    /// bold-only override exactly (the `with_horizontal_scale` branch is
    /// skipped, falling through to the same `draw_text_styled` call).
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
        let _ = (italic, underline);
        let dwrite = self.dwrite_or_panic();
        if (scale_x - 1.0).abs() > f32::EPSILON {
            with_horizontal_scale(self.target, scale_x, rect.x, || {
                let _ = dwrite.draw_text_styled(self.target, text, rect, color, bold);
            });
        } else {
            let _ = dwrite.draw_text_styled(self.target, text, rect, color, bold);
        }
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
        let _ = draw_line(self.target, from.x, from.y, to.x, to.y, color, stroke_width);
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        push_clip(self.target, rect);
    }

    fn surface_pop_clip(&mut self) {
        pop_clip(self.target);
    }

    fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
        ImagePaintResult::Unsupported
    }
}
