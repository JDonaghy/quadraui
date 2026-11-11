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
//! `AppShellEvent`/`BottomPanelEvent` each have their own hand-written
//! `Serialize` impl upstream (not derived -- see that impl's own doc for
//! why), so this binding converts them through `serde_json::to_value` +
//! `Env::to_js_value` directly, the same `encode_via_json` shape every
//! other derived-`Serialize` enum in this file gets.
//!
//! **Live run loop (`startTui`/`startNative`) / waker.** [`QuiApp`]
//! otherwise only ever drives a headless `quadraui::tui::testing::TuiDriver`.
//! `startTui`/`startNative` are this crate's one structural departure from
//! `quadraui-py`'s own `run_tui`/`run_native`: those are *blocking* pump
//! loops that only work because `py.detach` lets other Python threads run
//! underneath; under Node a blocking pump would freeze the one libuv
//! reactor the whole process shares, and a background-thread pump is
//! impossible (every runner, `BindingHost`, and `InteractionEngine` are
//! `Rc`-based, so `NodeAppLogic` is `!Send`). Each instead `std::mem::take`s
//! this app's whole [`NodeAppLogic`] out of [`QuiApp::driver`] into a real
//! [`TuiRunner`] (`startTui`) or `GtkRunner`/`MacRunner`/`WinRunner`
//! (`startNative`, wrapped in `quadraui_binding_core::NativeExitPolicy` --
//! see [`QuiApp::start_native`]'s own doc) and stores it on [`QuiApp::live`]
//! as a [`LiveRunner`]; [`QuiApp::step`]/[`QuiApp::run`] then drive that
//! runner's own non-blocking `step()` pass by pass instead of the headless
//! driver's own `step_driver`, exactly as fit for a JS `setInterval`/
//! `setImmediate` loop (see `js/index.ts`'s `pump` helper) as the headless
//! path already was. [`QuiApp::check_not_live`] rejects every configuration
//! method (`setHandler`, `insertButton`, `actions`, ...) once a live
//! session is running, the same "configure once, up front, before
//! starting" discipline `quadraui-py`'s own doc describes for
//! `run_tui`/`run_native` -- there, Python's blocking call makes that
//! discipline unavoidable; here it has to be an explicit, checked
//! contract, since control returns to JS after every `step()`. An
//! `actions()` handle obtained *before* `startTui`/`startNative` keeps
//! working correctly through the live session regardless (it only ever
//! holds an `Rc` clone of state that moves, not copies, with the
//! [`NodeAppLogic`]). A `waker()` handle obtained before `startTui`/
//! `startNative` (on the `gtk`, `macos`, or `win` feature) keeps working
//! too -- all three swap [`QuiApp::current_waker`] to the live runner's
//! own backend waker for the session's duration -- see this doc's next
//! paragraph for how each gets there.
//!
//! `GtkRunner`, like `TuiRunner`, exposes a `Backend::waker`-reachable
//! accessor (`GtkRunner::waker`, captured off its own live `GtkBackend`
//! before `activate` moves it into that window's event-controller
//! closures), so [`QuiApp::start_native`]'s `gtk` arm swaps
//! [`QuiApp::current_waker`] onto it the same way [`QuiApp::start_tui`]
//! does for [`TuiRunner`]. `MacRunner`/`WinRunner` expose no comparable
//! post-construction accessor, nor an `app_mut()` one to reach back into
//! the moved [`NodeAppLogic`] once construction has returned (their own
//! `AppLogic` lives behind the backend's own event-loop/widget closures)
//! -- this crate's own [`NativeLiveState`] is the side channel around
//! that: a clone of the same `env`/`pending_error` cells the moved
//! [`NodeAppLogic`] still carries, kept alongside the real runner inside
//! [`LiveRunner`]'s `Gtk`/`Mac`/`Win` variants (`Gtk` alongside the real
//! swap, needed for the same reason regardless of it). The `macos` and
//! `win` features close that gap a different way: [`NodeAppLogic::setup`]
//! -- called by every runner, synchronously, before construction returns
//! -- stashes `backend`'s own [`Backend::waker`] into
//! [`NodeAppLogic::captured_waker`], a field [`QuiApp::start_native`]'s
//! `macos`/`win` arms each clone *before* `MacRunner::new_with`/
//! `WinRunner::new_with` moves the rest of this struct, then read right
//! after that call returns, swapping [`QuiApp::current_waker`] onto it
//! exactly like the `gtk` arm does.
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
//! **Wake-driven stepping (`armWakeNotify`/`js/index.ts`'s `run`).**
//! [`QuiApp::arm_wake_notify`] registers a no-argument [`ThreadsafeFunction`]
//! that [`QuiAppWaker::post`]/`::wake`/`::post_after`/`::wake_after` each
//! fire (via [`QuiAppWaker::notify`]) right after delivering into the plain
//! queue `poll_events`/`wait_events` already drain -- the `uv_async_send`
//! analog this module's own doc once argued was unnecessary, back when the
//! only addressable gain was shaving a few milliseconds off a short poll
//! cadence. `js/index.ts`'s `run` arms this once per live session and calls
//! `step()` the instant it fires instead of polling on a fixed interval, so
//! a session with no pending waker/timer work makes zero `step()` calls
//! between wakes rather than one every `pump` tick. This covers two of the
//! three wake sources a live session can have -- a background thread's
//! `waker()` post, and a `postAfter`/`wake_after` timer, both already
//! funneled through this same closure -- but not the third: real native
//! input (a keypress, a mouse click) arriving on an idle `startNative`
//! window. No backend exposes a poll-able OS handle a background thread
//! could wait on without owning the window (`GtkRunner`/`MacRunner`/
//! `WinRunner` are thread-affine, and moving one to a dedicated non-JS
//! thread would still need every `AppLogic` callback it drives to cross
//! back to the JS thread, which is a bigger seam than one `Backend` trait
//! method); `run` falls back to a long-period safety-net poll for that case
//! until a backend grows one. [`QuiAppWaker::post_after`]/`::wake_after`
//! still spawn their own background thread purely to delay *when* the
//! same already-thread-safe waker closure (and the notify alongside it) is
//! called, reusing both rather than building a separate delay mechanism.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as JsonValue;

use quadraui::tui::testing::{driver_with_shell, TuiDriver};
use quadraui::tui::{RunConfig, StepOutcome, TuiRunner};
use quadraui::{
    AppLogic, AppShellEvent, AppShellLayout, Backend, BackendCaps, BottomPanelEvent, Button,
    ContextMenuItem, DataTable, DialogButton, FloatMeasure, ImageSource, ListView, Notification,
    Point, PointerShape, Reaction, Rect, RgbaImage, ShellApp, ShellConfig, ShellContext, TabBar,
    TextDisplay, Toast, ToastCorner, Tooltip, TreeView, UiEvent, UserPayload, WidgetId,
};
#[cfg(test)]
use quadraui_binding_core::callback_reaction;
// Exactly the three conditions under which `LiveRunner::Gtk`/`Mac`/`Win`
// below actually constructs a real runner (`not(feature = "gtk")`/
// `not(feature = "macos")` excludes the higher-priority native backends the
// same way those variants' own `#[cfg]`s do) -- a looser gate here would
// leave this import unused on a host building more than one native feature
// where a higher-priority one wins, e.g. `--features macos,win` on a
// Windows `target_os` (`macos` wins the priority order below but can't
// target-check there, and `Win`'s own `not(feature = "macos")` then rules
// that variant out too).
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos", not(feature = "gtk")),
    all(
        feature = "win",
        target_os = "windows",
        not(feature = "gtk"),
        not(feature = "macos")
    )
))]
use quadraui_binding_core::NativeExitPolicy;
use quadraui_binding_core::{
    add_recent_document, begin_drag_out, begin_power_save_block, callback_reaction_for_render,
    caret_shape, clear_recent_documents, click_down_up_events, color_depth_name,
    decode_extra_primitive, decode_render_output, decode_shell_config, dialog_severity, draw_bands,
    end_power_save_block, font_role, guarded as core_guarded, install_crash_handler,
    is_default_protocol_client, key_pressed_event, login_item_enabled, menu_for_surface,
    merge_overrides, message_dialog_options_from_buttons, paint_layout, pointer_shape,
    record_pending_error, remove_as_default_protocol_client, render_overlays, resolve_app_path,
    resolve_layout_node, resolve_render_refs, set_app_identifier, set_as_default_protocol_client,
    set_dock_menu, set_jump_list, set_login_item, step_driver,
    take_pending_error as core_take_pending_error, text_display_from_lines, theme_color_value,
    toast_corner, update_apply_on_quit, update_check_for_update, update_download_and_stage,
    widget_state_snapshot, window_close_reaction, with_tray, with_window, BindingHost, DecodeError,
    ErasedShellDriver, EventSink, GuardedFailure, HitDetail, InteractionEngine, OverrideError,
    RenderOutput, ServiceError, UpdateInfoWire, WindowCloseHandling, SLOT_BUTTON, SLOT_DATA_TABLE,
    SLOT_LIST, SLOT_TAB_BAR, SLOT_TEXT_DISPLAY, SLOT_TREE,
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

/// The threadsafe function [`QuiApp::arm_wake_notify`] registers, called
/// with no arguments every time [`QuiAppWaker::notify`] runs -- shared
/// (like [`SharedWaker`]) between every [`QuiAppWaker`] handle this app
/// hands out, so arming it once covers every handle regardless of which
/// one a background thread happens to call `post`/`wake`/`postAfter`/
/// `wakeAfter` through. `None` until `arm_wake_notify` is called, and
/// again after `disarm_wake_notify` or this app being dropped (dropping
/// the last `Arc<WakeNotifyFn>` releases its underlying
/// `napi_threadsafe_function`, unblocking the event loop it was keeping
/// alive). An `Arc` around the `ThreadsafeFunction` itself, not a bare
/// value: `ThreadsafeFunction` has no public way to construct a second
/// handle to the same underlying call target outside this crate, so
/// every clone of this cell's content needs to share the one instance
/// `arm_wake_notify` built.
type WakeNotify = Arc<Mutex<Option<Arc<WakeNotifyFn>>>>;

/// The exact `ThreadsafeFunction` instantiation [`QuiApp::arm_wake_notify`]
/// builds -- `CalleeHandled = false` (the `build_threadsafe_function`
/// builder's own default, unlike [`ThreadsafeFunction`]'s bare-type
/// default of `true`) is spelled out explicitly here rather than left
/// implicit, since the two callers on [`QuiAppWaker`] (an infallible,
/// no-payload "please step soon" ping) need [`ThreadsafeFunction::call`]'s
/// `CalleeHandled = false` overload, which takes `T` directly rather than
/// `Result<T, ErrorStatus>`.
type WakeNotifyFn = ThreadsafeFunction<(), (), (), Status, false>;

/// A single [`Backend::waker`] closure, captured once by
/// [`NodeAppLogic::setup`] and read back exactly once by
/// [`QuiApp::start_native`]'s `macos` arm -- see
/// [`NodeAppLogic::captured_waker`]'s own doc for why this exists. `Cell`
/// rather than [`SharedWaker`]'s `Mutex`: this never needs to be `Sync`,
/// since it is only ever written (inside `setup`, on the same thread
/// constructing the runner) and taken (right after that same constructor
/// call returns) from plain, single-threaded Rust code, never from a
/// background thread the way a live [`QuiAppWaker`] clone is.
type CapturedWaker = Rc<Cell<Option<Arc<dyn Fn(UserPayload) + Send + Sync>>>>;

/// Side channel into a live native runner's [`NodeAppLogic`], kept outside
/// the runner itself once construction has moved that logic in --
/// `GtkRunner`/`MacRunner`/`WinRunner`, unlike [`TuiRunner`], expose no
/// `app_mut()` seam to reach back into it afterwards (their own `AppLogic`
/// lives behind the backend's own event-loop/widget closures) -- `GtkRunner`'s
/// own `waker()` accessor (see this crate's module doc's "Live run loop"
/// section) is narrower than that: just the one closure, not the backend
/// or the moved `AppLogic` it wraps.
/// [`QuiApp::start_native`] clones both fields straight off the
/// [`NodeAppLogic`] it is about to move (mirroring `quadraui-py`'s own
/// `native_error`/`loop_error` `Rc` clones taken the same way ahead of its
/// own `GtkRunner`/`MacRunner`/`WinRunner::new_with` call) and keeps the
/// clones here so [`LiveRunner::set_env`]/[`LiveRunner::take_error`] can
/// still write/read them while that logic is live inside the opaque
/// runner. Not needed for [`LiveRunner::Tui`], whose own `TuiRunner::app_mut()`
/// reaches the real field directly.
///
/// Gated exactly like the [`NativeExitPolicy`] import above -- the same
/// three conditions under which a real `LiveRunner::Gtk`/`Mac`/`Win`
/// variant actually exists to carry this alongside it.
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos", not(feature = "gtk")),
    all(
        feature = "win",
        target_os = "windows",
        not(feature = "gtk"),
        not(feature = "macos")
    ),
    test
))]
struct NativeLiveState {
    env: Rc<Cell<Option<Env>>>,
    pending_error: Rc<RefCell<Option<Error>>>,
}

/// A runner stood in for a real `GtkRunner`/`MacRunner`/`WinRunner` by
/// `#[cfg(test)]` code only, so [`QuiApp::check_not_live`]/[`QuiApp::live_exited`]'s
/// shared state machine can be exercised with a scripted [`StepOutcome`]
/// and no real display -- see this module's test for
/// `check_not_live`/`live_exited` behaving identically for a non-`Tui`
/// [`LiveRunner`] variant. Carries its own [`NativeLiveState`] exactly like
/// a real native variant would, so the same `set_env`/`take_error` paths
/// are covered too.
#[cfg(test)]
struct TestLiveRunner {
    state: NativeLiveState,
    outcome: Cell<StepOutcome>,
}

/// The real terminal- or window-backed runner [`QuiApp::start_tui`]/
/// [`QuiApp::start_native`] moves this app's whole [`NodeAppLogic`] into --
/// see this module's doc's "Live run loop" section for the overall shape.
/// `Gtk`/`Mac`/`Win` each carry a [`NativeLiveState`] alongside the real
/// runner instead of the runner itself, for the reason that type's own doc
/// explains. `Mac` additionally carries the `exit_requested` flag
/// [`quadraui_binding_core::NativeExitPolicy`] sets on any non-`WindowClose`
/// `Reaction::Exit` (see that type's own doc for why only the `macos`
/// feature without `gtk` also enabled needs this). TUI is verified here,
/// on any host, through the black-box `quadraui-node/tests/live_runner.test.ts`
/// suite; a real `Gtk`/`Mac`/`Win` session needs its own backend-specific
/// issue verified on that backend's own capability-lane host (`gtk` on a
/// Linux box with GTK4 installed, `macos`/`win` only on their own
/// platform) -- the same one-backend-per-issue split `CLAUDE.md`'s own doc
/// describes for `quadraui` itself, applied here one level down inside
/// this single crate's own feature table; this crate's own `#[cfg(test)]`
/// `TestLiveRunner` variant covers the shared, backend-neutral state
/// machine without one.
enum LiveRunner {
    /// A live terminal takeover via [`TuiRunner`] -- [`QuiApp::start_tui`].
    /// `Box`ed because `TuiRunner<NodeAppLogic>` is far larger than every
    /// other variant (a `gtk`-only build would otherwise size the whole
    /// enum -- and every `Option<LiveRunner>`/match on it -- to this one
    /// variant's footprint for no benefit to the variants that are
    /// actually small).
    Tui(Box<TuiRunner<NodeAppLogic>>),
    /// A live GTK window -- [`QuiApp::start_native`]. Takes priority over
    /// `Mac`/`Win` when more than one native feature is enabled, mirroring
    /// `quadraui-py`'s own `run_native` priority.
    #[cfg(feature = "gtk")]
    Gtk(quadraui::gtk::GtkRunner, NativeLiveState),
    /// A live AppKit window -- [`QuiApp::start_native`]. Only ever
    /// constructed when also built for a macOS `target_os` and `gtk` is
    /// not also enabled -- see `start_native`'s own doc for the arm that
    /// covers every other combination of the `macos` feature.
    #[cfg(all(feature = "macos", target_os = "macos", not(feature = "gtk")))]
    Mac(quadraui::macos::MacRunner, NativeLiveState, Rc<Cell<bool>>),
    /// A live Win32 window -- [`QuiApp::start_native`]. Only ever
    /// constructed when also built for a Windows `target_os` and neither
    /// `gtk` nor `macos` is also enabled -- mirrors `Mac`'s own doc.
    #[cfg(all(
        feature = "win",
        target_os = "windows",
        not(feature = "gtk"),
        not(feature = "macos")
    ))]
    Win(
        quadraui::win::WinRunner<NativeExitPolicy<NodeAppLogic>>,
        NativeLiveState,
    ),
    /// Stands in for a real native variant in this module's own tests --
    /// see [`TestLiveRunner`]'s own doc.
    #[cfg(test)]
    Test(TestLiveRunner),
}

impl LiveRunner {
    /// Writes `env` into whichever cell [`NodeAppLogic::render`]/`handle`/
    /// `tick` will read it back from -- [`TuiRunner::app_mut`]'s real
    /// `env` field for [`LiveRunner::Tui`] (the napi-rs analog of
    /// [`TuiDriver::app_mut`] for a live runner instead of the headless
    /// driver), or the [`NativeLiveState`] side channel for every other
    /// variant, needed for the same reason [`QuiApp::guarded`] stashes its
    /// live `Env` before running its body.
    fn set_env(&mut self, env: Env) {
        match self {
            LiveRunner::Tui(runner) => runner.app_mut().env.set(Some(env)),
            #[cfg(feature = "gtk")]
            LiveRunner::Gtk(_, state) => state.env.set(Some(env)),
            #[cfg(all(feature = "macos", target_os = "macos", not(feature = "gtk")))]
            LiveRunner::Mac(_, state, _) => state.env.set(Some(env)),
            #[cfg(all(
                feature = "win",
                target_os = "windows",
                not(feature = "gtk"),
                not(feature = "macos")
            ))]
            LiveRunner::Win(_, state) => state.env.set(Some(env)),
            #[cfg(test)]
            LiveRunner::Test(test) => test.state.env.set(Some(env)),
        }
    }

    /// One non-blocking pass through whichever runner this variant wraps.
    /// Never blocks, mirroring every backend's own non-blocking `step()`.
    /// `Mac` folds its own `exit_requested` flag into the returned
    /// [`StepOutcome`] right here, so [`QuiApp::step_once`]'s own
    /// [`StepOutcome::Exited`] handling covers it without needing a second,
    /// variant-specific check of its own.
    fn step(&mut self) -> io::Result<StepOutcome> {
        match self {
            LiveRunner::Tui(runner) => runner.step(),
            #[cfg(feature = "gtk")]
            LiveRunner::Gtk(runner, _) => Ok(runner.step()),
            #[cfg(all(feature = "macos", target_os = "macos", not(feature = "gtk")))]
            LiveRunner::Mac(runner, _, exit_requested) => {
                let outcome = runner.step();
                Ok(if exit_requested.get() {
                    StepOutcome::Exited
                } else {
                    outcome
                })
            }
            #[cfg(all(
                feature = "win",
                target_os = "windows",
                not(feature = "gtk"),
                not(feature = "macos")
            ))]
            LiveRunner::Win(runner, _) => runner
                .step()
                .map_err(|err| io::Error::other(err.to_string())),
            #[cfg(test)]
            LiveRunner::Test(test) => Ok(test.outcome.get()),
        }
    }

    /// Takes and re-raises any error a registered callback recorded during
    /// the [`Self::step`] pass that just ran -- [`take_pending_error`]'s
    /// own [`NodeAppLogic::pending_error`] field for [`LiveRunner::Tui`],
    /// or the [`NativeLiveState`] side channel's clone of the same cell for
    /// every other variant.
    fn take_error(&mut self) -> Result<()> {
        match self {
            LiveRunner::Tui(runner) => take_pending_error(runner.app_mut()),
            #[cfg(feature = "gtk")]
            LiveRunner::Gtk(_, state) => take_pending_error_cell(&state.pending_error),
            #[cfg(all(feature = "macos", target_os = "macos", not(feature = "gtk")))]
            LiveRunner::Mac(_, state, _) => take_pending_error_cell(&state.pending_error),
            #[cfg(all(
                feature = "win",
                target_os = "windows",
                not(feature = "gtk"),
                not(feature = "macos")
            ))]
            LiveRunner::Win(_, state) => take_pending_error_cell(&state.pending_error),
            #[cfg(test)]
            LiveRunner::Test(test) => take_pending_error_cell(&test.state.pending_error),
        }
    }
}

/// Called by [`QuiApp::stop_live`] right before it drops `live` -- gives a
/// live [`LiveRunner::Gtk`] session a chance to tear its window down
/// first, since `GtkRunner` has no `Drop` impl of its own to do that (see
/// that type's own doc). A no-op for every other variant: `TuiRunner`
/// restores the terminal on `Drop` already, and neither `MacRunner` nor
/// `WinRunner` expose a comparable `request_exit()` to call here.
fn request_exit_before_drop(live: &mut LiveRunner) {
    #[cfg(feature = "gtk")]
    {
        if let LiveRunner::Gtk(runner, _) = live {
            runner.request_exit();
            let _ = runner.step();
        }
    }
    #[cfg(not(feature = "gtk"))]
    {
        let _ = live;
    }
}

/// One action a [`QuiAppActions`] method recorded, already decoded and
/// validated, waiting for [`flush_deferred_actions`] to apply it -- the
/// napi-rs analog of `quadraui-py`'s own `DeferredAction`. Each variant
/// carries exactly the already-decoded arguments the matching
/// [`QuiAppActions`] method passes to the same underlying
/// `quadraui`/`quadraui_binding_core` call, so applying one in
/// [`flush_deferred_actions`] is a direct Rust call with no further
/// decoding or validation left to do -- except [`Self::SetDockMenu`]/
/// [`Self::SetJumpList`], whose own `quadraui_binding_core::services` call
/// still does its own items decode at flush time, same as every other
/// variant's matching `QuiApp` method does synchronously -- see
/// `quadraui-py`'s `test_deferred_actions.py` module doc for why that one
/// exception exists. [`Self::HideTooltip`] carries no argument at all --
/// `QuiApp::hideTooltip` itself takes none -- so [`flush_deferred_actions`]
/// applies it via the same [`InteractionEngine::hide_tooltip`] call that
/// method makes directly.
enum DeferredAction {
    MessageDialog(quadraui::MessageDialogOptions),
    ContextMenu(Vec<ContextMenuItem>, Point),
    Toast(Toast, ToastCorner),
    Tooltip(Tooltip, Rect),
    HideTooltip,
    Notification(Notification),
    OpenPath(PathBuf),
    RevealInFileManager(PathBuf),
    MoveToTrash(PathBuf),
    RestoreFromTrash(PathBuf),
    ApplyUpdateOnQuit(u64),
    EndPowerSaveBlock(u64),
    SetLoginItem(bool, bool, Vec<String>),
    SetAsDefaultProtocolClient(String),
    RemoveAsDefaultProtocolClient(String),
    BeginDragOut(Vec<String>, Option<String>),
    AddRecentDocument(PathBuf),
    ClearRecentDocuments,
    SetDockMenu(JsonValue),
    SetJumpList(JsonValue),
    TrayIcon(ImageSource),
    TrayTooltip(String),
    TrayMenu(Vec<ContextMenuItem>),
    DocumentEdited(bool),
    RepresentedFilename(Option<PathBuf>),
    Cursor(PointerShape),
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
    ///
    /// `Rc`-wrapped, like [`Self::pending_error`], rather than a bare
    /// `Cell`: `GtkRunner`/`MacRunner`/`WinRunner` expose no `app_mut()`
    /// seam once this whole `NodeAppLogic` has moved into one (unlike
    /// `TuiRunner`, whose own `app_mut()` still reaches this field
    /// directly) -- see [`NativeLiveState`]'s own doc for how `startNative`
    /// keeps writing into this cell from outside regardless.
    env: Rc<Cell<Option<Env>>>,
    /// Captured by [`Self::setup`] off the live `backend` every runner
    /// hands it exactly once, synchronously, before returning
    /// (`TuiRunner::new`/`GtkRunner::new_with`/`MacRunner::new_with`/
    /// `win::run::run` all call `app.setup(&mut backend)` before this
    /// whole `NodeAppLogic` moves anywhere else). `TuiRunner`/`GtkRunner`
    /// expose their own post-construction `.waker()`/`Backend::waker()`
    /// accessor `QuiApp::start_tui`/`start_native`'s `gtk` arm reads
    /// instead and never touch this field; `MacRunner` exposes no such
    /// accessor once the `AppLogic` has moved in (see `start_native`'s own
    /// doc), so this is the only window in which anything outside the
    /// runner can still get at this session's waker -- `start_native`'s
    /// `macos` arm clones this `Rc` before `MacRunner::new_with` moves the
    /// rest of this struct, then `take()`s it right after that call
    /// returns.
    captured_waker: CapturedWaker,
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

    /// Drives [`Self::host`]'s hover-tooltip tracker for this dispatch —
    /// [`BindingHost::drive_hover_tooltip`] fed [`BindingHost::now_ms`]'s
    /// own real wall clock, or a test-pinned value once
    /// `QuiApp.setTestClockMs` has set [`BindingHost::test_clock_ms`] (see
    /// that field's own doc), and [`Self::host`]'s own
    /// `style.tooltip_delay_ms`/`tooltip_auto_dismiss_ms` (or the
    /// behaviour-preserving defaults when no style has been set) —
    /// mirrors `quadraui-py`'s own `PyAppLogic::drive_hover_tooltip`,
    /// including forwarding `backend` so the pending/auto-dismiss wake
    /// reaches `backend.request_frame_in` directly rather than only
    /// through the returned [`Reaction`].
    /// Called from both [`AppLogic::handle`] (so a hover change is
    /// noticed the same frame) and [`AppLogic::tick`] (so the delay still
    /// elapses with the pointer held still and no further event
    /// arriving).
    fn drive_hover_tooltip(&mut self, backend: &mut dyn Backend) -> Reaction {
        let now_ms = self.host.now_ms();
        let style = self.host.style.unwrap_or_default();
        self.host.drive_hover_tooltip(
            &mut self.engine,
            backend,
            now_ms,
            style.tooltip_delay_ms,
            style.tooltip_auto_dismiss_ms,
        )
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
    /// `PyAppLogic::setup`. Also stashes `backend`'s own [`Backend::waker`]
    /// into [`Self::captured_waker`] -- see that field's own doc for why.
    fn setup(&mut self, backend: &mut dyn Backend) {
        self.host.setup(backend);
        self.captured_waker.set(Some(backend.waker()));
    }

    /// With no render callback registered, resolves [`Self::host`]'s six
    /// fixed slots against [`InteractionEngine::widget_state`] via
    /// [`resolve_render_refs`] (so an uncontrolled `Tree`/`List`/
    /// `DataTable`'s cursor/scroll, `TabBar`'s active tab, etc. paint this
    /// frame's resolved value rather than whatever raw descriptor the app
    /// set) before painting them plus the anchored overlay pass every
    /// binding's `render` runs -- see [`render_overlays`]'s own doc. With
    /// a render callback registered, invokes it instead via
    /// [`render_output_from_callback`] and resolves+paints whatever flat/
    /// tree frame it returned the identical way, skipping [`Self::host`]
    /// entirely for this frame -- mirrors `quadraui-py`'s own `render`.
    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut hits = Vec::new();
        let chrome_interaction = |id: WidgetId| self.engine.chrome_interaction(id);
        let Some(callback) = self.render_callback.as_ref() else {
            let resolved =
                resolve_render_refs(self.engine.widget_state(), backend, self.host.render_refs());
            draw_bands(backend, resolved.as_refs(), &mut hits, &chrome_interaction);
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
                let resolved =
                    resolve_render_refs(self.engine.widget_state(), backend, frame.as_refs());
                draw_bands(backend, resolved.as_refs(), &mut hits, &chrome_interaction)
            }
            Some(RenderOutput::Tree(mut node)) => {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                resolve_layout_node(self.engine.widget_state(), backend, area, &mut node);
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
        let reaction = reaction.merge(self.drive_hover_tooltip(backend));
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
        let reaction = reaction.merge(self.drive_hover_tooltip(backend));
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
/// `quadraui-py`'s own `record_error`, via [`record_pending_error`]'s
/// shared "first error wins" contract. Without this, a handler raise
/// (recorded first) followed by the render callback also raising while
/// painting the same frame would have the render error silently clobber the
/// handler's -- the one a caller actually wants to see, since it is closer
/// to the root cause.
fn record_error(pending_error: &Rc<RefCell<Option<Error>>>, err: Error) {
    record_pending_error(&mut pending_error.borrow_mut(), err);
}

/// Takes and re-raises any error a registered callback recorded during the
/// `step`/render pass that just ran -- mirrors `quadraui-py`'s
/// `take_pending_error`.
fn take_pending_error(app: &mut NodeAppLogic) -> Result<()> {
    take_pending_error_cell(&app.pending_error)
}

/// [`take_pending_error`]'s shared tail, operating directly on the
/// `Rc`-shared cell rather than through a [`NodeAppLogic`] -- the form
/// [`LiveRunner::take_error`] needs for every variant but [`LiveRunner::Tui`],
/// whose own `NodeAppLogic` is reachable only through a [`NativeLiveState`]
/// clone of this same cell, never the real [`NodeAppLogic`] itself.
fn take_pending_error_cell(pending_error: &Rc<RefCell<Option<Error>>>) -> Result<()> {
    core_take_pending_error(&mut pending_error.borrow_mut())
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
            DeferredAction::Tooltip(tooltip, anchor) => {
                app.engine.show_tooltip(tooltip, anchor);
            }
            DeferredAction::HideTooltip => {
                app.engine.hide_tooltip();
            }
            DeferredAction::Notification(notification) => {
                if let Err(err) = service_result(
                    "send_notification",
                    backend.services().send_notification(notification),
                ) {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::OpenPath(path) => {
                if let Err(err) = service_result("open_path", backend.services().open_path(&path)) {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::RevealInFileManager(path) => {
                if let Err(err) = service_result(
                    "reveal_in_file_manager",
                    backend.services().reveal_in_file_manager(&path),
                ) {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::MoveToTrash(path) => {
                if let Err(err) =
                    service_result("move_to_trash", backend.services().move_to_trash(&path))
                {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::RestoreFromTrash(path) => {
                if let Err(err) = service_result(
                    "restore_from_trash",
                    backend.services().restore_from_trash(&path),
                ) {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::ApplyUpdateOnQuit(handle) => {
                if let Err(err) = update_apply_on_quit(backend.services(), handle) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("apply_on_quit", err),
                    );
                }
            }
            DeferredAction::EndPowerSaveBlock(handle) => {
                if let Err(err) = end_power_save_block(backend.services(), handle) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("end_power_save_block", err),
                    );
                }
            }
            DeferredAction::SetLoginItem(enabled, open_hidden, args) => {
                if let Err(err) = set_login_item(backend.services(), enabled, open_hidden, args) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("set_login_item", err),
                    );
                }
            }
            DeferredAction::SetAsDefaultProtocolClient(scheme) => {
                if let Err(err) = set_as_default_protocol_client(backend.services(), &scheme) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("set_as_default_protocol_client", err),
                    );
                }
            }
            DeferredAction::RemoveAsDefaultProtocolClient(scheme) => {
                if let Err(err) = remove_as_default_protocol_client(backend.services(), &scheme) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("remove_as_default_protocol_client", err),
                    );
                }
            }
            DeferredAction::BeginDragOut(files, text) => {
                if let Err(err) = begin_drag_out(backend.services(), files, text) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("begin_drag_out", err),
                    );
                }
            }
            DeferredAction::AddRecentDocument(path) => {
                if let Err(err) = add_recent_document(backend.services(), &path.to_string_lossy()) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("add_recent_document", err),
                    );
                }
            }
            DeferredAction::ClearRecentDocuments => {
                if let Err(err) = clear_recent_documents(backend.services()) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("clear_recent_documents", err),
                    );
                }
            }
            DeferredAction::SetDockMenu(items) => {
                if let Err(err) = set_dock_menu(backend.services(), items) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("set_dock_menu", err),
                    );
                }
            }
            DeferredAction::SetJumpList(items) => {
                if let Err(err) = set_jump_list(backend.services(), items) {
                    record_error(
                        &app.pending_error,
                        service_error_to_napi("set_jump_list", err),
                    );
                }
            }
            DeferredAction::TrayIcon(icon) => {
                if let Err(err) =
                    service_result("set_tray_icon", with_tray(backend, |t| t.set_icon(icon)))
                {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::TrayTooltip(tooltip) => {
                if let Err(err) = service_result(
                    "set_tray_tooltip",
                    with_tray(backend, |t| t.set_tooltip(&tooltip)),
                ) {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::TrayMenu(items) => {
                let menu = menu_for_surface(WidgetId::new("qui-tray-menu"), items);
                if let Err(err) =
                    service_result("set_tray_menu", with_tray(backend, |t| t.set_menu(&menu)))
                {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::DocumentEdited(edited) => {
                if let Err(err) = service_result(
                    "set_document_edited",
                    with_window(backend, |w| w.set_document_edited(edited)),
                ) {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::RepresentedFilename(path) => {
                if let Err(err) = service_result(
                    "set_represented_filename",
                    with_window(backend, |w| w.set_represented_filename(path.as_deref())),
                ) {
                    record_error(&app.pending_error, err);
                }
            }
            DeferredAction::Cursor(shape) => {
                // Fire-and-forget, the same shape `quadraui-py`'s own
                // `DeferredAction::Cursor` arm uses: whether the glyph was
                // actually applied is only observable synchronously via
                // `QuiApp::set_cursor`'s own return value, and a deferred
                // action by definition isn't applied until after that
                // return value would already be gone.
                let _ = backend.set_cursor(shape);
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
        return Err(invalid_dimensions_error(
            "width and height must both be non-zero",
        ));
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

/// Mirrors [`invalid_descriptor_error`] for a
/// [`quadraui_binding_core::services::ServiceError::Unsupported`] --
/// this backend has no implementation at all for the platform-services
/// seam called, distinct from a genuine failure ([`internal_error`]) --
/// the napi-rs analog of `quadraui-py`'s `UnsupportedError` exception
/// subclass.
fn unsupported_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("UnsupportedError: {detail}"))
}

/// Maps a `quadraui_binding_core::services` call's [`ServiceError`] to this
/// binding's own tagged-`Error` taxonomy, prefixed with `what` -- the one
/// place every platform-services method this crate exposes (update,
/// power-save blocker, launch-at-login, protocol registration, drag-out,
/// recent documents/dock menu/jump list, `appPath`, `installCrashHandler`)
/// routes its `Err` through, the napi-rs analog of `quadraui-py`'s
/// `service_error_to_py`. [`ServiceError::Unsupported`] becomes
/// [`unsupported_error`] (this backend has no implementation at all --
/// distinct from a genuine failure); [`ServiceError::Failure`] becomes
/// [`internal_error`] (the backend tried and it broke);
/// [`ServiceError::InvalidArgument`] becomes [`invalid_descriptor_error`]
/// (a wire argument -- an enum-name string, a descriptor object -- never
/// even reached a service call).
fn service_error_to_napi(what: &str, err: ServiceError) -> Error {
    match err {
        ServiceError::Unsupported => {
            unsupported_error(format!("{what}: unsupported on this backend"))
        }
        ServiceError::Failure(context) => internal_error(format!("{what}: {context}")),
        ServiceError::InvalidArgument(message) => {
            invalid_descriptor_error(format!("{what}: {message}"))
        }
        // `ServiceError` is `#[non_exhaustive]`: a future core variant this
        // leaf doesn't know about yet still raises a tagged error rather
        // than failing to compile.
        other => internal_error(format!("{what}: {other}")),
    }
}

/// Maps a [`quadraui::ServiceResult`] failure to [`internal_error`],
/// prefixed with `what` -- the one place every `PlatformServices`/
/// `WindowControl`/`TrayService` method this crate exposes
/// (`openPath`/`moveToTrash`/.../`setTrayIcon`/`setDocumentEdited`/...)
/// routes its `Err` through, the napi-rs analog of `quadraui-py`'s own
/// `service_result`.
fn service_result(what: &str, result: quadraui::ServiceResult<()>) -> Result<()> {
    result.map_err(|err| internal_error(format!("{what}: {err:?}")))
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

/// Mirrors [`invalid_descriptor_error`] for [`QuiApp::start_native`] called
/// on a binary built with none of the `gtk`/`macos`/`win` features --
/// there is no native runner at all to hand this app's [`NodeAppLogic`]
/// off to. Only ever called from `start_native`'s own matching arm, which
/// shares this exact `cfg` gate.
#[cfg(not(any(feature = "gtk", feature = "macos", feature = "win")))]
fn no_native_backend_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("NoNativeBackendError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for [`QuiApp::start_native`]'s `gtk`
/// arm finding no display to open a window on -- `quadraui::gtk::display_available`'s
/// own pre-flight, mirroring `quadraui-py`'s `NoDisplayError`. Checked
/// before `GtkRunner::new_with` is ever called: that constructor has no
/// way to fail gracefully here (see [`QuiApp::start_native`]'s own doc),
/// so this guard is the only thing standing between a headless call and
/// GTK's own `gtk_init()` printing a warning and calling `exit(1)` on the
/// whole Node process.
#[cfg(feature = "gtk")]
fn no_display_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("NoDisplayError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for [`QuiApp::start_native`]'s `gtk`
/// arm being given an `app_id` that is not a valid `GApplication` id --
/// `quadraui::gtk::app_id_is_valid`'s own check, mirroring `quadraui-py`'s
/// `InvalidAppIdError`.
#[cfg(feature = "gtk")]
fn invalid_app_id_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("InvalidAppIdError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for a slot id argument that is not
/// one of the `SLOT_*` constants -- [`QuiApp::slot_clear`]'s own check --
/// the napi-rs analog of `quadraui-py`'s `UnknownSlotError` exception
/// subclass.
fn unknown_slot_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("UnknownSlotError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for a `u32` code point argument
/// that is not a valid Unicode scalar value -- `pushKeyEvent`'s own check --
/// the napi-rs analog of `quadraui-py`'s `InvalidCharError` exception
/// subclass.
fn invalid_char_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("InvalidCharError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for a `QuiApp`/`QuiShellApp` method
/// called after an earlier caught panic poisoned this handle -- see
/// [`QuiApp::guarded`]'s doc -- the napi-rs analog of `quadraui-py`'s
/// `PoisonedError` exception subclass.
fn poisoned_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("PoisonedError: {detail}"))
}

/// Mirrors [`invalid_descriptor_error`] for a Rust panic caught at the
/// [`QuiApp::guarded`]/`QuiShellApp::guarded` boundary -- the napi-rs
/// analog of `quadraui-py`'s `PanicError` exception subclass.
fn panic_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("PanicError: {detail}"))
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

/// The camelCase JS shape `QuiApp::widget_state` encodes
/// [`quadraui_binding_core::WidgetStateSnapshot`] into -- every other
/// field this crate hands to JS already reaches it camelCase (`scrollPx`,
/// `scrollOffset` on the six fixed descriptors themselves), so this
/// mirrors that rather than exposing the snapshot's own Rust
/// `snake_case` field names verbatim.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WidgetStateJs {
    selected_path: Option<Vec<u16>>,
    selected_index: Option<usize>,
    scroll_offset: Option<usize>,
    // Widens through `serde_json::to_value`'s `f32` -> `f64` promotion on
    // its way to JS, same as every other `f32` this crate hands across the
    // boundary -- a ratio like `0.3` round-trips as `0.30000001192092896`,
    // not the shorter decimal a hand-authored `f64` literal would give.
    split_ratio: Option<f32>,
    active_tab: Option<usize>,
    expanded: Option<Vec<Vec<u16>>>,
}

impl From<quadraui_binding_core::WidgetStateSnapshot> for WidgetStateJs {
    fn from(snapshot: quadraui_binding_core::WidgetStateSnapshot) -> Self {
        Self {
            selected_path: snapshot.selected_path,
            selected_index: snapshot.selected_index,
            scroll_offset: snapshot.scroll_offset,
            split_ratio: snapshot.split_ratio,
            active_tab: snapshot.active_tab,
            expanded: snapshot.expanded,
        }
    }
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

    /// Queues a [`DeferredAction::Tooltip`] -- same arguments, same
    /// decoding, as `QuiApp.showTooltip`. `tooltip` decodes as a plain
    /// object shaped like [`Tooltip`]; `anchor` as a plain
    /// `{x, y, width, height}` object.
    #[napi(js_name = "showTooltip")]
    pub fn show_tooltip(&self, env: Env, tooltip: Unknown, anchor: Unknown) -> Result<()> {
        let tooltip: Tooltip = decode_direct(env, "showTooltip", tooltip)?;
        let anchor: Rect = decode_direct(env, "showTooltip", anchor)?;
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::Tooltip(tooltip, anchor));
        Ok(())
    }

    /// Queues a [`DeferredAction::HideTooltip`] -- no arguments, same as
    /// `QuiApp.hideTooltip`. For a tooltip source with no `Button` to carry
    /// it (a toolbar action's own `tooltip`, or a tree/data-table row), the
    /// handler is the only thing that knows when to dismiss it -- typically
    /// on the hit-tested `MouseLeft` for the widget it showed on
    /// `MouseEntered`. Calling `app.hideTooltip()` straight from inside that
    /// same handler would raise `ReentrantCallError`; this queues the exact
    /// same engine call instead.
    #[napi(js_name = "hideTooltip")]
    pub fn hide_tooltip(&self) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::HideTooltip);
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

    /// Queues a [`DeferredAction::ApplyUpdateOnQuit`] -- same argument as
    /// `quadraui-py`'s `apply_on_quit`. `handle` is `i64`, not `u64`:
    /// napi-rs has no `FromNapiValue`/`ToNapiValue` for a bare `u64` (unlike
    /// `i64`, which it maps to/from a JS `number`), and every handle this
    /// crate's registries mint (see `quadraui_binding_core::services`'s
    /// module doc) fits well inside `i64::MAX` in practice.
    #[napi(js_name = "applyOnQuit")]
    pub fn apply_on_quit(&self, handle: i64) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::ApplyUpdateOnQuit(handle as u64));
        Ok(())
    }

    /// Queues a [`DeferredAction::EndPowerSaveBlock`] -- same argument as
    /// `quadraui-py`'s `end_power_save_block`. Same `i64`-not-`u64` story
    /// as [`Self::apply_on_quit`].
    #[napi(js_name = "endPowerSaveBlock")]
    pub fn end_power_save_block(&self, handle: i64) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::EndPowerSaveBlock(handle as u64));
        Ok(())
    }

    /// Queues a [`DeferredAction::SetLoginItem`] -- same arguments as
    /// `quadraui-py`'s `set_login_item`.
    #[napi(js_name = "setLoginItem")]
    pub fn set_login_item(
        &self,
        enabled: bool,
        open_hidden: Option<bool>,
        args: Option<Vec<String>>,
    ) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::SetLoginItem(
                enabled,
                open_hidden.unwrap_or(false),
                args.unwrap_or_default(),
            ));
        Ok(())
    }

    /// Queues a [`DeferredAction::SetAsDefaultProtocolClient`] -- same
    /// argument as `quadraui-py`'s `set_as_default_protocol_client`.
    #[napi(js_name = "setAsDefaultProtocolClient")]
    pub fn set_as_default_protocol_client(&self, scheme: String) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::SetAsDefaultProtocolClient(scheme));
        Ok(())
    }

    /// Queues a [`DeferredAction::RemoveAsDefaultProtocolClient`] -- same
    /// argument as `quadraui-py`'s `remove_as_default_protocol_client`.
    #[napi(js_name = "removeAsDefaultProtocolClient")]
    pub fn remove_as_default_protocol_client(&self, scheme: String) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::RemoveAsDefaultProtocolClient(scheme));
        Ok(())
    }

    /// Queues a [`DeferredAction::BeginDragOut`] -- same arguments as
    /// `quadraui-py`'s `begin_drag_out`.
    #[napi(js_name = "beginDragOut")]
    pub fn begin_drag_out(&self, files: Option<Vec<String>>, text: Option<String>) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::BeginDragOut(
                files.unwrap_or_default(),
                text,
            ));
        Ok(())
    }

    /// Queues a [`DeferredAction::AddRecentDocument`] -- same argument as
    /// `quadraui-py`'s `add_recent_document`.
    #[napi(js_name = "addRecentDocument")]
    pub fn add_recent_document(&self, path: String) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::AddRecentDocument(PathBuf::from(path)));
        Ok(())
    }

    /// Queues a [`DeferredAction::ClearRecentDocuments`] -- same argument
    /// (none) as `quadraui-py`'s `clear_recent_documents`.
    #[napi(js_name = "clearRecentDocuments")]
    pub fn clear_recent_documents(&self) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::ClearRecentDocuments);
        Ok(())
    }

    /// Queues a [`DeferredAction::SetDockMenu`] -- same argument, same
    /// decoding, as `quadraui-py`'s `set_dock_menu`. `items` is only
    /// materialized into a [`JsonValue`] here; the actual item-list shape
    /// is decoded by `quadraui_binding_core::set_dock_menu` when
    /// [`flush_deferred_actions`] applies it -- see [`DeferredAction`]'s
    /// own doc for why.
    #[napi(js_name = "setDockMenu")]
    pub fn set_dock_menu(&self, items: Unknown) -> Result<()> {
        let items = to_descriptor_value(items)
            .map_err(|detail| invalid_descriptor_error(format!("setDockMenu: {detail}")))?;
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::SetDockMenu(items));
        Ok(())
    }

    /// Queues a [`DeferredAction::SetJumpList`] -- same argument, same
    /// decoding, as `quadraui-py`'s `set_jump_list`. Same decode-at-flush-
    /// time story as [`Self::set_dock_menu`].
    #[napi(js_name = "setJumpList")]
    pub fn set_jump_list(&self, items: Unknown) -> Result<()> {
        let items = to_descriptor_value(items)
            .map_err(|detail| invalid_descriptor_error(format!("setJumpList: {detail}")))?;
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::SetJumpList(items));
        Ok(())
    }

    /// Queues a [`DeferredAction::TrayIcon`] -- same argument, same
    /// decoding, as `quadraui-py`'s `set_tray_icon`. `icon` decodes via
    /// [`decode_via_json`], not [`decode_direct`]: a `Buffer`/`Uint8Array`
    /// `{Bytes: ...}` source needs the byte-array materialization
    /// [`to_descriptor_value`] does -- see [`QuiApp::set_tray_icon`]'s own
    /// doc.
    #[napi(js_name = "setTrayIcon")]
    pub fn set_tray_icon(&self, icon: Unknown) -> Result<()> {
        let icon: ImageSource = decode_via_json("setTrayIcon", icon)?;
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::TrayIcon(icon));
        Ok(())
    }

    /// Queues a [`DeferredAction::TrayTooltip`] -- same argument as
    /// `quadraui-py`'s `set_tray_tooltip`.
    #[napi(js_name = "setTrayTooltip")]
    pub fn set_tray_tooltip(&self, tooltip: String) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::TrayTooltip(tooltip));
        Ok(())
    }

    /// Queues a [`DeferredAction::TrayMenu`] -- same argument, same
    /// decoding, as `quadraui-py`'s `set_tray_menu`.
    #[napi(js_name = "setTrayMenu")]
    pub fn set_tray_menu(&self, env: Env, items: Unknown) -> Result<()> {
        let items: Vec<ContextMenuItem> = decode_direct(env, "setTrayMenu", items)?;
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::TrayMenu(items));
        Ok(())
    }

    /// Queues a [`DeferredAction::DocumentEdited`] -- same argument as
    /// `quadraui-py`'s `set_document_edited`.
    #[napi(js_name = "setDocumentEdited")]
    pub fn set_document_edited(&self, edited: bool) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::DocumentEdited(edited));
        Ok(())
    }

    /// Queues a [`DeferredAction::RepresentedFilename`] -- same argument as
    /// `quadraui-py`'s `set_represented_filename`.
    #[napi(js_name = "setRepresentedFilename")]
    pub fn set_represented_filename(&self, path: Option<String>) -> Result<()> {
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::RepresentedFilename(path.map(PathBuf::from)));
        Ok(())
    }

    /// Queues a [`DeferredAction::Cursor`] -- same arguments, same
    /// decoding, as `quadraui-py`'s `QuiAppActions.set_cursor`. Unlike
    /// [`QuiApp::set_cursor`], this has no return value: whether the glyph
    /// was actually applied is only observable synchronously, and a
    /// deferred action by definition isn't applied until after this call
    /// returns.
    #[napi(js_name = "setCursor")]
    pub fn set_cursor(&self, shape: String, edge: Option<String>) -> Result<()> {
        let shape: PointerShape = pointer_shape(&shape, edge.as_deref())
            .map_err(|err| invalid_descriptor_error(format!("setCursor: {err}")))?;
        self.queue
            .borrow_mut()
            .push_back(DeferredAction::Cursor(shape));
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
/// itself, which napi-rs pins to the thread that created it. The second
/// field mirrors [`QuiApp::wake_notify`] the same way the first mirrors
/// [`QuiApp::current_waker`] -- see this module's doc's "Wake-driven
/// stepping" section.
#[napi]
pub struct QuiAppWaker(SharedWaker, WakeNotify);

impl QuiAppWaker {
    /// Delivers `value` (already materialized into a `Send`-safe
    /// [`JsonValue`]) to the owning `QuiApp`, wrapped in a
    /// [`UserPayload`] -- the plain-Rust core [`Self::post`]'s JS-facing
    /// decode step calls into, and this crate's own background-thread test
    /// coverage calls directly, with no live napi `Env` needed (see
    /// [`Self::post`]'s doc for why a raw JS value can't cross this
    /// boundary at all). Calls [`Self::notify`] afterwards, same as every
    /// other wake path on this type.
    fn post_value(&self, value: JsonValue) {
        let waker = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        waker(UserPayload::new(value));
        self.notify();
    }

    /// Fires the threadsafe function [`QuiApp::arm_wake_notify`] registered
    /// (if any), with no arguments, in non-blocking mode -- queues the call
    /// without waiting for it to actually run on the JS thread, which is
    /// what lets this be called from this method's own background-thread
    /// callers (`post_after`/`wake_after`'s spawned sleep-then-wake thread)
    /// with no risk of deadlocking against a JS thread that is itself
    /// blocked waiting on something this background thread would otherwise
    /// need to produce first.
    fn notify(&self) {
        if let Some(tsfn) = self.1.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            tsfn.call((), ThreadsafeFunctionCallMode::NonBlocking);
        }
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
        let notify = Arc::clone(&self.1);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(u64::from(delay_ms)));
            let waker = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            waker(UserPayload::new(value));
            if let Some(tsfn) = notify.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                tsfn.call((), ThreadsafeFunctionCallMode::NonBlocking);
            }
        });
        Ok(())
    }

    /// Equivalent to `postAfter(delayMs, null)` -- see [`Self::wake`]'s doc
    /// for why a payload-free wake is useful on its own.
    #[napi(js_name = "wakeAfter")]
    pub fn wake_after(&self, delay_ms: u32) {
        let shared = Arc::clone(&self.0);
        let notify = Arc::clone(&self.1);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(u64::from(delay_ms)));
            let waker = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            waker(UserPayload::new(JsonValue::Null));
            if let Some(tsfn) = notify.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                tsfn.call((), ThreadsafeFunctionCallMode::NonBlocking);
            }
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
    /// The live terminal/window takeover `startTui`/`startNative` moved
    /// this app's whole [`NodeAppLogic`] into, if any -- see this module's
    /// doc's "Live run loop" section. `None` until one of them is called,
    /// and again once that runner's own `step` observes
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
    /// The threadsafe function [`Self::arm_wake_notify`] registers, shared
    /// with every [`QuiAppWaker`] handle [`Self::waker`] has ever returned
    /// -- see [`WakeNotify`]'s own doc.
    wake_notify: WakeNotify,
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
            wake_notify: Arc::new(Mutex::new(None)),
        })
    }

    /// Registers `callback` as a threadsafe, no-argument notifier
    /// [`QuiAppWaker::notify`] fires every time a handle this app's
    /// [`Self::waker`] has ever returned delivers a post/wake/postAfter/
    /// wakeAfter, from whatever thread that happened on -- see this
    /// module's doc's "Wake-driven stepping" section. `js/index.ts`'s `run`
    /// arms this once per live session to call `step()` promptly instead of
    /// polling on a fixed interval.
    ///
    /// Replaces any callback a previous call armed; there is only ever one
    /// slot. Does **not** raise `LiveRunnerActiveError` while a live
    /// session is running (unlike most configuration methods) -- arming
    /// this is exactly what a live session's own run loop needs to do
    /// right after `startTui`/`startNative` returns.
    #[napi(js_name = "armWakeNotify")]
    pub fn arm_wake_notify(&self, callback: Function<'_, (), ()>) -> Result<()> {
        let tsfn = callback.build_threadsafe_function::<()>().build()?;
        *self.wake_notify.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(tsfn));
        Ok(())
    }

    /// Clears whatever callback [`Self::arm_wake_notify`] last registered, if
    /// any -- a harmless no-op if none is armed. Releases the underlying
    /// `napi_threadsafe_function` (once every [`QuiAppWaker`] clone holding
    /// it is also dropped), unblocking the event loop it was keeping alive.
    #[napi(js_name = "disarmWakeNotify")]
    pub fn disarm_wake_notify(&self) {
        *self.wake_notify.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
            .ok_or_else(|| invalid_char_error(format!("invalid Unicode code point: {ch}")))?;
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
            return Err(unknown_slot_error(format!("unknown slot id: {slot}")));
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

    /// Test-oriented override for [`BindingHost::now_ms`] -- the headless
    /// clock [`NodeAppLogic::drive_hover_tooltip`] feeds
    /// [`BindingHost::drive_hover_tooltip`]'s own `now_ms` parameter.
    /// `ms` pins it to that absolute millisecond value until the next call
    /// to this method; omitting it (`undefined`) reverts to the real wall
    /// clock. Lets a test drive the hover-tooltip delay and auto-dismiss
    /// deadlines deterministically -- advancing past
    /// `tooltipDelayMs`/`tooltipAutoDismissMs` by raising `ms` -- instead
    /// of a real `setTimeout` wait.
    #[napi(js_name = "setTestClockMs")]
    pub fn set_test_clock_ms(&mut self, env: Env, ms: Option<u32>) -> Result<()> {
        self.guarded(env, |app| {
            app.driver.app().host.set_test_clock_ms(ms.map(u64::from));
            Ok(())
        })
    }

    /// Reads back `id`'s current resolved widget state from the store --
    /// a `Tree`/`List`/`DataTable`'s cursor + scroll, a `Split`'s ratio, a
    /// `TabBar`'s active tab index, or a `Tree`'s expanded row paths,
    /// whichever apply -- the shape
    /// [`quadraui_binding_core::widget_state_snapshot`] resolves, camelCased
    /// via [`WidgetStateJs`]. `id` names exactly one widget kind in
    /// practice, so the returned object only ever has one group of fields
    /// populated. Returns `null` when `id` has never been resolved through
    /// this app's own `render` (the no-callback fixed-slot path via
    /// [`resolve_render_refs`], or a render-callback's decoded tree via
    /// [`resolve_layout_node`]) -- `id` names a widget that hasn't
    /// rendered, or hasn't rendered yet this session.
    #[napi(js_name = "widgetState")]
    pub fn widget_state(&self, env: Env, id: String) -> Result<Unknown<'static>> {
        let snapshot =
            widget_state_snapshot(self.driver.app().engine.widget_state(), WidgetId::new(id))
                .map(WidgetStateJs::from);
        encode_via_json(env, "widgetState", &snapshot)
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
    /// rather than silently ticking that now-empty orphan -- and, via
    /// [`Self::check_live_not_exited`], on a second `startTui`/`startNative`
    /// call too, rather than opening a second session around that same
    /// now-empty orphan.
    #[napi(js_name = "startTui")]
    pub fn start_tui(&mut self, env: Env, mouse: Option<bool>) -> Result<()> {
        self.check_not_live()?;
        self.check_live_not_exited()?;
        self.guarded(env, |app| {
            let app_logic = std::mem::take(app.driver.app_mut());
            let config = if mouse.unwrap_or(true) {
                RunConfig::default()
            } else {
                RunConfig::no_mouse()
            };
            let runner = TuiRunner::new_with(app_logic, config)
                .map_err(|err| internal_error(format!("startTui: {err}")))?;
            // Read the waker off `runner`'s own backend before it is
            // boxed/wrapped in `LiveRunner::Tui` -- `start_native`'s own
            // `gtk` arm reads `GtkRunner::waker()` the same way, just
            // through that type's own narrower accessor rather than a full
            // `backend()` seam (see `LiveRunner`'s own doc).
            let waker = runner.backend().waker();
            *app.current_waker.lock().unwrap_or_else(|e| e.into_inner()) = waker;
            app.live = Some(LiveRunner::Tui(Box::new(runner)));
            Ok(())
        })
    }

    /// Moves this app's whole [`NodeAppLogic`] into a real native window --
    /// `quadraui::gtk::GtkRunner` on the `gtk` feature,
    /// `quadraui::macos::MacRunner` on the `macos` feature (only when also
    /// built for a macOS `target_os`), or `quadraui::win::WinRunner` on the
    /// `win` feature (only when also built for a Windows `target_os`) --
    /// `gtk` wins when more than one native feature is enabled, then
    /// `macos`, then `win`, mirroring `quadraui-py`'s own `run_native`
    /// priority. Non-blocking like [`Self::start_tui`]: this call only
    /// builds the window and returns immediately, wrapping the moved
    /// [`NodeAppLogic`] in [`NativeExitPolicy`] so a handler-less
    /// `WindowClose` still exits the window (mirroring `quadraui-py`'s own
    /// `NativeAppLogic`) -- see this module's doc's "Live run loop"
    /// section for the overall discipline.
    ///
    /// `title`/`app_id`/`icon_name`/`single_instance` mirror
    /// `quadraui-py`'s own `run_native` arguments; which ones apply
    /// depends on which native feature actually built this binary (`gtk`
    /// uses all four, `macos`/`win` ignore `app_id` -- there is no
    /// `GApplication`-style id on either platform -- and `single_instance`,
    /// since neither `MacRunner::new_with` nor this binding's own
    /// `WinRunner::new_with` call wires it).
    ///
    /// On the `macos` feature without `gtk` also enabled,
    /// [`NativeExitPolicy`] downgrades any other `Reaction::Exit` to a
    /// sticky `exit_requested` flag instead of letting it reach AppKit's
    /// `[NSApp terminate:]` and end the whole process before this session
    /// could; [`LiveRunner::step`] folds that flag into the
    /// [`StepOutcome`] it returns, so [`Self::step_once`]'s own
    /// [`StepOutcome::Exited`] handling already covers it.
    ///
    /// On the `gtk` feature, this swaps [`Self::current_waker`] exactly
    /// like [`Self::start_tui`] does, onto `GtkRunner::waker()` -- the one
    /// closure that runner captures off its own live `GtkBackend` before
    /// `activate` moves the rest of it into that window's event-controller
    /// closures. On the `macos`/`win` features it swaps too, just through
    /// a different seam: neither `MacRunner` nor `WinRunner` exposes a
    /// comparable post-construction accessor once the moved `AppLogic` is
    /// behind it, so [`NodeAppLogic::setup`] stashes `backend`'s own
    /// [`Backend::waker`] into [`NodeAppLogic::captured_waker`] while it
    /// still can (every runner calls `setup` synchronously, before
    /// construction returns), and this arm reads it back right after
    /// `MacRunner::new_with`/`WinRunner::new_with` returns.
    ///
    /// On the `win` feature, [`LiveRunner::step`] drives `WinRunner::step()`
    /// exactly once per call and never its blocking `pump()` -- the same
    /// non-blocking discipline this module's doc's "Live run loop" section
    /// describes for every variant. But that single call is not immune to
    /// Win32's own nested message loop: dragging this window's standard
    /// (non-CSD; see `quadraui::win::run`'s module doc) title bar or resize
    /// border sends `WM_NCLBUTTONDOWN`, which `wndproc` does not intercept,
    /// so it falls through to `DefWindowProcW` -- and `DefWindowProcW`'s own
    /// handling of that message is a synchronous modal loop that pumps
    /// `WM_SIZE`/`WM_PAINT` reentrantly (repainting the window live as the
    /// user drags) but does not return control to `WinRunner::step()`, and
    /// therefore not to the JS caller that invoked it, until the user
    /// releases the mouse button. A concurrent `setTimeout`/`setInterval`
    /// cannot fire for the whole duration of that one drag, since the one
    /// libuv reactor this process shares stays blocked inside that single
    /// `step()` call the entire time -- an OS-level property of the
    /// standard window frame, not something a future change to this
    /// binding's own pump cadence could route around without replacing
    /// that frame with a custom one `quadraui::win` paints itself.
    ///
    /// Raises `LiveRunnerActiveError` if a live session is already
    /// running. Raises `NoNativeBackendError` if this binary was built
    /// with none of the `gtk`/`macos`/`win` features. On the `gtk` feature,
    /// raises `NoDisplayError` if no display is reachable (no
    /// `DISPLAY`/`WAYLAND_DISPLAY`) or `InvalidAppIdError` if `appId` is
    /// given and is not a valid `GApplication` id -- both checked before
    /// `GtkRunner::new_with` is ever called, since that call has no way to
    /// fail gracefully on either condition (see [`no_display_error`]'s own
    /// doc). Raises `InternalError` if the `macos`/`win` feature is enabled
    /// but this binary was not built for that feature's own native
    /// `target_os` (a `cargo build`/`check` run that only type-checks the
    /// forwarded dependency graph -- see `Cargo.toml`'s own comment) --
    /// there is no real runner to hand off to in that case regardless of
    /// what this call passes -- or if the real takeover itself fails (not
    /// on the main thread on `macos`, a `WinRunner::new_with` failure on
    /// `win`). This app's prior configuration (handlers, inserted
    /// descriptors) is lost on any failure past the `gtk` pre-flight checks
    /// the same way [`Self::start_tui`]'s own doc describes, since
    /// `std::mem::take` already moved it out before the takeover attempt
    /// where one was actually made. Also raises `LiveRunnerExitedError`,
    /// via [`Self::check_live_not_exited`], on a second `startTui`/
    /// `startNative` call after an earlier session on this app has already
    /// ended -- see [`Self::start_tui`]'s own doc for why.
    #[napi(js_name = "startNative")]
    pub fn start_native(
        &mut self,
        env: Env,
        title: Option<String>,
        app_id: Option<String>,
        icon_name: Option<String>,
        single_instance: Option<bool>,
    ) -> Result<()> {
        self.check_not_live()?;
        self.check_live_not_exited()?;
        self.guarded(env, |app| {
            #[cfg(feature = "gtk")]
            {
                // Both checked before `mem::take` moves this app's
                // `NodeAppLogic` out: either failure must leave this app's
                // handlers/descriptors exactly as configured, since there
                // is no real takeover attempt to blame the loss on --
                // mirrors `quadraui-py`'s own `run_native` ordering.
                if !quadraui::gtk::display_available() {
                    return Err(no_display_error(
                        "startNative found no display to open a window on (no \
                         DISPLAY/WAYLAND_DISPLAY)",
                    ));
                }
                if let Some(app_id) = &app_id {
                    if !quadraui::gtk::app_id_is_valid(app_id) {
                        return Err(invalid_app_id_error(format!(
                            "startNative's appId {app_id:?} is not a valid GApplication id \
                             (see https://developer.gnome.org/documentation/tutorials/application-id.html)"
                        )));
                    }
                }
                let app_logic = std::mem::take(app.driver.app_mut());
                let state = NativeLiveState {
                    env: app_logic.env.clone(),
                    pending_error: app_logic.pending_error.clone(),
                };
                let defaults = quadraui::gtk::RunConfig::default();
                let mut config = quadraui::gtk::RunConfig::new(
                    app_id.unwrap_or(defaults.app_id),
                    title.unwrap_or(defaults.title),
                );
                if let Some(icon_name) = icon_name {
                    config = config.with_icon_name(icon_name);
                }
                config = config.with_single_instance(single_instance.unwrap_or(false));
                let runner = quadraui::gtk::GtkRunner::new_with(
                    NativeExitPolicy { inner: app_logic },
                    config,
                );
                // Unlike `Mac`/`Win`, `GtkRunner` does expose a
                // `Backend::waker`-reachable accessor (`GtkRunner::waker`),
                // captured off its own live window before `activate` moved
                // the real `GtkBackend` into that window's event-controller
                // closures -- see that method's own doc. Swapping it in
                // here mirrors `start_tui`'s own swap, so a `waker()`
                // handle obtained before, during, or after this call
                // delivers into this session's own event stream rather
                // than the dormant headless queue -- restored by
                // `stop_live`/`step_once` once this session ends, exactly
                // like `start_tui`'s own swap is.
                let waker = runner.waker();
                *app.current_waker.lock().unwrap_or_else(|e| e.into_inner()) = waker;
                app.live = Some(LiveRunner::Gtk(runner, state));
                Ok(())
            }

            #[cfg(all(feature = "macos", target_os = "macos", not(feature = "gtk")))]
            {
                // `app_id`/`icon_name`/`single_instance` have no macOS
                // analog; accepted for signature parity with the `gtk`
                // build and silently ignored here, mirroring
                // `quadraui-py`'s own `run_native`.
                let _ = (&app_id, &icon_name, single_instance);
                if objc2::MainThreadMarker::new().is_none() {
                    return Err(internal_error(
                        "startNative must be called from the main thread to open a window on \
                         macOS",
                    ));
                }
                let app_logic = std::mem::take(app.driver.app_mut());
                let state = NativeLiveState {
                    env: app_logic.env.clone(),
                    pending_error: app_logic.pending_error.clone(),
                };
                // Cloned before `MacRunner::new_with` moves `app_logic`
                // itself -- the same `Rc` `app_logic.setup()` writes into,
                // synchronously, inside that call (see
                // `NodeAppLogic::captured_waker`'s own doc). `take()` right
                // after reads back the one `Backend::waker` this live
                // session's `MacBackend` will ever hand out, since nothing
                // outside `MacRunner` can reach it again once this call
                // returns.
                let captured_waker = app_logic.captured_waker.clone();
                let exit_requested = Rc::new(Cell::new(false));
                let config = quadraui::macos::RunConfig::new(
                    title.unwrap_or(quadraui::macos::RunConfig::default().title),
                );
                let runner = quadraui::macos::MacRunner::new_with(
                    NativeExitPolicy {
                        inner: app_logic,
                        exit_requested: exit_requested.clone(),
                    },
                    config,
                );
                // Unlike `Win`, whose own runner exposes no comparable
                // hook either but is out of this issue's scope, swapping
                // this in mirrors `start_tui`'s/the `gtk` arm's own swap:
                // a `waker()` handle obtained before, during, or after
                // this call now delivers into this session's own event
                // stream rather than the dormant headless queue --
                // restored by `stop_live`/`step_once` once this session
                // ends, exactly like those swaps are.
                if let Some(waker) = captured_waker.take() {
                    *app.current_waker.lock().unwrap_or_else(|e| e.into_inner()) = waker;
                }
                app.live = Some(LiveRunner::Mac(runner, state, exit_requested));
                Ok(())
            }

            // Reachable only when the `macos` feature is enabled on a
            // non-Apple `target_os` -- e.g. a `cargo check -p quadraui-node
            // --features macos` typecheck run on Linux or Windows.
            // `quadraui::macos` is itself gated on `target_os = "macos"`,
            // so there is no runner to hand off to here; this arm exists
            // only to keep `startNative` buildable on other hosts, never
            // to run for real.
            #[cfg(all(feature = "macos", not(feature = "gtk"), not(target_os = "macos")))]
            {
                let _ = (&app, &title, &app_id, &icon_name, single_instance);
                Err(internal_error(
                    "startNative with the macos feature requires building for a macOS \
                     target_os; this binary was built for a different platform",
                ))
            }

            #[cfg(all(
                feature = "win",
                target_os = "windows",
                not(feature = "gtk"),
                not(feature = "macos")
            ))]
            {
                // `app_id`/`icon_name` have no Windows analog;
                // `single_instance` has no effect on `WinRunner::new_with`
                // (only `win::run::run` wires it) -- all three accepted
                // for signature parity and silently ignored, mirroring
                // `quadraui-py`'s own `run_native`.
                let _ = (&app_id, &icon_name, single_instance);
                let app_logic = std::mem::take(app.driver.app_mut());
                let state = NativeLiveState {
                    env: app_logic.env.clone(),
                    pending_error: app_logic.pending_error.clone(),
                };
                // Cloned before `WinRunner::new_with` moves `app_logic`
                // itself -- the same `Rc` `app_logic.setup()` writes into,
                // synchronously, inside that call (see
                // `NodeAppLogic::captured_waker`'s own doc). `take()` right
                // after reads back the one `Backend::waker` this live
                // session's `WinBackend` will ever hand out, since nothing
                // outside `WinRunner` can reach it again once this call
                // returns.
                let captured_waker = app_logic.captured_waker.clone();
                let defaults = quadraui::win::RunConfig::default();
                let config = quadraui::win::RunConfig::new(title.unwrap_or(defaults.title));
                // SAFETY: mirrors `quadraui-py`'s own `run_native`:
                // constructed and stepped on the same OS thread for its
                // whole lifetime, with no other window on that thread --
                // exactly what this `guarded` call does.
                let runner = unsafe {
                    quadraui::win::WinRunner::new_with(
                        NativeExitPolicy { inner: app_logic },
                        &config,
                    )
                }
                .map_err(|err| internal_error(format!("startNative: {err}")))?;
                // Mirrors the `macos` arm's own swap: a `waker()` handle
                // obtained before, during, or after this call now delivers
                // into this session's own event stream rather than the
                // dormant headless queue -- restored by `stop_live`/
                // `step_once` once this session ends, exactly like those
                // swaps are.
                if let Some(waker) = captured_waker.take() {
                    *app.current_waker.lock().unwrap_or_else(|e| e.into_inner()) = waker;
                }
                app.live = Some(LiveRunner::Win(runner, state));
                Ok(())
            }

            // Reachable only when the `win` feature is enabled on a
            // non-Windows `target_os`. `quadraui::win::WinRunner` is
            // itself gated on `target_os = "windows"`, so there is no
            // runner to hand off to here -- see the `macos` arm above's
            // own doc for why this exists.
            #[cfg(all(
                feature = "win",
                not(feature = "gtk"),
                not(feature = "macos"),
                not(target_os = "windows")
            ))]
            {
                let _ = (&app, &title, &app_id, &icon_name, single_instance);
                Err(internal_error(
                    "startNative with the win feature requires building for a Windows \
                     target_os; this binary was built for a different platform",
                ))
            }

            #[cfg(not(any(feature = "gtk", feature = "macos", feature = "win")))]
            {
                let _ = (&app, &title, &app_id, &icon_name, single_instance);
                Err(no_native_backend_error(
                    "startNative requires this binary to be built with the gtk, macos, or win \
                     feature -- none is enabled",
                ))
            }
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
    ///
    /// On a live [`LiveRunner::Gtk`] session, calls `request_exit()` and
    /// one more `step()` before dropping the runner -- `GtkRunner` has no
    /// `Drop` teardown of its own (see that type's own doc), so without
    /// this the window would stay open, frozen and unresponsive, until the
    /// whole Node process exits, rather than actually closing. `TuiRunner`
    /// restores the terminal on `Drop` already; `MacRunner`/`WinRunner`
    /// have no comparable `request_exit()` to call here (the same
    /// asymmetry `quadraui-py`'s own native loops document).
    #[napi(js_name = "stopLive")]
    pub fn stop_live(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| {
            let live = app.live.take();
            let was_live = live.is_some();
            if let Some(mut live) = live {
                request_exit_before_drop(&mut live);
                drop(live);
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
    /// while a live session is running. During a `startTui` session, or a
    /// `startNative` session on the `gtk` feature, [`Self::current_waker`]
    /// has been swapped to the live runner's own backend waker, so a
    /// handle obtained then still posts correctly -- during a
    /// `startNative` session on `macos`/`win` it has not (see
    /// [`QuiApp::start_native`]'s own doc for why), so a handle obtained
    /// then posts into the dormant headless queue instead, never actually
    /// delivered.
    #[napi]
    pub fn waker(&self) -> Result<QuiAppWaker> {
        if self.poisoned {
            return Err(poisoned_error(
                "this QuiApp is poisoned by an earlier caught panic and can no longer be used",
            ));
        }
        Ok(QuiAppWaker(
            Arc::clone(&self.current_waker),
            Arc::clone(&self.wake_notify),
        ))
    }

    /// Which native *feature* (if any) this build was compiled with -- `gtk`
    /// wins over `macos`, and both win over `win`, when more than one is
    /// enabled, mirroring [`Self::start_native`]'s own priority order. A
    /// build configuration report, not a promise that a window can
    /// actually open: `"macos"`/`"win"` is reported for any `target_os`,
    /// including a Linux/Windows typecheck build where `startNative` can
    /// only raise `InternalError` for the platform mismatch. Lets a test
    /// tell builds apart without the backend-specific side effects
    /// `startNative` itself would have (opening a real window, a
    /// `GApplication` id check that only applies to GTK, ...) -- the
    /// napi-rs analog of `quadraui-py`'s own `_NATIVE_BACKEND` module
    /// attribute. Not part of the stable JS API.
    #[napi(js_name = "nativeBackend")]
    pub fn native_backend() -> Option<String> {
        #[cfg(feature = "gtk")]
        return Some("gtk".to_string());
        #[cfg(all(feature = "macos", not(feature = "gtk")))]
        return Some("macos".to_string());
        #[cfg(all(feature = "win", not(feature = "gtk"), not(feature = "macos")))]
        return Some("win".to_string());
        #[cfg(not(any(feature = "gtk", feature = "macos", feature = "win")))]
        return None;
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

    /// Raises `LiveRunnerActiveError` if `startTui`/`startNative` is
    /// currently running on this app -- see this module's doc's "Live run
    /// loop" section. Every method on `QuiApp`
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

    /// Raises `LiveRunnerExitedError` if a `startTui`/`startNative` session
    /// has already run to completion on this app -- [`Self::start_tui`]/
    /// [`Self::start_native`]'s own extra guard, called right after
    /// [`Self::check_not_live`]. Without this, a second call after
    /// [`Self::live_exited`] is set would pass `check_not_live` (`self.live`
    /// is `None` again by then) and `std::mem::take` an empty, `Default`-d
    /// [`NodeAppLogic`] into a brand new window/terminal session that can
    /// never be stepped -- `step()`/`run()` already refuse to drive it
    /// (see [`Self::live_exited`]'s own doc), so that window could only
    /// ever sit there inert until [`Self::stop_live`] closes it. Mirrors
    /// `quadraui-py`'s own `native_consumed` → `NativeAlreadyRunError`
    /// guard on `run_native`, reusing `LiveRunnerExitedError` rather than a
    /// new tag: both describe the same fact -- this app's one live session
    /// has already ended -- from the two sides (`step`/`run` driving it
    /// further vs. starting a second one) of the same sticky flag.
    fn check_live_not_exited(&self) -> Result<()> {
        if self.live_exited.get() {
            return Err(live_runner_exited_error(
                "startTui/startNative already ran once on this QuiApp; its handlers and \
                 descriptors were moved into the session that call opened, so there is \
                 nothing left for a second call to show",
            ));
        }
        Ok(())
    }

    /// Writes `env` into whichever cell is currently driving this app --
    /// [`LiveRunner::set_env`] if a live session is running, otherwise
    /// [`Self::driver`]'s headless [`NodeAppLogic::env`] directly -- so a
    /// registered handler/tick/render callback reached through either
    /// [`Self::step_once`] or a direct [`Self::driver`] call (gated by
    /// [`Self::check_not_live`]) has a live `Env` to call back into JS
    /// with. See [`NodeAppLogic::env`]'s doc.
    fn set_active_env(&mut self, env: Env) {
        match self.live.as_mut() {
            Some(live) => live.set_env(env),
            None => self.driver.app_mut().env.set(Some(env)),
        }
    }

    /// Points [`Self::current_waker`] back at [`Self::driver`]'s own
    /// headless backend's waker -- called once a live session's own `step`
    /// observes [`StepOutcome::Exited`] (or [`Self::stop_live`] tears one
    /// down early), undoing [`Self::start_tui`]'s swap to that session's
    /// own backend's waker, or [`Self::start_native`]'s `gtk` arm's
    /// equivalent swap onto `GtkRunner::waker()`. A harmless no-op on
    /// `macos`/`win`, which never swapped [`Self::current_waker`] away
    /// from this in the first place.
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
    /// unnecessary there. `env` is stashed via [`Self::set_active_env`]
    /// before `body` runs, so a registered handler/tick/render callback
    /// `body` reaches (via `step_once`/`driver.render()`) has a live `Env`
    /// to call back into JS with -- see [`NodeAppLogic::env`]'s doc.
    fn guarded<T>(&mut self, env: Env, body: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.check_not_dispatching()?;
        core_guarded(
            self,
            |s| s.poisoned,
            |s| s.poisoned = true,
            |s| {
                s.set_active_env(env);
                s.in_dispatch.set(true);
            },
            |s| s.in_dispatch.set(false),
            |failure| match failure {
                GuardedFailure::Poisoned => poisoned_error(
                    "this QuiApp is poisoned by an earlier caught panic and can no longer be used",
                ),
                GuardedFailure::Panic(msg) => panic_error(msg),
            },
            body,
        )
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
        // below -- once dropped, there is no more `NodeAppLogic`/
        // `NativeLiveState` left to read it back off of.
        let pending = live.take_error();
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
    /// Invoked with one JS-converted `AppShellEvent` payload (via
    /// `Env::to_js_value`, using `AppShellEvent`'s own `Serialize` impl)
    /// whenever [`ShellApp::on_shell_event_ctx`] fires -- a panel switch,
    /// the sidebar being hidden/resized, the bottom panel being
    /// resized/hidden, or a bottom-item (no-panel) activity-bar click.
    on_shell_event: Option<NodeCallback>,
    /// Invoked with one JS-converted `BottomPanelEvent` payload (via
    /// `Env::to_js_value`, using `BottomPanelEvent`'s own `Serialize` impl)
    /// whenever [`ShellApp::on_bottom_panel_event`] fires. Only ever fires
    /// when this app's `config` included a `"bottom_panel"` entry.
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
    /// earlier error already claimed the slot -- [`record_pending_error`]'s
    /// "first error wins" contract, mirroring the free [`record_error`]
    /// function's own use of it. A method here (not a free function, like
    /// [`record_error`]) since `ShellState` holds its `pending_error` as a
    /// plain field rather than its own `Rc<RefCell<_>>`.
    fn record_error(&mut self, err: Error) {
        record_pending_error(&mut self.pending_error, err);
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
        let payload: Unknown<'static> = match env.to_js_value(event) {
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
        let payload: Unknown<'static> = match env.to_js_value(event) {
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
    core_take_pending_error(&mut state.borrow_mut().pending_error)
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
    /// `AppShellEvent` payload whenever a panel switch, sidebar show/hide/
    /// resize, or bottom-panel resize/hide occurs. Raises if called
    /// re-entrantly.
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
    /// `BottomPanelEvent` payload whenever the bottom-panel tab strip is
    /// interacted with. Only ever fires when this app's `config` included
    /// a `"bottom_panel"` entry. Raises if called re-entrantly.
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
            .ok_or_else(|| invalid_char_error(format!("invalid Unicode code point: {ch}")))?;
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
        core_guarded(
            self,
            |s| s.poisoned,
            |s| s.poisoned = true,
            |s| {
                s.state.borrow_mut().env = Some(env);
                s.in_dispatch.set(true);
            },
            |s| s.in_dispatch.set(false),
            |failure| {
                match failure {
                GuardedFailure::Poisoned => poisoned_error(
                    "this QuiShellApp is poisoned by an earlier caught panic and can no longer be used",
                ),
                GuardedFailure::Panic(msg) => panic_error(msg),
            }
            },
            body,
        )
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

    // A `startTui`/`startNative` round trip (success or a failure path) is
    // deliberately not exercised as an in-process `#[test]` here:
    // `TuiRunner::new_with`'s very first step is `enable_raw_mode()`,
    // which -- if this test process happens to have a controlling
    // terminal at all, which a plain `cargo test` invocation sometimes
    // does -- would actually raw-mode it instead of failing fast, hanging
    // or corrupting whatever terminal ran the test suite; a real native
    // session needs an actual display/main-thread/window besides. `quadraui-py`'s
    // own `test_run_tui.py` sidesteps the terminal case by shelling out to a
    // detached (`setsid`) subprocess per test; `quadraui-node/tests/`'s own
    // black-box coverage does the same with `child_process.spawn`. The two
    // tests below cover the shared, backend-neutral state machine itself
    // (`check_not_live`/`live_exited`/a pending error surfacing from
    // `step()`) through the `#[cfg(test)]`-only `LiveRunner::Test` variant
    // instead, which needs none of that.

    /// `check_not_live`/`live_exited` apply the same way regardless of
    /// which concrete runner [`QuiApp::live`] wraps -- exercised here via
    /// the `#[cfg(test)]`-only [`LiveRunner::Test`] variant standing in for
    /// a real `Gtk`/`Mac`/`Win` session, so `startNative`'s own share of
    /// this state machine is covered without a display. Mirrors
    /// `startTui`'s already-covered contract: every configuration method
    /// raises `LiveRunnerActiveError` while live, a live session's own
    /// `StepOutcome::Exited` drops it, sets `live_exited`, and restores
    /// the headless waker, and a further `step()`/`run()` afterwards
    /// raises `LiveRunnerExitedError` rather than silently ticking the
    /// now-empty orphan.
    #[test]
    fn check_not_live_and_live_exited_behave_the_same_for_a_non_tui_live_runner() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        app.live = Some(LiveRunner::Test(TestLiveRunner {
            state: NativeLiveState {
                env: Rc::new(Cell::new(None)),
                pending_error: Rc::new(RefCell::new(None)),
            },
            outcome: Cell::new(StepOutcome::Continue),
        }));

        assert!(
            app.check_not_live().is_err(),
            "a live Test runner must reject configuration the same way a live Tui runner does"
        );
        assert!(
            app.step(null_env_for_test())
                .expect("StepOutcome::Continue must not itself be an error"),
            "StepOutcome::Continue must report the app as still running"
        );
        assert!(
            app.live.is_some(),
            "a Continue outcome must not tear down the live session"
        );

        let LiveRunner::Test(test) = app.live.as_ref().expect("still live") else {
            panic!("expected the Test variant");
        };
        assert!(
            test.state.env.get().is_some(),
            "guarded's set_active_env must reach LiveRunner::set_env's NativeLiveState side \
             channel for a non-Tui variant, the one genuinely new mechanism `startNative` adds \
             (the `Rc<Cell<Option<Env>>>` rewrap -- see NativeLiveState's own doc)"
        );
        test.outcome.set(StepOutcome::Exited);

        assert!(
            !app.step(null_env_for_test())
                .expect("StepOutcome::Exited must not itself be an error"),
            "StepOutcome::Exited must report the app as no longer running"
        );
        assert!(
            app.live.is_none(),
            "StepOutcome::Exited must drop the live session"
        );
        assert!(
            app.live_exited.get(),
            "StepOutcome::Exited must set live_exited"
        );
        assert!(
            app.check_not_live().is_ok(),
            "once dropped, check_not_live must stop rejecting configuration"
        );
        assert!(
            app.step(null_env_for_test()).is_err(),
            "a further step() after live_exited must raise LiveRunnerExitedError rather than \
             silently ticking the now-empty orphan"
        );
        assert!(
            app.check_live_not_exited().is_err(),
            "check_not_live passing again once a session has ended must not also mean a \
             second startTui/startNative call is allowed -- the two checks are independent"
        );
    }

    /// [`QuiApp::check_live_not_exited`] is the extra guard
    /// [`QuiApp::start_tui`]/[`QuiApp::start_native`] run right after
    /// [`QuiApp::check_not_live`], refusing a second call once this app's
    /// one live session has already ended -- without it, `check_not_live`
    /// alone would let a second call through (`self.live` is back to
    /// `None` by then) to `mem::take` an empty, `Default`-d
    /// [`NodeAppLogic`] into a new session that `step()`/`run()` can never
    /// drive, since [`Self::live_exited`] already refuses those.
    #[test]
    fn check_live_not_exited_refuses_a_second_start_after_a_session_has_ended() {
        let app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(
            app.check_live_not_exited().is_ok(),
            "a fresh app with no session having run yet must not be refused"
        );

        app.live_exited.set(true);
        assert!(
            app.check_live_not_exited().is_err(),
            "once a session has ended, a second startTui/startNative call must be refused"
        );
    }

    /// A pending error recorded through [`NativeLiveState::pending_error`]
    /// while a native-shaped [`LiveRunner`] variant was stepping surfaces
    /// out of `step()` the same way [`NodeAppLogic::pending_error`] does
    /// for [`LiveRunner::Tui`] -- [`LiveRunner::take_error`]'s own
    /// contract, exercised here via [`LiveRunner::Test`] since no real
    /// native variant is safe to drive in an in-process `#[test]`.
    #[test]
    fn a_pending_error_recorded_through_native_live_state_surfaces_from_step() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        let pending_error = Rc::new(RefCell::new(None));
        app.live = Some(LiveRunner::Test(TestLiveRunner {
            state: NativeLiveState {
                env: Rc::new(Cell::new(None)),
                pending_error: pending_error.clone(),
            },
            outcome: Cell::new(StepOutcome::Continue),
        }));
        record_error(&pending_error, internal_error("boom"));

        assert!(
            app.step(null_env_for_test()).is_err(),
            "a pending error recorded through the side channel must surface out of step()"
        );
        assert!(
            app.live.is_some(),
            "StepOutcome::Continue plus a pending error must not itself tear down the \
             session, mirroring the Tui arm's own ordering"
        );
    }

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
        let LayoutNode::Row(container) = node else {
            panic!("expected a row");
        };
        assert_eq!(container.children.len(), 2);
    }

    /// A `"row"`'s `"padding"`/`"gap"` spacing keys are siblings of the
    /// `"row"` key in the same node dict, decoded into
    /// [`quadraui_binding_core::layout::Spacing`] alongside the children —
    /// see [`decode_layout_node_round_trips_a_row_of_leaves`]'s doc for why
    /// this needs no live napi `Env`.
    #[test]
    fn decode_layout_node_round_trips_row_spacing_keys() {
        let value = serde_json::json!({
            "row": [
                {"id": "left", "text_display": {"id": "text:left", "lines": []}},
            ],
            "padding": 12,
            "gap": 8,
            "margin": 4,
        });
        let node = decode_layout_node("test", &value).expect("valid layout tree");
        let LayoutNode::Row(container) = node else {
            panic!("expected a row");
        };
        assert_eq!(container.spacing.padding.top, 12.0);
        assert_eq!(container.spacing.padding.left, 12.0);
        assert_eq!(container.spacing.gap, 8.0);
        assert_eq!(container.spacing.margin, 4.0);
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

    /// [`AppShellEvent`]'s own `Serialize` impl gives every variant the
    /// same externally-tagged shape `quadraui-py`'s identical impl does --
    /// a struct-like variant's payload nested under its tag, a unit
    /// variant tagged with `null`. Pure JSON construction, so this needs
    /// no live napi `Env` and is covered here rather than in
    /// `quadraui-node/tests/`; `quadraui`'s own
    /// `app_shell_event_serializes_to_the_externally_tagged_dict_shape`
    /// pins the same shape at the source.
    #[test]
    fn app_shell_event_to_json_tags_every_variant() {
        assert_eq!(
            serde_json::to_value(AppShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:files")
            })
            .unwrap(),
            serde_json::json!({"PanelChanged": {"panel_id": "panel:files"}})
        );
        assert_eq!(
            serde_json::to_value(AppShellEvent::SidebarHidden).unwrap(),
            serde_json::json!({"SidebarHidden": null})
        );
        assert_eq!(
            serde_json::to_value(AppShellEvent::SidebarResized { new_width: 12.5 }).unwrap(),
            serde_json::json!({"SidebarResized": {"new_width": 12.5}})
        );
        assert_eq!(
            serde_json::to_value(AppShellEvent::BottomItemClicked {
                id: WidgetId::new("bottom:terminal")
            })
            .unwrap(),
            serde_json::json!({"BottomItemClicked": {"id": "bottom:terminal"}})
        );
        assert_eq!(
            serde_json::to_value(AppShellEvent::Consumed).unwrap(),
            serde_json::json!({"Consumed": null})
        );
    }

    /// [`BottomPanelEvent`]'s own `Serialize` impl tags a payload-carrying
    /// variant with its bare payload (no nested object), and a unit
    /// variant with `null` -- mirrors `quadraui-py`'s identical impl;
    /// `quadraui`'s own
    /// `bottom_panel_event_serializes_to_the_externally_tagged_dict_shape`
    /// pins the same shape at the source.
    #[test]
    fn bottom_panel_event_to_json_tags_every_variant() {
        assert_eq!(
            serde_json::to_value(BottomPanelEvent::TabActivated("bp:a".to_string())).unwrap(),
            serde_json::json!({"TabActivated": "bp:a"})
        );
        assert_eq!(
            serde_json::to_value(BottomPanelEvent::TabClosed("bp:b".to_string())).unwrap(),
            serde_json::json!({"TabClosed": "bp:b"})
        );
        assert_eq!(
            serde_json::to_value(BottomPanelEvent::MaximiseToggled).unwrap(),
            serde_json::json!({"MaximiseToggled": null})
        );
        assert_eq!(
            serde_json::to_value(BottomPanelEvent::Resized(8.0)).unwrap(),
            serde_json::json!({"Resized": 8.0})
        );
    }
}
