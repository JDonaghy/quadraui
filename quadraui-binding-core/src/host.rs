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

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Serialize;

use quadraui::compose::HoverTooltipTracker;
use quadraui::tui::testing::TuiDriver;
use quadraui::Rect;
use quadraui::{
    mouse_down, mouse_up, Accelerator, AcceleratorId, AppLogic, Backend, Button, DataTable,
    Decoration, Key, ListView, Modifiers, MouseButton, Reaction, Style, StyledSpan, TabBar,
    TextDisplay, TextDisplayLine, Theme, Tooltip, TreeView, UiEvent, WidgetId,
};

use crate::engine::InteractionEngine;
use crate::extra_paint::ExtraPaint;
use crate::hit::PaintedHit;
use crate::render::RenderRefs;

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
    /// rather than at construction). A caller that wants a fully
    /// synthetic, no-real-time-elapses clock instead — the way this
    /// crate's own tests and `quadraui::compose::HoverTooltipTracker`'s
    /// module doc both demonstrate — calls [`Self::drive_hover_tooltip`]
    /// with its own hand-picked `now_ms` rather than this method's value.
    pub fn now_ms(&self) -> u64 {
        let epoch = self.tooltip_clock.get().unwrap_or_else(|| {
            let started = Instant::now();
            self.tooltip_clock.set(Some(started));
            started
        });
        epoch.elapsed().as_millis() as u64
    }

    /// The hover tooltip text for `id`, if any of this host's slots that
    /// carry one names it — today, only [`Self::button`]'s own
    /// `Button::tooltip`. [`Self::drive_hover_tooltip`] uses this to
    /// decide whether a hovered widget has anything to show at all before
    /// arming [`Self::tooltip_tracker`] for it; a widget with no tooltip
    /// text never starts the delay.
    pub fn hover_tooltip_text(&self, id: WidgetId) -> Option<String> {
        self.button
            .as_ref()
            .filter(|button| button.id == id)
            .and_then(|button| button.tooltip.clone())
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
    /// [`quadraui::Style::tooltip_delay_ms`].
    ///
    /// Resyncs first: if [`Self::tooltip_tracker`] still thinks a tooltip
    /// is visible but `engine`'s own tooltip has already closed (e.g.
    /// [`InteractionEngine::handle`]'s own click/key auto-dismiss ran
    /// since the last call), the tracker is explicitly dismissed too, so
    /// a held-still hover over the same widget restarts the delay instead
    /// of believing a tooltip is still showing that the screen no longer
    /// has.
    ///
    /// Returns the [`Reaction`] the caller should merge into its own:
    /// [`Reaction::Redraw`] when a tooltip opened or closed this call,
    /// [`Reaction::RedrawAfter`] (via [`HoverTooltipTracker::next_wake_ms`])
    /// while one is still pending, [`Reaction::Continue`] otherwise.
    pub fn drive_hover_tooltip(
        &mut self,
        engine: &mut InteractionEngine,
        now_ms: u64,
        delay_ms: u64,
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

        if self.tooltip_tracker.tick(delay_ms, now_ms) {
            if let Some((id, text)) = hovered {
                if let Some(anchor) = Self::hover_anchor(engine, id) {
                    engine.show_tooltip(Tooltip::new(id, text), anchor);
                    reaction = Reaction::Redraw;
                }
            }
        }

        if let Some(remaining_ms) = self.tooltip_tracker.next_wake_ms(now_ms, delay_ms) {
            reaction = reaction.merge(Reaction::RedrawAfter(Duration::from_millis(remaining_ms)));
        }

        reaction
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
    use quadraui::{
        AppLogic, Backend, Button, ButtonMask, Point, Reaction, Rect, TextInput, Toast,
        ToastSeverity, UiEvent,
    };

    use super::*;
    use crate::engine::EventSink;
    use crate::render::draw_bands;

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
    struct HoverHostLogic {
        host: BindingHost,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
        clock_ms: Rc<Cell<u64>>,
        delay_ms: u64,
    }

    impl AppLogic for HoverHostLogic {
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
            reaction.merge(
                self.host
                    .drive_hover_tooltip(&mut self.engine, now, self.delay_ms),
            )
        }

        fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
            let now = self.clock_ms.get();
            self.host
                .drive_hover_tooltip(&mut self.engine, now, self.delay_ms)
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
}
