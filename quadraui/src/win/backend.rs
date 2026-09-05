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
//! `find_replace`/`rich_text_popup` (see `super::tooltip` etc.). #29
//! landed the five container/indicator rasterisers — `panel`/`split`/
//! `toast`/`progress`/`spinner`. #30 landed the three text-heavy
//! rasterisers — `terminal`/`text_display`/`message_list` (see
//! `super::terminal`/`super::text_display`/`super::message_list`).
//! Every other `draw_*`/`*_layout` rasteriser method is still a
//! `todo!()` stub — later issues implement each one against Direct2D /
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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::accelerator::{key_to_binding_name, parse_binding};
use crate::backend::{Backend, EditorPaintResult, PlatformServices, PointerShape};
use crate::dispatch::{DoubleClickDetector, DragState, TextRegion};
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
    Accelerator, AcceleratorId, AcceleratorScope, ActivityBar, Key, ListView, Modifiers, Palette,
    ParsedBinding, StatusBar, TabBar, Terminal, TextDisplay, TooltipChrome, TreeView,
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
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1RenderTarget,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES,
};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
#[cfg(target_os = "windows")]
use windows::Win32::UI::HiDpi::GetDpiForWindow;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, LoadCursorW, SetCursor, IDC_ARROW, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE,
    IDC_SIZEWE,
};

#[cfg(target_os = "windows")]
use super::text::DWrite;

/// Default editor font family for the Win-GUI backend — the historical
/// default monospace face shipped with every Windows version since Vista,
/// same role as GTK's hardcoded `"Monospace"` fontconfig alias.
#[cfg(target_os = "windows")]
const DEFAULT_EDITOR_FONT_FAMILY: &str = "Consolas";

/// Default chrome (UI) font family for the Win-GUI backend until
/// [`Backend::set_ui_font`] overrides it (#724) — the system UI font on
/// every Windows version since Vista, same role as GTK's `"Sans 11"`
/// fallback for `ui_font`.
#[cfg(target_os = "windows")]
const DEFAULT_UI_FONT_FAMILY: &str = "Segoe UI";

/// Default chrome font size in points, paired with
/// [`DEFAULT_UI_FONT_FAMILY`] — matches [`WinBackend::editor_font_size_pt`]'s
/// default.
#[cfg(target_os = "windows")]
const DEFAULT_UI_FONT_SIZE_PT: f32 = 11.0;

/// Parse a Pango-style font description (`Backend::set_ui_font`'s
/// documented shape, e.g. `"Segoe UI 11"`) into a DirectWrite-ready
/// `(family, size_pt)` pair. Win-GUI has no Pango parser to reuse the way
/// `gtk::chrome_font_description` reuses `pango::FontDescription::from_string`,
/// so this only understands the one convention that doc names: a
/// trailing whitespace-separated numeric token is the point size, and
/// everything before it is the family. Falls back to
/// `(DEFAULT_UI_FONT_FAMILY, DEFAULT_UI_FONT_SIZE_PT)` wholesale — rather
/// than guessing at a partial split — if `desc` doesn't parse that way,
/// same "don't fail, degrade" posture `super::text`'s font-metrics lookup
/// takes when a requested family isn't installed.
#[cfg(target_os = "windows")]
fn parse_ui_font_desc(desc: &str) -> (String, f32) {
    let desc = desc.trim();
    if let Some(idx) = desc.rfind(' ') {
        let (family, size_str) = desc.split_at(idx);
        let family = family.trim();
        if let (false, Ok(size)) = (family.is_empty(), size_str.trim().parse::<f32>()) {
            return (family.to_string(), size);
        }
    }
    (DEFAULT_UI_FONT_FAMILY.to_string(), DEFAULT_UI_FONT_SIZE_PT)
}

/// A live Direct2D render target: either a window-bound
/// `ID2D1HwndRenderTarget` ([`WinBackend::attach_surface`], a real
/// window) or an offscreen `ID2D1DCRenderTarget`
/// ([`WinBackend::attach_headless`], [`super::testing::WinDriver`] —
/// quadraui#707). Every rasteriser below (`super::activity_bar::draw_activity_bar`
/// et al.) only ever needs the shared `ID2D1RenderTarget` base interface
/// (`BeginDraw`/`Clear`/`EndDraw`/`FillRectangle`/`DrawText`/...) — both
/// D2D interfaces derive from it, and `windows-rs` generates a `Deref`
/// to the base interface for each, which this enum forwards below — so
/// none of the `&surface.target` call sites throughout this file need to
/// know or care which kind of target they're painting onto. The one
/// exception is [`WinBackend::resize_surface`]'s `Resize` call, which is
/// declared on `ID2D1HwndRenderTarget` itself (a DC render target has no
/// live-resize concept: [`super::testing::HeadlessSurface`] is a fixed
/// size for its whole lifetime) and so matches on the variant directly
/// instead of going through `Deref`.
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

/// The live Direct2D handles behind a real Win32 window, or (#707) an
/// offscreen headless surface. Only exists once
/// [`WinBackend::attach_surface`] or [`WinBackend::attach_headless`] has
/// run — a `WinBackend` constructed standalone (before either has run)
/// has `surface: None` and every draw call keeps hitting the `todo!()`
/// arm until a later issue implements it.
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

pub struct WinBackend {
    viewport: Viewport,
    /// `Rc<RefCell<>>` (not a plain field) so [`Backend::modal_stack_handle`]
    /// can hand back a handle that outlives any single `&mut self`
    /// borrow — the shape `GtkBackend`/`TuiBackend`/`MacBackend` already
    /// use for this (quadraui#699).
    modal_stack: Rc<RefCell<ModalStack>>,
    /// See `modal_stack`'s doc comment — same rationale.
    drag_state: Rc<RefCell<DragState>>,
    /// Folds a `MouseDown` translated from `WM_LBUTTONDOWN`/`WM_RBUTTONDOWN`/
    /// etc. into `DoubleClick` when it lands within the time/position
    /// window of the previous click (#729). `win::run`'s `wndproc`
    /// dispatches synchronously per Win32 message — same model as
    /// `macos::run` per `NSEvent` — so this runs on one event at a time
    /// via [`Self::fold_double_click`] rather than `TuiBackend`'s
    /// per-poll batch. Mirrors `MacBackend::double_click`; deliberately
    /// *not* a new `WM_*BUTTONDBLCLK` translator — see that issue for why.
    double_click: DoubleClickDetector,
    accelerators: HashMap<AcceleratorId, Accelerator>,
    /// Parsed form of every registered `Global`-scope-eligible
    /// accelerator, kept in registration order so [`Self::match_keypress`]
    /// is a linear scan — same shape as `TuiBackend`/`GtkBackend`/
    /// `MacBackend`'s `parsed_accelerators` (quadraui#707).
    parsed_accelerators: Vec<(ParsedBinding, AcceleratorId)>,
    /// `WidgetId` of the [`ActivityBar`] that declared
    /// `is_keyboard_focused = true` during the most recent
    /// [`Backend::draw_activity_bar`] call, or `None` if no bar is
    /// focused. Read by `win::run::dispatch_event` to decide whether a
    /// `KeyPressed` should be redirected into
    /// `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })`
    /// instead of reaching `AppLogic::handle` as a raw key — the Win-GUI
    /// twin of `GtkBackend`/`MacBackend`'s `focused_activity_bar`
    /// (quadraui#707, mirroring #465's macOS wiring). Without it,
    /// `ShellAdapter`'s built-in activity-bar keyboard navigation (#409)
    /// is unreachable on this backend.
    focused_activity_bar: Option<WidgetId>,
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
    /// DirectWrite factory + `IDWriteTextFormat` for the current chrome
    /// (UI) font (`ui_font_family`/`ui_font_size_pt`) — the chrome twin
    /// of `dwrite` above. `None` until [`Self::attach_surface`] creates
    /// it; recreated alongside it the same way (#724). Read via
    /// [`Self::chrome_dwrite`]; no `draw_*` rasteriser paints chrome text
    /// with it yet — wiring individual rasterisers off `dwrite` (the
    /// editor font) onto this is a follow-up, same posture as macOS's
    /// still-open `set_ui_font` gap (`ACCEPTED_DEFAULTS`, #624).
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
    /// The active [`crate::Theme`], set via [`Backend::set_theme`] and
    /// read by every `draw_*` rasteriser that used to fall back to
    /// `Theme::default()` regardless of what the app configured (#724).
    /// Not `cfg`-gated like `surface`/`dwrite` above: `Theme` is a plain,
    /// portable value with no WinAPI dependency — same rationale as
    /// `current_pointer_shape` below — so `set_theme`/`current_theme`
    /// stay testable on every host, not only `target_os = "windows"`.
    current_theme: crate::theme::Theme,
    /// The `PointerShape` [`Backend::set_cursor`] last applied — read back
    /// by `win::run`'s `WM_SETCURSOR` handler (#702) so the pointer glyph
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
    /// frame. [`super::testing::WinDriver`] turns it on (quadraui#721).
    /// Not `target_os`-gated: the flag itself is a plain `bool` with no
    /// WinAPI dependency, only the paint calls that would ever populate
    /// anything are.
    painted_text_recording: bool,
    /// Text runs recorded during the last frame, when
    /// `painted_text_recording` is on — the `WinDriver::find`/`find_bounds`
    /// /`inventory`/`screen_has` backing store (quadraui#721). Populated
    /// by draining the shared sink at the end of [`Self::end_frame`];
    /// mirrors `MacBackend::text_runs`'s lifecycle. Not `target_os`-gated
    /// for the same reason as `painted_text_recording` above.
    text_runs: Vec<crate::testing::TextRun>,
    /// Region registry + active-selection state shared by every
    /// `text_selection: true` backend (#741) — see
    /// [`crate::text_selection::TextSelectionState`]'s doc. Not
    /// `target_os`-gated: the state machine itself has no WinAPI
    /// dependency, only [`Self::apply_selection_highlight`]'s actual paint
    /// call does (mirrors `painted_text_recording`/`current_pointer_shape`
    /// above).
    text_selection: crate::text_selection::TextSelectionState,
}

impl WinBackend {
    pub fn new() -> Self {
        Self {
            viewport: Viewport::new(0.0, 0.0, 1.0),
            modal_stack: Rc::new(RefCell::new(ModalStack::new())),
            drag_state: Rc::new(RefCell::new(DragState::new())),
            double_click: DoubleClickDetector::with_radius(WIN_DOUBLE_CLICK_RADIUS),
            accelerators: HashMap::new(),
            parsed_accelerators: Vec::new(),
            focused_activity_bar: None,
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
            #[cfg(target_os = "windows")]
            chrome_dwrite: None,
            #[cfg(target_os = "windows")]
            ui_font_family: DEFAULT_UI_FONT_FAMILY.to_string(),
            #[cfg(target_os = "windows")]
            ui_font_size_pt: DEFAULT_UI_FONT_SIZE_PT,
            current_theme: crate::theme::Theme::default(),
            current_pointer_shape: PointerShape::Default,
            painted_text_recording: false,
            text_runs: Vec::new(),
            text_selection: crate::text_selection::TextSelectionState::default(),
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

    /// Update the cached theme. Ergonomic concrete-type sibling of
    /// [`Backend::set_theme`] — mirrors `GtkBackend::set_current_theme`
    /// (#724).
    pub fn set_current_theme(&mut self, theme: crate::theme::Theme) {
        self.current_theme = theme;
    }

    /// Read-only accessor for the cached theme. Mirrors
    /// `GtkBackend::current_theme`.
    pub fn current_theme(&self) -> &crate::theme::Theme {
        &self.current_theme
    }

    /// Live DirectWrite handles for the current chrome (UI) font, once
    /// [`Self::attach_surface`]/[`Self::attach_headless`] has built them —
    /// `None` beforehand, same lifecycle as the editor `dwrite` field.
    /// No `draw_*` rasteriser consumes this yet (see `chrome_dwrite`'s
    /// field doc); exposed now so a follow-up wiring chrome text onto it
    /// doesn't also need to add the accessor.
    #[cfg(target_os = "windows")]
    pub fn chrome_dwrite(&self) -> Option<&DWrite> {
        self.chrome_dwrite.as_ref()
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
        self.surface = Some(Surface {
            factory: Some(factory),
            target: RenderTarget::Hwnd(target),
        });
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

        // Chrome (UI) font bootstrap (#724) — same DirectWrite dance as
        // the editor font above, but for `ui_font_family`/`ui_font_size_pt`.
        // `line_height`/`char_width` from this format aren't needed:
        // those are always resolved from the editor font, so they're
        // discarded here rather than overwriting `self.current_line_height`.
        let (chrome_dwrite, _, _) = DWrite::new(&self.ui_font_family, self.ui_font_size_pt)?;
        self.chrome_dwrite = Some(chrome_dwrite);

        Ok(())
    }

    /// Attach an offscreen `ID2D1DCRenderTarget` instead of a live
    /// window's — the headless twin of [`Self::attach_surface`] (#707),
    /// used only by [`super::testing::driver_with_shell`] /
    /// [`super::testing::WinDriver`] so a `WinBackend`'s already-landed
    /// rasterisers (#25-#30) can paint somewhere a test can read pixels
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
        self.surface = Some(Surface {
            factory: None,
            target: RenderTarget::Dc(target),
        });

        // Same DirectWrite bootstrap as `attach_surface` — text formats
        // aren't tied to which kind of render target they paint onto.
        let (dwrite, line_height, char_width) =
            DWrite::new(&self.editor_font_family, self.editor_font_size_pt)?;
        self.dwrite = Some(dwrite);
        self.current_line_height = line_height;
        self.current_char_width = char_width;

        // Chrome (UI) font bootstrap — see `attach_surface`'s comment.
        let (chrome_dwrite, _, _) = DWrite::new(&self.ui_font_family, self.ui_font_size_pt)?;
        self.chrome_dwrite = Some(chrome_dwrite);

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
    ///
    /// Also a no-op on the render-target side for a
    /// [`RenderTarget::Dc`] surface ([`Self::attach_headless`]):
    /// `ID2D1HwndRenderTarget::Resize` has no `ID2D1DCRenderTarget`
    /// counterpart — a DC render target is bound once to a fixed-size
    /// DIB section ([`super::testing::HeadlessSurface::new`]) and never
    /// resized, so `win::run`'s `WM_SIZE` handler (the only caller) never
    /// reaches this arm for a headless driver in the first place.
    #[cfg(target_os = "windows")]
    pub(crate) fn resize_surface(&mut self, width: u32, height: u32) -> WinResult<()> {
        let width = width.max(1);
        let height = height.max(1);
        if let Some(surface) = &self.surface {
            if let RenderTarget::Hwnd(target) = &surface.target {
                let size = D2D_SIZE_U { width, height };
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

    // ── ActivityBar keyboard focus (#707) ─────────────────────────────

    /// `WidgetId` of the [`ActivityBar`] that declared
    /// `is_keyboard_focused = true` during the most recent
    /// [`Backend::draw_activity_bar`] call, or `None` if no bar is
    /// focused. Read by `win::run::dispatch_event` — see
    /// `focused_activity_bar`'s field doc for the full contract. Not
    /// `target_os`-gated: the field itself is a plain `Option<WidgetId>`
    /// with no Direct2D dependency, so this reads correctly even before
    /// any surface has attached.
    pub(crate) fn focused_activity_bar_id(&self) -> Option<&WidgetId> {
        self.focused_activity_bar.as_ref()
    }

    // ── Double-click folding (#729) ────────────────────────────────────

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

    // ── Accelerator matching (#707) ───────────────────────────────────

    /// Look up a registered `Global`-scope accelerator for a
    /// `(key, modifiers)` pair. Mirrors `TuiBackend::match_keypress` /
    /// `GtkBackend::match_keypress` / `MacBackend::match_keypress` —
    /// non-Global entries are skipped because this backend doesn't own
    /// focus/mode context the way a scoped `KeyMap` resolver does. Not
    /// `target_os`-gated for the same reason as `focused_activity_bar_id`
    /// above.
    pub(crate) fn match_keypress(&self, key: &Key, modifiers: Modifiers) -> Option<AcceleratorId> {
        let key_name = key_to_binding_name(key);
        for (parsed, id) in &self.parsed_accelerators {
            if parsed.modifiers == modifiers && parsed.key == key_name {
                if let Some(acc) = self.accelerators.get(id) {
                    if matches!(acc.scope, AcceleratorScope::Global) {
                        return Some(id.clone());
                    }
                }
            }
        }
        None
    }

    /// Re-apply `current_pointer_shape` via `SetCursor`/`LoadCursorW`
    /// (#702). Called both by [`Backend::set_cursor`] (an app-driven
    /// change) and by `win::run`'s `WM_SETCURSOR` handler (Windows
    /// re-asking "what cursor belongs here?" on every mouse move over the
    /// client area) — see `current_pointer_shape`'s field doc for why
    /// both call sites need to exist.
    #[cfg(target_os = "windows")]
    pub(crate) fn apply_current_cursor(&self) {
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

    // ── Paint-time text-run recording (quadraui#721) ────────────────────

    /// Enable/disable the paint-time text-run recording that backs
    /// [`super::testing::WinDriver::find`]/`find_bounds`/`inventory`/
    /// `screen_has`. Off by default — see [`Self::text_runs`].
    ///
    /// `target_os`-gated (unlike the `painted_text_recording` field it
    /// writes): its only caller, [`super::testing::WinDriver::new`], lives
    /// in `win::testing`, which is itself windows-only (see that module's
    /// doc) — so on every other host this method has no caller and would
    /// otherwise be flagged `dead_code` under this crate's `-D warnings`.
    #[cfg(target_os = "windows")]
    pub(crate) fn set_painted_text_recording(&mut self, enabled: bool) {
        self.painted_text_recording = enabled;
    }

    /// Text runs recorded during the last [`Self::begin_frame`]/
    /// [`Self::end_frame`] bracket, when [`Self::set_painted_text_recording`]
    /// is on. `target_os`-gated for the same reason that method is — see
    /// its doc.
    #[cfg(target_os = "windows")]
    pub(crate) fn text_runs(&self) -> &[crate::testing::TextRun] {
        &self.text_runs
    }

    // ── Text selection (#741) ────────────────────────────────────────────
    //
    // The region registry + active-selection state machine lives in
    // [`crate::text_selection::TextSelectionState`] — the same shared
    // implementation `GtkBackend`/`TuiBackend` embed. Every method below
    // except [`Self::apply_selection_highlight`]/[`Self::extract_selection_text`]
    // (Direct2D painting / `TextRegion::lines` extraction, this backend's
    // own — mirrors `GtkBackend`'s pixel-based twins via the shared
    // `crate::text_selection::pixel_selection_ranges`/`extract_lines_pixel`
    // helpers) is a thin delegation.

    /// Every `TextRegion` registered so far this frame.
    pub(crate) fn text_regions(&self) -> &[TextRegion] {
        &self.text_selection.text_regions
    }

    /// Return the current active text selection, if any.
    pub(crate) fn active_text_selection(&self) -> Option<&crate::text_selection::TextSelection> {
        self.text_selection.active_text_selection()
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
        self.text_selection
            .set_active_text_selection(region, anchor, focus);
    }

    /// Clear the active text selection highlight only (does NOT end an
    /// in-progress `TextSelection` drag). Called before dispatching a new
    /// mouse-down so the old highlight disappears without interrupting the
    /// drag that is about to start. Mirrors `GtkBackend::clear_selection_display`.
    pub(crate) fn clear_selection_display(&mut self) {
        self.text_selection.clear_selection_display();
    }

    /// Clear the active text selection and end any in-progress
    /// `TextSelection` drag. Called after Ctrl-C copies the selection or
    /// on a plain click outside any text region.
    pub(crate) fn clear_text_selection(&mut self) {
        let mut drag = self.drag_state.borrow_mut();
        self.text_selection.clear_text_selection(&mut drag);
    }

    /// End any in-progress `TextSelection` drag without clearing the
    /// displayed selection. Backs the `Backend` trait's
    /// `cancel_text_selection_drag` override — apps hosting an embedded
    /// terminal call it to abort a speculative drag before forwarding a
    /// click to a PTY. Mirrors `GtkBackend::cancel_text_selection_drag_impl`/
    /// `TuiBackend::cancel_text_selection_drag_impl`.
    fn cancel_text_selection_drag_impl(&mut self) {
        let mut drag = self.drag_state.borrow_mut();
        self.text_selection.cancel_text_selection_drag(&mut drag);
    }

    /// Record that `id` is the most-recently focused/clicked `TextRegion`.
    /// Called by `win::run`'s mouse-down handling after a `TextSelection`
    /// drag begins, so [`Self::select_all_text_region`] can resolve the
    /// correct target even before the first drag-move fires a
    /// `TextSelectionChanged` event.
    pub(crate) fn track_focused_text_region(&mut self, id: WidgetId) {
        self.text_selection.track_focused_text_region(id);
    }

    /// Set the active selection to cover the entire visible content of the
    /// most-recently focused `TextRegion` (the Ctrl-A target). See
    /// [`crate::text_selection::TextSelectionState::select_all_text_region`]
    /// for the resolution order and the viewport-only limitation.
    pub(crate) fn select_all_text_region(&mut self) -> bool {
        self.text_selection.select_all_text_region()
    }

    /// Extract the selected text from the active selection's `TextRegion`
    /// using its stored `lines` (Win-GUI is pixel-based, like GTK — see
    /// `TextRegion::lines`'s doc). Empty when there is no active selection,
    /// the region isn't registered this frame, or it has no `lines`
    /// content.
    pub(crate) fn extract_selection_text(&self) -> String {
        let Some(sel) = self.text_selection.active_text_selection() else {
            return String::new();
        };
        let Some(region) = self.text_selection.find_region(&sel.region) else {
            return String::new();
        };
        crate::text_selection::extract_lines_pixel(
            region,
            sel.anchor,
            sel.focus,
            self.current_line_height,
            self.current_char_width,
        )
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
    pub(crate) fn apply_selection_highlight(&self) {
        let Some(sel) = self.text_selection.active_text_selection() else {
            return;
        };
        let Some(region) = self.text_selection.find_region(&sel.region) else {
            return;
        };
        let Some(ranges) = crate::text_selection::pixel_selection_ranges(
            region.bounds,
            sel.anchor,
            sel.focus,
            self.current_line_height,
            self.current_char_width,
        ) else {
            return;
        };
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            let char_w = self.current_char_width;
            let line_h = self.current_line_height;
            // Same translucent-blue highlight `GtkBackend::apply_selection_highlight`
            // paints (`rgba(0.39, 0.58, 1.0, 0.30)`), converted to `Color`'s
            // 0-255 channels.
            let highlight = crate::Color::rgba(100, 148, 255, 77);
            for (row_cell, col_start, col_end) in ranges {
                let width = col_end - col_start;
                if width <= 0.0 {
                    continue;
                }
                let rect = crate::event::Rect::new(
                    region.bounds.x + col_start * char_w,
                    region.bounds.y + row_cell as f32 * line_h,
                    width * char_w,
                    line_h,
                );
                let _ = super::text::fill_rect(&surface.target, rect, highlight);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = ranges;
        }
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

/// Map a [`PointerShape`] to the Win32 `IDC_*` cursor-resource `PCWSTR`
/// [`LoadCursorW`] expects (#702, adopting `desktop::ALL_RESIZE_EDGES`/
/// `desktop::all_pointer_shapes` — see
/// `pointer_shape_to_win32_cursor_maps_every_variant` below).
///
/// Win32's stock cursor set has no *separate* NE-only vs. SW-only (resp.
/// NW-only vs. SE-only) diagonal-resize glyph the way GTK's CSS keyword
/// set does (`"ne-resize"` and `"sw-resize"` are visually identical but
/// distinct keywords there) — `IDC_SIZENESW`/`IDC_SIZENWSE` are each a
/// single double-headed arrow already shared between its two opposite
/// corners at the OS resource level, unrelated to AppKit's *complete
/// absence* of a public diagonal-resize cursor (see
/// `MacBackend::mac_cursor_for_shape`'s doc comment, a different and
/// more severe gap this backend doesn't have). Every
/// [`crate::backend::ResizeEdge`] variant below still gets a
/// direction-correct cursor; only the specific `IDC_*` resource is
/// shared per diagonal, matching how the double-headed arrow looks
/// identical from either end.
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
    }
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
        // Clear the focused activity bar — re-set by `draw_activity_bar`
        // during this frame's render pass if still focused. Same
        // lifecycle as `GtkBackend`/`MacBackend`'s `focused_activity_bar`
        // (quadraui#707).
        self.focused_activity_bar = None;
        // Clear per-frame text regions so stale registrations from the
        // previous frame don't linger. Mirrors `GtkBackend`/`TuiBackend`'s
        // identical `begin_frame` clear (#741).
        self.text_selection.begin_frame();
        // Install the shared paint-time text-run recording sink for the
        // duration of this frame — drained into `self.text_runs` by
        // `end_frame` below. Mirrors `MacBackend::enter_frame_scope`'s
        // start/stop bracket (quadraui#721); Win-GUI needs no closure-based
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
        // Drain the sink `begin_frame` installed, if recording was on —
        // see that method's doc for why this needs no closure-based frame
        // scope the way GTK/macOS's `enter_frame_scope` does.
        if self.painted_text_recording {
            self.text_runs = crate::testing::take_text_run_sink(None);
        }
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

    // ─── Theming ────────────────────────────────────────────────────────

    /// Store the theme so the `draw_*` rasterisers below stop falling
    /// back to `Theme::default()` regardless of what the app configured
    /// (#724 — quadraui#492's headline example). Mirrors
    /// `GtkBackend::set_theme`/`MacBackend::set_theme`.
    fn set_theme(&mut self, theme: crate::Theme) {
        self.set_current_theme(theme);
    }

    /// Store the chrome font description for the next
    /// [`Self::attach_surface`]/[`Self::attach_headless`] call to build a
    /// chrome `IDWriteTextFormat` from (#724), parsed via
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

    // ─── Events + keybindings ─────────────────────────────────────────

    fn poll_events(&mut self) -> Vec<UiEvent> {
        todo!("PeekMessage loop → translate WM_* → UiEvent")
    }

    fn wait_events(&mut self, _timeout: Duration) -> Vec<UiEvent> {
        todo!("MsgWaitForMultipleObjects + GetMessage → UiEvent")
    }

    fn register_accelerator(&mut self, acc: &Accelerator) {
        self.accelerators.insert(acc.id.clone(), acc.clone());
        // Keep `parsed_accelerators` in sync so `Self::match_keypress`
        // (read by `win::run::dispatch_event`, quadraui#707) actually
        // sees this registration — mirrors `GtkBackend`/`TuiBackend`/
        // `MacBackend::register_accelerator`. Before this, an entry only
        // ever landed in `self.accelerators`, which nothing in `win/`
        // read back.
        self.parsed_accelerators.retain(|(_, id)| id != &acc.id);
        if let Some(parsed) = parse_binding(&acc.binding) {
            self.parsed_accelerators.push((parsed, acc.id.clone()));
        }
    }

    fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.accelerators.remove(id);
        self.parsed_accelerators.retain(|(_, eid)| eid != id);
    }

    // ─── Text selection (#741) ──────────────────────────────────────────

    /// Overrides the trait's no-op default — see [`Self::text_regions`]
    /// and `crate::text_selection::TextSelectionState`.
    fn register_text_region(&mut self, region: TextRegion) {
        self.text_selection.register_text_region(region);
    }

    /// Overrides the trait's no-op default — see
    /// [`Self::cancel_text_selection_drag_impl`].
    fn cancel_text_selection_drag(&mut self) {
        self.cancel_text_selection_drag_impl();
    }

    // ─── Modal-overlay tracking ───────────────────────────────────────

    fn modal_stack_handle(&self) -> Rc<RefCell<ModalStack>> {
        self.modal_stack.clone()
    }

    fn drag_state_handle(&self) -> Rc<RefCell<DragState>> {
        self.drag_state.clone()
    }

    // ─── Platform services ────────────────────────────────────────────

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    // ─── Cursor ───────────────────────────────────────────────────────

    /// #702: maps `shape` onto a `SetCursor(LoadCursorW(..))` call via
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
    /// quadraui#723: `mouse`/`scroll`/`drag` are now declared `true` — the
    /// translators are wired (`WM_LBUTTONDOWN`/`WM_LBUTTONUP`/
    /// `WM_MOUSEMOVE`/`WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` all reach
    /// `dispatch_event`, per #20/#707), and the painted-text sink prereq
    /// (verification spine step 1) means a scenario that now runs instead
    /// of skipping fails on real signal rather than panicking mid-scenario
    /// on a missing text lookup.
    ///
    /// quadraui#741: `text_selection` is now declared `true` too —
    /// `register_text_region`/`cancel_text_selection_drag` are both
    /// overridden, backed by the same `crate::text_selection::TextSelectionState`
    /// `GtkBackend`/`TuiBackend` embed, so `panel.drag_select_copy` (which
    /// `requires: ["text_selection"]`) now runs instead of skipping.
    fn backend_caps(&self) -> crate::backend::BackendCaps {
        // A genuine struct literal (not `let mut caps = ...; caps.field =
        // true;`), matching every other backend's `backend_caps` shape —
        // `tests/conformance/caps.rs`'s `BackendSource::declared` parses
        // this method's source for `<field>: true,` lines, so an
        // assignment-statement form would silently parse as declaring
        // nothing (#702 review-fix note).
        #[cfg(target_os = "windows")]
        {
            // #23: file dialogs (`IFileOpenDialog`/`IFileSaveDialog`) and
            // notifications (`Shell_NotifyIconW`) go through COM/Shell
            // APIs independent of the Direct2D rasteriser work above, so
            // they're honestly `true` on Windows itself.
            //
            // `native_dialogs` (#744): `show_message_dialog` now shows a
            // real `TaskDialogIndirect` alert and returns the chosen
            // button's id — see `src/win/services.rs::win_show_message_dialog`.
            // `CAP_CONTRACTS`'s `native_dialogs` entry is `Unprovable`
            // (there is no no-op default `show_message_dialog` diverges
            // from — every backend implements it), so this declaration
            // isn't source-checked by the honesty test; it's true because
            // the alert is real, not because anything can parse that.
            //
            // `pointer_cursor` (#702): `set_cursor` now drives a real
            // `SetCursor`/`WM_SETCURSOR` round-trip instead of the
            // trait's no-op default — see `Self::set_cursor` /
            // `Self::apply_current_cursor`.
            //
            // `mouse`/`scroll`/`drag` (#723): the translators are wired —
            // `WM_LBUTTONDOWN`/`WM_LBUTTONUP`/`WM_MOUSEMOVE` reach
            // `dispatch_event` as `MouseDown`/`MouseUp`/`MouseMoved`, and
            // `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` reach it as `Scroll` (#20,
            // #707) — so a press → move → release sequence is a real
            // drag.
            //
            // `text_selection` (#741): `register_text_region` and
            // `cancel_text_selection_drag` are both overridden above,
            // backed by the shared `crate::text_selection::TextSelectionState`
            // GTK/TUI also embed — `panel.drag_select_copy` (which
            // `requires: ["text_selection"]`) now runs instead of skipping.
            crate::backend::BackendCaps {
                file_dialogs: true,
                native_dialogs: true,
                notifications: true,
                pointer_cursor: true,
                mouse: true,
                scroll: true,
                drag: true,
                text_selection: true,
                ..crate::backend::BackendCaps::empty()
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            crate::backend::BackendCaps::empty()
        }
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

    fn list_layout(&self, rect: Rect, list: &ListView) -> crate::ListViewLayout {
        // Same two-block shape as `tree_layout` below (and deliberately
        // NOT `return … ;` inside the windows arm): on a Windows host the
        // `not(windows)` block is cfg'd away, so the `return` would be the
        // function's own tail expression and `clippy::needless_return`
        // fires — a lint only the windows-latest CI leg can see, since on
        // Linux the second block follows and the `return` is load-bearing.
        #[cfg(target_os = "windows")]
        {
            super::list::win_list_layout(list, rect, self.current_line_height)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, list);
            todo!("DirectWrite list layout — src/win/list.rs is target_os=\"windows\"-gated")
        }
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

    /// #734: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_settings_chrome(
        &mut self,
        rect: Rect,
        header_text: &str,
        query: &str,
        placeholder: &str,
        active: bool,
    ) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::form::draw_settings_chrome(
                &surface.target,
                dwrite,
                rect,
                self.current_line_height,
                header_text,
                query,
                placeholder,
                active,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, header_text, query, placeholder, active);
        todo!("Direct2D settings chrome rasteriser (no surface attached yet)")
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
                &self.current_theme,
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
        // Track keyboard focus so `win::run::dispatch_event` can redirect
        // the next `KeyPressed` into this bar (#707) — same contract
        // `GtkBackend`/`MacBackend::draw_activity_bar` implement. Runs
        // regardless of whether a surface is attached yet, so a headless
        // `WinDriver` sees focus tracking even before #707's rasteriser
        // dependencies land a surface.
        if bar.is_keyboard_focused {
            self.focused_activity_bar = Some(bar.id.clone());
        }
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::activity_bar::draw_activity_bar(
                &surface.target,
                dwrite,
                rect,
                bar,
                hovered_idx,
                &self.current_theme,
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
                        y_start: vi.bounds.y,
                        y_end: vi.bounds.y + vi.bounds.height,
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

    /// #30: real Direct2D/DirectWrite rasteriser via `win::terminal` once
    /// a surface is attached. Mirrors `GtkBackend::draw_terminal`'s
    /// scrollbar handling (subtract the gutter from the cell area, paint
    /// cells, then paint the scrollbar over the freed strip) but skips
    /// the #417 dirty-row cache — see `win::terminal`'s module doc for
    /// why that's a deliberate, follow-up-sized scope cut. See
    /// [`Self::draw_status_bar`]'s doc for the "surface not attached yet"
    /// fallback posture.
    fn draw_terminal(&mut self, rect: Rect, term: &Terminal) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            let lh = self.current_line_height;
            let cw = self.current_char_width;
            let theme = &self.current_theme;

            let sb_width = match &term.scrollbar {
                Some(sb) => sb.width.map(|w| w as f32).unwrap_or(8.0),
                None => 0.0,
            };
            let cell_area_w = (rect.width - sb_width).max(0.0);

            super::terminal::draw_terminal_cells(
                &surface.target,
                dwrite,
                term,
                rect.x,
                rect.y,
                cell_area_w,
                rect.height,
                lh,
                cw,
                theme,
            );

            if let Some(ref sb_state) = term.scrollbar {
                let sb = crate::primitives::scrollbar::Scrollbar::vertical(
                    term.id.clone(),
                    Rect::new(rect.x + cell_area_w, rect.y, sb_width, rect.height),
                    sb_state.effective_scroll_offset() as f32,
                    sb_state.total_lines as f32,
                    sb_state.visible_lines as f32,
                    lh,
                );
                super::scrollbar::draw_scrollbar(&surface.target, &sb, theme);
            }
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, term);
        todo!("Direct2D terminal cell grid rasteriser (no surface attached yet)")
    }

    /// #30: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture. `Terminal` paints no text, so
    /// unlike most of that method's siblings this doesn't need
    /// `self.dwrite`.
    fn draw_terminal_divider(&mut self, rect: Rect) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            super::terminal::draw_terminal_divider(
                &surface.target,
                rect.x,
                rect.y,
                rect.height,
                &self.current_theme,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = rect;
        todo!("Direct2D terminal split divider rasteriser (no surface attached yet)")
    }

    /// #30: real Direct2D/DirectWrite rasteriser via `win::text_display`
    /// once a surface is attached. See [`Self::draw_status_bar`]'s doc
    /// for the "surface not attached yet" fallback posture.
    fn draw_text_display(&mut self, rect: Rect, td: &TextDisplay) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::text_display::draw_text_display(
                &surface.target,
                dwrite,
                rect,
                td,
                &self.current_theme,
                self.current_line_height,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, td);
        todo!("Direct2D text display rasteriser (no surface attached yet)")
    }

    /// #725: real Direct2D/DirectWrite rasteriser via `win::command_line`
    /// once a surface is attached. See [`Self::draw_status_bar`]'s doc
    /// for the "surface not attached yet" fallback posture.
    fn draw_command_line(
        &mut self,
        rect: Rect,
        cmd: &crate::primitives::command_line::CommandLine,
    ) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::command_line::draw_command_line(
                &surface.target,
                dwrite,
                rect,
                cmd,
                &self.current_theme,
                self.current_char_width,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, cmd);
        todo!("Direct2D command line rasteriser (no surface attached yet)")
    }

    /// #725: pure measurement — only needs `current_char_width`, not a
    /// live render target or `self.dwrite` (mirrors
    /// `Self::text_display_layout`'s doc for why this only needs the
    /// `target_os = "windows"` gate every method in this file shares). No
    /// `return` in the `windows` arm — see [`Self::activity_bar_layout`]'s
    /// doc for why.
    fn command_line_layout(
        &self,
        rect: Rect,
        cmd: &crate::primitives::command_line::CommandLine,
    ) -> crate::primitives::command_line::CommandLineLayout {
        #[cfg(target_os = "windows")]
        {
            super::command_line::win_command_line_layout(cmd, rect, self.current_char_width)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, cmd);
            todo!("Direct2D command line layout")
        }
    }

    /// #30: pure measurement — only needs `line_height`, not a live
    /// render target or `self.dwrite` (mirrors `MacBackend::text_display_layout`
    /// / `GtkBackend::text_display_layout`, both row-count-based rather
    /// than real text measurement) — but still lives in a Windows-only
    /// module, so this only needs the `target_os = "windows"` gate every
    /// method in this file shares. No `return` in the `windows` arm —
    /// see [`Self::activity_bar_layout`]'s doc for why.
    fn text_display_layout(&self, rect: Rect, td: &TextDisplay) -> TextDisplayLayout {
        #[cfg(target_os = "windows")]
        {
            super::text_display::win_text_display_layout(td, rect, self.current_line_height)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, td);
            todo!("DirectWrite text display layout")
        }
    }

    /// #733: real Direct2D/DirectWrite rasteriser via `win::text_input`
    /// once a surface is attached. See [`Self::draw_status_bar`]'s doc
    /// for the "surface not attached yet" fallback posture.
    fn draw_text_input(
        &mut self,
        rect: Rect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::text_input::draw_text_input(
                &surface.target,
                dwrite,
                rect,
                ti,
                &self.current_theme,
                self.current_line_height,
                self.current_char_width,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, ti);
        todo!("Direct2D text input rasteriser (no surface attached yet)")
    }

    /// #733: pure measurement — only needs `current_line_height`/
    /// `current_char_width`, not a live render target or `self.dwrite`
    /// (mirrors [`Self::command_line_layout`]'s doc for why this only
    /// needs the `target_os = "windows"` gate every method in this file
    /// shares). No `return` in the `windows` arm — see
    /// [`Self::activity_bar_layout`]'s doc for why.
    fn text_input_layout(
        &self,
        rect: Rect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        #[cfg(target_os = "windows")]
        {
            super::text_input::win_text_input_layout(
                ti,
                rect,
                self.current_line_height,
                self.current_char_width,
            )
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, ti);
            todo!("Direct2D text input layout")
        }
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
            return super::context_menu::draw_context_menu(
                &surface.target,
                dwrite,
                menu,
                layout,
                &self.current_theme,
            );
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

    /// #30: real Direct2D/DirectWrite rasteriser via `win::message_list`
    /// once a surface is attached. Mirrors `GtkBackend::draw_message_list`
    /// / `MacBackend::draw_message_list` — no panel-background fill here;
    /// hosts that want one paint it before calling (see
    /// `win::message_list`'s module doc). See [`Self::draw_status_bar`]'s
    /// doc for the "surface not attached yet" fallback posture.
    fn draw_message_list(&mut self, rect: Rect, list: &MessageList) {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            super::message_list::draw_message_list(
                &surface.target,
                dwrite,
                list,
                rect.x,
                rect.y,
                rect.y + rect.height,
                self.current_line_height,
            );
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, list);
        todo!("Direct2D message list rasteriser (no surface attached yet)")
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
                &self.current_theme,
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
            super::completions::draw_completions(
                &surface.target,
                dwrite,
                completions,
                layout,
                &self.current_theme,
            );
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
            super::scrollbar::draw_scrollbar(&surface.target, scrollbar, &self.current_theme);
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, scrollbar);
        todo!("Direct2D scrollbar rasteriser (no surface attached yet)")
    }

    /// #726: real Direct2D rasteriser via `win::drop_overlay` once a
    /// surface is attached. See [`Self::draw_status_bar`]'s doc for the
    /// "surface not attached yet" fallback posture.
    fn draw_drop_overlay(&mut self, overlay: &crate::primitives::drop_zone::DropOverlay) {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            super::drop_overlay::draw_drop_overlay(&surface.target, overlay, &self.current_theme);
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = overlay;
        todo!("Direct2D drop overlay rasteriser (no surface attached yet)")
    }

    /// #25: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_menu_bar(&mut self, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::menu_bar::draw_menu_bar(
                &surface.target,
                dwrite,
                rect,
                bar,
                &self.current_theme,
            );
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

    /// #29: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture. `Split` paints no text, so unlike
    /// most of that method's siblings this doesn't need `self.dwrite`.
    fn draw_split(&mut self, rect: Rect, split: &Split) -> SplitLayout {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            return super::split::draw_split(&surface.target, rect, split);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, split);
        todo!("Direct2D split rasteriser (no surface attached yet)")
    }

    /// #29: pure geometry — no measurer at all (uniform divider
    /// thickness), so unlike most `*_layout` siblings this doesn't even
    /// need `self.dwrite`, only kept behind the `target_os = "windows"`
    /// gate for consistency with every other method in this file.
    fn split_layout(&self, rect: Rect, split: &Split) -> SplitLayout {
        // No `return` here (unlike the `if let Some(dwrite)` early-returns
        // elsewhere in this file): on `target_os = "windows"` the
        // `not(windows)` block below is stripped, so this block *is* the
        // tail expression and an explicit `return` trips
        // `clippy::needless_return` — an error under CI's `-D warnings`,
        // and only on the windows-latest leg. See `activity_bar_layout`'s
        // matching comment.
        #[cfg(target_os = "windows")]
        {
            super::split::win_split_layout(rect, split)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, split);
            todo!("DirectWrite split layout (no surface attached yet)")
        }
    }

    /// #740: see [`Self::draw_split`]'s doc for the "surface not attached
    /// yet" fallback posture. `SplitTree` paints no text, so unlike most
    /// of that method's siblings this doesn't need `self.dwrite`.
    fn draw_split_tree(
        &mut self,
        rect: Rect,
        tree: &crate::primitives::split_tree::SplitTree,
    ) -> crate::primitives::split_tree::SplitTreeLayout {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            return super::split_tree::draw_split_tree(&surface.target, rect, tree);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, tree);
        todo!("Direct2D split-tree rasteriser (no surface attached yet)")
    }

    /// #740: pure geometry — no measurer at all (uniform divider
    /// thickness), so unlike most `*_layout` siblings this doesn't even
    /// need `self.dwrite`, only kept behind the `target_os = "windows"`
    /// gate for consistency with every other method in this file. See
    /// [`Self::split_layout`]'s comment for why this block has no
    /// `return`.
    fn split_tree_layout(
        &self,
        rect: Rect,
        tree: &crate::primitives::split_tree::SplitTree,
    ) -> crate::primitives::split_tree::SplitTreeLayout {
        #[cfg(target_os = "windows")]
        {
            super::split_tree::win_split_tree_layout(rect, tree)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, tree);
            todo!("DirectWrite split-tree layout (no surface attached yet)")
        }
    }

    /// #736: real Direct2D/DirectWrite rasteriser via `win::board` once a
    /// surface is attached. See [`Self::draw_status_bar`]'s doc for the
    /// "surface not attached yet" fallback posture.
    fn draw_board(&mut self, rect: Rect, model: &crate::BoardModel) -> crate::BoardLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::board::draw_board(
                &surface.target,
                dwrite,
                rect,
                model,
                &self.current_theme,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, model);
        todo!("Direct2D board rasteriser (no surface attached yet)")
    }

    /// #736: pure geometry — `board_layout` needs no measurer at all
    /// (fixed DIP constants, same posture as
    /// [`Self::pipeline_view_layout`]'s doc), only kept behind the
    /// `target_os = "windows"` gate for consistency with every other
    /// method in this file. No `return` in the `windows` arm — see
    /// [`Self::activity_bar_layout`]'s doc for why.
    fn board_layout(&self, rect: Rect, model: &crate::BoardModel) -> crate::BoardLayout {
        #[cfg(target_os = "windows")]
        {
            super::board::win_board_layout(model, rect)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, model);
            todo!("Direct2D board layout — no rasteriser to keep in sync with yet, see draw_board")
        }
    }

    /// #738: real Direct2D/DirectWrite rasteriser via `win::minimap` once a
    /// surface is attached — see [`Self::draw_status_bar`]'s doc for the
    /// "surface not attached yet" fallback posture.
    fn draw_minimap(
        &mut self,
        rect: Rect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::backend::MinimapPaintResult {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            let layout = super::minimap::draw_minimap(
                &surface.target,
                dwrite,
                rect,
                minimap,
                &self.current_theme,
            );
            return crate::backend::MinimapPaintResult { layout };
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, minimap);
        todo!("Direct2D minimap rasteriser (no surface attached yet)")
    }

    /// #738: pure geometry — `minimap_layout` needs no measurer at all
    /// (fixed DIP constants, same posture as
    /// [`Self::board_layout`]'s doc), only kept behind the
    /// `target_os = "windows"` gate for consistency with every other
    /// method in this file.
    fn minimap_layout(
        &self,
        rect: Rect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::primitives::minimap::MinimapLayout {
        #[cfg(target_os = "windows")]
        {
            super::minimap::win_minimap_layout(minimap, rect)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, minimap);
            todo!("Direct2D minimap layout — no rasteriser to keep in sync with yet, see draw_minimap")
        }
    }

    /// #739: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture. Only needs `self.surface` (not
    /// `self.dwrite` too) — WIC decoding and `DrawBitmap` are pure
    /// Direct2D, no text measurement involved.
    fn draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        #[cfg(target_os = "windows")]
        if let Some(surface) = &self.surface {
            return super::image::draw_image(&surface.target, rect, image);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, image);
        todo!("Direct2D image rasteriser (no surface attached yet)")
    }

    /// #29: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_panel(&mut self, rect: Rect, panel: &Panel) -> PanelLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::panel::draw_panel(
                &surface.target,
                dwrite,
                rect,
                panel,
                self.current_line_height,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, panel);
        todo!("Direct2D panel rasteriser (no surface attached yet)")
    }

    /// #29: pure geometry — `Panel::layout` needs only `line_height`,
    /// not text measurement, so this only needs `self.dwrite` to exist
    /// (for the `target_os = "windows"` gate below) rather than a live
    /// render target — same posture as [`Self::status_bar_layout`].
    fn panel_layout(&self, rect: Rect, panel: &Panel) -> PanelLayout {
        #[cfg(target_os = "windows")]
        if self.dwrite.is_some() {
            return super::panel::win_panel_layout(rect, panel, self.current_line_height);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, panel);
        todo!("DirectWrite panel layout (no surface attached yet)")
    }

    /// #29: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_toast_stack(&mut self, rect: Rect, stack: &ToastStack) -> ToastStackLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::toast::draw_toast_stack(
                &surface.target,
                dwrite,
                rect,
                stack,
                self.current_line_height,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, stack);
        todo!("Direct2D toast stack rasteriser (no surface attached yet)")
    }

    /// #29: pure measurement — only needs `self.dwrite`, not a live
    /// render target, so this works as soon as a surface has ever been
    /// attached — same posture as [`Self::status_bar_layout`].
    fn toast_stack_layout(&self, rect: Rect, stack: &ToastStack) -> ToastStackLayout {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = &self.dwrite {
            return super::toast::win_toast_stack_layout(
                dwrite,
                rect,
                stack,
                self.current_line_height,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, stack);
        todo!("DirectWrite toast stack layout (no surface attached yet)")
    }

    /// #735: real Direct2D/DirectWrite rasteriser via `win::pipeline_view`
    /// once a surface is attached. See [`Self::draw_status_bar`]'s doc for
    /// the "surface not attached yet" fallback posture.
    fn draw_pipeline_view(
        &mut self,
        rect: Rect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::pipeline_view::draw_pipeline_view(
                &surface.target,
                dwrite,
                rect,
                view,
                &self.current_theme,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, view);
        todo!("Direct2D pipeline view rasteriser (no surface attached yet)")
    }

    /// #735: pure geometry — `PipelineView::layout` needs no measurer at
    /// all (fixed DIP constants for arrow/action-height), so unlike most
    /// `*_layout` siblings this doesn't even need `self.dwrite`, only
    /// kept behind the `target_os = "windows"` gate for consistency with
    /// every other method in this file (mirrors
    /// [`Self::progress_layout`]'s doc). No `return` in the `windows`
    /// arm — see [`Self::activity_bar_layout`]'s doc for why.
    fn pipeline_view_layout(
        &self,
        rect: Rect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        #[cfg(target_os = "windows")]
        {
            super::pipeline_view::win_pipeline_view_layout(view, rect)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, view);
            todo!("DirectWrite pipeline view layout")
        }
    }

    /// #29: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_progress(&mut self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::progress::draw_progress(&surface.target, dwrite, rect, bar);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, bar);
        todo!("Direct2D progress bar rasteriser (no surface attached yet)")
    }

    /// #29: pure geometry — `ProgressBar::layout` needs no measurer at
    /// all (uniform cancel-affordance width), so unlike most
    /// `*_layout` siblings this doesn't even need `self.dwrite`, only
    /// kept behind the `target_os = "windows"` gate for consistency
    /// with every other method in this file.
    fn progress_layout(&self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
        // See `split_layout`'s comment on why this block has no `return`.
        #[cfg(target_os = "windows")]
        {
            super::progress::win_progress_layout(rect, bar)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, bar);
            todo!("DirectWrite progress layout (no surface attached yet)")
        }
    }

    /// #29: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_spinner(&mut self, rect: Rect, spinner: &Spinner) -> SpinnerLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::spinner::draw_spinner(&surface.target, dwrite, rect, spinner);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, spinner);
        todo!("Direct2D spinner rasteriser (no surface attached yet)")
    }

    /// #29: pure measurement — only needs `self.dwrite`, not a live
    /// render target, so this works as soon as a surface has ever been
    /// attached — same posture as [`Self::status_bar_layout`].
    fn spinner_layout(&self, rect: Rect, spinner: &Spinner) -> SpinnerLayout {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = &self.dwrite {
            return super::spinner::win_spinner_layout(dwrite, rect, spinner);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, spinner);
        todo!("DirectWrite spinner layout (no surface attached yet)")
    }

    /// #732: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_command_center(&mut self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::command_center::draw_command_center(
                &surface.target,
                dwrite,
                self.current_char_width,
                self.current_line_height,
                rect,
                cc,
                &self.current_theme,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, cc);
        todo!("Direct2D command center rasteriser (no surface attached yet)")
    }

    /// #732: pure `char_width` estimate (via the shared
    /// [`crate::primitives::command_center::CommandCenterMeasure::from_char_width`]
    /// formula) — no live `DWrite` measurer is needed, same posture as
    /// [`Self::chart_layout`].
    fn command_center_layout(&self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout {
        // See `progress_layout`'s comment on why this block has no `return`.
        #[cfg(target_os = "windows")]
        {
            super::command_center::win_command_center_layout(self.current_char_width, rect, cc)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (rect, cc);
            todo!("DirectWrite command center layout (no surface attached yet)")
        }
    }

    /// #730: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture.
    fn draw_toolbar(
        &mut self,
        rect: Rect,
        bar: &crate::primitives::toolbar::Toolbar,
        hovered_id: Option<&crate::types::WidgetId>,
        pressed_id: Option<&crate::types::WidgetId>,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::toolbar::draw_toolbar(
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
        todo!("Direct2D toolbar rasteriser (no surface attached yet)")
    }

    /// #730: see [`Self::status_bar_layout`]'s doc for why this only
    /// needs `self.dwrite`.
    fn toolbar_layout(
        &self,
        rect: Rect,
        bar: &crate::primitives::toolbar::Toolbar,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = &self.dwrite {
            return super::toolbar::win_toolbar_layout(dwrite, rect, bar);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, bar);
        todo!("DirectWrite toolbar layout (no surface attached yet)")
    }

    /// #731: see [`Self::draw_status_bar`]'s doc for the "surface not
    /// attached yet" fallback posture. `SidebarPanel` embeds a
    /// [`crate::primitives::toolbar::Toolbar`] header — `win::sidebar_panel`
    /// delegates the slot to `super::toolbar::draw_toolbar` rather than
    /// re-deriving toolbar geometry, mirroring `gtk::sidebar_panel` /
    /// `macos::sidebar_panel`.
    fn draw_sidebar_panel(
        &mut self,
        rect: Rect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
        hovered_toolbar_id: Option<&crate::types::WidgetId>,
        pressed_toolbar_id: Option<&crate::types::WidgetId>,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::sidebar_panel::draw_sidebar_panel(
                &surface.target,
                dwrite,
                self.current_line_height,
                rect,
                panel,
                hovered_toolbar_id,
                pressed_toolbar_id,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, panel, hovered_toolbar_id, pressed_toolbar_id);
        todo!("Direct2D sidebar-panel rasteriser (no surface attached yet)")
    }

    /// #731: see [`Self::toolbar_layout`]'s doc for why this only needs
    /// `self.dwrite` (text measurement for the nested toolbar's button
    /// widths), not a live render target.
    fn sidebar_panel_layout(
        &self,
        rect: Rect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        #[cfg(target_os = "windows")]
        if let Some(dwrite) = &self.dwrite {
            return super::sidebar_panel::win_sidebar_panel_layout(
                dwrite,
                self.current_line_height,
                rect,
                panel,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, panel);
        todo!("DirectWrite sidebar-panel layout (no surface attached yet)")
    }

    /// #737: `diff_view_layout` is **not** overridden on this backend —
    /// see `win::diff_view`'s module doc for why the trait default stays
    /// the honest answer even after the shared `DiffView::layout` landed.
    fn draw_diff_view(
        &mut self,
        rect: Rect,
        view: &crate::primitives::diff_view::DiffView,
    ) -> crate::primitives::diff_view::DiffViewLayout {
        #[cfg(target_os = "windows")]
        if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
            return super::diff_view::draw_diff_view(
                &surface.target,
                dwrite,
                rect,
                view,
                &self.current_theme,
                self.current_line_height,
            );
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (rect, view);
        todo!("Direct2D DiffView rasteriser (no surface attached yet)")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// quadraui#699: `WinBackend::modal_stack_handle` must hand back a
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

    /// quadraui#699: the stash-then-reuse pattern this issue exists to
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

    /// quadraui#699: same shared-state guarantee as
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

    // ── Accelerator matching + ActivityBar keyboard focus (#707) ───────
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

    #[test]
    fn register_and_unregister_accelerator_round_trip() {
        let mut b = WinBackend::new();
        let a = acc("save", "<C-s>");
        b.register_accelerator(&a);
        assert!(b.accelerators.contains_key(&AcceleratorId::new("save")));
        assert_eq!(b.parsed_accelerators.len(), 1);
        b.unregister_accelerator(&AcceleratorId::new("save"));
        assert!(!b.accelerators.contains_key(&AcceleratorId::new("save")));
        assert!(b.parsed_accelerators.is_empty());
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
        assert_eq!(b.parsed_accelerators.len(), 1);
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

    // ── Double-click folding (#729) ─────────────────────────────────────
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

    #[test]
    fn focused_activity_bar_id_starts_none() {
        let b = WinBackend::new();
        assert!(b.focused_activity_bar_id().is_none());
    }

    #[test]
    fn draw_activity_bar_tracks_keyboard_focus() {
        let mut b = WinBackend::new();
        let bar = ActivityBar {
            id: WidgetId::new("demo:bar"),
            top_items: Vec::new(),
            bottom_items: Vec::new(),
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: true,
        };
        // No surface attached — every real Direct2D call in
        // `draw_activity_bar` is behind `#[cfg(target_os = "windows")]`
        // and this test isn't, so on a non-Windows host it falls to the
        // `todo!()` stub *after* the focus-tracking below has already
        // run (see that method's doc). Catch the panic so this test
        // proves the tracking half of the contract everywhere, while
        // `win::testing`'s headless-surface driver tests prove the paint
        // half for real on Windows.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.draw_activity_bar(Rect::new(0.0, 0.0, 40.0, 200.0), &bar, None);
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

    /// #702: `PointerShape` -> Win32 `IDC_*` cursor-resource mapping
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
        // Iterates the shared `desktop::all_pointer_shapes()` scaffold
        // (#498, adopted here by #702) instead of hand-listing all 9
        // `Default` + `Resize(edge)` combinations.
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
        ];
        for (shape, expected) in crate::desktop::all_pointer_shapes()
            .iter()
            .zip(expected.iter())
        {
            assert_eq!(pointer_shape_to_win32_cursor(*shape), *expected);
        }
    }

    /// #724 acceptance: `Backend::set_theme` must actually reach the
    /// rasterisers, not just be stored and ignored — a real
    /// `HeadlessSurface` pixel probe, not a source-parse check (that's
    /// `tests/conformance/caps.rs`'s job). Probes
    /// [`super::terminal::draw_terminal_divider`]'s painted pixel: a
    /// single flat `theme.separator` fill with no blending
    /// (`super::scrollbar`/`super::multi_section_view`'s translucency
    /// premixes would make the expected colour only approximate), so a
    /// non-default theme's colour must come through pixel-exact if
    /// `set_theme` is wired at all. Before #724 this rasteriser always
    /// painted `Theme::default().separator` regardless of what was set.
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
        backend.draw_terminal_divider(Rect::new(0.0, 0.0, 1.0, H as f32));
        backend.end_frame();

        let px = surface.pixel_at(0, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_separator.r, custom_separator.g, custom_separator.b),
            "painted pixel must reflect the theme set via `set_theme`, not `Theme::default()`",
        );
    }

    /// quadraui#789 acceptance: `activity_bar`/`menu_bar`/`context_menu`/
    /// `completions`/`find_replace` used to each build their own
    /// `Theme::default()` instead of reading `WinBackend::current_theme`
    /// — a dark-themed app got a light activity bar, menu bar, context
    /// menu, completions popup and find/replace panel regardless of
    /// `set_theme`. Same pixel-probe pattern
    /// [`set_theme_reaches_the_rasterisers`] established for #724: paint
    /// with a theme colour that differs from the default, and demand the
    /// non-default colour show up pixel-exact. Confirmed to fail against
    /// unfixed `develop @ a9104f5` (each rasteriser painted
    /// `Theme::default()`'s colour instead).
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
        };

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.draw_activity_bar(Rect::new(0.0, 0.0, W as f32, H as f32), &bar, None);
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
        backend.draw_menu_bar(Rect::new(0.0, 0.0, W as f32, H as f32), &bar);
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
        use crate::primitives::context_menu::{ContextMenuItemMeasure, ContextMenuPlacement};
        use crate::theme::Theme;
        use crate::types::Color;
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

        // No `bg` override, so the fill falls back to `theme.hover_bg` —
        // and no items, so nothing else paints over the background.
        let menu = ContextMenu {
            id: WidgetId::new("ctx"),
            items: Vec::new(),
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        };
        let viewport = Rect::new(0.0, 0.0, W as f32, H as f32);
        let layout = menu.layout(4.0, 4.0, viewport, 60.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.draw_context_menu(&menu, &layout);
        backend.end_frame();

        let b = layout.bounds;
        assert!(
            b.width > 0.0 && b.height > 0.0,
            "test fixture bug: empty menu bounds"
        );
        let px = surface.pixel_at((b.x + b.width / 2.0) as u32, (b.y + b.height / 2.0) as u32);
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "context menu background must reflect the live theme, not `Theme::default()`",
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn set_theme_reaches_the_completions() {
        use crate::primitives::completions::CompletionItemMeasure;
        use crate::theme::Theme;
        use crate::types::Color;
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

        // No items, so only the background fill + border stroke paint —
        // nothing to obscure the centre-of-popup probe.
        let completions = Completions {
            id: WidgetId::new("comp"),
            items: Vec::new(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, W as f32, H as f32);
        let layout = completions.layout(10.0, 10.0, 16.0, viewport, 120.0, 60.0, |_| {
            CompletionItemMeasure::new(16.0)
        });

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.draw_completions(&completions, &layout);
        backend.end_frame();

        let b = layout.bounds;
        assert!(
            b.width > 0.0 && b.height > 0.0,
            "test fixture bug: empty popup bounds"
        );
        let px = surface.pixel_at((b.x + b.width / 2.0) as u32, (b.y + b.height / 2.0) as u32);
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

        // Mirrors `win::find_replace`'s own
        // `paints_panel_background_and_border` test's geometry — the
        // default `current_line_height`/`current_char_width` (16.0/8.0)
        // match its hardcoded values.
        let popup_w = panel.panel_width as f32 * 8.0;
        let popup_h = 3.0 * 16.0;
        let popup_x = (panel.group_bounds.x + panel.group_bounds.width - popup_w - 10.0)
            .max(panel.group_bounds.x);
        let popup_y = panel.group_bounds.y + 2.0;

        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.draw_find_replace(Rect::new(0.0, 0.0, W as f32, H as f32), &panel);
        backend.end_frame();

        let px = surface.pixel_at(
            (popup_x + popup_w - 4.0) as u32,
            (popup_y + popup_h - 4.0) as u32,
        );
        assert_eq!(
            (px.r, px.g, px.b),
            (custom_bg.r, custom_bg.g, custom_bg.b),
            "find/replace panel background must reflect the live theme, not `Theme::default()`",
        );
    }
}
