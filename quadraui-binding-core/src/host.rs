//! The binding **host** layer: everything a native binding (`quadraui-py`
//! today, a napi leaf next, #1126) needs around [`crate::engine::InteractionEngine`]
//! to drive a [`quadraui::AppLogic`], but that still carries zero
//! host-language FFI type. A binding's own `AppLogic` impl holds one
//! [`BindingHost`] (the six fixed primitive slots, any number of additive
//! slots, and the backend-settings replay store) plus its own callback
//! fields (a Python callable, a JS function value, ...) and an
//! [`crate::engine::InteractionEngine`]; [`render_overlays`] is the one
//! render-time call every binding's `AppLogic::render` makes after painting
//! its own slots, so the anchored-overlay z-order (dialog > float > menus >
//! toast > tooltip, in the exact order below) is written down once instead
//! of copy-pasted per binding.
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

#[cfg(all(feature = "macos", not(feature = "gtk")))]
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use serde::de::DeserializeOwned;
use serde::Serialize;

use quadraui::tui::testing::TuiDriver;
#[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
use quadraui::Rect;
use quadraui::{
    mouse_down, mouse_up, Accelerator, AcceleratorId, AppLogic, Backend, Button, DataTable,
    Decoration, Key, ListView, Modifiers, MouseButton, Reaction, Style, StyledSpan, TabBar,
    TextDisplay, TextDisplayLine, Theme, TreeView, UiEvent, WidgetId,
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
        }
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use quadraui::backend::MessageDialogOptions;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{AppLogic, Backend, Button, Reaction, Rect, Toast, ToastSeverity, UiEvent};

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
}
