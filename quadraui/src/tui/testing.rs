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

use crate::runner::{AppLogic, Reaction};
use crate::tui::backend::TuiBackend;
use crate::tui::run::{dispatch_event, render_frame, EventOutcome};
use crate::{Key, Modifiers, MouseButton, NamedKey, Point, UiEvent};

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

    /// Dispatch one event through the shared production dispatch path,
    /// applying its outcome (repaint on redraw, latch `exited` on exit).
    /// Returns the equivalent [`Reaction`] for convenient assertions.
    pub fn dispatch(&mut self, event: UiEvent) -> Reaction {
        if self.exited {
            return Reaction::Exit;
        }
        match dispatch_event(event, &mut self.backend, &mut self.app) {
            EventOutcome::Continue => Reaction::Continue,
            EventOutcome::Redraw => {
                self.render();
                Reaction::Redraw
            }
            EventOutcome::Exit => {
                self.exited = true;
                Reaction::Exit
            }
        }
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

    /// Left-click at backend coordinates `(x, y)` (cell units for TUI),
    /// delivered as a [`UiEvent::MouseDown`] — the event primitives'
    /// hit-test paths consume.
    pub fn click(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        })
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
