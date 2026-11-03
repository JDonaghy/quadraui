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
//! backend-settings replay store) plus a [`quadraui_binding_core::InteractionEngine`]
//! and a pending-error slot -- the napi-rs analog of `quadraui-py`'s
//! `PyAppLogic`. Today's surface is the headless driver loop (`new`, `step`,
//! `run`, `screen`, `screen_contains`, `resize`, `push_key_event`,
//! `slot_clear`) plus the descriptor decode adapter below: the six fixed
//! `insert*` methods, `pushEvent`, and the additive-primitive `insertExtra`/
//! `insertProgress`/`insertSpinner`/`insertImage`/`clearExtra`. No JS event
//! handler/render callback is registered anywhere yet -- every dispatched
//! event is accepted with no side effect -- that is an expected future
//! addition to this crate's surface, mirroring `quadraui-py`'s own.
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

use std::cell::RefCell;
use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Button, DataTable, ListView, Reaction, Rect, TabBar, TextDisplay, TreeView,
    UiEvent, WidgetId,
};
use quadraui_binding_core::{
    decode_extra_primitive, draw_bands, key_pressed_event, panic_message, render_overlays,
    step_driver, BindingHost, DecodeError, InteractionEngine, SLOT_BUTTON, SLOT_DATA_TABLE,
    SLOT_LIST, SLOT_TAB_BAR, SLOT_TEXT_DISPLAY, SLOT_TREE,
};
#[cfg(test)]
use quadraui_binding_core::{decode_layout_node, decode_render_output, LayoutNode, RenderOutput};

/// The [`AppLogic`] a [`QuiApp`] drives: a thin napi-rs leaf around
/// [`BindingHost`] (the six fixed primitive slots, any additive slots, and
/// the backend-settings replay store -- see that type's own doc) plus the
/// [`InteractionEngine`] every binding's `AppLogic` holds alongside it, and
/// a pending-error slot a registered JS handler/render callback would
/// populate the same way `quadraui-py`'s `PyAppLogic::pending_error` does --
/// see [`take_pending_error`]. Nothing writes it today: this struct's own
/// `handle` has no handler to call.
#[derive(Default)]
struct NodeAppLogic {
    /// The host-side slot/replay state this binding is a thin napi-rs leaf
    /// over -- see [`BindingHost`]'s own doc.
    host: BindingHost,
    /// The language-neutral `event -> hit -> semantic` orchestration engine
    /// this binding is a thin napi-rs leaf over -- see
    /// `quadraui_binding_core::engine`'s module doc. [`AppLogic::render`]
    /// feeds it this frame's painted hits via [`InteractionEngine::set_hits`];
    /// a registered JS handler would forward dispatch to it the same way
    /// `quadraui-py`'s `PyAppLogic::handle` does.
    engine: InteractionEngine,
    /// Would be set by a registered JS callback when it raises. Present on
    /// this struct even with no handler to call yet, so its shape already
    /// matches `quadraui-py`'s `PyAppLogic`; every render-triggering
    /// `QuiApp` method already drains it via [`take_pending_error`], so
    /// adding a handler only means writing to this slot, not touching any
    /// call site that reads it.
    pending_error: Rc<RefCell<Option<Error>>>,
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

impl AppLogic for NodeAppLogic {
    type AreaId = ();

    /// Replays [`Self::host`]'s accelerators and backend-settings fields
    /// onto `backend` -- [`BindingHost::setup`] -- mirrors `quadraui-py`'s
    /// `PyAppLogic::setup`.
    fn setup(&mut self, backend: &mut dyn Backend) {
        self.host.setup(backend);
    }

    /// Paints this app's six fixed slots (nothing, since no `insert_*`
    /// method exists yet to populate [`Self::host`]) plus the anchored
    /// overlay pass every binding's `render` runs -- see
    /// [`render_overlays`]'s own doc.
    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut hits = Vec::new();
        let chrome_interaction = |id: WidgetId| self.engine.chrome_interaction(id);
        draw_bands(
            backend,
            self.host.render_refs(),
            &mut hits,
            &chrome_interaction,
        );
        render_overlays(&self.engine, backend, hits);
    }

    /// Delegates to [`InteractionEngine::tab_stops`], which derives this
    /// frame's tab stops directly from the hit map [`Self::render`] just
    /// fed it via [`InteractionEngine::set_hits`].
    fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        self.engine.tab_stops()
    }

    /// No JS handler exists yet, so every event is accepted with no side
    /// effect -- the same `Reaction::Continue` default `quadraui-py`'s
    /// `PyAppLogic::handle` falls back to when no handler is registered.
    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// Takes and re-raises any error a registered callback recorded during the
/// `render()` call that just ran -- mirrors `quadraui-py`'s
/// `take_pending_error`. Always `Ok(())` today, since [`NodeAppLogic`] has
/// no handler that could ever write to `pending_error`; kept as the one
/// drain point every render-triggering [`QuiApp`] method already calls, so
/// a future registered handler/render callback only needs to write the
/// error, not add a new drain call site.
fn take_pending_error(app: &mut NodeAppLogic) -> Result<()> {
    if let Some(err) = app.pending_error.borrow_mut().take() {
        return Err(err);
    }
    Ok(())
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

/// A headless quadraui app, driven entirely from JavaScript/TypeScript.
///
/// Construct it with a `width`/`height` cell grid, insert descriptors with
/// `insertButton`/`insertTextDisplay`/`insertList`/`insertTree`/
/// `insertDataTable`/`insertTabBar`/`insertExtra` (and its typed
/// convenience wrappers `insertProgress`/`insertSpinner`/`insertImage`),
/// and inspect the rendered frame with `screen()`/`screenContains()`. No
/// JS event handler/render callback is registered anywhere yet, so a
/// `pushEvent`-queued event is accepted with no side effect -- an expected
/// future addition to this surface, mirroring `quadraui-py`'s own.
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
        })
    }

    /// Resizes the underlying cell grid to `width`x`height`. Raises if
    /// either is zero.
    #[napi]
    pub fn resize(&mut self, width: u16, height: u16) -> Result<()> {
        let (width, height) = checked_dimensions(width, height)?;
        self.guarded(|app| {
            app.driver.resize(width, height);
            Ok(())
        })
    }

    /// Queues a synthetic `KeyPressed` event for the character with Unicode
    /// code point `ch`, no modifiers held, not a repeat. Raises if `ch`
    /// isn't a valid Unicode scalar value.
    #[napi(js_name = "pushKeyEvent")]
    pub fn push_key_event(&mut self, ch: u32) -> Result<()> {
        let ch = char::from_u32(ch)
            .ok_or_else(|| Error::from_reason(format!("invalid Unicode code point: {ch}")))?;
        self.pending_events.push_back(key_pressed_event(ch));
        Ok(())
    }

    /// Advances the runner by one step: dispatches the oldest event queued
    /// by `pushKeyEvent`, or ticks if none is queued. Returns `true` if the
    /// app is still running afterwards, `false` once it has exited.
    #[napi]
    pub fn step(&mut self) -> Result<bool> {
        self.guarded(|app| app.step_once())
    }

    /// Runs up to `max_steps` steps (default 1). Stops early once the app
    /// exits. Returns `true` if still running afterwards, `false` once
    /// exited.
    #[napi]
    pub fn run(&mut self, max_steps: Option<u32>) -> Result<bool> {
        let steps = max_steps.unwrap_or(1).max(1);
        let mut running = true;
        for _ in 0..steps {
            running = self.guarded(|app| app.step_once())?;
            if !running {
                break;
            }
        }
        Ok(running)
    }

    /// Renders the current frame and returns it as a plain-text screen
    /// dump, one row per line.
    #[napi]
    pub fn screen(&mut self) -> Result<String> {
        self.guarded(|app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            Ok(app.driver.screen())
        })
    }

    /// Whether `needle` appears anywhere in the current rendered frame.
    #[napi(js_name = "screenContains")]
    pub fn screen_contains(&mut self, needle: String) -> Result<bool> {
        self.guarded(|app| {
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
    pub fn slot_clear(&mut self, slot: i64) -> Result<bool> {
        if !NodeAppLogic::is_known_slot(slot) {
            return Err(Error::from_reason(format!("unknown slot id: {slot}")));
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
    pub fn insert_progress(&mut self, descriptor: Unknown) -> Result<String> {
        self.store_extra("progress", "insertProgress", descriptor)
    }

    /// Inserts a `Spinner` descriptor from a JS object, returning
    /// `"spinner"` back. Raises `SlotOccupiedError` if `"spinner"` is
    /// already occupied -- call `clearExtra("spinner")` first. A thin,
    /// typed-descriptor convenience over [`Self::insert_extra`].
    #[napi(js_name = "insertSpinner")]
    pub fn insert_spinner(&mut self, descriptor: Unknown) -> Result<String> {
        self.store_extra("spinner", "insertSpinner", descriptor)
    }

    /// Inserts an `Image` descriptor from a JS object, returning `"image"`
    /// back. Raises `SlotOccupiedError` if `"image"` is already occupied --
    /// call `clearExtra("image")` first. A thin, typed-descriptor
    /// convenience over [`Self::insert_extra`].
    #[napi(js_name = "insertImage")]
    pub fn insert_image(&mut self, descriptor: Unknown) -> Result<String> {
        self.store_extra("image", "insertImage", descriptor)
    }

    /// Inserts a descriptor for any additive primitive `kind` registered in
    /// `quadraui_binding_core::extra_paint`'s `extra_primitives!` table (an
    /// `InvalidDescriptorError` raised for an unknown `kind` names every
    /// registered one), returning `kind` back. Raises `SlotOccupiedError`
    /// if `kind` is already occupied -- call `clearExtra(kind)` first. Lets
    /// a consumer insert a future additive primitive by name alone, with no
    /// new napi method needed on this class.
    #[napi(js_name = "insertExtra")]
    pub fn insert_extra(&mut self, kind: String, descriptor: Unknown) -> Result<String> {
        self.store_extra(&kind, "insertExtra", descriptor)
    }

    /// Empties an additive primitive slot (one registered in
    /// `quadraui_binding_core::extra_paint`'s `extra_primitives!` table),
    /// re-rendering so `screen()` no longer shows it. Returns whether the
    /// slot held anything; an unrecognized `kind` simply has nothing to
    /// clear (unlike `slotClear`, this never raises for an unknown key).
    #[napi(js_name = "clearExtra")]
    pub fn clear_extra(&mut self, kind: String) -> Result<bool> {
        self.guarded(|app| {
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
        self.guarded(|app| {
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
    fn store_extra(&mut self, kind: &str, what: &str, descriptor: Unknown) -> Result<String> {
        if self.driver.app().extra_occupied(kind) {
            return Err(slot_occupied_error(format!(
                "{what}: {kind:?} already occupied; call clearExtra({kind:?}) first"
            )));
        }
        let value = decode_via_core(what, descriptor, |value| {
            decode_extra_primitive(what, kind, value)
        })?;
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
}

impl QuiApp {
    /// Runs `body` on `self`, catching a Rust panic instead of letting it
    /// propagate as napi-rs's own generic panic-to-`Error` conversion (which
    /// stops the process aborting but does not poison this handle). On
    /// success returns `body`'s result unchanged; on a panic, poisons this
    /// app and raises an `Error` carrying [`panic_message`]'s text. If this
    /// app is already poisoned (an earlier call panicked), raises without
    /// running `body` at all -- mirrors `quadraui-py`'s `QuiApp::guarded`.
    fn guarded<T>(&mut self, body: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.poisoned {
            return Err(Error::from_reason(
                "this QuiApp is poisoned by an earlier caught panic and can no longer be used",
            ));
        }
        match panic::catch_unwind(AssertUnwindSafe(|| body(self))) {
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
        let screen = app.screen().expect("render must not fail");
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
    /// nothing in this crate's current surface can request exit.
    #[test]
    fn step_and_run_keep_running() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(app.step().expect("step must not fail"));
        assert!(app.run(Some(3)).expect("run must not fail"));
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
        assert!(app.slot_clear(999).is_err());
    }

    /// `slotClear` on a never-occupied slot is a no-op that reports
    /// `false`, not an error.
    #[test]
    fn slot_clear_on_empty_slot_reports_false() {
        let mut app = QuiApp::new(5, 5).expect("valid dimensions");
        assert!(!app
            .slot_clear(quadraui_binding_core::SLOT_BUTTON)
            .expect("a known slot id must not raise"));
    }

    /// A Rust panic raised from inside `guarded`'s body is caught and
    /// re-raised as an `Error`, and poisons the app so a later call raises
    /// without running its body -- mirrors `quadraui-py`'s
    /// `guarded_catches_panic_then_poisons`.
    #[test]
    fn guarded_catches_panic_then_poisons() {
        let mut app = QuiApp::new(10, 10).expect("valid dimensions");

        let panicked: Result<()> = app.guarded(|_| panic!("boom"));
        assert!(panicked.is_err(), "a caught panic must surface as an error");
        assert!(app.poisoned);

        let mut body_ran = false;
        let after_poison: Result<()> = app.guarded(|_| {
            body_ran = true;
            Ok(())
        });
        assert!(!body_ran, "a poisoned app must not run the guarded body");
        assert!(after_poison.is_err(), "a poisoned app must keep erroring");
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
