//! The one shared [`PaintSurface`] adapter over a bare
//! `&ID2D1RenderTarget`.
//!
//! This adapter varies on one axis: **`dwrite` presence.**
//! [`crate::Scrollbar`], [`crate::DropOverlay`],
//! [`crate::Split`] and [`crate::SplitTree`] paint with `target` alone
//! (no text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//! [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//! [`crate::ToastOverlay`] paint text too and need a
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
    /// `scale_x == 1.0` it reproduces the plain bold-only override exactly (the `with_horizontal_scale` branch is
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

    fn surface_draw_text_line(
        &mut self,
        origin: Point,
        text: &str,
        runs: &[crate::TextRunStyle],
        _role: crate::FontRole,
    ) {
        self.dwrite_or_panic()
            .draw_text_line(self.target, origin, text, runs);
    }

    fn surface_text_x_at_index(
        &mut self,
        text: &str,
        _role: crate::FontRole,
        byte_idx: usize,
    ) -> f32 {
        self.dwrite_or_panic().text_x_at_index(text, byte_idx)
    }

    fn surface_text_index_at_x(&mut self, text: &str, _role: crate::FontRole, x: f32) -> usize {
        self.dwrite_or_panic().text_index_at_x(text, x)
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

pub(crate) mod image {
    //! Direct2D + WIC rasteriser for [`crate::primitives::image::Image`]
    //!
    //! Decodes `image.source`'s encoded bytes through WIC
    //! (`IWICImagingFactory`/`IWICBitmapDecoder`/`IWICFormatConverter`),
    //! converts to a pixel format the 2D API can paint directly, and
    //! stretches it into `image.layout(rect)`'s resolved target rect.
    //! Collapses to `Unsupported` on any decode failure (missing file,
    //! corrupt bytes, unrecognised format) — see
    //! [`crate::backend::Backend::paint_image`]'s doc comment for why.
    //!
    //! Painting is always clipped to the caller's `rect`, so a `Cover` or
    //! `None` fit never bleeds into whatever the host paints next to it.
    //!
    //! # Decode cache
    //!
    //! An [`ID2D1Bitmap`] is bound to the [`ID2D1RenderTarget`]
    //! `CreateBitmapFromWicBitmap` built it from, so [`draw_image_cached`]'s
    //! cache lives on the backend's surface beside that target and is dropped
    //! with it whenever device-lost recovery rebuilds the target. [`paint_image`]
    //! stays uncached, one decode per call.
    //!
    //! # Why WIC instead of `ID2D1RenderTarget::CreateBitmap` from raw pixels
    //!
    //! `CreateBitmap` takes already-decoded pixels and has no PNG/JPEG decoder
    //! of its own; WIC is Direct2D's designated decode partner
    //! (`CreateBitmapFromWicBitmap`, behind the `windows` crate's
    //! `Win32_Graphics_Imaging` feature).
    //!
    //! # Why COM must be initialized first
    //!
    //! `CoCreateInstance(CLSID_WICImagingFactory, ..)` needs `CoInitializeEx`
    //! on the calling thread first, so this reuses
    //! [`super::super::services::ensure_com_initialized`]'s thread-local guard.
    //!
    //! Only compiled on `target_os = "windows"` — see `super::super::mod`'s
    //! `#[cfg(target_os = "windows")] mod image;` and `backend.rs`'s module
    //! docs for why the rest of this repo's `--features win` compile gate
    //! stays meaningful without a Windows host.

    use windows::core::IUnknown;
    use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
    use windows::Win32::Graphics::Direct2D::{
        ID2D1Bitmap, ID2D1RenderTarget, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    };
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
        WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnLoad,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::SHCreateMemStream;

    use super::super::services::ensure_com_initialized;
    use super::super::text::{pop_clip, push_clip};
    use crate::backend::ImagePaintResult;
    use crate::event::Rect;
    use crate::image_cache::ImageCache;
    use crate::primitives::image::{Image, ImageSource};

    /// Decode `source`'s encoded bytes into an [`ID2D1Bitmap`] bound to
    /// `target`, or `None` on any failure. `Path` and `Bytes` both go through
    /// an in-memory stream (`SHCreateMemStream` + `CreateDecoderFromStream`),
    /// so both are sniffed from content identically, with no MIME hint.
    pub(super) fn decode_bitmap(
        target: &ID2D1RenderTarget,
        source: &ImageSource,
    ) -> Option<ID2D1Bitmap> {
        let bytes: Vec<u8> = match source {
            ImageSource::Path(path) => std::fs::read(path).ok()?,
            ImageSource::Bytes(bytes) => bytes.clone(),
        };

        ensure_com_initialized();
        // SAFETY: COM is initialized on this thread by the call above (see
        // the module doc's "Why COM must be initialized first"). `factory`
        // is a live COM interface this scope owns and releases on drop;
        // every other call below is an ordinary COM call through it or
        // through an interface it returned.
        let factory: IWICImagingFactory = unsafe {
            CoCreateInstance(
                &CLSID_WICImagingFactory,
                None::<&IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
            .ok()?
        };
        // SAFETY: `bytes` is a local `Vec<u8>` that outlives this call;
        // `SHCreateMemStream` copies it into its own backing store rather
        // than borrowing the slice, per the documented API contract.
        let stream = unsafe { SHCreateMemStream(Some(&bytes)) }?;
        // SAFETY: `stream` is the live `IStream` returned above; `null()` for
        // the vendor GUID asks WIC to sniff the format from content.
        let decoder = unsafe {
            factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
        }
        .ok()?;
        // SAFETY: `decoder` is the live `IWICBitmapDecoder` from the call
        // above; frame index 0 is always present for a decoder that
        // succeeded (WIC guarantees at least one frame on success).
        let frame = unsafe { decoder.GetFrame(0) }.ok()?;

        // Direct2D bitmaps want a premultiplied-alpha BGRA buffer; the source
        // frame could be indexed, grayscale, straight-alpha, ... — the format
        // converter normalises whatever WIC decoded to the one format
        // `CreateBitmapFromWicBitmap` is guaranteed to accept.
        // SAFETY: `factory` is still the live interface from above.
        let converter = unsafe { factory.CreateFormatConverter() }.ok()?;
        // SAFETY: `converter` is the interface just returned; `frame` is the
        // live `IWICBitmapFrameDecode` from above and only borrowed for the
        // duration of this synchronous `Initialize` call.
        unsafe {
            converter.Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
        }
        .ok()?;

        // SAFETY: `target` is the caller's live `ID2D1RenderTarget`;
        // `converter` was initialized above into the one pixel format this
        // call is guaranteed to accept.
        unsafe { target.CreateBitmapFromWicBitmap(&converter, None) }.ok()
    }

    /// Paint `image` within `rect` (DIPs, target-relative), honoring
    /// `image.fit`, decoding afresh on every call. See the module docs.
    #[cfg(test)]
    pub fn paint_image(target: &ID2D1RenderTarget, rect: Rect, image: &Image) -> ImagePaintResult {
        draw_image_cached(target, rect, image, &mut ImageCache::with_capacity(1))
    }

    /// [`paint_image`], decoding through `cache` — see the module docs'
    /// "Decode cache". The bitmap is decoded at its own size and stretched at
    /// paint time, so the cache key's size and scale stay constant.
    pub(crate) fn draw_image_cached(
        target: &ID2D1RenderTarget,
        rect: Rect,
        image: &Image,
        cache: &mut ImageCache<ID2D1Bitmap>,
    ) -> ImagePaintResult {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return ImagePaintResult::Unsupported;
        }
        let source = &image.source;
        let Some(bitmap) = cache.get_or_decode(source, 0, 0, 1.0, || decode_bitmap(target, source))
        else {
            return ImagePaintResult::Unsupported;
        };
        let dest = image.layout(rect).bounds;
        let dest_f = D2D_RECT_F {
            left: dest.x,
            top: dest.y,
            right: dest.x + dest.width,
            bottom: dest.y + dest.height,
        };
        push_clip(target, rect);
        // SAFETY: `target` is the caller's live render target, and `bitmap`
        // is bound to it (see the module doc's "Decode cache"); `dest_f` is a
        // stack struct borrowed for the duration of this call.
        unsafe {
            target.DrawBitmap(
                bitmap,
                Some(&dest_f),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
        pop_clip(target);
        ImagePaintResult::Painted
    }
}

/// The terminal's shared pixel painter through [`D2dSurface`] against a
/// headless Direct2D target — cell backgrounds, wide glyphs, overlays,
/// clipping and the split divider.
#[cfg(test)]
mod tests {
    use crate::event::Rect;
    use crate::primitives::terminal::{Terminal, TerminalCell, TerminalCursorShape};
    use crate::terminal_style::wide_glyph_x_scale;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::{fill_rect, DWrite};

    const W: u32 = 200;
    const H: u32 = 120;
    const CHAR_W: f32 = 10.0;
    const LINE_H: f32 = 20.0;

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

    fn dwrite() -> DWrite {
        DWrite::new("Segoe UI", 10.0, None)
            .expect("create DWrite")
            .0
    }

    /// Paints `term` via [`super::D2dSurface`] — the same adapter
    /// `win::multi_section_view`'s embedded `Terminal` section body
    /// uses — so these tests keep controlling `char_width`/`line_height`
    /// explicitly instead of depending on a live `WinBackend`'s measured
    /// font metrics.
    fn paint(surface: &HeadlessSurface, dwrite: &DWrite, term: &Terminal, theme: &Theme) {
        surface
            .paint(|target| {
                let mut raw = super::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::terminal::paint(
                    term, &mut raw, theme, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, None,
                );
            })
            .expect("paint");
    }

    /// A cell's background must paint at its own grid position before
    /// the glyph is drawn — probing a corner the glyph doesn't reach
    /// isolates the fill from font rasterisation.
    #[test]
    fn cell_bg_paints_at_its_grid_position() {
        let magenta = Color::rgb(200, 30, 200);
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('A', Color::rgb(255, 255, 255), magenta)]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (magenta.r, magenta.g, magenta.b));
    }

    /// A double-width glyph (CJK) followed by vt100's blank continuation cell
    /// must have its background span both columns — the continuation cell's own
    /// background must NOT paint over the second half of the wide glyph's cell.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        // The wide glyph's foreground is set equal to its background
        // (magenta on magenta) so antialiased glyph ink can't shift the
        // probed colour — same trick as the GTK/macOS twin tests.
        let row = vec![cell('日', magenta, magenta), cell(' ', magenta, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        // Probe just past the first single-cell-width boundary, still
        // within the wide glyph's two-column span: must be magenta, not
        // cyan.
        let probe_x = (CHAR_W * 1.5) as u32;
        let px = surface.pixel_at(probe_x, 5);
        assert_eq!(
            (px.r, px.g, px.b),
            (magenta.r, magenta.g, magenta.b),
            "wide cell's background should span both columns, not be \
             overpainted by the continuation cell's background"
        );
    }

    /// Mirrors `terminal_style`'s `narrow_wide_glyph_is_stretched_to_fill_two_cells`
    /// — every backend now shares `wide_glyph_x_scale`, so a CJK glyph
    /// measuring 15px in an 18px (2 × 9px) box scales 1.2x here too.
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

    /// Mirrors `terminal_style`'s `exact_fit_wide_glyph_is_not_scaled`.
    #[test]
    fn exact_fit_wide_glyph_is_not_scaled() {
        assert_eq!(wide_glyph_x_scale(18.0, 18.0), 1.0);
    }

    /// Mirrors `terminal_style`'s `over_wide_glyph_is_shrunk_into_box`.
    #[test]
    fn over_wide_glyph_is_shrunk_into_box() {
        let scale = wide_glyph_x_scale(24.0, 18.0);
        assert!(
            scale < 1.0,
            "24px glyph in 18px box should shrink, got {scale}"
        );
        assert!((24.0 * scale - 18.0).abs() < 1e-9);
    }

    /// Companion regression, mirroring the `gtk`/`macos` surface tests'
    /// `narrow_cells_advance_by_single_char_width`: ordinary narrow
    /// (single-width) cells must still advance by exactly `char_width` —
    /// the wide-cell fix must not widen unrelated cells.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        let white = Color::rgb(255, 255, 255);
        let row = vec![cell('A', white, magenta), cell('B', cyan, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let probe_x = (CHAR_W * 1.5) as u32;
        let px = surface.pixel_at(probe_x, 5);
        assert_eq!(
            (px.r, px.g, px.b),
            (cyan.r, cyan.g, cyan.b),
            "narrow cells must still advance by exactly char_width"
        );
    }

    /// The cursor overlay swaps fg/bg: the cell background paints in
    /// the cell's *foreground* colour.
    #[test]
    fn cursor_cell_inverts_fg_bg() {
        let fg = Color::rgb(10, 220, 30);
        let bg = Color::rgb(40, 40, 40);
        let mut c = cell('X', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (fg.r, fg.g, fg.b));
    }

    /// A find-active match paints its cell background orange —
    /// matching the GTK/macOS overlay convention.
    #[test]
    fn find_active_cell_paints_orange() {
        let mut c = cell('z', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.is_find_active = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (255, 165, 0));
    }

    /// A selected cell uses `theme.selection_bg`.
    #[test]
    fn selected_cell_uses_theme_selection_bg() {
        let mut c = cell('x', Color::rgb(255, 255, 255), Color::rgb(10, 10, 10));
        c.selected = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            )
        );
    }

    /// Rows past the pane bottom must be clipped, not bled — the
    /// interactive-resize regression `gtk`/`macos` already guard
    #[test]
    fn rows_past_the_pane_bottom_are_clipped() {
        let magenta = Color::rgb(200, 30, 200);
        let bg_sentinel = Color::rgb(1, 2, 3);
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
        let pane_h = 60.0_f32;
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                let _ = fill_rect(target, Rect::new(0.0, 0.0, W as f32, H as f32), bg_sentinel);
                let mut raw = super::D2dSurface {
                    target,
                    dwrite: Some(&dwrite),
                };
                crate::primitives::terminal::paint(
                    &term, &mut raw, &theme, 0.0, 0.0, W as f32, pane_h, LINE_H, CHAR_W, None,
                );
            })
            .expect("paint");

        let inside = surface.pixel_at(4, 4);
        assert_eq!(
            (inside.r, inside.g, inside.b),
            (magenta.r, magenta.g, magenta.b)
        );

        let below = surface.pixel_at(4, H - 4);
        assert_eq!(
            (below.r, below.g, below.b),
            (bg_sentinel.r, bg_sentinel.g, bg_sentinel.b),
            "rows below the pane bottom must be clipped, not bled"
        );
    }

    /// A bold cell paints via `DWrite::draw_text_styled` rather than
    /// silently dropping the attribute — smoke-tested by confirming the
    /// paint call succeeds and the cell's own bg still lands correctly
    /// (glyph shape/weight isn't independently probable via pixel
    /// colour without OCR, so this only guards the plumbing).
    #[test]
    fn bold_cell_paints_without_error() {
        let mut c = cell('B', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.bold = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (20, 20, 20));
    }

    /// [`crate::primitives::terminal::paint_divider`] paints a 1-DIP
    /// line at `x`.
    #[test]
    fn divider_paints_a_one_dip_line() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let theme = Theme::default();
        surface
            .paint(|target| {
                let _ = fill_rect(
                    target,
                    Rect::new(0.0, 0.0, W as f32, H as f32),
                    Color::rgb(0, 0, 0),
                );
                let dwrite = dwrite();
                let mut raw = super::D2dSurface {
                    target,
                    dwrite: Some(&dwrite),
                };
                crate::primitives::terminal::paint_divider(&mut raw, 50.0, 0.0, H as f32, &theme);
            })
            .expect("paint");

        let on_line = surface.pixel_at(50, 10);
        let off_line = surface.pixel_at(52, 10);
        assert_eq!(
            (on_line.r, on_line.g, on_line.b),
            (theme.separator.r, theme.separator.g, theme.separator.b)
        );
        assert_eq!((off_line.r, off_line.g, off_line.b), (0, 0, 0));
    }
}

/// Form and settings-chrome paint through [`D2dSurface`] on a headless
/// Direct2D target.
#[cfg(test)]
mod form_paint_tests {
    use super::D2dSurface;
    use crate::primitives::form::{FieldKind, Form, FormHit, FormLayout, ToggleGroupItem};
    use crate::theme::Theme;
    use crate::types::{Color, StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;
    use crate::Rect;

    const W: f32 = 300.0;
    const H: f32 = 200.0;
    const LINE_HEIGHT: f32 = 14.0;

    /// The pixel form layout at `line_height`, measured through `dwrite`
    /// — the geometry a pixel form paints from.
    fn win_form_layout(dwrite: &DWrite, rect: Rect, form: &Form, line_height: f32) -> FormLayout {
        let row_h = crate::primitives::layout_metrics::form_row_height(line_height as f64);
        form.layout(rect.width, rect.height, |i| {
            crate::primitives::layout_metrics::form_field_measure(&form.fields[i], row_h, dwrite)
        })
    }

    /// Paint the settings chrome through the shared
    /// [`crate::compose::sidebar_panel_body::paint_settings_chrome`] on a
    /// [`D2dSurface`] over `target`.
    #[allow(clippy::too_many_arguments)]
    fn draw_settings_chrome(
        target: &windows::Win32::Graphics::Direct2D::ID2D1RenderTarget,
        dwrite: &DWrite,
        rect: Rect,
        line_height: f32,
        header_text: &str,
        query: &str,
        placeholder: &str,
        active: bool,
    ) {
        let mut raw = D2dSurface {
            target,
            dwrite: Some(dwrite),
        };
        crate::compose::sidebar_panel_body::paint_settings_chrome(
            &mut raw,
            &Theme::default(),
            rect,
            line_height,
            header_text,
            query,
            placeholder,
            active,
        );
    }

    /// Paint `form` via the shared [`crate::primitives::form::paint`]
    /// through a [`D2dSurface`] over `surface`'s headless target —
    /// the same adapter `win::multi_section_view`'s embedded-`Form` body uses.
    fn paint(surface: &HeadlessSurface, dwrite: &DWrite, rect: Rect, form: &Form) -> FormLayout {
        let layout = win_form_layout(dwrite, rect, form, LINE_HEIGHT);
        let theme = Theme::default();
        let origin = crate::Point::new(rect.x, rect.y);
        surface
            .paint(|target| {
                let mut raw = D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::form::paint(form, &layout, &mut raw, &theme, origin);
            })
            .expect("paint form");
        layout
    }

    fn field(id: &str, label: &str, kind: FieldKind) -> crate::primitives::form::FormField {
        crate::primitives::form::FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label.to_string()),
            kind,
            hint: StyledText::plain(""),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn make_form(fields: Vec<crate::primitives::form::FormField>) -> Form {
        Form {
            id: WidgetId::new("form"),
            fields,
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    /// Paint↔click round trip across a mix of field kinds, including a
    /// `ToggleGroup` whose per-item hit regions must land on the
    /// correct toggle id.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let form = make_form(vec![
            field(
                "name",
                "Name",
                FieldKind::TextInput {
                    value: "quadraui".into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    selection_anchor: None,
                },
            ),
            field(
                "flags",
                "Flags",
                FieldKind::ToggleGroup {
                    toggles: vec![
                        ToggleGroupItem {
                            id: WidgetId::new("flags:a"),
                            label: "A".into(),
                            value: true,
                        },
                        ToggleGroupItem {
                            id: WidgetId::new("flags:b"),
                            label: "B".into(),
                            value: false,
                        },
                    ],
                },
            ),
        ]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &form);

        assert_eq!(layout.visible_fields.len(), 2);
        for vf in &layout.visible_fields {
            let cx = vf.bounds.x + 1.0;
            let cy = vf.bounds.y + vf.bounds.height / 2.0;
            assert_eq!(layout.hit_test(cx, cy), FormHit::Field(vf.id));
        }

        let toggle_field = &layout.visible_fields[1];
        assert_eq!(toggle_field.item_bounds.len(), 2);
        for (id, item_rect) in &toggle_field.item_bounds {
            let cx = item_rect.x + item_rect.width / 2.0;
            let cy = item_rect.y + item_rect.height / 2.0;
            assert_eq!(layout.hit_test(cx, cy), FormHit::Field(*id));
        }
    }

    /// The focused row paints `selected_bg` at its own bounds.
    #[test]
    fn focused_row_paints_selected_bg() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut form = make_form(vec![
            field("a", "A", FieldKind::Toggle { value: false }),
            field("b", "B", FieldKind::Toggle { value: true }),
        ]);
        form.focused_field = Some(WidgetId::new("b"));
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &form);

        let theme = Theme::default();
        let bounds = layout.visible_fields[1].bounds;
        let px = surface.pixel_at((bounds.x + 1.0) as u32, (bounds.y + 1.0) as u32);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            )
        );
    }

    /// Both `paint` and `win_form_layout` consume the same layout computation,
    /// ensuring they cannot disagree on geometry. This is a smoke test that
    /// painting a `TextInput` + `PasswordInput` pair (with cursors set) doesn't panic.
    #[test]
    fn paint_with_cursors_does_not_panic() {
        let form = make_form(vec![
            field(
                "name",
                "Name",
                FieldKind::TextInput {
                    value: "hi".into(),
                    placeholder: String::new(),
                    cursor: None,
                    selection_anchor: None,
                },
            ),
            field(
                "pw",
                "Password",
                FieldKind::PasswordInput {
                    value: "secret".into(),
                    placeholder: String::new(),
                    cursor: Some(3),
                    mask_char: '*',
                },
            ),
        ]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let _ = paint(&surface, &dwrite, rect, &form);
    }

    /// [`draw_settings_chrome`]'s two rows must paint at the shared
    /// painter's geometry: row 0 (`y in [0, line_height)`) is `header_bg`, row 1
    /// (`y in [line_height, 2*line_height)`) is `tab_bar_bg` when
    /// `active` is `false`.
    #[test]
    fn settings_chrome_paints_header_and_inactive_search_rows() {
        let surface = HeadlessSurface::new(200, 60).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, 200.0, 60.0);
        let line_height = 18.0_f32;

        surface
            .paint(|target| {
                draw_settings_chrome(
                    target,
                    &dwrite,
                    rect,
                    line_height,
                    "Settings",
                    "",
                    "search…",
                    false,
                );
            })
            .expect("paint settings chrome");

        let theme = Theme::default();
        let header_px = surface.pixel_at(150, 2);
        assert_eq!(
            (header_px.r, header_px.g, header_px.b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
            "header row paints header_bg"
        );

        let search_px = surface.pixel_at(150, line_height as u32 + 2);
        assert_eq!(
            (search_px.r, search_px.g, search_px.b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "inactive search row paints tab_bar_bg"
        );
    }

    /// The search row switches to `selected_bg` while `active`.
    #[test]
    fn settings_chrome_active_search_row_paints_selected_bg() {
        let surface = HeadlessSurface::new(200, 60).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, 200.0, 60.0);
        let line_height = 18.0_f32;

        surface
            .paint(|target| {
                draw_settings_chrome(
                    target,
                    &dwrite,
                    rect,
                    line_height,
                    "Settings",
                    "op",
                    "",
                    true,
                );
            })
            .expect("paint settings chrome");

        let theme = Theme::default();
        let search_px = surface.pixel_at(150, line_height as u32 + 2);
        assert_eq!(
            (search_px.r, search_px.g, search_px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "active search row paints selected_bg"
        );
    }

    /// A `rect.height` of exactly one `line_height` (the shape
    /// [`crate::compose::sidebar_panel_body::SidebarPanelChrome::Header`]
    /// reserves) must not paint the search row — only the header row paints.
    #[test]
    fn settings_chrome_one_row_height_paints_no_search_row() {
        let surface = HeadlessSurface::new(200, 60).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let line_height = 18.0_f32;
        // Sentinel distinct from every colour this fn itself paints
        // (header_bg/tab_bar_bg/selected_bg), so a search-row pixel
        // reading the sentinel unambiguously means "nothing painted
        // here".
        let sentinel = Color::rgb(1, 2, 3);
        surface
            .fill_rect(Rect::new(0.0, 0.0, 200.0, 60.0), sentinel)
            .expect("fill sentinel");

        let one_row = Rect::new(0.0, 0.0, 200.0, line_height);
        surface
            .paint(|target| {
                draw_settings_chrome(
                    target,
                    &dwrite,
                    one_row,
                    line_height,
                    "HEADER",
                    "",
                    "placeholder",
                    false,
                );
            })
            .expect("paint settings chrome");

        let theme = Theme::default();
        let header_px = surface.pixel_at(150, 2);
        assert_eq!(
            (header_px.r, header_px.g, header_px.b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
            "the header row must still paint even when the search row doesn't"
        );

        let search_px = surface.pixel_at(150, line_height as u32 + 2);
        assert_eq!(
            (search_px.r, search_px.g, search_px.b),
            (sentinel.r, sentinel.g, sentinel.b),
            "a 1-row-tall chrome rect must not paint a search row past its own height"
        );
    }
}

/// Palette paint through [`D2dSurface`] on a headless Direct2D target.
#[cfg(test)]
mod palette_paint_tests {
    use super::D2dSurface;
    use crate::primitives::palette::{
        native_surface_paint, pixel_layout, Palette, PaletteLayout, PaletteMode,
    };
    use crate::theme::Theme;
    use crate::types::{Color, StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;
    use crate::Rect;
    use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

    /// The pixel palette layout at `line_height` — the geometry the
    /// palette paints from.
    fn win_palette_layout(rect: Rect, palette: &Palette, line_height: f32) -> PaletteLayout {
        pixel_layout(rect.width, rect.height, palette, line_height).0
    }

    /// Paint `palette` into `rect` through the shared pixel painter on a
    /// [`D2dSurface`] over `target`, with `Theme::default()`.
    fn draw_palette(
        target: &ID2D1RenderTarget,
        dwrite: &DWrite,
        rect: Rect,
        palette: &Palette,
        line_height: f32,
        nerd_fonts_enabled: bool,
    ) {
        let (palette_layout, rows_h) = pixel_layout(rect.width, rect.height, palette, line_height);
        let mut surface = D2dSurface {
            target,
            dwrite: Some(dwrite),
        };
        native_surface_paint::paint(
            palette,
            rect,
            &palette_layout,
            rows_h,
            line_height,
            nerd_fonts_enabled,
            &mut surface,
            &Theme::default(),
            &crate::style::Style::default(),
            crate::primitives::context_menu::PointerTint::default(),
        );
    }

    fn item(text: &str, match_positions: Vec<usize>) -> crate::primitives::palette::PaletteItem {
        crate::primitives::palette::PaletteItem {
            text: StyledText::plain(text),
            detail: None,
            icon: None,
            match_positions,
            depth: 0,
            expandable: false,
            expanded: false,
        }
    }

    fn palette() -> Palette {
        Palette {
            id: WidgetId::new("pal"),
            title: "Commands".into(),
            query: "op".into(),
            query_cursor: 2,
            items: vec![item("open file", vec![0, 1]), item("close", vec![])],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 0,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::List,
        }
    }

    #[test]
    fn renders_and_highlights_match_positions() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let first_item = layout.visible_items[0].bounds;

        let theme = Theme::default();
        // Scan the whole first-item row box for match-highlighted ink —
        // matched runs ('o', 'p' at byte offsets 0, 1 of "open file")
        // paint in `match_fg`, warm and far from every other colour on
        // that row. Two deliberate weakenings versus an exact-equality
        // probe at a guessed coordinate: the scan covers the row's full
        // height (real DirectWrite glyph placement inside the row box
        // isn't predictable ahead of a live Windows font pass), and a
        // pixel counts when it is *nearer* `match_fg` than the row's
        // other two colours rather than exactly equal to it, since
        // glyph edges are antialiased blends over the selection fill.
        let dist2 = |px: Color, c: Color| {
            let dr = px.r as i32 - c.r as i32;
            let dg = px.g as i32 - c.g as i32;
            let db = px.b as i32 - c.b as i32;
            dr * dr + dg * dg + db * db
        };
        let found = (first_item.y as u32..(first_item.y + first_item.height) as u32).any(|y| {
            (first_item.x as u32..(first_item.x + first_item.width) as u32).any(|x| {
                let px = surface.pixel_at(x, y);
                let d_match = dist2(px, theme.match_fg);
                d_match < dist2(px, theme.surface_fg) && d_match < dist2(px, theme.selected_bg)
            })
        });
        assert!(
            found,
            "expected to find match_fg-coloured pixels on the matched item's row"
        );
    }

    /// The shared `paint` gives every GUI backend, including Windows, a
    /// thin bar caret (not a full-glyph-width block) in `theme.cursor`.
    #[test]
    fn query_row_paints_a_cursor_bar() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let qb = layout.query_bounds.expect("query bounds present");
        let theme = Theme::default();
        // A cursor bar is a *solid fill* spanning the row's full height —
        // unlike glyph ink (which never covers a whole text-row column
        // solidly, since real fonts leave leading above ascenders and
        // below descenders) — but only ~2px wide, not a full-glyph-width
        // block. Scan for the column with the highest fraction of
        // cursor-near pixels and require it to be nearly fully covered,
        // and require most columns in the query row to NOT be covered
        // (ruling out a block spanning the whole glyph).
        //
        // `draw_palette` always paints with `Theme::default()` (it takes
        // no theme parameter — see that fn's doc), whose dark-mode
        // `cursor` and `query_fg` happen to be the same `rgb(220, 220,
        // 220)`, so probing `theme.cursor` below doesn't by itself
        // distinguish a caret from glyph ink; `fully_covered_cols <= 4`
        // (a bar is ~2px wide, a full-glyph block is not) is the
        // assertion that actually tells them apart.
        let dist2 = |px: Color, c: Color| {
            let dr = px.r as i32 - c.r as i32;
            let dg = px.g as i32 - c.g as i32;
            let db = px.b as i32 - c.b as i32;
            dr * dr + dg * dg + db * db
        };
        let rows: Vec<u32> = (qb.y as u32..(qb.y + qb.height) as u32).collect();
        let coverage: Vec<f32> = (qb.x as u32..(qb.x + qb.width) as u32)
            .map(|x| {
                let covered = rows
                    .iter()
                    .filter(|&&y| dist2(surface.pixel_at(x, y), theme.cursor) < 400)
                    .count();
                covered as f32 / rows.len().max(1) as f32
            })
            .collect();
        let best_coverage = coverage.iter().cloned().fold(0.0_f32, f32::max);
        let fully_covered_cols = coverage.iter().filter(|&&c| c > 0.9).count();
        assert!(
            best_coverage > 0.9,
            "expected a full-row-height cursor bar; best column coverage was {best_coverage}"
        );
        assert!(
            fully_covered_cols <= 4,
            "expected a thin ~2px bar caret, not a full-glyph-width block; \
             {fully_covered_cols} columns were fully covered"
        );
    }

    /// The query row height uses a formula that reserves space for a
    /// separator pixel at the bottom: `line_height + 1.0`. The separator
    /// stroke sits in the query band's last pixel, and the first item row
    /// moves down by 1.0 pixel accordingly.
    ///
    /// Paints through the real Direct2D `HeadlessSurface` and pins all
    /// three rows: the reserved separator pixel carries border ink, the
    /// query text row above it does not, and the first item row below it
    /// carries the selected row's fill — i.e. the separator neither
    /// climbs into the text row nor steals (and get overpainted by) the
    /// first item row.
    #[test]
    fn separator_paints_at_corrected_row_not_drifted_one() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0_f32;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let theme = Theme::default();
        let layout = win_palette_layout(rect, &p, line_height);
        let qb = layout.query_bounds.expect("query bounds present");
        assert_eq!(
            qb.height,
            line_height + 1.0,
            "query band must reserve line_height + 1.0 (the +1 is the separator)",
        );

        let title_h = line_height; // native_surface_paint::layout: title_h = line_height
        let sep_y = (qb.y + qb.height - 1.0) as u32; // corrected: the reserved pixel
        let first_row_y = (qb.y + qb.height) as u32; // This was sep_y
        assert_eq!(sep_y, (title_h + line_height) as u32);
        assert_eq!(first_row_y, (title_h + line_height + 1.0) as u32);
        assert_eq!(
            layout.visible_items[0].bounds.y,
            qb.y + qb.height,
            "first item row starts immediately below the query band",
        );

        // `p`'s row 0 is the selected one, so the first item row paints
        // `selected_bg` — the fill that must not overwrite the separator.
        let probe_x = 150_u32;
        let is_border = |y: u32| {
            let px = surface.pixel_at(probe_x, y);
            (px.r, px.g, px.b) == (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b)
        };
        assert!(
            is_border(sep_y),
            "expected separator ink in the query band's reserved pixel, row {sep_y}",
        );
        assert!(
            !is_border(sep_y - 1),
            "separator climbed into the query *text* row {} — it belongs in the \
             single pixel query_bounds reserves for it",
            sep_y - 1,
        );
        let first_row_px = surface.pixel_at(probe_x, first_row_y);
        assert_eq!(
            (first_row_px.r, first_row_px.g, first_row_px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "row {first_row_y} is the first item row (selected), not the separator's",
        );
    }

    /// The item list flooring: when available item-list height is not an
    /// exact multiple of the resolved row height, the row count is floored
    /// and a reduced `viewport_height` is used to prevent partial-height
    /// last rows. The rect height (200) and `line_height` (18) are chosen
    /// so the available item area (159px after title/query/bottom-inset)
    /// is not an exact multiple of the row pitch — the condition that
    /// produces a partial last row without flooring.
    #[test]
    fn item_list_never_shows_a_partial_last_row() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut p = palette();
        p.items = (0..40)
            .map(|i| item(&format!("item {i}"), vec![]))
            .collect();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0_f32;
        let row_height = native_surface_paint::item_row_height(line_height);

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let last = layout
            .visible_items
            .last()
            .expect("some items should be visible");
        assert_eq!(
            last.bounds.height, row_height,
            "last visible row must be full height — a partial row means the \
             unfloored-viewport drift regressed",
        );

        // Driver-tier: paint through the real Direct2D backend and
        // confirm no ink bleeds into the row immediately below the
        // floored boundary — a partial 9th row would have painted glyph
        // ink there.
        let theme = Theme::default();
        let boundary_y = (last.bounds.y + last.bounds.height + 1.0) as u32;
        let probe_x = (last.bounds.x + 4.0) as u32;
        let px = surface.pixel_at(probe_x, boundary_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
            "expected plain surface_bg just below the floored last row, not a \
             partially-clipped row's content",
        );
    }

    #[test]
    fn selected_row_paints_selection_bg() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let first_item = layout.visible_items[0].bounds;
        let sel = Theme::default().selected_bg;
        let px = surface.pixel_at(
            (first_item.x + first_item.width - 2.0) as u32,
            (first_item.y + 1.0) as u32,
        );
        assert_eq!((px.r, px.g, px.b), (sel.r, sel.g, sel.b));
    }
}

#[cfg(test)]
mod text_line_tests {
    use super::D2dSurface;
    use crate::paint_surface::PaintSurface;
    use crate::types::Color;
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;
    use crate::{FontRole, Point, TextRunStyle};

    /// A line with a wide glyph, a combining mark and mixed bold/italic.
    const LINE: &str = "a日e\u{301}bc";

    fn dwrite() -> DWrite {
        DWrite::new("Consolas", 12.0, None)
            .expect("create DWrite")
            .0
    }

    #[test]
    fn text_x_and_index_agree_with_directwrite_advances() {
        let dwrite = dwrite();
        let surface = HeadlessSurface::new(10, 10).expect("create surface");
        surface
            .paint(|target| {
                let mut raw = D2dSurface {
                    target,
                    dwrite: Some(&dwrite),
                };
                for idx in [0usize, 1, 4, 7, 8, 9] {
                    let expect = dwrite.measure_text(&LINE[..idx]).unwrap().0;
                    let x = raw.surface_text_x_at_index(LINE, FontRole::Editor, idx);
                    assert!(
                        (x - expect).abs() < 0.01,
                        "x at byte {idx}: {x} vs {expect}"
                    );
                    assert_eq!(raw.surface_text_index_at_x(LINE, FontRole::Editor, x), idx);
                }
                // A trailing space still advances the next glyph.
                let one = raw.surface_text_x_at_index("a  b", FontRole::Editor, 1);
                let three = raw.surface_text_x_at_index("a  b", FontRole::Editor, 3);
                assert!(three > one, "{three} vs {one}");
            })
            .expect("paint");
    }

    #[test]
    fn draw_text_line_paints_only_its_runs() {
        let dwrite = dwrite();
        let fg = Color::rgb(250, 250, 250);
        let runs = [
            TextRunStyle::new(0..1, fg),
            TextRunStyle::new(1..4, Color::rgb(220, 60, 60)).with_bold(true),
            TextRunStyle::new(4..7, fg).with_italic(true),
            TextRunStyle::new(7..9, fg),
        ];
        let lit = |runs: &[TextRunStyle]| {
            let surface = HeadlessSurface::new(120, 30).expect("create surface");
            surface
                .paint(|target| {
                    let mut raw = D2dSurface {
                        target,
                        dwrite: Some(&dwrite),
                    };
                    raw.surface_draw_text_line(Point::new(2.0, 2.0), LINE, runs, FontRole::Editor);
                })
                .expect("paint");
            (0..120)
                .flat_map(|x| (0..30).map(move |y| (x, y)))
                .filter(|&(x, y)| surface.pixel_at(x, y).r > 128)
                .count()
        };
        assert!(lit(&runs) > 0);
        assert_eq!(lit(&[]), 0);
    }
}

/// The `minimap` widget painted through this backend's adapter
/// surface, as the pixel tests below drive it.
#[cfg(test)]
mod minimap_pixel_tests {
    #![allow(dead_code)]

    use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

    use super::super::surface::D2dSurface;
    use super::super::text::DWrite;
    use crate::event::Rect;
    use crate::primitives::minimap::paint_pixels::{self, Glyphs};
    use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapScale};
    use crate::theme::Theme;

    /// The DIP layout of `minimap` in `rect` without painting, at
    /// [`MinimapScale::One`]. `rect.x`/`rect.y` are baked into every bound.
    pub fn win_minimap_layout(minimap: &Minimap, rect: Rect) -> MinimapLayout {
        win_minimap_layout_scaled(minimap, rect, MinimapScale::One)
    }

    /// [`win_minimap_layout`] at an explicit [`MinimapScale`] — see
    /// [`Minimap::pixel_layout`].
    pub(crate) fn win_minimap_layout_scaled(
        minimap: &Minimap,
        rect: Rect,
        scale: MinimapScale,
    ) -> MinimapLayout {
        minimap.pixel_layout(rect, scale, None)
    }

    /// Draw `minimap` into `rect` (DIPs, target-relative) on `target` at
    /// [`MinimapScale::One`]. Returns the layout for host click dispatch
    /// (`layout.hit_test(x, y)`).
    pub fn paint_minimap(
        target: &ID2D1RenderTarget,
        dwrite: &DWrite,
        rect: Rect,
        minimap: &Minimap,
        theme: &Theme,
    ) -> MinimapLayout {
        draw_minimap_scaled(target, dwrite, rect, minimap, theme, MinimapScale::One)
    }

    /// [`paint_minimap`] at an explicit [`MinimapScale`].
    pub(crate) fn draw_minimap_scaled(
        target: &ID2D1RenderTarget,
        dwrite: &DWrite,
        rect: Rect,
        minimap: &Minimap,
        theme: &Theme,
        scale: MinimapScale,
    ) -> MinimapLayout {
        let layout = win_minimap_layout_scaled(minimap, rect, scale);
        let mut surface = D2dSurface {
            target,
            dwrite: Some(dwrite),
        };
        paint_pixels::paint(
            minimap,
            &layout,
            &mut surface,
            theme,
            scale,
            Glyphs::Atlas { dpi_scale: 1.0 },
        );
        layout
    }

    mod tests {
        use super::*;
        use crate::primitives::minimap::{MinimapHit, MinimapLine, ROW_PITCH_PX};
        use crate::types::{Color, WidgetId};
        use crate::win::testing::HeadlessSurface;

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

        /// C0 smoke: `paint_minimap` must actually paint pixels and return a
        /// click-routable layout. The default fixed pitch stays below the legibility
        /// floor, so this exercises `ColumnBlocks`, not `DrawText`.
        #[test]
        fn draw_minimap_paints_column_blocks_and_returns_layout() {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let theme = Theme {
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                ..Theme::default()
            };
            let mm = minimap_from(vec!["fn main() {}"; 8], 8);
            let rect = Rect::new(0.0, 0.0, W, H);

            let layout = surface
                .paint(|target| {
                    paint_minimap(target, &dwrite, rect, &mm, &theme);
                })
                .map(|_| win_minimap_layout(&mm, rect))
                .expect("paint minimap");

            assert!(!layout.visible_lines.is_empty());

            let mut painted_any = false;
            for x in 0..W as u32 {
                for y in 0..(ROW_PITCH_PX as u32 * 8).max(8) {
                    let px = surface.pixel_at(x, y);
                    if (px.r, px.g, px.b) != (255, 255, 255) {
                        painted_any = true;
                    }
                }
            }
            assert!(
                painted_any,
                "expected paint_minimap to paint visible blocks"
            );
        }

        /// Paint↔click round trip at a non-zero origin — regression guard for
        /// LOCAL/ABSOLUTE mixup, mirrored from `win::board`/`win::diff_view`'s tests.
        #[test]
        fn paint_and_click_round_trip_at_nonzero_origin() {
            let origin_x = 7.0_f32;
            let origin_y = 13.0_f32;
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let theme = Theme::default();
            let mm = minimap_from(vec!["x"; 8], 8);
            let rect = Rect::new(origin_x, origin_y, 40.0, 100.0);

            let layout = surface
                .paint(|target| {
                    paint_minimap(target, &dwrite, rect, &mm, &theme);
                })
                .map(|_| win_minimap_layout(&mm, rect))
                .expect("paint minimap");

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
        /// painted — same contract every other `win::` rasteriser's
        /// `no_paint_layout_matches_paint_layout` test proves.
        #[test]
        fn no_paint_layout_matches_paint_layout() {
            let mm = minimap_from(vec!["fn main() {}"; 8], 8);
            let rect = Rect::new(0.0, 0.0, W, H);
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

            let painted = surface
                .paint(|target| {
                    paint_minimap(target, &dwrite, rect, &mm, &Theme::default());
                })
                .map(|_| win_minimap_layout(&mm, rect))
                .expect("paint");
            let no_paint = win_minimap_layout(&mm, rect);
            assert_eq!(painted, no_paint);
        }

        /// Zero-size rect is a no-op — mirrors every other `win::` rasteriser's
        /// same guard.
        #[test]
        fn zero_size_rect_is_a_no_op() {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let theme = Theme {
                background: Color::rgb(255, 255, 255),
                ..Theme::default()
            };
            let mm = minimap_from(vec!["fn main() {}"; 8], 8);
            let rect = Rect::new(0.0, 0.0, 0.0, H);

            surface
                .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
                .expect("fill background");

            surface
                .paint(|target| {
                    paint_minimap(target, &dwrite, rect, &mm, &theme);
                })
                .expect("paint minimap");

            let px = surface.pixel_at(1, 1);
            assert_eq!(
                (px.r, px.g, px.b),
                (255, 255, 255),
                "a zero-width minimap should paint nothing at all",
            );
        }

        /// The fixed pitch stays independent of both the strip height and the
        /// file length — same parity guarantee `gtk::minimap`'s tests pin, now
        /// proven on Win-GUI's own `win_minimap_layout`.
        #[test]
        fn row_pitch_is_independent_of_strip_height_and_file_length() {
            let short = minimap_from(vec!["fn main() {}"; 3], 3);
            let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
            let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
            let long = minimap_from(long_refs, 10_000);

            let tall = win_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 800.0));
            let short_strip = win_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 16.0));
            let long_layout = win_minimap_layout(&long, Rect::new(0.0, 0.0, 40.0, 400.0));

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

        /// At [`MinimapScale::Two`] each non-blank column paints a 2-DIP-wide
        /// block, and a blank column stays unpainted.
        #[test]
        fn scale_two_paints_two_dip_column_blocks() {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let theme = Theme {
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                ..Theme::default()
            };
            let mm = minimap_from(vec!["fn main() {}"], 1);
            let rect = Rect::new(0.0, 0.0, W, H);
            surface
                .paint(|target| {
                    draw_minimap_scaled(target, &dwrite, rect, &mm, &theme, MinimapScale::Two);
                })
                .expect("paint minimap");
            for x in 0..4u32 {
                let px = surface.pixel_at(x, 1);
                assert_eq!((px.r, px.g, px.b), (0, 0, 0), "'fn' column at x={x}");
            }
            for x in 4..6u32 {
                let px = surface.pixel_at(x, 1);
                assert_eq!((px.r, px.g, px.b), (255, 255, 255), "blank column at x={x}");
            }
        }
    }
}

#[cfg(test)]
mod image_tests {
    use super::image::*;
    use crate::backend::ImagePaintResult;
    use crate::event::Rect;
    use crate::primitives::image::ImageFit;
    use crate::primitives::image::{Image, ImageSource};
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 200;

    /// A tiny solid-colour PNG, generated at test time rather than
    /// checked in as a binary fixture — same approach
    /// `gtk::image::tests::tiny_png_bytes` takes, just built with the
    /// `png` crate's minimal writer instead of `gdk_pixbuf`.
    fn tiny_png_bytes() -> Vec<u8> {
        // Hand-rolled minimal 4x4 opaque-red PNG: a real decoder (WIC)
        // must parse this from scratch, so this is not a stand-in for a
        // "loads bytes" no-op — it exercises the actual decode path.
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

        // IDAT: 4 scanlines, each a filter byte (0 = None) + 4 RGB pixels
        // of opaque red, zlib-wrapped with stored (uncompressed) blocks.
        let mut raw = Vec::new();
        for _ in 0..4 {
            raw.push(0u8);
            for _ in 0..4 {
                raw.extend_from_slice(&[0xff, 0x00, 0x00]);
            }
        }
        let idat = zlib_store(&raw);
        chunk(&mut png, b"IDAT", &idat);

        chunk(&mut png, b"IEND", &[]);
        png
    }

    /// Zlib container with a single stored (uncompressed) deflate block —
    /// enough for WIC's PNG decoder to accept, no compression needed for
    /// a 4x4 fixture.
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

    /// The impl this replaced was a genuine `todo!()` — every image paint
    /// panicked.
    #[test]
    fn draw_image_from_bytes_paints_real_pixels() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        surface
            .fill_rect(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                Theme::default().background,
            )
            .expect("fill bg");

        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Fill);
        let rect = Rect::new(20.0, 20.0, 60.0, 60.0);
        let mut result = ImagePaintResult::Unsupported;
        surface
            .paint(|target| {
                result = paint_image(target, rect, &img);
            })
            .expect("paint image");

        assert_eq!(result, ImagePaintResult::Painted);
        let c = surface.pixel_at(50, 50);
        assert_eq!((c.r, c.g, c.b), (255, 0, 0), "decoded PNG should be red");

        // Outside the target rect: untouched.
        let outside = surface.pixel_at(5, 5);
        let bg = Theme::default().background;
        assert_eq!((outside.r, outside.g, outside.b), (bg.r, bg.g, bg.b));
    }

    #[test]
    fn draw_image_from_missing_path_reports_unsupported() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let img = image(
            ImageSource::Path("/nonexistent/does-not-exist.png".into()),
            ImageFit::Contain,
        );
        let mut result = ImagePaintResult::Painted;
        surface
            .paint(|target| {
                result = paint_image(target, Rect::new(0.0, 0.0, 40.0, 40.0), &img);
            })
            .expect("paint attempt");
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_from_corrupt_bytes_reports_unsupported() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let img = image(ImageSource::Bytes(vec![0, 1, 2, 3]), ImageFit::Contain);
        let mut result = ImagePaintResult::Painted;
        surface
            .paint(|target| {
                result = paint_image(target, Rect::new(0.0, 0.0, 40.0, 40.0), &img);
            })
            .expect("paint attempt");
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn zero_size_rect_reports_unsupported_without_decoding() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Contain);
        let mut result = ImagePaintResult::Painted;
        surface
            .paint(|target| {
                result = paint_image(target, Rect::new(0.0, 0.0, 0.0, 0.0), &img);
            })
            .expect("paint attempt");
        assert_eq!(result, ImagePaintResult::Unsupported);
    }
}

/// The `editor` widget painted through this backend's adapter
/// surface, as the pixel tests below drive it.
#[cfg(test)]
mod editor_pixel_tests {
    #![allow(dead_code)]

    use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

    use super::super::surface::D2dSurface;
    use super::super::text::DWrite;
    use crate::primitives::editor::{paint_pixels, Editor, EditorPaintOptions};
    use crate::theme::Theme;

    /// Draw an [`Editor`] viewport (`editor.rect`) on `target`, at
    /// `cell_width` / `line_height` (DIPs).
    ///
    /// Equivalent to [`paint_editor_with_options_and_v_scrollbar_w`] with
    /// `EditorPaintOptions::default()` and no scrollbar-width override.
    pub fn paint_editor(
        target: &ID2D1RenderTarget,
        dwrite: &DWrite,
        editor: &Editor,
        cell_width: f32,
        line_height: f32,
        theme: &Theme,
    ) {
        paint_editor_with_options_and_v_scrollbar_w(
            target,
            dwrite,
            editor,
            cell_width,
            line_height,
            theme,
            EditorPaintOptions::default(),
            None,
        )
    }

    /// [`paint_editor`], plus [`EditorPaintOptions`] and a host-settable
    /// vertical scrollbar width in DIPs (`None` = one `cell_width`). Both
    /// feed [`Editor::layout_with_options_and_v_scrollbar_w`], and the
    /// shared painter paints the vertical scrollbar column the layout
    /// reserves, as the production widget-contract path does.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_editor_with_options_and_v_scrollbar_w(
        target: &ID2D1RenderTarget,
        dwrite: &DWrite,
        editor: &Editor,
        cell_width: f32,
        line_height: f32,
        theme: &Theme,
        options: EditorPaintOptions,
        v_scrollbar_w: Option<f32>,
    ) {
        let geom = editor.layout_with_options_and_v_scrollbar_w(
            editor.rect,
            cell_width,
            line_height,
            options,
            v_scrollbar_w,
        );
        let mut surface = D2dSurface {
            target,
            dwrite: Some(dwrite),
        };
        paint_pixels::paint_with_options(editor, &geom, &mut surface, theme, options);
    }

    mod tests {
        use super::*;
        use crate::event::Rect as QRect;
        use crate::primitives::editor::{
            CursorPos, CursorShape, DiagnosticMark, DiagnosticSeverity, DiffLine, EditorCursor,
            EditorLine, EditorSelection, EditorStyledSpan, SelectionKind, Style,
        };
        use crate::types::Color;
        use crate::types::WidgetId;
        use crate::win::testing::HeadlessSurface;

        const CELL_W: f32 = 8.0;
        const LINE_H: f32 = 16.0;

        /// Whether `px` is within 2 of `c` on every channel — a translucent
        /// fill's alpha is quantised to 8 bits before Direct2D blends it.
        fn near(px: Color, c: Color) -> bool {
            [(px.r, c.r), (px.g, c.g), (px.b, c.b)]
                .iter()
                .all(|&(a, b)| (a as i32 - b as i32).abs() <= 2)
        }

        fn plain_line(idx: usize, text: &str) -> EditorLine {
            EditorLine {
                raw_text: text.to_string(),
                gutter_text: format!("{:>3}", idx + 1),
                spans: vec![EditorStyledSpan {
                    start_byte: 0,
                    end_byte: text.len(),
                    style: Style {
                        fg: Color::rgb(200, 200, 200),
                        bg: None,
                        bold: false,
                        italic: false,
                        font_scale: 1.0,
                    },
                }],
                line_idx: idx,
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

        fn editor(lines: Vec<EditorLine>) -> Editor {
            Editor {
                id: WidgetId::new("editor"),
                rect: QRect::new(0.0, 0.0, 200.0, 100.0),
                lines,
                cursor: None,
                extra_cursors: Vec::new(),
                selection: None,
                extra_selections: Vec::new(),
                yank_highlight: None,
                scroll_top: 0,
                scroll_left: 0,
                total_lines: 3,
                max_col: 20,
                gutter_char_width: 4,
                is_active: true,
                show_active_bg: false,
                has_git_diff: false,
                has_breakpoints: false,
                diagnostic_gutter: Default::default(),
                code_action_lines: Default::default(),
                bracket_match_positions: Vec::new(),
                active_indent_col: None,
                tabstop: 4,
                cursorline: true,
                lightbulb_glyph: '!',
                paint_options: Default::default(),
            }
        }

        /// The `v_scrollbar_w` override narrows `text_bounds` by exactly
        /// its delta from `cell_width` — the layout
        /// `paint_editor_with_options_and_v_scrollbar_w` clips text to. Pure
        /// layout, no `HeadlessSurface`/`DWrite` needed.
        #[test]
        fn v_scrollbar_w_option_narrows_text_bounds() {
            let mut e = editor(vec![plain_line(0, "line one")]);
            e.total_lines = 50; // overflow: far more than the viewport's rows
            let rect = e.rect;

            let baseline = e.layout(rect, CELL_W, LINE_H);
            let baseline_vsb = baseline
                .v_scrollbar_bounds
                .expect("overflowing buffer reserves a vertical scrollbar column");
            assert_eq!(baseline_vsb.width, CELL_W);

            let widened = e.layout_with_options_and_v_scrollbar_w(
                rect,
                CELL_W,
                LINE_H,
                EditorPaintOptions::default(),
                Some(14.0),
            );
            let widened_vsb = widened
                .v_scrollbar_bounds
                .expect("still overflowing with the override applied");
            assert_eq!(widened_vsb.width, 14.0);
            assert_eq!(
                widened.text_bounds.width,
                baseline.text_bounds.width - (14.0 - CELL_W)
            );
        }

        /// Painting must not panic across a mix of plain lines, a cursor, a
        /// selection, and a diagnostic underline, and the gutter's line
        /// number must be visible pixels distinct from the background.
        #[test]
        fn paints_without_panicking_and_gutter_is_visible() {
            let surface = HeadlessSurface::new(200, 100).expect("create surface");
            let (dwrite, _, _) = DWrite::new("Consolas", 10.0, None).expect("create DWrite");
            let mut lines = vec![
                plain_line(0, "fn main() {"),
                plain_line(1, "    ok();"),
                plain_line(2, "}"),
            ];
            lines[0].is_current_line = true;
            lines[1].diagnostics.push(DiagnosticMark {
                start_col: 4,
                end_col: 6,
                severity: DiagnosticSeverity::Error,
                message: "unused".into(),
            });
            let mut e = editor(lines);
            e.cursor = Some(EditorCursor {
                pos: CursorPos {
                    view_line: 0,
                    col: 3,
                },
                shape: CursorShape::Bar,
            });
            e.selection = Some(EditorSelection {
                kind: SelectionKind::Char,
                start_line: 1,
                start_col: 0,
                end_line: 1,
                end_col: 4,
            });

            surface
                .paint(|target| {
                    paint_editor(target, &dwrite, &e, CELL_W, LINE_H, &Theme::default());
                })
                .expect("paint editor");

            let theme = Theme::default();
            // Current-line gutter number is painted in `line_number_active_fg`,
            // distinct from the plain background. Scan the whole gutter
            // column (0..gutter width, the full first row's height) rather
            // than a guessed glyph position — real DirectWrite glyph
            // placement within that box isn't known ahead of a real
            // Windows font metrics pass.
            let gutter_w = (4.0 * CELL_W) as u32;
            let found = (0..gutter_w).any(|x| {
                (0..LINE_H as u32).any(|y| {
                    let px = surface.pixel_at(x, y);
                    (px.r, px.g, px.b)
                        == (
                            theme.line_number_active_fg.r,
                            theme.line_number_active_fg.g,
                            theme.line_number_active_fg.b,
                        )
                })
            });
            assert!(
            found,
            "current line's gutter number should paint line_number_active_fg somewhere in the gutter column"
        );
        }

        /// A `Line`-kind selection covers the full row width regardless of
        /// `start_col`/`end_col`.
        #[test]
        fn line_selection_spans_full_row() {
            let surface = HeadlessSurface::new(200, 100).expect("create surface");
            let (dwrite, _, _) = DWrite::new("Consolas", 10.0, None).expect("create DWrite");
            // 2 visible characters + 23 trailing spaces: `line_len` (25,
            // matching the selection's full-row width) is unaffected by the
            // trailing spaces, but space glyphs paint no ink — so the
            // sample point below lands on pure selection-tint background,
            // not real DirectWrite glyph rendering this test has no way to
            // predict pixel-for-pixel without a live Windows font pass.
            let text = format!("ab{}", " ".repeat(23));
            let lines = vec![plain_line(0, &text)];
            let mut e = editor(lines);
            e.selection = Some(EditorSelection {
                kind: SelectionKind::Line,
                start_line: 0,
                start_col: 2,
                end_line: 0,
                end_col: 3,
            });

            surface
                .paint(|target| {
                    paint_editor(target, &dwrite, &e, CELL_W, LINE_H, &Theme::default());
                })
                .expect("paint editor");

            let theme = Theme::default();
            let text_x = (e.gutter_char_width as f32 * CELL_W) as u32;
            // Sample near the right edge of the viewport, well past
            // `end_col`'s column — a `Line` selection should still cover it.
            let sample_x = 180u32.max(text_x + 5);
            let px = surface.pixel_at(sample_x, 4);
            let expected = theme
                .background
                .blend(theme.selection, theme.selection_alpha as f64);
            assert!(near(px, expected), "{px:?} vs {expected:?}");
        }

        /// A `Line` selection on a horizontally scrolled editor starts
        /// left of the text area, but the gutter keeps its background:
        /// the selection overlay is clipped to the text columns.
        #[test]
        fn line_selection_leaves_gutter_untouched_when_scrolled_left() {
            let surface = HeadlessSurface::new(200, 100).expect("create surface");
            let (dwrite, _, _) = DWrite::new("Consolas", 10.0, None).expect("create DWrite");
            let text = format!("ab{}", " ".repeat(23));
            let mut e = editor(vec![plain_line(0, &text)]);
            e.scroll_left = 4;
            e.selection = Some(EditorSelection {
                kind: SelectionKind::Line,
                start_line: 0,
                start_col: 0,
                end_line: 0,
                end_col: 0,
            });

            surface
                .paint(|target| {
                    paint_editor(target, &dwrite, &e, CELL_W, LINE_H, &Theme::default());
                })
                .expect("paint editor");

            let theme = Theme::default();
            // Top-left of the gutter: left of the right-aligned line number.
            let px = surface.pixel_at(1, 4);
            assert!(
                near(px, theme.background),
                "gutter pixel {px:?} should stay {:?}",
                theme.background
            );
            let text_x = (e.gutter_char_width as f32 * CELL_W) as u32;
            let px = surface.pixel_at(180u32.max(text_x + 5), 4);
            let expected = theme
                .background
                .blend(theme.selection, theme.selection_alpha as f64);
            assert!(near(px, expected), "{px:?} vs {expected:?}");
        }

        /// `EditorSelection::cols_on` (shared with every other backend) correctly
        /// computes selection columns, accounting for wrapped ghosts, padding rows,
        /// and segment offsets. This proves it through a real Direct2D paint.
        #[test]
        fn selection_respects_wrapped_ghost_and_diff_padding_rows() {
            let surface = HeadlessSurface::new(200, 100).expect("create surface");
            let (dwrite, _, _) = DWrite::new("Consolas", 10.0, None).expect("create DWrite");

            let lines = vec![
                // Row 0: buffer line 0, plain, selection's own start line.
                // Visible glyphs only in the first two columns, spaces after
                // — the sample column below lands on inkless space cells so
                // the pixel is pure selection tint, not DirectWrite glyph
                // antialiasing (same trick as `line_selection_spans_full_row`).
                plain_line(0, &format!("01{}", " ".repeat(8))),
                // Row 1: a wrap-continuation segment of buffer line 1,
                // starting at buffer column 5 — its own text (10 chars,
                // same glyph-then-spaces shape) is entirely inside the
                // selection.
                EditorLine {
                    segment_col_offset: 5,
                    is_wrap_continuation: true,
                    ..plain_line(1, &format!("ab{}", " ".repeat(8)))
                },
                // Row 2: the AI-ghost continuation of buffer line 1 — same
                // `line_idx` as row 1, but virtual, must never be selected.
                EditorLine {
                    is_ghost_continuation: true,
                    // Two glyphs, so the sample column stays inkless.
                    ghost_suffix: Some("ai".into()),
                    ..plain_line(1, "")
                },
                // Row 3: buffer line 2 (the selection's own end line), but a
                // diff-padding filler row — must never be selected either.
                EditorLine {
                    diff_status: Some(DiffLine::Padding),
                    ..plain_line(2, "")
                },
            ];
            let mut e = editor(lines);
            e.total_lines = 4;
            e.selection = Some(EditorSelection {
                kind: SelectionKind::Char,
                start_line: 0,
                start_col: 0,
                end_line: 2,
                end_col: 10,
            });

            surface
                .paint(|target| {
                    paint_editor(target, &dwrite, &e, CELL_W, LINE_H, &Theme::default());
                })
                .expect("paint editor");

            let theme = Theme::default();
            let blended = theme
                .background
                .blend(theme.selection, theme.selection_alpha as f64);
            let text_x = (e.gutter_char_width as f32 * CELL_W) as i32;
            // Middle of column 5: past the two visible glyphs, inside
            // every tinted row's selected range.
            let sample_x = (text_x as f32 + 5.0 * CELL_W + CELL_W / 2.0) as u32;
            let row_y = |row: i32| (row as f32 * LINE_H + LINE_H / 2.0) as u32;

            let row0 = surface.pixel_at(sample_x, row_y(0));
            assert!(
                near(row0, blended),
                "row 0 (plain, selection's start line) should be tinted: {row0:?}"
            );
            let row1 = surface.pixel_at(sample_x, row_y(1));
            assert!(
                near(row1, blended),
                "row 1 (wrapped segment of a fully-selected line) should be tinted: {row1:?}"
            );
            let row2 = surface.pixel_at(sample_x, row_y(2));
            assert_eq!(
                (row2.r, row2.g, row2.b),
                (theme.background.r, theme.background.g, theme.background.b),
                "row 2 (ghost continuation) must never be tinted by a selection"
            );
            // The diff-padding row shows its own diff background, untinted.
            let row3 = surface.pixel_at(sample_x, row_y(3));
            assert_eq!(
                (row3.r, row3.g, row3.b),
                (
                    theme.diff_padding_bg.r,
                    theme.diff_padding_bg.g,
                    theme.diff_padding_bg.b
                ),
                "row 3 (diff padding) must never be tinted by a selection"
            );
        }

        /// A block caret is a translucent `theme.cursor` cell over the glyph:
        /// the cell's inkless pixels carry the tint, and the glyph's ink still
        /// shows through it.
        #[test]
        fn block_cursor_tints_its_cell_and_keeps_the_glyph_visible() {
            let surface = HeadlessSurface::new(200, 100).expect("create surface");
            let (dwrite, _, _) = DWrite::new("Consolas", 10.0, None).expect("create DWrite");
            let lines = vec![plain_line(0, "1789518172522")];
            let mut e = editor(lines);
            e.cursor = Some(EditorCursor {
                pos: CursorPos {
                    view_line: 0,
                    col: 0,
                },
                shape: CursorShape::Block,
            });

            surface
                .paint(|target| {
                    paint_editor(target, &dwrite, &e, CELL_W, LINE_H, &Theme::default());
                })
                .expect("paint editor");

            let theme = Theme::default();
            let tint = theme
                .background
                .blend(theme.cursor, theme.cursor_normal_alpha as f64);
            let text_x = (e.gutter_char_width as f32 * CELL_W) as u32;
            let cell: Vec<Color> = (text_x + 1..text_x + 6)
                .flat_map(|x| (1..LINE_H as u32 - 1).map(move |y| (x, y)))
                .map(|(x, y)| surface.pixel_at(x, y))
                .collect();
            assert!(
                cell.iter().any(|&px| near(px, tint)),
                "the caret cell should carry the translucent cursor tint {tint:?}"
            );
            assert!(
                cell.iter().any(|&px| !near(px, tint)),
                "the glyph's ink should still show through the caret"
            );
        }

        /// The editor's row pitch is a host decision independent of
        /// `dwrite`'s own natural line height — mirrors
        /// `macos::editor::tests::body_text_centred_in_row_and_shares_baseline_with_gutter`
        /// and `primitives::editor::paint_pixels`'s GTK `draw_editor_body_text_centred_in_row_and_shares_baseline_with_gutter`:
        /// widening the pitch from the font's natural height to a VS
        /// Code-style wider row should move the body glyph's ink down by
        /// half the added slack, and the gutter number (same glyph, `"7"`)
        /// should land on the same rows as the body text once widened.
        #[test]
        fn body_text_centred_in_row_and_shares_baseline_with_gutter() {
            let (dwrite, natural_line_height, _) =
                DWrite::new("Consolas", 10.0, None).expect("create DWrite");
            const ROW_PITCH: f32 = 24.0;
            assert!(
                natural_line_height < ROW_PITCH,
                "fixture needs a font natural line height ({natural_line_height}) shorter than \
             the row pitch ({ROW_PITCH}) to leave any slack to centre in",
            );

            let gutter_chars = 2usize;
            let gutter_w = gutter_chars as f32 * CELL_W;
            let theme = Theme::default();

            let paint_at_pitch = |pitch: f32| -> HeadlessSurface {
                let surface =
                    HeadlessSurface::new(200, pitch.ceil() as u32 + 8).expect("create surface");
                let mut line = plain_line(0, "7");
                line.gutter_text = "7".into();
                let mut e = editor(vec![line]);
                e.gutter_char_width = gutter_chars;
                e.rect = QRect::new(0.0, 0.0, 200.0, pitch + 8.0);
                surface
                    .paint(|target| {
                        paint_editor(target, &dwrite, &e, CELL_W, pitch, &theme);
                    })
                    .expect("paint editor");
                surface
            };

            // Scan `surface` for the vertical extent of non-background ink
            // inside `[x_start, x_end) × [0, y_end)` — Win-GUI's counterpart
            // to the macOS/GTK twins' identically-named test helpers.
            let ink_extent = |surface: &HeadlessSurface, x_start: u32, x_end: u32, y_end: u32| {
                const TOL: i32 = 24;
                let mut extent: Option<(u32, u32)> = None;
                for y in 0..y_end {
                    let has_ink = (x_start..x_end).any(|x| {
                        let px = surface.pixel_at(x, y);
                        (px.r as i32 - theme.background.r as i32).abs() > TOL
                            || (px.g as i32 - theme.background.g as i32).abs() > TOL
                            || (px.b as i32 - theme.background.b as i32).abs() > TOL
                    });
                    if has_ink {
                        extent = Some(match extent {
                            Some((top, _)) => (top, y),
                            None => (y, y),
                        });
                    }
                }
                extent
            };

            let natural_surface = paint_at_pitch(natural_line_height);
            let natural = ink_extent(
                &natural_surface,
                gutter_w.round() as u32,
                (gutter_w + CELL_W).round() as u32,
                natural_line_height.ceil() as u32 + 8,
            )
            .expect("body glyph should paint some ink at the natural pitch");

            let wide_surface = paint_at_pitch(ROW_PITCH);
            let wide = ink_extent(
                &wide_surface,
                gutter_w.round() as u32,
                (gutter_w + CELL_W).round() as u32,
                ROW_PITCH as u32 + 8,
            )
            .expect("body glyph should paint some ink at the wide pitch");

            let expected_shift = (ROW_PITCH - natural_line_height) / 2.0;
            let top_shift = wide.0 as f32 - natural.0 as f32;
            let bottom_shift = wide.1 as f32 - natural.1 as f32;
            assert!(
                (top_shift - expected_shift).abs() <= 2.0
                    && (bottom_shift - expected_shift).abs() <= 2.0,
                "widening the row pitch from {natural_line_height} to {ROW_PITCH} should move \
             the body glyph down by half the added slack ({expected_shift}px), got top \
             {top_shift}px / bottom {bottom_shift}px (ink {natural:?} -> {wide:?})"
            );

            let gutter = ink_extent(
                &wide_surface,
                0,
                gutter_w.round() as u32,
                ROW_PITCH as u32 + 8,
            )
            .expect("gutter glyph should paint some ink");
            assert_eq!(
                gutter, wide,
                "gutter number and body text (same glyph, \"7\") should share a baseline, got \
             gutter={gutter:?}, body={wide:?}"
            );
        }
    }
}
