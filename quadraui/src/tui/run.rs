//! TUI runner — drives a [`crate::AppLogic`] implementation against
//! [`TuiBackend`].
//!
//! The runner absorbs every per-app-but-not-app-logic boilerplate
//! piece:
//! - Terminal raw-mode + alternate screen + mouse + bracketed-paste
//!   setup / teardown.
//! - Best-effort kitty keyboard-protocol push (REPORT_ALL_KEYS_AS_ESCAPE_CODES
//!   so Ctrl+Shift+L is unambiguous from Ctrl+L), with the outcome exposed
//!   via [`crate::backend::BackendCaps::kitty_keyboard`] (quadraui#827) so
//!   an app can tell whether it actually worked instead of finding out by
//!   a gesture silently never firing.
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

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
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
use crate::tui::color::DepthLimitedBackend;
use crate::UiEvent;

/// The concrete backend the live runner paints through: a real
/// `CrosstermBackend` over stdout, wrapped in [`DepthLimitedBackend`] so
/// every frame's SGR output is quantised to the terminal's actual colour
/// depth (quadraui#826) — [`ColorDepth::TrueColor`] making that wrapper a
/// no-op pass-through for the common truecolor case.
///
/// [`ColorDepth::TrueColor`]: crate::backend::ColorDepth::TrueColor
type LiveBackend = DepthLimitedBackend<CrosstermBackend<io::Stdout>>;

/// Idle-poll ceiling (quadraui#832) — see
/// [`crate::runtime::IDLE_POLL_CEILING`]'s doc for the full rationale.
/// Before #832 this was a fixed 16ms (≈60fps) poll *every* iteration
/// regardless of whether anything was scheduled; it's now only the upper
/// bound the loop falls back to when nothing has called
/// [`Backend::request_frame_in`] (directly, or via
/// [`crate::runner::Reaction::RedrawAfter`]) — a scheduled frame shortens
/// the actual `wait_events` timeout to just the remaining time until its
/// deadline.
const POLL_TIMEOUT_CEILING: Duration = crate::runtime::IDLE_POLL_CEILING;

/// Runtime configuration for [`run_with`]. `Default` matches [`run`]'s
/// previously-hardcoded behaviour, so `run_with(app, RunConfig::default())`
/// and `run(app)` behave identically.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RunConfig {
    /// Whether to negotiate mouse reporting with the terminal
    /// (`EnableMouseCapture`/`DisableMouseCapture`).
    ///
    /// Defaults to `true`. Set to `false` for **`no-mouse` mode**
    /// (quadraui#828): a host where mouse capture is refused or
    /// unavailable (e.g. some multiplexers, restrictive SSH sessions, a
    /// screen reader driving the terminal) never gets the capture escape
    /// sequences at all — instead of getting them and then discarding
    /// whatever the terminal sends back. An app can read the live
    /// answer via [`crate::tui::backend::TuiBackend::mouse_enabled`]
    /// (kept separate from [`crate::backend::BackendCaps::mouse`], which
    /// stays a static per-backend-type fact — see that accessor's doc).
    /// Every Tier-1 gesture in this crate's own examples has a key path
    /// that works with this flag off — see
    /// `tests/conformance/scenarios/**/*_keyboard.scn.json` — so turning
    /// it off costs an app nothing that a conformant `AppLogic` needs.
    pub mouse: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self { mouse: true }
    }
}

impl RunConfig {
    /// Convenience constructor for **`no-mouse` mode** (quadraui#828):
    /// equivalent to `RunConfig { mouse: false, ..Default::default() }`.
    ///
    /// `RunConfig` is `#[non_exhaustive]` so future fields can be added
    /// without breaking downstream callers — but that same attribute means
    /// a struct-literal expression (even one using `..Default::default()`)
    /// is rejected by rustc (`E0639`) from *outside* this crate. Without
    /// this constructor, no consumer could actually build a non-default
    /// `RunConfig` at all: the field is `pub`, but the type would be
    /// write-only from any caller that isn't `quadraui` itself. This method
    /// is the one supported way in.
    pub fn no_mouse() -> Self {
        Self {
            mouse: false,
            ..Default::default()
        }
    }
}

/// Drive `app` to completion in a TUI environment, using the default
/// [`RunConfig`] (mouse capture enabled). See [`run_with`] for a version
/// that takes an explicit config, e.g. to opt into `no-mouse` mode.
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
pub fn run<A: AppLogic>(app: A) -> io::Result<()> {
    run_with(app, RunConfig::default())
}

/// Like [`run`], but with an explicit [`RunConfig`] — the entry point for
/// **`no-mouse` mode** ([`RunConfig::no_mouse`], quadraui#828). See
/// `examples/tui_no_mouse.rs` for a runnable demo and
/// `tests/tui_pty_smoke.rs`'s `no_mouse` module for the black-box proof
/// that mouse capture is actually withheld over a real pty.
pub fn run_with<A: AppLogic>(app: A, config: RunConfig) -> io::Result<()> {
    let mut runner = TuiRunner::new_with(app, config)?;

    // Run the app inside `catch_unwind` so a panic in app code doesn't
    // leave the terminal in a broken state. `runner` is *borrowed* into
    // the closure below, not moved into it, so it's still here afterwards
    // regardless of whether the closure returned or panicked — that's
    // what lets the explicit `runner.finish()` below run on every exit
    // path, panic included.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
        match runner.pump(POLL_TIMEOUT_CEILING)? {
            StepOutcome::Continue => {}
            StepOutcome::Exited => return Ok(()),
        }
    }));

    // Idempotent (see `TuiRunner::finish`): a no-op if the loop above
    // already exited cleanly and tore the terminal down itself on its way
    // out; the safety net for a panic, which leaves `runner.finished`
    // false.
    runner.finish();

    match result {
        Ok(io_result) => io_result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Terminal state produced by [`setup_terminal`] — shared by the blocking
/// [`run_with`] and non-blocking [`TuiRunner`] (issue #1100), so the
/// negotiation (raw mode, alternate screen, mouse capture, bracketed
/// paste, kitty keyboard protocol, SGR-Pixels mouse mode) can't drift
/// between the two.
struct TerminalSetup {
    terminal: Rc<RefCell<Terminal<LiveBackend>>>,
    backend: TuiBackend,
    kbd_enhanced: bool,
    sgr_pixel_mouse: bool,
}

/// Negotiate the real terminal (raw mode, alternate screen, mouse
/// capture, bracketed paste, best-effort kitty keyboard protocol,
/// best-effort SGR-Pixels mouse mode) and construct the live
/// [`TuiBackend`] and ratatui [`Terminal`]. Extracted out of [`run_with`]
/// (issue #1100) — see [`TerminalSetup`]'s doc.
fn setup_terminal(config: RunConfig) -> io::Result<TerminalSetup> {
    use ratatui::crossterm::event::{EnableBracketedPaste, EnableMouseCapture};

    // ── Terminal setup ──────────────────────────────────────────
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if config.mouse {
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
    } else {
        // `no-mouse` mode: never negotiate mouse capture with the
        // terminal at all, rather than enabling it and then discarding
        // mouse events — a host that refuses capture (or has none to
        // give) should see no capture request in the first place.
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    }

    // Best-effort kitty keyboard enhancement push. Apps that
    // override this can call the crossterm functions before
    // `run()` and the runner won't double-push.
    let kbd_enhanced = push_keyboard_enhancement(&mut stdout);

    // SGR-Pixels mouse mode (quadraui#1048) — only attempted when mouse
    // capture itself is on. Requires *both* the live DECRQM probe saying
    // the terminal supports mode 1016 *and* a real, non-zero pixel cell
    // size — a terminal that would happily report pixel coordinates but
    // whose `window_size()` can't tell us how big a cell actually is (or
    // doesn't fill `ws_xpixel`/`ws_ypixel` at all) must never have `?1016h`
    // sent to it, since there would be no correct way to divide the
    // resulting coordinates back into cells. See `tui::caps`'s module doc
    // for why this probe (unlike the kitty-keyboard one) never falls back
    // to a heuristic guess on an ambiguous/absent answer.
    // Skip the live DECRQM round trip entirely on a multiplexer already
    // known not to forward mode 1016 (tmux, `TERM=screen*`/`tmux*`) —
    // quadraui#1048 review: the cheap heuristic already has a hard-`false`
    // answer for these, so paying the probe's up-to-2s wait (stacked on top
    // of the kitty-keyboard probe's own) buys nothing but startup latency
    // and a second window where a real keystroke could be read and
    // discarded as a candidate probe reply. See
    // `caps::sgr_pixel_mouse_blocked_by_multiplexer`'s doc.
    let probed_cell_pixel_size = if config.mouse
        && !super::caps::sgr_pixel_mouse_blocked_by_multiplexer()
        && super::caps::probe_sgr_pixel_mouse()
    {
        query_cell_pixel_size()
    } else {
        None
    };
    let sgr_pixel_mouse = probed_cell_pixel_size.is_some();
    let cell_pixel_size = probed_cell_pixel_size.unwrap_or(crate::TerminalCellSize::new(1.0, 1.0));
    if sgr_pixel_mouse {
        let _ = enable_sgr_pixel_mouse(&mut stdout);
    }

    let mut backend = TuiBackend::new();
    // Overwrite `TuiBackend::new()`'s environment-only guess with the live
    // answer: whether the enhancement flags were actually pushed just now
    // (quadraui#827). Before this line, `kbd_enhanced` was consulted only
    // to decide whether to pop the flags on exit below — an app had no way
    // to read it at all, so a gesture built on the assumption the push
    // worked would silently never fire on a terminal where it didn't. See
    // `crate::backend::BackendCaps::kitty_keyboard`'s doc.
    backend.set_kitty_keyboard(kbd_enhanced);
    // Record the `no-mouse` choice (quadraui#828) so an app can tell —
    // via `TuiBackend::mouse_enabled()` — that no mouse events will ever
    // arrive this session, instead of finding out by a mouse gesture
    // silently never firing.
    backend.set_mouse_enabled(config.mouse);
    // Record the SGR-Pixels negotiation outcome (quadraui#1048) — see
    // `crate::backend::BackendCaps::sgr_pixel_mouse`'s doc. `cell_pixel_size`
    // stays the identity `(1.0, 1.0)` whenever `sgr_pixel_mouse` is `false`,
    // so `TuiBackend::poll_events`/`wait_events` dividing by it is always a
    // no-op in that (overwhelmingly common) case.
    backend.set_sgr_pixel_mouse(sgr_pixel_mouse);
    backend.set_cell_pixel_size(cell_pixel_size);
    let crossterm_backend = CrosstermBackend::new(stdout);
    // Quantise every frame's SGR to what this terminal actually supports
    // (quadraui#826) — `backend.color_depth()` was detected from
    // `COLORTERM`/`TERM` inside `TuiBackend::new()` above.
    let depth_limited = DepthLimitedBackend::new(crossterm_backend, backend.color_depth());
    let mut terminal = Terminal::new(depth_limited)?;
    terminal.clear()?;
    // Shared, not owned outright: issue #965's nested dialog loop
    // (`TuiPlatformServices::show_file_open_dialog` et al.) needs to draw
    // through this *same* `Terminal` instance — a second, independently
    // constructed one would desync ratatui's diff cache from the
    // physical screen the moment control returns here (see
    // `tui::services`'s module doc, "Dialogs" section, for exactly why).
    let terminal = Rc::new(RefCell::new(terminal));
    let dialog_surface: Rc<RefCell<dyn crate::tui::services::DialogSurface>> = terminal.clone();
    backend.tui_services().set_dialog_surface(dialog_surface);

    Ok(TerminalSetup {
        terminal,
        backend,
        kbd_enhanced,
        sgr_pixel_mouse,
    })
}

/// Restore the terminal to its pre-[`setup_terminal`] state: pop the
/// kitty keyboard enhancement flags, disable SGR-Pixels mouse mode,
/// disable raw mode, disable mouse capture / bracketed paste, leave the
/// alternate screen, and show the cursor — the exact reverse of
/// [`setup_terminal`]'s negotiation. Every step is best-effort (`let _ =`):
/// a terminal that refused one of these on the way in isn't going to
/// error usefully on the way out either, and a caller tearing down mid-
/// panic (see [`run_with`]) needs this to never itself panic.
fn teardown_terminal(
    terminal: &Rc<RefCell<Terminal<LiveBackend>>>,
    kbd_enhanced: bool,
    sgr_pixel_mouse: bool,
    mouse: bool,
) {
    use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture};

    let mut terminal = terminal.borrow_mut();
    if kbd_enhanced {
        let _ = pop_keyboard_enhancement(terminal.backend_mut());
    }
    if sgr_pixel_mouse {
        // `?1016l` ahead of `DisableMouseCapture`, mirroring the enable
        // order (quadraui#1048) — this runs on every exit path (including
        // a panic), so a session that turned pixel mode on always turns
        // it back off.
        let _ = disable_sgr_pixel_mouse(terminal.backend_mut());
    }
    let _ = disable_raw_mode();
    if mouse {
        let _ = execute!(
            terminal.backend_mut(),
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
    } else {
        let _ = execute!(
            terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
    }
    let _ = terminal.show_cursor();
}

// `StepOutcome` — what happened during one non-blocking `step`/`pump` call
// — is defined once in `crate::runner` and shared by every backend's
// embedding-friendly runner (issue #1100: `TuiRunner` here,
// `crate::gtk::run::GtkRunner` on GTK), the same way `Reaction` already is;
// re-exported (not just imported) so existing in-crate callers that reach
// it through this module keep working.
pub use crate::runner::StepOutcome;

/// Non-blocking / bounded-wait entry points for a host that owns its own
/// event loop (issue #1100).
///
/// [`run`]/[`run_with`] each own the whole process: they block forever
/// (subject only to [`Reaction::Exit`]) inside a loop that waits on
/// crossterm, renders, and dispatches. That's the right shape for a
/// process whose only job is running one quadraui app, but it can't be
/// embedded inside a host that already owns its own loop and needs to
/// interleave quadraui's work with its own — Node's libuv reactor, a .NET
/// `SynchronizationContext`, Python's `asyncio` loop, a game engine's
/// per-frame tick. `TuiRunner` is the exact same setup, frame paint, and
/// event dispatch [`run_with`] uses (`run_with` is now written in terms of
/// it — see its body), split into a constructor plus two single-iteration
/// entry points a host calls at whatever cadence suits it:
///
/// - [`TuiRunner::step`] — never blocks. Repaints if needed, drains
///   whatever events are already queued ([`Backend::poll_events`]),
///   dispatches them, ticks, and returns. The counterpart of a
///   non-blocking `crossterm::event::poll(Duration::ZERO)` iteration.
/// - [`TuiRunner::pump`] — like `step`, but if nothing is queued yet,
///   blocks for up to `timeout` waiting for the first native event (or a
///   background [`Backend::waker`] payload) before giving up — bounded
///   the same way `crossterm::event::poll(timeout)` is.
///
/// Both return a [`StepOutcome`] rather than looping internally, so the
/// host decides what "call it again" means: a libuv idle handle, an
/// `asyncio` task rescheduled on its own loop, a fixed per-frame budget in
/// a game loop.
///
/// A background thread's [`Backend::waker`] call needs no extra wiring to
/// reach a `TuiRunner`-driven loop: TUI's `waker()` implementation simply
/// pushes the payload into a queue that [`Backend::poll_events`]/
/// [`Backend::wait_events`] already drain unconditionally on every call
/// (see that method's doc) — the same drain `step`/`pump` use internally.
/// A host polling via `step()` on its own timer observes the payload on
/// its next call; a host blocked in `pump(timeout)` observes it once
/// `timeout` elapses at the latest (TUI has no way to interrupt
/// crossterm's blocking read early — see [`Backend::waker`]'s "simplest of
/// the four" note — so a host that wants tighter wake latency than its own
/// poll cadence should keep `timeout` short rather than relying on an
/// early wake).
///
/// Terminal teardown (raw mode, alternate screen, mouse capture, kitty/
/// SGR-Pixels negotiation) happens automatically the moment `step`/`pump`
/// observes [`Reaction::Exit`] (surfaced as [`StepOutcome::Exited`]), and
/// again — idempotently — on [`Drop`], so a host that stops calling
/// `step`/`pump` early (abandons the runner, or lets a panic unwind
/// through it) still gets its terminal back.
pub struct TuiRunner<A: AppLogic> {
    terminal: Rc<RefCell<Terminal<LiveBackend>>>,
    backend: TuiBackend,
    app: A,
    needs_redraw: bool,
    // Trailing-edge resize debounce state (quadraui#437, shared utility
    // extracted in #496 — see `crate::runtime::ResizeDebouncer`). Holds
    // the most recent viewport from a burst of `WindowResized` events and
    // dispatches a single settled resize once `RESIZE_SETTLE` elapses with
    // no newer one.
    resize_debouncer: ResizeDebouncer,
    resize_deadline: Option<Instant>,
    kbd_enhanced: bool,
    sgr_pixel_mouse: bool,
    mouse: bool,
    finished: bool,
}

impl<A: AppLogic> TuiRunner<A> {
    /// Build a runner with the default [`RunConfig`] (mouse capture
    /// enabled). Negotiates the terminal (see [`setup_terminal`]), seeds
    /// the viewport from the real terminal size, and calls `app.setup()`
    /// before returning — exactly what [`run`]/[`run_with`] do before
    /// entering their internal loop.
    pub fn new(app: A) -> io::Result<Self> {
        Self::new_with(app, RunConfig::default())
    }

    /// Like [`Self::new`], but with an explicit [`RunConfig`] — e.g.
    /// [`RunConfig::no_mouse`].
    pub fn new_with(mut app: A, config: RunConfig) -> io::Result<Self> {
        let mouse = config.mouse;
        let TerminalSetup {
            terminal,
            mut backend,
            kbd_enhanced,
            sgr_pixel_mouse,
        } = setup_terminal(config)?;

        // Seed the viewport from the real terminal size BEFORE `setup()`.
        // `TuiBackend::new()` alone seeds a fixed `Viewport::default()`
        // (80×24); if `app.setup()` reads `backend.viewport()` to size a
        // side effect (e.g. spawning an embedded PTY at the viewport's
        // cell dimensions) it would otherwise always see 80×24 until the
        // first `WindowResized` event (quadraui#437, the TUI counterpart
        // of the original tiny-window bug).
        let size = terminal.borrow().size()?;
        backend.begin_frame(crate::Viewport::new(
            size.width as f32,
            size.height as f32,
            1.0,
        ));

        app.setup(&mut backend);

        Ok(Self {
            terminal,
            backend,
            app,
            needs_redraw: true,
            resize_debouncer: ResizeDebouncer::new(),
            resize_deadline: None,
            kbd_enhanced,
            sgr_pixel_mouse,
            mouse,
            finished: false,
        })
    }

    /// Shared access to the app — e.g. for a host inspecting state between
    /// `step`/`pump` calls.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// Mutable access to the app.
    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    /// Shared access to the backend.
    pub fn backend(&self) -> &TuiBackend {
        &self.backend
    }

    /// Mutable access to the backend — e.g. to call
    /// [`Backend::request_frame_in`] directly from host code.
    pub fn backend_mut(&mut self) -> &mut TuiBackend {
        &mut self.backend
    }

    /// One non-blocking pass: repaint if needed, drain whatever events are
    /// already queued (never blocks), dispatch them, tick, and return. See
    /// the type-level doc for the full contract.
    pub fn step(&mut self) -> io::Result<StepOutcome> {
        self.run_one(None)
    }

    /// One pass, blocking for up to `timeout` if nothing is queued yet.
    /// See the type-level doc for the full contract.
    pub fn pump(&mut self, timeout: Duration) -> io::Result<StepOutcome> {
        self.run_one(Some(timeout))
    }

    fn run_one(&mut self, block_for: Option<Duration>) -> io::Result<StepOutcome> {
        if self.finished {
            return Ok(StepOutcome::Exited);
        }

        // Issue #1037: a pending `Backend::request_full_repaint` forces a
        // redraw even if nothing else asked for one this iteration — see
        // `Backend::request_full_repaint`'s doc.
        let full_repaint = self.backend.take_full_repaint_requested();
        self.needs_redraw |= full_repaint;
        if self.needs_redraw {
            let mut guard = self.terminal.borrow_mut();
            if full_repaint {
                guard.clear()?;
            }
            render_frame(&mut guard, &mut self.backend, &self.app)?;
            drop(guard);
            self.needs_redraw = false;
        }

        // `step` (`block_for: None`) never blocks — `Backend::poll_events`
        // drains whatever's already queued. `pump` (`block_for:
        // Some(ceiling)`) blocks for up to the nearer of `ceiling`, any
        // pending `Backend::request_frame_in`/`Reaction::RedrawAfter`
        // deadline, and the still-pending debounced-resize deadline below
        // — quadraui#832's shortened-timeout logic, unchanged from the
        // pre-#1100 `run_inner` loop this replaces.
        let events = match block_for {
            Some(ceiling) => {
                let mut timeout = self.backend.frame_poll_timeout(ceiling);
                if let Some(d) = self.resize_deadline {
                    timeout = timeout.min(d.saturating_duration_since(Instant::now()));
                }
                self.backend.wait_events(timeout)
            }
            None => self.backend.poll_events(),
        };
        // Clear an elapsed frame deadline *before* dispatching this
        // batch's events or calling `tick` — see
        // `crate::runtime::FrameScheduler::clear_if_due`'s doc for why the
        // order matters (a fresh request made by either must survive
        // this).
        self.backend.clear_frame_deadline_if_due();

        for event in events {
            if let Some(outcome) = self.handle_native_event(event)? {
                return Ok(outcome);
            }
        }

        // Fire the debounced resize once the drag has settled.
        if self.resize_deadline.is_some_and(|d| Instant::now() >= d) {
            self.resize_deadline = None;
            if let Some(viewport) = self.resize_debouncer.take() {
                if let Some(outcome) = self.dispatch_and_map(UiEvent::WindowResized { viewport })? {
                    return Ok(outcome);
                }
            }
        }

        // Periodic tick — called after every event batch (including an
        // empty one, whether from an empty `poll_events` drain or a
        // `pump` timeout). Lets apps drive timer logic without synthetic
        // event injection.
        match self.app.tick(&mut self.backend) {
            Reaction::Continue => {}
            Reaction::Redraw => self.needs_redraw = true,
            Reaction::RedrawAfter(d) => self.backend.request_frame_in(d),
            Reaction::Exit => return Ok(self.finish()),
        }

        Ok(StepOutcome::Continue)
    }

    /// Debounce a `WindowResized` burst the same way the pre-#1100
    /// `run_inner` loop did (quadraui#437): coalesce to the latest size
    /// and defer dispatch until the drag settles (painting stays live
    /// regardless — [`render_frame`] re-reads the real terminal size every
    /// frame). Everything else dispatches through
    /// [`Self::dispatch_and_map`].
    fn handle_native_event(&mut self, event: UiEvent) -> io::Result<Option<StepOutcome>> {
        if let UiEvent::WindowResized { viewport } = event {
            // quadraui#1048: a font-size change resizes the terminal in
            // *pixels* without necessarily changing its row/column count,
            // silently invalidating the cached `TuiBackend::cell_pixel_size`
            // divisor SGR-Pixels mouse scaling depends on — re-query on
            // every resize, not just at startup.
            if self.backend.sgr_pixel_mouse() {
                match query_cell_pixel_size() {
                    Some(size) => self.backend.set_cell_pixel_size(size),
                    None => {
                        // The terminal stopped reporting a usable pixel
                        // size mid-session but is still in `?1016h` mode —
                        // turn it off at the terminal itself (mirroring
                        // teardown), not just in our own state, so we
                        // don't keep dividing raw pixel offsets by the
                        // identity divisor.
                        let _ = disable_sgr_pixel_mouse(self.terminal.borrow_mut().backend_mut());
                        self.backend.set_sgr_pixel_mouse(false);
                        self.backend
                            .set_cell_pixel_size(crate::TerminalCellSize::new(1.0, 1.0));
                    }
                }
            }
            self.resize_debouncer.note(viewport);
            self.resize_deadline = Some(Instant::now() + RESIZE_SETTLE);
            self.needs_redraw = true;
            return Ok(None);
        }
        self.dispatch_and_map(event)
    }

    fn dispatch_and_map(&mut self, event: UiEvent) -> io::Result<Option<StepOutcome>> {
        match dispatch_event(event, &mut self.backend, &mut self.app) {
            EventOutcome::Continue => Ok(None),
            EventOutcome::Redraw => {
                self.needs_redraw = true;
                Ok(None)
            }
            EventOutcome::RedrawAfter(d) => {
                self.backend.request_frame_in(d);
                Ok(None)
            }
            EventOutcome::Exit => Ok(Some(self.finish())),
        }
    }

    /// Idempotent terminal teardown. Called internally the moment
    /// `Reaction::Exit`/`EventOutcome::Exit` is observed, and again (as a
    /// no-op by then) from [`Drop`] — the safety net for a host that
    /// abandons the runner, or a panic that unwinds through it, without
    /// ever seeing [`StepOutcome::Exited`].
    fn finish(&mut self) -> StepOutcome {
        if !self.finished {
            teardown_terminal(
                &self.terminal,
                self.kbd_enhanced,
                self.sgr_pixel_mouse,
                self.mouse,
            );
            self.finished = true;
        }
        StepOutcome::Exited
    }
}

impl<A: AppLogic> Drop for TuiRunner<A> {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Render one frame.
///
/// Runs `app.render` inside the backend's frame scope, overlays the active
/// text-selection highlight, applies any editor cursor position painted
/// this frame, and finalises the frame. Generic over the ratatui backend
/// `B` so every caller — the live runner (`CrosstermBackend` wrapping real
/// stdout), the headless `TestBackend` driver, and the headless *vt100*
/// driver (`CrosstermBackend` wrapping an in-memory ANSI sink,
/// quadraui#555) — share one paint path.
///
/// Used to take an explicit `size: ratatui::layout::Size` parameter so a
/// caller who already knew its own fixed size (every `ConformanceDriver`
/// does — it's exactly the `LogicalViewport` it was built with) could avoid
/// `render_frame`'s `terminal.size()` query, which fails (or silently
/// returns the wrong dimensions) under `cargo test` — `CrosstermBackend`'s
/// `size()` queries the process's real controlling terminal (`/dev/tty` on
/// Unix) regardless of what `Write` sink the backend was actually
/// constructed with, and `cargo test` has no controlling terminal wired to
/// the vt100 driver's sink. #1040 removed that parameter: the viewport fed
/// to `begin_frame` is now derived from `frame.area()` *inside* the
/// `terminal.draw` closure below, which needs no size query at all — it
/// reads whatever `Terminal::draw`'s internal `autoresize()` just resized
/// the buffer to. That's also strictly more correct than a size passed in
/// by the caller: see the comment above `frame.area()`'s use below for why
/// a stale caller-supplied size could diverge from the buffer's real
/// extent and panic.
pub(crate) fn paint_frame<A, B>(
    terminal: &mut Terminal<B>,
    backend: &mut TuiBackend,
    app: &A,
) -> io::Result<()>
where
    A: AppLogic,
    B: ratatui::backend::Backend,
{
    // Issue #830: resolve the currently-focused widget's rect (if any)
    // from this frame's tab stops *before* entering the frame scope —
    // `AppLogic::tab_stops` is `&self`-only and cheap for an app that
    // already has this on hand from building its `ScreenLayout`. This
    // read is independent of `begin_frame`'s per-frame state (focus
    // tracking isn't touched by it), so it's safe to keep it ahead of
    // the `terminal.draw` call even though `begin_frame` itself moved
    // inside that closure below (#1040).
    let focus_ring_rect = crate::runtime::focused_stop_rect(backend, app, A::AreaId::default());
    terminal
        .draw(|frame| {
            // #1040: derive the layout-sizing viewport from the *actual*
            // frame area `Terminal::draw`'s internal `autoresize()` just
            // computed for this call — not from a size queried before this
            // closure ran (the pre-#1040 shape: `render_frame` queried
            // `terminal.size()`, then `paint_frame` fed that stale value
            // to `begin_frame` before ever calling `terminal.draw`).
            // Between such a pre-closure query and `terminal.draw` running,
            // the real terminal can shrink; `autoresize()` reallocates
            // `frame.buffer_mut()` for the new, smaller size right before
            // this closure runs, so `frame.area()` is always in sync with
            // the buffer `app.render` is about to paint into. Sizing the
            // layout pass from a stale, larger size instead produced
            // #1040's panic: window rects computed against the old size
            // indexed past the already-shrunk buffer's real extent.
            let frame_area = frame.area();
            backend.begin_frame(crate::Viewport::new(
                frame_area.width as f32,
                frame_area.height as f32,
                1.0,
            ));
            backend.enter_frame_scope(frame, |b| {
                // TUI is single-area; always pass the app's default
                // `AreaId`. Multi-area runners (GTK) pass the AreaId for
                // whichever surface is repainting.
                app.render(b, A::AreaId::default());
                // After app.render: paint the focus-ring convention
                // (#830) so it overlays on top of the widget's own
                // content, same ordering as the selection-highlight
                // overlay below.
                if let Some(rect) = focus_ring_rect {
                    b.draw_focus_ring(rect);
                }
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

/// Render one frame.
///
/// Thin, name-preserving wrapper over [`paint_frame`] for the live runner
/// (real stdout) and the headless `TestBackend` driver — both call sites
/// that pre-#1040 needed a pre-`terminal.draw` `terminal.size()` query to
/// seed `paint_frame`'s (now-removed) `size` parameter. `paint_frame` no
/// longer needs one (see its doc), so this wrapper's only remaining job is
/// giving the two live-ish callers a name distinct from the vt100 driver's
/// direct [`paint_frame`] call — it does not skip anything the direct
/// caller doesn't also get for free.
pub(crate) fn render_frame<A, B>(
    terminal: &mut Terminal<B>,
    backend: &mut TuiBackend,
    app: &A,
) -> io::Result<()>
where
    A: AppLogic,
    B: ratatui::backend::Backend,
{
    paint_frame(terminal, backend, app)
}

// `EventOutcome` — what the frame loop should do after [`dispatch_event`]
// handles one event — is defined once in `crate::runtime` and shared by
// every backend runner (quadraui#496); imported at the top of this file.

/// Dispatch one [`UiEvent`] through the app, applying the shared runner
/// pre-processing pipeline first.
///
/// The pre-processing itself — ActivityBar keyboard-focus redirect,
/// Tab/Shift+Tab focus cycling (#830), Ctrl-C copy, Ctrl-V/Ctrl-Shift-V paste,
/// middle-click PRIMARY-selection paste (a no-op on TUI — see
/// [`crate::backend::Clipboard::read_primary_selection`]'s default),
/// Ctrl-A select-all, selection-display clearing, `TextSelectionChanged`
/// — lives in [`crate::runtime::preprocess_event`] (quadraui#813),
/// shared with GTK/macOS/Windows; see that function's doc for the exact
/// priority order and `TuiBackend`'s `PreprocessBackend` impl for TUI's
/// two documented differences (a lenient Ctrl-C guard, and a no-op
/// `fold_double_click` — TUI already folds double-clicks upstream, in
/// [`TuiBackend::apply_dispatch`]/`translate_injected`).
///
/// One behavior change from before #813: a `Reaction::Continue` app now
/// stays `EventOutcome::Continue` after Ctrl-C, matching GTK/macOS/
/// Windows — this runner used to force `EventOutcome::Redraw`
/// unconditionally, which #496's original audit found was an
/// unintentional divergence (see `crate::runtime`'s module doc).
pub(crate) fn dispatch_event<A: AppLogic>(
    event: UiEvent,
    backend: &mut TuiBackend,
    app: &mut A,
) -> EventOutcome {
    crate::runtime::preprocess_event(event, backend, app)
}

/// Push kitty keyboard protocol flags (best-effort). Returns whether
/// the push succeeded; the caller pops on exit only if so, and (quadraui#827)
/// records this on [`TuiBackend`] via
/// [`crate::tui::backend::TuiBackend::set_kitty_keyboard`] so an app can
/// read it before relying on a gesture that needs it.
///
/// The support check itself is [`crate::tui::caps::probe_kitty_keyboard`],
/// not a direct `supports_keyboard_enhancement()` call — see that
/// function's doc for why the fallback to the environment heuristic
/// matters (a terminal that supports the protocol but sits behind a
/// multiplexer/relay that swallows the query/response round trip must not
/// collapse to "unsupported").
fn push_keyboard_enhancement(stdout: &mut io::Stdout) -> bool {
    use ratatui::crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    if !super::caps::probe_kitty_keyboard() {
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

fn pop_keyboard_enhancement(backend: &mut LiveBackend) -> io::Result<()> {
    use ratatui::crossterm::event::PopKeyboardEnhancementFlags;
    execute!(backend, PopKeyboardEnhancementFlags)
}

/// Resolve this terminal's real per-cell pixel size from a live
/// `window_size()` query (quadraui#1048), or `None` when it can't be
/// determined: the query itself fails (always the case on Windows —
/// crossterm's own doc says pixel size "is not implemented for the Windows
/// API"), or it succeeds but reports a `0` for any of `rows`/`columns`/
/// `width`/`height` — a real terminal that simply doesn't fill
/// `ws_xpixel`/`ws_ypixel` (documented as "unused" by
/// <https://man7.org/linux/man-pages/man4/tty_ioctl.4.html> on unix). Either
/// way, SGR-Pixels mode must not be enabled without a real, non-zero
/// divisor to scale its coordinates back into cells — see
/// [`run_with`]'s call site.
fn query_cell_pixel_size() -> Option<crate::TerminalCellSize> {
    let ws = ratatui::crossterm::terminal::window_size().ok()?;
    if ws.rows == 0 || ws.columns == 0 || ws.width == 0 || ws.height == 0 {
        return None;
    }
    Some(crate::TerminalCellSize::new(
        ws.width as f32 / ws.columns as f32,
        ws.height as f32 / ws.rows as f32,
    ))
}

/// Enable SGR-Pixels mouse mode (`CSI ? 1016 h`, quadraui#1048). Crossterm
/// has no typed `Command` for mode 1016 (only 1006, via
/// `EnableMouseCapture`), so this writes the raw DEC private-mode sequence
/// directly — the same escape-sequence shape [`ratatui::crossterm::event::EnableMouseCapture`]
/// itself would use for a mode crossterm did model.
fn enable_sgr_pixel_mouse(w: &mut impl io::Write) -> io::Result<()> {
    w.write_all(b"\x1b[?1016h")?;
    w.flush()
}

/// Disable SGR-Pixels mouse mode (`CSI ? 1016 l`) — the teardown-time
/// inverse of [`enable_sgr_pixel_mouse`]. Called ahead of
/// `DisableMouseCapture` in [`run_with`]'s teardown, mirroring the enable
/// order.
fn disable_sgr_pixel_mouse(w: &mut impl io::Write) -> io::Result<()> {
    w.write_all(b"\x1b[?1016l")?;
    w.flush()
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
    /// so tests can assert on them without a real clipboard. Also records
    /// every event `handle` receives (quadraui#813) so tests can assert an
    /// intercepted event never reaches the app at all — the empty-clipboard
    /// half of the Ctrl-V/middle-click paste contract.
    struct SelectionRecorder {
        last_copied: Option<String>,
        selection_changes: Vec<(Point, Point)>,
        events: Vec<UiEvent>,
    }

    impl SelectionRecorder {
        fn new() -> Self {
            Self {
                last_copied: None,
                selection_changes: Vec::new(),
                events: Vec::new(),
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
            self.events.push(event.clone());
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

    /// `DpiChanged` forces a repaint even when the app's own `handle`
    /// ignores it (issue #834). `SelectionRecorder`'s catch-all arm
    /// returns `Reaction::Continue` for any event it doesn't recognise —
    /// exactly the "app has no opinion on this event" shape most
    /// existing examples have for a brand-new variant. Without
    /// `crate::runtime::preprocess_event`'s dedicated force-redraw step
    /// this would leave whatever pixels were on screen before the DPI
    /// change untouched. This is a shared-runtime test (not a
    /// GTK/macOS/Win-specific one) because `preprocess_event` is the one
    /// piece of plumbing all four backends funnel `DpiChanged` through —
    /// proving it here proves it for all three GUI runners at once.
    #[test]
    fn dpi_changed_forces_redraw_even_when_app_ignores_it() {
        let mut driver = TuiDriver::new(SelectionRecorder::new(), 40, 10);

        let reaction = driver.dispatch(UiEvent::DpiChanged(2.0));

        assert_eq!(
            reaction,
            Reaction::Redraw,
            "DpiChanged must force a redraw regardless of the app's own Reaction"
        );
        assert!(
            driver.app().events.iter().any(
                |e| matches!(e, UiEvent::DpiChanged(scale) if (*scale - 2.0).abs() < f32::EPSILON)
            ),
            "the app must still observe the DpiChanged event itself, not just the forced redraw"
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

    /// quadraui#813: before this, TUI's `dispatch_event` never intercepted
    /// Ctrl-V at all (real terminals deliver a paste as crossterm's
    /// bracketed-paste `Event::Paste` → `UiEvent::ClipboardPaste` directly,
    /// a separate translation-layer path this test doesn't exercise). The
    /// shared `preprocess_event` pipeline now adds the same Ctrl-V/
    /// Ctrl-Shift-V keypress interception GTK/macOS/Windows already had, as
    /// a supplementary path (e.g. for a terminal/multiplexer that doesn't
    /// negotiate bracketed paste, or a scripted `KeyPressed` like this
    /// test's).
    ///
    /// Unlike `gtk::run::paste_tests`, this can't assert on the swallowed-
    /// vs-forwarded distinction against a *specific* clipboard payload:
    /// `TuiBackend` has no `install_test_clipboard` fake (GTK's is backed
    /// by a swappable `GtkClipboard` service; TUI's `TuiClipboard` always
    /// goes straight to a real `arboard::Clipboard`), and asserting on the
    /// host's real clipboard contents would be exactly the environment-
    /// dependent test `gtk::run::paste_tests`'s own doc comment warns
    /// against — green on headless CI, red (or silently wrong) on a
    /// developer desktop with something already copied. What *is* safe to
    /// pin here, on any host: Ctrl-V must never panic and must never reach
    /// `app.handle` as a literal `'v'` `KeyPressed` — whichever clipboard
    /// branch it takes, the raw keypress is always swallowed.
    #[test]
    fn ctrl_v_is_never_forwarded_as_a_raw_keypress() {
        let mut driver = TuiDriver::new(SelectionRecorder::new(), 40, 10);
        driver.ctrl_char('v');
        assert!(
            !driver.app().events.iter().any(|e| matches!(
                e,
                UiEvent::KeyPressed {
                    key: Key::Char('v'),
                    ..
                }
            )),
            "Ctrl-V must never fall through to app.handle as a raw 'v' \
             keypress, got {:?}",
            driver.app().events
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
            .drag_state_handle()
            .borrow_mut()
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
            !backend.drag_state_handle().borrow().is_active(),
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
