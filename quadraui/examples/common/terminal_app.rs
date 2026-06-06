//! Backend-agnostic `AppLogic` for the terminal engine example
//! ([`tui_terminal`]).
//!
//! [`TerminalApp`] spawns a single interactive shell session via
//! [`TerminalSession`] and drives it through the standard
//! `AppLogic::render` / `AppLogic::handle` / `AppLogic::tick` pattern.
//!
//! Controls:
//! - All printable characters are forwarded to the PTY.
//! - Arrow keys, Tab, Backspace, Enter, Delete, Home, End, PgUp/PgDn
//!   are translated to the appropriate VT100 escape sequences.
//! - **Shift+PageUp / Shift+PageDown** scroll the history view (not sent to PTY).
//! - **Shift+Home** jumps to the oldest available history line.
//! - Ctrl+C / Ctrl+D / Ctrl+Z forward as terminal control bytes.
//! - Scroll wheel forwards to the PTY when the child has mouse reporting enabled
//!   or is on the alternate screen (e.g. tmux, vim, less); otherwise scrolls local
//!   history (3 rows per notch).
//! - Click (press/release) is forwarded to the PTY when mouse reporting is enabled.
//! - Dragging the scrollbar thumb scrolls the history.
//! - Window resize triggers a PTY resize.
//! - Ctrl+Q quits.
//! - Bracketed paste is forwarded verbatim.
//! - When the shell exits a status line shows the exit code; Ctrl+Q closes.

use quadraui::terminal_engine::{default_shell, TerminalMouseKind, TerminalSession};
use quadraui::{
    AppLogic, Backend, ButtonMask, Color, Key, Modifiers, MouseButton, NamedKey, Reaction, Rect,
    ScrollDelta, StatusBar, StatusBarSegment, UiEvent, Viewport, WidgetId,
};

// ── Layout ───────────────────────────────────────────────────────────────────

/// Rows reserved at the bottom of the viewport for the hint / status footer.
const FOOTER_ROWS: u16 = 1;

// ── Drag-tracking state ───────────────────────────────────────────────────────

/// Tracks an in-progress scrollbar-thumb drag.
///
/// All values are captured at `MouseDown` time so that `MouseMoved` has
/// stable geometry to compute from even if the viewport changes mid-drag.
struct ScrollbarDrag {
    /// Y coordinate of the top of the scrollbar track (TUI: always 0.0).
    track_y: f32,
    /// Height of the scrollbar track in rows.
    track_h: f32,
    /// Total line count from the scrollbar state snapshot.
    total: usize,
    /// Visible line count from the scrollbar state snapshot.
    visible: usize,
}

// ── TerminalApp ───────────────────────────────────────────────────────────────

/// Single-session terminal example app.
pub struct TerminalApp {
    session: Option<TerminalSession>,
    last_viewport: Viewport,
    /// Status line message (shown only when session is absent).
    error_msg: String,
    /// In-progress scrollbar thumb drag, if any.
    scrollbar_drag: Option<ScrollbarDrag>,
}

impl TerminalApp {
    pub fn new() -> Self {
        Self {
            session: None,
            last_viewport: Viewport::default(),
            error_msg: String::new(),
            scrollbar_drag: None,
        }
    }

    /// Return the PTY dimensions (cols, rows) from a viewport,
    /// reserving `FOOTER_ROWS` at the bottom for the hint bar.
    fn viewport_to_pty(vp: Viewport) -> (u16, u16) {
        // TUI backend: viewport units are cells (f32 but always integral).
        let cols = vp.width.max(10.0) as u16;
        let rows = (vp.height - FOOTER_ROWS as f32).max(3.0) as u16;
        (cols, rows)
    }

    /// Height of the terminal grid area (viewport height minus footer).
    fn term_height(vp: Viewport) -> f32 {
        (vp.height - FOOTER_ROWS as f32).max(0.0)
    }

    /// X column of the scrollbar (rightmost column of the viewport).
    fn scrollbar_col(vp: Viewport) -> f32 {
        (vp.width - 1.0).max(0.0)
    }

    // ── Footer rendering ──────────────────────────────────────────────────────

    /// Render the one-row hint / status footer at the very bottom of the viewport.
    ///
    /// - Normal operation: shows keyboard shortcuts.
    /// - Process exited: shows the exit code and prompts the user to quit.
    fn render_footer(&self, backend: &mut dyn Backend, vp: Viewport) {
        let y = vp.height - FOOTER_ROWS as f32;
        let footer_rect = Rect::new(0.0, y, vp.width, FOOTER_ROWS as f32);

        let (text, fg, bg) = if let Some(ref sess) = self.session {
            if sess.is_exited() {
                let code = sess.exit_code().unwrap_or(0);
                (
                    format!(" [process exited {code}] — press Ctrl+Q to close"),
                    Color::rgb(255, 200, 100),
                    Color::rgb(60, 30, 0),
                )
            } else {
                (
                    " Ctrl+Q quit  ·  wheel / Shift+PgUp scroll  ·  type to run".to_string(),
                    Color::rgb(160, 160, 160),
                    Color::rgb(40, 40, 40),
                )
            }
        } else {
            (
                " Ctrl+Q quit".to_string(),
                Color::rgb(160, 160, 160),
                Color::rgb(40, 40, 40),
            )
        };

        let bar = StatusBar {
            id: WidgetId::new("term-footer"),
            left_segments: vec![StatusBarSegment {
                text,
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        backend.draw_status_bar(footer_rect, &bar, None, None);
    }

    // ── Scrollbar drag helper ─────────────────────────────────────────────────

    /// Compute and apply a new `scroll_offset` from a drag Y position.
    ///
    /// Uses the geometry captured in `self.scrollbar_drag` at `MouseDown`
    /// time. The scrollbar is **inverted**: dragging the thumb to the top
    /// of the track shows the oldest history, dragging to the bottom shows
    /// the live view.
    fn apply_scrollbar_drag(&mut self, y: f32) {
        // Copy values out of `scrollbar_drag` before mutably borrowing `session`.
        let (track_y, track_h, max_scroll) = match &self.scrollbar_drag {
            Some(d) => (d.track_y, d.track_h, d.total.saturating_sub(d.visible)),
            None => return,
        };
        if track_h <= 0.0 {
            return;
        }
        // fraction 0 = top of track, 1 = bottom of track
        let fraction = ((y - track_y) / track_h).clamp(0.0, 1.0);
        // Inverted: top → max history, bottom → live (offset 0)
        let offset = ((1.0 - fraction) * max_scroll as f32).round() as usize;
        if let Some(ref mut sess) = self.session {
            sess.set_scroll_offset(offset);
        }
    }
}

impl Default for TerminalApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for TerminalApp {
    type AreaId = ();

    fn setup(&mut self, backend: &mut dyn Backend) {
        let vp = backend.viewport();
        self.last_viewport = vp;
        let (cols, rows) = Self::viewport_to_pty(vp);
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        let shell = default_shell();
        match TerminalSession::spawn(cols, rows, &shell, &cwd, 10_000) {
            Ok(sess) => self.session = Some(sess),
            Err(e) => self.error_msg = format!("Failed to spawn PTY: {e}"),
        }
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let term_h = Self::term_height(vp);
        let rect = Rect::new(0.0, 0.0, vp.width, term_h);

        if let Some(ref sess) = self.session {
            let total = sess.history_len() + sess.rows() as usize;
            let sb = if total > sess.rows() as usize {
                Some(sess.scrollbar_state(None))
            } else {
                None
            };
            let snapshot = sess.to_terminal(WidgetId::new("terminal:0"), sb);
            backend.draw_terminal(rect, &snapshot);
        } else {
            // No session — show an error in the status bar.
            let msg = if self.error_msg.is_empty() {
                "Spawning terminal…".to_string()
            } else {
                self.error_msg.clone()
            };
            let bar = StatusBar {
                id: WidgetId::new("status"),
                left_segments: vec![StatusBarSegment {
                    text: format!("  {msg}  "),
                    fg: Color::rgb(255, 80, 80),
                    bg: Color::rgb(40, 40, 40),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let bar_rect = Rect::new(0.0, rect.height - 1.0, rect.width, 1.0);
            backend.draw_status_bar(bar_rect, &bar, None, None);
        }

        self.render_footer(backend, vp);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            // ── Quit ─────────────────────────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                modifiers: Modifiers { ctrl: true, .. },
                ..
            } => return Reaction::Exit,

            // ── Resize ───────────────────────────────────────────────────────
            UiEvent::WindowResized { viewport } => {
                self.last_viewport = viewport;
                if let Some(ref mut sess) = self.session {
                    let (cols, rows) = Self::viewport_to_pty(viewport);
                    sess.resize(cols, rows);
                }
                return Reaction::Redraw;
            }

            // ── Scroll wheel ─────────────────────────────────────────────────
            UiEvent::Scroll {
                delta, position, ..
            } => {
                if let Some(ref mut sess) = self.session {
                    let vp = self.last_viewport;
                    let term_h = Self::term_height(vp);
                    let in_term = position.y >= 0.0 && position.y < term_h;

                    // Try to forward the wheel to the PTY first.
                    // `forward_mouse` returns `true` when mouse reporting is on
                    // or the child is on the alt-screen (e.g. tmux / vim / less).
                    if in_term && delta.y != 0.0 {
                        let kind = if delta.y > 0.0 {
                            TerminalMouseKind::WheelUp
                        } else {
                            TerminalMouseKind::WheelDown
                        };
                        let col = position.x.max(0.0) as u16;
                        let row = position.y.max(0.0) as u16;
                        if sess.forward_mouse(
                            kind,
                            MouseButton::Left,
                            col,
                            row,
                            Modifiers::default(),
                        ) {
                            return Reaction::Redraw;
                        }
                    }

                    // Fall back to local scrollback.
                    // Positive y = scroll up (into history).
                    // Negative y = scroll down (toward live).
                    if delta.y > 0.0 {
                        sess.scroll_up(3);
                    } else if delta.y < 0.0 {
                        sess.scroll_down(3);
                    }
                    return Reaction::Redraw;
                }
            }

            // ── Scrollbar mouse-down: start drag / forward click to PTY ──────
            UiEvent::MouseDown {
                button,
                position,
                modifiers,
                ..
            } => {
                let vp = self.last_viewport;
                let term_h = Self::term_height(vp);
                let sb_col = Self::scrollbar_col(vp);

                // Left-button click on the scrollbar column → start a drag.
                let in_scrollbar = button == MouseButton::Left
                    && position.x >= sb_col
                    && position.y >= 0.0
                    && position.y < term_h;

                if in_scrollbar {
                    if let Some(ref sess) = self.session {
                        let total = sess.history_len() + sess.rows() as usize;
                        let visible = sess.rows() as usize;
                        if total > visible {
                            // Start drag only when the scrollbar is visible.
                            self.scrollbar_drag = Some(ScrollbarDrag {
                                track_y: 0.0,
                                track_h: term_h,
                                total,
                                visible,
                            });
                            self.apply_scrollbar_drag(position.y);
                            return Reaction::Redraw;
                        }
                    }
                }

                // Any click inside the terminal content area is forwarded to
                // the PTY when mouse reporting is enabled.
                let in_term = position.y >= 0.0 && position.y < term_h;
                if in_term {
                    if let Some(ref mut sess) = self.session {
                        let col = position.x.max(0.0) as u16;
                        let row = position.y.max(0.0) as u16;
                        if sess.forward_mouse(TerminalMouseKind::Press, button, col, row, modifiers)
                        {
                            return Reaction::Redraw;
                        }
                    }
                }
            }

            // ── Scrollbar mouse-move: update drag ─────────────────────────────
            UiEvent::MouseMoved {
                position,
                buttons: ButtonMask { left: true, .. },
            } => {
                if self.scrollbar_drag.is_some() {
                    self.apply_scrollbar_drag(position.y);
                    return Reaction::Redraw;
                }
            }

            // ── Mouse-up: end drag / forward release to PTY ────────────────────
            UiEvent::MouseUp {
                button, position, ..
            } => {
                // End any active scrollbar drag first.
                if self.scrollbar_drag.take().is_some() {
                    return Reaction::Redraw;
                }
                // Forward button release to the PTY when mouse reporting is on.
                let vp = self.last_viewport;
                let term_h = Self::term_height(vp);
                let in_term = position.y >= 0.0 && position.y < term_h;
                if in_term {
                    if let Some(ref mut sess) = self.session {
                        let col = position.x.max(0.0) as u16;
                        let row = position.y.max(0.0) as u16;
                        if sess.forward_mouse(
                            TerminalMouseKind::Release,
                            button,
                            col,
                            row,
                            Modifiers::default(),
                        ) {
                            return Reaction::Redraw;
                        }
                    }
                }
            }

            // ── Bracketed paste ───────────────────────────────────────────────
            UiEvent::ClipboardPaste(text) => {
                if let Some(ref mut sess) = self.session {
                    if !sess.is_exited() {
                        // Wrap in bracketed-paste markers so the shell handles it
                        // correctly (avoids interpreting pasted newlines as commands).
                        let mut bytes = b"\x1b[200~".to_vec();
                        bytes.extend_from_slice(text.as_bytes());
                        bytes.extend_from_slice(b"\x1b[201~");
                        sess.write_input(&bytes);
                        sess.scroll_reset();
                    }
                }
                return Reaction::Redraw;
            }

            // ── Shift+PageUp: scroll back one page ────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::PageUp),
                modifiers,
                ..
            } if modifiers.shift => {
                if let Some(ref mut sess) = self.session {
                    let page = sess.rows() as usize;
                    sess.scroll_up(page);
                }
                return Reaction::Redraw;
            }

            // ── Shift+PageDown: scroll forward one page ───────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::PageDown),
                modifiers,
                ..
            } if modifiers.shift => {
                if let Some(ref mut sess) = self.session {
                    let page = sess.rows() as usize;
                    sess.scroll_down(page);
                }
                return Reaction::Redraw;
            }

            // ── Shift+Home: jump to oldest history ────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Home),
                modifiers,
                ..
            } if modifiers.shift => {
                if let Some(ref mut sess) = self.session {
                    // set_scroll_offset clamps to history_len() internally.
                    sess.set_scroll_offset(usize::MAX);
                }
                return Reaction::Redraw;
            }

            // ── Key input ─────────────────────────────────────────────────────
            UiEvent::KeyPressed { key, modifiers, .. } => {
                if let Some(ref mut sess) = self.session {
                    // Dead PTY — swallow all key input.
                    if sess.is_exited() {
                        return Reaction::Continue;
                    }
                    // Any key press returns to live view.
                    sess.scroll_reset();
                    if let Some(bytes) = key_to_pty_bytes(key, modifiers) {
                        sess.write_input(&bytes);
                    }
                }
                return Reaction::Redraw;
            }

            // ── Printable characters typed ────────────────────────────────────
            UiEvent::CharTyped(ch) => {
                if let Some(ref mut sess) = self.session {
                    // Dead PTY — swallow all character input.
                    if sess.is_exited() {
                        return Reaction::Continue;
                    }
                    sess.scroll_reset();
                    let mut buf = [0u8; 4];
                    let s = ch.encode_utf8(&mut buf);
                    sess.write_input(s.as_bytes());
                }
                return Reaction::Redraw;
            }

            _ => {}
        }

        // Suppress the unused-variable warning from the compiler on
        // backends that don't provide viewport inline with events.
        let _ = backend.viewport();
        Reaction::Continue
    }

    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        if let Some(ref mut sess) = self.session {
            if sess.poll() {
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }
}

// ── Key-to-PTY-bytes conversion ───────────────────────────────────────────────

/// Convert a [`Key`] + [`Modifiers`] to the byte sequence sent to the PTY.
///
/// Covers the common VT100 / xterm-256color escape sequences. Keys that
/// have no meaningful PTY encoding (e.g. CapsLock) return `None`.
fn key_to_pty_bytes(key: Key, mods: Modifiers) -> Option<Vec<u8>> {
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

        Key::Named(named) => named_key_bytes(named, mods),
    }
}

/// Map named keys to their VT100 escape sequences.
fn named_key_bytes(key: NamedKey, mods: Modifiers) -> Option<Vec<u8>> {
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
        NamedKey::Up => Some(xterm_cursor_seq(b"A", mod_param)),
        NamedKey::Down => Some(xterm_cursor_seq(b"B", mod_param)),
        NamedKey::Right => Some(xterm_cursor_seq(b"C", mod_param)),
        NamedKey::Left => Some(xterm_cursor_seq(b"D", mod_param)),
        NamedKey::Home => Some(xterm_seq(b"1", mod_param)),
        NamedKey::End => Some(xterm_seq(b"4", mod_param)),
        NamedKey::Insert => Some(xterm_seq(b"2", mod_param)),
        NamedKey::PageUp => Some(xterm_seq(b"5", mod_param)),
        NamedKey::PageDown => Some(xterm_seq(b"6", mod_param)),
        NamedKey::F(n) => f_key_bytes(n, mod_param),
        // Keys with no PTY mapping.
        NamedKey::CapsLock | NamedKey::NumLock | NamedKey::ScrollLock | NamedKey::Menu => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_is_etx() {
        let bytes = key_to_pty_bytes(
            Key::Char('c'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
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
        );
        assert_eq!(bytes, Some(vec![0x04])); // EOT
    }

    #[test]
    fn printable_char_passes_through() {
        let bytes = key_to_pty_bytes(Key::Char('a'), Modifiers::default()).unwrap();
        assert_eq!(bytes, b"a");
    }

    #[test]
    fn enter_is_cr() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::Enter), Modifiers::default()).unwrap();
        assert_eq!(bytes, b"\r");
    }

    #[test]
    fn up_arrow_plain() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::Up), Modifiers::default()).unwrap();
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
        )
        .unwrap();
        // ctrl mod_param = 5 → "\x1b[1;5A"
        assert_eq!(bytes, b"\x1b[1;5A");
    }

    #[test]
    fn f1_plain() {
        // F1 unmodified must emit the SS3 sequence \x1bOP, NOT the CSI \x1b[P.
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::F(1)), Modifiers::default()).unwrap();
        assert_eq!(bytes, b"\x1bOP");
    }

    #[test]
    fn f2_plain() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::F(2)), Modifiers::default()).unwrap();
        assert_eq!(bytes, b"\x1bOQ");
    }

    #[test]
    fn f3_plain() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::F(3)), Modifiers::default()).unwrap();
        assert_eq!(bytes, b"\x1bOR");
    }

    #[test]
    fn f4_plain() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::F(4)), Modifiers::default()).unwrap();
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
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[1;5P");
    }

    #[test]
    fn delete_plain() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::Delete), Modifiers::default()).unwrap();
        assert_eq!(bytes, b"\x1b[3~");
    }

    #[test]
    fn page_up_plain() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::PageUp), Modifiers::default()).unwrap();
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
        )
        .unwrap();
        // shift+ctrl mod_param = 6 → "\x1b[6;6~"  (PageDown code = "6")
        assert_eq!(bytes, b"\x1b[6;6~");
    }

    #[test]
    fn caps_lock_is_none() {
        let bytes = key_to_pty_bytes(Key::Named(NamedKey::CapsLock), Modifiers::default());
        assert!(bytes.is_none());
    }

    // ── Scrollbar drag math ───────────────────────────────────────────────────

    /// Helper that mimics the offset computation in `apply_scrollbar_drag`.
    fn drag_to_offset(y: f32, track_y: f32, track_h: f32, total: usize, visible: usize) -> usize {
        if track_h <= 0.0 {
            return 0;
        }
        let max_scroll = total.saturating_sub(visible);
        let fraction = ((y - track_y) / track_h).clamp(0.0, 1.0);
        ((1.0 - fraction) * max_scroll as f32).round() as usize
    }

    #[test]
    fn drag_at_bottom_gives_live_view() {
        // When the thumb is at the bottom of the track (fraction = 1),
        // the offset should be 0 (live view) for an inverted scrollbar.
        let offset = drag_to_offset(20.0, 0.0, 20.0, 50, 20);
        assert_eq!(offset, 0, "bottom of track → live view (offset 0)");
    }

    #[test]
    fn drag_at_top_gives_max_offset() {
        // When the thumb is at the top (fraction = 0), offset = total - visible.
        let offset = drag_to_offset(0.0, 0.0, 20.0, 50, 20);
        assert_eq!(offset, 30, "top of track → max scroll offset");
    }

    #[test]
    fn drag_at_midpoint_gives_half_offset() {
        // Middle of track → half of max_scroll.
        let offset = drag_to_offset(10.0, 0.0, 20.0, 50, 20);
        // fraction = 0.5 → (1 - 0.5) * 30 = 15
        assert_eq!(offset, 15);
    }

    #[test]
    fn drag_clamped_below_zero() {
        // Y above track top should clamp to max offset.
        let offset = drag_to_offset(-5.0, 0.0, 20.0, 50, 20);
        assert_eq!(offset, 30);
    }

    #[test]
    fn drag_clamped_above_track_height() {
        // Y below track bottom should clamp to offset 0 (live view).
        let offset = drag_to_offset(100.0, 0.0, 20.0, 50, 20);
        assert_eq!(offset, 0);
    }

    // ── viewport_to_pty reserves footer row ───────────────────────────────────

    #[test]
    fn viewport_to_pty_reserves_footer() {
        let vp = Viewport::new(80.0, 24.0, 1.0);
        let (cols, rows) = TerminalApp::viewport_to_pty(vp);
        assert_eq!(cols, 80);
        // height 24 minus FOOTER_ROWS 1 = 23
        assert_eq!(rows, 23);
    }

    #[test]
    fn viewport_to_pty_minimum_rows() {
        // Very small terminal — rows must not go below 3.
        let vp = Viewport::new(80.0, 3.0, 1.0);
        let (_cols, rows) = TerminalApp::viewport_to_pty(vp);
        assert_eq!(rows, 3); // max(3.0 - 1.0 = 2.0, 3.0) = 3
    }
}
