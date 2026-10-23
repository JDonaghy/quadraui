/* Tiny C smoke harness for the quadraui-ffi PoC.
 *
 * Exercises the whole round trip a foreign-language binding would:
 * construct one descriptor of each covered primitive type from JSON, drive
 * the app through `qui_run`/`qui_step`, and assert the rendered frame
 * actually painted each descriptor's content — proving the opaque-handle +
 * JSON-descriptor + vtable-callback + non-blocking-step shape works end to
 * end from a real C translation unit built against cbindgen's generated
 * header, not just from Rust-side tests. Covers `Button`, `TextDisplay`
 * (quadraui's closest match to a "Label"), `ListView`, `TreeView`,
 * `DataTable`, and `TabBar` — one `qui_<name>_insert` call each.
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

/* Asserts `needle` is somewhere in the app's current rendered frame,
 * failing with `what` otherwise. */
static void expect_on_screen(struct QuiApp *app, const char *needle, const char *what) {
    int32_t found = qui_screen_contains(app, (const uint8_t *)needle, strlen(needle));
    if (found != 1) {
        fail(what);
    }
}

int main(void) {
    struct QuiVTable vtable;
    memset(&vtable, 0, sizeof(vtable));
    vtable.user_data = NULL;
    vtable.handle = on_handle;
    vtable.tick = NULL;

    /* Tall enough to stack all six primitives' fixed-height bands
     * (3+3+4+4+4+3 rows) without any of them falling off the bottom of
     * the viewport. */
    struct QuiApp *app = qui_app_new(40, 30, vtable);
    if (app == NULL) {
        fail("qui_app_new returned null");
    }

    const char *button_json = "{\"id\":\"demo:ok\",\"label\":\"OK\"}";
    int64_t button_id = qui_button_insert(app, (const uint8_t *)button_json, strlen(button_json));
    if (button_id != 0) {
        fail("qui_button_insert did not return the expected single-slot id");
    }

    const char *text_display_json =
        "{\"id\":\"text:1\",\"lines\":[{\"spans\":[{\"text\":\"Hello\",\"fg\":null,\"bg\":null}]}]}";
    int64_t text_display_id =
        qui_text_display_insert(app, (const uint8_t *)text_display_json, strlen(text_display_json));
    if (text_display_id != 0) {
        fail("qui_text_display_insert did not return the expected single-slot id");
    }

    const char *list_json =
        "{\"id\":\"list:1\",\"items\":[{\"text\":{\"spans\":[{\"text\":\"Row 1\",\"fg\":null,"
        "\"bg\":null}]}}],\"selected_idx\":0}";
    int64_t list_id = qui_list_insert(app, (const uint8_t *)list_json, strlen(list_json));
    if (list_id != 0) {
        fail("qui_list_insert did not return the expected single-slot id");
    }

    const char *tree_json =
        "{\"id\":\"tree:1\",\"rows\":[{\"path\":[0],\"indent\":0,\"icon\":null,\"text\":{\"spans\":"
        "[{\"text\":\"Alpha\",\"fg\":null,\"bg\":null}]},\"badge\":null,\"is_expanded\":null}],"
        "\"selection_mode\":\"Single\",\"selected_path\":null,\"style\":{\"indent\":2,"
        "\"show_chevrons\":true,\"chevron_expanded\":\"v\",\"chevron_collapsed\":\">\"}}";
    int64_t tree_id = qui_tree_insert(app, (const uint8_t *)tree_json, strlen(tree_json));
    if (tree_id != 0) {
        fail("qui_tree_insert did not return the expected single-slot id");
    }

    const char *data_table_json =
        "{\"id\":\"table:1\",\"columns\":[{\"title\":\"Name\"}],\"rows\":[{\"cells\":[{\"spans\":"
        "[{\"text\":\"Row1\",\"fg\":null,\"bg\":null}]}]}]}";
    int64_t data_table_id =
        qui_data_table_insert(app, (const uint8_t *)data_table_json, strlen(data_table_json));
    if (data_table_id != 0) {
        fail("qui_data_table_insert did not return the expected single-slot id");
    }

    const char *tab_bar_json = "{\"id\":\"tabbar:1\",\"tabs\":[{\"label\":\"Tab 1\"}]}";
    int64_t tab_bar_id = qui_tab_bar_insert(app, (const uint8_t *)tab_bar_json, strlen(tab_bar_json));
    if (tab_bar_id != 0) {
        fail("qui_tab_bar_insert did not return the expected single-slot id");
    }

    expect_on_screen(app, "OK", "rendered frame does not contain the Button's label");
    expect_on_screen(app, "Hello", "rendered frame does not contain the TextDisplay's line");
    expect_on_screen(app, "Row 1", "rendered frame does not contain the ListView's item");
    expect_on_screen(app, "Alpha", "rendered frame does not contain the TreeView's row");
    expect_on_screen(app, "Row1", "rendered frame does not contain the DataTable's cell");
    expect_on_screen(app, "Tab 1", "rendered frame does not contain the TabBar's tab");

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

    printf("ok: Button, TextDisplay, ListView, TreeView, DataTable and TabBar descriptors "
           "round-tripped through qui_run/qui_step\n");
    return 0;
}
