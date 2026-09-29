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

use crate::event::{Key, MouseButton, NamedKey};
use crate::primitives::terminal::{Terminal, TerminalCell, TerminalScrollbar};
use crate::types::{Color, Modifiers, WidgetId};

// ── Internal history cell ─────────────────────────────────────────────────────

/// A single captured terminal cell in the scrollback ring buffer.
///
/// Uses `vt100::Color` directly to defer RGB resolution until paint time,
/// matching the approach used by downstream terminal-history cell types.
///
/// `text` carries the cell's full grapheme cluster (base character plus
/// any combining marks), mirroring [`TerminalCell::text`] — no longer
/// `Copy` since `String` isn't (quadraui#337).
#[derive(Clone)]
struct HistCell {
    text: String,
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    italic: bool,
    underline: bool,
    /// SGR 2 (faint/dim) — `vt100::Cell::dim()`, tracked since vt100
    /// 0.16 (quadraui#345).
    dim: bool,
}

impl Default for HistCell {
    fn default() -> Self {
        HistCell {
            text: " ".to_string(),
            fg: vt100::Color::Default,
            bg: vt100::Color::Default,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
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

/// Encode pasted `text` as PTY input bytes.
///
/// When `bracketed` is `true`, wraps `text` in bracketed-paste markers
/// (`ESC[200~ ... ESC[201~`, DEC private mode 2004) so a program that
/// understands them (readline-based shells, `vim`, `claude`, ...) can
/// tell pasted text apart from typed text. When `bracketed` is `false`,
/// returns `text`'s raw UTF-8 bytes unchanged — wrapping unconditionally
/// would leak literal escape bytes into programs that never asked for
/// bracketed paste and don't know to strip them (e.g. `cat`, `less`).
///
/// Pulled out as a standalone function (rather than inlined into
/// [`TerminalSession::paste`]) so the encoding itself is unit-testable
/// without spawning a PTY — see the `tests` module below.
fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut bytes = Vec::with_capacity(text.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.as_bytes().to_vec()
    }
}

// ── Keyboard → PTY encoding ───────────────────────────────────────────────────
//
// Lifted out of `examples/common/terminal_app.rs` into the engine
// (quadraui#342) so every embedded consumer (vimcode-tui, a future GTK
// standalone app, ...) gets the same VT100/xterm key encoding instead of
// re-implementing or copying it. `TerminalSession::encode_key` is the
// entry point most callers want — it reads `application_cursor_keys()`
// internally. [`key_to_pty_bytes`] is exposed as a free function for
// callers that want to encode without a live session (e.g. tests).

/// Convert a [`Key`] + [`Modifiers`] to the byte sequence sent to the PTY.
///
/// `app_cursor` should be `true` when the child has enabled DECCKM
/// (application-cursor-keys mode, `ESC [ ? 1 h`). In that mode, unmodified
/// arrow keys and Home/End are encoded as SS3 sequences (`ESC O A…D/H/F`)
/// rather than the normal CSI sequences. Obtain the flag from
/// [`TerminalSession::application_cursor_keys`], or use
/// [`TerminalSession::encode_key`], which reads it for you.
///
/// Covers the common VT100 / xterm-256color escape sequences. Keys that
/// have no meaningful PTY encoding (e.g. CapsLock) return `None`.
pub fn key_to_pty_bytes(key: Key, mods: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    match key {
        Key::Char(ch) => {
            if mods.ctrl {
                // Ctrl+A-Z → bytes 0x01..0x1A.
                let c = ch.to_ascii_uppercase();
                if c.is_ascii_alphabetic() {
                    return Some(vec![c as u8 - b'@']);
                }
                // Ctrl+[ → ESC, Ctrl+\ → FS, Ctrl+] → GS, Ctrl+^ → RS, Ctrl+_ → US.
                match ch {
                    '[' => return Some(vec![0x1b]),
                    '\\' => return Some(vec![0x1c]),
                    ']' => return Some(vec![0x1d]),
                    '^' => return Some(vec![0x1e]),
                    '_' => return Some(vec![0x1f]),
                    _ => {}
                }
            }
            // Regular printable character — encode as UTF-8.
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            Some(s.as_bytes().to_vec())
        }

        Key::Named(named) => named_key_bytes(named, mods, app_cursor),
    }
}

/// Map named keys to their VT100 escape sequences.
///
/// `app_cursor` enables DECCKM encoding: unmodified arrow keys and Home/End
/// emit SS3 sequences (`ESC O x`) rather than CSI sequences (`ESC [ x`).
/// When a modifier is present the CSI form is always used regardless of mode.
fn named_key_bytes(key: NamedKey, mods: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    // Modifier prefix for xterm sequences: 1=plain 2=shift 3=alt 4=shift+alt
    // 5=ctrl 6=shift+ctrl 7=alt+ctrl 8=shift+alt+ctrl.
    let mod_param = modifier_param(mods);

    match key {
        NamedKey::Enter => Some(b"\r".to_vec()),
        NamedKey::Tab => {
            if mods.shift {
                Some(b"\x1b[Z".to_vec()) // Back-tab
            } else {
                Some(b"\t".to_vec())
            }
        }
        NamedKey::BackTab => Some(b"\x1b[Z".to_vec()),
        NamedKey::Backspace => Some(b"\x7f".to_vec()),
        NamedKey::Delete => Some(xterm_seq(b"3", mod_param)),
        NamedKey::Escape => Some(b"\x1b".to_vec()),
        // ── Arrow keys: SS3 in application-cursor mode (no modifier), CSI otherwise
        NamedKey::Up => {
            if app_cursor && mod_param.is_none() {
                Some(ss3_seq(b'A'))
            } else {
                Some(xterm_cursor_seq(b"A", mod_param))
            }
        }
        NamedKey::Down => {
            if app_cursor && mod_param.is_none() {
                Some(ss3_seq(b'B'))
            } else {
                Some(xterm_cursor_seq(b"B", mod_param))
            }
        }
        NamedKey::Right => {
            if app_cursor && mod_param.is_none() {
                Some(ss3_seq(b'C'))
            } else {
                Some(xterm_cursor_seq(b"C", mod_param))
            }
        }
        NamedKey::Left => {
            if app_cursor && mod_param.is_none() {
                Some(ss3_seq(b'D'))
            } else {
                Some(xterm_cursor_seq(b"D", mod_param))
            }
        }
        // ── Home/End: SS3 in application-cursor mode (no modifier), tilde-CSI otherwise
        NamedKey::Home => {
            if app_cursor && mod_param.is_none() {
                Some(ss3_seq(b'H'))
            } else {
                Some(xterm_seq(b"1", mod_param))
            }
        }
        NamedKey::End => {
            if app_cursor && mod_param.is_none() {
                Some(ss3_seq(b'F'))
            } else {
                Some(xterm_seq(b"4", mod_param))
            }
        }
        // PageUp/PageDown are not affected by DECCKM.
        NamedKey::Insert => Some(xterm_seq(b"2", mod_param)),
        NamedKey::PageUp => Some(xterm_seq(b"5", mod_param)),
        NamedKey::PageDown => Some(xterm_seq(b"6", mod_param)),
        NamedKey::F(n) => f_key_bytes(n, mod_param),
        // Keys with no PTY mapping.
        NamedKey::CapsLock | NamedKey::NumLock | NamedKey::ScrollLock | NamedKey::Menu => None,
    }
}

/// Build an SS3 sequence: `ESC O <letter>`.
///
/// Used for application-cursor-keys mode (DECCKM on): unmodified arrows emit
/// `ESC O A/B/C/D` and Home/End emit `ESC O H/F` instead of CSI sequences.
fn ss3_seq(letter: u8) -> Vec<u8> {
    vec![0x1b, b'O', letter]
}

/// Build an xterm modifier parameter (1-based; plain = `None`).
fn modifier_param(mods: Modifiers) -> Option<u8> {
    // mod_param = 1 + shift + 2*alt + 4*ctrl
    let n: u8 = 1
        + if mods.shift { 1 } else { 0 }
        + if mods.alt { 2 } else { 0 }
        + if mods.ctrl { 4 } else { 0 };
    if n == 1 {
        None
    } else {
        Some(n)
    }
}

/// Build `\x1b[<code>~` or `\x1b[<code>;<mod>~` for tilde-terminated sequences.
///
/// Per xterm conventions, the modifier parameter follows the code (separated by `;`)
/// for tilde-terminated sequences (Home, End, Insert, Delete, PageUp, PageDown, F5–F12).
/// Cursor-letter sequences use the `1;<mod>` prefix instead — see [`xterm_cursor_seq`].
fn xterm_seq(code: &[u8], mod_param: Option<u8>) -> Vec<u8> {
    let mut v = b"\x1b[".to_vec();
    v.extend_from_slice(code);
    if let Some(m) = mod_param {
        v.push(b';');
        v.push(b'0' + m);
    }
    v.push(b'~');
    v
}

/// Build cursor-movement sequences: `\x1b[<letter>` or `\x1b[1;<mod><letter>`.
fn xterm_cursor_seq(letter: &[u8], mod_param: Option<u8>) -> Vec<u8> {
    match mod_param {
        None => {
            let mut v = b"\x1b[".to_vec();
            v.extend_from_slice(letter);
            v
        }
        Some(m) => {
            let mut v = b"\x1b[1;".to_vec();
            v.push(b'0' + m);
            v.extend_from_slice(letter);
            v
        }
    }
}

/// Function-key byte sequences (xterm encoding).
fn f_key_bytes(n: u8, mod_param: Option<u8>) -> Option<Vec<u8>> {
    // F1-F4 use SS3 sequences when unmodified (\x1bOP…\x1bOS).
    // When a modifier is present they fall back to CSI: \x1b[1;<mod>P…S.
    // F5-F12 always use tilde-terminated CSI sequences.
    let bytes = match n {
        1 => {
            if mod_param.is_none() {
                b"\x1bOP".to_vec()
            } else {
                xterm_cursor_seq(b"P", mod_param)
            }
        }
        2 => {
            if mod_param.is_none() {
                b"\x1bOQ".to_vec()
            } else {
                xterm_cursor_seq(b"Q", mod_param)
            }
        }
        3 => {
            if mod_param.is_none() {
                b"\x1bOR".to_vec()
            } else {
                xterm_cursor_seq(b"R", mod_param)
            }
        }
        4 => {
            if mod_param.is_none() {
                b"\x1bOS".to_vec()
            } else {
                xterm_cursor_seq(b"S", mod_param)
            }
        }
        5 => xterm_seq(b"15", mod_param),
        6 => xterm_seq(b"17", mod_param),
        7 => xterm_seq(b"18", mod_param),
        8 => xterm_seq(b"19", mod_param),
        9 => xterm_seq(b"20", mod_param),
        10 => xterm_seq(b"21", mod_param),
        11 => xterm_seq(b"23", mod_param),
        12 => xterm_seq(b"24", mod_param),
        _ => return None, // F13+ not commonly used
    };
    Some(bytes)
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
fn reflow_screen<CB: vt100::Callbacks>(parser: &mut vt100::Parser<CB>, rows: u16, cols: u16) {
    let (cur_rows, cur_cols) = parser.screen().size();
    if (rows, cols) == (cur_rows, cur_cols) {
        return;
    }
    // Height-only change, or a self-repainting full-screen app: use the plain
    // non-reflow resize. (`alternate_screen()` reads the parser immutably; the
    // borrow ends before the `screen_mut()` below.)
    if cols == cur_cols || parser.screen().alternate_screen() {
        set_size_without_orphaning_wide_cells(parser, rows, cols);
        return;
    }
    // Width change on the normal screen: snapshot → resize → replay so each
    // logical line re-wraps at the new width instead of being truncated.
    // The snapshot is taken *before* the scrub inside the resize below, so a
    // wide glyph the scrub has to erase is still carried across and re-wrapped
    // at the new width rather than lost.
    let dump = parser.screen().contents_formatted();
    set_size_without_orphaning_wide_cells(parser, rows, cols);
    parser.process(&dump);
}

/// [`vt100::Screen::set_size`], plus the repair vt100 0.16.2 forgets to make
/// when a **width shrink** cuts a double-width glyph in half (quadraui#1130).
///
/// vt100 stores a wide glyph as two cells: a first half (`Cell::is_wide()`) and
/// a continuation half (`Cell::is_wide_continuation()`). `Grid::set_size`
/// narrows each row with a plain `Vec::resize`, which simply drops the cells
/// past the new width — unlike `Row::truncate`, which clears a first half whose
/// partner it is about to drop. So shrinking to a width that lands exactly
/// *between* a wide glyph's two halves leaves a first half orphaned at the new
/// final column with nothing after it.
///
/// That cell is then a landmine, and every route out of it is an upstream
/// `unwrap()` on an out-of-range column:
///
/// - writing a character over it panics at `screen.rs:870` (it looks for the
///   continuation half at `col + 1` to blank it), and
/// - *erasing* it panics too (`Row::clear_wide` indexes the same `col + 1`), so
///   it cannot be cleaned up after the fact — not even by `ED`/`EL`, which is
///   exactly what replaying a `contents_formatted` dump starts with. That is
///   why [`reflow_screen`]'s snapshot→resize→replay was affected as well as the
///   plain height-only / alternate-screen path.
///
/// The fix is therefore to *prevent* the orphan rather than repair it: while
/// both halves are still in range, erase (`CSI X`) any first half sitting on
/// the column that is about to become the last one. The shrink then truncates a
/// blank pair instead of splitting a glyph. `DECSC`/`DECRC` bracket the scrub so
/// the cursor position, origin mode and pending-wrap state survive it.
///
/// Costs nothing in the common case: the scan is skipped entirely unless the
/// width is actually shrinking, and no bytes are fed to the parser unless a
/// glyph really does straddle the new boundary.
fn set_size_without_orphaning_wide_cells<CB: vt100::Callbacks>(
    parser: &mut vt100::Parser<CB>,
    rows: u16,
    cols: u16,
) {
    let (cur_rows, cur_cols) = parser.screen().size();
    // Growing (or keeping) the width can never split a glyph; rows are dropped
    // whole, so a height change is safe too.
    // (`checked_sub` rather than `cols - 1`: a zero width is already fatal
    // one line down inside vt100 — see `MIN_VT100_COLS` — but this helper
    // must not be the thing that panics first, and there is no boundary
    // cell to scrub when there is no last column.)
    if let Some(last_col) = cols.checked_sub(1).filter(|_| cols < cur_cols) {
        let mut scrub = Vec::new();
        for row in 0..cur_rows {
            let straddles = parser
                .screen()
                .cell(row, last_col)
                .is_some_and(vt100::Cell::is_wide);
            if straddles {
                // CUP to (row, new last column), then ECH 1. Both 1-based.
                scrub.extend_from_slice(format!("\x1b[{};{}H\x1b[X", row + 1, cols).as_bytes());
            }
        }
        if !scrub.is_empty() {
            let mut bytes = b"\x1b7".to_vec();
            bytes.append(&mut scrub);
            bytes.extend_from_slice(b"\x1b8");
            parser.process(&bytes);
        }
    }
    parser.screen_mut().set_size(rows, cols);
}

/// Floor for `rows` handed to `vt100::Parser::new`/`Screen::set_size`.
///
/// vt100 0.16.2 panics (`attempt to subtract with overflow`, `grid.rs:683`)
/// constructing or resizing a grid with `rows < 2` — an upstream bug, not
/// anything this file does wrong. A terminal pane's rect can legitimately
/// shrink to a single visible row — or momentarily to zero, before a
/// drag-resize settles — so every caller-supplied `rows` is floored to
/// this at both call sites that construct/resize the parser
/// ([`TerminalSession::spawn`], [`TerminalSession::resize`]) rather than
/// trusted. Found by this file's own `vt100_parser_never_panics`
/// property test (quadraui#1130) before any real caller ever hit it.
const MIN_VT100_ROWS: u16 = 2;

/// Floor for `cols`, for the same reason as [`MIN_VT100_ROWS`] but a
/// different upstream panic site (`attempt to subtract with overflow`,
/// `screen.rs:730`, reachable with `cols == 1` and certain multi-byte
/// input regardless of `rows`) — found by the same property test.
const MIN_VT100_COLS: u16 = 2;

/// Floor a caller-supplied `(cols, rows)` pair to the smallest grid vt100
/// can construct or resize to without panicking.
///
/// Both [`TerminalSession::spawn`] and [`TerminalSession::resize`] funnel
/// their dimensions through this before anything reaches
/// `vt100::Parser::new`/`Screen::set_size`; see
/// [`MIN_VT100_ROWS`]/[`MIN_VT100_COLS`] for which upstream panic each
/// bound dodges. Pulled out as a standalone function so the floor itself
/// is testable on every platform — the end-to-end `spawn`/`resize`
/// assertions need a real PTY child and so are `cfg(unix)`-only.
fn clamp_vt100_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.max(MIN_VT100_COLS), rows.max(MIN_VT100_ROWS))
}

/// [`vt100::Callbacks`] implementation that records OSC 0/1/2 window-title
/// requests instead of discarding them.
///
/// vt100 0.16.2 reports these via callback methods rather than storing them
/// on [`vt100::Screen`] itself (see `set_window_title`/`set_window_icon_name`
/// in the upstream `Callbacks` trait), so [`TerminalSession`] carries one of
/// these alongside its parser and reads it back after each [`poll`](TerminalSession::poll).
///
/// Tracks a dirty flag so callers can cheaply ask "did the title change
/// since I last looked?" without diffing strings themselves — see
/// [`TerminalSession::take_title_changed`].
#[derive(Debug, Default)]
struct TitleCallbacks {
    /// Most recent window title set via OSC 0 or OSC 2. `None` until the
    /// child program sets one.
    title: Option<String>,
    /// `true` when [`title`](Self::title) has changed since the last
    /// [`TerminalSession::take_title_changed`] call.
    changed: bool,
}

impl vt100::Callbacks for TitleCallbacks {
    fn set_window_title(&mut self, _screen: &mut vt100::Screen, title: &[u8]) {
        let title = String::from_utf8_lossy(title).into_owned();
        if self.title.as_deref() != Some(title.as_str()) {
            self.title = Some(title);
            self.changed = true;
        }
    }
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
    ///
    /// Carries a [`TitleCallbacks`] so OSC 0/1/2 window-title requests are
    /// captured; read back via [`title()`](Self::title) /
    /// [`take_title_changed()`](Self::take_title_changed).
    parser: vt100::Parser<TitleCallbacks>,
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
        // See `MIN_VT100_ROWS`/`MIN_VT100_COLS`'s docs — vt100 panics below these.
        let (cols, rows) = clamp_vt100_size(cols, rows);
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
        let parser = vt100::Parser::new_with_callbacks(rows, cols, 1000, TitleCallbacks::default());

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

    // ── Window title (OSC 0/1/2) ─────────────────────────────────────────────

    /// The most recent window title set by the child program via an OSC 0 or
    /// OSC 2 escape sequence (e.g. `vim`, `tmux`, or a shell's own prompt
    /// hook), or `None` if it has never set one.
    ///
    /// This is a plain accessor — it does not consume the change. Backends
    /// that want to react only when the title actually changes (e.g. to set
    /// a native window title once per change rather than every frame)
    /// should use [`take_title_changed`](Self::take_title_changed) instead.
    ///
    /// Kept as an additive accessor rather than a `UiEvent` variant so
    /// non-`#[non_exhaustive]` consumers (`coord-tui`, `vimcode`) don't need
    /// to add a match arm — see issue #339.
    pub fn title(&self) -> Option<&str> {
        self.parser.callbacks().title.as_deref()
    }

    /// `true` exactly once per title change: returns whether
    /// [`title()`](Self::title) has changed since the last call to this
    /// method, clearing the dirty flag as a side effect.
    ///
    /// Call this once per [`poll()`](Self::poll)-driven frame; a caller
    /// (e.g. the GTK runner) can use it to cheaply gate a native window
    /// title update instead of setting it on every frame.
    pub fn take_title_changed(&mut self) -> bool {
        std::mem::take(&mut self.parser.callbacks_mut().changed)
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

    /// Send pasted text to the shell — the single paste entry point every
    /// paste source (GTK Ctrl-V/Ctrl-Shift-V, GTK middle-click PRIMARY
    /// selection, TUI crossterm bracketed paste) should call instead of
    /// hand-rolling the bracketed-paste wrap (quadraui#415). Also the
    /// natural entry point for a future supervising process that wants to
    /// inject text programmatically, though nothing wires that up today.
    ///
    /// Wraps `text` in bracketed-paste markers (`ESC[200~ ... ESC[201~`)
    /// when the child has enabled bracketed-paste mode
    /// ([`bracketed_paste_enabled`](Self::bracketed_paste_enabled)), so
    /// programs that understand it (readline-based shells, `vim`, `claude`,
    /// ...) can tell pasted text apart from typed text — most importantly,
    /// so pasted newlines don't get interpreted as "run this line" one at a
    /// time. Sends `text` raw when the child hasn't enabled that mode,
    /// since wrapping unconditionally would leak literal escape bytes into
    /// programs that don't strip them (e.g. `cat`, `less`).
    ///
    /// Also resets the scroll offset to the live view — like ordinary
    /// keystroke input — so a paste is always visible immediately even if
    /// the user had scrolled into history first.
    pub fn paste(&mut self, text: &str) {
        self.scroll_reset();
        let bytes = encode_paste(text, self.bracketed_paste_enabled());
        self.write_input(&bytes);
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
    /// stops working. [`encode_key`](Self::encode_key) queries this flag for
    /// you each keystroke; call this directly only if you're hand-rolling the
    /// encoding.
    ///
    /// Backed by [`vt100::Screen::application_cursor`].
    pub fn application_cursor_keys(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    // ── Keyboard → PTY encoding ───────────────────────────────────────────────

    /// Encode a key press as PTY bytes, reading the child's DECCKM
    /// (application-cursor-keys) state internally so callers don't have to
    /// thread [`application_cursor_keys`](Self::application_cursor_keys)
    /// through themselves.
    ///
    /// Returns `None` for keys with no meaningful PTY encoding (e.g.
    /// CapsLock) — callers should treat that as "swallow this keystroke",
    /// not as an error.
    ///
    /// This is the read-only half; most callers want
    /// [`write_key`](Self::write_key), which also writes the bytes and
    /// resets the scroll offset the same way [`paste`](Self::paste) does.
    pub fn encode_key(&self, key: Key, mods: Modifiers) -> Option<Vec<u8>> {
        key_to_pty_bytes(key, mods, self.application_cursor_keys())
    }

    /// Encode a key press with [`encode_key`](Self::encode_key) and write it
    /// to the PTY, resetting the scroll offset to the live view first (like
    /// [`paste`](Self::paste) and ordinary character input) so a keystroke
    /// is always visible immediately even if the user had scrolled into
    /// history. Returns `true` when bytes were written.
    ///
    /// Callers embedding a full interactive shell (arrows, F-keys,
    /// modifiers, Ctrl-combos) should route `UiEvent::KeyPressed` here
    /// instead of hand-rolling the escape-sequence encoding — see
    /// `examples/common/terminal_app.rs` for the reference wiring.
    pub fn write_key(&mut self, key: Key, mods: Modifiers) -> bool {
        match self.encode_key(key, mods) {
            Some(bytes) => {
                self.scroll_reset();
                self.write_input(&bytes);
                true
            }
            None => false,
        }
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

    /// Canonical wheel-notch policy: try to forward the wheel to the PTY
    /// child, and fall back to scrolling local scrollback when it doesn't
    /// take it.
    ///
    /// This is the forward-vs-scrollback composition every consumer was
    /// re-deriving by hand (quadraui#365): "is the child on the alt-screen
    /// or reporting mouse events? If so, let it paginate itself. Otherwise
    /// scroll our own scrollback by `step` rows." Equivalent to:
    ///
    /// ```ignore
    /// let kind = if up { WheelUp } else { WheelDown };
    /// if !sess.forward_mouse(kind, MouseButton::Left, col, row, Modifiers::default()) {
    ///     if up { sess.scroll_up(step) } else { sess.scroll_down(step) }
    /// }
    /// ```
    ///
    /// `col`/`row` are the cell the pointer is over (0-based, clamped to
    /// the viewport by the caller). SGR-1006 always encodes the pointer
    /// position on the wire ([`encode_mouse_sgr`]), and real alt-screen
    /// consumers — tmux is the canonical example — use it to route wheel
    /// input to the pane under the cursor, so the caller must supply the
    /// real position rather than a fixed `(0, 0)`.
    ///
    /// Returns `true` when the event was written to the child (the caller
    /// must not also touch local scrollback), `false` when it fell back to
    /// [`scroll_up`](Self::scroll_up) / [`scroll_down`](Self::scroll_down).
    pub fn handle_wheel(&mut self, up: bool, step: usize, col: u16, row: u16) -> bool {
        let kind = if up {
            TerminalMouseKind::WheelUp
        } else {
            TerminalMouseKind::WheelDown
        };
        let forwarded = self.forward_mouse(kind, MouseButton::Left, col, row, Modifiers::default());
        if !forwarded {
            if up {
                self.scroll_up(step);
            } else {
                self.scroll_down(step);
            }
        }
        forwarded
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
                    line.push_str(&hc.text);
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
        // See `MIN_VT100_ROWS`/`MIN_VT100_COLS`'s docs — vt100 panics below these.
        let (cols, rows) = clamp_vt100_size(cols, rows);
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
                    return hist_row.get(col as usize).cloned().unwrap_or_default().text;
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
                                // vt100's own convention for an empty /
                                // never-written cell (including a wide
                                // glyph's trailing spacer column) is
                                // `contents() == ""` — normalise that to
                                // a single space rather than storing an
                                // empty grapheme, so history playback
                                // still occupies its column
                                // (quadraui#337).
                                text: if raw.is_empty() {
                                    " ".to_string()
                                } else {
                                    raw.to_string()
                                },
                                fg: cell.fgcolor(),
                                bg: cell.bgcolor(),
                                bold: cell.bold(),
                                italic: cell.italic(),
                                underline: cell.underline(),
                                dim: cell.dim(),
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

                        let (text, fg, bg, bold, italic, underline, dim, is_cursor, selected) =
                            if display_r < scroll_offset {
                                // Row is in the scrollback history.
                                let hist_idx_signed =
                                    hist_len as isize - scroll_offset as isize + display_r as isize;
                                if hist_idx_signed >= 0 {
                                    if let Some(hist_row) =
                                        self.history.get(hist_idx_signed as usize)
                                    {
                                        let hc = hist_row.get(c).cloned().unwrap_or_default();
                                        (
                                            hc.text,
                                            map_vt100_color(hc.fg, false),
                                            map_vt100_color(hc.bg, true),
                                            hc.bold,
                                            hc.italic,
                                            hc.underline,
                                            hc.dim,
                                            false,
                                            is_selected(display_r, cu),
                                        )
                                    } else {
                                        (
                                            " ".to_string(),
                                            (229, 229, 229),
                                            (30, 30, 30),
                                            false,
                                            false,
                                            false,
                                            false,
                                            false,
                                            is_selected(display_r, cu),
                                        )
                                    }
                                } else {
                                    (
                                        " ".to_string(),
                                        (229, 229, 229),
                                        (30, 30, 30),
                                        false,
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
                                let (text, fg, bg, bold, italic, underline, dim) =
                                    if let Some(cell) = screen.cell(live_r, cu) {
                                        let contents = cell.contents();
                                        // Full grapheme cluster (base char
                                        // + any combining marks), not just
                                        // the first char — and vt100's
                                        // empty-string convention for a
                                        // blank / wide-glyph continuation
                                        // cell normalises to a single
                                        // space so it still occupies its
                                        // column (quadraui#337).
                                        let text = if contents.is_empty() {
                                            " ".to_string()
                                        } else {
                                            contents.to_string()
                                        };
                                        (
                                            text,
                                            map_vt100_color(cell.fgcolor(), false),
                                            map_vt100_color(cell.bgcolor(), true),
                                            cell.bold(),
                                            cell.italic(),
                                            cell.underline(),
                                            cell.dim(),
                                        )
                                    } else {
                                        (
                                            " ".to_string(),
                                            (229, 229, 229),
                                            (30, 30, 30),
                                            false,
                                            false,
                                            false,
                                            false,
                                        )
                                    };

                                let is_cursor = !self.exited
                                    && scroll_offset == 0
                                    && cursor_active
                                    && live_r == cursor_row
                                    && cu == cursor_col;

                                (
                                    text,
                                    fg,
                                    bg,
                                    bold,
                                    italic,
                                    underline,
                                    dim,
                                    is_cursor,
                                    is_selected(display_r, cu),
                                )
                            };

                        TerminalCell {
                            text,
                            fg: Color::rgb(fg.0, fg.1, fg.2),
                            bg: Color::rgb(bg.0, bg.1, bg.2),
                            bold,
                            italic,
                            underline,
                            dim,
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

/// Which "run a command string through the shell" convention applies.
///
/// Exists only to let [`shell_command_for`] be exercised for **both**
/// platform branches from a single test binary — `shell_command()` itself
/// picks the branch via `cfg(target_os = "windows")`, which a non-Windows
/// CI host can only ever compile one arm of.
/// `shell_command()` constructs exactly **one** of these two variants per
/// build target, so in any build without `cfg(test)` the other variant is
/// genuinely never constructed and `dead_code` fires — `Unix` on Windows,
/// `Windows` everywhere else. That asymmetry is the entire point of the
/// type, but CI sets `RUSTFLAGS: -D warnings` workspace-wide, so an
/// un-allowed variant is a hard *build failure* on the host whose branch
/// it isn't. Both arms below are therefore load-bearing: quadraui#970's
/// first CI run was green on ubuntu and red on windows-latest (the
/// pty-smoke step — the first Windows step that compiles this
/// `feature = "terminal"` module *without* `cfg(test)`) because only the
/// `Windows` arm existed. Keep them symmetric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellPlatform {
    #[cfg_attr(all(not(test), target_os = "windows"), allow(dead_code))]
    Unix,
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    Windows,
}

/// Pure decision logic behind [`shell_command`]: given the platform and the
/// already-read environment override (if any), returns `(shell, flag)`.
///
/// Factored out from [`shell_command`] so tests can cover both platform
/// branches and both "env var set" / "env var unset" cases without
/// mutating process-global `$SHELL` / `%COMSPEC%` state — that state is
/// shared with every other test in this binary (e.g.
/// `default_shell_is_nonempty` reads `$SHELL` too), so flipping it here
/// would race them under the default parallel test runner.
fn shell_command_for(platform: ShellPlatform, env_shell: Option<String>) -> (String, String) {
    match platform {
        ShellPlatform::Windows => (
            env_shell.unwrap_or_else(|| "cmd".to_string()),
            "/C".to_string(),
        ),
        ShellPlatform::Unix => (
            env_shell.unwrap_or_else(|| "sh".to_string()),
            "-c".to_string(),
        ),
    }
}

/// Returns the platform's shell binary plus its "run this command string"
/// flag, honouring `$SHELL` (Unix) / `%COMSPEC%` (Windows) where set —
/// the portable seam behind `Command::new(shell).arg(flag).arg(command)`.
///
/// Sibling of [`default_shell`]: `default_shell()` names a shell for an
/// *interactive* PTY session (used by [`TerminalSession::spawn`]);
/// `shell_command()` is for launching a single command string through
/// "the user's shell" — the case vimcode's `:!`, `:r !`, `!{motion}`
/// filters, and plugin async shells all hardcode as `Command::new("sh")`
/// today, with no Windows leg (quadraui#970). `default_shell()`'s own
/// behaviour is unchanged by this function's addition.
///
/// Falls back to `("sh", "-c")` on Unix and `("cmd", "/C")` on Windows
/// when the relevant environment variable isn't set. Both fallbacks are
/// resolved via `PATH` by the process spawner (`std::process::Command`
/// / `portable_pty::CommandBuilder`), not by an absolute path, matching
/// `core/lsp_manager.rs`'s existing `cmd /C` vs `sh -c` split in vimcode.
///
/// # Example
///
/// ```
/// use quadraui::terminal_engine::shell_command;
///
/// let (shell, flag) = shell_command();
/// let mut cmd = std::process::Command::new(shell);
/// cmd.arg(flag).arg("echo hello");
/// ```
pub fn shell_command() -> (String, String) {
    #[cfg(target_os = "windows")]
    {
        shell_command_for(ShellPlatform::Windows, std::env::var("COMSPEC").ok())
    }
    #[cfg(not(target_os = "windows"))]
    {
        shell_command_for(ShellPlatform::Unix, std::env::var("SHELL").ok())
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

    // ── Regression: vt100 rows<2 / cols<2 grid panics (quadraui#1130) ───────
    //
    // Found by this file's `vt100_parser_never_panics*` property tests: vt100
    // 0.16.2 panics constructing or resizing a grid with `rows < 2`
    // (`grid.rs:683`, "attempt to subtract with overflow") *and*, via a
    // different code path, with `cols == 1` and certain multi-byte input
    // regardless of `rows` (`screen.rs:730`, same panic message).
    // `MIN_VT100_ROWS`/`MIN_VT100_COLS` floor every caller-supplied
    // dimension before it ever reaches vt100 — see their docs for why a
    // real caller can plausibly hit this (a pane dragged down to a single
    // row/column, or momentarily to zero mid-resize).
    //
    // Two tiers, because the clamp is portable but the proof that
    // `spawn`/`resize` actually apply it is not: `clamp_vt100_size` and the
    // bare-parser no-panic checks run on every platform, while the four
    // end-to-end `TerminalSession` tests need a real PTY child (`/bin/sh`)
    // and so are `cfg(unix)`, matching every other PTY test in this file.

    #[test]
    fn clamp_vt100_size_floors_degenerate_dimensions() {
        // Zero — the momentary size a pane can report mid-drag.
        assert_eq!(
            clamp_vt100_size(0, 0),
            (MIN_VT100_COLS, MIN_VT100_ROWS),
            "a zero-sized pane must be floored on both axes"
        );
        // One row / one column — the two distinct upstream panic sites.
        assert_eq!(clamp_vt100_size(80, 1), (80, MIN_VT100_ROWS));
        assert_eq!(clamp_vt100_size(1, 24), (MIN_VT100_COLS, 24));
        // At and above the floor the caller's request is passed through
        // untouched — the clamp must not quietly resize a healthy pane.
        assert_eq!(
            clamp_vt100_size(MIN_VT100_COLS, MIN_VT100_ROWS),
            (MIN_VT100_COLS, MIN_VT100_ROWS)
        );
        assert_eq!(clamp_vt100_size(200, 60), (200, 60));
    }

    /// The floor is only worth anything if vt100 survives *at* it: build and
    /// resize a parser at exactly `MIN_VT100_ROWS` x `MIN_VT100_COLS`, and
    /// feed it the multi-byte input that triggers the `cols == 1`
    /// `screen.rs:730` overflow. Bare parser, no PTY — runs everywhere.
    #[test]
    fn vt100_survives_at_the_clamped_floor() {
        let (cols, rows) = clamp_vt100_size(0, 0);
        let mut p = vt100::Parser::new(rows, cols, 0);
        p.process("あéa\r\n\u{1b}[31mx".as_bytes());
        p.screen_mut().set_size(rows, cols);
        p.process("あ".as_bytes());
        assert_eq!(
            p.screen().size(),
            (MIN_VT100_ROWS, MIN_VT100_COLS),
            "the floored grid must survive construction, resize and wide-char input"
        );
    }

    #[test]
    #[cfg(unix)]
    fn spawn_with_a_single_row_does_not_panic() {
        let cwd = std::env::current_dir().expect("cwd");
        let mut sess = TerminalSession::spawn(80, 1, "/bin/sh", &cwd, 1000)
            .expect("spawn must succeed even with rows clamped internally");
        // The clamp is real, not just "didn't crash": the live grid must
        // actually be `MIN_VT100_ROWS` rows, not the requested 1.
        assert_eq!(sess.rows(), MIN_VT100_ROWS);
        sess.send_str("exit\n");
    }

    #[test]
    #[cfg(unix)]
    fn spawn_with_a_single_column_does_not_panic() {
        let cwd = std::env::current_dir().expect("cwd");
        let mut sess = TerminalSession::spawn(1, 24, "/bin/sh", &cwd, 1000)
            .expect("spawn must succeed even with cols clamped internally");
        assert_eq!(sess.cols(), MIN_VT100_COLS);
        sess.send_str("exit\n");
    }

    #[test]
    #[cfg(unix)]
    fn resize_to_a_single_row_does_not_panic() {
        let cwd = std::env::current_dir().expect("cwd");
        let mut sess = TerminalSession::spawn(80, 24, "/bin/sh", &cwd, 1000).expect("spawn failed");
        sess.resize(80, 1);
        assert_eq!(sess.rows(), MIN_VT100_ROWS);
        sess.send_str("exit\n");
    }

    #[test]
    #[cfg(unix)]
    fn resize_to_a_single_column_does_not_panic() {
        let cwd = std::env::current_dir().expect("cwd");
        let mut sess = TerminalSession::spawn(80, 24, "/bin/sh", &cwd, 1000).expect("spawn failed");
        sess.resize(1, 24);
        assert_eq!(sess.cols(), MIN_VT100_COLS);
        sess.send_str("exit\n");
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

    /// Pinned regression for quadraui#1130: the shrink must not leave a
    /// double-width glyph's first half orphaned at the new final column.
    ///
    /// These are the exact inputs proptest shrank the Windows CI failure to
    /// (`vt100_parser_never_panics_across_a_resize`, run
    /// 36607044052) — a TAB, 24 spaces and U+3000 IDEOGRAPHIC SPACE on a
    /// 2×65 grid, narrowed to 2×33 so the glyph straddles column 33, then a
    /// single space written over it. Before
    /// `set_size_without_orphaning_wide_cells` this panicked inside vt100 at
    /// `screen.rs:870` (`Option::unwrap` on the out-of-range continuation
    /// cell). No PTY, so it runs on every platform — Windows is where it
    /// actually fired.
    #[test]
    fn reflow_shrink_does_not_orphan_a_wide_glyph_at_the_boundary() {
        let mut p = vt100::Parser::new(2, 65, 0);
        let mut before = vec![b'\t'];
        before.extend_from_slice(&[b' '; 24]);
        before.extend_from_slice("\u{3000}".as_bytes());
        p.process(&before);
        assert!(
            p.screen().cell(0, 32).is_some_and(vt100::Cell::is_wide),
            "fixture drifted: the wide glyph must sit on the truncation boundary"
        );

        reflow_screen(&mut p, 2, 33);

        // The orphan is gone: the cell that survived the truncation is no
        // longer a first half missing its partner.
        let (_, cols) = p.screen().size();
        assert_eq!(cols, 33);
        for row in 0..2 {
            assert!(
                !p.screen()
                    .cell(row, cols - 1)
                    .is_some_and(vt100::Cell::is_wide),
                "row {row} still ends in an orphaned wide first half"
            );
        }
        // …and writing over it no longer panics (this was the CI failure).
        p.process(b"\x1b[1;33H ");
    }

    /// Same boundary, but on the **alternate** screen — `reflow_screen`
    /// deliberately skips the snapshot→replay reflow there (full-screen apps
    /// repaint themselves), so that branch needs its own coverage that the
    /// scrub still runs. quadraui#1130.
    #[test]
    fn reflow_shrink_does_not_orphan_a_wide_glyph_on_the_alternate_screen() {
        let mut p = vt100::Parser::new(2, 65, 0);
        p.process(b"\x1b[?1049h");
        let mut before = vec![b'\t'];
        before.extend_from_slice(&[b' '; 24]);
        before.extend_from_slice("\u{3000}".as_bytes());
        p.process(&before);
        assert!(p.screen().alternate_screen());

        reflow_screen(&mut p, 2, 33);

        let (_, cols) = p.screen().size();
        assert!(
            !p.screen()
                .cell(0, cols - 1)
                .is_some_and(vt100::Cell::is_wide),
            "alternate screen still ends in an orphaned wide first half"
        );
        p.process(b"\x1b[1;33H ");
    }

    /// The scrub is not allowed to move the cursor: it brackets its repair
    /// with DECSC/DECRC precisely so a resize stays invisible to the child.
    /// quadraui#1130.
    #[test]
    fn reflow_shrink_scrub_restores_the_cursor() {
        let mut p = vt100::Parser::new(4, 65, 0);
        let mut before = vec![b'\t'];
        before.extend_from_slice(&[b' '; 24]);
        before.extend_from_slice("\u{3000}".as_bytes());
        p.process(&before);
        // Park the cursor somewhere the scrub would clobber if it leaked.
        p.process(b"\x1b[3;5H");
        assert_eq!(p.screen().cursor_position(), (2, 4));

        reflow_screen(&mut p, 4, 33);

        assert_eq!(
            p.screen().cursor_position(),
            (2, 4),
            "resize must not move the cursor"
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

    // ── shell_command / shell_command_for (quadraui#970) ──────────────────────

    #[test]
    fn shell_command_for_unix_honours_shell_env_var() {
        let (shell, flag) = shell_command_for(ShellPlatform::Unix, Some("/bin/zsh".to_string()));
        assert_eq!(shell, "/bin/zsh");
        assert_eq!(flag, "-c");
    }

    #[test]
    fn shell_command_for_unix_falls_back_when_shell_env_var_unset() {
        let (shell, flag) = shell_command_for(ShellPlatform::Unix, None);
        assert_eq!(shell, "sh");
        assert_eq!(flag, "-c");
    }

    #[test]
    fn shell_command_for_windows_honours_comspec_env_var() {
        let (shell, flag) = shell_command_for(
            ShellPlatform::Windows,
            Some(r"C:\Windows\System32\cmd.exe".to_string()),
        );
        assert_eq!(shell, r"C:\Windows\System32\cmd.exe");
        assert_eq!(flag, "/C");
    }

    #[test]
    fn shell_command_for_windows_falls_back_when_comspec_env_var_unset() {
        let (shell, flag) = shell_command_for(ShellPlatform::Windows, None);
        assert_eq!(shell, "cmd");
        assert_eq!(flag, "/C");
    }

    /// `shell_command()` itself (as opposed to the pure `shell_command_for`
    /// helper above) picks its platform branch via `cfg(target_os =
    /// "windows")`, so this only exercises whichever branch the current
    /// build host compiles — the platform-parameterised tests above are
    /// what cover both branches regardless of host OS.
    #[test]
    fn shell_command_returns_nonempty_pair() {
        let (shell, flag) = shell_command();
        assert!(!shell.is_empty());
        assert!(!flag.is_empty());
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

    /// `TerminalSession::paste` resets the scroll offset back to the live
    /// view — mirroring ordinary keystroke input — so a paste is always
    /// visible immediately even if the user had scrolled into history.
    /// (Real-PTY test since `scroll_reset`/`cursor_visible` are session
    /// state, not the pure `encode_paste` byte-encoding covered above.)
    #[test]
    #[cfg(unix)]
    fn paste_resets_scroll_to_live_view() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(80, 24, "/bin/sh", &cwd, 1000).expect("spawn failed");

        for _ in 0..30 {
            sess.send_str("echo line\n");
        }
        let _ = poll_until(&mut sess, 5000, |s| s.history_len() > 0);

        sess.scroll_up(5);
        assert!(
            !sess.cursor_visible(),
            "precondition: scrolled into history should hide the cursor"
        );

        sess.paste("hello");
        assert!(
            sess.cursor_visible(),
            "paste should reset scroll back to the live view"
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

    /// [`TerminalSession::title`] tracks OSC 0/OSC 2 window-title requests
    /// (quadraui #339), and [`TerminalSession::take_title_changed`] is a
    /// one-shot dirty flag consumed by the read.
    #[test]
    #[cfg(unix)]
    fn title_tracks_osc_0_and_take_title_changed_is_one_shot() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 24, "/bin/sh", &cwd, 1000).expect("failed to spawn /bin/sh");

        // No title until the child sets one.
        assert_eq!(sess.title(), None);
        assert!(!sess.take_title_changed());

        // OSC 0 sets the window title (`\033]0;<text>\007`) — same sequence
        // vim/tmux/shell prompt hooks emit. `\033` is a POSIX printf octal
        // escape.
        sess.send_str("printf '\\033]0;hello-quadraui\\007'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.title() == Some("hello-quadraui")),
            "title() should track the child's OSC 0 sequence"
        );

        // The dirty flag fires exactly once per change...
        assert!(
            sess.take_title_changed(),
            "take_title_changed should report the change once"
        );
        // ...and is consumed by the read.
        assert!(
            !sess.take_title_changed(),
            "take_title_changed should not re-fire until the title changes again"
        );

        // Re-setting to a *different* title dirties it again.
        sess.send_str("printf '\\033]2;second-title\\007'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.title() == Some("second-title")),
            "title() should track a later OSC 2 sequence too"
        );
        assert!(sess.take_title_changed());

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

    /// Wheel events encode the pointer's column/row on the wire just like
    /// clicks do (quadraui#365 review) — real alt-screen consumers (tmux is
    /// the canonical example) use this to route a wheel notch to the pane
    /// under the cursor, so a caller must never collapse it to `(0, 0)`.
    #[test]
    fn encode_mouse_sgr_wheel_up_encodes_nonzero_position() {
        let bytes = encode_mouse_sgr(
            TerminalMouseKind::WheelUp,
            MouseButton::Left,
            5,
            10,
            Modifiers::default(),
        );
        // 1-indexed on the wire: col 5 -> 6, row 10 -> 11.
        assert_eq!(bytes, b"\x1b[<64;6;11M");
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

    // ── Paste → PTY encoding (quadraui#415) ──────────────────────────────────

    /// Bracketed-paste mode on: `text` gets wrapped in `ESC[200~` /
    /// `ESC[201~` markers so the child can tell pasted text apart from
    /// typed text.
    #[test]
    fn encode_paste_wraps_when_bracketed() {
        let bytes = encode_paste("hello", true);
        assert_eq!(bytes, b"\x1b[200~hello\x1b[201~");
    }

    /// Bracketed-paste mode off: `text` is sent completely raw — no
    /// markers. Wrapping unconditionally would leak literal escape bytes
    /// into programs that never asked for bracketed paste.
    #[test]
    fn encode_paste_is_raw_when_not_bracketed() {
        let bytes = encode_paste("hello", false);
        assert_eq!(bytes, b"hello");
    }

    /// Multi-line paste (the case bracketed paste exists to protect —
    /// without it, a shell would try to run each line as it arrives)
    /// wraps the whole blob once, not per line.
    #[test]
    fn encode_paste_wraps_multiline_text_once() {
        let bytes = encode_paste("echo one\necho two\n", true);
        assert_eq!(bytes, b"\x1b[200~echo one\necho two\n\x1b[201~");
    }

    /// Non-ASCII UTF-8 text round-trips through the wrap byte-for-byte.
    #[test]
    fn encode_paste_preserves_utf8() {
        let bytes = encode_paste("日本語 🎉", true);
        let mut expected = b"\x1b[200~".to_vec();
        expected.extend_from_slice("日本語 🎉".as_bytes());
        expected.extend_from_slice(b"\x1b[201~");
        assert_eq!(bytes, expected);
    }

    /// Empty paste still gets wrapped (a paste of "nothing" is still a
    /// paste event as far as the child's bracketed-paste state machine is
    /// concerned) — no special-casing to drop the markers.
    #[test]
    fn encode_paste_wraps_empty_text() {
        let bytes = encode_paste("", true);
        assert_eq!(bytes, b"\x1b[200~\x1b[201~");
    }

    // ── Keyboard → PTY encoding (moved from examples/common/terminal_app.rs, quadraui#342) ──

    // ── Normal-mode key encoding (DECCKM off / app_cursor = false) ───────────

    #[test]
    fn ctrl_c_is_etx() {
        let bytes = key_to_pty_bytes(
            Key::Char('c'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
            false,
        );
        assert_eq!(bytes, Some(vec![0x03])); // ETX
    }

    #[test]
    fn ctrl_d_is_eot() {
        let bytes = key_to_pty_bytes(
            Key::Char('d'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
            false,
        );
        assert_eq!(bytes, Some(vec![0x04])); // EOT
    }

    #[test]
    fn printable_char_passes_through() {
        let bytes = key_to_pty_bytes(Key::Char('a'), Modifiers::default(), false).unwrap();
        assert_eq!(bytes, b"a");
    }

    #[test]
    fn enter_is_cr() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::Enter), Modifiers::default(), false).unwrap();
        assert_eq!(bytes, b"\r");
    }

    #[test]
    fn up_arrow_plain() {
        // Normal mode (app_cursor = false) → CSI sequence.
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::Up), Modifiers::default(), false).unwrap();
        assert_eq!(bytes, b"\x1b[A");
    }

    #[test]
    fn up_arrow_ctrl() {
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::Up),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        // ctrl mod_param = 5 → "\x1b[1;5A"
        assert_eq!(bytes, b"\x1b[1;5A");
    }

    #[test]
    fn f1_plain() {
        // F1 unmodified must emit the SS3 sequence \x1bOP, NOT the CSI \x1b[P.
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::F(1)), Modifiers::default(), false).unwrap();
        assert_eq!(bytes, b"\x1bOP");
    }

    #[test]
    fn f2_plain() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::F(2)), Modifiers::default(), false).unwrap();
        assert_eq!(bytes, b"\x1bOQ");
    }

    #[test]
    fn f3_plain() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::F(3)), Modifiers::default(), false).unwrap();
        assert_eq!(bytes, b"\x1bOR");
    }

    #[test]
    fn f4_plain() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::F(4)), Modifiers::default(), false).unwrap();
        assert_eq!(bytes, b"\x1bOS");
    }

    #[test]
    fn f1_ctrl_uses_csi() {
        // Ctrl+F1 → \x1b[1;5P (CSI modifier form, not SS3).
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::F(1)),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[1;5P");
    }

    #[test]
    fn delete_plain() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::Delete), Modifiers::default(), false).unwrap();
        assert_eq!(bytes, b"\x1b[3~");
    }

    #[test]
    fn page_up_plain() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::PageUp), Modifiers::default(), false).unwrap();
        assert_eq!(bytes, b"\x1b[5~");
    }

    #[test]
    fn delete_ctrl() {
        // Tilde-terminated keys with a modifier must use the form `\x1b[<code>;<mod>~`,
        // NOT `\x1b[1;<mod><code>~` (which is the cursor-letter form).
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::Delete),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[3;5~");
    }

    #[test]
    fn page_up_ctrl() {
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::PageUp),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[5;5~");
    }

    #[test]
    fn home_shift() {
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::Home),
            Modifiers {
                shift: true,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        // shift mod_param = 2 → "\x1b[1;2~"  (Home code = "1")
        assert_eq!(bytes, b"\x1b[1;2~");
    }

    #[test]
    fn end_alt() {
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::End),
            Modifiers {
                alt: true,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        // alt mod_param = 3 → "\x1b[4;3~"  (End code = "4")
        assert_eq!(bytes, b"\x1b[4;3~");
    }

    #[test]
    fn f5_ctrl() {
        // F5+ are tilde-terminated; Ctrl+F5 → \x1b[15;5~ (not \x1b[1;515~).
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::F(5)),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[15;5~");
    }

    #[test]
    fn page_down_shift_ctrl() {
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::PageDown),
            Modifiers {
                shift: true,
                ctrl: true,
                ..Default::default()
            },
            false,
        )
        .unwrap();
        // shift+ctrl mod_param = 6 → "\x1b[6;6~"  (PageDown code = "6")
        assert_eq!(bytes, b"\x1b[6;6~");
    }

    #[test]
    fn caps_lock_is_none() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::CapsLock), Modifiers::default(), false);
        assert!(bytes.is_none());
    }

    // ── Application-cursor-keys mode (DECCKM on / app_cursor = true) ─────────

    /// In application-cursor mode, unmodified Up emits `ESC O A` (SS3).
    #[test]
    fn app_cursor_up_plain_is_ss3() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::Up), Modifiers::default(), true).unwrap();
        assert_eq!(bytes, b"\x1bOA");
    }

    /// In application-cursor mode, unmodified Down emits `ESC O B`.
    #[test]
    fn app_cursor_down_plain_is_ss3() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::Down), Modifiers::default(), true).unwrap();
        assert_eq!(bytes, b"\x1bOB");
    }

    /// In application-cursor mode, unmodified Right emits `ESC O C`.
    #[test]
    fn app_cursor_right_plain_is_ss3() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::Right), Modifiers::default(), true).unwrap();
        assert_eq!(bytes, b"\x1bOC");
    }

    /// In application-cursor mode, unmodified Left emits `ESC O D`.
    #[test]
    fn app_cursor_left_plain_is_ss3() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::Left), Modifiers::default(), true).unwrap();
        assert_eq!(bytes, b"\x1bOD");
    }

    /// In application-cursor mode, unmodified Home emits `ESC O H`.
    #[test]
    fn app_cursor_home_plain_is_ss3() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::Home), Modifiers::default(), true).unwrap();
        assert_eq!(bytes, b"\x1bOH");
    }

    /// In application-cursor mode, unmodified End emits `ESC O F`.
    #[test]
    fn app_cursor_end_plain_is_ss3() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::End), Modifiers::default(), true).unwrap();
        assert_eq!(bytes, b"\x1bOF");
    }

    /// Even in application-cursor mode, a modifier makes arrows fall back to
    /// the CSI form `ESC [ 1 ; <mod> A` — xterm does the same.
    #[test]
    fn app_cursor_up_ctrl_falls_back_to_csi() {
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::Up),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
            true,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[1;5A");
    }

    /// Even in application-cursor mode, Shift+Up falls back to CSI form.
    #[test]
    fn app_cursor_up_shift_falls_back_to_csi() {
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::Up),
            Modifiers {
                shift: true,
                ..Default::default()
            },
            true,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[1;2A");
    }

    /// Home with a modifier falls back to tilde-CSI even in app-cursor mode.
    #[test]
    fn app_cursor_home_shift_falls_back_to_csi() {
        let bytes = key_to_pty_bytes(
            Key::Named(NamedKey::Home),
            Modifiers {
                shift: true,
                ..Default::default()
            },
            true,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[1;2~");
    }

    /// PageUp is never affected by DECCKM — always `ESC [ 5 ~`.
    #[test]
    fn app_cursor_page_up_unchanged() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::PageUp), Modifiers::default(), true).unwrap();
        assert_eq!(bytes, b"\x1b[5~");
    }

    /// PageDown is never affected by DECCKM — always `ESC [ 6 ~`.
    #[test]
    fn app_cursor_page_down_unchanged() {
        let bytes =
            key_to_pty_bytes(Key::Named(NamedKey::PageDown), Modifiers::default(), true).unwrap();
        assert_eq!(bytes, b"\x1b[6~");
    }

    /// `encode_key`/`write_key` read `application_cursor_keys()` off the
    /// live session instead of requiring the caller to thread the flag
    /// through by hand — the whole point of moving this into the engine
    /// (quadraui#342).
    #[test]
    #[cfg(unix)]
    fn encode_key_reads_app_cursor_from_session() {
        let cwd = std::env::temp_dir();
        let sess =
            TerminalSession::spawn(80, 10, "/bin/sh", &cwd, 100).expect("failed to spawn /bin/sh");
        assert!(!sess.application_cursor_keys());
        let bytes = sess
            .encode_key(Key::Named(NamedKey::Up), Modifiers::default())
            .unwrap();
        // Not in application-cursor mode yet → plain CSI form.
        assert_eq!(bytes, b"\x1b[A");
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

    // ── `handle_wheel` — forward-vs-scrollback policy (quadraui#365) ─────────

    /// With no mouse reporting and no alt-screen, `handle_wheel` falls back
    /// to local scrollback: returns `false`, and `scroll_offset` moves by
    /// exactly `step`.
    #[test]
    #[cfg(unix)]
    fn handle_wheel_scrolls_locally_without_forwarding() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 10, "/bin/sh", &cwd, 100).expect("failed to spawn /bin/sh");
        // Give it enough history to scroll into.
        for _ in 0..20 {
            sess.send_str("echo line\n");
        }
        let _ = poll_until(&mut sess, 2000, |s| s.history_len() >= 10);

        assert!(!sess.should_forward_wheel());

        // Distinct, nonzero col/row on each call to prove they're accepted
        // (and would be threaded through to the child) even though this
        // path falls back to local scrollback.
        let forwarded = sess.handle_wheel(true, 3, 12, 4);
        assert!(!forwarded);
        assert_eq!(sess.scroll_offset(), 3);

        let forwarded = sess.handle_wheel(false, 2, 7, 9);
        assert!(!forwarded);
        assert_eq!(sess.scroll_offset(), 1);

        sess.send_str("exit\n");
    }

    /// Once the child owns the wheel (mouse reporting on), `handle_wheel`
    /// forwards instead of touching local scrollback: returns `true`, and
    /// `scroll_offset` is untouched.
    #[test]
    #[cfg(unix)]
    fn handle_wheel_forwards_when_child_owns_it() {
        let cwd = std::env::temp_dir();
        let mut sess =
            TerminalSession::spawn(80, 10, "/bin/sh", &cwd, 100).expect("failed to spawn /bin/sh");

        sess.send_str("printf '\\033[?1000h'\n");
        assert!(
            poll_until(&mut sess, 5000, |s| s.mouse_reporting_enabled()),
            "mouse reporting did not turn on"
        );

        let before = sess.scroll_offset();
        let forwarded = sess.handle_wheel(true, 3, 12, 4);
        assert!(forwarded);
        assert_eq!(sess.scroll_offset(), before);

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
                text: chars
                    .get(i)
                    .map_or_else(|| " ".to_string(), |c| c.to_string()),
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

    // ── Grapheme cluster / wide-char cell content (quadraui#337) ───────

    /// `build_rows()`'s live-screen branch must carry a cell's *full*
    /// grapheme cluster — base character plus any combining marks — not
    /// just the first `char`. Feeds vt100 directly via `sess.parser`
    /// (same technique as `resize_preserves_wide_chars`) so the byte
    /// content is exact and doesn't depend on shell echo timing.
    #[test]
    #[cfg(unix)]
    fn build_rows_live_screen_preserves_combining_mark_grapheme() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(10, 4, "/bin/sh", &cwd, 100).expect("spawn failed");

        // 'e' + U+0301 COMBINING ACUTE ACCENT — a two-codepoint grapheme
        // cluster vt100 stores in a single cell (`Cell::contents()`
        // returns both codepoints for one column).
        sess.parser.process("e\u{0301}".as_bytes());

        let rows = sess.build_rows(false);
        assert_eq!(
            rows[0][0].text, "e\u{0301}",
            "combining mark must survive into the cell's text, not just 'e'"
        );
        assert_eq!(
            rows[0][0].cell_width(),
            1,
            "base char + combining accent is still one column wide"
        );
        // The accent must not have consumed a second column — column 1
        // is still blank.
        assert_eq!(rows[0][1].text, " ");

        sess.send_str("exit\n");
    }

    /// `build_rows()`'s live-screen branch must reserve a two-column box
    /// for a wide (CJK) glyph and leave vt100's blank continuation
    /// column as-is, so column accounting downstream (TUI/GTK/macOS/win
    /// rasterisers) matches vt100's own layout exactly.
    #[test]
    #[cfg(unix)]
    fn build_rows_live_screen_wide_char_reserves_continuation_column() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(10, 4, "/bin/sh", &cwd, 100).expect("spawn failed");

        // '日' is double-width; vt100 gives it column 0 and leaves
        // column 1 as an empty continuation cell.
        sess.parser.process("日B".as_bytes());

        let rows = sess.build_rows(false);
        assert_eq!(rows[0][0].text, "日");
        assert_eq!(rows[0][0].cell_width(), 2);
        assert_eq!(
            rows[0][1].text, " ",
            "wide glyph's continuation column must be blank, not a second copy"
        );
        // 'B' lands in column 2 — the continuation column at index 1
        // wasn't (incorrectly) treated as a second content column.
        assert_eq!(rows[0][2].text, "B");
        assert_eq!(rows[0][2].cell_width(), 1);

        sess.send_str("exit\n");
    }

    // ── SGR 2 faint/dim (quadraui#345) ──────────────────────────────────
    //
    // vt100 0.15.2 didn't track SGR 2 at all — quadraui#345 was filed
    // against that gap. This repo is now pinned to vt100 ≥ 0.16 (see
    // `Cargo.toml`), which added `Cell::dim()`; these tests cover the
    // plumbing that reads it into `TerminalCell::dim` on both the live
    // and history paths. The colour-blending consequence of `dim = true`
    // (fading the resolved foreground toward the background) is covered
    // separately by `terminal_style::tests` — this file only needs to
    // prove the flag itself survives `build_rows()`.

    /// `build_rows()`'s live-screen branch must carry vt100's SGR 2
    /// (faint) attribute into `TerminalCell::dim`. Feeds vt100 directly
    /// via `sess.parser` (same technique as
    /// `build_rows_live_screen_preserves_combining_mark_grapheme`) so the
    /// SGR sequence is exact and doesn't depend on shell echo timing.
    #[test]
    #[cfg(unix)]
    fn build_rows_live_screen_carries_dim_attribute() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(10, 4, "/bin/sh", &cwd, 100).expect("spawn failed");

        // ESC[2m = SGR 2 (faint/dim) → 'D'. ESC[0m resets before 'N', a
        // normal-intensity glyph in the next column.
        sess.parser.process(b"\x1b[2mD\x1b[0mN");

        let rows = sess.build_rows(false);
        assert!(
            rows[0][0].dim,
            "cell painted under SGR 2 must carry dim = true"
        );
        assert!(
            !rows[0][1].dim,
            "cell painted after SGR 0 reset must not be dim"
        );

        sess.send_str("exit\n");
    }

    /// `build_rows()`'s history branch must also carry a captured cell's
    /// `dim` flag — mirrors `build_rows_highlights_selection_in_history`'s
    /// pure-unit technique (inject `HistCell`s directly, no PTY timing).
    #[test]
    #[cfg(unix)]
    fn build_rows_history_carries_dim_attribute() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(6, 4, "/bin/sh", &cwd, 100).expect("spawn failed");

        let mut row = make_hist_row_content("DIM ", sess.cols);
        row[0].dim = true;
        sess.history.push_back(row);
        sess.set_scroll_offset(1);

        let rows = sess.build_rows(false);
        assert!(
            rows[0][0].dim,
            "history cell's dim flag must survive into build_rows output"
        );
        assert!(!rows[0][1].dim, "non-dim history cells must stay non-dim");

        sess.send_str("exit\n");
    }

    /// `capture_scrolled_rows()` (the scrollback-capture path,
    /// `terminal_engine.rs` history capture — quadraui#337) must also
    /// preserve a full grapheme cluster rather than truncating to the
    /// first `char`. A 1-row screen makes a single newline immediately
    /// scroll that row into `history`, so `process_with_capture` (the
    /// private entry point `poll()` normally drives) can be called
    /// directly for a deterministic capture with no PTY/shell timing.
    #[test]
    #[cfg(unix)]
    fn capture_scrolled_rows_preserves_combining_mark_grapheme() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(10, 1, "/bin/sh", &cwd, 100).expect("spawn failed");

        sess.process_with_capture("e\u{0301}\n".as_bytes());

        assert_eq!(sess.history.len(), 1, "one row should have scrolled off");
        assert_eq!(
            sess.history[0][0].text, "e\u{0301}",
            "combining mark must survive scrollback capture, not just 'e'"
        );

        sess.send_str("exit\n");
    }

    /// `capture_scrolled_rows()` must also carry a captured cell's SGR 2
    /// (faint) attribute into `HistCell::dim` (quadraui#345) — same
    /// technique as the grapheme-cluster capture test above.
    #[test]
    #[cfg(unix)]
    fn capture_scrolled_rows_preserves_dim_attribute() {
        let cwd = std::env::temp_dir();
        let mut sess = TerminalSession::spawn(10, 1, "/bin/sh", &cwd, 100).expect("spawn failed");

        sess.process_with_capture(b"\x1b[2mD\x1b[0m\n");

        assert_eq!(sess.history.len(), 1, "one row should have scrolled off");
        assert!(
            sess.history[0][0].dim,
            "faint cell must survive scrollback capture as dim = true"
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

/// Property tests for the vt100 parser this engine wraps (quadraui#1130).
/// Everything fed to [`vt100::Parser::process`] is untrusted, foreign
/// data by construction — it's whatever bytes the child shell/program
/// running inside the PTY chooses to write, including a hostile or
/// buggy program deliberately or accidentally emitting malformed escape
/// sequences — so "never panics" needs to hold over the arbitrary byte
/// space, not just the hand-picked regression fixtures in `mod tests`
/// above (e.g. the #377 wide-char column-boundary case).
///
/// Deliberately drives a bare `vt100::Parser` rather than a full
/// [`TerminalSession`] — the latter needs a real PTY-spawned child
/// process per case, which is far too slow for proptest's
/// hundreds-of-cases-per-property default and adds nothing: the parser
/// itself, not the PTY plumbing around it, is what actually walks the
/// untrusted byte stream.
///
/// `rows`/`cols` are bounded below by [`MIN_VT100_ROWS`]/[`MIN_VT100_COLS`],
/// not `1`: this property test is *how* both upstream vt100 panics (see
/// those consts' docs) were originally found, by generating grid sizes
/// with no floor at all. That's now a pinned regression
/// (`clamp_vt100_size_floors_degenerate_dimensions` /
/// `vt100_survives_at_the_clamped_floor` on every platform, plus the
/// `cfg(unix)` PTY pair `spawn_with_a_single_{row,column}_does_not_panic` /
/// `resize_to_a_single_{row,column}_does_not_panic` in `mod tests` above)
/// covering the boundary this crate actually controls — where
/// `TerminalSession` clamps *before* constructing the parser. Re-widening
/// the range here would just rediscover the same, already-filed upstream
/// vt100 bugs on every run instead of testing anything this crate can fix.
#[cfg(test)]
mod proptests {
    use super::{reflow_screen, MIN_VT100_COLS, MIN_VT100_ROWS};
    use proptest::prelude::*;

    proptest! {
        /// No byte sequence, however malformed (a truncated CSI, an
        /// out-of-range SGR parameter, a lone high continuation byte, a
        /// wide-char glyph landing exactly on a column boundary — the
        /// #377 regression class), may panic the parser at any
        /// reasonable terminal size.
        #[test]
        fn vt100_parser_never_panics(
            rows in MIN_VT100_ROWS..60,
            cols in MIN_VT100_COLS..200,
            bytes in prop::collection::vec(any::<u8>(), 0..2000),
        ) {
            let mut parser = vt100::Parser::new(rows, cols, 0);
            parser.process(&bytes);
        }

        /// Same, but the byte stream is valid UTF-8 text (still
        /// arbitrary — no attempt to keep it well-formed *terminal*
        /// output) — the shape a foreign program's plain-text output,
        /// as opposed to raw garbage, actually takes.
        #[test]
        fn vt100_parser_never_panics_on_arbitrary_utf8(
            rows in MIN_VT100_ROWS..60,
            cols in MIN_VT100_COLS..200,
            s in ".{0,2000}",
        ) {
            let mut parser = vt100::Parser::new(rows, cols, 0);
            parser.process(s.as_bytes());
        }

        /// A resize mid-stream followed by more arbitrary bytes must not
        /// panic either — this is the exact scenario [`reflow_screen`]'s
        /// doc comment and the #377 fixtures above both call out as the
        /// highest-risk path (wide-char cells re-wrapping at a new width).
        ///
        /// Drives [`reflow_screen`] — the *crate's* resize entry point,
        /// the one and only thing [`TerminalSession::resize`] calls — and
        /// deliberately not `vt100::Screen::set_size` underneath it. The
        /// bare upstream call is known-broken on a width shrink and this
        /// crate stopped making it: see
        /// [`set_size_without_orphaning_wide_cells`], whose repair is what
        /// this property pins. Asserting the property against raw
        /// `set_size` would only re-derive that upstream bug on every run,
        /// the same dead end [`MIN_VT100_ROWS`]'s doc describes for grid
        /// dimensions.
        ///
        /// `alternate` covers both branches of `reflow_screen`'s gate:
        /// `false` takes the snapshot→resize→replay reflow, `true` the
        /// plain non-reflow resize a full-screen app gets. Both were
        /// affected by the orphaned-wide-cell bug.
        #[test]
        fn vt100_parser_never_panics_across_a_resize(
            rows in MIN_VT100_ROWS..60,
            cols in MIN_VT100_COLS..200,
            new_rows in MIN_VT100_ROWS..60,
            new_cols in MIN_VT100_COLS..200,
            alternate in any::<bool>(),
            before in prop::collection::vec(any::<u8>(), 0..1000),
            after in prop::collection::vec(any::<u8>(), 0..1000),
        ) {
            let mut parser = vt100::Parser::new(rows, cols, 0);
            if alternate {
                parser.process(b"\x1b[?1049h");
            }
            parser.process(&before);
            reflow_screen(&mut parser, new_rows, new_cols);
            parser.process(&after);
        }

        /// The orphaned-wide-cell shrink (quadraui#1130) narrowed to its
        /// essentials and re-randomised: a double-width glyph parked so
        /// that the shrink lands between its two halves, on every
        /// (old width, new width) pair that can straddle it.
        ///
        /// The generic property above finds this too, but only on a lucky
        /// seed — it was a ~5% hit rate there, which is why it reached CI
        /// green on Linux and red on Windows. This one hits it every run.
        #[test]
        fn reflow_survives_a_wide_glyph_split_by_the_new_width(
            rows in MIN_VT100_ROWS..8,
            new_cols in 4u16..40,
            extra in 0u16..12,
            alternate in any::<bool>(),
        ) {
            let cols = new_cols + 1 + extra;
            let mut parser = vt100::Parser::new(rows, cols, 0);
            if alternate {
                parser.process(b"\x1b[?1049h");
            }
            // Pad so the wide glyph's first half lands exactly on the
            // column that is about to become the last one, and its
            // continuation half on the first column to be truncated.
            let mut bytes = vec![b' '; usize::from(new_cols) - 1];
            bytes.extend_from_slice("\u{3000}".as_bytes());
            parser.process(&bytes);
            prop_assert!(
                parser.screen().cell(0, new_cols - 1).is_some_and(vt100::Cell::is_wide),
                "fixture must park a wide first half on the truncation boundary"
            );
            reflow_screen(&mut parser, rows, new_cols);
            // Writing over the (formerly) orphaned cell is what panicked.
            parser.process(b"\x1b[1;1H");
            parser.process(&vec![b'x'; usize::from(new_cols) * 2]);
        }
    }
}
