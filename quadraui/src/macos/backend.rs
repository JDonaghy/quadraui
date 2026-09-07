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
use std::time::Duration;

use core_graphics::base::CGFloat;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;
use objc2::rc::Retained;
use objc2_app_kit::{NSCursor, NSEvent, NSWindow};

use crate::accelerator::{key_to_binding_name, parse_binding};
use crate::backend::{Backend, EditorPaintResult, PointerShape, ResizeEdge};
use crate::desktop::WindowDragArm;
use crate::dispatch::{DoubleClickDetector, DragState, TextRegion};
use crate::event::{Point, Rect, UiEvent, Viewport};
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
    Modifiers, Palette, ParsedBinding, PlatformServices, StatusBar, TabBar, Terminal, TextDisplay,
    Theme, TreeView,
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
///   per-app font state. Apps set these once in `setup()` via
///   [`Self::set_current_font`].
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
    /// Set once via [`Self::set_current_font`] during app setup.
    /// `draw_*` methods recover this for text rendering +
    /// measurement. Wrapped in `Option` so apps that don't paint
    /// text can skip the setup call.
    current_font: Option<CTFont>,
    current_line_height: f64,
    current_char_width: f64,
    /// Retained installer target from the last [`Backend::install_menu_bar`]
    /// call. Holds it alive so action selectors on installed `NSMenuItem`s
    /// don't dangle. Replaced wholesale on each re-install.
    menu_target: Option<objc2::rc::Retained<super::menu_bar_install::QuadraMenuTarget>>,
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
}

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
            current_font: None,
            current_line_height: 16.0,
            current_char_width: 8.0,
            menu_target: None,
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
        }
    }

    /// Store the top-level window handle. Called once by
    /// `macos::run::run` right after the window is constructed. Backs
    /// [`Backend::begin_window_drag`] / [`Backend::toggle_window_maximize`]
    /// / [`Backend::set_cursor`] (#498) — mirrors `GtkBackend::set_window`.
    pub(crate) fn set_window(&mut self, window: Retained<NSWindow>) {
        self.window = Some(window);
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
    pub fn set_current_font(&mut self, font: CTFont) {
        let metrics = super::text::font_metrics(&font);
        self.current_line_height = metrics.line_height;
        self.current_char_width = metrics.char_width;
        self.current_font = Some(font);
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

    fn poll_events(&mut self) -> Vec<UiEvent> {
        self.events.borrow_mut().drain(..).collect()
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

    fn install_menu_bar(&mut self, bar: &crate::primitives::menu_bar::MenuBar) {
        let mtm = objc2_foundation::MainThreadMarker::new()
            .expect("MacBackend::install_menu_bar must be called from the main thread");
        // Replacing wholesale — the previous target drops when this
        // assignment runs, after the new menu is installed.
        let target = super::menu_bar_install::install_menu_bar(mtm, bar, self.events.clone());
        self.menu_target = Some(target);
    }

    fn show_context_menu(
        &mut self,
        menu: &crate::primitives::context_menu::ContextMenu,
        anchor: crate::event::Point,
    ) {
        let mtm = objc2_foundation::MainThreadMarker::new()
            .expect("MacBackend::show_context_menu must be called from the main thread");
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
    /// - `file_dialogs` / `notifications`: `MacPlatformServices` uses real
    ///   `NSOpenPanel`/`NSSavePanel` and `osascript` notifications
    ///   (`src/macos/services.rs`), not stubs.
    /// - `native_dialogs`: **not** declared —
    ///   `MacPlatformServices::show_message_dialog` is still a `None`
    ///   stub pending an `NSAlert` implementation (quadraui#666); the
    ///   in-canvas `Dialog` primitive stays the only dialog path here
    ///   for now, same as TUI.
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
    /// - Everything else — `ime` — is **not** declared: no macOS IME
    ///   integration exists yet.
    fn backend_caps(&self) -> crate::backend::BackendCaps {
        crate::backend::BackendCaps {
            mouse: true,
            scroll: true,
            drag: true,
            native_menu: true,
            file_dialogs: true,
            notifications: true,
            window_chrome: true,
            pointer_cursor: true,
            text_selection: true,
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

    fn line_height(&self) -> f32 {
        self.current_line_height as f32
    }

    fn char_width(&self) -> f32 {
        self.current_char_width as f32
    }

    // ── Drawing ────────────────────────────────────────────────────

    fn draw_tree(&mut self, rect: Rect, tree: &TreeView) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tree called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_tree requires set_current_font");
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_list requires set_current_font");
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
        let theme = self.current_theme;
        let line_height = self.current_line_height as f32;
        crate::primitives::status_bar::native_surface_paint::paint(
            bar,
            self,
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
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tab_bar called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_tab_bar requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: `ctx` is non-null inside the frame scope.
        unsafe {
            super::tab_bar::draw_tab_bar(
                ctx,
                font,
                rect.width as f64,
                line_height,
                rect.y as f64,
                rect.height as f64,
                bar,
                &theme,
                hovered_close_tab,
            )
        }
    }
    /// Per-tab icon glyphs (#620) are **not implemented on macOS yet** —
    /// `mac_tab_bar_layout` has no CoreText icon-width pass, and shipping
    /// a paint-only version would put every close-button hit box left of
    /// the glyph it draws. This forwards to the icon-less rasteriser so
    /// a macOS app that passes icons still paints correct (if
    /// undecorated) tabs, and fires a `debug_assert!` so the gap is loud
    /// in development rather than a silently-missing glyph.
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn draw_tab_bar_icons(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits {
        debug_assert!(
            icons.iter().all(Option::is_none),
            "MacBackend: TabIcon glyphs are not implemented yet (#620 follow-up); \
             tabs will paint without their icons",
        );
        self.draw_tab_bar(rect, bar, hovered_close_tab)
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_activity_bar requires set_current_font");
        let theme = self.current_theme;
        // SAFETY: ctx non-null inside frame scope.
        unsafe {
            super::activity_bar::draw_activity_bar(
                ctx,
                font,
                rect.width as f64,
                rect.height as f64,
                bar,
                &theme,
                hovered_idx,
                self.nerd_fonts_enabled,
            )
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_activity_bar_with_style requires set_current_font");
        let theme = self.current_theme;
        // SAFETY: ctx non-null inside frame scope.
        unsafe {
            super::activity_bar::draw_activity_bar_with_style(
                ctx,
                font,
                rect.width as f64,
                rect.height as f64,
                bar,
                style,
                &theme,
                hovered_idx,
                self.nerd_fonts_enabled,
            )
        }
    }

    fn status_bar_layout(&self, rect: Rect, bar: &StatusBar) -> StatusBarLayout {
        // No-paint twin of `draw_status_bar`: same `mac_status_bar_layout`
        // call, so hit regions match the painted frame exactly. Hit
        // regions are bar-local, so `rect.x` / `rect.y` are deliberately
        // not folded in (quadraui#552 — audited, no change needed).
        match self.current_font.as_ref() {
            Some(font) => super::status_bar::mac_status_bar_layout(
                font,
                rect.width as f64,
                self.current_line_height,
                bar,
            ),
            // Called before `set_current_font` (e.g. a click handler
            // firing before the first paint): fall back to the backend's
            // seeded `char_width` rather than panicking, matching the
            // `sidebar_panel_layout` precedent below.
            None => {
                let cw = self.current_char_width as f32;
                bar.layout(
                    rect.width,
                    self.current_line_height as f32,
                    super::status_bar::MIN_GAP_PX,
                    |seg| crate::StatusSegmentMeasure::new(seg.text.chars().count() as f32 * cw),
                )
            }
        }
    }

    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout(&self, rect: Rect, bar: &TabBar) -> TabBarHits {
        // No-paint twin of `draw_tab_bar`, routed through the same
        // `mac_tab_bar_layout`. See that function's docs for why macOS
        // returns bar-relative (not absolute) x, and why closing that
        // #552 gap is a paint change left to a follow-up.
        match self.current_font.as_ref() {
            Some(font) => super::tab_bar::mac_tab_bar_layout(font, rect.width as f64, bar),
            None => TabBarHits {
                slot_positions: vec![(0.0, 0.0); bar.tabs.len()],
                close_bounds: vec![None; bar.tabs.len()],
                right_segment_bounds: vec![(0.0, 0.0); bar.right_segments.len()],
                available_cols: 0,
                correct_scroll_offset: bar.scroll_offset,
            },
        }
    }

    /// No-paint twin of [`Self::draw_tab_bar_icons`] — and, like it, an
    /// icon-less forward until macOS grows a CoreText icon-width pass
    /// (#620 follow-up). Keeping both halves icon-blind is what preserves
    /// the load-bearing macOS invariant that `tab_bar_layout` returns
    /// exactly what `draw_tab_bar` painted.
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout_icons(
        &self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
    ) -> TabBarHits {
        debug_assert!(
            icons.iter().all(Option::is_none),
            "MacBackend: TabIcon glyphs are not implemented yet (#620 follow-up); \
             layout reserves no icon width",
        );
        self.tab_bar_layout(rect, bar)
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
    /// #810: shared text-display painting lives in
    /// [`crate::primitives::text_display::paint`] now — see that fn's
    /// doc for the divergence resolved while unifying
    /// `gtk::text_display::draw_text_display`,
    /// `macos::text_display::draw_text_display` and
    /// `win::text_display::draw_text_display` into one implementation.
    fn draw_text_display(&mut self, rect: Rect, td: &TextDisplay) {
        let theme = self.current_theme;
        let line_height = self.current_line_height as f32;
        crate::primitives::text_display::paint(td, rect, self, &theme, line_height);
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
        super::text_display::mac_text_display_layout(td, rect, self.current_line_height)
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_context_menu requires set_current_font");
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_dialog requires set_current_font");
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_multi_section_view requires set_current_font");
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
        super::tree::mac_tree_layout(tree, rect, self.current_line_height)
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_rich_text_popup requires set_current_font");
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_menu_bar requires set_current_font");
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::menu_bar_layout requires set_current_font");
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_command_center requires set_current_font");
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::command_center_layout requires set_current_font");
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
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_toolbar requires set_current_font");
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
        // Layout-only path: prefer the live font when present, else
        // synthesise widths from `char_width` to keep the contract
        // honest without forcing apps to pre-set a font.
        if let Some(font) = self.current_font.as_ref() {
            super::toolbar::mac_toolbar_layout(
                bar,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
            )
        } else {
            let cw = self.current_char_width as f32;
            bar.layout(rect.x, rect.y, rect.width, rect.height, |btn| {
                let chars = match btn {
                    crate::primitives::toolbar::ToolbarButton::Action {
                        label,
                        icon,
                        key_hint,
                        ..
                    } => {
                        let icon_w = icon.as_ref().map(|s| s.chars().count() + 1).unwrap_or(0);
                        let hint_w = key_hint
                            .as_ref()
                            .map(|s| s.chars().count() + 3)
                            .unwrap_or(0);
                        icon_w + label.chars().count() + hint_w
                    }
                    crate::primitives::toolbar::ToolbarButton::Separator => 2,
                    crate::primitives::toolbar::ToolbarButton::Label { text, .. } => {
                        text.chars().count()
                    }
                };
                crate::primitives::toolbar::ToolbarItemMeasure::new(chars as f32 * cw)
            })
        }
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
        debug_assert!(
            self.current_font.is_some(),
            "MacBackend::draw_sidebar_panel requires set_current_font",
        );
        let theme = self.current_theme;
        let line_height = self.current_line_height as f32;
        crate::primitives::sidebar_panel::native_surface_paint::paint(
            panel,
            self,
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
        if let Some(font) = self.current_font.as_ref() {
            super::sidebar_panel::mac_sidebar_panel_layout(
                panel,
                font,
                self.current_line_height,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
            )
        } else {
            // No font yet (called before first draw) — produce the
            // layout using the toolbar_layout fallback path. Hosts
            // that need accurate measurement must call this from
            // inside a frame scope (or after `set_current_font`).
            let bounds = crate::event::Rect::new(rect.x, rect.y, rect.width, rect.height);
            panel.layout(
                bounds,
                crate::primitives::sidebar_panel::SidebarPanelMeasure::new(
                    self.current_line_height as f32,
                    self.current_char_width as f32,
                ),
                |_btn| crate::primitives::toolbar::ToolbarItemMeasure::new(0.0),
            )
        }
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
    /// / `truncate_to_columns`) — a future macOS rasteriser consumes those
    /// directly, same as `gtk::minimap` and `win::minimap` do. What's left
    /// for that future rasteriser is the actual Core Graphics/Core Text
    /// paint calls (fill rects, `CTLine` glyph runs, the clip bracket) —
    /// the same shape and size of backend-specific work #738 did for
    /// Win-GUI, not a shim over shared logic.
    ///
    /// Until that lands, this used to be a reachable `todo!()` — a panic
    /// an app calling `Backend::draw_minimap` generically could hit on
    /// macOS alone, the exact inverse of the four-backend promise (#802).
    /// `super::minimap::mac_minimap_layout` computes the real
    /// [`MinimapLayout`](crate::primitives::minimap::MinimapLayout) (the
    /// same `Minimap::layout_with_sizing` call GTK/Win-GUI make, not a
    /// stub), so hit-testing/click-routing already works correctly; only
    /// [`MinimapPaintResult::painted`](crate::backend::MinimapPaintResult::painted)
    /// comes back `false` in place of pixels.
    fn draw_minimap(
        &mut self,
        rect: Rect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::backend::MinimapPaintResult {
        let layout = super::minimap::mac_minimap_layout(minimap, rect);
        self.register_zone(minimap.id.clone(), rect);
        crate::backend::MinimapPaintResult {
            layout,
            painted: false,
        }
    }

    /// Real geometry regardless of the paint gap above — see
    /// [`Self::draw_minimap`]'s doc comment.
    fn minimap_layout(
        &self,
        rect: Rect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::primitives::minimap::MinimapLayout {
        super::minimap::mac_minimap_layout(minimap, rect)
    }

    /// #662 scopes the `Image` rasteriser to GTK only for this first
    /// pass — macOS's natural decoder is `NSImage`, not `gdk_pixbuf`, and
    /// wiring that up is real, backend-specific work, not a shim over
    /// shared logic, the same way `draw_minimap` above is scoped out of
    /// #382. Until a real `NSImage` decoder lands here, this used to be
    /// a reachable `todo!()` — a panic an app calling
    /// `Backend::draw_image` generically could hit on macOS alone (#802).
    /// `super::image::mac_draw_image` reports
    /// [`ImagePaintResult::Unsupported`](crate::backend::ImagePaintResult::Unsupported)
    /// instead — the same signal TUI already uses for its own categorical
    /// "no pixel grid" case — so a host degrades deliberately rather than
    /// losing the whole app.
    fn draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        self.register_zone(image.id.clone(), rect);
        super::image::mac_draw_image()
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

    #[test]
    fn set_cursor_false_without_window() {
        let mut b = MacBackend::new();
        assert!(!Backend::set_cursor(&mut b, PointerShape::Default));
        assert!(!Backend::set_cursor(
            &mut b,
            PointerShape::Resize(ResizeEdge::North)
        ));
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

    // ── #802: Minimap/Image must not panic on macOS ─────────────────────
    //
    // Before this issue, `MacBackend::draw_minimap`/`draw_image` were
    // `todo!()` — any `AppLogic` calling either generically through `&mut
    // dyn Backend` (exactly what `examples/common/minimap_app.rs` /
    // `image_app.rs` do, the same fixtures `tests/macos_example_driver.rs`
    // now drives) panicked and took the whole host down on macOS while
    // working fine on TUI/GTK/Win-GUI. Observed RED before this fix: both
    // methods hit their `todo!()` immediately, with no cfg gate to skip
    // past on this (non-macOS) machine, so the confirmation here is
    // structural — the calls below no longer reach a `todo!()`/
    // `unimplemented!()` macro anywhere in their path — rather than a
    // captured panic backtrace, which only `macos-latest` CI can produce
    // for this target-gated module (see `CLAUDE.md`'s Downstream
    // consumers / macOS sections).

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

    #[test]
    fn draw_minimap_does_not_panic_and_reports_unpainted() {
        let mut b = MacBackend::new();
        let minimap = sample_minimap();
        let rect = Rect::new(0.0, 0.0, 20.0, 100.0);

        let result = b.draw_minimap(rect, &minimap);

        assert!(
            !result.painted,
            "macOS has no Core Graphics/Core Text minimap rasteriser yet (#382) -- \
             `painted` must honestly report `false`, not panic"
        );
        assert!(
            !result.layout.visible_lines.is_empty(),
            "the layout must still be real geometry, not an empty stub"
        );
        assert!(
            b.zones().iter().any(|z| z.id == minimap.id),
            "a click zone must still be registered so a host can route clicks \
             even though nothing painted"
        );
    }

    /// `minimap_layout` (the no-paint query `AppLogic::handle` calls for
    /// click routing) must agree with the layout `draw_minimap` just
    /// returned — same contract every other backend upholds.
    #[test]
    fn minimap_layout_agrees_with_draw_minimap() {
        let mut b = MacBackend::new();
        let minimap = sample_minimap();
        let rect = Rect::new(0.0, 0.0, 20.0, 100.0);

        let painted = b.draw_minimap(rect, &minimap);
        let layout_only = b.minimap_layout(rect, &minimap);

        assert_eq!(painted.layout, layout_only);
    }

    #[test]
    fn draw_image_does_not_panic_and_reports_unsupported() {
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
            "macOS has no NSImage decoder yet (#662's first pass, #802) -- this must \
             be a clean Unsupported result, not a panic"
        );
        assert!(
            b.zones().iter().any(|z| z.id == image.id),
            "a click/hover zone must still be registered even though nothing painted"
        );
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
            // macOS categorically returns `Unsupported` here until #802
            // lands a real `NSImage` decoder — this call only needs to
            // prove it reaches the same code `Backend::draw_image` does,
            // not any particular decode outcome.
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
}
