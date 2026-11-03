//! PyO3 bindings over `quadraui`'s headless TUI driver, plus the real
//! terminal and native-window runners that take it over.
//!
//! The language-neutral hit testing, semantic event synthesis, text-input/
//! focus/drag controllers, retained descriptor model, render -> frame
//! assembly, and descriptor wire decode (layout trees, render frames,
//! additive-primitive descriptors, `run_shell`'s `ShellConfig`) this
//! binding drives all live in `quadraui_binding_core` (see that crate's
//! module doc) — this file is the PyO3 leaf around it: `depythonize` a
//! Python `dict` into a `pythonize::Depythonizer`, hand that to the core
//! decoder, and map its `DecodeError` to `InvalidDescriptorError`, plus
//! [`PyAppLogic`]'s callback bridge and the `QuiApp` surface below.
//!
//! [`QuiApp`] wraps a `quadraui::tui::testing::TuiDriver` running an
//! in-crate [`AppLogic`] that holds one optional descriptor per core
//! primitive (button, text display, list, tree, data table, tab bar), plus
//! any number of additive primitives (`insert_extra`/`insert_progress`/
//! `insert_spinner`/`insert_image`) registered in
//! `quadraui_binding_core::extra_paint`'s `extra_primitives!` table — see
//! that macro's doc for why registering a further additive primitive never
//! touches the six fixed slots. Each `insert_*` method takes a Python
//! `dict`, decodes it into the matching descriptor type via
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
//!
//! [`QuiApp::waker`] returns a [`QuiAppWaker`] — unlike `QuiApp` itself
//! (`unsendable`, pinned to its creating thread), this handle is safe to
//! hand to a background thread (a worker thread, an asyncio loop running
//! elsewhere, a subprocess-watcher thread, a timer), which calls its
//! `post`/`wake` methods to deliver a value back into the app's event
//! stream as a `{"User": value}` event and wake whichever loop currently
//! owns it, without exiting — see that type's doc for the per-call-path
//! coverage (`step`/`run`, and `run_tui` for its own duration).
// Only spelled in this crate's own code inside `run_native`'s real
// `target_os = "macos"` arm and in `#[cfg(test)]` coverage for the
// downgrade `NativeExitPolicy` (aliased below as `NativeAppLogic`)
// performs on this feature -- genuinely unused on a `macos`-feature,
// non-Apple-`target_os` build (e.g. `cargo check --features macos` on
// Linux), which `run_native`'s own non-Apple arm exists to keep buildable
// without ever opening a real window.
#[cfg(all(feature = "macos", not(feature = "gtk")))]
#[allow(unused_imports)]
use std::cell::Cell;
use std::cell::RefCell;
#[cfg(test)]
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
#[cfg(test)]
use pyo3::types::PyTuple;
use pyo3::types::{PyDict, PyList};

#[cfg(test)]
use quadraui::primitives::scrollbar::{scroll_by_clamped, ThumbDrag};
use quadraui::tui::testing::{driver_with_shell, TuiDriver};
use quadraui::tui::{RunConfig, StepOutcome, TuiBackend, TuiRunner};
#[cfg(test)]
use quadraui::wheel_row_delta;
#[cfg(test)]
use quadraui::AcceleratorScope;
#[cfg(test)]
use quadraui::Key;
#[cfg(test)]
use quadraui::KeyBinding;
#[cfg(test)]
use quadraui::Modifiers;
#[cfg(test)]
use quadraui::MouseButton;
#[cfg(test)]
use quadraui::NamedKey;
#[cfg(test)]
use quadraui::Scrollbar;
use quadraui::{
    Accelerator, AcceleratorId, AppLogic, AppShellEvent, AppShellLayout, Backend, BackendCaps,
    BottomPanelEvent, Completions, ContextMenuItem, DialogButton, DialogSeverity,
    FileDialogOptions, Float, FloatMeasure, GlobalShortcutId, Notification, Point, Reaction, Rect,
    RgbaImage, RichTextPopup, ShellApp, ShellConfig, ShellContext, Toast, ToastCorner, Tooltip,
    UiEvent, UserPayload, WidgetId,
};

// The language-neutral `event -> hit -> semantic -> render` engine this
// binding is a thin PyO3 leaf over -- see
// `quadraui-binding-core/src/lib.rs`'s module doc. Imported under their
// bare names (rather than a `qbc::` path prefix) so this file's own call
// sites, most of which predate the crate split, need no further change.
#[cfg(test)]
use quadraui_binding_core::tree_scroll_state;
#[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
use quadraui_binding_core::NativeExitPolicy;
use quadraui_binding_core::{
    callback_reaction, caret_shape, click_down_up_events, color_depth_name, decode_extra_primitive,
    dialog_severity, draw_bands, font_role, key_pressed_event, merge_overrides,
    message_dialog_options_from_buttons, paint_layout, panic_message, render_overlays, step_driver,
    text_display_from_lines, theme_color_value, toast_corner, window_close_reaction, BindingHost,
    ErasedShellDriver, EventSink, HitDetail, InteractionEngine, OverrideError, RenderOutput,
    WindowCloseHandling,
};

/// A [`Backend::waker`] closure, shared (and swapped, by `QuiApp::run_tui`)
/// between a [`QuiApp`] and every [`QuiAppWaker`] handed out by
/// [`QuiApp::waker`]. Factored into a named alias rather than spelled out
/// inline on both of those — `clippy::type_complexity` rejects the bare
/// nested `Arc<Mutex<Arc<dyn Fn(...) + Send + Sync>>>` spelling under
/// `-D warnings`.
type SharedWaker = Arc<Mutex<Arc<dyn Fn(UserPayload) + Send + Sync>>>;

/// Restores a [`SharedWaker`] cell to its previous value when dropped —
/// used by [`QuiApp::run_tui`] around its `current_waker` swap so the
/// restore runs whether `py.detach`'s pump loop returns normally *or*
/// panics and unwinds through this scope. Without it, a panic there would
/// poison the app (via [`QuiApp::guarded`]'s `catch_unwind`) but leave
/// `current_waker` pointing at this dropped `TuiRunner`'s backend; most
/// `QuiApp` methods then immediately raise `PoisonedError` and never touch
/// it again, but [`QuiApp::waker`] bypasses `guarded` by design (see its
/// own poisoned check) and would otherwise still hand out a handle whose
/// `post`/`wake` silently post into a queue nothing drains anymore.
struct RestoreWakerOnDrop<'a> {
    current_waker: &'a SharedWaker,
    previous: Option<Arc<dyn Fn(UserPayload) + Send + Sync>>,
}

impl Drop for RestoreWakerOnDrop<'_> {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            *self.current_waker.lock().unwrap_or_else(|e| e.into_inner()) = previous;
        }
    }
}

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

create_exception!(
    _quadraui,
    InvalidFontRoleError,
    QuadrauiError,
    "`measure_text`'s `role` argument was neither \"chrome\" nor \
    \"editor\" -- the only two `FontRole` variants a caller can name."
);

/// Slot id of the app's `Button` descriptor.
const SLOT_BUTTON: i64 = quadraui_binding_core::SLOT_BUTTON;
/// Slot id of the app's `TextDisplay` descriptor.
const SLOT_TEXT_DISPLAY: i64 = quadraui_binding_core::SLOT_TEXT_DISPLAY;
/// Slot id of the app's `ListView` descriptor.
const SLOT_LIST: i64 = quadraui_binding_core::SLOT_LIST;
/// Slot id of the app's `TreeView` descriptor.
const SLOT_TREE: i64 = quadraui_binding_core::SLOT_TREE;
/// Slot id of the app's `DataTable` descriptor.
const SLOT_DATA_TABLE: i64 = quadraui_binding_core::SLOT_DATA_TABLE;
/// Slot id of the app's `TabBar` descriptor.
const SLOT_TAB_BAR: i64 = quadraui_binding_core::SLOT_TAB_BAR;

/// The [`AppLogic`] a [`QuiApp`] drives: a thin PyO3 leaf around
/// [`BindingHost`] (the six fixed primitive slots, any additive slots, and
/// the backend-settings replay store — see that type's own doc) plus the
/// Python-specific callback fields below. `render_callback`, when
/// registered, overrides [`BindingHost`]'s static slots for every render —
/// each frame asks it for a fresh set of descriptors instead.
#[derive(Default)]
struct PyAppLogic {
    /// The host-side slot/replay state this binding is a thin PyO3 leaf
    /// over — see [`BindingHost`]'s own doc.
    host: BindingHost,
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
    /// overrides [`Self::host`]'s static slots for every render — it is the
    /// one way an app's displayed state can change after the runner has
    /// taken over the event loop, since the event handler that mutates that
    /// state cannot itself re-borrow this `QuiApp` to call `insert_*`.
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
        self.host.slot_occupied(slot)
    }

    /// Whether `slot` names one of this app's fixed primitive slots.
    fn is_known_slot(slot: i64) -> bool {
        BindingHost::is_known_slot(slot)
    }

    /// Empties one slot, returning whether it held anything.
    fn clear_slot(&mut self, slot: i64) -> bool {
        self.host.clear_slot(slot)
    }

    /// Whether `kind` names a currently-occupied additive primitive slot.
    fn extra_occupied(&self, kind: &str) -> bool {
        self.host.extra_occupied(kind)
    }

    /// Empties one additive primitive slot, returning whether it held
    /// anything.
    fn clear_extra_slot(&mut self, kind: &str) -> bool {
        self.host.clear_extra_slot(kind)
    }
}

impl WindowCloseHandling for PyAppLogic {
    fn has_window_close_handler(&self) -> bool {
        self.handler.is_some()
    }
}

/// Maps a registered handler/tick callback's return value to a [`Reaction`]
/// via [`callback_reaction`]: returning `False` requests the same exit a
/// raised exception would, with no exception needed; returning anything
/// else (including the common case of no explicit `return`, i.e. `None`)
/// continues. Checked with `extract::<bool>`, which only matches an actual
/// Python `bool` — an int `0`, an empty string, or any other "falsy" value
/// keeps running rather than exiting, so only an explicit `return False`
/// opts in.
fn reaction_from_return(value: &Bound<'_, PyAny>) -> Reaction {
    callback_reaction(value.extract::<bool>().ok())
}

/// Wraps [`reaction_from_return`], promoting an ordinary `Continue` to
/// `Redraw` whenever `render_callback_present` — i.e. a render callback is
/// registered on this app. Mirrors [`callback_reaction_for_render`]'s
/// policy: see that function's doc for why. In short, a Python handler's
/// mutation of its own plain-Python state can't be observed by
/// `PyAppLogic::handle`/`tick`, so once a render callback is registered
/// every non-exit dispatch must be treated as one that might have changed
/// something visible.
fn reaction_for_callback(value: &Bound<'_, PyAny>, render_callback_present: bool) -> Reaction {
    match reaction_from_return(value) {
        Reaction::Continue if render_callback_present => Reaction::Redraw,
        other => other,
    }
}

/// Maps a registered handler's return value to a [`Reaction`] for a
/// `WindowClose` dispatch specifically, via [`window_close_reaction`] — the
/// opposite default from [`reaction_from_return`]. A `WindowClose` handler's
/// job is to decide whether the window may close, so the no-opinion case (no
/// explicit `return`, i.e. `None` — by far the common case for a handler
/// that just logs or ignores the event, like `examples/file_manager.py`'s
/// quit wrapper) must let the close proceed, exactly like having no handler
/// registered at all; only an explicit `return True` vetoes it and leaves
/// the window open. Checked with `extract::<bool>`, so a non-`bool` return
/// (an int, a string, any other "truthy" value) does not accidentally veto
/// a close the author never meant to block.
fn reaction_from_window_close_return(
    value: &Bound<'_, PyAny>,
    render_callback_present: bool,
) -> Reaction {
    window_close_reaction(value.extract::<bool>().ok(), render_callback_present)
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

/// Decodes `QuiApp::show_message_dialog`'s `severity` string into a
/// [`DialogSeverity`] via [`quadraui_binding_core::dialog_severity`],
/// mapping its `Err` into this binding's own `InvalidDescriptorError`
/// (prefixed with `what`, the calling method's own name).
fn parse_dialog_severity(what: &str, value: &str) -> PyResult<DialogSeverity> {
    dialog_severity(value).map_err(|err| InvalidDescriptorError::new_err(format!("{what}: {err}")))
}

/// Decodes `QuiApp::show_toast`'s `corner` string into a [`ToastCorner`]
/// via [`quadraui_binding_core::toast_corner`], the same
/// [`parse_dialog_severity`] error-mapping shape.
fn parse_toast_corner(what: &str, value: &str) -> PyResult<ToastCorner> {
    toast_corner(value).map_err(|err| InvalidDescriptorError::new_err(format!("{what}: {err}")))
}

impl AppLogic for PyAppLogic {
    type AreaId = ();

    /// Replays [`Self::host`]'s accelerators and backend-settings fields
    /// onto `backend` -- [`BindingHost::setup`] -- so a chord/theme/style/
    /// font set before `run_tui`/`run_native` moved this `PyAppLogic` into
    /// a freshly constructed `TuiBackend`/`GtkBackend`/`MacBackend`/
    /// `WinBackend` still applies to that live backend, not just the
    /// dormant headless one `QuiApp::step`/`run` drive directly. Every real
    /// runner (`TuiRunner::new_with`, `GtkRunner`'s `activate`,
    /// `MacRunner::new`, `WinRunner::new_with`) calls
    /// `app.setup(&mut backend)` right after building its backend and
    /// before the first frame, so this hook reaches all four.
    fn setup(&mut self, backend: &mut dyn Backend) {
        self.host.setup(backend);
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut hits = Vec::new();
        let chrome_interaction = |id: WidgetId| self.engine.chrome_interaction(id);
        let Some(callback) = self.render_callback.as_ref() else {
            draw_bands(
                backend,
                self.host.render_refs(),
                &mut hits,
                &chrome_interaction,
            );
            render_overlays(&self.engine, backend, hits);
            return;
        };
        match render_output_from_callback(callback, &self.pending_error) {
            Some(RenderOutput::Flat(frame)) => {
                draw_bands(backend, frame.as_refs(), &mut hits, &chrome_interaction)
            }
            Some(RenderOutput::Tree(node)) => {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                paint_layout(backend, area, &node, &mut hits, &chrome_interaction);
            }
            None => {}
        }
        render_overlays(&self.engine, backend, hits);
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
        // `UiEvent::User`'s `Serialize` deliberately errors (its payload is
        // an opaque `Arc<dyn Any>` with no wire representation to invent —
        // see `UserPayload`'s own doc) — matched here, before the
        // `pythonize` call below ever sees it, so a wake delivered through
        // `QuiAppWaker::post`/`::wake` (or a native tray/timer wake with no
        // Python-side payload at all) reaches the handler as a plain
        // `{"User": value}` dict instead of pythonize's error turning into
        // a recorded `InternalError` and `Reaction::Exit` that silently
        // kills the app the first time anything wakes it this way.
        // `UserPayload::downcast_ref` recovers the original `Py<PyAny>`
        // this binding's own `QuiAppWaker` wrapped it in; a payload from a
        // non-Python producer (a GTK tray wake, a future JS binding
        // sharing this same engine) downcasts to `None` and surfaces as
        // `{"User": None}` rather than crashing. No `hit_detail` ever
        // accompanies a `User` event (it carries no screen position), so
        // the "hit" dict key below never applies here.
        if let UiEvent::User(payload) = event {
            return Python::attach(|py| {
                let user_value = payload
                    .downcast_ref::<Py<PyAny>>()
                    .map_or_else(|| py.None(), |py_obj| py_obj.clone_ref(py));
                let dict = PyDict::new(py);
                if let Err(err) = dict.set_item("User", user_value) {
                    record_error(self.pending_error, err);
                    return Reaction::Exit;
                }
                match self.handler.call1(py, (dict,)) {
                    Ok(result) => {
                        reaction_for_callback(result.bind(py), self.render_callback_present)
                    }
                    Err(err) => {
                        record_error(self.pending_error, err);
                        Reaction::Exit
                    }
                }
            });
        }
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
                // `WindowClose` is the one event whose no-opinion default
                // (no explicit `return`, i.e. `None`) must let the close
                // proceed rather than continue — see
                // `reaction_from_window_close_return`'s doc.
                Ok(result) if matches!(event, UiEvent::WindowClose) => {
                    reaction_from_window_close_return(result.bind(py), self.render_callback_present)
                }
                Ok(result) => reaction_for_callback(result.bind(py), self.render_callback_present),
                Err(err) => {
                    record_error(self.pending_error, err);
                    Reaction::Exit
                }
            }
        })
    }
}

// ── ShellApp / `QuiApp.run_shell` ──────────────────────────────────────────
//
// `ShellApp`/`ShellConfig` (`quadraui::shell`) and the tabbed
// `BottomPanelConfig` (`quadraui::compose::bottom_panel`) give an app a
// VS Code-style frame — activity bar, sidebar, optional bottom-panel tab
// strip — around its own content, with the chrome interaction (panel
// switching, sidebar show/hide/resize, bottom-panel tab clicks) handled
// entirely inside `quadraui::shell_adapter::ShellAdapter` before anything
// reaches this binding. `run_shell` below is this binding's counterpart to
// `QuiApp::new`: it decodes a `ShellConfig` from a Python `dict` (there is
// no `Deserialize` impl on `ShellConfig`/`PanelDefinition`/
// `BottomPanelConfig` upstream — `shell_config_from_dict` below reads the
// handful of fields this binding supports field-by-field instead, the same
// style every other `*_from_dict` helper in this file already uses),
// builds a [`PyShellApp`] around the registered Python callbacks, and
// drives it headlessly via `quadraui::tui::testing::driver_with_shell` —
// the exact stack `quadraui::tui::shell_runner::run_with_shell` uses for a
// real terminal, minus the live event loop.
//
// `driver_with_shell` returns `TuiDriver<impl AppLogic>`: an opaque,
// per-call-site type with no name this file could give a struct field.
// [`QuiShellApp`] stores a `Box<dyn ErasedShellDriver>` instead — the
// blanket impl just below type-erases any `TuiDriver<A>` behind the narrow
// surface `QuiShellApp` actually needs, so `Box::new(driver) as Box<dyn
// ErasedShellDriver>` type-checks at the one call site that knows the
// concrete (opaque) type, with nothing to name anywhere else.
//
// `ShellAdapter`'s own `app: A` field is `pub(crate)` to `quadraui` — not
// reachable from this crate even through `TuiDriver::app_mut()` — so
// `PyShellApp` cannot be reached back out through the driver the way
// `QuiApp::set_handler` reaches `PyAppLogic` via `self.driver.app_mut()`.
// `ShellState` is the side-channel around that: `QuiShellApp` and the
// `PyShellApp` moved into the driver each hold their own `Rc` clone of the
// same `RefCell<ShellState>`, so `QuiShellApp::set_handler` et al. mutate
// it directly, independent of the driver.

/// Registered Python callbacks + the last callback-raised error for one
/// [`QuiShellApp`] session. Shared (via `Rc<RefCell<_>>`) between the
/// [`QuiShellApp`] a caller holds and the [`PyShellApp`] moved into its
/// driver — see this module's "ShellApp / `QuiApp.run_shell`" doc for why
/// a side channel is needed at all.
#[derive(Default)]
struct ShellState {
    /// Invoked with the pythonized event for every event [`PyShellApp::handle`]
    /// receives — i.e. every event the shell chrome itself didn't consume
    /// (see [`crate::ShellApp::handle`]'s doc). `None` leaves such events
    /// unhandled, same as a plain `QuiApp` with no `set_handler` call.
    handler: Option<Py<PyAny>>,
    /// Invoked with no arguments whenever [`PyShellApp::tick`] fires with no
    /// event queued, mirroring [`QuiApp::set_tick_handler`].
    on_tick: Option<Py<PyAny>>,
    /// Invoked with no arguments once per frame; its return value, if a
    /// `list[str]`, becomes the lines of a single-line-per-entry
    /// `TextDisplay` painted into [`crate::AppShellLayout::main_content_bounds`].
    /// `None` paints nothing into the main content area — the shell chrome
    /// (activity bar, sidebar, bottom-panel tab strip) still renders either
    /// way, since that is `AppShell`'s own unconditional chrome paint, not
    /// part of [`crate::ShellApp::render_content`].
    render_content: Option<Py<PyAny>>,
    /// Invoked with one pythonized `AppShellEvent` dict (see
    /// [`app_shell_event_to_py`]) whenever [`crate::ShellApp::on_shell_event_ctx`]
    /// fires — a panel switch, the sidebar being hidden/resized, the bottom
    /// panel being resized/hidden, or a bottom-item (no-panel) activity-bar
    /// click.
    on_shell_event: Option<Py<PyAny>>,
    /// Invoked with one pythonized `BottomPanelEvent` dict (see
    /// [`bottom_panel_event_to_py`]) whenever [`crate::ShellApp::on_bottom_panel_event`]
    /// fires — a tab activated, a tab closed, maximise toggled, or a
    /// drag-resize. Only ever fires when `run_shell`'s `config` included a
    /// `"bottom_panel"` entry.
    on_bottom_panel_event: Option<Py<PyAny>>,
    /// Set by any of the callbacks above when the Python callable they
    /// invoke raises. [`QuiShellApp::step_once`] takes this and re-raises it
    /// out of `step`/`run`, mirroring [`PyAppLogic::pending_error`].
    pending_error: Option<PyErr>,
}

impl ShellState {
    /// Records `err` as this session's pending error, but only if no
    /// earlier error already claimed the slot — mirrors [`record_error`]'s
    /// "first error wins" contract for [`PyAppLogic::pending_error`].
    fn record_error(&mut self, err: PyErr) {
        if self.pending_error.is_none() {
            self.pending_error = Some(err);
        }
    }
}

/// Pythonizes one [`AppShellEvent`] as an externally-tagged `dict`, e.g.
/// `{"PanelChanged": {"panel_id": "panel:explorer"}}` or
/// `{"SidebarHidden": None}` — the same shape `pythonize::pythonize` would
/// give a `#[derive(Serialize)]` enum, hand-built here since `AppShellEvent`
/// has no such derive upstream (see this module's "ShellApp /
/// `QuiApp.run_shell`" doc for why this binding doesn't add one).
fn app_shell_event_to_py<'py>(
    py: Python<'py>,
    event: &AppShellEvent,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    match event {
        AppShellEvent::PanelChanged { panel_id } => {
            let inner = PyDict::new(py);
            inner.set_item("panel_id", panel_id.as_str())?;
            dict.set_item("PanelChanged", inner)?;
        }
        AppShellEvent::SidebarHidden => dict.set_item("SidebarHidden", py.None())?,
        AppShellEvent::SidebarResized { new_width } => {
            let inner = PyDict::new(py);
            inner.set_item("new_width", new_width)?;
            dict.set_item("SidebarResized", inner)?;
        }
        AppShellEvent::BottomPanelResized { new_height } => {
            let inner = PyDict::new(py);
            inner.set_item("new_height", new_height)?;
            dict.set_item("BottomPanelResized", inner)?;
        }
        AppShellEvent::BottomPanelHidden => dict.set_item("BottomPanelHidden", py.None())?,
        AppShellEvent::BottomItemClicked { id } => {
            let inner = PyDict::new(py);
            inner.set_item("id", id.as_str())?;
            dict.set_item("BottomItemClicked", inner)?;
        }
        AppShellEvent::Consumed => dict.set_item("Consumed", py.None())?,
        AppShellEvent::Ignored => dict.set_item("Ignored", py.None())?,
    }
    Ok(dict)
}

/// Pythonizes one [`BottomPanelEvent`] as an externally-tagged `dict`, e.g.
/// `{"TabActivated": "bp:terminal"}` or `{"MaximiseToggled": None}` — see
/// [`app_shell_event_to_py`]'s doc for why this is hand-built rather than
/// derived.
fn bottom_panel_event_to_py<'py>(
    py: Python<'py>,
    event: &BottomPanelEvent,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    match event {
        BottomPanelEvent::TabActivated(id) => dict.set_item("TabActivated", id)?,
        BottomPanelEvent::TabClosed(id) => dict.set_item("TabClosed", id)?,
        BottomPanelEvent::MaximiseToggled => dict.set_item("MaximiseToggled", py.None())?,
        BottomPanelEvent::Resized(height) => dict.set_item("Resized", height)?,
    }
    Ok(dict)
}

/// `WidgetId` string `quadraui::compose::bottom_panel::BottomPanelController::build_tab_bar`
/// gives the `TabBar` it paints — fixed, not per-config, so
/// [`QuiShellApp::bottom_panel_tab_center`] can name it directly.
const BOTTOM_PANEL_TABS_WIDGET_ID: &str = "bottom-panel:tabs";

/// The [`ShellApp`] [`QuiApp::run_shell`] drives: a thin bridge from its
/// four hooks to the Python callables held in a shared [`ShellState`] —
/// see this module's "ShellApp / `QuiApp.run_shell`" doc for why the state
/// is held this way rather than reachable back out through the driver.
struct PyShellApp {
    state: Rc<RefCell<ShellState>>,
}

impl PyShellApp {
    /// Clones out the registered callback named by `pick`, if any, without
    /// holding the `RefCell` borrow across the call into Python that
    /// follows. `pick` is a plain field-accessor closure (`|s| &s.handler`,
    /// ...) rather than a method name, since `ShellState`'s callback fields
    /// aren't behind a common accessor trait.
    fn callback(&self, pick: impl FnOnce(&ShellState) -> &Option<Py<PyAny>>) -> Option<Py<PyAny>> {
        Python::attach(|py| {
            pick(&self.state.borrow())
                .as_ref()
                .map(|cb| cb.clone_ref(py))
        })
    }
}

impl ShellApp for PyShellApp {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let Some(callback) = self.callback(|s| &s.render_content) else {
            return;
        };
        let bounds = layout.main_content_bounds;
        Python::attach(|py| match callback.call0(py) {
            Ok(result) => {
                if let Ok(lines) = result.extract::<Vec<String>>(py) {
                    let td = text_display_from_lines(WidgetId::new("shell:content"), lines);
                    backend.draw_text_display(bounds, &td);
                }
            }
            Err(err) => self.state.borrow_mut().record_error(err),
        });
    }

    fn handle(
        &mut self,
        event: UiEvent,
        _backend: &mut dyn Backend,
        _ctx: &ShellContext,
    ) -> Reaction {
        let Some(callback) = self.callback(|s| &s.handler) else {
            return Reaction::Continue;
        };
        let render_content_present = self.state.borrow().render_content.is_some();
        Python::attach(|py| {
            let payload = match pythonize::pythonize(py, &event) {
                Ok(value) => value,
                Err(err) => {
                    self.state
                        .borrow_mut()
                        .record_error(InternalError::new_err(format!(
                            "failed to pythonize event: {err}"
                        )));
                    return Reaction::Exit;
                }
            };
            match callback.call1(py, (payload,)) {
                Ok(result) if matches!(event, UiEvent::WindowClose) => {
                    reaction_from_window_close_return(result.bind(py), render_content_present)
                }
                Ok(result) => reaction_for_callback(result.bind(py), render_content_present),
                Err(err) => {
                    self.state.borrow_mut().record_error(err);
                    Reaction::Exit
                }
            }
        })
    }

    fn on_shell_event_ctx(&mut self, event: &AppShellEvent, _ctx: &ShellContext) {
        let Some(callback) = self.callback(|s| &s.on_shell_event) else {
            return;
        };
        Python::attach(|py| {
            let payload = match app_shell_event_to_py(py, event) {
                Ok(value) => value,
                Err(err) => {
                    self.state.borrow_mut().record_error(err);
                    return;
                }
            };
            if let Err(err) = callback.call1(py, (payload,)) {
                self.state.borrow_mut().record_error(err);
            }
        });
    }

    fn on_bottom_panel_event(&mut self, event: &BottomPanelEvent) {
        let Some(callback) = self.callback(|s| &s.on_bottom_panel_event) else {
            return;
        };
        Python::attach(|py| {
            let payload = match bottom_panel_event_to_py(py, event) {
                Ok(value) => value,
                Err(err) => {
                    self.state.borrow_mut().record_error(err);
                    return;
                }
            };
            if let Err(err) = callback.call1(py, (payload,)) {
                self.state.borrow_mut().record_error(err);
            }
        });
    }

    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        let Some(callback) = self.callback(|s| &s.on_tick) else {
            return Reaction::Continue;
        };
        let render_content_present = self.state.borrow().render_content.is_some();
        Python::attach(|py| match callback.call0(py) {
            Ok(result) => reaction_for_callback(result.bind(py), render_content_present),
            Err(err) => {
                self.state.borrow_mut().record_error(err);
                Reaction::Exit
            }
        })
    }
}

/// Decodes `run_shell`'s `config` dict into a [`ShellConfig`] via the
/// language-neutral decoder in `quadraui_binding_core`, mapping its
/// `DecodeError` to `InvalidDescriptorError`. There is no `Deserialize`
/// impl on `ShellConfig`/`PanelDefinition`/`BottomPanelConfig` upstream, so
/// that decoder reads the subset of `ShellConfig`'s builder surface this
/// binding supports field-by-field instead of depending on one — see its
/// own doc for the recognized key vocabulary.
fn shell_config_from_dict(dict: &Bound<'_, PyDict>) -> PyResult<ShellConfig> {
    let mut de = pythonize::Depythonizer::from_object(dict.as_any());
    quadraui_binding_core::decode_shell_config(&mut de)
        .map_err(|err| InvalidDescriptorError::new_err(err.0))
}

/// Takes and returns this session's pending callback-raised error, if any
/// — the `ShellState` counterpart to [`take_pending_error`].
fn take_pending_shell_error(state: &Rc<RefCell<ShellState>>) -> PyResult<()> {
    if let Some(err) = state.borrow_mut().pending_error.take() {
        return Err(err);
    }
    Ok(())
}

/// Headless [`ShellApp`]-backed app: an AppShell (activity bar + sidebar +
/// optional bottom-panel tab strip) around Python-registered content and
/// event handlers. Built by [`QuiApp::run_shell`] — there is no public
/// constructor on this type itself, mirroring how `QuiApp::run_native`'s
/// `NativeAppLogic` is only ever reached through `run_native`.
///
/// `unsendable` for the same reason as [`QuiApp`]: it wraps a `TuiDriver`,
/// whose TUI backend handle is `Rc`/`RefCell`-based and not `Send`.
#[pyclass(module = "quadraui", unsendable)]
struct QuiShellApp {
    driver: Box<dyn ErasedShellDriver>,
    state: Rc<RefCell<ShellState>>,
    /// Mirrors [`QuiApp::pending_events`] — queued by `push_event`/
    /// `push_key_event`, drained one per `step_once` call.
    pending_events: VecDeque<UiEvent>,
    /// Mirrors [`QuiApp::poisoned`].
    poisoned: bool,
}

#[pymethods]
impl QuiShellApp {
    /// Registers `handler` as the callable invoked (with the pythonized
    /// event payload) for every event the shell chrome itself didn't
    /// consume. Replaces any previously registered handler.
    fn set_handler(&mut self, handler: Py<PyAny>) {
        self.state.borrow_mut().handler = Some(handler);
    }

    /// Registers `handler` as the no-argument callable invoked by
    /// `step`/`run` whenever it advances with no event queued. Replaces any
    /// previously registered tick handler.
    fn set_tick_handler(&mut self, handler: Py<PyAny>) {
        self.state.borrow_mut().on_tick = Some(handler);
    }

    /// Registers `callback` as the no-argument render callback invoked once
    /// per frame; its return value, if a `list[str]`, becomes the lines of
    /// a `TextDisplay` painted into the shell's main content area. Replaces
    /// any previously registered content callback.
    fn set_render_content(&mut self, callback: Py<PyAny>) {
        self.state.borrow_mut().render_content = Some(callback);
    }

    /// Registers `callback` as the callable invoked with one pythonized
    /// `AppShellEvent` dict (see [`app_shell_event_to_py`]) whenever a
    /// panel switch, sidebar show/hide/resize, or bottom-panel resize/hide
    /// occurs.
    fn set_on_shell_event(&mut self, callback: Py<PyAny>) {
        self.state.borrow_mut().on_shell_event = Some(callback);
    }

    /// Registers `callback` as the callable invoked with one pythonized
    /// `BottomPanelEvent` dict (see [`bottom_panel_event_to_py`]) whenever
    /// the bottom-panel tab strip is interacted with. Only ever fires when
    /// this app's `config` included a `"bottom_panel"` entry.
    fn set_on_bottom_panel_event(&mut self, callback: Py<PyAny>) {
        self.state.borrow_mut().on_bottom_panel_event = Some(callback);
    }

    /// Decodes `descriptor` into a `UiEvent` and queues it for the next
    /// `step`/`run` call. Raises `InvalidDescriptorError` on a shape
    /// mismatch.
    fn push_event(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<()> {
        let event: UiEvent = decode("push_event", descriptor)?;
        self.pending_events.push_back(event);
        Ok(())
    }

    /// Queues a synthetic `KeyPressed` event for the character with
    /// Unicode code point `ch`, no modifiers held, not a repeat. Raises
    /// `InvalidCharError` if `ch` isn't a valid Unicode scalar value.
    fn push_key_event(&mut self, ch: u32) -> PyResult<()> {
        let ch = char::from_u32(ch).ok_or_else(|| {
            InvalidCharError::new_err(format!("invalid Unicode code point: {ch}"))
        })?;
        self.pending_events.push_back(key_pressed_event(ch));
        Ok(())
    }

    /// Queues a synthetic `MouseDown` + `MouseUp` left-click at `(x, y)`,
    /// in the backend's native (cell) units — the same coordinates
    /// `AppShellLayout`'s bounds (as pythonized by `set_on_shell_event`'s
    /// payloads and this binding's own `find`) are expressed in. Use this
    /// to drive sidebar-toggle / panel-switch / bottom-panel-tab clicks
    /// from Python without hand-building two raw `push_event` dicts.
    fn push_click(&mut self, x: f32, y: f32) {
        self.pending_events.extend(click_down_up_events(x, y));
    }

    /// Resizes the underlying cell grid to `width`x`height`. Raises
    /// `InvalidDimensionsError` if either is zero.
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

    /// Advances the runner by one step, mirroring [`QuiApp::step`]'s
    /// queued-event / tick order (`run_shell` apps have no waker/`User`
    /// event path, so there is no posted-value step to interleave).
    /// Returns `True` if still running, `False` once exited. Re-raises
    /// whatever exception a registered callback raised while handling this
    /// step.
    fn step(&mut self) -> PyResult<bool> {
        self.guarded(|app| app.step_once())
    }

    /// Runs up to `max_steps` steps (default 1), releasing the GIL for the
    /// duration of the Rust loop and re-acquiring it only for each
    /// individual callback — mirrors [`QuiApp::run`].
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

    /// Returns the currently rendered screen as plain text, one line per
    /// row, mirroring `TuiDriver::screen`.
    fn screen(&self) -> String {
        self.driver.screen()
    }

    /// Whether `needle` appears anywhere in the currently rendered screen.
    fn screen_contains(&self, needle: &str) -> bool {
        self.driver.screen_contains(needle)
    }

    /// The `(x, y)` cell position of the first occurrence of `needle` in
    /// the currently rendered screen, or `None` if it isn't present.
    fn find(&self, needle: &str) -> Option<(f32, f32)> {
        self.driver.find(needle)
    }

    /// The `(x, y)` cell position of the center of bottom-panel tab
    /// `tab_idx` (`0` = first tab, in `config["bottom_panel"]["tabs"]`
    /// order), or `None` if this app's `config` had no `"bottom_panel"`, or
    /// `tab_idx` is out of range or scrolled out of view. A tab's label can
    /// repeat elsewhere on screen (or coincide with another tab's), so this
    /// resolves against the tab bar's own cached layout — the same
    /// disambiguation [`Self::find`]'s doc warns a text needle can't give —
    /// rather than `find(label)`.
    fn bottom_panel_tab_center(&self, tab_idx: usize) -> Option<(f32, f32)> {
        self.driver
            .tab_center(&WidgetId::new(BOTTOM_PANEL_TABS_WIDGET_ID), tab_idx)
    }
}

impl QuiShellApp {
    /// Mirrors [`QuiApp::guarded`]: catches a Rust panic instead of letting
    /// it unwind across the PyO3 boundary, poisoning this app on one.
    fn guarded<T>(&mut self, body: impl FnOnce(&mut Self) -> PyResult<T>) -> PyResult<T> {
        if self.poisoned {
            return Err(PoisonedError::new_err(
                "this QuiShellApp is poisoned by an earlier caught panic and can no longer be used",
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

    /// Mirrors [`QuiApp::step_once`], minus the `User`/waker path `run_shell`
    /// apps don't have.
    fn step_once(&mut self) -> PyResult<bool> {
        let running = step_driver(&mut *self.driver, &mut self.pending_events);
        take_pending_shell_error(&self.state)?;
        Ok(running)
    }
}

/// Invokes a registered render callback with no arguments and decodes its
/// returned `dict` into a [`RenderOutput`] via
/// [`quadraui_binding_core::decode_render_output`] -- see that function's
/// doc for the flat-vs-tree sniff this binding's [`RenderOutput`] import
/// shares with every other binding. On any failure -- the callback
/// raising, returning something other than a `dict`, or an entry with the
/// wrong shape -- records the error in `pending_error` and returns `None`,
/// leaving the caller to skip drawing this frame rather than paint a
/// half-decoded one.
///
/// Every call site that can trigger a render -- `step`/`run`, `screen`,
/// `screen_contains`, `insert_*`, and `slot_clear` -- drains `pending_error`
/// after rendering and re-raises it, so a render-callback failure surfaces
/// the same way a `handle`/`tick` callback failure does no matter which of
/// those triggered the repaint. `run_tui`/`run_native` additionally break
/// their loop as soon as `pending_error` is set, since nothing else is
/// polling it on the Python side while the GIL is detached.
const RENDER_CALLBACK_WHAT: &str = "render";

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

        let mut de = pythonize::Depythonizer::from_object(dict.as_any());
        match quadraui_binding_core::decode_render_output(RENDER_CALLBACK_WHAT, &mut de) {
            Ok(output) => Some(output),
            Err(err) => {
                record_error(pending_error, InvalidDescriptorError::new_err(err.0));
                None
            }
        }
    })
}

/// Wraps [`PyAppLogic`] for `run_native`'s native runner loop (`GtkRunner`
/// on the `gtk` feature, `MacRunner` on the `macos` feature, `WinRunner` on
/// the `win` feature) -- [`quadraui_binding_core::NativeExitPolicy`], whose
/// own doc covers the two corrections it makes to generic `Reaction`
/// handling: a handler-less `WindowClose` must actually exit (via
/// [`PyAppLogic`]'s [`WindowCloseHandling`] impl above, which answers
/// `self.handler.is_some()`), and -- on the `macos` feature without `gtk`
/// also enabled -- every *other* `Reaction::Exit` downgrades to `Continue`
/// plus a sticky `exit_requested` flag the `run_native` pump loop below
/// checks after every `pump` call, so `[NSApp terminate:]` never fires out
/// from underneath `run_native` and ends the whole OS process before it
/// could return control (and any caught exception with it) to its caller.
///
/// A registered Python handler's own `WindowClose` opinion (see
/// [`reaction_from_window_close_return`]: an explicit `True` vetoes the
/// close and leaves the window open; `None`/`False`/raising confirms it)
/// reaches the caller untouched either way -- this wrapper only overrides
/// the no-handler-at-all case, which [`PyAppLogic::handle`] alone would
/// otherwise map to `Reaction::Continue`, leaving a bare `run_native()`
/// call with no way to close its own window.
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
type NativeAppLogic = NativeExitPolicy<PyAppLogic>;

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
    /// The [`Backend::waker`] closure for whichever backend currently owns
    /// event delivery, cloned out into every [`QuiAppWaker`] handed out by
    /// [`QuiApp::waker`]. Starts out (and, once `run_tui` returns, goes
    /// back to) `self.driver.backend().waker()` — the headless driver's own
    /// `TuiBackend`, drained by [`QuiApp::step_once`]'s `pump_user_events`
    /// call; `run_tui` swaps this to the live `TuiRunner`'s own backend's
    /// waker for the duration of its pump loop, so a `QuiAppWaker` obtained
    /// before that call keeps delivering to the live terminal loop once it
    /// starts, rather than only to the dormant headless driver (`run_tui`
    /// blocks the Python thread that calls it and `QuiApp` is `unsendable`,
    /// so there is no reachable way to call `waker()` *during* the call —
    /// only before it). `run_native` does not swap this: a window
    /// opened via `run_native` has no `Backend::waker`-reachable accessor
    /// on `GtkRunner`/`MacRunner`/`WinRunner` today, so a `QuiAppWaker`
    /// obtained while a native window is open still points at the dormant
    /// headless backend — a post made then queues rather than being lost,
    /// and is only delivered once the window closes and `step`/`run`
    /// resumes driving the headless driver again. That queue
    /// (`UserEventQueue`, an unbounded `VecDeque`) is not actually free to
    /// leave filling, though: each queued post holds a strong `Py<PyAny>`
    /// reference, so a background thread posting periodically for a long
    /// `run_native` session accumulates Python objects for the window's
    /// whole lifetime and delivers the entire backlog in one burst on the
    /// first `step`/`run` call after it closes. An app that posts during a
    /// native window's lifetime and cares about that backlog should drain
    /// it with repeated `step()` calls right after `run_native` returns,
    /// or avoid posting while the window is open.
    ///
    /// A `Mutex` rather than a bare field: [`QuiAppWaker`] must be
    /// `Send + Sync` so a background thread can hold and call it, which
    /// rules out `Rc<RefCell<_>>`; the lock is only ever held for the
    /// instant of a clone or a swap, never across a call into the wrapped
    /// closure itself.
    current_waker: SharedWaker,
}

/// A thread-safe handle, returned by [`QuiApp::waker`], that a background
/// thread calls `post`/`wake` on to deliver a value into its `QuiApp`'s
/// event stream as a `{"User": value}` event and wake whichever loop
/// currently owns that app. See [`QuiApp::waker`]'s doc for why this needs
/// to be a separate object rather than a method on `QuiApp` itself.
///
/// Not `unsendable` — ordinary `Send + Sync` Rust state
/// (`Arc<Mutex<Arc<dyn Fn(UserPayload) + Send + Sync>>>`, and `Py<PyAny>`
/// is itself always `Send + Sync`) — so PyO3 lets Python pass it to another
/// thread freely, unlike `QuiApp`.
#[pyclass(module = "quadraui")]
struct QuiAppWaker(SharedWaker);

#[pymethods]
impl QuiAppWaker {
    /// Delivers `value` to the owning `QuiApp` as a `{"User": value}` event
    /// and wakes whichever loop currently owns it — `step`/`run`'s drain of
    /// `pump_user_events` on the next call for the headless path, or
    /// promptly (not waiting for the next native/terminal event) while
    /// `run_tui` is live. Safe to call from any thread, at any time,
    /// including before the owning `QuiApp` has run a single step or after
    /// it has exited (a post after exit is simply never observed — there is
    /// no receiver left to drain it).
    fn post(&self, value: Py<PyAny>) {
        let waker = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        waker(UserPayload::new(value));
    }

    /// Equivalent to `post(None)`: wakes the owning `QuiApp`'s loop with no
    /// payload of its own, just to make sure a pending tick-driven state
    /// change (e.g. one a background thread just finished mutating through
    /// some other, already-synchronized channel) is noticed promptly
    /// rather than only on the next poll-interval timeout or unrelated
    /// event.
    fn wake(&self, py: Python<'_>) {
        self.post(py.None());
    }

    /// Schedules `post(value)` to run `delay_ms` milliseconds from now, on a
    /// spawned background thread that sleeps for that long and then
    /// delivers — the Python-level equivalent of
    /// [`Backend::request_frame_in`]'s "the app itself knows it
    /// wants to be woken again in `delay`" timer contract. `Backend`'s own
    /// `request_frame_in` isn't reachable from Python at all: it lives on
    /// the live `Backend` passed to [`AppLogic::handle`]/`tick`, and this
    /// binding has no object to hand that reference to Python through (and
    /// no way to reach it from a background thread, since it is `&self`/
    /// `&mut self`-shaped, not `Send`) — see [`QuiApp::waker`]'s doc for the
    /// same "no reachable handle" story for `run_tui`/`run_native`. This
    /// method reuses the already-`Send + Sync` waker machinery instead: it
    /// re-locks the current waker only once `delay_ms` has elapsed, so a
    /// call made before `run_tui` starts still lands in the live terminal
    /// loop if the delay elapses after it starts, exactly like an immediate
    /// `post` would mid-run. Returns immediately; does not block the
    /// calling thread. Equivalent in effect to pairing `threading.Timer` (or
    /// `asyncio`'s `call_later`) with `post` from plain Python — provided
    /// here directly so the common "run_tick again in N ms" case needs no
    /// import beyond `quadraui` itself.
    fn post_after(&self, delay_ms: u64, value: Py<PyAny>) {
        let shared = Arc::clone(&self.0);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay_ms));
            let waker = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            waker(UserPayload::new(value));
        });
    }

    /// Equivalent to `post_after(delay_ms, None)` — see [`Self::wake`]'s doc
    /// for why a payload-free wake is useful on its own.
    fn wake_after(&self, py: Python<'_>, delay_ms: u64) {
        self.post_after(delay_ms, py.None());
    }
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
        let driver = TuiDriver::new(app_logic, width, height);
        let current_waker = Arc::new(Mutex::new(driver.backend().waker()));
        Ok(QuiApp {
            driver,
            pending_events: VecDeque::new(),
            poisoned: false,
            clipboard: None,
            #[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
            native_consumed: false,
            current_waker,
        })
    }

    /// Builds a [`QuiShellApp`]: a headless VS Code-style app frame
    /// (activity bar + sidebar + optional bottom-panel tab strip) around
    /// Python-registered content and event handlers, on a `width`x`height`
    /// cell grid.
    ///
    /// `config` is a `dict` decoded into a `quadraui::ShellConfig` by
    /// [`shell_config_from_dict`] — see that function's doc for the
    /// recognized keys. `on_event`, `render_content`, `on_shell_event`,
    /// `on_bottom_panel_event`, and `on_tick`, if given, are registered
    /// exactly as the matching `QuiShellApp.set_*` method would right
    /// after construction.
    ///
    /// This is a separate class ([`QuiShellApp`], not a mode switch on this
    /// `QuiApp`) rather than an instance method: a plain `QuiApp` wraps a
    /// fixed `TuiDriver<PyAppLogic>`, and `run_shell` drives a different
    /// `AppLogic` (`quadraui::shell_adapter::ShellAdapter<PyShellApp>`,
    /// erased behind `Box<dyn ErasedShellDriver>` since
    /// `driver_with_shell` returns an unnameable `TuiDriver<impl AppLogic>`)
    /// — there is no shared driver field a mode switch could flip between
    /// the two without turning every existing `QuiApp` method into a
    /// mode-dispatching wrapper.
    ///
    /// Raises `InvalidDimensionsError` if `width`/`height` is zero, or
    /// `InvalidDescriptorError` if `config` doesn't decode.
    #[staticmethod]
    #[pyo3(signature = (
        width,
        height,
        config,
        on_event=None,
        render_content=None,
        on_shell_event=None,
        on_bottom_panel_event=None,
        on_tick=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn run_shell(
        width: u16,
        height: u16,
        config: &Bound<'_, PyDict>,
        on_event: Option<Py<PyAny>>,
        render_content: Option<Py<PyAny>>,
        on_shell_event: Option<Py<PyAny>>,
        on_bottom_panel_event: Option<Py<PyAny>>,
        on_tick: Option<Py<PyAny>>,
    ) -> PyResult<QuiShellApp> {
        if width == 0 || height == 0 {
            return Err(InvalidDimensionsError::new_err(
                "width and height must both be non-zero",
            ));
        }
        let shell_config = shell_config_from_dict(config)?;
        let state = Rc::new(RefCell::new(ShellState {
            handler: on_event,
            on_tick,
            render_content,
            on_shell_event,
            on_bottom_panel_event,
            pending_error: None,
        }));
        let app = PyShellApp {
            state: state.clone(),
        };
        let driver = driver_with_shell(app, shell_config, width, height);
        Ok(QuiShellApp {
            driver: Box::new(driver),
            state,
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
        self.pending_events.push_back(key_pressed_event(ch));
        Ok(())
    }

    /// Returns a [`QuiAppWaker`] handle: a thread-safe, `Send + Sync`
    /// object a **background thread** (a worker thread, an asyncio event
    /// loop running elsewhere, a subprocess-watcher thread, a timer) can
    /// call `post`/`wake` on to deliver a value back into this app's event
    /// stream, waking whichever loop currently owns it so the value is
    /// observed promptly rather than only on the next unrelated event or
    /// tick.
    ///
    /// Unlike `QuiApp` itself (`#[pyclass(unsendable)]`, pinned to the
    /// thread that created it), the returned handle is safe to hand to
    /// another Python thread — that is the entire point of this method:
    /// `QuiApp`'s own methods can't be called cross-thread at all, so
    /// event injection from off-thread has to go through a separate,
    /// `Send`-safe object instead. Call this once up front and share the
    /// result with the background thread (e.g. pass it as a `threading.Thread`
    /// constructor argument) rather than trying to call it from that
    /// thread, which would hit the same cross-thread restriction `QuiApp`
    /// itself has.
    ///
    /// Raises `PoisonedError` on a poisoned app (an earlier caught panic —
    /// see [`QuiApp::guarded`]) rather than handing out a handle anyway:
    /// every other `QuiApp` method routes through `guarded`, which already
    /// refuses a poisoned app, and this is the one direct `&self` read of
    /// `current_waker` that would otherwise skip that check — a handle
    /// obtained from it would silently post into whatever queue
    /// `current_waker` last pointed at, with nothing left running to drain
    /// it.
    fn waker(&self) -> PyResult<QuiAppWaker> {
        if self.poisoned {
            return Err(PoisonedError::new_err(
                "this QuiApp is poisoned by an earlier caught panic and can no longer be used",
            ));
        }
        Ok(QuiAppWaker(self.current_waker.clone()))
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

    /// Advances the runner by one step: dispatches the oldest event queued
    /// by `push_event`/`push_key_event`, or — if none is queued — drains and
    /// dispatches every value a [`QuiAppWaker::post`]/`::wake` call delivered
    /// since the last step as a `{"User": value}` event, then ticks if
    /// still nothing was dispatched. A `push_event`/`push_key_event` call
    /// always takes priority over a pending `post`/`wake`, so a steady
    /// stream of pushed events can delay (but never drop) a posted value.
    /// Returns `True` if the app is still running afterwards, `False` once
    /// it has exited. Re-raises whatever exception a registered
    /// handler/tick callback raised while handling this step.
    fn step(&mut self) -> PyResult<bool> {
        self.guarded(|app| app.step_once())
    }

    /// Runs up to `max_steps` steps (default 1), releasing the GIL for the
    /// duration of the Rust loop and re-acquiring it only for each
    /// individual handler/tick callback. Each step follows the same
    /// queued-event / posted-`User`-event / tick order as [`QuiApp::step`].
    /// Stops early once the app exits. Returns `True` if still running
    /// afterwards, `False` once exited. Re-raises whatever exception a
    /// registered handler/tick callback raised during any step.
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
    /// closes the window on every backend by default — with no handler
    /// registered at all, or with one registered that returns `None`
    /// (no explicit `return`, the common case for a handler with no
    /// opinion about closing) or `False`. Only an explicit `return True`
    /// vetoes the close and leaves the window open, e.g. to show an
    /// "unsaved changes?" prompt and let the user decide — see
    /// [`NativeAppLogic`]'s own doc and
    /// [`reaction_from_window_close_return`].
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
    ///
    /// Constructs the real `TuiRunner` (and its `TuiBackend`) before
    /// releasing the GIL, rather than inside the detached pump loop below,
    /// specifically so this call can swap [`QuiApp::waker`]'s shared cell
    /// to that backend's own `Backend::waker` for the loop's whole
    /// duration — a `QuiAppWaker` obtained before or during this call then
    /// wakes the real terminal loop promptly instead of queuing into the
    /// dormant headless driver's backend. Swapped back once the loop
    /// returns control here, whether it exited cleanly, failed to set up,
    /// or a callback raised.
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
            let config = if mouse {
                RunConfig::default()
            } else {
                RunConfig::no_mouse()
            };
            let runner = TuiRunner::new_with(app_logic, config)
                .map_err(|err| InternalError::new_err(format!("run_tui: {err}")))?;
            let previous_waker = std::mem::replace(
                &mut *app.current_waker.lock().unwrap_or_else(|e| e.into_inner()),
                runner.backend().waker(),
            );
            let _restore_waker = RestoreWakerOnDrop {
                current_waker: &app.current_waker,
                previous: Some(previous_waker),
            };
            let wrapped = AssertUngil((runner, loop_error));
            let setup_result: io::Result<()> = py.detach(move || {
                let wrapped = wrapped;
                let (mut runner, loop_error) = wrapped.0;
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
            drop(_restore_waker);
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

    /// Reads the X11/Wayland PRIMARY selection — the platform convention
    /// behind middle-click paste, distinct from `clipboard_read_text`'s
    /// CLIPBOARD selection (populated by an explicit copy). `None` on
    /// every backend but GTK on Linux/BSD, the only one with a PRIMARY
    /// selection concept — including here, since `clipboard_backend`
    /// always wraps the headless `TuiBackend` regardless of which
    /// `run_native` backend (if any) is active.
    fn clipboard_read_primary_selection(&mut self) -> PyResult<Option<String>> {
        self.guarded(|app| {
            Ok(app
                .clipboard_backend()
                .services()
                .clipboard()
                .read_primary_selection())
        })
    }

    /// Reads the current clipboard contents as `(width, height, pixels)`,
    /// where `pixels` is a straight (non-premultiplied) RGBA8 buffer,
    /// row-major top-to-bottom left-to-right, exactly `width * height * 4`
    /// bytes.
    ///
    /// `None` collapses three different [`BackendError`]s this binding
    /// cannot tell apart from here: an empty / non-image clipboard
    /// (`PlatformFailure`, since `arboard` reports "nothing to read" the
    /// same way it reports a real platform error), and a backend with no
    /// image clipboard channel at all (`Unsupported` — always true on
    /// TUI, whose only remote-reachable channel, OSC 52, has no image
    /// form). Unlike [`QuiApp::clipboard_write_image`], this is a plain
    /// probe ("is there an image to read") rather than a demand ("put
    /// this image there"), so a caller checking before falling back to
    /// `clipboard_read_text` sees one uniform "nothing" result instead of
    /// having to catch an exception just to tell `Unsupported` apart from
    /// "clipboard happens to be empty".
    fn clipboard_read_image(&mut self) -> PyResult<Option<(u32, u32, Vec<u8>)>> {
        self.guarded(|app| {
            Ok(app
                .clipboard_backend()
                .services()
                .clipboard()
                .read_image()
                .ok()
                .map(|img| (img.width, img.height, img.pixels)))
        })
    }

    /// Writes `pixels` (straight RGBA8, row-major top-to-bottom
    /// left-to-right) to the system clipboard as an image. Raises
    /// `InvalidDimensionsError` if `pixels` isn't exactly
    /// `width * height * 4` bytes long, and `InternalError` if the backend
    /// itself rejects the write — including `Unsupported` on TUI, which
    /// has no image clipboard channel.
    fn clipboard_write_image(&mut self, width: u32, height: u32, pixels: Vec<u8>) -> PyResult<()> {
        self.guarded(|app| {
            // `checked_mul` rather than a plain `*`: an adversarial
            // `width`/`height` (e.g. `2**31`) overflows `usize` on a 32-bit
            // target and wraps silently in a release build, which would let
            // a zero-length `pixels` buffer slip past the length check
            // below instead of being rejected here.
            let expected = (width as usize)
                .checked_mul(height as usize)
                .and_then(|n| n.checked_mul(4))
                .ok_or_else(|| {
                    InvalidDimensionsError::new_err(format!(
                        "width * height * 4 overflows (width={width}, height={height})"
                    ))
                })?;
            if pixels.len() != expected {
                return Err(InvalidDimensionsError::new_err(format!(
                    "pixels must be exactly width * height * 4 bytes ({expected}), got {}",
                    pixels.len()
                )));
            }
            app.clipboard_backend()
                .services()
                .clipboard()
                .write_image(&RgbaImage {
                    width,
                    height,
                    pixels,
                })
                .map_err(|err| InternalError::new_err(format!("clipboard_write_image: {err:?}")))
        })
    }

    /// Writes `html` to the system clipboard, with `alt_text` as the
    /// plain-text fallback a paste target that only understands text falls
    /// back to. Raises `InternalError` on failure, including
    /// `Unsupported` on TUI, which has no HTML clipboard channel.
    fn clipboard_write_html(&mut self, html: &str, alt_text: &str) -> PyResult<()> {
        self.guarded(|app| {
            app.clipboard_backend()
                .services()
                .clipboard()
                .write_html(html, alt_text)
                .map_err(|err| InternalError::new_err(format!("clipboard_write_html: {err:?}")))
        })
    }

    /// Reads the list of file paths currently on the clipboard (a
    /// Finder/Explorer copy), as a list of their string representations.
    ///
    /// `None` collapses an empty list and a backend with no file-list
    /// clipboard channel at all (TUI) into one result, for the same
    /// reason [`QuiApp::clipboard_read_image`] does — see its doc.
    fn clipboard_read_file_list(&mut self) -> PyResult<Option<Vec<String>>> {
        self.guarded(|app| {
            Ok(app
                .clipboard_backend()
                .services()
                .clipboard()
                .read_file_list()
                .ok()
                .map(|paths| {
                    paths
                        .into_iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect()
                }))
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
                app.host.button = Some(value);
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
                app.host.text_display = Some(value);
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
                app.host.list = Some(value);
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
                app.host.tree = Some(value);
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
                app.host.data_table = Some(value);
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
                app.host.tab_bar = Some(value);
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
    /// `quadraui_binding_core::extra_paint`'s `extra_primitives!` table
    /// (an `InvalidDescriptorError` raised for an unknown `kind` names
    /// every registered one), returning `kind` back. Raises `RuntimeError`
    /// if `kind` is already occupied — call `clear_extra(kind)` first.
    /// Lets a consumer insert a future additive primitive by name alone,
    /// with no new pymethod needed on this class.
    fn insert_extra(&mut self, kind: &str, descriptor: &Bound<'_, PyDict>) -> PyResult<String> {
        self.store_extra(kind, "insert_extra", descriptor)
    }

    /// Empties an additive primitive slot (one registered in
    /// `quadraui_binding_core::extra_paint`'s `extra_primitives!` table),
    /// re-rendering so `screen()` no longer shows it.
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
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            app_logic
                .engine
                .open_context_menu(items, Point::new(position.0, position.1), backend);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Opens an in-canvas confirm/alert/prompt dialog built from `title`/
    /// `body`/`buttons`/`severity`, painting it immediately so
    /// `screen()`/`screen_contains()` reflect it without a separate render
    /// step — the "one call" shape `show_context_menu` already has, reusing
    /// the same `InteractionEngine`-owned anchored-overlay routing the
    /// context menu uses: `handle` checks the open dialog before the
    /// menu-bar dropdown and context-menu popup, so it intercepts every
    /// `KeyPressed`/`MouseDown` while open — opening a dialog also closes
    /// an already-open dropdown/popup, so there is nothing left underneath
    /// to contend for those two event kinds. `MouseUp`, `MouseMoved`,
    /// wheel and `CharTyped` still fall through to the widgets behind the
    /// dialog, since [`quadraui::compose::MessageDialogController::handle`]
    /// only looks at `KeyPressed`/`MouseDown`.
    ///
    /// `buttons` is a non-empty list of dicts shaped like the in-canvas
    /// `Dialog` primitive's own `buttons` field: `{"id": str, "label": str,
    /// "is_default": bool, "is_cancel": bool}` (`is_default`/`is_cancel`
    /// default to `False`). `tint` is accepted for shape compatibility with
    /// `Dialog.buttons` but not yet honored by this surface — passing a
    /// non-`None` `tint` raises `InvalidDescriptorError` rather than
    /// silently dropping it. `severity` is one of `"info"`, `"question"`,
    /// `"warning"`, `"error"`, or `None` for no severity tint.
    ///
    /// Left/Right cycle keyboard focus among the buttons; Enter activates
    /// the focused one (seeded on the first `is_default` button, or the
    /// first button); Escape resolves to the first `is_cancel` button,
    /// falling back to the first `is_default` one. Either way — and a
    /// click on any button — dispatches `UiEvent::MessageDialogResolved(id)`
    /// to the registered handler and closes the dialog, the same round
    /// trip `show_context_menu_then_clicking_an_item_emits_context_menu_item_activated`
    /// exercises for a context menu. A click anywhere else on the dialog
    /// (or outside it) is swallowed rather than falling through to
    /// whatever is behind it — [`Dialog`][quadraui::Dialog]'s own "modal
    /// overlay — intercept all clicks" contract. Tab/Shift+Tab do **not**
    /// reach the dialog's own focus cycling under `QuiApp`: both are
    /// claimed upstream, before `handle` ever runs, by the generic
    /// tab-stop intercept whenever any widget behind the dialog is
    /// focusable (`tab_stops()` reflects those widgets, never the dialog's
    /// buttons) — use Left/Right instead.
    ///
    /// Calling this again while a dialog from an earlier call is still
    /// open replaces it with the new one silently — `MessageDialogResolved`
    /// does not fire for the one it replaced. Call `dismiss_message_dialog`
    /// to close the open dialog without resolving it at all.
    ///
    /// Call from top-level script code only, never from inside the
    /// registered event handler or render callback — see
    /// `show_context_menu`'s own doc for why (the same PyO3 runtime-borrow
    /// wall applies here).
    ///
    /// This is the modal `Dialog` corner of the in-canvas overlay family
    /// only. Tooltip/Completions/Float/RichTextPopup have no binding
    /// surface yet, and the non-modal `Toast` overlay (app-owned toast
    /// list, keyboard-focus cursor rather than a blocking popup) needs a
    /// differently-shaped surface than this "one call, blocks input
    /// until resolved" method and isn't wired here either.
    #[pyo3(signature = (title, body, buttons, severity=None))]
    fn show_message_dialog(
        &mut self,
        title: String,
        body: String,
        buttons: &Bound<'_, PyList>,
        severity: Option<String>,
    ) -> PyResult<()> {
        let buttons: Vec<DialogButton> =
            pythonize::depythonize(buttons.as_any()).map_err(|err| {
                InvalidDescriptorError::new_err(format!(
                    "show_message_dialog: invalid buttons: {err}"
                ))
            })?;
        let severity = severity
            .map(|s| parse_dialog_severity("show_message_dialog", &s))
            .transpose()?;
        let opts =
            message_dialog_options_from_buttons(title, body, buttons, severity).map_err(|err| {
                InvalidDescriptorError::new_err(format!("show_message_dialog: {err}"))
            })?;
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            app_logic.engine.open_message_dialog(opts, backend);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the open message dialog (if any) without resolving it —
    /// `UiEvent::MessageDialogResolved` does not fire. Returns whether a
    /// dialog was open to close. The only way to clear a dialog from
    /// top-level script code without driving it through input; mirrors
    /// `show_message_dialog`'s own "call from top-level script code only"
    /// restriction and re-renders so `screen()` reflects the dismissal
    /// immediately.
    fn dismiss_message_dialog(&mut self) -> PyResult<bool> {
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let closed = app_logic.engine.close_message_dialog(backend);
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }

    // ── Remaining in-canvas overlays: Toast / Tooltip / Completions /
    // Float / RichTextPopup ────────────────────────────────────────────
    //
    // Every surface below follows `show_message_dialog`'s own shape: a
    // plain dict decoded straight into the matching `quadraui` primitive
    // (`decode::<Toast>`, etc. — see each primitive's own `Deserialize`
    // derive), routed onto `InteractionEngine`'s own anchored-overlay
    // slot, re-rendered immediately so `screen()`/`screen_contains()`
    // reflect the open overlay without a separate step. Call each from
    // top-level script code only, never from inside the registered event
    // handler or render callback — same PyO3 runtime-borrow wall
    // `show_message_dialog`'s own doc explains.

    /// Shows `toast` (a dict matching [`quadraui::Toast`]'s shape — `id`,
    /// `title`, `body`, `severity`, `actions`, `accent`) stacked in
    /// `corner` (one of `"bottom_right"` (default), `"bottom_left"`,
    /// `"top_right"`, `"top_left"`) among whatever toasts are already
    /// showing. Replaces any existing toast with the same `id`.
    ///
    /// A click on an action button dispatches
    /// `UiEvent::ToastActionClicked(id)` (that action's own id) to the
    /// registered handler; a click on the dismiss `×` closes the toast
    /// directly with no event. Toasts are non-modal — unlike
    /// `show_message_dialog`, they never block input to whatever is
    /// painted behind them.
    #[pyo3(signature = (toast, corner=None))]
    fn show_toast(&mut self, toast: &Bound<'_, PyDict>, corner: Option<String>) -> PyResult<()> {
        let toast: Toast = decode("show_toast", toast)?;
        let corner = corner
            .map(|c| parse_toast_corner("show_toast", &c))
            .transpose()?;
        self.guarded(|app| {
            app.driver
                .app_mut()
                .engine
                .show_toast(toast, corner.unwrap_or_default());
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the toast with this id, if showing. Returns whether one was
    /// closed. No event is dispatched — mirrors `dismiss_message_dialog`'s
    /// "close without resolving" shape.
    fn dismiss_toast(&mut self, id: String) -> PyResult<bool> {
        self.guarded(|app| {
            let closed = app.driver.app_mut().engine.dismiss_toast(WidgetId::new(id));
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }

    /// Shows `tooltip` (a dict matching [`quadraui::Tooltip`]'s shape —
    /// `id`, `text`, `styled_lines`, `placement`, `bg`, `fg`) anchored
    /// against `anchor`, an `(x, y, width, height)` rect in this app's
    /// own cell grid. Replaces any tooltip already showing.
    ///
    /// Tooltips are non-interactive — see
    /// [`quadraui::Backend::draw_tooltip`]'s own doc — so there is no
    /// click/selection event to report. The next `MouseDown` or
    /// `KeyPressed` this app handles (through `step`/`step_once`/`run`)
    /// auto-dismisses it; call `hide_tooltip` to dismiss it sooner (e.g.
    /// the hovered widget itself changed).
    fn show_tooltip(
        &mut self,
        tooltip: &Bound<'_, PyDict>,
        anchor: (f32, f32, f32, f32),
    ) -> PyResult<()> {
        let tooltip: Tooltip = decode("show_tooltip", tooltip)?;
        let anchor = Rect::new(anchor.0, anchor.1, anchor.2, anchor.3);
        self.guarded(|app| {
            app.driver.app_mut().engine.show_tooltip(tooltip, anchor);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Hides the open tooltip (if any). Returns whether one was open.
    fn hide_tooltip(&mut self) -> PyResult<bool> {
        self.guarded(|app| {
            let hidden = app.driver.app_mut().engine.hide_tooltip();
            if hidden {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(hidden)
        })
    }

    /// Shows `completions` (a dict matching [`quadraui::Completions`]'s
    /// shape — `id`, `items`, `selected_idx`, `scroll_offset`,
    /// `has_focus`) anchored at `cursor`, an `(x, y)` caret position in
    /// this app's own cell grid. Replaces any popup already open.
    ///
    /// A click on an item dispatches
    /// `UiEvent::CompletionItemActivated(id, index)` (the popup's own id
    /// + the chosen item's index into `items`) and closes the popup;
    /// Escape, or a click outside it, closes it with no event.
    fn show_completions(
        &mut self,
        completions: &Bound<'_, PyDict>,
        cursor: (f32, f32),
    ) -> PyResult<()> {
        let completions: Completions = decode("show_completions", completions)?;
        let cursor = Point::new(cursor.0, cursor.1);
        self.guarded(|app| {
            app.driver
                .app_mut()
                .engine
                .show_completions(completions, cursor);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the open completions popup (if any) without reporting a
    /// selection. Returns whether one was open.
    fn dismiss_completions(&mut self) -> PyResult<bool> {
        self.guarded(|app| {
            let closed = app.driver.app_mut().engine.dismiss_completions();
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }

    /// Opens `float` (a dict matching [`quadraui::Float`]'s shape — `id`,
    /// `anchor`, `focusable`, `border`, `bg`; see that type's own doc for
    /// `anchor`'s shape) at `width`x`height` cells.
    ///
    /// **Chrome only** — same contract as
    /// [`quadraui::Backend::draw_float`] itself: this paints the float's
    /// background/border, nothing else. There is no content surface on
    /// this binding yet for a host to paint into the resolved
    /// `content_bounds` — a future issue's concern, not this one's; see
    /// `quadraui::Float`'s own module doc for the primitive's full
    /// content contract.
    ///
    /// Escape closes a focusable float (`float.focusable`, `true` by
    /// default) with no event; a non-focusable hint float never claims
    /// Escape, matching [`quadraui::ModalStack::top_focusable`]'s own
    /// "never steals keys" doc.
    fn show_float(&mut self, float: &Bound<'_, PyDict>, width: f32, height: f32) -> PyResult<()> {
        let float: Float = decode("show_float", float)?;
        let measure = FloatMeasure::new(width, height);
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            app_logic.engine.show_float(float, measure, backend);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the open float (if any) with no event. Returns whether one
    /// was open.
    fn dismiss_float(&mut self) -> PyResult<bool> {
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let closed = app_logic.engine.dismiss_float(backend);
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
        })
    }

    /// Shows `popup` (a dict matching [`quadraui::RichTextPopup`]'s shape
    /// — `id`, `lines`, `line_text`, `max_visible_rows`, `links`, …)
    /// anchored at `anchor`, an `(x, y)` position in this app's own cell
    /// grid — typically the top-left of the cell the popup describes.
    /// Replaces any popup already open.
    ///
    /// A click on a link dispatches
    /// `UiEvent::RichTextLinkClicked(id, url)` (the popup's own id + the
    /// clicked [`quadraui::RichTextLink::url`]); Escape, or a click
    /// outside the popup, closes it with no event.
    fn show_rich_text_popup(
        &mut self,
        popup: &Bound<'_, PyDict>,
        anchor: (f32, f32),
    ) -> PyResult<()> {
        let popup: RichTextPopup = decode("show_rich_text_popup", popup)?;
        let anchor = Point::new(anchor.0, anchor.1);
        self.guarded(|app| {
            app.driver
                .app_mut()
                .engine
                .show_rich_text_popup(popup, anchor);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(())
        })
    }

    /// Closes the open rich-text popup (if any) without reporting a link
    /// click. Returns whether one was open.
    fn dismiss_rich_text_popup(&mut self) -> PyResult<bool> {
        self.guarded(|app| {
            let closed = app.driver.app_mut().engine.dismiss_rich_text_popup();
            if closed {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(closed)
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

    // ─── Metrics & capabilities ─────────────────────────────────────────
    //
    // Every method below reads this `QuiApp`'s own headless `TuiBackend` --
    // the same one `step`/`run`/`screen` always render against, regardless
    // of whether `run_native` currently has a native window open (see the
    // `driver` field's doc on `run_tui`/`run_native` not sharing a driver).
    // A layout computation that calls these alongside `insert_*`/the
    // render callback -- both of which only ever run against this same
    // headless backend too -- gets numbers that agree with what `screen`/
    // `screen_contains` actually show, on every platform this binding
    // builds for.

    /// This app's current viewport as `(width, height, scale)`, in cells
    /// -- [`Backend::viewport`]. `scale` is always `1.0` on the headless
    /// `TuiBackend`. Matches the `width`/`height` this app was constructed
    /// (or last `resize`d) with.
    fn viewport(&mut self) -> PyResult<(f32, f32, f32)> {
        self.guarded(|app| {
            let vp = app.driver.backend().viewport();
            Ok((vp.width, vp.height, vp.scale))
        })
    }

    /// Height of one standard text row, in the headless backend's native
    /// units -- [`Backend::line_height`]. Always `1.0` (one terminal
    /// cell); a layout computation multiplies this instead of hardcoding
    /// a row height, so the same arithmetic reads correctly against any
    /// backend's real metrics.
    fn line_height(&mut self) -> PyResult<f32> {
        self.guarded(|app| Ok(app.driver.backend().line_height()))
    }

    /// Approximate monospace character width, in the headless backend's
    /// native units -- [`Backend::char_width`]. Always `1.0` (one
    /// terminal cell).
    fn char_width(&mut self) -> PyResult<f32> {
        self.guarded(|app| Ok(app.driver.backend().char_width()))
    }

    /// The advance `draw_list` actually paints `ListView` row text with --
    /// [`Backend::list_char_width`]. Not always the same as `char_width`
    /// on a pixel backend, whose chrome font can differ from its editor
    /// font; use this instead of `char_width` when budgeting text for a
    /// list row.
    fn list_char_width(&mut self) -> PyResult<f32> {
        self.guarded(|app| Ok(app.driver.backend().list_char_width()))
    }

    /// Line height of the chrome / UI font -- [`Backend::chrome_line_height`].
    /// Equal to `line_height` on the headless backend (one font for the
    /// whole cell grid); use this instead of `line_height` when sizing
    /// chrome rows (toolbars, status bars, tab strips) on a backend with
    /// a separate chrome font.
    fn chrome_line_height(&mut self) -> PyResult<f32> {
        self.guarded(|app| Ok(app.driver.backend().chrome_line_height()))
    }

    /// Width this backend reserves for its own native scrollbar overlay
    /// alongside scrollable content -- [`Backend::scrollbar_reserve`].
    /// Always `0.0` on the headless backend, which has no overlay
    /// scrollbar chrome.
    fn scrollbar_reserve(&mut self) -> PyResult<f32> {
        self.guarded(|app| Ok(app.driver.backend().scrollbar_reserve()))
    }

    /// Whether this backend can render individual text rows at a larger
    /// font size -- [`Backend::scales_text_rows`]. Always `False` on the
    /// headless backend, a fixed-cell grid where a terminal cell can't
    /// grow.
    fn scales_text_rows(&mut self) -> PyResult<bool> {
        self.guarded(|app| Ok(app.driver.backend().scales_text_rows()))
    }

    /// Snaps a proposed height, in the same units as `line_height`, to
    /// what this backend will actually paint -- [`Backend::snap_height`].
    /// Identity on the headless backend, whose cell grid has no sub-row
    /// resolution to snap away.
    fn snap_height(&mut self, h: f32) -> PyResult<f32> {
        self.guarded(|app| Ok(app.driver.backend().snap_height(h)))
    }

    /// Real `(width, height)`, in the headless backend's native units, of
    /// `text` rendered in `role`'s font -- [`Backend::measure_text`].
    /// `role` is `"chrome"` (the default) or `"editor"`; the headless
    /// backend returns the same cell-count width (height always `1.0`)
    /// for either, since a terminal has one font by definition -- see
    /// [`FontRole`]'s doc. Raises `InvalidFontRoleError` for any other
    /// `role` value.
    #[pyo3(signature = (text, role="chrome"))]
    fn measure_text(&mut self, text: &str, role: &str) -> PyResult<(f32, f32)> {
        let role = font_role(role)
            .map_err(|err| InvalidFontRoleError::new_err(format!("measure_text: {err}")))?;
        self.guarded(|app| Ok(app.driver.backend().measure_text(text, role)))
    }

    /// This backend's declared capabilities -- [`Backend::backend_caps`]
    /// -- as a `dict` of capability name to whether it's declared. Covers
    /// every name in [`BackendCaps::vocabulary`] (`"mouse"`, `"scroll"`,
    /// `"drag"`, `"text_selection"`, `"native_menu"`, `"window_chrome"`,
    /// `"pointer_cursor"`, `"ime"`, `"file_dialogs"`, `"folder_dialogs"`,
    /// `"native_dialogs"`, `"notifications"`, `"app_font_registration"`,
    /// `"window_control"`, `"tray"`, `"generic_font_families"`) plus three
    /// fields that vocabulary excludes because they aren't plain
    /// capability-presence booleans: `"kitty_keyboard"` and
    /// `"sgr_pixel_mouse"` (bools, terminal input-protocol support) and
    /// `"color_depth"` (one of `"ansi16"`, `"indexed256"`, `"true_color"`).
    fn backend_caps(&mut self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        self.guarded(|app| {
            let caps = app.driver.backend().backend_caps();
            let dict = PyDict::new(py);
            for name in BackendCaps::vocabulary() {
                dict.set_item(name, caps.has(name))?;
            }
            dict.set_item("kitty_keyboard", caps.kitty_keyboard)?;
            dict.set_item("sgr_pixel_mouse", caps.sgr_pixel_mouse)?;
            dict.set_item("color_depth", color_depth_name(caps.color_depth))?;
            Ok(dict.unbind())
        })
    }

    // ─── Accelerators & global shortcuts ─────────────────────────────────

    /// Registers `descriptor` (decoded as an [`Accelerator`]) so a
    /// matching native key event fires `UiEvent::Accelerator(id,
    /// modifiers)` (see `push_event`'s `{"Accelerator": [id, modifiers]}`
    /// shape) instead of a plain `KeyPressed`. Re-registering the same
    /// `descriptor["id"]` replaces its previous binding/scope/label
    /// wholesale.
    ///
    /// `descriptor` requires all four keys -- `"id"`, `"binding"`,
    /// `"scope"`, and `"label"` (pass `None` for no display label; it is
    /// not defaulted, so omitting the key raises `InvalidDescriptorError`
    /// rather than falling back to `None`).
    ///
    /// Only `{"scope": "Global"}` actually fires today --
    /// [`Backend::register_accelerator`]'s matcher
    /// (`BackendCore::match_keypress`) skips any accelerator whose scope
    /// is `Widget`/`Mode`, since the backend has no focus/mode context of
    /// its own to resolve those against. A descriptor with a non-`Global`
    /// scope is accepted without error and then silently never fires.
    ///
    /// Takes effect immediately on this app's own live backend -- a
    /// chord pushed via `push_event` right after this call (not
    /// `push_key_event`, which hardcodes no modifiers and so can only
    /// ever match an unmodified binding) fires on the very next
    /// `step`/`run` -- and is also replayed onto whichever real backend
    /// `run_tui`/`run_native` goes on to construct, via this app's
    /// [`AppLogic::setup`] impl, so a chord registered before either
    /// call still fires once the terminal/window is live.
    fn register_accelerator(&mut self, descriptor: &Bound<'_, PyDict>) -> PyResult<()> {
        self.guarded(|app| {
            let acc: Accelerator = decode("register_accelerator", descriptor)?;
            let (backend, app_logic) = app.driver.parts_mut();
            backend.register_accelerator(&acc);
            app_logic.host.accelerators.insert(acc.id.clone(), acc);
            Ok(())
        })
    }

    /// Removes a previously registered accelerator --
    /// [`Backend::unregister_accelerator`]. A no-op if `id` was never
    /// registered, or was already unregistered.
    fn unregister_accelerator(&mut self, id: &str) -> PyResult<()> {
        self.guarded(|app| {
            let id = AcceleratorId::new(id);
            let (backend, app_logic) = app.driver.parts_mut();
            backend.unregister_accelerator(&id);
            app_logic.host.accelerators.remove(&id);
            Ok(())
        })
    }

    /// Registers `descriptor` (decoded as an [`Accelerator`]) as an
    /// OS-wide global shortcut --
    /// [`quadraui::PlatformServices::register_global_shortcut`] -- fired
    /// as `UiEvent::GlobalShortcut(id)` even while this app isn't
    /// focused. `descriptor["scope"]` is ignored: a global shortcut is
    /// OS-wide by construction, see
    /// `quadraui::accelerator::GlobalShortcutRegistry::register`'s doc.
    /// Returns an opaque handle (a plain Python `int`) to pass back to
    /// [`Self::unregister_global_shortcut`].
    ///
    /// Resolves [`quadraui::PlatformServices`] off this `QuiApp`'s own
    /// headless `TuiBackend` -- the one `step`/`run` drive directly --
    /// never off whatever backend a `run_native` call opened; `run_native`
    /// moves this app's `PyAppLogic` into a brand-new GTK/macOS/Win
    /// backend by value and has no handle back to it for this method to
    /// resolve services against. This is headless-only, independent of
    /// whether `run_native`'s window is currently open. It makes no
    /// practical difference today, since every in-tree
    /// `PlatformServices` override inherits
    /// [`quadraui::PlatformServices::register_global_shortcut`]'s
    /// `Unsupported` default regardless of which backend resolves it --
    /// but the day a backend (GTK's `org.freedesktop.portal.
    /// GlobalShortcuts`, Win32 `RegisterHotKey`, Cocoa
    /// `RegisterEventHotKey`) implements a real OS hotkey call, this
    /// method still won't reach it while a native window is open. This is
    /// a seam a Python app can already call against; the success path has
    /// no automated coverage here and needs manual verification once a
    /// backend implements one for real.
    fn register_global_shortcut(
        &mut self,
        py: Python<'_>,
        descriptor: &Bound<'_, PyDict>,
    ) -> PyResult<Py<PyAny>> {
        self.guarded(|app| {
            let acc: Accelerator = decode("register_global_shortcut", descriptor)?;
            let id = app
                .driver
                .backend()
                .services()
                .register_global_shortcut(acc)
                .map_err(|err| {
                    InternalError::new_err(format!("register_global_shortcut: {err:?}"))
                })?;
            pythonize::pythonize(py, &id)
                .map(|v| v.unbind())
                .map_err(|err| InternalError::new_err(format!("register_global_shortcut: {err}")))
        })
    }

    /// Releases a shortcut returned by [`Self::register_global_shortcut`]
    /// -- [`quadraui::PlatformServices::unregister_global_shortcut`]. See
    /// that method's doc for why this is headless-only and raises
    /// `InternalError` on every in-tree backend.
    fn unregister_global_shortcut(&mut self, py: Python<'_>, id: Py<PyAny>) -> PyResult<()> {
        self.guarded(|app| {
            let id: GlobalShortcutId = pythonize::depythonize(id.bind(py)).map_err(|err| {
                InvalidDescriptorError::new_err(format!("unregister_global_shortcut: {err}"))
            })?;
            app.driver
                .backend()
                .services()
                .unregister_global_shortcut(id)
                .map_err(|err| {
                    InternalError::new_err(format!("unregister_global_shortcut: {err:?}"))
                })
        })
    }

    /// Releases every currently registered global shortcut --
    /// [`quadraui::PlatformServices::unregister_all_global_shortcuts`].
    /// See [`Self::register_global_shortcut`]'s doc for why this is
    /// headless-only and raises `InternalError` on every in-tree backend.
    fn unregister_all_global_shortcuts(&mut self) -> PyResult<()> {
        self.guarded(|app| {
            app.driver
                .backend()
                .services()
                .unregister_all_global_shortcuts()
                .map_err(|err| {
                    InternalError::new_err(format!("unregister_all_global_shortcuts: {err:?}"))
                })
        })
    }

    // ─── Theme, style & fonts ────────────────────────────────────────────

    /// This app's active [`quadraui::Theme`] -- [`Backend::theme`] -- as a
    /// `dict` with one entry per `Theme` field, each colour itself a
    /// `{"r":.., "g":.., "b":.., "a":..}` dict. Pair with [`Self::set_theme`]
    /// to read a field back after overriding only some of its neighbours.
    fn theme(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.guarded(|app| {
            let theme = app.driver.backend().theme();
            pythonize::pythonize(py, &theme)
                .map(|v| v.unbind())
                .map_err(|err| InternalError::new_err(format!("theme: {err}")))
        })
    }

    /// Overrides one or more [`quadraui::Theme`] fields on this app's
    /// backend -- [`Backend::set_theme`] -- merged onto whatever the
    /// backend's current theme already has, so a caller only needs to name
    /// the fields it actually wants to change rather than restating all
    /// ~70. `overrides` keys must be real `Theme` field names (raises
    /// `InvalidDescriptorError` otherwise); each value is a
    /// `"#rrggbb"`/`"#rrggbbaa"` hex string, an `[r, g, b[, a]]` list/tuple,
    /// or a `{"r":.., "g":.., "b":.. [, "a":..]}` dict.
    ///
    /// Takes effect immediately on this app's own live backend, and is
    /// also replayed onto whichever real backend `run_tui`/`run_native`
    /// goes on to construct, via [`AppLogic::setup`] -- mirrors
    /// [`Self::register_accelerator`]'s replay contract.
    fn set_theme(&mut self, overrides: &Bound<'_, PyDict>) -> PyResult<()> {
        let keys = dict_keys(overrides)?;
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let theme = merge_overrides(backend.theme(), keys, |key| {
                let val = overrides
                    .get_item(key)
                    .map_err(|err| err.to_string())?
                    .expect("key was read from overrides.keys() above");
                theme_color_value(key, pyany_to_json(&val)?)
            })
            .map_err(|err| override_error_to_py("set_theme", err))?;
            backend.set_theme(theme);
            app_logic.host.theme = Some(theme);
            Ok(())
        })
    }

    /// This app's active [`quadraui::Style`] -- [`Backend::style`] -- as a
    /// `dict` with one entry per token (`focus_ring_width`, `padding`,
    /// `corner_radius`, `border_width`, `shadow_elevation`,
    /// `control_height`, `backdrop_opacity`).
    fn style(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.guarded(|app| {
            let style = app.driver.backend().style();
            pythonize::pythonize(py, &style)
                .map(|v| v.unbind())
                .map_err(|err| InternalError::new_err(format!("style: {err}")))
        })
    }

    /// Overrides one or more [`quadraui::Style`] tokens --
    /// [`Backend::set_style`] -- merged onto the backend's current style
    /// the same way [`Self::set_theme`] merges `Theme` fields. `overrides`
    /// keys must be real `Style` field names. Replayed onto a later
    /// `run_tui`/`run_native` backend the same way `set_theme` is.
    fn set_style(&mut self, overrides: &Bound<'_, PyDict>) -> PyResult<()> {
        let keys = dict_keys(overrides)?;
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let style = merge_overrides(backend.style(), keys, |key| {
                let val = overrides
                    .get_item(key)
                    .map_err(|err| err.to_string())?
                    .expect("key was read from overrides.keys() above");
                pyany_to_json(&val)
            })
            .map_err(|err| override_error_to_py("set_style", err))?;
            backend.set_style(style);
            app_logic.host.style = Some(style);
            Ok(())
        })
    }

    /// Syncs the nerd-fonts flag -- [`Backend::set_nerd_fonts`] -- so
    /// icon-bearing surfaces render `Icon::glyph` when `enabled` rather
    /// than `Icon::fallback`. Replayed onto a later `run_tui`/`run_native`
    /// backend the same way `set_theme` is.
    fn set_nerd_fonts(&mut self, enabled: bool) -> PyResult<()> {
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            backend.set_nerd_fonts(enabled);
            app_logic.host.nerd_fonts = Some(enabled);
            Ok(())
        })
    }

    /// Whatever was last handed to [`Self::set_nerd_fonts`] --
    /// [`Backend::nerd_fonts_enabled`].
    fn nerd_fonts_enabled(&mut self) -> PyResult<bool> {
        self.guarded(|app| Ok(app.driver.backend().nerd_fonts_enabled()))
    }

    /// Overrides the font that paints editor content --
    /// [`Backend::set_editor_font`]. No-op on the headless `TuiBackend`
    /// (a fixed-cell backend has no font concept), but still recorded and
    /// replayed onto a later `run_tui`/`run_native` backend the same way
    /// `set_theme` is -- so a GTK/macOS/Win-GUI window opened afterwards
    /// picks it up.
    fn set_editor_font(&mut self, family: &str, size_pt: f32) -> PyResult<()> {
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            backend.set_editor_font(family, size_pt);
            app_logic.host.editor_font = Some((family.to_string(), size_pt));
            Ok(())
        })
    }

    /// Overrides the font that paints chrome -- [`Backend::set_ui_font`].
    /// Same no-op-on-TUI-but-still-replayed contract as
    /// [`Self::set_editor_font`].
    fn set_ui_font(&mut self, font_desc: &str) -> PyResult<()> {
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            backend.set_ui_font(font_desc);
            app_logic.host.ui_font = Some(font_desc.to_string());
            Ok(())
        })
    }

    /// Registers `bytes` (raw TTF/OTF data) with the platform font manager
    /// -- [`Backend::register_font_from_memory`]. Returns the family
    /// name(s) it registered under, or `None` on the headless `TuiBackend`
    /// (a fixed-cell backend has no font concept) or if `bytes` isn't a
    /// font this backend's platform font manager can parse. Recorded and
    /// re-registered onto a later `run_tui`/`run_native` backend the same
    /// way `set_theme` is.
    fn register_font_from_memory(&mut self, bytes: Vec<u8>) -> PyResult<Option<Vec<String>>> {
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let families = backend.register_font_from_memory(&bytes);
            app_logic.host.registered_fonts.push(bytes);
            Ok(families)
        })
    }

    /// Sets the fallback font family consulted when the primary font can't
    /// cover a character -- [`Backend::set_nerd_font_fallback`]. Same
    /// no-op-on-TUI-but-still-replayed contract as
    /// [`Self::set_editor_font`].
    fn set_nerd_font_fallback(&mut self, family: &str) -> PyResult<()> {
        self.guarded(|app| {
            let (backend, app_logic) = app.driver.parts_mut();
            backend.set_nerd_font_fallback(family);
            app_logic.host.nerd_font_fallback = Some(family.to_string());
            Ok(())
        })
    }

    /// Whether `family` is already installed on this platform as a system
    /// font -- [`Backend::has_font_family`]. Always `None` on the headless
    /// `TuiBackend` -- see that method's own doc for why `None` means
    /// "don't know", never "false".
    fn has_font_family(&mut self, family: &str) -> PyResult<Option<bool>> {
        self.guarded(|app| Ok(app.driver.backend().has_font_family(family)))
    }

    /// Steers the hardware caret to `shape` (`"block"`, `"bar"`, or
    /// `"underline"`) -- [`Backend::set_caret_shape`]. On the headless
    /// `TuiBackend` this always writes a DECSCUSR escape sequence to real
    /// stdout regardless of whether a real terminal is attached (see that
    /// method's own doc) -- harmless under `pytest`'s captured stdout, but
    /// not something a driver test can observe landing anywhere.
    fn set_caret_shape(&mut self, shape: &str) -> PyResult<()> {
        let shape = caret_shape(shape)
            .map_err(|err| InvalidDescriptorError::new_err(format!("set_caret_shape: {err}")))?;
        self.guarded(|app| {
            app.driver.parts_mut().0.set_caret_shape(shape);
            Ok(())
        })
    }

    /// RGB colour painted at one headless-backend cell, read back as
    /// `((fg_r, fg_g, fg_b), (bg_r, bg_g, bg_b))` --
    /// [`quadraui::tui::testing::TuiDriver::style_at`]. `None` if `(x, y)`
    /// is outside the current viewport. Exists so a test can prove
    /// `set_theme`/`set_style` actually changed what gets painted, not
    /// just what `theme()`/`style()` read back -- every in-tree rasteriser
    /// paints an RGB colour via `quadraui::tui::ratatui_color`, so the
    /// `(0, 0, 0)` fallback below is purely defensive.
    fn cell_colors(&mut self, x: u16, y: u16) -> PyResult<Option<CellColors>> {
        self.guarded(|app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            let Some(style) = app.driver.style_at(x, y) else {
                return Ok(None);
            };
            Ok(Some((ratatui_rgb(style.fg), ratatui_rgb(style.bg))))
        })
    }

    // ─── Platform services ───────────────────────────────────────────────
    //
    // Every method below resolves `quadraui::PlatformServices` off this
    // `QuiApp`'s own headless `TuiBackend` -- the same "never off whatever
    // backend a `run_native` call opened" restriction
    // `register_global_shortcut`'s doc above explains, for the same reason:
    // `run_native` moves this app's `PyAppLogic` into a brand-new GTK/macOS/
    // Win backend by value and leaves this `QuiApp` no handle back to it.
    // `TuiPlatformServices` (`quadraui::tui::services`) is a genuine,
    // non-stub implementation for every method here -- see that module's
    // doc for exactly what each one does on a terminal (a real opener
    // shell-out, a real BEL, a nested draw-and-read loop over a `compose`
    // controller for the three dialogs/picker).

    /// Queues scripted input for the next nested dialog/picker loop a
    /// `show_file_open_dialog`/`show_file_save_dialog`/`show_folder_dialog`
    /// call drives -- [`quadraui::tui::testing::TuiDriver::queue_dialog_events`].
    /// `events` decodes the same way `push_event`'s single dict does, one
    /// [`UiEvent`] per list entry. With nothing queued, the very first
    /// event the nested loop synthesizes is Escape, so an un-driven dialog
    /// call resolves to `None` (cancelled) rather than hanging -- queue a
    /// script here first when a test wants a real confirmed path/choice
    /// instead.
    fn queue_dialog_events(&mut self, events: &Bound<'_, PyList>) -> PyResult<()> {
        let events: Vec<UiEvent> = pythonize::depythonize(events.as_any()).map_err(|err| {
            InvalidDescriptorError::new_err(format!(
                "queue_dialog_events: invalid descriptor: {err}"
            ))
        })?;
        self.guarded(|app| {
            app.driver.queue_dialog_events(events);
            Ok(())
        })
    }

    /// Opens a file-open picker -- [`quadraui::PlatformServices::show_file_open_dialog`].
    /// On the headless `TuiBackend` this drives
    /// [`quadraui::compose::FilePickerController`] through a nested
    /// draw-and-read loop fed by [`Self::queue_dialog_events`] (or Escape
    /// immediately, with nothing queued). Returns the confirmed path, or
    /// `None` on cancel. `filters` is a list of `(label, [ext, ...])`
    /// pairs, e.g. `[("Rust", ["rs"])]`.
    #[pyo3(signature = (title=None, initial_dir=None, filters=None))]
    fn show_file_open_dialog(
        &mut self,
        title: Option<String>,
        initial_dir: Option<String>,
        filters: Option<Vec<(String, Vec<String>)>>,
    ) -> PyResult<Option<String>> {
        self.guarded(|app| {
            let opts = FileDialogOptions {
                title,
                initial_dir: initial_dir.map(PathBuf::from),
                initial_filename: None,
                filters: filters.unwrap_or_default(),
            };
            Ok(app
                .driver
                .backend()
                .services()
                .show_file_open_dialog(opts)
                .map(|p| p.display().to_string()))
        })
    }

    /// Opens a file-save picker -- [`quadraui::PlatformServices::show_file_save_dialog`].
    /// Same nested-loop/`queue_dialog_events` story as
    /// [`Self::show_file_open_dialog`]; `initial_filename` seeds the
    /// picker's query field.
    #[pyo3(signature = (title=None, initial_dir=None, initial_filename=None, filters=None))]
    fn show_file_save_dialog(
        &mut self,
        title: Option<String>,
        initial_dir: Option<String>,
        initial_filename: Option<String>,
        filters: Option<Vec<(String, Vec<String>)>>,
    ) -> PyResult<Option<String>> {
        self.guarded(|app| {
            let opts = FileDialogOptions {
                title,
                initial_dir: initial_dir.map(PathBuf::from),
                initial_filename,
                filters: filters.unwrap_or_default(),
            };
            Ok(app
                .driver
                .backend()
                .services()
                .show_file_save_dialog(opts)
                .map(|p| p.display().to_string()))
        })
    }

    /// Opens a directory picker -- [`quadraui::PlatformServices::show_folder_open_dialog`].
    /// Same nested-loop/`queue_dialog_events` story as
    /// [`Self::show_file_open_dialog`], but drives
    /// [`quadraui::compose::FolderPickerController`] instead of
    /// `FilePickerController`.
    #[pyo3(signature = (title=None, initial_dir=None))]
    fn show_folder_dialog(
        &mut self,
        title: Option<String>,
        initial_dir: Option<String>,
    ) -> PyResult<Option<String>> {
        self.guarded(|app| {
            let opts = FileDialogOptions {
                title,
                initial_dir: initial_dir.map(PathBuf::from),
                initial_filename: None,
                filters: Vec::new(),
            };
            Ok(app
                .driver
                .backend()
                .services()
                .show_folder_open_dialog(opts)
                .map(|p| p.display().to_string()))
        })
    }

    /// Dispatches a system notification --
    /// [`quadraui::PlatformServices::send_notification`]. On the headless
    /// `TuiBackend` this is a genuine best-effort degrade (an OSC 9/777
    /// escape sequence plus a native `osascript`/`notify-send` spawn where
    /// available), not a no-op -- see `quadraui::tui::services`'s module
    /// doc's "Notifications" section. Fire-and-forget: there is no return
    /// value to assert on, and no error this binding can observe.
    #[pyo3(signature = (title, body, urgent=false, silent=false, tag=None))]
    fn send_notification(
        &mut self,
        title: String,
        body: String,
        urgent: bool,
        silent: bool,
        tag: Option<String>,
    ) -> PyResult<()> {
        self.guarded(|app| {
            let mut n = Notification::new(title, body).with_silent(silent);
            if let Some(tag) = tag {
                n = n.with_tag(tag);
            }
            n.urgent = urgent;
            app.driver.backend().services().send_notification(n);
            Ok(())
        })
    }

    /// Opens `url` in the platform's default browser --
    /// [`quadraui::PlatformServices::open_url_result`] (the fallible twin
    /// of `open_url`, used here so a genuine failure raises instead of
    /// being silently swallowed). On the headless `TuiBackend` this shells
    /// out to `xdg-open`/`open`/`cmd /c start`, falling back to an OSC 8
    /// hyperlink -- see `quadraui::tui::services`'s module doc's "URL
    /// opening" section. Raises `InternalError` only when neither leg
    /// reached anything.
    fn open_url(&mut self, url: &str) -> PyResult<()> {
        self.guarded(|app| {
            app.driver
                .backend()
                .services()
                .open_url_result(url)
                .map_err(|err| InternalError::new_err(format!("open_url: {err:?}")))
        })
    }

    /// Reveals `path` in the platform's file manager with it selected --
    /// [`quadraui::PlatformServices::reveal_in_file_manager`]. On the
    /// headless `TuiBackend` this reaches the desktop session underneath
    /// the terminal (`open -R` / Explorer `/select,` / D-Bus `ShowItems`,
    /// falling back to opening the parent directory) -- a terminal itself
    /// has no file-manager window, but the session running it usually
    /// does. Raises `InternalError` when no such session is reachable.
    fn reveal_in_file_manager(&mut self, path: &str) -> PyResult<()> {
        self.guarded(|app| {
            app.driver
                .backend()
                .services()
                .reveal_in_file_manager(Path::new(path))
                .map_err(|err| InternalError::new_err(format!("reveal_in_file_manager: {err:?}")))
        })
    }

    /// Opens `path` with the platform's default handler for its type --
    /// [`quadraui::PlatformServices::open_path`]. Distinct from
    /// [`Self::open_url`]: this takes a filesystem path, not a URL. Same
    /// shell-out story as `open_url` on the headless `TuiBackend`. Raises
    /// `InternalError` when no opener is reachable.
    fn open_path(&mut self, path: &str) -> PyResult<()> {
        self.guarded(|app| {
            app.driver
                .backend()
                .services()
                .open_path(Path::new(path))
                .map_err(|err| InternalError::new_err(format!("open_path: {err:?}")))
        })
    }

    /// Moves `path` to the platform trash/recycle bin --
    /// [`quadraui::PlatformServices::move_to_trash`]. Genuinely full
    /// support on the headless `TuiBackend`, not a degrade -- the
    /// cross-platform `trash` crate needs only a filesystem, no live
    /// desktop session. Raises `InternalError` on a real filesystem
    /// failure.
    fn move_to_trash(&mut self, path: &str) -> PyResult<()> {
        self.guarded(|app| {
            app.driver
                .backend()
                .services()
                .move_to_trash(Path::new(path))
                .map_err(|err| InternalError::new_err(format!("move_to_trash: {err:?}")))
        })
    }

    /// Rings the terminal bell -- [`quadraui::PlatformServices::beep`].
    /// Writes BEL (`\x07`) to both stdout and (Unix) `/dev/tty`; harmless
    /// under `pytest`'s captured stdout, but not itself observable from a
    /// driver test -- a terminal bell is this method's own "manual
    /// verification" story.
    fn beep(&mut self) -> PyResult<()> {
        self.guarded(|app| {
            app.driver
                .backend()
                .services()
                .beep()
                .map_err(|err| InternalError::new_err(format!("beep: {err:?}")))
        })
    }

    /// The OS-level light/dark preference, accent colour, and
    /// high-contrast setting, as a `{"dark": bool, "accent": ... or None,
    /// "high_contrast": bool, "reduce_motion": bool}` dict --
    /// [`quadraui::PlatformServices::system_theme`]. On the headless
    /// `TuiBackend` this is an environment-only heuristic
    /// (`$COLORFGBG`, parsed the same way `quadraui::tui::caps::detect_system_theme`
    /// does) -- `accent` is always `None` and `high_contrast`/
    /// `reduce_motion` are always `False` there, since no terminal exposes
    /// either signal. Raises `InternalError` when `$COLORFGBG` is unset or
    /// unparseable -- a terminal that never sets it genuinely gives no
    /// signal to report.
    fn system_theme(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.guarded(|app| {
            let theme = app
                .driver
                .backend()
                .services()
                .system_theme()
                .map_err(|err| InternalError::new_err(format!("system_theme: {err:?}")))?;
            pythonize::pythonize(py, &theme)
                .map(|v| v.unbind())
                .map_err(|err| InternalError::new_err(format!("system_theme: {err}")))
        })
    }

    /// This app's platform identifier -- [`quadraui::PlatformServices::platform_name`].
    /// Always `"tui"` here: every method on this page resolves services
    /// off this `QuiApp`'s own headless `TuiBackend`, never a native
    /// `run_native` window (see this section's own header doc).
    fn platform_name(&mut self) -> PyResult<&'static str> {
        self.guarded(|app| Ok(app.driver.backend().services().platform_name()))
    }
}

/// Converts an arbitrary Python value into a [`serde_json::Value`] via
/// `pythonize::depythonize`. Not itself a validation of any particular
/// target type — [`merge_overrides`]'s caller runs the merged value through
/// `serde_json::from_value::<T>` afterwards, which is where a type mismatch
/// (e.g. a string where `Style::padding` wants a number) actually surfaces.
/// The `Err` message carries no `what`/field context of its own —
/// [`override_error_to_py`] adds that once, uniformly, for every
/// [`OverrideError`] variant [`merge_overrides`] can produce.
fn pyany_to_json(value: &Bound<'_, PyAny>) -> Result<serde_json::Value, String> {
    pythonize::depythonize(value).map_err(|err| format!("invalid value: {err}"))
}

/// Maps a [`merge_overrides`] failure to this binding's own exception
/// taxonomy, prefixed with `what` (the calling method's own name) exactly
/// once — every [`OverrideError`] variant's `Display` already omits that
/// context so it is never doubled up.
fn override_error_to_py(what: &str, err: OverrideError) -> PyErr {
    match err {
        OverrideError::Internal(_) => InternalError::new_err(format!("{what}: {err}")),
        OverrideError::UnknownField(_)
        | OverrideError::InvalidValue(_)
        | OverrideError::Invalid(_) => InvalidDescriptorError::new_err(format!("{what}: {err}")),
    }
}

/// `overrides`' own keys, as `String`s, in iteration order —
/// [`merge_overrides`]'s `keys` argument, decoded once so its per-key
/// `value_for` closure can look each one's value back up in `overrides`
/// only after [`merge_overrides`] has confirmed that key actually names a
/// field of `T`.
fn dict_keys(overrides: &Bound<'_, PyDict>) -> PyResult<Vec<String>> {
    overrides.keys().iter().map(|k| k.extract()).collect()
}

/// `((fg_r, fg_g, fg_b), (bg_r, bg_g, bg_b))` -- [`QuiApp::cell_colors`]'s
/// return shape, named so the signature doesn't trip clippy's
/// `type_complexity` lint.
type CellColors = ((u8, u8, u8), (u8, u8, u8));

/// Extracts `(r, g, b)` from a [`quadraui::tui::testing::Color`] --
/// `(0, 0, 0)` for any non-`Rgb` variant, which no in-tree rasteriser ever
/// paints (every one goes through `quadraui::tui::ratatui_color`, which
/// only ever constructs `Rgb`).
fn ratatui_rgb(color: quadraui::tui::testing::Color) -> (u8, u8, u8) {
    match color {
        quadraui::tui::testing::Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
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
    /// in `quadraui_binding_core::extra_paint`'s `extra_primitives!` table)
    /// rather than a fixed `SLOT_*` integer, so a new additive primitive
    /// never needs a new slot constant.
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
        let mut de = pythonize::Depythonizer::from_object(descriptor.as_any());
        let value = decode_extra_primitive(what, kind, &mut de)
            .map_err(|err| InvalidDescriptorError::new_err(err.0))?;
        self.guarded(|app| {
            app.driver
                .app_mut()
                .host
                .extra_slots
                .insert(kind.to_string(), value);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(kind.to_string())
        })
    }

    /// Dispatches the oldest queued event (or, if none is queued, drains
    /// and dispatches every event a [`QuiAppWaker::post`]/`::wake` call
    /// queued since the last step as a `UiEvent::User` via
    /// `TuiDriver::pump_user_events`, then ticks), then surfaces any
    /// exception a Python callback raised while handling any of that.
    /// Returns whether the app is still running. `pump_user_events` itself
    /// already applies whichever `Reaction` each drained `User` event
    /// produced (repainting on `Redraw`, marking the app exited on `Exit`)
    /// and is a safe no-op when nothing was posted, so it costs nothing to
    /// call unconditionally ahead of the tick every plain-event-free step
    /// already took.
    fn step_once(&mut self) -> PyResult<bool> {
        let running = step_driver(&mut self.driver, &mut self.pending_events);
        take_pending_error(self.driver.app_mut())?;
        Ok(running)
    }
}

/// The compiled extension module (`_quadraui`) that `python/quadraui/__init__.py`
/// imports names from.
#[pymodule]
fn _quadraui(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<QuiApp>()?;
    m.add_class::<QuiAppWaker>()?;
    m.add_class::<QuiShellApp>()?;
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
    m.add(
        "InvalidFontRoleError",
        py.get_type::<InvalidFontRoleError>(),
    )?;
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
    use quadraui::{ButtonMask, ScrollDelta};

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

    /// `waker()` raises `PoisonedError` on a poisoned app rather than
    /// handing out a handle anyway: unlike every other `QuiApp` method, it
    /// doesn't route through `guarded`, so it needs its own explicit
    /// poisoned check (see its doc).
    #[test]
    fn waker_raises_poisoned_error_on_a_poisoned_app() {
        Python::attach(|py| {
            let mut app = QuiApp::new(10, 10, None, None).expect("valid dimensions");

            let panicked: PyResult<()> = app.guarded(|_| panic!("boom"));
            panicked.expect_err("a caught panic must surface as an error");
            assert!(app.poisoned);

            let err = match app.waker() {
                Ok(_) => panic!("a poisoned app must not hand out a waker"),
                Err(err) => err,
            };
            assert!(err.is_instance_of::<PoisonedError>(py));
        });
    }

    /// A `QuiAppWaker::post` call from a genuine background OS thread
    /// delivers its value as a `{"User": value}` event the next time
    /// `step` drains `pump_user_events`, without crashing or exiting the
    /// app. This exercises `PyEventSink::dispatch`'s `UiEvent::User` arm
    /// end to end: `pythonize::pythonize` always errors on `UserPayload`'s
    /// opaque `Arc<dyn Any>`, so delivery here only succeeds because that
    /// arm matches `User` ahead of the `pythonize` call and builds the
    /// `{"User": value}` dict itself instead.
    #[test]
    fn waker_post_from_a_background_thread_delivers_a_user_event() {
        let (handler, locals) = Python::attach(|py| {
            let locals = PyDict::new(py);
            locals
                .set_item("received", PyList::empty(py))
                .expect("set received");
            let handler = py
                .eval(
                    c"lambda event: received.append(event)",
                    Some(&locals),
                    Some(&locals),
                )
                .expect("lambda literal")
                .unbind();
            (handler, locals.unbind())
        });
        let mut app = QuiApp::new(10, 10, Some(handler), None).expect("valid width/height/handler");
        let waker = app.waker().expect("app is not poisoned");

        let value = Python::attach(|py| 42_i32.into_pyobject(py).expect("int").into_any().unbind());
        std::thread::spawn(move || {
            waker.post(value);
        })
        .join()
        .expect("a background thread calling post() must not panic");

        let running = app.step().expect("step must not raise");
        assert!(running, "delivering a User event must not exit the app");

        Python::attach(|py| {
            let locals = locals.bind(py);
            let received: Bound<'_, PyList> = locals
                .get_item("received")
                .expect("received key lookup")
                .expect("received must be set")
                .extract()
                .expect("received must be a list");
            assert_eq!(received.len(), 1, "exactly one event must be delivered");
            let event = received.get_item(0).expect("one item");
            let dict: Bound<'_, PyDict> = event.extract().expect("event must be a dict");
            let user_value = dict
                .get_item("User")
                .expect("\"User\" key lookup")
                .expect("\"User\" value must be present");
            let n: i32 = user_value.extract().expect("the posted int round-trips");
            assert_eq!(n, 42);
        });
    }

    /// A `User` event with no Python-side payload at all (the shape a
    /// native tray/timer wake would produce, with nothing this binding's
    /// own `UserPayload::downcast_ref::<Py<PyAny>>()` can recover) still
    /// reaches the handler, as `{"User": None}`, instead of crashing.
    #[test]
    fn user_event_with_a_foreign_payload_surfaces_as_user_none() {
        let (handler, locals) = Python::attach(|py| {
            let locals = PyDict::new(py);
            locals
                .set_item("received", PyList::empty(py))
                .expect("set received");
            let handler = py
                .eval(
                    c"lambda event: received.append(event)",
                    Some(&locals),
                    Some(&locals),
                )
                .expect("lambda literal")
                .unbind();
            (handler, locals.unbind())
        });
        let inner = PyAppLogic {
            handler: Some(handler),
            ..Default::default()
        };
        let mut driver = TuiDriver::new(inner, 10, 10);

        let reaction = driver.dispatch(UiEvent::User(UserPayload::new(7_i32)));

        assert_ne!(
            reaction,
            Reaction::Exit,
            "a foreign User payload must not crash the dispatch"
        );
        Python::attach(|py| {
            let locals = locals.bind(py);
            let received: Bound<'_, PyList> = locals
                .get_item("received")
                .expect("received key lookup")
                .expect("received must be set")
                .extract()
                .expect("received must be a list");
            assert_eq!(received.len(), 1);
            let event = received.get_item(0).expect("one item");
            let dict: Bound<'_, PyDict> = event.extract().expect("event must be a dict");
            let user_value = dict
                .get_item("User")
                .expect("\"User\" key lookup")
                .expect("\"User\" value must be present");
            assert!(user_value.is_none(), "a foreign payload surfaces as None");
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

    /// A `PyAppLogic` carrying a `QuiApp::register_accelerator`-populated
    /// `accelerators` map still fires `UiEvent::Accelerator` once moved,
    /// by value, into a brand-new `TuiDriver`/backend -- exactly the
    /// `std::mem::take(app.driver.app_mut())` + fresh-backend-construction
    /// sequence `run_tui`/`run_native` perform. Regression test for the
    /// gap where `register_accelerator` only ever touched the live
    /// headless backend directly, so a chord registered before either
    /// real run path silently never fired once that path built its own
    /// backend. `TuiDriver::new` calls `AppLogic::setup` exactly like
    /// `TuiRunner::new_with`/`GtkRunner`/`MacRunner`/`WinRunner` do, so
    /// exercising it here covers the hook all four real runners share.
    #[test]
    fn accelerator_registered_before_setup_fires_on_a_freshly_built_backend() {
        let (handler, locals) = Python::attach(|py| {
            let locals = PyDict::new(py);
            locals
                .set_item("received", PyList::empty(py))
                .expect("set received");
            let handler = py
                .eval(
                    c"lambda event: received.append(event)",
                    Some(&locals),
                    Some(&locals),
                )
                .expect("lambda literal")
                .unbind();
            (handler, locals.unbind())
        });

        let mut accelerators = HashMap::new();
        let acc = Accelerator {
            id: AcceleratorId::new("app.save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        };
        accelerators.insert(acc.id.clone(), acc);
        let inner = PyAppLogic {
            handler: Some(handler),
            host: BindingHost {
                accelerators,
                ..BindingHost::default()
            },
            ..Default::default()
        };

        // Stands in for `run_tui`/`run_native`'s fresh backend
        // construction: `TuiDriver::new` calls `app.setup(&mut backend)`
        // on this *just-moved* `PyAppLogic`, against a backend that has
        // never seen `register_accelerator` called on it directly.
        let mut driver = TuiDriver::new(inner, 10, 10);

        let reaction = driver.dispatch(UiEvent::KeyPressed {
            key: Key::Char('s'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        });
        assert_ne!(
            reaction,
            Reaction::Exit,
            "dispatching the registered chord must not exit the app"
        );

        Python::attach(|py| {
            let locals = locals.bind(py);
            let received: Bound<'_, PyList> = locals
                .get_item("received")
                .expect("received key lookup")
                .expect("received must be set")
                .extract()
                .expect("received must be a list");
            assert_eq!(received.len(), 1, "exactly one event must be delivered");
            let event = received.get_item(0).expect("one item");
            let dict: Bound<'_, PyDict> = event.extract().expect("event must be a dict");
            assert!(
                dict.contains("Accelerator").expect("contains check"),
                "the chord must surface as an Accelerator event, not a plain \
                KeyPressed: got {dict:?}"
            );
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

    /// Builds a [`NativeAppLogic`] wrapping `inner`, on whichever native
    /// feature is enabled — a thin helper so the `WindowClose`-only tests
    /// below (none of which exercise the macOS-only non-`WindowClose`
    /// downgrade) can run on `gtk`/`win` too, not just `macos`.
    #[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
    fn native_app_logic_for_window_close(inner: PyAppLogic) -> NativeAppLogic {
        NativeAppLogic {
            inner,
            #[cfg(all(feature = "macos", not(feature = "gtk")))]
            exit_requested: Rc::new(Cell::new(false)),
        }
    }

    /// A registered handler returning `True` (the "stay" answer — see
    /// [`reaction_from_window_close_return`]) vetoes a `WindowClose`:
    /// `NativeAppLogic` must return exactly what the handler produced, not
    /// force `Reaction::Exit`. This branch of `NativeAppLogic::handle` has
    /// nothing macOS-specific about it, so this runs on `gtk`/`win` too.
    #[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
    #[test]
    fn native_app_logic_window_close_handler_veto_stays_open() {
        Python::attach(|py| {
            let handler = py
                .eval(c"lambda event: True", None, None)
                .expect("lambda literal")
                .unbind();
            let inner = PyAppLogic {
                handler: Some(handler),
                ..Default::default()
            };
            let logic = native_app_logic_for_window_close(inner);
            let mut driver = TuiDriver::new(logic, 10, 10);

            let reaction = driver.dispatch(UiEvent::WindowClose);

            assert_eq!(
                reaction,
                Reaction::Continue,
                "a handler returning True must veto the close"
            );
        });
    }

    /// A registered handler returning `False` (the "confirm" answer)
    /// confirms a `WindowClose`: `NativeAppLogic` returns the handler's own
    /// `Reaction::Exit` unchanged.
    #[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
    #[test]
    fn native_app_logic_window_close_handler_confirm_exits() {
        Python::attach(|py| {
            let handler = py
                .eval(c"lambda event: False", None, None)
                .expect("lambda literal")
                .unbind();
            let inner = PyAppLogic {
                handler: Some(handler),
                ..Default::default()
            };
            let logic = native_app_logic_for_window_close(inner);
            let mut driver = TuiDriver::new(logic, 10, 10);

            let reaction = driver.dispatch(UiEvent::WindowClose);

            assert_eq!(reaction, Reaction::Exit);
        });
    }

    /// A registered handler that returns `None` (no explicit `return` —
    /// the common case for a handler with no opinion about closing, the
    /// same shape `examples/file_manager.py`'s quit wrapper uses) must
    /// *not* veto the close: the window stays closable by the window
    /// manager's own ×/Alt-F4/close button even though a handler is
    /// registered, exactly as if no handler had been registered at all.
    /// See [`reaction_from_window_close_return`]'s doc for why this is the
    /// opposite default from every other event.
    #[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
    #[test]
    fn native_app_logic_window_close_handler_no_opinion_exits_by_default() {
        Python::attach(|py| {
            let handler = py
                .eval(c"lambda event: None", None, None)
                .expect("lambda literal")
                .unbind();
            let inner = PyAppLogic {
                handler: Some(handler),
                ..Default::default()
            };
            let logic = native_app_logic_for_window_close(inner);
            let mut driver = TuiDriver::new(logic, 10, 10);

            let reaction = driver.dispatch(UiEvent::WindowClose);

            assert_eq!(
                reaction,
                Reaction::Exit,
                "a handler with no opinion about closing must not veto it"
            );
        });
    }

    /// With no handler registered at all, `WindowClose` still forces
    /// `Reaction::Exit` — the default for an app that never opted into
    /// vetoing a close.
    #[cfg(any(feature = "gtk", feature = "macos", feature = "win"))]
    #[test]
    fn native_app_logic_window_close_with_no_handler_exits_by_default() {
        let logic = native_app_logic_for_window_close(PyAppLogic::default());
        let mut driver = TuiDriver::new(logic, 10, 10);

        let reaction = driver.dispatch(UiEvent::WindowClose);

        assert_eq!(reaction, Reaction::Exit);
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

    /// A `tabs` node's per-tab `"is_dirty"` and strip-level
    /// `"right_segments"` reach the painted `TabBar` the same way they
    /// already do for the standalone `tab_bar` leaf: a dirty tab's close
    /// glyph becomes the unsaved-changes dot, and a right segment's own
    /// text paints in the strip even though it carries no tab content of
    /// its own.
    #[test]
    fn tabs_node_renders_a_dirty_marker_and_right_segments_supplied_from_python() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
def render():
    return {'tabs': {
        'id': 'tabs:main',
        'selected': 0,
        'right_segments': [
            {'text': 'SEGTXT', 'width_cells': 6, 'id': None, 'is_active': False},
        ],
        'tabs': [
            {
                'id': 'tab:a', 'title': 'Alpha', 'is_dirty': True,
                'content': {'id': 'text:a', 'text_display': {
                    'id': 'text:a',
                    'lines': [{'spans': [{'text': 'AlphaBody', 'fg': None, 'bg': None}]}],
                }},
            },
            {
                'id': 'tab:b', 'title': 'Beta',
                'content': {'id': 'text:b', 'text_display': {
                    'id': 'text:b',
                    'lines': [{'spans': [{'text': 'BetaBody', 'fg': None, 'bg': None}]}],
                }},
            },
        ],
    }}
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

            let mut app = QuiApp::new(60, 20, None, Some(render)).expect("valid dimensions");

            assert!(
                app.screen_contains("Alpha").expect("render must not raise"),
                "the dirty tab's title must still render"
            );
            assert!(
                app.screen_contains("●").expect("render must not raise"),
                "a dirty tab's close glyph must be the unsaved-changes dot, not '×'"
            );
            assert!(
                app.screen_contains("SEGTXT")
                    .expect("render must not raise"),
                "the strip-level right segment's own text must be painted"
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

    /// A key pressed while the focused tree's row is mid-rename
    /// (`TreeRow::edit` is `Some`) is replaced by the semantic
    /// `UiEvent::Tree(RowRenameChanged/Committed/Cancelled)` it stands for,
    /// not the tree's raw `KeyPressed` — so a Python handler can write the
    /// reported `text`/`cursor`/`selection_anchor` straight back onto the
    /// row's `edit` without reimplementing cursor math itself.
    #[test]
    fn key_pressed_during_a_tree_row_rename_emits_semantic_rename_events() {
        Python::attach(|py| {
            let locals = PyDict::new(py);
            py.run(
                c"
received = []

def render():
    return {'tree': {
        'id': 'tree:main',
        'rows': [{
            'path': [0], 'indent': 0, 'icon': None,
            'text': {'spans': [{'text': 'file.txt', 'fg': None, 'bg': None}]},
            'badge': None, 'is_expanded': None,
            'edit': {'text': 'fi', 'cursor': 2, 'selection_anchor': None, 'placeholder': None},
        }],
        'selection_mode': 'Single', 'selected_path': [0],
        'style': {
            'indent': 2, 'show_chevrons': True,
            'chevron_expanded': 'v', 'chevron_collapsed': '>',
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
                QuiApp::new(40, 20, Some(handle), Some(render)).expect("valid dimensions");
            let screen = app.screen().expect("render must not raise");
            assert!(
                screen.contains("fi"),
                "the in-progress rename text must be painted"
            );

            fn extract_received<'py>(locals: &Bound<'py, PyDict>) -> Bound<'py, PyList> {
                locals
                    .get_item("received")
                    .expect("dict lookup")
                    .expect("received must be defined")
                    .cast_into::<PyList>()
                    .expect("received must be a list")
            }

            fn tree_event<'py>(event_dict: &Bound<'py, PyDict>) -> (String, Bound<'py, PyDict>) {
                let payload = event_dict
                    .get_item("Tree")
                    .expect("dict lookup")
                    .expect("a rename key must dispatch a Tree event")
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
                (widget, inner)
            }

            // Tab focuses the tree (its only tab stop).
            app.driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            assert_eq!(
                extract_received(&locals).len(),
                1,
                "Tab must emit exactly one event"
            );

            // Typing 'x' at cursor 2 of \"fi\" must insert it there, not
            // reimplement the insert in Python -- the reported `text` is
            // already \"fix\" with `cursor` advanced to 3.
            app.driver.dispatch(UiEvent::KeyPressed {
                key: Key::Char('x'),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            let received_list = extract_received(&locals);
            assert_eq!(
                received_list.len(),
                2,
                "the rename keystroke must emit one more event"
            );
            let event_dict = received_list
                .get_item(1)
                .expect("second item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let (widget, inner) = tree_event(&event_dict);
            assert_eq!(widget, "tree:main");
            let changed = inner
                .get_item("RowRenameChanged")
                .expect("dict lookup")
                .expect("a rename keystroke must carry RowRenameChanged, not raw KeyPressed")
                .cast_into::<PyDict>()
                .expect("RowRenameChanged payload must be a dict");
            let text: String = changed
                .get_item("text")
                .expect("dict lookup")
                .expect("text key must be present")
                .extract()
                .expect("text must be a string");
            assert_eq!(
                text, "fix",
                "the typed char must land at the cursor, not corrupt the text"
            );
            let cursor: usize = changed
                .get_item("cursor")
                .expect("dict lookup")
                .expect("cursor key must be present")
                .extract()
                .expect("cursor must be an int");
            assert_eq!(cursor, 3, "the cursor must advance past the inserted char");

            // Enter commits the rename with the (unchanged by this event)
            // current text.
            app.driver.dispatch(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            });
            let received_list = extract_received(&locals);
            assert_eq!(received_list.len(), 3, "Enter must emit one more event");
            let event_dict = received_list
                .get_item(2)
                .expect("third item")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let (_, inner) = tree_event(&event_dict);
            let committed = inner
                .get_item("RowRenameCommitted")
                .expect("dict lookup")
                .expect("Enter during a rename must carry RowRenameCommitted")
                .cast_into::<PyDict>()
                .expect("RowRenameCommitted payload must be a dict");
            let text: String = committed
                .get_item("text")
                .expect("dict lookup")
                .expect("text key must be present")
                .extract()
                .expect("text must be a string");
            assert_eq!(
                text, "fi",
                "Enter commits the text the row's edit state still holds"
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
            let (mut app, locals) = app_recording_events(py);

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

    /// Builds the `(app, locals)` pair a `show_context_menu`/
    /// `show_message_dialog` test drives: a fresh `QuiApp` with a Python
    /// handler that appends every event it receives into
    /// `locals["received"]`.
    fn app_recording_events(py: Python<'_>) -> (QuiApp, Bound<'_, PyDict>) {
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
        let app = QuiApp::new(40, 20, Some(handle), None).expect("valid dimensions");
        (app, locals)
    }

    /// `QuiApp::show_message_dialog` opens a painted dialog; a click on one
    /// of its buttons closes it and dispatches
    /// `UiEvent::MessageDialogResolved` to the registered handler — the
    /// same round trip `show_context_menu`'s own test exercises for a
    /// context menu.
    #[test]
    fn show_message_dialog_then_clicking_a_button_emits_message_dialog_resolved() {
        Python::attach(|py| {
            let (mut app, locals) = app_recording_events(py);

            let buttons_any = py
                .eval(
                    c"[
                        {'id': 'save', 'label': 'Save', 'is_default': True},
                        {'id': 'cancel', 'label': 'Cancel', 'is_cancel': True},
                    ]",
                    None,
                    None,
                )
                .expect("list literal must evaluate");
            let buttons = buttons_any
                .cast::<PyList>()
                .expect("literal must be a list");

            app.show_message_dialog(
                "Unsaved changes".to_string(),
                "Do you want to save?".to_string(),
                buttons,
                Some("warning".to_string()),
            )
            .expect("show_message_dialog must open the dialog");
            assert!(
                app.screen_contains("Save").expect("render must not raise"),
                "the dialog's buttons must be painted"
            );

            let button_bounds = app
                .driver
                .find_bounds("Save")
                .expect("the 'Save' button must be on screen");

            // Only `x` gets a fractional nudge: the dialog box is
            // vertically centered in the (even-height) viewport against an
            // (odd-height) box, so every row's continuous-coordinate
            // button rect sits a half-cell above the integer row
            // `find_bounds` reports — `button_bounds.y` itself (the rect's
            // own top edge) is already inside it, but `+ 0.5` would land
            // exactly on that rect's exclusive bottom edge and miss.
            // `button_bounds.height` is always `1.0` here regardless (one
            // text row), not the continuous `DialogLayout` rect's own
            // height, so `+ height * 0.5` lands on that same excluded
            // boundary instead of fixing it.
            let click = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(button_bounds.x + 0.5, button_bounds.y),
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
                "clicking a dialog button must dispatch exactly one event"
            );
            let resolved = received_list
                .get_item(0)
                .expect("item must exist")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let resolved_id: String = resolved
                .get_item("MessageDialogResolved")
                .expect("dict lookup")
                .expect("a MessageDialogResolved event must be dispatched")
                .extract()
                .expect("MessageDialogResolved carries the chosen button's id as a string");
            assert_eq!(resolved_id, "save");

            assert!(
                !app.screen_contains("Save").expect("render must not raise"),
                "resolving the dialog must dismiss it"
            );
        });
    }

    /// Enter activates the keyboard-focused button — seeded on the first
    /// `is_default` button — closing the dialog the same way a click on it
    /// does.
    #[test]
    fn show_message_dialog_then_pressing_enter_resolves_the_default_button() {
        Python::attach(|py| {
            let (mut app, locals) = app_recording_events(py);

            let buttons_any = py
                .eval(
                    c"[
                        {'id': 'save', 'label': 'Save', 'is_default': True},
                        {'id': 'cancel', 'label': 'Cancel', 'is_cancel': True},
                    ]",
                    None,
                    None,
                )
                .expect("list literal must evaluate");
            let buttons = buttons_any
                .cast::<PyList>()
                .expect("literal must be a list");

            app.show_message_dialog(
                "Unsaved changes".to_string(),
                "Do you want to save?".to_string(),
                buttons,
                None,
            )
            .expect("show_message_dialog must open the dialog");

            let enter = UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            };
            let enter_any = pythonize::pythonize(py, &enter).expect("UiEvent must pythonize");
            app.push_event(
                enter_any
                    .cast::<PyDict>()
                    .expect("KeyPressed must pythonize to a dict"),
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
                "Enter on the default button must dispatch exactly one event"
            );
            let resolved = received_list
                .get_item(0)
                .expect("item must exist")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            let resolved_id: String = resolved
                .get_item("MessageDialogResolved")
                .expect("dict lookup")
                .expect("a MessageDialogResolved event must be dispatched")
                .extract()
                .expect("MessageDialogResolved carries the chosen button's id as a string");
            assert_eq!(resolved_id, "save");
        });
    }

    /// Opening a message dialog while a `"menu_bar"` extra's dropdown is
    /// open closes the dropdown first: paint order puts the dialog on top
    /// of the bar (`render_dialog` runs after `sync_menu_bar`), so without
    /// this, the dropdown would be stuck open but invisible underneath,
    /// and `handle` would route a click/Enter to it (`MenuActivated`)
    /// instead of to the dialog it paints on top of, since `handle`
    /// previously checked the dropdown before the dialog. Pins the fix:
    /// the dropdown's "Open" item vanishes the moment the dialog opens,
    /// and Enter resolves the dialog rather than activating the hidden
    /// menu item.
    #[test]
    fn show_message_dialog_closes_an_open_menu_bar_dropdown_and_claims_input() {
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
            assert!(
                app.screen_contains("Open").expect("render must not raise"),
                "the dropdown must be open before the dialog opens"
            );

            let buttons_any = py
                .eval(
                    c"[{'id': 'ok', 'label': 'OK', 'is_default': True}]",
                    None,
                    None,
                )
                .expect("list literal must evaluate");
            let buttons = buttons_any
                .cast::<PyList>()
                .expect("literal must be a list");
            app.show_message_dialog(
                "Heads up".to_string(),
                "Something happened.".to_string(),
                buttons,
                None,
            )
            .expect("show_message_dialog must open the dialog");

            assert!(
                !app.screen_contains("Open").expect("render must not raise"),
                "opening the dialog must close the dropdown painted underneath it"
            );
            assert!(
                app.screen_contains("OK").expect("render must not raise"),
                "the dialog's own button must be painted"
            );

            let enter = UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            };
            let enter_any = pythonize::pythonize(py, &enter).expect("UiEvent must pythonize");
            app.push_event(
                enter_any
                    .cast::<PyDict>()
                    .expect("KeyPressed must pythonize to a dict"),
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
                "Enter must resolve the dialog, not reach the already-closed dropdown"
            );
            let resolved = received_list
                .get_item(0)
                .expect("item must exist")
                .cast_into::<PyDict>()
                .expect("event payload must be a dict");
            assert!(
                resolved
                    .get_item("MessageDialogResolved")
                    .is_ok_and(|v| v.is_some()),
                "Enter must dispatch MessageDialogResolved, not MenuActivated"
            );
        });
    }

    /// `show_message_dialog` rejects an empty `buttons` list: a dialog
    /// with nothing to activate can never be resolved by Enter or Escape,
    /// and would otherwise swallow every `MouseDown` forever with no way
    /// to dismiss it.
    #[test]
    fn show_message_dialog_rejects_empty_buttons() {
        Python::attach(|py| {
            let (mut app, _locals) = app_recording_events(py);
            let empty = PyList::empty(py);
            let err = app
                .show_message_dialog("Title".to_string(), "Body".to_string(), &empty, None)
                .expect_err("an empty buttons list must be rejected");
            assert!(err.is_instance_of::<InvalidDescriptorError>(py));
        });
    }

    /// `show_message_dialog` rejects a button with a non-`None` `tint`
    /// rather than silently dropping it: this surface shares
    /// `MessageDialogButton`'s id/label/default/cancel shape with native
    /// alerts, which have no tint concept at all, so a caller relying on
    /// tint being honored must find out immediately.
    #[test]
    fn show_message_dialog_rejects_button_tint() {
        Python::attach(|py| {
            let (mut app, _locals) = app_recording_events(py);
            let buttons_any = py
                .eval(
                    c"[{'id': 'delete', 'label': 'Delete', 'is_default': True, \
                       'tint': {'r': 255, 'g': 0, 'b': 0, 'a': 255}}]",
                    None,
                    None,
                )
                .expect("list literal must evaluate");
            let buttons = buttons_any
                .cast::<PyList>()
                .expect("literal must be a list");
            let err = app
                .show_message_dialog("Title".to_string(), "Body".to_string(), buttons, None)
                .expect_err("a tinted button must be rejected");
            assert!(err.is_instance_of::<InvalidDescriptorError>(py));
        });
    }

    /// `dismiss_message_dialog` closes an open dialog without resolving
    /// it — no `MessageDialogResolved` is dispatched — and returns `False`
    /// when nothing was open to close.
    #[test]
    fn dismiss_message_dialog_closes_without_resolving() {
        Python::attach(|py| {
            let (mut app, locals) = app_recording_events(py);

            assert!(
                !app.dismiss_message_dialog().expect("must not raise"),
                "nothing is open yet"
            );

            let buttons_any = py
                .eval(
                    c"[{'id': 'ok', 'label': 'OK', 'is_default': True}]",
                    None,
                    None,
                )
                .expect("list literal must evaluate");
            let buttons = buttons_any
                .cast::<PyList>()
                .expect("literal must be a list");
            app.show_message_dialog("Title".to_string(), "Body".to_string(), buttons, None)
                .expect("show_message_dialog must open the dialog");
            assert!(app.screen_contains("OK").expect("render must not raise"));

            assert!(
                app.dismiss_message_dialog().expect("must not raise"),
                "a dialog was open to close"
            );
            assert!(!app.screen_contains("OK").expect("render must not raise"));

            let received_list = locals
                .get_item("received")
                .expect("dict lookup")
                .expect("received must be defined")
                .cast_into::<PyList>()
                .expect("received must be a list");
            assert_eq!(
                received_list.len(),
                0,
                "dismiss_message_dialog must not dispatch MessageDialogResolved"
            );
        });
    }
}
