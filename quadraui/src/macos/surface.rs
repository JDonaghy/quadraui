//! The one shared [`PaintSurface`] adapter over a bare `CGContextRef`.
//! Every adapter fills through `super::backend::ns_fill_rect`, which honours
//! `color.a` with a real alpha blend. This module varies only in one axis:
//!
//! - **`font` presence.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!   [`crate::Split`] and [`crate::SplitTree`] paint with `ctx` alone (no
//!   text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//!   [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//!   [`crate::ToastOverlay`] paint text too and need a
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
//! [`Self::surface_line_height`] computes `ascent + descent + leading` on
//! [`Self::font`] directly via [`super::text::font_metrics`] — the same
//! derivation [`crate::macos::MacBackend::set_chrome_font`] uses to seed
//! `chrome_line_height`, and `MacBackend` has no override setter for that
//! field (unlike `current_line_height`), so the two can never disagree.
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly, matching `MacBackend::paint_image`'s contract (no decoder
//! wired up yet); no primitive using this adapter paints an image.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::backend::ImagePaintResult;
use crate::paint_surface::{AlphaSheet, PaintSurface};
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

/// A rendered sheet with the `(family, chars, cell size)` it was rendered for.
type CachedSheet = (String, String, (u32, u32), AlphaSheet);

thread_local! {
    /// The last sheet [`glyph_alpha_sheet`] rendered, so a repeated request
    /// shapes nothing.
    static GLYPH_SHEET: std::cell::RefCell<Option<CachedSheet>> =
        const { std::cell::RefCell::new(None) };
}

/// [`PaintSurface::surface_glyph_alpha_sheet`] for Core Text: each char of
/// `chars` drawn bold in `family`, sized to `cell`'s height, white onto a
/// zeroed offscreen bitmap whose red channel is then the coverage.
pub(crate) fn glyph_alpha_sheet(
    family: &str,
    chars: &str,
    cell: crate::primitive::Size,
) -> Option<AlphaSheet> {
    let cell_px = (cell.width.round() as u32, cell.height.round() as u32);
    let hit = GLYPH_SHEET.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(f, ch, sz, _)| f == family && ch == chars && *sz == cell_px)
            .map(|(_, _, _, sheet)| sheet.clone())
    });
    if hit.is_some() {
        return hit;
    }
    let sheet = render_glyph_sheet(family, chars, cell_px)?;
    GLYPH_SHEET.with(|c| {
        *c.borrow_mut() = Some((
            family.to_string(),
            chars.to_string(),
            cell_px,
            sheet.clone(),
        ))
    });
    Some(sheet)
}

fn render_glyph_sheet(
    family: &str,
    chars: &str,
    (cell_w, cell_h): (u32, u32),
) -> Option<AlphaSheet> {
    use core_text::font_descriptor::kCTFontBoldTrait;
    let count = chars.chars().count() as u32;
    let (sheet_w, sheet_h) = (cell_w * count, cell_h);
    if sheet_w == 0 || sheet_h == 0 {
        return None;
    }
    let base_font = super::text::make_font(family, f64::from(cell_h) * 0.85)?;
    // A family with no bold face keeps its regular weight.
    let font = base_font
        .clone_with_symbolic_traits(kCTFontBoldTrait, kCTFontBoldTrait)
        .unwrap_or(base_font);
    let surface = super::headless::BitmapSurface::new(sheet_w, sheet_h);
    // `draw_text` records into the thread's painted-text sink; this sheet
    // is offscreen, so record into a scratch sink and discard it.
    let prev_sink = crate::testing::install_text_run_sink();
    for (i, ch) in chars.chars().enumerate() {
        // SAFETY: `surface.context_ptr()` is a live `CGContextRef` for the
        // surface's lifetime, which outlives this loop.
        unsafe {
            draw_text(
                surface.context_ptr(),
                &font,
                &ch.to_string(),
                f64::from(i as u32 * cell_w),
                0.0,
                (1.0, 1.0, 1.0, 1.0),
            );
        }
    }
    let _ = crate::testing::take_text_run_sink(prev_sink);
    let mut alpha = vec![0u8; (sheet_w * sheet_h) as usize];
    for y in 0..sheet_h {
        for x in 0..sheet_w {
            // White over transparent, premultiplied: red equals coverage.
            alpha[(y * sheet_w + x) as usize] = surface.pixel(x, y).0;
        }
    }
    Some(AlphaSheet::new(sheet_w, sheet_h, alpha))
}

/// [`PaintSurface::surface_blit_alpha`] for Core Graphics: one
/// `color`-at-coverage fill per non-zero pixel of `alpha`, each
/// `dest.width / width` by `dest.height / height` points.
///
/// # Safety
///
/// `ctx` must be a valid, live `CGContextRef` for the duration of this call.
pub(crate) unsafe fn blit_alpha(
    ctx: CGContextRef,
    dest: Rect,
    alpha: &[u8],
    width: u32,
    height: u32,
    color: Color,
) {
    use super::cg::{rect, CGContextFillRect, CGContextSetRGBFillColor};
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || alpha.len() < w * h {
        return;
    }
    let (r, g, b, _) = super::cg::color_to_cg(color);
    let px_w = f64::from(dest.width) / f64::from(width);
    let px_h = f64::from(dest.height) / f64::from(height);
    for row in 0..h {
        for col in 0..w {
            let coverage = alpha[row * w + col];
            if coverage == 0 {
                continue;
            }
            CGContextSetRGBFillColor(ctx, r, g, b, f64::from(coverage) / 255.0);
            CGContextFillRect(
                ctx,
                rect(
                    f64::from(dest.x) + col as f64 * px_w,
                    f64::from(dest.y) + row as f64 * px_h,
                    px_w,
                    px_h,
                ),
            );
        }
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
        super::text::font_metrics(self.font_or_panic()).line_height as f32
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

    fn surface_draw_text_line(
        &mut self,
        origin: Point,
        text: &str,
        runs: &[crate::TextRunStyle],
        _role: crate::FontRole,
    ) {
        let font = self.font_or_panic();
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            super::text::draw_text_line(
                self.ctx,
                font,
                origin.x as f64,
                origin.y as f64,
                text,
                runs,
            )
        };
    }

    fn surface_text_x_at_index(
        &mut self,
        text: &str,
        _role: crate::FontRole,
        byte_idx: usize,
    ) -> f32 {
        super::text::text_x_at_index(self.font_or_panic(), text, byte_idx)
    }

    fn surface_text_index_at_x(&mut self, text: &str, _role: crate::FontRole, x: f32) -> usize {
        super::text::text_index_at_x(self.font_or_panic(), text, x)
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

    fn surface_blit_alpha(
        &mut self,
        dest: Rect,
        alpha: &[u8],
        width: u32,
        height: u32,
        color: Color,
    ) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { blit_alpha(self.ctx, dest, alpha, width, height, color) };
    }

    /// Shapes in the adapter's own font's family; `role` is not
    /// consulted, as for every other text verb here.
    fn surface_glyph_alpha_sheet(
        &mut self,
        chars: &str,
        _role: crate::FontRole,
        cell: crate::primitive::Size,
    ) -> Option<AlphaSheet> {
        glyph_alpha_sheet(&self.font?.family_name(), chars, cell)
    }

    fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
        // Matches `MacBackend::paint_image`: no `NSImage` decoder wired up
        // yet.
        ImagePaintResult::Unsupported
    }
}

pub(crate) mod image {
    //! Core Graphics / ImageIO rasteriser for [`crate::primitives::image::Image`].
    //!
    //! Decodes `image.source`'s encoded bytes through `CGImageSource` (ImageIO),
    //! then paints the decoded bitmap into `image.layout(rect)`'s resolved target
    //! rect. Same collapse-to-`Unsupported` posture on any decode failure (missing
    //! file, corrupt bytes, unrecognised format): `CGImageSourceCreateImageAtIndex`
    //! reports all of these the same way, as a null return.
    //!
    //! Painting is always clipped to the caller's `rect` — the only
    //! [`crate::primitives::image::ImageFit`] variants that can extend past
    //! it are `Cover` and `None`, and neither should bleed into whatever the
    //! host paints next to it. Reuses [`super::super::backend::ns_push_clip`]/
    //! [`super::super::backend::ns_pop_clip`] — the same save/clip/restore pair
    //! every other clipping rasteriser in `macos::` uses — rather than
    //! hand-rolling a third clip helper.
    //!
    //! # Decode cache
    //!
    //! [`paint_image`] is `pub` inside a `pub mod` — reachable externally as
    //! `quadraui::macos::image::paint_image` — so it cannot change its
    //! signature without risking a break for external code already calling it
    //! directly. `paint_image` itself therefore stays exactly as it always has —
    //! uncached, one decode per call — and the caching lives in a sibling,
    //! crate-internal [`draw_image_cached`] that `MacBackend`'s
    //! `PaintSurface::surface_draw_image` calls instead. Both share the same
    //! post-decode clip/counter-flip/draw tail, [`paint_cg_image`].
    //!
    //! Unlike GTK's `gdk_pixbuf::Pixbuf::scale_simple` (which produces an
    //! already-scaled bitmap sized for one particular target rect),
    //! `CGContextDrawImage` stretches a `CGImage` into whatever destination
    //! rect it's given at draw time — so the decoded `CGImage` here is
    //! genuinely reusable across every target size a source is ever painted
    //! at, not just repeats of the same size. [`draw_image_cached`] caches
    //! the raw decode keyed on source only (width/height pinned to `0` in the
    //! [`crate::image_cache::ImageCache`] key — see the call site). See
    //! [`paint_cg_image`]'s doc for the counter-flip both entry points share.
    //!
    //! # Why raw FFI for `CGImageSource`
    //!
    //! Unlike `CGImage`/`CGDataProvider` (both wrapped by the `core-graphics`
    //! crate already in this crate's dependency graph), ImageIO's
    //! `CGImageSourceRef` has no representation there — `core-graphics`
    //! stops at CoreGraphics proper and never links ImageIO. This module
    //! owns the minimal FFI surface itself (an opaque `CGImageSource` type
    //! plus the two creation functions it needs), the same shape
    //! `macos::text` uses for `CTFontManagerRegisterGraphicsFont` (a gap in
    //! `core-text`) and `macos::headless` uses for `CGBitmapContextCreate` (a
    //! gap in `core-graphics`'s safe surface). `CGImage` itself is
    //! constructed via [`core_graphics::image::CGImage`]'s
    //! [`foreign_types::ForeignType::from_ptr`] once ImageIO hands back a
    //! raw `CGImageRef`, so its `Drop` impl (`CGImageRelease`) still owns
    //! release — only the `CGImageSourceRef` needs a manual `CFRelease`.

    use std::sync::Arc;

    use core_foundation::base::{CFRelease, CFTypeRef};
    use core_foundation::dictionary::CFDictionaryRef;
    use core_graphics::base::CGFloat;
    use core_graphics::data_provider::CGDataProvider;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics::image::CGImage;
    use core_graphics::sys::{CGContextRef, CGDataProviderRef, CGImageRef};
    use foreign_types::ForeignType;

    use super::super::backend::{ns_pop_clip, ns_push_clip};
    use crate::backend::ImagePaintResult;
    use crate::event::Rect;
    use crate::image_cache::ImageCache;
    use crate::primitives::image::{Image, ImageSource};

    /// Opaque ImageIO type — see the module doc's "Why raw FFI" section.
    pub(super) enum CGImageSource {}
    pub(super) type CGImageSourceRef = *mut CGImageSource;

    /// Decode `source`'s encoded bytes into a [`CGImage`], or `None` on any
    /// failure. See the module docs for why every failure mode collapses to
    /// the same `None` rather than a typed error.
    pub(super) fn decode_image(source: &ImageSource) -> Option<CGImage> {
        let bytes: Vec<u8> = match source {
            ImageSource::Path(path) => std::fs::read(path).ok()?,
            ImageSource::Bytes(bytes) => bytes.clone(),
        };
        if bytes.is_empty() {
            return None;
        }

        let provider = CGDataProvider::from_buffer(Arc::new(bytes));
        // SAFETY: `provider.as_ptr()` is a valid, live `CGDataProviderRef`
        // for the duration of this call — `provider` outlives it.
        let source_ref =
            unsafe { CGImageSourceCreateWithDataProvider(provider.as_ptr(), std::ptr::null()) };
        if source_ref.is_null() {
            return None;
        }
        // SAFETY: `source_ref` was just checked non-null. Released
        // immediately below regardless of whether decoding an image out of
        // it succeeds — `CGImageSourceCreateImageAtIndex` doesn't need the
        // source to outlive the image it decodes.
        let image_ref = unsafe { CGImageSourceCreateImageAtIndex(source_ref, 0, std::ptr::null()) };
        unsafe { CFRelease(source_ref as CFTypeRef) };
        if image_ref.is_null() {
            return None;
        }
        // SAFETY: `image_ref` is a live, `+1`-retained `CGImageRef` handed
        // to us by ImageIO. `CGImage`'s `foreign_type!`-generated `Drop`
        // impl calls `CGImageRelease`, so ownership transfers cleanly here.
        Some(unsafe { CGImage::from_ptr(image_ref) })
    }

    /// Shared clip/counter-flip/draw tail for [`paint_image`] and
    /// [`draw_image_cached`] — both decode `image.source` their own way
    /// (fresh vs. cached) and then hand the resulting `cg_image` here to
    /// actually paint it. Counter-flips the CTM locally around
    /// `CGContextDrawImage`, for the same reason
    /// [`super::super::text::draw_text_impl`] counter-flips the text matrix before
    /// `CTLineDraw`: this whole backend paints through a permanently
    /// flipped, top-left-origin, y-down context (`QuadraView`'s translate +
    /// `CGContextScaleCTM(1, -1)`, documented on
    /// `headless::BitmapSurface::new`), and `CGContextDrawImage` has no
    /// matrix-setter equivalent to compensate for that the way text does —
    /// the caller has to re-flip the CTM locally around the draw itself.
    /// Reuses [`super::super::backend::ns_push_clip`]/[`super::super::backend::ns_pop_clip`]
    /// — the same save/clip/restore pair every other clipping rasteriser in
    /// `macos::` uses — rather than hand-rolling a third clip helper.
    ///
    /// # Safety
    /// `ctx` must be a valid, live `CGContextRef` for the duration of the
    /// call.
    pub(super) unsafe fn paint_cg_image(
        ctx: CGContextRef,
        rect: Rect,
        image: &Image,
        cg_image: &CGImage,
    ) -> ImagePaintResult {
        debug_assert!(
            !ctx.is_null(),
            "macos::image: about to paint a decoded image with a null CGContextRef",
        );

        let dest = image.layout(rect).bounds;

        // SAFETY: `ctx` is valid per this function's own contract; `cg_image`
        // is a live, owned `CGImage` for the duration of this call.
        ns_push_clip(ctx, rect);

        // Undo the permanent y-flip locally: move the origin to the bottom
        // of `dest` (in the current, already-once-flipped space) and scale y
        // by -1, then draw into a zero-origin rect of the same size —
        // `ns_pop_clip`'s `CGContextRestoreGState` below restores the CTM
        // along with the clip, so this doesn't need its own save/restore
        // pair. See this function's doc for why the flip is needed at all.
        CGContextTranslateCTM(ctx, dest.x as f64, (dest.y + dest.height) as f64);
        CGContextScaleCTM(ctx, 1.0, -1.0);
        let local_rect = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(dest.width as f64, dest.height as f64),
        );
        CGContextDrawImage(ctx, local_rect, cg_image.as_ptr());

        ns_pop_clip(ctx);

        ImagePaintResult::Painted
    }

    /// Paint `image` within `rect` (points, target-relative), decoding
    /// `image.source` fresh on every call — see the module docs' "Decode
    /// cache" section for why this public entry point deliberately does
    /// *not* cache, and [`draw_image_cached`] for the crate-internal one
    /// that does.
    ///
    /// Decoding happens before `ctx` is ever touched, so a decode failure or
    /// a zero-size `rect` returns [`ImagePaintResult::Unsupported`] without
    /// requiring a live context — `ctx` only needs to be valid when this
    /// function is actually about to paint (checked with a `debug_assert!`
    /// inside [`paint_cg_image`], right before the CoreGraphics calls that
    /// dereference it), unlike [`super::super::minimap::paint_minimap`], which
    /// touches `ctx` unconditionally and so asserts on it up front.
    ///
    /// # Safety
    /// If a decodable `image.source` and non-zero `rect` cause this function
    /// to actually paint, `ctx` must be a valid, live `CGContextRef` for the
    /// duration of the call.
    #[cfg(test)]
    pub unsafe fn paint_image(ctx: CGContextRef, rect: Rect, image: &Image) -> ImagePaintResult {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return ImagePaintResult::Unsupported;
        }
        let Some(cg_image) = decode_image(&image.source) else {
            return ImagePaintResult::Unsupported;
        };
        paint_cg_image(ctx, rect, image, &cg_image)
    }

    /// Paint `image` within `rect` (points, target-relative), decoding
    /// through `cache` — see the module docs. `scale` is folded into
    /// the cache key alongside the (pinned-to-`0`) size for consistency with
    /// every other backend's [`ImageCache`] key, even though this backend's
    /// decode doesn't actually depend on either. Crate-internal:
    /// `MacBackend`'s `surface_draw_image` is the only caller, so this can take
    /// whatever shape is convenient without worrying about breaking an
    /// external caller the way changing [`paint_image`]'s signature would.
    ///
    /// # Safety
    /// Same contract as [`paint_image`].
    pub(crate) unsafe fn draw_image_cached(
        ctx: CGContextRef,
        rect: Rect,
        image: &Image,
        cache: &mut ImageCache<CGImage>,
        scale: f32,
    ) -> ImagePaintResult {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return ImagePaintResult::Unsupported;
        }
        let source = &image.source;
        // Width/height pinned to `0`, not the target rect's real size: this
        // decode is reused verbatim at every paint size (see the module
        // docs' "Decode cache" section) — the [`ImageCache`] key still needs
        // *some* size to stay shape-compatible with every other backend's
        // call, so a constant stands in for "doesn't vary."
        let Some(cg_image) = cache.get_or_decode(source, 0, 0, scale, || decode_image(source))
        else {
            return ImagePaintResult::Unsupported;
        };
        paint_cg_image(ctx, rect, image, cg_image)
    }

    // ImageIO has no binding in the `core-graphics`/`core-foundation` crates
    // this crate already depends on — see the module doc's "Why raw FFI"
    // section. `options` is always passed as `NULL` here (no thumbnail /
    // orientation hints needed for this rasteriser's decode-then-paint use),
    // so it's typed as the real `CFDictionaryRef` for signature accuracy
    // without this module ever constructing one.
    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageSourceCreateWithDataProvider(
            provider: CGDataProviderRef,
            options: CFDictionaryRef,
        ) -> CGImageSourceRef;
        fn CGImageSourceCreateImageAtIndex(
            isrc: CGImageSourceRef,
            index: usize,
            options: CFDictionaryRef,
        ) -> CGImageRef;
    }

    // Linked transitively via `core-graphics` — same pattern every other
    // `macos::*` rasteriser's raw CG FFI block uses (see e.g.
    // `macos::minimap`'s own `extern "C"` block). `CGContextTranslateCTM`/
    // `CGContextScaleCTM` implement the counter-flip documented on
    // `paint_image` above — the same pair `headless::BitmapSurface::new` and
    // `macos::text::draw_text_impl`'s callers use for their own CTM/text-
    // matrix flips.
    extern "C" {
        fn CGContextDrawImage(c: CGContextRef, rect: CGRect, image: CGImageRef);
        fn CGContextTranslateCTM(c: CGContextRef, tx: CGFloat, ty: CGFloat);
        fn CGContextScaleCTM(c: CGContextRef, sx: CGFloat, sy: CGFloat);
    }
}

/// The terminal's shared pixel painter against a CoreGraphics bitmap, both
/// through `MacBackend`'s widget-contract paint and through [`CgSurface`]
/// directly — cell backgrounds, wide glyphs, overlays and clipping.
#[cfg(test)]
mod tests {
    use crate::event::{Rect as QRect, Viewport};
    use crate::macos::headless::BitmapSurface;
    use crate::macos::text::make_font;
    use crate::macos::MacBackend;
    use crate::primitives::terminal::{Terminal, TerminalCell, TerminalCursorShape};
    use crate::terminal_style::wide_glyph_x_scale;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;
    use core_text::font::CTFont;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    /// A line with a wide glyph, a combining mark and mixed bold/italic.
    const LINE: &str = "a日e\u{301}bc";

    fn line_runs() -> Vec<crate::TextRunStyle> {
        let fg = Color::rgb(230, 230, 230);
        vec![
            crate::TextRunStyle::new(0..1, fg),
            crate::TextRunStyle::new(1..4, Color::rgb(220, 60, 60)).with_bold(true),
            crate::TextRunStyle::new(4..7, fg).with_italic(true),
            crate::TextRunStyle::new(7..9, fg),
        ]
    }

    /// `surface_draw_text_line` paints exactly what the editor's per-run
    /// Core Text paint does: each run at its prefix's measured advance.
    #[test]
    fn draw_text_line_matches_the_editors_per_run_paint() {
        use crate::paint_surface::PaintSurface;
        let f = font();
        let runs = line_runs();
        let via_verb = BitmapSurface::new(W, 30);
        via_verb.fill(0.0, 0.0, 0.0, 0.0);
        let mut raw = super::CgSurface {
            ctx: via_verb.context_ptr(),
            font: Some(&f),
        };
        raw.surface_draw_text_line(
            crate::Point::new(4.0, 3.0),
            LINE,
            &runs,
            crate::FontRole::Editor,
        );

        let reference = BitmapSurface::new(W, 30);
        reference.fill(0.0, 0.0, 0.0, 0.0);
        for run in &runs {
            let (prefix_w, _) = crate::macos::text::measure_text(&f, &LINE[..run.range.start]);
            // SAFETY: the bitmap context lives for this whole test.
            unsafe {
                crate::macos::text::draw_text(
                    reference.context_ptr(),
                    &f,
                    &LINE[run.range.clone()],
                    4.0 + prefix_w,
                    3.0,
                    crate::macos::cg::color_to_cg(run.fg),
                )
            };
        }
        assert!(via_verb.bytes().iter().any(|&b| b != 0));
        assert_eq!(via_verb.bytes(), reference.bytes());
    }

    #[test]
    fn text_x_and_index_agree_with_core_text_advances() {
        use crate::paint_surface::PaintSurface;
        let f = font();
        let mut raw = super::CgSurface {
            ctx: std::ptr::null_mut(),
            font: Some(&f),
        };
        let role = crate::FontRole::Editor;
        for idx in [0usize, 1, 4, 7, 8, 9] {
            let expect = crate::macos::text::measure_text(&f, &LINE[..idx]).0 as f32;
            let x = raw.surface_text_x_at_index(LINE, role, idx);
            assert_eq!(x, expect, "x at byte {idx}");
            assert_eq!(
                raw.surface_text_index_at_x(LINE, role, x),
                idx,
                "index at x {x}"
            );
        }
        let narrow = raw.surface_text_x_at_index(LINE, role, 1);
        let wide = raw.surface_text_x_at_index(LINE, role, 4) - narrow;
        assert!(wide > narrow * 1.5, "wide {wide} narrow {narrow}");
        // Between `e` and its combining mark is not a boundary.
        let e_x = raw.surface_text_x_at_index(LINE, role, 4);
        let next = raw.surface_text_x_at_index(LINE, role, 7);
        assert_eq!(
            raw.surface_text_index_at_x(LINE, role, e_x + (next - e_x) * 0.4),
            4
        );
        assert_eq!(
            raw.surface_text_index_at_x(LINE, role, e_x + (next - e_x) * 0.6),
            7
        );
    }

    fn cell(ch: char, fg: Color, bg: Color) -> TerminalCell {
        TerminalCell {
            text: ch.to_string(),
            fg,
            bg,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
            cursor_shape: TerminalCursorShape::Block,
            cursor_blinking: false,
        }
    }

    fn paint(term: &Terminal) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            crate::DrawsPrimitives::draw_primitive(
                b,
                QRect::new(0.0, 0.0, W as f32, H as f32),
                term,
            );
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn cell_bg_paints_through_backend() {
        // One cell, magenta background, glyph 'A'. The fill_rect path
        // should colour the whole cell magenta before the glyph paints.
        let magenta = Color::rgb(200, 30, 200);
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('A', Color::rgb(255, 255, 255), magenta)]],
            scrollbar: None,
        };
        let s = paint(&term);
        // Probe a corner of the cell that the glyph "A" doesn't reach
        // (glyph sits roughly in the middle); top-left of the cell is
        // background.
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!((r, g, b), (magenta.r, magenta.g, magenta.b));
    }

    /// A double-width glyph (CJK) followed by vt100's blank continuation cell must
    /// have its background span both columns — the continuation cell's own
    /// (different) background must NOT paint over the second half of the wide glyph's
    /// cell. This test paints via `CgSurface` directly (rather than through
    /// `MacBackend`'s widget-contract paint) to control `char_width` explicitly instead
    /// of depending on Menlo's measured advance.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        const CHAR_W: f32 = 10.0;
        const LINE_H: f32 = 20.0;

        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        // The wide glyph's foreground is set equal to its background
        // (magenta on magenta) so any antialiased glyph ink can't shift
        // the probed colour — same trick as the GTK twin test, and for
        // the same reason (font-fallback ink at the probe point would
        // otherwise vary by host).
        let row = vec![cell('日', magenta, magenta), cell(' ', magenta, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let theme = Theme::default();
        let f = font();
        let mut raw = super::CgSurface {
            ctx: surface.context_ptr(),
            font: Some(&f),
        };
        crate::primitives::terminal::paint(
            &term, &mut raw, &theme, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, None,
        );

        // Probe just past the first single-cell-width boundary, still
        // within the wide glyph's two-column span: must be magenta, not
        // cyan.
        let probe_x = (CHAR_W * 1.5) as u32;
        let (r, g, b, _) = surface.pixel(probe_x, 5);
        assert_eq!(
            (r, g, b),
            (magenta.r, magenta.g, magenta.b),
            "wide cell's background should span both columns, not be \
             overpainted by the continuation cell's background"
        );
    }

    /// Companion regression: ordinary narrow (single-width) cells must
    /// still advance by exactly `char_width` — the wide-cell fix must
    /// not widen unrelated cells. Mirrors `gtk::backend::tests`'s
    /// `narrow_cells_advance_by_single_char_width`.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        const CHAR_W: f32 = 10.0;
        const LINE_H: f32 = 20.0;

        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        let white = Color::rgb(255, 255, 255);
        // 'B' (the probed cell) paints its glyph in its own background
        // colour so antialiased glyph ink can't corrupt the background
        // probe.
        let row = vec![cell('A', white, magenta), cell('B', cyan, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let theme = Theme::default();
        let f = font();
        let mut raw = super::CgSurface {
            ctx: surface.context_ptr(),
            font: Some(&f),
        };
        crate::primitives::terminal::paint(
            &term, &mut raw, &theme, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, None,
        );

        // Just past the single-cell-width boundary: second cell's own
        // background (cyan) should already be showing.
        let probe_x = (CHAR_W * 1.5) as u32;
        let (r, g, b, _) = surface.pixel(probe_x, 5);
        assert_eq!(
            (r, g, b),
            (cyan.r, cyan.g, cyan.b),
            "narrow cells must still advance by exactly char_width"
        );
    }

    /// Mirrors the GTK twin — both backends share `wide_glyph_x_scale`,
    /// so a CJK glyph measuring 15px in an 18px (2 × 9px) box scales
    /// 1.2x on macOS too.
    #[test]
    fn narrow_wide_glyph_is_stretched_to_fill_two_cells() {
        let cell_w = 18.0;
        let scale = wide_glyph_x_scale(15.0, cell_w);
        assert!(
            (scale - 1.2).abs() < 1e-9,
            "15px glyph in an 18px box should scale 1.2x, got {scale}"
        );
        assert!((15.0 * scale - cell_w).abs() < 1e-9);
    }

    /// Mirrors the GTK twin.
    #[test]
    fn exact_fit_wide_glyph_is_not_scaled() {
        assert_eq!(wide_glyph_x_scale(18.0, 18.0), 1.0);
    }

    /// Mirrors the GTK twin.
    #[test]
    fn over_wide_glyph_is_shrunk_into_box() {
        let scale = wide_glyph_x_scale(24.0, 18.0);
        assert!(
            scale < 1.0,
            "24px glyph in 18px box should shrink, got {scale}"
        );
        assert!((24.0 * scale - 18.0).abs() < 1e-9);
    }

    #[test]
    fn cursor_cell_inverts_fg_bg() {
        // Cursor flag flips the cell so bg is painted with the cell's fg.
        let fg = Color::rgb(10, 220, 30);
        let bg = Color::rgb(40, 40, 40);
        let mut c = cell('X', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let (r, g, b, _) = s.pixel(0, 0);
        // Cursor swap: bg now uses fg.
        assert_eq!((r, g, b), (fg.r, fg.g, fg.b));
    }

    #[test]
    fn find_active_cell_paints_orange() {
        let mut c = cell('z', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.is_find_active = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!((r, g, b), (255, 165, 0));
    }

    #[test]
    fn selected_cell_uses_theme_selection_bg() {
        let mut c = cell('x', Color::rgb(255, 255, 255), Color::rgb(10, 10, 10));
        c.selected = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let theme = Theme::default();
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!(
            (r, g, b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
        );
    }

    #[test]
    fn cells_past_width_are_clipped() {
        // Build a row of 200 magenta cells. At a typical char_width of
        // ~8px, only ~25 fit in W=200. Past the clip we should still
        // see transparent black (surface initial fill).
        let magenta = Color::rgb(200, 30, 200);
        let row: Vec<_> = (0..200)
            .map(|_| cell(' ', Color::rgb(255, 255, 255), magenta))
            .collect();
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let s = paint(&term);
        // Right-most pixel column: cells should NOT have painted there
        // because clipping kicks in at `x + char_width > x + W` — and
        // 200 / char_width < 200 cells fit.
        // Probe just inside the right edge: this should still be magenta
        // (one of the cells fits there).
        let inside = s.pixel(W / 2, 0);
        assert_eq!(
            (inside.0, inside.1, inside.2),
            (magenta.r, magenta.g, magenta.b)
        );
    }

    /// Rows past the pane bottom are clipped.
    ///
    /// A grid taller than the pane — exactly what an un-debounced
    /// interactive-resize frame produces — must stop at the pane bottom
    /// rather than bleed into whatever the host paints below.
    #[test]
    fn rows_past_the_pane_bottom_are_clipped() {
        let magenta = Color::rgb(200, 30, 200);
        // 60 rows at ~16pt line height is ~960pt of grid for a 60pt pane.
        let cells: Vec<Vec<_>> = (0..60)
            .map(|_| {
                (0..20)
                    .map(|_| cell(' ', Color::rgb(255, 255, 255), magenta))
                    .collect()
            })
            .collect();
        let term = Terminal {
            id: WidgetId::new("term"),
            cells,
            scrollbar: None,
        };

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let pane_h = 60.0_f32;
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            crate::DrawsPrimitives::draw_primitive(
                b,
                QRect::new(0.0, 0.0, W as f32, pane_h),
                &term,
            );
        });
        backend.end_frame();

        // Inside the pane: painted.
        let (r, g, b, _) = surface.pixel(4, 4);
        assert_eq!(
            (r, g, b),
            (magenta.r, magenta.g, magenta.b),
            "the pane's own rows should still paint",
        );

        // Well below the pane bottom: untouched (transparent black).
        let below = surface.pixel(4, H - 4);
        assert_eq!(
            below,
            (0, 0, 0, 0),
            "rows below the pane bottom must be clipped, not bled",
        );
    }

    /// `cargo test -p quadraui --no-default-features --features macos -- --ignored --nocapture macos::surface::tests::dump_smoke_ppm`
    ///
    /// Paints a sample terminal — three rows of mixed cells, a cursor
    /// cell, two find matches with one active, a selection run, plus a
    /// scrollbar — and writes `/tmp/quadraui_terminal.ppm`. Open in
    /// Preview to confirm:
    /// - Row 0 reads `quadraui terminal` in white on dark.
    /// - Row 1 has an inverted (white-on-green) cursor inside the
    ///   word `cursor`.
    /// - Row 2 shows the orange find-active match and dim find-match
    ///   highlights, plus a contiguous selection band using
    ///   `theme.selection_bg`.
    /// - Right edge has a scrollbar gutter + thumb (~50% down the
    ///   track, since `scroll_offset = 50` of 100 lines, 16 visible).
    #[test]
    #[ignore = "writes /tmp/quadraui_terminal.ppm — opt in with --ignored"]
    fn dump_smoke_ppm() {
        use crate::primitives::terminal::TerminalScrollbar;

        let white = Color::rgb(230, 230, 230);
        let dark = Color::rgb(20, 20, 30);
        let green = Color::rgb(60, 200, 90);

        fn row(s: &str, fg: Color, bg: Color) -> Vec<TerminalCell> {
            s.chars()
                .map(|ch| TerminalCell {
                    text: ch.to_string(),
                    fg,
                    bg,
                    bold: false,
                    italic: false,
                    underline: false,
                    dim: false,
                    selected: false,
                    is_cursor: false,
                    is_find_match: false,
                    is_find_active: false,
                    cursor_shape: TerminalCursorShape::Block,
                    cursor_blinking: false,
                })
                .collect()
        }

        let r0 = row("quadraui terminal", white, dark);
        let mut r1 = row("  cursor here", white, dark);
        // Inverted cursor on the 'c' of "cursor".
        r1[2].fg = green;
        r1[2].is_cursor = true;
        let mut r2 = row("  find foo bar foo qux", white, dark);
        // Active find on first "foo" (chars 7..10), dim match on second
        // "foo" (chars 15..18).
        for c in r2.iter_mut().take(10).skip(7) {
            c.is_find_active = true;
        }
        for c in r2.iter_mut().take(18).skip(15) {
            c.is_find_match = true;
        }
        // Selection on " bar " (chars 10..15).
        for c in r2.iter_mut().take(15).skip(10) {
            c.selected = true;
        }

        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![r0, r1, r2],
            scrollbar: Some(TerminalScrollbar {
                total_lines: 100,
                visible_lines: 16,
                scroll_offset: 50,
                inverted: false,
                width: None,
            }),
        };
        let s = paint(&term);
        s.write_ppm_and_open("/tmp/quadraui_terminal.ppm");
    }
}

/// The `minimap` widget painted through this backend's adapter
/// surface, as the pixel tests below drive it.
#[cfg(test)]
mod minimap_pixel_tests {
    #![allow(dead_code)]

    use core_graphics::sys::CGContextRef;
    use core_text::font::CTFont;

    use super::super::surface::CgSurface;
    use crate::event::Rect;
    use crate::primitives::minimap::paint_pixels::{self, Glyphs};
    use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapScale};
    use crate::theme::Theme;

    /// The pixel layout of `minimap` in `rect` without painting, at
    /// [`MinimapScale::One`]. `rect.x`/`rect.y` are baked into every bound.
    pub fn mac_minimap_layout(minimap: &Minimap, rect: Rect) -> MinimapLayout {
        mac_minimap_layout_scaled(minimap, rect, MinimapScale::One)
    }

    /// [`mac_minimap_layout`] at an explicit [`MinimapScale`] — see
    /// [`Minimap::pixel_layout`].
    pub(crate) fn mac_minimap_layout_scaled(
        minimap: &Minimap,
        rect: Rect,
        scale: MinimapScale,
    ) -> MinimapLayout {
        minimap.pixel_layout(rect, scale, None)
    }

    /// # Safety
    ///
    /// `ctx` must be a valid `CGContextRef` for the duration of the call.
    unsafe fn paint(
        ctx: CGContextRef,
        font: &CTFont,
        rect: Rect,
        minimap: &Minimap,
        theme: &Theme,
        scale: MinimapScale,
        glyphs: Glyphs,
    ) -> MinimapLayout {
        let layout = mac_minimap_layout_scaled(minimap, rect, scale);
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return layout;
        }
        let mut surface = CgSurface {
            ctx,
            font: Some(font),
        };
        paint_pixels::paint(minimap, &layout, &mut surface, theme, scale, glyphs);
        layout
    }

    /// Draw `minimap` into `rect` at [`MinimapScale::One`] as column blocks,
    /// without the glyph atlas. Returns the layout for host click dispatch
    /// (`layout.hit_test(x, y)`).
    ///
    /// # Safety
    ///
    /// `ctx` must be a valid `CGContextRef` borrowed for the duration of this
    /// call (typical: the frame-scope pointer stashed on
    /// [`super::super::MacBackend`]). Calling with a freed or null pointer is UB.
    pub unsafe fn paint_minimap(
        ctx: CGContextRef,
        font: &CTFont,
        rect: Rect,
        minimap: &Minimap,
        theme: &Theme,
    ) -> MinimapLayout {
        paint(
            ctx,
            font,
            rect,
            minimap,
            theme,
            MinimapScale::One,
            Glyphs::Blocks,
        )
    }

    /// Draw `minimap` at `scale` with glyph-atlas character shapes sampled
    /// from `font`'s family, each tile `dpi_scale` device pixels per point.
    ///
    /// # Safety
    ///
    /// Same contract as [`paint_minimap`].
    pub(crate) unsafe fn paint_minimap_cached(
        ctx: CGContextRef,
        font: &CTFont,
        rect: Rect,
        minimap: &Minimap,
        theme: &Theme,
        scale: MinimapScale,
        dpi_scale: f64,
    ) -> MinimapLayout {
        paint(
            ctx,
            font,
            rect,
            minimap,
            theme,
            scale,
            Glyphs::Atlas {
                dpi_scale: dpi_scale as f32,
            },
        )
    }

    mod tests {
        use super::*;
        use crate::macos::headless::BitmapSurface;
        use crate::macos::text::make_font;
        use crate::primitives::minimap::{MinimapHit, MinimapLine, ROW_PITCH_PX};
        use crate::types::{Color, WidgetId};

        const W: f32 = 200.0;
        const H: f32 = 200.0;

        fn minimap_from(lines: Vec<&str>, total_buffer_lines: usize) -> Minimap {
            Minimap {
                id: WidgetId::new("mm"),
                lines: lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| MinimapLine {
                        text: t.into(),
                        line_idx: i,
                    })
                    .collect(),
                syntax_spans: Vec::new(),
                visible_row_start: 0,
                visible_row_count: 0,
                total_buffer_lines,
                scale: Default::default(),
                sizing: None,
            }
        }

        fn test_font() -> CTFont {
            make_font("Menlo", 10.0).expect("Menlo should be installed on every macOS host")
        }

        /// C0 smoke: `paint_minimap` must actually paint pixels + return a
        /// click-routable layout. The default fixed pitch stays below the
        /// legibility floor, so this exercises `ColumnBlocks`, not glyph shaping —
        /// the "non-background pixel actually painted" check every other `macos::`
        /// C0 smoke uses stands in for "text_ok" here.
        #[test]
        fn draw_minimap_paints_column_blocks_and_returns_layout() {
            let surface = BitmapSurface::new(W as u32, H as u32);
            surface.fill(1.0, 1.0, 1.0, 1.0);
            let font = test_font();
            let theme = Theme {
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                ..Theme::default()
            };
            let mm = minimap_from(vec!["fn main() {}"; 8], 8);
            let rect = Rect::new(0.0, 0.0, W, H);

            let layout = unsafe { paint_minimap(surface.context_ptr(), &font, rect, &mm, &theme) };

            assert!(!layout.visible_lines.is_empty());

            let mut painted_any = false;
            for x in 0..W as u32 {
                for y in 0..(ROW_PITCH_PX as u32 * 8).max(8) {
                    let (r, g, b, _) = surface.pixel(x, y);
                    if (r, g, b) != (255, 255, 255) {
                        painted_any = true;
                    }
                }
            }
            assert!(
                painted_any,
                "expected paint_minimap to paint visible blocks"
            );
        }

        /// Paint↔click round trip at a non-zero origin — tests the invariant
        /// that LOCAL coordinates are used consistently, mirrored from
        /// `macos::board`/`win::minimap`'s own nonzero-origin tests.
        #[test]
        fn paint_and_click_round_trip_at_nonzero_origin() {
            let origin_x = 7.0_f32;
            let origin_y = 13.0_f32;
            let surface = BitmapSurface::new(W as u32, H as u32);
            let font = test_font();
            let theme = Theme::default();
            let mm = minimap_from(vec!["x"; 8], 8);
            let rect = Rect::new(origin_x, origin_y, 40.0, 100.0);

            let layout = unsafe { paint_minimap(surface.context_ptr(), &font, rect, &mm, &theme) };

            assert_eq!(
                layout.hit_test(origin_x + 20.0, origin_y + 50.0),
                MinimapHit::Seek { fraction: 0.5 }
            );
            assert_eq!(
                layout.hit_test(origin_x + 20.0, origin_y),
                MinimapHit::Seek { fraction: 0.0 }
            );
        }

        /// No-paint layout must agree byte-for-byte with what `paint_minimap`
        /// painted — same contract every other backend's
        /// `no_paint_layout_matches_paint_layout` test proves.
        #[test]
        fn no_paint_layout_matches_paint_layout() {
            let mm = minimap_from(vec!["fn main() {}"; 8], 8);
            let rect = Rect::new(0.0, 0.0, W, H);
            let font = test_font();
            let surface = BitmapSurface::new(W as u32, H as u32);

            let painted = unsafe {
                paint_minimap(surface.context_ptr(), &font, rect, &mm, &Theme::default())
            };
            let no_paint = mac_minimap_layout(&mm, rect);
            assert_eq!(painted, no_paint);
        }

        /// Zero-size rect is a no-op — mirrors every other backend's same
        /// guard.
        #[test]
        fn zero_size_rect_is_a_no_op() {
            let surface = BitmapSurface::new(W as u32, H as u32);
            let font = test_font();
            let theme = Theme {
                background: Color::rgb(255, 255, 255),
                ..Theme::default()
            };
            let mm = minimap_from(vec!["fn main() {}"; 8], 8);
            let rect = Rect::new(0.0, 0.0, 0.0, H);

            surface.fill(1.0, 1.0, 1.0, 1.0);

            unsafe {
                paint_minimap(surface.context_ptr(), &font, rect, &mm, &theme);
            }

            let (r, g, b, _) = surface.pixel(1, 1);
            assert_eq!(
                (r, g, b),
                (255, 255, 255),
                "a zero-width minimap should paint nothing at all",
            );
        }

        /// The fixed pitch stays independent of both the strip height and the
        /// file length — same parity guarantee `gtk::minimap`'s/`win::minimap`'s
        /// `row_pitch_is_independent_of_the_strip_height` /
        /// `_of_the_file_length` tests pin, now proven on macOS's own
        /// `mac_minimap_layout`.
        #[test]
        fn row_pitch_is_independent_of_strip_height_and_file_length() {
            let short = minimap_from(vec!["fn main() {}"; 3], 3);
            let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
            let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
            let long = minimap_from(long_refs, 10_000);

            let tall = mac_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 800.0));
            let short_strip = mac_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 16.0));
            let long_layout = mac_minimap_layout(&long, Rect::new(0.0, 0.0, 40.0, 400.0));

            assert_eq!(tall.visible_lines[0].bounds.height as f64, ROW_PITCH_PX);
            assert_eq!(
                short_strip.visible_lines[0].bounds.height as f64,
                ROW_PITCH_PX
            );
            assert_eq!(
                long_layout.visible_lines[0].bounds.height as f64,
                ROW_PITCH_PX
            );
        }

        // ── glyph atlas ──────────────────────────────────────────────────────

        /// At [`MinimapScale::Two`] the atlas keeps every row's ink inside its
        /// own row band. Row 1 here is blank, so *any* ink in its band can
        /// only be row 0's glyph bleeding past its own row.
        #[test]
        fn atlas_mode_keeps_each_row_within_its_own_band() {
            let code_line = "fn main()";
            let mm = minimap_from(vec![code_line, ""], 2);
            let scale = MinimapScale::Two;
            let cell_w = scale.cell_w_px() as u32;
            let row_h = scale.row_pitch_px() as u32;
            let w = (code_line.chars().count() as u32) * cell_w;
            let h = row_h * 2;

            let theme = Theme {
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                ..Theme::default()
            };

            let surface = BitmapSurface::new(w, h);
            surface.fill(1.0, 1.0, 1.0, 1.0);
            let font = test_font();
            let rect = Rect::new(0.0, 0.0, w as f32, h as f32);

            let layout = unsafe {
                paint_minimap_cached(surface.context_ptr(), &font, rect, &mm, &theme, scale, 1.0)
            };
            assert_eq!(
                layout.visible_lines.len(),
                2,
                "expected exactly 2 visible rows"
            );

            // Row 1 (blank) must stay fully background across its whole band
            // — any ink there is row 0's glyph tiles bleeding past their own row.
            for y in row_h..h {
                for x in 0..w {
                    let (r, g, b, _) = surface.pixel(x, y);
                    assert_eq!(
                        (r, g, b),
                        (255, 255, 255),
                        "row 1 (blank) must stay fully background at ({x}, {y}) -- \
                     any ink here means row 0 bled past its own row band"
                    );
                }
            }

            // Sanity: row 0 must have actually painted *something*, or the
            // containment check above would be vacuous.
            let mut painted_row0 = false;
            for y in 0..row_h {
                for x in 0..w {
                    let (r, g, b, _) = surface.pixel(x, y);
                    if (r, g, b) != (255, 255, 255) {
                        painted_row0 = true;
                    }
                }
            }
            assert!(painted_row0, "expected row 0 to paint something");
        }

        /// At [`MinimapScale::Two`], a row of real code paints *shapes* —
        /// non-uniform pixel values across a character's own cell, not a
        /// solid, uniformly-coloured block — mirroring
        /// `gtk::minimap::scale_two_atlas_mode_paints_a_shape_and_leaves_blank_columns_empty`.
        #[test]
        fn atlas_mode_paints_a_shape_not_a_solid_block() {
            let line = "fn main()";
            let mm = minimap_from(vec![line], 1);
            let scale = MinimapScale::Two;
            let cell_w = scale.cell_w_px() as u32;
            let row_h = scale.row_pitch_px() as u32;
            let w = line.chars().count() as u32 * cell_w;

            let theme = Theme {
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                ..Theme::default()
            };

            let surface = BitmapSurface::new(w, row_h);
            surface.fill(1.0, 1.0, 1.0, 1.0);
            let font = test_font();
            let rect = Rect::new(0.0, 0.0, w as f32, row_h as f32);

            unsafe {
                paint_minimap_cached(surface.context_ptr(), &font, rect, &mm, &theme, scale, 1.0);
            }

            // The first character's own cell ('f', column 0) must show a
            // shape -- at least two pixels inside the cell with different
            // colours -- rather than a solid, uniformly-filled block.
            let mut cell_pixels = Vec::new();
            for dx in 0..cell_w {
                for y in 0..row_h {
                    cell_pixels.push(surface.pixel(dx, y));
                }
            }
            let distinct: std::collections::HashSet<_> = cell_pixels.iter().collect();
            assert!(
            distinct.len() > 1,
            "expected the 'f' cell to show a shape (multiple distinct pixel values), got {cell_pixels:?}"
        );
        }

        /// The sample sheet shapes each ASCII glyph
        /// into is a private, throwaway surface (never real screen content), but
        /// `crate::testing`'s text-run sink is a **thread-local**, not scoped to
        /// any particular `CGContextRef` — so if this function ever stopped
        /// pausing/discarding recording around its `draw_text` calls, every atlas
        /// cache miss during a `MacDriver`-based test (guaranteed on a fresh
        /// backend's first paint) would silently append all `ATLAS_CHAR_COUNT`
        /// sample glyphs' bogus bounds to the shared sink that backs
        /// `MacDriver::find`/`screen_contains`.
        #[test]
        fn the_glyph_sample_sheet_does_not_pollute_the_text_run_sink() {
            let previous = crate::testing::install_text_run_sink();
            let chars: String = (' '..='~').collect();
            let _ = crate::macos::surface::glyph_alpha_sheet(
                "Menlo",
                &chars,
                paint_pixels::SAMPLE_CELL,
            );
            let recorded = crate::testing::take_text_run_sink(previous);
            assert!(
                recorded.is_empty(),
                "the glyph sample sheet must never record into the text-run sink, got {recorded:?}"
            );
        }
    }
}

#[cfg(test)]
mod image_tests {
    use super::super::headless::BitmapSurface;
    use super::image::*;
    use crate::backend::ImagePaintResult;
    use crate::event::Rect;
    use crate::image_cache::ImageCache;
    use crate::primitives::image::ImageFit;
    use crate::primitives::image::{Image, ImageSource};
    use crate::types::WidgetId;
    use core_graphics::image::CGImage;

    const W: u32 = 200;
    const H: u32 = 200;

    /// A tiny solid-colour PNG, generated at test time rather than
    /// checked in as a binary fixture — same approach
    /// `win::image::tests::tiny_png_bytes` takes (itself mirroring
    /// `gtk::image::tests::tiny_png_bytes`), just re-derived here so this
    /// module has no cross-backend test dependency.
    fn tiny_png_bytes() -> Vec<u8> {
        // Hand-rolled minimal 4x4 opaque-red PNG: a real decoder
        // (ImageIO) must parse this from scratch, so this is not a
        // stand-in for a "loads bytes" no-op — it exercises the actual
        // decode path.
        //
        // `clippy::same_item_push` misreads the filter-byte push as a
        // "replace this loop with vec![0; N]" candidate — the loop body
        // also appends the row's actual pixel bytes right after it, so
        // that rewrite doesn't apply.
        #[allow(clippy::same_item_push)]
        let raw = {
            let mut raw = Vec::new();
            for _ in 0..4 {
                raw.push(0u8);
                for _ in 0..4 {
                    raw.extend_from_slice(&[0xff, 0x00, 0x00]);
                }
            }
            raw
        };
        png_from_rows(&raw)
    }

    /// A 4x4 PNG that is deliberately *not* symmetric top-to-bottom: rows
    /// 0-1 (the top of the raster, in file order) are opaque red, rows
    /// 2-3 (the bottom) are opaque blue. `tiny_png_bytes`'s uniform
    /// solid colour can't distinguish right-side-up from upside-down —
    /// swapping the two halves would still pass every assertion built on
    /// it. This fixture exists so
    /// [`draw_image_orientation_matches_raster_row_order`] actually fails
    /// if `paint_image`'s counter-flip (see the module docs) is missing
    /// or backwards.
    fn two_tone_png_bytes() -> Vec<u8> {
        let mut raw = Vec::new();
        for row in 0..4u32 {
            raw.push(0u8); // filter byte: None
            let colour: [u8; 3] = if row < 2 {
                [0xff, 0x00, 0x00] // top half: red
            } else {
                [0x00, 0x00, 0xff] // bottom half: blue
            };
            for _ in 0..4 {
                raw.extend_from_slice(&colour);
            }
        }
        png_from_rows(&raw)
    }

    /// Wrap `raw` (already-filtered 4x4 RGB scanline bytes, 8-bit,
    /// no interlace) in a minimal PNG container — the shared plumbing
    /// behind [`tiny_png_bytes`] and [`two_tone_png_bytes`].
    fn png_from_rows(raw: &[u8]) -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");

        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = Vec::with_capacity(4 + data.len());
            body.extend_from_slice(kind);
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        }

        // IHDR: 4x4, 8-bit depth, colour type 2 (RGB), no interlace.
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &ihdr);

        // IDAT: `raw`'s scanlines, zlib-wrapped with a stored
        // (uncompressed) block.
        let idat = zlib_store(raw);
        chunk(&mut png, b"IDAT", &idat);

        chunk(&mut png, b"IEND", &[]);
        png
    }

    /// Zlib container with a single stored (uncompressed) deflate block —
    /// enough for ImageIO's PNG decoder to accept, no compression needed
    /// for a 4x4 fixture.
    fn zlib_store(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01]; // zlib header (no compression)
        let len = data.len() as u16;
        out.push(1); // BFINAL=1, BTYPE=00 (stored)
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(&adler32(data).to_be_bytes());
        out
    }

    fn adler32(data: &[u8]) -> u32 {
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &byte in data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn image(source: ImageSource, fit: ImageFit) -> Image {
        Image {
            id: WidgetId::new("logo"),
            source,
            intrinsic_size: Some((4, 4)),
            fit,
            fallback_text: "[LOGO]".into(),
        }
    }

    fn cache() -> ImageCache<CGImage> {
        ImageCache::with_capacity(4)
    }

    /// A decodable source must paint pixels and report `Painted`.
    #[test]
    fn draw_image_from_bytes_paints_real_pixels() {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);

        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Fill);
        let rect = Rect::new(20.0, 20.0, 60.0, 60.0);

        // SAFETY: `surface.context_ptr()` is valid for the surface's
        // lifetime, which outlives this call.
        let result = unsafe { paint_image(surface.context_ptr(), rect, &img) };

        assert_eq!(result, ImagePaintResult::Painted);
        let (r, g, b, _) = surface.pixel(50, 50);
        assert_eq!((r, g, b), (255, 0, 0), "decoded PNG should be red");

        // Outside the target rect: untouched.
        let (r, g, b, _) = surface.pixel(5, 5);
        assert_eq!(
            (r, g, b),
            (255, 255, 255),
            "painting must not bleed outside the target rect"
        );
    }

    /// Corrupt or missing sources must degrade to `Unsupported`
    /// rather than panicking.
    #[test]
    fn draw_image_from_missing_path_reports_unsupported() {
        let surface = BitmapSurface::new(W, H);
        let img = image(
            ImageSource::Path("/nonexistent/does-not-exist.png".into()),
            ImageFit::Contain,
        );
        // SAFETY: surface context valid for this call.
        let result =
            unsafe { paint_image(surface.context_ptr(), Rect::new(0.0, 0.0, 40.0, 40.0), &img) };
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_from_corrupt_bytes_reports_unsupported() {
        let surface = BitmapSurface::new(W, H);
        let img = image(ImageSource::Bytes(vec![0, 1, 2, 3]), ImageFit::Contain);
        // SAFETY: surface context valid for this call.
        let result =
            unsafe { paint_image(surface.context_ptr(), Rect::new(0.0, 0.0, 40.0, 40.0), &img) };
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn zero_size_rect_reports_unsupported_without_decoding() {
        let surface = BitmapSurface::new(W, H);
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Contain);
        // SAFETY: surface context valid for this call.
        let result =
            unsafe { paint_image(surface.context_ptr(), Rect::new(0.0, 0.0, 0.0, 0.0), &img) };
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_cached_from_bytes_paints_real_pixels() {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);

        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Fill);
        let rect = Rect::new(20.0, 20.0, 60.0, 60.0);

        // SAFETY: `surface.context_ptr()` is valid for the surface's
        // lifetime, which outlives this call.
        let result =
            unsafe { draw_image_cached(surface.context_ptr(), rect, &img, &mut cache(), 1.0) };

        assert_eq!(result, ImagePaintResult::Painted);
        let (r, g, b, _) = surface.pixel(50, 50);
        assert_eq!((r, g, b), (255, 0, 0), "decoded PNG should be red");
    }

    #[test]
    fn draw_image_cached_from_missing_path_reports_unsupported() {
        let surface = BitmapSurface::new(W, H);
        let img = image(
            ImageSource::Path("/nonexistent/does-not-exist.png".into()),
            ImageFit::Contain,
        );
        // SAFETY: surface context valid for this call.
        let result = unsafe {
            draw_image_cached(
                surface.context_ptr(),
                Rect::new(0.0, 0.0, 40.0, 40.0),
                &img,
                &mut cache(),
                1.0,
            )
        };
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_cached_zero_size_rect_reports_unsupported_without_decoding() {
        let surface = BitmapSurface::new(W, H);
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Contain);
        // SAFETY: surface context valid for this call.
        let result = unsafe {
            draw_image_cached(
                surface.context_ptr(),
                Rect::new(0.0, 0.0, 0.0, 0.0),
                &img,
                &mut cache(),
                1.0,
            )
        };
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    /// A cache hit must skip re-decoding entirely — this
    /// backend keys on source alone (see the module docs' "Decode cache"
    /// section), so painting the *same* source at two different target
    /// sizes should still hit the cache the second time. Proven the same
    /// way `gtk::image`'s equivalent test is: corrupt the on-disk source
    /// between calls and confirm the second paint still succeeds.
    #[test]
    fn cached_decode_survives_source_becoming_unreadable_on_disk() {
        let path = std::env::temp_dir().join(format!(
            "quadraui_macos_image_cache_test_{}_{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        std::fs::write(&path, tiny_png_bytes()).expect("write tiny png fixture");

        let surface = BitmapSurface::new(W, H);
        let mut shared_cache = cache();
        let img = image(ImageSource::Path(path.clone()), ImageFit::Contain);

        // SAFETY: surface context valid for this call.
        let first = unsafe {
            draw_image_cached(
                surface.context_ptr(),
                Rect::new(0.0, 0.0, 40.0, 40.0),
                &img,
                &mut shared_cache,
                1.0,
            )
        };
        assert_eq!(
            first,
            ImagePaintResult::Painted,
            "first decode should succeed"
        );

        // Corrupt the file in place — a fresh decode from this point on
        // would fail.
        std::fs::write(&path, b"not a png").expect("corrupt fixture on disk");

        // Different target size too — this backend's cache key ignores
        // size (see the module docs), so this must still hit the cache.
        // SAFETY: surface context valid for this call.
        let second = unsafe {
            draw_image_cached(
                surface.context_ptr(),
                Rect::new(0.0, 0.0, 80.0, 80.0),
                &img,
                &mut shared_cache,
                1.0,
            )
        };
        assert_eq!(
            second,
            ImagePaintResult::Painted,
            "second draw must hit the cache rather than re-decoding the now-corrupt file"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// `ImageFit::Cover` can extend past `rect` on one axis — painting
    /// must still stay clipped to it, mirroring `win::image`'s and
    /// `gtk::image`'s own clip contract.
    #[test]
    fn cover_fit_is_clipped_to_rect() {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);

        // Square 4x4 source over a tall, narrow (10x30) rect: `Cover`
        // scales up until the rect's *taller* axis (height) is filled —
        // 7.5x — which drives the resulting square target well past the
        // rect's narrow width on both sides.
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Cover);
        let rect = Rect::new(90.0, 50.0, 10.0, 30.0);

        // SAFETY: surface context valid for this call.
        let result = unsafe { paint_image(surface.context_ptr(), rect, &img) };
        assert_eq!(result, ImagePaintResult::Painted);

        // `Cover` scales the 4x4 source by 7.5x (30 / 4, the taller axis)
        // to a 30x30 target centered on the rect: x in [80, 110], vs. the
        // clip rect's x in [90, 100]. (85, 65) and (105, 65) sit inside
        // the *unclipped* target but outside the clip rect, so a missing
        // or broken clip would paint red there instead of background.
        for (x, y) in [(85u32, 65u32), (105, 65)] {
            let (r, g, b, _) = surface.pixel(x, y);
            assert_eq!(
                (r, g, b),
                (255, 255, 255),
                "Cover fit must not bleed past the clip rect at ({x}, {y})"
            );
        }

        // Inside the clip rect: the image did paint.
        let (r, g, b, _) = surface.pixel(95, 65);
        assert_eq!(
            (r, g, b),
            (255, 0, 0),
            "expected the image inside the clip rect"
        );
    }

    /// Without the counter-flip documented on `paint_image` (mirroring
    /// `macos::text::draw_text_impl`'s text-matrix flip),
    /// `CGContextDrawImage` paints upside down inside this backend's
    /// permanently-flipped, top-left-origin context. `tiny_png_bytes`'s
    /// uniform fill cannot detect orientation — a mirrored image is
    /// pixel-for-pixel identical to the original along any horizontal
    /// scanline. `two_tone_png_bytes` (red top half, blue bottom half in
    /// raster row order) fails this assertion if the flip is missing or
    /// backwards.
    #[test]
    fn draw_image_orientation_matches_raster_row_order() {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);

        let img = image(ImageSource::Bytes(two_tone_png_bytes()), ImageFit::Fill);
        let rect = Rect::new(20.0, 20.0, 60.0, 60.0);

        // SAFETY: surface context valid for this call.
        let result = unsafe { paint_image(surface.context_ptr(), rect, &img) };
        assert_eq!(result, ImagePaintResult::Painted);

        // Near the top of the target rect: the raster's top half (red).
        let (r, g, b, _) = surface.pixel(50, 25);
        assert_eq!(
            (r, g, b),
            (255, 0, 0),
            "top of the painted rect should show the raster's top-half colour (red) — \
             a mirrored/upside-down paint would show blue here instead"
        );

        // Near the bottom of the target rect: the raster's bottom half
        // (blue).
        let (r, g, b, _) = surface.pixel(50, 75);
        assert_eq!(
            (r, g, b),
            (0, 0, 255),
            "bottom of the painted rect should show the raster's bottom-half colour (blue) — \
             a mirrored/upside-down paint would show red here instead"
        );
    }
}

/// The `editor` widget painted through this backend's adapter
/// surface, as the pixel tests below drive it.
#[cfg(test)]
mod editor_pixel_tests {
    #![allow(dead_code)]

    use core_graphics::sys::CGContextRef;
    use core_text::font::CTFont;

    use super::super::surface::CgSurface;
    use crate::paint_surface::PaintSurface;
    use crate::primitives::editor::{paint_pixels, Editor, EditorPaintOptions};
    use crate::theme::Theme;

    /// Paint `editor` onto `ctx`.
    ///
    /// Equivalent to [`paint_editor_with_options`] with
    /// `EditorPaintOptions::default()` — kept as a separate, unchanged
    /// function (rather than growing this one's argument list) so every
    /// existing caller keeps compiling untouched.
    ///
    /// # Safety
    ///
    /// `ctx` must be a valid `CGContextRef` borrowed for the duration of
    /// the call.
    pub unsafe fn paint_editor(
        ctx: CGContextRef,
        font: &CTFont,
        editor: &Editor,
        theme: &Theme,
        char_width: f64,
        line_height: f64,
    ) {
        paint_editor_with_options(
            ctx,
            font,
            editor,
            theme,
            char_width,
            line_height,
            EditorPaintOptions::default(),
        )
    }

    /// [`paint_editor`], plus [`EditorPaintOptions`] a host can set to
    /// override otherwise-automatic paint decisions — `suppress_v_scrollbar`.
    /// See the module doc's "Scrollbar" section.
    ///
    /// Equivalent to [`paint_editor_with_options_and_v_scrollbar_w`] with
    /// `v_scrollbar_w: None` (a `char_width`-wide scrollbar column).
    ///
    /// # Safety
    ///
    /// `ctx` must be a valid `CGContextRef` borrowed for the duration of
    /// the call.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn paint_editor_with_options(
        ctx: CGContextRef,
        font: &CTFont,
        editor: &Editor,
        theme: &Theme,
        char_width: f64,
        line_height: f64,
        options: EditorPaintOptions,
    ) {
        paint_editor_with_options_and_v_scrollbar_w(
            ctx,
            font,
            editor,
            theme,
            char_width,
            line_height,
            options,
            None,
        )
    }

    /// [`paint_editor_with_options`], plus a host-settable vertical scrollbar
    /// width in points — see [`Editor::layout_with_options_and_v_scrollbar_w`]
    /// for the geometry. `None` sizes the column at `char_width`.
    ///
    /// # Safety
    ///
    /// `ctx` must be a valid `CGContextRef` borrowed for the duration of
    /// the call.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn paint_editor_with_options_and_v_scrollbar_w(
        ctx: CGContextRef,
        font: &CTFont,
        editor: &Editor,
        theme: &Theme,
        char_width: f64,
        line_height: f64,
        options: EditorPaintOptions,
        v_scrollbar_w: Option<f32>,
    ) {
        let rect = editor.rect;
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let geom = editor.layout_with_options_and_v_scrollbar_w(
            rect,
            char_width as f32,
            line_height as f32,
            options,
            v_scrollbar_w,
        );
        let mut surface = CgSurface {
            ctx,
            font: Some(font),
        };
        surface.surface_push_clip(rect);
        paint_pixels::paint_with_options(editor, &geom, &mut surface, theme, options);
        surface.surface_pop_clip();
    }

    mod tests {
        use super::super::super::headless::BitmapSurface;
        use super::super::super::text::{font_metrics, make_font};
        use super::super::super::MacBackend;
        use super::*;
        use crate::event::{Rect as QRect, Viewport};
        use crate::primitives::editor::{
            CursorPos, EditorCursor, EditorLine, EditorStyledSpan as ESpan, Style,
        };
        use crate::primitives::editor::{CursorShape, DiffLine, EditorSelection, SelectionKind};
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::Backend;
        use std::collections::{HashMap, HashSet};

        const W: u32 = 400;
        const H: u32 = 160;

        fn font() -> CTFont {
            make_font("Menlo", 14.0).expect("Menlo installed")
        }

        fn one_line(text: &str) -> EditorLine {
            EditorLine {
                raw_text: text.into(),
                gutter_text: "1".into(),
                spans: vec![],
                line_idx: 0,
                is_current_line: true,
                is_fold_header: false,
                folded_line_count: 0,
                git_diff: None,
                diff_status: None,
                diagnostics: vec![],
                spell_errors: vec![],
                is_breakpoint: false,
                is_conditional_bp: false,
                is_dap_current: false,
                is_wrap_continuation: false,
                segment_col_offset: 0,
                annotation: None,
                ghost_suffix: None,
                is_ghost_continuation: false,
                indent_guides: vec![],
                colorcolumns: vec![],
            }
        }

        fn editor_with_cursor(text: &str, shape: CursorShape, col: usize) -> Editor {
            Editor {
                id: "editor:0".into(),
                rect: QRect::new(0.0, 0.0, W as f32, H as f32),
                lines: vec![one_line(text)],
                cursor: Some(EditorCursor {
                    pos: CursorPos { view_line: 0, col },
                    shape,
                }),
                extra_cursors: vec![],
                selection: None,
                extra_selections: vec![],
                yank_highlight: None,
                scroll_top: 0,
                scroll_left: 0,
                total_lines: 1,
                max_col: text.chars().count(),
                gutter_char_width: 3,
                is_active: true,
                show_active_bg: false,
                has_git_diff: false,
                has_breakpoints: false,
                diagnostic_gutter: HashMap::new(),
                code_action_lines: HashSet::new(),
                bracket_match_positions: vec![],
                active_indent_col: None,
                tabstop: 4,
                cursorline: false,
                lightbulb_glyph: '!',
                paint_options: Default::default(),
            }
        }

        fn paint_via_backend(editor: &Editor) -> BitmapSurface {
            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = MacBackend::new();
            backend.set_current_font(font());
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                crate::DrawsPrimitives::draw_primitive(b, editor.rect, editor);
            });
            backend.end_frame();
            surface
        }

        /// The block caret is a translucent `theme.cursor` cell at
        /// `cursor_normal_alpha` over the glyph — probed on a space so no ink
        /// shifts the colour.
        #[test]
        fn block_cursor_paints_translucent_theme_cursor_cell() {
            let editor = editor_with_cursor("let x = 1;", CursorShape::Block, 3);
            let surface = paint_via_backend(&editor);
            let theme = Theme::default();
            let metrics = font_metrics(&font());
            // Gutter is 3 char widths; column 3 is the space after "let".
            let cx = (6.0 * metrics.char_width) as u32 + 1;
            let cy = (metrics.line_height / 2.0) as u32;
            let (r, g, b, _) = surface.pixel(cx, cy);
            let expect = theme
                .background
                .blend(theme.cursor, theme.cursor_normal_alpha as f64);
            assert!(
                approx(r, expect.r, 3) && approx(g, expect.g, 3) && approx(b, expect.b, 3),
                "block cursor pixel at ({cx}, {cy}) should be theme.cursor at cursor_normal_alpha \
             over the background: got {:?}, expected {:?}",
                (r, g, b),
                (expect.r, expect.g, expect.b)
            );
        }

        /// The shared painter brings the GTK-only features to macOS: a
        /// secondary caret and an indent guide now paint.
        #[test]
        fn secondary_caret_and_indent_guide_paint() {
            let mut editor = editor_with_cursor("let x = 1;", CursorShape::Bar, 0);
            editor.extra_cursors = vec![CursorPos {
                view_line: 0,
                col: 3,
            }];
            editor.lines[0].indent_guides = vec![8];
            let surface = paint_via_backend(&editor);
            let theme = Theme::default();
            let metrics = font_metrics(&font());
            let cy = (metrics.line_height / 2.0) as u32;
            // One pixel in, so the 2px bar fully covers the probe.
            let caret_x = (6.0 * metrics.char_width) as u32 + 1;
            let (r, g, b, _) = surface.pixel(caret_x, cy);
            assert_eq!(
                (r, g, b),
                (theme.cursor.r, theme.cursor.g, theme.cursor.b),
                "secondary bar caret at column 3"
            );
            let guide_x = ((3.0 + 8.0) * metrics.char_width) as u32;
            let painted = (guide_x.saturating_sub(1)..=guide_x + 1).any(|x| {
                let (r, g, b, _) = surface.pixel(x, cy);
                (r, g, b) != (theme.background.r, theme.background.g, theme.background.b)
            });
            assert!(painted, "indent guide at column 8");
        }

        #[test]
        fn cursorline_highlight_painted_when_active() {
            let mut editor = editor_with_cursor("let x = 1;", CursorShape::Bar, 0);
            editor.cursorline = true;
            let surface = paint_via_backend(&editor);
            let theme = Theme::default();
            // Probe far to the right of any glyph or cursor.
            let px = W - 4;
            let py = 4_u32;
            let (r, g, b, _) = surface.pixel(px, py);
            assert_eq!(
                (r, g, b),
                (
                    theme.cursorline_bg.r,
                    theme.cursorline_bg.g,
                    theme.cursorline_bg.b
                ),
            );
        }

        #[test]
        fn span_bg_paints_over_raw_run() {
            // A line whose first 3 bytes carry a coloured background — the
            // probe inside that range should read the span bg, not the
            // editor bg.
            let mut line = one_line("alpha beta");
            line.spans = vec![ESpan {
                start_byte: 0,
                end_byte: 3,
                style: Style {
                    fg: Color::rgb(255, 255, 255),
                    bg: Some(Color::rgb(50, 100, 150)),
                    bold: false,
                    italic: false,
                    font_scale: 1.0,
                },
            }];
            let mut editor = editor_with_cursor("alpha beta", CursorShape::Bar, 99);
            editor.lines = vec![line];
            let surface = paint_via_backend(&editor);
            let metrics = font_metrics(&font());
            // The span covers raw_text[0..3] ("alp"). Probe at the very
            // top scanline (y=0) of the line — above any glyph ink and
            // its anti-aliased boundary, fully inside the span bg.
            let text_start = 3.0 * metrics.char_width;
            let px = (text_start + metrics.char_width * 0.5) as u32;
            let py = 0_u32;
            let (r, g, b, _) = surface.pixel(px, py);
            // Span bg painted over editor bg.
            assert_eq!((r, g, b), (50, 100, 150));
        }

        /// A span covering the entire line must still paint the line's text
        /// exactly once, not as an overlapping pair of runs. Core Text's
        /// recorded [`TextRun`]s (via `start_recording_text`/
        /// `MacBackend::text_runs`) let this be asserted directly: exactly
        /// one run for the line's text, regardless of how many spans cover
        /// it.
        #[test]
        fn span_covering_whole_line_paints_exactly_one_text_run() {
            let mut line = one_line("alpha beta");
            line.spans = vec![ESpan {
                start_byte: 0,
                end_byte: line.raw_text.len(),
                style: Style {
                    fg: Color::rgb(200, 100, 50),
                    bg: None,
                    bold: false,
                    italic: false,
                    font_scale: 1.0,
                },
            }];
            // `Bar` cursor at an out-of-range column: the cursor overlay
            // never draws a glyph of its own (only `Block` repaints one),
            // so the only `draw_text` calls for "alpha beta" come from the
            // line-text painter under test.
            let mut editor = editor_with_cursor("alpha beta", CursorShape::Bar, 99);
            editor.lines = vec![line];

            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = MacBackend::new();
            backend.set_current_font(font());
            backend.set_painted_text_recording(true);
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                crate::DrawsPrimitives::draw_primitive(b, editor.rect, &editor);
            });
            backend.end_frame();

            let whole_line_runs: Vec<_> = backend
                .text_runs()
                .iter()
                .filter(|run| run.text == "alpha beta")
                .collect();
            assert_eq!(
                whole_line_runs.len(),
                1,
                "\"alpha beta\" should be painted exactly once (overlapping raw-run + \
             span-overlay would record it twice at the same position), got: {:?}",
                whole_line_runs,
            );
        }

        /// A span covering only part of the line must leave the uncovered
        /// gaps painted too, and each run — gap or span — must appear
        /// exactly once with no overlap. Pins both: the recorded
        /// [`TextRun`]s for `"alpha beta"` with `spans = [0..5]` must be
        /// exactly the gap `" beta"` and the span `"alpha"`, each starting
        /// where the previous one ends.
        #[test]
        fn span_covering_part_of_line_leaves_gap_runs_contiguous() {
            let mut line = one_line("alpha beta");
            line.spans = vec![ESpan {
                start_byte: 0,
                end_byte: 5,
                style: Style {
                    fg: Color::rgb(200, 100, 50),
                    bg: None,
                    bold: false,
                    italic: false,
                    font_scale: 1.0,
                },
            }];
            let mut editor = editor_with_cursor("alpha beta", CursorShape::Bar, 99);
            editor.lines = vec![line];

            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = MacBackend::new();
            backend.set_current_font(font());
            backend.set_painted_text_recording(true);
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                crate::DrawsPrimitives::draw_primitive(b, editor.rect, &editor);
            });
            backend.end_frame();

            let metrics = font_metrics(&font());
            let text_start = 3.0 * metrics.char_width; // gutter_char_width = 3

            let span_runs: Vec<_> = backend
                .text_runs()
                .iter()
                .filter(|run| run.text == "alpha")
                .collect();
            assert_eq!(
                span_runs.len(),
                1,
                "the span \"alpha\" should be painted exactly once, got: {:?}",
                span_runs,
            );
            assert!(
                (span_runs[0].bounds.x as f64 - text_start).abs() < 0.5,
                "the span run should start at the line's text origin, got x={}",
                span_runs[0].bounds.x,
            );

            let gap_runs: Vec<_> = backend
                .text_runs()
                .iter()
                .filter(|run| run.text == " beta")
                .collect();
            assert_eq!(
                gap_runs.len(),
                1,
                "the uncovered gap \" beta\" should still be painted exactly \
             once, got: {:?}",
                gap_runs,
            );
            let gap_x_expected = text_start + 5.0 * metrics.char_width;
            assert!(
                (gap_runs[0].bounds.x as f64 - gap_x_expected).abs() < 0.5,
                "the gap run should start right after the span ends, got x={}, expected={}",
                gap_runs[0].bounds.x,
                gap_x_expected,
            );
        }

        /// `StyledSpan` byte offsets must be snapped to char boundaries.
        /// A span landing mid-multibyte-character (é / CJK / emoji)
        /// could panic `&line.raw_text[..start]` / `[start..end]`.
        /// This test exercises multibyte spans and a multibyte cursor
        /// column via `char_byte_offset`.
        #[test]
        fn multibyte_span_and_cursor_do_not_panic() {
            // "café🎉 beta" — byte 4 sits inside the 2-byte 'é' (starts at
            // byte 3); byte 8 sits inside the 4-byte emoji (starts at byte 6).
            let text = "café🎉 beta";
            assert!(!text.is_char_boundary(4));
            assert!(!text.is_char_boundary(8));

            let mut line = one_line(text);
            line.spans = vec![ESpan {
                start_byte: 4,
                end_byte: 8,
                style: Style {
                    fg: Color::rgb(255, 255, 255),
                    bg: Some(Color::rgb(50, 100, 150)),
                    bold: false,
                    italic: false,
                    font_scale: 1.0,
                },
            }];
            // Cursor column 3 lands on 'é' (a char index, not a byte
            // offset) — `char_byte_offset` must resolve it to a boundary.
            let mut editor = editor_with_cursor(text, CursorShape::Block, 3);
            editor.lines = vec![line];

            // Must not panic.
            let _surface = paint_via_backend(&editor);
        }

        #[test]
        fn empty_editor_returns_default_paint_result() {
            let editor = Editor {
                id: "editor:empty".into(),
                rect: QRect::new(0.0, 0.0, 0.0, 0.0),
                lines: vec![],
                cursor: None,
                extra_cursors: vec![],
                selection: None,
                extra_selections: vec![],
                yank_highlight: None,
                scroll_top: 0,
                scroll_left: 0,
                total_lines: 0,
                max_col: 0,
                gutter_char_width: 0,
                is_active: false,
                show_active_bg: false,
                has_git_diff: false,
                has_breakpoints: false,
                diagnostic_gutter: HashMap::new(),
                code_action_lines: HashSet::new(),
                bracket_match_positions: vec![],
                active_indent_col: None,
                tabstop: 4,
                cursorline: false,
                lightbulb_glyph: '!',
                paint_options: Default::default(),
            };
            let surface = BitmapSurface::new(10, 10);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = MacBackend::new();
            backend.set_current_font(font());
            backend.begin_frame(Viewport::new(10.0, 10.0, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                crate::DrawsPrimitives::draw_primitive(b, editor.rect, &editor);
            });
            backend.end_frame();
        }

        /// `a` and `b` agree within `tol` — used for alpha-blended overlay
        /// pixels, where the exact composited value depends on CG's
        /// blending internals and only the *direction* (blended toward the
        /// overlay colour) is being asserted.
        fn approx(a: u8, b: u8, tol: u8) -> bool {
            a.abs_diff(b) <= tol
        }

        /// `editor.selection` must be read by the rasteriser.
        /// A `Line`-kind selection should paint `theme.selection`
        /// alpha-blended over the background, not the plain background.
        #[test]
        fn line_selection_paints_over_background() {
            let mut editor = editor_with_cursor("let x = 1;", CursorShape::Bar, 99);
            editor.selection = Some(EditorSelection {
                kind: SelectionKind::Line,
                start_line: 0,
                start_col: 0,
                end_line: 0,
                end_col: 0,
            });
            let surface = paint_via_backend(&editor);
            let theme = Theme::default();

            // Far right of any glyph ink, same probe spot as the cursorline
            // test — inside the full-width Line-selection highlight.
            let (r, g, b, _) = surface.pixel(W - 4, 4);
            assert_ne!(
                (r, g, b),
                (theme.background.r, theme.background.g, theme.background.b),
                "selected row should not read as plain background"
            );
            let expected_r = (theme.selection.r as f32 * theme.selection_alpha
                + theme.background.r as f32 * (1.0 - theme.selection_alpha))
                as u8;
            let expected_g = (theme.selection.g as f32 * theme.selection_alpha
                + theme.background.g as f32 * (1.0 - theme.selection_alpha))
                as u8;
            let expected_b = (theme.selection.b as f32 * theme.selection_alpha
                + theme.background.b as f32 * (1.0 - theme.selection_alpha))
                as u8;
            assert!(
                approx(r, expected_r, 4) && approx(g, expected_g, 4) && approx(b, expected_b, 4),
                "expected ~({expected_r}, {expected_g}, {expected_b}), got ({r}, {g}, {b})"
            );
        }

        /// `yank_highlight` on `Editor` must be read by the rasteriser.
        /// The yank overlay paints its own themed colour,
        /// distinct from a plain `editor.selection` overlay.
        #[test]
        fn yank_highlight_paints_distinct_color() {
            let mut editor = editor_with_cursor("let x = 1;", CursorShape::Bar, 99);
            editor.yank_highlight = Some(EditorSelection {
                kind: SelectionKind::Line,
                start_line: 0,
                start_col: 0,
                end_line: 0,
                end_col: 0,
            });
            let surface = paint_via_backend(&editor);
            let theme = Theme::default();

            let (r, g, b, _) = surface.pixel(W - 4, 4);
            assert_ne!(
                (r, g, b),
                (theme.background.r, theme.background.g, theme.background.b),
                "yanked row should not read as plain background"
            );
            let expected_r = (theme.yank_highlight_bg.r as f32 * theme.yank_highlight_alpha
                + theme.background.r as f32 * (1.0 - theme.yank_highlight_alpha))
                as u8;
            let expected_g = (theme.yank_highlight_bg.g as f32 * theme.yank_highlight_alpha
                + theme.background.g as f32 * (1.0 - theme.yank_highlight_alpha))
                as u8;
            let expected_b = (theme.yank_highlight_bg.b as f32 * theme.yank_highlight_alpha
                + theme.background.b as f32 * (1.0 - theme.yank_highlight_alpha))
                as u8;
            assert!(
                approx(r, expected_r, 4) && approx(g, expected_g, 4) && approx(b, expected_b, 4),
                "expected ~({expected_r}, {expected_g}, {expected_b}), got ({r}, {g}, {b})"
            );
        }

        /// A `Char`-kind selection must only highlight the selected column
        /// range — columns outside it stay plain background.
        #[test]
        fn char_selection_highlights_only_selected_columns() {
            let mut editor = editor_with_cursor("alpha beta", CursorShape::Bar, 99);
            editor.selection = Some(EditorSelection {
                kind: SelectionKind::Char,
                start_line: 0,
                start_col: 0,
                end_line: 0,
                end_col: 2, // inclusive -> covers chars 0..=2, "alp"
            });
            let surface = paint_via_backend(&editor);
            let theme = Theme::default();
            let metrics = font_metrics(&font());
            let text_start = 3.0 * metrics.char_width; // gutter width

            // Inside the selected range (char index 1, "l").
            let in_px = (text_start + metrics.char_width * 1.5) as u32;
            let (r, g, b, _) = surface.pixel(in_px, 0);
            assert_ne!(
                (r, g, b),
                (theme.background.r, theme.background.g, theme.background.b),
                "column inside selection should be highlighted"
            );

            // Outside the selected range (char index 6, "e" of "beta").
            let out_px = (text_start + metrics.char_width * 6.5) as u32;
            let (r2, g2, b2, _) = surface.pixel(out_px, 0);
            assert_eq!(
                (r2, g2, b2),
                (theme.background.r, theme.background.g, theme.background.b),
                "column outside selection should stay plain background"
            );
        }

        /// Diff backgrounds (`DiffLine::Added` / `Removed` / `Padding`)
        /// must be painted by the rasteriser. Diff status takes priority
        /// over cursorline, matching `crate::gtk::editor::paint_editor`'s
        /// priority order.
        #[test]
        fn diff_added_background_overrides_cursorline() {
            let mut line = one_line("added line");
            line.diff_status = Some(DiffLine::Added);
            let mut editor = editor_with_cursor("added line", CursorShape::Bar, 99);
            editor.lines = vec![line];
            editor.cursorline = true; // would paint cursorline_bg if diff didn't win
            let surface = paint_via_backend(&editor);
            let theme = Theme::default();

            let (r, g, b, _) = surface.pixel(W - 4, 4);
            assert_eq!(
                (r, g, b),
                (
                    theme.diff_added_bg.r,
                    theme.diff_added_bg.g,
                    theme.diff_added_bg.b
                ),
                "diff-added row should paint theme.diff_added_bg, not cursorline_bg"
            );
        }

        /// Scan `surface` for the vertical extent of non-background ink
        /// inside `[x_start, x_end) × [y_start, y_end)` — the smallest `top`
        /// and largest `bottom` row (inclusive) where any pixel in the
        /// column range differs from `bg` by more than a small
        /// anti-aliasing tolerance. `None` if the window contains no ink.
        fn ink_vertical_extent(
            surface: &BitmapSurface,
            x_start: u32,
            x_end: u32,
            y_start: u32,
            y_end: u32,
            bg: Color,
        ) -> Option<(u32, u32)> {
            const TOL: i32 = 8;
            let mut extent: Option<(u32, u32)> = None;
            for y in y_start..y_end {
                let has_ink = (x_start..x_end).any(|x| {
                    let (r, g, b, _) = surface.pixel(x, y);
                    (r as i32 - bg.r as i32).abs() > TOL
                        || (g as i32 - bg.g as i32).abs() > TOL
                        || (b as i32 - bg.b as i32).abs() > TOL
                });
                if has_ink {
                    extent = Some(match extent {
                        Some((top, _)) => (top, y),
                        None => (y, y),
                    });
                }
            }
            extent
        }

        /// The editor's row pitch ([`Backend::set_current_line_height`]) is
        /// a host decision, independent of the font's own natural line
        /// height: a 12pt Menlo's ~14px of ascent + descent + leading inside
        /// a VS Code-style 18px row leaves ~4px of slack. Both halves of the
        /// contract that slack is split *evenly* are pinned here:
        ///
        /// - Widening the pitch from the font's natural height to 18px moves
        ///   the body glyph's ink down by half the slack that added — ink
        ///   anchored to the row's top edge instead wouldn't move at all.
        /// - At 18px the gutter number and the body text — the identical
        ///   glyph, `"7"`, hence identical ink — occupy exactly the same
        ///   rows, i.e. they share a baseline.
        ///
        /// Asserting on the *shift* rather than on the ink's absolute
        /// midpoint is deliberate: a digit has no descender, so its ink sits
        /// in the upper part of its glyph cell and its own midpoint is
        /// legitimately above the row's, by a glyph-shape-dependent amount
        /// no centring rule should be read as promising.
        #[test]
        fn body_text_centred_in_row_and_shares_baseline_with_gutter() {
            const ROW_PITCH: f64 = 18.0;
            let metrics = font_metrics(&make_font("Menlo", 12.0).expect("Menlo installed"));
            let natural_pitch = metrics.line_height;
            assert!(
                natural_pitch < ROW_PITCH,
                "fixture needs a font natural line height ({natural_pitch}) shorter \
             than the row pitch ({ROW_PITCH}) to leave any slack to centre in",
            );

            let theme = Theme::default();
            let gutter_chars = 2usize;
            let paint_at_pitch = |pitch: f64| {
                let mut line = one_line("7");
                line.gutter_text = "7".into();
                let mut editor = editor_with_cursor("7", CursorShape::Bar, 99);
                editor.cursor = None; // isolate glyph ink from any cursor paint
                editor.lines = vec![line];
                editor.gutter_char_width = gutter_chars;

                let surface = BitmapSurface::new(W, H);
                surface.fill(
                    theme.background.r as f64 / 255.0,
                    theme.background.g as f64 / 255.0,
                    theme.background.b as f64 / 255.0,
                    1.0,
                );
                let mut backend = MacBackend::new();
                backend.set_current_font(make_font("Menlo", 12.0).expect("Menlo installed"));
                backend.set_current_line_height(pitch);
                backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
                backend.enter_frame_scope(surface.context_ptr(), |b| {
                    crate::DrawsPrimitives::draw_primitive(b, editor.rect, &editor);
                });
                backend.end_frame();
                surface
            };

            let char_w = metrics.char_width;
            let gutter_w = gutter_chars as f64 * char_w;
            let body_ink = |surface: &BitmapSurface, pitch: f64| {
                ink_vertical_extent(
                    surface,
                    gutter_w.round() as u32,
                    (gutter_w + char_w).round() as u32,
                    0,
                    pitch.ceil() as u32,
                    theme.background,
                )
                .expect("body glyph should paint some ink")
            };

            let natural = body_ink(&paint_at_pitch(natural_pitch), natural_pitch);
            let wide_surface = paint_at_pitch(ROW_PITCH);
            let wide = body_ink(&wide_surface, ROW_PITCH);

            let expected_shift = (ROW_PITCH - natural_pitch) / 2.0;
            let top_shift = wide.0 as f64 - natural.0 as f64;
            let bottom_shift = wide.1 as f64 - natural.1 as f64;
            assert!(
                (top_shift - expected_shift).abs() <= 1.0
                    && (bottom_shift - expected_shift).abs() <= 1.0,
                "widening the row pitch from {natural_pitch} to {ROW_PITCH} should move \
             the body glyph down by half the added slack ({expected_shift}px), got \
             top {top_shift}px / bottom {bottom_shift}px (ink {natural:?} -> {wide:?})"
            );

            let gutter = ink_vertical_extent(
                &wide_surface,
                0,
                gutter_w.round() as u32,
                0,
                ROW_PITCH as u32,
                theme.background,
            )
            .expect("gutter glyph should paint some ink");
            assert_eq!(
                gutter, wide,
                "gutter number and body text (same glyph, \"7\") should share \
             a baseline, got gutter={gutter:?}, body={wide:?}"
            );
        }

        // ── Vertical scrollbar ────────────────────────────────────────────────
        //
        // Mirrors `gtk::editor::paint_editor_with_options`'s own scroll-test
        // harness: blank lines, no gutter, explicit `char_width`/`line_height`
        // passed directly to `paint_editor_with_options` (independent of the
        // real font's own metrics, same as `font` is only used for glyph
        // measurement/paint here, not for the geometry grid) — so the only
        // thing in play is the scrollbar-column geometry itself.

        const SCROLL_TEST_W: u32 = 200;
        const SCROLL_TEST_H: u32 = 80;
        const SCROLL_TEST_CHAR_W: f64 = 8.0;
        const SCROLL_TEST_LINE_H: f64 = 16.0;

        fn blank_line(line_idx: usize) -> EditorLine {
            EditorLine {
                raw_text: String::new(),
                gutter_text: String::new(),
                spans: Vec::new(),
                line_idx,
                is_current_line: false,
                is_fold_header: false,
                folded_line_count: 0,
                git_diff: None,
                diff_status: None,
                diagnostics: Vec::new(),
                spell_errors: Vec::new(),
                is_breakpoint: false,
                is_conditional_bp: false,
                is_dap_current: false,
                is_wrap_continuation: false,
                segment_col_offset: 0,
                annotation: None,
                ghost_suffix: None,
                is_ghost_continuation: false,
                indent_guides: Vec::new(),
                colorcolumns: Vec::new(),
            }
        }

        /// Editor fixture at `SCROLL_TEST_W`x`SCROLL_TEST_H` with no gutter,
        /// so the only geometry in play is the scrollbar reservation itself.
        fn scroll_test_editor(total_lines: usize, num_lines: usize) -> Editor {
            Editor {
                id: "editor:scroll".into(),
                rect: QRect::new(0.0, 0.0, SCROLL_TEST_W as f32, SCROLL_TEST_H as f32),
                lines: (0..num_lines).map(blank_line).collect(),
                cursor: None,
                extra_cursors: Vec::new(),
                selection: None,
                extra_selections: Vec::new(),
                yank_highlight: None,
                scroll_top: 0,
                scroll_left: 0,
                total_lines,
                max_col: 0,
                gutter_char_width: 0,
                is_active: true,
                show_active_bg: false,
                has_git_diff: false,
                has_breakpoints: false,
                diagnostic_gutter: HashMap::new(),
                code_action_lines: HashSet::new(),
                bracket_match_positions: Vec::new(),
                active_indent_col: None,
                tabstop: 4,
                cursorline: false,
                lightbulb_glyph: '!',
                paint_options: Default::default(),
            }
        }

        /// Paint `editor` with `options` via [`paint_editor_with_options`]
        /// directly on a fresh headless surface — bypassing `MacBackend`
        /// entirely, the same way `gtk::editor`'s own scroll tests bypass
        /// `GtkBackend` (its trait-level `paint_editor` has no options
        /// parameter; see `EditorPaintOptions`'s doc for why).
        fn scroll_test_paint(editor: &Editor, options: EditorPaintOptions) -> BitmapSurface {
            scroll_test_paint_with_v_scrollbar_w(editor, options, None)
        }

        /// [`scroll_test_paint`], plus a scrollbar-width override via
        /// [`paint_editor_with_options_and_v_scrollbar_w`].
        fn scroll_test_paint_with_v_scrollbar_w(
            editor: &Editor,
            options: EditorPaintOptions,
            v_scrollbar_w: Option<f32>,
        ) -> BitmapSurface {
            let surface = BitmapSurface::new(SCROLL_TEST_W, SCROLL_TEST_H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let f = font();
            let theme = Theme::default();
            unsafe {
                paint_editor_with_options_and_v_scrollbar_w(
                    surface.context_ptr(),
                    &f,
                    editor,
                    &theme,
                    SCROLL_TEST_CHAR_W,
                    SCROLL_TEST_LINE_H,
                    options,
                    v_scrollbar_w,
                );
            }
            surface
        }

        /// A buffer taller than the viewport must tint the reserved
        /// rightmost `cell_width`-wide column; a buffer that fits must not.
        #[test]
        fn draw_editor_paints_vertical_scrollbar_when_buffer_overflows() {
            let bg = Theme::default().background;
            let overflowing = scroll_test_editor(50, 5);
            let surface = scroll_test_paint(&overflowing, EditorPaintOptions::default());
            // Solidly inside the reserved v-scrollbar column (x in [192, 200))
            // and mid-track vertically.
            let (r, g, b, _) = surface.pixel(198, 40);
            assert_ne!(
            (r, g, b),
            (bg.r, bg.g, bg.b),
            "vertical scrollbar track should tint this pixel when total_lines overflows the viewport"
        );

            let fits = scroll_test_editor(3, 3);
            let surface2 = scroll_test_paint(&fits, EditorPaintOptions::default());
            let (r2, g2, b2, _) = surface2.pixel(198, 40);
            assert_eq!(
                (r2, g2, b2),
                (bg.r, bg.g, bg.b),
                "no vertical scrollbar should paint when the buffer fits the viewport"
            );
        }

        /// `EditorPaintOptions::suppress_v_scrollbar` stops the macOS
        /// rasteriser from painting the column too, mirroring GTK's opt-out
        /// for a `Minimap`-as-scrollbar host.
        #[test]
        fn draw_editor_with_options_suppress_v_scrollbar_paints_no_column() {
            let bg = Theme::default().background;
            let overflowing = scroll_test_editor(50, 5);
            let surface = scroll_test_paint(
                &overflowing,
                EditorPaintOptions {
                    suppress_v_scrollbar: true,
                    ..Default::default()
                },
            );
            let (r, g, b, _) = surface.pixel(198, 40);
            assert_eq!(
            (r, g, b),
            (bg.r, bg.g, bg.b),
            "suppress_v_scrollbar should stop the vertical scrollbar from painting even though \
             total_lines overflows the viewport"
        );
        }

        /// A `v_scrollbar_w` override widens the painted column to a
        /// host-chosen pixel width (e.g. VS Code's fixed 14px) instead of
        /// `cell_width`. Viewport is `SCROLL_TEST_W`
        /// = 200, `SCROLL_TEST_CHAR_W` = 8.0: the default column spans
        /// `[192, 200)`; a 14px override spans `[186, 200)`. `x = 188` sits
        /// in the gap between those two spans — plain background at
        /// baseline, inside the track once widened.
        #[test]
        fn draw_editor_v_scrollbar_w_widens_painted_column() {
            let bg = Theme::default().background;
            let overflowing = scroll_test_editor(50, 5);

            let baseline = scroll_test_paint(&overflowing, EditorPaintOptions::default());
            let (r, g, b, _) = baseline.pixel(188, 40);
            assert_eq!(
                (r, g, b),
                (bg.r, bg.g, bg.b),
                "x=188 sits outside the default 8px-wide column, [192, 200)"
            );

            let widened = scroll_test_paint_with_v_scrollbar_w(
                &overflowing,
                EditorPaintOptions::default(),
                Some(14.0),
            );
            let (r2, g2, b2, _) = widened.pixel(188, 40);
            assert_ne!(
            (r2, g2, b2),
            (bg.r, bg.g, bg.b),
            "a Some(14.0) v_scrollbar_w should widen the painted track to cover x=188 ([186, 200))"
        );
        }

        /// Companion to the widened-track test: the same `v_scrollbar_w`
        /// override that widens the painted scrollbar also narrows the text
        /// clip by the same amount, so a full-line background span never
        /// bleeds into the wider reserved column. Uses a real line with a
        /// full-line background span rather than
        /// `scroll_test_editor`'s blank fixture, since clip narrowing is
        /// otherwise invisible with no content to clip.
        #[test]
        fn draw_editor_v_scrollbar_w_narrows_text_clip() {
            let mut editor = scroll_test_editor(50, 5);
            // Spaces, not glyphs: the background fill is what's under test,
            // not anti-aliased ink from a probe landing mid-stroke.
            let text = " ".repeat(40);
            editor.lines[2] = EditorLine {
                spans: vec![ESpan {
                    start_byte: 0,
                    end_byte: text.len(),
                    style: Style {
                        fg: Color::rgb(255, 255, 255),
                        bg: Some(Color::rgb(10, 200, 10)),
                        bold: false,
                        italic: false,
                        font_scale: 1.0,
                    },
                }],
                raw_text: text,
                ..blank_line(2)
            };

            // Baseline: the span's background fills right up to the default
            // 8px-wide column's left edge (x=192), covering x=188.
            let baseline = scroll_test_paint(&editor, EditorPaintOptions::default());
            let (r, g, b, _) = baseline.pixel(188, 40);
            assert_eq!(
                (r, g, b),
                (10, 200, 10),
                "baseline text clip should reach x=188 (span bg), just short of the 8px column"
            );

            // Widened: the text clip now stops 14px short of the right
            // edge, so the same pixel (188) falls outside the (shrunk) text
            // area — it reads as the scrollbar track tint instead.
            let widened = scroll_test_paint_with_v_scrollbar_w(
                &editor,
                EditorPaintOptions::default(),
                Some(14.0),
            );
            let (r2, g2, b2, _) = widened.pixel(188, 40);
            assert_ne!(
            (r2, g2, b2),
            (10, 200, 10),
            "a Some(14.0) v_scrollbar_w should narrow the text clip so x=188 no longer shows span bg"
        );
        }

        /// Paint `editor` with `paint_options.v_scrollbar_w = v_scrollbar_w`
        /// through `MacBackend` — the path a host actually reaches, as
        /// opposed to calling the rasteriser directly.
        fn scroll_test_paint_via_backend(
            editor: &Editor,
            v_scrollbar_w: Option<f32>,
        ) -> BitmapSurface {
            let surface = BitmapSurface::new(SCROLL_TEST_W, SCROLL_TEST_H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = MacBackend::new();
            backend.set_current_font(font());
            let mut editor = editor.clone();
            editor.paint_options.v_scrollbar_w = v_scrollbar_w;
            backend.begin_frame(Viewport::new(
                SCROLL_TEST_W as f32,
                SCROLL_TEST_H as f32,
                1.0,
            ));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                crate::DrawsPrimitives::draw_primitive(b, editor.rect, &editor);
            });
            backend.end_frame();
            surface
        }

        /// Core Graphics anti-aliases a fill's boundary, bleeding a level or
        /// two of track colour into the neighbouring text-area pixel, so an
        /// exact `== background` on a pixel adjacent to the column edge is
        /// not a stable assertion across hosts. The scrollbar track sits
        /// ~20 levels per channel clear of the background in every shipped
        /// theme, far outside this tolerance, so "background" and "track"
        /// stay unambiguous.
        const SCROLL_TEST_AA_TOLERANCE: i32 = 4;

        /// Whether the pixel at `(x, y)` is the editor background rather
        /// than scrollbar track, tolerant of edge anti-aliasing.
        fn scroll_test_pixel_is_bg(surface: &BitmapSurface, x: u32, y: u32) -> bool {
            let bg = Theme::default().background;
            let (r, g, b, _) = surface.pixel(x, y);
            (r as i32 - bg.r as i32).abs() <= SCROLL_TEST_AA_TOLERANCE
                && (g as i32 - bg.g as i32).abs() <= SCROLL_TEST_AA_TOLERANCE
                && (b as i32 - bg.b as i32).abs() <= SCROLL_TEST_AA_TOLERANCE
        }

        /// `paint_options.v_scrollbar_w = Some(14.0)` makes the editor paint
        /// a scrollbar column exactly 14px wide — `[186, 200)` in a 200px
        /// viewport — regardless of the font's own char width, and its
        /// layout reports the same column for hit-testing.
        #[test]
        fn backend_editor_v_scrollbar_w_paints_14px_column() {
            let overflowing = scroll_test_editor(50, 5);
            let surface = scroll_test_paint_via_backend(&overflowing, Some(14.0));
            let y = SCROLL_TEST_H - 4;
            for x in 186..SCROLL_TEST_W {
                assert!(
                    !scroll_test_pixel_is_bg(&surface, x, y),
                    "x={x} lies inside the 14px scrollbar column [186, 200) and should be tinted"
                );
            }
            // Three pixels clear of the column edge at x=186, so edge
            // anti-aliasing cannot reach the probe.
            assert!(
                scroll_test_pixel_is_bg(&surface, 183, y),
                "x=183 lies left of the 14px scrollbar column and should be plain background"
            );

            let mut backend = MacBackend::new();
            backend.set_current_font(font());
            let mut overflowing = overflowing.clone();
            overflowing.paint_options.v_scrollbar_w = Some(14.0);
            let layout =
                crate::DrawsPrimitives::layout_primitive(&backend, overflowing.rect, &overflowing);
            let vsb = layout.v_scrollbar_bounds.expect("buffer overflows");
            assert_eq!(vsb.width, 14.0);
            assert_eq!(vsb.x, SCROLL_TEST_W as f32 - 14.0);
        }

        /// Without `paint_options.v_scrollbar_w`, the backend path keeps a
        /// column one `char_width` wide: narrower than the 14px override, so
        /// the pixels the override would have tinted stay background.
        #[test]
        fn backend_default_editor_v_scrollbar_width_is_one_char() {
            let overflowing = scroll_test_editor(50, 5);

            // The default column is `char_width` wide, which the real font
            // supplies — derive the boundary from the backend rather than
            // assuming a glyph advance.
            let mut backend = MacBackend::new();
            backend.set_current_font(font());
            assert_eq!(overflowing.paint_options.v_scrollbar_w, None);
            let char_w = backend.char_width();
            assert!(
                char_w > 0.0 && char_w < 14.0,
                "fixture needs a default column narrower than the 14px override, got {char_w}"
            );

            let surface = scroll_test_paint_via_backend(&overflowing, None);
            let y = SCROLL_TEST_H - 4;
            // Three pixels clear of the column's left edge, inside what the
            // 14px override would have tinted.
            let text_area_x = (SCROLL_TEST_W as f32 - char_w).floor() as u32 - 3;
            assert!(
                scroll_test_pixel_is_bg(&surface, text_area_x, y),
                "default column is {char_w}px wide, so x={text_area_x} is text-area background"
            );
            assert!(
                !scroll_test_pixel_is_bg(&surface, SCROLL_TEST_W - 2, y),
                "the default column still paints at the right edge"
            );
        }

        /// A host can hand `paint_editor` an `editor.rect` wider than the
        /// real window (e.g. a rounding/DPI mismatch upstream of this
        /// backend). `paint_editor` must still paint the scrollbar track fully
        /// inside the real viewport rather than laying it out past the right
        /// edge, where only a sliver (or nothing at all) would be visible.
        ///
        /// `editor.rect` here is 20px wider than the real 200x80 viewport
        /// `scroll_test_paint_via_backend` paints into. Without the clamp,
        /// the 14px column would land at `x = 0 + 220 - 14 = 206` —
        /// entirely outside the painted surface's `[0, 200)`. The clamp in
        /// `paint_editor` shrinks `editor.rect` back inside `self.viewport`,
        /// so the column lands at `[186, 200)` — the same span the in-bounds
        /// `_paints_14px_column` test above asserts.
        ///
        /// This only covers the paint path. `Backend::editor_layout` (the
        /// no-paint hit-test twin, `src/backend.rs`) is shared, trait-default
        /// code and is not overridden here — its own doc documents that
        /// passing it a `rect` which disagrees with what was actually
        /// painted is an uncovered caller invariant.
        #[test]
        fn backend_clamps_oversized_editor_rect_to_viewport() {
            let mut oversized = scroll_test_editor(50, 5);
            oversized.rect.width = SCROLL_TEST_W as f32 + 20.0;

            let surface = scroll_test_paint_via_backend(&oversized, Some(14.0));
            let y = SCROLL_TEST_H - 4;
            for x in 186..SCROLL_TEST_W {
                assert!(
                    !scroll_test_pixel_is_bg(&surface, x, y),
                    "x={x} lies inside the 14px scrollbar column [186, 200) and should be \
                 tinted — editor.rect ({}px) is wider than the real 200px viewport, so an \
                 unclamped paint_editor would have laid this column out past the window edge",
                    oversized.rect.width
                );
            }
            assert!(
                scroll_test_pixel_is_bg(&surface, 183, y),
                "x=183 lies left of the 14px scrollbar column and should be plain background"
            );
        }
    }
}
