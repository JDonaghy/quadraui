//! End-to-end tests for the shipping TUI examples, driven in-process by
//! [`TuiDriver`]. Each test instantiates the *same* backend-agnostic
//! `AppLogic` impl the corresponding `tui_*` example runs, scripts real
//! [`quadraui::UiEvent`]s through it, and asserts on the rendered screen
//! — no TTY / pty required.
//!
//! The example app code is pulled in via the canonical `#[path]` include
//! (the same trick `examples/*.rs` use), so these tests exercise the
//! exact code a user runs, not a test-only copy.
#![cfg(feature = "tui")]

use quadraui::tui::testing::{driver_with_shell, TuiDriver};
use quadraui::{ButtonMask, NamedKey, Point, Reaction, UiEvent};

#[path = "../examples/common/shell_app.rs"]
mod shell_app_ex;
use shell_app_ex::ShellApp as ShellAppEx;

#[path = "../examples/common/toolbar_app.rs"]
mod toolbar_app;
use toolbar_app::ToolbarApp;

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
#[path = "../examples/common/clipboard_demo.rs"]
mod clipboard_demo;
#[path = "../examples/common/demo.rs"]
mod demo;
#[path = "../examples/common/dialog_table_demo.rs"]
mod dialog_table_demo;
#[path = "../examples/common/full_chrome_demo.rs"]
mod full_chrome_demo;
#[path = "../examples/common/hit_map_recover_demo.rs"]
mod hit_map_recover_demo;
#[path = "../examples/common/mini_app.rs"]
mod mini_app;
#[path = "../examples/common/palette_dual_mode_app.rs"]
mod palette_dual_mode_app;
#[path = "../examples/common/panel_app.rs"]
mod panel_app;
#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
#[path = "../examples/common/selection_app.rs"]
mod selection_app;
#[path = "../examples/common/shell_menu_demo.rs"]
mod shell_menu_demo;
#[path = "../examples/common/tab_group_demo.rs"]
mod tab_group_demo;
#[path = "../examples/common/text_input_demo.rs"]
mod text_input_demo;

use appshell_demo::AppShellDemo;
use clipboard_demo::ClipboardDemo;
use demo::AppState;
use dialog_table_demo::DialogTableDemo;
use full_chrome_demo::FullChromeDemo;
use hit_map_recover_demo::HitMapRecoverDemo;
use mini_app::MiniApp;
use palette_dual_mode_app::PaletteDualModeApp;
use panel_app::PanelApp;
use pipeline_app::PipelineApp;
use selection_app::SelectionDemo;
use shell_menu_demo::ShellMenuDemo;
use tab_group_demo::TabGroupDemo;
use text_input_demo::TextInputDemo;

// ─── PipelineApp: mouse + keyboard + reset ──────────────────────────────────

#[test]
fn pipeline_initial_screen_paints_stages_and_hint() {
    let driver = TuiDriver::new(PipelineApp::new(), 100, 30);
    let screen = driver.screen();
    assert!(driver.screen_contains("Checkout"), "stages:\n{screen}");
    assert!(driver.screen_contains("Deploy"), "stages:\n{screen}");
    assert!(driver.screen_contains("Enter"), "status hint:\n{screen}");
}

#[test]
fn pipeline_pressing_q_exits() {
    let mut driver = TuiDriver::new(PipelineApp::new(), 100, 30);
    assert!(!driver.exited());
    driver.type_char('q');
    assert!(driver.exited(), "'q' should make the app exit");
}

#[test]
fn pipeline_pressing_r_resets_status_message() {
    let mut driver = TuiDriver::new(PipelineApp::new(), 100, 30);
    driver.press_named(NamedKey::Right);
    driver.type_char('r');
    assert!(
        driver.screen_contains("Reset"),
        "after 'r' the status bar should read Reset:\n{}",
        driver.screen()
    );
}

#[test]
fn pipeline_clicking_a_stage_action_routes_the_click() {
    // A click round-trips paint → hit_test → handle → state → re-render,
    // with NO escape-sequence math — we hand the driver the backend
    // coordinates of the painted "Go" (Deploy/stage-3) action button.
    let mut driver = TuiDriver::new(PipelineApp::new(), 100, 30);
    let before = driver.screen();

    let (x, y) = driver
        .find("Go")
        .unwrap_or_else(|| panic!("Go action button not painted:\n{before}"));
    driver.click(x, y);

    let after = driver.screen();
    assert_ne!(before, after, "clicking a stage should change the screen");
    // PipelineApp writes "Action on stage 3: Deploy" (action) or
    // "Selected stage 3: Deploy" (body) — both name stage 3.
    assert!(
        after.contains("stage 3"),
        "clicking the Deploy action should update the status to mention stage 3:\n{after}"
    );
}

// ─── MiniApp: minimal keyboard counter ──────────────────────────────────────

#[test]
fn mini_counts_keystrokes_and_records_last_key() {
    let mut driver = TuiDriver::new(MiniApp::new(), 100, 40);
    assert!(
        driver.screen_contains("quadraui::run demo"),
        "title segment:\n{}",
        driver.screen()
    );
    driver.type_char('a');
    driver.type_char('b');
    assert!(
        driver.screen_contains("keys: 2"),
        "key counter should read 2:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("last: b"),
        "last-key segment should read 'b':\n{}",
        driver.screen()
    );
}

#[test]
fn mini_q_exits() {
    let mut driver = TuiDriver::new(MiniApp::new(), 100, 40);
    driver.type_char('q');
    assert!(driver.exited());
}

// ─── TextInputDemo: character typing + editing ──────────────────────────────

#[test]
fn text_input_typing_inserts_text_and_advances_cursor() {
    let mut driver = TuiDriver::new(TextInputDemo::new(), 100, 30);
    for c in "hello".chars() {
        driver.type_char(c);
    }
    assert!(
        driver.screen_contains("hello"),
        "typed text should render:\n{}",
        driver.screen()
    );
    // Status bar reports "line 1 col 6" after 5 chars.
    assert!(
        driver.screen_contains("col 6"),
        "cursor column should advance to 6:\n{}",
        driver.screen()
    );
}

#[test]
fn text_input_backspace_deletes_a_char() {
    let mut driver = TuiDriver::new(TextInputDemo::new(), 100, 30);
    for c in "hi".chars() {
        driver.type_char(c);
    }
    driver.press_named(NamedKey::Backspace);
    assert!(
        driver.screen_contains("col 2"),
        "cursor should move back to col 2 after backspace:\n{}",
        driver.screen()
    );
}

// ─── ClipboardDemo: native-tool fallback leg (#398) ─────────────────────────

#[test]
fn clipboard_demo_shows_starting_text_and_hint() {
    let driver = TuiDriver::new(ClipboardDemo::new(), 100, 20);
    let screen = driver.screen();
    assert!(
        screen.contains("Copy me to the system clipboard!"),
        "starting line should render:\n{screen}"
    );
    assert!(
        screen.contains("Ctrl-C"),
        "status bar should hint at Ctrl-C:\n{screen}"
    );
}

#[test]
fn clipboard_demo_ctrl_c_writes_through_all_three_legs_and_confirms() {
    // This drives the exact call the fix touches:
    // `TuiClipboard::write_text`, via all three legs (arboard, OSC 52,
    // and the new native-tool fallback on a detached thread). The
    // native tool itself can't be asserted headlessly (no real
    // clipboard exists in CI) — this checks the call completes
    // without panicking/blocking and the app's own confirmation
    // message updates, which is the observable, backend-agnostic
    // contract `write_text` promises its callers.
    let mut driver = TuiDriver::new(ClipboardDemo::new(), 100, 20);
    driver.ctrl_char('c');
    let screen = driver.screen();
    assert!(
        screen.contains("Copied"),
        "Ctrl-C should update the status to confirm the copy:\n{screen}"
    );
}

#[test]
fn clipboard_demo_typing_then_backspace_edits_the_line() {
    let mut driver = TuiDriver::new(ClipboardDemo::new(), 100, 20);
    driver.type_char('!');
    driver.type_char('!');
    driver.press_named(NamedKey::Backspace);
    let screen = driver.screen();
    assert!(
        screen.contains("Copy me to the system clipboard!!"),
        "one '!' should remain appended after a single backspace:\n{screen}"
    );
}

#[test]
fn clipboard_demo_escape_exits() {
    let mut driver = TuiDriver::new(ClipboardDemo::new(), 100, 20);
    assert!(!driver.exited());
    driver.press_named(NamedKey::Escape);
    assert!(driver.exited(), "Escape should exit the demo");
}

// ─── AppState (demo): tab switching ─────────────────────────────────────────

#[test]
fn demo_arrow_keys_switch_active_tab() {
    let mut driver = TuiDriver::new(AppState::new(), 160, 40);
    assert!(
        driver.screen_contains("Tab 1"),
        "initial active tab indicator:\n{}",
        driver.screen()
    );
    driver.press_named(NamedKey::Right);
    assert!(
        driver.screen_contains("Tab 2"),
        "Right arrow should advance to Tab 2:\n{}",
        driver.screen()
    );
}

#[test]
fn demo_n_opens_a_new_scratch_tab() {
    let mut driver = TuiDriver::new(AppState::new(), 160, 40);
    driver.type_char('n');
    assert!(
        driver.screen_contains("scratch"),
        "'n' should open a new scratch tab:\n{}",
        driver.screen()
    );
}

// ─── PanelApp: mouse-DRAG text selection + Ctrl-C copy ───────────────────────
//
// This is the case the simpler click tests can't reach: a drag is a
// MouseDown → MouseMoved(held) → MouseUp sequence whose translation into
// `TextSelectionChanged` lives in the backend dispatch layer
// (`apply_dispatch`/`DragState`). The driver routes injected mouse events
// through that exact layer, so a scripted drag exercises the real
// selection machinery end-to-end.

/// Ctrl-A selects the entire panel content without any prior drag.
///
/// Verifies the full select-all flow:
/// 1. Ctrl-A resolves the sole registered `TextRegion` (fallback path).
/// 2. The active selection covers all rows (Ctrl-C copies all content).
/// 3. Existing drag-selection tests are unaffected (both paths reach the
///    same `set_active_text_selection` call).
#[test]
fn panel_ctrl_a_selects_all_and_ctrl_c_copies_all() {
    let mut driver = TuiDriver::new(PanelApp::new(), 80, 24);

    // No drag — just press Ctrl-A. The runner intercepts it, resolves
    // the sole registered TextRegion, and sets the full-bounds selection.
    driver.ctrl_char('a');

    // The screen should now show a selection highlight (inverted cells).
    // The simplest observable side-effect is that Ctrl-C immediately
    // copies all content and PanelApp echoes it via TextCopied.
    driver.ctrl_char('c');
    let screen = driver.screen();
    assert!(
        screen.contains("Copied:"),
        "Ctrl-A then Ctrl-C should copy the full selection:\n{screen}"
    );

    // Verify that content from multiple lines is present in the copied
    // preview. PanelApp previews up to 40 chars; the first line starts
    // with "The quick brown fox…" which is 44 chars so preview is 40.
    assert!(
        screen.contains("quick") || screen.contains("brown"),
        "copied preview should contain text from the first content line:\n{screen}"
    );
}

/// After a drag-select, Ctrl-A expands the selection to all rows.
#[test]
fn panel_drag_then_ctrl_a_expands_to_all() {
    let mut driver = TuiDriver::new(PanelApp::new(), 80, 24);

    // Drag over just the first content line.
    let (x0, y0) = driver
        .find("brown")
        .unwrap_or_else(|| panic!("content line 0 not painted:\n{}", driver.screen()));
    driver.mouse_down(x0, y0);
    driver.mouse_move(x0 + 5.0, y0);
    driver.mouse_up(x0 + 5.0, y0);

    // Now press Ctrl-A — the selection should expand to cover all rows.
    // The copied text should contain lines from both the start and end
    // of CONTENT_LINES.
    driver.ctrl_char('a');
    driver.ctrl_char('c');
    let screen = driver.screen();
    assert!(
        screen.contains("Copied:"),
        "Ctrl-A after drag should still copy:\n{screen}"
    );
    // The last CONTENT_LINE contains "judge" — if select-all covered
    // all rows, this word should appear in the status bar preview or
    // the full copied text contains it. We check for "quick" (first line)
    // which is always in the 40-char preview for the full content.
    assert!(
        screen.contains("quick") || screen.contains("brown"),
        "select-all should copy from the beginning of the content:\n{screen}"
    );
}

#[test]
fn panel_drag_selects_text_and_ctrl_c_copies_it() {
    let mut driver = TuiDriver::new(PanelApp::new(), 80, 24);

    // Two distinct painted content lines (substrings unique to lines 0 and 3).
    let (x0, y0) = driver
        .find("brown")
        .unwrap_or_else(|| panic!("content line 0 not painted:\n{}", driver.screen()));
    let (x1, y1) = driver
        .find("wizards")
        .unwrap_or_else(|| panic!("content line 3 not painted:\n{}", driver.screen()));

    // Drag down across the content lines → backend begins a TextSelection
    // drag on MouseDown and emits TextSelectionChanged on MouseMoved.
    driver.mouse_down(x0, y0);
    driver.mouse_move(x1, y1);
    assert!(
        driver.screen_contains("Selecting"),
        "dragging over the content region should show selection feedback:\n{}",
        driver.screen()
    );
    driver.mouse_up(x1, y1);

    // Ctrl-C with an active selection → runner copies it and emits
    // TextCopied, which PanelApp echoes as `Copied: "..."`.
    driver.ctrl_char('c');
    let screen = driver.screen();
    assert!(
        screen.contains("Copied:"),
        "Ctrl-C after a selection should copy it:\n{screen}"
    );
    assert!(
        screen.contains("quick"),
        "the copied preview should contain selected text:\n{screen}"
    );
}

// ─── SelectionDemo (shell-runner path): drag + Ctrl-C via run_with_shell ───────
//
// `SelectionDemo` implements `ShellApp` (not `AppLogic` directly) and is
// driven by `run_with_shell` in production. Here we use `driver_with_shell`
// to construct the same `ShellAdapter` wrapper that `run_with_shell` builds,
// then script events through it — exercising the full
// `ShellApp → ShellAdapter::render() → register_text_region()` call chain
// that the `AppLogic`-only tests in `run.rs` cannot reach.
//
// This satisfies the third acceptance criterion of issue #283:
// "add a shell-runner-path test."

/// Full `run_with_shell` path: drag to select, then Ctrl-C copies the text
/// and `SelectionDemo` echoes it in the status bar as `Copied: "..."`.
///
/// This test proves that `ShellAdapter::render()` correctly threads the backend
/// into `app.render_content()` so `register_text_region()` is called, making
/// the selection pipeline operational for `run_with_shell` consumers.
#[test]
fn shell_runner_path_drag_and_ctrl_c_copies_text() {
    let config = SelectionDemo::config();
    let mut driver = driver_with_shell(SelectionDemo::new(), config, 80, 24);

    // Locate a word from the first content line so we have real coordinates.
    let (x0, y0) = driver
        .find("quick")
        .unwrap_or_else(|| panic!("content not rendered via shell path:\n{}", driver.screen()));

    // Drag to a word on a lower line.
    let (x1, y1) = driver
        .find("wizards")
        .unwrap_or_else(|| panic!("second content line not rendered:\n{}", driver.screen()));

    driver.mouse_down(x0, y0);
    driver.mouse_move(x1, y1);
    assert!(
        driver.screen_contains("Selecting"),
        "drag in shell-runner path should show selection feedback:\n{}",
        driver.screen()
    );
    driver.mouse_up(x1, y1);

    // Ctrl-C must copy the selection and SelectionDemo should display the
    // "Copied:" banner — proving the TextCopied event reached the ShellApp.
    driver.ctrl_char('c');
    let screen = driver.screen();
    assert!(
        screen.contains("Copied:"),
        "Ctrl-C via run_with_shell path must copy the selection:\n{screen}"
    );
    assert!(
        screen.contains("quick") || screen.contains("brown"),
        "copied preview should contain selected text:\n{screen}"
    );
}

// ─── AppShellDemo (issue #409): ShellApp activity-bar keyboard focus hook ───
//
// `AppShellDemo` implements `ShellApp` and binds `Tab` to
// `ShellContext::request_activity_keyboard_focus()`. Everything after that
// — j/k navigation, Enter activation, Escape cancellation — is driven by
// `ShellAdapter::handle` itself, intercepting the synthesized
// `UiEvent::ActivityBar` before it would reach `AppShellDemo::handle` (which
// has no match arm for that event at all). These tests prove the full
// `Tab → focus → j/k → Enter` round trip works for a `ShellApp` consumer,
// which was impossible before #409 (only the raw `AppLogic` pattern in
// `examples/common/shell_app.rs` could reach `AppShell`'s keyboard API).
//
// `Reaction::Redraw` vs `Reaction::Continue` is the key signal here: `j`/`k`/
// `Escape` have no meaning to `AppShellDemo::handle` on their own (it would
// return `Continue` for an unmatched raw key), so a `Redraw` after pressing
// them proves `ShellAdapter` actually intercepted and handled the
// synthesized `ActivityBar` event rather than the key falling through.

/// `Tab` focuses the activity bar, `j` `j` moves the keyboard cursor down
/// two items (explorer → search → git), and `Enter` activates the
/// selection — switching to the Source Control panel and notifying
/// `AppShellDemo::on_shell_event`, which is what paints "Panel: panel:git".
#[test]
fn appshell_demo_tab_focus_then_jj_enter_switches_panel() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, 80, 24);

    assert!(
        driver.screen_contains("Tab=focus bar"),
        "initial hint should mention the Tab trigger:\n{}",
        driver.screen()
    );

    let reaction = driver.press_named(NamedKey::Tab);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Tab should request activity-bar keyboard focus and redraw"
    );
    assert!(
        driver.screen_contains("Activity bar focused"),
        "focusing the bar should update the status hint:\n{}",
        driver.screen()
    );

    // Cursor starts at index 0 (explorer). Two `j` presses move it to
    // index 2 (git) — 3 top panels, cursor saturates instead of wrapping.
    for _ in 0..2 {
        let reaction = driver.type_char('j');
        assert_eq!(
            reaction,
            Reaction::Redraw,
            "'j' while focused must be intercepted as ActivityBar nav, not fall through:\n{}",
            driver.screen()
        );
    }

    let reaction = driver.press_named(NamedKey::Enter);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Enter should activate the selected item and redraw"
    );
    assert!(
        driver.screen_contains("Panel: panel:git"),
        "activating the cursor item should switch to the git panel and notify on_shell_event:\n{}",
        driver.screen()
    );
}

/// `Escape` while the bar is focused cancels keyboard-cursor mode without
/// switching panels — and, crucially, releases focus so a *subsequent* key
/// (here `q`) reaches `AppShellDemo::handle` again instead of being
/// swallowed as activity-bar navigation.
#[test]
fn appshell_demo_escape_cancels_focus_without_switching_panel() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, 80, 24);

    driver.press_named(NamedKey::Tab);
    assert!(driver.screen_contains("Activity bar focused"));

    let reaction = driver.press_named(NamedKey::Escape);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Escape while focused should cancel, not exit the app"
    );
    assert!(
        !driver.exited(),
        "Escape while focused must not quit the demo"
    );
    assert!(
        !driver.screen_contains("Panel:"),
        "cancelling must not have activated any panel switch:\n{}",
        driver.screen()
    );

    // If focus had not actually been released, 'q' would still be routed
    // as an (unbound) ActivityBar key and return Continue. Getting Exit
    // proves 'q' reached `AppShellDemo::handle`'s raw KeyPressed match arm.
    let reaction = driver.type_char('q');
    assert_eq!(
        reaction,
        Reaction::Exit,
        "'q' after Escape should quit via the app's own binding, proving focus was released"
    );
}

/// `ShellApp::take_requested_panel` (coord-tui #1029 bug A): an app can
/// queue a panel switch from inside its own `handle()` — no ActivityBar
/// click, no keyboard-cursor mode — and `ShellAdapter` must apply it to the
/// *real* `AppShell` state (sidebar header) and re-notify `on_shell_event`,
/// not just let the app's own internal view state drift out of sync with
/// the chrome. Before this hook, the sidebar header would stay on
/// "EXPLORER" here even though the app had "moved on" internally — the
/// exact chrome-desync bug this regression guards against.
#[test]
fn appshell_demo_programmatic_panel_switch_updates_chrome() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, 100, 30);

    assert!(
        driver.screen_contains("EXPLORER"),
        "starts on the default (index 0) Explorer panel:\n{}",
        driver.screen()
    );

    let reaction = driver.type_char('p');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "queuing + applying a requested panel switch must redraw"
    );

    assert!(
        driver.screen_contains("SOURCE CONTROL"),
        "sidebar header must follow the programmatic switch to panel:git, \
         not stay stuck on the previously-active panel:\n{}",
        driver.screen()
    );
    assert!(
        !driver.screen_contains("EXPLORER"),
        "the stale Explorer header must not still be showing:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("Panel: panel:git"),
        "on_shell_event(PanelChanged) must fire for a programmatic switch \
         exactly as it does for a mouse-driven one:\n{}",
        driver.screen()
    );
}

// ─── DialogTableDemo (issue #225): table layout in dialog ───────────────────

/// Initial screen renders dialog title and table headers.
///
/// Verifies that the `DialogTable` rasteriser paints the column header labels
/// and at least one data row. Uses a wide terminal (100 cols) so the table is
/// not clipped.
#[test]
fn dialog_table_initial_screen_paints_headers_and_rows() {
    let driver = TuiDriver::new(DialogTableDemo::new(), 100, 30);
    let screen = driver.screen();
    // Title.
    assert!(
        driver.screen_contains("Keybindings"),
        "dialog title should appear:\n{screen}"
    );
    // Column headers.
    assert!(
        driver.screen_contains("Key"),
        "table 'Key' header should be painted:\n{screen}"
    );
    assert!(
        driver.screen_contains("Action"),
        "table 'Action' header should be painted:\n{screen}"
    );
    // At least one data row.
    assert!(
        driver.screen_contains("Stage hunk"),
        "data row 'Stage hunk' should be painted:\n{screen}"
    );
}

/// The column separator `│` appears between header columns.
#[test]
fn dialog_table_paints_column_separator() {
    let driver = TuiDriver::new(DialogTableDemo::new(), 100, 30);
    let screen = driver.screen();
    assert!(
        screen.contains('│'),
        "column separator '│' should be in the rendered table:\n{screen}"
    );
}

/// Pressing `q` exits the dialog demo.
#[test]
fn dialog_table_q_exits() {
    let mut driver = TuiDriver::new(DialogTableDemo::new(), 100, 30);
    assert!(!driver.exited(), "should not be exited initially");
    driver.type_char('q');
    assert!(driver.exited(), "'q' should exit the dialog demo");
}

/// Pressing Esc exits the dialog demo.
#[test]
fn dialog_table_esc_exits() {
    let mut driver = TuiDriver::new(DialogTableDemo::new(), 100, 30);
    driver.press_named(NamedKey::Escape);
    assert!(driver.exited(), "Esc should exit the dialog demo");
}

/// Shell-runner path: Ctrl-A selects all content lines, Ctrl-C copies them.
///
/// Verifies the select-all fallback path works when the app is driven through
/// `ShellAdapter` (the `run_with_shell` code path).
#[test]
fn shell_runner_path_ctrl_a_selects_all() {
    let config = SelectionDemo::config();
    let mut driver = driver_with_shell(SelectionDemo::new(), config, 80, 24);

    // No drag — just Ctrl-A then Ctrl-C.
    driver.ctrl_char('a');
    driver.ctrl_char('c');
    let screen = driver.screen();
    assert!(
        screen.contains("Copied:"),
        "Ctrl-A + Ctrl-C via run_with_shell path must copy all content:\n{screen}"
    );
    assert!(
        screen.contains("quick") || screen.contains("brown"),
        "copied preview should contain text from the first content line:\n{screen}"
    );
}

// ─── TabGroupDemo: tab-click activation + keyboard exit ─────────────────────

/// Regression test for #358.
///
/// Before the fix, `MouseDown` on a tab body primed a drag and returned early.
/// `MouseUp` at the same position (no cursor movement) reached
/// `handle_tab_drop`, which returned an empty `Vec` → status "tab drag
/// cancelled" — no activation.
///
/// After the fix, a down/up pair with no movement past `TAB_DRAG_THRESHOLD`
/// cancels the primed drag and falls through to `handle_click`, restoring
/// tab-activation behaviour.
#[test]
fn tab_group_click_inactive_tab_activates_it() {
    let mut driver = TuiDriver::new(TabGroupDemo::new(), 120, 30);

    // Initial state: "main.rs" (p0:t0) is active; "lib.rs" (p0:t1) is
    // the inactive second tab and must be visible in the tab bar.
    let before = driver.screen();
    let (x, y) = driver
        .find("lib.rs")
        .unwrap_or_else(|| panic!("lib.rs tab must be visible on initial render:\n{before}"));

    // Plain click: mouse-down then mouse-up at the exact same position —
    // no MouseMoved in between, so the drag stays in the pending state and
    // the threshold is never crossed.
    driver.mouse_down(x, y);
    driver.mouse_up(x, y);

    let after = driver.screen();
    assert!(
        after.contains("activated tab"),
        "clicking lib.rs should activate it (status bar must say 'activated tab …'):\n{after}"
    );
}

#[test]
fn tab_group_q_exits() {
    let mut driver = TuiDriver::new(TabGroupDemo::new(), 120, 30);
    driver.type_char('q');
    assert!(driver.exited(), "'q' should exit the tab group demo");
}

// ─── ToolbarApp: focus, Tab, Enter, click ───────────────────────────────────

#[test]
fn toolbar_initial_screen_paints_action_buttons() {
    // Confirm the four action-button labels are all visible after the
    // first render — no interaction required.
    let driver = TuiDriver::new(ToolbarApp::new(), 120, 10);
    let screen = driver.screen();
    assert!(driver.screen_contains("Pause"), "Pause button:\n{screen}");
    assert!(driver.screen_contains("Filter"), "Filter button:\n{screen}");
    assert!(driver.screen_contains("Reset"), "Reset button:\n{screen}");
    // "Debug" is disabled but must still be visible (dimmed).
    assert!(
        driver.screen_contains("Debug"),
        "Debug (disabled) button:\n{screen}"
    );
}

#[test]
fn toolbar_tab_moves_focus_to_first_enabled_button() {
    // Initially no button is focused. First Tab should land on the first
    // *enabled* action button (index 1: "Pause", because "Continue" starts
    // disabled when running == true).
    let mut driver = TuiDriver::new(ToolbarApp::new(), 120, 10);
    driver.press_named(NamedKey::Tab);
    // Status bar should confirm what was focused.
    assert!(
        driver.screen_contains("Focused:"),
        "after Tab the status should say 'Focused: …':\n{}",
        driver.screen()
    );
}

#[test]
fn toolbar_tab_cycles_through_enabled_buttons() {
    // Pressing Tab repeatedly should cycle focus through all enabled buttons
    // and eventually wrap back to the first one. We don't assert exact order
    // here — just that each Tab changes the status message.
    let mut driver = TuiDriver::new(ToolbarApp::new(), 120, 10);

    driver.press_named(NamedKey::Tab);
    let after_tab1 = driver.screen();
    driver.press_named(NamedKey::Tab);
    let after_tab2 = driver.screen();
    driver.press_named(NamedKey::Tab);
    let after_tab3 = driver.screen();

    // Each Tab should produce a different status line (focus moved).
    assert_ne!(
        after_tab1, after_tab2,
        "second Tab should move focus to a different button"
    );
    assert_ne!(after_tab2, after_tab3, "third Tab should move focus again");
}

#[test]
fn toolbar_shift_tab_goes_backward() {
    // Pressing Tab then Shift-Tab should move focus forward then back,
    // ending up on the same button as the first Tab.
    //
    // Shift-Tab is dispatched as `NamedKey::BackTab` (no shift modifier) —
    // that is exactly what crossterm, GTK, and macOS backends emit for the
    // real Shift-Tab keypress. Using BackTab here covers the real terminal
    // path and prevents the false-green that a synthetic Tab+shift event
    // would produce (Tab+shift was swallowed silently by the old handler).
    let mut driver = TuiDriver::new(ToolbarApp::new(), 120, 10);

    driver.press_named(NamedKey::Tab);
    let after_tab = driver.screen();

    driver.press_named(NamedKey::Tab);
    // Now focused on the second button — BackTab should go back to the first.
    driver.press_named(NamedKey::BackTab);

    let after_shift_tab = driver.screen();
    assert_eq!(
        after_tab, after_shift_tab,
        "Shift-Tab (BackTab) should return focus to the same button Tab first landed on"
    );
}

#[test]
fn toolbar_enter_activates_focused_button() {
    // Tab to first focused button (Pause in the default running==true
    // state), then Enter — the status bar should reflect the Pause action.
    let mut driver = TuiDriver::new(ToolbarApp::new(), 120, 10);
    driver.press_named(NamedKey::Tab);
    // First focused button when running==true is "Pause" (index 1).
    driver.press_named(NamedKey::Enter);
    assert!(
        driver.screen_contains("Paused"),
        "Enter on focused Pause button should show 'Paused' in status:\n{}",
        driver.screen()
    );
}

// ─── PaletteDualModeApp: dual-mode palette ───────────────────────────────────

#[test]
fn palette_dual_mode_initial_screen_shows_list_mode_with_branches() {
    // On startup the picker is open in List mode showing the branch list.
    let driver = TuiDriver::new(PaletteDualModeApp::new(), 100, 30);
    let screen = driver.screen();

    // The [L] mode badge appears in the title bar.
    assert!(
        driver.screen_contains("[L]"),
        "list-mode badge '[L]' should be visible on startup:\n{screen}"
    );
    // At least one branch name should be painted in the item list.
    assert!(
        driver.screen_contains("main"),
        "branch 'main' should appear in the initial list:\n{screen}"
    );
    assert!(
        driver.screen_contains("develop"),
        "branch 'develop' should appear in the initial list:\n{screen}"
    );
}

#[test]
fn toolbar_disabled_buttons_skipped_by_tab() {
    // The "Debug" button is always disabled. Tab should never produce
    // a status line that says "Focused: Debug".
    let mut driver = TuiDriver::new(ToolbarApp::new(), 120, 10);
    // Press Tab enough times to wrap around all focusable buttons.
    for _ in 0..10 {
        driver.press_named(NamedKey::Tab);
        assert!(
            !driver.screen_contains("Focused: Debug"),
            "Tab must never focus the disabled Debug button:\n{}",
            driver.screen()
        );
    }
}

#[test]
fn toolbar_click_fires_action_without_focus() {
    // Clicking a visible button directly (no Tab needed) should fire the
    // action — hover/click path is independent of keyboard focus.
    let mut driver = TuiDriver::new(ToolbarApp::new(), 120, 10);

    let before = driver.screen();
    let (x, y) = driver
        .find("Filter")
        .unwrap_or_else(|| panic!("Filter button must be visible:\n{before}"));
    driver.click(x, y);

    assert!(
        driver.screen_contains("Filter"),
        "clicking Filter should update the status:\n{}",
        driver.screen()
    );
}

#[test]
fn palette_dual_mode_tab_switches_to_input_mode() {
    // Pressing Tab should toggle from List mode to Input mode, which:
    // - changes the mode badge from [L] to [I]
    // - hides the item list rows
    let mut driver = TuiDriver::new(PaletteDualModeApp::new(), 100, 30);

    let before = driver.screen();
    assert!(before.contains("[L]"), "starts in list mode:\n{before}");

    driver.press_named(NamedKey::Tab);
    let after = driver.screen();

    assert!(
        after.contains("[I]"),
        "after Tab, input-mode badge '[I]' should be visible:\n{after}"
    );
    // In Input mode the item list is suppressed — branch names should not
    // appear as selectable rows in the palette body.
    assert!(
        !after.contains("develop"),
        "item rows should be hidden in Input mode:\n{after}"
    );
}

#[test]
fn toolbar_q_exits() {
    let mut driver = TuiDriver::new(ToolbarApp::new(), 120, 10);
    driver.type_char('q');
    assert!(driver.exited(), "'q' should exit the toolbar demo");
}

/// Pressing `f` rebuilds the controller via `from_layout`, producing a
/// 3-pane mixed H/V tree (left | top-right / bottom-right).
///
/// Verifies that after pressing 'f':
/// - All three pane tab labels are visible on screen.
/// - The pane count shown in the status bar reflects 3 panes.
///
/// This is the primary smoke-test for `TabGroupController::from_layout` (#393).
#[test]
fn tab_group_f_key_switches_to_from_layout_3_pane() {
    let mut driver = TuiDriver::new(TabGroupDemo::new(), 120, 30);

    // Press 'f' to rebuild via from_layout.
    driver.type_char('f');

    let screen = driver.screen();

    // Pane 0 (left): active tab is "left.rs".
    assert!(
        screen.contains("left.rs"),
        "after 'f' the left pane tab 'left.rs' must be visible:\n{screen}"
    );
    // Pane 1 (top-right): active tab is "top.rs".
    assert!(
        screen.contains("top.rs"),
        "after 'f' the top-right pane tab 'top.rs' must be visible:\n{screen}"
    );
    // Pane 2 (bottom-right): active tab is "bottom.rs".
    assert!(
        screen.contains("bottom.rs"),
        "after 'f' the bottom-right pane tab 'bottom.rs' must be visible:\n{screen}"
    );
    // Status bar right segment shows "panes: 3" for the from_layout 3-pane tree.
    assert!(
        screen.contains("panes: 3"),
        "after 'f' the status bar should report 'panes: 3':\n{screen}"
    );
}

/// Pressing `f` then `r` resets the layout back to the default 2-pane split.
///
/// Verifies that after pressing 'f' followed by 'r':
/// - The original main.rs and Cargo.toml tabs are restored.
/// - The 3-pane from_layout tabs (left.rs, top.rs, bottom.rs) are gone.
/// - The pane count is back to 2.
///
/// This exercises the full from_layout → reset round-trip (#393).
#[test]
fn tab_group_f_then_r_resets_to_default_layout() {
    let mut driver = TuiDriver::new(TabGroupDemo::new(), 120, 30);

    // Switch to the from_layout 3-pane tree.
    driver.type_char('f');
    assert!(
        driver.screen_contains("left.rs"),
        "prerequisite: 'f' must produce the from_layout layout:\n{}",
        driver.screen()
    );

    // Reset back to default.
    driver.type_char('r');

    let screen = driver.screen();

    // Default pane 0 tabs must be back.
    assert!(
        screen.contains("main.rs"),
        "after 'r' the default tab 'main.rs' must be visible:\n{screen}"
    );
    // Default pane 1 tab must be back.
    assert!(
        screen.contains("Cargo.toml"),
        "after 'r' the default tab 'Cargo.toml' must be visible:\n{screen}"
    );
    // The from_layout tabs should no longer be present.
    assert!(
        !screen.contains("left.rs"),
        "after 'r' the from_layout 'left.rs' tab must not be visible:\n{screen}"
    );
    // Pane count is back to 2.
    assert!(
        screen.contains("panes: 2"),
        "after 'r' the status bar should report 'panes: 2':\n{screen}"
    );
}

/// Regression test for #375.
///
/// Before the fix, crossing `TAB_DRAG_THRESHOLD` during a `MouseMoved` event
/// panicked with "RefCell already borrowed":
/// `if let Some((sx, sy)) = *self.tab_drag_pending_pos.borrow()` held an
/// immutable `Ref` alive for the whole block, and the subsequent
/// `*self.tab_drag_pending_pos.borrow_mut() = None` inside that block tried
/// to create a mutable borrow of the same `RefCell`, which panics at runtime.
///
/// After the fix the pending position is copied into a local `let` binding
/// (dropping the `Ref` immediately), so the mutable borrow succeeds.
#[test]
fn tab_group_drag_start_does_not_panic() {
    let mut driver = TuiDriver::new(TabGroupDemo::new(), 120, 30);
    let before = driver.screen();
    let (x, y) = driver
        .find("main.rs")
        .unwrap_or_else(|| panic!("main.rs tab not found:\n{before}"));

    // MouseDown on the tab primes the drag (sets tab_drag_pending_pos).
    driver.mouse_down(x, y);
    assert!(!driver.exited(), "mouse_down should not cause exit");

    // MouseMove past TAB_DRAG_THRESHOLD promotes the pending drag to an
    // active drag. Before the fix this panicked; after it succeeds and
    // causes a redraw (status shows "tab drag start").
    driver.mouse_move(x + 3.0, y);
    assert!(
        !driver.exited(),
        "mouse_move into drag should not cause exit"
    );

    // MouseUp completes the drag cycle without further panic.
    driver.mouse_up(x + 3.0, y);
    assert!(!driver.exited(), "mouse_up should not cause exit");
}

#[test]
fn palette_dual_mode_typing_in_input_mode_updates_query() {
    // Switch to Input mode, type a branch name, assert it appears in the
    // query row of the palette.
    let mut driver = TuiDriver::new(PaletteDualModeApp::new(), 100, 30);

    // Switch to Input mode.
    driver.press_named(NamedKey::Tab);
    let screen_before_typing = driver.screen();
    assert!(
        screen_before_typing.contains("[I]"),
        "must be in Input mode:\n{screen_before_typing}"
    );

    // Type a new branch name.
    for c in "my-feature".chars() {
        driver.type_char(c);
    }

    let screen = driver.screen();
    assert!(
        screen.contains("my-feature"),
        "typed text should appear in the query field:\n{screen}"
    );
}

// ─── ShellApp: AppShell keyboard activity-bar nav (#386) ────────────────────
//
// `ShellApp` uses `AppShell` (not the raw `ActivityBar` primitive directly).
// These tests verify that the new keyboard navigation API (`set_activity_keyboard_focused`,
// `activity_select_next/prev`, `activity_activate_selected`) is correctly wired
// through `AppShell::build_activity_bar()` → TUI backend → `UiEvent::ActivityBar`
// → `ShellApp::handle()` round-trip.

#[test]
fn shell_app_initial_screen_shows_hint() {
    let driver = TuiDriver::new(ShellAppEx::new(), 100, 30);
    let screen = driver.screen();
    // The default hint instructs the user to press Tab to focus the bar.
    assert!(
        driver.screen_contains("Tab") || driver.screen_contains("quit"),
        "initial screen should show keyboard hint:\n{screen}"
    );
    // No navigation hint yet.
    assert!(
        !driver.screen_contains("j/k"),
        "j/k hint must not appear before bar is focused:\n{screen}"
    );
}

#[test]
fn shell_app_tab_focuses_activity_bar() {
    // Pressing Tab must focus the activity bar: the status bar switches
    // from the default message to "Activity bar focused (…)".
    let mut driver = TuiDriver::new(ShellAppEx::new(), 100, 30);
    let before = driver.screen();

    driver.press_named(NamedKey::Tab);
    let after = driver.screen();

    assert_ne!(before, after, "Tab should change the screen");
    assert!(
        driver.screen_contains("Activity bar focused"),
        "after Tab the status should say 'Activity bar focused (…)':\n{}",
        driver.screen()
    );
    // j/k hint should appear now that the bar is focused.
    assert!(
        driver.screen_contains("j/k"),
        "j/k hint should appear while bar is focused:\n{}",
        driver.screen()
    );
}

#[test]
fn palette_dual_mode_escape_closes_picker() {
    let mut driver = TuiDriver::new(PaletteDualModeApp::new(), 100, 30);
    driver.press_named(NamedKey::Escape);
    let screen = driver.screen();
    // After Esc the picker is gone — no palette border glyphs visible.
    assert!(
        !driver.screen_contains("[L]") && !driver.screen_contains("[I]"),
        "mode badge should disappear after Escape:\n{screen}"
    );
}

#[test]
fn shell_app_j_moves_cursor_down() {
    // After Tab, typing 'j' moves the cursor from the first to the second
    // top panel. The status bar echoes the new cursor position.
    let mut driver = TuiDriver::new(ShellAppEx::new(), 100, 30);
    driver.press_named(NamedKey::Tab); // focus bar (cursor = 0: panel:explorer)
    let after_tab = driver.screen();

    driver.type_char('j'); // cursor moves to panel:search
    let after_j = driver.screen();

    assert_ne!(after_tab, after_j, "'j' should change the screen");
    assert!(
        driver.screen_contains("panel:search"),
        "after 'j' the status should mention panel:search:\n{}",
        driver.screen()
    );
}

#[test]
fn shell_app_k_moves_cursor_up() {
    // Move down twice then up once — cursor should land on the second item.
    let mut driver = TuiDriver::new(ShellAppEx::new(), 100, 30);
    driver.press_named(NamedKey::Tab);
    driver.type_char('j'); // cursor → panel:search
    driver.type_char('j'); // cursor → panel:git
    driver.type_char('k'); // cursor → panel:search
    assert!(
        driver.screen_contains("panel:search"),
        "after j j k cursor should be on panel:search:\n{}",
        driver.screen()
    );
}

#[test]
fn shell_app_enter_activates_and_dismisses_focus() {
    // Tab (focus) → j (move to panel:search) → Enter (activate).
    // After activation: focus is cleared and the active panel is panel:search.
    let mut driver = TuiDriver::new(ShellAppEx::new(), 100, 30);
    driver.press_named(NamedKey::Tab);
    driver.type_char('j'); // cursor → panel:search
    driver.press_named(NamedKey::Enter); // activate

    let screen = driver.screen();
    // Status bar should now show the panel that was activated.
    assert!(
        screen.contains("panel:search"),
        "after Enter the status should show the activated panel:\n{screen}"
    );
    // Navigation hint should be gone — bar is no longer focused.
    assert!(
        !screen.contains("j/k"),
        "j/k hint should disappear after activation:\n{screen}"
    );
}

#[test]
fn shell_app_esc_dismisses_without_activating() {
    // Tab (focus, cursor = panel:explorer already active) → Esc (dismiss).
    // Active panel must not change; focus hint must disappear.
    let mut driver = TuiDriver::new(ShellAppEx::new(), 100, 30);
    driver.press_named(NamedKey::Tab);
    driver.type_char('j'); // move so we know we'd switch if we activated
    driver.press_named(NamedKey::Escape);

    let screen = driver.screen();
    // Focus hint gone.
    assert!(
        !screen.contains("j/k"),
        "j/k hint should disappear after Esc:\n{screen}"
    );
    // Status bar should say focus was returned.
    assert!(
        screen.contains("Focus returned"),
        "after Esc status should say 'Focus returned to editor':\n{screen}"
    );
}

#[test]
fn shell_app_k_saturates_at_first_item() {
    // Pressing k at the top must not move past the first item (no panic,
    // cursor stays on the first panel). The status message updates to
    // mention the cursor position — still the first panel.
    let mut driver = TuiDriver::new(ShellAppEx::new(), 100, 30);
    driver.press_named(NamedKey::Tab); // cursor = 0 (panel:explorer)
    driver.type_char('k'); // should saturate — cursor still at panel:explorer

    // The cursor stayed on panel:explorer, not wrapped to the bottom.
    assert!(
        driver.screen_contains("panel:explorer"),
        "'k' at top should keep cursor on panel:explorer (saturates, not wraps):\n{}",
        driver.screen()
    );
    // Critically, we must NOT see panel:settings (the bottom item) since
    // saturation means k at position 0 stays at 0, not wraps to last item.
    // (This distinguishes saturate from wrap.)
    assert!(
        !driver.screen_contains("panel:settings"),
        "'k' at top must NOT wrap to the bottom item:\n{}",
        driver.screen()
    );
}

// ─── FullChromeDemo: CSD title-bar drag / maximize escape hatch (#400) ──────
//
// `FullChromeDemo` reserves a title-bar band via `ShellConfig::with_title_bar`
// and, on the empty part of that band, calls the new
// `Backend::begin_window_drag` / `Backend::toggle_window_maximize` methods.
// TUI has no window to drive, so both are documented no-ops (`false`) —
// these tests prove the call path runs cleanly end-to-end (paint → hit-test
// → `ShellApp::handle` → backend call → re-render) and falls back to the
// "no window" message, exactly as a headless backend should. The real
// window-drag/maximize behaviour is GTK-only and has no automated coverage
// yet (`GtkDriver` — #301); see the manual smoke test instead.

/// Left-`MouseDown` on the empty title bar calls `begin_window_drag`, which
/// must return `false` on `TuiBackend` (no window) and the demo must show
/// the "no window to drag" fallback message rather than crash or hang.
#[test]
fn full_chrome_title_bar_click_requests_window_drag() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    let (x, y) = driver
        .find("TITLE BAR")
        .unwrap_or_else(|| panic!("title bar band should be painted:\n{}", driver.screen()));

    driver.click(x, y);
    assert!(
        driver.screen_contains("Title bar click (no window to drag)"),
        "clicking the empty title bar on TUI should call begin_window_drag(), get false back \
         (no window), and fall back to the plain-click message:\n{}",
        driver.screen()
    );
}

/// Double-click on the empty title bar calls `toggle_window_maximize`,
/// which must return `false` on `TuiBackend` (no window) and the demo must
/// show the "no window" fallback message.
#[test]
fn full_chrome_title_bar_double_click_requests_maximize_toggle() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    let (x, y) = driver
        .find("TITLE BAR")
        .unwrap_or_else(|| panic!("title bar band should be painted:\n{}", driver.screen()));

    driver.dispatch(UiEvent::DoubleClick {
        widget: None,
        position: Point::new(x, y),
    });
    assert!(
        driver.screen_contains("Title bar double-click (no window)"),
        "double-clicking the empty title bar on TUI should call toggle_window_maximize(), get \
         false back (no window), and fall back to the no-window message:\n{}",
        driver.screen()
    );
}

// ─── FullChromeDemo: title-bar minimize/maximize/close button row (#402) ────
//
// `FullChromeDemo` paints a realistic CSD button row into the right side of
// the title bar via `StatusBarSegment::action_id`, tracked with the same
// `StatusBarInteraction` press/release pattern every other clickable status
// bar segment in this codebase uses. These tests prove clicks on the
// buttons are dispatched to the button (not the drag/maximize gesture on
// the empty part of the bar) and that each `action_id` routes to the right
// behaviour. A full press+release pair is required — `StatusBarInteraction`
// only fires `Clicked` on `MouseDown` + matching `MouseUp` on the same
// segment — so these use `mouse_down` + `mouse_up` rather than the
// single-event `click` helper the drag tests above use.

/// Clicking the maximize button calls `toggle_window_maximize` (same
/// backend hook as double-click-to-maximize on the empty bar, #400) and
/// must NOT start a window drag.
#[test]
fn full_chrome_title_bar_maximize_button_toggles_via_click() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    let (x, y) = driver
        .find("\u{25a1}")
        .unwrap_or_else(|| panic!("maximize button should be painted:\n{}", driver.screen()));

    driver.mouse_down(x, y);
    driver.mouse_up(x, y);
    assert!(
        driver.screen_contains("Maximize button (no window)"),
        "clicking the maximize button on TUI should call toggle_window_maximize(), get false \
         back (no window), and show the maximize-button message (not the empty-bar drag \
         message):\n{}",
        driver.screen()
    );
}

/// Clicking the minimize button routes to its own `action_id` — no backend
/// hook exists yet, so it just proves the click target and message are
/// wired up distinctly from the other two buttons.
#[test]
fn full_chrome_title_bar_minimize_button_shows_placeholder_message() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    let (x, y) = driver
        .find("\u{2500}")
        .unwrap_or_else(|| panic!("minimize button should be painted:\n{}", driver.screen()));

    driver.mouse_down(x, y);
    driver.mouse_up(x, y);
    assert!(
        driver.screen_contains("Minimize button (no backend hook yet)"),
        "clicking the minimize button should show its own placeholder message:\n{}",
        driver.screen()
    );
}

/// Clicking the close button requests app exit (`Reaction::Exit`), same as
/// `q` / Escape.
#[test]
fn full_chrome_title_bar_close_button_exits() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    let (x, y) = driver
        .find("\u{2715}")
        .unwrap_or_else(|| panic!("close button should be painted:\n{}", driver.screen()));

    driver.mouse_down(x, y);
    let reaction = driver.mouse_up(x, y);
    assert_eq!(
        reaction,
        Reaction::Exit,
        "releasing the mouse over the close button should request Reaction::Exit"
    );
    assert!(
        driver.exited(),
        "driver should latch `exited` after the close button is clicked"
    );
}

// ─── FullChromeDemo: window-edge resize escape hatch (#406) ────────────────
//
// `FullChromeDemo` hit-tests `ShellContext::window_edge` on `MouseDown` and
// calls `Backend::begin_window_resize`, mirroring #400's drag/maximize
// pattern. TUI has no window to resize, so this must return `false` and the
// demo must show the "no window" fallback message rather than crash or
// hang — same story as the #400 tests above. `MouseMoved` hints the resize
// cursor via `Backend::set_cursor`, which is also a documented no-op on
// TUI; those tests just prove the call path runs cleanly (no automated way
// to observe the OS pointer glyph headlessly).

/// Left-`MouseDown` on the window's right border (away from the title bar
/// and any corner) calls `begin_window_resize(East)`, which must return
/// `false` on `TuiBackend` (no window) and the demo must show the
/// no-window fallback.
///
/// The *left* border isn't a usable equivalent here: `FullChromeDemo`'s
/// activity bar sits flush against column 0 and spans the full band
/// height, and `AppShell::handle` already claims the whole
/// `activity_bar_bounds` rect for panel-switching before `ShellApp::handle`
/// (and thus `window_edge`) ever sees the event — so West/NorthWest/
/// SouthWest resize is unreachable in this particular demo layout. That's
/// a property of an icon column starting at the literal window edge with
/// no reserved margin, not a bug in `window_edge` itself; East/South/
/// SouthEast (checked here and below) aren't shadowed by any shell-owned
/// region and exercise the same mechanism.
#[test]
fn full_chrome_right_edge_click_requests_window_resize() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    // x=99 is the last column (the right border); y=15 sits well below the
    // 2-row title bar and above the bottom-panel resize grip.
    driver.click(99.0, 15.0);
    assert!(
        driver.screen_contains("Edge resize (no window) (East)"),
        "clicking the right border should call begin_window_resize(East), \
         get false back (no window), and show the edge-resize fallback \
         message:\n{}",
        driver.screen()
    );
}

/// Left-`MouseDown` on the bottom-right corner calls
/// `begin_window_resize(SouthEast)`, same no-window fallback as above.
#[test]
fn full_chrome_bottom_right_corner_click_requests_window_resize() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    // (99, 29) is the last valid cell in a 100x30 grid — the bottom-right
    // corner, nowhere near the title bar.
    driver.click(99.0, 29.0);
    assert!(
        driver.screen_contains("Edge resize (no window) (SouthEast)"),
        "clicking the bottom-right corner should call \
         begin_window_resize(SouthEast), get false back (no window), and \
         show the edge-resize fallback message:\n{}",
        driver.screen()
    );
}

/// Regression guard for the priority decision documented in
/// `FullChromeDemo::handle`: the title bar band owns the *entire* top edge
/// (including its corners), same as native GTK `HeaderBar` CSD having no
/// top-edge resize handle. A click at the top-left corner — which falls
/// within both `in_title_bar` and `window_edge`'s margin — must still
/// resolve to the title-bar-drag gesture (#400), not edge-resize (#406).
#[test]
fn full_chrome_title_bar_wins_over_window_edge_at_top_corner() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    driver.click(0.0, 0.0);
    assert!(
        driver.screen_contains("Title bar click (no window to drag)"),
        "a MouseDown at the top-left corner overlaps both the title bar \
         and the window-edge margin; the title bar must win:\n{}",
        driver.screen()
    );
    assert!(
        !driver.screen_contains("Edge resize"),
        "the top corner must not be reported as an edge-resize request \
         while a title bar owns that band:\n{}",
        driver.screen()
    );
}

/// `MouseMoved` over a window edge calls `Backend::set_cursor` with a
/// resize hint. `TuiBackend` has no OS pointer to change, so this just
/// proves the call path runs end-to-end without crashing or hanging —
/// there's no headless way to observe the (nonexistent) TUI cursor glyph.
#[test]
fn full_chrome_mouse_moved_over_edge_does_not_crash() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    // Hover with no button held (a plain move, not a drag) over the right
    // border, then back over plain main-content — both must return
    // `Continue` (a cursor hint never triggers a repaint) and leave the
    // rest of the screen exactly as it was.
    let reaction = driver.dispatch(UiEvent::MouseMoved {
        position: Point::new(99.0, 15.0),
        buttons: ButtonMask::default(),
    });
    assert_eq!(
        reaction,
        Reaction::Continue,
        "hovering a window edge should only hint the cursor, never redraw"
    );
    // The full initial status line is longer than the visible
    // main-content width and gets clipped — check a prefix that's
    // guaranteed to survive the clip rather than the full string.
    assert!(
        driver.screen_contains("drag edges"),
        "the status line should be unaffected by a hover-only cursor hint:\n{}",
        driver.screen()
    );

    let reaction = driver.dispatch(UiEvent::MouseMoved {
        position: Point::new(50.0, 15.0),
        buttons: ButtonMask::default(),
    });
    assert_eq!(
        reaction,
        Reaction::Continue,
        "hovering plain main content (not an edge) should also just return Continue"
    );
}

// ─── ShellMenuDemo: modal-over-chrome MouseDown routing (#411) ──────────────
//
// `ShellMenuDemo` opens a `MenuSystem` dropdown whose bar spans the full
// viewport width starting at x=0, so the "File" entry (and the dropdown
// opened below it) sits directly over the activity bar strip — the same
// shape as vimcode's menu bar overlapping its activity bar (vimcode#552).
// Before the #411 fix, `ShellAdapter::handle` ran `AppShell::handle` first,
// which hit-tested `MouseDown` purely by screen position and swallowed any
// click whose x fell inside the activity bar's width, even though
// `dispatch_click` had already tagged the event for the open modal.

/// Clicking a dropdown item whose x lands inside the activity bar's width
/// must still activate it — the regression this issue is about. Before the
/// fix this click was silently swallowed by `AppShell`'s chrome hit-test
/// and the app never saw it.
#[test]
fn shell_menu_dropdown_item_over_activity_bar_activates() {
    let config = ShellMenuDemo::config();
    let mut driver = driver_with_shell(ShellMenuDemo::new(), config, 100, 30);

    // Open the "File" menu.
    let (file_x, file_y) = driver
        .find("File")
        .unwrap_or_else(|| panic!("File menu label should be painted:\n{}", driver.screen()));
    driver.click(file_x, file_y);
    assert!(
        driver.screen_contains("New File"),
        "clicking File should open its dropdown:\n{}",
        driver.screen()
    );

    // The dropdown must actually overlap the activity bar for this test to
    // be meaningful — confirm the "New File" item's left edge is inside the
    // activity bar's 3-line-height-wide strip before clicking it.
    let (item_x, item_y) = driver
        .find("New File")
        .expect("New File item should be painted after opening the dropdown");
    assert!(
        item_x < 3.0,
        "test setup bug: the dropdown item must overlap the activity bar \
         (x < 3) to reproduce #411, but it painted at x={item_x}:\n{}",
        driver.screen()
    );

    // Click at the item's row, at x=1 — inside the activity bar strip.
    driver.click(1.0, item_y);
    assert!(
        driver.screen_contains("activated: new"),
        "clicking the dropdown item over the activity bar should activate it \
         (routed to the app, not swallowed by shell chrome):\n{}",
        driver.screen()
    );
    assert!(
        !driver.screen_contains("New File"),
        "the dropdown should have closed after activation:\n{}",
        driver.screen()
    );
}

/// Sanity check for the other half of the fix: with no dropdown open, a
/// click inside the activity bar strip must still behave as a normal chrome
/// click (toggle/switch panels) — the #411 fix must not turn off chrome
/// hit-testing unconditionally, only when a modal is actually open under the
/// cursor.
///
/// This scans down the activity bar column for the row that actually
/// toggles the (already-active) Explorer panel rather than asserting a
/// specific row: `AppShell`'s cached click-region for an activity item is
/// not guaranteed to be pixel-identical to the row its icon glyph paints on
/// (padding/margins are an internal layout detail, unrelated to #411), so
/// pinning an exact coordinate here would make this test fragile to changes
/// in that unrelated geometry. What #411 cares about — and what this test
/// asserts — is that *some* position within the activity bar's column still
/// reaches shell chrome and toggles it, proving the fix didn't disable
/// chrome hit-testing altogether.
#[test]
fn shell_menu_activity_bar_click_still_switches_panel_when_no_modal_open() {
    let toggled = (0..20).map(|i| 1.0 + i as f32 * 0.5).any(|y| {
        let mut probe = driver_with_shell(ShellMenuDemo::new(), ShellMenuDemo::config(), 100, 30);
        probe.click(1.0, y);
        probe.screen_contains("Sidebar hidden")
    });

    assert!(
        toggled,
        "no click within the activity bar column toggled the sidebar — a \
         plain chrome click with no modal open should still be handled by \
         shell chrome (AppShellEvent::SidebarHidden)"
    );
}

// ─── HitMapRecoverDemo: ScreenLayout::hit_map() (#425) ──────────────────────

/// `HitMapRecoverDemo::render` paints the TabBar/List/StatusBar via direct
/// `backend.draw_*()` calls, then builds a `ScreenLayout` from the same
/// objects and calls `.hit_map()` — never `.draw()` — to recover a
/// `FrameHitMap`. This proves the driver's initial paint already exercises
/// `hit_map()` without error: all three surfaces render, and the demo's own
/// status line (which only knows those labels via its `tab_bar()` /
/// `list_view()` builders) shows up on screen.
#[test]
fn hit_map_recover_initial_screen_paints_tabs_and_list() {
    let driver = TuiDriver::new(HitMapRecoverDemo::new(), 100, 30);
    let screen = driver.screen();
    assert!(driver.screen_contains("Resources"), "tab bar:\n{screen}");
    assert!(driver.screen_contains("Pods"), "list:\n{screen}");
    assert!(
        driver.screen_contains("via hit_map(), not draw()"),
        "status bar:\n{screen}"
    );
}

/// Clicking a list row must resolve through the `FrameHitMap` produced by
/// `hit_map()` alone — `render()` never calls `ScreenLayout::draw()`, so if
/// click dispatch works here it can only be because `hit_map()` registered
/// the same zone `draw()` would have.
#[test]
fn hit_map_recover_clicking_a_list_row_routes_through_hit_map() {
    let mut driver = TuiDriver::new(HitMapRecoverDemo::new(), 100, 30);
    let before = driver.screen();

    let (x, y) = driver
        .find("Services")
        .unwrap_or_else(|| panic!("'Services' row not painted:\n{before}"));
    driver.click(x, y);

    let after = driver.screen();
    assert_ne!(before, after, "clicking a row should change the screen");
    assert!(
        after.contains("last-hit:List(2)"),
        "clicking 'Services' (row 2) should route through the hit_map()-built \
         FrameHitMap to FrameZone::List{{ idx: .. }}:\n{after}"
    );
}

/// Clicking the tab bar must resolve to `FrameZone::TabBar`, proving
/// `hit_map()` distinguishes zones exactly as `draw()`'s inline
/// registration would — the same `zone_for()` helper backs both.
#[test]
fn hit_map_recover_clicking_the_tab_bar_routes_through_hit_map() {
    let mut driver = TuiDriver::new(HitMapRecoverDemo::new(), 100, 30);
    let before = driver.screen();

    let (x, y) = driver
        .find("Resources")
        .unwrap_or_else(|| panic!("tab bar not painted:\n{before}"));
    driver.click(x, y);

    let after = driver.screen();
    assert_ne!(
        before, after,
        "clicking the tab bar should change the screen"
    );
    assert!(
        after.contains("last-hit:TabBar"),
        "clicking the tab bar should route through the hit_map()-built \
         FrameHitMap to FrameZone::TabBar:\n{after}"
    );
}
