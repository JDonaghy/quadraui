//! PyO3 bindings over `quadraui`'s headless TUI driver, plus the real
//! terminal and native-window runners that take it over.
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

use quadraui::tui::testing::TuiDriver;
use quadraui::tui::{RunConfig, StepOutcome, TuiBackend, TuiRunner};
use quadraui::{
    ActivityBar, AppLogic, Backend, BoardModel, Button, ButtonHit, ButtonLayout, Chart, ComboBox,
    CommandCenter, CommandLine, CompletionItemMeasure, DataTable, DataTableEvent, DataTableHit,
    DataTableLayout, DatePicker, DiffView, Form, Image, InteractionState, Key, ListView,
    ListViewEvent, ListViewHit, ListViewLayout, MenuBar, MessageList, Minimap, Modifiers,
    MultiSectionView, NamedKey, NumberInput, Palette, Panel, PipelineView, Point, ProgressBar,
    RadioGroup, Reaction, Rect, Scrollbar, SidebarPanel, Spinner, Split, SplitDirection, SplitHit,
    SplitLayout, SplitTree, StatusBar, Switch, SwitchHit, TabBar, TabBarEvent, TabBarHit,
    TabBarLayout, TabItem, TextDisplay, TextInput, Toolbar, TreeEvent, TreePath, TreeView,
    TreeViewHit, TreeViewLayout, UiEvent, WidgetId,
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
    /// Every primitive this app painted this frame, in paint order, as a
    /// [`PaintedHit`] carrying its id and the hit-testable layout its
    /// backend `draw_*` call returned. Rebuilt from scratch at the start of
    /// every [`AppLogic::render`] call and read by [`handle`] to translate
    /// an incoming pointer event's screen coordinates into the `WidgetId`
    /// (and finer [`HitDetail`]) it landed on, before forwarding it to the
    /// registered Python handler.
    ///
    /// `RefCell` rather than a plain field because `render` takes `&self`
    /// (every backend's `AppLogic::render` is called through a shared
    /// reference — see `AppLogic::AreaId`/`render`'s signature below), so
    /// mutating this during paint needs interior mutability even though
    /// `handle`/`tick` otherwise only ever need `&mut self`.
    hit_map: RefCell<Vec<PaintedHit>>,
    /// Armed by a `MouseDown` landing on a split divider (per [`hit_map`]'s
    /// [`PaintedHit::Split`] hit-testing) and cleared on the matching
    /// `MouseUp`. While armed, [`handle`] turns every `MouseMoved` into a
    /// synthesized `UiEvent::SplitDividerDragged` instead of forwarding the
    /// raw move, so a Python handler never has to recompute split geometry
    /// itself to support divider dragging.
    active_drag: RefCell<Option<ActiveDrag>>,
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
            *self.hit_map.borrow_mut() = hits;
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
        *self.hit_map.borrow_mut() = hits;
    }

    /// Derives this frame's tab stops directly from the hit map `render`
    /// just painted, in paint order. `Split` carries no `WidgetId` and is
    /// not a focusable leaf, so it is skipped; `Extra` is a tab stop only
    /// when its descriptor opts in via [`ExtraPaint::tab_stop`] (currently
    /// just [`Switch`]); every other variant yields its id and bounding
    /// `Rect`.
    fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        self.hit_map
            .borrow()
            .iter()
            .filter_map(|hit| match hit {
                PaintedHit::Button { id, layout } => Some((*id, layout.bounds)),
                PaintedHit::Tree { id, area, .. }
                | PaintedHit::List { id, area, .. }
                | PaintedHit::DataTable { id, area, .. }
                | PaintedHit::TabBar { id, area, .. } => Some((*id, *area)),
                PaintedHit::Split { .. } => None,
                PaintedHit::Extra { id, area, extra } => extra.tab_stop().then_some((*id, *area)),
            })
            .collect()
    }

    fn handle(&mut self, mut event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let Some(handler) = self.handler.as_ref() else {
            return Reaction::Continue;
        };
        let render_callback_present = self.render_callback.is_some();

        // A move while a divider drag is armed becomes the drag itself —
        // checked before the hit-test below, since `MouseMoved` has no
        // `widget` field for `event_position`/`set_event_widget` to touch.
        if let UiEvent::MouseMoved { position, .. } = &event {
            if let Some(drag) = self.active_drag.borrow().as_ref() {
                let new_ratio = drag.ratio_at(*position);
                event = UiEvent::SplitDividerDragged {
                    tree: drag.tree,
                    split_index: 0,
                    new_ratio,
                };
            }
        }
        if matches!(event, UiEvent::MouseUp { .. }) {
            *self.active_drag.borrow_mut() = None;
        }

        // A plain `KeyPressed` (Tab/Shift+Tab already claimed upstream by
        // `preprocess_event` once `tab_stops` is non-empty) is routed to
        // whichever leaf currently holds focus, wrapped into that leaf's
        // own `KeyPressed` variant so Python sees a focus-scoped event
        // instead of a backend-global one.
        if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
            if let Some(focused) = backend.focus_manager().focused().copied() {
                event = focus_scoped_key_event(focused, key, *modifiers, &self.hit_map.borrow());
            }
        }

        let hit = event_position(&event)
            .and_then(|position| hit_test_painted(&self.hit_map.borrow(), position));
        if let Some((id, _)) = hit {
            set_event_widget(&mut event, id);
        }

        // A `MouseDown` (or a bare `DoubleClick` pushed without the
        // `MouseDown`/`MouseUp` pair that precedes one on a real backend —
        // a host synthesizing clicks, e.g. a test, does this) landing on a
        // focusable painted leaf moves keyboard focus there first — matching
        // every native backend's click-to-focus convention — and, if that
        // actually changed the focused widget, dispatches a `FocusChanged`
        // event to Python ahead of the click event itself. Its `Reaction` is
        // combined with the main dispatch's below (via `Reaction::merge`)
        // rather than returned on its own, so a non-`Continue` result from
        // the `FocusChanged` handler (e.g. a `Redraw`) never suppresses the
        // click event that follows it.
        let mut focus_reaction = Reaction::Continue;
        if let Some((id, _)) = &hit {
            if matches!(
                event,
                UiEvent::MouseDown { .. } | UiEvent::DoubleClick { .. }
            ) {
                let is_focusable = AppLogic::tab_stops(self, ())
                    .iter()
                    .any(|(tid, _)| tid == id);
                if is_focusable && backend.set_focus(Some(*id)) {
                    focus_reaction = self.dispatch_event(
                        handler,
                        &UiEvent::FocusChanged(Some(*id)),
                        None,
                        render_callback_present,
                    );
                    if matches!(focus_reaction, Reaction::Exit) {
                        return focus_reaction;
                    }
                }
            }
        }

        // A `MouseDown`/`DoubleClick` either arms a divider drag or, for a
        // tab/tree-row/tree-chevron/list-item/data-table-row/data-table-header
        // hit, is replaced outright by the semantic event it stands for — in
        // which case the raw `MouseDown`/`DoubleClick` (and its `"hit"` dict)
        // never reaches Python at all.
        let mut attach_hit = hit.is_some();
        if let Some((id, detail)) = &hit {
            let modifiers_and_double_click = match &event {
                UiEvent::MouseDown { modifiers, .. } => Some((*modifiers, false)),
                UiEvent::DoubleClick { .. } => Some((Modifiers::default(), true)),
                _ => None,
            };
            if let Some((modifiers, double_click)) = modifiers_and_double_click {
                if !double_click && matches!(detail, HitDetail::SplitDivider) {
                    if let UiEvent::MouseDown { position, .. } = &event {
                        if let Some(layout) =
                            find_split_layout(&self.hit_map.borrow(), *position, *id)
                        {
                            *self.active_drag.borrow_mut() = Some(ActiveDrag { tree: *id, layout });
                        }
                    }
                } else if let Some(semantic) = semantic_event_for_hit(
                    *id,
                    detail,
                    modifiers,
                    double_click,
                    &self.hit_map.borrow(),
                ) {
                    event = semantic;
                    attach_hit = false;
                }
            }
        }

        let hit_detail = if attach_hit {
            hit.as_ref().map(|(_, detail)| detail)
        } else {
            None
        };
        let event_reaction =
            self.dispatch_event(handler, &event, hit_detail, render_callback_present);
        focus_reaction.merge(event_reaction)
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

impl PyAppLogic {
    /// Pythonizes `event` (attaching `hit_detail` under the `"hit"` key when
    /// present), calls the registered Python handler with it, and translates
    /// the handler's return value into a [`Reaction`]. Shared by the main
    /// event dispatch and the synthetic `FocusChanged` dispatch click-to-focus
    /// sends ahead of it, so both go through identical error handling.
    fn dispatch_event(
        &self,
        handler: &Py<PyAny>,
        event: &UiEvent,
        hit_detail: Option<&HitDetail>,
        render_callback_present: bool,
    ) -> Reaction {
        Python::attach(|py| {
            let payload = match pythonize::pythonize(py, event) {
                Ok(value) => value,
                Err(err) => {
                    record_error(
                        &self.pending_error,
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
                                record_error(&self.pending_error, err);
                                return Reaction::Exit;
                            }
                        }
                        Err(err) => {
                            record_error(
                                &self.pending_error,
                                InternalError::new_err(format!(
                                    "failed to pythonize hit detail: {err}"
                                )),
                            );
                            return Reaction::Exit;
                        }
                    },
                    Err(_) => {
                        record_error(
                            &self.pending_error,
                            InternalError::new_err(
                                "failed to attach hit detail: pythonized event payload was not a dict"
                                    .to_string(),
                            ),
                        );
                        return Reaction::Exit;
                    }
                }
            }
            match handler.call1(py, (payload,)) {
                Ok(result) => reaction_for_callback(result.bind(py), render_callback_present),
                Err(err) => {
                    record_error(&self.pending_error, err);
                    Reaction::Exit
                }
            }
        })
    }
}

/// One primitive this app painted during the current frame: its id and the
/// hit-testable layout its backend `draw_*` call returned (or, for
/// `Backend::draw_tree`/`draw_list`, which paint without returning one, the
/// layout its no-paint twin `tree_layout`/`list_layout` produced for the
/// same `area`). Recorded into [`PyAppLogic::hit_map`] by `draw_bands` /
/// `paint_layout` / `paint_leaf`, in paint order, and consulted by
/// [`hit_test_painted`] to translate an incoming pointer event's screen
/// position into the primitive (and sub-region) under it.
enum PaintedHit {
    /// `Button`'s layout is in the backend's ABSOLUTE coordinate frame, so
    /// no `area` is stored alongside it — same as [`Self::Split`].
    Button { id: WidgetId, layout: ButtonLayout },
    /// `area` is the absolute rect this tree was painted into. `Tree`'s
    /// layout (like `List`/`DataTable`/`TabBar`'s) is in the LOCAL
    /// coordinate frame, relative to `area`'s origin, so a hit-test
    /// against it must first translate the click into that frame.
    /// `row_paths[i]` is the `TreePath` of the row at visual index `i`
    /// (i.e. `TreeView::rows[i].path`), kept alongside the layout so a
    /// `TreeViewHit::Row(i)` can be turned into a semantic
    /// `UiEvent::Tree(RowClicked)` without re-borrowing the `TreeView`
    /// itself.
    Tree {
        id: WidgetId,
        area: Rect,
        layout: TreeViewLayout,
        row_paths: Vec<TreePath>,
    },
    List {
        id: WidgetId,
        area: Rect,
        layout: ListViewLayout,
    },
    DataTable {
        id: WidgetId,
        area: Rect,
        layout: DataTableLayout,
        scroll_offset: usize,
        total_rows: usize,
    },
    TabBar {
        id: WidgetId,
        area: Rect,
        layout: TabBarLayout,
    },
    /// `Split`'s layout is already in the ABSOLUTE frame, and its own
    /// `WidgetId` only matters once a hit is found — carried by
    /// `SplitHit`'s own variants, not stored here.
    Split { layout: SplitLayout },
    /// One additive primitive (anything registered in [`EXTRA_PRIMITIVES`]),
    /// painted by `draw_bands`'s extras loop or by a [`LeafDescriptor::Extra`]
    /// leaf. `area` is the ABSOLUTE rect it was painted into — the same frame
    /// [`ExtraPaint::hit_test`] expects, so no translation is needed before
    /// calling it (matching `Button`/`Switch`'s own layout convention: no
    /// local-frame remap). `extra` is an [`Rc`] clone of the slot's/leaf's
    /// descriptor, kept alive past this frame's render so a later `handle`
    /// call can still hit-test and tab-stop against it.
    Extra {
        id: WidgetId,
        area: Rect,
        extra: Rc<dyn ExtraPaint>,
    },
}

/// The sub-region of a primitive an incoming pointer event landed on,
/// attached to the enriched event's `"hit"` key (alongside `widget`, the
/// primitive's own id) so a Python handler can react to "row 3" or "the
/// divider" without re-deriving hit geometry itself.
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HitDetail {
    ButtonBody,
    TreeRow {
        row: usize,
    },
    TreeChevron {
        row: usize,
    },
    ListTitle,
    ListItem {
        index: usize,
    },
    DataTableHeader {
        col: usize,
    },
    DataTableHeaderDivider {
        col: usize,
    },
    DataTableRow {
        row: usize,
    },
    DataTableFooter,
    TabBarTab {
        tab: usize,
    },
    TabBarTabClose {
        tab: usize,
    },
    TabBarScrollLeft,
    TabBarScrollRight,
    TabBarRightSegment,
    SplitFirstPane,
    SplitSecondPane,
    SplitDivider,
    /// Landed on an additive primitive's interactive region (currently only
    /// [`Switch`]). Carries no sub-region detail yet — just enough for
    /// [`PyAppLogic::handle`] to set `widget` and attach this as `hit`, so a
    /// Python handler sees the click reached the primitive. Per-kind
    /// sub-region detail (e.g. which combo-box row) is a later, separate
    /// extension of this variant.
    ExtraBody,
}

/// Hit-tests `point` (in the backend's native absolute coordinate space,
/// same as `UiEvent::MouseDown`'s `position`) against every primitive
/// painted this frame, returning the id and fine-grained detail of
/// whichever one claims it.
///
/// Checked in reverse paint order — later-painted primitives sit on top of
/// earlier ones — and a miss against one candidate (e.g. outside its own
/// paint `area`, or `TreeViewHit::Empty` inside it) falls through to the
/// next rather than stopping, so a click that misses the topmost primitive
/// can still land on one painted earlier.
fn hit_test_painted(hit_map: &[PaintedHit], point: Point) -> Option<(WidgetId, HitDetail)> {
    for hit in hit_map.iter().rev() {
        match hit {
            PaintedHit::Button { id, layout } => {
                if matches!(layout.hit_test(point), ButtonHit::Body) {
                    return Some((*id, HitDetail::ButtonBody));
                }
            }
            PaintedHit::Tree {
                id, area, layout, ..
            } => {
                if !area.contains(point) {
                    continue;
                }
                match layout.hit_test(point.x - area.x, point.y - area.y) {
                    TreeViewHit::Row(row) => return Some((*id, HitDetail::TreeRow { row })),
                    TreeViewHit::Chevron(row) => {
                        return Some((*id, HitDetail::TreeChevron { row }))
                    }
                    TreeViewHit::Empty => {}
                }
            }
            PaintedHit::List { id, area, layout } => {
                if !area.contains(point) {
                    continue;
                }
                match layout.hit_test(point.x - area.x, point.y - area.y) {
                    ListViewHit::Title => return Some((*id, HitDetail::ListTitle)),
                    ListViewHit::Item(index) => return Some((*id, HitDetail::ListItem { index })),
                    ListViewHit::Empty => {}
                }
            }
            PaintedHit::DataTable {
                id,
                area,
                layout,
                scroll_offset,
                total_rows,
            } => {
                if !area.contains(point) {
                    continue;
                }
                match layout.hit_test(
                    point.x - area.x,
                    point.y - area.y,
                    *scroll_offset,
                    *total_rows,
                ) {
                    DataTableHit::Header { col } => {
                        return Some((*id, HitDetail::DataTableHeader { col }))
                    }
                    DataTableHit::HeaderDivider { col } => {
                        return Some((*id, HitDetail::DataTableHeaderDivider { col }))
                    }
                    DataTableHit::Row { idx } => {
                        return Some((*id, HitDetail::DataTableRow { row: idx }))
                    }
                    DataTableHit::Footer => return Some((*id, HitDetail::DataTableFooter)),
                    DataTableHit::Empty => {}
                }
            }
            PaintedHit::TabBar { id, area, layout } => {
                if !area.contains(point) {
                    continue;
                }
                match layout.hit_test(point.x - area.x, point.y - area.y) {
                    TabBarHit::Tab(tab) => return Some((*id, HitDetail::TabBarTab { tab })),
                    TabBarHit::TabClose(tab) => {
                        return Some((*id, HitDetail::TabBarTabClose { tab }))
                    }
                    TabBarHit::ScrollLeft => return Some((*id, HitDetail::TabBarScrollLeft)),
                    TabBarHit::ScrollRight => return Some((*id, HitDetail::TabBarScrollRight)),
                    TabBarHit::RightSegment(right_id) => {
                        return Some((right_id, HitDetail::TabBarRightSegment))
                    }
                    TabBarHit::Empty => {}
                }
            }
            PaintedHit::Split { layout } => match layout.hit_test(point.x, point.y) {
                SplitHit::FirstPane(id) => return Some((id, HitDetail::SplitFirstPane)),
                SplitHit::SecondPane(id) => return Some((id, HitDetail::SplitSecondPane)),
                SplitHit::Divider(id) => return Some((id, HitDetail::SplitDivider)),
                SplitHit::Outside => {}
            },
            PaintedHit::Extra { id, area, extra } => {
                if extra.hit_test(*area, point) {
                    return Some((*id, HitDetail::ExtraBody));
                }
            }
        }
    }
    None
}

/// The screen position to hit-test against [`PaintedHit`]s, for the one
/// `event` variants whose own `widget` field this module enriches. `None`
/// for every other variant (e.g. `MouseMoved`, `MouseEntered`, key events),
/// which [`PyAppLogic::handle`] forwards to Python exactly as received.
fn event_position(event: &UiEvent) -> Option<Point> {
    match event {
        UiEvent::MouseDown { position, .. }
        | UiEvent::MouseUp { position, .. }
        | UiEvent::DoubleClick { position, .. }
        | UiEvent::Scroll { position, .. } => Some(*position),
        _ => None,
    }
}

/// Sets `event`'s own `widget` field to `id`, for whichever of the four
/// variants [`event_position`] matched. A no-op for every other variant.
fn set_event_widget(event: &mut UiEvent, id: WidgetId) {
    match event {
        UiEvent::MouseDown { widget, .. }
        | UiEvent::MouseUp { widget, .. }
        | UiEvent::DoubleClick { widget, .. }
        | UiEvent::Scroll { widget, .. } => *widget = Some(id),
        _ => {}
    }
}

/// A divider drag in progress: armed by [`PyAppLogic::handle`] on a
/// `MouseDown` landing on `HitDetail::SplitDivider`, and cleared on the
/// matching `MouseUp`. `layout` is the `Split`'s own layout as painted the
/// frame the drag started, snapshotted rather than re-read each move so a
/// drag stays anchored to the geometry the user actually grabbed even if a
/// handler-driven re-render shifts the divider under the cursor mid-drag.
struct ActiveDrag {
    tree: WidgetId,
    layout: SplitLayout,
}

impl ActiveDrag {
    /// The ratio `point` (in the same absolute frame as `layout`) implies
    /// along this split's axis, clamped to `[0.0, 1.0]`. Falls back to the
    /// layout's own already-resolved ratio when the split has no room to
    /// drag in (e.g. a pane collapsed to zero by its own `first_min`/
    /// `second_min`), rather than dividing by zero.
    fn ratio_at(&self, point: Point) -> f32 {
        let divider = self.layout.divider_bounds;
        let bounds = self.layout.bounds;
        let vertical_divider = divider.width < divider.height;
        let (total, offset, divider_thickness) = if vertical_divider {
            (bounds.width, point.x - bounds.x, divider.width)
        } else {
            (bounds.height, point.y - bounds.y, divider.height)
        };
        let available = (total - divider_thickness).max(0.0);
        if available <= 0.0 {
            return self.layout.resolved_ratio;
        }
        (offset / available).clamp(0.0, 1.0)
    }
}

/// Re-finds the [`SplitLayout`] whose divider `id` claims at `point`, so a
/// `MouseDown` landing on `HitDetail::SplitDivider` can snapshot it into an
/// [`ActiveDrag`]. Re-runs the hit-test (rather than trusting
/// [`hit_test_painted`]'s already-discarded match) because `PaintedHit::Split`
/// never stores its own id — only `SplitHit`'s variants carry one — so the id
/// has to be recovered the same way `hit_test_painted` found it the first time.
fn find_split_layout(hit_map: &[PaintedHit], point: Point, id: WidgetId) -> Option<SplitLayout> {
    hit_map.iter().rev().find_map(|hit| match hit {
        PaintedHit::Split { layout } => {
            matches!(layout.hit_test(point.x, point.y), SplitHit::Divider(hit_id) if hit_id == id)
                .then(|| layout.clone())
        }
        _ => None,
    })
}

/// The [`TreePath`] of the row at visual index `row` within the `Tree` hit
/// painted under id `id` this frame, or `None` if that tree isn't in the
/// current hit map (e.g. the frame that produced this hit test is stale).
fn tree_row_path(hit_map: &[PaintedHit], id: WidgetId, row: usize) -> Option<TreePath> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Tree {
            id: hit_id,
            row_paths,
            ..
        } if *hit_id == id => row_paths.get(row).cloned(),
        _ => None,
    })
}

/// The tab bar's own id for a `TabBarRightSegment` hit, given the `id` of
/// the clicked button — `hit_test_painted` returns the button's id (the one
/// [`TabBarHit::RightSegment`] carries), not the enclosing bar's, so this
/// walks the hit map's painted tab bars to find the one whose
/// `hit_regions` contains that button id.
fn tab_bar_for_right_segment(hit_map: &[PaintedHit], button_id: WidgetId) -> Option<WidgetId> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::TabBar { id, layout, .. }
            if layout.hit_regions.iter().any(
                |(_, hit)| matches!(hit, TabBarHit::RightSegment(rid) if *rid == button_id),
            ) =>
        {
            Some(*id)
        }
        _ => None,
    })
}

/// The semantic [`UiEvent`] a click on `detail` (within primitive `id`)
/// stands for, if any — the set [`PyAppLogic::handle`] synthesizes so a tab
/// click, tab close-button click, tree-row/chevron click, list-item click,
/// data-table-row/header click, or tab-bar button click reaches a Python
/// handler as the same shape it would get by pushing that event directly
/// (`UiEvent::TabBar`/`UiEvent::Tree`/`UiEvent::List`/`UiEvent::DataTable`),
/// rather than only as a raw `MouseDown`/`DoubleClick` plus a `"hit"` dict
/// the handler would have to re-interpret itself. `double_click` selects
/// between a variant's single- and double-click shapes where both exist
/// (e.g. `TreeEvent::RowClicked` vs `RowDoubleClicked`,
/// `ListViewEvent::SelectionChanged` vs `ItemActivated`). `None` for every
/// other `HitDetail` — a data-table header divider or a split divider
/// (handled separately, via [`ActiveDrag`], since either spans a drag rather
/// than a single click), a tab-bar scroll-left/right arrow (no semantic
/// event exists for it), [`HitDetail::ExtraBody`] (an extra's own
/// `ExtraPaint` impl owns its click behavior), or a modified list-item click
/// (see the `ListItem` arm below, which loses the gesture if folded into
/// `ListViewEvent::SelectionChanged`) — for which the raw
/// `MouseDown`/`DoubleClick` and its `"hit"` dict reach Python unchanged.
fn semantic_event_for_hit(
    id: WidgetId,
    detail: &HitDetail,
    modifiers: Modifiers,
    double_click: bool,
    hit_map: &[PaintedHit],
) -> Option<UiEvent> {
    match detail {
        HitDetail::TabBarTab { tab } => Some(UiEvent::TabBar(
            id,
            TabBarEvent::TabActivated { index: *tab },
        )),
        HitDetail::TabBarTabClose { tab } => {
            Some(UiEvent::TabBar(id, TabBarEvent::TabClosed { index: *tab }))
        }
        HitDetail::TabBarRightSegment => {
            let tab_bar_id = tab_bar_for_right_segment(hit_map, id)?;
            Some(UiEvent::TabBar(
                tab_bar_id,
                TabBarEvent::ButtonClicked { id },
            ))
        }
        HitDetail::TreeRow { row } => {
            let path = tree_row_path(hit_map, id, *row)?;
            if double_click {
                Some(UiEvent::Tree(id, TreeEvent::RowDoubleClicked { path }))
            } else {
                Some(UiEvent::Tree(id, TreeEvent::RowClicked { path, modifiers }))
            }
        }
        HitDetail::TreeChevron { row } => {
            let path = tree_row_path(hit_map, id, *row)?;
            Some(UiEvent::Tree(id, TreeEvent::RowToggleExpand { path }))
        }
        HitDetail::ListItem { index } => {
            if double_click {
                Some(UiEvent::List(
                    id,
                    ListViewEvent::ItemActivated { idx: *index },
                ))
            } else if modifiers == Modifiers::default() {
                Some(UiEvent::List(
                    id,
                    ListViewEvent::SelectionChanged { idx: *index },
                ))
            } else {
                // `ListViewEvent::SelectionChanged` carries no modifiers, so a
                // Ctrl/Shift/Cmd click (a multi-select gesture) can't be
                // represented by it without losing which gesture it was.
                // Leaving the raw `MouseDown` (with its `"hit"` dict still
                // attached, since this returns `None`) lets a Python handler
                // read `event["modifiers"]` and `event["hit"]["index"]` and
                // compute the selection itself, the same way it would for any
                // other modified click.
                None
            }
        }
        HitDetail::DataTableHeader { col } => Some(UiEvent::DataTable(
            id,
            DataTableEvent::HeaderClicked { col: *col },
        )),
        HitDetail::DataTableRow { row } => Some(UiEvent::DataTable(
            id,
            DataTableEvent::RowActivated { idx: *row },
        )),
        _ => None,
    }
}

/// Stringifies a raw key the same way every quadraui backend does, so a
/// `*Event::KeyPressed` variant built here carries the identical `key`
/// string an app would see from the TUI, GTK or macOS backend directly.
fn key_to_focus_string(key: &Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Named(named) => {
            let s = match named {
                NamedKey::Escape => "Escape".to_string(),
                NamedKey::Tab => "Tab".to_string(),
                NamedKey::BackTab => "BackTab".to_string(),
                NamedKey::Enter => "Enter".to_string(),
                NamedKey::Backspace => "Backspace".to_string(),
                NamedKey::Delete => "Delete".to_string(),
                NamedKey::Insert => "Insert".to_string(),
                NamedKey::Home => "Home".to_string(),
                NamedKey::End => "End".to_string(),
                NamedKey::PageUp => "PageUp".to_string(),
                NamedKey::PageDown => "PageDown".to_string(),
                NamedKey::Up => "Up".to_string(),
                NamedKey::Down => "Down".to_string(),
                NamedKey::Left => "Left".to_string(),
                NamedKey::Right => "Right".to_string(),
                NamedKey::F(n) => format!("F{n}"),
                NamedKey::CapsLock => "CapsLock".to_string(),
                NamedKey::NumLock => "NumLock".to_string(),
                NamedKey::ScrollLock => "ScrollLock".to_string(),
                NamedKey::Menu => "Menu".to_string(),
            };
            s
        }
    }
}

/// Routes a raw `KeyPressed` to whichever leaf the `FocusManager` names as
/// focused, wrapping it into that leaf's own semantic `KeyPressed` variant.
/// `DataTable` and `Button` have no such variant, so a key focused on one
/// of them passes through as the original `UiEvent::KeyPressed` unchanged;
/// the same happens when `focused` names a leaf no longer present in this
/// frame's hit map (for example a tree that scrolled out of the render).
fn focus_scoped_key_event(
    focused: WidgetId,
    key: &Key,
    modifiers: Modifiers,
    hit_map: &[PaintedHit],
) -> UiEvent {
    let fallback = UiEvent::KeyPressed {
        key: key.clone(),
        modifiers,
        repeat: false,
    };
    let key = key_to_focus_string(key);
    hit_map
        .iter()
        .find_map(|hit| match hit {
            PaintedHit::Tree { id, .. } if *id == focused => Some(UiEvent::Tree(
                *id,
                TreeEvent::KeyPressed {
                    key: key.clone(),
                    modifiers,
                },
            )),
            PaintedHit::List { id, .. } if *id == focused => Some(UiEvent::List(
                *id,
                ListViewEvent::KeyPressed {
                    key: key.clone(),
                    modifiers,
                },
            )),
            PaintedHit::TabBar { id, .. } if *id == focused => Some(UiEvent::TabBar(
                *id,
                TabBarEvent::KeyPressed {
                    key: key.clone(),
                    modifiers,
                },
            )),
            _ => None,
        })
        .unwrap_or(fallback)
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
    /// Additive primitive slots, keyed by kind name. Painted after the six
    /// fixed bands above, in [`EXTRA_PRIMITIVES`] order.
    extra: &'a HashMap<String, Rc<dyn ExtraPaint>>,
}

/// Paints each present slot top-to-bottom in a fixed-height band, skipping
/// any slot once the viewport runs out of room. Appends a [`PaintedHit`] to
/// `hits` for every slot painted, so a later pointer event can be routed to
/// it.
fn draw_bands(backend: &mut dyn Backend, refs: RenderRefs<'_>, hits: &mut Vec<PaintedHit>) {
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
            let layout = backend.draw_button(rect, button);
            hits.push(PaintedHit::Button {
                id: button.id,
                layout,
            });
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
            let layout = backend.list_layout(rect, list);
            hits.push(PaintedHit::List {
                id: list.id,
                area: rect,
                layout,
            });
        }
    }
    if let Some(tree) = refs.tree {
        if let Some(rect) = next_band(4.0) {
            draw_tree_with_scrollbar(backend, rect, tree);
            let layout = backend.tree_layout(rect, tree);
            hits.push(PaintedHit::Tree {
                id: tree.id,
                area: rect,
                layout,
                row_paths: tree.rows.iter().map(|row| row.path.clone()).collect(),
            });
        }
    }
    if let Some(data_table) = refs.data_table {
        if let Some(rect) = next_band(4.0) {
            let layout = backend.draw_data_table(rect, data_table, None);
            hits.push(PaintedHit::DataTable {
                id: data_table.id,
                area: rect,
                layout,
                scroll_offset: data_table.scroll_offset,
                total_rows: data_table.rows.len(),
            });
        }
    }
    if let Some(tab_bar) = refs.tab_bar {
        if let Some(rect) = next_band(3.0) {
            let layout = backend.draw_tab_bar_layout(rect, tab_bar, None);
            hits.push(PaintedHit::TabBar {
                id: tab_bar.id,
                area: rect,
                layout,
            });
        }
    }
    // Additive primitives, painted in canonical `EXTRA_PRIMITIVES` order so
    // the output is deterministic regardless of `HashMap` iteration order.
    for (kind, _) in EXTRA_PRIMITIVES {
        if let Some(extra) = refs.extra.get(*kind) {
            if let Some(rect) = next_band(3.0) {
                extra.paint(backend, rect);
                hits.push(PaintedHit::Extra {
                    id: extra.id(),
                    area: rect,
                    extra: Rc::clone(extra),
                });
            }
        }
    }
}

/// Paints a layout tree into `area`. A `Row`/`Column` divides its area into
/// equal shares — one per child, side by side or stacked — and recurses;
/// equal shares (rather than `draw_bands`'s fixed line-count bands) fill
/// the whole area regardless of viewport size. A `Leaf` paints its one
/// descriptor into the whole area its container gave it. A `Split` divides
/// its area into two unequal panes at `ratio` via `Backend::draw_split`
/// (the same divider paint/hit-test every backend already shares for the
/// Rust-side `Split` primitive) and recurses into each with its resolved
/// bounds. A `Tabs` reserves a top band for its tab strip (painted via
/// `Backend::draw_tab_bar_layout`, the same strip every backend already
/// shares for the Rust-side `TabBar` primitive) and recurses into the
/// remaining area with the selected tab's content, so a tab group is just a
/// `Split` whose panes are each a `Tabs`. Per-child flex weights on
/// `Row`/`Column` are a follow-up; every `Row`/`Column` child gets the same
/// share for now.
fn paint_layout(
    backend: &mut dyn Backend,
    area: Rect,
    node: &LayoutNode,
    hits: &mut Vec<PaintedHit>,
) {
    match node {
        LayoutNode::Row(children) => {
            for (child, rect) in children.iter().zip(split_rects(area, children.len(), true)) {
                paint_layout(backend, rect, child, hits);
            }
        }
        LayoutNode::Column(children) => {
            for (child, rect) in children
                .iter()
                .zip(split_rects(area, children.len(), false))
            {
                paint_layout(backend, rect, child, hits);
            }
        }
        LayoutNode::Leaf(leaf) => paint_leaf(backend, area, leaf, hits),
        LayoutNode::Split(split) => {
            // `split.split.first_min`/`second_min` are decoded as portable
            // line counts (see `layout_split_from_dict`); scale them into
            // the backend's native unit here, right before painting, so
            // every backend gets a minimum pane size of the same visual
            // weight from the same Python-supplied number.
            let line_height = backend.line_height();
            let native_split = Split {
                first_min: split.split.first_min * line_height,
                second_min: split.split.second_min * line_height,
                ..split.split.clone()
            };
            let layout = backend.draw_split(area, &native_split);
            // Pushed before recursing so the divider/pane hit only wins
            // over a descendant primitive's own, more specific hit when
            // nothing painted inside either pane claims the point first —
            // `hit_test_painted` checks in reverse paint order.
            hits.push(PaintedHit::Split {
                layout: layout.clone(),
            });
            paint_layout(backend, layout.first_bounds, &split.first, hits);
            paint_layout(backend, layout.second_bounds, &split.second, hits);
        }
        LayoutNode::Tabs(tabs) => {
            let strip_height = (3.0 * backend.line_height()).min(area.height);
            let strip_rect = Rect::new(area.x, area.y, area.width, strip_height);
            let layout = backend.draw_tab_bar_layout(strip_rect, &tabs.tab_bar, None);
            hits.push(PaintedHit::TabBar {
                id: tabs.tab_bar.id,
                area: strip_rect,
                layout,
            });
            let content_rect = Rect::new(
                area.x,
                area.y + strip_height,
                area.width,
                (area.height - strip_height).max(0.0),
            );
            if let Some(selected) = tabs.entries.get(tabs.selected) {
                paint_layout(backend, content_rect, &selected.content, hits);
            }
        }
        LayoutNode::Panel(panel_node) => {
            let layout = backend.draw_panel(area, &panel_node.panel);
            paint_layout(backend, layout.content_bounds, &panel_node.content, hits);
        }
        LayoutNode::SidebarPanel(sidebar_node) => {
            let layout = backend.draw_sidebar_panel_interactive(
                area,
                &sidebar_node.sidebar_panel,
                &InteractionState::default(),
            );
            paint_layout(backend, layout.content_bounds, &sidebar_node.content, hits);
        }
        LayoutNode::SplitTree(split_tree) => {
            let layout = backend.draw_split_tree(area, &split_tree.tree);
            for ((_, rect), content) in layout.leaves.iter().zip(split_tree.leaves.iter()) {
                paint_layout(backend, *rect, content, hits);
            }
        }
    }
}

/// Divides `area` into `n` equal shares, side by side when `horizontal` or
/// stacked top-to-bottom otherwise. Returns an empty `Vec` for `n == 0`.
fn split_rects(area: Rect, n: usize, horizontal: bool) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let n_f = n as f32;
    (0..n)
        .map(|i| {
            let i_f = i as f32;
            if horizontal {
                let share = area.width / n_f;
                Rect::new(area.x + share * i_f, area.y, share, area.height)
            } else {
                let share = area.height / n_f;
                Rect::new(area.x, area.y + share * i_f, area.width, share)
            }
        })
        .collect()
}

/// Paints `tree` into `area`, then -- on a backend that doesn't paint its
/// own tree scrollbar inside `draw_tree` (the TUI backend; the GUI backends
/// paint theirs inline, reporting [`Backend::tree_paints_own_scrollbar`]) --
/// composes the vertical scrollbar from the backend's own
/// [`Backend::tree_vscrollbar`] geometry and paints it over the pane's right
/// edge. This is the layout-then-paint two-step a Rust consumer runs by
/// hand, so a declarative tree gets a working scrollbar on every backend.
/// `tree_vscrollbar` returns `None` when the rows fit, so a short tree paints
/// no scrollbar.
fn draw_tree_with_scrollbar(backend: &mut dyn Backend, area: Rect, tree: &TreeView) {
    backend.draw_tree(area, tree);
    if !backend.tree_paints_own_scrollbar() {
        if let Some(scrollbar) = backend.tree_vscrollbar(area, tree) {
            backend.draw_scrollbar(area, &scrollbar);
        }
    }
}

/// Paints one leaf's descriptor into the whole `area` its container gave
/// it. Appends a [`PaintedHit`] for it, keyed by the leaf's own tree-address
/// `id` (not the descriptor's embedded `id`, which a render callback may
/// leave at its default and reuse across leaves) so a later pointer event
/// can be routed back to this exact slot in the layout tree.
fn paint_leaf(
    backend: &mut dyn Backend,
    area: Rect,
    leaf: &LayoutLeaf,
    hits: &mut Vec<PaintedHit>,
) {
    let id = WidgetId::new(leaf.id.clone());
    match &leaf.descriptor {
        LeafDescriptor::Button(button) => {
            let layout = backend.draw_button(area, button);
            hits.push(PaintedHit::Button { id, layout });
        }
        LeafDescriptor::TextDisplay(text_display) => {
            backend.draw_text_display(area, text_display);
        }
        LeafDescriptor::List(list) => {
            backend.draw_list(area, list);
            let layout = backend.list_layout(area, list);
            hits.push(PaintedHit::List { id, area, layout });
        }
        LeafDescriptor::Tree(tree) => {
            draw_tree_with_scrollbar(backend, area, tree);
            let layout = backend.tree_layout(area, tree);
            hits.push(PaintedHit::Tree {
                id,
                area,
                layout,
                row_paths: tree.rows.iter().map(|row| row.path.clone()).collect(),
            });
        }
        LeafDescriptor::DataTable(data_table) => {
            let layout = backend.draw_data_table(area, data_table, None);
            hits.push(PaintedHit::DataTable {
                id,
                area,
                layout,
                scroll_offset: data_table.scroll_offset,
                total_rows: data_table.rows.len(),
            });
        }
        LeafDescriptor::TabBar(tab_bar) => {
            let layout = backend.draw_tab_bar_layout(area, tab_bar, None);
            hits.push(PaintedHit::TabBar { id, area, layout });
        }
        LeafDescriptor::Extra(extra) => {
            extra.paint(backend, area);
            hits.push(PaintedHit::Extra {
                id,
                area,
                extra: Rc::clone(extra),
            });
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
    /// Additive primitive slots present in this frame, keyed by kind name
    /// (one registered in [`EXTRA_PRIMITIVES`]).
    extra: HashMap<String, Rc<dyn ExtraPaint>>,
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
            extra: &self.extra,
        }
    }

    /// Decodes a render callback's returned `dict`, raising
    /// `InvalidDescriptorError` if it isn't a `dict`, if an entry isn't a
    /// `dict`/`None`, or if an entry's shape doesn't match its slot's
    /// descriptor type. The exact recognized key set is `"button"`,
    /// `"text_display"`, `"list"`, `"tree"`, `"data_table"`, `"tab_bar"` —
    /// one per fixed slot, same names `insert_*`'s Rust-side `what` argument
    /// uses — plus any kind registered in [`EXTRA_PRIMITIVES`] for an
    /// additive primitive. Any other key (including a near-miss typo like
    /// `"tabbar"`) is ignored rather than rejected, so a render callback
    /// built against a future slot set stays forward-compatible with this
    /// one.
    fn from_dict(dict: &Bound<'_, PyDict>) -> PyResult<Self> {
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

/// One node of a render callback's layout tree: a `Row`/`Column` container
/// holding child nodes, a `Leaf` wrapping one id-addressable primitive
/// descriptor, a `Split` holding exactly two named child nodes divided by a
/// draggable ratio, a `Tabs` holding a tab strip plus one content subtree
/// per tab (of which only the selected one paints), a `Panel` or
/// `SidebarPanel` holding one chrome descriptor plus a single content
/// subtree, or a `SplitTree` holding an arbitrarily nested divider tree of
/// content subtrees.
enum LayoutNode {
    Row(Vec<LayoutNode>),
    Column(Vec<LayoutNode>),
    Leaf(LayoutLeaf),
    Split(LayoutSplit),
    Tabs(LayoutTabs),
    Panel(LayoutPanel),
    SidebarPanel(LayoutSidebarPanel),
    SplitTree(LayoutSplitTree),
}

/// A [`LayoutNode::Split`]'s decoded shape: the `quadraui::Split` descriptor
/// (its own `id`, `direction`, `ratio`, pane minimums) paired with the two
/// named child nodes it divides. `first`/`second` are full [`LayoutNode`]s,
/// not leaf-only slots, so a pane may itself hold a nested row/column/split.
struct LayoutSplit {
    split: Split,
    first: Box<LayoutNode>,
    second: Box<LayoutNode>,
}

/// A [`LayoutNode::Tabs`]'s decoded shape: a `quadraui::TabBar` descriptor
/// built from each tab's title (with `selected`'s item marked active) for
/// painting the strip, paired with `entries` holding the full content
/// subtree behind each tab. A tab group is a `Split` whose `first`/`second`
/// are each a `Tabs` — no separate "group" concept.
struct LayoutTabs {
    tab_bar: TabBar,
    entries: Vec<LayoutTabEntry>,
    selected: usize,
}

/// One tab inside a [`LayoutTabs`]: the id that addresses it, and the full
/// [`LayoutNode`] subtree painted beneath the strip while it is selected.
struct LayoutTabEntry {
    id: String,
    content: Box<LayoutNode>,
}

/// A [`LayoutNode::Panel`]'s decoded shape: the `quadraui::Panel` chrome
/// descriptor (title/actions/accent/collapsed) paired with the single
/// content subtree it frames. `content` is a full [`LayoutNode`], not a
/// leaf-only slot, so a panel may itself hold a nested row/column/split.
struct LayoutPanel {
    panel: Panel,
    content: Box<LayoutNode>,
}

/// A [`LayoutNode::SidebarPanel`]'s decoded shape: the `quadraui::SidebarPanel`
/// chrome descriptor (optional toolbar) paired with the single content
/// subtree painted beneath the toolbar slot.
struct LayoutSidebarPanel {
    sidebar_panel: SidebarPanel,
    content: Box<LayoutNode>,
}

/// A [`LayoutNode::SplitTree`]'s decoded shape: the `quadraui::SplitTree`
/// geometry tree (every leaf position holding a synthetic, internal-only
/// `WidgetId`) paired with `leaves`, the matching content subtree for each
/// leaf position in the same pre-order [`SplitTree::leaf_ids`] would
/// produce. [`paint_layout`]'s `LayoutNode::SplitTree` arm zips
/// `SplitTreeLayout::leaves`' resolved rects back against `leaves` by
/// position — the synthetic ids in `tree` are never user-facing and never
/// checked for uniqueness; `id` is the node's own user-supplied id instead.
struct LayoutSplitTree {
    id: String,
    tree: SplitTree,
    leaves: Vec<LayoutNode>,
}

/// One id-addressable primitive inside a layout tree, decoded from a
/// `{"id": ..., "<kind>": <descriptor>}` dict. `id` is unique across the
/// whole tree ([`validate_unique_leaf_ids`]), so a kind may appear more
/// than once (e.g. two `data_table`s side by side) with each instance
/// still addressable on its own.
struct LayoutLeaf {
    id: String,
    descriptor: LeafDescriptor,
}

/// The decoded descriptor carried by a [`LayoutLeaf`] — one variant per
/// primitive kind a layout tree leaf may wrap.
enum LeafDescriptor {
    Button(Button),
    TextDisplay(TextDisplay),
    List(ListView),
    Tree(TreeView),
    DataTable(DataTable),
    TabBar(TabBar),
    /// Any additive primitive kind registered in [`EXTRA_PRIMITIVES`].
    /// Unlike the six variants above, a new additive kind never adds a new
    /// `LeafDescriptor` variant — it routes through this one.
    Extra(Rc<dyn ExtraPaint>),
}

/// The descriptor keys a layout tree leaf dict may carry, one per
/// [`LeafDescriptor`] variant — the same names [`RenderFrame::from_dict`]
/// uses for its flat slots.
const LEAF_KIND_KEYS: [&str; 6] = [
    "button",
    "text_display",
    "list",
    "tree",
    "data_table",
    "tab_bar",
];

/// Paints one additive primitive descriptor painted outside the six fixed
/// slots above. Implemented once per primitive type (see the `impl`s just
/// below) so that registering a kind in [`EXTRA_PRIMITIVES`] never needs a
/// central match arm.
///
/// [`ExtraPaint::id`] is mechanical (`self.id`) on every impl and backs the
/// [`PaintedHit::Extra`] entry pushed for this descriptor. [`ExtraPaint::hit_test`]
/// and [`ExtraPaint::tab_stop`] default to "not interactive" so the 24
/// pure-display primitives keep today's behavior unchanged; [`Switch`]
/// overrides both to route pointer and keyboard focus through its existing
/// `layout`/`hit_test`. Per-kind semantic event synthesis (toggling the
/// switch's value, etc.) is a separate, later step — this trait only gets a
/// hit to Python with `widget` set.
trait ExtraPaint {
    fn paint(&self, backend: &mut dyn Backend, area: Rect);

    /// The widget id this descriptor paints under, used to populate
    /// [`PaintedHit::Extra`] without threading a leaf-tree address through.
    fn id(&self) -> WidgetId;

    /// Whether `point` lands on an interactive region of this descriptor,
    /// given the ABSOLUTE `area` it was painted into (the same frame
    /// `point` itself is in — no translation, matching `Button`/`Split`'s
    /// own layout convention). `false` by default: most additive
    /// primitives are pure display and a miss here lets
    /// [`hit_test_painted`] fall through to whatever painted behind them.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        let _ = (area, point);
        false
    }

    /// Whether this descriptor should be included in [`PyAppLogic::tab_stops`].
    /// `false` by default, matching [`ExtraPaint::hit_test`]'s "not
    /// interactive unless overridden" default.
    fn tab_stop(&self) -> bool {
        false
    }
}

impl ExtraPaint for ProgressBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_progress(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Spinner {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_spinner(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Image {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_image(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Form {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_form(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for TextInput {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_text_input(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for RadioGroup {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_radio_group(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Switch {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_switch(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    fn hit_test(&self, area: Rect, point: Point) -> bool {
        self.layout(area).hit_test(point) == SwitchHit::Body
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for NumberInput {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let layout = self.layout(area);
        backend.draw_number_input(self, &layout);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for ComboBox {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        // Standalone (non-`Form`-embedded) display: the field itself only
        // ever needs its own row, so `field_bounds` is the top row of
        // `area` while the full `area` is passed as the dropdown's
        // viewport -- otherwise `Completions::layout`'s placement math has
        // no room above or below the field to open the dropdown into and
        // always clips it to zero height. A single-row item height is
        // assumed since no measurement pass runs outside a real UI loop.
        // The dropdown itself only paints when `self.open` is true with a
        // non-empty filtered list, so a closed/default combo box renders
        // just its field regardless of these choices. "Row" is one line
        // scaled by `line_height`, the backend-native unit (cells on TUI,
        // pixels on native), so the field and its popup rows size correctly
        // on every backend instead of collapsing to one pixel on native.
        let line_height = backend.line_height();
        let field_bounds = Rect::new(area.x, area.y, area.width.max(line_height), line_height);
        let max_popup_height = (area.height - line_height).max(line_height);
        let layout = self.layout(
            field_bounds,
            area,
            area.width.max(line_height),
            max_popup_height,
            |_| CompletionItemMeasure::new(line_height),
        );
        backend.draw_combo_box(self, &layout);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for DatePicker {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        // Standalone (non-`Form`-embedded) display: derive a 7-column,
        // 6-row month grid plus a one-row header (and a one-row time field
        // when `self.time` is set), clipped to the given area. The header
        // and time rows are one line each, scaled by `line_height` (the
        // backend-native unit: cells on TUI, pixels on native) so they size
        // correctly on every backend instead of collapsing to one pixel on
        // native; the grid rows are then whatever is left over divided by
        // six, floored at one line, so a date picker painted into a band
        // shorter than 7 lines deliberately overflows the band rather than
        // shrinking its cells below a usable size.
        let line_height = backend.line_height();
        let header_height = line_height;
        let time_height = if self.time.is_some() {
            line_height
        } else {
            0.0
        };
        let cell_width = (area.width / 7.0).max(line_height);
        let cell_height = ((area.height - header_height - time_height) / 6.0).max(line_height);
        let layout = self.layout(
            area.x,
            area.y,
            cell_width,
            cell_height,
            header_height,
            time_height,
        );
        backend.draw_date_picker(self, &layout);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Chart {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        // Display-only: no hover/crosshair state is tracked by this
        // binding, so both are `None` and the returned `ChartLayout` (for
        // click/nearest-point routing) is discarded.
        backend.draw_chart(area, self, None, None);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for MessageList {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_message_list(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for BoardModel {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_board(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for PipelineView {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_pipeline_view(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Minimap {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_minimap(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for MultiSectionView {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_multi_section_view(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for DiffView {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_diff_view(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for ActivityBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_activity_bar(area, self, None);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Toolbar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_toolbar_interactive(area, self, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for MenuBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_menu_bar(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for StatusBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_status_bar_interactive(area, self, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for CommandLine {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_command_line(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for CommandCenter {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_command_center(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Palette {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_palette(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Scrollbar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_scrollbar(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
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

        match RenderFrame::from_dict(dict) {
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
    use quadraui::MouseButton;

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
}
