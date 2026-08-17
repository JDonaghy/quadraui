//! In-process TUI conformance observer backed by a real ANSI byte stream
//! (quadraui#555, epic #480 pillar 2 follow-up).
//!
//! [`TuiVtDriver`] drives the same [`AppLogic`] path [`super::testing::TuiDriver`]
//! does — same `setup`/`render`/`handle`, same [`super::run::paint_frame`] /
//! [`super::run::dispatch_event`] — but paints through
//! `ratatui::backend::CrosstermBackend` instead of `TestBackend`, so the
//! bytes it observes are the *real* ANSI stream a live terminal would
//! receive: `MoveTo`/`Print`/SGR sequences, diffed frame-to-frame exactly as
//! `tui::run` diffs them for a live session. Those bytes are parsed back
//! into a screen model with the `vt100` crate, which — unlike `TestBackend`,
//! which just hands back its own retained [`ratatui::buffer::Buffer`] —
//! models double-width cells, combining characters, and SGR the way a real
//! terminal emulator does.
//!
//! ## What this closes
//!
//! [`quadraui::testing::ConformanceDriver`]'s contract only ever promised
//! "what did the rasteriser ask for" (the paint-time inventory `TestBackend`
//! reads back). It cannot answer "what would a terminal actually show",
//! and for a cell-grid backend those two questions can diverge: content the
//! rasteriser genuinely painted can still be lost in translation to bytes,
//! or in a real emulator's interpretation of those bytes, in a way a
//! same-process buffer read never observes. `TuiVtDriver` is a second
//! [`ConformanceDriver`] implementation over the exact same `AppLogic`
//! fixtures and the exact same scenario files — see
//! `tests/conformance/scenarios/` — so the conformance matrix can run a
//! scenario against both and report them as distinct rows (see
//! `docs/TESTING.md` → *TUI: two observers*).
//!
//! ## Why no real OS pty
//!
//! [`super::testing::TuiDriver`] drives its `AppLogic` fixture with scripted
//! [`UiEvent`]s, never real keystrokes — there is no ANSI *input* to decode,
//! only output to observe. `tests/tui_pty_smoke.rs` (#302) needs a real pty
//! because it spawns an actual example *binary* and must answer that
//! process's `ESC [ 6 n` cursor-position query the way a real terminal
//! would, or the process's own `Terminal::new()` fails before it ever
//! renders. `TuiVtDriver` never spawns a process and never lets
//! `ratatui`/`crossterm` query a real terminal for *anything* — see
//! [`super::run::paint_frame`]'s doc for exactly which call that would be
//! (`CrosstermBackend::size()`, which reads `/dev/tty`, unrelated to
//! whatever `Write` sink the backend wraps) and how this driver avoids
//! it entirely by using a fixed ratatui `Viewport` and never calling
//! `Terminal::size()`. The result is deterministic and needs no
//! subprocess, no thread, and no wall-clock polling — the same properties
//! that make `TestBackend`-based `TuiDriver` fast, just with a real ANSI
//! byte stream in the loop. `tests/tui_pty_smoke.rs`'s real-pty tier
//! remains the one place terminal-*input* protocol bugs (raw-mode setup,
//! SGR mouse decoding) are covered; this driver is scoped to output
//! fidelity only.
//!
//! ## Limitations
//!
//! - No real terminal-input decoding (see above) — this is an *output*
//!   observer.
//! - `vt100` (like every real terminal) has its own quirks and coverage
//!   gaps; a scenario failing only here is worth confirming against a real
//!   terminal before trusting it over `TestBackend`.
//! - Zones ([`FrameInventory::zones`]) come straight from
//!   [`crate::tui::backend::TuiBackend::zones`], same as `TuiDriver` — zone
//!   registration is backend bookkeeping, not rasterised output, so there is
//!   nothing for a byte-stream observer to add there.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Rect as RtRect, Size};
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::backend::Backend;
use crate::runner::{AppLogic, Reaction};
use crate::testing::{Anchor, ConformanceDriver, FrameInventory, LogicalViewport, TextRun};
use crate::tui::backend::TuiBackend;
use crate::tui::run::{dispatch_event, paint_frame, EventOutcome};
use crate::{ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, Rect, ScrollDelta, UiEvent};

/// `io::Write` sink that feeds every byte `CrosstermBackend` emits straight
/// into a `vt100::Parser` — the "terminal" on the other end of the ANSI
/// stream, minus the OS pty plumbing (see the module doc's "Why no real OS
/// pty").
struct VtSink(Rc<RefCell<vt100::Parser>>);

impl io::Write for VtSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().process(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Drives an [`AppLogic`] impl headlessly, observing it through a real ANSI
/// byte stream parsed by `vt100` rather than a same-process buffer read.
/// See the module doc for the full rationale; see
/// [`super::testing::TuiDriver`] for the paired `TestBackend` observer this
/// mirrors method-for-method.
pub struct TuiVtDriver<A: AppLogic> {
    app: A,
    backend: TuiBackend,
    terminal: Terminal<CrosstermBackend<VtSink>>,
    parser: Rc<RefCell<vt100::Parser>>,
    cols: u16,
    rows: u16,
    exited: bool,
}

impl<A: AppLogic> TuiVtDriver<A> {
    /// Build a driver for `app` on a `width`×`height` cell grid, run the
    /// app's `setup` hook, and paint the first frame.
    pub fn new(app: A, width: u16, height: u16) -> Self {
        let parser = Rc::new(RefCell::new(vt100::Parser::new(height, width, 0)));
        let sink = VtSink(Rc::clone(&parser));
        let crossterm_backend = CrosstermBackend::new(sink);
        // `Viewport::Fixed` is the load-bearing choice: `with_options` only
        // calls `backend.size()` for `Fullscreen`/`Inline` (see
        // `paint_frame`'s doc) — `Fixed` takes the given `RtRect` verbatim,
        // so construction never touches a real terminal.
        let terminal = Terminal::with_options(
            crossterm_backend,
            TerminalOptions {
                viewport: Viewport::Fixed(RtRect::new(0, 0, width, height)),
            },
        )
        .expect("CrosstermBackend + Viewport::Fixed never queries a real terminal");

        let mut backend = TuiBackend::new();
        // Seed the viewport from the driver's terminal size BEFORE setup,
        // mirroring `TuiDriver::new` (quadraui#437).
        backend.begin_frame(crate::Viewport::new(width as f32, height as f32, 1.0));
        let mut app = app;
        app.setup(&mut backend);

        let mut driver = Self {
            app,
            backend,
            terminal,
            parser,
            cols: width,
            rows: height,
            exited: false,
        };
        driver.render();
        driver
    }

    /// Repaint one frame through the shared production render path,
    /// flushing through `CrosstermBackend` into the `vt100` sink rather than
    /// `TestBackend`'s in-memory buffer.
    pub fn render(&mut self) {
        paint_frame(
            &mut self.terminal,
            &mut self.backend,
            &self.app,
            Size::new(self.cols, self.rows),
        )
        .expect("CrosstermBackend render into an in-memory vt100 sink is infallible");
    }

    /// Feed one synthetic event through the full production pipeline — see
    /// [`super::testing::TuiDriver::dispatch`], which this mirrors exactly.
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

    /// Press a named (non-printable) key.
    pub fn press_named(&mut self, key: NamedKey) -> Reaction {
        self.press(Key::Named(key))
    }

    /// Press a character key with Ctrl held.
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

    /// Left-click at backend coordinates `(x, y)` (cell units for TUI).
    pub fn click(&mut self, x: f32, y: f32) -> Reaction {
        self.mouse_down(x, y)
    }

    /// Press the left mouse button down at `(x, y)`.
    pub fn mouse_down(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        })
    }

    /// Move the cursor to `(x, y)` with the left button held.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask {
                left: true,
                ..ButtonMask::default()
            },
        })
    }

    /// Release the left mouse button at `(x, y)`.
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

    /// The vt100-observed screen as newline-joined rows — what a real
    /// terminal emulator would show after processing the emitted bytes.
    pub fn screen(&self) -> String {
        let parser = self.parser.borrow();
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let mut out = String::with_capacity((rows as usize) * (cols as usize + 1));
        for y in 0..rows {
            for x in 0..cols {
                let Some(cell) = screen.cell(y, x) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let s = cell.contents();
                out.push_str(if s.is_empty() { " " } else { s });
            }
            out.push('\n');
        }
        out
    }

    /// True if any rendered row contains `needle`.
    pub fn screen_contains(&self, needle: &str) -> bool {
        self.screen().contains(needle)
    }

    /// This row's non-continuation cells as `(char, cell_x, cell_width)`
    /// triples, left to right — the `vt100`-backed twin of
    /// `TuiDriver::row_cells`, reading `vt100::Cell::is_wide`/
    /// `is_wide_continuation` directly instead of recomputing width from
    /// `char_cell_width`, since the parser already tracked it from the real
    /// byte stream.
    fn row_cells(&self, y: u16) -> Vec<(char, u16, u16)> {
        let parser = self.parser.borrow();
        let screen = parser.screen();
        let (_, cols) = screen.size();
        let mut cells = Vec::with_capacity(cols as usize);
        let mut x = 0u16;
        while x < cols {
            let Some(cell) = screen.cell(y, x) else {
                break;
            };
            if cell.is_wide_continuation() {
                // Defensive: a well-formed stream never lands here mid-scan
                // (the wide cell below already advances past it), but skip
                // forward rather than loop forever if it ever does.
                x += 1;
                continue;
            }
            let w = if cell.is_wide() { 2 } else { 1 };
            let ch = cell.contents().chars().next().unwrap_or(' ');
            cells.push((ch, x, w));
            x += w;
        }
        cells
    }

    /// Cell bounds of the first row containing `needle`, wide-char aware —
    /// the `vt100`-backed twin of `TuiDriver::find_bounds`.
    pub fn find_bounds(&self, needle: &str) -> Option<Rect> {
        let needle: Vec<char> = needle.chars().collect();
        if needle.is_empty() {
            return None;
        }
        let rows = self.parser.borrow().screen().size().0;
        for y in 0..rows {
            let cells = self.row_cells(y);
            if cells.len() < needle.len() {
                continue;
            }
            if let Some(start) = cells
                .windows(needle.len())
                .position(|w| w.iter().map(|(c, _, _)| *c).eq(needle.iter().copied()))
            {
                let (_, start_x, _) = cells[start];
                let (_, last_x, last_w) = cells[start + needle.len() - 1];
                let width = (last_x + last_w).saturating_sub(start_x);
                return Some(Rect::new(start_x as f32, y as f32, width as f32, 1.0));
            }
        }
        None
    }

    /// Coordinates of the first row containing `needle`, at the center of
    /// the matched span's first cell — see `TuiDriver::find` for why.
    pub fn find(&self, needle: &str) -> Option<(f32, f32)> {
        self.find_bounds(needle)
            .map(|b| (b.x + 0.5, b.y + b.height / 2.0))
    }
}

impl<A: AppLogic> ConformanceDriver for TuiVtDriver<A> {
    type App = A;

    fn new_fixture(app: Self::App, viewport: LogicalViewport) -> Self {
        TuiVtDriver::new(app, viewport.cols as u16, viewport.rows as u16)
    }

    fn backend_caps(&self) -> crate::BackendCaps {
        // Straight off the real `TuiBackend` this driver wraps too, exactly
        // like `TuiDriver` — the two observers watch the same backend paint,
        // just through different pipes, so their capability claim is
        // identical by construction.
        Backend::backend_caps(&self.backend)
    }

    fn press_named(&mut self, key: NamedKey) {
        TuiVtDriver::press_named(self, key);
    }

    fn type_char(&mut self, c: char) {
        TuiVtDriver::type_char(self, c);
    }

    fn ctrl_char(&mut self, c: char) {
        TuiVtDriver::ctrl_char(self, c);
    }

    fn click_text_at(&mut self, needle: &str, at: Anchor) {
        let bounds = self
            .find_bounds(needle)
            .unwrap_or_else(|| panic!("TuiVtDriver: {needle:?} not painted:\n{}", self.screen()));
        let y = bounds.y + bounds.height / 2.0;
        let x = match at {
            Anchor::Center => bounds.x + bounds.width / 2.0,
            Anchor::LeftEdge => bounds.x + 0.5,
            Anchor::RightEdge => bounds.x + bounds.width - 0.5,
        };
        self.click(x, y);
    }

    fn drag_text(&mut self, from: &str, to: &str) {
        let (x0, y0) = self
            .find(from)
            .unwrap_or_else(|| panic!("TuiVtDriver: {from:?} not painted:\n{}", self.screen()));
        let (x1, y1) = self
            .find(to)
            .unwrap_or_else(|| panic!("TuiVtDriver: {to:?} not painted:\n{}", self.screen()));
        self.drag(x0, y0, x1, y1);
    }

    fn scroll_at(&mut self, needle: &str, lines: i32) {
        let (x, y) = self
            .find(needle)
            .unwrap_or_else(|| panic!("TuiVtDriver: {needle:?} not painted:\n{}", self.screen()));
        let line_height = self.backend.line_height();
        self.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, lines as f32 * line_height),
            position: Point::new(x, y),
        });
    }

    fn inventory(&self) -> FrameInventory {
        let rows = self.parser.borrow().screen().size().0;
        let mut text_runs = Vec::new();
        for y in 0..rows {
            let cells = self.row_cells(y);
            let mut run: Option<(String, u16, u16)> = None; // (text, start_x, end_x)
            for (ch, x, w) in cells {
                if ch == ' ' {
                    if let Some((text, start_x, end_x)) = run.take() {
                        text_runs.push(TextRun {
                            text,
                            bounds: Rect::new(
                                start_x as f32,
                                y as f32,
                                (end_x - start_x) as f32,
                                1.0,
                            ),
                        });
                    }
                    continue;
                }
                match &mut run {
                    Some((text, _start_x, end_x)) => {
                        text.push(ch);
                        *end_x = x + w;
                    }
                    None => run = Some((ch.to_string(), x, x + w)),
                }
            }
            if let Some((text, start_x, end_x)) = run {
                text_runs.push(TextRun {
                    text,
                    bounds: Rect::new(start_x as f32, y as f32, (end_x - start_x) as f32, 1.0),
                });
            }
        }
        FrameInventory {
            text_runs,
            // Zone bookkeeping, not rasterised output — see the module doc.
            zones: self.backend.zones().to_vec(),
        }
    }

    fn screen_has(&self, needle: &str) -> bool {
        self.screen_contains(needle)
    }

    fn exited(&self) -> bool {
        TuiVtDriver::exited(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
    use crate::types::{Color, WidgetId};
    use crate::Rect as QRect;

    /// Paints one status-bar line verbatim, mirroring
    /// `tui::testing::tests::OneLineApp` so the two drivers' wide-char
    /// handling can be compared on an identical fixture.
    struct OneLineApp {
        text: &'static str,
    }

    impl AppLogic for OneLineApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_status_bar(
                QRect::new(0.0, 0.0, 30.0, 1.0),
                &StatusBar {
                    id: WidgetId::new("status"),
                    left_segments: vec![StatusBarSegment {
                        text: self.text.to_string(),
                        fg: Color::rgb(255, 255, 255),
                        bg: Color::rgb(0, 0, 0),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                },
                None,
                None,
            );
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// `ConformanceDriver` smoke test for the vt100 impl, mirroring
    /// `tui::testing::tests::conformance_driver_new_fixture_scroll_at_and_inventory_round_trip`:
    /// `new_fixture`, `inventory`, and `scroll_at` all round-trip through
    /// the promoted trait with no real terminal involved.
    #[test]
    fn conformance_driver_new_fixture_scroll_at_and_inventory_round_trip() {
        let driver: TuiVtDriver<OneLineApp> = ConformanceDriver::new_fixture(
            OneLineApp {
                text: "你好 world"
            },
            LogicalViewport::new(30, 3),
        );

        let inv = ConformanceDriver::inventory(&driver);
        let texts: Vec<&str> = inv.text_runs().iter().map(|t| t.text.as_str()).collect();
        assert!(
            texts.contains(&"你好") && texts.contains(&"world"),
            "vt100-observed inventory should synthesize a TextRun per whitespace-separated \
             run, wide-char aware: {texts:?} (screen:\n{})",
            driver.screen()
        );

        let mut driver = driver;
        driver.scroll_at("world", 1);
    }

    /// Two adjacent double-width glyphs must be found as one contiguous
    /// span through the real ANSI byte stream too — the vt100-backed twin
    /// of `tui::testing::tests::find_bounds_is_wide_char_aware_for_adjacent_cjk_glyphs`.
    #[test]
    fn find_bounds_is_wide_char_aware_for_adjacent_cjk_glyphs() {
        let driver = TuiVtDriver::new(
            OneLineApp {
                text: "你好 world"
            },
            30,
            3,
        );

        let cjk = driver.find_bounds("你好").expect(
            "adjacent double-width glyphs should match as one span over the vt100 screen too",
        );
        assert_eq!((cjk.x, cjk.width), (0.0, 4.0));

        let world = driver
            .find_bounds("world")
            .expect("world should be found after the CJK run");
        assert_eq!(world.x, cjk.x + cjk.width + 1.0);
    }

    /// `exited` latches once the app returns `Reaction::Exit`, same
    /// contract as `TuiDriver`.
    #[test]
    fn exited_latches_after_reaction_exit() {
        struct QuitOnQ;
        impl AppLogic for QuitOnQ {
            type AreaId = ();
            fn render(&self, _backend: &mut dyn Backend, _area: ()) {}
            fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
                match event {
                    UiEvent::KeyPressed {
                        key: Key::Char('q'),
                        ..
                    } => Reaction::Exit,
                    _ => Reaction::Continue,
                }
            }
        }

        let mut driver = TuiVtDriver::new(QuitOnQ, 10, 3);
        assert!(!driver.exited());
        driver.type_char('q');
        assert!(driver.exited());
    }
}
