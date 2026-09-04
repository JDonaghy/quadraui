//! Direct2D + WIC rasteriser for [`crate::primitives::image::Image`]
//! (#739).
//!
//! Port of [`crate::gtk::image::draw_image`]: decode `image.source`'s
//! encoded bytes through the platform's native image decoder — WIC
//! (`IWICImagingFactory`/`IWICBitmapDecoder`/`IWICFormatConverter`) here,
//! `gdk_pixbuf` there — convert to a pixel format the 2D API can paint
//! directly, and stretch it into `image.layout(rect)`'s resolved target
//! rect. Same collapse-to-`Unsupported` posture on any decode failure
//! (missing file, corrupt bytes, unrecognised format): WIC reports all of
//! these as an `Err` at one step or another, and this rasteriser doesn't
//! distinguish further — see [`crate::backend::Backend::draw_image`]'s
//! doc comment for why.
//!
//! Painting is always clipped to the caller's `rect` — the only
//! [`crate::primitives::image::ImageFit`] variants that can extend past
//! it are `Cover` and `None`, and neither should bleed into whatever the
//! host paints next to it (same [`super::text::push_clip`]/`pop_clip`
//! pair `super::panel`/`super::sidebar_panel` use for their own
//! overflow-prone content).
//!
//! # Why WIC instead of `ID2D1RenderTarget::CreateBitmap` from raw pixels
//!
//! `CreateBitmap` takes already-decoded pixel bytes at a known stride —
//! it has no PNG/JPEG decoder of its own. WIC is Direct2D's designated
//! decode partner for exactly this (`CreateBitmapFromWicBitmap`, gated in
//! the `windows` crate on the `Win32_Graphics_Imaging` feature added
//! alongside this module — see `Cargo.toml`'s comment on that feature).
//!
//! # Why COM must be initialized first
//!
//! `CoCreateInstance(CLSID_WICImagingFactory, ..)` needs
//! `CoInitializeEx` to have run on the calling thread first, exactly like
//! the `IFileOpenDialog`/`IFileSaveDialog` construction in
//! `super::services`. Reuses that module's
//! [`super::services::ensure_com_initialized`] thread-local guard rather
//! than duplicating it — both call sites want "COM is ready on this
//! thread," not two independent `Cell<bool>` dances.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
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

use super::services::ensure_com_initialized;
use super::text::{pop_clip, push_clip};
use crate::backend::ImagePaintResult;
use crate::event::Rect;
use crate::primitives::image::{Image, ImageSource};

/// Decode `source`'s encoded bytes into an [`ID2D1Bitmap`] bound to
/// `target`, or `None` on any failure. `Path` and `Bytes` funnel through
/// the same in-memory-stream path (`SHCreateMemStream` +
/// `CreateDecoderFromStream`) rather than WIC's separate
/// `CreateDecoderFromFilename` entry point, so both sources are sniffed
/// from content identically — mirroring
/// [`crate::gtk::image::load_pixbuf`]'s "the decoder sniffs the format,
/// no separate MIME hint" contract (see `primitives::image`'s module
/// docs).
fn decode_bitmap(target: &ID2D1RenderTarget, source: &ImageSource) -> Option<ID2D1Bitmap> {
    let bytes: Vec<u8> = match source {
        ImageSource::Path(path) => std::fs::read(path).ok()?,
        ImageSource::Bytes(bytes) => bytes.clone(),
    };

    ensure_com_initialized();
    let factory: IWICImagingFactory = unsafe {
        CoCreateInstance(
            &CLSID_WICImagingFactory,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .ok()?
    };
    let stream = unsafe { SHCreateMemStream(Some(&bytes)) }?;
    let decoder = unsafe {
        factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
    }
    .ok()?;
    let frame = unsafe { decoder.GetFrame(0) }.ok()?;

    // Direct2D bitmaps want a premultiplied-alpha BGRA buffer; the source
    // frame could be indexed, grayscale, straight-alpha, ... — the format
    // converter normalises whatever WIC decoded to the one format
    // `CreateBitmapFromWicBitmap` is guaranteed to accept.
    let converter = unsafe { factory.CreateFormatConverter() }.ok()?;
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

    unsafe { target.CreateBitmapFromWicBitmap(&converter, None) }.ok()
}

/// Paint `image` within `rect` (DIPs, target-relative), honoring
/// `image.fit`. See the module docs.
pub fn draw_image(target: &ID2D1RenderTarget, rect: Rect, image: &Image) -> ImagePaintResult {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return ImagePaintResult::Unsupported;
    }
    let Some(bitmap) = decode_bitmap(target, &image.source) else {
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
    unsafe {
        target.DrawBitmap(
            &bitmap,
            Some(&dest_f),
            1.0,
            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            None,
        );
    }
    pop_clip(target);

    ImagePaintResult::Painted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::image::ImageFit;
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
    /// panicked (quadraui#739).
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
                result = draw_image(target, rect, &img);
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
                result = draw_image(target, Rect::new(0.0, 0.0, 40.0, 40.0), &img);
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
                result = draw_image(target, Rect::new(0.0, 0.0, 40.0, 40.0), &img);
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
                result = draw_image(target, Rect::new(0.0, 0.0, 0.0, 0.0), &img);
            })
            .expect("paint attempt");
        assert_eq!(result, ImagePaintResult::Unsupported);
    }
}
