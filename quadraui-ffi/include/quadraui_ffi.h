#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Opaque handle to one running PoC app. Never constructed or inspected by
 * the caller; only ever passed back into this crate's other functions.
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
 * with a future reaction this PoC doesn't define yet, same tolerance
 * `#[non_exhaustive]` gives Rust callers of [`quadraui::Reaction`]).
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
 * Create a new headless PoC app on a `width`×`height` cell grid.
 *
 * Returns null (and sets the last error, see [`qui_last_error`]) if
 * `width`/`height` is `0`.
 *
 * # Safety
 * `vtable`'s `user_data` is an opaque pointer this crate never
 * dereferences; the caller is responsible for its lifetime outliving every
 * call made with the returned [`QuiApp`] handle.
 */
struct QuiApp *qui_app_new(uint16_t width, uint16_t height, struct QuiVTable vtable);

/**
 * Destroy an app created by [`qui_app_new`]. `app` may be null (no-op).
 *
 * # Safety
 * `app` must be either null or a still-live pointer returned by
 * [`qui_app_new`] that has not already been freed.
 */
void qui_app_free(struct QuiApp *app);

/**
 * Insert the one `Button` descriptor this PoC supports, JSON-encoded at
 * `json`/`json_len`. On success, immediately repaints the app's single
 * frame so the inserted button is visible to [`qui_screen_contains`] even
 * before the next [`qui_step`]/[`qui_run`] call.
 *
 * Returns `0` on success (the PoC's single-slot handle — there is only
 * ever one descriptor, so no caller-visible numbering is needed yet) or
 * `-1` on error (bad JSON, null app/json, or a descriptor already
 * inserted), with [`qui_last_error`] set to why.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`]. `json` must be either
 * null (only valid when `json_len` is `0`, and rejected as an error) or a
 * pointer to `json_len` valid UTF-8 bytes.
 */
int64_t qui_button_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the one `TextDisplay` descriptor this PoC supports, JSON-encoded
 * at `json`/`json_len`. quadraui has no primitive literally named "Label";
 * `TextDisplay` is the closest match for that role. Behaves exactly like
 * [`qui_button_insert`] otherwise: immediately repaints on success, and
 * returns `0`/`-1` with [`qui_last_error`] set on failure.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_text_display_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the one `ListView` descriptor this PoC supports, JSON-encoded at
 * `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_list_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the one `TreeView` descriptor this PoC supports, JSON-encoded at
 * `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_tree_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the one `DataTable` descriptor this PoC supports, JSON-encoded at
 * `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_data_table_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Insert the one `TabBar` descriptor this PoC supports, JSON-encoded at
 * `json`/`json_len`. Behaves exactly like [`qui_button_insert`] otherwise.
 *
 * # Safety
 * Same contract as [`qui_button_insert`].
 */
int64_t qui_tab_bar_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

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
 * Returns `0` on success, `-1` if `app` is null, `json` is null (with
 * `json_len != 0`), the bytes aren't valid UTF-8, or the JSON doesn't
 * decode to a `UiEvent` — see [`qui_last_error`] for which.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`]. `json` must be either
 * null (only valid when `json_len` is `0`, and rejected as an error) or a
 * pointer to `json_len` valid bytes.
 */
int32_t qui_push_event(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Queue a synthetic `KeyPressed` event for the next [`qui_step`]/
 * [`qui_run`] call — a convenience over [`qui_push_event`] for the common
 * single-character-key case, so a host doesn't have to hand-build the
 * `KeyPressed` JSON just to type a character.
 *
 * Returns `0` on success, `-1` if `app` is null.
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
 * Returns `0` on success, `-1` if `app` is null or `width`/`height` is `0`.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
int32_t qui_resize(struct QuiApp *app, uint16_t width, uint16_t height);

/**
 * Advance the app by one non-blocking step: dispatch one queued event (if
 * any; see [`qui_push_event`]/[`qui_push_key_event`]/[`qui_resize`] —
 * events dispatch in the order they were queued) through the app's
 * `handle` callback, otherwise call its `tick`
 * callback, then repaint if the reaction calls for it. Never blocks
 * waiting for input, because there is nothing to wait on in this headless
 * PoC.
 *
 * Returns `0` (continue), `1` (the app exited; further calls are a no-op
 * returning `1` again), or `-1` on error.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
int32_t qui_step(struct QuiApp *app);

/**
 * Run [`qui_step`] in a loop until the app exits or `max_steps` is reached
 * (whichever comes first): this headless PoC's stand-in for a real blocking
 * `qui_run` entry point, reduced to "drain the queue, polling `tick` in
 * between", since there is no real event source to block on. `max_steps`
 * is clamped to at least `1`, so `qui_run(app, 0)` still performs one step
 * rather than returning immediately without advancing the app at all.
 *
 * Returns the same codes as [`qui_step`].
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
int32_t qui_run(struct QuiApp *app, uint32_t max_steps);

/**
 * `true` (`1`) if the app's current rendered frame contains `needle`,
 * `false` (`0`) if not, `-1` on error — the assertion both this crate's
 * own smoke test and a foreign-language caller use to confirm a frame
 * actually painted the inserted [`Button`]'s label.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`]. `needle` must be
 * either null (only valid when `needle_len` is `0`, and rejected as an
 * error) or a pointer to `needle_len` valid UTF-8 bytes.
 */
int32_t qui_screen_contains(const struct QuiApp *app, const uint8_t *needle, uintptr_t needle_len);

/**
 * Return the last error message set by a call on the current thread, as a
 * callee-allocated null-terminated UTF-8 string the caller must free with
 * [`qui_free`]. Returns null if (and only if) there is no error — a
 * message that itself contains an interior NUL (not producible by any
 * error this crate sets today, since they're all built from serde/UTF-8
 * error `Display` output) falls back to a fixed placeholder string rather
 * than null, so "no error" is never ambiguous with "there was an error we
 * couldn't represent".
 */
char *qui_last_error(void);

/**
 * Free a string returned by [`qui_last_error`]. `ptr` may be null (no-op).
 *
 * # Safety
 * `ptr` must be either null or a pointer previously returned by
 * [`qui_last_error`] on this same allocator, not already freed.
 */
void qui_free(char *ptr);
