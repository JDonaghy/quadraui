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
use std::collections::VecDeque;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Button, DataTable, Key, ListView, Modifiers, Reaction, Rect, TabBar,
    TextDisplay, TreeView, UiEvent,
};

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
    pending_error: Option<PyErr>,
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
                    self.pending_error = Some(PyErr::from(err));
                    return Reaction::Exit;
                }
            };
            match handler.call1(py, (payload,)) {
                Ok(_) => Reaction::Continue,
                Err(err) => {
                    self.pending_error = Some(err);
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
                self.pending_error = Some(err);
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
}

/// Wraps a value that is provably safe to move across [`Python::detach`]'s
/// closure boundary even though it isn't [`Send`].
///
/// `detach` only suspends *this* thread's GIL attachment so other Python
/// threads can run concurrently; it never migrates `T` onto a different OS
/// thread. `QuiApp` (the only type this wraps) is `#[pyclass(unsendable)]`,
/// so pyo3 already panics at runtime before any other thread can reach the
/// same instance — nothing can race the detached closure for access to the
/// wrapped value.
struct AssertUngil<T>(T);

// SAFETY: see the type's doc comment — `QuiApp` is pinned to one Python
// thread by `unsendable`, and `detach` keeps execution on that same thread,
// so no concurrent access to the wrapped `!Send` value can occur.
unsafe impl<T> Send for AssertUngil<T> {}

/// Decodes a Python `dict` into `T` via `pythonize`, raising `ValueError`
/// on a shape mismatch rather than panicking across the PyO3 boundary.
fn decode<T: serde::de::DeserializeOwned>(what: &str, dict: &Bound<'_, PyDict>) -> PyResult<T> {
    pythonize::depythonize(dict.as_any())
        .map_err(|err| PyValueError::new_err(format!("{what}: invalid descriptor: {err}")))
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
            return Err(PyValueError::new_err(
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
    /// code point `ch`, no modifiers held, not a repeat. Raises `ValueError`
    /// if `ch` isn't a valid Unicode scalar value.
    fn push_key_event(&mut self, ch: u32) -> PyResult<()> {
        let ch = char::from_u32(ch)
            .ok_or_else(|| PyValueError::new_err(format!("invalid Unicode code point: {ch}")))?;
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
            return Err(PyValueError::new_err(
                "width and height must both be non-zero",
            ));
        }
        self.driver.resize(width, height);
        Ok(())
    }

    /// Advances the runner by one step: dispatches the oldest queued event,
    /// or ticks the app if none is queued. Returns `True` if the app is
    /// still running afterwards, `False` once it has exited. Re-raises
    /// whatever exception a registered handler/tick callback raised while
    /// handling this step.
    fn step(&mut self) -> PyResult<bool> {
        self.step_once()
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
                running = app.0.step_once()?;
                if !running {
                    break;
                }
            }
            Ok(running)
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
    /// slot held anything. Raises `ValueError` if `slot` doesn't name one
    /// of this app's six fixed primitive slots.
    fn slot_clear(&mut self, slot: i64) -> PyResult<bool> {
        if !PyAppLogic::is_known_slot(slot) {
            return Err(PyValueError::new_err(format!("unknown slot id: {slot}")));
        }
        let cleared = self.driver.app_mut().clear_slot(slot);
        if cleared {
            self.driver.render();
        }
        Ok(cleared)
    }

    /// Renders the current frame and returns it as a plain-text screen
    /// dump, one row per line.
    fn screen(&mut self) -> String {
        self.driver.render();
        self.driver.screen()
    }

    /// Whether `needle` appears anywhere in the current rendered frame.
    fn screen_contains(&mut self, needle: &str) -> bool {
        self.driver.render();
        self.driver.screen_contains(needle)
    }
}

impl QuiApp {
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
            return Err(PyRuntimeError::new_err(format!(
                "{what}: slot already occupied; call slot_clear({slot}) first"
            )));
        }
        let value: T = decode(what, descriptor)?;
        store(self.driver.app_mut(), value);
        self.driver.render();
        Ok(slot)
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
        if let Some(err) = self.driver.app_mut().pending_error.take() {
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
    Ok(())
}
