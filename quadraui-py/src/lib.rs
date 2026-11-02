//! PyO3 bindings over `quadraui`'s headless TUI driver, plus the real
//! terminal and native-window runners that take it over.
//!
//! The language-neutral hit testing, semantic event synthesis, text-input/
//! focus/drag controllers, retained descriptor model, and render -> frame
//! assembly this binding drives all live in `quadraui_binding_core` (see
//! that crate's module doc) — this file is the PyO3 leaf around it: Python
//! `dict` decode (`insert_*`, [`render_frame_from_dict`],
//! `layout_node_from_dict`), [`PyAppLogic`]'s callback bridge, and the
//! `QuiApp` surface below.
//!
//! [`QuiApp`] wraps a `quadraui::tui::testing::TuiDriver` running an
//! in-crate [`AppLogic`] that holds one optional descriptor per core
//! primitive (button, text display, list, tree, data table, tab bar), plus
//! any number of additive primitives (`insert_extra`/`insert_progress`/
//! `insert_spinner`/`insert_image`) registered in [`EXTRA_PRIMITIVES`] —
//! see that constant's doc for why registering a further additive
//! primitive never touches the six fixed slots. Each `insert_*` method
//! takes a Python `dict`, decodes it into the matching descriptor type via
//! [`pythonize::depythonize`], stores it in that primitive's slot, and
//! re-renders immediately so `screen()`/`screen_contains()` reflect it
//! without a separate step.
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
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;
use std::time::Duration;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
#[cfg(test)]
use pyo3::types::PyTuple;
use pyo3::types::{PyDict, PyList};

#[cfg(test)]
use quadraui::primitives::scrollbar::{scroll_by_clamped, ThumbDrag};
use quadraui::tui::testing::TuiDriver;
use quadraui::tui::{RunConfig, StepOutcome, TuiBackend, TuiRunner};
#[cfg(test)]
use quadraui::wheel_row_delta;
#[cfg(test)]
use quadraui::NamedKey;
use quadraui::{
    ActivityBar, AppLogic, Backend, BoardModel, Button, Chart, ComboBox, CommandCenter,
    CommandLine, ContextMenu, ContextMenuItem, ContextMenuPlacement, DataTable, DatePicker,
    DiffView, Form, Image, Key, ListView, MenuBar, MessageList, Minimap, Modifiers,
    MultiSectionView, NumberInput, Palette, Panel, PipelineView, Point, ProgressBar, RadioGroup,
    Reaction, Rect, Scrollbar, SidebarPanel, Spinner, Split, SplitDirection, SplitTree, StatusBar,
    Switch, TabBar, TabItem, TextDisplay, TextInput, Toolbar, TreeView, UiEvent, WidgetId,
};

// The language-neutral `event -> hit -> semantic -> render` engine this
// binding is a thin PyO3 leaf over -- see
// `quadraui-binding-core/src/lib.rs`'s module doc. Imported under their
// bare names (rather than a `qbc::` path prefix) so this file's own call
// sites, most of which predate the crate split, need no further change.
use quadraui_binding_core::{
    draw_bands, paint_layout, EventSink, ExtraPaint, HitDetail, InteractionEngine, LayoutLeaf,
    LayoutNode, LayoutPanel, LayoutSidebarPanel, LayoutSplit, LayoutSplitTree, LayoutTabEntry,
    LayoutTabs, LeafDescriptor, RenderFrame, RenderRefs, LEAF_KIND_KEYS,
};
#[cfg(test)]
use quadraui_binding_core::{tree_scroll_state, EXTRA_PRIMITIVE_KINDS};

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
/// fresh set of descriptors instead of reading these static fields. Beyond
/// the six fixed slots below, `extra_slots` holds any number of additional,
/// additive primitives keyed by kind name (see [`EXTRA_PRIMITIVES`]);
/// adding a new one there never touches the six fixed fields or their
/// `SLOT_*`/`is_known_slot`/`slot_clear` machinery.
#[derive(Default)]
struct PyAppLogic {
    button: Option<Button>,
    text_display: Option<TextDisplay>,
    list: Option<ListView>,
    tree: Option<TreeView>,
    data_table: Option<DataTable>,
    tab_bar: Option<TabBar>,
    /// Additive primitive slots, keyed by kind name (`"progress"`,
    /// `"spinner"`, `"image"`, ...). Painted after the six fixed bands in
    /// [`EXTRA_PRIMITIVES`] order. A new kind here is a self-contained add —
    /// unlike the six fields above, it needs no new field on this struct and
    /// no new arm on `slot_occupied`/`is_known_slot`/`clear_slot`.
    extra_slots: HashMap<String, Rc<dyn ExtraPaint>>,
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
    /// The language-neutral `event -> hit -> semantic` orchestration engine
    /// (hit map, divider/scrollbar drag state, text-input cursor cache,
    /// menu-bar/context-menu controllers) this binding is a thin PyO3 leaf
    /// over — see `quadraui_binding_core::engine`'s module doc.
    /// [`AppLogic::render`] feeds it this frame's painted hits via
    /// [`InteractionEngine::set_hits`]; [`handle`] forwards to
    /// [`InteractionEngine::handle`] with a [`PyEventSink`] that bridges
    /// back into the registered Python handler.
    engine: InteractionEngine,
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

impl PyAppLogic {
    /// Whether `kind` names a currently-occupied additive primitive slot.
    fn extra_occupied(&self, kind: &str) -> bool {
        self.extra_slots.contains_key(kind)
    }

    /// Empties one additive primitive slot, returning whether it held
    /// anything.
    fn clear_extra_slot(&mut self, kind: &str) -> bool {
        self.extra_slots.remove(kind).is_some()
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
        let mut hits = Vec::new();
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
                    extra: &self.extra_slots,
                },
                &mut hits,
            );
            self.engine.sync_menu_bar(backend, &hits);
            self.engine.render_ctx_menu(backend);
            self.engine.set_hits(hits);
            return;
        };
        match render_output_from_callback(callback, &self.pending_error) {
            Some(RenderOutput::Flat(frame)) => draw_bands(backend, frame.as_refs(), &mut hits),
            Some(RenderOutput::Tree(node)) => {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                paint_layout(backend, area, &node, &mut hits);
            }
            None => {}
        }
        self.engine.sync_menu_bar(backend, &hits);
        self.engine.render_ctx_menu(backend);
        self.engine.set_hits(hits);
    }

    /// Delegates to [`InteractionEngine::tab_stops`], which derives this
    /// frame's tab stops directly from the hit map `render` just fed it via
    /// [`InteractionEngine::set_hits`].
    fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        self.engine.tab_stops()
    }

    /// The thin PyO3 leaf around [`InteractionEngine::handle`]: an app with
    /// no registered handler skips the engine entirely (so an app that
    /// never calls `set_handler` never pays for any of this engine's
    /// state), otherwise this wraps the registered Python handler in a
    /// [`PyEventSink`] and forwards everything else to the engine, which
    /// owns the full `event -> hit -> semantic` orchestration (menu-bar/
    /// context-menu routing, divider/scrollbar drag, click-to-focus,
    /// focus-scoped key/paste routing, and semantic event substitution).
    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let Some(handler) = self.handler.as_ref() else {
            return Reaction::Continue;
        };
        let mut sink = PyEventSink {
            handler,
            pending_error: &self.pending_error,
            render_callback_present: self.render_callback.is_some(),
        };
        self.engine.handle(event, backend, &mut sink)
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

/// Bridges [`quadraui_binding_core::EventSink`] to this binding's
/// registered Python event handler: pythonizes `event` (attaching
/// `hit_detail` under the `"hit"` key when present), calls the handler, and
/// translates its return value into a [`Reaction`]. [`InteractionEngine::handle`]
/// calls this once per dispatched event (twice for a `MouseDown`/
/// `DoubleClick` that also changes focus: once for the synthetic
/// `FocusChanged`, once for the click itself), so every call site goes
/// through identical error handling.
struct PyEventSink<'a> {
    handler: &'a Py<PyAny>,
    pending_error: &'a Rc<RefCell<Option<PyErr>>>,
    render_callback_present: bool,
}

impl EventSink for PyEventSink<'_> {
    fn dispatch(&mut self, event: &UiEvent, hit_detail: Option<&HitDetail>) -> Reaction {
        Python::attach(|py| {
            let payload = match pythonize::pythonize(py, event) {
                Ok(value) => value,
                Err(err) => {
                    record_error(
                        self.pending_error,
                        InternalError::new_err(format!("failed to pythonize event: {err}")),
                    );
                    return Reaction::Exit;
                }
            };
            if let Some(detail) = hit_detail {
                match payload.cast::<PyDict>() {
                    Ok(dict) => match pythonize::pythonize(py, detail) {
                        Ok(detail_value) => {
                            if let Err(err) = dict.set_item("hit", detail_value) {
                                record_error(self.pending_error, err);
                                return Reaction::Exit;
                            }
                        }
                        Err(err) => {
                            record_error(
                                self.pending_error,
                                InternalError::new_err(format!(
                                    "failed to pythonize hit detail: {err}"
                                )),
                            );
                            return Reaction::Exit;
                        }
                    },
                    Err(_) => {
                        record_error(
                            self.pending_error,
                            InternalError::new_err(
                                "failed to attach hit detail: pythonized event payload was not a dict"
                                    .to_string(),
                            ),
                        );
                        return Reaction::Exit;
                    }
                }
            }
            match self.handler.call1(py, (payload,)) {
                Ok(result) => reaction_for_callback(result.bind(py), self.render_callback_present),
                Err(err) => {
                    record_error(self.pending_error, err);
                    Reaction::Exit
                }
            }
        })
    }
}

/// Decodes a render callback's returned `dict` into a [`RenderFrame`],
/// raising `InvalidDescriptorError` if it isn't a `dict`, if an entry isn't
/// a `dict`/`None`, or if an entry's shape doesn't match its slot's
/// descriptor type. The exact recognized key set is `"button"`,
/// `"text_display"`, `"list"`, `"tree"`, `"data_table"`, `"tab_bar"` — one
/// per fixed slot, same names `insert_*`'s Rust-side `what` argument uses —
/// plus any kind registered in [`EXTRA_PRIMITIVES`] for an additive
/// primitive. Any other key (including a near-miss typo like `"tabbar"`)
/// is ignored rather than rejected, so a render callback built against a
/// future slot set stays forward-compatible with this one.
///
/// A free function rather than an inherent `RenderFrame::from_dict` method
/// — `RenderFrame` is defined in `quadraui-binding-core`, so only that
/// crate may add inherent impls for it (Rust's orphan rule).
fn render_frame_from_dict(dict: &Bound<'_, PyDict>) -> PyResult<RenderFrame> {
    let mut extra = HashMap::new();
    for (kind, _) in EXTRA_PRIMITIVES {
        if let Some(value) = decode_extra_slot_entry(RENDER_CALLBACK_WHAT, dict, kind)? {
            extra.insert(kind.to_string(), value);
        }
    }
    Ok(RenderFrame {
        button: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "button")?,
        text_display: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "text_display")?,
        list: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "list")?,
        tree: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "tree")?,
        data_table: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "data_table")?,
        tab_bar: decode_slot_entry(RENDER_CALLBACK_WHAT, dict, "tab_bar")?,
        extra,
    })
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

/// Reads and decodes one optional additive primitive slot entry, keyed by
/// `kind` (one registered in [`EXTRA_PRIMITIVES`]), from a render
/// callback's returned `dict`. Mirrors [`decode_slot_entry`]'s
/// missing/`None`/dict-or-error shape, dispatching the decode through
/// [`decode_extra_primitive`] instead of a single fixed type.
fn decode_extra_slot_entry(
    what: &str,
    dict: &Bound<'_, PyDict>,
    kind: &str,
) -> PyResult<Option<Rc<dyn ExtraPaint>>> {
    match dict.get_item(kind)? {
        Some(value) if !value.is_none() => {
            let value_dict = value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!("{what}: {kind:?} must be a dict or None"))
            })?;
            Ok(Some(decode_extra_primitive(what, kind, value_dict)?))
        }
        _ => Ok(None),
    }
}

/// Decodes one additive primitive descriptor from a Python `dict` into its
/// boxed [`ExtraPaint`] value. Registered per kind in [`EXTRA_PRIMITIVES`].
type ExtraDecoder = fn(&str, &Bound<'_, PyDict>) -> PyResult<Rc<dyn ExtraPaint>>;

/// The additive primitive registry: one `(kind name, decoder)` entry per
/// primitive painted outside the six fixed slots above (see
/// [`LeafDescriptor::Extra`], [`PyAppLogic::extra_slots`] and
/// [`QuiApp::insert_extra`]). Registering a further additive primitive is
/// exactly one appended line here plus that type's [`ExtraPaint`] impl
/// above — one line for a whole-area primitive, a little layout derivation
/// for one that needs a measure pass (`NumberInput`, `ComboBox`,
/// `DatePicker`) — no enum variant, no central match arm, and no length
/// literal to bump, unlike the six fixed slots' `SLOT_*`/`LeafDescriptor`
/// machinery.
const EXTRA_PRIMITIVES: &[(&str, ExtraDecoder)] = &[
    ("progress", |what, dict| {
        Ok(Rc::new(decode::<ProgressBar>(what, dict)?))
    }),
    ("spinner", |what, dict| {
        Ok(Rc::new(decode::<Spinner>(what, dict)?))
    }),
    ("image", |what, dict| {
        Ok(Rc::new(decode::<Image>(what, dict)?))
    }),
    ("form", |what, dict| {
        Ok(Rc::new(decode::<Form>(what, dict)?))
    }),
    ("text_input", |what, dict| {
        Ok(Rc::new(decode::<TextInput>(what, dict)?))
    }),
    ("number_input", |what, dict| {
        Ok(Rc::new(decode::<NumberInput>(what, dict)?))
    }),
    ("combo_box", |what, dict| {
        Ok(Rc::new(decode::<ComboBox>(what, dict)?))
    }),
    ("date_picker", |what, dict| {
        Ok(Rc::new(decode::<DatePicker>(what, dict)?))
    }),
    ("radio_group", |what, dict| {
        Ok(Rc::new(decode::<RadioGroup>(what, dict)?))
    }),
    ("switch", |what, dict| {
        Ok(Rc::new(decode::<Switch>(what, dict)?))
    }),
    ("chart", |what, dict| {
        Ok(Rc::new(decode::<Chart>(what, dict)?))
    }),
    ("message_list", |what, dict| {
        Ok(Rc::new(decode::<MessageList>(what, dict)?))
    }),
    ("board", |what, dict| {
        Ok(Rc::new(decode::<BoardModel>(what, dict)?))
    }),
    ("pipeline_view", |what, dict| {
        Ok(Rc::new(decode::<PipelineView>(what, dict)?))
    }),
    ("minimap", |what, dict| {
        Ok(Rc::new(decode::<Minimap>(what, dict)?))
    }),
    ("multi_section_view", |what, dict| {
        Ok(Rc::new(decode::<MultiSectionView>(what, dict)?))
    }),
    ("diff_view", |what, dict| {
        Ok(Rc::new(decode::<DiffView>(what, dict)?))
    }),
    ("activity_bar", |what, dict| {
        Ok(Rc::new(decode::<ActivityBar>(what, dict)?))
    }),
    ("toolbar", |what, dict| {
        Ok(Rc::new(decode::<Toolbar>(what, dict)?))
    }),
    ("menu_bar", |what, dict| {
        Ok(Rc::new(decode::<MenuBar>(what, dict)?))
    }),
    ("status_bar", |what, dict| {
        Ok(Rc::new(decode::<StatusBar>(what, dict)?))
    }),
    ("command_line", |what, dict| {
        Ok(Rc::new(decode::<CommandLine>(what, dict)?))
    }),
    ("command_center", |what, dict| {
        Ok(Rc::new(decode::<CommandCenter>(what, dict)?))
    }),
    ("palette", |what, dict| {
        Ok(Rc::new(decode::<Palette>(what, dict)?))
    }),
    ("scrollbar", |what, dict| {
        Ok(Rc::new(decode::<Scrollbar>(what, dict)?))
    }),
];

/// The registered additive primitive kind names, in registration order —
/// used to spell out "or one of these" in error messages alongside
/// [`LEAF_KIND_KEYS`].
fn extra_kind_names() -> Vec<&'static str> {
    EXTRA_PRIMITIVES.iter().map(|(kind, _)| *kind).collect()
}

/// Decodes one additive primitive descriptor from a Python dict, dispatched
/// by `kind` through [`EXTRA_PRIMITIVES`]. Raises `InvalidDescriptorError`
/// if `kind` isn't registered.
fn decode_extra_primitive(
    what: &str,
    kind: &str,
    dict: &Bound<'_, PyDict>,
) -> PyResult<Rc<dyn ExtraPaint>> {
    let decoder = EXTRA_PRIMITIVES
        .iter()
        .find(|(registered, _)| *registered == kind)
        .map(|(_, decoder)| *decoder)
        .ok_or_else(|| {
            InvalidDescriptorError::new_err(format!(
                "{what}: unknown additive primitive kind {kind:?} (expected one of {:?})",
                extra_kind_names()
            ))
        })?;
    decoder(what, dict)
}

/// Decodes one layout tree node from a render callback's returned `dict`:
/// a `"row"` or `"column"` key holds the container's children, a `"split"`
/// key holds a two-pane divider ([`layout_split_from_dict`]), a `"tabs"`
/// key holds a tab strip plus per-tab content ([`layout_tabs_from_dict`]);
/// any other shape is decoded as a leaf via [`layout_leaf_from_dict`].
fn layout_node_from_dict(what: &str, dict: &Bound<'_, PyDict>) -> PyResult<LayoutNode> {
    if let Some(value) = dict.get_item("row")? {
        if !value.is_none() {
            return Ok(LayoutNode::Row(layout_children_from_value(what, &value)?));
        }
    }
    if let Some(value) = dict.get_item("column")? {
        if !value.is_none() {
            return Ok(LayoutNode::Column(layout_children_from_value(
                what, &value,
            )?));
        }
    }
    if let Some(value) = dict.get_item("split")? {
        if !value.is_none() {
            let split_dict = value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!("{what}: \"split\" must be a dict"))
            })?;
            return Ok(LayoutNode::Split(layout_split_from_dict(what, split_dict)?));
        }
    }
    if let Some(value) = dict.get_item("tabs")? {
        if !value.is_none() {
            let tabs_dict = value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!("{what}: \"tabs\" must be a dict"))
            })?;
            return Ok(LayoutNode::Tabs(layout_tabs_from_dict(what, tabs_dict)?));
        }
    }
    if let Some(value) = dict.get_item("panel")? {
        if !value.is_none() {
            let panel_dict = value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!("{what}: \"panel\" must be a dict"))
            })?;
            return Ok(LayoutNode::Panel(layout_panel_from_dict(what, panel_dict)?));
        }
    }
    if let Some(value) = dict.get_item("sidebar_panel")? {
        if !value.is_none() {
            let sidebar_panel_dict = value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!("{what}: \"sidebar_panel\" must be a dict"))
            })?;
            return Ok(LayoutNode::SidebarPanel(layout_sidebar_panel_from_dict(
                what,
                sidebar_panel_dict,
            )?));
        }
    }
    if let Some(value) = dict.get_item("split_tree")? {
        if !value.is_none() {
            let split_tree_dict = value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!("{what}: \"split_tree\" must be a dict"))
            })?;
            return Ok(LayoutNode::SplitTree(layout_split_tree_from_dict(
                what,
                split_tree_dict,
            )?));
        }
    }
    Ok(LayoutNode::Leaf(layout_leaf_from_dict(what, dict)?))
}

/// The orientation strings a `"split"` node's `"orientation"` field accepts
/// — lowercase, Python-idiomatic spellings, independent of
/// `quadraui::SplitDirection`'s own (PascalCase, serde-default) wire form.
const SPLIT_ORIENTATIONS: [&str; 2] = ["horizontal", "vertical"];

/// Decodes a `"split"` node: `{"id", "orientation", "ratio", "first",
/// "second"}`, with `"first_min"`/`"second_min"` optional (defaulting to
/// `0.0`). Unlike `quadraui::Split::first_min`/`second_min`, which are in
/// the backend's native unit, these two fields are portable line counts —
/// [`paint_layout`]'s `LayoutNode::Split` arm scales them by
/// `Backend::line_height()` before constructing the `Split` it paints, so a
/// Python caller names a minimum pane size the same way on every backend.
/// `"first"`/`"second"` are full child node dicts, recursively decoded by
/// [`layout_node_from_dict`].
fn layout_split_from_dict(what: &str, dict: &Bound<'_, PyDict>) -> PyResult<LayoutSplit> {
    let id: String = match dict.get_item("id")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!("{what}: split \"id\" must be a string"))
        })?,
        _ => {
            return Err(InvalidDescriptorError::new_err(format!(
                "{what}: split is missing required \"id\""
            )));
        }
    };

    let orientation: String = match dict.get_item("orientation")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!(
                "{what}: split {id:?} \"orientation\" must be a string"
            ))
        })?,
        _ => {
            return Err(InvalidDescriptorError::new_err(format!(
                "{what}: split {id:?} is missing required \"orientation\""
            )));
        }
    };
    let direction = match orientation.as_str() {
        "horizontal" => SplitDirection::Horizontal,
        "vertical" => SplitDirection::Vertical,
        _ => {
            return Err(InvalidDescriptorError::new_err(format!(
                "{what}: split {id:?} \"orientation\" must be one of {SPLIT_ORIENTATIONS:?}, \
                 got {orientation:?}"
            )));
        }
    };

    let ratio: f32 = match dict.get_item("ratio")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!(
                "{what}: split {id:?} \"ratio\" must be a number"
            ))
        })?,
        _ => {
            return Err(InvalidDescriptorError::new_err(format!(
                "{what}: split {id:?} is missing required \"ratio\""
            )));
        }
    };

    let first_min: f32 = match dict.get_item("first_min")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!(
                "{what}: split {id:?} \"first_min\" must be a number"
            ))
        })?,
        _ => 0.0,
    };
    let second_min: f32 = match dict.get_item("second_min")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!(
                "{what}: split {id:?} \"second_min\" must be a number"
            ))
        })?,
        _ => 0.0,
    };

    let first = layout_child_node_from_value(what, &id, "first", dict)?;
    let second = layout_child_node_from_value(what, &id, "second", dict)?;

    Ok(LayoutSplit {
        split: Split {
            id: WidgetId::new(id),
            direction,
            ratio,
            first_min,
            second_min,
        },
        first: Box::new(first),
        second: Box::new(second),
    })
}

/// Reads a `"split"` node's required `"first"`/`"second"` child dict and
/// recursively decodes it via [`layout_node_from_dict`].
fn layout_child_node_from_value(
    what: &str,
    split_id: &str,
    key: &str,
    dict: &Bound<'_, PyDict>,
) -> PyResult<LayoutNode> {
    let value = dict
        .get_item(key)?
        .filter(|v| !v.is_none())
        .ok_or_else(|| {
            InvalidDescriptorError::new_err(format!(
                "{what}: split {split_id:?} is missing required {key:?}"
            ))
        })?;
    let child_dict = value.cast::<PyDict>().map_err(|_| {
        InvalidDescriptorError::new_err(format!(
            "{what}: split {split_id:?} {key:?} must be a dict"
        ))
    })?;
    layout_node_from_dict(what, child_dict)
}

/// Decodes a `"row"`/`"column"` container's children: a `list` of child
/// dicts, each recursively decoded by [`layout_node_from_dict`].
fn layout_children_from_value(what: &str, value: &Bound<'_, PyAny>) -> PyResult<Vec<LayoutNode>> {
    let list = value.cast::<PyList>().map_err(|_| {
        InvalidDescriptorError::new_err(format!("{what}: row/column children must be a list"))
    })?;
    list.iter()
        .map(|child| {
            let child_dict = child.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!(
                    "{what}: each row/column child must be a dict"
                ))
            })?;
            layout_node_from_dict(what, child_dict)
        })
        .collect()
}

/// Reads a container node's required single `"content"` child dict and
/// recursively decodes it via [`layout_node_from_dict`]. Shared by
/// `"panel"` and `"sidebar_panel"` nodes, which each hold exactly one child
/// subtree — unlike `"split"`'s two named panes or `"row"`/`"column"`'s list.
fn layout_required_child_node(
    what: &str,
    node_label: &str,
    node_id: &str,
    key: &str,
    dict: &Bound<'_, PyDict>,
) -> PyResult<LayoutNode> {
    let value = dict
        .get_item(key)?
        .filter(|v| !v.is_none())
        .ok_or_else(|| {
            InvalidDescriptorError::new_err(format!(
                "{what}: {node_label} {node_id:?} is missing required {key:?}"
            ))
        })?;
    let child_dict = value.cast::<PyDict>().map_err(|_| {
        InvalidDescriptorError::new_err(format!(
            "{what}: {node_label} {node_id:?} {key:?} must be a dict"
        ))
    })?;
    layout_node_from_dict(what, child_dict)
}

/// Decodes a `"panel"` node: the chrome dict decoded generically via
/// `decode::<Panel>` (`title`/`actions`/`accent`/`collapsed`, each
/// `#[serde(default)]`, so a minimal `{"id": ...}` is enough) plus a
/// required `"content"` child node, recursively decoded by
/// [`layout_node_from_dict`]. The `"content"` key is harmless noise to
/// `decode::<Panel>` itself — `Panel` has no `deny_unknown_fields`.
fn layout_panel_from_dict(what: &str, dict: &Bound<'_, PyDict>) -> PyResult<LayoutPanel> {
    let panel: Panel = decode(what, dict)?;
    let id = panel.id.as_str().to_owned();
    let content = layout_required_child_node(what, "panel", &id, "content", dict)?;
    Ok(LayoutPanel {
        panel,
        content: Box::new(content),
    })
}

/// Decodes a `"sidebar_panel"` node: the chrome dict decoded generically via
/// `decode::<SidebarPanel>` (optional `toolbar`/`toolbar_height`) plus a
/// required `"content"` child node, recursively decoded by
/// [`layout_node_from_dict`].
fn layout_sidebar_panel_from_dict(
    what: &str,
    dict: &Bound<'_, PyDict>,
) -> PyResult<LayoutSidebarPanel> {
    let sidebar_panel: SidebarPanel = decode(what, dict)?;
    let id = sidebar_panel.id.as_str().to_owned();
    let content = layout_required_child_node(what, "sidebar_panel", &id, "content", dict)?;
    Ok(LayoutSidebarPanel {
        sidebar_panel,
        content: Box::new(content),
    })
}

/// Decodes a `"split_tree"` node: `{"id", "root"}`, where `"root"` is
/// recursively either `{"leaf": <child node dict>}` or `{"split":
/// {"orientation", "ratio", "first", "second"}}` (the same `"orientation"`
/// spelling and values as a top-level `"split"` node — see
/// [`SPLIT_ORIENTATIONS`]). Each `"leaf"` becomes one position in the
/// `quadraui::SplitTree` geometry built alongside it, addressed by a
/// synthetic, internal-only id; its decoded content node is pushed onto the
/// matching [`LayoutSplitTree::leaves`] in the same pre-order, for
/// [`paint_layout`] to zip back against [`SplitTreeLayout::leaves`]' resolved
/// rects at paint time.
fn layout_split_tree_from_dict(what: &str, dict: &Bound<'_, PyDict>) -> PyResult<LayoutSplitTree> {
    let id: String = match dict.get_item("id")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!("{what}: split_tree \"id\" must be a string"))
        })?,
        _ => {
            return Err(InvalidDescriptorError::new_err(format!(
                "{what}: split_tree is missing required \"id\""
            )));
        }
    };

    let root_value = dict
        .get_item("root")?
        .filter(|v| !v.is_none())
        .ok_or_else(|| {
            InvalidDescriptorError::new_err(format!(
                "{what}: split_tree {id:?} is missing required \"root\""
            ))
        })?;
    let root_dict = root_value.cast::<PyDict>().map_err(|_| {
        InvalidDescriptorError::new_err(format!(
            "{what}: split_tree {id:?} \"root\" must be a dict"
        ))
    })?;

    let mut leaves = Vec::new();
    let tree = split_tree_node_from_dict(what, &id, root_dict, &mut leaves)?;
    Ok(LayoutSplitTree { id, tree, leaves })
}

/// Recursively decodes one node of a `"split_tree"`'s `"root"` tree. A
/// `{"leaf": <node dict>}` decodes its content via [`layout_node_from_dict`],
/// pushes it onto `leaves`, and returns a `SplitTree::Leaf` carrying a
/// synthetic id addressing its position (`"{tree_id}::leaf::{index}"`,
/// never user-facing, never checked by [`validate_unique_leaf_ids`]). A
/// `{"split": {...}}` recurses into `"first"`/`"second"` in pre-order,
/// matching the order [`SplitTreeLayout::leaves`] resolves rects in.
fn split_tree_node_from_dict(
    what: &str,
    tree_id: &str,
    dict: &Bound<'_, PyDict>,
    leaves: &mut Vec<LayoutNode>,
) -> PyResult<SplitTree> {
    if let Some(value) = dict.get_item("leaf")? {
        if !value.is_none() {
            let leaf_dict = value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!(
                    "{what}: split_tree {tree_id:?} \"leaf\" must be a dict"
                ))
            })?;
            let content = layout_node_from_dict(what, leaf_dict)?;
            let placeholder = WidgetId::new(format!("{tree_id}::leaf::{}", leaves.len()));
            leaves.push(content);
            return Ok(SplitTree::leaf(placeholder));
        }
    }
    if let Some(value) = dict.get_item("split")? {
        if !value.is_none() {
            let split_dict = value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!(
                    "{what}: split_tree {tree_id:?} \"split\" must be a dict"
                ))
            })?;

            let orientation: String = match split_dict.get_item("orientation")? {
                Some(v) if !v.is_none() => v.extract().map_err(|_| {
                    InvalidDescriptorError::new_err(format!(
                        "{what}: split_tree {tree_id:?} split \"orientation\" must be a string"
                    ))
                })?,
                _ => {
                    return Err(InvalidDescriptorError::new_err(format!(
                        "{what}: split_tree {tree_id:?} split is missing required \"orientation\""
                    )));
                }
            };
            let direction = match orientation.as_str() {
                "horizontal" => SplitDirection::Horizontal,
                "vertical" => SplitDirection::Vertical,
                _ => {
                    return Err(InvalidDescriptorError::new_err(format!(
                        "{what}: split_tree {tree_id:?} split \"orientation\" must be one of \
                         {SPLIT_ORIENTATIONS:?}, got {orientation:?}"
                    )));
                }
            };

            let ratio: f32 = match split_dict.get_item("ratio")? {
                Some(v) if !v.is_none() => v.extract().map_err(|_| {
                    InvalidDescriptorError::new_err(format!(
                        "{what}: split_tree {tree_id:?} split \"ratio\" must be a number"
                    ))
                })?,
                _ => {
                    return Err(InvalidDescriptorError::new_err(format!(
                        "{what}: split_tree {tree_id:?} split is missing required \"ratio\""
                    )));
                }
            };

            let first_value = split_dict
                .get_item("first")?
                .filter(|v| !v.is_none())
                .ok_or_else(|| {
                    InvalidDescriptorError::new_err(format!(
                        "{what}: split_tree {tree_id:?} split is missing required \"first\""
                    ))
                })?;
            let first_dict = first_value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!(
                    "{what}: split_tree {tree_id:?} split \"first\" must be a dict"
                ))
            })?;
            let first = split_tree_node_from_dict(what, tree_id, first_dict, leaves)?;

            let second_value = split_dict
                .get_item("second")?
                .filter(|v| !v.is_none())
                .ok_or_else(|| {
                    InvalidDescriptorError::new_err(format!(
                        "{what}: split_tree {tree_id:?} split is missing required \"second\""
                    ))
                })?;
            let second_dict = second_value.cast::<PyDict>().map_err(|_| {
                InvalidDescriptorError::new_err(format!(
                    "{what}: split_tree {tree_id:?} split \"second\" must be a dict"
                ))
            })?;
            let second = split_tree_node_from_dict(what, tree_id, second_dict, leaves)?;

            return Ok(SplitTree::split(direction, ratio, first, second));
        }
    }
    Err(InvalidDescriptorError::new_err(format!(
        "{what}: split_tree {tree_id:?} node must have a \"leaf\" or \"split\" key"
    )))
}

/// Decodes a `"tabs"` node: `{"id", "tabs", "selected", "closable"}`, where
/// `"tabs"` is a non-empty `list` of `{"id", "title", "content", "closable"}`
/// entries and `"selected"` is the index of the entry whose `"content"`
/// paints beneath the strip this frame. `"content"` is a full child node
/// dict, recursively decoded by [`layout_node_from_dict`], so a tab may hold
/// any primitive or nested row/column/split — giving a per-tab instance by
/// construction rather than a single shared slot.
///
/// The strip-level `"closable"` (default `true`) gates whether *any* tab in
/// this strip draws a close `×` at all — maps to [`TabBar::show_tab_close`].
/// A per-tab `"closable"` (default `true`) then opts that one tab out even
/// when the strip allows it — maps to [`TabItem::is_closable`]. Clicking a
/// drawn close button reaches the handler as
/// `UiEvent::TabBar(id, TabBarEvent::TabClosed { index })`, synthesized by
/// [`semantic_event_for_hit`] the same way a tab click becomes
/// `TabActivated`.
fn layout_tabs_from_dict(what: &str, dict: &Bound<'_, PyDict>) -> PyResult<LayoutTabs> {
    let id: String = match dict.get_item("id")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!("{what}: tabs \"id\" must be a string"))
        })?,
        _ => {
            return Err(InvalidDescriptorError::new_err(format!(
                "{what}: tabs is missing required \"id\""
            )));
        }
    };

    let tabs_value = dict
        .get_item("tabs")?
        .filter(|v| !v.is_none())
        .ok_or_else(|| {
            InvalidDescriptorError::new_err(format!(
                "{what}: tabs {id:?} is missing required \"tabs\""
            ))
        })?;
    let tabs_list = tabs_value.cast::<PyList>().map_err(|_| {
        InvalidDescriptorError::new_err(format!("{what}: tabs {id:?} \"tabs\" must be a list"))
    })?;
    // An empty `"tabs"` list is a valid empty state (no tab strip entries,
    // no selected content) rather than an error -- a render callback that
    // starts with zero tabs (e.g. before any file is open) can still return
    // a `tabs` node instead of special-casing the empty case itself.

    let mut items = Vec::with_capacity(tabs_list.len());
    let mut entries = Vec::with_capacity(tabs_list.len());
    for entry in tabs_list.iter() {
        let entry_dict = entry.cast::<PyDict>().map_err(|_| {
            InvalidDescriptorError::new_err(format!("{what}: tabs {id:?} each tab must be a dict"))
        })?;

        let tab_id: String = match entry_dict.get_item("id")? {
            Some(value) if !value.is_none() => value.extract().map_err(|_| {
                InvalidDescriptorError::new_err(format!(
                    "{what}: tabs {id:?} tab \"id\" must be a string"
                ))
            })?,
            _ => {
                return Err(InvalidDescriptorError::new_err(format!(
                    "{what}: tabs {id:?} tab is missing required \"id\""
                )));
            }
        };
        let title: String = match entry_dict.get_item("title")? {
            Some(value) if !value.is_none() => value.extract().map_err(|_| {
                InvalidDescriptorError::new_err(format!(
                    "{what}: tabs {id:?} tab {tab_id:?} \"title\" must be a string"
                ))
            })?,
            _ => {
                return Err(InvalidDescriptorError::new_err(format!(
                    "{what}: tabs {id:?} tab {tab_id:?} is missing required \"title\""
                )));
            }
        };
        let content_value = entry_dict
            .get_item("content")?
            .filter(|v| !v.is_none())
            .ok_or_else(|| {
                InvalidDescriptorError::new_err(format!(
                    "{what}: tabs {id:?} tab {tab_id:?} is missing required \"content\""
                ))
            })?;
        let content_dict = content_value.cast::<PyDict>().map_err(|_| {
            InvalidDescriptorError::new_err(format!(
                "{what}: tabs {id:?} tab {tab_id:?} \"content\" must be a dict"
            ))
        })?;
        let content = layout_node_from_dict(what, content_dict)?;

        let is_closable: bool = match entry_dict.get_item("closable")? {
            Some(value) if !value.is_none() => value.extract().map_err(|_| {
                InvalidDescriptorError::new_err(format!(
                    "{what}: tabs {id:?} tab {tab_id:?} \"closable\" must be a bool"
                ))
            })?,
            _ => true,
        };

        items.push(TabItem {
            label: title,
            is_closable,
            ..Default::default()
        });
        entries.push(LayoutTabEntry {
            id: tab_id,
            content: Box::new(content),
        });
    }

    let selected: usize = match dict.get_item("selected")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!(
                "{what}: tabs {id:?} \"selected\" must be an integer"
            ))
        })?,
        _ => 0,
    };
    // An explicit "selected" is still validated against the tab count when
    // there are tabs to select; with zero tabs there is nothing to select
    // and `selected` is ignored (`LayoutNode::Tabs`'s paint arm already
    // treats an out-of-range/empty `entries.get(selected)` as "no content").
    if !items.is_empty() && selected >= items.len() {
        return Err(InvalidDescriptorError::new_err(format!(
            "{what}: tabs {id:?} \"selected\" index {selected} is out of range for \
             {len} tab(s)",
            len = items.len()
        )));
    }
    if let Some(item) = items.get_mut(selected) {
        item.is_active = true;
    }

    let show_tab_close: bool = match dict.get_item("closable")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!(
                "{what}: tabs {id:?} \"closable\" must be a bool"
            ))
        })?,
        _ => true,
    };

    Ok(LayoutTabs {
        tab_bar: TabBar {
            id: WidgetId::new(id),
            tabs: items,
            scroll_offset: 0,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close,
            compact: false,
        },
        entries,
        selected,
    })
}

/// Decodes one `{"id": ..., "<kind>": <descriptor>}` leaf dict. `"id"`
/// must be present and a string; exactly one of [`LEAF_KIND_KEYS`] or an
/// [`EXTRA_PRIMITIVES`] kind must be present and hold a `dict` — a leaf
/// with two keys from either set (fixed+fixed, fixed+extra, or
/// extra+extra) is rejected by the single `found` duplicate check shared
/// across both loops below.
fn layout_leaf_from_dict(what: &str, dict: &Bound<'_, PyDict>) -> PyResult<LayoutLeaf> {
    let id: String = match dict.get_item("id")? {
        Some(value) if !value.is_none() => value.extract().map_err(|_| {
            InvalidDescriptorError::new_err(format!("{what}: leaf \"id\" must be a string"))
        })?,
        _ => {
            return Err(InvalidDescriptorError::new_err(format!(
                "{what}: leaf is missing required \"id\""
            )));
        }
    };

    let mut found: Option<LeafDescriptor> = None;
    for key in LEAF_KIND_KEYS {
        let Some(value) = dict.get_item(key)? else {
            continue;
        };
        if value.is_none() {
            continue;
        }
        let value_dict = value.cast::<PyDict>().map_err(|_| {
            InvalidDescriptorError::new_err(format!("{what}: leaf {id:?} {key:?} must be a dict"))
        })?;
        if found.is_some() {
            return Err(InvalidDescriptorError::new_err(format!(
                "{what}: leaf {id:?} has more than one descriptor kind"
            )));
        }
        found = Some(match key {
            "button" => LeafDescriptor::Button(decode(what, value_dict)?),
            "text_display" => LeafDescriptor::TextDisplay(decode(what, value_dict)?),
            "list" => LeafDescriptor::List(decode(what, value_dict)?),
            "tree" => LeafDescriptor::Tree(decode(what, value_dict)?),
            "data_table" => LeafDescriptor::DataTable(decode(what, value_dict)?),
            "tab_bar" => LeafDescriptor::TabBar(decode(what, value_dict)?),
            _ => unreachable!("LEAF_KIND_KEYS is exhaustive"),
        });
    }
    // Additive primitive kinds share the same leaf dict shape and the same
    // `found` duplicate-detection as the six fixed kinds above; a new
    // additive primitive only ever adds a line to `EXTRA_PRIMITIVES`, never
    // a new loop here.
    for (kind, _) in EXTRA_PRIMITIVES {
        let Some(value) = dict.get_item(*kind)? else {
            continue;
        };
        if value.is_none() {
            continue;
        }
        let value_dict = value.cast::<PyDict>().map_err(|_| {
            InvalidDescriptorError::new_err(format!("{what}: leaf {id:?} {kind:?} must be a dict"))
        })?;
        if found.is_some() {
            return Err(InvalidDescriptorError::new_err(format!(
                "{what}: leaf {id:?} has more than one descriptor kind"
            )));
        }
        found = Some(LeafDescriptor::Extra(decode_extra_primitive(
            what, kind, value_dict,
        )?));
    }

    let descriptor = found.ok_or_else(|| {
        InvalidDescriptorError::new_err(format!(
            "{what}: leaf {id:?} is missing a descriptor (one of {LEAF_KIND_KEYS:?} or {:?})",
            extra_kind_names()
        ))
    })?;

    Ok(LayoutLeaf { id, descriptor })
}

/// Walks `node` collecting every leaf and split `id` into one namespace,
/// raising `InvalidDescriptorError` on the first repeat. A unique id per
/// leaf/split lets host code address one instance unambiguously even when a
/// kind appears more than once, and keeps a split's id from silently
/// shadowing a leaf's.
fn validate_unique_leaf_ids(node: &LayoutNode) -> PyResult<()> {
    fn walk(node: &LayoutNode, seen: &mut HashSet<String>) -> PyResult<()> {
        match node {
            LayoutNode::Row(children) | LayoutNode::Column(children) => {
                for child in children {
                    walk(child, seen)?;
                }
                Ok(())
            }
            LayoutNode::Leaf(leaf) => {
                if !seen.insert(leaf.id.clone()) {
                    return Err(InvalidDescriptorError::new_err(format!(
                        "render: duplicate leaf id {:?}",
                        leaf.id
                    )));
                }
                Ok(())
            }
            LayoutNode::Split(split) => {
                let id = split.split.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(InvalidDescriptorError::new_err(format!(
                        "render: duplicate leaf id {id:?}"
                    )));
                }
                walk(&split.first, seen)?;
                walk(&split.second, seen)
            }
            LayoutNode::Tabs(tabs) => {
                let id = tabs.tab_bar.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(InvalidDescriptorError::new_err(format!(
                        "render: duplicate leaf id {id:?}"
                    )));
                }
                for entry in &tabs.entries {
                    if !seen.insert(entry.id.clone()) {
                        return Err(InvalidDescriptorError::new_err(format!(
                            "render: duplicate leaf id {:?}",
                            entry.id
                        )));
                    }
                    walk(&entry.content, seen)?;
                }
                Ok(())
            }
            LayoutNode::Panel(panel_node) => {
                let id = panel_node.panel.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(InvalidDescriptorError::new_err(format!(
                        "render: duplicate leaf id {id:?}"
                    )));
                }
                walk(&panel_node.content, seen)
            }
            LayoutNode::SidebarPanel(sidebar_node) => {
                let id = sidebar_node.sidebar_panel.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(InvalidDescriptorError::new_err(format!(
                        "render: duplicate leaf id {id:?}"
                    )));
                }
                walk(&sidebar_node.content, seen)
            }
            LayoutNode::SplitTree(split_tree) => {
                if !seen.insert(split_tree.id.clone()) {
                    return Err(InvalidDescriptorError::new_err(format!(
                        "render: duplicate leaf id {:?}",
                        split_tree.id
                    )));
                }
                for leaf in &split_tree.leaves {
                    walk(leaf, seen)?;
                }
                Ok(())
            }
        }
    }
    let mut seen = HashSet::new();
    walk(node, &mut seen)
}

/// The decoded shape of a render callback's return value: either the flat
/// per-slot `dict` ([`RenderFrame`], the pre-layout-tree shape) or a
/// layout tree ([`LayoutNode`]).
enum RenderOutput {
    Flat(Box<RenderFrame>),
    Tree(LayoutNode),
}

/// Invokes a registered render callback with no arguments and decodes its
/// returned `dict` into a [`RenderOutput`]. A `dict` carrying a `"row"`,
/// `"column"`, `"split"`, `"tabs"`, `"panel"`, `"sidebar_panel"`, or
/// `"split_tree"` key is parsed as a layout tree; any other `dict` is
/// parsed as the flat per-slot shape, so a render callback written against
/// the flat shape keeps working unchanged. On any failure
/// — the callback raising,
/// returning something other than a `dict`, or an entry with the wrong
/// shape — records the error in `pending_error` and returns `None`, leaving
/// the caller to skip drawing this frame rather than paint a half-decoded
/// one.
///
/// Every call site that can trigger a render — `step`/`run`, `screen`,
/// `screen_contains`, `insert_*`, and `slot_clear` — drains `pending_error`
/// after rendering and re-raises it, so a render-callback failure surfaces
/// the same way a `handle`/`tick` callback failure does no matter which of
/// those triggered the repaint. `run_tui`/`run_native` additionally break
/// their loop as soon as `pending_error` is set, since nothing else is
/// polling it on the Python side while the GIL is detached.
fn render_output_from_callback(
    callback: &Py<PyAny>,
    pending_error: &Rc<RefCell<Option<PyErr>>>,
) -> Option<RenderOutput> {
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

        let is_tree = matches!(dict.get_item("row"), Ok(Some(ref value)) if !value.is_none())
            || matches!(dict.get_item("column"), Ok(Some(ref value)) if !value.is_none())
            || matches!(dict.get_item("split"), Ok(Some(ref value)) if !value.is_none())
            || matches!(dict.get_item("tabs"), Ok(Some(ref value)) if !value.is_none())
            || matches!(dict.get_item("panel"), Ok(Some(ref value)) if !value.is_none())
            || matches!(dict.get_item("sidebar_panel"), Ok(Some(ref value)) if !value.is_none())
            || matches!(dict.get_item("split_tree"), Ok(Some(ref value)) if !value.is_none());
        if is_tree {
            let parsed = layout_node_from_dict(RENDER_CALLBACK_WHAT, dict)
                .and_then(|node| validate_unique_leaf_ids(&node).map(|()| node));
            return match parsed {
                Ok(node) => Some(RenderOutput::Tree(node)),
                Err(err) => {
                    record_error(pending_error, err);
                    None
                }
            };
        }

        match render_frame_from_dict(dict) {
            Ok(frame) => Some(RenderOutput::Flat(Box::new(frame))),
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

    fn tab_stops(&self, area: ()) -> Vec<(WidgetId, Rect)> {
        self.inner.tab_stops(area)
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
    /// Lazily constructed on the first `clipboard_read_text`/
    /// `clipboard_write_text` call and then kept alive for this `QuiApp`'s
    /// whole lifetime, independent of whether `run_tui` is (or has ever
    /// been) running. `TuiBackend`'s wrapped `TuiClipboard` holds an
    /// `arboard::Clipboard` handle that must stay alive for its Linux
    /// X11/Wayland clipboard-serving thread to keep running: constructing
    /// and dropping a fresh `TuiBackend` per call would risk the clipboard
    /// being silently cleared again right after a write.
    clipboard: Option<TuiBackend>,
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
            clipboard: None,
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
    ///
    /// `mouse=False` skips negotiating `EnableMouseCapture` with the
    /// terminal (`quadraui::tui::RunConfig::no_mouse`), the same opt-out
    /// `examples/tui_no_mouse.rs` demonstrates at the Rust level. With the
    /// default `mouse=True`, mouse reporting is enabled as before and the
    /// terminal's native click-drag text selection stays suppressed for the
    /// app's whole run; an app that never reads mouse events itself should
    /// pass `mouse=False` so a user can select and copy text with the
    /// terminal's own mouse handling instead.
    #[pyo3(signature = (mouse=true))]
    fn run_tui(&mut self, py: Python<'_>, mouse: bool) -> PyResult<()> {
        self.guarded(|app| {
            let app_logic = std::mem::take(app.driver.app_mut());
            // Same double-clone rationale as `run_native`: `tui_error` stays
            // outside the `detach` closure for the caller to read back,
            // `loop_error` is the clone the loop writes a
            // `check_signals()` failure into.
            let tui_error = app_logic.pending_error.clone();
            let loop_error = app_logic.pending_error.clone();
            let wrapped = AssertUngil((app_logic, loop_error));
            let config = if mouse {
                RunConfig::default()
            } else {
                RunConfig::no_mouse()
            };
            let setup_result: io::Result<()> = py.detach(move || {
                let wrapped = wrapped;
                let (app_logic, loop_error) = wrapped.0;
                let mut runner = TuiRunner::new_with(app_logic, config)?;
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

    /// Writes `text` to the system clipboard, via the same
    /// `quadraui::tui::services::TuiClipboard` fallback chain (arboard, then
    /// OSC 52 sent to stdout and `/dev/tty`, then a native CLI tool such as
    /// `wl-copy`/`xclip`/`xsel`) a real terminal backend uses internally —
    /// usable whether or not `run_tui` is (or has ever been) running on
    /// this `QuiApp`, e.g. from a tick/event handler while headlessly
    /// stepping, so a Python app can copy text programmatically instead of
    /// relying on the terminal's own mouse-drag selection. Pair with
    /// `run_tui(mouse=False)` to leave that native selection usable too.
    fn clipboard_write_text(&mut self, text: &str) -> PyResult<()> {
        self.guarded(|app| {
            app.clipboard_backend()
                .services()
                .clipboard()
                .write_text(text);
            Ok(())
        })
    }

    /// Reads the system clipboard's current plain-text contents, or `None`
    /// on an empty / non-text clipboard, or a platform error reading it.
    fn clipboard_read_text(&mut self) -> PyResult<Option<String>> {
        self.guarded(|app| Ok(app.clipboard_backend().services().clipboard().read_text()))
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

    /// Inserts a `ProgressBar` descriptor from a Python dict, returning
    /// `"progress"` back. Raises `RuntimeError` if `"progress"` is already
    /// occupied — call `clear_extra("progress")` first. A thin,
    /// typed-descriptor convenience over [`Self::insert_extra`].
    fn insert_progress(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<String> {
        self.store_extra("progress", "insert_progress", descriptor)
    }

    /// Inserts a `Spinner` descriptor from a Python dict, returning
    /// `"spinner"` back. Raises `RuntimeError` if `"spinner"` is already
    /// occupied — call `clear_extra("spinner")` first. A thin,
    /// typed-descriptor convenience over [`Self::insert_extra`].
    fn insert_spinner(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<String> {
        self.store_extra("spinner", "insert_spinner", descriptor)
    }

    /// Inserts an `Image` descriptor from a Python dict, returning
    /// `"image"` back. Raises `RuntimeError` if `"image"` is already
    /// occupied — call `clear_extra("image")` first. A thin,
    /// typed-descriptor convenience over [`Self::insert_extra`].
    fn insert_image(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<String> {
        self.store_extra("image", "insert_image", descriptor)
    }

    /// Inserts a descriptor for any additive primitive `kind` registered in
    /// `EXTRA_PRIMITIVES` (see [`extra_kind_names`] for the current list),
    /// returning `kind` back. Raises `RuntimeError` if `kind` is already
    /// occupied — call `clear_extra(kind)` first — and
    /// `InvalidDescriptorError` if `kind` isn't registered. Lets a consumer
    /// insert a future additive primitive by name alone, with no new
    /// pymethod needed on this class.
    fn insert_extra(&mut self, kind: &str, descriptor: &Bound<'_, PyDict>) -> PyResult<String> {
        self.store_extra(kind, "insert_extra", descriptor)
    }

    /// Empties an additive primitive slot (one registered in
    /// `EXTRA_PRIMITIVES`), re-rendering so `screen()` no longer shows it.
    /// Returns whether the slot held anything; an unrecognized `kind`
    /// simply has nothing to clear (unlike `slot_clear`, this never raises
    /// for an unknown key).
    fn clear_extra(&mut self, kind: &str) -> PyResult<bool> {
        self.guarded(|app| {
            let cleared = app.driver.app_mut().clear_extra_slot(kind);
            if cleared {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(cleared)
        })
    }

    /// Opens a right-click-style popup menu at `position` (view-local
    /// coordinates) built from `items` — a list of dicts, each matching
    /// `ContextMenuItem`'s shape (`id`, `label`, `detail`, `disabled`,
    /// `checked`, `submenu`; see `quadraui::ContextMenuItem`'s own doc). The
    /// popup is routed through the same `compose::ContextMenuController`
    /// every in-window backend shares, so a click on an item, an Up/Down +
    /// Enter, or Escape/click-outside all resolve the same way regardless
    /// of platform (`TuiBackend::effective_menu_style` always resolves
    /// `Custom`, so this never opens a native popup under `QuiApp`).
    /// Re-renders immediately so `screen()`/`screen_contains()` reflect the
    /// open popup without a separate step.
    ///
    /// Selecting an item surfaces as `UiEvent::ContextMenuItemActivated`
    /// to the registered handler on a later dispatched event; dismissing
    /// with Escape or a click outside surfaces as
    /// `UiEvent::ContextMenuDismissed`. Both arrive through the normal
    /// `handle` pipeline exactly like any other `UiEvent` — there is no
    /// separate callback for them. Calling this again while a popup from
    /// an earlier call is still open replaces it with the new one
    /// silently — `ContextMenuDismissed` does not fire for the one it
    /// replaced.
    ///
    /// Call this from top-level script code (e.g. after `step`/`step_once`
    /// returns, having seen a raw `MouseDown { button: Right, .. }` land
    /// where a menu should open), never from inside the registered event
    /// handler or render callback: both already hold this `QuiApp`'s one
    /// mutable borrow for the whole dispatch, so a reentrant call here hits
    /// the same PyO3 runtime-borrow wall `QuiApp.insert_*`/`slot_clear` do —
    /// see the module doc's "cannot call back into `QuiApp`" note.
    fn show_context_menu(
        &mut self,
        items: &Bound<'_, PyList>,
        position: (f32, f32),
    ) -> PyResult<()> {
        let items: Vec<ContextMenuItem> =
            pythonize::depythonize(items.as_any()).map_err(|err| {
                InvalidDescriptorError::new_err(format!("show_context_menu: invalid items: {err}"))
            })?;
        let mut menu = ContextMenu {
            id: WidgetId::new("qui-context-menu"),
            items,
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        };
        menu.selected_idx = menu.first_selectable();
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            app_logic
                .engine
                .ctx_menu_mut()
                .open(menu, Point::new(position.0, position.1), backend);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
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

    /// Returns this app's lazily constructed, lifetime-persistent
    /// clipboard backend — see the `clipboard` field's doc comment for why
    /// it must not be constructed fresh per call.
    fn clipboard_backend(&mut self) -> &TuiBackend {
        self.clipboard.get_or_insert_with(TuiBackend::new)
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

    /// Shared body of every additive-primitive `insert_*` method (including
    /// the public `insert_extra` pymethod above): reject an already-occupied
    /// `kind`, decode the dict through [`decode_extra_primitive`], store it,
    /// then re-render, returning `kind` back for symmetry with the fixed
    /// `insert_*` methods' slot-handle return value. The additive
    /// counterpart to [`Self::insert`] — `kind` is a string (one registered
    /// in [`EXTRA_PRIMITIVES`]) rather than a fixed `SLOT_*` integer, so a
    /// new additive primitive never needs a new slot constant.
    fn store_extra(
        &mut self,
        kind: &str,
        what: &str,
        descriptor: &Bound<'_, PyDict>,
    ) -> PyResult<String> {
        if self.driver.app().extra_occupied(kind) {
            return Err(SlotOccupiedError::new_err(format!(
                "{what}: {kind:?} already occupied; call clear_extra({kind:?}) first"
            )));
        }
        let value = decode_extra_primitive(what, kind, descriptor)?;
        self.guarded(|app| {
            app.driver
                .app_mut()
                .extra_slots
                .insert(kind.to_string(), value);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(kind.to_string())
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
    use quadraui::{ButtonMask, MouseButton, ScrollDelta};

    /// `quadraui_binding_core::EXTRA_PRIMITIVE_KINDS` is a hand-maintained
    /// list of this binding's own `EXTRA_PRIMITIVES` decode table's keys,
    /// in the same order — `draw_bands`'s extras loop iterates the former
    /// to paint deterministically, so a kind registered in the latter
    /// without a matching entry here would decode and store fine but never
    /// get painted. This guards the two from drifting apart.
    #[test]
    fn extra_primitive_kinds_matches_extra_primitives_key_order() {
        assert!(
            EXTRA_PRIMITIVES
                .iter()
                .map(|(kind, _)| *kind)
                .eq(EXTRA_PRIMITIVE_KINDS.iter().copied()),
            "EXTRA_PRIMITIVE_KINDS must list EXTRA_PRIMITIVES' keys in the same order"
        );
    }

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

    /// A render callback returning `{"row": [...]}` lays its children out
    /// side by side rather than stacked: a `tree` leaf and a `data_table`
    /// leaf share the same top row, with the tree's columns entirely to
    /// the left of the table's.
    #[test]
    fn render_callback_row_places_children_side_by_side() {
        Python::attach(|py| {
            let mut app = QuiApp::new(40, 20, None, None).expect("valid dimensions");
            let callback = py
                .eval(
                    c"lambda: {\"row\": [\
                        {\"id\": \"tree:1\", \"tree\": {\
                            \"id\": \"tree:1\", \
                            \"rows\": [{\
                                \"path\": [0], \"indent\": 0, \"icon\": None, \
                                \"text\": {\"spans\": [{\"text\": \"Alpha\", \"fg\": None, \"bg\": None}]}, \
                                \"badge\": None, \"is_expanded\": None\
                            }], \
                            \"selection_mode\": \"Single\", \"selected_path\": None, \
                            \"style\": {\
                                \"indent\": 2, \"show_chevrons\": True, \
                                \"chevron_expanded\": \"v\", \"chevron_collapsed\": \">\"\
                            }\
                        }}, \
                        {\"id\": \"table:1\", \"data_table\": {\
                            \"id\": \"table:1\", \
                            \"columns\": [{\"title\": \"Name\"}], \
                            \"rows\": [{\"cells\": [{\"spans\": [{\"text\": \"Row1\", \"fg\": None, \"bg\": None}]}]}]\
                        }}\
                    ]}",
                    None,
                    None,
                )
                .expect("dict literal must evaluate")
                .unbind();
            app.set_render(callback);

            let screen = app.screen().expect("render must not raise");
            assert!(screen.contains("Alpha"), "tree content must be painted");
            assert!(screen.contains("Name"), "table header must be painted");
            assert!(screen.contains("Row1"), "table content must be painted");

            let tree_bounds = app
                .driver
                .find_bounds("Alpha")
                .expect("tree content must be on screen");
            // The table's own header row ("Name") shares the leaf's top row
            // with the tree's first row ("Alpha"); the table's data row
            // ("Row1") is one row further down *inside the table's own
            // leaf*, not stacked below the tree's leaf.
            let header_bounds = app
                .driver
                .find_bounds("Name")
                .expect("table header must be on screen");
            let table_bounds = app
                .driver
                .find_bounds("Row1")
                .expect("table content must be on screen");

            assert_eq!(
                tree_bounds.y, header_bounds.y,
                "a Row's children must share the same top row, not stack vertically"
            );
            assert!(
                tree_bounds.x + tree_bounds.width <= header_bounds.x,
                "the tree's columns must be entirely left of the table's: tree={tree_bounds:?} header={header_bounds:?}"
            );
            assert!(
                tree_bounds.x + tree_bounds.width <= table_bounds.x,
                "the tree's columns must be entirely left of the table's: tree={tree_bounds:?} table={table_bounds:?}"
            );
        });
    }

    /// A render callback may place more than one instance of the same
    /// primitive kind in a layout tree -- two `data_table`s in a `Row` both
    /// render, each addressable by its own leaf `id`.
    #[test]
    fn render_callback_row_supports_multiple_instances_of_the_same_kind() {
        Python::attach(|py| {
            let mut app = QuiApp::new(60, 20, None, None).expect("valid dimensions");
            let callback = py
                .eval(
                    c"lambda: {\"row\": [\
                        {\"id\": \"table:left\", \"data_table\": {\
                            \"id\": \"table:left\", \
                            \"columns\": [{\"title\": \"Name\"}], \
                            \"rows\": [{\"cells\": [{\"spans\": [{\"text\": \"LeftCell\", \"fg\": None, \"bg\": None}]}]}]\
                        }}, \
                        {\"id\": \"table:right\", \"data_table\": {\
                            \"id\": \"table:right\", \
                            \"columns\": [{\"title\": \"Name\"}], \
                            \"rows\": [{\"cells\": [{\"spans\": [{\"text\": \"RightCell\", \"fg\": None, \"bg\": None}]}]}]\
                        }}\
                    ]}",
                    None,
                    None,
                )
                .expect("dict literal must evaluate")
                .unbind();
            app.set_render(callback);

            assert!(
                app.screen_contains("LeftCell")
                    .expect("render must not raise"),
                "the first data_table instance must render"
            );
            assert!(
                app.screen_contains("RightCell")
                    .expect("render must not raise"),
                "the second data_table instance must render"
            );
        });
    }

    /// A render callback returning `{"split": {...}}` divides its area into
    /// two unequal panes at `ratio`; pushing a `SplitDividerDragged` event
    /// to a handler that writes the new ratio into the same Python state the
    /// render callback reads moves the divider on the very next frame — the
    /// state -> `render(state)` loop, applied to a `Split` instead of a flat
    /// slot. The new boundary is found via `find_bounds`, never a hardcoded
    /// column, so this only passes if the divider actually moved to ~70% of
    /// the width rather than merely re-painting at the old position.
    #[test]
    fn split_divider_drag_event_moves_the_divider_on_next_frame() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
state = {'ratio': 0.5}

def render():
    return {'split': {
        'id': 'main-split',
        'orientation': 'horizontal',
        'ratio': state['ratio'],
        'first': {'id': 'text:left', 'text_display': {
            'id': 'text:left',
            'lines': [{'spans': [{'text': 'LEFT', 'fg': None, 'bg': None}]}],
        }},
        'second': {'id': 'text:right', 'text_display': {
            'id': 'text:right',
            'lines': [{'spans': [{'text': 'RIGHT', 'fg': None, 'bg': None}]}],
        }},
    }}

def handle(event):
    state['ratio'] = event['SplitDividerDragged']['new_ratio']
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let width = 40u16;
            let mut app =
                QuiApp::new(width, 20, Some(handle), Some(render)).expect("valid dimensions");

            assert!(
                app.screen_contains("LEFT").expect("render must not raise"),
                "left pane must be painted at the initial ratio"
            );
            let left_before = app
                .driver
                .find_bounds("LEFT")
                .expect("left pane must be on screen");
            let right_before = app
                .driver
                .find_bounds("RIGHT")
                .expect("right pane must be on screen");
            assert!(
                left_before.x < right_before.x,
                "the first pane must be left of the second at a 0.5 ratio"
            );

            let event = py
                .eval(
                    c"{'SplitDividerDragged': {'tree': 'main-split', 'split_index': 0, 'new_ratio': 0.7}}",
                    None,
                    None,
                )
                .expect("dict literal must evaluate");
            let event_dict = event.cast::<PyDict>().expect("dict literal must be a dict");
            app.push_event(event_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let right_after = app
                .driver
                .find_bounds("RIGHT")
                .expect("right pane must still be on screen");
            assert!(
                right_after.x > right_before.x,
                "dragging the divider to a larger ratio must widen the first pane: \
                 before={right_before:?} after={right_after:?}"
            );
            let fraction = right_after.x / f32::from(width);
            assert!(
                (fraction - 0.7).abs() <= 0.05,
                "the second pane must now start at ~70% of the width, got {fraction} \
                 (right_after={right_after:?}, width={width})"
            );
        });
    }

    /// A render callback returning `{"tabs": {...}}` paints the strip plus
    /// the `selected` tab's `content`; pushing a `TabBar`/`TabActivated`
    /// event to a handler that writes the new index into the same Python
    /// state the render callback reads switches which tab's content paints
    /// on the very next frame, each tab's content being an ordinary
    /// `data_table` subtree distinct from the other tab's.
    #[test]
    fn tabs_container_shows_selected_tabs_content_and_switches_on_select_event() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
state = {'selected': 0}

def render():
    return {'tabs': {
        'id': 'tabs:main',
        'selected': state['selected'],
        'tabs': [
            {'id': 'tab:alpha', 'title': 'Alpha', 'content': {'id': 'table:alpha', 'data_table': {
                'id': 'table:alpha',
                'columns': [{'title': 'Name'}],
                'rows': [{'cells': [{'spans': [{'text': 'AlphaRow', 'fg': None, 'bg': None}]}]}],
            }}},
            {'id': 'tab:beta', 'title': 'Beta', 'content': {'id': 'table:beta', 'data_table': {
                'id': 'table:beta',
                'columns': [{'title': 'Name'}],
                'rows': [{'cells': [{'spans': [{'text': 'BetaRow', 'fg': None, 'bg': None}]}]}],
            }}},
        ],
    }}

def handle(event):
    state['selected'] = event['TabBar'][1]['TabActivated']['index']
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(60, 20, Some(handle), Some(render)).expect("valid dimensions");

            assert!(
                app.screen_contains("Alpha").expect("render must not raise"),
                "the first tab's title must render in the strip"
            );
            assert!(
                app.screen_contains("Beta").expect("render must not raise"),
                "the second tab's title must render in the strip even while unselected"
            );
            assert!(
                app.screen_contains("AlphaRow")
                    .expect("render must not raise"),
                "the first (selected) tab's content must render"
            );
            assert!(
                !app.screen_contains("BetaRow")
                    .expect("render must not raise"),
                "the second (unselected) tab's content must not render"
            );

            let event = py
                .eval(
                    c"{'TabBar': ['tabs:main', {'TabActivated': {'index': 1}}]}",
                    None,
                    None,
                )
                .expect("dict literal must evaluate");
            let event_dict = event.cast::<PyDict>().expect("dict literal must be a dict");
            app.push_event(event_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            assert!(
                app.screen_contains("BetaRow")
                    .expect("render must not raise"),
                "selecting the second tab must paint its content"
            );
            assert!(
                !app.screen_contains("AlphaRow")
                    .expect("render must not raise"),
                "the first tab's content must no longer render once deselected"
            );
        });
    }

    /// A click on a tab's close `×` reaches the handler as
    /// `UiEvent::TabBar(id, TabBarEvent::TabClosed { index })`, distinct
    /// from a click on the tab body (`TabActivated`) — so a Python handler
    /// can drop the entry from its own `state["open"]` list and re-render
    /// without that tab's title or content.
    #[test]
    fn tabs_container_close_button_click_dispatches_tab_closed_with_its_index() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
state = {'open': ['Alpha', 'Beta'], 'selected': 0}

def render():
    return {'tabs': {
        'id': 'tabs:main',
        'selected': state['selected'],
        'tabs': [
            {'id': f'tab:{name}', 'title': name, 'content': {'id': f'text:{name}', 'text_display': {
                'id': f'text:{name}',
                'lines': [{'spans': [{'text': name, 'fg': None, 'bg': None}]}],
            }}}
            for name in state['open']
        ],
    }}

def handle(event):
    if 'FocusChanged' in event:
        return
    index = event['TabBar'][1]['TabClosed']['index']
    del state['open'][index]
    if state['selected'] >= len(state['open']):
        state['selected'] = max(0, len(state['open']) - 1)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(60, 20, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("Beta").expect("render must not raise"),
                "the second tab's title must render before it is closed"
            );

            let bar_id = WidgetId::new("tabs:main");
            let (cx, cy) = app
                .driver
                .tab_close_center(&bar_id, 1)
                .expect("the second tab must draw a close button by default");
            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(cx, cy),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            assert!(
                !app.screen_contains("Beta").expect("render must not raise"),
                "closing the second tab must drop its title from the strip"
            );
            assert!(
                app.screen_contains("Alpha").expect("render must not raise"),
                "the first tab must remain after closing the second"
            );
        });
    }

    /// A tab group is just a `Split` whose panes are each a `Tabs` -- no
    /// separate "group" concept. Nesting two `Tabs` under `first`/`second`
    /// renders both strips side by side, each with its own tab painting its
    /// own content independently of the other.
    #[test]
    fn tabs_nested_in_split_form_a_tab_group_rendered_side_by_side() {
        Python::attach(|py| {
            let mut app = QuiApp::new(60, 20, None, None).expect("valid dimensions");
            let callback = py
                .eval(
                    c"lambda: {'split': {\
                        'id': 'main-split', \
                        'orientation': 'horizontal', \
                        'ratio': 0.5, \
                        'first': {'tabs': {\
                            'id': 'tabs:left', \
                            'selected': 0, \
                            'tabs': [{'id': 'left:a', 'title': 'LeftTab', 'content': {\
                                'id': 'text:left', 'text_display': {\
                                    'id': 'text:left', \
                                    'lines': [{'spans': [{'text': 'LEFTCONTENT', 'fg': None, 'bg': None}]}], \
                                }\
                            }}], \
                        }}, \
                        'second': {'tabs': {\
                            'id': 'tabs:right', \
                            'selected': 0, \
                            'tabs': [{'id': 'right:a', 'title': 'RightTab', 'content': {\
                                'id': 'text:right', 'text_display': {\
                                    'id': 'text:right', \
                                    'lines': [{'spans': [{'text': 'RIGHTCONTENT', 'fg': None, 'bg': None}]}], \
                                }\
                            }}], \
                        }}, \
                    }}",
                    None,
                    None,
                )
                .expect("dict literal must evaluate")
                .unbind();
            app.set_render(callback);

            assert!(
                app.screen_contains("LeftTab")
                    .expect("render must not raise"),
                "the left tab group's strip must render"
            );
            assert!(
                app.screen_contains("RightTab")
                    .expect("render must not raise"),
                "the right tab group's strip must render"
            );
            assert!(
                app.screen_contains("LEFTCONTENT")
                    .expect("render must not raise"),
                "the left tab group's selected content must render"
            );
            assert!(
                app.screen_contains("RIGHTCONTENT")
                    .expect("render must not raise"),
                "the right tab group's selected content must render"
            );

            let left_bounds = app
                .driver
                .find_bounds("LeftTab")
                .expect("left strip must be on screen");
            let right_bounds = app
                .driver
                .find_bounds("RightTab")
                .expect("right strip must be on screen");
            assert!(
                left_bounds.x < right_bounds.x,
                "the two tab groups must render side by side, left before right: \
                 left={left_bounds:?} right={right_bounds:?}"
            );
        });
    }

    /// `MouseDown`'s own `widget` field arrives `None` on the wire; once a
    /// frame has painted a `button` leaf, dispatching a `MouseDown` at that
    /// button's painted position fills `widget` in with the button's id and
    /// attaches a `"hit"` key naming the sub-region struck -- `button_body`
    /// for a click anywhere on its body. Located via `find_bounds`, never a
    /// hardcoded coordinate.
    #[test]
    fn mouse_down_over_a_painted_button_is_enriched_with_its_widget_id() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    return {'button': {'id': 'btn:ok', 'label': 'OK'}}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(40, 10, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("OK").expect("render must not raise"),
                "the button's label must be painted"
            );
            let bounds = app
                .driver
                .find_bounds("OK")
                .expect("button label must be on screen");

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(bounds.x + 0.5, bounds.y + 0.5),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined");
            let received_list = received.cast::<PyList>().expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "the click must emit both a FocusChanged (nothing was focused yet) and \
                 the MouseDown itself"
            );
            let focus_dict = received_list
                .get_item(0)
                .expect("first item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let focused: String = focus_dict
                .get_item("FocusChanged")
                .expect("dict lookup")
                .expect("the click must focus the button before the click event itself")
                .extract()
                .expect("FocusChanged must carry the newly focused widget's id as a string");
            assert_eq!(focused, "btn:ok", "the clicked button must receive focus");
            let event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let mouse_down = event_dict
                .get_item("MouseDown")
                .expect("dict lookup")
                .expect("payload must carry a MouseDown key")
                .cast_into::<PyDict>()
                .expect("MouseDown payload must be a dict");
            let widget: String = mouse_down
                .get_item("widget")
                .expect("dict lookup")
                .expect("widget key must be present")
                .extract()
                .expect("widget must be a string once populated");
            assert_eq!(
                widget, "btn:ok",
                "the button's own id must be filled into widget"
            );
            let hit = event_dict
                .get_item("hit")
                .expect("dict lookup")
                .expect("hit key must be attached alongside the MouseDown key")
                .cast_into::<PyDict>()
                .expect("hit must be a dict");
            let kind: String = hit
                .get_item("kind")
                .expect("dict lookup")
                .expect("kind key must be present")
                .extract()
                .expect("kind must be a string");
            assert_eq!(
                kind, "button_body",
                "a click on the body must report button_body"
            );
        });
    }

    /// A primitive nested inside a `Split` pane -- not the `Split` itself --
    /// must claim a click landing on it: `PaintedHit::Split` covers its
    /// whole pane with no "miss" case, so it is pushed into the hit map
    /// *before* its children are painted, letting the child's own
    /// `PaintedHit` (pushed after, so checked first in reverse paint order)
    /// win. Without that ordering the handler below would see `widget` set
    /// to the split's own id and `hit.kind` of `split_first_pane` instead of
    /// the button's.
    #[test]
    fn mouse_down_over_a_button_nested_in_a_split_pane_resolves_to_the_button_not_the_pane() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    return {'split': {
        'id': 'main-split',
        'orientation': 'horizontal',
        'ratio': 0.5,
        'first': {'id': 'btn:left', 'button': {'id': 'btn:left', 'label': 'OK'}},
        'second': {'id': 'text:right', 'text_display': {
            'id': 'text:right',
            'lines': [{'spans': [{'text': 'RIGHT', 'fg': None, 'bg': None}]}],
        }},
    }}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(40, 10, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("OK").expect("render must not raise"),
                "the nested button's label must be painted"
            );
            let bounds = app
                .driver
                .find_bounds("OK")
                .expect("button label must be on screen");

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(bounds.x + 0.5, bounds.y + 0.5),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined");
            let received_list = received.cast::<PyList>().expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "the click must emit both a FocusChanged (nothing was focused yet) and \
                 the MouseDown itself"
            );
            let focus_dict = received_list
                .get_item(0)
                .expect("first item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let focused: String = focus_dict
                .get_item("FocusChanged")
                .expect("dict lookup")
                .expect("the click must focus the button before the click event itself")
                .extract()
                .expect("FocusChanged must carry the newly focused widget's id as a string");
            assert_eq!(focused, "btn:left", "the clicked button must receive focus");
            let event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let mouse_down = event_dict
                .get_item("MouseDown")
                .expect("dict lookup")
                .expect("payload must carry a MouseDown key")
                .cast_into::<PyDict>()
                .expect("MouseDown payload must be a dict");
            let widget: String = mouse_down
                .get_item("widget")
                .expect("dict lookup")
                .expect("widget key must be present")
                .extract()
                .expect("widget must be a string once populated");
            assert_eq!(
                widget, "btn:left",
                "the nested button's own id must win over the split's generic pane hit"
            );
            let hit = event_dict
                .get_item("hit")
                .expect("dict lookup")
                .expect("hit key must be attached alongside the MouseDown key")
                .cast_into::<PyDict>()
                .expect("hit must be a dict");
            let kind: String = hit
                .get_item("kind")
                .expect("dict lookup")
                .expect("kind key must be present")
                .extract()
                .expect("kind must be a string");
            assert_eq!(
                kind, "button_body",
                "the hit detail must resolve to the button, not split_first_pane"
            );
        });
    }

    /// A `tree` leaf nested in a `Split`'s `second` pane paints at a
    /// non-zero origin, so its `PaintedHit::Tree` entry's `area` is offset
    /// from `(0, 0)` and `hit_test_painted` must translate the click into
    /// the tree's own LOCAL frame (`point - area.origin`) before consulting
    /// `TreeViewLayout::hit_test`. A `MouseDown` landing on a non-first row
    /// must be replaced outright by the semantic `UiEvent::Tree(RowClicked)`
    /// it stands for, carrying that row's own `TreePath` — not row 0's, and
    /// not left as a raw `MouseDown` with a `"hit"` dict.
    #[test]
    fn mouse_down_over_a_tree_row_nested_in_a_split_pane_emits_a_semantic_row_clicked_event() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    return {'split': {
        'id': 'main-split',
        'orientation': 'horizontal',
        'ratio': 0.3,
        'first': {'id': 'text:left', 'text_display': {
            'id': 'text:left',
            'lines': [{'spans': [{'text': 'LEFT', 'fg': None, 'bg': None}]}],
        }},
        'second': {'id': 'tree:main', 'tree': {
            'id': 'tree:main',
            'rows': [
                {
                    'path': [0],
                    'indent': 0,
                    'icon': None,
                    'text': {'spans': [{'text': 'AlphaRow', 'fg': None, 'bg': None}]},
                    'badge': None,
                    'is_expanded': None,
                },
                {
                    'path': [1],
                    'indent': 0,
                    'icon': None,
                    'text': {'spans': [{'text': 'BetaRow', 'fg': None, 'bg': None}]},
                    'badge': None,
                    'is_expanded': None,
                },
            ],
            'selection_mode': 'Single',
            'selected_path': [0],
            'scroll_offset': 0,
            'has_focus': True,
            'style': {
                'indent': 2,
                'show_chevrons': True,
                'chevron_expanded': '▾',
                'chevron_collapsed': '▸',
                'row_height': 22,
            },
        }},
    }}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(60, 20, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("BetaRow")
                    .expect("render must not raise"),
                "the second row must be painted"
            );
            let bounds = app
                .driver
                .find_bounds("BetaRow")
                .expect("second row's label must be on screen");
            assert!(
                bounds.x > 0.0,
                "the tree pane must be offset from the screen origin (nested in a split's \
                 second pane): bounds={bounds:?}"
            );

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(bounds.x + 0.5, bounds.y + 0.5),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined");
            let received_list = received.cast::<PyList>().expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "the click must emit both a FocusChanged (nothing was focused yet) and the \
                 semantic Tree event"
            );
            let focus_dict = received_list
                .get_item(0)
                .expect("first item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let focused: String = focus_dict
                .get_item("FocusChanged")
                .expect("dict lookup")
                .expect("the click must focus the tree before the click event itself")
                .extract()
                .expect("FocusChanged must carry the newly focused widget's id as a string");
            assert_eq!(focused, "tree:main", "the clicked tree must receive focus");
            let event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let tree_payload = event_dict
                .get_item("Tree")
                .expect("dict lookup")
                .expect("a row click must dispatch a Tree key, not a raw MouseDown")
                .cast_into::<PyTuple>()
                .expect("Tree must pythonize to a (id, event) tuple");
            assert_eq!(tree_payload.len(), 2, "Tree must carry (id, event)");
            let widget: String = tree_payload
                .get_item(0)
                .expect("first element")
                .extract()
                .expect("the tree's own id must be a string");
            assert_eq!(widget, "tree:main", "the widget id must be the tree's own");
            let inner = tree_payload
                .get_item(1)
                .expect("second element")
                .cast_into::<PyDict>()
                .expect("the inner TreeEvent must pythonize to a dict");
            let row_clicked = inner
                .get_item("RowClicked")
                .expect("dict lookup")
                .expect("a row click must carry a RowClicked key")
                .cast_into::<PyDict>()
                .expect("RowClicked payload must be a dict");
            let path: Vec<u16> = row_clicked
                .get_item("path")
                .expect("dict lookup")
                .expect("path key must be present")
                .extract()
                .expect("path must be a list of ints");
            assert_eq!(
                path,
                vec![1],
                "the clicked row's own path (the second row) must be reported, not row 0's"
            );
        });
    }

    /// A `tabs` leaf nested in a `Split`'s `second` pane paints its strip at
    /// a non-zero origin, exercising the same LOCAL-frame translation in
    /// `hit_test_painted`'s `PaintedHit::TabBar` arm as the tree test above.
    /// A `MouseDown` landing on a non-first tab must be replaced by the
    /// semantic `UiEvent::TabBar(TabActivated)` it stands for, carrying that
    /// tab's own index.
    #[test]
    fn mouse_down_over_a_tab_nested_in_a_split_pane_emits_a_semantic_tab_activated_event() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    return {'split': {
        'id': 'main-split',
        'orientation': 'horizontal',
        'ratio': 0.3,
        'first': {'id': 'text:left', 'text_display': {
            'id': 'text:left',
            'lines': [{'spans': [{'text': 'LEFT', 'fg': None, 'bg': None}]}],
        }},
        'second': {'tabs': {
            'id': 'tabs:main',
            'selected': 0,
            'tabs': [
                {'id': 'tab:a', 'title': 'AlphaTab', 'content': {
                    'id': 'text:a', 'text_display': {
                        'id': 'text:a',
                        'lines': [{'spans': [{'text': 'ALPHACONTENT', 'fg': None, 'bg': None}]}],
                    },
                }},
                {'id': 'tab:b', 'title': 'BetaTab', 'content': {
                    'id': 'text:b', 'text_display': {
                        'id': 'text:b',
                        'lines': [{'spans': [{'text': 'BETACONTENT', 'fg': None, 'bg': None}]}],
                    },
                }},
            ],
        }},
    }}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(60, 20, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("BetaTab")
                    .expect("render must not raise"),
                "the second tab's strip label must be painted"
            );
            let bounds = app
                .driver
                .find_bounds("BetaTab")
                .expect("second tab's label must be on screen");
            assert!(
                bounds.x > 0.0,
                "the tab strip must be offset from the screen origin (nested in a split's \
                 second pane): bounds={bounds:?}"
            );

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(bounds.x + 0.5, bounds.y + 0.5),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined");
            let received_list = received.cast::<PyList>().expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "the click must emit both a FocusChanged (nothing was focused yet) and the \
                 semantic TabBar event"
            );
            let focus_dict = received_list
                .get_item(0)
                .expect("first item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let focused: String = focus_dict
                .get_item("FocusChanged")
                .expect("dict lookup")
                .expect("the click must focus the tab bar before the click event itself")
                .extract()
                .expect("FocusChanged must carry the newly focused widget's id as a string");
            assert_eq!(
                focused, "tabs:main",
                "the clicked tab bar must receive focus"
            );
            let event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let tab_bar_payload = event_dict
                .get_item("TabBar")
                .expect("dict lookup")
                .expect("a tab click must dispatch a TabBar key, not a raw MouseDown")
                .cast_into::<PyTuple>()
                .expect("TabBar must pythonize to a (id, event) tuple");
            assert_eq!(tab_bar_payload.len(), 2, "TabBar must carry (id, event)");
            let widget: String = tab_bar_payload
                .get_item(0)
                .expect("first element")
                .extract()
                .expect("the tab bar's own id must be a string");
            assert_eq!(
                widget, "tabs:main",
                "the widget id must be the tab bar's own"
            );
            let inner = tab_bar_payload
                .get_item(1)
                .expect("second element")
                .cast_into::<PyDict>()
                .expect("the inner TabBarEvent must pythonize to a dict");
            let activated = inner
                .get_item("TabActivated")
                .expect("dict lookup")
                .expect("a tab click must carry a TabActivated key")
                .cast_into::<PyDict>()
                .expect("TabActivated payload must be a dict");
            let index: usize = activated
                .get_item("index")
                .expect("dict lookup")
                .expect("index key must be present")
                .extract()
                .expect("index must be an int");
            assert_eq!(
                index, 1,
                "the clicked tab's own index (the second tab) must be reported, not tab 0's"
            );
        });
    }

    /// Two `tree` leaves side by side in a `Row`: `Tab` cycles focus between
    /// them (via core's shared `preprocess_event`/`FocusManager` pipeline,
    /// driven by `PyAppLogic::tab_stops`), and a plain key then routes to
    /// whichever leaf is focused as that leaf's own semantic
    /// `Tree(KeyPressed)` event, not a raw global `KeyPressed`.
    #[test]
    fn tab_cycles_focus_between_two_leaves_and_a_key_routes_to_the_focused_one() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    def leaf(id, label):
        return {'id': id, 'tree': {
            'id': id,
            'rows': [{
                'path': [0], 'indent': 0, 'icon': None,
                'text': {'spans': [{'text': label, 'fg': None, 'bg': None}]},
                'badge': None, 'is_expanded': None,
            }],
            'selection_mode': 'Single', 'selected_path': None,
            'style': {
                'indent': 2, 'show_chevrons': True,
                'chevron_expanded': 'v', 'chevron_collapsed': '>',
            },
        }}
    return {'row': [leaf('tree:1', 'Alpha'), leaf('tree:2', 'Beta')]}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(40, 20, Some(handle), Some(render)).expect("valid dimensions");
            // Force the first render so `hit_map`/`tab_stops` are populated
            // before any key is dispatched.
            let screen = app.screen().expect("render must not raise");
            assert!(screen.contains("Alpha"), "first leaf must be painted");
            assert!(screen.contains("Beta"), "second leaf must be painted");

            fn extract_received<'py>(locals: &Bound<'py, PyDict>) -> Bound<'py, PyList> {
                locals
                    .get_item("received")
                    .expect("dict lookup")
                    .expect("received must be defined")
                    .cast_into::<PyList>()
                    .expect("received must be a list")
            }

            fn focused_id(event_dict: &Bound<'_, PyDict>) -> Option<String> {
                event_dict
                    .get_item("FocusChanged")
                    .expect("dict lookup")
                    .and_then(|value| value.extract::<String>().ok())
            }

            fn tree_key_event(event_dict: &Bound<'_, PyDict>) -> (String, String) {
                let payload = event_dict
                    .get_item("Tree")
                    .expect("dict lookup")
                    .expect("a focused key must dispatch a Tree event, not a raw KeyPressed")
                    .cast_into::<PyTuple>()
                    .expect("Tree must pythonize to a (id, event) tuple");
                let widget: String = payload
                    .get_item(0)
                    .expect("first element")
                    .extract()
                    .expect("the tree's own id must be a string");
                let inner = payload
                    .get_item(1)
                    .expect("second element")
                    .cast_into::<PyDict>()
                    .expect("the inner TreeEvent must pythonize to a dict");
                let key_payload = inner
                    .get_item("KeyPressed")
                    .expect("dict lookup")
                    .expect("a key on a focused tree must carry a KeyPressed key")
                    .cast_into::<PyDict>()
                    .expect("KeyPressed payload must be a dict");
                let key: String = key_payload
                    .get_item("key")
                    .expect("dict lookup")
                    .expect("key field must be present")
                    .extract()
                    .expect("key must be a string");
                (widget, key)
            }

            // First Tab: nothing focused yet, so focus lands on the first
            // tab stop in paint order -- "tree:1".
            app.driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            let received_list = extract_received(&locals);
            assert_eq!(received_list.len(), 1, "Tab must emit exactly one event");
            let event_dict = received_list
                .get_item(0)
                .expect("first item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            assert_eq!(
                focused_id(&event_dict),
                Some("tree:1".to_string()),
                "the first Tab must focus the first leaf in paint order"
            );

            // A plain key now routes to "tree:1", the focused leaf.
            app.driver.dispatch(UiEvent::KeyPressed {
                key: Key::Char('j'),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            let received_list = extract_received(&locals);
            assert_eq!(
                received_list.len(),
                2,
                "the focus-scoped key must emit exactly one more event"
            );
            let event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            assert_eq!(
                tree_key_event(&event_dict),
                ("tree:1".to_string(), "j".to_string()),
                "the key must dispatch as the focused leaf's own semantic Tree(KeyPressed)"
            );

            // Second Tab: focus advances to the other leaf, "tree:2".
            app.driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            let received_list = extract_received(&locals);
            assert_eq!(
                received_list.len(),
                3,
                "the second Tab must emit one more event"
            );
            let event_dict = received_list
                .get_item(2)
                .expect("third item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            assert_eq!(
                focused_id(&event_dict),
                Some("tree:2".to_string()),
                "the second Tab must move focus to the other leaf"
            );

            // A plain key now routes to "tree:2", the newly focused leaf.
            app.driver.dispatch(UiEvent::KeyPressed {
                key: Key::Char('k'),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            let received_list = extract_received(&locals);
            assert_eq!(
                received_list.len(),
                4,
                "the focus-scoped key must emit exactly one more event"
            );
            let event_dict = received_list
                .get_item(3)
                .expect("fourth item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            assert_eq!(
                tree_key_event(&event_dict),
                ("tree:2".to_string(), "k".to_string()),
                "the key must dispatch as the newly focused leaf's own semantic Tree(KeyPressed)"
            );
        });
    }

    /// A branch row's chevron is a distinct hit region from the row body
    /// (`TreeViewLayout::hit_test` carves `[area.x, area.x + chevron_w)` off
    /// as `TreeViewHit::Chevron`, `chevron_w` wide, before the row text
    /// starts). A click there must be replaced by the semantic
    /// `UiEvent::Tree(RowToggleExpand)`, not the row's own `RowClicked`.
    #[test]
    fn mouse_down_over_a_tree_chevron_emits_a_semantic_row_toggle_expand_event() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    return {'tree': {
        'id': 'tree:main',
        'rows': [
            {
                'path': [0], 'indent': 0, 'icon': None,
                'text': {'spans': [{'text': 'AlphaRow', 'fg': None, 'bg': None}]},
                'badge': None, 'is_expanded': True,
            },
            {
                'path': [1], 'indent': 0, 'icon': None,
                'text': {'spans': [{'text': 'BetaRow', 'fg': None, 'bg': None}]},
                'badge': None, 'is_expanded': None,
            },
        ],
        'selection_mode': 'Single', 'selected_path': [0],
        'scroll_offset': 0, 'has_focus': True,
        'style': {
            'indent': 2, 'show_chevrons': True,
            'chevron_expanded': '▾', 'chevron_collapsed': '▸',
            'row_height': 22,
        },
    }}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(40, 10, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("AlphaRow")
                    .expect("render must not raise"),
                "the first (branch) row must be painted"
            );
            // The row's own label starts two cells past the chevron glyph +
            // gap (`draw_tree` leaves `col == 2` once the chevron block is
            // painted), so a click 1.5 cells to the left of the label lands
            // inside the chevron's own hit region without assuming the
            // tree's `area.x` is any particular value.
            let bounds = app
                .driver
                .find_bounds("AlphaRow")
                .expect("first row's label must be on screen");

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(bounds.x - 1.5, bounds.y + 0.5),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined");
            let received_list = received.cast::<PyList>().expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "the click must emit both a FocusChanged (nothing was focused yet) and the \
                 semantic Tree event"
            );
            let focus_dict = received_list
                .get_item(0)
                .expect("first item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let focused: String = focus_dict
                .get_item("FocusChanged")
                .expect("dict lookup")
                .expect("the click must focus the tree before the click event itself")
                .extract()
                .expect("FocusChanged must carry the newly focused widget's id as a string");
            assert_eq!(focused, "tree:main", "the clicked tree must receive focus");
            let event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let tree_payload = event_dict
                .get_item("Tree")
                .expect("dict lookup")
                .expect("a chevron click must dispatch a Tree key, not a raw MouseDown")
                .cast_into::<PyTuple>()
                .expect("Tree must pythonize to a (id, event) tuple");
            assert_eq!(tree_payload.len(), 2, "Tree must carry (id, event)");
            let widget: String = tree_payload
                .get_item(0)
                .expect("first element")
                .extract()
                .expect("the tree's own id must be a string");
            assert_eq!(widget, "tree:main", "the widget id must be the tree's own");
            let inner = tree_payload
                .get_item(1)
                .expect("second element")
                .cast_into::<PyDict>()
                .expect("the inner TreeEvent must pythonize to a dict");
            let toggle = inner
                .get_item("RowToggleExpand")
                .expect("dict lookup")
                .expect("a chevron click must carry a RowToggleExpand key, not RowClicked")
                .cast_into::<PyDict>()
                .expect("RowToggleExpand payload must be a dict");
            let path: Vec<u16> = toggle
                .get_item("path")
                .expect("dict lookup")
                .expect("path key must be present")
                .extract()
                .expect("path must be a list of ints");
            assert_eq!(
                path,
                vec![0],
                "the toggled row's own path (the first, branch row) must be reported"
            );
        });
    }

    /// Double-clicking a tree row must be replaced by the semantic
    /// `UiEvent::Tree(RowDoubleClicked)` it stands for -- the
    /// semantic-substitution block in `PyAppLogic::handle` runs for
    /// `UiEvent::DoubleClick` the same way it runs for `MouseDown`. The tree
    /// is also a tab stop and nothing is focused yet, so click-to-focus fires
    /// a `FocusChanged` ahead of it, exactly as it would for a `MouseDown`
    /// landing in the same spot.
    #[test]
    fn double_click_over_a_tree_row_emits_a_semantic_row_double_clicked_event() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    return {'tree': {
        'id': 'tree:main',
        'rows': [
            {
                'path': [0], 'indent': 0, 'icon': None,
                'text': {'spans': [{'text': 'AlphaRow', 'fg': None, 'bg': None}]},
                'badge': None, 'is_expanded': None,
            },
            {
                'path': [1], 'indent': 0, 'icon': None,
                'text': {'spans': [{'text': 'BetaRow', 'fg': None, 'bg': None}]},
                'badge': None, 'is_expanded': None,
            },
        ],
        'selection_mode': 'Single', 'selected_path': [0],
        'scroll_offset': 0, 'has_focus': True,
        'style': {
            'indent': 2, 'show_chevrons': True,
            'chevron_expanded': '▾', 'chevron_collapsed': '▸',
            'row_height': 22,
        },
    }}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(40, 10, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("BetaRow")
                    .expect("render must not raise"),
                "the second row must be painted"
            );
            let bounds = app
                .driver
                .find_bounds("BetaRow")
                .expect("second row's label must be on screen");

            let click = UiEvent::DoubleClick {
                widget: None,
                position: Point::new(bounds.x + 0.5, bounds.y + 0.5),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("DoubleClick must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined");
            let received_list = received.cast::<PyList>().expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "click-to-focus must fire a FocusChanged ahead of the double-click"
            );
            let focus_dict = received_list
                .get_item(0)
                .expect("first item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let focused: String = focus_dict
                .get_item("FocusChanged")
                .expect("dict lookup")
                .expect("the first event must be a FocusChanged")
                .extract()
                .expect("FocusChanged must carry the tree's own id");
            assert_eq!(
                focused, "tree:main",
                "the double-clicked tree must be focused"
            );
            let event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let tree_payload = event_dict
                .get_item("Tree")
                .expect("dict lookup")
                .expect("a row double-click must dispatch a Tree key, not a raw DoubleClick")
                .cast_into::<PyTuple>()
                .expect("Tree must pythonize to a (id, event) tuple");
            assert_eq!(tree_payload.len(), 2, "Tree must carry (id, event)");
            let widget: String = tree_payload
                .get_item(0)
                .expect("first element")
                .extract()
                .expect("the tree's own id must be a string");
            assert_eq!(widget, "tree:main", "the widget id must be the tree's own");
            let inner = tree_payload
                .get_item(1)
                .expect("second element")
                .cast_into::<PyDict>()
                .expect("the inner TreeEvent must pythonize to a dict");
            let double_clicked = inner
                .get_item("RowDoubleClicked")
                .expect("dict lookup")
                .expect("a double-click must carry a RowDoubleClicked key, not RowClicked")
                .cast_into::<PyDict>()
                .expect("RowDoubleClicked payload must be a dict");
            let path: Vec<u16> = double_clicked
                .get_item("path")
                .expect("dict lookup")
                .expect("path key must be present")
                .extract()
                .expect("path must be a list of ints");
            assert_eq!(
                path,
                vec![1],
                "the double-clicked row's own path (the second row) must be reported"
            );
        });
    }

    /// Clicking an item in a `list` leaf must both focus that leaf (emitting
    /// `FocusChanged`, via `PyAppLogic::handle`'s click-to-focus step) and
    /// dispatch the semantic `List(SelectionChanged)` the click stands for.
    /// A plain key dispatched afterwards must then route to that same,
    /// now-focused leaf as its own `List(KeyPressed)` -- proving
    /// click-to-focus actually redirects subsequent key routing, not just
    /// that a `FocusChanged` event fired once.
    #[test]
    fn mouse_down_over_a_list_item_focuses_it_and_a_later_key_routes_to_it() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    def leaf(id, labels):
        return {'id': id, 'list': {
            'id': id,
            'items': [
                {'text': {'spans': [{'text': label, 'fg': None, 'bg': None}]}}
                for label in labels
            ],
            'selected_idx': 0,
        }}
    return {'row': [leaf('list:1', ['Alpha1', 'Alpha2']), leaf('list:2', ['Beta1', 'Beta2'])]}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(40, 20, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("Beta1").expect("render must not raise"),
                "the second leaf's first item must be painted"
            );
            let bounds = app
                .driver
                .find_bounds("Beta1")
                .expect("second leaf's first item must be on screen");

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(bounds.x + 0.5, bounds.y + 0.5),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined");
            let received_list = received.cast::<PyList>().expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "the click must emit both a FocusChanged and the semantic List event"
            );

            let focus_dict = received_list
                .get_item(0)
                .expect("first item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let focused: String = focus_dict
                .get_item("FocusChanged")
                .expect("dict lookup")
                .expect("the click must focus the clicked leaf before the click event itself")
                .extract()
                .expect("FocusChanged must carry the newly focused widget's id as a string");
            assert_eq!(
                focused, "list:2",
                "the clicked leaf, not the other one, must receive focus"
            );

            let click_event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let list_payload = click_event_dict
                .get_item("List")
                .expect("dict lookup")
                .expect("an item click must dispatch a List key, not a raw MouseDown")
                .cast_into::<PyTuple>()
                .expect("List must pythonize to a (id, event) tuple");
            let widget: String = list_payload
                .get_item(0)
                .expect("first element")
                .extract()
                .expect("the list's own id must be a string");
            assert_eq!(
                widget, "list:2",
                "the widget id must be the clicked leaf's own"
            );
            let inner = list_payload
                .get_item(1)
                .expect("second element")
                .cast_into::<PyDict>()
                .expect("the inner ListViewEvent must pythonize to a dict");
            let changed = inner
                .get_item("SelectionChanged")
                .expect("dict lookup")
                .expect("an item click must carry a SelectionChanged key")
                .cast_into::<PyDict>()
                .expect("SelectionChanged payload must be a dict");
            let idx: usize = changed
                .get_item("idx")
                .expect("dict lookup")
                .expect("idx key must be present")
                .extract()
                .expect("idx must be an int");
            assert_eq!(idx, 0, "the clicked item's own index must be reported");

            // A plain key now routes to "list:2" -- the leaf the click just
            // focused -- as that leaf's own semantic List(KeyPressed), not
            // "list:1" and not a raw global KeyPressed.
            app.driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            let received_list = received
                .cast::<PyList>()
                .expect("received must still be a list");
            assert_eq!(
                received_list.len(),
                3,
                "the focus-scoped key must emit exactly one more event"
            );
            let key_event_dict = received_list
                .get_item(2)
                .expect("third item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let key_payload = key_event_dict
                .get_item("List")
                .expect("dict lookup")
                .expect("the key must dispatch as the focused leaf's own semantic List key")
                .cast_into::<PyTuple>()
                .expect("List must pythonize to a (id, event) tuple");
            let key_widget: String = key_payload
                .get_item(0)
                .expect("first element")
                .extract()
                .expect("the list's own id must be a string");
            assert_eq!(
                key_widget, "list:2",
                "the key must route to the leaf the click focused, not the other leaf"
            );
            let key_inner = key_payload
                .get_item(1)
                .expect("second element")
                .cast_into::<PyDict>()
                .expect("the inner ListViewEvent must pythonize to a dict");
            let key_pressed = key_inner
                .get_item("KeyPressed")
                .expect("dict lookup")
                .expect("the key must carry a KeyPressed key")
                .cast_into::<PyDict>()
                .expect("KeyPressed payload must be a dict");
            let key: String = key_pressed
                .get_item("key")
                .expect("dict lookup")
                .expect("key field must be present")
                .extract()
                .expect("key must be a string");
            assert_eq!(
                key, "Down",
                "the raw key must be mapped to its focus-string name"
            );
        });
    }

    /// A Ctrl-click on a list item is a multi-select gesture
    /// `ListViewEvent::SelectionChanged` can't represent (it carries no
    /// modifiers), so `semantic_event_for_hit` leaves it unmapped: the raw
    /// `MouseDown` reaches Python with its own `modifiers` and its `"hit"`
    /// dict still attached, letting the handler compute the selection
    /// itself. Click-to-focus still runs ahead of it, the same as a plain
    /// click.
    #[test]
    fn ctrl_click_over_a_list_item_leaves_the_raw_mouse_down_and_hit_attached() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    return {'list': {
        'id': 'list:main',
        'items': [
            {'text': {'spans': [{'text': label, 'fg': None, 'bg': None}]}}
            for label in ['Alpha', 'Beta']
        ],
        'selected_idx': 0,
    }}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(40, 10, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("Beta").expect("render must not raise"),
                "the second item must be painted"
            );
            let bounds = app
                .driver
                .find_bounds("Beta")
                .expect("second item's label must be on screen");

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(bounds.x + 0.5, bounds.y + 0.5),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined");
            let received_list = received.cast::<PyList>().expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "the click must still emit a FocusChanged ahead of the raw MouseDown"
            );

            let click_event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let mouse_down = click_event_dict
                .get_item("MouseDown")
                .expect("dict lookup")
                .expect("a Ctrl-click must dispatch the raw MouseDown, not a List key")
                .cast_into::<PyDict>()
                .expect("MouseDown payload must be a dict");
            let widget: String = mouse_down
                .get_item("widget")
                .expect("dict lookup")
                .expect("widget key must be present")
                .extract()
                .expect("widget must be the clicked list's own id");
            assert_eq!(widget, "list:main");
            let modifiers = mouse_down
                .get_item("modifiers")
                .expect("dict lookup")
                .expect("modifiers key must be present")
                .cast_into::<PyDict>()
                .expect("modifiers payload must be a dict");
            let ctrl: bool = modifiers
                .get_item("ctrl")
                .expect("dict lookup")
                .expect("ctrl key must be present")
                .extract()
                .expect("ctrl must be a bool");
            assert!(
                ctrl,
                "the Ctrl modifier must survive onto the raw MouseDown"
            );
            let hit = click_event_dict
                .get_item("hit")
                .expect("dict lookup")
                .expect("the hit dict must still be attached since no semantic event replaced it")
                .cast_into::<PyDict>()
                .expect("hit payload must be a dict");
            let index: usize = hit
                .get_item("index")
                .expect("dict lookup")
                .expect("index key must be present")
                .extract()
                .expect("index must be an int");
            assert_eq!(index, 1, "the clicked item's own index must be reported");
        });
    }

    /// Builds a 30-row tree (`QuiApp::new(40, 10, ...)` can't fit all of
    /// them in its viewport, so a scrollbar is always painted) whose
    /// `handle` tracks the live `scroll_offset` in a Python global so a
    /// later `render()` reflects whatever `ScrollOffsetChanged` the drag
    /// produced. Returns `(app, received-list accessor via `locals`)`.
    fn tree_with_scrollbar_app(py: Python<'_>, locals: &Bound<'_, PyDict>) -> QuiApp {
        py.run(
            c"
received = []
scroll = 0

def render():
    rows = []
    for i in range(30):
        rows.append({
            'path': [i], 'indent': 0, 'icon': None,
            'text': {'spans': [{'text': 'Row%02d' % i, 'fg': None, 'bg': None}]},
            'badge': None, 'is_expanded': None,
        })
    return {'tree': {
        'id': 'tree:main',
        'rows': rows,
        'selection_mode': 'Single', 'selected_path': [0],
        'scroll_offset': scroll, 'has_focus': True,
        'style': {
            'indent': 2, 'show_chevrons': True,
            'chevron_expanded': '▾', 'chevron_collapsed': '▸',
            'row_height': 22,
        },
    }}

def handle(event):
    global scroll
    received.append(event)
    if isinstance(event, dict) and 'ScrollOffsetChanged' in event:
        scroll = event['ScrollOffsetChanged']['new_offset']
",
            Some(locals),
            Some(locals),
        )
        .expect("python source must execute");
        let render: Py<PyAny> = locals
            .get_item("render")
            .expect("dict lookup")
            .expect("render must be defined")
            .unbind();
        let handle: Py<PyAny> = locals
            .get_item("handle")
            .expect("dict lookup")
            .expect("handle must be defined")
            .unbind();
        QuiApp::new(40, 10, Some(handle), Some(render)).expect("valid dimensions")
    }

    /// Reads back the real painted `(Scrollbar, total_rows, viewport_rows,
    /// scroll_offset)` for `tree:main`, forcing the first render first
    /// (`QuiApp::new` leaves `hit_map` empty until something actually
    /// paints) and asserting that 30 rows in a 10-row viewport really did
    /// paint a scrollbar.
    fn painted_tree_scrollbar_state(app: &mut QuiApp) -> (Scrollbar, usize, usize, usize) {
        assert!(
            app.screen_contains("Row00").expect("render must not raise"),
            "the top row must be painted before any scroll"
        );
        let hit_map = app.driver.app().engine.hit_map();
        tree_scroll_state(&hit_map, WidgetId::new("tree:main"))
            .expect("a 30-row tree in a 10-row viewport must paint a scrollbar")
    }

    /// Dragging a tree's scrollbar thumb must move the painted window by
    /// translating the drag's absolute motion through
    /// [`ThumbDrag::offset_at`] into a semantic `ScrollOffsetChanged`,
    /// mirroring `compose::TreeController`'s own thumb-drag handling. The
    /// initial press only arms the drag — the raw `MouseDown` still
    /// reaches Python (the thumb-press arm never replaces `event`, it only
    /// strips the `"hit"` payload) — and the window only actually moves
    /// once the drag-translated `MouseMoved` lands.
    #[test]
    fn dragging_a_tree_scrollbar_thumb_moves_the_painted_window() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            let mut app = tree_with_scrollbar_app(py, &locals);
            let (scrollbar, total_rows, viewport_rows, scroll_offset) =
                painted_tree_scrollbar_state(&mut app);
            assert_eq!(total_rows, 30);
            assert_eq!(scroll_offset, 0);
            let max_offset = total_rows.saturating_sub(viewport_rows);
            assert!(
                max_offset > 0,
                "the tree must actually overflow its viewport"
            );

            let track = scrollbar.track;
            let thumb_center_y = track.y + scrollbar.thumb_start + scrollbar.thumb_len / 2.0;
            let press_x = track.x + track.width / 2.0;

            let press = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(press_x, thumb_center_y),
                modifiers: Modifiers::default(),
            };
            let press_any = pythonize::pythonize(py, &press).expect("UiEvent must pythonize");
            let press_dict = press_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(press_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received_list = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined")
                .cast_into::<PyList>()
                .expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "the first click must emit FocusChanged plus the raw MouseDown — the \
                 thumb-press arm only arms the drag, it never replaces the event"
            );
            let raw_down = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            assert!(
                raw_down
                    .get_item("MouseDown")
                    .expect("dict lookup")
                    .is_some(),
                "a thumb press must still dispatch the raw MouseDown, not a semantic substitute"
            );
            assert!(
                raw_down.get_item("hit").expect("dict lookup").is_none(),
                "the thumb-press arm strips the hit payload even though it keeps the raw event"
            );

            let travel = scrollbar.travel();
            let target_y = thumb_center_y + travel;
            let drag = ThumbDrag::begin(
                thumb_center_y,
                scroll_offset as f32,
                travel,
                max_offset as f32,
            );
            let expected_offset = drag
                .offset_at(target_y)
                .expect("travel and max_offset are both positive")
                .round() as usize;
            assert_eq!(
                expected_offset, max_offset,
                "dragging the full travel distance must reach the last valid offset"
            );

            let drag_move = UiEvent::MouseMoved {
                position: Point::new(press_x, target_y),
                buttons: ButtonMask::default(),
            };
            let move_any = pythonize::pythonize(py, &drag_move).expect("UiEvent must pythonize");
            let move_dict = move_any
                .cast::<PyDict>()
                .expect("MouseMoved must pythonize to a dict");
            app.push_event(move_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received_list = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined")
                .cast_into::<PyList>()
                .expect("received must be a list");
            assert_eq!(
                received_list.len(),
                3,
                "the drag-translated MouseMoved must emit exactly one new event"
            );
            let scroll_event = received_list
                .get_item(2)
                .expect("third item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            assert!(
                scroll_event.get_item("hit").expect("dict lookup").is_none(),
                "a drag-translated ScrollOffsetChanged never carries a hit payload"
            );
            let payload = scroll_event
                .get_item("ScrollOffsetChanged")
                .expect("dict lookup")
                .expect(
                    "the drag-translated MouseMoved must dispatch a semantic ScrollOffsetChanged",
                )
                .cast_into::<PyDict>()
                .expect("ScrollOffsetChanged payload must be a dict");
            let new_offset: usize = payload
                .get_item("new_offset")
                .expect("dict lookup")
                .expect("new_offset key must be present")
                .extract()
                .expect("new_offset must be an int");
            assert_eq!(
                new_offset, expected_offset,
                "the reported offset must match ThumbDrag's own math"
            );

            assert!(
                !app.screen_contains("Row00").expect("render must not raise"),
                "the drag must have scrolled the top row out of view"
            );
            // The drag's own MouseDown focuses the tree, so the focus ring
            // now overlays the band's top and bottom rows with border
            // glyphs; check a row strictly inside the new window instead of
            // `new_offset` itself.
            let expected_row = format!("Row{:02}", new_offset + 1);
            assert!(
                app.screen_contains(&expected_row)
                    .expect("render must not raise"),
                "a row from the new scroll offset's window must be painted"
            );
        });
    }

    /// Clicking the scrollbar track below the thumb must page the tree
    /// forward by one viewport — `click_scrollbar`'s `PressZone::After` arm
    /// — replacing the raw `MouseDown` outright with a semantic
    /// `ScrollOffsetChanged` (never dispatching the raw click, never
    /// attaching a `"hit"` payload), so the painted window actually moves.
    #[test]
    fn clicking_the_tree_scrollbar_track_pages_the_painted_window() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            let mut app = tree_with_scrollbar_app(py, &locals);
            let (scrollbar, total_rows, viewport_rows, scroll_offset) =
                painted_tree_scrollbar_state(&mut app);
            assert_eq!(scroll_offset, 0);
            let max_offset = total_rows.saturating_sub(viewport_rows);
            assert!(
                max_offset > 0,
                "the tree must actually overflow its viewport"
            );

            let track = scrollbar.track;
            // Just past the thumb's own bottom edge, strictly inside the
            // track so `Rect::contains`'s half-open bound doesn't miss it.
            let after_y = (track.y + scrollbar.thumb_start + scrollbar.thumb_len + 0.5)
                .min(track.y + track.height - 0.5);
            let press_x = track.x + track.width / 2.0;

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(press_x, after_y),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received_list = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined")
                .cast_into::<PyList>()
                .expect("received must be a list");
            assert_eq!(
                received_list.len(),
                2,
                "a track click must emit FocusChanged plus the semantic ScrollOffsetChanged, \
                 never the raw MouseDown"
            );
            let scroll_event = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            assert!(
                scroll_event
                    .get_item("MouseDown")
                    .expect("dict lookup")
                    .is_none(),
                "a track click must never dispatch the raw MouseDown"
            );
            assert!(
                scroll_event.get_item("hit").expect("dict lookup").is_none(),
                "a track-click ScrollOffsetChanged never carries a hit payload"
            );
            let payload = scroll_event
                .get_item("ScrollOffsetChanged")
                .expect("dict lookup")
                .expect("a track click must dispatch a semantic ScrollOffsetChanged")
                .cast_into::<PyDict>()
                .expect("ScrollOffsetChanged payload must be a dict");
            let new_offset: usize = payload
                .get_item("new_offset")
                .expect("dict lookup")
                .expect("new_offset key must be present")
                .extract()
                .expect("new_offset must be an int");
            let expected_offset = scroll_by_clamped(0, viewport_rows as isize, max_offset);
            assert_eq!(
                new_offset, expected_offset,
                "a track-after click must page forward by exactly one viewport"
            );
            assert!(
                expected_offset > 0,
                "paging forward must actually move the offset"
            );

            // The click's own MouseDown focuses the tree, so the focus ring
            // now overlays the band's top and bottom rows with border
            // glyphs; check a row strictly inside the new window instead of
            // `new_offset` itself.
            let expected_row = format!("Row{:02}", new_offset + 1);
            assert!(
                app.screen_contains(&expected_row)
                    .expect("render must not raise"),
                "a row from the page-click's new scroll offset window must be painted"
            );
        });
    }

    /// A wheel notch landing anywhere over the tree — rows, chevrons, or
    /// the scrollbar itself — must translate straight into a semantic
    /// `ScrollOffsetChanged` via `wheel_row_delta` + `scroll_by_clamped`,
    /// replacing the raw `Scroll` outright rather than dispatching both.
    #[test]
    fn wheel_over_the_tree_scrolls_the_painted_window() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            let mut app = tree_with_scrollbar_app(py, &locals);
            let (_, total_rows, viewport_rows, scroll_offset) =
                painted_tree_scrollbar_state(&mut app);
            assert_eq!(scroll_offset, 0);
            let max_offset = total_rows.saturating_sub(viewport_rows);
            assert!(
                max_offset > 0,
                "the tree must actually overflow its viewport"
            );

            let bounds = app
                .driver
                .find_bounds("Row00")
                .expect("the top row's label must be on screen");
            let wheel = UiEvent::Scroll {
                widget: None,
                delta: ScrollDelta::new(0.0, -3.0),
                position: Point::new(bounds.x + 0.5, bounds.y + 0.5),
            };
            let wheel_any = pythonize::pythonize(py, &wheel).expect("UiEvent must pythonize");
            let wheel_dict = wheel_any
                .cast::<PyDict>()
                .expect("Scroll must pythonize to a dict");
            app.push_event(wheel_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received_list = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined")
                .cast_into::<PyList>()
                .expect("received must be a list");
            assert_eq!(
                received_list.len(),
                1,
                "a wheel notch over the tree must dispatch exactly one event — no FocusChanged \
                 (a wheel notch doesn't move focus) and no raw Scroll alongside it"
            );
            let scroll_event = received_list
                .get_item(0)
                .expect("first item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            assert!(
                scroll_event
                    .get_item("Scroll")
                    .expect("dict lookup")
                    .is_none(),
                "the raw Scroll must never reach Python once it's translated"
            );
            let payload = scroll_event
                .get_item("ScrollOffsetChanged")
                .expect("dict lookup")
                .expect("a wheel notch over the tree must dispatch a semantic ScrollOffsetChanged")
                .cast_into::<PyDict>()
                .expect("ScrollOffsetChanged payload must be a dict");
            let new_offset: usize = payload
                .get_item("new_offset")
                .expect("dict lookup")
                .expect("new_offset key must be present")
                .extract()
                .expect("new_offset must be an int");
            let rows = wheel_row_delta(-3.0, 1.0);
            let expected_offset = scroll_by_clamped(0, rows, max_offset);
            assert_eq!(
                new_offset, expected_offset,
                "the reported offset must match wheel_row_delta + scroll_by_clamped"
            );
            assert!(
                expected_offset > 0,
                "the wheel notch must actually move the offset"
            );

            let expected_row = format!("Row{:02}", new_offset);
            assert!(
                app.screen_contains(&expected_row)
                    .expect("render must not raise"),
                "the wheel notch must scroll the row now at the new offset into view"
            );
        });
    }

    /// A click landing in the scrollbar's own gutter — track or thumb —
    /// must never fall through to tree-row selection: `hit_test_painted`
    /// resolves a scrollbar hit to a `TreeScrollbarThumb`/
    /// `TreeScrollbarTrackBefore`/`TreeScrollbarTrackAfter` detail, and
    /// `handle` never routes any of those through `semantic_event_for_hit`
    /// (the row-selection path), so no `Tree` event is ever dispatched for
    /// a gutter click.
    #[test]
    fn a_click_in_the_tree_scrollbar_gutter_does_not_select_a_row() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            let mut app = tree_with_scrollbar_app(py, &locals);
            let (scrollbar, total_rows, viewport_rows, _) = painted_tree_scrollbar_state(&mut app);
            let max_offset = total_rows.saturating_sub(viewport_rows);
            assert!(
                max_offset > 0,
                "the tree must actually overflow its viewport"
            );

            let track = scrollbar.track;
            let thumb_center_y = track.y + scrollbar.thumb_start + scrollbar.thumb_len / 2.0;
            let press_x = track.x + track.width / 2.0;

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(press_x, thumb_center_y),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            let click_dict = click_any
                .cast::<PyDict>()
                .expect("MouseDown must pythonize to a dict");
            app.push_event(click_dict).expect("valid event shape");
            app.step().expect("no handler/render error");

            let received_list = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined")
                .cast_into::<PyList>()
                .expect("received must be a list");
            for i in 0..received_list.len() {
                let event_dict = received_list
                    .get_item(i)
                    .expect("item must exist")
                    .cast_into::<PyDict>()
                    .expect("event payload must be a dict");
                assert!(
                    event_dict.get_item("Tree").expect("dict lookup").is_none(),
                    "a gutter click must never dispatch a Tree (row-selection) event"
                );
            }
        });
    }

    /// Clicking a `"menu_bar"` extra's top-level item opens its dropdown
    /// (no event reaches Python — a bare open/close is navigation, not an
    /// app-level action) and a later click on one of its items closes the
    /// dropdown and dispatches exactly one `UiEvent::MenuActivated` to the
    /// registered handler, carrying the activated item's id.
    #[test]
    fn clicking_a_menu_bar_item_opens_its_dropdown_and_selecting_emits_menu_activated() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    return {'menu_bar': {
        'id': 'bar:main',
        'items': [
            {'id': 'menu:file', 'label': '&File', 'disabled': False, 'submenu': [
                {'id': 'item:open', 'label': {'spans': [{'text': 'Open', 'fg': None, 'bg': None}]}},
                {'id': 'item:quit', 'label': {'spans': [{'text': 'Quit', 'fg': None, 'bg': None}]}},
            ]},
        ],
        'open_item': None,
        'focused_item': None,
    }}

def handle(event):
    received.append(event)
",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let render: Py<PyAny> = locals
                .get_item("render")
                .expect("dict lookup")
                .expect("render must be defined")
                .unbind();
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app =
                QuiApp::new(60, 20, Some(handle), Some(render)).expect("valid dimensions");
            assert!(
                app.screen_contains("File").expect("render must not raise"),
                "the menu bar's top-level item must be painted"
            );

            let bar_bounds = app
                .driver
                .find_bounds("File")
                .expect("the File label must be on screen");

            let open_click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(bar_bounds.x + 0.5, bar_bounds.y + 0.5),
                modifiers: Modifiers::default(),
            };
            let open_click_any =
                pythonize::pythonize(py, &open_click).expect("UiEvent must pythonize");
            app.push_event(
                open_click_any
                    .cast::<PyDict>()
                    .expect("MouseDown must pythonize to a dict"),
            )
            .expect("valid event shape");
            app.step().expect("no handler/render error");

            let received_list = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined")
                .cast_into::<PyList>()
                .expect("received must be a list");
            assert_eq!(
                received_list.len(),
                0,
                "opening the dropdown is pure navigation: nothing reaches Python yet"
            );

            let item_bounds = app
                .driver
                .find_bounds("Open")
                .expect("the open dropdown must paint its 'Open' item");

            let select_click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(item_bounds.x + 0.5, item_bounds.y + 0.5),
                modifiers: Modifiers::default(),
            };
            let select_click_any =
                pythonize::pythonize(py, &select_click).expect("UiEvent must pythonize");
            app.push_event(
                select_click_any
                    .cast::<PyDict>()
                    .expect("MouseDown must pythonize to a dict"),
            )
            .expect("valid event shape");
            app.step().expect("no handler/render error");

            assert_eq!(
                received_list.len(),
                1,
                "selecting the item must dispatch exactly one event"
            );
            let activated = received_list
                .get_item(0)
                .expect("item must exist")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let activated_id: String = activated
                .get_item("MenuActivated")
                .expect("dict lookup")
                .expect("a MenuActivated event must be dispatched")
                .extract()
                .expect("MenuActivated carries the activated item's id as a string");
            assert_eq!(activated_id, "item:open");
        });
    }

    /// `QuiApp::show_context_menu` opens a painted popup at the given
    /// position; a click on one of its items closes it and dispatches
    /// `UiEvent::ContextMenuItemActivated` to the registered handler —
    /// the same round trip a native right-click context menu produces.
    #[test]
    fn show_context_menu_then_clicking_an_item_emits_context_menu_item_activated() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"received = []\ndef handle(event):\n    received.append(event)\n",
                Some(&locals),
                Some(&locals),
            )
            .expect("python source must execute");
            let handle: Py<PyAny> = locals
                .get_item("handle")
                .expect("dict lookup")
                .expect("handle must be defined")
                .unbind();

            let mut app = QuiApp::new(40, 20, Some(handle), None).expect("valid dimensions");

            let items_any = py
                .eval(
                    c"[
                        {'id': 'copy', 'label': {'spans': [{'text': 'Copy', 'fg': None, 'bg': None}]}},
                        {'id': 'paste', 'label': {'spans': [{'text': 'Paste', 'fg': None, 'bg': None}]}},
                    ]",
                    None,
                    None,
                )
                .expect("list literal must evaluate");
            let items = items_any.cast::<PyList>().expect("literal must be a list");

            app.show_context_menu(items, (2.0, 2.0))
                .expect("show_context_menu must open the popup");
            assert!(
                app.screen_contains("Copy").expect("render must not raise"),
                "the popup's first item must be painted"
            );

            let item_bounds = app
                .driver
                .find_bounds("Copy")
                .expect("the 'Copy' item must be on screen");

            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(item_bounds.x + 0.5, item_bounds.y + 0.5),
                modifiers: Modifiers::default(),
            };
            let click_any = pythonize::pythonize(py, &click).expect("UiEvent must pythonize");
            app.push_event(
                click_any
                    .cast::<PyDict>()
                    .expect("MouseDown must pythonize to a dict"),
            )
            .expect("valid event shape");
            app.step().expect("no handler/render error");

            let received_list = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined")
                .cast_into::<PyList>()
                .expect("received must be a list");
            assert_eq!(
                received_list.len(),
                1,
                "selecting a context menu item must dispatch exactly one event"
            );
            let activated = received_list
                .get_item(0)
                .expect("item must exist")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let activated_id: String = activated
                .get_item("ContextMenuItemActivated")
                .expect("dict lookup")
                .expect("a ContextMenuItemActivated event must be dispatched")
                .extract()
                .expect("ContextMenuItemActivated carries the activated item's id as a string");
            assert_eq!(activated_id, "copy");
        });
    }
}
