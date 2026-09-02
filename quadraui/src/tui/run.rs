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
//! - Frame loop: [`render_frame`] (`terminal.draw(|f|
//!   backend.enter_frame_scope(f, |b| app.render(b)))`).
//! - Event drain via [`crate::Backend::wait_events`], dispatched through
//!   [`dispatch_event`].
//! - [`Reaction`] dispatch (Continue / Redraw / Exit).
//!
//! The app implements [`crate::AppLogic`] and calls
//! [`run`] with its instance. See `examples/tui_app.rs` for an
//! end-to-end usage.
//!
//! ## Shared with the headless test driver
//!
//! [`render_frame`] and [`dispatch_event`] are `pub(crate)` so the
//! in-process [`crate::tui::testing::TuiDriver`] renders + dispatches
//! through the *exact same* code as the live runner. The driver swaps
//! `CrosstermBackend` for ratatui's `TestBackend` and supplies scripted
//! events instead of polling crossterm — but the frame paint and the
//! event pre-processing (text selection, Ctrl-C copy) cannot drift,
//! because there is only one implementation of each.

use std::io;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use crate::backend::Backend;
use crate::runner::{AppLogic, Reaction};
use crate::runtime::{ResizeDebouncer, RESIZE_SETTLE};
// Re-exported (not just imported) so `tui::testing` — and any other
// in-crate caller that historically reached `EventOutcome` through this
// module — keeps working unchanged after the type moved to
// `crate::runtime` (quadraui#496).
pub(crate) use crate::runtime::EventOutcome;
use crate::tui::backend::TuiBackend;
use crate::{Key, UiEvent};

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
    // ── Seed the viewport from the real terminal size BEFORE setup ──
    //
    // `TuiBackend::new()` seeds `Viewport::default()` (80×24). If
    // `app.setup()` reads `backend.viewport()` to size a side effect
    // — e.g. `TerminalApp` spawns its PTY at the viewport's cell
    // dimensions — it would otherwise always get 80×24 and only snap to
    // the real size on the first `WindowResized` event, leaving the pane
    // under-filled until the user's first interaction (quadraui#437, the
    // TUI counterpart of the original tiny-window bug). Sync the real
    // size up front so `setup()` sees true dimensions.
    let size = terminal.size()?;
    backend.begin_frame(crate::Viewport::new(
        size.width as f32,
        size.height as f32,
        1.0,
    ));

    // ── App setup hook ─────────────────────────────────────────
    app.setup(backend);

    // ── Frame loop ─────────────────────────────────────────────
    let mut needs_redraw = true;
    // Trailing-edge resize debounce state (quadraui#437, shared utility
    // extracted in #496 — see `crate::runtime::ResizeDebouncer`). We hold
    // the most recent viewport from a burst of `WindowResized` events in
    // `resize_debouncer` and dispatch a single settled resize once
    // `RESIZE_SETTLE` elapses with no newer one — the deadline itself is
    // TUI's own `Instant`-poll mechanism (see `ResizeDebouncer`'s doc for
    // why the timing stays per-backend).
    let mut resize_debouncer = ResizeDebouncer::new();
    let mut resize_deadline: Option<Instant> = None;
    loop {
        if needs_redraw {
            render_frame(terminal, backend, app)?;
            needs_redraw = false;
        }

        // Drain events. `wait_events` blocks for up to POLL_TIMEOUT.
        let events = backend.wait_events(POLL_TIMEOUT);
        for event in events {
            // Debounce PTY-thrashing resize storms: coalesce the burst to
            // the latest size and defer dispatch until the drag settles.
            // Painting stays live because `render_frame` re-reads the real
            // terminal size every frame.
            if let UiEvent::WindowResized { viewport } = event {
                resize_debouncer.note(viewport);
                resize_deadline = Some(Instant::now() + RESIZE_SETTLE);
                needs_redraw = true;
                continue;
            }
            match dispatch_event(event, backend, app) {
                EventOutcome::Continue => {}
                EventOutcome::Redraw => needs_redraw = true,
                EventOutcome::Exit => return Ok(()),
            }
        }

        // Fire the debounced resize once the drag has settled.
        if resize_deadline.is_some_and(|d| Instant::now() >= d) {
            resize_deadline = None;
            if let Some(viewport) = resize_debouncer.take() {
                match dispatch_event(UiEvent::WindowResized { viewport }, backend, app) {
                    EventOutcome::Continue => {}
                    EventOutcome::Redraw => needs_redraw = true,
                    EventOutcome::Exit => return Ok(()),
                }
            }
        }

        // Periodic tick — called after every event batch (including
        // timeout-triggered empty batches). Lets apps drive timer
        // logic without synthetic event injection.
        match app.tick(backend) {
            Reaction::Continue => {}
            Reaction::Redraw => needs_redraw = true,
            Reaction::Exit => return Ok(()),
        }
    }
}

/// Render one frame into a `size`-sized surface.
///
/// Seeds the viewport from the given `size` (rather than querying the
/// backend), runs `app.render` inside the backend's frame scope, overlays
/// the active text-selection highlight, applies any editor cursor position
/// painted this frame, and finalises the frame. Generic over the ratatui
/// backend `B` so every caller — the live runner (`CrosstermBackend`
/// wrapping real stdout), the headless `TestBackend` driver, and the
/// headless *vt100* driver (`CrosstermBackend` wrapping an in-memory ANSI
/// sink, quadraui#555) — share one paint path.
///
/// Split out of [`render_frame`] (which still queries `terminal.size()` for
/// the live runner and `TestBackend`) because `CrosstermBackend::size()`
/// queries the process's real controlling terminal (`/dev/tty` on Unix) via
/// `crossterm::terminal::size()` — a query that has nothing to do with
/// whatever `Write` sink the backend was constructed with, and fails (or
/// silently returns the wrong dimensions) under `cargo test`, which has no
/// controlling terminal wired to the vt100 driver's sink. A caller that
/// already knows its own fixed size (every `ConformanceDriver` does — it's
/// exactly the `LogicalViewport` it was built with) calls this directly and
/// never triggers that query at all.
pub(crate) fn paint_frame<A, B>(
    terminal: &mut Terminal<B>,
    backend: &mut TuiBackend,
    app: &A,
    size: ratatui::layout::Size,
) -> io::Result<()>
where
    A: AppLogic,
    B: ratatui::backend::Backend,
{
    backend.begin_frame(crate::Viewport::new(
        size.width as f32,
        size.height as f32,
        1.0,
    ));
    terminal
        .draw(|frame| {
            backend.enter_frame_scope(frame, |b| {
                // TUI is single-area; always pass the app's default
                // `AreaId`. Multi-area runners (GTK) pass the AreaId for
                // whichever surface is repainting.
                app.render(b, A::AreaId::default());
            });
            // After app.render: overlay selection highlight on the rendered
            // buffer. Done outside enter_frame_scope so the closure lifetime
            // doesn't conflict with the frame borrow.
            backend.apply_selection_highlight(frame.buffer_mut());
            // Apply the editor cursor position cached by `draw_editor`
            // (quadraui#466). `Backend::draw_editor` only has the buffer,
            // not the `Frame`, so it stashes the position on `TuiBackend`
            // for us to apply here — the one place per frame with access
            // to the real `Frame::set_cursor_position`. `run_with_shell`
            // and `TuiDriver` both go through this same `render_frame`, so
            // they pick up the behavior for free.
            if let Some(pos) = backend.take_last_cursor_position() {
                frame.set_cursor_position(pos);
            }
        })
        .map_err(|e| io::Error::other(e.to_string()))?;
    backend.end_frame();
    Ok(())
}

/// Render one frame, syncing the viewport from the *backend's* real size.
///
/// Thin wrapper over [`paint_frame`] for callers whose backend can actually
/// answer `terminal.size()` honestly — the live runner (real stdout) and
/// the headless `TestBackend` driver (told its size at construction, so
/// `size()` is just a getter). See [`paint_frame`]'s doc for why a
/// `CrosstermBackend` wrapping anything else needs to skip this query.
pub(crate) fn render_frame<A, B>(
    terminal: &mut Terminal<B>,
    backend: &mut TuiBackend,
    app: &A,
) -> io::Result<()>
where
    A: AppLogic,
    B: ratatui::backend::Backend,
{
    let size = terminal
        .size()
        .map_err(|e| io::Error::other(e.to_string()))?;
    paint_frame(terminal, backend, app, size)
}

// `EventOutcome` — what the frame loop should do after [`dispatch_event`]
// handles one event — is defined once in `crate::runtime` and shared by
// every backend runner (quadraui#496); imported at the top of this file.

/// Dispatch one [`UiEvent`] through the app, applying the runner's
/// built-in pre-processing first.
///
/// Pre-processing handled here (before — or instead of — the app's
/// `handle`):
/// - [`UiEvent::TextSelectionChanged`]: update the backend's active
///   selection, then still forward the event to the app and force a
///   redraw.
/// - [`UiEvent::MouseDown`]: clear the displayed selection highlight,
///   then forward.
/// - Ctrl-C with an active selection: copy the selection to the
///   clipboard and emit [`UiEvent::TextCopied`] to the app (so it can
///   show copy-confirmation UI) **instead of** forwarding the Ctrl-C —
///   forwarding it could trigger quit/copy-all handlers, and
///   `ClipboardPaste` would wrongly insert text.
pub(crate) fn dispatch_event<A: AppLogic>(
    event: UiEvent,
    backend: &mut TuiBackend,
    app: &mut A,
) -> EventOutcome {
    let mut force_redraw = false;
    match &event {
        // Update active selection while dragging.
        UiEvent::TextSelectionChanged {
            region,
            anchor,
            focus,
        } => {
            backend.set_active_text_selection(region.clone(), *anchor, *focus);
            force_redraw = true;
        }
        // Clear the displayed selection highlight on any mouse-down. Use
        // `clear_selection_display` rather than `clear_text_selection`
        // so we don't cancel the `TextSelection` drag that `wait_events`
        // may have just started for this very event.
        UiEvent::MouseDown { .. } => {
            backend.clear_selection_display();
        }
        // Ctrl-C (any case, any extra modifiers) with an active
        // selection → copy to clipboard and notify the app via
        // TextCopied. Accepts 'C' (CapsLock) and tolerates stray
        // modifier bits some terminals attach to Ctrl-C.
        UiEvent::KeyPressed {
            key: Key::Char('c') | Key::Char('C'),
            modifiers,
            ..
        } if modifiers.ctrl
            && !modifiers.alt
            && !modifiers.cmd
            && backend.active_text_selection().is_some() =>
        {
            let text = backend.cached_selection_text();
            backend.services().clipboard().write_text(&text);
            backend.clear_text_selection();
            return match app.handle(UiEvent::TextCopied(text), backend) {
                Reaction::Exit => EventOutcome::Exit,
                _ => EventOutcome::Redraw,
            };
        }
        // Ctrl-A (any case; not Ctrl-Shift-A) → select the entire
        // content of the most-recently focused `TextRegion`, if one is
        // registered. Accepts 'A' (CapsLock). Falls through to the app
        // when no region resolves so app-level Ctrl-A handlers (e.g. a
        // tree-node inline-edit select-all) are unaffected.
        //
        // Priority note: when a `TextRegion` is registered the runner
        // takes Ctrl-A; apps that register a `TextRegion` and also want
        // their own Ctrl-A handler should clear the region first.
        UiEvent::KeyPressed {
            key: Key::Char('a') | Key::Char('A'),
            modifiers,
            ..
        } if modifiers.ctrl
            && !modifiers.shift
            && !modifiers.alt
            && !modifiers.cmd
            && backend.select_all_text_region() =>
        {
            return EventOutcome::Redraw;
        }
        _ => {}
    }

    // ── Normal app dispatch ─────────────────────────────────────
    match app.handle(event, backend) {
        Reaction::Continue => {
            if force_redraw {
                EventOutcome::Redraw
            } else {
                EventOutcome::Continue
            }
        }
        Reaction::Redraw => EventOutcome::Redraw,
        Reaction::Exit => EventOutcome::Exit,
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

// ── Selection pipeline tests ──────────────────────────────────────────────────
//
// These use `TuiDriver` to exercise the full dispatch_event + render_frame
// path (the same code the live runner and `run_with_shell` both use).
// Each test builds a minimal app that registers a text region, then
// verifies the selection pipeline's observable behaviour.

#[cfg(test)]
mod tests {
    use crate::runner::{AppLogic, Reaction};
    use crate::tui::testing::TuiDriver;
    use crate::{Backend, Key, Point, Rect, TextRegion, UiEvent, WidgetId};

    // ── Minimal test app ──────────────────────────────────────────────────────

    /// Records `TextCopied` payloads and `TextSelectionChanged` anchor/focus
    /// so tests can assert on them without a real clipboard.
    struct SelectionRecorder {
        last_copied: Option<String>,
        selection_changes: Vec<(Point, Point)>,
    }

    impl SelectionRecorder {
        fn new() -> Self {
            Self {
                last_copied: None,
                selection_changes: Vec::new(),
            }
        }

        fn config_rect() -> Rect {
            // 20-wide × 5-tall text region at the top-left corner.
            Rect::new(0.0, 0.0, 20.0, 5.0)
        }
    }

    impl AppLogic for SelectionRecorder {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            // Register the text region every frame so dispatch_click can find it.
            backend.register_text_region(TextRegion {
                id: WidgetId::new("test-region"),
                bounds: Self::config_rect(),
                lines: vec![
                    "line one".into(),
                    "line two".into(),
                    "line three".into(),
                    "line four".into(),
                    "line five".into(),
                ],
            });
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            match event {
                UiEvent::TextCopied(text) => {
                    self.last_copied = Some(text);
                    Reaction::Redraw
                }
                UiEvent::TextSelectionChanged { anchor, focus, .. } => {
                    self.selection_changes.push((anchor, focus));
                    Reaction::Redraw
                }
                UiEvent::KeyPressed {
                    key: Key::Char('q'),
                    ..
                } => Reaction::Exit,
                _ => Reaction::Continue,
            }
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    /// Dragging across a registered `TextRegion` emits `TextSelectionChanged`.
    ///
    /// This exercises the full pipeline:
    ///   `MouseDown` → `dispatch_click` starts drag →
    ///   `MouseMoved` → `dispatch_mouse_drag` → `TextSelectionChanged` →
    ///   `dispatch_event` calls `set_active_text_selection`.
    #[test]
    fn drag_over_text_region_emits_selection_changed() {
        let mut driver = TuiDriver::new(SelectionRecorder::new(), 40, 10);

        // Start drag at (2, 0), move to (2, 2) — two rows.
        driver.mouse_down(2.0, 0.0);
        driver.mouse_move(2.0, 2.0);

        assert!(
            !driver.app().selection_changes.is_empty(),
            "drag over a text region must emit TextSelectionChanged"
        );
        let (anchor, _focus) = driver.app().selection_changes[0];
        // Anchor should be close to where the drag started.
        assert!(
            anchor.y < 1.0,
            "anchor row should be 0 (drag started at y=0), got y={}",
            anchor.y
        );
    }

    /// `Ctrl-C` with an active selection emits `TextCopied` to the app
    /// instead of forwarding the raw key press.
    #[test]
    fn ctrl_c_with_selection_emits_text_copied() {
        let mut driver = TuiDriver::new(SelectionRecorder::new(), 40, 10);

        // Build a selection by dragging.
        driver.mouse_down(0.0, 0.0);
        driver.mouse_move(0.0, 1.0);
        driver.mouse_up(0.0, 1.0);

        // Ctrl-C should fire TextCopied, not a raw KeyPressed.
        driver.ctrl_char('c');

        assert!(
            driver.app().last_copied.is_some(),
            "Ctrl-C with active selection must emit TextCopied to the app"
        );
    }

    /// `Ctrl-C` without any selection does NOT emit `TextCopied` — the raw
    /// `KeyPressed` is forwarded to the app instead (exit handled by 'q',
    /// but Ctrl-C without selection stays as a KeyPressed for the app).
    #[test]
    fn ctrl_c_without_selection_does_not_emit_text_copied() {
        let mut driver = TuiDriver::new(SelectionRecorder::new(), 40, 10);

        // No drag — no active selection.
        driver.ctrl_char('c');

        assert!(
            driver.app().last_copied.is_none(),
            "Ctrl-C without an active selection must NOT emit TextCopied"
        );
    }

    /// `Ctrl-A` selects the entire registered text region and subsequent
    /// `Ctrl-C` copies the full content.
    #[test]
    fn ctrl_a_then_ctrl_c_copies_full_region() {
        let mut driver = TuiDriver::new(SelectionRecorder::new(), 40, 10);

        driver.ctrl_char('a'); // select-all
        driver.ctrl_char('c'); // copy

        assert!(
            driver.app().last_copied.is_some(),
            "Ctrl-A + Ctrl-C must copy the region content"
        );
        let text = driver.app().last_copied.as_deref().unwrap_or("");
        // The content must contain at least some of the region's lines.
        assert!(
            !text.is_empty(),
            "copied text must not be empty after Ctrl-A"
        );
    }

    /// A `MouseDown` clears the displayed selection without ending an ongoing
    /// drag (so the new drag can replace the old selection).
    #[test]
    fn mouse_down_clears_selection_display() {
        let mut driver = TuiDriver::new(SelectionRecorder::new(), 40, 10);

        // Build a selection.
        driver.mouse_down(0.0, 0.0);
        driver.mouse_move(0.0, 2.0);
        driver.mouse_up(0.0, 2.0);

        // Verify selection exists (active_selection is Some after mouse-up
        // because the TUI runner preserves the finalised selection).
        assert!(
            driver.backend().active_text_selection().is_some(),
            "drag should have established a selection before the test"
        );

        // A new MouseDown should clear the displayed selection.
        driver.mouse_down(0.0, 4.0);

        // After the new MouseDown the old highlight should be gone.
        assert!(
            driver.backend().active_text_selection().is_none(),
            "MouseDown must clear the previously displayed selection"
        );
    }

    /// `cancel_text_selection_drag` ends an in-progress drag without
    /// clearing the active selection display. This is the #454 pattern:
    /// the app forwards a click to the PTY and then cancels the speculative
    /// drag the runner started.
    #[test]
    fn cancel_text_selection_drag_does_not_clear_display() {
        use crate::DragTarget;

        let mut backend = crate::tui::backend::TuiBackend::new();

        // Manually start a TextSelection drag (simulates what apply_dispatch
        // does on MouseDown inside a text region).
        backend
            .drag_and_modal_mut()
            .0
            .begin(DragTarget::TextSelection {
                region: WidgetId::new("r"),
                anchor: Point::new(0.0, 0.0),
            });

        // Also set an active (finalised) selection.
        backend.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.0),
        );

        // Cancel the drag (PTY forwarding path).
        backend.cancel_text_selection_drag();

        // Drag state should be cleared.
        assert!(
            !backend.drag_and_modal_mut().0.is_active(),
            "cancel_text_selection_drag must end the active drag"
        );

        // But the displayed selection should still be there.
        assert!(
            backend.active_text_selection().is_some(),
            "cancel_text_selection_drag must NOT clear the active selection display"
        );
    }

    // ── Editor cursor-position pipeline (quadraui#466) ─────────────────────────
    //
    // `Backend::draw_editor`'s `EditorPaintResult::cursor_position` used to have
    // no consumer downstream of `AppLogic::render` — the runner never applied it
    // to the real ratatui `Frame`. These tests exercise the fix: `render_frame`
    // takes `TuiBackend`'s cached position (set by the `draw_editor` trait impl)
    // and calls `Frame::set_cursor_position`, observable via `TestBackend`'s own
    // cursor state.

    /// Minimal app that paints a single-line `Editor` with a `Bar`-shaped
    /// cursor at a configurable column. `Bar`/`Underline` cursors are the
    /// shapes `tui::draw_editor` reports via `EditorPaintResult::cursor_position`
    /// (a `Block` cursor is drawn as an inverted cell instead — see
    /// `tui/editor.rs`'s cursor-paint match).
    struct EditorCursorApp {
        cursor_col: usize,
        /// When false, `render` paints no editor at all — used to verify the
        /// cursor position doesn't linger from a previous frame.
        show_editor: bool,
        /// Last `Backend::draw_editor` return value, captured verbatim
        /// (issue #504's `cursor_position` → `cursor_position_native`
        /// mapping test reads this directly; the terminal-cursor tests
        /// below go through the higher-level `TuiDriver` handoff instead).
        last_result: std::cell::RefCell<Option<crate::backend::EditorPaintResult>>,
    }

    impl EditorCursorApp {
        fn new(cursor_col: usize) -> Self {
            Self {
                cursor_col,
                show_editor: true,
                last_result: std::cell::RefCell::new(None),
            }
        }

        fn build_editor(&self) -> crate::Editor {
            crate::Editor {
                id: WidgetId::new("editor"),
                rect: Rect::new(0.0, 0.0, 20.0, 5.0),
                lines: vec![crate::EditorLine {
                    raw_text: "hello world".into(),
                    gutter_text: String::new(),
                    spans: vec![],
                    line_idx: 0,
                    is_current_line: true,
                    is_fold_header: false,
                    folded_line_count: 0,
                    git_diff: None,
                    diff_status: None,
                    diagnostics: vec![],
                    spell_errors: vec![],
                    is_breakpoint: false,
                    is_conditional_bp: false,
                    is_dap_current: false,
                    is_wrap_continuation: false,
                    segment_col_offset: 0,
                    annotation: None,
                    ghost_suffix: None,
                    is_ghost_continuation: false,
                    indent_guides: vec![],
                    colorcolumns: vec![],
                }],
                cursor: Some(crate::EditorCursor {
                    pos: crate::EditorCursorPos {
                        view_line: 0,
                        col: self.cursor_col,
                    },
                    shape: crate::EditorCursorShape::Bar,
                }),
                extra_cursors: vec![],
                selection: None,
                extra_selections: vec![],
                yank_highlight: None,
                scroll_top: 0,
                scroll_left: 0,
                total_lines: 1,
                max_col: 11,
                gutter_char_width: 0,
                is_active: true,
                show_active_bg: false,
                has_git_diff: false,
                has_breakpoints: false,
                diagnostic_gutter: Default::default(),
                code_action_lines: Default::default(),
                bracket_match_positions: vec![],
                active_indent_col: None,
                tabstop: 4,
                cursorline: false,
                lightbulb_glyph: '\0',
            }
        }
    }

    impl AppLogic for EditorCursorApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            if self.show_editor {
                let editor = self.build_editor();
                let result = backend.draw_editor(editor.rect, &editor);
                *self.last_result.borrow_mut() = Some(result);
            }
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// A `Bar`-cursor `Editor` paints its `cursor_position` onto the real
    /// `Frame` — `render_frame` must apply it via `Frame::set_cursor_position`,
    /// observable through `TestBackend`'s own cursor state.
    #[test]
    fn editor_bar_cursor_position_reaches_the_terminal_frame() {
        let mut driver = TuiDriver::new(EditorCursorApp::new(3), 40, 10);

        // Gutter width 0, scroll_left 0 → screen x == cursor_col, screen y ==
        // the editor rect's origin row (0).
        assert_eq!(
            driver.terminal_cursor_position(),
            Some((3, 0)),
            "draw_editor's cursor_position must reach Frame::set_cursor_position"
        );
    }

    /// Moving the cursor and re-rendering updates the applied terminal
    /// position — confirms the handoff isn't a one-shot artifact of the
    /// first frame.
    #[test]
    fn editor_bar_cursor_position_updates_across_frames() {
        let mut driver = TuiDriver::new(EditorCursorApp::new(3), 40, 10);
        assert_eq!(driver.terminal_cursor_position(), Some((3, 0)));

        driver.app_mut().cursor_col = 7;
        driver.render();

        assert_eq!(
            driver.terminal_cursor_position(),
            Some((7, 0)),
            "a later frame's draw_editor call must overwrite the previous cursor position"
        );
    }

    /// Issue #504: `TuiBackend::draw_editor` must widen the TUI-internal,
    /// already cell-rounded `(u16, u16)` cursor position into the portable
    /// `EditorPaintResult::cursor_position_native` `Point` with the same
    /// `(x, y)` ordering — and keep populating the deprecated
    /// `cursor_position` tuple field with the exact same pair, since
    /// `vimcode`'s `Frame::set_cursor_position(result.cursor_position)`
    /// call site still relies on it (see that field's doc for the full
    /// deprecation contract).
    #[test]
    fn draw_editor_result_maps_cell_cursor_to_native_point() {
        let mut driver = TuiDriver::new(EditorCursorApp::new(3), 40, 10);
        driver.render();

        let result = driver
            .app()
            .last_result
            .borrow()
            .clone()
            .expect("render() must call draw_editor and capture its result");

        #[allow(deprecated)] // issue #504: asserting the deprecated shim field too
        let cell = result.cursor_position;
        assert_eq!(
            cell,
            Some((3, 0)),
            "deprecated cursor_position must still carry the cell-rounded pair"
        );
        assert_eq!(
            result.cursor_position_native,
            cell.map(|(x, y)| Point::new(x as f32, y as f32)),
            "cursor_position_native must be the same (x, y) pair widened to f32, \
             not swapped or independently computed"
        );
    }
}
