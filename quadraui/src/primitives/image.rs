//! `Image` primitive: a raster image (SVG/PNG/JPEG) painted at a target
//! rect.
//!
//! This is **chrome** — a fixed-size decorative element (an app logo, a
//! toolbar button glyph that has no font-icon equivalent). It is
//! deliberately **not**:
//! - an image-viewer widget (no zoom, pan, or multi-image gallery state),
//! - animation (no GIF/APNG frame stepping),
//! - an asset-management layer (callers still own the bytes/path they
//!   hand in — `Image` carries no asset registry, cache eviction policy,
//!   or lifetime tied to anything but the value itself).
//!
//! # Decoding is a surface concern
//!
//! [`Image`] only carries a source (bytes or a path) plus layout
//! metadata; it never decodes pixels itself. Its [`crate::Primitive`]
//! impl hands the source to the surface it paints on: a pixel backend's
//! [`crate::PaintSurface::surface_draw_image`] decodes through its native
//! stack — GTK via `gdk_pixbuf`, macOS via ImageIO, Win via WIC — and a
//! cell grid's [`crate::CellSurface::emit_graphics`] (TUI, behind the
//! `tui-images` Cargo feature) decodes via `crate::tui::image_decode`, so
//! the primitive stays free of image-format dependencies. TUI's graphics
//! channel has two tiers past plain [`Image::fallback_text`]: real pixels
//! via the Kitty graphics protocol on a terminal that has confirmed
//! support for it, or — on any other truecolor terminal — a `▀`-glyph
//! half-block mosaic, a lower-fidelity but still recognisable thumbnail.
//! When the surface reports it painted nothing, the primitive paints
//! [`Image::fallback_text`] itself.
//!
//! Each surface also keeps a small fixed-capacity LRU decode cache
//! (`crate::image_cache::ImageCache`) keyed on a hash of `image.source`
//! plus the resolved paint size/DPI scale, so repeated paints of the
//! same source at the same size reuse the decoded bitmap instead of
//! re-decoding.

use crate::backend::ImagePaintResult;
use crate::event::Rect;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Where the image's encoded bytes come from.
///
/// `Hash`: backends key their decode cache on a hash of
/// this value plus the resolved paint size/scale — see
/// `crate::image_cache::ImageCache`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageSource {
    /// Raw encoded image bytes (PNG/JPEG/SVG/...) — the backend's
    /// decoder sniffs the format from content, so no separate MIME hint
    /// is carried here.
    Bytes(Vec<u8>),
    /// Filesystem path to an image file, decoded lazily by the backend
    /// at the given paint size/scale and cached for reuse.
    Path(std::path::PathBuf),
}

/// How an image's intrinsic size maps onto its target rect. Named after
/// the equivalent CSS `object-fit` keywords since the semantics are
/// identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ImageFit {
    /// Scale to fit entirely within the target rect, preserving aspect
    /// ratio. May letterbox (leave empty space on one axis).
    #[default]
    Contain,
    /// Scale to fill the target rect, preserving aspect ratio. May crop
    /// (overflow one axis, clipped to the rect).
    Cover,
    /// Stretch to exactly fill the target rect, ignoring aspect ratio.
    Fill,
    /// Paint at intrinsic size, anchored at the rect's top-left corner —
    /// no scaling. Backends clip to the caller's rect if the image is
    /// larger.
    None,
}

/// Declarative description of an `Image` widget.
///
/// # Examples
///
/// ```
/// use quadraui::{Image, ImageFit, ImageSource, Rect, WidgetId};
///
/// let logo = Image {
///     id: WidgetId::new("image:app_logo"),
///     source: ImageSource::Path("assets/logo.png".into()),
///     intrinsic_size: Some((64, 64)),
///     fit: ImageFit::Contain,
///     fallback_text: "[LOGO]".to_string(),
/// };
///
/// // A wide target rect with `Contain`: the square logo stays square
/// // (letterboxed) rather than stretching to fill the wide rect.
/// let layout = logo.layout(Rect::new(0.0, 0.0, 200.0, 64.0));
/// assert_eq!(layout.bounds.width, 64.0);
/// assert_eq!(layout.bounds.height, 64.0);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub id: WidgetId,
    pub source: ImageSource,
    /// The image's natural pixel dimensions, if known up front. Required
    /// for [`ImageFit::Contain`] / [`ImageFit::Cover`] / [`ImageFit::None`]
    /// to compute a meaningful target rect via [`Image::layout`] — when
    /// `None`, [`Image::layout`] falls back to filling the whole target
    /// rect (equivalent to [`ImageFit::Fill`]) regardless of `fit`, since
    /// there is no aspect ratio to preserve. Backends that decode the
    /// image themselves (GTK/macOS) still learn the *real* intrinsic size
    /// from the decoded asset and may use that in preference to this
    /// hint; this field exists so [`Image::layout`] can be computed
    /// without decoding.
    pub intrinsic_size: Option<(u32, u32)>,
    pub fit: ImageFit,
    /// Required, not optional: TUI paints this in place of real pixels
    /// whenever it can't rasterise them — a bare `tui` build (no
    /// `tui-images` feature), a terminal that hasn't confirmed Kitty-
    /// protocol graphics support, or a real decode failure — and it's
    /// what GTK/macOS
    /// paint nothing in place of on a decode failure (bad path, corrupt
    /// bytes), so a host can still show *something* on every backend and
    /// every failure mode. An empty string means "leave the rect blank",
    /// a legitimate choice for a purely decorative image.
    pub fallback_text: String,
}

/// Resolved target rect for painting an [`Image`] — the result of
/// applying `fit` against a bounding rect. See [`Image::layout`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageLayout {
    /// The rect the image was fitted within — the paint clip, and the
    /// area a surface resolves its own placement against (a cell grid
    /// fits in pixel space, since terminal cells are not square).
    pub area: Rect,
    /// Where the image should actually be painted, in the same
    /// coordinate space as the `bounds` passed to [`Image::layout`].
    /// Always contained within `bounds` for [`ImageFit::Contain`] and
    /// [`ImageFit::Fill`]; may extend past `bounds` on one axis for
    /// [`ImageFit::Cover`] and [`ImageFit::None`] — backends are
    /// expected to clip painting to `bounds` in that case.
    pub bounds: Rect,
}

/// Hit-test classification for clicks against an [`Image`] widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageHit {
    /// Click landed on the image's resolved paint bounds.
    Image,
    /// Click missed the image's paint bounds — e.g. inside letterbox
    /// space [`ImageFit::Contain`] leaves empty, or entirely outside the
    /// widget's rect.
    Empty,
}

impl ImageLayout {
    /// Hit-test a click at `(x, y)` against this layout's resolved
    /// [`Self::bounds`].
    ///
    /// Coordinate frame: **ABSOLUTE** — same frame as [`Self::bounds`],
    /// which already carries the `bounds` rect passed to [`Image::layout`].
    pub fn hit_test(&self, x: f32, y: f32) -> ImageHit {
        let b = self.bounds;
        if x >= b.x && x < b.x + b.width && y >= b.y && y < b.y + b.height {
            ImageHit::Image
        } else {
            ImageHit::Empty
        }
    }
}

impl Image {
    /// Compute where this image should paint within `bounds`, honoring
    /// `fit`. Pure geometry — no decoding, so it works identically
    /// whether or not the backend has actually loaded `source` yet, and
    /// hosts can call it from `AppLogic::handle` (no `&mut Backend`
    /// needed) to reserve space before a click-routing pass, the same
    /// way every other primitive's `layout` works.
    pub fn layout(&self, bounds: Rect) -> ImageLayout {
        let Some((iw, ih)) = self.intrinsic_size else {
            // No intrinsic size to preserve an aspect ratio against —
            // filling the rect is the only sensible default.
            return ImageLayout {
                area: bounds,
                bounds,
            };
        };
        if iw == 0 || ih == 0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
            return ImageLayout {
                area: bounds,
                bounds,
            };
        }
        let iw = iw as f32;
        let ih = ih as f32;
        let target = match self.fit {
            ImageFit::Fill => bounds,
            ImageFit::None => Rect::new(bounds.x, bounds.y, iw, ih),
            ImageFit::Contain => fit_within(bounds, iw, ih, true),
            ImageFit::Cover => fit_within(bounds, iw, ih, false),
        };
        ImageLayout {
            area: bounds,
            bounds: target,
        }
    }

    /// Paint through `surface`'s decoder into `layout`'s area, reporting
    /// whether real pixels landed. [`crate::Primitive::paint`] discards
    /// the report; a caller that needs it calls this directly.
    pub(crate) fn paint_reporting(
        &self,
        layout: &ImageLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
    ) -> ImagePaintResult {
        surface.surface_draw_image(layout.area, self)
    }

    /// Paint onto a cell grid: the surface's graphics channel when it has
    /// one, else [`Self::fallback_text`] via [`Self::paint_fallback_cells`].
    /// Reports [`ImagePaintResult::Painted`] only when the graphics
    /// channel painted.
    #[cfg(feature = "tui")]
    pub(crate) fn paint_cells_reporting(
        &self,
        layout: &ImageLayout,
        cells: &mut dyn crate::CellSurface,
        theme: &crate::Theme,
    ) -> ImagePaintResult {
        if cells.emit_graphics(layout.area, self) {
            return ImagePaintResult::Painted;
        }
        self.paint_fallback_cells(layout.area, cells, theme);
        ImagePaintResult::Unsupported
    }

    /// Paint [`Self::fallback_text`] into `area`, centered horizontally
    /// on the vertical middle row and clipped to `area`'s width. An empty
    /// fallback or a zero-size `area` paints nothing.
    #[cfg(feature = "tui")]
    pub(crate) fn paint_fallback_cells(
        &self,
        area: Rect,
        cells: &mut dyn crate::CellSurface,
        theme: &crate::Theme,
    ) {
        let x = area.x.max(0.0).round() as u16;
        let y = area.y.max(0.0).round() as u16;
        let width = area.width.max(0.0).round() as u16;
        let height = area.height.max(0.0).round() as u16;
        if width == 0 || height == 0 || self.fallback_text.is_empty() {
            return;
        }
        let visible = crate::text_util::display_width(&self.fallback_text);
        let start_col = (width as usize).saturating_sub(visible) / 2;
        let style = crate::cell_surface::solid_style(theme.foreground, theme.background);
        cells.put_str_clipped(
            x + start_col as u16,
            y + height / 2,
            &self.fallback_text,
            width - start_col as u16,
            style,
        );
    }
}

impl crate::primitive::Primitive for Image {
    type Layout = ImageLayout;
    type Event = ImageHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Image(self.clone())
    }

    /// On a cell grid, [`Self::fallback_text`]'s measured width by one
    /// row — the size that paints wherever the terminal cannot rasterise
    /// the image, and the only one with a defined cell mapping; an empty
    /// fallback reports no size. On a pixel surface, `intrinsic_size`
    /// when known.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        if ctx.char_width <= 1.0 {
            if self.fallback_text.is_empty() {
                return crate::NaturalSize::NONE;
            }
            let width = ctx.text_width(&self.fallback_text, crate::FontRole::Chrome);
            return crate::NaturalSize::new(Some(width), Some(ctx.editor_line_height()));
        }
        match self.intrinsic_size {
            Some((w, h)) => crate::NaturalSize::new(Some(w as f32), Some(h as f32)),
            None => crate::NaturalSize::NONE,
        }
    }

    fn layout(&self, _ctx: &crate::primitive::LayoutCtx, rect: Rect) -> ImageLayout {
        Image::layout(self, rect)
    }

    fn paint_zone(&self, layout: &ImageLayout, _units: crate::primitive::Units) -> Option<Rect> {
        Some(layout.area)
    }

    fn paint(
        &self,
        layout: &ImageLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        _env: &crate::PaintEnv,
    ) {
        self.paint_reporting(layout, surface);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ImageLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        self.paint_cells_reporting(layout, cells, env.theme());
    }
}

crate::non_interactive!(Image);

/// Scale `(iw, ih)` to fit `bounds` — `contain` picks the smaller of the
/// two axis scales (never overflows `bounds`), `!contain` (cover) picks
/// the larger (never underflows `bounds`, may overflow one axis). Result
/// is centered within `bounds` on both axes, matching CSS
/// `object-fit: contain|cover` + `object-position: center`.
fn fit_within(bounds: Rect, iw: f32, ih: f32, contain: bool) -> Rect {
    let scale_x = bounds.width / iw;
    let scale_y = bounds.height / ih;
    let scale = if contain {
        scale_x.min(scale_y)
    } else {
        scale_x.max(scale_y)
    };
    let w = iw * scale;
    let h = ih * scale;
    let x = bounds.x + (bounds.width - w) / 2.0;
    let y = bounds.y + (bounds.height - h) / 2.0;
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(fit: ImageFit, intrinsic_size: Option<(u32, u32)>) -> Image {
        Image {
            id: WidgetId::new("img"),
            source: ImageSource::Path("/tmp/logo.png".into()),
            intrinsic_size,
            fit,
            fallback_text: "[LOGO]".into(),
        }
    }

    #[test]
    fn no_intrinsic_size_fills_bounds_regardless_of_fit() {
        let bounds = Rect::new(0.0, 0.0, 40.0, 20.0);
        let layout = image(ImageFit::Contain, None).layout(bounds);
        assert_eq!(layout.bounds, bounds);
    }

    #[test]
    fn fill_always_matches_bounds() {
        let bounds = Rect::new(5.0, 5.0, 40.0, 20.0);
        let layout = image(ImageFit::Fill, Some((100, 50))).layout(bounds);
        assert_eq!(layout.bounds, bounds);
    }

    #[test]
    fn none_paints_at_intrinsic_size_anchored_top_left() {
        let bounds = Rect::new(5.0, 5.0, 40.0, 20.0);
        let layout = image(ImageFit::None, Some((100, 50))).layout(bounds);
        assert_eq!(layout.bounds, Rect::new(5.0, 5.0, 100.0, 50.0));
    }

    #[test]
    fn contain_never_overflows_bounds_and_preserves_aspect_ratio() {
        // Intrinsic 200x100 (2:1) into a 40x40 square — contain must
        // shrink to 40x20 (limited by height... wait width) — width
        // scale = 40/200 = 0.2, height scale = 40/100 = 0.4, contain
        // picks the smaller (0.2) => 40x20.
        let bounds = Rect::new(0.0, 0.0, 40.0, 40.0);
        let layout = image(ImageFit::Contain, Some((200, 100))).layout(bounds);
        assert_eq!(layout.bounds.width, 40.0);
        assert_eq!(layout.bounds.height, 20.0);
        // Centered vertically within the 40-tall bounds.
        assert_eq!(layout.bounds.y, 10.0);
        assert_eq!(layout.bounds.x, 0.0);
        assert!(layout.bounds.width <= bounds.width);
        assert!(layout.bounds.height <= bounds.height);
    }

    #[test]
    fn cover_fills_bounds_on_the_shorter_axis_and_overflows_the_other() {
        // Same 200x100 image into the same 40x40 square — cover picks
        // the larger scale (0.4) => 80x40, overflowing width.
        let bounds = Rect::new(0.0, 0.0, 40.0, 40.0);
        let layout = image(ImageFit::Cover, Some((200, 100))).layout(bounds);
        assert_eq!(layout.bounds.width, 80.0);
        assert_eq!(layout.bounds.height, 40.0);
        // Centered horizontally, so it overflows both left and right.
        assert_eq!(layout.bounds.x, -20.0);
        assert_eq!(layout.bounds.y, 0.0);
    }

    #[test]
    fn zero_size_intrinsic_falls_back_to_bounds() {
        let bounds = Rect::new(0.0, 0.0, 40.0, 20.0);
        let layout = image(ImageFit::Contain, Some((0, 100))).layout(bounds);
        assert_eq!(layout.bounds, bounds);
    }

    #[test]
    fn hit_test_inside_resolved_bounds_is_image() {
        let bounds = Rect::new(5.0, 5.0, 40.0, 20.0);
        let layout = image(ImageFit::Fill, Some((100, 50))).layout(bounds);
        assert_eq!(layout.hit_test(10.0, 10.0), ImageHit::Image);
    }

    #[test]
    fn hit_test_outside_resolved_bounds_is_empty() {
        let bounds = Rect::new(5.0, 5.0, 40.0, 20.0);
        let layout = image(ImageFit::Fill, Some((100, 50))).layout(bounds);
        assert_eq!(layout.hit_test(0.0, 0.0), ImageHit::Empty);
    }
}

#[cfg(all(test, feature = "tui"))]
mod cell_tests {
    use super::*;
    use crate::backend::ImagePaintResult;
    use crate::theme::Theme;
    use crate::tui::BufferCellSurface;
    use crate::types::WidgetId;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as RtRect;

    fn image(fallback_text: &str) -> Image {
        Image {
            id: WidgetId::new("logo"),
            source: ImageSource::Path("/tmp/logo.png".into()),
            intrinsic_size: Some((16, 16)),
            fit: ImageFit::Contain,
            fallback_text: fallback_text.into(),
        }
    }

    /// Paints `image` into `buf` through a surface with no graphics
    /// channel, so only the fallback text can paint.
    fn paint_image(
        buf: &mut Buffer,
        area: RtRect,
        image: &Image,
        theme: &Theme,
    ) -> ImagePaintResult {
        let rect = Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        );
        let layout = image.layout(rect);
        let mut cells = BufferCellSurface::new(buf);
        image.paint_cells_reporting(&layout, &mut cells, theme)
    }

    fn screen_text(buf: &Buffer, area: RtRect) -> String {
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
        }
        out
    }

    #[test]
    fn a_surface_without_graphics_reports_unsupported() {
        let mut buf = Buffer::empty(RtRect::new(0, 0, 20, 3));
        let theme = Theme::default();
        let result = paint_image(&mut buf, RtRect::new(0, 0, 20, 3), &image("[LOGO]"), &theme);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn fallback_text_paints_into_the_rect() {
        let area = RtRect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        paint_image(&mut buf, area, &image("[LOGO]"), &theme);
        assert!(
            screen_text(&buf, area).contains("[LOGO]"),
            "fallback text should be painted somewhere in the rect:\n{}",
            screen_text(&buf, area)
        );
    }

    #[test]
    fn empty_fallback_text_paints_nothing() {
        let area = RtRect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let result = paint_image(&mut buf, area, &image(""), &theme);
        assert_eq!(result, ImagePaintResult::Unsupported);
        assert!(screen_text(&buf, area).trim().is_empty());
    }

    #[test]
    fn zero_size_rect_does_not_panic() {
        let area = RtRect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(RtRect::new(0, 0, 1, 1));
        let theme = Theme::default();
        let result = paint_image(&mut buf, area, &image("[LOGO]"), &theme);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }
}
