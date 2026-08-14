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
use quadraui::{Backend, ButtonMask, NamedKey, Point, Reaction, UiEvent};

#[path = "../examples/common/shell_app.rs"]
mod shell_app_ex;
use shell_app_ex::ShellApp as ShellAppEx;

#[path = "../examples/common/toolbar_app.rs"]
mod toolbar_app;
use toolbar_app::ToolbarApp;

#[path = "../examples/common/ai_transcript.rs"]
mod ai_transcript;
#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
// `ChartApp::last_chart_rect` is write-only in the shared example source
// (see `examples/common/chart_app.rs`) — `examples/common/mod.rs` blankets
// its whole tree with `#![allow(dead_code)]` when the example binaries
// pull it in, but that attribute doesn't reach this file's own `#[path]`
// include of the same source, so it needs a local opt-out here too.
#[path = "../examples/common/chart_app.rs"]
#[allow(dead_code)]
mod chart_app;
#[path = "../examples/common/chat_demo.rs"]
mod chat_demo;
#[path = "../examples/common/clipboard_demo.rs"]
mod clipboard_demo;
#[path = "../examples/common/data_table_app.rs"]
mod data_table_app;
#[path = "../examples/common/demo.rs"]
mod demo;
#[path = "../examples/common/dialog_table_demo.rs"]
mod dialog_table_demo;
#[path = "../examples/common/file_dialog_demo.rs"]
mod file_dialog_demo;
#[path = "../examples/common/form_groups.rs"]
mod form_groups;
#[path = "../examples/common/full_chrome_demo.rs"]
mod full_chrome_demo;
#[path = "../examples/common/help_layer_demo.rs"]
mod help_layer_demo;
#[path = "../examples/common/hit_map_recover_demo.rs"]
mod hit_map_recover_demo;
#[path = "../examples/common/indicators_app.rs"]
mod indicators_app;
#[path = "../examples/common/markdown_demo.rs"]
mod markdown_demo;
#[path = "../examples/common/mini_app.rs"]
mod mini_app;
#[path = "../examples/common/modal_occlusion_demo.rs"]
mod modal_occlusion_demo;
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
#[path = "../examples/common/sidebar_panel_app.rs"]
mod sidebar_panel_app;
#[path = "../examples/common/split_app.rs"]
mod split_app;
#[path = "../examples/common/split_tree_app.rs"]
mod split_tree_app;
#[path = "../examples/common/tab_group_demo.rs"]
mod tab_group_demo;
#[path = "../examples/common/text_input_demo.rs"]
mod text_input_demo;
#[path = "../examples/common/toast_app.rs"]
mod toast_app;

use ai_transcript::AiTranscript;
use appshell_demo::AppShellDemo;
use chart_app::ChartApp;
use chat_demo::ChatDemo;
use clipboard_demo::ClipboardDemo;
use data_table_app::DataTableApp;
use demo::AppState;
use dialog_table_demo::DialogTableDemo;
use file_dialog_demo::FileDialogDemo;
use form_groups::FormGroupsApp;
use full_chrome_demo::FullChromeDemo;
use help_layer_demo::HelpLayerDemo;
use hit_map_recover_demo::HitMapRecoverDemo;
use indicators_app::IndicatorsApp;
use markdown_demo::MarkdownDemo;
use mini_app::MiniApp;
use modal_occlusion_demo::ModalOcclusionDemo;
use palette_dual_mode_app::PaletteDualModeApp;
use panel_app::PanelApp;
use pipeline_app::PipelineApp;
use selection_app::SelectionDemo;
use shell_menu_demo::ShellMenuDemo;
use sidebar_panel_app::SidebarPanelApp;
use split_app::SplitApp;
use split_tree_app::SplitTreeApp;
use tab_group_demo::TabGroupDemo;
use text_input_demo::TextInputDemo;
use toast_app::ToastApp;

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

/// #307: `tui_app`'s bare-bones AppLogic renders cleanly (title segment +
/// quit hint painted, no panic) and exits on Ctrl-Q. `MiniApp::handle`
/// matches `Key::Char('q')` alone — it destructures only the `key` field
/// out of `UiEvent::KeyPressed { key, .. }`, so the `..` silently drops
/// whatever `modifiers` the event carries. That makes plain `q` (already
/// covered by `mini_q_exits` above) and Ctrl-Q take the exact same exit
/// path today; this test pins that modifier-agnostic contract for Ctrl-Q
/// specifically; a future change that started matching on `modifiers.ctrl`
/// to require the combo (or that gated the char match some other way)
/// would break it.
#[test]
fn mini_renders_cleanly_and_ctrl_q_exits() {
    let mut driver = TuiDriver::new(MiniApp::new(), 100, 40);
    let screen = driver.screen();
    assert!(
        driver.screen_contains("quadraui::run demo"),
        "title segment should render cleanly on the first frame:\n{screen}"
    );
    assert!(
        driver.screen_contains("press any key"),
        "quit hint should render cleanly on the first frame:\n{screen}"
    );
    assert!(!driver.exited());

    let reaction = driver.ctrl_char('q');
    assert_eq!(reaction, Reaction::Exit, "Ctrl-Q should exit");
    assert!(driver.exited(), "Ctrl-Q should make the app exit");
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

// ─── FileDialogDemo: TUI's documented "unsupported" contract ───────────────
//
// #427 implements real file dialogs for GTK only; `PlatformServices`'s TUI
// impl keeps returning `None` unconditionally (apps should provide an
// in-TUI picker instead). These tests pin that documented contract so a
// future change can't silently make the TUI path block waiting on
// something that will never resolve headlessly. The GTK path (a real,
// modal, nested-mainloop-pumped `gtk4::FileDialog`) can't be driven by
// `TuiDriver` — it's covered by the `gtk_file_dialog` example's manual
// smoke test instead (see SMOKE_TESTS in the #427 PR).

#[test]
fn file_dialog_demo_shows_starting_hint() {
    let driver = TuiDriver::new(FileDialogDemo::new(), 100, 20);
    let screen = driver.screen();
    assert!(
        screen.contains("o = open"),
        "status bar should hint at the open/save keys:\n{screen}"
    );
}

#[test]
fn file_dialog_demo_open_reports_unsupported_on_tui() {
    let mut driver = TuiDriver::new(FileDialogDemo::new(), 100, 20);
    driver.type_char('o');
    let screen = driver.screen();
    assert!(
        screen.contains("unsupported"),
        "open dialog must report None as unsupported on TUI:\n{screen}"
    );
}

#[test]
fn file_dialog_demo_save_reports_unsupported_on_tui() {
    let mut driver = TuiDriver::new(FileDialogDemo::new(), 100, 20);
    driver.type_char('s');
    let screen = driver.screen();
    assert!(
        screen.contains("unsupported"),
        "save dialog must report None as unsupported on TUI:\n{screen}"
    );
}

#[test]
fn file_dialog_demo_escape_exits() {
    let mut driver = TuiDriver::new(FileDialogDemo::new(), 100, 20);
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

/// #305: pressing `c` toggles `PanelApp`'s collapsed state. Collapsing sets
/// `layout.content_bounds` to a zero-size rect (see
/// `src/tui/panel.rs::collapsed_panel_has_no_content_hit`), so
/// `fill_content` — which bails out early when `bounds.width < 1.0` —
/// stops painting the content lines entirely. Content reappearing (or
/// not) is the observable proxy for the content rect actually
/// shrinking/growing: a handler that forgot to flip `collapsed`, or a
/// `render` that ignored it, would leave "quick" on screen throughout.
#[test]
fn panel_c_toggles_collapsed_and_hides_then_restores_content() {
    let mut driver = TuiDriver::new(PanelApp::new(), 80, 24);
    assert!(
        driver.screen_contains("quick"),
        "content should be painted before any collapse:\n{}",
        driver.screen()
    );

    driver.type_char('c');
    let collapsed_screen = driver.screen();
    assert!(
        collapsed_screen.contains("Collapsed"),
        "status bar should confirm the collapse:\n{collapsed_screen}"
    );
    assert!(
        !collapsed_screen.contains("quick"),
        "a collapsed panel's zero-size content rect must stop painting content:\n{collapsed_screen}"
    );

    driver.type_char('c');
    let expanded_screen = driver.screen();
    assert!(
        expanded_screen.contains("Expanded"),
        "status bar should confirm re-expansion:\n{expanded_screen}"
    );
    assert!(
        expanded_screen.contains("quick"),
        "re-expanding must restore the content rect and repaint content:\n{expanded_screen}"
    );
}

/// Clicking the maximize action button (rather than pressing `c`) drives the
/// same collapse toggle through `PanelHit::Action("maximize")` in
/// `PanelApp::handle`'s `MouseDown` arm — a distinct code path from the
/// keyboard shortcut that a routing bug could break independently.
#[test]
fn panel_maximize_button_click_toggles_collapsed() {
    let mut driver = TuiDriver::new(PanelApp::new(), 80, 24);
    let before = driver.screen();

    let (x, y) = driver
        .find("□")
        .unwrap_or_else(|| panic!("maximize action button not painted:\n{before}"));
    driver.click(x, y);

    let after = driver.screen();
    assert!(
        after.contains("Collapsed"),
        "clicking maximize should collapse the panel:\n{after}"
    );
    assert!(
        !after.contains("quick"),
        "collapsed content rect must stop painting content:\n{after}"
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

/// #454: `ctx.shell_mut()` reaches the real `AppShell` instance
/// `ShellAdapter` renders — `AppShellDemo` binds `Ctrl+B` to
/// `ctx.shell_mut().toggle_sidebar()`, exactly the call a consumer like
/// vimcode needs for a toggle-sidebar keybinding. Before #454 a `ShellApp`
/// had no way to reach the rendered `AppShell` at all, so this had to be
/// faked with a second, shadow `AppShell` that silently drifted from the
/// one actually painted (vimcode's `Ctrl+B` was dead on GTK because of
/// exactly this). Driving this through `driver_with_shell` — the same
/// `ShellApp` → `ShellAdapter` → `AppShell` dispatch `run_with_shell` uses
/// in production — proves the fix on the real path, not a synthetic
/// `ShellContext` built by hand.
#[test]
fn appshell_demo_ctrl_b_toggles_the_real_rendered_sidebar() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, 100, 30);

    // Sidebar starts visible: the demo paints its sidebar-content
    // placeholder text into `layout.sidebar_content_bounds`, which is
    // `Some` only while the sidebar is shown.
    assert!(
        driver.screen_contains("(sidebar content"),
        "sidebar should be visible on the initial screen:\n{}",
        driver.screen()
    );

    let reaction = driver.ctrl_char('b');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Ctrl+B toggling the real AppShell must redraw"
    );
    assert!(
        !driver.screen_contains("(sidebar content"),
        "Ctrl+B must hide the sidebar that ShellAdapter actually renders, \
         not a shadow copy:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("Sidebar hidden (Ctrl+B via ctx.shell_mut())"),
        "status line should confirm the toggle went through ctx.shell_mut():\n{}",
        driver.screen()
    );

    // Toggling again brings it back — proves `ctx.shell_mut()` mutates the
    // same live instance `render_content` reads from on the next frame,
    // round-tripping the real state rather than a one-way flag.
    let reaction = driver.ctrl_char('b');
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.screen_contains("(sidebar content"),
        "a second Ctrl+B should show the sidebar again:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("Sidebar shown (Ctrl+B via ctx.shell_mut())"),
        "status line should confirm the second toggle:\n{}",
        driver.screen()
    );
}

/// #307: the issue's suggested scenario for this demo was "open/close the
/// bottom panel (#289)" — but #289's tabbed bottom panel
/// (`BottomPanelConfig`/`BottomPanelController`) belongs to the separate
/// `tui_bottom_panel` example (`BottomPanelDemo`), not `AppShellDemo`:
/// `AppShellDemo::config()` never calls `.with_bottom_panel_config(..)` or
/// even the plain `.with_bottom_panel(..)`, so there is no bottom panel to
/// open or close here (`tui_bottom_panel` is out of scope for #307's named
/// batch). `AppShellDemo` does have its own real, currently-untested
/// open/close-a-chrome-region toggle though: `t` reveals/hides the title
/// bar at runtime via `ctx.shell_mut().set_title_bar_visible(..)` (the
/// #552 transition, see the module doc comment on `appshell_demo.rs`).
/// Reserving that row shifts everything below it down by the title bar's
/// height — `AppShell::compute_layout` re-derives `sidebar_content_bounds`
/// (and `activity_bar_bounds`/`main_content_bounds`) from `band_y` on
/// every call rather than caching an offset, so this is a real layout
/// reflow, not just a repaint. Tracking the sidebar-content label's row
/// (rather than asserting a hardcoded row number) is what makes this
/// robust to the title bar's exact height changing later.
#[test]
fn appshell_demo_t_toggles_title_bar_and_reflows_sidebar_content_row() {
    let config = AppShellDemo::config();
    let mut driver = driver_with_shell(AppShellDemo::new(), config, 100, 30);

    let before = driver
        .find_bounds("(sidebar content")
        .unwrap_or_else(|| panic!("sidebar content label not painted:\n{}", driver.screen()));

    let reaction = driver.type_char('t');
    assert_eq!(reaction, Reaction::Redraw, "'t' should redraw");
    let shown_screen = driver.screen();
    assert!(
        shown_screen.contains("Title bar shown (t)"),
        "status line should confirm the title bar was revealed:\n{shown_screen}"
    );
    let shown = driver
        .find_bounds("(sidebar content")
        .unwrap_or_else(|| panic!("sidebar content label not painted after 't':\n{shown_screen}"));
    assert!(
        shown.y > before.y,
        "reserving the title bar row should push the sidebar content row down: \
         before y={}, after y={}\n{shown_screen}",
        before.y,
        shown.y
    );

    let reaction = driver.type_char('t');
    assert_eq!(reaction, Reaction::Redraw, "second 't' should redraw");
    let hidden_screen = driver.screen();
    assert!(
        hidden_screen.contains("Title bar hidden (t)"),
        "status line should confirm the title bar was hidden again:\n{hidden_screen}"
    );
    let restored = driver.find_bounds("(sidebar content").unwrap_or_else(|| {
        panic!("sidebar content label not painted after second 't':\n{hidden_screen}")
    });
    assert_eq!(
        restored.y, before.y,
        "hiding the title bar again should restore the sidebar content row to its \
         original position:\n{hidden_screen}"
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

/// #307: `Shift+Tab` is `ShellApp`'s *other* focus-region control (see the
/// module doc comment's "`Shift+Tab` cycle sidebar sections"), distinct
/// from the activity-bar `Tab` focus the tests above cover. Real
/// terminals/backends report Shift+Tab as the dedicated
/// `NamedKey::BackTab` keycode (not `Tab` plus a shift modifier) — this
/// raw `KeyPressed` event isn't intercepted by the Tab-focuses-activity-bar
/// special case (which matches only `NamedKey::Tab`) or by
/// `AppShell::handle` (no `BackTab` arm at all), so it falls through to
/// `SidebarSystem::handle`'s own `NamedKey::BackTab` arm, which calls
/// `cycle_active(-1)`. `SidebarSystem::build_view` prefixes the active
/// section's title with `"▶ "` — the focus indicator this test follows
/// as it moves from one sidebar region to another.
#[test]
fn shell_app_shift_tab_moves_sidebar_focus_indicator_between_sections() {
    let mut driver = TuiDriver::new(ShellAppEx::new(), 100, 30);
    let before = driver.screen();
    assert!(
        driver.screen_contains("▶ PROJECT"),
        "the explorer sidebar starts with PROJECT as the active section:\n{before}"
    );
    assert!(
        !driver.screen_contains("▶ OPEN EDITORS"),
        "OPEN EDITORS should not carry the focus indicator yet:\n{before}"
    );

    driver.press_named(NamedKey::BackTab);
    let after = driver.screen();
    assert!(
        driver.screen_contains("▶ OPEN EDITORS"),
        "Shift+Tab should move the focus indicator to OPEN EDITORS:\n{after}"
    );
    assert!(
        !driver.screen_contains("▶ PROJECT"),
        "the focus indicator should leave PROJECT once it moves elsewhere:\n{after}"
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

// ─── FullChromeDemo: runtime title-bar visibility toggle (#532) ────────────
//
// `AppShell::set_title_bar_visible` (#532) is the runtime counterpart to the
// construction-time `ShellConfig::with_title_bar` builder — see its doc
// comment in `compose/app_shell.rs` for the motivating vimcode case (a menu
// bar painted into the title-bar row that the app shows/hides at runtime).
// Unit tests alongside `AppShell::layout()` already prove the layout math
// directly; these tests prove the `ctx.shell_mut()` integration point a
// `ShellApp` actually uses works end-to-end through the real
// `ShellAdapter`-rendered `AppShell` — the same precedent
// `appshell_demo_ctrl_b_toggles_the_real_rendered_sidebar` set for
// `toggle_sidebar()` (#454).

/// `Ctrl+T` on `FullChromeDemo` (which reserves a title bar via
/// `with_title_bar` in `config()`) hides the real rendered title-bar band
/// and shows it again, round-tripping through
/// `ctx.shell_mut().set_title_bar_visible`.
#[test]
fn full_chrome_ctrl_t_hides_and_shows_the_real_rendered_title_bar() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    // Title bar starts visible: `with_title_bar(1.5)` in `config()` reserves
    // the row, so the "TITLE BAR" label painted into `title_bar_bounds`
    // should be on the initial screen.
    assert!(
        driver.screen_contains("TITLE BAR"),
        "title bar should be visible on the initial screen:\n{}",
        driver.screen()
    );

    let reaction = driver.ctrl_char('t');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Ctrl+T toggling the real AppShell must redraw"
    );
    assert!(
        !driver.screen_contains("TITLE BAR"),
        "Ctrl+T must hide the title bar that ShellAdapter actually renders, \
         not a shadow copy:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("Title bar hidden (Ctrl+T via ctx.shell_mut())"),
        "status line should confirm the toggle went through ctx.shell_mut():\n{}",
        driver.screen()
    );

    // Toggling again brings it back — proves `ctx.shell_mut()` mutates the
    // same live instance `render_content` reads from on the next frame,
    // round-tripping the real state rather than a one-way flag, and that
    // the configured height (1.5 line-heights) survives the round trip.
    let reaction = driver.ctrl_char('t');
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.screen_contains("TITLE BAR"),
        "a second Ctrl+T should show the title bar again:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("Title bar shown (Ctrl+T via ctx.shell_mut())"),
        "status line should confirm the second toggle:\n{}",
        driver.screen()
    );
}

/// The operator's pinned ask on #532: confirm the activity bar reclaims the
/// title bar's row rather than leaving it blank, through the *real*
/// rendered path (not `AppShell::layout()` called directly, which the
/// `compose/app_shell.rs` unit tests already cover). Row 0 must switch from
/// the title bar's label to the activity bar's top icon once the title bar
/// is hidden — proving `ShellAdapter` re-paints from the freshly toggled
/// `has_title_bar`, not a stale cached offset.
#[test]
fn full_chrome_ctrl_t_hands_the_title_bar_row_to_the_activity_bar() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    let visible_top_row = driver.screen().lines().next().unwrap().to_string();
    assert!(
        visible_top_row.contains("TITLE BAR"),
        "row 0 should be the title bar band while it's reserved:\n{visible_top_row}"
    );

    driver.ctrl_char('t');
    let hidden_top_row = driver.screen().lines().next().unwrap().to_string();
    assert!(
        !hidden_top_row.contains("TITLE BAR"),
        "row 0 should no longer show the title bar label once it's hidden:\n{hidden_top_row}"
    );
    assert!(
        hidden_top_row.contains('E'),
        "row 0 should now be the activity bar's top row (Explorer icon 'E'), \
         proving the activity bar reclaimed the row rather than it going \
         blank:\n{hidden_top_row}"
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

    // Right border = the last column, whatever the driver's width; mid
    // height sits well below the 2-row title bar and above the
    // bottom-panel resize grip regardless of the driver's height.
    let viewport = driver.backend().viewport();
    driver.click(viewport.width - 0.5, viewport.height / 2.0);
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

    // The last valid cell of the driver's grid — the bottom-right corner,
    // nowhere near the title bar — whatever the driver's dimensions.
    let viewport = driver.backend().viewport();
    driver.click(viewport.width - 0.5, viewport.height - 0.5);
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
    let viewport = driver.backend().viewport();

    // Hover with no button held (a plain move, not a drag) over the right
    // border, then back over plain main-content — both must return
    // `Continue` (a cursor hint never triggers a repaint) and leave the
    // rest of the screen exactly as it was.
    let reaction = driver.dispatch(UiEvent::MouseMoved {
        position: Point::new(viewport.width - 0.5, viewport.height / 2.0),
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
        position: Point::new(viewport.width / 2.0, viewport.height / 2.0),
        buttons: ButtonMask::default(),
    });
    assert_eq!(
        reaction,
        Reaction::Continue,
        "hovering plain main content (not an edge) should also just return Continue"
    );
}

/// #307: the issue's suggested scenario for this demo was "menu bar,
/// toolbar, status line interactions, asserting on each" — but
/// `FullChromeDemo` has no real `MenuBar`/`Toolbar` widgets wired in (the
/// "File  Edit  View  Help" text in its title bar is a decorative label,
/// not an interactive menu — see `build_title_bar`), and its status-bar
/// row's own `ctx.in_status_bar(..)` branch is unreachable via `MouseDown`:
/// `status_bar_bounds` is exactly the window's last row, which
/// `ShellAppTrait::handle`'s `ctx.window_edge(..)` check (checked first,
/// same #406 mechanism the edge-resize tests above exercise) always claims
/// as the South edge before `ctx.in_status_bar` ever gets a look. What
/// *is* real, live, and untested here are the other two "chrome slot"
/// `MouseDown` branches this demo's own module doc says it proves
/// (bottom panel + command line) — each just updates `last_event`, which
/// `render_content` always repaints into `main_content_bounds`, so a
/// mis-routed hit-test (or a click silently swallowed by `window_edge`
/// like status-bar's) would leave the wrong message on screen.
#[test]
fn full_chrome_bottom_panel_and_command_line_clicks_update_status_line() {
    let config = FullChromeDemo::config();
    let mut driver = driver_with_shell(FullChromeDemo::new(), config, 100, 30);

    let (x, y) = driver
        .find("drag top edge to resize")
        .unwrap_or_else(|| panic!("bottom panel label not painted:\n{}", driver.screen()));
    driver.click(x, y);
    assert!(
        driver.screen_contains("Bottom panel click"),
        "clicking inside the bottom panel body should route through \
         ctx.in_bottom_panel and update the status line:\n{}",
        driver.screen()
    );

    let (x, y) = driver
        .find(":command-line-slot")
        .unwrap_or_else(|| panic!("command line slot not painted:\n{}", driver.screen()));
    driver.click(x, y);
    assert!(
        driver.screen_contains("Command line click"),
        "clicking the command line row should route through \
         ctx.in_command_line and update the status line:\n{}",
        driver.screen()
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

    // Click at the item's own painted position — already confirmed above
    // (`item_x < 3.0`) to land inside the activity bar strip.
    driver.click(item_x, item_y);
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
/// Locates the click target instead of guessing a row: `ShellMenuDemo`'s
/// first panel (Explorer) is active by default, and
/// `tui::activity_bar::draw_activity_bar` paints its left-edge accent glyph
/// (`'▎'`) at exactly the row `AppShell`'s cached hit-region reports for
/// that item — see that module's `icons_still_paint_at_the_absolute_row`
/// and `hit_regions_are_bar_relative_not_absolute` tests. `'▎'` is unique
/// to that one call site (nothing else in the TUI backend paints it), so
/// `find` unambiguously locates the active item's row without a brute-force
/// scan.
#[test]
fn shell_menu_activity_bar_click_still_switches_panel_when_no_modal_open() {
    let mut driver = driver_with_shell(ShellMenuDemo::new(), ShellMenuDemo::config(), 100, 30);

    let (x, y) = driver.find("▎").unwrap_or_else(|| {
        panic!(
            "Explorer (active by default) should paint its accent bar:\n{}",
            driver.screen()
        )
    });
    driver.click(x, y);

    assert!(
        driver.screen_contains("Sidebar hidden"),
        "clicking the active panel's activity-bar row with no modal open \
         should still be handled by shell chrome \
         (AppShellEvent::SidebarHidden):\n{}",
        driver.screen()
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

// ─── DataTableApp: pinned footer/summary row (#432) ─────────────────────────
//
// `DataTableApp` sorts by Name ascending by default, so alphabetically
// "api-gateway-..." is the first row and "worker-batch-..." is the
// last. The table's `min_total_width` is 80 columns, so the driver
// uses a wide-enough viewport (100) to keep the Restarts column (and
// its footer total) on-screen without horizontal scrolling.

#[test]
fn data_table_initial_screen_shows_footer_totals() {
    let driver = TuiDriver::new(DataTableApp::new(), 100, 20);
    let screen = driver.screen();
    assert!(
        driver.screen_contains("20 pods"),
        "footer should show the pod count total:\n{screen}"
    );
    assert!(
        driver.screen_contains("22"),
        "footer should show the summed restarts total:\n{screen}"
    );
    assert!(
        driver.screen_contains("api-gateway"),
        "alphabetically-first pod row should be visible initially:\n{screen}"
    );
}

#[test]
fn data_table_footer_stays_pinned_while_body_scrolls() {
    // A short viewport (12 rows) with 20 pods forces scrolling.
    let mut driver = TuiDriver::new(DataTableApp::new(), 100, 12);
    let before = driver.screen();
    assert!(
        driver.screen_contains("20 pods"),
        "footer should be visible before scrolling:\n{before}"
    );
    assert!(
        driver.screen_contains("api-gateway"),
        "alphabetically-first pod should be visible before scrolling:\n{before}"
    );

    // Jump selection to the last row — `ensure_visible` scrolls the
    // body so the footer's pinned position is genuinely exercised.
    driver.press_named(NamedKey::End);

    let after = driver.screen();
    assert_ne!(
        before, after,
        "scrolling to the end should change the screen"
    );
    assert!(
        !after.contains("api-gateway"),
        "first pod should have scrolled out of view:\n{after}"
    );
    assert!(
        after.contains("worker-batch"),
        "alphabetically-last pod should now be visible:\n{after}"
    );
    assert!(
        after.contains("20 pods") && after.contains("22"),
        "footer totals must stay pinned regardless of scroll_offset:\n{after}"
    );
}

#[test]
fn data_table_footer_click_does_not_change_selection() {
    let mut driver = TuiDriver::new(DataTableApp::new(), 100, 12);
    driver.press_named(NamedKey::End); // select + scroll to the last row
    let before = driver.screen();
    assert!(
        before.contains("row 20 / 20"),
        "status bar should report the last row selected:\n{before}"
    );

    let (x, y) = driver
        .find("pods")
        .unwrap_or_else(|| panic!("footer 'pods' label not painted:\n{before}"));
    driver.click(x, y);

    let after = driver.screen();
    assert!(
        after.contains("row 20 / 20"),
        "clicking the pinned footer must not change row selection \
         (footer is excluded from hit-testing as a row):\n{after}"
    );
}

#[test]
fn data_table_f_key_toggles_footer_off() {
    let mut driver = TuiDriver::new(DataTableApp::new(), 100, 20);
    assert!(driver.screen_contains("20 pods"));

    driver.type_char('f');
    let after = driver.screen();
    assert!(
        !after.contains("20 pods"),
        "'f' should hide the footer (regression guard for footer: None):\n{after}"
    );

    driver.type_char('f');
    assert!(
        driver.screen_contains("20 pods"),
        "'f' should toggle the footer back on"
    );
}

// ─── DataTableApp: body clipping, separators, resize direction (#516) ──────
//
// A tall-enough viewport (26 rows) fits all 20 pod rows + header + the
// 2-row pinned footer band + the 1-row status bar with no scrolling, so
// these tests don't have to scroll to reach any particular row first.

#[test]
fn data_table_wide_middle_column_does_not_corrupt_neighbouring_columns() {
    // Defect 1 regression: "Status" (column index 1, a *middle* column —
    // not last) holds a value far wider than its resolved Flex(1.5)
    // share. Before the fix this interleaved into Age/Restarts instead
    // of clipping at its own column boundary, corrupting their rendered
    // values (the exact "precisi0n" / "abil…" symptom from the issue).
    let driver = TuiDriver::new(DataTableApp::new(), 100, 26);
    let screen = driver.screen();
    assert!(
        driver.screen_contains("grafana"),
        "wide-status pod row should be visible:\n{screen}"
    );

    let (_, y) = driver
        .find("grafana-5f4c8d")
        .unwrap_or_else(|| panic!("grafana row not painted:\n{screen}"));
    let row = screen
        .lines()
        .nth(y as usize)
        .unwrap_or_else(|| panic!("row {y} missing from screen:\n{screen}"));

    assert!(
        row.contains("1h"),
        "Age column must survive intact next to the over-long Status cell: {row:?}"
    );
    assert!(
        row.contains("14"),
        "Restarts column must survive intact next to the over-long Status cell: {row:?}"
    );
    assert!(
        row.contains('…'),
        "the over-long Status cell should be truncated with an ellipsis, not a hard cut: {row:?}"
    );
    assert!(
        !row.contains("ImagePullBackOff waiting for registry retry backoff window"),
        "the full over-long value must not render past its own column: {row:?}"
    );
}

#[test]
fn data_table_body_rows_show_column_separators_aligned_with_header() {
    // Defect 2: body rows previously drew no separator at all, so
    // adjacent cells butted directly together.
    let driver = TuiDriver::new(DataTableApp::new(), 100, 26);
    let screen = driver.screen();

    fn sep_positions(line: &str) -> Vec<usize> {
        // Char-cell index, not byte offset — a header cell can carry a
        // multi-byte sort-arrow suffix that a body cell never has, which
        // would desync a byte-based comparison even though the cells
        // line up on screen.
        line.chars()
            .enumerate()
            .filter(|&(_, c)| c == '│')
            .map(|(i, _)| i)
            .collect()
    }

    let mut lines = screen.lines();
    let header = lines.next().expect("header row");
    let header_seps = sep_positions(header);
    assert_eq!(
        header_seps.len(),
        3,
        "header should have 3 separators (4 columns): {header:?}"
    );

    let body_row = lines.next().expect("first body row");
    let body_seps = sep_positions(body_row);
    assert_eq!(
        body_seps, header_seps,
        "body row separators should sit at the same columns as the header's:\n\
         header: {header:?}\nbody:   {body_row:?}"
    );
}

#[test]
fn data_table_divider_before_last_column_resizes_in_drag_direction() {
    // Defect 3, the literal reported symptom: dragging the divider
    // immediately before the last column (Age | Restarts) must widen
    // Age when dragged right and narrow it when dragged left — the same
    // direction as every other divider, never inverted. `Age` is
    // Flex-declared and directly precedes the last column (`Restarts`),
    // the exact shape that reproduced "resizes backward".
    let mut driver = TuiDriver::new(DataTableApp::new(), 100, 26);

    let before = driver.app().resolved_column_widths(driver.backend())[2];

    // Age's natural resolved width (Flex(0.5) among a small total weight)
    // is well under the app's own 20-unit resize floor, so the deltas
    // below are chosen generously enough that both the widen *and* the
    // narrow target land clear of that floor — otherwise both drags
    // would clamp to the same 20 and the direction assertion would be
    // vacuous rather than a real test of #516 defect 3.
    let layout = driver.app().table_layout(driver.backend());
    let age = layout.columns[2];
    let divider_x = age.x + age.width;
    let divider_y = 0.5;
    driver.drag(divider_x, divider_y, divider_x + 40.0, divider_y);

    let widened = driver.app().resolved_column_widths(driver.backend())[2];
    assert!(
        widened > before,
        "dragging the divider before the last column right should widen it: \
         before={before}, after={widened}"
    );

    // Because `Restarts` (the last column) is `Fixed`, this divider's x
    // (Age's right edge = Restarts' left edge) doesn't move when Age's
    // width changes — it's invariant. A second `mouse_down` at that
    // exact same point would fold into a synthetic `DoubleClick`
    // (`DoubleClickDetector`, `DOUBLE_CLICK_RADIUS` = 1.5) instead of a
    // fresh resize-drag start, so nudge the down-click 2 units off
    // (still within `DIVIDER_GRAB_PX` = 3.0's hit-test tolerance) —
    // the resize amount itself comes from the *move* target below, not
    // the down position, so this doesn't affect what's being measured.
    let layout2 = driver.app().table_layout(driver.backend());
    let age2 = layout2.columns[2];
    let divider_x2 = age2.x + age2.width;
    driver.drag(divider_x2 + 2.0, divider_y, divider_x2 - 20.0, divider_y);

    let narrowed = driver.app().resolved_column_widths(driver.backend())[2];
    assert!(
        narrowed < widened,
        "dragging the divider before the last column left should narrow it: \
         before={widened}, after={narrowed}"
    );
}

// ─── DataTableApp: horizontally-scrolled click routing (#550) ──────────────
//
// `DataTableApp` sets `min_total_width = 80` cells, so a viewport
// narrower than that lays the columns out wider than the visible area
// and `h_scroll` becomes genuinely drivable (`L`/`H` step it by 5).
//
// Before #550 the renderer subtracted `h_scroll` when painting but
// `DataTableLayout::hit_test` did not add it back, so every click
// resolved against the *unscrolled* column geometry — the defect
// coord-tui had latent in its Audit table (claude-coordinator#1853),
// where a header click sorted whichever column happened to sit at that
// x before scrolling.

/// Which column title carries the sort indicator, read off the painted
/// header row. The status bar's `sort:` segment is the more direct
/// signal but does not fit in the narrow viewport these tests need to
/// force horizontal scrolling in the first place, so the header's own
/// `▲`/`▼` suffix is the observable.
fn sorted_header(screen: &str) -> Option<&'static str> {
    let header = screen.lines().next()?;
    ["Name", "Status", "Age", "Restarts"]
        .into_iter()
        .find(|title| {
            header.contains(&format!("{title} ▲")) || header.contains(&format!("{title} ▼"))
        })
}

/// The Audit regression: with the table scrolled horizontally, clicking
/// a painted header must sort *that* column — not the one that used to
/// occupy those cells at `h_scroll == 0`.
#[test]
fn data_table_h_scrolled_header_click_sorts_the_column_under_the_cursor() {
    let mut driver = TuiDriver::new(DataTableApp::new(), 50, 26);
    assert_eq!(
        sorted_header(&driver.screen()),
        Some("Name"),
        "app starts sorted by Name:\n{}",
        driver.screen()
    );

    // Scroll right far enough that "Status" no longer sits over the
    // cells "Name" occupied at rest.
    let unscrolled_status_x = driver
        .find("Status")
        .unwrap_or_else(|| panic!("Status header not painted:\n{}", driver.screen()))
        .0;
    for _ in 0..6 {
        driver.type_char('L');
    }
    let screen = driver.screen();
    let (status_x, status_y) = driver
        .find("Status")
        .unwrap_or_else(|| panic!("Status header not painted after scrolling:\n{screen}"));
    assert_eq!(
        status_y, 0.5,
        "Status should be on the header row:\n{screen}"
    );
    assert!(
        status_x < unscrolled_status_x - 1.0,
        "'L' should have scrolled the Status header left (was {unscrolled_status_x}, \
         now {status_x}):\n{screen}"
    );

    // Click a few cells into the title, clear of the preceding column's
    // ±3-cell divider grab zone.
    driver.click(status_x + 4.0, status_y);

    let after = driver.screen();
    assert_eq!(
        sorted_header(&after),
        Some("Status"),
        "clicking the painted 'Status' header while h-scrolled must sort Status, and must \
         not sort whichever column occupied those cells before scrolling (the \
         claude-coordinator#1853 mis-routing):\n{after}"
    );
}

/// `h_scroll == 0` routing is untouched: the same header click sorts the
/// same column it always did.
#[test]
fn data_table_unscrolled_header_click_still_sorts_that_column() {
    let mut driver = TuiDriver::new(DataTableApp::new(), 50, 26);
    let (x, y) = driver
        .find("Status")
        .unwrap_or_else(|| panic!("Status header not painted:\n{}", driver.screen()));
    driver.click(x + 4.0, y);
    let after = driver.screen();
    assert_eq!(
        sorted_header(&after),
        Some("Status"),
        "unscrolled header click must keep routing to the column under the cursor:\n{after}"
    );
}

/// A body click while h-scrolled still selects the row under the cursor
/// — the vertical axis is orthogonal to `h_scroll` and must not shift.
#[test]
fn data_table_h_scrolled_body_click_selects_the_row_under_the_cursor() {
    let mut driver = TuiDriver::new(DataTableApp::new(), 50, 26);
    for _ in 0..6 {
        driver.type_char('L');
    }
    let screen = driver.screen();
    // "Running" is a Status-column value; at this scroll offset it is
    // painted well left of where it sits at rest.
    let (x, y) = driver
        .find("Running")
        .unwrap_or_else(|| panic!("no Running row painted while scrolled:\n{screen}"));
    driver.click(x, y);
    let after = driver.screen();
    let expected_row = y as usize; // header occupies row 0, body starts at 1
    assert!(
        after.contains(&format!("row {expected_row} / 20")),
        "a body click while h-scrolled should select the row it landed on \
         (expected row {expected_row}):\n{after}"
    );
}

// ─── HelpLayerDemo (#431): context-sensitive help registry + cheatsheet ────
//
// `HelpLayerDemo` implements `ShellApp` with two panels ("Explorer" and
// "Source Control"), each registered with distinct notes + actions via
// `HelpRegistry`. `?` opens a cheatsheet (`HelpOverlayController`, built on
// `Panel` + `TextDisplay`) for the *active* panel; `p` opens a command
// palette (`DualModePaletteController`) populated from the same registered
// actions via `help_actions_to_palette_items` / `filter_help_actions`.
// These tests prove the acceptance bar from #431: registered help renders
// via `?`, the palette lists the same actions, and the palette query
// matches on description text as well as label.

/// `?` opens the cheatsheet for the default active panel (Explorer) and
/// renders its registered notes and actions — including the accelerator
/// column, which only the "Actions" section carries.
#[test]
fn help_layer_demo_question_mark_opens_cheatsheet_for_active_panel() {
    let config = HelpLayerDemo::config();
    let mut driver = driver_with_shell(HelpLayerDemo::new(), config, 100, 30);

    assert!(
        driver.screen_contains("?=help"),
        "initial hint should mention the ? trigger:\n{}",
        driver.screen()
    );

    let reaction = driver.type_char('?');
    assert_eq!(reaction, Reaction::Redraw, "? should open the cheatsheet");

    let screen = driver.screen();
    assert!(
        screen.contains("Help — Explorer"),
        "cheatsheet title should name the active panel:\n{screen}"
    );
    assert!(
        screen.contains("File has unsaved changes"),
        "cheatsheet should render the active panel's registered notes:\n{screen}"
    );
    assert!(
        screen.contains("New File") && screen.contains("Ctrl+N"),
        "cheatsheet should render the active panel's registered actions \
         with their accelerator:\n{screen}"
    );
}

/// `Escape` closes an open cheatsheet without quitting the demo, and the
/// entries it was showing disappear from the screen.
#[test]
fn help_layer_demo_escape_closes_cheatsheet() {
    let config = HelpLayerDemo::config();
    let mut driver = driver_with_shell(HelpLayerDemo::new(), config, 100, 30);

    driver.type_char('?');
    assert!(driver.screen_contains("Help — Explorer"));

    let reaction = driver.press_named(NamedKey::Escape);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Escape while the cheatsheet is open should close it, not quit"
    );
    assert!(!driver.exited(), "Escape must not quit the demo");
    assert!(
        !driver.screen_contains("New File"),
        "closing the cheatsheet should remove its content from the screen:\n{}",
        driver.screen()
    );
}

/// Switching the active panel (Tab → focus bar → j → Enter, the same
/// activity-bar keyboard flow proven by the `AppShellDemo` tests above)
/// changes which panel's help `?` shows next — the "context-sensitive"
/// half of #431.
#[test]
fn help_layer_demo_switching_panel_changes_cheatsheet_content() {
    let config = HelpLayerDemo::config();
    let mut driver = driver_with_shell(HelpLayerDemo::new(), config, 100, 30);

    driver.press_named(NamedKey::Tab);
    driver.type_char('j'); // explorer -> git
    let reaction = driver.press_named(NamedKey::Enter);
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.screen_contains("Panel: panel:git"),
        "activating the cursor item should switch to the git panel:\n{}",
        driver.screen()
    );

    driver.type_char('?');
    let screen = driver.screen();
    assert!(
        screen.contains("Help — Source Control"),
        "cheatsheet title should follow the newly active panel:\n{screen}"
    );
    assert!(
        screen.contains("Commit") && screen.contains("Ctrl+Enter"),
        "cheatsheet should show the git panel's own registered actions:\n{screen}"
    );
    assert!(
        !screen.contains("New File"),
        "cheatsheet must not leak the previous panel's actions:\n{screen}"
    );
}

/// `p` opens a command palette listing the active panel's registered
/// actions — same data the cheatsheet renders, fed through
/// `help_actions_to_palette_items`.
#[test]
fn help_layer_demo_p_opens_command_palette_with_registered_actions() {
    let config = HelpLayerDemo::config();
    let mut driver = driver_with_shell(HelpLayerDemo::new(), config, 100, 30);

    let reaction = driver.type_char('p');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "p should open the command palette"
    );

    let screen = driver.screen();
    assert!(
        screen.contains("Commands"),
        "palette should render its title:\n{screen}"
    );
    assert!(
        screen.contains("New File") && screen.contains("Ctrl+N"),
        "palette should list the registered action's label and accelerator:\n{screen}"
    );
    assert!(
        screen.contains("Reveal in Finder"),
        "palette should list every registered action for the active panel:\n{screen}"
    );
}

/// Typing a query that matches only an action's *description* (not its
/// label) still surfaces that action and filters out the rest — proving
/// the palette integration is searchable by description, not just label.
#[test]
fn help_layer_demo_palette_query_matches_description_not_label() {
    let config = HelpLayerDemo::config();
    let mut driver = driver_with_shell(HelpLayerDemo::new(), config, 100, 30);

    driver.type_char('p');
    assert!(driver.screen_contains("New File"));

    // "disk" appears only in "Reveal in Finder"'s description ("Show the
    // selected file on disk"), never in either action's label.
    for c in "disk".chars() {
        driver.type_char(c);
    }

    let screen = driver.screen();
    assert!(
        screen.contains("Reveal in Finder"),
        "query matching only the description should still surface that action:\n{screen}"
    );
    assert!(
        !screen.contains("New File"),
        "query matching neither the label nor description should filter the action out:\n{screen}"
    );
}

/// A panel with **no** registered help (the demo's "Settings" panel)
/// still lets `?` open a visible cheatsheet and `Escape` still closes
/// it — the fix for #431 review finding 1. Before the fix,
/// `HelpOverlayController::render` silently drew nothing when the
/// active view had no `ViewHelp`, while `handle()` still unconditionally
/// swallowed every subsequent key as if a modal cheatsheet were open —
/// an unrecoverable-looking freeze for any consumer that hasn't
/// registered help for every panel.
#[test]
fn help_layer_demo_no_registered_help_still_dismisses_via_escape() {
    let config = HelpLayerDemo::config();
    let mut driver = driver_with_shell(HelpLayerDemo::new(), config, 100, 30);

    // Explorer -> Source Control -> Settings (no help registered).
    driver.press_named(NamedKey::Tab);
    driver.type_char('j');
    driver.type_char('j');
    let reaction = driver.press_named(NamedKey::Enter);
    assert_eq!(reaction, Reaction::Redraw);
    assert!(
        driver.screen_contains("Panel: panel:settings"),
        "activating the cursor item twice should switch to the settings panel:\n{}",
        driver.screen()
    );

    let reaction = driver.type_char('?');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "? should still open the cheatsheet even with no registered help"
    );
    let screen = driver.screen();
    assert!(
        screen.contains("No help available"),
        "cheatsheet should show a visible fallback instead of rendering nothing:\n{screen}"
    );

    let reaction = driver.press_named(NamedKey::Escape);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Escape should close the cheatsheet even though no help was registered"
    );
    assert!(!driver.exited(), "Escape must not quit the demo");
    assert!(
        !driver.screen_contains("No help available"),
        "closing the cheatsheet should remove the fallback message:\n{}",
        driver.screen()
    );

    // And the app must not be left swallowing keys forever: a normal key
    // now falls through to the app instead of being consumed by a
    // still-open (but invisible) overlay.
    let reaction = driver.type_char('q');
    assert_eq!(
        reaction,
        Reaction::Exit,
        "q should quit normally once the overlay is closed"
    );
}

// ─── SplitApp (issue #307): draggable single divider ───────────────────────

/// Dragging `SplitApp`'s single divider must move it and shrink the pane
/// it moves towards — `SplitApp::handle`'s `MouseMoved` arm recomputes
/// `ratio` straight from the cursor's absolute x each move (clamped to
/// [0.05, 0.95]), and the status bar's `ratio: {:.0}%` segment mirrors
/// that field every frame. Tracking the divider glyph's own painted
/// column (rather than reading `SplitApp.ratio` directly, which has no
/// public accessor) is what makes this a real paint round-trip: a drag
/// handler that updated `ratio` but never affected `backend.draw_split`'s
/// layout — or vice versa — would fail this, not just a hand-inspected
/// field.
///
/// Clicks land on `find_bounds("│")`'s left edge, not its center like
/// most tests in this file: `Split::layout`'s divider hit-region for a
/// 0.5 ratio here is `[39.5, 40.5)`, but `tui/split.rs::draw_split` paints
/// the glyph at `div.x.round()` = cell 40 (`[40.0, 41.0)`) — the two
/// spans only overlap in `[40.0, 40.5)`. `find`'s usual cell-center
/// coordinate (`40.5`) sits exactly on that boundary and resolves to
/// `SecondPane`, not `Divider` — so a click there would silently miss the
/// divider it just found painted. The left edge (`40.0`) is inside both
/// spans regardless of the exact half-cell rounding, so it's the robust
/// choice here.
#[test]
fn split_dragging_divider_moves_it_and_updates_ratio() {
    let mut driver = TuiDriver::new(SplitApp::new(), 80, 24);
    assert!(
        driver.screen_contains("ratio: 50% (H)"),
        "starts centered 50/50:\n{}",
        driver.screen()
    );

    let before = driver
        .find_bounds("│")
        .unwrap_or_else(|| panic!("divider not painted:\n{}", driver.screen()));
    let (dx, dy) = (before.x, before.y + before.height / 2.0);

    driver.drag(dx, dy, dx - 20.0, dy);

    let after = driver.find_bounds("│").unwrap_or_else(|| {
        panic!(
            "divider should still be painted after drag:\n{}",
            driver.screen()
        )
    });
    assert!(
        after.x < before.x - 15.0,
        "dragging the divider 20 cols left should move it left by roughly that much \
         (shrinking the first pane's width by the same amount): before={}, after={}",
        before.x,
        after.x
    );
    assert!(
        !driver.screen_contains("ratio: 50% (H)"),
        "status bar ratio should move away from 50% after the drag:\n{}",
        driver.screen()
    );
}

// ─── SplitTreeApp: N-way nested split via DragTarget::SplitDivider ─────────
//
// Tree shape: Split(Horizontal, Split(Vertical, a, b), c) — split_index 0
// is the root (outer, side-by-side) divider painted as `│`; split_index 1
// is the nested (stacked) divider painted as `─`, only inside the left
// column. Exercises issue #435's DragTarget::SplitDivider end to end:
// hit-test -> DragState::begin -> dispatch_mouse_drag ->
// UiEvent::SplitDividerDragged -> SplitTree::set_ratio_at_index.

#[test]
fn split_tree_initial_screen_paints_all_leaves_and_status() {
    let driver = TuiDriver::new(SplitTreeApp::new(), 80, 24);
    let screen = driver.screen();
    assert!(
        screen.contains(" a "),
        "leaf a should be painted:\n{screen}"
    );
    assert!(
        screen.contains(" b "),
        "leaf b should be painted:\n{screen}"
    );
    assert!(
        screen.contains(" c "),
        "leaf c should be painted:\n{screen}"
    );
    assert!(
        screen.contains("3 leaves, 2 dividers"),
        "status bar should report the tree shape:\n{screen}"
    );
    assert_eq!(driver.app().tree().ratio_at_index(0), Some(0.5));
    assert_eq!(driver.app().tree().ratio_at_index(1), Some(0.5));
}

#[test]
fn split_tree_dragging_outer_divider_updates_root_ratio_only() {
    let mut driver = TuiDriver::new(SplitTreeApp::new(), 80, 24);

    // The root split is Horizontal (side-by-side) -> painted as a
    // vertical `│` line. It's the only Horizontal split in this tree,
    // so it's the only `│` on screen.
    let (dx, dy) = driver
        .find("│")
        .unwrap_or_else(|| panic!("outer divider not painted:\n{}", driver.screen()));

    driver.drag(dx, dy, dx - 20.0, dy);

    let root_ratio = driver
        .app()
        .tree()
        .ratio_at_index(0)
        .expect("root split still present after drag");
    assert!(
        root_ratio < 0.4,
        "dragging the outer divider 20 cols left should shrink ratio_at_index(0) well below 0.5, got {root_ratio}"
    );
    assert_eq!(
        driver.app().tree().ratio_at_index(1),
        Some(0.5),
        "dragging the outer divider must not touch the nested split's ratio"
    );
}

#[test]
fn split_tree_dragging_inner_divider_updates_nested_ratio_only() {
    let mut driver = TuiDriver::new(SplitTreeApp::new(), 80, 24);

    // The nested split is Vertical (stacked) -> painted as a
    // horizontal `─` line, only within the left column.
    let (dx, dy) = driver
        .find("─")
        .unwrap_or_else(|| panic!("inner divider not painted:\n{}", driver.screen()));

    driver.drag(dx, dy, dx, dy - 5.0);

    let nested_ratio = driver
        .app()
        .tree()
        .ratio_at_index(1)
        .expect("nested split still present after drag");
    assert!(
        nested_ratio < 0.4,
        "dragging the inner divider up should shrink ratio_at_index(1) well below 0.5, got {nested_ratio}"
    );
    assert_eq!(
        driver.app().tree().ratio_at_index(0),
        Some(0.5),
        "dragging the inner divider must not touch the root split's ratio"
    );
}

#[test]
fn split_tree_r_resets_ratios_after_drag() {
    let mut driver = TuiDriver::new(SplitTreeApp::new(), 80, 24);
    let (dx, dy) = driver
        .find("│")
        .unwrap_or_else(|| panic!("outer divider not painted:\n{}", driver.screen()));
    driver.drag(dx, dy, dx - 20.0, dy);
    assert_ne!(driver.app().tree().ratio_at_index(0), Some(0.5));

    let reaction = driver.type_char('r');
    assert_eq!(reaction, Reaction::Redraw, "r should reset and redraw");
    assert_eq!(driver.app().tree().ratio_at_index(0), Some(0.5));
    assert_eq!(driver.app().tree().ratio_at_index(1), Some(0.5));
}

#[test]
fn split_tree_q_exits() {
    let mut driver = TuiDriver::new(SplitTreeApp::new(), 80, 24);
    let reaction = driver.type_char('q');
    assert_eq!(reaction, Reaction::Exit);
    assert!(driver.exited());
}

// ─── ModalOcclusionDemo (issue #455 / #491): modal click-through guard ─────
//
// `ModalOcclusionDemo` is the fixture behind
// `tests/conformance/scenarios/modal/dialog.blocks_click_through.scn.json`
// (quadraui#491). It has no `if dialog_open` guard anywhere in its own
// `handle` — occlusion is entirely quadraui's `ModalStack` + `dispatch_click`
// tagging a `MouseDown` that lands inside the topmost modal's bounds with
// that modal's `WidgetId`. These tests exercise the same behaviour the
// conformance scenario asserts, but through the driver directly (CLAUDE.md's
// per-example Tier-1 obligation, independent of the Tier-4 conformance
// suite).

/// Initial paint: rows, the "Open dialog" row, and the starting status bar
/// text are all painted; no dialog is visible yet.
#[test]
fn modal_occlusion_initial_screen_paints_rows_and_open_button() {
    let driver = TuiDriver::new(ModalOcclusionDemo::new(), 100, 30);
    let screen = driver.screen();
    assert!(
        driver.screen_contains("pod-00"),
        "first row should be painted:\n{screen}"
    );
    assert!(
        driver.screen_contains("Open dialog"),
        "'Open dialog' row should be painted:\n{screen}"
    );
    assert!(
        driver.screen_contains("selected: nothing"),
        "status bar should start with no selection:\n{screen}"
    );
    assert!(
        !driver.screen_contains("Confirm delete"),
        "dialog should not be open initially:\n{screen}"
    );
}

/// Clicking a row (no dialog open) selects it — the base-layer click path.
#[test]
fn modal_occlusion_row_click_selects_row() {
    let mut driver = TuiDriver::new(ModalOcclusionDemo::new(), 100, 30);
    let (x, y) = driver
        .find("pod-03")
        .unwrap_or_else(|| panic!("'pod-03' row should be painted:\n{}", driver.screen()));
    driver.click(x, y);
    assert!(
        driver.screen_contains("selected: pod-03"),
        "clicking a row should select it:\n{}",
        driver.screen()
    );
}

/// Clicking "Open dialog" opens the centred confirm dialog.
#[test]
fn modal_occlusion_open_button_opens_dialog() {
    let mut driver = TuiDriver::new(ModalOcclusionDemo::new(), 100, 30);
    let (x, y) = driver
        .find("Open dialog")
        .unwrap_or_else(|| panic!("'Open dialog' row should be painted:\n{}", driver.screen()));
    driver.click(x, y);
    assert!(
        driver.screen_contains("Confirm delete"),
        "clicking 'Open dialog' should open the dialog:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("Really delete?"),
        "dialog body text should be painted:\n{}",
        driver.screen()
    );
}

/// The #455 regression, made executable: clicking the dialog's own body
/// text — which sits directly over a row in the middle of the list — must
/// be tagged with the dialog's id and must NOT reach the row underneath. If
/// a backend forgot to route `MouseDown` through `ModalStack` first, this
/// click would arrive as `widget: None` and select the covered row instead.
#[test]
fn modal_occlusion_dialog_body_click_does_not_fall_through() {
    let mut driver = TuiDriver::new(ModalOcclusionDemo::new(), 100, 30);

    // Select a row first so a fall-through would be visible as a change.
    let (rx, ry) = driver
        .find("pod-03")
        .unwrap_or_else(|| panic!("'pod-03' row should be painted:\n{}", driver.screen()));
    driver.click(rx, ry);
    assert!(driver.screen_contains("selected: pod-03"));

    let (ox, oy) = driver
        .find("Open dialog")
        .unwrap_or_else(|| panic!("'Open dialog' row should be painted:\n{}", driver.screen()));
    driver.click(ox, oy);
    assert!(driver.screen_contains("Confirm delete"));

    let (bx, by) = driver
        .find("Really delete?")
        .unwrap_or_else(|| panic!("dialog body text should be painted:\n{}", driver.screen()));
    driver.click(bx, by);

    assert!(
        driver.screen_contains("selected: pod-03"),
        "clicking the dialog's own body text must not change the selection \
         underneath it:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("Confirm delete"),
        "clicking the dialog body (not a button) must not close it:\n{}",
        driver.screen()
    );
}

/// Clicking "Cancel" closes the dialog; the base layer is live again
/// afterwards (proves the earlier block was the modal, not a dead row
/// list).
#[test]
fn modal_occlusion_cancel_closes_dialog_and_base_layer_recovers() {
    let mut driver = TuiDriver::new(ModalOcclusionDemo::new(), 100, 30);

    let (ox, oy) = driver
        .find("Open dialog")
        .unwrap_or_else(|| panic!("'Open dialog' row should be painted:\n{}", driver.screen()));
    driver.click(ox, oy);
    assert!(driver.screen_contains("Confirm delete"));

    let (cx, cy) = driver
        .find("Cancel")
        .unwrap_or_else(|| panic!("'Cancel' button should be painted:\n{}", driver.screen()));
    driver.click(cx, cy);
    assert!(
        !driver.screen_contains("Confirm delete"),
        "clicking Cancel should close the dialog:\n{}",
        driver.screen()
    );

    let (rx, ry) = driver
        .find("pod-12")
        .unwrap_or_else(|| panic!("'pod-12' row should be painted:\n{}", driver.screen()));
    driver.click(rx, ry);
    assert!(
        driver.screen_contains("selected: pod-12"),
        "with the dialog closed, row clicks should reach the base layer again:\n{}",
        driver.screen()
    );
}

/// Esc closes an open dialog without exiting the app; Esc again (no dialog
/// open) exits.
#[test]
fn modal_occlusion_esc_closes_dialog_then_exits() {
    let mut driver = TuiDriver::new(ModalOcclusionDemo::new(), 100, 30);

    let (ox, oy) = driver
        .find("Open dialog")
        .unwrap_or_else(|| panic!("'Open dialog' row should be painted:\n{}", driver.screen()));
    driver.click(ox, oy);
    assert!(driver.screen_contains("Confirm delete"));

    let reaction = driver.press_named(NamedKey::Escape);
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Esc should close the dialog, not exit"
    );
    assert!(!driver.exited());
    assert!(!driver.screen_contains("Confirm delete"));

    let reaction = driver.press_named(NamedKey::Escape);
    assert_eq!(
        reaction,
        Reaction::Exit,
        "Esc with no dialog open should exit"
    );
    assert!(driver.exited());
}

/// `q` exits regardless of dialog state.
#[test]
fn modal_occlusion_q_exits() {
    let mut driver = TuiDriver::new(ModalOcclusionDemo::new(), 100, 30);
    let reaction = driver.type_char('q');
    assert_eq!(reaction, Reaction::Exit);
    assert!(driver.exited());
}

// ─── ChatDemo (issue #305): ChatController submit round-trip ────────────────
//
// `ChatDemo` wraps a `ChatController`; typing goes through
// `ChatController::handle`'s `Key::Char(c) if !ctrl && !alt` arm (character
// insertion), and submitting requires `Ctrl+S` (or Alt/Ctrl+Enter — plain
// Enter only inserts a newline). A missed `ChatDemo::sync_controller()` call
// or a dropped `Reaction::Redraw` on `ChatControllerEvent::Submit` would
// leave the typed text stuck in the input box and never reach the
// transcript — exactly what this test would catch.

#[test]
fn chat_typing_and_ctrl_s_submits_message_into_transcript() {
    let mut driver = TuiDriver::new(ChatDemo::new(), 100, 30);

    for c in "hello world".chars() {
        driver.type_char(c);
    }
    assert!(
        driver.screen_contains("hello world"),
        "typed text should render in the input box before submit:\n{}",
        driver.screen()
    );

    let reaction = driver.ctrl_char('s');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "Ctrl+S submit should redraw the transcript"
    );

    let screen = driver.screen();
    assert!(
        screen.contains("You"),
        "a submitted user turn should paint the 'You' role header:\n{screen}"
    );
    assert!(
        screen.contains("hello world"),
        "the submitted message text should appear in the transcript:\n{screen}"
    );
}

// ─── SidebarPanelApp (issue #305): sidebar item click updates main panel ────
//
// Clicking a task row is handled by `SidebarPanelApp::handle`'s `MouseDown`
// arm via `SidebarPanelHit::Content` — a different hit-test branch than the
// toolbar buttons above it. A handler that routed content clicks to the
// toolbar dispatch (or vice versa) would silently no-op here.

#[test]
fn sidebar_panel_click_task_row_selects_it_and_updates_status() {
    let mut driver = TuiDriver::new(SidebarPanelApp::new(), 100, 24);
    let before = driver.screen();

    let (x, y) = driver
        .find("Write toolbar tests")
        .unwrap_or_else(|| panic!("task row not painted:\n{before}"));
    driver.click(x, y);

    let after = driver.screen();
    assert!(
        after.contains("Selected: Write toolbar tests"),
        "clicking a sidebar task row should select it and update the status bar:\n{after}"
    );
}

// ─── FormGroupsApp (issue #305): ToggleGroup click flips a rendered field ───
//
// The issue's suggested scenario for this demo is "type into a field", but
// `FormGroupsApp::handle` has no keyboard text-entry arm at all for its
// `TextInput` fields (only `q`/Esc/Tab/Shift+Tab are bound — see the module
// doc comment listing "Controls"); typing into "Find"/"Replace" is simply
// not wired in this example. The demo's actual interactive surface for its
// form fields is mouse clicks on `ToggleGroup` items and `ButtonRow`
// buttons, routed through `FormGroupsApp::click` → `FormHit::Field`. This
// test exercises that real path instead: clicking the "Aa" (case-sensitive)
// toggle must flip `case_sensitive` and the status bar must reflect the new
// rendered value — a mis-routed `FormHit` or a stale `build_form()` snapshot
// would leave the status bar unchanged.

#[test]
fn form_groups_click_toggle_flips_rendered_value() {
    let mut driver = TuiDriver::new(FormGroupsApp::new(), 100, 20);
    let before = driver.screen();

    let (x, y) = driver
        .find("Aa")
        .unwrap_or_else(|| panic!("case toggle not painted:\n{before}"));
    driver.click(x, y);

    let after = driver.screen();
    assert!(
        after.contains("last: case=false"),
        "clicking the case-sensitive toggle (starts true) should flip it and show \
         case=false in the status bar:\n{after}"
    );
}

// ─── ChartApp (issue #308): state-derived axis labels + gridlines ─────────
//
// `ChartApp::new()` starts on the sparkline view, whose `Chart` has no
// `x_label`/`y_label`/`show_grid` — so the axis-label/gridline codepath in
// `tui::chart::paint_axis_labels` never runs on the first frame. Pressing
// '2' switches to the line-chart view, whose `Chart` sets
// `x_label: Some("Time (s)")`, `y_label: Some("Usage")`, `y_ticks:
// Some(5)`, and `show_grid: true` — this is the real "switch to a chart
// with known data, assert axis labels and gridlines render" scenario named
// in the issue. A dropped `paint_axis_labels` call or a `show_grid` check
// that never fires would leave the axis text and the '┄' gridline glyph
// off the screen even though the chart itself repaints.

#[test]
fn chart_switching_to_line_view_paints_axis_labels_and_gridlines() {
    let mut driver = TuiDriver::new(ChartApp::new(), 100, 30);
    let before = driver.screen();
    assert!(
        !before.contains("Time (s)") && !before.contains("Usage"),
        "sparkline view has no axis labels configured, so none should paint:\n{before}"
    );

    driver.type_char('2');

    let after = driver.screen();
    assert!(
        after.contains("Time (s)"),
        "line chart's x_label should paint below the plot area:\n{after}"
    );
    // `paint_axis_labels` only paints the y_label up to `col < px` (the
    // plot area's left edge, reserved for tick-value labels) — for this
    // chart's y-axis gutter that clips "Usage" to its first 4 chars. That
    // clip is real, deterministic layout behavior (see
    // `tui/chart.rs::paint_axis_labels`), not a rendering bug this test
    // should paper over.
    assert!(
        after.contains("Usag"),
        "line chart's y_label should paint left of the plot area:\n{after}"
    );
    // State-derived gridline positions: y_range (0.0, 100.0) with
    // y_ticks: Some(5) produces tick values at 20/40/60/80 (the interior
    // ticks; 0 and 100 land on the plot area's edges and are excluded by
    // the `row > pa.y && row < pa.y + pa.height` interior check).
    for tick in ["80", "60", "40", "20"] {
        assert!(
            after.contains(tick),
            "y-axis tick label {tick:?} should paint at its data-derived row:\n{after}"
        );
    }
    assert!(
        after.contains('┄'),
        "line chart sets show_grid: true with y_ticks: Some(5), so horizontal \
         gridline glyphs should paint across the plot area at each tick row:\n{after}"
    );
}

// ─── IndicatorsApp (issue #308): toggling cancellable paints/clears '×' ────
//
// `IndicatorsApp::handle`'s `'c'` arm flips `self.cancellable`, which
// `tui::progress::draw_progress` uses to decide whether to reserve a
// 3-cell cancel affordance and paint the '×' glyph into it. This is the
// "toggle indicator state, assert symbol change" scenario named in the
// issue — a dropped `cancellable` flag (or a cancel-bounds calc that never
// paints the glyph) would leave the bar looking identical before and after
// the toggle.

#[test]
fn indicators_toggling_cancellable_paints_and_clears_cancel_symbol() {
    let mut driver = TuiDriver::new(IndicatorsApp::new(), 100, 20);
    let before = driver.screen();
    assert!(
        !before.contains('×'),
        "cancellable starts false, so no cancel glyph should paint yet:\n{before}"
    );

    driver.type_char('c');
    let toggled_on = driver.screen();
    assert!(
        toggled_on.contains('×'),
        "toggling 'c' should enable the cancel affordance and paint '×':\n{toggled_on}"
    );
    assert!(
        toggled_on.contains("Cancel enabled"),
        "status bar should report the new state:\n{toggled_on}"
    );

    driver.type_char('c');
    let toggled_off = driver.screen();
    assert!(
        !toggled_off.contains('×'),
        "toggling 'c' again should disable the cancel affordance and clear '×':\n{toggled_off}"
    );
}

// ─── MarkdownDemo (issue #308): headings + fenced code render correctly ───
//
// `MarkdownDemo::render` pushes a fixed markdown document through
// `render_markdown_to_styled` into a `RichTextPopup`. The adapter strips
// heading `#` markers down to plain text, renders the fenced code block's
// opening fence as a dim `lang` header (the ` ``` ` markers themselves are
// never shown), and prefixes code content lines with the `┃` rail glyph
// (see `compose::markdown`'s doc table). This test pins that contract at
// the whole-example level: a regression in the adapter or a dropped
// `render_markdown_to_styled` call in `MarkdownDemo::render` would surface
// here even though the primitive-level adapter tests still pass.

#[test]
fn markdown_demo_renders_headings_and_fenced_code_block() {
    let driver = TuiDriver::new(MarkdownDemo::new(), 100, 30);
    let screen = driver.screen();

    // Headings: '#' markers stripped, text kept.
    assert!(
        screen.contains("Markdown adapter demo"),
        "H1 text should render:\n{screen}"
    );
    assert!(
        screen.contains("Headings scale up"),
        "H2 text should render:\n{screen}"
    );
    assert!(
        screen.contains("And down again"),
        "H3 text should render:\n{screen}"
    );

    // Fenced code block: language header + code-rail-prefixed content,
    // fence markers themselves never shown.
    assert!(
        screen.contains("rust"),
        "opening fence's language tag should render as a header:\n{screen}"
    );
    assert!(
        screen.contains('┃'),
        "code content lines should carry the code-rail glyph:\n{screen}"
    );
    assert!(
        screen.contains("fn main"),
        "code block content should render verbatim:\n{screen}"
    );
    assert!(
        !screen.contains("```"),
        "fence delimiters themselves must never appear in the rendered output:\n{screen}"
    );
}

// ─── ToastApp (issue #308): triggering + dismissing a toast ────────────────
//
// The issue's suggested scenario is "trigger a toast, assert it appears;
// advance time, assert it disappears" — but `ToastApp` (and the
// `ToastStack` primitive it drives) has no time-based auto-dismiss wired
// in; `primitives::toast`'s own doc comment says lifecycle/auto-dismiss is
// an app concern this demo simply doesn't implement. Its real dismiss path
// is a click on the toast's '×' affordance, routed through
// `ToastHit::Dismiss` in `ToastApp::handle`'s `MouseDown` arm. This test
// exercises that real appear/disappear round-trip instead: `ToastApp::new`
// seeds exactly one toast ("Welcome"), so its '×' is the only dismiss
// glyph on screen — unambiguous to `find`. Clicking it makes the toast
// disappear; pressing '2' afterward triggers a fresh Success toast that
// appears. A mis-routed `ToastHit` or a stale `self.toasts` retain would
// leave the dismissed toast on screen; a dropped `Reaction::Redraw` or a
// broken `add_toast` would leave the new one off it.

#[test]
fn toast_dismiss_click_removes_it_then_trigger_adds_a_new_one() {
    let mut driver = TuiDriver::new(ToastApp::new(), 100, 24);
    let before = driver.screen();
    assert!(
        before.contains("Welcome"),
        "ToastApp::new seeds a Welcome toast:\n{before}"
    );

    let (x, y) = driver
        .find("×")
        .unwrap_or_else(|| panic!("no dismiss glyph painted for the seeded toast:\n{before}"));
    driver.click(x, y);

    let after_dismiss = driver.screen();
    assert!(
        !after_dismiss.contains("Welcome"),
        "clicking the dismiss glyph should remove the toast from the stack:\n{after_dismiss}"
    );

    driver.type_char('2');
    let after_add = driver.screen();
    assert!(
        after_add.contains("Success notification"),
        "pressing '2' should trigger and paint a new Success toast:\n{after_add}"
    );
}

// ─── AiTranscript (issue #308): appended turn renders below prior turns ────
//
// `AiTranscript::send_next` (invoked on `ChatControllerEvent::Submit`)
// pushes a user turn followed by a markdown-rendered assistant turn onto
// the `ChatController`'s transcript, on top of the seeded system greeting.
// This is the "append a turn, assert it renders below prior turns"
// scenario named in the issue: the seeded greeting must still be present
// (prior turns aren't clobbered) and the new turns' text must appear after
// submitting — a dropped `Reaction::Redraw` on `Submit` or a
// `push_turn`/`push_turn_markdown` call that overwrote instead of
// appended would leave this failing.
//
// `ChatController::handle_key`'s Ctrl+S arm is a no-op on an empty
// `input_buf` (`ctrl_s_on_empty_input_ignored`, chat_controller.rs), even
// though `AiTranscript::send_next` ignores the *typed* text and always
// sends the next canned `EXCHANGES` entry — so a character has to be
// typed first to make the real submit gesture fire, exactly as a live
// user would.

#[test]
fn ai_transcript_submit_appends_turn_below_prior_turns() {
    let mut driver = TuiDriver::new(AiTranscript::new(), 100, 30);
    let before = driver.screen();
    assert!(
        before.contains("Connected."),
        "seeded system greeting should paint on the first frame:\n{before}"
    );

    driver.type_char('x');
    let reaction = driver.ctrl_char('s');
    assert_eq!(
        reaction,
        Reaction::Redraw,
        "submitting the canned prompt should redraw the transcript"
    );

    let after = driver.screen();
    assert!(
        after.contains("Connected."),
        "the prior system turn should still be present, not overwritten:\n{after}"
    );
    assert!(
        after.contains("How do I convert markdown to styled text?"),
        "the newly submitted user turn should render:\n{after}"
    );
    assert!(
        after.contains("Markdown"),
        "the assistant's markdown-rendered reply heading should render below it:\n{after}"
    );
}
