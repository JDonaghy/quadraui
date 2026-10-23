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
 * foreign caller registers. Deliberately has no `render` member — see
 * [`FfiApp::render`] for why.
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
 * `json`/`json_len`.
 *
 * Returns `0` on success (the PoC's single-slot handle — there is only
 * ever one descriptor, so no caller-visible numbering is needed yet) or
 * `-1` on error (bad JSON, null app, or a descriptor already inserted),
 * with [`qui_last_error`] set to why.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`]. `json` must point at
 * `json_len` valid UTF-8 bytes.
 */
int64_t qui_button_insert(struct QuiApp *app, const uint8_t *json, uintptr_t json_len);

/**
 * Queue a synthetic `KeyPressed` event for the next [`qui_step`]/
 * [`qui_run`] call — the PoC's stand-in for a real backend's input source.
 *
 * Returns `0` on success, `-1` if `app` is null.
 *
 * # Safety
 * `app` must be a live pointer from [`qui_app_new`].
 */
int32_t qui_push_key_event(struct QuiApp *app, uint32_t ch);

/**
 * Advance the app by one non-blocking step: dispatch one queued event (if
 * any; see [`qui_push_key_event`]) through the app's `handle` callback,
 * otherwise call its `tick` callback, then repaint if the reaction calls
 * for it. Never blocks waiting for input, because there is nothing to wait
 * on in this headless PoC.
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
 * between", since there is no real event source to block on.
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
 * `app` must be a live pointer from [`qui_app_new`]. `needle` must point
 * at `needle_len` valid UTF-8 bytes.
 */
int32_t qui_screen_contains(const struct QuiApp *app, const uint8_t *needle, uintptr_t needle_len);

/**
 * Return the last error message set by a call on the current thread, as a
 * callee-allocated null-terminated UTF-8 string the caller must free with
 * [`qui_free`]. Returns null if there is no error.
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
