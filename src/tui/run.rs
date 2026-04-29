//! TUI runner — drives a [`crate::AppLogic`] implementation against
//! [`TuiBackend`].
//!
//! The runner absorbs every per-app-but-not-app-logic boilerplate
//! piece:
//! - Terminal raw-mode + alternate screen + mouse + bracketed-paste
//!   setup / teardown.
//! - Best-effort kitty keyboard-protocol push (REPORT_ALL_KEYS_AS_ESCAPE_CODES
//!   so Ctrl+Shift+L is unambiguous from Ctrl+L).
//! - `Terminal::new` + `TuiBackend` construction.
//! - Frame loop: `terminal.draw(|f| backend.enter_frame_scope(f, |b|
//!   app.render(b)))`.
//! - Event drain via [`crate::Backend::wait_events`].
//! - [`Reaction`] dispatch (Continue / Redraw / Exit).
//!
//! The app implements [`crate::AppLogic`] and calls
//! [`run`] with its instance. See `examples/tui_app.rs` for an
//! end-to-end usage.

use std::io;
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use crate::backend::Backend;
use crate::runner::{AppLogic, Reaction};
use crate::tui::backend::TuiBackend;

/// Default poll timeout — 16 ms ≈ 60 fps. The runner sleeps inside
/// `wait_events(timeout)` waiting for input; on timeout the loop
/// continues which gives the app a chance to redraw if its state
/// advanced asynchronously.
const POLL_TIMEOUT: Duration = Duration::from_millis(16);

/// Drive `app` to completion in a TUI environment.
///
/// Returns `Ok(())` on graceful exit (the app returned
/// [`Reaction::Exit`] from its `handle` method), or an
/// [`io::Error`] from terminal setup / tear-down. Panics inside the
/// app propagate after the runner restores the terminal so the user
/// doesn't end up with a broken terminal state.
///
/// # Single-frame contract
///
/// The runner ships with a single-frame model: one `terminal.draw`
/// call per redraw, one `app.render(backend)` invocation inside it.
/// Apps with multiple independently-drawn surfaces (vimcode's
/// per-DrawingArea GTK model) are out of scope today; the
/// single-frame model covers most TUI apps cleanly.
pub fn run<A: AppLogic>(mut app: A) -> io::Result<()> {
    use ratatui::crossterm::event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    };

    // ── Terminal setup ──────────────────────────────────────────
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;

    // Best-effort kitty keyboard enhancement push. Apps that
    // override this can call the crossterm functions before
    // `run()` and the runner won't double-push.
    let kbd_enhanced = push_keyboard_enhancement(&mut stdout);

    let crossterm_backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(crossterm_backend)?;
    terminal.clear()?;

    let mut backend = TuiBackend::new();

    // Run the app inside `catch_unwind` so a panic in app code
    // doesn't leave the terminal in a broken state.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_inner(&mut terminal, &mut backend, &mut app)
    }));

    // ── Terminal tear-down (always) ─────────────────────────────
    if kbd_enhanced {
        let _ = pop_keyboard_enhancement(terminal.backend_mut());
    }
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();

    match result {
        Ok(io_result) => io_result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn run_inner<A: AppLogic>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    backend: &mut TuiBackend,
    app: &mut A,
) -> io::Result<()> {
    // ── App setup hook ─────────────────────────────────────────
    app.setup(backend);

    // ── Frame loop ─────────────────────────────────────────────
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            let size = terminal.size()?;
            backend.begin_frame(crate::Viewport::new(
                size.width as f32,
                size.height as f32,
                1.0,
            ));
            terminal.draw(|frame| {
                backend.enter_frame_scope(frame, |b| {
                    app.render(b);
                });
            })?;
            backend.end_frame();
            needs_redraw = false;
        }

        // Drain events. `wait_events` blocks for up to POLL_TIMEOUT.
        let events = backend.wait_events(POLL_TIMEOUT);
        for event in events {
            match app.handle(event, backend) {
                Reaction::Continue => {}
                Reaction::Redraw => needs_redraw = true,
                Reaction::Exit => return Ok(()),
            }
        }
    }
}

/// Push kitty keyboard protocol flags (best-effort). Returns whether
/// the push succeeded; the caller pops on exit only if so.
fn push_keyboard_enhancement(stdout: &mut io::Stdout) -> bool {
    use ratatui::crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    use ratatui::crossterm::terminal::supports_keyboard_enhancement;
    if !supports_keyboard_enhancement().unwrap_or(false) {
        return false;
    }
    execute!(
        stdout,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        )
    )
    .is_ok()
}

fn pop_keyboard_enhancement(backend: &mut CrosstermBackend<io::Stdout>) -> io::Result<()> {
    use ratatui::crossterm::event::PopKeyboardEnhancementFlags;
    execute!(backend, PopKeyboardEnhancementFlags)
}
