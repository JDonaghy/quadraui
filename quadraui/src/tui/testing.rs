//! In-process headless test driver for [`AppLogic`] TUI apps.
//!
//! [`TuiDriver`] drives a *whole* [`AppLogic`] impl — the same type the
//! shipping `tui_*` examples instantiate — through the real
//! event → `handle` → `render` path, but against ratatui's in-memory
//! [`TestBackend`] instead of a live terminal. No TTY, no pty: it runs
//! under `cargo test` and is deterministic.
//!
//! ## What it tests that the primitive round-trips don't
//!
//! The per-primitive paint/click round-trips (e.g. `tui/tree.rs::tests`)
//! call a single `draw_*` fn on a hand-built struct and assert on the
//! buffer + the `hit_test` return value. They never construct an app,
//! never dispatch a [`UiEvent`] through [`AppLogic::handle`], and never
//! touch the run loop. [`TuiDriver`] tests the *example's wiring*: feed
//! a keystroke or a click, observe the re-rendered screen. It catches a
//! handler routing a click to the wrong target, a missing
//! [`Reaction::Redraw`], or stale state — none of which the primitive
//! tests can see.
//!
//! ## No drift from production
//!
//! [`Self::render`] and [`Self::dispatch`] call the same
//! [`super::run::render_frame`] / [`super::run::dispatch_event`] the live
//! runner uses, so the test path renders and pre-processes events
//! (text-selection, Ctrl-C copy) identically to `tui::run`. Mouse input
//! is a direct [`UiEvent::MouseDown`] in backend coordinates — no SGR
//! escape-sequence math, which is the ergonomic win over a pty runner
//! for click-heavy primitives.
//!
//! ## Limitations
//!
//! It renders into a `TestBackend` buffer, so it does **not** exercise
//! real ANSI/escape emission — terminal-protocol bugs (raw-mode setup,
//! escape parsing, SGR mouse decoding) are out of scope and need a
//! pty-based smoke test instead.
//!
//! ```no_run
//! # use quadraui::tui::testing::TuiDriver;
//! # use quadraui::{AppLogic, Backend, Reaction, UiEvent};
//! # fn demo<A: AppLogic + Default>() {
//! let mut driver = TuiDriver::new(A::default(), 100, 30);
//! driver.type_char('x');
//! assert!(driver.screen_contains("…"));
//! # }
//! ```

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::backend::Backend;
use crate::runner::{AppLogic, Reaction};
use crate::shell::{ShellApp, ShellConfig};
use crate::tui::backend::TuiBackend;
use crate::tui::run::{dispatch_event, render_frame, EventOutcome};
use crate::{ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, UiEvent};

/// Build a [`TuiDriver`] that wraps `app` in the full
/// [`crate::shell_adapter::ShellAdapter`] stack, mirroring exactly what
/// [`crate::tui::shell_runner::run_with_shell`] does at runtime — but
/// returning a testable driver instead of entering the live event loop.
///
/// Use this constructor in tests that need to verify the full
/// `ShellApp → ShellAdapter → dispatch_event` integration path, e.g.
/// confirming that `register_text_region()` is reached via
/// `ShellAdapter::render()` and that the selection pipeline (drag → Ctrl-C)
/// works end-to-end for `run_with_shell` callers.
///
/// # Example
///
/// ```no_run
/// # use quadraui::tui::testing::driver_with_shell;
/// # use quadraui::{ShellApp, ShellConfig, Backend, ShellContext, Reaction, UiEvent};
/// # struct MyApp;
/// # impl ShellApp for MyApp {
/// #     fn render_content(&self, _: &mut dyn Backend, _: &quadraui::compose::app_shell::AppShellLayout) {}
/// #     fn handle(&mut self, _: UiEvent, _: &mut dyn Backend, _: &ShellContext) -> Reaction { Reaction::Continue }
/// # }
/// let config = ShellConfig::new("Demo", vec![]);
/// let mut driver = driver_with_shell(MyApp, config, 80, 24);
/// assert!(driver.screen_contains("Demo"));
/// ```
pub fn driver_with_shell<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
    width: u16,
    height: u16,
) -> TuiDriver<impl AppLogic> {
    let adapter = crate::tui::shell_runner::build_shell_adapter(app, config);
    TuiDriver::new(adapter, width, height)
}

/// Drives an [`AppLogic`] impl headlessly for tests.
///
/// Construct with [`Self::new`] (which runs `setup` + paints the first
/// frame), poke it with [`Self::press`] / [`Self::type_char`] /
/// [`Self::press_named`] / [`Self::click`], and read the rendered grid
/// back with [`Self::screen`] / [`Self::screen_contains`] / [`Self::find`].
pub struct TuiDriver<A: AppLogic> {
    app: A,
    backend: TuiBackend,
    terminal: Terminal<TestBackend>,
    exited: bool,
}

impl<A: AppLogic> TuiDriver<A> {
    /// Build a driver for `app` on a `width`×`height` cell grid, run the
    /// app's `setup` hook, and paint the first frame.
    pub fn new(app: A, width: u16, height: u16) -> Self {
        let terminal =
            Terminal::new(TestBackend::new(width, height)).expect("TestBackend terminal");
        let mut backend = TuiBackend::new();
        // Seed the viewport from the driver's terminal size BEFORE setup,
        // exactly as the live runner does (quadraui#437). Without this,
        // `app.setup()` would read the default 80×24 viewport instead of
        // the requested `width`×`height`, so any setup-time sizing (e.g.
        // `TerminalApp` spawning its PTY) would use the wrong dimensions.
        backend.begin_frame(crate::Viewport::new(width as f32, height as f32, 1.0));
        let mut app = app;
        app.setup(&mut backend);
        let mut driver = Self {
            app,
            backend,
            terminal,
            exited: false,
        };
        driver.render();
        driver
    }

    /// Repaint one frame through the shared production render path.
    pub fn render(&mut self) {
        render_frame(&mut self.terminal, &mut self.backend, &self.app)
            .expect("TestBackend render is infallible");
    }

    /// Feed one synthetic event through the **full production pipeline**:
    /// backend translation ([`TuiBackend::translate_injected`] — drag
    /// state, accelerator matching, double-click folding) followed by the
    /// shared [`dispatch_event`] for each resulting event. Repaints on
    /// redraw and latches `exited`. Returns the strongest [`Reaction`]
    /// (Exit > Redraw > Continue) for convenient assertions.
    ///
    /// Routing through `translate_injected` (not straight to
    /// `dispatch_event`) is what makes drag sequences work: a `MouseDown`
    /// in a registered text region begins a `TextSelection` drag, and a
    /// following `MouseMoved` is translated into `TextSelectionChanged` —
    /// exactly as `wait_events` does for real crossterm input.
    pub fn dispatch(&mut self, event: UiEvent) -> Reaction {
        if self.exited {
            return Reaction::Exit;
        }
        let mut result = Reaction::Continue;
        for ev in self.backend.translate_injected(vec![event]) {
            match dispatch_event(ev, &mut self.backend, &mut self.app) {
                EventOutcome::Continue => {}
                EventOutcome::Redraw => {
                    self.render();
                    if result == Reaction::Continue {
                        result = Reaction::Redraw;
                    }
                }
                EventOutcome::Exit => {
                    self.exited = true;
                    return Reaction::Exit;
                }
            }
        }
        result
    }

    /// Press a key (no modifiers).
    pub fn press(&mut self, key: Key) -> Reaction {
        self.dispatch(UiEvent::KeyPressed {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// Type a single character key (no modifiers).
    pub fn type_char(&mut self, c: char) -> Reaction {
        self.press(Key::Char(c))
    }

    /// Press a named (non-printable) key, e.g. [`NamedKey::Enter`].
    pub fn press_named(&mut self, key: NamedKey) -> Reaction {
        self.press(Key::Named(key))
    }

    /// Press a character key with Ctrl held (e.g. `ctrl_char('c')` to
    /// trigger the runner's copy-on-selection path).
    pub fn ctrl_char(&mut self, c: char) -> Reaction {
        self.dispatch(UiEvent::KeyPressed {
            key: Key::Char(c),
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        })
    }

    /// Left-click at backend coordinates `(x, y)` (cell units for TUI),
    /// delivered as a [`UiEvent::MouseDown`] — the event primitives'
    /// hit-test paths consume.
    pub fn click(&mut self, x: f32, y: f32) -> Reaction {
        self.mouse_down(x, y)
    }

    /// Press the left mouse button down at `(x, y)`. Begins a drag if it
    /// lands on a draggable target (text region, scrollbar thumb).
    pub fn mouse_down(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        })
    }

    /// Move the cursor to `(x, y)` with the left button held. During an
    /// active drag this is translated to the drag's high-level event
    /// (e.g. [`UiEvent::TextSelectionChanged`] /
    /// [`UiEvent::ScrollOffsetChanged`]).
    pub fn mouse_move(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask {
                left: true,
                ..ButtonMask::default()
            },
        })
    }

    /// Release the left mouse button at `(x, y)`, ending any active drag.
    pub fn mouse_up(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
        })
    }

    /// Left-button drag from `(x0, y0)` to `(x1, y1)`: down → move → up.
    pub fn drag(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) -> Reaction {
        self.mouse_down(x0, y0);
        self.mouse_move(x1, y1);
        self.mouse_up(x1, y1)
    }

    /// Whether the app has returned [`Reaction::Exit`].
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// The current rendered screen as newline-joined rows.
    pub fn screen(&self) -> String {
        let buf = self.terminal.backend().buffer();
        let area = buf.area;
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// True if any rendered row contains `needle`.
    pub fn screen_contains(&self, needle: &str) -> bool {
        self.screen().contains(needle)
    }

    /// Access the app state for test assertions.
    ///
    /// Useful when the test app records side-effects (e.g. last copied text,
    /// selection changes) that would otherwise require screen-scraping.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// Access the backend for test assertions (e.g. active selection state,
    /// drag state).
    pub fn backend(&self) -> &TuiBackend {
        &self.backend
    }

    /// Cell-centre coordinates of the first row containing `needle`, at
    /// the start of the match. Counts in *character cells* (not bytes),
    /// so it works on rows full of multi-byte box-drawing glyphs.
    /// Assumes 1 cell per char (no double-width CJK) — adequate for
    /// clicking painted ASCII labels in tests.
    pub fn find(&self, needle: &str) -> Option<(f32, f32)> {
        let needle: Vec<char> = needle.chars().collect();
        if needle.is_empty() {
            return None;
        }
        for (y, line) in self.screen().lines().enumerate() {
            let row: Vec<char> = line.chars().collect();
            if let Some(col) = row
                .windows(needle.len())
                .position(|w| w == needle.as_slice())
            {
                return Some((col as f32 + 0.5, y as f32 + 0.5));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Viewport;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Minimal app that records the viewport it observes in `setup()`.
    ///
    /// This is the driver-reachable regression guard for quadraui#437's
    /// TUI initial-layout bug: `setup()` must see the driver's real
    /// terminal size, not the `Viewport::default()` (80×24) that
    /// `TuiBackend::new()` seeds. An app that sizes a side effect from the
    /// setup-time viewport (e.g. `TerminalApp` spawning its PTY) would
    /// otherwise under-fill the pane until the first resize event.
    struct SetupViewportRecorder {
        seen: Rc<Cell<Option<Viewport>>>,
    }

    impl AppLogic for SetupViewportRecorder {
        type AreaId = ();

        fn setup(&mut self, backend: &mut dyn Backend) {
            self.seen.set(Some(backend.viewport()));
        }

        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// `setup()` must observe the driver's real terminal dimensions, not
    /// the default 80×24 viewport. Regression guard for the #437 TUI
    /// initial-layout bug (pane under-filled until first interaction).
    #[test]
    fn setup_sees_real_viewport_not_default() {
        let seen = Rc::new(Cell::new(None));
        let app = SetupViewportRecorder { seen: seen.clone() };
        // Deliberately not 80×24 so a regression to the default is visible.
        let _driver = TuiDriver::new(app, 120, 40);

        let vp = seen.get().expect("setup() should have run");
        assert_eq!(
            (vp.width, vp.height),
            (120.0, 40.0),
            "setup() must see the driver's real size, not Viewport::default() (80×24)"
        );
    }
}
