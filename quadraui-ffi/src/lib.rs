//! C ABI over quadraui's headless TUI backend.
//!
//! One opaque handle per app, JSON for every descriptor and event payload, a
//! C vtable for the app's callbacks, a non-blocking step function, and a
//! thread-local error channel. The crate is deliberately narrow — it covers
//! six primitives (`Button`, `TextDisplay`, `ListView`, `TreeView`,
//! `DataTable`, `TabBar`), one descriptor per type — but the boundary itself
//! is meant to be shippable: every entry point is panic-guarded, every
//! failure maps to a documented integer code, and every pointer handed out
//! has a stated owner.
//!
//! Everything runs on the headless `tui` backend
//! ([`quadraui::tui::testing::TuiDriver`]), so a consumer gets a real
//! `event -> handle -> render` pipeline with no terminal attached: it works
//! in a sandbox, in CI, and under a foreign-language test runner.
//! `README.md` records the binding-generator evaluation (cbindgen vs
//! Diplomat vs UniFFI) that led to this shape. Primitives outside the six
//! above (ComboBox, NumberInput, DatePicker, ScrollView, context menu,
//! dialog) are not exposed yet.
//!
//! ## Surface
//!
//! - [`qui_app_new`] / [`qui_app_free`] — create and destroy a [`QuiApp`],
//!   the opaque per-app handle every other function takes.
//! - `qui_<name>_insert` — install one JSON-encoded descriptor into its
//!   slot, repainting immediately. Each returns its `QUI_SLOT_*` id on
//!   success, so the id can be handed straight back to [`qui_slot_clear`].
//!   A slot holds at most one descriptor; inserting over an occupied slot is
//!   [`QUI_ERR_SLOT_OCCUPIED`], clearing it first is the way to swap.
//! - [`QuiVTable`] — the host's `handle`/`tick` callbacks. There is
//!   deliberately no `render` member: quadraui paints the descriptors it
//!   holds, so a foreign renderer callback would have nothing to do.
//! - [`qui_push_event`] / [`qui_push_key_event`] / [`qui_resize`] — queue
//!   input. All three share one FIFO queue, so events dispatch in exactly
//!   the order they were pushed, whichever function pushed them.
//! - [`qui_step`] / [`qui_run`] — advance the app without blocking.
//! - [`qui_screen_contains`] / [`qui_screen`] — assert on, or dump, the
//!   rendered frame.
//! - [`qui_last_error_code`] / [`qui_last_error`] / [`qui_error_name`] —
//!   read back why the previous call failed.
//! - [`qui_free`] — free any string this crate allocated for the caller.
//!
//! ## Error taxonomy
//!
//! Fallible entry points return `QUI_OK` (`0`) or a non-negative result on
//! success, and a negative `QUI_ERR_*` code on failure; [`qui_app_new`]
//! returns null instead, since it has no handle to report through.
//! **Every** failure path also sets the thread-local last error, readable as
//! an integer with [`qui_last_error_code`], as a human-readable message with
//! [`qui_last_error`], and as the code's spelling with [`qui_error_name`].
//!
//! | code | meaning |
//! |---|---|
//! | [`QUI_OK`] | success |
//! | [`QUI_ERR_NULL_HANDLE`] | the [`QuiApp`] handle was null |
//! | [`QUI_ERR_NULL_POINTER`] | a non-handle pointer argument was null |
//! | [`QUI_ERR_EMPTY_INPUT`] | a `(pointer, length)` argument had length `0` |
//! | [`QUI_ERR_INVALID_UTF8`] | input bytes were not valid UTF-8 |
//! | [`QUI_ERR_INVALID_JSON`] | input did not decode to the expected type |
//! | [`QUI_ERR_SLOT_OCCUPIED`] | the descriptor slot already holds a descriptor |
//! | [`QUI_ERR_UNKNOWN_SLOT`] | the slot id is not one of the `QUI_SLOT_*` values |
//! | [`QUI_ERR_INVALID_DIMENSIONS`] | a width/height argument was `0` |
//! | [`QUI_ERR_INVALID_CHAR`] | a `uint32_t` was not a Unicode scalar value |
//! | [`QUI_ERR_PANIC`] | a Rust panic was caught at the boundary |
//! | [`QUI_ERR_POISONED`] | this handle is unusable after a caught panic |
//! | [`QUI_ERR_INTERNAL`] | a bug in this crate (serialization failure, …) |
//!
//! Each failure is reported exactly once at its own layer: the first check
//! that fails wins, and no call both fails and leaves a stale success code
//! behind — a successful call clears the last error before doing any work.
//! The four pure readback functions ([`qui_last_error`],
//! [`qui_last_error_code`], [`qui_error_name`]) and the two deallocators
//! ([`qui_app_free`], [`qui_free`]) deliberately leave the last error
//! untouched, so a host can free its resources on the error path and still
//! report what went wrong afterwards.
//!
//! ## Memory ownership
//!
//! Caller-owned, borrowed by this crate for the duration of one call only:
//!
//! - every `(const uint8_t *ptr, uintptr_t len)` pair (descriptor JSON,
//!   event JSON, screen needle). Never retained past the call — the bytes
//!   are decoded into owned Rust values before returning.
//! - [`QuiVTable::user_data`] — never dereferenced by this crate, only
//!   passed back to the host's own callbacks. Its lifetime must outlive the
//!   [`QuiApp`] handle.
//! - `event_json` inside the `handle` callback — owned by this crate, valid
//!   only for the duration of that call. Copy it if you need it later.
//!
//! Callee-allocated, caller frees:
//!
//! - [`QuiApp`] from [`qui_app_new`] — freed with [`qui_app_free`], exactly
//!   once. Never with `free()`.
//! - the `char *` from [`qui_last_error`] and [`qui_screen`] — freed with
//!   [`qui_free`], exactly once. Never with `free()`: the string comes from
//!   Rust's allocator, which need not be C's.
//!
//! Static, caller must **not** free:
//!
//! - the `const char *` from [`qui_error_name`] — a pointer into this
//!   crate's read-only data, valid for the life of the process.
//!
//! Nothing in this ABI takes ownership of a caller-allocated pointer, so no
//! function here can be the cause of a double free on the C side.
//!
//! ## Panic safety
//!
//! Every `extern "C"` entry point wraps its body in
//! [`std::panic::catch_unwind`], so a Rust panic — in a descriptor
//! rasteriser, in serde, anywhere below the boundary — is converted into
//! [`QUI_ERR_PANIC`] plus a last-error message instead of unwinding into C,
//! which would be undefined behaviour. A panic caught mid-dispatch can
//! leave the driver's state inconsistent, so the [`QuiApp`] handle is
//! *poisoned*: every subsequent call on it fails fast with
//! [`QUI_ERR_POISONED`], and the only useful thing left to do with it is
//! [`qui_app_free`].
//!
//! The one thing this cannot guard is a panic *inside a host callback*.
//! `handle` and `tick` are `extern "C"` function pointers, and unwinding out
//! of a non-`C-unwind` ABI boundary aborts the process before any frame of
//! this crate runs. A host callback must not let an exception or panic
//! escape.
//!
//! ## Threading
//!
//! The last-error channel is thread-local: read it on the same thread that
//! made the failing call. A [`QuiApp`] handle is not synchronised and must
//! be used from one thread at a time, and a thread-safe
//! wake-from-another-thread entry point is intentionally not part of this
//! surface.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::slice;
use std::time::Duration;

use quadraui::descriptor_store::{DescriptorStore, Handle};
use quadraui::paths::{self, AppPathKind};
use quadraui::tui::testing::TuiDriver;
use quadraui::tui::{StepOutcome, TuiRunner};
use quadraui::{
    AppLogic, Backend, Button, DataTable, Key, ListItemMeasure, ListView, ListViewHit, Modifiers,
    NamedKey, Point, Reaction, Rect, TabBar, TextDisplay, TreeView, UiEvent, Viewport,
};

/// The call succeeded. Also the "continue" step result of [`qui_step`].
pub const QUI_OK: i32 = 0;
/// The [`QuiApp`] handle argument was null.
pub const QUI_ERR_NULL_HANDLE: i32 = -1;
/// A pointer argument other than the app handle was null.
pub const QUI_ERR_NULL_POINTER: i32 = -2;
/// A `(pointer, length)` argument had length `0`, which never names a
/// meaningful descriptor, event or needle.
pub const QUI_ERR_EMPTY_INPUT: i32 = -3;
/// Input bytes were not valid UTF-8.
pub const QUI_ERR_INVALID_UTF8: i32 = -4;
/// Input was valid UTF-8 but did not decode to the type the entry point
/// expected (malformed JSON, or JSON of the wrong shape).
pub const QUI_ERR_INVALID_JSON: i32 = -5;
/// The descriptor slot already holds a descriptor. Clear it with
/// [`qui_slot_clear`] before inserting another.
pub const QUI_ERR_SLOT_OCCUPIED: i32 = -6;
/// The slot id was not one of the `QUI_SLOT_*` values.
pub const QUI_ERR_UNKNOWN_SLOT: i32 = -7;
/// A width or height argument was `0`.
pub const QUI_ERR_INVALID_DIMENSIONS: i32 = -8;
/// A `uint32_t` character argument was not a Unicode scalar value (a
/// surrogate, or above `U+10FFFF`).
pub const QUI_ERR_INVALID_CHAR: i32 = -9;
/// A Rust panic was caught at the ABI boundary. The app handle it happened
/// on is poisoned; see [`QUI_ERR_POISONED`].
pub const QUI_ERR_PANIC: i32 = -10;
/// The app handle is poisoned by an earlier caught panic. Only
/// [`qui_app_free`] still works on it.
pub const QUI_ERR_POISONED: i32 = -11;
/// A bug in this crate — a failure that is not the caller's fault, such as
/// an event that would not serialize.
pub const QUI_ERR_INTERNAL: i32 = -12;
/// The `kind` passed to [`qui_app_path`] was not one of the `QUI_PATH_*`
/// values, or this platform or build could not resolve it.
pub const QUI_ERR_UNSUPPORTED: i32 = -13;
/// A real-terminal [`QuiRtApp`] operation failed at the OS/terminal layer
/// (negotiating raw mode, polling the input fd, resizing, …). Carries the
/// underlying `io::Error` message as the last-error text.
pub const QUI_ERR_IO: i32 = -14;

/// [`qui_step`] advanced the app and it is still running.
pub const QUI_STEP_CONTINUE: i32 = QUI_OK;
/// [`qui_step`] observed the app exit. Further steps are a no-op returning
/// this same value.
pub const QUI_STEP_EXITED: i32 = 1;

/// Slot id of the app's `Button` descriptor.
pub const QUI_SLOT_BUTTON: i32 = 0;
/// Slot id of the app's `TextDisplay` descriptor.
pub const QUI_SLOT_TEXT_DISPLAY: i32 = 1;
/// Slot id of the app's `ListView` descriptor.
pub const QUI_SLOT_LIST: i32 = 2;
/// Slot id of the app's `TreeView` descriptor.
pub const QUI_SLOT_TREE: i32 = 3;
/// Slot id of the app's `DataTable` descriptor.
pub const QUI_SLOT_DATA_TABLE: i32 = 4;
/// Slot id of the app's `TabBar` descriptor.
pub const QUI_SLOT_TAB_BAR: i32 = 5;

/// `kind` for [`qui_app_path`]: durable, app-scoped data (Electron's
/// `userData`), scoped by [`qui_set_app_identifier`].
pub const QUI_PATH_USER_DATA: i32 = 0;
/// `kind` for [`qui_app_path`]: app-scoped configuration, scoped by
/// [`qui_set_app_identifier`].
pub const QUI_PATH_APP_CONFIG: i32 = 1;
/// `kind` for [`qui_app_path`]: app-scoped cache, scoped by
/// [`qui_set_app_identifier`].
pub const QUI_PATH_APP_CACHE: i32 = 2;
/// `kind` for [`qui_app_path`]: app-scoped log directory, scoped by
/// [`qui_set_app_identifier`].
pub const QUI_PATH_LOGS: i32 = 3;
/// `kind` for [`qui_app_path`]: the OS-wide temporary directory, unscoped
/// by app identity.
pub const QUI_PATH_TEMP: i32 = 4;
/// `kind` for [`qui_app_path`]: the user's Downloads folder, unscoped by
/// app identity.
pub const QUI_PATH_DOWNLOADS: i32 = 5;
/// `kind` for [`qui_app_path`]: the user's Documents folder, unscoped by
/// app identity.
pub const QUI_PATH_DOCUMENTS: i32 = 6;
/// `kind` for [`qui_app_path`]: the user's home directory, unscoped by
/// app identity.
pub const QUI_PATH_HOME: i32 = 7;

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    static LAST_CODE: Cell<i32> = const { Cell::new(QUI_OK) };
}

/// Records `code` and `message` as this thread's last error.
fn set_last_error(code: i32, message: impl Into<String>) {
    LAST_ERROR.with(|cell| *cell.borrow_mut() = Some(message.into()));
    LAST_CODE.with(|cell| cell.set(code));
}

/// Records an error and returns its code, so a failure path reads as a
/// single `return fail(CODE, "why")`.
fn fail(code: i32, message: impl Into<String>) -> i32 {
    set_last_error(code, message);
    code
}

/// [`fail`] for the entry points that return a slot id.
fn fail_i64(code: i32, message: impl Into<String>) -> i64 {
    i64::from(fail(code, message))
}

fn clear_last_error() {
    LAST_ERROR.with(|cell| *cell.borrow_mut() = None);
    LAST_CODE.with(|cell| cell.set(QUI_OK));
}

/// Maps an error code onto the value an entry point returns for it, so one
/// generic boundary guard can serve every return type this ABI uses: a
/// status `int`, a slot `int64_t`, a pointer, or nothing at all.
trait AbiResult {
    fn from_error(code: i32) -> Self;
}

impl AbiResult for i32 {
    fn from_error(code: i32) -> Self {
        code
    }
}

impl AbiResult for i64 {
    fn from_error(code: i32) -> Self {
        i64::from(code)
    }
}

impl<T> AbiResult for *mut T {
    fn from_error(_code: i32) -> Self {
        std::ptr::null_mut()
    }
}

impl<T> AbiResult for *const T {
    fn from_error(_code: i32) -> Self {
        std::ptr::null()
    }
}

impl AbiResult for () {
    fn from_error(_code: i32) {}
}

/// Turns a caught panic payload into [`QUI_ERR_PANIC`] plus a last-error
/// message naming the entry point and, when the payload is a string (the
/// shape `panic!`, `assert!` and `unwrap` all produce), the panic's own
/// message.
fn record_panic(name: &str, payload: &(dyn Any + Send)) -> i32 {
    let detail = if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<non-string panic payload>".to_owned()
    };
    fail(
        QUI_ERR_PANIC,
        format!("{name}: caught a Rust panic at the ABI boundary: {detail}"),
    )
}

/// Runs `body` with unwinding stopped at the boundary, mapping a panic to
/// [`QUI_ERR_PANIC`]. Used by the entry points that take no app handle;
/// handle-taking ones use [`with_app`] / [`with_app_ref`], which additionally
/// poison the handle.
fn boundary<T: AbiResult>(name: &str, body: impl FnOnce() -> T) -> T {
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(payload) => T::from_error(record_panic(name, &*payload)),
    }
}

/// Resolves `app` to a live, unpoisoned handle and runs `body` on it behind
/// the panic boundary. A caught panic poisons the handle: the panic may have
/// unwound out of the middle of a dispatch or a repaint, leaving the driver
/// half-updated, and the only safe thing a host can still do with it is
/// [`qui_app_free`].
///
/// # Safety
/// `app` must be either null or a live pointer from [`qui_app_new`].
unsafe fn with_app<T: AbiResult>(
    name: &str,
    app: *mut QuiApp,
    body: impl FnOnce(&mut QuiApp) -> T,
) -> T {
    let Some(borrowed) = app.as_mut() else {
        return T::from_error(fail(
            QUI_ERR_NULL_HANDLE,
            format!("{name}: app handle is null"),
        ));
    };
    if borrowed.poisoned {
        return T::from_error(fail(
            QUI_ERR_POISONED,
            format!("{name}: app handle is poisoned by an earlier caught panic; free it with qui_app_free"),
        ));
    }
    match panic::catch_unwind(AssertUnwindSafe(|| body(borrowed))) {
        Ok(value) => value,
        Err(payload) => {
            (*app).poisoned = true;
            T::from_error(record_panic(name, &*payload))
        }
    }
}

/// [`with_app`] for the read-only entry points. A panic here cannot have
/// mutated the app, so the handle is left usable.
///
/// # Safety
/// `app` must be either null or a live pointer from [`qui_app_new`].
unsafe fn with_app_ref<T: AbiResult>(
    name: &str,
    app: *const QuiApp,
    body: impl FnOnce(&QuiApp) -> T,
) -> T {
    let Some(borrowed) = app.as_ref() else {
        return T::from_error(fail(
            QUI_ERR_NULL_HANDLE,
            format!("{name}: app handle is null"),
        ));
    };
    if borrowed.poisoned {
        return T::from_error(fail(
            QUI_ERR_POISONED,
            format!("{name}: app handle is poisoned by an earlier caught panic; free it with qui_app_free"),
        ));
    }
    boundary(name, || body(borrowed))
}

/// Borrows a caller-supplied `(pointer, length)` byte pair as a `&str`,
/// rejecting the three ways it can be unusable with their own error codes.
/// `what` names the argument for the error message, e.g. `"json"`.
///
/// # Safety
/// `ptr` must be either null or valid for reads of `len` bytes.
unsafe fn borrow_utf8<'a>(
    name: &str,
    what: &str,
    ptr: *const u8,
    len: usize,
) -> Result<&'a str, i32> {
    if ptr.is_null() {
        return Err(fail(
            QUI_ERR_NULL_POINTER,
            format!("{name}: {what} pointer is null"),
        ));
    }
    if len == 0 {
        return Err(fail(
            QUI_ERR_EMPTY_INPUT,
            format!("{name}: {what} is empty"),
        ));
    }
    std::str::from_utf8(slice::from_raw_parts(ptr, len)).map_err(|err| {
        fail(
            QUI_ERR_INVALID_UTF8,
            format!("{name}: {what} is not valid UTF-8: {err}"),
        )
    })
}

/// Hands `text` to the caller as a callee-allocated C string it frees with
/// [`qui_free`], or null (with the last error set) if the string cannot be
/// represented as one.
fn into_owned_c_string(name: &str, text: String) -> *mut c_char {
    match CString::new(text) {
        Ok(owned) => owned.into_raw(),
        Err(_) => {
            fail(
                QUI_ERR_INTERNAL,
                format!("{name}: the string contains an interior NUL byte"),
            );
            std::ptr::null_mut()
        }
    }
}

/// Vtable a host passes to [`qui_app_new`]: the `handle`/`tick` callbacks a
/// foreign caller registers. Deliberately has no `render` member: quadraui
/// paints every descriptor it holds itself, so there is nothing for a
/// foreign callback to do.
///
/// Both callbacks return a `Reaction` code: `0` = Continue, `1` = Redraw,
/// `2` = Exit. Any other value is treated as `Continue` (forward-compatible
/// with a future reaction this ABI doesn't define yet, the same tolerance
/// `#[non_exhaustive]` gives Rust callers of [`quadraui::Reaction`]).
///
/// Neither callback may let a panic or an exception escape: they are plain
/// `extern "C"` function pointers, and unwinding out of one aborts the
/// process. Catch on the host side and return a reaction code instead.
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

/// The [`AppLogic`] the C side drives: a bag of optional descriptors plus
/// the host's callbacks.
struct FfiApp {
    button: Option<Button>,
    text_display: Option<TextDisplay>,
    list: Option<ListView>,
    tree: Option<TreeView>,
    data_table: Option<DataTable>,
    tab_bar: Option<TabBar>,
    vtable: QuiVTable,
}

impl FfiApp {
    fn slot_occupied(&self, slot: i32) -> bool {
        match slot {
            QUI_SLOT_BUTTON => self.button.is_some(),
            QUI_SLOT_TEXT_DISPLAY => self.text_display.is_some(),
            QUI_SLOT_LIST => self.list.is_some(),
            QUI_SLOT_TREE => self.tree.is_some(),
            QUI_SLOT_DATA_TABLE => self.data_table.is_some(),
            QUI_SLOT_TAB_BAR => self.tab_bar.is_some(),
            _ => false,
        }
    }

    /// Empties one slot, returning whether it held anything.
    fn clear_slot(&mut self, slot: i32) -> bool {
        match slot {
            QUI_SLOT_BUTTON => self.button.take().is_some(),
            QUI_SLOT_TEXT_DISPLAY => self.text_display.take().is_some(),
            QUI_SLOT_LIST => self.list.take().is_some(),
            QUI_SLOT_TREE => self.tree.take().is_some(),
            QUI_SLOT_DATA_TABLE => self.data_table.take().is_some(),
            QUI_SLOT_TAB_BAR => self.tab_bar.take().is_some(),
            _ => false,
        }
    }
}

/// Name of the primitive a slot id holds, or `None` for an unknown id.
fn slot_label(slot: i32) -> Option<&'static str> {
    Some(match slot {
        QUI_SLOT_BUTTON => "Button",
        QUI_SLOT_TEXT_DISPLAY => "TextDisplay",
        QUI_SLOT_LIST => "ListView",
        QUI_SLOT_TREE => "TreeView",
        QUI_SLOT_DATA_TABLE => "DataTable",
        QUI_SLOT_TAB_BAR => "TabBar",
        _ => return None,
    })
}

/// Panic injection point the crate's own tests use to drive a real unwind
/// through the ABI boundary. Inert outside `cfg(test)`.
#[cfg(test)]
fn panic_hook_for_tests() {
    if tests::PANIC_IN_RENDER.with(Cell::get) {
        panic!("injected panic in FfiApp::render");
    }
}

#[cfg(not(test))]
fn panic_hook_for_tests() {}

impl AppLogic for FfiApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        panic_hook_for_tests();
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
        let Some(handle_fn) = self.vtable.handle else {
            return Reaction::Continue;
        };
        let json = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(err) => {
                // Not reachable from any `UiEvent` a caller can push (they
                // all arrived as JSON), so this is a crate bug rather than
                // caller error: record it and drop the event.
                set_last_error(
                    QUI_ERR_INTERNAL,
                    format!("failed to serialize UiEvent to JSON: {err}"),
                );
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

/// Opaque handle to one running app. Never constructed or inspected by the
/// caller; only ever passed back into this crate's other functions.
pub struct QuiApp {
    driver: TuiDriver<FfiApp>,
    /// Events queued by `qui_push_*`/`qui_resize` and not yet dispatched.
    /// One queue shared by every push function, drained front-first by
    /// [`qui_step`], so dispatch order is exactly push order.
    pending_events: VecDeque<UiEvent>,
    /// Set when a panic was caught while this handle was borrowed. The
    /// driver may be mid-update, so every entry point but [`qui_app_free`]
    /// refuses to touch it again.
    poisoned: bool,
}

/// Create a new headless app on a `width`×`height` cell grid.
///
/// Returns null on failure, with [`qui_last_error_code`] set to
/// [`QUI_ERR_INVALID_DIMENSIONS`] if `width`/`height` is `0`, or
/// [`QUI_ERR_PANIC`] if constructing the driver panicked.
///
/// The returned handle is owned by the caller and must be released with
/// [`qui_app_free`], never with `free()`.
///
/// # Safety
/// `vtable`'s `user_data` is an opaque pointer this crate never
/// dereferences; the caller is responsible for its lifetime outliving every
/// call made with the returned [`QuiApp`] handle.
#[no_mangle]
pub unsafe extern "C" fn qui_app_new(width: u16, height: u16, vtable: QuiVTable) -> *mut QuiApp {
    const NAME: &str = "qui_app_new";
    clear_last_error();
    boundary(NAME, || {
        if width == 0 || height == 0 {
            fail(
                QUI_ERR_INVALID_DIMENSIONS,
                format!("{NAME}: width and height must both be non-zero"),
            );
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
        Box::into_raw(Box::new(QuiApp {
            driver: TuiDriver::new(app, width, height),
            pending_events: VecDeque::new(),
            poisoned: false,
        }))
    })
}

/// Destroy an app created by [`qui_app_new`]. `app` may be null (no-op).
/// Works even on a handle poisoned by a caught panic — this is the one
/// function that does.
///
/// Leaves the last error untouched, so a host can free the app on its error
/// path and still report what failed afterwards.
///
/// # Safety
/// `app` must be either null or a still-live pointer returned by
/// [`qui_app_new`] that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn qui_app_free(app: *mut QuiApp) {
    boundary("qui_app_free", || {
        if app.is_null() {
            return;
        }
        drop(Box::from_raw(app));
    });
}

/// Shared body of every `qui_<name>_insert` function: validate, check the
/// slot, decode, repaint.
///
/// # Safety
/// `app` must be either null or a live pointer from [`qui_app_new`], and
/// `json` either null or valid for reads of `json_len` bytes.
unsafe fn insert_descriptor(
    name: &str,
    slot: i32,
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    clear_last_error();
    with_app(name, app, |app| {
        let text = match borrow_utf8(name, "json", json, json_len) {
            Ok(text) => text,
            Err(code) => return i64::from(code),
        };
        let Some(label) = slot_label(slot) else {
            return fail_i64(
                QUI_ERR_UNKNOWN_SLOT,
                format!("{name}: unknown slot id {slot}"),
            );
        };
        if app.driver.app().slot_occupied(slot) {
            return fail_i64(
                QUI_ERR_SLOT_OCCUPIED,
                format!(
                    "{name}: the {label} slot already holds a descriptor; clear it with qui_slot_clear first"
                ),
            );
        }

        // One arm per slot rather than a generic helper: the crate has no
        // direct `serde` dependency, so there is no `DeserializeOwned` bound
        // to be generic over.
        macro_rules! decode_into {
            ($ty:ty, $field:ident) => {
                match serde_json::from_str::<$ty>(text) {
                    Ok(decoded) => app.driver.app_mut().$field = Some(decoded),
                    Err(err) => {
                        return fail_i64(
                            QUI_ERR_INVALID_JSON,
                            format!("{name}: invalid {label} JSON: {err}"),
                        )
                    }
                }
            };
        }

        match slot {
            QUI_SLOT_BUTTON => decode_into!(Button, button),
            QUI_SLOT_TEXT_DISPLAY => decode_into!(TextDisplay, text_display),
            QUI_SLOT_LIST => decode_into!(ListView, list),
            QUI_SLOT_TREE => decode_into!(TreeView, tree),
            QUI_SLOT_DATA_TABLE => decode_into!(DataTable, data_table),
            QUI_SLOT_TAB_BAR => decode_into!(TabBar, tab_bar),
            _ => {
                return fail_i64(
                    QUI_ERR_UNKNOWN_SLOT,
                    format!("{name}: unknown slot id {slot}"),
                )
            }
        }

        app.driver.render();
        i64::from(slot)
    })
}

/// Insert the app's `Button` descriptor, JSON-encoded at `json`/`json_len`.
/// On success, immediately repaints the app's frame so the inserted button
/// is visible to [`qui_screen_contains`] even before the next
/// [`qui_step`]/[`qui_run`] call.
///
/// Returns the descriptor's slot id ([`QUI_SLOT_BUTTON`]) on success, or a
/// negative `QUI_ERR_*` code on failure: [`QUI_ERR_NULL_HANDLE`],
/// [`QUI_ERR_NULL_POINTER`], [`QUI_ERR_EMPTY_INPUT`],
/// [`QUI_ERR_INVALID_UTF8`], [`QUI_ERR_INVALID_JSON`],
/// [`QUI_ERR_SLOT_OCCUPIED`], [`QUI_ERR_POISONED`] or [`QUI_ERR_PANIC`],
/// with [`qui_last_error`] set to why. The slot id can be passed straight to
/// [`qui_slot_clear`] to take the descriptor back out.
///
/// The JSON bytes are borrowed for the duration of the call only.
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`]. `json` must be either
/// null (rejected as an error) or valid for reads of `json_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn qui_button_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    insert_descriptor("qui_button_insert", QUI_SLOT_BUTTON, app, json, json_len)
}

/// Insert the app's `TextDisplay` descriptor, JSON-encoded at
/// `json`/`json_len`. quadraui has no primitive literally named "Label";
/// `TextDisplay` is the closest match for that role. Behaves exactly like
/// [`qui_button_insert`] otherwise, returning [`QUI_SLOT_TEXT_DISPLAY`] on
/// success.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_text_display_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    insert_descriptor(
        "qui_text_display_insert",
        QUI_SLOT_TEXT_DISPLAY,
        app,
        json,
        json_len,
    )
}

/// Insert the app's `ListView` descriptor, JSON-encoded at `json`/`json_len`.
/// Behaves exactly like [`qui_button_insert`] otherwise, returning
/// [`QUI_SLOT_LIST`] on success.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_list_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    insert_descriptor("qui_list_insert", QUI_SLOT_LIST, app, json, json_len)
}

/// Insert the app's `TreeView` descriptor, JSON-encoded at `json`/`json_len`.
/// Behaves exactly like [`qui_button_insert`] otherwise, returning
/// [`QUI_SLOT_TREE`] on success.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_tree_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    insert_descriptor("qui_tree_insert", QUI_SLOT_TREE, app, json, json_len)
}

/// Insert the app's `DataTable` descriptor, JSON-encoded at
/// `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise,
/// returning [`QUI_SLOT_DATA_TABLE`] on success.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_data_table_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    insert_descriptor(
        "qui_data_table_insert",
        QUI_SLOT_DATA_TABLE,
        app,
        json,
        json_len,
    )
}

/// Insert the app's `TabBar` descriptor, JSON-encoded at `json`/`json_len`.
/// Behaves exactly like [`qui_button_insert`] otherwise, returning
/// [`QUI_SLOT_TAB_BAR`] on success.
///
/// # Safety
/// Same contract as [`qui_button_insert`].
#[no_mangle]
pub unsafe extern "C" fn qui_tab_bar_insert(
    app: *mut QuiApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    insert_descriptor("qui_tab_bar_insert", QUI_SLOT_TAB_BAR, app, json, json_len)
}

/// Take the descriptor back out of one slot and repaint, so the slot can be
/// filled again with a different descriptor. `slot` is one of the
/// `QUI_SLOT_*` values — the same id the matching `qui_<name>_insert`
/// returned.
///
/// Returns [`QUI_OK`] whether or not the slot held anything (clearing an
/// empty slot is not an error), or [`QUI_ERR_UNKNOWN_SLOT`] if `slot` is not
/// a `QUI_SLOT_*` value, plus the usual [`QUI_ERR_NULL_HANDLE`],
/// [`QUI_ERR_POISONED`] and [`QUI_ERR_PANIC`].
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_slot_clear(app: *mut QuiApp, slot: i32) -> i32 {
    const NAME: &str = "qui_slot_clear";
    clear_last_error();
    with_app(NAME, app, |app| {
        if slot_label(slot).is_none() {
            return fail(
                QUI_ERR_UNKNOWN_SLOT,
                format!("{NAME}: unknown slot id {slot}"),
            );
        }
        app.driver.app_mut().clear_slot(slot);
        app.driver.render();
        QUI_OK
    })
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
/// Returns [`QUI_OK`], or [`QUI_ERR_NULL_HANDLE`],
/// [`QUI_ERR_NULL_POINTER`], [`QUI_ERR_EMPTY_INPUT`],
/// [`QUI_ERR_INVALID_UTF8`], [`QUI_ERR_INVALID_JSON`],
/// [`QUI_ERR_POISONED`] or [`QUI_ERR_PANIC`] — see [`qui_last_error`] for
/// which.
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`]. `json` must be either
/// null (rejected as an error) or valid for reads of `json_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn qui_push_event(app: *mut QuiApp, json: *const u8, json_len: usize) -> i32 {
    const NAME: &str = "qui_push_event";
    clear_last_error();
    with_app(NAME, app, |app| {
        let text = match borrow_utf8(NAME, "json", json, json_len) {
            Ok(text) => text,
            Err(code) => return code,
        };
        let event: UiEvent = match serde_json::from_str(text) {
            Ok(event) => event,
            Err(err) => {
                return fail(
                    QUI_ERR_INVALID_JSON,
                    format!("{NAME}: invalid UiEvent JSON: {err}"),
                )
            }
        };
        app.pending_events.push_back(event);
        QUI_OK
    })
}

/// Queue a synthetic `KeyPressed` event for the next [`qui_step`]/
/// [`qui_run`] call — a convenience over [`qui_push_event`] for the common
/// single-character-key case, so a host doesn't have to hand-build the
/// `KeyPressed` JSON just to type a character.
///
/// Returns [`QUI_OK`], [`QUI_ERR_NULL_HANDLE`], [`QUI_ERR_INVALID_CHAR`] if
/// `ch` is not a Unicode scalar value, [`QUI_ERR_POISONED`] or
/// [`QUI_ERR_PANIC`].
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_push_key_event(app: *mut QuiApp, ch: u32) -> i32 {
    const NAME: &str = "qui_push_key_event";
    clear_last_error();
    with_app(NAME, app, |app| {
        let Some(ch) = char::from_u32(ch) else {
            return fail(
                QUI_ERR_INVALID_CHAR,
                format!("{NAME}: {ch} is not a valid Unicode scalar value"),
            );
        };
        app.pending_events.push_back(UiEvent::KeyPressed {
            key: Key::Char(ch),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        QUI_OK
    })
}

/// Resize the app's headless cell grid to `width`×`height` and queue the
/// matching `WindowResized` event — the "resize" leg of the real input
/// surface, kept separate from [`qui_push_event`] because resizing the
/// underlying grid itself is a [`quadraui::tui::testing::TuiDriver`]
/// operation, not something any `UiEvent` payload can express on its own; a
/// plain [`qui_push_event`] with a `WindowResized` payload would notify the
/// app's `handle` callback but leave the grid at its old size.
///
/// Returns [`QUI_OK`], [`QUI_ERR_NULL_HANDLE`],
/// [`QUI_ERR_INVALID_DIMENSIONS`] if `width`/`height` is `0`,
/// [`QUI_ERR_POISONED`] or [`QUI_ERR_PANIC`].
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_resize(app: *mut QuiApp, width: u16, height: u16) -> i32 {
    const NAME: &str = "qui_resize";
    clear_last_error();
    with_app(NAME, app, |app| {
        if width == 0 || height == 0 {
            return fail(
                QUI_ERR_INVALID_DIMENSIONS,
                format!("{NAME}: width and height must both be non-zero"),
            );
        }
        app.driver.resize(width, height);
        app.pending_events.push_back(UiEvent::WindowResized {
            viewport: Viewport::new(f32::from(width), f32::from(height), 1.0),
        });
        QUI_OK
    })
}

/// Advance the app by one non-blocking step: dispatch one queued event (if
/// any; see [`qui_push_event`]/[`qui_push_key_event`]/[`qui_resize`] —
/// events dispatch in the order they were queued) through the app's
/// `handle` callback, otherwise call its `tick` callback, then repaint if
/// the reaction calls for it. Never blocks waiting for input, because there
/// is nothing to wait on in a headless app.
///
/// Returns [`QUI_STEP_CONTINUE`], [`QUI_STEP_EXITED`] (the app exited;
/// further calls are a no-op returning this again), or a negative
/// `QUI_ERR_*` code. A [`QUI_ERR_PANIC`] here is fatal for this handle: the
/// panic unwound out of a half-finished dispatch, so the handle is poisoned
/// and every later call but [`qui_app_free`] returns
/// [`QUI_ERR_POISONED`].
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_step(app: *mut QuiApp) -> i32 {
    const NAME: &str = "qui_step";
    clear_last_error();
    with_app(NAME, app, |app| {
        if app.driver.exited() {
            return QUI_STEP_EXITED;
        }
        // `TuiDriver::dispatch`/`tick` apply the reaction themselves
        // (repainting on redraw, latching `exited` on exit), so the
        // returned reaction is only read to decide what to report back.
        let reaction = match app.pending_events.pop_front() {
            Some(event) => app.driver.dispatch(event),
            None => app.driver.tick(),
        };
        if matches!(reaction, Reaction::Exit) {
            QUI_STEP_EXITED
        } else {
            QUI_STEP_CONTINUE
        }
    })
}

/// Run [`qui_step`] in a loop until the app exits, a step fails, or
/// `max_steps` is reached (whichever comes first): a headless app's stand-in
/// for a real blocking `qui_run` entry point, reduced to "drain the queue,
/// polling `tick` in between", since there is no real event source to block
/// on. `max_steps` is clamped to at least `1`, so `qui_run(app, 0)` still
/// performs one step rather than returning without advancing the app at all.
///
/// Returns the same codes as [`qui_step`], stopping at the first step that
/// does not return [`QUI_STEP_CONTINUE`].
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_run(app: *mut QuiApp, max_steps: u32) -> i32 {
    boundary("qui_run", || {
        let mut last = QUI_STEP_CONTINUE;
        for _ in 0..max_steps.max(1) {
            last = qui_step(app);
            if last != QUI_STEP_CONTINUE {
                return last;
            }
        }
        last
    })
}

/// `true` (`1`) if the app's current rendered frame contains `needle`,
/// `false` (`0`) if not — the assertion a foreign-language caller uses to
/// confirm a frame actually painted an inserted descriptor's text.
///
/// Returns a negative `QUI_ERR_*` code on failure:
/// [`QUI_ERR_NULL_HANDLE`], [`QUI_ERR_NULL_POINTER`],
/// [`QUI_ERR_EMPTY_INPUT`] (an empty needle is rejected rather than
/// trivially "found"), [`QUI_ERR_INVALID_UTF8`], [`QUI_ERR_POISONED`] or
/// [`QUI_ERR_PANIC`].
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`]. `needle` must be
/// either null (rejected as an error) or valid for reads of `needle_len`
/// bytes.
#[no_mangle]
pub unsafe extern "C" fn qui_screen_contains(
    app: *const QuiApp,
    needle: *const u8,
    needle_len: usize,
) -> i32 {
    const NAME: &str = "qui_screen_contains";
    clear_last_error();
    with_app_ref(NAME, app, |app| {
        let text = match borrow_utf8(NAME, "needle", needle, needle_len) {
            Ok(text) => text,
            Err(code) => return code,
        };
        i32::from(app.driver.screen_contains(text))
    })
}

/// Return the app's current rendered frame as a callee-allocated,
/// null-terminated UTF-8 string of newline-separated rows — the diagnostic
/// companion to [`qui_screen_contains`], for printing what was actually
/// painted when an assertion fails.
///
/// The caller owns the returned string and must release it with
/// [`qui_free`], never with `free()`. Returns null on failure
/// ([`QUI_ERR_NULL_HANDLE`], [`QUI_ERR_POISONED`], [`QUI_ERR_PANIC`]), with
/// [`qui_last_error`] set to why.
///
/// # Safety
/// `app` must be a live pointer from [`qui_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_screen(app: *const QuiApp) -> *mut c_char {
    const NAME: &str = "qui_screen";
    clear_last_error();
    with_app_ref(NAME, app, |app| {
        into_owned_c_string(NAME, app.driver.screen())
    })
}

/// Resolve a standard, per-app filesystem location —
/// `quadraui::PlatformServices::app_path` surfaced over FFI. `kind` is one
/// of the `QUI_PATH_*` constants.
///
/// Needs no [`QuiApp`] handle: resolving a standard path needs no window,
/// display server or driver, only the filesystem and environment
/// (`quadraui::paths`'s module doc has the full rationale).
///
/// Returns a caller-owned, null-terminated UTF-8 string the caller must
/// free with [`qui_free`], or null on failure — [`QUI_ERR_UNSUPPORTED`] for
/// a `kind` outside the `QUI_PATH_*` set, or one this platform or build
/// cannot resolve; [`QUI_ERR_INTERNAL`] if the resolved path is not valid
/// UTF-8 (possible on Unix, where paths are arbitrary bytes) rather than
/// silently mangling it with `to_string_lossy` — with [`qui_last_error`]
/// set to why.
#[no_mangle]
pub extern "C" fn qui_app_path(kind: i32) -> *mut c_char {
    const NAME: &str = "qui_app_path";
    clear_last_error();
    boundary(NAME, || {
        let mapped = match kind {
            QUI_PATH_USER_DATA => AppPathKind::UserData,
            QUI_PATH_APP_CONFIG => AppPathKind::AppConfig,
            QUI_PATH_APP_CACHE => AppPathKind::AppCache,
            QUI_PATH_LOGS => AppPathKind::Logs,
            QUI_PATH_TEMP => AppPathKind::Temp,
            QUI_PATH_DOWNLOADS => AppPathKind::Downloads,
            QUI_PATH_DOCUMENTS => AppPathKind::Documents,
            QUI_PATH_HOME => AppPathKind::Home,
            _ => {
                fail(
                    QUI_ERR_UNSUPPORTED,
                    format!("{NAME}: {kind} is not one of the QUI_PATH_* values"),
                );
                return std::ptr::null_mut();
            }
        };
        match paths::resolve(mapped) {
            Ok(path) => match path.to_str() {
                Some(s) => into_owned_c_string(NAME, s.to_owned()),
                None => {
                    fail(
                        QUI_ERR_INTERNAL,
                        format!("{NAME}: {mapped:?} resolved to a non-UTF-8 path"),
                    );
                    std::ptr::null_mut()
                }
            },
            Err(_) => {
                fail(
                    QUI_ERR_UNSUPPORTED,
                    format!("{NAME}: {mapped:?} could not be resolved on this platform or build"),
                );
                std::ptr::null_mut()
            }
        }
    })
}

/// Set the app identifier that scopes [`QUI_PATH_USER_DATA`],
/// [`QUI_PATH_APP_CONFIG`], [`QUI_PATH_APP_CACHE`] and [`QUI_PATH_LOGS`] —
/// `quadraui::paths::set_app_identifier` surfaced over FFI.
///
/// Process-global, not per-[`QuiApp`] handle: call this once at startup,
/// before the first [`qui_app_path`] call that needs it.
///
/// # Safety
/// `ptr` must be either null or valid for reads of `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn qui_set_app_identifier(ptr: *const u8, len: usize) -> i32 {
    const NAME: &str = "qui_set_app_identifier";
    clear_last_error();
    boundary(NAME, || {
        let identifier = match borrow_utf8(NAME, "identifier", ptr, len) {
            Ok(identifier) => identifier,
            Err(code) => return code,
        };
        paths::set_app_identifier(identifier);
        QUI_OK
    })
}

/// Return the error code of the last call on the current thread: [`QUI_OK`]
/// if it succeeded, otherwise the same negative `QUI_ERR_*` code that call
/// returned. Lets a host read the machine-checkable half of the error
/// channel without allocating the message, and recover the code from the
/// calls that can only signal failure by returning null
/// ([`qui_app_new`], [`qui_screen`], [`qui_last_error`]).
///
/// Does not disturb the stored error, so it can be called before or after
/// [`qui_last_error`], in either order, as often as the host likes.
#[no_mangle]
pub extern "C" fn qui_last_error_code() -> i32 {
    boundary("qui_last_error_code", || LAST_CODE.with(Cell::get))
}

/// Return the last error message set by a call on the current thread, as a
/// callee-allocated null-terminated UTF-8 string the caller must free with
/// [`qui_free`] (never with `free()`).
///
/// Returns null if (and only if) there is no error — a message that itself
/// contains an interior NUL (not producible by any error this crate sets
/// today, since they are all built from `Display` output of serde/UTF-8
/// errors and panic payloads) falls back to a fixed placeholder string
/// rather than null, so "no error" is never ambiguous with "there was an
/// error we couldn't represent". Pair it with [`qui_last_error_code`] for
/// the machine-readable half, which is also what to check when this returns
/// null unexpectedly.
///
/// Does not clear the stored error: two calls in a row return two
/// independently owned copies of the same message, each needing its own
/// [`qui_free`].
#[no_mangle]
pub extern "C" fn qui_last_error() -> *mut c_char {
    boundary("qui_last_error", || {
        let Some(message) = LAST_ERROR.with(|cell| cell.borrow().clone()) else {
            return std::ptr::null_mut();
        };
        match CString::new(message) {
            Ok(owned) => owned.into_raw(),
            Err(_) => CString::new("<error message contained an interior NUL>")
                .expect("the placeholder has no interior NUL")
                .into_raw(),
        }
    })
}

/// Return the spelling of an error code — `"QUI_ERR_INVALID_JSON"` for
/// [`QUI_ERR_INVALID_JSON`], and so on — so a host can log or assert on a
/// code without hard-coding a parallel table of names.
///
/// The returned pointer is static storage owned by this crate, valid for the
/// life of the process: the caller must **not** free it, and must not pass
/// it to [`qui_free`]. Returns null for a code this crate does not define.
/// Leaves the last error untouched.
#[no_mangle]
pub extern "C" fn qui_error_name(code: i32) -> *const c_char {
    boundary("qui_error_name", || {
        // `QUI_STEP_CONTINUE` is `QUI_OK` and so is spelled by its `QUI_OK`
        // name here; `QUI_STEP_EXITED` is a step result rather than an
        // error, but naming it keeps every value `qui_step` can return
        // printable.
        let name: &'static CStr = match code {
            QUI_OK => c"QUI_OK",
            QUI_STEP_EXITED => c"QUI_STEP_EXITED",
            QUI_ERR_NULL_HANDLE => c"QUI_ERR_NULL_HANDLE",
            QUI_ERR_NULL_POINTER => c"QUI_ERR_NULL_POINTER",
            QUI_ERR_EMPTY_INPUT => c"QUI_ERR_EMPTY_INPUT",
            QUI_ERR_INVALID_UTF8 => c"QUI_ERR_INVALID_UTF8",
            QUI_ERR_INVALID_JSON => c"QUI_ERR_INVALID_JSON",
            QUI_ERR_SLOT_OCCUPIED => c"QUI_ERR_SLOT_OCCUPIED",
            QUI_ERR_UNKNOWN_SLOT => c"QUI_ERR_UNKNOWN_SLOT",
            QUI_ERR_INVALID_DIMENSIONS => c"QUI_ERR_INVALID_DIMENSIONS",
            QUI_ERR_INVALID_CHAR => c"QUI_ERR_INVALID_CHAR",
            QUI_ERR_PANIC => c"QUI_ERR_PANIC",
            QUI_ERR_POISONED => c"QUI_ERR_POISONED",
            QUI_ERR_INTERNAL => c"QUI_ERR_INTERNAL",
            QUI_ERR_UNSUPPORTED => c"QUI_ERR_UNSUPPORTED",
            QUI_ERR_IO => c"QUI_ERR_IO",
            _ => return std::ptr::null(),
        };
        name.as_ptr()
    })
}

/// Free a string this crate allocated for the caller: the message from
/// [`qui_last_error`] or the frame dump from [`qui_screen`]. `ptr` may be
/// null (no-op). Leaves the last error untouched.
///
/// This is the *only* correct deallocator for those two strings — they come
/// from Rust's allocator, so calling `free()` on one is undefined behaviour.
/// Conversely, never pass this function anything else: not a pointer the
/// caller allocated, not the static string from [`qui_error_name`], and not
/// a pointer already freed.
///
/// # Safety
/// `ptr` must be either null or a pointer previously returned by
/// [`qui_last_error`] or [`qui_screen`], not already freed.
#[no_mangle]
pub unsafe extern "C" fn qui_free(ptr: *mut c_char) {
    boundary("qui_free", || {
        if ptr.is_null() {
            return;
        }
        drop(CString::from_raw(ptr));
    });
}

// ─── Real-terminal spike: `QuiRtApp` ────────────────────────────────────────
//
// Everything above runs the headless `TuiDriver` — no terminal, fixed slots,
// one descriptor per primitive type. `QuiRtApp` is a narrow, additive spike
// proving out the other half: a real terminal driven over `TuiRunner`, a
// handle-addressed store so a descriptor can be updated in place instead of
// replaced wholesale, and hit-tested semantic events (a `ListView` row
// selected/activated) instead of raw mouse coordinates. It covers exactly
// one primitive (`ListView`) end to end; see `SPIKE_FINDINGS.md` for the
// go/no-go verdict this was built to answer.

/// The [`AppLogic`] `QuiRtApp` drives: a handle-addressed store of
/// [`ListView`] descriptors (only the most recently inserted one is
/// painted/hit-tested — see `active`), the host's callbacks, and the last
/// rect the active list was painted into (needed because [`AppLogic::render`]
/// takes `&self`: hit-testing in `handle` has to recover where things were
/// drawn without being able to recompute it from scratch).
struct FfiRtApp {
    store: DescriptorStore<ListView>,
    /// `handles[id as usize]` is the internal store handle for ffi id `id`.
    /// Ids are assigned densely by insertion order; there is no removal in
    /// this spike, so the index never needs reuse bookkeeping.
    handles: Vec<Handle<ListView>>,
    /// The ffi id of the list currently painted and hit-tested. `None`
    /// until the first insert.
    active: Option<i64>,
    /// The rect `render` last painted the active list into, in viewport
    /// coordinates. `handle` translates a `MouseDown` position into this
    /// rect's local space before hit-testing.
    last_rect: Cell<Option<Rect>>,
    vtable: QuiVTable,
}

impl FfiRtApp {
    fn new(vtable: QuiVTable) -> Self {
        Self {
            store: DescriptorStore::new(),
            handles: Vec::new(),
            active: None,
            last_rect: Cell::new(None),
            vtable,
        }
    }

    /// Forwards a raw, serialized `event` to the host's `handle` callback
    /// unchanged — the fallback path for anything `handle` doesn't turn
    /// into a semantic event itself.
    fn forward_raw(&self, event: &UiEvent) -> Reaction {
        let Some(handle_fn) = self.vtable.handle else {
            return Reaction::Continue;
        };
        let json = match serde_json::to_string(event) {
            Ok(json) => json,
            Err(err) => {
                set_last_error(
                    QUI_ERR_INTERNAL,
                    format!("failed to serialize UiEvent to JSON: {err}"),
                );
                return Reaction::Continue;
            }
        };
        let code = unsafe { handle_fn(self.vtable.user_data, json.as_ptr(), json.len()) };
        reaction_from_code(code)
    }

    /// Sends a semantic (non-`UiEvent`) payload to the host's `handle`
    /// callback, e.g. `{"type":"list_item_activated",...}`.
    fn emit_semantic(&self, payload: &serde_json::Value) -> Reaction {
        let Some(handle_fn) = self.vtable.handle else {
            return Reaction::Continue;
        };
        let json = payload.to_string();
        let code = unsafe { handle_fn(self.vtable.user_data, json.as_ptr(), json.len()) };
        reaction_from_code(code)
    }

    /// Hit-tests a `MouseDown` at `position` (viewport coordinates) against
    /// the active list's last-painted rect. Returns `None` (meaning: fall
    /// back to forwarding the raw event) when there is no active list, no
    /// rect has been painted yet, or the click landed on the title/empty
    /// area rather than an item row.
    fn handle_mouse_down(&mut self, position: Point) -> Option<Reaction> {
        let rect = self.last_rect.get()?;
        let id = self.active?;
        let handle = *self.handles.get(id as usize)?;
        let hit = {
            let list = self.store.get(handle)?;
            let local_x = position.x - rect.x;
            let local_y = position.y - rect.y;
            let layout = list.layout(rect.width, rect.height, 0.0, |_| ListItemMeasure::new(1.0));
            layout.hit_test(local_x, local_y)
        };
        match hit {
            ListViewHit::Item(idx) => {
                self.store.update(handle, |list| list.selected_idx = idx);
                let payload = serde_json::json!({
                    "type": "list_item_activated",
                    "handle": id,
                    "idx": idx,
                });
                Some(self.emit_semantic(&payload))
            }
            ListViewHit::Title | ListViewHit::Empty => None,
        }
    }

    /// Handles Up/Down/Enter against the active list's selection directly,
    /// without a round trip through the host: Up/Down move `selected_idx`
    /// in place and ask for a redraw; Enter emits the same semantic
    /// `list_item_activated` payload a mouse click on the selected row
    /// would. Any other key falls back to forwarding the raw event.
    fn handle_key(&mut self, key: &Key) -> Option<Reaction> {
        let id = self.active?;
        let handle = *self.handles.get(id as usize)?;
        match key {
            Key::Named(NamedKey::Up) => {
                self.store.update(handle, |list| {
                    if list.selected_idx > 0 {
                        list.selected_idx -= 1;
                    }
                });
                Some(Reaction::Redraw)
            }
            Key::Named(NamedKey::Down) => {
                self.store.update(handle, |list| {
                    let len = list.items.len();
                    if len > 0 && list.selected_idx + 1 < len {
                        list.selected_idx += 1;
                    }
                });
                Some(Reaction::Redraw)
            }
            Key::Named(NamedKey::Enter) => {
                let idx = self.store.get(handle)?.selected_idx;
                let payload = serde_json::json!({
                    "type": "list_item_activated",
                    "handle": id,
                    "idx": idx,
                });
                Some(self.emit_semantic(&payload))
            }
            _ => None,
        }
    }
}

impl AppLogic for FfiRtApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        self.last_rect.set(Some(rect));
        let Some(id) = self.active else {
            return;
        };
        let Some(handle) = self.handles.get(id as usize) else {
            return;
        };
        if let Some(list) = self.store.get(*handle) {
            backend.draw_list(rect, list);
        }
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let semantic = match &event {
            UiEvent::MouseDown { position, .. } => self.handle_mouse_down(*position),
            UiEvent::KeyPressed { key, .. } => self.handle_key(key),
            _ => None,
        };
        semantic.unwrap_or_else(|| self.forward_raw(&event))
    }

    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        let Some(tick_fn) = self.vtable.tick else {
            return Reaction::Continue;
        };
        let code = unsafe { tick_fn(self.vtable.user_data) };
        reaction_from_code(code)
    }
}

/// Opaque handle to one real-terminal app driven over [`TuiRunner`]. Never
/// constructed or inspected by the caller; only ever passed back into this
/// crate's `qui_rt_*` functions.
pub struct QuiRtApp {
    runner: TuiRunner<FfiRtApp>,
    /// Set when a panic was caught while this handle was borrowed; see
    /// [`QuiApp::poisoned`] for the equivalent on the headless handle.
    poisoned: bool,
}

/// [`with_app`] for [`QuiRtApp`].
///
/// # Safety
/// `app` must be either null or a live pointer from [`qui_rt_app_new`].
unsafe fn with_rt_app<T: AbiResult>(
    name: &str,
    app: *mut QuiRtApp,
    body: impl FnOnce(&mut QuiRtApp) -> T,
) -> T {
    let Some(borrowed) = app.as_mut() else {
        return T::from_error(fail(
            QUI_ERR_NULL_HANDLE,
            format!("{name}: app handle is null"),
        ));
    };
    if borrowed.poisoned {
        return T::from_error(fail(
            QUI_ERR_POISONED,
            format!(
                "{name}: app handle is poisoned by an earlier caught panic; free it with qui_rt_app_free"
            ),
        ));
    }
    match panic::catch_unwind(AssertUnwindSafe(|| body(borrowed))) {
        Ok(value) => value,
        Err(payload) => {
            (*app).poisoned = true;
            T::from_error(record_panic(name, &*payload))
        }
    }
}

/// Create a new app that negotiates and owns a real terminal (raw mode,
/// mouse capture, alternate screen) for the lifetime of the handle.
///
/// Returns null on failure, with [`qui_last_error_code`] set to
/// [`QUI_ERR_IO`] if negotiating the terminal failed (not a TTY, no
/// permission, …) or [`QUI_ERR_PANIC`] if construction itself panicked.
///
/// The returned handle is owned by the caller and must be released with
/// [`qui_rt_app_free`], never with `free()`. Releasing it restores the
/// terminal to its state before this call.
///
/// # Safety
/// `vtable`'s `user_data` is an opaque pointer this crate never
/// dereferences; the caller is responsible for its lifetime outliving every
/// call made with the returned [`QuiRtApp`] handle.
#[no_mangle]
pub unsafe extern "C" fn qui_rt_app_new(vtable: QuiVTable) -> *mut QuiRtApp {
    const NAME: &str = "qui_rt_app_new";
    clear_last_error();
    boundary(NAME, || {
        let app = FfiRtApp::new(vtable);
        match TuiRunner::new(app) {
            Ok(runner) => Box::into_raw(Box::new(QuiRtApp {
                runner,
                poisoned: false,
            })),
            Err(err) => {
                fail(
                    QUI_ERR_IO,
                    format!("{NAME}: failed to open terminal: {err}"),
                );
                std::ptr::null_mut()
            }
        }
    })
}

/// Destroy an app created by [`qui_rt_app_new`], restoring the terminal.
/// `app` may be null (no-op). Works even on a handle poisoned by a caught
/// panic — this is the one function that does.
///
/// Leaves the last error untouched, so a host can free the app on its error
/// path and still report what failed afterwards.
///
/// # Safety
/// `app` must be either null or a still-live pointer returned by
/// [`qui_rt_app_new`] that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn qui_rt_app_free(app: *mut QuiRtApp) {
    boundary("qui_rt_app_free", || {
        if app.is_null() {
            return;
        }
        drop(Box::from_raw(app));
    });
}

/// Insert a new [`ListView`] descriptor, decoded from `json`, into the
/// app's store, make it the active (painted/hit-tested) list, and request a
/// repaint. Returns the new descriptor's non-negative handle id on success —
/// pass it back to [`qui_rt_list_update`] to mutate it in place — or a
/// negative `QUI_ERR_*` code on failure.
///
/// Unlike the headless `qui_list_insert`, there is no slot to occupy:
/// inserting again just adds another descriptor and makes it the active
/// one, so many lists can be held at once even though only one is painted.
///
/// # Safety
/// `app` must be a live pointer from [`qui_rt_app_new`]; `(json, json_len)`
/// must name `json_len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn qui_rt_list_insert(
    app: *mut QuiRtApp,
    json: *const u8,
    json_len: usize,
) -> i64 {
    const NAME: &str = "qui_rt_list_insert";
    clear_last_error();
    with_rt_app(NAME, app, |app| {
        let text = match borrow_utf8(NAME, "json", json, json_len) {
            Ok(text) => text,
            Err(code) => return i64::from(code),
        };
        let list: ListView = match serde_json::from_str(text) {
            Ok(list) => list,
            Err(err) => {
                return fail_i64(
                    QUI_ERR_INVALID_JSON,
                    format!("{NAME}: json does not decode to a ListView: {err}"),
                );
            }
        };
        let rt_app = app.runner.app_mut();
        let handle = rt_app.store.insert(list);
        let id = rt_app.handles.len() as i64;
        rt_app.handles.push(handle);
        rt_app.active = Some(id);
        app.runner.backend_mut().request_full_repaint();
        id
    })
}

/// Replace the [`ListView`] named by `handle_id` (the id [`qui_rt_list_insert`]
/// returned) with the descriptor decoded from `json`, in place, and request
/// a repaint. This is the operation the headless ABI cannot do at all: there,
/// updating means clearing the slot and inserting again, losing the
/// old descriptor's identity; here the store handle — and so anything a host
/// keyed off it — stays valid across the update.
///
/// Returns [`QUI_OK`] on success, [`QUI_ERR_UNKNOWN_SLOT`] if `handle_id`
/// does not name a descriptor this app holds, or another negative
/// `QUI_ERR_*` code on failure.
///
/// # Safety
/// `app` must be a live pointer from [`qui_rt_app_new`]; `(json, json_len)`
/// must name `json_len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn qui_rt_list_update(
    app: *mut QuiRtApp,
    handle_id: i64,
    json: *const u8,
    json_len: usize,
) -> i32 {
    const NAME: &str = "qui_rt_list_update";
    clear_last_error();
    with_rt_app(NAME, app, |app| {
        let text = match borrow_utf8(NAME, "json", json, json_len) {
            Ok(text) => text,
            Err(code) => return code,
        };
        let list: ListView = match serde_json::from_str(text) {
            Ok(list) => list,
            Err(err) => {
                return fail(
                    QUI_ERR_INVALID_JSON,
                    format!("{NAME}: json does not decode to a ListView: {err}"),
                );
            }
        };
        let rt_app = app.runner.app_mut();
        let Ok(index) = usize::try_from(handle_id) else {
            return fail(
                QUI_ERR_UNKNOWN_SLOT,
                format!("{NAME}: handle {handle_id} is negative"),
            );
        };
        let Some(&handle) = rt_app.handles.get(index) else {
            return fail(
                QUI_ERR_UNKNOWN_SLOT,
                format!("{NAME}: handle {handle_id} does not name a descriptor this app holds"),
            );
        };
        if !rt_app.store.update(handle, |existing| *existing = list) {
            return fail(
                QUI_ERR_UNKNOWN_SLOT,
                format!("{NAME}: handle {handle_id} was removed"),
            );
        }
        app.runner.backend_mut().request_full_repaint();
        QUI_OK
    })
}

/// Advance the app by polling the real terminal for exactly one native
/// event (input, resize, …) without blocking, dispatch it if one arrived,
/// and repaint if needed.
///
/// Returns [`QUI_STEP_CONTINUE`], [`QUI_STEP_EXITED`] (further calls are a
/// no-op returning this again), or [`QUI_ERR_IO`] if polling the terminal
/// failed.
///
/// # Safety
/// `app` must be a live pointer from [`qui_rt_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_rt_step(app: *mut QuiRtApp) -> i32 {
    const NAME: &str = "qui_rt_step";
    clear_last_error();
    with_rt_app(NAME, app, |app| match app.runner.step() {
        Ok(StepOutcome::Exited) => QUI_STEP_EXITED,
        Ok(_) => QUI_STEP_CONTINUE,
        Err(err) => fail(QUI_ERR_IO, format!("{NAME}: {err}")),
    })
}

/// Like [`qui_rt_step`], but blocks up to `timeout_ms` milliseconds waiting
/// for the first native event to arrive, instead of returning immediately
/// when there is nothing to dispatch. This is the entry point a host's own
/// blocking event loop should call at whatever cadence suits it.
///
/// # Safety
/// `app` must be a live pointer from [`qui_rt_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_rt_pump(app: *mut QuiRtApp, timeout_ms: u32) -> i32 {
    const NAME: &str = "qui_rt_pump";
    clear_last_error();
    with_rt_app(NAME, app, |app| {
        match app
            .runner
            .pump(Duration::from_millis(u64::from(timeout_ms)))
        {
            Ok(StepOutcome::Exited) => QUI_STEP_EXITED,
            Ok(_) => QUI_STEP_CONTINUE,
            Err(err) => fail(QUI_ERR_IO, format!("{NAME}: {err}")),
        }
    })
}

/// Run [`qui_rt_pump`] in a loop until the app exits, a step fails, or
/// `max_steps` is reached, blocking up to `timeout_ms` per step. `max_steps`
/// is clamped to at least `1`.
///
/// # Safety
/// `app` must be a live pointer from [`qui_rt_app_new`].
#[no_mangle]
pub unsafe extern "C" fn qui_rt_run(app: *mut QuiRtApp, timeout_ms: u32, max_steps: u32) -> i32 {
    boundary("qui_rt_run", || {
        let mut last = QUI_STEP_CONTINUE;
        for _ in 0..max_steps.max(1) {
            last = qui_rt_pump(app, timeout_ms);
            if last != QUI_STEP_CONTINUE {
                return last;
            }
        }
        last
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// When set, [`FfiApp::render`] panics. Lets a test drive a real
        /// unwind out of quadraui's own render path and observe the ABI
        /// boundary catching it. Always reset before the test returns, since
        /// the test harness reuses threads.
        pub(crate) static PANIC_IN_RENDER: Cell<bool> = const { Cell::new(false) };
    }

    /// Runs `body` with [`PANIC_IN_RENDER`] set, clearing it afterwards even
    /// if `body` itself unwinds.
    fn with_render_panics<T>(body: impl FnOnce() -> T) -> T {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                PANIC_IN_RENDER.with(|flag| flag.set(false));
            }
        }
        PANIC_IN_RENDER.with(|flag| flag.set(true));
        let _reset = Reset;
        body()
    }

    /// Takes the current thread's last error message, asserting there is one.
    unsafe fn take_last_error() -> String {
        let ptr = qui_last_error();
        assert!(!ptr.is_null(), "expected a last-error message");
        let message = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        qui_free(ptr);
        message
    }

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
            assert_eq!(inserted, i64::from(QUI_SLOT_BUTTON));
            assert_eq!(qui_last_error_code(), QUI_OK);
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
            assert_eq!(inserted, i64::from(QUI_ERR_INVALID_JSON));
            assert_eq!(qui_last_error_code(), QUI_ERR_INVALID_JSON);

            let message = take_last_error();
            assert!(message.contains("invalid Button JSON"));

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
            assert_eq!(inserted, i64::from(QUI_SLOT_TEXT_DISPLAY));
            assert!(qui_last_error().is_null());
            assert_eq!(qui_screen_contains(app, b"Hello".as_ptr(), 5), 1);

            // Each primitive type has exactly one slot, so a second insert
            // into an occupied one is an error rather than a replacement.
            let again = qui_text_display_insert(app, json.as_ptr(), json.len());
            assert_eq!(again, i64::from(QUI_ERR_SLOT_OCCUPIED));
            let message = take_last_error();
            assert!(message.contains("TextDisplay"), "{message}");

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
            assert_eq!(inserted, i64::from(QUI_SLOT_LIST));
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
            assert_eq!(inserted, i64::from(QUI_SLOT_TREE));
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
            assert_eq!(inserted, i64::from(QUI_SLOT_DATA_TABLE));
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
            assert_eq!(inserted, i64::from(QUI_SLOT_TAB_BAR));
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
                assert_eq!(
                    inserted,
                    i64::from(QUI_ERR_INVALID_JSON),
                    "{name} should reject non-JSON input"
                );
                let message = take_last_error();
                assert!(
                    message.contains("invalid"),
                    "{name} should describe the decode failure, got {message}"
                );
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
            assert_eq!(
                qui_push_event(app, bad.as_ptr(), bad.len()),
                QUI_ERR_INVALID_JSON
            );
            let message = take_last_error();
            assert!(message.contains("invalid UiEvent JSON"));

            assert_eq!(
                qui_push_event(std::ptr::null_mut(), bad.as_ptr(), bad.len()),
                QUI_ERR_NULL_HANDLE
            );
            assert_eq!(qui_last_error_code(), QUI_ERR_NULL_HANDLE);
            assert!(!qui_last_error().is_null());

            assert_eq!(
                qui_push_event(app, std::ptr::null(), 4),
                QUI_ERR_NULL_POINTER
            );
            assert_eq!(qui_last_error_code(), QUI_ERR_NULL_POINTER);

            assert_eq!(qui_push_event(app, bad.as_ptr(), 0), QUI_ERR_EMPTY_INPUT);
            assert_eq!(qui_last_error_code(), QUI_ERR_EMPTY_INPUT);

            let invalid_utf8 = [0xffu8, 0xfe, 0xfd];
            assert_eq!(
                qui_push_event(app, invalid_utf8.as_ptr(), invalid_utf8.len()),
                QUI_ERR_INVALID_UTF8
            );
            assert_eq!(qui_last_error_code(), QUI_ERR_INVALID_UTF8);

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
                QUI_ERR_INVALID_JSON
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

            assert_eq!(qui_resize(app, 0, 5), QUI_ERR_INVALID_DIMENSIONS);
            assert!(!qui_last_error().is_null());
            assert_eq!(qui_resize(app, 5, 0), QUI_ERR_INVALID_DIMENSIONS);
            assert!(!qui_last_error().is_null());
            assert_eq!(qui_resize(std::ptr::null_mut(), 5, 5), QUI_ERR_NULL_HANDLE);
            assert_eq!(qui_last_error_code(), QUI_ERR_NULL_HANDLE);

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

    #[test]
    fn qui_app_new_rejects_zero_dimensions() {
        unsafe {
            for (width, height) in [(0u16, 5u16), (5, 0), (0, 0)] {
                let app = qui_app_new(width, height, no_op_vtable());
                assert!(app.is_null(), "{width}x{height} should be rejected");
                assert_eq!(qui_last_error_code(), QUI_ERR_INVALID_DIMENSIONS);
                assert!(take_last_error().contains("non-zero"));
            }
        }
    }

    /// Every slot id a caller can name is one of the `QUI_SLOT_*` constants;
    /// anything else is reported as such rather than silently ignored.
    #[test]
    fn unknown_slot_ids_are_rejected() {
        unsafe {
            let app = qui_app_new(20, 12, no_op_vtable());
            assert!(!app.is_null());

            for slot in [-1, 6, 99] {
                assert_eq!(qui_slot_clear(app, slot), QUI_ERR_UNKNOWN_SLOT);
                assert_eq!(qui_last_error_code(), QUI_ERR_UNKNOWN_SLOT);
                assert!(take_last_error().contains("unknown slot id"));

                let json = button_json();
                let inserted =
                    insert_descriptor("qui_test_insert", slot, app, json.as_ptr(), json.len());
                assert_eq!(inserted, i64::from(QUI_ERR_UNKNOWN_SLOT));
                assert_eq!(qui_last_error_code(), QUI_ERR_UNKNOWN_SLOT);
            }

            qui_app_free(app);
        }
    }

    /// The ownership story for a slot: an insert fills it, a clear empties it
    /// and repaints, clearing an already-empty slot is a no-op rather than an
    /// error, and the emptied slot accepts a different descriptor afterwards.
    #[test]
    fn qui_slot_clear_round_trips_and_is_idempotent() {
        unsafe {
            let app = qui_app_new(40, 12, no_op_vtable());
            assert!(!app.is_null());

            let first = button_json();
            assert_eq!(
                qui_button_insert(app, first.as_ptr(), first.len()),
                i64::from(QUI_SLOT_BUTTON)
            );
            assert_eq!(qui_screen_contains(app, b"OK".as_ptr(), 2), 1);

            assert_eq!(qui_slot_clear(app, QUI_SLOT_BUTTON), QUI_OK);
            assert_eq!(qui_last_error_code(), QUI_OK);
            assert_eq!(qui_screen_contains(app, b"OK".as_ptr(), 2), 0);

            // Clearing an empty slot is deliberately not an error.
            assert_eq!(qui_slot_clear(app, QUI_SLOT_BUTTON), QUI_OK);

            let second = serde_json::json!({ "id": "demo:redo", "label": "Redo" }).to_string();
            assert_eq!(
                qui_button_insert(app, second.as_ptr(), second.len()),
                i64::from(QUI_SLOT_BUTTON)
            );
            assert_eq!(qui_screen_contains(app, b"Redo".as_ptr(), 4), 1);

            qui_app_free(app);
        }
    }

    #[test]
    fn qui_push_key_event_rejects_a_non_scalar_code_point() {
        unsafe {
            let app = qui_app_new(10, 3, no_op_vtable());
            assert!(!app.is_null());

            // A lone UTF-16 surrogate and an out-of-range code point are both
            // outside Unicode's scalar values, so neither names a `char`.
            for ch in [0xD800u32, 0x11_0000] {
                assert_eq!(qui_push_key_event(app, ch), QUI_ERR_INVALID_CHAR);
                assert_eq!(qui_last_error_code(), QUI_ERR_INVALID_CHAR);
                assert!(take_last_error().contains("Unicode scalar value"));
            }

            assert_eq!(qui_push_key_event(app, 'z' as u32), QUI_OK);
            assert_eq!(
                qui_push_key_event(std::ptr::null_mut(), 'z' as u32),
                QUI_ERR_NULL_HANDLE
            );

            qui_app_free(app);
        }
    }

    #[test]
    fn qui_screen_contains_rejects_bad_needles() {
        unsafe {
            let app = qui_app_new(20, 6, no_op_vtable());
            assert!(!app.is_null());

            let needle = b"OK";
            assert_eq!(
                qui_screen_contains(app, needle.as_ptr(), 0),
                QUI_ERR_EMPTY_INPUT
            );
            assert_eq!(qui_last_error_code(), QUI_ERR_EMPTY_INPUT);

            assert_eq!(
                qui_screen_contains(app, std::ptr::null(), 2),
                QUI_ERR_NULL_POINTER
            );
            assert_eq!(qui_last_error_code(), QUI_ERR_NULL_POINTER);

            let invalid = [0xffu8, 0xfe];
            assert_eq!(
                qui_screen_contains(app, invalid.as_ptr(), invalid.len()),
                QUI_ERR_INVALID_UTF8
            );
            assert_eq!(qui_last_error_code(), QUI_ERR_INVALID_UTF8);

            assert_eq!(
                qui_screen_contains(std::ptr::null(), needle.as_ptr(), needle.len()),
                QUI_ERR_NULL_HANDLE
            );
            assert_eq!(qui_last_error_code(), QUI_ERR_NULL_HANDLE);

            qui_app_free(app);
        }
    }

    #[test]
    fn qui_screen_hands_out_a_freeable_frame_dump() {
        unsafe {
            let app = qui_app_new(40, 6, no_op_vtable());
            assert!(!app.is_null());
            let json = button_json();
            assert_eq!(
                qui_button_insert(app, json.as_ptr(), json.len()),
                i64::from(QUI_SLOT_BUTTON)
            );

            let dump = qui_screen(app);
            assert!(!dump.is_null());
            assert_eq!(qui_last_error_code(), QUI_OK);
            let text = CStr::from_ptr(dump).to_string_lossy().into_owned();
            assert!(text.contains("OK"), "frame dump was {text:?}");
            qui_free(dump);

            assert!(qui_screen(std::ptr::null()).is_null());
            assert_eq!(qui_last_error_code(), QUI_ERR_NULL_HANDLE);

            qui_app_free(app);
        }
    }

    /// Both halves of the error channel stay in step: a failure sets a code
    /// and a message, a later success clears both, and the message can be
    /// taken as many times as the caller likes — each copy independently
    /// owned and independently freed.
    #[test]
    fn the_error_channel_is_set_on_failure_and_cleared_on_success() {
        unsafe {
            let app = qui_app_new(40, 6, no_op_vtable());
            assert!(!app.is_null());

            let bad = b"{";
            assert_eq!(
                qui_button_insert(app, bad.as_ptr(), bad.len()),
                i64::from(QUI_ERR_INVALID_JSON)
            );
            let first = take_last_error();
            let second = take_last_error();
            assert_eq!(first, second);
            assert_eq!(qui_last_error_code(), QUI_ERR_INVALID_JSON);

            let json = button_json();
            assert_eq!(
                qui_button_insert(app, json.as_ptr(), json.len()),
                i64::from(QUI_SLOT_BUTTON)
            );
            assert_eq!(qui_last_error_code(), QUI_OK);
            assert!(qui_last_error().is_null());

            // Freeing null is a documented no-op.
            qui_free(std::ptr::null_mut());

            qui_app_free(app);
        }
    }

    #[test]
    fn qui_error_name_spells_every_defined_code() {
        let expected = [
            (QUI_OK, "QUI_OK"),
            (QUI_STEP_EXITED, "QUI_STEP_EXITED"),
            (QUI_ERR_NULL_HANDLE, "QUI_ERR_NULL_HANDLE"),
            (QUI_ERR_NULL_POINTER, "QUI_ERR_NULL_POINTER"),
            (QUI_ERR_EMPTY_INPUT, "QUI_ERR_EMPTY_INPUT"),
            (QUI_ERR_INVALID_UTF8, "QUI_ERR_INVALID_UTF8"),
            (QUI_ERR_INVALID_JSON, "QUI_ERR_INVALID_JSON"),
            (QUI_ERR_SLOT_OCCUPIED, "QUI_ERR_SLOT_OCCUPIED"),
            (QUI_ERR_UNKNOWN_SLOT, "QUI_ERR_UNKNOWN_SLOT"),
            (QUI_ERR_INVALID_DIMENSIONS, "QUI_ERR_INVALID_DIMENSIONS"),
            (QUI_ERR_INVALID_CHAR, "QUI_ERR_INVALID_CHAR"),
            (QUI_ERR_PANIC, "QUI_ERR_PANIC"),
            (QUI_ERR_POISONED, "QUI_ERR_POISONED"),
            (QUI_ERR_INTERNAL, "QUI_ERR_INTERNAL"),
            (QUI_ERR_UNSUPPORTED, "QUI_ERR_UNSUPPORTED"),
            (QUI_ERR_IO, "QUI_ERR_IO"),
        ];
        for (code, name) in expected {
            let ptr = qui_error_name(code);
            assert!(!ptr.is_null(), "no name for code {code}");
            let spelled = unsafe { CStr::from_ptr(ptr) };
            assert_eq!(spelled.to_str().expect("names are ASCII"), name);
        }
        assert!(qui_error_name(-999).is_null());
        assert!(qui_error_name(12345).is_null());
    }

    /// Every `QUI_PATH_*` kind resolves to a non-empty, caller-freeable
    /// path on this host, mirroring `quadraui`'s own
    /// `paths::tests::every_kind_resolves_to_a_non_empty_path`.
    #[test]
    fn qui_app_path_resolves_every_known_kind() {
        for kind in [
            QUI_PATH_USER_DATA,
            QUI_PATH_APP_CONFIG,
            QUI_PATH_APP_CACHE,
            QUI_PATH_LOGS,
            QUI_PATH_TEMP,
            QUI_PATH_DOWNLOADS,
            QUI_PATH_DOCUMENTS,
            QUI_PATH_HOME,
        ] {
            clear_last_error();
            let ptr = qui_app_path(kind);
            assert!(!ptr.is_null(), "kind {kind} should resolve on this host");
            let resolved = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
            assert!(
                !resolved.is_empty(),
                "kind {kind} resolved to an empty path"
            );
            unsafe { qui_free(ptr) };
        }
    }

    #[test]
    fn qui_app_path_rejects_an_unknown_kind() {
        clear_last_error();
        let ptr = qui_app_path(999);
        assert!(ptr.is_null());
        assert_eq!(qui_last_error_code(), QUI_ERR_UNSUPPORTED);
    }

    #[test]
    fn qui_set_app_identifier_rejects_a_null_pointer() {
        clear_last_error();
        let code = unsafe { qui_set_app_identifier(std::ptr::null(), 5) };
        assert_eq!(code, QUI_ERR_NULL_POINTER);
        assert_eq!(qui_last_error_code(), QUI_ERR_NULL_POINTER);
    }

    /// Setting a new identifier moves `QUI_PATH_USER_DATA`/`QUI_PATH_APP_CONFIG`
    /// to a different path, mirroring `quadraui`'s own
    /// `paths::tests::app_identifier_scopes_user_data_and_config`. No other
    /// test in this module touches the process-global identifier, so this
    /// one is safe to run alongside them without its own lock.
    #[test]
    fn qui_set_app_identifier_scopes_user_data_and_config() {
        unsafe fn resolved(kind: i32) -> String {
            clear_last_error();
            let ptr = qui_app_path(kind);
            assert!(!ptr.is_null());
            let text = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            qui_free(ptr);
            text
        }

        unsafe {
            let one = b"quadraui-ffi-test-one";
            assert_eq!(qui_set_app_identifier(one.as_ptr(), one.len()), QUI_OK);
            let data_one = resolved(QUI_PATH_USER_DATA);
            let config_one = resolved(QUI_PATH_APP_CONFIG);

            let two = b"quadraui-ffi-test-two";
            assert_eq!(qui_set_app_identifier(two.as_ptr(), two.len()), QUI_OK);
            let data_two = resolved(QUI_PATH_USER_DATA);
            let config_two = resolved(QUI_PATH_APP_CONFIG);

            assert_ne!(data_one, data_two);
            assert_ne!(config_one, config_two);

            paths::clear_app_identifier();
        }
    }

    /// Dispatching an event repaints when the host's `handle` asks for a
    /// redraw, which is what lets this test drive a real unwind out of
    /// quadraui's render path and observe the boundary guard catching it
    /// instead of letting it cross into C.
    unsafe extern "C" fn redraw_handle(
        _user_data: *mut c_void,
        _json: *const u8,
        _len: usize,
    ) -> i32 {
        1
    }

    #[test]
    fn a_panic_in_a_step_is_caught_and_poisons_the_handle() {
        let vtable = QuiVTable {
            user_data: std::ptr::null_mut(),
            handle: Some(redraw_handle),
            tick: None,
        };
        unsafe {
            let app = qui_app_new(40, 6, vtable);
            assert!(!app.is_null());
            assert_eq!(qui_push_key_event(app, 'x' as u32), QUI_OK);

            let stepped = with_render_panics(|| qui_step(app));
            assert_eq!(stepped, QUI_ERR_PANIC);
            assert_eq!(qui_last_error_code(), QUI_ERR_PANIC);
            let message = take_last_error();
            assert!(message.contains("qui_step"), "message was {message:?}");
            assert!(
                message.contains("injected panic in FfiApp::render"),
                "message was {message:?}"
            );

            // The unwind left the app half-dispatched, so the handle is
            // poisoned: every entry point but `qui_app_free` refuses it.
            assert_eq!(qui_step(app), QUI_ERR_POISONED);
            assert_eq!(qui_last_error_code(), QUI_ERR_POISONED);
            assert_eq!(qui_push_key_event(app, 'y' as u32), QUI_ERR_POISONED);
            assert_eq!(qui_resize(app, 10, 4), QUI_ERR_POISONED);
            assert_eq!(qui_slot_clear(app, QUI_SLOT_BUTTON), QUI_ERR_POISONED);
            assert_eq!(
                qui_screen_contains(app, b"OK".as_ptr(), 2),
                QUI_ERR_POISONED
            );
            assert!(qui_screen(app).is_null());
            let json = button_json();
            assert_eq!(
                qui_button_insert(app, json.as_ptr(), json.len()),
                i64::from(QUI_ERR_POISONED)
            );

            // Freeing a poisoned handle still works, so a host can always
            // clean up after a caught panic.
            qui_app_free(app);
        }
    }

    /// The repaint an insert performs is inside the guard too, so a panic
    /// there is reported rather than unwinding through the `extern "C"`
    /// frame.
    #[test]
    fn a_panic_during_an_insert_repaint_is_caught() {
        unsafe {
            let app = qui_app_new(40, 6, no_op_vtable());
            assert!(!app.is_null());

            let json = button_json();
            let inserted = with_render_panics(|| qui_button_insert(app, json.as_ptr(), json.len()));
            assert_eq!(inserted, i64::from(QUI_ERR_PANIC));
            assert_eq!(qui_last_error_code(), QUI_ERR_PANIC);
            assert!(take_last_error().contains("qui_button_insert"));

            qui_app_free(app);
        }
    }
}
