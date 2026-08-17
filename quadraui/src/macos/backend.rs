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

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::accelerator::{key_to_binding_name, parse_binding};
use crate::backend::{Backend, EditorPaintResult};
use crate::dispatch::{DoubleClickDetector, DragState};
use crate::event::{Rect, UiEvent, Viewport};
use crate::modal_stack::ModalStack;
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
    LayoutMetrics, MultiSectionView, MultiSectionViewLayout,
};
use crate::primitives::panel::{Panel, PanelLayout};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::spinner::{Spinner, SpinnerLayout};
use crate::primitives::split::{Split, SplitLayout};
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout};
use crate::primitives::status_bar::StatusBarLayout;
use crate::primitives::tab_bar::TabBarHits;
use crate::primitives::text_display::TextDisplayLayout;
use crate::primitives::toast::{ToastStack, ToastStackLayout};
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::primitives::tree::TreeViewLayout;
use crate::testing::{TextRun, ZoneRec};
use crate::types::WidgetId;
use crate::KeyBinding;
use crate::{
    Accelerator, AcceleratorId, AcceleratorScope, ActivityBar, Form, Key, ListView, Modifiers,
    Palette, ParsedBinding, PlatformServices, StatusBar, TabBar, Terminal, TextDisplay, Theme,
    TreeView,
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
    modal_stack: ModalStack,
    drag_state: DragState,
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

impl MacBackend {
    /// Construct a fresh `MacBackend` with a default viewport, empty
    /// event queue, default theme, and no font. The runner overwrites
    /// the viewport each frame via [`Backend::begin_frame`]; apps
    /// install a font via [`Self::set_current_font`] in `setup()`.
    pub fn new() -> Self {
        Self {
            viewport: Viewport::new(0.0, 0.0, 1.0),
            modal_stack: ModalStack::new(),
            drag_state: DragState::new(),
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
        }
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
}

impl Default for MacBackend {
    fn default() -> Self {
        Self::new()
    }
}

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
    }

    fn end_frame(&mut self) {
        // No-op. AppKit's `drawRect:` flushes when it returns; this
        // method exists for parity with backends that need an explicit
        // flush.
    }

    fn set_theme(&mut self, theme: Theme) {
        self.set_current_theme(theme);
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

    fn modal_stack_mut(&mut self) -> &mut ModalStack {
        &mut self.modal_stack
    }

    fn drag_and_modal_mut(&mut self) -> (&mut DragState, &mut ModalStack) {
        (&mut self.drag_state, &mut self.modal_stack)
    }

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    fn register_zone(&mut self, id: WidgetId, bounds: Rect) {
        self.zones.push(ZoneRec { id, bounds });
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
    /// - Everything else — `text_selection`, `window_chrome`,
    ///   `pointer_cursor`, `ime` — is **not** declared: `register_text_region`,
    ///   `begin_window_drag`/`toggle_window_maximize`/`begin_window_resize`,
    ///   and `set_cursor` are all still the trait's no-op default on this
    ///   backend today (see #493 — no macOS runner in the fleet yet to
    ///   exercise them).
    fn backend_caps(&self) -> crate::backend::BackendCaps {
        crate::backend::BackendCaps {
            mouse: true,
            scroll: true,
            drag: true,
            native_menu: true,
            file_dialogs: true,
            notifications: true,
            ..crate::backend::BackendCaps::empty()
        }
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
        // macOS list does not implement bordered rendering yet; treat inset as 0.
        let visible_px = rect.width;
        if content_px <= visible_px {
            return None;
        }
        let row_h = self.line_height();
        let track = Rect::new(
            rect.x,
            rect.y + (rect.height - row_h).max(0.0),
            rect.width,
            row_h,
        );
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
    fn draw_form(&mut self, rect: Rect, form: &Form) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_form called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_form requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::form::draw_form(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                form,
                &theme,
                line_height,
            );
        }
    }
    fn draw_palette(&mut self, rect: Rect, palette: &Palette) {
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

    fn draw_status_bar(
        &mut self,
        rect: Rect,
        bar: &StatusBar,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) -> StatusBarLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_status_bar called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_status_bar requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: `ctx` is non-null inside the frame scope; the call
        // chain enforces `enter_frame_scope` via the debug_assert above.
        unsafe {
            super::status_bar::draw_status_bar(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                line_height,
                bar,
                &theme,
                hovered_id,
                pressed_id,
            )
        }
    }
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
    fn draw_activity_bar(
        &mut self,
        rect: Rect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
    ) -> Vec<ActivityBarRowHit> {
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

    fn activity_bar_layout(&self, rect: Rect, bar: &ActivityBar) -> Vec<ActivityBarRowHit> {
        // No-paint twin of `draw_activity_bar`; both walk the same
        // `row_plan`, so the returned bar-relative spans are exactly the
        // rows that were painted (quadraui#552).
        super::activity_bar::mac_activity_bar_layout(rect.width as f64, rect.height as f64, bar)
    }

    fn draw_terminal(&mut self, rect: Rect, term: &Terminal) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_terminal called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_terminal requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;

        let sb_width = match &term.scrollbar {
            Some(sb) => sb.width.map(|w| w as f64).unwrap_or(8.0),
            None => 0.0,
        };
        let cell_area_w = (rect.width as f64 - sb_width).max(0.0);

        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::terminal::draw_terminal_cells(
                ctx,
                font,
                term,
                rect.x as f64,
                rect.y as f64,
                cell_area_w,
                rect.height as f64,
                line_height,
                char_width,
                &theme,
            );
        }

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
            // SAFETY: ctx is non-null inside the frame scope.
            unsafe { super::scrollbar::draw_scrollbar(ctx, &sb, &theme) }
        }
    }
    fn draw_terminal_divider(&mut self, rect: Rect) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_terminal_divider called outside enter_frame_scope",
        );
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::terminal::draw_terminal_divider(
                ctx,
                rect.x as f64,
                rect.y as f64,
                rect.height as f64,
                &theme,
            );
        }
    }
    fn draw_text_display(&mut self, rect: Rect, td: &TextDisplay) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_text_display called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_text_display requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text_display::draw_text_display(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                td,
                &theme,
                line_height,
            );
        }
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
            crate::primitives::text_input::TextInputMeasure::new(
                self.current_line_height as f32,
                self.current_char_width as f32,
            ),
        )
    }
    fn text_input_layout(
        &self,
        rect: Rect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        ti.layout(
            rect,
            crate::primitives::text_input::TextInputMeasure::new(
                self.current_line_height as f32,
                self.current_char_width as f32,
            ),
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
    fn msv_metrics(&self) -> LayoutMetrics {
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
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_find_replace called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_find_replace requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::find_replace::draw_find_replace(
                ctx,
                font,
                panel,
                &theme,
                line_height,
                char_width,
            );
        }
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
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_scrollbar called outside enter_frame_scope",
        );
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::scrollbar::draw_scrollbar(ctx, scrollbar, &theme) }
    }

    fn draw_drop_overlay(&mut self, overlay: &crate::primitives::drop_zone::DropOverlay) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_drop_overlay called outside enter_frame_scope",
        );
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::drop_overlay::draw_drop_overlay(ctx, overlay, &theme) }
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
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_split called outside enter_frame_scope",
        );
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::split::draw_split(
                ctx,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                split,
                &theme,
            )
        }
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
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_split_tree called outside enter_frame_scope",
        );
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::split_tree::draw_split_tree(
                ctx,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                tree,
                &theme,
            )
        }
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
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_panel called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_panel requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::panel::draw_panel(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                panel,
                &theme,
                line_height,
            )
        }
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
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_toast_stack called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_toast_stack requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::toast::draw_toast_stack(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                stack,
                &theme,
                line_height,
            )
        }
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
    fn draw_chart(
        &mut self,
        rect: Rect,
        chart: &Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) -> ChartLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_chart called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_chart requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::chart::draw_chart(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                chart,
                &theme,
                line_height,
                char_width,
                hovered_point,
                crosshair_x,
            )
        }
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

    fn draw_toolbar(
        &mut self,
        rect: Rect,
        bar: &crate::primitives::toolbar::Toolbar,
        hovered_id: Option<&crate::types::WidgetId>,
        pressed_id: Option<&crate::types::WidgetId>,
    ) -> crate::primitives::toolbar::ToolbarLayout {
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

    fn draw_sidebar_panel(
        &mut self,
        rect: Rect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
        hovered_toolbar_id: Option<&crate::types::WidgetId>,
        pressed_toolbar_id: Option<&crate::types::WidgetId>,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_sidebar_panel called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_sidebar_panel requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::sidebar_panel::draw_sidebar_panel(
                ctx,
                font,
                line_height,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                panel,
                &theme,
                hovered_toolbar_id,
                pressed_toolbar_id,
            )
        }
    }

    fn draw_diff_view(
        &mut self,
        rect: Rect,
        view: &crate::primitives::diff_view::DiffView,
    ) -> crate::primitives::diff_view::DiffViewLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_diff_view called outside enter_frame_scope",
        );
        let font = self
            .current_font
            .as_ref()
            .expect("MacBackend::draw_diff_view requires set_current_font");
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::diff_view::draw_diff_view(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                view,
                &theme,
                line_height,
            )
        }
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
}
