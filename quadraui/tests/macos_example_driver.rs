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

use quadraui::macos::testing::{driver_with_shell, MacDriver};
use quadraui::{NamedKey, Reaction};

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
use appshell_demo::AppShellDemo;

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

// ─── AppShellDemo: `driver_with_shell` (ShellApp) coverage (#465) ──────────
//
// `AppShellDemo` implements `ShellApp` (not `AppLogic` directly) and is
// driven by `macos::shell_runner::run_with_shell` in production.
// `driver_with_shell` builds the identical `ShellAdapter` stack — through
// the same `build_shell_adapter` factory `run_with_shell` calls — then
// scripts events through it headlessly, with no `NSApplication` / window.
// These are the macOS twins of `tests/tui_example_driver.rs` /
// `tests/gtk_example_driver.rs`'s `appshell_demo_*` tests, proving #465's
// `macos::shell_runner` reaches the same `ShellAdapter` composition the
// other two backends already do.

// Point canvas sized for the shell chrome (activity bar + sidebar + main
// content), distinct from the pipeline canvas above.
const SHELL_W: u32 = 800;
const SHELL_H: u32 = 480;

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
/// This is the acceptance criterion that the macOS test path and the live
/// `macos::run` path build the adapter through the same function: the
/// ActivityBar keyboard-focus intercept lives in `macos::run::dispatch_event`
/// (shared by `MacDriver::dispatch`) and reads `ShellAdapter` state built by
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
        "on_shell_event_ctx(PanelChanged) must fire for a keyboard-driven switch, \
         mirroring the TUI/GTK paths: {:?}",
        driver.painted_texts()
    );
}

/// #454's fix, proven on macOS: `ctx.shell_mut()` reaches the real `AppShell`
/// instance `ShellAdapter` renders, so `Ctrl+B` hides/shows the sidebar
/// `driver_with_shell` actually painted — not a shadow copy. Mirrors
/// `tests/tui_example_driver.rs` / `tests/gtk_example_driver.rs`'s
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

/// #516 defect 3: the same divider-drag script the TUI and GTK driver
/// tests run (`tests/tui_example_driver.rs`'s
/// `data_table_divider_before_last_column_resizes_in_drag_direction`,
/// `tests/gtk_example_driver.rs`'s
/// `data_table_divider_before_last_column_widens_on_right_drag`), run
/// against macOS — dragging the divider immediately before the last
/// column (Age | Restarts) must move Age's width in the *drag's*
/// direction, never inverted.
///
/// The point of running it here too is that the fix lives in the shared
/// `primitives::data_table::resolve_columns`, so every rasteriser must
/// agree; `DataTableApp::resolved_column_widths` reads the width back
/// through the very same `DataTable::layout` the macOS backend paints
/// through, so nothing per-backend is hardcoded.
#[test]
fn data_table_divider_before_last_column_resizes_in_drag_direction() {
    // Widen: drag the Age|Restarts divider right. `Restarts` is
    // `Fixed(10.0)` and `DataTableApp` pair-resizes with a 4.0 floor, so
    // the achievable widening is small — the assertion is on direction,
    // not magnitude, exactly as on TUI/GTK.
    let mut driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    let before = driver.app().resolved_column_widths(driver.backend())[2];

    let layout = driver.app().table_layout(driver.backend());
    let age = layout.columns[2];
    let divider_x = age.x + age.width;
    let divider_y = layout.header_height / 2.0;
    driver.drag(divider_x, divider_y, divider_x + 80.0, divider_y);

    let widened = driver.app().resolved_column_widths(driver.backend())[2];
    assert!(
        widened > before,
        "dragging the divider before the last column right should widen it: \
         before={before}, after={widened}"
    );

    // Narrow: a *fresh* driver rather than a second drag on this one —
    // a second `mouse_down` near the same point would fold into a
    // synthetic `DoubleClick` (`DoubleClickDetector`, radius 1.5) in
    // `macos::run::dispatch_event` instead of starting a fresh resize
    // drag. Age's natural width here is ~1/10 of the 900pt viewport, so
    // a 40pt leftward drag lands well clear of the 4.0 floor and the
    // direction assertion is a real one rather than a clamp artifact.
    let mut driver = MacDriver::new(DataTableApp::new(), DT_W, DT_H);
    let natural = driver.app().resolved_column_widths(driver.backend())[2];
    assert!(
        natural > 44.0,
        "test precondition: Age's natural width ({natural}) must leave room \
         to narrow by 40pt without hitting the 4.0 pair-resize floor"
    );

    let layout = driver.app().table_layout(driver.backend());
    let age = layout.columns[2];
    let divider_x = age.x + age.width;
    let divider_y = layout.header_height / 2.0;
    driver.drag(divider_x, divider_y, divider_x - 40.0, divider_y);

    let narrowed = driver.app().resolved_column_widths(driver.backend())[2];
    assert!(
        narrowed < natural,
        "dragging the divider before the last column left should narrow it: \
         before={natural}, after={narrowed}"
    );
}
