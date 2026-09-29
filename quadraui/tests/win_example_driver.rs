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
