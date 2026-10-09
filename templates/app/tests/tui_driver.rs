//! End-to-end test for `App`, driven in-process by `quadraui`'s
//! `TuiDriver` — no terminal or display required. Mirrors the pattern
//! quadraui's own examples use in `quadraui/tests/tui_example_driver.rs`:
//! script real `UiEvent`s through the same `ShellApp` this app's TUI
//! binary runs, and assert on the rendered screen.
#![cfg(feature = "tui")]

// Aliased, and kept in its own `use` group (blank line below) — rustfmt
// sorts imports alphabetically *within* a group but preserves group order
// as written, so this stays first regardless of whether the generated
// crate name alphabetically precedes or follows `quadraui`.
use {{crate_name}} as app;

use quadraui::tui::testing::{driver_with_shell, TuiDriver};
use quadraui::{AppLogic, Reaction};

fn app_driver(width: u16, height: u16) -> TuiDriver<impl AppLogic> {
    driver_with_shell(app::App::new(), app::App::config(), width, height)
}

#[test]
fn renders_the_status_bar_on_the_first_frame() {
    let driver = app_driver(80, 10);
    let screen = driver.screen();
    assert!(
        driver.screen_contains("keys: 0"),
        "counter should start at 0:\n{screen}"
    );
    assert!(
        driver.screen_contains("q to quit"),
        "quit hint should render on the first frame:\n{screen}"
    );
}

#[test]
fn counts_keystrokes() {
    let mut driver = app_driver(80, 10);
    driver.type_char('a');
    driver.type_char('b');
    assert!(
        driver.screen_contains("keys: 2"),
        "counter should read 2 after two keystrokes:\n{}",
        driver.screen()
    );
}

#[test]
fn q_exits() {
    let mut driver = app_driver(80, 10);
    let reaction = driver.type_char('q');
    assert_eq!(reaction, Reaction::Exit, "'q' should exit");
    assert!(driver.exited());
}
