//! Spike C ABI proof-of-concept over quadraui's headless TUI backend.
//!
//! This crate is a scaffold, not a full C ABI — it proves
//! the opaque-handle + JSON-descriptor + vtable-callback + non-blocking-step
//! shape end to end for a handful of core primitives, each inserted through
//! its own `qui_<name>_insert` function following exactly the same pattern:
//! [`quadraui::Button`] ([`qui_button_insert`]), [`quadraui::TextDisplay`]
//! ([`qui_text_display_insert`] — quadraui has no primitive literally named
//! "Label"; `TextDisplay` is the closest match), [`quadraui::ListView`]
//! ([`qui_list_insert`]), [`quadraui::TreeView`] ([`qui_tree_insert`]),
//! [`quadraui::DataTable`] ([`qui_data_table_insert`]), and
//! [`quadraui::TabBar`] ([`qui_tab_bar_insert`]). Driven through quadraui's
//! headless `tui` backend ([`quadraui::tui::testing::TuiDriver`]) so the PoC
//! runs in any sandbox or CI host with no real terminal and no
//! platform-specific GUI toolkit. See `README.md` at the root of this crate
//! for the toolchain evaluation (cbindgen vs Diplomat vs UniFFI), the
//! rationale for `tui` over `gtk` as the PoC backend, and the call on how
//! this work relates to #118.
//!
//! Remaining primitives (ComboBox, NumberInput, DatePicker, ScrollView,
//! context menu, dialog, …) are intentionally out of scope here; they follow
//! as sibling batches rather than one big-bang extension.
//!
//! ## Surface
//!
//! - [`QuiApp`] is the opaque handle (`QuiApp*` on the C side); every other
//!   function takes one as its first argument.
//! - Each `qui_<name>_insert` function takes a UTF-8 JSON-encoded descriptor
//!   and returns an opaque per-app slot id (an `i64`, `-1` on error). Every
//!   primitive gets exactly one slot per app — a second insert of the same
//!   type is an error — so the returned id is always `0` on success; it
//!   exists for forward compatibility with a future multi-instance slot.
//! - [`QuiVTable`] is the vtable callbacks a host registers: `handle`/`tick`
//!   function pointers plus a `user_data` the host owns. There is
//!   deliberately no `render` member: quadraui's retained-descriptor model
//!   means this crate always owns painting a descriptor it holds, so a
//!   foreign `render` callback would mean reimplementing `draw_button`'s
//!   layout/paint logic on the other side of the ABI for every consumer
//!   language — exactly the per-backend duplication the descriptor-store
//!   design exists to avoid.
//! - [`qui_push_event`] queues any JSON-encoded [`quadraui::UiEvent`] (mouse,
//!   key, scroll, focus, window, …) for the next [`qui_step`]/[`qui_run`]
//!   call — the real input surface a host drives: construct the JSON for
//!   whichever variant the host's own input source produced and hand it
//!   across the ABI unchanged, same JSON-only-for-payloads shape every
//!   `qui_<name>_insert` function already uses. [`qui_push_key_event`] stays
//!   as an ergonomic convenience over it for the common single-character-key
//!   case. [`qui_resize`] additionally resizes the headless backend's cell
//!   grid before queuing the matching `WindowResized` event, since resizing
//!   the grid itself is not something any `UiEvent` payload can express.
//!   Every queued event dispatches in the order it was pushed (FIFO).
//! - [`qui_step`]/[`qui_run`]/[`qui_screen_contains`] are the non-blocking
//!   step loop and the assertion a host (or this crate's own smoke test)
//!   uses to prove a frame actually painted the button's label.
//! - [`qui_last_error`]/[`qui_free`] are the `int` error code + callee-
//!   allocated string + explicit-free shape a C caller needs to recover
//!   error detail without leaking memory.
//!
//! A thread-safe wake-from-another-thread entry point is intentionally
//! **not** part of this PoC: its `TuiDriver` is single-threaded with no
//! background wake source to exercise. It stays a design note for the real
//! C ABI, not something this scaffold implements.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::slice;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Button, DataTable, Key, ListView, Modifiers, Reaction, Rect, TabBar,
    TextDisplay, TreeView, UiEvent, Viewport,
};

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn set_last_error(message: impl Into<String>) {
    LAST_ERROR.with(|cell| *cell.borrow_mut() = Some(message.into()));
}

fn clear_last_error() {
    LAST_ERROR.with(|cell| *cell.borrow_mut() = None);
}

/// Vtable a host passes to [`qui_app_new`]: the `handle`/`tick` callbacks a
/// foreign caller registers. Deliberately has no `render` member: quadraui
/// paints every descriptor it holds itself, so there is nothing for a
/// foreign callback to do.
///
/// Both callbacks return a `Reaction` code: `0` = Continue, `1` = Redraw,
/// `2` = Exit. Any other value is treated as `Continue` (forward-compatible
/// with a future reaction this PoC doesn't define yet, same tolerance
/// `#[non_exhaustive]` gives Rust callers of [`quadraui::Reaction`]).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct QuiVTable {
    /// Opaque pointer the host owns; passed back unchanged into `handle`
    /// and `tick`. Never dereferenced by this crate.
    pub user_data: *mut c_void,
    /// Called once per event `qui_step`/`qui_run` dequeues. `event_json`
    /// points at `event_json_len` UTF-8 bytes (not null-terminated); the
    /// pointer is valid only for the duration of this call.
    pub handle: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize) -> i32>,
    /// Called once per `qui_step`/`qui_run` call when no event is queued.
    pub tick: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
}

fn reaction_from_code(code: i32) -> Reaction {
    match code {
        1 => Reaction::Redraw,
        2 => Reaction::Exit,
        _ => Reaction::Continue,
    }
}

/// Backend-generic app logic the PoC drives through
/// [`quadraui::tui::testing::TuiDriver`]: one retained descriptor per
/// supported primitive, plus the two foreign callbacks from [`QuiVTable`].
struct FfiApp {
    button: Option<Button>,
    text_display: Option<TextDisplay>,
    list: Option<ListView>,
    tree: Option<TreeView>,
    data_table: Option<DataTable>,
    tab_bar: Option<TabBar>,
    vtable: QuiVTable,
}

impl AppLogic for FfiApp {
    type AreaId = ();

    /// Paints whatever descriptors are currently held, each through the
    /// same `Backend::draw_*` method every in-tree TUI app uses, stacked
    /// vertically in a fixed band order so no two primitives overlap. See
    /// [`QuiVTable`]'s doc for why there is no foreign `render` callback.
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
                let _ = backend.draw_button(rect, button);
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
                let _ = backend.draw_data_table(rect, data_table, None);
            }
        }
        if let Some(tab_bar) = &self.tab_bar {
            if let Some(rect) = next_band(3.0) {
                let _ = backend.draw_tab_bar_layout(rect, tab_bar, None);
            }
        }
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let Some(handle_fn) = self.vtable.handle else {
            return Reaction::Continue;
        };
        let json = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(err) => {
                set_last_error(format!("failed to serialize UiEvent to JSON: {err}"));
                return Reaction::Continue;
            }
        };
        let code = unsafe { handle_fn(self.vtable.user_data, json.as_ptr(), json.len()) };
        reaction_from_code(code)
    }

    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        let Some(tick_fn) = self.vtable.tick else {
            return Reaction::Continue;
        };
        let code = unsafe { tick_fn(self.vtable.user_data) };
        reaction_from_code(code)
    }
}

/// Opaque handle to one running PoC app. Never constructed or inspected by
/// the caller; only ever passed back into this crate's other functions.
pub struct QuiApp {
    driver: TuiDriver<FfiApp>,
    /// FIFO queue: [`qui_push_event`], [`qui_push_key_event`] and
    /// [`qui_resize`] all push at the back, [`qui_step`] pops from the
    /// front, so events dispatch in the order they were queued regardless
    /// of which function queued them.
    pending_events: VecDeque<UiEvent>,
}

/// Create a new headless PoC app on a `width`×`height` cell grid.
///
/// Returns null (and sets the last error, see [`qui_last_error`]) if
/// `width`/`height` is `0`.
///
/// # Safety
/// `vtable`'s `user_data` is an opaque pointer this crate never
/// dereferences; the caller is responsible for its lifetime outliving every
/// call made with the returned [`QuiApp`] handle.
#[no_mangle]
pub unsafe extern "C" fn qui_app_new(width: u16, height: u16, vtable: QuiVTable) -> *mut QuiApp {
    clear_last_error();
    if width == 0 || height == 0 {
        set_last_error("qui_app_new: width and height must both be non-zero");
        return std::ptr::null_mut();
    }
    let app = FfiApp {
        button: None,
        text_display: None,
        list: None,
        tree: None,
        data_table: None,
        tab_bar: None,
        vtable,
    };
    let driver = TuiDriver::new(app, width, height);
    let handle = QuiApp {
        driver,
        pending_events: VecDeque::new(),
    };
    Box::into_raw(Box::new(handle))
}

/// Destroy an app created by [`qui_app_new`]. `app` may be null (no-op).
///
/// # Safety
/// `app` must be either null or a still-live pointer returned by
/// [`qui_app_new`] that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn qui_app_free(app: *mut QuiApp) {
    if app.is_null() {
        return;
    }
    drop(Box::from_raw(app));
}

/// Insert the one `Button` descriptor this PoC supports, JSON-encoded at
/// `json`/`json_len`. On success, immediately repaints the app's single
/// frame so the inserted button is visible to [`qui_screen_contains`] even
/// before the next [`qui_step`]/[`qui_run`] call.
///
/// Returns `0` on success (the PoC's single-slot handle — there is only
/// ever one descriptor, so no caller-visible numbering is needed yet) or
/// `-1` on error (bad JSON, null app/json, or a descriptor already
/// inserted), with [`qui_last_error`] set to why.
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`]. `json` must be either
/// null (only valid when `json_len` is `0`, and rejected as an error) or a
/// pointer to `json_len` valid UTF-8 bytes.
#[no_mangle]
pub unsafe extern "C" fn qui_button_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_button_insert: app is null");
        return -1;
    };
    if json.is_null() {
        set_last_error("qui_button_insert: json is null");
        return -1;
    }
    if app.driver.app().button.is_some() {
        set_last_error("qui_button_insert: this PoC supports exactly one Button descriptor");
        return -1;
    }
    let bytes = slice::from_raw_parts(json, json_len);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            set_last_error(format!("qui_button_insert: invalid UTF-8: {err}"));
            return -1;
        }
    };
    let button: Button = match serde_json::from_str(text) {
        Ok(button) => button,
        Err(err) => {
            set_last_error(format!("qui_button_insert: invalid Button JSON: {err}"));
            return -1;
        }
    };
    app.driver.app_mut().button = Some(button);
    app.driver.render();
    0
}

/// Insert the one `TextDisplay` descriptor this PoC supports, JSON-encoded
/// at `json`/`json_len`. quadraui has no primitive literally named "Label";
/// `TextDisplay` is the closest match for that role. Behaves exactly like
/// [`qui_button_insert`] otherwise: immediately repaints on success, and
/// returns `0`/`-1` with [`qui_last_error`] set on failure.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_text_display_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_text_display_insert: app is null");
        return -1;
    };
    if json.is_null() {
        set_last_error("qui_text_display_insert: json is null");
        return -1;
    }
    if app.driver.app().text_display.is_some() {
        set_last_error(
            "qui_text_display_insert: this PoC supports exactly one TextDisplay descriptor",
        );
        return -1;
    }
    let bytes = slice::from_raw_parts(json, json_len);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            set_last_error(format!("qui_text_display_insert: invalid UTF-8: {err}"));
            return -1;
        }
    };
    let text_display: TextDisplay = match serde_json::from_str(text) {
        Ok(text_display) => text_display,
        Err(err) => {
            set_last_error(format!(
                "qui_text_display_insert: invalid TextDisplay JSON: {err}"
            ));
            return -1;
        }
    };
    app.driver.app_mut().text_display = Some(text_display);
    app.driver.render();
    0
}

/// Insert the one `ListView` descriptor this PoC supports, JSON-encoded at
/// `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_list_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_list_insert: app is null");
        return -1;
    };
    if json.is_null() {
        set_last_error("qui_list_insert: json is null");
        return -1;
    }
    if app.driver.app().list.is_some() {
        set_last_error("qui_list_insert: this PoC supports exactly one ListView descriptor");
        return -1;
    }
    let bytes = slice::from_raw_parts(json, json_len);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            set_last_error(format!("qui_list_insert: invalid UTF-8: {err}"));
            return -1;
        }
    };
    let list: ListView = match serde_json::from_str(text) {
        Ok(list) => list,
        Err(err) => {
            set_last_error(format!("qui_list_insert: invalid ListView JSON: {err}"));
            return -1;
        }
    };
    app.driver.app_mut().list = Some(list);
    app.driver.render();
    0
}

/// Insert the one `TreeView` descriptor this PoC supports, JSON-encoded at
/// `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_tree_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_tree_insert: app is null");
        return -1;
    };
    if json.is_null() {
        set_last_error("qui_tree_insert: json is null");
        return -1;
    }
    if app.driver.app().tree.is_some() {
        set_last_error("qui_tree_insert: this PoC supports exactly one TreeView descriptor");
        return -1;
    }
    let bytes = slice::from_raw_parts(json, json_len);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            set_last_error(format!("qui_tree_insert: invalid UTF-8: {err}"));
            return -1;
        }
    };
    let tree: TreeView = match serde_json::from_str(text) {
        Ok(tree) => tree,
        Err(err) => {
            set_last_error(format!("qui_tree_insert: invalid TreeView JSON: {err}"));
            return -1;
        }
    };
    app.driver.app_mut().tree = Some(tree);
    app.driver.render();
    0
}

/// Insert the one `DataTable` descriptor this PoC supports, JSON-encoded at
/// `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_data_table_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_data_table_insert: app is null");
        return -1;
    };
    if json.is_null() {
        set_last_error("qui_data_table_insert: json is null");
        return -1;
    }
    if app.driver.app().data_table.is_some() {
        set_last_error("qui_data_table_insert: this PoC supports exactly one DataTable descriptor");
        return -1;
    }
    let bytes = slice::from_raw_parts(json, json_len);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            set_last_error(format!("qui_data_table_insert: invalid UTF-8: {err}"));
            return -1;
        }
    };
    let data_table: DataTable = match serde_json::from_str(text) {
        Ok(data_table) => data_table,
        Err(err) => {
            set_last_error(format!(
                "qui_data_table_insert: invalid DataTable JSON: {err}"
            ));
            return -1;
        }
    };
    app.driver.app_mut().data_table = Some(data_table);
    app.driver.render();
    0
}

/// Insert the one `TabBar` descriptor this PoC supports, JSON-encoded at
/// `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_tab_bar_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_tab_bar_insert: app is null");
        return -1;
    };
    if json.is_null() {
        set_last_error("qui_tab_bar_insert: json is null");
        return -1;
    }
    if app.driver.app().tab_bar.is_some() {
        set_last_error("qui_tab_bar_insert: this PoC supports exactly one TabBar descriptor");
        return -1;
    }
    let bytes = slice::from_raw_parts(json, json_len);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            set_last_error(format!("qui_tab_bar_insert: invalid UTF-8: {err}"));
            return -1;
        }
    };
    let tab_bar: TabBar = match serde_json::from_str(text) {
        Ok(tab_bar) => tab_bar,
        Err(err) => {
            set_last_error(format!("qui_tab_bar_insert: invalid TabBar JSON: {err}"));
            return -1;
        }
    };
    app.driver.app_mut().tab_bar = Some(tab_bar);
    app.driver.render();
    0
}

/// Queue any [`quadraui::UiEvent`] for the next [`qui_step`]/[`qui_run`]
/// call, JSON-encoded at `json`/`json_len` — the real input surface a host
/// drives: mouse, key, scroll, focus, window and every other variant all go
/// through this one function, exactly the same JSON-only-for-payloads shape
/// every `qui_<name>_insert` function already uses. Mirror quadraui's own
/// `#[derive(Serialize, Deserialize)]` field names and shape for the variant
/// being sent (see `quadraui::UiEvent`'s docs for each variant's payload).
///
/// [`quadraui::UiEvent::User`] deliberately has no `Serialize`/`Deserialize`
/// impl (its payload is an in-process-only `Arc<dyn Any>`), so JSON for it
/// always fails to decode here — there is nothing meaningful to send across
/// a wire boundary for that variant.
///
/// Returns `0` on success, `-1` if `app` is null, `json` is null (with
/// `json_len != 0`), the bytes aren't valid UTF-8, or the JSON doesn't
/// decode to a `UiEvent` — see [`qui_last_error`] for which.
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`]. `json` must be either
/// null (only valid when `json_len` is `0`, and rejected as an error) or a
/// pointer to `json_len` valid bytes.
#[no_mangle]
pub unsafe extern "C" fn qui_push_event(app: *mut QuiApp, json: *const u8, json_len: usize) -> i32 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_push_event: app is null");
        return -1;
    };
    if json.is_null() {
        set_last_error("qui_push_event: json is null");
        return -1;
    }
    let bytes = slice::from_raw_parts(json, json_len);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            set_last_error(format!("qui_push_event: invalid UTF-8: {err}"));
            return -1;
        }
    };
    let event: UiEvent = match serde_json::from_str(text) {
        Ok(event) => event,
        Err(err) => {
            set_last_error(format!("qui_push_event: invalid UiEvent JSON: {err}"));
            return -1;
        }
    };
    app.pending_events.push_back(event);
    0
}

/// Queue a synthetic `KeyPressed` event for the next [`qui_step`]/
/// [`qui_run`] call — a convenience over [`qui_push_event`] for the common
/// single-character-key case, so a host doesn't have to hand-build the
/// `KeyPressed` JSON just to type a character.
///
/// Returns `0` on success, `-1` if `app` is null.
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_push_key_event(app: *mut QuiApp, ch: u32) -> i32 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_push_key_event: app is null");
        return -1;
    };
    let Some(ch) = char::from_u32(ch) else {
        set_last_error("qui_push_key_event: not a valid Unicode scalar value");
        return -1;
    };
    app.pending_events.push_back(UiEvent::KeyPressed {
        key: Key::Char(ch),
        modifiers: Modifiers::default(),
        repeat: false,
    });
    0
}

/// Resize the app's headless cell grid to `width`×`height` and queue the
/// matching `WindowResized` event — the "resize" leg of the real input
/// surface, kept separate from [`qui_push_event`] because resizing the
/// underlying grid itself is a [`quadraui::tui::testing::TuiDriver`]
/// operation, not something any `UiEvent` payload can express on its own; a
/// plain [`qui_push_event`] with a `WindowResized` payload would notify the
/// app's `handle` callback but leave the grid at its old size.
///
/// Returns `0` on success, `-1` if `app` is null or `width`/`height` is `0`.
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_resize(app: *mut QuiApp, width: u16, height: u16) -> i32 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_resize: app is null");
        return -1;
    };
    if width == 0 || height == 0 {
        set_last_error("qui_resize: width and height must both be non-zero");
        return -1;
    }
    app.driver.resize(width, height);
    app.pending_events.push_back(UiEvent::WindowResized {
        viewport: Viewport::new(f32::from(width), f32::from(height), 1.0),
    });
    0
}

/// `qui_step`'s result — mirrors `quadraui::StepOutcome`, plus `-1` for a
/// call error (see [`qui_last_error`]).
const QUI_STEP_CONTINUE: i32 = 0;
const QUI_STEP_EXITED: i32 = 1;
const QUI_STEP_ERROR: i32 = -1;

/// Advance the app by one non-blocking step: dispatch one queued event (if
/// any; see [`qui_push_event`]/[`qui_push_key_event`]/[`qui_resize`] —
/// events dispatch in the order they were queued) through the app's
/// `handle` callback, otherwise call its `tick`
/// callback, then repaint if the reaction calls for it. Never blocks
/// waiting for input, because there is nothing to wait on in this headless
/// PoC.
///
/// Returns `0` (continue), `1` (the app exited; further calls are a no-op
/// returning `1` again), or `-1` on error.
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_step(app: *mut QuiApp) -> i32 {
    clear_last_error();
    let Some(app) = app.as_mut() else {
        set_last_error("qui_step: app is null");
        return QUI_STEP_ERROR;
    };
    if app.driver.exited() {
        return QUI_STEP_EXITED;
    }
    // `TuiDriver::dispatch`/`tick` already apply the returned `Reaction`
    // themselves (repainting on `Redraw`, latching exit on `Exit`), so this
    // only needs to translate their result into the C-side step code.
    //
    // `catch_unwind` here cannot catch a panic from `handle_fn`/`tick_fn`
    // themselves: those are `extern "C"` functions, and a panic unwinding
    // out of a non-`C-unwind` ABI boundary aborts the process before it
    // would ever reach this frame. What this guards is a panic inside
    // quadraui's own `dispatch`/`tick`/`render` path (e.g. a bug in
    // `Backend::draw_button`). A caught panic may leave `app.driver` in an
    // inconsistent state; callers should treat `QUI_STEP_ERROR` as fatal
    // for that `QuiApp` rather than continuing to step it.
    let reaction = if let Some(event) = app.pending_events.pop_front() {
        match panic::catch_unwind(AssertUnwindSafe(|| app.driver.dispatch(event))) {
            Ok(reaction) => reaction,
            Err(_) => {
                set_last_error("qui_step: a panic occurred while dispatching the event");
                return QUI_STEP_ERROR;
            }
        }
    } else {
        match panic::catch_unwind(AssertUnwindSafe(|| app.driver.tick())) {
            Ok(reaction) => reaction,
            Err(_) => {
                set_last_error("qui_step: a panic occurred while ticking the app");
                return QUI_STEP_ERROR;
            }
        }
    };
    if matches!(reaction, Reaction::Exit) {
        QUI_STEP_EXITED
    } else {
        QUI_STEP_CONTINUE
    }
}

/// Run [`qui_step`] in a loop until the app exits or `max_steps` is reached
/// (whichever comes first): this headless PoC's stand-in for a real blocking
/// `qui_run` entry point, reduced to "drain the queue, polling `tick` in
/// between", since there is no real event source to block on. `max_steps`
/// is clamped to at least `1`, so `qui_run(app, 0)` still performs one step
/// rather than returning immediately without advancing the app at all.
///
/// Returns the same codes as [`qui_step`].
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_run(app: *mut QuiApp, max_steps: u32) -> i32 {
    let mut last = QUI_STEP_CONTINUE;
    for _ in 0..max_steps.max(1) {
        last = qui_step(app);
        if last != QUI_STEP_CONTINUE {
            return last;
        }
    }
    last
}

/// `true` (`1`) if the app's current rendered frame contains `needle`,
/// `false` (`0`) if not, `-1` on error — the assertion both this crate's
/// own smoke test and a foreign-language caller use to confirm a frame
/// actually painted the inserted [`Button`]'s label.
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`]. `needle` must be
/// either null (only valid when `needle_len` is `0`, and rejected as an
/// error) or a pointer to `needle_len` valid UTF-8 bytes.
#[no_mangle]
pub unsafe extern "C" fn qui_screen_contains(
    app: *const QuiApp,
    needle: *const u8,
    needle_len: usize,
) -> i32 {
    clear_last_error();
    let Some(app) = app.as_ref() else {
        set_last_error("qui_screen_contains: app is null");
        return -1;
    };
    if needle.is_null() {
        set_last_error("qui_screen_contains: needle is null");
        return -1;
    }
    let bytes = slice::from_raw_parts(needle, needle_len);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => {
            set_last_error(format!("qui_screen_contains: invalid UTF-8: {err}"));
            return -1;
        }
    };
    i32::from(app.driver.screen_contains(text))
}

/// Return the last error message set by a call on the current thread, as a
/// callee-allocated null-terminated UTF-8 string the caller must free with
/// [`qui_free`]. Returns null if (and only if) there is no error — a
/// message that itself contains an interior NUL (not producible by any
/// error this crate sets today, since they're all built from serde/UTF-8
/// error `Display` output) falls back to a fixed placeholder string rather
/// than null, so "no error" is never ambiguous with "there was an error we
/// couldn't represent".
#[no_mangle]
pub extern "C" fn qui_last_error() -> *mut c_char {
    LAST_ERROR.with(|cell| match cell.borrow().as_deref() {
        Some(message) => match CString::new(message) {
            Ok(c_string) => c_string.into_raw(),
            Err(_) => CString::new("<error message contained an interior NUL>")
                .expect("fixed fallback string has no interior NUL")
                .into_raw(),
        },
        None => std::ptr::null_mut(),
    })
}

/// Free a string returned by [`qui_last_error`]. `ptr` may be null (no-op).
///
/// # Safety
/// `ptr` must be either null or a pointer previously returned by
/// [`qui_last_error`] on this same allocator, not already freed.
#[no_mangle]
pub unsafe extern "C" fn qui_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    drop(CString::from_raw(ptr));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button_json() -> String {
        serde_json::json!({ "id": "demo:ok", "label": "OK" }).to_string()
    }

    unsafe extern "C" fn record_handle(
        user_data: *mut c_void,
        _json: *const u8,
        _len: usize,
    ) -> i32 {
        let count = user_data as *mut i32;
        *count += 1;
        0
    }

    /// Decodes the dispatched event's JSON and records which character it
    /// carried, so the FIFO-vs-LIFO ordering test below can tell the two
    /// apart.
    unsafe extern "C" fn record_order_handle(
        user_data: *mut c_void,
        json: *const u8,
        len: usize,
    ) -> i32 {
        let bytes = slice::from_raw_parts(json, len);
        let text = std::str::from_utf8(bytes).expect("event JSON is valid UTF-8");
        let event: UiEvent = serde_json::from_str(text).expect("event JSON round-trips");
        if let UiEvent::KeyPressed {
            key: Key::Char(ch), ..
        } = event
        {
            let order = user_data as *mut Vec<char>;
            (*order).push(ch);
        }
        0
    }

    #[test]
    fn insert_run_and_find_the_button_label() {
        let mut handle_calls: i32 = 0;
        let vtable = QuiVTable {
            user_data: &mut handle_calls as *mut i32 as *mut c_void,
            handle: Some(record_handle),
            tick: None,
        };
        unsafe {
            let app = qui_app_new(40, 5, vtable);
            assert!(!app.is_null());

            let json = button_json();
            let inserted = qui_button_insert(app, json.as_ptr(), json.len());
            assert_eq!(inserted, 0);
            assert!(qui_last_error().is_null());

            assert_eq!(qui_screen_contains(app, b"OK".as_ptr(), 2), 1);

            assert_eq!(qui_push_key_event(app, 'x' as u32), 0);
            assert_eq!(qui_run(app, 4), QUI_STEP_CONTINUE);
            assert_eq!(handle_calls, 1);

            qui_app_free(app);
        }
    }

    /// Regression test for the event queue's ordering contract: both this
    /// module's and the public doc's wording promise FIFO dispatch, so two
    /// events queued as `'a'` then `'b'` must be observed by the handle
    /// callback in that same order, not reversed.
    #[test]
    fn events_dispatch_in_fifo_order() {
        let mut order: Vec<char> = Vec::new();
        let vtable = QuiVTable {
            user_data: &mut order as *mut Vec<char> as *mut c_void,
            handle: Some(record_order_handle),
            tick: None,
        };
        unsafe {
            let app = qui_app_new(10, 3, vtable);
            assert!(!app.is_null());

            assert_eq!(qui_push_key_event(app, 'a' as u32), 0);
            assert_eq!(qui_push_key_event(app, 'b' as u32), 0);
            assert_eq!(qui_run(app, 2), QUI_STEP_CONTINUE);

            qui_app_free(app);
        }
        assert_eq!(order, vec!['a', 'b']);
    }

    #[test]
    fn bad_json_reports_last_error() {
        let vtable = QuiVTable {
            user_data: std::ptr::null_mut(),
            handle: None,
            tick: None,
        };
        unsafe {
            let app = qui_app_new(10, 3, vtable);
            let bad = b"not json";
            let inserted = qui_button_insert(app, bad.as_ptr(), bad.len());
            assert_eq!(inserted, -1);

            let error_ptr = qui_last_error();
            assert!(!error_ptr.is_null());
            let message = std::ffi::CStr::from_ptr(error_ptr)
                .to_string_lossy()
                .into_owned();
            assert!(message.contains("invalid Button JSON"));
            qui_free(error_ptr);

            qui_app_free(app);
        }
    }

    fn no_op_vtable() -> QuiVTable {
        QuiVTable {
            user_data: std::ptr::null_mut(),
            handle: None,
            tick: None,
        }
    }

    #[test]
    fn insert_run_and_find_the_text_display_line() {
        let json = serde_json::json!({
            "id": "text:1",
            "lines": [{ "spans": [{ "text": "Hello", "fg": null, "bg": null }] }]
        })
        .to_string();
        unsafe {
            let app = qui_app_new(40, 5, no_op_vtable());
            assert!(!app.is_null());

            let inserted = qui_text_display_insert(app, json.as_ptr(), json.len());
            assert_eq!(inserted, 0);
            assert!(qui_last_error().is_null());
            assert_eq!(qui_screen_contains(app, b"Hello".as_ptr(), 5), 1);

            // A second insert is rejected: the PoC keeps exactly one slot
            // per type.
            let again = qui_text_display_insert(app, json.as_ptr(), json.len());
            assert_eq!(again, -1);
            let error_ptr = qui_last_error();
            assert!(!error_ptr.is_null());
            qui_free(error_ptr);

            qui_app_free(app);
        }
    }

    #[test]
    fn insert_run_and_find_the_list_item() {
        let json = serde_json::json!({
            "id": "list:1",
            "items": [{ "text": { "spans": [{ "text": "Row 1", "fg": null, "bg": null }] } }],
            "selected_idx": 0
        })
        .to_string();
        unsafe {
            let app = qui_app_new(40, 5, no_op_vtable());
            assert!(!app.is_null());

            let inserted = qui_list_insert(app, json.as_ptr(), json.len());
            assert_eq!(inserted, 0);
            assert!(qui_last_error().is_null());
            assert_eq!(qui_screen_contains(app, b"Row 1".as_ptr(), 5), 1);

            qui_app_free(app);
        }
    }

    #[test]
    fn insert_run_and_find_the_tree_row() {
        let json = serde_json::json!({
            "id": "tree:1",
            "rows": [{
                "path": [0],
                "indent": 0,
                "icon": null,
                "text": { "spans": [{ "text": "Alpha", "fg": null, "bg": null }] },
                "badge": null,
                "is_expanded": null
            }],
            "selection_mode": "Single",
            "selected_path": null,
            "style": {
                "indent": 2,
                "show_chevrons": true,
                "chevron_expanded": "v",
                "chevron_collapsed": ">"
            }
        })
        .to_string();
        unsafe {
            let app = qui_app_new(40, 5, no_op_vtable());
            assert!(!app.is_null());

            let inserted = qui_tree_insert(app, json.as_ptr(), json.len());
            assert_eq!(inserted, 0);
            assert!(qui_last_error().is_null());
            assert_eq!(qui_screen_contains(app, b"Alpha".as_ptr(), 5), 1);

            qui_app_free(app);
        }
    }

    #[test]
    fn insert_run_and_find_the_data_table_cell() {
        let json = serde_json::json!({
            "id": "table:1",
            "columns": [{ "title": "Name" }],
            "rows": [{ "cells": [{ "spans": [{ "text": "Row1", "fg": null, "bg": null }] }] }]
        })
        .to_string();
        unsafe {
            let app = qui_app_new(40, 5, no_op_vtable());
            assert!(!app.is_null());

            let inserted = qui_data_table_insert(app, json.as_ptr(), json.len());
            assert_eq!(inserted, 0);
            assert!(qui_last_error().is_null());
            assert_eq!(qui_screen_contains(app, b"Row1".as_ptr(), 4), 1);

            qui_app_free(app);
        }
    }

    #[test]
    fn insert_run_and_find_the_tab_bar_label() {
        let json = serde_json::json!({
            "id": "tabbar:1",
            "tabs": [{ "label": "Tab 1" }]
        })
        .to_string();
        unsafe {
            let app = qui_app_new(40, 5, no_op_vtable());
            assert!(!app.is_null());

            let inserted = qui_tab_bar_insert(app, json.as_ptr(), json.len());
            assert_eq!(inserted, 0);
            assert!(qui_last_error().is_null());
            assert_eq!(qui_screen_contains(app, b"Tab 1".as_ptr(), 5), 1);

            qui_app_free(app);
        }
    }

    #[test]
    fn bad_json_reports_last_error_for_each_new_primitive() {
        let bad = b"not json";
        unsafe {
            let app = qui_app_new(10, 3, no_op_vtable());
            assert!(!app.is_null());

            for (name, inserted) in [
                (
                    "text_display",
                    qui_text_display_insert(app, bad.as_ptr(), bad.len()),
                ),
                ("list", qui_list_insert(app, bad.as_ptr(), bad.len())),
                ("tree", qui_tree_insert(app, bad.as_ptr(), bad.len())),
                (
                    "data_table",
                    qui_data_table_insert(app, bad.as_ptr(), bad.len()),
                ),
                ("tab_bar", qui_tab_bar_insert(app, bad.as_ptr(), bad.len())),
            ] {
                assert_eq!(inserted, -1, "{name} should reject non-JSON input");
                let error_ptr = qui_last_error();
                assert!(!error_ptr.is_null(), "{name} should set qui_last_error");
                qui_free(error_ptr);
            }

            qui_app_free(app);
        }
    }

    /// Decodes the dispatched event's JSON and records the outer tag (the
    /// externally-tagged variant name serde emits, e.g. `"MouseDown"`) so a
    /// test can assert which `UiEvent` variants actually arrived, in order.
    unsafe extern "C" fn record_variant_tag_handle(
        user_data: *mut c_void,
        json: *const u8,
        len: usize,
    ) -> i32 {
        let bytes = slice::from_raw_parts(json, len);
        let text = std::str::from_utf8(bytes).expect("event JSON is valid UTF-8");
        let value: serde_json::Value =
            serde_json::from_str(text).expect("event JSON is valid JSON");
        let tag = value
            .as_object()
            .and_then(|obj| obj.keys().next())
            .cloned()
            .unwrap_or_else(|| value.to_string());
        let order = user_data as *mut Vec<String>;
        (*order).push(tag);
        0
    }

    /// `qui_push_event` round-trips every non-`KeyPressed` shape the issue
    /// calls out by name (mouse down, scroll, window resize) through the
    /// exact same `handle`-callback JSON path [`qui_push_key_event`] already
    /// proved for keys — no per-variant special-casing in `handle` means
    /// none is needed here either.
    #[test]
    fn qui_push_event_round_trips_mouse_and_scroll_variants() {
        let mut order: Vec<String> = Vec::new();
        let vtable = QuiVTable {
            user_data: &mut order as *mut Vec<String> as *mut c_void,
            handle: Some(record_variant_tag_handle),
            tick: None,
        };
        unsafe {
            let app = qui_app_new(40, 10, vtable);
            assert!(!app.is_null());

            let mouse_down = serde_json::json!({
                "MouseDown": {
                    "widget": null,
                    "button": "Left",
                    "position": { "x": 3.0, "y": 1.0 },
                    "modifiers": {}
                }
            })
            .to_string();
            assert_eq!(
                qui_push_event(app, mouse_down.as_ptr(), mouse_down.len()),
                0
            );
            assert!(qui_last_error().is_null());

            let scroll = serde_json::json!({
                "Scroll": {
                    "widget": null,
                    "delta": { "x": 0.0, "y": 1.0 },
                    "position": { "x": 3.0, "y": 1.0 }
                }
            })
            .to_string();
            assert_eq!(qui_push_event(app, scroll.as_ptr(), scroll.len()), 0);
            assert!(qui_last_error().is_null());

            assert_eq!(qui_run(app, 2), QUI_STEP_CONTINUE);

            qui_app_free(app);
        }
        assert_eq!(order, vec!["MouseDown".to_string(), "Scroll".to_string()]);
    }

    /// `qui_push_event` rejects malformed input through the same
    /// null/UTF-8/JSON error ladder every other push/insert function uses.
    #[test]
    fn qui_push_event_rejects_bad_input() {
        unsafe {
            let app = qui_app_new(10, 3, no_op_vtable());
            assert!(!app.is_null());

            let bad = b"not an event";
            assert_eq!(qui_push_event(app, bad.as_ptr(), bad.len()), -1);
            let error_ptr = qui_last_error();
            assert!(!error_ptr.is_null());
            let message = std::ffi::CStr::from_ptr(error_ptr)
                .to_string_lossy()
                .into_owned();
            assert!(message.contains("invalid UiEvent JSON"));
            qui_free(error_ptr);

            assert_eq!(
                qui_push_event(std::ptr::null_mut(), bad.as_ptr(), bad.len()),
                -1
            );
            assert!(!qui_last_error().is_null());

            qui_app_free(app);
        }
    }

    /// [`quadraui::UiEvent::User`] has no `Deserialize` impl by design (its
    /// payload is an in-process-only `Arc<dyn Any>`), so JSON claiming to be
    /// one must be rejected the same way any other malformed payload is,
    /// not treated as a special case.
    #[test]
    fn qui_push_event_rejects_the_user_variant() {
        unsafe {
            let app = qui_app_new(10, 3, no_op_vtable());
            assert!(!app.is_null());

            let user_event = serde_json::json!({ "User": {} }).to_string();
            assert_eq!(
                qui_push_event(app, user_event.as_ptr(), user_event.len()),
                -1
            );
            assert!(!qui_last_error().is_null());

            qui_app_free(app);
        }
    }

    /// `qui_resize` both grows the headless grid `qui_app_new` sized and
    /// queues the matching `WindowResized` event, so a host doesn't have to
    /// separately remember to call both.
    #[test]
    fn qui_resize_grows_the_grid_and_queues_window_resized() {
        let mut order: Vec<String> = Vec::new();
        let vtable = QuiVTable {
            user_data: &mut order as *mut Vec<String> as *mut c_void,
            handle: Some(record_variant_tag_handle),
            tick: None,
        };
        unsafe {
            let app = qui_app_new(10, 3, vtable);
            assert!(!app.is_null());

            assert_eq!(qui_resize(app, 80, 24), 0);
            assert!(qui_last_error().is_null());
            // `TuiDriver::screen` reads the physical cell buffer directly
            // (not the `Backend::viewport()` cache, which only resyncs on
            // the next render — see `TuiDriver::resize`'s doc), so it
            // reflects the resize immediately.
            let screen = (*app).driver.screen();
            let lines: Vec<&str> = screen.lines().collect();
            assert_eq!(lines.len(), 24, "grid should have grown to 24 rows");
            assert_eq!(
                lines[0].chars().count(),
                80,
                "grid should have grown to 80 columns"
            );

            assert_eq!(qui_run(app, 1), QUI_STEP_CONTINUE);

            qui_app_free(app);
        }
        assert_eq!(order, vec!["WindowResized".to_string()]);
    }

    #[test]
    fn qui_resize_rejects_zero_dimensions_and_null_app() {
        unsafe {
            let app = qui_app_new(10, 3, no_op_vtable());
            assert!(!app.is_null());

            assert_eq!(qui_resize(app, 0, 5), -1);
            assert!(!qui_last_error().is_null());
            assert_eq!(qui_resize(app, 5, 0), -1);
            assert!(!qui_last_error().is_null());
            assert_eq!(qui_resize(std::ptr::null_mut(), 5, 5), -1);
            assert!(!qui_last_error().is_null());

            qui_app_free(app);
        }
    }

    /// FIFO ordering holds across every push function, not just within one
    /// of them: a key queued via [`qui_push_key_event`], a mouse event via
    /// [`qui_push_event`], and a resize via [`qui_resize`] must dispatch in
    /// the order they were queued.
    #[test]
    fn events_from_every_push_function_dispatch_in_fifo_order() {
        let mut order: Vec<String> = Vec::new();
        let vtable = QuiVTable {
            user_data: &mut order as *mut Vec<String> as *mut c_void,
            handle: Some(record_variant_tag_handle),
            tick: None,
        };
        unsafe {
            let app = qui_app_new(10, 3, vtable);
            assert!(!app.is_null());

            assert_eq!(qui_push_key_event(app, 'a' as u32), 0);
            let mouse_up = serde_json::json!({
                "MouseUp": {
                    "widget": null,
                    "button": "Left",
                    "position": { "x": 0.0, "y": 0.0 }
                }
            })
            .to_string();
            assert_eq!(qui_push_event(app, mouse_up.as_ptr(), mouse_up.len()), 0);
            assert_eq!(qui_resize(app, 20, 6), 0);

            assert_eq!(qui_run(app, 3), QUI_STEP_CONTINUE);

            qui_app_free(app);
        }
        assert_eq!(
            order,
            vec![
                "KeyPressed".to_string(),
                "MouseUp".to_string(),
                "WindowResized".to_string(),
            ]
        );
    }
}
