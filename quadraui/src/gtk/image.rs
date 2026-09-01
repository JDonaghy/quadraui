//! GTK rasteriser for [`crate::Image`] (#662): decodes `image.source` via
//! `gdk_pixbuf` and paints it into a [`Context`] through
//! [`gdk::prelude::GdkCairoContextExt::set_source_pixbuf`], honoring
//! `image.fit`'s already-resolved target rect ([`Image::layout`]).
//!
//! Painting is always clipped to the caller's `(x, y, w, h)` rect — the
//! only [`ImageFit`] variants that can extend past it are `Cover` and
//! `None`, and neither should bleed into whatever the host paints next
//! to it.

use gtk4::cairo::Context;
use gtk4::gdk::prelude::GdkCairoContextExt;
use gtk4::gdk_pixbuf::{InterpType, Pixbuf};

use crate::backend::ImagePaintResult;
use crate::event::Rect as QRect;
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

/// Paint `image` into `(x, y, w, h)`. See the module docs.
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

    if cr.save().is_err() {
        return ImagePaintResult::Unsupported;
    }
    cr.rectangle(x, y, w, h);
    cr.clip();
    cr.set_source_pixbuf(&scaled, target.x as f64, target.y as f64);
    let _ = cr.paint();
    let _ = cr.restore();

    ImagePaintResult::Painted
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
}
