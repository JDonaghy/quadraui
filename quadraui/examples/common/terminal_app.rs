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
//! - `ClipboardPaste` is routed through [`TerminalSession::paste`], which
//!   bracketed-paste-wraps the text when the child has enabled that mode
//!   and sends it raw otherwise. On GTK this fires for Ctrl-V,
//!   Ctrl-Shift-V, and middle-click (PRIMARY selection) — all three route
//!   to the same `ClipboardPaste` event (quadraui#415).
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
    ///
    /// `char_w`/`line_h` are `Backend::char_width()` /
    /// `Backend::line_height()` — `1.0` on TUI (viewport units already
    /// are cells, so this is a no-op division) and real Pango-resolved
    /// pixel metrics on GTK, where `Viewport` is in pixels, not cells
    /// (quadraui#437 — this used to treat GTK's pixel viewport as a
    /// cell count directly, spawning wildly wrong PTY sizes).
    fn viewport_to_pty(vp: Viewport, char_w: f32, line_h: f32) -> (u16, u16) {
        let char_w = char_w.max(1.0);
        let line_h = line_h.max(1.0);
        let cols = (vp.width / char_w).max(10.0) as u16;
        let rows = (Self::term_height(vp, line_h) / line_h).max(3.0) as u16;
        (cols, rows)
    }

    /// Height of the terminal grid area (viewport height minus footer),
    /// in the same native units as `vp` (cells on TUI, pixels on GTK).
    fn term_height(vp: Viewport, line_h: f32) -> f32 {
        let line_h = line_h.max(1.0);
        (vp.height - FOOTER_ROWS as f32 * line_h).max(0.0)
    }

    /// X position of the scrollbar (rightmost character column of the
    /// viewport), in the same native units as `vp`.
    fn scrollbar_col(vp: Viewport, char_w: f32) -> f32 {
        let char_w = char_w.max(1.0);
        (vp.width - char_w).max(0.0)
    }

    // ── Footer rendering ──────────────────────────────────────────────────────

    /// Render the one-row hint / status footer at the very bottom of the viewport.
    ///
    /// - Normal operation: shows keyboard shortcuts.
    /// - Process exited: shows the exit code and prompts the user to quit.
    fn render_footer(&self, backend: &mut dyn Backend, vp: Viewport) {
        let footer_h = FOOTER_ROWS as f32 * backend.line_height().max(1.0);
        let y = vp.height - footer_h;
        let footer_rect = Rect::new(0.0, y, vp.width, footer_h);

        let (text, fg, bg) = if let Some(ref sess) = self.session {
            if sess.is_exited() {
                let code = sess.exit_code().unwrap_or(0);
                (
                    format!(" [process exited {code}] — press Ctrl+Q to close"),
                    Color::rgb(255, 200, 100),
                    Color::rgb(60, 30, 0),
                )
            } else {
                // Show [APP KEYS] indicator when DECCKM is active so users can
                // see that arrow keys are being encoded in application-cursor
                // mode (ESC O A…D rather than ESC [ A…D) — quadraui #336.
                let app_indicator = if sess.application_cursor_keys() {
                    "  · [APP KEYS]"
                } else {
                    ""
                };
                (
                    format!(
                        " Ctrl+Q quit  ·  wheel / Shift+PgUp scroll  ·  type to run{app_indicator}"
                    ),
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
        let (cols, rows) = Self::viewport_to_pty(vp, backend.char_width(), backend.line_height());
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        let shell = default_shell();
        match TerminalSession::spawn(cols, rows, &shell, &cwd, 10_000) {
            Ok(sess) => self.session = Some(sess),
            Err(e) => self.error_msg = format!("Failed to spawn PTY: {e}"),
        }
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let term_h = Self::term_height(vp, backend.line_height());
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
            let line_h = backend.line_height().max(1.0);
            let bar_rect = Rect::new(0.0, (rect.height - line_h).max(0.0), rect.width, line_h);
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
                    let (cols, rows) = Self::viewport_to_pty(
                        viewport,
                        backend.char_width(),
                        backend.line_height(),
                    );
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
                    let term_h = Self::term_height(vp, backend.line_height());
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
                let term_h = Self::term_height(vp, backend.line_height());
                let sb_col = Self::scrollbar_col(vp, backend.char_width());

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
                let term_h = Self::term_height(vp, backend.line_height());
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

            // ── Paste ────────────────────────────────────────────────────────
            UiEvent::ClipboardPaste(text) => {
                if let Some(ref mut sess) = self.session {
                    if !sess.is_exited() {
                        // `TerminalSession::paste` centralizes the
                        // bracketed-paste wrap (only applied when the
                        // child has enabled bracketed-paste mode) so this
                        // example doesn't hand-roll it — quadraui#415.
                        sess.paste(&text);
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
                    // Query DECCKM so arrow/Home/End get the right encoding.
                    let app_cursor = sess.application_cursor_keys();
                    if let Some(bytes) = key_to_pty_bytes(key, modifiers, app_cursor) {
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
/// `app_cursor` should be `true` when the child has enabled DECCKM
/// (application-cursor-keys mode, `ESC [ ? 1 h`). In that mode, unmodified
/// arrow keys and Home/End are encoded as SS3 sequences (`ESC O A…D/H/F`)
/// rather than the normal CSI sequences. Obtain the flag from
/// [`TerminalSession::application_cursor_keys`].
///
/// Covers the common VT100 / xterm-256color escape sequences. Keys that
/// have no meaningful PTY encoding (e.g. CapsLock) return `None`.
pub(crate) fn key_to_pty_bytes(key: Key, mods: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    //
    // char_w = line_h = 1.0 exercises the TUI path (viewport units
    // already are cells, so the conversion is a no-op) — mirrors what
    // `TuiBackend::char_width()`/`line_height()` actually return.

    #[test]
    fn viewport_to_pty_reserves_footer() {
        let vp = Viewport::new(80.0, 24.0, 1.0);
        let (cols, rows) = TerminalApp::viewport_to_pty(vp, 1.0, 1.0);
        assert_eq!(cols, 80);
        // height 24 minus FOOTER_ROWS 1 = 23
        assert_eq!(rows, 23);
    }

    #[test]
    fn viewport_to_pty_minimum_rows() {
        // Very small terminal — rows must not go below 3.
        let vp = Viewport::new(80.0, 3.0, 1.0);
        let (_cols, rows) = TerminalApp::viewport_to_pty(vp, 1.0, 1.0);
        assert_eq!(rows, 3); // max(3.0 - 1.0 = 2.0, 3.0) = 3
    }

    // ── viewport_to_pty converts pixels → cells (GTK path) ────────────────────
    //
    // quadraui#437: GTK's `Viewport` is in pixels, not cells. These pin
    // the conversion so a future edit can't silently regress back to
    // treating pixel dimensions as cell counts.

    #[test]
    fn viewport_to_pty_converts_gtk_pixels_to_cells() {
        // 800×600px window, 8px chars, 16px lines (GtkBackend's defaults
        // before the first real Pango measurement) → 100 cols; 600/16 =
        // 37.5 total rows minus the 1-row footer = 36.5, truncated to 36.
        let vp = Viewport::new(800.0, 600.0, 1.0);
        let (cols, rows) = TerminalApp::viewport_to_pty(vp, 8.0, 16.0);
        assert_eq!(cols, 100);
        assert_eq!(rows, 36);
    }

    #[test]
    fn viewport_to_pty_gtk_minimum_rows() {
        // A tiny GTK window still clamps to the same 10-col/3-row floor
        // as TUI, once converted through the char metrics.
        let vp = Viewport::new(40.0, 20.0, 1.0);
        let (cols, rows) = TerminalApp::viewport_to_pty(vp, 8.0, 16.0);
        assert_eq!(cols, 10); // (40/8=5) clamped up to the 10-col floor
        assert_eq!(rows, 3); // (20/16=1.25 → term_height 4px/16=0) clamped up to 3
    }

    // ── term_height / scrollbar_col unit conversion ────────────────────────────

    #[test]
    fn term_height_tui_units_unchanged() {
        // line_h = 1.0 (TUI): behaves exactly like the old hardcoded
        // `vp.height - FOOTER_ROWS` formula.
        let vp = Viewport::new(80.0, 24.0, 1.0);
        assert_eq!(TerminalApp::term_height(vp, 1.0), 23.0);
    }

    #[test]
    fn term_height_gtk_subtracts_pixel_footer() {
        // line_h = 16px (GTK): footer is FOOTER_ROWS * line_h pixels,
        // not a flat 1px sliver.
        let vp = Viewport::new(800.0, 600.0, 1.0);
        assert_eq!(TerminalApp::term_height(vp, 16.0), 584.0);
    }

    #[test]
    fn scrollbar_col_gtk_uses_char_width() {
        let vp = Viewport::new(800.0, 600.0, 1.0);
        assert_eq!(TerminalApp::scrollbar_col(vp, 8.0), 792.0);
    }
}
