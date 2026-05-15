//! Offscreen test surface for the macOS backend.
//!
//! Wraps `CGBitmapContextCreate` to give tests a `CGContextRef` they
//! can paint into and a pixel buffer they can read back, with no
//! `NSWindow` or display server. Pairs with
//! [`super::MacBackend::enter_frame_scope`] so paint↔click round-trip
//! harnesses (added with each rasteriser ticket, #38–#43) can drive
//! the exact same code paths the live runner uses inside `drawRect:`.
//!
//! ## Pixel layout
//!
//! 32-bit per pixel, 8 bits per channel, byte order **R, G, B, A**
//! (`kCGImageAlphaPremultipliedLast`). Matches the `core-graphics`
//! crate's own `create_bitmap_context_test`, so callers reading raw
//! bytes can rely on that ordering on every macOS host.
//!
//! ## Coordinate convention
//!
//! Top-left origin, matching `QuadraView` (which sets `isFlipped` to
//! `YES`). The constructor applies a translate + vertical flip to the
//! CTM so callers paint in the same coordinate frame as the live
//! backend without branchy logic. Buffer scanlines are stored
//! top-down in memory (CG's default scanline layout), so the flipped
//! drawing space aligns directly with row indexing —
//! [`BitmapSurface::pixel`] reads `y` as a memory row with no
//! inversion.
//!
//! ## Why direct FFI
//!
//! The high-level `core_graphics::context::CGContext` wrapper exposes
//! its raw pointer only via the `foreign_types::ForeignType` trait,
//! which isn't a direct dependency of this crate. Rather than add the
//! dep, we own the `CGContextRef` end-to-end here — same style as
//! [`super::text`], which already speaks raw CG FFI to render text
//! against the borrowed `drawRect:` context.

use std::ffi::c_void;
use std::ptr;

use core_graphics::base::{kCGImageAlphaPremultipliedLast, CGFloat};
use core_graphics::geometry::CGRect;
use core_graphics::sys::{CGColorSpaceRef, CGContextRef};

/// 32 bpp, RGBA — see module docs.
const BYTES_PER_PIXEL: usize = 4;

/// Offscreen `CGBitmapContext` plus pixel-readback helpers.
///
/// Owns a `CGContextRef` plus a device-RGB colour space, both released
/// on drop. The pointer returned by [`Self::context_ptr`] is valid only
/// for the surface's lifetime — pass it to short-lived FFI or
/// [`super::MacBackend::enter_frame_scope`]; never retain it past the
/// surface's drop.
pub struct BitmapSurface {
    ctx: CGContextRef,
    color_space: CGColorSpaceRef,
    width: u32,
    height: u32,
}

impl BitmapSurface {
    /// Build a `width × height` RGBA surface initialised to transparent
    /// black (CG zero-fills the buffer on creation). Applies the
    /// top-left flip described in the module docs.
    ///
    /// # Panics
    ///
    /// - `width` and `height` must be non-zero.
    /// - Panics if `CGBitmapContextCreate` returns null (effectively
    ///   only possible if the system is out of memory for the buffer).
    pub fn new(width: u32, height: u32) -> Self {
        assert!(
            width > 0 && height > 0,
            "BitmapSurface dimensions must be non-zero, got {}×{}",
            width,
            height,
        );
        let bytes_per_row = (width as usize) * BYTES_PER_PIXEL;
        // SAFETY: every FFI call is to a documented CG API. The color
        // space is retained until Drop; the bitmap context owns its
        // own data buffer because we pass `null` for `data`.
        unsafe {
            let cs = CGColorSpaceCreateDeviceRGB();
            assert!(!cs.is_null(), "CGColorSpaceCreateDeviceRGB returned null");
            let ctx = CGBitmapContextCreate(
                ptr::null_mut(),
                width as usize,
                height as usize,
                8,
                bytes_per_row,
                cs,
                kCGImageAlphaPremultipliedLast,
            );
            assert!(
                !ctx.is_null(),
                "CGBitmapContextCreate returned null for {}×{}",
                width,
                height,
            );
            // Apply translate + vertical flip so callers paint with
            // top-left origin (matching `QuadraView`). Without this,
            // CG's native bottom-left origin would invert every paint
            // relative to the live runner — rasteriser code that
            // works in the window would draw upside-down in tests.
            CGContextTranslateCTM(ctx, 0.0, height as CGFloat);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            Self {
                ctx,
                color_space: cs,
                width,
                height,
            }
        }
    }

    /// Surface width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Surface height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Raw `CGContextRef` for FFI / backend integration. The pointer
    /// is owned by `self`; do not retain it past the surface's drop.
    pub fn context_ptr(&self) -> CGContextRef {
        self.ctx
    }

    /// Fill the entire surface with `(r, g, b, a)` in 0.0–1.0
    /// components. Convenience for tests that want a known background
    /// before painting on top.
    pub fn fill(&self, r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat) {
        let rect = CGRect::new(
            &core_graphics::geometry::CGPoint::new(0.0, 0.0),
            &core_graphics::geometry::CGSize::new(self.width as f64, self.height as f64),
        );
        // SAFETY: `self.ctx` is a valid bitmap context for the
        // lifetime of `self`.
        unsafe {
            CGContextSetRGBFillColor(self.ctx, r, g, b, a);
            CGContextFillRect(self.ctx, rect);
        }
    }

    /// Read pixel `(x, y)` in top-left coordinates as `(R, G, B, A)`.
    ///
    /// Buffer rows are laid out top-down in memory (the CG scanline
    /// convention) and the constructor's CTM flip aligns CG's
    /// drawing space with that layout — so `y` indexes the memory
    /// row directly with no inversion.
    ///
    /// # Panics
    ///
    /// If `(x, y)` is outside the surface.
    pub fn pixel(&self, x: u32, y: u32) -> (u8, u8, u8, u8) {
        assert!(
            x < self.width && y < self.height,
            "BitmapSurface::pixel ({}, {}) out of bounds for {}×{}",
            x,
            y,
            self.width,
            self.height,
        );
        let bytes = self.bytes();
        let row_stride = self.width as usize * BYTES_PER_PIXEL;
        let base = (y as usize) * row_stride + (x as usize) * BYTES_PER_PIXEL;
        (
            bytes[base],
            bytes[base + 1],
            bytes[base + 2],
            bytes[base + 3],
        )
    }

    /// Borrow the raw pixel buffer. Row 0 is the **top** scanline
    /// (matches the [`Self::pixel`] coordinate frame and the live
    /// `QuadraView`'s `isFlipped = YES`).
    ///
    /// Length is exactly `width × height × 4`.
    pub fn bytes(&self) -> &[u8] {
        let row_stride = self.width as usize * BYTES_PER_PIXEL;
        let total = row_stride * self.height as usize;
        // SAFETY: `self.ctx` is a non-null bitmap context owned for
        // the lifetime of `self`. `CGBitmapContextGetData` returns the
        // pointer to the backing store, which is valid for the whole
        // context lifetime and at least `total` bytes long.
        unsafe {
            let ptr = CGBitmapContextGetData(self.ctx) as *const u8;
            std::slice::from_raw_parts(ptr, total)
        }
    }
}

impl Drop for BitmapSurface {
    fn drop(&mut self) {
        // SAFETY: both pointers were created in `new` and not handed
        // out elsewhere. Release order matches CG conventions
        // (context first — it depends on the color space).
        unsafe {
            CGContextRelease(self.ctx);
            CGColorSpaceRelease(self.color_space);
        }
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGColorSpaceRelease(cs: CGColorSpaceRef);

    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
    ) -> CGContextRef;
    fn CGBitmapContextGetData(context: CGContextRef) -> *mut c_void;

    fn CGContextRelease(c: CGContextRef);
    fn CGContextTranslateCTM(c: CGContextRef, tx: CGFloat, ty: CGFloat);
    fn CGContextScaleCTM(c: CGContextRef, sx: CGFloat, sy: CGFloat);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: CGFloat,
        green: CGFloat,
        blue: CGFloat,
        alpha: CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::geometry::{CGPoint, CGSize};

    #[test]
    fn new_initialises_transparent_black() {
        let s = BitmapSurface::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(
                    s.pixel(x, y),
                    (0, 0, 0, 0),
                    "fresh surface pixel ({}, {}) should be transparent black",
                    x,
                    y,
                );
            }
        }
    }

    #[test]
    fn dimensions_reported() {
        let s = BitmapSurface::new(32, 16);
        assert_eq!(s.width(), 32);
        assert_eq!(s.height(), 16);
        assert_eq!(s.bytes().len(), 32 * 16 * 4);
    }

    #[test]
    fn fill_paints_expected_colour() {
        // Paint a full-surface red rectangle and confirm every pixel
        // reads back as opaque red (255, 0, 0, 255). Exercises the
        // RGBA byte ordering documented in the module header.
        let s = BitmapSurface::new(8, 8);
        s.fill(1.0, 0.0, 0.0, 1.0);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(
                    s.pixel(x, y),
                    (255, 0, 0, 255),
                    "pixel ({}, {}) should be opaque red after full fill",
                    x,
                    y,
                );
            }
        }
    }

    #[test]
    fn top_left_origin_for_partial_fill() {
        // Fill only the top half (top-left coords) and assert the top
        // half reads green while the bottom half stays transparent.
        // Proves both invariants: the CTM flip aligns CG's drawing
        // origin with the top-left convention, and `pixel()` reads
        // memory in the matching direction.
        let s = BitmapSurface::new(4, 4);
        // SAFETY: ctx valid for the surface's lifetime.
        unsafe {
            CGContextSetRGBFillColor(s.context_ptr(), 0.0, 1.0, 0.0, 1.0);
            CGContextFillRect(
                s.context_ptr(),
                CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(4.0, 2.0)),
            );
        }
        for x in 0..4 {
            assert_eq!(s.pixel(x, 0), (0, 255, 0, 255), "top row should be green");
            assert_eq!(
                s.pixel(x, 1),
                (0, 255, 0, 255),
                "second row should be green"
            );
            assert_eq!(s.pixel(x, 2), (0, 0, 0, 0), "third row should be untouched");
            assert_eq!(
                s.pixel(x, 3),
                (0, 0, 0, 0),
                "bottom row should be untouched"
            );
        }
    }

    /// `cargo test -p quadraui --features macos -- --ignored --nocapture macos::headless::tests::dump_smoke_ppm`
    ///
    /// Paints a four-corner colour grid plus a Core Text label into a
    /// 200×120 surface and writes `/tmp/quadraui_headless.ppm`. Open
    /// in Preview / Quick Look to visually confirm:
    /// - **Red** in the top-left, **green** top-right,
    ///   **blue** bottom-left, **yellow** bottom-right.
    /// - Label reads left-to-right (not mirrored) and is anchored to
    ///   the top of the surface (16pt down from the top edge).
    ///
    /// `#[ignore]`d so the default `cargo test` run stays fast +
    /// side-effect-free; the file write is opt-in.
    #[test]
    #[ignore = "writes /tmp/quadraui_headless.ppm — opt in with --ignored"]
    fn dump_smoke_ppm() {
        use super::super::text::{draw_text, make_font};
        use std::fs::File;
        use std::io::Write;

        const W: u32 = 200;
        const H: u32 = 120;

        let s = BitmapSurface::new(W, H);

        // Dark grey background — matches `QuadraView::drawRect:`
        // so the visual output reads like the live runner.
        s.fill(0.12, 0.12, 0.14, 1.0);

        // Four corner quadrants, 40×40 each, anchored to the four
        // corners. Distinct colours so orientation regressions are
        // immediately obvious.
        let q = 40.0;
        let corners = [
            (0.0, 0.0, 1.0, 0.0, 0.0),       // top-left, red
            (W as f64 - q, 0.0, 0.0, 1.0, 0.0), // top-right, green
            (0.0, H as f64 - q, 0.0, 0.0, 1.0), // bottom-left, blue
            (W as f64 - q, H as f64 - q, 1.0, 1.0, 0.0), // bottom-right, yellow
        ];
        // SAFETY: ctx valid for the surface's lifetime.
        unsafe {
            for &(x, y, r, g, b) in &corners {
                CGContextSetRGBFillColor(s.context_ptr(), r, g, b, 1.0);
                CGContextFillRect(
                    s.context_ptr(),
                    CGRect::new(&CGPoint::new(x, y), &CGSize::new(q, q)),
                );
            }
        }

        // Core Text label — exercises the #34 path through the test
        // surface so visual regressions in text rendering show up
        // here too.
        if let Some(font) = make_font("Menlo", 14.0) {
            // SAFETY: ctx + font valid; `draw_text` documents its
            // pointer requirements.
            unsafe {
                draw_text(
                    s.context_ptr(),
                    &font,
                    "quadraui · headless · 200x120",
                    16.0,
                    16.0,
                    (1.0, 1.0, 1.0, 1.0),
                );
            }
        }

        // Write PPM P6 — Apple Preview reads it natively. Drop the
        // alpha channel: P6 is RGB only.
        let path = "/tmp/quadraui_headless.ppm";
        let mut f = File::create(path).expect("create ppm file");
        writeln!(f, "P6").unwrap();
        writeln!(f, "{} {}", W, H).unwrap();
        writeln!(f, "255").unwrap();
        let bytes = s.bytes();
        let mut rgb = Vec::with_capacity((W * H * 3) as usize);
        for chunk in bytes.chunks_exact(4) {
            rgb.push(chunk[0]);
            rgb.push(chunk[1]);
            rgb.push(chunk[2]);
        }
        f.write_all(&rgb).expect("write ppm pixels");
        drop(f);
        eprintln!("wrote {} — launching Preview", path);
        // Best-effort launch via `open`. Failures (no Preview, sandbox)
        // are non-fatal — the file is on disk either way.
        let _ = std::process::Command::new("open").arg(path).status();
    }

    #[test]
    fn integrates_with_mac_backend_frame_scope() {
        // Drive a paint through `MacBackend::enter_frame_scope` — the
        // exact path future rasteriser tests will use. The closure
        // recovers the stashed CG pointer and paints a blue rect via
        // raw FFI, the same shape #38–#43 rasterisers will adopt.
        use super::super::MacBackend;
        let mut backend = MacBackend::new();
        let surface = BitmapSurface::new(4, 4);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let cg = b.current_cg();
            assert!(!cg.is_null());
            // SAFETY: `cg` is the surface's context, valid for the
            // closure's duration via the surface's lifetime.
            unsafe {
                CGContextSetRGBFillColor(cg, 0.0, 0.0, 1.0, 1.0);
                CGContextFillRect(
                    cg,
                    CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(4.0, 4.0)),
                );
            }
        });
        // After the scope exits the pointer is cleared.
        assert!(backend.current_cg().is_null());
        assert_eq!(surface.pixel(1, 1), (0, 0, 255, 255));
    }
}
