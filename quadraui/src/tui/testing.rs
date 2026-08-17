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

/// Re-exported so a downstream test crate can name `Color`/`Modifier` (e.g.
/// `style.modifiers.contains(Modifier::ITALIC)`) without adding its own
/// `ratatui` dependency — it already depends on `quadraui` to get
/// [`TuiDriver`]. Scoped to `tui::testing` rather than the crate root to
/// avoid colliding with [`crate::types::Color`], which is re-exported there
/// already (quadraui#593).
pub use ratatui::style::{Color, Modifier};

use crate::backend::Backend;
use crate::runner::{AppLogic, Reaction};
use crate::shell::{ShellApp, ShellConfig};
use crate::testing::{Anchor, ConformanceDriver, FrameInventory, LogicalViewport, TextRun};
use crate::tui::backend::TuiBackend;
use crate::tui::run::{dispatch_event, render_frame, EventOutcome};
use crate::tui::text::char_cell_width;
use crate::{
    ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, Rect, ScrollDelta, UiEvent, WidgetId,
};

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

/// The style painted onto one rendered cell: foreground, background, and
/// text modifiers (bold/italic/underline/…).
///
/// Mirrors `ratatui::buffer::Cell::style()`'s three fields but drops the
/// `sub_modifier`/`underline_color` ratatui carries for *diffing* two
/// styles against each other — a rendered cell has no "subtracted"
/// modifiers, only the resolved `add_modifier` bits ratatui reports back as
/// [`Modifier`], which is what a driver test wants to assert against
/// (quadraui#593). `fg`/`bg` are ratatui's own [`Color`] (re-exported as
/// [`self::Color`], see [`TuiDriver::style_at`]) rather than
/// `quadraui::types::Color` because that's what the rasterisers actually
/// paint into the buffer — comparing against a theme token means wrapping
/// the token with [`crate::tui::ratatui_color`] first.
///
/// `#[non_exhaustive]`: fields may grow (e.g. `underline_color`, dropped
/// from this first cut — see above) without that being a breaking change
/// for callers. `CellStyle` is only ever returned, never constructed, by
/// consumers, so this only forbids exhaustive destructuring, not field
/// access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CellStyle {
    pub fg: Color,
    pub bg: Color,
    pub modifiers: Modifier,
}

/// Drives an [`AppLogic`] impl headlessly for tests.
///
/// Construct with [`Self::new`] (which runs `setup` + paints the first
/// frame), poke it with [`Self::press`] / [`Self::type_char`] /
/// [`Self::press_named`] / [`Self::click`], and read the rendered grid
/// back with [`Self::screen`] / [`Self::screen_contains`] / [`Self::find`],
/// or its style with [`Self::style_at`] / [`Self::styled_row`].
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

    /// Deliver a [`UiEvent::DoubleClick`] at `(x, y)` directly — the same
    /// event `TuiBackend`'s `DoubleClickDetector` would eventually
    /// synthesise from two close-together `MouseDown`s, but produced here
    /// with no dependence on wall-clock timing (quadraui#592). A test that
    /// fired two `click()`s and relied on the 400ms/1.5-cell detector
    /// window was a race against real time — flaky under load, identical
    /// on the 1st and 1000th call here since no clock is consulted.
    ///
    /// Still routed through [`Self::dispatch`] (backend translation +
    /// `dispatch_event`), not straight to `app.handle`, so it sees the
    /// same pre-processing (e.g. `clear_selection_display`) a real
    /// detector-folded double click would — see the module doc's "No
    /// drift from production" note. `apply_dispatch`/`DoubleClickDetector`
    /// both pass a `DoubleClick` event through unchanged, so this doesn't
    /// risk a second fold.
    pub fn double_click(&mut self, x: f32, y: f32) -> Reaction {
        self.dispatch(UiEvent::DoubleClick {
            widget: None,
            position: Point::new(x, y),
        })
    }

    /// Press-and-release the middle mouse button at `(x, y)`, delivered as
    /// a [`UiEvent::MouseDown`] with [`MouseButton::Middle`]. Shorthand for
    /// [`Self::click_with`] with the default modifiers.
    pub fn middle_click(&mut self, x: f32, y: f32) -> Reaction {
        self.click_with(MouseButton::Middle, Modifiers::default(), x, y)
    }

    /// Press-and-release the right mouse button at `(x, y)`, delivered as
    /// a [`UiEvent::MouseDown`] with [`MouseButton::Right`]. Shorthand for
    /// [`Self::click_with`] with the default modifiers.
    pub fn right_click(&mut self, x: f32, y: f32) -> Reaction {
        self.click_with(MouseButton::Right, Modifiers::default(), x, y)
    }

    /// Click at `(x, y)` with an arbitrary [`MouseButton`] and
    /// [`Modifiers`], delivered as a [`UiEvent::MouseDown`] — the general
    /// form [`Self::click`]/[`Self::middle_click`]/[`Self::right_click`]
    /// build on. Lets a test assert modifier-gated behaviour (e.g.
    /// Ctrl-click) without hand-assembling the event.
    pub fn click_with(
        &mut self,
        button: MouseButton,
        modifiers: Modifiers,
        x: f32,
        y: f32,
    ) -> Reaction {
        self.dispatch(UiEvent::MouseDown {
            widget: None,
            button,
            position: Point::new(x, y),
            modifiers,
        })
    }

    /// Enable or disable double-click folding for subsequent
    /// [`Self::click`]/[`Self::mouse_down`] calls (quadraui#592). Defaults
    /// to `true` — today's behaviour, where two `click()`s landing inside
    /// `TuiBackend`'s `DoubleClickDetector` window (400ms, 1.5 cells) fold
    /// into a single `DoubleClick`. Pass `false` when a test means two
    /// *separate* single clicks — e.g. exercising click-outside-to-dismiss
    /// twice in a row — so it deterministically gets two `MouseDown`s
    /// instead of a result that depends on how much wall-clock time
    /// elapsed between the two `dispatch` calls.
    pub fn set_double_click_folding(&mut self, enabled: bool) {
        self.backend.set_double_click_folding(enabled);
    }

    /// Whether the app has returned [`Reaction::Exit`].
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// The current rendered screen as newline-joined rows.
    ///
    /// Wide-char aware (quadraui#555): strides by each glyph's
    /// [`char_cell_width`], the same way [`Self::row_cells`] does, rather
    /// than one buffer column per iteration. A double-width glyph's
    /// reserved continuation cell is always blank by construction (see
    /// `tui::set_cell_wide`) and never reaches `next` in `Buffer::diff`'s
    /// output, so `TestBackend` never actually writes it — it just
    /// retains whatever was in that column *before* this frame, which for
    /// a freshly-painted glyph is `Cell::default()`'s `symbol()`, `" "`.
    /// Reading column-by-column therefore inserted a phantom space after
    /// every wide glyph that was never really there (a real terminal
    /// shows the glyph occupying both columns with nothing "under" it),
    /// which made `screen_contains`/`screen_has` — the direct backer of
    /// the conformance suite's `assert_screen_has` step — report a needle
    /// spanning a wide glyph as absent even though [`Self::find_bounds`]
    /// (already wide-char aware, quadraui#488) found it painted as one
    /// contiguous run. Striding here brings `screen()` in line with what
    /// `row_cells`/`find_bounds`/`inventory` already saw, and with what
    /// [`crate::tui::vt_testing::TuiVtDriver::screen`]'s vt100-observed
    /// twin reports for the identical content.
    pub fn screen(&self) -> String {
        let buf = self.terminal.backend().buffer();
        let area = buf.area;
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            let mut x = area.left();
            while x < area.right() {
                let symbol = buf[(x, y)].symbol();
                let ch = symbol.chars().next().unwrap_or(' ');
                out.push_str(symbol);
                x += char_cell_width(ch).max(1);
            }
            out.push('\n');
        }
        out
    }

    /// True if any rendered row contains `needle`.
    pub fn screen_contains(&self, needle: &str) -> bool {
        self.screen().contains(needle)
    }

    /// The style a rasteriser painted onto one cell: foreground, background,
    /// and text modifiers (bold/italic/underline/…).
    ///
    /// [`Self::screen`] only surfaces glyphs, so a driver test can prove a
    /// cell exists but not, e.g., that it's the *preview*-tab italic rather
    /// than the permanent-tab plain style (quadraui#593) — both stringify
    /// identically. `style_at` reads the same buffer cell `screen()` does,
    /// just its [`ratatui::buffer::Cell::style`] instead of its symbol.
    ///
    /// `fg`/`bg`/`modifiers` are `ratatui`'s own [`Color`]/[`Modifier`],
    /// re-exported from this module (`quadraui::tui::testing::Modifier`) so
    /// a downstream test crate can assert on them (`style.modifiers
    /// .contains(Modifier::ITALIC)`) without adding its own `ratatui`
    /// dependency — it already depends on `quadraui` to get `TuiDriver`.
    pub fn style_at(&self, x: u16, y: u16) -> Option<CellStyle> {
        let buf = self.terminal.backend().buffer();
        let area = buf.area;
        if x < area.left() || x >= area.right() || y < area.top() || y >= area.bottom() {
            return None;
        }
        let cell = &buf[(x, y)];
        Some(CellStyle {
            fg: cell.fg,
            bg: cell.bg,
            modifiers: cell.modifier,
        })
    }

    /// [`Self::style_at`] for every cell of row `y`, left to right — one
    /// entry per cell, so a test can assert "every cell of the tab label is
    /// italic" without indexing arithmetic. Empty if `y` is out of range.
    ///
    /// `styled_row(y).len()` equals the buffer width (`area.width`) for any
    /// in-range `y` — always one entry per cell, unlike [`Self::screen`],
    /// whose row length before the trailing `\n` is only the same count for
    /// buffers with no wide characters: `screen()` concatenates each cell's
    /// `symbol()`, so a double-width glyph's continuation cell contributes
    /// zero chars there, while `styled_row` still emits an entry for it
    /// (its char defaults to `' '`, since a continuation cell's `symbol()`
    /// is empty).
    pub fn styled_row(&self, y: u16) -> Vec<(char, CellStyle)> {
        let buf = self.terminal.backend().buffer();
        let area = buf.area;
        if y < area.top() || y >= area.bottom() {
            return Vec::new();
        }
        (area.left()..area.right())
            .map(|x| {
                let cell = &buf[(x, y)];
                let ch = cell.symbol().chars().next().unwrap_or(' ');
                (
                    ch,
                    CellStyle {
                        fg: cell.fg,
                        bg: cell.bg,
                        modifiers: cell.modifier,
                    },
                )
            })
            .collect()
    }

    /// Access the app state for test assertions.
    ///
    /// Useful when the test app records side-effects (e.g. last copied text,
    /// selection changes) that would otherwise require screen-scraping.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// Mutable access to the app state for tests that need to poke state
    /// directly rather than through a scripted [`UiEvent`] — e.g. asserting
    /// a render-time invariant (like editor cursor placement) across a
    /// state change with no dedicated event/handler of its own.
    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    /// Access the backend for test assertions (e.g. active selection state,
    /// drag state).
    pub fn backend(&self) -> &TuiBackend {
        &self.backend
    }

    /// The `(x, y)` position `Terminal::draw` last applied to the
    /// underlying `TestBackend` via `Frame::set_cursor_position` —
    /// reflecting [`super::run::render_frame`]'s `take_last_cursor_position`
    /// handoff from `TuiBackend::draw_editor` (quadraui#466).
    ///
    /// Note `TestBackend` tracks only the last-set position, not whether
    /// the cursor is currently hidden — so this always returns the most
    /// recent position ever applied, even on a later frame that painted no
    /// editor and would hide the real terminal cursor. Fine for asserting
    /// "the editor's cursor position reached the Frame", not for asserting
    /// hide/show transitions.
    pub fn terminal_cursor_position(&mut self) -> Option<(u16, u16)> {
        self.terminal.get_cursor_position().ok().map(|p| (p.x, p.y))
    }

    /// This row's cells as `(char, cell_x, cell_width)` triples, in
    /// left-to-right order — the shared scan [`Self::find_bounds`] and
    /// [`ConformanceDriver::inventory`] both walk.
    ///
    /// Skips the reserved continuation cell(s) a double-width glyph's
    /// rendering leaves behind: ratatui resets them to a blank cell whose
    /// `symbol()` reads back as `" "`, indistinguishable from a real space
    /// by content alone, so the *width* of the character just placed (via
    /// [`char_cell_width`]), not the following cell's symbol, drives how
    /// far `x` advances. Without this, two adjacent double-width
    /// characters (e.g. CJK) get a spurious blank cell wedged between them
    /// in the reconstructed row, and a needle spanning them would never
    /// match (quadraui#488).
    fn row_cells(&self, y: u16) -> Vec<(char, u16, u16)> {
        let buf = self.terminal.backend().buffer();
        let area = buf.area;
        let mut cells = Vec::new();
        let mut x = area.left();
        while x < area.right() {
            let symbol = buf[(x, y)].symbol();
            let ch = symbol.chars().next().unwrap_or(' ');
            let w = char_cell_width(ch).max(1);
            cells.push((ch, x, w));
            x += w;
        }
        cells
    }

    /// Cell bounds of the first row containing `needle`, wide-char aware
    /// (see [`Self::row_cells`]): a needle spanning two double-width
    /// glyphs (CJK, most emoji) matches and reports the full cell span,
    /// not just its first character's cell.
    pub fn find_bounds(&self, needle: &str) -> Option<Rect> {
        let needle: Vec<char> = needle.chars().collect();
        if needle.is_empty() {
            return None;
        }
        let area = self.terminal.backend().buffer().area;
        for y in area.top()..area.bottom() {
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
    /// the matched span's *first* cell — preserved from the pre-#488
    /// behaviour (a click there always lands inside the match regardless
    /// of span width) even though [`Self::find_bounds`] now reports the
    /// full wide-char-aware span. Callers that want the full span's
    /// center use `find_bounds` directly (e.g.
    /// [`ConformanceDriver::click_text_at`]'s `Anchor::Center`).
    pub fn find(&self, needle: &str) -> Option<(f32, f32)> {
        self.find_bounds(needle)
            .map(|b| (b.x + 0.5, b.y + b.height / 2.0))
    }

    /// Cell coordinates of tab `tab_idx`'s center in the tab bar `bar`, for
    /// [`Self::click`] to land on that specific tab (quadraui#594).
    ///
    /// `find` can't do this job: every tab paints the same chrome (and a
    /// closed/active tab's label may repeat elsewhere on screen), so there
    /// is no needle that disambiguates "tab 3" from "tab 0". This instead
    /// resolves against the [`crate::TabBarLayout`] `TuiBackend::draw_tab_bar`
    /// cached the last time `bar` painted — `None` if `bar` didn't paint
    /// this frame, or if `tab_idx` is scrolled out of view behind the
    /// bar's `scroll_offset`.
    pub fn tab_center(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)> {
        let (rect, layout) = self.backend.cached_tab_bar_layout(bar)?;
        let (cx, cy) = layout.tab_center(tab_idx)?;
        Some((rect.x + cx, rect.y + cy))
    }

    /// Cell coordinates of tab `tab_idx`'s close-button center in the tab
    /// bar `bar` — the `×`/`●` glyph every tab shares, so [`Self::find`]
    /// can't target one tab's close button over another's (quadraui#594).
    ///
    /// `None` for the same reasons as [`Self::tab_center`], plus: the tab
    /// drew no close button this frame (`is_closable: false` on the tab,
    /// or `show_tab_close: false` on the bar).
    pub fn tab_close_center(&self, bar: &WidgetId, tab_idx: usize) -> Option<(f32, f32)> {
        let (rect, layout) = self.backend.cached_tab_bar_layout(bar)?;
        let (cx, cy) = layout.tab_close_center(tab_idx)?;
        Some((rect.x + cx, rect.y + cy))
    }
}

impl<A: AppLogic> ConformanceDriver for TuiDriver<A> {
    type App = A;

    fn new_fixture(app: Self::App, viewport: LogicalViewport) -> Self {
        // TUI's native unit *is* the cell, so a `LogicalViewport` maps
        // straight through — no char_width/line_height scaling needed
        // (contrast `GtkDriver::new_fixture`, a pixel backend).
        TuiDriver::new(app, viewport.cols as u16, viewport.rows as u16)
    }

    fn backend_caps(&self) -> crate::BackendCaps {
        // Straight off the real `TuiBackend` this driver wraps — never a
        // re-statement (quadraui#492).
        crate::Backend::backend_caps(&self.backend)
    }

    fn press_named(&mut self, key: NamedKey) {
        TuiDriver::press_named(self, key);
    }

    fn type_char(&mut self, c: char) {
        TuiDriver::type_char(self, c);
    }

    fn ctrl_char(&mut self, c: char) {
        TuiDriver::ctrl_char(self, c);
    }

    fn click_text_at(&mut self, needle: &str, at: Anchor) {
        let bounds = self
            .find_bounds(needle)
            .unwrap_or_else(|| panic!("TuiDriver: {needle:?} not painted:\n{}", self.screen()));
        let y = bounds.y + bounds.height / 2.0;
        let x = match at {
            Anchor::Center => bounds.x + bounds.width / 2.0,
            // Half a cell in from each edge — the outermost cell's
            // center — so the click reliably lands inside the run even
            // for a single-cell-wide match.
            Anchor::LeftEdge => bounds.x + 0.5,
            Anchor::RightEdge => bounds.x + bounds.width - 0.5,
        };
        self.click(x, y);
    }

    fn drag_text(&mut self, from: &str, to: &str) {
        let (x0, y0) = self
            .find(from)
            .unwrap_or_else(|| panic!("TuiDriver: {from:?} not painted:\n{}", self.screen()));
        let (x1, y1) = self
            .find(to)
            .unwrap_or_else(|| panic!("TuiDriver: {to:?} not painted:\n{}", self.screen()));
        self.drag(x0, y0, x1, y1);
    }

    fn scroll_at(&mut self, needle: &str, lines: i32) {
        let (x, y) = self
            .find(needle)
            .unwrap_or_else(|| panic!("TuiDriver: {needle:?} not painted:\n{}", self.screen()));
        let line_height = self.backend.line_height();
        self.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, lines as f32 * line_height),
            position: Point::new(x, y),
        });
    }

    fn inventory(&self) -> FrameInventory {
        let area = self.terminal.backend().buffer().area;
        let mut text_runs = Vec::new();
        for y in area.top()..area.bottom() {
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
            // Zones registered this frame via `Backend::register_zone` —
            // currently the shell-chrome zones `AppShell::render` records
            // (activity-bar items, sidebar header/content, status bar,
            // ...). Primitives that don't yet call `register_zone`
            // contribute no zone (quadraui#490).
            zones: self.backend.zones().to_vec(),
        }
    }

    fn screen_has(&self, needle: &str) -> bool {
        self.screen_contains(needle)
    }

    fn exited(&self) -> bool {
        TuiDriver::exited(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Viewport;
    use std::cell::{Cell, RefCell};
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

    // ─── quadraui#488 ────────────────────────────────────────────────────

    use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
    use crate::types::{Color, WidgetId};

    /// Paints one status-bar line whose text is given verbatim (no
    /// automatic padding), so a test can control exactly which glyphs are
    /// adjacent — needed to exercise the double-width continuation-cell
    /// case `find`/`find_bounds` must handle correctly.
    struct OneLineApp {
        text: &'static str,
    }

    impl AppLogic for OneLineApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_status_bar(
                Rect::new(0.0, 0.0, 30.0, 1.0),
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

    /// Regression guard for quadraui#488's wide-char `find`/`find_bounds`
    /// fix: two adjacent double-width (CJK) glyphs must be found as one
    /// 4-cell-wide span, not silently unmatched because ratatui wedges a
    /// blank continuation cell between them that isn't a real space.
    #[test]
    fn find_bounds_is_wide_char_aware_for_adjacent_cjk_glyphs() {
        let driver = TuiDriver::new(
            OneLineApp {
                text: "你好 world"
            },
            30,
            3,
        );

        let cjk = driver.find_bounds("你好").expect(
            "adjacent double-width glyphs should match as one span, not be split by a \
             spurious blank continuation cell",
        );
        assert_eq!(
            (cjk.x, cjk.width),
            (0.0, 4.0),
            "two double-width glyphs occupy 4 cells total"
        );

        // `find` still returns the first cell's center (pre-#488
        // behaviour preserved — see its doc comment), not the span's
        // midpoint.
        let (x, y) = driver.find("你好").expect("find should also locate it");
        assert_eq!((x, y), (0.5, 0.5));

        // "world" is offset past the 4-cell CJK run and the separating
        // space — proves the continuation-cell fix didn't just get the
        // *first* match right by accident.
        let world = driver
            .find_bounds("world")
            .expect("world should be found after the CJK run");
        assert_eq!(world.x, cjk.x + cjk.width + 1.0);
    }

    /// Regression guard for quadraui#555's `screen()` fix: adjacent
    /// double-width glyphs must read back as a contiguous substring, with
    /// no phantom space inserted for the wide glyph's (always-blank)
    /// continuation cell — the same needle `find_bounds` above already
    /// located as one 4-cell span. Before this fix `screen()` walked one
    /// buffer column per iteration, so `screen_contains`/`screen_has` (the
    /// direct backer of the conformance suite's `assert_screen_has` step)
    /// disagreed with `find_bounds`/`inventory` on the exact same frame —
    /// discovered via `tests/conformance/scenarios/tabs/
    /// tabbar.wide_label_text_fidelity.scn.json`, which runs this same
    /// class of content against both the `TestBackend` and the vt100
    /// observer (`quadraui::tui::vt_testing::TuiVtDriver`) and caught the
    /// two disagreeing on a needle both had, in fact, painted.
    #[test]
    fn screen_is_wide_char_aware_for_adjacent_cjk_glyphs() {
        let driver = TuiDriver::new(
            OneLineApp {
                text: "你好 world"
            },
            30,
            3,
        );

        assert!(
            driver.screen_contains("你好"),
            "screen() must not insert a phantom space inside a contiguous \
             double-width run:\n{}",
            driver.screen()
        );
        assert!(
            driver.screen_contains("你好 world"),
            "the whole line must read back exactly as painted:\n{}",
            driver.screen()
        );
    }

    /// `ConformanceDriver` smoke test for the TUI impl: `new_fixture` (the
    /// `LogicalViewport`-aligned constructor), `scroll_at`, and
    /// `inventory` all round-trip through the promoted trait.
    #[test]
    fn conformance_driver_new_fixture_scroll_at_and_inventory_round_trip() {
        let driver: TuiDriver<OneLineApp> = ConformanceDriver::new_fixture(
            OneLineApp {
                text: "你好 world"
            },
            LogicalViewport::new(30, 3),
        );
        assert_eq!(
            (
                driver.backend().viewport().width,
                driver.backend().viewport().height
            ),
            (30.0, 3.0),
            "new_fixture's LogicalViewport should map straight through to TUI cells"
        );

        let inv = ConformanceDriver::inventory(&driver);
        let texts: Vec<&str> = inv.text_runs().iter().map(|t| t.text.as_str()).collect();
        assert!(
            texts.contains(&"你好") && texts.contains(&"world"),
            "inventory() should synthesize a TextRun per whitespace-separated \
             run on the cell grid, wide-char aware: {texts:?}"
        );

        // scroll_at must not panic when the target text exists — the
        // dispatched Scroll event's downstream handling is exercised by
        // primitive-level tests; this just confirms the driver-level
        // find-then-dispatch plumbing works.
        let mut driver = driver;
        driver.scroll_at("world", 1);
    }

    // ─── quadraui#592 ────────────────────────────────────────────────────

    /// Records every `UiEvent` the app's `handle()` receives, verbatim —
    /// lets a test assert exactly what reached the app (button, modifiers,
    /// `DoubleClick` vs `MouseDown`) without screen-scraping.
    #[derive(Default)]
    struct EventRecorder {
        events: Rc<RefCell<Vec<UiEvent>>>,
    }

    impl AppLogic for EventRecorder {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            self.events.borrow_mut().push(event);
            Reaction::Continue
        }
    }

    /// `double_click` must deliver exactly one `UiEvent::DoubleClick` at
    /// the given position, with no prior `MouseDown`, and do so
    /// identically whether it's the 1st or 1000th call — the deterministic
    /// alternative to two `click()`s racing `DoubleClickDetector`'s 400ms
    /// wall-clock window (quadraui#592).
    #[test]
    fn double_click_is_deterministic_across_many_calls() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let app = EventRecorder {
            events: events.clone(),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        for i in 0..1000 {
            events.borrow_mut().clear();
            driver.double_click(3.0, 2.0);
            let recorded = events.borrow();
            assert_eq!(
                recorded.len(),
                1,
                "call {i}: double_click should deliver exactly one event, got {recorded:?}"
            );
            assert_eq!(
                recorded[0],
                UiEvent::DoubleClick {
                    widget: None,
                    position: Point::new(3.0, 2.0),
                },
                "call {i}: expected a single DoubleClick with no prior MouseDown"
            );
        }
    }

    /// `middle_click` delivers `MouseDown { button: MouseButton::Middle, .. }`.
    #[test]
    fn middle_click_delivers_middle_mouse_down() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let app = EventRecorder {
            events: events.clone(),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        driver.middle_click(4.0, 1.0);

        let recorded = events.borrow();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0],
            UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Middle,
                position: Point::new(4.0, 1.0),
                modifiers: Modifiers::default(),
            }
        );
    }

    /// `right_click` delivers `MouseDown { button: MouseButton::Right, .. }`.
    #[test]
    fn right_click_delivers_right_mouse_down() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let app = EventRecorder {
            events: events.clone(),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        driver.right_click(4.0, 1.0);

        let recorded = events.borrow();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0],
            UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Right,
                position: Point::new(4.0, 1.0),
                modifiers: Modifiers::default(),
            }
        );
    }

    /// `click_with` threads an arbitrary button + modifiers through to the
    /// app, e.g. a Ctrl-held left click.
    #[test]
    fn click_with_delivers_button_and_modifiers() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let app = EventRecorder {
            events: events.clone(),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        driver.click_with(MouseButton::Left, ctrl, 2.0, 3.0);

        let recorded = events.borrow();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0],
            UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(2.0, 3.0),
                modifiers: ctrl,
            }
        );
    }

    /// With folding disabled, two `click()` calls in immediate succession
    /// (well inside `DoubleClickDetector`'s 400ms/1.5-cell window) must
    /// deliver two `MouseDown` events and zero `DoubleClick` — proving the
    /// toggle actually suppresses folding rather than just being ignored.
    #[test]
    fn folding_disabled_keeps_two_clicks_separate() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let app = EventRecorder {
            events: events.clone(),
        };
        let mut driver = TuiDriver::new(app, 20, 5);
        driver.set_double_click_folding(false);

        driver.click(5.0, 5.0);
        driver.click(5.0, 5.0);

        let recorded = events.borrow();
        let mouse_downs = recorded
            .iter()
            .filter(|e| matches!(e, UiEvent::MouseDown { .. }))
            .count();
        let double_clicks = recorded
            .iter()
            .filter(|e| matches!(e, UiEvent::DoubleClick { .. }))
            .count();
        assert_eq!(
            (mouse_downs, double_clicks),
            (2, 0),
            "folding disabled should yield two MouseDowns and no DoubleClick, got {recorded:?}"
        );
    }

    /// Sanity check for the *default* (folding enabled) behaviour this
    /// issue must not regress: two `click()`s at the same position can
    /// still fold into a `DoubleClick` via the real detector when a test
    /// wants that (contrast the deterministic `double_click` helper above,
    /// which needs no timing at all).
    #[test]
    fn folding_enabled_by_default_still_folds_two_clicks() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let app = EventRecorder {
            events: events.clone(),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        driver.click(5.0, 5.0);
        driver.click(5.0, 5.0);

        let recorded = events.borrow();
        let double_clicks = recorded
            .iter()
            .filter(|e| matches!(e, UiEvent::DoubleClick { .. }))
            .count();
        assert_eq!(
            double_clicks, 1,
            "default folding should still fold two same-position clicks: {recorded:?}"
        );
    }

    // ─── quadraui#593 ────────────────────────────────────────────────────

    use crate::primitives::tab_bar::{TabBar, TabItem};
    use crate::theme::Theme;
    use crate::types::WidgetId as QWidgetId;

    /// Paints a two-tab `TabBar` — one permanent, one preview — so a test
    /// can assert `style_at`/`styled_row` distinguish them. Mirrors the
    /// `OneLineApp` pattern above but drives `draw_tab_bar` instead of
    /// `draw_status_bar`.
    struct TabBarApp {
        theme: Theme,
    }

    impl AppLogic for TabBarApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.set_theme(self.theme);
            backend.draw_tab_bar(
                Rect::new(0.0, 0.0, 20.0, 1.0),
                &TabBar {
                    id: QWidgetId::new("tabs"),
                    tabs: vec![
                        TabItem {
                            label: "perm".to_string(),
                            is_active: false,
                            is_dirty: false,
                            is_preview: false,
                            is_closable: false,
                        },
                        TabItem {
                            label: "prev".to_string(),
                            is_active: false,
                            is_dirty: false,
                            is_preview: true,
                            is_closable: false,
                        },
                    ],
                    scroll_offset: 0,
                    right_segments: vec![],
                    active_accent: None,
                    show_tab_close: false,
                    compact: false,
                },
                None,
            );
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// Acceptance criterion: `style_at` distinguishes a preview tab from a
    /// permanent one by `Modifier::ITALIC` and by the theme's
    /// `tab_preview_inactive_fg` vs `tab_inactive_fg` foreground — neither
    /// of which `screen()` can see, since both tabs stringify identically
    /// (quadraui#593).
    #[test]
    fn style_at_distinguishes_preview_tab_from_permanent_tab() {
        let theme = Theme::default();
        let driver = TuiDriver::new(TabBarApp { theme }, 20, 1);

        // "perm" occupies cells 0..4, "prev" occupies cells 4..8 (no close
        // buttons reserved — `is_closable: false` on both tabs); the rest
        // of the 20-cell bar is blank fill.
        assert!(driver.screen_contains("permprev"));

        let perm_style = driver
            .style_at(0, 0)
            .expect("(0, 0) is inside the 20x1 buffer");
        let preview_style = driver
            .style_at(4, 0)
            .expect("(4, 0) is inside the 20x1 buffer");

        assert!(
            !perm_style.modifiers.contains(Modifier::ITALIC),
            "permanent tab must not be italic: {perm_style:?}"
        );
        assert!(
            preview_style.modifiers.contains(Modifier::ITALIC),
            "preview tab must be italic: {preview_style:?}"
        );

        assert_eq!(
            perm_style.fg,
            crate::tui::ratatui_color(theme.tab_inactive_fg),
            "permanent tab's fg should be the theme's tab_inactive_fg token"
        );
        assert_eq!(
            preview_style.fg,
            crate::tui::ratatui_color(theme.tab_preview_inactive_fg),
            "preview tab's fg should be the theme's tab_preview_inactive_fg token"
        );
    }

    /// `style_at` outside the buffer returns `None` rather than panicking
    /// (acceptance criterion) — both past the right/bottom edge and on the
    /// exact boundary row/column, which is the first out-of-range value.
    #[test]
    fn style_at_out_of_bounds_returns_none() {
        let driver = TuiDriver::new(
            TabBarApp {
                theme: Theme::default(),
            },
            20,
            1,
        );

        assert!(
            driver.style_at(20, 0).is_none(),
            "x == width is out of range"
        );
        assert!(
            driver.style_at(0, 1).is_none(),
            "y == height is out of range"
        );
        assert!(
            driver.style_at(100, 100).is_none(),
            "far outside is out of range"
        );
        assert!(
            driver.style_at(19, 0).is_some(),
            "x == width - 1 is the last valid column"
        );
    }

    /// `styled_row(y).len()` equals the buffer width for any in-range `y`
    /// (acceptance criterion), and is empty for an out-of-range row rather
    /// than panicking.
    #[test]
    fn styled_row_length_matches_buffer_width() {
        let driver = TuiDriver::new(
            TabBarApp {
                theme: Theme::default(),
            },
            20,
            1,
        );

        let row = driver.styled_row(0);
        assert_eq!(row.len(), 20, "row length should equal buffer width");
        assert_eq!(
            row.iter().map(|(ch, _)| *ch).collect::<String>(),
            "permprev            "[..20],
            "styled_row's chars should match screen()'s glyphs left to right"
        );
        // Same per-cell distinction as style_at, reachable via styled_row.
        assert!(!row[0].1.modifiers.contains(Modifier::ITALIC));
        assert!(row[4].1.modifiers.contains(Modifier::ITALIC));

        assert!(
            driver.styled_row(1).is_empty(),
            "out-of-range row should be empty, not panic"
        );
    }

    // ─── quadraui#594 ────────────────────────────────────────────────────

    /// Three closable tabs on a 20-cell bar. `handle()` hit-tests raw
    /// `MouseDown` clicks against the *live*, non-painting
    /// [`crate::Backend::tab_bar_layout`] query (mirrors the "cached
    /// layout hit-test pattern" `gtk/testing.rs`'s `ToggleStatusBarApp`
    /// uses) — close regions win over tab bodies, matching
    /// `TabBarLayout::hit_regions`' close-before-body ordering. Exercises
    /// `TuiDriver::tab_center`/`tab_close_center` end-to-end: the driver
    /// locates a specific tab's click target with no hardcoded
    /// coordinates, and the resulting click reaches the app through the
    /// ordinary event path.
    struct InteractiveTabBarApp {
        active: usize,
        closed_idx: Option<usize>,
    }

    impl InteractiveTabBarApp {
        const RECT: Rect = Rect::new(0.0, 0.0, 20.0, 1.0);

        fn bar(&self) -> TabBar {
            TabBar {
                id: QWidgetId::new("tabs"),
                tabs: (0..3)
                    .map(|i| TabItem {
                        label: format!("tab{i}"),
                        is_active: i == self.active,
                        is_dirty: false,
                        is_preview: false,
                        is_closable: true,
                    })
                    .collect(),
                scroll_offset: 0,
                right_segments: vec![],
                active_accent: None,
                show_tab_close: true,
                compact: false,
            }
        }
    }

    impl AppLogic for InteractiveTabBarApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_tab_bar(Self::RECT, &self.bar(), None);
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            let UiEvent::MouseDown { position, .. } = event else {
                return Reaction::Continue;
            };
            let hits = backend.tab_bar_layout(Self::RECT, &self.bar());
            let x = position.x as f64;
            // Close regions before tab bodies — mirrors
            // `TabBarLayout::hit_regions`' ordering (close-before-body).
            for (i, close) in hits.close_bounds.iter().enumerate() {
                if let Some((sx, ex)) = close {
                    if x >= *sx && x < *ex {
                        self.closed_idx = Some(i);
                        return Reaction::Redraw;
                    }
                }
            }
            for (i, (sx, ex)) in hits.slot_positions.iter().enumerate() {
                if x >= *sx && x < *ex {
                    self.active = i;
                    return Reaction::Redraw;
                }
            }
            Reaction::Continue
        }
    }

    /// Acceptance criterion: with a 3-tab bar rendered,
    /// `driver.click(tab_center(&bar, 1))` activates tab 1.
    #[test]
    fn tab_center_click_activates_target_tab() {
        let app = InteractiveTabBarApp {
            active: 0,
            closed_idx: None,
        };
        let mut driver = TuiDriver::new(app, 20, 1);
        let bar_id = QWidgetId::new("tabs");

        assert_eq!(driver.app().active, 0);
        let (x, y) = driver
            .tab_center(&bar_id, 1)
            .expect("tab 1 should be visible on a 20-cell bar with 3 short tabs");
        driver.click(x, y);

        assert_eq!(
            driver.app().active,
            1,
            "clicking tab 1's center should activate it"
        );
    }

    /// Acceptance criterion: `driver.click(tab_close_center(&bar, 1))`
    /// closes tab 1 and does **not** merely activate it.
    #[test]
    fn tab_close_center_click_closes_target_tab_not_just_activates() {
        let app = InteractiveTabBarApp {
            active: 0,
            closed_idx: None,
        };
        let mut driver = TuiDriver::new(app, 20, 1);
        let bar_id = QWidgetId::new("tabs");

        let (x, y) = driver
            .tab_close_center(&bar_id, 1)
            .expect("tab 1 is closable and should have a close button");
        driver.click(x, y);

        assert_eq!(
            driver.app().closed_idx,
            Some(1),
            "clicking tab 1's close button should close it"
        );
        assert_ne!(
            driver.app().active,
            1,
            "closing tab 1 must not merely activate it — active should stay at its prior value"
        );
    }

    /// Acceptance criterion: `tab_close_center` returns `None` for a tab
    /// rendered with `is_closable: false`.
    #[test]
    fn tab_close_center_none_when_tab_not_closable() {
        struct NonClosableTabBarApp;

        impl AppLogic for NonClosableTabBarApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                backend.draw_tab_bar(
                    Rect::new(0.0, 0.0, 20.0, 1.0),
                    &TabBar {
                        id: QWidgetId::new("tabs"),
                        tabs: vec![TabItem {
                            label: "a".to_string(),
                            is_active: true,
                            is_dirty: false,
                            is_preview: false,
                            is_closable: false,
                        }],
                        scroll_offset: 0,
                        right_segments: vec![],
                        active_accent: None,
                        show_tab_close: true,
                        compact: false,
                    },
                    None,
                );
            }

            fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
                Reaction::Continue
            }
        }

        let driver = TuiDriver::new(NonClosableTabBarApp, 20, 1);
        let bar_id = QWidgetId::new("tabs");

        assert!(
            driver.tab_center(&bar_id, 0).is_some(),
            "the tab itself is visible"
        );
        assert!(
            driver.tab_close_center(&bar_id, 0).is_none(),
            "a non-closable tab drew no close button, so its center must be None"
        );
    }

    /// Acceptance criterion: `tab_center` returns `None` for a tab hidden
    /// behind the bar's `scroll_offset`.
    #[test]
    fn tab_center_none_when_scrolled_out_of_view() {
        struct ScrolledTabBarApp;

        impl AppLogic for ScrolledTabBarApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                backend.draw_tab_bar(
                    Rect::new(0.0, 0.0, 5.0, 1.0),
                    &TabBar {
                        id: QWidgetId::new("tabs"),
                        tabs: vec![
                            TabItem {
                                label: "a".to_string(),
                                is_active: false,
                                is_dirty: false,
                                is_preview: false,
                                is_closable: true,
                            },
                            TabItem {
                                label: "b".to_string(),
                                is_active: true,
                                is_dirty: false,
                                is_preview: false,
                                is_closable: true,
                            },
                            TabItem {
                                label: "c".to_string(),
                                is_active: false,
                                is_dirty: false,
                                is_preview: false,
                                is_closable: true,
                            },
                        ],
                        // Bar is far too narrow (5 cells) for all 3 tabs
                        // (each ~3 cells with a close button); TUI honours
                        // this caller-supplied offset directly rather than
                        // computing its own (no scroll arrows in TUI).
                        scroll_offset: 1,
                        right_segments: vec![],
                        active_accent: None,
                        show_tab_close: true,
                        compact: false,
                    },
                    None,
                );
            }

            fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
                Reaction::Continue
            }
        }

        let driver = TuiDriver::new(ScrolledTabBarApp, 5, 1);
        let bar_id = QWidgetId::new("tabs");

        assert!(
            driver.tab_center(&bar_id, 0).is_none(),
            "tab 0 is scrolled out of view behind scroll_offset=1"
        );
        assert!(driver.tab_close_center(&bar_id, 0).is_none());
        assert!(
            driver.tab_center(&bar_id, 1).is_some(),
            "tab 1 (the scroll target) should still be visible"
        );
    }
}
