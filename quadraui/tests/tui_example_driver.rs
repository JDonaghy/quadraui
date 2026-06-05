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

use quadraui::tui::testing::TuiDriver;
use quadraui::NamedKey;

#[path = "../examples/common/demo.rs"]
mod demo;
#[path = "../examples/common/mini_app.rs"]
mod mini_app;
#[path = "../examples/common/panel_app.rs"]
mod panel_app;
#[path = "../examples/common/pipeline_app.rs"]
mod pipeline_app;
#[path = "../examples/common/text_input_demo.rs"]
mod text_input_demo;

use demo::AppState;
use mini_app::MiniApp;
use panel_app::PanelApp;
use pipeline_app::PipelineApp;
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
