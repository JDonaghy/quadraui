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
//! `insertImage`/`clearExtra`.
//!
//! **Callback bridge.** [`NodeEventSink`] is the napi-rs analog of
//! `quadraui-py`'s `PyEventSink`: it converts a dispatched [`UiEvent`] to a
//! JS value via `Env::to_js_value` (replacing `pythonize::pythonize`),
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
//!   a derived `Deserialize` type -- the six fixed primitive slots and
//!   `pushEvent`'s `UiEvent`, neither of which carries a byte-string or
//!   float field at risk of the two pre-checks below.
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

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::Rc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Button, ContextMenuItem, DataTable, DialogButton, ListView, Notification,
    Point, Reaction, Rect, TabBar, TextDisplay, Toast, ToastCorner, TreeView, UiEvent, WidgetId,
};
#[cfg(test)]
use quadraui_binding_core::callback_reaction;
use quadraui_binding_core::{
    callback_reaction_for_render, decode_extra_primitive, decode_render_output, dialog_severity,
    draw_bands, key_pressed_event, message_dialog_options_from_buttons, paint_layout,
    panic_message, render_overlays, step_driver, toast_corner, window_close_reaction, BindingHost,
    DecodeError, EventSink, HitDetail, InteractionEngine, RenderOutput, WindowCloseHandling,
    SLOT_BUTTON, SLOT_DATA_TABLE, SLOT_LIST, SLOT_TAB_BAR, SLOT_TEXT_DISPLAY, SLOT_TREE,
};
#[cfg(test)]
use quadraui_binding_core::{decode_layout_node, LayoutNode};

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
/// converts `event` to a JS value via `Env::to_js_value` (napi-rs's analog
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
        let payload: Unknown<'static> = match self.env.to_js_value(event) {
            Ok(value) => value,
            Err(err) => {
                record_error(
                    self.pending_error,
                    invalid_descriptor_error(format!("failed to convert event to JS: {err}")),
                );
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
            let hit_value: Unknown<'static> = env.to_js_value(detail).map_err(|err| {
                invalid_descriptor_error(format!("failed to convert hit detail to JS: {err}"))
            })?;
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

/// Mirrors [`invalid_descriptor_error`] for a `QuiApp` method called
/// re-entrantly from inside a registered event handler/tick/render
/// callback's own JS call stack -- see [`QuiApp::check_not_dispatching`]'s
/// doc for why napi-rs needs this guard where PyO3 does not.
fn reentrant_call_error(detail: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("ReentrantCallError: {detail}"))
}

/// Decodes `descriptor` straight into `T` via napi's own
/// `Env::from_js_value` -- the napi-rs analog of `quadraui-py`'s
/// `pythonize::depythonize`, for any descriptor type that derives
/// `Deserialize` with no byte-string/non-finite-float field at risk (the
/// six fixed primitive slots, `pushEvent`'s `UiEvent`).
fn decode_direct<T: DeserializeOwned>(env: Env, what: &str, descriptor: Unknown) -> Result<T> {
    env.from_js_value(descriptor)
        .map_err(|err| invalid_descriptor_error(format!("{what}: {err}")))
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
}

#[napi]
impl QuiApp {
    /// Creates a new headless app on a `width`x`height` cell grid. Raises if
    /// either is zero.
    #[napi(constructor)]
    pub fn new(width: u16, height: u16) -> Result<Self> {
        let (width, height) = checked_dimensions(width, height)?;
        let driver = TuiDriver::new(NodeAppLogic::default(), width, height);
        Ok(QuiApp {
            driver,
            pending_events: VecDeque::new(),
            poisoned: false,
            in_dispatch: Cell::new(false),
        })
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

    /// Queues a synthetic `KeyPressed` event for the character with Unicode
    /// code point `ch`, no modifiers held, not a repeat. Raises if `ch`
    /// isn't a valid Unicode scalar value.
    #[napi(js_name = "pushKeyEvent")]
    pub fn push_key_event(&mut self, ch: u32) -> Result<()> {
        self.check_not_dispatching()?;
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
    /// dump, one row per line.
    #[napi]
    pub fn screen(&mut self, env: Env) -> Result<String> {
        self.guarded(env, |app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(app.driver.screen())
        })
    }

    /// Whether `needle` appears anywhere in the current rendered frame.
    #[napi(js_name = "screenContains")]
    pub fn screen_contains(&mut self, env: Env, needle: String) -> Result<bool> {
        self.guarded(env, |app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(app.driver.screen_contains(&needle))
        })
    }

    /// Empties a slot (one of the six fixed primitive slot ids
    /// `quadraui_binding_core` defines -- `SLOT_BUTTON` etc.), re-rendering
    /// so `screen()` no longer shows it. Returns whether the slot held
    /// anything. Raises if `slot` doesn't name one of the six.
    #[napi(js_name = "slotClear")]
    pub fn slot_clear(&mut self, env: Env, slot: i64) -> Result<bool> {
        self.check_not_dispatching()?;
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
    /// inside an already-live dispatch.
    #[napi(js_name = "setHandler")]
    pub fn set_handler(
        &mut self,
        handler: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.driver.app_mut().handler = Some(handler.create_ref()?);
        Ok(())
    }

    /// Registers `handler` as this app's tick handler, called whenever the
    /// runner advances with no event queued. Raises if called re-entrantly.
    #[napi(js_name = "setTickHandler")]
    pub fn set_tick_handler(
        &mut self,
        handler: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        self.driver.app_mut().on_tick = Some(handler.create_ref()?);
        Ok(())
    }

    /// Registers `callback` as this app's render callback, called once per
    /// frame and expected to return a plain object decodable the same way a
    /// `pushEvent` descriptor is -- a flat map of slot names to descriptors,
    /// or a layout tree (see `quadraui_binding_core::decode_render_output`'s
    /// doc for the flat-vs-tree sniff). Overrides the six fixed `insert*`
    /// slots for every subsequent render. Raises if called re-entrantly.
    #[napi(js_name = "setRender")]
    pub fn set_render(
        &mut self,
        callback: Function<'_, Unknown<'static>, Unknown<'static>>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
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
    /// up front" discipline every other `QuiApp` method enforces).
    #[napi]
    pub fn actions(&self) -> Result<QuiAppActions> {
        self.check_not_dispatching()?;
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
    #[napi(js_name = "clearExtra")]
    pub fn clear_extra(&mut self, env: Env, kind: String) -> Result<bool> {
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
    /// Raises `InvalidDescriptorError` on a shape mismatch.
    #[napi(js_name = "pushEvent")]
    pub fn push_event(&mut self, env: Env, descriptor: Unknown) -> Result<()> {
        self.check_not_dispatching()?;
        let event: UiEvent = decode_direct(env, "pushEvent", descriptor)?;
        self.pending_events.push_back(event);
        Ok(())
    }
}

impl QuiApp {
    /// Shared body of every fixed-slot `insert*` method: reject an
    /// occupied slot, decode `descriptor` into `T` via [`decode_direct`],
    /// store it with `store`, then re-render -- the napi-rs analog of
    /// `quadraui-py`'s own `insert` helper.
    fn insert_fixed<T: DeserializeOwned>(
        &mut self,
        env: Env,
        slot: i64,
        what: &str,
        store: impl FnOnce(&mut NodeAppLogic, T),
        descriptor: Unknown,
    ) -> Result<i64> {
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
    /// helper.
    fn store_extra(
        &mut self,
        env: Env,
        kind: &str,
        what: &str,
        descriptor: Unknown,
    ) -> Result<String> {
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

    /// Runs `body` on `self`, catching a Rust panic instead of letting it
    /// propagate as napi-rs's own generic panic-to-`Error` conversion (which
    /// stops the process aborting but does not poison this handle). On
    /// success returns `body`'s result unchanged; on a panic, poisons this
    /// app and raises an `Error` carrying [`panic_message`]'s text. If this
    /// app is already poisoned (an earlier call panicked) or already
    /// dispatching (see [`Self::check_not_dispatching`]), raises without
    /// running `body` at all -- mirrors `quadraui-py`'s `QuiApp::guarded`,
    /// plus the reentrancy guard PyO3's own runtime borrow check makes
    /// unnecessary there. `env` is stashed on this app's [`NodeAppLogic`]
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
        self.driver.app_mut().env.set(Some(env));
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
    /// Returns whether the app is still running.
    fn step_once(&mut self) -> Result<bool> {
        let running = step_driver(&mut self.driver, &mut self.pending_events);
        take_pending_error(self.driver.app_mut())?;
        Ok(running)
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
}
