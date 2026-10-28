#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * The call succeeded. Also the "continue" step result of [`qui_step`].
 */
#define QUI_OK 0

/**
 * The [`QuiApp`] handle argument was null.
 */
#define QUI_ERR_NULL_HANDLE -1

/**
 * A pointer argument other than the app handle was null.
 */
#define QUI_ERR_NULL_POINTER -2

/**
 * A `(pointer, length)` argument had length `0`, which never names a
 * meaningful descriptor, event or needle.
 */
#define QUI_ERR_EMPTY_INPUT -3

/**
 * Input bytes were not valid UTF-8.
 */
#define QUI_ERR_INVALID_UTF8 -4

/**
 * Input was valid UTF-8 but did not decode to the type the entry point
 * expected (malformed JSON, or JSON of the wrong shape).
 */
#define QUI_ERR_INVALID_JSON -5

/**
 * The descriptor slot already holds a descriptor. Clear it with
 * [`qui_slot_clear`] before inserting another.
 */
#define QUI_ERR_SLOT_OCCUPIED -6

/**
 * The slot id was not one of the `QUI_SLOT_*` values.
 */
#define QUI_ERR_UNKNOWN_SLOT -7

/**
 * A width or height argument was `0`.
 */
#define QUI_ERR_INVALID_DIMENSIONS -8

/**
 * A `uint32_t` character argument was not a Unicode scalar value (a
 * surrogate, or above `U+10FFFF`).
 */
#define QUI_ERR_INVALID_CHAR -9

/**
 * A Rust panic was caught at the ABI boundary. The app handle it happened
 * on is poisoned; see [`QUI_ERR_POISONED`].
 */
#define QUI_ERR_PANIC -10

/**
 * The app handle is poisoned by an earlier caught panic. Only
 * [`qui_app_free`] still works on it.
 */
#define QUI_ERR_POISONED -11

/**
 * A bug in this crate — a failure that is not the caller's fault, such as
 * an event that would not serialize.
 */
#define QUI_ERR_INTERNAL -12

/**
 * The `kind` passed to [`qui_app_path`] was not one of the `QUI_PATH_*`
 * values, or this platform or build could not resolve it.
 */
#define QUI_ERR_UNSUPPORTED -13

/**
 * [`qui_step`] advanced the app and it is still running.
 */
#define QUI_STEP_CONTINUE QUI_OK

/**
 * [`qui_step`] observed the app exit. Further steps are a no-op returning
 * this same value.
 */
#define QUI_STEP_EXITED 1

/**
 * Slot id of the app's `Button` descriptor.
 */
#define QUI_SLOT_BUTTON 0

/**
 * Slot id of the app's `TextDisplay` descriptor.
 */
#define QUI_SLOT_TEXT_DISPLAY 1

/**
 * Slot id of the app's `ListView` descriptor.
 */
#define QUI_SLOT_LIST 2

/**
 * Slot id of the app's `TreeView` descriptor.
 */
#define QUI_SLOT_TREE 3

/**
 * Slot id of the app's `DataTable` descriptor.
 */
#define QUI_SLOT_DATA_TABLE 4

/**
 * Slot id of the app's `TabBar` descriptor.
 */
#define QUI_SLOT_TAB_BAR 5

/**
 * `kind` for [`qui_app_path`]: durable, app-scoped data (Electron's
 * `userData`), scoped by [`qui_set_app_identifier`].
 */
#define QUI_PATH_USER_DATA 0

/**
 * `kind` for [`qui_app_path`]: app-scoped configuration, scoped by
 * [`qui_set_app_identifier`].
 */
#define QUI_PATH_APP_CONFIG 1

/**
 * `kind` for [`qui_app_path`]: app-scoped cache, scoped by
 * [`qui_set_app_identifier`].
 */
#define QUI_PATH_APP_CACHE 2

/**
 * `kind` for [`qui_app_path`]: app-scoped log directory, scoped by
 * [`qui_set_app_identifier`].
 */
#define QUI_PATH_LOGS 3

/**
 * `kind` for [`qui_app_path`]: the OS-wide temporary directory, unscoped
 * by app identity.
 */
#define QUI_PATH_TEMP 4

/**
 * `kind` for [`qui_app_path`]: the user's Downloads folder, unscoped by
 * app identity.
 */
#define QUI_PATH_DOWNLOADS 5

/**
 * `kind` for [`qui_app_path`]: the user's Documents folder, unscoped by
 * app identity.
 */
#define QUI_PATH_DOCUMENTS 6

/**
 * `kind` for [`qui_app_path`]: the user's home directory, unscoped by
 * app identity.
 */
#define QUI_PATH_HOME 7

/**
 * Opaque handle to one running app. Never constructed or inspected by the
 * caller; only ever passed back into this crate's other functions.
 */
typedef struct QuiApp QuiApp;

/**
 * Vtable a host passes to [`qui_app_new`]: the `handle`/`tick` callbacks a
 * foreign caller registers. Deliberately has no `render` member: quadraui
 * paints every descriptor it holds itself, so there is nothing for a
 * foreign callback to do.
 *
 * Both callbacks return a `Reaction` code: `0` = Continue, `1` = Redraw,
 * `2` = Exit. Any other value is treated as `Continue` (forward-compatible
 * with a future reaction this ABI doesn't define yet, the same tolerance
 * `#[non_exhaustive]` gives Rust callers of [`quadraui::Reaction`]).
 *
 * Neither callback may let a panic or an exception escape: they are plain
 * `extern "C"` function pointers, and unwinding out of one aborts the
 * process. Catch on the host side and return a reaction code instead.
 */
typedef struct QuiVTable {
  /**
   * Opaque pointer the host owns; passed back unchanged into `handle`
   * and `tick`. Never dereferenced by this crate.
   */
  void *user_data;
  /**
   * Called once per event `qui_step`/`qui_run` dequeues. `event_json`
   * points at `event_json_len` UTF-8 bytes (not null-terminated); the
   * pointer is valid only for the duration of this call.
   */
  int32_t (*handle)(void*, const uint8_t*, uintptr_t);
  /**
   * Called once per `qui_step`/`qui_run` call when no event is queued.
   */
  int32_t (*tick)(void*);
} QuiVTable;

/**
 * Create a new headless app on a `width`×`height` cell grid.
 *
 * Returns null on failure, with [`qui_last_error_code`] set to
 * [`QUI_ERR_INVALID_DIMENSIONS`] if `width`/`height` is `0`, or
 * [`QUI_ERR_PANIC`] if constructing the driver panicked.
 *
 * The returned handle is owned by the caller and must be released with
 * [`qui_app_free`], never with `free()`.
 *
 * # Safety
 * `vtable`'s `user_data` is an opaque pointer this crate never
 * dereferences; the caller is responsible for its lifetime outliving every
 * call made with the returned [`QuiApp`] handle.
 */
struct QuiApp *qui_app_new(uint16_t width, uint16_t height, struct QuiVTable vtable);

/**
 * Destroy an app created by [`qui_app_new`]. `app` may be null (no-op).
 * Works even on a handle poisoned by a caught panic — this is the one
 * function that does.
 *
 * Leaves the last error untouched, so a host can free the app on its error
 * path and still report what failed afterwards.
 *
 * # Safety
 * `app` must be either null or a still-live pointer returned by
 * [`qui_app_new`] that has not already been freed.
 */
void qui_app_free(struct QuiApp *app);

/**
 * Insert the app's `Button` descriptor, JSON-encoded at `json`/`json_len`.
 * On success, immediately repaints the app's frame so the inserted button
 * is visible to [`qui_screen_contains`] even before the next
 * [`qui_step`]/[`qui_run`] call.
 *
 * Returns the descriptor's slot id ([`QUI_SLOT_BUTTON`]) on success, or a
 * negative `QUI_ERR_*` code on failure: [`QUI_ERR_NULL_HANDLE`],
 * [`QUI_ERR_NULL_POINTER`], [`QUI_ERR_EMPTY_INPUT`],
 * [`QUI_ERR_INVALID_UTF8`], [`QUI_ERR_INVALID_JSON`],
 * [`QUI_ERR_SLOT_OCCUPIED`], [`QUI_ERR_POISONED`] or [`QUI_ERR_PANIC`],
 * with [`qui_last_error`] set to why. The slot id can be passed straight to
 * [`qui_slot_clear`] to take the descriptor back out.
 *
 * The JSON bytes are borrowed for the duration of the call only.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`]. `json` must be either
 * null (rejected as an error) or valid for reads of `json_len` bytes.
 */
int64_t qui_button_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the app's `TextDisplay` descriptor, JSON-encoded at
 * `json`/`json_len`. quadraui has no primitive literally named "Label";
 * `TextDisplay` is the closest match for that role. Behaves exactly like
 * [`qui_button_insert`] otherwise, returning [`QUI_SLOT_TEXT_DISPLAY`] on
 * success.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_text_display_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the app's `ListView` descriptor, JSON-encoded at `json`/`json_len`.
 * Behaves exactly like [`qui_button_insert`] otherwise, returning
 * [`QUI_SLOT_LIST`] on success.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_list_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the app's `TreeView` descriptor, JSON-encoded at `json`/`json_len`.
 * Behaves exactly like [`qui_button_insert`] otherwise, returning
 * [`QUI_SLOT_TREE`] on success.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_tree_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the app's `DataTable` descriptor, JSON-encoded at
 * `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise,
 * returning [`QUI_SLOT_DATA_TABLE`] on success.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_data_table_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the app's `TabBar` descriptor, JSON-encoded at `json`/`json_len`.
 * Behaves exactly like [`qui_button_insert`] otherwise, returning
 * [`QUI_SLOT_TAB_BAR`] on success.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_tab_bar_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Take the descriptor back out of one slot and repaint, so the slot can be
 * filled again with a different descriptor. `slot` is one of the
 * `QUI_SLOT_*` values — the same id the matching `qui_<name>_insert`
 * returned.
 *
 * Returns [`QUI_OK`] whether or not the slot held anything (clearing an
 * empty slot is not an error), or [`QUI_ERR_UNKNOWN_SLOT`] if `slot` is not
 * a `QUI_SLOT_*` value, plus the usual [`QUI_ERR_NULL_HANDLE`],
 * [`QUI_ERR_POISONED`] and [`QUI_ERR_PANIC`].
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
int32_t qui_slot_clear(struct QuiApp *app, int32_t slot);

/**
 * Queue any [`quadraui::UiEvent`] for the next [`qui_step`]/[`qui_run`]
 * call, JSON-encoded at `json`/`json_len` — the real input surface a host
 * drives: mouse, key, scroll, focus, window and every other variant all go
 * through this one function, exactly the same JSON-only-for-payloads shape
 * every `qui_<name>_insert` function already uses. Mirror quadraui's own
 * `#[derive(Serialize, Deserialize)]` field names and shape for the variant
 * being sent (see `quadraui::UiEvent`'s docs for each variant's payload).
 *
 * [`quadraui::UiEvent::User`] deliberately has no `Serialize`/`Deserialize`
 * impl (its payload is an in-process-only `Arc<dyn Any>`), so JSON for it
 * always fails to decode here — there is nothing meaningful to send across
 * a wire boundary for that variant.
 *
 * Returns [`QUI_OK`], or [`QUI_ERR_NULL_HANDLE`],
 * [`QUI_ERR_NULL_POINTER`], [`QUI_ERR_EMPTY_INPUT`],
 * [`QUI_ERR_INVALID_UTF8`], [`QUI_ERR_INVALID_JSON`],
 * [`QUI_ERR_POISONED`] or [`QUI_ERR_PANIC`] — see [`qui_last_error`] for
 * which.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`]. `json` must be either
 * null (rejected as an error) or valid for reads of `json_len` bytes.
 */
int32_t qui_push_event(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Queue a synthetic `KeyPressed` event for the next [`qui_step`]/
 * [`qui_run`] call — a convenience over [`qui_push_event`] for the common
 * single-character-key case, so a host doesn't have to hand-build the
 * `KeyPressed` JSON just to type a character.
 *
 * Returns [`QUI_OK`], [`QUI_ERR_NULL_HANDLE`], [`QUI_ERR_INVALID_CHAR`] if
 * `ch` is not a Unicode scalar value, [`QUI_ERR_POISONED`] or
 * [`QUI_ERR_PANIC`].
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
int32_t qui_push_key_event(struct QuiApp *app, uint32_t ch);

/**
 * Resize the app's headless cell grid to `width`×`height` and queue the
 * matching `WindowResized` event — the "resize" leg of the real input
 * surface, kept separate from [`qui_push_event`] because resizing the
 * underlying grid itself is a [`quadraui::tui::testing::TuiDriver`]
 * operation, not something any `UiEvent` payload can express on its own; a
 * plain [`qui_push_event`] with a `WindowResized` payload would notify the
 * app's `handle` callback but leave the grid at its old size.
 *
 * Returns [`QUI_OK`], [`QUI_ERR_NULL_HANDLE`],
 * [`QUI_ERR_INVALID_DIMENSIONS`] if `width`/`height` is `0`,
 * [`QUI_ERR_POISONED`] or [`QUI_ERR_PANIC`].
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
int32_t qui_resize(struct QuiApp *app, uint16_t width, uint16_t height);

/**
 * Advance the app by one non-blocking step: dispatch one queued event (if
 * any; see [`qui_push_event`]/[`qui_push_key_event`]/[`qui_resize`] —
 * events dispatch in the order they were queued) through the app's
 * `handle` callback, otherwise call its `tick` callback, then repaint if
 * the reaction calls for it. Never blocks waiting for input, because there
 * is nothing to wait on in a headless app.
 *
 * Returns [`QUI_STEP_CONTINUE`], [`QUI_STEP_EXITED`] (the app exited;
 * further calls are a no-op returning this again), or a negative
 * `QUI_ERR_*` code. A [`QUI_ERR_PANIC`] here is fatal for this handle: the
 * panic unwound out of a half-finished dispatch, so the handle is poisoned
 * and every later call but [`qui_app_free`] returns
 * [`QUI_ERR_POISONED`].
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
int32_t qui_step(struct QuiApp *app);

/**
 * Run [`qui_step`] in a loop until the app exits, a step fails, or
 * `max_steps` is reached (whichever comes first): a headless app's stand-in
 * for a real blocking `qui_run` entry point, reduced to "drain the queue,
 * polling `tick` in between", since there is no real event source to block
 * on. `max_steps` is clamped to at least `1`, so `qui_run(app, 0)` still
 * performs one step rather than returning without advancing the app at all.
 *
 * Returns the same codes as [`qui_step`], stopping at the first step that
 * does not return [`QUI_STEP_CONTINUE`].
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
int32_t qui_run(struct QuiApp *app, uint32_t max_steps);

/**
 * `true` (`1`) if the app's current rendered frame contains `needle`,
 * `false` (`0`) if not — the assertion a foreign-language caller uses to
 * confirm a frame actually painted an inserted descriptor's text.
 *
 * Returns a negative `QUI_ERR_*` code on failure:
 * [`QUI_ERR_NULL_HANDLE`], [`QUI_ERR_NULL_POINTER`],
 * [`QUI_ERR_EMPTY_INPUT`] (an empty needle is rejected rather than
 * trivially "found"), [`QUI_ERR_INVALID_UTF8`], [`QUI_ERR_POISONED`] or
 * [`QUI_ERR_PANIC`].
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`]. `needle` must be
 * either null (rejected as an error) or valid for reads of `needle_len`
 * bytes.
 */
int32_t qui_screen_contains(const struct QuiApp *app, const uint8_t *needle, uintptr_t needle_len);

/**
 * Return the app's current rendered frame as a callee-allocated,
 * null-terminated UTF-8 string of newline-separated rows — the diagnostic
 * companion to [`qui_screen_contains`], for printing what was actually
 * painted when an assertion fails.
 *
 * The caller owns the returned string and must release it with
 * [`qui_free`], never with `free()`. Returns null on failure
 * ([`QUI_ERR_NULL_HANDLE`], [`QUI_ERR_POISONED`], [`QUI_ERR_PANIC`]), with
 * [`qui_last_error`] set to why.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
char *qui_screen(const struct QuiApp *app);

/**
 * Resolve a standard, per-app filesystem location —
 * `quadraui::PlatformServices::app_path` surfaced over FFI. `kind` is one
 * of the `QUI_PATH_*` constants.
 *
 * Needs no [`QuiApp`] handle: resolving a standard path needs no window,
 * display server or driver, only the filesystem and environment
 * (`quadraui::paths`'s module doc has the full rationale).
 *
 * Returns a caller-owned, null-terminated UTF-8 string the caller must
 * free with [`qui_free`], or null on failure — [`QUI_ERR_UNSUPPORTED`] for
 * a `kind` outside the `QUI_PATH_*` set, or one this platform or build
 * cannot resolve — with [`qui_last_error`] set to why.
 */
char *qui_app_path(int32_t kind);

/**
 * Set the app identifier that scopes [`QUI_PATH_USER_DATA`],
 * [`QUI_PATH_APP_CONFIG`], [`QUI_PATH_APP_CACHE`] and [`QUI_PATH_LOGS`] —
 * `quadraui::paths::set_app_identifier` surfaced over FFI.
 *
 * Process-global, not per-[`QuiApp`] handle: call this once at startup,
 * before the first [`qui_app_path`] call that needs it.
 *
 * # Safety
 * `ptr` must be either null or valid for reads of `len` bytes.
 */
int32_t qui_set_app_identifier(const uint8_t *ptr, uintptr_t len);

/**
 * Return the error code of the last call on the current thread: [`QUI_OK`]
 * if it succeeded, otherwise the same negative `QUI_ERR_*` code that call
 * returned. Lets a host read the machine-checkable half of the error
 * channel without allocating the message, and recover the code from the
 * calls that can only signal failure by returning null
 * ([`qui_app_new`], [`qui_screen`], [`qui_last_error`]).
 *
 * Does not disturb the stored error, so it can be called before or after
 * [`qui_last_error`], in either order, as often as the host likes.
 */
int32_t qui_last_error_code(void);

/**
 * Return the last error message set by a call on the current thread, as a
 * callee-allocated null-terminated UTF-8 string the caller must free with
 * [`qui_free`] (never with `free()`).
 *
 * Returns null if (and only if) there is no error — a message that itself
 * contains an interior NUL (not producible by any error this crate sets
 * today, since they are all built from `Display` output of serde/UTF-8
 * errors and panic payloads) falls back to a fixed placeholder string
 * rather than null, so "no error" is never ambiguous with "there was an
 * error we couldn't represent". Pair it with [`qui_last_error_code`] for
 * the machine-readable half, which is also what to check when this returns
 * null unexpectedly.
 *
 * Does not clear the stored error: two calls in a row return two
 * independently owned copies of the same message, each needing its own
 * [`qui_free`].
 */
char *qui_last_error(void);

/**
 * Return the spelling of an error code — `"QUI_ERR_INVALID_JSON"` for
 * [`QUI_ERR_INVALID_JSON`], and so on — so a host can log or assert on a
 * code without hard-coding a parallel table of names.
 *
 * The returned pointer is static storage owned by this crate, valid for the
 * life of the process: the caller must **not** free it, and must not pass
 * it to [`qui_free`]. Returns null for a code this crate does not define.
 * Leaves the last error untouched.
 */
const char *qui_error_name(int32_t code);

/**
 * Free a string this crate allocated for the caller: the message from
 * [`qui_last_error`] or the frame dump from [`qui_screen`]. `ptr` may be
 * null (no-op). Leaves the last error untouched.
 *
 * This is the *only* correct deallocator for those two strings — they come
 * from Rust's allocator, so calling `free()` on one is undefined behaviour.
 * Conversely, never pass this function anything else: not a pointer the
 * caller allocated, not the static string from [`qui_error_name`], and not
 * a pointer already freed.
 *
 * # Safety
 * `ptr` must be either null or a pointer previously returned by
 * [`qui_last_error`] or [`qui_screen`], not already freed.
 */
void qui_free(char *ptr);
