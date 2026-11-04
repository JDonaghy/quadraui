//! napi-rs bindings over `quadraui`'s headless TUI driver.
//!
//! The language-neutral hit testing, semantic event synthesis, text-input/
//! focus/drag controllers, retained descriptor model, and render -> frame
//! assembly this binding drives all live in `quadraui_binding_core` (see
//! that crate's module doc) -- this crate is the napi-rs leaf around it,
//! the same role `quadraui-py` plays for Python.
//!
//! [`QuiApp`] wraps a `quadraui::tui::testing::TuiDriver` running an
//! in-crate [`NodeAppLogic`] that holds one [`quadraui_binding_core::BindingHost`]
//! (the six fixed primitive slots, any additive slots, and the
//! backend-settings replay store) plus a [`quadraui_binding_core::InteractionEngine`],
//! the registered-callback bridge, and a pending-error slot -- the napi-rs
//! analog of `quadraui-py`'s `PyAppLogic`. The surface is the headless
//! driver loop (`new`, `step`, `run`, `screen`, `screen_contains`, `resize`,
//! `push_key_event`, `slot_clear`), the callback bridge (`setHandler`,
//! `setTickHandler`, `setRender`, `actions`), and the descriptor decode
//! adapter below: the six fixed `insert*` methods, `pushEvent`, and the
//! additive-primitive `insertExtra`/`insertProgress`/`insertSpinner`/
//! `insertImage`/`clearExtra`. The remaining mechanical method groups each
//! get their own sibling module, every one a thin leaf over the same
//! `BindingHost`/`InteractionEngine`/`Backend`/`PlatformServices` surface
//! `quadraui-py` already wraps: `clipboard` (`Clipboard`), `overlays`
//! (`InteractionEngine`'s toast/tooltip/completions/float/rich-text-popup
//! overlays), `metrics` (`Backend`'s measurement/capability surface),
//! `theme` (theme/style/font configuration), `accelerators`
//! (accelerators & global shortcuts), and `platform` (file dialogs,
//! notifications, URL/path opening, trash, beep, system theme, platform
//! identity). Each is declared `mod` rather than `pub mod` -- every method
//! it adds to `QuiApp` is still part of this crate's public napi surface
//! (`#[napi] impl QuiApp` blocks across modules all extend the one exported
//! type), only the Rust-level module path is private, so the names above
//! are plain code spans rather than intra-doc links `cargo doc` could
//! resolve.
//!
//! **Callback bridge.** [`NodeEventSink`] is the napi-rs analog of
//! `quadraui-py`'s `PyEventSink`: it converts a dispatched [`UiEvent`] to a
//! JS value via [`encode_via_json`] (replacing `pythonize::pythonize`),
//! attaches a painted hit's [`HitDetail`] under the event payload's `"hit"`
//! key the same way, calls the registered JS handler as a plain synchronous
//! `Function::call` (it runs inside a step the leaf itself invokes -- no
//! `ThreadsafeFunction`/async hop needed, and PyO3's GIL dance has no
//! equivalent to begin with), and narrows the return via
//! [`bool_from_return`]'s `ValueType::Boolean` check before mapping it
//! through `callback_reaction`/`callback_reaction_for_render`/
//! `window_close_reaction` (`quadraui_binding_core::host`). [`record_error`]/
//! [`take_pending_error`] are this crate's own first-error-wins port.
//! `setHandler`/`setTickHandler`/`setRender` each store a [`NodeCallback`] --
//! napi-rs's own [`FunctionRef`] reference type for keeping a `Function`
//! alive past the call that handed it out, the analog of `quadraui-py`'s
//! `Py<PyAny>`.
//!
//! **Re-entrancy guard.** Unlike PyO3, which rejects a second mutable
//! borrow of the same `#[pyclass]` cell at runtime, napi-rs's own
//! per-instance native-borrow tracking already rejects a handler calling
//! straight back into another method on the same `QuiApp` mid-dispatch at
//! the FFI boundary -- confirmed empirically, not merely assumed; see
//! [`QuiApp`]'s own doc. [`QuiApp::check_not_dispatching`] (backed by
//! `QuiApp::in_dispatch`) adds this crate's own explicit,
//! `ReentrantCallError`-tagged guard for the same scenario -- defense in
//! depth, not the only thing standing between a handler and undefined
//! behaviour. [`DeferredAction`]/[`flush_deferred_actions`]/[`QuiAppActions`]
//! are the sanctioned way around the guard: a handler queues a dialog/
//! context-menu/toast/notification/file action through `actions()` instead
//! of calling the blocked method directly, and `NodeAppLogic::handle`/`tick`
//! apply the queue right after each dispatch finishes.
//!
//! **Descriptor decode adapter.** The napi-rs analog of `quadraui-py`'s
//! `pythonize::depythonize` path, split by how much of `quadraui_binding_core`'s
//! decode medium a given entry point needs:
//! - [`decode_direct`] hands napi's own `Env::from_js_value` (gated behind
//!   this crate's `serde-json` feature on the `napi` dependency) straight to
//!   a derived `Deserialize` type -- the six fixed primitive slots and every
//!   overlay's own descriptor shape, none of which carries a byte-string/
//!   float field at risk of the two pre-checks below, or a nested
//!   newtype-struct identifier (see [`decode_via_json`]).
//! - [`decode_via_core`] first materializes the JS value into one
//!   [`serde_json::Value`] via [`to_descriptor_value`], then hands `&value`
//!   to a kind-dispatched core decoder (`decode_extra_primitive` today;
//!   `decode_layout_node`/`decode_render_output` are exercised directly
//!   against a hand-built `serde_json::Value` by this module's own tests,
//!   with no napi-visible method routing JS input through them yet, since
//!   no render callback exists to call them from) -- exactly the
//!   `&serde_json::Value` path `quadraui_binding_core`'s own tests use.
//!   [`to_descriptor_value`] is this crate's own walk rather than a reuse of
//!   `quadraui_binding_core::wire`'s private `materialize` visitor (not
//!   reachable from here) or napi's own `FromNapiValue for serde_json::Value`
//!   (reachable, but not reused: its array detection is a bare
//!   `Array.isArray` check, so a `Buffer`/`Uint8Array`/`ArrayBuffer` falls
//!   through to its generic object branch -- every index becomes a
//!   *string* map key, not the array `quadraui::ImageSource::Bytes` decodes
//!   from). It folds a `Buffer`/`Uint8Array`/`ArrayBuffer` into a plain
//!   array of byte numbers before any object/array branch can see it, and
//!   rejects a non-finite float (`NaN`/`Infinity`) outright instead of
//!   letting it alias `null` the way a naive conversion would -- the same
//!   two deltas `quadraui_binding_core::wire::materialize` keeps versus
//!   `serde_json::Value`'s own `Deserialize` impl, applied here against
//!   napi's JS object model instead of pythonize's.
//! - [`decode_via_json`] also materializes through [`to_descriptor_value`],
//!   like [`decode_via_core`], but then hands the resulting
//!   [`serde_json::Value`] to `T`'s own derived `Deserialize` directly
//!   (`serde_json::from_value`) instead of a core decoder -- the napi-3
//!   `Deserializer`'s `deserialize_newtype_struct` workaround for a
//!   standalone one-field tuple-struct identifier
//!   ([`quadraui::AcceleratorId`], [`quadraui::GlobalShortcutId`]), which
//!   `decode_direct`'s `Env::from_js_value` can't decode directly.
//! - [`encode_via_json`] mirrors the decode side for the opposite direction:
//!   materializes `T` into one [`serde_json::Value`] via `serde_json::to_value`
//!   first, then converts that to a JS value via `Env::to_js_value`, working
//!   around napi 3's own `Serializer` discarding the enum-variant tag a plain
//!   `Env::to_js_value` on `T` directly would need. Every call site handing a
//!   `quadraui`/`quadraui_binding_core` value to JS goes through this.
//! - Either path's failure becomes a JS `Error` tagged `InvalidDescriptorError`
//!   in its message via [`invalid_descriptor_error`] -- napi-rs has no
//!   custom-exception-class mechanism at the FFI boundary the way PyO3's
//!   `create_exception!` does, so the tag lives in the message instead of a
//!   distinct JS class (`assert.throws(fn, /InvalidDescriptorError/)` on the
//!   JS side). An already-occupied slot/kind raises [`slot_occupied_error`]
//!   (tagged `SlotOccupiedError`) before any decode is attempted.
//!
//! A Rust panic reaching a driver call is caught at the [`QuiApp::guarded`]
//! boundary, raised as a JS `Error` instead of unwinding into Node, and
//! poisons the handle so every later call raises rather than touching a
//! driver that may have panicked mid-mutation -- mirrors `quadraui-py`'s
//! `QuiApp::guarded`. napi-rs itself already wraps every exported method in
//! `catch_unwind` to stop a panic aborting the whole process, but that alone
//! does not poison the handle for later calls; this boundary adds exactly
//! that.
//!
//! **ShellApp / `QuiApp.runShell`.** [`QuiShellApp`] wraps a
//! `quadraui::shell_adapter::ShellAdapter<NodeShellApp>` running headlessly
//! over a `TuiDriver` (an `AppShell` -- activity bar, sidebar, optional
//! bottom-panel tab strip -- around JS-registered content and event
//! handlers), the napi-rs analog of `quadraui-py`'s own `QuiShellApp` /
//! `PyShellApp`. `driver_with_shell` returns `TuiDriver<impl AppLogic>`: an
//! opaque, per-call-site type [`QuiShellApp`] cannot name, so it stores a
//! `Box<dyn ErasedShellDriver>` instead, erased by the blanket impl
//! `quadraui_binding_core` already provides. `ShellAdapter`'s own `app: A`
//! field is `pub(crate)` to `quadraui`, unreachable from this crate even
//! through `TuiDriver::app_mut()`, so [`ShellState`] is the side channel
//! around that: [`QuiShellApp`] and the [`NodeShellApp`] moved into its
//! driver each hold their own `Rc` clone of the same `RefCell<ShellState>`
//! -- including the live `Env` for whichever `QuiShellApp` method call is
//! currently driving it, since there is no `app_mut()` seam to stash that
//! on the way `NodeAppLogic::env` does. `QuiApp::runShell` decodes its
//! `config` object into a `ShellConfig` via `quadraui_binding_core::decode_shell_config`
//! (through the same [`to_descriptor_value`] materialization every other
//! kind-dispatched decoder in this crate uses -- `&serde_json::Value`
//! implements `serde::Deserializer` directly, so no extra wrapper is
//! needed) and builds the [`NodeShellApp`]/driver pair around it.
//! `AppShellEvent`/`BottomPanelEvent` have no `Serialize` derive upstream
//! (same reason `quadraui-py` hand-builds their Python dict shape), so
//! [`app_shell_event_to_json`]/[`bottom_panel_event_to_json`] hand-build
//! the same externally-tagged JSON shape [`encode_via_json`] would give a
//! derived enum, then convert that through `Env::to_js_value` directly.
//!
//! **Live run loop (`startTui`) / waker.** [`QuiApp`] otherwise only ever
//! drives a headless `quadraui::tui::testing::TuiDriver`. `startTui` is
//! this crate's one structural departure from `quadraui-py`'s own
//! `run_tui`/`run_native`: those are *blocking* pump loops that only work
//! because `py.detach` lets other Python threads run underneath; under
//! Node a blocking pump would freeze the one libuv reactor the whole
//! process shares, and a background-thread pump is impossible (every
//! runner, `BindingHost`, and `InteractionEngine` are `Rc`-based, so
//! `NodeAppLogic` is `!Send`). `startTui` instead `std::mem::take`s this
//! app's whole [`NodeAppLogic`] out of [`QuiApp::driver`] into a real
//! [`TuiRunner`] -- a live terminal takeover -- and stores it on
//! [`QuiApp::live`]; [`QuiApp::step`]/[`QuiApp::run`] then drive that
//! [`TuiRunner::step`] non-blocking pass by non-blocking pass instead of
//! the headless driver's own `step_driver`, exactly as fit for a JS
//! `setInterval`/`setImmediate` loop (see `js/index.ts`'s `pump` helper) as
//! the headless path already was. [`QuiApp::check_not_live`] rejects every
//! configuration method (`setHandler`, `insertButton`, `actions`, ...)
//! once a live session is running, the same "configure once, up front,
//! before starting" discipline `quadraui-py`'s own doc describes for
//! `run_tui`/`run_native` -- there, Python's blocking call makes that
//! discipline unavoidable; here it has to be an explicit, checked
//! contract, since control returns to JS after every `step()`. An
//! `actions()`/`waker()` handle obtained *before* `startTui` keeps working
//! correctly through the live session regardless (both only ever hold an
//! `Rc`/`Arc` clone of state that moves, not copies, with the
//! [`NodeAppLogic`] `startTui` relocates).
//!
//! [`QuiApp::waker`] returns a [`QuiAppWaker`] -- unlike `QuiApp` itself,
//! `Send + Sync`, so a background thread can hold and call it -- wrapping
//! this app's [`SharedWaker`]: the one [`Backend::waker`] closure currently
//! reachable, headless until `startTui` swaps it to the live `TuiRunner`'s
//! own backend's waker for that session's duration (restored the instant
//! `step` observes [`StepOutcome::Exited`]), mirroring `quadraui-py`'s own
//! `current_waker`/`QuiAppWaker`. `UserPayload` must be `Send` to cross
//! that thread boundary, which rules out a raw JS value (`napi::Env`-bound,
//! single-threaded by construction) the way `quadraui-py`'s own
//! `Py<PyAny>` payload needs no such care (always `Send + Sync`) --
//! [`QuiAppWaker::post`] converts its argument through the same
//! [`to_descriptor_value`] walk every other JS-to-Rust decode in this
//! crate uses, landing a plain [`serde_json::Value`] (`Send`) in the
//! [`UserPayload`] instead. [`NodeEventSink::dispatch_user`] is the
//! encode-direction counterpart: `UiEvent::User`'s own `Serialize` impl
//! deliberately errors (see [`UserPayload`]'s own doc) on its opaque
//! payload, so it is special-cased ahead of [`encode_via_json`] the same
//! way `quadraui-py`'s `PyEventSink::dispatch` special-cases it, recovering
//! the wrapped `JsonValue` via `UserPayload::downcast_ref` and delivering
//! `{"User": value}` to the registered handler.
//!
//! No `ThreadsafeFunction` is needed to deliver a wake promptly: `TuiBackend`'s
//! own `waker()` doc already spells out the embedding contract every host
//! (this one included) gets for free -- a background post lands in a
//! plain queue `poll_events`/`wait_events` drain unconditionally, with no
//! way to interrupt an in-progress blocking wait early, so "prompt" means
//! "observed on the next `step()` call" and a host that wants tighter
//! latency keeps its own poll cadence short rather than relying on an
//! early wake. [`QuiAppWaker::post_after`]/`::wake_after` spawn their own
//! background thread purely to delay *when* the same already-thread-safe
//! waker closure is called, reusing it rather than adding a second,
//! TSFN-based notification path whose only addressable gain -- shaving the
//! few milliseconds between that thread waking and the JS-side `pump`
//! loop's next tick -- is already covered by `pump`'s own `intervalMs`
//! knob.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as JsonValue;

use quadraui::tui::testing::{driver_with_shell, TuiDriver};
use quadraui::tui::{RunConfig, StepOutcome, TuiRunner};
use quadraui::{
    AppLogic, AppShellEvent, AppShellLayout, Backend, BackendCaps, BottomPanelEvent, Button,
    ContextMenuItem, DataTable, DialogButton, FloatMeasure, ListView, Notification, Point,
    Reaction, Rect, RgbaImage, ShellApp, ShellConfig, ShellContext, TabBar, TextDisplay, Toast,
    ToastCorner, TreeView, UiEvent, UserPayload, WidgetId,
};
#[cfg(test)]
use quadraui_binding_core::callback_reaction;
use quadraui_binding_core::{
    callback_reaction_for_render, caret_shape, click_down_up_events, color_depth_name,
    decode_extra_primitive, decode_render_output, decode_shell_config, dialog_severity, draw_bands,
    font_role, key_pressed_event, merge_overrides, message_dialog_options_from_buttons,
    paint_layout, panic_message, render_overlays, step_driver, text_display_from_lines,
    theme_color_value, toast_corner, window_close_reaction, BindingHost, DecodeError,
    ErasedShellDriver, EventSink, HitDetail, InteractionEngine, OverrideError, RenderOutput,
    WindowCloseHandling, SLOT_BUTTON, SLOT_DATA_TABLE, SLOT_LIST, SLOT_TAB_BAR, SLOT_TEXT_DISPLAY,
    SLOT_TREE,
};
#[cfg(test)]
use quadraui_binding_core::{decode_layout_node, LayoutNode};

mod accelerators;
mod clipboard;
mod metrics;
mod overlays;
mod platform;
mod theme;

/// A callback registered via `setHandler`/`setTickHandler`/`setRender`,
/// stored across calls as a [`FunctionRef`] -- napi-rs's own reference type
/// for exactly this (see its doc: "If you want to use \[a `Function`\]
/// outside the scope of a function call, you can turn it into a reference
/// by calling the `create_ref` method"), the napi-rs analog of
/// `quadraui-py`'s `Py<PyAny>`. Parameterized `Unknown` both ways: every
/// call site here calls with exactly one JS value (the event payload, or a
/// JSON `null` placeholder for a zero-argument tick/render callback -- see
/// [`invoke_callback`]) and narrows the return itself via
/// [`bool_from_return`]/[`render_output_from_callback`] rather than
/// leaning on `Function`'s own typed `Return`.
type NodeCallback = FunctionRef<Unknown<'static>, Unknown<'static>>;

/// A [`Backend::waker`] closure, shared (and swapped, by [`QuiApp::start_tui`])
/// between [`QuiApp::driver`]'s headless backend and whichever live
/// [`LiveRunner`]'s own backend is currently running -- the napi-rs analog
/// of `quadraui-py`'s own `SharedWaker`. A `Mutex` rather than a bare field:
/// [`QuiAppWaker`] must be `Send + Sync` so a background thread can hold
/// and call it, which rules out `Rc<RefCell<_>>`; the lock is only ever
/// held for the instant of a clone or a swap, never across a call into the
/// wrapped closure itself.
type SharedWaker = Arc<Mutex<Arc<dyn Fn(UserPayload) + Send + Sync>>>;

/// The real terminal- or window-backed runner [`QuiApp::start_tui`]/a
/// future `startNative` moves this app's whole [`NodeAppLogic`] into --
/// see this module's doc's "Live run loop" section for the overall shape.
/// Only the TUI variant exists today; a `gtk`/`macos`/`win` variant is
/// each a separate backend's own follow-on (this crate's `gtk`/`macos`/
/// `win` Cargo features today only forward to `quadraui`'s/
/// `quadraui-binding-core`'s same-named feature -- see `Cargo.toml`'s own
/// comment -- with no napi-visible `startNative` method built on them
/// yet). TUI is verified here, on any host, through the black-box
/// `quadraui-node/tests/live_runner.test.ts` suite; a `startNative`
/// variant needs its own backend-specific issue verified on that
/// backend's own capability-lane host (`gtk` on a Linux box with GTK4
/// installed, `macos`/`win` only on their own platform) -- the same
/// one-backend-per-issue split `CLAUDE.md`'s own doc describes for
/// `quadraui` itself, applied here one level down inside this single
/// crate's own feature table.
enum LiveRunner {
    /// A live terminal takeover via [`TuiRunner`] -- [`QuiApp::start_tui`].
    Tui(TuiRunner<NodeAppLogic>),
}

impl LiveRunner {
    /// Mutable access to whichever variant's own [`NodeAppLogic`] -- the
    /// napi-rs analog of [`TuiDriver::app_mut`] for a live runner instead
    /// of the headless driver, needed for the same reason: [`QuiApp::guarded`]
    /// stashes its live `Env` on it before running its body.
    fn app_mut(&mut self) -> &mut NodeAppLogic {
        match self {
            LiveRunner::Tui(runner) => runner.app_mut(),
        }
    }

    /// One non-blocking pass -- [`TuiRunner::step`] for the one variant
    /// that exists today. Never blocks, mirroring every backend's own
    /// non-blocking `step()` -- a `gtk`/`macos`/`win` variant drives its
    /// own runner's `step()` the same way once it exists.
    fn step(&mut self) -> io::Result<StepOutcome> {
        match self {
            LiveRunner::Tui(runner) => runner.step(),
        }
    }

    /// This variant's own [`Backend::waker`] closure -- read right after
    /// construction to swap [`QuiApp::current_waker`] to it, and again
    /// (via [`QuiApp::driver`]'s headless backend instead) once this
    /// runner's own `step` reports [`StepOutcome::Exited`].
    fn waker(&self) -> Arc<dyn Fn(UserPayload) + Send + Sync> {
        match self {
            LiveRunner::Tui(runner) => runner.backend().waker(),
        }
    }
}

/// One action a [`QuiAppActions`] method recorded, already decoded and
/// validated, waiting for [`flush_deferred_actions`] to apply it -- the
/// napi-rs analog of `quadraui-py`'s own `DeferredAction`, covering the
/// subset of its variants this crate's `actions()` exposes today
/// (dialogs/menus/toasts/notifications/OS file actions; no window-control
/// or tray surface exists on this crate's `QuiApp` yet to mirror). Each
/// variant carries exactly the already-decoded arguments the matching
/// [`QuiAppActions`] method passes to the same underlying
/// `quadraui`/`quadraui_binding_core` call, so applying one in
/// [`flush_deferred_actions`] is a direct Rust call with no further
/// decoding or validation left to do.
enum DeferredAction {
    MessageDialog(quadraui::MessageDialogOptions),
    ContextMenu(Vec<ContextMenuItem>, Point),
    Toast(Toast, ToastCorner),
    Notification(Notification),
    OpenPath(PathBuf),
    RevealInFileManager(PathBuf),
    MoveToTrash(PathBuf),
    RestoreFromTrash(PathBuf),
}

/// The [`AppLogic`] a [`QuiApp`] drives: a thin napi-rs leaf around
/// [`BindingHost`] (the six fixed primitive slots, any additive slots, and
/// the backend-settings replay store -- see that type's own doc), the
/// [`InteractionEngine`] every binding's `AppLogic` holds alongside it, and
/// the registered-callback bridge below -- the napi-rs analog of
/// `quadraui-py`'s `PyAppLogic`.
#[derive(Default)]
struct NodeAppLogic {
    /// The host-side slot/replay state this binding is a thin napi-rs leaf
    /// over -- see [`BindingHost`]'s own doc.
    host: BindingHost,
    /// The language-neutral `event -> hit -> semantic` orchestration engine
    /// this binding is a thin napi-rs leaf over -- see
    /// `quadraui_binding_core::engine`'s module doc. [`AppLogic::render`]
    /// feeds it this frame's painted hits via [`InteractionEngine::set_hits`];
    /// [`AppLogic::handle`] forwards dispatch to it via a [`NodeEventSink`]
    /// that bridges back into [`Self::handler`].
    engine: InteractionEngine,
    /// Registered via `QuiApp::setHandler`; called with the JS-converted
    /// event payload for every dispatched [`UiEvent`].
    handler: Option<NodeCallback>,
    /// Registered via `QuiApp::setTickHandler`; called with no meaningful
    /// argument whenever the runner advances with no event queued.
    on_tick: Option<NodeCallback>,
    /// Registered via `QuiApp::setRender`; called with no meaningful
    /// argument once per frame and expected to return a plain object
    /// decodable via [`decode_render_output`] (the same flat-vs-tree shape
    /// every binding's render callback returns). When set, this overrides
    /// [`Self::host`]'s static slots for every render.
    render_callback: Option<NodeCallback>,
    /// Set by [`AppLogic::handle`]/[`AppLogic::tick`]/[`AppLogic::render`]
    /// when the JS callback they invoke raises, or when this binding's own
    /// JS-value conversion fails. `step_once` takes this and re-raises it
    /// out of `step`/`run` instead of letting it vanish at the `AppLogic`
    /// boundary -- mirrors `quadraui-py`'s own `pending_error` field; see
    /// [`record_error`] for the first-error-wins policy every write goes
    /// through.
    pending_error: Rc<RefCell<Option<Error>>>,
    /// Actions a [`QuiAppActions`] method queued (typically called from
    /// inside the registered event handler, mid-dispatch) and not yet
    /// applied. Drained by [`flush_deferred_actions`] right after the
    /// dispatch that queued them finishes -- mirrors `quadraui-py`'s own
    /// `deferred_actions` field; see that function's doc for why a queued
    /// action must be applied as a direct Rust call rather than through
    /// another `QuiApp` method call.
    deferred_actions: Rc<RefCell<VecDeque<DeferredAction>>>,
    /// The live `Env` for whichever `QuiApp` method call is currently
    /// driving this app, set by [`QuiApp::guarded`] right before it steps
    /// into the driver and read back out by [`Self::handle`]/[`Self::tick`]/
    /// [`Self::render`] to call a registered callback. `Env` is `Copy` (a
    /// bare pointer wrapper) and this cell is only ever read from inside
    /// the same synchronous call that just set it, so a value left over
    /// from an earlier, already-returned call is simply overwritten before
    /// its next use and never read in between -- nothing here keeps an
    /// N-API `Env` alive past the native call that handed it out.
    env: Cell<Option<Env>>,
}

impl NodeAppLogic {
    /// Whether `slot` names one of this app's six fixed primitive slots.
    fn is_known_slot(slot: i64) -> bool {
        BindingHost::is_known_slot(slot)
    }

    /// Whether `slot` currently holds a descriptor.
    fn slot_occupied(&self, slot: i64) -> bool {
        self.host.slot_occupied(slot)
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

impl WindowCloseHandling for NodeAppLogic {
    fn has_window_close_handler(&self) -> bool {
        self.handler.is_some()
    }
}

impl AppLogic for NodeAppLogic {
    type AreaId = ();

    /// Replays [`Self::host`]'s accelerators and backend-settings fields
    /// onto `backend` -- [`BindingHost::setup`] -- mirrors `quadraui-py`'s
    /// `PyAppLogic::setup`.
    fn setup(&mut self, backend: &mut dyn Backend) {
        self.host.setup(backend);
    }

    /// With no render callback registered, paints [`Self::host`]'s six
    /// fixed slots plus the anchored overlay pass every binding's `render`
    /// runs -- see [`render_overlays`]'s own doc. With one registered,
    /// invokes it instead via [`render_output_from_callback`] and paints
    /// whatever flat/tree frame it returned, skipping [`Self::host`]
    /// entirely for this frame -- mirrors `quadraui-py`'s own `render`.
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
        let Some(env) = self.env.get() else {
            // No live `Env` for this render pass -- every `QuiApp` method
            // that can trigger one sets it first (see `Self::env`'s doc),
            // so this is unreached in practice; treated the same as
            // `render_output_from_callback`'s own failure path (skip
            // painting this frame) rather than panicking with no caller
            // around to catch it.
            render_overlays(&self.engine, backend, hits);
            return;
        };
        match render_output_from_callback(env, callback, &self.pending_error) {
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
    /// frame's tab stops directly from the hit map [`Self::render`] just
    /// fed it via [`InteractionEngine::set_hits`].
    fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        self.engine.tab_stops()
    }

    /// With no handler registered, every event is accepted with no side
    /// effect. With one registered, wraps it in a [`NodeEventSink`] and
    /// forwards to [`InteractionEngine::handle`], which owns the full
    /// `event -> hit -> semantic` orchestration -- mirrors `quadraui-py`'s
    /// own `handle`. Drains [`Self::deferred_actions`] via
    /// [`flush_deferred_actions`] right after dispatch, against this same
    /// `backend`, folding the result into the returned [`Reaction`] so a
    /// queued action that actually applied something still gets its
    /// repaint even when the handler itself returned `Continue`.
    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let reaction = match (self.handler.as_ref(), self.env.get()) {
            (Some(handler), Some(env)) => {
                let mut sink = NodeEventSink {
                    handler,
                    env,
                    pending_error: &self.pending_error,
                    render_callback_present: self.render_callback.is_some(),
                };
                self.engine.handle(event, backend, &mut sink)
            }
            _ => Reaction::Continue,
        };
        let applied = flush_deferred_actions(backend, self);
        reaction.merge(if applied {
            Reaction::Redraw
        } else {
            Reaction::Continue
        })
    }

    /// Same [`flush_deferred_actions`] drain as [`Self::handle`] -- a
    /// deferred action can be queued from a tick callback too, since
    /// `QuiApp::actions()` is obtained once and shared with whatever
    /// callback the app registers, not just the event handler.
    fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
        let reaction = match (self.on_tick.as_ref(), self.env.get()) {
            (Some(on_tick), Some(env)) => {
                let render_callback_present = self.render_callback.is_some();
                match invoke_callback(env, on_tick, &self.pending_error) {
                    Some(result) => callback_reaction_for_render(
                        bool_from_return(result),
                        render_callback_present,
                    ),
                    None => Reaction::Exit,
                }
            }
            _ => Reaction::Continue,
        };
        let applied = flush_deferred_actions(backend, self);
        reaction.merge(if applied {
            Reaction::Redraw
        } else {
            Reaction::Continue
        })
    }
}

/// Records `err` into `pending_error`, but only if no earlier error in the
/// same dispatch already claimed the slot -- the napi-rs analog of
/// `quadraui-py`'s own `record_error`. Without this, a handler raise
/// (recorded first) followed by the render callback also raising while
/// painting the same frame would have the render error silently clobber the
/// handler's -- the one a caller actually wants to see, since it is closer
/// to the root cause.
fn record_error(pending_error: &Rc<RefCell<Option<Error>>>, err: Error) {
    let mut slot = pending_error.borrow_mut();
    if slot.is_none() {
        *slot = Some(err);
    }
}

/// Takes and re-raises any error a registered callback recorded during the
/// `step`/render pass that just ran -- mirrors `quadraui-py`'s
/// `take_pending_error`.
fn take_pending_error(app: &mut NodeAppLogic) -> Result<()> {
    if let Some(err) = app.pending_error.borrow_mut().take() {
        return Err(err);
    }
    Ok(())
}

/// Calls `callback` (borrowed back from its stored [`NodeCallback`] via
/// `env`) with a single JSON `null` placeholder argument -- the napi-rs
/// stand-in for a Python `call0`'s "no arguments" (see [`NodeCallback`]'s
/// doc for why this crate always calls with exactly one JS value; a plain
/// JS function declared with zero parameters, the overwhelmingly common
/// shape for a tick/render callback, simply ignores it). On any failure --
/// borrowing the reference back, converting the placeholder, or the call
/// itself raising -- records the error via [`record_error`] and returns
/// `None`, leaving the caller to treat this the same as an explicit exit
/// request.
fn invoke_callback(
    env: Env,
    callback: &NodeCallback,
    pending_error: &Rc<RefCell<Option<Error>>>,
) -> Option<Unknown<'static>> {
    let placeholder: Unknown<'static> = match env.to_js_value(&JsonValue::Null) {
        Ok(value) => value,
        Err(err) => {
            record_error(pending_error, err);
            return None;
        }
    };
    let func = match callback.borrow_back(&env) {
        Ok(func) => func,
        Err(err) => {
            record_error(pending_error, err);
            return None;
        }
    };
    match func.call(placeholder) {
        Ok(result) => Some(result),
        Err(err) => {
            record_error(pending_error, err);
            None
        }
    }
}

/// Narrows a callback's JS return value to `Option<bool>` by checking
/// `ValueType::Boolean` -- the napi-rs analog of `quadraui-py`'s
/// `value.extract::<bool>().ok()`: a non-boolean return (`undefined`, an
/// object, a number, ...), including the common case of no explicit
/// `return`, narrows to `None` rather than raising, so only a literal `true`/
/// `false` ever reaches [`callback_reaction`]/[`callback_reaction_for_render`]/
/// [`window_close_reaction`] as `Some`.
fn bool_from_return(value: Unknown<'static>) -> Option<bool> {
    match value.get_type() {
        // SAFETY: just confirmed `value` is a JS boolean.
        Ok(ValueType::Boolean) => unsafe { value.cast::<bool>() }.ok(),
        _ => None,
    }
}

/// Invokes a registered render callback and decodes its returned plain
/// object into a [`RenderOutput`] via [`decode_render_output`], through the
/// same [`to_descriptor_value`] materialization every other kind-dispatched
/// decoder in this crate uses -- the napi-rs analog of `quadraui-py`'s own
/// `render_output_from_callback`. On any failure -- the callback raising,
/// `invoke_callback` itself failing, or the returned value not decoding --
/// records the error in `pending_error` (via [`invoke_callback`] for the
/// former, directly here for the latter) and returns `None`, leaving the
/// caller to skip drawing this frame rather than paint a half-decoded one.
fn render_output_from_callback(
    env: Env,
    callback: &NodeCallback,
    pending_error: &Rc<RefCell<Option<Error>>>,
) -> Option<RenderOutput> {
    let result = invoke_callback(env, callback, pending_error)?;
    let value = match to_descriptor_value(result) {
        Ok(value) => value,
        Err(detail) => {
            record_error(
                pending_error,
                invalid_descriptor_error(format!("render: {detail}")),
            );
            return None;
        }
    };
    match decode_render_output("render", &value) {
        Ok(output) => Some(output),
        Err(err) => {
            record_error(pending_error, invalid_descriptor_error(err));
            None
        }
    }
}

/// Bridges [`EventSink`] to this binding's registered JS event handler:
/// converts `event` to a JS value via [`encode_via_json`] (napi-rs's analog
/// of `pythonize::pythonize`), attaching `hit_detail` under the `"hit"` key
/// when present, calls the handler, and translates its return value into a
/// [`Reaction`] -- the napi-rs analog of `quadraui-py`'s `PyEventSink`.
/// [`InteractionEngine::handle`] calls this once per dispatched event
/// (twice for a `MouseDown`/`DoubleClick` that also changes focus: once for
/// the synthetic `FocusChanged`, once for the click itself), so every call
/// site goes through identical error handling.
struct NodeEventSink<'a> {
    handler: &'a NodeCallback,
    env: Env,
    pending_error: &'a Rc<RefCell<Option<Error>>>,
    render_callback_present: bool,
}

impl EventSink for NodeEventSink<'_> {
    fn dispatch(&mut self, event: &UiEvent, hit_detail: Option<&HitDetail>) -> Reaction {
        // `UiEvent::User`'s `Serialize` impl deliberately errors (its
        // payload is an opaque `Arc<dyn Any>` with no wire representation
        // to invent -- see `quadraui::UserPayload`'s own doc) -- matched
        // here, before `encode_via_json` ever sees it, so a wake delivered
        // through `QuiAppWaker::post`/`::wake`/`::postAfter` reaches the
        // handler as a plain `{"User": value}` object instead of that
        // error turning into a recorded `InternalError` and
        // `Reaction::Exit` that silently kills the app the first time
        // anything wakes it this way. No `hit_detail` ever accompanies a
        // `User` event (it carries no screen position), so the "hit" key
        // this function attaches below never applies here.
        if let UiEvent::User(payload) = event {
            return self.dispatch_user(payload);
        }
        let payload: Unknown<'static> = match encode_via_json(self.env, "dispatch", event) {
            Ok(value) => value,
            Err(err) => {
                record_error(self.pending_error, err);
                return Reaction::Exit;
            }
        };
        let payload = match hit_detail {
            Some(detail) => match attach_hit(self.env, payload, detail) {
                Ok(value) => value,
                Err(err) => {
                    record_error(self.pending_error, err);
                    return Reaction::Exit;
                }
            },
            None => payload,
        };
        let func = match self.handler.borrow_back(&self.env) {
            Ok(func) => func,
            Err(err) => {
                record_error(self.pending_error, err);
                return Reaction::Exit;
            }
        };
        match func.call(payload) {
            // `WindowClose` is the one event whose no-opinion default (no
            // explicit `return`, i.e. a non-boolean/`undefined` return)
            // must let the close proceed rather than continue --
            // `window_close_reaction`'s own doc.
            Ok(result) if matches!(event, UiEvent::WindowClose) => {
                window_close_reaction(bool_from_return(result), self.render_callback_present)
            }
            Ok(result) => {
                callback_reaction_for_render(bool_from_return(result), self.render_callback_present)
            }
            Err(err) => {
                record_error(self.pending_error, err);
                Reaction::Exit
            }
        }
    }
}

impl NodeEventSink<'_> {
    /// Delivers a `UiEvent::User(payload)` as `{"User": value}` instead of
    /// going through [`encode_via_json`] -- see [`EventSink::dispatch`]'s
    /// own doc for why this is special-cased ahead of it. [`QuiAppWaker`]
    /// is this crate's only producer of a [`UserPayload`], always wrapping
    /// a [`JsonValue`] (see that type's own doc for why `Send` rules out a
    /// raw JS value), so `downcast_ref::<JsonValue>` recovers it directly;
    /// any other payload shape (none reachable from this crate today, but
    /// the same opaque `UserPayload` a native tray/timer wake or a future
    /// binding sharing this engine could deliver) surfaces as
    /// `{"User": null}` instead of panicking.
    fn dispatch_user(&mut self, payload: &UserPayload) -> Reaction {
        let value = payload
            .downcast_ref::<JsonValue>()
            .cloned()
            .unwrap_or(JsonValue::Null);
        let wrapped = serde_json::json!({ "User": value });
        let js_payload: Unknown<'static> = match self.env.to_js_value(&wrapped) {
            Ok(value) => value,
            Err(err) => {
                record_error(self.pending_error, err);
                return Reaction::Exit;
            }
        };
        let func = match self.handler.borrow_back(&self.env) {
            Ok(func) => func,
            Err(err) => {
                record_error(self.pending_error, err);
                return Reaction::Exit;
            }
        };
        match func.call(js_payload) {
            Ok(result) => {
                callback_reaction_for_render(bool_from_return(result), self.render_callback_present)
            }
            Err(err) => {
                record_error(self.pending_error, err);
                Reaction::Exit
            }
        }
    }
}

/// Attaches `detail` to `payload` under its `"hit"` key, for a dispatched
/// event that landed on a specific sub-area -- the napi-rs analog of
/// `quadraui-py`'s own hit-detail attachment in `PyEventSink::dispatch`.
/// `payload` not being a plain object (e.g. the bare-string JS value a unit
/// `UiEvent` variant with no hit-testable sub-area converts to) is mapped
/// to an `InternalError`-tagged failure rather than silently dropping the
/// hit detail, mirroring `quadraui-py`'s own fold for the same mismatch.
fn attach_hit(env: Env, payload: Unknown<'static>, detail: &HitDetail) -> Result<Unknown<'static>> {
    match payload.get_type()? {
        ValueType::Object => {
            // SAFETY: just confirmed `payload` is a JS object.
            let mut obj: Object = unsafe { payload.cast() }?;
            let hit_value = encode_via_json(env, "attach_hit", detail)?;
            obj.set("hit", hit_value)?;
            Ok(obj.to_unknown())
        }
        other => Err(invalid_descriptor_error(format!(
            "failed to attach hit detail: converted event payload was {other:?}, not an object"
        ))),
    }
}

/// Applies every action queued on `app.deferred_actions` directly against
/// `backend`/`app`, then drops them -- called from [`NodeAppLogic::handle`]/
/// [`NodeAppLogic::tick`] themselves, right after each dispatch, while the
/// caller already holds a plain Rust `&mut` to both (never through another
/// napi-rs method call on `QuiApp` itself, which is exactly the re-entrant
/// call a [`QuiAppActions`] method queued this action to avoid) -- the
/// napi-rs analog of `quadraui-py`'s own `flush_deferred_actions`. A failing
/// `openPath`/`revealInFileManager`/`moveToTrash`/`restoreFromTrash` records
/// its error on `app.pending_error`, the same cell `handle`/`tick`/the
/// render callback already use to report a failure back out of `step`/`run`.
///
/// Returns whether anything was applied, so the caller can fold that into
/// the [`Reaction`] it returns and only pay for an extra repaint when a
/// queued action actually changed something visible.
fn flush_deferred_actions(backend: &mut dyn Backend, app: &mut NodeAppLogic) -> bool {
    let mut applied = false;
    loop {
        let action = app.deferred_actions.borrow_mut().pop_front();
        let Some(action) = action else { break };
        applied = true;
        match action {
            DeferredAction::MessageDialog(opts) => {
                app.engine.open_message_dialog(opts, backend);
            }
            DeferredAction::ContextMenu(items, position) => {
                app.engine.open_context_menu(items, position, backend);
            }
            DeferredAction::Toast(toast, corner) => {
                app.engine.show_toast(toast, corner);
            }
            DeferredAction::Notification(notification) => {
                backend.services().send_notification(notification);
            }
            DeferredAction::OpenPath(path) => {
                if let Err(err) = backend.services().open_path(&path) {
                    record_error(
                        &app.pending_error,
                        internal_error(format!("open_path: {err:?}")),
                    );
                }
            }
            DeferredAction::RevealInFileManager(path) => {
                if let Err(err) = backend.services().reveal_in_file_manager(&path) {
                    record_error(
                        &app.pending_error,
                        internal_error(format!("reveal_in_file_manager: {err:?}")),
                    );
                }
            }
            DeferredAction::MoveToTrash(path) => {
                if let Err(err) = backend.services().move_to_trash(&path) {
                    record_error(
                        &app.pending_error,
                        internal_error(format!("move_to_trash: {err:?}")),
                    );
                }
            }
            DeferredAction::RestoreFromTrash(path) => {
                if let Err(err) = backend.services().restore_from_trash(&path) {
                    record_error(
                        &app.pending_error,
                        internal_error(format!("restore_from_trash: {err:?}")),
                    );
                }
            }
        }
    }
    applied
}

/// Rejects a zero `width`/`height`, otherwise narrows both to the `u16`
/// [`TuiDriver`]/[`quadraui::tui::testing::TuiBackend`] take -- the napi-rs
/// analog of `quadraui-py`'s own zero-dimension check in `QuiApp::new`/
/// `resize`.
fn checked_dimensions(width: u16, height: u16) -> Result<(u16, u16)> {
    if width == 0 || height == 0 {
        return Err(Error::from_reason("width and height must both be non-zero"));
    }
    Ok((width, height))
}

// ─── Descriptor decode adapter ──────────────────────────────────────────────
//
// See the module doc's "Descriptor decode adapter" section for the overall
// shape; the functions below are its pieces.

/// Maps a decode failure -- this crate's own JS-value walk
/// ([`to_descriptor_value`]) or a `quadraui_binding_core` decoder's
/// [`DecodeError`] -- to a JS `Error` whose message is tagged
/// `InvalidDescriptorError`, the napi-rs analog of `quadraui-py`'s
/// `InvalidDescriptorError` exception subclass (see the module doc for why
/// this crate tags the message instead of raising a distinct JS class).
fn invalid_descriptor_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("InvalidDescriptorError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for an already-occupied fixed slot
/// or additive-primitive kind -- the napi-rs analog of `quadraui-py`'s
/// `SlotOccupiedError` exception subclass.
fn slot_occupied_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("SlotOccupiedError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for a failure internal to this
/// binding rather than caused by caller input -- e.g. a deferred OS-service
/// action ([`flush_deferred_actions`]) failing -- the napi-rs analog of
/// `quadraui-py`'s `InternalError` exception subclass.
fn internal_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("InternalError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for a byte buffer whose length
/// doesn't match the `width`/`height` it's supposed to encode --
/// [`QuiApp::clipboard_write_image`]'s own check -- the napi-rs analog of
/// `quadraui-py`'s `InvalidDimensionsError` exception subclass.
fn invalid_dimensions_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("InvalidDimensionsError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for a `role` string that isn't one
/// of [`font_role`]'s recognized names -- [`QuiApp::measure_text`]'s own
/// check -- the napi-rs analog of `quadraui-py`'s `InvalidFontRoleError`
/// exception subclass.
fn invalid_font_role_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("InvalidFontRoleError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for a `QuiApp` method called
/// re-entrantly from inside a registered event handler/tick/render
/// callback's own JS call stack -- see [`QuiApp::check_not_dispatching`]'s
/// doc for why napi-rs needs this guard where PyO3 does not.
fn reentrant_call_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("ReentrantCallError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for a `QuiApp` method called while
/// `startTui`/`startNative` is active -- see
/// [`QuiApp::check_not_live`]'s doc for why.
fn live_runner_active_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("LiveRunnerActiveError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for `step`/`run` called after a
/// live `startTui`/`startNative` session this app once held has already
/// ended -- see [`QuiApp::live_exited`]'s doc for why this is sticky
/// rather than silently falling back to ticking the orphaned headless
/// app.
fn live_runner_exited_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("LiveRunnerExitedError: {detail}"))
}

/// Decodes `descriptor` straight into `T` via napi's own
/// `Env::from_js_value` -- the napi-rs analog of `quadraui-py`'s
/// `pythonize::depythonize`, for any descriptor type that derives
/// `Deserialize` with no byte-string/non-finite-float field, and no
/// newtype-struct-identifier field (see [`decode_via_json`]), at risk (the
/// six fixed primitive slots, every overlay's own descriptor shape).
fn decode_direct<T: DeserializeOwned>(env: Env, what: &str, descriptor: Unknown) -> Result<T> {
    env.from_js_value(descriptor)
        .map_err(|err| invalid_descriptor_error(format!("{what}: {err}")))
}

/// Converts `value` to a JS value by materializing it through a plain
/// [`serde_json::Value`] first, rather than handing it straight to
/// `Env::to_js_value` -- the encode-direction counterpart of
/// [`decode_via_core`]'s own materialize-first shape, needed for a
/// different reason: napi 3's own `Serializer` impl
/// (`napi::js_values::ser`) builds the right `{"VariantName": ...}`
/// wrapper for `serialize_tuple_variant`/`serialize_struct_variant` but
/// then discards it, returning the un-tagged inner value instead --
/// verified empirically (a multi-field tuple variant or any struct variant,
/// e.g. [`UiEvent::CompletionItemActivated`] or
/// `quadraui_binding_core::HitDetail::TreeRow`, reaches a registered
/// handler with its own tag missing entirely). [`serde_json::Value`]'s own
/// `Serialize` impl never calls those two `Serializer` methods -- by the
/// time `value` has been folded into a `Value` tree, there is no more
/// enum-variant information left to tag, only plain maps/arrays/scalars,
/// which `Env::to_js_value` converts correctly. Every call site that hands
/// a `quadraui`/`quadraui_binding_core` value to JS goes through this
/// rather than `Env::to_js_value` directly, so a future field or variant
/// addition on either side of that boundary is covered by construction
/// instead of needing this same bug rediscovered per call site.
fn encode_via_json<T: Serialize>(env: Env, what: &str, value: &T) -> Result<Unknown<'static>> {
    let json = serde_json::to_value(value).map_err(|err| {
        invalid_descriptor_error(format!("{what}: failed to convert to JSON: {err}"))
    })?;
    env.to_js_value(&json)
        .map_err(|err| invalid_descriptor_error(format!("{what}: failed to convert to JS: {err}")))
}

/// Decodes `descriptor` by first materializing it into one
/// [`serde_json::Value`] via [`to_descriptor_value`], then handing `&value`
/// to `core` -- the shared tail of every kind-dispatched decoder this crate
/// feeds through the same medium `quadraui_binding_core`'s own tests use.
fn decode_via_core<T>(
    what: &str,
    descriptor: Unknown,
    core: impl FnOnce(&JsonValue) -> std::result::Result<T, DecodeError>,
) -> Result<T> {
    let value = to_descriptor_value(descriptor)
        .map_err(|detail| invalid_descriptor_error(format!("{what}: {detail}")))?;
    core(&value).map_err(invalid_descriptor_error)
}

/// Decodes `descriptor` by first materializing it into one
/// [`serde_json::Value`] via [`to_descriptor_value`], then deserializing
/// through `serde_json::Value`'s own `Deserializer` impl rather than
/// `Env::from_js_value` directly -- works around a gap in napi 3's own
/// `Deserializer` (`napi::js_values::de::De`): every method it doesn't
/// implement by hand, including `deserialize_newtype_struct`, forwards
/// straight to `deserialize_any` via serde's `forward_to_deserialize_any!`
/// macro, so a standalone one-field tuple struct with no other way to be
/// constructed from outside `quadraui` itself ([`quadraui::AcceleratorId`],
/// [`quadraui::GlobalShortcutId`] -- deliberately opaque identifiers, see
/// either type's own doc) fails to decode from the bare JS string/number
/// its single field would otherwise come from, with a confusing "invalid
/// type: string ..., expected tuple struct ..." (verified empirically).
/// `serde_json::Value`'s own `Deserializer` implements
/// `deserialize_newtype_struct` the standard way (forwarding to
/// `visit_newtype_struct` without first collapsing to a scalar), so it
/// decodes the same input correctly. Every other scalar/struct/enum shape
/// `Env::from_js_value` already decodes correctly (see [`decode_direct`]),
/// so this is reserved for exactly the newtype-struct-identifier case --
/// [`quadraui::Accelerator`] (its own `id` field is one) and
/// `GlobalShortcutId` itself, plus every descriptor that can carry one of
/// those nested inside it: [`Self::register_accelerator`] and
/// [`Self::register_global_shortcut`]/[`Self::unregister_global_shortcut`]
/// decode directly, while `pushEvent`'s [`UiEvent`] and
/// `queueDialogEvents`' `Vec<UiEvent>` need it for exactly
/// [`UiEvent::Accelerator`]'s `AcceleratorId` field and
/// [`UiEvent::GlobalShortcut`]'s `GlobalShortcutId` field.
fn decode_via_json<T: DeserializeOwned>(what: &str, descriptor: Unknown) -> Result<T> {
    let value = to_descriptor_value(descriptor)
        .map_err(|detail| invalid_descriptor_error(format!("{what}: {detail}")))?;
    serde_json::from_value(value).map_err(|err| invalid_descriptor_error(format!("{what}: {err}")))
}

/// Converts a JS value into a [`serde_json::Value`] tree -- this crate's
/// own walk, not a reuse of `quadraui_binding_core::wire`'s private
/// `materialize` visitor (not reachable from here) or napi's own
/// `FromNapiValue for serde_json::Value` (reachable, but not reused -- see
/// the module doc's "Descriptor decode adapter" section for why). Folds a
/// `Buffer`/`Uint8Array`/`ArrayBuffer` into a [`JsonValue::Array`] of its
/// byte values before any object/array branch gets a chance to see it (the
/// shape `Vec<u8>`'s own `Deserialize` impl expects, so
/// `quadraui::ImageSource::Bytes` keeps decoding through `{"Bytes": ...}`),
/// and rejects a non-finite float (`NaN`/`Infinity`) outright rather than
/// letting it alias `null`.
///
/// Every `unsafe { value.cast() }` below immediately follows the
/// `get_type()`/`is_buffer()`/`is_array()`/... check that proves `value` is
/// shaped the way the cast target expects.
fn to_descriptor_value(value: Unknown) -> std::result::Result<JsonValue, String> {
    match value.get_type().map_err(|err| err.reason)? {
        ValueType::Undefined | ValueType::Null => Ok(JsonValue::Null),
        ValueType::Boolean => {
            // SAFETY: `get_type` just confirmed this value is a JS boolean.
            let b: bool = unsafe { value.cast() }.map_err(|err| err.reason)?;
            Ok(JsonValue::Bool(b))
        }
        ValueType::Number => {
            // SAFETY: `get_type` just confirmed this value is a JS number.
            let n: f64 = unsafe { value.cast() }.map_err(|err| err.reason)?;
            if !n.is_finite() {
                return Err(format!("non-finite number {n} is not supported"));
            }
            // Mirrors napi's own `De::deserialize_any`: a whole number
            // decodes as an integer, everything else as a float.
            Ok(if (n.trunc() - n).abs() < f64::EPSILON {
                JsonValue::Number((n as i64).into())
            } else {
                serde_json::Number::from_f64(n)
                    .map(JsonValue::Number)
                    .unwrap_or(JsonValue::Null)
            })
        }
        ValueType::String => {
            // SAFETY: `get_type` just confirmed this value is a JS string.
            let s: String = unsafe { value.cast() }.map_err(|err| err.reason)?;
            Ok(JsonValue::String(s))
        }
        ValueType::Object => {
            if value.is_buffer().map_err(|err| err.reason)? {
                // SAFETY: `is_buffer` just confirmed this value is a Buffer.
                let bytes: BufferSlice = unsafe { value.cast() }.map_err(|err| err.reason)?;
                Ok(bytes_to_value(&bytes))
            } else if value.is_typedarray().map_err(|err| err.reason)? {
                // SAFETY: `is_typedarray` just confirmed this value is a
                // typed array; `Uint8Array::from_napi_value` itself still
                // rejects one whose element type isn't `Uint8`.
                let bytes: Uint8Array = unsafe { value.cast() }.map_err(|err| err.reason)?;
                Ok(bytes_to_value(&bytes))
            } else if value.is_arraybuffer().map_err(|err| err.reason)? {
                // SAFETY: `is_arraybuffer` just confirmed this value is an
                // ArrayBuffer.
                let bytes: ArrayBuffer = unsafe { value.cast() }.map_err(|err| err.reason)?;
                Ok(bytes_to_value(&bytes))
            } else if value.is_array().map_err(|err| err.reason)? {
                // SAFETY: `is_array` just confirmed this value is a JS array.
                let arr: Array = unsafe { value.cast() }.map_err(|err| err.reason)?;
                let mut elements = Vec::with_capacity(arr.len() as usize);
                for index in 0..arr.len() {
                    let element: Option<Unknown> = arr.get(index).map_err(|err| err.reason)?;
                    elements.push(match element {
                        Some(element) => to_descriptor_value(element)?,
                        None => JsonValue::Null,
                    });
                }
                Ok(JsonValue::Array(elements))
            } else {
                // SAFETY: every other `ValueType::Object` is a plain object.
                let obj: Object = unsafe { value.cast() }.map_err(|err| err.reason)?;
                let mut map = serde_json::Map::new();
                for key in Object::keys(&obj).map_err(|err| err.reason)? {
                    let property: Option<Unknown> = obj.get(&key).map_err(|err| err.reason)?;
                    if let Some(property) = property {
                        map.insert(key, to_descriptor_value(property)?);
                    }
                }
                Ok(JsonValue::Object(map))
            }
        }
        other => Err(format!(
            "{other:?} cannot be represented as a descriptor value"
        )),
    }
}

/// Folds a byte slice into the [`JsonValue::Array`] of `u8` numbers
/// `Vec<u8>`'s own `Deserialize` impl expects -- shared by
/// [`to_descriptor_value`]'s `Buffer`/`Uint8Array`/`ArrayBuffer` branches.
fn bytes_to_value(bytes: &[u8]) -> JsonValue {
    JsonValue::Array(
        bytes
            .iter()
            .map(|byte| JsonValue::Number((*byte).into()))
            .collect(),
    )
}

/// A handle, returned by `QuiApp::actions()`, that queues a dialog/
/// context-menu/toast/notification/file action for `flush_deferred_actions`
/// to apply right after the dispatch that queued it finishes -- the
/// napi-rs analog of `quadraui-py`'s own `QuiAppActions`, exposing the
/// subset of its methods [`DeferredAction`]'s own doc names. A registered
/// event handler already holds this `QuiApp`'s one `&mut self` for the
/// whole dispatch that invoked it, so calling, say, `app.insertButton(...)`
/// straight from inside it raises `ReentrantCallError` -- see
/// `QuiApp::check_not_dispatching`. Calling the matching method here
/// instead touches no borrow of `QuiApp` itself: each method below decodes
/// and validates its arguments immediately (so a malformed descriptor still
/// raises synchronously, from the handler's own call) and records the
/// already-decoded action on a queue shared with the owning `QuiApp`,
/// rather than applying it right away.
///
/// Call `QuiApp::actions()` once up front, from top-level script code, and
/// share the result with whatever closure is registered as the event/tick
/// handler.
#[napi]
pub struct QuiAppActions {
    queue: Rc<RefCell<VecDeque<DeferredAction>>>,
}

#[napi]
impl QuiAppActions {
    /// Queues a [`DeferredAction::MessageDialog`] -- same arguments, same
    /// decoding, as `quadraui-py`'s `QuiApp.show_message_dialog`/
    /// `QuiAppActions.show_message_dialog`. `buttons` decodes as a JS array
    /// of plain objects shaped like [`DialogButton`].
    #[napi(js_name = "showMessageDialog")]
    pub fn show_message_dialog(
        &self,
        env: Env,
        title: String,
        body: String,
        buttons: Unknown,
        severity: Option<String>,
    ) -> Result<()> {
        let buttons: Vec<DialogButton> = decode_direct(env, "showMessageDialog", buttons)?;
        let severity = severity
            .map(|s| {
                dialog_severity(&s)
                    .map_err(|err| invalid_descriptor_error(format!("showMessageDialog: {err}")))
            })
            .transpose()?;
        let opts = message_dialog_options_from_buttons(title, body, buttons, severity)
            .map_err(|err| invalid_descriptor_error(format!("showMessageDialog: {err}")))?;
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::MessageDialog(opts));
        Ok(())
    }

    /// Queues a [`DeferredAction::ContextMenu`] -- same arguments, same
    /// decoding, as `quadraui-py`'s `show_context_menu`. `items` decodes as
    /// a JS array of plain objects shaped like [`ContextMenuItem`]; `point`
    /// as a plain `{x, y}` object.
    #[napi(js_name = "showContextMenu")]
    pub fn show_context_menu(&self, env: Env, items: Unknown, point: Unknown) -> Result<()> {
        let items: Vec<ContextMenuItem> = decode_direct(env, "showContextMenu", items)?;
        let point: Point = decode_direct(env, "showContextMenu", point)?;
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::ContextMenu(items, point));
        Ok(())
    }

    /// Queues a [`DeferredAction::Toast`] -- same arguments, same decoding,
    /// as `quadraui-py`'s `show_toast`. `toast` decodes as a plain object
    /// shaped like [`Toast`].
    #[napi(js_name = "showToast")]
    pub fn show_toast(&self, env: Env, toast: Unknown, corner: Option<String>) -> Result<()> {
        let toast: Toast = decode_direct(env, "showToast", toast)?;
        let corner = corner
            .map(|c| {
                toast_corner(&c)
                    .map_err(|err| invalid_descriptor_error(format!("showToast: {err}")))
            })
            .transpose()?
            .unwrap_or_default();
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::Toast(toast, corner));
        Ok(())
    }

    /// Queues a [`DeferredAction::Notification`] -- same arguments, same
    /// decoding, as `quadraui-py`'s `send_notification`.
    #[napi(js_name = "sendNotification")]
    pub fn send_notification(
        &self,
        title: String,
        body: String,
        urgent: Option<bool>,
        silent: Option<bool>,
        tag: Option<String>,
    ) -> Result<()> {
        let mut n = Notification::new(title, body).with_silent(silent.unwrap_or(false));
        if let Some(tag) = tag {
            n = n.with_tag(tag);
        }
        n.urgent = urgent.unwrap_or(false);
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::Notification(n));
        Ok(())
    }

    /// Queues a [`DeferredAction::OpenPath`] -- same argument as
    /// `quadraui-py`'s `open_path`.
    #[napi(js_name = "openPath")]
    pub fn open_path(&self, path: String) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::OpenPath(PathBuf::from(path)));
        Ok(())
    }

    /// Queues a [`DeferredAction::RevealInFileManager`] -- same argument as
    /// `quadraui-py`'s `reveal_in_file_manager`.
    #[napi(js_name = "revealInFileManager")]
    pub fn reveal_in_file_manager(&self, path: String) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::RevealInFileManager(PathBuf::from(path)));
        Ok(())
    }

    /// Queues a [`DeferredAction::MoveToTrash`] -- same argument as
    /// `quadraui-py`'s `move_to_trash`.
    #[napi(js_name = "moveToTrash")]
    pub fn move_to_trash(&self, path: String) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::MoveToTrash(PathBuf::from(path)));
        Ok(())
    }

    /// Queues a [`DeferredAction::RestoreFromTrash`] -- same argument as
    /// `quadraui-py`'s `restore_from_trash`.
    #[napi(js_name = "restoreFromTrash")]
    pub fn restore_from_trash(&self, path: String) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::RestoreFromTrash(PathBuf::from(path)));
        Ok(())
    }
}

/// A thread-safe handle, returned by [`QuiApp::waker`], that a background
/// thread calls `post`/`wake`/`postAfter` on to deliver a value into the
/// owning [`QuiApp`]'s event stream as a `{"User": value}` event and wake
/// whichever loop currently owns it -- `step`/`run`'s drain of queued user
/// events on the next call for the headless path, or promptly (not
/// waiting for the next terminal/native event) while a live `startTui`/
/// `startNative` session is running. See this module's doc's "Live run
/// loop" section and [`QuiApp::waker`]'s own doc for why this needs to be
/// a separate object rather than a method on `QuiApp` itself -- the
/// napi-rs analog of `quadraui-py`'s own `QuiAppWaker`.
///
/// Safe to call from any thread, at any time, including before the owning
/// `QuiApp` has run a single step or after it has exited (a post after
/// exit is simply never observed -- there is no receiver left to drain
/// it). Ordinary `Send + Sync` Rust state (`SharedWaker` is
/// `Arc<Mutex<Arc<dyn Fn(UserPayload) + Send + Sync>>>`), unlike `QuiApp`
/// itself, which napi-rs pins to the thread that created it.
#[napi]
pub struct QuiAppWaker(SharedWaker);

impl QuiAppWaker {
    /// Delivers `value` (already materialized into a `Send`-safe
    /// [`JsonValue`]) to the owning `QuiApp`, wrapped in a
    /// [`UserPayload`] -- the plain-Rust core [`Self::post`]'s JS-facing
    /// decode step calls into, and this crate's own background-thread test
    /// coverage calls directly, with no live napi `Env` needed (see
    /// [`Self::post`]'s doc for why a raw JS value can't cross this
    /// boundary at all).
    fn post_value(&self, value: JsonValue) {
        let waker = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        waker(UserPayload::new(value));
    }
}

#[napi]
impl QuiAppWaker {
    /// Delivers `value` to the owning `QuiApp` as a `{"User": value}` event
    /// and wakes whichever loop currently owns it. `value` is converted
    /// through [`to_descriptor_value`] (the same JS-to-`JsonValue` walk
    /// every other decode in this crate uses) *before* this call returns,
    /// not inside whatever thread eventually delivers it -- `Unknown` is
    /// bound to the calling JS thread's `Env` and cannot itself cross a
    /// thread boundary, which is exactly why [`UserPayload`] wraps a plain
    /// [`JsonValue`] here rather than a raw JS value (see this module's
    /// doc's "Live run loop" section).
    #[napi]
    pub fn post(&self, value: Unknown) -> Result<()> {
        let value = to_descriptor_value(value)
            .map_err(|detail| invalid_descriptor_error(format!("post: {detail}")))?;
        self.post_value(value);
        Ok(())
    }

    /// Equivalent to `post(null)`: wakes the owning `QuiApp`'s loop with no
    /// payload of its own, just to make sure a pending state change (e.g.
    /// one a background thread just finished mutating through some other,
    /// already-synchronized channel) is noticed promptly rather than only
    /// on the next poll-interval timeout or unrelated event.
    #[napi]
    pub fn wake(&self) {
        self.post_value(JsonValue::Null);
    }

    /// Schedules `post(value)` to run `delayMs` milliseconds from now, on a
    /// spawned background thread that sleeps for that long and then
    /// delivers -- the JS-level equivalent of `Backend::request_frame_in`'s
    /// "the app itself knows it wants to be woken again in `delay`" timer
    /// contract, reusing this already-`Send + Sync` waker machinery rather
    /// than needing a `Backend` reference reachable from JS at all (there
    /// isn't one -- mirrors `quadraui-py`'s own `QuiAppWaker::post_after`
    /// doc for the same "no reachable handle" story). Returns immediately;
    /// does not block the calling thread.
    #[napi(js_name = "postAfter")]
    pub fn post_after(&self, delay_ms: u32, value: Unknown) -> Result<()> {
        let value = to_descriptor_value(value)
            .map_err(|detail| invalid_descriptor_error(format!("postAfter: {detail}")))?;
        let shared = Arc::clone(&self.0);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(u64::from(delay_ms)));
            let waker = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            waker(UserPayload::new(value));
        });
        Ok(())
    }

    /// Equivalent to `postAfter(delayMs, null)` -- see [`Self::wake`]'s doc
    /// for why a payload-free wake is useful on its own.
    #[napi(js_name = "wakeAfter")]
    pub fn wake_after(&self, delay_ms: u32) {
        let shared = Arc::clone(&self.0);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(u64::from(delay_ms)));
            let waker = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            waker(UserPayload::new(JsonValue::Null));
        });
    }
}

/// A headless quadraui app, driven entirely from JavaScript/TypeScript.
///
/// Construct it with a `width`/`height` cell grid, insert descriptors with
/// `insertButton`/`insertTextDisplay`/`insertList`/`insertTree`/
/// `insertDataTable`/`insertTabBar`/`insertExtra` (and its typed
/// convenience wrappers `insertProgress`/`insertSpinner`/`insertImage`),
/// register `setHandler`/`setTickHandler`/`setRender` callbacks, and
/// inspect the rendered frame with `screen()`/`screenContains()`.
///
/// A registered handler runs while `step`/`run` holds this app's one `&mut
/// self` for the whole step. napi-rs's own per-instance native-borrow
/// tracking (`napi::bindgen_runtime`'s internal `NativeBorrowState`) already
/// rejects a handler calling straight back into, say, `insertButton` on the
/// same instance mid-dispatch, at the FFI boundary, before any of this
/// crate's own code runs -- but that rejection carries a generic napi-rs
/// message ("the same native value cannot be borrowed mutably while another
/// borrow is active"), not one of this crate's own tagged error kinds, and
/// is an implementation detail of the installed napi-rs version rather than
/// a contract this crate's own tests can assert against directly.
/// [`QuiApp::check_not_dispatching`] adds this crate's own explicit,
/// documented, `ReentrantCallError`-tagged guard for the same scenario --
/// defense in depth alongside napi-rs's own, not a replacement for it.
/// `actions()` is the sanctioned way for a handler to request an overlay/OS
/// action instead -- see [`QuiAppActions`]'s own doc.
#[napi]
pub struct QuiApp {
    driver: TuiDriver<NodeAppLogic>,
    /// Events queued by `pushKeyEvent`, drained one per `step`/`run` call
    /// (oldest first); a step ticks the app instead when this is empty.
    pending_events: VecDeque<UiEvent>,
    /// Set once a call into `driver` has panicked and been caught by
    /// [`QuiApp::guarded`]. Every method on a poisoned app raises instead of
    /// touching the driver again.
    poisoned: bool,
    /// Set for the duration of every [`QuiApp::guarded`] call (i.e. while a
    /// step/render triggered by this app's own `#[napi]` surface is live) --
    /// see [`QuiApp::check_not_dispatching`] and this struct's own doc for
    /// what this guards against.
    in_dispatch: Cell<bool>,
    /// The live terminal/window takeover `startTui`/a future `startNative`
    /// moved this app's whole [`NodeAppLogic`] into, if any -- see this
    /// module's doc's "Live run loop" section. `None` until `startTui` is
    /// called, and again once that runner's own `step` observes
    /// [`StepOutcome::Exited`] (or `stopLive` is called explicitly).
    live: Option<LiveRunner>,
    /// Set the instant [`Self::live`] goes from `Some` back to `None` --
    /// whether because that runner's own `step` observed
    /// [`StepOutcome::Exited`], it failed outright, or [`QuiApp::stop_live`]
    /// tore it down early -- and never cleared again. Once set,
    /// [`QuiApp::step_once`] raises `LiveRunnerExitedError` instead of
    /// falling back to ticking [`Self::driver`]'s own [`NodeAppLogic`]:
    /// `startTui` already moved that logic's handlers/descriptors into the
    /// live session and never moves them back (see `startTui`'s own doc),
    /// so a `step()` call after the session that owned them has ended --
    /// whether a stray extra call or a JS pump loop mistakenly restarted
    /// after seeing `step()` return `false` -- would otherwise silently
    /// spin on that now-empty orphan instead of raising.
    live_exited: Cell<bool>,
    /// The [`Backend::waker`] closure currently reachable through
    /// [`QuiApp::waker`]'s returned handle -- [`Self::driver`]'s own
    /// headless backend's waker until [`Self::live`] is `Some`, swapped to
    /// that live runner's own backend's waker for the duration of its
    /// session. See [`SharedWaker`]'s own doc.
    current_waker: SharedWaker,
}

#[napi]
impl QuiApp {
    /// Creates a new headless app on a `width`x`height` cell grid. Raises if
    /// either is zero.
    #[napi(constructor)]
    pub fn new(width: u16, height: u16) -> Result<Self> {
        let (width, height) = checked_dimensions(width, height)?;
        let driver = TuiDriver::new(NodeAppLogic::default(), width, height);
        let current_waker = Arc::new(Mutex::new(driver.backend().waker()));
        Ok(QuiApp {
            driver,
            pending_events: VecDeque::new(),
            poisoned: false,
            in_dispatch: Cell::new(false),
            live: None,
            live_exited: Cell::new(false),
            current_waker,
        })
    }

    /// Resizes the underlying cell grid to `width`x`height`. Raises if
    /// either is zero.
    #[napi]
    pub fn resize(&mut self, env: Env, width: u16, height: u16) -> Result<()> {
        let (width, height) = checked_dimensions(width, height)?;
        self.check_not_live()?;
        self.guarded(env, |app| {
            app.driver.resize(width, height);
            Ok(())
        })
    }

    /// Queues a synthetic `KeyPressed` event for the character with Unicode
    /// code point `ch`, no modifiers held, not a repeat. Raises if `ch`
    /// isn't a valid Unicode scalar value. Raises `LiveRunnerActiveError`
    /// while `startTui` is active -- a live [`TuiRunner`] reads real
    /// terminal input directly, never this app's own [`Self::pending_events`]
    /// queue, so a synthetic key queued here would simply never be drained.
    #[napi(js_name = "pushKeyEvent")]
    pub fn push_key_event(&mut self, ch: u32) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        let ch = char::from_u32(ch)
            .ok_or_else(|| Error::from_reason(format!("invalid Unicode code point: {ch}")))?;
        self.pending_events.push_back(key_pressed_event(ch));
        Ok(())
    }

    /// Advances the runner by one step: dispatches the oldest event queued
    /// by `pushKeyEvent`, or ticks (invoking a registered tick handler) if
    /// none is queued. Returns `true` if the app is still running
    /// afterwards, `false` once it has exited.
    #[napi]
    pub fn step(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| app.step_once())
    }

    /// Runs up to `max_steps` steps (default 1). Stops early once the app
    /// exits. Returns `true` if still running afterwards, `false` once
    /// exited.
    #[napi]
    pub fn run(&mut self, env: Env, max_steps: Option<u32>) -> Result<bool> {
        let steps = max_steps.unwrap_or(1).max(1);
        let mut running = true;
        for _ in 0..steps {
            running = self.guarded(env, |app| app.step_once())?;
            if !running {
                break;
            }
        }
        Ok(running)
    }

    /// Renders the current frame and returns it as a plain-text screen
    /// dump, one row per line. Raises `LiveRunnerActiveError` while
    /// `startTui` is active -- there is no headless frame to dump once this
    /// app's own `NodeAppLogic` has moved into a live [`TuiRunner`]; look
    /// at the real terminal instead.
    #[napi]
    pub fn screen(&mut self, env: Env) -> Result<String> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(app.driver.screen())
        })
    }

    /// Whether `needle` appears anywhere in the current rendered frame.
    /// Raises `LiveRunnerActiveError` while `startTui` is active -- see
    /// [`Self::screen`]'s doc.
    #[napi(js_name = "screenContains")]
    pub fn screen_contains(&mut self, env: Env, needle: String) -> Result<bool> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(app.driver.screen_contains(&needle))
        })
    }

    /// Empties a slot (one of the six fixed primitive slot ids
    /// `quadraui_binding_core` defines -- `SLOT_BUTTON` etc.), re-rendering
    /// so `screen()` no longer shows it. Returns whether the slot held
    /// anything. Raises if `slot` doesn't name one of the six, or
    /// `LiveRunnerActiveError` while `startTui` is active.
    #[napi(js_name = "slotClear")]
    pub fn slot_clear(&mut self, env: Env, slot: i64) -> Result<bool> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        if !NodeAppLogic::is_known_slot(slot) {
            return Err(Error::from_reason(format!("unknown slot id: {slot}")));
        }
        self.guarded(env, |app| {
            let cleared = app.driver.app_mut().clear_slot(slot);
            if cleared {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(cleared)
        })
    }

    /// Registers `handler` as this app's event handler, called with the
    /// JS-converted payload for every dispatched `UiEvent` -- see
    /// [`NodeEventSink`]'s doc for the payload shape and return-value
    /// contract (only a literal `true`/`false` return has any effect; a
    /// `WindowClose` event's own default flips the other way, per
    /// `window_close_reaction`'s doc). Raises if called re-entrantly from
    /// inside an already-live dispatch, or `LiveRunnerActiveError` once
    /// `startTui` is active -- call this before `startTui`, same as
    /// `quadraui-py`'s own discipline for `run_tui`/`run_native`; see this
    /// module's doc's "Live run loop" section.
    #[napi(js_name = "setHandler")]
    pub fn set_handler(
        &mut self,
        handler: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        self.driver.app_mut().handler = Some(handler.create_ref()?);
        Ok(())
    }

    /// Registers `handler` as this app's tick handler, called whenever the
    /// runner advances with no event queued. Raises if called re-entrantly,
    /// or `LiveRunnerActiveError` once `startTui` is active -- see
    /// [`Self::set_handler`]'s doc.
    #[napi(js_name = "setTickHandler")]
    pub fn set_tick_handler(
        &mut self,
        handler: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        self.driver.app_mut().on_tick = Some(handler.create_ref()?);
        Ok(())
    }

    /// Registers `callback` as this app's render callback, called once per
    /// frame and expected to return a plain object decodable the same way a
    /// `pushEvent` descriptor is -- a flat map of slot names to descriptors,
    /// or a layout tree (see `quadraui_binding_core::decode_render_output`'s
    /// doc for the flat-vs-tree sniff). Overrides the six fixed `insert*`
    /// slots for every subsequent render. Raises if called re-entrantly, or
    /// `LiveRunnerActiveError` once `startTui` is active -- see
    /// [`Self::set_handler`]'s doc.
    #[napi(js_name = "setRender")]
    pub fn set_render(
        &mut self,
        callback: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        self.driver.app_mut().render_callback = Some(callback.create_ref()?);
        Ok(())
    }

    /// Returns a [`QuiAppActions`] handle sharing this app's deferred-action
    /// queue -- the sanctioned way for a registered handler/tick callback to
    /// request a dialog/context-menu/toast/notification/file action without
    /// re-entering this `QuiApp`'s own `&mut self` mid-dispatch (which
    /// [`Self::check_not_dispatching`] rejects). Call this once, from
    /// top-level script code, before registering a handler that needs it --
    /// see [`QuiAppActions`]'s own doc. Raises if called re-entrantly (not
    /// because sharing the queue is unsafe, but for the same "obtain once,
    /// up front" discipline every other `QuiApp` method enforces), or
    /// `LiveRunnerActiveError` once `startTui` is active -- call this
    /// before `startTui`; the returned handle keeps working correctly
    /// through the live session regardless (see this module's doc's "Live
    /// run loop" section).
    #[napi]
    pub fn actions(&self) -> Result<QuiAppActions> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        Ok(QuiAppActions {
            queue: Rc::clone(&self.driver.app().deferred_actions),
        })
    }

    /// Inserts a `Button` descriptor from a JS object, returning its slot
    /// handle. Raises `SlotOccupiedError` if the button slot is already
    /// occupied -- call `slotClear` first.
    #[napi(js_name = "insertButton")]
    pub fn insert_button(&mut self, env: Env, descriptor: Unknown) -> Result<i64> {
        self.insert_fixed(
            env,
            SLOT_BUTTON,
            "insertButton",
            |app, value: Button| app.host.button = Some(value),
            descriptor,
        )
    }

    /// Inserts a `TextDisplay` descriptor from a JS object, returning its
    /// slot handle. Raises `SlotOccupiedError` if the text-display slot is
    /// already occupied -- call `slotClear` first.
    #[napi(js_name = "insertTextDisplay")]
    pub fn insert_text_display(&mut self, env: Env, descriptor: Unknown) -> Result<i64> {
        self.insert_fixed(
            env,
            SLOT_TEXT_DISPLAY,
            "insertTextDisplay",
            |app, value: TextDisplay| app.host.text_display = Some(value),
            descriptor,
        )
    }

    /// Inserts a `ListView` descriptor from a JS object, returning its slot
    /// handle. Raises `SlotOccupiedError` if the list slot is already
    /// occupied -- call `slotClear` first.
    #[napi(js_name = "insertList")]
    pub fn insert_list(&mut self, env: Env, descriptor: Unknown) -> Result<i64> {
        self.insert_fixed(
            env,
            SLOT_LIST,
            "insertList",
            |app, value: ListView| app.host.list = Some(value),
            descriptor,
        )
    }

    /// Inserts a `TreeView` descriptor from a JS object, returning its slot
    /// handle. Raises `SlotOccupiedError` if the tree slot is already
    /// occupied -- call `slotClear` first.
    #[napi(js_name = "insertTree")]
    pub fn insert_tree(&mut self, env: Env, descriptor: Unknown) -> Result<i64> {
        self.insert_fixed(
            env,
            SLOT_TREE,
            "insertTree",
            |app, value: TreeView| app.host.tree = Some(value),
            descriptor,
        )
    }

    /// Inserts a `DataTable` descriptor from a JS object, returning its
    /// slot handle. Raises `SlotOccupiedError` if the data-table slot is
    /// already occupied -- call `slotClear` first.
    #[napi(js_name = "insertDataTable")]
    pub fn insert_data_table(&mut self, env: Env, descriptor: Unknown) -> Result<i64> {
        self.insert_fixed(
            env,
            SLOT_DATA_TABLE,
            "insertDataTable",
            |app, value: DataTable| app.host.data_table = Some(value),
            descriptor,
        )
    }

    /// Inserts a `TabBar` descriptor from a JS object, returning its slot
    /// handle. Raises `SlotOccupiedError` if the tab-bar slot is already
    /// occupied -- call `slotClear` first.
    #[napi(js_name = "insertTabBar")]
    pub fn insert_tab_bar(&mut self, env: Env, descriptor: Unknown) -> Result<i64> {
        self.insert_fixed(
            env,
            SLOT_TAB_BAR,
            "insertTabBar",
            |app, value: TabBar| app.host.tab_bar = Some(value),
            descriptor,
        )
    }

    /// Inserts a `ProgressBar` descriptor from a JS object, returning
    /// `"progress"` back. Raises `SlotOccupiedError` if `"progress"` is
    /// already occupied -- call `clearExtra("progress")` first. A thin,
    /// typed-descriptor convenience over [`Self::insert_extra`].
    #[napi(js_name = "insertProgress")]
    pub fn insert_progress(&mut self, env: Env, descriptor: Unknown) -> Result<String> {
        self.store_extra(env, "progress", "insertProgress", descriptor)
    }

    /// Inserts a `Spinner` descriptor from a JS object, returning
    /// `"spinner"` back. Raises `SlotOccupiedError` if `"spinner"` is
    /// already occupied -- call `clearExtra("spinner")` first. A thin,
    /// typed-descriptor convenience over [`Self::insert_extra`].
    #[napi(js_name = "insertSpinner")]
    pub fn insert_spinner(&mut self, env: Env, descriptor: Unknown) -> Result<String> {
        self.store_extra(env, "spinner", "insertSpinner", descriptor)
    }

    /// Inserts an `Image` descriptor from a JS object, returning `"image"`
    /// back. Raises `SlotOccupiedError` if `"image"` is already occupied --
    /// call `clearExtra("image")` first. A thin, typed-descriptor
    /// convenience over [`Self::insert_extra`].
    #[napi(js_name = "insertImage")]
    pub fn insert_image(&mut self, env: Env, descriptor: Unknown) -> Result<String> {
        self.store_extra(env, "image", "insertImage", descriptor)
    }

    /// Inserts a descriptor for any additive primitive `kind` registered in
    /// `quadraui_binding_core::extra_paint`'s `extra_primitives!` table (an
    /// `InvalidDescriptorError` raised for an unknown `kind` names every
    /// registered one), returning `kind` back. Raises `SlotOccupiedError`
    /// if `kind` is already occupied -- call `clearExtra(kind)` first. Lets
    /// a consumer insert a future additive primitive by name alone, with no
    /// new napi method needed on this class.
    #[napi(js_name = "insertExtra")]
    pub fn insert_extra(&mut self, env: Env, kind: String, descriptor: Unknown) -> Result<String> {
        self.store_extra(env, &kind, "insertExtra", descriptor)
    }

    /// Empties an additive primitive slot (one registered in
    /// `quadraui_binding_core::extra_paint`'s `extra_primitives!` table),
    /// re-rendering so `screen()` no longer shows it. Returns whether the
    /// slot held anything; an unrecognized `kind` simply has nothing to
    /// clear (unlike `slotClear`, this never raises for an unknown key).
    /// Raises `LiveRunnerActiveError` once `startTui` is active.
    #[napi(js_name = "clearExtra")]
    pub fn clear_extra(&mut self, env: Env, kind: String) -> Result<bool> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let cleared = app.driver.app_mut().clear_extra_slot(&kind);
            if cleared {
                app.driver.render();
                take_pending_error(app.driver.app_mut())?;
            }
            Ok(cleared)
        })
    }

    /// Queues a [`UiEvent`] decoded from a JS object, drained one per
    /// `step`/`run` call the same way a `pushKeyEvent`-queued event is.
    /// Raises `InvalidDescriptorError` on a shape mismatch. Goes through
    /// [`decode_via_json`] rather than [`decode_direct`] because `UiEvent`
    /// carries two variants ([`UiEvent::Accelerator`],
    /// [`UiEvent::GlobalShortcut`]) whose payload is a newtype-struct
    /// identifier ([`quadraui::AcceleratorId`], [`quadraui::GlobalShortcutId`]).
    /// Raises `LiveRunnerActiveError` once `startTui` is active -- a live
    /// [`TuiRunner`] drains real terminal input, never this app's own
    /// [`Self::pending_events`] queue, so a synthetic event queued here
    /// would simply never be drained; see [`Self::push_key_event`]'s doc.
    #[napi(js_name = "pushEvent")]
    pub fn push_event(&mut self, descriptor: Unknown) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        let event: UiEvent = decode_via_json("pushEvent", descriptor)?;
        self.pending_events.push_back(event);
        Ok(())
    }

    /// Moves this app's whole [`NodeAppLogic`] into a real
    /// `quadraui::tui::TuiRunner` -- a live terminal takeover, the napi-rs
    /// analog of `quadraui-py`'s blocking `run_tui`, but non-blocking: this
    /// call only negotiates the terminal and swaps [`Self::current_waker`]
    /// to the new live backend's own waker, then returns immediately.
    /// Drive it forward by calling `step()`/`run()` repeatedly from JS,
    /// e.g. from a `setInterval`/`setImmediate` loop (see `js/index.ts`'s
    /// `pump` helper) -- see this module's doc's "Live run loop" section
    /// for why this must never be a blocking pump the way `quadraui-py`'s
    /// `run_tui` is.
    ///
    /// `mouse` mirrors `quadraui-py`'s own `run_tui(mouse=...)`: `true`
    /// (the default) captures the mouse for this app's own hit testing;
    /// pass `false` to leave the terminal's native text selection usable
    /// instead.
    ///
    /// Raises `LiveRunnerActiveError` if a live session is already
    /// running. Raises `InternalError` if the terminal takeover itself
    /// fails (no real tty -- see `quadraui-py/tests/test_run_tui.py`'s own
    /// doc for exactly when that happens), in which case this app's prior
    /// configuration (handlers, inserted descriptors) is lost the same way
    /// `quadraui-py`'s `run_tui` loses it on the same failure
    /// (`std::mem::take` already moved it out before the takeover
    /// attempt), leaving [`Self::driver`] usable but empty. The same data
    /// loss applies once a *successful* session ends too, whether its own
    /// `step` observes [`StepOutcome::Exited`] or [`Self::stop_live`] tears
    /// it down early: neither moves the handlers/descriptors back onto
    /// [`Self::driver`], and [`Self::live_exited`]'s sticky flag then
    /// raises `LiveRunnerExitedError` on any further `step()`/`run()`
    /// rather than silently ticking that now-empty orphan.
    #[napi(js_name = "startTui")]
    pub fn start_tui(&mut self, env: Env, mouse: Option<bool>) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let app_logic = std::mem::take(app.driver.app_mut());
            let config = if mouse.unwrap_or(true) {
                RunConfig::default()
            } else {
                RunConfig::no_mouse()
            };
            let runner = TuiRunner::new_with(app_logic, config)
                .map_err(|err| internal_error(format!("startTui: {err}")))?;
            let live = LiveRunner::Tui(runner);
            *app.current_waker.lock().unwrap_or_else(|e| e.into_inner()) = live.waker();
            app.live = Some(live);
            Ok(())
        })
    }

    /// Tears down a live `startTui`/`startNative` session early, without
    /// waiting for its own `step` to observe [`StepOutcome::Exited`] on its
    /// own (e.g. the terminal's native close/interrupt) -- the `LiveRunner`
    /// variant's own `Drop` impl (`TuiRunner`'s restores raw mode/the
    /// alternate screen) runs as this app's [`Self::live`] is replaced with
    /// `None`. Returns whether a live session was actually running (`false`
    /// is not an error -- calling this with no live session active is a
    /// harmless no-op, matching `slotClear`'s own "nothing to do" shape
    /// rather than `insertButton`'s "state already there" one). This app's
    /// prior configuration (handlers/descriptors `startTui` moved out) does
    /// not come back -- see [`Self::live_exited`]'s own doc, which this
    /// also sets, so a later `step()`/`run()` raises `LiveRunnerExitedError`
    /// rather than silently ticking the now-empty orphan.
    #[napi(js_name = "stopLive")]
    pub fn stop_live(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| {
            let was_live = app.live.take().is_some();
            if was_live {
                app.live_exited.set(true);
                app.restore_headless_waker();
            }
            Ok(was_live)
        })
    }

    /// Returns a [`QuiAppWaker`] handle: a thread-safe, `Send + Sync`
    /// object a background thread can call `post`/`wake`/`postAfter` on to
    /// deliver a value back into this app's event stream as a
    /// `{"User": value}` event, waking whichever loop currently owns it --
    /// see this module's doc's "Live run loop" section and
    /// [`QuiAppWaker`]'s own doc.
    ///
    /// Raises if this app is poisoned (an earlier caught panic -- see
    /// [`Self::guarded`]) rather than handing out a handle anyway: every
    /// other `QuiApp` method routes through `guarded`, which already
    /// refuses a poisoned app, and this is the one direct `&self` read of
    /// [`Self::current_waker`] that would otherwise skip that check.
    /// Unlike most other configuration methods, this does **not** raise
    /// while a live session is running -- [`Self::current_waker`] always
    /// points at whichever backend (headless or live) is actually current,
    /// so a handle obtained during a live session still posts into it
    /// correctly.
    #[napi]
    pub fn waker(&self) -> Result<QuiAppWaker> {
        if self.poisoned {
            return Err(Error::from_reason(
                "this QuiApp is poisoned by an earlier caught panic and can no longer be used",
            ));
        }
        Ok(QuiAppWaker(Arc::clone(&self.current_waker)))
    }
}

impl QuiApp {
    /// Shared body of every fixed-slot `insert*` method: reject an
    /// occupied slot, decode `descriptor` into `T` via [`decode_direct`],
    /// store it with `store`, then re-render -- the napi-rs analog of
    /// `quadraui-py`'s own `insert` helper. Raises `LiveRunnerActiveError`
    /// once `startTui` is active -- see this module's doc's "Live run
    /// loop" section.
    fn insert_fixed<T: DeserializeOwned>(
        &mut self,
        env: Env,
        slot: i64,
        what: &str,
        store: impl FnOnce(&mut NodeAppLogic, T),
        descriptor: Unknown,
    ) -> Result<i64> {
        self.check_not_live()?;
        if self.driver.app().slot_occupied(slot) {
            return Err(slot_occupied_error(format!(
                "{what}: slot already occupied; call slotClear({slot}) first"
            )));
        }
        let value: T = decode_direct(env, what, descriptor)?;
        self.guarded(env, |app| {
            store(app.driver.app_mut(), value);
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(slot)
        })
    }

    /// Shared body of every additive-primitive `insert*` method (including
    /// the public `insertExtra` method above): reject an already-occupied
    /// `kind`, decode the JS object through [`decode_extra_primitive`] via
    /// [`decode_via_core`], store it, then re-render, returning `kind` back
    /// for symmetry with the fixed `insert*` methods' slot-handle return
    /// value -- the napi-rs analog of `quadraui-py`'s own `store_extra`
    /// helper. Raises `LiveRunnerActiveError` once `startTui` is active --
    /// see [`Self::insert_fixed`]'s doc.
    fn store_extra(
        &mut self,
        env: Env,
        kind: &str,
        what: &str,
        descriptor: Unknown,
    ) -> Result<String> {
        self.check_not_live()?;
        if self.driver.app().extra_occupied(kind) {
            return Err(slot_occupied_error(format!(
                "{what}: {kind:?} already occupied; call clearExtra({kind:?}) first"
            )));
        }
        let value = decode_via_core(what, descriptor, |value| {
            decode_extra_primitive(what, kind, value)
        })?;
        self.guarded(env, |app| {
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
}

impl QuiApp {
    /// Raises `ReentrantCallError` if called while a step/render triggered
    /// by this app's own `#[napi]` surface is still live (see
    /// [`Self::in_dispatch`] and this struct's own doc) -- the first thing
    /// every method on this class does, whether or not it goes on to call
    /// [`Self::guarded`] itself.
    fn check_not_dispatching(&self) -> Result<()> {
        if self.in_dispatch.get() {
            return Err(reentrant_call_error(
                "cannot call a QuiApp method from inside this same app's own registered \
                 handler/tick/render callback -- use actions() to request an overlay/OS \
                 action instead",
            ));
        }
        Ok(())
    }

    /// Raises `LiveRunnerActiveError` if `startTui` (or a future
    /// `startNative`) is currently running on this app -- see this
    /// module's doc's "Live run loop" section. Every method on `QuiApp`
    /// (in this module and in the sibling `overlays`/`accelerators`/
    /// `theme`/`metrics`/`platform` modules) that reaches into
    /// [`Self::driver`] directly -- its headless backend or the
    /// [`NodeAppLogic`] `startTui` leaves behind -- calls this first: once
    /// `startTui` has moved this app's own `NodeAppLogic` into
    /// [`Self::live`], mutating or reading [`Self::driver`] again would
    /// silently touch the emptied, orphaned logic and the dormant backend
    /// left behind instead of the live session actually running, rather
    /// than raising a clear error. `step`/`run` (which drive
    /// [`Self::live`] itself) and `waker` (which only ever reads an
    /// already-shared `Arc` that keeps working correctly regardless of
    /// where the `NodeAppLogic` it was obtained from now lives) are the
    /// only exceptions and do not call this. `clipboard.rs`'s methods are
    /// a deliberate, documented exception too: they resolve against
    /// `self.driver.backend()`'s own clipboard regardless of a live
    /// session, by design -- see that module's own doc.
    fn check_not_live(&self) -> Result<()> {
        if self.live.is_some() {
            return Err(live_runner_active_error(
                "cannot call this method while startTui/startNative is active -- configure \
                 handlers and descriptors, and obtain actions()/waker() handles, before \
                 calling startTui/startNative",
            ));
        }
        Ok(())
    }

    /// Mutable access to whichever [`NodeAppLogic`] is currently driving
    /// this app -- [`Self::live`]'s own, if a live session is running,
    /// otherwise [`Self::driver`]'s headless one. [`Self::guarded`] stashes
    /// its live `Env` on whichever this returns, so a registered handler/
    /// tick/render callback reached through either [`Self::step_once`] or
    /// a direct [`Self::driver`] call (gated by [`Self::check_not_live`])
    /// has one to call back into JS with -- see [`NodeAppLogic::env`]'s
    /// doc.
    fn active_app_mut(&mut self) -> &mut NodeAppLogic {
        match self.live.as_mut() {
            Some(live) => live.app_mut(),
            None => self.driver.app_mut(),
        }
    }

    /// Points [`Self::current_waker`] back at [`Self::driver`]'s own
    /// headless backend's waker -- called once a live session's own `step`
    /// observes [`StepOutcome::Exited`] (or [`Self::stop_live`] tears one
    /// down early), undoing [`Self::start_tui`]'s swap to that session's
    /// own backend's waker instead.
    fn restore_headless_waker(&self) {
        *self.current_waker.lock().unwrap_or_else(|e| e.into_inner()) =
            self.driver.backend().waker();
    }

    /// Runs `body` on `self`, catching a Rust panic instead of letting it
    /// propagate as napi-rs's own generic panic-to-`Error` conversion (which
    /// stops the process aborting but does not poison this handle). On
    /// success returns `body`'s result unchanged; on a panic, poisons this
    /// app and raises an `Error` carrying [`panic_message`]'s text. If this
    /// app is already poisoned (an earlier call panicked) or already
    /// dispatching (see [`Self::check_not_dispatching`]), raises without
    /// running `body` at all -- mirrors `quadraui-py`'s `QuiApp::guarded`,
    /// plus the reentrancy guard PyO3's own runtime borrow check makes
    /// unnecessary there. `env` is stashed on [`Self::active_app_mut`]
    /// before `body` runs, so a registered handler/tick/render callback
    /// `body` reaches (via `step_once`/`driver.render()`) has a live `Env`
    /// to call back into JS with -- see [`NodeAppLogic::env`]'s doc.
    fn guarded<T>(&mut self, env: Env, body: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.check_not_dispatching()?;
        if self.poisoned {
            return Err(Error::from_reason(
                "this QuiApp is poisoned by an earlier caught panic and can no longer be used",
            ));
        }
        self.active_app_mut().env.set(Some(env));
        self.in_dispatch.set(true);
        let result = panic::catch_unwind(AssertUnwindSafe(|| body(self)));
        self.in_dispatch.set(false);
        match result {
            Ok(result) => result,
            Err(payload) => {
                self.poisoned = true;
                Err(Error::from_reason(panic_message(&*payload)))
            }
        }
    }

    /// Dispatches the oldest queued event, or ticks if none is queued, then
    /// surfaces any pending error a callback recorded while handling it.
    /// Returns whether the app is still running. With no live session
    /// ([`Self::live`] is `None`) and none having ever ended
    /// ([`Self::live_exited`] is unset), drains [`Self::driver`]'s queue/
    /// backend via [`step_driver`], exactly as before `startTui` existed.
    /// Raises `LiveRunnerExitedError` instead if a live session once ran
    /// here and has since ended -- see [`Self::live_exited`]'s own doc.
    /// With a live session currently running, instead takes one
    /// non-blocking [`LiveRunner::step`] pass -- never a blocking `pump`,
    /// since that would freeze the one libuv reactor this whole process
    /// shares (see this module's doc's "Live run loop" section) -- and,
    /// once that pass reports [`StepOutcome::Exited`] (or itself fails),
    /// drops [`Self::live`], sets [`Self::live_exited`], and restores
    /// [`Self::current_waker`] to the headless path via
    /// [`Self::restore_headless_waker`].
    fn step_once(&mut self) -> Result<bool> {
        let Some(live) = self.live.as_mut() else {
            if self.live_exited.get() {
                return Err(live_runner_exited_error(
                    "the live startTui/startNative session this app once held has already \
                     ended -- construct a new QuiApp to run another one",
                ));
            }
            let running = step_driver(&mut self.driver, &mut self.pending_events);
            take_pending_error(self.driver.app_mut())?;
            return Ok(running);
        };
        let step_result = live.step();
        // Take any callback-raised error before possibly dropping `live`
        // below -- once dropped, there is no more `NodeAppLogic` left to
        // read it back off of.
        let pending = take_pending_error(live.app_mut());
        let outcome = match step_result {
            Ok(outcome) => outcome,
            Err(err) => {
                self.live = None;
                self.live_exited.set(true);
                self.restore_headless_waker();
                pending?;
                return Err(internal_error(format!("step: {err}")));
            }
        };
        pending?;
        // `StepOutcome` is `#[non_exhaustive]` (today: `Continue`/`Exited`)
        // -- an equality check against the one variant that needs
        // teardown, mirroring `quadraui-py`'s own `run_tui`/`run_native`
        // pump loops, stays correct against a future third variant without
        // this crate needing a wildcard arm that silently swallows it.
        if outcome == StepOutcome::Exited {
            self.live = None;
            self.live_exited.set(true);
            self.restore_headless_waker();
            Ok(false)
        } else {
            Ok(true)
        }
    }
}

// ─── ShellApp / `QuiApp.runShell` ───────────────────────────────────────────
//
// See the module doc's "ShellApp / `QuiApp.runShell`" section for the
// overall shape; the items below are its pieces.

/// `WidgetId` string `quadraui::compose::bottom_panel::BottomPanelController::build_tab_bar`
/// gives the `TabBar` it paints -- fixed, not per-config, so
/// [`QuiShellApp::bottom_panel_tab_center`] can name it directly. Mirrors
/// `quadraui-py`'s own `BOTTOM_PANEL_TABS_WIDGET_ID`.
const BOTTOM_PANEL_TABS_WIDGET_ID: &str = "bottom-panel:tabs";

/// Registered JS callbacks, the live `Env` for whichever [`QuiShellApp`]
/// method call is currently driving it, and the last callback-raised error
/// for one [`QuiShellApp`] session. Shared (via `Rc<RefCell<_>>`) between
/// the [`QuiShellApp`] a caller holds and the [`NodeShellApp`] moved into
/// its driver -- the napi-rs analog of `quadraui-py`'s own `ShellState`;
/// see this module's "ShellApp / `QuiApp.runShell`" doc for why a side
/// channel is needed at all.
#[derive(Default)]
struct ShellState {
    /// Invoked with the JS-converted event payload for every event
    /// [`NodeShellApp::handle`] receives -- i.e. every event the shell
    /// chrome itself didn't consume. `None` leaves such events unhandled,
    /// same as a plain `QuiApp` with no `setHandler` call.
    handler: Option<NodeCallback>,
    /// Invoked with no meaningful argument whenever [`NodeShellApp::tick`]
    /// fires with no event queued, mirroring [`QuiApp::setTickHandler`].
    on_tick: Option<NodeCallback>,
    /// Invoked with no meaningful argument once per frame; its return
    /// value, if a JS array of strings, becomes the lines of a single
    /// `TextDisplay` painted into `AppShellLayout::main_content_bounds`.
    /// `None` paints nothing into the main content area -- the shell
    /// chrome still renders either way.
    render_content: Option<NodeCallback>,
    /// Invoked with one JS-converted `AppShellEvent` payload (see
    /// [`app_shell_event_to_json`]) whenever [`ShellApp::on_shell_event_ctx`]
    /// fires -- a panel switch, the sidebar being hidden/resized, the
    /// bottom panel being resized/hidden, or a bottom-item (no-panel)
    /// activity-bar click.
    on_shell_event: Option<NodeCallback>,
    /// Invoked with one JS-converted `BottomPanelEvent` payload (see
    /// [`bottom_panel_event_to_json`]) whenever [`ShellApp::on_bottom_panel_event`]
    /// fires. Only ever fires when this app's `config` included a
    /// `"bottom_panel"` entry.
    on_bottom_panel_event: Option<NodeCallback>,
    /// The live `Env` for whichever [`QuiShellApp`] method call is
    /// currently driving this app, set by [`QuiShellApp::guarded`] right
    /// before it steps into the driver -- the `ShellState` counterpart of
    /// [`NodeAppLogic::env`], needed here because [`ErasedShellDriver`]
    /// has no `app_mut()` seam a shell driver could stash it on directly.
    env: Option<Env>,
    /// Set by any of the callbacks above when the JS callable they invoke
    /// raises, or when this binding's own JS-value conversion fails.
    /// [`QuiShellApp::step_once`] takes this and re-raises it out of
    /// `step`/`run`, mirroring [`NodeAppLogic::pending_error`]; see
    /// [`ShellState::record_error`] for the first-error-wins policy every
    /// write goes through.
    pending_error: Option<Error>,
}

impl ShellState {
    /// Records `err` as this session's pending error, but only if no
    /// earlier error already claimed the slot -- mirrors the free
    /// [`record_error`] function's "first error wins" contract, as a
    /// method here since `ShellState` holds its `pending_error` as a plain
    /// field rather than its own `Rc<RefCell<_>>`.
    fn record_error(&mut self, err: Error) {
        if self.pending_error.is_none() {
            self.pending_error = Some(err);
        }
    }
}

/// Converts one [`AppShellEvent`] into the externally-tagged JSON shape
/// [`encode_via_json`] would give a `#[derive(Serialize)]` enum, e.g.
/// `{"PanelChanged": {"panel_id": "panel:explorer"}}` or
/// `{"SidebarHidden": null}` -- hand-built since `AppShellEvent` has no
/// such derive upstream, mirrors `quadraui-py`'s own `app_shell_event_to_py`.
fn app_shell_event_to_json(event: &AppShellEvent) -> JsonValue {
    match event {
        AppShellEvent::PanelChanged { panel_id } => {
            serde_json::json!({"PanelChanged": {"panel_id": panel_id.as_str()}})
        }
        AppShellEvent::SidebarHidden => serde_json::json!({"SidebarHidden": null}),
        AppShellEvent::SidebarResized { new_width } => {
            serde_json::json!({"SidebarResized": {"new_width": new_width}})
        }
        AppShellEvent::BottomPanelResized { new_height } => {
            serde_json::json!({"BottomPanelResized": {"new_height": new_height}})
        }
        AppShellEvent::BottomPanelHidden => serde_json::json!({"BottomPanelHidden": null}),
        AppShellEvent::BottomItemClicked { id } => {
            serde_json::json!({"BottomItemClicked": {"id": id.as_str()}})
        }
        AppShellEvent::Consumed => serde_json::json!({"Consumed": null}),
        AppShellEvent::Ignored => serde_json::json!({"Ignored": null}),
    }
}

/// Converts one [`BottomPanelEvent`] into the externally-tagged JSON shape,
/// e.g. `{"TabActivated": "bp:terminal"}` or `{"MaximiseToggled": null}` --
/// see [`app_shell_event_to_json`]'s doc for why this is hand-built rather
/// than derived.
fn bottom_panel_event_to_json(event: &BottomPanelEvent) -> JsonValue {
    match event {
        BottomPanelEvent::TabActivated(id) => serde_json::json!({"TabActivated": id}),
        BottomPanelEvent::TabClosed(id) => serde_json::json!({"TabClosed": id}),
        BottomPanelEvent::MaximiseToggled => serde_json::json!({"MaximiseToggled": null}),
        BottomPanelEvent::Resized(height) => serde_json::json!({"Resized": height}),
    }
}

/// The [`ShellApp`] [`QuiApp::run_shell`] drives: a thin napi-rs bridge
/// from its hooks to the JS callables held in a shared [`ShellState`] --
/// the napi-rs analog of `quadraui-py`'s own `PyShellApp`.
struct NodeShellApp {
    state: Rc<RefCell<ShellState>>,
}

impl NodeShellApp {
    /// Borrows `state` just long enough to pull out the live `Env` (`Copy`,
    /// so no borrow survives the call) and re-borrow the callback named by
    /// `pick`, if any, converting it to a callable [`Function`] via
    /// [`FunctionRef::borrow_back`] before the borrow is dropped -- the
    /// returned `Function` holds no reference back into `state`, so a
    /// callback invoked through it is free to record an error on `state`
    /// (via [`ShellState::record_error`]) without conflicting with this
    /// borrow. Returns `None` with nothing recorded when either no `Env`
    /// is live yet or no callback is registered under `pick`; records an
    /// error and returns `None` if `borrow_back` itself fails.
    fn callback<'scope>(
        state: &Rc<RefCell<ShellState>>,
        pick: impl FnOnce(&ShellState) -> &Option<NodeCallback>,
        env: &'scope Env,
    ) -> Option<Function<'scope, Unknown<'static>, Unknown<'static>>> {
        let borrowed = state.borrow();
        let callback = pick(&borrowed).as_ref()?;
        match callback.borrow_back(env) {
            Ok(func) => Some(func),
            Err(err) => {
                drop(borrowed);
                state.borrow_mut().record_error(err);
                None
            }
        }
    }
}

impl ShellApp for NodeShellApp {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let Some(env) = self.state.borrow().env else {
            return;
        };
        let Some(func) = Self::callback(&self.state, |s| &s.render_content, &env) else {
            return;
        };
        let placeholder: Unknown<'static> = match env.to_js_value(&JsonValue::Null) {
            Ok(value) => value,
            Err(err) => {
                self.state.borrow_mut().record_error(err);
                return;
            }
        };
        match func.call(placeholder) {
            Ok(result) => {
                if let Ok(lines) = env.from_js_value::<Vec<String>, _>(result) {
                    let td = text_display_from_lines(WidgetId::new("shell:content"), lines);
                    backend.draw_text_display(layout.main_content_bounds, &td);
                }
            }
            Err(err) => self.state.borrow_mut().record_error(err),
        }
    }

    fn handle(
        &mut self,
        event: UiEvent,
        _backend: &mut dyn Backend,
        _ctx: &ShellContext,
    ) -> Reaction {
        let Some(env) = self.state.borrow().env else {
            return Reaction::Continue;
        };
        let Some(func) = Self::callback(&self.state, |s| &s.handler, &env) else {
            return Reaction::Continue;
        };
        let render_content_present = self.state.borrow().render_content.is_some();
        let payload = match encode_via_json(env, "handle", &event) {
            Ok(value) => value,
            Err(err) => {
                self.state.borrow_mut().record_error(err);
                return Reaction::Exit;
            }
        };
        match func.call(payload) {
            Ok(result) if matches!(event, UiEvent::WindowClose) => {
                window_close_reaction(bool_from_return(result), render_content_present)
            }
            Ok(result) => {
                callback_reaction_for_render(bool_from_return(result), render_content_present)
            }
            Err(err) => {
                self.state.borrow_mut().record_error(err);
                Reaction::Exit
            }
        }
    }

    fn on_shell_event_ctx(&mut self, event: &AppShellEvent, _ctx: &ShellContext) {
        let Some(env) = self.state.borrow().env else {
            return;
        };
        let Some(func) = Self::callback(&self.state, |s| &s.on_shell_event, &env) else {
            return;
        };
        let payload: Unknown<'static> = match env.to_js_value(&app_shell_event_to_json(event)) {
            Ok(value) => value,
            Err(err) => {
                self.state.borrow_mut().record_error(err);
                return;
            }
        };
        if let Err(err) = func.call(payload) {
            self.state.borrow_mut().record_error(err);
        }
    }

    fn on_bottom_panel_event(&mut self, event: &BottomPanelEvent) {
        let Some(env) = self.state.borrow().env else {
            return;
        };
        let Some(func) = Self::callback(&self.state, |s| &s.on_bottom_panel_event, &env) else {
            return;
        };
        let payload: Unknown<'static> = match env.to_js_value(&bottom_panel_event_to_json(event)) {
            Ok(value) => value,
            Err(err) => {
                self.state.borrow_mut().record_error(err);
                return;
            }
        };
        if let Err(err) = func.call(payload) {
            self.state.borrow_mut().record_error(err);
        }
    }

    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        let Some(env) = self.state.borrow().env else {
            return Reaction::Continue;
        };
        let Some(func) = Self::callback(&self.state, |s| &s.on_tick, &env) else {
            return Reaction::Continue;
        };
        let render_content_present = self.state.borrow().render_content.is_some();
        let placeholder: Unknown<'static> = match env.to_js_value(&JsonValue::Null) {
            Ok(value) => value,
            Err(err) => {
                self.state.borrow_mut().record_error(err);
                return Reaction::Exit;
            }
        };
        match func.call(placeholder) {
            Ok(result) => {
                callback_reaction_for_render(bool_from_return(result), render_content_present)
            }
            Err(err) => {
                self.state.borrow_mut().record_error(err);
                Reaction::Exit
            }
        }
    }
}

/// Takes and returns this session's pending callback-raised error, if any
/// -- the `ShellState` counterpart of [`take_pending_error`].
fn take_pending_shell_error(state: &Rc<RefCell<ShellState>>) -> Result<()> {
    if let Some(err) = state.borrow_mut().pending_error.take() {
        return Err(err);
    }
    Ok(())
}

/// Decodes `runShell`'s `config` object into a [`ShellConfig`] via the
/// language-neutral decoder in `quadraui_binding_core`, through the same
/// [`decode_via_core`] materialization every other kind-dispatched decoder
/// in this crate uses -- `&serde_json::Value` implements
/// `serde::Deserializer` directly, so [`decode_shell_config`] is handed the
/// materialized value with no further wrapper. The napi-rs analog of
/// `quadraui-py`'s own `shell_config_from_dict`.
fn shell_config_from_descriptor(descriptor: Unknown) -> Result<ShellConfig> {
    decode_via_core("runShell", descriptor, |value| decode_shell_config(value))
}

/// Headless [`ShellApp`]-backed app: an AppShell (activity bar + sidebar +
/// optional bottom-panel tab strip) around JS-registered content and event
/// handlers. Built by [`QuiApp::run_shell`] -- there is no public
/// constructor on this type itself, mirroring how `quadraui-py`'s own
/// `QuiShellApp` is only ever reached through `QuiApp.run_shell`.
#[napi]
pub struct QuiShellApp {
    driver: Box<dyn ErasedShellDriver>,
    state: Rc<RefCell<ShellState>>,
    /// Mirrors [`QuiApp::pending_events`] -- queued by `pushEvent`/
    /// `pushKeyEvent`/`pushClick`, drained one per `step`/`run` call.
    pending_events: VecDeque<UiEvent>,
    /// Mirrors [`QuiApp::poisoned`].
    poisoned: bool,
    /// Mirrors [`QuiApp::in_dispatch`].
    in_dispatch: Cell<bool>,
}

#[napi]
impl QuiShellApp {
    /// Registers `handler` as the callable invoked (with the JS-converted
    /// event payload) for every event the shell chrome itself didn't
    /// consume. Replaces any previously registered handler. Raises if
    /// called re-entrantly.
    #[napi(js_name = "setHandler")]
    pub fn set_handler(
        &mut self,
        handler: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.state.borrow_mut().handler = Some(handler.create_ref()?);
        Ok(())
    }

    /// Registers `handler` as the no-argument callable invoked by
    /// `step`/`run` whenever it advances with no event queued. Replaces
    /// any previously registered tick handler. Raises if called
    /// re-entrantly.
    #[napi(js_name = "setTickHandler")]
    pub fn set_tick_handler(
        &mut self,
        handler: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.state.borrow_mut().on_tick = Some(handler.create_ref()?);
        Ok(())
    }

    /// Registers `callback` as the no-argument render callback invoked
    /// once per frame; its return value, if a JS array of strings, becomes
    /// the lines of a single-line-per-entry `TextDisplay` painted into the
    /// shell's main content area. Replaces any previously registered
    /// content callback. Raises if called re-entrantly.
    #[napi(js_name = "setRenderContent")]
    pub fn set_render_content(
        &mut self,
        callback: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.state.borrow_mut().render_content = Some(callback.create_ref()?);
        Ok(())
    }

    /// Registers `callback` as the callable invoked with one JS-converted
    /// `AppShellEvent` payload (see [`app_shell_event_to_json`]) whenever a
    /// panel switch, sidebar show/hide/resize, or bottom-panel resize/hide
    /// occurs. Raises if called re-entrantly.
    #[napi(js_name = "setOnShellEvent")]
    pub fn set_on_shell_event(
        &mut self,
        callback: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.state.borrow_mut().on_shell_event = Some(callback.create_ref()?);
        Ok(())
    }

    /// Registers `callback` as the callable invoked with one JS-converted
    /// `BottomPanelEvent` payload (see [`bottom_panel_event_to_json`])
    /// whenever the bottom-panel tab strip is interacted with. Only ever
    /// fires when this app's `config` included a `"bottom_panel"` entry.
    /// Raises if called re-entrantly.
    #[napi(js_name = "setOnBottomPanelEvent")]
    pub fn set_on_bottom_panel_event(
        &mut self,
        callback: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.state.borrow_mut().on_bottom_panel_event = Some(callback.create_ref()?);
        Ok(())
    }

    /// Decodes `descriptor` into a `UiEvent` and queues it for the next
    /// `step`/`run` call. Raises `InvalidDescriptorError` on a shape
    /// mismatch.
    #[napi(js_name = "pushEvent")]
    pub fn push_event(&mut self, descriptor: Unknown) -> Result<()> {
        self.check_not_dispatching()?;
        let event: UiEvent = decode_via_json("pushEvent", descriptor)?;
        self.pending_events.push_back(event);
        Ok(())
    }

    /// Queues a synthetic `KeyPressed` event for the character with
    /// Unicode code point `ch`, no modifiers held, not a repeat. Raises if
    /// `ch` isn't a valid Unicode scalar value.
    #[napi(js_name = "pushKeyEvent")]
    pub fn push_key_event(&mut self, ch: u32) -> Result<()> {
        self.check_not_dispatching()?;
        let ch = char::from_u32(ch)
            .ok_or_else(|| Error::from_reason(format!("invalid Unicode code point: {ch}")))?;
        self.pending_events.push_back(key_pressed_event(ch));
        Ok(())
    }

    /// Queues a synthetic `MouseDown` + `MouseUp` left-click at `(x, y)`,
    /// in the backend's native (cell) units -- the same coordinates
    /// `AppShellLayout`'s bounds (as JS-converted by `setOnShellEvent`'s
    /// payloads and this binding's own `find`) are expressed in. Use this
    /// to drive sidebar-toggle / panel-switch / bottom-panel-tab clicks
    /// from JS without hand-building two raw `pushEvent` descriptors.
    #[napi(js_name = "pushClick")]
    pub fn push_click(&mut self, x: f64, y: f64) -> Result<()> {
        self.check_not_dispatching()?;
        self.pending_events
            .extend(click_down_up_events(x as f32, y as f32));
        Ok(())
    }

    /// Resizes the underlying cell grid to `width`x`height`. Raises if
    /// either is zero.
    #[napi]
    pub fn resize(&mut self, env: Env, width: u16, height: u16) -> Result<()> {
        let (width, height) = checked_dimensions(width, height)?;
        self.guarded(env, |app| {
            app.driver.resize(width, height);
            Ok(())
        })
    }

    /// Advances the runner by one step, mirroring [`QuiApp::step`]'s
    /// queued-event / tick order (a `runShell` app has no waker/`User`
    /// event path, so there is no posted-value step to interleave).
    /// Returns `true` if still running, `false` once exited. Re-raises
    /// whatever exception a registered callback raised while handling this
    /// step.
    #[napi]
    pub fn step(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| app.step_once())
    }

    /// Runs up to `max_steps` steps (default 1). Stops early once the app
    /// exits. Returns `true` if still running afterwards, `false` once
    /// exited -- mirrors [`QuiApp::run`].
    #[napi]
    pub fn run(&mut self, env: Env, max_steps: Option<u32>) -> Result<bool> {
        let steps = max_steps.unwrap_or(1).max(1);
        let mut running = true;
        for _ in 0..steps {
            running = self.guarded(env, |app| app.step_once())?;
            if !running {
                break;
            }
        }
        Ok(running)
    }

    /// Returns the currently rendered screen as plain text, one line per
    /// row, mirroring `TuiDriver::screen`.
    #[napi]
    pub fn screen(&self) -> String {
        self.driver.screen()
    }

    /// Whether `needle` appears anywhere in the currently rendered screen.
    #[napi(js_name = "screenContains")]
    pub fn screen_contains(&self, needle: String) -> bool {
        self.driver.screen_contains(&needle)
    }

    /// The `(x, y)` cell position of the first occurrence of `needle` in
    /// the currently rendered screen, or `None` if it isn't present.
    #[napi]
    pub fn find(&self, needle: String) -> Option<(f64, f64)> {
        self.driver.find(&needle).map(|(x, y)| (x as f64, y as f64))
    }

    /// The `(x, y)` cell position of the center of bottom-panel tab
    /// `tab_idx` (`0` = first tab, in `config["bottom_panel"]["tabs"]`
    /// order), or `None` if this app's `config` had no `"bottom_panel"`,
    /// or `tabIdx` is out of range or scrolled out of view. A tab's label
    /// can repeat elsewhere on screen (or coincide with another tab's), so
    /// this resolves against the tab bar's own cached layout -- the same
    /// disambiguation [`Self::find`]'s doc warns a text needle can't give
    /// -- rather than `find(label)`.
    #[napi(js_name = "bottomPanelTabCenter")]
    pub fn bottom_panel_tab_center(&self, tab_idx: u32) -> Option<(f64, f64)> {
        self.driver
            .tab_center(
                &WidgetId::new(BOTTOM_PANEL_TABS_WIDGET_ID),
                tab_idx as usize,
            )
            .map(|(x, y)| (x as f64, y as f64))
    }
}

impl QuiShellApp {
    /// Mirrors [`QuiApp::check_not_dispatching`].
    fn check_not_dispatching(&self) -> Result<()> {
        if self.in_dispatch.get() {
            return Err(reentrant_call_error(
                "cannot call a QuiShellApp method from inside this same app's own registered \
                 handler/tick/render callback",
            ));
        }
        Ok(())
    }

    /// Mirrors [`QuiApp::guarded`], stashing `env` on this app's shared
    /// [`ShellState`] (rather than on an `AppLogic` field reachable via
    /// `driver.app_mut()`, which `Box<dyn ErasedShellDriver>` has no seam
    /// for) before `body` runs, so a registered handler/tick/render
    /// callback `body` reaches has a live `Env` to call back into JS with.
    fn guarded<T>(&mut self, env: Env, body: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.check_not_dispatching()?;
        if self.poisoned {
            return Err(Error::from_reason(
                "this QuiShellApp is poisoned by an earlier caught panic and can no longer be used",
            ));
        }
        self.state.borrow_mut().env = Some(env);
        self.in_dispatch.set(true);
        let result = panic::catch_unwind(AssertUnwindSafe(|| body(self)));
        self.in_dispatch.set(false);
        match result {
            Ok(result) => result,
            Err(payload) => {
                self.poisoned = true;
                Err(Error::from_reason(panic_message(&*payload)))
            }
        }
    }

    /// Mirrors [`QuiApp::step_once`], minus the `User`/waker path a
    /// `runShell` app doesn't have.
    fn step_once(&mut self) -> Result<bool> {
        let running = step_driver(&mut *self.driver, &mut self.pending_events);
        take_pending_shell_error(&self.state)?;
        Ok(running)
    }
}

#[napi]
impl QuiApp {
    /// Builds a [`QuiShellApp`]: a headless `ShellApp`/`ShellConfig`-backed
    /// app frame (activity bar + sidebar + optional bottom-panel tab
    /// strip) on a `width`x`height` cell grid, with `onEvent`/
    /// `renderContent`/`onShellEvent`/`onBottomPanelEvent`/`onTick`
    /// registered exactly as the matching `QuiShellApp.set*` method would
    /// right after construction -- the napi-rs analog of `quadraui-py`'s
    /// own `QuiApp.run_shell` staticmethod.
    ///
    /// This is a separate class ([`QuiShellApp`], not a mode switch on this
    /// `QuiApp`) rather than an instance method, for the same reason
    /// `quadraui-py`'s own `run_shell` is a separate class: `QuiApp` wraps
    /// a fixed `TuiDriver<NodeAppLogic>`, and `runShell` drives a
    /// different `AppLogic` (`quadraui::shell_adapter::ShellAdapter<NodeShellApp>`,
    /// erased behind `Box<dyn ErasedShellDriver>` since `driver_with_shell`
    /// returns an unnameable `TuiDriver<impl AppLogic>`).
    ///
    /// Raises if `width`/`height` is zero, or `InvalidDescriptorError` if
    /// `config` doesn't decode.
    #[napi(js_name = "runShell")]
    #[allow(clippy::too_many_arguments)]
    pub fn run_shell(
        env: Env,
        width: u16,
        height: u16,
        config: Unknown,
        on_event: Option<Function<'_, Unknown<'static>, Unknown<'static>>>,
        render_content: Option<Function<'_, Unknown<'static>, Unknown<'static>>>,
        on_shell_event: Option<Function<'_, Unknown<'static>, Unknown<'static>>>,
        on_bottom_panel_event: Option<Function<'_, Unknown<'static>, Unknown<'static>>>,
        on_tick: Option<Function<'_, Unknown<'static>, Unknown<'static>>>,
    ) -> Result<QuiShellApp> {
        let (width, height) = checked_dimensions(width, height)?;
        let shell_config = shell_config_from_descriptor(config)?;
        let state = Rc::new(RefCell::new(ShellState {
            handler: on_event.map(|f| f.create_ref()).transpose()?,
            on_tick: on_tick.map(|f| f.create_ref()).transpose()?,
            render_content: render_content.map(|f| f.create_ref()).transpose()?,
            on_shell_event: on_shell_event.map(|f| f.create_ref()).transpose()?,
            on_bottom_panel_event: on_bottom_panel_event.map(|f| f.create_ref()).transpose()?,
            // `TuiDriver::new` (via `driver_with_shell` below) paints the
            // first frame as part of construction, which can reach
            // `NodeShellApp::render_content` before `QuiShellApp::guarded`
            // ever runs -- so `env` must already be live on `state` before
            // that call, not only set lazily by `guarded` afterwards. Every
            // later dispatch refreshes this through `guarded` the normal
            // way; this is only load-bearing for frame 0.
            env: Some(env),
            pending_error: None,
        }));
        let app = NodeShellApp {
            state: state.clone(),
        };
        let driver = driver_with_shell(app, shell_config, width, height);
        Ok(QuiShellApp {
            driver: Box::new(driver),
            state,
            pending_events: VecDeque::new(),
            poisoned: false,
            in_dispatch: Cell::new(false),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A placeholder `Env` for a test that never registers a handler/tick/
    /// render callback: `QuiApp::guarded` always stashes its `env` argument
    /// on this app's `NodeAppLogic` before running its body (see
    /// `NodeAppLogic::env`'s doc), but nothing in this crate ever
    /// dereferences that value unless a callback is actually registered --
    /// and a plain Rust `#[test]`, with no live Node/V8 runtime behind it,
    /// has no real `napi_env` to construct one from anyway. Every test
    /// below that calls a method needing an `Env` registers no callback, so
    /// this value is stored but never read through.
    ///
    /// # Safety
    /// `Env::from_raw` itself performs no FFI call -- it only wraps the raw
    /// pointer -- so constructing one from a null pointer is safe by
    /// itself; the invariant this helper's callers must uphold is to never
    /// register a handler/tick/render callback on the same app, since any
    /// call made *through* this `Env` (e.g. `Env::to_js_value`) with a null
    /// `napi_env` would be undefined behaviour.
    fn null_env_for_test() -> Env {
        Env::from_raw(std::ptr::null_mut())
    }

    /// `QuiApp::new` rejects a zero `width`/`height` instead of constructing
    /// a driver that can never paint anything.
    #[test]
    fn new_rejects_zero_dimensions() {
        assert!(QuiApp::new(0, 10).is_err());
        assert!(QuiApp::new(10, 0).is_err());
        assert!(QuiApp::new(0, 0).is_err());
    }

    /// A fresh app renders a blank screen of the requested size: `height`
    /// lines, each `width` cells, every cell blank since nothing has been
    /// inserted into any slot yet.
    #[test]
    fn fresh_app_renders_blank_screen() {
        let mut app = QuiApp::new(8, 3).expect("valid dimensions");
        let screen = app
            .screen(null_env_for_test())
            .expect("render must not fail");
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines.len(), 3);
        for line in lines {
            assert!(
                line.trim().is_empty(),
                "expected a blank line, got {line:?}"
            );
        }
    }

    /// `step`/`run` advance the app and report it as still running, since
    /// no tick handler is registered to request exit.
    #[test]
    fn step_and_run_keep_running() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(app.step(null_env_for_test()).expect("step must not fail"));
        assert!(app
            .run(null_env_for_test(), Some(3))
            .expect("run must not fail"));
    }

    /// `pushKeyEvent` rejects a code point with no valid Unicode scalar
    /// value instead of queuing a malformed event.
    #[test]
    fn push_key_event_rejects_invalid_code_point() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(app.push_key_event(0xD800).is_err());
    }

    /// `slotClear` raises on an id outside the six fixed primitive slots.
    #[test]
    fn slot_clear_rejects_unknown_slot() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(app.slot_clear(null_env_for_test(), 999).is_err());
    }

    /// `slotClear` on a never-occupied slot is a no-op that reports
    /// `false`, not an error.
    #[test]
    fn slot_clear_on_empty_slot_reports_false() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(!app
            .slot_clear(null_env_for_test(), quadraui_binding_core::SLOT_BUTTON)
            .expect("a known slot id must not raise"));
    }

    /// A Rust panic raised from inside `guarded`'s body is caught and
    /// re-raised as an `Error`, and poisons the app so a later call raises
    /// without running its body -- mirrors `quadraui-py`'s
    /// `guarded_catches_panic_then_poisons`.
    #[test]
    fn guarded_catches_panic_then_poisons() {
        let mut app = QuiApp::new(10, 10).expect("valid dimensions");

        let panicked: Result<()> = app.guarded(null_env_for_test(), |_| panic!("boom"));
        assert!(panicked.is_err(), "a caught panic must surface as an error");
        assert!(app.poisoned);
        assert!(
            !app.in_dispatch.get(),
            "a caught panic must still clear the reentrancy guard"
        );

        let mut body_ran = false;
        let after_poison: Result<()> = app.guarded(null_env_for_test(), |_| {
            body_ran = true;
            Ok(())
        });
        assert!(!body_ran, "a poisoned app must not run the guarded body");
        assert!(after_poison.is_err(), "a poisoned app must keep erroring");
    }

    /// A [`QuiAppWaker`] wraps its posted value in a [`UserPayload`]
    /// recoverable via `downcast_ref::<JsonValue>` -- the exact mechanism
    /// [`NodeEventSink::dispatch_user`] relies on to deliver it to a
    /// registered handler as `{"User": value}`. Needs no live napi `Env`
    /// or `QuiApp` at all, unlike the end-to-end test below.
    #[test]
    fn user_payload_wraps_a_json_value_round_trippable_via_downcast() {
        let payload = UserPayload::new(serde_json::json!({"answer": 42}));
        let recovered = payload
            .downcast_ref::<JsonValue>()
            .expect("downcast to the wrapped JsonValue");
        assert_eq!(recovered, &serde_json::json!({"answer": 42}));
    }

    /// Posting through a [`QuiAppWaker`] from a background thread must not
    /// panic or poison the owning app, and the next `step()` must drain
    /// the delivered `UiEvent::User` without raising -- the napi-rs port
    /// of `quadraui-py`'s own
    /// `waker_post_from_a_background_thread_delivers_a_user_event`. No
    /// handler is registered here (constructing one needs a live napi
    /// `Env`, not available to a plain `#[test]` -- see
    /// `null_env_for_test`'s own doc), so this covers the waker/queue
    /// plumbing end to end rather than the JS-handler delivery itself,
    /// which [`user_payload_wraps_a_json_value_round_trippable_via_downcast`]
    /// and `quadraui-node/tests/`'s own black-box coverage exercise
    /// instead.
    #[test]
    fn waker_post_from_a_background_thread_delivers_a_user_event() {
        let mut app = QuiApp::new(10, 10).expect("valid dimensions");
        let waker = app.waker().expect("app is not poisoned");

        thread::spawn(move || {
            waker.post_value(serde_json::json!(42));
        })
        .join()
        .expect("a background thread calling post_value must not panic");

        let running = app
            .step(null_env_for_test())
            .expect("step must drain the delivered User event without raising");
        assert!(running, "delivering a User event must not exit the app");
    }

    /// `waker()` raises rather than handing out a handle once this app is
    /// poisoned by an earlier caught panic -- the one direct `&self` read
    /// of [`QuiApp::current_waker`] that bypasses [`QuiApp::guarded`]'s own
    /// poison check, so it needs this check inline instead. Delivering a
    /// stale handle obtained before the poisoning would still technically
    /// work (it only ever posts into a queue a live `step()` would drain),
    /// but every other `QuiApp` method already refuses a poisoned app, so
    /// `waker()` keeping that same promise is the contract this covers.
    #[test]
    fn waker_raises_poisoned_error_on_a_poisoned_app() {
        let mut app = QuiApp::new(10, 10).expect("valid dimensions");
        let panicked: Result<()> = app.guarded(null_env_for_test(), |_| panic!("boom"));
        assert!(panicked.is_err());
        assert!(app.poisoned);

        assert!(
            app.waker().is_err(),
            "waker() must raise on a poisoned app rather than handing out a handle"
        );
    }

    /// [`NodeEventSink::dispatch_user`]'s `unwrap_or(JsonValue::Null)`
    /// fallback: a [`UserPayload`] wrapping anything other than a
    /// [`JsonValue`] -- unreachable from this crate's own [`QuiAppWaker`],
    /// which only ever wraps one, but reachable from a future binding
    /// sharing this same `InteractionEngine` -- recovers as `{"User":
    /// null}` instead of panicking on the failed `downcast_ref`.
    #[test]
    fn user_event_with_a_foreign_payload_surfaces_as_user_none() {
        let payload = UserPayload::new(42_i32);
        let value = payload.downcast_ref::<JsonValue>().cloned();
        assert_eq!(value, None, "an i32 payload must not downcast to JsonValue");
        let wrapped = serde_json::json!({ "User": value.unwrap_or(JsonValue::Null) });
        assert_eq!(wrapped, serde_json::json!({ "User": null }));
    }

    // A `startTui` round trip (success or the no-real-tty failure path) is
    // deliberately not exercised as an in-process `#[test]` here:
    // `TuiRunner::new_with`'s very first step is `enable_raw_mode()`,
    // which -- if this test process happens to have a controlling
    // terminal at all, which a plain `cargo test` invocation sometimes
    // does -- would actually raw-mode it instead of failing fast, hanging
    // or corrupting whatever terminal ran the test suite. `quadraui-py`'s
    // own `test_run_tui.py` sidesteps exactly this by shelling out to a
    // detached (`setsid`) subprocess per test; `quadraui-node/tests/`'s
    // own black-box coverage does the same with `child_process.spawn`.

    /// `callback_reaction` (the pure function every callback-return mapping
    /// in this crate goes through -- see [`bool_from_return`]) treats only
    /// an explicit JS `true`/`false` as an opinion; `bool_from_return`
    /// narrows everything else (no explicit return, a non-boolean value) to
    /// `None`, which keeps running rather than exiting -- the napi-rs
    /// analog of `quadraui-py`'s
    /// `reaction_from_return_treats_only_explicit_false_as_exit`.
    #[test]
    fn callback_reaction_treats_only_explicit_false_as_exit() {
        assert_eq!(callback_reaction(Some(false)), Reaction::Exit);
        assert_eq!(callback_reaction(Some(true)), Reaction::Continue);
        assert_eq!(callback_reaction(None), Reaction::Continue);
    }

    /// `window_close_reaction` -- the opposite default from
    /// `callback_reaction` -- lets a `WindowClose` proceed by default (no
    /// handler, or a handler with no opinion) and only an explicit `true`
    /// vetoes it, mirroring `quadraui-py`'s own
    /// `native_app_logic_window_close_handler_veto_stays_open`/
    /// `..._confirm_exits`/`..._no_opinion_exits_by_default`/
    /// `..._with_no_handler_exits_by_default` cases.
    #[test]
    fn window_close_reaction_default_and_veto() {
        assert_eq!(
            window_close_reaction(None, false),
            Reaction::Exit,
            "no handler at all must let the close proceed"
        );
        assert_eq!(
            window_close_reaction(Some(false), false),
            Reaction::Exit,
            "an explicit false confirms the close"
        );
        assert_eq!(
            window_close_reaction(None, false),
            Reaction::Exit,
            "a handler with no opinion (no explicit return) must let the close proceed"
        );
        assert_eq!(
            window_close_reaction(Some(true), false),
            Reaction::Continue,
            "an explicit true vetoes the close and leaves the window open"
        );
        assert_eq!(
            window_close_reaction(Some(true), true),
            Reaction::Redraw,
            "a veto still redraws when a render callback is registered"
        );
    }

    /// `decode_layout_node` -- one of this crate's kind-dispatched core
    /// decoders (see the module doc) -- accepts a `&serde_json::Value`
    /// directly, exactly the medium `quadraui_binding_core`'s own tests
    /// pass it and the one [`to_descriptor_value`] would materialize a JS
    /// render-callback return value into once a render callback exists to
    /// call it from. No live napi `Env` is needed for this path, so it is
    /// covered here rather than in `quadraui-node/tests/`.
    #[test]
    fn decode_layout_node_round_trips_a_row_of_leaves() {
        let value = serde_json::json!({
            "row": [
                {"id": "left", "text_display": {"id": "text:left", "lines": []}},
                {"id": "right", "text_display": {"id": "text:right", "lines": []}},
            ]
        });
        let node = decode_layout_node("test", &value).expect("valid layout tree");
        let LayoutNode::Row(children) = node else {
            panic!("expected a row");
        };
        assert_eq!(children.len(), 2);
    }

    /// `decode_render_output` round-trips a flat render-callback payload
    /// into [`RenderOutput::Flat`] -- the shape [`to_descriptor_value`]
    /// would feed it once a render callback exists to call it from. See
    /// [`decode_layout_node_round_trips_a_row_of_leaves`]'s doc for why this
    /// needs no live napi `Env`.
    #[test]
    fn decode_render_output_round_trips_a_flat_frame() {
        let value = serde_json::json!({});
        let output = decode_render_output("test", &value).expect("an empty flat frame is valid");
        assert!(matches!(output, RenderOutput::Flat(_)));
    }

    /// A render-callback payload with a `"row"`/`"column"`/... key sniffs
    /// as the tree shape instead, round-tripping into
    /// [`RenderOutput::Tree`].
    #[test]
    fn decode_render_output_round_trips_a_tree_frame() {
        let value = serde_json::json!({
            "column": [
                {"id": "only", "text_display": {"id": "text:only", "lines": []}},
            ]
        });
        let output = decode_render_output("test", &value).expect("valid tree frame");
        assert!(matches!(output, RenderOutput::Tree(_)));
    }

    /// [`app_shell_event_to_json`] gives every [`AppShellEvent`] variant
    /// the same externally-tagged shape `quadraui-py`'s own
    /// `app_shell_event_to_py` builds -- a struct-like variant's payload
    /// nested under its tag, a unit variant tagged with `null`. Pure JSON
    /// construction, so this needs no live napi `Env` and is covered here
    /// rather than in `quadraui-node/tests/`.
    #[test]
    fn app_shell_event_to_json_tags_every_variant() {
        assert_eq!(
            app_shell_event_to_json(&AppShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:files")
            }),
            serde_json::json!({"PanelChanged": {"panel_id": "panel:files"}})
        );
        assert_eq!(
            app_shell_event_to_json(&AppShellEvent::SidebarHidden),
            serde_json::json!({"SidebarHidden": null})
        );
        assert_eq!(
            app_shell_event_to_json(&AppShellEvent::SidebarResized { new_width: 12.5 }),
            serde_json::json!({"SidebarResized": {"new_width": 12.5}})
        );
        assert_eq!(
            app_shell_event_to_json(&AppShellEvent::BottomItemClicked {
                id: WidgetId::new("bottom:terminal")
            }),
            serde_json::json!({"BottomItemClicked": {"id": "bottom:terminal"}})
        );
        assert_eq!(
            app_shell_event_to_json(&AppShellEvent::Consumed),
            serde_json::json!({"Consumed": null})
        );
    }

    /// [`bottom_panel_event_to_json`] tags a payload-carrying variant with
    /// its bare payload (no nested object), and a unit variant with
    /// `null` -- mirrors `quadraui-py`'s own `bottom_panel_event_to_py`.
    #[test]
    fn bottom_panel_event_to_json_tags_every_variant() {
        assert_eq!(
            bottom_panel_event_to_json(&BottomPanelEvent::TabActivated("bp:a".to_string())),
            serde_json::json!({"TabActivated": "bp:a"})
        );
        assert_eq!(
            bottom_panel_event_to_json(&BottomPanelEvent::TabClosed("bp:b".to_string())),
            serde_json::json!({"TabClosed": "bp:b"})
        );
        assert_eq!(
            bottom_panel_event_to_json(&BottomPanelEvent::MaximiseToggled),
            serde_json::json!({"MaximiseToggled": null})
        );
        assert_eq!(
            bottom_panel_event_to_json(&BottomPanelEvent::Resized(8.0)),
            serde_json::json!({"Resized": 8.0})
        );
    }
}
