//! The binding **host** layer: everything a native binding (`quadraui-py`
//! today, a napi leaf next, #1126) needs around [`crate::engine::InteractionEngine`]
//! to drive a [`quadraui::AppLogic`], but that still carries zero
//! host-language FFI type. A binding's own `AppLogic` impl holds one
//! [`BindingHost`] (the six fixed primitive slots, any number of additive
//! slots, and the backend-settings replay store) plus its own callback
//! fields (a Python callable, a JS function value, ...) and an
//! [`crate::engine::InteractionEngine`]; [`render_overlays`] is the one
//! render-time call every binding's `AppLogic::render` makes after painting
//! its own slots, so the anchored-overlay z-order — context menu, float,
//! rich-text popup, completions, dialog, tooltip, toast topmost of all, in
//! the exact order [`render_overlays`]'s own body paints them — is written
//! down once instead of copy-pasted per binding.
//!
//! [`NativeExitPolicy`] wraps a binding's own `AppLogic` for the duration of
//! a `run_native`-style call into a real `GtkRunner`/`MacRunner`/`WinRunner`:
//! it forces a handler-less `WindowClose` to actually exit, and — on the
//! `macos` feature without `gtk` also enabled — downgrades any *other*
//! `Reaction::Exit` to `Continue` plus a sticky `exit_requested` flag, so
//! `[NSApp terminate:]`
//! never fires from underneath a binding's own `run_native` call (that
//! downgrade would otherwise end the whole process before `run_native`
//! could return control — and any caught exception with it — to its caller).
//! A napi leaf under `MacRunner` hits the identical hazard, which is why
//! this lives here rather than back in `quadraui-py`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;

use quadraui::compose::{
    classify_tree_nav_key, Edge, HoverTooltipTracker, TreeController, TreeControllerEvent,
    TreeNavKey,
};
use quadraui::tui::testing::TuiDriver;
use quadraui::Rect;
use quadraui::{
    mouse_down, mouse_up, Accelerator, AcceleratorId, AppLogic, Backend, Button, DataTable,
    Decoration, Key, ListView, Modifiers, MouseButton, Reaction, Style, StyledSpan, StyledText,
    TabBar, TextDisplay, TextDisplayLine, Theme, Tooltip, TreePath, TreeRow, TreeView, UiEvent,
    WidgetId,
};

use crate::engine::InteractionEngine;
use crate::extra_paint::ExtraPaint;
use crate::hit::PaintedHit;
use crate::render::RenderRefs;
use crate::widget_state::WidgetState;

/// Slot id of the host's `Button` descriptor.
pub const SLOT_BUTTON: i64 = 0;
/// Slot id of the host's `TextDisplay` descriptor.
pub const SLOT_TEXT_DISPLAY: i64 = 1;
/// Slot id of the host's `ListView` descriptor.
pub const SLOT_LIST: i64 = 2;
/// Slot id of the host's `TreeView` descriptor.
pub const SLOT_TREE: i64 = 3;
/// Slot id of the host's `DataTable` descriptor.
pub const SLOT_DATA_TABLE: i64 = 4;
/// Slot id of the host's `TabBar` descriptor.
pub const SLOT_TAB_BAR: i64 = 5;

/// The host-side half of a binding's `AppLogic`: a bag of optional
/// descriptors, one per core primitive, plus any number of additive
/// primitives keyed by kind name, plus the backend-settings replay store a
/// `run_tui`/`run_native`-style call needs to carry onto a freshly
/// constructed backend. A binding's own `AppLogic` impl holds one of these
/// alongside its own callback fields and [`InteractionEngine`] — nothing
/// here is host-language-specific.
///
/// Adding a further additive primitive kind to [`Self::extra_slots`] never
/// touches the six fixed fields or their [`Self::slot_occupied`]/
/// [`Self::is_known_slot`]/[`Self::clear_slot`] machinery.
#[derive(Default)]
pub struct BindingHost {
    pub button: Option<Button>,
    pub text_display: Option<TextDisplay>,
    pub list: Option<ListView>,
    pub tree: Option<TreeView>,
    pub data_table: Option<DataTable>,
    pub tab_bar: Option<TabBar>,
    /// Additive primitive slots, keyed by kind name (`"progress"`,
    /// `"spinner"`, `"image"`, ...). Painted after the six fixed bands, in
    /// a binding's own additive-primitive registry order.
    pub extra_slots: HashMap<String, Rc<dyn ExtraPaint>>,
    /// Set via a binding's own `register_accelerator`/`unregister_accelerator`
    /// surface. Kept here (not just applied directly to the live backend)
    /// so this whole host — moved by value into a real
    /// `TuiRunner`/`GtkRunner`/`MacRunner`/`WinRunner` once a binding's own
    /// `run_tui`/`run_native` is called — carries its accelerators with it;
    /// [`Self::setup`] replays every entry onto whichever backend that
    /// runner just constructed. Keyed by [`AcceleratorId`] so re-registering
    /// the same id replaces its previous binding/scope/label wholesale.
    pub accelerators: HashMap<AcceleratorId, Accelerator>,
    /// Set via a binding's own `set_theme` surface, replayed onto a new
    /// backend by [`Self::setup`] for the same reason [`Self::accelerators`]
    /// is — a `run_tui`/`run_native` call moves this whole host by value
    /// into a brand-new backend, which starts back at `Theme::default()`
    /// unless told otherwise.
    pub theme: Option<Theme>,
    /// Set via a binding's own `set_style` surface; replayed the same way
    /// as [`Self::theme`].
    pub style: Option<Style>,
    /// Set via a binding's own `set_nerd_fonts` surface; replayed the same
    /// way as [`Self::theme`].
    pub nerd_fonts: Option<bool>,
    /// Set via a binding's own `set_editor_font` surface as
    /// `(family, size_pt)`; replayed the same way as [`Self::theme`].
    pub editor_font: Option<(String, f32)>,
    /// Set via a binding's own `set_ui_font` surface; replayed the same way
    /// as [`Self::theme`].
    pub ui_font: Option<String>,
    /// Set via a binding's own `set_nerd_font_fallback` surface; replayed
    /// the same way as [`Self::theme`].
    pub nerd_font_fallback: Option<String>,
    /// Raw font bytes handed to a binding's own
    /// `register_font_from_memory` surface, kept here so each one can be
    /// re-registered onto a new backend in registration order, the same
    /// replay reasoning as [`Self::accelerators`].
    pub registered_fonts: Vec<Vec<u8>>,
    /// Delay + auto-dismiss state machine for whichever hovered widget's
    /// tooltip text [`Self::hover_tooltip_text`] resolves, driven by
    /// [`Self::drive_hover_tooltip`] — the "host decides when to show it"
    /// half of `Button::tooltip`'s own contract, shared by every binding
    /// instead of each reimplementing [`HoverTooltipTracker`] wiring like
    /// a native backend's own hover-driven test does directly against
    /// `Backend`.
    pub tooltip_tracker: HoverTooltipTracker<WidgetId>,
    /// Tree-address id -> tooltip text for every layout-tree `Button` leaf
    /// a binding's own `render` painted through `crate::render::paint_layout`
    /// this frame, via `crate::render::collect_leaf_tooltips`. Replaced
    /// wholesale by [`Self::set_leaf_tooltips`] each render; consulted by
    /// [`Self::hover_tooltip_text`] as the layout-tree counterpart of
    /// [`Self::button`]'s direct slot read. A binding whose render never
    /// paints a layout tree at all (or paints only via [`Self::render_refs`]'s
    /// six fixed slots) leaves this empty forever, so
    /// [`Self::hover_tooltip_text`] falls through to [`Self::button`]
    /// exactly as before this field existed. A [`RefCell`] (not a plain
    /// field) because `AppLogic::render` takes `&self`, not `&mut self` --
    /// the same reason [`Self::tooltip_clock`]/[`Self::test_clock_ms`]
    /// below are `Cell`s rather than plain fields.
    pub leaf_tooltips: RefCell<HashMap<WidgetId, String>>,
    /// Wall-clock epoch [`Self::now_ms`] measures from, lazily set on its
    /// first call rather than at construction — a host is typically built
    /// well before its first frame (a binding constructs its `App` before
    /// `run_tui`/`run_native`/`step` ever runs it), so starting the clock
    /// at construction would count that dead time against the hover
    /// delay. `None` means "not started yet". `pub` like every other
    /// field here only because `BindingHost { .., ..BindingHost::default() }`
    /// struct-update literals outside this crate require every field
    /// visible — a binding should still read this only via [`Self::now_ms`].
    pub tooltip_clock: Cell<Option<Instant>>,
    /// Test-oriented override for [`Self::now_ms`]: when set, `now_ms()`
    /// returns this value directly instead of reading [`Self::tooltip_clock`]'s
    /// real wall clock. `None` (the default) means "use the real clock" —
    /// setting this never changes behaviour for a binding that never calls
    /// [`Self::set_test_clock_ms`]. A binding exposes a thin wrapper around
    /// [`Self::set_test_clock_ms`] on its own app object (e.g. Python's
    /// `QuiApp.set_test_clock_ms`, Node's `QuiApp.setTestClockMs`) so a test
    /// can drive [`Self::drive_hover_tooltip`]'s delay/auto-dismiss deadlines
    /// deterministically — advancing past `tooltip_delay_ms`/
    /// `tooltip_auto_dismiss_ms` by setting an absolute millisecond value,
    /// the same headless-clock contract [`HoverTooltipTracker`]'s own module
    /// doc and this crate's `#[cfg(test)]` module already use directly —
    /// rather than sleeping past them in real wall-clock time.
    pub test_clock_ms: Cell<Option<u64>>,
}

impl BindingHost {
    /// Whether `slot` currently holds a descriptor.
    pub fn slot_occupied(&self, slot: i64) -> bool {
        match slot {
            SLOT_BUTTON => self.button.is_some(),
            SLOT_TEXT_DISPLAY => self.text_display.is_some(),
            SLOT_LIST => self.list.is_some(),
            SLOT_TREE => self.tree.is_some(),
            SLOT_DATA_TABLE => self.data_table.is_some(),
            SLOT_TAB_BAR => self.tab_bar.is_some(),
            _ => false,
        }
    }

    /// Whether `slot` names one of the six fixed primitive slots.
    pub fn is_known_slot(slot: i64) -> bool {
        matches!(
            slot,
            SLOT_BUTTON
                | SLOT_TEXT_DISPLAY
                | SLOT_LIST
                | SLOT_TREE
                | SLOT_DATA_TABLE
                | SLOT_TAB_BAR
        )
    }

    /// Empties one slot, returning whether it held anything.
    pub fn clear_slot(&mut self, slot: i64) -> bool {
        match slot {
            SLOT_BUTTON => self.button.take().is_some(),
            SLOT_TEXT_DISPLAY => self.text_display.take().is_some(),
            SLOT_LIST => self.list.take().is_some(),
            SLOT_TREE => self.tree.take().is_some(),
            SLOT_DATA_TABLE => self.data_table.take().is_some(),
            SLOT_TAB_BAR => self.tab_bar.take().is_some(),
            _ => false,
        }
    }

    /// Whether `kind` names a currently-occupied additive primitive slot.
    pub fn extra_occupied(&self, kind: &str) -> bool {
        self.extra_slots.contains_key(kind)
    }

    /// Empties one additive primitive slot, returning whether it held
    /// anything.
    pub fn clear_extra_slot(&mut self, kind: &str) -> bool {
        self.extra_slots.remove(kind).is_some()
    }

    /// Replays every accelerator and backend-settings field this host has
    /// recorded onto `backend` — call from a binding's own
    /// [`AppLogic::setup`] so a chord/theme/style/font set before
    /// `run_tui`/`run_native` moved this host into a freshly constructed
    /// `TuiBackend`/`GtkBackend`/`MacBackend`/`WinBackend` still applies to
    /// that live backend, not just whichever one was dormant and headless
    /// before. Every real runner calls `app.setup(&mut backend)` right
    /// after building its backend and before the first frame, so this hook
    /// reaches all four.
    pub fn setup(&self, backend: &mut dyn Backend) {
        for acc in self.accelerators.values() {
            backend.register_accelerator(acc);
        }
        if let Some(theme) = self.theme {
            backend.set_theme(theme);
        }
        if let Some(style) = self.style {
            backend.set_style(style);
        }
        if let Some(enabled) = self.nerd_fonts {
            backend.set_nerd_fonts(enabled);
        }
        for bytes in &self.registered_fonts {
            backend.register_font_from_memory(bytes);
        }
        if let Some((family, size_pt)) = &self.editor_font {
            backend.set_editor_font(family, *size_pt);
        }
        if let Some(font_desc) = &self.ui_font {
            backend.set_ui_font(font_desc);
        }
        if let Some(family) = &self.nerd_font_fallback {
            backend.set_nerd_font_fallback(family);
        }
    }

    /// Borrows this host's six fixed slots (plus [`Self::extra_slots`]) as
    /// the [`RenderRefs`] shape [`crate::render::draw_bands`] paints from.
    pub fn render_refs(&self) -> RenderRefs<'_> {
        RenderRefs {
            button: self.button.as_ref(),
            text_display: self.text_display.as_ref(),
            list: self.list.as_ref(),
            tree: self.tree.as_ref(),
            data_table: self.data_table.as_ref(),
            tab_bar: self.tab_bar.as_ref(),
            extra: &self.extra_slots,
            // `BindingHost`'s slots are set directly from Rust fields, not
            // decoded from a raw dict, so there is no `scroll_px` key to
            // lift out here — a host that wants pixel-precise scrolling
            // drives `Backend::draw_*_with_state` itself instead of this
            // helper.
            list_scroll_px: None,
            tree_scroll_px: None,
            data_table_scroll_px: None,
        }
    }

    /// Milliseconds elapsed since this host's own first call to this
    /// method — a monotonic "headless clock" for [`Self::drive_hover_tooltip`]
    /// to feed [`HoverTooltipTracker`] with, since there is no
    /// `Backend`-level clock method to read one from instead (see
    /// [`Self::tooltip_clock`]'s own doc for why the epoch starts here
    /// rather than at construction). Returns [`Self::test_clock_ms`]
    /// directly instead, once a binding has set one via
    /// [`Self::set_test_clock_ms`] — the real wall clock above is never
    /// consulted again after that, so a test can drive deterministic
    /// delay/auto-dismiss deadlines with no real sleep at all. A caller
    /// that wants a fully synthetic, no-real-time-elapses clock without
    /// going through [`Self::test_clock_ms`] at all — the way this crate's
    /// own tests and `quadraui::compose::HoverTooltipTracker`'s module doc
    /// both demonstrate — can still call [`Self::drive_hover_tooltip`]
    /// directly with its own hand-picked `now_ms` rather than this
    /// method's value.
    pub fn now_ms(&self) -> u64 {
        if let Some(ms) = self.test_clock_ms.get() {
            return ms;
        }
        let epoch = self.tooltip_clock.get().unwrap_or_else(|| {
            let started = Instant::now();
            self.tooltip_clock.set(Some(started));
            started
        });
        epoch.elapsed().as_millis() as u64
    }

    /// Sets or clears [`Self::test_clock_ms`], the override
    /// [`Self::now_ms`] returns once set. `Some(ms)` pins `now_ms()` to
    /// `ms` until the next call to this method; `None` reverts to the real
    /// wall clock (as if this had never been called). A binding's own app
    /// object exposes a thin wrapper around this — e.g. Python's
    /// `QuiApp.set_test_clock_ms`, Node's `QuiApp.setTestClockMs` — so a
    /// test can advance the clock deterministically (set an increasing
    /// absolute value) past `tooltip_delay_ms`/`tooltip_auto_dismiss_ms`
    /// without a real sleep.
    pub fn set_test_clock_ms(&self, ms: Option<u64>) {
        self.test_clock_ms.set(ms);
    }

    /// The hover tooltip text for `id`: first [`Self::button`]'s own
    /// `Button::tooltip` (matched by that slot's own `Button::id`), then
    /// [`Self::leaf_tooltips`] (matched by tree-address id) -- the
    /// layout-tree leaf a render callback painted through
    /// `crate::render::paint_layout`. [`Self::drive_hover_tooltip`] uses
    /// this to decide whether a hovered widget has anything to show at
    /// all before arming [`Self::tooltip_tracker`] for it; a widget with
    /// no tooltip text in either source never starts the delay.
    pub fn hover_tooltip_text(&self, id: WidgetId) -> Option<String> {
        self.button
            .as_ref()
            .filter(|button| button.id == id)
            .and_then(|button| button.tooltip.clone())
            .or_else(|| self.leaf_tooltips.borrow().get(&id).cloned())
    }

    /// Replaces [`Self::leaf_tooltips`] wholesale with `tooltips` --
    /// typically `crate::render::collect_leaf_tooltips`'s own return
    /// value, walked fresh from the layout tree a render callback just
    /// returned. A binding calls this once per render, right after
    /// painting via `crate::render::paint_layout`, so
    /// [`Self::hover_tooltip_text`] resolves against this frame's tree,
    /// not a stale one from before the tree's shape changed. Takes
    /// `&self` (not `&mut self`), like [`Self::set_test_clock_ms`] --
    /// `AppLogic::render` only ever gets `&self`.
    pub fn set_leaf_tooltips(&self, tooltips: HashMap<WidgetId, String>) {
        *self.leaf_tooltips.borrow_mut() = tooltips;
    }

    /// This host's most recently painted bounds for `id`, read from
    /// `engine`'s own [`InteractionEngine::hit_map`] — the anchor rect
    /// [`Self::drive_hover_tooltip`] shows a tooltip against. Only
    /// [`PaintedHit::Button`] carries a tooltip source today (see
    /// [`Self::hover_tooltip_text`]), so this only matches that variant;
    /// extending [`Self::hover_tooltip_text`] to a further primitive
    /// extends this the same way.
    fn hover_anchor(engine: &InteractionEngine, id: WidgetId) -> Option<Rect> {
        engine.hit_map().iter().find_map(|hit| match hit {
            PaintedHit::Button { id: hit_id, layout } if *hit_id == id => Some(layout.bounds),
            _ => None,
        })
    }

    /// Drives [`Self::tooltip_tracker`] from `engine`'s current hit-tested
    /// hover ([`InteractionEngine::hovered`]) and the given headless
    /// clock, opening/closing `engine`'s own tooltip the same way a
    /// native backend's hover-driven test wires [`HoverTooltipTracker`]
    /// directly against `Backend::draw_tooltip` — except here the hovered
    /// widget's tooltip text comes from this host's own descriptor (via
    /// [`Self::hover_tooltip_text`]) rather than a bespoke test
    /// `AppLogic`, since a binding's `render` call is what populated that
    /// descriptor in the first place.
    ///
    /// Call this once per `handle`/`tick` dispatch, after `engine.handle`/
    /// [`InteractionEngine::set_hits`] has already run for the frame, with
    /// the same `now_ms` headless clock [`crate::engine::InteractionEngine`]'s
    /// own module doc and [`HoverTooltipTracker`]'s "headless-clock
    /// contract" describe — [`Self::now_ms`] for a real wall clock, or a
    /// hand-picked counter in a test. `delay_ms` is normally
    /// [`quadraui::Style::tooltip_delay_ms`], `auto_dismiss_ms` normally
    /// [`quadraui::Style::tooltip_auto_dismiss_ms`] (`0` disables it).
    ///
    /// Resyncs first: if [`Self::tooltip_tracker`] still thinks a tooltip
    /// is visible but `engine`'s own tooltip has already closed (e.g.
    /// [`InteractionEngine::handle`]'s own click/key auto-dismiss ran
    /// since the last call), the tracker is explicitly dismissed too, so
    /// a held-still hover over the same widget restarts the delay instead
    /// of believing a tooltip is still showing that the screen no longer
    /// has.
    ///
    /// The pending-delay tick only promotes [`HoverTooltipTracker`] to
    /// visible once [`Self::hover_anchor`] actually finds a painted rect
    /// for the hovered id. Without that guard, a hovered id with no
    /// anchor (not yet painted this frame, or no [`PaintedHit::Button`]
    /// for it at all) would flip the tracker's own phase to visible with
    /// nothing shown on screen; the next call's resync above would then
    /// see `engine`'s tooltip still closed, dismiss the tracker, and
    /// `on_hover` would re-arm it from scratch — pending, visible-but-
    /// unpainted, dismissed, forever, for a pointer that never moved. The
    /// wake schedule below has the same guard, for the same reason:
    /// without an anchor, [`HoverTooltipTracker::tick`] never ran, so
    /// [`HoverTooltipTracker::next_wake_ms`] would otherwise keep
    /// returning an already-elapsed `Some(0)` and arm a zero-delay wake
    /// every single dispatch for as long as the hover stays stale.
    ///
    /// A tooltip that [`HoverTooltipTracker::tick_auto_dismiss`] hides
    /// stays hidden while the pointer keeps hovering the same anchor —
    /// the tracker latches into its own `Dismissed` phase rather than
    /// resetting to idle, so the delay doesn't simply re-arm and show the
    /// same tooltip again `delay_ms` later (which would auto-dismiss
    /// again after that, forever, with no user input at all). Hovering a
    /// different widget, or leaving, clears the latch.
    ///
    /// Schedules every wake this call still needs — the pending-delay
    /// deadline while an anchor exists, and, while a tooltip is visible
    /// with `auto_dismiss_ms > 0`, its auto-dismiss deadline — directly
    /// on `backend` via [`Backend::request_frame_in`], not only through
    /// the returned [`Reaction`]. A `Reaction::RedrawAfter` folded into a
    /// caller's own `Reaction::Redraw` via [`Reaction::merge`] loses the
    /// deadline (`Redraw` always wins); `request_frame_in` doesn't go
    /// through that merge at all, so the wake survives even on the frame
    /// a hover change *also* triggers an unrelated `Redraw`. The wake is
    /// still deliberately *also* folded into the returned `Reaction`, so
    /// a steady-state pending/visible hover arms two wakes per call
    /// (direct + the caller's own `request_frame_in` off the returned
    /// `Reaction`) rather than one; [`Reaction::RedrawAfter`]'s own doc
    /// calls an early extra wake harmless, and this guarantees the exact
    /// deadline reaches `backend` even when the caller's runner doesn't
    /// re-derive it from the merged reaction at all.
    ///
    /// Returns the [`Reaction`] the caller should merge into its own:
    /// [`Reaction::Redraw`] when a tooltip opened, closed, or
    /// auto-dismissed this call, [`Reaction::RedrawAfter`] while one is
    /// still pending (with an anchor) or counting down to auto-dismiss,
    /// [`Reaction::Continue`] otherwise.
    pub fn drive_hover_tooltip(
        &mut self,
        engine: &mut InteractionEngine,
        backend: &mut dyn Backend,
        now_ms: u64,
        delay_ms: u64,
        auto_dismiss_ms: u64,
    ) -> Reaction {
        if self.tooltip_tracker.visible_item().is_some() && !engine.tooltip_open() {
            self.tooltip_tracker.dismiss();
        }

        let hovered = engine
            .hovered()
            .and_then(|(id, _)| self.hover_tooltip_text(id).map(|text| (id, text)));
        let hovered_id = hovered.as_ref().map(|(id, _)| *id);

        let mut reaction = if self.tooltip_tracker.on_hover(hovered_id, now_ms) {
            engine.hide_tooltip();
            Reaction::Redraw
        } else {
            Reaction::Continue
        };

        let anchor = hovered
            .as_ref()
            .and_then(|(id, _)| Self::hover_anchor(engine, *id));
        if anchor.is_some() && self.tooltip_tracker.tick(delay_ms, now_ms) {
            if let (Some((id, text)), Some(anchor)) = (hovered, anchor) {
                engine.show_tooltip(Tooltip::new(id, text), anchor);
                reaction = Reaction::Redraw;
            }
        }

        if auto_dismiss_ms > 0
            && self
                .tooltip_tracker
                .tick_auto_dismiss(auto_dismiss_ms, now_ms)
        {
            engine.hide_tooltip();
            reaction = Reaction::Redraw;
        }

        // Gated on `anchor.is_some()`: without a painted anchor, `tick`
        // above never ran, so `Phase::Pending` never advances and
        // `next_wake_ms` would return `Some(0)` on every call once the
        // delay has already elapsed -- a zero-delay wake re-armed every
        // dispatch for as long as the hover stays stale. Nothing can
        // make the tracker progress without a new anchor anyway, so
        // skip arming a wake for it at all.
        if anchor.is_some() {
            if let Some(remaining_ms) = self.tooltip_tracker.next_wake_ms(now_ms, delay_ms) {
                let wake = Duration::from_millis(remaining_ms);
                backend.request_frame_in(wake);
                reaction = reaction.merge(Reaction::RedrawAfter(wake));
            }
        }

        if auto_dismiss_ms > 0 {
            if let Some(remaining_ms) = self
                .tooltip_tracker
                .next_auto_dismiss_wake_ms(now_ms, auto_dismiss_ms)
            {
                let wake = Duration::from_millis(remaining_ms);
                backend.request_frame_in(wake);
                reaction = reaction.merge(Reaction::RedrawAfter(wake));
            }
        }

        reaction
    }
}

// ─── Tree cursor + scroll, uncontrolled by default ─────────────────────────
//
// A `TreeView` descriptor's own `selected_path`/`scroll_offset` stay the
// source of truth whenever an app sets them (controlled mode, unchanged
// from before this module existed). When it leaves them at their defaults
// (`None` / `0`) the app gets working keyboard navigation for free: the
// cursor and its scroll position live in `WidgetState`, keyed by the
// tree's own id, moved and scrolled into view through
// `quadraui::compose::TreeController`'s own navigation primitives rather
// than re-deriving that arithmetic here.
//
// [`apply_tree_nav_key`] is the event-time half — called from a key
// handler (e.g. [`crate::engine::InteractionEngine::handle`]) with the
// `PaintedHit::Tree` geometry the most recent render produced, it moves
// the stored cursor and persists the result. [`resolve_tree_cursor`] is
// the render-time half — called right before painting, it merges the
// descriptor's own (possibly controlled) fields with whatever is stored,
// re-clamps the result into view against the current viewport, and
// returns the `TreeView` a binding should actually paint.

/// The subset of `TreeNavKey` this module's cursor navigation
/// (`Tree`/`List`/`DataTable` alike) actually moves a stored cursor for.
/// Enter and Space stay out of scope: row activation and expand/collapse
/// keep going through each widget's own existing semantic-event
/// translation (`crate::semantic::list_key_event`/`data_table_key_event`,
/// or `Tree`'s raw `TreeEvent::KeyPressed` fallback) rather than moving
/// into the widget-state store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorNav {
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

/// Classifies a raw key into [`CursorNav`] by delegating to
/// `quadraui::compose::classify_tree_nav_key` — the single key map
/// `TreeController::handle_key` and `SidebarSystem`'s own key dispatch
/// already share — rather than hand-copying it a second time here, which
/// would drift the moment that shared map changes. `vim_keys: true`
/// always, matching `TreeController::new`'s own default: none of `Tree`/
/// `List`/`DataTable`'s descriptors carry a per-widget toggle for this
/// crate to read (`ListView`'s own module doc already documents `j`/`k`
/// as `Down`/`Up` aliases unconditionally, matching this).
fn cursor_nav_for_key(key: &Key) -> Option<CursorNav> {
    match classify_tree_nav_key(key, true)? {
        TreeNavKey::Up => Some(CursorNav::Up),
        TreeNavKey::Down => Some(CursorNav::Down),
        TreeNavKey::Home => Some(CursorNav::Home),
        TreeNavKey::End => Some(CursorNav::End),
        TreeNavKey::PageUp => Some(CursorNav::PageUp),
        TreeNavKey::PageDown => Some(CursorNav::PageDown),
        TreeNavKey::Enter | TreeNavKey::ToggleExpand => None,
    }
}

/// `(viewport_rows.max(1) - 1).max(1)`, as an `isize` delta — the page
/// size `TreeController::handle_key`'s own `PageUp`/`PageDown` arms use,
/// shared here so `Tree`/`List`/`DataTable`'s own page navigation agrees
/// with it instead of re-deriving the formula per call site.
fn page_delta(viewport_rows: usize) -> isize {
    (viewport_rows.max(1) - 1).max(1) as isize
}

/// A `Tree`'s cursor + scroll, persisted in [`WidgetState`] keyed by the
/// tree's own id. [`apply_tree_nav_key`] writes it on a recognized
/// navigation key; [`resolve_tree_cursor`] reads it back — merged with
/// whatever the descriptor itself pins — at render time.
///
/// `scroll_controlled` latches `true` the first time a frame's own
/// `TreeView::scroll_offset` is non-zero, and then stays `true` forever
/// after (even across a later frame whose `scroll_offset` goes back to
/// `0`) — see [`resolve_tree_cursor`]'s own doc for why `scroll_offset`
/// alone can't tell "controlled, pinned to the top" apart from
/// "uncontrolled, never pinned" and why this latch is the fix.
#[derive(Debug, Clone, Default)]
struct TreeCursorState {
    selected_path: Option<TreePath>,
    scroll_offset: usize,
    scroll_controlled: bool,
}

/// A throwaway [`TreeRow`] standing in for `path`, with every other field
/// at its default — used only to satisfy
/// [`quadraui::compose::TreeController::set_rows`]'s shape so its own
/// `move_selection_by`/`move_selection_to_edge` can be reused verbatim for
/// pure path/count arithmetic that never reads a row's icon, text, badge,
/// or expand state.
fn placeholder_tree_row(path: TreePath) -> TreeRow {
    TreeRow {
        path,
        indent: 0,
        icon: None,
        text: StyledText::plain(""),
        badge: None,
        is_expanded: None,
        decoration: Decoration::default(),
        edit: None,
    }
}

/// Up/Down/vim `j`/`k`/Home/End/PageUp/PageDown navigation on a focused
/// `Tree`, resolved against [`WidgetState`] rather than the app: moves the
/// stored cursor for `id` by one row (or to an edge, or by a page) and
/// scrolls it into view, via `quadraui::compose::TreeController`'s own
/// navigation primitives (never re-derived here) seeded from whichever
/// cursor is currently stored. `row_paths` and `viewport_rows` should come
/// from `crate::hit::tree_rows_and_viewport` against the most recent
/// render's hit map — the same geometry [`crate::drag::tree_scroll_state`]
/// already reads for scrollbar drag/wheel handling — so movement matches
/// what was actually painted rather than a stale descriptor, and so a
/// tree whose selected row is currently mid-rename is never navigated out
/// from under the edit in progress (`tree_rows_and_viewport` already
/// returns `None` for that case).
///
/// Returns `true` (and leaves [`WidgetState`] updated) for a recognized
/// navigation key that actually moved the cursor, so the caller knows to
/// redraw; `false` for any other key, or for a recognized key that moved
/// nothing (an empty tree, or already at the edge an arrow key pushes
/// further against), which the caller should fall through to unchanged
/// (today's raw `TreeEvent::KeyPressed` dispatch keeps working for Enter,
/// Space, and anything app-specific). A controlled `Tree` (whose
/// descriptor sets a non-default `selected_path`/`scroll_offset`) still
/// has its stored cursor moved here — harmless, since [`resolve_tree_cursor`]
/// discards it again in favor of the descriptor's own value on the very
/// next render, the same "controlled wins and is written back" rule
/// [`WidgetState::resolve`]'s own doc already describes.
pub fn apply_tree_nav_key(
    widget_state: &WidgetState,
    id: WidgetId,
    row_paths: &[TreePath],
    viewport_rows: usize,
    key: &Key,
) -> bool {
    let Some(nav) = cursor_nav_for_key(key) else {
        return false;
    };
    if row_paths.is_empty() {
        return false;
    }
    let stored = widget_state.get::<TreeCursorState>(id).unwrap_or_default();
    let mut controller = TreeController::new(id.as_str());
    controller.set_rows(
        row_paths
            .iter()
            .cloned()
            .map(placeholder_tree_row)
            .collect(),
    );
    controller.set_selected_path(stored.selected_path);
    controller.set_scroll_offset(stored.scroll_offset);
    let event = match nav {
        CursorNav::Up => controller.move_selection_by(-1, viewport_rows),
        CursorNav::Down => controller.move_selection_by(1, viewport_rows),
        CursorNav::Home => controller.move_selection_to_edge(Edge::Start, viewport_rows),
        CursorNav::End => controller.move_selection_to_edge(Edge::End, viewport_rows),
        CursorNav::PageUp => {
            controller.move_selection_by(-page_delta(viewport_rows), viewport_rows)
        }
        CursorNav::PageDown => {
            controller.move_selection_by(page_delta(viewport_rows), viewport_rows)
        }
    };
    widget_state.set(
        id,
        TreeCursorState {
            selected_path: controller.selected_path().cloned(),
            scroll_offset: controller.scroll_offset(),
            scroll_controlled: stored.scroll_controlled,
        },
    );
    matches!(event, TreeControllerEvent::RowSelected { .. })
}

/// Resolves `tree`'s cursor + scroll for painting, through the
/// controlled/uncontrolled policy [`WidgetState`]'s own module doc
/// describes: `tree.selected_path` wins outright when `Some`; otherwise
/// the id's stored cursor wins (`None`, no selection, the first time it's
/// seen).
///
/// `scroll_offset` needs one more rule on top of that, because `0` is
/// already `TreeView::scroll_offset`'s own "unscrolled" sentinel and so
/// can't tell "the app explicitly pins the scroll to the top" apart from
/// "the app never set this field at all". [`TreeCursorState::scroll_controlled`]
/// resolves the ambiguity by latching: the very first frame whose
/// `tree.scroll_offset` is non-zero marks this id controlled from then on,
/// including every later frame whose `scroll_offset` goes back to `0` —
/// so an app that pins `20`, lets the user wheel back to the top, and then
/// paints `0` again keeps that `0` rather than getting stuck at `20`
/// forever (the bug this latch exists to fix) or silently falling back to
/// treating `0` as uncontrolled (which would just move the bug to "pin at
/// exactly `0`" instead). The one case this still can't distinguish —
/// an app that pins `0` from the very first frame and never changes it —
/// is indistinguishable from "uncontrolled, never touched" without a
/// breaking change to `TreeView::scroll_offset`'s own type; see
/// `CLAUDE.md`'s *Downstream consumers* section for why that cost isn't
/// paid here.
///
/// Once controlled, the descriptor's value wins outright and is returned
/// exactly as given — never nudged to keep the selection visible — while
/// uncontrolled resolves from the store and IS scrolled into view (via
/// `TreeController::scroll_to_visible`, never re-derived here) against the
/// current `viewport_rows`, so a resize or a freshly uncontrolled
/// selection are both caught here even without an intervening
/// [`apply_tree_nav_key`] call.
///
/// Call once per frame, before painting `tree` — the returned [`TreeView`]
/// is what a binding's render should hand to [`crate::render::draw_bands`]/
/// [`crate::render::paint_layout`], not `tree` itself.
pub fn resolve_tree_cursor(
    widget_state: &WidgetState,
    tree: &TreeView,
    viewport_rows: usize,
) -> TreeView {
    let id = tree.id;
    let stored = widget_state.get::<TreeCursorState>(id).unwrap_or_default();

    let selected_path = tree.selected_path.clone().or(stored.selected_path);
    let scroll_controlled = stored.scroll_controlled || tree.scroll_offset != 0;
    let mut scroll_offset = if scroll_controlled {
        tree.scroll_offset
    } else {
        stored.scroll_offset
    };

    if !scroll_controlled {
        if let Some(path) = &selected_path {
            if let Some(row) = tree.rows.iter().position(|r| &r.path == path) {
                let mut controller = TreeController::new(id.as_str());
                controller.set_scroll_offset(scroll_offset);
                controller.scroll_to_visible(row, viewport_rows);
                scroll_offset = controller.scroll_offset();
            }
        }
    }

    widget_state.set(
        id,
        TreeCursorState {
            selected_path: selected_path.clone(),
            scroll_offset,
            scroll_controlled,
        },
    );

    TreeView {
        selected_path,
        scroll_offset,
        ..tree.clone()
    }
}

// ─── List cursor + scroll, uncontrolled by default ─────────────────────────
//
// Same policy as the `Tree` section above, for a flat `ListView`. Unlike
// `Tree`, there is no composed `ListController` in `quadraui::compose` to
// reuse for movement/scroll-into-view (only `TreeController` exists today)
// — [`move_flat_index`]/[`edge_flat_index`]/[`scroll_into_view`] below are
// the pure index arithmetic a future `ListController` would also need,
// kept as free functions here rather than duplicated per widget since
// `List`/`DataTable` share the identical flat-cursor shape.

/// A `List`'s cursor + scroll, persisted in [`WidgetState`] keyed by the
/// list's own id. `selected_idx`/`selected_controlled` mirror
/// [`TreeCursorState::scroll_controlled`]'s latch, because
/// `ListView::selected_idx` (a bare `usize`, always a valid index into a
/// non-empty list) has the exact same `0`-is-ambiguous shape
/// `TreeView::scroll_offset` does — see [`resolve_list_cursor`]'s own doc.
#[derive(Debug, Clone, Default)]
struct ListCursorState {
    selected_idx: Option<usize>,
    selected_controlled: bool,
    scroll_offset: usize,
    scroll_controlled: bool,
}

/// Moves a flat (non-hierarchical) cursor by `delta` rows within `[0,
/// count)`, clamped at either edge — the `List`/`DataTable` counterpart of
/// `TreeController::move_selection_by`'s own clamp. `None` means "nothing
/// selected yet": a forward move (`delta > 0`) starts at row `0`, a
/// backward one at the last row, matching `TreeController::move_selection_by`'s
/// own "no current selection" branch. Returns `None` only when `count == 0`.
fn move_flat_index(current: Option<usize>, delta: isize, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let new = match current {
        Some(cur) => {
            let target = cur as isize + delta;
            target.max(0).min(count as isize - 1) as usize
        }
        None if delta > 0 => 0,
        None => count - 1,
    };
    Some(new)
}

/// The first (`Edge::Start`) or last (`Edge::End`) row of a `count`-row
/// flat list — the `List`/`DataTable` counterpart of
/// `TreeController::move_selection_to_edge`'s own edge row. Returns `None`
/// only when `count == 0`.
fn edge_flat_index(edge: Edge, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match edge {
        Edge::Start => 0,
        Edge::End => count - 1,
    })
}

/// Scrolls `scroll_offset` by the minimum amount needed to bring `row`
/// into a `viewport_rows`-tall window — the `List`/`DataTable` counterpart
/// of `TreeController::scroll_to_visible`'s own formula.
fn scroll_into_view(scroll_offset: usize, row: usize, viewport_rows: usize) -> usize {
    if viewport_rows == 0 {
        return scroll_offset;
    }
    if row < scroll_offset {
        row
    } else if row >= scroll_offset + viewport_rows {
        row.saturating_sub(viewport_rows.saturating_sub(1))
    } else {
        scroll_offset
    }
}

/// Up/Down/vim `j`/`k`/Home/End/PageUp/PageDown navigation on a focused
/// `List`, resolved against [`WidgetState`] — the `List` counterpart of
/// [`apply_tree_nav_key`]. `total_items`/`viewport_rows` should come from
/// `crate::hit::list_rows_and_viewport` against the most recent render's
/// hit map. Returns `true` (and leaves [`WidgetState`] updated) only when
/// a recognized navigation key actually moved the cursor; `false`
/// otherwise. Either way the event the caller goes on to dispatch is
/// unchanged — `crate::semantic::list_key_event`'s Enter translation, or
/// the raw `ListViewEvent::KeyPressed` fallback for everything else,
/// navigation keys included, so an app that tracks its own
/// `ListView::selected_idx` still sees them.
pub fn apply_list_nav_key(
    widget_state: &WidgetState,
    id: WidgetId,
    total_items: usize,
    viewport_rows: usize,
    key: &Key,
) -> bool {
    let Some(nav) = cursor_nav_for_key(key) else {
        return false;
    };
    let stored = widget_state.get::<ListCursorState>(id).unwrap_or_default();
    let new_idx = match nav {
        CursorNav::Up => move_flat_index(stored.selected_idx, -1, total_items),
        CursorNav::Down => move_flat_index(stored.selected_idx, 1, total_items),
        CursorNav::Home => edge_flat_index(Edge::Start, total_items),
        CursorNav::End => edge_flat_index(Edge::End, total_items),
        CursorNav::PageUp => {
            move_flat_index(stored.selected_idx, -page_delta(viewport_rows), total_items)
        }
        CursorNav::PageDown => {
            move_flat_index(stored.selected_idx, page_delta(viewport_rows), total_items)
        }
    };
    let moved = new_idx != stored.selected_idx;
    let scroll_offset = match new_idx {
        Some(row) if moved => scroll_into_view(stored.scroll_offset, row, viewport_rows),
        _ => stored.scroll_offset,
    };
    widget_state.set(
        id,
        ListCursorState {
            selected_idx: new_idx,
            selected_controlled: stored.selected_controlled,
            scroll_offset,
            scroll_controlled: stored.scroll_controlled,
        },
    );
    moved
}

/// Resolves `list`'s cursor + scroll for painting — the `List` counterpart
/// of [`resolve_tree_cursor`]. `ListView::selected_idx` has no "unset"
/// state of its own (always a valid index into a non-empty list, default
/// `0`), so it gets the exact same latch [`resolve_tree_cursor`]'s own doc
/// describes for `scroll_offset`: `selected_controlled` turns `true` the
/// first time `list.selected_idx` is non-zero and then stays `true`,
/// including a later frame whose `selected_idx` goes back to `0`. An app
/// that genuinely pins the selection at row `0` from the very first frame
/// is indistinguishable from "uncontrolled, never touched" — the same
/// residual ambiguity [`resolve_tree_cursor`] accepts for `scroll_offset`.
///
/// Call once per frame, before painting `list`.
pub fn resolve_list_cursor(
    widget_state: &WidgetState,
    list: &ListView,
    viewport_rows: usize,
) -> ListView {
    let id = list.id;
    let stored = widget_state.get::<ListCursorState>(id).unwrap_or_default();

    let selected_controlled = stored.selected_controlled || list.selected_idx != 0;
    let selected_idx = if selected_controlled {
        Some(list.selected_idx)
    } else {
        stored.selected_idx
    };
    let resolved_selected_idx = selected_idx.unwrap_or(0);

    let scroll_controlled = stored.scroll_controlled || list.scroll_offset != 0;
    let mut scroll_offset = if scroll_controlled {
        list.scroll_offset
    } else {
        stored.scroll_offset
    };

    if !scroll_controlled {
        scroll_offset = scroll_into_view(scroll_offset, resolved_selected_idx, viewport_rows);
    }

    widget_state.set(
        id,
        ListCursorState {
            selected_idx,
            selected_controlled,
            scroll_offset,
            scroll_controlled,
        },
    );

    ListView {
        selected_idx: resolved_selected_idx,
        scroll_offset,
        ..list.clone()
    }
}

// ─── DataTable cursor + scroll, uncontrolled by default ────────────────────
//
// Same policy as `List` above, reusing the same flat-cursor arithmetic.
// `DataTable::selected_idx` is already `Option<usize>` (unlike `ListView`'s
// bare `usize`), so its own resolution needs no latch — `None` already
// unambiguously means "no selection, uncontrolled" the same way
// `TreeView::selected_path` does.

/// A `DataTable`'s cursor + scroll, persisted in [`WidgetState`] keyed by
/// the table's own id.
#[derive(Debug, Clone, Default)]
struct DataTableCursorState {
    selected_idx: Option<usize>,
    scroll_offset: usize,
    scroll_controlled: bool,
}

/// Up/Down/vim `j`/`k`/Home/End/PageUp/PageDown navigation on a focused
/// `DataTable`, resolved against [`WidgetState`] — the `DataTable`
/// counterpart of [`apply_tree_nav_key`]/[`apply_list_nav_key`].
/// `total_rows`/`viewport_rows` should come from
/// `crate::hit::data_table_rows_and_viewport` against the most recent
/// render's hit map. Returns `true` (and leaves [`WidgetState`] updated)
/// only when a recognized navigation key actually moved the cursor;
/// `false` otherwise, so the caller falls through unchanged — today's
/// `crate::semantic::data_table_key_event` Enter/Up/Down event
/// translation.
pub fn apply_data_table_nav_key(
    widget_state: &WidgetState,
    id: WidgetId,
    total_rows: usize,
    viewport_rows: usize,
    key: &Key,
) -> bool {
    let Some(nav) = cursor_nav_for_key(key) else {
        return false;
    };
    let stored = widget_state
        .get::<DataTableCursorState>(id)
        .unwrap_or_default();
    let new_idx = match nav {
        CursorNav::Up => move_flat_index(stored.selected_idx, -1, total_rows),
        CursorNav::Down => move_flat_index(stored.selected_idx, 1, total_rows),
        CursorNav::Home => edge_flat_index(Edge::Start, total_rows),
        CursorNav::End => edge_flat_index(Edge::End, total_rows),
        CursorNav::PageUp => {
            move_flat_index(stored.selected_idx, -page_delta(viewport_rows), total_rows)
        }
        CursorNav::PageDown => {
            move_flat_index(stored.selected_idx, page_delta(viewport_rows), total_rows)
        }
    };
    let moved = new_idx != stored.selected_idx;
    let scroll_offset = match new_idx {
        Some(row) if moved => scroll_into_view(stored.scroll_offset, row, viewport_rows),
        _ => stored.scroll_offset,
    };
    widget_state.set(
        id,
        DataTableCursorState {
            selected_idx: new_idx,
            scroll_offset,
            scroll_controlled: stored.scroll_controlled,
        },
    );
    moved
}

/// Resolves `table`'s cursor + scroll for painting — the `DataTable`
/// counterpart of [`resolve_tree_cursor`]/[`resolve_list_cursor`]. No
/// latch needed for `selected_idx` (unlike `List`'s): `DataTable::selected_idx`
/// is already `Option<usize>`, so `None` unambiguously means
/// "uncontrolled". `scroll_offset` still needs the same latch
/// [`resolve_tree_cursor`]'s own doc describes, for the identical reason
/// (`DataTable::scroll_offset` is a bare `usize`).
///
/// Call once per frame, before painting `table`.
pub fn resolve_data_table_cursor(
    widget_state: &WidgetState,
    table: &DataTable,
    viewport_rows: usize,
) -> DataTable {
    let id = table.id;
    let stored = widget_state
        .get::<DataTableCursorState>(id)
        .unwrap_or_default();

    let selected_idx = table.selected_idx.or(stored.selected_idx);

    let scroll_controlled = stored.scroll_controlled || table.scroll_offset != 0;
    let mut scroll_offset = if scroll_controlled {
        table.scroll_offset
    } else {
        stored.scroll_offset
    };

    if !scroll_controlled {
        if let Some(row) = selected_idx {
            scroll_offset = scroll_into_view(scroll_offset, row, viewport_rows);
        }
    }

    widget_state.set(
        id,
        DataTableCursorState {
            selected_idx,
            scroll_offset,
            scroll_controlled,
        },
    );

    DataTable {
        selected_idx,
        scroll_offset,
        ..table.clone()
    }
}

/// The anchored-overlay paint pass every binding's `AppLogic::render` runs
/// once, after painting its own fixed/additive slots (via [`crate::render::draw_bands`]
/// or [`crate::render::paint_layout`]) and collecting `hits`: dialog on top of
/// everything, then float/rich-text-popup/completions, then tooltip, then
/// toast last of all — a correctness contract worth writing down in exactly
/// one place rather than copy-pasted at each of a binding's render paths
/// (the static-slots path and the render-callback path both reach this).
/// Takes `hits` by value and hands it to [`InteractionEngine::set_hits`] at
/// the end, after every `sync_*`/`render_*` call above has had a chance to
/// read it by reference.
pub fn render_overlays(
    engine: &InteractionEngine,
    backend: &mut dyn Backend,
    hits: Vec<PaintedHit>,
) {
    engine.sync_menu_bar(backend, &hits);
    engine.sync_forms(backend, &hits);
    engine.sync_text_input_carets(backend, &hits);
    engine.render_ctx_menu(backend);
    engine.render_float(backend);
    engine.render_rich_text_popup(backend);
    engine.render_completions(backend);
    engine.render_dialog(backend);
    engine.render_tooltip(backend);
    engine.render_toast(backend);
    engine.set_hits(hits);
}

/// Maps a registered handler/tick callback's return value to a [`Reaction`]:
/// `Some(false)` requests the same exit a raised exception would, with no
/// exception needed; anything else (including the common case of no
/// explicit `return` and a non-bool return) continues. `value` is the
/// callback's return value, already narrowed to `Option<bool>` by the
/// binding's own "was this literally a bool" check (an int `0`, an empty
/// string, or any other "falsy" host-language value must keep running
/// rather than exiting, so only an explicit `false` opts in — the binding
/// decides what "literally a bool" means for its own host language, this
/// function only decides what to do once it knows).
pub fn callback_reaction(value: Option<bool>) -> Reaction {
    match value {
        Some(false) => Reaction::Exit,
        _ => Reaction::Continue,
    }
}

/// Wraps [`callback_reaction`], promoting an ordinary [`Reaction::Continue`]
/// to [`Reaction::Redraw`] whenever `render_callback_present` — i.e. a
/// render callback is registered on this app.
///
/// A render callback only runs when the runner decides a frame needs
/// repainting (`AppLogic::render` is reached only after an
/// `EventOutcome::Redraw`/`RedrawAfter`-driven repaint). A binding's own
/// `handle`/`tick` have no way to tell whether a callback's mutation of its
/// own host-language state changed anything visible, so once a render
/// callback is registered every non-exit dispatch is treated as one that
/// might have — the same "redraw every frame, let the render callback
/// decide what that looks like" contract a React/Electron-style
/// `render(state)` callback expects. Without this, a handler returning the
/// common "no explicit return" case maps to `Reaction::Continue`, the
/// runner never calls `AppLogic::render` again, and the render callback
/// never re-runs — the screen stays frozen at whatever the last repaint
/// drew.
pub fn callback_reaction_for_render(
    value: Option<bool>,
    render_callback_present: bool,
) -> Reaction {
    match callback_reaction(value) {
        Reaction::Continue if render_callback_present => Reaction::Redraw,
        other => other,
    }
}

/// Maps a registered handler's return value to a [`Reaction`] for a
/// `WindowClose` dispatch specifically — the opposite default from
/// [`callback_reaction`]. A `WindowClose` handler's job is to decide
/// whether the window may close, so the no-opinion case (`None` — by far
/// the common case for a handler that just logs or ignores the event) must
/// let the close proceed, exactly like having no handler registered at
/// all; only an explicit `Some(true)` vetoes it and leaves the window open.
/// A non-bool return (an int, a string, any other "truthy" host-language
/// value, narrowed to `None` by the binding's own check) does not
/// accidentally veto a close the author never meant to block.
pub fn window_close_reaction(value: Option<bool>, render_callback_present: bool) -> Reaction {
    match value {
        Some(true) if render_callback_present => Reaction::Redraw,
        Some(true) => Reaction::Continue,
        _ => Reaction::Exit,
    }
}

/// Whether a binding's own `AppLogic` has a registered handler for
/// `WindowClose` events — the one piece of host-specific knowledge
/// [`NativeExitPolicy`] needs that it cannot recover from a [`Reaction`]
/// alone (a no-handler-at-all dispatch and an explicit "stay open" veto
/// both resolve to [`Reaction::Continue`], but only the former should force
/// an exit). A binding implements this once, trivially (`self.handler.is_some()`
/// or equivalent), alongside its own [`AppLogic`] impl.
pub trait WindowCloseHandling {
    fn has_window_close_handler(&self) -> bool;
}

/// Wraps a binding's own `AppLogic` for the duration of a `run_native`-style
/// call into a real `GtkRunner`/`MacRunner`/`WinRunner` — see this module's
/// own doc for the two things it corrects that `apply_reaction`'s generic
/// `Reaction` handling cannot: a handler-less `WindowClose` must actually
/// exit (the inner `AppLogic`'s own "no handler" default is
/// [`Reaction::Continue`], which would otherwise leave `run_native()` with
/// no way to close its own window), and — on the `macos` feature without
/// `gtk` also enabled — every *other* `Reaction::Exit` must downgrade to
/// `Continue` plus `exit_requested` instead of reaching
/// `apply_reaction`'s own `Reaction::Exit` branch, which calls
/// `[NSApp terminate:]` and ends the whole OS process before `run_native`
/// could return.
///
/// `win` needs no `exit_requested` field, for the same reason `gtk`
/// doesn't: both already call their own window-destroy path for *any*
/// `Reaction::Exit`, not just one reached through `WindowClose` — there is
/// no AppKit-style `[NSApp terminate:]` for this wrapper to head off there.
#[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
#[cfg_attr(
    any(
        all(feature = "macos", not(feature = "gtk"), not(target_os = "macos")),
        all(
            feature = "win",
            not(feature = "gtk"),
            not(feature = "macos"),
            not(target_os = "windows")
        )
    ),
    allow(dead_code)
)]
pub struct NativeExitPolicy<A> {
    pub inner: A,
    /// Set by `handle`/`tick` when a non-`WindowClose` `Reaction::Exit` is
    /// intercepted; the caller's own pump loop breaks once this is set.
    /// Only present on the `macos` feature without `gtk` also enabled —
    /// GTK's own `apply_reaction` and `win`'s dispatch/tick already handle
    /// `Reaction::Exit` from any event without this wrapper's help.
    #[cfg(all(feature = "macos", not(feature = "gtk")))]
    pub exit_requested: Rc<Cell<bool>>,
}

#[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
impl<A: AppLogic + WindowCloseHandling> AppLogic for NativeExitPolicy<A> {
    type AreaId = A::AreaId;

    fn setup(&mut self, backend: &mut dyn Backend) {
        self.inner.setup(backend);
    }

    fn render(&self, backend: &mut dyn Backend, area: Self::AreaId) {
        self.inner.render(backend, area);
    }

    fn tab_stops(&self, area: Self::AreaId) -> Vec<(WidgetId, Rect)> {
        self.inner.tab_stops(area)
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let is_window_close = matches!(event, UiEvent::WindowClose);
        // A registered handler's own opinion on `WindowClose` — including a
        // veto (anything but `Reaction::Exit`) — must reach the caller
        // untouched; only the no-handler-at-all case gets a forced default.
        let has_window_close_handler = is_window_close && self.inner.has_window_close_handler();
        let reaction = self.inner.handle(event, backend);
        if is_window_close {
            return if has_window_close_handler {
                reaction
            } else {
                Reaction::Exit
            };
        }
        #[cfg(all(feature = "macos", not(feature = "gtk")))]
        if matches!(reaction, Reaction::Exit) {
            self.exit_requested.set(true);
            return Reaction::Continue;
        }
        reaction
    }

    fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
        let reaction = self.inner.tick(backend);
        #[cfg(all(feature = "macos", not(feature = "gtk")))]
        if matches!(reaction, Reaction::Exit) {
            self.exit_requested.set(true);
            return Reaction::Continue;
        }
        reaction
    }
}

/// Narrow, dyn-safe surface of [`TuiDriver`] a binding's own headless-shell
/// app (one built around `quadraui::tui::shell_adapter`'s `ShellApp`, whose
/// concrete `TuiDriver<impl AppLogic>` type is opaque per call site) can
/// store behind `Box<dyn ErasedShellDriver>` instead.
pub trait ErasedShellDriver {
    fn dispatch(&mut self, event: UiEvent) -> Reaction;
    fn pump_user_events(&mut self) -> Reaction;
    fn tick(&mut self) -> Reaction;
    fn resize(&mut self, width: u16, height: u16);
    fn exited(&self) -> bool;
    fn screen(&self) -> String;
    fn screen_contains(&self, needle: &str) -> bool;
    fn find(&self, needle: &str) -> Option<(f32, f32)>;
    fn tab_center(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)>;
}

impl<A: AppLogic> ErasedShellDriver for TuiDriver<A> {
    fn dispatch(&mut self, event: UiEvent) -> Reaction {
        self.dispatch(event)
    }

    fn pump_user_events(&mut self) -> Reaction {
        self.pump_user_events()
    }

    fn tick(&mut self) -> Reaction {
        self.tick()
    }

    fn resize(&mut self, width: u16, height: u16) {
        self.resize(width, height)
    }

    fn exited(&self) -> bool {
        self.exited()
    }

    fn screen(&self) -> String {
        self.screen()
    }

    fn screen_contains(&self, needle: &str) -> bool {
        self.screen_contains(needle)
    }

    fn find(&self, needle: &str) -> Option<(f32, f32)> {
        self.find(needle)
    }

    fn tab_center(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)> {
        self.tab_center(bar, tab_idx)
    }
}

/// Advances `driver` by one step: dispatches the oldest queued event, or —
/// if none is queued — pumps any posted user events and then ticks. Returns
/// whether the app is still running (`false` once `driver.exited()`). The
/// "drain the queue, else pump-and-tick" order every binding's own
/// `step`/`step_once` surface needs identically; the one thing that differs
/// between a headless app and a shell app — where a pending callback error
/// is recorded, and in what shape it is re-raised — stays with the caller.
pub fn step_driver(
    driver: &mut dyn ErasedShellDriver,
    pending_events: &mut VecDeque<UiEvent>,
) -> bool {
    if driver.exited() {
        return false;
    }
    match pending_events.pop_front() {
        Some(event) => {
            driver.dispatch(event);
        }
        None => {
            driver.pump_user_events();
            if !driver.exited() {
                driver.tick();
            }
        }
    }
    !driver.exited()
}

/// A synthetic `KeyPressed` event for the character `ch`, no modifiers
/// held, not a repeat — the event shape behind a binding's own
/// `push_key_event` convenience.
pub fn key_pressed_event(ch: char) -> UiEvent {
    UiEvent::KeyPressed {
        key: Key::Char(ch),
        modifiers: Modifiers::default(),
        repeat: false,
    }
}

/// A synthetic `MouseDown` + `MouseUp` left-click at `(x, y)` — the event
/// pair behind a binding's own `push_click` convenience.
pub fn click_down_up_events(x: f32, y: f32) -> [UiEvent; 2] {
    [
        mouse_down(MouseButton::Left, x, y, Modifiers::default()),
        mouse_up(MouseButton::Left, x, y),
    ]
}

/// Downcasts a caught panic payload to a human-readable message — shared by
/// every binding's own panic-at-the-boundary guard (mirrors
/// `quadraui-ffi`'s `record_panic`).
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

/// Why [`guarded`] returned early with an error instead of `body`'s own
/// result — the one piece of information `to_err` needs to build that
/// binding's own exception type (`PoisonedError`/`PanicError` in
/// `quadraui-py`, their napi-rs equivalents in `quadraui-node`).
pub enum GuardedFailure {
    /// `state` was already poisoned by an earlier caught panic; `body`
    /// never ran.
    Poisoned,
    /// `body` panicked; [`panic_message`]'s text for that panic.
    Panic(String),
}

/// Runs `body` on `state`, catching a Rust panic instead of letting it
/// unwind across the binding's own FFI boundary (PyO3, napi-rs, ...) and
/// poisoning `state` on one — the "catch panic, poison on panic, refuse
/// every call once poisoned" shape every binding's own `QuiApp::guarded`/
/// `QuiShellApp::guarded` pair repeats (`quadraui-py`'s and
/// `quadraui-node`'s alike), parameterized over that binding's own
/// `state` type `S`, its own error type `E` (`PyErr`, napi-rs's `Error`,
/// ...), and the small pieces that differ per call site: how `S` itself
/// stores its poison flag (`is_poisoned`/`set_poisoned` — a plain `bool`
/// field for every call site today, but this takes closures rather than
/// assuming that shape so a future call site backed by a `Cell`/
/// `AtomicBool` still fits), and how to build `E` from a [`GuardedFailure`]
/// (a binding's own exception type, e.g.
/// `PoisonedError::new_err(..)`/`PanicError::new_err(..)`).
///
/// `before`/`after` run immediately before/after the guarded `body` call —
/// empty no-ops for a binding with nothing else to do there
/// (`quadraui-py`'s `QuiApp`/`QuiShellApp`), or napi-rs's own
/// "stash the live `Env`, flip a re-entrancy flag, run `body`, flip the
/// flag back" shape (`quadraui-node`'s `QuiApp`/`QuiShellApp`) — `after`
/// still runs even when `body` panics, since `before`/`after` bracket the
/// `catch_unwind` call itself rather than running inside it. A binding's
/// own re-entrancy check (`check_not_dispatching`-style) runs before this
/// function is even called, since it must reject *without* touching the
/// poison flag or running `before` at all.
///
/// On success, returns `body`'s result unchanged. If `state` is already
/// poisoned, returns `to_err(GuardedFailure::Poisoned)` without calling
/// `before`/`body`/`after` at all. On a caught panic, poisons `state` via
/// `set_poisoned` and returns `to_err(GuardedFailure::Panic(..))`.
pub fn guarded<S, T, E>(
    state: &mut S,
    is_poisoned: impl FnOnce(&S) -> bool,
    set_poisoned: impl FnOnce(&mut S),
    before: impl FnOnce(&mut S),
    after: impl FnOnce(&mut S),
    to_err: impl FnOnce(GuardedFailure) -> E,
    body: impl FnOnce(&mut S) -> Result<T, E>,
) -> Result<T, E> {
    if is_poisoned(state) {
        return Err(to_err(GuardedFailure::Poisoned));
    }
    before(state);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(state)));
    after(state);
    match result {
        Ok(result) => result,
        Err(payload) => {
            set_poisoned(state);
            Err(to_err(GuardedFailure::Panic(panic_message(&*payload))))
        }
    }
}

/// Records `err` into `slot`, but only if no earlier error already
/// claimed it — "first error wins", so that (for example) a handler raise
/// recorded first isn't silently clobbered by a render callback also
/// raising while painting the same frame. The same first-wins contract
/// every binding's own `pending_error`/`ShellState::record_error` already
/// implements by hand, generalized over that binding's own error type
/// `E` (`PyErr`, napi-rs's `Error`, ...) and wherever it stores the slot
/// (a plain field, or the `Option<E>` a `Rc<RefCell<_>>`/`RefMut`
/// dereferences to).
pub fn record_pending_error<E>(slot: &mut Option<E>, err: E) {
    if slot.is_none() {
        *slot = Some(err);
    }
}

/// Takes `slot`'s own pending error, if any, turning it into an `Err` the
/// caller re-raises out of its own `step`/`run` — the `Result`-shaped
/// counterpart of [`record_pending_error`]'s "first error wins" slot,
/// generalized the same way.
pub fn take_pending_error<E>(slot: &mut Option<E>) -> Result<(), E> {
    match slot.take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// A single-line [`TextDisplay`] descriptor per entry of `lines`, under
/// `id` — the shape behind both a binding's own static bottom-panel-tab
/// content and its `render_content`/`set_render_content` callback result.
pub fn text_display_from_lines(id: WidgetId, lines: Vec<String>) -> TextDisplay {
    TextDisplay {
        id,
        lines: lines
            .into_iter()
            .map(|text| TextDisplayLine {
                spans: vec![StyledSpan::plain(text)],
                decoration: Decoration::Normal,
                timestamp: None,
            })
            .collect(),
        scroll_offset: 0,
        auto_scroll: false,
        max_lines: 0,
        has_focus: false,
        title: None,
        show_scrollbar: false,
    }
}

/// An error from [`merge_overrides`] — a binding maps each variant to its
/// own error type (an "invalid input" kind for [`OverrideError::UnknownField`]/
/// [`OverrideError::InvalidValue`]/[`OverrideError::Invalid`], an "internal
/// bug" kind for [`OverrideError::Internal`]).
#[derive(Debug)]
pub enum OverrideError {
    /// Serializing `current` to JSON failed, or its JSON form wasn't an
    /// object — a bug in the caller's own `T`, never the caller's input.
    Internal(String),
    /// An override key names a field `T` doesn't have.
    UnknownField(String),
    /// `value_for` rejected one override's value.
    InvalidValue(String),
    /// The merged value no longer deserializes into `T` — a type mismatch
    /// between an override's value and the field it names.
    Invalid(String),
}

impl std::fmt::Display for OverrideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverrideError::Internal(msg) => write!(f, "{msg}"),
            OverrideError::UnknownField(key) => {
                write!(f, "{key:?} is not a recognized field")
            }
            OverrideError::InvalidValue(msg) | OverrideError::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

/// Merges one override per `keys` entry onto `current`'s own JSON
/// representation, then deserializes the merged value back into `T` — a
/// binding's own `set_theme`/`set_style`-style "override only the fields
/// you name" mechanism, so a caller never has to restate every field just
/// to change one. `value_for` is called with each key only after this
/// function has confirmed `T`'s JSON form actually has that field (so a
/// binding's own per-key value decode never runs for an unrecognized key),
/// and converts that binding's own wire value for `key` into the
/// [`serde_json::Value`] to splice in — see [`theme_color_value`] for the
/// one binding-specific convenience ([`crate`]'s own `Theme` colour fields)
/// built on top of this.
pub fn merge_overrides<T, F>(
    current: T,
    keys: Vec<String>,
    mut value_for: F,
) -> Result<T, OverrideError>
where
    T: Serialize + DeserializeOwned,
    F: FnMut(&str) -> Result<serde_json::Value, String>,
{
    let mut value =
        serde_json::to_value(&current).map_err(|err| OverrideError::Internal(err.to_string()))?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| OverrideError::Internal("expected a JSON object".to_string()))?;
    for key in keys {
        if !obj.contains_key(&key) {
            return Err(OverrideError::UnknownField(key));
        }
        let json = value_for(&key).map_err(OverrideError::InvalidValue)?;
        obj.insert(key, json);
    }
    serde_json::from_value(value).map_err(|err| OverrideError::Invalid(err.to_string()))
}

/// [`merge_overrides`]'s one [`Theme`]-specific convenience: when `value` is
/// a JSON string, treats it as a `"#rrggbb"`/`"#rrggbbaa"` hex colour and
/// converts it to the JSON shape [`quadraui::Color`]'s own `Deserialize`
/// expects, rather than requiring every `Theme` colour override to spell
/// out the `{"r":.., "g":.., "b":.., "a":..}` dict (or `[r, g, b[, a]]`
/// list/tuple) shape [`quadraui::Color`]'s derived `Deserialize` already
/// accepts on its own. Any other JSON shape passes through unchanged.
pub fn theme_color_value(key: &str, value: serde_json::Value) -> Result<serde_json::Value, String> {
    if let serde_json::Value::String(hex) = &value {
        let color = quadraui::Color::from_hex(hex).ok_or_else(|| {
            format!("{key:?}: {hex:?} is not a valid \"#rrggbb\"/\"#rrggbbaa\" hex colour")
        })?;
        return serde_json::to_value(color).map_err(|err| err.to_string());
    }
    Ok(value)
}

/// Snake-cased name for a [`quadraui::ColorDepth`] variant — the vocabulary
/// a binding's own `backend_caps`-style surface reports under
/// `"color_depth"`.
pub fn color_depth_name(depth: quadraui::ColorDepth) -> &'static str {
    match depth {
        quadraui::ColorDepth::Ansi16 => "ansi16",
        quadraui::ColorDepth::Indexed256 => "indexed256",
        quadraui::ColorDepth::TrueColor => "true_color",
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::DialogSeverity`] —
/// the vocabulary a binding's own `show_message_dialog`-style surface
/// accepts for its `severity` argument. `Err` carries the same message a
/// binding's own "unknown severity" error wraps.
pub fn dialog_severity(value: &str) -> Result<quadraui::DialogSeverity, String> {
    match value {
        "info" => Ok(quadraui::DialogSeverity::Info),
        "question" => Ok(quadraui::DialogSeverity::Question),
        "warning" => Ok(quadraui::DialogSeverity::Warning),
        "error" => Ok(quadraui::DialogSeverity::Error),
        other => Err(format!(
            "unknown severity {other:?}; expected one of \"info\", \"question\", \"warning\", \
             \"error\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::ToastCorner`] — the
/// vocabulary a binding's own `show_toast`-style surface accepts for its
/// `corner` argument, the same friendly-lowercase-word shape
/// [`dialog_severity`] decodes `severity` with.
pub fn toast_corner(value: &str) -> Result<quadraui::ToastCorner, String> {
    match value {
        "bottom_right" => Ok(quadraui::ToastCorner::BottomRight),
        "bottom_left" => Ok(quadraui::ToastCorner::BottomLeft),
        "top_right" => Ok(quadraui::ToastCorner::TopRight),
        "top_left" => Ok(quadraui::ToastCorner::TopLeft),
        other => Err(format!(
            "unknown corner {other:?}; expected one of \"bottom_right\", \"bottom_left\", \
             \"top_right\", \"top_left\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::FontRole`] — the
/// vocabulary a binding's own `measure_text`-style surface accepts for its
/// `role` argument.
pub fn font_role(value: &str) -> Result<quadraui::FontRole, String> {
    match value {
        "chrome" => Ok(quadraui::FontRole::Chrome),
        "editor" => Ok(quadraui::FontRole::Editor),
        other => Err(format!(
            "unknown role {other:?}; expected \"chrome\" or \"editor\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::EditorCursorShape`]
/// — the vocabulary a binding's own `set_caret_shape`-style surface accepts.
pub fn caret_shape(value: &str) -> Result<quadraui::EditorCursorShape, String> {
    match value {
        "block" => Ok(quadraui::EditorCursorShape::Block),
        "bar" => Ok(quadraui::EditorCursorShape::Bar),
        "underline" => Ok(quadraui::EditorCursorShape::Underline),
        other => Err(format!(
            "unknown shape {other:?}; expected \"block\", \"bar\", or \"underline\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::ResizeEdge`] — the
/// vocabulary [`pointer_shape`]'s `edge` argument accepts for the `"resize"`
/// shape.
pub fn resize_edge(value: &str) -> Result<quadraui::ResizeEdge, String> {
    match value {
        "north" => Ok(quadraui::ResizeEdge::North),
        "south" => Ok(quadraui::ResizeEdge::South),
        "east" => Ok(quadraui::ResizeEdge::East),
        "west" => Ok(quadraui::ResizeEdge::West),
        "north_east" => Ok(quadraui::ResizeEdge::NorthEast),
        "north_west" => Ok(quadraui::ResizeEdge::NorthWest),
        "south_east" => Ok(quadraui::ResizeEdge::SouthEast),
        "south_west" => Ok(quadraui::ResizeEdge::SouthWest),
        other => Err(format!(
            "unknown edge {other:?}; expected one of \"north\", \"south\", \"east\", \"west\", \
             \"north_east\", \"north_west\", \"south_east\", \"south_west\""
        )),
    }
}

/// Decodes a friendly lowercase word (plus, for `"resize"` only, an `edge`
/// decoded via [`resize_edge`]) into a [`quadraui::PointerShape`] — the
/// vocabulary a binding's own `set_cursor`-style surface accepts. `edge` is
/// required when `value` is `"resize"` and ignored otherwise.
pub fn pointer_shape(value: &str, edge: Option<&str>) -> Result<quadraui::PointerShape, String> {
    match value {
        "default" => Ok(quadraui::PointerShape::Default),
        "resize" => {
            let edge = edge.ok_or_else(|| "shape \"resize\" requires an edge".to_string())?;
            resize_edge(edge).map(quadraui::PointerShape::Resize)
        }
        "text" => Ok(quadraui::PointerShape::Text),
        "pointer" => Ok(quadraui::PointerShape::Pointer),
        "grab" => Ok(quadraui::PointerShape::Grab),
        "grabbing" => Ok(quadraui::PointerShape::Grabbing),
        "crosshair" => Ok(quadraui::PointerShape::Crosshair),
        "wait" => Ok(quadraui::PointerShape::Wait),
        "not_allowed" => Ok(quadraui::PointerShape::NotAllowed),
        "col_resize" => Ok(quadraui::PointerShape::ColResize),
        "row_resize" => Ok(quadraui::PointerShape::RowResize),
        other => Err(format!(
            "unknown shape {other:?}; expected one of \"default\", \"resize\", \"text\", \
             \"pointer\", \"grab\", \"grabbing\", \"crosshair\", \"wait\", \"not_allowed\", \
             \"col_resize\", \"row_resize\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::AppPathKind`] — the
/// vocabulary a binding's own `app_path`/`resolve_app_path`-style surface
/// accepts, the same friendly-lowercase-word shape [`dialog_severity`]
/// decodes `severity` with.
pub fn app_path_kind(value: &str) -> Result<quadraui::AppPathKind, String> {
    match value {
        "user_data" => Ok(quadraui::AppPathKind::UserData),
        "app_config" => Ok(quadraui::AppPathKind::AppConfig),
        "app_cache" => Ok(quadraui::AppPathKind::AppCache),
        "logs" => Ok(quadraui::AppPathKind::Logs),
        "temp" => Ok(quadraui::AppPathKind::Temp),
        "downloads" => Ok(quadraui::AppPathKind::Downloads),
        "documents" => Ok(quadraui::AppPathKind::Documents),
        "home" => Ok(quadraui::AppPathKind::Home),
        other => Err(format!(
            "unknown path kind {other:?}; expected one of \"user_data\", \"app_config\", \
             \"app_cache\", \"logs\", \"temp\", \"downloads\", \"documents\", \"home\""
        )),
    }
}

/// Decodes a friendly lowercase word into a [`quadraui::PowerSaveKind`] —
/// the vocabulary a binding's own `begin_power_save_block`-style surface
/// accepts.
pub fn power_save_kind(value: &str) -> Result<quadraui::PowerSaveKind, String> {
    match value {
        "prevent_display_sleep" => Ok(quadraui::PowerSaveKind::PreventDisplaySleep),
        "prevent_app_suspension" => Ok(quadraui::PowerSaveKind::PreventAppSuspension),
        other => Err(format!(
            "unknown power save kind {other:?}; expected one of \
             \"prevent_display_sleep\", \"prevent_app_suspension\""
        )),
    }
}

/// Applies `f` to `backend`'s [`quadraui::WindowControl`] surface, folding
/// [`Backend::window`] returning `None` (no window at all, or not
/// constructed yet) into the same [`quadraui::BackendError::Unsupported`] a
/// missing method on a real window already reports — the same fold
/// `quadraui/examples/common/window_control_demo.rs`'s `describe` helper
/// uses, so a caller sees one uniform error shape for either case. Shared
/// by every binding's `set_title`/`set_document_edited`/
/// `set_represented_filename` surface rather than copied per leaf.
pub fn with_window<T>(
    backend: &mut dyn Backend,
    f: impl FnOnce(&mut dyn quadraui::WindowControl) -> quadraui::ServiceResult<T>,
) -> quadraui::ServiceResult<T> {
    match backend.window() {
        Some(window) => f(window),
        None => Err(quadraui::BackendError::Unsupported),
    }
}

/// [`with_window`]'s [`quadraui::backend::TrayService`] counterpart — folds
/// [`Backend::tray`] returning `None` into
/// [`quadraui::BackendError::Unsupported`]. Shared by every binding's
/// `set_tray_icon`/`set_tray_tooltip`/`set_tray_menu` surface.
pub fn with_tray<T>(
    backend: &mut dyn Backend,
    f: impl FnOnce(&mut dyn quadraui::backend::TrayService) -> quadraui::ServiceResult<T>,
) -> quadraui::ServiceResult<T> {
    match backend.tray() {
        Some(tray) => f(tray),
        None => Err(quadraui::BackendError::Unsupported),
    }
}

/// Builds a [`quadraui::ContextMenu`] from a flat `items` list for
/// attaching to a non-primitive menu surface — a tray icon's menu or a
/// Dock icon's menu — neither of which has its own on-screen anchor
/// point/selection state the way a right-click [`quadraui::ContextMenu`]
/// does. `id` names the surface (`"qui-tray-menu"`, `"qui-dock-menu"`) so
/// a [`quadraui::UiEvent::ContextMenuItemActivated`] fired from it is
/// still traceable back to its source; `selected_idx` is seeded at the
/// first selectable item the same way a freshly opened right-click menu
/// starts, even though neither surface actually renders a selection
/// highlight today.
pub fn menu_for_surface(
    id: WidgetId,
    items: Vec<quadraui::ContextMenuItem>,
) -> quadraui::ContextMenu {
    let mut menu = quadraui::ContextMenu {
        id,
        items,
        selected_idx: 0,
        bg: None,
        placement: quadraui::ContextMenuPlacement::AnchorPoint,
    };
    menu.selected_idx = menu.first_selectable();
    menu
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use quadraui::backend::MessageDialogOptions;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::tui::TuiBackend;
    use quadraui::{
        AppLogic, Backend, Button, ButtonLayout, ButtonMask, Point, Reaction, Rect, TextInput,
        Toast, ToastSeverity, UiEvent,
    };

    use super::*;
    use crate::engine::EventSink;
    use crate::layout::{LayoutContainer, LayoutLeaf, LayoutNode, LeafDescriptor, Spacing};
    use crate::render::{collect_leaf_tooltips, draw_bands, paint_layout};

    #[derive(Default)]
    struct RecordingSink;

    impl EventSink for RecordingSink {
        fn dispatch(
            &mut self,
            _event: &UiEvent,
            _hit_detail: Option<&crate::hit::HitDetail>,
        ) -> Reaction {
            Reaction::Continue
        }
    }

    /// A minimal [`AppLogic`] whose `render` goes through [`BindingHost::render_refs`]
    /// and [`render_overlays`] exactly the way a real binding's own thin
    /// `AppLogic` leaf would.
    struct HostLogic {
        host: BindingHost,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for HostLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            draw_bands(backend, self.host.render_refs(), &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            render_overlays(&self.engine, backend, hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// [`render_overlays`] writes the anchored-overlay paint order (every
    /// `sync_*`/`render_*` call, dialog before tooltip before toast) down
    /// in one place instead of copy-pasting it per binding render path.
    /// Opens a toast and a message dialog together — two independent,
    /// non-overlapping overlays — and renders through a single
    /// `render_overlays` call, asserting both still show up: the one call
    /// now covers every overlay kind the two former duplicate blocks did.
    #[test]
    fn render_overlays_paints_every_registered_overlay_in_one_call() {
        let logic = HostLogic {
            host: BindingHost {
                button: Some(Button::new(WidgetId::new("go"), "Go")),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let (backend, app) = driver.parts_mut();
        app.engine.show_toast(
            Toast {
                id: WidgetId::new("toast"),
                title: "Saved".to_string(),
                body: String::new(),
                severity: ToastSeverity::Info,
                actions: Vec::new(),
                accent: None,
            },
            quadraui::ToastCorner::BottomRight,
        );
        app.engine.open_message_dialog(
            MessageDialogOptions {
                title: "Heads up".to_string(),
                body: "Something happened.".to_string(),
                buttons: Vec::new(),
                severity: None,
            },
            backend,
        );

        driver.render();

        assert!(
            driver.screen_contains("Heads up"),
            "the message dialog must still paint"
        );
        assert!(
            driver.screen_contains("Saved"),
            "the toast opened alongside the dialog must still paint through \
             the same render_overlays call"
        );
    }

    /// An [`AppLogic`] whose `render` paints a focused, bordered
    /// [`TextInput`] across the whole viewport directly (not through a
    /// [`BindingHost`] slot — `TextInput` is a `"text_input"` additive
    /// extra, and this test only needs its border painted, not its full
    /// extra-slot wiring) before calling [`render_overlays`], so a toast
    /// opened on top of it has to paint over real bordered chrome, not
    /// just empty space.
    struct BorderedEditorLogic {
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for BorderedEditorLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut editor = TextInput::new(WidgetId::new("editor"));
            editor.lines = vec!["fn main() {}".to_string()];
            editor.has_focus = true;
            backend.draw_text_input(rect, &editor);
            render_overlays(&self.engine, backend, Vec::new());
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            Vec::new()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// The paint-order contract [`render_overlays`]'s own doc states —
    /// "dialog > float > menus > toast > tooltip" at the module level,
    /// toast last of all in this function's own body — must hold even
    /// when the base layer underneath is a *bordered, focused* editor,
    /// not empty space: a toast opened over it must still show its own
    /// text, not get painted over by the editor's own border/background.
    #[test]
    fn toast_paints_over_a_bordered_focused_editor() {
        let logic = BorderedEditorLogic {
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let (_backend, app) = driver.parts_mut();
        app.engine.show_toast(
            Toast {
                id: WidgetId::new("toast"),
                title: "Saved".to_string(),
                body: String::new(),
                severity: ToastSeverity::Info,
                actions: Vec::new(),
                accent: None,
            },
            quadraui::ToastCorner::BottomRight,
        );

        driver.render();

        assert!(
            driver.screen_contains("fn main"),
            "the editor's own content must still paint"
        );
        assert!(
            driver.screen_contains("Saved"),
            "the toast must paint over the bordered editor underneath it, \
             not get overwritten by its border/background"
        );
    }

    /// The paint-order contract [`render_overlays`]'s own doc states must
    /// hold for a tooltip too, not just a toast: shown over a bordered,
    /// focused editor, its own text must paint over that chrome — above
    /// the editor's focus ring and border, not underneath it.
    #[test]
    fn tooltip_paints_over_a_bordered_focused_editor() {
        let logic = BorderedEditorLogic {
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let (_backend, app) = driver.parts_mut();
        app.engine.show_tooltip(
            Tooltip::new(WidgetId::new("tip"), "Hint"),
            Rect::new(2.0, 2.0, 10.0, 1.0),
        );

        driver.render();

        assert!(
            driver.screen_contains("fn main"),
            "the editor's own content must still paint"
        );
        assert!(
            driver.screen_contains("Hint"),
            "the tooltip must paint over the bordered editor underneath it, \
             not get overwritten by its border/focus-ring chrome"
        );
    }

    /// An [`AppLogic`] wiring [`BindingHost::drive_hover_tooltip`] into
    /// real `MouseMoved` dispatch + `tick` — the shared-host equivalent of
    /// a native backend's own `hover_driven` test
    /// (`quadraui::macos::tooltip::tests::hover_driven`), proving
    /// `quadraui-binding-core` drives [`HoverTooltipTracker`] through a
    /// real [`BindingHost`]/[`InteractionEngine`] pair instead of each
    /// binding (and each binding's own test) reimplementing the wiring.
    /// `clock_ms` is a caller-driven millisecond counter, not a wall
    /// clock — the tracker's own "headless-clock contract" — so the test
    /// controls elapsed time exactly with no sleeping.
    ///
    /// Generic over the [`EventSink`] `S`: most tests use [`RecordingSink`]
    /// (always `Continue`), but
    /// [`drive_hover_tooltip_landing_frame_still_schedules_the_pending_wake_even_when_the_frame_also_redraws`]
    /// needs a sink that mimics a real binding's handler returning `Redraw`
    /// for the very `MouseEntered` that arms the pending tooltip — the
    /// exact same-frame collision [`BindingHost::drive_hover_tooltip`]'s
    /// own doc names.
    struct HoverHostLogic<S: EventSink> {
        host: BindingHost,
        engine: InteractionEngine,
        sink: RefCell<S>,
        clock_ms: Rc<Cell<u64>>,
        delay_ms: u64,
        auto_dismiss_ms: u64,
    }

    impl<S: EventSink> AppLogic for HoverHostLogic<S> {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            draw_bands(backend, self.host.render_refs(), &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            render_overlays(&self.engine, backend, hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            let now = self.clock_ms.get();
            let reaction = self
                .engine
                .handle(event, backend, &mut *self.sink.borrow_mut());
            reaction.merge(self.host.drive_hover_tooltip(
                &mut self.engine,
                backend,
                now,
                self.delay_ms,
                self.auto_dismiss_ms,
            ))
        }

        fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
            let now = self.clock_ms.get();
            self.host.drive_hover_tooltip(
                &mut self.engine,
                backend,
                now,
                self.delay_ms,
                self.auto_dismiss_ms,
            )
        }
    }

    /// Drives [`BindingHost::drive_hover_tooltip`] through real
    /// `MouseMoved`/`tick` dispatch with a headless clock: the tooltip
    /// must not appear the instant the pointer lands, must appear once
    /// [`HoverHostLogic::delay_ms`] has elapsed with no further movement,
    /// and must hide again the moment the pointer leaves — the same
    /// three assertions a native backend's own `hover_driven` test makes,
    /// here proving `quadraui-binding-core` does the identical job for a
    /// `Button::tooltip` reached through [`BindingHost`].
    #[test]
    fn drive_hover_tooltip_shows_after_the_delay_and_hides_on_pointer_leave() {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let logic = HoverHostLogic {
            host: BindingHost {
                button: Some(button),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            auto_dismiss_ms: 0,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the button's own label must be painted");

        // The pointer lands on the button -- still pending, no tooltip yet.
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the tooltip must not show the instant the pointer lands"
        );

        // The delay elapses with no further pointer movement -- the
        // `tick()` a real runner's `RedrawAfter`-scheduled wake would have
        // triggered now shows the tooltip.
        clock.set(DELAY_MS);
        driver.tick();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must show once the hover delay has elapsed"
        );

        // The pointer leaves the button entirely -- the tooltip must
        // disappear immediately, no further delay.
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(39.0, 19.0),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the tooltip must hide as soon as the pointer leaves its anchor"
        );
    }

    /// A click anywhere dismisses a visible tooltip through
    /// [`InteractionEngine::handle`]'s own generic auto-dismiss, which
    /// [`BindingHost::tooltip_tracker`] has no way to observe on its own
    /// (it only ever sees `on_hover`/`tick`, not engine-internal state).
    /// Without [`BindingHost::drive_hover_tooltip`]'s resync, the tracker
    /// would be stuck believing a tooltip is still visible for the rest
    /// of this continuous hover and never re-arm; with it, holding the
    /// pointer still over the same button past a further delay re-shows
    /// the tooltip exactly as if the hover had restarted.
    #[test]
    fn drive_hover_tooltip_resyncs_after_a_click_dismisses_the_tooltip() {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let logic = HoverHostLogic {
            host: BindingHost {
                button: Some(button),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            auto_dismiss_ms: 0,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the button's own label must be painted");

        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        clock.set(DELAY_MS);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must be showing before the click"
        );

        // A click on the button itself (still hovering it) dismisses the
        // tooltip via `InteractionEngine::handle`'s own generic auto-dismiss.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: quadraui::MouseButton::Left,
            position: Point::new(x, y),
            modifiers: quadraui::Modifiers::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the click must dismiss the tooltip immediately"
        );

        // Still hovering the same button, unmoved, well past another full
        // delay -- without the resync, the tracker would still believe
        // the old (now-closed) tooltip is visible and never re-arm, so
        // this would stay hidden forever.
        clock.set(DELAY_MS * 10);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "holding the pointer still over the same button after the \
             click dismissed its tooltip must eventually re-show it, not \
             stay stuck believing a tooltip is open that the screen no \
             longer has"
        );
    }

    /// A tooltip held visible past [`HoverHostLogic::auto_dismiss_ms`] with
    /// the pointer never moving off it must still hide itself —
    /// [`HoverTooltipTracker::tick_auto_dismiss`]'s own third dismissal
    /// path, wired through [`BindingHost::drive_hover_tooltip`] via
    /// [`quadraui::Style::tooltip_auto_dismiss_ms`] — and, with the
    /// pointer still held on the exact same anchor, must *stay* hidden
    /// rather than re-arming the delay and showing (then auto-dismissing)
    /// the same tooltip again every `delay_ms + auto_dismiss_ms`
    /// indefinitely.
    #[test]
    fn drive_hover_tooltip_auto_dismisses_after_the_timeout_with_the_pointer_held_still() {
        const DELAY_MS: u64 = 500;
        const AUTO_DISMISS_MS: u64 = 1_000;
        let clock = Rc::new(Cell::new(0u64));
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let logic = HoverHostLogic {
            host: BindingHost {
                button: Some(button),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            auto_dismiss_ms: AUTO_DISMISS_MS,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the button's own label must be painted");

        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        clock.set(DELAY_MS);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must be showing before the auto-dismiss timeout"
        );

        // Held still, well short of the auto-dismiss deadline -- stays shown.
        clock.set(DELAY_MS + AUTO_DISMISS_MS - 1);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must not hide before its own timeout elapses"
        );

        // The pointer never moved, but the auto-dismiss deadline has now
        // passed -- the tooltip must hide itself with no further input.
        clock.set(DELAY_MS + AUTO_DISMISS_MS);
        driver.tick();
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "a tooltip left open past tooltip_auto_dismiss_ms must hide \
             itself even though the pointer never left its anchor"
        );

        // Several more delay-plus-dismiss cycles pass with the pointer
        // still held on the very same button -- without a latch this
        // would re-show (then auto-dismiss) the tooltip every cycle
        // forever. It must stay hidden instead.
        for n in 1..=3u64 {
            clock.set(DELAY_MS + n * (DELAY_MS + AUTO_DISMISS_MS));
            driver.tick();
            driver.render();
            assert!(
                !driver.screen_contains("Save the file"),
                "cycle {n}: an auto-dismissed tooltip must not blink back \
                 on while the pointer keeps hovering the same anchor"
            );
        }

        // The pointer finally leaves and comes back -- the delay is free
        // to start over for a genuinely new hover.
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(20.0, 10.0),
            buttons: ButtonMask::default(),
        });
        let resume_at = clock.get() + 1;
        clock.set(resume_at);
        driver.tick();
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "leaving the anchor must not instantly show a new tooltip \
             either -- the delay still applies"
        );

        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        clock.set(resume_at + DELAY_MS);
        driver.tick();
        driver.render();
        assert!(
            driver.screen_contains("Save the file"),
            "a fresh hover after leaving the anchor must show the tooltip \
             again once its own delay elapses"
        );
    }

    /// Mimics a real binding's registered handler: returns [`Reaction::Redraw`]
    /// for every `MouseEntered`/`MouseLeft` [`InteractionEngine::handle`]
    /// synthesizes, the same way a Python/JS callback that repaints its own
    /// hover-highlight state would — unlike [`RecordingSink`], which never
    /// does, and so never reproduces the same-frame collision this module's
    /// own test below exercises.
    #[derive(Default)]
    struct RedrawOnHoverChangeSink;

    impl EventSink for RedrawOnHoverChangeSink {
        fn dispatch(
            &mut self,
            event: &UiEvent,
            _hit_detail: Option<&crate::hit::HitDetail>,
        ) -> Reaction {
            match event {
                UiEvent::MouseEntered { .. } | UiEvent::MouseLeft { .. } => Reaction::Redraw,
                _ => Reaction::Continue,
            }
        }
    }

    /// Proves the fix for the exact collision [`BindingHost::drive_hover_tooltip`]'s
    /// own doc names: the pointer landing on a button both arms the
    /// pending-tooltip delay *and* triggers an unrelated `Redraw` (here,
    /// [`RedrawOnHoverChangeSink`]'s hover-highlight repaint) on the very
    /// same dispatch. Before the fix, [`Reaction::merge`] let that `Redraw`
    /// swallow the pending delay's own `RedrawAfter`, so the tooltip only
    /// ever showed once the backend's much coarser idle-poll ceiling
    /// happened to notice. Scheduling the wake directly on `backend` (not
    /// only through the returned [`Reaction`]) survives that merge.
    #[test]
    fn drive_hover_tooltip_landing_frame_still_schedules_the_pending_wake_even_when_the_frame_also_redraws(
    ) {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let logic = HoverHostLogic {
            host: BindingHost {
                button: Some(button),
                ..BindingHost::default()
            },
            engine: InteractionEngine::default(),
            sink: RefCell::new(RedrawOnHoverChangeSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            auto_dismiss_ms: 0,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the button's own label must be painted");

        let reaction = driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        assert_eq!(
            reaction,
            Reaction::Redraw,
            "the landing frame's own hover-highlight repaint must still \
             return Redraw -- that's the collision being tested, not an \
             incidental setup detail"
        );
        assert!(
            driver.backend().frame_requests() > 0,
            "the pending tooltip's wake must be scheduled directly on the \
             backend even though this frame's merged Reaction is Redraw, \
             not Continue/RedrawAfter"
        );
        let armed = driver
            .backend()
            .pending_frame_delay()
            .expect("a wake must be armed for the still-pending tooltip");
        assert!(
            armed <= Duration::from_millis(DELAY_MS),
            "the armed wake must be no later than the remaining hover \
             delay, got {armed:?}"
        );
    }

    /// A hovered id with tooltip text but no [`PaintedHit::Button`] anchor
    /// in this frame's hit map (a layout race, a mid-resize frame, or any
    /// other reason `render` hasn't repainted it yet) must never reach
    /// [`HoverTooltipTracker::visible_item`] — and, critically, repeated
    /// calls must not cycle through pending/visible-unpainted/dismissed
    /// forever for a pointer that never moved. Drives
    /// [`BindingHost::drive_hover_tooltip`] directly against a hand-built
    /// [`InteractionEngine`] rather than through [`HoverHostLogic`], so the
    /// anchor can be removed independently of the hover itself (which only
    /// [`UiEvent::MouseMoved`] re-dispatch would otherwise change).
    #[test]
    fn drive_hover_tooltip_never_reaches_visible_without_a_painted_anchor_and_does_not_oscillate() {
        const DELAY_MS: u64 = 500;
        let mut button = Button::new(WidgetId::new("save"), "Save");
        button.tooltip = Some("Save the file".to_string());
        let mut host = BindingHost {
            button: Some(button),
            ..BindingHost::default()
        };
        let mut engine = InteractionEngine::default();
        let mut backend = TuiBackend::new();
        let mut sink = RecordingSink;

        let id = WidgetId::new("save");
        engine.set_hits(vec![PaintedHit::Button {
            id,
            layout: ButtonLayout {
                bounds: Rect::new(0.0, 0.0, 10.0, 1.0),
            },
        }]);
        engine.handle(
            UiEvent::MouseMoved {
                position: Point::new(1.0, 0.0),
                buttons: ButtonMask::default(),
            },
            &mut backend,
            &mut sink,
        );
        assert_eq!(
            engine.hovered().map(|(hovered_id, _)| hovered_id),
            Some(id),
            "the hover must land on the button while it's actually painted"
        );

        // The button's own paint vanishes from this frame's hit map while
        // `engine`'s cached hover still points at it.
        engine.set_hits(vec![]);

        for n in 0..5u64 {
            // Each call is well past the hover delay, so a painted anchor
            // would have let `tick` flip to `Visible` on the very first
            // one -- proving the guard, not just that not enough time
            // passed.
            let now_ms = DELAY_MS + n * DELAY_MS;
            let reaction = host.drive_hover_tooltip(&mut engine, &mut backend, now_ms, DELAY_MS, 0);
            assert_eq!(
                host.tooltip_tracker.visible_item(),
                None,
                "call {n}: a hovered id with no painted anchor must never \
                 show a tooltip"
            );
            assert!(
                host.tooltip_tracker.is_pending(),
                "call {n}: without an anchor the tracker must stay pending, \
                 not cycle through dismiss/re-arm"
            );
            assert_ne!(
                reaction,
                Reaction::Redraw,
                "call {n}: nothing actually changed on screen, so this must \
                 not force a redraw"
            );
            assert_eq!(
                backend.pending_frame_delay(),
                None,
                "call {n}: without an anchor, `next_wake_ms`'s Pending \
                 deadline has already elapsed, so arming a wake for it \
                 would be a zero-delay busy-spin re-armed every dispatch \
                 for as long as the hover stays stale -- the call must \
                 skip the wake entirely instead"
            );
        }
    }

    /// A minimal [`AppLogic`] whose `render` paints a layout tree through
    /// [`crate::render::paint_layout`] -- not [`BindingHost::render_refs`]'s
    /// six fixed slots -- and refreshes [`BindingHost::leaf_tooltips`] from
    /// it every frame via [`crate::render::collect_leaf_tooltips`], the
    /// same two calls `quadraui-py`'s own `PyAppLogic::render` makes for
    /// its `RenderOutput::Tree` path.
    struct TreeHoverHostLogic<S: EventSink> {
        host: BindingHost,
        engine: InteractionEngine,
        sink: RefCell<S>,
        clock_ms: Rc<Cell<u64>>,
        delay_ms: u64,
        tree: LayoutNode,
    }

    impl<S: EventSink> AppLogic for TreeHoverHostLogic<S> {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            paint_layout(backend, area, &self.tree, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            let mut leaf_tooltips = HashMap::new();
            collect_leaf_tooltips(&self.tree, &mut leaf_tooltips);
            self.host.set_leaf_tooltips(leaf_tooltips);
            render_overlays(&self.engine, backend, hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            let now = self.clock_ms.get();
            let reaction = self
                .engine
                .handle(event, backend, &mut *self.sink.borrow_mut());
            reaction.merge(self.host.drive_hover_tooltip(
                &mut self.engine,
                backend,
                now,
                self.delay_ms,
                0,
            ))
        }

        fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
            let now = self.clock_ms.get();
            self.host
                .drive_hover_tooltip(&mut self.engine, backend, now, self.delay_ms, 0)
        }
    }

    /// A `Button` leaf reached through a layout tree (not
    /// [`BindingHost::button`]'s own static slot) still shows its own
    /// `Button::tooltip` once hovered continuously for `delay_ms` --
    /// anchored to that exact leaf's own painted bounds, since the second
    /// leaf alongside it never arms a tooltip at all (its own `tooltip` is
    /// `None`) -- and hides the instant the pointer moves onto that second,
    /// tooltip-less leaf.
    #[test]
    fn drive_hover_tooltip_shows_a_layout_tree_leaf_tooltip_after_delay_anchored_to_its_bounds_and_hides_on_leave(
    ) {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let mut save_button = Button::new(WidgetId::new("save"), "Save");
        save_button.tooltip = Some("Save the file".to_string());
        let other_button = Button::new(WidgetId::new("other"), "Other");
        let tree = LayoutNode::Row(LayoutContainer {
            children: vec![
                LayoutNode::Leaf(LayoutLeaf {
                    id: "leaf:save".to_string(),
                    descriptor: LeafDescriptor::Button(save_button),
                    scroll_px: None,
                }),
                LayoutNode::Leaf(LayoutLeaf {
                    id: "leaf:other".to_string(),
                    descriptor: LeafDescriptor::Button(other_button),
                    scroll_px: None,
                }),
            ],
            spacing: Spacing::default(),
        });
        let logic = TreeHoverHostLogic {
            host: BindingHost::default(),
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            tree,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();
        let (x, y) = driver
            .find("Save")
            .expect("the leaf's own label must be painted");

        // The pointer lands on the leaf -- still pending, no tooltip yet.
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the tooltip must not show the instant the pointer lands"
        );

        // The delay elapses with no further pointer movement.
        clock.set(DELAY_MS);
        driver.tick();
        assert!(
            driver.screen_contains("Save the file"),
            "the tooltip must show once the hover delay has elapsed, \
             resolved by this leaf's own tree-address id even though it \
             was painted through `paint_layout`, not `BindingHost::button`"
        );

        // The pointer moves onto the second leaf, which carries no
        // tooltip at all -- the first leaf's tooltip must hide
        // immediately, proving the shown tooltip was anchored to the
        // first leaf's own painted bounds rather than some global state.
        let (ox, oy) = driver
            .find("Other")
            .expect("the second leaf's own label must be painted");
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(ox, oy),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Save the file"),
            "the tooltip must hide as soon as the pointer leaves its anchor"
        );
    }

    /// Two layout-tree leaves whose `Button` descriptors share the exact
    /// same `Button::id` ("shared") still resolve their own, independent
    /// tooltip text -- [`BindingHost::leaf_tooltips`] is keyed by each
    /// leaf's own tree-address id (`"leaf:alpha"`/`"leaf:bravo"`), not the
    /// descriptor's embedded (here, duplicated) `Button::id`.
    #[test]
    fn drive_hover_tooltip_resolves_each_leaf_independently_when_descriptors_share_an_id() {
        const DELAY_MS: u64 = 500;
        let clock = Rc::new(Cell::new(0u64));
        let shared_id = WidgetId::new("shared");
        let mut alpha = Button::new(shared_id, "Alpha");
        alpha.tooltip = Some("Tooltip for Alpha".to_string());
        let mut bravo = Button::new(shared_id, "Bravo");
        bravo.tooltip = Some("Tooltip for Bravo".to_string());
        let tree = LayoutNode::Row(LayoutContainer {
            children: vec![
                LayoutNode::Leaf(LayoutLeaf {
                    id: "leaf:alpha".to_string(),
                    descriptor: LeafDescriptor::Button(alpha),
                    scroll_px: None,
                }),
                LayoutNode::Leaf(LayoutLeaf {
                    id: "leaf:bravo".to_string(),
                    descriptor: LeafDescriptor::Button(bravo),
                    scroll_px: None,
                }),
            ],
            spacing: Spacing::default(),
        });
        let logic = TreeHoverHostLogic {
            host: BindingHost::default(),
            engine: InteractionEngine::default(),
            sink: RefCell::new(RecordingSink),
            clock_ms: clock.clone(),
            delay_ms: DELAY_MS,
            tree,
        };
        let mut driver = TuiDriver::new(logic, 40, 20);
        driver.render();

        let (ax, ay) = driver
            .find("Alpha")
            .expect("the first leaf's own label must be painted");
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(ax, ay),
            buttons: ButtonMask::default(),
        });
        clock.set(DELAY_MS);
        driver.tick();
        assert!(
            driver.screen_contains("Tooltip for Alpha"),
            "the first leaf's own tooltip must show"
        );
        assert!(
            !driver.screen_contains("Tooltip for Bravo"),
            "the second leaf's tooltip must not show while hovering the first"
        );

        let (bx, by) = driver
            .find("Bravo")
            .expect("the second leaf's own label must be painted");
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(bx, by),
            buttons: ButtonMask::default(),
        });
        driver.render();
        assert!(
            !driver.screen_contains("Tooltip for Alpha"),
            "moving onto the second leaf must hide the first leaf's tooltip immediately"
        );

        clock.set(2 * DELAY_MS);
        driver.tick();
        assert!(
            driver.screen_contains("Tooltip for Bravo"),
            "the second leaf's own tooltip must show once its own delay has elapsed"
        );
        assert!(
            !driver.screen_contains("Tooltip for Alpha"),
            "the first leaf's tooltip must stay hidden"
        );
    }

    /// Minimal stand-in for a binding's own `QuiApp`/`QuiShellApp`: just a
    /// poison flag and a counter [`guarded`]'s `before`/`after` hooks can
    /// record calls into, proving both run around `body` (even when it
    /// panics) and neither runs once poisoned.
    #[derive(Default)]
    struct FakeApp {
        poisoned: bool,
        before_after_calls: u32,
    }

    /// [`GuardedFailure`] -> `String` for every test below: `"poisoned"`
    /// for [`GuardedFailure::Poisoned`], `"panic: <msg>"` for
    /// [`GuardedFailure::Panic`].
    fn fake_to_err(failure: GuardedFailure) -> String {
        match failure {
            GuardedFailure::Poisoned => "poisoned".to_string(),
            GuardedFailure::Panic(msg) => format!("panic: {msg}"),
        }
    }

    /// A successful `body` call returns its own result, with `state`
    /// untouched (not poisoned) and both hooks having run exactly once.
    #[test]
    fn guarded_runs_before_and_after_around_a_successful_body() {
        let mut app = FakeApp::default();
        let result: Result<i32, String> = guarded(
            &mut app,
            |s| s.poisoned,
            |s| s.poisoned = true,
            |s| s.before_after_calls += 1,
            |s| s.before_after_calls += 1,
            fake_to_err,
            |_s| Ok(42),
        );
        assert_eq!(result, Ok(42));
        assert!(!app.poisoned);
        assert_eq!(app.before_after_calls, 2);
    }

    /// A `body` that panics is caught, not propagated: [`guarded`] returns
    /// `to_err`'s own `Err` and poisons `state`. `after` still runs (it
    /// brackets the `catch_unwind` call, not the inside of it), but
    /// `before` only ran once, for this one `body` call.
    #[test]
    fn guarded_catches_a_panic_and_poisons() {
        let mut app = FakeApp::default();
        let result: Result<i32, String> = guarded(
            &mut app,
            |s| s.poisoned,
            |s| s.poisoned = true,
            |s| s.before_after_calls += 1,
            |s| s.before_after_calls += 1,
            fake_to_err,
            |_s| -> Result<i32, String> { panic!("boom") },
        );
        assert_eq!(result, Err("panic: boom".to_string()));
        assert!(app.poisoned);
        assert_eq!(app.before_after_calls, 2);
    }

    /// Once `state` is poisoned, a further [`guarded`] call rejects
    /// immediately via `to_err(GuardedFailure::Poisoned)`, running neither
    /// `before`/`after` nor `body` at all.
    #[test]
    fn guarded_rejects_without_running_body_once_poisoned() {
        let mut app = FakeApp {
            poisoned: true,
            before_after_calls: 0,
        };
        let result: Result<i32, String> = guarded(
            &mut app,
            |s| s.poisoned,
            |s| s.poisoned = true,
            |s| s.before_after_calls += 1,
            |s| s.before_after_calls += 1,
            fake_to_err,
            |_s| -> Result<i32, String> { panic!("must not run") },
        );
        assert_eq!(result, Err("poisoned".to_string()));
        assert_eq!(app.before_after_calls, 0);
    }

    /// [`record_pending_error`]'s "first error wins": a second recorded
    /// error never clobbers the first, the same contract every binding's
    /// own hand-written `record_error` implements.
    #[test]
    fn record_pending_error_keeps_the_first_error() {
        let mut slot: Option<String> = None;
        record_pending_error(&mut slot, "first".to_string());
        record_pending_error(&mut slot, "second".to_string());
        assert_eq!(slot, Some("first".to_string()));
    }

    /// [`take_pending_error`] turns a recorded error into an `Err` and
    /// empties the slot; with nothing recorded, it's a no-op `Ok(())`.
    #[test]
    fn take_pending_error_drains_the_slot_once() {
        let mut slot: Option<String> = None;
        assert_eq!(take_pending_error(&mut slot), Ok(()));

        record_pending_error(&mut slot, "boom".to_string());
        assert_eq!(take_pending_error(&mut slot), Err("boom".to_string()));
        assert_eq!(take_pending_error(&mut slot), Ok(()));
    }

    /// [`BindingHost::set_test_clock_ms`] pins [`BindingHost::now_ms`] to
    /// an exact value, repeatedly, with no real time elapsing between
    /// calls -- the seam a Python/Node test drives instead of a real
    /// sleep. Clearing the override with `None` falls back to the real
    /// wall clock, which is monotonic and non-zero once started.
    #[test]
    fn set_test_clock_ms_overrides_now_ms_until_cleared() {
        let host = BindingHost::default();

        host.set_test_clock_ms(Some(1_000));
        assert_eq!(host.now_ms(), 1_000);
        // Reading it again doesn't drift forward on its own, unlike the
        // real wall clock -- it stays pinned until explicitly re-set.
        assert_eq!(host.now_ms(), 1_000);

        host.set_test_clock_ms(Some(5_000));
        assert_eq!(host.now_ms(), 5_000);

        host.set_test_clock_ms(None);
        assert!(host.now_ms() < 5_000);
    }

    // ─── Tree/List/DataTable cursor + scroll ───────────────────────────

    use quadraui::{Column, DataRow, ListItem, NamedKey, SelectionMode, TreeStyle};

    fn down_key() -> Key {
        Key::Named(NamedKey::Down)
    }

    fn home_key() -> Key {
        Key::Named(NamedKey::Home)
    }

    fn end_key() -> Key {
        Key::Named(NamedKey::End)
    }

    fn test_tree(
        row_count: usize,
        selected_path: Option<TreePath>,
        scroll_offset: usize,
    ) -> TreeView {
        TreeView {
            id: WidgetId::new("host-test:tree"),
            rows: (0..row_count)
                .map(|i| placeholder_tree_row(vec![i as u16]))
                .collect(),
            selection_mode: SelectionMode::Single,
            selected_path,
            scroll_offset,
            style: TreeStyle::default(),
            has_focus: true,
        }
    }

    fn test_list(row_count: usize, selected_idx: usize, scroll_offset: usize) -> ListView {
        ListView {
            id: WidgetId::new("host-test:list"),
            title: None,
            items: (0..row_count)
                .map(|i| ListItem {
                    text: StyledText::plain(format!("row {i}")),
                    icon: None,
                    detail: None,
                    decoration: Decoration::default(),
                })
                .collect(),
            selected_idx,
            scroll_offset,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    fn test_data_table(
        row_count: usize,
        selected_idx: Option<usize>,
        scroll_offset: usize,
    ) -> DataTable {
        DataTable {
            id: WidgetId::new("host-test:table"),
            columns: vec![Column {
                title: "Col".to_string(),
                width: Default::default(),
                align: Default::default(),
            }],
            rows: (0..row_count)
                .map(|i| DataRow {
                    cells: vec![StyledText::plain(format!("row {i}"))],
                    decoration: Decoration::default(),
                })
                .collect(),
            selected_idx,
            scroll_offset,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        }
    }

    /// A fully uncontrolled `Tree` (no `selected_path`, `scroll_offset: 0`)
    /// moves its stored cursor one row per `Down` and keeps it within the
    /// scrolled viewport — [`apply_tree_nav_key`]'s own basic contract,
    /// exercised directly (no [`quadraui::tui::testing::TuiDriver`]
    /// needed) rather than only through `crate::engine`'s integration
    /// tests.
    #[test]
    fn apply_tree_nav_key_down_moves_the_cursor_and_scrolls_it_into_view() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let row_paths: Vec<TreePath> = (0..500).map(|i| vec![i as u16]).collect();

        for _ in 0..40 {
            let moved = apply_tree_nav_key(&widget_state, id, &row_paths, 8, &down_key());
            assert!(moved, "every Down from row 0..39 must move the cursor");
        }

        let tree = test_tree(500, None, 0);
        let resolved = resolve_tree_cursor(&widget_state, &tree, 8);
        assert_eq!(
            resolved.selected_path,
            Some(vec![39]),
            "40 Downs starting from no selection must land exactly on row 39"
        );
        assert!(
            (resolved.scroll_offset..resolved.scroll_offset + 8).contains(&39),
            "row 39 must be scrolled into the 8-row viewport: offset {}",
            resolved.scroll_offset
        );
    }

    /// `Home`/`End` jump straight to the first/last row; a further `Down`
    /// past the last row is a no-op that reports no movement, matching
    /// [`apply_tree_nav_key`]'s own "return `false` when nothing moved"
    /// contract.
    #[test]
    fn apply_tree_nav_key_home_and_end_jump_to_the_edges_and_further_movement_past_the_end_is_a_no_op(
    ) {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let row_paths: Vec<TreePath> = (0..10).map(|i| vec![i as u16]).collect();

        assert!(apply_tree_nav_key(
            &widget_state,
            id,
            &row_paths,
            4,
            &end_key()
        ));
        let tree = test_tree(10, None, 0);
        let resolved = resolve_tree_cursor(&widget_state, &tree, 4);
        assert_eq!(resolved.selected_path, Some(vec![9]));

        assert!(
            !apply_tree_nav_key(&widget_state, id, &row_paths, 4, &down_key()),
            "Down past the last row must report no movement"
        );

        assert!(apply_tree_nav_key(
            &widget_state,
            id,
            &row_paths,
            4,
            &home_key()
        ));
        let tree = test_tree(10, None, 0);
        let resolved = resolve_tree_cursor(&widget_state, &tree, 4);
        assert_eq!(resolved.selected_path, Some(vec![0]));
    }

    /// Vim `j`/`k` are recognized aliases for `Down`/`Up`, via the shared
    /// `quadraui::compose::classify_tree_nav_key` key map rather than a
    /// second hand-copied one.
    #[test]
    fn apply_tree_nav_key_recognizes_vim_j_and_k_aliases() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let row_paths: Vec<TreePath> = (0..10).map(|i| vec![i as u16]).collect();

        assert!(apply_tree_nav_key(
            &widget_state,
            id,
            &row_paths,
            4,
            &Key::Char('j')
        ));
        assert!(apply_tree_nav_key(
            &widget_state,
            id,
            &row_paths,
            4,
            &Key::Char('j')
        ));
        assert!(apply_tree_nav_key(
            &widget_state,
            id,
            &row_paths,
            4,
            &Key::Char('k')
        ));

        let tree = test_tree(10, None, 0);
        let resolved = resolve_tree_cursor(&widget_state, &tree, 4);
        assert_eq!(resolved.selected_path, Some(vec![0]));
    }

    /// A controlled `selected_path` wins outright over the stored cursor,
    /// and the viewport re-clamps the scroll on every call even with no
    /// intervening [`apply_tree_nav_key`] — covering the resize case
    /// [`resolve_tree_cursor`]'s own doc describes.
    #[test]
    fn resolve_tree_cursor_controlled_selected_path_wins_and_rescrolls_on_viewport_resize() {
        let widget_state = WidgetState::new();
        let tree = test_tree(50, Some(vec![30]), 0);

        let resolved = resolve_tree_cursor(&widget_state, &tree, 8);
        assert_eq!(resolved.selected_path, Some(vec![30]));
        assert!((resolved.scroll_offset..resolved.scroll_offset + 8).contains(&30));

        // The viewport shrinks (e.g. the window resized) with the same
        // controlled selection and no key pressed in between -- row 30
        // must still be visible in the new, smaller viewport.
        let resolved = resolve_tree_cursor(&widget_state, &tree, 4);
        assert!(
            (resolved.scroll_offset..resolved.scroll_offset + 4).contains(&30),
            "a shrunk viewport must re-clamp the scroll to keep row 30 visible: {}",
            resolved.scroll_offset
        );
    }

    /// A `Tree` pinned at a non-zero `scroll_offset` that later goes back
    /// to `0` must actually scroll back to the top, not get stuck at the
    /// old pinned value — the regression this module's own
    /// `scroll_controlled` latch exists to fix. Before the latch, `0` was
    /// read as "uncontrolled", so `resolve_tree_cursor` fell back to the
    /// stored (still-pinned) offset and the tree could never scroll back
    /// to the top.
    #[test]
    fn resolve_tree_cursor_can_scroll_back_to_zero_after_being_pinned_nonzero() {
        let widget_state = WidgetState::new();

        let pinned = test_tree(50, None, 20);
        let resolved = resolve_tree_cursor(&widget_state, &pinned, 8);
        assert_eq!(resolved.scroll_offset, 20);

        // The app (or the user wheeling back to the top) now pins the
        // scroll at 0 on the very next frame.
        let pinned_at_top = test_tree(50, None, 0);
        let resolved = resolve_tree_cursor(&widget_state, &pinned_at_top, 8);
        assert_eq!(
            resolved.scroll_offset, 0,
            "a controlled tree must be able to scroll back to the top"
        );
    }

    /// A fully uncontrolled `List` (`selected_idx: 0`, `scroll_offset: 0`)
    /// moves its stored cursor one row per `Down` and scrolls it into
    /// view — the `List` counterpart of
    /// `apply_tree_nav_key_down_moves_the_cursor_and_scrolls_it_into_view`.
    #[test]
    fn apply_list_nav_key_down_moves_the_cursor_and_scrolls_it_into_view() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:list");

        for _ in 0..40 {
            assert!(apply_list_nav_key(&widget_state, id, 500, 8, &down_key()));
        }

        let list = test_list(500, 0, 0);
        let resolved = resolve_list_cursor(&widget_state, &list, 8);
        assert_eq!(resolved.selected_idx, 39);
        assert!((resolved.scroll_offset..resolved.scroll_offset + 8).contains(&39));
    }

    /// A `List` whose descriptor pins a non-zero `scroll_offset` keeps
    /// that value even while uncontrolled navigation keys move the
    /// selection — controlled mode wins outright, mirroring
    /// `engine::tests::controlled_tree_scroll_offset_is_pinned_and_not_moved_by_navigation_keys`.
    #[test]
    fn resolve_list_cursor_controlled_scroll_offset_is_pinned() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:list");

        for _ in 0..10 {
            apply_list_nav_key(&widget_state, id, 500, 8, &down_key());
        }

        let list = test_list(500, 0, 50);
        let resolved = resolve_list_cursor(&widget_state, &list, 8);
        assert_eq!(resolved.scroll_offset, 50);
    }

    /// A fully uncontrolled `DataTable` (`selected_idx: None`,
    /// `scroll_offset: 0`) moves its stored cursor one row per `Down` and
    /// scrolls it into view — the `DataTable` counterpart of
    /// `apply_tree_nav_key_down_moves_the_cursor_and_scrolls_it_into_view`.
    #[test]
    fn apply_data_table_nav_key_down_moves_the_cursor_and_scrolls_it_into_view() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:table");

        for _ in 0..40 {
            assert!(apply_data_table_nav_key(
                &widget_state,
                id,
                500,
                8,
                &down_key()
            ));
        }

        let table = test_data_table(500, None, 0);
        let resolved = resolve_data_table_cursor(&widget_state, &table, 8);
        assert_eq!(resolved.selected_idx, Some(39));
        assert!((resolved.scroll_offset..resolved.scroll_offset + 8).contains(&39));
    }

    /// A `DataTable` whose descriptor pins a non-zero `scroll_offset`
    /// keeps that value even while uncontrolled navigation keys move the
    /// selection, the same as [`resolve_list_cursor_controlled_scroll_offset_is_pinned`].
    #[test]
    fn resolve_data_table_cursor_controlled_scroll_offset_is_pinned() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:table");

        for _ in 0..10 {
            apply_data_table_nav_key(&widget_state, id, 500, 8, &down_key());
        }

        let table = test_data_table(500, None, 50);
        let resolved = resolve_data_table_cursor(&widget_state, &table, 8);
        assert_eq!(resolved.scroll_offset, 50);
    }
}
