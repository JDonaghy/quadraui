//! TUI implementation of [`crate::Backend`].
//!
//! `TuiBackend` owns the persistent UI state the trait requires —
//! viewport dimensions, modal stack, drag state, accelerator registry,
//! platform services — plus a transient frame pointer set inside
//! [`Self::enter_frame_scope`] so trait `draw_*` methods can reach
//! the ratatui `&mut Frame<'_>` (which only exists inside
//! `terminal.draw(|frame| …)`'s closure).
//!
//! ### Frame-scope mechanism
//!
//! ratatui's `terminal.draw(|frame| …)` API only yields `&mut Frame`
//! inside the closure, so `TuiBackend` can't hold one across method
//! calls. Instead [`Self::enter_frame_scope`] takes the frame, stashes
//! a type-erased `*mut ()` in `current_frame_ptr`, runs the caller's
//! closure (where trait `draw_*` methods can reach the frame via
//! [`Self::current_frame_mut`]), and clears the pointer on exit.
//! The pointer is null outside the scope, so the safe accessor
//! returns `None` and `draw_*` methods can detect misuse.
//!
//! ### What the trait covers
//!
//! As of #13 the `Backend` trait covers every primitive that has
//! TUI + GTK rasterisers. New primitives must add a trait method as
//! part of the same change that adds the rasteriser (see CLAUDE.md
//! Primitive Authoring Rule #7). Generic `<B: Backend>` render code
//! works against `TuiBackend`, `GtkBackend`, the test `MockBackend`,
//! and any future Win-GUI / macOS backend implementer.
//!
//! Drag-state observation is normally a backend implementation
//! detail — only `crate::dispatch::*` needs to inspect it directly.
//! It's still reachable via [`crate::Backend::drag_state_handle`]
//! (#467, #699, #704) so `&mut dyn Backend` / `&dyn Backend`
//! consumers (e.g. a `ShellApp`'s mouse-dispatch logic) can reach it
//! without downcasting to the concrete backend.
//!
//! Event flow goes through the trait: [`Self::wait_events`] reads
//! crossterm events, translates them via
//! [`super::events::crossterm_to_uievents`], then runs
//! [`Self::apply_accelerators`] to rewrite registered key bindings as
//! [`UiEvent::Accelerator`] before returning. The event loop in
//! [`super::event_loop`] consumes those `UiEvent`s via
//! [`Backend::wait_events`].

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::accelerator::{key_to_binding_name, parse_binding};
use crate::backend::{activity_bar_hits, tab_bar_hits_from_layout, ColorDepth};
use crate::dispatch::TextRegion;
use crate::testing::ZoneRec;
use crate::{
    Accelerator, AcceleratorId, AcceleratorScope, ActivityBar, Backend, CommandLine, DragState,
    DragTarget, Form, ListView, MenuBar, ModalStack, Palette, ParsedBinding, PlatformServices,
    Point, Rect as QRect, Split, StatusBar, TabBar, TabBarLayout, TabChrome, TabFrame,
    Terminal as TerminalPrim, TextDisplay, TreeView, UiEvent, Viewport, WidgetId,
};
// `KeyBinding` is only referenced by `#[cfg(test)]` code below (the rest of
// this file matches already-parsed `Accelerator`s) — gate the import the
// same way so a non-test build doesn't flag it as unused.
#[cfg(test)]
use crate::KeyBinding;
use ratatui::layout::Rect;
use ratatui::Frame;

use super::services::TuiPlatformServices;
use super::text::display_width;

/// Minimum gap (in cells) between left and right status-bar halves
/// before priority drop kicks in. Mirrors `crate::gtk::status_bar`'s
/// `MIN_GAP_PX = 16.0`. Irrelevant for bars without right segments.
const MIN_GAP_CELLS: f32 = 2.0;

/// TUI backend implementing [`crate::Backend`].
///
/// Owns the persistent UI state the trait requires plus a transient
/// "current frame" pointer + theme set inside
/// [`Self::enter_frame_scope`]. The pointer is type-erased
/// (`*mut ()`) and cleared on scope exit; safe accessors deref it
/// only while the scope is active.
///
/// The ratatui `Terminal` is **not** owned here — it stays as a local
/// in [`super::event_loop`]. See `docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §11 for
/// rationale and the eventual migration plan.
pub struct TuiBackend {
    viewport: Viewport,
    /// `Rc<RefCell<>>` (not a plain field) so [`Backend::modal_stack_handle`]
    /// can hand back a handle that outlives any single `&mut self` borrow
    /// — the same shape `GtkBackend` uses for its own callback-driven
    /// access (quadraui#699).
    modal_stack: Rc<RefCell<ModalStack>>,
    /// See `modal_stack`'s doc comment — same rationale.
    drag_state: Rc<RefCell<DragState>>,
    accelerators: HashMap<AcceleratorId, Accelerator>,
    /// Pre-parsed bindings, kept in lock-step with `accelerators`. Stage 6
    /// uses this for the `wait_events`/`poll_events` matcher to avoid
    /// re-parsing on every keystroke. First-match-wins iteration order
    /// matches insertion order (`Vec`, not `HashMap`).
    parsed_accelerators: Vec<(ParsedBinding, AcceleratorId)>,
    services: TuiPlatformServices,
    /// Type-erased `&mut Frame<'_>` pointer; non-null only inside
    /// [`Self::enter_frame_scope`]. `Cell` (not `RefCell`) because
    /// trait methods borrow `&mut self` already; we only need
    /// shared-cell semantics for `Copy`-able pointer values.
    current_frame_ptr: Cell<*mut ()>,
    /// Theme captured by the most recent
    /// [`Self::set_current_theme`] call. Defaults to
    /// `crate::Theme::default()` until set.
    current_theme: crate::Theme,
    /// Whether the rasterisers should use Nerd Font glyphs (`true`)
    /// or ASCII fallbacks (`false`). Apps set this from their own
    /// settings via [`Self::set_nerd_fonts`]; defaults to `false` —
    /// see that trait method's doc for why every backend now agrees
    /// on this default (issue #683; was `true` here and `false` on
    /// GTK, so the same app showed a different icon variant depending
    /// on which backend it launched under).
    nerd_fonts_enabled: bool,
    double_click: super::events::DoubleClickDetector,
    /// Whether [`Self::translate_injected`] should run raw `MouseDown`
    /// pairs through [`Self::double_click`]'s folding logic. Defaults to
    /// `true` (matches production `poll_events`/`wait_events`, which
    /// always fold). [`super::testing::TuiDriver::set_double_click_folding`]
    /// flips this to `false` so a test that means two distinct clicks
    /// (quadraui#592) isn't at the mercy of `Instant::now()` landing two
    /// calls inside the 400ms window under load. Only `translate_injected`
    /// (the driver's path) honours this — `poll_events`/`wait_events`
    /// (the live terminal runner) always fold, since a real user's two
    /// clicks should still coalesce.
    double_click_folding: bool,
    /// Widget zones registered during the current frame via
    /// [`crate::Backend::register_zone`]. Cleared at the start of each
    /// frame by [`Self::begin_frame`], mirroring `text_regions`. Read by
    /// [`crate::tui::testing::TuiDriver::inventory`] to populate
    /// [`crate::testing::FrameInventory::zones`] (quadraui#490).
    zones: Vec<ZoneRec>,
    /// Region registry + active-selection state shared by every
    /// `text_selection: true` backend (#741) — see
    /// [`crate::text_selection::TextSelectionState`]'s doc. Replaces the
    /// formerly TUI-local `text_regions`/`active_selection`/
    /// `last_text_region_id` fields verbatim; [`Self::apply_dispatch`] (a
    /// `MouseDown` starting a `DragTarget::TextSelection` drag) is the
    /// other call site that updates it, via `track_focused_text_region`.
    text_selection: crate::text_selection::TextSelectionState,
    /// Text extracted from the last rendered buffer for the active
    /// selection. Populated by `apply_selection_highlight` (which has
    /// access to the live buffer inside `terminal.draw`). After the
    /// draw closure returns ratatui swaps its double-buffer, so
    /// `terminal.current_buffer_mut()` would return an empty buffer —
    /// caching here is the only reliable way to get the text.
    cached_selection_text: String,
    /// `WidgetId` of the `ActivityBar` that declared `is_keyboard_focused`
    /// during the most recent render pass. Set by `draw_activity_bar`;
    /// cleared at the start of each frame by `begin_frame` (same lifecycle
    /// as `text_regions`). When `Some`, `apply_dispatch` converts incoming
    /// `KeyPressed` events to `UiEvent::ActivityBar(id, KeyPressed { … })`
    /// so app logic receives activity-bar keys through a typed channel.
    focused_activity_bar: Option<WidgetId>,
    /// Screen-space `(x, y)` for `Frame::set_cursor_position`, cached from
    /// the most recent [`Backend::draw_editor`] call this frame (quadraui#466).
    ///
    /// `draw_editor` only has access to the ratatui buffer, not the `Frame`
    /// itself (see [`Self::current_frame_mut`]'s note on why trait methods
    /// can't reach `Frame::set_cursor_position` directly), so it stashes
    /// the position here. [`super::run::render_frame`] takes it via
    /// [`Self::take_last_cursor_position`] after the render closure returns
    /// and applies it to the real `Frame`, mirroring how
    /// `apply_selection_highlight` bridges buffer-only paint to a
    /// frame-level side effect.
    ///
    /// Cleared at the start of every frame by [`Self::begin_frame`] (same
    /// lifecycle as `text_regions`) so a frame that doesn't paint an editor
    /// doesn't inherit a stale cursor position from the previous one.
    last_cursor_position: Option<(u16, u16)>,
    /// `(bar rect, resolved layout)` from the most recent `draw_tab_bar`
    /// call, per tab-bar `WidgetId`. Cleared at the start of every frame
    /// by [`Self::begin_frame`] (same lifecycle as `zones`) so a tab bar
    /// that stops painting doesn't leave a stale entry that would resolve
    /// a tab no longer on screen.
    ///
    /// `TabBarLayout`'s own `visible_tabs`/`close_bounds` rects are
    /// bar-relative (origin at the bar's own top-left) — the same
    /// convention `crate::tui::draw_tab_bar` paints with (it adds
    /// `area.x`/`area.y` itself). [`Self::cached_tab_bar_layout`] hands
    /// back the paired rect so callers can shift into absolute
    /// screen-space coordinates. Read by
    /// [`crate::tui::testing::TuiDriver::tab_center`] /
    /// [`crate::tui::testing::TuiDriver::tab_close_center`] (quadraui#594)
    /// — every tab paints the same close glyph, so `find` can't
    /// disambiguate tab 3's target from tab 0's the way it can for
    /// uniquely-labeled text.
    tab_bar_layouts: HashMap<WidgetId, (QRect, TabBarLayout)>,
    /// Colour fidelity this backend's rasterisers should quantise to —
    /// see [`crate::backend::ColorDepth`]. Detected from the process
    /// environment (`COLORTERM`/`TERM`) at construction time via
    /// [`super::caps::detect_color_depth`]; overridable via
    /// [`Self::set_color_depth`] for tests and hosts that already know
    /// their terminal's real capability (quadraui#826).
    color_depth: ColorDepth,
    /// Whether the kitty keyboard protocol is actually active — seeded
    /// from [`super::caps::detect_kitty_keyboard`]'s cheap environment
    /// heuristic at construction time, then overwritten by
    /// [`super::run::run`] via [`Self::set_kitty_keyboard`] once it has
    /// [`super::caps::probe_kitty_keyboard`]'s live, authoritative answer
    /// (quadraui#827). Exposed to apps via
    /// [`crate::backend::BackendCaps::kitty_keyboard`] — see that field's
    /// doc for why this exists.
    kitty_keyboard: bool,
    /// Whether mouse reporting is actually active for this session.
    /// Defaults to `true`; [`super::run::run_with`] sets this to `false`
    /// when the caller opts into `RunConfig { mouse: false, .. }` — the
    /// "capture refused" `no-mouse` mode (quadraui#828) — so the terminal
    /// never gets `EnableMouseCapture` and every Tier-1 gesture must have
    /// a key path instead.
    ///
    /// Deliberately **not** folded into
    /// [`crate::backend::BackendCaps::mouse`]/`scroll`/`drag`: those three
    /// are a static per-backend-*type* fact
    /// (`tests/conformance/caps.rs`'s `source_parsed_caps_match_the_running_backend`
    /// mechanically parses `backend_caps()`'s source for literal `: true,`
    /// declarations and would flag a field that reads from `self` as
    /// drift), the same distinction that already keeps
    /// [`Self::kitty_keyboard`] a plain inherent accessor rather than
    /// folded into the bool-capability vocabulary. Read via
    /// [`Self::mouse_enabled`].
    mouse_enabled: bool,
    /// Single owner of keyboard focus (issue #830) — see
    /// [`crate::focus`]'s module doc. Mutated only by the shared
    /// Tab/Shift+Tab intercept in [`crate::runtime::preprocess_event`]
    /// via [`crate::runtime::PreprocessBackend::focus_manager_mut`];
    /// read elsewhere via [`Backend::focus_manager`].
    focus: crate::focus::FocusManager,
    /// Thread-safe inbox for [`crate::UiEvent::User`] payloads (issue
    /// #831) — see [`crate::runtime::UserEventQueue`]'s doc. [`Self::waker`]
    /// clones this `Arc` into the closure it hands out; [`Self::poll_events`]/
    /// [`Self::wait_events`] drain it into `UiEvent::User`, appended after
    /// whatever crossterm produced this call, on every call. Before
    /// quadraui#832, TUI's live runner polled unconditionally every 16ms
    /// (`tui::run::POLL_TIMEOUT`), which incidentally bounded a background
    /// wake's latency without any native interrupt of the blocking
    /// crossterm read. Since #832 the idle poll is coarser
    /// (`crate::runtime::IDLE_POLL_CEILING`, 250ms) — still no native
    /// interrupt exists (unlike GTK/macOS/Windows, see `Backend::waker`'s
    /// doc for why those three need one), so a background-only wake with
    /// no scheduled frame and no concurrent input now has up to that
    /// ceiling of latency instead of 16ms. A documented, deliberate
    /// tradeoff — see [`Self::request_frame_in`]'s doc.
    user_events: std::sync::Arc<crate::runtime::UserEventQueue>,
    /// Pending [`Self::request_frame_in`] deadline (issue #832) — see
    /// [`crate::runtime::FrameScheduler`]'s doc for why TUI needs this
    /// where GTK/macOS/Windows instead arm a real native timer directly
    /// from `request_frame_in`. Consulted by the live runner
    /// (`tui::run::run_inner`) each loop iteration, via
    /// [`Self::frame_poll_timeout`]/[`Self::clear_frame_deadline_if_due`],
    /// to compute the next `wait_events` timeout.
    /// [`crate::tui::testing::TuiDriver`]'s headless loop is scripted
    /// rather than timer-driven, so it never *waits* on this deadline —
    /// but it does arm it (its `dispatch`/`pump_user_events`/`tick` call
    /// `request_frame_in` exactly where the live loop does), and tests
    /// read it back through [`Self::frame_requests`] /
    /// [`Self::pending_frame_delay`].
    frame_scheduler: crate::runtime::FrameScheduler,
}

impl TuiBackend {
    /// Construct the backend with default viewport (80×24) and
    /// default quadraui theme. The caller calls [`Backend::begin_frame`]
    /// each frame (after `terminal.size()`) to keep
    /// [`Backend::viewport`] in sync, and [`Self::set_current_theme`]
    /// before drawing so the trait `draw_*` methods see the right
    /// palette.
    pub fn new() -> Self {
        Self {
            viewport: Viewport::default(),
            modal_stack: Rc::new(RefCell::new(ModalStack::new())),
            drag_state: Rc::new(RefCell::new(DragState::new())),
            accelerators: HashMap::new(),
            parsed_accelerators: Vec::new(),
            services: TuiPlatformServices::new(),
            current_frame_ptr: Cell::new(std::ptr::null_mut()),
            current_theme: crate::Theme::default(),
            nerd_fonts_enabled: false,
            double_click: super::events::DoubleClickDetector::new(),
            double_click_folding: true,
            zones: Vec::new(),
            text_selection: crate::text_selection::TextSelectionState::default(),
            cached_selection_text: String::new(),
            focused_activity_bar: None,
            last_cursor_position: None,
            tab_bar_layouts: HashMap::new(),
            color_depth: super::caps::detect_color_depth(),
            kitty_keyboard: super::caps::detect_kitty_keyboard(),
            mouse_enabled: true,
            focus: crate::focus::FocusManager::new(),
            user_events: crate::runtime::UserEventQueue::new(),
            frame_scheduler: crate::runtime::FrameScheduler::new(),
        }
    }

    /// The colour depth [`Backend::backend_caps`] currently reports —
    /// see [`crate::backend::ColorDepth`] and [`Self::set_color_depth`].
    pub fn color_depth(&self) -> ColorDepth {
        self.color_depth
    }

    /// How long the run loop may block in [`Backend::wait_events`] before
    /// it needs to re-check state (quadraui#832) — the pending
    /// [`Backend::request_frame_in`] deadline's remaining time, clamped to
    /// `ceiling`, or `ceiling` itself if nothing is pending. See
    /// [`crate::runtime::FrameScheduler::poll_timeout`].
    pub(crate) fn frame_poll_timeout(&self, ceiling: Duration) -> Duration {
        self.frame_scheduler.poll_timeout(ceiling)
    }

    /// Clear the pending [`Backend::request_frame_in`] deadline if it has
    /// passed. Call once per loop iteration right after `wait_events`
    /// returns, before dispatching events or calling `tick` — see
    /// [`crate::runtime::FrameScheduler::clear_if_due`].
    pub(crate) fn clear_frame_deadline_if_due(&mut self) {
        self.frame_scheduler.clear_if_due();
    }

    /// How many times [`Backend::request_frame_in`] has been called on
    /// this backend since it was constructed (quadraui#832).
    ///
    /// Real hosts never need this — it exists so a headless
    /// [`crate::tui::testing::TuiDriver`] test can assert on an app's
    /// *scheduling* behaviour, which is otherwise invisible: an app that
    /// re-arms itself every tick and one that relies on a fixed idle
    /// cadence paint identical screens and return identical
    /// [`crate::Reaction`]s. Counting the calls (rather than reading
    /// [`Self::pending_frame_delay`]) is what makes the chained-re-arm
    /// pattern testable, because repeated requests at the same interval
    /// coalesce to one deadline — see
    /// [`crate::runtime::FrameScheduler`].
    ///
    /// Monotonic: a fired deadline clears the pending wake, not the
    /// count.
    pub fn frame_requests(&self) -> u64 {
        self.frame_scheduler.requests()
    }

    /// Time remaining until the pending [`Backend::request_frame_in`]
    /// deadline, or `None` when no frame is scheduled (quadraui#832).
    ///
    /// The companion to [`Self::frame_requests`]: that one proves *how
    /// often* an app asked, this one proves *what interval* it asked
    /// for. Also test-facing — the live runner uses the `pub(crate)`
    /// [`Self::frame_poll_timeout`] instead, which clamps to the idle
    /// ceiling and so can't distinguish "nothing scheduled" from
    /// "scheduled beyond the ceiling".
    pub fn pending_frame_delay(&self) -> Option<Duration> {
        self.frame_scheduler.pending_delay()
    }

    /// Override the detected colour depth. Real hosts never need this —
    /// [`Self::new`] already detects it from the environment — but tests
    /// that want to force a specific SGR shape (quadraui#826's Tier-3 pty
    /// fixture, or any in-process test asserting quantised output) call
    /// this to pin the value instead of depending on the process's real
    /// `TERM`/`COLORTERM`.
    pub fn set_color_depth(&mut self, depth: ColorDepth) {
        self.color_depth = depth;
    }

    /// The kitty-keyboard-protocol state [`Backend::backend_caps`]
    /// currently reports — see
    /// [`crate::backend::BackendCaps::kitty_keyboard`] and
    /// [`Self::set_kitty_keyboard`].
    pub fn kitty_keyboard(&self) -> bool {
        self.kitty_keyboard
    }

    /// Override the kitty-keyboard-protocol flag. [`super::run::run`]
    /// calls this once at startup with
    /// [`super::caps::probe_kitty_keyboard`]'s live answer, overwriting
    /// the environment-only guess [`Self::new`] seeded it with. Also the
    /// hook a test (or a host that already knows its terminal's real
    /// capability) uses to pin the value without touching a real terminal
    /// (quadraui#827's Tier-3 pty fixture, or any in-process test
    /// asserting the flag reaches an app).
    pub fn set_kitty_keyboard(&mut self, supported: bool) {
        self.kitty_keyboard = supported;
    }

    /// Drain pending [`crate::UiEvent::User`] payloads without touching
    /// crossterm — the same [`Self::user_events`] queue [`Backend::poll_events`]/
    /// [`Backend::wait_events`] fold in, exposed standalone for
    /// [`super::testing::TuiDriver`] (issue #831). The driver runs against
    /// ratatui's `TestBackend`, never a live terminal, so it has no
    /// crossterm event source to poll for a background-thread wake to ride
    /// in on — this lets it observe the wake directly instead.
    pub(crate) fn drain_user_events(&mut self) -> Vec<UiEvent> {
        let mut out = Vec::new();
        self.user_events.drain_into(&mut out);
        out
    }

    /// Whether mouse reporting is active this session — see
    /// [`Self::set_mouse_enabled`]'s doc for why this is a plain inherent
    /// accessor rather than a [`crate::backend::BackendCaps`] field.
    pub fn mouse_enabled(&self) -> bool {
        self.mouse_enabled
    }

    /// Override whether this session has mouse reporting.
    /// [`super::run::run_with`] calls this with `false` when the caller
    /// opts into `no-mouse` mode (`RunConfig { mouse: false, .. }`,
    /// quadraui#828), so the terminal is never asked to enable mouse
    /// capture. Also the hook a test uses to simulate "capture refused"
    /// without a real terminal.
    pub fn set_mouse_enabled(&mut self, enabled: bool) {
        self.mouse_enabled = enabled;
    }

    /// Enter the frame-scope: stash the `&mut Frame<'_>` pointer for
    /// trait `draw_*` methods to access, run `f`, then clear the
    /// pointer. **Must** be called from inside a
    /// `terminal.draw(|frame| …)` closure.
    ///
    /// Type-erased through `*mut ()` because `Frame<'a>` carries a
    /// lifetime parameter we don't want to thread onto `TuiBackend`.
    /// Safety relies on three invariants enforced by this function's
    /// shape:
    ///   1. The pointer is set immediately before running `f` and
    ///      cleared immediately after, including on panic (via
    ///      [`scopeguard`]-style restore).
    ///   2. `f` cannot move the pointer out — it only sees it via
    ///      [`Self::current_frame_mut`] which returns a fresh
    ///      `&mut Frame<'_>` borrow scoped to the call.
    ///   3. `enter_frame_scope` calls don't nest meaningfully —
    ///      the inner call would overwrite the pointer with the
    ///      same `&mut` (already aliased) which Rust's borrow-checker
    ///      forbids at the caller side.
    pub fn enter_frame_scope<R>(
        &mut self,
        frame: &mut Frame<'_>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let ptr = frame as *mut Frame<'_> as *mut ();
        let prev = self.current_frame_ptr.replace(ptr);
        let result = f(self);
        self.current_frame_ptr.set(prev);
        result
    }

    /// Get the current frame inside [`Self::enter_frame_scope`], or
    /// `None` outside it. Trait `draw_*` methods call this and bail
    /// (panic in dev, silent return otherwise) if `None`.
    fn current_frame_mut(&mut self) -> Option<&mut Frame<'static>> {
        let ptr = self.current_frame_ptr.get();
        if ptr.is_null() {
            None
        } else {
            // SAFETY: `enter_frame_scope` set this from a real
            // `&mut Frame<'_>` and won't return until the scope
            // ends, at which point the pointer is cleared. Outside
            // the scope `ptr` is null and we return `None`.
            // The `'static` lifetime here is a fiction — the borrow
            // is actually scoped to the enclosing
            // `enter_frame_scope` call. Methods using this never let
            // the borrow escape past their own return.
            Some(unsafe { &mut *(ptr as *mut Frame<'static>) })
        }
    }

    /// Update the cached quadraui theme. Call once per frame from
    /// `paint`, before any `backend.draw_*` calls. Subsequent
    /// `draw_*` invocations consume the stored theme.
    pub fn set_current_theme(&mut self, theme: crate::Theme) {
        self.current_theme = theme;
    }

    /// Widget zones registered via [`crate::Backend::register_zone`] during
    /// the current frame. Read by
    /// [`crate::tui::testing::TuiDriver::inventory`] to populate
    /// [`crate::testing::FrameInventory::zones`] (quadraui#490).
    pub(crate) fn zones(&self) -> &[ZoneRec] {
        &self.zones
    }

    /// The `(bar rect, resolved layout)` cached from the most recent
    /// `draw_tab_bar` call for tab bar `id` this frame, or `None` if that
    /// bar didn't paint this frame. See [`Self::tab_bar_layouts`]'s docs
    /// for the coordinate-space contract. Read by
    /// [`crate::tui::testing::TuiDriver::tab_center`] /
    /// [`crate::tui::testing::TuiDriver::tab_close_center`] (quadraui#594).
    pub(crate) fn cached_tab_bar_layout(&self, id: &WidgetId) -> Option<(QRect, &TabBarLayout)> {
        self.tab_bar_layouts
            .get(id)
            .map(|(rect, layout)| (*rect, layout))
    }

    // ── Text selection ─────────────────────────────────────────────────────
    //
    // The region registry + active-selection state machine itself lives in
    // [`crate::text_selection::TextSelectionState`] (#741) — every method
    // below except `apply_selection_highlight`/`extract_selection_text`
    // (live-ratatui-buffer extraction, TUI-only — see the module doc on
    // `crate::text_selection`) is a thin delegation.

    /// Every `TextRegion` registered so far this frame. Test-only: unlike
    /// `GtkBackend`'s twin (read by `gtk::run`/`GtkDriver`), nothing in the
    /// live TUI runner needs this outside `self` — [`Self::apply_dispatch`]
    /// reads `self.text_selection.text_regions` directly.
    #[cfg(test)]
    pub(crate) fn text_regions(&self) -> &[TextRegion] {
        &self.text_selection.text_regions
    }

    /// Return the current active text selection, if any.
    pub(crate) fn active_text_selection(
        &self,
    ) -> Option<&crate::text_selection::ActiveTextSelection> {
        self.text_selection.active_text_selection()
    }

    /// Update (or start) the active text selection. Called by the runner
    /// when a [`UiEvent::TextSelectionChanged`] event arrives, and by
    /// [`Self::select_all_text_region`].
    pub(crate) fn set_active_text_selection(
        &mut self,
        region: WidgetId,
        anchor: Point,
        focus: Point,
    ) {
        self.text_selection
            .set_active_text_selection(region, anchor, focus);
    }

    /// Clear the active text selection and, if a `TextSelection` drag is
    /// in progress, end it. Called by the runner on `MouseDown` or after
    /// Ctrl-C copies the selection.
    pub(crate) fn clear_text_selection(&mut self) {
        let mut drag_state = self.drag_state.borrow_mut();
        self.text_selection.clear_text_selection(&mut drag_state);
    }

    /// Clear only the displayed selection highlight without touching drag
    /// state. Used by the run loop on `MouseDown` so that the drag just
    /// initiated by `dispatch_click` is not immediately cancelled.
    pub(crate) fn clear_selection_display(&mut self) {
        self.text_selection.clear_selection_display();
    }

    /// End any in-progress `TextSelection` drag without clearing the
    /// displayed `active_selection`. Called by the `Backend` trait impl of
    /// [`Backend::cancel_text_selection_drag`] so apps can abort a
    /// speculative drag (started by `apply_dispatch` before the app had a
    /// chance to forward the click to a PTY) while preserving any
    /// previously finalised selection highlight on screen.
    fn cancel_text_selection_drag_impl(&mut self) {
        let mut drag_state = self.drag_state.borrow_mut();
        self.text_selection
            .cancel_text_selection_drag(&mut drag_state);
    }

    /// Invert (highlight) the cells in the ratatui buffer that fall
    /// within the active text selection range, and cache the extracted
    /// text in `self.cached_selection_text`.
    ///
    /// Must be called inside `terminal.draw(|frame| …)` after
    /// `app.render`. The cache is necessary because ratatui swaps its
    /// double-buffer after `draw` returns, so `terminal.current_buffer_mut()`
    /// would return an empty buffer by the time Ctrl-C fires.
    pub(crate) fn apply_selection_highlight(&mut self, buf: &mut ratatui::buffer::Buffer) {
        let sel = match self.text_selection.active_text_selection() {
            Some(s) => s,
            None => return,
        };
        let region = match self.text_selection.find_region(&sel.region) {
            Some(r) => r,
            None => return,
        };
        let bounds = crate::event::Rect::new(
            region.bounds.x,
            region.bounds.y,
            region.bounds.width,
            region.bounds.height,
        );
        let ranges = crate::dispatch::text_selection_line_range(sel.anchor, sel.focus, bounds);
        let area = buf.area;

        // Cache text before inverting so we read the original cell content.
        // `text_selection_line_range` reports cell-space columns as f32
        // (issue #504); TUI cells are always whole units, so round-trip
        // through `u16` for buffer indexing.
        let mut lines: Vec<String> = Vec::with_capacity(ranges.len());
        for &(row, col_start, col_end) in &ranges {
            if row >= area.y + area.height {
                continue;
            }
            let mut line = String::new();
            for col in (col_start as u16)..(col_end as u16) {
                if col < area.x + area.width {
                    line.push_str(buf[(col, row)].symbol());
                }
            }
            let trimmed = line.trim_end_matches(|c: char| c.is_whitespace() || c == '\0');
            lines.push(trimmed.to_string());
        }
        self.cached_selection_text = lines.join("\n");

        // Now invert cells for the highlight.
        for (row, col_start, col_end) in ranges {
            for col in (col_start as u16)..(col_end as u16) {
                if col < area.x + area.width && row < area.y + area.height {
                    let cell = &mut buf[(col, row)];
                    let fg = cell.fg;
                    let bg = cell.bg;
                    cell.fg = bg;
                    cell.bg = fg;
                }
            }
        }
    }

    /// Return a clone of the selection text cached by the last
    /// `apply_selection_highlight` call. Named `cached_selection_text`
    /// rather than `take_*` because the value is cloned, not consumed —
    /// the cache persists until the selection is cleared.
    pub(crate) fn cached_selection_text(&self) -> String {
        self.cached_selection_text.clone()
    }

    /// Take the editor cursor position cached by the most recent
    /// [`Backend::draw_editor`] call this frame, leaving `None` behind.
    ///
    /// Called by [`super::run::render_frame`] once per frame, after the
    /// render closure returns but still inside `terminal.draw(|frame| …)`,
    /// so it can apply `frame.set_cursor_position(pos)` — the only place
    /// with access to the real `Frame` (quadraui#466). "Take" (not a plain
    /// getter) because the value is frame-scoped: [`Self::begin_frame`]
    /// clears it again before the next `draw_editor` call, so nothing relies
    /// on the value surviving past this one read.
    pub(crate) fn take_last_cursor_position(&mut self) -> Option<(u16, u16)> {
        self.last_cursor_position.take()
    }

    /// Set the active selection to cover the entire visible content of the
    /// most-recently focused `TextRegion`. Returns `true` when a region was
    /// found and the selection was set; `false` when no region can be
    /// resolved (zero registered regions, or multiple with no prior
    /// interaction).
    ///
    /// Target resolution order:
    /// 1. [`Self::last_text_region_id`] if the region is still registered
    ///    this frame.
    /// 2. The sole registered region (if exactly one exists).
    /// 3. Returns `false` — caller should fall through to the app.
    ///
    /// # Viewport-only limitation
    ///
    /// `TextRegion.bounds` is the painted viewport. For scrolled panels
    /// (e.g. long issue bodies) only the on-screen rows are selected.
    /// Full-document select-all requires `TextRegion` to carry
    /// total-content rows; a follow-up issue tracks this.
    ///
    /// # `last_text_region_id` persistence
    ///
    /// [`Self::last_text_region_id`] is intentionally NOT cleared on
    /// `clear_text_selection` / `clear_selection_display` so Ctrl-A
    /// still targets the right region after Ctrl-C or a plain click.
    /// A future multi-panel focus model may need to revise this.
    // TODO: full-document select-all requires TextRegion to carry
    // total-content rows; for now this selects only the visible
    // viewport (bounds).
    pub(crate) fn select_all_text_region(&mut self) -> bool {
        self.text_selection.select_all_text_region()
    }

    /// `WidgetId` of the `ActivityBar` that declared
    /// `is_keyboard_focused = true` during the most recent render pass,
    /// if any. Added for quadraui#813's `PreprocessBackend` impl to
    /// mirror `Gtk`/`Mac`/`WinBackend::focused_activity_bar_id` — TUI
    /// itself never calls this: [`Self::apply_dispatch`] already redirects
    /// a focused bar's `KeyPressed` to `UiEvent::ActivityBar` one layer
    /// upstream of `dispatch_event`, so by the time an event reaches
    /// `preprocess_event` a `KeyPressed` on TUI never has a bar focused —
    /// the shared step's `KeyPressed` guard just never matches here. See
    /// `PreprocessBackend for TuiBackend`'s doc.
    pub(crate) fn focused_activity_bar_id(&self) -> Option<&WidgetId> {
        self.focused_activity_bar.as_ref()
    }

    /// Read the selected cells back from `buf`, trim trailing whitespace
    /// per line, and return the joined text (lines separated by `\n`).
    ///
    /// Falls back to an empty `String` when there is no active selection
    /// or the region id can no longer be found in the registered regions.
    ///
    /// Only used in tests — production code uses the text cached by
    /// `apply_selection_highlight` (the live buffer is unavailable after
    /// `terminal.draw` swaps ratatui's double-buffer).
    #[cfg(test)]
    pub(crate) fn extract_selection_text(&self, buf: &ratatui::buffer::Buffer) -> String {
        let sel = match self.text_selection.active_text_selection() {
            Some(s) => s,
            None => return String::new(),
        };
        let region = match self.text_selection.find_region(&sel.region) {
            Some(r) => r,
            None => return String::new(),
        };
        let bounds = crate::event::Rect::new(
            region.bounds.x,
            region.bounds.y,
            region.bounds.width,
            region.bounds.height,
        );
        let ranges = crate::dispatch::text_selection_line_range(sel.anchor, sel.focus, bounds);
        let area = buf.area;
        let mut lines: Vec<String> = Vec::with_capacity(ranges.len());
        for (row, col_start, col_end) in ranges {
            if row >= area.y + area.height {
                continue;
            }
            let mut line = String::new();
            for col in (col_start as u16)..(col_end as u16) {
                if col < area.x + area.width {
                    line.push_str(buf[(col, row)].symbol());
                }
            }
            // Trim trailing whitespace and NUL padding per line.
            let trimmed = line.trim_end_matches(|c: char| c.is_whitespace() || c == '\0');
            lines.push(trimmed.to_string());
        }
        lines.join("\n")
    }

    // ── Accelerators ───────────────────────────────────────────────────────

    /// Walk `events` and rewrite any `UiEvent::KeyPressed` whose key +
    /// modifiers match a registered `Global`-scope accelerator into
    /// `UiEvent::Accelerator(id, modifiers)`. Stage 6's whole point: the
    /// app dispatches on stable IDs, never on raw key strings, for
    /// keybindings the user can rebind.
    ///
    /// Widget- and Mode-scoped accelerators are skipped here — the
    /// backend doesn't know which widget has focus or what mode the app
    /// is in. Apps that want those scopes match against `KeyPressed`
    /// themselves once they have that context.
    /// Run raw translated events through the dispatch layer so that
    /// `MouseDown` on a text region begins a `TextSelection` drag and
    /// `MouseMoved` (with button held) emits `TextSelectionChanged`.
    ///
    /// # TODO: scrollbar dispatch
    ///
    /// Scroll-surface arbitration is not wired here yet — scroll surfaces
    /// are not registered per-frame by `TuiBackend`. Passing an empty slice
    /// to `dispatch_click` means text regions work correctly today and
    /// scrollbar drags are unaffected (they continue to be handled by
    /// app-side hit-tests as before). The consequence is that the
    /// "scrollbar wins over an overlapping text region" acceptance
    /// criterion is not enforced in TUI. Tracked as a follow-up issue
    /// (scrollbar dispatch epic).
    fn apply_dispatch(&mut self, raw: Vec<UiEvent>) -> Vec<UiEvent> {
        let mut out = Vec::with_capacity(raw.len());
        for event in raw {
            match event {
                UiEvent::MouseDown {
                    button,
                    position,
                    modifiers,
                    ..
                } => {
                    let modal_stack = self.modal_stack.borrow();
                    let mut drag_state = self.drag_state.borrow_mut();
                    out.extend(crate::dispatch::dispatch_click(
                        &modal_stack,
                        &[],
                        &self.text_selection.text_regions,
                        &mut drag_state,
                        position,
                        button,
                        modifiers,
                    ));
                    // Track which region was clicked so Ctrl-A can target
                    // the right region even before the first drag move.
                    if let Some(DragTarget::TextSelection { region, .. }) = drag_state.target() {
                        self.text_selection
                            .track_focused_text_region(region.clone());
                    }
                }
                UiEvent::MouseMoved { position, buttons } => {
                    let drag_state = self.drag_state.borrow();
                    out.extend(crate::dispatch::dispatch_mouse_drag(
                        &drag_state,
                        position,
                        buttons,
                    ));
                }
                UiEvent::MouseUp {
                    button, position, ..
                } => {
                    let modal_stack = self.modal_stack.borrow();
                    let mut drag_state = self.drag_state.borrow_mut();
                    out.extend(crate::dispatch::dispatch_mouse_up(
                        &modal_stack,
                        &mut drag_state,
                        position,
                        button,
                    ));
                }
                // When an ActivityBar has `is_keyboard_focused`, redirect
                // raw `KeyPressed` events to it so the app receives
                // `UiEvent::ActivityBar(id, KeyPressed { … })` instead of
                // `UiEvent::KeyPressed { … }`. This runs before the
                // accelerator pass so the bar can intercept any key.
                UiEvent::KeyPressed {
                    key,
                    modifiers,
                    repeat,
                } => {
                    if let Some(id) = self.focused_activity_bar.clone() {
                        let key_str =
                            crate::primitives::activity_bar::key_to_activity_bar_string(&key);
                        out.push(UiEvent::ActivityBar(
                            id,
                            crate::ActivityBarEvent::KeyPressed {
                                key: key_str,
                                modifiers,
                            },
                        ));
                    } else {
                        out.push(UiEvent::KeyPressed {
                            key,
                            modifiers,
                            repeat,
                        });
                    }
                }
                other => out.push(other),
            }
        }
        out
    }

    /// Run injected (synthetic) raw events through the same translation
    /// pipeline [`Backend::wait_events`] applies to crossterm input —
    /// drag-state dispatch, accelerator matching, double-click folding —
    /// minus the terminal read. Used by the headless
    /// [`super::testing::TuiDriver`] so scripted mouse drags exercise the
    /// real `DragState` / text-selection machinery instead of bypassing
    /// it. Mirrors the body of [`Backend::wait_events`] exactly.
    pub(crate) fn translate_injected(&mut self, raw: Vec<UiEvent>) -> Vec<UiEvent> {
        let mut out = self.apply_dispatch(raw);
        self.apply_accelerators(&mut out);
        if self.double_click_folding {
            self.double_click.process(&mut out);
        }
        out
    }

    /// Toggle whether [`Self::translate_injected`] folds a `MouseDown`
    /// into a `DoubleClick` when it lands within the detector's time/radius
    /// window of the previous one. Driver-facing knob for
    /// [`super::testing::TuiDriver::set_double_click_folding`] (quadraui#592)
    /// — disable it so a test that dispatches two deliberate single clicks
    /// in immediate succession gets two `MouseDown`s, not a coin-flip on
    /// wall-clock timing.
    pub(crate) fn set_double_click_folding(&mut self, enabled: bool) {
        self.double_click_folding = enabled;
    }

    fn apply_accelerators(&self, events: &mut [UiEvent]) {
        if self.parsed_accelerators.is_empty() {
            return;
        }
        for ev in events.iter_mut() {
            if let UiEvent::KeyPressed { key, modifiers, .. } = ev {
                if let Some(id) = self.match_keypress(key, *modifiers) {
                    *ev = UiEvent::Accelerator(id, *modifiers);
                }
            }
        }
    }

    fn match_keypress(
        &self,
        key: &crate::Key,
        modifiers: crate::Modifiers,
    ) -> Option<AcceleratorId> {
        let key_name = key_to_binding_name(key);
        for (parsed, id) in &self.parsed_accelerators {
            if parsed.modifiers == modifiers && parsed.key == key_name {
                // Skip non-Global-scope entries — the backend doesn't
                // own focus/mode context.
                if let Some(acc) = self.accelerators.get(id) {
                    if matches!(acc.scope, AcceleratorScope::Global) {
                        return Some(id.clone());
                    }
                }
            }
        }
        None
    }
}

impl Default for TuiBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::runtime::PreprocessBackend for TuiBackend {
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
        self.cached_selection_text()
    }

    fn focused_activity_bar_id(&self) -> Option<&WidgetId> {
        self.focused_activity_bar_id()
    }

    fn focus_manager_mut(&mut self) -> &mut crate::focus::FocusManager {
        &mut self.focus
    }

    fn match_keypress(
        &self,
        key: &crate::Key,
        modifiers: crate::Modifiers,
    ) -> Option<AcceleratorId> {
        self.match_keypress(key, modifiers)
    }

    /// No-op passthrough — see this impl block's own doc and
    /// `PreprocessBackend::fold_double_click`'s doc for why. TUI folds
    /// double-clicks earlier, once per batch of translated crossterm
    /// events in [`Self::translate_injected`]/`Backend::wait_events`
    /// (via [`Self::double_click`]'s own `DoubleClickDetector`), before
    /// any individual event reaches `dispatch_event`/`preprocess_event`.
    /// Folding again here — even against a *fresh* detector instance —
    /// would silently coin-flip on whichever fold ran first; reusing
    /// `self.double_click` instead would double-consume the same
    /// detector state the batch pass already advanced. Either way is
    /// wrong, so this stays a deliberate identity function.
    fn fold_double_click(&mut self, ev: UiEvent) -> UiEvent {
        ev
    }

    /// TUI-only override (quadraui#496's original audit; preserved by
    /// #813): tolerates a stray Shift and accepts `'C'` (CapsLock) as
    /// well as `'c'`. Real terminals attach modifier noise to Ctrl-C
    /// that the crate-wide strict default would silently drop, turning
    /// a real copy request into a no-op.
    fn is_copy_keypress(&self, key: &crate::Key, modifiers: &crate::Modifiers) -> bool {
        matches!(key, crate::Key::Char('c') | crate::Key::Char('C'))
            && modifiers.ctrl
            && !modifiers.alt
            && !modifiers.cmd
    }
}

/// Convert a [`crate::Rect`] (f32 coordinates) to a
/// [`ratatui::layout::Rect`] (u16). Any negative values clamp to 0;
/// fractional widths/heights round to nearest. Used by every trait
/// `draw_*` method to translate the trait's `Rect` argument.
fn q_rect_to_ratatui(r: QRect) -> Rect {
    let x = r.x.max(0.0).round() as u16;
    let y = r.y.max(0.0).round() as u16;
    let w = r.width.max(0.0).round() as u16;
    let h = r.height.max(0.0).round() as u16;
    Rect::new(x, y, w, h)
}

/// Reassemble one leaked SGR mouse escape sequence (#293) out of a run of
/// individual `KeyPressed(Char(_))` events, if `events[0]` opens one.
///
/// `events[0]` must already be `KeyPressed(Char('['))` — the caller checks
/// that cheaply before calling in. This confirms the very next event is
/// `Char('<')` (the SGR-extended marker; the legacy X10/`rxvt` mouse
/// encodings aren't affected by the race this recovers from, so they're
/// out of scope), then scans forward for a run of `Char(_)` events made
/// only of ASCII digits and `;`, terminated by `Char('M')` or `Char('m')`
/// — exactly the tail of `ESC [ < Cb ; Cx ; Cy (M|m)` with its leading
/// `ESC` already consumed. Any event that isn't a plain, non-repeat
/// `KeyPressed(Char(_))`, any character outside that alphabet, or running
/// past the bound before finding a terminator aborts the match (`None`) —
/// the run is left untouched as ordinary keystrokes.
///
/// See [`recover_leaked_sgr_mouse_fragments`] for why this exists at all.
fn try_reassemble_sgr_mouse(events: &[UiEvent]) -> Option<(usize, UiEvent)> {
    let UiEvent::KeyPressed {
        key: crate::Key::Char('<'),
        repeat: false,
        ..
    } = events.get(1)?
    else {
        return None;
    };

    let mut body = String::new();
    let mut consumed = 2; // the '[' the caller matched, plus the '<' above.
    let mut terminator = None;
    // A real `Cb;Cx;Cy` triple never needs more than a handful of digits —
    // cap the scan well above that so a run of unrelated real keystrokes
    // (someone actually typing "<1234...") can't be walked indefinitely.
    for ev in events.iter().skip(2).take(24) {
        let UiEvent::KeyPressed {
            key: crate::Key::Char(c),
            repeat: false,
            ..
        } = ev
        else {
            return None;
        };
        consumed += 1;
        if *c == 'M' || *c == 'm' {
            terminator = Some(*c);
            break;
        }
        if c.is_ascii_digit() || *c == ';' {
            body.push(*c);
        } else {
            return None;
        }
    }
    let terminator = terminator?;

    let mut parts = body.split(';');
    let cb: u16 = parts.next()?.parse().ok()?;
    let x: u16 = parts.next()?.parse().ok()?;
    let y: u16 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }

    let event = decode_sgr_mouse_report(cb, x, y, terminator == 'm')?;
    Some((consumed, event))
}

/// Decode one already-parsed `Cb ; Cx ; Cy` SGR mouse triple into a
/// [`UiEvent`], via the same crossterm plumbing a well-formed escape
/// sequence would have used ([`super::events::crossterm_mouse_to_uievent`]).
///
/// The `Cb` bit layout replicated here (button number in bits 0–1 and
/// 6–7, drag flag in bit 5, modifiers in bits 2–4) is xterm's public SGR
/// mouse-tracking protocol, not a crossterm implementation detail — see
/// <http://www.xfree86.org/current/ctlseqs.html#Mouse%20Tracking> — so
/// this mirrors crossterm's own (private) `parse_cb` deliberately rather
/// than reusing it.
fn decode_sgr_mouse_report(cb: u16, x: u16, y: u16, is_release: bool) -> Option<UiEvent> {
    use ratatui::crossterm::event::{
        KeyModifiers, MouseButton as CtMouseButton, MouseEvent as CtMouseEvent, MouseEventKind,
    };

    let button_number = ((cb & 0b11) | ((cb & 0b1100_0000) >> 4)) as u8;
    let dragging = cb & 0b0010_0000 != 0;
    let mut modifiers = KeyModifiers::empty();
    if cb & 0b0000_0100 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if cb & 0b0000_1000 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if cb & 0b0001_0000 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }

    let kind = match (button_number, dragging) {
        (0, false) => MouseEventKind::Down(CtMouseButton::Left),
        (1, false) => MouseEventKind::Down(CtMouseButton::Middle),
        (2, false) => MouseEventKind::Down(CtMouseButton::Right),
        (0, true) => MouseEventKind::Drag(CtMouseButton::Left),
        (1, true) => MouseEventKind::Drag(CtMouseButton::Middle),
        (2, true) => MouseEventKind::Drag(CtMouseButton::Right),
        (3, false) => MouseEventKind::Up(CtMouseButton::Left),
        (3, true) | (4, true) | (5, true) => MouseEventKind::Moved,
        (4, false) => MouseEventKind::ScrollUp,
        (5, false) => MouseEventKind::ScrollDown,
        (6, false) => MouseEventKind::ScrollLeft,
        (7, false) => MouseEventKind::ScrollRight,
        _ => return None,
    };
    // SGR mode ends the sequence with lowercase `m` for a release, since
    // `Cb`'s button-3 slot can't otherwise distinguish which button went
    // up (mirrors crossterm's own `parse_csi_sgr_mouse`).
    let kind = if is_release {
        match kind {
            MouseEventKind::Down(b) => MouseEventKind::Up(b),
            other => other,
        }
    } else {
        kind
    };

    super::events::crossterm_mouse_to_uievent(CtMouseEvent {
        kind,
        column: x.saturating_sub(1),
        row: y.saturating_sub(1),
        modifiers,
    })
}

/// Recover an SGR mouse escape sequence that leaked into individual
/// `KeyPressed(Char(_))` events (quadraui#293).
///
/// crossterm's own terminal-input reader disambiguates a lone `ESC` byte
/// by whether more bytes are *already* available in the same read — not
/// by waiting for them. When the OS delivers `ESC [ < Cb ; Cx ; Cy (M|m)`
/// (a real mouse-motion/click report) split across two reads right after
/// that leading `ESC` — which happens under perfectly ordinary scheduling
/// jitter, not just a contrived race — crossterm commits to "standalone
/// Escape key" for the lone byte and then decodes the *rest* of the
/// report byte-by-byte as ordinary printable characters, since `[` isn't
/// a recognised lead byte on its own. Observed live: the literal text
/// `[<35;10;5M` typed into whatever has focus, e.g. coord-tui's chat
/// input (see #293's repro). The `Escape` itself still fires correctly —
/// it's only the report's tail that leaks — but landing in a focused
/// `TextInput` right before it makes `Escape` look like it "didn't
/// register" (it cleared/typed-into the just-polluted field instead of
/// doing whatever an already-empty field would have triggered).
///
/// This can't be fixed by *not asking for* motion reports (mode 1003):
/// hover state ([`super::toolbar_hover_tracker`] and friends) is a real,
/// shipped TUI feature that depends on no-button `MouseMoved` events, so
/// disabling any-motion tracking would trade this bug for silently
/// breaking every hover affordance. Instead, this runs over each frame's
/// already-drained batch of native events (still the same-frame window
/// crossterm's own race lands in) and reconstitutes any leaked report
/// back into the mouse event it should have been — before
/// [`coalesce_mouse_moved`] and [`TuiBackend::apply_dispatch`] ever see
/// it, so a recovered event flows through the exact same pipeline a
/// cleanly decoded one would.
fn recover_leaked_sgr_mouse_fragments(events: Vec<UiEvent>) -> Vec<UiEvent> {
    let mut out = Vec::with_capacity(events.len());
    let mut i = 0;
    while i < events.len() {
        let opens_escape = matches!(
            events[i],
            UiEvent::KeyPressed {
                key: crate::Key::Char('['),
                repeat: false,
                ..
            }
        );
        if opens_escape {
            if let Some((consumed, mouse_event)) = try_reassemble_sgr_mouse(&events[i..]) {
                out.push(mouse_event);
                i += consumed;
                continue;
            }
        }
        out.push(events[i].clone());
        i += 1;
    }
    out
}

/// Coalesce consecutive `MouseMoved` events in a raw batch, keeping only
/// the last in each consecutive run.  A non-`MouseMoved` event breaks a
/// run; the flushed move and the other event are both preserved in order.
///
/// Applied before [`TuiBackend::apply_dispatch`] so that a burst of N
/// `MouseMoved` events (e.g. a fast drag) results in a single
/// `dispatch_mouse_drag` call at the final cursor position, preventing N
/// redundant selection updates or SGR-cursor writes to embedded PTYs.
///
/// # Ordering guarantee
///
/// `MouseDown … MouseMoved* … MouseUp` bursts survive intact — only
/// intermediate positions within a consecutive run are elided.
fn coalesce_mouse_moved(raw: Vec<UiEvent>) -> Vec<UiEvent> {
    let mut out = Vec::with_capacity(raw.len());
    let mut pending_move: Option<UiEvent> = None;
    for ev in raw {
        match ev {
            UiEvent::MouseMoved { .. } => {
                // Replace pending move with the newer position.
                pending_move = Some(ev);
            }
            other => {
                // Flush the pending move before the non-move event so
                // ordering is preserved (e.g. MouseDown before MouseMoved).
                if let Some(m) = pending_move.take() {
                    out.push(m);
                }
                out.push(other);
            }
        }
    }
    // Flush any trailing move (common case: burst ends with a move).
    if let Some(m) = pending_move.take() {
        out.push(m);
    }
    out
}

impl crate::backend::sealed::Sealed for TuiBackend {}

impl Backend for TuiBackend {
    fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn begin_frame(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        // Clear per-frame text regions so stale registrations from the
        // previous frame don't linger.
        self.text_selection.begin_frame();
        // Clear per-frame widget zones for the same reason. Same
        // lifecycle as text_regions.
        self.zones.clear();
        // Clear the focused activity bar — re-set by draw_activity_bar
        // during the render pass if still focused.
        self.focused_activity_bar = None;
        // Clear the cached editor cursor position — re-set by draw_editor
        // during the render pass if an editor paints this frame. Without
        // this a frame that stops painting an editor (e.g. it's hidden)
        // would keep showing the terminal cursor at the last-known spot.
        self.last_cursor_position = None;
        // Clear per-frame tab-bar layout cache for the same reason as
        // `zones` — a bar that stops painting must stop resolving
        // `tab_center`/`tab_close_center` too.
        self.tab_bar_layouts.clear();
        // #455: clear last frame's modal paint marks so this frame has
        // to earn them again (via draw_dialog/draw_palette/draw_context_menu).
        self.modal_stack.borrow_mut().reset_frame_paint();
    }

    fn register_text_region(&mut self, region: TextRegion) {
        self.text_selection.register_text_region(region);
    }

    fn register_zone(&mut self, id: WidgetId, bounds: QRect) {
        self.zones.push(ZoneRec { id, bounds });
    }

    fn cancel_text_selection_drag(&mut self) {
        self.cancel_text_selection_drag_impl();
    }

    fn end_frame(&mut self) {
        // No-op. The frame's actual flush happens when ratatui's
        // `terminal.draw(|frame| …)` closure returns; this method
        // exists for parity with backends that need explicit flush.
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

    fn set_theme(&mut self, theme: crate::Theme) {
        self.set_current_theme(theme);
    }

    fn set_nerd_fonts(&mut self, enabled: bool) {
        self.nerd_fonts_enabled = enabled;
    }

    fn poll_events(&mut self) -> Vec<UiEvent> {
        // Drain every queued crossterm event; never blocks. Each
        // native event translates to zero, one, or more `UiEvent`s
        // via [`super::events::crossterm_to_uievents`], then runs
        // through the dispatch layer (text-region hit-test, drag state)
        // and [`Self::apply_accelerators`].  Consecutive `MouseMoved`
        // events are coalesced to the final position before dispatch.
        let mut raw = Vec::new();
        while ratatui::crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
            match ratatui::crossterm::event::read() {
                Ok(ev) => raw.extend(super::events::crossterm_to_uievents(ev)),
                Err(_) => break,
            }
        }
        // See `recover_leaked_sgr_mouse_fragments`'s doc (#293): recover
        // any SGR mouse report that crossterm's own reader split around
        // its leading `ESC`, before it reaches coalescing/dispatch as
        // stray printable characters.
        let raw = recover_leaked_sgr_mouse_fragments(raw);
        let coalesced = coalesce_mouse_moved(raw);
        let mut out = self.apply_dispatch(coalesced);
        self.apply_accelerators(&mut out);
        self.double_click.process(&mut out);
        // Issue #831: fold in any `UiEvent::User` payloads a background
        // thread queued via `waker()` since the last drain. Unconditional
        // (not gated on `raw` being non-empty) so a background-only wake
        // with no concurrent keyboard/mouse activity still surfaces here.
        self.user_events.drain_into(&mut out);
        out
    }

    fn wait_events(&mut self, timeout: Duration) -> Vec<UiEvent> {
        // Block up to `timeout` for the first native event, then drain
        // the remainder of the queue non-blocking so a burst of events
        // (e.g. a fast mouse drag) is processed in a single frame instead
        // of one event per render cycle.  Consecutive `MouseMoved` events
        // are coalesced to the final position before dispatch, preventing
        // N redundant selection updates or SGR-cursor writes to embedded
        // PTYs.  Returns an empty `Vec` on timeout.
        if let Ok(true) = ratatui::crossterm::event::poll(timeout) {
            let mut raw = Vec::new();
            match ratatui::crossterm::event::read() {
                Ok(ev) => raw.extend(super::events::crossterm_to_uievents(ev)),
                Err(_) => {
                    // Issue #831: even a crossterm read error shouldn't
                    // drop a background wake that arrived in the same
                    // window — still surface anything already queued.
                    let mut out = Vec::new();
                    self.user_events.drain_into(&mut out);
                    return out;
                }
            }
            // Drain the rest of the queue without blocking.
            while ratatui::crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
                match ratatui::crossterm::event::read() {
                    Ok(ev) => raw.extend(super::events::crossterm_to_uievents(ev)),
                    Err(_) => break,
                }
            }
            // See `recover_leaked_sgr_mouse_fragments`'s doc (#293).
            let raw = recover_leaked_sgr_mouse_fragments(raw);
            let coalesced = coalesce_mouse_moved(raw);
            let mut out = self.apply_dispatch(coalesced);
            self.apply_accelerators(&mut out);
            self.double_click.process(&mut out);
            self.user_events.drain_into(&mut out);
            return out;
        }
        // Issue #831: crossterm timed out with no native input, but a
        // background thread may have called `waker()` during the wait —
        // this is the primary path a pure background-thread wake takes,
        // since it has no crossterm event of its own to ride in on.
        let mut out = Vec::new();
        self.user_events.drain_into(&mut out);
        out
    }

    /// See [`crate::Backend::waker`]'s doc for the full cross-backend
    /// contract. TUI's implementation is the simplest of the four: the
    /// live runner's `wait_events` call already blocks for at most
    /// [`super::run::POLL_TIMEOUT`] (16ms) before looping back around, so
    /// feeding the payload into [`Self::user_events`] — drained
    /// unconditionally by both [`Self::poll_events`] and
    /// [`Self::wait_events`] above — is sufficient on its own; there is no
    /// blocking native read to interrupt the way GTK/macOS/Windows have.
    fn waker(&self) -> std::sync::Arc<dyn Fn(crate::UserPayload) + Send + Sync> {
        let queue = std::sync::Arc::clone(&self.user_events);
        std::sync::Arc::new(move |payload: crate::UserPayload| {
            queue.push(payload.into_arc());
        })
    }

    fn request_frame_in(&self, delay: Duration) {
        // See `crate::runtime::FrameScheduler`'s doc: TUI has no native
        // run loop to arm a timer against, so this just records the
        // deadline; the live runner (`tui::run::run_inner`) folds it into
        // its next `wait_events` timeout via `Self::frame_poll_timeout`/
        // `Self::clear_frame_deadline_if_due`. `TuiDriver`'s headless
        // loop is scripted rather than timer-driven, so it doesn't
        // consult this at all — a test that wants to observe a
        // `RedrawAfter` chain calls `AppLogic::tick` directly instead.
        self.frame_scheduler.request(delay);
    }

    fn register_accelerator(&mut self, acc: &Accelerator) {
        // Re-registration replaces the prior entry — both in the map and
        // the parsed list, otherwise stale bindings would shadow the new
        // one in `match_accelerator`.
        self.accelerators.insert(acc.id.clone(), acc.clone());
        self.parsed_accelerators.retain(|(_, id)| id != &acc.id);
        if let Some(parsed) = parse_binding(&acc.binding) {
            self.parsed_accelerators.push((parsed, acc.id.clone()));
        }
    }

    fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.accelerators.remove(id);
        self.parsed_accelerators.retain(|(_, eid)| eid != id);
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

    fn draw_focus_ring(&mut self, rect: QRect) {
        let theme = self.current_theme;
        let area = q_rect_to_ratatui(rect);
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_focus_ring called outside enter_frame_scope");
        crate::tui::draw_focus_ring(frame.buffer_mut(), area, &theme);
        // #492 C0 contract §5b: a chrome-only paint (no text of its own)
        // is only "observable" via a registered zone — mirrors
        // `draw_terminal_divider`'s identical registration above.
        self.register_zone(WidgetId::new("chrome:focus-ring"), rect);
    }

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    /// quadraui#492: honest per-method, not aspirational.
    ///
    /// - `mouse` / `scroll` / `drag`: `tui::run` sends crossterm's
    ///   `EnableMouseCapture` on entry, and
    ///   `tui::events::crossterm_mouse_to_uievent` maps
    ///   `MouseEventKind::{Down,Up,Drag,ScrollUp,ScrollDown,…}` to the
    ///   matching `UiEvent`, so all three input kinds are real *when this
    ///   backend type's usual runner is used*. This is a static
    ///   per-backend-type fact, not this session's actual configuration —
    ///   see [`Self::mouse_enabled`] for the runtime answer:
    ///   `tui::run::run_with(app, RunConfig { mouse: false, .. })`
    ///   (`no-mouse` mode, quadraui#828, for hosts where capture is
    ///   refused or unavailable) never negotiates capture with the
    ///   terminal at all, so no mouse-derived `UiEvent` ever arrives that
    ///   session even though this field still reads `true` — the same
    ///   split [`Self::kitty_keyboard`] already draws between "can this
    ///   backend type" and "is it active right now".
    /// - `text_selection`: `register_text_region` /
    ///   `cancel_text_selection_drag` are both overridden below —
    ///   mouse-drag selection highlight is real. Also reachable with no
    ///   mouse at all, via Ctrl-A (select-all) plus Ctrl-C (see
    ///   `crate::runtime::preprocess_event`'s Ctrl-A interception) — the
    ///   `no-mouse` mode key path for Tier-1's drag-select gesture.
    /// - everything else: **not** declared. No window to
    ///   drag/resize/maximize, no native pointer glyph, no native menu,
    ///   no IME positioning, and every `PlatformServices` dialog method
    ///   unconditionally returns `None`
    ///   (`TuiPlatformServices::show_file_open_dialog` /
    ///   `show_file_save_dialog` / `show_message_dialog`) with
    ///   notifications a no-op. The in-canvas `Dialog` primitive
    ///   (`draw_dialog`) stays the only dialog path on this backend
    ///   (quadraui#666).
    fn backend_caps(&self) -> crate::backend::BackendCaps {
        crate::backend::BackendCaps {
            mouse: true,
            scroll: true,
            drag: true,
            text_selection: true,
            color_depth: self.color_depth,
            kitty_keyboard: self.kitty_keyboard,
            ..crate::backend::BackendCaps::empty()
        }
    }

    fn line_height(&self) -> f32 {
        1.0
    }

    fn char_width(&self) -> f32 {
        1.0
    }

    /// TUI's terminal scrollbar gutter is one character cell, not GTK's
    /// 8px default — matching `src/tui/terminal.rs`'s
    /// `sb_cols: … .unwrap_or(1)` (issue #506 review fix).
    fn terminal_scrollbar_default_width(&self) -> f32 {
        1.0
    }

    fn snap_height(&self, h: f32) -> f32 {
        // Mirrors the height component of `q_rect_to_ratatui` exactly —
        // same `.max(0.0).round()` — so a height computed via this method
        // always matches what a rect with that height paints as. A unit
        // test below pins the two against each other so they can't drift
        // (quadraui#632).
        h.max(0.0).round()
    }

    // ─── Drawing ───────────────────────────────────────────────────────────
    //
    // Implementations call into the public `crate::tui::draw_*` free
    // functions; this trait impl is the thin wrapper. The frame is
    // stashed by `enter_frame_scope`; the theme by `set_current_theme`.
    // Calling these outside `enter_frame_scope` is a programmer error
    // and panics in dev (the `expect` makes the boundary loud).

    fn draw_tree(&mut self, rect: QRect, tree: &TreeView) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_tree called outside enter_frame_scope");
        crate::tui::draw_tree(frame.buffer_mut(), area, tree, &theme, nerd_fonts);
    }

    fn draw_list(&mut self, rect: QRect, list: &ListView) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_list called outside enter_frame_scope");
        crate::tui::draw_list(frame.buffer_mut(), area, list, &theme, nerd_fonts);
    }

    fn draw_data_table(
        &mut self,
        rect: QRect,
        table: &crate::DataTable,
        hovered_idx: Option<usize>,
    ) -> crate::DataTableLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_data_table called outside enter_frame_scope");
        crate::tui::draw_data_table(frame.buffer_mut(), area, table, &theme, hovered_idx)
    }

    fn data_table_layout(&self, rect: QRect, table: &crate::DataTable) -> crate::DataTableLayout {
        // Must go through the *same* resolver `draw_data_table` uses —
        // in particular the cell-granular `h_scroll.round()`. This is the
        // cache-free, layout-on-demand path real apps hit-test through
        // (see `examples/common/data_table_app.rs`), so resolving the
        // geometry independently here reopened #550 for every fractional
        // `h_scroll` (round 3).
        let area = q_rect_to_ratatui(rect);
        crate::tui::data_table_layout(area, table)
    }

    fn list_hscrollbar(&self, rect: QRect, list: &ListView) -> Option<crate::Scrollbar> {
        // Snap through the same u16 cell truncation `draw_list` uses so
        // the consumer's thumb hit-region matches the painted thumb.
        let area = q_rect_to_ratatui(rect);
        list.hscrollbar(
            crate::event::Rect::new(
                area.x as f32,
                area.y as f32,
                area.width as f32,
                area.height as f32,
            ),
            1.0,
        )
    }

    fn list_vscrollbar(&self, rect: QRect, list: &ListView) -> Option<crate::Scrollbar> {
        // Snap through the same u16 cell truncation `draw_list` uses so
        // the consumer's thumb hit-region matches the painted thumb.
        let area = q_rect_to_ratatui(rect);
        list.vscrollbar(
            crate::event::Rect::new(
                area.x as f32,
                area.y as f32,
                area.width as f32,
                area.height as f32,
            ),
            1.0,
        )
    }

    fn list_layout(&self, rect: QRect, list: &ListView) -> crate::ListViewLayout {
        crate::tui::tui_list_layout(q_rect_to_ratatui(rect), list)
    }

    fn draw_form(&mut self, rect: QRect, form: &Form) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_form called outside enter_frame_scope");
        crate::tui::draw_form(frame.buffer_mut(), area, form, &theme);
    }

    fn draw_palette(&mut self, rect: QRect, palette: &Palette) {
        // #455: mark before borrowing the frame — a modal-stack entry
        // whose id matches this palette is now known-painted this frame.
        self.modal_stack.borrow_mut().mark_painted(&palette.id);
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_palette called outside enter_frame_scope");
        crate::tui::draw_palette(frame.buffer_mut(), area, palette, &theme, nerd_fonts);
    }

    fn palette_layout(&self, rect: QRect, palette: &Palette) -> crate::PaletteLayout {
        crate::tui::tui_palette_layout(q_rect_to_ratatui(rect), palette)
    }

    fn draw_settings_chrome(
        &mut self,
        rect: QRect,
        header_text: &str,
        query: &str,
        placeholder: &str,
        active: bool,
    ) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_settings_chrome called outside enter_frame_scope");
        crate::tui::draw_settings_chrome(
            frame.buffer_mut(),
            area,
            header_text,
            query,
            placeholder,
            active,
            &theme,
        );
    }

    // ─── Layout-passthrough primitives — Stage 3 / trait migration ──────
    //
    // These take a pre-computed `*Layout` in their existing TUI
    // shims. Migrating them through the trait needs either the
    // trait to take `&Layout` (per `docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §6.2)
    // or a per-method recompute. Deferred until Stage 3.

    // Phase B.5b Stage 9: trait extended with `&Layout` parameters
    // per `docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §6.2. The TUI free functions
    // for these primitives take `&Layout` directly — the trait impls
    // are now thin pass-throughs, mirroring the GTK impls in
    // `gtk/backend.rs`.

    fn draw_status_bar_interactive(
        &mut self,
        rect: QRect,
        bar: &StatusBar,
        interaction: &crate::interaction::InteractionState,
    ) -> crate::StatusBarLayout {
        let (hovered_id, pressed_id) = (interaction.hovered(), interaction.pressed());
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let layout = bar.layout(area.width as f32, 1.0, MIN_GAP_CELLS, |seg| {
            crate::StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_status_bar called outside enter_frame_scope");
        crate::tui::draw_status_bar(
            frame.buffer_mut(),
            area,
            bar,
            &layout,
            &theme,
            hovered_id,
            pressed_id,
        )
    }

    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn draw_tab_bar(
        &mut self,
        rect: QRect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> crate::TabBarHits {
        // Icon-less bars are the empty-sidecar case of the icon path, so
        // there is exactly one measurer + one paint loop to keep in sync.
        self.draw_tab_bar_icons(rect, bar, &[], hovered_close_tab)
    }

    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn draw_tab_bar_icons(
        &mut self,
        rect: QRect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
        _hovered_close_tab: Option<usize>,
    ) -> crate::TabBarHits {
        // TUI doesn't render close-button hover bg; the parameter is
        // accepted for trait parity with GTK.
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        // Cell-unit measurer mirrors `render_impl::render_tab_bar`.
        // Respect per-tab `is_closable`: non-closable tabs get no close-column
        // reservation even when `show_tab_close` is set on the bar.
        let close_cols = if bar.show_tab_close {
            crate::tui::TAB_CLOSE_COLS as usize
        } else {
            0
        };
        // Measure in display columns, not `char`s (#554) — a CJK/emoji
        // glyph occupies two columns, and `draw_tab_bar_icons` paints
        // with `char_cell_width` strides, so the budget must agree. Also
        // add `tab_icon_cols` (#620) so a tab's optional icon glyph + gap
        // is reserved the same way the GTK rasteriser reserves its own
        // Pango-measured icon width.
        let tab_widths: Vec<usize> = bar
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let tab_close = if bar.show_tab_close && t.is_closable {
                    close_cols
                } else {
                    0
                };
                display_width(&t.label) + tab_close + crate::tab_icon_cols(icons, i) as usize
            })
            .collect();
        let layout = bar.layout(
            area.width as f32,
            area.height as f32,
            0.0, // no scroll arrows in TUI
            |i| {
                let tab_close_cols = if bar.show_tab_close && bar.tabs[i].is_closable {
                    close_cols
                } else {
                    0
                };
                crate::TabMeasure::new(tab_widths[i] as f32, tab_close_cols as f32)
            },
            |i| crate::SegmentMeasure::new(bar.right_segments[i].width_cells as f32),
        );
        // Cache the resolved layout for this bar's WidgetId before it's
        // consumed below — `TuiDriver::tab_center`/`tab_close_center`
        // (quadraui#594) resolve against this, since a driver sitting
        // outside the app has no other way to reach a specific tab's
        // geometry (every tab paints the same close glyph, so `find`
        // can't disambiguate tab N's target).
        self.tab_bar_layouts
            .insert(bar.id.clone(), (rect, layout.clone()));
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_tab_bar called outside enter_frame_scope");
        crate::tui::draw_tab_bar_icons(frame.buffer_mut(), area, bar, icons, &layout, &theme)
    }

    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn draw_tab_bar_with_chrome(
        &mut self,
        rect: QRect,
        bar: &TabBar,
        _hovered_close_tab: Option<usize>,
        chrome: &TabChrome,
    ) -> crate::TabBarHits {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let close_cols = if bar.show_tab_close {
            crate::tui::TAB_CLOSE_COLS as usize
        } else {
            0
        };
        let brackets = matches!(chrome.active_frame, TabFrame::Brackets);
        // #631: a bracket-framed active tab reserves one extra column on
        // each side of its ordinary close-column reservation — see
        // `tui::tab_bar`'s module doc for the cell-by-cell breakdown.
        let tab_widths: Vec<usize> = bar
            .tabs
            .iter()
            .map(|t| {
                let has_close = bar.show_tab_close && t.is_closable;
                let is_bracket = brackets && t.is_active;
                let base = display_width(&t.label);
                if is_bracket && has_close {
                    // '[' + glyph(1) + ']' , replacing the plain
                    // glyph+separator reservation.
                    base + 1 + 1 + 1
                } else if is_bracket {
                    // '[' + ']' around a label with no close button.
                    base + 2
                } else if has_close {
                    base + close_cols
                } else {
                    base
                }
            })
            .collect();
        let layout = bar.layout(
            area.width as f32,
            area.height as f32,
            0.0,
            |i| {
                let has_close = bar.show_tab_close && bar.tabs[i].is_closable;
                let is_bracket = brackets && bar.tabs[i].is_active;
                if is_bracket && has_close {
                    crate::TabMeasure::new(tab_widths[i] as f32, 1.0).with_trailing(1.0)
                } else if has_close {
                    crate::TabMeasure::new(tab_widths[i] as f32, close_cols as f32)
                } else {
                    crate::TabMeasure::new(tab_widths[i] as f32, 0.0)
                }
            },
            |i| crate::SegmentMeasure::new(bar.right_segments[i].width_cells as f32),
        );
        self.tab_bar_layouts
            .insert(bar.id.clone(), (rect, layout.clone()));
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_tab_bar_with_chrome called outside enter_frame_scope");
        crate::tui::draw_tab_bar_with_chrome(frame.buffer_mut(), area, bar, chrome, &layout, &theme)
    }

    fn draw_activity_bar(
        &mut self,
        rect: QRect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
    ) -> Vec<crate::ActivityBarRowHit> {
        // Track keyboard focus: if this bar declares `is_keyboard_focused`,
        // record its id so `apply_dispatch` can convert incoming `KeyPressed`
        // events to `UiEvent::ActivityBar(id, KeyPressed { … })`.
        if bar.is_keyboard_focused {
            self.focused_activity_bar = Some(bar.id.clone());
        }
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_activity_bar called outside enter_frame_scope");
        crate::tui::draw_activity_bar(
            frame.buffer_mut(),
            area,
            bar,
            &theme,
            hovered_idx,
            nerd_fonts,
        )
    }

    fn draw_activity_bar_with_style(
        &mut self,
        rect: QRect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
        style: &crate::ActivityBarStyle,
    ) -> Vec<crate::ActivityBarRowHit> {
        // Track keyboard focus: if this bar declares `is_keyboard_focused`,
        // record its id so `apply_dispatch` can convert incoming `KeyPressed`
        // events to `UiEvent::ActivityBar(id, KeyPressed { … })`.
        if bar.is_keyboard_focused {
            self.focused_activity_bar = Some(bar.id.clone());
        }
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_activity_bar_with_style called outside enter_frame_scope");
        crate::tui::draw_activity_bar_with_style(
            frame.buffer_mut(),
            area,
            bar,
            style,
            &theme,
            hovered_idx,
            nerd_fonts,
        )
    }

    fn status_bar_layout(&self, rect: QRect, bar: &StatusBar) -> crate::StatusBarLayout {
        bar.layout(rect.width, 1.0, MIN_GAP_CELLS, |seg| {
            crate::StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        })
    }

    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout(&self, rect: QRect, bar: &TabBar) -> crate::TabBarHits {
        self.tab_bar_layout_icons(rect, bar, &[])
    }

    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout_icons(
        &self,
        rect: QRect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
    ) -> crate::TabBarHits {
        let close_cols = if bar.show_tab_close {
            crate::tui::TAB_CLOSE_COLS as usize
        } else {
            0
        };
        // Compute per-tab widths respecting `is_closable`: non-closable tabs
        // get no close-column reservation even when `show_tab_close` is set.
        // Measured in display columns, not `char`s (#554) — see the twin
        // computation in `draw_tab_bar_icons` above for why. The
        // `tab_icon_cols` term is that twin's too (#620): the no-paint
        // path must reserve the icon exactly as the paint does, or click
        // routing lands left of the glyphs the user sees.
        let tab_widths: Vec<usize> = bar
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let tab_close = if bar.show_tab_close && t.is_closable {
                    close_cols
                } else {
                    0
                };
                display_width(&t.label) + tab_close + crate::tab_icon_cols(icons, i) as usize
            })
            .collect();
        let layout = bar.layout(
            rect.width,
            rect.height,
            0.0,
            |i| {
                let tab_close_cols = if bar.show_tab_close && bar.tabs[i].is_closable {
                    close_cols
                } else {
                    0
                };
                crate::TabMeasure::new(tab_widths[i] as f32, tab_close_cols as f32)
            },
            |i| crate::SegmentMeasure::new(bar.right_segments[i].width_cells as f32),
        );

        let mut hits = tab_bar_hits_from_layout(&layout, bar);
        // `TabBarHits` are target-surface (absolute) coordinates per the
        // trait doc — the same space `draw_tab_bar` returns. Without this
        // the no-paint path returned bar-relative x, off by `rect.x`
        // (nonzero for any tab bar right of a sidebar). Issue #552.
        crate::backend::shift_tab_bar_hits(&mut hits, rect.x as f64);

        let active_idx = bar.tabs.iter().position(|t| t.is_active);
        let reserved: usize = bar
            .right_segments
            .iter()
            .map(|s| s.width_cells as usize)
            .sum();
        let effective_tab_area = (rect.width as usize).saturating_sub(reserved);

        hits.correct_scroll_offset = if let Some(active) = active_idx {
            TabBar::fit_active_scroll_offset(active, bar.tabs.len(), effective_tab_area, |i| {
                tab_widths[i]
            })
        } else {
            bar.scroll_offset
        };

        hits
    }

    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout_with_chrome(
        &self,
        rect: QRect,
        bar: &TabBar,
        chrome: &TabChrome,
    ) -> crate::TabBarHits {
        let close_cols = if bar.show_tab_close {
            crate::tui::TAB_CLOSE_COLS as usize
        } else {
            0
        };
        let brackets = matches!(chrome.active_frame, TabFrame::Brackets);
        // Mirrors `draw_tab_bar_with_chrome`'s measurer exactly — the
        // no-paint twin must reserve the same columns the paint path did.
        let tab_widths: Vec<usize> = bar
            .tabs
            .iter()
            .map(|t| {
                let has_close = bar.show_tab_close && t.is_closable;
                let is_bracket = brackets && t.is_active;
                let base = display_width(&t.label);
                if is_bracket && has_close {
                    base + 1 + 1 + 1
                } else if is_bracket {
                    base + 2
                } else if has_close {
                    base + close_cols
                } else {
                    base
                }
            })
            .collect();
        let layout = bar.layout(
            rect.width,
            rect.height,
            0.0,
            |i| {
                let has_close = bar.show_tab_close && bar.tabs[i].is_closable;
                let is_bracket = brackets && bar.tabs[i].is_active;
                if is_bracket && has_close {
                    crate::TabMeasure::new(tab_widths[i] as f32, 1.0).with_trailing(1.0)
                } else if has_close {
                    crate::TabMeasure::new(tab_widths[i] as f32, close_cols as f32)
                } else {
                    crate::TabMeasure::new(tab_widths[i] as f32, 0.0)
                }
            },
            |i| crate::SegmentMeasure::new(bar.right_segments[i].width_cells as f32),
        );

        let mut hits = tab_bar_hits_from_layout(&layout, bar);
        crate::backend::shift_tab_bar_hits(&mut hits, rect.x as f64);

        let active_idx = bar.tabs.iter().position(|t| t.is_active);
        let reserved: usize = bar
            .right_segments
            .iter()
            .map(|s| s.width_cells as usize)
            .sum();
        let effective_tab_area = (rect.width as usize).saturating_sub(reserved);

        hits.correct_scroll_offset = if let Some(active) = active_idx {
            TabBar::fit_active_scroll_offset(active, bar.tabs.len(), effective_tab_area, |i| {
                tab_widths[i]
            })
        } else {
            bar.scroll_offset
        };

        hits
    }

    fn activity_bar_layout(&self, rect: QRect, bar: &ActivityBar) -> Vec<crate::ActivityBarRowHit> {
        let lh = 1.0_f32;
        activity_bar_hits(rect, bar, lh)
    }

    fn draw_terminal(&mut self, rect: QRect, term: &TerminalPrim) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_terminal called outside enter_frame_scope");
        crate::tui::draw_terminal(frame.buffer_mut(), area, term, &theme);
    }

    fn draw_terminal_divider(&mut self, rect: QRect) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_terminal_divider called outside enter_frame_scope");
        crate::tui::draw_terminal_divider(frame.buffer_mut(), area.x, area.y, area.height, &theme);
        // #492: the primitive takes no `WidgetId` of its own (there is at
        // most one divider on screen at a time), so register a fixed
        // chrome id — otherwise this frame is indistinguishable from one
        // where the no-op trait default silently dropped the call (C0
        // paint smoke, contract §5b).
        self.register_zone(WidgetId::new("chrome:terminal-divider"), rect);
    }

    fn draw_text_display(&mut self, rect: QRect, td: &TextDisplay) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_text_display called outside enter_frame_scope");
        crate::tui::draw_text_display(frame.buffer_mut(), area, td, &theme);
    }

    fn draw_command_line(&mut self, rect: QRect, cmd: &CommandLine) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_command_line called outside enter_frame_scope");
        crate::tui::command_line::draw_command_line(frame.buffer_mut(), area, cmd, &theme);
    }

    fn command_line_layout(
        &self,
        rect: QRect,
        cmd: &CommandLine,
    ) -> crate::primitives::command_line::CommandLineLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::command_line::tui_command_line_layout(cmd, area)
    }

    fn text_display_layout(
        &self,
        rect: QRect,
        td: &TextDisplay,
    ) -> crate::primitives::text_display::TextDisplayLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_text_display_layout(td, area)
    }

    fn draw_text_input(
        &mut self,
        rect: QRect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_text_input called outside enter_frame_scope");
        crate::tui::draw_text_input(frame.buffer_mut(), area, ti, &theme)
    }

    fn text_input_layout(
        &self,
        rect: QRect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_text_input_layout(ti, area)
    }

    fn draw_tooltip(&mut self, tooltip: &crate::Tooltip, layout: &crate::TooltipLayout) {
        self.draw_tooltip_with_chrome(tooltip, layout, &crate::TooltipChrome::default());
    }

    fn draw_tooltip_with_chrome(
        &mut self,
        tooltip: &crate::Tooltip,
        layout: &crate::TooltipLayout,
        chrome: &crate::TooltipChrome,
    ) {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_tooltip called outside enter_frame_scope");
        crate::tui::draw_tooltip_with_chrome(frame.buffer_mut(), tooltip, layout, chrome, &theme);
        // #542: register the tooltip's own surface so a structural-parity
        // observer (`ConformanceDriver::inventory().zones()`) can see the
        // tooltip was drawn at all, not just infer it from text presence —
        // `screen_has("Keybindings")` stayed true on both backends through
        // #541 even though only one of them drew the chrome around it.
        //
        // Register `tooltip_painted_bounds`, not the raw float
        // `layout.bounds` — `draw_tooltip` rounds to whole cells before
        // painting, and registering the unrounded rect would report a
        // surface that doesn't match what was actually drawn (#542 review).
        self.register_zone(
            tooltip.id.clone(),
            crate::tui::tooltip_painted_bounds(layout),
        );
    }

    fn draw_context_menu(
        &mut self,
        menu: &crate::ContextMenu,
        layout: &crate::ContextMenuLayout,
    ) -> Vec<(QRect, crate::WidgetId)> {
        // #455: see draw_palette for why this happens before the frame borrow.
        self.modal_stack.borrow_mut().mark_painted(&menu.id);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_context_menu called outside enter_frame_scope");
        crate::tui::draw_context_menu(frame.buffer_mut(), menu, layout, &theme);
        // TUI rasteriser doesn't return hit data — derive from layout.
        // The primitive's hit_test() is the canonical way; this Vec
        // is here for trait parity with GTK.
        let _ = menu;
        layout
            .hit_regions
            .iter()
            .filter_map(|(rect, hit)| match hit {
                crate::primitives::context_menu::ContextMenuHit::Item(id) => {
                    Some((*rect, id.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn draw_dialog(
        &mut self,
        dialog: &crate::primitives::dialog::Dialog,
        layout: &crate::primitives::dialog::DialogLayout,
    ) -> Vec<QRect> {
        // #455: see draw_palette for why this happens before the frame borrow.
        self.modal_stack.borrow_mut().mark_painted(&dialog.id);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_dialog called outside enter_frame_scope");
        crate::tui::draw_dialog(frame.buffer_mut(), dialog, layout, &theme);
        // Derive button rects from the layout (TUI rasteriser doesn't
        // return them; the primitive owns the layout).
        layout
            .visible_buttons
            .iter()
            .map(|vis| vis.bounds)
            .collect()
    }

    // ─── #13: trait coverage for the rest of the rasterised primitives ──

    fn draw_multi_section_view(
        &mut self,
        rect: QRect,
        view: &crate::primitives::multi_section_view::MultiSectionView,
    ) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_multi_section_view called outside enter_frame_scope");
        crate::tui::draw_multi_section_view(frame.buffer_mut(), area, view, &theme, nerd_fonts);
    }

    fn msv_layout(
        &self,
        rect: QRect,
        view: &crate::primitives::multi_section_view::MultiSectionView,
    ) -> crate::primitives::multi_section_view::MultiSectionViewLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_msv_layout(view, area)
    }

    fn msv_metrics(&self) -> crate::primitives::multi_section_view::MsvLayoutMetrics {
        crate::primitives::multi_section_view::MsvLayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        }
    }

    fn tree_layout(&self, rect: QRect, tree: &TreeView) -> crate::primitives::tree::TreeViewLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_tree_layout(tree, area)
    }

    fn form_layout(&self, rect: QRect, form: &Form) -> crate::primitives::form::FormLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_form_layout(form, area)
    }

    fn draw_editor(
        &mut self,
        rect: QRect,
        editor: &crate::primitives::editor::Editor,
    ) -> crate::backend::EditorPaintResult {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_editor called outside enter_frame_scope");
        let tui_result = crate::tui::draw_editor(frame.buffer_mut(), area, editor, &theme);
        // Cache for `render_frame` (quadraui#466) — `draw_editor` only sees
        // the buffer, not the `Frame`, so it can't call
        // `Frame::set_cursor_position` itself. See
        // `last_cursor_position`'s field doc for the full handoff.
        self.last_cursor_position = tui_result.cursor_position;
        #[allow(deprecated)] // issue #504: populate the deprecated cell-tuple
        // field too, until vimcode's `render_impl.rs` call site migrates to
        // `cursor_position_native` — see `EditorPaintResult::cursor_position`'s
        // doc for the full deprecation contract.
        crate::backend::EditorPaintResult {
            cursor_position: tui_result.cursor_position,
            // `tui_result.cursor_position` is the TUI-internal, already
            // cell-rounded `(u16, u16)` shape `Frame::set_cursor_position`
            // needs (see `last_cursor_position`'s doc); widen to the
            // portable `Point` (issue #504) for the trait's return value.
            cursor_position_native: tui_result
                .cursor_position
                .map(|(x, y)| crate::event::Point::new(x as f32, y as f32)),
        }
    }

    fn draw_message_list(
        &mut self,
        rect: QRect,
        list: &crate::primitives::message_list::MessageList,
    ) {
        let area = q_rect_to_ratatui(rect);
        let panel_bg = self.current_theme.background;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_message_list called outside enter_frame_scope");
        crate::tui::draw_message_list(frame.buffer_mut(), area, list, panel_bg);
    }

    fn draw_rich_text_popup(
        &mut self,
        popup: &crate::primitives::rich_text_popup::RichTextPopup,
        layout: &crate::primitives::rich_text_popup::RichTextPopupLayout,
    ) {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_rich_text_popup called outside enter_frame_scope");
        crate::tui::draw_rich_text_popup(frame.buffer_mut(), popup, layout, &theme);
    }

    fn draw_find_replace(
        &mut self,
        rect: QRect,
        panel: &crate::primitives::find_replace::FindReplacePanel,
    ) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        // TUI free function takes `editor_left: u16` for editor-relative
        // positioning. The trait abstraction passes the panel's full
        // rect; downstream consumers that want a non-zero editor offset
        // should compose into a sub-rect.
        let editor_left = area.x;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_find_replace called outside enter_frame_scope");
        crate::tui::draw_find_replace(frame.buffer_mut(), area, panel, &theme, editor_left);
    }

    fn draw_completions(
        &mut self,
        completions: &crate::primitives::completions::Completions,
        layout: &crate::primitives::completions::CompletionsLayout,
    ) {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_completions called outside enter_frame_scope");
        crate::tui::draw_completions(frame.buffer_mut(), completions, layout, &theme);
    }

    fn draw_scrollbar(
        &mut self,
        _rect: QRect,
        scrollbar: &crate::primitives::scrollbar::Scrollbar,
    ) {
        let theme = self.current_theme;
        let cell_bg = theme.background;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_scrollbar called outside enter_frame_scope");
        // The standalone TUI scrollbar primitive paints from its own
        // `track` bounds; `rect` is unused (the primitive owns layout).
        // Forward-compat parameter for backends that need a clip rect.
        crate::tui::draw_scrollbar(frame.buffer_mut(), scrollbar, &theme, cell_bg);
        // #492: a scrollbar paints only glyphs (thumb/track), so on a
        // pixel backend it would otherwise be indistinguishable from the
        // no-op default. Register the primitive's own id at its own
        // track bounds (not `_rect`, which the primitive ignores).
        self.register_zone(scrollbar.id.clone(), scrollbar.track);
    }

    fn draw_drop_overlay(&mut self, overlay: &crate::primitives::drop_zone::DropOverlay) {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_drop_overlay called outside enter_frame_scope");
        crate::tui::draw_drop_overlay(frame.buffer_mut(), overlay, &theme);
        // #492: `DropOverlay` carries no `WidgetId` (there is at most one
        // overlay active at a time), so register a fixed chrome id at
        // whichever sub-rect it actually drew — otherwise this frame is
        // indistinguishable from one where the no-op default silently
        // dropped the call (C0 paint smoke, contract §5b).
        if let Some(bounds) = overlay.highlight.or(overlay.insertion_bar) {
            self.register_zone(WidgetId::new("chrome:drop-overlay"), bounds);
        }
    }

    fn draw_menu_bar(
        &mut self,
        rect: QRect,
        bar: &MenuBar,
    ) -> crate::primitives::menu_bar::MenuBarLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_menu_bar called outside enter_frame_scope");
        crate::tui::draw_menu_bar(frame.buffer_mut(), area, bar, &theme)
    }

    fn menu_bar_layout(
        &self,
        rect: QRect,
        bar: &MenuBar,
    ) -> crate::primitives::menu_bar::MenuBarLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_menu_bar_layout(bar, area)
    }

    fn draw_split(&mut self, rect: QRect, split: &Split) -> crate::primitives::split::SplitLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_split called outside enter_frame_scope");
        let layout = crate::tui::draw_split(frame.buffer_mut(), area, split, &theme);
        // #492: `Split` paints a divider only — no text of its own — so a
        // registered zone is the only way this frame is attributable to
        // the primitive rather than indistinguishable from the trait's
        // no-op default (C0 paint smoke, contract §5b).
        self.register_zone(split.id.clone(), rect);
        layout
    }

    fn split_layout(&self, rect: QRect, split: &Split) -> crate::primitives::split::SplitLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_split_layout(split, area)
    }

    fn draw_split_tree(
        &mut self,
        rect: QRect,
        tree: &crate::primitives::split_tree::SplitTree,
    ) -> crate::primitives::split_tree::SplitTreeLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_split_tree called outside enter_frame_scope");
        let layout = crate::tui::draw_split_tree(frame.buffer_mut(), area, tree, &theme);
        // #492: dividers only, and `SplitTree` (unlike `Split`) carries no
        // id of its own — register a fixed chrome id, same pattern as
        // `draw_terminal_divider` / `draw_drop_overlay`.
        self.register_zone(WidgetId::new("chrome:split-tree"), rect);
        layout
    }

    fn split_tree_layout(
        &self,
        rect: QRect,
        tree: &crate::primitives::split_tree::SplitTree,
    ) -> crate::primitives::split_tree::SplitTreeLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_split_tree_layout(tree, area)
    }

    fn draw_panel(
        &mut self,
        rect: QRect,
        panel: &crate::primitives::panel::Panel,
    ) -> crate::primitives::panel::PanelLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_panel called outside enter_frame_scope");
        crate::tui::draw_panel(frame.buffer_mut(), area, panel, &theme)
    }

    fn panel_layout(
        &self,
        rect: QRect,
        panel: &crate::primitives::panel::Panel,
    ) -> crate::primitives::panel::PanelLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_panel_layout(panel, area)
    }

    fn draw_toast_stack(
        &mut self,
        rect: QRect,
        stack: &crate::primitives::toast::ToastStack,
    ) -> crate::primitives::toast::ToastStackLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_toast_stack called outside enter_frame_scope");
        crate::tui::draw_toast_stack(frame.buffer_mut(), area, stack, &theme)
    }

    fn toast_stack_layout(
        &self,
        rect: QRect,
        stack: &crate::primitives::toast::ToastStack,
    ) -> crate::primitives::toast::ToastStackLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_toast_stack_layout(stack, area)
    }

    fn draw_pipeline_view(
        &mut self,
        rect: QRect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_pipeline_view called outside enter_frame_scope");
        crate::tui::draw_pipeline_view(frame.buffer_mut(), area, view, &theme)
    }

    fn pipeline_view_layout(
        &self,
        rect: QRect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_pipeline_view_layout(view, area)
    }

    fn draw_progress(
        &mut self,
        rect: QRect,
        bar: &crate::primitives::progress::ProgressBar,
    ) -> crate::primitives::progress::ProgressBarLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_progress called outside enter_frame_scope");
        crate::tui::draw_progress(frame.buffer_mut(), area, bar, &theme)
    }

    fn progress_layout(
        &self,
        rect: QRect,
        bar: &crate::primitives::progress::ProgressBar,
    ) -> crate::primitives::progress::ProgressBarLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_progress_layout(bar, area)
    }

    fn draw_spinner(
        &mut self,
        rect: QRect,
        spinner: &crate::primitives::spinner::Spinner,
    ) -> crate::primitives::spinner::SpinnerLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_spinner called outside enter_frame_scope");
        crate::tui::draw_spinner(frame.buffer_mut(), area, spinner, &theme)
    }

    fn spinner_layout(
        &self,
        rect: QRect,
        spinner: &crate::primitives::spinner::Spinner,
    ) -> crate::primitives::spinner::SpinnerLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_spinner_layout(spinner, area)
    }

    fn draw_command_center(
        &mut self,
        rect: QRect,
        cc: &crate::primitives::command_center::CommandCenter,
    ) -> crate::primitives::command_center::CommandCenterLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_command_center called outside enter_frame_scope");
        crate::tui::draw_command_center(frame.buffer_mut(), area, cc, &theme)
    }

    fn command_center_layout(
        &self,
        rect: QRect,
        cc: &crate::primitives::command_center::CommandCenter,
    ) -> crate::primitives::command_center::CommandCenterLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_command_center_layout(cc, area)
    }

    fn draw_chart(
        &mut self,
        rect: QRect,
        chart: &crate::primitives::chart::Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) -> crate::primitives::chart::ChartLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_chart called outside enter_frame_scope");
        crate::tui::draw_chart(
            frame.buffer_mut(),
            area,
            chart,
            &theme,
            hovered_point,
            crosshair_x,
        )
    }

    fn chart_layout(
        &self,
        rect: QRect,
        chart: &crate::primitives::chart::Chart,
    ) -> crate::primitives::chart::ChartLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_chart_layout(chart, area)
    }

    fn draw_toolbar_interactive(
        &mut self,
        rect: QRect,
        bar: &crate::primitives::toolbar::Toolbar,
        interaction: &crate::interaction::InteractionState,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_toolbar_interactive called outside enter_frame_scope");
        crate::tui::draw_toolbar(
            frame.buffer_mut(),
            area,
            bar,
            &theme,
            interaction.hovered(),
            interaction.pressed(),
        )
    }

    fn toolbar_layout(
        &self,
        rect: QRect,
        bar: &crate::primitives::toolbar::Toolbar,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_toolbar_layout(bar, area)
    }

    fn draw_sidebar_panel_interactive(
        &mut self,
        rect: QRect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
        interaction: &crate::interaction::InteractionState,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        let (hovered_toolbar_id, pressed_toolbar_id) =
            (interaction.hovered(), interaction.pressed());
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_sidebar_panel called outside enter_frame_scope");
        crate::tui::draw_sidebar_panel(
            frame.buffer_mut(),
            area,
            panel,
            &theme,
            hovered_toolbar_id,
            pressed_toolbar_id,
        )
    }

    fn sidebar_panel_layout(
        &self,
        rect: QRect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_sidebar_panel_layout(panel, area)
    }

    fn draw_diff_view(
        &mut self,
        rect: QRect,
        view: &crate::primitives::diff_view::DiffView,
    ) -> crate::primitives::diff_view::DiffViewLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_diff_view called outside enter_frame_scope");
        crate::tui::draw_diff_view(frame.buffer_mut(), area, view, &theme)
    }

    fn draw_board(
        &mut self,
        rect: QRect,
        model: &crate::primitives::board::BoardModel,
    ) -> crate::primitives::board::BoardLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_board called outside enter_frame_scope");
        crate::tui::draw_board(frame.buffer_mut(), area, model, &theme)
    }

    fn board_layout(
        &self,
        rect: QRect,
        model: &crate::primitives::board::BoardModel,
    ) -> crate::primitives::board::BoardLayout {
        crate::tui::tui_board_layout(model, q_rect_to_ratatui(rect))
    }

    fn draw_minimap(
        &mut self,
        rect: QRect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::backend::MinimapPaintResult {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_minimap called outside enter_frame_scope");
        let layout = crate::tui::draw_minimap(frame.buffer_mut(), area, minimap, &theme);
        self.register_zone(minimap.id.clone(), rect);
        crate::backend::MinimapPaintResult {
            layout,
            painted: true,
        }
    }

    fn minimap_layout(
        &self,
        rect: QRect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::primitives::minimap::MinimapLayout {
        crate::tui::tui_minimap_layout(minimap, q_rect_to_ratatui(rect))
    }

    fn draw_image(
        &mut self,
        rect: QRect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let result = {
            let frame = self
                .current_frame_mut()
                .expect("TuiBackend::draw_image called outside enter_frame_scope");
            crate::tui::draw_image(frame.buffer_mut(), area, image, &theme)
        };
        self.register_zone(image.id.clone(), rect);
        result
    }
}

// ─── Cross-backend validation tests ──────────────────────────────────────────
//
// Phase B.4 Stage 3b: prove the `Backend` trait is genuinely consumable
// by app code that's *generic* over the backend, not just by `TuiBackend`
// specifically. A minimal `MockBackend` records each `draw_*` call into
// a `Vec<DrawCall>`; a generic `<B: Backend>` helper invokes the trait
// methods; assertions verify the calls landed.
//
// This is the architectural proof point Stage 3 was designed around:
// once the trait works against TuiBackend AND a foreign mock, future
// backends (GtkBackend in B.5, WinBackend in B.6, MacOSBackend in B.7)
// drop in without forking the app's render code.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{
        Clipboard, FileDialogOptions, MessageDialogChoice, MessageDialogOptions, Notification,
    };
    use crate::{ListItem, ListView, Palette, PaletteItem, StyledSpan, StyledText, WidgetId};

    /// Records every draw call so tests can assert what the trait
    /// boundary actually delivers.
    #[derive(Debug, Clone, PartialEq)]
    enum DrawCall {
        List { rect: QRect, item_count: usize },
        Palette { rect: QRect, item_count: usize },
    }

    struct NoopClipboard;
    impl Clipboard for NoopClipboard {
        fn read_text(&self) -> Option<String> {
            None
        }
        fn write_text(&self, _t: &str) {}
    }

    struct MockServices {
        clipboard: NoopClipboard,
    }
    impl MockServices {
        fn new() -> Self {
            Self {
                clipboard: NoopClipboard,
            }
        }
    }
    impl PlatformServices for MockServices {
        fn clipboard(&self) -> &dyn Clipboard {
            &self.clipboard
        }
        fn show_file_open_dialog(&self, _opts: FileDialogOptions) -> Option<std::path::PathBuf> {
            None
        }
        fn show_file_save_dialog(&self, _opts: FileDialogOptions) -> Option<std::path::PathBuf> {
            None
        }
        fn show_message_dialog(&self, _opts: MessageDialogOptions) -> Option<MessageDialogChoice> {
            None
        }
        fn send_notification(&self, _n: Notification) {}
        fn open_url(&self, _url: &str) {}
        fn platform_name(&self) -> &'static str {
            "mock"
        }
    }

    struct MockBackend {
        calls: Vec<DrawCall>,
        modal_stack: Rc<RefCell<ModalStack>>,
        drag_state: Rc<RefCell<DragState>>,
        services: MockServices,
        viewport: Viewport,
        theme: crate::Theme,
        focus: crate::focus::FocusManager,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                calls: Vec::new(),
                modal_stack: Rc::new(RefCell::new(ModalStack::new())),
                drag_state: Rc::new(RefCell::new(DragState::new())),
                services: MockServices::new(),
                viewport: Viewport::new(80.0, 24.0, 1.0),
                theme: crate::Theme::default(),
                focus: crate::focus::FocusManager::new(),
            }
        }
    }

    impl crate::backend::sealed::Sealed for MockBackend {}

    impl Backend for MockBackend {
        fn viewport(&self) -> Viewport {
            self.viewport
        }
        fn begin_frame(&mut self, viewport: Viewport) {
            self.viewport = viewport;
        }
        fn end_frame(&mut self) {}
        fn set_theme(&mut self, theme: crate::Theme) {
            self.theme = theme;
        }
        fn poll_events(&mut self) -> Vec<UiEvent> {
            Vec::new()
        }
        fn wait_events(&mut self, _t: Duration) -> Vec<UiEvent> {
            Vec::new()
        }
        fn waker(&self) -> std::sync::Arc<dyn Fn(crate::UserPayload) + Send + Sync> {
            // `MockBackend` only records `draw_*` calls for assertions — no
            // event loop for a wake to reach. No-op, matching `poll_events`/
            // `wait_events` above. Real cross-thread wake delivery is
            // covered against `TuiBackend` itself, not this recorder.
            std::sync::Arc::new(|_payload| {})
        }
        fn request_frame_in(&self, _delay: Duration) {
            // No event loop to wake, same rationale as `waker` above.
        }
        fn register_accelerator(&mut self, _a: &Accelerator) {}
        fn unregister_accelerator(&mut self, _id: &AcceleratorId) {}
        fn modal_stack_handle(&self) -> Rc<RefCell<ModalStack>> {
            self.modal_stack.clone()
        }
        fn drag_state_handle(&self) -> Rc<RefCell<DragState>> {
            self.drag_state.clone()
        }
        fn focus_manager(&self) -> &crate::focus::FocusManager {
            &self.focus
        }
        fn draw_focus_ring(&mut self, _r: QRect) {}
        fn services(&self) -> &dyn PlatformServices {
            &self.services
        }

        fn backend_caps(&self) -> crate::backend::BackendCaps {
            crate::backend::BackendCaps::empty()
        }

        fn draw_list(&mut self, rect: QRect, list: &ListView) {
            self.calls.push(DrawCall::List {
                rect,
                item_count: list.items.len(),
            });
        }

        fn draw_data_table(
            &mut self,
            _rect: QRect,
            _table: &crate::DataTable,
            _hovered_idx: Option<usize>,
        ) -> crate::DataTableLayout {
            crate::DataTableLayout {
                header_height: 0.0,
                row_height: 0.0,
                columns: Vec::new(),
                visible_rows: 0,
                viewport_width: 0.0,
                viewport_height: 0.0,
                scrollbar_width: 0.0,
                content_width: 0.0,
                h_scrollbar_height: 0.0,
                footer_height: 0.0,
                h_scroll: 0.0,
            }
        }
        fn data_table_layout(
            &self,
            _rect: QRect,
            _table: &crate::DataTable,
        ) -> crate::DataTableLayout {
            crate::DataTableLayout {
                header_height: 0.0,
                row_height: 0.0,
                columns: Vec::new(),
                visible_rows: 0,
                viewport_width: 0.0,
                viewport_height: 0.0,
                scrollbar_width: 0.0,
                content_width: 0.0,
                h_scrollbar_height: 0.0,
                footer_height: 0.0,
                h_scroll: 0.0,
            }
        }
        fn list_hscrollbar(&self, _rect: QRect, _list: &ListView) -> Option<crate::Scrollbar> {
            None
        }
        fn list_vscrollbar(&self, _rect: QRect, _list: &ListView) -> Option<crate::Scrollbar> {
            None
        }
        fn list_layout(&self, rect: QRect, list: &ListView) -> crate::ListViewLayout {
            crate::tui::tui_list_layout(q_rect_to_ratatui(rect), list)
        }
        fn draw_palette(&mut self, rect: QRect, palette: &Palette) {
            self.calls.push(DrawCall::Palette {
                rect,
                item_count: palette.items.len(),
            });
        }
        fn palette_layout(&self, rect: QRect, palette: &Palette) -> crate::PaletteLayout {
            crate::tui::tui_palette_layout(q_rect_to_ratatui(rect), palette)
        }

        // The other 7 trait methods are unimplemented — this mock only
        // records the ones the cross-backend test actually exercises.
        fn draw_tree(&mut self, _r: QRect, _t: &TreeView) {}
        fn draw_form(&mut self, _r: QRect, _f: &Form) {}
        fn draw_settings_chrome(
            &mut self,
            _r: QRect,
            _header_text: &str,
            _query: &str,
            _placeholder: &str,
            _active: bool,
        ) {
        }
        fn draw_status_bar_interactive(
            &mut self,
            _r: QRect,
            _b: &StatusBar,
            _interaction: &crate::interaction::InteractionState,
        ) -> crate::StatusBarLayout {
            crate::StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            }
        }
        #[allow(deprecated)] // `TabBarHits` is `#[deprecated]` (issue #823)
        fn draw_tab_bar(
            &mut self,
            _r: QRect,
            _b: &TabBar,
            _hovered_close_tab: Option<usize>,
        ) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        #[allow(deprecated)] // `TabBarHits` is `#[deprecated]` (issue #823)
        fn draw_tab_bar_icons(
            &mut self,
            _r: QRect,
            _b: &TabBar,
            _icons: &[Option<crate::TabIcon>],
            _hovered_close_tab: Option<usize>,
        ) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn draw_activity_bar(
            &mut self,
            _r: QRect,
            _b: &ActivityBar,
            _h: Option<usize>,
        ) -> Vec<crate::ActivityBarRowHit> {
            Vec::new()
        }
        fn draw_terminal(&mut self, _r: QRect, _t: &TerminalPrim) {}
        fn draw_terminal_divider(&mut self, _r: QRect) {}
        fn draw_text_display(&mut self, _r: QRect, _t: &TextDisplay) {}
        fn draw_command_line(&mut self, _r: QRect, _c: &CommandLine) {}
        fn command_line_layout(
            &self,
            _r: QRect,
            _c: &CommandLine,
        ) -> crate::primitives::command_line::CommandLineLayout {
            Default::default()
        }
        fn status_bar_layout(&self, _r: QRect, _b: &StatusBar) -> crate::StatusBarLayout {
            crate::StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            }
        }
        #[allow(deprecated)] // `TabBarHits` is `#[deprecated]` (issue #823)
        fn tab_bar_layout(&self, _r: QRect, _b: &TabBar) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        #[allow(deprecated)] // `TabBarHits` is `#[deprecated]` (issue #823)
        fn tab_bar_layout_icons(
            &self,
            _r: QRect,
            _b: &TabBar,
            _icons: &[Option<crate::TabIcon>],
        ) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn activity_bar_layout(
            &self,
            _r: QRect,
            _b: &ActivityBar,
        ) -> Vec<crate::ActivityBarRowHit> {
            Vec::new()
        }
        fn text_display_layout(
            &self,
            r: QRect,
            td: &TextDisplay,
        ) -> crate::primitives::text_display::TextDisplayLayout {
            td.layout(r.width, r.height, |_| {
                crate::primitives::text_display::TextDisplayLineMeasure::new(1.0)
            })
        }
        fn draw_text_input(
            &mut self,
            r: QRect,
            ti: &crate::primitives::text_input::TextInput,
        ) -> crate::primitives::text_input::TextInputLayout {
            ti.layout(
                r,
                crate::primitives::text_input::TextInputMeasure::new(1.0, 1.0),
            )
        }
        fn text_input_layout(
            &self,
            r: QRect,
            ti: &crate::primitives::text_input::TextInput,
        ) -> crate::primitives::text_input::TextInputLayout {
            ti.layout(
                r,
                crate::primitives::text_input::TextInputMeasure::new(1.0, 1.0),
            )
        }
        fn draw_tooltip(&mut self, _t: &crate::Tooltip, _l: &crate::TooltipLayout) {}
        fn draw_context_menu(
            &mut self,
            _m: &crate::ContextMenu,
            _l: &crate::ContextMenuLayout,
        ) -> Vec<(QRect, crate::WidgetId)> {
            Vec::new()
        }
        fn draw_dialog(
            &mut self,
            _d: &crate::primitives::dialog::Dialog,
            _l: &crate::primitives::dialog::DialogLayout,
        ) -> Vec<QRect> {
            Vec::new()
        }

        fn char_width(&self) -> f32 {
            1.0
        }
        fn line_height(&self) -> f32 {
            1.0
        }

        // ── #13: stubs for the trait methods added with this issue ──

        fn draw_multi_section_view(
            &mut self,
            _r: QRect,
            _v: &crate::primitives::multi_section_view::MultiSectionView,
        ) {
        }

        fn msv_layout(
            &self,
            r: QRect,
            v: &crate::primitives::multi_section_view::MultiSectionView,
        ) -> crate::primitives::multi_section_view::MultiSectionViewLayout {
            // Mock returns the primitive's natural layout with default
            // metrics — sufficient for cross-backend compile checks.
            v.layout(
                r,
                crate::primitives::multi_section_view::MsvLayoutMetrics::default(),
                |_| crate::primitives::multi_section_view::SectionMeasure::default(),
            )
        }

        fn msv_metrics(&self) -> crate::primitives::multi_section_view::MsvLayoutMetrics {
            crate::primitives::multi_section_view::MsvLayoutMetrics::default()
        }

        fn tree_layout(&self, r: QRect, t: &TreeView) -> crate::primitives::tree::TreeViewLayout {
            t.layout(r.width, r.height, |_| {
                crate::primitives::tree::TreeRowMeasure::new(1.0)
            })
        }

        fn form_layout(&self, r: QRect, form: &Form) -> crate::primitives::form::FormLayout {
            let area = q_rect_to_ratatui(r);
            crate::tui::tui_form_layout(form, area)
        }

        fn draw_editor(
            &mut self,
            _r: QRect,
            _e: &crate::primitives::editor::Editor,
        ) -> crate::backend::EditorPaintResult {
            crate::backend::EditorPaintResult::default()
        }

        fn draw_message_list(
            &mut self,
            _r: QRect,
            _l: &crate::primitives::message_list::MessageList,
        ) {
        }

        fn draw_rich_text_popup(
            &mut self,
            _p: &crate::primitives::rich_text_popup::RichTextPopup,
            _l: &crate::primitives::rich_text_popup::RichTextPopupLayout,
        ) {
        }

        fn draw_find_replace(
            &mut self,
            _r: QRect,
            _p: &crate::primitives::find_replace::FindReplacePanel,
        ) {
        }

        fn draw_completions(
            &mut self,
            _c: &crate::primitives::completions::Completions,
            _l: &crate::primitives::completions::CompletionsLayout,
        ) {
        }

        fn draw_scrollbar(&mut self, _r: QRect, _s: &crate::primitives::scrollbar::Scrollbar) {}
        fn draw_drop_overlay(&mut self, _o: &crate::primitives::drop_zone::DropOverlay) {}

        fn draw_menu_bar(
            &mut self,
            _r: QRect,
            bar: &MenuBar,
        ) -> crate::primitives::menu_bar::MenuBarLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            bar.layout(bounds, |_| {
                crate::primitives::menu_bar::MenuBarItemMeasure::new(0.0)
            })
        }

        fn menu_bar_layout(
            &self,
            _r: QRect,
            bar: &MenuBar,
        ) -> crate::primitives::menu_bar::MenuBarLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            bar.layout(bounds, |_| {
                crate::primitives::menu_bar::MenuBarItemMeasure::new(0.0)
            })
        }

        fn draw_split(
            &mut self,
            _r: QRect,
            split: &Split,
        ) -> crate::primitives::split::SplitLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            split.layout(bounds, crate::primitives::split::SplitMeasure::new(1.0))
        }

        fn split_layout(&self, _r: QRect, split: &Split) -> crate::primitives::split::SplitLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            split.layout(bounds, crate::primitives::split::SplitMeasure::new(1.0))
        }

        fn draw_split_tree(
            &mut self,
            _r: QRect,
            tree: &crate::primitives::split_tree::SplitTree,
        ) -> crate::primitives::split_tree::SplitTreeLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            tree.layout(
                bounds,
                crate::primitives::split_tree::SplitTreeMeasure::new(1.0),
            )
        }

        fn split_tree_layout(
            &self,
            _r: QRect,
            tree: &crate::primitives::split_tree::SplitTree,
        ) -> crate::primitives::split_tree::SplitTreeLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            tree.layout(
                bounds,
                crate::primitives::split_tree::SplitTreeMeasure::new(1.0),
            )
        }

        fn draw_panel(
            &mut self,
            _r: QRect,
            panel: &crate::primitives::panel::Panel,
        ) -> crate::primitives::panel::PanelLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            panel.layout(bounds, crate::primitives::panel::PanelMeasure::new(1.0))
        }

        fn panel_layout(
            &self,
            _r: QRect,
            panel: &crate::primitives::panel::Panel,
        ) -> crate::primitives::panel::PanelLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            panel.layout(bounds, crate::primitives::panel::PanelMeasure::new(1.0))
        }

        fn draw_toast_stack(
            &mut self,
            _r: QRect,
            stack: &crate::primitives::toast::ToastStack,
        ) -> crate::primitives::toast::ToastStackLayout {
            stack.layout(_r.x, _r.y, _r.width, _r.height, 1.0, 1.0, |_| {
                crate::primitives::toast::ToastMeasure::new(40.0, 1.0)
            })
        }

        fn toast_stack_layout(
            &self,
            _r: QRect,
            stack: &crate::primitives::toast::ToastStack,
        ) -> crate::primitives::toast::ToastStackLayout {
            stack.layout(_r.x, _r.y, _r.width, _r.height, 1.0, 1.0, |_| {
                crate::primitives::toast::ToastMeasure::new(40.0, 1.0)
            })
        }

        fn draw_pipeline_view(
            &mut self,
            _r: QRect,
            view: &crate::primitives::pipeline_view::PipelineView,
        ) -> crate::primitives::pipeline_view::PipelineViewLayout {
            view.layout(
                _r.x,
                _r.y,
                crate::primitives::pipeline_view::PipelineViewMeasure::new(
                    _r.width, _r.height, 4.0, 10.0,
                ),
            )
        }

        fn pipeline_view_layout(
            &self,
            _r: QRect,
            view: &crate::primitives::pipeline_view::PipelineView,
        ) -> crate::primitives::pipeline_view::PipelineViewLayout {
            view.layout(
                _r.x,
                _r.y,
                crate::primitives::pipeline_view::PipelineViewMeasure::new(
                    _r.width, _r.height, 4.0, 10.0,
                ),
            )
        }

        fn draw_progress(
            &mut self,
            _r: QRect,
            bar: &crate::primitives::progress::ProgressBar,
        ) -> crate::primitives::progress::ProgressBarLayout {
            bar.layout(
                _r.x,
                _r.y,
                crate::primitives::progress::ProgressBarMeasure::new(_r.width, _r.height),
            )
        }

        fn progress_layout(
            &self,
            _r: QRect,
            bar: &crate::primitives::progress::ProgressBar,
        ) -> crate::primitives::progress::ProgressBarLayout {
            bar.layout(
                _r.x,
                _r.y,
                crate::primitives::progress::ProgressBarMeasure::new(_r.width, _r.height),
            )
        }

        fn draw_spinner(
            &mut self,
            _r: QRect,
            spinner: &crate::primitives::spinner::Spinner,
        ) -> crate::primitives::spinner::SpinnerLayout {
            spinner.layout(
                _r.x,
                _r.y,
                crate::primitives::spinner::SpinnerMeasure::new(_r.width, 1.0),
            )
        }

        fn spinner_layout(
            &self,
            _r: QRect,
            spinner: &crate::primitives::spinner::Spinner,
        ) -> crate::primitives::spinner::SpinnerLayout {
            spinner.layout(
                _r.x,
                _r.y,
                crate::primitives::spinner::SpinnerMeasure::new(_r.width, 1.0),
            )
        }

        fn draw_command_center(
            &mut self,
            _r: QRect,
            cc: &crate::primitives::command_center::CommandCenter,
        ) -> crate::primitives::command_center::CommandCenterLayout {
            cc.layout(
                crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height),
                crate::primitives::command_center::CommandCenterMeasure {
                    arrow_width: 2.0,
                    gap: 1.0,
                    search_box_width: 0.0,
                    height: 1.0,
                },
            )
        }

        fn command_center_layout(
            &self,
            _r: QRect,
            cc: &crate::primitives::command_center::CommandCenter,
        ) -> crate::primitives::command_center::CommandCenterLayout {
            cc.layout(
                crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height),
                crate::primitives::command_center::CommandCenterMeasure {
                    arrow_width: 2.0,
                    gap: 1.0,
                    search_box_width: 0.0,
                    height: 1.0,
                },
            )
        }

        fn draw_chart(
            &mut self,
            _r: QRect,
            chart: &crate::primitives::chart::Chart,
            _hovered_point: Option<(usize, usize)>,
            _crosshair_x: Option<f64>,
        ) -> crate::primitives::chart::ChartLayout {
            chart.layout(
                _r.x,
                _r.y,
                crate::primitives::chart::ChartMeasure {
                    width: _r.width,
                    height: _r.height,
                    char_width: 1.0,
                    line_height: 1.0,
                },
            )
        }

        fn chart_layout(
            &self,
            _r: QRect,
            chart: &crate::primitives::chart::Chart,
        ) -> crate::primitives::chart::ChartLayout {
            chart.layout(
                _r.x,
                _r.y,
                crate::primitives::chart::ChartMeasure {
                    width: _r.width,
                    height: _r.height,
                    char_width: 1.0,
                    line_height: 1.0,
                },
            )
        }

        fn draw_toolbar_interactive(
            &mut self,
            r: QRect,
            bar: &crate::primitives::toolbar::Toolbar,
            _interaction: &crate::interaction::InteractionState,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            bar.layout(r.x, r.y, r.width, r.height, |_| {
                crate::primitives::toolbar::ToolbarItemMeasure::new(0.0)
            })
        }

        fn toolbar_layout(
            &self,
            r: QRect,
            bar: &crate::primitives::toolbar::Toolbar,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            bar.layout(r.x, r.y, r.width, r.height, |_| {
                crate::primitives::toolbar::ToolbarItemMeasure::new(0.0)
            })
        }

        fn draw_sidebar_panel_interactive(
            &mut self,
            r: QRect,
            panel: &crate::primitives::sidebar_panel::SidebarPanel,
            _interaction: &crate::interaction::InteractionState,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            panel.layout(
                r,
                crate::primitives::sidebar_panel::SidebarPanelMeasure::new(1.0, 0.0),
                |_| crate::primitives::toolbar::ToolbarItemMeasure::new(0.0),
            )
        }

        fn sidebar_panel_layout(
            &self,
            r: QRect,
            panel: &crate::primitives::sidebar_panel::SidebarPanel,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            panel.layout(
                r,
                crate::primitives::sidebar_panel::SidebarPanelMeasure::new(1.0, 0.0),
                |_| crate::primitives::toolbar::ToolbarItemMeasure::new(0.0),
            )
        }

        fn draw_diff_view(
            &mut self,
            _r: QRect,
            view: &crate::primitives::diff_view::DiffView,
        ) -> crate::primitives::diff_view::DiffViewLayout {
            crate::primitives::diff_view::DiffViewLayout {
                visible_rows: 0,
                total_rows: view.total_rows(),
            }
        }

        fn draw_board(
            &mut self,
            r: QRect,
            model: &crate::primitives::board::BoardModel,
        ) -> crate::primitives::board::BoardLayout {
            crate::primitives::board::board_layout(
                model,
                r.x,
                r.y,
                r.width,
                r.height,
                crate::primitives::board::BoardMeasure::new(
                    crate::tui::board::TUI_BOARD_COL_MIN_CELLS,
                    1.0,
                    1.0,
                    crate::tui::board::TUI_BOARD_CARD_H,
                    0.0,
                ),
            )
        }

        fn board_layout(
            &self,
            r: QRect,
            model: &crate::primitives::board::BoardModel,
        ) -> crate::primitives::board::BoardLayout {
            crate::primitives::board::board_layout(
                model,
                r.x,
                r.y,
                r.width,
                r.height,
                crate::primitives::board::BoardMeasure::new(
                    crate::tui::board::TUI_BOARD_COL_MIN_CELLS,
                    1.0,
                    1.0,
                    crate::tui::board::TUI_BOARD_CARD_H,
                    0.0,
                ),
            )
        }

        fn draw_minimap(
            &mut self,
            _r: QRect,
            _m: &crate::primitives::minimap::Minimap,
        ) -> crate::backend::MinimapPaintResult {
            crate::backend::MinimapPaintResult::default()
        }

        fn minimap_layout(
            &self,
            _r: QRect,
            _m: &crate::primitives::minimap::Minimap,
        ) -> crate::primitives::minimap::MinimapLayout {
            crate::primitives::minimap::MinimapLayout::default()
        }

        fn draw_image(
            &mut self,
            _r: QRect,
            _i: &crate::primitives::image::Image,
        ) -> crate::backend::ImagePaintResult {
            crate::backend::ImagePaintResult::Unsupported
        }
    }

    /// Generic helper — the minimal "app render code" that consumes
    /// `Backend` through `<B>`. Future backends slot in here without
    /// changes.
    fn paint_overlays<B: Backend>(backend: &mut B, palette: &Palette, list: &ListView) {
        backend.draw_palette(QRect::new(10.0, 5.0, 60.0, 14.0), palette);
        backend.draw_list(QRect::new(0.0, 20.0, 80.0, 4.0), list);
    }

    fn sample_palette() -> Palette {
        Palette {
            id: WidgetId::new("test:palette"),
            title: "Pick one".to_string(),
            query: String::new(),
            query_cursor: 0,
            items: vec![
                PaletteItem {
                    text: StyledText {
                        spans: vec![StyledSpan::plain("alpha")],
                    },
                    detail: None,
                    icon: None,
                    match_positions: Vec::new(),
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
                PaletteItem {
                    text: StyledText {
                        spans: vec![StyledSpan::plain("beta")],
                    },
                    detail: None,
                    icon: None,
                    match_positions: Vec::new(),
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
            ],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 2,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: crate::primitives::palette::PaletteMode::List,
        }
    }

    fn sample_list() -> ListView {
        ListView {
            id: WidgetId::new("test:list"),
            title: None,
            items: vec![ListItem {
                text: StyledText {
                    spans: vec![StyledSpan::plain("only")],
                },
                icon: None,
                detail: None,
                decoration: crate::Decoration::Normal,
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    #[test]
    fn paint_overlays_records_through_mock_backend() {
        let mut mock = MockBackend::new();
        let palette = sample_palette();
        let list = sample_list();

        paint_overlays(&mut mock, &palette, &list);

        assert_eq!(mock.calls.len(), 2);
        assert!(matches!(
            mock.calls[0],
            DrawCall::Palette { item_count: 2, .. }
        ));
        assert!(matches!(
            mock.calls[1],
            DrawCall::List { item_count: 1, .. }
        ));
    }

    #[test]
    fn paint_overlays_compiles_against_tui_backend() {
        // Compile-only assertion — the same generic function used with
        // MockBackend above is also valid for TuiBackend. We don't run
        // the draws (they require an active frame scope) but the type
        // monomorphisation proves the trait constraint is satisfied
        // for every backend impl.
        let _: fn(&mut TuiBackend, &Palette, &ListView) = paint_overlays::<TuiBackend>;
    }

    #[test]
    fn mock_backend_modal_stack_routes_through_trait() {
        // Modal stack is on the trait too — backends that implement it
        // wire into `crate::dispatch::dispatch_mouse_down` automatically.
        let mock = MockBackend::new();
        mock.modal_stack_handle()
            .borrow_mut()
            .push(WidgetId::new("test:popup"), QRect::new(0.0, 0.0, 10.0, 5.0));
        assert_eq!(mock.modal_stack_handle().borrow().len(), 1);
    }

    /// quadraui#699: `TuiBackend::modal_stack_handle` must hand back a
    /// handle that shares state with the backend's own modal stack (the
    /// same guarantee `GtkBackend::modal_stack_handle` already proves in
    /// `gtk::backend::tests::gtk_backend_modal_stack_handle_shares_state`)
    /// — two independently-obtained handles must observe each other's
    /// writes, not two disconnected copies.
    #[test]
    fn tui_backend_modal_stack_handle_shares_state() {
        let backend = TuiBackend::new();
        let h1 = backend.modal_stack_handle();
        let h2 = backend.modal_stack_handle();
        h1.borrow_mut()
            .push(WidgetId::new("test:popup"), QRect::new(0.0, 0.0, 10.0, 5.0));
        assert_eq!(h2.borrow().len(), 1);
    }

    /// quadraui#704: the bridge methods this issue removed
    /// (`modal_stack_mut()` / `drag_and_modal_mut()`) synthesized a
    /// `&mut ModalStack` via `unsafe { Rc::as_ptr(..) }`, which bypassed
    /// `RefCell`'s runtime borrow check entirely — two live mutable
    /// aliases would silently corrupt state instead of panicking. Now
    /// that `modal_stack_handle()` is the only way to reach the stack,
    /// holding a `borrow_mut()` while a second one is taken through the
    /// same (or another) handle must panic loudly, not alias silently.
    #[test]
    #[should_panic(expected = "already borrowed")]
    fn modal_stack_handle_reentrant_borrow_mut_panics_loudly() {
        let backend = TuiBackend::new();
        let handle = backend.modal_stack_handle();
        let _first = handle.borrow_mut();
        let _second = handle.borrow_mut(); // must panic: real double-borrow check
    }

    /// quadraui#699: the whole point of `modal_stack_handle` is that
    /// the handle outlives the borrow that produced it — this is the
    /// stash-then-reuse pattern GTK hosts
    /// (and vimcode's macOS host, once #699 lands there too) depend on.
    /// Prove it compiles and behaves through `&mut dyn Backend`, not
    /// just the concrete `TuiBackend` type.
    #[test]
    fn modal_stack_handle_outlives_the_backend_borrow_through_the_trait() {
        let mut backend = TuiBackend::new();
        let stack_rc = {
            let dyn_backend: &mut dyn Backend = &mut backend;
            dyn_backend.modal_stack_handle() // stash, then the `&mut dyn Backend` borrow ends
        };
        // `backend` is usable again here — the handle didn't keep it borrowed.
        backend.begin_frame(Viewport::new(80.0, 24.0, 1.0));
        stack_rc
            .borrow_mut()
            .push(WidgetId::new("test:popup"), QRect::new(0.0, 0.0, 10.0, 5.0));
        assert_eq!(backend.modal_stack_handle().borrow().len(), 1);
    }

    /// quadraui#699: same shared-state guarantee as
    /// `tui_backend_modal_stack_handle_shares_state`, for
    /// `drag_state_handle`.
    #[test]
    fn tui_backend_drag_state_handle_shares_state() {
        let backend = TuiBackend::new();
        let h1 = backend.drag_state_handle();
        let h2 = backend.drag_state_handle();
        h1.borrow_mut().begin(DragTarget::TextSelection {
            region: WidgetId::new("r"),
            anchor: Point::new(0.0, 0.0),
        });
        assert!(h2.borrow().is_active());
    }

    /// #455 regression: `TuiBackend::draw_dialog` must mark the modal
    /// stack entry it paints, so `ModalStack::unpainted_ids` can catch a
    /// backend that registers a modal for hit-testing but never actually
    /// paints it — vimcode#587's exact failure shape ("registered but
    /// invisible"), reproduced here at the backend-wiring level rather
    /// than `ModalStack` in isolation (already covered in
    /// `crate::modal_stack::tests`).
    #[test]
    fn draw_dialog_marks_its_modal_stack_entry_painted() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let dialog_id = WidgetId::new("confirm");
        let dialog = crate::Dialog {
            id: dialog_id.clone(),
            title: crate::StyledText::plain("Confirm"),
            body: vec![crate::StyledText::plain("Really?")],
            table: None,
            buttons: vec![crate::DialogButton {
                id: WidgetId::new("ok"),
                label: "OK".into(),
                is_default: true,
                is_cancel: false,
                tint: None,
            }],
            severity: None,
            vertical_buttons: false,
            input: None,
        };
        let measure = crate::DialogMeasure {
            width: 40.0,
            title_height: 1.0,
            body_height: 1.0,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: 1.0,
            button_width: 10.0,
            button_gap: 2.0,
            padding: 1.0,
        };
        let viewport_rect = QRect::new(0.0, 0.0, 80.0, 24.0);
        let layout = dialog.layout(viewport_rect, measure, |_| {
            crate::ToolbarItemMeasure::new(0.0)
        });

        let mut backend = TuiBackend::new();
        backend.begin_frame(Viewport::new(80.0, 24.0, 1.0));
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push(dialog_id.clone(), layout.bounds);

        // Registered but not yet drawn this frame — exactly the drift
        // #455 wants surfaced.
        assert_eq!(
            backend.modal_stack_handle().borrow().unpainted_ids(),
            vec![dialog_id.clone()]
        );

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        terminal
            .draw(|frame| {
                backend.enter_frame_scope(frame, |b| {
                    b.draw_dialog(&dialog, &layout);
                });
            })
            .expect("draw");

        // draw_dialog ran through the real trait method and marked its
        // own id painted — the drift is gone for this frame.
        assert!(backend
            .modal_stack_handle()
            .borrow()
            .unpainted_ids()
            .is_empty());

        // The next frame must reset the mark: a backend that stops
        // calling draw_dialog (even though the modal stays registered)
        // has to be caught again, not remembered as painted forever.
        backend.begin_frame(Viewport::new(80.0, 24.0, 1.0));
        assert_eq!(
            backend.modal_stack_handle().borrow().unpainted_ids(),
            vec![dialog_id]
        );
    }

    // ─── Stage 6: accelerator matching ──────────────────────────────────────

    use crate::{Key, Modifiers, NamedKey};

    fn ctrl_p_keypress() -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char('p'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }
    }

    fn make_acc(id: &str, binding: &str) -> Accelerator {
        Accelerator {
            id: AcceleratorId::new(id),
            binding: KeyBinding::Literal(binding.to_string()),
            scope: AcceleratorScope::Global,
            label: None,
        }
    }

    /// Four `Fixed(30.0)` columns (content boundaries 0/30/60/90/120)
    /// in a 60-wide viewport — the #550 repro geometry.
    fn wide_hscroll_table() -> crate::DataTable {
        crate::DataTable {
            id: WidgetId::new("wide-hscroll"),
            columns: ["a", "b", "c", "d"]
                .iter()
                .map(|t| crate::Column {
                    title: (*t).into(),
                    width: crate::ColumnWidth::Fixed(30.0),
                    align: crate::ColumnAlign::Left,
                })
                .collect(),
            rows: vec![crate::DataRow {
                cells: vec![
                    StyledText::plain("a-one"),
                    StyledText::plain("b-two"),
                    StyledText::plain("c-three"),
                    StyledText::plain("d-four"),
                ],
                decoration: crate::Decoration::Normal,
            }],
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: Some(120.0),
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        }
    }

    /// #550 round 3: `Backend::data_table_layout` is the *cache-free,
    /// layout-on-demand* hit-test path (see
    /// `examples/common/data_table_app.rs`, which calls it from every
    /// mouse handler without repainting). It resolved geometry with its
    /// own `table.layout(..)` call and so returned the raw fractional
    /// `h_scroll`, while `draw_data_table` painted at the rounded one —
    /// reopening the original defect through a second entry point.
    #[test]
    fn data_table_layout_rounds_h_scroll_like_the_renderer() {
        let backend = TuiBackend::new();
        let rect = QRect::new(0.0, 0.0, 60.0, 3.0);
        let mut table = wide_hscroll_table();

        // A trackpad/wheel h-scroll lands on an arbitrary fraction; the
        // example's `UiEvent::Scroll` handler does no rounding at all.
        table.h_scroll = 23.7;
        let layout = backend.data_table_layout(rect, &table);
        assert_eq!(
            layout.h_scroll, 24.0,
            "layout-on-demand must carry the same rounded offset the renderer paints at"
        );

        // The review's exact repro value, and the click it misroutes.
        table.h_scroll = 15.6;
        let layout = backend.data_table_layout(rect, &table);
        assert_eq!(layout.h_scroll, 16.0);
        assert_eq!(
            layout.column_hit(14.0),
            Some(1),
            "integer click x=14 sits on c1 as painted at h_scroll=15.6"
        );
        assert_ne!(
            layout.column_hit(14.0),
            Some(0),
            "un-rounded 15.6 would put this at content x=29.6, inside c0"
        );
    }

    /// The two TUI entry points must be interchangeable: a consumer that
    /// hit-tests against a repaint-free layout has to land on exactly
    /// what the last `draw_data_table` put on screen.
    #[test]
    fn data_table_layout_matches_draw_data_table_at_every_fractional_offset() {
        let backend = TuiBackend::new();
        let rect = QRect::new(0.0, 0.0, 60.0, 3.0);
        let area = q_rect_to_ratatui(rect);
        let mut table = wide_hscroll_table();

        for tenths in 0..=600u32 {
            table.h_scroll = tenths as f32 / 10.0;
            let on_demand = backend.data_table_layout(rect, &table);
            let mut buf = ratatui::buffer::Buffer::empty(area);
            let painted =
                crate::tui::draw_data_table(&mut buf, area, &table, &backend.current_theme, None);
            assert_eq!(
                on_demand, painted,
                "layouts diverged at h_scroll = {}",
                table.h_scroll
            );
        }
    }

    #[test]
    fn accelerator_match_replaces_keypressed_with_accelerator() {
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("tui.fuzzy_finder", "<C-p>"));

        let mut events = vec![ctrl_p_keypress()];
        backend.apply_accelerators(&mut events);

        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::Accelerator(id, mods) => {
                assert_eq!(id.as_str(), "tui.fuzzy_finder");
                assert!(mods.ctrl);
            }
            other => panic!("expected Accelerator, got {:?}", other),
        }
    }

    #[test]
    fn accelerator_match_named_keys() {
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("debug.continue", "<F5>"));

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Named(NamedKey::F(5)),
            modifiers: Modifiers::default(),
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);

        match &events[0] {
            UiEvent::Accelerator(id, _) => assert_eq!(id.as_str(), "debug.continue"),
            other => panic!("expected Accelerator, got {:?}", other),
        }
    }

    #[test]
    fn accelerator_match_uppercase_letter_normalised() {
        // `<C-S-T>` and a Shift+T keypress (which arrives as Char('T')
        // from crossterm with SHIFT in modifiers) must match.
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("test.upper", "<C-S-T>"));

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Char('T'),
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);

        match &events[0] {
            UiEvent::Accelerator(id, _) => assert_eq!(id.as_str(), "test.upper"),
            other => panic!("expected Accelerator, got {:?}", other),
        }
    }

    #[test]
    fn accelerator_no_match_stays_keypressed() {
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("tui.fuzzy_finder", "<C-p>"));

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Char('q'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);

        assert!(matches!(events[0], UiEvent::KeyPressed { .. }));
    }

    #[test]
    fn accelerator_modifier_mismatch_no_match() {
        // `<C-p>` should NOT fire on `p` alone (no modifiers).
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("tui.fuzzy_finder", "<C-p>"));

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Char('p'),
            modifiers: Modifiers::default(),
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);

        assert!(matches!(events[0], UiEvent::KeyPressed { .. }));
    }

    #[test]
    fn accelerator_unregister_removes_match() {
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("tui.fuzzy_finder", "<C-p>"));
        backend.unregister_accelerator(&AcceleratorId::new("tui.fuzzy_finder"));

        let mut events = vec![ctrl_p_keypress()];
        backend.apply_accelerators(&mut events);

        assert!(matches!(events[0], UiEvent::KeyPressed { .. }));
    }

    #[test]
    fn accelerator_re_register_replaces_binding() {
        // Registering the same id twice should swap the binding, not
        // accumulate stale entries.
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("test.toggle", "<C-p>"));
        backend.register_accelerator(&make_acc("test.toggle", "<C-q>"));

        let mut events = vec![ctrl_p_keypress()];
        backend.apply_accelerators(&mut events);
        assert!(
            matches!(events[0], UiEvent::KeyPressed { .. }),
            "old binding must not match after re-register"
        );

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Char('q'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);
        assert!(matches!(&events[0], UiEvent::Accelerator(id, _) if id.as_str() == "test.toggle"));
    }

    #[test]
    fn accelerator_widget_scope_skipped() {
        // Backend doesn't know which widget has focus; widget-scoped
        // accelerators must NOT match here. The app keeps inline
        // matching for those.
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&Accelerator {
            id: AcceleratorId::new("widget.local"),
            binding: KeyBinding::Literal("<C-p>".into()),
            scope: AcceleratorScope::Widget(WidgetId::new("test:input")),
            label: None,
        });

        let mut events = vec![ctrl_p_keypress()];
        backend.apply_accelerators(&mut events);

        assert!(matches!(events[0], UiEvent::KeyPressed { .. }));
    }

    // ── Text selection / highlight round-trip tests ──────────────────────────

    /// Build a minimal ratatui buffer filled with known text, set up a
    /// text selection, and verify that:
    ///   1. `apply_selection_highlight` inverts the selected cells.
    ///   2. `extract_selection_text` returns the trimmed, newline-joined text.
    #[test]
    fn selection_highlight_and_extract_round_trip() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;
        use ratatui::style::Color as RC;

        // 10 wide × 3 tall buffer filled with known characters.
        let area = RRect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);

        // Row 0: "hello     " (trailing spaces)
        // Row 1: "world     "
        // Row 2: "!         "
        for (col, ch) in "hello     ".chars().enumerate() {
            buf[(col as u16, 0)]
                .set_char(ch)
                .set_fg(RC::White)
                .set_bg(RC::Black);
        }
        for (col, ch) in "world     ".chars().enumerate() {
            buf[(col as u16, 1)]
                .set_char(ch)
                .set_fg(RC::White)
                .set_bg(RC::Black);
        }
        for (col, ch) in "!         ".chars().enumerate() {
            buf[(col as u16, 2)]
                .set_char(ch)
                .set_fg(RC::White)
                .set_bg(RC::Black);
        }

        // Register text region and set selection: anchor at (0,0), focus at (0,2).
        // That covers rows 0–2, starting from col 0.
        let mut backend = TuiBackend::new();
        backend.register_text_region(crate::dispatch::TextRegion {
            id: WidgetId::new("log:body"),
            bounds: crate::event::Rect::new(0.0, 0.0, 10.0, 3.0),
            lines: vec![],
        });
        backend.set_active_text_selection(
            WidgetId::new("log:body"),
            Point::new(0.0, 0.0),
            Point::new(0.0, 2.0),
        );

        // 1. Extract text before highlight (cells still normal).
        let text = backend.extract_selection_text(&buf);
        // anchor(0,0) → focus(0,2):
        //   row 0: col 0..10 → "hello     " trimmed → "hello"
        //   row 1: col 0..10 → "world     " trimmed → "world"
        //   row 2: col 0..1  → "!" trimmed → "!"
        assert_eq!(text, "hello\nworld\n!");

        // 2. Apply highlight — selected cells should swap fg/bg.
        backend.apply_selection_highlight(&mut buf);
        // Spot-check: (0,0) should be inverted (fg=Black, bg=White).
        assert_eq!(buf[(0u16, 0u16)].fg, RC::Black);
        assert_eq!(buf[(0u16, 0u16)].bg, RC::White);
        // A cell outside the last row's range (col 1 of row 2 is outside
        // 0..1, so col=1 should be un-inverted).
        assert_eq!(buf[(1u16, 2u16)].fg, RC::White);
        assert_eq!(buf[(1u16, 2u16)].bg, RC::Black);
    }

    #[test]
    fn selection_clears_on_clear_text_selection() {
        let mut backend = TuiBackend::new();
        backend.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.0),
        );
        assert!(backend.active_text_selection().is_some());
        backend.clear_text_selection();
        assert!(backend.active_text_selection().is_none());
        // Drag state should also be cleared if it was a TextSelection.
        backend
            .drag_state
            .borrow_mut()
            .begin(DragTarget::TextSelection {
                region: WidgetId::new("r"),
                anchor: Point::new(0.0, 0.0),
            });
        backend.clear_text_selection();
        assert!(!backend.drag_state.borrow().is_active());
    }

    #[test]
    fn text_regions_cleared_on_begin_frame() {
        let mut backend = TuiBackend::new();
        backend.register_text_region(crate::dispatch::TextRegion {
            id: WidgetId::new("r"),
            bounds: crate::event::Rect::new(0.0, 0.0, 40.0, 20.0),
            lines: vec![],
        });
        assert_eq!(backend.text_regions().len(), 1);
        backend.begin_frame(Viewport::default());
        assert_eq!(
            backend.text_regions().len(),
            0,
            "text_regions must be cleared on begin_frame"
        );
    }

    /// Verify that `select_all_text_region` sets the full viewport bounds as
    /// the active selection. This directly tests the core acceptance criterion:
    /// "Ctrl-A sets the expected full range; extracted text == full region
    /// content" (the extraction half is covered by the round-trip test above).
    #[test]
    fn select_all_text_region_sets_full_bounds() {
        let mut backend = TuiBackend::new();
        backend.register_text_region(crate::dispatch::TextRegion {
            id: WidgetId::new("body"),
            bounds: crate::event::Rect::new(0.0, 0.0, 10.0, 5.0),
            lines: vec![],
        });
        assert!(
            backend.select_all_text_region(),
            "should return true when exactly one region is registered"
        );
        let sel = backend
            .active_text_selection()
            .expect("selection should be active after select_all_text_region");
        assert_eq!(
            sel.anchor,
            Point::new(0.0, 0.0),
            "anchor should be at top-left of region bounds"
        );
        assert_eq!(
            sel.focus,
            Point::new(10.0, 5.0),
            "focus should be at bottom-right of region bounds"
        );
    }

    /// Verify that `select_all_text_region` returns `false` when no regions
    /// are registered, and the fallthrough path is taken.
    #[test]
    fn select_all_text_region_returns_false_when_no_regions() {
        let mut backend = TuiBackend::new();
        assert!(
            !backend.select_all_text_region(),
            "should return false with no registered regions"
        );
        assert!(
            backend.active_text_selection().is_none(),
            "no selection should be set when select_all returns false"
        );
    }

    // ── ActivityBar keyboard focus tests ─────────────────────────────────────

    /// `begin_frame` clears the focused activity bar so stale state from
    /// the previous render doesn't persist.
    #[test]
    fn focused_activity_bar_cleared_on_begin_frame() {
        let mut backend = TuiBackend::new();
        backend.focused_activity_bar = Some(WidgetId::new("demo:bar"));
        backend.begin_frame(Viewport::default());
        assert!(
            backend.focused_activity_bar.is_none(),
            "focused_activity_bar must be cleared by begin_frame"
        );
    }

    /// When an `ActivityBar` with `is_keyboard_focused = true` is drawn,
    /// `apply_dispatch` converts the next `KeyPressed` into
    /// `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })`.
    #[test]
    fn activity_bar_focused_redirects_key_presses() {
        use crate::ActivityBarEvent;

        let mut backend = TuiBackend::new();
        // Simulate the render pass recording a focused bar.
        backend.focused_activity_bar = Some(WidgetId::new("demo:bar"));

        let raw = vec![UiEvent::KeyPressed {
            key: Key::Char('j'),
            modifiers: Modifiers::default(),
            repeat: false,
        }];
        let out = backend.apply_dispatch(raw);
        assert_eq!(
            out.len(),
            1,
            "one input event must produce one output event"
        );
        match &out[0] {
            UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { key, modifiers }) => {
                assert_eq!(id.as_str(), "demo:bar");
                assert_eq!(key, "j");
                assert_eq!(*modifiers, Modifiers::default());
            }
            other => panic!("expected UiEvent::ActivityBar KeyPressed, got {other:?}"),
        }
    }

    /// Without a focused activity bar, raw `KeyPressed` events pass through
    /// unchanged (no accidental interception).
    #[test]
    fn activity_bar_unfocused_passes_key_presses_through() {
        let mut backend = TuiBackend::new();
        // No focused bar.
        assert!(backend.focused_activity_bar.is_none());

        let raw = vec![UiEvent::KeyPressed {
            key: Key::Char('j'),
            modifiers: Modifiers::default(),
            repeat: false,
        }];
        let out = backend.apply_dispatch(raw);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(
                &out[0],
                UiEvent::KeyPressed {
                    key: Key::Char('j'),
                    ..
                }
            ),
            "key must pass through unchanged when no bar is focused"
        );
    }

    /// `key_to_activity_bar_string` maps printable chars and common
    /// named keys to their expected string forms.
    #[test]
    fn key_to_activity_bar_string_maps_correctly() {
        use crate::primitives::activity_bar::key_to_activity_bar_string;

        assert_eq!(key_to_activity_bar_string(&Key::Char('j')), "j");
        assert_eq!(key_to_activity_bar_string(&Key::Char('K')), "K");
        assert_eq!(
            key_to_activity_bar_string(&Key::Named(NamedKey::Escape)),
            "Escape"
        );
        assert_eq!(
            key_to_activity_bar_string(&Key::Named(NamedKey::Enter)),
            "Enter"
        );
        assert_eq!(key_to_activity_bar_string(&Key::Named(NamedKey::Up)), "Up");
        assert_eq!(
            key_to_activity_bar_string(&Key::Named(NamedKey::Down)),
            "Down"
        );
    }

    // ── coalesce_mouse_moved tests ────────────────────────────────────────────

    /// A burst of N consecutive `MouseMoved` events collapses to a single
    /// event at the final position.
    #[test]
    fn coalesce_collapses_consecutive_mouse_moved() {
        use crate::ButtonMask;
        let raw = vec![
            UiEvent::MouseMoved {
                position: Point::new(1.0, 1.0),
                buttons: ButtonMask::default(),
            },
            UiEvent::MouseMoved {
                position: Point::new(2.0, 1.0),
                buttons: ButtonMask::default(),
            },
            UiEvent::MouseMoved {
                position: Point::new(3.0, 1.0),
                buttons: ButtonMask::default(),
            },
        ];
        let out = coalesce_mouse_moved(raw);
        assert_eq!(out.len(), 1, "three consecutive moves must collapse to one");
        assert!(
            matches!(&out[0], UiEvent::MouseMoved { position, .. } if position.x == 3.0),
            "surviving event must carry the final position (x=3), got {:?}",
            out
        );
    }

    /// A `MouseDown … MouseMoved×N … MouseUp` burst collapses the moves
    /// to the final position while preserving relative order with the
    /// flanking down/up events.
    #[test]
    fn coalesce_preserves_ordering_around_down_and_up() {
        use crate::{ButtonMask, Modifiers, MouseButton};
        let raw = vec![
            UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(0.0, 0.0),
                modifiers: Modifiers::default(),
            },
            UiEvent::MouseMoved {
                position: Point::new(1.0, 0.0),
                buttons: ButtonMask {
                    left: true,
                    ..ButtonMask::default()
                },
            },
            UiEvent::MouseMoved {
                position: Point::new(2.0, 0.0),
                buttons: ButtonMask {
                    left: true,
                    ..ButtonMask::default()
                },
            },
            UiEvent::MouseMoved {
                position: Point::new(3.0, 0.0),
                buttons: ButtonMask {
                    left: true,
                    ..ButtonMask::default()
                },
            },
            UiEvent::MouseUp {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(3.0, 0.0),
            },
        ];
        let out = coalesce_mouse_moved(raw);
        assert_eq!(out.len(), 3, "expected [Down, Moved, Up], got {out:?}");
        assert!(
            matches!(&out[0], UiEvent::MouseDown { .. }),
            "first event must be MouseDown"
        );
        assert!(
            matches!(&out[1], UiEvent::MouseMoved { position, .. } if position.x == 3.0),
            "middle event must be the final MouseMoved (x=3), got {:?}",
            out[1]
        );
        assert!(
            matches!(&out[2], UiEvent::MouseUp { .. }),
            "last event must be MouseUp"
        );
    }

    /// `MouseMoved` events separated by a non-move event are NOT merged —
    /// each consecutive run collapses independently.
    #[test]
    fn coalesce_does_not_merge_moves_separated_by_other_events() {
        use crate::{ButtonMask, Modifiers};
        let raw = vec![
            UiEvent::MouseMoved {
                position: Point::new(1.0, 0.0),
                buttons: ButtonMask::default(),
            },
            UiEvent::MouseMoved {
                position: Point::new(2.0, 0.0),
                buttons: ButtonMask::default(),
            },
            UiEvent::KeyPressed {
                key: Key::Char('x'),
                modifiers: Modifiers::default(),
                repeat: false,
            },
            UiEvent::MouseMoved {
                position: Point::new(5.0, 0.0),
                buttons: ButtonMask::default(),
            },
            UiEvent::MouseMoved {
                position: Point::new(6.0, 0.0),
                buttons: ButtonMask::default(),
            },
        ];
        let out = coalesce_mouse_moved(raw);
        // Expected: [Moved(2.0), Key('x'), Moved(6.0)]
        assert_eq!(out.len(), 3, "expected 3 events, got {out:?}");
        assert!(
            matches!(&out[0], UiEvent::MouseMoved { position, .. } if position.x == 2.0),
            "first run must collapse to x=2, got {:?}",
            out[0]
        );
        assert!(
            matches!(&out[1], UiEvent::KeyPressed { .. }),
            "middle event must be KeyPressed"
        );
        assert!(
            matches!(&out[2], UiEvent::MouseMoved { position, .. } if position.x == 6.0),
            "second run must collapse to x=6, got {:?}",
            out[2]
        );
    }

    // ── recover_leaked_sgr_mouse_fragments tests (#293) ─────────────────────

    /// Builds a run of `KeyPressed(Char(_))` events, one per `char` in `s`
    /// — the shape crossterm's reader produces when it decodes a leaked
    /// escape-sequence tail byte-by-byte as ordinary text.
    fn char_run(s: &str) -> Vec<UiEvent> {
        s.chars()
            .map(|c| UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers: Modifiers::default(),
                repeat: false,
            })
            .collect()
    }

    /// A leaked pure-motion report (`Cb=35`, the exact shape #293 was
    /// filed against) reassembles into the `MouseMoved` it should have
    /// decoded as, with no leftover `KeyPressed` events.
    #[test]
    fn recovers_leaked_pure_motion_report() {
        use crate::ButtonMask;
        let raw = char_run("[<35;10;5M");
        let out = recover_leaked_sgr_mouse_fragments(raw);
        assert_eq!(
            out.len(),
            1,
            "expected exactly one recovered event: {out:?}"
        );
        assert!(
            matches!(
                &out[0],
                UiEvent::MouseMoved { position, buttons }
                    if position.x == 9.0 && position.y == 4.0 && *buttons == ButtonMask::default()
            ),
            "expected MouseMoved(9,4) with no buttons held, got {:?}",
            out[0]
        );
    }

    /// The leading `Escape` this race dispatches standalone is preserved
    /// verbatim, immediately followed by the recovered mouse event — the
    /// exact batch shape `wait_events`/`poll_events` hands to
    /// `coalesce_mouse_moved` once the leak is fixed.
    #[test]
    fn preserves_a_genuine_leading_escape_before_the_recovered_report() {
        let mut raw = vec![UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        }];
        raw.extend(char_run("[<35;10;5M"));
        let out = recover_leaked_sgr_mouse_fragments(raw);
        assert_eq!(out.len(), 2, "expected [Escape, MouseMoved]: {out:?}");
        assert!(matches!(
            &out[0],
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            }
        ));
        assert!(matches!(&out[1], UiEvent::MouseMoved { .. }));
    }

    /// A left-button press (`Cb=0`) and its SGR release (`Cb=0`, lowercase
    /// `m`) both reassemble correctly — release flips `Down` to `Up`,
    /// matching crossterm's own `parse_csi_sgr_mouse`.
    #[test]
    fn recovers_leaked_click_down_and_up() {
        use crate::MouseButton;
        let down = recover_leaked_sgr_mouse_fragments(char_run("[<0;3;4M"));
        assert!(
            matches!(
                &down[..],
                [UiEvent::MouseDown { button: MouseButton::Left, position, .. }]
                    if position.x == 2.0 && position.y == 3.0
            ),
            "expected a single MouseDown(2,3): {down:?}"
        );

        let up = recover_leaked_sgr_mouse_fragments(char_run("[<0;3;4m"));
        assert!(
            matches!(
                &up[..],
                [UiEvent::MouseUp { button: MouseButton::Left, position, .. }]
                    if position.x == 2.0 && position.y == 3.0
            ),
            "expected a single MouseUp(2,3): {up:?}"
        );
    }

    /// A drag report (`Cb=32`, left button held while moving) reassembles
    /// with the held button carried on `buttons`, mirroring
    /// `crossterm_mouse_to_uievent`'s `Drag` handling.
    #[test]
    fn recovers_leaked_drag_report() {
        let out = recover_leaked_sgr_mouse_fragments(char_run("[<32;7;8M"));
        assert!(
            matches!(
                &out[..],
                [UiEvent::MouseMoved { position, buttons }]
                    if position.x == 6.0 && position.y == 7.0 && buttons.left
            ),
            "expected a single held-left MouseMoved(6,7): {out:?}"
        );
    }

    /// A real user typing a literal `[<...` that never completes into a
    /// well-formed SGR triple (no `M`/`m` terminator, or a `;`-delimited
    /// group that isn't three integers) must not be touched — the whole
    /// run survives as ordinary `KeyPressed` events.
    #[test]
    fn leaves_non_matching_bracket_runs_untouched() {
        let raw = char_run("[<12;34");
        let out = recover_leaked_sgr_mouse_fragments(raw.clone());
        assert_eq!(
            out, raw,
            "an incomplete/unterminated run must pass through verbatim"
        );

        let raw = char_run("[hello");
        let out = recover_leaked_sgr_mouse_fragments(raw.clone());
        assert_eq!(
            out, raw,
            "'[' not followed by '<' must pass through verbatim"
        );
    }

    /// Events unrelated to the leak (plain keystrokes, real mouse events)
    /// pass through untouched, and multiple independent leaks in one
    /// batch each recover independently.
    #[test]
    fn passes_through_unrelated_events_and_recovers_multiple_leaks_in_one_batch() {
        let mut raw = vec![UiEvent::KeyPressed {
            key: Key::Char('h'),
            modifiers: Modifiers::default(),
            repeat: false,
        }];
        raw.extend(char_run("[<35;10;5M"));
        raw.push(UiEvent::KeyPressed {
            key: Key::Char('i'),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        raw.extend(char_run("[<35;11;6M"));

        let out = recover_leaked_sgr_mouse_fragments(raw);
        assert_eq!(
            out.len(),
            4,
            "expected [Char(h), Moved, Char(i), Moved]: {out:?}"
        );
        assert!(matches!(
            &out[0],
            UiEvent::KeyPressed {
                key: Key::Char('h'),
                ..
            }
        ));
        assert!(matches!(&out[1], UiEvent::MouseMoved { position, .. } if position.x == 9.0));
        assert!(matches!(
            &out[2],
            UiEvent::KeyPressed {
                key: Key::Char('i'),
                ..
            }
        ));
        assert!(matches!(&out[3], UiEvent::MouseMoved { position, .. } if position.x == 10.0));
    }

    // ── Issue #552 sibling audit: tab-bar hit geometry ──────────────
    //
    // `TabBarHits` is documented as **target-surface (absolute)**
    // coordinates — the opposite convention from `ActivityBarRowHit`,
    // which is bar-relative. `draw_tab_bar` honoured that on both TUI and
    // GTK by shifting the primitive's bar-relative output by the bar's
    // origin. `Backend::tab_bar_layout` honoured it on neither: it
    // returned `tab_bar_hits_from_layout` verbatim, so the no-paint path
    // was off by `rect.x` — nonzero for any tab bar sitting right of a
    // sidebar, which is every AppShell layout.
    //
    // Same class of defect as #552 itself (a hit-region seam silently
    // disagreeing about coordinate space), one primitive over, found by
    // the audit the issue asked for. Both backends now route through the
    // shared `backend::shift_tab_bar_hits`.

    fn audit_bar() -> TabBar {
        TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![
                crate::primitives::tab_bar::TabItem {
                    label: "main.rs".into(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
                crate::primitives::tab_bar::TabItem {
                    label: "lib.rs".into(),
                    is_active: false,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
            ],
            right_segments: vec![],
            active_accent: None,
            scroll_offset: 0,
            show_tab_close: true,
            compact: false,
        }
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout_returns_absolute_x_not_bar_relative() {
        let backend = TuiBackend::new();
        let bar = audit_bar();

        let at_origin = backend.tab_bar_layout(QRect::new(0.0, 0.0, 40.0, 1.0), &bar);
        let shifted = backend.tab_bar_layout(QRect::new(12.0, 0.0, 40.0, 1.0), &bar);

        assert!(
            !at_origin.slot_positions.is_empty(),
            "sanity: some tabs should be laid out"
        );
        for (i, (base, moved)) in at_origin
            .slot_positions
            .iter()
            .zip(shifted.slot_positions.iter())
            .enumerate()
        {
            if *base == (0.0, 0.0) {
                // Scrolled-out sentinel — must stay recognisable, not
                // become (12.0, 12.0).
                assert_eq!(
                    *moved,
                    (0.0, 0.0),
                    "tab {i}: the scrolled-out sentinel must not be shifted"
                );
                continue;
            }
            assert_eq!(
                (moved.0, moved.1),
                (base.0 + 12.0, base.1 + 12.0),
                "tab {i}: `tab_bar_layout` is documented to return \
                 target-surface coordinates, so moving the bar right by 12 \
                 must move its hit spans right by 12 (issue #552 audit)"
            );
        }
    }

    /// The no-paint path must agree with the painting path, since callers
    /// use them interchangeably to route the same clicks. Runs
    /// `draw_tab_bar` through a real frame scope so it goes down the
    /// production rasteriser, not a reimplementation of it.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout_agrees_with_draw_tab_bar_on_coordinate_space() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut backend = TuiBackend::new();
        backend.begin_frame(Viewport::new(80.0, 24.0, 1.0));
        let bar = audit_bar();
        // Nonzero x: the whole point. At x = 0 the two paths agreed even
        // before the fix, which is exactly how the defect stayed hidden.
        let rect = QRect::new(12.0, 0.0, 40.0, 1.0);

        let from_layout = backend.tab_bar_layout(rect, &bar);

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        let mut painted = None;
        terminal
            .draw(|frame| {
                painted =
                    Some(backend.enter_frame_scope(frame, |b| b.draw_tab_bar(rect, &bar, None)));
            })
            .expect("draw");
        let from_paint = painted.expect("draw closure ran");

        assert!(
            from_paint.slot_positions.iter().any(|s| *s != (0.0, 0.0)),
            "sanity: the rasteriser should have placed at least one tab"
        );
        assert_eq!(
            from_layout.slot_positions, from_paint.slot_positions,
            "`tab_bar_layout` and `draw_tab_bar` must report tab slots in \
             the same coordinate space (issue #552 audit)"
        );
        assert_eq!(
            from_layout.close_bounds, from_paint.close_bounds,
            "close-button spans must match between the paint and no-paint \
             paths too"
        );
    }

    /// `Backend::snap_height` must agree with `q_rect_to_ratatui`'s height
    /// rounding for every height a rect can carry, since it exists to let
    /// consumers predict that rounding without reimplementing it
    /// (quadraui#632). If the two ever drift, a consumer's pre-snapped
    /// layout height stops matching what the rasteriser actually paints —
    /// the exact one-row misalignment that bit coord-tui in #464 and #995.
    #[test]
    fn snap_height_matches_q_rect_to_ratatui_height_rounding() {
        let backend = TuiBackend::new();
        for h in [
            0.0_f32, 0.2, 0.49, 0.5, 0.51, 1.0, 1.4, 1.6, 2.5, 13.999, -1.0, -0.4,
        ] {
            let snapped = backend.snap_height(h);
            let via_rect = q_rect_to_ratatui(QRect::new(0.0, 0.0, 0.0, h)).height;
            assert_eq!(
                snapped as u16, via_rect,
                "snap_height({h}) = {snapped}, but q_rect_to_ratatui reports \
                 height {via_rect} for the same input"
            );
        }
    }

    /// #776: TUI has no windowing scrollbar overlay to dodge, so it
    /// relies on `Backend::scrollbar_reserve`'s trait default (0.0)
    /// rather than overriding it — unlike GTK (see
    /// `GtkBackend::scrollbar_reserve`, 8.0).
    #[test]
    fn tui_backend_scrollbar_reserve_uses_the_zero_default() {
        let backend = TuiBackend::new();
        assert_eq!(Backend::scrollbar_reserve(&backend), 0.0);
    }

    /// A pixel backend must not quantize — `snap_height` returns the input
    /// unchanged via the trait's default impl. `MockBackend` doesn't
    /// override `snap_height`, so it stands in for "any backend that only
    /// paints pixels/DIPs" here (quadraui#632).
    #[test]
    fn snap_height_default_impl_is_identity() {
        let backend = MockBackend::new();
        for h in [0.0_f32, 0.4, 1.6, 13.999, -1.0] {
            assert_eq!(
                Backend::snap_height(&backend, h),
                h,
                "default snap_height impl must be identity for pixel backends"
            );
        }
    }

    /// quadraui#666: TUI has no native alert facility — `backend_caps`
    /// must not claim `native_dialogs`, and `services().show_message_dialog`
    /// must actually return `None` regardless of what's asked for. Both
    /// halves of the same honesty contract `file_dialogs` already has.
    #[test]
    fn tui_backend_declares_no_native_dialogs() {
        let backend = TuiBackend::new();
        assert!(
            !backend.backend_caps().native_dialogs,
            "TUI has no native alert facility; native_dialogs must stay false"
        );
        let opts = MessageDialogOptions {
            title: "Unsaved Changes".to_string(),
            body: "Do you want to save?".to_string(),
            buttons: Vec::new(),
            severity: None,
        };
        assert!(backend.services().show_message_dialog(opts).is_none());
    }

    /// Issue #506: `list_layout` is documented **LOCAL** — a caller
    /// subtracts `rect.x`/`rect.y` itself before hit-testing — so a
    /// non-zero origin must not change the returned geometry at all
    /// (same regression shape as `gtk_backend_data_table_layout_ignores_rect_origin`).
    #[test]
    fn list_layout_ignores_rect_origin() {
        let backend = TuiBackend::new();
        let list = sample_list();

        let at_origin = backend.list_layout(QRect::new(0.0, 0.0, 20.0, 5.0), &list);
        let shifted = backend.list_layout(QRect::new(7.0, 3.0, 20.0, 5.0), &list);

        assert_eq!(
            at_origin, shifted,
            "list_layout must ignore rect.x/rect.y (LOCAL frame, issue #505)"
        );
        assert!(
            !at_origin.visible_items.is_empty(),
            "sanity: the sample list has an item to lay out"
        );
    }

    /// `list_layout` must be the exact resolver `draw_list` uses
    /// internally, not a parallel reimplementation that can drift from
    /// it (`PRIMITIVE_RULES.md` rule 5) — paint into a real `Buffer` (the
    /// same free-fn path `crate::tui::draw_list` uses) and compare the
    /// helper both paths share.
    #[test]
    fn list_layout_matches_draw_list_resolver() {
        let backend = TuiBackend::new();
        let list = sample_list();
        let rect = QRect::new(3.0, 1.0, 20.0, 5.0);
        let area = q_rect_to_ratatui(rect);

        let no_paint = backend.list_layout(rect, &list);

        // `draw_list` returns `()`, but it computes its geometry by
        // calling `tui_list_layout(area, list)` internally (see
        // `tui/list.rs::draw_list`) — the same helper `list_layout`
        // calls. Paint into a real buffer (proving it doesn't panic on
        // the shared geometry) and assert the helper output directly.
        let mut buf = ratatui::buffer::Buffer::empty(area);
        crate::tui::draw_list(&mut buf, area, &list, &backend.current_theme, false);
        let via_paint_helper = crate::tui::tui_list_layout(area, &list);

        assert_eq!(
            no_paint, via_paint_helper,
            "list_layout must equal the exact helper draw_list uses internally"
        );
        assert!(
            !no_paint.visible_items.is_empty(),
            "sanity: the sample list has an item to lay out"
        );
    }

    /// Issue #506: `board_layout` is documented **ABSOLUTE** — bounds
    /// are shifted by `rect.x`/`rect.y`, matching `BoardLayout::hit_test`'s
    /// contract that a caller compares it directly against raw click
    /// coordinates. Also proves `board_layout` and `draw_board` agree
    /// byte-for-byte, since both route through `tui_board_layout`.
    #[test]
    fn board_layout_is_absolute_and_matches_draw_board() {
        let backend = TuiBackend::new();
        let model = crate::primitives::board::BoardModel {
            id: WidgetId::new("test:board"),
            columns: vec![crate::primitives::board::BoardColumn {
                id: WidgetId::new("col:a"),
                title: "Backlog".into(),
                cards: vec![crate::primitives::board::BoardCard {
                    id: WidgetId::new("card:1"),
                    title: "Do the thing".into(),
                    labels: vec![],
                    badges: vec![],
                    hint: None,
                }],
                scroll_offset: 0,
            }],
            selected_card_id: None,
            col_scroll_offset: 0,
        };
        let rect = QRect::new(5.0, 2.0, 40.0, 12.0);
        let area = q_rect_to_ratatui(rect);

        let no_paint = backend.board_layout(rect, &model);
        assert_eq!(
            no_paint.bounds,
            crate::event::Rect::new(rect.x, rect.y, rect.width, rect.height),
            "board_layout must fold rect's origin into `bounds` (ABSOLUTE, issue #505)"
        );

        let mut buf = ratatui::buffer::Buffer::empty(area);
        let painted = crate::tui::draw_board(&mut buf, area, &model, &backend.current_theme);
        assert_eq!(
            no_paint, painted,
            "board_layout must equal the exact layout draw_board painted with"
        );
    }

    /// Issue #506 review fix: `terminal_layout`'s default body must
    /// reserve the same scrollbar gutter width `draw_terminal` reserves
    /// before iterating cells (`cell_area_w =
    /// area.width.saturating_sub(sb_cols)`, `src/tui/terminal.rs`), or a
    /// click on the gutter resolves to `TerminalHit::Cell` when the
    /// paint path shows a scrollbar there, not a cell — "paint and
    /// no-paint silently disagree" (rule 5).
    #[test]
    fn terminal_layout_reserves_scrollbar_gutter_matching_draw_terminal() {
        let backend = TuiBackend::new();
        let cell = crate::TerminalCell {
            text: "x".to_string(),
            fg: crate::Color::rgb(200, 200, 200),
            bg: crate::Color::rgb(20, 20, 20),
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
        };
        let term = TerminalPrim {
            id: WidgetId::new("t"),
            cells: vec![vec![cell; 20]; 5],
            scrollbar: Some(crate::TerminalScrollbar {
                total_lines: 100,
                visible_lines: 5,
                scroll_offset: 0,
                inverted: false,
                // `None` — draw_terminal's `sb_cols: … .unwrap_or(1)` fallback,
                // which `terminal_scrollbar_default_width` must reproduce.
                width: None,
            }),
        };
        let rect = QRect::new(0.0, 0.0, 10.0, 5.0);
        let area = q_rect_to_ratatui(rect);

        let layout = backend.terminal_layout(rect, &term);
        assert_eq!(
            layout.grid_cols, 9,
            "terminal_layout must reserve the 1-column scrollbar gutter (TUI's \
             terminal_scrollbar_default_width), not the full unreduced rect.width"
        );
        assert_eq!(
            layout.hit_test(9.0, 0.0),
            crate::primitives::terminal::TerminalHit::Empty,
            "a click at x=9 (the scrollbar gutter column) must not resolve to a grid cell"
        );

        // Confirm against the real paint path: column 9 is where
        // draw_terminal paints the scrollbar track, not cell glyphs.
        let mut buf = ratatui::buffer::Buffer::empty(area);
        crate::tui::draw_terminal(&mut buf, area, &term, &backend.current_theme);
        assert_ne!(
            buf[(area.x + 9, area.y)].symbol(),
            "x",
            "column 9 is draw_terminal's scrollbar gutter — it must not show painted cell \
             content, matching terminal_layout's grid_cols=9 exclusion of that column"
        );
    }

    /// Minimal two-row-hunk `DiffView` fixture for `diff_view_layout`
    /// parity tests below.
    fn sample_diff_view(mode: crate::DiffMode, left_label: Option<String>) -> crate::DiffView {
        crate::DiffView {
            id: WidgetId::new("diff"),
            left: String::new(),
            right: String::new(),
            left_label,
            right_label: None,
            hunks: vec![crate::DiffHunk {
                left_start: 1,
                right_start: 1,
                rows: vec![
                    crate::DiffRow {
                        left: Some("alpha".into()),
                        right: Some("ALPHA".into()),
                        kind: crate::DiffRowKind::Changed,
                    },
                    crate::DiffRow {
                        left: Some("beta".into()),
                        right: Some("beta".into()),
                        kind: crate::DiffRowKind::Same,
                    },
                ],
            }],
            mode,
            editability: crate::DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: crate::DiffPane::Left,
            has_focus: false,
        }
    }

    /// Issue #506 review fix: `diff_view_layout` is claimed to be
    /// "verified byte-for-byte against each backend's real paint
    /// formula" — this is the test that makes that claim true rather
    /// than aspirational. `draw_diff_view` returns its `DiffViewLayout`
    /// directly, so this compares the no-paint default body against the
    /// exact value the real paint path produced, in `SideBySide` mode
    /// with a header row reserved.
    #[test]
    fn diff_view_layout_matches_draw_diff_view_side_by_side_with_header() {
        let backend = TuiBackend::new();
        let view = sample_diff_view(crate::DiffMode::SideBySide, Some("left.txt".into()));
        let rect = QRect::new(0.0, 0.0, 40.0, 5.0);
        let area = q_rect_to_ratatui(rect);

        let no_paint = backend.diff_view_layout(rect, &view);

        let mut buf = ratatui::buffer::Buffer::empty(area);
        let painted = crate::tui::draw_diff_view(&mut buf, area, &view, &backend.current_theme);

        assert_eq!(
            no_paint, painted,
            "diff_view_layout must equal the exact layout draw_diff_view painted with"
        );
        assert_eq!(
            no_paint.visible_rows, 4,
            "a 5-row rect minus a 1-row header (left_label is set) leaves 4 content rows"
        );
    }

    /// Same parity check in `Unified` mode, where no header row is
    /// reserved and `total_rows` folds in one synthesized `@@` header
    /// line per hunk.
    #[test]
    fn diff_view_layout_matches_draw_diff_view_unified() {
        let backend = TuiBackend::new();
        let view = sample_diff_view(crate::DiffMode::Unified, None);
        let rect = QRect::new(0.0, 0.0, 40.0, 5.0);
        let area = q_rect_to_ratatui(rect);

        let no_paint = backend.diff_view_layout(rect, &view);

        let mut buf = ratatui::buffer::Buffer::empty(area);
        let painted = crate::tui::draw_diff_view(&mut buf, area, &view, &backend.current_theme);

        assert_eq!(
            no_paint, painted,
            "diff_view_layout must equal the exact layout draw_diff_view painted with (unified \
             mode reserves no header band)"
        );
    }

    /// Minimal blank `EditorLine` for `editor_layout` parity tests below
    /// — content doesn't matter, only that `lines.len()` matches the
    /// viewport row count so `draw_editor` doesn't early-`break`.
    fn blank_editor_line(idx: usize) -> crate::EditorLine {
        crate::EditorLine {
            raw_text: String::new(),
            gutter_text: String::new(),
            spans: Vec::new(),
            line_idx: idx,
            is_current_line: false,
            is_fold_header: false,
            folded_line_count: 0,
            git_diff: None,
            diff_status: None,
            diagnostics: Vec::new(),
            spell_errors: Vec::new(),
            is_breakpoint: false,
            is_conditional_bp: false,
            is_dap_current: false,
            is_wrap_continuation: false,
            segment_col_offset: 0,
            annotation: None,
            ghost_suffix: None,
            is_ghost_continuation: false,
            indent_guides: Vec::new(),
            colorcolumns: Vec::new(),
        }
    }

    /// Issue #506 review fix: `editor_layout` is claimed to be
    /// "verified byte-for-byte against each backend's real paint
    /// formula" — this test makes that claim true for TUI. TUI's
    /// `draw_editor` re-derives its own scrollbar-presence formula by
    /// hand rather than calling `Editor::layout` (`src/tui/editor.rs`:
    /// `has_scrollbar = total_lines > viewport_lines && area.width >
    /// gutter_w + 1`, v-scrollbar track at `(area.x + area.width - 1,
    /// area.y, 1, track_h)`); this test pins `editor_layout`'s
    /// `v_scrollbar_bounds` against that exact independently-derived
    /// track, so the two can't silently drift apart.
    #[test]
    fn editor_layout_matches_draw_editor_scrollbar_track() {
        let backend = TuiBackend::new();
        let rect = QRect::new(2.0, 1.0, 20.0, 5.0);
        let editor = crate::Editor {
            id: WidgetId::new("ed"),
            rect,
            lines: (0..5).map(blank_editor_line).collect(),
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            // > viewport_lines (5) so a v-scrollbar is present.
            total_lines: 100,
            // Small enough that no h-scrollbar is triggered, keeping
            // TUI's two-pass visible_lines/text_h resolution collapsed
            // to a single pass (see `Editor::layout`'s doc).
            max_col: 4,
            gutter_char_width: 0,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: HashMap::new(),
            code_action_lines: std::collections::HashSet::new(),
            bracket_match_positions: Vec::new(),
            active_indent_col: None,
            tabstop: 4,
            cursorline: false,
            lightbulb_glyph: '!',
        };

        let layout = backend.editor_layout(rect, &editor);
        let vsb = layout
            .v_scrollbar_bounds
            .expect("100 lines in a 5-row viewport must trigger a v-scrollbar");
        assert_eq!(
            vsb,
            crate::event::Rect::new(rect.x + rect.width - 1.0, rect.y, 1.0, rect.height),
            "editor_layout's v_scrollbar_bounds must match draw_editor's own track formula \
             `(area.x + area.width - 1, area.y, 1, track_h)` (src/tui/editor.rs)"
        );

        // Paint via the exact free fn TuiBackend::draw_editor calls —
        // proves the shared scrollbar-presence formula doesn't panic
        // against this geometry, and that the hit-test the layout
        // exposes resolves at the painted track's own origin.
        let area = q_rect_to_ratatui(rect);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        crate::tui::draw_editor(&mut buf, area, &editor, &backend.current_theme);

        assert_eq!(
            layout.hit_test(vsb.x, vsb.y),
            crate::EditorHit::VScrollbar,
            "a click at the painted scrollbar track's origin must resolve via editor_layout's \
             own hit_test as VScrollbar"
        );
    }

    // ── mouse_enabled / `no-mouse` mode (quadraui#828) ──────────────────

    /// `TuiBackend::new()` defaults to mouse reporting enabled — matches
    /// [`super::run::RunConfig::default`] (`mouse: true`), so a backend
    /// nobody has called `set_mouse_enabled` on behaves exactly as it did
    /// before `no-mouse` mode existed. `backend_caps().mouse` stays `true`
    /// regardless (it is the static per-backend-*type* fact, not this
    /// session's config — see `backend_caps`'s doc).
    #[test]
    fn mouse_enabled_defaults_to_true() {
        let backend = TuiBackend::new();
        assert!(backend.mouse_enabled());
        assert!(backend.backend_caps().mouse);
    }

    /// `set_mouse_enabled(false)` — what `run_with(app, RunConfig { mouse:
    /// false, .. })` calls before entering the frame loop — flips the
    /// session-level accessor without touching `backend_caps()`, which
    /// stays a static per-backend-type declaration (see
    /// `mouse_enabled` field's doc for why `tests/conformance/caps.rs`'s
    /// source-parsing check requires that split).
    #[test]
    fn set_mouse_enabled_false_is_a_session_level_toggle_only() {
        let mut backend = TuiBackend::new();
        backend.set_mouse_enabled(false);
        assert!(!backend.mouse_enabled());
        assert!(
            backend.backend_caps().mouse,
            "backend_caps().mouse is a static per-backend-type fact, unaffected by \
             set_mouse_enabled"
        );
    }

    /// Round-trip: re-enabling restores the original state —
    /// `set_mouse_enabled` is a plain toggle, not a one-way ratchet.
    #[test]
    fn set_mouse_enabled_true_restores_state() {
        let mut backend = TuiBackend::new();
        backend.set_mouse_enabled(false);
        backend.set_mouse_enabled(true);
        assert!(backend.mouse_enabled());
    }

    // ── request_frame_in (quadraui#832) ─────────────────────────────────

    /// With nothing requested, the poll timeout is the full ceiling —
    /// this is the "idle app doesn't poll tightly" behavior the issue's
    /// acceptance criterion asks for, made concrete at the `TuiBackend`
    /// level (`tui::run::run_inner` is what actually calls
    /// `wait_events` with this value).
    #[test]
    fn no_request_frame_in_call_uses_the_full_ceiling() {
        let backend = TuiBackend::new();
        assert_eq!(
            backend.frame_poll_timeout(Duration::from_millis(250)),
            Duration::from_millis(250)
        );
    }

    /// `request_frame_in` shortens the next poll timeout to (at most) the
    /// requested delay.
    #[test]
    fn request_frame_in_shortens_the_poll_timeout() {
        let backend = TuiBackend::new();
        Backend::request_frame_in(&backend, Duration::from_millis(10));
        let timeout = backend.frame_poll_timeout(Duration::from_millis(250));
        assert!(
            timeout <= Duration::from_millis(10),
            "expected a short timeout, got {timeout:?}"
        );
    }

    /// `clear_frame_deadline_if_due` is a no-op before the deadline has
    /// elapsed — the run loop must not busy-spin at a near-zero timeout
    /// immediately after arming a longer-lived request.
    #[test]
    fn clear_frame_deadline_if_due_is_a_noop_before_the_deadline() {
        let mut backend = TuiBackend::new();
        Backend::request_frame_in(&backend, Duration::from_secs(60));
        backend.clear_frame_deadline_if_due();
        let timeout = backend.frame_poll_timeout(Duration::from_millis(1));
        assert!(
            timeout <= Duration::from_millis(1),
            "the still-pending far-future deadline must keep governing the \
             ceiling-clamped timeout, got {timeout:?}"
        );
    }

    /// Once cleared, a fresh ceiling-bound timeout returns — proves
    /// `clear_frame_deadline_if_due` actually clears rather than just
    /// reading the deadline.
    #[test]
    fn clear_frame_deadline_if_due_clears_an_elapsed_deadline() {
        let mut backend = TuiBackend::new();
        Backend::request_frame_in(&backend, Duration::ZERO);
        std::thread::sleep(Duration::from_millis(1));
        backend.clear_frame_deadline_if_due();
        assert_eq!(
            backend.frame_poll_timeout(Duration::from_millis(250)),
            Duration::from_millis(250)
        );
    }
}
