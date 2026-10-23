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
//! Event marshalling and a GIL-releasing run loop are a later milestone leg
//! — [`PyAppLogic::handle`] and [`PyAppLogic::tick`] are no-ops today.
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Button, DataTable, ListView, Reaction, Rect, TabBar, TextDisplay, TreeView,
    UiEvent,
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

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
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
/// `Sync` here. A GIL-releasing, thread-safe run loop is a later milestone
/// leg.
#[pyclass(module = "quadraui", unsendable)]
struct QuiApp {
    driver: TuiDriver<PyAppLogic>,
}

/// Decodes a Python `dict` into `T` via `pythonize`, raising `ValueError`
/// on a shape mismatch rather than panicking across the PyO3 boundary.
fn decode<'py, T: serde::de::DeserializeOwned>(
    what: &str,
    dict: &Bound<'py, PyDict>,
) -> PyResult<T> {
    pythonize::depythonize(dict.as_any())
        .map_err(|err| PyValueError::new_err(format!("{what}: invalid descriptor: {err}")))
}

#[pymethods]
impl QuiApp {
    /// Creates a new headless app on a `width`x`height` cell grid.
    #[new]
    fn new(width: u16, height: u16) -> PyResult<Self> {
        if width == 0 || height == 0 {
            return Err(PyValueError::new_err(
                "width and height must both be non-zero",
            ));
        }
        Ok(QuiApp {
            driver: TuiDriver::new(PyAppLogic::default(), width, height),
        })
    }

    /// Inserts a `Button` descriptor from a Python dict, returning its slot
    /// handle. Raises `ValueError` if the button slot is already occupied —
    /// call `slot_clear` first.
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
    /// slot handle.
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
    /// slot handle.
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
    /// slot handle.
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
    /// slot handle.
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
    /// handle.
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
    /// slot held anything.
    fn slot_clear(&mut self, slot: i64) -> bool {
        let cleared = self.driver.app_mut().clear_slot(slot);
        if cleared {
            self.driver.render();
        }
        cleared
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
}

/// The compiled extension module `quadraui.python-source`'s
/// `quadraui/__init__.py` imports names from.
#[pymodule]
fn _quadraui(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<QuiApp>()?;
    Ok(())
}
