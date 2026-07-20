//! First core smoke set for the GTK example-driver harness (quadraui#448,
//! GD-3) — the GTK twin of `tests/tui_example_driver.rs`. Each test
//! instantiates the *same* backend-agnostic `AppLogic` impl the
//! corresponding `gtk_*` example runs, scripts real [`quadraui::UiEvent`]s
//! through it via [`GtkDriver`], and asserts on the rendered surface — no
//! display, no `gtk::init`, no window (quadraui#446/#447, GD-1/GD-2).
//!
//! Deliberately thin per the issue's scope ("a few tests, not a big-bang
//! suite") — coverage grows incrementally per behaviour-changing issue,
//! same as the TUI suite. `PipelineApp` is the one already used as the
//! reference example in `docs/TESTING.md`'s cross-backend sample code, so
//! it doubles as the parity test's subject in
//! `tests/cross_backend_parity.rs`.
#![cfg(feature = "gtk")]

use quadraui::gtk::testing::GtkDriver;
use quadraui::{NamedKey, Reaction};

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

// Pixel canvas — big enough for five stage boxes + arrow connectors + the
// bottom status bar at GTK's native (pixel, not cell) scale.
const W: i32 = 800;
const H: i32 = 480;

// ─── PipelineApp: mouse + keyboard + reset ──────────────────────────────────

#[test]
fn pipeline_initial_screen_paints_stages_and_hint() {
    let driver = GtkDriver::new(PipelineApp::new(), W, H);
    assert!(
        driver.screen_contains("Checkout"),
        "stage label should be painted"
    );
    assert!(
        driver.screen_contains("Deploy"),
        "stage label should be painted"
    );
    assert!(
        driver.screen_contains("Enter"),
        "status bar hint should be painted"
    );
}

#[test]
fn pipeline_pressing_q_exits() {
    let mut driver = GtkDriver::new(PipelineApp::new(), W, H);
    assert!(!driver.exited());
    driver.type_char('q');
    assert!(driver.exited(), "'q' should make the app exit");
}

#[test]
fn pipeline_pressing_r_resets_status_message() {
    let mut driver = GtkDriver::new(PipelineApp::new(), W, H);
    driver.press_named(NamedKey::Right);
    driver.type_char('r');
    assert!(
        driver.screen_contains("Reset"),
        "after 'r' the status bar should read Reset"
    );
}

#[test]
fn pipeline_clicking_a_stage_action_routes_the_click() {
    // A click round-trips paint -> hit_test -> handle -> state -> re-render,
    // with NO hardcoded coordinates: `find` locates the painted "Go"
    // (Deploy/stage-3) action button from the same (text, bounds) map GD-2
    // added, extended to the pipeline view in this issue (GD-3).
    let mut driver = GtkDriver::new(PipelineApp::new(), W, H);
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
        "clicking the Deploy action should update the status to mention stage 3"
    );
}
