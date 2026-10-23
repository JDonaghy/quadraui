/* Worked C consumer for quadraui-ffi, doubling as the C-side integration
 * test for the ABI.
 *
 * It is written the way a real binding author would write their first
 * program against this header, and it asserts every promise the header
 * makes, in five phases:
 *
 *   1. Build a multi-widget screen — one descriptor of each covered
 *      primitive type, decoded from JSON exactly as a host's own
 *      serializer would emit it — and assert the rendered frame actually
 *      painted each one's content.
 *   2. Drive the real input surface: `qui_push_key_event`, `qui_push_event`
 *      (a mouse click, JSON-encoded like a wire protocol) and `qui_resize`,
 *      checking each one's event reaches the registered vtable callback in
 *      the order it was queued.
 *   3. Walk the error taxonomy: provoke every failure mode a C caller can
 *      provoke and assert both the returned code and `qui_last_error_code`,
 *      printing each one's `qui_error_name` spelling.
 *   4. Exercise memory ownership: free every callee-allocated string with
 *      `qui_free`, never touch the static string from `qui_error_name`, and
 *      take a frame dump for diagnostics.
 *   5. Reach the exit reaction and confirm the app latches it.
 *
 * Covers `Button`, `TextDisplay` (quadraui's closest match to a "Label"),
 * `ListView`, `TreeView`, `DataTable` and `TabBar` — one `qui_<name>_insert`
 * call each, plus `qui_slot_clear` to take one back out and refill it.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "quadraui_ffi.h"

#define LAST_EVENT_JSON_CAP 256

/* Reaction codes the vtable callbacks return, as documented on QuiVTable. */
#define REACTION_CONTINUE 0
#define REACTION_REDRAW 1
#define REACTION_EXIT 2

static int g_handle_calls = 0;
static int g_tick_calls = 0;
static int g_error_paths = 0;
static int g_exit_on_next_event = 0;
static char g_last_event_json[LAST_EVENT_JSON_CAP];

/* The descriptors this consumer builds its screen from. A real host would
 * emit these with its own serializer; spelling them out here keeps the C
 * side dependency-free and makes the expected field names visible. */
static const char *const BUTTON_JSON = "{\"id\":\"demo:ok\",\"label\":\"OK\"}";
static const char *const BUTTON_RETRY_JSON = "{\"id\":\"demo:retry\",\"label\":\"Retry\"}";
static const char *const TEXT_DISPLAY_JSON =
    "{\"id\":\"text:1\",\"lines\":[{\"spans\":[{\"text\":\"Hello\",\"fg\":null,\"bg\":null}]}]}";
static const char *const LIST_JSON =
    "{\"id\":\"list:1\",\"items\":[{\"text\":{\"spans\":[{\"text\":\"Row 1\",\"fg\":null,"
    "\"bg\":null}]}}],\"selected_idx\":0}";
static const char *const TREE_JSON =
    "{\"id\":\"tree:1\",\"rows\":[{\"path\":[0],\"indent\":0,\"icon\":null,\"text\":{\"spans\":"
    "[{\"text\":\"Alpha\",\"fg\":null,\"bg\":null}]},\"badge\":null,\"is_expanded\":null}],"
    "\"selection_mode\":\"Single\",\"selected_path\":null,\"style\":{\"indent\":2,"
    "\"show_chevrons\":true,\"chevron_expanded\":\"v\",\"chevron_collapsed\":\">\"}}";
static const char *const DATA_TABLE_JSON =
    "{\"id\":\"table:1\",\"columns\":[{\"title\":\"Name\"}],\"rows\":[{\"cells\":[{\"spans\":"
    "[{\"text\":\"Row1\",\"fg\":null,\"bg\":null}]}]}]}";
static const char *const TAB_BAR_JSON = "{\"id\":\"tabbar:1\",\"tabs\":[{\"label\":\"Tab 1\"}]}";
static const char *const MOUSE_DOWN_JSON =
    "{\"MouseDown\":{\"widget\":null,\"button\":\"Left\","
    "\"position\":{\"x\":2.0,\"y\":1.0},\"modifiers\":{}}}";

/* Convenience for the `(const uint8_t *, uintptr_t)` byte-slice pairs the
 * ABI takes: it borrows the bytes for the duration of the call only, so a
 * plain string literal is a valid argument. */
#define BYTES(s) (const uint8_t *)(s), strlen(s)

static int32_t on_handle(void *user_data, const uint8_t *event_json, uintptr_t len) {
    (void)user_data;
    g_handle_calls += 1;
    size_t copy_len = len < LAST_EVENT_JSON_CAP - 1 ? len : LAST_EVENT_JSON_CAP - 1;
    memcpy(g_last_event_json, event_json, copy_len);
    g_last_event_json[copy_len] = '\0';
    return g_exit_on_next_event ? REACTION_EXIT : REACTION_CONTINUE;
}

static int32_t on_tick(void *user_data) {
    (void)user_data;
    g_tick_calls += 1;
    return REACTION_CONTINUE;
}

/* Prints the full error channel — code, its spelling, and the message —
 * then exits. `qui_last_error` hands out an owned copy the caller must
 * release with `qui_free`; `qui_error_name` hands out static storage that
 * must never be freed. */
static void fail(const char *what) {
    int32_t code = qui_last_error_code();
    const char *name = qui_error_name(code);
    fprintf(stderr, "FAIL: %s\n", what);
    fprintf(stderr, "  last error code: %d (%s)\n", code, name != NULL ? name : "<undefined>");
    char *err = qui_last_error();
    if (err != NULL) {
        fprintf(stderr, "  last error: %s\n", err);
        qui_free(err);
    }
    exit(1);
}

/* Fails unless `actual` is `expected`, reporting both by name. */
static void expect_code(int32_t actual, int32_t expected, const char *what) {
    if (actual == expected) {
        return;
    }
    const char *actual_name = qui_error_name(actual);
    const char *expected_name = qui_error_name(expected);
    fprintf(stderr, "  expected %d (%s), got %d (%s)\n", expected,
            expected_name != NULL ? expected_name : "<undefined>", actual,
            actual_name != NULL ? actual_name : "<undefined>");
    fail(what);
}

/* Fails unless `actual` is `expected`, for the `int64_t`-returning inserts
 * (whose success value is a `QUI_SLOT_*` id, not `QUI_OK`). */
static void expect_slot(int64_t actual, int64_t expected, const char *what) {
    if (actual == expected) {
        return;
    }
    fprintf(stderr, "  expected slot/code %lld, got %lld\n", (long long)expected,
            (long long)actual);
    fail(what);
}

/* Fails unless the call both returned `expected_code` and left that same
 * code in the thread's error channel, with a non-empty message — the
 * contract every failing entry point promises. */
static void expect_failure(int32_t actual, int32_t expected_code, const char *what) {
    expect_code(actual, expected_code, what);
    expect_code(qui_last_error_code(), expected_code, what);
    char *message = qui_last_error();
    if (message == NULL || message[0] == '\0') {
        fprintf(stderr, "FAIL: %s: no last-error message was set\n", what);
        exit(1);
    }
    const char *name = qui_error_name(expected_code);
    printf("  %-26s %s\n", name != NULL ? name : "<undefined>", message);
    qui_free(message);
    g_error_paths += 1;
}

/* Prints the app's current frame, for diagnosing a failed screen
 * assertion. The dump is callee-allocated and freed with `qui_free`. */
static void dump_screen(const struct QuiApp *app) {
    char *screen = qui_screen(app);
    if (screen == NULL) {
        fprintf(stderr, "  (could not read the frame)\n");
        return;
    }
    fprintf(stderr, "--- frame ---\n%s\n-------------\n", screen);
    qui_free(screen);
}

/* Fails with `what` unless the most recent `handle` callback's JSON names
 * `variant_tag` as its outer (externally-tagged) key, e.g. `"MouseDown"`. */
static void expect_last_event_variant(const char *variant_tag, const char *what) {
    char needle[64];
    snprintf(needle, sizeof(needle), "\"%s\"", variant_tag);
    if (strstr(g_last_event_json, needle) == NULL) {
        fprintf(stderr, "  last event JSON: %s\n", g_last_event_json);
        fail(what);
    }
}

static void expect_handle_calls(int expected, const char *what) {
    if (g_handle_calls != expected) {
        fprintf(stderr, "FAIL: %s: expected %d handle callbacks, got %d\n", what, expected,
                g_handle_calls);
        exit(1);
    }
}

/* Asserts `needle` is somewhere in the app's current rendered frame. */
static void expect_on_screen(const struct QuiApp *app, const char *needle, const char *what) {
    int32_t found = qui_screen_contains(app, BYTES(needle));
    if (found != 1) {
        fprintf(stderr, "  %s: expected %s on screen, qui_screen_contains returned %d\n", what,
                needle, found);
        dump_screen(app);
        fail(what);
    }
}

/* The complement: asserts `needle` is *not* on screen, which is how a
 * cleared slot is observed. */
static void expect_off_screen(const struct QuiApp *app, const char *needle, const char *what) {
    int32_t found = qui_screen_contains(app, BYTES(needle));
    if (found != 0) {
        fprintf(stderr, "  %s: expected %s gone from screen, qui_screen_contains returned %d\n",
                what, needle, found);
        dump_screen(app);
        fail(what);
    }
}

/* Phase 1: one descriptor of each primitive type, then assert the frame
 * painted all six. Each insert returns the slot id it filled, which is the
 * same id `qui_slot_clear` takes. */
static void build_the_screen(struct QuiApp *app) {
    expect_slot(qui_button_insert(app, BYTES(BUTTON_JSON)), QUI_SLOT_BUTTON,
                "qui_button_insert did not return QUI_SLOT_BUTTON");
    expect_slot(qui_text_display_insert(app, BYTES(TEXT_DISPLAY_JSON)), QUI_SLOT_TEXT_DISPLAY,
                "qui_text_display_insert did not return QUI_SLOT_TEXT_DISPLAY");
    expect_slot(qui_list_insert(app, BYTES(LIST_JSON)), QUI_SLOT_LIST,
                "qui_list_insert did not return QUI_SLOT_LIST");
    expect_slot(qui_tree_insert(app, BYTES(TREE_JSON)), QUI_SLOT_TREE,
                "qui_tree_insert did not return QUI_SLOT_TREE");
    expect_slot(qui_data_table_insert(app, BYTES(DATA_TABLE_JSON)), QUI_SLOT_DATA_TABLE,
                "qui_data_table_insert did not return QUI_SLOT_DATA_TABLE");
    expect_slot(qui_tab_bar_insert(app, BYTES(TAB_BAR_JSON)), QUI_SLOT_TAB_BAR,
                "qui_tab_bar_insert did not return QUI_SLOT_TAB_BAR");

    /* A run of successful calls leaves the error channel clean. */
    expect_code(qui_last_error_code(), QUI_OK, "a successful insert left an error behind");
    if (qui_last_error() != NULL) {
        fail("a successful insert left an error message behind");
    }

    expect_on_screen(app, "OK", "the Button's label is missing");
    expect_on_screen(app, "Hello", "the TextDisplay's line is missing");
    expect_on_screen(app, "Row 1", "the ListView's item is missing");
    expect_on_screen(app, "Alpha", "the TreeView's row is missing");
    expect_on_screen(app, "Row1", "the DataTable's cell is missing");
    expect_on_screen(app, "Tab 1", "the TabBar's tab is missing");
}

/* Phase 2: the real input surface. Each push queues one event; each
 * `qui_run` drains the queue through the vtable's `handle` callback. */
static void drive_the_input_surface(struct QuiApp *app) {
    expect_code(qui_push_key_event(app, (uint32_t)'x'), QUI_OK, "qui_push_key_event failed");
    expect_code(qui_run(app, 4), QUI_STEP_CONTINUE, "qui_run reported an error");
    expect_handle_calls(1, "after a key event");
    expect_last_event_variant("KeyPressed", "qui_push_key_event's event did not reach handle");

    /* `qui_push_event` carries any `UiEvent` variant, JSON-encoded by the
     * host exactly as it would be for a wire protocol. */
    expect_code(qui_push_event(app, BYTES(MOUSE_DOWN_JSON)), QUI_OK,
                "qui_push_event failed to queue a MouseDown event");
    expect_code(qui_run(app, 4), QUI_STEP_CONTINUE, "qui_run reported an error after a mouse event");
    expect_handle_calls(2, "after a mouse event");
    expect_last_event_variant("MouseDown", "qui_push_event's MouseDown event did not reach handle");

    /* `qui_resize` grows the headless grid and queues the matching
     * `WindowResized` event in one call. */
    expect_code(qui_resize(app, 60, 40), QUI_OK, "qui_resize failed");
    expect_code(qui_run(app, 4), QUI_STEP_CONTINUE, "qui_run reported an error after a resize");
    expect_handle_calls(3, "after a resize");
    expect_last_event_variant("WindowResized", "qui_resize's WindowResized event did not reach handle");

    /* With the queue drained, a step polls `tick` instead. */
    int ticks_before = g_tick_calls;
    expect_code(qui_step(app), QUI_STEP_CONTINUE, "an idle qui_step reported an error");
    expect_handle_calls(3, "after an idle step");
    if (g_tick_calls != ticks_before + 1) {
        fprintf(stderr, "FAIL: an idle qui_step did not call tick (%d -> %d)\n", ticks_before,
                g_tick_calls);
        exit(1);
    }
}

/* Phase 3: every failure mode a C caller can reach, asserted against both
 * the returned code and the thread-local error channel. */
static void walk_the_error_taxonomy(struct QuiApp *app) {
    printf("error taxonomy:\n");

    /* Null handle: every entry point taking an app rejects it. */
    expect_failure((int32_t)qui_button_insert(NULL, BYTES(BUTTON_JSON)), QUI_ERR_NULL_HANDLE,
                   "a null app handle was not rejected by qui_button_insert");
    expect_failure(qui_push_key_event(NULL, (uint32_t)'x'), QUI_ERR_NULL_HANDLE,
                   "a null app handle was not rejected by qui_push_key_event");
    expect_failure(qui_step(NULL), QUI_ERR_NULL_HANDLE,
                   "a null app handle was not rejected by qui_step");

    /* Null byte pointer, and a zero length, are distinct from each other. */
    expect_failure(qui_push_event(app, NULL, 4), QUI_ERR_NULL_POINTER,
                   "a null json pointer was not rejected");
    expect_failure(qui_push_event(app, (const uint8_t *)MOUSE_DOWN_JSON, 0), QUI_ERR_EMPTY_INPUT,
                   "an empty json length was not rejected");
    expect_failure(qui_screen_contains(app, (const uint8_t *)"OK", 0), QUI_ERR_EMPTY_INPUT,
                   "an empty needle was not rejected");

    /* Bytes that are not UTF-8 are reported as such, not as bad JSON. */
    static const uint8_t not_utf8[] = {0xff, 0xfe, 0xfd};
    expect_failure(qui_push_event(app, not_utf8, sizeof(not_utf8)), QUI_ERR_INVALID_UTF8,
                   "non-UTF-8 bytes were not rejected");

    /* Valid UTF-8 that does not decode to the expected type. The slot has
     * to be free for the decode to be attempted at all: an occupied slot
     * is reported before the JSON is looked at. */
    expect_code(qui_slot_clear(app, QUI_SLOT_LIST), QUI_OK,
                "qui_slot_clear failed on an occupied slot");
    expect_failure((int32_t)qui_list_insert(app, BYTES("not json")), QUI_ERR_INVALID_JSON,
                   "malformed JSON was not rejected");
    expect_slot(qui_list_insert(app, BYTES(LIST_JSON)), QUI_SLOT_LIST,
                "a rejected insert left the slot unusable");
    expect_failure(qui_push_event(app, BYTES("{\"NoSuchVariant\":{}}")), QUI_ERR_INVALID_JSON,
                   "an unknown event variant was not rejected");

    /* Each primitive type has exactly one slot, so a second insert into an
     * occupied one is an error rather than a silent replacement. */
    expect_failure((int32_t)qui_button_insert(app, BYTES(BUTTON_RETRY_JSON)), QUI_ERR_SLOT_OCCUPIED,
                   "inserting into an occupied slot was not rejected");

    /* Slot ids outside the `QUI_SLOT_*` range. */
    expect_failure(qui_slot_clear(app, 99), QUI_ERR_UNKNOWN_SLOT,
                   "an unknown slot id was not rejected");

    /* A `uint32_t` that is not a Unicode scalar value (a lone surrogate). */
    expect_failure(qui_push_key_event(app, 0xD800), QUI_ERR_INVALID_CHAR,
                   "a non-scalar code point was not rejected");

    /* Zero dimensions, on both the constructor and the resize path. The
     * constructor can only signal failure by returning null, which is
     * exactly what `qui_last_error_code` is for. */
    expect_failure(qui_resize(app, 0, 10), QUI_ERR_INVALID_DIMENSIONS,
                   "a zero width was not rejected by qui_resize");
    struct QuiVTable empty_vtable;
    memset(&empty_vtable, 0, sizeof(empty_vtable));
    struct QuiApp *rejected = qui_app_new(0, 10, empty_vtable);
    if (rejected != NULL) {
        fail("qui_app_new accepted a zero width");
    }
    expect_failure(QUI_ERR_INVALID_DIMENSIONS, QUI_ERR_INVALID_DIMENSIONS,
                   "qui_app_new's failure was not reported in the error channel");

    /* None of the above disturbed the app: the screen is intact and the
     * next successful call clears the error channel. */
    expect_on_screen(app, "OK", "an error path damaged the rendered frame");
    expect_code(qui_last_error_code(), QUI_OK, "a successful call did not clear the error channel");
}

/* Phase 4: ownership. Every pointer the ABI hands out is either
 * caller-owned-and-`qui_free`d or static-and-never-freed; nothing here
 * transfers ownership of a caller-allocated pointer into the library. */
static void exercise_memory_ownership(struct QuiApp *app) {
    /* Two calls in a row return two independently owned copies of the same
     * message, each needing its own free. */
    expect_failure(qui_push_event(app, BYTES("{")), QUI_ERR_INVALID_JSON,
                   "malformed event JSON was not rejected");
    char *first = qui_last_error();
    char *second = qui_last_error();
    if (first == NULL || second == NULL) {
        fail("qui_last_error returned null while an error was set");
    }
    if (first == second) {
        fail("qui_last_error handed out the same pointer twice");
    }
    if (strcmp(first, second) != 0) {
        fail("qui_last_error's two copies disagree");
    }
    qui_free(first);
    qui_free(second);

    /* Freeing null is a documented no-op. */
    qui_free(NULL);

    /* A frame dump is caller-owned too. */
    char *screen = qui_screen(app);
    if (screen == NULL || strstr(screen, "Hello") == NULL) {
        fail("qui_screen did not return the painted frame");
    }
    qui_free(screen);

    /* `qui_error_name` is static storage: repeated calls return the very
     * same pointer, and it must never be passed to qui_free. */
    const char *name_once = qui_error_name(QUI_ERR_PANIC);
    const char *name_twice = qui_error_name(QUI_ERR_PANIC);
    if (name_once == NULL || name_once != name_twice) {
        fail("qui_error_name is not stable static storage");
    }
    if (qui_error_name(-9999) != NULL) {
        fail("qui_error_name named a code it does not define");
    }

    /* A slot can be emptied and refilled: the cleared descriptor stops
     * painting, and the slot accepts a different one. */
    expect_code(qui_slot_clear(app, QUI_SLOT_BUTTON), QUI_OK, "qui_slot_clear failed");
    expect_off_screen(app, "OK", "the cleared Button is still painted");
    expect_code(qui_slot_clear(app, QUI_SLOT_BUTTON), QUI_OK,
                "clearing an already-empty slot should be a no-op");
    expect_slot(qui_button_insert(app, BYTES(BUTTON_RETRY_JSON)), QUI_SLOT_BUTTON,
                "a cleared slot did not accept a new descriptor");
    expect_on_screen(app, "Retry", "the refilled Button is not painted");
}

/* Phase 5: the exit reaction. Once the app's callback asks to exit, the
 * app latches it and every later step reports it again. */
static void reach_the_exit_reaction(struct QuiApp *app) {
    g_exit_on_next_event = 1;
    expect_code(qui_push_key_event(app, (uint32_t)'q'), QUI_OK, "qui_push_key_event failed");
    expect_code(qui_step(app), QUI_STEP_EXITED, "the exit reaction was not reported");
    expect_handle_calls(4, "after the exit event");
    expect_code(qui_step(app), QUI_STEP_EXITED, "the exited app did not latch its exit");
    expect_code(qui_run(app, 4), QUI_STEP_EXITED, "qui_run did not report the latched exit");
}

int main(void) {
    struct QuiVTable vtable;
    memset(&vtable, 0, sizeof(vtable));
    vtable.user_data = NULL;
    vtable.handle = on_handle;
    vtable.tick = on_tick;

    /* Tall enough to stack all six primitives' fixed-height bands
     * (3+3+4+4+4+3 rows) without any of them falling off the bottom of
     * the viewport. */
    struct QuiApp *app = qui_app_new(40, 30, vtable);
    if (app == NULL) {
        fail("qui_app_new returned null");
    }

    build_the_screen(app);
    drive_the_input_surface(app);
    walk_the_error_taxonomy(app);
    exercise_memory_ownership(app);
    reach_the_exit_reaction(app);

    qui_app_free(app);

    printf("ok: six primitives painted, three input paths delivered, %d error paths walked, "
           "every callee-allocated string freed, and the exit reaction latched\n",
           g_error_paths);
    return 0;
}
