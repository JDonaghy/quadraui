//! `WinBackend` — Direct2D + DirectWrite implementation of [`Backend`].
//!
//! The render target, text infrastructure, and rasterisers are described in
//! the "Implementation notes" section below.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::backend::{
    Backend, BackendError, PlatformServices, PointerShape, ServiceResult, WindowControl,
};
use crate::dispatch::{DoubleClickDetector, DragState, TextRegion};
use crate::event::{Point, Rect, UiEvent, Viewport};
#[cfg(target_os = "windows")]
use crate::generic_font::GenericFamily;
use crate::modal_stack::ModalStack;
use crate::paint_surface::PaintSurface;
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::spinner::{Spinner, SpinnerLayout};
// Only named by the Windows tests below.
#[cfg(all(test, target_os = "windows"))]
use crate::primitives::toolbar::ToolbarButton;
use crate::types::WidgetId;
use crate::{Accelerator, AcceleratorId, Key, ListView, Modifiers, UserPayload};
// `ActivityBar`, `MenuBar`, `StatusBar` and `TabBar` are only named by
// `#[cfg(test)]` code below — each paints through its widget contract, not a
// method on this backend. `ContextMenu` is named only by the Windows-host
// tests.
#[cfg(all(test, target_os = "windows"))]
use crate::ContextMenu;
#[cfg(all(test, target_os = "windows"))]
use crate::Theme;
#[cfg(test)]
use crate::{ActivityBar, MenuBar, StatusBar, TabBar};
// `AcceleratorScope` is only referenced by `#[cfg(test)]` code below.
// Gate the import so a non-test build doesn't flag it as unused.
#[cfg(test)]
use crate::AcceleratorScope;

use super::services::WinPlatformServices;

// ─── Cross-platform fallback measurement ────────────────────────────────
//
// Every `draw_*`/`*_layout` method below that needs a text-width
// measurement gated on `self.dwrite` being `Some` — built once by
// `attach_surface`/`attach_headless`, both `target_os = "windows"`-only —
// and fell through to an unconditional `todo!()` otherwise. That `None`
// state isn't Windows-vs-not: it's also the state a fresh
// `WinBackend::new()` starts in (on *any* platform) and the state
// `end_frame` drops back to after a device-lost `EndDraw` failure, so a
// live Windows host hit the same panic a Linux `cargo test --features
// win` run does. `nominal_text_width` gives every such site a plausible, finite substitute (`char_width` per character,
// no real glyph metrics) instead — real widths resume automatically once
// a surface exists.
fn nominal_text_width(text: &str, char_width: f32) -> f32 {
    text.chars().count() as f32 * char_width
}

/// Whether rects `a` and `b` share any area — used by
/// [`WinBackend::draw_focus_ring`] to avoid stroking on top of a
/// tooltip/toast painted earlier this frame; see
/// `overlay_bounds_this_frame`'s own doc. A shared edge with zero overlap
/// width/height doesn't count as overlapping. Mirrors
/// `macos::backend::rects_overlap`.
fn rects_overlap(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

/// Whether `overlay` would land on the pixels [`WinBackend::draw_focus_ring`]
/// actually paints for `rect` at `stroke_width`.
///
/// `surface_stroke_rect` centers a `stroke_width`-thick line on each of
/// `rect`'s four edges, so the ring only occupies the thin band straddling
/// those edges by `stroke_width / 2` in each direction — never `rect`'s full
/// interior. An overlay that lands strictly inside `rect` without reaching
/// any edge (e.g. a toast inset from the viewport it's raised over, landing
/// inside a focused widget that spans that same viewport) never shares a
/// pixel with the ring, so it must not suppress the stroke. Mirrors
/// `macos::backend::focus_ring_band_overlaps`.
fn focus_ring_band_overlaps(rect: Rect, stroke_width: f32, overlay: Rect) -> bool {
    let half = stroke_width / 2.0;
    let top = Rect::new(
        rect.x - half,
        rect.y - half,
        rect.width + stroke_width,
        stroke_width,
    );
    let bottom = Rect::new(
        rect.x - half,
        rect.y + rect.height - half,
        rect.width + stroke_width,
        stroke_width,
    );
    let left = Rect::new(
        rect.x - half,
        rect.y - half,
        stroke_width,
        rect.height + stroke_width,
    );
    let right = Rect::new(
        rect.x + rect.width - half,
        rect.y - half,
        stroke_width,
        rect.height + stroke_width,
    );
    [top, bottom, left, right]
        .into_iter()
        .any(|band| rects_overlap(band, overlay))
}

/// [`super::spinner::SpinnerMeasureSource`] impl backed by
/// [`nominal_text_width`] for the width and a fixed `line_height` for the
/// height (`DWrite::measure_text` isn't available yet) — matches the
/// pre-#1078 `win_spinner_nominal_layout`'s behaviour exactly.
struct NominalSpinnerMeasure {
    char_width: f32,
    line_height: f32,
}

impl super::spinner::SpinnerMeasureSource for NominalSpinnerMeasure {
    fn measure(&self, text: &str) -> (f32, f32) {
        (nominal_text_width(text, self.char_width), self.line_height)
    }
}

// ─── Direct2D bootstrap ─────────────────────────────────────────────────
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
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_UNKNOWN, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1RenderTarget,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES,
};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::DirectWrite::{IDWriteFontCollection1, IDWriteFontFallback};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
// `MonitorFromWindow`/`GetMonitorInfoW`/`MONITORINFO` live under `Gdi`
// in the `windows` crate, not `WindowsAndMessaging`, though they're
// conceptually "window" APIs — already-enabled `Win32_Graphics_Gdi`
// feature (Cargo.toml), no new feature needed.
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::HiDpi::GetDpiForWindow;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowLongPtrW, GetWindowRect, IsZoomed, LoadCursorW, PostMessageW,
    SetCursor, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
    ShowWindow, GWL_STYLE, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, IDC_SIZENESW, IDC_SIZENS,
    IDC_SIZENWSE, IDC_SIZEWE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, WS_CAPTION,
    WS_OVERLAPPEDWINDOW,
};

#[cfg(target_os = "windows")]
use super::text::{new_brush_cache, BrushCache, DWrite};

/// Default editor font family for the Win-GUI backend — the historical
/// default monospace face shipped with every Windows version since Vista,
/// same role as GTK's hardcoded `"Monospace"` fontconfig alias.
#[cfg(target_os = "windows")]
const DEFAULT_EDITOR_FONT_FAMILY: &str = "Consolas";

/// Default chrome (UI) font family for the Win-GUI backend until
/// [`Backend::set_ui_font`] overrides it — the system UI font on
/// every Windows version since Vista, same role as GTK's `"Sans 11"`
/// fallback for `ui_font`.
#[cfg(target_os = "windows")]
const DEFAULT_UI_FONT_FAMILY: &str = "Segoe UI";

/// Default chrome font size in points, paired with
/// [`DEFAULT_UI_FONT_FAMILY`]. Unlike macOS (where 1pt == 1px), the Win
/// backend converts points to DIPs via `pt_to_dip` (`size_pt * 96 / 72`,
/// see `win/msg.rs`), so this is deliberately not a round point value: it's
/// chosen so the *rendered* size lands on VS Code's own 13 DIP Windows
/// chrome text size (`9.75 * 96 / 72 == 13.0`). Not `#[cfg(target_os =
/// "windows")]`, unlike most consts in this module — [`Backend::default_fonts`]
/// reads it directly (on every host, per that method's doc) so the font
/// `WinBackend::new` actually installs and the size `default_fonts()`
/// advertises can never drift apart again.
const DEFAULT_UI_FONT_SIZE_PT: f32 = 9.75;

/// Parse a Pango-style font description (e.g. `"Segoe UI 11"`) into a
/// DirectWrite-ready `(family, size_pt)` pair. A trailing whitespace-separated
/// numeric token is the point size (defaulting to [`DEFAULT_UI_FONT_SIZE_PT`]),
/// and everything before it is a comma-separated family fallback list, taking
/// the first entry. A CSS/Pango generic token resolves to its DirectWrite
/// default; a concrete name is trusted as-is. Falls back to
/// `(DEFAULT_UI_FONT_FAMILY, DEFAULT_UI_FONT_SIZE_PT)` when `desc` has no
/// usable family at all.
#[cfg(target_os = "windows")]
fn parse_ui_font_desc(desc: &str) -> (String, f32) {
    let desc = desc.trim();
    let (family_part, size_pt) = match desc.rfind(' ') {
        Some(idx) if desc[idx + 1..].trim().parse::<f32>().is_ok() => {
            let (family, size_str) = desc.split_at(idx);
            (
                family.trim(),
                size_str
                    .trim()
                    .parse::<f32>()
                    .expect("just matched by the guard above"),
            )
        }
        _ => (desc, DEFAULT_UI_FONT_SIZE_PT),
    };
    let first = family_part
        .split(',')
        .map(str::trim)
        .find(|f| !f.is_empty());
    match first {
        None => (DEFAULT_UI_FONT_FAMILY.to_string(), DEFAULT_UI_FONT_SIZE_PT),
        Some(candidate) => match GenericFamily::parse(candidate) {
            Some(GenericFamily::Monospace) => (DEFAULT_EDITOR_FONT_FAMILY.to_string(), size_pt),
            Some(GenericFamily::SansSerif | GenericFamily::SystemUi) => {
                (DEFAULT_UI_FONT_FAMILY.to_string(), size_pt)
            }
            None => (candidate.to_string(), size_pt),
        },
    }
}

/// A live Direct2D render target: either a window-bound
/// `ID2D1HwndRenderTarget` or an offscreen `ID2D1DCRenderTarget` for
/// headless tests. Every rasteriser only needs the shared `ID2D1RenderTarget`
/// base interface — both derive from it and forward via `Deref` — so none of
/// the `&surface.target` call sites need to know which kind they're painting
/// onto. The exception is [`WinBackend::resize_surface`]'s `Resize` call,
/// which is declared only on `ID2D1HwndRenderTarget` and so matches the
/// variant directly.
#[cfg(target_os = "windows")]
enum RenderTarget {
    Hwnd(ID2D1HwndRenderTarget),
    Dc(ID2D1DCRenderTarget),
}

#[cfg(target_os = "windows")]
impl std::ops::Deref for RenderTarget {
    type Target = ID2D1RenderTarget;

    fn deref(&self) -> &ID2D1RenderTarget {
        match self {
            RenderTarget::Hwnd(target) => target,
            RenderTarget::Dc(target) => target,
        }
    }
}

/// The live Direct2D handles behind a real Win32 window, or an
/// offscreen headless surface. Only exists once
/// [`WinBackend::attach_surface`] or [`WinBackend::attach_headless`] has
/// run — a `WinBackend` constructed standalone (before either has run)
/// has `surface: None` and every draw call degrades to its no-paint
/// fallback until a later issue lands a real rasteriser.
#[cfg(target_os = "windows")]
struct Surface {
    /// Kept alive only because `ID2D1HwndRenderTarget` was created from
    /// it — never called again after `attach_surface` populates `target`.
    /// `None` for [`WinBackend::attach_headless`]: `ID2D1DCRenderTarget`
    /// keeps its own creating factory alive internally, and the caller
    /// (`super::testing::HeadlessSurface`) already owns a reference of
    /// its own, so there's no second factory to keep alive here.
    #[allow(dead_code)]
    factory: Option<ID2D1Factory>,
    target: RenderTarget,
    /// Brushes created against `target`, reused across frames.
    /// Lives and dies with this `Surface`, so a recreated target always
    /// starts from an empty cache — see `super::text`'s `Brush cache`
    /// section for how paint helpers reach it.
    brushes: BrushCache,
    /// Decoded images bound to `target`, dropped with it like `brushes`.
    images: crate::image_cache::ImageCache<windows::Win32::Graphics::Direct2D::ID2D1Bitmap>,
}

/// Position tolerance, in DIPs, for [`WinBackend::fold_double_click`]'s
/// [`DoubleClickDetector`].
///
/// `win::events`' `win_button_down` (via `super::msg::point_from_lparam`)
/// divides device pixels by the window's DPI scale before building a
/// `Point`, so `MouseDown` positions here are point-precision DIPs — not
/// TUI's whole character cells — same situation `MacBackend` documents for
/// its own `MAC_DOUBLE_CLICK_RADIUS`. The detector's default radius
/// (`crate::dispatch::DOUBLE_CLICK_RADIUS`, 1.5, tuned for TUI's integral
/// cell grid) is far tighter than two real clicks can reliably land
/// within, so this reuses the same heuristic value macOS settled on rather
/// than inventing a third unverified constant.
const WIN_DOUBLE_CLICK_RADIUS: f32 = 4.0;

/// Custom `wndproc` message posted via `PostMessageW` to wake a possibly-
/// blocked message loop from a background thread. `win::run`'s `wndproc`
/// match arm on this value drains and dispatches — see that arm's doc.
/// Applications can use `WM_APP` (`0x8000`) through `0xBFFF`; the `windows`
/// crate has no `WM_APP` constant of its own, so this is defined locally.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) const WM_QUADRAUI_USER_EVENT: u32 = 0x8000 + 1;

/// `NOTIFYICONDATAW::uCallbackMessage` — the message
/// `Shell_NotifyIconW`'s tray icon posts back to this app's `HWND` on
/// every mouse event over the icon (`lparam` carries the Win32 mouse
/// message: `WM_LBUTTONUP`/`WM_RBUTTONUP`/etc — see
/// `super::tray::tray_click_button`'s doc for the decode). `win::run`'s
/// `wndproc` match arm on this value is what actually decodes and
/// dispatches — see that arm's doc. Same `WM_APP`-range numbering
/// rationale as [`WM_QUADRAUI_USER_EVENT`] above, offset by one so the
/// two custom messages never collide.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) const WM_QUADRAUI_TRAY_CALLBACK: u32 = 0x8000 + 2;

/// `SetTimer`/`KillTimer`'s `nIDEvent` for [`WinBackend::request_frame_in`]'s
/// one-shot scheduled-wake timer — the sibling of
/// [`WM_QUADRAUI_USER_EVENT`] for a scheduled frame rather than a
/// cross-thread wake. Distinct from `win::run`'s `SMOKE_TIMER_ID` (1) and
/// `RESIZE_TIMER_ID` (2) — Win32 timer ids are scoped per-`hwnd`, and any
/// of the three can legitimately be live on the same window at once.
/// `win::run`'s `wndproc` `WM_TIMER` arm on this id is what actually
/// kills the timer and re-dispatches, mirroring `WM_QUADRAUI_USER_EVENT`'s
/// arm.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) const FRAME_TIMER_ID: usize = 3;

/// # Before a surface is attached
///
/// `self.surface`/`self.dwrite` are always populated together by
/// [`Self::attach_surface`]/`attach_headless`, so a standalone
/// `WinBackend` no window has attached to yet has neither. Every
/// rasteriser then paints nothing and every layout query falls back to
/// [`nominal_text_width`] instead of panicking; the next real attach
/// resumes painting.
pub struct WinBackend {
    viewport: Viewport,
    /// `Rc<RefCell<>>` (not a plain field) so [`Backend::modal_stack_handle`]
    /// can hand back a handle that outlives any single `&mut self`
    /// borrow — the shape `GtkBackend`/`TuiBackend`/`MacBackend` already
    /// use for this.
    modal_stack: Rc<RefCell<ModalStack>>,
    /// See `modal_stack`'s doc comment — same rationale.
    drag_state: Rc<RefCell<DragState>>,
    /// Folds a `MouseDown` translated from `WM_LBUTTONDOWN`/`WM_RBUTTONDOWN`/
    /// etc. into `DoubleClick` when it lands within the time/position
    /// window of the previous click. `win::run`'s `wndproc`
    /// dispatches synchronously per Win32 message — same model as
    /// `macos::run` per `NSEvent` — so this runs on one event at a time
    /// via [`Self::fold_double_click`] rather than `TuiBackend`'s
    /// per-poll batch. Mirrors `MacBackend::double_click`; deliberately
    /// *not* a new `WM_*BUTTONDBLCLK` translator — see that issue for why.
    double_click: DoubleClickDetector,
    /// Accelerator registry + text-selection bookkeeping — see
    /// [`crate::backend_core::BackendCore`]'s module doc. Replaces the
    /// formerly Win-local `accelerators`/`parsed_accelerators` fields
    /// verbatim, plus (below) the formerly separate `text_selection`
    /// field.
    core: crate::backend_core::BackendCore,
    /// `WidgetId` of the [`ActivityBar`] that declared
    /// `is_keyboard_focused = true` in the most recently painted frame
    /// (set by [`Backend::end_frame`]), or `None` if no bar is focused.
    /// Read by `win::run::dispatch_event` to redirect a `KeyPressed` into
    /// `UiEvent::ActivityBar(...)` instead of reaching `AppLogic::handle`
    /// as a raw key — the Win-GUI twin of `GtkBackend`/`MacBackend`'s
    /// `focused_activity_bar`. Without it, `ShellAdapter`'s activity-bar
    /// keyboard navigation is unreachable on this backend.
    focused_activity_bar: Option<WidgetId>,
    /// Queue-adapter between event producers and the trait's poll-style
    /// [`Backend::poll_events`]/[`Backend::wait_events`] — the
    /// `docs/BACKEND.md` "option A event queue" shape, same
    /// `Rc<RefCell<VecDeque<UiEvent>>>` field GTK/macOS ship
    /// (`GtkBackend::events`/`MacBackend::events`). `Rc<RefCell<>>` (not a
    /// plain field) so a producer holding only an [`Self::events_handle`]
    /// clone — a future `wndproc` wiring, or a test via [`Self::push_event`]
    /// — can push without a `&mut WinBackend` borrow. Not `target_os`-gated:
    /// the queue itself is plain `VecDeque<UiEvent>` with no WinAPI
    /// dependency, same rationale as `current_pointer_shape` above — only a
    /// real Win32 producer wiring this queue to `wndproc`'s live message
    /// pump would need a real host, and nothing does that yet (see
    /// `win::run`'s "Dispatch model" module doc: the live message loop
    /// still dispatches directly to `AppLogic::handle`, mirroring GTK's own
    /// "ships the API but producer wiring is separate work" posture).
    events: Rc<RefCell<VecDeque<UiEvent>>>,
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
    /// The `ID2D1DCRenderTarget` [`Self::attach_headless`] was given,
    /// kept alongside (not inside) `surface` so [`Self::ensure_surface`]
    /// can rebuild `surface` after `end_frame`'s device-lost recovery
    /// drops it — mirroring `hwnd` below, which does the same job for a
    /// real window. `None` for a `WinBackend` that never called
    /// `attach_headless` (every live app, `attach_surface`'s `hwnd`
    /// path) — cloning an `ID2D1DCRenderTarget` is a cheap COM `AddRef`,
    /// same rationale as `Surface`'s own `factory: None` comment for why
    /// this needs no separate factory to keep alive.
    #[cfg(target_os = "windows")]
    headless_target: Option<ID2D1DCRenderTarget>,
    /// The last `HWND` a surface was successfully attached to, kept
    /// *outside* `Surface` (and outlasting it) so a dropped surface
    /// (`end_frame`'s device-lost recovery) can still be re-created by
    /// [`Self::ensure_surface`] without needing `win::run` to thread the
    /// `HWND` back through a second time. `None` until the first
    /// `attach_surface` call succeeds.
    #[cfg(target_os = "windows")]
    hwnd: Option<HWND>,
    /// Tray/status-bar icon state — see [`super::tray`]'s
    /// module doc and `impl TrayService for WinBackend` below.
    #[cfg(target_os = "windows")]
    tray: super::tray::WinTrayState,
    /// Window rect + `GWL_STYLE` value saved when entering fullscreen, so
    /// exiting can restore both — Win-GUI's fullscreen is a manual
    /// `WS_OVERLAPPEDWINDOW` style strip + `SetWindowPos` to the monitor
    /// rect, not a single reversible OS call the way GTK or AppKit provide.
    /// `None` outside fullscreen.
    #[cfg(target_os = "windows")]
    fullscreen_saved: Option<(RECT, isize)>,
    /// Live, thread-safe mirror of `hwnd`'s raw handle value, read fresh on
    /// every invocation of the [`Self::waker`] closure. `HWND` itself isn't
    /// `Send`, so it can't be captured directly into the `Send + Sync`
    /// closure — but `waker()` is callable during `AppLogic::setup`, *before*
    /// [`Self::attach_surface`] has run. A closure capturing `self.hwnd`
    /// as a one-time snapshot would permanently bake in "no window", silently
    /// dropping every future wake nudge since a live `GetMessageW` loop
    /// blocks indefinitely with no other way to notice a queued payload.
    /// An `AtomicIsize` shared between `self` and the closure lets the
    /// closure re-read the *current* value at invocation time, mirroring the
    /// live-lookup pattern `GtkBackend::waker` and `MacBackend::waker` use.
    /// `0` means "no window yet", mirroring `hwnd`'s own `None` — a real
    /// `HWND` is never null.
    #[cfg(target_os = "windows")]
    hwnd_raw: std::sync::Arc<std::sync::atomic::AtomicIsize>,
    /// DirectWrite factory + `IDWriteTextFormat` for the current editor
    /// font (`editor_font_family`/`editor_font_size_pt`). `None` until
    /// [`Self::attach_surface`] creates it — same lifecycle as `surface`,
    /// and recreated alongside it whenever [`Self::ensure_surface`]'s
    /// device-lost recovery re-attaches.
    #[cfg(target_os = "windows")]
    dwrite: Option<DWrite>,
    /// Font family used to paint text via DirectWrite. Defaults to
    /// [`DEFAULT_EDITOR_FONT_FAMILY`]. Set via [`Backend::set_editor_font`];
    /// read when constructing the `IDWriteTextFormat` — mirrors GTK's
    /// `editor_font_family`. `cfg`-gated like `dwrite`.
    #[cfg(target_os = "windows")]
    editor_font_family: String,
    /// Editor font size in points, paired with `editor_font_family`.
    /// Defaults to `11.0`, matching GTK's default.
    #[cfg(target_os = "windows")]
    editor_font_size_pt: f32,
    /// DirectWrite factory + `IDWriteTextFormat` for the current chrome
    /// (UI) font (`ui_font_family`/`ui_font_size_pt`) — the chrome twin
    /// of `dwrite` above. `None` until [`Self::attach_surface`] creates
    /// it; recreated alongside it the same way. Read via
    /// [`Self::chrome_dwrite`]; `Dialog` and every other
    /// `ChromePrimitive` rasteriser (`Tree`, `List`, `MenuBar`,
    /// `ContextMenu`, `CommandCenter`, `MultiSectionView`,
    /// `SidebarPanel`, `StatusBar`, `ActivityBar`, `Toolbar`, `TabBar`)
    /// paint chrome text through this handle, falling back to the
    /// editor `dwrite` handle if no live chrome one exists yet —
    /// matching GTK's `ui_font` swap and macOS's `chrome_font`.
    /// `RichTextPopup` paints through this handle too, but only for
    /// [`crate::FontRole::Chrome`] (the default, via plain
    /// `draw_rich_text_popup`) — a caller that asks for
    /// [`crate::FontRole::Editor`] via
    /// `Backend::draw_rich_text_popup_with_font_role` is routed to the
    /// editor `dwrite` handle instead.
    #[cfg(target_os = "windows")]
    chrome_dwrite: Option<DWrite>,
    /// Chrome font family, parsed from the Pango-style description
    /// [`Backend::set_ui_font`] receives via [`parse_ui_font_desc`].
    /// Defaults to [`DEFAULT_UI_FONT_FAMILY`]. `cfg`-gated like
    /// `editor_font_family`: nothing outside the `target_os = "windows"`
    /// methods reads or writes it.
    #[cfg(target_os = "windows")]
    ui_font_family: String,
    /// Chrome font size in points, paired with `ui_font_family`. Defaults
    /// to [`DEFAULT_UI_FONT_SIZE_PT`].
    #[cfg(target_os = "windows")]
    ui_font_size_pt: f32,
    /// Live font fallback built by [`Backend::set_nerd_font_fallback`]
    /// — `None` until that method is called. Read by
    /// [`Self::attach_surface`]/[`Self::attach_headless`] when they build
    /// [`DWrite`] for the editor/chrome fonts, so a `set_nerd_font_fallback`
    /// call made before either (the documented "call once from `setup()`"
    /// convention every font setter on this backend shares) is baked into
    /// both `IDWriteTextFormat`s at construction. Like `editor_font_family`
    /// above, a live surface does not rebuild immediately on a later
    /// change — same accepted limitation `set_editor_font`'s doc already
    /// names.
    #[cfg(target_os = "windows")]
    nerd_font_fallback: Option<IDWriteFontFallback>,
    /// Private font collection built by the most recent
    /// [`Backend::register_font_from_memory`] call, if any — passed to
    /// [`crate::win::text::build_nerd_font_fallback`] so a later
    /// `set_nerd_font_fallback(family)` resolves `family` against the
    /// font this process actually registered rather than a same-named
    /// system font. Only the most recent registration is
    /// kept: nothing in this backend's contract promises more than one
    /// app-registered font resolves at a time, mirroring
    /// `MacBackend`/vimcode's own single-subset-font usage.
    #[cfg(target_os = "windows")]
    registered_font_collection: Option<IDWriteFontCollection1>,
    /// Private font collection for the bundled codicon font, built once
    /// by [`Self::new`] via [`crate::win::text::register_font_from_memory`] —
    /// kept separate from `registered_font_collection`,
    /// which is reserved for the app's own
    /// [`Backend::register_font_from_memory`] calls and is overwritten
    /// by each one; this field must survive those calls so
    /// [`Self::nerd_font_fallback`] can always resolve the codicon
    /// family regardless of what the app has registered since. No
    /// equivalent to `registered_font_bytes`'s owned-copy dance is
    /// needed: [`crate::codicon::FONT_BYTES`] is `'static` (compiled
    /// into the binary's own data section), so it already outlives
    /// every `IDWriteFontFile`/collection/fallback built from it for the
    /// rest of the process.
    #[cfg(target_os = "windows")]
    codicon_font_collection: Option<IDWriteFontCollection1>,
    /// Every byte buffer ever handed to a
    /// [`Backend::register_font_from_memory`] call, in an owned copy
    /// that is **appended** here and never overwritten or dropped for
    /// the rest of the process's lifetime.
    ///
    /// This is not a cosmetic cache: `IDWriteInMemoryFontFileLoader::
    /// CreateInMemoryFontFileReference` (`win::text::register_font_from_memory`)
    /// does **not** copy the `fontData` it's given — the memory it
    /// points at must stay valid for as long as the `IDWriteFontFile` it
    /// returns, and everything built from that file (the private
    /// collection, any `IDWriteFontFallback` built against it, any live
    /// `IDWriteTextFormat`/`IDWriteTextLayout` that resolves through
    /// that fallback), remains alive — and DirectWrite gives this
    /// backend no callback for when that chain is actually done with
    /// the pointer (the API's `ownerObject` parameter exists precisely
    /// to supply one, but this backend passes `None` for it — see
    /// `register_font_from_memory`'s doc below).
    ///
    /// Rather than implement a custom `IUnknown` just to receive that
    /// signal, this field sidesteps the problem: nothing is ever freed.
    /// Each [`Backend::register_font_from_memory`] call's bytes are
    /// copied into a fresh `Vec` and pushed here *before* DirectWrite
    /// ever sees a pointer into it (never the caller's original `bytes`
    /// slice, which the trait's own doc explicitly allows to be a local
    /// dropped immediately after the call returns), and the entry is
    /// kept even if a later call replaces
    /// `registered_font_collection` — matching the "left registered for
    /// the process lifetime" posture `register_font_from_memory`
    /// already documents for the font-file loader itself.
    #[cfg(target_os = "windows")]
    registered_font_bytes: Vec<Vec<u8>>,
    /// The active [`crate::Theme`], set via [`Backend::set_theme`] and
    /// read by every `draw_*` rasteriser. Not `cfg`-gated: `Theme` is a plain,
    /// portable value with no WinAPI dependency — same rationale as
    /// `current_pointer_shape` — so this stays testable on every host.
    current_theme: crate::theme::Theme,
    /// Captured once per frame, mirroring `current_theme` — see
    /// [`crate::Style`]'s module doc for why this is a separate field
    /// rather than folded into `Theme`.
    current_style: crate::Style,
    /// The `PointerShape` [`Backend::set_cursor`] last applied — read back
    /// by `win::run`'s `WM_SETCURSOR` handler so the pointer glyph
    /// stays put across every `WM_SETCURSOR` Windows sends for the
    /// client area (on every mouse move, not just the ones that also
    /// deliver `WM_MOUSEMOVE`), instead of snapping back to the window
    /// class's `IDC_ARROW` the instant `DefWindowProcW` would otherwise
    /// handle it. Not `cfg`-gated like `surface`/`dwrite` above: the enum
    /// itself is a plain, portable value with no WinAPI dependency, and
    /// [`Backend::set_cursor`] (below) always records it regardless of
    /// host — only *applying* it via `SetCursor` is Windows-only.
    current_pointer_shape: PointerShape,
    /// Whether [`Self::begin_frame`]/[`Self::end_frame`] should bracket
    /// the frame in the shared paint-time text-run recording sink
    /// (`crate::testing::install_text_run_sink`/`take_text_run_sink`) and
    /// drain it into [`Self::text_runs`] — mirrors
    /// `GtkBackend::painted_text_recording` / `MacBackend::painted_text_recording`.
    /// Off by default: a live app never reads `text_runs`, and recording
    /// every run would allocate a `String` per painted text run per
    /// frame. [`super::testing::WinDriver`] turns it on.
    /// Not `target_os`-gated: the flag itself is a plain `bool` with no
    /// WinAPI dependency, only the paint calls that would ever populate
    /// anything are.
    painted_text_recording: bool,
    /// Text runs recorded during the last frame, when
    /// `painted_text_recording` is on — the `WinDriver::find`/`find_bounds`
    /// /`inventory`/`screen_has` backing store. Populated
    /// by draining the shared sink at the end of [`Self::end_frame`];
    /// mirrors `MacBackend::text_runs`'s lifecycle. Not `target_os`-gated
    /// for the same reason as `painted_text_recording` above.
    text_runs: Vec<crate::testing::TextRun>,
    /// Mirrors `TuiBackend::nerd_fonts_enabled` / `GtkBackend::nerd_fonts_enabled`
    /// / `MacBackend::nerd_fonts_enabled`. Picks `Icon::glyph` vs `Icon::fallback`
    /// in `TreeView`'s paint. Set via [`Backend::set_nerd_fonts`]; defaults to `true`
    /// on this backend. Not `target_os`-gated: the flag itself has no WinAPI
    /// dependency, same rationale as `current_pointer_shape` — only the
    /// DirectWrite paint call that reads it needs a real host.
    nerd_fonts_enabled: bool,
    /// The most recent [`BackendError`] recorded by `end_frame`, drained and
    /// cleared by [`Backend::last_error`]. Not `target_os`-gated: the field
    /// itself has no WinAPI dependency, same rationale as `current_pointer_shape`
    /// — only the `EndDraw` call that would populate it needs a real host.
    last_error: Option<BackendError>,
    /// Single owner of keyboard focus — see [`crate::focus`]'s module
    /// doc. Mutated by [`Self::set_focus`] (whole-widget focus changes)
    /// and by the shared Tab/Shift+Tab intercept in
    /// [`crate::runtime::preprocess_event`] via
    /// [`crate::runtime::PreprocessBackend::focus_tree_mut`]; read
    /// elsewhere via [`Backend::focus_tree`].
    focus_tree: crate::focus::FocusTree,
    /// Thread-safe inbox for [`crate::UiEvent::User`] payloads — see
    /// [`crate::runtime::UserEventQueue`]'s doc. Not `target_os`-gated: the
    /// queue has no WinAPI dependency, same rationale as `events` above; only
    /// [`Self::waker`]'s `PostMessageW` nudge needs a real host.
    user_events: std::sync::Arc<crate::runtime::UserEventQueue>,
    /// Zones registered this frame via [`Backend::register_zone`] —
    /// `WinBackend`'s twin of `GtkBackend::zones`/`MacBackend::zones`.
    /// Cleared at the start of every frame by [`Self::begin_frame`], same
    /// lifecycle as `GtkBackend::zones`. Not `target_os`-gated: the zone list
    /// is a plain `Vec` with no WinAPI dependency. `pub(crate)` so
    /// [`super::testing::WinDriver::inventory`] can read it back.
    pub(crate) zones: Vec<crate::testing::ZoneRec>,
    /// Bounds of every tooltip/toast painted so far this frame, via
    /// [`Backend::draw_tooltip_with_chrome`]/[`Backend::draw_toast_overlay`].
    /// Cleared at the start of each frame by [`Self::begin_frame`], same
    /// per-frame lifecycle as `zones`. Mirrors
    /// `MacBackend::overlay_bounds_this_frame`; GTK has no equivalent yet
    /// (`gtk::backend::draw_focus_ring` strokes unconditionally).
    ///
    /// [`Backend::draw_focus_ring`] reads this to decide whether to skip
    /// painting the generic focus-ring convention entirely for a given
    /// call, so it never strokes on top of an overlay drawn earlier in the
    /// same frame — tooltip/toast must stay topmost (see that method's own
    /// doc for the regression this guards).
    overlay_bounds_this_frame: Vec<Rect>,
    /// Test-facing [`Backend::request_frame_in`] call-count/deadline
    /// bookkeeping, mirroring
    /// [`crate::tui::TuiBackend`]'s own `frame_scheduler` field — see
    /// [`crate::runtime::FrameScheduler`]'s doc for why Windows, which
    /// already has a real native timer (`SetTimer`/`WM_TIMER`, armed
    /// directly from [`Self::request_frame_in`]), still needs this: that
    /// real timer is a no-op before [`Self::attach_surface`] has ever
    /// run (no `HWND` yet), which is exactly the state a headless
    /// [`super::testing::WinDriver`] test runs in. `request_frame_in`
    /// records into this unconditionally, `hwnd` or not; read back via
    /// [`Self::frame_requests`]/[`Self::pending_frame_delay`]. Not
    /// `target_os`-gated: a plain value with no WinAPI dependency, same
    /// rationale as `zones`/`user_events` above.
    frame_scheduler: crate::runtime::FrameScheduler,
    /// Backend-neutral caret-blink phase for `MultiSectionView` aux-row
    /// text inputs (`SectionAux::Input`/`Search`) — see
    /// [`crate::caret_blink`]'s module doc for the state machine and the
    /// Windows-specific wiring it prescribes. `super::run`'s
    /// `CARET_BLINK_TIMER_ID` timer, armed once at startup, ticks this
    /// every [`crate::caret_blink::CaretBlinkState::period`] via
    /// `WM_TIMER` and invalidates the window when the phase flips;
    /// `super::run::dispatch_event` calls [`Self::note_caret_activity`]
    /// on `KeyPressed`/`CharTyped` so the caret holds solid while typing,
    /// mirroring `MacBackend`'s `caret_visible`/`paused_until` cells and
    /// `GtkBackend::caret_blink`. Not `target_os`-gated: a plain value
    /// with no WinAPI dependency, same rationale as `zones`/`user_events`
    /// above.
    caret_blink: crate::caret_blink::CaretBlinkState,
    /// Timestamp of the most recent mouse-wheel scroll, fed in by
    /// `super::run`'s `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` handler via
    /// [`Self::note_scroll_activity`] — the Win-GUI twin of
    /// `MacBackend::scroll_fade_last_interaction`. [`Self::new`] seeds
    /// this with its own construction instant rather than `None`, so a
    /// scrollbar painted immediately after startup (a live app's first
    /// frame, a headless pixel test, a gallery capture) still shows at
    /// full opacity like VS Code's overlay scrollbars briefly do on
    /// load, instead of starting invisible. With no further scroll
    /// activity it fades out exactly like any other idle period. `None`
    /// is a reachable test seam ([`Self::set_scroll_last_interaction`])
    /// for "never interacted, already fully faded". Not `target_os`-gated:
    /// the field itself has no WinAPI dependency, same rationale as
    /// `current_pointer_shape` above — only the real `WM_MOUSEWHEEL`
    /// handler that would populate it needs a real host.
    scroll_fade_last_interaction: Option<std::time::Instant>,
    /// Test seam overriding "now" for
    /// [`idle_fade_opacity`](crate::primitives::scrollbar::idle_fade_opacity)
    /// instead of [`std::time::Instant::now`], so pixel tests can assert
    /// the fade curve deterministically without a real sleep — the
    /// Win-GUI twin of `MacBackend::scroll_fade_clock_override`. `None`
    /// (the default) uses real wall-clock time, as a live app does.
    scroll_fade_clock_override: Option<std::time::Instant>,
    /// Opt-in retention of the frame this backend last painted, handed
    /// out by [`Backend::frame_capture`] and read by
    /// [`Backend::last_frame`]. Disabled and empty by default, so a live
    /// app that never opts in never pays for the clone
    /// [`crate::frame::ScreenLayout::draw`] would otherwise build. Mirrors
    /// `TuiBackend::frame_capture`/`GtkBackend::frame_capture`/
    /// `MacBackend::frame_capture` exactly — see `crate::frame`'s module
    /// doc, "Live-frame capture for accessibility". Not `target_os`-gated:
    /// a plain value with no WinAPI dependency, same rationale as
    /// `zones`/`user_events` above.
    frame_capture: crate::frame::FrameCapture,
}

impl WinBackend {
    pub fn new() -> Self {
        // Self-register the bundled codicon font and seed a
        // codicon-only `nerd_font_fallback` before anything paints —
        // mirrors `crate::gtk::app_font::ensure_codicon_registered`'s
        // "degrade, don't fail" posture: a registration failure just
        // leaves both locals `None`, so the worst case is a missing
        // codicon falling through to tofu rather than this constructor
        // failing outright. Real `IDWriteFactory`/
        // `IDWriteInMemoryFontFileLoader` calls, so this can only run
        // for real on `target_os = "windows"`; elsewhere (`cargo check
        // --features win` on Linux) this whole block is compiled out,
        // matching every other real WinAPI call in this constructor.
        #[cfg(target_os = "windows")]
        let (codicon_font_collection, nerd_font_fallback) =
            match crate::win::text::register_font_from_memory(crate::codicon::FONT_BYTES) {
                Ok((collection, _names)) => {
                    let fallback = crate::win::text::build_nerd_font_fallback_multi(&[(
                        crate::codicon::FONT_FAMILY,
                        Some(&collection),
                    )])
                    .ok();
                    (Some(collection), fallback)
                }
                Err(_) => (None, None),
            };

        Self {
            viewport: Viewport::new(0.0, 0.0, 1.0),
            modal_stack: Rc::new(RefCell::new(ModalStack::new())),
            drag_state: Rc::new(RefCell::new(DragState::new())),
            double_click: DoubleClickDetector::with_radius(WIN_DOUBLE_CLICK_RADIUS),
            core: crate::backend_core::BackendCore::default(),
            focused_activity_bar: None,
            events: Rc::new(RefCell::new(VecDeque::new())),
            services: WinPlatformServices::new(),
            current_line_height: 16.0,
            current_char_width: 8.0,
            #[cfg(target_os = "windows")]
            dpi_scale: 1.0,
            #[cfg(target_os = "windows")]
            surface: None,
            #[cfg(target_os = "windows")]
            headless_target: None,
            #[cfg(target_os = "windows")]
            hwnd: None,
            #[cfg(target_os = "windows")]
            tray: super::tray::WinTrayState::default(),
            #[cfg(target_os = "windows")]
            fullscreen_saved: None,
            #[cfg(target_os = "windows")]
            hwnd_raw: std::sync::Arc::new(std::sync::atomic::AtomicIsize::new(0)),
            #[cfg(target_os = "windows")]
            dwrite: None,
            #[cfg(target_os = "windows")]
            editor_font_family: DEFAULT_EDITOR_FONT_FAMILY.to_string(),
            #[cfg(target_os = "windows")]
            editor_font_size_pt: 11.0,
            #[cfg(target_os = "windows")]
            chrome_dwrite: None,
            #[cfg(target_os = "windows")]
            ui_font_family: DEFAULT_UI_FONT_FAMILY.to_string(),
            #[cfg(target_os = "windows")]
            ui_font_size_pt: DEFAULT_UI_FONT_SIZE_PT,
            #[cfg(target_os = "windows")]
            nerd_font_fallback,
            #[cfg(target_os = "windows")]
            registered_font_collection: None,
            #[cfg(target_os = "windows")]
            codicon_font_collection,
            #[cfg(target_os = "windows")]
            registered_font_bytes: Vec::new(),
            current_theme: crate::theme::Theme::default(),
            current_style: crate::Style::default(),
            current_pointer_shape: PointerShape::Default,
            painted_text_recording: false,
            text_runs: Vec::new(),
            nerd_fonts_enabled: true,
            last_error: None,
            focus_tree: crate::focus::FocusTree::new(),
            user_events: crate::runtime::UserEventQueue::new(),
            zones: Vec::new(),
            overlay_bounds_this_frame: Vec::new(),
            frame_scheduler: crate::runtime::FrameScheduler::new(),
            caret_blink: crate::caret_blink::CaretBlinkState::default(),
            scroll_fade_last_interaction: Some(std::time::Instant::now()),
            scroll_fade_clock_override: None,
            frame_capture: crate::frame::FrameCapture::default(),
        }
    }

    /// Drain pending [`crate::UiEvent::User`] payloads — the same
    /// [`Self::user_events`] queue [`Backend::poll_events`] folds in,
    /// exposed standalone so `win::run`'s `WM_QUADRAUI_USER_EVENT` handler
    /// can drain-and-dispatch directly without going through
    /// the otherwise-vestigial `poll_events`/`wait_events` pair (see that
    /// pair's doc for why the live `wndproc` doesn't normally call them).
    ///
    /// Only called from `wndproc`, which is entirely
    /// `#[cfg(target_os = "windows")]`-gated — see [`WM_QUADRAUI_USER_EVENT`]'s
    /// doc for why that reads as dead code on a non-Windows `cargo check`
    /// and isn't.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn drain_user_events(&mut self) -> Vec<UiEvent> {
        let mut out = Vec::new();
        self.user_events.drain_into(&mut out);
        out
    }

    /// Drain pending [`UiEvent::NotificationActivated`] events queued by
    /// [`WinPlatformServices::send_notification`]'s WinRT toast
    /// `Activated` handler — mirrors
    /// [`Self::drain_user_events`]'s shape so `win::run`'s
    /// `WM_QUADRAUI_USER_EVENT` handler can drain-and-dispatch this queue
    /// the same way.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn drain_notification_events(&self) -> Vec<UiEvent> {
        self.services.drain_notification_events()
    }

    /// Store this process's AppUserModelID, computed by
    /// `win32::create_window` from `RunConfig::title`, so
    /// [`WinPlatformServices::send_notification`] can pass it to
    /// `ToastNotificationManager::CreateToastNotifierWithId` — mirrors
    /// [`Self::attach_surface`] calling `self.services.set_window`.
    /// `display_name` is the human-readable name registered alongside
    /// `aumid` and shown as the toast header, Action Center group label,
    /// and Settings → Notifications entry.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn set_aumid(&self, aumid: String, display_name: String) {
        self.services.set_aumid(aumid, display_name);
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

    /// Update the cached theme. Ergonomic concrete-type sibling of
    /// [`Backend::set_theme`] — mirrors `GtkBackend::set_current_theme`
    pub fn set_current_theme(&mut self, theme: crate::theme::Theme) {
        self.current_theme = theme;
    }

    /// Read-only accessor for the cached theme. Mirrors
    /// `GtkBackend::current_theme`.
    pub fn current_theme(&self) -> &crate::theme::Theme {
        &self.current_theme
    }

    /// Update the cached style. Mirrors [`Self::set_current_theme`] —
    /// see [`crate::Style`]'s module doc.
    pub fn set_current_style(&mut self, style: crate::Style) {
        self.current_style = style;
    }

    /// Read-only accessor for the cached style. Mirrors
    /// [`Self::current_theme`].
    pub fn current_style(&self) -> &crate::Style {
        &self.current_style
    }

    /// Record that a mouse-wheel scroll just arrived, resetting the
    /// overlay scrollbar's idle-fade clock to "now". `super::run`'s
    /// `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` handler calls this for every
    /// message it receives before dispatching the translated
    /// [`UiEvent::Scroll`] — mirrors `MacBackend::note_scroll_activity`.
    ///
    /// [`UiEvent::Scroll`]: crate::UiEvent::Scroll
    pub fn note_scroll_activity(&mut self) {
        self.scroll_fade_last_interaction = Some(self.fade_now());
    }

    /// Test seam: pin the overlay scrollbar's last-interaction instant
    /// directly instead of going through a synthetic
    /// [`Self::note_scroll_activity`] call. `None` resets to "never
    /// scrolled" (fully faded). Mirrors `MacBackend::set_scroll_last_interaction`.
    pub fn set_scroll_last_interaction(&mut self, at: Option<std::time::Instant>) {
        self.scroll_fade_last_interaction = at;
    }

    /// The overlay scrollbar's current last-interaction instant —
    /// [`Self::new`]'s own construction time until a real scroll (or a
    /// test) resets it. `None` once explicitly cleared via
    /// [`Self::set_scroll_last_interaction`].
    pub fn scroll_last_interaction(&self) -> Option<std::time::Instant> {
        self.scroll_fade_last_interaction
    }

    /// Test seam: override what [`Self::fade_now`] returns, so a pixel
    /// test can assert the fade curve at an exact offset from the last
    /// interaction without a real sleep. `None` (the default) falls
    /// back to [`std::time::Instant::now`]. Mirrors
    /// `MacBackend::set_fade_clock_override`.
    pub fn set_fade_clock_override(&mut self, now: Option<std::time::Instant>) {
        self.scroll_fade_clock_override = now;
    }

    /// "Now", for overlay-scrollbar fade purposes — real wall-clock
    /// time unless a test pinned [`Self::set_fade_clock_override`].
    fn fade_now(&self) -> std::time::Instant {
        self.scroll_fade_clock_override
            .unwrap_or_else(std::time::Instant::now)
    }

    /// Opacity multiplier [`Backend::draw_scrollbar`] paints the track
    /// and thumb at: `0.0` if [`Self::scroll_last_interaction`] has been
    /// explicitly cleared to `None`, otherwise
    /// [`idle_fade_opacity`](crate::primitives::scrollbar::idle_fade_opacity)
    /// sampled at [`Self::fade_now`]. Mirrors `MacBackend::scrollbar_fade_opacity`.
    ///
    /// Only read from [`Backend::draw_scrollbar`]'s `target_os =
    /// "windows"` painted path (`self.surface` itself is `cfg`-gated to
    /// that target) — see [`Self::drain_user_events`]'s doc for why that
    /// reads as dead code on a non-Windows `cargo check` and isn't.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    fn scrollbar_fade_opacity(&self) -> f32 {
        match self.scroll_fade_last_interaction {
            Some(last) => crate::primitives::scrollbar::idle_fade_opacity(last, self.fade_now()),
            None => 0.0,
        }
    }

    /// Resolve the row pitch `draw_list` lays its rows out at: `Style::row_height` on a pixel backend
    /// (`candidate_line_height > 1.0`), floored at the font's own line
    /// height so a chrome font taller than the token isn't clipped — see
    /// `MacBackend::resolved_row_height`'s identical rationale.
    fn resolved_row_height(&self, candidate_line_height: f32) -> f32 {
        if candidate_line_height > 1.0 {
            candidate_line_height.max(self.current_style.row_height)
        } else {
            candidate_line_height
        }
    }

    /// Live DirectWrite handles for the current chrome (UI) font, once
    /// [`Self::attach_surface`]/[`Self::attach_headless`] has built them —
    /// `None` beforehand, same lifecycle as the editor `dwrite` field.
    /// See `chrome_dwrite`'s field doc for the full list of `draw_*`
    /// rasterisers that consume this.
    ///
    /// All 13 `ChromePrimitive`s (`crate::font_role`) paint through this
    /// handle, matching GTK's `ui_font` swap and macOS's `chrome_font`.
    #[cfg(target_os = "windows")]
    pub fn chrome_dwrite(&self) -> Option<&DWrite> {
        self.chrome_dwrite.as_ref()
    }

    /// Create a single-threaded `ID2D1Factory` and an `ID2D1HwndRenderTarget`
    /// sized to `hwnd`'s current client rect, and store both on `self`.
    /// Also seeds [`Backend::viewport`][crate::backend::Backend::viewport]
    /// with the window's real pixel size and DPI scale.
    #[cfg(target_os = "windows")]
    pub(crate) fn attach_surface(&mut self, hwnd: HWND) -> WinResult<()> {
        let factory: ID2D1Factory =
            // SAFETY: `D2D1CreateFactory` takes a plain enum and an
            // `Option<*const D2D1_FACTORY_OPTIONS>`; `None` is the
            // documented "use defaults" sentinel, not a dereferenced
            // pointer.
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };

        let mut rect = RECT::default();
        // SAFETY: `hwnd` is the caller's live window (this is called once,
        // right after `CreateWindowExW` succeeds — see this function's
        // doc); `rect` is stack-local and outlives the call.
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
        // SAFETY: `render_props`/`hwnd_props` are stack-local structs read
        // synchronously by this call; `hwnd_props.hwnd` is the same live
        // `hwnd` this function was called with.
        let target = unsafe { factory.CreateHwndRenderTarget(&render_props, &hwnd_props)? };

        self.dpi_scale = dpi_scale_for_window(hwnd);
        self.viewport = Viewport::new(width as f32, height as f32, self.dpi_scale);
        self.surface = Some(Surface {
            factory: Some(factory),
            target: RenderTarget::Hwnd(target),
            brushes: new_brush_cache(),
            images: Default::default(),
        });
        self.hwnd = Some(hwnd);
        // Keep the waker's live mirror in sync — see `Self::hwnd_raw`'s doc
        // for why this can't just be read off `self.hwnd` from inside the closure.
        self.hwnd_raw
            .store(hwnd.0 as isize, std::sync::atomic::Ordering::Release);
        // A waker called before this point (from `AppLogic::setup`, or a
        // background thread racing window creation) queued its payload but
        // had no window to nudge, so nothing would ever drain it. Post one
        // wake now that the window exists; `wndproc`'s handler drains
        // whatever is queued, and an empty queue makes it a no-op.
        //
        // SAFETY: `hwnd` is the live window this function was called with;
        // `PostMessageW` only enqueues a message for it.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_QUADRAUI_USER_EVENT, WPARAM(0), LPARAM(0));
        }
        // Give `WinPlatformServices` the live window so file dialogs
        // open parented to it and notifications have an owning `HWND` —
        // mirrors `GtkBackend::set_window` calling
        // `GtkPlatformServices::set_window` right after its own window is
        // constructed.
        self.services.set_window(hwnd);

        // DirectWrite bootstrap: build the factory + text format for
        // the currently-configured editor font and seed `line_height()` /
        // `char_width()` from its real font metrics, same role as
        // `gtk::run::render_frame`'s per-frame Pango metrics resolve —
        // except here it only needs to happen once per surface, since
        // DirectWrite text formats aren't tied to the Direct2D device the
        // way the render target is.
        let (dwrite, line_height, char_width) = DWrite::new(
            &self.editor_font_family,
            self.editor_font_size_pt,
            self.nerd_font_fallback.as_ref(),
        )?;
        self.dwrite = Some(dwrite);
        self.current_line_height = line_height;
        self.current_char_width = char_width;

        // Chrome (UI) font bootstrap — same DirectWrite dance as
        // the editor font above, but for `ui_font_family`/`ui_font_size_pt`.
        // `line_height`/`char_width` from this format aren't needed:
        // those are always resolved from the editor font, so they're
        // discarded here rather than overwriting `self.current_line_height`.
        let (chrome_dwrite, _, _) = DWrite::new(
            &self.ui_font_family,
            self.ui_font_size_pt,
            self.nerd_font_fallback.as_ref(),
        )?;
        self.chrome_dwrite = Some(chrome_dwrite);

        Ok(())
    }

    /// Attach an offscreen `ID2D1DCRenderTarget` instead of a live
    /// window's — the headless twin of [`Self::attach_surface`],
    /// used only by [`super::testing::driver_with_shell`] /
    /// [`super::testing::WinDriver`] so a `WinBackend`'s already-landed
    /// rasterisers can paint somewhere a test can read pixels
    /// back from, without a live `HWND`.
    ///
    /// `target` is expected to be [`super::testing::HeadlessSurface::target`]
    /// cloned (a cheap `AddRef` — Direct2D COM interfaces are reference
    /// counted, and `HeadlessSurface` keeps the authoritative reference
    /// plus the backing DIB section alive for as long as it lives). This
    /// method deliberately skips everything HWND-specific that
    /// [`Self::attach_surface`] does: `GetClientRect`,
    /// `dpi_scale_for_window`, and `WinPlatformServices::set_window`.
    /// `self.hwnd` stays `None`, so [`Self::ensure_surface`]'s
    /// device-lost recovery (which only knows how to reattach to a
    /// `hwnd`) correctly stays a no-op for a headless surface — a
    /// `HeadlessSurface` doesn't hit device loss the way a live
    /// compositor can.
    #[cfg(target_os = "windows")]
    pub(crate) fn attach_headless(
        &mut self,
        target: ID2D1DCRenderTarget,
        width: u32,
        height: u32,
    ) -> WinResult<()> {
        self.dpi_scale = 1.0;
        self.viewport = Viewport::new(width as f32, height as f32, self.dpi_scale);
        // Keep a second clone around (cheap COM `AddRef`) so
        // `ensure_surface` can rebuild `surface` after `end_frame`'s
        // device-lost recovery drops it, even though headless mode has
        // no `hwnd` to reattach through — see the
        // `headless_target` field doc.
        self.headless_target = Some(target.clone());
        self.surface = Some(Surface {
            factory: None,
            target: RenderTarget::Dc(target),
            brushes: new_brush_cache(),
            images: Default::default(),
        });

        // Same DirectWrite bootstrap as `attach_surface` — text formats
        // aren't tied to which kind of render target they paint onto.
        let (dwrite, line_height, char_width) = DWrite::new(
            &self.editor_font_family,
            self.editor_font_size_pt,
            self.nerd_font_fallback.as_ref(),
        )?;
        self.dwrite = Some(dwrite);
        self.current_line_height = line_height;
        self.current_char_width = char_width;

        // Chrome (UI) font bootstrap — see `attach_surface`'s comment.
        let (chrome_dwrite, _, _) = DWrite::new(
            &self.ui_font_family,
            self.ui_font_size_pt,
            self.nerd_font_fallback.as_ref(),
        )?;
        self.chrome_dwrite = Some(chrome_dwrite);

        Ok(())
    }

    /// `(width, height)` DIPs of `text` measured against the current
    /// editor font. Returns `(0.0, 0.0)` if no surface has attached yet.
    #[cfg(target_os = "windows")]
    pub fn measure_text(&self, text: &str) -> (f32, f32) {
        self.dwrite
            .as_ref()
            .and_then(|d| d.measure_text(text).ok())
            .unwrap_or((0.0, 0.0))
    }

    /// Paint `text` inside `rect` (DIPs) in `color` onto the live render
    /// target. No-op if no surface/DirectWrite handle is attached yet.
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
    /// no-op returning `Ok(())` if neither a window nor a headless target
    /// has ever attached one yet (nothing to recover to — covers the
    /// synchronous `WM_SIZE` Windows fires from inside `CreateWindowExW`,
    /// before `run_inner` has a `HWND` to attach at all).
    ///
    /// Headless surfaces ([`Self::attach_headless`], `WinDriver`) recover
    /// through `headless_target` instead of `hwnd`: before
    /// this, a headless surface dropped by `end_frame`'s device-lost path
    /// stayed `None` forever, since `hwnd` is never set in headless mode
    /// — the exact gap that made this method's recovery unreachable from
    /// a test. Checked first because it's the cheaper, more specific
    /// case; a `WinBackend` only ever has one of the two attached at a
    /// time.
    #[cfg(target_os = "windows")]
    pub(crate) fn ensure_surface(&mut self) -> WinResult<()> {
        if self.surface.is_some() {
            return Ok(());
        }
        if let Some(target) = self.headless_target.clone() {
            self.surface = Some(Surface {
                factory: None,
                target: RenderTarget::Dc(target),
                brushes: new_brush_cache(),
                images: Default::default(),
            });
            return Ok(());
        }
        match self.hwnd {
            Some(hwnd) => self.attach_surface(hwnd),
            None => Ok(()),
        }
    }

    /// Test-only hook: force `self.surface` back to the
    /// "not attached" state a genuine `EndDraw` failure (device lost /
    /// `D2DERR_RECREATE_TARGET`) leaves it in — see [`Self::end_frame`]'s
    /// doc — without needing a real Direct2D error to get there.
    ///
    /// `self.surface`/`self.hwnd`/`self.headless_target` are all private
    /// to this module, so nothing outside it — including
    /// `super::testing::WinDriver`, the one place a downstream crate can
    /// reach a `WinBackend` at all — can reproduce this state directly.
    /// [`super::testing::WinDriver::drop_surface`] is this method's only
    /// caller, and exists to close exactly that gap: before it, the only
    /// way any test (in-crate or down) could observe a `draw_*`'s
    /// surface-less fallback was to never attach a surface in the
    /// first place (`minimap_with_no_surface_resolves_the_same_layout_as_its_query`,
    /// below) — proving the fallback on a backend that *had* a surface
    /// and then lost it (the actual, narrower reported bug) was
    /// impossible from outside this file. `self.hwnd`/`self.headless_target`
    /// are left untouched, same as the real `EndDraw`-failure path in
    /// [`Self::end_frame`], so [`Self::ensure_surface`] (called by every
    /// production `WM_PAINT`/`WM_SIZE` handler, and by
    /// [`super::testing::WinDriver::render`] before every frame) still
    /// recreates it on the next call, exactly like the real failure this
    /// simulates.
    #[cfg(target_os = "windows")]
    pub(crate) fn drop_surface_for_testing(&mut self) {
        self.surface = None;
    }

    /// Resize the live render target to `width` x `height` device pixels.
    /// Called from `win::run`'s `WM_SIZE` handler.
    ///
    /// A no-op on the render-target side if no surface is attached yet —
    /// Windows can fire an initial `WM_SIZE` synchronously from inside
    /// `CreateWindowExW`, before the caller has the `HWND` back to pass to
    /// [`Self::attach_surface`]. The viewport is still updated so
    /// `Backend::viewport()` reflects the window's real size as soon as
    /// it's known.
    ///
    /// Also a no-op on the render-target side for a [`RenderTarget::Dc`]
    /// surface: `ID2D1DCRenderTarget` is bound once to a fixed-size DIB
    /// section and never resized.
    #[cfg(target_os = "windows")]
    pub(crate) fn resize_surface(&mut self, width: u32, height: u32) -> WinResult<()> {
        let width = width.max(1);
        let height = height.max(1);
        if let Some(surface) = &self.surface {
            if let RenderTarget::Hwnd(target) = &surface.target {
                let size = D2D_SIZE_U { width, height };
                // SAFETY: `target` is a live `ID2D1HwndRenderTarget` COM
                // object (this crate's `windows-rs` bindings hold a strong
                // reference for as long as `self.surface` does); `Resize`
                // takes `size` by const reference and has no other
                // precondition.
                unsafe { target.Resize(&size)? };
            }
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

    // ── ActivityBar keyboard focus ────────────────────────────────────

    /// `WidgetId` of the [`ActivityBar`] that declared
    /// `is_keyboard_focused = true` in the most recently painted frame, or
    /// `None` if no bar is focused. Read by `win::run::dispatch_event` — see
    /// `focused_activity_bar`'s field doc for the full contract. Not
    /// `target_os`-gated: the field itself is a plain `Option<WidgetId>`
    /// with no Direct2D dependency, so this reads correctly even before
    /// any surface has attached.
    pub(crate) fn focused_activity_bar_id(&self) -> Option<&WidgetId> {
        self.focused_activity_bar.as_ref()
    }

    // ── Double-click folding ───────────────────────────────────────────

    /// Fold a `MouseDown` into `DoubleClick` if it lands within the
    /// detector's time/position window of the previous click. Every other
    /// variant passes through unchanged. `win::run::dispatch_event` calls
    /// this on each translated Win32 mouse-button message before handing
    /// it to `AppLogic` — mirrors `MacBackend::fold_double_click`. Not
    /// `target_os`-gated: `DoubleClickDetector::process` has no WinAPI
    /// dependency, so this compiles and behaves identically on every host.
    pub(crate) fn fold_double_click(&mut self, ev: UiEvent) -> UiEvent {
        let mut events = [ev];
        self.double_click.process(&mut events);
        let [ev] = events;
        ev
    }

    // ── Accelerator matching ──────────────────────────────────────────
    //
    // The registry itself lives in [`crate::backend_core::BackendCore`]
    // — both methods below are thin delegations.

    /// Look up a registered `Global`-scope accelerator for a
    /// `(key, modifiers)` pair. Not `target_os`-gated for the same reason
    /// as `focused_activity_bar_id` above.
    pub(crate) fn match_keypress(&self, key: &Key, modifiers: Modifiers) -> Option<AcceleratorId> {
        self.core.match_keypress(key, modifiers)
    }

    /// Rewrite matching `KeyPressed` events to `UiEvent::Accelerator` in
    /// place — the `docs/BACKEND.md` "Event poll / wait" step 3, run by
    /// [`Backend::poll_events`]/[`Backend::wait_events`] over
    /// whatever this call drained from [`Self::events`].
    /// `win::run::dispatch_event`'s synchronous per-message path calls
    /// [`Self::match_keypress`] directly instead of going through this,
    /// since it already has one event at a time rather than a batch.
    pub(crate) fn apply_accelerators(&self, events: &mut [UiEvent]) {
        self.core.apply_accelerators(events);
    }

    // ── Event queue adapter ─────────────────────────────────────────────

    /// Shared handle to the backend's event queue, for a future producer
    /// (a live `wndproc` wiring, or any other event source) to clone and
    /// push into without needing a `&mut WinBackend` borrow. Mirrors
    /// `GtkBackend::events_handle`/`MacBackend::events_handle`. Not
    /// `target_os`-gated: `Rc<RefCell<VecDeque<UiEvent>>>` has no WinAPI
    /// dependency.
    pub fn events_handle(&self) -> Rc<RefCell<VecDeque<UiEvent>>> {
        self.events.clone()
    }

    /// Push one event onto the adapter queue, drained by
    /// [`Backend::poll_events`]/[`Backend::wait_events`]. The producer side
    /// of the queue pattern — mirrors `MacBackend::push_event`.
    pub fn push_event(&self, ev: UiEvent) {
        self.events.borrow_mut().push_back(ev);
    }

    /// Re-apply `current_pointer_shape` via `SetCursor`/`LoadCursorW`.
    /// Called both by [`Backend::set_cursor`] (an app-driven
    /// change) and by `win::run`'s `WM_SETCURSOR` handler (Windows
    /// re-asking "what cursor belongs here?" on every mouse move over the
    /// client area) — see `current_pointer_shape`'s field doc for why
    /// both call sites need to exist.
    #[cfg(target_os = "windows")]
    pub(crate) fn apply_current_cursor(&self) {
        // SAFETY: `LoadCursorW(None, ...)` loads a built-in system cursor
        // by predefined id — see `run.rs::run_inner`'s identical call for
        // the same reasoning; `SetCursor` takes the resulting `HCURSOR`
        // (or `None` on load failure, itself a documented valid input
        // meaning "no cursor") by value.
        unsafe {
            let _ = SetCursor(
                LoadCursorW(
                    None,
                    pointer_shape_to_win32_cursor(self.current_pointer_shape),
                )
                .ok(),
            );
        }
    }

    // ── Custom-caption hit testing ───────────────────────────────────────

    /// Reserved [`WidgetId`] a host registers (via [`Backend::register_zone`])
    /// for the drawn title-bar band it wants Windows to treat as a native
    /// caption — the drag-to-move / double-click-to-maximize row. Matches
    /// [`crate::compose::app_shell::ShellView`]'s own `"app-shell:title-bar"`
    /// zone id, so a host built on `ShellView` gets [`Self::nc_hit_test`]
    /// for free; a bespoke host (one that doesn't use `ShellView`) opts in
    /// with one `register_zone` call of its own each frame.
    pub const TITLE_BAR_DRAG_ZONE: &'static str = "app-shell:title-bar";

    /// Classify a client-area point (DIPs, same units [`Backend::viewport`]
    /// reports) for `win::run`'s `WM_NCHITTEST` handler:
    ///
    /// - `None` — the point isn't inside [`Self::TITLE_BAR_DRAG_ZONE`] at
    ///   all (no zone registered this frame, or the point falls outside
    ///   it). The caller should leave `DefWindowProcW`'s own answer alone
    ///   — resize border, below-the-band content, native caption buttons,
    ///   etc.
    /// - `Some(true)` — the point is in the band and not covered by any
    ///   more specific zone, so it should report `HTCAPTION`: dragging it
    ///   moves the window and double-clicking it maximizes/restores,
    ///   exactly like a native titlebar (`DefWindowProcW`'s own
    ///   non-client move loop handles both once this returns `HTCAPTION`
    ///   — see the `Backend::toggle_window_maximize` override's doc for
    ///   why no `begin_window_drag`/`begin_window_resize` call is needed
    ///   here).
    /// - `Some(false)` — the point is in the band but also inside another
    ///   registered zone strictly smaller than the band (a button, or any
    ///   other interactive widget a host paints into the row), or inside
    ///   an addressable sub-part of a widget-contract widget painted this
    ///   frame (a command center's arrows and search box, recorded by
    ///   [`crate::DrawsPrimitives::draw_dyn`] into
    ///   [`crate::primitive::PaintedWidgets`]) — report `HTCLIENT`
    ///   instead, so the click reaches that widget as an ordinary
    ///   `UiEvent::MouseDown` rather than being swallowed by the OS
    ///   move/maximize gesture.
    ///
    /// Zones whose bounds are *not* strictly smaller than the band (e.g.
    /// `ShellView`'s own `"app-shell:window"` whole-window zone, which
    /// necessarily contains every point in the band too) are never
    /// treated as an exclusion — otherwise every point in the band would
    /// read as "covered by another zone" and this would always answer
    /// `Some(false)`, exactly the double-titlebar bug this issue exists
    /// to fix.
    pub fn nc_hit_test(&self, x: f32, y: f32) -> Option<bool> {
        let point = Point::new(x, y);
        let band = self
            .zones
            .iter()
            .find(|z| z.id.as_str() == Self::TITLE_BAR_DRAG_ZONE)?;
        if !band.bounds.contains(point) {
            return None;
        }
        let band_area = band.bounds.width * band.bounds.height;
        let excluded = self.zones.iter().any(|z| {
            z.id.as_str() != Self::TITLE_BAR_DRAG_ZONE
                && z.bounds.contains(point)
                && z.bounds.width * z.bounds.height < band_area
        }) || crate::primitive::with_painted_widgets(|painted| {
            painted
                .iter()
                .any(|w| w.sub_regions.iter().any(|(r, _)| r.contains(point)))
        });
        Some(!excluded)
    }

    // ── Band-content zone registration ──────────────────────────────────
    //
    // These helpers register zones at the one place each widget's real
    // screen bounds are known: right after its `draw_*` call. Called
    // unconditionally — a zone outside the band is simply never found by
    // `nc_hit_test`'s `band.bounds.contains(point)` check, so this is harmless
    // for the common case and mirrors how `ShellView::render` already registers
    // every activity-bar item's zone regardless of position.

    // ── Paint-time text-run recording ───────────────────────────────────

    /// Enable/disable the paint-time text-run recording that backs
    /// [`super::testing::WinDriver::find`]/`find_bounds`/`inventory`/
    /// `screen_has`. Off by default. Not `target_os`-gated: the
    /// `painted_text_recording` field it writes has no WinAPI dependency,
    /// and its caller [`super::testing::WinDriver::new`] exists on every
    /// host too, so this must compile everywhere `WinDriver` does.
    pub(crate) fn set_painted_text_recording(&mut self, enabled: bool) {
        self.painted_text_recording = enabled;
    }

    /// Text runs recorded during the last [`Self::begin_frame`]/
    /// [`Self::end_frame`] bracket, when [`Self::set_painted_text_recording`]
    /// is on. Not `target_os`-gated, for the same reason that method
    /// isn't — see its doc.
    pub(crate) fn text_runs(&self) -> &[crate::testing::TextRun] {
        &self.text_runs
    }

    // ── request_frame_in scheduling ─────────────────────────────────────

    /// How many times [`Backend::request_frame_in`] has been called on
    /// this backend since it was constructed — [`crate::tui::TuiBackend::
    /// frame_requests`]'s twin for Win-GUI.
    ///
    /// Real hosts never need this. It exists so a headless
    /// [`super::testing::WinDriver`] test can assert on an app's
    /// *scheduling* behaviour even though [`Self::request_frame_in`]'s
    /// real `SetTimer` arm is a no-op before [`Self::attach_surface`]
    /// has ever run (no live `HWND` yet — exactly `WinDriver`'s
    /// situation) — see [`Self::frame_scheduler`]'s field doc. Not
    /// `target_os`-gated, for the same reason
    /// [`Self::set_painted_text_recording`] isn't: the field it reads has
    /// no WinAPI dependency, and its caller exists on every host `win`
    /// compiles on.
    ///
    /// Monotonic: a fired deadline clears the pending wake (see
    /// [`Self::pending_frame_delay`]), not the count.
    pub fn frame_requests(&self) -> u64 {
        self.frame_scheduler.requests()
    }

    /// Time remaining until the pending [`Backend::request_frame_in`]
    /// deadline, or `None` when no frame is scheduled — the companion to
    /// [`Self::frame_requests`]: that one proves *how often* an app
    /// asked, this one proves *what interval* it asked for. Not
    /// `target_os`-gated, same reason as [`Self::frame_requests`].
    pub fn pending_frame_delay(&self) -> Option<Duration> {
        self.frame_scheduler.pending_delay()
    }

    // ── Caret blink ──────────────────────────────────────────────────────
    //
    // Thin delegation onto `self.caret_blink` — see that field's doc for
    // the full `CARET_BLINK_TIMER_ID` wiring in `super::run`.

    /// Current blink phase. Only this file's tests read it.
    #[cfg(test)]
    pub(crate) fn caret_visible(&self) -> bool {
        self.caret_blink.visible()
    }

    /// Advance the caret-blink phase to `now`. Called from
    /// `super::run`'s `CARET_BLINK_TIMER_ID` `WM_TIMER` handler; returns
    /// `true` when the phase flipped, telling the caller to invalidate
    /// the window.
    ///
    /// `#[allow(dead_code)]`: this method's only non-test caller is
    /// windows-gated, so a non-Windows `--features win` build has no
    /// caller outside this file's tests.
    #[allow(dead_code)]
    pub(crate) fn tick_caret_blink(&mut self, now: Instant) -> bool {
        self.caret_blink.tick(now)
    }

    /// The configured caret-blink period — the interval `super::run`
    /// arms `CARET_BLINK_TIMER_ID` at.
    ///
    /// `#[allow(dead_code)]`: this method's only non-test caller is
    /// windows-gated, so a non-Windows `--features win` build has no
    /// caller outside this file's tests.
    #[allow(dead_code)]
    pub(crate) fn caret_blink_period(&self) -> Duration {
        self.caret_blink.period()
    }

    /// Record typing activity at `now`, holding the caret solid for a
    /// typing pause. Called from `super::run::dispatch_event` on
    /// `KeyPressed`/`CharTyped`.
    pub(crate) fn note_caret_activity(&mut self, now: Instant) {
        self.caret_blink.on_activity(now);
    }

    /// Test-only: force the caret-blink phase to `visible` for
    /// reproducible paint snapshots, mirroring
    /// `MacBackend::set_caret_visible`/`GtkBackend::set_caret_visible`.
    #[cfg(test)]
    pub(crate) fn set_caret_visible(&mut self, visible: bool) {
        if self.caret_blink.visible() != visible {
            let far_future = Instant::now() + Duration::from_secs(3600);
            self.caret_blink.tick(far_future);
        }
    }

    // ── Text selection ───────────────────────────────────────────────────
    //
    // The region registry + active-selection state machine, plus the
    // accelerator registry alongside it, live in
    // [`crate::backend_core::BackendCore`] — every method below
    // except [`Self::apply_selection_highlight`]/[`Self::extract_selection_text`]
    // (Direct2D painting / `TextRegion::lines` extraction, this backend's
    // own — mirrors `GtkBackend`'s pixel-based twins via the shared
    // `BackendCore::selection_highlight_rects`/`extract_selection_text_pixel`
    // helpers) is a thin delegation.

    /// Mutable access to the [`crate::text_selection::TextSelectionState`]
    /// — the shape [`crate::dispatch::route_pointer`] needs to both read
    /// `text_regions` and call `track_focused_text_region` without separate
    /// accessors. Mirrors `GtkBackend::text_selection_state_mut`.
    pub(crate) fn text_selection_state_mut(
        &mut self,
    ) -> &mut crate::text_selection::TextSelectionState {
        &mut self.core.text_selection
    }

    /// Return the current active text selection, if any.
    pub(crate) fn active_text_selection(
        &self,
    ) -> Option<&crate::text_selection::ActiveTextSelection> {
        self.core.active_text_selection()
    }

    /// Update (or start) the active text selection. Called by
    /// `win::run::dispatch_event` when a [`UiEvent::TextSelectionChanged`]
    /// event arrives, and by [`Self::select_all_text_region`].
    pub(crate) fn set_active_text_selection(
        &mut self,
        region: WidgetId,
        anchor: crate::event::Point,
        focus: crate::event::Point,
    ) {
        self.core.set_active_text_selection(region, anchor, focus);
    }

    /// Clear the active text selection highlight only (does NOT end an
    /// in-progress `TextSelection` drag). Called before dispatching a new
    /// mouse-down so the old highlight disappears without interrupting the
    /// drag that is about to start. Mirrors `GtkBackend::clear_selection_display`.
    pub(crate) fn clear_selection_display(&mut self) {
        self.core.clear_selection_display();
    }

    /// Clear the active text selection and end any in-progress
    /// `TextSelection` drag. Called after Ctrl-C copies the selection or
    /// on a plain click outside any text region.
    pub(crate) fn clear_text_selection(&mut self) {
        let mut drag = self.drag_state.borrow_mut();
        self.core.clear_text_selection(&mut drag);
    }

    /// End any in-progress `TextSelection` drag without clearing the
    /// displayed selection. Backs the `Backend` trait's
    /// `cancel_text_selection_drag` override — apps hosting an embedded
    /// terminal call it to abort a speculative drag before forwarding a
    /// click to a PTY. Mirrors `GtkBackend::cancel_text_selection_drag_impl`/
    /// `TuiBackend::cancel_text_selection_drag_impl`.
    fn cancel_text_selection_drag_impl(&mut self) {
        let mut drag = self.drag_state.borrow_mut();
        self.core.cancel_text_selection_drag(&mut drag);
    }

    /// Set the active selection to cover the entire visible content of the
    /// most-recently focused `TextRegion` (the Ctrl-A target). See
    /// [`crate::text_selection::TextSelectionState::select_all_text_region`]
    /// for the resolution order and the viewport-only limitation.
    pub(crate) fn select_all_text_region(&mut self) -> bool {
        self.core.select_all_text_region()
    }

    /// Extract the selected text from the active selection's `TextRegion`
    /// using its stored `lines` (Win-GUI is pixel-based, like GTK — see
    /// `TextRegion::lines`'s doc). Empty when there is no active selection,
    /// the region isn't registered this frame, or it has no `lines`
    /// content.
    pub(crate) fn extract_selection_text(&self) -> String {
        self.core
            .extract_selection_text_pixel(self.current_line_height, self.current_char_width)
    }

    /// Paint the active text-selection highlight on top of the frame's
    /// already-painted content. Must be called after `app.render` inside
    /// the same `begin_frame`/`end_frame` bracket (`win::run::render_frame`)
    /// — mirrors `GtkBackend::apply_selection_highlight`'s Cairo twin and
    /// `TuiBackend::apply_selection_highlight`'s cell-invert twin, except
    /// Direct2D's `FillRectangle` paints directly onto `self.surface`
    /// rather than needing an external target handle. No-op when there is
    /// no active selection, the region isn't registered this frame, metrics
    /// aren't known yet, or (off Windows / before a surface is attached)
    /// there is nothing to paint into.
    ///
    /// The rect resolution is
    /// [`crate::backend_core::BackendCore::selection_highlight_rects`] —
    /// shared with `GtkBackend`/`MacBackend`'s twins; only the
    /// `super::text::fill_rect` Direct2D calls below are Win-GUI-specific
    /// real toolkit code.
    pub(crate) fn apply_selection_highlight(&self) {
        let Some(rects) = self
            .core
            .selection_highlight_rects(self.current_line_height, self.current_char_width)
        else {
            return;
        };
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            let highlight = self.current_theme.text_selection_highlight();
            for rect in rects {
                let _ = super::text::fill_rect(&surface.target, rect, highlight);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = rects;
        }
    }
}

/// `GetDpiForWindow(hwnd) / 96.0` — the ratio `Viewport::scale` carries
/// for this backend. The divide and its zero-DPI fallback live in
/// [`crate::win::msg::dpi_ratio`] so they stay unit-tested off Windows;
#[cfg(target_os = "windows")]
fn dpi_scale_for_window(hwnd: HWND) -> f32 {
    // SAFETY: `GetDpiForWindow` takes only a handle and returns a `u32` by
    // value — `hwnd` is the caller's live window, and there is no other
    // precondition.
    crate::win::msg::dpi_ratio(unsafe { GetDpiForWindow(hwnd) })
}

/// Resolve [`WinBackend::hwnd_raw`]'s current raw value back into an
/// `HWND`, or `None` if no window has attached yet (`0`). Split out of
/// [`WinBackend::waker`]'s closure so the "read the live handle" step is
/// a plain, directly unit-testable function.
#[cfg(target_os = "windows")]
fn live_hwnd(hwnd_raw: &std::sync::atomic::AtomicIsize) -> Option<HWND> {
    let raw = hwnd_raw.load(std::sync::atomic::Ordering::Acquire);
    if raw == 0 {
        None
    } else {
        Some(HWND(raw as *mut core::ffi::c_void))
    }
}

/// Map a [`PointerShape`] to the Win32 `IDC_*` cursor-resource `PCWSTR`
/// [`LoadCursorW`] expects. Win32's stock cursor set has no separate NE-only
/// vs. SW-only (or NW-only vs. SE-only) diagonal-resize glyph — `IDC_SIZENESW`
/// and `IDC_SIZENWSE` are each shared between their two opposite corners at the
/// OS resource level. Every [`crate::backend::ResizeEdge`] variant still gets a
/// direction-correct cursor; only the specific `IDC_*` resource is shared per
/// diagonal.
#[cfg(target_os = "windows")]
fn pointer_shape_to_win32_cursor(shape: PointerShape) -> windows::core::PCWSTR {
    use crate::backend::ResizeEdge;
    match shape {
        PointerShape::Default => IDC_ARROW,
        PointerShape::Resize(ResizeEdge::North) | PointerShape::Resize(ResizeEdge::South) => {
            IDC_SIZENS
        }
        PointerShape::Resize(ResizeEdge::East) | PointerShape::Resize(ResizeEdge::West) => {
            IDC_SIZEWE
        }
        PointerShape::Resize(ResizeEdge::NorthEast)
        | PointerShape::Resize(ResizeEdge::SouthWest) => IDC_SIZENESW,
        PointerShape::Resize(ResizeEdge::NorthWest)
        | PointerShape::Resize(ResizeEdge::SouthEast) => IDC_SIZENWSE,
        // Win32 has native cursors for several of these (`IDC_IBEAM`,
        // `IDC_HAND`, `IDC_CROSS`, `IDC_WAIT`, `IDC_NO`, `IDC_SIZEWE`/
        // `IDC_SIZENS` for col/row-resize) but none are wired up yet, so
        // falling back to the plain arrow is the deliberate no-regression
        // default.
        _ => IDC_ARROW,
    }
}

impl Default for WinBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::runtime::PreprocessBackend for WinBackend {
    fn active_text_selection(&self) -> Option<&crate::text_selection::ActiveTextSelection> {
        self.active_text_selection()
    }

    fn set_active_text_selection(&mut self, region: WidgetId, anchor: Point, focus: Point) {
        self.set_active_text_selection(region, anchor, focus)
    }

    fn clear_text_selection(&mut self) {
        self.clear_text_selection()
    }

    fn clear_selection_display(&mut self) {
        self.clear_selection_display()
    }

    fn select_all_text_region(&mut self) -> bool {
        self.select_all_text_region()
    }

    fn selection_text_for_copy(&self) -> String {
        self.extract_selection_text()
    }

    fn focused_activity_bar_id(&self) -> Option<&WidgetId> {
        self.focused_activity_bar_id()
    }

    fn match_keypress(
        &self,
        key: &crate::Key,
        modifiers: crate::Modifiers,
    ) -> Option<AcceleratorId> {
        self.match_keypress(key, modifiers)
    }

    fn fold_double_click(&mut self, ev: UiEvent) -> UiEvent {
        self.fold_double_click(ev)
    }

    /// Wires `WinBackend` onto the widget-contract routing engine via its
    /// embedded [`crate::backend_core::BackendCore`] — see
    /// [`crate::runtime::PreprocessBackend::widget_routing_mut`]'s doc for
    /// what overriding the trait's `None` default opts into.
    fn widget_routing_mut(&mut self) -> Option<&mut crate::widget_state::WidgetRoutingState> {
        Some(self.core.widget_routing_mut())
    }

    fn focus_tree_mut(&mut self) -> &mut crate::focus::FocusTree {
        &mut self.focus_tree
    }
}

impl crate::backend::sealed::Sealed for WinBackend {}

impl Backend for WinBackend {
    // ─── Frame + viewport ─────────────────────────────────────────────

    fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn cell_grid(&self) -> bool {
        // Direct2D draws in device-independent pixels on a continuous
        // render target: `viewport` is in DIPs, scaled by the window's DPI,
        // not in whole terminal cells.
        false
    }

    fn begin_frame(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        // Clear the focused activity bar — re-set by `end_frame` if this
        // frame paints it focused again. Same
        // lifecycle as `GtkBackend`/`MacBackend`'s `focused_activity_bar`
        self.focused_activity_bar = None;
        // Clear per-frame text regions so stale registrations from the
        // previous frame don't linger. Mirrors `GtkBackend`/`TuiBackend`'s
        // identical `begin_frame` clear.
        self.core.begin_frame();
        // Clear last frame's `register_zone` entries — a zone a widget stops
        // painting must stop resolving hit tests too. Mirrors
        // `GtkBackend::begin_frame`'s identical `self.zones.clear()`.
        self.zones.clear();
        // Clear last frame's tooltip/toast bounds — see
        // `overlay_bounds_this_frame`'s own doc. Same per-frame lifecycle
        // as `zones`.
        self.overlay_bounds_this_frame.clear();
        // Install the shared paint-time text-run recording sink for the
        // duration of this frame — drained into `self.text_runs` by
        // `end_frame` below. Mirrors `MacBackend::enter_frame_scope`'s
        // start/stop bracket; Win-GUI needs no closure-based
        // frame scope of its own, since `draw_*` trait methods already
        // have `&mut self` throughout the frame.
        if self.painted_text_recording {
            let _ = crate::testing::install_text_run_sink();
        }
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            // `BeginDraw`/`Clear` are infallible on `ID2D1RenderTarget`
            // (device-lost errors only surface later, from `EndDraw` —
            // handled in `end_frame` below).
            //
            // SAFETY: `surface.target` is a live `ID2D1RenderTarget` COM
            // object for as long as `self.surface` holds it; `BeginDraw`
            // takes no arguments and its only precondition (must be
            // paired with a later `EndDraw`, never called twice without
            // one in between) is upheld by `end_frame` below being this
            // frame's only other caller.
            unsafe {
                surface.target.BeginDraw();
            }
            // Route this frame's paint helpers through the surface's
            // brush cache; `end_frame` deactivates it.
            super::text::activate_brush_cache(&surface.target, &surface.brushes);
            // Use the theme's background colour instead of a hard-coded dark
            // shade — unpainted areas must show the correct background on light
            // themes too.
            let clear_color = super::text::color_to_d2d(self.current_theme.background);
            // SAFETY: same live `surface.target` as `BeginDraw` above,
            // now inside the `BeginDraw`/`EndDraw` bracket that call
            // opened; `clear_color` is a stack-local `D2D1_COLOR_F`
            // passed by reference for the duration of the call.
            unsafe {
                surface.target.Clear(Some(&clear_color));
            }
        }
        // A non-Windows host never has Direct2D types to call
        // `BeginDraw`/`Clear` against at all — same "nothing to paint
        // onto" posture as the `self.surface.is_none()` case above, not a
        // gap this issue's fallbacks need to fake. The
        // `#[cfg]` still binds directly to this statement (not a sibling
        // unused-param line), unlike the bug this issue otherwise fixes —
        // see the module-level issue doc.
        #[cfg(not(target_os = "windows"))]
        {}
    }

    fn end_frame(&mut self) {
        // The rail that holds keyboard focus is whichever activity bar this
        // frame painted with `is_keyboard_focused` set.
        self.focused_activity_bar = crate::primitives::activity_bar::painted_keyboard_focused_bar();
        // Drain the sink `begin_frame` installed, if recording was on —
        // see that method's doc for why this needs no closure-based frame
        // scope the way GTK/macOS's `enter_frame_scope` does.
        if self.painted_text_recording {
            self.text_runs = crate::testing::take_text_run_sink(None);
        }
        #[cfg(target_os = "windows")]
        super::text::deactivate_brush_cache();
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            // `EndDraw` fails for two distinct reasons, both handled the
            // same way here — the well-known `D2DERR_RECREATE_TARGET`
            // (GPU driver reset, remote-desktop session change, etc.) and
            // anything else — since either way the render target must be
            // rebuilt before the next frame can paint onto it. Drop the
            // surface rather than silently pretending the frame
            // presented, or letting the next frame paint onto a target
            // Direct2D has already discarded. `self.hwnd`/`headless_target`
            // (set by `attach_surface`/`attach_headless`, untouched here)
            // survive the drop, so the next `WM_PAINT`/`WM_SIZE` reaching
            // `win::run` (or the next `WinDriver::render`, issue #805)
            // actually recreates it via `Self::ensure_surface` — see that
            // method's docs.
            //
            // Now reported through the trait's error channel (issue
            // #507/#805, `docs/decisions/DECISIONS.md` D-009) instead of
            // silently dropping on the floor: `last_error()` hands the
            // caller `BackendError::SurfaceLost`, and `diagnostics::emit`
            // gives a host with a sink installed (`src/diagnostics.rs`)
            // a chance to log it even if nothing ever polls
            // `last_error()`.
            //
            // SAFETY: `surface.target` is the same live COM object
            // `begin_frame`'s `BeginDraw` call opened a draw bracket on;
            // `EndDraw(None, None)` (no tag pointers requested back) is
            // the documented way to close it without the optional
            // per-primitive-tag diagnostics.
            if unsafe { surface.target.EndDraw(None, None) }.is_err() {
                self.surface = None;
                self.last_error = Some(BackendError::SurfaceLost);
                crate::diagnostics::emit(
                    "quadraui win backend: EndDraw failed (device lost / \
                     D2DERR_RECREATE_TARGET) — render target dropped, will \
                     recreate via ensure_surface on the next frame",
                );
            }
        }
        // See `begin_frame`'s matching comment.
        #[cfg(not(target_os = "windows"))]
        {}
    }

    // ─── Theming ────────────────────────────────────────────────────────

    /// Store the theme so the `draw_*` rasterisers below use it instead of
    /// falling back to `Theme::default()`. Mirrors `GtkBackend::set_theme`
    /// and `MacBackend::set_theme`.
    fn set_theme(&mut self, theme: crate::Theme) {
        self.set_current_theme(theme);
    }

    fn theme(&self) -> crate::Theme {
        self.current_theme
    }

    fn set_style(&mut self, style: crate::Style) {
        self.set_current_style(style);
    }

    fn style(&self) -> crate::Style {
        self.current_style
    }

    /// Store the nerd-fonts flag so `TreeView`'s paint paints `Icon::glyph`
    /// instead of always falling back to `Icon::fallback`.
    /// Mirrors `TuiBackend::set_nerd_fonts`/`GtkBackend::set_nerd_fonts`/
    /// `MacBackend::set_nerd_fonts` — see [`Backend::set_nerd_fonts`]'s
    /// doc for why every backend defaults this to `false`.
    fn set_nerd_fonts(&mut self, enabled: bool) {
        self.nerd_fonts_enabled = enabled;
    }

    fn nerd_fonts_enabled(&self) -> bool {
        self.nerd_fonts_enabled
    }

    /// Store the chrome font description for the next
    /// [`Self::attach_surface`]/[`Self::attach_headless`] call to build a
    /// chrome `IDWriteTextFormat` from, parsed via
    /// [`parse_ui_font_desc`]. Same "doesn't rebuild a live surface's
    /// text format immediately" limitation as [`Self::set_editor_font`] —
    /// see that method's doc.
    fn set_ui_font(&mut self, font_desc: &str) {
        #[cfg(target_os = "windows")]
        {
            let (family, size_pt) = parse_ui_font_desc(font_desc);
            self.ui_font_family = family;
            self.ui_font_size_pt = size_pt;
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = font_desc;
        }
    }

    /// Win-GUI's platform-native convention: `Consolas
    /// 14` for editor content, `Segoe UI` (rendered at VS Code's 13 DIP
    /// chrome size) for UI chrome — VS Code's own Windows defaults.
    /// `editor_family`/`ui_family` are plain literals rather than the
    /// `DEFAULT_EDITOR_FONT_FAMILY`/`DEFAULT_UI_FONT_FAMILY` consts
    /// themselves — unlike [`Self::set_ui_font`] above, this method has no
    /// DirectWrite call to gate, and must type-check on every host per
    /// this crate's cross-platform commitment (see `CLAUDE.md`'s Win-GUI
    /// section) — and `editor_size_pt` stays a plain `14.0` literal: it
    /// models this platform recommendation, distinct from
    /// `editor_font_size_pt`'s own `11.0` "unset" runtime default.
    /// `ui_size_pt` is the one exception: it reads [`DEFAULT_UI_FONT_SIZE_PT`]
    /// (itself not `cfg`-gated, so it type-checks on every host too)
    /// rather than a second literal, so the chrome size `WinBackend::new`
    /// actually installs and the size advertised here can never drift
    /// apart — see that const's doc.
    fn default_fonts(&self) -> crate::backend::PlatformFontDefaults {
        crate::backend::PlatformFontDefaults {
            editor_family: "Consolas".to_string(),
            editor_size_pt: 14.0,
            ui_family: "Segoe UI".to_string(),
            ui_size_pt: DEFAULT_UI_FONT_SIZE_PT,
        }
    }

    /// Register `bytes` with DirectWrite via
    /// [`crate::win::text::register_font_from_memory`] — no
    /// filesystem write, process-lifetime only. Stores the resulting
    /// private `IDWriteFontCollection1` (so a later
    /// [`Self::set_nerd_font_fallback`] resolves against the font that
    /// was actually registered).
    ///
    /// Copies `bytes` into `self.registered_font_bytes` *first*, then
    /// calls `text::register_font_from_memory` with a pointer into that
    /// owned copy — never `bytes` itself. `bytes` is the caller's slice
    /// and, per this trait method's doc, may be a local the caller drops
    /// the instant this call returns; `CreateInMemoryFontFileReference`
    /// does not copy its input, so handing it a pointer into a
    /// caller-owned, possibly-transient buffer would leave DirectWrite
    /// holding a dangling pointer the moment that buffer is freed. See
    /// `registered_font_bytes`'s field doc for the rest of this
    /// backend's lifetime story.
    fn register_font_from_memory(&mut self, bytes: &[u8]) -> Option<Vec<String>> {
        #[cfg(target_os = "windows")]
        {
            self.registered_font_bytes.push(bytes.to_vec());
            let owned: &[u8] = self
                .registered_font_bytes
                .last()
                .expect("just pushed a value above");
            match crate::win::text::register_font_from_memory(owned) {
                Ok((collection, names)) => {
                    self.registered_font_collection = Some(collection);
                    Some(names)
                }
                Err(_) => {
                    // Registration failed, so nothing references this
                    // copy — but there's no live pointer into it either,
                    // and leaving it in place (rather than popping it
                    // back off) keeps this method's error path as simple
                    // as its success path. See `registered_font_bytes`'s
                    // field doc: entries are never removed anyway.
                    None
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = bytes;
            None
        }
    }

    /// Answers via [`crate::win::text::has_font_family`]:
    /// DirectWrite's system font collection lookup (`FindFamilyName`),
    /// distinct from `register_font_from_memory`'s private,
    /// app-registered collection. `Err` (a DirectWrite factory/
    /// collection failure, not "family not found" — that's `Ok(false)`)
    /// degrades to `None`, same "don't fail, don't know" posture as
    /// every other fallible-but-non-critical Win-GUI query.
    fn has_font_family(&self, family: &str) -> Option<bool> {
        #[cfg(target_os = "windows")]
        {
            crate::win::text::has_font_family(family).ok()
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = family;
            None
        }
    }

    /// Build a Nerd-Font (or other PUA-codepoint) fallback for `family`
    /// via [`crate::win::text::build_nerd_font_fallback`] and store it
    /// for the next [`Self::attach_surface`]/[`Self::attach_headless`]
    /// call to bake into both `IDWriteTextFormat`s. Resolves
    /// against `registered_font_collection` when a prior
    /// [`Self::register_font_from_memory`] call populated one, otherwise
    /// against the system font collection — see
    /// `build_nerd_font_fallback`'s doc. Same "doesn't rebuild a live
    /// surface immediately" limitation as [`Self::set_editor_font`].
    fn set_nerd_font_fallback(&mut self, family: &str) {
        #[cfg(target_os = "windows")]
        {
            // `build_nerd_font_fallback_multi` with both
            // mappings, not a bare `build_nerd_font_fallback(family,
            // ...)` — a single `IDWriteFontFallback` is a full
            // replacement when stored into `self.nerd_font_fallback`
            // (DirectWrite text formats carry exactly one), so
            // rebuilding from `family` alone here would drop the
            // bundled codicon mapping `Self::new` seeded.
            let built = crate::win::text::build_nerd_font_fallback_multi(&[
                (family, self.registered_font_collection.as_ref()),
                (
                    crate::codicon::FONT_FAMILY,
                    self.codicon_font_collection.as_ref(),
                ),
            ]);
            match built {
                Ok(fallback) => self.nerd_font_fallback = Some(fallback),
                Err(err) => {
                    // This trait method returns `()`, so there's no
                    // `Result`/`Option` to hand a caller the way
                    // `register_font_from_memory` does — but silently
                    // dropping the built fallback with no diagnostic at
                    // all left a caller no way to learn this didn't take
                    // effect. Emit via
                    // `crate::diagnostics` (never a raw `eprintln!` —
                    // this is live library code a host may be running
                    // with the terminal in raw mode, see that module's
                    // doc) and keep whatever fallback, if any, was
                    // already active.
                    crate::diagnostics::emit(format!(
                        "quadraui: set_nerd_font_fallback({family:?}) failed to build a font \
                         fallback ({err:?}); icon glyphs may render as tofu"
                    ));
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = family;
        }
    }

    // ─── Events + keybindings ─────────────────────────────────────────

    /// Drain [`Self::events`] and run the result through
    /// [`Self::apply_accelerators`]. Never blocks — a plain `VecDeque` drain.
    /// Nothing feeds this queue from the live `wndproc` yet — see [`Self::events`]'s
    /// field doc and `win::run`'s "Dispatch model" module doc. A driver or other
    /// caller that pushes via [`Self::push_event`] gets a real, non-panicking
    /// drain.
    fn poll_events(&mut self) -> Vec<UiEvent> {
        let mut out: Vec<UiEvent> = self.events.borrow_mut().drain(..).collect();
        self.apply_accelerators(&mut out);
        // Fold in any `UiEvent::User` payloads a background thread queued via
        // `waker()` since the last drain. The live `wndproc` calls
        // `Self::drain_user_events` directly instead, but this keeps `poll_events`
        // correct for any other caller (a driver, a test).
        self.user_events.drain_into(&mut out);
        out
    }

    /// Same drain as [`Self::poll_events`] — `_timeout` is accepted (per
    /// the trait signature) but never waited out. A Win32 message loop
    /// has no native "block up to N ms for the next message" primitive
    /// that fits this poll-style trait (`MsgWaitForMultipleObjects` waits
    /// on kernel handles, not "either a message or a timeout elapses
    /// with the queue left untouched"), and nothing in this backend
    /// drives a real `HWND`'s message queue from here — the live
    /// `wndproc` pump stays the direct-dispatch path (see
    /// [`Self::poll_events`]'s doc). Returning a plain drain rather than
    /// spinning `GetMessageW` (a real, uninterruptible block with no
    /// timeout at all) mirrors `MacBackend::wait_events`'s identical
    /// "today this is a plain drain" posture — both callback-driven
    /// backends where the native run loop, not this trait method, is
    /// what actually waits.
    fn wait_events(&mut self, _timeout: Duration) -> Vec<UiEvent> {
        self.poll_events()
    }

    /// See [`Backend::waker`]'s doc for the full cross-backend contract.
    /// A live `GetMessageW` loop blocks *indefinitely* for the next Win32
    /// message with no timeout at all — the most severe case of the four
    /// backends (TUI/GTK both re-poll on a bounded interval regardless;
    /// see `Backend::waker`'s doc). `PostMessageW` is Win32's own
    /// documented thread-safe "queue a message for this window, waking a
    /// blocked `GetMessageW`" primitive, posting
    /// [`WM_QUADRAUI_USER_EVENT`] — `win::run`'s `wndproc` match arm on
    /// that value is what actually drains [`Self::user_events`] and
    /// dispatches, since `wndproc` already has direct access to the app/
    /// backend state a live message carries with it (unlike GTK, which
    /// needs the `WakeId`/thread-local `wake_callback` indirection because
    /// its wake primitive's callback has no such access — see
    /// `GtkBackend::waker`'s doc).
    ///
    /// `self.hwnd` is `None` until [`Self::attach_surface`] runs (or on
    /// any non-Windows host, where it never exists at all — see that
    /// field's doc) — the closure is a documented no-op in that case,
    /// same posture as `RecordingBackend::waker` and
    /// `GtkBackend::waker`'s "no `wake_callback` installed yet" case: a
    /// payload still lands in [`Self::user_events`] for any later
    /// `poll_events` caller, there is just no live window to nudge yet.
    /// That no-op is *transient*, not permanent: the closure reads
    /// [`Self::hwnd_raw`] fresh on every call rather than snapshotting
    /// `self.hwnd` once at `waker()` call time, so a waker obtained
    /// during `AppLogic::setup` (before `attach_surface` has run) still
    /// wakes the loop correctly once a real window exists — see
    /// `hwnd_raw`'s doc for the full rationale and the GTK/macOS
    /// precedent this mirrors.
    fn waker(&self) -> std::sync::Arc<dyn Fn(UserPayload) + Send + Sync> {
        let queue = std::sync::Arc::clone(&self.user_events);
        #[cfg(target_os = "windows")]
        {
            // `HWND` wraps a raw pointer and isn't `Send`, but the numeric
            // handle value is — Win32 handles are opaque integers the OS
            // resolves, not memory this process dereferences directly, so
            // round-tripping through `isize` and reconstructing `HWND` on
            // the other side is exactly as valid as the original value.
            // `PostMessageW` is documented safe to call against a window
            // owned by a different thread than the caller (that's its
            // entire purpose here). `hwnd_raw` is the same `Arc` `self`
            // holds (not a value copied out of it), so this closure sees
            // whatever `attach_surface` stores into it later, however
            // long after this closure was created.
            let hwnd_raw = std::sync::Arc::clone(&self.hwnd_raw);
            std::sync::Arc::new(move |payload: UserPayload| {
                queue.push(payload.into_arc());
                if let Some(hwnd) = live_hwnd(&hwnd_raw) {
                    // SAFETY: `hwnd` came from `live_hwnd`, which reads
                    // `hwnd_raw` fresh on every call — see the doc comment
                    // above for why round-tripping through `isize` this
                    // way is valid even from a background thread, and why
                    // `PostMessageW` (documented as thread-safe to call
                    // against another thread's window) is the right
                    // primitive here rather than `SendMessageW`.
                    unsafe {
                        let _ =
                            PostMessageW(Some(hwnd), WM_QUADRAUI_USER_EVENT, WPARAM(0), LPARAM(0));
                    }
                }
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::sync::Arc::new(move |payload: UserPayload| {
                queue.push(payload.into_arc());
            })
        }
    }

    /// See [`Backend::request_frame_in`]'s doc for the cross-backend
    /// contract. Unlike [`Self::waker`] this always runs
    /// synchronously on the thread that owns the message loop — called
    /// directly from event/tick handling, never from a background
    /// thread — so it needs none of `waker`'s cross-thread `AtomicIsize`
    /// live-mirror machinery: a plain read of `self.hwnd` is safe.
    /// Arms [`FRAME_TIMER_ID`] via `SetTimer`, the same Win32
    /// one-shot-timer primitive `win::run`'s resize-settle debounce uses;
    /// `win::run::wndproc`'s `WM_TIMER` arm on that id is what actually
    /// fires `AppLogic::tick`/re-dispatches, mirroring `WM_QUADRAUI_USER_EVENT`'s
    /// arm for `waker`.
    ///
    /// A no-op before [`Self::attach_surface`] has ever run (`self.hwnd`
    /// is still `None`) — same posture as `waker`'s "no window to nudge
    /// yet" case, except here there is no queued payload to preserve for
    /// later delivery: a `request_frame_in` call with no window is simply
    /// lost, matching [`crate::runner::Reaction::RedrawAfter`]'s "may
    /// wake earlier... but never required to fire at all before a native
    /// event exists to carry it" posture — there is no event loop yet for
    /// an unattached backend to wake.
    ///
    /// Records into [`Self::frame_scheduler`] unconditionally, `hwnd` or
    /// not — that bookkeeping is test-facing only (see
    /// [`Self::frame_requests`]/[`Self::pending_frame_delay`]) and must
    /// not depend on the real `SetTimer` arm below, which stays exactly
    /// as no-op-before-attach as before.
    fn request_frame_in(&self, delay: Duration) {
        self.frame_scheduler.request(delay);
        #[cfg(target_os = "windows")]
        {
            if let Some(hwnd) = self.hwnd {
                // SAFETY: this runs synchronously on the thread that owns
                // the message loop (per the doc comment above), so
                // `self.hwnd` is a live window read on its own thread;
                // `lpTimerFunc: None` posts `WM_TIMER` through the normal
                // message queue rather than invoking a raw function
                // pointer.
                unsafe {
                    SetTimer(
                        Some(hwnd),
                        FRAME_TIMER_ID,
                        delay.as_millis().min(u32::MAX as u128) as u32,
                        None,
                    );
                }
            }
        }
    }

    fn invalidate_rect(&mut self, rect: crate::event::Rect) {
        self.core.record_invalidation(rect);
    }

    fn invalidation_log(&self) -> &crate::backend::InvalidationLog {
        self.core.invalidation_log()
    }

    fn register_accelerator(&mut self, acc: &Accelerator) {
        self.core.register_accelerator(acc);
    }

    fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.core.unregister_accelerator(id);
    }

    /// Persists via `self.core`, same one-liner delegation as the
    /// accelerator methods above.
    fn menu_style(&self) -> crate::backend::MenuStyle {
        self.core.menu_style()
    }

    fn set_menu_style(&mut self, style: crate::backend::MenuStyle) {
        self.core.set_menu_style(style);
    }

    // ─── Text selection ─────────────────────────────────────────────────

    /// Overrides the trait's no-op default — see [`Self::text_regions`]
    /// and `crate::text_selection::TextSelectionState`.
    fn register_text_region(&mut self, region: TextRegion) {
        self.core.register_text_region(region);
    }

    /// Overrides the trait's no-op default — see
    /// [`Self::cancel_text_selection_drag_impl`].
    fn cancel_text_selection_drag(&mut self) {
        self.cancel_text_selection_drag_impl();
    }

    // ─── Zone registration ──────────────────────────────────────────────────
    //
    // Overrides the trait's no-op default. Every `draw_*` rasteriser on this
    // backend that already called `self.register_zone(...)` used to have that
    // call silently swallowed; this is the first thing that reads `self.zones`
    // back.

    /// Overrides the trait's no-op default. Every `draw_*` rasteriser that
    /// already called `self.register_zone(...)` used to have that call silently
    /// swallowed. This reads `self.zones` back via [`Self::nc_hit_test`]'s
    /// `WM_NCHITTEST` classification. Mirrors `GtkBackend::register_zone` and
    /// `MacBackend::register_zone`.
    fn register_zone(&mut self, id: WidgetId, bounds: Rect) {
        self.zones.push(crate::testing::ZoneRec { id, bounds });
    }

    // ─── Modal-overlay tracking ───────────────────────────────────────

    fn modal_stack_handle(&self) -> Rc<RefCell<ModalStack>> {
        self.modal_stack.clone()
    }

    fn drag_state_handle(&self) -> Rc<RefCell<DragState>> {
        self.drag_state.clone()
    }

    fn focus_tree(&self) -> &crate::focus::FocusTree {
        &self.focus_tree
    }

    fn set_focus(&mut self, id: Option<WidgetId>) -> bool {
        self.focus_tree.set_focus(id.map(|id| (id, None)))
    }

    fn frame_capture(&mut self) -> Option<&mut crate::frame::FrameCapture> {
        Some(&mut self.frame_capture)
    }

    fn last_frame(&self) -> Option<&crate::frame::OwnedFrame> {
        self.frame_capture.last_frame()
    }

    /// Strokes the generic tab-stop focus-ring convention around `rect`.
    ///
    /// Called by [`super::run::render_frame`] *after* `AppLogic::render`
    /// returns, deliberately — so the ring paints on top of whatever the
    /// focused widget's own content drew. That same lateness means it
    /// would also paint on top of a tooltip or toast `render` drew over
    /// this same rect, inverting the overlay-order contract (tooltip/toast
    /// stay topmost of all). Skipping the stroke wherever it would land on
    /// a tooltip/toast bound recorded this frame — see
    /// `overlay_bounds_this_frame` — keeps the ring out of the way of an
    /// overlay without needing to defer either paint call.
    ///
    /// The ring is a perimeter stroke, not a filled rect
    /// ([`focus_ring_band_overlaps`]), so an overlay that lands strictly
    /// inside `rect` without touching the stroke band — e.g. a toast inset
    /// within a focused widget that spans the whole viewport — never
    /// conflicts with the ring and leaves it (and its `register_zone`
    /// chrome entry) intact. Mirrors
    /// `macos::backend::MacBackend::draw_focus_ring`'s identical fix.
    fn draw_focus_ring(&mut self, rect: Rect) {
        let stroke_width = self.current_style.focus_ring_width;
        if self
            .overlay_bounds_this_frame
            .iter()
            .any(|overlay| focus_ring_band_overlaps(rect, stroke_width, *overlay))
        {
            return;
        }
        let theme = self.current_theme;
        self.surface_stroke_rect(rect, theme.accent_fg, stroke_width);
        self.register_zone(WidgetId::new("chrome:focus-ring"), rect);
    }

    // ─── Error reporting ────────────────────────────────────────────

    /// Drain the `BackendError` `end_frame` recorded on a failed
    /// `EndDraw`, if any, since the last call. Mirrors
    /// [`Self::attach_surface`]/[`Self::attach_headless`]'s "surface is
    /// gone, `ensure_surface` recreates it lazily" recovery: this is the
    /// method a caller polls to learn that happened at all.
    fn last_error(&mut self) -> Option<BackendError> {
        self.last_error.take()
    }

    // ─── Platform services ────────────────────────────────────────────

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    // ─── Paint surface ─────────────────────────────────────────────────

    /// `WinBackend` implements [`PaintSurface`] directly (see this
    /// module's own `impl PaintSurface for WinBackend` block below), so
    /// this always answers `Some(self)` — never the `None` default
    /// `Backend::paint_surface` falls back to. Each verb degrades on its
    /// own (see e.g. `surface_measure_text`'s doc) when `self.surface`/
    /// `self.dwrite` aren't attached yet, matching `Backend::draw_*`'s
    /// existing "no surface yet" posture — there is no additional check
    /// to make here.
    fn paint_surface(&mut self) -> Option<&mut dyn PaintSurface> {
        Some(self)
    }

    // ─── Cursor ───────────────────────────────────────────────────────

    /// Maps `shape` onto a `SetCursor(LoadCursorW(..))` call via
    /// [`pointer_shape_to_win32_cursor`] and records it in
    /// `current_pointer_shape` so `win::run`'s `WM_SETCURSOR` handler can
    /// keep re-applying the same shape for as long as the pointer stays
    /// over the client area — see that field's doc comment for why a
    /// one-shot `SetCursor` call here isn't enough on its own. Always
    /// records the shape (portable, no WinAPI dependency); only the
    /// `SetCursor` call itself is Windows-only, mirroring every other
    /// `#[cfg(target_os = "windows")]` split in this file.
    fn set_cursor(&mut self, shape: PointerShape) -> bool {
        self.current_pointer_shape = shape;
        #[cfg(target_os = "windows")]
        {
            if self.hwnd.is_none() {
                // No window yet — matches `GtkBackend::set_cursor`'s
                // and `MacBackend::set_cursor`'s identical no-window
                // no-op posture.
                return false;
            }
            self.apply_current_cursor();
            true
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    // ─── Window control ─────────────────────────────────────────────────
    fn window(&mut self) -> Option<&mut dyn crate::backend::WindowControl> {
        #[cfg(target_os = "windows")]
        {
            self.hwnd?;
            Some(self)
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    // ─── Clipping ────────────────────────────────────────────────────────
    //
    // Direct2D already owns a push/pop clip stack reached through
    // `surface_push_clip`/`surface_pop_clip` (this module's own `impl
    // PaintSurface for WinBackend`, below). These two forward to that same
    // stack instead of layering a second one on top, so a caller that mixes
    // `Backend::push_clip` (binding-core's layout painter) and
    // `PaintSurface::surface_push_clip` (in-tree primitive paint code) on
    // the same frame nests correctly either way.
    fn push_clip(&mut self, rect: Rect) {
        self.surface_push_clip(rect);
    }

    fn pop_clip(&mut self) {
        self.surface_pop_clip();
    }

    // ─── Tray / status-bar icon ─────────────────────────────────────────
    /// Gated on `self.hwnd` for the same reason as [`Self::window`]:
    /// `Shell_NotifyIconW` needs a real `HWND` to own the icon and
    /// receive its callback message, which only exists once
    /// `attach_surface` has run.
    fn tray(&mut self) -> Option<&mut dyn crate::backend::TrayService> {
        #[cfg(target_os = "windows")]
        {
            self.hwnd?;
            Some(self)
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    // ─── Capability declaration ──────────────────────────────────────────

    /// Honest, not aspirational: the window + render-target bootstrap
    /// (`begin_frame`/`end_frame`) and the three window-lifecycle events
    /// the message loop itself needs (`WindowResized`/`DpiChanged`/
    /// `WindowClose`); the rest of the input table — mouse buttons/
    /// motion/wheel, `WM_KEYDOWN`/`WM_CHAR`, and focus — all dispatched
    /// directly from `win::run`'s `WndProc`; the queue-adapter drain is
    /// wired but nothing feeds that queue from the live `WndProc` yet.
    ///
    /// `mouse`/`scroll`/`drag` are declared `true`: the translators are
    /// wired (`WM_LBUTTONDOWN`/`WM_LBUTTONUP`/`WM_MOUSEMOVE`/
    /// `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` all reach `dispatch_event`), and
    /// the painted-text sink means a scenario that now runs instead of
    /// skipping fails on real signal rather than panicking.
    ///
    /// `text_selection` is declared `true`: `register_text_region`/
    /// `cancel_text_selection_drag` are both overridden, backed by the
    /// same `crate::text_selection::TextSelectionState` `GtkBackend`/
    /// `TuiBackend` embed, so a conformance scenario requiring
    /// `"text_selection"` now runs instead of skipping.
    fn backend_caps(&self) -> crate::backend::BackendCaps {
        // A genuine struct literal (not `let mut caps = ...; caps.field =
        // true;`), matching every other backend's `backend_caps` shape —
        // `tests/conformance/caps.rs`'s `BackendSource::declared` parses
        // this method's source for `<field>: true,` lines, so an
        // assignment-statement form would silently parse as declaring nothing.
        #[cfg(target_os = "windows")]
        {
            // File dialogs (`IFileOpenDialog`/`IFileSaveDialog`) and
            // notifications (`Shell_NotifyIconW`) go through COM/Shell APIs
            // independent of the Direct2D rasteriser, so they're honestly `true`
            // on Windows itself.
            //
            // `folder_dialogs`: the same `IFileOpenDialog`, with
            // `FOS_PICKFOLDERS` set — see
            // `src/win/services.rs::win_show_folder_open_dialog`.
            //
            // `native_dialogs`: `show_message_dialog` now shows a real
            // `TaskDialogIndirect` alert and returns the chosen button's id —
            // see `src/win/services.rs::win_show_message_dialog`. `CAP_CONTRACTS`'s
            // `native_dialogs` entry is `Unprovable` (every backend implements it),
            // so this declaration isn't source-checked; it's true because the
            // alert is real.
            //
            // `pointer_cursor`: `set_cursor` now drives a real
            // `SetCursor`/`WM_SETCURSOR` round-trip instead of the trait's no-op
            // default — see `Self::set_cursor` / `Self::apply_current_cursor`.
            //
            // `mouse`/`scroll`/`drag`: the translators are wired —
            // `WM_LBUTTONDOWN`/`WM_LBUTTONUP`/`WM_MOUSEMOVE` reach
            // `dispatch_event` as `MouseDown`/`MouseUp`/`MouseMoved`, and
            // `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` reach it as `Scroll` — so a
            // press → move → release sequence is a real drag.
            //
            // `text_selection`: `register_text_region` and
            // `cancel_text_selection_drag` are both overridden, backed by the
            // shared `crate::text_selection::TextSelectionState` GTK/TUI also
            // embed — a conformance scenario requiring `"text_selection"`
            // now runs instead of skipping.
            crate::backend::BackendCaps {
                file_dialogs: true,
                folder_dialogs: true,
                native_dialogs: true,
                notifications: true,
                pointer_cursor: true,
                mouse: true,
                scroll: true,
                drag: true,
                text_selection: true,
                // `register_font_from_memory`/`set_nerd_font_fallback`:
                // both overridden above, backed by real
                // DirectWrite calls (`crate::win::text::register_font_from_memory`
                // / `build_nerd_font_fallback`) rather than the trait's
                // no-op default.
                app_font_registration: true,
                // `generic_font_families`: `set_editor_font`/
                // `set_ui_font` are both overridden above and now resolve
                // CSS/Pango generic tokens to their DirectWrite defaults
                // (`DEFAULT_EDITOR_FONT_FAMILY`/`DEFAULT_UI_FONT_FAMILY`)
                // via `parse_ui_font_desc`/`GenericFamily::parse` instead
                // of treating them as an unresolvable literal family name.
                generic_font_families: true,
                // `window` is overridden below and returns
                // `Some` once `attach_surface` has stashed a real
                // `HWND` — see `impl WindowControl for WinBackend`'s doc
                // for which methods it backs with real `SetWindowPos`/
                // `ShowWindow`/`SetWindowTextW` calls versus report
                // `Unsupported` (`set_min_size`/`set_max_size`, which
                // would need `WM_GETMINMAXINFO` wired into `win::run`'s
                // `wndproc` to mean anything during a live interactive
                // resize — tracked as follow-up, not silently faked
                // here as a one-shot clamp).
                window_control: true,
                // `tray` is overridden below and returns
                // `Some` once `attach_surface` has stashed a real
                // `HWND` — see `impl TrayService for WinBackend`'s doc
                // for the real `Shell_NotifyIconW`/`TrackPopupMenuEx`
                // calls backing each method.
                tray: true,
                // `native_menu`: a real `HMENU` used to be built and attached here
                // via `SetMenu`, but that sits in the *non-client* area — exactly
                // the strip the `WM_NCCALCSIZE` handler (below) reclaims for the
                // app's own drawn caption. With both landed, the native menu was
                // permanently covered by the drawn band and unreachable, and
                // `window_chrome`'s custom caption stays; a native menu would need
                // the native caption back. So Win-GUI now matches `GtkBackend`/
                // `TuiBackend`: no `Backend` override at all, taking the trait's
                // no-op default — `install_menu_bar` callers get a painted in-window
                // `MenuBar` widget instead. Left `false` rather than
                // declared and unprovable — the honesty check requires *no*
                // `install_menu_bar`/`show_context_menu` override to back a `false`
                // here, which holds: this backend has neither.
                //
                // `window_chrome`: `toggle_window_maximize` is overridden below (real
                // `IsZoomed`/`ShowWindow` calls), and `win::run`'s `wndproc` now
                // handles `WM_NCCALCSIZE`/`WM_NCHITTEST` to fold a host's drawn
                // title-bar band into the native caption the way `GtkBackend`'s CSD
                // titlebar already does — see [`Self::nc_hit_test`]'s doc for the
                // classification and `App::render_content`'s doc for the consumer-side
                // branch this un-gates. Declaring this before `win::run` grew that
                // handling would have just stacked a second, always-drawn band under
                // the real native caption instead of replacing it.
                window_chrome: true,
                ..crate::backend::BackendCaps::empty()
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            crate::backend::BackendCaps::empty()
        }
    }

    // ─── Window chrome ──────────────────────────────────────────────────
    //
    // Unlike `GtkBackend`/`MacBackend`, Win-GUI keeps its native
    // `WS_OVERLAPPEDWINDOW` non-client frame (`win::run`'s window creation
    // — this issue's other half handles only `WM_NCCALCSIZE`/
    // `WM_NCHITTEST`, not `WS_POPUP`/undecorated CSD). Windows itself
    // still owns drag-to-move and double-click-to-maximize for any point
    // `WM_NCHITTEST` classifies as `HTCAPTION` (`win::run`'s `wndproc`,
    // via `Self::nc_hit_test`) — `DefWindowProcW`'s own non-client move
    // loop handles both without this backend ever calling
    // `Backend::begin_window_drag`/`begin_window_resize`, so only
    // `toggle_window_maximize` below is overridden: a host that wants a
    // "Maximize" menu item or keybinding alongside the pointer gesture has
    // something real to call. `CAP_CONTRACTS`'s `window_chrome` entry is
    // `Proof::Any` — one overridden method is enough to make the
    // `backend_caps` declaration honest.

    /// `IsZoomed`/`ShowWindow(SW_MAXIMIZE/SW_RESTORE)` — mirrors
    /// `GtkBackend::toggle_window_maximize`/`MacBackend::toggle_window_maximize`.
    /// Returns `false` when this backend owns no window yet (matching
    /// every other backend's `begin_window_drag`-family contract).
    fn toggle_window_maximize(&mut self) -> bool {
        #[cfg(target_os = "windows")]
        {
            let Some(hwnd) = self.hwnd else {
                return false;
            };
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; both calls take only a handle (plus, for
            // `ShowWindow`, a plain show-command enum).
            let target = if unsafe { IsZoomed(hwnd) }.as_bool() {
                SW_RESTORE
            } else {
                SW_MAXIMIZE
            };
            // SAFETY: same as `IsZoomed` above — `hwnd` is still
            // `self.hwnd`.
            unsafe {
                let _ = ShowWindow(hwnd, target);
            }
            true
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    // ─── Measurement ──────────────────────────────────────────────────

    fn line_height(&self) -> f32 {
        self.current_line_height
    }

    fn char_width(&self) -> f32 {
        self.current_char_width
    }

    /// Forwards to [`Self::set_current_line_height`] — the
    /// `Backend`-trait-object-reachable twin of that inherent method, the
    /// Win-GUI counterpart of `GtkBackend`'s / `MacBackend`'s overrides of
    /// the same trait method.
    fn set_current_line_height(&mut self, line_height: f32) {
        WinBackend::set_current_line_height(self, line_height);
    }

    /// Forwards to [`Self::set_current_char_width`] — see
    /// [`Backend::set_current_line_height`]'s doc.
    fn set_current_char_width(&mut self, char_width: f32) {
        WinBackend::set_current_char_width(self, char_width);
    }

    /// Same value as [`Self::char_width`] (the *editor* font's advance)
    /// even though [`Self::draw_list`] (`super::list::draw_list`) now
    /// paints row text through `self.chrome_dwrite` — this
    /// backend has no `chrome_char_width` field of its own to report
    /// instead (unlike `GtkBackend::current_chrome_char_width`, a real
    /// per-frame Pango re-measurement of `ui_font`). A consumer budgeting
    /// `ListView` row width against this value may therefore get a
    /// slightly wrong estimate when the chrome and editor fonts differ
    /// in advance width; adding a real `chrome_char_width` field
    /// (mirroring GTK's) is tracked separately, unchanged by this
    /// issue.
    fn list_char_width(&self) -> f32 {
        self.current_char_width
    }

    /// Real DirectWrite measurement via `DWrite::measure_text` against
    /// `chrome_dwrite`/`dwrite` — the same per-role handle
    /// [`Self::surface_draw_text_run_with_role`] already resolves,
    /// including its "chrome falls back to the editor handle
    /// if no live chrome one exists yet" degrade. Before a surface has
    /// attached (`dwrite`/`chrome_dwrite` both still `None`, e.g. an app
    /// measuring a `Dialog` before the first frame), falls back to a
    /// `current_char_width`-scaled estimate — the same "degrade, don't
    /// panic" convention the [`WinBackend`] "Before a surface is attached" doc describes for the
    /// pre-attach window on this backend.
    fn measure_text(&self, text: &str, role: crate::FontRole) -> (f32, f32) {
        #[cfg(target_os = "windows")]
        {
            let dwrite = match role {
                crate::FontRole::Editor => self.dwrite.as_ref(),
                _ => self.chrome_dwrite.as_ref().or(self.dwrite.as_ref()),
            };
            if let Some(dwrite) = dwrite {
                let bold = role == crate::FontRole::ChromeBold;
                if let Ok((w, h)) = dwrite.measure_text_styled(text, bold) {
                    return (w, h);
                }
            }
        }
        let _ = role;
        (
            text.chars().count() as f32 * self.current_char_width,
            self.current_line_height,
        )
    }

    /// Store the editor font family + size for the next
    /// [`Self::attach_surface`]/[`Self::ensure_surface`] call to build an
    /// `IDWriteTextFormat` from. Mirrors `GtkBackend::set_editor_font`:
    /// a live surface doesn't rebuild its `IDWriteTextFormat`
    /// immediately on a runtime font change — same limitation the GTK
    /// backend has today (its Pango layout is rebuilt fresh every frame
    /// from these fields instead, which this backend's once-per-surface
    /// DirectWrite bootstrap doesn't yet mirror). A future issue can wire
    /// a live-reload path through `win::run`'s `WM_PAINT` handler if that
    /// gap needs closing.
    fn set_editor_font(&mut self, family: &str, size_pt: f32) {
        #[cfg(target_os = "windows")]
        {
            // A CSS/Pango generic token resolves to its DirectWrite default
            // immediately — `monospace` has no installed-family lookup to run,
            // only `DEFAULT_EDITOR_FONT_FAMILY` to substitute in. `sans-serif`/
            // `system-ui` aren't monospace (this method's doc requires that), but
            // there's no error channel to reject the mismatch through, so they
            // resolve honestly to the chrome default rather than being treated as
            // an unknown literal family name.
            self.editor_font_family = match GenericFamily::parse(family) {
                Some(GenericFamily::Monospace) => DEFAULT_EDITOR_FONT_FAMILY.to_string(),
                Some(GenericFamily::SansSerif | GenericFamily::SystemUi) => {
                    DEFAULT_UI_FONT_FAMILY.to_string()
                }
                None => family.to_string(),
            };
            self.editor_font_size_pt = size_pt;
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (family, size_pt);
        }
    }

    // ─── Drawing ──────────────────────────────────────────────────────

    /// see the [`WinBackend`] "Before a surface is attached" doc. `List` is
    /// `ChromePrimitive::List` — see `TreeView`'s paint's note for the
    /// font-swap shape.
    fn draw_list(&mut self, rect: Rect, list: &ListView) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            let dwrite = self.chrome_dwrite.as_ref().unwrap_or(dwrite);
            super::list::draw_list(
                &surface.target,
                dwrite,
                rect,
                list,
                self.resolved_row_height(self.current_line_height),
                &self.current_style,
            );
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a no-op instead
        // of panicking.
        let _ = (rect, list);
    }

    /// [`Self::draw_list`], plus hover/press row tinting, a
    /// focus-visible ring, and — when `state.scroll_px` is `Some` — a
    /// pixel-precise scroll position instead of only
    /// [`ListView::scroll_offset`]'s whole-row value, via
    /// [`super::list::draw_list_with_state`]. `state.scroll_px: None`
    /// paints pixel-identically to [`Self::draw_list`].
    fn draw_list_with_state(
        &mut self,
        rect: Rect,
        list: &ListView,
        state: &crate::primitives::list::ListPaintState,
        focus_ring_visible: bool,
    ) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            let dwrite = self.chrome_dwrite.as_ref().unwrap_or(dwrite);
            super::list::draw_list_with_state(
                &surface.target,
                dwrite,
                rect,
                list,
                self.resolved_row_height(self.current_line_height),
                &self.current_style,
                state,
                focus_ring_visible,
            );
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a no-op instead
        // of panicking.
        let _ = (rect, list, state, focus_ring_visible);
    }

    /// GTK's twin (`GtkBackend::list_hscrollbar`) builds the
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
        // Row pitch must match `draw_list`'s own (`resolved_row_height`),
        // or the thumb geometry drifts from the rows it tracks.
        let row_h = self.resolved_row_height(self.current_line_height);
        let track = crate::primitives::list::hscrollbar_track(rect, list.bordered, char_w, row_h);
        Some(crate::Scrollbar::horizontal(
            list.id,
            track,
            list.h_scroll as f32 * char_w,
            content_px,
            visible_px,
            row_h,
        ))
    }

    /// `vscrollbar_with_track_width` deals purely in row counts/row-height
    /// plus an explicit track width, so unlike [`Self::list_hscrollbar`] the
    /// primitive's own geometry method is unit-correct for every backend —
    /// see [`ListView::vscrollbar_with_track_width`]. `scrollbar_width` sets
    /// the track's column width so this hit-test agrees exactly with what
    /// the shared painter paints.
    fn list_vscrollbar(&self, rect: Rect, list: &ListView) -> Option<crate::Scrollbar> {
        list.vscrollbar_with_track_width(
            rect,
            self.resolved_row_height(self.current_line_height),
            self.current_style.scrollbar_width,
        )
    }

    fn list_layout(&self, rect: Rect, list: &ListView) -> crate::ListViewLayout {
        // `win_list_layout` is pure geometry — no Direct2D/DirectWrite type
        // in its signature — so it can be called directly instead of being
        // duplicated inline here.
        super::list::win_list_layout(
            list,
            rect,
            self.resolved_row_height(self.current_line_height),
            self.current_char_width,
        )
    }

    /// Real Direct2D/DirectWrite
    /// rendering via [`crate::primitives::find_replace::paint`] once a
    /// surface is attached. See the [`WinBackend`] "Before a surface is attached" doc for the
    /// "surface not attached yet" fallback posture.
    fn draw_find_replace(&mut self, rect: Rect, panel: &FindReplacePanel) {
        #[cfg(target_os = "windows")]
        if self.surface.is_some() && self.dwrite.is_some() {
            let _ = rect;
            let theme = self.current_theme;
            crate::primitives::find_replace::paint(panel, self, &theme);
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a no-op instead
        // of panicking.
        let _ = (rect, panel);
    }

    /// Real Direct2D rasteriser via `win::scrollbar` once a surface
    /// is attached. See the [`WinBackend`] "Before a surface is attached" doc for the "surface
    /// not attached yet" fallback posture. `rect` is unused on the
    /// painted path — `scrollbar.track` is authoritative, mirroring
    /// `MacBackend::draw_scrollbar`/`GtkBackend::draw_scrollbar`. Paints
    /// through [`crate::primitives::scrollbar::native_surface_paint::paint_with_opacity`]
    /// at [`Self::scrollbar_fade_opacity`] so the bar follows the same
    /// overlay idle-fade curve as the macOS leg, rather than staying
    /// permanently opaque.
    fn draw_scrollbar(&mut self, rect: Rect, scrollbar: &Scrollbar) {
        #[cfg(target_os = "windows")]
        if self.surface.is_some() {
            let _ = rect;
            let theme = self.current_theme;
            let opacity = self.scrollbar_fade_opacity();
            crate::primitives::scrollbar::native_surface_paint::paint_with_opacity(
                scrollbar, self, &theme, opacity,
            );
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a no-op instead
        // of panicking.
        let _ = (rect, scrollbar);
    }

    /// Paint via the shared
    /// [`crate::primitives::progress::native_surface_paint::paint`] once
    /// a surface is attached. The
    /// Windows backend now passes `self.current_theme` instead of painting
    /// `Theme::default()`, matching the macOS and GTK backends. See
    /// the [`WinBackend`] "Before a surface is attached" doc for the "surface not attached yet"
    /// fallback posture.
    fn draw_progress(&mut self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
        #[cfg(target_os = "windows")]
        if self.surface.is_some() && self.dwrite.is_some() {
            let theme = self.current_theme;
            let style = self.style();
            return crate::primitives::progress::native_surface_paint::paint(
                bar, self, &theme, rect, &style,
            );
        }
        // No surface/DWrite yet — return the real (pure-geometry) layout
        // via `progress_layout` and paint nothing.
        self.progress_layout(rect, bar)
    }

    /// Pure geometry — `ProgressBar::layout` needs no measurer at all
    /// (uniform cancel-affordance width), so this needs no
    /// `self.dwrite`/surface at all.
    ///
    /// This used to duplicate `super::progress`'s `CANCEL_WIDTH_DIP`
    /// constant by value (per `PRIMITIVE_RULES.md`#713, to avoid reaching
    /// into that Windows-only module).
    /// [`crate::primitives::layout_metrics::pixel_progress_layout`] is
    /// plain, platform-independent geometry, so it can be called directly
    /// here instead — one shared constant.
    fn progress_layout(&self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
        crate::primitives::layout_metrics::pixel_progress_layout(
            bar,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
        )
    }

    /// Paint via the shared
    /// [`crate::primitives::spinner::native_surface_paint::paint`] once
    /// a surface and a text handle are attached — mirrors
    /// [`Self::draw_progress`]. `self`'s
    /// [`crate::paint_surface::PaintSurface`] impl (`surface_measure_text`
    /// / `surface_draw_text_run`) already prefers `chrome_dwrite` over
    /// `dwrite` for chrome text, so the handle resolution happens inside that impl.
    /// See the [`WinBackend`] "Before a surface is attached" doc for the "surface not attached
    /// yet" fallback posture.
    fn draw_spinner(&mut self, rect: Rect, spinner: &Spinner) -> SpinnerLayout {
        #[cfg(target_os = "windows")]
        if self.surface.is_some() && (self.chrome_dwrite.is_some() || self.dwrite.is_some()) {
            let theme = self.current_theme;
            return crate::primitives::spinner::native_surface_paint::paint(
                spinner, self, &theme, rect.x, rect.y,
            );
        }
        // No surface/DWrite yet — compute the real layout via the same
        // nominal measurer `spinner_layout` falls back to, and paint
        // nothing.
        super::spinner::win_spinner_layout(
            &NominalSpinnerMeasure {
                char_width: self.current_char_width,
                line_height: self.current_line_height,
            },
            rect,
            spinner,
        )
    }

    /// Pure measurement — only needs `self.dwrite`, not a live
    /// render target, so this works as soon as a surface has ever been
    /// attached. Prefers
    /// `chrome_dwrite`, matching [`Self::draw_spinner`]'s paint font.
    fn spinner_layout(&self, rect: Rect, spinner: &Spinner) -> SpinnerLayout {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = self.chrome_dwrite.as_ref().or(self.dwrite.as_ref()) {
            return super::spinner::win_spinner_layout(dwrite, rect, spinner);
        }
        super::spinner::win_spinner_layout(
            &NominalSpinnerMeasure {
                char_width: self.current_char_width,
                line_height: self.current_line_height,
            },
            rect,
            spinner,
        )
    }
}

/// Win-GUI's `WindowControl` surface. Every method guards on
/// `self.hwnd` the same way [`Backend::set_cursor`] does — cheap defense
/// against a caller that stashed the trait object past a window teardown,
/// even though [`Backend::window`] only ever hands one out once `hwnd` is
/// `Some`. `bounds`/`set_bounds`/`set_size` operate on the **outer window
/// rect** (`GetWindowRect`/`SetWindowPos` semantics — includes the
/// title bar and borders), not the client area: computing an exact
/// client-area size would need an `AdjustWindowRectEx` round-trip through
/// the window's current style, which this issue doesn't attempt — see
/// [`Self::bounds`]'s own doc.
///
/// All coordinates cross the DIP ⇄ physical-pixel boundary via
/// [`WinBackend::dpi_scale`], the same field [`Backend::viewport`]'s
/// `scale` mirrors — so a caller working in this crate's native units
/// (DIPs, per every other backend) gets consistent numbers back from
/// [`Self::bounds`] regardless of the monitor's DPI.
impl WindowControl for WinBackend {
    fn set_title(&mut self, title: &str) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            let wide = win_wide_nul_terminated(title);
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `wide` is a `\0`-terminated `Vec<u16>` still in
            // scope, so `wide.as_ptr()` stays valid for the duration of
            // this synchronous call.
            unsafe { SetWindowTextW(hwnd, windows::core::PCWSTR::from_raw(wide.as_ptr())) }.map_err(
                |e| BackendError::PlatformFailure {
                    context: format!("SetWindowTextW: {e}"),
                },
            )
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = title;
            Err(BackendError::Unsupported)
        }
    }

    fn set_size(&mut self, width: f32, height: f32) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            let scale = self.dpi_scale;
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `SetWindowPos` takes only handles and plain
            // integers/flags here (`hWndInsertAfter: None` since
            // `SWP_NOZORDER` makes it ignored anyway).
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    (width * scale).round() as i32,
                    (height * scale).round() as i32,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            }
            .map_err(|e| BackendError::PlatformFailure {
                context: format!("SetWindowPos: {e}"),
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (width, height);
            Err(BackendError::Unsupported)
        }
    }

    /// **Not available.** A real minimum-size *floor* has to keep holding
    /// during live interactive resize, which Win32 only offers through
    /// `WM_GETMINMAXINFO` — a `wndproc` message this issue doesn't wire
    /// into `win::run` (out of this issue's file list, which names only
    /// `src/win/backend.rs`). A one-shot `SetWindowPos` clamp applied
    /// only at call time would silently stop enforcing the moment the
    /// user next drags an edge, which is a worse lie than declaring the
    /// gap outright — see this trait's own doc for why every method here
    /// prefers `Unsupported` over a partial, silently-abandoned effect.
    fn set_min_size(&mut self, _width: f32, _height: f32) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// **Not available**, same `WM_GETMINMAXINFO` reasoning as
    /// [`Self::set_min_size`].
    fn set_max_size(&mut self, _width: f32, _height: f32) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// The outer window rect (`GetWindowRect`), converted from physical
    /// pixels to DIPs via [`WinBackend::dpi_scale`] — see this `impl`
    /// block's own doc for why this is the outer rect, not the client
    /// area.
    fn bounds(&self) -> ServiceResult<Rect> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            let mut rect = RECT::default();
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `rect` is stack-local and outlives the call.
            unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|e| {
                BackendError::PlatformFailure {
                    context: format!("GetWindowRect: {e}"),
                }
            })?;
            let scale = self.dpi_scale.max(0.01);
            Ok(Rect::new(
                rect.left as f32 / scale,
                rect.top as f32 / scale,
                (rect.right - rect.left) as f32 / scale,
                (rect.bottom - rect.top) as f32 / scale,
            ))
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    fn set_bounds(&mut self, bounds: Rect) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            let scale = self.dpi_scale;
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; only handles and plain integers/flags cross this
            // call.
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    (bounds.x * scale).round() as i32,
                    (bounds.y * scale).round() as i32,
                    (bounds.width * scale).round() as i32,
                    (bounds.height * scale).round() as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            }
            .map_err(|e| BackendError::PlatformFailure {
                context: format!("SetWindowPos: {e}"),
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = bounds;
            Err(BackendError::Unsupported)
        }
    }

    fn center(&mut self) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            let mut window_rect = RECT::default();
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `window_rect` is stack-local and outlives the call.
            unsafe { GetWindowRect(hwnd, &mut window_rect) }.map_err(|e| {
                BackendError::PlatformFailure {
                    context: format!("GetWindowRect: {e}"),
                }
            })?;
            let monitor_rect = win_monitor_rect(hwnd)?;
            let w = window_rect.right - window_rect.left;
            let h = window_rect.bottom - window_rect.top;
            let mw = monitor_rect.right - monitor_rect.left;
            let mh = monitor_rect.bottom - monitor_rect.top;
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; only handles and plain integers/flags cross this
            // call.
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    monitor_rect.left + (mw - w) / 2,
                    monitor_rect.top + (mh - h) / 2,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            }
            .map_err(|e| BackendError::PlatformFailure {
                context: format!("SetWindowPos: {e}"),
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    /// Manual `WS_OVERLAPPEDWINDOW` style strip + `SetWindowPos` to the
    /// nearest monitor's full rect — Win32 has no single reversible
    /// "fullscreen" call the way GTK's `fullscreen`/`unfullscreen` or
    /// AppKit's `toggleFullScreen:` are, so [`WinBackend::fullscreen_saved`]
    /// does the remembering `toggleFullScreen:`'s own window-manager state
    /// would otherwise do for free. Idempotent: entering while already
    /// fullscreen, or exiting while not, is a no-op `Ok(())` rather than
    /// clobbering the saved rect with an already-fullscreen one.
    fn set_fullscreen(&mut self, fullscreen: bool) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            if fullscreen {
                if self.fullscreen_saved.is_some() {
                    return Ok(());
                }
                let mut rect = RECT::default();
                // SAFETY: (every `unsafe` block in this arm) `hwnd` is
                // `self.hwnd`, this backend's own live window, for the
                // whole call; `rect` is stack-local; `GetWindowLongPtrW`/
                // `SetWindowLongPtrW` exchange plain `isize` bitmasks (no
                // pointer), and `SetWindowPos` below takes only handles
                // and integers.
                unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|e| {
                    BackendError::PlatformFailure {
                        context: format!("GetWindowRect: {e}"),
                    }
                })?;
                // SAFETY: see this arm's opening comment above.
                let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
                self.fullscreen_saved = Some((rect, style));
                let stripped = style & !(WS_OVERLAPPEDWINDOW.0 as isize);
                // SAFETY: see this arm's opening comment above.
                unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, stripped) };
                let monitor_rect = win_monitor_rect(hwnd)?;
                // SAFETY: see this arm's opening comment above.
                unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        monitor_rect.left,
                        monitor_rect.top,
                        monitor_rect.right - monitor_rect.left,
                        monitor_rect.bottom - monitor_rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    )
                }
                .map_err(|e| BackendError::PlatformFailure {
                    context: format!("SetWindowPos: {e}"),
                })
            } else {
                let Some((rect, style)) = self.fullscreen_saved.take() else {
                    return Ok(());
                };
                // SAFETY: `hwnd` is `self.hwnd`; `style` is the exact
                // `isize` bitmask `GetWindowLongPtrW` returned when this
                // window entered fullscreen (saved above), so restoring
                // it verbatim is exactly what `SetWindowLongPtrW`
                // documents this call for.
                unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, style) };
                // SAFETY: `hwnd` is `self.hwnd`; only handles and plain
                // integers/flags cross this call.
                unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    )
                }
                .map_err(|e| BackendError::PlatformFailure {
                    context: format!("SetWindowPos: {e}"),
                })
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = fullscreen;
            Err(BackendError::Unsupported)
        }
    }

    /// `IsZoomed` — Win32's own maximize-state query, the
    /// same notion `ShowWindow(SW_MAXIMIZE)`/[`Self::restore`] flip.
    fn is_maximized(&self) -> ServiceResult<bool> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `IsZoomed` takes only a handle.
            Ok(unsafe { IsZoomed(hwnd) }.as_bool())
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    /// Whether the window is currently fullscreen. Win-GUI has no live OS
    /// query distinct from the toggle itself, so this answers from
    /// `Self::fullscreen_saved` — the same rect/style snapshot
    /// [`Self::set_fullscreen`] stashes on entry and clears on exit.
    /// Guards on `self.hwnd` the same way [`Self::is_maximized`] does, so
    /// an unattached backend gets `Unsupported` rather than a misleading
    /// `Ok(false)`.
    fn is_fullscreen(&self) -> ServiceResult<bool> {
        #[cfg(target_os = "windows")]
        {
            if self.hwnd.is_none() {
                return Err(BackendError::Unsupported);
            }
            Ok(self.fullscreen_saved.is_some())
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    /// `SetWindowPos(HWND_TOPMOST/HWND_NOTOPMOST)` — the one always-on-top
    /// mechanism among the four in-tree backends that actually works
    /// (contrast [`crate::gtk::backend::GtkBackend`]'s `Unsupported` on
    /// GTK4/Wayland).
    fn set_always_on_top(&mut self, on_top: bool) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            let insert_after = if on_top { HWND_TOPMOST } else { HWND_NOTOPMOST };
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `insert_after` is one of the two predefined
            // `HWND_TOPMOST`/`HWND_NOTOPMOST` sentinel values, not a
            // window handle that needs to be independently live.
            unsafe {
                SetWindowPos(
                    hwnd,
                    Some(insert_after),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            }
            .map_err(|e| BackendError::PlatformFailure {
                context: format!("SetWindowPos: {e}"),
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = on_top;
            Err(BackendError::Unsupported)
        }
    }

    /// Toggles the `WS_CAPTION` style bit — backs the client-side-
    /// decoration path. `WS_CAPTION` carries both the titlebar and
    /// (implicitly, per Win32's style-bit dependencies) the thin sizing
    /// border that comes with it, so clearing it removes the whole native
    /// non-client frame a host painting its own titlebar wants gone.
    /// `SWP_FRAMECHANGED` forces Windows to recompute the non-client area
    /// from the new style — same requirement `set_fullscreen`'s own
    /// `SetWindowPos` calls already document.
    fn set_decorated(&mut self, decorated: bool) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            // SAFETY: (every `unsafe` block in this arm) `hwnd` is
            // `self.hwnd`, this backend's own live window; `GetWindowLongPtrW`/
            // `SetWindowLongPtrW` exchange plain `isize` bitmasks, and
            // `SetWindowPos` below takes only handles and integers/flags.
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
            let new_style = if decorated {
                style | (WS_CAPTION.0 as isize)
            } else {
                style & !(WS_CAPTION.0 as isize)
            };
            // SAFETY: see this arm's opening comment above.
            unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, new_style) };
            // SAFETY: see this arm's opening comment above.
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                )
            }
            .map_err(|e| BackendError::PlatformFailure {
                context: format!("SetWindowPos: {e}"),
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = decorated;
            Err(BackendError::Unsupported)
        }
    }

    fn minimize(&mut self) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `ShowWindow` takes only a handle and a plain
            // show-command enum.
            unsafe {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            }
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    /// `ShowWindow(SW_RESTORE)` undoes both a minimize and a maximize —
    /// matching `GtkBackend::restore`'s "restores from either state"
    /// posture (see that method's doc for why).
    fn restore(&mut self) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `ShowWindow` takes only a handle and a plain
            // show-command enum.
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    fn hide(&mut self) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `ShowWindow` takes only a handle and a plain
            // show-command enum.
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    fn show(&mut self) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `ShowWindow` takes only a handle and a plain
            // show-command enum.
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }

    fn focus(&mut self) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            // SAFETY: `hwnd` is `self.hwnd`, this backend's own live
            // window; `SetForegroundWindow` takes only a handle.
            unsafe {
                let _ = SetForegroundWindow(hwnd);
            }
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(BackendError::Unsupported)
        }
    }
}

/// Win-GUI's `TrayService` surface, backed by
/// `Shell_NotifyIconW` — see [`super::tray`]'s module doc for the full
/// design (icon decode, the menu-vs-plain-click sequencing, why
/// `ContextMenuDismissed` fires here unlike macOS). Every method just
/// forwards to a free function in that module and a `self.hwnd`
/// guard, the same "thin impl on the struct, real logic in a sibling
/// module" shape [`WindowControl for WinBackend`](WindowControl) above
/// already uses for `super::tray`'s sibling gaps.
impl crate::backend::TrayService for WinBackend {
    fn set_icon(&mut self, icon: crate::primitives::image::ImageSource) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            super::tray::set_icon(&mut self.tray, hwnd, icon)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = icon;
            Err(BackendError::Unsupported)
        }
    }

    fn set_tooltip(&mut self, tooltip: &str) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            super::tray::set_tooltip(&mut self.tray, hwnd, tooltip)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = tooltip;
            Err(BackendError::Unsupported)
        }
    }

    fn set_menu(
        &mut self,
        menu: &crate::primitives::context_menu::ContextMenu,
    ) -> ServiceResult<()> {
        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd.ok_or(BackendError::Unsupported)?;
            super::tray::set_menu(&mut self.tray, hwnd, menu)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = menu;
            Err(BackendError::Unsupported)
        }
    }
}

impl WinBackend {
    /// The currently-attached tray menu, if any — read by `win::run`'s
    /// `WM_QUADRAUI_TRAY_CALLBACK` wndproc arm to decide whether a tray
    /// click should show a menu or dispatch `UiEvent::TrayClicked`. See
    /// [`super::tray::attached_menu`].
    #[cfg(target_os = "windows")]
    pub(crate) fn tray_menu(&self) -> Option<crate::primitives::context_menu::ContextMenu> {
        super::tray::attached_menu(&self.tray)
    }
}

/// UTF-16, NUL-terminated — the framing every `PCWSTR`-taking Win32
/// call in this backend needs. The one implementation shared crate-wide.
#[cfg(target_os = "windows")]
use crate::desktop::wide_nul_terminated as win_wide_nul_terminated;

/// The nearest monitor's full rect (not the work area — `set_fullscreen`
/// wants to cover the taskbar too) for the monitor `hwnd` is currently
/// on, via `MonitorFromWindow(MONITOR_DEFAULTTONEAREST)` +
/// `GetMonitorInfoW`. Shared by [`WindowControl::center`] (work area
/// would arguably be more correct there, but a single helper covering
/// both callers is a smaller diff than two, and centering a couple of
/// taskbar-heights off from work-area-perfect is a cosmetic gap, not a
/// correctness one) and [`WindowControl::set_fullscreen`].
#[cfg(target_os = "windows")]
fn win_monitor_rect(hwnd: HWND) -> ServiceResult<RECT> {
    // SAFETY: `hwnd` is the caller's live window; `MonitorFromWindow`
    // with `MONITOR_DEFAULTTONEAREST` never fails (it falls back to the
    // nearest monitor rather than erroring), so there is no failure mode
    // to guard against here beyond the type system already requiring a
    // live `HWND`.
    let hmonitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `hmonitor` came from `MonitorFromWindow` just above;
    // `info.cbSize` is set to `MONITORINFO`'s size immediately above —
    // the documented Win32 idiom for telling `GetMonitorInfoW` which
    // struct variant was passed (mirrors `services.rs::win_displays`'s
    // identical `MONITORINFOEXW` idiom).
    unsafe { GetMonitorInfoW(hmonitor, &mut info) }
        .ok()
        .map_err(|e| BackendError::PlatformFailure {
            context: format!("GetMonitorInfoW: {e}"),
        })?;
    Ok(info.rcMonitor)
}

// ─── PaintSurface ──────────────────────────────────────────────────────────
//
// The ~15-verb drawing surface underneath `Backend::draw_*`, extracted from
// helpers this backend already had: `super::text::fill_rect`/`stroke_rect`/
// `push_clip`/`pop_clip`/`draw_line` and the inherent `Self::measure_text`/
// `Self::draw_text` (both already `Rect`-taking). Every verb either forwards
// to the identically-named `Backend` method (frame lifecycle, measurement,
// viewport, image) or to the same free function/inherent method a real
// `draw_*` rasteriser would call. Same `#[cfg(target_os = "windows")]` split
// and no-op/nominal-estimate fallback as every other method on this backend.
// See the module doc's "Implementation notes" for why that keeps `cargo check
// --features win` meaningful on Linux. See `paint_surface`'s module doc for
// the full scope note and why these methods are `surface_`-prefixed instead of
// colliding with `Backend`'s.
//
// The default is chrome: the four text-shaped verbs resolve `chrome_dwrite`,
// falling back to the editor `dwrite` handle only when no live chrome handle
// exists yet — the same default `MacBackend`'s and `GtkBackend`'s own
// `PaintSurface` impls carry. [`EditorSurface`] (below) is the opt-out adapter
// the three editor-class call sites that reach this trait wrap `self` in
// instead, mirroring `MacBackend::EditorSurface`/`GtkBackend::EditorSurface`.
impl WinBackend {
    /// A [`super::surface::D2dSurface`] over the attached render target
    /// painting in `role`'s font: the editor handle for `Editor`, the
    /// chrome handle (else the editor one) for every chrome role. `None`
    /// before a surface is attached.
    #[cfg(target_os = "windows")]
    fn text_surface_for(&self, role: crate::FontRole) -> Option<super::surface::D2dSurface<'_>> {
        let surface = self.surface.as_ref()?;
        let dwrite = match role {
            crate::FontRole::Editor => self.dwrite.as_ref(),
            _ => self.chrome_dwrite.as_ref().or(self.dwrite.as_ref()),
        }?;
        Some(super::surface::D2dSurface {
            target: &surface.target,
            dwrite: Some(dwrite),
        })
    }
}

impl PaintSurface for WinBackend {
    /// Remembers the box so a later [`Backend::draw_focus_ring`] this
    /// frame keeps clear of it — see `overlay_bounds_this_frame`.
    fn surface_note_overlay(&mut self, rect: Rect) {
        self.overlay_bounds_this_frame.push(rect);
    }

    fn surface_begin_frame(&mut self, viewport: Viewport) {
        Backend::begin_frame(self, viewport);
    }

    /// Paints through [`EditorSurface`], this backend's editor-font twin.
    fn surface_with_editor_font(&mut self, paint: &mut dyn FnMut(&mut dyn PaintSurface)) -> bool {
        paint(&mut EditorSurface { backend: self });
        true
    }

    /// Paints through a [`super::surface::D2dSurface`] carrying a
    /// same-family, same-fallback clone of `chrome_dwrite` (else the
    /// editor `dwrite`) at `size_px` — DirectWrite sizes are points at
    /// the legacy 96/72 dpi ratio, so `size_px` is converted by
    /// `72 / 96`. Returns `false`, painting nothing, when no surface is
    /// attached yet.
    fn surface_with_icon_font(
        &mut self,
        size_px: f32,
        paint: &mut dyn FnMut(&mut dyn PaintSurface),
    ) -> bool {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            let dwrite = self.chrome_dwrite.as_ref().unwrap_or(dwrite);
            let icon_dwrite = dwrite.with_size(size_px * 72.0 / 96.0);
            let icon_dwrite = icon_dwrite.as_ref().unwrap_or(dwrite);
            paint(&mut super::surface::D2dSurface {
                target: &surface.target,
                dwrite: Some(icon_dwrite),
            });
            return true;
        }
        let _ = (size_px, paint);
        false
    }

    fn surface_end_frame(&mut self) {
        Backend::end_frame(self);
    }

    fn surface_viewport(&self) -> Viewport {
        Backend::viewport(self)
    }

    fn surface_line_height(&self) -> f32 {
        Backend::line_height(self)
    }

    fn surface_char_width(&self) -> f32 {
        Backend::char_width(self)
    }

    /// `self` is the surface every non-editor-class primitive
    /// measures/paints through unwrapped (`draw_toast_overlay`, …), so this
    /// resolves `chrome_dwrite` — falling back to the editor `dwrite`
    /// handle only when no live chrome handle exists yet, the same
    /// "degrade, don't panic" convention
    /// [`Self::surface_draw_text_run_with_role`] already documents —
    /// rather than `dwrite` unconditionally. See [`EditorSurface`]'s doc
    /// for the three editor-class primitives that opt back into the
    /// editor font via a dedicated adapter instead, and
    /// `crate::font_role`'s module doc for why a non-editor-class
    /// primitive must never resolve the editor font.
    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        #[cfg(target_os = "windows")]
        if let Some(d) = self.chrome_dwrite.as_ref().or(self.dwrite.as_ref()) {
            if let Ok(m) = d.measure_text(text) {
                return m;
            }
        }
        // No live DWrite handle (fresh backend, device lost, or a
        // non-Windows host where this never compiles) — a measurement
        // can't be faked meaningfully, but a wrong-but-finite nominal
        // per-character estimate beats a dead process. Real
        // widths resume once a surface exists.
        (
            nominal_text_width(text, self.current_char_width),
            self.current_line_height,
        )
    }

    /// Overrides the default (which drops `bold`) with
    /// `DWrite::measure_text_styled`, the bold-aware width the status
    /// bar painter measures segments with, in `chrome_dwrite` as
    /// [`Self::surface_measure_text`] does.
    fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
        #[cfg(not(target_os = "windows"))]
        let _ = bold;
        #[cfg(target_os = "windows")]
        if let Some(d) = self.chrome_dwrite.as_ref().or(self.dwrite.as_ref()) {
            if let Ok(m) = d.measure_text_styled(text, bold) {
                return m;
            }
        }
        // See `surface_measure_text`'s doc for why this nominal estimate
        // (rather than `(0.0, 0.0)`) is the chosen degrade.
        (
            nominal_text_width(text, self.current_char_width),
            self.current_line_height,
        )
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: crate::Color) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            let _ = super::text::fill_rect(&surface.target, rect, color);
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a
        // no-op instead of panicking.
        let _ = (rect, color);
    }

    /// `super::text::fill_rounded_rect` is the
    /// `FillRoundedRectangle` twin of `fill_rect` above — same
    /// no-surface degrade as every other verb here.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: crate::Color) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            let _ = super::text::fill_rounded_rect(&surface.target, rect, radius, color);
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a
        // no-op instead of panicking.
        let _ = (rect, radius, color);
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: crate::Color, stroke_width: f32) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            let _ = super::text::stroke_rect(&surface.target, rect, color, stroke_width);
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a
        // no-op instead of panicking.
        let _ = (rect, color, stroke_width);
    }

    /// `super::text::stroke_rounded_rect` is the
    /// `DrawRoundedRectangle` twin of `stroke_rect` above — same
    /// no-surface degrade as every other verb here.
    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: crate::Color,
        stroke_width: f32,
    ) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            let _ = super::text::stroke_rounded_rect(
                &surface.target,
                rect,
                radius,
                color,
                stroke_width,
            );
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a
        // no-op instead of panicking.
        let _ = (rect, radius, color, stroke_width);
    }

    /// See [`Self::surface_measure_text`]'s doc for why this resolves
    /// `chrome_dwrite` rather than the editor `dwrite` handle
    /// unconditionally.
    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: crate::Color) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (
            &self.surface,
            self.chrome_dwrite.as_ref().or(self.dwrite.as_ref()),
        ) {
            let _ = dwrite.draw_text(&surface.target, text, rect, color);
            return;
        }
        // No live surface/DWrite handle — degrades to a no-op rather
        // than panicking, matching every other verb on this impl.
        let _ = (rect, text, color);
    }

    /// Overrides the default (which drops styling) for `bold`
    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: crate::Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let _ = (italic, underline);
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (
            &self.surface,
            self.chrome_dwrite.as_ref().or(self.dwrite.as_ref()),
        ) {
            if (scale_x - 1.0).abs() > f32::EPSILON {
                super::text::with_horizontal_scale(&surface.target, scale_x, rect.x, || {
                    let _ = dwrite.draw_text_styled(&surface.target, text, rect, color, bold);
                });
            } else {
                let _ = dwrite.draw_text_styled(&surface.target, text, rect, color, bold);
            }
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a
        // no-op instead of panicking.
        let _ = (rect, text, color, bold, scale_x);
    }

    /// Overrides the default (which ignores `role`) to select `chrome_dwrite`
    fn surface_draw_text_run_with_role(
        &mut self,
        rect: Rect,
        text: &str,
        color: crate::Color,
        role: crate::FontRole,
        italic: bool,
    ) {
        let _ = italic;
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            // Chrome text falls back to the editor `dwrite` handle if no
            // live chrome one exists yet — same "degrade, don't panic"
            // convention as `WinBackend`'s "Before a surface is attached" doc describes.
            let dwrite = match role {
                crate::FontRole::Editor => self.dwrite.as_ref(),
                _ => self.chrome_dwrite.as_ref().or(self.dwrite.as_ref()),
            };
            if let Some(dwrite) = dwrite {
                let bold = role == crate::FontRole::ChromeBold;
                let _ = dwrite.draw_text_styled(&surface.target, text, rect, color, bold);
                return;
            }
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a
        // no-op instead of panicking.
        let _ = (rect, text, color, role);
    }

    /// Overrides the default only to document why it needn't
    /// change anything — `DWrite::new` already bakes
    /// [`Self::set_nerd_font_fallback`]'s `IDWriteFontFallback` into
    /// every `IDWriteTextFormat` it builds (editor and chrome alike) via
    /// `IDWriteTextFormat1::SetFontFallback`, so
    /// [`Self::surface_draw_text_run`] already resolves an icon glyph
    /// the same way this verb would.
    fn surface_draw_icon_glyph(&mut self, rect: Rect, text: &str, color: crate::Color) {
        self.surface_draw_text_run(rect, text, color)
    }

    fn surface_draw_text_line(
        &mut self,
        origin: Point,
        text: &str,
        runs: &[crate::TextRunStyle],
        role: crate::FontRole,
    ) {
        #[cfg(target_os = "windows")]
        if let Some(mut surface) = self.text_surface_for(role) {
            surface.surface_draw_text_line(origin, text, runs, role);
        }
        let _ = (origin, text, runs, role);
    }

    fn surface_text_x_at_index(
        &mut self,
        text: &str,
        runs: &[crate::TextRunStyle],
        role: crate::FontRole,
        byte_idx: usize,
    ) -> f32 {
        #[cfg(target_os = "windows")]
        if let Some(mut surface) = self.text_surface_for(role) {
            return surface.surface_text_x_at_index(text, runs, role, byte_idx);
        }
        let _ = role;
        crate::paint_surface::monospace_text_x(text, runs, byte_idx, Backend::char_width(self))
    }

    fn surface_draw_line(
        &mut self,
        from: Point,
        to: Point,
        color: crate::Color,
        stroke_width: f32,
    ) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            let _ = super::text::draw_line(
                &surface.target,
                from.x,
                from.y,
                to.x,
                to.y,
                color,
                stroke_width,
            );
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a
        // no-op instead of panicking.
        let _ = (from, to, color, stroke_width);
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            super::text::push_clip(&surface.target, rect);
            return;
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a
        // no-op instead of panicking.
        let _ = rect;
    }

    fn surface_pop_clip(&mut self) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            super::text::pop_clip(&surface.target);
        }
        // See `WinBackend`'s "Before a surface is attached" doc for why this degrades to a
        // no-op instead of panicking.
    }

    /// See the [`WinBackend`] "Before a surface is attached" doc for the "surface not attached yet"
    /// posture: no pixels to rasterise onto reports `Unsupported`. WIC
    /// decoding and `DrawBitmap` are pure Direct2D, no `self.dwrite`.
    fn surface_draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &mut self.surface {
            return super::surface::image::draw_image_cached(
                &surface.target,
                rect,
                image,
                &mut surface.images,
            );
        }
        let _ = (rect, image);
        crate::backend::ImagePaintResult::Unsupported
    }
}

/// The Win-GUI twin of `MacBackend::EditorSurface`/
/// `GtkBackend::EditorSurface` (see those types' own docs): it exists to
/// reach the editor `dwrite` handle from a primitive-generic call site, since `WinBackend`'s own
/// [`PaintSurface`] impl (above) now defaults to `chrome_dwrite` and a
/// handful of primitives still need the editor font. The terminal and
/// the plain text display paint through this trait and need
/// `EditorSurface`; the diff view also paints through this trait and
/// needs it too. The editor and the minimap select the editor font
/// through `FontRole::Editor` on this backend's own surface.
///
/// Every font-agnostic method (fills, strokes, clip, lines, images, frame
/// lifecycle) forwards straight through to [`WinBackend`]'s own impl,
/// which doesn't read the font either way. Line height / char width are
/// **not** overridden — row geometry stays seeded from the editor font's
/// own metrics, since `WinBackend` has no separate `chrome_line_height`/
/// `chrome_char_width` fields to diverge from it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct EditorSurface<'a> {
    backend: &'a mut WinBackend,
}

impl PaintSurface for EditorSurface<'_> {
    fn surface_begin_frame(&mut self, viewport: Viewport) {
        self.backend.surface_begin_frame(viewport)
    }

    fn surface_end_frame(&mut self) {
        self.backend.surface_end_frame()
    }

    fn surface_viewport(&self) -> Viewport {
        self.backend.surface_viewport()
    }

    fn surface_line_height(&self) -> f32 {
        self.backend.surface_line_height()
    }

    fn surface_char_width(&self) -> f32 {
        self.backend.surface_char_width()
    }

    /// Forces the editor `dwrite` handle explicitly rather than
    /// `WinBackend`'s own (now chrome-resolving) default — see this
    /// type's own doc.
    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        #[cfg(target_os = "windows")]
        if let Some(d) = self.backend.dwrite.as_ref() {
            if let Ok(m) = d.measure_text(text) {
                return m;
            }
        }
        (
            nominal_text_width(text, self.backend.current_char_width),
            self.backend.current_line_height,
        )
    }

    /// See [`Self::surface_measure_text`]'s doc for why this forces the
    /// editor `dwrite` handle explicitly.
    fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
        #[cfg(not(target_os = "windows"))]
        let _ = bold;
        #[cfg(target_os = "windows")]
        if let Some(d) = self.backend.dwrite.as_ref() {
            if let Ok(m) = d.measure_text_styled(text, bold) {
                return m;
            }
        }
        (
            nominal_text_width(text, self.backend.current_char_width),
            self.backend.current_line_height,
        )
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: crate::Color) {
        self.backend.surface_fill_rect(rect, color)
    }

    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: crate::Color) {
        self.backend.surface_fill_rounded_rect(rect, radius, color)
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: crate::Color, stroke_width: f32) {
        self.backend.surface_stroke_rect(rect, color, stroke_width)
    }

    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: crate::Color,
        stroke_width: f32,
    ) {
        self.backend
            .surface_stroke_rounded_rect(rect, radius, color, stroke_width)
    }

    /// See [`Self::surface_measure_text`]'s doc for why this forces the
    /// editor `dwrite` handle explicitly.
    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: crate::Color) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.backend.surface, self.backend.dwrite.as_ref())
        {
            let _ = dwrite.draw_text(&surface.target, text, rect, color);
            return;
        }
        // No live editor `dwrite` handle — degrades to a no-op rather
        // than panicking, matching every other verb on this adapter.
        let _ = (rect, text, color);
    }

    /// The editor-`dwrite` twin of `WinBackend`'s own (now
    /// chrome-resolving) override — same `italic`/`underline`-
    /// unsupported posture, same `scale_x` support via
    /// `super::text::with_horizontal_scale`, just forced onto the editor
    /// `dwrite` handle instead of `chrome_dwrite`.
    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: crate::Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let _ = (italic, underline);
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.backend.surface, self.backend.dwrite.as_ref())
        {
            if (scale_x - 1.0).abs() > f32::EPSILON {
                super::text::with_horizontal_scale(&surface.target, scale_x, rect.x, || {
                    let _ = dwrite.draw_text_styled(&surface.target, text, rect, color, bold);
                });
            } else {
                let _ = dwrite.draw_text_styled(&surface.target, text, rect, color, bold);
            }
            return;
        }
        // No live editor `dwrite` handle — degrades to a no-op rather
        // than panicking, matching every other verb on this adapter.
        let _ = (rect, text, color, bold, scale_x);
    }

    fn surface_draw_text_line(
        &mut self,
        origin: Point,
        text: &str,
        runs: &[crate::TextRunStyle],
        role: crate::FontRole,
    ) {
        #[cfg(target_os = "windows")]
        if let Some(mut surface) = self.backend.text_surface_for(crate::FontRole::Editor) {
            surface.surface_draw_text_line(origin, text, runs, role);
        }
        let _ = (origin, text, runs, role);
    }

    fn surface_text_x_at_index(
        &mut self,
        text: &str,
        runs: &[crate::TextRunStyle],
        role: crate::FontRole,
        byte_idx: usize,
    ) -> f32 {
        #[cfg(target_os = "windows")]
        if let Some(mut surface) = self.backend.text_surface_for(crate::FontRole::Editor) {
            return surface.surface_text_x_at_index(text, runs, role, byte_idx);
        }
        let _ = role;
        crate::paint_surface::monospace_text_x(
            text,
            runs,
            byte_idx,
            Backend::char_width(&*self.backend),
        )
    }

    fn surface_draw_line(
        &mut self,
        from: Point,
        to: Point,
        color: crate::Color,
        stroke_width: f32,
    ) {
        self.backend
            .surface_draw_line(from, to, color, stroke_width)
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        self.backend.surface_push_clip(rect)
    }

    fn surface_pop_clip(&mut self) {
        self.backend.surface_pop_clip()
    }

    fn surface_draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        self.backend.surface_draw_image(rect, image)
    }
}

#[cfg(test)]
mod tests {
    // Tests exercise the deprecated row-family `Backend::draw_*`/`*_layout`
    // methods directly, alongside their `DrawsPrimitives::draw_primitive`
    // replacements — this deprecation targets callers, not
    // this file's own coverage of the methods it deprecates.
    #![allow(deprecated)]
    use super::*;

    /// Build a flat tree of `n_rows` leaf rows for `TreeView::vscrollbar`
    /// integration tests.
    fn flat_tree(n_rows: usize) -> crate::TreeView {
        crate::TreeView {
            id: WidgetId::new("test:tree:vscrollbar"),
            rows: (0..n_rows)
                .map(|i| crate::primitives::tree::TreeRow {
                    path: vec![i as u16],
                    indent: 0,
                    icon: None,
                    text: crate::types::StyledText::plain(format!("row{i}")),
                    badge: None,
                    is_expanded: None,
                    decoration: crate::types::Decoration::Normal,
                    edit: None,
                })
                .collect(),
            selection_mode: crate::types::SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: crate::types::TreeStyle::default(),
            has_focus: false,
        }
    }

    /// Pre-attach fallback: before a surface/DirectWrite
    /// handle exists (`dwrite`/`chrome_dwrite` both `None`, the state any
    /// fresh `WinBackend` starts in, and the only state this crate's
    /// non-Windows CI leg ever exercises for this method — see
    /// `Backend::measure_text`'s doc), `measure_text` degrades to a
    /// `current_char_width`-scaled estimate, matching every other
    /// pre-attach degrade on this backend. Cross-platform: no
    /// `target_os = "windows"` gate needed, same rationale as
    /// `win_backend_tree_vscrollbar_uses_tree_layout_row_pitch_not_raw_line_height`
    /// above.
    #[test]
    fn win_backend_measure_text_falls_back_to_char_width_before_a_surface_attaches() {
        let mut backend = WinBackend::new();
        backend.current_char_width = 9.0;
        backend.current_line_height = 18.0;

        let (w, h) = Backend::measure_text(&backend, "abcd", crate::FontRole::Chrome);
        assert_eq!(w, 4.0 * 9.0);
        assert_eq!(h, 18.0);
    }

    /// Once a headless surface attaches, `measure_text` must
    /// resolve a genuinely different `IDWriteTextFormat` per
    /// [`crate::FontRole`] — `chrome_dwrite` for
    /// [`crate::FontRole::Chrome`], `dwrite` for
    /// [`crate::FontRole::Editor`] — mirroring
    /// `measure_text_resolves_a_different_font_per_role` in
    /// `macos::backend`'s own test module. Windows-only: DirectWrite
    /// isn't available to attach a real surface against on the ubuntu
    /// `cargo check --features win` leg.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_measure_text_resolves_a_different_font_per_role() {
        use crate::win::testing::HeadlessSurface;

        let surface = HeadlessSurface::new(100, 100).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend.set_editor_font(DEFAULT_EDITOR_FONT_FAMILY, 10.0);
        backend.set_ui_font(&format!("{DEFAULT_UI_FONT_FAMILY} 40"));
        backend
            .attach_headless(surface.target().clone(), 100, 100)
            .expect("attach headless surface");

        let (chrome_w, _) = Backend::measure_text(&backend, "quadraui", crate::FontRole::Chrome);
        let (editor_w, _) = Backend::measure_text(&backend, "quadraui", crate::FontRole::Editor);

        assert!(
            chrome_w > editor_w * 2.0,
            "a 40pt chrome font should measure \"quadraui\" much wider than a \
             10pt editor font: chrome_w={chrome_w}, editor_w={editor_w}"
        );
    }

    /// Repainting the same frame twice must
    /// not create any new `ID2D1SolidColorBrush`es, and painting the
    /// first frame must create no more brushes than the number of
    /// distinct colors it actually drew. Before `super::text`'s brush
    /// cache, every `surface_fill_rect` call below (a stand-in for a
    /// chrome rasteriser's per-primitive fill/stroke/text calls, all of
    /// which go through the same `get_or_create_brush` choke point) would
    /// have created and dropped its own brush, so this would have failed
    /// with `created_frame_1 == 12` (one per fill call) and
    /// `created_frame_2 == 12` again — the exact "brushes scale with
    /// primitive count, not color count" problem this issue measured at
    /// 647 brushes/frame on a real 1639x1079 window.
    #[cfg(target_os = "windows")]
    #[test]
    fn repaint_reuses_cached_brushes_creating_no_new_ones() {
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 64;
        const H: u32 = 64;
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        // Three distinct colors, each painted four times — more
        // primitives than colors, mirroring a real chrome rasteriser
        // painting many same-colored fills/strokes/text runs per frame.
        let colors = [
            crate::Color::rgb(10, 20, 30),
            crate::Color::rgb(200, 50, 50),
            crate::Color::rgb(0, 128, 255),
        ];
        const FILLS_PER_COLOR: usize = 4;

        let paint_frame = |backend: &mut WinBackend| {
            backend.surface_begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            for (i, &color) in colors.iter().enumerate() {
                for j in 0..FILLS_PER_COLOR {
                    let x = (i * FILLS_PER_COLOR + j) as f32 * 2.0;
                    backend.surface_fill_rect(Rect::new(x, 0.0, 2.0, 2.0), color);
                }
            }
            backend.surface_end_frame();
        };

        crate::win::text::reset_brush_creation_count();

        paint_frame(&mut backend);
        let created_frame_1 = crate::win::text::brush_creation_count();
        assert!(
            created_frame_1 > 0 && created_frame_1 as usize <= colors.len(),
            "painting {} distinct colors ({} fills total) should create at most \
             {} brushes (one per distinct color), got {created_frame_1}",
            colors.len(),
            colors.len() * FILLS_PER_COLOR,
            colors.len()
        );

        paint_frame(&mut backend);
        let created_frame_2 = crate::win::text::brush_creation_count() - created_frame_1;
        assert_eq!(
            created_frame_2,
            0,
            "repainting the exact same colors in frame 2 must create zero new brushes \
             (cache miss count went from {created_frame_1} to \
             {})",
            crate::win::text::brush_creation_count()
        );

        // Render-target recreate (the `EndDraw`-failure path): the new
        // surface starts with an empty cache, so the next frame re-creates
        // exactly one brush per color, and the frame after that none.
        backend.drop_surface_for_testing();
        backend.ensure_surface().expect("recreate headless surface");
        crate::win::text::reset_brush_creation_count();
        paint_frame(&mut backend);
        assert_eq!(
            crate::win::text::brush_creation_count() as usize,
            colors.len(),
            "a recreated render target must not reuse the old target's brushes"
        );
        paint_frame(&mut backend);
        assert_eq!(
            crate::win::text::brush_creation_count() as usize,
            colors.len(),
            "repainting after a recreate must hit the new surface's cache"
        );

        // Outside a backend frame no cache is active, so helpers painting
        // straight onto the target create one-off brushes, as before.
        crate::win::text::reset_brush_creation_count();
        surface
            .paint(|target| {
                crate::win::text::fill_rect(target, Rect::new(0.0, 0.0, 2.0, 2.0), colors[0])
                    .expect("fill_rect");
                crate::win::text::fill_rect(target, Rect::new(2.0, 0.0, 2.0, 2.0), colors[0])
                    .expect("fill_rect");
            })
            .expect("headless paint");
        assert_eq!(crate::win::text::brush_creation_count(), 2);
    }

    /// `default_fonts()` reports Win-GUI's VS-Code-alignment convention
    /// (`Consolas 14`/`Segoe UI` at `DEFAULT_UI_FONT_SIZE_PT`, which renders
    /// at VS Code's 13 DIP chrome size) — cross-platform, no `target_os =
    /// "windows"` gate needed, same rationale as
    /// `win_backend_tree_vscrollbar_uses_tree_layout_row_pitch_not_raw_line_height` above.
    #[test]
    fn win_backend_default_fonts_reports_platform_convention() {
        let backend = WinBackend::new();
        let defaults = Backend::default_fonts(&backend);
        assert_eq!(defaults.editor_family, "Consolas");
        assert_eq!(defaults.editor_size_pt, 14.0);
        assert_eq!(defaults.ui_family, "Segoe UI");
        assert_eq!(defaults.ui_size_pt, DEFAULT_UI_FONT_SIZE_PT);
    }

    /// A prior `set_editor_font`/`set_ui_font` call must not perturb
    /// `default_fonts()` — it reports a static platform fact, not this
    /// instance's current, possibly-overridden state (see
    /// `PlatformFontDefaults`'s doc). Cross-platform: `set_editor_font`/
    /// `set_ui_font` are real, stateful implementations here (unlike
    /// TUI's no-ops), so this is worth checking even off `target_os =
    /// "windows"`, same rationale as
    /// `win_backend_default_fonts_reports_platform_convention` above.
    #[test]
    fn win_backend_default_fonts_is_unaffected_by_set_editor_font_and_set_ui_font() {
        let mut backend = WinBackend::new();
        Backend::set_editor_font(&mut backend, "Fira Code", 40.0);
        Backend::set_ui_font(&mut backend, "Cantarell 30");
        let defaults = Backend::default_fonts(&backend);
        assert_eq!(defaults.editor_family, "Consolas");
        assert_eq!(defaults.editor_size_pt, 14.0);
        assert_eq!(defaults.ui_family, "Segoe UI");
        assert_eq!(defaults.ui_size_pt, DEFAULT_UI_FONT_SIZE_PT);
    }

    /// The chrome `ui_font_size_pt` [`WinBackend::new`] installs before any
    /// [`Backend::set_ui_font`] call must carry the exact point size
    /// [`Backend::default_fonts`] advertises as `ui_size_pt` — a consumer
    /// that renders chrome straight off a fresh backend (never calling
    /// `set_ui_font` itself) still gets the size it would see by reading
    /// `default_fonts()`, rather than a smaller, un-advertised runtime
    /// default. And that shared point size must actually render at VS
    /// Code's 13 DIP Windows chrome text size once converted through
    /// `pt_to_dip` — not just agree with itself in points while still
    /// landing on the wrong pixel size.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_installed_ui_font_size_matches_default_fonts_and_renders_at_13_dip() {
        let backend = WinBackend::new();
        let advertised = Backend::default_fonts(&backend).ui_size_pt;
        assert_eq!(
            backend.ui_font_size_pt, advertised,
            "WinBackend::new's installed ui_font_size_pt must match \
             default_fonts().ui_size_pt"
        );
        assert_eq!(
            DEFAULT_UI_FONT_SIZE_PT, advertised,
            "DEFAULT_UI_FONT_SIZE_PT must stay in lockstep with default_fonts()'s \
             ui_size_pt so the two cannot drift again"
        );

        let rendered_dip = super::super::msg::pt_to_dip(advertised);
        assert!(
            (rendered_dip - 13.0).abs() < 0.5,
            "the installed chrome font must render at VS Code's 13 DIP Windows chrome \
             size (±0.5), got {rendered_dip} DIP from {advertised}pt"
        );
    }

    /// `list_vscrollbar`'s track width must follow a host-set
    /// `Style::scrollbar_width`, the same live field
    /// `native_surface_paint::paint` reads — so the painted track and
    /// the hit-tested track can never drift apart, at the default width
    /// or a host override. Pure geometry, no `target_os = "windows"`
    /// gate needed.
    #[test]
    fn win_backend_list_vscrollbar_track_width_follows_style_token() {
        let mut backend = WinBackend::new();
        let list = ListView {
            id: WidgetId::new("l"),
            title: None,
            items: (0..30)
                .map(|i| crate::ListItem {
                    text: crate::types::StyledText::plain(format!("row {i}")),
                    detail: None,
                    icon: None,
                    decoration: crate::types::Decoration::default(),
                })
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: true,
            striped: false,
        };
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);

        let default_sb = Backend::list_vscrollbar(&backend, rect, &list)
            .expect("30 rows must overflow a 100px viewport");
        assert_eq!(
            default_sb.track.width,
            backend.current_style().scrollbar_width
        );

        backend.set_style(crate::Style::default().with_scrollbar_width(24.0));
        let overridden_sb = Backend::list_vscrollbar(&backend, rect, &list)
            .expect("30 rows must overflow a 100px viewport");
        assert_eq!(overridden_sb.track.width, 24.0);
    }

    /// `parse_ui_font_desc` splits a Pango-style comma-separated fallback list
    /// and takes the first candidate — generic or concrete — instead of treating
    /// the whole list as one unmatchable literal family name.
    #[cfg(target_os = "windows")]
    #[test]
    fn parse_ui_font_desc_splits_a_comma_separated_fallback_list() {
        assert_eq!(
            parse_ui_font_desc("Segoe UI, Tahoma, Sans 12"),
            ("Segoe UI".to_string(), 12.0)
        );
        // No trailing size at all — the real-world shape this issue was
        // filed over (vimcode's `UI_FONT_FAMILY` constants carry no
        // size, only a comma-separated family list).
        assert_eq!(
            parse_ui_font_desc("Segoe UI, Tahoma, Sans"),
            ("Segoe UI".to_string(), DEFAULT_UI_FONT_SIZE_PT)
        );
    }

    /// A CSS/Pango generic token resolves to its DirectWrite default
    /// (`DEFAULT_EDITOR_FONT_FAMILY`/`DEFAULT_UI_FONT_FAMILY`) rather than being
    /// treated as a literal, unmatchable family name — including when it's the
    /// first entry of a comma-separated list.
    #[cfg(target_os = "windows")]
    #[test]
    fn parse_ui_font_desc_resolves_generic_tokens() {
        assert_eq!(
            parse_ui_font_desc("monospace 13"),
            (DEFAULT_EDITOR_FONT_FAMILY.to_string(), 13.0)
        );
        assert_eq!(
            parse_ui_font_desc("sans-serif 13"),
            (DEFAULT_UI_FONT_FAMILY.to_string(), 13.0)
        );
        assert_eq!(
            parse_ui_font_desc("system-ui 13"),
            (DEFAULT_UI_FONT_FAMILY.to_string(), 13.0)
        );
        // Pango's own alias spellings resolve the same way.
        assert_eq!(
            parse_ui_font_desc("Monospace, Sans 13"),
            (DEFAULT_EDITOR_FONT_FAMILY.to_string(), 13.0)
        );
    }

    /// `set_editor_font("monospace", …)` resolves through the same
    /// `GenericFamily` path as `parse_ui_font_desc` —
    /// `editor_font_family` must land on `DEFAULT_EDITOR_FONT_FAMILY`,
    /// not the literal string `"monospace"`.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_editor_font_resolves_the_monospace_generic_token() {
        let mut b = WinBackend::new();
        Backend::set_editor_font(&mut b, "monospace", 14.0);
        assert_eq!(b.editor_font_family, DEFAULT_EDITOR_FONT_FAMILY);
        assert_eq!(b.editor_font_size_pt, 14.0);
    }

    /// `WinBackend::modal_stack_handle` must hand back a
    /// handle that shares state with the backend's own modal stack —
    /// same guarantee already proved for `GtkBackend`
    /// (`gtk::backend::tests::gtk_backend_modal_stack_handle_shares_state`),
    /// `TuiBackend`
    /// (`tui::backend::tests::tui_backend_modal_stack_handle_shares_state`),
    /// and `MacBackend`
    /// (`macos::backend::tests::mac_backend_modal_stack_handle_shares_state`).
    /// Pure-Rust `Rc<RefCell<>>` plumbing, so this runs on every host —
    /// no `target_os = "windows"` gate needed, unlike most of this file.
    #[test]
    fn win_backend_modal_stack_handle_shares_state() {
        let backend = WinBackend::new();
        let h1 = backend.modal_stack_handle();
        let h2 = backend.modal_stack_handle();
        h1.borrow_mut()
            .push(WidgetId::new("test:popup"), Rect::new(0.0, 0.0, 10.0, 5.0));
        assert_eq!(h2.borrow().len(), 1);
    }

    /// `is_maximized`/`set_decorated` guard on the (target-gated)
    /// `hwnd` field the same way every other `WindowControl` method here
    /// does. `WinBackend::new()` never sets `hwnd`, on Windows or off, so
    /// both must report `Unsupported` rather than panic — no
    /// `target_os = "windows"` gate needed, same reasoning as
    /// `win_backend_modal_stack_handle_shares_state` above.
    #[test]
    fn win_backend_is_maximized_err_without_window() {
        let b = WinBackend::new();
        assert!(matches!(
            WindowControl::is_maximized(&b),
            Err(BackendError::Unsupported)
        ));
    }

    #[test]
    fn win_backend_set_decorated_err_without_window() {
        let mut b = WinBackend::new();
        assert!(matches!(
            WindowControl::set_decorated(&mut b, false),
            Err(BackendError::Unsupported)
        ));
    }

    /// Pins the Windows column of the shared `hscrollbar_track` fix
    /// as the reference behaviour macOS's copy had silently dropped —
    /// `list_hscrollbar` insets the track by one char width on each side
    /// (and moves it up one row) for a `bordered` list, so the thumb sits
    /// inside the border rather than overpainting it. Pure geometry, no
    /// `target_os = "windows"` gate needed.
    #[test]
    fn win_backend_list_hscrollbar_insets_track_for_bordered_list() {
        let mut b = WinBackend::new();
        b.set_current_char_width(8.0);
        b.set_current_line_height(16.0);
        // The hscrollbar's row pitch now follows `draw_list`'s own
        // (`resolved_row_height`, which defaults to `Style::row_height`
        // on a pixel backend) — not the raw `current_line_height` set
        // above, which this test keeps only for the char-width-based
        // inset math on `track.x`/`track.width`.
        let row_h = b.current_style().row_height;
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let list = |bordered: bool| -> ListView {
            ListView {
                id: WidgetId::new("l"),
                title: None,
                items: vec![crate::ListItem {
                    text: crate::types::StyledText::plain(&"x".repeat(40)),
                    detail: None,
                    icon: None,
                    decoration: crate::types::Decoration::default(),
                }],
                selected_idx: 0,
                scroll_offset: 0,
                has_focus: true,
                bordered,
                h_scroll: 0,
                max_content_width: Some(40),
                show_v_scrollbar: false,
                striped: false,
            }
        };

        let flat_sb = b
            .list_hscrollbar(rect, &list(false))
            .expect("overflowing content should yield a scrollbar");
        assert_eq!(flat_sb.track.x, 0.0);
        assert_eq!(flat_sb.track.width, 200.0);

        let bordered_sb = b
            .list_hscrollbar(rect, &list(true))
            .expect("overflowing content should yield a scrollbar");
        assert_eq!(bordered_sb.track.x, 8.0);
        assert_eq!(bordered_sb.track.width, 184.0);
        assert_eq!(bordered_sb.track.y, 100.0 - 2.0 * row_h);
        assert!(bordered_sb.track.x >= rect.x + 8.0);
        assert!(bordered_sb.track.x + bordered_sb.track.width <= rect.x + rect.width - 8.0);
    }

    /// `Backend::set_style` overriding `row_height` changes the row
    /// pitch `list_layout` resolves — the acceptance bar for this chrome
    /// metric, mirrored from `MacBackend`'s equivalent test.
    #[test]
    fn win_backend_list_layout_row_pitch_follows_a_set_style_override() {
        let mut b = WinBackend::new();
        b.set_current_line_height(10.0);
        let list = ListView {
            id: WidgetId::new("l"),
            title: None,
            items: vec![crate::ListItem {
                text: crate::types::StyledText::plain("row"),
                detail: None,
                icon: None,
                decoration: crate::types::Decoration::default(),
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);

        let default_layout = Backend::list_layout(&b, rect, &list);
        assert_eq!(
            default_layout.visible_items[0].bounds.height,
            b.current_style().row_height
        );

        b.set_style(crate::Style::default().with_row_height(30.0));
        let overridden_layout = Backend::list_layout(&b, rect, &list);
        assert_eq!(overridden_layout.visible_items[0].bounds.height, 30.0);
    }

    /// The stash-then-reuse pattern this issue exists to
    /// unblock — obtain the handle through `&mut dyn Backend`, drop
    /// that borrow, and use the handle afterwards from an unrelated
    /// borrow scope.
    #[test]
    fn modal_stack_handle_outlives_the_backend_borrow_through_the_trait() {
        let mut backend = WinBackend::new();
        let stack_rc = {
            let dyn_backend: &mut dyn Backend = &mut backend;
            dyn_backend.modal_stack_handle()
        };
        stack_rc
            .borrow_mut()
            .push(WidgetId::new("test:popup"), Rect::new(0.0, 0.0, 10.0, 5.0));
        assert_eq!(backend.modal_stack_handle().borrow().len(), 1);
    }

    /// Same shared-state guarantee as
    /// `win_backend_modal_stack_handle_shares_state`, for
    /// `drag_state_handle`.
    #[test]
    fn win_backend_drag_state_handle_shares_state() {
        let backend = WinBackend::new();
        let h1 = backend.drag_state_handle();
        let h2 = backend.drag_state_handle();
        h1.borrow_mut()
            .begin(crate::dispatch::DragTarget::TextSelection {
                region: WidgetId::new("r"),
                anchor: crate::event::Point::new(0.0, 0.0),
            });
        assert!(h2.borrow().is_active());
    }

    // ── Accelerator matching + ActivityBar keyboard focus ──────────────
    //
    // Pure `HashMap`/`Vec`/`Option<WidgetId>` logic, no Direct2D — runs
    // on every host, same as the handle-sharing tests above. Mirrors
    // `macos::backend::tests`' `match_keypress_*` suite (Ctrl instead of
    // Cmd, since Win-GUI has no macOS-style universal-binding modifier
    // translation).

    fn acc(id: &str, key: &str) -> Accelerator {
        Accelerator {
            id: AcceleratorId::new(id),
            binding: crate::KeyBinding::Literal(key.to_string()),
            scope: AcceleratorScope::Global,
            label: None,
        }
    }

    /// The registry itself now lives in `BackendCore` — its
    /// `contains_key`/`len()` aren't reachable from here anymore, so this
    /// observes the same round trip behaviourally via `match_keypress`.
    #[test]
    fn register_and_unregister_accelerator_round_trip() {
        let mut b = WinBackend::new();
        let a = acc("save", "<C-s>");
        b.register_accelerator(&a);
        let ctrl_s = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            b.match_keypress(&Key::Char('s'), ctrl_s),
            Some(AcceleratorId::new("save"))
        );
        b.unregister_accelerator(&AcceleratorId::new("save"));
        assert_eq!(b.match_keypress(&Key::Char('s'), ctrl_s), None);
    }

    #[test]
    fn match_keypress_finds_registered_global_binding() {
        let mut b = WinBackend::new();
        b.register_accelerator(&acc("save", "<C-s>"));
        let id = b.match_keypress(
            &Key::Char('s'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert_eq!(id, Some(AcceleratorId::new("save")));
    }

    #[test]
    fn match_keypress_modifier_mismatch_no_match() {
        let mut b = WinBackend::new();
        b.register_accelerator(&acc("save", "<C-s>"));
        // Same key, wrong modifiers (Alt instead of Ctrl).
        let id = b.match_keypress(
            &Key::Char('s'),
            Modifiers {
                alt: true,
                ..Default::default()
            },
        );
        assert_eq!(id, None);
    }

    #[test]
    fn match_keypress_skips_non_global_scope() {
        let mut b = WinBackend::new();
        b.register_accelerator(&Accelerator {
            id: AcceleratorId::new("find-in-tree"),
            binding: crate::KeyBinding::Literal("<C-f>".to_string()),
            scope: AcceleratorScope::Widget(WidgetId::new("tree")),
            label: None,
        });
        let id = b.match_keypress(
            &Key::Char('f'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert_eq!(id, None);
    }

    #[test]
    fn match_keypress_re_register_replaces_binding() {
        let mut b = WinBackend::new();
        b.register_accelerator(&acc("save", "<C-s>"));
        b.register_accelerator(&acc("save", "<C-S-s>"));
        assert_eq!(
            b.match_keypress(
                &Key::Char('s'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                }
            ),
            None,
            "old binding must no longer match after re-registration",
        );
        assert_eq!(
            b.match_keypress(
                &Key::Char('s'),
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(AcceleratorId::new("save")),
        );
    }

    // ── Double-click folding ────────────────────────────────────────────
    //
    // Pure `DoubleClickDetector` logic, no Direct2D — runs on every host.
    // Mirrors `macos::backend::tests`' `fold_double_click_*` suite.

    fn win_mouse_down(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: crate::event::Point::new(x, y),
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn fold_double_click_second_click_same_position_becomes_double_click() {
        let mut b = WinBackend::new();
        let first = b.fold_double_click(win_mouse_down(5.0, 3.0));
        assert!(matches!(first, UiEvent::MouseDown { .. }));

        let second = b.fold_double_click(win_mouse_down(5.0, 3.0));
        assert!(
            matches!(second, UiEvent::DoubleClick { .. }),
            "second click at the same position should fold to DoubleClick"
        );
    }

    #[test]
    fn fold_double_click_different_position_stays_mouse_down() {
        let mut b = WinBackend::new();
        let _ = b.fold_double_click(win_mouse_down(5.0, 3.0));
        let second = b.fold_double_click(win_mouse_down(50.0, 30.0));
        assert!(matches!(second, UiEvent::MouseDown { .. }));
    }

    #[test]
    fn fold_double_click_passes_non_mouse_down_events_through() {
        let mut b = WinBackend::new();
        let ev = b.fold_double_click(UiEvent::WindowFocused(true));
        assert_eq!(ev, UiEvent::WindowFocused(true));
    }

    /// `PreprocessBackend::widget_routing_mut` returns `Some`, wiring
    /// `WinBackend` onto `runtime::preprocess_event`'s widget-contract
    /// routing step — and the returned state is the same `BackendCore`
    /// instance across calls, not a fresh one each time.
    #[test]
    fn widget_routing_mut_is_wired_and_stable_across_calls() {
        use crate::runtime::PreprocessBackend;

        let mut b = WinBackend::new();
        let routing = b
            .widget_routing_mut()
            .expect("WinBackend must opt into widget-contract routing");
        routing.hovered = Some(WidgetId::new("hovered"));

        assert_eq!(
            b.widget_routing_mut().unwrap().hovered,
            Some(WidgetId::new("hovered"))
        );
    }

    // ── Event queue adapter ──────────────────────────────────────────────
    //
    // Before this issue, `WinBackend::poll_events`/`wait_events` were
    // unconditional `todo!()` — calling either panicked immediately,
    // which is exactly what every test in this section would have hit
    // (`RED`) before the queue-adapter implementation below landed.
    // Pure `VecDeque`/`HashMap` logic, no Direct2D — runs on every host,
    // mirrors `macos::backend::tests::poll_events_drains_queue_fifo` and
    // the `docs/BACKEND.md` "option A event queue" contract `GtkBackend`
    // ships.

    #[test]
    fn poll_events_drains_queue_fifo() {
        let b = WinBackend::new();
        b.push_event(win_mouse_down(1.0, 2.0));
        b.push_event(UiEvent::WindowFocused(true));
        // `poll_events` takes `&mut self`, so re-acquire after the
        // (shared-ref) `push_event` calls above — mirrors
        // `MacBackend`'s identical test structure.
        let mut b = b;
        let evs = b.poll_events();
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0], UiEvent::MouseDown { .. }));
        assert!(matches!(evs[1], UiEvent::WindowFocused(true)));
        // Second drain yields nothing — draining doesn't panic on an
        // empty queue either.
        assert!(b.poll_events().is_empty());
    }

    // ── Background-thread wake ──────────────────────────────────────────
    //
    // `WinBackend::waker`'s `PostMessageW` half only exists under
    // `cfg(target_os = "windows")` and needs a real `HWND`/message loop to
    // exercise for real (that's `ci.yml`'s windows-latest leg's job, via
    // `wndproc`'s `WM_QUADRAUI_USER_EVENT` arm). What's host-independent
    // and testable here is the `UserEventQueue` plumbing every platform's
    // `waker()` shares — `self.hwnd` is `None` on this host regardless
    // (never attached to a real window), which is exactly the "no live
    // window to nudge yet" branch `waker`'s doc describes.

    /// A payload pushed from a background thread through the closure
    /// `Backend::waker()` hands out lands in `poll_events()`'s output as a
    /// `UiEvent::User`, even with no `HWND` attached (`self.hwnd == None`
    /// on every host this test runs on) — the queue half of `waker()`
    /// doesn't depend on a live window at all.
    #[test]
    fn waker_payload_reaches_poll_events_as_user_event() {
        let backend = WinBackend::new();
        let waker = Backend::waker(&backend);

        let handle = std::thread::spawn(move || {
            waker(crate::UserPayload::new("from background".to_string()));
        });
        handle.join().expect("background thread must not panic");

        let mut backend = backend;
        let events = backend.poll_events();
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::User(payload) => {
                assert_eq!(
                    payload.downcast_ref::<String>().map(String::as_str),
                    Some("from background")
                );
            }
            other => panic!("expected UiEvent::User, got {other:?}"),
        }
    }

    /// `request_frame_in` before any window is attached
    /// (`self.hwnd == None` on every host this test runs on, including a
    /// real Windows one that just hasn't called `attach_surface` yet)
    /// must be a safe no-op, not a panic — mirrors `waker`'s "no wake
    /// target yet" posture. There is nothing further to assert on a
    /// non-Windows host: no `SetTimer` call is reachable at all here (see
    /// `Backend::request_frame_in`'s doc for the real one-shot-timer
    /// behavior, exercised for real only on `windows-latest`).
    #[test]
    fn request_frame_in_before_a_window_is_attached_does_not_panic() {
        let backend = WinBackend::new();
        Backend::request_frame_in(&backend, Duration::from_millis(50));
    }

    /// `frame_requests`/`pending_frame_delay` must observe
    /// every `request_frame_in` call even with no `HWND` attached — the
    /// whole point of this bookkeeping existing alongside the real
    /// `SetTimer` arm (which stays silently no-op in this exact state,
    /// per the test above). Mirrors `TuiBackend`'s identically-shaped
    /// `frame_requests`/`pending_frame_delay` unit coverage.
    #[test]
    fn frame_requests_and_pending_delay_observe_calls_with_no_hwnd() {
        let backend = WinBackend::new();
        assert_eq!(backend.frame_requests(), 0);
        assert!(backend.pending_frame_delay().is_none());

        Backend::request_frame_in(&backend, Duration::from_millis(50));
        assert_eq!(backend.frame_requests(), 1);
        assert!(backend.pending_frame_delay().is_some());

        // A second call at the same interval coalesces to the earlier
        // deadline but still increments the call count — the
        // chained-re-arm behaviour `TuiBackend`'s own test documents.
        Backend::request_frame_in(&backend, Duration::from_millis(50));
        assert_eq!(backend.frame_requests(), 2);
    }

    /// A closure obtained before `attach_surface` ever runs must notice a
    /// window attaching later. `attach_surface` itself needs a real `HWND`/D2D
    /// device and can't run on this host, so this test drives `hwnd_raw` (the
    /// live mirror `attach_surface` writes into) and `live_hwnd` (the read
    /// `waker`'s closure performs on every call) directly — the same two pieces
    /// of state the closure actually touches, without needing `PostMessageW` to
    /// succeed against a real window. Windows-only: `hwnd_raw`/`live_hwnd` don't
    /// exist on other hosts (see `hwnd_raw`'s field doc).
    #[cfg(target_os = "windows")]
    #[test]
    fn waker_resolves_hwnd_set_after_the_waker_was_obtained() {
        let backend = WinBackend::new();

        // Simulates: `waker()` called during `AppLogic::setup`, before
        // `attach_surface` has ever run.
        assert!(
            live_hwnd(&backend.hwnd_raw).is_none(),
            "no window attached yet"
        );

        // Simulates: `win::run::run_inner` later calls `attach_surface`,
        // which stores the real `HWND` into the same `Arc` the closure
        // holds — not into a value the closure already snapshotted.
        let fake_hwnd_value: isize = 0xDEAD_BEEFusize as isize;
        backend
            .hwnd_raw
            .store(fake_hwnd_value, std::sync::atomic::Ordering::Release);

        // The live read now sees the attached window — proving a waker
        // instance obtained pre-attach isn't permanently stuck on `None`.
        let resolved = live_hwnd(&backend.hwnd_raw).expect("window is now attached");
        assert_eq!(resolved.0 as isize, fake_hwnd_value);
    }

    #[test]
    fn poll_events_does_not_panic_on_empty_queue() {
        // `poll_events` must not panic with nothing queued.
        let mut b = WinBackend::new();
        assert!(b.poll_events().is_empty());
    }

    #[test]
    fn wait_events_does_not_panic_and_drains_like_poll_events() {
        // `wait_events` must not panic with nothing queued. No true blocking
        // primitive is wired up (see `Backend::wait_events`'s doc on `WinBackend`),
        // so a queued event is still returned immediately rather than lost.
        let mut b = WinBackend::new();
        assert!(b.wait_events(Duration::from_millis(50)).is_empty());

        b.push_event(win_mouse_down(1.0, 2.0));
        let evs = b.wait_events(Duration::from_millis(50));
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], UiEvent::MouseDown { .. }));
    }

    #[test]
    fn poll_events_and_wait_events_agree_for_the_same_sequence() {
        // Acceptance bar: the queue-adapter behaviour must match
        // `GtkBackend`'s for the same event sequence. `GtkBackend::
        // poll_events`/`wait_events` are both a plain drain + the same
        // `apply_accelerators` pass over the batch (see
        // `gtk::backend::GtkBackend::{poll_events,wait_events}`); this
        // pins `WinBackend` doing byte-for-byte the same thing for a
        // fixed input rather than the two diverging (e.g. one applying
        // accelerators and the other not).
        let seq = |mut b: WinBackend| -> WinBackend {
            b.register_accelerator(&acc("save", "<C-s>"));
            b.push_event(UiEvent::KeyPressed {
                key: Key::Char('s'),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                repeat: false,
            });
            b.push_event(win_mouse_down(1.0, 2.0));
            b
        };

        let mut by_poll = seq(WinBackend::new());
        let via_poll = by_poll.poll_events();

        let mut by_wait = seq(WinBackend::new());
        let via_wait = by_wait.wait_events(Duration::from_millis(0));

        assert_eq!(via_poll, via_wait);
        assert_eq!(
            via_poll[0],
            UiEvent::Accelerator(
                AcceleratorId::new("save"),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                }
            ),
            "registered KeyPressed must rewrite to Accelerator, matching \
             GtkBackend::apply_accelerators",
        );
        assert!(matches!(via_poll[1], UiEvent::MouseDown { .. }));
    }

    #[test]
    fn focused_activity_bar_id_starts_none() {
        let b = WinBackend::new();
        assert!(b.focused_activity_bar_id().is_none());
    }

    #[test]
    fn painted_activity_bar_tracks_keyboard_focus() {
        let mut b = WinBackend::new();
        let bar = ActivityBar {
            id: WidgetId::new("demo:bar"),
            top_items: Vec::new(),
            bottom_items: Vec::new(),
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: true,
            style: Default::default(),
        };
        // The frame's painted-widget record is what `end_frame` reads the
        // focused rail from, so paint the bar through the widget contract
        // and close the frame. No surface is attached, so neither call
        // reaches Direct2D.
        crate::primitive::clear_painted_widgets();
        let _ =
            crate::DrawsPrimitives::draw_dyn(&mut b, Rect::new(0.0, 0.0, 40.0, 200.0), &bar, &());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.end_frame();
        }));
        assert_eq!(
            b.focused_activity_bar_id(),
            Some(&WidgetId::new("demo:bar"))
        );
    }

    #[test]
    fn begin_frame_clears_focused_activity_bar() {
        let mut b = WinBackend::new();
        b.focused_activity_bar = Some(WidgetId::new("demo:bar"));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.begin_frame(Viewport::new(80.0, 24.0, 1.0));
        }));
        assert!(
            b.focused_activity_bar_id().is_none(),
            "focused_activity_bar must be cleared by begin_frame"
        );
    }

    // ─── #1199: window_chrome ───────────────────────────────────────────

    /// `CAP_CONTRACTS`'s `window_chrome` entry is `Proof::Any` — this is
    /// the one method that makes `backend_caps().window_chrome: true`
    /// honest (see that struct literal's doc). No window on any test
    /// host (real Windows CI included — this unit test never calls
    /// `attach_surface`), so this stays `false` the same way
    /// `MacBackend::toggle_window_maximize_false_without_window` does.
    #[test]
    fn toggle_window_maximize_false_without_window() {
        let mut b = WinBackend::new();
        assert!(!Backend::toggle_window_maximize(&mut b));
    }

    #[test]
    fn backend_caps_declares_window_chrome() {
        let b = WinBackend::new();
        #[cfg(target_os = "windows")]
        assert!(
            b.backend_caps().window_chrome,
            "issue #1199: WinBackend must declare window_chrome now that \
             toggle_window_maximize is overridden and win::run handles \
             WM_NCCALCSIZE/WM_NCHITTEST"
        );
        #[cfg(not(target_os = "windows"))]
        assert!(
            !b.backend_caps().window_chrome,
            "off-Windows backend_caps() is BackendCaps::empty() — see that method's doc"
        );
    }

    /// A native `HMENU` sits in the non-client area, which `window_chrome`'s
    /// custom caption permanently covers, so the native menu bar is unreachable —
    /// this asserts `native_menu: false` and `window_chrome: true`. The
    /// `tests/conformance/caps.rs` generic check
    /// `backends_declare_only_what_they_override` ties override existence to
    /// `native_menu: false` (no `install_menu_bar`/`show_context_menu` override
    /// may exist); this test pins the declared values themselves.
    #[test]
    fn backend_caps_declares_no_native_menu_but_keeps_window_chrome() {
        let b = WinBackend::new();
        #[cfg(target_os = "windows")]
        {
            assert!(
                !b.backend_caps().native_menu,
                "issue #1228: a native HMENU sits in the non-client area that the drawn \
                 caption (#1199) permanently covers, so Win-GUI must not claim native_menu \
                 — install_menu_bar is the trait's no-op default now, matching GtkBackend/\
                 TuiBackend"
            );
            assert!(
                b.backend_caps().window_chrome,
                "issue #1228 keeps the drawn caption from #1199 — only the native menu bar \
                 half of #1200 was reverted"
            );
        }
        #[cfg(not(target_os = "windows"))]
        assert!(
            !b.backend_caps().native_menu && !b.backend_caps().window_chrome,
            "off-Windows backend_caps() is BackendCaps::empty() — see that method's doc"
        );
    }

    /// `GetMenu(hwnd)` must be null on a real window — a caller that invokes
    /// `Backend::install_menu_bar` unaware `native_menu` went false must still
    /// get a no-op, matching the behaviour of other backends without a native
    /// menu bar. `WM_NCHITTEST`'s "drawn close button → `HTCLIENT`, empty band →
    /// `HTCAPTION`" behaviour is covered separately by
    /// [`nc_hit_test_client_on_an_inline_button`] / [`nc_hit_test_caption_in_the_empty_band`].
    /// Real hardware isn't needed to prove pure geometry, only to prove `GetMenu`.
    #[cfg(target_os = "windows")]
    #[test]
    fn install_menu_bar_never_attaches_a_native_menu_to_a_live_window() {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HINSTANCE;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetMenu, RegisterClassExW,
            CW_USEDEFAULT, WINDOW_EX_STYLE, WNDCLASSEXW,
        };

        unsafe extern "system" fn test_wndproc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> windows::Win32::Foundation::LRESULT {
            // SAFETY: `hwnd`/`msg`/`wparam`/`lparam` are this call's own
            // parameters, forwarded unchanged — same as every
            // `DefWindowProcW` stand-in elsewhere in this module's tests.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        // SAFETY: `GetModuleHandleW(PCWSTR::null())` is the documented
        // idiom for "this process's own module handle" — same as
        // `win::run::run_inner`'s identical call.
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .expect("GetModuleHandleW(None) always succeeds for the current process")
            .into();
        let class_name = win_wide_nul_terminated("quadraui_test_1228_no_native_menu_wndclass");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(test_wndproc),
            hInstance: hinstance,
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };
        // Ignore the return value: a second test in this same process
        // re-registering the same class name fails
        // (`ERROR_CLASS_ALREADY_EXISTS`), which is harmless here — only
        // the handle `CreateWindowExW` resolves by name matters, not
        // which call minted the class.
        //
        // SAFETY: `wc` is a fully-initialized `WNDCLASSEXW` on the stack,
        // still in scope; `RegisterClassExW` reads it synchronously and
        // does not retain the pointer. `class_name.as_ptr()` (via
        // `lpszClassName`) stays valid because `class_name` outlives this
        // call.
        unsafe {
            let _ = RegisterClassExW(&wc);
        }

        // SAFETY: `class_name` is a `Vec<u16>` that outlives this call, so
        // `class_name.as_ptr()` stays valid; `hinstance` came from the
        // successful `GetModuleHandleW` above; `lpCreateParams: None`
        // means this test window never populates `GWLP_USERDATA`, which
        // is fine since `test_wndproc` above never reads it.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR::from_raw(class_name.as_ptr()),
                PCWSTR::null(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(hinstance),
                None,
            )
        }
        .expect(
            "CreateWindowExW for an unshown top-level test window must succeed on real \
             Windows hardware",
        );

        let mut backend = WinBackend::new();
        backend.hwnd = Some(hwnd);

        // SAFETY: `hwnd` is this test's own live window from
        // `CreateWindowExW` above — reading its current menu handle has
        // no precondition beyond that.
        let menu_before = unsafe { GetMenu(hwnd) };
        assert!(
            menu_before.is_invalid(),
            "a freshly created window must start with no menu at all"
        );

        // A caller that invokes `Backend::install_menu_bar` — matching a caller
        // that hasn't noticed `native_menu` went false, or a cross-backend app
        // that calls this unconditionally on every backend — must get the same
        // no-op every other backend without a native menu bar gives it.
        {
            use crate::primitives::menu_bar::MenuBarItem;
            Backend::install_menu_bar(
                &mut backend,
                &MenuBar {
                    id: WidgetId::new("menubar"),
                    items: vec![MenuBarItem {
                        id: WidgetId::new("file"),
                        label: "&File".to_string(),
                        disabled: false,
                        submenu: None,
                    }],
                    open_item: None,
                    focused_item: None,
                },
            );
        }

        // SAFETY: same as above — `hwnd` is still this test's own live
        // window.
        let menu_after = unsafe { GetMenu(hwnd) };
        assert!(
            menu_after.is_invalid(),
            "issue #1228: install_menu_bar is the trait's no-op default on Win-GUI now — \
             GetMenu(hwnd) must stay null even after a caller invokes it"
        );

        // SAFETY: `hwnd` is this test's own still-live window from
        // `CreateWindowExW` above; `test_wndproc` (unlike the real
        // `win::run::wndproc`) never reads `GWLP_USERDATA`, so there is
        // no boxed state that must outlive this call the way
        // `run_inner`'s shutdown path requires.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    /// `register_zone` must record the zone into the backend's state.
    #[test]
    fn register_zone_records_the_zone() {
        let mut b = WinBackend::new();
        Backend::register_zone(
            &mut b,
            WidgetId::new("demo:zone"),
            Rect::new(1.0, 2.0, 3.0, 4.0),
        );
        assert_eq!(b.zones.len(), 1);
        assert_eq!(b.zones[0].id, WidgetId::new("demo:zone"));
        assert_eq!(b.zones[0].bounds, Rect::new(1.0, 2.0, 3.0, 4.0));
    }

    /// `begin_frame` must clear last frame's zones the same way it clears
    /// `focused_activity_bar` above — a zone a widget stops painting must
    /// stop resolving `nc_hit_test` too.
    #[test]
    fn begin_frame_clears_zones() {
        let mut b = WinBackend::new();
        Backend::register_zone(
            &mut b,
            WidgetId::new("demo:zone"),
            Rect::new(0.0, 0.0, 1.0, 1.0),
        );
        assert_eq!(b.zones.len(), 1);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.begin_frame(Viewport::new(80.0, 24.0, 1.0));
        }));
        assert!(
            b.zones.is_empty(),
            "begin_frame must clear last frame's zones"
        );
    }

    /// No `TITLE_BAR_DRAG_ZONE` registered this frame — every point
    /// answers `None`, so `win::run`'s `WM_NCHITTEST` handler leaves
    /// `DefWindowProcW`'s own answer alone.
    #[test]
    fn nc_hit_test_none_with_no_band_registered() {
        let b = WinBackend::new();
        assert_eq!(b.nc_hit_test(10.0, 10.0), None);
    }

    /// A point outside the registered band is `None` even once a band
    /// exists — same "leave DefWindowProcW alone" contract.
    #[test]
    fn nc_hit_test_none_outside_the_band() {
        let mut b = WinBackend::new();
        Backend::register_zone(
            &mut b,
            WidgetId::new(WinBackend::TITLE_BAR_DRAG_ZONE),
            Rect::new(0.0, 0.0, 800.0, 32.0),
        );
        assert_eq!(b.nc_hit_test(10.0, 100.0), None);
    }

    /// The empty part of the drawn band reports `Some(true)` (`HTCAPTION`)
    /// — the Test section's acceptance scenario, first half.
    #[test]
    fn nc_hit_test_caption_in_the_empty_band() {
        let mut b = WinBackend::new();
        Backend::register_zone(
            &mut b,
            WidgetId::new(WinBackend::TITLE_BAR_DRAG_ZONE),
            Rect::new(0.0, 0.0, 800.0, 32.0),
        );
        assert_eq!(b.nc_hit_test(400.0, 16.0), Some(true));
    }

    /// A smaller zone registered inside the band (an inline min/max/close
    /// button, the command-centre search box) excludes its own footprint
    /// — `Some(false)` (`HTCLIENT`), so the click reaches the widget as a
    /// normal `UiEvent::MouseDown` instead of starting an OS move/maximize
    /// gesture. Second half of the Test section's acceptance scenario.
    #[test]
    fn nc_hit_test_client_on_an_inline_button() {
        let mut b = WinBackend::new();
        Backend::register_zone(
            &mut b,
            WidgetId::new(WinBackend::TITLE_BAR_DRAG_ZONE),
            Rect::new(0.0, 0.0, 800.0, 32.0),
        );
        Backend::register_zone(
            &mut b,
            WidgetId::new("titlebar:minimize-button"),
            Rect::new(700.0, 4.0, 24.0, 24.0),
        );
        // Inside the button.
        assert_eq!(b.nc_hit_test(712.0, 16.0), Some(false));
        // Outside the button but still in the band.
        assert_eq!(b.nc_hit_test(100.0, 16.0), Some(true));
    }

    /// End-to-end: a `MenuBar` painted through the widget contract inside
    /// the title-bar band records its items as sub-regions, so a click on a
    /// label excludes itself from `HTCAPTION` with no hand-registered zone
    /// of its own.
    #[test]
    fn menu_bar_painted_through_the_contract_excludes_its_items_from_the_band() {
        let mut b = WinBackend::new();
        let band = Rect::new(0.0, 0.0, 800.0, 32.0);
        Backend::register_zone(&mut b, WidgetId::new(WinBackend::TITLE_BAR_DRAG_ZONE), band);

        let bar = MenuBar {
            id: WidgetId::new("bar"),
            items: vec![
                crate::primitives::menu_bar::MenuBarItem {
                    id: WidgetId::new("bar:file"),
                    label: "&File".into(),
                    disabled: false,
                    submenu: None,
                },
                crate::primitives::menu_bar::MenuBarItem {
                    id: WidgetId::new("bar:edit"),
                    label: "&Edit".into(),
                    disabled: false,
                    submenu: None,
                },
            ],
            open_item: None,
            focused_item: None,
        };
        let bar_rect = Rect::new(0.0, 0.0, 300.0, 32.0);
        let _ = crate::DrawsPrimitives::draw_dyn(&mut b, bar_rect, &bar, &());
        let layout = crate::DrawsPrimitives::layout_primitive(&b, bar_rect, &bar).layout;
        let file_item = layout
            .visible_items
            .iter()
            .find(|v| v.id == WidgetId::new("bar:file"))
            .expect("File item must be visible at this width");
        let (cx, cy) = (
            file_item.bounds.x + file_item.bounds.width / 2.0,
            file_item.bounds.y + file_item.bounds.height / 2.0,
        );
        // On "File": the OS must not swallow this as a caption drag.
        assert_eq!(b.nc_hit_test(cx, cy), Some(false));
        // Still empty band space well past the drawn items.
        assert_eq!(b.nc_hit_test(790.0, 16.0), Some(true));
    }

    /// A status bar painted into the band through the widget contract
    /// (vimcode's inline minimize/maximize/close buttons are ordinary
    /// `action_id` status-bar segments) needs no zone of its own: its
    /// segments are recorded as painted sub-regions, and `nc_hit_test`
    /// reads those as `HTCLIENT`.
    #[test]
    fn status_bar_segments_read_as_client_inside_the_band() {
        use crate::backend::DrawsPrimitives;
        let mut b = WinBackend::new();
        let band = Rect::new(0.0, 0.0, 800.0, 32.0);
        Backend::register_zone(&mut b, WidgetId::new(WinBackend::TITLE_BAR_DRAG_ZONE), band);

        let bar = StatusBar {
            id: WidgetId::new("titlebar:controls"),
            left_segments: vec![],
            right_segments: vec![
                crate::primitives::status_bar::StatusBarSegment {
                    text: "_".into(),
                    fg: crate::types::Color::rgb(255, 255, 255),
                    bg: crate::types::Color::rgb(0, 0, 0),
                    bold: false,
                    action_id: Some(WidgetId::new("titlebar:minimize")),
                },
                crate::primitives::status_bar::StatusBarSegment {
                    text: "X".into(),
                    fg: crate::types::Color::rgb(255, 255, 255),
                    bg: crate::types::Color::rgb(0, 0, 0),
                    bold: false,
                    action_id: Some(WidgetId::new("titlebar:close")),
                },
            ],
        };
        crate::primitive::clear_painted_widgets();
        // Non-zero x and y, so a probe derived from bar-local bounds alone
        // would miss the painted segment.
        let status_rect = Rect::new(600.0, 2.0, 200.0, 28.0);
        let painted = b.draw_dyn(status_rect, &bar, &());
        let layout = &painted
            .downcast_ref::<crate::primitives::status_bar::StatusBarPrimitiveLayout>()
            .expect("a StatusBar lays out as StatusBarPrimitiveLayout")
            .layout;
        let (close_rect, _) = layout
            .hit_regions
            .iter()
            .find(|(_, hit)| {
                matches!(
                    hit,
                    crate::primitives::status_bar::StatusBarHit::Segment(id)
                        if *id == WidgetId::new("titlebar:close")
                )
            })
            .expect("close segment must be a hit region");
        // `close_rect` is bar-local; the probe adds `status_rect`'s own
        // absolute origin back in.
        let (cx, cy) = (
            status_rect.x + close_rect.x + close_rect.width / 2.0,
            status_rect.y + close_rect.y + close_rect.height / 2.0,
        );
        assert_eq!(b.nc_hit_test(cx, cy), Some(false));
        assert_eq!(b.nc_hit_test(10.0, 16.0), Some(true));
    }

    /// Paint `cc` into `rect` through the widget contract, the way a host
    /// pushes it as a `Surface::Widget`, and hand back its layout.
    fn draw_command_center_widget(
        b: &mut WinBackend,
        rect: Rect,
        cc: &crate::CommandCenter,
    ) -> crate::CommandCenterLayout {
        use crate::backend::DrawsPrimitives;
        b.draw_dyn(rect, cc, &())
            .downcast_ref::<crate::CommandCenterLayout>()
            .expect("a CommandCenter lays out as a CommandCenterLayout")
            .clone()
    }

    /// A widget-contract `CommandCenter` painted into the band needs no
    /// zone of its own: its arrows and search box are recorded as
    /// painted sub-regions, and `nc_hit_test` reads those as `HTCLIENT`.
    #[test]
    fn command_center_sub_regions_read_as_client_inside_the_band() {
        let mut b = WinBackend::new();
        let band = Rect::new(0.0, 0.0, 800.0, 32.0);
        Backend::register_zone(&mut b, WidgetId::new(WinBackend::TITLE_BAR_DRAG_ZONE), band);

        let cc = crate::CommandCenter {
            id: WidgetId::new("titlebar:cc"),
            back_enabled: true,
            forward_enabled: true,
            search_label: "project-name".into(),
        };
        let layout = draw_command_center_widget(&mut b, Rect::new(300.0, 0.0, 400.0, 32.0), &cc);
        let search = layout.search_bounds.expect("search box must be laid out");
        let (cx, cy) = (
            search.x + search.width / 2.0,
            search.y + search.height / 2.0,
        );
        // On the search box: must reach the app as a normal click.
        assert_eq!(b.nc_hit_test(cx, cy), Some(false));
        assert_eq!(b.nc_hit_test(1.0, 16.0), Some(true));
    }

    /// When the Command Center's container is wider than its centred content,
    /// the left/right padding inside the container — but still inside the drag
    /// band — must read `Some(true)` (`HTCAPTION`), not get swallowed by an
    /// over-eager exclusion. The blank strip the Command Center's padding should
    /// leave `HTCAPTION` must stay that way. The probes inside the search box
    /// alone can't catch a content block that over-fills its container or a
    /// container with unclaimed room — this test probes the gap between the
    /// container's left edge and the centred content instead.
    #[test]
    fn command_center_leaves_its_own_container_padding_as_caption() {
        let mut b = WinBackend::new();
        let band = Rect::new(0.0, 0.0, 900.0, 32.0);
        Backend::register_zone(&mut b, WidgetId::new(WinBackend::TITLE_BAR_DRAG_ZONE), band);

        // A short label floors at `CommandCenterMeasure`'s 280px minimum
        // search-box width, so the centred content (2 arrows + 2 gaps +
        // the 280px floor) is well short of the 400px-wide container
        // below — leaving real padding on both sides to probe.
        let cc = crate::CommandCenter {
            id: WidgetId::new("titlebar:cc"),
            back_enabled: true,
            forward_enabled: true,
            search_label: "x".into(),
        };
        let container = Rect::new(300.0, 0.0, 400.0, 32.0);
        let layout = draw_command_center_widget(&mut b, container, &cc);

        let back = layout.back_bounds.expect("back arrow must be laid out");
        assert!(
            back.x > container.x,
            "fixture must actually leave left padding to probe (back.x={}, container.x={})",
            back.x,
            container.x
        );

        // In the container, left of the back arrow, still inside the
        // drag band: must read as a draggable caption point, not get
        // excluded by a zone that over-claims the container's own
        // padding.
        let pad_x = (container.x + back.x) / 2.0;
        assert_eq!(b.nc_hit_test(pad_x, 16.0), Some(true));
    }

    /// A coarser zone that contains the whole band (e.g. `ShellView`'s own
    /// `"app-shell:window"` whole-window zone) must **not** be treated as
    /// an exclusion — otherwise every point in the band would read as
    /// "covered by another zone" and this would always answer
    /// `Some(false)`, the exact double-titlebar bug this issue exists to
    /// fix. Only zones strictly *smaller* than the band count.
    #[test]
    fn nc_hit_test_ignores_zones_at_least_as_large_as_the_band() {
        let mut b = WinBackend::new();
        Backend::register_zone(
            &mut b,
            WidgetId::new("app-shell:window"),
            Rect::new(0.0, 0.0, 800.0, 600.0),
        );
        Backend::register_zone(
            &mut b,
            WidgetId::new(WinBackend::TITLE_BAR_DRAG_ZONE),
            Rect::new(0.0, 0.0, 800.0, 32.0),
        );
        assert_eq!(b.nc_hit_test(400.0, 16.0), Some(true));
    }

    /// `PointerShape` -> Win32 `IDC_*` cursor-resource mapping
    /// covers every variant, mirroring
    /// `gtk::backend::tests::pointer_shape_cursor_name_maps_every_variant`.
    /// Windows-only: unlike GTK's CSS-keyword table, the mapping function
    /// itself uses real `windows`-crate types (`PCWSTR`), so this only
    /// compiles (and only runs) once `target_os = "windows"` also
    /// compiles the rest of this file's real WinAPI calls — see the
    /// module docs' "on Linux every real WinAPI call compiles to its
    /// `todo!()` fallback body" note; the `x86_64-pc-windows-msvc`
    /// cross-target `cargo check`/`clippy` runs in this repo's own dev
    /// loop (not `ci.yml`, which only runs the real Windows job) type-
    /// check it without executing it.
    #[cfg(target_os = "windows")]
    #[test]
    fn pointer_shape_to_win32_cursor_maps_every_variant() {
        // Uses the shared `desktop::all_pointer_shapes()` scaffold instead of
        // hand-listing all 18 `Default`/`Resize(edge)`/rest combinations.
        // The non-`Resize` variants beyond `Default` have no Win32 cursor
        // wired up yet, so they all fall back to `IDC_ARROW` — see
        // `pointer_shape_to_win32_cursor`'s doc.
        let expected = [
            IDC_ARROW,
            IDC_SIZENS,   // North
            IDC_SIZENS,   // South
            IDC_SIZEWE,   // East
            IDC_SIZEWE,   // West
            IDC_SIZENESW, // NorthEast
            IDC_SIZENWSE, // NorthWest
            IDC_SIZENWSE, // SouthEast
            IDC_SIZENESW, // SouthWest
            IDC_ARROW,    // Text
            IDC_ARROW,    // Pointer
            IDC_ARROW,    // Grab
            IDC_ARROW,    // Grabbing
            IDC_ARROW,    // Crosshair
            IDC_ARROW,    // Wait
            IDC_ARROW,    // NotAllowed
            IDC_ARROW,    // ColResize
            IDC_ARROW,    // RowResize
        ];
        let shapes = crate::desktop::all_pointer_shapes();
        assert_eq!(
            shapes.len(),
            expected.len(),
            "every PointerShape must have an expected Win32 cursor — a new variant means a new \
             row here, not a silently-truncated zip"
        );
        for (shape, expected) in shapes.iter().zip(expected.iter()) {
            assert_eq!(pointer_shape_to_win32_cursor(*shape), *expected);
        }
    }

    /// `Backend::set_theme` must reach the rasterisers, not just be stored and
    /// ignored — a real `HeadlessSurface` pixel probe. Tests a
    /// [`crate::TerminalDivider`]'s painted pixel: a single flat
    /// `theme.separator` fill with no blending, so a non-default theme's colour
    /// must come through pixel-exact if `set_theme` is wired at all.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_rasterisers() {
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 32;
        const H: u32 = 32;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_separator = Color::rgb(0x12, 0x34, 0x56);
        assert_ne!(
            custom_separator,
            Theme::default().separator,
            "test fixture bug: the probe colour must differ from the default theme's, or a \
             hardcoded `Theme::default()` would pass this test too"
        );
        backend.set_theme(Theme {
            separator: custom_separator,
            ..Theme::default()
        });

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        // 1 DIP wide at x=0, spanning the full probe height — see
        // `crate::terminal_style::divider_geometry`.
        crate::DrawsPrimitives::draw_primitive(
            &mut backend,
            Rect::new(0.0, 0.0, 1.0, H as f32),
            &crate::TerminalDivider::new(WidgetId::new("divider")),
        );
        backend.end_frame();

        let px = surface.pixel_at(0, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_separator.r, custom_separator.g, custom_separator.b),
            "painted pixel must reflect the theme set via `set_theme`, not `Theme::default()`",
        );
    }

    /// `activity_bar`/`menu_bar`/`context_menu`/`completions`/`find_replace` must
    /// read `WinBackend::current_theme`, not each build their own
    /// `Theme::default()` — a dark-themed app must get a dark activity bar,
    /// menu bar, context menu, completions popup and find/replace panel. Tests
    /// with a theme colour that differs from the default, and demands the
    /// non-default colour show up pixel-exact, the same shape
    /// [`set_theme_reaches_the_rasterisers`] established.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_activity_bar() {
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 40;
        const H: u32 = 40;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_bg = Color::rgb(0x11, 0x22, 0x33);
        assert_ne!(
            custom_bg,
            Theme::default().tab_bar_bg,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            tab_bar_bg: custom_bg,
            ..Theme::default()
        });

        let bar = ActivityBar {
            id: WidgetId::new("bar"),
            top_items: Vec::new(),
            bottom_items: Vec::new(),
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
        };

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let _ = crate::DrawsPrimitives::draw_dyn(
            &mut backend,
            Rect::new(0.0, 0.0, W as f32, H as f32),
            &bar,
            &(),
        );
        backend.end_frame();

        let px = surface.pixel_at(W / 2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "activity bar background must reflect the live theme, not `Theme::default()`",
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_menu_bar() {
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 100;
        const H: u32 = 24;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_bg = Color::rgb(0x44, 0x55, 0x66);
        assert_ne!(
            custom_bg,
            Theme::default().tab_bar_bg,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            tab_bar_bg: custom_bg,
            ..Theme::default()
        });

        let bar = MenuBar {
            id: WidgetId::new("bar"),
            items: Vec::new(),
            open_item: None,
            focused_item: None,
        };

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let _ = crate::DrawsPrimitives::draw_dyn(
            &mut backend,
            Rect::new(0.0, 0.0, W as f32, H as f32),
            &bar,
            &(),
        );
        backend.end_frame();

        let px = surface.pixel_at(W / 2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "menu bar background must reflect the live theme, not `Theme::default()`",
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_context_menu() {
        use crate::primitives::context_menu::{ContextMenuItem, ContextMenuPlacement};
        use crate::theme::Theme;
        use crate::types::{Color, StyledText};
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 100;
        const H: u32 = 100;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_bg = Color::rgb(0x77, 0x88, 0x99);
        assert_ne!(
            custom_bg,
            Theme::default().hover_bg,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            hover_bg: custom_bg,
            ..Theme::default()
        });

        // Two real items, because the popup's height is the summed
        // per-item heights: an empty `items` gives `bounds.height == 0`,
        // the painter early-returns without painting a single pixel, and the probe
        // reads `begin_frame`'s clear colour instead of the fill under
        // test. Each item carries an `id` (an `id`-less item *is* the
        // separator — `ContextMenuItem::is_separator`), a one-char label
        // so no glyph reaches the probe column, and no
        // `detail`/`key_equivalent` so no right-aligned shortcut paints
        // either. `bg` stays `None` so the fill falls back to
        // `theme.hover_bg`.
        let item = |id: &str, label: &str| ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(label),
            ..Default::default()
        };
        let menu = ContextMenu {
            id: WidgetId::new("ctx"),
            items: vec![item("a", "A"), item("b", "B")],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        };
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = crate::DrawsPrimitives::draw_primitive(
            &mut backend,
            Rect::new(4.0, 4.0, 60.0, H as f32 - 8.0),
            &menu,
        )
        .layout;
        backend.end_frame();

        assert!(
            layout.bounds.width > 0.0 && layout.bounds.height > 0.0,
            "test fixture bug: empty menu bounds"
        );
        // Probe the *second* row: `selected_idx` is 0, so row 0 is
        // covered by the `theme.selected_bg` highlight. Three-quarters
        // across the row keeps clear of the left-aligned label and of
        // the border stroke on either edge. Derived from the layout, not
        // hardcoded.
        let row = layout
            .visible_items
            .get(1)
            .expect("test fixture bug: second menu row not visible")
            .bounds;
        let px = surface.pixel_at(
            (row.x + row.width * 0.75) as u32,
            (row.y + row.height / 2.0) as u32,
        );
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "context menu background must reflect the live theme, not `Theme::default()`",
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_completions() {
        use crate::primitives::completions::{CompletionItem, Completions};
        use crate::theme::Theme;
        use crate::types::{Color, StyledText};
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 200;
        const H: u32 = 100;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_bg = Color::rgb(0xaa, 0xbb, 0xcc);
        assert_ne!(
            custom_bg,
            Theme::default().completion_bg,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            completion_bg: custom_bg,
            ..Theme::default()
        });

        // Two real items, so the popup has a second, unselected row for
        // the probe: with no items there are no rows, nothing paints,
        // and the probe reads `begin_frame`'s clear colour instead of
        // the fill under test. `detail` stays `None` so no right-aligned
        // text paints into the probe column.
        let item = |label: &str| CompletionItem {
            label: StyledText::plain(label),
            detail: None,
            documentation: None,
            kind: Default::default(),
            icon: None,
        };
        let completions = Completions {
            id: WidgetId::new("comp"),
            items: vec![item("alpha"), item("beta")],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        // The popup's already-placed box, tall enough for both rows at
        // this backend's own chrome row pitch.
        let popup = Rect::new(10.0, 10.0, 120.0, 80.0);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout =
            crate::DrawsPrimitives::draw_primitive(&mut backend, popup, &completions).layout;
        backend.end_frame();

        assert!(
            layout.bounds.width > 0.0 && layout.bounds.height > 0.0,
            "test fixture bug: empty popup bounds"
        );
        // Probe the *second* row: `selected_idx` is 0, so row 0 carries
        // the `theme.completion_selected_bg` highlight. Three-quarters
        // across the row clears the left-aligned `" {label}"` text and
        // the border stroke on either edge. Derived from the layout, not
        // hardcoded.
        let row = layout
            .visible_items
            .get(1)
            .expect("test fixture bug: second completion row not visible")
            .bounds;
        let px = surface.pixel_at(
            (row.x + row.width * 0.75) as u32,
            (row.y + row.height / 2.0) as u32,
        );
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "completions popup background must reflect the live theme, not `Theme::default()`",
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_find_replace() {
        use crate::primitives::find_replace::compute_hit_regions;
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 600;
        const H: u32 = 200;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_bg = Color::rgb(0x22, 0x44, 0x66);
        assert_ne!(
            custom_bg,
            Theme::default().surface_bg,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            surface_bg: custom_bg,
            ..Theme::default()
        });

        let (hit_regions, _input_width) = compute_hit_regions(50, false, "1 of 3", 2, 2);
        let panel = FindReplacePanel {
            query: "needle".into(),
            replacement: String::new(),
            show_replace: false,
            focus: 0,
            cursor: 3,
            sel_anchor: None,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds: Rect::new(0.0, 0.0, W as f32, H as f32),
            panel_width: 50,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions,
        };

        // Reads the cell metrics off the backend rather than assuming
        // `WinBackend::new`'s 16.0/8.0 defaults: `attach_headless`
        // overwrites `current_line_height`/`current_char_width` with the
        // *measured* DirectWrite metrics of the editor font, and those
        // are what `Backend::draw_find_replace` then hands
        // `crate::primitives::find_replace::paint`. Hardcoding
        // 16.0/8.0 here would put the probe at coordinates the painter
        // never painted on any host whose font measures differently.
        let cw = backend.char_width().max(1.0);
        let lh = backend.line_height().max(1.0);
        // Same geometry `native_surface_paint::paint` resolves at paint
        // time, via the shared `FindReplaceMetrics` seam.
        let style = crate::style::Style::default();
        let metrics =
            crate::primitives::find_replace::FindReplaceMetrics::resolve(&panel, &style, cw, lh);
        let popup = metrics.popup_rect;
        assert!(
            popup.width > 8.0
                && popup.height > 8.0
                && popup.x + popup.width <= W as f32
                && popup.y + popup.height <= H as f32,
            "test fixture bug: popup {popup:?} (cell {cw}x{lh}) does not fit the \
             {W}x{H} probe surface",
        );

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.draw_find_replace(Rect::new(0.0, 0.0, W as f32, H as f32), &panel);
        backend.end_frame();

        // 4 DIPs inside the popup's bottom-right corner: past the border
        // stroke and inside the bottom padding below the single
        // (`show_replace == false`) row of hit regions, so nothing but
        // the background fill can have painted here.
        let px = surface.pixel_at(
            (popup.x + popup.width - 4.0) as u32,
            (popup.y + popup.height - 4.0) as u32,
        );
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "find/replace panel background must reflect the live theme, not `Theme::default()`",
        );
    }

    /// The editor must read the live theme from `WinBackend::current_theme`
    /// rather than building its own `Theme::default()`, so a dark-themed app
    /// gets a dark editor background after `:colorscheme`/`set_theme`. Same
    /// pixel-probe pattern as the activity bar's `set_theme` coverage.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_editor() {
        use crate::backend::DrawsPrimitives;
        use crate::primitives::editor::Editor;
        use crate::theme::Theme;
        use crate::types::{Color, WidgetId};
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 80;
        const H: u32 = 40;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_bg = Color::rgb(0x77, 0x88, 0x99);
        assert_ne!(
            custom_bg,
            Theme::default().background,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            background: custom_bg,
            ..Theme::default()
        });

        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let editor = Editor::new(WidgetId::new("editor"), rect);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.draw_primitive(rect, &editor);
        backend.end_frame();

        let px = surface.pixel_at(W / 2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "editor background must reflect the live theme, not `Theme::default()`",
        );
    }

    /// `paint_options.v_scrollbar_w = Some(14.0)` makes the editor paint a
    /// scrollbar column exactly 14 DIPs wide — `[186, 200)` in a 200-DIP
    /// viewport — instead of one `current_char_width` (8 DIPs), and its
    /// layout reports the same column for hit-testing.
    #[cfg(target_os = "windows")]
    #[test]
    fn editor_v_scrollbar_w_paints_14px_column() {
        use crate::backend::DrawsPrimitives;
        use crate::primitives::editor::Editor;
        use crate::theme::Theme;
        use crate::types::WidgetId;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 200;
        const H: u32 = 80;

        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let mut editor = Editor::new(WidgetId::new("editor"), rect);
        editor.total_lines = 50;
        let bg = Theme::default().background;
        let bg = (bg.r, bg.g, bg.b);

        let paint = |v_scrollbar_w: Option<f32>| {
            let surface = HeadlessSurface::new(W, H).expect("create headless surface");
            let mut backend = WinBackend::new();
            backend
                .attach_headless(surface.target().clone(), W, H)
                .expect("attach headless surface");
            let mut editor = editor.clone();
            editor.paint_options.v_scrollbar_w = v_scrollbar_w;
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.draw_primitive(rect, &editor);
            backend.end_frame();
            surface
        };
        let probe = |surface: &HeadlessSurface, x: u32| {
            let px = surface.pixel_at(x, H - 4);
            (px.r, px.g, px.b)
        };
        // Direct2D anti-aliases a fill's boundary pixel, bleeding a level or
        // two of track colour into the neighbouring text-area pixel, so an
        // exact `== bg` on a pixel next to the column edge is not a stable
        // assertion. The track (40, 44, 56) is ~20 levels per channel clear
        // of the background (20, 22, 30), far outside this tolerance, so
        // "background" and "track" stay unambiguous.
        const AA_TOLERANCE: i32 = 4;
        let is_bg = |px: (u8, u8, u8)| {
            (px.0 as i32 - bg.0 as i32).abs() <= AA_TOLERANCE
                && (px.1 as i32 - bg.1 as i32).abs() <= AA_TOLERANCE
                && (px.2 as i32 - bg.2 as i32).abs() <= AA_TOLERANCE
        };

        let widened = paint(Some(14.0));
        assert!(
            is_bg(probe(&widened, 183)),
            "x=183 lies left of the 14px column at [186, 200)"
        );
        for x in 186..W {
            assert!(
                !is_bg(probe(&widened, x)),
                "x={x} lies inside the 14px column [186, 200)"
            );
        }

        let default = paint(None);
        assert!(
            is_bg(probe(&default, 189)),
            "default column is 8px: x=189 is text area"
        );
        assert!(
            !is_bg(probe(&default, 194)),
            "default column is 8px: x=194 is track"
        );

        let backend = WinBackend::new();
        let mut editor = editor.clone();
        editor.paint_options.v_scrollbar_w = Some(14.0);
        let vsb = backend
            .layout_primitive(rect, &editor)
            .v_scrollbar_bounds
            .expect("buffer overflows");
        assert_eq!((vsb.x, vsb.width), (186.0, 14.0));
    }

    /// `TreeView`'s paint (the Explorer sidebar's own content rasteriser) must read
    /// the live theme from `WinBackend::current_theme` rather than building
    /// its own `Theme::default()`.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_tree() {
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 80;
        const H: u32 = 40;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_bg = Color::rgb(0xaa, 0xbb, 0xcc);
        assert_ne!(
            custom_bg,
            Theme::default().tab_bar_bg,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            tab_bar_bg: custom_bg,
            ..Theme::default()
        });

        let tree = flat_tree(1);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let _ = crate::backend::DrawsPrimitives::draw_dyn(&mut backend, rect, &tree, &());
        backend.end_frame();

        // Sample past the single row's band — still inside the tree's
        // own background fill, clear of any row chrome.
        let px = surface.pixel_at(W / 2, H - 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "Explorer sidebar (tree) background must reflect the live theme, not `Theme::default()`",
        );
    }

    /// The tab strip's painter must read the live theme from
    /// `WinBackend::current_theme` rather than building its own
    /// `Theme::default()`.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_tab_bar() {
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 100;
        const H: u32 = 24;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_bg = Color::rgb(0x33, 0x66, 0x99);
        assert_ne!(
            custom_bg,
            Theme::default().tab_bar_bg,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            tab_bar_bg: custom_bg,
            ..Theme::default()
        });

        let tab_bar = TabBar {
            id: WidgetId::new("tab-bar"),
            tabs: Vec::new(),
            scroll_offset: 0,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: false,
            compact: false,
            ..Default::default()
        };
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        crate::DrawsPrimitives::draw_dyn(&mut backend, rect, &tab_bar, &());
        backend.end_frame();

        let px = surface.pixel_at(W / 2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "tab bar background must reflect the live theme, not `Theme::default()`",
        );
    }

    /// A `SidebarPanel`'s embedded toolbar chrome must paint from the
    /// live theme rather than a hardcoded `Theme::default()`.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_sidebar_panel_toolbar() {
        use crate::theme::Theme;
        use crate::types::{Color, WidgetId};
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 100;
        const H: u32 = 60;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_header_bg = Color::rgb(0x55, 0x11, 0x77);
        assert_ne!(
            custom_header_bg,
            Theme::default().header_bg,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            header_bg: custom_header_bg,
            ..Theme::default()
        });

        let panel = crate::primitives::sidebar_panel::SidebarPanel {
            id: WidgetId::new("sidebar-panel"),
            toolbar: Some(crate::primitives::toolbar::Toolbar {
                id: WidgetId::new("toolbar"),
                buttons: Vec::new(),
                bg: None,
                focused_index: None,
                ..Default::default()
            }),
            toolbar_height: Some(24.0),
        };
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &panel);
        backend.end_frame();

        let tb = layout
            .toolbar_bounds
            .expect("panel has a toolbar, so a toolbar slot is reserved");
        let px = surface.pixel_at(
            (tb.x + tb.width / 2.0) as u32,
            (tb.y + tb.height / 2.0) as u32,
        );
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_header_bg.r, custom_header_bg.g, custom_header_bg.b),
            "sidebar panel toolbar background must reflect the live theme, not `Theme::default()`",
        );
    }

    /// A split's divider chrome must paint from the live theme rather
    /// than a hardcoded `Theme::default()`.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_split() {
        use crate::primitives::split::{Split, SplitDirection};
        use crate::theme::Theme;
        use crate::types::{Color, WidgetId};
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 100;
        const H: u32 = 60;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_separator = Color::rgb(0x44, 0x22, 0x88);
        assert_ne!(
            custom_separator,
            Theme::default().separator,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            separator: custom_separator,
            ..Theme::default()
        });

        let split = Split {
            id: WidgetId::new("split"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &split);
        backend.end_frame();

        let db = layout.divider_bounds;
        let px = surface.pixel_at(
            (db.x + db.width / 2.0) as u32,
            (db.y + db.height / 2.0) as u32,
        );
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_separator.r, custom_separator.g, custom_separator.b),
            "split divider must reflect the live theme, not `Theme::default()`",
        );
    }

    /// A split tree's divider chrome must paint from the live theme
    /// rather than a hardcoded `Theme::default()`.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_split_tree() {
        use crate::primitives::split::SplitDirection;
        use crate::primitives::split_tree::SplitTree;
        use crate::theme::Theme;
        use crate::types::{Color, WidgetId};
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 100;
        const H: u32 = 60;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let custom_separator = Color::rgb(0x66, 0x33, 0x99);
        assert_ne!(
            custom_separator,
            Theme::default().separator,
            "test fixture bug: the probe colour must differ from the default theme's"
        );
        backend.set_theme(Theme {
            separator: custom_separator,
            ..Theme::default()
        });

        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(WidgetId::new("left")),
            SplitTree::leaf(WidgetId::new("right")),
        );
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &tree);
        backend.end_frame();

        let div = layout
            .dividers
            .first()
            .expect("one divider between two leaves");
        let (cx, cy) = match div.direction {
            SplitDirection::Horizontal => (
                div.position + div.thickness / 2.0,
                div.cross_start + div.cross_size / 2.0,
            ),
            SplitDirection::Vertical => (
                div.cross_start + div.cross_size / 2.0,
                div.position + div.thickness / 2.0,
            ),
        };
        let px = surface.pixel_at(cx as u32, cy as u32);
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_separator.r, custom_separator.g, custom_separator.b),
            "split-tree divider must reflect the live theme, not `Theme::default()`",
        );
    }

    // ── PaintSurface (#807, Phase 1) ────────────────────────────────────────
    //
    // Real Direct2D execution — `#[cfg(target_os = "windows")]`-gated because
    // `attach_headless`/`HeadlessSurface` only exist on that target; `cargo
    // check --features win` on Linux still type-checks the impl block above.

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_paint_surface_fill_rect_paints_solid_color() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 32;
        const H: u32 = 32;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let red = Color::rgb(200, 20, 20);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), red);
        backend.end_frame();

        let px = surface.pixel_at(5, 5);
        assert_eq!(
            (px.r, px.g, px.b),
            (red.r, red.g, red.b),
            "surface_fill_rect must paint the solid color it was given"
        );
    }

    /// The colour an unpainted pixel carries after [`Backend::begin_frame`] on a
    /// freshly-attached [`HeadlessSurface`].
    ///
    /// Win-GUI's `begin_frame` issues a `Clear` with its placeholder background,
    /// so "untouched" is **not** `(0, 0, 0)` the way it is for the GTK twins of the
    /// tests below — those probe a zeroed `cairo::ImageSurface` that nothing
    /// clears. Reading one pixel from a live empty frame keeps these assertions
    /// honest whichever colour that clear grows into.
    #[cfg(target_os = "windows")]
    fn cleared_frame_background(width: u32, height: u32) -> (u8, u8, u8) {
        use crate::win::testing::HeadlessSurface;

        let surface = HeadlessSurface::new(width, height).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), width, height)
            .expect("attach headless surface");
        backend.begin_frame(Viewport::new(width as f32, height as f32, 1.0));
        backend.end_frame();
        let px = surface.pixel_at(0, 0);
        (px.r, px.g, px.b)
    }

    /// [`cleared_frame_background`] samples a single pixel as "the
    /// background every other headless test compares against"; this is
    /// the test that proves one pixel is enough, i.e. that
    /// [`Backend::begin_frame`]'s `Clear` really covers the whole render
    /// target rather than some sub-rect.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_begin_frame_clears_the_whole_surface_uniformly() {
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 16;
        const H: u32 = 16;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.end_frame();

        let expected = cleared_frame_background(W, H);
        for y in 0..H {
            for x in 0..W {
                let px = surface.pixel_at(x, y);
                assert_eq!(
                    (px.r, px.g, px.b),
                    expected,
                    "begin_frame's Clear must cover every pixel — ({x}, {y}) differs, so \
                     sampling one pixel as 'the background' would be wrong"
                );
            }
        }
    }

    /// Real `FillRoundedRectangle` execution, mirroring
    /// `gtk_backend_paint_surface_fill_rounded_rect_clips_the_corners` —
    /// a centre pixel must land the fill colour, and a corner pixel well
    /// inside a generous radius must stay at [`begin_frame`](Backend::begin_frame)'s
    /// clear colour (see [`cleared_frame_background`] for why that isn't
    /// black here, unlike the GTK twin), or this would just be
    /// `surface_fill_rect` under a new name.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_paint_surface_fill_rounded_rect_clips_the_corners() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 40;
        const H: u32 = 40;

        let background = cleared_frame_background(W, H);
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let red = Color::rgb(200, 20, 20);
        assert_ne!(
            background,
            (red.r, red.g, red.b),
            "the fill colour must differ from the frame's clear colour, or \
             'this corner stayed untouched' is unprovable"
        );
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.surface_fill_rounded_rect(Rect::new(0.0, 0.0, W as f32, H as f32), 15.0, red);
        backend.end_frame();

        let centre = surface.pixel_at(20, 20);
        assert_eq!(
            (centre.r, centre.g, centre.b),
            (red.r, red.g, red.b),
            "surface_fill_rounded_rect must paint the solid color at the box's centre"
        );
        let corner = surface.pixel_at(1, 1);
        assert_eq!(
            (corner.r, corner.g, corner.b),
            background,
            "a corner pixel well inside a radius-15 fillet on a 40x40 box must stay at \
             the frame's clear colour — otherwise this is just `surface_fill_rect` \
             under a new name"
        );
    }

    /// Real `DrawRoundedRectangle` execution, the stroke twin of
    /// `win_backend_paint_surface_fill_rounded_rect_clips_the_corners` —
    /// the straight middle of an edge lands the stroke colour, a corner
    /// well inside the fillet stays at the clear colour, and so does the
    /// box's own centre (proving this strokes rather than fills).
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_paint_surface_stroke_rounded_rect_clips_the_corners() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 40;
        const H: u32 = 40;

        let background = cleared_frame_background(W, H);
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let blue = Color::rgb(60, 120, 220);
        assert_ne!(
            background,
            (blue.r, blue.g, blue.b),
            "the stroke colour must differ from the frame's clear colour"
        );
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.surface_stroke_rounded_rect(
            Rect::new(0.0, 0.0, W as f32, H as f32),
            15.0,
            blue,
            4.0,
        );
        backend.end_frame();

        let edge = surface.pixel_at(20, 1);
        assert_eq!(
            (edge.r, edge.g, edge.b),
            (blue.r, blue.g, blue.b),
            "the straight middle of the top edge must land the stroke colour"
        );
        let corner = surface.pixel_at(1, 1);
        assert_eq!(
            (corner.r, corner.g, corner.b),
            background,
            "a corner pixel well inside a radius-15 fillet on a 40x40 box must stay at \
             the frame's clear colour"
        );
        let centre = surface.pixel_at(20, 20);
        assert_eq!(
            (centre.r, centre.g, centre.b),
            background,
            "the box's centre, far from any stroke, must stay at the frame's clear \
             colour — otherwise this is `surface_fill_rounded_rect` under a new name"
        );
    }

    /// `surface_draw_shadow` has no `WinBackend` override — it
    /// inherits `PaintSurface`'s default, composed out of
    /// `surface_fill_rounded_rect`. Proves that default verb genuinely
    /// paints on Direct2D's own render target, the Win-GUI twin of
    /// `gtk_backend_paint_surface_draw_shadow_elevation_three_paints_below_the_box`.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_paint_surface_draw_shadow_elevation_three_paints_below_the_box() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 40;
        const H: u32 = 40;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let white = Color::rgb(255, 255, 255);
        let shadow = Color::rgba(0, 0, 0, 200);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), white);
        backend.surface_draw_shadow(Rect::new(5.0, 5.0, 20.0, 20.0), 6.0, 3, shadow);
        backend.end_frame();

        let px = surface.pixel_at(15, 29);
        assert_ne!(
            (px.r, px.g, px.b),
            (white.r, white.g, white.b),
            "a pixel just below the box's bottom edge must pick up shadow ink at \
             elevation 3"
        );
    }

    /// `surface_fill_rect_alpha` has no `WinBackend` override —
    /// it inherits `PaintSurface`'s default, which forwards to
    /// `surface_fill_rect` with `color.a` replaced. This proves that
    /// default genuinely blends on Direct2D's own compositing pipeline
    /// (not merely that the Rust dispatch is correct), the Win-GUI twin
    /// of `gtk_backend_paint_surface_fill_rect_alpha_blends_with_the_background`.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_paint_surface_fill_rect_alpha_blends_with_the_background() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 40;
        const H: u32 = 40;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let white = Color::rgb(255, 255, 255);
        let red = Color::rgb(200, 20, 20);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), white);
        backend.surface_fill_rect_alpha(Rect::new(0.0, 0.0, W as f32, H as f32), red, 0.5);
        backend.end_frame();

        let px = surface.pixel_at(20, 20);
        assert!(
            (px.r, px.g, px.b) != (white.r, white.g, white.b)
                && (px.r, px.g, px.b) != (red.r, red.g, red.b),
            "a 50%-alpha fill over an opaque background must land a real blend, not the \
             background or the fill colour verbatim: got ({}, {}, {})",
            px.r,
            px.g,
            px.b
        );
        // Real source-over compositing of 50%-alpha red onto opaque white
        // lands roughly halfway between the two on every channel. The
        // bands are deliberately generous rather than exact: which colour
        // space Direct2D blends in (gamma-encoded for this target's
        // non-`_SRGB` `DXGI_FORMAT_B8G8R8A8_UNORM`, linear for an `_SRGB`
        // one) shifts the midpoint without changing what's being asserted
        // — that a blend happened at all, and towards the right end of
        // each channel.
        assert!(
            (200..=245).contains(&px.r),
            "red channel should sit between the fill's 200 and white's 255: got {}",
            px.r
        );
        assert!(
            (100..=200).contains(&px.g) && (100..=200).contains(&px.b),
            "green/blue channels should sit between the fill's 20 and white's 255: \
             got ({}, {})",
            px.g,
            px.b
        );
    }

    /// `role` must select a genuinely different live `IDWriteTextFormat`
    /// (the `chrome_dwrite` handle vs the editor `dwrite` one), not a documented
    /// no-op — proven by a huge chrome font size versus a tiny editor font size
    /// painting visibly different amounts of ink for the same glyph. Win-GUI twin
    /// of `gtk_backend_paint_surface_draw_text_run_with_role_uses_the_requested_fonts_size`.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_paint_surface_draw_text_run_with_role_uses_the_requested_fonts_size() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 80;
        const H: u32 = 80;

        // "Ink" is any pixel the glyph moved off `begin_frame`'s clear
        // colour — not "any pixel brighter than black", which on this
        // backend counts the whole cleared surface (see
        // `cleared_frame_background`).
        fn ink_pixel_count(surface: &HeadlessSurface, background: (u8, u8, u8)) -> u32 {
            let mut count = 0;
            for y in 0..H {
                for x in 0..W {
                    let px = surface.pixel_at(x, y);
                    if (px.r, px.g, px.b) != background {
                        count += 1;
                    }
                }
            }
            count
        }

        let background = cleared_frame_background(W, H);
        let white = Color::rgb(255, 255, 255);

        // `set_editor_font`/`set_ui_font` must precede `attach_headless`
        // — that's where the `IDWriteTextFormat` pair is built from these
        // fields (see `set_editor_font`'s doc).
        let chrome_surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend.set_editor_font(DEFAULT_UI_FONT_FAMILY, 6.0);
        backend.set_ui_font(&format!("{DEFAULT_UI_FONT_FAMILY} 60"));
        backend
            .attach_headless(chrome_surface.target().clone(), W, H)
            .expect("attach headless surface");
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.surface_draw_text_run_with_role(
            Rect::new(2.0, 2.0, 76.0, 76.0),
            "A",
            white,
            crate::FontRole::Chrome,
            false,
        );
        backend.end_frame();
        let chrome_ink = ink_pixel_count(&chrome_surface, background);

        let editor_surface = HeadlessSurface::new(W, H).expect("create headless surface");
        backend
            .attach_headless(editor_surface.target().clone(), W, H)
            .expect("attach headless surface");
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.surface_draw_text_run_with_role(
            Rect::new(2.0, 2.0, 76.0, 76.0),
            "A",
            white,
            crate::FontRole::Editor,
            false,
        );
        backend.end_frame();
        let editor_ink = ink_pixel_count(&editor_surface, background);

        assert!(
            chrome_ink > editor_ink * 4,
            "FontRole::Chrome (60pt) must paint far more ink than FontRole::Editor \
             (6pt) for the same glyph: chrome={chrome_ink}, editor={editor_ink}"
        );
        assert!(editor_ink > 0, "FontRole::Editor must still paint real ink");
    }

    /// A `Dialog` paints its body text in the chrome font, so it should
    /// render at the *chrome* font's size, not the editor's.
    ///
    /// The editor font size is held **fixed** across both runs — only the chrome
    /// (ui) font size varies. If it were painted through the editor `dwrite`
    /// handle, both runs would paint identical ink regardless of the chrome size.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_dialog_paints_body_text_in_the_chrome_fonts_size() {
        use crate::primitives::dialog::Dialog;
        use crate::theme::Theme;
        use crate::types::{StyledText, WidgetId};
        use crate::win::testing::HeadlessSurface;

        // Room for the dialog to lay itself out at either chrome size:
        // its box is sized from the chrome font's own metrics, so at
        // 40pt it needs far more than a 6pt run does.
        const W: u32 = 400;
        const H: u32 = 300;
        const EDITOR_SIZE_PT: f32 = 14.0;

        fn body_only_dialog() -> Dialog {
            Dialog {
                id: WidgetId::new("d"),
                title: StyledText::plain(""),
                body: vec![StyledText::plain("A")],
                buttons: vec![],
                severity: None,
                vertical_buttons: false,
                table: None,
                input: None,
            }
        }

        /// Rightmost ink column within `body_bounds` that differs from
        /// `theme.surface_bg` — i.e. the right edge of the body text's
        /// glyph ink, relative to `body_bounds.x`. Horizontal extent,
        /// not pixel *count*: a glyph taller than its body row is
        /// vertically clipped by `D2D1_DRAW_TEXT_OPTIONS_CLIP`, which
        /// makes a raw ink-pixel *count* comparison weak. Its *width* is
        /// never clipped (the row is far wider than one glyph), and a
        /// larger font paints a strictly wider glyph for any real
        /// typeface. Scanning stops at the surface's own edge.
        fn body_ink_right_edge(surface: &HeadlessSurface, body_bounds: Rect) -> i32 {
            let bg = Theme::default().surface_bg;
            let mut last = 0i32;
            let y_end = ((body_bounds.y + body_bounds.height) as u32).min(H);
            let x_end = ((body_bounds.x + body_bounds.width) as u32).min(W);
            for y in body_bounds.y as u32..y_end {
                for x in body_bounds.x as u32..x_end {
                    let px = surface.pixel_at(x, y);
                    if (px.r, px.g, px.b) != (bg.r, bg.g, bg.b) {
                        last = last.max(x as i32 - body_bounds.x as i32);
                    }
                }
            }
            last
        }

        fn paint_with_chrome_size(chrome_size_pt: f32) -> i32 {
            let surface = HeadlessSurface::new(W, H).expect("create headless surface");
            let mut backend = WinBackend::new();
            backend.set_editor_font(DEFAULT_UI_FONT_FAMILY, EDITOR_SIZE_PT);
            backend.set_ui_font(&format!("{DEFAULT_UI_FONT_FAMILY} {chrome_size_pt}"));
            backend
                .attach_headless(surface.target().clone(), W, H)
                .expect("attach headless surface");
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            // The dialog centres itself in the viewport it is handed.
            let layout = crate::DrawsPrimitives::draw_primitive(
                &mut backend,
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &body_only_dialog(),
            )
            .layout;
            backend.end_frame();
            body_ink_right_edge(&surface, layout.body_bounds)
        }

        let right_tiny_chrome = paint_with_chrome_size(6.0);
        let right_huge_chrome = paint_with_chrome_size(40.0);

        assert!(
            right_huge_chrome > right_tiny_chrome * 2,
            "with the editor font size held fixed at {EDITOR_SIZE_PT}pt, only varying \
             the chrome font size (6pt vs 40pt), the dialog body's glyph should paint \
             far wider at the larger chrome size — proves the dialog paints body text \
             via chrome_dwrite, not the (unchanged) editor dwrite handle \
             (right_tiny_chrome={right_tiny_chrome}, right_huge_chrome={right_huge_chrome})"
        );
        assert!(
            right_tiny_chrome > 0,
            "the tiny-chrome run must still paint some real ink, or this comparison \
             proves nothing"
        );
    }

    /// [`crate::RichTextPopup::font_role`] must select a genuinely
    /// different live `IDWriteTextFormat`, not a documented no-op. A
    /// descriptor left at the default role must paint through the chrome
    /// handle, and one carrying `FontRole::Editor` must paint through the
    /// editor handle instead.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_rich_text_popup_honours_font_role() {
        use crate::primitives::rich_text_popup::{PopupPlacement, RichTextPopup};
        use crate::types::{Color, StyledText, WidgetId};
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 120;
        const H: u32 = 120;
        const EDITOR_SIZE_PT: f32 = 6.0;
        const CHROME_SIZE_PT: f32 = 40.0;
        let bg = Color::rgb(0, 0, 0);
        let fg = Color::rgb(255, 255, 255);

        fn popup(bg: Color, fg: Color) -> RichTextPopup {
            RichTextPopup {
                id: WidgetId::new("rtp:font-role"),
                lines: vec![StyledText::plain("M")],
                line_text: vec!["M".to_string()],
                line_scales: vec![],
                scroll_top: 0,
                max_visible_rows: 4,
                has_focus: false,
                selection: None,
                links: vec![],
                focused_link: None,
                placement: PopupPlacement::Below,
                padding: 0.0,
                fg: Some(fg),
                bg: Some(bg),
                font_role: Default::default(),
            }
        }

        /// Rightmost glyph-ink column strictly inside `bounds` (the
        /// popup's `content_bounds`, which its own `bg` fills), relative
        /// to `bounds.x`. Strictly inside, so the frame stroke around the
        /// box — the same in both font configurations — is never counted.
        fn ink_right_edge(surface: &HeadlessSurface, bounds: Rect, bg: (u8, u8, u8)) -> i32 {
            let mut last = 0i32;
            let y0 = bounds.y.ceil().max(0.0) as u32;
            let y1 = (bounds.y + bounds.height).floor().min(H as f32) as u32;
            let x0 = bounds.x.ceil().max(0.0) as u32;
            let x1 = (bounds.x + bounds.width).floor().min(W as f32) as u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let px = surface.pixel_at(x, y);
                    if (px.r, px.g, px.b) != bg {
                        last = last.max(x as i32 - bounds.x as i32);
                    }
                }
            }
            last
        }

        fn paint_with_role(role: Option<crate::FontRole>, bg: Color, fg: Color) -> (i32, Rect) {
            let surface = HeadlessSurface::new(W, H).expect("create headless surface");
            let mut backend = WinBackend::new();
            backend.set_editor_font(DEFAULT_UI_FONT_FAMILY, EDITOR_SIZE_PT);
            backend.set_ui_font(&format!("{DEFAULT_UI_FONT_FAMILY} {CHROME_SIZE_PT}"));
            backend
                .attach_headless(surface.target().clone(), W, H)
                .expect("attach headless surface");
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            // The descriptor carries the font role the popup measures and
            // paints its body in.
            let p = RichTextPopup {
                font_role: role.unwrap_or_default(),
                ..popup(bg, fg)
            };
            let content = crate::DrawsPrimitives::draw_primitive(
                &mut backend,
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &p,
            )
            .layout
            .content_bounds;
            backend.end_frame();
            (
                ink_right_edge(&surface, content, (bg.r, bg.g, bg.b)),
                content,
            )
        }

        // `None` = a descriptor with the default role — must paint in the
        // chrome font.
        let (chrome_right, _) = paint_with_role(None, bg, fg);
        let (editor_right, _) = paint_with_role(Some(crate::FontRole::Editor), bg, fg);

        assert!(
            editor_right > 0,
            "FontRole::Editor must still paint real ink, got {editor_right}"
        );
        assert!(
            chrome_right > editor_right * 2,
            "with the editor font size held fixed at {EDITOR_SIZE_PT}pt, a popup at \
             the default role painting the {CHROME_SIZE_PT}pt chrome font should \
             paint a far wider glyph than FontRole::Editor: \
             chrome_right={chrome_right}, editor_right={editor_right}"
        );
    }

    // ── the eleven remaining `ChromePrimitive` rasterisers ──────
    //
    // `Dialog`/`RichTextPopup` (above) and the `FontRole::Chrome`
    // plumbing itself were already wired; this closes the
    // gap for the rest of `font_role::ChromePrimitive::ALL`: `Tree`,
    // `List`, `MenuBar`, `ContextMenu`, `CommandCenter`,
    // `MultiSectionView`, `SidebarPanel`, `StatusBar`, `ActivityBar`,
    // `Toolbar`, `TabBar`.
    //
    // Each test below fails first: pre-change, every one of these always
    // painted through the editor `dwrite` handle regardless of
    // `set_ui_font`, so two renders that differ *only* in chrome font
    // size were byte-identical. Diffing the whole painted frame (rather
    // than measuring one glyph's ink extent, `win_backend_dialog_paints_body_text_in_the_chrome_fonts_size`'s
    // approach above) is primitive-agnostic: it needs no per-primitive
    // knowledge of background colour or exact glyph bounds, just "this
    // primitive paints some real text".
    #[cfg(target_os = "windows")]
    fn chrome_font_diff_pixel_count(
        w: u32,
        h: u32,
        ui_font_a: &str,
        ui_font_b: &str,
        paint: impl Fn(&mut WinBackend),
    ) -> u32 {
        use crate::win::testing::HeadlessSurface;

        fn render(
            w: u32,
            h: u32,
            ui_font: &str,
            paint: &dyn Fn(&mut WinBackend),
        ) -> HeadlessSurface {
            let surface = HeadlessSurface::new(w, h).expect("create headless surface");
            let mut backend = WinBackend::new();
            // Editor font held fixed across both renders — only the
            // chrome (`ui_font`) one varies, same discipline
            // `win_backend_dialog_paints_body_text_in_the_chrome_fonts_size`
            // documents.
            backend.set_editor_font(DEFAULT_UI_FONT_FAMILY, 10.0);
            backend.set_ui_font(ui_font);
            backend
                .attach_headless(surface.target().clone(), w, h)
                .expect("attach headless surface");
            backend.begin_frame(Viewport::new(w as f32, h as f32, 1.0));
            paint(&mut backend);
            backend.end_frame();
            surface
        }
        let a = render(w, h, ui_font_a, &paint);
        let b = render(w, h, ui_font_b, &paint);
        let mut diff = 0u32;
        for y in 0..h {
            for x in 0..w {
                if a.pixel_at(x, y) != b.pixel_at(x, y) {
                    diff += 1;
                }
            }
        }
        diff
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_draw_tree_paints_row_text_in_the_chrome_fonts_size() {
        let tree = flat_tree(3);
        let diff = chrome_font_diff_pixel_count(
            200,
            120,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let _ = crate::backend::DrawsPrimitives::draw_dyn(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 120.0),
                    &tree,
                    &(),
                );
            },
        );
        assert!(
            diff > 0,
            "#1266: varying only the chrome font size must repaint different pixels — \
             the tree must paint row text through chrome_dwrite, not the (unchanged) \
             editor dwrite handle"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_draw_list_paints_row_text_in_the_chrome_fonts_size() {
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![
                crate::primitives::list::ListItem {
                    text: crate::types::StyledText::plain("alpha"),
                    icon: None,
                    detail: None,
                    decoration: crate::types::Decoration::Normal,
                },
                crate::primitives::list::ListItem {
                    text: crate::types::StyledText::plain("beta"),
                    icon: None,
                    detail: None,
                    decoration: crate::types::Decoration::Normal,
                },
            ],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            120,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                backend.draw_list(Rect::new(0.0, 0.0, 200.0, 120.0), &list);
            },
        );
        assert!(
            diff > 0,
            "#1266: draw_list must paint row text through chrome_dwrite, not the \
             (unchanged) editor dwrite handle"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_menu_bar_paints_item_labels_in_the_chrome_fonts_size() {
        use crate::backend::DrawsPrimitives;
        let bar = MenuBar {
            id: WidgetId::new("bar"),
            items: vec![crate::primitives::menu_bar::MenuBarItem {
                id: WidgetId::new("bar:file"),
                label: "&File".into(),
                disabled: false,
                submenu: None,
            }],
            open_item: None,
            focused_item: None,
        };
        let diff = chrome_font_diff_pixel_count(
            300,
            32,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let _ = backend.draw_dyn(Rect::new(0.0, 0.0, 300.0, 32.0), &bar, &());
            },
        );
        assert!(
            diff > 0,
            "the menu bar must paint item labels through chrome_dwrite, not the \
             (unchanged) editor dwrite handle"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_context_menu_paints_item_labels_in_the_chrome_fonts_size() {
        use crate::backend::DrawsPrimitives;
        let menu = ContextMenu {
            id: WidgetId::new("ctx"),
            items: vec![crate::primitives::context_menu::ContextMenuItem {
                id: Some(WidgetId::new("ctx:copy")),
                label: crate::types::StyledText::plain("copy"),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            100,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let _ = backend.draw_dyn(Rect::new(10.0, 10.0, 120.0, 80.0), &menu, &());
            },
        );
        assert!(
            diff > 0,
            "the context menu must paint item labels through chrome_dwrite, not the \
             (unchanged) editor dwrite handle"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_command_center_paints_the_search_label_in_the_chrome_fonts_size() {
        use crate::backend::DrawsPrimitives;
        let cc = crate::CommandCenter {
            id: WidgetId::new("cc"),
            back_enabled: true,
            forward_enabled: true,
            search_label: "project-name".into(),
        };
        let diff = chrome_font_diff_pixel_count(
            300,
            32,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let _ = backend.draw_dyn(Rect::new(0.0, 0.0, 300.0, 32.0), &cc, &());
            },
        );
        assert!(
            diff > 0,
            "#1266: CommandCenter must paint the search label through \
             chrome_dwrite, not the (unchanged) editor dwrite handle"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_draw_multi_section_view_paints_header_titles_in_the_chrome_fonts_size() {
        let view = crate::MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![crate::primitives::multi_section_view::Section {
                id: "alpha".into(),
                header: crate::primitives::multi_section_view::SectionHeader {
                    icon: None,
                    title: crate::types::StyledText::plain("alpha"),
                    badge: None,
                    actions: vec![],
                    show_chevron: true,
                },
                body: crate::primitives::multi_section_view::SectionBody::Text(vec![
                    crate::types::StyledText::plain("line one"),
                ]),
                aux: None,
                size: crate::primitives::multi_section_view::SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: crate::primitives::multi_section_view::Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: crate::primitives::multi_section_view::ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            150,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 150.0),
                    &view,
                );
            },
        );
        assert!(
            diff > 0,
            "a MultiSectionView must paint section header titles through \
             chrome_dwrite, not the (unchanged) editor dwrite handle"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_sidebar_panel_paints_the_toolbar_label_in_the_chrome_fonts_size() {
        let panel = crate::primitives::sidebar_panel::SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(crate::primitives::toolbar::Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("sb:refine"),
                    label: "Refine".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                }],
                bg: None,
                focused_index: None,
                ..Default::default()
            }),
            toolbar_height: None,
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            100,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_dyn(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 100.0),
                    &panel,
                    &(),
                );
            },
        );
        assert!(
            diff > 0,
            "the sidebar panel header must paint its toolbar label through \
             chrome_dwrite, not the (unchanged) editor dwrite handle"
        );
    }

    /// A palette painted through its widget contract measures and paints
    /// its text through `impl PaintSurface for WinBackend`, which prefers
    /// `chrome_dwrite`; this test pins that the item text follows the
    /// chrome font.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_draw_palette_paints_entries_in_the_chrome_fonts_size() {
        let palette = crate::Palette {
            id: WidgetId::new("palette:commands"),
            title: "Commands".to_string(),
            query: String::new(),
            query_cursor: 0,
            items: vec![crate::primitives::palette::PaletteItem {
                text: crate::types::StyledText::plain("Save File"),
                detail: None,
                icon: None,
                match_positions: Vec::new(),
                depth: 0,
                expandable: false,
                expanded: false,
            }],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 1,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: Default::default(),
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            120,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_dyn(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 120.0),
                    &palette,
                    &(),
                );
            },
        );
        assert!(
            diff > 0,
            "varying only the chrome font size must repaint different pixels — \
             the palette must paint item text through chrome_dwrite, not the \
             (unchanged) editor dwrite handle"
        );
    }

    /// Spinner paints through `surface_draw_text_run` (plain `self`, not
    /// a per-call adapter), so this is fixed purely by `impl PaintSurface
    /// for WinBackend`'s inverted default.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_draw_spinner_paints_label_in_the_chrome_fonts_size() {
        let spinner = Spinner {
            id: WidgetId::new("spinner"),
            label: "Indexing…".to_string(),
            frame_idx: 0,
            accent: None,
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            40,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let _ = backend.draw_spinner(Rect::new(0.0, 0.0, 200.0, 40.0), &spinner);
            },
        );
        assert!(
            diff > 0,
            "varying only the chrome font size must repaint different pixels — \
             draw_spinner must paint its label through chrome_dwrite, not the \
             (unchanged) editor dwrite handle"
        );
    }

    /// `Backend::set_theme` must reach `Self::draw_spinner`'s shared
    /// painter, not just be stored and ignored. A spinner's glyph is text (no solid-fill
    /// region to probe pixel-exact like `set_theme_reaches_the_rasterisers`
    /// does for the terminal divider), so this finds the most-inked pixel
    /// (max colour distance from an unpainted background sample) and checks
    /// it lands nearer the theme actually set on the backend than
    /// `Theme::default()` — robust to anti-aliasing.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_spinner() {
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 220;
        const H: u32 = 24;

        let custom_fg = Color::rgb(30, 160, 230);
        let default_fg = Theme::default().foreground;
        assert_ne!(
            custom_fg, default_fg,
            "test fixture bug: the probe colour must differ from the default theme's, or a \
             hardcoded `Theme::default()` would pass this test too"
        );

        let spinner = Spinner {
            id: WidgetId::new("spinner"),
            label: "Indexing…".to_string(),
            frame_idx: 0,
            accent: None,
        };
        // Rect offset from the surface's own origin so `(0, 0)` is a
        // reliable unpainted background sample.
        let rect = Rect::new(10.0, 4.0, 0.0, 0.0);

        let most_inked = |theme: Theme| -> (u8, u8, u8) {
            let surface = HeadlessSurface::new(W, H).expect("create headless surface");
            let mut backend = WinBackend::new();
            backend
                .attach_headless(surface.target().clone(), W, H)
                .expect("attach headless surface");
            backend.set_theme(theme);

            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            let _ = backend.draw_spinner(rect, &spinner);
            backend.end_frame();

            let bg = surface.pixel_at(0, 0);
            let bg = (bg.r, bg.g, bg.b);
            let mut best = bg;
            let mut best_d = -1i32;
            for y in 0..H {
                for x in 0..W {
                    let px = surface.pixel_at(x, y);
                    let c = (px.r, px.g, px.b);
                    let d = (c.0 as i32 - bg.0 as i32).pow(2)
                        + (c.1 as i32 - bg.1 as i32).pow(2)
                        + (c.2 as i32 - bg.2 as i32).pow(2);
                    if d > best_d {
                        best_d = d;
                        best = c;
                    }
                }
            }
            best
        };

        let dist = |a: (u8, u8, u8), c: Color| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
            d(a.0, c.r) + d(a.1, c.g) + d(a.2, c.b)
        };

        let custom_ink = most_inked(Theme {
            foreground: custom_fg,
            ..Theme::default()
        });
        assert!(
            dist(custom_ink, custom_fg) < dist(custom_ink, default_fg),
            "most-inked pixel {custom_ink:?} painted after `set_theme` with a non-default \
             foreground should be closer to that theme's foreground {custom_fg:?} than to \
             `Theme::default()`'s {default_fg:?}",
        );
    }

    /// The editor-class twin of the three tests above: `TextDisplay`
    /// paints through `EditorSurface`, so unlike `Palette`/
    /// `Spinner` its rendered pixels must stay identical regardless of
    /// the chrome font size — only the editor font (held fixed by
    /// `chrome_font_diff_pixel_count`) should affect it.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_text_display_does_not_vary_with_chrome_font_size() {
        let td = crate::TextDisplay {
            id: WidgetId::new("log"),
            lines: vec![crate::primitives::text_display::TextDisplayLine {
                spans: vec![crate::types::StyledSpan::plain("hello world")],
                decoration: crate::types::Decoration::Normal,
                timestamp: None,
            }],
            scroll_offset: 0,
            auto_scroll: true,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            40,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 40.0),
                    &td,
                );
            },
        );
        assert_eq!(
            diff, 0,
            "a text display is editor-class — it must keep painting through \
             EditorSurface (the editor dwrite handle) regardless of the chrome \
             font's size"
        );
    }

    /// `Terminal`'s paint through `EditorSurface` — see
    /// [`win_backend_text_display_does_not_vary_with_chrome_font_size`]'s
    /// doc for why this must stay pixel-identical across chrome font
    /// sizes.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_terminal_does_not_vary_with_chrome_font_size() {
        let term = crate::Terminal {
            id: WidgetId::new("terminal"),
            cells: vec![vec![crate::primitives::terminal::TerminalCell {
                text: "Z".to_string(),
                fg: crate::Color::rgb(220, 220, 220),
                bg: crate::Color::rgb(30, 30, 30),
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                selected: false,
                is_cursor: false,
                cursor_shape: crate::primitives::terminal::TerminalCursorShape::Block,
                cursor_blinking: false,
                is_find_match: false,
                is_find_active: false,
            }]],
            scrollbar: None,
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            40,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 40.0),
                    &term,
                );
            },
        );
        assert_eq!(
            diff, 0,
            "the terminal is editor-class — it must keep painting cell glyphs \
             through EditorSurface (the editor dwrite handle) regardless of the \
             chrome font's size"
        );
    }

    /// `DiffView`'s call site through `EditorSurface` — same contract as
    /// [`win_backend_terminal_does_not_vary_with_chrome_font_size`]
    /// above.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_draw_diff_view_does_not_vary_with_chrome_font_size() {
        let view = crate::primitives::diff_view::DiffView {
            id: WidgetId::new("diff"),
            left: "hello world".to_string(),
            right: "other".to_string(),
            left_label: None,
            right_label: None,
            hunks: vec![crate::primitives::diff_view::DiffHunk {
                left_start: 1,
                right_start: 1,
                rows: vec![crate::primitives::diff_view::DiffRow {
                    left: Some("hello world".to_string()),
                    right: Some("other".to_string()),
                    kind: crate::primitives::diff_view::DiffRowKind::Changed,
                }],
            }],
            mode: crate::primitives::diff_view::DiffMode::SideBySide,
            editability: crate::primitives::diff_view::DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: crate::primitives::diff_view::DiffPane::Left,
            has_focus: false,
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            80,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let _ = crate::DrawsPrimitives::draw_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 80.0),
                    &view,
                );
            },
        );
        assert_eq!(
            diff, 0,
            "a DiffView is editor-class — it must keep painting through \
             EditorSurface (the editor dwrite handle) regardless of the chrome \
             font's size"
        );
    }

    /// A `DataTable` paints through `impl PaintSurface for WinBackend`,
    /// which must resolve `chrome_dwrite` for chrome text — this test pins
    /// that.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_draw_data_table_paints_cell_text_in_the_chrome_fonts_size() {
        let table = crate::DataTable {
            id: WidgetId::new("table"),
            columns: vec![crate::Column {
                title: "col".to_string(),
                width: crate::ColumnWidth::Flex(1.0),
                align: crate::ColumnAlign::Left,
            }],
            rows: vec![crate::DataRow {
                cells: vec![crate::types::StyledText::plain("alpha")],
                decoration: crate::types::Decoration::Normal,
            }],
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            bordered: false,
            striped: false,
            glyphs: None,
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            120,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let _ = crate::DrawsPrimitives::draw_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 120.0),
                    &table,
                );
            },
        );
        assert!(
            diff > 0,
            "varying only the chrome font size must repaint different pixels — \
             a DataTable must paint cell text through chrome_dwrite, not the \
             (unchanged) editor dwrite handle"
        );
    }

    /// A `Form`'s `FieldKind::Toolbar` row paints its embedded toolbar
    /// through the toolbar's own contract on `WinBackend`'s
    /// chrome-resolving `PaintSurface`, so the button label follows
    /// `chrome_dwrite`.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_draw_form_paints_toolbar_button_label_in_the_chrome_fonts_size() {
        let form = crate::Form {
            id: WidgetId::new("settings"),
            fields: vec![crate::FormField {
                id: WidgetId::new("settings:toolbar"),
                label: crate::types::StyledText::default(),
                kind: crate::FieldKind::Toolbar(crate::Toolbar {
                    id: WidgetId::new("settings:toolbar:bar"),
                    buttons: vec![ToolbarButton::Action {
                        spec: None,
                        id: WidgetId::new("settings:toolbar:refine"),
                        label: "Refine".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    }],
                    bg: None,
                    focused_index: None,
                    ..Default::default()
                }),
                hint: crate::types::StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            80,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_dyn(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 80.0),
                    &form,
                    &(),
                );
            },
        );
        assert!(
            diff > 0,
            "varying only the chrome font size must repaint different pixels — \
             the form must paint the toolbar button's label through \
             chrome_dwrite, not the (unchanged) editor dwrite handle"
        );
    }

    /// Same chrome-font check as the data table test above.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_toast_overlay_paints_title_in_the_chrome_fonts_size() {
        let stack = crate::ToastOverlay {
            id: WidgetId::new("toast-stack"),
            corner: crate::ToastCorner::BottomRight,
            toasts: vec![crate::Toast {
                id: WidgetId::new("toast"),
                title: "Saved".to_string(),
                body: String::new(),
                severity: crate::ToastSeverity::Info,
                actions: vec![],
                accent: None,
            }],
            focus: None,
        };
        let diff = chrome_font_diff_pixel_count(
            360,
            120,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 360.0, 120.0),
                    &stack,
                );
            },
        );
        assert!(
            diff > 0,
            "varying only the chrome font size must repaint different pixels — \
             a toast overlay must paint the toast's title through \
             chrome_dwrite (WinBackend's own PaintSurface default), not the \
             editor dwrite handle"
        );
    }

    /// Same chrome-font check as the data table test above.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_text_input_paints_text_in_the_chrome_fonts_size() {
        let mut ti = crate::TextInput::new(WidgetId::new("ti"));
        ti.lines = vec!["hello world".to_string()];
        ti.has_focus = true;
        let diff = chrome_font_diff_pixel_count(
            200,
            40,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 40.0),
                    &ti,
                );
            },
        );
        assert!(
            diff > 0,
            "varying only the chrome font size must repaint different pixels — \
             a text input must paint its text through chrome_dwrite, not the \
             (unchanged) editor dwrite handle"
        );
    }

    /// Same "builds its own adapter from the passed `dwrite`" shape as
    /// the data table test above, painted into the box `Tooltip::layout`
    /// places for an anchor.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_tooltip_paints_text_in_the_chrome_fonts_size() {
        let mut tooltip = crate::Tooltip::new(WidgetId::new("tip"), "Explain this control");
        tooltip.placement = crate::TooltipPlacement::Bottom;
        let anchor = Rect::new(10.0, 4.0, 60.0, 20.0);
        let viewport = Rect::new(0.0, 0.0, 200.0, 120.0);
        let measure = crate::TooltipMeasure::new(160.0, 60.0);
        let layout = tooltip.layout(anchor, viewport, measure, 4.0);
        let diff = chrome_font_diff_pixel_count(
            200,
            120,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_primitive(backend, layout.bounds, &tooltip);
            },
        );
        assert!(
            diff > 0,
            "varying only the chrome font size must repaint different pixels — \
             a tooltip must paint its text \
             through chrome_dwrite, not the (unchanged) editor dwrite handle"
        );
    }

    /// Same chrome-font check as the data table test above.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_completions_paints_item_labels_in_the_chrome_fonts_size() {
        let item = |label: &str| crate::CompletionItem {
            label: crate::types::StyledText::plain(label),
            detail: None,
            documentation: None,
            kind: Default::default(),
            icon: None,
        };
        let completions = crate::Completions {
            id: WidgetId::new("comp"),
            items: vec![item("alpha"), item("beta")],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        // The popup's already-placed box, tall enough for a row at either
        // chrome size, so both renders paint a label.
        let popup = Rect::new(10.0, 10.0, 180.0, 100.0);
        let diff = chrome_font_diff_pixel_count(
            200,
            120,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_primitive(backend, popup, &completions);
            },
        );
        assert!(
            diff > 0,
            "varying only the chrome font size must repaint different pixels — \
             completions must paint item labels through chrome_dwrite, not \
             the (unchanged) editor dwrite handle"
        );
    }

    /// Same chrome-font check as the data table test above.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_draw_message_list_paints_row_text_in_the_chrome_fonts_size() {
        let list = crate::MessageList {
            id: WidgetId::new("messages"),
            rows: vec![crate::MessageRow::new(
                "hello world",
                crate::Color::rgb(255, 255, 255),
                0.0,
            )],
            scroll_top: 0,
        };
        let diff = chrome_font_diff_pixel_count(
            200,
            80,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 200.0, 80.0),
                    &list,
                );
            },
        );
        assert!(
            diff > 0,
            "varying only the chrome font size must repaint different pixels — \
             a MessageList must paint row text through chrome_dwrite, not \
             the (unchanged) editor dwrite handle"
        );
    }

    // ── no-paint `*_layout` twins: pure measurement, proven via a
    // measured value rather than painted pixels ─────────────────────
    //
    // Each of the four methods below has its own `chrome_dwrite`-
    // preferring swap, independent of the paint-side swap its
    // `draw_*` twin above carries (see each method's own doc for why
    // both must resolve the same handle). `chrome_font_diff_layout`
    // attaches a live chrome/editor `DWrite` pair the same way
    // `chrome_font_diff_pixel_count` does, but returns a measured
    // value from each run instead of a rendered frame — the right
    // oracle for a method that computes geometry without painting
    // anything.

    /// Measurement-twin of `chrome_font_diff_pixel_count` (see that fn's
    /// doc): attaches a live chrome/editor `DWrite` pair — same
    /// discipline, editor font held fixed, only the chrome `ui_font`
    /// varies — and returns whatever `measure` computes from each, for a
    /// `*_layout` twin that measures geometry without painting.
    #[cfg(target_os = "windows")]
    fn chrome_font_diff_layout<T>(
        ui_font_a: &str,
        ui_font_b: &str,
        measure: impl Fn(&WinBackend) -> T,
    ) -> (T, T) {
        use crate::win::testing::HeadlessSurface;

        fn make<T>(ui_font: &str, measure: &dyn Fn(&WinBackend) -> T) -> T {
            let w = 64u32;
            let h = 64u32;
            let surface = HeadlessSurface::new(w, h).expect("create headless surface");
            let mut backend = WinBackend::new();
            backend.set_editor_font(DEFAULT_UI_FONT_FAMILY, 10.0);
            backend.set_ui_font(ui_font);
            backend
                .attach_headless(surface.target().clone(), w, h)
                .expect("attach headless surface");
            measure(&backend)
        }
        (make(ui_font_a, &measure), make(ui_font_b, &measure))
    }

    /// A `DataTable` layout must measure a `ColumnWidth::Content` column
    /// against the same `chrome_dwrite`-preferring handle the table
    /// paints with.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_data_table_layout_measures_a_content_column_in_the_chrome_fonts_size() {
        let table = crate::DataTable {
            id: WidgetId::new("table"),
            columns: vec![
                crate::Column {
                    title: "a rather long column title".to_string(),
                    width: crate::ColumnWidth::Content {
                        min: 0.0,
                        max: 4000.0,
                    },
                    align: crate::ColumnAlign::Left,
                },
                // The layout's last column absorbs any slack up to
                // `viewport_width`, so the measured column must not be last
                // or both runs read back the full viewport width
                // regardless of font.
                crate::Column {
                    title: String::new(),
                    width: crate::ColumnWidth::FixedPx(0.0),
                    align: crate::ColumnAlign::Left,
                },
            ],
            rows: vec![],
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            bordered: false,
            striped: false,
            glyphs: None,
        };
        let (small, big) = chrome_font_diff_layout(
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                // `remaining` (here `viewport_width`, since there's no
                // scrollbar) clamps a `Content` column's measured width
                // — wide enough that even the 40pt run's real measured
                // width never saturates that clamp, or both runs would
                // read back the same clamped-to-`remaining` value
                // regardless of font.
                crate::DrawsPrimitives::layout_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 4000.0, 200.0),
                    &table,
                )
                .layout
                .columns[0]
                    .width
            },
        );
        assert!(
            big > small * 2.0,
            "a DataTable layout must measure the Content column through \
             chrome_dwrite, not the (unchanged) editor dwrite handle: \
             small={small}, big={big}"
        );
    }

    /// A form's widget-contract layout must measure a `FieldKind::Toolbar`
    /// item against the same `chrome_dwrite`-preferring handle the paint
    /// uses.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_form_layout_measures_a_toolbar_item_in_the_chrome_fonts_size() {
        let form = crate::Form {
            id: WidgetId::new("settings"),
            fields: vec![crate::FormField {
                id: WidgetId::new("settings:toolbar"),
                label: crate::types::StyledText::default(),
                kind: crate::FieldKind::Toolbar(crate::Toolbar {
                    id: WidgetId::new("settings:toolbar:bar"),
                    buttons: vec![ToolbarButton::Action {
                        spec: None,
                        id: WidgetId::new("settings:toolbar:refine"),
                        label: "Refine".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                    }],
                    bg: None,
                    focused_index: None,
                    ..Default::default()
                }),
                hint: crate::types::StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let (small, big) = chrome_font_diff_layout(
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let flayout = crate::DrawsPrimitives::layout_primitive(
                    &*backend,
                    Rect::new(0.0, 0.0, 300.0, 80.0),
                    &form,
                )
                .layout;
                flayout.visible_fields[0].item_bounds[0].1.width
            },
        );
        assert!(
            big > small * 2.0,
            "the form layout must measure the toolbar button through \
             chrome_dwrite, not the (unchanged) editor dwrite handle: \
             small={small}, big={big}"
        );
    }

    /// A toast overlay's no-paint layout must wrap the toast's body
    /// against the same chrome font the overlay paints with.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_toast_overlay_layout_wraps_body_in_the_chrome_fonts_size() {
        let stack = crate::ToastOverlay {
            id: WidgetId::new("toast-stack"),
            corner: crate::ToastCorner::BottomRight,
            toasts: vec![crate::Toast {
                id: WidgetId::new("toast"),
                title: "Saved".to_string(),
                body: "one two three four five six seven eight nine ten eleven twelve".to_string(),
                severity: crate::ToastSeverity::Info,
                actions: vec![],
                accent: None,
            }],
            focus: None,
        };
        let (small_h, big_h) = chrome_font_diff_layout(
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::layout_primitive(
                    backend,
                    Rect::new(0.0, 0.0, 400.0, 400.0),
                    &stack,
                )
                .layout
                .visible_toasts[0]
                    .bounds
                    .height
            },
        );
        assert!(
            big_h > small_h,
            "the toast overlay layout must wrap the toast body through chrome_dwrite, \
             not the (unchanged) editor dwrite handle: small_h={small_h}, \
             big_h={big_h}"
        );
    }

    /// `spinner_layout` must measure the spinner's label against the
    /// same `chrome_dwrite`-preferring handle `Self::draw_spinner` paints
    /// with — see that method's doc.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_spinner_layout_measures_label_in_the_chrome_fonts_size() {
        let spinner = Spinner {
            id: WidgetId::new("spinner"),
            label: "Indexing…".to_string(),
            frame_idx: 0,
            accent: None,
        };
        let (small, big) = chrome_font_diff_layout(
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                backend
                    .spinner_layout(Rect::new(0.0, 0.0, 200.0, 40.0), &spinner)
                    .bounds
                    .width
            },
        );
        assert!(
            big > small * 2.0,
            "spinner_layout must measure the label through chrome_dwrite, not \
             the (unchanged) editor dwrite handle: small={small}, big={big}"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_status_bar_paints_segment_text_in_the_chrome_fonts_size() {
        let bar = StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![crate::primitives::status_bar::StatusBarSegment {
                text: "main.rs".into(),
                fg: crate::types::Color::rgb(255, 255, 255),
                bg: crate::types::Color::rgb(0, 0, 0),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let diff = chrome_font_diff_pixel_count(
            300,
            24,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let _ = crate::DrawsPrimitives::draw_dyn(
                    backend,
                    Rect::new(0.0, 0.0, 300.0, 24.0),
                    &bar,
                    &(),
                );
            },
        );
        assert!(
            diff > 0,
            "a status bar must paint segment text through chrome_dwrite, not \
             the editor dwrite handle"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_toolbar_paints_button_labels_in_the_chrome_fonts_size() {
        use crate::backend::DrawsPrimitives;
        let bar = crate::primitives::toolbar::Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                spec: None,
                id: WidgetId::new("tb:on"),
                label: "On".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
            ..Default::default()
        };
        let diff = chrome_font_diff_pixel_count(
            100,
            32,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                let _ = backend.draw_dyn(Rect::new(0.0, 0.0, 100.0, 32.0), &bar, &());
            },
        );
        assert!(
            diff > 0,
            "the toolbar must paint button labels through chrome_dwrite, not the \
             (unchanged) editor dwrite handle"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn win_backend_tab_bar_paints_tab_labels_in_the_chrome_fonts_size() {
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![crate::primitives::tab_bar::TabItem {
                label: "main.rs".into(),
                is_active: true,
                is_dirty: false,
                is_preview: false,
                is_closable: true,
            }],
            scroll_offset: 0,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: true,
            compact: false,
            ..Default::default()
        };
        let diff = chrome_font_diff_pixel_count(
            300,
            30,
            &format!("{DEFAULT_UI_FONT_FAMILY} 6.0"),
            &format!("{DEFAULT_UI_FONT_FAMILY} 40.0"),
            |backend| {
                crate::DrawsPrimitives::draw_dyn(
                    backend,
                    Rect::new(0.0, 0.0, 300.0, 30.0),
                    &bar,
                    &(),
                );
            },
        );
        assert!(
            diff > 0,
            "the tab strip must paint its labels through chrome_dwrite, not the \
             (unchanged) editor dwrite handle"
        );
    }

    /// `surface_draw_icon_glyph` forwards to `surface_draw_text_run`
    /// on this backend (DirectWrite's font-fallback cascade is already
    /// baked into every `IDWriteTextFormat` — see the trait override's
    /// doc) — this proves that forward actually paints real ink rather
    /// than silently no-opping. Win-GUI twin of
    /// `gtk_backend_paint_surface_draw_icon_glyph_paints_real_ink`.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_paint_surface_draw_icon_glyph_paints_real_ink() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 40;
        const H: u32 = 40;

        let background = cleared_frame_background(W, H);
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let white = Color::rgb(255, 255, 255);
        assert_ne!(
            background,
            (white.r, white.g, white.b),
            "the glyph colour must differ from the frame's clear colour, or \
             'it painted ink' is unprovable"
        );
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.surface_draw_icon_glyph(Rect::new(2.0, 2.0, 30.0, 30.0), "i", white);
        backend.end_frame();

        // Pixels the glyph moved off `begin_frame`'s clear colour — not a
        // channel sum against black, which every cleared pixel would pass
        // (see `cleared_frame_background`).
        let mut ink = 0u32;
        for y in 0..H {
            for x in 0..W {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != background {
                    ink += 1;
                }
            }
        }
        assert!(
            ink > 0,
            "surface_draw_icon_glyph must paint real ink through the fallback-baked \
             DirectWrite text format, not silently no-op"
        );
    }

    /// Exercise every remaining `PaintSurface` method at least once end-to-end
    /// (frame lifecycle, measurement, stroke, line, clip push/pop, text run,
    /// rounded rect, alpha fill, role text, icon glyph, image) so an
    /// implementation bug (wrong arg order, a missing surface guard, a panic
    /// inside the `todo!()` split) fails a test instead of shipping silently.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_paint_surface_verbs_do_not_panic() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 64;
        const H: u32 = 64;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let viewport = Viewport::new(W as f32, H as f32, 1.0);
        backend.surface_begin_frame(viewport);
        assert_eq!(
            backend.surface_viewport(),
            viewport,
            "surface_viewport must forward to Backend::begin_frame's stored value"
        );
        assert_eq!(
            backend.surface_line_height(),
            Backend::line_height(&backend)
        );
        assert_eq!(backend.surface_char_width(), Backend::char_width(&backend));

        let blue = Color::rgb(20, 20, 200);
        let white = Color::rgb(255, 255, 255);
        backend.surface_stroke_rect(Rect::new(0.0, 0.0, 40.0, 40.0), blue, 2.0);
        backend.surface_draw_line(Point::new(0.0, 0.0), Point::new(40.0, 40.0), blue, 1.0);
        backend.surface_push_clip(Rect::new(0.0, 0.0, 40.0, 40.0));
        backend.surface_draw_text_run(Rect::new(2.0, 2.0, 30.0, 10.0), "hi", white);
        backend.surface_pop_clip();
        // The four new verbs, exercised here too (pixel-precision
        // coverage of each lives in the dedicated tests above).
        backend.surface_fill_rounded_rect(Rect::new(44.0, 0.0, 16.0, 16.0), 4.0, blue);
        backend.surface_fill_rect_alpha(Rect::new(44.0, 20.0, 16.0, 16.0), blue, 0.5);
        backend.surface_draw_text_run_with_role(
            Rect::new(2.0, 14.0, 30.0, 10.0),
            "hi",
            white,
            crate::FontRole::Chrome,
            false,
        );
        backend.surface_draw_icon_glyph(Rect::new(2.0, 26.0, 10.0, 10.0), "i", white);

        let (w, h) = backend.surface_measure_text("hi");
        assert!(
            w > 0.0 && h > 0.0,
            "surface_measure_text must report a nonzero footprint for non-empty \
             text: got ({w}, {h})"
        );

        let image = crate::primitives::image::Image {
            id: WidgetId::new("test:native-surface-image"),
            source: crate::primitives::image::ImageSource::Bytes(Vec::new()),
            intrinsic_size: Some((8, 8)),
            fit: crate::primitives::image::ImageFit::Contain,
            fallback_text: "[i]".to_string(),
        };
        // Direct2D bitmap decode fails on empty bytes on the real path
        // too — this call only needs to prove it reaches the decoder, not
        // any particular decode outcome.
        let _ = backend.surface_draw_image(Rect::new(0.0, 40.0, 8.0, 8.0), &image);

        backend.surface_end_frame();
    }

    // ── Backend::push_clip/pop_clip ─────────────────────────────────────
    //
    // `WinBackend::push_clip`/`pop_clip` forward straight to this same
    // module's `surface_push_clip`/`surface_pop_clip` (Direct2D's own
    // `PushAxisAlignedClip`/`PopAxisAlignedClip` stack). These two
    // exercise the `Backend` trait entry point itself — the seam a
    // backend-generic caller (binding-core's layout painter, a future
    // `ScrollContainer`) reaches for — rather than the `PaintSurface` one
    // in-tree primitives call directly, and pin both that an overflowing
    // fill is confined to the clip's exact pixel edge and that a nested
    // pop restores the outer clip rather than dropping it.

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_push_clip_confines_an_overflowing_fill_to_the_clipped_pixels() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 40;
        const H: u32 = 40;

        let background = cleared_frame_background(W, H);
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let red = Color::rgb(200, 20, 20);
        assert_ne!(
            background,
            (red.r, red.g, red.b),
            "the fill colour must differ from the frame's clear colour, or \
             'this pixel stayed untouched' is unprovable"
        );

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        Backend::push_clip(&mut backend, Rect::new(0.0, 0.0, 10.0, 10.0));
        crate::backend::DrawsPrimitives::draw_dyn(
            &mut backend,
            Rect::new(0.0, 0.0, W as f32, H as f32),
            &crate::testing::solid_canvas(Rect::new(0.0, 0.0, W as f32, H as f32), red),
            &(),
        );
        Backend::pop_clip(&mut backend);
        backend.end_frame();

        let inside = surface.pixel_at(9, 9);
        assert_eq!(
            (inside.r, inside.g, inside.b),
            (red.r, red.g, red.b),
            "the last pixel inside the pushed clip must keep the overflowing fill"
        );
        let outside = surface.pixel_at(10, 10);
        assert_eq!(
            (outside.r, outside.g, outside.b),
            background,
            "the first pixel outside the pushed clip must stay untouched by the \
             fill, which reaches well past it"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_nested_pop_clip_restores_the_outer_clip() {
        use crate::types::Color;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 40;
        const H: u32 = 40;

        let background = cleared_frame_background(W, H);
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let green = Color::rgb(0, 200, 0);
        let white = Color::rgb(255, 255, 255);
        assert_ne!(background, (green.r, green.g, green.b));
        assert_ne!(background, (white.r, white.g, white.b));

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        // Outer clip: pixels (0,0)..(20,20).
        Backend::push_clip(&mut backend, Rect::new(0.0, 0.0, 20.0, 20.0));
        // Inner clip, nested inside the outer one: pixels (5,5)..(15,15).
        Backend::push_clip(&mut backend, Rect::new(5.0, 5.0, 10.0, 10.0));
        Backend::pop_clip(&mut backend);
        // Paint only after the inner pop: if it restored the outer clip,
        // this reaches (0,0)..(20,20) but not past it; if it dropped
        // clipping entirely, it would reach the whole 40x40 surface,
        // including (30,30) checked below.
        crate::backend::DrawsPrimitives::draw_dyn(
            &mut backend,
            Rect::new(0.0, 0.0, W as f32, H as f32),
            &crate::testing::solid_canvas(Rect::new(0.0, 0.0, W as f32, H as f32), green),
            &(),
        );
        Backend::pop_clip(&mut backend);
        // Paint only after the outer pop, confined to pixels
        // (20,20)..(40,40) — entirely outside the now-popped outer clip,
        // so this fill can only land there if the outer pop actually
        // cleared clipping rather than leaving the outer rect active.
        crate::backend::DrawsPrimitives::draw_dyn(
            &mut backend,
            Rect::new(20.0, 20.0, 20.0, 20.0),
            &crate::testing::solid_canvas(Rect::new(20.0, 20.0, 20.0, 20.0), white),
            &(),
        );
        backend.end_frame();

        let inside_outer = surface.pixel_at(17, 17);
        assert_eq!(
            (inside_outer.r, inside_outer.g, inside_outer.b),
            (green.r, green.g, green.b),
            "inside the outer clip but outside the inner one, the inner pop must have \
             restored the outer clip rather than leaving it narrowed to the inner rect, \
             letting the post-pop fill land here"
        );
        let outside_outer = surface.pixel_at(35, 5);
        assert_eq!(
            (outside_outer.r, outside_outer.g, outside_outer.b),
            background,
            "outside the outer clip entirely, the fill painted after the inner pop \
             must not have reached here — proving that pop restored the outer clip \
             rather than dropping clipping entirely"
        );
        let after_outer_pop = surface.pixel_at(30, 30);
        assert_eq!(
            (after_outer_pop.r, after_outer_pop.g, after_outer_pop.b),
            (white.r, white.g, white.b),
            "the fill painted after the outer pop must reach here, proving that pop \
             cleared the clip rather than leaving the outer rect active"
        );
    }

    // ── Chart real-pixel driver tests ─────────────────────────────────────────
    //
    // Drive a chart through `draw_primitive` end to end, the same shape `set_theme_reaches_the_find_replace` above uses.

    #[cfg(target_os = "windows")]
    fn chart_line_fixture(data: Vec<f64>) -> crate::primitives::chart::Chart {
        use crate::primitives::chart::{ChartKind, Series};
        crate::primitives::chart::Chart {
            id: WidgetId::new("chart"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "A".into(),
                data,
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: None,
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
            hovered_point: None,
            crosshair_x: None,
        }
    }

    /// Paint↔hover round trip: a nearest-point lookup at a data point's
    /// resolved screen position must resolve back to that point, and
    /// painting with it as `hovered_point` must not panic.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_chart_paint_and_nearest_point_round_trip() {
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 200;
        const H: u32 = 100;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let chart = chart_line_fixture(vec![1.0, 5.0, 2.0, 8.0, 3.0]);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &chart).chart;
        backend.end_frame();

        assert_eq!(layout.data_point_positions.len(), 5);
        for &(si, di, x, y) in &layout.data_point_positions {
            let nearest = layout.nearest_point(x, y, 1.0);
            assert_eq!(nearest, Some((si, di)));
        }

        // Painting with a hover marker + crosshair must not panic.
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let hovered = crate::primitives::chart::Chart {
            hovered_point: Some((0, 0)),
            crosshair_x: Some(1.0),
            ..chart.clone()
        };
        crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &hovered);
        backend.end_frame();
    }

    /// The no-paint layout must agree byte-for-byte with what the paint
    /// returned — the same contract `primitives::form::paint` documents
    /// ("paint and hit-test can never disagree").
    #[cfg(target_os = "windows")]
    #[test]
    fn win_chart_no_paint_layout_matches_paint_layout() {
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 200;
        const H: u32 = 100;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let chart = chart_line_fixture(vec![1.0, 4.0, 2.0]);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let painted = crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &chart).chart;
        backend.end_frame();

        let no_paint = crate::DrawsPrimitives::layout_primitive(&backend, rect, &chart).chart;
        assert_eq!(painted, no_paint);
    }

    /// The shared chart painter brackets its body in
    /// `surface_push_clip`/`surface_pop_clip`, so a hover marker never
    /// escapes the chart's rect. Mirrors
    /// `gtk::backend::tests::gtk_chart_hover_marker_does_not_escape_chart_rect`
    /// pixel-for-pixel (same fixture, same geometry).
    #[cfg(target_os = "windows")]
    #[test]
    fn win_chart_hover_marker_does_not_escape_chart_rect() {
        use crate::primitives::chart::{ChartKind, Series};
        use crate::win::testing::HeadlessSurface;
        use crate::Color;

        // origin=1.0 at index 0 → top edge; 0.0 at index 1 → bottom
        // edge. n=2 → index 0's x is the plot area's left edge. So data
        // point (0, 0) sits exactly at the chart's top-left corner.
        let chart = crate::primitives::chart::Chart {
            id: WidgetId::new("chart"),
            kind: ChartKind::Sparkline,
            series: vec![Series {
                label: "a".into(),
                data: vec![10.0, 0.0],
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: Some((0.0, 10.0)),
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
            hovered_point: None,
            crosshair_x: None,
        };

        const CANVAS_W: u32 = 40;
        const CANVAS_H: u32 = 40;
        let sentinel = Color::rgb(0, 0, 0);
        let (chart_x, chart_y, chart_w, chart_h) = (10.0_f32, 10.0_f32, 20.0_f32, 20.0_f32);

        let surface = HeadlessSurface::new(CANVAS_W, CANVAS_H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), CANVAS_W, CANVAS_H)
            .expect("attach headless surface");

        backend.begin_frame(Viewport::new(CANVAS_W as f32, CANVAS_H as f32, 1.0));
        backend.surface_fill_rect(
            Rect::new(0.0, 0.0, CANVAS_W as f32, CANVAS_H as f32),
            sentinel,
        );
        let chart = crate::primitives::chart::Chart {
            hovered_point: Some((0, 0)),
            ..chart
        };
        crate::DrawsPrimitives::draw_primitive(
            &mut backend,
            Rect::new(chart_x, chart_y, chart_w, chart_h),
            &chart,
        );
        backend.end_frame();

        // Inside the marker's 8-unit-radius ring, centred at the chart's
        // own top-left corner (10, 10), but outside the chart's rect.
        let px = surface.pixel_at(5, 5);
        assert_eq!(
            (px.r, px.g, px.b),
            (sentinel.r, sentinel.g, sentinel.b),
            "hover marker must not paint outside the chart's own rect",
        );
    }

    /// `WinBackend::surface_measure_text_styled`'s bold override must ask
    /// DirectWrite for a wider bold measurement, not just a synthetic bonus.
    /// Mirrors `gtk::backend::tests::gtk_backend_draw_status_bar_bold_segment_measures_wider`:
    /// paints the same text through the widget contract once
    /// bold, once not, and asserts the bold segment's resolved (painted) width is
    /// strictly wider.
    ///
    /// **Both the editor and chrome fonts are overridden on purpose — don't drop
    /// either call.** `PaintSurface::surface_measure_text_styled` measures through
    /// `self.dwrite` (the editor font); a status bar measures/paints through
    /// `chrome_dwrite` instead. Both must be set to the
    /// *same* `FONT`/`SIZE_PT` or the comparison would drift.
    ///
    /// `DEFAULT_EDITOR_FONT_FAMILY` (`"Consolas"`) is monospaced and shares **one
    /// advance width across every weight** by design, so `measure_text_styled(t, true)`
    /// and `measure_text_styled(t, false)` return identical widths there regardless
    /// of how faithfully the `bold` flag is threaded through. This test drives a
    /// *proportional* font (`"Segoe UI"` — the `DEFAULT_UI_FONT_FAMILY`) where
    /// "bold is wider" is a property the font can actually express.
    ///
    /// This chrome primitive measures through `chrome_dwrite`, so the fixture sets
    /// `set_ui_font` to match the bold-override font.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_status_bar_bold_segment_measures_wider() {
        // `DWrite` (and `DEFAULT_UI_FONT_FAMILY` below) come from this
        // module's own `use super::*` — both are `cfg(target_os =
        // "windows")` items, same as this test.
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::win::testing::HeadlessSurface;
        use crate::Color;

        const W: u32 = 400;
        const H: u32 = 20;
        const TEXT: &str = "READY";
        // Proportional, and present on every supported Windows — see this
        // test's doc for why the default (monospaced) editor font cannot
        // express the property under test.
        const FONT: &str = DEFAULT_UI_FONT_FAMILY;
        // Deliberately larger than `DEFAULT_UI_FONT_SIZE_PT`: the
        // bold/regular advance-width delta scales with the em size, so a
        // bigger size keeps the difference comfortably above float noise.
        const SIZE_PT: f32 = 20.0;

        // Fixture guard: if DirectWrite itself reports the same width for
        // both weights of `FONT`, the assertion below would be
        // unsatisfiable and its failure would say nothing about the
        // override under test. Fail with a message that names *that*
        // instead.
        let (probe, _, _) = DWrite::new(FONT, SIZE_PT, None).expect("create probe DWrite");
        let (probe_regular, _) = probe
            .measure_text_styled(TEXT, false)
            .expect("measure regular");
        let (probe_bold, _) = probe.measure_text_styled(TEXT, true).expect("measure bold");
        assert!(
            probe_bold > probe_regular,
            "test fixture bug: `{FONT}` must measure bold text wider than regular for this \
             assertion to mean anything (regular={probe_regular}, bold={probe_bold}) — pick a \
             proportional font that distinguishes weights"
        );

        let width_for = |bold: bool| {
            let surface = HeadlessSurface::new(W, H).expect("create headless surface");
            let mut backend = WinBackend::new();
            // Must precede `attach_headless`: that's where the
            // `IDWriteTextFormat` pair `self.dwrite` wraps is built from
            // `editor_font_family`/`editor_font_size_pt` (see
            // `set_editor_font`'s doc — a live surface doesn't rebuild
            // them on a later font change).
            backend.set_editor_font(FONT, SIZE_PT);
            // A status bar measures/paints through `chrome_dwrite`,
            // not `self.dwrite` — see this
            // test's own doc. Set to the same `FONT`/`SIZE_PT` as the
            // editor font above (and the probe `DWrite` instance), or
            // the final `assert_eq!` below would compare against the
            // wrong font entirely.
            backend.set_ui_font(&format!("{FONT} {SIZE_PT}"));
            backend
                .attach_headless(surface.target().clone(), W, H)
                .expect("attach headless surface");
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            let layout = crate::DrawsPrimitives::draw_dyn(
                &mut backend,
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &StatusBar {
                    id: WidgetId::new("test:status-bar"),
                    left_segments: vec![StatusBarSegment {
                        text: TEXT.into(),
                        fg: Color::rgb(255, 255, 255),
                        bg: Color::rgb(0, 0, 0),
                        bold,
                        action_id: None,
                    }],
                    right_segments: vec![],
                },
                &(),
            );
            backend.end_frame();
            layout
                .downcast_ref::<crate::primitives::status_bar::StatusBarPrimitiveLayout>()
                .expect("a StatusBar lays out as StatusBarPrimitiveLayout")
                .layout
                .visible_segments[0]
                .bounds
                .width
        };

        let regular_width = width_for(false);
        let bold_width = width_for(true);
        assert!(
            bold_width > regular_width,
            "a bold segment must measure (and paint) wider than the same text non-bold: \
             regular={regular_width}, bold={bold_width}"
        );
        // …and the widths that reached the layout are the *same* ones
        // DirectWrite reports for each weight — proves the override
        // forwards to `measure_text_styled` rather than merely producing
        // some pair of different numbers.
        //
        // Issue #1155: a segment's `bounds` is its measured text width
        // plus `PIXEL_SEGMENT_PADDING` on *both* sides (pixel backends
        // call `StatusBar::layout_padded`), so the comparison is against
        // the probe widths grown by that known, constant amount — the
        // per-weight *delta* this test is really about is unaffected by it.
        const PAD: f32 = 2.0 * crate::primitives::status_bar::PIXEL_SEGMENT_PADDING;
        assert_eq!(
            (regular_width, bold_width),
            (probe_regular + PAD, probe_bold + PAD),
            "resolved segment widths must match DirectWrite's own per-weight measurement \
             plus #1155's per-segment padding"
        );
    }

    // ── #929: register_font_from_memory / set_nerd_font_fallback ────────────

    /// Garbage bytes aren't a font Core Graphics/DirectWrite can parse —
    /// `register_font_from_memory` must report that as `None` rather than
    /// panicking or returning a bogus family list. Doesn't need a real
    /// font file fixture (which this suite has none of), so it covers the
    /// one path that's fully exercisable without one.
    #[cfg(target_os = "windows")]
    #[test]
    fn register_font_from_memory_rejects_bytes_that_are_not_a_font() {
        let mut backend = WinBackend::new();
        let garbage = [0u8; 64];
        assert!(
            backend.register_font_from_memory(&garbage).is_none(),
            "64 zero bytes are not a parseable font — must report None, not a fabricated family"
        );
    }

    /// `register_font_from_memory` must copy `bytes` into self-owned storage
    /// *before* the caller's buffer can be dropped, not after — otherwise a
    /// caller passing a short-lived local (the shape this trait method's own doc
    /// explicitly permits) leaves DirectWrite holding a dangling pointer. This
    /// asserts the fix: `bytes` is copied into `registered_font_bytes` before
    /// the caller's buffer goes out of scope, and that copy is a distinct
    /// allocation from the caller's.
    #[cfg(target_os = "windows")]
    #[test]
    fn register_font_from_memory_copies_bytes_before_the_callers_buffer_can_be_dropped() {
        let mut backend = WinBackend::new();
        {
            // Scoped so `transient` is dropped at the end of this block,
            // before any assertion below runs — mirroring the exact
            // "call once from setup(), then drop the source buffer"
            // shape the trait doc permits.
            let transient = vec![0xABu8; 64];
            let _ = backend.register_font_from_memory(&transient);
        }
        assert_eq!(
            backend.registered_font_bytes.len(),
            1,
            "the call must copy bytes into backend-owned storage regardless of whether \
             registration itself succeeds"
        );
        assert_eq!(
            backend.registered_font_bytes[0],
            vec![0xABu8; 64],
            "the stored copy must match what was passed in, read back from backend-owned \
             memory rather than the (by now dropped) caller buffer"
        );
    }

    /// `set_nerd_font_fallback` works with no prior
    /// `register_font_from_memory` call at all — it should resolve
    /// `family` against the *system* font collection (see
    /// `crate::win::text::build_nerd_font_fallback`'s doc), which is the
    /// shape an app pointing at an already-installed Nerd Font uses.
    /// Asserts the whole pipeline (`set_nerd_font_fallback` ->
    /// `attach_headless` -> `DWrite::new` baking the fallback into both
    /// `IDWriteTextFormat`s -> a real paint) survives end to end and
    /// keeps painting ordinary text correctly — the regression this test
    /// guards is "wiring a fallback breaks normal (non-fallback) text",
    /// not "the fallback glyph is pixel-identical to some reference",
    /// which would need an embedded custom font fixture this suite
    /// doesn't have.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_nerd_font_fallback_against_the_system_collection_does_not_break_normal_paint() {
        use crate::win::testing::HeadlessSurface;
        use crate::Color;

        const W: u32 = 200;
        const H: u32 = 30;
        const TEXT: &str = "OK";

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        // "Segoe UI Symbol" ships on every supported Windows version and
        // is a real, distinct family from the default UI font — a
        // plausible stand-in for an app-installed Nerd Font pointed at
        // via a plain family name with no `register_font_from_memory`
        // call. Must precede `attach_headless`, same "bake at surface
        // construction" convention as `set_editor_font`/`set_ui_font`.
        backend.set_nerd_font_fallback("Segoe UI Symbol");
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach_headless must still succeed with a fallback wired in");

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let sentinel = Color::rgb(0, 0, 0);
        backend.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), sentinel);
        backend.draw_text(
            TEXT,
            Rect::new(4.0, 4.0, W as f32 - 8.0, H as f32 - 8.0),
            Color::rgb(255, 255, 255),
        );
        backend.end_frame();

        let (w, h) = backend.measure_text(TEXT);
        assert!(
            w > 0.0 && h > 0.0,
            "ordinary text must still measure to a real, positive size with a fallback wired \
             in (w={w}, h={h})"
        );

        // At least one non-background pixel landed inside the paint
        // target — a coarse but real proof that `draw_text` actually
        // painted glyphs rather than silently no-op'ing once a fallback
        // was attached.
        let mut painted_something = false;
        'outer: for y in 0..H {
            for x in 0..W {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (sentinel.r, sentinel.g, sentinel.b) {
                    painted_something = true;
                    break 'outer;
                }
            }
        }
        assert!(
            painted_something,
            "draw_text must paint something other than the sentinel background"
        );
    }

    // ── bundled codicon font self-registration ───────────────────────────

    /// `WinBackend::new` self-registers the bundled codicon font and
    /// seeds a codicon-only `nerd_font_fallback` before any app code
    /// runs — no app configuration required.
    #[cfg(target_os = "windows")]
    #[test]
    fn new_self_registers_codicon_and_seeds_a_fallback() {
        let backend = WinBackend::new();
        assert!(
            backend.codicon_font_collection.is_some(),
            "WinBackend::new() must self-register the bundled codicon font"
        );
        assert!(
            backend.nerd_font_fallback.is_some(),
            "a codicon-only fallback must be seeded even before any app \
             set_nerd_font_fallback call"
        );
    }

    /// `nerd_fonts_enabled` defaults to `true` on this backend — see
    /// the field's own doc for why that default differs from TUI's.
    #[test]
    fn nerd_fonts_enabled_defaults_to_true() {
        let backend = WinBackend::new();
        assert!(backend.nerd_fonts_enabled());
    }

    /// `set_nerd_font_fallback` must not drop the bundled codicon
    /// mapping `Self::new` seeded — `build_nerd_font_fallback_multi`
    /// rebuilds the whole `IDWriteFontFallback` from scratch each call
    /// (DirectWrite text formats carry exactly one), so this guards
    /// against a future edit that narrows it back down to just the
    /// app's own family. Painting the codicon close glyph through a
    /// `DWrite` built *after* an app `set_nerd_font_fallback` call must
    /// still produce real ink, not tofu.
    #[cfg(target_os = "windows")]
    #[test]
    fn set_nerd_font_fallback_keeps_the_codicon_mapping() {
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 40;
        const H: u32 = 40;
        const BG: crate::Color = crate::Color::rgb(10, 20, 30);

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend.set_nerd_font_fallback("Segoe UI Symbol");
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach_headless");

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), BG);
        let glyph = crate::codicon::CLOSE.to_string();
        backend.draw_text(
            &glyph,
            Rect::new(4.0, 4.0, W as f32 - 8.0, H as f32 - 8.0),
            crate::Color::rgb(220, 40, 40),
        );
        backend.end_frame();

        let mut painted_something = false;
        'outer: for y in 0..H {
            for x in 0..W {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (BG.r, BG.g, BG.b) {
                    painted_something = true;
                    break 'outer;
                }
            }
        }
        assert!(
            painted_something,
            "the codicon close glyph must still paint real ink after an app \
             set_nerd_font_fallback call, not just before one"
        );
    }

    // ── #1024: has_font_family ───────────────────────────────────────────────

    /// "Segoe UI" ships on every supported Windows version — a real
    /// system-installed family that `has_font_family` must report as
    /// present via `FindFamilyName`, not just "resolved to something".
    #[cfg(target_os = "windows")]
    #[test]
    fn has_font_family_finds_a_real_system_family() {
        let backend = WinBackend::new();
        assert_eq!(
            backend.has_font_family("Segoe UI"),
            Some(true),
            "Segoe UI ships on every supported Windows version"
        );
    }

    /// A family name nothing is installed under must report `Some(false)`
    /// — a successful DirectWrite lookup that found nothing, not `None`
    /// ("don't know"). Conflating the two would be exactly the bug this
    /// issue's `Backend::has_font_family` doc warns callers not to make.
    #[cfg(target_os = "windows")]
    #[test]
    fn has_font_family_is_some_false_for_a_name_nothing_is_installed_under() {
        let backend = WinBackend::new();
        assert_eq!(
            backend.has_font_family("Definitely Not A Real Font Family Quadraui 1024"),
            Some(false)
        );
    }

    // ── TextDisplay real-pixel driver tests ──────────────────────────────────
    //
    // Drive the real `DrawsPrimitives::draw_primitive` →
    // `primitives::text_display::paint` path end to end.

    #[cfg(target_os = "windows")]
    fn td_line(text: &str) -> crate::primitives::text_display::TextDisplayLine {
        crate::primitives::text_display::TextDisplayLine {
            spans: vec![crate::types::StyledSpan::plain(text)],
            decoration: crate::types::Decoration::Normal,
            timestamp: None,
        }
    }

    #[cfg(target_os = "windows")]
    fn make_td(lines: usize, show_scrollbar: bool) -> crate::primitives::text_display::TextDisplay {
        crate::primitives::text_display::TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..lines).map(|i| td_line(&format!("ln{i}"))).collect(),
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar,
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_text_display_background_fills_theme_background() {
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 240;
        const H: u32 = 160;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let td = make_td(0, false);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        crate::DrawsPrimitives::draw_primitive(
            &mut backend,
            Rect::new(0.0, 0.0, W as f32, H as f32),
            &td,
        );
        backend.end_frame();

        let theme = Theme::default();
        let px = surface.pixel_at(W / 2, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.background.r, theme.background.g, theme.background.b)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_text_display_scrollbar_gutter_and_thumb_paint() {
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 240;
        const H: u32 = 160;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let td = make_td(100, true);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &td).layout;
        backend.end_frame();

        let theme = Theme::default();

        let gutter = layout.scrollbar_bounds.expect("gutter present");
        let probe_x = (gutter.x + gutter.width / 2.0) as u32;
        let probe_y = (gutter.y + gutter.height - 2.0) as u32;
        let px = surface.pixel_at(probe_x, probe_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.scrollbar_track.r,
                theme.scrollbar_track.g,
                theme.scrollbar_track.b
            )
        );

        let thumb = layout.thumb_bounds.expect("thumb present");
        let probe_x = (thumb.x + thumb.width / 2.0) as u32;
        let probe_y = (thumb.y + thumb.height / 2.0) as u32;
        let px = surface.pixel_at(probe_x, probe_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.scrollbar_thumb.r,
                theme.scrollbar_thumb.g,
                theme.scrollbar_thumb.b
            )
        );
    }

    /// Paint at a non-zero rect origin with a title row present, then
    /// round-trip an absolute click the way a real host does — layout helpers
    /// must return coords in the same frame across backends.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_text_display_layout_hit_test_resolves_lines_at_nonzero_origin() {
        use crate::primitives::text_display::TextDisplayHit;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 240;
        const H: u32 = 160;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let mut td = make_td(20, false);
        td.title = Some(crate::types::StyledText::plain("Logs"));

        let rect_x = 9.0_f32;
        let rect_y = 17.0_f32;
        let rect = Rect::new(rect_x, rect_y, W as f32 - rect_x, H as f32 - rect_y);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let painted = crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &td);
        backend.end_frame();

        // The body strip starts below the title row, at the editor row
        // pitch the display was painted on.
        let body_y = painted.body.y;
        assert!(
            body_y > rect_y,
            "the title row must push the body below the rect's own top: body_y={body_y}"
        );
        let layout = &painted.layout;
        let vis = &layout.visible_lines[0];
        assert_eq!(
            vis.bounds.y, 0.0,
            "visible_lines bounds.y must be body-local"
        );

        let abs_x = rect_x + vis.bounds.x + vis.bounds.width * 0.5;
        let abs_y = body_y + vis.bounds.y + vis.bounds.height * 0.5;
        match painted.hit_test(abs_x, abs_y) {
            TextDisplayHit::Line(idx) => assert_eq!(idx, vis.line_idx),
            other => panic!("expected Line, got {:?}", other),
        }
    }

    /// No-paint layout must agree byte-for-byte with what the paint
    /// path resolved.
    #[cfg(target_os = "windows")]
    #[test]
    fn win_backend_text_display_no_paint_layout_matches_paint_layout() {
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 240;
        const H: u32 = 160;

        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let td = make_td(30, true);
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let painted = crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &td);
        backend.end_frame();

        let no_paint = crate::DrawsPrimitives::layout_primitive(&backend, rect, &td);
        assert_eq!(painted, no_paint);
    }

    // ─── Issue #924: no-surface fallback survives instead of panicking ────────

    fn sample_minimap() -> crate::primitives::minimap::Minimap {
        crate::primitives::minimap::Minimap {
            id: WidgetId::new("minimap"),
            lines: (0..40)
                .map(|i| crate::primitives::minimap::MinimapLine {
                    text: format!("line {i}"),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: 10,
            total_buffer_lines: 40,
            scale: Default::default(),
            sizing: None,
        }
    }

    /// A `WinBackend` that has never had `attach_surface`/`attach_headless`
    /// called — the same surface-less state a fresh `WinBackend::new()` starts in
    /// and the state `end_frame` drops back to after a device-lost `EndDraw`
    /// failure — must survive a full `begin_frame` -> a spread of draw/layout/paint
    /// calls -> `end_frame`, without panicking. No `target_os = "windows"` gate: the
    /// whole point is that this degrades instead of aborting on every host.
    #[test]
    fn win_backend_does_not_panic_with_no_surface_attached_across_a_full_frame() {
        let mut backend = WinBackend::new();
        let viewport = Viewport::new(400.0, 300.0, 1.0);
        Backend::begin_frame(&mut backend, viewport);

        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);

        // `()` category. None of these types derive `Default`, so build
        // each with an empty/idle content set instead.
        let msv = crate::MultiSectionView {
            id: WidgetId::new("msv"),
            sections: Vec::new(),
            active_section: None,
            axis: crate::primitives::multi_section_view::Axis::Vertical,
            allow_resize: false,
            allow_collapse: false,
            scroll_mode: crate::primitives::multi_section_view::ScrollMode::PerSection,
            has_focus: false,
            panel_scroll: 0.0,
        };
        let _ = crate::DrawsPrimitives::draw_dyn(&mut backend, rect, &msv, &());

        // Widget-contract rail (fixed row geometry, no measurer).
        let activity_bar = ActivityBar {
            id: WidgetId::new("activity-bar"),
            top_items: Vec::new(),
            bottom_items: Vec::new(),
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
        };
        let _ = crate::DrawsPrimitives::draw_dyn(&mut backend, rect, &activity_bar, &());

        // Widget-contract category (nominal-measurer fallback): a
        // `TabBar` pushed through `draw_dyn` resolves its layout and
        // skips paint when no surface answers.
        let tab_bar = TabBar {
            id: WidgetId::new("tab-bar"),
            tabs: Vec::new(),
            scroll_offset: 0,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: false,
            compact: false,
            ..Default::default()
        };
        let _ = crate::DrawsPrimitives::draw_dyn(&mut backend, rect, &tab_bar, &());

        // A `Toolbar` through `draw_dyn` takes the same nominal-measurer
        // fallback.
        let toolbar = crate::primitives::toolbar::Toolbar {
            id: WidgetId::new("toolbar"),
            buttons: Vec::new(),
            bg: None,
            focused_index: None,
            ..Default::default()
        };
        let _ = crate::DrawsPrimitives::draw_dyn(&mut backend, rect, &toolbar, &());

        // A `Minimap` through `draw_primitive` resolves its real layout
        // and skips paint when no surface answers.
        let minimap = sample_minimap();
        let _ = crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &minimap);

        // `PaintSurface` measurement category (nominal char-width
        // estimate).
        let _ = PaintSurface::surface_measure_text(&backend, "hello");
        let _ = PaintSurface::surface_measure_text_styled(&backend, "hello", true);

        Backend::end_frame(&mut backend);
    }

    /// A minimap painted with no surface attached still resolves the same
    /// real geometry its no-paint layout query does.
    #[test]
    fn minimap_with_no_surface_resolves_the_same_layout_as_its_query() {
        use crate::backend::DrawsPrimitives;

        let mut backend = WinBackend::new();
        let minimap = sample_minimap();
        let rect = Rect::new(0.0, 0.0, 20.0, 100.0);

        let painted = backend.draw_primitive(rect, &minimap);
        assert!(
            !painted.visible_lines.is_empty(),
            "the layout must still be real geometry, not an empty stub"
        );
        assert_eq!(painted, backend.layout_primitive(rect, &minimap));
    }

    /// A minimap's `scale` changes the row pitch its layout resolves. This
    /// path needs no Windows surface, so it provides real signal on every
    /// host.
    #[test]
    fn minimap_scale_changes_the_resolved_row_pitch() {
        use crate::backend::DrawsPrimitives;
        use crate::primitives::minimap::MinimapScale;

        let backend = WinBackend::new();
        let mut minimap = sample_minimap();
        let rect = Rect::new(0.0, 0.0, 20.0, 100.0);

        assert_eq!(minimap.scale, MinimapScale::One);
        let default_layout = backend.layout_primitive(rect, &minimap);
        assert_eq!(
            default_layout.visible_lines[0].bounds.height,
            MinimapScale::One.row_pitch_px() as f32
        );

        minimap.scale = MinimapScale::Two;
        let scaled_layout = backend.layout_primitive(rect, &minimap);
        assert_eq!(
            scaled_layout.visible_lines[0].bounds.height,
            MinimapScale::Two.row_pitch_px() as f32
        );
    }

    /// An image painted on a surface-less backend reports `Unsupported`
    /// rather than panicking.
    #[test]
    fn image_with_no_surface_reports_unsupported() {
        let mut backend = WinBackend::new();
        let image = crate::primitives::image::Image {
            id: WidgetId::new("img"),
            source: crate::primitives::image::ImageSource::Bytes(Vec::new()),
            intrinsic_size: None,
            fit: crate::primitives::image::ImageFit::Contain,
            fallback_text: "img".to_string(),
        };
        let rect = Rect::new(0.0, 0.0, 50.0, 50.0);

        let result = PaintSurface::surface_draw_image(&mut backend, rect, &image);
        assert_eq!(result, crate::backend::ImagePaintResult::Unsupported);
    }

    /// `PaintSurface::surface_measure_text`/`surface_measure_text_styled`
    /// on a surface-less backend must return a finite nominal estimate
    /// (`char_width` per character) instead of panicking, so a host doing
    /// hit-testing against the measurement gets a non-degenerate box.
    #[test]
    fn surface_measure_text_with_no_surface_returns_a_nominal_estimate() {
        let backend = WinBackend::new();
        let (w, h) = PaintSurface::surface_measure_text(&backend, "hello");
        assert_eq!(w, 5.0 * backend.current_char_width);
        assert_eq!(h, backend.current_line_height);

        let (w2, h2) = PaintSurface::surface_measure_text_styled(&backend, "hello", true);
        assert_eq!(w2, 5.0 * backend.current_char_width);
        assert_eq!(h2, backend.current_line_height);
    }

    // ── Caret blink ──────────────────────────────────────────────────────

    /// `WinBackend`'s caret-blink accessors forward to
    /// [`crate::caret_blink::CaretBlinkState`] faithfully: `tick_caret_blink`
    /// flips `caret_visible` after `caret_blink_period` elapses,
    /// `note_caret_activity` holds it solid through a typing pause, and
    /// `set_caret_visible` lets a test pin the phase directly — mirrors
    /// `GtkBackend`/`MacBackend`'s equivalent coverage for the same state
    /// machine.
    #[test]
    fn win_backend_caret_blink_methods_drive_the_shared_state_machine() {
        let mut backend = WinBackend::new();
        let period = backend.caret_blink_period();
        assert!(period > std::time::Duration::ZERO);

        backend.set_caret_visible(true);
        assert!(backend.caret_visible());
        backend.set_caret_visible(false);
        assert!(!backend.caret_visible());

        let start = Instant::now();
        let flipped = backend.tick_caret_blink(start + period);
        assert!(flipped, "a tick a full period later must flip the phase");
        assert!(backend.caret_visible());

        // A typing-pause bump holds the caret solid: a tick that would
        // otherwise flip it must now be a no-op.
        backend.note_caret_activity(start + period);
        let flipped_during_pause = backend.tick_caret_blink(start + period);
        assert!(
            !flipped_during_pause,
            "a tick during an active typing pause must not flip the phase"
        );
        assert!(backend.caret_visible());
    }

    // ── overlay order: tooltip/toast stay topmost of the focus ring ──────

    #[cfg(target_os = "windows")]
    fn sample_toast_overlay() -> crate::ToastOverlay {
        use crate::primitives::toast::{Toast, ToastCorner, ToastOverlay, ToastSeverity};
        ToastOverlay {
            id: WidgetId::new("toasts:bottom_right"),
            corner: ToastCorner::BottomRight,
            toasts: vec![Toast {
                id: WidgetId::new("toast:1"),
                title: "Saved".to_string(),
                body: String::new(),
                severity: ToastSeverity::Success,
                actions: vec![],
                accent: None,
            }],
            focus: None,
        }
    }

    /// `super::run::render_frame` calls `WinBackend::draw_focus_ring` *after*
    /// `AppLogic::render` returns, so an app that paints a toast overlapping
    /// the currently-focused widget's rect — e.g. a toast that lands over a
    /// focused, bordered editor — must not have the generic focus-ring
    /// convention stroke on top of it on the very same frame. Regression for
    /// the overlay-order seam (tooltip/toast topmost of all,
    /// `quadraui-binding-core::host::render_overlays`'s doc). Mirrors
    /// `macos::backend::tests::draw_focus_ring_does_not_paint_over_a_toast_from_the_same_frame`.
    #[cfg(target_os = "windows")]
    #[test]
    fn draw_focus_ring_does_not_paint_over_a_toast_from_the_same_frame() {
        use crate::theme::Theme;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 400;
        const H: u32 = 200;

        let overlay = sample_toast_overlay();
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = crate::DrawsPrimitives::draw_primitive(
            &mut backend,
            Rect::new(0.0, 0.0, W as f32, H as f32),
            &overlay,
        )
        .layout;
        let vt = layout.visible_toasts[0].bounds;
        // A tab-stop focus ring whose rect is the toast's own bounds — the
        // worst case, a toast that lands exactly over the focused widget it
        // was raised from.
        backend.draw_focus_ring(vt);
        backend.end_frame();

        let theme = Theme::default();
        let success_bg = theme.success_bg();
        // Probe right at the toast's edge, where the ring would have
        // stroked had it not been skipped.
        let probe_x = (vt.x + vt.width / 2.0) as u32;
        let probe_y = vt.y as u32;
        let px = surface.pixel_at(probe_x, probe_y);
        assert_ne!(
            (px.r, px.g, px.b),
            (
                theme.focus_ring_color().r,
                theme.focus_ring_color().g,
                theme.focus_ring_color().b
            ),
            "the focus ring must not stroke over a toast painted earlier this frame",
        );
        // The toast's own fill is still intact at its centre.
        let cx = (vt.x + vt.width / 2.0) as u32;
        let cy = (vt.y + vt.height / 2.0) as u32;
        let px = surface.pixel_at(cx, cy);
        assert_eq!(
            (px.r, px.g, px.b),
            (success_bg.r, success_bg.g, success_bg.b),
            "the toast's own fill must still paint",
        );
    }

    /// The ring is a perimeter stroke ([`focus_ring_band_overlaps`]), not a
    /// filled rect, so a toast that lands strictly *inside* a large focused
    /// widget — e.g. a toast overlay insetting the stack from a
    /// focused editor/list that spans the whole viewport — must leave the
    /// ring (and its `chrome:focus-ring` zone) intact: there is no pixel
    /// conflict to suppress. Mirrors
    /// `macos::backend::tests::draw_focus_ring_survives_a_toast_interior_to_the_focused_widget`.
    #[cfg(target_os = "windows")]
    #[test]
    fn draw_focus_ring_survives_a_toast_interior_to_the_focused_widget() {
        use crate::theme::Theme;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 400;
        const H: u32 = 200;

        let overlay = sample_toast_overlay();
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        // The focused widget spans the whole viewport (a focused
        // editor/list behind the toast stack); the toast itself is inset
        // from that same rect by the toast margin, so it never reaches the
        // focused rect's edges — the stroke band the ring actually paints.
        let focused_rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        crate::DrawsPrimitives::draw_primitive(&mut backend, focused_rect, &overlay);
        backend.draw_focus_ring(focused_rect);
        backend.end_frame();

        let theme = Theme::default();
        // Probe at the focused rect's own top edge, inside the stroke
        // band, well away from the inset toast box.
        let probe_x = (focused_rect.x + 2.0) as u32;
        let probe_y = focused_rect.y as u32;
        let px = surface.pixel_at(probe_x, probe_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.focus_ring_color().r,
                theme.focus_ring_color().g,
                theme.focus_ring_color().b
            ),
            "the focus ring must still paint when the toast is interior to the \
             focused widget and never touches the ring's stroke band",
        );
        assert!(
            backend
                .zones
                .iter()
                .any(|z| z.id == WidgetId::new("chrome:focus-ring")),
            "the ring's chrome zone must still be registered",
        );
    }

    /// Same contract as the toast regression above, for a tooltip anchored
    /// directly over the currently-focused widget — the exact "hover
    /// tooltip over a focused, bordered widget" scenario this issue's
    /// acceptance criteria name. Mirrors
    /// `macos::backend::tests::draw_focus_ring_does_not_paint_over_a_tooltip_from_the_same_frame`.
    #[cfg(target_os = "windows")]
    #[test]
    fn draw_focus_ring_does_not_paint_over_a_tooltip_from_the_same_frame() {
        use crate::primitives::tooltip::Tooltip;
        use crate::theme::Theme;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 400;
        const H: u32 = 200;

        let rect = Rect::new(20.0, 20.0, 100.0, 30.0);
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let tooltip = Tooltip::new(WidgetId::new("tip"), "Hover text");
        crate::DrawsPrimitives::draw_primitive(&mut backend, rect, &tooltip);
        // The focused widget this tooltip is anchored to shares the
        // tooltip's own bounds exactly — the ring must yield.
        backend.draw_focus_ring(rect);
        backend.end_frame();

        let theme = Theme::default();
        let probe_x = (rect.x + rect.width / 2.0) as u32;
        let probe_y = rect.y as u32;
        let px = surface.pixel_at(probe_x, probe_y);
        assert_ne!(
            (px.r, px.g, px.b),
            (
                theme.focus_ring_color().r,
                theme.focus_ring_color().g,
                theme.focus_ring_color().b
            ),
            "the focus ring must not stroke over a tooltip painted earlier this frame",
        );
        // The tooltip's own fill is present at its centre — a positive
        // assertion that the tooltip actually painted something there,
        // not just that the ring's colour is absent. Probed at mid-height
        // just inside the right border: the left-aligned "Hover text" label
        // ends well short of there at any chrome font size, so the pixel
        // is pure fill rather than a glyph whose position depends on the
        // installed font metrics.
        let fx = (rect.x + rect.width - 4.0) as u32;
        let fy = (rect.y + rect.height / 2.0) as u32;
        let px = surface.pixel_at(fx, fy);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
            "the tooltip's own fill must paint inside its bounds",
        );
    }

    /// The overlay-order contract documented on
    /// [`crate::Backend::draw_focus_ring`] — tooltip/toast stay topmost of
    /// all — must hold on Win-GUI even when the base layer underneath is a
    /// bordered, focused [`crate::primitives::text_input::TextInput`] (e.g.
    /// the `clipboard_demo` example's editor), not empty space: a toast
    /// opened over it must still show its own fill, not get painted over
    /// by the editor's own border/background, nor by the generic focus
    /// ring `draw_focus_ring` strokes around that same focused editor on
    /// this same frame. The toast stack is inset from the editor rect, so
    /// it never reaches the editor's own edges — the ring must still paint
    /// there, on the far side from the bottom-right toast stack. Mirrors
    /// `macos::toast::tests::toast_paints_over_a_bordered_focused_text_input`.
    #[cfg(target_os = "windows")]
    #[test]
    fn draw_toast_overlay_paints_over_a_bordered_focused_text_input() {
        use crate::primitives::text_input::TextInput;
        use crate::theme::Theme;
        use crate::win::testing::HeadlessSurface;

        const W: u32 = 400;
        const H: u32 = 200;

        let overlay = sample_toast_overlay();
        let surface = HeadlessSurface::new(W, H).expect("create headless surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");

        let mut editor = TextInput::new(WidgetId::new("editor"));
        editor.lines = vec!["fn main() {}".to_string()];
        editor.has_focus = true;
        let editor_rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        crate::DrawsPrimitives::draw_primitive(&mut backend, editor_rect, &editor);
        let layout =
            crate::DrawsPrimitives::draw_primitive(&mut backend, editor_rect, &overlay).layout;
        // The chrome-level focus ring the runner strokes around
        // `editor_rect` once `AppLogic::render` returns — must not land on
        // top of the toast just painted above.
        backend.draw_focus_ring(editor_rect);
        backend.end_frame();

        let vt = &layout.visible_toasts[0];
        let success_bg = Theme::default().success_bg();
        let cx = (vt.bounds.x + vt.bounds.width / 2.0) as u32;
        let cy = (vt.bounds.y + vt.bounds.height / 2.0) as u32;
        let px = surface.pixel_at(cx, cy);
        assert_eq!(
            (px.r, px.g, px.b),
            (success_bg.r, success_bg.g, success_bg.b),
            "the toast must paint over the bordered, focused editor underneath it, \
             not get overwritten by its border/background or its focus ring",
        );

        // The toast stack is inset from `editor_rect`, so it never reaches
        // the editor's own edges — the ring must still paint there, on the
        // far (top-left) side from the bottom-right toast stack.
        let theme = Theme::default();
        let px = surface.pixel_at(2, 0);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.focus_ring_color().r,
                theme.focus_ring_color().g,
                theme.focus_ring_color().b
            ),
            "the focus ring must still paint around the focused editor where the \
             toast, inset within it, never reaches",
        );
    }

    /// Drives [`crate::compose::HoverTooltipTracker`] through real
    /// [`UiEvent::MouseMoved`] dispatch + [`super::testing::WinDriver::tick`]
    /// — the same `AppLogic::tick` path the live `win::run` runner uses
    /// for its `request_frame_in`-scheduled repaint, feeding the event
    /// straight into dispatch rather than through the live `wndproc`'s
    /// `route_mouse_move`/`route_pointer` step first. Not a test of
    /// `HoverTooltipTracker` itself (that lives in its own module) — this
    /// proves the Win-GUI dispatch + tick + tooltip paint wiring actually
    /// shows and hides a tooltip in response to real pointer input.
    /// Mirrors
    /// `macos::tooltip::tests::hover_driven::tooltip_appears_after_the_delay_and_hides_on_pointer_leave`.
    #[cfg(target_os = "windows")]
    mod hover_driven {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        use super::*;
        use crate::compose::HoverTooltipTracker;
        use crate::primitives::tooltip::Tooltip;
        use crate::runner::Reaction;
        use crate::win::testing::WinDriver;
        use crate::{AppLogic, ButtonMask, Point, UiEvent};

        const DELAY_MS: u64 = 500;

        /// Anchor rect the tooltip is shown for — matches both the
        /// hover hit-test target and the tooltip's own bounds, so a
        /// `MouseMoved` anywhere inside it hovers the one tracked item.
        fn anchor() -> Rect {
            Rect::new(10.0, 10.0, 60.0, 20.0)
        }

        /// Minimal [`AppLogic`] wiring a single-widget
        /// [`HoverTooltipTracker`] to real pointer input: `handle` feeds
        /// `MouseMoved`'s position through a plain [`Rect::contains`]
        /// hit-test, `tick` advances the delay, `render` paints the
        /// tooltip only while [`HoverTooltipTracker::visible_item`] is
        /// `Some`. `clock_ms` is a caller-driven millisecond counter (the
        /// tracker's own "headless-clock contract" — see its module doc),
        /// not a wall clock, so the test controls elapsed time exactly
        /// without sleeping.
        struct HoverApp {
            tracker: RefCell<HoverTooltipTracker<WidgetId>>,
            clock_ms: Rc<Cell<u64>>,
        }

        impl AppLogic for HoverApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                if self.tracker.borrow().visible_item().is_some() {
                    let tooltip = Tooltip::new(WidgetId::new("tip"), "Hover hint");
                    crate::DrawsPrimitives::draw_dyn(backend, anchor(), &tooltip, &());
                }
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                match event {
                    UiEvent::MouseMoved { position, .. } => {
                        let hovered = anchor().contains(position).then(|| WidgetId::new("w"));
                        let now = self.clock_ms.get();
                        let changed = self.tracker.borrow_mut().on_hover(hovered, now);
                        if hovered.is_some() {
                            backend.request_frame_in(std::time::Duration::from_millis(DELAY_MS));
                        }
                        if changed {
                            Reaction::Redraw
                        } else {
                            Reaction::Continue
                        }
                    }
                    _ => Reaction::Continue,
                }
            }

            fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
                let now = self.clock_ms.get();
                if self.tracker.borrow_mut().tick(DELAY_MS, now) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
        }

        fn hovering_move(driver: &mut WinDriver<HoverApp>, x: f32, y: f32) {
            driver.dispatch(UiEvent::MouseMoved {
                position: Point::new(x, y),
                buttons: ButtonMask::default(),
            });
        }

        #[test]
        fn tooltip_appears_after_the_delay_and_hides_on_pointer_leave() {
            let clock = Rc::new(Cell::new(0u64));
            let app = HoverApp {
                tracker: RefCell::new(HoverTooltipTracker::new()),
                clock_ms: clock.clone(),
            };
            let mut driver = WinDriver::new(app, 100, 50);

            // Pointer lands on the anchor — still pending, no tooltip yet.
            hovering_move(&mut driver, 40.0, 20.0);
            assert!(
                !driver.screen_contains("Hover hint"),
                "the tooltip must not show the instant the pointer lands"
            );
            // `HoverApp::handle` asked the real `WinBackend::request_frame_in`
            // to schedule the redraw this delay needs — not just this test's
            // hand-driven `clock`/`tick()` below. Without this, nothing here
            // would fail if `request_frame_in` stopped being called at all.
            assert_eq!(
                driver.backend().frame_requests(),
                1,
                "hovering the anchor must request exactly one tick after the hover delay"
            );
            let pending = driver
                .backend()
                .pending_frame_delay()
                .expect("hovering the anchor must schedule a pending tick");
            assert!(
                pending <= std::time::Duration::from_millis(DELAY_MS),
                "the pending tick must be due within the hover delay, got {pending:?}"
            );

            // The delay elapses with no further pointer movement — the
            // `tick()` the real runner's `request_frame_in` timer would
            // have fired now shows the tooltip.
            clock.set(DELAY_MS);
            driver.tick();
            assert!(
                driver.screen_contains("Hover hint"),
                "the tooltip must show once the hover delay has elapsed"
            );

            // The pointer leaves the anchor entirely — the tooltip must
            // disappear immediately, no further delay.
            hovering_move(&mut driver, 90.0, 40.0);
            assert!(
                !driver.screen_contains("Hover hint"),
                "the tooltip must hide as soon as the pointer leaves its anchor"
            );
        }
    }
}
