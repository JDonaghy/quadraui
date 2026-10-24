//! PyO3 bindings over `quadraui`'s headless TUI driver.
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
//! callback (`set_handler`/`set_tick_handler`). An exception raised inside a
//! callback is captured and re-raised out of `step`/`run` rather than lost.
//!
//! Every error this binding can raise itself (as opposed to one a
//! registered Python callback raised, which passes through unchanged) is a
//! [`QuadrauiError`] subclass — see the `errors` section below. A Rust
//! panic reaching a driver call is caught at the [`QuiApp::guarded`]
//! boundary, raised as [`PanicError`] instead of unwinding into Python, and
//! poisons the handle so every later call raises [`PoisonedError`] rather
//! than touching a driver that may have panicked mid-mutation.
//!
//! When built with the `gtk` feature, [`QuiApp::run_native`] opens a real
//! OS window instead of driving the headless driver: it moves this app's
//! whole [`PyAppLogic`] into `quadraui::gtk::run_with`, which also releases
//! the GIL for its event loop and re-acquires it per callback, the same
//! discipline `run` uses for the headless loop.
use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use quadraui::tui::testing::TuiDriver;
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

/// The [`AppLogic`] a [`QuiApp`] drives: a bag of optional descriptors,
/// one per core primitive, painted top-to-bottom in fixed-height bands.
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

impl AppLogic for PyAppLogic {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let mut y = 0.0_f32;
        let mut next_band = |height: f32| -> Option<Rect> {
            let remaining = viewport.height - y;
            if remaining <= 0.0 {
                return None;
            }
            let band_height = height.min(remaining);
            let rect = Rect::new(0.0, y, viewport.width, band_height);
            y += band_height;
            Some(rect)
        };

        if let Some(button) = &self.button {
            if let Some(rect) = next_band(3.0) {
                backend.draw_button(rect, button);
            }
        }
        if let Some(text_display) = &self.text_display {
            if let Some(rect) = next_band(3.0) {
                backend.draw_text_display(rect, text_display);
            }
        }
        if let Some(list) = &self.list {
            if let Some(rect) = next_band(4.0) {
                backend.draw_list(rect, list);
            }
        }
        if let Some(tree) = &self.tree {
            if let Some(rect) = next_band(4.0) {
                backend.draw_tree(rect, tree);
            }
        }
        if let Some(data_table) = &self.data_table {
            if let Some(rect) = next_band(4.0) {
                backend.draw_data_table(rect, data_table, None);
            }
        }
        if let Some(tab_bar) = &self.tab_bar {
            if let Some(rect) = next_band(3.0) {
                backend.draw_tab_bar_layout(rect, tab_bar, None);
            }
        }
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let Some(handler) = self.handler.as_ref() else {
            return Reaction::Continue;
        };
        Python::attach(|py| {
            let payload = match pythonize::pythonize(py, &event) {
                Ok(value) => value,
                Err(err) => {
                    *self.pending_error.borrow_mut() = Some(InternalError::new_err(format!(
                        "failed to pythonize event: {err}"
                    )));
                    return Reaction::Exit;
                }
            };
            match handler.call1(py, (payload,)) {
                Ok(_) => Reaction::Continue,
                Err(err) => {
                    *self.pending_error.borrow_mut() = Some(err);
                    Reaction::Exit
                }
            }
        })
    }

    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        let Some(on_tick) = self.on_tick.as_ref() else {
            return Reaction::Continue;
        };
        Python::attach(|py| match on_tick.call0(py) {
            Ok(_) => Reaction::Continue,
            Err(err) => {
                *self.pending_error.borrow_mut() = Some(err);
                Reaction::Exit
            }
        })
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
/// thread could reach the same instance even if it tried); `run_native`
/// wraps an owned `(PyAppLogic, RunConfig)` that `run_native` has already
/// moved out of its `QuiApp` — in both cases the value lives only on this
/// one thread for the closure's duration.
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
    /// to calling `set_handler` right after construction.
    #[new]
    #[pyo3(signature = (width, height, on_event=None))]
    fn new(width: u16, height: u16, on_event: Option<Py<PyAny>>) -> PyResult<Self> {
        if width == 0 || height == 0 {
            return Err(InvalidDimensionsError::new_err(
                "width and height must both be non-zero",
            ));
        }
        let app_logic = PyAppLogic {
            handler: on_event,
            ..PyAppLogic::default()
        };
        Ok(QuiApp {
            driver: TuiDriver::new(app_logic, width, height),
            pending_events: VecDeque::new(),
            poisoned: false,
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

    /// Opens a real OS window (via `quadraui::gtk::run_with`) showing this
    /// app's current descriptors, and blocks the calling thread until that
    /// window closes. `title`, if given, overrides the generic default
    /// window title. Only present when this extension is built with the
    /// `gtk` feature.
    ///
    /// Moves this app's whole [`PyAppLogic`] — every inserted descriptor
    /// plus both registered handlers — out of the headless driver this
    /// `QuiApp` wraps and into the native runner, which takes it by value.
    /// The GIL is released for the duration of the native event loop
    /// (mirroring [`Self::run`]'s `py.detach`); `handle`/`tick` re-acquire
    /// it per callback via `Python::attach`, exactly as they do for the
    /// headless path — neither method needed a single change to drive a
    /// native backend instead of the TUI one, since both only ever see a
    /// `&mut dyn Backend`.
    ///
    /// After the window closes, the headless driver is left holding an
    /// empty `PyAppLogic` (nothing to hand back — `gtk::run_with` owns the
    /// one this call moved out for as long as the window was open):
    /// `step`/`run`/`screen` still work, they just have nothing left to
    /// paint or dispatch to until the caller inserts descriptors/handlers
    /// again. Re-raises whatever exception a registered handler/tick
    /// callback raised while the window was open.
    #[cfg(feature = "gtk")]
    #[pyo3(signature = (title=None))]
    fn run_native(&mut self, py: Python<'_>, title: Option<String>) -> PyResult<()> {
        if self.poisoned {
            return Err(PoisonedError::new_err(
                "this QuiApp is poisoned by an earlier caught panic and can no longer be used",
            ));
        }
        let app_logic = std::mem::take(self.driver.app_mut());
        let native_error = app_logic.pending_error.clone();
        let config = match title {
            Some(title) => quadraui::gtk::RunConfig::new("org.quadraui.app", title),
            None => quadraui::gtk::RunConfig::default(),
        };
        let wrapped = AssertUngil((app_logic, config));
        py.detach(move || {
            // Bind the whole `wrapped` first so the closure captures the
            // `AssertUngil`-wrapped value itself rather than `wrapped.0`
            // directly — 2021-edition disjoint capture would otherwise
            // capture just the field, which isn't `Send` on its own and
            // defeats the point of the wrapper.
            let wrapped = wrapped;
            let (app_logic, config) = wrapped.0;
            quadraui::gtk::run_with(app_logic, config);
        });
        if let Some(err) = native_error.borrow_mut().take() {
            return Err(err);
        }
        Ok(())
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
            }
            Ok(cleared)
        })
    }

    /// Renders the current frame and returns it as a plain-text screen
    /// dump, one row per line.
    fn screen(&mut self) -> PyResult<String> {
        self.guarded(|app| {
            app.driver.render();
            Ok(app.driver.screen())
        })
    }

    /// Whether `needle` appears anywhere in the current rendered frame.
    fn screen_contains(&mut self, needle: &str) -> PyResult<bool> {
        self.guarded(|app| {
            app.driver.render();
            Ok(app.driver.screen_contains(needle))
        })
    }
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
        if let Some(err) = self.driver.app_mut().pending_error.borrow_mut().take() {
            return Err(err);
        }
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
            let mut app = QuiApp::new(10, 10, None).expect("valid dimensions");

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
}
