//! Core Graphics / ImageIO rasteriser for [`crate::primitives::image::Image`]
//! (#962).
//!
//! #662 scoped the `Image` rasteriser to GTK only for its first pass;
//! #739 ported it to Win-GUI via WIC. macOS's own gap sat open the
//! longest: #802 replaced a reachable `todo!()` here with an honest
//! [`ImagePaintResult::Unsupported`], but that made macOS the *only*
//! backend that painted nothing at all for an unrecognised source — TUI
//! at least paints `image.fallback_text` for its own categorical
//! `Unsupported` case (see [`crate::backend::Backend::draw_image`]'s doc
//! comment). This module closes that gap with a real decoder.
//!
//! Port of [`crate::win::image::draw_image`], which is itself a port of
//! [`crate::gtk::image::draw_image`]: decode `image.source`'s encoded
//! bytes through the platform's native image decoder — `CGImageSource`
//! (ImageIO) here, WIC there, `gdk_pixbuf` in GTK — then paint the
//! decoded bitmap into `image.layout(rect)`'s resolved target rect. Same
//! collapse-to-`Unsupported` posture on any decode failure (missing
//! file, corrupt bytes, unrecognised format): `CGImageSourceCreateImageAtIndex`
//! reports all of these the same way, as a null return, and this
//! rasteriser doesn't distinguish further.
//!
//! Painting is always clipped to the caller's `rect` — the only
//! [`crate::primitives::image::ImageFit`] variants that can extend past
//! it are `Cover` and `None`, and neither should bleed into whatever the
//! host paints next to it. Reuses [`super::backend::ns_push_clip`]/
//! [`super::backend::ns_pop_clip`] — the same save/clip/restore pair
//! every other clipping rasteriser in `macos::` uses — rather than
//! hand-rolling a third clip helper.
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
use core_graphics::data_provider::CGDataProvider;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use core_graphics::sys::{CGContextRef, CGDataProviderRef, CGImageRef};
use foreign_types::ForeignType;

use super::backend::{ns_pop_clip, ns_push_clip};
use crate::backend::ImagePaintResult;
use crate::event::Rect;
use crate::primitives::image::{Image, ImageSource};

/// Opaque ImageIO type — see the module doc's "Why raw FFI" section.
enum CGImageSource {}
type CGImageSourceRef = *mut CGImageSource;

/// Decode `source`'s encoded bytes into a [`CGImage`], or `None` on any
/// failure. See the module docs for why every failure mode collapses to
/// the same `None` rather than a typed error.
fn decode_image(source: &ImageSource) -> Option<CGImage> {
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

/// Paint `image` within `rect` (points, target-relative). See the module
/// docs.
///
/// Decoding happens before `ctx` is ever touched, so a decode failure or
/// a zero-size `rect` returns [`ImagePaintResult::Unsupported`] without
/// requiring a live context — `ctx` only needs to be valid when this
/// function is actually about to paint (checked with a `debug_assert!`
/// right before the CoreGraphics calls that dereference it), unlike
/// [`super::minimap::draw_minimap`], which touches `ctx` unconditionally
/// and so asserts on it up front.
///
/// # Safety
/// If a decodable `image.source` and non-zero `rect` cause this function
/// to actually paint, `ctx` must be a valid, live `CGContextRef` for the
/// duration of the call.
pub unsafe fn draw_image(ctx: CGContextRef, rect: Rect, image: &Image) -> ImagePaintResult {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return ImagePaintResult::Unsupported;
    }
    let Some(cg_image) = decode_image(&image.source) else {
        return ImagePaintResult::Unsupported;
    };

    debug_assert!(
        !ctx.is_null(),
        "macos::image::draw_image about to paint a decoded image with a null CGContextRef",
    );

    let dest = image.layout(rect).bounds;
    let dest_rect = CGRect::new(
        &CGPoint::new(dest.x as f64, dest.y as f64),
        &CGSize::new(dest.width as f64, dest.height as f64),
    );

    // SAFETY: `ctx` is valid per this function's own contract; `cg_image`
    // is a live, owned `CGImage` for the duration of this call.
    ns_push_clip(ctx, rect);
    CGContextDrawImage(ctx, dest_rect, cg_image.as_ptr());
    ns_pop_clip(ctx);

    ImagePaintResult::Painted
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
// `macos::minimap`'s own `extern "C"` block).
extern "C" {
    fn CGContextDrawImage(c: CGContextRef, rect: CGRect, image: CGImageRef);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::*;
    use crate::primitives::image::ImageFit;
    use crate::types::WidgetId;

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
        let idat = zlib_store(&raw);
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

    /// The impl this replaces reported `Unsupported` unconditionally
    /// (quadraui#802) — macOS was the only backend that painted nothing
    /// at all for `draw_image`. #962's positive replacement: a decodable
    /// source must actually paint pixels and report `Painted`.
    #[test]
    fn draw_image_from_bytes_paints_real_pixels() {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);

        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Fill);
        let rect = Rect::new(20.0, 20.0, 60.0, 60.0);

        // SAFETY: `surface.context_ptr()` is valid for the surface's
        // lifetime, which outlives this call.
        let result = unsafe { draw_image(surface.context_ptr(), rect, &img) };

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

    /// Corrupt / missing sources must still degrade to `Unsupported`
    /// rather than panicking — replaces #802's
    /// `mac_draw_image_reports_unsupported`/
    /// `draw_image_does_not_panic_and_reports_unsupported` coverage now
    /// that there's a real decode path to fail out of, rather than an
    /// unconditional stub.
    #[test]
    fn draw_image_from_missing_path_reports_unsupported() {
        let surface = BitmapSurface::new(W, H);
        let img = image(
            ImageSource::Path("/nonexistent/does-not-exist.png".into()),
            ImageFit::Contain,
        );
        // SAFETY: surface context valid for this call.
        let result =
            unsafe { draw_image(surface.context_ptr(), Rect::new(0.0, 0.0, 40.0, 40.0), &img) };
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_from_corrupt_bytes_reports_unsupported() {
        let surface = BitmapSurface::new(W, H);
        let img = image(ImageSource::Bytes(vec![0, 1, 2, 3]), ImageFit::Contain);
        // SAFETY: surface context valid for this call.
        let result =
            unsafe { draw_image(surface.context_ptr(), Rect::new(0.0, 0.0, 40.0, 40.0), &img) };
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn zero_size_rect_reports_unsupported_without_decoding() {
        let surface = BitmapSurface::new(W, H);
        let img = image(ImageSource::Bytes(tiny_png_bytes()), ImageFit::Contain);
        // SAFETY: surface context valid for this call.
        let result =
            unsafe { draw_image(surface.context_ptr(), Rect::new(0.0, 0.0, 0.0, 0.0), &img) };
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    /// `ImageFit::Cover` can extend past `rect` on one axis — painting
    /// must still stay clipped to it, mirroring `win::image`'s /
    /// `gtk::image`'s own clip contract (quadraui#962's acceptance bar).
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
        let result = unsafe { draw_image(surface.context_ptr(), rect, &img) };
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
}
