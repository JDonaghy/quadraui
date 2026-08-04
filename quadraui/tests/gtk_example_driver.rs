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

use quadraui::gtk::testing::{driver_with_shell, GtkDriver};
use quadraui::{NamedKey, Reaction};

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
use appshell_demo::AppShellDemo;

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

// ─── AppShellDemo: `driver_with_shell` (ShellApp) coverage (quadraui#518) ───
//
// `AppShellDemo` implements `ShellApp` (not `AppLogic` directly) and is
// driven by `gtk::shell_runner::run_with_shell` in production.
// `driver_with_shell` builds the identical `ShellAdapter` stack — through
// the same `gtk::shell_runner::build_shell_adapter` factory `run_with_shell`
// calls — then scripts events through it headlessly. Before quadraui#518
// there was no way for a `ShellApp` consumer to reach `GtkDriver` at all;
// these are the GTK twins of `tests/tui_example_driver.rs`'s
// `appshell_demo_*` tests.

// Pixel canvas sized for the shell chrome (activity bar + sidebar + main
// content), distinct from the pipeline canvas above.
const SHELL_W: i32 = 800;
const SHELL_H: i32 = 480;

/// The initial frame paints real shell chrome — the sidebar header for the
/// default-active panel and the app's own content — asserted via
/// `find_bounds`/`painted_texts` (real painted geometry), not just "it did
/// not panic".
#[test]
fn appshell_demo_renders_shell_chrome_via_driver_with_shell() {
    let config = AppShellDemo::config();
    let driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    let bounds = driver
        .find_bounds("EXPLORER")
        .expect("sidebar header for the default-active panel should be painted");
    assert!(
        bounds.width > 0.0 && bounds.height > 0.0,
        "sidebar header bounds should be non-empty: {bounds:?}"
    );
    assert!(
        driver
            .painted_texts()
            .iter()
            .any(|t| t.contains("Tab=focus bar")),
        "main content hint should be painted: {:?}",
        driver.painted_texts()
    );
}

/// `Tab` focuses the activity bar, `j` `j` moves the keyboard cursor down
/// two items (explorer → search → git), and `Enter` activates the
/// selection — switching the *real* `AppShell` panel, visible as the
/// sidebar header flipping from "EXPLORER" to "SOURCE CONTROL".
///
/// This is the acceptance criterion that the GTK test path and the live
/// `gtk::run` path build the adapter through the same function: the
/// ActivityBar keyboard-focus intercept lives in `gtk::run::dispatch_event`
/// (shared by `GtkDriver::dispatch`) and reads `ShellAdapter` state built by
/// `build_shell_adapter` — if `driver_with_shell` constructed that adapter
/// differently than `run_with_shell` does, this round trip would desync
/// exactly the way vimcode's Ctrl+B bug (#454) did for the sidebar toggle.
#[test]
fn appshell_demo_tab_focus_then_jj_enter_switches_panel() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    assert!(
        driver.find_bounds("EXPLORER").is_some(),
        "starts on the default (index 0) Explorer panel"
    );

    let reaction = driver.press_named(NamedKey::Tab);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Tab should request activity-bar keyboard focus and redraw"
    );
    assert!(
        driver.screen_contains("Activity bar focused"),
        "focusing the bar should update the status hint: {:?}",
        driver.painted_texts()
    );

    // Cursor starts at index 0 (explorer). Two `j` presses move it to
    // index 2 (git) — 3 top panels, cursor saturates instead of wrapping.
    for _ in 0..2 {
        let reaction = driver.type_char('j');
        assert_eq!(
            reaction,
            Reaction::Redraw,
            "'j' while focused must be intercepted as ActivityBar nav, not fall through"
        );
    }

    let reaction = driver.press_named(NamedKey::Enter);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Enter should activate the selected item and redraw"
    );

    let bounds = driver
        .find_bounds("SOURCE CONTROL")
        .expect("sidebar header must switch to the git panel's real title");
    assert!(bounds.width > 0.0 && bounds.height > 0.0);
    assert!(
        driver.find_bounds("EXPLORER").is_none(),
        "the stale Explorer header must not still be painted"
    );
    assert!(
        driver.screen_contains("Panel: panel:git"),
        "on_shell_event(PanelChanged) must fire for a keyboard-driven switch, \
         mirroring the TUI path: {:?}",
        driver.painted_texts()
    );
}

/// #454's fix, proven on GTK: `ctx.shell_mut()` reaches the real `AppShell`
/// instance `ShellAdapter` renders, so `Ctrl+B` hides/shows the sidebar
/// `driver_with_shell` actually painted — not a shadow copy. Mirrors
/// `tests/tui_example_driver.rs`'s
/// `appshell_demo_ctrl_b_toggles_the_real_rendered_sidebar`.
#[test]
fn appshell_demo_ctrl_b_toggles_the_real_rendered_sidebar() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    assert!(
        driver.find_bounds("(sidebar content here)").is_some(),
        "sidebar should be visible on the initial screen"
    );

    let reaction = driver.ctrl_char('b');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Ctrl+B toggling the real AppShell must redraw"
    );
    assert!(
        driver.find_bounds("(sidebar content here)").is_none(),
        "Ctrl+B must hide the sidebar that ShellAdapter actually renders, not a shadow copy"
    );
    assert!(
        driver.screen_contains("Sidebar hidden (Ctrl+B via ctx.shell_mut())"),
        "status line should confirm the toggle went through ctx.shell_mut()"
    );

    let reaction = driver.ctrl_char('b');
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.find_bounds("(sidebar content here)").is_some(),
        "a second Ctrl+B should show the sidebar again"
    );
    assert!(
        driver.screen_contains("Sidebar shown (Ctrl+B via ctx.shell_mut())"),
        "status line should confirm the second toggle"
    );
}
