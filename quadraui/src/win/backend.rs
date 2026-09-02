//! `WinBackend` — Direct2D + DirectWrite implementation of [`Backend`].
//!
//! #19 landed the window + render-target bootstrap: `begin_frame` /
//! `end_frame` bracket real `ID2D1HwndRenderTarget::BeginDraw` /
//! `Clear` / `EndDraw` calls (gated on `cfg(target_os = "windows")` —
//! see [`Surface`] and [`WinBackend::attach_surface`]), and
//! `Viewport::scale` carries the real per-window DPI ratio. #21 landed
//! the DirectWrite text infrastructure — [`super::text::DWrite`],
//! [`WinBackend::measure_text`], [`WinBackend::draw_text`] — so
//! `line_height()`/`char_width()` return real font metrics instead of
//! the `16.0`/`8.0` placeholder defaults. #25 landed the four chrome-strip
//! rasterisers — `status_bar`, `tab_bar`, `activity_bar`, `menu_bar` (see
//! `super::status_bar`/`super::tab_bar`/`super::activity_bar`/
//! `super::menu_bar`) — each wired into its `Backend` trait method below,
//! falling back to the `todo!()` stub only for a `WinBackend` no window
//! has ever attached a surface to (see [`WinBackend::draw_status_bar`]'s
//! doc). #26 landed the six content-area rasterisers (`tree`/`list`/
//! `form`/`data_table`/`editor`/`chart`), #27 the multi-section view +
//! standalone scrollbar, and #28 the seven overlay rasterisers —
//! `tooltip`/`context_menu`/`dialog`/`palette`/`completions`/
//! `find_replace`/`rich_text_popup` (see `super::tooltip` etc.). Every
//! other `draw_*`/`*_layout` rasteriser method is still a `todo!()`
//! stub — later issues implement each one against Direct2D /
//! DirectWrite, same as the GTK backend did one primitive at a time.
//!
//! # Implementation notes
//!
//! - **Render target**: `ID2D1HwndRenderTarget` for the main window
//!   (this issue). Offscreen: [`super::testing::HeadlessSurface`] wraps an
//!   `ID2D1DCRenderTarget` bound to an in-memory DIB section for headless
//!   tests (#24) — a lower-level building block than a `WinBackend`
//!   driver, since every `draw_*`/`*_layout` rasteriser below is still a
//!   `todo!()` stub with nothing yet for a driver to paint. Only actually
//!   *runs* on the `windows-latest` leg of `ci.yml`'s `tui` job (see
//!   `HeadlessSurface`'s module docs and that workflow's "Test (win
//!   feature, real Windows)" step) — `cargo check --features win` on
//!   Linux still only type-checks this file, same as everything else
//!   `target_os = "windows"`-gated in this module.
//! - **Text** (#21): `IDWriteTextFormat` + `IDWriteTextLayout` for
//!   measurement and rendering (`super::text`). `line_height`/`char_width`
//!   are resolved once per surface from `IDWriteFontFace::GetMetrics`
//!   (same role as GTK's `current_line_height` / `current_char_width`,
//!   see [`WinBackend::set_current_line_height`] /
//!   [`WinBackend::set_current_char_width`]).
//! - **Frame scope**: `BeginDraw()` / `EndDraw()` bracket each frame.
//!   Unlike GTK, the render target is available outside the frame
//!   scope for measurement — `_layout()` methods can use
//!   `IDWriteTextLayout` directly.
//! - **DPI**: `GetDpiForWindow()` / 96.0 → `Viewport::scale` (this
//!   issue). Rasterisers landing later scale coordinates by it the same
//!   way GTK's rasterisers use `DrawingArea::scale_factor()`.
//! - **Events**: `WM_LBUTTONDOWN` → `UiEvent::MouseDown`, `WM_KEYDOWN` →
//!   `UiEvent::KeyPressed`, etc. — landed in #20 (`win::events`'
//!   translators, dispatched from `win::run`'s `WndProc`). #19 wired only
//!   the three window-lifecycle events the message-loop bootstrap itself
//!   needs to stay alive: `WM_SIZE` → `UiEvent::WindowResized`,
//!   `WM_DPICHANGED` → `UiEvent::DpiChanged`, and `WM_CLOSE` →
//!   `UiEvent::WindowClose`.

use std::collections::HashMap;
use std::time::Duration;

use crate::backend::{Backend, EditorPaintResult, PlatformServices};
use crate::dispatch::DragState;
use crate::event::{Rect, UiEvent, Viewport};
use crate::modal_stack::ModalStack;
use crate::primitives::activity_bar::ActivityBarRowHit;
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout};
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::primitives::dialog::{Dialog, DialogLayout};
use crate::primitives::editor::Editor;
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::form::{Form, FormLayout};
use crate::primitives::menu_bar::{MenuBar, MenuBarLayout};
use crate::primitives::message_list::MessageList;
use crate::primitives::multi_section_view::{
    LayoutMetrics, MultiSectionView, MultiSectionViewLayout,
};
use crate::primitives::panel::{Panel, PanelLayout};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::spinner::{Spinner, SpinnerLayout};
use crate::primitives::split::{Split, SplitLayout};
use crate::primitives::status_bar::StatusBarLayout;
use crate::primitives::tab_bar::TabBarHits;
use crate::primitives::text_display::TextDisplayLayout;
use crate::primitives::toast::{ToastStack, ToastStackLayout};
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::primitives::tree::TreeViewLayout;
use crate::types::WidgetId;
use crate::{
    Accelerator, AcceleratorId, ActivityBar, ListView, Palette, StatusBar, TabBar, Terminal,
    TextDisplay, TooltipChrome, TreeView,
};

use super::services::WinPlatformServices;

// ─── Direct2D bootstrap (#19) ───────────────────────────────────────────
//
// Real WinAPI/Direct2D calls only compile where the `windows` crate
// actually has bindings, i.e. `target_os = "windows"` (see
// `Cargo.toml`'s target-specific `windows` dependency). Everywhere else
// — including a plain `cargo check --features win` on Linux — the
// `Backend` trait methods below fall back to the original `todo!()`
// stub bodies, which is what keeps this file's per-repo "every trait
// method has a WinBackend impl" compile gate (`ci.yml`'s "Compile check
// (win feature)" step) meaningful without a Windows runner.
#[cfg(target_os = "windows")]
use windows::core::Result as WinResult;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, RECT};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES,
};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
#[cfg(target_os = "windows")]
use windows::Win32::UI::HiDpi::GetDpiForWindow;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

#[cfg(target_os = "windows")]
use super::text::DWrite;

/// Default editor font family for the Win-GUI backend — the historical
/// default monospace face shipped with every Windows version since Vista,
/// same role as GTK's hardcoded `"Monospace"` fontconfig alias.
#[cfg(target_os = "windows")]
const DEFAULT_EDITOR_FONT_FAMILY: &str = "Consolas";

/// The live Direct2D handles behind a real Win32 window. Only exists once
/// [`WinBackend::attach_surface`] has run — a `WinBackend` constructed
/// standalone (headless, or before `CreateWindowExW` returns a `HWND`)
/// has `surface: None` and every draw call keeps hitting the `todo!()`
/// arm until a later issue implements it.
#[cfg(target_os = "windows")]
struct Surface {
    /// Kept alive only because `ID2D1HwndRenderTarget` was created from
    /// it — never called again after `attach_surface` populates `target`.
    #[allow(dead_code)]
    factory: ID2D1Factory,
    target: ID2D1HwndRenderTarget,
}

pub struct WinBackend {
    viewport: Viewport,
    modal_stack: ModalStack,
    drag_state: DragState,
    accelerators: HashMap<AcceleratorId, Accelerator>,
    services: WinPlatformServices,
    current_line_height: f32,
    current_char_width: f32,
    /// DPI ratio (`GetDpiForWindow(hwnd) / 96.0`). Mirrored into
    /// `viewport.scale` on every attach/resize/`WM_DPICHANGED` so
    /// `Backend::viewport()` and this field never drift. Kept as its own
    /// field (rather than always re-deriving from `viewport.scale`)
    /// because `WM_DPICHANGED` updates it independently of a resize.
    /// `cfg`-gated like `surface` below — nothing outside the
    /// `target_os = "windows"` methods reads or writes it, so on every
    /// other host it would otherwise be dead code.
    #[cfg(target_os = "windows")]
    dpi_scale: f32,
    #[cfg(target_os = "windows")]
    surface: Option<Surface>,
    /// The last `HWND` a surface was successfully attached to, kept
    /// *outside* `Surface` (and outlasting it) so a dropped surface
    /// (`end_frame`'s device-lost recovery) can still be re-created by
    /// [`Self::ensure_surface`] without needing `win::run` to thread the
    /// `HWND` back through a second time. `None` until the first
    /// `attach_surface` call succeeds.
    #[cfg(target_os = "windows")]
    hwnd: Option<HWND>,
    /// DirectWrite factory + `IDWriteTextFormat` for the current editor
    /// font (`editor_font_family`/`editor_font_size_pt`). `None` until
    /// [`Self::attach_surface`] creates it — same lifecycle as `surface`,
    /// and recreated alongside it whenever [`Self::ensure_surface`]'s
    /// device-lost recovery re-attaches (#21).
    #[cfg(target_os = "windows")]
    dwrite: Option<DWrite>,
    /// Font family used to paint text via DirectWrite. Defaults to
    /// [`DEFAULT_EDITOR_FONT_FAMILY`]. Set via
    /// [`Backend::set_editor_font`]; [`Self::attach_surface`] reads it
    /// when constructing the `IDWriteTextFormat` — mirrors GTK's
    /// `editor_font_family` (#422). `cfg`-gated like `dwrite`: nothing
    /// outside the `target_os = "windows"` methods reads or writes it.
    #[cfg(target_os = "windows")]
    editor_font_family: String,
    /// Editor font size in points, paired with `editor_font_family`.
    /// Defaults to `11.0`, matching GTK's default.
    #[cfg(target_os = "windows")]
    editor_font_size_pt: f32,
}

impl WinBackend {
    pub fn new() -> Self {
        Self {
            viewport: Viewport::new(0.0, 0.0, 1.0),
            modal_stack: ModalStack::new(),
            drag_state: DragState::new(),
            accelerators: HashMap::new(),
            services: WinPlatformServices::new(),
            current_line_height: 16.0,
            current_char_width: 8.0,
            #[cfg(target_os = "windows")]
            dpi_scale: 1.0,
            #[cfg(target_os = "windows")]
            surface: None,
            #[cfg(target_os = "windows")]
            hwnd: None,
            #[cfg(target_os = "windows")]
            dwrite: None,
            #[cfg(target_os = "windows")]
            editor_font_family: DEFAULT_EDITOR_FONT_FAMILY.to_string(),
            #[cfg(target_os = "windows")]
            editor_font_size_pt: 11.0,
        }
    }

    /// Update the cached DirectWrite line height (in DIPs). Mirrors
    /// `GtkBackend::set_current_line_height` — normally set once by
    /// [`Self::attach_surface`] from real font metrics, but exposed
    /// publicly (like GTK's) so tests and callers can override it
    /// directly.
    pub fn set_current_line_height(&mut self, line_height: f32) {
        self.current_line_height = line_height;
    }

    /// Update the cached DirectWrite approximate-char-width (in DIPs).
    /// Mirrors `GtkBackend::set_current_char_width`.
    pub fn set_current_char_width(&mut self, char_width: f32) {
        self.current_char_width = char_width;
    }

    /// Create a single-threaded `ID2D1Factory` and an `ID2D1HwndRenderTarget`
    /// sized to `hwnd`'s current client rect, and store both on `self`.
    ///
    /// Called once by [`crate::win::run`] right after `CreateWindowExW`
    /// returns a live `HWND` (#19's bootstrap). Also seeds
    /// [`Backend::viewport`][crate::backend::Backend::viewport] with the
    /// window's real pixel size and DPI scale, so the very first
    /// `WM_PAINT` (and any app code reading `backend.viewport()` from
    /// `setup()`, which per the trait doc runs *before* this is called
    /// and so still sees the zeroed seed) catches up to reality on that
    /// first paint.
    #[cfg(target_os = "windows")]
    pub(crate) fn attach_surface(&mut self, hwnd: HWND) -> WinResult<()> {
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };

        let mut rect = RECT::default();
        unsafe { GetClientRect(hwnd, &mut rect)? };
        let width = (rect.right - rect.left).max(1) as u32;
        let height = (rect.bottom - rect.top).max(1) as u32;

        let render_props = D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
            },
            ..Default::default()
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U { width, height },
            ..Default::default()
        };
        let target = unsafe { factory.CreateHwndRenderTarget(&render_props, &hwnd_props)? };

        self.dpi_scale = dpi_scale_for_window(hwnd);
        self.viewport = Viewport::new(width as f32, height as f32, self.dpi_scale);
        self.surface = Some(Surface { factory, target });
        self.hwnd = Some(hwnd);
        // #23: give `WinPlatformServices` the live window so file dialogs
        // open parented to it and notifications have an owning `HWND` —
        // mirrors `GtkBackend::set_window` calling
        // `GtkPlatformServices::set_window` right after its own window is
        // constructed.
        self.services.set_window(hwnd);

        // DirectWrite bootstrap (#21): build the factory + text format for
        // the currently-configured editor font and seed `line_height()` /
        // `char_width()` from its real font metrics, same role as
        // `gtk::run::render_frame`'s per-frame Pango metrics resolve —
        // except here it only needs to happen once per surface, since
        // DirectWrite text formats aren't tied to the Direct2D device the
        // way the render target is.
        let (dwrite, line_height, char_width) =
            DWrite::new(&self.editor_font_family, self.editor_font_size_pt)?;
        self.dwrite = Some(dwrite);
        self.current_line_height = line_height;
        self.current_char_width = char_width;

        Ok(())
    }

    /// `(width, height)` DIPs of `text` measured against the current
    /// editor font (#21's `measure_text(text) -> (width_dips,
    /// height_dips)` helper). Returns `(0.0, 0.0)` if no surface has
    /// attached yet — nothing to measure against, same "not wired up
    /// yet" posture as every `todo!()` rasteriser below.
    #[cfg(target_os = "windows")]
    pub fn measure_text(&self, text: &str) -> (f32, f32) {
        self.dwrite
            .as_ref()
            .and_then(|d| d.measure_text(text).ok())
            .unwrap_or((0.0, 0.0))
    }

    /// Paint `text` inside `rect` (DIPs) in `color` onto the live render
    /// target (#21's `draw_text(target, text, rect, color)` helper, with
    /// `target` implicit via `self.surface` rather than threaded through
    /// explicitly — matching every other `draw_*` method on this
    /// backend). No-op if no surface/DirectWrite handle is attached yet.
    #[cfg(target_os = "windows")]
    pub fn draw_text(&self, text: &str, rect: Rect, color: crate::Color) {
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            let _ = dwrite.draw_text(&surface.target, text, rect, color);
        }
    }

    /// Re-create the render target after a prior `EndDraw` failure
    /// dropped it — the actual implementation of the recovery path
    /// [`Backend::end_frame`]'s docs promise. `win::run` calls this from
    /// its `WM_PAINT` and `WM_SIZE` handlers before touching the surface,
    /// so a lost surface comes back on the next paint or resize instead
    /// of leaving the window permanently blank.
    ///
    /// A no-op returning `Ok(())` if a surface is already live, and a
    /// no-op returning `Ok(())` if no window has ever attached one yet
    /// (nothing to recover to — covers the synchronous `WM_SIZE` Windows
    /// fires from inside `CreateWindowExW`, before `run_inner` has a
    /// `HWND` to attach at all).
    #[cfg(target_os = "windows")]
    pub(crate) fn ensure_surface(&mut self) -> WinResult<()> {
        if self.surface.is_some() {
            return Ok(());
        }
        match self.hwnd {
            Some(hwnd) => self.attach_surface(hwnd),
            None => Ok(()),
        }
    }

    /// Resize the live render target to `width` x `height` device pixels.
    /// Called from `win::run`'s `WM_SIZE` handler (#19's "responds to
    /// resize without crashing" acceptance criterion).
    ///
    /// A no-op on the render-target side if no surface is attached yet —
    /// Windows can fire an initial `WM_SIZE` synchronously from inside
    /// `CreateWindowExW`, before the caller has the `HWND` back to pass
    /// to [`Self::attach_surface`]. The viewport is still updated so
    /// `Backend::viewport()` reflects the window's real size as soon as
    /// it's known, even before there's a surface to paint onto.
    #[cfg(target_os = "windows")]
    pub(crate) fn resize_surface(&mut self, width: u32, height: u32) -> WinResult<()> {
        let width = width.max(1);
        let height = height.max(1);
        if let Some(surface) = &self.surface {
            let size = D2D_SIZE_U { width, height };
            unsafe { surface.target.Resize(&size)? };
        }
        self.viewport = Viewport::new(width as f32, height as f32, self.dpi_scale);
        Ok(())
    }

    /// Update the cached DPI scale (`WM_DPICHANGED`) without touching the
    /// render target's pixel size — the caller is responsible for
    /// resizing separately, once it has applied the new-DPI suggested
    /// window rect Windows hands back with that message.
    #[cfg(target_os = "windows")]
    pub(crate) fn set_dpi_scale(&mut self, scale: f32) {
        self.dpi_scale = scale;
        self.viewport.scale = scale;
    }
}

/// `GetDpiForWindow(hwnd) / 96.0` — the ratio `Viewport::scale` carries
/// for this backend (issue #19's "DPI scale factor plumbed to
/// `Viewport::scale`" acceptance criterion).
///
/// The divide (and its zero-DPI fallback) lives in
/// [`crate::win::msg::dpi_ratio`] so it stays unit-tested off Windows;
/// this wrapper is only the `GetDpiForWindow` call that can't be.
#[cfg(target_os = "windows")]
fn dpi_scale_for_window(hwnd: HWND) -> f32 {
    crate::win::msg::dpi_ratio(unsafe { GetDpiForWindow(hwnd) })
}

impl Default for WinBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for WinBackend {
    // ─── Frame + viewport ─────────────────────────────────────────────

    fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn begin_frame(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            // `BeginDraw`/`Clear` are infallible on `ID2D1RenderTarget`
            // (device-lost errors only surface later, from `EndDraw` —
            // handled in `end_frame` below).
            unsafe {
                surface.target.BeginDraw();
            }
            // Placeholder clear color until #20+ wires the app's real
            // `Theme` background through. Rasterisers land per-primitive
            // in later issues; this bootstrap only needs a cleared
            // surface (issue #19's acceptance criterion).
            let clear_color = D2D1_COLOR_F {
                r: 0.117,
                g: 0.117,
                b: 0.117,
                a: 1.0,
            };
            unsafe {
                surface.target.Clear(Some(&clear_color));
            }
        }
        #[cfg(not(target_os = "windows"))]
        todo!("ID2D1RenderTarget::BeginDraw()")
    }

    fn end_frame(&mut self) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            // `EndDraw` fails for two distinct reasons, both handled the
            // same way here since the `Backend` trait's frame lifecycle
            // has no error channel to report through
            // (`docs/SMELL_AUDIT_2026-07.md` #93): the well-known
            // `D2DERR_RECREATE_TARGET` (GPU driver reset, remote-desktop
            // session change, etc.) and anything else. Either way, drop
            // the surface rather than silently pretending the frame
            // presented, or letting the next frame paint onto a target
            // Direct2D has already discarded. `self.hwnd` (set by
            // `attach_surface`, untouched here) survives the drop, so
            // the next `WM_PAINT`/`WM_SIZE` reaching `win::run` actually
            // recreates it via `Self::ensure_surface` — see that
            // method's docs.
            if unsafe { surface.target.EndDraw(None, None) }.is_err() {
                self.surface = None;
            }
        }
        #[cfg(not(target_os = "windows"))]
        todo!("ID2D1RenderTarget::EndDraw()")
    }

    // ─── Events + keybindings ─────────────────────────────────────────

    fn poll_events(&mut self) -> Vec<UiEvent> {
        todo!("PeekMessage loop → translate WM_* → UiEvent")
    }

    fn wait_events(&mut self, _timeout: Duration) -> Vec<UiEvent> {
        todo!("MsgWaitForMultipleObjects + GetMessage → UiEvent")
    }

    fn register_accelerator(&mut self, acc: &Accelerator) {
        self.accelerators.insert(acc.id.clone(), acc.clone());
    }

    fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.accelerators.remove(id);
    }

    // ─── Modal-overlay tracking ───────────────────────────────────────

    fn modal_stack_mut(&mut self) -> &mut ModalStack {
        &mut self.modal_stack
    }

    fn drag_and_modal_mut(&mut self) -> (&mut DragState, &mut ModalStack) {
        (&mut self.drag_state, &mut self.modal_stack)
    }

    // ─── Platform services ────────────────────────────────────────────

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    // ─── Capability declaration ──────────────────────────────────────────

    /// quadraui#492: honest, not aspirational. #19 landed the window +
    /// render-target bootstrap (`begin_frame`/`end_frame`) and the three
    /// window-lifecycle events the message loop itself needs
    /// (`WindowResized`/`DpiChanged`/`WindowClose`); #20 added the rest of
    /// the input table — mouse buttons/motion/wheel, `WM_KEYDOWN`/
    /// `WM_CHAR`, and focus — all dispatched directly from `win::run`'s
    /// `WndProc` (see that module's docs for why this mirrors the GTK
    /// backend's signal-callback dispatch rather than
    /// `poll_events`/`wait_events`, which stay `todo!("PeekMessage loop →
    /// translate WM_* → UiEvent")` since they're not on that hot path).
    ///
    /// `mouse`/`scroll`/`drag` still read `false` here despite that
    /// translation existing: every `draw_*`/`*_layout` rasteriser below is
    /// still a `todo!()` stub, so a conformance scenario that declares
    /// `requires: ["mouse"]` and then exercises a click against a real
    /// widget would panic mid-scenario rather than the named `skip`
    /// `BackendCaps` exists to produce (`docs/TESTING.md`'s coverage
    /// taxonomy). Flip these once the rasterisers they'd actually be
    /// clicking on land.
    fn backend_caps(&self) -> crate::backend::BackendCaps {
        // Only mutated under `cfg(target_os = "windows")` below — `mut`
        // would otherwise warn as unused on every other host, same
        // `cfg_attr` pattern `win::msg` uses for its host-independent
        // helpers.
        #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
        let mut caps = crate::backend::BackendCaps::empty();
        // #23: file dialogs (`IFileOpenDialog`/`IFileSaveDialog`) and
        // notifications (`Shell_NotifyIconW`) go through COM/Shell APIs
        // independent of the Direct2D rasteriser work above — unlike
        // `mouse`/`scroll`/`drag`, there is no unfinished rasteriser
        // gating these on, so they're honestly `true` on Windows itself.
        // `native_dialogs` (message/alert dialogs) stays unset — that's
        // still a `None`-returning stub pending quadraui#666.
        #[cfg(target_os = "windows")]
        {
            caps.file_dialogs = true;
            caps.notifications = true;
        }
        caps
    }

    // ─── Measurement ──────────────────────────────────────────────────

    fn line_height(&self) -> f32 {
        self.current_line_height
    }

    fn char_width(&self) -> f32 {
        self.current_char_width
    }

    /// Store the editor font family + size for the next
    /// [`Self::attach_surface`]/[`Self::ensure_surface`] call to build an
    /// `IDWriteTextFormat` from (#21). Mirrors `GtkBackend::set_editor_font`
    /// (#422): a live surface doesn't rebuild its `IDWriteTextFormat`
    /// immediately on a runtime font change — same limitation the GTK
    /// backend has today (its Pango layout is rebuilt fresh every frame
    /// from these fields instead, which this backend's once-per-surface
    /// DirectWrite bootstrap doesn't yet mirror). A future issue can wire
    /// a live-reload path through `win::run`'s `WM_PAINT` handler if that
    /// gap needs closing.
    fn set_editor_font(&mut self, family: &str, size_pt: f32) {
        #[cfg(target_os = "windows")]
        {
            self.editor_font_family = family.to_string();
            self.editor_font_size_pt = size_pt;
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (family, size_pt);
        }
    }

    // ─── Drawing ──────────────────────────────────────────────────────

    /// #26: real Direct2D/DirectWrite rasteriser via `win::tree` once a
    /// surface is attached. See [`Self::draw_status_bar`]'s doc for the
    /// "surface not attached yet" fallback posture. `draw_tree` returns
    /// `()` per the trait (unlike the chrome rasterisers) — hosts get
    /// hit-test data from [`Self::tree_layout`] instead.
    fn draw_tree(&mut self, rect: Rect, tree: &TreeView) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::tree::draw_tree(
                &surface.target,
                dwrite,
                rect,
                tree,
                self.current_line_height,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, tree);
        todo!("Direct2D tree rasteriser (no surface attached yet)")
    }

    /// #26: see [`Self::draw_tree`]'s doc.
    fn draw_list(&mut self, rect: Rect, list: &ListView) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::list::draw_list(
                &surface.target,
                dwrite,
                rect,
                list,
                self.current_line_height,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, list);
        todo!("Direct2D list rasteriser (no surface attached yet)")
    }

    /// #26: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_data_table(
        &mut self,
        rect: Rect,
        table: &crate::DataTable,
        hovered_idx: Option<usize>,
    ) -> crate::DataTableLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::data_table::draw_data_table(
                &surface.target,
                dwrite,
                rect,
                table,
                self.current_line_height,
                hovered_idx,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, table, hovered_idx);
        todo!("Direct2D data table rasteriser (no surface attached yet)")
    }

    /// #26: pure measurement — only needs `self.dwrite`, not a live
    /// render target. See [`Self::status_bar_layout`]'s doc.
    fn data_table_layout(&self, rect: Rect, table: &crate::DataTable) -> crate::DataTableLayout {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = &self.dwrite {
            return super::data_table::win_data_table_layout(
                dwrite,
                rect,
                table,
                self.current_line_height,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, table);
        todo!("DirectWrite data table layout (no surface attached yet)")
    }

    /// #26: GTK's twin (`GtkBackend::list_hscrollbar`) builds the
    /// `Scrollbar` directly from `self.current_char_width` rather than
    /// delegating to [`ListView::hscrollbar`] — that primitive method
    /// treats `max_content_width` as already being in the caller's
    /// native unit (correct for TUI's 1-char-per-cell grid, wrong for a
    /// pixel backend where chars must first be scaled by char width).
    /// Cross-platform: no Direct2D/DirectWrite call needed, so this
    /// compiles (and is exercised by `cargo check --features win`) on
    /// every host, not just `target_os = "windows"`.
    fn list_hscrollbar(&self, rect: Rect, list: &ListView) -> Option<crate::Scrollbar> {
        let char_w = self.current_char_width;
        let max_w_chars = list.max_content_width? as f32;
        let content_px = max_w_chars * char_w;
        let border_inset = if list.bordered { char_w } else { 0.0 };
        let visible_px = (rect.width - 2.0 * border_inset).max(0.0);
        if content_px <= visible_px {
            return None;
        }
        let row_h = self.current_line_height;
        let (track_x, track_w, track_y) = if list.bordered {
            (
                rect.x + char_w,
                (rect.width - 2.0 * char_w).max(0.0),
                rect.y + (rect.height - 2.0 * row_h).max(0.0),
            )
        } else {
            (rect.x, rect.width, rect.y + (rect.height - row_h).max(0.0))
        };
        let track = Rect::new(track_x, track_y, track_w, row_h);
        Some(crate::Scrollbar::horizontal(
            list.id.clone(),
            track,
            list.h_scroll as f32 * char_w,
            content_px,
            visible_px,
            row_h,
        ))
    }

    /// #26: `vscrollbar` deals purely in row counts/row-height, so
    /// unlike [`Self::list_hscrollbar`] the primitive's own geometry
    /// method is unit-correct for every backend — see
    /// [`ListView::vscrollbar`]. Cross-platform, same reasoning as
    /// `list_hscrollbar`.
    fn list_vscrollbar(&self, rect: Rect, list: &ListView) -> Option<crate::Scrollbar> {
        list.vscrollbar(rect, self.current_line_height)
    }

    /// #26: see [`Self::draw_tree`]'s doc.
    fn draw_form(&mut self, rect: Rect, form: &Form) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::form::draw_form(
                &surface.target,
                dwrite,
                rect,
                form,
                self.current_line_height,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, form);
        todo!("Direct2D form rasteriser (no surface attached yet)")
    }

    /// #28: real Direct2D/DirectWrite rasteriser via `win::palette` once a
    /// surface is attached — `Palette` has no layout-passthrough trait
    /// method (unlike `ContextMenu`/`Dialog`), so `win::win_palette_layout`
    /// is computed internally by the rasteriser itself. See
    /// [`Self::draw_status_bar`]'s doc for the "surface not attached yet"
    /// fallback posture.
    fn draw_palette(&mut self, rect: Rect, palette: &Palette) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::palette::draw_palette(
                &surface.target,
                dwrite,
                rect,
                palette,
                self.current_line_height,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, palette);
        todo!("Direct2D palette rasteriser (no surface attached yet)")
    }

    fn draw_settings_chrome(
        &mut self,
        _rect: Rect,
        _header_text: &str,
        _query: &str,
        _placeholder: &str,
        _active: bool,
    ) {
        todo!("Direct2D settings chrome rasteriser")
    }

    /// #25: real Direct2D/DirectWrite rasteriser via `win::status_bar`
    /// once a surface is attached. Falls through to the `todo!()` stub
    /// otherwise — `self.surface`/`self.dwrite` are always populated
    /// together by [`Self::attach_surface`], so that only happens for a
    /// standalone `WinBackend` no window has ever attached to yet, the
    /// same "not wired up" posture every other still-`todo!()` method
    /// here has.
    fn draw_status_bar(
        &mut self,
        rect: Rect,
        bar: &StatusBar,
        hovered_id: Option<&crate::types::WidgetId>,
        pressed_id: Option<&crate::types::WidgetId>,
    ) -> StatusBarLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::status_bar::draw_status_bar(
                &surface.target,
                dwrite,
                rect,
                bar,
                hovered_id,
                pressed_id,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, bar, hovered_id, pressed_id);
        todo!("Direct2D status bar rasteriser (no surface attached yet)")
    }

    fn draw_tab_bar(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits {
        // Icon-less bars are the empty-sidecar case of the icon path (see
        // `crate::Backend::draw_tab_bar_icons`'s doc) — one paint loop to
        // keep in sync, same as `GtkBackend`/`TuiBackend`.
        self.draw_tab_bar_icons(rect, bar, &[], hovered_close_tab)
    }

    /// #25: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_tab_bar_icons(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::tab_bar::draw_tab_bar_icons(
                &surface.target,
                dwrite,
                rect,
                bar,
                icons,
                hovered_close_tab,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, bar, icons, hovered_close_tab);
        todo!("Direct2D tab bar rasteriser (with per-tab icons) — no surface attached yet")
    }

    /// #25: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_activity_bar(
        &mut self,
        rect: Rect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
    ) -> Vec<ActivityBarRowHit> {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::activity_bar::draw_activity_bar(
                &surface.target,
                dwrite,
                rect,
                bar,
                hovered_idx,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, bar, hovered_idx);
        todo!("Direct2D activity bar rasteriser (no surface attached yet)")
    }

    /// #25: pure measurement — only needs `self.dwrite`, not a live
    /// render target, so this works as soon as a surface has ever been
    /// attached (DirectWrite handles outlive device loss; see
    /// `Self::ensure_surface`'s docs).
    fn status_bar_layout(&self, rect: Rect, bar: &StatusBar) -> StatusBarLayout {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = &self.dwrite {
            return super::status_bar::win_status_bar_layout(dwrite, rect, bar);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, bar);
        todo!("DirectWrite status bar layout (no surface attached yet)")
    }

    fn tab_bar_layout(&self, rect: Rect, bar: &TabBar) -> TabBarHits {
        self.tab_bar_layout_icons(rect, bar, &[])
    }

    /// #25: see [`Self::status_bar_layout`]'s doc for why this only needs
    /// `self.dwrite`.
    fn tab_bar_layout_icons(
        &self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
    ) -> TabBarHits {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = &self.dwrite {
            return super::tab_bar::win_tab_bar_layout_icons(dwrite, rect, bar, icons);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, bar, icons);
        todo!("DirectWrite tab bar layout (with per-tab icons) — no surface attached yet")
    }

    /// #25: activity-bar layout needs no measurer at all (uniform
    /// `ACTIVITY_ROW_DIP` row height), so unlike its siblings this
    /// doesn't even need `self.dwrite` — only kept behind the
    /// `target_os = "windows"` gate for consistency with every other
    /// method in this file.
    fn activity_bar_layout(&self, rect: Rect, bar: &ActivityBar) -> Vec<ActivityBarRowHit> {
        // No `return` here (unlike the `if let Some(dwrite)` early-returns
        // above): on `target_os = "windows"` the `not(windows)` block below
        // is stripped, so this block *is* the tail expression and an
        // explicit `return` trips `clippy::needless_return` — an error under
        // CI's `-D warnings`, and only on the windows-latest leg.
        #[cfg(target_os = "windows")]
        {
            super::activity_bar::win_activity_bar_layout(rect, bar)
                .visible_items
                .into_iter()
                .map(|vi| {
                    let item = match vi.side {
                        crate::primitives::activity_bar::ActivitySide::Top => {
                            &bar.top_items[vi.item_idx]
                        }
                        crate::primitives::activity_bar::ActivitySide::Bottom => {
                            &bar.bottom_items[vi.item_idx]
                        }
                    };
                    ActivityBarRowHit {
                        y_start: vi.bounds.y as f64,
                        y_end: (vi.bounds.y + vi.bounds.height) as f64,
                        id: item.id.clone(),
                        tooltip: item.tooltip.clone(),
                    }
                })
                .collect()
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, bar);
            todo!("DirectWrite activity bar layout")
        }
    }

    fn draw_terminal(&mut self, _rect: Rect, _term: &Terminal) {
        todo!("Direct2D terminal cell grid rasteriser")
    }

    fn draw_terminal_divider(&mut self, _rect: Rect) {
        todo!("Direct2D terminal split divider rasteriser")
    }

    fn draw_text_display(&mut self, _rect: Rect, _td: &TextDisplay) {
        todo!("Direct2D text display rasteriser")
    }

    fn draw_command_line(
        &mut self,
        _rect: Rect,
        _cmd: &crate::primitives::command_line::CommandLine,
    ) {
        todo!("Direct2D command line rasteriser")
    }

    fn text_display_layout(&self, _rect: Rect, _td: &TextDisplay) -> TextDisplayLayout {
        todo!("DirectWrite text display layout")
    }

    fn draw_text_input(
        &mut self,
        _rect: Rect,
        _ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        todo!("Direct2D text input rasteriser")
    }

    fn text_input_layout(
        &self,
        _rect: Rect,
        _ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        todo!("Direct2D text input layout")
    }

    /// #28: real Direct2D/DirectWrite rasteriser via `win::tooltip` once a
    /// surface is attached. Renders `TooltipChrome::default()` — see
    /// [`Self::draw_tooltip_with_chrome`] for the full-chrome entry point.
    /// See [`Self::draw_status_bar`]'s doc for the "surface not attached
    /// yet" fallback posture.
    fn draw_tooltip(&mut self, tooltip: &Tooltip, layout: &TooltipLayout) {
        self.draw_tooltip_with_chrome(tooltip, layout, &TooltipChrome::default());
    }

    /// #28: see [`Self::draw_tooltip`]'s doc. Overridden (rather than
    /// relying on the trait's default delegate-to-`draw_tooltip` body) so
    /// a `Sides`/`None`/title chrome request is honoured, matching the
    /// TUI, GTK and macOS backends (see `Backend::draw_tooltip_with_chrome`'s
    /// doc).
    fn draw_tooltip_with_chrome(
        &mut self,
        tooltip: &Tooltip,
        layout: &TooltipLayout,
        chrome: &TooltipChrome,
    ) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::tooltip::draw_tooltip_with_chrome(
                &surface.target,
                dwrite,
                tooltip,
                layout,
                chrome,
                self.current_line_height,
                self.current_char_width,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (tooltip, layout, chrome);
        todo!("Direct2D tooltip rasteriser (no surface attached yet)")
    }

    /// #28: real Direct2D/DirectWrite rasteriser via `win::context_menu`
    /// once a surface is attached. `layout` is fully resolved upstream
    /// (see `crate::compose::menu_system`) — this only paints it and
    /// collects the per-clickable-item hit rectangles. See
    /// [`Self::draw_status_bar`]'s doc for the "surface not attached yet"
    /// fallback posture.
    fn draw_context_menu(
        &mut self,
        menu: &ContextMenu,
        layout: &ContextMenuLayout,
    ) -> Vec<(Rect, WidgetId)> {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::context_menu::draw_context_menu(&surface.target, dwrite, menu, layout);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (menu, layout);
        todo!("Direct2D context menu rasteriser (no surface attached yet)")
    }

    /// #28: real Direct2D/DirectWrite rasteriser via `win::dialog` once a
    /// surface is attached. `layout` is fully resolved upstream (see
    /// [`crate::primitives::dialog::Dialog::layout`]) — this only paints
    /// it and returns the per-button hit rectangles in
    /// `layout.visible_buttons` order. See [`Self::draw_status_bar`]'s
    /// doc for the "surface not attached yet" fallback posture.
    fn draw_dialog(&mut self, dialog: &Dialog, layout: &DialogLayout) -> Vec<Rect> {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::dialog::draw_dialog(
                &surface.target,
                dwrite,
                dialog,
                layout,
                self.current_line_height,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (dialog, layout);
        todo!("Direct2D dialog rasteriser (no surface attached yet)")
    }

    /// #27: real Direct2D/DirectWrite rasteriser via `win::multi_section_view`
    /// once a surface is attached. See [`Self::draw_status_bar`]'s doc for
    /// the "surface not attached yet" fallback posture.
    fn draw_multi_section_view(&mut self, rect: Rect, view: &MultiSectionView) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::multi_section_view::draw_multi_section_view(
                &surface.target,
                dwrite,
                rect,
                view,
                self.current_line_height,
                self.current_char_width,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, view);
        todo!("Direct2D MSV rasteriser (no surface attached yet)")
    }

    /// #27: like [`Self::tree_layout`], this is pure measurement — only
    /// needs `line_height`, not a live render target or `self.dwrite`
    /// (`win_msv_layout`'s `body_measure` is row-count-based, not real
    /// text measurement, mirroring `MacBackend::msv_layout`/
    /// `GtkBackend::msv_layout`) — but still lives in a Windows-only
    /// module, so this only needs the `target_os = "windows"` gate every
    /// method in this file shares. No `return` in the `windows` arm
    /// (unlike [`Self::draw_multi_section_view`] above): on
    /// `target_os = "windows"` the `not(windows)` block below is
    /// stripped, so this block *is* the tail expression and an explicit
    /// `return` trips `clippy::needless_return` under CI's `-D warnings`
    /// — see [`Self::activity_bar_layout`]'s doc for the same pattern.
    fn msv_layout(&self, rect: Rect, view: &MultiSectionView) -> MultiSectionViewLayout {
        #[cfg(target_os = "windows")]
        {
            super::multi_section_view::win_msv_layout(view, rect, self.current_line_height)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, view);
            todo!("DirectWrite MSV layout")
        }
    }

    /// #27: see [`Self::msv_layout`]'s doc for both the "no measurer
    /// needed, but still Windows-only" posture and the no-`return`
    /// pattern. `allow_resize` is hardcoded `false` here (no `view` to
    /// read it from), matching `MacBackend::msv_metrics`/
    /// `GtkBackend::msv_metrics`'s identical shortcut. Callers that need
    /// the resize-aware divider size should go through
    /// [`Self::msv_layout`] instead.
    fn msv_metrics(&self) -> LayoutMetrics {
        #[cfg(target_os = "windows")]
        {
            super::multi_section_view::win_msv_metrics(self.current_line_height, false)
        }
        #[cfg(not(target_os = "windows"))]
        {
            todo!("DirectWrite MSV metrics")
        }
    }

    /// #26: like [`Self::activity_bar_layout`], this needs no measurer
    /// at all — chevron width is a `line_height`-derived estimate (see
    /// `win::tree::win_tree_layout`'s doc), not a real DirectWrite
    /// measurement — so this doesn't even need `self.dwrite`, only the
    /// `target_os = "windows"` gate every method in this file shares
    /// (the callee lives in a Windows-only module).
    fn tree_layout(&self, rect: Rect, tree: &TreeView) -> TreeViewLayout {
        #[cfg(target_os = "windows")]
        {
            super::tree::win_tree_layout(tree, rect, self.current_line_height)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, tree);
            todo!("DirectWrite tree layout")
        }
    }

    /// #26: pure measurement — only needs `self.dwrite`, not a live
    /// render target. See [`Self::status_bar_layout`]'s doc.
    fn form_layout(&self, rect: Rect, form: &Form) -> FormLayout {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = &self.dwrite {
            return super::form::win_form_layout(dwrite, rect, form, self.current_line_height);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, form);
        todo!("DirectWrite form layout (no surface attached yet)")
    }

    /// #26: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture. `rect` is unused on the painted
    /// path — `editor.rect` is authoritative (mirrors
    /// `GtkBackend::draw_editor`, which does the same).
    fn draw_editor(&mut self, rect: Rect, editor: &Editor) -> EditorPaintResult {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            let _ = rect;
            return super::editor::draw_editor(
                &surface.target,
                dwrite,
                editor,
                self.current_char_width,
                self.current_line_height,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, editor);
        todo!("Direct2D editor rasteriser (no surface attached yet)")
    }

    fn draw_message_list(&mut self, _rect: Rect, _list: &MessageList) {
        todo!("Direct2D message list rasteriser")
    }

    /// #28: real Direct2D/DirectWrite rasteriser via `win::rich_text_popup`
    /// once a surface is attached. The rasteriser returns per-link hit
    /// regions for its own tests; the trait signature discards them (same
    /// posture as `GtkBackend::draw_rich_text_popup` — hosts that need
    /// link hit-testing query `popup.layout(...).hit_test(...)` directly).
    /// See [`Self::draw_status_bar`]'s doc for the "surface not attached
    /// yet" fallback posture.
    fn draw_rich_text_popup(&mut self, popup: &RichTextPopup, layout: &RichTextPopupLayout) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            let _ = super::rich_text_popup::draw_rich_text_popup(
                &surface.target,
                dwrite,
                popup,
                layout,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (popup, layout);
        todo!("Direct2D rich text popup rasteriser (no surface attached yet)")
    }

    /// #28: real Direct2D/DirectWrite rasteriser via `win::find_replace`
    /// once a surface is attached. See [`Self::draw_status_bar`]'s doc
    /// for the "surface not attached yet" fallback posture.
    fn draw_find_replace(&mut self, rect: Rect, panel: &FindReplacePanel) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            let _ = rect;
            super::find_replace::draw_find_replace(
                &surface.target,
                dwrite,
                panel,
                self.current_line_height,
                self.current_char_width,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, panel);
        todo!("Direct2D find/replace rasteriser (no surface attached yet)")
    }

    /// #28: real Direct2D/DirectWrite rasteriser via `win::completions`
    /// once a surface is attached. See [`Self::draw_status_bar`]'s doc
    /// for the "surface not attached yet" fallback posture.
    fn draw_completions(&mut self, completions: &Completions, layout: &CompletionsLayout) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::completions::draw_completions(&surface.target, dwrite, completions, layout);
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (completions, layout);
        todo!("Direct2D completions rasteriser (no surface attached yet)")
    }

    /// #27: real Direct2D rasteriser via `win::scrollbar` once a surface
    /// is attached. See [`Self::draw_status_bar`]'s doc for the "surface
    /// not attached yet" fallback posture. `rect` is unused on the
    /// painted path — `scrollbar.track` is authoritative, mirroring
    /// `MacBackend::draw_scrollbar`/`GtkBackend::draw_scrollbar`.
    fn draw_scrollbar(&mut self, rect: Rect, scrollbar: &Scrollbar) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            let _ = rect;
            super::scrollbar::draw_scrollbar(
                &surface.target,
                scrollbar,
                &crate::theme::Theme::default(),
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, scrollbar);
        todo!("Direct2D scrollbar rasteriser (no surface attached yet)")
    }

    fn draw_drop_overlay(&mut self, _overlay: &crate::primitives::drop_zone::DropOverlay) {
        todo!("Direct2D drop overlay rasteriser")
    }

    /// #25: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_menu_bar(&mut self, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::menu_bar::draw_menu_bar(&surface.target, dwrite, rect, bar);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, bar);
        todo!("Direct2D menu bar rasteriser (no surface attached yet)")
    }

    /// #25: see [`Self::status_bar_layout`]'s doc for why this only needs
    /// `self.dwrite`.
    fn menu_bar_layout(&self, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = &self.dwrite {
            return super::menu_bar::win_menu_bar_layout(dwrite, rect, bar);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, bar);
        todo!("DirectWrite menu bar layout (no surface attached yet)")
    }

    fn draw_split(&mut self, _rect: Rect, _split: &Split) -> SplitLayout {
        todo!("Direct2D split rasteriser")
    }

    fn split_layout(&self, _rect: Rect, _split: &Split) -> SplitLayout {
        todo!("DirectWrite split layout")
    }

    fn draw_split_tree(
        &mut self,
        _rect: Rect,
        _tree: &crate::primitives::split_tree::SplitTree,
    ) -> crate::primitives::split_tree::SplitTreeLayout {
        todo!("Direct2D split-tree rasteriser")
    }

    fn split_tree_layout(
        &self,
        _rect: Rect,
        _tree: &crate::primitives::split_tree::SplitTree,
    ) -> crate::primitives::split_tree::SplitTreeLayout {
        todo!("DirectWrite split-tree layout")
    }

    fn draw_board(&mut self, _rect: Rect, _model: &crate::BoardModel) -> crate::BoardLayout {
        todo!("Direct2D board rasteriser")
    }

    fn draw_minimap(
        &mut self,
        _rect: Rect,
        _minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::backend::MinimapPaintResult {
        todo!("Direct2D minimap rasteriser — out of scope per #382")
    }

    fn minimap_layout(
        &self,
        _rect: Rect,
        _minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::primitives::minimap::MinimapLayout {
        todo!("Direct2D minimap layout — out of scope per #382")
    }

    fn draw_image(
        &mut self,
        _rect: Rect,
        _image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        todo!("Direct2D image rasteriser — out of scope per #662's first pass")
    }

    fn draw_panel(&mut self, _rect: Rect, _panel: &Panel) -> PanelLayout {
        todo!("Direct2D panel rasteriser")
    }

    fn panel_layout(&self, _rect: Rect, _panel: &Panel) -> PanelLayout {
        todo!("DirectWrite panel layout")
    }

    fn draw_toast_stack(&mut self, _rect: Rect, _stack: &ToastStack) -> ToastStackLayout {
        todo!("Direct2D toast stack rasteriser")
    }

    fn toast_stack_layout(&self, _rect: Rect, _stack: &ToastStack) -> ToastStackLayout {
        todo!("DirectWrite toast stack layout")
    }

    fn draw_pipeline_view(
        &mut self,
        _rect: Rect,
        _view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        todo!("Direct2D pipeline view rasteriser")
    }

    fn pipeline_view_layout(
        &self,
        _rect: Rect,
        _view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        todo!("DirectWrite pipeline view layout")
    }

    fn draw_progress(&mut self, _rect: Rect, _bar: &ProgressBar) -> ProgressBarLayout {
        todo!("Direct2D progress bar rasteriser")
    }

    fn progress_layout(&self, _rect: Rect, _bar: &ProgressBar) -> ProgressBarLayout {
        todo!("DirectWrite progress layout")
    }

    fn draw_spinner(&mut self, _rect: Rect, _spinner: &Spinner) -> SpinnerLayout {
        todo!("Direct2D spinner rasteriser")
    }

    fn spinner_layout(&self, _rect: Rect, _spinner: &Spinner) -> SpinnerLayout {
        todo!("DirectWrite spinner layout")
    }

    fn draw_command_center(&mut self, _rect: Rect, _cc: &CommandCenter) -> CommandCenterLayout {
        todo!("Direct2D command center rasteriser")
    }

    fn command_center_layout(&self, _rect: Rect, _cc: &CommandCenter) -> CommandCenterLayout {
        todo!("DirectWrite command center layout")
    }

    fn draw_toolbar(
        &mut self,
        _rect: Rect,
        _bar: &crate::primitives::toolbar::Toolbar,
        _hovered_id: Option<&crate::types::WidgetId>,
        _pressed_id: Option<&crate::types::WidgetId>,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        todo!("Direct2D toolbar rasteriser")
    }

    fn toolbar_layout(
        &self,
        _rect: Rect,
        _bar: &crate::primitives::toolbar::Toolbar,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        todo!("DirectWrite toolbar layout")
    }

    fn draw_sidebar_panel(
        &mut self,
        _rect: Rect,
        _panel: &crate::primitives::sidebar_panel::SidebarPanel,
        _hovered_toolbar_id: Option<&crate::types::WidgetId>,
        _pressed_toolbar_id: Option<&crate::types::WidgetId>,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        todo!("Direct2D sidebar-panel rasteriser")
    }

    fn sidebar_panel_layout(
        &self,
        _rect: Rect,
        _panel: &crate::primitives::sidebar_panel::SidebarPanel,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        todo!("DirectWrite sidebar-panel layout")
    }

    fn draw_diff_view(
        &mut self,
        _rect: Rect,
        _view: &crate::primitives::diff_view::DiffView,
    ) -> crate::primitives::diff_view::DiffViewLayout {
        todo!("Direct2D DiffView rasteriser")
    }

    /// #26: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_chart(
        &mut self,
        rect: Rect,
        chart: &crate::primitives::chart::Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) -> crate::primitives::chart::ChartLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::chart::draw_chart(
                &surface.target,
                dwrite,
                rect,
                chart,
                self.current_char_width,
                self.current_line_height,
                hovered_point,
                crosshair_x,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, chart, hovered_point, crosshair_x);
        todo!("Direct2D chart rasteriser (no surface attached yet)")
    }

    /// #26: like [`Self::tree_layout`], no measurer is needed — chart
    /// tick-label sizing only uses `char_width`/`line_height`, both
    /// already cross-platform fields — so this doesn't need
    /// `self.dwrite`, only the `target_os = "windows"` gate every
    /// method in this file shares (the callee lives in a Windows-only
    /// module).
    fn chart_layout(
        &self,
        rect: Rect,
        chart: &crate::primitives::chart::Chart,
    ) -> crate::primitives::chart::ChartLayout {
        #[cfg(target_os = "windows")]
        {
            super::chart::win_chart_layout(
                chart,
                rect,
                self.current_char_width,
                self.current_line_height,
            )
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, chart);
            todo!("DirectWrite chart layout")
        }
    }
}
