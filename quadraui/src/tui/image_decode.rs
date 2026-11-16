//! Decode + downscale raster images for the Kitty graphics protocol.
//!
//! Behind the `tui-images` Cargo feature (see that feature's own
//! comment in `Cargo.toml`) so a plain `tui` build never pulls in the
//! `image` crate. [`decode_and_downscale`] turns an [`ImageSource`]
//! (PNG, JPEG, GIF — first frame only, animation is out of scope here
//! the same way it is for [`crate::primitives::image::Image`] — or
//! WebP) into tightly-packed RGBA8 pixels, already shrunk to fit a
//! caller-given target pixel size; [`crate::tui::kitty_graphics`] turns
//! that pixel buffer into protocol bytes. `tui::image::draw_image_kitty`
//! is the paint-path caller that wires both steps together.
//!
//! # Downscale, never upscale
//!
//! The target size is a cap, not a request to stretch: a source already
//! smaller than the target pixel rect is decoded at its own size, never
//! blown up. [`MAX_TARGET_DIMENSION_PX`] additionally caps the target
//! itself, so a caller-computed `cell rect × cell pixel size` that
//! works out to something absurd (a huge terminal, huge cell pixels)
//! can't force a huge intermediate allocation.
//!
//! # Decode cache
//!
//! [`decode_and_downscale_cached`] routes through
//! [`crate::image_cache::ImageCache`] — the same shared LRU every pixel
//! backend's `Backend::draw_image` uses, keyed the same way (source +
//! target size + scale) — so repainting the same image at the same size
//! across frames decodes once. A failed decode is never cached (see
//! `ImageCache::get_or_decode`'s own doc), so a transient failure (a
//! path that doesn't exist yet) is retried on the next paint rather than
//! wedged.

use crate::image_cache::ImageCache;
use crate::primitives::image::ImageSource;

/// Safety cap on the pixel dimensions a decode will ever resize to —
/// see the module doc's "Downscale, never upscale" section.
pub(crate) const MAX_TARGET_DIMENSION_PX: u32 = 2048;

/// Decoded, already-downscaled image: tightly-packed RGBA8 pixels,
/// row-major, exactly `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

/// Why [`decode_and_downscale`] failed — maps to a caller's
/// `Image::fallback_text` the same way every other backend's decode
/// failure does (see `crate::primitives::image::Image::fallback_text`'s
/// own doc).
#[derive(Debug)]
pub(crate) enum DecodeError {
    /// `ImageSource::Path` couldn't be read from disk.
    Io(std::io::Error),
    /// The bytes (from either `ImageSource` variant) aren't a format
    /// `image` recognises, or are corrupt.
    Decode(image::ImageError),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(e) => write!(f, "failed to read image source: {e}"),
            DecodeError::Decode(e) => write!(f, "failed to decode image: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DecodeError::Io(e) => Some(e),
            DecodeError::Decode(e) => Some(e),
        }
    }
}

impl From<image::ImageError> for DecodeError {
    fn from(e: image::ImageError) -> Self {
        DecodeError::Decode(e)
    }
}

fn load_bytes(source: &ImageSource) -> Result<Vec<u8>, DecodeError> {
    match source {
        ImageSource::Bytes(bytes) => Ok(bytes.clone()),
        ImageSource::Path(path) => std::fs::read(path).map_err(DecodeError::Io),
    }
}

fn clamp_target((w, h): (u32, u32)) -> (u32, u32) {
    (
        w.clamp(1, MAX_TARGET_DIMENSION_PX),
        h.clamp(1, MAX_TARGET_DIMENSION_PX),
    )
}

/// Decode `source` (sniffed from content, like every other backend's
/// image loader) and downscale to fit within `target_px`, preserving
/// aspect ratio. Never upscales — see the module doc.
pub(crate) fn decode_and_downscale(
    source: &ImageSource,
    target_px: (u32, u32),
) -> Result<DecodedImage, DecodeError> {
    let bytes = load_bytes(source)?;
    let img = image::load_from_memory(&bytes)?;
    let (tw, th) = clamp_target(target_px);
    let rgba = if img.width() > tw || img.height() > th {
        img.resize(tw, th, image::imageops::FilterType::Triangle)
            .to_rgba8()
    } else {
        img.to_rgba8()
    };
    Ok(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// [`decode_and_downscale`], cached in `cache` by `(source, target size,
/// scale)` — see the module doc's "Decode cache" section.
pub(crate) fn decode_and_downscale_cached<'a>(
    source: &ImageSource,
    target_px: (u32, u32),
    scale: f32,
    cache: &'a mut ImageCache<DecodedImage>,
) -> Result<&'a DecodedImage, DecodeError> {
    let (tw, th) = clamp_target(target_px);
    let mut last_err = None;
    let decoded = cache.get_or_decode(source, tw, th, scale, || {
        match decode_and_downscale(source, (tw, th)) {
            Ok(img) => Some(img),
            Err(err) => {
                last_err = Some(err);
                None
            }
        }
    });
    decoded.ok_or_else(|| {
        last_err.expect("a cache miss that returns None always records its error first")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny solid-colour PNG, generated at test time rather than
    /// checked in as a binary fixture — same approach as
    /// `gtk::image`'s own test helper.
    fn tiny_png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("encode tiny test PNG");
        out
    }

    #[test]
    fn decodes_bytes_source_to_exact_rgba() {
        let png = tiny_png_bytes(4, 4);
        let decoded = decode_and_downscale(&ImageSource::Bytes(png), (40, 40))
            .expect("tiny PNG should decode");
        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 4);
        assert_eq!(decoded.rgba.len(), 4 * 4 * 4);
        // Every pixel is opaque red.
        for px in decoded.rgba.chunks_exact(4) {
            assert_eq!(px, &[255, 0, 0, 255]);
        }
    }

    #[test]
    fn downscales_to_fit_target_preserving_aspect_ratio() {
        let png = tiny_png_bytes(200, 100);
        let decoded = decode_and_downscale(&ImageSource::Bytes(png), (40, 40))
            .expect("should decode and downscale");
        assert!(decoded.width <= 40);
        assert!(decoded.height <= 40);
        // 200x100 is 2:1 — fitting 40x40 should land on 40x20.
        assert_eq!(decoded.width, 40);
        assert_eq!(decoded.height, 20);
    }

    #[test]
    fn never_upscales_a_source_smaller_than_the_target() {
        let png = tiny_png_bytes(4, 4);
        let decoded = decode_and_downscale(&ImageSource::Bytes(png), (400, 400))
            .expect("should decode without upscaling");
        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 4);
    }

    #[test]
    fn corrupt_bytes_report_a_decode_error() {
        let err = decode_and_downscale(&ImageSource::Bytes(vec![0, 1, 2, 3]), (40, 40))
            .expect_err("corrupt bytes must fail to decode");
        assert!(matches!(err, DecodeError::Decode(_)));
    }

    #[test]
    fn missing_path_reports_an_io_error() {
        let err = decode_and_downscale(
            &ImageSource::Path("/nonexistent/does-not-exist.png".into()),
            (40, 40),
        )
        .expect_err("missing path must fail to read");
        assert!(matches!(err, DecodeError::Io(_)));
    }

    #[test]
    fn cached_decode_survives_source_becoming_unreadable_on_disk() {
        let path = std::env::temp_dir().join(format!(
            "quadraui_image_decode_cache_test_{}_{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        std::fs::write(&path, tiny_png_bytes(4, 4)).expect("write tiny png fixture");

        let mut cache: ImageCache<DecodedImage> = ImageCache::with_capacity(4);
        let source = ImageSource::Path(path.clone());

        let first = decode_and_downscale_cached(&source, (40, 40), 1.0, &mut cache)
            .expect("first decode should succeed")
            .clone();

        // Corrupt the file in place — a fresh decode from this point on
        // would fail.
        std::fs::write(&path, b"not a png").expect("corrupt fixture on disk");

        let second = decode_and_downscale_cached(&source, (40, 40), 1.0, &mut cache)
            .expect("second call at the same size/scale must hit the cache");
        assert_eq!(&first, second);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn different_target_size_is_a_distinct_cache_entry() {
        let png = tiny_png_bytes(200, 200);
        let mut cache: ImageCache<DecodedImage> = ImageCache::with_capacity(4);
        let source = ImageSource::Bytes(png);

        let first = decode_and_downscale_cached(&source, (40, 40), 1.0, &mut cache)
            .expect("first size should decode")
            .clone();
        let second = decode_and_downscale_cached(&source, (80, 80), 1.0, &mut cache)
            .expect("second size should decode");
        assert_ne!(first.width, second.width);
    }

    #[test]
    fn failed_decode_is_not_cached_and_is_retried() {
        let mut cache: ImageCache<DecodedImage> = ImageCache::with_capacity(4);
        let bad = ImageSource::Bytes(vec![0, 1, 2, 3]);
        assert!(decode_and_downscale_cached(&bad, (40, 40), 1.0, &mut cache).is_err());
        // A second attempt with the same (failing) source must not panic
        // on a stale cached error — it simply fails again.
        assert!(decode_and_downscale_cached(&bad, (40, 40), 1.0, &mut cache).is_err());
    }
}
