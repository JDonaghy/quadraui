//! First core smoke set for the macOS example-driver harness (quadraui#493)
//! — the macOS twin of `tests/gtk_example_driver.rs` / `tests/tui_example_driver.rs`.
//! Each test instantiates the *same* backend-agnostic `AppLogic` impl the
//! corresponding `macos_*` example runs, scripts real [`quadraui::UiEvent`]s
//! through it via [`MacDriver`], and asserts on the painted text — no
//! window, no `NSApplication`.
//!
//! Deliberately thin per the pattern the GTK/TUI suites already set ("a
//! few tests, not a big-bang suite") — coverage grows incrementally per
//! behaviour-changing issue. `PipelineApp` and `DataTableApp` are the two
//! `examples/common` shapes quadraui#493 asks for explicitly: the same
//! fixtures already covered on TUI/GTK in `tests/tui_example_driver.rs` /
//! `tests/gtk_example_driver.rs` and in `tests/cross_backend_parity.rs`'s
//! `pipeline_parity_macos_agrees_with_tui_and_gtk_on_logical_state`.
#![cfg(all(feature = "macos", target_os = "macos"))]

use quadraui::macos::testing::MacDriver;
use quadraui::Reaction;

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

#[path = "../examples/common/data_table_app.rs"]
mod data_table_app;
use data_table_app::DataTableApp;

// Point canvas — big enough for five pipeline stage boxes + arrow
// connectors + the bottom status bar at macOS's native (point, not cell)
// scale. Same size `tests/cross_backend_parity.rs` uses for its `MacDriver`
// row.
const W: u32 = 800;
const H: u32 = 480;

// ─── PipelineApp: initial paint, keyboard, mouse-routed click ──────────────

#[test]
fn pipeline_initial_screen_paints_stages_and_hint() {
    let driver = MacDriver::new(PipelineApp::new(), W, H);
    assert!(
        driver.screen_contains("Checkout"),
        "stage label should be painted: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Deploy"),
        "stage label should be painted: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Enter"),
        "status bar hint should be painted: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn pipeline_pressing_q_exits() {
    let mut driver = MacDriver::new(PipelineApp::new(), W, H);
    assert!(!driver.exited());
    driver.type_char('q');
    assert!(driver.exited(), "'q' should make the app exit");
}

#[test]
fn pipeline_pressing_r_resets_status_message() {
    let mut driver = MacDriver::new(PipelineApp::new(), W, H);
    driver.press_named(quadraui::NamedKey::Right);
    driver.type_char('r');
    assert!(
        driver.screen_contains("Reset"),
        "after 'r' the status bar should read Reset: {:?}",
        driver.painted_texts()
    );
}

/// A click round-trips paint -> hit_test -> handle -> state -> re-render,
/// with NO hardcoded coordinates: `find` locates the painted "Go"
/// (Deploy/stage-3) action button from the `text_runs` `MacBackend`
/// records at the `draw_text` choke point (quadraui#493).
#[test]
fn pipeline_clicking_a_stage_action_routes_the_click() {
    let mut driver = MacDriver::new(PipelineApp::new(), W, H);
    assert!(
        !driver.screen_contains("stage 3"),
        "status bar should not yet mention stage 3"
    );

    let (x, y) = driver
        .find("Go")
        .expect("Go action button should be painted with locatable bounds");
    let reaction = driver.click(x, y);

    assert_eq!(reaction, Reaction::Redraw, "click should trigger a redraw");
    // PipelineApp writes "Action on stage 3: Deploy" (action) or
    // "Selected stage 3: Deploy" (body) -- both name stage 3.
    assert!(
        driver.screen_contains("stage 3"),
        "clicking the Deploy action should update the status to mention stage 3: {:?}",
        driver.painted_texts()
    );
}

// ─── DataTableApp: initial paint, sort cycling, row selection ──────────────
//
// Pixel canvas big enough for all 20 pod rows + header + the 2-row footer
// band + status bar at once, at the nominal 8px char / 16px line metrics
// `MacDriver::new_fixture`/`MacBackend::new()` start with
// (`min_total_width = 80 * char_width` = 640px), with no scrolling.

const DT_W: u32 = 900;
const DT_H: u32 = 600;

#[test]
fn data_table_initial_screen_paints_headers_and_rows() {
    let driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    assert!(
        driver.screen_contains("Name"),
        "column header should be painted: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("nginx-7d9b8c66b-x2j4k"),
        "a pod row should be painted: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("sort: Name asc"),
        "status bar should report the default sort: {:?}",
        driver.painted_texts()
    );
}

/// `s` cycles the sort column (Name → Status → …); the status bar's
/// "sort: <column> asc/desc" segment is the observable proof, same
/// substring `DataTableApp::status_bar` always paints.
#[test]
fn data_table_pressing_s_cycles_sort_column() {
    let mut driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    assert!(driver.screen_contains("sort: Name asc"));
    driver.type_char('s');
    assert!(
        driver.screen_contains("sort: Status asc"),
        "after one 's' the status bar should read Status: {:?}",
        driver.painted_texts()
    );
}

/// `j` moves the row selection down one; the status bar's "row N / 20"
/// segment is the observable proof.
#[test]
fn data_table_pressing_j_moves_selection() {
    let mut driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    assert!(driver.screen_contains("row 1 / 20"));
    driver.type_char('j');
    assert!(
        driver.screen_contains("row 2 / 20"),
        "after one 'j' the status bar should read row 2 / 20: {:?}",
        driver.painted_texts()
    );
}
