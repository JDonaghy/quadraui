//! PTY + vt100 + scrollback engine for embedded terminal emulators.
//!
//! Provides [`TerminalSession`] (single pane) and [`TerminalManager`]
//! (multi-tab) that own the PTY process, background reader thread,
//! vt100 screen parser, and scrollback ring buffer. Paint output is a
//! [`crate::Terminal`] snapshot — directly consumable by the existing
//! [`crate::tui::draw_terminal`] / `Backend::draw_terminal` rasterisers.
//!
//! # Feature gate
//!
//! This module is compiled only when the `terminal` Cargo feature is
//! enabled. The rasteriser lives in [`crate::primitives::terminal`] and
//! is always available (no extra feature needed to paint snapshots you
//! obtained elsewhere).
//!
//! # Quick start
//!
//! ```ignore
//! use quadraui::terminal_engine::{TerminalSession, default_shell};
//!
//! let cwd = std::env::current_dir()?;
//! let mut session =
//!     TerminalSession::spawn(80, 24, &default_shell(), &cwd, 5_000)?;
//!
//! // In your event loop tick():
//! if session.poll() {
//!     let sb = session.scrollbar_state(None);
//!     let snapshot = session.to_terminal(WidgetId::new("term:0"), Some(sb));
//!     // pass snapshot to Backend::draw_terminal(rect, &snapshot)
//! }
//!
//! // Forward keyboard input:
//! session.write_input(b"ls\n");
//!
//! // Resize on layout change:
//! session.resize(120, 40);
//! ```
//!
//! # Multi-tab / multi-pane
//!
//! [`TerminalManager`] wraps a `Vec<TerminalSession>` and tracks the
//! active index. Tab-switching keybindings and split-pane layouts are
//! the **consuming app's** responsibility — this type exposes only the
//! data-management API.
//!
//! # Design decisions
//!
//! - The vt100 parser is always kept at `scrollback = 0` (live view).
//!   Lines that scroll off the screen are captured into a private
//!   [`VecDeque`] ring buffer instead — this avoids fighting with the
//!   vt100 crate's own bounded scrollback buffer and gives us full
//!   control over the history capacity.
//! - [`TerminalSession::to_terminal`] is the only public snapshot
//!   builder. Find-match overlays (`is_find_match`, `is_find_active`)
//!   are left as `false` — callers that implement in-terminal search
//!   can post-process the returned `Terminal::cells`.
//! - The cross-backend portability commitment: `TerminalSession` is
//!   backend-agnostic. The GTK backend can call `to_terminal()` just
//!   as easily as the TUI backend.

use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::event::MouseButton;
use crate::primitives::terminal::{Terminal, TerminalCell, TerminalScrollbar};
use crate::types::{Color, Modifiers, WidgetId};

// ── Internal history cell ─────────────────────────────────────────────────────

/// A single captured terminal cell in the scrollback ring buffer.
///
/// Uses `vt100::Color` directly to defer RGB resolution until paint time,
/// matching the approach used by downstream terminal-history cell types.
#[derive(Clone, Copy)]
struct HistCell {
    ch: char,
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    italic: bool,
    underline: bool,
}

impl Default for HistCell {
    fn default() -> Self {
        HistCell {
            ch: ' ',
            fg: vt100::Color::Default,
            bg: vt100::Color::Default,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

// ── Selection ─────────────────────────────────────────────────────────────────

/// Mouse text-selection state for a terminal pane.
///
/// All coordinates are 0-based into the **visible grid** (not into history).
/// End-column is inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSelection {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
}

/// Normalise a selection so `(r0, c0) ≤ (r1, c1)` in reading order.
fn normalize_selection(sel: &TerminalSelection) -> (u16, u16, u16, u16) {
    if (sel.start_row, sel.start_col) <= (sel.end_row, sel.end_col) {
        (sel.start_row, sel.start_col, sel.end_row, sel.end_col)
    } else {
        (sel.end_row, sel.end_col, sel.start_row, sel.start_col)
    }
}

// ── Colour helpers ────────────────────────────────────────────────────────────

/// Map a `vt100::Color` to an RGB triple.
///
/// `Default` resolves to dark-theme terminal defaults matching a
/// common OneDark-style baseline (`#e5e5e5` fg, `#1e1e1e` bg). Callers
/// that want theme-aware colours should post-process cells after
/// calling [`TerminalSession::to_terminal`].
fn map_vt100_color(color: vt100::Color, is_bg: bool) -> (u8, u8, u8) {
    match color {
        vt100::Color::Default => {
            if is_bg {
                (30, 30, 30) // terminal background (~#1e1e1e)
            } else {
                (229, 229, 229) // terminal foreground (~#e5e5e5)
            }
        }
        vt100::Color::Rgb(r, g, b) => (r, g, b),
        vt100::Color::Idx(n) => xterm_256_color(n),
    }
}

/// Standard xterm 256-colour palette lookup.
fn xterm_256_color(n: u8) -> (u8, u8, u8) {
    // System colours 0-15.
    const SYSTEM: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    if n < 16 {
        return SYSTEM[n as usize];
    }
    // 6×6×6 colour cube: indices 16-231.
    if n < 232 {
        let idx = n - 16;
        let b = idx % 6;
        let g = (idx / 6) % 6;
        let r = idx / 36;
        let to_byte = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return (to_byte(r), to_byte(g), to_byte(b));
    }
    // Greyscale ramp: indices 232-255.
    let gray = 8 + (n - 232) * 10;
    (gray, gray, gray)
}

// ── Mouse → PTY encoding (SGR-1006) ──────────────────────────────────────────

/// Mouse event kinds that can be forwarded to a PTY child via SGR-1006.
///
/// Mirrors the granularity an embedded terminal needs to dispatch: button
/// press/release, motion (with a button held, per DEC 1002), and wheel.
/// Wheel direction is encoded in the kind because wheel events have no
/// matching `MouseButton` in [`crate::event::MouseButton`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMouseKind {
    /// Button press.
    Press,
    /// Button release.
    Release,
    /// Cursor motion. For SGR-1006 the `button` field still indicates which
    /// (if any) button is being held; pass [`MouseButton::Left`] with no
    /// button-down state if the caller can't disambiguate.
    Move,
    /// Mouse wheel scrolled up (toward the top of content).
    WheelUp,
    /// Mouse wheel scrolled down (toward the bottom of content).
    WheelDown,
}

/// Encode a mouse event as SGR-1006 bytes ready to write to a PTY.
///
/// Format: `ESC [ < Cb ; Cx ; Cy M` (press / wheel / motion) or
/// `ESC [ < Cb ; Cx ; Cy m` (release). Coordinates in the output are 1-based;
/// the `col` / `row` inputs are **0-based** cell indices.
///
/// The `Cb` byte packs button identity + modifier state + motion bit:
///
/// | bits      | meaning                                              |
/// |-----------|------------------------------------------------------|
/// | 0–1       | button low bits (0=left, 1=middle, 2=right)          |
/// | 2 (= 4)   | shift                                                |
/// | 3 (= 8)   | alt / meta                                           |
/// | 4 (= 16)  | ctrl                                                 |
/// | 5 (= 32)  | motion event                                         |
/// | 6 (= 64)  | wheel (combined with bits 0–1 for direction)         |
/// | 7 (= 128) | extra button (X1 → 128, X2 → 129)                    |
///
/// Wheel events always use the `M` terminator (they have no release).
/// [`MouseButton::Other(n)`] forwards `n` directly into the low button bits.
pub fn encode_mouse_sgr(
    kind: TerminalMouseKind,
    button: MouseButton,
    col: u16,
    row: u16,
    modifiers: Modifiers,
) -> Vec<u8> {
    // Wheel codes are independent of `button`.
    let mut cb: u32 = match kind {
        TerminalMouseKind::WheelUp => 64,
        TerminalMouseKind::WheelDown => 65,
        TerminalMouseKind::Press | TerminalMouseKind::Release | TerminalMouseKind::Move => {
            match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                MouseButton::X1 => 128,
                MouseButton::X2 => 129,
                MouseButton::Other(n) => n as u32,
            }
        }
    };

    if matches!(kind, TerminalMouseKind::Move) {
        cb |= 32; // motion bit
    }
    if modifiers.shift {
        cb |= 4;
    }
    if modifiers.alt {
        cb |= 8;
    }
    if modifiers.ctrl {
        cb |= 16;
    }

    let terminator = match kind {
        TerminalMouseKind::Release => b'm',
        // Press / Move / WheelUp / WheelDown all use uppercase 'M'.
        _ => b'M',
    };

    // Convert 0-based cell to 1-based protocol coordinates. Saturate at u16::MAX.
    let cx = col.saturating_add(1);
    let cy = row.saturating_add(1);

    format!("\x1b[<{cb};{cx};{cy}{}", terminator as char).into_bytes()
}

// ── TerminalSession ───────────────────────────────────────────────────────────

/// Longest [`TerminalSession::resize`] waits for the child to *begin* its
/// post-SIGWINCH redraw — i.e. for the **first** byte to arrive (quadraui#437,
/// blocking #2).
///
/// SIGWINCH delivery + the shell's trap/prompt-reprint is normally a few ms,
/// but under load (a busy machine, a scheduler-starved child) the child can
/// take noticeably longer just to *start* writing. The settle must not give up
/// during that initial silence — that was the original bug: an 8 ms idle
/// window elapsed before the child had reacted at all, so `resize()` returned
/// having consumed nothing and the redraw was later reparsed at a changed
/// width (the ghost). This window is therefore generous. If nothing arrives
/// within it the child is not redrawing (e.g. it ignores SIGWINCH) and
/// `resize()` returns.
const RESIZE_SETTLE_FIRST: std::time::Duration = std::time::Duration::from_millis(80);

/// Once the child's redraw is *flowing*, an idle gap this long means it has
/// finished. Consuming stops here so a subsequent resize can't reparse the
/// redraw against a grid whose width has since changed.
const RESIZE_SETTLE_IDLE: std::time::Duration = std::time::Duration::from_millis(8);

/// Absolute cap on total settle time, so a continuously chatty child
/// (e.g. `yes`) that never goes idle can't stall the UI indefinitely.
const RESIZE_SETTLE_MAX: std::time::Duration = std::time::Duration::from_millis(120);

/// Block for the child's post-SIGWINCH redraw and return the chunks it produced,
/// in arrival order (quadraui#437, blocking #2).
///
/// This is the timing policy behind [`TerminalSession::settle_after_resize`],
/// factored into a free function that takes only the receiver so it can be
/// unit-tested against a synthetic [`std::sync::mpsc::channel`] — no PTY, no
/// shell, no flaky dependence on when a particular shell chooses to run a
/// WINCH trap.
///
/// Two phases:
///
/// 1. **Wait for the redraw to *start*.** Until the first chunk arrives we wait
///    up to [`RESIZE_SETTLE_FIRST`]. Treating the initial silence as "settled"
///    (waiting only [`RESIZE_SETTLE_IDLE`] from the outset) was the bug: under
///    load the child hasn't reacted to SIGWINCH within a few ms, so the settle
///    returned having consumed nothing and its redraw was later reparsed at a
///    changed width — the ghost. If nothing arrives in this window the child is
///    not redrawing and we return empty.
/// 2. **Drain the redraw.** Once chunks are flowing we keep consuming until the
///    channel has been idle for [`RESIZE_SETTLE_IDLE`] (redraw complete).
///
/// Total blocking is capped at [`RESIZE_SETTLE_MAX`].
fn collect_post_resize_output(rx: &Receiver<Vec<u8>>) -> Vec<Vec<u8>> {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Instant;

    let start = Instant::now();
    let hard_deadline = start + RESIZE_SETTLE_MAX;
    let first_deadline = start + RESIZE_SETTLE_FIRST;
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    loop {
        let now = Instant::now();
        let cap = hard_deadline.saturating_duration_since(now);
        if cap.is_zero() {
            break;
        }
        // Phase 1 (nothing captured yet): wait up to RESIZE_SETTLE_FIRST for the
        // child to *begin* redrawing. Phase 2 (redraw flowing): wait only for
        // the short idle gap that marks it complete. Both clamped to the cap.
        let wait = if chunks.is_empty() {
            first_deadline.saturating_duration_since(now)
        } else {
            RESIZE_SETTLE_IDLE
        }
        .min(cap);
        if wait.is_zero() {
            break;
        }
        match rx.recv_timeout(wait) {
            Ok(data) => chunks.push(data),
            // Timeout → phase 1: child never reacted; phase 2: redraw done.
            // Disconnected → child gone. Either way we are settled.
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    chunks
}

/// Re-wrap a vt100 parser's visible screen to a new size, preserving on-screen
/// content across a **width** change (quadraui#437).
///
/// vt100 0.16's [`vt100::Screen::set_size`] is **non-reflowing**: shrinking the
/// column count truncates each row's tail and widening pads with blanks, so a
/// naive shrink→expand drag permanently loses text. This helper restores
/// logical-line reflow using only vt100's public API — no vendored/patched
/// crate:
///
/// 1. Snapshot the current screen as a formatted byte stream
///    ([`vt100::Screen::contents_formatted`]). Wrapped rows are emitted as one
///    continuous run (no interior line break), so a logical line keeps its
///    structure regardless of where it currently wraps.
/// 2. Resize the grid.
/// 3. Replay the snapshot; vt100 re-wraps each logical line at the *new* width.
///
/// Gated to genuine **width** changes on the **normal** screen:
///
/// - A height-only change keeps the cheap [`vt100::Screen::set_size`] path — no
///   horizontal content is at risk, and re-wrapping would be wasted work.
/// - The **alternate** screen is never reflowed. Full-screen apps (vim, htop,
///   tmux) repaint themselves from scratch on SIGWINCH and their
///   absolute-positioned output must not be re-wrapped.
///
/// **Limitation:** `contents_formatted` covers only the *visible* screen, so a
/// shrink deep enough to push rows into scrollback cannot restore those rows on
/// a later expand — the public API exposes no formatted scrollback dump. The
/// common case (the live prompt / on-screen command output) round-trips
/// losslessly, which is the regression #437 chased.
fn reflow_screen(parser: &mut vt100::Parser, rows: u16, cols: u16) {
    let (cur_rows, cur_cols) = parser.screen().size();
    if (rows, cols) == (cur_rows, cur_cols) {
        return;
    }
    // Height-only change, or a self-repainting full-screen app: use the plain
    // non-reflow resize. (`alternate_screen()` reads the parser immutably; the
    // borrow ends before the `screen_mut()` below.)
    if cols == cur_cols || parser.screen().alternate_screen() {
        parser.screen_mut().set_size(rows, cols);
        return;
    }
    // Width change on the normal screen: snapshot → resize → replay so each
    // logical line re-wraps at the new width instead of being truncated.
    let dump = parser.screen().contents_formatted();
    parser.screen_mut().set_size(rows, cols);
    parser.process(&dump);
}

/// A single PTY-backed terminal session: PTY process, reader thread,
/// vt100 parser, and scrollback ring buffer.
///
/// Call [`poll`](Self::poll) each frame tick to drain PTY output, then
/// [`to_terminal`](Self::to_terminal) to build a paint snapshot.
///
/// ## Scrollback model
///
/// The vt100 parser is kept at `scrollback = 0` (live view) at all
/// times. Lines that scroll off the live screen are captured into an
/// internal [`VecDeque`] ring. Calling
/// [`scroll_up`](Self::scroll_up) / [`scroll_down`](Self::scroll_down)
/// adjusts `scroll_offset`; the snapshot builder blends history rows
/// and live rows accordingly.
pub struct TerminalSession {
    /// VT100 screen parser — always at `scrollback = 0` (live view).
    parser: vt100::Parser,
    /// Write half of the PTY master — sends keyboard input to the shell.
    writer: Box<dyn Write + Send>,
    /// PTY master — kept alive for `resize()` calls (SIGWINCH).
    master: Box<dyn MasterPty + Send>,
    /// Child shell process.
    child: Box<dyn Child + Send + Sync>,
    /// PTY output bytes from the background reader thread.
    rx: Receiver<Vec<u8>>,
    /// Current terminal width in columns.
    ///
    /// Read via [`cols()`](Self::cols); change only through [`resize()`](Self::resize)
    /// to keep the vt100 parser and PTY master in sync.
    cols: u16,
    /// Current terminal height in rows.
    ///
    /// Read via [`rows()`](Self::rows); change only through [`resize()`](Self::resize).
    rows: u16,
    /// Mouse text selection, if any.
    pub selection: Option<TerminalSelection>,
    /// `true` once the child process has exited.
    ///
    /// Read via [`is_exited()`](Self::is_exited). Set only by [`poll()`](Self::poll)
    /// when `child.try_wait()` returns a status — setting it externally would
    /// desynchronise the exit-code state.
    exited: bool,
    /// Exit code of the child process once it has exited, or `None` while
    /// still running. `0` conventionally means success.
    exit_code: Option<u32>,
    /// How many rows above the live bottom the user has scrolled.
    /// `0` = live view; maximum = `history.len()`.
    ///
    /// Read via [`scroll_offset()`](Self::scroll_offset); change through
    /// [`set_scroll_offset()`](Self::set_scroll_offset) /
    /// [`scroll_up()`](Self::scroll_up) / [`scroll_down()`](Self::scroll_down)
    /// so that `parser.set_scrollback(0)` is always called consistently.
    scroll_offset: usize,
    /// Scrollback ring buffer (oldest at index 0, newest at the back).
    history: VecDeque<Vec<HistCell>>,
    /// Maximum number of rows kept in `history` (`0` = unlimited — not
    /// recommended for long-lived sessions).
    history_capacity: usize,
}

impl TerminalSession {
    // ── Construction ─────────────────────────────────────────────────────────

    /// Spawn a new interactive shell session.
    ///
    /// - `cols`, `rows` — initial PTY dimensions.
    /// - `shell` — shell binary path (e.g. `"/bin/bash"`). Use
    ///   [`default_shell`] to read `$SHELL`.
    /// - `cwd` — working directory for the shell process.
    /// - `history_capacity` — maximum scrollback lines to retain.
    ///   `0` means unlimited (use a large finite value for production).
    pub fn spawn(
        cols: u16,
        rows: u16,
        shell: &str,
        cwd: &Path,
        history_capacity: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        // Present the embedded terminal as a clean, top-level terminal. When
        // the host app is itself launched from inside tmux,
        // the spawned shell would otherwise inherit $TMUX/$TMUX_PANE and any
        // tmux command run here would be treated as *nested* in the host's
        // outer session — e.g. `tmux attach-session` refuses ("sessions should
        // be nested with care") and `switch-client` hijacks the host's outer
        // client instead of rendering in this pane. Scrubbing them makes an
        // interactive session launched in this pane attach in-pane as expected.
        cmd.env_remove("TMUX");
        cmd.env_remove("TMUX_PANE");
        cmd.cwd(cwd);
        let child = pair.slave.spawn_command(cmd)?;

        let writer = pair.master.take_writer()?;
        let reader = pair.master.try_clone_reader()?;
        let master = pair.master;

        // Background reader thread: pushes PTY bytes to the main thread
        // via a channel without blocking the event loop.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break; // main thread dropped the session
                        }
                    }
                }
            }
        });

        // 1 000-line internal vt100 scrollback — used only to read back
        // the rows that just scrolled off the live screen into `history`.
        // We never call `set_scrollback()` for user-facing scrolling.
        let parser = vt100::Parser::new(rows, cols, 1000);

        Ok(Self {
            parser,
            writer,
            master,
            child,
            rx,
            cols,
            rows,
            selection: None,
            exited: false,
            exit_code: None,
            scroll_offset: 0,
            history: VecDeque::new(),
            history_capacity,
        })
    }

    // ── I/O ──────────────────────────────────────────────────────────────────

    /// Drain pending PTY output and feed it to the vt100 parser.
    ///
    /// Lines that scroll off the live screen are captured into the
    /// scrollback ring buffer. Also polls child-process exit status.
    ///
    /// Returns `true` when any new data was processed — the caller
    /// should trigger a repaint.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(data) = self.rx.try_recv() {
            changed = true;
            self.process_with_capture(&data);
        }
        if !self.exited {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.exited = true;
                self.exit_code = Some(status.exit_code());
                changed = true;
            }
        }
        changed
    }

    /// Send raw bytes as keyboard input to the shell.
    ///
    /// The bytes are written directly to the PTY master write-end.
    /// Callers are responsible for encoding key presses into the
    /// appropriate escape sequences (see the `key_to_pty_bytes` helper
    /// in the `tui_terminal` example).
    pub fn write_input(&mut self, data: &[u8]) {
        let _ = self.writer.write_all(data);
        let _ = self.writer.flush();
    }

    /// Send a UTF-8 string as input to the shell.
    ///
    /// Convenience wrapper around [`write_input`](Self::write_input).
    /// A supervising process uses this to inject prompts programmatically.
    pub fn send_str(&mut self, s: &str) {
        self.write_input(s.as_bytes());
    }

    // ── Exit status ───────────────────────────────────────────────────────────

    /// Current terminal width in columns.
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// Current terminal height in rows.
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// `true` once the child process has exited.
    ///
    /// Use [`exit_code()`](Self::exit_code) for the numeric exit status.
    pub fn is_exited(&self) -> bool {
        self.exited
    }

    /// Current scroll offset (rows above the live bottom).
    ///
    /// `0` = live view; maximum = [`history_len()`](Self::history_len).
    /// Change via [`set_scroll_offset()`](Self::set_scroll_offset) /
    /// [`scroll_up()`](Self::scroll_up) / [`scroll_down()`](Self::scroll_down).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Exit code of the child process, or `None` while still running.
    ///
    /// `0` conventionally means success. Populated by the first [`poll`](Self::poll)
    /// call that observes the child exiting. Check [`is_exited()`](Self::is_exited)
    /// first if you only need a boolean; this method returns the actual numeric code.
    pub fn exit_code(&self) -> Option<u32> {
        self.exit_code
    }

    /// Returns `true` when the cursor should be rendered visible.
    ///
    /// The cursor is suppressed when:
    /// - The child process has exited ([`exited`](Self::exited) is `true`), or
    /// - The view is scrolled into history (`scroll_offset > 0`).
    ///
    /// Use this to gate cursor rendering in the app layer instead of
    /// relying on cell-level `is_cursor` flags alone.
    pub fn cursor_visible(&self) -> bool {
        !self.exited && self.scroll_offset == 0
    }

    // ── Text scrape ───────────────────────────────────────────────────────────

    /// Extract the current vt100 live screen as plain text.
    ///
    /// Returns one line per screen row, trailing whitespace stripped.
    /// Completely blank rows at the **bottom** of the screen are omitted.
    /// This is the "what does the visible terminal look like right now?"
    /// getter — useful for programmatic drivers that need to scrape output
    /// (e.g. detecting a shell prompt line) without maintaining a full
    /// cell snapshot.
    pub fn screen_text(&self) -> String {
        let screen = self.parser.screen();
        let mut lines: Vec<String> = (0..self.rows)
            .map(|r| {
                let mut line = String::new();
                for c in 0..self.cols {
                    if let Some(cell) = screen.cell(r, c) {
                        let s = cell.contents();
                        if s.is_empty() {
                            line.push(' ');
                        } else {
                            line.push_str(s);
                        }
                    } else {
                        line.push(' ');
                    }
                }
                line.trim_end().to_string()
            })
            .collect();
        // Drop trailing blank rows.
        while lines.last().is_some_and(|l: &String| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Returns `true` when the child has enabled bracketed-paste mode
    /// (`ESC[?2004h`).
    ///
    /// Interactive programs (e.g. `claude`, shells with line editors) turn
    /// this mode on once their input prompt is live and ready to accept
    /// keystrokes, so it doubles as a reliable **input-readiness signal**
    /// for programmatic drivers: wait for this to flip `true` before
    /// injecting a bracketed paste, otherwise early bytes are silently
    /// dropped. Backed by vt100's tracking of the DEC private mode `2004`.
    pub fn bracketed_paste_enabled(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    /// Returns `true` when the child has enabled application-cursor-keys mode
    /// (DECCKM, DEC private mode `?1h` / `ESC [ ? 1 h`).
    ///
    /// In this mode, **unmodified** arrow keys and Home/End must be encoded as
    /// SS3 sequences (`ESC O A`…`ESC O D`, `ESC O H`, `ESC O F`) rather than
    /// the normal CSI sequences (`ESC [ A`…`ESC [ 4 ~`). Modifier combinations
    /// (e.g. Ctrl+Up) continue to use the CSI form regardless of this flag.
    ///
    /// Full-TUI programs — `vim`, `neovim`, `claude`, `htop` — set DECCKM when
    /// active. Without honouring it, navigation inside those programs silently
    /// stops working. Key encoders must query this flag each keystroke and pass
    /// it to `key_to_pty_bytes` (see `examples/common/terminal_app.rs`).
    ///
    /// Backed by [`vt100::Screen::application_cursor`].
    pub fn application_cursor_keys(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    // ── Alt-screen + mouse reporting state ───────────────────────────────────

    /// `true` when the child is currently rendering on the alternate screen
    /// (DEC private mode `1047` / `1049`, or legacy `47`).
    ///
    /// Full-TUI applications (`vim`, `tmux`, `less`, `claude`, `htop`) switch
    /// to the alternate screen on launch and back to the primary screen on
    /// exit. The engine uses this signal to:
    ///
    /// 1. **Suppress scrollback capture** — alt-screen churn must never
    ///    pollute the shell's scrollback (quadraui #335).
    /// 2. **Route the wheel** — when the child is on the alt-screen, wheel
    ///    events forward to the PTY rather than scrolling our local
    ///    scrollback (quadraui #334). See
    ///    [`should_forward_wheel`](Self::should_forward_wheel).
    ///
    /// Backed by vt100's `Screen::alternate_screen()`.
    pub fn on_alt_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// `true` when the child has enabled any xterm mouse-reporting mode
    /// (DEC private modes `1000` / `1002` / `1003`). Independent of the
    /// `1006` (SGR) encoding bit, which selects the wire format but doesn't
    /// turn reporting on or off.
    ///
    /// Used by [`should_forward_wheel`](Self::should_forward_wheel) and
    /// [`forward_mouse`](Self::forward_mouse) to decide whether mouse events
    /// belong to the child rather than our local UI (selection, scrollback).
    pub fn mouse_reporting_enabled(&self) -> bool {
        self.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    /// Whether wheel scroll events should be forwarded to the PTY child
    /// rather than handled locally.
    ///
    /// Returns `true` when **either**:
    ///
    /// - the child has enabled mouse reporting
    ///   ([`mouse_reporting_enabled`](Self::mouse_reporting_enabled)), or
    /// - the child is on the alternate screen
    ///   ([`on_alt_screen`](Self::on_alt_screen)).
    ///
    /// The alt-screen clause is what makes embedded `claude` / `tmux` /
    /// `less` usable: even when those programs don't request mouse reporting,
    /// scrolling our local (now-empty, alt-screen-shadowed) scrollback would
    /// be jarring — forwarding the wheel lets the inner app paginate
    /// (quadraui #334).
    pub fn should_forward_wheel(&self) -> bool {
        self.mouse_reporting_enabled() || self.on_alt_screen()
    }

    // ── Mouse → PTY forwarding ────────────────────────────────────────────────

    /// Encode a mouse event as SGR-1006 PTY bytes, **without** writing it.
    ///
    /// Returns `None` when the engine has determined the event should not be
    /// reported to the child:
    ///
    /// - Wheel events are gated on
    ///   [`should_forward_wheel`](Self::should_forward_wheel).
    /// - Press / Release / Move are gated on
    ///   [`mouse_reporting_enabled`](Self::mouse_reporting_enabled).
    ///
    /// Callers that want to bypass the gate (e.g. for testing) can call the
    /// free function [`encode_mouse_sgr`] directly.
    pub fn encode_mouse(
        &self,
        kind: TerminalMouseKind,
        button: MouseButton,
        col: u16,
        row: u16,
        modifiers: Modifiers,
    ) -> Option<Vec<u8>> {
        let allow = match kind {
            TerminalMouseKind::WheelUp | TerminalMouseKind::WheelDown => {
                self.should_forward_wheel()
            }
            _ => self.mouse_reporting_enabled(),
        };
        if !allow {
            return None;
        }
        Some(encode_mouse_sgr(kind, button, col, row, modifiers))
    }

    /// Forward a mouse event to the PTY child as SGR-1006 bytes, gated on
    /// the current reporting / alt-screen state.
    ///
    /// Returns `true` when bytes were written. When `false`, the caller
    /// should fall back to local handling — wheel events scroll our own
    /// scrollback ([`scroll_up`](Self::scroll_up) / [`scroll_down`](Self::scroll_down)),
    /// clicks drive local selection, etc.
    pub fn forward_mouse(
        &mut self,
        kind: TerminalMouseKind,
        button: MouseButton,
        col: u16,
        row: u16,
        modifiers: Modifiers,
    ) -> bool {
        match self.encode_mouse(kind, button, col, row, modifiers) {
            Some(bytes) => {
                self.write_input(&bytes);
                true
            }
            None => false,
        }
    }

    /// Extract all captured scrollback history as plain text.
    ///
    /// Returns one line per history row (oldest first), trailing
    /// whitespace stripped, joined by newlines. Does **not** include the
    /// live screen — call [`screen_text`](Self::screen_text) for that, or
    /// [`full_text`](Self::full_text) for both together.
    pub fn scrollback_text(&self) -> String {
        let mut lines: Vec<String> = self
            .history
            .iter()
            .map(|row| {
                let mut line = String::new();
                for hc in row {
                    line.push(hc.ch);
                }
                line.trim_end().to_string()
            })
            .collect();
        // Drop trailing blank rows.
        while lines.last().is_some_and(|l: &String| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Concatenate [`scrollback_text`](Self::scrollback_text) and
    /// [`screen_text`](Self::screen_text) into a single string, separated
    /// by a newline when both are non-empty.
    ///
    /// Useful for programmatic scraping: scan `full_text()` for a prompt or
    /// completion marker after each `poll()` cycle.
    pub fn full_text(&self) -> String {
        let hist = self.scrollback_text();
        let live = self.screen_text();
        match (hist.is_empty(), live.is_empty()) {
            (true, _) => live,
            (_, true) => hist,
            _ => format!("{hist}\n{live}"),
        }
    }

    // ── Resize ───────────────────────────────────────────────────────────────

    /// Resize the PTY and update the vt100 parser dimensions.
    ///
    /// Sends SIGWINCH to the child process so running programs
    /// (e.g. `vim`, `htop`) re-layout their output.
    ///
    /// # Preserving content across a width change (quadraui#437)
    ///
    /// upstream vt100 0.16 `set_size` does **not** reflow — it truncates each
    /// row on a column shrink and pads with blanks on a widen, so a naive
    /// shrink→expand drag loses text permanently. We restore logical-line
    /// reflow via [`reflow_screen`] (public-API snapshot → resize → replay),
    /// gated to genuine width changes on the normal screen. This runs in the
    /// shared engine, so **both** the TUI and GTK backends get it for free.
    ///
    /// # Closing the shell-redraw-vs-resize race (quadraui#437)
    ///
    /// The corruption #437 also chased ("ghost copies of the prompt / status
    /// line stuck at wrong columns after a fast resize drag") is a *timing* bug
    /// in the shared engine, reproduced on **both** the TUI and GTK backends,
    /// so the fix lives here rather than in a backend paint path:
    ///
    /// 1. We set the grid to width `N` and SIGWINCH the shell.
    /// 2. The shell redraws its prompt/status line for width `N` — those
    ///    bytes use cursor-relative moves that only make sense on an
    ///    `N`-wide grid, and they queue in the reader thread's channel.
    /// 3. A fast drag re-sizes the grid *again* to width `M` **before** those
    ///    bytes are parsed.
    /// 4. The width-`N` redraw is then applied to an `M`-wide grid: every
    ///    relative move lands in the wrong column/row, scattering duplicated
    ///    prompt fragments that stay stuck until the next resize churns them.
    ///
    /// Two guards close this race:
    ///
    /// - **Before** re-sizing we drain + process any output *already queued*
    ///   at the current size, so bytes the shell already emitted are parsed on
    ///   the grid they were computed for.
    /// - **After** re-sizing + SIGWINCH we briefly wait for the reader thread
    ///   to go quiescent (see [`settle_after_resize`](Self::settle_after_resize)),
    ///   consuming *this* SIGWINCH's redraw at the width it was computed for.
    ///   The pre-resize drain alone cannot do this: the redraw triggered by
    ///   this resize has not been written by the child yet when `resize()` is
    ///   called, so on a rapid multi-step drag the next `resize()` would change
    ///   the width again before those bytes were parsed. Waiting for
    ///   quiescence bounds that window deterministically.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        // No-op guard: avoid a needless resize + SIGWINCH storm when the
        // caller re-sends the current size (common when a backend recomputes
        // the same cell dimensions every frame during a drag).
        if cols == self.cols && rows == self.rows {
            return;
        }
        // Drain + process any output queued at the *current* size before we
        // change dimensions, so in-flight redraws land on the grid they were
        // computed for rather than the one we are about to re-size to.
        while let Ok(data) = self.rx.try_recv() {
            self.process_with_capture(&data);
        }
        self.cols = cols;
        self.rows = rows;
        // Re-wrap on-screen content to the new width instead of truncating it
        // (see [`reflow_screen`]); height-only / alt-screen changes fall back to
        // the plain non-reflow resize internally.
        reflow_screen(&mut self.parser, rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        // Consume the child's post-SIGWINCH redraw at *this* width before we
        // return, so a subsequent fast resize can't reparse it at a new width.
        self.settle_after_resize();
    }

    /// Consume the child's post-SIGWINCH redraw so a rapid follow-up resize
    /// can't reparse those cursor-relative bytes against a grid whose width has
    /// since changed (quadraui#437, blocking #2).
    ///
    /// This blocks the caller, but only for a **bounded** window, and it runs
    /// in two phases so it neither gives up too early nor stalls forever:
    ///
    /// 1. **Wait for the redraw to start.** Until the first byte arrives we
    ///    wait up to [`RESIZE_SETTLE_FIRST`]. This is the fix for the original
    ///    bug — the child needs time just to *react* to SIGWINCH, and treating
    ///    that initial silence as "settled" (the old single-[`RESIZE_SETTLE_IDLE`]
    ///    loop) returned before consuming anything, leaving the redraw to be
    ///    reparsed later at the wrong width. If no byte arrives in this window
    ///    the child isn't redrawing and we return.
    /// 2. **Drain the redraw.** Once output is flowing we consume until the
    ///    reader thread has been idle for [`RESIZE_SETTLE_IDLE`] (redraw done).
    ///
    /// The whole thing is capped at [`RESIZE_SETTLE_MAX`] so a continuously
    /// chatty child (e.g. `yes`) can't stall the UI. Every byte it consumes is
    /// parsed at the *current* grid width — exactly the width the child
    /// computed the redraw for.
    ///
    /// The blocking/timing policy lives in [`collect_post_resize_output`] (a
    /// free function so it can be unit-tested against a synthetic channel
    /// without a real PTY — see the `settle_*` tests); this method only wires it
    /// to the parser. Chunks are captured then processed in arrival order, so
    /// vt100 parse time never inflates the idle-gap measurement.
    fn settle_after_resize(&mut self) {
        for chunk in collect_post_resize_output(&self.rx) {
            self.process_with_capture(&chunk);
        }
    }

    // ── Scrollback ───────────────────────────────────────────────────────────

    /// Number of scrollback rows captured so far.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Set the scroll offset.
    ///
    /// `0` = live view. `history_len()` = oldest available row at the top.
    /// Clamped to `[0, history_len()]`.
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset.min(self.history.len());
        // Keep the vt100 parser at the live view at all times.
        self.parser.screen_mut().set_scrollback(0);
    }

    /// Scroll up into history by `n` rows.
    pub fn scroll_up(&mut self, n: usize) {
        let new = self.scroll_offset.saturating_add(n);
        self.set_scroll_offset(new);
    }

    /// Scroll down toward the live view by `n` rows.
    pub fn scroll_down(&mut self, n: usize) {
        let new = self.scroll_offset.saturating_sub(n);
        self.set_scroll_offset(new);
    }

    /// Return to the live view (`scroll_offset = 0`).
    pub fn scroll_reset(&mut self) {
        self.set_scroll_offset(0);
    }

    // ── Selection helpers ─────────────────────────────────────────────────────

    /// Extract selected text, blending history and live screen correctly.
    ///
    /// Selection coordinates (`TerminalSelection`) are always in
    /// **display-row** space (0-based from the visible top), which is the
    /// same coordinate system `build_rows` uses.  This means the function
    /// works at any `scroll_offset`, including inside scrollback history.
    ///
    /// Returns `None` when there is no active selection.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let (r0, c0, r1, c1) = normalize_selection(sel);
        let mut lines: Vec<String> = Vec::new();
        for row in r0..=r1 {
            let mut line = String::new();
            let col_start = if row == r0 { c0 } else { 0 };
            let col_end = if row == r1 {
                c1
            } else {
                self.cols.saturating_sub(1)
            };
            for col in col_start..=col_end {
                line.push_str(&self.cell_content_at_display_row(row as usize, col));
            }
            lines.push(line.trim_end().to_string());
        }
        Some(lines.join("\n"))
    }

    /// Return the text content of the cell at the given **display row** and
    /// column, resolving from `self.history` when
    /// `display_r < self.scroll_offset` or from the live vt100 screen
    /// otherwise.
    ///
    /// Returns a single space for empty or out-of-range cells, matching the
    /// convention used by [`selected_text`](Self::selected_text).  This is
    /// the canonical blending helper — both `selected_text` and `build_rows`
    /// use the same mapping so the two can never drift apart.
    fn cell_content_at_display_row(&self, display_r: usize, col: u16) -> String {
        let scroll_offset = self.scroll_offset;
        let hist_len = self.history.len();

        if display_r < scroll_offset {
            let hist_idx_signed = hist_len as isize - scroll_offset as isize + display_r as isize;
            if hist_idx_signed >= 0 {
                if let Some(hist_row) = self.history.get(hist_idx_signed as usize) {
                    let ch = hist_row.get(col as usize).copied().unwrap_or_default().ch;
                    return ch.to_string();
                }
            }
            " ".to_string()
        } else {
            let live_r = (display_r - scroll_offset) as u16;
            let screen = self.parser.screen();
            if let Some(cell) = screen.cell(live_r, col) {
                let s = cell.contents();
                if s.is_empty() {
                    " ".to_string()
                } else {
                    s.to_string()
                }
            } else {
                " ".to_string()
            }
        }
    }

    // ── Snapshot ──────────────────────────────────────────────────────────────

    /// Build a paint snapshot for [`Backend::draw_terminal`].
    ///
    /// Pass `Some(scrollbar)` (e.g. from [`scrollbar_state`](Self::scrollbar_state))
    /// to show a scrollbar when the history is non-empty.
    pub fn to_terminal(&self, id: WidgetId, scrollbar: Option<TerminalScrollbar>) -> Terminal {
        Terminal {
            id,
            cells: self.build_rows(true),
            scrollbar,
        }
    }

    /// Build a `TerminalScrollbar` reflecting the current scrollback state.
    ///
    /// - `scrollbar_width`: visual width in backend-native units.
    ///   `None` → backend default (1 TUI cell, ~8 px GTK).
    ///
    /// The scrollbar uses `inverted = true` so that offset `0` (live view)
    /// places the thumb at the track bottom, matching normal terminal UX.
    pub fn scrollbar_state(&self, scrollbar_width: Option<u16>) -> TerminalScrollbar {
        let total = self.history.len() + self.rows as usize;
        TerminalScrollbar {
            total_lines: total,
            visible_lines: self.rows as usize,
            scroll_offset: self.scroll_offset,
            inverted: true,
            width: scrollbar_width,
        }
    }

    // ── Private implementation ────────────────────────────────────────────────

    /// Process a data chunk, splitting at `rows`-newline boundaries so
    /// that each sub-chunk causes at most `rows` lines to scroll off
    /// the live screen — the safe maximum for vt100's scrollback read-back.
    ///
    /// While the child is on the **alternate screen** (vim / tmux / less /
    /// claude) `capture_scrolled_rows` is a no-op: alt-screen churn must
    /// never pollute the shell's scrollback (quadraui #335).
    fn process_with_capture(&mut self, data: &[u8]) {
        let max_nl = self.rows as usize;
        let mut start = 0;
        let mut nl_count = 0;

        for (i, &b) in data.iter().enumerate() {
            if b == b'\n' {
                nl_count += 1;
                if nl_count >= max_nl {
                    let chunk = &data[start..=i];
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        self.parser.process(chunk);
                    }))
                    .is_err()
                    {
                        crate::diagnostics::emit(
                            "quadraui: vt100 parser panic in process_with_capture; \
                             dropping chunk, session kept alive",
                        );
                    }
                    self.capture_scrolled_rows(nl_count);
                    start = i + 1;
                    nl_count = 0;
                }
            }
        }
        if start < data.len() {
            let chunk = &data[start..];
            let remaining_nl = chunk.iter().filter(|&&b| b == b'\n').count();
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.parser.process(chunk);
            }))
            .is_err()
            {
                crate::diagnostics::emit(
                    "quadraui: vt100 parser panic in process_with_capture; \
                     dropping chunk, session kept alive",
                );
            }
            if remaining_nl > 0 {
                self.capture_scrolled_rows(remaining_nl);
            }
        }
    }

    /// Read the rows that just scrolled off the live screen top and append
    /// them to `self.history`.
    ///
    /// Temporarily shifts the vt100 viewport to see the rows that just
    /// scrolled off (they're still in the vt100 internal scrollback at
    /// this point), reads them, then restores the live view.
    ///
    /// Skipped entirely while the child is on the alternate screen — vt100
    /// keeps a separate scrollback buffer for the alt grid and full-TUI
    /// apps re-render every frame, so capturing those rows would both leak
    /// frame churn into the shell's scrollback **and** read from the wrong
    /// grid. The check is evaluated *after* `parser.process(...)` so the
    /// mode-switch escape (`ESC[?1049h` / `ESC[?1049l`) takes effect first.
    fn capture_scrolled_rows(&mut self, n_newlines: usize) {
        if self.parser.screen().alternate_screen() {
            return;
        }
        let to_capture = n_newlines.min(self.rows as usize);
        self.parser.screen_mut().set_scrollback(to_capture);
        {
            let screen = self.parser.screen();
            for r in 0..to_capture as u16 {
                let row: Vec<HistCell> = (0..self.cols)
                    .map(|c| match screen.cell(r, c) {
                        Some(cell) => {
                            let raw = cell.contents();
                            HistCell {
                                ch: raw.chars().next().unwrap_or(' '),
                                fg: cell.fgcolor(),
                                bg: cell.bgcolor(),
                                bold: cell.bold(),
                                italic: cell.italic(),
                                underline: cell.underline(),
                            }
                        }
                        None => HistCell::default(),
                    })
                    .collect();
                if self.history_capacity > 0 && self.history.len() >= self.history_capacity {
                    self.history.pop_front();
                }
                self.history.push_back(row);
            }
        }
        self.parser.screen_mut().set_scrollback(0);
    }

    /// Build the full cell grid for the current view.
    ///
    /// Blends history rows (when `scroll_offset > 0`) with live screen rows.
    /// `cursor_active` controls whether the vt100 cursor position is marked
    /// `is_cursor = true` in the snapshot.
    fn build_rows(&self, cursor_active: bool) -> Vec<Vec<TerminalCell>> {
        let screen = self.parser.screen();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let rows_count = self.rows as usize;
        let cols_count = self.cols as usize;
        let scroll_offset = self.scroll_offset;
        let hist_len = self.history.len();

        // Selection coordinates are always in display-row space (0 = visible
        // top), matching the coordinate system expected by a host's
        // pixel-to-cell mapping helper.  The gate on `scroll_offset == 0` that
        // previously existed here was overly conservative: the `display_r`
        // comparison below is correct at any offset.
        let sel_bounds = self.selection.as_ref().map(normalize_selection);

        // Closure: is the cell at (display_r, cu) inside the selection?
        // Uses display-row coordinates throughout — no offset math needed.
        let is_selected = |display_r: usize, cu: u16| -> bool {
            sel_bounds.is_some_and(|(r0, c0, r1, c1)| {
                let dr = display_r as u16;
                if r0 == r1 {
                    dr == r0 && cu >= c0 && cu <= c1
                } else if dr == r0 {
                    cu >= c0
                } else if dr == r1 {
                    cu <= c1
                } else {
                    dr > r0 && dr < r1
                }
            })
        };

        (0..rows_count)
            .map(|display_r| {
                (0..cols_count)
                    .map(|c| {
                        let cu = c as u16;

                        let (ch, fg, bg, bold, italic, underline, is_cursor, selected) =
                            if display_r < scroll_offset {
                                // Row is in the scrollback history.
                                let hist_idx_signed =
                                    hist_len as isize - scroll_offset as isize + display_r as isize;
                                if hist_idx_signed >= 0 {
                                    if let Some(hist_row) =
                                        self.history.get(hist_idx_signed as usize)
                                    {
                                        let hc = hist_row.get(c).copied().unwrap_or_default();
                                        (
                                            hc.ch,
                                            map_vt100_color(hc.fg, false),
                                            map_vt100_color(hc.bg, true),
                                            hc.bold,
                                            hc.italic,
                                            hc.underline,
                                            false,
                                            is_selected(display_r, cu),
                                        )
                                    } else {
                                        (
                                            ' ',
                                            (229, 229, 229),
                                            (30, 30, 30),
                                            false,
                                            false,
                                            false,
                                            false,
                                            is_selected(display_r, cu),
                                        )
                                    }
                                } else {
                                    (
                                        ' ',
                                        (229, 229, 229),
                                        (30, 30, 30),
                                        false,
                                        false,
                                        false,
                                        false,
                                        false,
                                    )
                                }
                            } else {
                                // Row is in the live vt100 screen.
                                let live_r = (display_r - scroll_offset) as u16;
                                let (ch, fg, bg, bold, italic, underline) =
                                    if let Some(cell) = screen.cell(live_r, cu) {
                                        let contents = cell.contents();
                                        let ch = contents.chars().next().unwrap_or(' ');
                                        (
                                            ch,
                                            map_vt100_color(cell.fgcolor(), false),
                                            map_vt100_color(cell.bgcolor(), true),
                                            cell.bold(),
                                            cell.italic(),
                                            cell.underline(),
                                        )
                                    } else {
                                        (' ', (229, 229, 229), (30, 30, 30), false, false, false)
                                    };

                                let is_cursor = !self.exited
                                    && scroll_offset == 0
                                    && cursor_active
                                    && live_r == cursor_row
                                    && cu == cursor_col;

                                (
                                    ch,
                                    fg,
                                    bg,
                                    bold,
                                    italic,
                                    underline,
                                    is_cursor,
                                    is_selected(display_r, cu),
                                )
                            };

                        TerminalCell {
                            ch,
                            fg: Color::rgb(fg.0, fg.1, fg.2),
                            bg: Color::rgb(bg.0, bg.1, bg.2),
                            bold,
                            italic,
                            underline,
                            selected,
                            is_cursor,
                            is_find_match: false,
                            is_find_active: false,
                        }
                    })
                    .collect()
            })
            .collect()
    }
}

// ── TerminalManager ───────────────────────────────────────────────────────────

/// Multi-tab terminal manager.
///
/// Owns a `Vec<TerminalSession>` and tracks the active index. Provides
/// CRUD methods for sessions and a [`poll_all`](Self::poll_all) helper
/// for the event-loop tick.
///
/// ## Keybindings and coupling
///
/// Alt+1-9 tab switching, Ctrl+W close, and split-pane keybindings are
/// **not** part of this type. Consuming apps implement those in their
/// `AppLogic::handle` method and call the appropriate methods here.
pub struct TerminalManager {
    /// All open sessions.
    pub sessions: Vec<TerminalSession>,
    /// Index of the currently-active session.
    pub active: usize,
}

impl TerminalManager {
    /// Create a new manager with no open sessions.
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            active: 0,
        }
    }

    /// Reference to the active session, or `None` if empty.
    pub fn active_session(&self) -> Option<&TerminalSession> {
        self.sessions.get(self.active)
    }

    /// Mutable reference to the active session, or `None` if empty.
    pub fn active_session_mut(&mut self) -> Option<&mut TerminalSession> {
        self.sessions.get_mut(self.active)
    }

    /// Spawn a new session and make it active. Returns the new index.
    pub fn new_session(
        &mut self,
        cols: u16,
        rows: u16,
        shell: &str,
        cwd: &Path,
        history_capacity: usize,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let session = TerminalSession::spawn(cols, rows, shell, cwd, history_capacity)?;
        self.sessions.push(session);
        self.active = self.sessions.len() - 1;
        Ok(self.active)
    }

    /// Close the active session. The adjacent session becomes active;
    /// `active` is reset to `0` when the last session is removed.
    pub fn close_active(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.sessions.remove(self.active);
        if self.sessions.is_empty() {
            self.active = 0;
        } else {
            self.active = self.active.min(self.sessions.len() - 1);
        }
    }

    /// Switch to session `idx` (clamped to `[0, len-1]`).
    pub fn switch_to(&mut self, idx: usize) {
        if !self.sessions.is_empty() {
            self.active = idx.min(self.sessions.len() - 1);
        }
    }

    /// Poll every session for PTY output. Returns `true` when any
    /// session produced new data (the caller should schedule a repaint).
    pub fn poll_all(&mut self) -> bool {
        self.sessions.iter_mut().fold(false, |acc, s| {
            let changed = s.poll();
            acc || changed
        })
    }

    /// Number of open sessions.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// `true` when there are no open sessions.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Utility ───────────────────────────────────────────────────────────────────

/// Return the user's preferred shell.
///
/// Reads `$SHELL`; falls back to `/bin/bash` on Unix and
/// `powershell.exe` on Windows.
pub fn default_shell() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        return shell;
    }
    #[cfg(target_os = "windows")]
    {
        "powershell.exe".to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        "/bin/bash".to_string()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Regression: vt100 0.15.2 alt-screen resize cursor-clamp panic (#397) ─

    /// Reproduces the exact crash sequence from issue #397:
    ///
    ///  1. Cursor is low in a tall grid (row ≥ future height after resize).
    ///  2. Child enters the alternate screen (`ESC[?1049h`). vt100 DEC-saves the
    ///     cursor position (`decsc`) — saving row 35.
    ///  3. The pane is resized to 20 rows. On vt100 0.15.2 `Grid::set_size`
    ///     clamped the live `pos` but **not** `saved_pos` — so `saved_pos.row`
    ///     remained 35.
    ///  4. Child exits the alternate screen (`ESC[?1049l`). vt100 restores the
    ///     saved cursor (`decrc`) — restoring row 35 onto a 20-row grid.
    ///  5. Any printable byte → `Grid::drawing_cell(35).unwrap()` → `None` → panic.
    ///
    /// vt100 ≥ 0.16 clamps `saved_pos.row` / `saved_pos.col` inside `set_size`,
    /// so step 5 must **not** panic. The test also asserts the restored cursor row
    /// is within the new bounds.
    ///
    /// **Fails on vt100 0.15.2** (panics at step 5). **Passes on vt100 0.16.x**.
    /// No wide chars are involved — the trigger is purely cursor position + resize.
    #[test]
    fn vt100_alt_screen_resize_cursor_clamp_no_panic() {
        // 40-row × 80-col grid.  Row indices are 0-based inside vt100.
        let mut p = vt100::Parser::new(40, 80, 0);

        // Move cursor to row 35 (1-indexed: ESC[36;1H).
        // Row 35 ≥ the future 20-row height, so restoring it unpatched → OOB.
        p.process(b"\x1b[36;1H");
        let (row, col) = p.screen().cursor_position();
        assert_eq!(row, 35, "cursor must be at row 35 before alt-screen enter");
        assert_eq!(col, 0);

        // Enter alternate screen — DEC saves cursor (saves row=35).
        p.process(b"\x1b[?1049h");
        assert!(p.screen().alternate_screen(), "must be on alternate screen");

        // Resize to 20 rows.  On 0.15.2 saved_pos.row remains 35 — this is the bug.
        p.screen_mut().set_size(20, 80);

        // Exit alternate screen — restores saved cursor (row=35 on 0.15.2 → OOB panic).
        p.process(b"\x1b[?1049l");
        assert!(
            !p.screen().alternate_screen(),
            "must have left alternate screen"
        );

        // Print a printable byte.  On 0.15.2 this panics at Grid::drawing_cell.
        p.process(b"X"); // must not panic

        // The restored cursor row MUST be clamped inside the new 20-row grid.
        let (restored_row, _) = p.screen().cursor_position();
        assert!(
            restored_row < 20,
            "restored cursor row {restored_row} must be < 20 (new grid height)"
        );
    }

    /// The same #397 crash sequence, driven end-to-end through a **real
    /// `TerminalSession`** rather than a bare `vt100::Parser`.
    ///
    /// This is the test that actually covers the code this fix touches:
    /// `TerminalSession::resize()` (which calls `screen_mut().set_size()` via
    /// the 0.16 API) and `TerminalSession::poll()` →
    /// `process_with_capture()` → the `catch_unwind`-guarded
    /// `parser.process()` call sites. The raw-parser test above proves the
    /// vt100 bump cures the underlying bug; this one proves the session
    /// survives the sequence and stays usable — the issue's acceptance bar.
    ///
    /// A panic inside `process_with_capture` is caught by `catch_unwind`, so
    /// a regression there surfaces as a *dead session* (the final "still
    /// alive" assertion fails) rather than as a test-harness panic.
    #[test]
    #[cfg(unix)]
    fn session_alt_screen_resize_cursor_clamp_no_panic() {
        let cwd = std::env::temp_dir();
        // 40 rows: tall enough to park the cursor at row 35, well below the
        // 20-row height we shrink to mid-alt-screen.
        let mut sess =
            TerminalSession::spawn(80, 40, "/bin/sh", &cwd, 1000).expect("failed to spawn /bin/sh");

        // Steps 1+2: park the cursor at row 35 (ESC[36;1H, 1-indexed) and enter
        // the alternate screen — vt100 DEC-saves the cursor, recording row 35.
        sess.send_str("printf '\\033[36;1H\\033[?1049h'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.on_alt_screen()),
            "child should have entered the alternate screen"
        );

        // Step 3: the host shrinks the pane while the child is on the alt
        // screen. On vt100 0.15.2 this clamped `pos` but left `saved_pos.row`
        // at 35 — the bug.
        sess.resize(80, 20);
        assert_eq!(sess.rows(), 20, "resize should have taken effect");

        // Steps 4+5: leave the alternate screen (DEC-restores the saved cursor)
        // and print a byte at the restored position. On 0.15.2 the byte hit
        // `Grid::drawing_cell(35).unwrap()` → `None` → panic inside
        // `process_with_capture`.
        sess.send_str("printf '\\033[?1049lX'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| !s.on_alt_screen()),
            "child should have left the alternate screen"
        );

        // The session must still be usable: if the parser had panicked, the
        // chunk would have been dropped — and a regression in the catch_unwind
        // wiring (e.g. swallowing all subsequent input) shows up right here.
        sess.send_str("echo still-alive\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.screen_text().contains("still-alive")),
            "session must still process input after the alt-screen resize round-trip"
        );
        assert!(!sess.is_exited(), "session must not have died");

        sess.send_str("exit\n");
    }

    // ── Regression: vt100 0.15.2 wide-char column-boundary panic (#377) ─────

    /// Feed wide Unicode characters to the vt100 parser such that a 2-cell
    /// glyph straddles or lands exactly at the right column edge.  The
    /// patched vt100 must NOT panic; before the patch both `screen.rs:934`
    /// and `grid.rs:672` would fire `unwrap()` on `None`.
    ///
    /// We exercise three progressively harder layouts:
    ///  1. Glyphs that fill a row exactly (no boundary straddle).
    ///  2. A glyph whose first cell is the last column — forces `col_wrap`.
    ///  3. Many glyphs across multiple rows, interspersed with CR/LF, to
    ///     stress the scroll-path that triggers the grid.rs unwrap.
    #[test]
    fn vt100_wide_char_column_boundary_no_panic() {
        // '日' is U+65E5, width=2.  Three raw UTF-8 bytes: 0xe6 0x97 0xa5.
        let wide = "日";
        // '→' is U+2192, width=1 (sanity filler between wide chars).
        let narrow = "x";

        // Case 1: 10-column terminal, fill with exactly 5 wide chars (10 cols).
        {
            let mut p = vt100::Parser::new(3, 10, 0);
            let row: String = wide.repeat(5); // 10 cells, exactly full
            p.process(row.as_bytes());
            p.process(b"\r\n");
            p.process(row.as_bytes());
            // second row write must not panic even with a full-row wrap
            let _ = p.screen().cell(0, 0);
        }

        // Case 2: 10-column terminal, 4 wide chars (8 cells) then one more
        // wide char — the second cell of the 5th char would be col 9 → 10,
        // which is out of bounds, triggering col_wrap → drawing_row_mut.
        {
            let mut p = vt100::Parser::new(3, 10, 0);
            let four_wide: String = wide.repeat(4); // 8 cells
            p.process(four_wide.as_bytes());
            p.process(narrow.as_bytes()); // col 8, fills col 9 implicitly
                                          // Now feed a wide char starting at col 9 (last col) — the second
                                          // half would fall at col 10 → out of bounds → col_wrap fires.
            p.process(wide.as_bytes()); // must not panic
            let _ = p.screen().cell(0, 0);
        }

        // Case 3: stress-test with 80-column terminal and many wide chars
        // across scrolling rows (exit-repaint scenario).
        {
            let mut p = vt100::Parser::new(24, 80, 0);
            // 40 wide chars = 80 cells = exactly one full row
            let full_row: String = wide.repeat(40);
            for _ in 0..50 {
                p.process(full_row.as_bytes());
                p.process(b"\r\n");
            }
            // One more line where an odd wide char straddles the boundary.
            // 39 wide chars (78 cells) + 1 narrow (col 78) → next wide char
            // starts at col 79 (last col), second half would be at col 80.
            let boundary_line = format!("{}{}{}", wide.repeat(39), narrow, wide);
            p.process(boundary_line.as_bytes()); // must not panic
            let _ = p.screen().cell(0, 0);
        }
    }

    // ── Regression: destructive resize corrupts content (#437) ──────────────
    //
    // These drive the vendored vt100 parser's `set_size` directly (no PTY),
    // so they're deterministic. Before the reflow patch, shrinking the column
    // count truncated every row's cells and widening back padded with blanks
    // instead of restoring them — a shrink-then-expand window drag left the
    // shell output permanently truncated. The reflow re-wraps logical lines
    // so the round-trip is lossless.

    /// Collect the visible screen rows as trimmed strings.
    fn screen_lines(p: &vt100::Parser) -> Vec<String> {
        let (rows, _cols) = p.screen().size();
        (0..rows)
            .map(|r| {
                p.screen()
                    .rows(0, p.screen().size().1)
                    .nth(r as usize)
                    .unwrap_or_default()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn resize_height_only_preserves_content() {
        // Upstream vt100 0.16 `set_size` does not reflow wrapped lines, but a
        // height-only change (same width) must still keep existing rows intact.
        let mut p = vt100::Parser::new(24, 80, 1000);
        p.process(b"hello world\r\nsecond line");
        p.screen_mut().set_size(40, 80); // taller, same width
        let lines = screen_lines(&p);
        assert!(lines.iter().any(|l| l == "hello world"));
        assert!(lines.iter().any(|l| l == "second line"));
    }

    // ── Reflow round-trip (restored on vt100 0.16 via `reflow_screen`) ──────
    //
    // These drive `reflow_screen` — the public-API (snapshot → resize → replay)
    // reflow that replaced the vendored vt100 0.15.2 `Grid::set_size` patch when
    // develop moved to real vt100 0.16 (#397 deleted the vendored tree). vt100
    // 0.16's own `set_size` is *non-reflowing*: a bare `screen_mut().set_size()`
    // truncates every row on a column shrink and pads with blanks on widen, so a
    // shrink→expand window drag left the shell output permanently truncated.
    // `reflow_screen` re-wraps logical lines so the round-trip is lossless for
    // on-screen content. (Scrollback-deep shrinks are out of scope — see the
    // helper's doc comment.)

    #[test]
    fn resize_no_op_when_width_unchanged() {
        // Height-only change routes through `reflow_screen`'s cheap non-reflow
        // path (same width) and must not corrupt content.
        let mut p = vt100::Parser::new(24, 80, 1000);
        p.process(b"hello world\r\nsecond line");
        reflow_screen(&mut p, 40, 80); // taller, same width
        let lines = screen_lines(&p);
        assert!(lines.iter().any(|l| l == "hello world"));
        assert!(lines.iter().any(|l| l == "second line"));
    }

    #[test]
    fn resize_shrink_wraps_without_losing_tail() {
        // A single line longer than the shrunk width must wrap, not truncate.
        let mut p = vt100::Parser::new(24, 100, 1000);
        let long = "0123456789ABCDEFGHIJ0123456789ABCDEFGHIJ0123456789ABCDEFGHIJ";
        p.process(long.as_bytes());
        reflow_screen(&mut p, 24, 20);
        // The full text must still be reconstructable from the wrapped rows.
        let joined: String = screen_lines(&p).join("");
        assert!(
            joined.contains("GHIJ0123456789ABCDEFGHIJ"),
            "tail of long line lost on shrink: {joined:?}"
        );
        // And expanding back restores the single unbroken line.
        reflow_screen(&mut p, 24, 100);
        assert!(
            screen_lines(&p).iter().any(|l| l == long),
            "long line not restored on expand: {:?}",
            screen_lines(&p)
        );
    }

    #[test]
    fn resize_preserves_wide_chars() {
        // Wide (CJK) glyphs must survive a reflow round-trip without being
        // split from their continuation cell. One logical line, so no row ever
        // scrolls into scrollback across the shrink.
        let mut p = vt100::Parser::new(24, 100, 1000);
        let line = "日本語テスト-CJK-日本語テスト-CJK-日本語テスト";
        p.process(line.as_bytes());
        reflow_screen(&mut p, 24, 30);
        reflow_screen(&mut p, 24, 100);
        assert!(
            screen_lines(&p).iter().any(|l| l == line),
            "wide-char line not restored across resize: {:?}",
            screen_lines(&p)
        );
    }

    #[test]
    fn resize_shrink_then_expand_preserves_content() {
        // Several full-width lines. Shrink hard on width, then expand back —
        // every line must survive. The shrunk grid is kept tall enough that no
        // wrapped row scrolls into scrollback (the public-API reflow only
        // covers the visible screen; see `reflow_screen`'s doc comment).
        let mut p = vt100::Parser::new(24, 100, 1000);
        for i in 1..=6 {
            let line = format!("ROW{i:02}-abcdefghijklmnopqrstuvwxyz-0123456789-END\r\n");
            p.process(line.as_bytes());
        }
        for i in 1..=6 {
            let needle = format!("ROW{i:02}-abcdefghijklmnopqrstuvwxyz-0123456789-END");
            assert!(
                screen_lines(&p).iter().any(|l| l == &needle),
                "line {i} missing before resize"
            );
        }

        // Shrink width to 40 (each 47-char line wraps to 2 rows → 12 rows); keep
        // 20 rows so nothing scrolls off. Then expand back to the original size.
        reflow_screen(&mut p, 20, 40);
        reflow_screen(&mut p, 24, 100);

        for i in 1..=6 {
            let needle = format!("ROW{i:02}-abcdefghijklmnopqrstuvwxyz-0123456789-END");
            assert!(
                screen_lines(&p).iter().any(|l| l == &needle),
                "line {i} lost across shrink→expand round-trip: {:?}",
                screen_lines(&p)
            );
        }
    }

    #[test]
    fn resize_multistep_drag_leaves_no_ghost_rows() {
        // A window drag fires many intermediate resizes (shrink then expand).
        // After it settles back at the start size, the reflowed grid must be
        // byte-for-byte identical to the pre-drag grid — no duplicated prompt
        // lines, no orphaned fragments stranded on rows that should be blank
        // (quadraui#437: the on-screen "stale ~ / > ghost" symptom). Each line
        // is unique, so any duplication shows up as a repeated needle. Sizes are
        // chosen so wrapped rows never exceed the grid height — no scroll-off.
        let mut p = vt100::Parser::new(14, 40, 1000);
        for i in 1..=4 {
            p.process(format!("unique_line_{i:02}_content\r\n").as_bytes());
        }
        p.process(b"prompt$ "); // fresh prompt, cursor after it
        let before = screen_lines(&p);

        // Multi-step drag: shrink through several widths (each 21-char line
        // wraps to at most 2 rows → ≤ 9 rows, well under the 10-row floor),
        // then expand back to the start size.
        for (rows, cols) in [(11, 30), (10, 22), (10, 20), (12, 26), (13, 34), (14, 40)] {
            reflow_screen(&mut p, rows, cols);
        }
        let after = screen_lines(&p);

        assert_eq!(
            before, after,
            "grid changed across a shrink→expand drag round-trip (ghost rows)"
        );
        // Belt-and-braces: no unique content line appears more than once.
        for i in 1..=4 {
            let needle = format!("unique_line_{i:02}_content");
            let count = after.iter().filter(|l| l.contains(&needle)).count();
            assert_eq!(count, 1, "line {i} duplicated across resize: {after:?}");
        }
    }

    // ── Regression: shell-redraw-vs-reflow race on fast resize (#437) ───────
    //
    // A fast resize drag reflows the grid to width N and SIGWINCHes the shell;
    // the shell redraws its prompt for width N (cursor-relative bytes queued in
    // the reader thread), then the drag reflows the grid *again* to width M
    // before those bytes are processed. Applying a width-N redraw to a width-M
    // grid scattered duplicated prompt fragments across the pane that stayed
    // stuck until the next resize — reproduced on both TUI and GTK, so the bug
    // was in this shared engine, not a backend paint path.
    //
    // `resize()` now drains + processes queued PTY output at the current size
    // before reflowing, so each redraw lands on the grid it was computed for.
    //
    // This test locks the fix's *contract* deterministically: output that is
    // already queued in the reader channel when `resize()` is called must be
    // parsed **before** the grid changes size. We prove it by leaving a known
    // sentinel line queued-but-unprocessed (write it, wait for the reader
    // thread to enqueue it, but never `poll()`), then calling `resize()` to a
    // new width. If `resize()` drains first, the sentinel is already on the
    // (old-width) grid and survives the reflow, so it is visible immediately —
    // with no intervening `poll()`. Before the fix, `resize()` reflowed without
    // draining, so the queued width-N bytes were only parsed by a later
    // `poll()` against the width-M grid — the corruption path. The absence of
    // a `poll()` between the wait and the assertion is what makes this a real
    // guard rather than a timing coincidence.
    #[test]
    fn resize_drains_queued_output_before_reflow() {
        use std::thread::sleep;
        use std::time::{Duration, Instant};

        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        let mut sess = match TerminalSession::spawn(80, 24, "/bin/bash", &cwd, 1000) {
            Ok(s) => s,
            // No PTY available (sandbox): skip rather than fail spuriously.
            Err(_) => return,
        };

        // Poll until `needle` shows up or we time out (drains + processes).
        fn wait_for(sess: &mut TerminalSession, needle: &str, ms: u64) -> bool {
            let deadline = Instant::now() + Duration::from_millis(ms);
            loop {
                sess.poll();
                if sess.screen_text().contains(needle) {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
                sleep(Duration::from_millis(5));
            }
        }

        // Quiet, fixed prompt and wait for the shell to be ready.
        sess.write_input(b"PS1='RDYMARK$ '\n");
        if !wait_for(&mut sess, "RDYMARK$", 4000) {
            return; // cold shell — don't assert
        }

        // Emit a unique sentinel, then wait for the reader thread to have it
        // enqueued WITHOUT processing it into the parser. We can't peek the
        // channel, so give it a generous, machine-load-robust window.
        sess.write_input(b"echo SENTINEL_QZX_9137\n");
        sleep(Duration::from_millis(400));

        // Precondition: the sentinel is NOT on the grid yet (still queued),
        // because we have not polled since writing it. If the shell were
        // somehow already drained, the test still holds but proves less; guard
        // against a flaky environment by only asserting the core property when
        // the sentinel is genuinely still queued.
        let queued = !sess.screen_text().contains("SENTINEL_QZX_9137");

        // The fix: resize() must drain + process the queued sentinel at the
        // current (80-col) width *before* reflowing to the new width. No poll()
        // is called here or before the assertion.
        sess.resize(50, 18);

        if queued {
            assert!(
                sess.screen_text().contains("SENTINEL_QZX_9137"),
                "resize() must drain queued PTY output before reflowing so \
                 in-flight redraws land on the width they were computed for \
                 (quadraui#437); sentinel was still queued yet resize() did not \
                 process it.\nscreen:\n{}",
                sess.screen_text()
            );
        }

        sess.write_input(b"exit\n");
    }

    #[test]
    fn resize_noop_same_size_is_cheap() {
        // Re-sending the current size must be a no-op (no reflow / SIGWINCH
        // churn), matching the guard added for the #437 drag fix.
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        let mut sess = match TerminalSession::spawn(80, 24, "/bin/bash", &cwd, 100) {
            Ok(s) => s,
            Err(_) => return,
        };
        assert_eq!((sess.cols(), sess.rows()), (80, 24));
        sess.resize(80, 24); // no-op
        assert_eq!((sess.cols(), sess.rows()), (80, 24));
        sess.resize(100, 30);
        assert_eq!((sess.cols(), sess.rows()), (100, 30));
    }

    // ── Regression: rapid multi-step resize drag (quadraui#437, blocking #2) ──
    //
    // The failure the earlier drain-only fix could NOT catch: a fast drag
    // fires several `resize()` calls back-to-back with no settle time between
    // them. Each resize SIGWINCHes the shell; the shell's redraw for that
    // width is still being written when the *next* resize changes the width
    // again. If that redraw is parsed at the wrong width, its cursor-relative
    // moves scatter duplicated prompt/echo fragments that stick across rows.
    //
    // `resize()` now waits (bounded) for the reader thread to go quiescent
    // after each SIGWINCH (`settle_after_resize`), so every redraw is parsed
    // at the width it was computed for even under a no-pause drag. This test
    // drives exactly that pattern — many resizes with no sleeps between them —
    // and asserts a unique sentinel line never ends up duplicated on the grid.
    #[test]
    fn resize_rapid_multistep_drag_leaves_no_ghosts() {
        use std::thread::sleep;
        use std::time::{Duration, Instant};

        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        let mut sess = match TerminalSession::spawn(80, 24, "/bin/bash", &cwd, 2000) {
            Ok(s) => s,
            Err(_) => return, // no PTY in sandbox — skip rather than fail
        };

        fn wait_for(sess: &mut TerminalSession, needle: &str, ms: u64) -> bool {
            let deadline = Instant::now() + Duration::from_millis(ms);
            loop {
                sess.poll();
                if sess.screen_text().contains(needle) {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
                sleep(Duration::from_millis(5));
            }
        }

        // Fixed, unique prompt so its redraw is what churns on SIGWINCH.
        sess.write_input(b"PS1='GHOSTMK> '\n");
        if !wait_for(&mut sess, "GHOSTMK>", 4000) {
            return; // cold shell — don't assert
        }

        // Emit a unique sentinel line, then let it land on the grid. The
        // sentinel is assembled by `printf` from two fragments so the literal
        // `GHOST_5571` appears ONLY in the command's *output*, never in the
        // typed command line the tty echoes back — otherwise the harmless
        // command echo would inflate the occurrence count and mask/forge a
        // ghost. Any extra copy on the grid is therefore a genuine duplication.
        let needle = "GHOST_5571";
        sess.write_input(b"printf 'GHO%s\\n' ST_5571\n");
        if !wait_for(&mut sess, needle, 4000) {
            return;
        }

        // The actual failure trigger: a rapid multi-step drag — several
        // resizes with NO settle time between the calls. `resize()` itself is
        // responsible for consuming each width's redraw before returning.
        for (cols, rows) in [(60, 20), (48, 16), (40, 14), (52, 18), (68, 22), (80, 24)] {
            sess.resize(cols, rows);
        }

        // Drain anything still in flight and let the grid settle.
        for _ in 0..40 {
            sess.poll();
            sleep(Duration::from_millis(10));
        }

        // The output line must appear at most once — the #437 ghost duplicated
        // it (and scattered prompt fragments) across many rows.
        let screen = sess.screen_text();
        let count = screen.matches(needle).count();
        assert!(
            count <= 1,
            "rapid multi-step resize drag duplicated a unique line \
             ({count} copies) — the #437 resize ghost. screen:\n{screen}"
        );
        assert_eq!((sess.cols(), sess.rows()), (80, 24));

        sess.write_input(b"exit\n");
    }

    // ── Regression: the post-SIGWINCH settle timing policy (blocking #2) ─────
    //
    // The deterministic core of blocking #2, isolated from any shell. A rapid
    // resize drag corrupts the grid when a resize's SIGWINCH redraw is parsed
    // at a *later* width because the next resize changed the width before that
    // redraw was consumed. `resize()` closes the race by *waiting* (bounded)
    // for the child's post-SIGWINCH output before returning, via
    // `collect_post_resize_output`.
    //
    // These tests drive that timing policy against a synthetic
    // `std::sync::mpsc::channel` — no PTY, no shell. That matters: the previous
    // real-shell test relied on a bash `WINCH` trap firing during the resize,
    // but bash defers a user WINCH trap while sitting at the readline prompt
    // (it redraws the prompt line but runs the trap only after the next command
    // is submitted), so the trap output never arrived and the test was a flaky
    // false negative. The synthetic channel reproduces the exact bug — a child
    // that reacts to SIGWINCH *later than the idle gap* — deterministically.

    #[test]
    fn settle_waits_past_idle_gap_for_a_delayed_redraw() {
        use std::sync::mpsc::channel;
        use std::time::Duration;

        // The child reacts to SIGWINCH after `child_delay` — deliberately
        // *longer* than the idle gap (RESIZE_SETTLE_IDLE) so the ORIGINAL bug
        // (return after one idle gap having consumed nothing) is exercised, yet
        // comfortably *shorter* than the first-byte window (RESIZE_SETTLE_FIRST)
        // so the fix captures it. `sleep` only ever overshoots, so the "> idle"
        // side is unconditional; the margin below FIRST (4×) absorbs load.
        let child_delay = Duration::from_millis(20);
        assert!(
            RESIZE_SETTLE_IDLE < child_delay,
            "delay must exceed the idle gap"
        );
        assert!(
            child_delay * 3 < RESIZE_SETTLE_FIRST,
            "delay needs headroom below the first-byte window"
        );

        let (tx, rx) = channel::<Vec<u8>>();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(child_delay);
            let _ = tx.send(b"post-winch-redraw".to_vec());
            // Keep the channel connected briefly so the drain phase ends on a
            // genuine idle gap (Timeout), not a Disconnected shortcut.
            std::thread::sleep(Duration::from_millis(40));
        });

        let chunks = collect_post_resize_output(&rx);
        sender.join().unwrap();

        assert_eq!(
            chunks.concat(),
            b"post-winch-redraw",
            "settle must wait past the initial idle gap for a child that reacts \
             to SIGWINCH slowly — the blocking #2 ghost bug"
        );
    }

    #[test]
    fn settle_returns_bounded_when_child_stays_silent() {
        use std::sync::mpsc::channel;
        use std::time::{Duration, Instant};

        // A child that ignores SIGWINCH (emits nothing). Settle must not hang:
        // it waits up to RESIZE_SETTLE_FIRST for a first byte, then gives up.
        // `_tx` is held so the channel is NOT disconnected — proving the return
        // is driven by the first-byte timeout, not a Disconnected shortcut.
        let (_tx, rx) = channel::<Vec<u8>>();

        let start = Instant::now();
        let chunks = collect_post_resize_output(&rx);
        let elapsed = start.elapsed();

        assert!(chunks.is_empty(), "silent child must yield no chunks");
        assert!(
            elapsed >= RESIZE_SETTLE_FIRST.saturating_sub(Duration::from_millis(10)),
            "settle must actually wait ~RESIZE_SETTLE_FIRST for a first byte \
             (waited {elapsed:?})"
        );
        assert!(
            elapsed <= RESIZE_SETTLE_MAX + Duration::from_millis(80),
            "settle must stay bounded and not hang (waited {elapsed:?})"
        );
    }

    #[test]
    fn settle_drains_a_multi_chunk_redraw_then_stops() {
        use std::sync::mpsc::channel;
        use std::time::Duration;

        // A redraw delivered as several chunks. Settle must capture ALL of them
        // in order and stop once the child goes quiet — not truncate mid-redraw.
        // The chunks are sent back-to-back (no inter-send sleep) so the test has
        // no dependence on a per-chunk gap staying under the idle window — the
        // only timing requirement is the final hold exceeding the idle gap.
        let (tx, rx) = channel::<Vec<u8>>();
        let sender = std::thread::spawn(move || {
            let _ = tx.send(b"aaa".to_vec());
            let _ = tx.send(b"bbb".to_vec());
            let _ = tx.send(b"ccc".to_vec());
            // Hold the channel open past the idle gap so the drain ends via a
            // Timeout (idle), not a Disconnected, exercising "redraw complete".
            std::thread::sleep(Duration::from_millis(40));
        });

        let chunks = collect_post_resize_output(&rx);
        sender.join().unwrap();

        assert_eq!(
            chunks.concat(),
            b"aaabbbccc",
            "settle must drain the whole multi-chunk redraw in arrival order"
        );
    }

    #[test]
    fn xterm_256_system_colors() {
        // Colour 0 = black.
        assert_eq!(xterm_256_color(0), (0, 0, 0));
        // Colour 15 = white.
        assert_eq!(xterm_256_color(15), (255, 255, 255));
    }

    #[test]
    fn xterm_256_cube_first() {
        // Colour 16: r=0, g=0, b=0 in the cube → (0, 0, 0).
        assert_eq!(xterm_256_color(16), (0, 0, 0));
    }

    #[test]
    fn xterm_256_cube_pure_red() {
        // r=5, g=0, b=0 → index 16 + 36*5 = 196.
        let (r, g, b) = xterm_256_color(196);
        assert_eq!(r, 55 + 5 * 40); // 255
        assert_eq!(g, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn xterm_256_greyscale() {
        // Colour 232 = darkest grey (8, 8, 8).
        assert_eq!(xterm_256_color(232), (8, 8, 8));
        // Colour 255 = lightest grey (238 = 8 + 23*10).
        assert_eq!(xterm_256_color(255), (238, 238, 238));
    }

    #[test]
    fn map_vt100_default_colors() {
        let (r, g, b) = map_vt100_color(vt100::Color::Default, false);
        assert_eq!((r, g, b), (229, 229, 229)); // fg default
        let (r, g, b) = map_vt100_color(vt100::Color::Default, true);
        assert_eq!((r, g, b), (30, 30, 30)); // bg default
    }

    #[test]
    fn map_vt100_rgb() {
        let (r, g, b) = map_vt100_color(vt100::Color::Rgb(1, 2, 3), false);
        assert_eq!((r, g, b), (1, 2, 3));
    }

    #[test]
    fn normalize_selection_already_ordered() {
        let sel = TerminalSelection {
            start_row: 1,
            start_col: 5,
            end_row: 3,
            end_col: 10,
        };
        assert_eq!(normalize_selection(&sel), (1, 5, 3, 10));
    }

    #[test]
    fn normalize_selection_reversed() {
        let sel = TerminalSelection {
            start_row: 3,
            start_col: 10,
            end_row: 1,
            end_col: 5,
        };
        assert_eq!(normalize_selection(&sel), (1, 5, 3, 10));
    }

    #[test]
    fn terminal_manager_new_is_empty() {
        let mgr = TerminalManager::new();
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
        assert!(mgr.active_session().is_none());
    }

    #[test]
    fn terminal_manager_close_active_on_empty_is_noop() {
        let mut mgr = TerminalManager::new();
        mgr.close_active(); // should not panic
        assert!(mgr.is_empty());
    }

    #[test]
    fn terminal_manager_switch_to_empty_is_noop() {
        let mut mgr = TerminalManager::new();
        mgr.switch_to(5); // should not panic
        assert_eq!(mgr.active, 0);
    }

    #[test]
    fn default_shell_is_nonempty() {
        let shell = default_shell();
        assert!(!shell.is_empty());
    }

    #[test]
    fn map_vt100_idx_196_is_pure_red() {
        // Colour index 196 = r=5,g=0,b=0 in the 6×6×6 cube → (255, 0, 0).
        let (r, g, b) = map_vt100_color(vt100::Color::Idx(196), false);
        assert_eq!((r, g, b), (255, 0, 0));
    }

    // ── cursor_visible integration tests (require a real PTY / Unix shell) ────

    /// After the child exits, `cursor_visible()` must return `false`
    /// regardless of the scroll offset.
    #[test]
    #[cfg(unix)]
    fn cursor_hidden_after_exit() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(80, 24, "/bin/sh", &cwd, 1000).expect("spawn failed");

        // Verify cursor is visible before exit (scroll_offset = 0, not exited).
        assert!(
            sess.cursor_visible(),
            "cursor should be visible before exit"
        );

        // Exit the shell.
        sess.send_str("exit 0\n");
        let exited = poll_until(&mut sess, 5000, |s| s.exited);
        assert!(exited, "shell did not exit within timeout");

        // After exit, cursor must be hidden.
        assert!(
            !sess.cursor_visible(),
            "cursor should be hidden after shell exits"
        );
    }

    /// When scrolled into history, `cursor_visible()` returns `false`.
    #[test]
    #[cfg(unix)]
    fn cursor_hidden_when_scrolled() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(80, 24, "/bin/sh", &cwd, 1000).expect("spawn failed");

        // Generate some history so we can scroll.
        for _ in 0..30 {
            sess.send_str("echo line\n");
        }
        let _ = poll_until(&mut sess, 5000, |s| s.history_len() > 0);

        // At live view (scroll_offset = 0), cursor should be visible.
        assert!(
            sess.cursor_visible(),
            "cursor should be visible at live view"
        );

        // Scroll up into history.
        sess.scroll_up(5);
        assert!(
            !sess.cursor_visible(),
            "cursor should be hidden when scrolled into history"
        );

        // Scroll back to live view.
        sess.scroll_reset();
        assert!(
            sess.cursor_visible(),
            "cursor should be visible again after scroll_reset"
        );

        sess.send_str("exit\n");
    }

    /// `bracketed_paste_enabled()` reflects the child's DEC private mode
    /// 2004 (`ESC[?2004h` / `ESC[?2004l`) — the input-readiness signal a
    /// programmatic driver waits on before injecting input (quadraui #343).
    #[test]
    #[cfg(unix)]
    fn bracketed_paste_enabled_tracks_mode_2004() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 24, "/bin/sh", &cwd, 1000).expect("failed to spawn /bin/sh");

        // Off until the child enables it.
        assert!(!sess.bracketed_paste_enabled());

        // Emit ESC[?2004h from the child (as interactive programs do once
        // their input prompt is ready). `\033` is a POSIX printf octal escape.
        sess.send_str("printf '\\033[?2004h'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.bracketed_paste_enabled()),
            "bracketed paste should be enabled after the child emits ESC[?2004h"
        );

        // And clears again on ESC[?2004l.
        sess.send_str("printf '\\033[?2004l'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| !s.bracketed_paste_enabled()),
            "bracketed paste should be disabled after the child emits ESC[?2004l"
        );

        sess.send_str("exit\n");
    }

    // ── Integration tests (require a real PTY / Unix shell) ───────────────────

    /// Helper: poll `session` until `predicate(&session)` is true or
    /// `max_ms` milliseconds elapse. Returns whether the predicate was satisfied.
    #[cfg(unix)]
    fn poll_until(
        sess: &mut TerminalSession,
        max_ms: u64,
        predicate: impl Fn(&TerminalSession) -> bool,
    ) -> bool {
        let start = std::time::Instant::now();
        let limit = std::time::Duration::from_millis(max_ms);
        while start.elapsed() < limit {
            sess.poll();
            if predicate(sess) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    /// Spawn a session that runs `echo hello` and verify:
    ///   1. `screen_text()` eventually contains "hello"
    ///   2. The process exits with code 0
    ///   3. `exit_code()` returns `Some(0)` after exit
    #[test]
    #[cfg(unix)]
    fn session_send_str_screen_text_and_exit_code() {
        let cwd = std::env::temp_dir();
        // Use sh -c so we get a short-lived process with predictable output.
        let mut sess =
            TerminalSession::spawn(80, 24, "/bin/sh", &cwd, 1000).expect("failed to spawn /bin/sh");

        // Before process exits, exit_code is None.
        // Send a command that outputs a known string then exits.
        sess.send_str("echo __marker__\nexit 0\n");

        // Poll until the marker appears in the visible screen or history,
        // or the process exits — whichever comes first (max 5 s).
        let found = poll_until(&mut sess, 5000, |s| {
            s.full_text().contains("__marker__") || s.exited
        });
        assert!(
            found,
            "marker '__marker__' never appeared in terminal output"
        );

        // Process should have exited with code 0.
        let exited = poll_until(&mut sess, 3000, |s| s.exited);
        assert!(exited, "process did not exit within timeout");
        assert_eq!(sess.exit_code(), Some(0), "expected exit code 0");

        // Verify the screen or scrollback text contained the marker.
        let text = sess.full_text();
        assert!(
            text.contains("__marker__"),
            "full_text() does not contain '__marker__'; got: {text:?}"
        );
    }

    /// Verify that `send_str` is equivalent to `write_input` for ASCII text.
    #[test]
    #[cfg(unix)]
    fn send_str_is_write_input_for_ascii() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 10, "/bin/sh", &cwd, 100).expect("failed to spawn /bin/sh");

        // Both methods should deliver the same bytes to the PTY.
        // We test that send_str doesn't panic or error.
        sess.send_str("echo ok\n");
        let _ = poll_until(&mut sess, 2000, |s| {
            s.full_text().contains("ok") || s.exited
        });
        // Just verify no panic occurred and the session is still valid.
        assert!(sess.cols > 0 && sess.rows > 0);
        sess.send_str("exit\n");
    }

    // ── Mouse → PTY encoding (SGR-1006) ──────────────────────────────────────

    /// SGR-1006 left-button press at (col=0, row=0) → `ESC[<0;1;1M`.
    #[test]
    fn encode_mouse_sgr_left_press_origin() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::Press,
            MouseButton::Left,
            0,
            0,
            Modifiers::default(),
        );
        assert_eq!(bytes, b"\x1b[<0;1;1M");
    }

    /// SGR-1006 left-button release uses lowercase `m` and reports the actual
    /// button (not the legacy `3` placeholder).
    #[test]
    fn encode_mouse_sgr_left_release_uses_lowercase_m() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::Release,
            MouseButton::Left,
            4,
            9,
            Modifiers::default(),
        );
        assert_eq!(bytes, b"\x1b[<0;5;10m");
    }

    /// Right-button press at (col=11, row=4) with shift → `ESC[<6;12;5M`
    /// (button 2 | shift bit 4 = 6).
    #[test]
    fn encode_mouse_sgr_right_press_with_shift() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::Press,
            MouseButton::Right,
            11,
            4,
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert_eq!(bytes, b"\x1b[<6;12;5M");
    }

    /// Wheel-up event at (col=0, row=0) → `ESC[<64;1;1M`. Wheel always uses
    /// `M` (no matching release) regardless of `button`.
    #[test]
    fn encode_mouse_sgr_wheel_up() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::WheelUp,
            MouseButton::Left,
            0,
            0,
            Modifiers::default(),
        );
        assert_eq!(bytes, b"\x1b[<64;1;1M");
    }

    /// Wheel-down event with ctrl modifier → `ESC[<81;1;1M` (65 | 16 = 81).
    #[test]
    fn encode_mouse_sgr_wheel_down_with_ctrl() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::WheelDown,
            MouseButton::Left,
            0,
            0,
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert_eq!(bytes, b"\x1b[<81;1;1M");
    }

    /// Motion event with left button held → bit 5 (32) set + button 0 = 32.
    #[test]
    fn encode_mouse_sgr_motion_with_left() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::Move,
            MouseButton::Left,
            7,
            2,
            Modifiers::default(),
        );
        assert_eq!(bytes, b"\x1b[<32;8;3M");
    }

    /// X1 extra-button press → `Cb = 128`.
    #[test]
    fn encode_mouse_sgr_x1_press() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::Press,
            MouseButton::X1,
            0,
            0,
            Modifiers::default(),
        );
        assert_eq!(bytes, b"\x1b[<128;1;1M");
    }

    /// X2 extra-button press → `Cb = 129`.
    #[test]
    fn encode_mouse_sgr_x2_press() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::Press,
            MouseButton::X2,
            0,
            0,
            Modifiers::default(),
        );
        assert_eq!(bytes, b"\x1b[<129;1;1M");
    }

    /// Middle button press → `Cb = 1` (lowercase `M` terminator).
    #[test]
    fn encode_mouse_sgr_middle_press() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::Press,
            MouseButton::Middle,
            4,
            2,
            Modifiers::default(),
        );
        // col 4 + 1 = 5, row 2 + 1 = 3
        assert_eq!(bytes, b"\x1b[<1;5;3M");
    }

    /// Middle button release → `Cb = 1` with lowercase `m` terminator.
    #[test]
    fn encode_mouse_sgr_middle_release() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::Release,
            MouseButton::Middle,
            4,
            2,
            Modifiers::default(),
        );
        assert_eq!(bytes, b"\x1b[<1;5;3m");
    }

    // ── Mouse → PTY encoding (require a real PTY) ────────────────────────────

    /// With no mouse reporting and no alt-screen, the engine refuses to
    /// forward — `forward_mouse` returns `false` and `encode_mouse`
    /// returns `None`. Local handling (selection / scrollback) keeps
    /// working unchanged.
    #[test]
    #[cfg(unix)]
    fn no_forward_without_reporting_or_alt_screen() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 10, "/bin/sh", &cwd, 100).expect("failed to spawn /bin/sh");

        assert!(!sess.mouse_reporting_enabled());
        assert!(!sess.on_alt_screen());
        assert!(!sess.should_forward_wheel());

        // Wheel + click are both refused.
        assert!(sess
            .encode_mouse(
                TerminalMouseKind::WheelUp,
                MouseButton::Left,
                0,
                0,
                Modifiers::default()
            )
            .is_none());
        assert!(!sess.forward_mouse(
            TerminalMouseKind::Press,
            MouseButton::Left,
            0,
            0,
            Modifiers::default()
        ));

        sess.send_str("exit\n");
    }

    /// After the child enables xterm mouse reporting (`ESC[?1000h`), the
    /// engine starts forwarding mouse events to the PTY.
    #[test]
    #[cfg(unix)]
    fn forwards_after_mouse_reporting_enabled() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 10, "/bin/sh", &cwd, 100).expect("failed to spawn /bin/sh");

        // Child enables mouse reporting.
        sess.send_str("printf '\\033[?1000h'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.mouse_reporting_enabled()),
            "mouse reporting did not turn on"
        );
        assert!(sess.should_forward_wheel());

        let bytes = sess.encode_mouse(
            TerminalMouseKind::WheelDown,
            MouseButton::Left,
            0,
            0,
            Modifiers::default(),
        );
        assert_eq!(bytes.as_deref(), Some(&b"\x1b[<65;1;1M"[..]));

        // Disable again — forwarding stops.
        sess.send_str("printf '\\033[?1000l'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| !s.mouse_reporting_enabled()),
            "mouse reporting did not turn off"
        );
        assert!(!sess.should_forward_wheel());

        sess.send_str("exit\n");
    }

    /// Alt-screen entry alone (without explicit mouse reporting) makes the
    /// engine forward wheel events to the child — the routing rule that
    /// fixes embedded `claude` / `tmux` / `less`.
    #[test]
    #[cfg(unix)]
    fn wheel_forwards_on_alt_screen_even_without_mouse_reporting() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 10, "/bin/sh", &cwd, 100).expect("failed to spawn /bin/sh");

        sess.send_str("printf '\\033[?1049h'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.on_alt_screen()),
            "did not enter alt-screen"
        );
        assert!(!sess.mouse_reporting_enabled()); // alt-screen alone
        assert!(sess.should_forward_wheel());

        // Press / Release / Move still gated on mouse reporting — alt-screen
        // alone is not enough for those (they only matter to apps that asked).
        assert!(sess
            .encode_mouse(
                TerminalMouseKind::Press,
                MouseButton::Left,
                0,
                0,
                Modifiers::default()
            )
            .is_none());

        // Wheel IS forwarded.
        assert!(sess
            .encode_mouse(
                TerminalMouseKind::WheelUp,
                MouseButton::Left,
                0,
                0,
                Modifiers::default()
            )
            .is_some());

        sess.send_str("printf '\\033[?1049l'\n");
        let _ = poll_until(&mut sess, 5000, |s| !s.on_alt_screen());
        sess.send_str("exit\n");
    }

    /// Scrollback must NOT grow while the child is on the alternate screen,
    /// and MUST resume growing after the child returns to the primary screen
    /// (quadraui #335 — without this, `claude` / `tmux` / `vim`
    /// frames leak into the shell's scrollback as cold-frame garbage).
    #[test]
    #[cfg(unix)]
    fn scrollback_skipped_on_alt_screen_resumes_on_primary() {
        let cwd = std::env::temp_dir();
        // Small screen so each `echo` line scrolls quickly.
        let mut sess =
            TerminalSession::spawn(40, 5, "/bin/sh", &cwd, 1000).expect("failed to spawn /bin/sh");

        // Drain initial prompt.
        let _ = poll_until(&mut sess, 1000, |_| false);

        // Enter alt-screen.
        sess.send_str("printf '\\033[?1049h'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.on_alt_screen()),
            "did not enter alt-screen"
        );

        // Record baseline AFTER we're on alt-screen (the `printf` command
        // line itself may have scrolled some rows on the primary screen).
        let baseline = sess.history_len();

        // Generate many lines of output. Each scrolls the alt-screen, but
        // alt-screen scrolls MUST NOT touch our history ring.
        for _ in 0..40 {
            sess.send_str("echo alt_content\n");
        }
        // Wait long enough for all the output to flush through the PTY.
        let _ = poll_until(&mut sess, 2000, |_| false);

        assert_eq!(
            sess.history_len(),
            baseline,
            "history grew while on alt-screen — alt-screen churn must not pollute shell scrollback"
        );

        // Exit alt-screen.
        sess.send_str("printf '\\033[?1049l'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| !s.on_alt_screen()),
            "did not exit alt-screen"
        );

        // Generate more lines on the primary screen — history SHOULD grow now.
        for _ in 0..40 {
            sess.send_str("echo primary_content\n");
        }
        assert!(
            poll_until(&mut sess, 5000, |s| s.history_len() > baseline + 5),
            "history did not resume growing after returning to primary screen"
        );

        sess.send_str("exit\n");
    }

    /// `application_cursor_keys()` reflects the child's DECCKM state
    /// (DEC private mode `?1h` / `?1l`, i.e. "application cursor keys").
    ///
    /// Full-TUI programs like vim, neovim, and claude set this mode on entry
    /// and clear it on exit. The key encoder must honour it so that arrow keys
    /// are sent as `ESC O A…D` (SS3) instead of `ESC [ A…D` (CSI) while the
    /// child is in application-cursor mode (quadraui #336).
    #[test]
    #[cfg(unix)]
    fn application_cursor_keys_tracks_decckm() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 24, "/bin/sh", &cwd, 1000).expect("failed to spawn /bin/sh");

        // Off by default.
        assert!(!sess.application_cursor_keys());

        // Emit ESC[?1h — the sequence programs like vim/claude use on entry.
        sess.send_str("printf '\\033[?1h'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.application_cursor_keys()),
            "DECCKM should be enabled after ESC[?1h"
        );

        // Emit ESC[?1l — the exit/restore sequence.
        sess.send_str("printf '\\033[?1l'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| !s.application_cursor_keys()),
            "DECCKM should be disabled after ESC[?1l"
        );

        sess.send_str("exit\n");
    }

    /// Verify that `screen_text()` returns non-empty content after the shell
    /// produces output, and that it strips trailing blank rows.
    #[test]
    #[cfg(unix)]
    fn screen_text_strips_trailing_blanks() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 10, "/bin/sh", &cwd, 100).expect("failed to spawn /bin/sh");

        // Wait for the shell prompt (any output at all).
        let _ = poll_until(&mut sess, 3000, |s| !s.screen_text().is_empty());
        let text = sess.screen_text();
        // Must not end with a blank line.
        assert!(
            !text.ends_with('\n'),
            "screen_text() should not end with a newline; got: {text:?}"
        );
        sess.send_str("exit\n");
    }

    // ── Selection-in-scrollback tests (pure unit — no PTY output needed) ─────
    //
    // These tests inject history rows directly into `TerminalSession::history`
    // (accessible because `mod tests` is a child of the same module) and
    // exercise `selected_text` / `build_rows` without waiting for shell output.
    // A real PTY is still spawned so the struct is fully initialised, but no
    // `poll()` calls are made, so PTY output cannot race with the injected
    // history rows.

    /// Build a `Vec<HistCell>` row that fills `cols` columns with `ch` in the
    /// first `text_len` cells and spaces thereafter.
    #[cfg(unix)]
    fn make_hist_row_content(text: &str, cols: u16) -> Vec<HistCell> {
        let chars: Vec<char> = text.chars().collect();
        (0..cols as usize)
            .map(|i| HistCell {
                ch: chars.get(i).copied().unwrap_or(' '),
                ..Default::default()
            })
            .collect()
    }

    /// `selected_text()` returns the correct history text when the view is
    /// fully scrolled into the scrollback buffer (all display rows are history
    /// rows, `scroll_offset == rows`).
    ///
    /// Before the fix this returned `None` regardless of the selection.
    #[test]
    #[cfg(unix)]
    fn selected_text_from_pure_history() {
        let cwd = std::env::temp_dir();
        // 10 cols × 4 rows — small enough that scrolling is easy to reason about.
        let mut sess = TerminalSession::spawn(10, 4, "/bin/sh", &cwd, 100).expect("spawn failed");

        // Inject three known history rows (no poll() — avoids races with PTY).
        sess.history
            .push_back(make_hist_row_content("AAAA", sess.cols));
        sess.history
            .push_back(make_hist_row_content("BBBB", sess.cols));
        sess.history
            .push_back(make_hist_row_content("CCCC", sess.cols));

        // Scroll so all 3 history rows are visible at the top; the 4th
        // display row would be a live row we don't care about.
        // scroll_offset = 3: display_r 0,1,2 → hist[0,1,2]; display_r 3 → live.
        sess.set_scroll_offset(3);

        // Select display row 1 (= history row "BBBB"), columns 0-3 inclusive.
        sess.selection = Some(TerminalSelection {
            start_row: 1,
            start_col: 0,
            end_row: 1,
            end_col: 3,
        });

        let text = sess
            .selected_text()
            .expect("selected_text() must be Some when scrolled");
        assert_eq!(
            text, "BBBB",
            "expected 'BBBB' from history row 1, got: {text:?}"
        );

        sess.send_str("exit\n");
    }

    /// `selected_text()` returns the correct text for a multi-row selection
    /// that spans multiple history rows.
    #[test]
    #[cfg(unix)]
    fn selected_text_multi_row_in_history() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(10, 5, "/bin/sh", &cwd, 100).expect("spawn failed");

        sess.history
            .push_back(make_hist_row_content("LINE0", sess.cols));
        sess.history
            .push_back(make_hist_row_content("LINE1", sess.cols));
        sess.history
            .push_back(make_hist_row_content("LINE2", sess.cols));

        // Scroll_offset = 3: display rows 0,1,2 → history rows 0,1,2.
        sess.set_scroll_offset(3);

        // Select display rows 0–2, full width (cols 0 to 4 each).
        sess.selection = Some(TerminalSelection {
            start_row: 0,
            start_col: 0,
            end_row: 2,
            end_col: 4,
        });

        let text = sess.selected_text().expect("selected_text() must be Some");
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 3, "expected 3 lines, got: {lines:?}");
        assert_eq!(lines[0], "LINE0", "row 0: got {:?}", lines[0]);
        assert_eq!(lines[1], "LINE1", "row 1: got {:?}", lines[1]);
        assert_eq!(lines[2], "LINE2", "row 2: got {:?}", lines[2]);

        sess.send_str("exit\n");
    }

    /// `build_rows()` marks cells `selected = true` for history rows when a
    /// selection covers them.
    ///
    /// Before the fix the `selected` flag was always `false` for history rows.
    #[test]
    #[cfg(unix)]
    fn build_rows_highlights_selection_in_history() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(6, 4, "/bin/sh", &cwd, 100).expect("spawn failed");

        // One history row, 6 columns: ['H','I','S','T',' ',' '].
        sess.history
            .push_back(make_hist_row_content("HIST", sess.cols));
        // One more to ensure display_r=0 maps to the right row.
        sess.history
            .push_back(make_hist_row_content("XXXX", sess.cols));

        // scroll_offset = 2: display_r 0 → history[0]="HIST", display_r 1 → history[1]="XXXX"
        sess.set_scroll_offset(2);

        // Select display_r = 0, cols 1-3 → "IST"
        sess.selection = Some(TerminalSelection {
            start_row: 0,
            start_col: 1,
            end_row: 0,
            end_col: 3,
        });

        let rows = sess.build_rows(false);

        // display row 0: cols 1,2,3 must be selected; col 0 and 4+ must not.
        let row0 = &rows[0];
        assert!(!row0[0].selected, "col 0 should NOT be selected");
        assert!(row0[1].selected, "col 1 should be selected");
        assert!(row0[2].selected, "col 2 should be selected");
        assert!(row0[3].selected, "col 3 should be selected");
        assert!(!row0[4].selected, "col 4 should NOT be selected");

        // display row 1 (different history row) should have no selection.
        let row1 = &rows[1];
        assert!(
            row1.iter().all(|c| !c.selected),
            "row 1 should have no selection"
        );

        sess.send_str("exit\n");
    }

    /// `selected_text()` at `scroll_offset == 0` (live view) continues to
    /// work correctly — regression guard.
    #[test]
    #[cfg(unix)]
    fn selected_text_live_view_no_regression() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(10, 4, "/bin/sh", &cwd, 100).expect("spawn failed");

        // At live view with no selection, must return None.
        assert!(sess.selection.is_none());
        assert!(sess.selected_text().is_none());

        // Set a selection and confirm we get Some (content is live-screen
        // dependent so we only check it's non-None and correctly typed).
        sess.selection = Some(TerminalSelection {
            start_row: 0,
            start_col: 0,
            end_row: 0,
            end_col: 4,
        });
        assert!(
            sess.selected_text().is_some(),
            "selected_text() must be Some when selection is set at live view"
        );

        sess.send_str("exit\n");
    }

    /// A selection spanning the history/live boundary: the history part is
    /// extracted from `self.history` and the live part from the vt100 screen.
    /// The result must be `Some(_)` and contain the history text on the first line.
    #[test]
    #[cfg(unix)]
    fn selected_text_spans_history_live_boundary() {
        let cwd = std::env::temp_dir();
        // 10 cols × 5 rows.
        let mut sess = TerminalSession::spawn(10, 5, "/bin/sh", &cwd, 100).expect("spawn failed");

        // Inject two history rows.
        sess.history
            .push_back(make_hist_row_content("HIST0", sess.cols));
        sess.history
            .push_back(make_hist_row_content("HIST1", sess.cols));

        // scroll_offset = 2: display_r 0 → HIST0, display_r 1 → HIST1,
        //                    display_r 2+ → live rows.
        sess.set_scroll_offset(2);

        // Selection: display row 1 (last history row) through display row 2
        // (first live row), columns 0-4.
        sess.selection = Some(TerminalSelection {
            start_row: 1,
            start_col: 0,
            end_row: 2,
            end_col: 4,
        });

        let text = sess
            .selected_text()
            .expect("selected_text() must be Some at boundary");
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(
            lines.len(),
            2,
            "expected 2 lines (history + live); got: {lines:?}"
        );
        assert_eq!(
            lines[0], "HIST1",
            "first line must come from history; got: {:?}",
            lines[0]
        );
        // lines[1] is from the live screen (shell prompt) — content is
        // non-deterministic, so we just verify it was included.

        sess.send_str("exit\n");
    }
}
