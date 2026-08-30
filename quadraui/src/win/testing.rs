//! Headless Direct2D test surface (issue #24).
//!
//! Per `docs/TESTING.md`'s backend-testability requirement, every backend
//! must support paint-to-memory so tests don't need a real display —
//! `gtk::testing`/`gtk::multi_section_view::tests` use a headless
//! `cairo::ImageSurface`, `tui::split::tests` paints into a `ratatui::Buffer`.
//! [`HeadlessSurface`] is the Win-GUI equivalent: a real `ID2D1RenderTarget`
//! that paints into an in-process pixel buffer, no `HWND`/`WM_PAINT`/GPU
//! required.
//!
//! # Why a DC render target, not `CreateWicBitmapRenderTarget`
//!
//! A WIC bitmap render target needs COM (`CoInitializeEx`) and the
//! `Win32_Graphics_Imaging` crate feature. `ID2D1Factory::CreateDCRenderTarget`
//! only needs GDI (`Win32_Graphics_Gdi`, already pulled in by the `win`
//! feature for [`super::backend::WinBackend::attach_surface`]'s
//! `GetClientRect`) plus an in-memory device context bound to a
//! `CreateDIBSection` bitmap. `CreateDIBSection` hands back a raw pointer
//! to the pixel buffer directly ([`HeadlessSurface::pixel_at`]), so reading
//! a pixel back is a pointer offset — no `IWICBitmap::Lock`/copy step, and
//! no COM apartment-per-thread bookkeeping for `cargo test`'s
//! multi-threaded test runner to trip over.
//!
//! # Software, not hardware
//!
//! [`D2D1_RENDER_TARGET_TYPE_SOFTWARE`] forces Direct2D's WARP software
//! rasteriser — no GPU, no driver, no display adapter needed, only a
//! CPU. `CreateCompatibleDC(None)` still asks GDI for a DC "compatible
//! with the current screen", but that's a virtual/basic display device
//! every Windows host has (including headless CI runners) — it does not
//! require a physically attached monitor.
//!
//! This module only exists on `target_os = "windows"` — see `super`'s
//! `mod testing;` declaration and `backend.rs`'s module docs for why the
//! rest of this repo's `--features win` compile gate stays meaningful
//! without a Windows host. It is `pub` (like [`crate::tui::testing`] and
//! [`crate::gtk::testing`]) rather than `pub(crate)`, both to match those
//! siblings' convention and so a plain `cargo build`/`clippy --features
//! win` (which compiles this module as part of the library regardless of
//! whether any `#[cfg(test)]` block currently calls it) never flags it as
//! dead code.

use windows::core::Result as WinResult;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
};

use crate::event::Rect;
use crate::Color;

/// An offscreen `ID2D1DCRenderTarget` bound to a top-down 32bpp DIB
/// section. Construct with [`Self::new`], paint with [`Self::paint`] (or
/// the [`Self::fill_rect`] convenience for the common case), read a
/// painted pixel back with [`Self::pixel_at`].
///
/// Deliberately *not* wired into [`super::backend::WinBackend`] itself:
/// that backend's `Surface` is built around `ID2D1HwndRenderTarget`
/// (window-bound) and every `Backend::draw_*` rasteriser is still a
/// `todo!()` stub (see `backend.rs`'s module docs) — there is nothing yet
/// for a `WinBackend`-level driver to paint. `HeadlessSurface` is the
/// lower-level primitive future rasteriser tests build on, the same role
/// `cairo::ImageSurface` played for the GTK backend before `GtkDriver`
/// existed.
pub struct HeadlessSurface {
    /// Kept alive only because `ID2D1DCRenderTarget` was created from it.
    #[allow(dead_code)]
    factory: ID2D1Factory,
    target: ID2D1DCRenderTarget,
    hdc: HDC,
    bitmap: HBITMAP,
    /// Raw pointer into the `CreateDIBSection` pixel buffer — valid for
    /// as long as `bitmap` lives, i.e. for the lifetime of `self`.
    bits: *mut u8,
    width: u32,
    height: u32,
}

impl HeadlessSurface {
    /// Create a `width` x `height` (device pixels, clamped to at least
    /// `1x1`) headless render target.
    pub fn new(width: u32, height: u32) -> WinResult<Self> {
        let width = width.max(1);
        let height = height.max(1);

        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };

        // `D2D1_ALPHA_MODE_IGNORE`: DC render targets only support
        // `IGNORE` or `PREMULTIPLIED` (MSDN), and nothing here composites
        // partial transparency, so `IGNORE` avoids any premultiply step
        // between `fill_rect`'s solid colour and the DIB's stored bytes.
        let render_props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            ..Default::default()
        };
        let target = unsafe { factory.CreateDCRenderTarget(&render_props)? };

        // Negative `biHeight` = top-down DIB, so `pixel_at`'s row math
        // matches on-screen row order (row 0 = top) with no vertical
        // flip — GDI only allows this for uncompressed (`BI_RGB`) DIBs,
        // which is what we're creating.
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let hdc = unsafe { CreateCompatibleDC(None) };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = match unsafe {
            CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
        } {
            Ok(bitmap) => bitmap,
            Err(err) => {
                unsafe {
                    let _ = DeleteDC(hdc);
                }
                return Err(err);
            }
        };
        unsafe { SelectObject(hdc, bitmap.into()) };

        let rect = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        if let Err(err) = unsafe { target.BindDC(hdc, &rect) } {
            unsafe {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(hdc);
            }
            return Err(err);
        }

        Ok(Self {
            factory,
            target,
            hdc,
            bitmap,
            bits: bits as *mut u8,
            width,
            height,
        })
    }

    /// `(width, height)` device pixels this surface was created with.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The live render target — same `ID2D1RenderTarget` surface every
    /// other backend method in this module (and, eventually, a future
    /// rasteriser test) paints against directly.
    pub fn target(&self) -> &ID2D1DCRenderTarget {
        &self.target
    }

    /// Bracket `paint` between `BeginDraw`/`EndDraw`, same frame lifecycle
    /// as [`super::backend::WinBackend::begin_frame`]/`end_frame`. Returns
    /// the `EndDraw` result — `Err` on device loss or an invalid drawing
    /// call, matching every other fallible call in this module.
    pub fn paint(&self, paint: impl FnOnce(&ID2D1DCRenderTarget)) -> WinResult<()> {
        unsafe { self.target.BeginDraw() };
        paint(&self.target);
        unsafe { self.target.EndDraw(None, None) }
    }

    /// Fill `rect` (DIPs, target-relative) with a solid `color` — the
    /// smoke-test primitive this module's acceptance criterion asks for.
    pub fn fill_rect(&self, rect: Rect, color: Color) -> WinResult<()> {
        self.paint(|target| {
            let brush = match unsafe { target.CreateSolidColorBrush(&color_to_d2d(color), None) } {
                Ok(brush) => brush,
                Err(_) => return,
            };
            let rect_f = D2D_RECT_F {
                left: rect.x,
                top: rect.y,
                right: rect.x + rect.width,
                bottom: rect.y + rect.height,
            };
            unsafe { target.FillRectangle(&rect_f, &brush) };
        })
    }

    /// Read back the colour of the pixel at device-pixel coordinate
    /// `(x, y)`. `a` is always `255`: the render target was created with
    /// `D2D1_ALPHA_MODE_IGNORE` (required for DC render targets), so the
    /// DIB's alpha byte never carries meaningful coverage data.
    ///
    /// # Panics
    ///
    /// If `x >= width` or `y >= height` for the size this surface was
    /// created with.
    pub fn pixel_at(&self, x: u32, y: u32) -> Color {
        assert!(
            x < self.width && y < self.height,
            "pixel_at({x}, {y}) out of bounds for a {}x{} surface",
            self.width,
            self.height
        );
        // BGRA in memory (DXGI_FORMAT_B8G8R8A8_UNORM), 4 bytes/pixel,
        // top-down rows (see the negative `biHeight` above).
        let offset = (y as isize * self.width as isize + x as isize) * 4;
        unsafe {
            let px = self.bits.offset(offset);
            let b = *px;
            let g = *px.add(1);
            let r = *px.add(2);
            Color::rgb(r, g, b)
        }
    }
}

impl Drop for HeadlessSurface {
    fn drop(&mut self) {
        // `target`/`factory` release themselves via `windows-rs`'s
        // `Drop`/`Release` machinery; the raw GDI handles below don't and
        // must be torn down explicitly, mirroring the cleanup already
        // done on `Self::new`'s error paths.
        unsafe {
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.hdc);
        }
    }
}

fn color_to_d2d(color: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: color.r as f32 / 255.0,
        g: color.g as f32 / 255.0,
        b: color.b as f32 / 255.0,
        a: color.a as f32 / 255.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_a_solid_rect_and_reads_the_pixel_back() {
        let surface = HeadlessSurface::new(64, 64).expect("create headless surface");
        surface
            .fill_rect(Rect::new(0.0, 0.0, 64.0, 64.0), Color::rgb(200, 40, 40))
            .expect("fill rect");

        let center = surface.pixel_at(32, 32);
        assert_eq!((center.r, center.g, center.b), (200, 40, 40));
    }

    #[test]
    fn a_rect_that_does_not_cover_the_whole_surface_leaves_the_rest_cleared() {
        let surface = HeadlessSurface::new(32, 32).expect("create headless surface");
        surface
            .paint(|target| unsafe {
                target.Clear(Some(&D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                }));
            })
            .expect("clear");
        surface
            .fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Color::rgb(0, 255, 0))
            .expect("fill rect");

        let inside = surface.pixel_at(2, 2);
        let outside = surface.pixel_at(20, 20);
        assert_eq!((inside.r, inside.g, inside.b), (0, 255, 0));
        assert_eq!((outside.r, outside.g, outside.b), (0, 0, 0));
    }
}
