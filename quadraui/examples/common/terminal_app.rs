//! Backend-agnostic `AppLogic` for the terminal engine example
//! ([`tui_terminal`]).
//!
//! [`TerminalApp`] spawns a single interactive shell session via
//! [`TerminalSession`] and drives it through the standard
//! `AppLogic::render` / `AppLogic::handle` / `AppLogic::tick` pattern.
//!
//! Controls:
//! - All printable characters are forwarded to the PTY.
//! - Arrow keys, Tab, Backspace, Enter, Delete, Home, End, PgUp/PgDn are
//!   translated to the appropriate VT100 escape sequences by
//!   [`TerminalSession::write_key`] (quadraui#342) — this example no longer
//!   hand-rolls the escape-sequence encoding.
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
//! - **IME / dead-key composed input is out of scope here.** quadraui#415
//!   ("route clipboard paste + IME/dead-key composition into the focused
//!   terminal PTY") covers only the clipboard-paste half in this example
//!   and in `gtk::run::dispatch_event`. No backend runs an IME composition
//!   pipeline yet — see [`quadraui::UiEvent::CharTyped`]'s doc comment and
//!   epic quadraui#481, which owns wiring a real `GtkIMContext` (commit +
//!   preedit) end to end. Composed characters (e.g. a dead-key `´` + `e` →
//!   `é`) are not yet forwarded to the PTY by this example on any backend.
//! - When the shell exits a status line shows the exit code; Ctrl+Q closes.

use quadraui::terminal_engine::{default_shell, TerminalMouseKind, TerminalSession};
use quadraui::{
    AppLogic, Backend, ButtonMask, Color, InteractionState, Key, Modifiers, MouseButton, NamedKey,
    Reaction, Rect, ScrollDelta, StatusBar, StatusBarSegment, UiEvent, Viewport, WidgetId,
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
        backend.draw_status_bar_interactive(footer_rect, &bar, &InteractionState::new());
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
            backend.draw_status_bar_interactive(bar_rect, &bar, &InteractionState::new());
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

            // GTK only (quadraui#501): the OS "×" / Alt-F4 / window-manager
            // close dispatches this before the window is torn down, and an
            // app that returns anything other than `Exit` implicitly vetoes
            // it (`gtk::run::activate`'s `connect_close_request`). This demo
            // has no unsaved-state veto reason, so it just lets the close
            // proceed — same as pressing Ctrl-Q above. This is also the
            // default `gtk_smoke.sh` target, but the smoke harness itself
            // no longer depends on this arm: `schedule_smoke_check` forces
            // the window shut with `window.destroy()`, which bypasses the
            // close-request veto entirely.
            UiEvent::WindowClose => return Reaction::Exit,

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

                    // Delegate the forward-vs-scrollback policy to
                    // `TerminalSession::handle_wheel` (quadraui#365): it
                    // tries to forward the wheel to the PTY child first
                    // (mouse reporting / alt-screen, e.g. tmux / vim /
                    // less), and falls back to local scrollback otherwise.
                    // Positive delta.y = scroll up (into history), negative
                    // = scroll down (toward live) — 3 rows per notch. The
                    // real pointer cell is forwarded so alt-screen
                    // consumers like tmux route the wheel to the pane
                    // under the cursor, same as the MouseDown/MouseUp arms
                    // below.
                    if in_term && delta.y != 0.0 {
                        let up = delta.y > 0.0;
                        let col = position.x.max(0.0) as u16;
                        let row = position.y.max(0.0) as u16;
                        sess.handle_wheel(up, 3, col, row);
                        return Reaction::Redraw;
                    }
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
                    // `TerminalSession::write_key` centralizes the
                    // VT100/xterm key encoding (arrows, F-keys, modifiers,
                    // Ctrl-combos, DECCKM) so this example doesn't hand-roll
                    // it — quadraui#342. It also resets the scroll offset
                    // to the live view, same as ordinary character input.
                    sess.write_key(key, modifiers);
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

    fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
        let mut redraw = false;
        if let Some(ref mut sess) = self.session {
            if sess.poll() {
                redraw = true;
            }
            // OSC 0/2 window title (quadraui#339): the child program (vim,
            // tmux, a shell prompt hook, …) may have retitled the window.
            // `take_title_changed` is a one-shot dirty flag so this only
            // calls into the backend when the title actually changed, not
            // every tick. Routed through `WindowControl::set_title` — the
            // same backend-agnostic surface `window_control_demo.rs` uses
            // — so the GTK runner retitles its real window while TUI's own
            // `set_title` impl re-emits OSC 0/2 for the host terminal/tmux
            // pane, with no GTK-specific code in this engine or example.
            if sess.take_title_changed() {
                if let Some(title) = sess.title() {
                    let title = title.to_string();
                    if let Some(window) = backend.window() {
                        let _ = window.set_title(&title);
                    }
                }
            }
        }
        if redraw {
            Reaction::Redraw
        } else {
            Reaction::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
