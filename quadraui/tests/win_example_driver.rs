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
use quadraui::{NamedKey, Reaction};

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;
use appshell_demo::AppShellDemo;

#[path = "../examples/common/panel_app.rs"]
mod panel_app;
use panel_app::PanelApp;

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
