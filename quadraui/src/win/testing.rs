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

use crate::event::{Rect, Viewport};
use crate::runner::{AppLogic, Reaction};
use crate::shell::{ShellApp, ShellConfig};
use crate::{Color, Key, Modifiers, MouseButton, NamedKey, Point, UiEvent};

use super::backend::WinBackend;
use super::run::{dispatch_event, render_frame, EventOutcome};

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

/// Build a [`WinDriver`] that wraps `app` in the full
/// [`crate::shell_adapter::ShellAdapter`] stack, mirroring exactly what
/// [`crate::win::shell_runner::run_with_shell`] does at runtime — but
/// painting into a [`HeadlessSurface`] instead of opening a live Win32
/// window. The Win-GUI twin of [`crate::gtk::testing::driver_with_shell`]
/// / [`crate::macos::testing::driver_with_shell`] /
/// [`crate::tui::testing::driver_with_shell`] (quadraui#707); all four
/// share the same [`ShellApp`] + [`ShellConfig`] input, differing only in
/// the native units their respective drivers take (Win-GUI: DIPs, same
/// as GTK/macOS points/pixels).
///
/// Use this constructor in tests that need to verify the full
/// `ShellApp → ShellAdapter → dispatch_event` integration path on the
/// Win-GUI backend — e.g. confirming that shell chrome (activity bar,
/// status bar) actually paints through the composed adapter, the way
/// macOS's precedent test
/// (`tests/macos_example_driver.rs::appshell_demo_renders_shell_chrome_via_driver_with_shell`,
/// #465) does for `MacDriver`.
///
/// # Example
///
/// ```no_run
/// # use quadraui::win::testing::driver_with_shell;
/// # use quadraui::{ShellApp, ShellConfig, Backend, ShellContext, Reaction, UiEvent};
/// # struct MyApp;
/// # impl ShellApp for MyApp {
/// #     fn render_content(&self, _: &mut dyn Backend, _: &quadraui::compose::app_shell::AppShellLayout) {}
/// #     fn handle(&mut self, _: UiEvent, _: &mut dyn Backend, _: &ShellContext) -> Reaction { Reaction::Continue }
/// # }
/// let config = ShellConfig::new("Demo", vec![]);
/// let mut driver = driver_with_shell(MyApp, config, 800, 480);
/// let _ = driver.pixel(0, 0);
/// ```
pub fn driver_with_shell<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
    width: u32,
    height: u32,
) -> WinDriver<impl AppLogic> {
    let adapter = crate::shell_adapter::build_shell_adapter(app, config);
    WinDriver::new(adapter, width, height)
}

/// Drives an [`AppLogic`] impl headlessly against the Win-GUI backend for
/// tests. Construct with [`Self::new`] (runs `setup` + paints the first
/// frame), poke it with [`Self::press`] / [`Self::type_char`] /
/// [`Self::click`], and read painted pixels back with [`Self::pixel`].
///
/// ## Display-free
///
/// Renders into a headless [`HeadlessSurface`] (an `ID2D1DCRenderTarget`
/// bound to an in-memory DIB section — see that type's module docs) via
/// [`WinBackend::attach_headless`] — no live `HWND`, no `WndProc`, no
/// display. `Self::render`/`Self::dispatch` call the exact same
/// [`super::run::render_frame`] / [`super::run::dispatch_event`] the live
/// `win::run` message loop uses, so this exercises production
/// pre-processing (ActivityBar keyboard-focus redirect, global
/// accelerator matching) identically to a real keypress — mirrors
/// `MacDriver`'s "no drift from production" contract.
///
/// ## Limitations
///
/// Unlike [`crate::tui::testing::TuiDriver`] / [`crate::gtk::testing::GtkDriver`]
/// / [`crate::macos::testing::MacDriver`], this driver has no
/// `find`/`screen_contains`/painted-text-run recording yet: `WinBackend`
/// doesn't instrument its `draw_text` call sites the way
/// `GtkBackend`/`MacBackend` do (no `TextRun` list, no
/// `set_painted_text_recording`). Assert on [`Self::pixel`] colours
/// instead until that instrumentation lands — the same gap
/// `HeadlessSurface`'s own module docs note for `WinBackend` generally.
/// Also doesn't implement [`crate::testing::ConformanceDriver`] for the
/// same reason (several of that trait's methods are text-run-based).
pub struct WinDriver<A: AppLogic> {
    app: A,
    backend: WinBackend,
    surface: HeadlessSurface,
    width: u32,
    height: u32,
    exited: bool,
}

impl<A: AppLogic> WinDriver<A> {
    /// Build a driver for `app` on a `width`×`height` pixel surface, run
    /// the app's `setup` hook, and paint the first frame.
    ///
    /// # Panics
    ///
    /// If creating the offscreen Direct2D DC render target
    /// ([`HeadlessSurface::new`]) or attaching it
    /// ([`WinBackend::attach_headless`]) fails — both are effectively
    /// infallible on a real Windows host (WARP software rasterisation
    /// needs no GPU/driver/display, see `HeadlessSurface`'s module docs),
    /// so a failure here means something is fundamentally wrong with the
    /// test environment rather than a condition tests should recover
    /// from — same posture `HeadlessSurface`'s own doctest/unit tests
    /// take with `.expect(..)`.
    pub fn new(app: A, width: u32, height: u32) -> Self {
        let surface = HeadlessSurface::new(width, height)
            .expect("WinDriver::new: create offscreen Direct2D DC render target");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), width, height)
            .expect("WinDriver::new: attach headless surface to WinBackend");
        let mut app = app;
        app.setup(&mut backend);
        let mut driver = Self {
            app,
            backend,
            surface,
            width,
            height,
            exited: false,
        };
        driver.render();
        driver
    }

    /// Repaint one frame through the shared production render path.
    pub fn render(&mut self) {
        let viewport = Viewport::new(self.width as f32, self.height as f32, 1.0);
        render_frame(&mut self.backend, &self.app, viewport);
    }

    /// Feed one synthetic event through the shared production
    /// [`dispatch_event`] path. Repaints on redraw and latches `exited`.
    pub fn dispatch(&mut self, event: UiEvent) -> Reaction {
        if self.exited {
            return Reaction::Exit;
        }
        match dispatch_event(event, &mut self.backend, &mut self.app) {
            EventOutcome::Continue => Reaction::Continue,
            EventOutcome::Redraw => {
                self.render();
                Reaction::Redraw
            }
            EventOutcome::Exit => {
                self.exited = true;
                Reaction::Exit
            }
        }
    }

    /// Press a key (no modifiers).
    pub fn press(&mut self, key: Key) -> Reaction {
        self.dispatch(UiEvent::KeyPressed {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// Type a single character key (no modifiers).
    pub fn type_char(&mut self, c: char) -> Reaction {
        self.press(Key::Char(c))
    }

    /// Press a named (non-printable) key, e.g. [`NamedKey::Enter`].
    pub fn press_named(&mut self, key: NamedKey) -> Reaction {
        self.press(Key::Named(key))
    }

    /// Left-click at surface coordinates `(x, y)` (DIPs): down then up.
    pub fn click(&mut self, x: f32, y: f32) -> Reaction {
        self.mouse_down(x, y);
        self.mouse_up(x, y)
    }

    /// Press the left mouse button down at `(x, y)`.
    pub fn mouse_down(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        })
    }

    /// Release the left mouse button at `(x, y)`.
    pub fn mouse_up(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
        })
    }

    /// Whether the app has returned [`Reaction::Exit`].
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Access the app state for test assertions.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// Mutable access to the app state for tests that need to poke state
    /// directly rather than through a scripted [`UiEvent`].
    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    /// Access the backend for test assertions.
    pub fn backend(&self) -> &WinBackend {
        &self.backend
    }

    /// Access the underlying offscreen surface.
    pub fn surface(&self) -> &HeadlessSurface {
        &self.surface
    }

    /// Read a painted pixel back from the rendered surface at device-pixel
    /// coordinate `(x, y)` — see [`HeadlessSurface::pixel_at`].
    pub fn pixel(&self, x: u32, y: u32) -> Color {
        self.surface.pixel_at(x, y)
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
