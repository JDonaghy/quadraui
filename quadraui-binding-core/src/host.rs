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
use std::collections::{HashMap, HashSet, VecDeque};
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
    Decoration, Key, ListView, Modifiers, MouseButton, Reaction, Split, SplitMeasure, Style,
    StyledSpan, StyledText, TabBar, TextDisplay, TextDisplayLine, Theme, Tooltip, TreePath,
    TreeRow, TreeView, UiEvent, WidgetId,
};

use crate::engine::InteractionEngine;
use crate::extra_paint::ExtraPaint;
use crate::hit::PaintedHit;
use crate::layout::{
    quantise_horizontal, quantise_vertical, LayoutLeaf, LayoutNode, LayoutTabs, LeafDescriptor,
};
use crate::render::{
    apply_split_gap, data_table_viewport_rows, layout_container_rects, list_viewport_rows,
    natural_main_length, scroll_content_slots, scroll_viewport_and_track, shrink_main_axis,
    tree_viewport_rows, RenderFrame, RenderRefs, ScrollMetrics, ScrollSlots,
};
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
    /// Deprecated slot/extra-API method names (`"insert_button"`/
    /// `"insertButton"`, ...) this host has already reported deprecated
    /// once -- consulted by [`Self::first_deprecation_report`]. Per-app
    /// (not process-wide) on purpose: a binding's own one-time warning
    /// surface exists so a render loop re-inserting the same slot every
    /// frame (the pattern the slot/extra API forces, since a descriptor is
    /// immutable once inserted) reports it exactly once per *app*, rather
    /// than flooding every call with the same warning -- and a fresh
    /// `QuiApp` in a fresh test case gets a clean slate for free, with no
    /// process-wide state to reset between tests.
    pub warned_deprecations: RefCell<HashSet<String>>,
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

    /// The shared deprecation message for the slot/extra API, naming `what`
    /// (a binding's own idiomatic spelling of the deprecated method --
    /// `"insert_list"` for `quadraui-py`, `"insertList"` for
    /// `quadraui-node`) and `guidance` (a clause naming the inline
    /// replacement, e.g. ``"render an inline `list` layout leaf from a
    /// `render` callback instead"`` -- see [`Self::slot_kind_name`] for the
    /// fixed-slot leaf-kind names a `slot_clear` caller doesn't otherwise
    /// have on hand). Every one of a binding's own
    /// `insert_*`/`store_extra`/`slot_clear`/`clear_extra_slot` leaves calls
    /// this rather than building its own wording, so the shared "deprecated
    /// and scheduled for removal" framing is written down exactly once
    /// instead of copy-pasted per binding.
    pub fn slot_deprecation_message(what: &str, guidance: &str) -> String {
        format!(
            "{what} is deprecated and scheduled for removal in a future \
             breaking release; {guidance}."
        )
    }

    /// [`Self::slot_deprecation_message`] for inserting primitive
    /// `leaf_kind` via the deprecated `what` method -- the one call every
    /// binding's own `insert_*`/`insert_extra` leaf makes, naming the
    /// inline layout leaf of that same kind as the replacement. Shared here
    /// (rather than built up by each binding's own `warn_insert_deprecated`
    /// forward) so the exact wording lives in one place instead of being
    /// copy-pasted per binding. Note this only names the *first* `leaf_kind`
    /// a given app warns about for a dynamic `what` like `insert_extra` --
    /// see [`Self::first_deprecation_report`]'s own doc for why the dedup
    /// key is `what` alone.
    pub fn insert_deprecation_message(what: &str, leaf_kind: &str) -> String {
        Self::slot_deprecation_message(
            what,
            &format!("render an inline `{leaf_kind}` layout leaf from a `render` callback instead"),
        )
    }

    /// [`Self::slot_deprecation_message`] for `clear_extra`/`clearExtra`,
    /// naming omitting `kind`'s layout leaf from the next `render`
    /// callback's returned layout as the replacement.
    pub fn clear_extra_deprecation_message(what: &str, kind: &str) -> String {
        Self::slot_deprecation_message(
            what,
            &format!(
                "omit the `{kind}` layout leaf from the next `render` callback's \
                 returned layout instead of calling `{what}`"
            ),
        )
    }

    /// [`Self::slot_deprecation_message`] for `slot_clear`/`slotClear`,
    /// naming omitting `slot`'s fixed-primitive layout leaf from the next
    /// `render` callback's returned layout as the replacement. Falls back
    /// to the generic `"fixed-slot"` name for an unrecognized `slot` --
    /// see [`Self::slot_kind_name`]'s own doc for when that fallback
    /// actually fires.
    pub fn slot_clear_deprecation_message(what: &str, slot: i64) -> String {
        let kind = Self::slot_kind_name(slot).unwrap_or("fixed-slot");
        Self::slot_deprecation_message(
            what,
            &format!(
                "omit the `{kind}` layout leaf from the next `render` callback's \
                 returned layout instead of calling `{what}`"
            ),
        )
    }

    /// The layout-leaf kind name for fixed slot id `slot` -- `"button"` for
    /// [`SLOT_BUTTON`], `"text_display"` for [`SLOT_TEXT_DISPLAY`], and so
    /// on through [`SLOT_TAB_BAR`] -- or `None` if `slot` doesn't name one
    /// of the six (see [`Self::is_known_slot`]). A binding's own
    /// `slot_clear` leaf uses this to name the inline replacement in its
    /// own [`Self::slot_deprecation_message`] call without needing to track
    /// which primitive it originally inserted into `slot`.
    pub fn slot_kind_name(slot: i64) -> Option<&'static str> {
        match slot {
            SLOT_BUTTON => Some("button"),
            SLOT_TEXT_DISPLAY => Some("text_display"),
            SLOT_LIST => Some("list"),
            SLOT_TREE => Some("tree"),
            SLOT_DATA_TABLE => Some("data_table"),
            SLOT_TAB_BAR => Some("tab_bar"),
            _ => None,
        }
    }

    /// Whether this is the first time this host has reported `what`
    /// deprecated -- `true` only the first call for a given `what` on this
    /// host, `false` on every call after. `quadraui-node`'s own one-time
    /// `process.emitWarning` convention consults this before emitting, so a
    /// render loop calling the same deprecated method every frame reports
    /// it exactly once per app instead of flooding stderr -- see
    /// [`Self::warned_deprecations`]'s own doc for why the dedup is
    /// per-host. `quadraui-py` ignores this and raises a Python
    /// `DeprecationWarning` on every call unconditionally, since that is
    /// already a per-call-site event CPython's own `warnings` filter
    /// machinery decides whether to print.
    pub fn first_deprecation_report(&self, what: &str) -> bool {
        self.warned_deprecations
            .borrow_mut()
            .insert(what.to_string())
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

// ─── Split divider ratio, uncontrolled by default ──────────────────────────
//
// A two-pane `Split`'s divider resolves against `WidgetState` the same
// "controlled value wins and is written back" way `resolve_tree_cursor`'s
// own module doc describes, but `Split::ratio` (a bare `f32`, never
// `Option`) has no structural "unset" sentinel at all — unlike
// `TreeView::scroll_offset`'s `0`, every ratio from `0.0` to `1.0` is
// equally plausible as a deliberately pinned value, so there is no single
// default this module could latch against the way `TreeCursorState::scroll_controlled`
// latches against `0`. Instead, `SplitRatioState` remembers the last ratio
// this module actually SAW the descriptor send (`baseline`), not the last
// ratio it handed back (`resolved`): a frame counts as controlled only when
// its `Split::ratio` differs from that baseline, i.e. the caller itself
// changed what it's sending, never merely because it differs from whatever
// a previous divider drag persisted. An uncontrolled caller that always
// resends the same literal ratio therefore keeps seeing its own dragged
// value, because that literal ratio never stops matching `baseline` — the
// same ping-pong `resolve_tree_cursor`'s own `scroll_controlled` latch
// exists to avoid, solved here by comparing against what was last
// observed rather than a fixed constant.
#[derive(Debug, Clone, Copy)]
struct SplitRatioState {
    baseline: f32,
    resolved: f32,
}

/// The id [`SplitRatioState`] is actually stored under for `split_index`.
/// `split_index` is always `0` for a two-pane [`Split`] — `resolve_split_ratio`
/// looks it up by the split's own `id` with no further addressing, so index
/// `0` keys straight off `tree` unchanged. A `SplitTree`'s N-way divider set
/// can report any index (including `0`, for its own root divider) for a
/// [`quadraui::UiEvent::SplitDividerDragged`] sharing the same event shape,
/// so every OTHER index gets a distinct synthetic id — otherwise dragging
/// divider 3 then divider 1 of the same tree would silently overwrite one
/// shared entry keyed only by `tree`, each drag clobbering the last.
fn split_ratio_state_id(tree: WidgetId, split_index: usize) -> WidgetId {
    if split_index == 0 {
        tree
    } else {
        WidgetId::new(format!(
            "quadraui-binding-core::split-ratio::{}::{split_index}",
            tree.as_str()
        ))
    }
}

/// Persists `new_ratio` for `tree`'s `split_index`'th divider — call when a
/// [`quadraui::UiEvent::SplitDividerDragged`] fires, for a two-pane [`Split`]
/// (`split_index` always `0`) or one divider of a multi-divider `SplitTree`
/// alike; see [`split_ratio_state_id`] for how the two share
/// [`WidgetState`] without colliding. Leaves `baseline` untouched: it
/// tracks what the descriptor last sent, which a drag never changes by
/// itself — only [`resolve_split_ratio`]'s own next call, once the
/// caller's descriptor catches up (or doesn't), decides whether this
/// drag's value keeps winning. Nothing resolves a `SplitTree`'s divider
/// ratios back out today (there is no `SplitTree` counterpart of
/// `resolve_split_ratio` yet), so persisting one only prevents the
/// cross-divider corruption described above until that counterpart exists.
pub fn apply_split_divider_drag(
    widget_state: &WidgetState,
    tree: WidgetId,
    split_index: usize,
    new_ratio: f32,
) {
    let id = split_ratio_state_id(tree, split_index);
    let mut state = widget_state
        .get::<SplitRatioState>(id)
        .unwrap_or(SplitRatioState {
            baseline: new_ratio,
            resolved: new_ratio,
        });
    state.resolved = new_ratio;
    widget_state.set(id, state);
}

/// Resolves `split`'s ratio for painting — see this section's own doc for
/// why the controlled/uncontrolled decision compares `split.ratio` against
/// the last-seen `baseline` rather than a fixed sentinel. A caller that
/// pins `split.ratio` to the same literal value every frame — including
/// after rejecting a drag outright — reads back as controlled only once
/// that literal value itself changes, never merely because it differs from
/// whatever a previous drag persisted; a caller that never touches `ratio`
/// at all keeps whatever [`apply_split_divider_drag`] persisted
/// indefinitely. Call once per frame, before painting `split`; the
/// returned [`Split`] is what a binding's render should hand to
/// `Backend::draw_split`, not `split` itself.
pub fn resolve_split_ratio(widget_state: &WidgetState, split: &Split) -> Split {
    let id = split_ratio_state_id(split.id, 0);
    let previous = widget_state.get::<SplitRatioState>(id);
    let (baseline, resolved) = match previous {
        None => (split.ratio, split.ratio),
        Some(state) if split.ratio != state.baseline => (split.ratio, split.ratio),
        Some(state) => (state.baseline, state.resolved),
    };
    widget_state.set(id, SplitRatioState { baseline, resolved });
    Split {
        ratio: resolved,
        ..split.clone()
    }
}

// ─── Scroll offset, uncontrolled by default ────────────────────────────────
//
// Unlike `Split::ratio` above (a bare `f32` with no "unset" sentinel),
// `LayoutScroll::offset` is already `Option<f32>` -- see that field's own
// doc for why -- so [`WidgetState::resolve`]'s own controlled/uncontrolled
// policy (its module doc's "Controlled / uncontrolled resolution" section)
// applies directly here, with no baseline-latch struct of this module's
// own to maintain.

/// Resolves a [`crate::layout::LayoutNode::Scroll`]'s offset for painting
/// — the `ScrollView` counterpart of [`resolve_split_ratio`]. `offset` is
/// `scroll_node.offset` as decoded this frame (`Some` pins the controlled
/// position, `None` leaves it to whatever this id's own [`WidgetState`]
/// entry already holds, seeded at `0.0` the first time `id` is seen).
/// The resolved value is clamped into `[0, max_offset]` on every call
/// (not persisted pre-clamped) so a later frame whose content grows back
/// out doesn't stay stuck at a smaller clamp than the one in effect when
/// a previous, shorter-content frame stored it.
pub fn resolve_scroll_offset(
    widget_state: &WidgetState,
    id: WidgetId,
    offset: Option<f32>,
    max_offset: f32,
) -> f32 {
    let raw = widget_state.resolve(id, offset, || 0.0_f32);
    raw.clamp(0.0, max_offset.max(0.0))
}

/// Persists `new_offset` as `id`'s own stored scroll offset — call when the
/// engine's wheel/keyboard/scrollbar-drag handling resolves a `Scroll`
/// container to a new position. Always writes, mirroring
/// [`apply_split_divider_drag`]'s own "the next frame's resolve decides
/// whether this sticks" shape: [`resolve_scroll_offset`]'s own
/// [`WidgetState::resolve`] call overwrites whatever this stores the
/// moment the matching `LayoutScroll::offset` is `Some` again, so a
/// controlled `Scroll` moving here never actually changes what's
/// painted — only an uncontrolled one does.
pub fn apply_scroll_offset(widget_state: &WidgetState, id: WidgetId, new_offset: f32) {
    widget_state.set(id, new_offset);
}

// ─── TabBar active tab, uncontrolled by default ────────────────────────────
//
// Same "compare against the last-seen descriptor, not a fixed sentinel"
// policy as `Split::ratio` above, and for the same reason:
// `TabItem::is_active` all `false` (nothing marked active) is a legitimate
// descriptor state — most real `TabBar`s mark one tab active from their
// very first frame, so latching "controlled" the first time ANY tab is
// active (mirroring `TreeCursorState::scroll_controlled`'s `!= 0` rule)
// would mark nearly every real caller controlled immediately and defeat
// "uncontrolled by default" outright.
#[derive(Debug, Clone, Copy, Default)]
struct TabActiveState {
    baseline: Option<usize>,
    resolved: Option<usize>,
}

/// Persists `index` as `id`'s active tab — call when a
/// [`quadraui::TabBarEvent::TabActivated`] fires. Leaves `baseline`
/// untouched, for the same reason [`apply_split_divider_drag`] leaves
/// [`SplitRatioState::baseline`] untouched: it tracks the descriptor's own
/// last value, which a click never changes by itself.
pub fn apply_tab_activated(widget_state: &WidgetState, id: WidgetId, index: usize) {
    let mut state = widget_state.get::<TabActiveState>(id).unwrap_or_default();
    state.resolved = Some(index);
    widget_state.set(id, state);
}

/// Shifts `id`'s persisted active tab when a
/// [`quadraui::TabBarEvent::TabClosed`] fires for `closed_index` — closing
/// a tab renumbers every later tab down by one (`TabItem` has no stable id
/// of its own, see [`quadraui::TabBar`]'s own doc, so index addressing is
/// all either side has), and a stored index that doesn't shift with it
/// would silently point at the wrong tab from the very next render on. The
/// closed tab's own index (if it was the one persisted as active) resolves
/// to `None` rather than clamping to a neighbor — closing the active tab
/// leaves nothing active until the app or a later click picks one, the
/// same "nothing marked active is a legitimate state" rule
/// [`resolve_tab_bar_active`]'s own section doc opens with. A no-op when
/// nothing has been persisted for `id` yet (nothing to shift). `baseline`
/// shifts the identical way, so the very next [`resolve_tab_bar_active`]
/// call keeps comparing against a baseline addressed the same way the
/// descriptor's own next, already-renumbered `tabs` will be.
pub fn apply_tab_closed(widget_state: &WidgetState, id: WidgetId, closed_index: usize) {
    let Some(mut state) = widget_state.get::<TabActiveState>(id) else {
        return;
    };
    let shift = |idx: Option<usize>| match idx {
        Some(i) if i == closed_index => None,
        Some(i) if i > closed_index => Some(i - 1),
        other => other,
    };
    state.baseline = shift(state.baseline);
    state.resolved = shift(state.resolved);
    widget_state.set(id, state);
}

/// Shared resolution step behind [`resolve_tab_bar_active`] and
/// [`resolve_layout_tabs_active`]: stores the controlled/uncontrolled
/// decision in [`WidgetState`] under `tab_bar.id` and returns the resolved
/// active index, so both callers — one rebuilding a bare [`TabBar`], the
/// other also steering a [`LayoutTabs`] tab group's content pane — apply
/// the identical rule rather than one of them drifting out of sync with
/// the other. See [`resolve_tab_bar_active`]'s own doc for the rule itself.
fn resolved_tab_active_index(widget_state: &WidgetState, tab_bar: &TabBar) -> Option<usize> {
    let id = tab_bar.id;
    let descriptor_active = tab_bar.tabs.iter().position(|tab| tab.is_active);
    let previous = widget_state.get::<TabActiveState>(id);
    let (baseline, resolved) = match previous {
        None => (descriptor_active, descriptor_active),
        Some(state) if descriptor_active != state.baseline => {
            (descriptor_active, descriptor_active)
        }
        Some(state) => (state.baseline, state.resolved),
    };
    widget_state.set(id, TabActiveState { baseline, resolved });
    resolved
}

/// Resolves `tab_bar`'s active tab for painting — the `TabBar` counterpart
/// of [`resolve_split_ratio`]. `descriptor_active` is whichever tab (if
/// any) `tab_bar` itself marks `is_active`; a frame counts as controlled
/// only when that differs from the baseline this module last saw — a
/// caller that pins `is_active` to the same tab every frame, including one
/// that rejects a click outright and resends the identical `tab_bar` it
/// always has, reads back as controlled only once that pinned value itself
/// changes, never merely because it differs from whatever
/// [`apply_tab_activated`] persisted. A caller that never touches
/// `is_active` at all (`descriptor_active` stays `None`, or stays pinned at
/// the same index every frame) keeps whatever [`apply_tab_activated`]
/// persisted indefinitely. Call once per frame, before painting `tab_bar`.
///
/// Only resolves the strip's own `is_active` flags — a `tab_bar` reached
/// through a [`LayoutTabs`] tab group (`LayoutNode::Tabs`) also needs
/// [`LayoutTabs::selected`] resolved the identical way, or the content pane
/// `crate::render::paint_layout` paints stays on the old tab while the
/// strip highlights the new one; see [`resolve_layout_tabs_active`] for
/// that case.
pub fn resolve_tab_bar_active(widget_state: &WidgetState, tab_bar: &TabBar) -> TabBar {
    let resolved = resolved_tab_active_index(widget_state, tab_bar);
    match resolved {
        Some(idx) if idx < tab_bar.tabs.len() => {
            let mut tabs = tab_bar.tabs.clone();
            for (i, tab) in tabs.iter_mut().enumerate() {
                tab.is_active = i == idx;
            }
            TabBar {
                tabs,
                ..tab_bar.clone()
            }
        }
        _ => tab_bar.clone(),
    }
}

/// Resolves a [`LayoutTabs`] tab group's active tab for *both* halves a
/// layout-tree binding paints it with: the strip
/// ([`resolve_tab_bar_active`]'s own job, applied here to `tabs.tab_bar`)
/// and the content pane `crate::render::paint_layout`'s `LayoutNode::Tabs`
/// arm selects via `tabs.entries.get(tabs.selected)`. A `Split` whose
/// `first`/`second` are each a `Tabs` IS this crate's tab group — see
/// [`LayoutTabs`]'s own doc — so this is the one call a layout-tree
/// binding needs to make the content pane track the clicked tab with no
/// app state at all, rather than the strip highlighting one tab while the
/// pane underneath keeps painting another. Call once per frame on the
/// decoded layout tree, before `crate::render::paint_layout`, the same way
/// a decoded `TreeView` needs [`resolve_tree_cursor`] called on it first.
///
/// Mutates `tabs` in place rather than returning a new one:
/// [`crate::layout::LayoutNode`] holds content that isn't [`Clone`] (an
/// `Rc<dyn crate::extra_paint::ExtraPaint>` extra primitive, an arbitrarily
/// nested subtree), so rebuilding `LayoutTabs::entries` wholesale the way
/// [`resolve_tab_bar_active`] rebuilds a bare [`TabBar`] isn't an option
/// here.
pub fn resolve_layout_tabs_active(widget_state: &WidgetState, tabs: &mut LayoutTabs) {
    let resolved = resolved_tab_active_index(widget_state, &tabs.tab_bar);
    let Some(idx) = resolved else {
        return;
    };
    if idx < tabs.tab_bar.tabs.len() {
        for (i, tab) in tabs.tab_bar.tabs.iter_mut().enumerate() {
            tab.is_active = i == idx;
        }
    }
    if idx < tabs.entries.len() {
        tabs.selected = idx;
    }
}

// ─── Tree expanded set, uncontrolled by default ────────────────────────────
//
// Same "compare against the last-seen descriptor, not a fixed sentinel"
// policy as `Split`/`TabBar` above, applied per [`TreePath`] rather than to
// one whole-widget value: `TreeRow::is_expanded` is `Some(bool)` for every
// branch row, and both `true` and `false` are equally plausible starting
// states (a tree that opens fully expanded vs. one that opens fully
// collapsed), so there is no fixed sentinel this module could latch
// against the way `TreeCursorState::scroll_controlled` latches against `0`.

/// One row's expand/collapse resolution — the per-path counterpart of
/// [`SplitRatioState`]/[`TabActiveState`].
#[derive(Debug, Clone, Copy)]
struct ExpandedEntry {
    baseline: bool,
    resolved: bool,
}

/// One tree's per-row expand/collapse overrides, keyed by [`TreePath`].
#[derive(Debug, Clone, Default)]
struct TreeExpandedState {
    entries: HashMap<TreePath, ExpandedEntry>,
}

/// The id [`TreeExpandedState`] is actually stored under — a synthetic id
/// derived from `id`, NOT `id` itself. `WidgetState` holds exactly one
/// typed value per id (see its own module doc on a reused id "discarded
/// and reseeded" when the stored type changes), and a focused tree's
/// cursor/scroll ([`apply_tree_nav_key`]/[`resolve_tree_cursor`]'s own
/// `TreeCursorState`) is already stored under `id` itself — storing
/// `TreeExpandedState` there too would silently clobber whichever of the
/// two was written most recently, on every frame both are in use for the
/// same tree. Deriving a distinct key sidesteps that collision entirely.
fn expanded_state_id(id: WidgetId) -> WidgetId {
    WidgetId::new(format!(
        "quadraui-binding-core::tree-expanded::{}",
        id.as_str()
    ))
}

/// Flips `id`'s stored expand state for `path` — call when a
/// [`quadraui::TreeEvent::RowToggleExpand`] fires. Seeds a never-before-seen
/// path from `false`; the very next [`resolve_tree_expansion`] call
/// reconciles that guess against whatever the descriptor actually says for
/// `path`, the same first-frame seeding [`resolve_split_ratio`]/
/// [`resolve_tab_bar_active`] do for their own state. In practice this only
/// seeds from `false` when [`resolve_tree_expansion`] hasn't run yet this
/// session (a click can't land on a row that was never painted), so the
/// guess is immediately corrected on the next render regardless.
pub fn apply_tree_toggle_expand(widget_state: &WidgetState, id: WidgetId, path: &TreePath) {
    let state_id = expanded_state_id(id);
    let mut state = widget_state
        .get::<TreeExpandedState>(state_id)
        .unwrap_or_default();
    let entry = state.entries.entry(path.clone()).or_insert(ExpandedEntry {
        baseline: false,
        resolved: false,
    });
    entry.resolved = !entry.resolved;
    widget_state.set(state_id, state);
}

/// Resolves `tree`'s per-row expand/collapse state for painting — the
/// `Tree` counterpart of [`resolve_split_ratio`]/[`resolve_tab_bar_active`],
/// applied once per branch row (`is_expanded: Some(_)`) rather than to the
/// whole widget; a leaf row (`is_expanded: None`) is never touched. Also
/// drops every row underneath a resolved-collapsed ancestor outright —
/// `TreeRow::is_expanded` only drives a branch row's own chevron glyph
/// (see `quadraui::tui::tree::draw_tree`'s own doc), so without this an
/// uncontrolled tree's resolved collapse would flip the chevron and leave
/// every descendant row painting anyway, the row set being whatever flat
/// list the caller's descriptor already sent. Prunes any stored path no
/// longer present in `tree.rows` (before that drop — a row hidden by its
/// own collapsed ancestor keeps its override, in case the ancestor
/// re-expands later), mirroring [`WidgetState::gc`]'s own per-frame
/// cleanup at the whole-id level: a row removed from the tree outright
/// drops its entry rather than resurrecting a stale override if the same
/// path is ever reused. As with [`resolve_split_ratio`]/
/// [`resolve_tab_bar_active`], "controlled" here means "this row's own
/// `is_expanded` differs from what this module last saw the descriptor
/// send", not merely "the descriptor sets `is_expanded` at all" — a row
/// pinned at the same `Some(false)`/`Some(true)` every frame, including
/// one that rejects a toggle outright, still reads back as uncontrolled
/// and keeps whatever [`apply_tree_toggle_expand`] resolved. Call once
/// per frame, before painting `tree`.
pub fn resolve_tree_expansion(widget_state: &WidgetState, tree: &TreeView) -> TreeView {
    let state_id = expanded_state_id(tree.id);
    let previous = widget_state.get::<TreeExpandedState>(state_id);
    let had_stored_state = previous.is_some();
    let mut state = previous.unwrap_or_default();
    let mut rows = tree.rows.clone();
    for row in &mut rows {
        if let Some(descriptor_value) = row.is_expanded {
            let entry = state
                .entries
                .entry(row.path.clone())
                .or_insert(ExpandedEntry {
                    baseline: descriptor_value,
                    resolved: descriptor_value,
                });
            if descriptor_value != entry.baseline {
                entry.baseline = descriptor_value;
                entry.resolved = descriptor_value;
            }
            row.is_expanded = Some(entry.resolved);
        }
    }
    let current_paths: HashSet<TreePath> = rows
        .iter()
        .filter(|row| row.is_expanded.is_some())
        .map(|row| row.path.clone())
        .collect();
    state.entries.retain(|path, _| current_paths.contains(path));
    // A tree with no branch rows at all (every `is_expanded` stays `None`)
    // never populates `state.entries`, and never had anything stored to
    // prune either — skip writing back an empty `TreeExpandedState` in
    // that case, since `unwrap_or_default` already treats "nothing
    // stored" and "stored but empty" identically on the next call. A
    // frame whose own `retain` just emptied a *previously* non-empty
    // entry set still writes it back — that pruning is itself the
    // observable effect this call needs to persist, not a no-op to skip.
    if !state.entries.is_empty() || had_stored_state {
        widget_state.set(state_id, state);
    }

    let collapsed_ancestors: Vec<TreePath> = rows
        .iter()
        .filter(|row| row.is_expanded == Some(false))
        .map(|row| row.path.clone())
        .collect();
    rows.retain(|row| {
        !collapsed_ancestors.iter().any(|ancestor| {
            row.path.len() > ancestor.len() && row.path.starts_with(ancestor.as_slice())
        })
    });

    TreeView {
        rows,
        ..tree.clone()
    }
}

// ─── Resolving the six fixed slots as a unit, and reading the store back ───
//
// Every `resolve_*` function above takes one descriptor and returns the
// same shape back; a binding's own `render` still has to know which of its
// six fixed slots are present, in what order they'll be painted, and
// which ones even carry resolvable state. [`resolve_render_refs`] is that
// wiring, done once here so `quadraui-py`/`quadraui-node` each call it
// instead of re-deriving [`crate::render::draw_bands`]'s own band order by
// hand. [`widget_state_snapshot`] is the read-only counterpart an app-layer
// `app.widget_state(id)`/`app.widgetState(id)` surface calls to hand the
// resolved state for one id back to app code, after a frame has already
// resolved it through the functions above.

/// Resolves whichever of `refs`'s `list`/`tree`/`data_table`/`tab_bar`
/// slots are present against `widget_state` — `button`/`text_display`
/// carry no widget-state of their own and pass through unresolved, as do
/// any `extra` slots. Walks [`crate::render::next_band`] in the exact same
/// top-to-bottom order and fixed line counts [`crate::render::draw_bands`]
/// itself paints into, so an occupied slot gets the same `viewport_rows`
/// its own `resolve_tree_cursor`/`resolve_list_cursor`/
/// `resolve_data_table_cursor` would see once actually painted, including
/// the "ran out of vertical room, painted nothing" case — that slot's
/// descriptor passes through unresolved, which costs more than just this
/// frame's resolution: nothing touches its stored [`WidgetState`] entry
/// either, so [`WidgetState::gc`] drops it at the end of this same frame.
/// A transient tiny viewport (or a resize that comes back) silently
/// resets that slot's cursor/scroll rather than preserving it. A tree's
/// per-row expand/collapse is resolved before its cursor, via
/// [`resolve_tree_expansion`], so a collapsed ancestor's now-pruned
/// descendants are never considered when scrolling the resolved selection
/// into view.
///
/// Each occupied slot re-derives its own `viewport_rows` via a full
/// `list_viewport_rows`/`tree_viewport_rows`/`data_table_viewport_rows`
/// shadow layout pass here, then [`crate::render::draw_bands`] does its own
/// layout work again right after to actually paint — roughly doubling
/// per-frame layout cost for whichever of the three slots are present.
/// Fine at today's scale (a handful of fixed slots, not a deep tree); a
/// shared layout result would need the one below to accept it as an input
/// rather than recomputing it.
///
/// Call once per frame, immediately before [`crate::render::draw_bands`] —
/// [`crate::render::RenderFrame::as_refs`] on the returned
/// [`crate::render::RenderFrame`] is what should be painted, not `refs`
/// itself.
pub fn resolve_render_refs(
    widget_state: &WidgetState,
    backend: &mut dyn Backend,
    refs: RenderRefs<'_>,
) -> RenderFrame {
    let viewport = backend.viewport();
    let line_height = backend.line_height();
    let mut y = 0.0_f32;
    let mut band = |lines: f32| {
        crate::render::next_band(&mut y, viewport.width, viewport.height, line_height, lines)
    };

    let button = refs.button.cloned();
    if button.is_some() {
        band(3.0);
    }
    let text_display = refs.text_display.cloned();
    if text_display.is_some() {
        band(3.0);
    }
    let list = refs.list.map(|list| match band(4.0) {
        Some(rect) => {
            resolve_list_cursor(widget_state, list, list_viewport_rows(backend, rect, list))
        }
        None => list.clone(),
    });
    let tree = refs.tree.map(|tree| {
        let expanded = resolve_tree_expansion(widget_state, tree);
        match band(4.0) {
            Some(rect) => {
                let viewport_rows = tree_viewport_rows(backend, rect, &expanded);
                resolve_tree_cursor(widget_state, &expanded, viewport_rows)
            }
            None => expanded,
        }
    });
    let data_table = refs.data_table.map(|table| match band(4.0) {
        Some(rect) => resolve_data_table_cursor(
            widget_state,
            table,
            data_table_viewport_rows(backend, rect, table),
        ),
        None => table.clone(),
    });
    let tab_bar = refs.tab_bar.map(|tab_bar| match band(3.0) {
        Some(_) => resolve_tab_bar_active(widget_state, tab_bar),
        None => tab_bar.clone(),
    });

    RenderFrame {
        button,
        text_display,
        list,
        tree,
        data_table,
        tab_bar,
        extra: refs.extra.clone(),
        list_scroll_px: refs.list_scroll_px,
        tree_scroll_px: refs.tree_scroll_px,
        data_table_scroll_px: refs.data_table_scroll_px,
    }
}

/// [`resolve_render_refs`]'s counterpart for a render callback's decoded
/// [`LayoutNode`] tree: resolves every `Tree`/`List`/`DataTable` leaf,
/// every `Split`'s ratio, and every `Tabs` group's active tab against
/// `widget_state`, mutating `node` in place. A leaf's `Tree`/`List`/
/// `DataTable`/`TabBar` descriptor is resolved under its enclosing
/// [`LayoutLeaf`]'s own tree-address id, not the descriptor's embedded id
/// — the same id [`crate::render::paint_leaf`] keys its [`PaintedHit`] by
/// and [`crate::engine::InteractionEngine`]'s nav/expand hooks write back
/// under, so a render callback that leaves a descriptor's embedded id at
/// its default (or reuses one across leaves) still resolves and persists
/// correctly. Walks `Row`/`Column`'s own [`crate::render::layout_container_rects`]
/// subdivision and `Split`'s own [`Split::layout`] exactly as
/// [`crate::render::paint_layout`] does — including quantising `first_min`/
/// `second_min` into native units and carving the quantised gap out
/// before layout — so a `Tree`/`List`/`DataTable` leaf nested under either
/// gets the same rect it will actually be painted into, for
/// `viewport_rows`'s sake — a `Tabs` group's content area is computed the
/// identical way [`crate::render::paint_layout`]'s own `LayoutNode::Tabs`
/// arm does (`3.0 * line_height` strip, remainder to content). A tree's
/// expand state is resolved before its cursor, same ordering reasoning as
/// [`resolve_render_refs`]'s own tree leaf.
///
/// `Panel`/`SidebarPanel`/`SplitTree` hold no *leaf* widget-state of their
/// own (no cursor/scroll/ratio/active-tab), so their own content recurses
/// with `area` passed straight through rather than their real (paint-time,
/// backend-side-effecting) content rect — the only cost is a `Tree`/
/// `List`/`DataTable` leaf nested under one of those three computing its
/// very first `viewport_rows` against a larger-than-actual rect; every
/// later `apply_*_nav_key` call still scrolls it correctly, against the
/// real painted geometry `crate::hit::tree_rows_and_viewport` (and its
/// `list`/`data_table` counterparts) read back from the hit map
/// `crate::render::paint_layout` itself just populated. A `SplitTree`'s own
/// divider ratios are a different matter — see [`apply_split_divider_drag`]'s
/// own doc.
///
/// Call once per frame, immediately before [`crate::render::paint_layout`]
/// — `node` is mutated in place, so the exact same tree handed to
/// `paint_layout` right after is the resolved one.
pub fn resolve_layout_node(
    widget_state: &WidgetState,
    backend: &mut dyn Backend,
    area: Rect,
    node: &mut LayoutNode,
) {
    match node {
        LayoutNode::Row(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let inner = container.spacing.inset(area, char_width, line_height);
            let rects = layout_container_rects(backend, inner, container, true);
            for (child, rect) in container.children.iter_mut().zip(rects) {
                resolve_layout_node(widget_state, backend, rect, &mut child.node);
            }
        }
        LayoutNode::Column(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let inner = container.spacing.inset(area, char_width, line_height);
            let rects = layout_container_rects(backend, inner, container, false);
            for (child, rect) in container.children.iter_mut().zip(rects) {
                resolve_layout_node(widget_state, backend, rect, &mut child.node);
            }
        }
        LayoutNode::Leaf(LayoutLeaf {
            id: leaf_id,
            descriptor,
            ..
        }) => {
            // Keyed on `leaf_id` — this leaf's own tree-address id — never
            // the descriptor's embedded id. [`crate::render::paint_leaf`]
            // pushes this leaf's `PaintedHit` under `WidgetId::new(leaf_id)`
            // and the engine's nav/expand hooks (`apply_tree_nav_key`,
            // `apply_list_nav_key`, `apply_data_table_nav_key`,
            // `apply_tree_toggle_expand`) write back under that same hit
            // id, so resolving against anything else would read a
            // different (likely empty, and never GC-preserved) entry than
            // the one navigation just wrote. A render callback is free to
            // leave the descriptor's own id at its default and reuse it
            // across leaves — see `crate::wire`'s decode of the leaf `"id"`
            // and the descriptor's own `"id"` as fully independent fields
            // — so the descriptor's id is overwritten here to match before
            // resolving, keeping the rest of this function's logic (and
            // `resolve_tree_cursor`/`resolve_list_cursor`/
            // `resolve_data_table_cursor`/`resolve_tab_bar_active`, shared
            // with [`resolve_render_refs`]'s fixed-slot path, which has no
            // separate leaf id and keys on the descriptor's id directly)
            // unchanged.
            let widget_id = WidgetId::new(leaf_id.clone());
            match descriptor {
                LeafDescriptor::Tree(tree) => {
                    tree.id = widget_id;
                    let expanded = resolve_tree_expansion(widget_state, tree);
                    let viewport_rows = tree_viewport_rows(backend, area, &expanded);
                    *tree = resolve_tree_cursor(widget_state, &expanded, viewport_rows);
                }
                LeafDescriptor::List(list) => {
                    list.id = widget_id;
                    let viewport_rows = list_viewport_rows(backend, area, list);
                    *list = resolve_list_cursor(widget_state, list, viewport_rows);
                }
                LeafDescriptor::DataTable(table) => {
                    table.id = widget_id;
                    let viewport_rows = data_table_viewport_rows(backend, area, table);
                    *table = resolve_data_table_cursor(widget_state, table, viewport_rows);
                }
                LeafDescriptor::TabBar(tab_bar) => {
                    tab_bar.id = widget_id;
                    *tab_bar = resolve_tab_bar_active(widget_state, tab_bar);
                }
                LeafDescriptor::Button(_)
                | LeafDescriptor::TextDisplay(_)
                | LeafDescriptor::Extra(_) => {}
            }
        }
        LayoutNode::Split(split) => {
            split.split = resolve_split_ratio(widget_state, &split.split);
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let inner = split.spacing.inset(area, char_width, line_height);
            // Mirrors `crate::render::paint_layout`'s own `LayoutNode::Split`
            // arm exactly: `first_min`/`second_min` are decoded as portable
            // line counts and must be scaled into native units before
            // `Split::layout` clamps against them, and the quantised gap is
            // carved out of `inner` before layout (`Split::layout`'s min-size
            // clamp evaluates against whatever rect it's handed) and
            // reinserted into `second_bounds` after, via the identical
            // `shrink_main_axis`/`apply_split_gap` pair -- otherwise a
            // non-zero `first_min`/`second_min`/`gap` would hand a nested
            // `Tree`/`List`/`DataTable` leaf a first-frame rect that
            // diverges from the one it's actually painted into.
            let native_split = Split {
                first_min: split.split.first_min * line_height,
                second_min: split.split.second_min * line_height,
                ..split.split.clone()
            };
            let horizontal = matches!(native_split.direction, quadraui::SplitDirection::Horizontal);
            let gap = if horizontal {
                quantise_horizontal(split.spacing.gap, char_width)
            } else {
                quantise_vertical(split.spacing.gap, line_height)
            };
            let draw_area = shrink_main_axis(inner, horizontal, gap);
            let mut layout = native_split.layout(draw_area, SplitMeasure::new(line_height));
            apply_split_gap(&mut layout, horizontal, gap);
            resolve_layout_node(widget_state, backend, layout.first_bounds, &mut split.first);
            resolve_layout_node(
                widget_state,
                backend,
                layout.second_bounds,
                &mut split.second,
            );
        }
        LayoutNode::Tabs(tabs) => {
            resolve_layout_tabs_active(widget_state, tabs);
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let inner = tabs.spacing.inset(area, char_width, line_height);
            let strip_height = (3.0 * line_height).min(inner.height);
            let content_rect = Rect::new(
                inner.x,
                inner.y + strip_height,
                inner.width,
                (inner.height - strip_height).max(0.0),
            );
            let selected = tabs.selected;
            // Only the selected entry's content recurses -- an unselected
            // tab's own `Tree`/`List`/`DataTable` leaf is never resolved
            // this frame, so [`WidgetState::gc`] drops its stored cursor/
            // scroll at the end of it. Switching away and back resets that
            // leaf's cursor rather than preserving it, the same cost
            // [`resolve_render_refs`]'s own doc calls out for its
            // "ran out of vertical room" case.
            if let Some(entry) = tabs.entries.get_mut(selected) {
                resolve_layout_node(widget_state, backend, content_rect, &mut entry.content);
            }
        }
        #[allow(deprecated)] // `Panel` is `#[deprecated]`; this arm is its own plumbing.
        LayoutNode::Panel(panel_node) => {
            resolve_layout_node(widget_state, backend, area, &mut panel_node.content);
        }
        LayoutNode::SidebarPanel(sidebar_node) => {
            resolve_layout_node(widget_state, backend, area, &mut sidebar_node.content);
        }
        LayoutNode::SplitTree(split_tree) => {
            for leaf in &mut split_tree.leaves {
                resolve_layout_node(widget_state, backend, area, leaf);
            }
        }
        LayoutNode::Scroll(scroll_node) => {
            let metrics = ScrollMetrics {
                char_width: backend.char_width(),
                line_height: backend.line_height(),
            };
            let inner = scroll_node
                .spacing
                .inset(area, metrics.char_width, metrics.line_height);
            let (viewport, _track) =
                scroll_viewport_and_track(inner, scroll_node.axis, metrics.line_height);
            let content_len =
                natural_main_length(backend, &scroll_node.content, scroll_node.axis, metrics);
            let visible_len = match scroll_node.axis {
                quadraui::ScrollAxis::Vertical => viewport.height,
                quadraui::ScrollAxis::Horizontal => viewport.width,
            };
            let max_offset = (content_len - visible_len).max(0.0);
            let id = WidgetId::new(scroll_node.id.as_str());
            let offset = resolve_scroll_offset(widget_state, id, scroll_node.offset, max_offset);
            scroll_node.offset = Some(offset);

            let slots = scroll_content_slots(
                backend,
                viewport,
                scroll_node.axis,
                offset,
                &scroll_node.content,
                content_len,
                metrics,
            );
            match slots {
                ScrollSlots::Children(slots) => {
                    // `scroll_node.content` may be a `LayoutNode::Styled`
                    // wrapper around the `Row`/`Column` -- a `"style"` key
                    // is purely cosmetic, so match against
                    // `LayoutNode::unstyled_mut` here rather than the raw
                    // node, the same structural match
                    // `crate::render::scroll_content_slots`'s own
                    // `matching_container` makes.
                    if let LayoutNode::Row(container) | LayoutNode::Column(container) =
                        scroll_node.content.unstyled_mut()
                    {
                        for (child, slot) in container.children.iter_mut().zip(slots) {
                            if slot.visible {
                                resolve_layout_node(
                                    widget_state,
                                    backend,
                                    slot.rect,
                                    &mut child.node,
                                );
                            }
                        }
                    }
                }
                ScrollSlots::Atomic(rect, visible) => {
                    if visible {
                        resolve_layout_node(widget_state, backend, rect, &mut scroll_node.content);
                    }
                }
            }
        }
        // A `"style"` wrapper carries no widget state of its own -- it's
        // purely a `crate::render::paint_layout`-time overlay -- so this
        // resolve pass is a straight passthrough to whatever it wraps.
        LayoutNode::Styled(styled) => {
            resolve_layout_node(widget_state, backend, area, &mut styled.node);
        }
    }
}

/// A read-only merge of whatever [`WidgetState`] currently holds for one
/// id, across every shape this module tracks — [`resolve_tree_cursor`]/
/// [`resolve_list_cursor`]/[`resolve_data_table_cursor`]'s cursor +
/// scroll, [`resolve_split_ratio`]'s ratio, [`resolve_tab_bar_active`]/
/// [`resolve_layout_tabs_active`]'s active tab, and
/// [`resolve_tree_expansion`]'s per-row expand set — for a binding's own
/// introspection surface (`quadraui-py`'s `QuiApp.widget_state`) rather
/// than for resolving a frame's descriptor. Each field is independently
/// `None` when that shape isn't what `id` (or, for `expanded`, `id`'s
/// derived expansion key — see [`expanded_state_id`]) is storing; a tree
/// widget populates `selected_path`/`scroll_offset` from its own id and
/// `expanded` from the derived one in the same snapshot, since both can
/// be live for the same tree at once.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct WidgetStateSnapshot {
    /// A `Tree`'s stored cursor row, if `id` is a tree being tracked by
    /// [`resolve_tree_cursor`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_path: Option<TreePath>,
    /// A `List`/`DataTable`'s stored cursor row, if `id` is one being
    /// tracked by [`resolve_list_cursor`]/[`resolve_data_table_cursor`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_idx: Option<usize>,
    /// A `Tree`/`List`/`DataTable`'s stored scroll offset, alongside
    /// whichever of `selected_path`/`selected_idx` applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_offset: Option<usize>,
    /// A `Split`'s stored divider ratio, if `id` is one being tracked by
    /// [`resolve_split_ratio`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f32>,
    /// A `TabBar`/`LayoutTabs`' stored active tab index, if `id` is one
    /// being tracked by [`resolve_tab_bar_active`]/
    /// [`resolve_layout_tabs_active`]. `None` both when nothing is stored
    /// for `id` under this shape and when it is stored but nothing is
    /// currently active — a read-only snapshot has no need to tell those
    /// two apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tab: Option<usize>,
    /// A `Tree`'s stored set of currently-expanded row paths, if `id` is
    /// one being tracked by [`resolve_tree_expansion`] — sorted for a
    /// deterministic read, since the underlying store is a `HashMap`.
    /// `Some(vec![])` (tracked, nothing expanded) and `None` (not tracked
    /// at all) are kept distinct.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded: Option<Vec<TreePath>>,
}

/// Reads [`WidgetStateSnapshot`]'s fields back out of [`WidgetState`] for
/// `id`, or `None` if none of the shapes it tracks have anything stored
/// for `id` (or its derived expansion key) at all. Never mutates the
/// store and never marks `id` seen — a snapshot is a side-effect-free
/// read, unlike every `resolve_*` function above.
pub fn widget_state_snapshot(
    widget_state: &WidgetState,
    id: WidgetId,
) -> Option<WidgetStateSnapshot> {
    let mut snapshot = WidgetStateSnapshot::default();
    let mut found = false;

    if let Some(stored) = widget_state.get::<TreeCursorState>(id) {
        snapshot.selected_path = stored.selected_path;
        snapshot.scroll_offset = Some(stored.scroll_offset);
        found = true;
    } else if let Some(stored) = widget_state.get::<ListCursorState>(id) {
        snapshot.selected_idx = stored.selected_idx;
        snapshot.scroll_offset = Some(stored.scroll_offset);
        found = true;
    } else if let Some(stored) = widget_state.get::<DataTableCursorState>(id) {
        snapshot.selected_idx = stored.selected_idx;
        snapshot.scroll_offset = Some(stored.scroll_offset);
        found = true;
    }

    if let Some(stored) = widget_state.get::<SplitRatioState>(id) {
        snapshot.ratio = Some(stored.resolved);
        found = true;
    }

    if let Some(stored) = widget_state.get::<TabActiveState>(id) {
        snapshot.active_tab = stored.resolved;
        found = true;
    }

    if let Some(stored) = widget_state.get::<TreeExpandedState>(expanded_state_id(id)) {
        let mut paths: Vec<TreePath> = stored
            .entries
            .iter()
            .filter(|(_, entry)| entry.resolved)
            .map(|(path, _)| path.clone())
            .collect();
        paths.sort();
        snapshot.expanded = Some(paths);
        found = true;
    }

    found.then_some(snapshot)
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
    use crate::layout::{
        LayoutContainer, LayoutLeaf, LayoutNode, LayoutSplit, LayoutTabEntry, LeafDescriptor,
        Spacing,
    };
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

    /// A minimal [`AppLogic`] whose `render` resolves [`BindingHost::render_refs`]
    /// through [`resolve_render_refs`] before painting — exactly the
    /// resolve-then-paint sequence a real binding's own `render` runs for
    /// its six fixed slots, as opposed to [`HostLogic`]'s raw (unresolved)
    /// `render_refs()` call above.
    struct ResolvedHostLogic {
        host: BindingHost,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ResolvedHostLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let resolved =
                resolve_render_refs(self.engine.widget_state(), backend, self.host.render_refs());
            let mut hits = Vec::new();
            draw_bands(backend, resolved.as_refs(), &mut hits, &|id| {
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

    /// Builds a `row_count`-row `Tree` with no selection/scroll set and
    /// drives it through a [`ResolvedHostLogic`] — the [`BindingHost`]
    /// fixed-slot counterpart of `engine::tests::tree_driver`.
    fn resolved_tree_driver(
        row_count: usize,
        selected_path: Option<TreePath>,
        scroll_offset: usize,
    ) -> TuiDriver<ResolvedHostLogic> {
        use quadraui::StyledText;

        let rows: Vec<TreeRow> = (0..row_count)
            .map(|i| TreeRow {
                path: vec![i as u16],
                indent: 0,
                icon: None,
                text: StyledText::plain(format!("row {i}")),
                badge: None,
                is_expanded: None,
                decoration: Default::default(),
                edit: None,
            })
            .collect();

        let tree = TreeView {
            id: WidgetId::new("tree"),
            rows,
            selection_mode: quadraui::SelectionMode::Single,
            selected_path,
            scroll_offset,
            style: Default::default(),
            has_focus: false,
        };
        let logic = ResolvedHostLogic {
            host: BindingHost {
                tree: Some(tree),
                ..BindingHost::default()
            },
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink),
        };
        TuiDriver::new(logic, 24, 8)
    }

    /// A 500-row `Tree` painted through [`BindingHost`]'s fixed `tree` slot
    /// with no `selected_path`/`scroll_offset` set must still move its
    /// cursor and keep it scrolled into view from arrow-key presses alone
    /// — [`resolve_render_refs`] is what makes a fixed-slot binding see the
    /// same uncontrolled-by-default behaviour `engine::tests`' own
    /// `TreeLogic` harness already covers directly against
    /// `resolve_tree_cursor`. [`widget_state_snapshot`] must report the
    /// identical resolved cursor/scroll back to app code.
    #[test]
    fn resolve_render_refs_scrolls_an_uncontrolled_tree_into_view() {
        use quadraui::{Key, Modifiers, NamedKey};

        let mut driver = resolved_tree_driver(500, None, 0);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("tree"))),
                "the tree is a tab stop and must accept focus"
            );
        }

        for _ in 0..40 {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

        let hits = driver.app().engine.hit_map();
        let tree_hit = hits.iter().find_map(|hit| match hit {
            PaintedHit::Tree {
                id,
                scroll_offset,
                viewport_rows,
                ..
            } if *id == WidgetId::new("tree") => Some((*scroll_offset, *viewport_rows)),
            _ => None,
        });
        let (scroll_offset, viewport_rows) =
            tree_hit.expect("the tree must still be painted after 40 Down presses");
        assert!(
            scroll_offset > 0,
            "500 rows in a {viewport_rows}-row tree viewport must have scrolled: {scroll_offset}"
        );
        assert!(
            (scroll_offset..scroll_offset + viewport_rows).contains(&39),
            "row 39 (the moved cursor) must be within the scrolled window [{scroll_offset}, {})",
            scroll_offset + viewport_rows
        );

        let snapshot =
            widget_state_snapshot(driver.app().engine.widget_state(), WidgetId::new("tree"))
                .expect("the tree's id must have resolved state after 40 Down presses");
        assert_eq!(
            snapshot.selected_path,
            Some(vec![39]),
            "the snapshot's selected_path must match what was painted"
        );
        assert_eq!(
            snapshot.scroll_offset,
            Some(scroll_offset),
            "the snapshot's scroll_offset must match what was painted"
        );
    }

    /// A `Tree` whose descriptor pins a non-zero `scroll_offset` keeps that
    /// exact value through [`resolve_render_refs`], even while arrow keys
    /// move the (uncontrolled) selection — and [`widget_state_snapshot`]
    /// reports the pinned value back, not whatever an uncontrolled scroll
    /// would have computed.
    #[test]
    fn resolve_render_refs_keeps_a_controlled_scroll_offset_pinned() {
        use quadraui::{Key, Modifiers, NamedKey};

        let mut driver = resolved_tree_driver(500, None, 50);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("tree"))),
                "the tree is a tab stop and must accept focus"
            );
        }

        for _ in 0..10 {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

        let hits = driver.app().engine.hit_map();
        let scroll_offset = hits.iter().find_map(|hit| match hit {
            PaintedHit::Tree {
                id, scroll_offset, ..
            } if *id == WidgetId::new("tree") => Some(*scroll_offset),
            _ => None,
        });
        assert_eq!(
            scroll_offset,
            Some(50),
            "a controlled scroll_offset must stay pinned at the descriptor's own value: {:?}",
            scroll_offset
        );

        let snapshot =
            widget_state_snapshot(driver.app().engine.widget_state(), WidgetId::new("tree"))
                .expect("the tree's id must have resolved state");
        assert_eq!(snapshot.scroll_offset, Some(50));
    }

    /// A `Split`'s ratio persists after a divider drag — the same
    /// `apply_split_divider_drag`/`resolve_split_ratio` hook
    /// `InteractionEngine::handle` already applies for every binding's own
    /// `UiEvent::SplitDividerDragged` — and [`widget_state_snapshot`]
    /// reports the dragged ratio back to app code, not the descriptor's
    /// original one.
    #[test]
    fn widget_state_snapshot_reports_a_split_ratio_persisted_after_a_drag() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("split:main");
        let split = Split {
            id,
            direction: quadraui::SplitDirection::Horizontal,
            ratio: 0.3,
            first_min: 0.0,
            second_min: 0.0,
        };
        // The first frame resolves the descriptor's own starting ratio.
        resolve_split_ratio(&widget_state, &split);
        // A drag persists a new ratio.
        apply_split_divider_drag(&widget_state, id, 0, 0.7);

        let snapshot =
            widget_state_snapshot(&widget_state, id).expect("a ratio must be stored for id");
        assert_eq!(snapshot.ratio, Some(0.7));

        // The next frame's descriptor still sends the OLD ratio (the same
        // literal the app always has) -- uncontrolled, so the dragged
        // value keeps winning.
        let resolved = resolve_split_ratio(&widget_state, &split);
        assert_eq!(resolved.ratio, 0.7);
    }

    /// [`widget_state_snapshot`] reports a `TabBar`'s active tab once
    /// [`apply_tab_activated`] has persisted a click.
    #[test]
    fn widget_state_snapshot_reports_a_tab_bars_active_tab_after_a_click() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("tabs:main");
        apply_tab_activated(&widget_state, id, 2);
        let snapshot =
            widget_state_snapshot(&widget_state, id).expect("an active tab must be stored for id");
        assert_eq!(snapshot.active_tab, Some(2));
    }

    /// [`widget_state_snapshot`] reports a `Tree`'s expanded row paths once
    /// [`apply_tree_toggle_expand`] has persisted a toggle — stored under a
    /// synthetic id derived from `id`, per [`expanded_state_id`]'s own doc,
    /// so this must resolve transparently from the plain `id` callers use.
    #[test]
    fn widget_state_snapshot_reports_a_trees_expanded_paths_after_a_toggle() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("tree:main");
        apply_tree_toggle_expand(&widget_state, id, &vec![0]);
        let snapshot =
            widget_state_snapshot(&widget_state, id).expect("expand state must be stored for id");
        assert_eq!(snapshot.expanded, Some(vec![vec![0]]));
    }

    /// An `id` nothing has ever resolved through any `resolve_*` function
    /// has no stored state of any kind — [`widget_state_snapshot`] must say
    /// so plainly (`None`) rather than returning a snapshot of all-`None`
    /// fields an app could mistake for "resolved, but empty".
    #[test]
    fn widget_state_snapshot_is_none_for_an_id_never_resolved() {
        let widget_state = WidgetState::new();
        assert!(widget_state_snapshot(&widget_state, WidgetId::new("never-rendered")).is_none());
    }

    /// A minimal [`AppLogic`] painting a single two-pane `Split` through a
    /// decoded [`LayoutNode`] tree, resolving the WHOLE tree through
    /// [`resolve_layout_node`] before every render — the generic,
    /// render-callback-path counterpart of `engine::tests::SplitLogic`
    /// (which calls `resolve_split_ratio` directly on a bare `Split`, not
    /// through a decoded tree). `split` is the raw descriptor an app would
    /// hand a binding, never mutated by a drag.
    struct ResolvedSplitLogic {
        split: Split,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ResolvedSplitLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut node = LayoutNode::Split(LayoutSplit {
                split: self.split.clone(),
                first: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "left".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(WidgetId::new("left-btn"), "L")),
                    scroll_px: None,
                })),
                second: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "right".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(
                        WidgetId::new("right-btn"),
                        "R",
                    )),
                    scroll_px: None,
                })),
                spacing: Spacing::default(),
            });
            resolve_layout_node(self.engine.widget_state(), backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Dragging a two-pane `Split`'s divider persists the resolved ratio
    /// through [`resolve_layout_node`]'s generic `LayoutNode::Split`
    /// recursion — not just [`resolve_split_ratio`] called directly, which
    /// `engine::tests::dragging_a_split_divider_persists_the_ratio_into_the_widget_state_store`
    /// already covers — and [`widget_state_snapshot`] reports the dragged
    /// ratio back to app code afterward.
    #[test]
    fn resolve_layout_node_persists_a_split_ratio_after_a_drag() {
        let split = Split {
            id: WidgetId::new("split"),
            direction: quadraui::SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let logic = ResolvedSplitLogic {
            split,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        let divider_bounds = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Split { layout } => Some(layout.divider_bounds),
                    _ => None,
                })
                .expect("the split should have painted a divider")
        };
        let divider_point = Point::new(
            divider_bounds.x,
            divider_bounds.y + divider_bounds.height / 2.0,
        );

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: quadraui::MouseButton::Left,
            position: divider_point,
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(divider_point.x + 15.0, divider_point.y),
            buttons: quadraui::ButtonMask {
                left: true,
                right: false,
                middle: false,
            },
        });

        // `apply_split_divider_drag` persists the drag inside
        // `InteractionEngine::handle` itself, before `sink.dispatch` even
        // runs (see that function's own doc) -- so the widget-state store
        // already reflects it immediately, with no need to inspect what
        // `RecordingSink` (a no-op here) did with the dispatched event.
        let dragged_ratio =
            widget_state_snapshot(driver.app().engine.widget_state(), WidgetId::new("split"))
                .and_then(|snapshot| snapshot.ratio)
                .expect("the drag must have persisted a split_ratio for id");
        assert_ne!(
            dragged_ratio, 0.5,
            "the drag must have actually moved the ratio away from its starting 0.5"
        );

        // The app's own `Split` descriptor is never touched by the drag —
        // this render still hands `resolve_layout_node` the exact same
        // `ratio: 0.5` it always has.
        driver.render();
        let resolved_ratio = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Split { layout } => Some(layout.resolved_ratio),
                    _ => None,
                })
                .expect("the split should still have painted a divider")
        };
        assert!(
            (resolved_ratio - dragged_ratio).abs() < 0.001,
            "the dragged ratio must survive a render whose descriptor still says 0.5: got {resolved_ratio}, expected {dragged_ratio}"
        );

        let snapshot =
            widget_state_snapshot(driver.app().engine.widget_state(), WidgetId::new("split"))
                .expect("the split's id must have resolved state");
        assert!(
            (snapshot.ratio.expect("ratio must be set") - dragged_ratio).abs() < 0.001,
            "the snapshot must report the same dragged ratio back to app code"
        );
    }

    /// A minimal [`AppLogic`] painting a single `Tree` leaf through a
    /// decoded [`LayoutNode`] tree whose [`LayoutLeaf::id`] deliberately
    /// differs from the `Tree` descriptor's own embedded `id` — the shape
    /// [`resolve_layout_node`]'s own doc calls out as legal (a render
    /// callback is free to leave a descriptor's embedded id at its
    /// default, or reuse one across leaves).
    struct ResolvedTreeLeafLogic {
        tree: TreeView,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for ResolvedTreeLeafLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut node = LayoutNode::Leaf(LayoutLeaf {
                id: "leaf:tree".to_string(),
                descriptor: LeafDescriptor::Tree(self.tree.clone()),
                scroll_px: None,
            });
            resolve_layout_node(self.engine.widget_state(), backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// The case [`resolve_layout_node`]'s own doc calls out: a `Tree`
    /// leaf reached through a decoded [`LayoutNode`] tree whose
    /// outer [`LayoutLeaf::id`] (`"leaf:tree"`) differs from the
    /// descriptor's own embedded [`TreeView::id`] (`"tree:inner"`).
    /// [`crate::render::paint_leaf`] pushes this leaf's [`PaintedHit`]
    /// under the OUTER id, and [`InteractionEngine::handle`]'s nav hook
    /// writes `TreeCursorState` back under that exact same (focused/hit)
    /// id — so [`resolve_layout_node`] must resolve and persist under the
    /// outer id too, or navigation would silently do nothing on screen
    /// and the hit id's freshly-written state would be swept by
    /// [`WidgetState::gc`] the very next frame.
    #[test]
    fn resolve_layout_node_keys_a_leaf_whose_descriptor_id_differs_from_its_own_on_the_outer_leaf_id(
    ) {
        use quadraui::{Key, Modifiers, NamedKey, StyledText};

        let rows: Vec<TreeRow> = (0..500)
            .map(|i| TreeRow {
                path: vec![i as u16],
                indent: 0,
                icon: None,
                text: StyledText::plain(format!("row {i}")),
                badge: None,
                is_expanded: None,
                decoration: Default::default(),
                edit: None,
            })
            .collect();
        let tree = TreeView {
            // Deliberately NOT `"leaf:tree"` — the whole point of this
            // test is that the two ids disagree.
            id: WidgetId::new("tree:inner"),
            rows,
            selection_mode: quadraui::SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: Default::default(),
            has_focus: false,
        };
        let logic = ResolvedTreeLeafLogic {
            tree,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink),
        };
        let mut driver = TuiDriver::new(logic, 24, 8);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("leaf:tree"))),
                "the leaf's own outer tree-address id must be a tab stop"
            );
        }

        for _ in 0..40 {
            driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
        }

        let tree_hit = {
            let hits = driver.app().engine.hit_map();
            hits.iter().find_map(|hit| match hit {
                PaintedHit::Tree {
                    id,
                    scroll_offset,
                    viewport_rows,
                    ..
                } if *id == WidgetId::new("leaf:tree") => Some((*scroll_offset, *viewport_rows)),
                _ => None,
            })
        };
        let (scroll_offset, viewport_rows) =
            tree_hit.expect("the tree must still be painted, keyed by its own outer leaf id");
        assert!(
            scroll_offset > 0,
            "500 rows in a {viewport_rows}-row tree viewport must have scrolled: {scroll_offset}"
        );
        assert!(
            (scroll_offset..scroll_offset + viewport_rows).contains(&39),
            "row 39 (the moved cursor) must be within the scrolled window [{scroll_offset}, {})",
            scroll_offset + viewport_rows
        );

        let snapshot = widget_state_snapshot(
            driver.app().engine.widget_state(),
            WidgetId::new("leaf:tree"),
        )
        .expect("the leaf's own outer id must have resolved state after 40 Down presses");
        assert_eq!(
            snapshot.selected_path,
            Some(vec![39]),
            "the snapshot keyed by the outer leaf id must match what was painted"
        );

        assert!(
            widget_state_snapshot(
                driver.app().engine.widget_state(),
                WidgetId::new("tree:inner")
            )
            .is_none(),
            "the descriptor's own embedded id must never accumulate widget state of its own"
        );

        // A further render — nothing new dispatched — must not have the
        // leaf id's entry swept by `WidgetState::gc()`: `resolve_layout_node`
        // itself has to touch it every frame, not just the nav hook.
        driver.render();
        assert!(
            widget_state_snapshot(
                driver.app().engine.widget_state(),
                WidgetId::new("leaf:tree")
            )
            .is_some(),
            "the outer leaf id's widget state must survive a further render instead of being GC'd"
        );
    }

    /// An [`AppLogic`] whose `render` resolves then paints a
    /// `LayoutNode::Scroll` hosting a column of buttons -- drives
    /// [`resolve_layout_node`]'s own `LayoutNode::Scroll` arm through a
    /// real [`TuiDriver`] frame, the way a binding's own `render` would
    /// (resolve, then paint, never paint alone).
    struct ScrollLeafLogic {
        button_count: usize,
        offset: Option<f32>,
        engine: InteractionEngine,
    }

    impl AppLogic for ScrollLeafLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let content = LayoutNode::Column(LayoutContainer::new(
                (0..self.button_count)
                    .map(|i| {
                        let label = i.to_string();
                        LayoutNode::Leaf(LayoutLeaf {
                            id: label.clone(),
                            descriptor: LeafDescriptor::Button(Button::new(
                                WidgetId::new(label.clone()),
                                label,
                            )),
                            scroll_px: None,
                        })
                    })
                    .collect(),
                Default::default(),
            ));
            let mut node = LayoutNode::Scroll(crate::layout::LayoutScroll {
                id: "scroll".to_string(),
                axis: quadraui::ScrollAxis::Vertical,
                offset: self.offset,
                content: Box::new(content),
                spacing: Default::default(),
            });
            resolve_layout_node(self.engine.widget_state(), backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    fn painted_button_ids(hits: &[PaintedHit]) -> Vec<String> {
        hits.iter()
            .filter_map(|hit| match hit {
                PaintedHit::Button { id, .. } => Some(id.as_str().to_string()),
                _ => None,
            })
            .collect()
    }

    /// A controlled offset resolved through [`resolve_layout_node`] (not
    /// just [`crate::render::paint_layout`] called alone, as
    /// `crate::render::tests` already covers) still paints exactly the
    /// ten buttons the issue's own acceptance scenario names.
    #[test]
    fn resolve_layout_node_scroll_paints_the_ten_buttons_at_a_controlled_offset() {
        let logic = ScrollLeafLogic {
            button_count: 40,
            offset: Some(20.0),
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let want: Vec<String> = (20..30).map(|i| i.to_string()).collect();
        assert_eq!(painted_button_ids(&hits), want);
    }

    /// An uncontrolled (`None`) offset resolves to `0.0` the first frame,
    /// and — unlike a bare [`crate::render::paint_layout`] call with no
    /// `resolve_layout_node` pass first — persists across a further
    /// uncontrolled render rather than being swept by
    /// [`crate::widget_state::WidgetState::gc`], mirroring
    /// [`resolve_layout_node_keys_a_leaf_whose_descriptor_id_differs_from_its_own_on_the_outer_leaf_id`]'s
    /// own "survives a further render" assertion for a `Tree` leaf.
    #[test]
    fn resolve_layout_node_scroll_uncontrolled_offset_persists_across_renders() {
        let logic = ScrollLeafLogic {
            button_count: 40,
            offset: None,
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let want: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        assert_eq!(painted_button_ids(&driver.app().engine.hit_map()), want);

        driver.render();
        assert_eq!(
            painted_button_ids(&driver.app().engine.hit_map()),
            want,
            "an uncontrolled scroll offset must survive a further render unchanged"
        );
    }

    /// [`ScrollLeafLogic`]'s own scenario, but with `content` wrapped in a
    /// `LayoutNode::Styled` (a `"style"` key on the `"scroll"` node's
    /// `"content"` dict) -- a purely cosmetic `"style"` sibling must not
    /// change the scroll's own structural behaviour: `resolve_layout_node`
    /// must still resolve each child's own offset against the wrapped
    /// `Column`'s children (not fall back to the atomic "whole subtree is
    /// one leaf" path), `scroll_content_slots` must still report one slot
    /// per child, and the painted button whose own `style` overrides `fg`
    /// must show that colour. Covers the review's own "a `Styled` wrapper
    /// is invisible to the `"scroll"` path's structural matches" finding.
    struct StyledScrollContentLogic {
        button_count: usize,
        offset: Option<f32>,
        engine: InteractionEngine,
    }

    impl AppLogic for StyledScrollContentLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let content = LayoutNode::Column(LayoutContainer::new(
                (0..self.button_count)
                    .map(|i| {
                        let label = i.to_string();
                        let leaf = LayoutNode::Leaf(LayoutLeaf {
                            id: label.clone(),
                            descriptor: LeafDescriptor::Button(Button::new(
                                WidgetId::new(label.clone()),
                                label,
                            )),
                            scroll_px: None,
                        });
                        if i == 0 {
                            LayoutNode::Styled(crate::layout::LayoutStyled {
                                style: crate::layout::StyleOverride {
                                    fg: Some(quadraui::Color::rgb(0, 255, 0)),
                                    ..Default::default()
                                },
                                node: Box::new(leaf),
                            })
                        } else {
                            leaf
                        }
                    })
                    .collect(),
                Default::default(),
            ));
            // Wrap the whole content in its own `"style"` override too --
            // a cosmetic `bg` that must never flip `scroll_content_slots`
            // from `Children` to `Atomic`.
            let styled_content = LayoutNode::Styled(crate::layout::LayoutStyled {
                style: crate::layout::StyleOverride {
                    bg: Some(quadraui::Color::rgb(255, 0, 0)),
                    ..Default::default()
                },
                node: Box::new(content),
            });
            let mut node = LayoutNode::Scroll(crate::layout::LayoutScroll {
                id: "scroll".to_string(),
                axis: quadraui::ScrollAxis::Vertical,
                offset: self.offset,
                content: Box::new(styled_content),
                spacing: Default::default(),
            });
            resolve_layout_node(self.engine.widget_state(), backend, area, &mut node);
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// A `"style"` key on a scroll's `content` node must not change which
    /// buttons paint at a given offset -- the same ten-of-forty window
    /// [`resolve_layout_node_scroll_paints_the_ten_buttons_at_a_controlled_offset`]
    /// asserts for an unstyled `Column`.
    #[test]
    fn resolve_layout_node_scroll_paints_the_ten_buttons_when_content_is_styled() {
        let logic = StyledScrollContentLogic {
            button_count: 40,
            offset: Some(20.0),
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let want: Vec<String> = (20..30).map(|i| i.to_string()).collect();
        assert_eq!(
            painted_button_ids(&hits),
            want,
            "a styled scroll content must resolve/paint the same per-child \
             slots as an unstyled one"
        );
    }

    /// A styled scroll's own `PaintedHit::Scroll::children` must still be
    /// populated per-child -- a `LayoutNode::Styled` wrapper around the
    /// content `Column` must not drop `ScrollSlots::Children` down to
    /// `ScrollSlots::Atomic`, which would report an empty `children` list
    /// (`crate::render::paint_scroll`'s own `ScrollChildHit` match).
    #[test]
    fn resolve_layout_node_scroll_children_hits_survive_a_styled_content_wrapper() {
        let logic = StyledScrollContentLogic {
            button_count: 5,
            offset: Some(0.0),
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let scroll_children = hits.iter().find_map(|hit| match hit {
            PaintedHit::Scroll { children, .. } => Some(children.clone()),
            _ => None,
        });
        assert_eq!(
            scroll_children.map(|c| c.len()),
            Some(5),
            "a styled scroll content column must still report one \
             ScrollChildHit per child"
        );
    }

    /// The styled first button's own `"fg"` override must still paint --
    /// proof [`crate::render::paint_scroll`]'s own `push_styled_scope`
    /// call actually brackets the per-child paint, not just the atomic
    /// fallback path.
    #[test]
    fn resolve_layout_node_scroll_content_child_style_override_still_paints() {
        let logic = StyledScrollContentLogic {
            button_count: 5,
            offset: Some(0.0),
            engine: InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();

        let (x, y) = driver.find("0").expect("button \"0\" should paint");
        let fg = driver
            .style_at(x as u16, y as u16)
            .expect("style at button 0")
            .fg;
        assert_eq!(
            fg,
            quadraui::tui::ratatui_color(quadraui::Color::rgb(0, 255, 0)),
            "button \"0\"'s own nested \"style\" override must still apply \
             when painted through a styled scroll content wrapper"
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
        let tree = LayoutNode::Row(LayoutContainer::new(
            vec![
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
            Spacing::default(),
        ));
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
        let tree = LayoutNode::Row(LayoutContainer::new(
            vec![
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
            Spacing::default(),
        ));
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

    fn test_split(ratio: f32) -> Split {
        Split {
            id: WidgetId::new("host-test:split"),
            direction: quadraui::SplitDirection::Horizontal,
            ratio,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    /// Dragging a split divider with no app-supplied ratio on the next
    /// frame (the descriptor keeps resending its original literal `0.5`,
    /// as an app tracking no state of its own would) leaves the dragged
    /// ratio in place — this module's own acceptance contract for the
    /// split-ratio half of the widget-state store.
    #[test]
    fn apply_split_divider_drag_persists_across_an_uncontrolled_render() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:split");
        let split = test_split(0.5);

        // A render always happens before a drag can land on anything —
        // this seeds the baseline from the descriptor's own starting
        // ratio, the same order `InteractionEngine::handle` and a
        // binding's `render` run in for real.
        resolve_split_ratio(&widget_state, &split);
        apply_split_divider_drag(&widget_state, id, 0, 0.7);

        let resolved = resolve_split_ratio(&widget_state, &split);
        assert_eq!(
            resolved.ratio, 0.7,
            "a dragged ratio must survive a render with no ratio explicitly supplied"
        );
        // And it keeps surviving on a further uncontrolled render, not just
        // the first one right after the drag.
        let resolved_again = resolve_split_ratio(&widget_state, &split);
        assert_eq!(resolved_again.ratio, 0.7);
    }

    /// An app that explicitly changes `Split::ratio` to a new value (not
    /// merely resending the same literal it always has) wins outright over
    /// whatever a previous drag persisted.
    #[test]
    fn resolve_split_ratio_controlled_change_overrides_a_stored_drag() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:split");

        resolve_split_ratio(&widget_state, &test_split(0.5));
        apply_split_divider_drag(&widget_state, id, 0, 0.7);

        let split = test_split(0.3);
        let resolved = resolve_split_ratio(&widget_state, &split);
        assert_eq!(
            resolved.ratio, 0.3,
            "an app explicitly supplying a new ratio must override the stored drag"
        );
    }

    /// Uncontrolled (`None`) resolves to `0.0` the first time an id is
    /// seen, same "never touched = default" rule every other
    /// uncontrolled-by-default field in this module follows.
    #[test]
    fn resolve_scroll_offset_defaults_to_zero_when_never_set() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:scroll");
        assert_eq!(resolve_scroll_offset(&widget_state, id, None, 100.0), 0.0);
    }

    /// `Some(offset)` pins the controlled position outright, and is
    /// written back so a later uncontrolled frame resumes from it rather
    /// than re-seeding `0.0` -- mirroring
    /// [`apply_split_divider_drag_persists_across_an_uncontrolled_render`]'s
    /// own contract for `Split::ratio`.
    #[test]
    fn resolve_scroll_offset_controlled_value_is_written_back() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:scroll");
        assert_eq!(
            resolve_scroll_offset(&widget_state, id, Some(20.0), 100.0),
            20.0
        );
        assert_eq!(
            resolve_scroll_offset(&widget_state, id, None, 100.0),
            20.0,
            "an uncontrolled frame right after must resume from the pinned offset"
        );
    }

    /// The resolved offset is clamped into `[0, max_offset]` on every
    /// call, even for an already-stored (previously valid) value whose
    /// content has since shrunk -- `resolve_scroll_offset` re-clamps
    /// rather than trusting a stale stored clamp.
    #[test]
    fn resolve_scroll_offset_clamps_a_stored_value_whose_max_offset_shrank() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:scroll");
        resolve_scroll_offset(&widget_state, id, Some(90.0), 100.0);
        assert_eq!(resolve_scroll_offset(&widget_state, id, None, 30.0), 30.0);
    }

    fn test_tab_bar(tabs: Vec<bool>) -> TabBar {
        TabBar {
            id: WidgetId::new("host-test:tab-bar"),
            tabs: tabs
                .into_iter()
                .enumerate()
                .map(|(i, is_active)| quadraui::TabItem {
                    label: format!("tab {i}"),
                    is_active,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }

    /// Clicking a tab (`TabBarEvent::TabActivated`) with no app state at
    /// all — every subsequent render keeps resending the same tab marked
    /// active it always has — leaves the clicked tab active instead.
    #[test]
    fn apply_tab_activated_persists_across_an_uncontrolled_render() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tab-bar");
        let tab_bar = test_tab_bar(vec![true, false, false, false]);

        // A render always happens before a click can land on anything —
        // this seeds the baseline from the descriptor's own starting
        // active tab, the same order a real binding runs in.
        resolve_tab_bar_active(&widget_state, &tab_bar);
        apply_tab_activated(&widget_state, id, 2);

        let resolved = resolve_tab_bar_active(&widget_state, &tab_bar);
        assert!(!resolved.tabs[0].is_active);
        assert!(
            resolved.tabs[2].is_active,
            "a clicked tab must stay active across an uncontrolled render"
        );
    }

    /// An app that explicitly moves its own active tab (not merely
    /// resending the same one it always has) wins outright over whatever
    /// a previous click persisted.
    #[test]
    fn resolve_tab_bar_active_controlled_change_overrides_a_stored_click() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tab-bar");

        resolve_tab_bar_active(
            &widget_state,
            &test_tab_bar(vec![true, false, false, false]),
        );
        apply_tab_activated(&widget_state, id, 2);

        let tab_bar = test_tab_bar(vec![false, true, false, false]);
        let resolved = resolve_tab_bar_active(&widget_state, &tab_bar);
        assert!(
            resolved.tabs[1].is_active,
            "an app explicitly activating a different tab must override the stored click"
        );
        assert!(!resolved.tabs[2].is_active);
    }

    /// A tree with no tab/row ever marked active at all stays that way —
    /// clicking nothing leaves nothing active, the state every real
    /// `TabBar` that hasn't loaded its tabs yet would be in.
    #[test]
    fn resolve_tab_bar_active_with_nothing_active_and_nothing_clicked_stays_inactive() {
        let widget_state = WidgetState::new();
        let tab_bar = test_tab_bar(vec![false, false, false]);
        let resolved = resolve_tab_bar_active(&widget_state, &tab_bar);
        assert!(resolved.tabs.iter().all(|tab| !tab.is_active));
    }

    fn test_layout_tabs(tabs: Vec<bool>) -> LayoutTabs {
        let tab_bar = test_tab_bar(tabs);
        let entries = (0..tab_bar.tabs.len())
            .map(|i| LayoutTabEntry {
                id: format!("content-{i}"),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: format!("content-{i}-btn"),
                    descriptor: LeafDescriptor::Button(Button::new(
                        WidgetId::new(format!("content-{i}-btn")),
                        format!("tab {i} body"),
                    )),
                    scroll_px: None,
                })),
            })
            .collect();
        LayoutTabs {
            tab_bar,
            entries,
            selected: 0,
            spacing: Spacing::default(),
            focused_index: None,
        }
    }

    /// Clicking a tab inside a `LayoutTabs` tab group with no app state at
    /// all must move BOTH the strip's own active flag AND
    /// `LayoutTabs::selected` — a binding addresses the content pane
    /// `crate::render::paint_layout`'s `LayoutNode::Tabs` arm paints
    /// through `selected`, not through `tab_bar.tabs[*].is_active`, so
    /// resolving only the strip (`resolve_tab_bar_active` alone) would
    /// leave the content pane on the old tab while the strip highlights
    /// the new one.
    #[test]
    fn resolve_layout_tabs_active_moves_both_the_strip_and_the_content_pane() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tab-bar");
        let mut tabs = test_layout_tabs(vec![true, false, false, false]);

        // A render always happens before a click can land on anything.
        resolve_layout_tabs_active(&widget_state, &mut tabs);
        apply_tab_activated(&widget_state, id, 2);

        // The app's own tab group is never touched by the click -- this
        // resolves against the exact same "uncontrolled" descriptor as
        // every other render.
        let mut tabs = test_layout_tabs(vec![true, false, false, false]);
        resolve_layout_tabs_active(&widget_state, &mut tabs);

        assert!(
            tabs.tab_bar.tabs[2].is_active,
            "the clicked tab must stay active in the strip across an uncontrolled render"
        );
        assert!(!tabs.tab_bar.tabs[0].is_active);
        assert_eq!(
            tabs.selected, 2,
            "the content pane must follow the clicked tab, not stay on the descriptor's own tab 0"
        );
    }

    /// An app that explicitly moves its own active tab overrides a stored
    /// click for both the strip and the content pane, the `LayoutTabs`
    /// counterpart of `resolve_tab_bar_active_controlled_change_overrides_a_stored_click`.
    #[test]
    fn resolve_layout_tabs_active_controlled_change_overrides_a_stored_click() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tab-bar");

        let mut seed = test_layout_tabs(vec![true, false, false, false]);
        resolve_layout_tabs_active(&widget_state, &mut seed);
        apply_tab_activated(&widget_state, id, 2);

        let mut tabs = test_layout_tabs(vec![false, true, false, false]);
        resolve_layout_tabs_active(&widget_state, &mut tabs);
        assert!(tabs.tab_bar.tabs[1].is_active);
        assert!(!tabs.tab_bar.tabs[2].is_active);
        assert_eq!(
            tabs.selected, 1,
            "an app explicitly activating a different tab must override the stored click \
             for the content pane too"
        );
    }

    /// Closing a tab after the stored active index renumbers the stored
    /// index down by one, so it keeps pointing at the same tab. The
    /// descriptor itself never marks any tab active in any of these three
    /// tests (`descriptor_active` stays `None` throughout) — the same
    /// "uncontrolled, no app state" shape every other persistence test in
    /// this module uses, so the only thing moving the resolved index is
    /// the click/close pair under test, not a descriptor change.
    #[test]
    fn apply_tab_closed_shifts_a_stored_active_index_past_the_closed_tab() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tab-bar");

        resolve_tab_bar_active(
            &widget_state,
            &test_tab_bar(vec![false, false, false, false]),
        );
        apply_tab_activated(&widget_state, id, 2);
        apply_tab_closed(&widget_state, id, 0);

        let resolved =
            resolve_tab_bar_active(&widget_state, &test_tab_bar(vec![false, false, false]));
        assert!(
            resolved.tabs[1].is_active,
            "closing tab 0 must shift the stored active index 2 down to 1"
        );
    }

    /// Closing the stored active tab itself clears it rather than leaving
    /// it pointing at whichever tab now occupies that slot.
    #[test]
    fn apply_tab_closed_clears_a_stored_active_index_at_the_closed_tab() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tab-bar");

        resolve_tab_bar_active(
            &widget_state,
            &test_tab_bar(vec![false, false, false, false]),
        );
        apply_tab_activated(&widget_state, id, 2);
        apply_tab_closed(&widget_state, id, 2);

        let resolved =
            resolve_tab_bar_active(&widget_state, &test_tab_bar(vec![false, false, false]));
        assert!(
            resolved.tabs.iter().all(|tab| !tab.is_active),
            "closing the active tab must leave nothing active rather than resurrecting a stale index"
        );
    }

    /// Closing a tab after the stored active index leaves it untouched.
    #[test]
    fn apply_tab_closed_leaves_an_index_before_the_closed_tab_untouched() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tab-bar");

        resolve_tab_bar_active(
            &widget_state,
            &test_tab_bar(vec![false, false, false, false]),
        );
        apply_tab_activated(&widget_state, id, 1);
        apply_tab_closed(&widget_state, id, 3);

        let resolved =
            resolve_tab_bar_active(&widget_state, &test_tab_bar(vec![false, false, false]));
        assert!(resolved.tabs[1].is_active);
    }

    fn branch_row(path: TreePath, is_expanded: bool) -> TreeRow {
        TreeRow {
            is_expanded: Some(is_expanded),
            ..placeholder_tree_row(path)
        }
    }

    /// Toggling a collapsed branch row (`TreeEvent::RowToggleExpand`) with
    /// no app state at all — the descriptor keeps resending the same
    /// `is_expanded: Some(false)` it always has — leaves that row expanded
    /// instead.
    #[test]
    fn apply_tree_toggle_expand_persists_across_an_uncontrolled_render() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let path = vec![0_u16];
        let tree = TreeView {
            rows: vec![branch_row(path.clone(), false)],
            ..test_tree(1, None, 0)
        };

        // A render always happens before a click can land on a chevron —
        // this seeds the baseline from the descriptor's own starting
        // `is_expanded`, the same order a real binding runs in.
        resolve_tree_expansion(&widget_state, &tree);
        apply_tree_toggle_expand(&widget_state, id, &path);

        let resolved = resolve_tree_expansion(&widget_state, &tree);
        assert_eq!(
            resolved.rows[0].is_expanded,
            Some(true),
            "a toggled row must stay expanded across an uncontrolled render"
        );
    }

    /// An app that explicitly changes a row's `is_expanded` relative to
    /// what it last sent (not merely resending the same literal value it
    /// always has) wins outright over whatever a previous toggle
    /// persisted — even collapsing a row a toggle just expanded.
    #[test]
    fn resolve_tree_expansion_controlled_change_overrides_a_stored_toggle() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let path = vec![0_u16];
        let collapsed = TreeView {
            rows: vec![branch_row(path.clone(), false)],
            ..test_tree(1, None, 0)
        };

        resolve_tree_expansion(&widget_state, &collapsed);
        apply_tree_toggle_expand(&widget_state, id, &path);

        // The render right after the toggle still matches the baseline
        // (`false`) seeded just above, so the toggle (`true`) wins.
        let resolved = resolve_tree_expansion(&widget_state, &collapsed);
        assert_eq!(resolved.rows[0].is_expanded, Some(true));

        // The app now explicitly sends `true` itself, establishing a new
        // baseline to diverge from next.
        let tree = TreeView {
            rows: vec![branch_row(path.clone(), true)],
            ..test_tree(1, None, 0)
        };
        resolve_tree_expansion(&widget_state, &tree);

        // The app explicitly collapses this row — a genuine change from
        // its own last value, not just an unchanged resend — which must
        // win even though a toggle alone would have kept it expanded.
        let tree = TreeView {
            rows: vec![branch_row(path, false)],
            ..test_tree(1, None, 0)
        };
        let resolved = resolve_tree_expansion(&widget_state, &tree);
        assert_eq!(
            resolved.rows[0].is_expanded,
            Some(false),
            "an app explicitly re-collapsing the row must override the stored toggle"
        );
    }

    /// A leaf row (`is_expanded: None`) is never touched.
    #[test]
    fn resolve_tree_expansion_leaves_leaf_rows_untouched() {
        let widget_state = WidgetState::new();
        let tree = test_tree(3, None, 0);
        let resolved = resolve_tree_expansion(&widget_state, &tree);
        assert!(resolved.rows.iter().all(|row| row.is_expanded.is_none()));
    }

    /// Collapsing a branch row with no app state at all hides its
    /// descendant row outright, not just its own chevron -- `is_expanded`
    /// only drives the glyph upstream of this module (see
    /// `quadraui::tui::tree::draw_tree`'s own doc), so this module is the
    /// one place an uncontrolled tree's resolved collapse actually
    /// controls which rows paint.
    #[test]
    fn resolve_tree_expansion_hides_rows_under_a_resolved_collapsed_ancestor() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let parent = vec![0_u16];
        let child = vec![0_u16, 0_u16];
        let tree = TreeView {
            rows: vec![
                branch_row(parent.clone(), true),
                branch_row(child.clone(), true),
            ],
            ..test_tree(2, None, 0)
        };

        // Seeds the baseline from the descriptor's own starting
        // `is_expanded: Some(true)`, same order a real binding runs in.
        let resolved = resolve_tree_expansion(&widget_state, &tree);
        assert_eq!(
            resolved.rows.len(),
            2,
            "both rows must paint while the parent is expanded"
        );

        apply_tree_toggle_expand(&widget_state, id, &parent);

        // The descriptor still resends the exact same `Some(true), Some(true)`
        // it always has -- the "uncontrolled, no app state" shape.
        let resolved = resolve_tree_expansion(&widget_state, &tree);
        assert_eq!(
            resolved.rows.len(),
            1,
            "collapsing the parent must drop its child row, not only flip its own chevron"
        );
        assert_eq!(resolved.rows[0].path, parent);
        assert_eq!(resolved.rows[0].is_expanded, Some(false));
    }

    /// A stored toggle for a path no longer present in `tree.rows` is
    /// dropped (per this function's own pruning doc) rather than
    /// resurrected if the same path reappears later -- a later render re-
    /// adding the row reseeds its override from the descriptor's own
    /// value, not from whatever a now-stale entry remembered.
    #[test]
    fn resolve_tree_expansion_reseeds_a_pruned_then_reappearing_path_from_the_descriptor() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let path = vec![0_u16];
        let tree = TreeView {
            rows: vec![branch_row(path.clone(), false)],
            ..test_tree(1, None, 0)
        };

        resolve_tree_expansion(&widget_state, &tree);
        apply_tree_toggle_expand(&widget_state, id, &path);
        let resolved = resolve_tree_expansion(&widget_state, &tree);
        assert_eq!(resolved.rows[0].is_expanded, Some(true));

        // The row disappears entirely for a frame (e.g. an ancestor the
        // app itself collapsed) -- the pruning branch drops its entry.
        let empty_tree = TreeView {
            rows: Vec::new(),
            ..test_tree(1, None, 0)
        };
        resolve_tree_expansion(&widget_state, &empty_tree);

        // The row reappears with the descriptor's own original value.
        // Without the prune, a stale entry would still say `true`.
        let resolved = resolve_tree_expansion(&widget_state, &tree);
        assert_eq!(
            resolved.rows[0].is_expanded,
            Some(false),
            "a path pruned while absent must reseed from the descriptor, not resurrect the \
             toggle it had before it disappeared"
        );
    }

    /// `resolve_tree_expansion` and `resolve_tree_cursor`/`apply_tree_nav_key`
    /// both persist state for the same tree `id` in the one `WidgetState`
    /// store, which holds exactly one typed value per id — this asserts
    /// they don't clobber each other's entry when both are used for the
    /// same tree in the same frame, the regression `expanded_state_id`'s
    /// own doc exists to prevent.
    #[test]
    fn tree_expansion_and_tree_cursor_state_coexist_for_the_same_tree_id() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let path = vec![0_u16];
        let tree = TreeView {
            rows: vec![branch_row(path.clone(), false)],
            ..test_tree(1, None, 0)
        };

        resolve_tree_expansion(&widget_state, &tree);
        apply_tree_toggle_expand(&widget_state, id, &path);

        let row_paths = vec![path.clone()];
        assert!(
            apply_tree_nav_key(&widget_state, id, &row_paths, 4, &down_key()),
            "cursor navigation for the same id must still work"
        );

        let cursor = resolve_tree_cursor(&widget_state, &test_tree(1, None, 0), 4);
        assert_eq!(
            cursor.selected_path,
            Some(path.clone()),
            "the cursor move must have survived the expand-state calls for the same id"
        );

        let expansion = resolve_tree_expansion(&widget_state, &tree);
        assert_eq!(
            expansion.rows[0].is_expanded,
            Some(true),
            "the toggled expand state must have survived the cursor-nav calls for the same id"
        );
    }

    /// Nothing stored for an id under any shape — e.g. one that was never
    /// resolved/applied this process — snapshots to `None`.
    #[test]
    fn widget_state_snapshot_is_none_for_an_untouched_id() {
        let widget_state = WidgetState::new();
        assert_eq!(
            widget_state_snapshot(&widget_state, WidgetId::new("host-test:untouched")),
            None
        );
    }

    /// A tree's cursor + scroll read back exactly as `resolve_tree_cursor`
    /// left them stored.
    #[test]
    fn widget_state_snapshot_reads_back_a_tree_cursor() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let tree = test_tree(10, Some(vec![3_u16]), 0);
        resolve_tree_cursor(&widget_state, &tree, 4);

        let snapshot = widget_state_snapshot(&widget_state, id).expect("tree cursor stored");
        assert_eq!(snapshot.selected_path, Some(vec![3_u16]));
        assert_eq!(snapshot.selected_idx, None);
        assert!(snapshot.scroll_offset.is_some());
        assert_eq!(snapshot.ratio, None);
        assert_eq!(snapshot.active_tab, None);
        assert_eq!(snapshot.expanded, None);
    }

    /// A list's cursor + scroll read back exactly as `resolve_list_cursor`
    /// left them stored.
    #[test]
    fn widget_state_snapshot_reads_back_a_list_cursor() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:list");
        let list = test_list(10, 2, 0);
        resolve_list_cursor(&widget_state, &list, 4);

        let snapshot = widget_state_snapshot(&widget_state, id).expect("list cursor stored");
        assert_eq!(snapshot.selected_idx, Some(2));
        assert_eq!(snapshot.selected_path, None);
        assert!(snapshot.scroll_offset.is_some());
    }

    /// A split's ratio reads back exactly as `resolve_split_ratio` left it
    /// stored, once a drag has moved it off the descriptor's own literal.
    #[test]
    fn widget_state_snapshot_reads_back_a_split_ratio() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:split");
        resolve_split_ratio(&widget_state, &test_split(0.5));
        apply_split_divider_drag(&widget_state, id, 0, 0.7);

        let snapshot = widget_state_snapshot(&widget_state, id).expect("split ratio stored");
        assert_eq!(snapshot.ratio, Some(0.7));
        assert_eq!(snapshot.selected_path, None);
    }

    /// A tab bar's active tab reads back exactly as `apply_tab_activated`
    /// left it stored.
    #[test]
    fn widget_state_snapshot_reads_back_an_active_tab() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tab-bar");
        resolve_tab_bar_active(&widget_state, &test_tab_bar(vec![true, false]));
        apply_tab_activated(&widget_state, id, 1);

        let snapshot = widget_state_snapshot(&widget_state, id).expect("active tab stored");
        assert_eq!(snapshot.active_tab, Some(1));
    }

    /// A tree's expanded-row set reads back exactly as
    /// `apply_tree_toggle_expand` left it stored, alongside (not instead
    /// of) that same tree's cursor — the id and its derived expansion key
    /// coexist in one snapshot since both can be live for the same tree.
    #[test]
    fn widget_state_snapshot_reads_back_an_expanded_set_alongside_the_cursor() {
        let widget_state = WidgetState::new();
        let id = WidgetId::new("host-test:tree");
        let path = vec![0_u16];
        let tree = TreeView {
            rows: vec![branch_row(path.clone(), false)],
            selected_path: Some(path.clone()),
            ..test_tree(1, None, 0)
        };
        resolve_tree_expansion(&widget_state, &tree);
        apply_tree_toggle_expand(&widget_state, id, &path);
        resolve_tree_cursor(&widget_state, &tree, 4);

        let snapshot = widget_state_snapshot(&widget_state, id).expect("tree state stored");
        assert_eq!(snapshot.expanded, Some(vec![path.clone()]));
        assert_eq!(snapshot.selected_path, Some(path));
    }

    /// The shared slot/extra deprecation message names both the deprecated
    /// method and the inline-leaf guidance a binding's own leaf passes in.
    #[test]
    fn slot_deprecation_message_names_the_method_and_the_guidance() {
        let message = BindingHost::slot_deprecation_message(
            "insert_list",
            "render an inline `list` layout leaf instead",
        );
        assert!(message.contains("insert_list"));
        assert!(message.contains("render an inline `list` layout leaf instead"));
    }

    /// Every fixed `SLOT_*` constant resolves to its own layout-leaf kind
    /// name; an id outside the six resolves to `None`.
    #[test]
    fn slot_kind_name_covers_every_fixed_slot() {
        assert_eq!(BindingHost::slot_kind_name(SLOT_BUTTON), Some("button"));
        assert_eq!(
            BindingHost::slot_kind_name(SLOT_TEXT_DISPLAY),
            Some("text_display")
        );
        assert_eq!(BindingHost::slot_kind_name(SLOT_LIST), Some("list"));
        assert_eq!(BindingHost::slot_kind_name(SLOT_TREE), Some("tree"));
        assert_eq!(
            BindingHost::slot_kind_name(SLOT_DATA_TABLE),
            Some("data_table")
        );
        assert_eq!(BindingHost::slot_kind_name(SLOT_TAB_BAR), Some("tab_bar"));
        assert_eq!(BindingHost::slot_kind_name(999), None);
    }

    /// [`BindingHost::first_deprecation_report`] reports `true` only the
    /// first time a given method name is passed for this host, `false` on
    /// every call after -- and independently per method name.
    #[test]
    fn first_deprecation_report_is_true_only_once_per_method_per_host() {
        let host = BindingHost::default();
        assert!(host.first_deprecation_report("insert_list"));
        assert!(!host.first_deprecation_report("insert_list"));
        assert!(!host.first_deprecation_report("insert_list"));
        // A different method name on the same host still reports `true`
        // its own first time.
        assert!(host.first_deprecation_report("slot_clear"));
        assert!(!host.first_deprecation_report("slot_clear"));

        // A fresh host starts with a clean slate.
        let other_host = BindingHost::default();
        assert!(other_host.first_deprecation_report("insert_list"));
    }
}
