//! PyO3 bindings over `quadraui`'s headless TUI driver, plus the real
//! terminal and native-window runners that take it over.
//!
//! [`QuiApp`] wraps a `quadraui::tui::testing::TuiDriver` running an
//! in-crate [`AppLogic`] that holds one optional descriptor per core
//! primitive (button, text display, list, tree, data table, tab bar). Each
//! `insert_*` method takes a Python `dict`, decodes it into the matching
//! descriptor type via [`pythonize::depythonize`], stores it in that
//! primitive's slot, and re-renders immediately so `screen()`/
//! `screen_contains()` reflect it without a separate step.
//!
//! `push_event`/`push_key_event` queue a [`UiEvent`] (decoded from a Python
//! dict, or synthesized from a key code); `step`/`run` drain that queue
//! through [`PyAppLogic::handle`], releasing the GIL for the run loop itself
//! and re-acquiring it only for the duration of each registered Python
//! callback (`set_handler`/`set_tick_handler`/`set_render`). An exception
//! raised inside a callback is captured and re-raised out of `step`/`run`
//! rather than lost. A callback can also request exit without raising, by
//! returning `False` (see [`reaction_from_return`]) — useful for a handler
//! that wants to stop the loop as part of its normal control flow rather
//! than via an exception.
//!
//! A registered event handler runs while the run loop holds the one
//! mutable borrow of this app's [`PyAppLogic`] for the whole step, so it
//! cannot call back into `QuiApp.insert_*`/`slot_clear` on the same
//! instance — PyO3's runtime borrow guard rejects that second borrow. The
//! `render` callback (`QuiApp(..., render=...)` or `set_render`) is the way
//! around that: it takes no arguments and returns a `dict` mapping slot
//! names (`"button"`, `"text_display"`, `"list"`, `"tree"`, `"data_table"`,
//! `"tab_bar"`) to a descriptor `dict` (or `None`/omitted to draw nothing in
//! that slot this frame). [`PyAppLogic::render`] calls it once per frame —
//! the same frame the runner already repaints after every dispatched event
//! or tick — so an app keeps its live state in a plain Python object the
//! event handler mutates directly and the render callback reads back out,
//! instead of mutating `QuiApp` itself. The render callback must not call
//! back into `QuiApp` either; like the event handler, it only returns data.
//!
//! Every error this binding can raise itself (as opposed to one a
//! registered Python callback raised, which passes through unchanged) is a
//! [`QuadrauiError`] subclass — see the `errors` section below. A Rust
//! panic reaching a driver call is caught at the [`QuiApp::guarded`]
//! boundary, raised as [`PanicError`] instead of unwinding into Python, and
//! poisons the handle so every later call raises [`PoisonedError`] rather
//! than touching a driver that may have panicked mid-mutation.
//!
//! When built with the `gtk`, `macos`, or `win` feature, [`QuiApp::run_native`]
//! opens a real OS window instead of driving the headless driver: it moves
//! this app's whole [`PyAppLogic`] (wrapped in [`NativeAppLogic`]) into a
//! `quadraui::gtk::GtkRunner`, `quadraui::macos::MacRunner`, or
//! `quadraui::win::WinRunner` respectively (`gtk` wins over `macos`, and
//! both win over `win`, when more than one is enabled on the same build —
//! only one is ever enabled for real, since each targets a different OS),
//! pumped from a loop that also releases the GIL for the duration of each
//! wait and re-acquires it per callback, the same discipline `run` uses for
//! the headless loop. `GtkRunner` calls `activate` directly instead of
//! parsing the host process's own argv — the right entry point for an
//! embedded interpreter, which has no `gio::Application`-shaped argv of its
//! own to hand over. `MacRunner` requires its caller to already be on the
//! main thread (a `MainThreadMarker` check `run_native` makes itself,
//! raising [`NotOnMainThreadError`] rather than letting the AppKit call
//! beneath it panic off-thread). `WinRunner::new_with`/`pump` need no
//! equivalent preflight — Win32 has no display/main-thread precondition
//! this binding has to check up front — but both are `unsafe`, with a
//! single-thread construct-and-step contract `run_native`'s pump loop
//! satisfies simply by doing both inside the same `py.detach` closure.
//!
//! [`QuiApp::run_tui`] is `run_native`'s terminal counterpart: it moves this
//! app's whole [`PyAppLogic`] into a real `quadraui::tui::TuiRunner`, which
//! takes over the controlling terminal (raw mode, alternate screen, real
//! crossterm keyboard/resize events) and restores it on exit — no new Cargo
//! feature gates it, because the `quadraui` dependency's `tui` feature
//! (which already compiles in both `testing::TuiDriver` and the real
//! `TuiRunner`) is unconditionally on for this whole crate; the headless
//! driver `QuiApp` otherwise wraps needs it too.
use std::any::Any;
#[cfg(all(feature = "macos", not(feature = "gtk")))]
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;
use std::time::Duration;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use quadraui::tui::testing::TuiDriver;
use quadraui::tui::{StepOutcome, TuiRunner};
use quadraui::{
    AppLogic, Backend, Button, DataTable, Key, ListView, Modifiers, Reaction, Rect, TabBar,
    TextDisplay, TreeView, UiEvent,
};

create_exception!(
    _quadraui,
    QuadrauiError,
    PyException,
    "Base class for every exception this binding raises itself. Mirrors \
    the `QUI_ERR_*` taxonomy `quadraui-ffi` exposes at its C boundary -- a \
    specific subclass corresponds to one `QUI_ERR_*` code."
);

create_exception!(
    _quadraui,
    InvalidDescriptorError,
    QuadrauiError,
    "A descriptor dict passed to `push_event` or an `insert_*` method did \
    not decode into the shape it was expected to have (mirrors \
    `QUI_ERR_INVALID_JSON`)."
);

create_exception!(
    _quadraui,
    SlotOccupiedError,
    QuadrauiError,
    "An `insert_*` method was called on a slot that already holds a \
    descriptor (mirrors `QUI_ERR_SLOT_OCCUPIED`)."
);

create_exception!(
    _quadraui,
    UnknownSlotError,
    QuadrauiError,
    "`slot_clear` was given an id that isn't one of this app's six fixed \
    primitive slots (mirrors `QUI_ERR_UNKNOWN_SLOT`)."
);

create_exception!(
    _quadraui,
    InvalidDimensionsError,
    QuadrauiError,
    "A `width`/`height` argument to `QuiApp` or `resize` was zero \
    (mirrors `QUI_ERR_INVALID_DIMENSIONS`)."
);

create_exception!(
    _quadraui,
    InvalidCharError,
    QuadrauiError,
    "`push_key_event` was given a code point that is not a Unicode scalar \
    value (mirrors `QUI_ERR_INVALID_CHAR`)."
);

create_exception!(
    _quadraui,
    PanicError,
    QuadrauiError,
    "A Rust panic was caught at the binding boundary; the `QuiApp` it \
    happened on is now poisoned (mirrors `QUI_ERR_PANIC`)."
);

create_exception!(
    _quadraui,
    PoisonedError,
    QuadrauiError,
    "A `QuiApp` is unusable because an earlier call on it panicked and \
    was caught (mirrors `QUI_ERR_POISONED`)."
);

create_exception!(
    _quadraui,
    InternalError,
    QuadrauiError,
    "A failure internal to this binding rather than caused by a caller's \
    input, e.g. being unable to convert an event back into a Python value \
    (mirrors `QUI_ERR_INTERNAL`)."
);

create_exception!(
    _quadraui,
    NativeAlreadyRunError,
    QuadrauiError,
    "`run_native` was called a second time on a `QuiApp` that already ran \
    it once. The first call moves this app's handlers and descriptors into \
    the native window it opens; once that window closes, this `QuiApp` has \
    nothing left to show, so a second call raises instead of silently \
    opening an empty window with no handlers."
);

create_exception!(
    _quadraui,
    NoDisplayError,
    QuadrauiError,
    "On the `gtk` feature, `run_native` was called with no display \
    reachable (no `DISPLAY`/`WAYLAND_DISPLAY`). Raised instead of letting \
    GTK's own init path fail in a way this binding can't catch. macOS has \
    no equivalent preflight — see `NotOnMainThreadError` for the one \
    precondition that feature does check up front."
);

create_exception!(
    _quadraui,
    NotOnMainThreadError,
    QuadrauiError,
    "On the `macos` feature, `run_native` was called from a thread other \
    than the one the interpreter started on. AppKit only opens windows on \
    the main thread; `MacRunner::new_with` enforces this with an \
    `expect()` that panics instead of degrading gracefully, so this \
    binding checks `objc2::MainThreadMarker::new()` itself first and \
    raises this instead of letting that panic cross the PyO3 boundary."
);

create_exception!(
    _quadraui,
    InvalidAppIdError,
    QuadrauiError,
    "`run_native`'s `app_id` argument is not a syntactically valid \
    `GApplication` id (see `quadraui::gtk::app_id_is_valid`). Raised \
    instead of letting GLib's own `g_application_id_is_valid` assertion \
    trip an uncatchable `GLib-GIO-CRITICAL` once the id reaches \
    `Application::builder()`."
);

/// Slot id of the app's `Button` descriptor.
const SLOT_BUTTON: i64 = 0;
/// Slot id of the app's `TextDisplay` descriptor.
const SLOT_TEXT_DISPLAY: i64 = 1;
/// Slot id of the app's `ListView` descriptor.
const SLOT_LIST: i64 = 2;
/// Slot id of the app's `TreeView` descriptor.
const SLOT_TREE: i64 = 3;
/// Slot id of the app's `DataTable` descriptor.
const SLOT_DATA_TABLE: i64 = 4;
/// Slot id of the app's `TabBar` descriptor.
const SLOT_TAB_BAR: i64 = 5;

/// The [`AppLogic`] a [`QuiApp`] drives: a bag of optional descriptors, one
/// per core primitive, painted top-to-bottom in fixed-height bands — unless
/// `render_callback` is registered, in which case each frame asks it for a
/// fresh set of descriptors instead of reading these static fields.
#[derive(Default)]
struct PyAppLogic {
    button: Option<Button>,
    text_display: Option<TextDisplay>,
    list: Option<ListView>,
    tree: Option<TreeView>,
    data_table: Option<DataTable>,
    tab_bar: Option<TabBar>,
    /// Registered via `QuiApp::set_handler` / the `on_event` constructor
    /// argument; called with the pythonized event payload for every
    /// dispatched `UiEvent`.
    handler: Option<Py<PyAny>>,
    /// Registered via `QuiApp::set_tick_handler`; called with no arguments
    /// whenever the runner advances with no event queued.
    on_tick: Option<Py<PyAny>>,
    /// Registered via `QuiApp::set_render` / the `render` constructor
    /// argument; called with no arguments once per frame and expected to
    /// return a `dict` mapping slot names to a descriptor `dict` (or
    /// `None`/omitted to draw nothing in that slot). When set, this
    /// overrides the static `button`/`text_display`/.../`tab_bar` fields
    /// above for every render — it is the one way an app's displayed state
    /// can change after the runner has taken over the event loop, since the
    /// event handler that mutates that state cannot itself re-borrow this
    /// `QuiApp` to call `insert_*`.
    render_callback: Option<Py<PyAny>>,
    /// Set by `handle`/`tick` when the Python callback they invoke raises.
    /// `step_once` takes this and re-raises it out of `step`/`run` instead
    /// of letting it vanish at the `AppLogic` boundary.
    ///
    /// `Rc<RefCell<_>>` rather than a bare `Option<PyErr>` so `run_native`
    /// can keep reading it after handing this whole `PyAppLogic` by value
    /// to `quadraui::gtk::run_with` — it clones the `Rc` first, and that
    /// clone keeps observing writes `handle`/`tick` make to the moved
    /// original for as long as the native loop runs.
    pending_error: Rc<RefCell<Option<PyErr>>>,
}

impl PyAppLogic {
    fn slot_occupied(&self, slot: i64) -> bool {
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

    /// Whether `slot` names one of this app's fixed primitive slots.
    fn is_known_slot(slot: i64) -> bool {
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
    fn clear_slot(&mut self, slot: i64) -> bool {
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
}

/// Maps a registered handler/tick callback's return value to a [`Reaction`]:
/// returning `False` requests the same exit a raised exception would, with
/// no exception needed; returning anything else (including the common case
/// of no explicit `return`, i.e. `None`) continues. Checked with
/// `extract::<bool>`, which only matches an actual Python `bool` — an int
/// `0`, an empty string, or any other "falsy" value keeps running rather
/// than exiting, so only an explicit `return False` opts in.
fn reaction_from_return(value: &Bound<'_, PyAny>) -> Reaction {
    match value.extract::<bool>() {
        Ok(false) => Reaction::Exit,
        _ => Reaction::Continue,
    }
}

/// Wraps [`reaction_from_return`], promoting an ordinary `Continue` to
/// `Redraw` whenever `render_callback_present` — i.e. a render callback is
/// registered on this app.
///
/// A render callback only runs when the runner decides a frame needs
/// repainting (`AppLogic::render` is reached only after an
/// `EventOutcome::Redraw`/`RedrawAfter`-driven repaint — see
/// `quadraui::tui::run::TuiRunner::pump` and the GTK/macOS/Windows
/// equivalents). `PyAppLogic::handle`/`tick` have no way to tell whether a
/// Python handler's mutation of its own plain-Python state changed
/// anything visible, so once a render callback is registered every
/// non-exit dispatch is treated as one that might have — the same
/// "redraw every frame, let the render callback decide what that looks
/// like" contract a React/Electron-style `render(state)` callback expects.
/// Without this, a handler returning `None` (the common case: no explicit
/// `return`) maps to `Reaction::Continue`, the runner never calls
/// `AppLogic::render` again, and the render callback never re-runs — the
/// screen stays frozen at whatever the last repaint drew.
fn reaction_for_callback(value: &Bound<'_, PyAny>, render_callback_present: bool) -> Reaction {
    match reaction_from_return(value) {
        Reaction::Continue if render_callback_present => Reaction::Redraw,
        other => other,
    }
}

/// Records `err` into `pending_error`, but only if no earlier error in the
/// same dispatch already claimed the slot. Without this, a handler raise
/// (recorded first) followed by the render callback also raising while
/// painting the same frame would have the render error silently clobber
/// the handler's — the one a caller actually wants to see, since it is
/// closer to the root cause.
fn record_error(pending_error: &Rc<RefCell<Option<PyErr>>>, err: PyErr) {
    let mut slot = pending_error.borrow_mut();
    if slot.is_none() {
        *slot = Some(err);
    }
}

impl AppLogic for PyAppLogic {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let Some(callback) = self.render_callback.as_ref() else {
            draw_bands(
                backend,
                RenderRefs {
                    button: self.button.as_ref(),
                    text_display: self.text_display.as_ref(),
                    list: self.list.as_ref(),
                    tree: self.tree.as_ref(),
                    data_table: self.data_table.as_ref(),
                    tab_bar: self.tab_bar.as_ref(),
                },
            );
            return;
        };
        if let Some(frame) = render_frame_from_callback(callback, &self.pending_error) {
            draw_bands(backend, frame.as_refs());
        }
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let Some(handler) = self.handler.as_ref() else {
            return Reaction::Continue;
        };
        let render_callback_present = self.render_callback.is_some();
        Python::attach(|py| {
            let payload = match pythonize::pythonize(py, &event) {
                Ok(value) => value,
                Err(err) => {
                    record_error(
                        &self.pending_error,
                        InternalError::new_err(format!("failed to pythonize event: {err}")),
                    );
                    return Reaction::Exit;
                }
            };
            match handler.call1(py, (payload,)) {
                Ok(result) => reaction_for_callback(result.bind(py), render_callback_present),
                Err(err) => {
                    record_error(&self.pending_error, err);
                    Reaction::Exit
                }
            }
        })
    }

    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        let Some(on_tick) = self.on_tick.as_ref() else {
            return Reaction::Continue;
        };
        let render_callback_present = self.render_callback.is_some();
        Python::attach(|py| match on_tick.call0(py) {
            Ok(result) => reaction_for_callback(result.bind(py), render_callback_present),
            Err(err) => {
                record_error(&self.pending_error, err);
                Reaction::Exit
            }
        })
    }
}

/// One descriptor reference per slot, borrowed either from `PyAppLogic`'s
/// static fields or from a [`RenderFrame`] just decoded from a render
/// callback's return value — the one shape [`draw_bands`] paints from,
/// regardless of which path produced it.
struct RenderRefs<'a> {
    button: Option<&'a Button>,
    text_display: Option<&'a TextDisplay>,
    list: Option<&'a ListView>,
    tree: Option<&'a TreeView>,
    data_table: Option<&'a DataTable>,
    tab_bar: Option<&'a TabBar>,
}

/// Paints each present slot top-to-bottom in a fixed-height band, skipping
/// any slot once the viewport runs out of room.
fn draw_bands(backend: &mut dyn Backend, refs: RenderRefs<'_>) {
    let viewport = backend.viewport();
    // Band heights below are given in text LINES, not raw viewport units: a
    // button is 3 lines tall, a tree 4. `viewport` and `line_height` share
    // the backend's own unit (cells on TUI, where `line_height` is 1.0;
    // device pixels on the native backends, where it is the font's pixel line
    // height), so scaling the line count by `line_height` lays the bands out
    // correctly on every backend instead of collapsing them to a few pixels
    // tall on native.
    let line_height = backend.line_height();
    let mut y = 0.0_f32;
    let mut next_band = |lines: f32| -> Option<Rect> {
        let remaining = viewport.height - y;
        if remaining <= 0.0 {
            return None;
        }
        let band_height = (lines * line_height).min(remaining);
        let rect = Rect::new(0.0, y, viewport.width, band_height);
        y += band_height;
        Some(rect)
    };

    if let Some(button) = refs.button {
        if let Some(rect) = next_band(3.0) {
            backend.draw_button(rect, button);
        }
    }
    if let Some(text_display) = refs.text_display {
        if let Some(rect) = next_band(3.0) {
            backend.draw_text_display(rect, text_display);
        }
    }
    if let Some(list) = refs.list {
        if let Some(rect) = next_band(4.0) {
            backend.draw_list(rect, list);
        }
    }
    if let Some(tree) = refs.tree {
        if let Some(rect) = next_band(4.0) {
            backend.draw_tree(rect, tree);
        }
    }
    if let Some(data_table) = refs.data_table {
        if let Some(rect) = next_band(4.0) {
            backend.draw_data_table(rect, data_table, None);
        }
    }
    if let Some(tab_bar) = refs.tab_bar {
        if let Some(rect) = next_band(3.0) {
            backend.draw_tab_bar_layout(rect, tab_bar, None);
        }
    }
}

/// Owned decode of a render callback's returned `dict` — one optional
/// descriptor per slot, keyed by `"button"`, `"text_display"`, `"list"`,
/// `"tree"`, `"data_table"`, and `"tab_bar"`.
#[derive(Default)]
struct RenderFrame {
    button: Option<Button>,
    text_display: Option<TextDisplay>,
    list: Option<ListView>,
    tree: Option<TreeView>,
    data_table: Option<DataTable>,
    tab_bar: Option<TabBar>,
}

impl RenderFrame {
    fn as_refs(&self) -> RenderRefs<'_> {
        RenderRefs {
            button: self.button.as_ref(),
            text_display: self.text_display.as_ref(),
            list: self.list.as_ref(),
            tree: self.tree.as_ref(),
            data_table: self.data_table.as_ref(),
            tab_bar: self.tab_bar.as_ref(),
        }
    }

    /// Decodes a render callback's returned `dict`, raising
    /// `InvalidDescriptorError` if it isn't a `dict`, if an entry isn't a
    /// `dict`/`None`, or if an entry's shape doesn't match its slot's
    /// descriptor type. The exact recognized key set is `"button"`,
    /// `"text_display"`, `"list"`, `"tree"`, `"data_table"`, `"tab_bar"` —
    /// one per fixed slot, same names `insert_*`'s Rust-side `what` argument
    /// uses. Any other key (including a near-miss typo like `"tabbar"`) is
    /// ignored rather than rejected, so a render callback built against a
    /// future slot set stays forward-compatible with this one.
    fn from_dict(dict: &Bound<'_, PyDict>) -> PyResult<Self> {
        Ok(RenderFrame {
            button: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "button")?,
            text_display: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "text_display")?,
            list: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "list")?,
            tree: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "tree")?,
            data_table: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "data_table")?,
            tab_bar: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "tab_bar")?,
        })
    }
}

/// The `what` label passed to [`decode_slot_entry`] for every slot of a
/// render callback's returned `dict` — identifies the source of a decode
/// error as "render" (vs. an `insert_*` call) in the raised
/// `InvalidDescriptorError`'s message.
const RENDER_CALLBACK_WHAT: &str = "render";

/// Reads and decodes one optional slot entry from a render callback's
/// returned `dict`: a missing key or a `None` value both mean "draw nothing
/// in this slot this frame"; anything else must be a `dict` decodable into
/// `T`.
fn decode_slot_entry<T: serde::de::DeserializeOwned>(
    what: &str,
    dict: &Bound<'_, PyDict>,
    key: &str,
) -> PyResult<Option<T>> {
    match dict.get_item(key)? {
        Some(value) if !value.is_none() => {
            let value_dict = value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!("{what}: {key:?} must be a dict or None"))
            })?;
            Ok(Some(decode(what, value_dict)?))
        }
        _ => Ok(None),
    }
}

/// Invokes a registered render callback with no arguments and decodes its
/// returned `dict` into a [`RenderFrame`]. On any failure — the callback
/// raising, returning something other than a `dict`, or an entry with the
/// wrong shape — records the error in `pending_error` and returns `None`,
/// leaving the caller to skip drawing this frame rather than paint a
/// half-decoded one.
///
/// Every call site that can trigger a render — `step`/`run`, `screen`,
/// `screen_contains`, `insert_*`, and `slot_clear` — drains `pending_error`
/// after rendering and re-raises it, so a render-callback failure surfaces
/// the same way a `handle`/`tick` callback failure does no matter which of
/// those triggered the repaint. `run_tui`/`run_native` additionally break
/// their loop as soon as `pending_error` is set, since nothing else is
/// polling it on the Python side while the GIL is detached.
fn render_frame_from_callback(
    callback: &Py<PyAny>,
    pending_error: &Rc<RefCell<Option<PyErr>>>,
) -> Option<RenderFrame> {
    Python::attach(|py| {
        let result = match callback.call0(py) {
            Ok(result) => result,
            Err(err) => {
                record_error(pending_error, err);
                return None;
            }
        };
        let bound = result.bind(py);
        let dict = match bound.cast::<PyDict>() {
            Ok(dict) => dict,
            Err(_) => {
                record_error(
                    pending_error,
                    InvalidDescriptorError::new_err("render callback must return a dict"),
                );
                return None;
            }
        };
        match RenderFrame::from_dict(dict) {
            Ok(frame) => Some(frame),
            Err(err) => {
                record_error(pending_error, err);
                None
            }
        }
    })
}

/// Wraps [`PyAppLogic`] for `run_native`'s native runner loop (`GtkRunner`
/// on the `gtk` feature, `MacRunner` on the `macos` feature, `WinRunner` on
/// the `win` feature), forcing a `WindowClose` event to exit the loop
/// regardless of what the registered handler (if any) returns.
///
/// All three backends' own window-close wiring vetoes the OS close request
/// ("×"/Alt-F4/window-manager close on GTK, the red traffic-light button on
/// macOS, the "×"/Alt-F4 close on Windows) unless the dispatched handler
/// returns `Reaction::Exit` — the right default for an app that wants to
/// confirm/veto a close, but
/// `PyAppLogic::handle` returns `Reaction::Continue` whenever no handler is
/// registered at all, which would otherwise leave a bare `run_native()`
/// call with no way to close its own window. This wrapper still calls
/// through to the inner `PyAppLogic::handle` first (so a registered
/// handler keeps seeing `WindowClose` and can still ask to exit itself,
/// either by raising or by returning `False`, same as every other event —
/// see [`reaction_from_return`]), then overrides only the `WindowClose`
/// case to always return `Reaction::Exit` afterwards regardless of what the
/// handler returned.
///
/// On macOS this `Reaction::Exit` is evaluated by
/// `QuadraView::evaluate_window_should_close`, which hands AppKit a `bool`
/// instead of routing through `apply_reaction`'s `Reaction::Exit` branch —
/// so the window closes on its own without calling `[NSApp terminate:]`,
/// the same "closes cleanly, returns control to the caller" outcome GTK's
/// `request_exit()` gives for its own window-close path. That outcome also
/// depends on a timing property of `MacRunner`: closing the last window
/// makes AppKit ask `applicationShouldTerminateAfterLastWindowClosed:`,
/// which answers `true` and would route on to `applicationShouldTerminate:`
/// and end the process — but `MacRunner::run_one` reads its
/// `windowWillClose:` latch and reports `StepOutcome::Exited`
/// *synchronously*, so `run_native`'s pump loop breaks (and `run_native`
/// returns) before that deferred last-window check ever runs.
///
/// On the `macos` feature (with `gtk` not also enabled), this wrapper also
/// intercepts any *other* `Reaction::Exit` — a registered handler raising,
/// or explicitly returning `False` (see [`reaction_from_return`]) — before
/// it reaches `MacRunner`. `apply_reaction`'s own `Reaction::Exit` branch
/// calls `[NSApp terminate:]`, which ends the whole OS process; `run_native`
/// would never return and the caller's `except`/`finally` would never run.
/// Instead, `handle`/`tick` swap that `Reaction::Exit` for `Reaction::Continue`
/// and set `exit_requested`, which the `run_native` pump loop checks after
/// every `pump` call and breaks on — so the loop stops, `run_native` returns,
/// and any pending exception set on [`PyAppLogic::pending_error`] is
/// re-raised normally, exactly like GTK. The window itself is left open
/// (not torn down) until the process exits, the same outcome already
/// documented for a Ctrl-C on this backend.
///
/// None of that interception touches AppKit — it is a `Cell<bool>` and a
/// `matches!` — so the type is compiled (and unit-tested) on a `macos`-feature
/// build for any `target_os`, not just an Apple one. Only the
/// `run_native` branch that constructs it is Apple-only, which is why the
/// non-Apple configuration needs the `dead_code` allowance below.
///
/// `win` needs no `exit_requested` field either, for the same reason `gtk`
/// doesn't: `win::run::dispatch_with`/`tick` already call `DestroyWindow`
/// directly for *any* `Reaction::Exit`, not just one reached through
/// `WindowClose` — there is no AppKit-style `[NSApp terminate:]` for this
/// wrapper to head off.
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
struct NativeAppLogic {
    inner: PyAppLogic,
    /// Set by `handle`/`tick` when a non-`WindowClose` `Reaction::Exit`
    /// is intercepted; `run_native`'s pump loop breaks once this is set.
    /// Only present on the `macos` feature without `gtk` also enabled —
    /// GTK's own `apply_reaction` and `win`'s `dispatch_with`/`tick`
    /// already handle `Reaction::Exit` from any event without this
    /// wrapper's help.
    #[cfg(all(feature = "macos", not(feature = "gtk")))]
    exit_requested: Rc<Cell<bool>>,
}

#[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
impl AppLogic for NativeAppLogic {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, area: ()) {
        self.inner.render(backend, area);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let is_window_close = matches!(event, UiEvent::WindowClose);
        let reaction = self.inner.handle(event, backend);
        if is_window_close {
            return Reaction::Exit;
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

/// A headless quadraui app, driven entirely from Python.
///
/// Construct it with a `width`/`height` cell grid, insert descriptors with
/// the `insert_*` methods, and inspect the rendered frame with `screen()`/
/// `screen_contains()`.
///
/// `unsendable`: the TUI backend's internal terminal handle uses `Rc`/
/// `RefCell` (shared with its dialog-surface plumbing), so a `QuiApp` is
/// pinned to the Python thread that created it — attempting to hand it to
/// another thread raises at the Python level rather than needing `Send`/
/// `Sync` here. `run` still releases the GIL for the duration of the Rust
/// loop (see [`AssertUngil`]); that pinning is what makes doing so sound.
#[pyclass(module = "quadraui", unsendable)]
struct QuiApp {
    driver: TuiDriver<PyAppLogic>,
    /// Events queued by `push_event`/`push_key_event`, drained one per
    /// `step_once` call (oldest first); `step_once` ticks the app instead
    /// when this is empty.
    pending_events: VecDeque<UiEvent>,
    /// Set once a call into `driver` has panicked and been caught by
    /// [`QuiApp::guarded`]. Every method on a poisoned app raises
    /// `PoisonedError` instead of touching the driver again.
    poisoned: bool,
    /// Set once `run_native` has moved this app's [`PyAppLogic`] into a
    /// native window. A later `run_native` call on the same `QuiApp` raises
    /// `NativeAlreadyRunError` instead of silently opening a window backed
    /// by the empty `PyAppLogic` the first call left behind.
    #[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
    native_consumed: bool,
}

/// Wraps a value that is provably safe to move across [`Python::detach`]'s
/// closure boundary even though it isn't [`Send`].
///
/// `detach` only suspends *this* thread's GIL attachment so other Python
/// threads can run concurrently; it never migrates `T` onto a different OS
/// thread, and the closure given to it runs to completion before `detach`
/// returns — so nothing outside the closure can observe or race the
/// wrapped value while it runs. `run` wraps `&mut QuiApp` (itself
/// `#[pyclass(unsendable)]`, pinned to one Python thread, so no other
/// thread could reach the same instance even if it tried); when built
/// with the `gtk`, `macos`, or `win` feature, `run_native` wraps an owned
/// `(PyAppLogic, RunConfig, ...)` that it has already moved out of its
/// `QuiApp` — in both cases the value lives only on this one thread for
/// the closure's duration.
struct AssertUngil<T>(T);

// SAFETY: see the type's doc comment — `detach` keeps execution on the
// calling OS thread for the closure's whole lifetime, so no concurrent
// access to the wrapped `!Send` value can occur.
unsafe impl<T> Send for AssertUngil<T> {}

/// Decodes a Python `dict` into `T` via `pythonize`, raising
/// `InvalidDescriptorError` on a shape mismatch rather than panicking
/// across the PyO3 boundary.
fn decode<T: serde::de::DeserializeOwned>(what: &str, dict: &Bound<'_, PyDict>) -> PyResult<T> {
    pythonize::depythonize(dict.as_any()).map_err(|err| {
        InvalidDescriptorError::new_err(format!("{what}: invalid descriptor: {err}"))
    })
}

/// Downcasts a caught panic payload to a human-readable message, mirroring
/// `quadraui-ffi`'s `record_panic`.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

#[pymethods]
impl QuiApp {
    /// Creates a new headless app on a `width`x`height` cell grid.
    /// `on_event`, if given, is registered as the event handler — equivalent
    /// to calling `set_handler` right after construction. `render`, if
    /// given, is registered as the render callback — equivalent to calling
    /// `set_render` right after construction.
    #[new]
    #[pyo3(signature = (width, height, on_event=None, render=None))]
    fn new(
        width: u16,
        height: u16,
        on_event: Option<Py<PyAny>>,
        render: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        if width == 0 || height == 0 {
            return Err(InvalidDimensionsError::new_err(
                "width and height must both be non-zero",
            ));
        }
        let app_logic = PyAppLogic {
            handler: on_event,
            render_callback: render,
            ..PyAppLogic::default()
        };
        Ok(QuiApp {
            driver: TuiDriver::new(app_logic, width, height),
            pending_events: VecDeque::new(),
            poisoned: false,
            #[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
            native_consumed: false,
        })
    }

    /// Registers `handler` as the callable invoked (with the pythonized
    /// event payload) for every event drained by `step`/`run`. Replaces any
    /// previously registered handler.
    fn set_handler(&mut self, handler: Py<PyAny>) {
        self.driver.app_mut().handler = Some(handler);
    }

    /// Registers `handler` as the no-argument callable invoked by
    /// `step`/`run` whenever it advances with no event queued. Replaces any
    /// previously registered tick handler.
    fn set_tick_handler(&mut self, handler: Py<PyAny>) {
        self.driver.app_mut().on_tick = Some(handler);
    }

    /// Registers `callback` as the no-argument render callback invoked once
    /// per frame. It must return a `dict` mapping slot names ("button",
    /// "text_display", "list", "tree", "data_table", "tab_bar") to a
    /// descriptor `dict` (or `None`/omitted to draw nothing in that slot),
    /// and must not call back into this `QuiApp` — it only returns data.
    /// Replaces any previously registered render callback, and overrides
    /// whatever `insert_*` put in the static slots for every frame from
    /// here on; `slot_clear`/`insert_*` still work but their effect on what
    /// is drawn is masked while a render callback is registered.
    fn set_render(&mut self, callback: Py<PyAny>) {
        self.driver.app_mut().render_callback = Some(callback);
    }

    /// Decodes `descriptor` into a `UiEvent` and queues it for the next
    /// `step`/`run` call. Raises `ValueError` on a shape mismatch.
    fn push_event(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<()> {
        let event: UiEvent = decode("push_event", descriptor)?;
        self.pending_events.push_back(event);
        Ok(())
    }

    /// Queues a synthetic `KeyPressed` event for the character with Unicode
    /// code point `ch`, no modifiers held, not a repeat. Raises
    /// `InvalidCharError` if `ch` isn't a valid Unicode scalar value.
    fn push_key_event(&mut self, ch: u32) -> PyResult<()> {
        let ch = char::from_u32(ch).ok_or_else(|| {
            InvalidCharError::new_err(format!("invalid Unicode code point: {ch}"))
        })?;
        self.pending_events.push_back(UiEvent::KeyPressed {
            key: Key::Char(ch),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        Ok(())
    }

    /// Resizes the underlying cell grid to `width`x`height`.
    fn resize(&mut self, width: u16, height: u16) -> PyResult<()> {
        if width == 0 || height == 0 {
            return Err(InvalidDimensionsError::new_err(
                "width and height must both be non-zero",
            ));
        }
        self.guarded(|app| {
            app.driver.resize(width, height);
            Ok(())
        })
    }

    /// Advances the runner by one step: dispatches the oldest queued event,
    /// or ticks the app if none is queued. Returns `True` if the app is
    /// still running afterwards, `False` once it has exited. Re-raises
    /// whatever exception a registered handler/tick callback raised while
    /// handling this step.
    fn step(&mut self) -> PyResult<bool> {
        self.guarded(|app| app.step_once())
    }

    /// Runs up to `max_steps` steps (default 1), releasing the GIL for the
    /// duration of the Rust loop and re-acquiring it only for each
    /// individual handler/tick callback. Stops early once the app exits.
    /// Returns `True` if still running afterwards, `False` once exited.
    /// Re-raises whatever exception a registered handler/tick callback
    /// raised during any step.
    #[pyo3(signature = (max_steps=1))]
    fn run(&mut self, py: Python<'_>, max_steps: u32) -> PyResult<bool> {
        let steps = max_steps.max(1);
        let wrapped = AssertUngil(self);
        py.detach(move || {
            let app = wrapped;
            let mut running = true;
            for _ in 0..steps {
                running = app.0.guarded(|app| app.step_once())?;
                if !running {
                    break;
                }
            }
            Ok(running)
        })
    }

    /// Opens a real OS window (via `quadraui::gtk::GtkRunner` on the `gtk`
    /// feature, `quadraui::macos::MacRunner` on the `macos` feature,
    /// `quadraui::win::WinRunner` on the `win` feature) showing this app's
    /// current descriptors, and blocks the calling thread until that window
    /// closes. `title`, if given, overrides the generic default window title
    /// on all three backends. `app_id` and `icon_name` set the window's
    /// `gio::Application` id and icon on GTK, falling back to `RunConfig`'s
    /// own defaults when omitted; `single_instance` defaults to `False` on
    /// GTK, which sets `ApplicationFlags::NON_UNIQUE` so a concurrent second
    /// `run_native()` call — or any other quadraui `gtk_*` process sharing
    /// this `app_id` — always gets its own window. Passing `True` only drops
    /// that flag; unlike `quadraui::gtk::run`/`run_with`, `GtkRunner` (what
    /// `run_native` is built on) never wires `Application::connect_open` and
    /// never calls `Application::run()`, so there is no argv/open forwarding
    /// here for GLib's own D-Bus dedup to deliver to — a second launch
    /// sharing this `app_id` with `single_instance=True` is absorbed by the
    /// first instance with no window and no event, not GTK's usual "bring
    /// the existing window forward" behaviour. `MacRunner` has no
    /// `gio::Application`-style id or single-instance concept, and no
    /// equivalent of GTK's `icon_name` — its `RunConfig::app_icon` takes a
    /// `quadraui` `ImageSource`, not an icon-theme name a string could be
    /// forwarded to — so on the `macos` feature (with `gtk` not also
    /// enabled) `app_id`, `icon_name` and `single_instance` are accepted but
    /// silently ignored. `WinRunner` is in the same position as
    /// `MacRunner` here, for a different reason: `RunConfig::single_instance`
    /// is only ever read by the blocking, process-owning `win::run::run`
    /// (which creates a `CreateMutexW` guard and forwards a second launch's
    /// `argv` to the first instance's window via `WM_COPYDATA`), not by
    /// `WinRunner::new_with`, the runner `run_native` is built on — so on
    /// the `win` feature (with neither `gtk` nor `macos` also enabled)
    /// `app_id`, `icon_name` and `single_instance` are all accepted but
    /// silently ignored, same as on `macos`. Only present when
    /// this extension is built with the `gtk`, `macos`, or `win` feature; if
    /// more than one is enabled in the same build, `gtk` wins over `macos`,
    /// and both win over `win` — the branches below never run for whichever
    /// features lose that precedence.
    ///
    /// Raises `NativeAlreadyRunError` if this `QuiApp` already ran
    /// `run_native` once before — see that exception's doc for why a
    /// second call can't just open another window. Raises `NoDisplayError`
    /// on the `gtk` feature if no display is reachable (no
    /// `DISPLAY`/`WAYLAND_DISPLAY`) instead of letting GTK's own init path
    /// fail in a way this binding can't catch. Raises `NotOnMainThreadError`
    /// on the `macos` feature if there is no main thread available to open a
    /// window on (AppKit only runs on the thread the interpreter started
    /// on, which this binding can't know in advance until it checks)
    /// instead of letting `MacRunner::new_with`'s own `expect()` panic.
    /// Raises `InvalidAppIdError` on the `gtk` feature if `app_id` is given
    /// but is not a syntactically valid `GApplication` id, instead of
    /// letting GLib's own assertion trip an uncatchable `GLib-GIO-CRITICAL`
    /// once it reaches `Application::builder()`. Raises `InternalError` if
    /// this extension was built with the `macos` feature for a non-Apple
    /// `target_os`, or with the `win` feature for a non-Windows `target_os`
    /// — a build/platform mismatch no thread or argument the caller picks
    /// can satisfy, since there is no `quadraui::macos`/`quadraui::win`
    /// runner compiled into such a build at all. Also raises `InternalError`
    /// on the `win` feature if `WinRunner::new_with`/`pump` itself reports a
    /// Win32 failure (e.g. window-class registration) — wrapping whatever
    /// `windows::core::Error` message it carries, since there is no more
    /// specific exception this binding can distinguish it into up front the
    /// way the `gtk`/`macos` preflight checks above can.
    ///
    /// On GTK this routes through `GtkRunner` rather than
    /// `quadraui::gtk::run`/`run_with`, which own the whole process and
    /// parse the real `argv` — wrong for an embedded interpreter, since
    /// that argv is whatever Python itself was launched with (`python
    /// demo.py`, `python -m demo`, `pytest ...`, ...), not a
    /// `gio::Application`-shaped argument list, and the first positional
    /// argument isn't a file to `open`. `GtkRunner` calls `activate`
    /// directly with no argv parsing and deliberately doesn't wire
    /// `connect_open`.
    ///
    /// `WinRunner` has no equivalent of that GTK argv-avoidance: its
    /// `create_window` (which `WinRunner::new_with` calls) unconditionally
    /// reads the real process `argv` (skipping `argv[0]`) and, if
    /// non-empty, dispatches it as `UiEvent::OpenRequested` into the
    /// handler the moment the window is created — independent of
    /// `single_instance`. Under `run_native`, that argv belongs to the
    /// host Python interpreter, not this app, so `python demo.py` (or
    /// `python -m demo`, `pytest ...`, ...) delivers `OpenRequested {
    /// files: ["demo.py"] }` (or similar) to the registered handler on
    /// the `win` feature as soon as the window opens, with no way for a
    /// caller to suppress it. A handler that tolerates an unrecognised
    /// event (returning `None`/`False` without raising) is unaffected;
    /// one that raises on it will see the window open and immediately
    /// close, with `run_native()` re-raising that exception once the
    /// pump loop below notices — on Windows only.
    ///
    /// Moves this app's whole [`PyAppLogic`] — every inserted descriptor
    /// plus both registered handlers — out of the headless driver this
    /// `QuiApp` wraps, wraps it in [`NativeAppLogic`] (so closing the
    /// window from the OS always works, even with no handler registered —
    /// see that type's doc), and hands it to the native runner by value.
    /// The GIL is released for the duration of each bounded wait inside
    /// the pump loop (mirroring [`Self::run`]'s `py.detach`), with a
    /// `py.check_signals()` call between waits so a Ctrl-C at the
    /// controlling terminal still raises `KeyboardInterrupt` promptly
    /// instead of only being noticed once the window closes on its own.
    /// On GTK, when that happens, `GtkRunner::request_exit()` tears the
    /// window down before the loop returns, so a Ctrl-C closes the window
    /// rather than leaving it open with a dead event loop behind it;
    /// `MacRunner` has no equivalent teardown call, so on macOS a Ctrl-C
    /// simply stops the pump loop and lets `runner` drop, leaving the
    /// window itself open until the process exits. `WinRunner` falls
    /// between the two: it has no `request_exit()` either, so a Ctrl-C on
    /// Windows also just stops the pump loop and lets `runner` drop — but
    /// its `Drop` impl calls `DestroyWindow` for a window that's still
    /// open, so the window still closes, just as a side effect of the drop
    /// rather than an explicit teardown call before it. `handle`/`tick`
    /// re-acquire the GIL per callback via `Python::attach`, exactly as
    /// they do for the headless path.
    ///
    /// A `Reaction::Exit` returned for any event other than `WindowClose`
    /// (e.g. a handler raising, or explicitly asking to exit, in response
    /// to a key press) returns control to this call on every backend.
    /// `GtkRunner::request_exit()` closes the window and returns control
    /// directly. On macOS, [`NativeAppLogic`] intercepts that `Reaction::Exit`
    /// before it would otherwise reach `apply_reaction`'s own `Exit` branch
    /// (which calls `[NSApp terminate:]` and would end the whole OS process),
    /// swaps it for `Reaction::Continue`, and flags the pump loop below to
    /// break on its next iteration — so the window is left open (same as a
    /// Ctrl-C, above) rather than torn down, but `run_native` still returns
    /// and any pending exception is still re-raised. On the `win` feature no
    /// such interception is needed: `win::run::dispatch_with`/`tick` already
    /// call `DestroyWindow` directly for *any* `Reaction::Exit`, the same way
    /// GTK's own `apply_reaction` does, so the window closes and
    /// `WinRunner::pump` reports `StepOutcome::Exited` on the very next call,
    /// breaking the loop below with no extra flag required. `WindowClose`
    /// itself always closes the window on every backend, since
    /// [`NativeAppLogic`] forces that one case through
    /// `evaluate_window_should_close`/`request_exit`/`DestroyWindow` rather
    /// than leaving it to the general `Reaction::Exit` path (which the
    /// registered handler could otherwise veto by returning anything else).
    ///
    /// That `WindowClose` case returning control to Python (rather than
    /// AppKit terminating the interpreter) is a *timing* property worth
    /// stating outright: `MacRunner::run_one` reads the latch
    /// `windowWillClose:` sets and reports `StepOutcome::Exited`
    /// synchronously, within the same pump call, so the loop below breaks
    /// before AppKit's deferred
    /// `applicationShouldTerminateAfterLastWindowClosed:` →
    /// `applicationShouldTerminate:` → `NSApplicationTerminateReply`
    /// sequence gets a chance to run. Closing the window from the
    /// titlebar on macOS is therefore the load-bearing manual check for
    /// this binding: if that ordering ever changed, `run_native` would
    /// take the interpreter down with the window instead of returning.
    ///
    /// After the window closes, the headless driver is left holding an
    /// empty `PyAppLogic` (nothing to hand back — the native runner owned
    /// the one this call moved out for as long as the window was open):
    /// `step`/`run`/`screen` still work, they just have nothing left to
    /// paint or dispatch to until the caller inserts descriptors/handlers
    /// again, and a further `run_native` call raises `NativeAlreadyRunError`.
    /// Re-raises whatever exception a registered handler/tick callback
    /// raised while the window was open, or a `KeyboardInterrupt` from a
    /// Ctrl-C the pump loop observed.
    #[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
    #[pyo3(signature = (title=None, app_id=None, icon_name=None, single_instance=false))]
    fn run_native(
        &mut self,
        py: Python<'_>,
        title: Option<String>,
        app_id: Option<String>,
        icon_name: Option<String>,
        single_instance: bool,
    ) -> PyResult<()> {
        self.guarded(|app| {
            if app.native_consumed {
                return Err(NativeAlreadyRunError::new_err(
                    "run_native already ran once on this QuiApp; its handlers and \
                    descriptors were moved into the window that call opened, so \
                    there is nothing left for a second call to show",
                ));
            }

            #[cfg(feature = "gtk")]
            {
                if !quadraui::gtk::display_available() {
                    return Err(NoDisplayError::new_err(
                        "run_native found no display to open a window on (no \
                        DISPLAY/WAYLAND_DISPLAY)",
                    ));
                }
                if let Some(app_id) = &app_id {
                    if !quadraui::gtk::app_id_is_valid(app_id) {
                        return Err(InvalidAppIdError::new_err(format!(
                            "run_native's app_id {app_id:?} is not a valid GApplication id \
                            (see https://developer.gnome.org/documentation/tutorials/application-id.html)"
                        )));
                    }
                }
                app.native_consumed = true;

                let app_logic = std::mem::take(app.driver.app_mut());
                // Cloned twice: `native_error` stays outside the `detach`
                // closure for the caller to read back once the window closes;
                // `loop_error` is the clone the loop itself writes a
                // `check_signals()` failure into — both are the same
                // `Rc<RefCell<_>>` cell underneath, so either write is visible
                // through either handle.
                let native_error = app_logic.pending_error.clone();
                let loop_error = app_logic.pending_error.clone();
                // `RunConfig::new` for the fields with a documented default
                // (`app_id`/`title`), builder calls for the rest, rather than
                // mixing direct field writes on a `default()` value with
                // builder calls on the same `config`.
                let defaults = quadraui::gtk::RunConfig::default();
                let mut config = quadraui::gtk::RunConfig::new(
                    app_id.unwrap_or(defaults.app_id),
                    title.unwrap_or(defaults.title),
                );
                if let Some(icon_name) = icon_name {
                    config = config.with_icon_name(icon_name);
                }
                config = config.with_single_instance(single_instance);
                let wrapped = AssertUngil((app_logic, config, loop_error));
                py.detach(move || {
                    // Bind the whole `wrapped` first so the closure captures the
                    // `AssertUngil`-wrapped value itself rather than `wrapped.0`
                    // directly — 2021-edition disjoint capture would otherwise
                    // capture just the field, which isn't `Send` on its own and
                    // defeats the point of the wrapper.
                    let wrapped = wrapped;
                    let (app_logic, config, loop_error) = wrapped.0;
                    let mut runner = quadraui::gtk::GtkRunner::new_with(
                        NativeAppLogic { inner: app_logic },
                        config,
                    );
                    let poll_interval = std::time::Duration::from_millis(50);
                    loop {
                        if runner.pump(poll_interval) == quadraui::gtk::StepOutcome::Exited {
                            break;
                        }
                        // A render callback invoked by this `pump` may have
                        // recorded an unrecoverable error; nothing else is
                        // polling `loop_error` while the GIL is detached, so
                        // stop driving the window now instead of looping on
                        // a broken app until some other exit condition fires.
                        if loop_error.borrow().is_some() {
                            runner.request_exit();
                            runner.step();
                            break;
                        }
                        // Re-acquiring the GIL here, even briefly, is what lets
                        // CPython's pending-signal check run — with no
                        // handler/tick ever called (e.g. a window just sitting
                        // open with no events), control otherwise never
                        // returns to Python at all for the whole native loop,
                        // and a Ctrl-C would go unnoticed until the window
                        // closes some other way.
                        if let Err(err) = Python::attach(|py| py.check_signals()) {
                            record_error(&loop_error, err);
                            runner.request_exit();
                            // `request_exit` only calls `window.destroy()`; one
                            // more non-blocking pass lets the main context
                            // actually process that destroy (and whatever
                            // `destroy`-signal/idle follow-up it queues) before
                            // this loop exits and `runner` drops, rather than
                            // leaving it to whatever GLib does on an
                            // already-abandoned `MainContext`.
                            runner.step();
                            break;
                        }
                    }
                });
                if let Some(err) = native_error.borrow_mut().take() {
                    return Err(err);
                }
                Ok(())
            }

            #[cfg(all(feature = "macos", target_os = "macos", not(feature = "gtk")))]
            {
                // `app_id`/`icon_name`/`single_instance` have no macOS
                // analog; accepted for signature parity with the `gtk`
                // build and silently ignored here.
                let _ = (&app_id, &icon_name, single_instance);

                // `MacBootstrap`/`MacRunner::new_with` call
                // `MainThreadMarker::new().expect(...)`, which panics off
                // the main thread instead of degrading gracefully — check
                // first so a wrong-thread call raises `NotOnMainThreadError`
                // instead of aborting the whole process.
                if objc2::MainThreadMarker::new().is_none() {
                    return Err(NotOnMainThreadError::new_err(
                        "run_native must be called from the main thread to open a \
                        window on macOS",
                    ));
                }

                app.native_consumed = true;

                let app_logic = std::mem::take(app.driver.app_mut());
                let native_error = app_logic.pending_error.clone();
                let loop_error = app_logic.pending_error.clone();
                let config = quadraui::macos::RunConfig::new(
                    title.unwrap_or(quadraui::macos::RunConfig::default().title),
                );
                let wrapped = AssertUngil((app_logic, config, loop_error));
                py.detach(move || {
                    let wrapped = wrapped;
                    let (app_logic, config, loop_error) = wrapped.0;
                    let exit_requested = Rc::new(Cell::new(false));
                    let mut runner = quadraui::macos::MacRunner::new_with(
                        NativeAppLogic {
                            inner: app_logic,
                            exit_requested: exit_requested.clone(),
                        },
                        config,
                    );
                    let poll_interval = std::time::Duration::from_millis(50);
                    loop {
                        if runner.pump(poll_interval) == quadraui::macos::StepOutcome::Exited {
                            break;
                        }
                        if exit_requested.get() {
                            break;
                        }
                        // A render callback invoked by this `pump` may have
                        // recorded an unrecoverable error; stop pumping now
                        // rather than looping on a broken app.
                        if loop_error.borrow().is_some() {
                            break;
                        }
                        // Same signal-check discipline as the `gtk` loop
                        // above. `MacRunner` has no `request_exit()`
                        // equivalent to tear the window down on a Ctrl-C
                        // (see this method's doc for the known asymmetry),
                        // so this just stops pumping and lets `runner`
                        // drop, mirroring `run_tui`'s simpler
                        // record-and-break pattern below rather than the
                        // `gtk` branch's teardown-then-step pattern.
                        if let Err(err) = Python::attach(|py| py.check_signals()) {
                            record_error(&loop_error, err);
                            break;
                        }
                    }
                });
                if let Some(err) = native_error.borrow_mut().take() {
                    return Err(err);
                }
                Ok(())
            }

            // Reachable only when the `macos` feature is enabled on a
            // non-Apple `target_os` — e.g. a `cargo check -p quadraui-py
            // --features macos` typecheck run on Linux or Windows, mirroring
            // the `target_os = "macos"` gate on the `objc2` dependency this
            // build arm needs. `quadraui::macos` is itself gated on
            // `target_os = "macos"`, so there is no runner to hand off to
            // here; this arm exists only to keep `run_native` buildable on
            // other hosts, never to run for real. It raises `InternalError`
            // rather than `NotOnMainThreadError` because the condition is a
            // build/platform mismatch — no thread the caller could have
            // picked would make it work.
            #[cfg(all(feature = "macos", not(feature = "gtk"), not(target_os = "macos")))]
            {
                // Every argument the two real arms consume, discarded in one
                // place: this arm opens no window, so each would otherwise
                // be an unused-variable warning under `-D warnings`.
                let _ = (py, &app_id, &icon_name, single_instance, &title);
                Err(InternalError::new_err(
                    "run_native with the `macos` feature requires building for a \
                    macOS target_os; this binary was built for a different platform",
                ))
            }

            #[cfg(all(
                feature = "win",
                target_os = "windows",
                not(feature = "gtk"),
                not(feature = "macos")
            ))]
            {
                // `app_id`/`icon_name` have no Windows analog
                // (`RunConfig::with_app_icon` takes an `ImageSource`, not an
                // icon-theme name, and Windows has no GTK-style
                // application-id concept); accepted for signature parity
                // with the `gtk` build and silently ignored here.
                // `single_instance` is accepted and ignored too:
                // `RunConfig::single_instance` is only ever consumed by
                // `win::run::run` (the mutex guard and `WM_COPYDATA`
                // forwarding live there), not by `WinRunner::new_with`,
                // which this binding uses instead.
                let _ = (&app_id, &icon_name, single_instance);

                app.native_consumed = true;

                let app_logic = std::mem::take(app.driver.app_mut());
                let native_error = app_logic.pending_error.clone();
                let loop_error = app_logic.pending_error.clone();
                let defaults = quadraui::win::RunConfig::default();
                let config = quadraui::win::RunConfig::new(title.unwrap_or(defaults.title));
                let wrapped = AssertUngil((app_logic, config, loop_error));
                let pump_result: Result<(), String> = py.detach(move || {
                    let wrapped = wrapped;
                    let (app_logic, config, loop_error) = wrapped.0;
                    // SAFETY: `new_with`/`pump` require constructing and
                    // stepping the runner on the same OS thread for its
                    // whole lifetime, with no other window on that thread —
                    // exactly what this `detach` closure does: `detach`
                    // keeps execution on the calling thread, and nothing
                    // else on it touches a `WinRunner` for the closure's
                    // duration.
                    let mut runner = unsafe {
                        quadraui::win::WinRunner::new_with(
                            NativeAppLogic { inner: app_logic },
                            &config,
                        )
                    }
                    .map_err(|err| err.to_string())?;
                    let poll_interval = std::time::Duration::from_millis(50);
                    loop {
                        let outcome = runner.pump(poll_interval).map_err(|err| err.to_string())?;
                        if outcome == quadraui::win::StepOutcome::Exited {
                            break;
                        }
                        // A render callback invoked by this `pump` may have
                        // recorded an unrecoverable error; stop pumping now
                        // rather than looping on a broken app.
                        if loop_error.borrow().is_some() {
                            break;
                        }
                        // Same signal-check discipline as the `gtk`/`macos`
                        // loops above. `WinRunner` has no `request_exit()`
                        // equivalent, but its `Drop` calls `DestroyWindow`
                        // if the window is still open, so a Ctrl-C here
                        // just stops pumping and lets `runner` drop, same
                        // as the `macos` branch.
                        if let Err(err) = Python::attach(|py| py.check_signals()) {
                            record_error(&loop_error, err);
                            break;
                        }
                    }
                    Ok(())
                });
                if let Some(err) = native_error.borrow_mut().take() {
                    return Err(err);
                }
                pump_result.map_err(|err| InternalError::new_err(format!("run_native: {err}")))
            }

            // Reachable only when the `win` feature is enabled on a
            // non-Windows `target_os` — e.g. a `cargo check -p quadraui-py
            // --features win` typecheck run on Linux or macOS.
            // `quadraui::win::WinRunner` is itself gated on
            // `target_os = "windows"`, so there is no runner to hand off to
            // here. Raises `InternalError` rather than anything more
            // specific since this is a build/platform mismatch no argument
            // the caller picks could satisfy.
            #[cfg(all(
                feature = "win",
                not(feature = "gtk"),
                not(feature = "macos"),
                not(target_os = "windows")
            ))]
            {
                let _ = (py, &app_id, &icon_name, single_instance, &title);
                Err(InternalError::new_err(
                    "run_native with the `win` feature requires building for a \
                    Windows target_os; this binary was built for a different platform",
                ))
            }
        })
    }

    /// Takes over the controlling terminal (raw mode, alternate screen,
    /// real crossterm keyboard/resize events) via a real
    /// `quadraui::tui::TuiRunner` showing this app's current descriptors,
    /// and blocks the calling thread until the app exits — restoring the
    /// terminal on the way out even if a handler raises or the process
    /// panics.
    ///
    /// A terminal has no close button or window manager to force an exit
    /// the way `run_native`'s `NativeAppLogic` does for `WindowClose`: this
    /// only returns once the moved app's `handle`/`tick` itself returns
    /// `Reaction::Exit`, which — same as the headless path — only happens
    /// when a registered handler or tick callback raises. Register one
    /// that raises on whichever keystroke(s) should quit before calling
    /// this; with no handler registered at all, `run_tui()` blocks
    /// forever (bar a signal).
    ///
    /// Moves this app's whole `PyAppLogic` — every inserted descriptor
    /// plus both registered handlers — out of the headless driver this
    /// `QuiApp` wraps and hands it to `TuiRunner` by value; no
    /// `NativeAppLogic`-style wrapper is needed here, since there's no
    /// runner-forced exit event to intercept. The GIL is released for the
    /// duration of each bounded `pump` wait (mirroring `run_native`), with
    /// the same per-iteration `py.check_signals()` call `run_native` uses —
    /// but note raw mode (`cfmakeraw`) disables `ISIG`, so a Ctrl-C *at the
    /// terminal* no longer raises `SIGINT` at all; it instead arrives as an
    /// ordinary key event through `handle`, same as any other keystroke, and
    /// it's that handler's job to turn it into an exit if desired.
    /// `check_signals()` here only catches a signal delivered some other
    /// way (e.g. `kill -INT` from another process) promptly instead of only
    /// being noticed once the app exits on its own; `handle`/`tick`
    /// re-acquire the GIL per callback via `Python::attach`, exactly as
    /// they do for the headless path.
    ///
    /// After the terminal is released, the headless driver is left holding
    /// an empty `PyAppLogic`, same as after `run_native` returns. Raises
    /// `InternalError` if the terminal couldn't be taken over at all (most
    /// commonly: no controlling terminal is reachable, e.g. a subprocess
    /// detached from its session with `setsid`), or re-raises whatever
    /// exception a registered handler/tick callback raised, or a
    /// `KeyboardInterrupt` from an externally delivered signal the pump loop
    /// observed.
    fn run_tui(&mut self, py: Python<'_>) -> PyResult<()> {
        self.guarded(|app| {
            let app_logic = std::mem::take(app.driver.app_mut());
            // Same double-clone rationale as `run_native`: `tui_error` stays
            // outside the `detach` closure for the caller to read back,
            // `loop_error` is the clone the loop writes a
            // `check_signals()` failure into.
            let tui_error = app_logic.pending_error.clone();
            let loop_error = app_logic.pending_error.clone();
            let wrapped = AssertUngil((app_logic, loop_error));
            let setup_result: io::Result<()> = py.detach(move || {
                let wrapped = wrapped;
                let (app_logic, loop_error) = wrapped.0;
                let mut runner = TuiRunner::new(app_logic)?;
                let poll_interval = Duration::from_millis(50);
                loop {
                    if runner.pump(poll_interval)? == StepOutcome::Exited {
                        break;
                    }
                    // A render callback invoked by this `pump` may have
                    // recorded an unrecoverable error; stop pumping now
                    // rather than looping on a broken app.
                    if loop_error.borrow().is_some() {
                        break;
                    }
                    // Re-acquiring the GIL here, even briefly, is what lets
                    // CPython's pending-signal check run: with no
                    // handler/tick ever called, control otherwise never
                    // returns to Python for the whole terminal loop, and a
                    // Ctrl-C would go unnoticed until the app exits some
                    // other way.
                    if let Err(err) = Python::attach(|py| py.check_signals()) {
                        record_error(&loop_error, err);
                        break;
                    }
                }
                Ok(())
            });
            if let Some(err) = tui_error.borrow_mut().take() {
                return Err(err);
            }
            setup_result.map_err(|err| InternalError::new_err(format!("run_tui: {err}")))
        })
    }

    /// Inserts a `Button` descriptor from a Python dict, returning its slot
    /// handle. Raises `RuntimeError` if the button slot is already
    /// occupied — call `slot_clear` first.
    fn insert_button(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<i64> {
        self.insert(
            SLOT_BUTTON,
            "insert_button",
            |app, value| {
                app.button = Some(value);
            },
            descriptor,
        )
    }

    /// Inserts a `TextDisplay` descriptor from a Python dict, returning its
    /// slot handle. Raises `RuntimeError` if the text-display slot is
    /// already occupied — call `slot_clear` first.
    fn insert_text_display(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<i64> {
        self.insert(
            SLOT_TEXT_DISPLAY,
            "insert_text_display",
            |app, value| {
                app.text_display = Some(value);
            },
            descriptor,
        )
    }

    /// Inserts a `ListView` descriptor from a Python dict, returning its
    /// slot handle. Raises `RuntimeError` if the list slot is already
    /// occupied — call `slot_clear` first.
    fn insert_list(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<i64> {
        self.insert(
            SLOT_LIST,
            "insert_list",
            |app, value| {
                app.list = Some(value);
            },
            descriptor,
        )
    }

    /// Inserts a `TreeView` descriptor from a Python dict, returning its
    /// slot handle. Raises `RuntimeError` if the tree slot is already
    /// occupied — call `slot_clear` first.
    fn insert_tree(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<i64> {
        self.insert(
            SLOT_TREE,
            "insert_tree",
            |app, value| {
                app.tree = Some(value);
            },
            descriptor,
        )
    }

    /// Inserts a `DataTable` descriptor from a Python dict, returning its
    /// slot handle. Raises `RuntimeError` if the data-table slot is
    /// already occupied — call `slot_clear` first.
    fn insert_data_table(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<i64> {
        self.insert(
            SLOT_DATA_TABLE,
            "insert_data_table",
            |app, value| {
                app.data_table = Some(value);
            },
            descriptor,
        )
    }

    /// Inserts a `TabBar` descriptor from a Python dict, returning its slot
    /// handle. Raises `RuntimeError` if the tab-bar slot is already
    /// occupied — call `slot_clear` first.
    fn insert_tab_bar(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<i64> {
        self.insert(
            SLOT_TAB_BAR,
            "insert_tab_bar",
            |app, value| {
                app.tab_bar = Some(value);
            },
            descriptor,
        )
    }

    /// Empties a slot (one of the integers returned by an `insert_*` call),
    /// re-rendering so `screen()` no longer shows it. Returns whether the
    /// slot held anything. Raises `UnknownSlotError` if `slot` doesn't name
    /// one of this app's six fixed primitive slots.
    fn slot_clear(&mut self, slot: i64) -> PyResult<bool> {
        if !PyAppLogic::is_known_slot(slot) {
            return Err(UnknownSlotError::new_err(format!(
                "unknown slot id: {slot}"
            )));
        }
        self.guarded(|app| {
            let cleared = app.driver.app_mut().clear_slot(slot);
            if cleared {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(cleared)
        })
    }

    /// Renders the current frame and returns it as a plain-text screen
    /// dump, one row per line.
    fn screen(&mut self) -> PyResult<String> {
        self.guarded(|app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(app.driver.screen())
        })
    }

    /// Whether `needle` appears anywhere in the current rendered frame.
    fn screen_contains(&mut self, needle: &str) -> PyResult<bool> {
        self.guarded(|app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(app.driver.screen_contains(needle))
        })
    }
}

/// Takes and re-raises any error a render callback recorded during the
/// `render()` call that just ran, mirroring `step_once`'s handling of a
/// `handle`/`tick` callback failure. Every `QuiApp` method that can trigger
/// a render outside of `step`/`run` — `screen`, `screen_contains`,
/// `insert_*`, `slot_clear` — calls this right after rendering so a render
/// callback's exception is never silently swallowed.
fn take_pending_error(app: &mut PyAppLogic) -> PyResult<()> {
    if let Some(err) = app.pending_error.borrow_mut().take() {
        return Err(err);
    }
    Ok(())
}

impl QuiApp {
    /// Runs `body` on `self`, catching a Rust panic instead of letting it
    /// unwind across the PyO3 boundary. On success returns `body`'s result
    /// unchanged; on a panic, poisons this app and raises `PanicError`. If
    /// this app is already poisoned (an earlier call panicked), raises
    /// `PoisonedError` without running `body` at all — mirrors
    /// `quadraui-ffi`'s `with_app`/`boundary` pair.
    fn guarded<T>(&mut self, body: impl FnOnce(&mut Self) -> PyResult<T>) -> PyResult<T> {
        if self.poisoned {
            return Err(PoisonedError::new_err(
                "this QuiApp is poisoned by an earlier caught panic and can no longer be used",
            ));
        }
        match panic::catch_unwind(AssertUnwindSafe(|| body(self))) {
            Ok(result) => result,
            Err(payload) => {
                self.poisoned = true;
                Err(PanicError::new_err(panic_message(&*payload)))
            }
        }
    }

    /// Shared body of every `insert_*` method: reject an occupied slot,
    /// decode the dict into `T`, store it with `store`, then re-render.
    fn insert<T: serde::de::DeserializeOwned>(
        &mut self,
        slot: i64,
        what: &str,
        store: impl FnOnce(&mut PyAppLogic, T),
        descriptor: &Bound<'_, PyDict>,
    ) -> PyResult<i64> {
        if self.driver.app().slot_occupied(slot) {
            return Err(SlotOccupiedError::new_err(format!(
                "{what}: slot already occupied; call slot_clear({slot}) first"
            )));
        }
        let value: T = decode(what, descriptor)?;
        self.guarded(|app| {
            store(app.driver.app_mut(), value);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(slot)
        })
    }

    /// Dispatches the oldest queued event (or ticks, if none is queued),
    /// then surfaces any exception a Python callback raised while handling
    /// it. Returns whether the app is still running.
    fn step_once(&mut self) -> PyResult<bool> {
        if self.driver.exited() {
            return Ok(false);
        }
        match self.pending_events.pop_front() {
            Some(event) => {
                self.driver.dispatch(event);
            }
            None => {
                self.driver.tick();
            }
        }
        take_pending_error(self.driver.app_mut())?;
        Ok(!self.driver.exited())
    }
}

/// The compiled extension module (`_quadraui`) that `python/quadraui/__init__.py`
/// imports names from.
#[pymodule]
fn _quadraui(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<QuiApp>()?;
    let py = m.py();
    m.add("QuadrauiError", py.get_type::<QuadrauiError>())?;
    m.add(
        "InvalidDescriptorError",
        py.get_type::<InvalidDescriptorError>(),
    )?;
    m.add("SlotOccupiedError", py.get_type::<SlotOccupiedError>())?;
    m.add("UnknownSlotError", py.get_type::<UnknownSlotError>())?;
    m.add(
        "InvalidDimensionsError",
        py.get_type::<InvalidDimensionsError>(),
    )?;
    m.add("InvalidCharError", py.get_type::<InvalidCharError>())?;
    m.add("PanicError", py.get_type::<PanicError>())?;
    m.add("PoisonedError", py.get_type::<PoisonedError>())?;
    m.add("InternalError", py.get_type::<InternalError>())?;
    m.add(
        "NativeAlreadyRunError",
        py.get_type::<NativeAlreadyRunError>(),
    )?;
    m.add("NoDisplayError", py.get_type::<NoDisplayError>())?;
    m.add(
        "NotOnMainThreadError",
        py.get_type::<NotOnMainThreadError>(),
    )?;
    m.add("InvalidAppIdError", py.get_type::<InvalidAppIdError>())?;
    // Which native *feature* (if any) this build was compiled with — `gtk`
    // wins over `macos`, and both win over `win`, when more than one is
    // enabled (see `run_native`'s doc). This is a build configuration
    // report, not a promise that a window can actually open: `"macos"` (or
    // `"win"`) is reported by a `--features macos` (or `--features win`)
    // build for any `target_os`, including the Linux/Windows typecheck builds
    // where `run_native` can only raise `InternalError` for the platform
    // mismatch. Exists so a test/tool can tell the builds apart without the
    // backend-specific side effects `run_native` itself would have (opening
    // a real window, a `GApplication` id check that only applies to GTK,
    // ...); not part of the stable Python API.
    #[cfg(feature = "gtk")]
    m.add("_NATIVE_BACKEND", "gtk")?;
    #[cfg(all(feature = "macos", not(feature = "gtk")))]
    m.add("_NATIVE_BACKEND", "macos")?;
    #[cfg(all(feature = "win", not(feature = "gtk"), not(feature = "macos")))]
    m.add("_NATIVE_BACKEND", "win")?;
    #[cfg(not(any(feature = "gtk", feature = "macos", feature = "win")))]
    m.add("_NATIVE_BACKEND", py.None())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Rust panic raised from inside `guarded`'s body is caught and
    /// re-raised as `PanicError`, and poisons the app so a later call
    /// raises `PoisonedError` without running its body.
    #[test]
    fn guarded_catches_panic_then_poisons() {
        Python::attach(|py| {
            let mut app = QuiApp::new(10, 10, None, None).expect("valid dimensions");

            let panicked: PyResult<()> = app.guarded(|_| panic!("boom"));
            let err = panicked.expect_err("a caught panic must surface as an error");
            assert!(err.is_instance_of::<PanicError>(py));
            assert!(app.poisoned);

            let mut body_ran = false;
            let after_poison: PyResult<()> = app.guarded(|_| {
                body_ran = true;
                Ok(())
            });
            assert!(!body_ran, "a poisoned app must not run the guarded body");
            let err = after_poison.expect_err("a poisoned app must keep erroring");
            assert!(err.is_instance_of::<PoisonedError>(py));
        });
    }

    /// `reaction_from_return` treats only an explicit Python `False` as a
    /// request to exit; `True`, `None` (the common no-explicit-`return`
    /// case), and a falsy non-bool like `0` all continue.
    #[test]
    fn reaction_from_return_treats_only_explicit_false_as_exit() {
        Python::attach(|py| {
            let false_val = false.into_pyobject(py).expect("bool");
            assert_eq!(reaction_from_return(&false_val), Reaction::Exit);

            let true_val = true.into_pyobject(py).expect("bool");
            assert_eq!(reaction_from_return(&true_val), Reaction::Continue);

            let none_val = py.None().into_bound(py);
            assert_eq!(reaction_from_return(&none_val), Reaction::Continue);

            let zero_val = 0i32.into_pyobject(py).expect("int").into_any();
            assert_eq!(reaction_from_return(&zero_val), Reaction::Continue);
        });
    }

    /// `reaction_for_callback` promotes an ordinary `Continue` to `Redraw`
    /// only when a render callback is registered — the mechanism that lets
    /// `run_tui`/`run_native` actually repaint after a handler/tick mutates
    /// plain Python state, instead of leaving the screen frozen at the last
    /// frame a `Reaction::Redraw` happened to trigger for some other reason.
    /// `Exit` passes through unchanged either way, since nothing should
    /// repaint after an explicit exit request.
    #[test]
    fn reaction_for_callback_promotes_continue_only_when_render_callback_present() {
        Python::attach(|py| {
            let none_val = py.None().into_bound(py);
            assert_eq!(
                reaction_for_callback(&none_val, false),
                Reaction::Continue,
                "no render callback registered: Continue must stay Continue"
            );
            assert_eq!(
                reaction_for_callback(&none_val, true),
                Reaction::Redraw,
                "a render callback is registered: Continue must promote to Redraw"
            );

            let false_val = false.into_pyobject(py).expect("bool");
            assert_eq!(
                reaction_for_callback(&false_val, true),
                Reaction::Exit,
                "an explicit False exit request must not be overridden by Redraw"
            );
        });
    }

    /// A tick handler that returns `False` exits the driver loop on its own
    /// step, with no exception raised — `step()` returns `Ok(false)` and
    /// `pending_error` stays unset, unlike the raise-to-exit path.
    #[test]
    fn tick_handler_returning_false_exits_without_raising() {
        Python::attach(|py| {
            let mut app = QuiApp::new(10, 10, None, None).expect("valid dimensions");
            let on_tick = py
                .eval(c"lambda: False", None, None)
                .expect("lambda literal")
                .unbind();
            app.set_tick_handler(on_tick);

            let running = app
                .guarded(|app| app.step_once())
                .expect("no handler raised, so step_once must not error");
            assert!(
                !running,
                "returning False from the tick handler must stop the loop"
            );
        });
    }

    /// `record_error` keeps the first error recorded in a dispatch and
    /// ignores a later one — a render-callback failure painting the same
    /// frame a handler already failed on must not clobber the handler's
    /// error, which is closer to the root cause.
    #[test]
    fn record_error_preserves_the_first_error() {
        Python::attach(|py| {
            let pending_error: Rc<RefCell<Option<PyErr>>> = Rc::new(RefCell::new(None));
            record_error(&pending_error, InternalError::new_err("first"));
            record_error(&pending_error, InternalError::new_err("second"));
            let recorded = pending_error
                .borrow_mut()
                .take()
                .expect("an error must be recorded");
            assert_eq!(
                recorded.to_string(),
                InternalError::new_err("first").to_string()
            );
            let _ = py;
        });
    }

    /// `_NATIVE_BACKEND` reports exactly one native-feature name per build,
    /// picking `gtk` over `macos` over `win` when more than one is enabled
    /// (the same precedence `run_native`'s own `#[cfg]` branches use).
    /// `test_run_native.py`'s skip guard only compares this against `"gtk"`
    /// (it is GTK-specific, not a shared native-backend suite), so it tells
    /// a `gtk` build apart from every non-`gtk` build (`macos`, `win`, or
    /// neither) but does not itself distinguish `macos` from `win`; this
    /// test checks the `win` value precisely, without opening a real
    /// window.
    #[test]
    fn native_backend_attribute_matches_enabled_feature() {
        Python::attach(|py| {
            let module = PyModule::new(py, "_quadraui").expect("module");
            _quadraui(&module).expect("module init must not fail");
            let backend: Option<String> = module
                .getattr("_NATIVE_BACKEND")
                .expect("_NATIVE_BACKEND must be set")
                .extract()
                .expect("_NATIVE_BACKEND must be a str or None");

            #[cfg(feature = "gtk")]
            assert_eq!(backend, Some("gtk".to_string()));
            #[cfg(all(feature = "macos", not(feature = "gtk")))]
            assert_eq!(backend, Some("macos".to_string()));
            #[cfg(all(feature = "win", not(feature = "gtk"), not(feature = "macos")))]
            assert_eq!(backend, Some("win".to_string()));
            #[cfg(not(any(feature = "gtk", feature = "macos", feature = "win")))]
            assert_eq!(backend, None);
        });
    }

    /// On the `macos` feature without `gtk`, `NativeAppLogic` downgrades a
    /// non-`WindowClose` `Reaction::Exit` (e.g. a handler returning
    /// `False`) to `Reaction::Continue` and records the request on
    /// `exit_requested`, instead of letting it reach `apply_reaction`'s own
    /// `Exit` branch -- which would call `[NSApp terminate:]` and kill the
    /// whole process rather than letting `run_native`'s pump loop return
    /// control normally. The `q` keystroke mirrors the quit key
    /// `examples/native_window_demo.py` answers with `return False`.
    /// Driven through `TuiDriver` purely as a headless `AppLogic` harness
    /// here; no real window is involved, and no `target_os` gate, so this
    /// covers the downgrade on every host.
    #[cfg(all(feature = "macos", not(feature = "gtk")))]
    #[test]
    fn native_app_logic_downgrades_handler_exit_to_continue() {
        Python::attach(|py| {
            let handler = py
                .eval(c"lambda event: False", None, None)
                .expect("lambda literal")
                .unbind();
            let inner = PyAppLogic {
                handler: Some(handler),
                ..Default::default()
            };
            let exit_requested = Rc::new(Cell::new(false));
            let logic = NativeAppLogic {
                inner,
                exit_requested: exit_requested.clone(),
            };
            let mut driver = TuiDriver::new(logic, 10, 10);

            let reaction = driver.dispatch(UiEvent::KeyPressed {
                key: Key::Char('q'),
                modifiers: Modifiers::default(),
                repeat: false,
            });

            assert_eq!(
                reaction,
                Reaction::Continue,
                "a non-WindowClose exit must not propagate as Reaction::Exit"
            );
            assert!(
                exit_requested.get(),
                "the downgrade must still flag the pump loop to stop"
            );
        });
    }

    /// Same downgrade, but via the `tick` path (an `on_tick` handler
    /// returning `False` rather than an event handler).
    #[cfg(all(feature = "macos", not(feature = "gtk")))]
    #[test]
    fn native_app_logic_downgrades_tick_exit_to_continue() {
        Python::attach(|py| {
            let on_tick = py
                .eval(c"lambda: False", None, None)
                .expect("lambda literal")
                .unbind();
            let inner = PyAppLogic {
                on_tick: Some(on_tick),
                ..Default::default()
            };
            let exit_requested = Rc::new(Cell::new(false));
            let logic = NativeAppLogic {
                inner,
                exit_requested: exit_requested.clone(),
            };
            let mut driver = TuiDriver::new(logic, 10, 10);

            let reaction = driver.tick();

            assert_eq!(
                reaction,
                Reaction::Continue,
                "a non-WindowClose exit must not propagate as Reaction::Exit"
            );
            assert!(
                exit_requested.get(),
                "the downgrade must still flag the pump loop to stop"
            );
        });
    }

    /// `WindowClose` always forces `Reaction::Exit` regardless of what the
    /// inner handler returns, and does *not* go through the downgrade --
    /// `exit_requested` stays unset because `run_native`'s native runner
    /// (not this flag) tears the window down for that case.
    #[cfg(all(feature = "macos", not(feature = "gtk")))]
    #[test]
    fn native_app_logic_window_close_still_exits_without_flag() {
        Python::attach(|py| {
            let handler = py
                .eval(c"lambda event: True", None, None)
                .expect("lambda literal")
                .unbind();
            let inner = PyAppLogic {
                handler: Some(handler),
                ..Default::default()
            };
            let exit_requested = Rc::new(Cell::new(false));
            let logic = NativeAppLogic {
                inner,
                exit_requested: exit_requested.clone(),
            };
            let mut driver = TuiDriver::new(logic, 10, 10);

            let reaction = driver.dispatch(UiEvent::WindowClose);

            assert_eq!(reaction, Reaction::Exit);
            assert!(
                !exit_requested.get(),
                "WindowClose exits through the normal path, not the flag"
            );
        });
    }
}
