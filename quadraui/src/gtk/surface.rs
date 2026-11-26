//! The shared [`PaintSurface`] adapter over a bare Cairo context.
//! Every primitive that uses this adapter is listed below, grouped by
//! two characteristics that vary per-primitive:
//!
//! 1. **`layout` presence.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!    [`crate::Split`] and [`crate::SplitTree`] paint with `cr` alone (no
//!    text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//!    [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//!    [`crate::ToastOverlay`] and [`crate::TextInput`]
//!    paint text too and need a live `pango::Layout`.
//!    [`CairoSurface::layout`] is `Option` so one struct covers both —
//!    [`Self::layout_or_panic`] panics only if a text verb is reached with
//!    `layout: None`.
//! 2. **Fill translucency.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!    `gtk::status_bar`, `gtk::diff_view` and `gtk::toast`
//!    fill with `set_source_rgba` (honouring `color.a`); [`crate::Form`],
//!    [`crate::Split`], [`crate::SplitTree`] and
//!    [`crate::TextInput`] fill with `set_source` (opaque-only).
//!    [`CairoSurface::translucent_fill`] carries that choice per call site.
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
use crate::paint_surface::{AlphaSheet, PaintSurface, TextRunStyle};
use crate::{Color, Image, Point, Rect, Viewport};

/// Adapts a live `pango::Layout` (falling back to a `char_width`-based
/// estimate when none is available, e.g. from a layout-only call between
/// paint frames) to the shared
/// [`crate::primitives::layout_metrics::TextMeasure`] trait, so a shared
/// layout formula never has to name a Pango type and every GTK caller
/// measures through the exact same adapter its paint uses.
pub(crate) struct PangoMeasure<'a> {
    pub(crate) pango_layout: Option<&'a pango::Layout>,
    pub(crate) char_width: f64,
}

impl crate::primitives::layout_metrics::TextMeasure for PangoMeasure<'_> {
    fn width_of(&self, text: &str) -> f32 {
        if let Some(pl) = self.pango_layout {
            pl.set_text(text);
            pl.pixel_size().0.max(0) as f32
        } else {
            (text.chars().count() as f64 * self.char_width).ceil() as f32
        }
    }
}

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

/// Pango attributes for `runs` over `text`: each run's colour, weight,
/// style and size scale on its byte range.
fn line_attrs(text: &str, runs: &[TextRunStyle]) -> pango::AttrList {
    let attrs = pango::AttrList::new();
    for run in runs {
        let Some(range) = run.byte_range_in(text) else {
            continue;
        };
        let (start, end) = (range.start as u32, range.end as u32);
        let insert = |mut attr: pango::Attribute| {
            attr.set_start_index(start);
            attr.set_end_index(end);
            attrs.insert(attr);
        };
        let c = run.fg;
        insert(
            pango::AttrColor::new_foreground(
                u16::from(c.r) * 257,
                u16::from(c.g) * 257,
                u16::from(c.b) * 257,
            )
            .into(),
        );
        if run.bold {
            insert(pango::AttrInt::new_weight(pango::Weight::Bold).into());
        }
        if run.italic {
            insert(pango::AttrInt::new_style(pango::Style::Italic).into());
        }
        if run.is_scaled() {
            insert(pango::AttrFloat::new_scale(f64::from(run.scale)).into());
        }
    }
    attrs
}

/// `text` as one Pango line on `layout`, drawn with its top-left at
/// `origin`, with `runs`' attributes applied. The cairo source is cleared
/// first, so bytes no run covers paint nothing.
pub(crate) fn draw_text_line(
    cr: &Context,
    layout: &pango::Layout,
    origin: Point,
    text: &str,
    runs: &[TextRunStyle],
) {
    layout.set_text(text);
    layout.set_attributes(Some(&line_attrs(text, runs)));
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
    cr.move_to(origin.x as f64, origin.y as f64);
    super::painted_text::show_layout(cr, layout);
    layout.set_attributes(None);
}

/// Advance of `text[..byte_idx]` on `layout` with `runs`' attributes
/// applied, in pixels.
pub(crate) fn text_x_at_index(
    layout: &pango::Layout,
    text: &str,
    runs: &[TextRunStyle],
    byte_idx: usize,
) -> f32 {
    layout.set_text(text);
    layout.set_attributes(Some(&line_attrs(text, runs)));
    let idx = crate::text_util::snap_to_char_boundary(text, byte_idx);
    let x = layout.index_to_pos(idx as i32).x();
    layout.set_attributes(None);
    (x as f64 / pango::SCALE as f64) as f32
}

/// The grapheme boundary of `text` on `layout`, with `runs`' attributes
/// applied, nearest to `x` pixels from the line start: Pango's hit
/// index, moved past the cluster when `x` falls in its trailing half.
pub(crate) fn text_index_at_x(
    layout: &pango::Layout,
    text: &str,
    runs: &[TextRunStyle],
    x: f32,
) -> usize {
    layout.set_text(text);
    layout.set_attributes(Some(&line_attrs(text, runs)));
    let x_pango = (f64::from(x.max(0.0)) * pango::SCALE as f64).round() as i32;
    let (_inside, index, trailing) = layout.xy_to_index(x_pango, 0);
    layout.set_attributes(None);
    let mut idx = crate::text_util::snap_to_char_boundary(text, index.max(0) as usize);
    for _ in 0..trailing.max(0) {
        idx = crate::text_util::next_char_boundary(text, idx);
    }
    idx.min(text.len())
}

/// A rendered sheet with the `(family, chars, cell size)` it was rendered for.
type CachedSheet = (String, String, (u32, u32), AlphaSheet);

thread_local! {
    /// The last sheet [`glyph_alpha_sheet`] rendered, so a repeated request
    /// shapes nothing.
    static GLYPH_SHEET: std::cell::RefCell<Option<CachedSheet>> =
        const { std::cell::RefCell::new(None) };
}

/// [`PaintSurface::surface_glyph_alpha_sheet`] for Pango: each char of
/// `chars` shaped bold in `family`, sized to `cell`'s height, painted
/// white onto a transparent `ARgb32` sheet whose alpha channel is the
/// coverage.
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
    use gtk4::cairo::{Format, ImageSurface};
    let count = chars.chars().count() as u32;
    let (sheet_w, sheet_h) = (cell_w * count, cell_h);
    if sheet_w == 0 || sheet_h == 0 {
        return None;
    }
    let surface = ImageSurface::create(Format::ARgb32, sheet_w as i32, sheet_h as i32).ok()?;
    let cr = Context::new(&surface).ok()?;
    let pango_layout = pangocairo::functions::create_layout(&cr);
    let mut font = pango::FontDescription::new();
    font.set_family(family);
    font.set_weight(pango::Weight::Bold);
    font.set_absolute_size(f64::from(cell_h) * 0.85 * pango::SCALE as f64);
    pango_layout.set_font_description(Some(&font));
    cr.set_source_rgb(1.0, 1.0, 1.0);
    for (i, ch) in chars.chars().enumerate() {
        pango_layout.set_text(&ch.to_string());
        cr.move_to(f64::from(i as u32 * cell_w), 0.0);
        // The plain `show_layout`, not the recording wrapper: this paints
        // a private offscreen sheet, which must not reach the frame's
        // painted-text sink.
        pangocairo::functions::show_layout(&cr, &pango_layout);
    }
    // `data()` needs the only reference to the surface.
    drop(pango_layout);
    drop(cr);
    surface.flush();
    let stride = surface.stride() as usize;
    let mut surface = surface;
    let data = surface.data().ok()?;
    let (w, h) = (sheet_w as usize, sheet_h as usize);
    let mut alpha = vec![0u8; w * h];
    for row in 0..h {
        for col in 0..w {
            // Cairo's `ARgb32` is [B, G, R, A] in memory.
            alpha[row * w + col] = data[row * stride + col * 4 + 3];
        }
    }
    Some(AlphaSheet::new(sheet_w, sheet_h, alpha))
}

/// [`PaintSurface::surface_blit_alpha`] for Cairo: `alpha` as an `A8`
/// mask, scaled onto `dest`, painting `color` through it.
pub(crate) fn blit_alpha(
    cr: &Context,
    dest: Rect,
    alpha: &[u8],
    width: u32,
    height: u32,
    color: Color,
) {
    use gtk4::cairo::{Format, ImageSurface};
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || alpha.len() < w * h {
        return;
    }
    let Ok(mut mask) = ImageSurface::create(Format::A8, width as i32, height as i32) else {
        return;
    };
    {
        let stride = mask.stride() as usize;
        let Ok(mut data) = mask.data() else {
            return;
        };
        for row in 0..h {
            data[row * stride..row * stride + w].copy_from_slice(&alpha[row * w..(row + 1) * w]);
        }
    }
    mask.flush();
    let (r, g, b) = super::cairo_rgb(color);
    cr.save().ok();
    cr.translate(f64::from(dest.x), f64::from(dest.y));
    cr.scale(
        f64::from(dest.width) / f64::from(width),
        f64::from(dest.height) / f64::from(height),
    );
    cr.set_source_rgb(r, g, b);
    cr.mask_surface(&mask, 0.0, 0.0).ok();
    cr.restore().ok();
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

    fn surface_draw_text_line(
        &mut self,
        origin: Point,
        text: &str,
        runs: &[TextRunStyle],
        _role: crate::FontRole,
    ) {
        draw_text_line(self.cr, self.layout_or_panic(), origin, text, runs);
    }

    fn surface_text_x_at_index(
        &mut self,
        text: &str,
        runs: &[TextRunStyle],
        _role: crate::FontRole,
        byte_idx: usize,
    ) -> f32 {
        text_x_at_index(self.layout_or_panic(), text, runs, byte_idx)
    }

    fn surface_text_index_at_x(
        &mut self,
        text: &str,
        runs: &[TextRunStyle],
        _role: crate::FontRole,
        x: f32,
    ) -> usize {
        text_index_at_x(self.layout_or_panic(), text, runs, x)
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

    fn surface_blit_alpha(
        &mut self,
        dest: Rect,
        alpha: &[u8],
        width: u32,
        height: u32,
        color: Color,
    ) {
        blit_alpha(self.cr, dest, alpha, width, height, color);
    }

    /// Shapes in the adapter's own layout's font family (`Monospace` when
    /// the layout names none); `role` is not consulted, as for every
    /// other text verb here.
    fn surface_glyph_alpha_sheet(
        &mut self,
        chars: &str,
        _role: crate::FontRole,
        cell: crate::primitive::Size,
    ) -> Option<AlphaSheet> {
        let family = self
            .layout?
            .font_description()
            .and_then(|f| f.family())
            .map(|g| g.to_string())
            .unwrap_or_else(|| "Monospace".to_string());
        glyph_alpha_sheet(&family, chars, cell)
    }

    fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
        ImagePaintResult::Unsupported
    }
}

pub(crate) mod image {
    //! GTK rasteriser for [`crate::Image`]: decodes `image.source` via
    //! `gdk_pixbuf` and paints it into a [`Context`] through
    //! [`gdk::prelude::GdkCairoContextExt::set_source_pixbuf`], honoring
    //! `image.fit`'s already-resolved target rect ([`Image::layout`]).
    //!
    //! Painting is always clipped to the caller's `(x, y, w, h)` rect — the
    //! only [`ImageFit`] variants that can extend past it are `Cover` and
    //! `None`, and neither should bleed into whatever the host paints next
    //! to it.
    //!
    //! # Decode cache
    //!
    //! [`paint_image`] is `pub` (re-exported as `quadraui::gtk::paint_image`).
    //! To keep its signature stable, decoding and scaling are cached in the
    //! sibling crate-internal [`draw_image_cached`] that
    //! the backend's `PaintSurface::surface_draw_image` calls
    //! instead. Decoding *and* scaling both cost real time per call —
    //! `scale_simple` on top of `Pixbuf::from_file`/`from_read` — and the
    //! cached value is the already-scaled [`Pixbuf`] for a given
    //! `(source, target size, DPI scale)`, not just the raw decode — a cache
    //! hit skips both steps, not just the first. See [`crate::image_cache`]'s
    //! module doc for the shared cache shape every pixel backend plugs into.

    use gtk4::cairo::Context;
    use gtk4::gdk::prelude::GdkCairoContextExt;
    use gtk4::gdk_pixbuf::{InterpType, Pixbuf};

    use crate::backend::ImagePaintResult;
    use crate::event::Rect as QRect;
    use crate::image_cache::ImageCache;
    use crate::primitives::image::{Image, ImageSource};

    /// Decode `source` into a [`Pixbuf`], or `None` on any failure (missing
    /// file, unreadable bytes, unsupported format — `gdk_pixbuf` reports all
    /// of these the same way, as an `Err`, and this rasteriser collapses
    /// them to `None` rather than distinguishing further; see
    /// [`Backend::paint_image`](crate::backend::Backend::paint_image)'s doc
    /// comment for why that collapses to [`ImagePaintResult::Unsupported`]
    /// rather than propagating a typed error).
    pub(super) fn load_pixbuf(source: &ImageSource) -> Option<Pixbuf> {
        match source {
            ImageSource::Path(path) => Pixbuf::from_file(path).ok(),
            ImageSource::Bytes(bytes) => {
                Pixbuf::from_read(std::io::Cursor::new(bytes.clone())).ok()
            }
        }
    }

    /// Clip to `bounds` and paint `scaled` at `target`'s origin — the shared
    /// tail end of both [`paint_image`] and [`draw_image_cached`], factored out
    /// to avoid duplicating the save/clip/paint/restore dance.
    pub(super) fn paint_scaled(
        cr: &Context,
        bounds: QRect,
        target: QRect,
        scaled: &Pixbuf,
    ) -> ImagePaintResult {
        if cr.save().is_err() {
            return ImagePaintResult::Unsupported;
        }
        cr.rectangle(
            bounds.x as f64,
            bounds.y as f64,
            bounds.width as f64,
            bounds.height as f64,
        );
        cr.clip();
        cr.set_source_pixbuf(scaled, target.x as f64, target.y as f64);
        let _ = cr.paint();
        let _ = cr.restore();
        ImagePaintResult::Painted
    }

    /// Paint `image` into `(x, y, w, h)`, decoding `image.source` fresh on
    /// every call — see the module docs' "Decode cache" section for why this
    /// public entry point deliberately does *not* cache, and
    /// [`draw_image_cached`] for the crate-internal one that does.
    #[cfg(test)]
    pub fn paint_image(
        cr: &Context,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        image: &Image,
    ) -> ImagePaintResult {
        if w <= 0.0 || h <= 0.0 {
            return ImagePaintResult::Unsupported;
        }
        let Some(pixbuf) = load_pixbuf(&image.source) else {
            return ImagePaintResult::Unsupported;
        };

        let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
        let target = image.layout(bounds).bounds;
        let tw = (target.width.round() as i32).max(1);
        let th = (target.height.round() as i32).max(1);

        let Some(scaled) = pixbuf.scale_simple(tw, th, InterpType::Bilinear) else {
            return ImagePaintResult::Unsupported;
        };

        paint_scaled(cr, bounds, target, &scaled)
    }

    /// Paint `image` into `bounds`, decoding through `cache` — see
    /// the module docs. `dpi_scale` is folded into the cache key so a live
    /// DPI change doesn't paint a stale bitmap rasterised for a
    /// different scale. Crate-internal: the backend's
    /// [`crate::PaintSurface::surface_draw_image`] is the only caller, so this can take whatever shape is convenient (a
    /// [`QRect`] instead of four `f64`s, an [`ImageCache`] reference) without
    /// worrying about breaking an external caller the way changing
    /// [`paint_image`]'s signature would.
    pub(crate) fn draw_image_cached(
        cr: &Context,
        bounds: QRect,
        image: &Image,
        cache: &mut ImageCache<Pixbuf>,
        dpi_scale: f32,
    ) -> ImagePaintResult {
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return ImagePaintResult::Unsupported;
        }

        let target = image.layout(bounds).bounds;
        let tw = (target.width.round() as i32).max(1);
        let th = (target.height.round() as i32).max(1);

        let source = &image.source;
        let scaled = cache.get_or_decode(source, tw as u32, th as u32, dpi_scale, || {
            let pixbuf = load_pixbuf(source)?;
            pixbuf.scale_simple(tw, th, InterpType::Bilinear)
        });
        let Some(scaled) = scaled else {
            return ImagePaintResult::Unsupported;
        };

        paint_scaled(cr, bounds, target, scaled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    /// A line with a wide glyph, a combining mark and mixed bold/italic.
    const LINE: &str = "a日e\u{301}bc";

    fn text_layout(cr: &Context) -> pango::Layout {
        let ctx = pangocairo::functions::create_context(cr);
        ctx.set_font_description(&pango::FontDescription::from_string("Monospace 12"));
        pango::Layout::new(&ctx)
    }

    fn line_runs() -> Vec<TextRunStyle> {
        let fg = Color::rgb(230, 230, 230);
        vec![
            TextRunStyle::new(0..1, fg),
            TextRunStyle::new(1..4, Color::rgb(220, 60, 60)).with_bold(true),
            TextRunStyle::new(4..7, fg).with_italic(true),
            TextRunStyle::new(7..9, fg),
        ]
    }

    /// The same line drawn the way the editor's own Pango path draws it:
    /// one layout, the default colour as the cairo source, and per-span
    /// colour/weight/style attributes.
    fn reference_line(cr: &Context, layout: &pango::Layout, runs: &[TextRunStyle]) {
        let attrs = pango::AttrList::new();
        for run in runs {
            let c = run.fg;
            let mut fg = pango::AttrColor::new_foreground(
                u16::from(c.r) * 257,
                u16::from(c.g) * 257,
                u16::from(c.b) * 257,
            );
            fg.set_start_index(run.range.start as u32);
            fg.set_end_index(run.range.end as u32);
            attrs.insert(fg);
            if run.bold {
                let mut w = pango::AttrInt::new_weight(pango::Weight::Bold);
                w.set_start_index(run.range.start as u32);
                w.set_end_index(run.range.end as u32);
                attrs.insert(w);
            }
            if run.italic {
                let mut st = pango::AttrInt::new_style(pango::Style::Italic);
                st.set_start_index(run.range.start as u32);
                st.set_end_index(run.range.end as u32);
                attrs.insert(st);
            }
        }
        layout.set_text(LINE);
        layout.set_attributes(Some(&attrs));
        cr.set_source_rgb(0.9, 0.9, 0.9);
        cr.move_to(4.0, 3.0);
        pangocairo::functions::show_layout(cr, layout);
    }

    fn render(paint: impl FnOnce(&Context, &pango::Layout)) -> Vec<u8> {
        let mut surface =
            ImageSurface::create(Format::ARgb32, 160, 30).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let layout = text_layout(&cr);
            paint(&cr, &layout);
        }
        surface.flush();
        let bytes = surface.data().expect("surface data").to_vec();
        bytes
    }

    #[test]
    fn draw_text_line_matches_the_editors_single_layout_paint() {
        let runs = line_runs();
        let via_verb = render(|cr, layout| {
            let mut s = CairoSurface {
                cr,
                layout: Some(layout),
                translucent_fill: true,
            };
            s.surface_draw_text_line(Point::new(4.0, 3.0), LINE, &runs, crate::FontRole::Editor);
        });
        let reference = render(|cr, layout| reference_line(cr, layout, &runs));
        assert!(via_verb.iter().any(|&b| b != 0), "the verb painted nothing");
        assert_eq!(via_verb, reference);
    }

    #[test]
    fn draw_text_line_leaves_uncovered_bytes_unpainted() {
        let fg = Color::rgb(230, 230, 230);
        let blank = render(|cr, layout| {
            let mut s = CairoSurface {
                cr,
                layout: Some(layout),
                translucent_fill: true,
            };
            s.surface_draw_text_line(Point::new(4.0, 3.0), "abc", &[], crate::FontRole::Editor);
        });
        assert!(blank.iter().all(|&b| b == 0));
        let partial = render(|cr, layout| {
            let mut s = CairoSurface {
                cr,
                layout: Some(layout),
                translucent_fill: true,
            };
            s.surface_draw_text_line(
                Point::new(4.0, 3.0),
                "abc",
                &[TextRunStyle::new(0..1, fg)],
                crate::FontRole::Editor,
            );
        });
        assert!(partial.iter().any(|&b| b != 0));
    }

    #[test]
    fn text_x_and_index_agree_with_pango_glyph_positions() {
        let surface = ImageSurface::create(Format::ARgb32, 10, 10).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let layout = text_layout(&cr);
        let mut s = CairoSurface {
            cr: &cr,
            layout: Some(&layout),
            translucent_fill: true,
        };
        let role = crate::FontRole::Editor;
        // Cluster boundaries of "a日e\u{301}bc": after a, 日, é, b, c.
        for idx in [0usize, 1, 4, 7, 8, 9] {
            layout.set_text(LINE);
            layout.set_attributes(None);
            let expect = layout.index_to_pos(idx as i32).x() as f32 / pango::SCALE as f32;
            let x = s.surface_text_x_at_index(LINE, &[], role, idx);
            assert_eq!(x, expect, "x at byte {idx}");
            assert_eq!(
                s.surface_text_index_at_x(LINE, &[], role, x),
                idx,
                "index at x {x}"
            );
        }
        // The wide glyph advances further than a narrow one.
        let narrow = s.surface_text_x_at_index(LINE, &[], role, 1);
        let wide = s.surface_text_x_at_index(LINE, &[], role, 4) - narrow;
        assert!(wide > narrow * 1.5, "wide {wide} narrow {narrow}");
        // Between `e` and its combining mark is not a boundary.
        let e_x = s.surface_text_x_at_index(LINE, &[], role, 4);
        let next = s.surface_text_x_at_index(LINE, &[], role, 7);
        let mid = s.surface_text_index_at_x(LINE, &[], role, e_x + (next - e_x) * 0.3);
        assert_eq!(mid, 4);
        let past = s.surface_text_index_at_x(LINE, &[], role, e_x + (next - e_x) * 0.95);
        assert_eq!(past, 7);
    }

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

/// The terminal's shared pixel painter, driven through `GtkBackend`'s
/// widget-contract paint against a Cairo `ImageSurface` and read back
/// pixel by pixel — terminal-cell background fill, wide glyphs and cursor
/// accents, without a display.
#[cfg(test)]
mod terminal_paint_tests {
    use crate::event::{Rect as QRect, Viewport};
    use crate::gtk::backend::GtkBackend;
    use crate::primitives::terminal::{Terminal, TerminalCell, TerminalCursorShape};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    const W: i32 = 200;
    const H: i32 = 40;
    const CHAR_W: f64 = 10.0;
    const LINE_H: f64 = 20.0;

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    /// ARgb32 stores each pixel as [B, G, R, A] in native (little-endian)
    /// byte order; `stride` is in bytes and may include padding.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
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

    fn paint(term: &Terminal) -> ImageSurface {
        let mut backend = GtkBackend::new();
        backend.set_current_theme(Theme::default());
        backend.set_current_line_height(LINE_H);
        backend.set_current_char_width(CHAR_W);
        Backend::begin_frame(&mut backend, Viewport::new(W as f32, H as f32, 1.0));
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            backend.enter_frame_scope(&cr, &pango_layout, |b| {
                crate::DrawsPrimitives::draw_primitive(
                    b,
                    QRect::new(0.0, 0.0, W as f32, H as f32),
                    term,
                );
            });
        }
        surface
    }

    /// A double-width glyph (CJK) followed by vt100's
    /// blank continuation cell must have its background span both
    /// columns — the continuation cell's own (different) background must
    /// NOT paint over the second half of the wide glyph's cell.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        // '日' is a double-width CJK character. Its vt100-derived
        // continuation cell carries a *different* background (cyan) to
        // prove the rasteriser doesn't just get lucky on matching colours.
        //
        // The wide glyph's foreground is set equal to its background
        // (magenta on magenta) so the glyph is invisible. This isolates the
        // thing under test — the two-column *background* fill — from the
        // glyph raster itself: with wide glyphs now scaled to fill the full
        // two-cell box (`wide_glyph_x_scale`), antialiased glyph ink reaches
        // deep into the second column and its exact colour depends on which
        // fallback font Pango picks, which varies by machine. A white glyph
        // here made the probe read a magenta/white blend on some hosts and
        // fail spuriously. Painting the glyph in the background colour keeps
        // the probe a pure, deterministic background read on every host,
        // while the glyph-scaling maths stays covered by the dedicated
        // `wide_glyph_x_scale` unit tests in `terminal_style`.
        let row = vec![cell('日', magenta, magenta), cell(' ', magenta, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Probe just past the first single-cell-width boundary, still
        // within the wide glyph's two-column span: must be magenta, not
        // cyan.
        let probe_x = (CHAR_W * 1.5) as i32;
        let (r, g, b) = pixel(&data, stride, probe_x, 5);
        assert_eq!(
            (r, g, b),
            (magenta.r, magenta.g, magenta.b),
            "wide cell's background should span both columns, not be \
             overpainted by the continuation cell's background"
        );
    }

    /// Companion regression: ordinary narrow (single-width) cells must
    /// still advance by exactly `char_width` — the wide-cell fix must not
    /// widen unrelated cells.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        let white = Color::rgb(255, 255, 255);
        // 'B' (the probed cell) paints its glyph in its own background
        // colour so antialiased glyph ink can't corrupt the background
        // probe; 'A' keeps a visible (white) glyph as it isn't probed.
        // This tests the *advance* logic — that 'A' occupies exactly one
        // char_width and doesn't bleed into 'B''s column — independent of
        // glyph rendering.
        let row = vec![cell('A', white, magenta), cell('B', cyan, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Just past the single-cell-width boundary: second cell's own
        // background (cyan) should already be showing.
        let probe_x = (CHAR_W * 1.5) as i32;
        let (r, g, b) = pixel(&data, stride, probe_x, 5);
        assert_eq!(
            (r, g, b),
            (cyan.r, cyan.g, cyan.b),
            "narrow cells must still advance by exactly char_width"
        );
    }

    // ── Cursor shape accent ─────────────────────────────────────────────
    //
    // These paint through the same shared `primitives::terminal::paint`
    // every pixel backend's terminal paint uses — see that
    // fn's own `cursor_accent_visible`/`cursor_accent_rect` call for the
    // logic under test. The probed cell's text is a blank space so no
    // glyph ink can land on the probed pixels (matching this module's
    // other pixel-probe tests' convention), isolating the accent fill
    // from glyph rasterisation.

    /// A `Bar` cursor paints a thin accent stripe at the cell's *left*
    /// edge in the cell's own foreground colour, and does **not** invert
    /// the rest of the cell (unlike `Block`).
    #[test]
    fn bar_cursor_paints_a_left_edge_accent_stripe() {
        let fg = Color::rgb(255, 0, 0);
        let bg = Color::rgb(0, 0, 0);
        let mut c = cell(' ', fg, bg);
        c.is_cursor = true;
        c.cursor_shape = TerminalCursorShape::Bar;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Inside the ~2px accent stripe at the cell's left edge.
        let (r, g, b) = pixel(&data, stride, 1, 5);
        assert_eq!(
            (r, g, b),
            (fg.r, fg.g, fg.b),
            "bar cursor accent should paint at the cell's left edge"
        );
        // Well past the stripe, still inside the (10px-wide) cell: the
        // cell's own (uninverted) background, not the accent colour.
        let (r2, g2, b2) = pixel(&data, stride, 8, 5);
        assert_eq!(
            (r2, g2, b2),
            (bg.r, bg.g, bg.b),
            "bar cursor must not invert the rest of the cell"
        );
    }

    /// An `Underline` cursor paints a thin accent stripe at the cell's
    /// *bottom* edge, and likewise does not invert the rest of the cell.
    #[test]
    fn underline_cursor_paints_a_bottom_edge_accent_stripe() {
        let fg = Color::rgb(0, 255, 0);
        let bg = Color::rgb(0, 0, 0);
        let mut c = cell(' ', fg, bg);
        c.is_cursor = true;
        c.cursor_shape = TerminalCursorShape::Underline;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Inside the ~2px accent stripe at the cell's bottom edge
        // (LINE_H = 20.0).
        let (r, g, b) = pixel(&data, stride, 5, 19);
        assert_eq!(
            (r, g, b),
            (fg.r, fg.g, fg.b),
            "underline cursor accent should paint at the cell's bottom edge"
        );
        // Near the top of the same cell: the cell's own background.
        let (r2, g2, b2) = pixel(&data, stride, 5, 2);
        assert_eq!(
            (r2, g2, b2),
            (bg.r, bg.g, bg.b),
            "underline cursor must not invert the rest of the cell"
        );
    }

    /// A `Block` cursor (the default shape) still inverts the
    /// whole cell — no separate accent stripe.
    #[test]
    fn block_cursor_still_inverts_whole_cell() {
        let fg = Color::rgb(255, 0, 0);
        let bg = Color::rgb(0, 0, 0);
        let mut c = cell(' ', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        let (r, g, b) = pixel(&data, stride, 5, 10);
        assert_eq!(
            (r, g, b),
            (fg.r, fg.g, fg.b),
            "block cursor should invert: the whole cell shows the cell's own fg as its background"
        );
    }

    // The paint-level contract of repainting only the rows in `dirty_rows`
    // is covered by `primitives::terminal::native_surface_paint::tests::
    // dirty_rows_filter_skips_untouched_rows`, which runs on every backend.
}

#[cfg(test)]
mod form_paint_tests {
    use super::PangoMeasure;
    use crate::primitives::form::{FieldKind, Form, FormField, LabelAlign, LabelPosition};
    use crate::theme::Theme;
    use crate::types::{StyledText, WidgetId};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    /// Paint `form` via the shared [`crate::primitives::form::paint`]
    /// through a [`crate::gtk::surface::CairoSurface`] over a fresh in-memory
    /// `ImageSurface` — the same adapter `gtk::multi_section_view`'s
    /// embedded-`Form` body uses. Builds its own [`crate::FormLayout`]
    /// with the shared `form_field_measure` (the same measurer
    /// `Form`'s pixel `Primitive::layout` uses) so painted rows match what
    /// hit-testing would resolve.
    fn paint(form: &Form) {
        let surface = ImageSurface::create(Format::ARgb32, 320, 160).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);
        let theme = Theme::default();
        let row_h = crate::primitives::layout_metrics::form_row_height(14.0);
        let measure = PangoMeasure {
            pango_layout: Some(&pango_layout),
            char_width: 8.0,
        };
        let flayout = form.layout(320.0, 160.0, |i| {
            crate::primitives::layout_metrics::form_field_measure(&form.fields[i], row_h, &measure)
        });
        let mut raw = crate::gtk::surface::CairoSurface {
            cr: &cr,
            layout: Some(&pango_layout),
            translucent_fill: false,
        };
        crate::primitives::form::paint(
            form,
            &flayout,
            &mut raw,
            &theme,
            crate::Point::new(0.0, 0.0),
        );
    }

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    /// Cairo's `ARgb32` stores each pixel as four bytes in native
    /// (little-endian) byte order: [B, G, R, A] — mirrors
    /// `gtk::activity_bar`'s test-local helper of the
    /// same name.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    /// Regression test: `Form::layout` (the hit-test path) clamps
    /// `scroll_offset` via `clamp_scroll_offset`, which affects which
    /// field occupies row 0.
    #[test]
    fn draw_form_clamps_out_of_range_scroll_offset_like_form_layout() {
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![
                FormField {
                    id: WidgetId::new("hdr"),
                    label: StyledText::plain("Header"),
                    kind: FieldKind::Label,
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
                FormField {
                    id: WidgetId::new("enabled"),
                    label: StyledText::plain("Enabled"),
                    kind: FieldKind::Toggle { value: true },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
            ],
            // Focused so the resolved field paints `selected_bg` —
            // distinct from both the plain background and the header's
            // `header_bg`, so row 0's colour unambiguously identifies
            // which field (if any) painted there.
            focused_field: Some(WidgetId::new("enabled")),
            scroll_offset: 50, // far past `fields.len()` == 2
            has_focus: true,
            label_position: LabelPosition::Left,
            label_align: LabelAlign::Start,
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };

        let mut surface =
            ImageSurface::create(Format::ARgb32, 320, 160).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            let theme = Theme::default();
            let row_h = crate::primitives::layout_metrics::form_row_height(14.0);
            let measure = PangoMeasure {
                pango_layout: Some(&pango_layout),
                char_width: 8.0,
            };
            let flayout = form.layout(320.0, 160.0, |i| {
                crate::primitives::layout_metrics::form_field_measure(
                    &form.fields[i],
                    row_h,
                    &measure,
                )
            });
            let mut raw = crate::gtk::surface::CairoSurface {
                cr: &cr,
                layout: Some(&pango_layout),
                translucent_fill: false,
            };
            crate::primitives::form::paint(
                &form,
                &flayout,
                &mut raw,
                &theme,
                crate::Point::new(0.0, 0.0),
            );
        }

        let theme = Theme::default();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (r, g, b) = pixel(&data, stride, 4, 4);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "row 0 should paint the focused 'enabled' field (the field \
             `clamp_scroll_offset` resolves to), not leave the background \
             untouched the way the pre-fix raw `.skip()` did",
        );
    }

    /// `TextInput.cursor`/`selection_anchor` are host-supplied byte offsets
    /// with no guarantee they land on a char boundary; slicing `&shown[lo..hi]`
    /// must snap to char boundaries to avoid panicking on multibyte characters.
    #[test]
    fn text_input_with_multibyte_cursor_and_selection_does_not_panic() {
        // "café🎉" — byte 4 sits inside 'é' (starts at byte 3); byte 7
        // sits inside the emoji (starts at byte 6).
        let value = "café🎉";
        assert!(!value.is_char_boundary(4));
        assert!(!value.is_char_boundary(7));
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("name"),
                label: StyledText::plain("Name"),
                kind: FieldKind::TextInput {
                    value: value.into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    selection_anchor: Some(7),
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
            label_position: LabelPosition::Left,
            label_align: LabelAlign::Start,
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };

        // Must not panic.
        paint(&form);
    }

    /// `PasswordInput` byte-cursor-to-mask-char-position conversion must snap
    /// to char boundaries to handle multibyte characters.
    #[test]
    fn password_input_with_multibyte_cursor_does_not_panic() {
        let value = "café🎉";
        assert!(!value.is_char_boundary(4));
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("pw"),
                label: StyledText::plain("Password"),
                kind: FieldKind::PasswordInput {
                    value: value.into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    mask_char: '*',
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: Some(WidgetId::new("pw")),
            scroll_offset: 0,
            has_focus: true,
            label_position: LabelPosition::Left,
            label_align: LabelAlign::Start,
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };

        // Must not panic.
        paint(&form);
    }

    /// `TextArea` clamps `cursor` against `first_line.len()`, but slicing
    /// `shown` (which may be `first_line` or a differently-lengthed placeholder)
    /// must snap to char boundaries.
    #[test]
    fn text_area_with_multibyte_cursor_does_not_panic() {
        let value = "café🎉\nsecond line";
        assert!(!value.is_char_boundary(4));
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("notes"),
                label: StyledText::plain("Notes"),
                kind: FieldKind::TextArea {
                    value: value.into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    visible_rows: 3,
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: Some(WidgetId::new("notes")),
            scroll_offset: 0,
            has_focus: true,
            label_position: LabelPosition::Left,
            label_align: LabelAlign::Start,
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };

        // Must not panic.
        paint(&form);
    }

    const CHROME_W: i32 = 200;
    const CHROME_H: i32 = 60;
    const LINE_HEIGHT: f64 = 18.0;
    // Distinct from every colour `draw_settings_chrome` itself paints
    // (header_bg/tab_bar_bg/selected_bg), so a search-row pixel that
    // still reads this sentinel unambiguously means "nothing painted
    // here", not "painted a colour that happens to match by chance."
    const SENTINEL: (u8, u8, u8) = (1, 2, 3);

    /// Paint `GtkBackend::draw_settings_chrome` into a `CHROME_W`x`CHROME_H` surface
    /// pre-filled with [`SENTINEL`], at the given `height`, and return
    /// the raw pixel buffer + stride for probing.
    fn paint_chrome(height: f64) -> (Vec<u8>, usize) {
        let mut surface =
            ImageSurface::create(Format::ARgb32, CHROME_W, CHROME_H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let (sr, sg, sb) = (
                SENTINEL.0 as f64 / 255.0,
                SENTINEL.1 as f64 / 255.0,
                SENTINEL.2 as f64 / 255.0,
            );
            cr.set_source_rgb(sr, sg, sb);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let mut backend = crate::gtk::GtkBackend::new();
            backend.set_current_theme(Theme::default());
            backend.set_current_line_height(LINE_HEIGHT);
            crate::Backend::begin_frame(
                &mut backend,
                crate::Viewport::new(CHROME_W as f32, CHROME_H as f32, 1.0),
            );
            backend.enter_frame_scope(&cr, &pango_layout, |b| {
                crate::Backend::draw_settings_chrome(
                    b,
                    crate::Rect::new(0.0, 0.0, CHROME_W as f32, height as f32),
                    "HEADER",
                    "",
                    "placeholder",
                    false,
                );
            });
        }
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data").to_vec();
        (data, stride)
    }

    /// A 1-row-tall `height` (the shape
    /// [`crate::compose::sidebar_panel_body::SidebarPanelChrome::Header`]
    /// reserves) must leave the would-be search row untouched, since
    /// `draw_settings_chrome` only paints rows that fit within the given rect.
    #[test]
    fn draw_settings_chrome_one_row_height_paints_no_search_row() {
        let (data, stride) = paint_chrome(LINE_HEIGHT);

        // Probe x=150, clear of the left-aligned "HEADER" glyphs (same
        // convention the Win settings-chrome tests use).
        let probe_x = 150;

        // Sample inside where row 1 (search) would have started —
        // comfortably past the header row, comfortably before the
        // surface's bottom edge.
        let probe_y = (LINE_HEIGHT * 1.5) as i32;
        assert_eq!(
            pixel(&data, stride, probe_x, probe_y),
            SENTINEL,
            "a 1-row-tall chrome rect must not paint a search row past its own height"
        );

        // The header row itself must still have painted.
        let theme = Theme::default();
        assert_eq!(
            pixel(&data, stride, probe_x, (LINE_HEIGHT / 2.0) as i32),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
            "the header row must still paint even when the search row doesn't"
        );
    }

    /// A full 2-row-tall `height` (the shape
    /// [`crate::compose::sidebar_panel_body::SidebarPanelChrome::HeaderAndSearch`]
    /// reserves) must still paint the search row — the height-aware
    /// fix must not regress the documented 2-row shape.
    #[test]
    fn draw_settings_chrome_two_row_height_paints_search_row() {
        let (data, stride) = paint_chrome(LINE_HEIGHT * 2.0);

        let theme = Theme::default();
        let probe_y = (LINE_HEIGHT * 1.5) as i32;
        assert_eq!(
            pixel(&data, stride, 4, probe_y),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "a 2-row-tall chrome rect must still paint the inactive search row"
        );
    }
}

/// The `minimap` widget painted through this backend's adapter
/// surface, as the pixel tests below drive it.
#[cfg(test)]
mod minimap_pixel_tests {
    #![allow(dead_code)]

    use gtk4::cairo::Context;
    use gtk4::pango;

    use super::super::surface::CairoSurface;
    use crate::event::Rect as QRect;
    use crate::primitives::minimap::paint_pixels::{self, Glyphs};
    use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapScale};
    use crate::theme::Theme;

    /// The pixel layout of `minimap` in `(x, y, w, h)` without painting, at
    /// [`MinimapScale::One`].
    pub fn gtk_minimap_layout(minimap: &Minimap, x: f64, y: f64, w: f64, h: f64) -> MinimapLayout {
        gtk_minimap_layout_scaled(minimap, x, y, w, h, MinimapScale::One)
    }

    /// [`gtk_minimap_layout`] at an explicit [`MinimapScale`] — see
    /// [`Minimap::pixel_layout`].
    pub(crate) fn gtk_minimap_layout_scaled(
        minimap: &Minimap,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        scale: MinimapScale,
    ) -> MinimapLayout {
        minimap.pixel_layout(
            QRect::new(x as f32, y as f32, w as f32, h as f32),
            scale,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        cr: &Context,
        pango_layout: &pango::Layout,
        rect: (f64, f64, f64, f64),
        minimap: &Minimap,
        theme: &Theme,
        scale: MinimapScale,
        glyphs: Glyphs,
    ) -> MinimapLayout {
        let (x, y, w, h) = rect;
        let layout = gtk_minimap_layout_scaled(minimap, x, y, w, h, scale);
        let mut surface = CairoSurface {
            cr,
            layout: Some(pango_layout),
            translucent_fill: true,
        };
        paint_pixels::paint(minimap, &layout, &mut surface, theme, scale, glyphs);
        layout
    }

    /// Draw `minimap` onto `cr` at [`MinimapScale::One`] as column blocks,
    /// without the glyph atlas. Returns the layout for host click dispatch
    /// (`layout.hit_test(x, y)`).
    #[allow(clippy::too_many_arguments)]
    pub fn paint_minimap(
        cr: &Context,
        pango_layout: &pango::Layout,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        minimap: &Minimap,
        theme: &Theme,
    ) -> MinimapLayout {
        paint(
            cr,
            pango_layout,
            (x, y, w, h),
            minimap,
            theme,
            MinimapScale::One,
            Glyphs::Blocks,
        )
    }

    /// Draw `minimap` at `mm_scale` with glyph-atlas character shapes, each
    /// tile `dpi_scale` device pixels per logical pixel.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_minimap_cached(
        cr: &Context,
        pango_layout: &pango::Layout,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        minimap: &Minimap,
        theme: &Theme,
        dpi_scale: f64,
        mm_scale: MinimapScale,
    ) -> MinimapLayout {
        paint(
            cr,
            pango_layout,
            (x, y, w, h),
            minimap,
            theme,
            mm_scale,
            Glyphs::Atlas {
                dpi_scale: dpi_scale as f32,
            },
        )
    }

    mod tests {
        use super::*;
        use crate::primitives::minimap::{MinimapHit, MinimapLine, COLUMN_CAPACITY, ROW_PITCH_PX};
        use crate::types::{Color, WidgetId};
        use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

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
                scale: MinimapScale::One,
                sizing: None,
            }
        }

        fn surface_and_layout() -> (CairoContext, pango::Layout) {
            let surface =
                ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
            let cr = CairoContext::new(&surface).expect("Context::new");
            let layout = pangocairo::functions::create_layout(&cr);
            (cr, layout)
        }

        // ── fixed pitch, not file-length-dependent ───────────────────────

        #[test]
        fn row_pitch_is_independent_of_the_strip_height() {
            // The same buffer painted into strips of two very different
            // heights must resolve to the *same* row pitch under
            // [`MinimapSizing::FixedPitch`]. See `Minimap::layout`'s module docs.
            let lines: Vec<&str> = vec!["fn main() {}"; 8];
            let mm = minimap_from(lines, 8);

            let tall = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 800.0);
            let short = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 16.0);

            let tall_px = tall.visible_lines[0].bounds.height as f64;
            let short_px = short.visible_lines[0].bounds.height as f64;
            assert_eq!(tall_px, ROW_PITCH_PX);
            assert_eq!(short_px, ROW_PITCH_PX);
        }

        #[test]
        fn row_pitch_is_independent_of_the_file_length() {
            // The other half of the same guarantee: a 3-line file and a
            // 10,000-line file painted into the *same* strip must also
            // resolve to the same row pitch.
            let short_mm = minimap_from(vec!["fn main() {}"; 3], 3);
            let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
            let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
            let long_mm = minimap_from(long_refs, 10_000);

            let short_layout = gtk_minimap_layout(&short_mm, 0.0, 0.0, 40.0, 400.0);
            let long_layout = gtk_minimap_layout(&long_mm, 0.0, 0.0, 40.0, 400.0);

            assert_eq!(
                short_layout.visible_lines[0].bounds.height as f64,
                ROW_PITCH_PX
            );
            assert_eq!(
                long_layout.visible_lines[0].bounds.height as f64,
                ROW_PITCH_PX
            );
        }

        #[test]
        fn short_file_in_a_tall_strip_does_not_stretch() {
            // A 3-line file at the fixed pitch occupies 6px of a 200px
            // strip -- it must not stretch to fill it.
            let mm = minimap_from(vec!["//! Placeholder module"; 3], 3);
            let layout = gtk_minimap_layout(&mm, 0.0, 0.0, 200.0, 200.0);
            assert_eq!(layout.visible_lines.len(), 3);
            let last = layout.visible_lines.last().unwrap();
            assert!(
                (last.bounds.y + last.bounds.height) < 200.0,
                "a short file must not stretch its rows to fill the whole strip"
            );
        }

        // ── legibility floor ─────────────────────────────────────────────
        //
        // `is_legible`/`render_mode`'s own threshold behaviour is covered by
        // `primitives::minimap::tests::legibility_floor_switches_render_mode_on_both_sides`
        // — this module keeps only the paint-level regression that a
        // real Cairo/Pango pass actually respects that threshold.

        #[test]
        fn default_fixed_pitch_stays_below_the_floor_and_never_shapes_text() {
            // ROW_PITCH_PX (2px) is below LEGIBILITY_FLOOR_PX (4px), so the
            // default GTK minimap always lands in ColumnBlocks.
            let mm = minimap_from(vec!["fn main() {}"; 4], 4);
            let (cr, layout) = surface_and_layout();

            let _ = paint_minimap(&cr, &layout, 0.0, 0.0, 40.0, 4.0, &mm, &Theme::default());
            assert_eq!(layout.text(), "");

            let _ = paint_minimap(&cr, &layout, 0.0, 0.0, 40.0, 400.0, &mm, &Theme::default());
            assert_eq!(
                layout.text(),
                "",
                "the fixed pitch stays below the legibility floor regardless of strip height"
            );
        }

        // ── glyph atlas ─────────────────────────────────────────────────────

        /// Paint `mm` through [`paint_minimap_cached`] into a fresh surface and
        /// return the raw pixel bytes.
        fn paint_cached_pixels(mm: &Minimap, w: i32, h: i32) -> Vec<u8> {
            paint_cached_pixels_at_scale(mm, w, h, MinimapScale::One, &Theme::default())
        }

        /// [`paint_cached_pixels`], but at an explicit [`MinimapScale`] and
        /// [`Theme`].
        fn paint_cached_pixels_at_scale(
            mm: &Minimap,
            w: i32,
            h: i32,
            mm_scale: MinimapScale,
            theme: &Theme,
        ) -> Vec<u8> {
            let mut surface = ImageSurface::create(Format::ARgb32, w, h).expect("create surface");
            {
                let cr = CairoContext::new(&surface).expect("Context::new");
                let pango_layout = pangocairo::functions::create_layout(&cr);
                paint_minimap_cached(
                    &cr,
                    &pango_layout,
                    0.0,
                    0.0,
                    w as f64,
                    h as f64,
                    mm,
                    theme,
                    1.0,
                    mm_scale,
                );
            }
            surface.flush();
            let pixels = surface.data().expect("surface data").to_vec();
            pixels
        }

        #[test]
        fn atlas_mode_differentiates_content_column_blocks_would_make_identical() {
            // Two rows of equal length, both entirely non-blank, must paint
            // *different* alpha patterns under real character shapes — solid
            // columns would paint identically for both.
            let code_line = "fn main() {";
            let comment_line = "///////////";
            assert_eq!(
                code_line.chars().count(),
                comment_line.chars().count(),
                "both lines must be the same length for this to be a fair comparison"
            );

            let code_pixels = paint_cached_pixels(&minimap_from(vec![code_line], 1), 40, 4);
            let comment_pixels = paint_cached_pixels(&minimap_from(vec![comment_line], 1), 40, 4);

            assert_ne!(
                code_pixels, comment_pixels,
                "Characters-mode (atlas) rows for different same-length, all-non-blank \
             content must paint different pixels"
            );
        }

        #[test]
        fn the_glyph_sample_sheet_does_not_pollute_the_text_run_sink() {
            // The sample sheet shapes each ASCII glyph into a private,
            // throwaway surface (never real screen content), but the
            // text-run sink is **thread-local**, not scoped to any
            // particular `Context` — the recording wrapper must never be
            // called here, else every atlas cache miss during a test would
            // append bogus bounds to the shared sink.
            let previous = crate::testing::install_text_run_sink();
            let chars: String = (' '..='~').collect();
            let _ = super::super::super::surface::glyph_alpha_sheet(
                "sans-serif",
                &chars,
                paint_pixels::SAMPLE_CELL,
            );
            let recorded = crate::testing::take_text_run_sink(previous);
            assert!(
                recorded.is_empty(),
                "the glyph sample sheet must never record into the text-run sink, got {recorded:?}"
            );
        }

        #[test]
        fn atlas_mode_paints_something_for_a_non_blank_line() {
            // A baseline sanity check alongside the differentiation test
            // above: a non-blank line must actually paint ink, not silently
            // no-op into an all-background surface (which would make the
            // "differentiates content" test above meaningless if both inputs
            // painted nothing).
            let bg = ImageSurface::create(Format::ARgb32, 40, 4)
                .and_then(|mut s| {
                    let cr = CairoContext::new(&s)?;
                    cr.set_source_rgb(0.0, 0.0, 0.0);
                    cr.paint()?;
                    drop(cr); // release the surface borrow before `s.data()`
                    s.flush();
                    Ok(s.data().expect("surface data").to_vec())
                })
                .expect("blank surface");

            let painted = paint_cached_pixels(&minimap_from(vec!["fn main() {"], 1), 40, 4);
            assert_ne!(
                painted, bg,
                "a non-blank line must paint something other than a uniform background"
            );
        }

        /// At [`MinimapScale::Two`] (a 2px wide x 4px tall cell), a row of
        /// real code paints *shapes* — non-uniform pixel values across a
        /// character's own cell, not solid, uniformly-coloured blocks — and
        /// blank (whitespace) columns stay fully background throughout their cells.
        #[test]
        fn scale_two_atlas_mode_paints_a_shape_and_leaves_blank_columns_empty() {
            let line = "fn main()";
            let mm = minimap_from(vec![line], 1);
            let cell_w = MinimapScale::Two.cell_w_px() as i32;
            let row_h = MinimapScale::Two.row_pitch_px() as i32;
            let w = line.chars().count() as i32 * cell_w;

            let theme = Theme {
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                ..Theme::default()
            };

            let pixels = paint_cached_pixels_at_scale(&mm, w, row_h, MinimapScale::Two, &theme);
            let stride = ImageSurface::create(Format::ARgb32, w, row_h)
                .expect("create surface")
                .stride() as usize;
            let bg = (theme.background.r, theme.background.g, theme.background.b);

            // The space between "fn" and "main" is column 2 -- its whole
            // `cell_w`-wide, `row_h`-tall cell must stay fully background.
            let space_col = 2i32;
            for dx in 0..cell_w {
                for y in 0..row_h {
                    let x = space_col * cell_w + dx;
                    assert_eq!(
                        pixel(&pixels, stride, x, y),
                        bg,
                        "a blank column must stay fully background at ({x}, {y})"
                    );
                }
            }

            // The first character's own cell ('f', column 0) must show a
            // *shape* — at least two pixels inside the cell with different
            // colours — rather than a solid, uniformly-filled block.
            let mut cell_pixels = Vec::new();
            for dx in 0..cell_w {
                for y in 0..row_h {
                    cell_pixels.push(pixel(&pixels, stride, dx, y));
                }
            }
            assert!(
            cell_pixels.windows(2).any(|w| w[0] != w[1]),
            "expected non-uniform alpha (a glyph shape) across the 'f' cell, got {cell_pixels:?}"
        );
        }

        /// The thread's atlas cache is keyed on the tile size as well as the
        /// sample sheet, so a [`MinimapScale`] change rebuilds the atlas rather
        /// than serving the previous scale's smaller tile.
        #[test]
        fn atlas_cache_key_includes_minimap_scale_so_a_scale_change_rebuilds() {
            let mm = minimap_from(vec!["fn main() {}"], 1);
            let theme = Theme::default();
            let bg = (theme.background.r, theme.background.g, theme.background.b);
            let w = 40;
            let two_h = MinimapScale::Two.row_pitch_px() as i32;

            // Paint once at `One` (a 1x2 cell) through the shared cache...
            let surface_one =
                ImageSurface::create(Format::ARgb32, w, MinimapScale::One.row_pitch_px() as i32)
                    .expect("create surface");
            {
                let cr = CairoContext::new(&surface_one).expect("Context::new");
                let pango_layout = pangocairo::functions::create_layout(&cr);
                paint_minimap_cached(
                    &cr,
                    &pango_layout,
                    0.0,
                    0.0,
                    w as f64,
                    MinimapScale::One.row_pitch_px(),
                    &mm,
                    &theme,
                    1.0,
                    MinimapScale::One,
                );
            }

            // ...then at `Two` (a 2x4 cell) through the *same* cache instance.
            let mut surface_two =
                ImageSurface::create(Format::ARgb32, w, two_h).expect("create surface");
            {
                let cr = CairoContext::new(&surface_two).expect("Context::new");
                let pango_layout = pangocairo::functions::create_layout(&cr);
                paint_minimap_cached(
                    &cr,
                    &pango_layout,
                    0.0,
                    0.0,
                    w as f64,
                    two_h as f64,
                    &mm,
                    &theme,
                    1.0,
                    MinimapScale::Two,
                );
            }
            surface_two.flush();
            let stride = surface_two.stride() as usize;
            let data = surface_two.data().expect("surface data");

            // If the cache had wrongly reused `One`'s 1x2 tile for this
            // `Two` paint, the blit would only ever touch the tile's own
            // (much smaller) device-pixel footprint, leaving the bottom of
            // this 4px-tall row untouched (still background). A correctly
            // rebuilt 2x4 tile paints something below row 2.
            let mut painted_below_row_2 = false;
            for y in 2..two_h {
                for x in 0..w {
                    if pixel(&data, stride, x, y) != bg {
                        painted_below_row_2 = true;
                    }
                }
            }
            assert!(
                painted_below_row_2,
                "MinimapScale::Two's own 4px-tall cell must paint below row 2 -- a stale \
             1x2 tile carried over from MinimapScale::One would not"
            );
        }

        #[test]
        fn column_blocks_skip_indentation_and_internal_gaps() {
            // 4-space indent, "x", 2-space internal gap, "y": the walk must
            // paint no block over the indent or the gap -- only over the two
            // non-blank columns, preserving the indent silhouette.
            let mm = minimap_from(vec!["    x  y"], 1);
            let theme = Theme {
                background: Color::rgb(10, 10, 10),
                foreground: Color::rgb(200, 200, 200),
                ..Theme::default()
            };

            let mut surface = ImageSurface::create(Format::ARgb32, 20, 4).expect("create surface");
            {
                let cr = CairoContext::new(&surface).expect("Context::new");
                let pango_layout = pangocairo::functions::create_layout(&cr);
                paint_minimap(&cr, &pango_layout, 0.0, 0.0, 20.0, 4.0, &mm, &theme);
            }
            surface.flush();
            let stride = surface.stride() as usize;
            let data = surface.data().expect("surface data");

            let bg = (theme.background.r, theme.background.g, theme.background.b);
            let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);

            for col in 0..4 {
                assert_eq!(
                    pixel(&data, stride, col, 0),
                    bg,
                    "indent column {col} must stay blank"
                );
            }
            assert_eq!(
                pixel(&data, stride, 4, 0),
                fg,
                "'x' column must paint a block"
            );
            for col in 5..7 {
                assert_eq!(
                    pixel(&data, stride, col, 0),
                    bg,
                    "gap column {col} must stay blank"
                );
            }
            assert_eq!(
                pixel(&data, stride, 7, 0),
                fg,
                "'y' column must paint a block"
            );
        }

        #[test]
        fn column_blocks_stop_at_the_column_capacity() {
            let long_line = "x".repeat(10_000);
            let mm = minimap_from(vec![&long_line], 1);
            let theme = Theme {
                background: Color::rgb(1, 1, 1),
                foreground: Color::rgb(250, 250, 250),
                ..Theme::default()
            };
            let strip_w = (COLUMN_CAPACITY + 50) as i32;

            let mut surface =
                ImageSurface::create(Format::ARgb32, strip_w, 4).expect("create surface");
            {
                let cr = CairoContext::new(&surface).expect("Context::new");
                let pango_layout = pangocairo::functions::create_layout(&cr);
                paint_minimap(
                    &cr,
                    &pango_layout,
                    0.0,
                    0.0,
                    strip_w as f64,
                    4.0,
                    &mm,
                    &theme,
                );
            }
            surface.flush();
            let stride = surface.stride() as usize;
            let data = surface.data().expect("surface data");

            let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);
            let bg = (theme.background.r, theme.background.g, theme.background.b);
            assert_eq!(
                pixel(&data, stride, (COLUMN_CAPACITY - 1) as i32, 0),
                fg,
                "the last in-capacity column must still paint"
            );
            assert_eq!(
                pixel(&data, stride, COLUMN_CAPACITY as i32, 0),
                bg,
                "the walk must stop at COLUMN_CAPACITY and paint no further"
            );
        }

        // `color_at_column`/`SpanCursor`'s own behaviour is covered by
        // `primitives::minimap::tests::color_at_column_*` /
        // `span_cursor_matches_a_full_linear_scan_per_row` — nothing
        // GTK-specific left to re-test here.

        // ── paint/click round trip ────────────────────────────────────────

        /// Shared body for `paint_and_click_round_trip_returns_seek_for_the_clicked_fraction`
        /// — the minimap's layout returns coordinates in absolute frame
        /// (see `docs/PRIMITIVE_RULES.md`), so `hit_test` must be called with
        /// coordinates shifted by the same `x`/`y` origin the strip was painted at.
        fn paint_and_click_round_trip_at(x: f64, y: f64) {
            let mm = minimap_from(vec!["x"; 8], 8);
            let (cr, pango_layout) = surface_and_layout();
            let layout = paint_minimap(
                &cr,
                &pango_layout,
                x,
                y,
                40.0,
                100.0,
                &mm,
                &Theme::default(),
            );
            assert_eq!(
                layout.hit_test((x + 20.0) as f32, (y + 50.0) as f32),
                MinimapHit::Seek { fraction: 0.5 }
            );
            assert_eq!(
                layout.hit_test((x + 20.0) as f32, y as f32),
                MinimapHit::Seek { fraction: 0.0 }
            );
        }

        #[test]
        fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction() {
            paint_and_click_round_trip_at(0.0, 0.0);
        }

        /// Regression guard: `(x, y) = (0, 0)` is exactly the case where a
        /// LOCAL/ABSOLUTE mixup in `gtk_minimap_layout` would be invisible.
        #[test]
        fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction_at_nonzero_origin() {
            paint_and_click_round_trip_at(7.0, 13.0);
        }

        #[test]
        fn empty_minimap_is_a_no_op() {
            let mm = minimap_from(vec![], 0);
            let (cr, pango_layout) = surface_and_layout();
            let layout = paint_minimap(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                40.0,
                100.0,
                &mm,
                &Theme::default(),
            );
            assert!(layout.visible_lines.is_empty());
        }

        // ── strip clipping ──────────────────────────────────────────────────

        /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
        ///
        /// Cairo's `ARgb32` stores each pixel as four bytes in native
        /// (little-endian) byte order: [B, G, R, A].
        fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
            let off = y as usize * stride + x as usize * 4;
            (data[off + 2], data[off + 1], data[off])
        }

        #[test]
        fn wide_line_blocks_never_paint_right_of_bounds() {
            // Without `cr.clip()` and Pango layout width, a line wider than
            // the strip paints across whatever sat beside it. Paint a
            // pathologically long line into a strip embedded in a wider white
            // canvas and assert nothing right of the strip is touched.
            const STRIP_W: i32 = 40;
            const CANVAS_W: i32 = 200;
            const H: i32 = 40;

            let long_line = "x".repeat(500);
            let mm = minimap_from(vec![&long_line], 1);

            let mut surface =
                ImageSurface::create(Format::ARgb32, CANVAS_W, H).expect("create surface");
            {
                let cr = CairoContext::new(&surface).expect("Context::new");
                cr.set_source_rgb(1.0, 1.0, 1.0); // white sentinel
                cr.paint().ok();

                let pango_layout = pangocairo::functions::create_layout(&cr);
                let theme = Theme {
                    background: Color::rgb(20, 20, 20),
                    foreground: Color::rgb(255, 255, 255),
                    ..Theme::default()
                };
                // The fixed pitch (ROW_PITCH_PX, 2px) is below the
                // legibility floor, so this exercises the ColumnBlocks
                // branch -- the column-capacity walk on its own already
                // bounds how far right it paints, but the clip is still the
                // hard guarantee this test checks.
                paint_minimap(
                    &cr,
                    &pango_layout,
                    0.0,
                    0.0,
                    STRIP_W as f64,
                    H as f64,
                    &mm,
                    &theme,
                );
            }
            surface.flush();
            let stride = surface.stride() as usize;
            let data = surface.data().expect("surface data");

            for y in 0..H {
                for x in STRIP_W..CANVAS_W {
                    assert_eq!(
                        pixel(&data, stride, x, y),
                        (255, 255, 255),
                        "pixel ({x},{y}) is right of bounds.x + bounds.width and must be untouched"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod image_tests {
    use super::image::*;
    use crate::backend::ImagePaintResult;
    use crate::event::Rect as QRect;
    use crate::image_cache::ImageCache;
    use crate::primitives::image::ImageFit;
    use crate::primitives::image::{Image, ImageSource};
    use crate::types::WidgetId;
    use gtk4::cairo::{Context as CairoContext, Format, ImageSurface};
    use gtk4::gdk_pixbuf::{Colorspace, Pixbuf};

    fn surface() -> CairoContext {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        CairoContext::new(&surface).expect("Context::new")
    }

    /// A tiny solid-colour PNG, generated at test time rather than
    /// checked in as a binary fixture.
    fn tiny_png_bytes() -> Vec<u8> {
        let pixbuf = Pixbuf::new(Colorspace::Rgb, false, 8, 4, 4).expect("Pixbuf::new");
        pixbuf.fill(0xff0000ff); // opaque red
        pixbuf
            .save_to_bufferv("png", &[])
            .expect("Pixbuf::save_to_bufferv")
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

    fn cache() -> ImageCache<Pixbuf> {
        ImageCache::with_capacity(4)
    }

    fn bounds(w: f32, h: f32) -> QRect {
        QRect::new(0.0, 0.0, w, h)
    }

    #[test]
    fn draw_image_from_bytes_reports_painted() {
        let cr = surface();
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Contain);
        let result = paint_image(&cr, 0.0, 0.0, 40.0, 40.0, &img);
        assert_eq!(result, ImagePaintResult::Painted);
    }

    #[test]
    fn draw_image_from_missing_path_reports_unsupported() {
        let cr = surface();
        let img = image(
            ImageSource::Path("/nonexistent/does-not-exist.png".into()),
            ImageFit::Contain,
        );
        let result = paint_image(&cr, 0.0, 0.0, 40.0, 40.0, &img);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_from_corrupt_bytes_reports_unsupported() {
        let cr = surface();
        let img = image(ImageSource::Bytes(vec![0, 1, 2, 3]), ImageFit::Contain);
        let result = paint_image(&cr, 0.0, 0.0, 40.0, 40.0, &img);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn zero_size_rect_reports_unsupported_without_decoding() {
        let cr = surface();
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Contain);
        let result = paint_image(&cr, 0.0, 0.0, 0.0, 0.0, &img);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_cached_from_bytes_reports_painted() {
        let cr = surface();
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Contain);
        let result = draw_image_cached(&cr, bounds(40.0, 40.0), &img, &mut cache(), 1.0);
        assert_eq!(result, ImagePaintResult::Painted);
    }

    #[test]
    fn draw_image_cached_from_missing_path_reports_unsupported() {
        let cr = surface();
        let img = image(
            ImageSource::Path("/nonexistent/does-not-exist.png".into()),
            ImageFit::Contain,
        );
        let result = draw_image_cached(&cr, bounds(40.0, 40.0), &img, &mut cache(), 1.0);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_cached_from_corrupt_bytes_reports_unsupported() {
        let cr = surface();
        let img = image(ImageSource::Bytes(vec![0, 1, 2, 3]), ImageFit::Contain);
        let result = draw_image_cached(&cr, bounds(40.0, 40.0), &img, &mut cache(), 1.0);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_cached_zero_size_rect_reports_unsupported_without_decoding() {
        let cr = surface();
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Contain);
        let result = draw_image_cached(&cr, bounds(0.0, 0.0), &img, &mut cache(), 1.0);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    /// Black-box proof that a cache hit skips re-decoding entirely:
    /// a `Path` source that decodes fine once, then gets
    /// corrupted on disk, must still paint successfully on a second call
    /// with the same size/scale — that's only possible if the second
    /// call never touched the (now-corrupt) file again. Without the
    /// cache this test fails, because `load_pixbuf` would re-read the
    /// corrupted file and return `None`.
    #[test]
    fn cached_decode_survives_source_becoming_unreadable_on_disk() {
        let path = std::env::temp_dir().join(format!(
            "quadraui_image_cache_test_{}_{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        std::fs::write(&path, tiny_png_bytes()).expect("write tiny png fixture");

        let mut shared_cache = cache();
        let img = image(ImageSource::Path(path.clone()), ImageFit::Contain);

        let cr1 = surface();
        let first = draw_image_cached(&cr1, bounds(40.0, 40.0), &img, &mut shared_cache, 1.0);
        assert_eq!(
            first,
            ImagePaintResult::Painted,
            "first decode should succeed"
        );

        // Corrupt the file in place — a fresh decode from this point on
        // would fail.
        std::fs::write(&path, b"not a png").expect("corrupt fixture on disk");

        let cr2 = surface();
        let second = draw_image_cached(&cr2, bounds(40.0, 40.0), &img, &mut shared_cache, 1.0);
        assert_eq!(
            second,
            ImagePaintResult::Painted,
            "second draw at the same size/scale must hit the cache rather than \
             re-decoding the now-corrupt file"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// A different target size is a different cache key — the
    /// scaled `Pixbuf` cached for one size must not be reused (and
    /// stretched further) for another.
    #[test]
    fn different_target_size_still_decodes_and_paints() {
        let mut shared_cache = cache();
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Fill);

        let cr1 = surface();
        let first = draw_image_cached(&cr1, bounds(40.0, 40.0), &img, &mut shared_cache, 1.0);
        assert_eq!(first, ImagePaintResult::Painted);

        let cr2 = surface();
        let second = draw_image_cached(&cr2, bounds(80.0, 80.0), &img, &mut shared_cache, 1.0);
        assert_eq!(second, ImagePaintResult::Painted);
    }
}
