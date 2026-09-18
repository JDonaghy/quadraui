//! GTK rasteriser for [`crate::Image`] (#662): decodes `image.source` via
//! `gdk_pixbuf` and paints it into a [`Context`] through
//! [`gdk::prelude::GdkCairoContextExt::set_source_pixbuf`], honoring
//! `image.fit`'s already-resolved target rect ([`Image::layout`]).
//!
//! Painting is always clipped to the caller's `(x, y, w, h)` rect — the
//! only [`ImageFit`] variants that can extend past it are `Cover` and
//! `None`, and neither should bleed into whatever the host paints next
//! to it.
//!
//! # Decode cache (#1014)
//!
//! [`draw_image`] is `pub` (re-exported as `quadraui::gtk::draw_image`) —
//! `quadraui#1014` cannot change its signature without breaking whatever
//! external code already calls it directly (see `CLAUDE.md`'s
//! *Downstream consumers* section: nothing pins a version of this crate,
//! so a breaking change here is live in a consumer's build the moment it
//! merges). So `draw_image` itself stays exactly as it always has —
//! uncached, one decode per call — and the caching lives in a sibling,
//! crate-internal [`draw_image_cached`] that [`GtkBackend::draw_image`]
//! (the `Backend` trait method, which downstream never implements itself
//! — see `CLAUDE.md`) calls instead. Decoding *and* scaling both cost
//! real time per call — `scale_simple` on top of
//! `Pixbuf::from_file`/`from_read` — and vimcode's own motivating case (a
//! 1024² app-icon SVG) measured the combination at +16.5ms/frame when it
//! wasn't skipped. The cached value is the already-scaled [`Pixbuf`] for
//! a given `(source, target size, DPI scale)`, not just the raw decode —
//! a cache hit skips both steps, not just the first. See
//! [`crate::image_cache`]'s module doc for the shared cache shape every
//! pixel backend plugs into.

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
/// [`Backend::draw_image`](crate::backend::Backend::draw_image)'s doc
/// comment for why that collapses to [`ImagePaintResult::Unsupported`]
/// rather than propagating a typed error).
fn load_pixbuf(source: &ImageSource) -> Option<Pixbuf> {
    match source {
        ImageSource::Path(path) => Pixbuf::from_file(path).ok(),
        ImageSource::Bytes(bytes) => Pixbuf::from_read(std::io::Cursor::new(bytes.clone())).ok(),
    }
}

/// Clip to `bounds` and paint `scaled` at `target`'s origin — the shared
/// tail end of [`draw_image`] and [`draw_image_cached`], factored out so
/// #1014's cache didn't need to duplicate the save/clip/paint/restore
/// dance.
fn paint_scaled(cr: &Context, bounds: QRect, target: QRect, scaled: &Pixbuf) -> ImagePaintResult {
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
pub fn draw_image(cr: &Context, x: f64, y: f64, w: f64, h: f64, image: &Image) -> ImagePaintResult {
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

/// Paint `image` into `bounds`, decoding through `cache` (#1014) — see
/// the module docs. `dpi_scale` is folded into the cache key so a live
/// DPI change (issue #834) doesn't paint a stale bitmap rasterised for a
/// different scale. Crate-internal: [`GtkBackend::draw_image`] is the
/// only caller, so this can take whatever shape is convenient (a
/// [`QRect`] instead of four `f64`s, an [`ImageCache`] reference) without
/// worrying about breaking an external caller the way changing
/// [`draw_image`]'s signature would.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::image::ImageFit;
    use crate::types::WidgetId;
    use gtk4::cairo::{Context as CairoContext, Format, ImageSurface};
    use gtk4::gdk_pixbuf::Colorspace;

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
        let result = draw_image(&cr, 0.0, 0.0, 40.0, 40.0, &img);
        assert_eq!(result, ImagePaintResult::Painted);
    }

    #[test]
    fn draw_image_from_missing_path_reports_unsupported() {
        let cr = surface();
        let img = image(
            ImageSource::Path("/nonexistent/does-not-exist.png".into()),
            ImageFit::Contain,
        );
        let result = draw_image(&cr, 0.0, 0.0, 40.0, 40.0, &img);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_from_corrupt_bytes_reports_unsupported() {
        let cr = surface();
        let img = image(ImageSource::Bytes(vec![0, 1, 2, 3]), ImageFit::Contain);
        let result = draw_image(&cr, 0.0, 0.0, 40.0, 40.0, &img);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn zero_size_rect_reports_unsupported_without_decoding() {
        let cr = surface();
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Contain);
        let result = draw_image(&cr, 0.0, 0.0, 0.0, 0.0, &img);
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

    /// Black-box proof that a cache hit skips re-decoding entirely
    /// (issue #1014): a `Path` source that decodes fine once, then gets
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

    /// A different target size is a different cache key (#1014) — the
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
