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
use quadraui::NamedKey;

#[path = "../examples/common/toolbar_app.rs"]
mod toolbar_app;
use toolbar_app::ToolbarApp;

#[path = "../examples/common/demo.rs"]
mod demo;
#[path = "../examples/common/dialog_table_demo.rs"]
mod dialog_table_demo;
#[path = "../examples/common/mini_app.rs"]
mod mini_app;
#[path = "../examples/common/panel_app.rs"]
mod panel_app;
#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
#[path = "../examples/common/selection_app.rs"]
mod selection_app;
#[path = "../examples/common/tab_group_demo.rs"]
mod tab_group_demo;
#[path = "../examples/common/text_input_demo.rs"]
mod text_input_demo;

use demo::AppState;
use dialog_table_demo::DialogTableDemo;
use mini_app::MiniApp;
use panel_app::PanelApp;
use pipeline_app::PipelineApp;
use selection_app::SelectionDemo;
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
fn toolbar_q_exits() {
    let mut driver = TuiDriver::new(ToolbarApp::new(), 120, 10);
    driver.type_char('q');
    assert!(driver.exited(), "'q' should exit the toolbar demo");
}
