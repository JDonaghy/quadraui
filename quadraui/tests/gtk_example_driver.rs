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
use quadraui::testing::ConformanceDriver;
use quadraui::{Key, Modifiers, MouseButton, NamedKey, Point, Reaction, Theme, UiEvent, WidgetId};

#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
use pipeline_app::PipelineApp;

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
use appshell_demo::AppShellDemo;

#[path = "../examples/common/data_table_app.rs"]
mod data_table_app;
use data_table_app::DataTableApp;

#[path = "../examples/common/toolbar_app.rs"]
mod toolbar_app;
use toolbar_app::ToolbarApp;

#[path = "../examples/common/shell_app.rs"]
mod shell_app;
use shell_app::ShellApp;

#[path = "../examples/common/menu_bar_app.rs"]
mod menu_bar_app;
use menu_bar_app::MenuBarApp;

#[path = "../examples/common/workspace_demo.rs"]
#[allow(dead_code)]
mod workspace_demo;
use workspace_demo::WorkspaceDemo;

#[path = "../examples/common/split_app.rs"]
mod split_app;
use split_app::SplitApp;

#[path = "../examples/common/sidebar_panel_body_demo.rs"]
mod sidebar_panel_body_demo;
use sidebar_panel_body_demo::SidebarPanelBodyDemo;

#[path = "../examples/common/context_menu_style_demo.rs"]
mod context_menu_style_demo;
use context_menu_style_demo::ContextMenuStyleDemo;

#[path = "../examples/common/submenu_app.rs"]
mod submenu_app;
use submenu_app::SubmenuApp;

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

/// Regression test for issue #996: the sidebar/editor resize divider used
/// to be painted as N stacked one-row `StatusBar`s, and GTK's
/// `draw_status_bar_interactive` fills only `current_line_height`
/// regardless of the row rect's own height — so on GTK every row painted
/// short of its own pitch and the gaps between rows rendered as a dashed
/// line down the divider's full height. Sample pixel colors down the
/// divider's registered zone (real geometry, not a hardcoded coordinate)
/// and assert they're all identical, i.e. one solid fill top to bottom.
#[test]
fn appshell_demo_resize_divider_is_solid_with_no_gaps() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    let divider = driver
        .inventory()
        .zones()
        .iter()
        .find(|z| z.id == WidgetId::new("app-shell:divider"))
        .expect("AppShell::render should register the divider's chrome zone")
        .bounds;
    assert!(
        divider.height > 8.0,
        "divider should span most of the shell height, got {divider:?}"
    );

    let x = (divider.x + divider.width / 2.0).round() as i32;
    let top = divider.y.round() as i32 + 1;
    let bottom = (divider.y + divider.height).round() as i32 - 1;

    let mut colors = std::collections::HashSet::new();
    let mut y = top;
    while y < bottom {
        colors.insert(driver.pixel(x, y));
        y += 2;
    }

    assert_eq!(
        colors.len(),
        1,
        "divider must be one solid color top to bottom (a dashed divider \
         samples more than one color across its height); sampled: {colors:?}"
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
        "on_shell_event_ctx(PanelChanged) must fire for a keyboard-driven switch, \
         mirroring the TUI path: {:?}",
        driver.painted_texts()
    );
}

/// #1055, proven on GTK: a bottom item (the Settings gear) can own the
/// sidebar header once its `BottomItemClicked` handler calls `show_panel` —
/// before the fix, `show_panel` silently no-opped for a bottom item's id,
/// so GTK's header stayed captioned "EXPLORER" no matter what the sidebar
/// actually showed (vimcode#1258/#1343's exact symptom). Mirrors
/// `tests/tui_example_driver.rs`'s
/// `appshell_demo_bottom_item_show_panel_retitles_sidebar_header`.
#[test]
fn appshell_demo_bottom_item_show_panel_retitles_sidebar_header() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    assert!(
        driver.find_bounds("EXPLORER").is_some(),
        "starts on the default (index 0) Explorer panel"
    );

    // Tab focuses the activity bar; three `j`s move the keyboard cursor
    // from Explorer (index 0) past Search/Git (1, 2) to the Settings
    // bottom item (combined index 3 — the first item past the 3 top
    // panels), same cursor arithmetic as
    // `appshell_demo_tab_focus_then_jj_enter_switches_panel`'s git case.
    driver.press_named(NamedKey::Tab);
    for _ in 0..3 {
        let reaction = driver.type_char('j');
        assert_eq!(
            reaction,
            Reaction::Redraw,
            "'j' while focused must be intercepted as ActivityBar nav"
        );
    }
    let reaction = driver.press_named(NamedKey::Enter);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "activating the Settings bottom item must redraw"
    );

    let bounds = driver
        .find_bounds("Settings")
        .expect("sidebar header must retitle to the bottom item that now owns it");
    assert!(bounds.width > 0.0 && bounds.height > 0.0);
    assert!(
        driver.find_bounds("EXPLORER").is_none(),
        "the stale Explorer header must not still be painted once Settings owns the sidebar"
    );
    assert!(
        driver.screen_contains("Bottom: panel:settings"),
        "on_shell_event_ctx(BottomItemClicked) must still fire as before: {:?}",
        driver.painted_texts()
    );

    // #1055 follow-up (review): now navigate back to a top panel through
    // the real keyboard activity-bar path (not `show_panel` directly) and
    // confirm the header reclaims from Settings. Before the fix,
    // `handle_activity_click`'s top-panel arm never cleared
    // `sidebar_bottom_owner`, so the header stayed stuck on "Settings"
    // forever after this point.
    //
    // Activating an item turns keyboard focus back off
    // (`ShellAdapter::handle`'s `"l" | "Enter" | ...` arm), so `Tab` is
    // needed to refocus the bar — which also resets the cursor to combined
    // index 0 (Explorer, `ShellAdapter::handle`'s
    // `take_activity_focus_requested()` block). `active_panel` was never
    // moved away from Explorer (only `show_panel` touched Settings), so
    // this also exercises the second symptom from the review: reclaiming
    // the *same* `active_panel` index that was active before the bottom
    // item took over must switch the header back to it, not hit the
    // toggle-hide branch and hide the sidebar.
    let reaction = driver.press_named(NamedKey::Tab);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Tab should request activity-bar keyboard focus and redraw"
    );
    let reaction = driver.press_named(NamedKey::Enter);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "activating Explorer from the activity bar must redraw"
    );

    let bounds = driver
        .find_bounds("EXPLORER")
        .expect("sidebar header must reclaim from the Settings bottom item");
    assert!(bounds.width > 0.0 && bounds.height > 0.0);
    assert!(
        driver.find_bounds("Settings").is_none(),
        "the stale Settings header must not still be painted once Explorer \
         reclaims the sidebar: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Panel: panel:explorer"),
        "on_shell_event_ctx(PanelChanged) must fire for the reclaiming switch, \
         not a SidebarHidden event: {:?}",
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

// ─── DataTableApp: body clipping, separators, resize direction (#516) ──────
//
// `DataTableApp` had zero GTK coverage before this issue. Pixel canvas big
// enough (900x600) for all 20 pod rows + header + the 2-row footer band +
// status bar at once, at GTK's default 8px char / 16px line metrics
// (`min_total_width = 80 * char_width` = 640px), with no scrolling.

const DT_W: i32 = 900;
const DT_H: i32 = 600;

/// #516 defect 1 (GTK guard, app level): the "Status" column (index 1, a
/// *middle* column) holds a value far wider than its resolved share.
/// `src/gtk/data_table.rs`'s own tests already pin the `cr.clip()`
/// behaviour in isolation; this proves the same thing through the real
/// app + the painted-text map this issue adds to `GtkBackend::draw_data_table`
/// — a corrupted/merged cell would show up here as a missing or mangled
/// `painted_texts()` entry, not just a pixel discrepancy.
#[test]
fn data_table_wide_middle_column_leaves_neighbours_intact_in_painted_texts() {
    let driver = GtkDriver::new(DataTableApp::new(), DT_W, DT_H);
    assert!(
        driver.screen_contains("grafana"),
        "wide-status pod row should be painted: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver
            .painted_texts()
            .iter()
            .any(|t| t.contains("ImagePullBackOff waiting for registry retry backoff window")),
        "the full over-long Status value should still be recorded intact (GTK clips visually \
         via cr.clip(), not by truncating the underlying text): {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.find_bounds("1h").is_some(),
        "Age cell for the wide-status row should be painted as its own intact entry: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.find_bounds("14").is_some(),
        "Restarts cell for the wide-status row should be painted as its own intact entry: {:?}",
        driver.painted_texts()
    );
}

/// #516 defect 2 (GTK): body rows previously drew no separator at all.
#[test]
fn data_table_body_rows_draw_separators_at_same_x_as_header() {
    let mut driver = GtkDriver::new(DataTableApp::new(), DT_W, DT_H);
    let layout = driver.app().table_layout(driver.backend());
    assert!(
        layout.columns.len() > 1,
        "sanity: table should have more than one column"
    );

    let header_y = (layout.header_height / 2.0) as i32;
    // Row 1, not row 0: `DataTableApp` starts with row 0 selected, and
    // the selection highlight tints the row background under the
    // separator's antialiased blend — comparing against a differently
    // -tinted body row would fail even with the fix correctly applied.
    let body_y = (layout.header_height + layout.row_height * 1.5) as i32;

    // Antialiasing rasterizes the header's and body's separator rects
    // independently (different heights: `header_height` vs `line_height`),
    // which can land a shared boundary pixel's blend fraction a shade of
    // a channel apart (observed: off by 1/255) even though both are
    // unmistakably "the separator colour blended over the row
    // background" and not "background" or "text ink" — so compare with
    // a small tolerance rather than bit-exact equality.
    fn close(a: (u8, u8, u8), b: (u8, u8, u8), tol: i32) -> bool {
        (a.0 as i32 - b.0 as i32).abs() <= tol
            && (a.1 as i32 - b.1 as i32).abs() <= tol
            && (a.2 as i32 - b.2 as i32).abs() <= tol
    }

    for col_idx in 0..layout.columns.len() - 1 {
        let col = layout.columns[col_idx];
        let sep_x = (col.x + col.width) as i32;
        let header_px = driver.pixel(sep_x, header_y);
        let body_px = driver.pixel(sep_x, body_y);
        assert!(
            close(body_px, header_px, 3),
            "column {col_idx}'s body separator should sit at the same x={sep_x} as the \
             header's: header pixel {header_px:?}, body pixel {body_px:?}"
        );
    }
}

// ─── ToolbarApp / ShellApp / MenuBarApp: unblocked by quadraui#489 ──────────
//
// These three apps had zero GTK coverage before #489, and could not have
// had any: every assertion below needs `find`/`screen_contains` to see
// text painted by a rasteriser that recorded nothing into
// `GtkBackend::painted_text` (toolbar buttons, tree rows, menu-bar
// items). With the paint-time recorder in place they are straight twins
// of the TUI suite's scripts. Breadth-first coverage of the recorder
// itself lives in `tests/gtk_painted_text_coverage.rs`; these are the
// behavioural round trips.

/// GTK twin of `tui_example_driver.rs`'s
/// `toolbar_initial_screen_paints_action_buttons`.
#[test]
fn toolbar_initial_screen_paints_action_buttons() {
    let driver = GtkDriver::new(ToolbarApp::new(), W, H);
    let painted = driver.painted_texts();
    for label in ["Continue", "Pause", "Filter", "Reset"] {
        assert!(
            driver.screen_contains(label),
            "{label} button should be painted: {painted:?}"
        );
    }
    // "Debug" is permanently disabled but must still be painted (dimmed).
    assert!(
        driver.screen_contains("Debug"),
        "disabled Debug button should still be painted: {painted:?}"
    );
}

/// GTK twin of `tui_example_driver.rs`'s
/// `toolbar_click_fires_action_without_focus`: a click round-trips paint
/// → hit_test → handle → state → re-render with **no hardcoded
/// coordinates**, on a primitive whose labels only became locatable in
/// #489.
#[test]
fn toolbar_clicking_filter_toggles_it_without_keyboard_focus() {
    let mut driver = GtkDriver::new(ToolbarApp::new(), W, H);
    // Two deliberate single clicks at the same spot, back to back, with
    // no real GDK press-count or wall-clock gap between them — without
    // this, `crate::dispatch::DoubleClickDetector` (quadraui#813) could
    // fold the second press into a `DoubleClick` instead of a second
    // `MouseDown`, and `ToolbarApp` doesn't bind `DoubleClick` at all.
    // Mirrors `tui_example_driver.rs`'s identical
    // `set_double_click_folding(false)` calls for the same reason.
    driver.set_double_click_folding(false);
    assert!(
        !driver.screen_contains("Filter on"),
        "filter should start off: {:?}",
        driver.painted_texts()
    );

    let (x, y) = driver
        .find("Filter")
        .expect("Filter toolbar button should be painted with locatable bounds");
    // `ToolbarApp` follows the press-then-release click contract (fire
    // only if the release lands on the same button as the press), so this
    // also proves the recorded label rect sits inside the button's hit
    // region for *both* events, not just one.
    driver.mouse_down(x, y);
    let reaction = driver.mouse_up(x, y);

    assert_eq!(reaction, Reaction::Redraw, "click should trigger a redraw");
    assert!(
        driver.screen_contains("Filter on"),
        "clicking Filter should flip it on and update the status: {:?}",
        driver.painted_texts()
    );

    // Same coordinates, second click — toggles back off.
    driver.mouse_down(x, y);
    driver.mouse_up(x, y);
    assert!(
        driver.screen_contains("Filter off"),
        "a second click should toggle the filter back off: {:?}",
        driver.painted_texts()
    );
}

/// `ShellApp`'s sidebar rows are painted by `draw_tree`. Clicking one by
/// text (not by coordinate) must select it, which the main-content label
/// echoes — the shell round trip the GTK suite could not assert before
/// #489.
#[test]
fn shell_app_clicking_a_sidebar_tree_row_selects_it() {
    let mut driver = GtkDriver::new(ShellApp::new(), SHELL_W, SHELL_H);
    assert!(
        driver.screen_contains("Selected: nothing selected"),
        "nothing should be selected on the initial frame: {:?}",
        driver.painted_texts()
    );

    let (x, y) = driver
        .find("backend.rs")
        .expect("sidebar tree row should be painted with locatable bounds");
    let reaction = driver.click(x, y);

    assert_eq!(
        reaction,
        Reaction::Redraw,
        "clicking a sidebar row should redraw"
    );
    assert!(
        !driver.screen_contains("Selected: nothing selected"),
        "the click should have selected a row: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Selected: section"),
        "the main-content label should name the selected section/row: {:?}",
        driver.painted_texts()
    );
}

/// `MenuBarApp`'s items are painted by `draw_menu_bar`. Clicking one by
/// text opens its dropdown — and the dropdown's own items become
/// locatable in the same map, so the whole menu interaction is now
/// scriptable coordinate-free on GTK.
#[test]
fn menu_bar_clicking_an_item_opens_its_dropdown() {
    let mut driver = GtkDriver::new(MenuBarApp::new(), W, H);
    assert!(
        driver.screen_contains("menu closed"),
        "no menu should be open initially: {:?}",
        driver.painted_texts()
    );

    let (x, y) = driver
        .find("View")
        .expect("menu bar item should be painted with locatable bounds");
    let reaction = driver.click(x, y);

    assert_eq!(reaction, Reaction::Redraw, "opening a menu should redraw");
    assert!(
        driver.screen_contains("menu open"),
        "clicking the View item should open its dropdown: {:?}",
        driver.painted_texts()
    );
}

/// #516 defect 3: same script as the TUI driver test, run against GTK —
/// dragging the divider immediately before the last column (Age |
/// Restarts) must widen Age when dragged right.
#[test]
fn data_table_divider_before_last_column_widens_on_right_drag() {
    let mut driver = GtkDriver::new(DataTableApp::new(), DT_W, DT_H);

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
}

// ─── WorkspaceDemo: `WorkspaceController` inside an AppShell panel (#596) ───
//
// GTK twin of `tests/tui_example_driver.rs`'s workspace set. Same
// `ShellApp`, so the controller is driven through the Pango-measured
// tab-bar rasteriser — which, unlike the TUI one, reports its own
// `correct_scroll_offset` and therefore exercises `WorkspaceController::render`'s
// second, corrected paint pass. Kept deliberately thin per this file's
// scope note; the exhaustive behaviour matrix lives on the TUI side and in
// `compose::workspace`'s own unit tests.

fn workspace_bar_id() -> quadraui::WidgetId {
    quadraui::WidgetId::new(workspace_demo::BAR_ID)
}

#[test]
fn workspace_paints_a_tab_per_document_and_the_active_body() {
    let driver = driver_with_shell(WorkspaceDemo::new(), WorkspaceDemo::config(), W, H);
    for (_, label) in workspace_demo::INITIAL {
        assert!(
            driver.screen_contains(label),
            "every open document gets a tab ({label} missing)"
        );
    }
    assert!(
        driver.screen_contains("viewing: doc:alpha"),
        "the host paints the active document's body itself"
    );
}

#[test]
fn workspace_clicking_a_tab_activates_that_document() {
    // No hardcoded pixels: the click target comes from the `TabBarLayout`
    // the GTK rasteriser cached for this bar on the last paint.
    let mut driver = driver_with_shell(WorkspaceDemo::new(), WorkspaceDemo::config(), W, H);
    let (x, y) = driver
        .tab_center(&workspace_bar_id(), 1)
        .expect("tab 1 should have painted geometry");
    let reaction = driver.click(x, y);

    assert_eq!(reaction, Reaction::Redraw, "click should trigger a redraw");
    assert!(
        driver.screen_contains("viewing: doc:beta"),
        "clicking tab 1's body activates it"
    );
}

#[test]
fn workspace_clicking_close_glyph_closes_and_activates_the_right_neighbour() {
    let mut driver = driver_with_shell(WorkspaceDemo::new(), WorkspaceDemo::config(), W, H);
    let (x, y) = driver
        .tab_close_center(&workspace_bar_id(), 0)
        .expect("tab 0 should paint a close glyph");
    driver.click(x, y);

    assert!(
        driver.screen_contains("closed doc:alpha"),
        "clicking the × must close, not merely activate"
    );
    assert!(
        driver.screen_contains("viewing: doc:beta"),
        "closing the active document activates its right-hand neighbour"
    );
}

#[test]
fn workspace_ctrl_tab_cycles_and_wraps() {
    let mut driver = driver_with_shell(WorkspaceDemo::new(), WorkspaceDemo::config(), W, H);
    for expected in ["doc:beta", "doc:gamma", "doc:alpha"] {
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        });
        assert!(
            driver.screen_contains(&format!("viewing: {expected}")),
            "Ctrl+Tab should step (and wrap) to {expected}"
        );
    }
}

// ─── SplitApp: the shared `PaintSurface` divider paint (#864) ───────────────
//
// Driver-tier cover for `primitives::split::native_surface_paint::paint`,
// the one shared implementation that replaced `gtk::draw_split`'s,
// `macos::split::draw_split`'s and `win::split::draw_split`'s three
// separate divider fills (#864, `PaintSurface` Phase 2d slice 7/9, child
// of #811). The macOS and Windows halves of that path have their own
// in-crate pixel tests, but neither compiles on the Linux `gtk` CI leg —
// these are the tests that actually run there, driving the *same*
// backend-agnostic `SplitApp` the `gtk_split` example runs through
// `GtkBackend::draw_split` -> the shared paint, and asserting on real
// Cairo pixels rather than on a recorded call list.
//
// `Split` paints no text at all (see `GtkDriver::painted_texts`' doc), so
// `find`/`screen_contains` can't locate the divider the way every other
// test in this file locates its target. Scanning for the divider's own
// painted colour is the equivalent move — still derived from the surface,
// never a hardcoded coordinate.

/// Longest contiguous run of `theme.separator`-coloured pixels along row
/// `y`, as `(start_x, len)`. `None` if the row has none.
fn separator_run_in_row(driver: &mut GtkDriver<SplitApp>, y: i32) -> Option<(i32, i32)> {
    let sep = Theme::default().separator;
    let mut best: Option<(i32, i32)> = None;
    let mut run_start: Option<i32> = None;
    for x in 0..=W {
        let hit = x < W && driver.pixel(x, y) == (sep.r, sep.g, sep.b);
        match (hit, run_start) {
            (true, None) => run_start = Some(x),
            (false, Some(s)) => {
                let len = x - s;
                if best.is_none_or(|(_, bl)| len > bl) {
                    best = Some((s, len));
                }
                run_start = None;
            }
            _ => {}
        }
    }
    best
}

/// Column twin of [`separator_run_in_row`] — `(start_y, len)` down `x`.
fn separator_run_in_col(driver: &mut GtkDriver<SplitApp>, x: i32) -> Option<(i32, i32)> {
    let sep = Theme::default().separator;
    let mut best: Option<(i32, i32)> = None;
    let mut run_start: Option<i32> = None;
    // Stop above the status bar, which is chrome this primitive doesn't own.
    let limit = H - 40;
    for y in 0..=limit {
        let hit = y < limit && driver.pixel(x, y) == (sep.r, sep.g, sep.b);
        match (hit, run_start) {
            (true, None) => run_start = Some(y),
            (false, Some(s)) => {
                let len = y - s;
                if best.is_none_or(|(_, bl)| len > bl) {
                    best = Some((s, len));
                }
                run_start = None;
            }
            _ => {}
        }
    }
    best
}

/// The shared paint fills exactly `SplitLayout::divider_bounds` in
/// `theme.separator` and nothing else — a vertical band, one divider
/// thickness wide, roughly centred for the app's default 0.5 ratio, with
/// both panes left untouched (pane content is the host's job on every
/// backend, before and after #864).
#[test]
fn split_divider_paints_a_separator_band_through_the_shared_paint_surface_path() {
    let mut driver = GtkDriver::new(SplitApp::new(), W, H);
    // Well below the pane labels' one-line-high status bars, well above
    // the bottom status bar.
    let row = H / 2;

    let (x, len) = separator_run_in_row(&mut driver, row)
        .expect("GtkBackend::draw_split should paint a separator-coloured divider band");

    assert!(
        (3..=6).contains(&len),
        "divider band should be one divider thickness wide (GTK_DIVIDER_PX = 4), got {len}px",
    );
    // Default ratio is 0.5, so the band sits near the middle — asserted as
    // a broad window, not an exact pixel, so font metrics can't break it.
    let centre = x as f32 + len as f32 / 2.0;
    assert!(
        (centre - W as f32 / 2.0).abs() < 20.0,
        "0.5 ratio should centre the divider, got centre={centre}",
    );

    // Panes are unpainted by the primitive: a column well inside the first
    // pane has no separator pixels at all.
    assert_eq!(
        separator_run_in_col(&mut driver, 20),
        None,
        "split paints chrome only — the first pane must be left to the host",
    );
}

/// Toggling direction re-runs the same shared paint against the other
/// `SplitDirection`'s `divider_bounds`: the band stops being a column and
/// becomes a row. Catches a paint that ignored the resolved layout.
#[test]
fn split_toggling_direction_repaints_the_divider_as_a_horizontal_band() {
    let mut driver = GtkDriver::new(SplitApp::new(), W, H);
    assert_eq!(
        separator_run_in_col(&mut driver, 20),
        None,
        "horizontal split has no divider pixels in the first pane's column",
    );

    driver.type_char('v');

    let (y, len) = separator_run_in_col(&mut driver, 20)
        .expect("a Vertical split's divider spans the full width, so column x=20 crosses it");
    assert!(
        (3..=6).contains(&len),
        "divider band should be one divider thickness tall, got {len}px",
    );
    assert!(
        y > 0 && y < H - 40,
        "the divider band should sit inside the split area, got y={y}",
    );
}

/// Paint↔hit-test round trip across the shared path: a drag that starts on
/// the *painted* divider must be recognised as `SplitHit::Divider` by the
/// layout that painted it, and the next frame must repaint the band at the
/// new ratio. A paint that drifted from `Backend::split_layout`'s geometry
/// would fail here even though both halves looked right in isolation.
#[test]
fn split_dragging_the_painted_divider_moves_it_and_updates_the_ratio() {
    let mut driver = GtkDriver::new(SplitApp::new(), W, H);
    assert!(
        driver.screen_contains("ratio: 50% (H)"),
        "starts centred 50/50",
    );

    let row = H / 2;
    let (before_x, before_len) =
        separator_run_in_row(&mut driver, row).expect("divider should be painted");
    let grab_x = before_x as f32 + before_len as f32 / 2.0;

    let target_x = grab_x - 150.0;
    driver.drag(grab_x, row as f32, target_x, row as f32);

    let (after_x, after_len) = separator_run_in_row(&mut driver, row)
        .expect("divider should still be painted after the drag");
    let after_centre = after_x as f32 + after_len as f32 / 2.0;

    assert!(
        after_centre < grab_x - 100.0,
        "dragging 150px left should move the painted divider left by roughly that much: \
         before={grab_x}, after={after_centre}",
    );
    assert!(
        !driver.screen_contains("ratio: 50% (H)"),
        "the status bar ratio should follow the divider away from 50%",
    );
}

// ─── draw_solid_fill vs draw_status_bar_interactive: the #996 root-cause
//     mechanism, fixed for real by #1179 ─────────────────────────────────
//
// `AppShellDemo`'s own line height happens to make `Backend::line_height()`
// and the fill height `draw_status_bar_interactive` actually uses agree, so
// the end-to-end `appshell_demo_resize_divider_is_solid_with_no_gaps` test
// above can't by itself demonstrate the mechanism issue #996 originally
// described: a caller that hands `draw_status_bar_interactive` a row rect
// taller than the backend's `current_line_height` used to silently get
// back a shorter fill, with no way to notice — GTK's flavour of the same
// bug #1179 fixed on macOS (a hard-coded clear colour and a status bar
// that ignored `rect.height` both showed through the leftover strip).
// This probe reproduces that mechanism directly: it still pins why
// `AppShell` stops depending on `draw_status_bar_interactive` for its
// divider (a plain solid fill is cheaper and carries no segment/text
// layout at all — see `app_shell.rs`'s own #996 comment), and proves
// `Backend::draw_solid_fill` (the replacement) never depended on the fix
// either way.

/// Paints one rect, taller than the backend's default line height, with
/// each of the two candidate primitives — `draw_status_bar_interactive`
/// (the old per-row divider hack's building block) and `draw_solid_fill`
/// (issue #996's replacement) — side by side so a single frame can assert
/// on both.
struct SolidFillVsStatusBarProbe;

/// Height of the probe rects — comfortably taller than any backend's
/// default `current_line_height` so a fill that silently caps at
/// `current_line_height` is unambiguously distinguishable from one that
/// covers the whole rect.
const PROBE_RECT_HEIGHT: f32 = 120.0;
const PROBE_FILL_COLOR: quadraui::Color = quadraui::Color::rgb(100, 100, 110);

impl quadraui::AppLogic for SolidFillVsStatusBarProbe {
    type AreaId = ();

    fn render(&self, backend: &mut dyn quadraui::Backend, _area: ()) {
        use quadraui::{InteractionState, StatusBar, StatusBarSegment, WidgetId};

        let status_bar_rect = quadraui::Rect::new(0.0, 0.0, 40.0, PROBE_RECT_HEIGHT);
        let bar = StatusBar {
            id: WidgetId::new("probe:status-bar-fill"),
            left_segments: vec![StatusBarSegment {
                text: "    ".to_string(),
                fg: PROBE_FILL_COLOR,
                bg: PROBE_FILL_COLOR,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ =
            backend.draw_status_bar_interactive(status_bar_rect, &bar, &InteractionState::new());

        let solid_fill_rect = quadraui::Rect::new(60.0, 0.0, 40.0, PROBE_RECT_HEIGHT);
        backend.draw_solid_fill(solid_fill_rect, PROBE_FILL_COLOR);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn quadraui::Backend) -> Reaction {
        Reaction::Continue
    }
}

/// #1179 flipped this from a defect pin to a positive assertion:
/// `GtkBackend::draw_status_bar_interactive` used to fill only
/// `current_line_height` regardless of the row rect's own height — the
/// mechanism issue #996 identified as unsafe for a caller that wants a
/// tall, single-color rect, and the same class of bug #1179 reports for
/// macOS (a caller-supplied `rect` taller than the chrome font's own
/// line height left an unpainted strip showing the frame's clear colour
/// instead of the bar's own fill). It now measures its fill from
/// `rect.height`, matching `draw_solid_fill` below. Sampling near the
/// bottom of a `PROBE_RECT_HEIGHT`-tall rect must land on the fill.
#[test]
fn draw_status_bar_interactive_fills_a_rect_taller_than_line_height_completely() {
    let mut driver = GtkDriver::new(SolidFillVsStatusBarProbe, 200, PROBE_RECT_HEIGHT as i32);
    let near_bottom_y = (PROBE_RECT_HEIGHT - 4.0) as i32;

    assert_eq!(
        driver.pixel(20, near_bottom_y),
        (PROBE_FILL_COLOR.r, PROBE_FILL_COLOR.g, PROBE_FILL_COLOR.b),
        "draw_status_bar_interactive must fill all the way to the bottom of a \
         rect taller than current_line_height (#1179) — a caller-supplied rect \
         taller than the chrome line height must not leave an unpainted strip",
    );
}

/// `Backend::draw_solid_fill` (issue #996's replacement primitive for
/// `AppShell`'s divider) must fill the *whole* rect it's given — no
/// `current_line_height` ceiling, no per-row seams. Sampling top, middle,
/// and near the bottom of the same `PROBE_RECT_HEIGHT`-tall rect must all
/// land on the fill color.
#[test]
fn draw_solid_fill_fills_a_rect_taller_than_line_height_completely() {
    let mut driver = GtkDriver::new(SolidFillVsStatusBarProbe, 200, PROBE_RECT_HEIGHT as i32);
    let expected = (PROBE_FILL_COLOR.r, PROBE_FILL_COLOR.g, PROBE_FILL_COLOR.b);

    for y in [
        2,
        (PROBE_RECT_HEIGHT / 2.0) as i32,
        (PROBE_RECT_HEIGHT - 4.0) as i32,
    ] {
        assert_eq!(
            driver.pixel(80, y),
            expected,
            "draw_solid_fill should cover the entire rect height, including y={y}",
        );
    }
}

// ─── SidebarPanelBody (issue #1041) ─────────────────────────────────────

const SIDEBAR_PANEL_BODY_W: i32 = 300;
const SIDEBAR_PANEL_BODY_H: i32 = 240;

/// GTK-side smoke test for the `gtk_sidebar_panel_body` example
/// (issue #1041's review non-blocking concern: only `tui_example_driver.rs`
/// exercised the new example pair). Confirms the composer's chrome header
/// paints on the GTK backend at all — the default mode is
/// `HeaderAndSearch`, so both the header label and the search
/// placeholder should be visible.
#[test]
fn sidebar_panel_body_gtk_initial_paint_shows_header_and_search() {
    let driver = GtkDriver::new(
        SidebarPanelBodyDemo::new(),
        SIDEBAR_PANEL_BODY_W,
        SIDEBAR_PANEL_BODY_H,
    );
    assert!(
        driver.screen_contains("ITEMS"),
        "header label should be painted"
    );
    assert!(
        driver.screen_contains("Filter items"),
        "search placeholder should be painted in HeaderAndSearch mode"
    );
    assert!(
        driver.screen_contains("item0"),
        "tree body should paint its rows"
    );
}

/// Cycling to `Header`-only chrome (no search row) must still paint the
/// header label — and must no longer show the search placeholder text,
/// which only ever appears in `HeaderAndSearch` mode. This is the GTK
/// twin of `tui_example_driver.rs`'s
/// `sidebar_panel_body_header_only_mode_has_no_search_row` (issue #1041
/// review): together with the direct `draw_settings_chrome` unit tests in
/// `src/gtk/form.rs`, this exercises the same `Header` mode through the
/// full composed example on the backend the review's blocking finding
/// was filed against.
#[test]
fn sidebar_panel_body_gtk_header_only_mode_paints_header_without_search_placeholder() {
    let mut driver = GtkDriver::new(
        SidebarPanelBodyDemo::new(),
        SIDEBAR_PANEL_BODY_W,
        SIDEBAR_PANEL_BODY_H,
    );
    driver.type_char('c'); // HeaderAndSearch → None
    driver.type_char('c'); // None → Header

    assert!(
        driver.screen_contains("chrome=header"),
        "status bar should confirm Header-only mode"
    );
    assert!(
        driver.screen_contains("ITEMS"),
        "header row should still be visible"
    );
    assert!(
        !driver.screen_contains("Filter items"),
        "search row/placeholder must not appear in header-only mode"
    );
    assert!(
        driver.screen_contains("item0"),
        "tree body should still paint its rows beneath the header-only chrome"
    );
}

// ─── ContextMenuStyleDemo: MenuStyle + ContextMenuController (#1187) ───────

const CTX_MENU_W: i32 = 400;
const CTX_MENU_H: i32 = 300;

/// Issue #1187 acceptance: on a GTK test backend, opening a context menu
/// through `ContextMenuController::open` with the resolved `Custom`
/// style paints it and makes it hit-testable — clicking an item
/// delivers `UiEvent::ContextMenuItemActivated` with the right id.
/// `GtkBackend` never declares `native_menu`, so `Auto` (the default)
/// already resolves `Custom` here without touching `set_menu_style`.
#[test]
fn context_menu_style_demo_gtk_click_activates_item() {
    let mut driver = GtkDriver::new(ContextMenuStyleDemo::new(), CTX_MENU_W, CTX_MENU_H);
    assert!(
        driver.screen_contains("effective=custom"),
        "GTK has no native_menu capability, so Auto must resolve Custom"
    );

    driver.dispatch(UiEvent::MouseDown {
        widget: None,
        button: MouseButton::Right,
        position: Point::new(50.0, 50.0),
        modifiers: Modifiers::default(),
    });
    assert!(
        driver.screen_contains("Copy"),
        "right-click should paint the context menu"
    );

    let (x, y) = driver
        .find("Copy")
        .unwrap_or_else(|| panic!("'Copy' must be visible after right-click"));
    driver.click(x, y);

    assert!(
        driver.screen_contains("Last action: ctx.copy"),
        "clicking Copy should deliver ContextMenuItemActivated(ctx.copy)"
    );
    assert!(
        !driver.screen_contains("Cut"),
        "the menu should close after activation"
    );
}

/// Issue #1187 acceptance: Escape dismisses the painted menu and
/// delivers `UiEvent::ContextMenuDismissed` without activating anything.
#[test]
fn context_menu_style_demo_gtk_escape_dismisses() {
    let mut driver = GtkDriver::new(ContextMenuStyleDemo::new(), CTX_MENU_W, CTX_MENU_H);
    driver.dispatch(UiEvent::MouseDown {
        widget: None,
        button: MouseButton::Right,
        position: Point::new(50.0, 50.0),
        modifiers: Modifiers::default(),
    });
    assert!(driver.screen_contains("Copy"));

    driver.press_named(NamedKey::Escape);
    assert!(
        !driver.screen_contains("Copy"),
        "Escape should dismiss the painted menu"
    );
    assert!(
        !driver.screen_contains("Last action"),
        "dismissal must not report an activated action"
    );
}

// ─── SubmenuApp: pull-right cascading submenus on GTK (#371, GTK twin of
// #370's TUI work) ───────────────────────────────────────────────────────
//
// Same `SubmenuApp` as `tui_submenu`/`gtk_submenu` — driven entirely
// through `Backend::draw_context_menu` / `MenuSystem`, so this is the
// coordinate-free proof that GTK's shared `native_surface_paint::paint`
// now renders the `▶` affordance and that the cascading open/close/click
// state machine (already backend-agnostic in `compose::MenuSystem` and
// the example's own `CtxState`) behaves the same as it does on TUI.

const SUBMENU_W: i32 = 500;
const SUBMENU_H: i32 = 400;

#[test]
fn submenu_context_menu_click_opens_nested_level_and_activates_leaf() {
    let mut driver = GtkDriver::new(SubmenuApp::new(), SUBMENU_W, SUBMENU_H);

    driver.dispatch(UiEvent::MouseDown {
        widget: None,
        button: MouseButton::Right,
        position: Point::new(60.0, 60.0),
        modifiers: Modifiers::default(),
    });
    assert!(
        driver.screen_contains("Refactor"),
        "right-click should paint the root context menu: {:?}",
        driver.painted_texts()
    );

    let (x, y) = driver
        .find("Refactor")
        .expect("Refactor (a submenu parent) should be locatable");
    driver.click(x, y);
    assert!(
        driver.screen_contains("Rename") && driver.screen_contains("Extract"),
        "clicking a submenu-parent item should open its pull-right child: {:?}",
        driver.painted_texts()
    );

    let (rx, ry) = driver
        .find("Rename")
        .expect("Rename should be locatable once the submenu is open");
    driver.click(rx, ry);

    assert!(
        driver.screen_contains("activated: rename"),
        "clicking a leaf in the nested submenu should activate it: {:?}",
        driver.painted_texts()
    );
    assert!(
        !driver.screen_contains("Cut"),
        "activating an item should close the whole menu stack: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn submenu_menu_bar_dropdown_opens_two_nested_levels() {
    let mut driver = GtkDriver::new(SubmenuApp::new(), SUBMENU_W, SUBMENU_H);

    let (x, y) = driver
        .find("View")
        .expect("menu bar item should be painted with locatable bounds");
    driver.click(x, y);
    assert!(
        driver.screen_contains("Export"),
        "View dropdown should be open: {:?}",
        driver.painted_texts()
    );

    let (ex, ey) = driver
        .find("Export")
        .expect("Export (depth-1 submenu parent) should be locatable");
    driver.click(ex, ey);
    assert!(
        driver.screen_contains("PNG") && driver.screen_contains("SVG"),
        "clicking Export should open its pull-right child: {:?}",
        driver.painted_texts()
    );

    let (px, py) = driver
        .find("PNG")
        .expect("PNG (depth-2 submenu parent) should be locatable");
    driver.click(px, py);
    assert!(
        driver.screen_contains("Lossless") && driver.screen_contains("Compressed"),
        "clicking PNG should open a third nested level: {:?}",
        driver.painted_texts()
    );

    let (lx, ly) = driver
        .find("Lossless")
        .expect("Lossless should be locatable at the third nested level");
    driver.click(lx, ly);
    assert!(
        driver.screen_contains("activated: export-png-lossless"),
        "activating the deepest leaf should report its id: {:?}",
        driver.painted_texts()
    );
}
