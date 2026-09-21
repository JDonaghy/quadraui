//! macOS implementation of [`quadraui::Backend`].
//!
//! `MacBackend` mirrors the shape of [`crate::gtk::backend::GtkBackend`]:
//! it owns the persistent state the trait surface requires (viewport,
//! modal stack, accelerator registry, event queue, theme, current font
//! metrics, platform services) plus a transient frame-scope holding
//! the active `CGContextRef` so trait `draw_*` methods can rasterise
//! inside `drawRect:` without re-querying AppKit.
//!
//! ### Frame-scope mechanism
//!
//! `drawRect:` receives a `CGContextRef` owned by AppKit for the
//! duration of the call. [`MacBackend::enter_frame_scope`] stashes the
//! pointer in a `Cell`, runs the caller's closure, and restores the
//! previous value on exit. Type-erased through `*const ()` so the
//! struct doesn't need a lifetime parameter. Inside the closure,
//! `draw_*` methods recover the pointer from
//! [`MacBackend::current_cg_ptr`] and call CoreGraphics + CoreText FFI.
//!
//! ### Event queue
//!
//! [`crate::macos::run`]'s responder methods translate `NSEvent` into
//! [`UiEvent`] (via [`crate::macos::events`]) and dispatch the result
//! through the app's [`crate::runner::AppLogic`] synchronously —
//! including the accelerator-match / double-click-fold / paste-
//! interception pre-processing `run`'s `handle` closure applies before
//! `AppLogic::handle` sees the event (#486). The queue here exists for
//! parity with [`Backend`] callers that prefer the poll API and for
//! backend-side producers (native menu activations, context-menu
//! results). `WindowResized` (from the `NSViewFrameDidChangeNotification`
//! observer, #486) dispatches synchronously like mouse/keyboard events
//! rather than going through the queue.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use core_graphics::base::CGFloat;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::rc::Retained;
use objc2_app_kit::{
    NSCursor, NSEvent, NSFloatingWindowLevel, NSNormalWindowLevel, NSWindow, NSWindowButton,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use crate::accelerator::{key_to_binding_name, parse_binding};
use crate::backend::{
    Backend, BackendError, EditorPaintResult, PointerShape, ResizeEdge, ServiceResult,
    WindowControl,
};
use crate::desktop::WindowDragArm;
use crate::dispatch::{DoubleClickDetector, DragState, TextRegion};
use crate::event::{Point, Rect, UiEvent, UserPayload, Viewport};
use crate::generic_font::GenericFamily;
use crate::modal_stack::ModalStack;
use crate::native_surface::NativeSurface;
use crate::primitives::activity_bar::ActivityBarRowHit;
use crate::primitives::board::{BoardLayout, BoardModel};
use crate::primitives::chart::{Chart, ChartLayout};
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout};
use crate::primitives::command_line::CommandLine;
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::primitives::data_table::{DataTable, DataTableLayout};
use crate::primitives::dialog::{Dialog, DialogLayout};
use crate::primitives::editor::Editor;
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::form::FormLayout;
use crate::primitives::menu_bar::{MenuBar, MenuBarLayout};
use crate::primitives::message_list::MessageList;
use crate::primitives::multi_section_view::{
    MsvLayoutMetrics, MultiSectionView, MultiSectionViewLayout,
};
use crate::primitives::panel::{Panel, PanelLayout};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::spinner::{Spinner, SpinnerLayout};
use crate::primitives::split::{Split, SplitLayout};
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout};
use crate::primitives::status_bar::StatusBarLayout;
// `TabBarHits` is `#[deprecated]` (issue #823) — this backend still
// constructs it directly (per #504's audit: `macos::tab_bar` has no
// intermediate `TabBarLayout` to source native coordinates from), so the
// import needs the same allow every use site below does.
#[allow(deprecated)]
use crate::primitives::tab_bar::TabBarHits;
use crate::primitives::text_display::TextDisplayLayout;
use crate::primitives::toast::{ToastStack, ToastStackLayout};
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::primitives::tree::TreeViewLayout;
use crate::testing::{TextRun, ZoneRec};
use crate::types::{Color, WidgetId};
use crate::KeyBinding;
use crate::{
    Accelerator, AcceleratorId, AcceleratorScope, ActivityBar, FieldKind, Form, Key, ListView,
    Modifiers, Palette, ParsedBinding, PlatformServices, StatusBar, TabBar, TabBarLayout, Terminal,
    TextDisplay, Theme, TreeView,
};

use super::services::MacPlatformServices;

/// macOS backend implementing [`Backend`].
///
/// Field roles (mirroring [`crate::gtk::backend::GtkBackend`]):
/// - `viewport` — width × height in points, scale = `backingScaleFactor`.
///   Updated each frame from the active `QuadraView`'s bounds.
/// - `modal_stack` — pushed by hosts on modal open, popped on close.
/// - `accelerators` / `parsed_accelerators` — registered keybindings and
///   their parsed form; [`Self::match_keypress`] resolves a native
///   keypress against them (#486). [`run`][super::run]'s `handle`
///   closure rewrites a matching `KeyPressed` into `Accelerator` before
///   `AppLogic::handle` sees it.
/// - `double_click` — folds a `MouseDown` into `DoubleClick` (#486); see
///   [`Self::fold_double_click`].
/// - `events` — adapter queue. [`run`][super::run]'s responder methods
///   dispatch synchronously today; the queue is used for backend-side
///   producers (native menu activations, context-menu results).
/// - `current_cg_ptr` — frame-scope pointer; non-null only inside
///   [`Self::enter_frame_scope`].
/// - `current_font` / `current_line_height` / `current_char_width` —
///   per-app *editor* font state. Apps set these once in `setup()` via
///   [`Self::set_current_font`].
/// - `chrome_font` / `chrome_line_height` / `chrome_char_width` — the
///   independent *chrome* (UI) font state (issue #963), defaulting to the
///   CoreText system UI font. Set via [`Self::set_chrome_font`]
///   (`Backend::set_ui_font`'s inherent twin).
pub struct MacBackend {
    viewport: Viewport,
    /// `Rc<RefCell<>>` (not a plain field) so [`Backend::modal_stack_handle`]
    /// can hand back a handle that outlives any single `&mut self`
    /// borrow — the shape `GtkBackend` and `TuiBackend` already use for
    /// this (quadraui#699).
    modal_stack: Rc<RefCell<ModalStack>>,
    /// See `modal_stack`'s doc comment — same rationale.
    drag_state: Rc<RefCell<DragState>>,
    accelerators: HashMap<AcceleratorId, Accelerator>,
    /// Parsed form of `accelerators`, kept in sync by
    /// `register_accelerator` / `unregister_accelerator`. Mirrors
    /// `TuiBackend::parsed_accelerators` / `GtkBackend::parsed_accelerators`
    /// — a `Vec` rather than a map because match order matters (first
    /// registered wins on an accidental duplicate binding).
    parsed_accelerators: Vec<(ParsedBinding, AcceleratorId)>,
    /// Folds a `MouseDown` `NSEvent` into `DoubleClick` when it lands
    /// within the time/position window of the previous click (#486).
    /// `macos::run` dispatches synchronously per `NSEvent`, so this
    /// runs on one event at a time via `Self::fold_double_click`
    /// rather than `TuiBackend`'s per-poll batch.
    double_click: DoubleClickDetector,
    events: Rc<std::cell::RefCell<VecDeque<UiEvent>>>,
    services: MacPlatformServices,
    /// Type-erased `CGContextRef`; non-null only inside
    /// [`Self::enter_frame_scope`]. Stored as `*const ()` so the
    /// struct doesn't need a lifetime parameter.
    current_cg_ptr: Cell<*const ()>,
    current_theme: Theme,
    /// Decoded-`CGImage` cache for [`Backend::draw_image`] (issue #1014)
    /// — see [`crate::image_cache`]'s module doc and
    /// [`super::image::draw_image`]'s "Decode cache" section for why
    /// this backend keys on source only (a `CGImage` decode is reused at
    /// every paint size, unlike GTK's already-scaled `Pixbuf`). Survives
    /// across frames, mirroring `GtkBackend::image_cache`.
    image_cache: crate::image_cache::ImageCache<CGImage>,
    /// Set once via [`Self::set_current_font`] during app setup.
    /// `draw_*` methods recover this for text rendering +
    /// measurement. Wrapped in `Option` so apps that don't paint
    /// text can skip the setup call.
    current_font: Option<CTFont>,
    current_line_height: f64,
    current_char_width: f64,
    /// Chrome (UI) font — issue #963's fix for macOS being the only
    /// pixel backend where `set_ui_font` was a silent no-op and every
    /// piece of chrome (status bar today; more rasterisers follow-up)
    /// painted in `current_font`, the *editor* font, instead. Unlike
    /// `current_font`, this is never `None`: [`Self::new`] seeds it with
    /// the CoreText system UI font so chrome already looks right before
    /// any [`Backend::set_ui_font`] call, mirroring GTK's `"Sans 11"` /
    /// Win-GUI's `Segoe UI` un-set defaults rather than macOS's own
    /// editor font falling back to "no font at all". Set via
    /// [`Self::set_chrome_font`]; never touched by
    /// [`Self::set_current_font`]/[`Backend::set_editor_font`] — see
    /// #912 for the metric-mismatch hazard that separation exists to
    /// avoid.
    chrome_font: CTFont,
    /// Cached [`super::text::font_metrics`] line height for
    /// `chrome_font`, in points — the chrome twin of
    /// `current_line_height`. Recomputed by [`Self::set_chrome_font`]
    /// whenever `chrome_font` changes.
    chrome_line_height: f64,
    /// Cached `char_width` for `chrome_font` — the chrome twin of
    /// `current_char_width`.
    chrome_char_width: f64,
    /// Family last set via [`Backend::set_nerd_font_fallback`] (issue
    /// #929), if any. Applied to `current_font` immediately if one is
    /// already set (see [`Self::set_nerd_font_fallback`]), and consulted
    /// by [`Self::set_current_font`] so a later `set_current_font` call
    /// (e.g. a runtime font-preference change) doesn't silently drop a
    /// fallback that was configured first — the two setters can land in
    /// either order.
    nerd_font_fallback_family: Option<String>,
    /// Retained installer target from the last [`Backend::install_menu_bar`]
    /// call. Holds it alive so action selectors on installed `NSMenuItem`s
    /// don't dangle. Replaced wholesale on each re-install.
    menu_target: Option<objc2::rc::Retained<super::menu_bar_install::QuadraMenuTarget>>,
    /// Tray/status-bar icon state (issue #953) — see [`super::tray`]'s
    /// module doc and `impl TrayService for MacBackend` below.
    tray: super::tray::MacTrayState,
    /// Whether `InlineInput` carets should currently paint their stroke
    /// (the "on" half of the blink cycle). Shared `Rc<Cell>` so the
    /// macOS run-loop blink timer can toggle it without holding a
    /// `MacBackend` reference. Defaults to `true` so headless tests
    /// (and the first frame after startup) paint a visible caret
    /// without any timer running.
    caret_visible: std::rc::Rc<std::cell::Cell<bool>>,
    /// Until this instant, the blink timer's tick callback skips
    /// toggling — used to keep the caret solid while the user types.
    /// Reset on every `KeyPressed` event in `macos::run`.
    caret_blink_pause_until: std::rc::Rc<std::cell::Cell<std::time::Instant>>,
    /// Widget zones registered during the current frame via
    /// [`Backend::register_zone`]. Cleared at the start of each frame by
    /// [`Backend::begin_frame`]. Mirrors `GtkBackend::zones` /
    /// `TuiBackend::zones`. Read by
    /// [`crate::testing::FrameInventory::zones`] via
    /// [`super::testing::MacDriver::inventory`] (quadraui#493).
    zones: Vec<ZoneRec>,
    /// Whether [`Self::enter_frame_scope`] should wrap its closure in
    /// [`super::text::start_recording_text`] / `stop_recording_text` and
    /// stash the result into `text_runs`. Off by default — the live
    /// runner never reads it — [`super::testing::MacDriver::new`] turns
    /// it on. Mirrors `GtkBackend::painted_text_recording`.
    painted_text_recording: bool,
    /// Every [`super::text::draw_text`] call recorded during the last
    /// [`Self::enter_frame_scope`] with `painted_text_recording` on —
    /// see [`Self::set_painted_text_recording`].
    text_runs: Vec<TextRun>,
    /// `WidgetId` of the [`ActivityBar`] that painted itself with
    /// `is_keyboard_focused = true` this frame, if any. Set by
    /// [`Backend::draw_activity_bar`], cleared by [`Backend::begin_frame`]
    /// (same per-frame lifecycle as `zones`), read by
    /// [`super::run::dispatch_event`] to redirect the next `KeyPressed`
    /// into the bar. Mirrors `GtkBackend::focused_activity_bar` (#465).
    focused_activity_bar: Option<WidgetId>,
    /// Top-level window handle, set once by `macos::run::run` via
    /// [`Self::set_window`]. `None` until the runner finishes
    /// constructing the window (and in unit tests, which never call
    /// it). Backs [`Backend::begin_window_drag`] /
    /// [`Backend::toggle_window_maximize`] / [`Backend::set_cursor`]
    /// (#498) — mirrors `GtkBackend::window`.
    window: Option<Retained<NSWindow>>,
    /// Raw press `NSEvent`, stashed by `macos::run`'s `mouseDown:`
    /// responder before the press is translated to a portable
    /// `UiEvent`. [`Backend::begin_window_drag`] consumes this (via
    /// `.take()`) — `performWindowDragWithEvent:` requires the
    /// *originating* mouse-down event, not a synthesized one.
    ///
    /// Stored in the shared [`crate::desktop::WindowDragArm`] (#498)
    /// purely as an arm/take stash: unlike `GtkBackend`'s
    /// `armed_window_drag`, there's no threshold-gated commit step
    /// here, because AppKit's `performWindowDragWithEvent:` already
    /// disambiguates a drag from a double-click internally (see that
    /// type's doc comment) — so the `origin_x`/`origin_y` `arm()`
    /// arguments are always `0.0` and go unread.
    pending_window_press: WindowDragArm<Retained<NSEvent>>,
    /// Mirrors `TuiBackend::nerd_fonts_enabled` / `GtkBackend::nerd_fonts_enabled`
    /// (issue #683). Picks `Icon::glyph` vs `Icon::fallback` in
    /// `draw_activity_bar` and (since #804) `draw_tree`. Set via
    /// [`Backend::set_nerd_fonts`]; defaults to `false` — see that
    /// method's doc for why every backend now agrees on this default.
    nerd_fonts_enabled: bool,
    /// Region registry + active-selection state shared by every
    /// `text_selection: true` backend (#741, adopted here in #803) — see
    /// [`crate::text_selection::TextSelectionState`]'s doc. Mirrors
    /// `GtkBackend`/`WinBackend`'s identically-named field; only the
    /// paint call ([`Self::apply_selection_highlight`], via
    /// [`super::text_selection::draw_selection_highlight`]) and text
    /// extraction ([`Self::extract_selection_text`]) are backend-owned.
    text_selection: crate::text_selection::TextSelectionState,
    /// Single owner of keyboard focus (issue #830) — see
    /// [`crate::focus`]'s module doc. Mutated only by the shared
    /// Tab/Shift+Tab intercept in [`crate::runtime::preprocess_event`]
    /// via [`crate::runtime::PreprocessBackend::focus_manager_mut`];
    /// read elsewhere via [`Backend::focus_manager`].
    focus: crate::focus::FocusManager,
    /// Thread-safe inbox for [`crate::UiEvent::User`] payloads (issue
    /// #831) — see [`crate::runtime::UserEventQueue`]'s doc. [`Self::waker`]
    /// clones this `Arc` into the closure it hands out; [`Self::poll_events`]
    /// drains it into `UiEvent::User` alongside `events`, on the AppKit
    /// main thread, same as any other queued event.
    user_events: Arc<crate::runtime::UserEventQueue>,
    /// Set once by `macos::run::run` via [`Self::set_wake_callback`],
    /// right after the `QuadraView` exists — see that method's doc for
    /// what it stores and why `waker()` needs a handle to it rather than
    /// touching AppKit directly. `None` until then (e.g. a `MacBackend`
    /// constructed directly by a test, never handed to `macos::run`);
    /// `waker()`'s returned closure is a documented no-op in that case.
    wake_callback: WakeCallback,
    /// Set once by `macos::run::run` via [`Self::set_tick_callback`]
    /// (quadraui#832), right after the `QuadraView` exists — same shape
    /// as [`Self::wake_callback`], but wired to `view.dispatch_tick()`
    /// (fires `AppLogic::tick`) instead of a bare repaint request.
    /// [`Self::request_frame_in`]'s `DispatchQueue::main().after` closure
    /// looks this up when the scheduled delay elapses. `None` until then,
    /// same "no-op, not a panic" posture as `wake_callback`.
    tick_callback: WakeCallback,
}

/// The wake-target [`MacBackend::waker`] invokes (issue #831) — see
/// [`MacBackend::set_wake_callback`]'s doc for what installs it and why
/// it's shaped this way. Named to keep `MacBackend`'s field declaration
/// (and `clippy::type_complexity`) readable — mirrors
/// `gtk::backend`'s wake plumbing in intent only, not in shape: this one
/// wraps the `!Send` `Rc` in `dispatch2::MainThreadBound` (upstream's
/// audited wrapper — it carries a `MainThreadMarker` proof and
/// re-dispatches its own `Drop` back to the main thread), whereas GTK
/// never lets the `Rc` cross a thread boundary at all, parking it in a
/// thread-local keyed by an integer id.
type WakeCallback = Arc<std::sync::OnceLock<MainThreadBound<Rc<dyn Fn()>>>>;

/// Position tolerance, in points, for [`MacBackend::fold_double_click`]'s
/// [`DoubleClickDetector`].
///
/// `ns_mouse_down` (`macos/events.rs`) passes raw `NSEvent` `x`/`y`
/// straight through — unlike TUI's whole character cells, these are
/// point-precision, so the detector's default radius
/// (`crate::dispatch::DOUBLE_CLICK_RADIUS`, 1.5 — tuned for TUI's
/// integral cell grid) is far tighter than two real mouse/trackpad
/// clicks can reliably land within. `4.0` points is a rough approximation
/// of AppKit's own double-click hit region; it's a heuristic, not a
/// measured constant, since there's no public API exposing the system's
/// actual tolerance the way `NSEvent.clickCount` would sidestep this
/// entirely (see the #486 review's non-blocking note — reading
/// `clickCount` natively remains the more robust fix and should be
/// revisited, ideally verified on real hardware).
const MAC_DOUBLE_CLICK_RADIUS: f32 = 4.0;

/// Default chrome (UI) font size in points, paired with the CoreText
/// system UI font [`MacBackend::new`] installs as `chrome_font` before
/// any [`Backend::set_ui_font`] call — matches GTK's `"Sans 11"` /
/// Win-GUI's `DEFAULT_UI_FONT_SIZE_PT` default (issue #963).
const DEFAULT_UI_FONT_SIZE_PT: f64 = 11.0;

/// What [`parse_ui_font_desc`] resolved a font description's family
/// portion to.
enum ParsedFamily {
    /// A CSS/Pango generic token (issue #1023) — always resolvable, since
    /// it names a native system role rather than an installed family; see
    /// [`GenericFamily`].
    Generic(GenericFamily),
    /// A concrete family name that [`super::text::make_font_exact`]
    /// confirmed is actually installed, or — if nothing in `desc`'s
    /// comma-separated family list resolved — the first candidate,
    /// unchecked, for the caller to make its own final degrade decision
    /// against (matching this function's pre-#1023 contract for a single
    /// unresolvable family).
    Named(String),
}

/// Parse a Pango-style font description (`Backend::set_ui_font`'s
/// documented shape, e.g. `"Sans 13"`) into a Core-Text-ready
/// `(family, size_pt)` pair — the same convention
/// `win::backend::parse_ui_font_desc` implements for the identical trait
/// contract (issue #963 ports Win-GUI's #724 shape onto macOS). A
/// trailing whitespace-separated numeric token is the point size,
/// defaulting to [`DEFAULT_UI_FONT_SIZE_PT`] when `desc` carries none —
/// a bare comma-separated fallback list (`"SF Pro Text, Helvetica Neue,
/// Sans"`, no size at all) is a real shape `Backend::set_ui_font`
/// callers use, not a parse failure.
///
/// `desc`'s family portion is a Pango-style comma-separated fallback
/// list (issue #1023): everything before the size is split on `,`, each
/// candidate trimmed, and tried in order — a CSS/Pango generic token
/// ([`GenericFamily::parse`]) always resolves; a concrete name resolves
/// if [`super::text::make_font_exact`] confirms Core Text has it
/// installed. The first candidate that resolves either way wins, the
/// same semantics Pango/fontconfig give a comma family list. Before this
/// fix, `rfind(' ')` alone treated the whole list as one (unmatchable)
/// literal family name.
///
/// Because trying each candidate is now this function's job (it is the
/// only place that has the whole ordered list to retry against), it
/// calls [`super::text::make_font_exact`] itself rather than leaving
/// every installed-family check to the caller the way the pre-#1023
/// version did. The caller ([`MacBackend::set_ui_font`]) still owns the
/// *final* degrade decision — what to do when not one candidate in the
/// list resolved — via [`ParsedFamily::Named`]'s unchecked-fallback
/// case.
///
/// Returns `None` only when `desc` is empty/whitespace-only, or its
/// family portion is empty (e.g. `desc` is just a bare size).
fn parse_ui_font_desc(desc: &str) -> Option<(ParsedFamily, f64)> {
    let desc = desc.trim();
    if desc.is_empty() {
        return None;
    }
    let (family_part, size_pt) = match desc.rfind(' ') {
        Some(idx) if desc[idx + 1..].trim().parse::<f64>().is_ok() => {
            let (family, size_str) = desc.split_at(idx);
            (
                family.trim(),
                size_str
                    .trim()
                    .parse::<f64>()
                    .expect("just matched by the guard above"),
            )
        }
        _ => (desc, DEFAULT_UI_FONT_SIZE_PT),
    };
    if family_part.is_empty() {
        return None;
    }

    for candidate in family_part.split(',') {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        if let Some(generic) = GenericFamily::parse(candidate) {
            return Some((ParsedFamily::Generic(generic), size_pt));
        }
        if super::text::make_font_exact(candidate, size_pt).is_some() {
            return Some((ParsedFamily::Named(candidate.to_string()), size_pt));
        }
    }

    // Nothing in the list resolved — hand the caller the first candidate
    // unchecked, same as this function always did for a single
    // unresolvable family, so `set_ui_font` still makes the final
    // "degrade to the system font" call itself.
    let first = family_part
        .split(',')
        .map(str::trim)
        .find(|c| !c.is_empty())?;
    Some((ParsedFamily::Named(first.to_string()), size_pt))
}

/// Translate a parsed universal [`KeyBinding`] to macOS's native Cmd
/// idiom.
///
/// [`crate::accelerator::parse_binding`] is shared with `TuiBackend`, so
/// its universal arms (`KeyBinding::Save`, `Copy`, `Paste`, …) resolve to
/// a **Ctrl**-modifier `ParsedBinding` regardless of platform — correct
/// for TUI, wrong for macOS. `menu_bar_install::accelerator_to_ns` (the
/// native menu path) and `crate::accelerator::render_binding` (the
/// display/tooltip path) both already render these as **Cmd** on macOS,
/// so a `MacBackend::match_keypress` that compared the raw Ctrl
/// `ParsedBinding` against a real Cmd keypress would never fire (#486
/// review). Swap Ctrl for Cmd here so registration, native-menu
/// resolution, and rendering all agree.
///
/// `KeyBinding::Literal` bindings are left untouched — the app author
/// already chose the exact modifier they want (e.g. `<C-s>` for a
/// deliberate Ctrl+S that coexists with the native Cmd+S), and
/// `accelerator_to_ns`/`render_binding` don't rewrite literals either.
fn macos_universal_binding_modifiers(
    binding: &KeyBinding,
    mut parsed: ParsedBinding,
) -> ParsedBinding {
    if !matches!(binding, KeyBinding::Literal(_)) && parsed.modifiers.ctrl {
        parsed.modifiers.ctrl = false;
        parsed.modifiers.cmd = true;
    }
    parsed
}

/// Which of AppKit's three usable public cursor singletons a
/// [`PointerShape`] resolves to (#498).
///
/// This exists so the *mapping decision* — the part with an opinion in
/// it, and the part a regression can silently change — is a plain Rust
/// value that can be asserted anywhere, on any thread, on any OS.
/// Turning a variant into the actual `Retained<NSCursor>` is a separate,
/// trivial step ([`mac_cursor_for_shape`]) that has to call AppKit and
/// therefore can only be exercised on a real macOS main thread.
///
/// Splitting the two is not cosmetic: `cargo test`'s libtest harness
/// runs each `#[test]` on a spawned worker thread, and AppKit object
/// vending off the main thread (in a process that never created an
/// `NSApplication`) is exactly the class of call `macos::menu_bar_install`'s
/// tests already gate behind `MainThreadMarker::new()`. A mapping test
/// written directly against `NSCursor` singletons inherits that hazard
/// while proving nothing AppKit-specific — the singleton identity is
/// Apple's invariant, not ours. Ours is *which* singleton each shape
/// picks, and that is what [`mac_cursor_kind`] makes testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacCursorKind {
    /// `NSCursor::arrowCursor` — the plain pointer.
    Arrow,
    /// `NSCursor::resizeUpDownCursor` — vertical resize glyph.
    ResizeUpDown,
    /// `NSCursor::resizeLeftRightCursor` — horizontal resize glyph.
    ResizeLeftRight,
}

/// Map a [`PointerShape`] to the [`MacCursorKind`] it should show (#498).
/// Mirrors `gtk::backend::pointer_shape_cursor_name`, but `NSCursor`'s
/// public API (`objc2-app-kit`'s binding of it — see
/// `MacBackend::set_cursor`) only exposes horizontal
/// (`resizeLeftRightCursor`) and vertical (`resizeUpDownCursor`) resize
/// glyphs, unlike GTK's CSS cursor-name space which has all 8 directional
/// keywords. There is no public diagonal-resize `NSCursor` to fall back to
/// (`_windowResize*Cursor` selectors are private API), so the 4 corner
/// edges fall back to the plain arrow — honest about the gap (see
/// `crate::desktop`'s pointer-shape-scaffold doc) rather than pointing a
/// horizontal or vertical glyph in a direction the user isn't actually
/// dragging.
///
/// Pure and AppKit-free on purpose — see [`MacCursorKind`].
fn mac_cursor_kind(shape: PointerShape) -> MacCursorKind {
    match shape {
        PointerShape::Default => MacCursorKind::Arrow,
        PointerShape::Resize(ResizeEdge::North | ResizeEdge::South) => MacCursorKind::ResizeUpDown,
        PointerShape::Resize(ResizeEdge::East | ResizeEdge::West) => MacCursorKind::ResizeLeftRight,
        PointerShape::Resize(
            ResizeEdge::NorthEast
            | ResizeEdge::NorthWest
            | ResizeEdge::SouthEast
            | ResizeEdge::SouthWest,
        ) => MacCursorKind::Arrow,
    }
}

/// Vend the shared `NSCursor` singleton for a [`PointerShape`], via
/// [`mac_cursor_kind`] (#498). The only AppKit-touching half of the
/// mapping; see [`MacCursorKind`] for why the two halves are separate.
fn mac_cursor_for_shape(shape: PointerShape) -> Retained<NSCursor> {
    // `resizeUpDownCursor`/`resizeLeftRightCursor` were deprecated in the
    // AppKit SDK objc2-app-kit 0.3 (#796 bump) now ships bindings for, in
    // favour of `rowResizeCursorInDirections:`/`columnResizeCursorInDirections:`
    // (divider-drag) or `frameResizeCursorFromPosition:inDirections:`
    // (rectangular-frame resize) — neither is a drop-in replacement (both
    // take a directions argument this call site has no opinion on), so
    // this keeps the simple singleton for now rather than guessing at the
    // right direction set. The legacy cursors still work on every macOS
    // version quadraui supports.
    #[allow(deprecated)]
    match mac_cursor_kind(shape) {
        MacCursorKind::Arrow => NSCursor::arrowCursor(),
        MacCursorKind::ResizeUpDown => NSCursor::resizeUpDownCursor(),
        MacCursorKind::ResizeLeftRight => NSCursor::resizeLeftRightCursor(),
    }
}

impl MacBackend {
    /// Construct a fresh `MacBackend` with a default viewport, empty
    /// event queue, default theme, and no font. The runner overwrites
    /// the viewport each frame via [`Backend::begin_frame`]; apps
    /// install a font via [`Self::set_current_font`] in `setup()`.
    pub fn new() -> Self {
        // Seed chrome with the CoreText system UI font (issue #963) so
        // chrome paints correctly before any `set_ui_font` call — see
        // `chrome_font`'s field doc.
        let chrome_font = super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT);
        let chrome_metrics = super::text::font_metrics(&chrome_font);
        Self {
            viewport: Viewport::new(0.0, 0.0, 1.0),
            modal_stack: Rc::new(RefCell::new(ModalStack::new())),
            drag_state: Rc::new(RefCell::new(DragState::new())),
            accelerators: HashMap::new(),
            parsed_accelerators: Vec::new(),
            double_click: DoubleClickDetector::with_radius(MAC_DOUBLE_CLICK_RADIUS),
            events: Rc::new(std::cell::RefCell::new(VecDeque::new())),
            services: MacPlatformServices::new(),
            current_cg_ptr: Cell::new(std::ptr::null()),
            current_theme: Theme::default(),
            image_cache: crate::image_cache::ImageCache::default(),
            current_font: None,
            current_line_height: 16.0,
            current_char_width: 8.0,
            chrome_font,
            chrome_line_height: chrome_metrics.line_height,
            chrome_char_width: chrome_metrics.char_width,
            nerd_font_fallback_family: None,
            menu_target: None,
            tray: super::tray::MacTrayState::default(),
            caret_visible: std::rc::Rc::new(std::cell::Cell::new(true)),
            caret_blink_pause_until: std::rc::Rc::new(std::cell::Cell::new(
                std::time::Instant::now(),
            )),
            zones: Vec::new(),
            painted_text_recording: false,
            text_runs: Vec::new(),
            focused_activity_bar: None,
            window: None,
            pending_window_press: WindowDragArm::new(),
            nerd_fonts_enabled: false,
            text_selection: crate::text_selection::TextSelectionState::default(),
            focus: crate::focus::FocusManager::new(),
            user_events: crate::runtime::UserEventQueue::new(),
            wake_callback: Arc::new(std::sync::OnceLock::new()),
            tick_callback: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Store the top-level window handle. Called once by
    /// `macos::run::run` right after the window is constructed. Backs
    /// [`Backend::begin_window_drag`] / [`Backend::toggle_window_maximize`]
    /// / [`Backend::set_cursor`] (#498) — mirrors `GtkBackend::set_window`.
    pub(crate) fn set_window(&mut self, window: Retained<NSWindow>) {
        self.window = Some(window);
    }

    /// Install the closure [`Backend::waker`]'s returned handle invokes on
    /// the AppKit main thread (issue #831). `macos::run::run` calls this
    /// once, right after constructing the `QuadraView`, with a closure
    /// that calls `view.setNeedsDisplay(true)` — the same
    /// `ReactionSink::request_redraw` path `EventOutcome::Redraw` already
    /// uses for every other event, so a background-thread wake schedules a
    /// repaint through one call site rather than a second bespoke one.
    /// `mtm` proves the call happens on the main thread, matching
    /// [`dispatch2::MainThreadBound::new`]'s contract.
    ///
    /// A second call is a no-op ([`std::sync::OnceLock::set`] silently
    /// ignores it) — `run` only ever calls this once per backend instance.
    pub(crate) fn set_wake_callback(&self, callback: Rc<dyn Fn()>, mtm: MainThreadMarker) {
        let _ = self.wake_callback.set(MainThreadBound::new(callback, mtm));
    }

    /// Install the closure [`Self::request_frame_in`]'s scheduled-wake
    /// timer invokes once its delay elapses (quadraui#832).
    /// `macos::run::run` calls this once, right after constructing the
    /// `QuadraView`, with a closure that calls `view.dispatch_tick()` —
    /// firing `AppLogic::tick` and applying its `Reaction`, same as
    /// [`Self::set_wake_callback`]'s closure does for `request_redraw`.
    ///
    /// A second call is a no-op ([`std::sync::OnceLock::set`] silently
    /// ignores it) — `run` only ever calls this once per backend instance.
    pub(crate) fn set_tick_callback(&self, callback: Rc<dyn Fn()>, mtm: MainThreadMarker) {
        let _ = self.tick_callback.set(MainThreadBound::new(callback, mtm));
    }

    /// Stash the raw press `NSEvent`. Called by `macos::run`'s
    /// `mouseDown:` responder before the press is translated to a
    /// portable `UiEvent`, so [`Backend::begin_window_drag`] has the
    /// originating event `performWindowDragWithEvent:` requires.
    /// Overwritten by the next press; harmless if never consumed
    /// (mirrors `GtkBackend::stash_window_press`).
    pub(crate) fn stash_window_press(&mut self, event: Retained<NSEvent>) {
        self.pending_window_press.arm(event, 0.0, 0.0);
    }

    /// Shared `Rc<Cell<bool>>` controlling whether `InlineInput` carets
    /// paint their stroke each frame. The run-loop blink timer clones
    /// this and toggles the cell to drive the blink animation; tests
    /// can pin a deterministic phase via [`Self::set_caret_visible`].
    pub fn caret_visible_handle(&self) -> std::rc::Rc<std::cell::Cell<bool>> {
        self.caret_visible.clone()
    }

    /// Shared `Rc<Cell<Instant>>` the blink timer reads to decide
    /// whether to skip its toggle this tick. `macos::run` resets it to
    /// `now + 500ms` on every `KeyPressed` so the caret stays solid
    /// while the user types.
    pub fn caret_blink_pause_handle(&self) -> std::rc::Rc<std::cell::Cell<std::time::Instant>> {
        self.caret_blink_pause_until.clone()
    }

    /// Override the caret-blink phase. Tests pin this to get
    /// reproducible paint snapshots; live apps let the blink timer
    /// drive it instead.
    pub fn set_caret_visible(&mut self, visible: bool) {
        self.caret_visible.set(visible);
    }

    /// Current blink phase. Read once per paint; the
    /// `multi_section_view` rasteriser skips the caret `fill_rect`
    /// when this is `false`.
    pub fn caret_visible(&self) -> bool {
        self.caret_visible.get()
    }

    /// Install the font that subsequent `draw_*` calls use for text.
    /// Updates `current_line_height` + `current_char_width` from the
    /// font's typographic metrics.
    ///
    /// If [`Backend::set_nerd_font_fallback`] was already called, `font`
    /// gets that fallback family applied (via
    /// [`super::text::font_with_fallback`]) before it's stored — issue
    /// #929, and the reason `set_nerd_font_fallback`/`set_current_font`
    /// can land in either order: whichever runs second re-applies the
    /// other's effect instead of silently dropping it.
    pub fn set_current_font(&mut self, font: CTFont) {
        let font = match &self.nerd_font_fallback_family {
            Some(family) => super::text::font_with_fallback(&font, family),
            None => font,
        };
        let metrics = super::text::font_metrics(&font);
        self.current_line_height = metrics.line_height;
        self.current_char_width = metrics.char_width;
        self.current_font = Some(font);
    }

    /// Install the chrome (UI) font and refresh `chrome_line_height` /
    /// `chrome_char_width` from its metrics — the chrome twin of
    /// [`Self::set_current_font`] (issue #963).
    ///
    /// Issue #1003: unlike the #963-era doc this replaces, `chrome_font`
    /// now *does* consult [`Self::nerd_font_fallback_family`] — the same
    /// `font_with_fallback` cascade [`Self::set_current_font`] applies.
    /// Bringing `draw_tree`/`draw_tab_bar_icons`/`draw_activity_bar_with_style`/
    /// `draw_toolbar_interactive`/`draw_sidebar_panel_interactive` to
    /// chrome-font parity with GTK (#1003) means their icon glyphs now
    /// paint through `chrome_font`, not `current_font` — without this,
    /// a host that calls `set_nerd_font_fallback` would see icon glyphs
    /// render correctly in the editor but as tofu in every chrome
    /// primitive, since `chrome_font` would never have received the
    /// fallback cascade. Mirrors `GtkBackend::chrome_font_description`,
    /// which has applied `with_nerd_font_fallback` at every #624 call
    /// site since #929.
    pub fn set_chrome_font(&mut self, font: CTFont) {
        let font = match &self.nerd_font_fallback_family {
            Some(family) => super::text::font_with_fallback(&font, family),
            None => font,
        };
        let metrics = super::text::font_metrics(&font);
        self.chrome_line_height = metrics.line_height;
        self.chrome_char_width = metrics.char_width;
        self.chrome_font = font;
    }

    /// Override the cached line height (in points) that every `draw_*`
    /// method reads via `self.current_line_height` — mirrors
    /// `GtkBackend::set_current_line_height` / `WinBackend::set_current_line_height`.
    ///
    /// Normally this backend derives `current_line_height` itself, from
    /// the real `CTFont` metrics, inside [`Self::set_current_font`] —
    /// unlike `GtkBackend`, which is *told* its metrics because Pango
    /// layout only happens host-side. That asymmetry was flagged (issue
    /// #934) as a real hazard: a host that computes its own chrome layout
    /// generically across backends — sizing a header row, a toolbar strip,
    /// or the first painted line's offset from a *portable* line-height
    /// value — has no way to align that value with what `MacBackend`
    /// actually paints with, because `GtkBackend`/`WinBackend` accept the
    /// override and `MacBackend` silently ignored it (there was no method
    /// to call). This setter closes that gap: a host that measures its own
    /// notion of line height (or wants pixel-for-pixel parity with a
    /// GTK/Win sibling build) can push it here, same call shape as the
    /// other two backends, instead of being stuck with whatever
    /// `set_current_font` derived.
    ///
    /// Does not touch `current_font` itself — glyph baseline positioning
    /// inside [`super::text::draw_text`] still uses `font.ascent()`
    /// directly, independent of this cached row-spacing value, exactly as
    /// it did before this method existed.
    pub fn set_current_line_height(&mut self, line_height: f64) {
        self.current_line_height = line_height;
    }

    /// Override the cached character width (in points) that every
    /// `draw_*` method reads via `self.current_char_width` — mirrors
    /// [`Self::set_current_line_height`]'s rationale and
    /// `GtkBackend::set_current_char_width` / `WinBackend::set_current_char_width`.
    pub fn set_current_char_width(&mut self, char_width: f64) {
        self.current_char_width = char_width;
    }

    /// Override the current theme. The default ([`Theme::default()`])
    /// is installed at construction; apps that use a non-default
    /// theme call this from `setup()` or each frame.
    pub fn set_current_theme(&mut self, theme: Theme) {
        self.current_theme = theme;
    }

    /// The current theme. `draw_*` methods (landing in later tickets)
    /// read this for per-primitive colour resolution.
    pub fn current_theme(&self) -> &Theme {
        &self.current_theme
    }

    /// Shared handle to the backend's event queue. The runner clones
    /// this into responder-method closures (when async producers land
    /// alongside #36 notifications).
    pub fn events_handle(&self) -> Rc<std::cell::RefCell<VecDeque<UiEvent>>> {
        self.events.clone()
    }

    /// Push an event onto the queue, drained by [`Backend::poll_events`].
    pub fn push_event(&self, ev: UiEvent) {
        self.events.borrow_mut().push_back(ev);
    }

    /// Run `f` with the current `CGContextRef` stashed on `self` so
    /// trait `draw_*` methods can recover it. The previous pointer
    /// (typically null) is restored on exit, matching the GTK
    /// `enter_frame_scope` contract.
    pub fn enter_frame_scope<R>(&mut self, ctx: CGContextRef, f: impl FnOnce(&mut Self) -> R) -> R {
        let prev = self.current_cg_ptr.replace(ctx as *const ());
        if self.painted_text_recording {
            super::text::start_recording_text();
        }
        let result = f(self);
        if self.painted_text_recording {
            self.text_runs = super::text::stop_recording_text();
        }
        self.current_cg_ptr.set(prev);
        result
    }

    /// Toggle whether [`Self::enter_frame_scope`] records every
    /// [`super::text::draw_text`] call into `text_runs`. Off by default;
    /// [`super::testing::MacDriver::new`] turns it on so
    /// [`crate::testing::FrameInventory::text_runs`] has something to
    /// report — mirrors `GtkBackend::set_painted_text_recording`.
    pub(crate) fn set_painted_text_recording(&mut self, enabled: bool) {
        self.painted_text_recording = enabled;
    }

    /// Text runs recorded during the last [`Self::enter_frame_scope`]
    /// call, when [`Self::set_painted_text_recording`] is on.
    pub(crate) fn text_runs(&self) -> &[TextRun] {
        &self.text_runs
    }

    /// `WidgetId` of the [`ActivityBar`] that declared
    /// `is_keyboard_focused = true` during the most recent render pass,
    /// or `None` if no bar is focused.
    ///
    /// Read by [`super::run::dispatch_event`] to decide whether a
    /// `KeyPressed` should be redirected into
    /// `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })`
    /// instead of reaching `AppLogic::handle` as a raw key. The macOS twin
    /// of `GtkBackend::focused_activity_bar_id` / `TuiBackend`'s
    /// `apply_dispatch` translation (#465) — without it, `ShellAdapter`'s
    /// built-in activity-bar keyboard navigation (#409) is unreachable on
    /// this backend.
    pub(crate) fn focused_activity_bar_id(&self) -> Option<&WidgetId> {
        self.focused_activity_bar.as_ref()
    }

    /// Zones registered during the last frame via
    /// [`Backend::register_zone`].
    pub(crate) fn zones(&self) -> &[ZoneRec] {
        &self.zones
    }

    /// The currently-stashed `CGContextRef`, or null outside a frame
    /// scope. `draw_*` methods panic if this returns null — same
    /// shape as `GtkBackend::current_cr`.
    pub(crate) fn current_cg(&self) -> CGContextRef {
        self.current_cg_ptr.get() as CGContextRef
    }

    // ── Accelerator matching (#486) ──────────────────────────────────

    /// Look up a registered `Global`-scope accelerator for a
    /// `(key, modifiers)` pair. Mirrors `TuiBackend::match_keypress` /
    /// `GtkBackend::match_keypress` — non-Global entries are skipped
    /// because this backend doesn't own focus/mode context the way a
    /// scoped `KeyMap` resolver does.
    ///
    /// Native Cmd keypresses (`ns_modifier_flags_to_quadraui` maps a real
    /// Cmd into `Modifiers { cmd: true, .. }`) compare directly against
    /// `parsed_accelerators`, which already stores universal bindings
    /// with Cmd instead of Ctrl — see
    /// [`macos_universal_binding_modifiers`], applied once at
    /// `register_accelerator` time so this lookup stays a plain
    /// equality check.
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

    // ── Double-click folding (#486) ──────────────────────────────────

    /// Fold a `MouseDown` into `DoubleClick` if it lands within the
    /// detector's time/position window of the previous click. Every
    /// other variant passes through unchanged. `macos::run` calls this
    /// on each translated `NSEvent` before handing it to `AppLogic`.
    pub(crate) fn fold_double_click(&mut self, ev: UiEvent) -> UiEvent {
        let mut events = [ev];
        self.double_click.process(&mut events);
        let [ev] = events;
        ev
    }

    // ── Text selection (#803) ────────────────────────────────────────
    //
    // The region registry + active-selection state machine lives in
    // [`crate::text_selection::TextSelectionState`] — the same shared
    // implementation `GtkBackend`/`TuiBackend`/`WinBackend` embed. Every
    // method below except [`Self::apply_selection_highlight`]/
    // [`Self::extract_selection_text`] (CoreGraphics painting via
    // [`super::text_selection::draw_selection_highlight`] /
    // `TextRegion::lines` extraction via the shared pixel-based
    // `crate::text_selection::extract_lines_pixel` — same helper GTK/Win
    // use, since macOS is pixel-based too) is a thin delegation. Mirrors
    // `WinBackend`'s identically-named, identically-shaped block in
    // `win/backend.rs` (#741).

    /// Every `TextRegion` registered so far this frame.
    pub(crate) fn text_regions(&self) -> &[TextRegion] {
        &self.text_selection.text_regions
    }

    /// Return the current active text selection, if any.
    pub(crate) fn active_text_selection(
        &self,
    ) -> Option<&crate::text_selection::ActiveTextSelection> {
        self.text_selection.active_text_selection()
    }

    /// Update (or start) the active text selection. Called by
    /// `macos::run::dispatch_event` when a [`UiEvent::TextSelectionChanged`]
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
    /// `WinBackend`'s identically-named method.
    fn cancel_text_selection_drag_impl(&mut self) {
        let mut drag = self.drag_state.borrow_mut();
        self.text_selection.cancel_text_selection_drag(&mut drag);
    }

    /// Record that `id` is the most-recently focused/clicked `TextRegion`.
    /// Called by `macos::run`'s mouse-down handling after a `TextSelection`
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
    /// using its stored `lines` (macOS is pixel-based, like GTK/Win-GUI —
    /// see `TextRegion::lines`'s doc). Empty when there is no active
    /// selection, the region isn't registered this frame, or it has no
    /// `lines` content.
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
            self.current_line_height as f32,
            self.current_char_width as f32,
        )
    }

    /// Paint the active text-selection highlight on top of the frame's
    /// already-painted content. Must be called from inside
    /// [`Self::enter_frame_scope`], after `app.render` has run (so the
    /// highlight sits on top of the rendered content) — see
    /// `macos::run::render_frame`. Mirrors `GtkBackend::apply_selection_highlight`'s
    /// Cairo twin and `WinBackend::apply_selection_highlight`'s Direct2D
    /// twin. No-op when there is no active selection, the region isn't
    /// registered this frame, or metrics aren't known yet.
    pub(crate) fn apply_selection_highlight(&self) {
        let Some(sel) = self.text_selection.active_text_selection() else {
            return;
        };
        let Some(region) = self.text_selection.find_region(&sel.region) else {
            return;
        };
        let char_w = self.current_char_width as f32;
        let line_h = self.current_line_height as f32;
        let Some(ranges) = crate::text_selection::pixel_selection_ranges(
            region.bounds,
            sel.anchor,
            sel.focus,
            line_h,
            char_w,
        ) else {
            return;
        };
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::apply_selection_highlight called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope (see the
        // debug_assert above — a null ctx here means a caller ran this
        // outside `enter_frame_scope`, same contract every other draw_*
        // method on this backend relies on).
        unsafe {
            super::text_selection::draw_selection_highlight(
                ctx,
                region.bounds,
                &ranges,
                char_w as f64,
                line_h as f64,
            );
        }
    }
}

impl Default for MacBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::runtime::PreprocessBackend for MacBackend {
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

    fn focus_manager_mut(&mut self) -> &mut crate::focus::FocusManager {
        &mut self.focus
    }

    fn match_keypress(&self, key: &Key, modifiers: Modifiers) -> Option<AcceleratorId> {
        self.match_keypress(key, modifiers)
    }

    fn fold_double_click(&mut self, ev: UiEvent) -> UiEvent {
        self.fold_double_click(ev)
    }

    /// macOS pastes on Cmd-V/Cmd-Shift-V — the platform convention. The
    /// copy shortcut stays literal Ctrl-C everywhere (this trait's
    /// `is_copy_keypress` default, which `MacBackend` does not override)
    /// — a deliberate cross-platform choice, not an oversight; see
    /// `macos::run::dispatch_event`'s doc.
    fn paste_modifier(&self) -> crate::desktop::PasteModifier {
        crate::desktop::PasteModifier::Cmd
    }
}

impl crate::backend::sealed::Sealed for MacBackend {}

impl Backend for MacBackend {
    fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn begin_frame(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        // Cleared here (frame start) rather than at `end_frame`, matching
        // `GtkBackend`/`TuiBackend`: zones must survive from the moment
        // `register_zone` is called (during `app.render`, inside
        // `enter_frame_scope`) until whatever reads them after the frame
        // (e.g. `MacDriver::inventory`).
        self.zones.clear();
        // Clear the focused activity bar — re-set by `draw_activity_bar`
        // during this render pass if a bar is still keyboard-focused. Same
        // per-frame lifecycle as `zones`, matching `GtkBackend::begin_frame`.
        self.focused_activity_bar = None;
        // #455: clear last frame's modal paint marks so this frame has
        // to earn them again (via draw_dialog/draw_palette/draw_context_menu).
        self.modal_stack.borrow_mut().reset_frame_paint();
        // Clear per-frame text regions so stale registrations from the
        // previous frame don't linger. Mirrors `GtkBackend`/`TuiBackend`/
        // `WinBackend`'s identical `begin_frame` clear (#741, #803).
        self.text_selection.begin_frame();
    }

    fn end_frame(&mut self) {
        // No-op flush-wise. AppKit's `drawRect:` flushes when it returns;
        // this method exists for parity with backends that need an
        // explicit flush.
        //
        // #455: in debug builds, warn about any modal that's registered
        // in the ModalStack (and therefore hit-testable) but that this
        // frame's `AppLogic::render` never painted — the "registered but
        // invisible" defect class (vimcode#587) made detectable instead
        // of silently shipping.
        #[cfg(debug_assertions)]
        for id in self.modal_stack.borrow().unpainted_ids() {
            crate::diagnostics::emit(crate::modal_stack::ModalStack::unpainted_modal_message(&id));
        }
    }

    fn set_theme(&mut self, theme: Theme) {
        self.set_current_theme(theme);
    }

    fn set_nerd_fonts(&mut self, enabled: bool) {
        self.nerd_fonts_enabled = enabled;
    }

    fn nerd_fonts_enabled(&self) -> bool {
        self.nerd_fonts_enabled
    }

    /// Register `bytes` with Core Text via
    /// [`super::text::register_font_from_memory`] (issue #929) — no
    /// filesystem write, process-lifetime only.
    fn register_font_from_memory(&mut self, bytes: &[u8]) -> Option<Vec<String>> {
        super::text::register_font_from_memory(bytes).map(|name| vec![name])
    }

    /// Store `family` as the Nerd-Font (or other PUA-codepoint) fallback
    /// and, if [`Self::set_current_font`] already installed a font,
    /// re-apply it immediately via [`super::text::font_with_fallback`]
    /// (issue #929) — see that method's doc for why the two setters can
    /// land in either order without either effect being lost.
    ///
    /// Issue #1003: also re-applies to `chrome_font` (`set_chrome_font`'s
    /// doc explains why chrome icon glyphs need the same cascade as
    /// editor ones now).
    fn set_nerd_font_fallback(&mut self, family: &str) {
        self.nerd_font_fallback_family = Some(family.to_string());
        if let Some(font) = self.current_font.take() {
            self.current_font = Some(super::text::font_with_fallback(&font, family));
        }
        self.chrome_font = super::text::font_with_fallback(&self.chrome_font, family);
    }

    /// Maps onto the existing [`Self::set_current_font`] machinery
    /// (issue #963) — `current_font` already *is* "the editor font" on
    /// this backend; there was previously just no `Backend`-trait-level
    /// way to reach it by (family, size) the way GTK/Win-GUI expose. If
    /// `family` doesn't resolve to an installed Core Text family, this
    /// silently keeps whatever font was already installed rather than
    /// panicking or clearing it — same "degrade, don't fail" posture
    /// [`super::text::font_with_fallback`]'s doc already documents for
    /// this backend's font handling generally. That "doesn't resolve"
    /// check goes through [`super::text::make_font_exact`], not plain
    /// `make_font`: Core Text substitutes Helvetica for an unknown
    /// family instead of failing, so the plain constructor would install
    /// Helvetica as the *editor* font here rather than keeping the
    /// caller's previous monospace face.
    fn set_editor_font(&mut self, family: &str, size_pt: f32) {
        // Issue #1023: a CSS/Pango generic token (`"monospace"`, …)
        // always resolves to a native system font rather than going
        // through the installed-family check below — there is no
        // "family" to look up, only a system role to build directly.
        let font = match GenericFamily::parse(family) {
            Some(GenericFamily::Monospace) => {
                Some(super::text::system_monospace_font(size_pt as f64))
            }
            // `sans-serif`/`system-ui` aren't monospace — `set_editor_font`'s
            // doc asks for a monospace family — but there is no error
            // channel to reject the mismatch through, so resolve them
            // honestly (the system UI font) rather than silently treating
            // them as an unknown concrete name.
            Some(GenericFamily::SansSerif | GenericFamily::SystemUi) => {
                Some(super::text::system_ui_font(size_pt as f64))
            }
            None => super::text::make_font_exact(family, size_pt as f64),
        };
        if let Some(font) = font {
            self.set_current_font(font);
        }
    }

    /// Chrome twin of [`Self::set_editor_font`] (issue #963) — installs
    /// `font_desc` (Pango-style, e.g. `"Sans 13"`) as `chrome_font` via
    /// [`Self::set_chrome_font`], parsed by [`parse_ui_font_desc`]. Never
    /// touches `current_font`/`current_line_height`/`current_char_width`
    /// — keeping editor and chrome metrics from the same font is exactly
    /// the #912 hazard this separation exists to avoid.
    ///
    /// Degrades to the CoreText system UI font (at the requested size, or
    /// [`DEFAULT_UI_FONT_SIZE_PT`] if `font_desc` doesn't parse at all)
    /// when the named family isn't installed, rather than panicking or
    /// silently keeping a stale chrome font a caller explicitly asked to
    /// change.
    ///
    /// "Isn't installed" is decided by [`super::text::make_font_exact`]
    /// rather than plain `make_font` — `CTFontCreateWithName` answers an
    /// unknown family with Helvetica instead of an error, so the plain
    /// constructor would quietly paint chrome in Helvetica here and never
    /// reach the system-UI-font fallback this doc promises.
    fn set_ui_font(&mut self, font_desc: &str) {
        let font = match parse_ui_font_desc(font_desc) {
            Some((ParsedFamily::Generic(GenericFamily::Monospace), size_pt)) => {
                super::text::system_monospace_font(size_pt)
            }
            Some((
                ParsedFamily::Generic(GenericFamily::SansSerif | GenericFamily::SystemUi),
                size_pt,
            )) => super::text::system_ui_font(size_pt),
            Some((ParsedFamily::Named(family), size_pt)) => {
                super::text::make_font_exact(&family, size_pt)
                    .unwrap_or_else(|| super::text::system_ui_font(size_pt))
            }
            None => super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT),
        };
        self.set_chrome_font(font);
    }

    fn poll_events(&mut self) -> Vec<UiEvent> {
        let mut out: Vec<UiEvent> = self.events.borrow_mut().drain(..).collect();
        // Issue #831: fold in any `UiEvent::User` payloads a background
        // thread queued via `waker()` since the last drain. Unconditional,
        // matching `TuiBackend`/`GtkBackend::poll_events` — `macos::run`'s
        // `paint` closure calls this on every repaint, and `waker()`'s
        // `DispatchQueue::main().exec_async` nudge (see that method's doc)
        // schedules exactly such a repaint out-of-band the moment a
        // background thread wakes.
        self.user_events.drain_into(&mut out);
        out
    }

    fn wait_events(&mut self, _timeout: Duration) -> Vec<UiEvent> {
        // AppKit's run loop is callback-driven; there's no native
        // "wait up to N ms for next event" surface that fits the
        // poll-style trait. Apps that drive macOS through the trait
        // (rather than relying on [`super::run`]'s `AppLogic` flow)
        // should `poll_events` and yield to AppKit via a manual
        // `CFRunLoopRun` iteration. Today this is a plain drain —
        // identical to `poll_events` — and works because the standard
        // app flow goes through `super::run`.
        self.poll_events()
    }

    /// See [`Backend::waker`]'s doc for the full cross-backend contract.
    /// macOS's implementation, like GTK's, can't rely on a bounded poll
    /// alone: `macos::run`'s `paint` closure only drains
    /// [`Self::poll_events`] when AppKit actually calls `drawRect:`, and
    /// nothing periodic forces that — unlike GTK's 33ms idle timer or
    /// TUI's bounded `wait_events(timeout)`, a `MacBackend` app with no
    /// unrelated user input or timer would never repaint at all, so a
    /// background result would sit in the queue forever with nothing to
    /// surface it. `dispatch2::DispatchQueue::main().exec_async` is GCD's
    /// own thread-safe "run this on the main thread" primitive — the piece
    /// #831 found missing crate-wide.
    ///
    /// The dispatched closure can't touch `Rc<RefCell<MacBackend>>`/
    /// `Rc<RefCell<App>>`/the `QuadraView` directly (none are `Send`, and
    /// AppKit's own `NSObject` types are main-thread-only by design in
    /// `objc2` — `waker()` only has `&self` in any case, no handle to the
    /// app or view at all). It instead calls back into
    /// [`Self::wake_callback`], the `Rc<dyn Fn()>` `macos::run::run`
    /// installs via [`Self::set_wake_callback`] that calls
    /// `view.setNeedsDisplay(true)` — the same repaint request
    /// [`crate::runtime::ReactionSink::request_redraw`] issues for every
    /// other event. [`dispatch2::MainThreadBound`] is what makes carrying
    /// that `!Send` closure across the `Send + Sync` boundary `waker()`'s
    /// return type demands sound — see its doc for the argument. macOS
    /// uses `dispatch2`'s own audited wrapper (already a dependency here
    /// for `exec_async`) rather than anything homegrown, precisely because
    /// the hard part is the terminal drop: `MainThreadBound` stores its
    /// value in a `ManuallyDrop` and re-dispatches `T`'s destructor back
    /// to the main thread via `run_on_main`. The other two backends dodge
    /// the question instead of answering it — GTK keeps its `Rc` in a
    /// main-thread thread-local and sends only an integer id
    /// (`gtk::backend::WAKE_CALLBACKS`), and Windows needs no wrapper at
    /// all because `wndproc` already holds the state its `PostMessageW`
    /// wake dispatches through.
    fn waker(&self) -> Arc<dyn Fn(UserPayload) + Send + Sync> {
        let queue = Arc::clone(&self.user_events);
        let wake_callback = Arc::clone(&self.wake_callback);
        Arc::new(move |payload: UserPayload| {
            queue.push(payload.into_arc());
            let wake_callback = Arc::clone(&wake_callback);
            DispatchQueue::main().exec_async(move || {
                if let Some(bound) = wake_callback.get() {
                    // SAFETY: `exec_async`'s closure is guaranteed by GCD's
                    // own contract to run on the main queue's thread — the
                    // real, OS-level main thread for a process that has
                    // called `NSApplicationMain`/run an `NSApplication`,
                    // which every `macos::run::run` caller has by the time
                    // `set_wake_callback` could have installed anything
                    // here to call.
                    let mtm = unsafe { MainThreadMarker::new_unchecked() };
                    (bound.get(mtm))();
                }
            });
        })
    }

    /// See [`Backend::request_frame_in`]'s doc for the cross-backend
    /// contract (quadraui#832). Runs synchronously on the main thread
    /// (called directly from event/tick handling, never from a
    /// background thread), but still needs [`Self::tick_callback`]'s
    /// `MainThreadBound` indirection rather than capturing `view`/`app`
    /// state directly: `DispatchQueue::after`'s closure type is bounded
    /// `Send` regardless of when it actually runs (GCD's Rust binding
    /// can't statically prove "this queue happens to be the main one, so
    /// a `!Send` capture is fine"), the same restriction [`Self::waker`]
    /// works around — this just reaches the *tick* target instead of the
    /// *redraw* one.
    ///
    /// A no-op if [`Self::tick_callback`] hasn't been installed yet (a
    /// `MacBackend` never handed to `macos::run::run`, or a call during
    /// the narrow window before `run` installs it) — the scheduled
    /// closure below simply finds nothing to call, same posture as
    /// `waker`'s own "no callback yet" case.
    fn request_frame_in(&self, delay: Duration) {
        let tick_callback = Arc::clone(&self.tick_callback);
        let Ok(when) = DispatchTime::try_from(delay) else {
            return;
        };
        let _ = DispatchQueue::main().after(when, move || {
            if let Some(bound) = tick_callback.get() {
                // SAFETY: see `waker`'s identical comment above — `after`
                // guarantees this closure runs on the main queue's thread.
                let mtm = unsafe { MainThreadMarker::new_unchecked() };
                (bound.get(mtm))();
            }
        });
    }

    fn register_accelerator(&mut self, acc: &Accelerator) {
        // Re-registration replaces the prior entry — both in the map and
        // the parsed list, otherwise a stale binding would shadow the
        // new one in `match_keypress`. Mirrors
        // `TuiBackend::register_accelerator` / `GtkBackend`'s equivalent.
        self.accelerators.insert(acc.id.clone(), acc.clone());
        self.parsed_accelerators.retain(|(_, id)| id != &acc.id);
        if let Some(parsed) = parse_binding(&acc.binding) {
            let parsed = macos_universal_binding_modifiers(&acc.binding, parsed);
            self.parsed_accelerators.push((parsed, acc.id.clone()));
        }
    }

    fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.accelerators.remove(id);
        self.parsed_accelerators.retain(|(_, eid)| eid != id);
    }

    /// Issue #930: this used to `.expect()` the `MainThreadMarker`, which
    /// panics whenever called off the main thread. Every *real* call site
    /// (`macos::shell_runner`) is already on the main thread, so that
    /// never fired in production — but Rust's test runner hands every
    /// `#[test]` fn its own spawned thread, so any harness driving
    /// `ShellApp::setup` through a call to this method panicked, taking
    /// down a `setup()` that has nothing else to do with menus. The five
    /// sibling helpers in [`super::menu_bar_install`] already degrade
    /// gracefully (`let Some(mtm) = MainThreadMarker::new() else { ... }`)
    /// instead of panicking; this matches that shape — a silent no-op
    /// off the main thread, same posture as `request_frame_in`'s
    /// "no callback yet" no-op above.
    fn install_menu_bar(&mut self, bar: &crate::primitives::menu_bar::MenuBar) {
        let Some(mtm) = objc2_foundation::MainThreadMarker::new() else {
            return;
        };
        // Replacing wholesale — the previous target drops when this
        // assignment runs, after the new menu is installed.
        let target = super::menu_bar_install::install_menu_bar(mtm, bar, self.events.clone());
        self.menu_target = Some(target);
    }

    /// See [`Self::install_menu_bar`]'s doc (#930) — same off-main-thread
    /// degradation, same rationale.
    fn show_context_menu(
        &mut self,
        menu: &crate::primitives::context_menu::ContextMenu,
        anchor: crate::event::Point,
    ) {
        let Some(mtm) = objc2_foundation::MainThreadMarker::new() else {
            return;
        };
        // Blocks on AppKit's modal pop-up loop until the user picks
        // an item or dismisses; pushes `ContextMenuItemActivated` /
        // `ContextMenuDismissed` onto the events queue.
        super::menu_bar_install::show_context_menu(
            mtm,
            menu,
            anchor.x as f64,
            anchor.y as f64,
            self.events.clone(),
        );
    }

    fn modal_stack_handle(&self) -> Rc<RefCell<ModalStack>> {
        self.modal_stack.clone()
    }

    fn drag_state_handle(&self) -> Rc<RefCell<DragState>> {
        self.drag_state.clone()
    }

    fn focus_manager(&self) -> &crate::focus::FocusManager {
        &self.focus
    }

    fn draw_focus_ring(&mut self, rect: Rect) {
        let theme = self.current_theme;
        self.surface_stroke_rect(rect, theme.accent_fg, crate::focus::FOCUS_RING_STROKE_WIDTH);
        self.register_zone(WidgetId::new("chrome:focus-ring"), rect);
    }

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    fn register_zone(&mut self, id: WidgetId, bounds: Rect) {
        self.zones.push(ZoneRec { id, bounds });
    }

    // ─── Text selection (#803) ──────────────────────────────────────────

    /// Overrides the trait's no-op default — see [`Self::text_regions`]
    /// and `crate::text_selection::TextSelectionState`. Mirrors
    /// `GtkBackend`/`WinBackend`'s identical override.
    fn register_text_region(&mut self, region: TextRegion) {
        self.text_selection.register_text_region(region);
    }

    /// Overrides the trait's no-op default — see
    /// [`Self::cancel_text_selection_drag_impl`].
    fn cancel_text_selection_drag(&mut self) {
        self.cancel_text_selection_drag_impl();
    }

    /// quadraui#492: honest per-method, not aspirational.
    ///
    /// - `mouse` / `scroll` / `drag`: `macos::run`'s view subclass forwards
    ///   `mouseDown:`/`mouseUp:`/`scrollWheel:`/`mouseDragged:` through
    ///   `macos::events`, so all three input kinds reach `poll_events`.
    /// - `native_menu`: `install_menu_bar` / `show_context_menu` are both
    ///   overridden below (`NSMenu`).
    /// - `file_dialogs` / `folder_dialogs` / `notifications`:
    ///   `MacPlatformServices` uses real `NSOpenPanel`/`NSSavePanel` (the
    ///   former in both file-picking and, since quadraui#935,
    ///   directory-picking mode) and `osascript` notifications
    ///   (`src/macos/services.rs`), not stubs.
    /// - `native_dialogs` (quadraui#936): `MacPlatformServices::show_message_dialog`
    ///   now shows a real `NSAlert` and returns the chosen button's id —
    ///   see `src/macos/services.rs`. Like Win-GUI's equivalent
    ///   declaration (`win/backend.rs`, #744), `CAP_CONTRACTS`'s
    ///   `native_dialogs` entry is `Unprovable` (there is no no-op
    ///   default `show_message_dialog` diverges from — every backend
    ///   implements it), so this declaration isn't source-checked by the
    ///   honesty test; it's true because the alert is real.
    /// - `window_chrome`: `begin_window_drag` / `toggle_window_maximize`
    ///   are overridden below, via the shared `crate::desktop::WindowDragArm`
    ///   (#498) — `CAP_CONTRACTS`'s `window_chrome` cap only requires
    ///   *any* of the three CSD methods, and these two satisfy it.
    ///   `begin_window_resize` stays the trait's no-op default: the
    ///   vendored `objc2-app-kit` binding has no public "begin native
    ///   edge-resize" primitive (unlike `performWindowDragWithEvent:` /
    ///   `zoom:`), and a manual `setFrame_display`-driven implementation
    ///   would need live mouse-motion wiring through `QuadraView` that
    ///   doesn't exist yet — a real gap, tracked as follow-up rather than
    ///   guessed at blind (no macOS host in this dev loop to verify the
    ///   geometry math against).
    /// - `pointer_cursor`: `set_cursor` is overridden below, via
    ///   `crate::desktop`'s `PointerShape`/`ResizeEdge` scaffold.
    /// - `text_selection` (#803): `register_text_region`/
    ///   `cancel_text_selection_drag` are both overridden above, backed by
    ///   the shared `crate::text_selection::TextSelectionState`
    ///   `GtkBackend`/`TuiBackend`/`WinBackend` also embed —
    ///   `macos::run::dispatch_event` wires drag-select and Ctrl-C through
    ///   the same pipeline, so `panel.drag_select_copy` (which
    ///   `requires: ["text_selection"]`) now runs against `MacDriver`
    ///   instead of skipping. Before #803 this backend declared neither
    ///   `register_text_region` nor a drag pipeline for it, so the cap
    ///   stayed unset (see #493's original note, now stale).
    /// - `app_font_registration` (#929): `register_font_from_memory` /
    ///   `set_nerd_font_fallback` are both overridden above, backed by
    ///   real `CTFontManagerRegisterGraphicsFont` /
    ///   `CTFontCreateCopyWithAttributes` calls in `super::text` rather
    ///   than the trait's no-op default.
    /// - `generic_font_families` (#1023): `set_editor_font`/`set_ui_font`
    ///   are both overridden above and resolve CSS/Pango generic family
    ///   tokens to a real CoreText font (`super::text::system_monospace_font`/
    ///   `system_ui_font`) instead of discarding an unresolvable literal
    ///   family name.
    /// - Everything else — `ime` — is **not** declared: no macOS IME
    ///   integration exists yet.
    fn backend_caps(&self) -> crate::backend::BackendCaps {
        crate::backend::BackendCaps {
            mouse: true,
            scroll: true,
            drag: true,
            native_menu: true,
            file_dialogs: true,
            folder_dialogs: true,
            notifications: true,
            window_chrome: true,
            pointer_cursor: true,
            text_selection: true,
            app_font_registration: true,
            native_dialogs: true,
            // `generic_font_families` (issue #1023): `set_editor_font`/
            // `set_ui_font` are both overridden above and now resolve
            // CSS/Pango generic tokens to a real CoreText font
            // (`super::text::system_monospace_font`/`system_ui_font`)
            // instead of an unresolvable literal family name.
            generic_font_families: true,
            // `window` (issue #950) is overridden below and returns
            // `Some` once `set_window` has stashed a real `NSWindow` —
            // see `impl WindowControl for MacBackend`'s doc for the
            // AppKit calls backing each method.
            window_control: true,
            // `tray` (issue #953) is overridden below and always returns
            // `Some` — see `impl TrayService for MacBackend`'s doc for
            // the `NSStatusBar`/`NSStatusItem` calls backing each method.
            tray: true,
            ..crate::backend::BackendCaps::empty()
        }
    }

    // ─── Window chrome (CSD, #498) ──────────────────────────────────────
    //
    // See `backend_caps`'s doc above for exactly which of the three
    // window-chrome methods are overridden and why.

    fn begin_window_drag(&mut self) -> bool {
        // Unlike `GtkBackend::begin_window_drag`, this does not need to
        // defer past a movement threshold: `NSWindow::performWindowDragWithEvent:`
        // is designed to be called synchronously from `mouseDown:` and
        // disambiguates a drag from a double-click internally (that's
        // the whole point of the API — see Apple's docs), so there is no
        // "swallows the second press" hazard `WindowDragArm::commit_if_past_threshold`
        // exists to avoid on GTK/GDK. Takes the stashed press
        // unconditionally instead.
        let Some(window) = self.window.as_ref() else {
            return false;
        };
        let Some(event) = self.pending_window_press.take() else {
            return false;
        };
        window.performWindowDragWithEvent(&event);
        true
    }

    fn toggle_window_maximize(&mut self) -> bool {
        let Some(window) = self.window.as_ref() else {
            return false;
        };
        // `zoom:` (not `NSWindow::toggleFullScreen:`) is the
        // double-click-to-maximize equivalent: it toggles between the
        // window's current frame and its "zoomed" (ideal) frame, same
        // as clicking the native green titlebar button.
        window.zoom(None);
        true
    }

    /// Sets the cursor via `NSCursor::set()`, not the `push`/`pop`
    /// pairing AppKit's own `cursorUpdate:`/tracking-area handlers
    /// conventionally use. That pairing exists to *restore* whatever
    /// cursor was showing before a transient region was entered; this
    /// backend has no such transient-region concept — `Backend::set_cursor`
    /// is called once per frame with the pointer shape the app wants
    /// showing *now* (mirroring GTK's equally stateless `set_cursor`
    /// override), and there is no "previous" cursor context to pop back
    /// to when that ends. A `push`/`pop` implementation here would leak
    /// stack depth with no matching `pop` call to balance it. If a
    /// future caller needs scoped push/pop semantics (e.g. a transient
    /// hover cursor that must restore the ambient one), that's a new,
    /// deliberately-scoped API on top of this one — not a reason to
    /// replace this plain `.set()`.
    fn set_cursor(&mut self, shape: PointerShape) -> bool {
        if self.window.is_none() {
            return false;
        }
        // `NSCursor::set` is a safe method as of objc2-app-kit 0.3 (#796
        // bump) — previously `unsafe`, documented safe to call any time
        // this view's window is key; called from `AppLogic::handle` (via
        // `Backend::set_cursor`), which only ever runs on the main thread
        // inside the live AppKit run loop.
        mac_cursor_for_shape(shape).set();
        true
    }

    // ─── Window control (issue #950) ────────────────────────────────────
    fn window(&mut self) -> Option<&mut dyn crate::backend::WindowControl> {
        self.window.as_ref()?;
        Some(self)
    }

    // ─── Tray / status-bar icon (issue #953) ────────────────────────────
    /// Unlike [`Self::window`], not gated on any prior state — an
    /// `NSStatusItem` is created lazily, on first [`TrayService::set_icon`]
    /// call (see `impl TrayService for MacBackend`'s doc), so there is no
    /// "not constructed yet" case to report `None` for the way an
    /// `NSWindow` genuinely can be absent before `macos::run` builds one.
    fn tray(&mut self) -> Option<&mut dyn crate::backend::TrayService> {
        Some(self)
    }

    /// #947: reports the region AppKit's native traffic-light controls
    /// (close/minimize/zoom) occupy, in `QuadraView`'s own top-left-origin,
    /// y-down coordinate space — the same space every other `Rect` this
    /// trait hands back (and receives, e.g. `draw_focus_ring`) already
    /// uses, since `QuadraView` sets `isFlipped = YES` (see
    /// `macos::headless`'s module doc for the same convention on the
    /// headless bitmap path).
    ///
    /// Derives the inset from
    /// `standardWindowButton(NSWindowButton::CloseButton)`'s immediate
    /// superview — AppKit's own private container view for the
    /// three-button cluster, already sized to exactly their combined
    /// bounding box — rather than unioning three individual button
    /// frames or hardcoding the ~78pt Apple's HIG happens to use today.
    /// That spacing is not an API contract: it moves with accessibility
    /// settings (`Increase contrast`, larger click targets) and could
    /// change in a future macOS release, so reading AppKit's own layout
    /// is the only way this stays correct without needing to be
    /// revisited by hand.
    ///
    /// Returns `Rect::default()` — same as the trait default — when no
    /// window is set yet (`self.window` stays `None` until
    /// `macos::run::run_with` calls [`Self::set_window`]) or AppKit
    /// hands back no button/container, e.g. a borderless style mask with
    /// no `Titled` bit. Never called before `client_side_titlebar` was
    /// set at window-creation time in practice, but safe to call any
    /// time regardless.
    fn titlebar_control_inset(&self) -> Rect {
        let Some(window) = self.window.as_ref() else {
            return Rect::default();
        };
        let Some(close_button) = window.standardWindowButton(NSWindowButton::CloseButton) else {
            return Rect::default();
        };
        // SAFETY: `superview` is safe to call on any live `NSView` on the
        // main thread — `close_button` is a retained, currently-installed
        // subview of the window's titlebar, so it always has one.
        let Some(container) = (unsafe { close_button.superview() }) else {
            return Rect::default();
        };
        // `convertRect_toView(_, None)` reaches the window's base
        // coordinate system (bottom-left origin) directly, regardless of
        // how deep `container` sits in AppKit's private titlebar view
        // hierarchy — more robust than assuming `container.frame()` is
        // already in window coordinates.
        let in_window = container.convertRect_toView(container.bounds(), None);
        let content_height = window
            .contentView()
            .map(|view| view.frame().size.height)
            .unwrap_or_else(|| window.frame().size.height);
        // Flip AppKit's bottom-left-origin button frame into
        // `QuadraView`'s top-left-origin, y-down space.
        let bottom_from_top = (content_height - in_window.origin.y).max(0.0);
        Rect::new(
            0.0,
            0.0,
            (in_window.origin.x + in_window.size.width) as f32,
            bottom_from_top as f32,
        )
    }

    fn line_height(&self) -> f32 {
        self.current_line_height as f32
    }

    fn char_width(&self) -> f32 {
        self.current_char_width as f32
    }

    /// `chrome_char_width`, not `current_char_width` — [`Self::draw_list`]
    /// paints row text with `chrome_font` (issue #1003), and
    /// `chrome_char_width` is that font's real advance, kept in sync by
    /// [`Self::set_chrome_font`] whenever [`Backend::set_ui_font`]
    /// changes it. This is exactly the metric-mismatch hazard `chrome_font`
    /// / `current_font`'s separate cached widths exist to avoid (#912) —
    /// see `chrome_font`'s field doc.
    fn list_char_width(&self) -> f32 {
        self.chrome_char_width as f32
    }

    // ── Drawing ────────────────────────────────────────────────────

    fn draw_tree(&mut self, rect: Rect, tree: &TreeView) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tree called outside enter_frame_scope",
        );
        // Issue #1003: row labels/badges/chevrons are chrome
        // (`ChromePrimitive::Tree`), not editor content — paint through
        // `chrome_font`, matching `GtkBackend::draw_tree`'s #624 swap.
        // Unlike `current_font`, `chrome_font` is never `None` (seeded at
        // construction — see its field doc), so there is no "no font yet"
        // fallback branch to keep. `line_height` deliberately stays
        // `current_line_height` (the editor pitch) — mirroring
        // `GtkBackend::draw_tree`, which passes its own
        // `self.current_line_height` unchanged; row pitch tracking
        // `chrome_font`'s metrics is the pre-existing, deliberately
        // deferred half of #624's gap (see `GtkBackend::draw_context_menu`'s
        // comment), not something this fix introduces or closes.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::tree::draw_tree(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                tree,
                &theme,
                line_height,
                self.nerd_fonts_enabled,
            );
        }
    }
    fn draw_list(&mut self, rect: Rect, list: &ListView) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_list called outside enter_frame_scope",
        );
        // Issue #1003: the list is chrome (`ChromePrimitive::List`) — see
        // `draw_tree`'s comment just above for why this reads
        // `chrome_font` instead of `current_font` while `line_height`
        // deliberately stays the editor pitch.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::list::draw_list(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                list,
                &theme,
                line_height,
            );
        }
    }
    fn draw_data_table(
        &mut self,
        rect: Rect,
        table: &DataTable,
        hovered_idx: Option<usize>,
    ) -> DataTableLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_data_table called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_data_table requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::data_table::draw_data_table(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                table,
                &theme,
                line_height,
                hovered_idx,
            )
        }
    }
    fn data_table_layout(&self, rect: Rect, table: &DataTable) -> DataTableLayout {
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::data_table_layout requires set_current_font");
        super::data_table::mac_data_table_layout(
            table,
            font,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
        )
    }
    fn list_hscrollbar(&self, rect: Rect, list: &ListView) -> Option<crate::Scrollbar> {
        // `ListView::h_scroll` and `max_content_width` are in character columns,
        // but macOS works in pixels.  Convert with `current_char_width` so the
        // returned `Scrollbar` track/thumb are in pixel units — matching what
        // `macos::draw_list` paints and what mouse-event coords use.
        let char_w = self.current_char_width as f32;
        let max_w_chars = list.max_content_width? as f32;
        let content_px = max_w_chars * char_w;
        let border_inset = if list.bordered { char_w } else { 0.0 };
        let visible_px = (rect.width - 2.0 * border_inset).max(0.0);
        if content_px <= visible_px {
            return None;
        }
        let row_h = self.line_height();
        let track = crate::primitives::list::hscrollbar_track(rect, list.bordered, char_w, row_h);
        Some(crate::Scrollbar::horizontal(
            list.id.clone(),
            track,
            list.h_scroll as f32 * char_w,
            content_px,
            visible_px,
            row_h,
        ))
    }
    fn list_vscrollbar(&self, rect: Rect, list: &ListView) -> Option<crate::Scrollbar> {
        // macOS ListView vertical-scrollbar rasteriser not yet implemented.
        // Delegate to the primitive's geometry method using pixel units:
        // each "row" is one line_height tall (the primitive column-width
        // parameter is reused as row_height here, same as for TUI).
        let row_h = self.line_height();
        list.vscrollbar(
            crate::event::Rect::new(rect.x, rect.y, rect.width, rect.height),
            row_h,
        )
    }
    fn list_layout(&self, rect: Rect, list: &ListView) -> crate::ListViewLayout {
        // No font-role change here (issue #1003): `mac_list_layout` takes
        // no font at all, only pitch — same `current_line_height`/
        // `current_char_width` `GtkBackend::list_layout` uses.
        super::list::mac_list_layout(
            list,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
            self.current_char_width,
        )
    }
    /// #808: shared field-kind painting lives in
    /// [`crate::primitives::form::paint`] now — see that fn's doc for why
    /// `FieldKind::Toolbar` is the one variant painted here instead,
    /// after `paint` releases its exclusive borrow of `self`.
    fn draw_form(&mut self, rect: Rect, form: &Form) {
        debug_assert!(
            !self.current_cg().is_null(),
            "MacBackend::draw_form called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .clone()
            .expect("MacBackend::draw_form requires set_current_font");
        let theme = self.current_theme;
        let flayout = super::form::mac_form_layout(form, rect, self.current_line_height, &font);
        let origin = Point::new(rect.x, rect.y);
        crate::primitives::form::paint(form, &flayout, self, &theme, origin);

        for vf in &flayout.visible_fields {
            let Some(field) = form.fields.get(vf.field_idx) else {
                continue;
            };
            let FieldKind::Toolbar(toolbar) = &field.kind else {
                continue;
            };
            let label_text: String = field.label.spans.iter().map(|s| s.text.as_str()).collect();
            let no_label = label_text.is_empty();
            let (label_w, _) = super::text::measure_text(&font, &label_text);
            let row_x = (origin.x + vf.bounds.x) as f64;
            let row_y = (origin.y + vf.bounds.y) as f64;
            let row_w = vf.bounds.width as f64;
            let row_h = vf.bounds.height as f64;
            let toolbar_x = if no_label {
                row_x + 6.0
            } else {
                row_x + 6.0 + label_w + 12.0
            };
            let toolbar_w = row_x + row_w - toolbar_x;
            if toolbar_w > 0.0 {
                let ctx = self.current_cg();
                debug_assert!(
                    !ctx.is_null(),
                    "MacBackend::draw_form called outside enter_frame_scope",
                );
                // SAFETY: ctx is non-null inside the frame scope.
                unsafe {
                    super::toolbar::draw_toolbar(
                        ctx, &font, toolbar_x, row_y, toolbar_w, row_h, toolbar, &theme, None, None,
                    );
                }
            }
        }
    }
    fn draw_palette(&mut self, rect: Rect, palette: &Palette) {
        // #455: see `modal_stack.rs`'s "Paint-consistency detection" docs —
        // records that this palette's surface was actually painted this frame.
        self.modal_stack.borrow_mut().mark_painted(&palette.id);
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_palette called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_palette requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::palette::draw_palette(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                palette,
                &theme,
                line_height,
            );
        }
    }

    fn palette_layout(&self, rect: Rect, palette: &Palette) -> crate::PaletteLayout {
        super::palette::mac_palette_layout(
            palette,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
        )
    }

    fn draw_settings_chrome(
        &mut self,
        rect: Rect,
        header_text: &str,
        query: &str,
        placeholder: &str,
        active: bool,
    ) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_settings_chrome called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_settings_chrome requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::form::draw_settings_chrome(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                line_height,
                header_text,
                query,
                placeholder,
                active,
                &theme,
            );
        }
    }

    fn draw_status_bar_interactive(
        &mut self,
        rect: Rect,
        bar: &StatusBar,
        interaction: &crate::interaction::InteractionState,
    ) -> StatusBarLayout {
        let (hovered_id, pressed_id) = (interaction.hovered(), interaction.pressed());
        // `NativeSurface::surface_fill_rect`/`surface_draw_text_run` (etc)
        // each debug_assert their own `!ctx.is_null()` internally — see
        // `Self::surface_fill_rect` — so this method needs no separate
        // ctx/font fetch of its own, matching `Self::draw_panel`'s #859
        // shape.
        //
        // Issue #963: the status bar is chrome, not editor content, so it
        // paints through `ChromeSurface` (routes text through
        // `chrome_font`/`chrome_line_height`) rather than `self` directly
        // (which would paint through `current_font`, the *editor* font,
        // via `MacBackend`'s own `NativeSurface` impl below).
        let theme = self.current_theme;
        let line_height = self.chrome_line_height as f32;
        let mut surface = ChromeSurface { backend: self };
        crate::primitives::status_bar::native_surface_paint::paint(
            bar,
            &mut surface,
            &theme,
            rect.x,
            rect.y,
            rect.width,
            line_height,
            hovered_id,
            pressed_id,
        )
    }
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn draw_tab_bar(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits {
        // Icon-less spelling of `draw_tab_bar_icons` — an empty sidecar
        // reproduces this bar pixel for pixel (#926), so the two share
        // one implementation rather than two paint paths that can drift.
        self.draw_tab_bar_icons(rect, bar, &[], hovered_close_tab)
    }
    /// Per-tab icon glyphs (#620), implemented on macOS as of #926:
    /// [`super::tab_bar::mac_tab_icon_extras`] measures each glyph with
    /// CoreText and every geometry consumer reserves that width, so a
    /// decorated tab's label and close-button hit box shift together and
    /// a click on the painted × still closes the tab. Before #926 this
    /// forwarded to the icon-less rasteriser (dropping the glyphs) behind
    /// a `debug_assert!` that killed the host process on its first
    /// decorated frame (#931 replaced the assert; this closes the gap it
    /// was guarding).
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn draw_tab_bar_icons(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tab_bar called outside enter_frame_scope",
        );
        // Issue #1003: tab labels are chrome (`ChromePrimitive::TabBar`)
        // — see `draw_tree`'s comment. This one root method covers
        // `draw_tab_bar`/`draw_tab_bar_with_chrome` too: both delegate
        // here with an empty icon sidecar (`Backend::draw_tab_bar_with_chrome`'s
        // default body forwards to `draw_tab_bar`, which forwards here).
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: `ctx` is non-null inside the frame scope.
        // `super::tab_bar::draw_tab_bar_icons` already honours `rect.y`
        // via its own `y_offset` parameter, but is never handed `rect.x`
        // and always paints/measures from `x = 0` — issue #934. The
        // `CGContextTranslateCTM` X-only shift is what places the ink at
        // `rect.x` (mirroring `Self::draw_activity_bar`'s CTM treatment of
        // that other bar-relative-in-both-axes rasteriser); `y_offset`
        // stays `rect.y` as before, unaffected. `shift_tab_bar_hits`
        // afterward is what makes the *returned* `TabBarHits` honour
        // `Backend::tab_bar_layout`'s documented absolute-x contract,
        // matching the TUI/GTK convention.
        unsafe {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, rect.x as f64, 0.0);
            let mut hits = super::tab_bar::draw_tab_bar_icons(
                ctx,
                font,
                rect.width as f64,
                line_height,
                rect.y as f64,
                rect.height as f64,
                bar,
                &theme,
                hovered_close_tab,
                icons,
            );
            CGContextRestoreGState(ctx);
            crate::backend::shift_tab_bar_hits(&mut hits, rect.x as f64);
            hits
        }
    }
    /// Issue #919's `TabBarLayout`-returning counterpart to
    /// [`Self::draw_tab_bar`] above. Paints exactly as that method does
    /// (discarding the deprecated `TabBarHits` it returns), then
    /// separately resolves the real `TabBarLayout` via
    /// [`super::tab_bar::mac_tab_bar_native_layout_icons`] — see that
    /// function's doc for why macOS can't share one measurement path
    /// between its `TabBarHits` and `TabBarLayout` accessors the way
    /// TUI/GTK/Win do. Both halves happen inside
    /// [`Self::draw_tab_bar_icons_layout`], which this forwards to with
    /// an empty sidecar (#926).
    fn draw_tab_bar_layout(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> TabBarLayout {
        // Icon-less spelling of `draw_tab_bar_icons_layout` (#926) — see
        // `draw_tab_bar` above for why the empty sidecar shares one path.
        self.draw_tab_bar_icons_layout(rect, bar, &[], hovered_close_tab)
    }
    /// `TabBarLayout`-returning counterpart to
    /// [`Self::draw_tab_bar_icons`] — same CoreText icon-width pass
    /// (#926), applied to both the paint and the separately-computed
    /// [`super::tab_bar::mac_tab_bar_native_layout_icons`] below so the
    /// returned layout describes the pixels this call just painted.
    fn draw_tab_bar_icons_layout(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
        hovered_close_tab: Option<usize>,
    ) -> TabBarLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tab_bar_layout called outside enter_frame_scope",
        );
        // Issue #1003: `TabBarLayout`-returning twin of
        // `draw_tab_bar_icons` — see that method's comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: `ctx` is non-null inside the frame scope. Same X-only
        // CTM translate as `Self::draw_tab_bar_icons` above — this call
        // discards the painted `TabBarHits` anyway, so there is no return
        // value left to shift; only the ink needs to land at `rect.x`.
        #[allow(deprecated)] // discarded `TabBarHits` — issue #823
        unsafe {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, rect.x as f64, 0.0);
            let _ = super::tab_bar::draw_tab_bar_icons(
                ctx,
                font,
                rect.width as f64,
                line_height,
                rect.y as f64,
                rect.height as f64,
                bar,
                &theme,
                hovered_close_tab,
                icons,
            );
            CGContextRestoreGState(ctx);
        }
        super::tab_bar::mac_tab_bar_native_layout_icons(
            font,
            rect.width as f64,
            rect.height as f64,
            bar,
            icons,
        )
    }
    fn draw_activity_bar(
        &mut self,
        rect: Rect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
    ) -> Vec<ActivityBarRowHit> {
        // Track keyboard focus so `run::dispatch_event` can redirect the
        // next `KeyPressed` into this bar (#465) — same contract
        // `GtkBackend::draw_activity_bar` implements.
        if bar.is_keyboard_focused {
            self.focused_activity_bar = Some(bar.id.clone());
        }
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_activity_bar called outside enter_frame_scope",
        );
        // Issue #1003: this is the mandatory, non-default `Backend`
        // method — `AppShell::render` (`compose/app_shell.rs`) and
        // `ScreenLayout::draw`'s `Surface::ActivityBar` arm
        // (`frame.rs`) both call this method directly, never
        // `draw_activity_bar_with_style`, whose own trait-level default
        // just forwards back to this one. So this is the method real
        // hosts actually reach, and it painted its icon glyph through
        // `current_font` (the editor font) — the exact headline symptom
        // this issue was filed over — even after
        // `draw_activity_bar_with_style` below was fixed to read
        // `chrome_font`. Route through `chrome_font` here too, matching
        // that fix and every other chrome primitive's swap (see
        // `draw_tree`'s comment). Unlike `current_font`, `chrome_font`
        // is never `None` (seeded at construction — see its field doc),
        // so there is no "no font yet" fallback to preserve.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx non-null inside frame scope. `super::activity_bar`'s
        // rasteriser is bar-relative by contract (issue #552) — it always
        // paints into `(0, 0, width, height)` and returns bar-relative hit
        // spans — so the CTM translate below is what actually places the
        // ink at `rect`'s absolute origin (issue #934), matching
        // `GtkBackend::draw_activity_bar`'s `cr.translate(rect.x, rect.y)`.
        // The returned `ActivityBarRowHit`s are untouched by the
        // translate — they stay bar-relative, per contract.
        unsafe {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, rect.x as f64, rect.y as f64);
            let hits = super::activity_bar::draw_activity_bar(
                ctx,
                font,
                rect.width as f64,
                rect.height as f64,
                bar,
                &theme,
                hovered_idx,
                self.nerd_fonts_enabled,
            );
            CGContextRestoreGState(ctx);
            hits
        }
    }

    fn draw_activity_bar_with_style(
        &mut self,
        rect: Rect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
        style: &crate::ActivityBarStyle,
    ) -> Vec<ActivityBarRowHit> {
        // Track keyboard focus — same contract as `draw_activity_bar` above.
        if bar.is_keyboard_focused {
            self.focused_activity_bar = Some(bar.id.clone());
        }
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_activity_bar_with_style called outside enter_frame_scope",
        );
        // Issue #1003: despite this method's name appearing in the
        // original audit's "already correct" column, it painted its icon
        // glyph through `current_font` (the editor font) exactly like
        // the other 14 — a bar whose icon size tracked the user's
        // editor-font size, not the fixed chrome size every other chrome
        // primitive uses. `ChromePrimitive::ActivityBar` — see
        // `draw_tree`'s comment for the swap. `draw_activity_bar` above
        // (the mandatory, non-default method real hosts call — see its
        // comment) needed the identical fix; this one is fixed too so
        // the two methods agree.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx non-null inside frame scope. See `draw_activity_bar`
        // above for why the CTM translate is what makes this bar-relative
        // rasteriser paint at `rect`'s absolute origin (issue #934).
        unsafe {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, rect.x as f64, rect.y as f64);
            let hits = super::activity_bar::draw_activity_bar_with_style(
                ctx,
                font,
                rect.width as f64,
                rect.height as f64,
                bar,
                style,
                &theme,
                hovered_idx,
                self.nerd_fonts_enabled,
            );
            CGContextRestoreGState(ctx);
            hits
        }
    }

    fn status_bar_layout(&self, rect: Rect, bar: &StatusBar) -> StatusBarLayout {
        // No-paint twin of `draw_status_bar`: same `mac_status_bar_layout`
        // call, so hit regions match the painted frame exactly. Hit
        // regions are bar-local, so `rect.x` / `rect.y` are deliberately
        // not folded in (quadraui#552 — audited, no change needed).
        //
        // Issue #963: measures against `chrome_font`/`chrome_line_height`,
        // matching `draw_status_bar_interactive`'s `ChromeSurface` switch
        // above — unlike `current_font`, `chrome_font` is never `None`
        // (seeded at construction, see its field doc), so this no longer
        // needs a "called before a font was installed" fallback branch.
        super::status_bar::mac_status_bar_layout(
            &self.chrome_font,
            rect.width as f64,
            self.chrome_line_height,
            bar,
        )
    }

    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout(&self, rect: Rect, bar: &TabBar) -> TabBarHits {
        // No-paint twin of `draw_tab_bar`, routed through the same
        // `mac_tab_bar_layout_icons` + `shift_tab_bar_hits` pair
        // `tab_bar_layout_icons` below uses, so both agree on the
        // documented absolute contract (issue #552 / #934).
        self.tab_bar_layout_icons(rect, bar, &[])
    }

    /// No-paint twin of [`Self::draw_tab_bar_icons`], routed through the
    /// same [`super::tab_bar::mac_tab_bar_layout_icons`] with the same
    /// sidecar — so the load-bearing macOS invariant (`tab_bar_layout*`
    /// returns exactly what `draw_tab_bar*` painted) holds for decorated
    /// tabs too, not just icon-less ones (#926).
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout_icons(
        &self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
    ) -> TabBarHits {
        // Issue #1003: no-paint twin of `draw_tab_bar_icons` — must
        // agree with what that method painted, so it measures against
        // `chrome_font` too. The old `current_font.is_none()` fallback
        // is gone: `chrome_font` is never `None` (seeded at
        // construction — see its field doc).
        let mut hits = super::tab_bar::mac_tab_bar_layout_icons(
            &self.chrome_font,
            rect.width as f64,
            bar,
            icons,
        );
        // Bar-relative → target-surface-absolute (issue #934), the same
        // shift `Self::draw_tab_bar_icons` applies to its own returned
        // hits after painting, so a caller that measures here and paints
        // there sees one agreed geometry.
        crate::backend::shift_tab_bar_hits(&mut hits, rect.x as f64);
        hits
    }

    /// Issue #919's `TabBarLayout`-returning counterpart to
    /// [`Self::tab_bar_layout`] above, routed through
    /// [`super::tab_bar::mac_tab_bar_native_layout_icons`] (via
    /// [`Self::resolve_tab_bar_layout_icons`] with an empty sidecar,
    /// #926) — see that function's doc for why macOS can't share one
    /// measurement path between its `TabBarHits` and `TabBarLayout`
    /// accessors.
    fn resolve_tab_bar_layout(&self, rect: Rect, bar: &TabBar) -> TabBarLayout {
        self.resolve_tab_bar_layout_icons(rect, bar, &[])
    }

    /// No-paint twin of [`Self::draw_tab_bar_icons_layout`], routed
    /// through [`super::tab_bar::mac_tab_bar_native_layout_icons`] with
    /// the same sidecar (#926) — so a consumer that measures with this
    /// and paints with `draw_tab_bar_icons` gets one agreed geometry.
    fn resolve_tab_bar_layout_icons(
        &self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
    ) -> TabBarLayout {
        // Issue #1003: no-paint twin of `draw_tab_bar_icons_layout` —
        // see `tab_bar_layout_icons`'s comment just above.
        super::tab_bar::mac_tab_bar_native_layout_icons(
            &self.chrome_font,
            rect.width as f64,
            rect.height as f64,
            bar,
            icons,
        )
    }

    fn activity_bar_layout(&self, rect: Rect, bar: &ActivityBar) -> Vec<ActivityBarRowHit> {
        // No-paint twin of `draw_activity_bar`; both walk the same
        // `row_plan`, so the returned bar-relative spans are exactly the
        // rows that were painted (quadraui#552).
        super::activity_bar::mac_activity_bar_layout(rect.width as f64, rect.height as f64, bar)
    }

    /// #810: shared cell-grid painting lives in
    /// [`crate::primitives::terminal::paint`] now — see that fn's doc
    /// for the divergences resolved while unifying
    /// `gtk::terminal::draw_terminal_cells`,
    /// `macos::terminal::draw_terminal_cells` and
    /// `win::terminal::draw_terminal_cells` into one implementation.
    fn draw_terminal(&mut self, rect: Rect, term: &Terminal) {
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;

        let sb_width = match &term.scrollbar {
            Some(sb) => sb.width.map(|w| w as f64).unwrap_or(8.0),
            None => 0.0,
        };
        let cell_area_w = (rect.width as f64 - sb_width).max(0.0);

        crate::primitives::terminal::paint(
            term,
            self,
            &theme,
            rect.x,
            rect.y,
            cell_area_w as f32,
            rect.height,
            line_height as f32,
            char_width as f32,
            None,
        );

        if let Some(ref sb_state) = term.scrollbar {
            let sb = crate::primitives::scrollbar::Scrollbar::vertical(
                term.id.clone(),
                Rect::new(
                    rect.x + cell_area_w as f32,
                    rect.y,
                    sb_width as f32,
                    rect.height,
                ),
                sb_state.effective_scroll_offset() as f32,
                sb_state.total_lines as f32,
                sb_state.visible_lines as f32,
                line_height as f32,
            );
            crate::primitives::scrollbar::native_surface_paint::paint(&sb, self, &theme);
        }
    }
    /// #810: shared divider painting lives in
    /// [`crate::primitives::terminal::paint_divider`] now.
    fn draw_terminal_divider(&mut self, rect: Rect) {
        let theme = self.current_theme;
        crate::primitives::terminal::paint_divider(self, rect.x, rect.y, rect.height, &theme);
    }
    /// #996: see `Backend::draw_solid_fill`'s doc — a pixel backend can
    /// honor `rect` exactly via `NativeSurface::surface_fill_rect`.
    fn draw_solid_fill(&mut self, rect: Rect, color: Color) {
        self.surface_fill_rect(rect, color);
        // #492: chrome-only paint (no text of its own) — see
        // `TuiBackend::draw_solid_fill`'s identical registration for why.
        self.register_zone(WidgetId::new("chrome:solid-fill"), rect);
    }
    /// #810: shared text-display painting lives in
    /// [`crate::primitives::text_display::paint`] now — see that fn's
    /// doc for the divergence resolved while unifying
    /// `gtk::text_display::draw_text_display`,
    /// `macos::text_display::draw_text_display` and
    /// `win::text_display::draw_text_display` into one implementation.
    fn draw_text_display(&mut self, rect: Rect, td: &TextDisplay) {
        let theme = self.current_theme;
        let line_height = self.current_line_height as f32;
        let char_width = self.current_char_width as f32;
        crate::primitives::text_display::paint(td, rect, self, &theme, line_height, char_width);
    }
    fn draw_command_line(&mut self, rect: Rect, cmd: &CommandLine) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_command_line called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_command_line requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::command_line::draw_command_line(
                ctx,
                font,
                cmd,
                &theme,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                line_height,
            );
        }
    }
    /// No visual highlight yet — issue #1001 scoped the paint work to
    /// GTK/Cairo and TUI/ratatui (`macos::command_line::draw_command_line`
    /// has no selection-aware paint path of its own). Delegates to the
    /// plain paint so callers get correct (if unhighlighted) text instead
    /// of a missing trait impl; tracked as a follow-up for the macOS
    /// rasteriser alongside its other selection-highlight primitives.
    fn draw_command_line_selection(
        &mut self,
        rect: Rect,
        cmd: &CommandLine,
        _selection: Option<(usize, usize)>,
    ) {
        self.draw_command_line(rect, cmd);
    }
    fn command_line_layout(
        &self,
        rect: Rect,
        cmd: &CommandLine,
    ) -> crate::primitives::command_line::CommandLineLayout {
        cmd.layout(
            rect,
            crate::primitives::command_line::CommandLineMeasure::from_metrics(&self.measure()),
        )
    }
    fn text_display_layout(&self, rect: Rect, td: &TextDisplay) -> TextDisplayLayout {
        super::text_display::mac_text_display_layout(
            td,
            rect,
            self.current_line_height,
            self.current_char_width,
        )
    }
    fn draw_text_input(
        &mut self,
        rect: Rect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        // macOS TextInput rasteriser: future work. Return layout only.
        ti.layout(
            rect,
            crate::primitives::text_input::TextInputMeasure::from_metrics(&self.measure()),
        )
    }
    fn text_input_layout(
        &self,
        rect: Rect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        ti.layout(
            rect,
            crate::primitives::text_input::TextInputMeasure::from_metrics(&self.measure()),
        )
    }
    fn draw_tooltip(&mut self, tooltip: &Tooltip, layout: &TooltipLayout) {
        self.draw_tooltip_with_chrome(tooltip, layout, &crate::TooltipChrome::default());
    }

    fn draw_tooltip_with_chrome(
        &mut self,
        tooltip: &Tooltip,
        layout: &TooltipLayout,
        chrome: &crate::TooltipChrome,
    ) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tooltip called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_tooltip requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::tooltip::draw_tooltip_with_chrome(
                ctx,
                font,
                tooltip,
                layout,
                chrome,
                line_height,
                char_width,
                &theme,
            );
        }
    }
    fn draw_context_menu(
        &mut self,
        menu: &ContextMenu,
        layout: &ContextMenuLayout,
    ) -> Vec<(Rect, WidgetId)> {
        // #455: see draw_palette for why this happens before the CG borrow.
        self.modal_stack.borrow_mut().mark_painted(&menu.id);
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_context_menu called outside enter_frame_scope",
        );
        // Issue #1003: the context menu is chrome
        // (`ChromePrimitive::ContextMenu`) — see `draw_tree`'s comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::context_menu::draw_context_menu(ctx, font, menu, layout, &theme) }
    }
    fn draw_dialog(&mut self, dialog: &Dialog, layout: &DialogLayout) -> Vec<Rect> {
        // #455: see draw_palette for why this happens before the CG borrow.
        self.modal_stack.borrow_mut().mark_painted(&dialog.id);
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_dialog called outside enter_frame_scope",
        );
        // Issue #1003: the dialog chrome (title, buttons) is
        // `ChromePrimitive::Dialog` — see `draw_tree`'s comment for the
        // font swap and why `line_height` stays the editor pitch.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::dialog::draw_dialog(ctx, font, dialog, layout, line_height, &theme) }
    }
    fn draw_multi_section_view(&mut self, rect: Rect, view: &MultiSectionView) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_multi_section_view called outside enter_frame_scope",
        );
        // Issue #1003: section headers and Tree/List body rows are
        // sidebar chrome (`ChromePrimitive::MultiSectionView`), not
        // editor content — mirrors `GtkBackend::draw_multi_section_view`'s
        // #416 font swap, which likewise only swaps the *font* here and
        // leaves `line_height`/`char_width` (below) reading the editor
        // values, since embedded `SectionBody::Chart`/editor-adjacent
        // content still measures against those.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        let caret_visible = self.caret_visible.get();
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::multi_section_view::draw_multi_section_view(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                view,
                &theme,
                line_height,
                char_width,
                caret_visible,
            )
        }
    }
    fn msv_layout(&self, rect: Rect, view: &MultiSectionView) -> MultiSectionViewLayout {
        super::multi_section_view::mac_msv_layout(view, rect, self.current_line_height)
    }
    fn msv_metrics(&self) -> MsvLayoutMetrics {
        super::multi_section_view::mac_msv_metrics(self.current_line_height, false)
    }
    fn tree_layout(&self, rect: Rect, tree: &TreeView) -> TreeViewLayout {
        // No font-role change here (issue #1003) — see `list_layout`'s
        // comment above; `mac_tree_layout` only takes pitch, and
        // `GtkBackend::tree_layout` passes its own editor
        // `current_line_height` too.
        super::tree::mac_tree_layout(tree, rect, self.current_line_height)
    }
    fn tree_vscrollbar(&self, rect: Rect, tree: &TreeView) -> Option<crate::Scrollbar> {
        // macOS tree vertical-scrollbar rasteriser not yet implemented
        // (#1043). Delegate to the primitive's geometry method, but
        // using the same per-row pitch `Self::tree_layout` (via
        // `mac_tree_layout`/`layout_metrics::tree_layout`) actually
        // paints — not the raw `line_height`. Unlike `ListView` (whose
        // items really are `line_height` tall, see
        // `MacBackend::list_vscrollbar`), `TreeView` rows pitch at
        // `line_height * 1.4` (or `TreeStyle::row_height`); see
        // `layout_metrics::tree_row_pitch`'s doc.
        let row_h =
            crate::primitives::layout_metrics::tree_row_pitch(tree, self.current_line_height);
        tree.vscrollbar(rect, row_h as f32)
    }
    fn form_layout(&self, rect: Rect, form: &Form) -> FormLayout {
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::form_layout requires set_current_font");
        super::form::mac_form_layout(form, rect, self.current_line_height, font)
    }
    fn draw_editor(&mut self, _rect: Rect, editor: &Editor) -> EditorPaintResult {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_editor called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_editor requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::editor::draw_editor(ctx, font, editor, &theme, char_width, line_height) }
    }
    fn draw_message_list(&mut self, rect: Rect, list: &MessageList) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_message_list called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_message_list requires set_current_font");
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::message_list::draw_message_list(
                ctx,
                font,
                list,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                (rect.y + rect.height) as f64,
                line_height,
            );
        }
    }
    fn draw_rich_text_popup(&mut self, popup: &RichTextPopup, layout: &RichTextPopupLayout) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_rich_text_popup called outside enter_frame_scope",
        );
        // Issue #1003: the rich-text popup is chrome
        // (`ChromePrimitive::RichTextPopup`) — see `draw_tree`'s comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::rich_text_popup::draw_rich_text_popup(ctx, font, popup, layout, &theme) }
    }
    fn draw_find_replace(&mut self, _rect: Rect, panel: &FindReplacePanel) {
        debug_assert!(
            !self.current_cg().is_null(),
            "MacBackend::draw_find_replace called outside enter_frame_scope",
        );
        debug_assert!(
            self.current_font.is_some(),
            "MacBackend::draw_find_replace requires set_current_font",
        );
        let theme = self.current_theme;
        crate::primitives::find_replace::paint(panel, self, &theme);
    }
    fn draw_completions(&mut self, completions: &Completions, layout: &CompletionsLayout) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_completions called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_completions requires set_current_font");
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::completions::draw_completions(ctx, font, completions, layout, &theme) }
    }
    fn draw_scrollbar(&mut self, _rect: Rect, scrollbar: &Scrollbar) {
        let theme = self.current_theme;
        crate::primitives::scrollbar::native_surface_paint::paint(scrollbar, self, &theme);
    }

    fn draw_drop_overlay(&mut self, overlay: &crate::primitives::drop_zone::DropOverlay) {
        let theme = self.current_theme;
        crate::primitives::drop_zone::native_surface_paint::paint(overlay, self, &theme);
    }
    fn draw_menu_bar(&mut self, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_menu_bar called outside enter_frame_scope",
        );
        // Issue #1003: the menu bar is chrome (`ChromePrimitive::MenuBar`)
        // — see `draw_tree`'s comment for why this reads `chrome_font`
        // instead of `current_font`.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx non-null inside frame scope.
        unsafe {
            super::menu_bar::draw_menu_bar(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                bar,
                &theme,
            )
        }
    }
    fn menu_bar_layout(&self, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
        // Issue #1003: no-paint twin of `draw_menu_bar` — must agree
        // with what that method painted.
        let font = &self.chrome_font;
        super::menu_bar::mac_menu_bar_layout(
            font,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            bar,
        )
    }
    fn draw_split(&mut self, rect: Rect, split: &Split) -> SplitLayout {
        let layout = super::split::mac_split_layout(
            split,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        let theme = self.current_theme;
        crate::primitives::split::native_surface_paint::paint(&layout, self, &theme);
        layout
    }
    fn split_layout(&self, rect: Rect, split: &Split) -> SplitLayout {
        super::split::mac_split_layout(
            split,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    fn draw_split_tree(&mut self, rect: Rect, tree: &SplitTree) -> SplitTreeLayout {
        let theme = self.current_theme;
        let layout = super::split_tree::mac_split_tree_layout(
            tree,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        crate::primitives::split_tree::native_surface_paint::paint(&layout, self, &theme);
        layout
    }
    fn split_tree_layout(&self, rect: Rect, tree: &SplitTree) -> SplitTreeLayout {
        super::split_tree::mac_split_tree_layout(
            tree,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    fn draw_panel(&mut self, rect: Rect, panel: &Panel) -> PanelLayout {
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let layout = super::panel::mac_panel_layout(
            panel,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            line_height,
        );
        crate::primitives::panel::native_surface_paint::paint(panel, &layout, self, &theme);
        layout
    }
    fn panel_layout(&self, rect: Rect, panel: &Panel) -> PanelLayout {
        super::panel::mac_panel_layout(
            panel,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
        )
    }
    fn draw_toast_stack(&mut self, rect: Rect, stack: &ToastStack) -> ToastStackLayout {
        // `NativeSurface::surface_fill_rect`/`surface_draw_text_run` (etc)
        // each debug_assert/expect their own frame + font internally —
        // see `Self::surface_fill_rect`/`Self::surface_measure_text` — so
        // this method needs no separate ctx/font fetch of its own,
        // matching `Self::draw_status_bar`'s #860 shape.
        let theme = self.current_theme;
        let line_height = self.current_line_height as f32;
        crate::primitives::toast::native_surface_paint::paint(
            stack,
            self,
            &theme,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            line_height,
        )
    }
    fn toast_stack_layout(&self, rect: Rect, stack: &ToastStack) -> ToastStackLayout {
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::toast_stack_layout requires set_current_font");
        super::toast::mac_toast_stack_layout(
            stack,
            font,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            self.current_line_height,
        )
    }
    fn draw_pipeline_view(
        &mut self,
        rect: Rect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_pipeline_view called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_pipeline_view requires set_current_font");
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::pipeline_view::draw_pipeline_view(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                view,
                &theme,
            )
        }
    }
    fn pipeline_view_layout(
        &self,
        rect: Rect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        super::pipeline_view::mac_pipeline_view_layout(
            view,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    fn draw_progress(&mut self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_progress called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_progress requires set_current_font");
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::progress::draw_progress(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                bar,
                &theme,
            )
        }
    }
    fn progress_layout(&self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
        super::progress::mac_progress_layout(
            bar,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    fn draw_spinner(&mut self, rect: Rect, spinner: &Spinner) -> SpinnerLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_spinner called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_spinner requires set_current_font");
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::spinner::draw_spinner(ctx, font, rect.x as f64, rect.y as f64, spinner, &theme)
        }
    }
    fn spinner_layout(&self, rect: Rect, spinner: &Spinner) -> SpinnerLayout {
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::spinner_layout requires set_current_font");
        super::spinner::mac_spinner_layout(spinner, font, rect.x as f64, rect.y as f64)
    }
    fn draw_command_center(&mut self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_command_center called outside enter_frame_scope",
        );
        // Issue #1003: the command centre's back/forward arrows and
        // search label are chrome (`ChromePrimitive::CommandCenter`) —
        // see `draw_tree`'s comment for the font swap and why
        // `line_height` (used only for vertical centring) stays the
        // editor pitch, matching `GtkBackend::draw_command_center`'s
        // #637 comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx non-null inside frame scope.
        unsafe {
            super::command_center::draw_command_center(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                cc,
                &theme,
                line_height,
            )
        }
    }
    fn command_center_layout(&self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout {
        // Issue #1003: no-paint twin of `draw_command_center` — must
        // agree with what that method painted.
        let font = &self.chrome_font;
        super::command_center::mac_command_center_layout(
            cc,
            font,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    /// #810: shared chart painting lives in
    /// [`crate::primitives::chart::paint`] now — see that fn's doc for
    /// the divergences resolved while unifying `gtk::chart::draw_chart`,
    /// `macos::chart::draw_chart` and `win::chart::draw_chart` into one
    /// implementation.
    fn draw_chart(
        &mut self,
        rect: Rect,
        chart: &Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) -> ChartLayout {
        let theme = self.current_theme;
        let layout = self.chart_layout(rect, chart);
        crate::primitives::chart::paint(chart, &layout, self, &theme, hovered_point, crosshair_x);
        layout
    }
    fn chart_layout(&self, rect: Rect, chart: &Chart) -> ChartLayout {
        super::chart::mac_chart_layout(
            chart,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
            self.current_char_width,
        )
    }

    fn draw_toolbar_interactive(
        &mut self,
        rect: Rect,
        bar: &crate::primitives::toolbar::Toolbar,
        interaction: &crate::interaction::InteractionState,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        let (hovered_id, pressed_id) = (interaction.hovered(), interaction.pressed());
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_toolbar called outside enter_frame_scope",
        );
        // Issue #1003: action labels and icon glyphs are chrome
        // (`ChromePrimitive::Toolbar`) — see `draw_tree`'s comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::toolbar::draw_toolbar(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                bar,
                &theme,
                hovered_id,
                pressed_id,
            )
        }
    }

    fn toolbar_layout(
        &self,
        rect: Rect,
        bar: &crate::primitives::toolbar::Toolbar,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        // Issue #1003: no-paint twin of `draw_toolbar_interactive` — must
        // agree with what that method painted, so it measures against
        // `chrome_font` too. The old `current_font.is_none()` fallback
        // (a `char_width`-only estimate) is gone: `chrome_font` is never
        // `None` (seeded at construction — see its field doc), so the
        // "no font installed yet" case this was guarding against cannot
        // happen for chrome.
        super::toolbar::mac_toolbar_layout(
            bar,
            &self.chrome_font,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }

    fn draw_sidebar_panel_interactive(
        &mut self,
        rect: Rect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
        interaction: &crate::interaction::InteractionState,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        let (hovered_toolbar_id, pressed_toolbar_id) =
            (interaction.hovered(), interaction.pressed());
        debug_assert!(
            !self.current_cg().is_null(),
            "MacBackend::draw_sidebar_panel called outside enter_frame_scope",
        );
        // Issue #1003: `SidebarPanel` composes a `Toolbar` header — its
        // icon glyphs are chrome (`ChromePrimitive::SidebarPanel`), not
        // editor content — so this paints through `ChromeSurface`
        // (routes text through `chrome_font`) rather than `self`
        // directly, the same switch `draw_status_bar_interactive` makes
        // (#963) and `GtkBackend::draw_sidebar_panel_interactive` makes
        // via its own font-swap (#416/#862). `line_height` stays the
        // editor pitch, matching `GtkBackend`'s own unchanged
        // `current_line_height` there.
        let theme = self.current_theme;
        let line_height = self.current_line_height as f32;
        let mut surface = ChromeSurface { backend: self };
        crate::primitives::sidebar_panel::native_surface_paint::paint(
            panel,
            &mut surface,
            &theme,
            rect,
            line_height,
            hovered_toolbar_id,
            pressed_toolbar_id,
        )
    }

    /// #866: paint via the shared
    /// [`crate::primitives::diff_view::native_surface_paint::paint`] —
    /// see that fn's doc for the two named divergences (row/header text
    /// vertical alignment; header-label ellipsize vs. hard-clip) found
    /// while unifying `gtk::diff_view::draw_diff_view`,
    /// `macos::diff_view::draw_diff_view` and
    /// `win::diff_view::draw_diff_view` into one implementation.
    fn draw_diff_view(
        &mut self,
        rect: Rect,
        view: &crate::primitives::diff_view::DiffView,
    ) -> crate::primitives::diff_view::DiffViewLayout {
        // `NativeSurface::surface_fill_rect`/`surface_draw_text_run` (etc)
        // each debug_assert their own `!ctx.is_null()` internally — see
        // `Self::surface_fill_rect` — so this method needs no separate
        // ctx/font fetch of its own, matching `Self::draw_status_bar`'s
        // #860 shape.
        let theme = self.current_theme;
        let line_height = self.current_line_height as f32;
        crate::primitives::diff_view::native_surface_paint::paint(
            view,
            self,
            &theme,
            rect,
            line_height,
        )
    }

    fn sidebar_panel_layout(
        &self,
        rect: Rect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        // Issue #1003: no-paint twin of `draw_sidebar_panel_interactive`
        // — must agree with what that method painted, so its header
        // toolbar measures against `chrome_font` too. The old
        // `current_font.is_none()` fallback is gone: `chrome_font` is
        // never `None` (seeded at construction — see its field doc), so
        // the "no font installed yet" case it guarded against cannot
        // happen for chrome. `line_height` stays the editor pitch,
        // matching `draw_sidebar_panel_interactive` above.
        super::sidebar_panel::mac_sidebar_panel_layout(
            panel,
            &self.chrome_font,
            self.current_line_height,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }

    /// Override of the trait's no-op default (`Backend::draw_board`),
    /// which returns an empty `BoardLayout` and paints nothing.
    ///
    /// The compiler cannot report the missing override — that default is
    /// exactly why macOS silently painted an empty board — so this is
    /// implemented ahead of quadraui#600 removing the default. When #600
    /// lands, this method is already here and the lane does not regress.
    fn draw_board(&mut self, rect: Rect, model: &BoardModel) -> BoardLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_board called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_board requires set_current_font");
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::board::draw_board(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                model,
                &theme,
            )
        }
    }

    fn board_layout(&self, rect: Rect, model: &BoardModel) -> BoardLayout {
        super::board::mac_board_layout(
            model,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }

    /// #382 scoped the `Minimap` rasteriser to GTK (fixed-pitch colour
    /// blocks, #667) and TUI (braille) only — macOS was explicitly out of
    /// scope until this backend carries the rest of the editor chrome.
    /// #738 added the third rasteriser (Win-GUI) and, along with it, lifted
    /// the legibility/render-mode threshold and span-lookup helpers a
    /// macOS rasteriser would otherwise have had to reinvent
    /// ([`crate::primitives::minimap::is_legible`] /
    /// `render_mode` / `minimap_font_px` / `SpanCursor` / `color_at_column`
    /// / `truncate_to_columns`) — `super::minimap::draw_minimap` consumes
    /// those directly, same as `gtk::minimap` and `win::minimap` do (#961).
    ///
    /// Before #961 this was a reachable `todo!()`-turned-honest-no-paint —
    /// `super::minimap::mac_minimap_layout` always computed the real
    /// [`MinimapLayout`](crate::primitives::minimap::MinimapLayout) (the
    /// same `Minimap::layout_with_sizing` call GTK/Win-GUI make, not a
    /// stub), so hit-testing/click-routing already worked correctly; only
    /// the pixels were missing (#802). Now the Core Graphics/Core Text
    /// paint calls (fill rects, `CTLine` glyph runs, the clip bracket) —
    /// the same shape and size of backend-specific work #738 did for
    /// Win-GUI, not a shim over shared logic — actually run, and
    /// [`MinimapPaintResult::painted`](crate::backend::MinimapPaintResult::painted)
    /// reports `true`.
    fn draw_minimap(
        &mut self,
        rect: Rect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::backend::MinimapPaintResult {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_minimap called outside enter_frame_scope",
        );
        self.register_zone(minimap.id.clone(), rect);
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_minimap requires set_current_font");
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope (checked above).
        let layout = unsafe { super::minimap::draw_minimap(ctx, font, rect, minimap, &theme) };
        crate::backend::MinimapPaintResult {
            layout,
            painted: true,
        }
    }

    /// The no-paint layout query — same geometry `draw_minimap` computes
    /// internally, exposed standalone for click-routing callers that don't
    /// need a repaint. See [`Self::draw_minimap`]'s doc comment.
    fn minimap_layout(
        &self,
        rect: Rect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::primitives::minimap::MinimapLayout {
        super::minimap::mac_minimap_layout(minimap, rect)
    }

    /// #662 scoped the `Image` rasteriser to GTK only for its first
    /// pass, and #802 replaced macOS's reachable `todo!()` with an
    /// honest [`ImagePaintResult::Unsupported`] — a panic an app calling
    /// `Backend::draw_image` generically could hit on macOS alone. That
    /// made macOS the only backend painting nothing at all for an image
    /// (TUI at least paints `image.fallback_text`); #962 closes the gap
    /// with a real Core Graphics/ImageIO decode-and-paint
    /// (`super::image::draw_image`), the same shape and size of
    /// backend-specific work #739 did for Win-GUI. #1014 adds the decode
    /// cache — via `super::image::draw_image_cached`, the crate-internal
    /// sibling of the public `super::image::draw_image`, since this
    /// trait method's own signature (and thus its ability to call
    /// whichever free function it likes) isn't part of the downstream
    /// contract the way that public function's signature is (see
    /// `super::image`'s module docs).
    fn draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        self.register_zone(image.id.clone(), rect);
        let ctx = self.current_cg();
        let scale = self.viewport.scale;
        // Unlike `draw_minimap`, `draw_image` can legitimately return
        // `Unsupported` (a decode failure, or a zero-size `rect`) without
        // ever touching `ctx` — so the null-context guard lives inside
        // `super::image::draw_image_cached` itself, right before the
        // CoreGraphics calls that actually need it, rather than an
        // unconditional upfront `debug_assert!` here.
        //
        // SAFETY: `super::image::draw_image_cached` only dereferences
        // `ctx` after confirming it's non-null.
        unsafe { super::image::draw_image_cached(ctx, rect, image, &mut self.image_cache, scale) }
    }
}

/// macOS's `WindowControl` surface (issue #950), backed by exactly the
/// `NSWindow` methods the issue's own design note names:
/// `setTitle`/`setContentSize`/`setContentMinSize`/`setContentMaxSize`/
/// `setLevel(NSFloatingWindowLevel)`/`toggleFullScreen:`/`miniaturize:`/
/// `orderOut:`/`makeKeyAndOrderFront:`. Every method is real here — macOS
/// has no `GtkBackend`-style structural gap the way Wayland's missing
/// window-position/always-on-top protocols force on GTK.
///
/// **Coordinate note:** [`Self::bounds`]/[`Self::set_bounds`] hand back
/// and accept `NSWindow::frame()`'s raw AppKit screen coordinates —
/// **origin bottom-left, Y increasing upward** — unlike every other
/// in-tree backend's top-left/Y-down convention (GTK's `bounds` — itself
/// position-less on Wayland — and Win's `GetWindowRect`/`SetWindowPos`
/// both use top-left/Y-down screen space). This is a genuine AppKit
/// platform difference, not an oversight: flipping it would need the
/// *screen's* height (`NSScreen::frame()`, tracked separately per
/// display and changeable at runtime as displays are added/removed/
/// resized), and getting that flip wrong silently would be worse than
/// documenting the native convention plainly. Contrast
/// [`Self::titlebar_control_inset`], which *does* flip — but that flips
/// relative to the window's own content-view height, a value already at
/// hand for an unrelated reason, not the screen's.
impl WindowControl for MacBackend {
    fn set_title(&mut self, title: &str) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setTitle(&NSString::from_str(title));
        Ok(())
    }

    fn set_size(&mut self, width: f32, height: f32) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setContentSize(NSSize {
            width: width as f64,
            height: height as f64,
        });
        Ok(())
    }

    fn set_min_size(&mut self, width: f32, height: f32) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setContentMinSize(NSSize {
            width: width as f64,
            height: height as f64,
        });
        Ok(())
    }

    fn set_max_size(&mut self, width: f32, height: f32) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setContentMaxSize(NSSize {
            width: width as f64,
            height: height as f64,
        });
        Ok(())
    }

    /// `NSWindow::frame()` — see this `impl` block's own doc for the
    /// bottom-left-origin coordinate note.
    fn bounds(&self) -> ServiceResult<Rect> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        let frame = window.frame();
        Ok(Rect::new(
            frame.origin.x as f32,
            frame.origin.y as f32,
            frame.size.width as f32,
            frame.size.height as f32,
        ))
    }

    fn set_bounds(&mut self, bounds: Rect) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setFrame_display(
            NSRect {
                origin: NSPoint {
                    x: bounds.x as f64,
                    y: bounds.y as f64,
                },
                size: NSSize {
                    width: bounds.width as f64,
                    height: bounds.height as f64,
                },
            },
            true,
        );
        Ok(())
    }

    /// `NSWindow::center()` — unlike [`crate::win::backend::WinBackend`]'s
    /// `center`/[`crate::gtk::backend::GtkBackend`]'s `Unsupported`, this
    /// needs no manual monitor-geometry math: AppKit centers on whichever
    /// screen "most closely intersects [the window's] current position"
    /// (Apple's own doc for the call) in one round trip.
    fn center(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.center();
        Ok(())
    }

    /// `NSWindow::toggleFullScreen:` — unlike
    /// [`Backend::toggle_window_maximize`]'s `zoom:` (the
    /// double-click-to-maximize equivalent, see that method's doc for why
    /// it is deliberately *not* this call), this is the real AppKit
    /// fullscreen transition. Idempotent: reports the current state via
    /// `styleMask().contains(NSWindowStyleMask::FullScreen)` and only
    /// calls `toggleFullScreen:` when the requested state actually
    /// differs, so entering while already fullscreen (or exiting while
    /// not) is a no-op rather than toggling to the wrong state.
    fn set_fullscreen(&mut self, fullscreen: bool) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        let currently_fullscreen = window.styleMask().contains(NSWindowStyleMask::FullScreen);
        if currently_fullscreen != fullscreen {
            window.toggleFullScreen(None);
        }
        Ok(())
    }

    /// `NSWindow::isZoomed` (issue #1022) — AppKit's own maximize-state
    /// query, the same notion [`Backend::toggle_window_maximize`]'s
    /// `zoom:` call flips.
    fn is_maximized(&self) -> ServiceResult<bool> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        Ok(window.isZoomed())
    }

    /// `NSWindow::setLevel(NSFloatingWindowLevel)` — the one always-on-top
    /// mechanism among the four in-tree backends with no platform-level
    /// gap (contrast [`crate::gtk::backend::GtkBackend`]'s `Unsupported`
    /// on GTK4/Wayland).
    fn set_always_on_top(&mut self, on_top: bool) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setLevel(if on_top {
            NSFloatingWindowLevel
        } else {
            NSNormalWindowLevel
        });
        Ok(())
    }

    /// Toggles `NSWindowStyleMask::Titled` (issue #1022) — backs
    /// vimcode's client-side-decoration path (its #552). AppKit ties the
    /// titlebar, not a separate flag, to this style bit: removing it also
    /// removes the traffic-light buttons and title text, matching what a
    /// host that paints its own titlebar wants.
    fn set_decorated(&mut self, decorated: bool) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        let mut style = window.styleMask();
        if decorated {
            style.insert(NSWindowStyleMask::Titled);
        } else {
            style.remove(NSWindowStyleMask::Titled);
        }
        window.setStyleMask(style);
        Ok(())
    }

    fn minimize(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.miniaturize(None);
        Ok(())
    }

    /// Undoes both a minimize (`deminiaturize:`) and a maximize (`zoom:`,
    /// if currently zoomed) — same "restore from either state" posture as
    /// [`crate::gtk::backend::GtkBackend::restore`] (see that method's
    /// doc for the Electron-parity rationale).
    fn restore(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        if window.isMiniaturized() {
            window.deminiaturize(None);
        }
        if window.isZoomed() {
            window.zoom(None);
        }
        Ok(())
    }

    fn hide(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.orderOut(None);
        Ok(())
    }

    fn show(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.makeKeyAndOrderFront(None);
        Ok(())
    }

    fn focus(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.makeKeyAndOrderFront(None);
        Ok(())
    }
}

/// macOS's `TrayService` surface (issue #953), backed by
/// `NSStatusBar`/`NSStatusItem` — see [`super::tray`]'s module doc for
/// the full design (lazy item creation, the menu-vs-plain-click
/// trade-off, why `ContextMenuDismissed` doesn't fire for a tray menu).
/// Every method here just forwards to a free function in that module,
/// the same "thin impl on the struct, real logic in a sibling module"
/// shape `Backend::install_menu_bar`/`Backend::show_context_menu` already
/// use for `super::menu_bar_install`.
impl crate::backend::TrayService for MacBackend {
    fn set_icon(&mut self, icon: crate::primitives::image::ImageSource) -> ServiceResult<()> {
        super::tray::set_icon(&mut self.tray, self.events.clone(), icon)
    }

    fn set_tooltip(&mut self, tooltip: &str) -> ServiceResult<()> {
        super::tray::set_tooltip(&mut self.tray, self.events.clone(), tooltip)
    }

    fn set_menu(
        &mut self,
        menu: &crate::primitives::context_menu::ContextMenu,
    ) -> ServiceResult<()> {
        super::tray::set_menu(&mut self.tray, self.events.clone(), menu)
    }
}

// ─── NativeSurface (#807, Phase 1) ───────────────────────────────────────────
//
// The ~15-verb drawing surface underneath `Backend::draw_*`, extracted from
// helpers this backend already had privately — every macOS rasteriser module
// (`activity_bar.rs`, `tooltip.rs`, `board.rs`, ...) declares its own private
// copy of `color_to_cg`/`fill_rect`/`stroke_rect`/the `CGContext*` extern
// block; this impl is one canonical copy, scoped to this file, that Phase 2
// can point those ~30 call sites at instead of their own duplicate. Phase 1
// itself changes no behaviour: nothing calls through `NativeSurface` yet, so
// every existing rasteriser keeps using its own private copy untouched.
// See `native_surface`'s module doc for the full scope note and why these
// methods are `surface_`-prefixed instead of colliding with `Backend`'s.
/// Adapts `&mut MacBackend` to [`NativeSurface`], routing text
/// measurement and painting through `chrome_font`/`chrome_line_height`/
/// `chrome_char_width` instead of the `current_font`/`current_line_height`/
/// `current_char_width` `MacBackend`'s own `NativeSurface` impl (below)
/// uses — that impl is the shared choke point every other `self`-as-surface
/// primitive (`draw_form`, `draw_chart`, `draw_scrollbar`, …) still paints
/// through, so it has to keep serving `current_font`; this adapter exists
/// precisely so chrome primitives don't have to (issue #963 — see
/// `chrome_font`'s field doc for why the two must stay separate, and #912
/// for what goes wrong when they don't).
///
/// Every font-agnostic method (fills, strokes, clip, lines, images, frame
/// lifecycle) forwards straight through to `MacBackend`'s own impl, which
/// doesn't touch the font either way — only the three text-shaped methods
/// below actually differ.
///
/// [`Backend::draw_status_bar_interactive`] is the first call site; wiring
/// the rest of `super`'s chrome rasterisers (tab bar, tree, menu bar,
/// dialogs, …) off this same adapter is tracked follow-up — the
/// `ACCEPTED_DEFAULTS` entries this issue removes only gated on
/// `set_ui_font`/`set_editor_font` no longer being no-ops, not on every
/// chrome rasteriser having migrated yet (mirrors the scope Win-GUI's
/// #724 `chrome_dwrite` shipped with).
struct ChromeSurface<'a> {
    backend: &'a mut MacBackend,
}

impl NativeSurface for ChromeSurface<'_> {
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
        self.backend.chrome_line_height as f32
    }

    fn surface_char_width(&self) -> f32 {
        self.backend.chrome_char_width as f32
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let (w, h) = super::text::measure_text(&self.backend.chrome_font, text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        self.backend.surface_fill_rect(rect, color)
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        self.backend.surface_stroke_rect(rect, color, stroke_width)
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let ctx = self.backend.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "ChromeSurface::surface_draw_text_run called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text(
                ctx,
                &self.backend.chrome_font,
                text,
                rect.x as f64,
                rect.y as f64,
                ns_color_to_cg(color),
            );
        }
    }

    /// The chrome twin of `MacBackend`'s own #810 override — same
    /// `bold`/`italic`/`underline`-unsupported posture, same `scale_x`
    /// support via [`super::text::draw_text_scaled_x`], just against
    /// `chrome_font`.
    ///
    /// Not optional: this is the method the shared status-bar paint
    /// actually calls. Without it the adapter would inherit the trait
    /// default (forward to [`Self::surface_draw_text_run`]) — right font,
    /// but `scale_x` silently dropped, a divergence from the editor path
    /// that would only surface the day a chrome primitive uses it.
    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let _ = (bold, italic, underline);
        let ctx = self.backend.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "ChromeSurface::surface_draw_text_run_styled called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text_scaled_x(
                ctx,
                &self.backend.chrome_font,
                text,
                rect.x as f64,
                rect.y as f64,
                scale_x as f64,
                ns_color_to_cg(color),
            );
        }
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
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

impl NativeSurface for MacBackend {
    fn surface_begin_frame(&mut self, viewport: Viewport) {
        Backend::begin_frame(self, viewport);
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

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::surface_measure_text requires set_current_font");
        let (w, h) = super::text::measure_text(font, text);
        (w as f32, h as f32)
    }

    // #860: no `surface_measure_text_styled` override — the trait
    // default (ignore `bold`, forward here) already reproduces this
    // backend's pre-#860 `status_bar` rasteriser exactly: it measured
    // (and rendered) every segment at the same weight regardless of
    // `bold` (see `macos::status_bar`'s module doc, "Bold segments").

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_fill_rect called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { ns_fill_rect(ctx, rect, color) };
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_stroke_rect called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { ns_stroke_rect(ctx, rect, color, stroke_width as f64) };
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_draw_text_run called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::surface_draw_text_run requires set_current_font");
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text(
                ctx,
                font,
                text,
                rect.x as f64,
                rect.y as f64,
                ns_color_to_cg(color),
            );
        }
    }

    /// #810: overrides the default (which drops both styling and
    /// scale) for `scale_x` only — `bold`/`italic`/`underline` stay
    /// unsupported (this backend's own pre-#810 documented "not
    /// rendered yet" posture; see `macos::terminal`'s former module
    /// doc), but `draw_text_scaled_x` already existed here for the
    /// wide-glyph advance fix (#500/#703), so that capability isn't
    /// lost by routing through this trait.
    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let _ = (bold, italic, underline);
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_draw_text_run_styled called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::surface_draw_text_run_styled requires set_current_font");
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text_scaled_x(
                ctx,
                font,
                text,
                rect.x as f64,
                rect.y as f64,
                scale_x as f64,
                ns_color_to_cg(color),
            );
        }
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_draw_line called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            ns_draw_line(
                ctx,
                from.x as f64,
                from.y as f64,
                to.x as f64,
                to.y as f64,
                color,
                stroke_width as f64,
            );
        }
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_push_clip called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope. Every push here
        // must be balanced by a `surface_pop_clip` call — see that method.
        unsafe { ns_push_clip(ctx, rect) };
    }

    fn surface_pop_clip(&mut self) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_pop_clip called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { CGContextRestoreGState(ctx) };
    }

    fn surface_draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        Backend::draw_image(self, rect, image)
    }
}

/// `Color` (0-255 per channel) to CoreGraphics' 0.0-1.0 RGBA tuple —
/// identical math to every per-rasteriser private `color_to_cg` this
/// canonicalises (see this section's module comment).
pub(crate) fn ns_color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

fn ns_cg_rect(rect: Rect) -> CGRect {
    CGRect::new(
        &CGPoint::new(rect.x as f64, rect.y as f64),
        &CGSize::new(rect.width as f64, rect.height as f64),
    )
}

/// # Safety
/// `ctx` must be a valid, non-null `CGContextRef` borrowed for the
/// duration of this call.
pub(crate) unsafe fn ns_fill_rect(ctx: CGContextRef, rect: Rect, c: Color) {
    let (r, g, b, a) = ns_color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, ns_cg_rect(rect));
}

/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_stroke_rect(ctx: CGContextRef, rect: Rect, c: Color, line_width: f64) {
    let (r, g, b, a) = ns_color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    CGContextStrokeRect(ctx, ns_cg_rect(rect));
}

/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_draw_line(
    ctx: CGContextRef,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    c: Color,
    line_width: f64,
) {
    let (r, g, b, a) = ns_color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    CGContextMoveToPoint(ctx, x0, y0);
    CGContextAddLineToPoint(ctx, x1, y1);
    CGContextStrokePath(ctx);
}

/// Push an axis-aligned clip rect by saving graphics state then clipping —
/// CoreGraphics has no clip-only push/pop pair, so (as every macOS
/// rasteriser that clips already does — e.g. `macos::board`'s column/card
/// clipping) this piggybacks on the save/restore GState stack:
/// `MacBackend`'s `NativeSurface::surface_pop_clip` impl calls
/// `CGContextRestoreGState` directly to match.
///
/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_push_clip(ctx: CGContextRef, rect: Rect) {
    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, ns_cg_rect(rect));
}

/// Pop a clip pushed by [`ns_push_clip`]. `pub(crate)` alongside it so a
/// caller with only a raw `CGContextRef` (no live `MacBackend`) — e.g.
/// [`crate::primitives::multi_section_view`]'s embedded-`Form` section
/// body — can balance its own `ns_push_clip` call the same way
/// `MacBackend`'s `NativeSurface::surface_pop_clip` impl does.
///
/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_pop_clip(ctx: CGContextRef) {
    CGContextRestoreGState(ctx);
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextTranslateCTM(c: CGContextRef, tx: CGFloat, ty: CGFloat);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: CGFloat,
        green: CGFloat,
        blue: CGFloat,
        alpha: CGFloat,
    );
    fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: CGFloat,
        green: CGFloat,
        blue: CGFloat,
        alpha: CGFloat,
    );
    fn CGContextSetLineWidth(c: CGContextRef, width: CGFloat);
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextStrokeRect(c: CGContextRef, rect: CGRect);
    fn CGContextMoveToPoint(c: CGContextRef, x: CGFloat, y: CGFloat);
    fn CGContextAddLineToPoint(c: CGContextRef, x: CGFloat, y: CGFloat);
    fn CGContextStrokePath(c: CGContextRef);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accelerator::{Accelerator, AcceleratorScope};
    use crate::event::Point;
    use crate::types::Modifiers;
    use crate::KeyBinding;

    fn acc(id: &str, key: &str) -> Accelerator {
        Accelerator {
            id: AcceleratorId::new(id),
            binding: KeyBinding::Literal(key.to_string()),
            scope: AcceleratorScope::Global,
            label: None,
        }
    }

    #[test]
    fn new_starts_with_default_viewport() {
        let b = MacBackend::new();
        let v = b.viewport();
        assert_eq!(v.width, 0.0);
        assert_eq!(v.height, 0.0);
        assert_eq!(v.scale, 1.0);
    }

    #[test]
    fn begin_frame_updates_viewport() {
        let mut b = MacBackend::new();
        b.begin_frame(Viewport::new(800.0, 600.0, 2.0));
        let v = b.viewport();
        assert_eq!(v.width, 800.0);
        assert_eq!(v.height, 600.0);
        assert_eq!(v.scale, 2.0);
    }

    /// quadraui#699: `MacBackend::modal_stack_handle` must hand back a
    /// handle that shares state with the backend's own modal stack —
    /// this is the whole point of the issue: a host holding only
    /// `&mut dyn Backend` needs this to work identically on macOS to
    /// how it already works on `GtkBackend`
    /// (`gtk::backend::tests::gtk_backend_modal_stack_handle_shares_state`)
    /// and `TuiBackend`
    /// (`tui::backend::tests::tui_backend_modal_stack_handle_shares_state`).
    #[test]
    fn mac_backend_modal_stack_handle_shares_state() {
        let backend = MacBackend::new();
        let h1 = backend.modal_stack_handle();
        let h2 = backend.modal_stack_handle();
        h1.borrow_mut().push(
            crate::types::WidgetId::new("test:popup"),
            crate::event::Rect::new(0.0, 0.0, 10.0, 5.0),
        );
        assert_eq!(h2.borrow().len(), 1);
    }

    /// Regression test for #930: `install_menu_bar` used to
    /// `.expect()` a `MainThreadMarker`, which panics on any thread
    /// other than the real OS main thread. `MacBackend` itself is
    /// `!Send` (it carries `Rc<RefCell<_>>` state), so this builds the
    /// backend *inside* the spawned thread rather than moving one in —
    /// what matters is that `install_menu_bar` runs somewhere that is
    /// provably not the main thread, which every `std::thread::spawn`
    /// worker satisfies.
    ///
    /// Before the fix this `.join()` would return `Err` (the spawned
    /// thread panicked). After the fix it degrades exactly like its
    /// sibling helpers in `menu_bar_install.rs`: a silent no-op, no
    /// `menu_target` retained.
    #[test]
    fn install_menu_bar_off_main_thread_does_not_panic() {
        let bar = crate::primitives::menu_bar::MenuBar {
            id: WidgetId::new("menubar"),
            items: vec![],
            open_item: None,
            focused_item: None,
        };
        let handle = std::thread::spawn(move || {
            let mut backend = MacBackend::new();
            backend.install_menu_bar(&bar);
            backend.menu_target.is_none()
        });
        let no_target_installed = handle
            .join()
            .expect("install_menu_bar must return, not panic, off the main thread (#930)");
        assert!(
            no_target_installed,
            "off the main thread there is no MainThreadMarker, so install_menu_bar must be a \
             no-op that leaves menu_target unset"
        );
    }

    /// Same shape as `install_menu_bar_off_main_thread_does_not_panic`
    /// above, for `show_context_menu` (#930 calls this out as the
    /// sibling that needed the identical fix).
    #[test]
    fn show_context_menu_off_main_thread_does_not_panic() {
        let menu = crate::primitives::context_menu::ContextMenu {
            id: WidgetId::new("ctx"),
            items: vec![],
            selected_idx: 0,
            bg: None,
            placement: Default::default(),
        };
        let handle = std::thread::spawn(move || {
            let mut backend = MacBackend::new();
            backend.show_context_menu(&menu, Point::new(0.0, 0.0));
        });
        handle
            .join()
            .expect("show_context_menu must return, not panic, off the main thread (#930)");
    }

    /// quadraui#699: the stash-then-reuse pattern this issue exists to
    /// unblock — obtain the handle through `&mut dyn Backend`, drop
    /// that borrow, and use the handle afterwards from an unrelated
    /// borrow scope. Mirrors
    /// `tui::backend::tests::modal_stack_handle_outlives_the_backend_borrow_through_the_trait`.
    #[test]
    fn modal_stack_handle_outlives_the_backend_borrow_through_the_trait() {
        let mut backend = MacBackend::new();
        let stack_rc = {
            let dyn_backend: &mut dyn Backend = &mut backend;
            dyn_backend.modal_stack_handle()
        };
        backend.begin_frame(Viewport::new(80.0, 24.0, 1.0));
        stack_rc.borrow_mut().push(
            crate::types::WidgetId::new("test:popup"),
            crate::event::Rect::new(0.0, 0.0, 10.0, 5.0),
        );
        assert_eq!(backend.modal_stack_handle().borrow().len(), 1);
    }

    /// quadraui#699: same shared-state guarantee as
    /// `mac_backend_modal_stack_handle_shares_state`, for
    /// `drag_state_handle`.
    #[test]
    fn mac_backend_drag_state_handle_shares_state() {
        let backend = MacBackend::new();
        let h1 = backend.drag_state_handle();
        let h2 = backend.drag_state_handle();
        h1.borrow_mut()
            .begin(crate::dispatch::DragTarget::TextSelection {
                region: crate::types::WidgetId::new("r"),
                anchor: Point::new(0.0, 0.0),
            });
        assert!(h2.borrow().is_active());
    }

    #[test]
    fn services_platform_name_is_macos() {
        let b = MacBackend::new();
        assert_eq!(b.services().platform_name(), "macos");
    }

    #[test]
    fn line_height_and_char_width_seed_to_defaults() {
        let b = MacBackend::new();
        assert_eq!(b.line_height(), 16.0);
        assert_eq!(b.char_width(), 8.0);
    }

    #[test]
    fn register_and_unregister_accelerator_round_trip() {
        let mut b = MacBackend::new();
        let a = acc("save", "<C-s>");
        b.register_accelerator(&a);
        assert!(b.accelerators.contains_key(&AcceleratorId::new("save")));
        b.unregister_accelerator(&AcceleratorId::new("save"));
        assert!(!b.accelerators.contains_key(&AcceleratorId::new("save")));
    }

    // ── Accelerator matching (#486) ──────────────────────────────────

    #[test]
    fn match_keypress_finds_registered_global_binding() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("save", "<D-s>"));
        let id = b.match_keypress(
            &crate::Key::Char('s'),
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        );
        assert_eq!(id, Some(AcceleratorId::new("save")));
    }

    #[test]
    fn match_keypress_modifier_mismatch_no_match() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("save", "<D-s>"));
        // Same key, wrong modifiers (Ctrl instead of Cmd).
        let id = b.match_keypress(
            &crate::Key::Char('s'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert_eq!(id, None);
    }

    #[test]
    fn match_keypress_skips_non_global_scope() {
        let mut b = MacBackend::new();
        b.register_accelerator(&Accelerator {
            id: AcceleratorId::new("find-in-tree"),
            binding: KeyBinding::Literal("<D-f>".to_string()),
            scope: AcceleratorScope::Mode("tree".into()),
            label: None,
        });
        let id = b.match_keypress(
            &crate::Key::Char('f'),
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        );
        assert_eq!(id, None, "non-Global scope must not match here");
    }

    #[test]
    fn match_keypress_unregister_removes_match() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("save", "<D-s>"));
        b.unregister_accelerator(&AcceleratorId::new("save"));
        let id = b.match_keypress(
            &crate::Key::Char('s'),
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        );
        assert_eq!(id, None);
    }

    #[test]
    fn match_keypress_re_register_replaces_binding() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("save", "<D-s>"));
        b.register_accelerator(&acc("save", "<D-S-s>")); // Cmd+Shift+S
        assert!(b
            .match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    cmd: true,
                    ..Default::default()
                }
            )
            .is_none());
        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    cmd: true,
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(AcceleratorId::new("save"))
        );
    }

    /// Regression test for the blocking review finding on this PR:
    /// `parse_binding` (shared with `TuiBackend`) resolves every
    /// universal `KeyBinding` variant to a **Ctrl** `ParsedBinding`
    /// regardless of platform. `accelerator_to_ns` (native menu path)
    /// and `render_binding` (display path) both already render these
    /// as **Cmd** on macOS, so a real Cmd+S keypress — the one the UI
    /// tells the user to press — must match a `KeyBinding::Save`
    /// registration.
    #[test]
    fn match_keypress_universal_binding_matches_native_cmd_not_ctrl() {
        let mut b = MacBackend::new();
        b.register_accelerator(&Accelerator {
            id: AcceleratorId::new("save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        });

        // The advertised shortcut (⌘S, matching `accelerator_to_ns` /
        // `render_binding`) must fire.
        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    cmd: true,
                    ..Default::default()
                },
            ),
            Some(AcceleratorId::new("save")),
        );

        // The raw literal Ctrl+S that `parse_binding` alone would
        // produce must NOT fire — that's not what's on screen and not
        // the native macOS idiom.
        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            None,
        );
    }

    /// `KeyBinding::Redo` parses to `<C-S-z>` (Ctrl+Shift+Z) — verifies
    /// the Ctrl→Cmd translation preserves the co-occurring Shift
    /// modifier instead of clobbering it, matching
    /// `accelerator_to_ns`'s `cmd() | shift()` for the same variant.
    #[test]
    fn match_keypress_universal_binding_preserves_shift_modifier() {
        let mut b = MacBackend::new();
        b.register_accelerator(&Accelerator {
            id: AcceleratorId::new("redo"),
            binding: KeyBinding::Redo,
            scope: AcceleratorScope::Global,
            label: None,
        });

        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('z'),
                Modifiers {
                    cmd: true,
                    shift: true,
                    ..Default::default()
                },
            ),
            Some(AcceleratorId::new("redo")),
        );
    }

    /// `KeyBinding::Literal` bindings are an app author's deliberate,
    /// exact choice (e.g. a literal Ctrl+S that coexists with the
    /// native Cmd+S) — `accelerator_to_ns`/`render_binding` don't
    /// rewrite literals either, so `match_keypress` must not.
    #[test]
    fn match_keypress_literal_binding_ctrl_is_not_translated_to_cmd() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("literal-ctrl-s", "<C-s>"));

        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            Some(AcceleratorId::new("literal-ctrl-s")),
        );
        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    cmd: true,
                    ..Default::default()
                },
            ),
            None,
        );
    }

    // ── Double-click folding (#486) ──────────────────────────────────

    fn mouse_down(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn fold_double_click_second_click_same_position_becomes_double_click() {
        let mut b = MacBackend::new();
        let first = b.fold_double_click(mouse_down(5.0, 3.0));
        assert!(matches!(first, UiEvent::MouseDown { .. }));

        let second = b.fold_double_click(mouse_down(5.0, 3.0));
        assert!(
            matches!(second, UiEvent::DoubleClick { .. }),
            "second click at the same position should fold to DoubleClick"
        );
    }

    #[test]
    fn fold_double_click_different_position_stays_mouse_down() {
        let mut b = MacBackend::new();
        let _ = b.fold_double_click(mouse_down(5.0, 3.0));
        let second = b.fold_double_click(mouse_down(50.0, 30.0));
        assert!(matches!(second, UiEvent::MouseDown { .. }));
    }

    #[test]
    fn fold_double_click_passes_non_mouse_down_events_through() {
        let mut b = MacBackend::new();
        let ev = b.fold_double_click(UiEvent::WindowFocused(true));
        assert_eq!(ev, UiEvent::WindowFocused(true));
    }

    #[test]
    fn poll_events_drains_queue_fifo() {
        let b = MacBackend::new();
        b.push_event(UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: Point::new(1.0, 2.0),
            modifiers: Modifiers::default(),
        });
        b.push_event(UiEvent::WindowFocused(true));
        // `poll_events` takes &mut so we re-acquire after `push_event`.
        let mut b = b;
        let evs = b.poll_events();
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0], UiEvent::MouseDown { .. }));
        assert!(matches!(evs[1], UiEvent::WindowFocused(true)));
        // Second drain yields nothing.
        assert!(b.poll_events().is_empty());
    }

    // ── Background-thread wake (issue #831) ─────────────────────────────
    //
    // `MacBackend::waker`'s `DispatchQueue::main().exec_async` half needs a
    // real, running `NSApplication` main-queue pump to observe — a plain
    // `cargo test` process never drains the main dispatch queue, so
    // asserting on `wake_callback` actually firing here would be
    // meaningless (unlike GTK's `MainContext::invoke`, GCD gives no
    // "no owner, run synchronously" fallback). That half is exercised by
    // `macos.yml`'s real `macos-latest` runner instead. What's
    // host-independent and testable here is the `UserEventQueue` plumbing
    // every platform's `waker()` shares.

    /// A payload pushed from a background thread through the closure
    /// `Backend::waker()` hands out lands in `poll_events()`'s output as a
    /// `UiEvent::User` — the same `Send + Sync` staging queue every
    /// backend's `waker()` shares (`crate::runtime::UserEventQueue`).
    #[test]
    fn waker_payload_reaches_poll_events_as_user_event() {
        let backend = MacBackend::new();
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

    /// Before `set_wake_callback` is ever called (a `MacBackend`
    /// constructed directly, never handed to `macos::run`), `waker()`
    /// must not panic — it silently has nothing to wake.
    #[test]
    fn waker_is_a_safe_no_op_before_wake_callback_is_installed() {
        let backend = MacBackend::new();
        let waker = Backend::waker(&backend);
        waker(crate::UserPayload::new(1_i32));
    }

    #[test]
    fn enter_frame_scope_saves_and_restores_ptr() {
        let mut b = MacBackend::new();
        assert!(b.current_cg().is_null());
        // Cast a dummy non-null integer to satisfy the pointer type
        // (never dereferenced — the scope wrapper just stashes + restores).
        let dummy: CGContextRef = 0x1 as CGContextRef;
        b.enter_frame_scope(dummy, |inner| {
            assert_eq!(inner.current_cg(), dummy);
        });
        assert!(b.current_cg().is_null());
    }

    #[test]
    fn line_height_picks_up_set_current_line_height_via_font_install() {
        // `set_current_font` flows through `font_metrics`, exercised
        // in `macos::text::tests`. Here we just assert the setter
        // path mutates `line_height` / `char_width` away from defaults.
        let mut b = MacBackend::new();
        let font = super::super::text::make_font("Menlo", 14.0).expect("Menlo installed");
        b.set_current_font(font);
        // 14pt Menlo's line_height is ~16.something — defaults are
        // (16.0, 8.0); both should be updated regardless.
        assert!(b.line_height() > 0.0);
        assert!(b.char_width() > 0.0);
        assert!(b.current_font.is_some());
    }

    /// Issue #934: `MacBackend` previously had no public counterpart to
    /// `GtkBackend::set_current_line_height` / `set_current_char_width` /
    /// `WinBackend::set_current_line_height` / `set_current_char_width` —
    /// a host computing chrome layout generically across backends had no
    /// way to force this backend's cached row-spacing metrics to agree
    /// with whatever value it used elsewhere. Confirms the new setters
    /// (a) actually override the values `Backend::line_height`/
    /// `char_width` return, taking priority over whatever
    /// `set_current_font` last derived, and (b) don't require a font to
    /// be installed at all — mirroring the other two backends' setters,
    /// which are plain field writes with no font dependency.
    ///
    /// This test would not even compile before this fix — there was no
    /// `set_current_line_height`/`set_current_char_width` method on
    /// `MacBackend` to call, so it stands as its own RED-verification.
    #[test]
    fn set_current_line_height_and_char_width_override_font_derived_defaults() {
        let mut b = MacBackend::new();
        let font = super::super::text::make_font("Menlo", 14.0).expect("Menlo installed");
        b.set_current_font(font);
        let font_derived_line_height = b.line_height();
        let font_derived_char_width = b.char_width();

        // Override with values a host's own (portable) measurement
        // produced — deliberately not equal to whatever Menlo-14pt
        // resolved to, so the assertions below can't pass by coincidence.
        let overridden_line_height = font_derived_line_height + 3.0;
        let overridden_char_width = font_derived_char_width + 1.5;
        b.set_current_line_height(overridden_line_height as f64);
        b.set_current_char_width(overridden_char_width as f64);

        assert_eq!(b.line_height(), overridden_line_height);
        assert_eq!(b.char_width(), overridden_char_width);

        // The override doesn't clear `current_font` — draw_* methods that
        // require a font (see e.g. `draw_tree`'s `.expect(...)`) must
        // keep working after a metrics override.
        assert!(b.current_font.is_some());
    }

    // ── Window chrome (CSD, #498) ────────────────────────────────────
    //
    // Every unit test runs with no `NSWindow` constructed (`self.window`
    // stays `None` — `set_window` is only ever called by `macos::run::run`)
    // and no `NSApplication` main-loop running, so these only cover the
    // no-window guard clauses: each method must short-circuit and return
    // `false` *before* touching any `objc2-app-kit` API, not attempt a
    // real AppKit call (which would need a live window server connection
    // this CI/test environment doesn't have) and hang or panic. Mirrors
    // `gtk::backend`'s equivalent `*_false_without_window` tests.

    #[test]
    fn begin_window_drag_false_without_window() {
        let mut b = MacBackend::new();
        assert!(!Backend::begin_window_drag(&mut b));
    }

    /// #498: also no-ops when a window is present but no press was ever
    /// stashed — there's no real `NSEvent` to hand
    /// `performWindowDragWithEvent:`. Can't construct a real `NSWindow`
    /// headlessly, so this exercises the guard against the private
    /// `pending_window_press` field directly, same rationale as GTK's
    /// analogous `armed_window_drag`-field test.
    #[test]
    fn begin_window_drag_false_without_pending_press() {
        let mut b = MacBackend::new();
        assert!(b.pending_window_press.take().is_none());
    }

    #[test]
    fn toggle_window_maximize_false_without_window() {
        let mut b = MacBackend::new();
        assert!(!Backend::toggle_window_maximize(&mut b));
    }

    /// #1022: `is_maximized`/`set_decorated` guard on `self.window` the
    /// same way every other `WindowControl` method here does — no real
    /// `NSWindow` exists in a headless unit test, so both must report
    /// `Unsupported` rather than panic.
    #[test]
    fn is_maximized_err_without_window() {
        let b = MacBackend::new();
        assert!(matches!(
            crate::backend::WindowControl::is_maximized(&b),
            Err(BackendError::Unsupported)
        ));
    }

    #[test]
    fn set_decorated_err_without_window() {
        let mut b = MacBackend::new();
        assert!(matches!(
            crate::backend::WindowControl::set_decorated(&mut b, false),
            Err(BackendError::Unsupported)
        ));
    }

    #[test]
    fn set_cursor_false_without_window() {
        let mut b = MacBackend::new();
        assert!(!Backend::set_cursor(&mut b, PointerShape::Default));
        assert!(!Backend::set_cursor(
            &mut b,
            PointerShape::Resize(ResizeEdge::North)
        ));
    }

    /// #947: same no-window guard as the three tests above —
    /// `titlebar_control_inset` must short-circuit to the trait's own
    /// `Rect::default()` before touching `standardWindowButton` at all.
    /// The live-window case (non-empty inset matching AppKit's actual
    /// traffic-light layout) can't be constructed headlessly in this test
    /// binary either, same rationale as `begin_window_drag`'s sibling
    /// comment above — covered by the operator-run smoke tier instead.
    #[test]
    fn titlebar_control_inset_default_without_window() {
        let b = MacBackend::new();
        assert_eq!(Backend::titlebar_control_inset(&b), Rect::default());
    }

    /// Regression test for the blocking review finding on this PR: the
    /// `PointerShape` → cursor mapping this backend introduced had zero
    /// test coverage, despite `desktop::all_pointer_shapes()` existing
    /// specifically to make an exhaustive mapping test trivial — mirrors
    /// `gtk::backend::pointer_shape_cursor_name_maps_every_variant`.
    ///
    /// Asserts against [`MacCursorKind`], **not** against vended
    /// `NSCursor` singletons. The first cut of this test built a
    /// `[Retained<NSCursor>; 9]` expectation array and compared
    /// `Retained::as_ptr` identity; that reds the `macos (build, test)`
    /// check, because libtest runs every `#[test]` on a spawned worker
    /// thread and `+[NSCursor arrowCursor]` is an AppKit call in a
    /// process that never created an `NSApplication` — the same hazard
    /// `macos::menu_bar_install`'s tests already gate behind
    /// `MainThreadMarker::new()`. It also asserted the wrong thing:
    /// "these factory methods return a shared instance" is Apple's
    /// invariant, not quadraui's. quadraui's invariant is *which* cursor
    /// each shape picks, which is exactly what `mac_cursor_kind` returns
    /// — so this version covers strictly more of what can regress here,
    /// deterministically, on every platform and every thread.
    /// [`mac_cursor_for_shape`]'s one-line kind → singleton dispatch is
    /// covered by `mac_cursor_for_shape_vends_the_kind_it_maps_to` below,
    /// on the one thread where it can run at all.
    #[test]
    fn mac_cursor_kind_maps_every_variant() {
        // One expected kind per `desktop::all_pointer_shapes()` entry, in
        // the same order (`Default`, then one `Resize(edge)` per
        // `desktop::ALL_RESIZE_EDGES`: N, S, E, W, NE, NW, SE, SW). The
        // four corner edges fall back to the plain arrow — see
        // `mac_cursor_kind`'s doc for why there's no public
        // diagonal-resize `NSCursor` to use instead.
        let expected = [
            MacCursorKind::Arrow,
            MacCursorKind::ResizeUpDown,
            MacCursorKind::ResizeUpDown,
            MacCursorKind::ResizeLeftRight,
            MacCursorKind::ResizeLeftRight,
            MacCursorKind::Arrow,
            MacCursorKind::Arrow,
            MacCursorKind::Arrow,
            MacCursorKind::Arrow,
        ];
        let shapes = crate::desktop::all_pointer_shapes();
        assert_eq!(
            shapes.len(),
            expected.len(),
            "every PointerShape must have an expected cursor kind — a new variant means a new \
             row here, not a silently-truncated zip"
        );

        for (shape, expected) in shapes.iter().zip(expected.iter()) {
            assert_eq!(
                mac_cursor_kind(*shape),
                *expected,
                "PointerShape {shape:?} did not map to the expected cursor kind",
            );
        }
    }

    /// The AppKit half of the mapping: each [`MacCursorKind`] vends the
    /// `NSCursor` singleton it names, and distinct kinds are distinct
    /// cursors (so a copy-paste slip in `mac_cursor_for_shape`'s dispatch
    /// — every arm returning `arrowCursor()` — is caught).
    ///
    /// Main-thread-gated, and therefore a no-op under a default
    /// `cargo test` run (libtest hands each test to a worker thread):
    /// vending AppKit objects off the main thread in a process with no
    /// `NSApplication` is the hazard this whole split exists to keep out
    /// of CI. Same guard, same reason, as
    /// `macos::menu_bar_install`'s `install_then_simulate_activation_pushes_event`.
    /// Runs for real under `cargo test -- --test-threads=1` on a macOS
    /// host; the mapping itself is covered unconditionally by
    /// `mac_cursor_kind_maps_every_variant` above.
    #[test]
    // `NSCursor::resizeUpDownCursor`/`resizeLeftRightCursor` are deprecated
    // (see `mac_cursor_for_shape`'s comment) — this test asserts against
    // the exact singletons that function still (deliberately) vends.
    #[allow(deprecated)]
    fn mac_cursor_for_shape_vends_the_kind_it_maps_to() {
        if objc2_foundation::MainThreadMarker::new().is_none() {
            return;
        }
        let arrow = mac_cursor_for_shape(PointerShape::Default);
        let up_down = mac_cursor_for_shape(PointerShape::Resize(ResizeEdge::North));
        let left_right = mac_cursor_for_shape(PointerShape::Resize(ResizeEdge::East));

        assert_eq!(
            Retained::as_ptr(&arrow),
            Retained::as_ptr(&NSCursor::arrowCursor())
        );
        assert_eq!(
            Retained::as_ptr(&up_down),
            Retained::as_ptr(&NSCursor::resizeUpDownCursor())
        );
        assert_eq!(
            Retained::as_ptr(&left_right),
            Retained::as_ptr(&NSCursor::resizeLeftRightCursor())
        );
        assert_ne!(Retained::as_ptr(&arrow), Retained::as_ptr(&up_down));
        assert_ne!(Retained::as_ptr(&arrow), Retained::as_ptr(&left_right));
        assert_ne!(Retained::as_ptr(&up_down), Retained::as_ptr(&left_right));
    }

    /// #498: `backend_caps` must stay honest — declaring `window_chrome`
    /// / `pointer_cursor` requires the corresponding methods actually be
    /// overridden (see `tests/conformance/caps.rs`'s
    /// `backends_declare_only_what_they_override`, which checks this
    /// mechanically from source; this just pins the two flags directly).
    #[test]
    fn backend_caps_declares_window_chrome_and_pointer_cursor() {
        let b = MacBackend::new();
        let caps = b.backend_caps();
        assert!(caps.window_chrome);
        assert!(caps.pointer_cursor);
    }

    /// quadraui#936 (RED-verify companion): before this issue's fix,
    /// `backend_caps().native_dialogs` was `false` (the stub's own doc
    /// comment said so). `native_dialogs` is `Unprovable` in
    /// `CAP_CONTRACTS` (see `backend_caps`'s doc comment above), so
    /// nothing else pins this mechanically — this direct assertion is
    /// the whole gate.
    #[test]
    fn backend_caps_declares_native_dialogs() {
        let b = MacBackend::new();
        assert!(b.backend_caps().native_dialogs);
    }

    /// Build a flat `ListView` whose widest row is `content_width` chars
    /// wide, scrolled to `h_scroll`, with `max_content_width` populated so
    /// it always overflows a narrow viewport. Mirrors
    /// `primitives::list::hscrollbar_tests::list`.
    fn bordered_overflow_list(content_width: usize, h_scroll: usize, bordered: bool) -> ListView {
        ListView {
            id: crate::types::WidgetId::new("l"),
            title: None,
            items: vec![crate::ListItem {
                text: crate::types::StyledText::plain("x".repeat(content_width)),
                detail: None,
                icon: None,
                decoration: crate::types::Decoration::default(),
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered,
            h_scroll,
            max_content_width: Some(content_width),
            show_v_scrollbar: false,
        }
    }

    /// #790: `MacBackend::list_hscrollbar` used to ignore `list.bordered`
    /// entirely, so a bordered list's horizontal scrollbar overpainted its
    /// own border and the thumb hit-rect was off by one cell at each end
    /// — the vertical sibling of this exact bug was fixed for GTK/macOS
    /// layout reservation in #712, but the horizontal `list_hscrollbar`
    /// geometry API was missed because GTK/Windows/macOS each carried
    /// their own copy of the track math. Both now delegate to
    /// [`crate::primitives::list::hscrollbar_track`], so this asserts the
    /// macOS column of the fix directly: track inset matches the shared
    /// helper (and GTK/Windows' `list_hscrollbar`, which already had this
    /// right) rather than the flat, un-inset track the old macOS copy
    /// produced.
    #[test]
    fn list_hscrollbar_insets_track_for_bordered_list() {
        let mut b = MacBackend::new();
        b.current_char_width = 8.0;
        b.current_line_height = 16.0;
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);

        let flat = bordered_overflow_list(40, 0, false);
        let flat_sb = b
            .list_hscrollbar(rect, &flat)
            .expect("overflowing content should yield a scrollbar");
        assert_eq!(flat_sb.track.x, 0.0);
        assert_eq!(flat_sb.track.width, 200.0);

        let bordered = bordered_overflow_list(40, 0, true);
        let bordered_sb = b
            .list_hscrollbar(rect, &bordered)
            .expect("overflowing content should yield a scrollbar");
        // Inset by one char width (8px) on each side, and the thumb
        // (contained within the track) must therefore sit strictly inside
        // the border rather than overpainting it.
        assert_eq!(bordered_sb.track.x, 8.0);
        assert_eq!(bordered_sb.track.width, 184.0);
        assert_eq!(bordered_sb.track.y, 100.0 - 2.0 * 16.0);
        assert!(bordered_sb.track.x >= rect.x + 8.0);
        assert!(bordered_sb.track.x + bordered_sb.track.width <= rect.x + rect.width - 8.0);
    }

    // ── #802/#961/#962: Minimap/Image must not panic on macOS, and both
    // must actually paint ────────────────────────────────────────────────
    //
    // Before #802, `MacBackend::draw_minimap`/`draw_image` were `todo!()`
    // — any `AppLogic` calling either generically through `&mut dyn
    // Backend` (exactly what `examples/common/minimap_app.rs` /
    // `image_app.rs` do, the same fixtures `tests/macos_example_driver.rs`
    // now drives) panicked and took the whole host down on macOS while
    // working fine on TUI/GTK/Win-GUI. #802 replaced that with an honest
    // `painted: false` no-op / `Unsupported` result. #961 replaced the
    // minimap no-op with a real Core Graphics/Core Text paint
    // (`super::minimap::draw_minimap`); #962 does the same for `draw_image`
    // via `super::image::draw_image` (Core Graphics + ImageIO). macOS is
    // no longer the only backend painting nothing for either primitive, so
    // `draw_minimap_paints_and_reports_painted` and
    // `draw_image_paints_and_reports_painted` below assert pixels actually
    // landed, not just "didn't panic" — the corrupt/missing-source cases
    // still assert the clean `Unsupported` degrade (`draw_image_from_*`
    // below).

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
        }
    }

    /// #961: `draw_minimap` must actually paint pixels + report
    /// `painted: true` — the positive replacement for #802's
    /// `draw_minimap_does_not_panic_and_reports_unpainted`, now that macOS
    /// has a real Core Graphics/Core Text minimap rasteriser
    /// (`super::minimap::draw_minimap`).
    #[test]
    fn draw_minimap_paints_and_reports_painted() {
        use super::super::headless::BitmapSurface;
        use super::super::text::make_font;

        const W: u32 = 100;
        const H: u32 = 200;

        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut b = MacBackend::new();
        b.set_current_font(make_font("Menlo", 12.0).expect("Menlo installed"));
        let minimap = sample_minimap();
        let rect = Rect::new(0.0, 0.0, 20.0, H as f32);

        let result = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            *result.borrow_mut() = Some(backend.draw_minimap(rect, &minimap));
        });
        let result = result
            .into_inner()
            .expect("draw_minimap ran inside the frame scope");

        assert!(
            result.painted,
            "macOS now has a Core Graphics/Core Text minimap rasteriser (#961) -- \
             `painted` must report `true`"
        );
        assert!(
            !result.layout.visible_lines.is_empty(),
            "the layout must be real geometry"
        );
        assert!(
            b.zones().iter().any(|z| z.id == minimap.id),
            "a click zone must be registered so a host can route clicks"
        );

        let painted_any = (0..W).any(|x| {
            (0..H).any(|y| {
                let (r, g, bl, _) = surface.pixel(x, y);
                (r, g, bl) != (255, 255, 255)
            })
        });
        assert!(painted_any, "draw_minimap must paint non-background pixels");
    }

    /// `minimap_layout` (the no-paint query `AppLogic::handle` calls for
    /// click routing) must agree with the layout `draw_minimap` just
    /// returned — same contract every other backend upholds.
    #[test]
    fn minimap_layout_agrees_with_draw_minimap() {
        use super::super::headless::BitmapSurface;
        use super::super::text::make_font;

        let surface = BitmapSurface::new(100, 200);
        let mut b = MacBackend::new();
        b.set_current_font(make_font("Menlo", 12.0).expect("Menlo installed"));
        let minimap = sample_minimap();
        let rect = Rect::new(0.0, 0.0, 20.0, 100.0);

        let painted = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            *painted.borrow_mut() = Some(backend.draw_minimap(rect, &minimap));
        });
        let painted = painted
            .into_inner()
            .expect("draw_minimap ran inside the frame scope");
        let layout_only = b.minimap_layout(rect, &minimap);

        assert_eq!(painted.layout, layout_only);
    }

    /// Renamed from #802-era `draw_image_does_not_panic_and_reports_unsupported`:
    /// with a real decoder now wired in (#962), an empty byte source is no
    /// longer "there's no decoder to try" but "the decoder was tried and
    /// had nothing to decode" — still a clean `Unsupported`, just for a
    /// different reason. Deliberately called outside `enter_frame_scope`
    /// (no `ctx`) to prove decoding is attempted, and fails, before
    /// `draw_image` ever needs a live CoreGraphics context — see
    /// `super::image::draw_image`'s doc comment.
    #[test]
    fn draw_image_from_empty_bytes_reports_unsupported() {
        let mut b = MacBackend::new();
        let image = crate::primitives::image::Image {
            id: WidgetId::new("logo"),
            source: crate::primitives::image::ImageSource::Bytes(Vec::new()),
            intrinsic_size: Some((24, 24)),
            fit: crate::primitives::image::ImageFit::Contain,
            fallback_text: "[Q]".into(),
        };
        let rect = Rect::new(0.0, 0.0, 24.0, 24.0);

        let result = b.draw_image(rect, &image);

        assert_eq!(
            result,
            crate::backend::ImagePaintResult::Unsupported,
            "an empty byte source has nothing to decode -- this must be a clean \
             Unsupported result, not a panic"
        );
        assert!(
            b.zones().iter().any(|z| z.id == image.id),
            "a click/hover zone must still be registered even though nothing painted"
        );
    }

    /// A tiny solid-colour PNG, generated at test time — same fixture
    /// shape `super::super::image::tests::tiny_png_bytes` /
    /// `win::image::tests::tiny_png_bytes` build, duplicated here (rather
    /// than shared) because it's a test-only fixture with no runtime
    /// purpose, matching how each backend's own `image.rs` test module
    /// already keeps its own copy.
    fn tiny_png_bytes() -> Vec<u8> {
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

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &ihdr);

        // `clippy::same_item_push` misreads the filter-byte push as a
        // "replace this loop with vec![0; N]" candidate — the loop body
        // also appends the row's actual pixel bytes right after it, so
        // that rewrite doesn't apply (see `macos::image::tests::tiny_png_bytes`,
        // whose scanline loop has the same shape).
        #[allow(clippy::same_item_push)]
        let raw = {
            let mut raw = Vec::new();
            for _ in 0..4 {
                raw.push(0u8);
                for _ in 0..4 {
                    raw.extend_from_slice(&[0x00, 0xff, 0x00]);
                }
            }
            raw
        };
        let idat = zlib_store(&raw);
        chunk(&mut png, b"IDAT", &idat);

        chunk(&mut png, b"IEND", &[]);
        png
    }

    fn zlib_store(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let len = data.len() as u16;
        out.push(1);
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

    /// #962: `draw_image` must actually paint pixels + report `Painted` —
    /// the positive replacement for #802's degrade-to-`Unsupported`
    /// coverage, now that macOS has a real Core Graphics/ImageIO image
    /// rasteriser (`super::image::draw_image`). Mirrors
    /// `draw_minimap_paints_and_reports_painted` above.
    #[test]
    fn draw_image_paints_and_reports_painted() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 64;
        const H: u32 = 64;

        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut b = MacBackend::new();
        let image = crate::primitives::image::Image {
            id: WidgetId::new("logo"),
            source: crate::primitives::image::ImageSource::Bytes(tiny_png_bytes()),
            intrinsic_size: Some((4, 4)),
            fit: crate::primitives::image::ImageFit::Fill,
            fallback_text: "[Q]".into(),
        };
        let rect = Rect::new(10.0, 10.0, 20.0, 20.0);

        let result = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            *result.borrow_mut() = Some(backend.draw_image(rect, &image));
        });
        let result = result
            .into_inner()
            .expect("draw_image ran inside the frame scope");

        assert_eq!(
            result,
            crate::backend::ImagePaintResult::Painted,
            "macOS now has a Core Graphics/ImageIO image rasteriser (#962) -- a \
             decodable source must report Painted"
        );
        assert!(
            b.zones().iter().any(|z| z.id == image.id),
            "a click/hover zone must be registered"
        );

        let painted_any = (0..W).any(|x| {
            (0..H).any(|y| {
                let (r, g, bl, _) = surface.pixel(x, y);
                (r, g, bl) != (255, 255, 255)
            })
        });
        assert!(painted_any, "draw_image must paint non-background pixels");
    }

    // ── Text selection (#803) ────────────────────────────────────────
    //
    // State-machine delegation coverage, mirroring `gtk::backend::tests`'
    // (now-lifted, see `crate::text_selection`'s own test module for the
    // shared behaviour) `gtk_*_text_selection*` suite. `apply_selection_highlight`'s
    // actual CoreGraphics paint is covered separately below, against a real
    // `BitmapSurface`.

    fn text_region(id: &str, x: f32, y: f32, w: f32, h: f32, lines: Vec<&str>) -> TextRegion {
        TextRegion {
            id: WidgetId::new(id),
            bounds: crate::event::Rect::new(x, y, w, h),
            lines: lines.into_iter().map(String::from).collect(),
        }
    }

    /// Mirrors `line_height_picks_up_set_current_line_height_via_font_install`'s
    /// inline `make_font` call above, lifted into a helper since the
    /// text-selection extraction tests below need it more than once.
    fn font() -> CTFont {
        super::super::text::make_font("Menlo", 14.0).expect("Menlo installed")
    }

    #[test]
    fn mac_backend_declares_text_selection_capability() {
        let b = MacBackend::new();
        assert!(
            b.backend_caps().text_selection,
            "#803: MacBackend must declare text_selection now that register_text_region/\
             cancel_text_selection_drag are both overridden"
        );
    }

    // ── #929: register_font_from_memory / set_nerd_font_fallback ────────

    #[test]
    fn mac_backend_declares_app_font_registration_capability() {
        let b = MacBackend::new();
        assert!(
            b.backend_caps().app_font_registration,
            "#929: MacBackend must declare app_font_registration now that \
             register_font_from_memory/set_nerd_font_fallback are both overridden"
        );
    }

    /// `set_nerd_font_fallback` called *before* `set_current_font` (the
    /// documented "call once from `setup()`" order) must still land on
    /// the font `set_current_font` installs.
    #[test]
    fn set_nerd_font_fallback_then_set_current_font_carries_the_fallback() {
        use core_foundation::base::TCFType;
        use core_foundation::string::CFString;
        use core_text::font_descriptor::kCTFontCascadeListAttribute;

        let mut b = MacBackend::new();
        b.set_nerd_font_fallback("Helvetica");
        b.set_current_font(font());
        let attrs = b
            .current_font
            .as_ref()
            .expect("set_current_font must install a font")
            .copy_descriptor()
            .attributes()
            .to_untyped();
        let cascade_key = unsafe { CFString::wrap_under_get_rule(kCTFontCascadeListAttribute) };
        assert!(
            attrs.contains_key(&cascade_key.as_CFTypeRef()),
            "a fallback set before set_current_font must still be applied to the installed font"
        );
    }

    /// The reverse order — `set_current_font` first, `set_nerd_font_fallback`
    /// second — must retroactively apply the fallback to the
    /// already-installed font rather than requiring `set_current_font`
    /// to be called again.
    #[test]
    fn set_current_font_then_set_nerd_font_fallback_retroactively_applies() {
        use core_foundation::base::TCFType;
        use core_foundation::string::CFString;
        use core_text::font_descriptor::kCTFontCascadeListAttribute;

        let mut b = MacBackend::new();
        b.set_current_font(font());
        b.set_nerd_font_fallback("Helvetica");
        let attrs = b
            .current_font
            .as_ref()
            .expect("current_font must still be installed")
            .copy_descriptor()
            .attributes()
            .to_untyped();
        let cascade_key = unsafe { CFString::wrap_under_get_rule(kCTFontCascadeListAttribute) };
        assert!(
            attrs.contains_key(&cascade_key.as_CFTypeRef()),
            "set_nerd_font_fallback must retroactively apply to an already-installed \
             current_font"
        );
    }

    /// Garbage bytes aren't a font Core Graphics can parse —
    /// `register_font_from_memory` must report that as `None`.
    #[test]
    fn mac_backend_register_font_from_memory_rejects_bytes_that_are_not_a_font() {
        let mut b = MacBackend::new();
        let garbage = [0u8; 64];
        assert!(
            b.register_font_from_memory(&garbage).is_none(),
            "64 zero bytes are not a parseable font — must report None, not a fabricated family"
        );
    }

    #[test]
    fn mac_register_text_region_and_begin_frame_clears_it() {
        let mut b = MacBackend::new();
        b.register_text_region(text_region("r", 0.0, 0.0, 100.0, 50.0, vec![]));
        assert_eq!(b.text_regions().len(), 1);
        b.begin_frame(Viewport::new(200.0, 100.0, 1.0));
        assert!(
            b.text_regions().is_empty(),
            "begin_frame must clear the previous frame's registered regions"
        );
    }

    #[test]
    fn mac_clear_selection_display_clears_active_selection_only() {
        let mut b = MacBackend::new();
        assert!(b.active_text_selection().is_none());
        b.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(50.0, 20.0),
        );
        assert!(b.active_text_selection().is_some());
        b.clear_selection_display();
        assert!(b.active_text_selection().is_none());
    }

    #[test]
    fn mac_clear_text_selection_also_ends_a_text_selection_drag() {
        let mut b = MacBackend::new();
        b.drag_state
            .borrow_mut()
            .begin(crate::dispatch::DragTarget::TextSelection {
                region: WidgetId::new("r"),
                anchor: Point::new(0.0, 0.0),
            });
        b.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
        );
        b.clear_text_selection();
        assert!(b.active_text_selection().is_none());
        assert!(
            !b.drag_state.borrow().is_active(),
            "clear_text_selection must also end an in-progress TextSelection drag"
        );
    }

    #[test]
    fn mac_cancel_text_selection_drag_preserves_the_displayed_selection() {
        let mut b = MacBackend::new();
        b.drag_state
            .borrow_mut()
            .begin(crate::dispatch::DragTarget::TextSelection {
                region: WidgetId::new("r"),
                anchor: Point::new(0.0, 0.0),
            });
        b.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
        );
        b.cancel_text_selection_drag_impl();
        assert!(!b.drag_state.borrow().is_active());
        assert!(
            b.active_text_selection().is_some(),
            "cancel_text_selection_drag must not clear the displayed selection"
        );
    }

    #[test]
    fn mac_select_all_text_region_targets_the_sole_region() {
        let mut b = MacBackend::new();
        b.register_text_region(text_region("body", 0.0, 0.0, 10.0, 5.0, vec![]));
        assert!(b.select_all_text_region());
        let sel = b
            .active_text_selection()
            .expect("selection should be active after select_all_text_region");
        assert_eq!(sel.anchor, Point::new(0.0, 0.0));
        assert_eq!(sel.focus, Point::new(10.0, 5.0));
    }

    #[test]
    fn mac_select_all_text_region_returns_false_with_no_regions() {
        let mut b = MacBackend::new();
        assert!(!b.select_all_text_region());
        assert!(b.active_text_selection().is_none());
    }

    #[test]
    fn mac_extract_selection_text_single_row() {
        let mut b = MacBackend::new();
        b.set_current_font(font());
        let line = "The quick brown fox jumps over the lazy dog.";
        // Size the region from the backend's *real* CoreText char_width
        // (Menlo 14pt) rather than a hardcoded pixel width — GTK's/Win's
        // equivalent tests get to assume a synthetic fixed char_width;
        // macOS's is whatever CoreText actually measures. A couple of
        // cells of slack past the line's length keeps `end_col`'s clamp
        // (see `text_selection_line_range`'s doc) from truncating it.
        let width = (line.chars().count() as f32 + 2.0) * b.char_width();
        b.register_text_region(text_region("body", 0.0, 0.0, width, 100.0, vec![line]));
        b.set_active_text_selection(
            WidgetId::new("body"),
            Point::new(0.0, 0.0),
            Point::new(width, 1.0),
        );
        let text = b.extract_selection_text();
        assert_eq!(text, line);
    }

    #[test]
    fn mac_extract_selection_text_empty_when_no_selection() {
        let b = MacBackend::new();
        assert_eq!(b.extract_selection_text(), "");
    }

    #[test]
    fn mac_extract_selection_text_empty_when_no_lines() {
        let mut b = MacBackend::new();
        b.set_current_font(font());
        b.register_text_region(text_region("body", 0.0, 0.0, 200.0, 32.0, vec![]));
        b.set_active_text_selection(
            WidgetId::new("body"),
            Point::new(0.0, 0.0),
            Point::new(200.0, 32.0),
        );
        assert_eq!(
            b.extract_selection_text(),
            "",
            "a region with no `lines` content has nothing to extract"
        );
    }

    /// `apply_selection_highlight`'s real CoreGraphics paint, checked
    /// against a headless `BitmapSurface` — the pixel-level half of
    /// #803's acceptance criteria. Paints a solid-colour `TextRegion`
    /// background via `draw_status_bar`, drags a selection across it
    /// (`set_active_text_selection` directly — the dispatch-level drag
    /// round trip is covered in `macos::run`'s own tests), calls
    /// `apply_selection_highlight`, and asserts the pixel at the
    /// selection's location shifted toward the translucent-blue highlight
    /// instead of staying the plain background colour.
    ///
    /// ## Why the probe column is derived, not hardcoded
    ///
    /// The blend is `bg + 0.30 * (highlight - bg)`, so the size of the
    /// blue-channel rise depends entirely on what was underneath. A
    /// fixed `x = 2` probe lands on the antialiased stem of the first
    /// glyph (~`(206, 206, 206)` for white-on-dark Menlo), where the
    /// rise shrinks to `0.30 * (255 - 206) ≈ 15` and a
    /// "blue must rise noticeably" threshold fails even though the
    /// paint is perfectly correct. So the probe is taken from the
    /// **painted `StatusBarLayout`** — just past the text segment's
    /// right edge, where the bar fill is still `BG` and no glyph can
    /// reach — the same glyph-free-padding trick
    /// `macos::status_bar`'s own tests use.
    #[test]
    fn mac_apply_selection_highlight_paints_over_the_background() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::types::Color;

        const W: u32 = 200;
        const H: u32 = 32;
        const BG: Color = Color::rgb(10, 10, 10);
        const TEXT: &str = "hello world";

        let surface = BitmapSurface::new(W, H);
        let mut b = MacBackend::new();
        b.set_current_font(font());
        b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |bk| {
            let l = bk.draw_status_bar_interactive(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &StatusBar {
                    id: WidgetId::new("bg"),
                    left_segments: vec![StatusBarSegment {
                        text: TEXT.into(),
                        fg: Color::rgb(255, 255, 255),
                        bg: BG,
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                },
                &crate::InteractionState::new(),
            );
            *layout.borrow_mut() = Some(l);
        });
        let layout = layout.into_inner().expect("draw_status_bar ran");
        let seg = layout
            .visible_segments
            .first()
            .expect("the single left segment is visible in a 200pt-wide bar");

        // Glyph-free background column: two points past the text
        // segment's right edge, still inside the bar's full-width fill.
        let probe_x = (seg.bounds.x + seg.bounds.width) as u32 + 2;
        assert!(
            probe_x < W,
            "probe column {probe_x} must stay inside the {W}pt surface — the \
             '{TEXT}' segment measured wider than expected ({seg:?})"
        );
        let probe_y = 2;
        // A row below the single selected row: still inside the surface
        // but outside the highlight, so it proves the fill is clipped to
        // the selection instead of flooding the region.
        let line_h = b.line_height();
        let unselected_y = line_h as u32 + 2;
        assert!(
            unselected_y < H,
            "the unselected probe row {unselected_y} must stay inside the \
             {H}pt surface (line_height = {line_h})"
        );

        b.register_text_region(text_region("bg", 0.0, 0.0, W as f32, H as f32, vec![TEXT]));
        b.set_active_text_selection(
            WidgetId::new("bg"),
            Point::new(0.0, 0.0),
            Point::new(W as f32, 1.0),
        );

        // Sample before painting the highlight, then paint it and sample
        // again — same probe pixels, so the diff isolates exactly what
        // `apply_selection_highlight` changed.
        let before = surface.pixel(probe_x, probe_y);
        let before_unselected = surface.pixel(probe_x, unselected_y);
        assert_eq!(
            before,
            (BG.r, BG.g, BG.b, 255),
            "the probe pixel must sit on the bar's flat background fill, not on \
             a glyph — otherwise the blend below is measured against the wrong base"
        );
        b.enter_frame_scope(surface.context_ptr(), |bk| {
            bk.apply_selection_highlight();
        });
        let after = surface.pixel(probe_x, probe_y);
        let after_unselected = surface.pixel(probe_x, unselected_y);

        assert_ne!(
            before, after,
            "apply_selection_highlight must paint over the background at a selected pixel"
        );
        // i32 arithmetic so an unexpected sample can never overflow a u8
        // and panic *before* the assertion message gets to print it.
        let (ar, ag, ab) = (after.0 as i32, after.1 as i32, after.2 as i32);
        assert!(
            ab > before.2 as i32 + 20,
            "the highlight is translucent blue, so the blue channel must rise \
             noticeably at a selected pixel: before={before:?} after={after:?}"
        );
        assert!(
            ab > ar + 20 && ab > ag + 20,
            "blue must end up the dominant channel after a translucent-blue \
             highlight: after={after:?}"
        );
        assert_eq!(
            before_unselected, after_unselected,
            "the highlight must be clipped to the selected row — row at y={unselected_y} \
             is outside the one-row selection and must be untouched"
        );
    }

    // ── NativeSurface (#807, Phase 1) ────────────────────────────────
    //
    // Unlike the GTK/Windows twins of these two tests, these only run on
    // a real Mac — this whole module (`mod macos` in `lib.rs`) is gated
    // `#[cfg(all(feature = "macos", target_os = "macos"))]`, so there is
    // no "type-checks on Linux, runs for real on CI" split to call out
    // here the way `win_backend_native_surface_verbs_do_not_panic` does:
    // if this file compiles at all, it's on `macos.yml`'s `macos-latest`
    // runner, and these tests run there for real, pixels and all.

    #[test]
    fn mac_backend_native_surface_fill_rect_paints_solid_color() {
        use super::super::headless::BitmapSurface;
        use crate::types::Color;

        const W: u32 = 32;
        const H: u32 = 32;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());

        let red = Color::rgb(200, 20, 20);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), red);
        });
        backend.end_frame();

        let (r, g, b, _a) = surface.pixel(5, 5);
        assert_eq!(
            (r, g, b),
            (red.r, red.g, red.b),
            "surface_fill_rect must paint the solid color it was given"
        );
    }

    /// Not a pixel-precision test for every verb (that's `fill_rect`'s job
    /// above) — this exercises every remaining `NativeSurface` method at
    /// least once end-to-end (frame lifecycle, measurement, stroke, line,
    /// clip push/pop, text run, image) so an implementation bug (wrong arg
    /// order, a swapped field, a panic inside the frame-scope guard) fails
    /// a test instead of shipping silently — the same coverage
    /// `gtk_backend_native_surface_verbs_do_not_panic` /
    /// `win_backend_native_surface_verbs_do_not_panic` give their
    /// backends, closing the gap this issue's review flagged: until this
    /// test existed, nothing anywhere called a `MacBackend::surface_*`
    /// method, so the hand-written `ns_fill_rect`/`ns_stroke_rect`/
    /// `ns_draw_line`/`ns_push_clip` CoreGraphics FFI was verified by
    /// nothing beyond "it compiles".
    #[test]
    fn mac_backend_native_surface_verbs_do_not_panic() {
        use super::super::headless::BitmapSurface;
        use crate::types::Color;

        const W: u32 = 64;
        const H: u32 = 64;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());

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

        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let blue = Color::rgb(20, 20, 200);
            let white = Color::rgb(255, 255, 255);
            b.surface_stroke_rect(Rect::new(0.0, 0.0, 40.0, 40.0), blue, 2.0);
            b.surface_draw_line(Point::new(0.0, 0.0), Point::new(40.0, 40.0), blue, 1.0);
            b.surface_push_clip(Rect::new(0.0, 0.0, 40.0, 40.0));
            b.surface_draw_text_run(Rect::new(2.0, 2.0, 30.0, 10.0), "hi", white);
            b.surface_pop_clip();

            let (w, h) = b.surface_measure_text("hi");
            assert!(
                w > 0.0 && h > 0.0,
                "surface_measure_text must report a nonzero footprint for \
                 non-empty text: got ({w}, {h})"
            );

            let image = crate::primitives::image::Image {
                id: WidgetId::new("test:native-surface-image"),
                source: crate::primitives::image::ImageSource::Bytes(Vec::new()),
                intrinsic_size: Some((8, 8)),
                fit: crate::primitives::image::ImageFit::Contain,
                fallback_text: "[i]".to_string(),
            };
            // Empty bytes have nothing to decode, so this stays
            // `Unsupported` even with #962's real decoder wired in — this
            // call only needs to prove it reaches the same code
            // `Backend::draw_image` does (inside a live frame scope, this
            // time), not any particular decode outcome.
            let _ = b.surface_draw_image(Rect::new(0.0, 40.0, 8.0, 8.0), &image);
        });

        backend.surface_end_frame();
    }

    // ── Form (#808, NativeSurface Phase 2a) ──────────────────────────

    /// Regression for #808: pre-fix, `macos::form::draw_form` matched
    /// only 10 of 14 `FieldKind` variants and silently fell through
    /// (`_ => {}`) on `Slider` / `ColorPicker` / `Dropdown` / `TextArea`
    /// — a form using one of those painted nothing beyond its label and
    /// row background, with no error anywhere. Confirmed to reproduce
    /// against unfixed `develop @ a9104f5`. `draw_form` now routes
    /// through the shared `primitives::form::paint`, which handles all
    /// 14 variants — this scans each field's row, past where its label
    /// ends, for a pixel that isn't the row background, proving the
    /// *value* painted something (a label-only hit doesn't count: every
    /// pre-fix backend already painted labels regardless of whether the
    /// value fell through).
    #[test]
    fn mac_backend_draw_form_paints_the_four_field_kinds_macos_used_to_drop() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::form::{FieldKind, Form, FormField};
        use crate::types::StyledText;

        const W: u32 = 320;
        const H: u32 = 160;

        fn field(id: &str, label: &str, kind: FieldKind) -> FormField {
            FormField {
                id: WidgetId::new(id),
                label: StyledText::plain(label),
                kind,
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }
        }

        let form = Form {
            id: WidgetId::new("regression-808"),
            fields: vec![
                field(
                    "font-size",
                    "Font size",
                    FieldKind::Slider {
                        value: 14.0,
                        min: 8.0,
                        max: 32.0,
                        step: 1.0,
                    },
                ),
                field(
                    "accent",
                    "Accent",
                    FieldKind::ColorPicker {
                        value: Color::rgb(0x7a, 0xb4, 0xff),
                    },
                ),
                field(
                    "theme",
                    "Theme",
                    FieldKind::Dropdown {
                        options: vec![
                            StyledText::plain("One Dark"),
                            StyledText::plain("Solarized"),
                        ],
                        selected_idx: 1,
                    },
                ),
                field(
                    "notes",
                    "Notes",
                    FieldKind::TextArea {
                        value: "release notes".into(),
                        placeholder: String::new(),
                        cursor: None,
                        visible_rows: 3,
                    },
                ),
            ],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };

        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let flayout =
            crate::macos::form::mac_form_layout(&form, rect, backend.line_height() as f64, &font());
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_form(rect, &form);
        });
        backend.end_frame();

        let theme = Theme::default();
        let bg = (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b);

        for vf in &flayout.visible_fields {
            let field = &form.fields[vf.field_idx];
            let x0 = (vf.bounds.x + vf.bounds.width * 0.6) as u32;
            let x1 = ((vf.bounds.x + vf.bounds.width) as u32).min(W);
            let y0 = vf.bounds.y as u32;
            let y1 = ((vf.bounds.y + vf.bounds.height) as u32).min(H);

            let mut painted_beyond_label = false;
            'scan: for y in y0..y1 {
                for x in x0..x1 {
                    let (r, g, b, _a) = surface.pixel(x, y);
                    if (r, g, b) != bg {
                        painted_beyond_label = true;
                        break 'scan;
                    }
                }
            }
            assert!(
                painted_beyond_label,
                "field {:?} ({:?}) must paint something in the right ~40% of its \
                 row (past the label) — this is exactly the #808 silent-drop bug: \
                 pre-fix, macos::form::draw_form's `_ => {{}}` matched Slider/\
                 ColorPicker/Dropdown/TextArea and painted only the row background \
                 there",
                field.id, field.kind,
            );
        }
    }

    // ── find_replace (#809, `NativeSurface` Phase 2b) ──────────────────
    //
    // `macos::find_replace` used to carry its own `#[cfg(test)]` module
    // with `panel_paints_surface_bg`, `panel_with_multibyte_query_does_not_panic`
    // and `hit_regions_present_for_basic_panel`, exercising
    // `macos::find_replace::draw_find_replace` against a real
    // `BitmapSurface`/Core Text stack. That module (and the whole file)
    // is deleted — the paint logic it tested now lives in
    // `crate::primitives::find_replace::paint`, already covered on every
    // host by that module's own `RecordingSurface` tests. These two
    // tests are the twin that stays macOS-only for a reason: they prove
    // the *plumbing* — `MacBackend::draw_find_replace` really does reach
    // `paint` and `paint` really does land real Core Text pixels through
    // `MacBackend`'s `NativeSurface` impl — not the paint logic itself.

    fn find_replace_sample_panel(w: f32, h: f32) -> FindReplacePanel {
        let (hit_regions, _input_width) =
            crate::primitives::find_replace::compute_hit_regions(50, false, "1 of 3", 2, 2);
        FindReplacePanel {
            query: "needle".into(),
            replacement: String::new(),
            show_replace: false,
            focus: 0,
            cursor: 6,
            sel_anchor: None,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds: crate::event::Rect::new(0.0, 0.0, w, h),
            panel_width: 50,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions,
        }
    }

    /// End-to-end pixel probe: `Backend::draw_find_replace` on a real
    /// `MacBackend`, over a headless `BitmapSurface`, must actually paint
    /// the popup background — proving `MacBackend`'s `NativeSurface`
    /// impl really reaches Core Graphics, not just that the shared
    /// painter emits the right verb (the primitive-level
    /// `RecordingSurface` test already covers that half, portably).
    #[test]
    fn mac_backend_draw_find_replace_paints_popup_background() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 600;
        const H: u32 = 200;

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let panel = find_replace_sample_panel(W as f32, H as f32);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_find_replace(
                crate::event::Rect::new(0.0, 0.0, W as f32, H as f32),
                &panel,
            );
        });
        backend.end_frame();

        let theme = crate::theme::Theme::default();
        let cw = backend.char_width().max(1.0);
        let lh = backend.line_height().max(1.0);
        let popup_w = panel.panel_width as f32 * cw;
        let popup_h = 3.0 * lh; // show_replace == false: 1 content row + 2 border rows
        let popup_x = (panel.group_bounds.x + panel.group_bounds.width - popup_w - 10.0)
            .max(panel.group_bounds.x);
        let popup_y = panel.group_bounds.y + 2.0;

        // A few px inside the popup's bottom-right corner: past the
        // border stroke and below the single (`row == 0`) content row,
        // so nothing but the background fill can have painted here.
        let (r, g, b, _a) = surface.pixel(
            (popup_x + popup_w - 4.0) as u32,
            (popup_y + popup_h - 4.0) as u32,
        );
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
            "find/replace popup background must be painted through MacBackend's \
             real NativeSurface impl",
        );
    }

    /// Regression twin of `primitives::find_replace`'s own
    /// `multibyte_query_with_out_of_range_selection_does_not_panic`:
    /// that test proves the shared slicing logic is safe in the
    /// abstract; this proves real Core Text measurement/drawing of the
    /// same multibyte text through `MacBackend` doesn't panic either.
    #[test]
    fn mac_backend_draw_find_replace_with_multibyte_query_does_not_panic() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 600;
        const H: u32 = 200;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let mut panel = find_replace_sample_panel(W as f32, H as f32);
        panel.query = "café🎉中文".into();
        panel.cursor = 3;
        panel.sel_anchor = Some(6);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_find_replace(
                crate::event::Rect::new(0.0, 0.0, W as f32, H as f32),
                &panel,
            );
        });
        backend.end_frame();
    }

    // ── #810: draw_chart real-pixel driver tests ────────────────────────
    //
    // Ported from the deleted `macos::chart::tests` (that module already
    // drove the real `MacBackend::draw_chart` via its own local
    // `paint_via_backend` helper — the shape every other backend's #810
    // migration now also uses) to live alongside every other
    // `draw_*`-family driver test in this module.

    use super::super::headless::BitmapSurface;
    use crate::primitives::chart::{
        Chart as ChartModel, ChartHit, ChartKind, Series, SERIES_COLORS,
    };

    const CHART_W: u32 = 200;
    const CHART_H: u32 = 120;

    fn chart_sample_line() -> ChartModel {
        ChartModel {
            id: WidgetId::new("ch"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "x".into(),
                data: vec![0.0, 1.0, 0.5, 2.0, 1.0],
                color: Some(Color::rgb(80, 160, 255)),
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: Some((0.0, 2.0)),
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
        }
    }

    fn paint_chart_via_backend(
        chart: &ChartModel,
        hovered: Option<(usize, usize)>,
    ) -> (BitmapSurface, ChartLayout) {
        let surface = BitmapSurface::new(CHART_W, CHART_H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(CHART_W as f32, CHART_H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_chart(
                Rect::new(0.0, 0.0, CHART_W as f32, CHART_H as f32),
                chart,
                hovered,
                None,
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn mac_backend_draw_chart_plot_area_paints_background() {
        let chart = chart_sample_line();
        let (surface, layout) = paint_chart_via_backend(&chart, None);
        let theme = Theme::default();
        let px = (layout.plot_area.x + layout.plot_area.width - 2.0) as u32;
        let py = (layout.plot_area.y + 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
        );
    }

    #[test]
    fn mac_backend_draw_chart_line_stroke_paints_series_color() {
        let chart = chart_sample_line();
        let (surface, layout) = paint_chart_via_backend(&chart, None);
        // Sample at data point index 1 (value 1.0 in [0..2] range = mid
        // plot, comfortably inside bounds — first and last points sit
        // on the plot edges).
        let (_, _, sx, sy) = layout.data_point_positions[1];
        let (r, g, b, _) = surface.pixel(sx as u32, sy as u32);
        assert!(
            b > r,
            "expected blue dominant at data point 1 ({}, {}), got ({}, {}, {})",
            sx as u32,
            sy as u32,
            r,
            g,
            b,
        );
    }

    #[test]
    fn mac_backend_draw_chart_hit_test_inside_plot_area_returns_body() {
        let chart = chart_sample_line();
        let (_surface, layout) = paint_chart_via_backend(&chart, None);
        let cx = layout.plot_area.x + layout.plot_area.width * 0.5;
        let cy = layout.plot_area.y + layout.plot_area.height * 0.5;
        assert_eq!(layout.hit_test(cx, cy), ChartHit::Body(WidgetId::new("ch")),);
    }

    #[test]
    fn mac_backend_draw_chart_hover_marker_painted_when_set() {
        let chart = chart_sample_line();
        let (surface, layout) = paint_chart_via_backend(&chart, Some((0, 2)));
        let (_, _, sx, sy) = layout.data_point_positions[2];
        let (r, _, b, _) = surface.pixel(sx as u32, sy as u32);
        assert!(b > r);
    }

    // ── Multi-series bars (#584 review — macOS had no bar coverage) ──────

    fn chart_bar_series(label: &str, data: Vec<f64>) -> Series {
        Series {
            label: label.into(),
            data,
            color: None,
            fill: false,
        }
    }

    fn chart_sample_bar(kind: ChartKind, series: Vec<Series>, y_range: (f64, f64)) -> ChartModel {
        ChartModel {
            id: WidgetId::new("ch"),
            kind,
            series,
            x_label: None,
            y_label: None,
            y_range: Some(y_range),
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
        }
    }

    #[test]
    fn mac_backend_draw_chart_stacked_bar_paints_each_series_in_its_own_color() {
        let chart = chart_sample_bar(
            ChartKind::Bar,
            vec![
                chart_bar_series("a", vec![1.0]),
                chart_bar_series("b", vec![1.0]),
                chart_bar_series("c", vec![1.0]),
            ],
            (0.0, 3.0),
        );
        let (surface, _layout) = paint_chart_via_backend(&chart, None);

        // slot_w = pw = 200, gap = 30, bar_w = 170, slot_x = 15 → mid-x = 100.
        let mid_x = 100;
        let (r0, g0, b0, _) = surface.pixel(mid_x, 100);
        assert_eq!(
            (r0, g0, b0),
            (SERIES_COLORS[0].r, SERIES_COLORS[0].g, SERIES_COLORS[0].b)
        );
        let (r1, g1, b1, _) = surface.pixel(mid_x, 60);
        assert_eq!(
            (r1, g1, b1),
            (SERIES_COLORS[1].r, SERIES_COLORS[1].g, SERIES_COLORS[1].b)
        );
        let (r2, g2, b2, _) = surface.pixel(mid_x, 20);
        assert_eq!(
            (r2, g2, b2),
            (SERIES_COLORS[2].r, SERIES_COLORS[2].g, SERIES_COLORS[2].b)
        );
    }

    #[test]
    fn mac_backend_draw_chart_grouped_bar_paints_series_side_by_side() {
        let chart = chart_sample_bar(
            ChartKind::BarGrouped,
            vec![
                chart_bar_series("a", vec![1.0]),
                chart_bar_series("b", vec![2.0]),
                chart_bar_series("c", vec![3.0]),
            ],
            (0.0, 3.0),
        );
        let (surface, _layout) = paint_chart_via_backend(&chart, None);

        let probe_y = 100;
        let (r0, g0, b0, _) = surface.pixel(43, probe_y);
        assert_eq!(
            (r0, g0, b0),
            (SERIES_COLORS[0].r, SERIES_COLORS[0].g, SERIES_COLORS[0].b),
            "leftmost sub-bar should be series 0"
        );
        let (r1, g1, b1, _) = surface.pixel(100, probe_y);
        assert_eq!(
            (r1, g1, b1),
            (SERIES_COLORS[1].r, SERIES_COLORS[1].g, SERIES_COLORS[1].b),
            "middle sub-bar should be series 1"
        );
        let (r2, g2, b2, _) = surface.pixel(157, probe_y);
        assert_eq!(
            (r2, g2, b2),
            (SERIES_COLORS[2].r, SERIES_COLORS[2].g, SERIES_COLORS[2].b),
            "rightmost sub-bar should be series 2"
        );
    }

    /// Regression for quadraui#791/#810: before #791, `macos::chart`
    /// already clipped via `CGContextSaveGState`/`CGContextClipToRect`;
    /// #810 unified all three backends' `draw_chart` onto the shared
    /// `primitives::chart::paint`, which is what now owns this clip for
    /// every pixel backend — this is the "driver-tier" proof for macOS,
    /// mirroring `gtk::backend::tests`'s and `win::backend::tests`'s
    /// twin tests pixel-for-pixel (same fixture, same geometry).
    #[test]
    fn mac_backend_draw_chart_hover_marker_does_not_escape_chart_rect() {
        let chart = chart_sample_bar(
            ChartKind::Sparkline,
            vec![chart_bar_series("a", vec![10.0, 0.0])],
            (0.0, 10.0),
        );

        let canvas_w = 40;
        let canvas_h = 40;
        let sentinel = Color::rgb(0, 0, 0);
        let (chart_x, chart_y, chart_w, chart_h) = (10.0_f32, 10.0_f32, 20.0_f32, 20.0_f32);

        let surface = BitmapSurface::new(canvas_w, canvas_h);
        surface.fill(0.0, 0.0, 0.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(canvas_w as f32, canvas_h as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.surface_fill_rect(
                Rect::new(0.0, 0.0, canvas_w as f32, canvas_h as f32),
                sentinel,
            );
            b.draw_chart(
                Rect::new(chart_x, chart_y, chart_w, chart_h),
                &chart,
                Some((0, 0)),
                None,
            );
        });
        backend.end_frame();

        // Inside the marker's 8-unit-radius ring, centred at the
        // chart's own top-left corner (10, 10), but outside the
        // chart's rect.
        let (r, g, b, _) = surface.pixel(5, 5);
        assert_eq!(
            (r, g, b),
            (sentinel.r, sentinel.g, sentinel.b),
            "hover marker must not paint outside the chart's own rect",
        );
    }

    /// #860 review follow-up: driver-tier coverage (mirrors
    /// `gtk::backend::tests::gtk_backend_draw_status_bar_negative_width_does_not_paint`)
    /// proving the shared
    /// `primitives::status_bar::native_surface_paint::paint`'s #791
    /// zero/negative-size guard actually short-circuits through the real
    /// `MacBackend`, not just the primitive-tier `RecordingSurface` test.
    /// Unlike GTK, macOS already carried this guard pre-#860 (see that
    /// module's own doc), so this is completeness coverage rather than a
    /// regression proof — and it uses a literal zero width rather than a
    /// negative one: CoreGraphics's `CGContextClipToRect`/`CGContextFillRect`
    /// don't share Cairo's negative-size "mirrors into a real rect"
    /// behaviour that makes a negative width meaningful there.
    #[test]
    fn mac_backend_draw_status_bar_zero_width_does_not_paint() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::types::Color;

        const W: u32 = 40;
        const H: u32 = 40;
        let sentinel = Color::rgb(7, 8, 9);

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), sentinel);
            b.draw_status_bar_interactive(
                Rect::new(10.0, 10.0, 0.0, 15.0),
                &StatusBar {
                    id: WidgetId::new("test:status-bar"),
                    left_segments: vec![StatusBarSegment {
                        text: "READY".into(),
                        fg: Color::rgb(255, 255, 255),
                        bg: Color::rgb(200, 0, 0),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                },
                &crate::InteractionState::new(),
            );
        });
        backend.end_frame();

        for y in 0..H {
            for x in 0..W {
                let (r, g, b, _) = surface.pixel(x, y);
                assert_eq!(
                    (r, g, b),
                    (sentinel.r, sentinel.g, sentinel.b),
                    "pixel ({x}, {y}) must stay untouched: a zero-width rect must \
                     short-circuit to the no-paint layout",
                );
            }
        }
    }

    // ── #810: draw_text_display real-pixel driver tests ─────────────────
    //
    // Ported from the deleted `macos::text_display::tests` (that module
    // already drove the real `MacBackend::draw_text_display` via its own
    // local `paint`/`paint_at` helpers) to live alongside every other
    // `draw_*`-family driver test in this module.

    use crate::primitives::text_display::{
        TextDisplay as TextDisplayModel, TextDisplayHit, TextDisplayLine as TextDisplayLineModel,
    };
    use crate::types::Decoration;

    const TD_W: u32 = 240;
    const TD_H: u32 = 160;

    fn td_line(text: &str) -> TextDisplayLineModel {
        TextDisplayLineModel {
            spans: vec![crate::types::StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn make_td(lines: usize, show_scrollbar: bool) -> TextDisplayModel {
        TextDisplayModel {
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

    fn paint_td_via_backend(
        td: &TextDisplayModel,
        rect: Rect,
    ) -> (
        BitmapSurface,
        crate::primitives::text_display::TextDisplayLayout,
    ) {
        let surface = BitmapSurface::new(TD_W, TD_H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(TD_W as f32, TD_H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_text_display(rect, td);
            let l = b.text_display_layout(rect, td);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn mac_backend_draw_text_display_background_fills_theme_background() {
        let td = make_td(0, false);
        let (s, _) = paint_td_via_backend(&td, Rect::new(0.0, 0.0, TD_W as f32, TD_H as f32));
        let theme = Theme::default();
        let (r, g, b, _) = s.pixel(TD_W / 2, TD_H / 2);
        assert_eq!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
        );
    }

    #[test]
    fn mac_backend_draw_text_display_scrollbar_gutter_paints_track_colour() {
        let td = make_td(100, true);
        let (s, layout) = paint_td_via_backend(&td, Rect::new(0.0, 0.0, TD_W as f32, TD_H as f32));
        let gutter = layout.scrollbar_bounds.expect("gutter present");
        let probe_x = (gutter.x + gutter.width / 2.0) as u32;
        let probe_y = (gutter.y + gutter.height - 2.0) as u32;
        let (r, g, b, _) = s.pixel(probe_x, probe_y);
        let theme = Theme::default();
        assert_eq!(
            (r, g, b),
            (
                theme.scrollbar_track.r,
                theme.scrollbar_track.g,
                theme.scrollbar_track.b,
            ),
        );
    }

    #[test]
    fn mac_backend_draw_text_display_scrollbar_thumb_paints_thumb_colour() {
        let td = make_td(100, true);
        let (s, layout) = paint_td_via_backend(&td, Rect::new(0.0, 0.0, TD_W as f32, TD_H as f32));
        let thumb = layout.thumb_bounds.expect("thumb present");
        let probe_x = (thumb.x + thumb.width / 2.0) as u32;
        let probe_y = (thumb.y + thumb.height / 2.0) as u32;
        let (r, g, b, _) = s.pixel(probe_x, probe_y);
        let theme = Theme::default();
        assert_eq!(
            (r, g, b),
            (
                theme.scrollbar_thumb.r,
                theme.scrollbar_thumb.g,
                theme.scrollbar_thumb.b,
            ),
        );
    }

    #[test]
    fn mac_backend_draw_text_display_layout_hit_test_resolves_lines() {
        let td = make_td(20, false);
        let (_, layout) = paint_td_via_backend(&td, Rect::new(0.0, 0.0, TD_W as f32, TD_H as f32));
        let vis = &layout.visible_lines[0];
        let cx = vis.bounds.x + vis.bounds.width / 2.0;
        let cy = vis.bounds.y + vis.bounds.height / 2.0;
        match layout.hit_test(cx, cy) {
            TextDisplayHit::Line(idx) => assert_eq!(idx, vis.line_idx),
            other => panic!("expected Line, got {:?}", other),
        }
    }

    /// Regression for quadraui#494 / LESSONS.md "Layout helpers must
    /// return coords in the same frame across backends": paint at a
    /// non-zero rect origin with a title row present, then round-trip
    /// an absolute click the way a real host does.
    #[test]
    fn mac_backend_draw_text_display_layout_hit_test_resolves_lines_at_nonzero_origin() {
        let mut td = make_td(20, false);
        td.title = Some(crate::types::StyledText::plain("Logs"));

        let rect_x = 9.0_f32;
        let rect_y = 17.0_f32;
        let rect = Rect::new(rect_x, rect_y, TD_W as f32 - rect_x, TD_H as f32 - rect_y);
        let (_surface, layout) = paint_td_via_backend(&td, rect);

        // `MacBackend::new()`'s default `current_line_height`, matching
        // `paint_td_via_backend`'s backend construction (no explicit
        // line-height override in this test module).
        let line_height = 16.0_f32;
        let body_y = rect_y + line_height;

        let vis = &layout.visible_lines[0];
        assert_eq!(
            vis.bounds.y, 0.0,
            "visible_lines bounds.y must be body-local, got {}",
            vis.bounds.y,
        );

        let abs_x = rect_x + vis.bounds.x + vis.bounds.width * 0.5;
        let abs_y = body_y + vis.bounds.y + vis.bounds.height * 0.5;
        let local_x = abs_x - rect_x;
        let local_y = abs_y - body_y;
        match layout.hit_test(local_x, local_y) {
            TextDisplayHit::Line(idx) => assert_eq!(idx, vis.line_idx),
            other => panic!("expected Line, got {:?}", other),
        }
    }

    // ── Chrome font (issue #963) ─────────────────────────────────────
    //
    // Acceptance coverage: macOS now carries a `chrome_font` distinct
    // from `current_font` (the editor font), defaulting to the CoreText
    // system UI font, and `Backend::set_ui_font`/`set_editor_font` are no
    // longer no-ops. See `ChromeSurface`'s doc for the paint-time wiring
    // these tests exercise through `draw_status_bar_interactive`.

    /// A `MacBackend` that never received a `set_ui_font` call must still
    /// default its chrome to the CoreText system UI font, not a monospace
    /// editor face — matching GTK's `"Sans 11"` / Win-GUI's `"Segoe UI"`
    /// un-set defaults instead of macOS's old "no chrome font at all"
    /// posture.
    #[test]
    fn chrome_font_defaults_to_system_ui_font_not_monospace() {
        let b = MacBackend::new();
        let expected = super::super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT);
        assert_eq!(
            b.chrome_font.family_name(),
            expected.family_name(),
            "an untouched MacBackend's chrome_font must be the CoreText system UI font"
        );
        assert_ne!(
            b.chrome_font.family_name(),
            "Menlo",
            "the default chrome font must not be a monospace editor face"
        );
    }

    /// `set_editor_font` and `set_ui_font` must land on independent state
    /// — the #912 hazard this issue exists to avoid. Growing one must
    /// never move the other's cached line height, and each setter must
    /// install exactly the font it was asked for.
    #[test]
    fn set_editor_font_and_set_ui_font_are_independent() {
        use crate::Backend;

        let mut b = MacBackend::new();
        b.set_editor_font("Menlo", 14.0);
        let editor_line_height_before = b.current_line_height;
        let chrome_line_height_before = b.chrome_line_height;

        // A much larger chrome font must move `chrome_line_height` but
        // leave the editor's untouched.
        Backend::set_ui_font(&mut b, "Helvetica 40");
        assert_eq!(
            b.current_line_height, editor_line_height_before,
            "set_ui_font must not touch the editor font's cached line height"
        );
        assert!(
            b.chrome_line_height > chrome_line_height_before,
            "set_ui_font(\"Helvetica 40\") must grow chrome_line_height from the \
             {DEFAULT_UI_FONT_SIZE_PT}pt default: before={chrome_line_height_before}, \
             after={}",
            b.chrome_line_height,
        );
        assert_eq!(b.chrome_font.family_name(), "Helvetica");
        assert_eq!(b.chrome_font.pt_size(), 40.0);

        // And the reverse: growing the editor font afterwards must not
        // move `chrome_line_height` back.
        let chrome_line_height_after_ui = b.chrome_line_height;
        Backend::set_editor_font(&mut b, "Menlo", 40.0);
        assert_eq!(
            b.chrome_line_height, chrome_line_height_after_ui,
            "set_editor_font must not touch chrome_line_height"
        );
        assert_eq!(
            b.current_font
                .as_ref()
                .expect("set_current_font ran")
                .family_name(),
            "Menlo",
        );
    }

    /// `set_ui_font` degrades to the CoreText system UI font (rather
    /// than panicking or silently keeping the old chrome font) when the
    /// description doesn't parse or names a family Core Text doesn't
    /// have installed.
    #[test]
    fn set_ui_font_degrades_to_system_font_on_unparseable_or_unknown_family() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_ui_font(&mut b, "not-a-valid-description");
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT).family_name(),
            "an unparseable font_desc must degrade to the system UI font"
        );

        Backend::set_ui_font(&mut b, "Definitely Not A Real Font Family 12");
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_ui_font(12.0).family_name(),
            "an unknown family must degrade to the system UI font at the requested size"
        );
        assert_eq!(b.chrome_font.pt_size(), 12.0);
    }

    /// `set_editor_font` keeps the previously installed face when the
    /// requested family isn't installed — the doc'd "degrade, don't
    /// fail" posture. Without [`super::super::text::make_font_exact`]
    /// this silently installed Helvetica (Core Text's substitute for an
    /// unknown name) as the *editor* font, i.e. a proportional face where
    /// the app asked for monospace.
    #[test]
    fn set_editor_font_keeps_the_installed_font_for_an_unknown_family() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_editor_font(&mut b, "Menlo", 14.0);
        let line_height_before = b.current_line_height;

        Backend::set_editor_font(&mut b, "Definitely Not A Real Font Family", 30.0);
        assert_eq!(
            b.current_font
                .as_ref()
                .expect("editor font still installed")
                .family_name(),
            "Menlo",
            "an unknown family must not replace the installed editor font",
        );
        assert_eq!(
            b.current_line_height, line_height_before,
            "a rejected set_editor_font must not move the cached line height",
        );
    }

    /// Issue #1023: `set_editor_font("monospace", …)` resolves to the
    /// CoreText fixed-pitch system font, not a literal (and unresolvable)
    /// family named `"monospace"`.
    #[test]
    fn set_editor_font_resolves_the_monospace_generic_token() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_editor_font(&mut b, "monospace", 14.0);
        let expected = super::super::text::system_monospace_font(14.0);
        assert_eq!(
            b.current_font
                .as_ref()
                .expect("set_current_font ran")
                .family_name(),
            expected.family_name(),
            "\"monospace\" must resolve to the CoreText fixed-pitch system font"
        );
    }

    /// Issue #1023: `set_ui_font` resolves all three CSS generic tokens
    /// (plus Pango's `Sans`/`Monospace` aliases) to their native CoreText
    /// counterparts, at the requested size.
    #[test]
    fn set_ui_font_resolves_generic_tokens() {
        use crate::Backend;

        let mut b = MacBackend::new();

        Backend::set_ui_font(&mut b, "monospace 13");
        let expected_mono = super::super::text::system_monospace_font(13.0);
        assert_eq!(b.chrome_font.family_name(), expected_mono.family_name());
        assert_eq!(b.chrome_font.pt_size(), 13.0);

        Backend::set_ui_font(&mut b, "sans-serif 15");
        let expected_ui = super::super::text::system_ui_font(15.0);
        assert_eq!(b.chrome_font.family_name(), expected_ui.family_name());
        assert_eq!(b.chrome_font.pt_size(), 15.0);

        Backend::set_ui_font(&mut b, "system-ui 17");
        assert_eq!(b.chrome_font.family_name(), expected_ui.family_name());
        assert_eq!(b.chrome_font.pt_size(), 17.0);

        // Pango's own alias spellings for the first two resolve the same
        // way — GTK already treats them as synonyms via fontconfig, and
        // this backend now agrees.
        Backend::set_ui_font(&mut b, "Monospace 19");
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_monospace_font(19.0).family_name()
        );
        Backend::set_ui_font(&mut b, "Sans 21");
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_ui_font(21.0).family_name()
        );
    }

    /// Issue #1023: `parse_ui_font_desc` splits a Pango-style
    /// comma-separated fallback list and resolves to the first family
    /// that's actually installed — `"Definitely Not A Real Font Family
    /// One, Definitely Not A Real Font Family Two, Menlo"` must land on
    /// `Menlo`, not fail the whole description the way a single
    /// `rfind(' ')` split used to (it would see `"...Family Two, Menlo"`
    /// as everything-before-a-nonexistent-size and reject the lot).
    #[test]
    fn set_ui_font_resolves_the_first_installed_family_in_a_comma_list() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_ui_font(
            &mut b,
            "Definitely Not A Real Font Family One, Definitely Not A Real Font Family Two, \
             Menlo 16",
        );
        assert_eq!(b.chrome_font.family_name(), "Menlo");
        assert_eq!(b.chrome_font.pt_size(), 16.0);
    }

    /// Same as the sized case above, but with no trailing size at all —
    /// the real-world shape this issue was filed over (vimcode's
    /// `UI_FONT_FAMILY` constants carry no size, only a comma-separated
    /// family list). Must still resolve to the first installed family,
    /// at the chrome default size.
    #[test]
    fn set_ui_font_resolves_a_comma_list_with_no_trailing_size() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_ui_font(
            &mut b,
            "Definitely Not A Real Font Family One, Definitely Not A Real Font Family Two, Menlo",
        );
        assert_eq!(b.chrome_font.family_name(), "Menlo");
        assert_eq!(b.chrome_font.pt_size(), DEFAULT_UI_FONT_SIZE_PT);
    }

    /// A generic token appearing later in a comma list (Pango's own
    /// "…, Sans" convention for "fall back to the platform default") is
    /// still an always-resolvable stop, exactly like a concrete
    /// installed family — the fallback chain from this issue's own
    /// motivating example (`"SF Pro Text, Helvetica Neue, Lucida Grande,
    /// Sans"`).
    #[test]
    fn set_ui_font_resolves_a_generic_token_later_in_a_comma_list() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_ui_font(
            &mut b,
            "Definitely Not A Real Font Family One, Definitely Not A Real Font Family Two, \
             sans-serif",
        );
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT).family_name()
        );
    }

    /// Black-box acceptance test (issue #963): paint a `StatusBar`
    /// through the real `Backend::draw_status_bar_interactive` path and
    /// prove its painted geometry tracks `set_ui_font`, not
    /// `set_editor_font` — the two-font separation actually reaching a
    /// rasteriser, not just backend-internal state.
    #[test]
    fn draw_status_bar_interactive_paints_with_chrome_font_not_editor_font() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::Backend;

        const W: u32 = 600;
        const H: u32 = 40;

        fn painted_segment_width(chrome_desc: &str, editor: (&str, f32)) -> f32 {
            let surface = BitmapSurface::new(W, H);
            let mut b = MacBackend::new();
            Backend::set_editor_font(&mut b, editor.0, editor.1);
            Backend::set_ui_font(&mut b, chrome_desc);
            b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

            let bar = StatusBar {
                id: WidgetId::new("status"),
                left_segments: vec![StatusBarSegment {
                    text: "Save".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(10, 10, 10),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let layout = std::cell::RefCell::new(None);
            b.enter_frame_scope(surface.context_ptr(), |backend| {
                let l = backend.draw_status_bar_interactive(
                    Rect::new(0.0, 0.0, W as f32, H as f32),
                    &bar,
                    &crate::InteractionState::new(),
                );
                *layout.borrow_mut() = Some(l);
            });
            b.end_frame();
            layout.into_inner().unwrap().visible_segments[0]
                .bounds
                .width
        }

        // Same chrome font, wildly different editor font sizes: painted
        // width must not move — proves the status bar reads chrome_font,
        // not current_font.
        let w_small_editor = painted_segment_width("Menlo 11", ("Menlo", 10.0));
        let w_huge_editor = painted_segment_width("Menlo 11", ("Menlo", 80.0));
        assert!(
            (w_small_editor - w_huge_editor).abs() < 0.01,
            "status bar width must be unaffected by set_editor_font: {w_small_editor} vs \
             {w_huge_editor}"
        );

        // Same editor font, wildly different chrome font sizes: painted
        // width MUST move — proves it does read chrome_font.
        let w_small_chrome = painted_segment_width("Menlo 8", ("Menlo", 14.0));
        let w_huge_chrome = painted_segment_width("Menlo 60", ("Menlo", 14.0));
        assert!(
            w_huge_chrome > w_small_chrome * 2.0,
            "status bar width must grow with set_ui_font: {w_small_chrome} vs {w_huge_chrome}"
        );
    }

    /// `status_bar_layout` (the no-paint twin) must agree with what
    /// `draw_status_bar_interactive` actually painted once both read
    /// `chrome_font` — regression guard for #963 keeping the two
    /// call sites in sync the way #484 already required for the
    /// pre-#963 `current_font`-only world.
    #[test]
    fn status_bar_layout_matches_painted_layout_after_set_ui_font() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::Backend;

        const W: u32 = 300;
        const H: u32 = 30;

        let mut b = MacBackend::new();
        Backend::set_editor_font(&mut b, "Menlo", 14.0);
        Backend::set_ui_font(&mut b, "Helvetica 22");
        b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let bar = StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: "Save".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(10, 10, 10),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };

        let surface = BitmapSurface::new(W, H);
        let painted = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            let l = backend.draw_status_bar_interactive(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &bar,
                &crate::InteractionState::new(),
            );
            *painted.borrow_mut() = Some(l);
        });
        b.end_frame();
        let painted = painted.into_inner().unwrap();

        let computed = b.status_bar_layout(Rect::new(0.0, 0.0, W as f32, H as f32), &bar);
        assert_eq!(
            painted.visible_segments.len(),
            computed.visible_segments.len(),
        );
        for (p, c) in painted
            .visible_segments
            .iter()
            .zip(computed.visible_segments.iter())
        {
            assert!((p.bounds.x - c.bounds.x).abs() < 0.001);
            assert!((p.bounds.width - c.bounds.width).abs() < 0.001);
        }
    }

    /// Issue #1003 acceptance test: paint a `TreeView` through the real
    /// `Backend::draw_tree` path and prove its painted row-label extent
    /// tracks `set_ui_font`, not `set_editor_font` — the headline symptom
    /// this issue was filed over (a file tree rendered in the editor's
    /// monospace font instead of the system UI font every other native
    /// app uses). Same shape as
    /// `GtkBackend`'s `gtk_backend_draw_tree_uses_ui_font_not_editor_font`
    /// (`gtk/backend.rs`): paint the same single-row tree under two
    /// wildly different *editor* font sizes with `ui_font` left at its
    /// default — the painted extent must be identical — then change
    /// `ui_font` alone as a positive control and see the extent move.
    ///
    /// The one place this can't copy its GTK twin verbatim is the row
    /// *geometry*. `GtkBackend` is **told** its metrics (`current_line_height`
    /// / `current_char_width` are pushed in by the host), so swapping the
    /// Pango layout's font description there changes only the glyphs.
    /// `MacBackend` **derives** them, inside `set_editor_font` — and row
    /// pitch (`line_height * 1.4`) plus the leaf row's own horizontal
    /// chevron reservation (`macos::tree::draw_tree`'s
    /// `cursor_x += line_height * 0.8`) both read that derived value, by
    /// design: row pitch tracking the editor line height is the
    /// deliberately-deferred half of #624/#1003 (see
    /// `MacBackend::draw_tree`'s comment), not part of the font swap under
    /// test. So the two cached metrics are pinned to fixed values after
    /// each `set_editor_font`, leaving the *glyph font* as the only thing
    /// that varies between calls. Without that pin, a `Menlo 60` editor
    /// font alone pushes row 0's height past the surface (the rasteriser
    /// then skips the clipped row entirely) and shifts the label's start
    /// x by ~40px — the test would swing on row layout, never reaching
    /// the question it was written to ask.
    #[test]
    fn draw_tree_uses_ui_font_not_editor_font() {
        use super::super::headless::BitmapSurface;
        use crate::types::{Decoration, SelectionMode, StyledText, TreeStyle};

        const W: u32 = 800;
        /// Tall enough for a whole `PINNED_LINE_HEIGHT * 1.4` row plus the
        /// 60pt positive-control glyph, so nothing under test is clipped.
        const H: u32 = 120;
        /// Pinned row metrics — see this test's doc comment.
        const PINNED_LINE_HEIGHT: f64 = 60.0;
        const PINNED_CHAR_WIDTH: f64 = 8.0;

        fn row_text_extent(editor: (&str, f32), ui_font: Option<&str>) -> u32 {
            let surface = BitmapSurface::new(W, H);
            surface.fill(1.0, 1.0, 1.0, 1.0);

            let mut b = MacBackend::new();
            Backend::set_editor_font(&mut b, editor.0, editor.1);
            // Pin row geometry *after* the editor font installed its own
            // derived metrics — see this test's doc comment for why.
            b.set_current_line_height(PINNED_LINE_HEIGHT);
            b.set_current_char_width(PINNED_CHAR_WIDTH);
            if let Some(f) = ui_font {
                Backend::set_ui_font(&mut b, f);
            }
            // White `tab_bar_bg`/`background` so `draw_tree`'s own
            // row-background fill doesn't itself read as "non-white" and
            // saturate the rightmost-painted-pixel scan below — mirrors
            // `GtkBackend`'s `gtk_backend_draw_tree_uses_ui_font_not_editor_font`.
            b.set_current_theme(crate::Theme {
                tab_bar_bg: Color::rgb(255, 255, 255),
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                ..crate::Theme::default()
            });
            b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

            let tree = TreeView {
                id: WidgetId::new("test:tree"),
                rows: vec![crate::primitives::tree::TreeRow {
                    path: vec![0],
                    indent: 0,
                    icon: None,
                    text: StyledText::plain("m".to_string()),
                    badge: None,
                    is_expanded: None,
                    decoration: Decoration::Normal,
                    edit: None,
                }],
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: TreeStyle::default(),
                has_focus: false,
            };
            b.enter_frame_scope(surface.context_ptr(), |backend| {
                backend.draw_tree(Rect::new(0.0, 0.0, W as f32, H as f32), &tree);
            });
            b.end_frame();

            // Rightmost non-white pixel anywhere on the surface — a
            // proxy for the painted label's glyph extent. The whole
            // surface is scanned rather than one chosen scanline
            // because the label is vertically centred in its row using
            // the *chrome* font's measured height, so an 11pt default
            // and a 60pt `ui_font` put ink on completely different rows;
            // a fixed probe line would measure "is there a glyph at
            // y=N", not "how wide is the glyph". Row background and
            // area fill are both white (see the theme above), so only
            // glyph ink can trip this.
            (0..W)
                .rev()
                .find(|&x| (0..H).any(|y| surface.pixel(x, y) != (255, 255, 255, 255)))
                .unwrap_or(0)
        }

        let small_editor_extent = row_text_extent(("Menlo", 8.0), None);
        let large_editor_extent = row_text_extent(("Menlo", 60.0), None);
        assert!(
            small_editor_extent.abs_diff(large_editor_extent) <= 1,
            "tree row glyph extent must be editor-font-size independent: \
             small_editor={small_editor_extent}, large_editor={large_editor_extent}"
        );

        let ui_font_extent = row_text_extent(("Menlo", 8.0), Some("Helvetica 60"));
        assert!(
            ui_font_extent > small_editor_extent + 20,
            "changing ui_font alone must visibly widen the painted row label: \
             default_ui_font={small_editor_extent}, ui_font_Helvetica_60={ui_font_extent}"
        );
    }

    /// Build a flat tree of `n_rows` leaf rows for `tree_vscrollbar`
    /// integration tests.
    fn flat_tree(n_rows: usize) -> TreeView {
        TreeView {
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

    /// #1043 regression: `tree_vscrollbar` used to pass the raw
    /// `line_height` (16px default) as `TreeView::vscrollbar`'s
    /// `row_height`, not the `line_height * 1.4` pitch `mac_tree_layout`
    /// actually paints for non-header rows. With `current_line_height`
    /// left at its 16px default, 15 rows painted at the real 22.4px→22px
    /// pitch only fit 13 into a 300px-tall viewport (300/22 = 13), so an
    /// overflow scrollbar must appear — the old raw-`line_height` math
    /// (300/16 = 18) would have wrongly reported "everything fits".
    #[test]
    fn mac_backend_tree_vscrollbar_uses_tree_layout_row_pitch_not_raw_line_height() {
        let backend = MacBackend::new();
        let tree = flat_tree(15);
        let rect = Rect::new(0.0, 0.0, 20.0, 300.0);

        let sb = Backend::tree_vscrollbar(&backend, rect, &tree)
            .expect("15 rows at the real 22px row pitch overflow a 300px viewport");

        let expected_row_h = ((backend.current_line_height * 1.4).round()) as f32;
        assert_eq!(
            sb.track.width, expected_row_h,
            "track width (and row pitch) must match tree_layout's non-header \
             item_height, not the raw line_height"
        );
    }

    /// #1043 / #623: a host-set `TreeStyle::row_height` must be honored
    /// by `tree_vscrollbar`, exactly as `mac_tree_layout`/`draw_tree`
    /// honor it — not silently ignored in favor of `line_height`.
    #[test]
    fn mac_backend_tree_vscrollbar_honors_tree_style_row_height_override() {
        let backend = MacBackend::new();
        let mut tree = flat_tree(15);
        tree.style.row_height = Some(50);
        let rect = Rect::new(0.0, 0.0, 20.0, 300.0);

        let sb = Backend::tree_vscrollbar(&backend, rect, &tree)
            .expect("15 rows at a 50px override overflow a 300px viewport (6 visible)");
        assert_eq!(sb.track.width, 50.0);
    }

    /// Issue #1003: `menu_bar_layout`/`draw_menu_bar` twin of
    /// `draw_tree_uses_ui_font_not_editor_font` above, using the returned
    /// `MenuBarLayout`'s measured item width instead of a pixel scan —
    /// `draw_menu_bar` hands back real geometry, so there is no need to
    /// rasterise and re-measure.
    #[test]
    fn draw_menu_bar_uses_ui_font_not_editor_font() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 800;
        const H: u32 = 30;

        fn item_width(editor: (&str, f32), ui_font: &str) -> f32 {
            let surface = BitmapSurface::new(W, H);
            let mut b = MacBackend::new();
            Backend::set_editor_font(&mut b, editor.0, editor.1);
            Backend::set_ui_font(&mut b, ui_font);
            b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

            let bar = MenuBar {
                id: WidgetId::new("test:menu-bar"),
                items: vec![crate::MenuBarItem {
                    id: WidgetId::new("test:menu-bar:file"),
                    label: "&File".to_string(),
                    disabled: false,
                    submenu: None,
                }],
                open_item: None,
                focused_item: None,
            };
            let width = std::cell::RefCell::new(0.0f32);
            b.enter_frame_scope(surface.context_ptr(), |backend| {
                let layout = backend.draw_menu_bar(Rect::new(0.0, 0.0, W as f32, H as f32), &bar);
                *width.borrow_mut() = layout.visible_items[0].bounds.width;
            });
            b.end_frame();
            width.into_inner()
        }

        let w_small_editor = item_width(("Menlo", 10.0), "Helvetica 11");
        let w_huge_editor = item_width(("Menlo", 80.0), "Helvetica 11");
        assert!(
            (w_small_editor - w_huge_editor).abs() < 0.5,
            "menu bar item width must be unaffected by set_editor_font: \
             {w_small_editor} vs {w_huge_editor}"
        );

        let w_small_chrome = item_width(("Menlo", 14.0), "Helvetica 8");
        let w_huge_chrome = item_width(("Menlo", 14.0), "Helvetica 60");
        assert!(
            w_huge_chrome > w_small_chrome * 2.0,
            "menu bar item width must grow with set_ui_font: {w_small_chrome} vs {w_huge_chrome}"
        );
    }

    /// Issue #1003 regression test for `draw_activity_bar` — the
    /// mandatory, non-default `Backend` method (see its own comment for
    /// why `draw_activity_bar_with_style`, which already had an
    /// equivalent guard's worth of review attention, doesn't cover this
    /// path: `AppShell::render` and `ScreenLayout::draw`'s
    /// `Surface::ActivityBar` arm both call this method directly, never
    /// the styled one). Same shape as
    /// `draw_tree_uses_ui_font_not_editor_font` above: paint the same
    /// single-row activity bar under two wildly different *editor* font
    /// sizes with `ui_font` left at its default — the painted icon
    /// glyph's horizontal extent must be identical — then change
    /// `ui_font` alone as a positive control and see it grow.
    #[test]
    fn draw_activity_bar_uses_ui_font_not_editor_font() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 200;
        const H: u32 = 60;

        fn icon_extent(editor: (&str, f32), ui_font: Option<&str>) -> u32 {
            let surface = BitmapSurface::new(W, H);
            surface.fill(1.0, 1.0, 1.0, 1.0);

            let mut b = MacBackend::new();
            Backend::set_editor_font(&mut b, editor.0, editor.1);
            if let Some(f) = ui_font {
                Backend::set_ui_font(&mut b, f);
            }
            // White `tab_bar_bg`/`background`, black `inactive_fg`, so
            // only glyph ink trips the pixel scan below — mirrors
            // `draw_tree_uses_ui_font_not_editor_font`'s theme override.
            b.set_current_theme(crate::Theme {
                tab_bar_bg: Color::rgb(255, 255, 255),
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                inactive_fg: Color::rgb(0, 0, 0),
                ..crate::Theme::default()
            });
            b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

            let bar = crate::ActivityBar {
                id: WidgetId::new("test:activity-bar"),
                top_items: vec![crate::ActivityItem {
                    id: WidgetId::new("test:activity:item"),
                    // `nerd_fonts_enabled` defaults to `false` (see
                    // `MacBackend::new`), so the rasteriser paints
                    // `fallback`, not `glyph` — a multi-character
                    // fallback makes the width swing more visible than
                    // a single glyph would.
                    icon: crate::types::Icon::new("x", "WWWW"),
                    tooltip: String::new(),
                    is_active: false,
                    is_keyboard_selected: false,
                }],
                bottom_items: vec![],
                active_accent: None,
                selection_bg: None,
                is_keyboard_focused: false,
            };
            b.enter_frame_scope(surface.context_ptr(), |backend| {
                backend.draw_activity_bar(Rect::new(0.0, 0.0, W as f32, H as f32), &bar, None);
            });
            b.end_frame();

            // Horizontal span between the leftmost and rightmost
            // non-white pixel anywhere on the surface — a proxy for the
            // painted icon glyph's width.
            let mut min_x: Option<u32> = None;
            let mut max_x: Option<u32> = None;
            for x in 0..W {
                for y in 0..H {
                    if surface.pixel(x, y) != (255, 255, 255, 255) {
                        min_x = Some(min_x.map_or(x, |m| m.min(x)));
                        max_x = Some(max_x.map_or(x, |m| m.max(x)));
                    }
                }
            }
            match (min_x, max_x) {
                (Some(lo), Some(hi)) => hi - lo,
                _ => 0,
            }
        }

        let small_editor_extent = icon_extent(("Menlo", 8.0), None);
        let large_editor_extent = icon_extent(("Menlo", 60.0), None);
        assert!(
            small_editor_extent.abs_diff(large_editor_extent) <= 1,
            "activity bar icon glyph extent must be editor-font-size independent: \
             small_editor={small_editor_extent}, large_editor={large_editor_extent}"
        );

        let ui_font_extent = icon_extent(("Menlo", 8.0), Some("Helvetica 60"));
        assert!(
            ui_font_extent > small_editor_extent + 20,
            "changing ui_font alone must visibly widen the painted activity bar icon: \
             default_ui_font={small_editor_extent}, ui_font_Helvetica_60={ui_font_extent}"
        );
    }
}
