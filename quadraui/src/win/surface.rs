//! The one shared [`NativeSurface`] adapter over a bare
//! `&ID2D1RenderTarget`, consolidating the 10 private per-file `Raw*Surface`
//! copies that #811's Phase 2d slices each left behind (issue #1072).
//! Every one of those copies differed from its neighbours only in struct
//! name and `unreachable!` panic strings — like the macOS side of the
//! same issue, Windows has **no fill-translucency divergence to
//! preserve**: all 10 old adapters filled through
//! [`super::text::fill_rect`], which already honours `color.a` with a
//! real translucent `ID2D1SolidColorBrush` (the quadraui#791 fix,
//! predating this issue — several of the old adapters' doc comments call
//! this out explicitly). So this module needs only one axis of variation:
//!
//! - **`dwrite` presence.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!   [`crate::Split`] and [`crate::SplitTree`] paint with `target` alone
//!   (no text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//!   [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//!   [`crate::ToastStack`] and [`crate::Panel`] paint text too and need a
//!   live [`DWrite`]. [`D2dSurface::dwrite`] is `Option` so one struct
//!   covers both — [`Self::dwrite_or_panic`] panics only if a text verb
//!   is reached with `dwrite: None`, exactly mirroring what the old
//!   per-file `unreachable!("... has no text measurement")` arms already
//!   guaranteed (those primitives' `paint` fns never call a text verb).
//!
//! Every other verb (`surface_stroke_rect`, `surface_draw_text_run`,
//! `surface_draw_line`, `surface_push_clip`/`surface_pop_clip`,
//! `surface_draw_image`) was already byte-identical across every adapter
//! that implemented it for real (the rest `unreachable!()`d because their
//! primitive's `paint` never calls it) — this module implements all of
//! them unconditionally; the primitives that never call a given verb
//! simply never reach that code, so this is not a behaviour change.
//!
//! `surface_measure_text_styled`/`surface_draw_text_run_styled` (bold +
//! `scale_x`-aware) were previously overridden only by `win::status_bar`
//! (bold only, no `scale_x` — status bar segments never scale) and
//! `win::form` (bold *and* `scale_x`, needed by
//! `win::multi_section_view`'s embedded-`Terminal` section body); every
//! other adapter took the trait default (drop `bold`/`scale_x`, forward
//! to the plain verb). `win::form`'s version strictly generalises
//! `win::status_bar`'s: at `scale_x == 1.0` (every status-bar call) its
//! `with_horizontal_scale` branch is skipped and it falls through to the
//! exact same `dwrite.draw_text_styled(...)` call `win::status_bar`'s own
//! override made — so implementing that one version unconditionally here
//! reproduces both call sites' behaviour exactly, and is inert (never
//! reached) for the primitives that used the default.
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly (matching `win::form`'s and `win::toast`'s old adapters,
//! the only two of the 10 that didn't `unreachable!()` here); no
//! primitive that uses this adapter ever paints an image, so this is a
//! strictly safer default than the panic the other eight copies carried,
//! not a behaviour change for any live call site.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::backend::ImagePaintResult;
use crate::native_surface::NativeSurface;
use crate::{Color, Image, Point, Rect, Viewport};

use super::text::{
    draw_line, fill_rect, fill_rounded_rect, pop_clip, push_clip, stroke_rect,
    with_horizontal_scale, DWrite,
};

/// See the module doc for [`Self::dwrite`], the one field that carries
/// genuine per-call-site behaviour; every other [`NativeSurface`] verb
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

impl NativeSurface for D2dSurface<'_> {
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

    /// #1073: `super::text::fill_rounded_rect`'s `ID2D1RenderTarget`
    /// twin of [`Self::surface_fill_rect`] above.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        let _ = fill_rounded_rect(self.target, rect, radius, color);
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        let _ = stroke_rect(self.target, rect, color, stroke_width);
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
