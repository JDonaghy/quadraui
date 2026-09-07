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
use quadraui::{Key, Modifiers, NamedKey, Reaction, Theme, UiEvent};

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
        "on_shell_event_ctx(PanelChanged) must fire for a keyboard-driven switch, \
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

// ─── SplitApp: the shared `NativeSurface` divider paint (#864) ───────────────
//
// Driver-tier cover for `primitives::split::native_surface_paint::paint`,
// the one shared implementation that replaced `gtk::draw_split`'s,
// `macos::split::draw_split`'s and `win::split::draw_split`'s three
// separate divider fills (#864, `NativeSurface` Phase 2d slice 7/9, child
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
fn split_divider_paints_a_separator_band_through_the_shared_native_surface_path() {
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
