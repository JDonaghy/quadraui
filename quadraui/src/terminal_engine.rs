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
/// matching the approach in vimcode's `HistCell`.
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
/// `Default` resolves to dark-theme terminal defaults matching the
/// vimcode OneDark baseline (`#e5e5e5` fg, `#1e1e1e` bg). Callers
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
        // the host app (e.g. coord-tui) is itself launched from inside tmux,
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
    /// The coordinator uses this to inject prompts programmatically.
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
                            line.push_str(&s);
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
    ///    scrollback (quadraui #334, coord #446). See
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
    /// (quadraui #334 / coord #446).
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
    /// Useful for coordinator scraping: scan `full_text()` for a prompt or
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
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.parser.set_size(rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
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
        self.parser.set_scrollback(0);
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

    /// Extract selected text from the live vt100 screen.
    ///
    /// Returns `None` when there is no selection or when the user is
    /// scrolled into history (selection is only tracked in the live
    /// screen coordinate space).
    pub fn selected_text(&self) -> Option<String> {
        if self.scroll_offset != 0 {
            return None;
        }
        let sel = self.selection.as_ref()?;
        let screen = self.parser.screen();
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
                if let Some(cell) = screen.cell(row, col) {
                    let s = cell.contents();
                    if s.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&s);
                    }
                }
            }
            lines.push(line.trim_end().to_string());
        }
        Some(lines.join("\n"))
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
                    self.parser.process(&data[start..=i]);
                    self.capture_scrolled_rows(nl_count);
                    start = i + 1;
                    nl_count = 0;
                }
            }
        }
        if start < data.len() {
            let chunk = &data[start..];
            let remaining_nl = chunk.iter().filter(|&&b| b == b'\n').count();
            self.parser.process(chunk);
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
        self.parser.set_scrollback(to_capture);
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
        self.parser.set_scrollback(0);
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

        // Selection is only available in live view (scroll_offset == 0).
        let sel_bounds = if scroll_offset == 0 {
            self.selection.as_ref().map(normalize_selection)
        } else {
            None
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
                                            false,
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
                                            false,
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

                                let selected = sel_bounds.is_some_and(|(r0, c0, r1, c1)| {
                                    if r0 == r1 {
                                        live_r == r0 && cu >= c0 && cu <= c1
                                    } else if live_r == r0 {
                                        cu >= c0
                                    } else if live_r == r1 {
                                        cu <= c1
                                    } else {
                                        live_r > r0 && live_r < r1
                                    }
                                });

                                (ch, fg, bg, bold, italic, underline, is_cursor, selected)
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
    /// programmatic driver waits on before injecting input (quadraui #343,
    /// consumed by coord-tui #446).
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
    /// engine forward wheel events to the child — the routing rule from
    /// coord #446 that fixes embedded `claude` / `tmux` / `less`.
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
    /// (quadraui #335 + coord #446 — without this, `claude` / `tmux` / `vim`
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
}
