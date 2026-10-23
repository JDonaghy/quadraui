/* Tiny C smoke harness for the quadraui-ffi PoC.
 *
 * Exercises the whole round trip a foreign-language binding would:
 * construct one `Button` descriptor from JSON, drive it through
 * `qui_run`/`qui_step`, and assert the rendered frame actually painted the
 * button's label — proving the opaque-handle + JSON-descriptor +
 * vtable-callback + non-blocking-step shape works end to end from a real
 * C translation unit built against cbindgen's generated header, not just
 * from Rust-side tests.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "quadraui_ffi.h"

static int g_handle_calls = 0;

static int32_t on_handle(void *user_data, const uint8_t *event_json, uintptr_t len) {
    (void)user_data;
    (void)event_json;
    (void)len;
    g_handle_calls += 1;
    return 0; /* Reaction::Continue */
}

static void fail(const char *what) {
    fprintf(stderr, "FAIL: %s\n", what);
    char *err = qui_last_error();
    if (err != NULL) {
        fprintf(stderr, "  last error: %s\n", err);
        qui_free(err);
    }
    exit(1);
}

int main(void) {
    struct QuiVTable vtable;
    memset(&vtable, 0, sizeof(vtable));
    vtable.user_data = NULL;
    vtable.handle = on_handle;
    vtable.tick = NULL;

    struct QuiApp *app = qui_app_new(40, 5, vtable);
    if (app == NULL) {
        fail("qui_app_new returned null");
    }

    const char *json = "{\"id\":\"demo:ok\",\"label\":\"OK\"}";
    int64_t inserted = qui_button_insert(app, (const uint8_t *)json, strlen(json));
    if (inserted != 0) {
        fail("qui_button_insert did not return the expected single-slot id");
    }

    const char *needle = "OK";
    int32_t found = qui_screen_contains(app, (const uint8_t *)needle, strlen(needle));
    if (found != 1) {
        fail("rendered frame does not contain the Button's label");
    }

    if (qui_push_key_event(app, (uint32_t)'x') != 0) {
        fail("qui_push_key_event failed");
    }
    int32_t step_result = qui_run(app, 4);
    if (step_result < 0) {
        fail("qui_run reported an error");
    }
    if (g_handle_calls != 1) {
        fprintf(stderr, "FAIL: expected exactly 1 handle callback, got %d\n", g_handle_calls);
        exit(1);
    }

    qui_app_free(app);

    printf("ok: Button descriptor round-tripped through qui_run/qui_step\n");
    return 0;
}
