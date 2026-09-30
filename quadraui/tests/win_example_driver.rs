//! First driver-harness smoke set for the Win-GUI backend (quadraui#707)
//! — the Win-GUI twin of `tests/macos_example_driver.rs` /
//! `tests/gtk_example_driver.rs` / `tests/tui_example_driver.rs`'s
//! `appshell_demo_*` tests.
//!
//! Only compiled on `target_os = "windows"`: [`quadraui::win::testing`]
//! (and the `WinBackend` rasterisers it drives) only exist there — see
//! `win::mod`'s module docs. Runs for real on `ci.yml`'s `windows-latest`
//! leg's "Test (win feature, real Windows)" step, against a real (WARP
//! software) Direct2D device via `HeadlessSurface` — no live window, no
//! GPU, no display.
//!
//! ## Why pixel/`Reaction` assertions, not `screen_contains`/`find`
//!
//! `WinBackend` doesn't instrument its `draw_text` call sites into a
//! `TextRun` list the way `GtkBackend`/`MacBackend` do yet (see
//! `win::testing::WinDriver`'s "Limitations" doc), so there is no
//! painted-text lookup to assert on here. Instead these tests assert on:
//! - [`AppShellDemo::probe`]'s real `ActivityBar` bounds (proves layout
//!   reached `render_content` through the composed `ShellAdapter`, not a
//!   shadow copy); and
//! - the `Reaction` each scripted event returns — in particular, the
//!   `Redraw`-vs-`Continue` split a `'j'` keypress gets depending on
//!   whether the activity bar is keyboard-focused proves
//!   `win::run::dispatch_event`'s ActivityBar intercept (quadraui#707's
//!   second review finding) is actually wired, without needing to reach
//!   `WinBackend::focused_activity_bar_id` directly — that method is
//!   `pub(crate)` and this file is a separate crate (an integration test),
//!   same visibility boundary `tests/macos_example_driver.rs` /
//!   `tests/gtk_example_driver.rs` respect for their own backends'
//!   equivalent internals.
#![cfg(all(feature = "win", target_os = "windows"))]

use quadraui::win::testing::{driver_with_shell, WinDriver};
use quadraui::{
    Backend, Key, Modifiers, NamedKey, Reaction, Rect, Split, SplitDirection, UiEvent, WidgetId,
};

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
use appshell_demo::AppShellDemo;

#[path = "../examples/common/panel_app.rs"]
mod panel_app;
use panel_app::PanelApp;

// ─── quadraui#1116 slice 1: parity with the vimcode-relevant Win examples ──
//
// `tests/tui_example_driver.rs` / `tests/gtk_example_driver.rs` /
// `tests/macos_example_driver.rs` already cover these `examples/common`
// shapes; each block below ports the same script (same `AppLogic`, same
// scripted `UiEvent`s / driver calls, same assertions on painted text and
// status-bar state) onto `WinDriver` — proving the whole
// `AppLogic → WinBackend` chain agrees with the other three backends, not
// just that it compiles. `WinDriver::new`/`driver_with_shell` paint the
// first frame with painted-text-run recording on unconditionally
// (quadraui#721), so `find`/`screen_contains` work here exactly as they do
// on `GtkDriver`/`MacDriver` — this file's module doc predates that
// landing (see the `PanelApp` section above).

#[path = "../examples/common/menu_bar_app.rs"]
mod menu_bar_app;
use menu_bar_app::MenuBarApp;

#[path = "../examples/common/split_app.rs"]
mod split_app;
use split_app::SplitApp;

#[path = "../examples/common/toast_app.rs"]
mod toast_app;
use toast_app::ToastApp;

#[path = "../examples/common/sidebar_search.rs"]
mod sidebar_search;
use sidebar_search::SidebarSearchApp;

#[path = "../examples/common/search_panel.rs"]
mod search_panel;
use search_panel::SearchPanelApp;

#[path = "../examples/common/multi_tree.rs"]
mod multi_tree;
use multi_tree::DebugSidebar;

#[path = "../examples/common/data_table_app.rs"]
mod data_table_app;
use data_table_app::DataTableApp;

// ─── quadraui#1229: slice 2, the remaining examples ────────────────────────
//
// `win_app`, `win_demo`, `win_chart`, `win_form_groups`, `win_hscroll`,
// `win_indicators` and `win_platform_services` — the rest of #1116's
// scope, left untracked when #1116 was closed by slice 1 (PR #1226).
// Same posture as the slice-1 block above: port each shape's
// `tests/tui_example_driver.rs` script onto `WinDriver` where one exists,
// or author directly from the `examples/common` module when it doesn't
// (`mini_app`/`hscroll_editor` have no TUI/GTK/macOS driver test to port
// — grepped, see each section's own doc).

#[path = "../examples/common/mini_app.rs"]
mod mini_app;
use mini_app::MiniApp;

#[path = "../examples/common/demo.rs"]
mod demo;
use demo::AppState;

// `ChartApp::last_chart_rect` is write-only in the shared example source
// — see `tests/tui_example_driver.rs`'s identical `#[allow(dead_code)]`
// on its own `#[path]` include of the same file for why this needs a
// local opt-out here too (`examples/common/mod.rs`'s blanket
// `#![allow(dead_code)]` doesn't reach a bare `#[path]` include).
#[path = "../examples/common/chart_app.rs"]
#[allow(dead_code)]
mod chart_app;
use chart_app::ChartApp;

#[path = "../examples/common/form_groups.rs"]
mod form_groups;
use form_groups::FormGroupsApp;

#[path = "../examples/common/hscroll_editor.rs"]
mod hscroll_editor;
use hscroll_editor::HScrollEditor;

#[path = "../examples/common/indicators_app.rs"]
mod indicators_app;
use indicators_app::IndicatorsApp;

// `win_platform_services`'s `AppLogic` lives in the example file itself,
// not `examples/common/` (see that file's module doc for why) — the
// example's own doc comment on `PlatformServicesDemo` explains the `pub`
// visibility bump this `#[path]` include needed. `#[allow(dead_code)]`
// for the same reason as `chart_app` above: this include pulls in the
// example's own `fn main`, which this test binary never calls.
#[path = "../examples/win_platform_services.rs"]
#[allow(dead_code)]
mod win_platform_services;
use win_platform_services::PlatformServicesDemo;

// DIP canvas sized for the shell chrome (activity bar + sidebar + main
// content) — same nominal size the GTK/macOS/TUI `appshell_demo_*` driver
// tests use for their own `SHELL_W`/`SHELL_H`.
const SHELL_W: u32 = 800;
const SHELL_H: u32 = 480;

/// `driver_with_shell` composes `AppShellDemo` through the exact same
/// `build_shell_adapter` factory `win::shell_runner::run_with_shell` calls
/// in production (quadraui#707's first review finding) — and the first
/// frame it paints reaches real Direct2D calls (`WinBackend::draw_activity_bar`
/// / `draw_status_bar`, both landed in #25) with no `todo!()` panic, proving
/// the whole `ShellApp → ShellAdapter → WinBackend` chain actually runs.
///
/// The activity-bar bounds `AppShellDemo::render_content` publishes into
/// `ActivityProbe` come from the real `AppShellLayout` the shell computed —
/// non-empty here means `AppShell::compute_layout` genuinely ran and handed
/// real geometry through, not a zeroed default.
#[test]
fn appshell_demo_renders_shell_chrome_via_driver_with_shell() {
    let app = AppShellDemo::new();
    let probe = app.probe();
    let config = AppShellDemo::config();

    // Constructing the driver runs `setup` + paints the first frame
    // (`WinDriver::new`) — if any rasteriser `AppShell::render` or
    // `AppShellDemo::render_content` calls were still a `todo!()` stub,
    // this would panic right here instead of reaching the assertions
    // below.
    let _driver = driver_with_shell(app, config, SHELL_W, SHELL_H);

    let bounds = probe
        .bounds()
        .expect("activity bar bounds should be published by render_content");
    assert!(
        bounds.width > 0.0 && bounds.height > 0.0,
        "activity bar bounds should be non-empty: {bounds:?}"
    );
}

/// The activity-bar keyboard-focus redirect (quadraui#707's second
/// blocking review finding): `AppShellDemo::handle` has no `'j'` arm of
/// its own (its match falls through to `_ => Reaction::Continue`), so a
/// plain `'j'` reaching it unintercepted must return `Continue`. `Tab`
/// asks `ShellContext` to enter activity-bar keyboard-cursor mode; from
/// then on a `'j'` must instead be redirected by
/// `win::run::dispatch_event` into `UiEvent::ActivityBar(..)` navigation,
/// which the real, composed `ShellAdapter` handles by moving its cursor
/// and returning `Redraw` — a real behavioural difference this test can
/// observe without reaching `WinBackend::focused_activity_bar_id`
/// directly (see this file's module doc).
#[test]
fn appshell_demo_tab_then_j_is_intercepted_as_activity_bar_navigation() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    // Negative control: before Tab, the bar isn't keyboard-focused, so
    // 'j' isn't intercepted and reaches `AppShellDemo::handle` unmatched.
    let reaction = driver.type_char('j');
    assert_eq!(
        reaction,
        Reaction::Continue,
        "'j' before Tab has nothing to intercept it and no handler of its own"
    );

    let reaction = driver.press_named(NamedKey::Tab);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Tab should request activity-bar keyboard focus and redraw"
    );

    let reaction = driver.type_char('j');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "'j' while focused must be intercepted as ActivityBar nav, not fall through unmatched"
    );
}

/// Global accelerator dispatch (the other half of #707's second review
/// finding): a registered `Global`-scope accelerator must reach the app
/// as `UiEvent::Accelerator`, not a raw `KeyPressed` — proven here by
/// pressing the plain `q`/`Escape` exit path `AppShellDemo::handle`
/// already wires as a raw `KeyPressed` match (no accelerator registered
/// for it), which still reaches the app unchanged when nothing intercepts
/// it. This is the negative case: confirms `dispatch_event`'s pipeline
/// doesn't swallow ordinary keys that match neither the activity-bar
/// focus nor any registered accelerator.
#[test]
fn appshell_demo_unintercepted_key_still_reaches_the_app() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, SHELL_W, SHELL_H);

    assert!(!driver.exited());
    let reaction = driver.type_char('q');
    assert_eq!(
        reaction,
        Reaction::Exit,
        "'q' has no activity-bar focus or accelerator to intercept it, \
         so it must still reach AppShellDemo::handle and exit"
    );
    assert!(driver.exited());
}

// ─── PanelApp: drag text selection + Ctrl-C copy (#741) ────────────────────
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `panel_drag_selects_text_and_ctrl_c_copies_it` — same `PanelApp`
// `AppLogic`, same script (drag across two painted content lines, assert
// selection feedback, Ctrl-C, assert the copy landed). `WinDriver::new`
// (unlike `driver_with_shell`) enables painted-text-run recording
// unconditionally (quadraui#721), so `find`/`screen_contains` resolve real
// `StatusBar` text the same way `GtkDriver`'s do — the stale "no painted-text
// lookup yet" caveat in this file's module doc predates that landing.
//
// Panel content pixel size for `PanelApp`'s five sample lines plus title
// bar and status bar — comfortably larger than the painted content so
// every line's `find` target has a real `TextRun` to hit.
const PANEL_W: u32 = 800;
const PANEL_H: u32 = 300;

#[test]
fn panel_drag_selects_text_and_ctrl_c_copies_it() {
    let mut driver = WinDriver::new(PanelApp::new(), PANEL_W, PANEL_H);

    // Two distinct painted content lines (substrings unique to lines 0 and 3
    // of `PanelApp`'s `CONTENT_LINES` — see `examples/common/panel_app.rs`).
    let (x0, y0) = driver
        .find("brown")
        .unwrap_or_else(|| panic!("content line 0 not painted"));
    let (x1, y1) = driver
        .find("wizards")
        .unwrap_or_else(|| panic!("content line 3 not painted"));

    // Drag down across the content lines → `route_mouse_down` begins a
    // TextSelection drag on MouseDown and `route_mouse_move` emits
    // TextSelectionChanged, which `dispatch_event` turns into an active
    // selection (#741).
    driver.mouse_down(x0, y0);
    driver.mouse_move(x1, y1);
    assert!(
        driver.screen_contains("Selecting"),
        "dragging over the content region should show selection feedback"
    );
    driver.mouse_up(x1, y1);

    // Ctrl-C with an active selection → `dispatch_event` copies it and
    // delivers TextCopied, which PanelApp echoes via its status bar.
    driver.ctrl_char('c');
    assert!(
        driver.screen_contains("Copied:"),
        "Ctrl-C after a selection should copy it"
    );
    assert!(
        driver.screen_contains("quick"),
        "the copied preview should contain selected text"
    );
}

#[test]
fn panel_ctrl_a_selects_the_sole_content_region_and_ctrl_c_copies_it() {
    let mut driver = WinDriver::new(PanelApp::new(), PANEL_W, PANEL_H);

    // No prior click — Ctrl-A must still resolve the sole registered
    // `TextRegion` fallback path (`select_all_text_region`, #741).
    driver.ctrl_char('a');
    driver.ctrl_char('c');
    assert!(
        driver.screen_contains("Copied:"),
        "Ctrl-A then Ctrl-C should copy the full selection"
    );
    assert!(
        driver.screen_contains("quick") || driver.screen_contains("brown"),
        "select-all should copy from the beginning of the content"
    );
}

// ─── MenuBarApp: open / navigate / select via MenuSystem (issue #1116) ────
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `menu_bar_alt_f_opens_file_dropdown` /
// `menu_bar_down_then_enter_navigates_and_activates_open_file`, plus a
// click-routed variant modelled on `tests/gtk_example_driver.rs`'s
// `menu_bar_clicking_an_item_opens_its_dropdown` — `MenuBarApp` is the
// same backend-agnostic `AppLogic` on every backend, so the same script
// should produce the same status-bar text here.

const MENU_W: u32 = 800;
const MENU_H: u32 = 200;

#[test]
fn menu_bar_alt_f_opens_file_dropdown() {
    let mut driver = WinDriver::new(MenuBarApp::new(), MENU_W, MENU_H);

    assert!(
        !driver.screen_contains("New File"),
        "dropdown starts closed: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("menu closed"),
        "status bar should report no menu open initially: {:?}",
        driver.painted_texts()
    );

    let reaction = driver.dispatch(UiEvent::KeyPressed {
        key: Key::Char('f'),
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Alt+F should open the File dropdown"
    );

    assert!(driver.screen_contains("New File"));
    assert!(driver.screen_contains("Open File"));
    assert!(driver.screen_contains("Quit"));
    assert!(
        driver.screen_contains("menu open"),
        "status bar should report the menu is open: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn menu_bar_down_then_enter_navigates_and_activates_open_file() {
    let mut driver = WinDriver::new(MenuBarApp::new(), MENU_W, MENU_H);

    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Char('f'),
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    let r_down = driver.press_named(NamedKey::Down);
    assert_eq!(r_down, Reaction::Redraw);
    let r_enter = driver.press_named(NamedKey::Enter);
    assert_eq!(r_enter, Reaction::Redraw);

    assert!(
        driver.screen_contains("last: activated: open"),
        "Down then Enter should select and activate 'Open File' (id \"open\"): {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("menu closed"),
        "activating an item should close the dropdown: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn menu_bar_clicking_view_item_opens_its_dropdown() {
    let mut driver = WinDriver::new(MenuBarApp::new(), MENU_W, MENU_H);
    assert!(driver.screen_contains("menu closed"));

    let (x, y) = driver
        .find("View")
        .unwrap_or_else(|| panic!("View menu label not painted: {:?}", driver.painted_texts()));
    // `mouse_down`, not `click`: `MenuSystem` opens the dropdown on
    // press; `WinDriver::click`'s extra release (unlike GTK/TUI's bare
    // press-down `click`) then lands back on the same label and closes
    // it again, so asserting on the full down+up round trip would see
    // the dropdown open and shut within one `click` call.
    let reaction = driver.mouse_down(x, y);

    assert_eq!(reaction, Reaction::Redraw, "opening a menu should redraw");
    assert!(
        driver.screen_contains("menu open"),
        "clicking the View item should open its dropdown: {:?}",
        driver.painted_texts()
    );
}

// ─── SplitApp: draggable single divider (issue #1116) ──────────────────────
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `split_dragging_divider_moves_it_and_updates_ratio` /
// `split_bracket_keys_resize_the_divider_with_no_mouse_step`. `Split`
// paints no glyph to `find` the way TUI's `│` divider does (see
// `tests/gtk_example_driver.rs`'s `SplitApp` section doc), so the divider's
// real pixel position is derived from `Backend::split_layout` — the same
// pure-geometry call `SplitApp::handle`'s own `MouseDown`/`MouseMoved` arms
// use to hit-test — rather than a painted-text lookup or a hardcoded
// coordinate.

const SPLIT_W: u32 = 800;
const SPLIT_H: u32 = 300;

/// The `Rect` `SplitApp::render`/`SplitApp::handle` both compute for the
/// split area: full width, full height minus one status-bar line.
fn split_area_rect(driver: &WinDriver<SplitApp>) -> Rect {
    let backend = driver.backend();
    let viewport = backend.viewport();
    let lh = backend.line_height();
    Rect::new(0.0, 0.0, viewport.width, viewport.height - lh)
}

/// Resolve the divider's real painted bounds for a given `ratio` via the
/// same `Backend::split_layout` the app itself calls — not a copy of the
/// primitive's geometry math.
fn split_layout_at(
    driver: &WinDriver<SplitApp>,
    ratio: f32,
    direction: SplitDirection,
) -> quadraui::SplitLayout {
    let rect = split_area_rect(driver);
    let split = Split {
        id: WidgetId::new("main-split"),
        direction,
        ratio,
        first_min: 0.0,
        second_min: 0.0,
    };
    driver.backend().split_layout(rect, &split)
}

#[test]
fn split_dragging_divider_moves_it_and_updates_ratio() {
    let mut driver = WinDriver::new(SplitApp::new(), SPLIT_W, SPLIT_H);
    assert!(
        driver.screen_contains("ratio: 50% (H)"),
        "starts centered 50/50: {:?}",
        driver.painted_texts()
    );

    let layout = split_layout_at(&driver, 0.5, SplitDirection::Horizontal);
    let dx = layout.divider_bounds.x + layout.divider_bounds.width / 2.0;
    let dy = layout.divider_bounds.y + layout.divider_bounds.height / 2.0;

    driver.drag(dx, dy, dx - 100.0, dy);

    assert!(
        !driver.screen_contains("ratio: 50% (H)"),
        "dragging the divider 100px left should move the ratio away from 50%: {:?}",
        driver.painted_texts()
    );
    // `SplitApp::handle`'s `MouseMoved` arm recomputes `ratio` straight
    // from the cursor's absolute x over the split area's width — assert
    // the exact resulting percentage, not just "some" change, so a drag
    // handler that moved the ratio the wrong direction (or by the wrong
    // amount) would still fail this.
    let rect = split_area_rect(&driver);
    let expected_pct = (((dx - 100.0 - rect.x) / rect.width) * 100.0).round() as i32;
    let expected = format!("ratio: {expected_pct}% (H)");
    assert!(
        driver.screen_contains(&expected),
        "expected {expected:?} after dragging 100px left from the divider's \
         resolved centre, got: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn split_bracket_keys_resize_the_divider_with_no_mouse_step() {
    let mut driver = WinDriver::new(SplitApp::new(), SPLIT_W, SPLIT_H);
    assert!(driver.screen_contains("ratio: 50% (H)"));

    driver.type_char(']');
    driver.type_char(']');
    assert!(
        driver.screen_contains("ratio: 60% (H)"),
        "two ']' presses should grow the ratio by 5% each, no mouse step \
         involved: {:?}",
        driver.painted_texts()
    );

    driver.type_char('[');
    driver.type_char('[');
    driver.type_char('[');
    driver.type_char('[');
    assert!(
        driver.screen_contains("ratio: 40% (H)"),
        "four '[' presses from 60% should land on 40%: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn split_v_toggles_direction_and_r_resets_ratio() {
    let mut driver = WinDriver::new(SplitApp::new(), SPLIT_W, SPLIT_H);
    driver.type_char(']');
    assert!(driver.screen_contains("ratio: 55% (H)"));

    let reaction = driver.type_char('v');
    assert_eq!(reaction, Reaction::Redraw, "'v' should redraw");
    assert!(
        driver.screen_contains("ratio: 55% (V)"),
        "'v' should toggle Horizontal -> Vertical, keeping the ratio: {:?}",
        driver.painted_texts()
    );

    let reaction = driver.type_char('r');
    assert_eq!(reaction, Reaction::Redraw, "'r' should redraw");
    assert!(
        driver.screen_contains("ratio: 50% (V)"),
        "'r' should reset the ratio back to 50% without touching direction: {:?}",
        driver.painted_texts()
    );
}

// ─── ToastApp: triggering + dismissing a toast (issue #1116) ───────────────
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `toast_dismiss_click_removes_it_then_trigger_adds_a_new_one`.

const TOAST_W: u32 = 800;
const TOAST_H: u32 = 300;

#[test]
fn toast_dismiss_click_removes_it_then_trigger_adds_a_new_one() {
    let mut driver = WinDriver::new(ToastApp::new(), TOAST_W, TOAST_H);
    assert!(
        driver.screen_contains("Welcome"),
        "ToastApp::new seeds a Welcome toast: {:?}",
        driver.painted_texts()
    );

    let (x, y) = driver.find("×").unwrap_or_else(|| {
        panic!(
            "no dismiss glyph painted for the seeded toast: {:?}",
            driver.painted_texts()
        )
    });
    driver.click(x, y);

    assert!(
        !driver.screen_contains("Welcome"),
        "clicking the dismiss glyph should remove the toast from the stack: {:?}",
        driver.painted_texts()
    );

    driver.type_char('2');
    assert!(
        driver.screen_contains("Success notification"),
        "pressing '2' should trigger and paint a new Success toast: {:?}",
        driver.painted_texts()
    );
}

// Two more scripted directly from `examples/common/toast_app.rs`'s
// `add_toast`/`handle` (issue #1229) — bringing this slice-1 shape up to
// three tests: an action-button toast and a plain body click, neither
// covered by the dismiss-then-trigger test above.

#[test]
fn toast_pressing_a_adds_action_toast_and_clicking_retry_logs_the_action() {
    let mut driver = WinDriver::new(ToastApp::new(), TOAST_W, TOAST_H);
    driver.type_char('a');
    assert!(
        driver.screen_contains("Retry"),
        "'a' should add a toast with a Retry action button: {:?}",
        driver.painted_texts()
    );

    let (x, y) = driver
        .find("Retry")
        .unwrap_or_else(|| panic!("Retry button not painted: {:?}", driver.painted_texts()));
    // `mouse_down`, not `click`: `ToastApp::handle`'s `ToastHit::Action`
    // arm fires on press; `WinDriver`'s extra release lands on the same
    // button and this test isn't scripting a second click.
    driver.mouse_down(x, y);
    assert!(
        driver.screen_contains("Action: retry"),
        "clicking the Retry action should log it to the status bar: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn toast_clicking_the_toast_body_logs_the_click() {
    let mut driver = WinDriver::new(ToastApp::new(), TOAST_W, TOAST_H);
    driver.type_char('1');
    assert!(driver.screen_contains("Info notification"));

    let (x, y) = driver
        .find("Info notification")
        .unwrap_or_else(|| panic!("Info toast title not painted: {:?}", driver.painted_texts()));
    driver.mouse_down(x, y);
    assert!(
        driver.screen_contains("Clicked toast-1"),
        "clicking the toast body (not its dismiss/action glyphs) should log \
         a body click for this toast's id: {:?}",
        driver.painted_texts()
    );
}

// ─── SidebarSearchApp: SidebarSystem search panel (issue #1116) ───────────
//
// No TUI/GTK/macOS driver test exists yet for this shape (grepped —
// `SidebarSearchApp` doesn't appear in any other `tests/*_example_driver.rs`),
// so this is authored directly from `examples/common/sidebar_search.rs`'s
// `handle`/`build_status_bar` rather than ported from a sibling backend.

const SIDEBAR_SEARCH_W: u32 = 800;
const SIDEBAR_SEARCH_H: u32 = 420;

#[test]
fn sidebar_search_initial_screen_paints_toggles_and_results() {
    let driver = WinDriver::new(SidebarSearchApp::new(), SIDEBAR_SEARCH_W, SIDEBAR_SEARCH_H);
    assert!(
        driver.screen_contains("Search..."),
        "query placeholder should paint: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Aa"),
        "case-sensitive toggle label should paint: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("src/main.rs"),
        "a file header row should paint: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("fn main() {"),
        "a match row under the expanded header should paint: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Aa:off"),
        "status bar should report the case-sensitive flag off initially: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn sidebar_search_clicking_the_case_toggle_flips_the_status_bar_flag() {
    let mut driver = WinDriver::new(SidebarSearchApp::new(), SIDEBAR_SEARCH_W, SIDEBAR_SEARCH_H);
    let (x, y) = driver
        .find("Aa")
        .unwrap_or_else(|| panic!("Aa toggle not painted: {:?}", driver.painted_texts()));

    // `mouse_down`, not `click`: the toggle flips on press; `WinDriver`'s
    // extra release lands on the same toggle and would flip it straight
    // back, unlike GTK/TUI's bare press-down `click`.
    let reaction = driver.mouse_down(x, y);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "clicking a toggle should redraw"
    );
    assert!(
        driver.screen_contains("Aa:on"),
        "clicking the Aa toggle should flip case_sensitive to true: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn sidebar_search_clicking_a_file_header_collapses_then_reexpands_its_matches() {
    let mut driver = WinDriver::new(SidebarSearchApp::new(), SIDEBAR_SEARCH_W, SIDEBAR_SEARCH_H);
    assert!(driver.screen_contains("fn main() {"));

    let (x, y) = driver
        .find("src/main.rs")
        .unwrap_or_else(|| panic!("file header not painted: {:?}", driver.painted_texts()));
    // `mouse_down`, not `click`: the row toggles expand/collapse on
    // press, so `WinDriver`'s extra release would toggle it right back.
    driver.mouse_down(x, y);
    assert!(
        !driver.screen_contains("fn main() {"),
        "clicking the file header should collapse its match rows: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Collapsed file 0"),
        "status bar should confirm the collapse: {:?}",
        driver.painted_texts()
    );

    // Re-find, then click a few pixels to the right of the label's own
    // centre rather than the exact same point: a second `mouse_down` at
    // the *identical* position within `WIN_DOUBLE_CLICK_RADIUS` (4px) of
    // the first, inside `DOUBLE_CLICK_MS`, folds into a synthetic
    // `UiEvent::DoubleClick` (`dispatch::DoubleClickDetector`) instead of
    // a plain `MouseDown` — which `SidebarSearchApp::handle` doesn't
    // match, silently dropping the second toggle (see
    // `tests/tui_example_driver.rs`'s
    // `toolbar_press_release_on_filter_toggles_is_active_state` for the
    // same trap on a different primitive). The header row spans the
    // section's full width, so anywhere along it still hits the same row.
    let (x, y) = driver
        .find("src/main.rs")
        .expect("file header still painted after collapsing");
    driver.mouse_down(x + 20.0, y);
    assert!(
        driver.screen_contains("fn main() {"),
        "clicking the file header again should re-expand its match rows: {:?}",
        driver.painted_texts()
    );
    assert!(driver.screen_contains("Expanded file 0"));
}

// ─── SearchPanelApp: MSV + TreeView file-search results (issue #1116) ─────
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `search_panel_typing_updates_the_search_status`, plus a click-routed
// "jump" test modelled on `SearchPanelApp::handle`'s `UiEvent::MouseDown`
// arm.

const SEARCH_PANEL_W: u32 = 800;
const SEARCH_PANEL_H: u32 = 420;

#[test]
fn search_panel_typing_updates_the_search_status() {
    let mut driver = WinDriver::new(SearchPanelApp::new(), SEARCH_PANEL_W, SEARCH_PANEL_H);
    assert!(
        driver.screen_contains("src/main.rs"),
        "fake result rows should render before typing: {:?}",
        driver.painted_texts()
    );

    for c in "main".chars() {
        driver.type_char(c);
    }

    assert!(
        driver.screen_contains("Searching: main"),
        "typed query should echo in the status bar: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("src/main.rs"),
        "result rows should still be visible while searching: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn search_panel_clicking_a_match_row_jumps_and_logs_the_status() {
    let mut driver = WinDriver::new(SearchPanelApp::new(), SEARCH_PANEL_W, SEARCH_PANEL_H);
    let (x, y) = driver
        .find("fn main() {")
        .unwrap_or_else(|| panic!("match row not painted: {:?}", driver.painted_texts()));

    // `mouse_down`, not `click`: `TreeViewHit::Row`'s "jump" arm fires on
    // press, so `WinDriver`'s extra release is a second, unscripted click
    // at the same point.
    driver.mouse_down(x, y);
    assert!(
        driver.screen_contains("Jump: src/main.rs:12"),
        "clicking a match row should log a jump to its file:line: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn search_panel_clicking_a_file_header_collapses_its_matches() {
    let mut driver = WinDriver::new(SearchPanelApp::new(), SEARCH_PANEL_W, SEARCH_PANEL_H);
    assert!(driver.screen_contains("pub struct Config {"));

    let (x, y) = driver
        .find("src/config.rs")
        .unwrap_or_else(|| panic!("file header not painted: {:?}", driver.painted_texts()));
    driver.mouse_down(x, y);
    assert!(
        driver.screen_contains("Collapsed src/config.rs"),
        "clicking a file header should collapse its match rows and log it: {:?}",
        driver.painted_texts()
    );
    assert!(
        !driver.screen_contains("pub struct Config {"),
        "the collapsed file's match rows should no longer paint: {:?}",
        driver.painted_texts()
    );
}

// ─── DebugSidebar / multi_tree: 4-section debug sidebar (issue #1116) ─────
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `multi_tree_tab_then_down_selects_a_row_in_the_newly_active_section`,
// plus a click-routed header-activation test straight from
// `DebugSidebar::handle`'s `SidebarEvent::HeaderActivated` arm.

const MULTI_TREE_W: u32 = 800;
const MULTI_TREE_H: u32 = 500;

#[test]
fn multi_tree_initial_screen_paints_all_sections() {
    let driver = WinDriver::new(DebugSidebar::new(), MULTI_TREE_W, MULTI_TREE_H);
    assert!(driver.screen_contains("VARIABLES"));
    assert!(driver.screen_contains("WATCH"));
    assert!(driver.screen_contains("CALL STACK"));
    assert!(driver.screen_contains("BREAKPOINTS"));
    assert!(
        driver.screen_contains("active: section 0"),
        "VARIABLES (section 0) should be active initially: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn multi_tree_tab_then_down_selects_a_row_in_the_newly_active_section() {
    let mut driver = WinDriver::new(DebugSidebar::new(), MULTI_TREE_W, MULTI_TREE_H);
    assert!(driver.screen_contains("active: section 0"));

    driver.press_named(NamedKey::Tab); // active section -> 1 (WATCH)
    assert!(
        driver.screen_contains("active: section 1"),
        "Tab should move the active section to WATCH (section 1): {:?}",
        driver.painted_texts()
    );

    driver.press_named(NamedKey::Down);
    assert!(
        driver.screen_contains("sel→1 [0]"),
        "Down after Tab should select WATCH's own first row (w0), not \
         VARIABLES': {:?}",
        driver.painted_texts()
    );
}

#[test]
fn multi_tree_clicking_a_header_activates_that_section() {
    let mut driver = WinDriver::new(DebugSidebar::new(), MULTI_TREE_W, MULTI_TREE_H);
    let (x, y) = driver
        .find("WATCH")
        .unwrap_or_else(|| panic!("WATCH header not painted: {:?}", driver.painted_texts()));

    // `mouse_down`, not `click`: activation happens on press, so
    // `WinDriver`'s extra release is a second, separate click at the same
    // point that this test isn't scripting.
    let reaction = driver.mouse_down(x, y);
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.screen_contains("header→1"),
        "clicking the WATCH header should activate section 1: {:?}",
        driver.painted_texts()
    );
}

// ─── DataTableApp: initial paint, sort cycling, row selection, divider drag
// (issue #1116) ──────────────────────────────────────────────────────────
//
// Win-GUI twin of `tests/macos_example_driver.rs`'s
// `data_table_initial_screen_paints_headers_and_rows` /
// `data_table_pressing_s_cycles_sort_column` /
// `data_table_pressing_j_moves_selection` /
// `data_table_divider_before_last_column_resizes_in_drag_direction` — same
// `DataTableApp`, same script, same assertions; only the driver type
// differs. Pixel canvas sized the same as `tests/macos_example_driver.rs`'s
// `DT_W`/`DT_H`.

const DT_W: u32 = 900;
const DT_H: u32 = 600;

#[test]
fn data_table_initial_screen_paints_headers_and_rows() {
    let driver = WinDriver::new(DataTableApp::new(), DT_W, DT_H);
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

#[test]
fn data_table_pressing_s_cycles_sort_column() {
    let mut driver = WinDriver::new(DataTableApp::new(), DT_W, DT_H);
    assert!(driver.screen_contains("sort: Name asc"));
    driver.type_char('s');
    assert!(
        driver.screen_contains("sort: Status asc"),
        "after one 's' the status bar should read Status: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn data_table_pressing_j_moves_selection() {
    let mut driver = WinDriver::new(DataTableApp::new(), DT_W, DT_H);
    assert!(driver.screen_contains("row 1 / 20"));
    driver.type_char('j');
    assert!(
        driver.screen_contains("row 2 / 20"),
        "after one 'j' the status bar should read row 2 / 20: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn data_table_divider_before_last_column_resizes_in_drag_direction() {
    // Widen: drag the Age|Restarts divider right.
    let mut driver = WinDriver::new(DataTableApp::new(), DT_W, DT_H);
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

    // Narrow: a *fresh* driver rather than a second drag on this one — see
    // the equivalent macOS test's note on `DoubleClickDetector`.
    let mut driver = WinDriver::new(DataTableApp::new(), DT_W, DT_H);
    let natural = driver.app().resolved_column_widths(driver.backend())[2];
    assert!(
        natural > 44.0,
        "test precondition: Age's natural width ({natural}) must leave room \
         to narrow by 40px without hitting the 4.0 pair-resize floor"
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

// ─── MiniApp: single-StatusBar smoke app (issue #1229) ─────────────────────
//
// No TUI/GTK/macOS driver test exists for `MiniApp` yet (grepped —
// `MiniApp`/`mini_app` appears in none of the other
// `tests/*_example_driver.rs` files), so this is authored directly from
// `examples/common/mini_app.rs`'s `status_bar`/`handle` rather than
// ported from a sibling backend.

const MINI_APP_W: u32 = 800;
const MINI_APP_H: u32 = 120;

#[test]
fn mini_app_initial_screen_shows_hint_and_zero_count() {
    let driver = WinDriver::new(MiniApp::new(), MINI_APP_W, MINI_APP_H);
    assert!(
        driver.screen_contains("press any key"),
        "starting hint should paint: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("keys: 0"),
        "key counter should start at 0: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn mini_app_key_press_increments_counter_and_records_last_key() {
    let mut driver = WinDriver::new(MiniApp::new(), MINI_APP_W, MINI_APP_H);
    let reaction = driver.type_char('a');
    assert_eq!(reaction, Reaction::Redraw, "any non-quit key should redraw");
    assert!(
        driver.screen_contains("keys: 1"),
        "one keypress should bump the counter to 1: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("last: a"),
        "the pressed key should be echoed as the last key: {:?}",
        driver.painted_texts()
    );

    driver.type_char('b');
    assert!(
        driver.screen_contains("keys: 2"),
        "a second keypress should bump the counter again: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn mini_app_q_exits() {
    let mut driver = WinDriver::new(MiniApp::new(), MINI_APP_W, MINI_APP_H);
    assert!(!driver.exited());
    let reaction = driver.type_char('q');
    assert_eq!(reaction, Reaction::Exit, "'q' should exit MiniApp");
    assert!(driver.exited());
}

// ─── AppState / demo: tabs + status-segment focus cycling (issue #1229) ───
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `demo_arrow_keys_switch_active_tab` / `demo_n_opens_a_new_scratch_tab`,
// plus two more scripted directly from `examples/common/demo.rs`'s
// `close_active`/`cycle_status_focus`/`handle_status_action` that neither
// sibling backend covers yet.

const DEMO_W: u32 = 800;
const DEMO_H: u32 = 200;

#[test]
fn demo_right_arrow_switches_active_tab() {
    let mut driver = WinDriver::new(AppState::new(), DEMO_W, DEMO_H);
    assert!(
        driver.screen_contains("main.rs"),
        "first tab should be active initially: {:?}",
        driver.painted_texts()
    );

    let reaction = driver.press_named(NamedKey::Right);
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.screen_contains("Tab 2"),
        "Right arrow should advance the status bar's 'Tab N' segment to 2: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn demo_n_opens_a_new_scratch_tab() {
    let mut driver = WinDriver::new(AppState::new(), DEMO_W, DEMO_H);
    driver.type_char('n');
    assert!(
        driver.screen_contains("scratch"),
        "'n' should open a new scratch tab: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn demo_x_closes_the_active_tab() {
    let mut driver = WinDriver::new(AppState::new(), DEMO_W, DEMO_H);
    driver.type_char('n');
    assert!(driver.screen_contains("scratch"));

    let reaction = driver.type_char('x');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "'x' should close the active tab"
    );
    assert!(
        !driver.screen_contains("scratch"),
        "closing the freshly-opened scratch tab should remove its label: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("tests.rs"),
        "closing back down should leave the prior last tab active: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn demo_enter_with_no_prior_tab_dismisses_the_default_focused_segment() {
    let mut driver = WinDriver::new(AppState::new(), DEMO_W, DEMO_H);
    assert!(
        driver.screen_contains("ready"),
        "status bar should start with the 'ready' message: {:?}",
        driver.painted_texts()
    );

    // `AppState::new`'s `focused_status_idx` already starts at 0 — the
    // first interactive right segment, `status:dismiss` — so Enter alone
    // (no Tab needed) should activate it.
    let reaction = driver.press_named(NamedKey::Enter);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Enter should activate the focused segment"
    );
    assert!(
        driver.screen_contains("dismissed"),
        "Enter with the default focus (index 0, 'status:dismiss') should \
         set the last_message to 'dismissed': {:?}",
        driver.painted_texts()
    );
}

#[test]
fn demo_tab_then_enter_activates_the_next_status_segment() {
    let mut driver = WinDriver::new(AppState::new(), DEMO_W, DEMO_H);

    // One Tab moves focus from index 0 (`status:dismiss`) to index 1
    // (`status:encoding`) — `cycle_status_focus`'s `+1` step.
    let reaction = driver.press_named(NamedKey::Tab);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Tab should move status-segment keyboard focus"
    );

    let reaction = driver.press_named(NamedKey::Enter);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Enter should activate the focused segment"
    );
    assert!(
        driver.screen_contains("encoding picker (mock)"),
        "Tab then Enter should activate 'status:encoding', not 'status:dismiss': {:?}",
        driver.painted_texts()
    );
}

// ─── ChartApp: sparkline / line / bar chart views (issue #1229) ────────────
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `chart_switching_to_line_view_paints_axis_labels_and_gridlines` /
// `chart_stacked_bar_view_paints_every_series_scaled_to_column_totals` /
// `chart_grouped_bar_view_paints_series_side_by_side`, but text-only: TUI's
// versions additionally assert on `█`/`┄` glyph geometry in the character
// grid, which has no equivalent here — `WinDriver` paints real pixels, not
// a text grid, and `crate::primitives::chart::paint` draws each axis
// label/legend entry as one whole `surface_draw_text_run` call (unlike
// TUI's per-cell grid, nothing here gets glyph-clipped), so `screen_contains`
// on the full label text is the correct - and sufficient - assertion for
// this backend. Pixel-level bar-height verification would need to walk
// `HeadlessSurface::pixel_at` column by column; left out of scope for this
// slice (see this file's own doc for the "about 3-5 tests" bar).

const CHART_W: u32 = 900;
const CHART_H: u32 = 400;

#[test]
fn chart_initial_screen_shows_sparkline_status_and_no_axis_labels() {
    let driver = WinDriver::new(ChartApp::new(), CHART_W, CHART_H);
    assert!(
        driver.screen_contains("Chart: Sparkline"),
        "status bar should name the starting view: {:?}",
        driver.painted_texts()
    );
    assert!(
        !driver.screen_contains("Time (s)"),
        "sparkline view has no axis labels configured, so none should paint: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn chart_switching_to_line_view_paints_axis_labels_and_legend() {
    let mut driver = WinDriver::new(ChartApp::new(), CHART_W, CHART_H);
    let reaction = driver.type_char('2');
    assert_eq!(reaction, Reaction::Redraw);

    assert!(driver.screen_contains("Chart: Line"));
    assert!(
        driver.screen_contains("Time (s)"),
        "line chart's x_label should paint: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("Usage"),
        "line chart's y_label should paint: {:?}",
        driver.painted_texts()
    );
    assert!(driver.screen_contains("CPU"));
    assert!(driver.screen_contains("Memory"));
}

#[test]
fn chart_switching_to_bar_stacked_shows_legend_and_every_series() {
    let mut driver = WinDriver::new(ChartApp::new(), CHART_W, CHART_H);
    driver.type_char('3');
    assert!(driver.screen_contains("Chart: Bar (stacked)"));
    for label in ["OK", "Slow", "Failed"] {
        assert!(
            driver.screen_contains(label),
            "stacked-bar legend should name every series, missing {label:?}: {:?}",
            driver.painted_texts()
        );
    }
}

#[test]
fn chart_switching_to_bar_grouped_shows_its_own_status_label() {
    let mut driver = WinDriver::new(ChartApp::new(), CHART_W, CHART_H);
    driver.type_char('3');
    assert!(driver.screen_contains("Chart: Bar (stacked)"));
    driver.type_char('4');
    assert!(
        driver.screen_contains("Chart: Bar (grouped)"),
        "'4' should switch the status label from stacked to grouped: {:?}",
        driver.painted_texts()
    );
    for label in ["OK", "Slow", "Failed"] {
        assert!(driver.screen_contains(label));
    }
}

// ─── FormGroupsApp: ToggleGroup / ButtonRow / Toolbar field clicks (#1229) ─
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `form_groups_click_toggle_flips_rendered_value`, plus two more scripted
// directly from `examples/common/form_groups.rs`'s `click` handler (the
// scope-toolbar and Find-Next-button arms) that no sibling backend covers
// yet.

const FORM_GROUPS_W: u32 = 800;
const FORM_GROUPS_H: u32 = 400;

#[test]
fn form_groups_initial_screen_paints_fields_and_default_status() {
    let driver = WinDriver::new(FormGroupsApp::new(), FORM_GROUPS_W, FORM_GROUPS_H);
    for needle in [
        "Find",
        "Aa",
        "Ab|",
        ".*",
        "Replace",
        "Find Next",
        "Replace All",
        "Workspace",
        "File",
        "Selection",
    ] {
        assert!(
            driver.screen_contains(needle),
            "expected {needle:?} painted somewhere: {:?}",
            driver.painted_texts()
        );
    }
    assert!(driver.screen_contains("last: —"));
}

#[test]
fn form_groups_clicking_case_toggle_flips_and_updates_status() {
    let mut driver = WinDriver::new(FormGroupsApp::new(), FORM_GROUPS_W, FORM_GROUPS_H);
    let (x, y) = driver
        .find("Aa")
        .unwrap_or_else(|| panic!("case toggle not painted: {:?}", driver.painted_texts()));

    // `mouse_down`, not `click`: `FormGroupsApp::handle` flips the toggle
    // on `MouseDown` alone — `WinDriver`'s extra release lands on the same
    // toggle and would flip it straight back.
    let reaction = driver.mouse_down(x, y);
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.screen_contains("last: case=false"),
        "clicking the case-sensitive toggle (starts true) should flip it: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn form_groups_clicking_the_file_scope_button_logs_the_scope_change() {
    let mut driver = WinDriver::new(FormGroupsApp::new(), FORM_GROUPS_W, FORM_GROUPS_H);
    let (x, y) = driver.find("File").unwrap_or_else(|| {
        panic!(
            "File scope button not painted: {:?}",
            driver.painted_texts()
        )
    });
    driver.mouse_down(x, y);
    assert!(
        driver.screen_contains("last: scope=File"),
        "clicking the File scope toolbar button should log the scope change: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn form_groups_clicking_find_next_logs_the_action() {
    let mut driver = WinDriver::new(FormGroupsApp::new(), FORM_GROUPS_W, FORM_GROUPS_H);
    let (x, y) = driver
        .find("Find Next")
        .unwrap_or_else(|| panic!("Find Next button not painted: {:?}", driver.painted_texts()));
    driver.mouse_down(x, y);
    assert!(
        driver.screen_contains("last: Find Next"),
        "clicking Find Next should log it to the status bar: {:?}",
        driver.painted_texts()
    );
}

// ─── HScrollEditor: horizontal scroll via key (issue #1229) ───────────────
//
// No TUI/GTK/macOS driver test exists for `HScrollEditor` yet — grepped,
// `HScrollEditor`/`hscroll_editor` appears in none of the other
// `tests/*_example_driver.rs` files (`tests/tui_example_driver.rs`'s
// `hscroll_dollar_key_scrolls_visible_window_to_line_end` is for a
// different shape, `tab_icons_demo`'s hscroll helper — see that file's own
// section comment). Authored directly from
// `examples/common/hscroll_editor.rs`'s status-bar text (`col N / 500
// scroll_left M viewport_cols K`) rather than TUI's character-grid glyph
// positions, which have no equivalent against a pixel `WinDriver` canvas —
// same reasoning as the `ChartApp` section above.

const HSCROLL_W: u32 = 700;
const HSCROLL_H: u32 = 120;

#[test]
fn hscroll_initial_screen_shows_unscrolled_status() {
    let driver = WinDriver::new(HScrollEditor::new(), HSCROLL_W, HSCROLL_H);
    assert!(
        driver.screen_contains("col 1 / 500"),
        "cursor should start at column 1: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("scroll_left 0"),
        "view should start unscrolled: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn hscroll_dollar_key_scrolls_to_end_and_updates_status() {
    let mut driver = WinDriver::new(HScrollEditor::new(), HSCROLL_W, HSCROLL_H);
    let reaction = driver.type_char('$');
    assert_eq!(reaction, Reaction::Redraw, "'$' should redraw");

    assert!(
        driver.screen_contains("col 500 / 500"),
        "'$' should jump the cursor to the line's last column: {:?}",
        driver.painted_texts()
    );
    assert!(
        !driver.screen_contains("scroll_left 0"),
        "jumping to column 500 should scroll the view away from 0: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn hscroll_zero_key_after_dollar_returns_to_start() {
    let mut driver = WinDriver::new(HScrollEditor::new(), HSCROLL_W, HSCROLL_H);
    driver.type_char('$');
    assert!(!driver.screen_contains("scroll_left 0"));

    let reaction = driver.type_char('0');
    assert_eq!(reaction, Reaction::Redraw, "'0' should redraw");
    assert!(
        driver.screen_contains("col 1 / 500"),
        "'0' should jump the cursor back to the first column: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("scroll_left 0"),
        "jumping back to column 0 should scroll the view back to 0: {:?}",
        driver.painted_texts()
    );
}

// ─── IndicatorsApp: ProgressBar + Spinner (issue #1229) ───────────────────
//
// Win-GUI twin of `tests/tui_example_driver.rs`'s
// `indicators_toggling_cancellable_paints_and_clears_cancel_symbol` — the
// same `×` glyph (`\u{d7}`) is drawn as its own `surface_draw_text_run`
// call by the shared `crate::primitives::progress` painter every backend
// (including Win) delegates to, so it resolves via `find`/`screen_contains`
// here exactly as it does on TUI. Two more scripted directly from
// `examples/common/indicators_app.rs`'s `handle` that no sibling backend
// covers yet.

const INDICATORS_W: u32 = 800;
const INDICATORS_H: u32 = 200;

#[test]
fn indicators_initial_screen_paints_spinner_and_progress_label() {
    let driver = WinDriver::new(IndicatorsApp::new(), INDICATORS_W, INDICATORS_H);
    assert!(
        driver.screen_contains("Loading..."),
        "spinner label should paint: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("30%"),
        "progress starts at 0.3: {:?}",
        driver.painted_texts()
    );
    assert!(
        !driver.screen_contains("\u{d7}"),
        "cancellable starts false, so no cancel glyph should paint yet: {:?}",
        driver.painted_texts()
    );
}

#[test]
fn indicators_toggling_cancellable_paints_and_clears_cancel_symbol() {
    let mut driver = WinDriver::new(IndicatorsApp::new(), INDICATORS_W, INDICATORS_H);
    driver.type_char('c');
    assert!(
        driver.screen_contains("\u{d7}"),
        "toggling 'c' should enable the cancel affordance and paint '×': {:?}",
        driver.painted_texts()
    );
    assert!(driver.screen_contains("Cancel enabled"));

    driver.type_char('c');
    assert!(
        !driver.screen_contains("\u{d7}"),
        "toggling 'c' again should disable the cancel affordance and clear '×': {:?}",
        driver.painted_texts()
    );
}

#[test]
fn indicators_space_key_advances_progress() {
    let mut driver = WinDriver::new(IndicatorsApp::new(), INDICATORS_W, INDICATORS_H);
    let reaction = driver.type_char(' ');
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.screen_contains("40%"),
        "space should advance progress by 10%, from 30% to 40%: {:?}",
        driver.painted_texts()
    );
    assert!(driver.screen_contains("Progress: 40%"));
}

#[test]
fn indicators_clicking_cancel_resets_progress() {
    let mut driver = WinDriver::new(IndicatorsApp::new(), INDICATORS_W, INDICATORS_H);
    driver.type_char('c'); // enable the cancel affordance
    let (x, y) = driver
        .find("\u{d7}")
        .unwrap_or_else(|| panic!("cancel glyph not painted: {:?}", driver.painted_texts()));

    driver.click(x, y);
    assert!(
        driver.screen_contains("Cancelled!"),
        "clicking the cancel glyph should log the cancellation: {:?}",
        driver.painted_texts()
    );
    assert!(
        driver.screen_contains("0%"),
        "cancelling should reset progress back to 0%: {:?}",
        driver.painted_texts()
    );
}

// ─── PlatformServicesDemo: win_platform_services (issue #1229) ────────────
//
// Unlike every other section in this file, `win_platform_services.rs`'s
// `render` paints nothing (see that example's module doc: "every
// `WinBackend::draw_*` rasteriser is still a `todo!()` stub" — stale now
// that #25–#30 landed the chrome rasterisers, but still true *for this
// example*, which never calls one), and every one of its key handlers
// reports its result to stderr rather than an in-window status bar,
// specifically so a human can watch it against a real desktop session.
// So there is no painted output for a headless `WinDriver` to assert on
// for `o`/`s`/`m` (blocking native dialogs — would hang a test run
// waiting on user input that never comes), `n` (a tray balloon has no
// pixel presence in a `HeadlessSurface`, and no live tray to inspect
// either), `u`/`f`/`p`/`x`/`t`/`d`/`b` (real desktop side effects: a
// browser launch, Explorer/shell UI, filesystem/Recycle-Bin mutation, a
// registry-backed theme query, real monitor enumeration, an audible
// beep — none of which a `HeadlessSurface` renders and none of which
// this crate's own `src/win/services.rs` unit tests leave unexercised
// already, see e.g. `open_url_result_and_open_path_report_a_real_shell_execute_failure`).
//
// What *is* headlessly observable:
// - the `Reaction`/exit behaviour of the app's own key dispatch (proves
//   `WinDriver` can drive this example's `AppLogic` at all, the same bar
//   every other section here clears first); and
// - the clipboard round trip, which needs no window/desktop UI at all
//   (`OpenClipboard(None)` — see `src/win/services.rs`'s
//   `win_clipboard_read`/`win_clipboard_write` docs) — exercised here via
//   `WinBackend::services()` directly rather than routing through `'c'`
//   and reading the app's `eprintln!`, which a `WinDriver` test has no
//   way to capture. This is the same real `WinPlatformServices` the `'c'`
//   handler calls, just reached without needing to observe stderr.

const PLATFORM_SERVICES_W: u32 = 400;
const PLATFORM_SERVICES_H: u32 = 200;

#[test]
fn platform_services_q_exits() {
    let mut driver = WinDriver::new(
        PlatformServicesDemo,
        PLATFORM_SERVICES_W,
        PLATFORM_SERVICES_H,
    );
    assert!(!driver.exited());
    let reaction = driver.type_char('q');
    assert_eq!(reaction, Reaction::Exit);
    assert!(driver.exited());
}

#[test]
fn platform_services_escape_exits() {
    let mut driver = WinDriver::new(
        PlatformServicesDemo,
        PLATFORM_SERVICES_W,
        PLATFORM_SERVICES_H,
    );
    let reaction = driver.press_named(NamedKey::Escape);
    assert_eq!(reaction, Reaction::Exit);
}

#[test]
fn platform_services_unbound_key_continues_and_paints_nothing() {
    let mut driver = WinDriver::new(
        PlatformServicesDemo,
        PLATFORM_SERVICES_W,
        PLATFORM_SERVICES_H,
    );
    // Every key this example binds (`c/o/s/n/m/u/t/f/p/x/b/d`) triggers a
    // real platform side effect — 'z' is deliberately unbound so this
    // reaches `_ => Reaction::Continue` with no side effect at all.
    let reaction = driver.type_char('z');
    assert_eq!(reaction, Reaction::Continue);
    assert!(
        driver.painted_texts().is_empty(),
        "this example's `render` paints nothing (see this section's own \
         doc): {:?}",
        driver.painted_texts()
    );
}

#[test]
fn platform_services_clipboard_round_trips_through_the_real_win32_clipboard() {
    let driver = WinDriver::new(
        PlatformServicesDemo,
        PLATFORM_SERVICES_W,
        PLATFORM_SERVICES_H,
    );
    const PAYLOAD: &str = "quadraui#1229 win_platform_services WinDriver clipboard round trip";

    let clipboard = driver.backend().services().clipboard();
    clipboard.write_text(PAYLOAD);
    assert_eq!(
        clipboard.read_text().as_deref(),
        Some(PAYLOAD),
        "the real Win32 clipboard (`OpenClipboard(None)`, no window \
         required) should round-trip the exact text just written — the \
         same `WinPlatformServices::clipboard()` this example's 'c' \
         handler calls"
    );
}
