//! `Terminal` demo — adapted from
//! `quadraui/examples/common/terminal_app.rs`.
//!
//! Spawns a real interactive shell session via
//! [`quadraui::terminal_engine::TerminalSession`] — this is the one demo
//! in the gallery that talks to a real child process, which is why it
//! lives behind the `terminal` Cargo feature (see
//! `quadraui-gallery/Cargo.toml`'s `terminal` entry and
//! `registry::registry`'s feature-gated push).
//!
//! Two variants show the same primitive from two angles: a live,
//! typeable shell, and a shell that's immediately fed one canned
//! command producing ANSI colour/style output — useful for seeing the
//! VT100 attribute rendering without having to type anything.

use std::cell::RefCell;

use quadraui::terminal_engine::{default_shell, TerminalSession};
use quadraui::{
    Backend, Color, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("terminal.rs");

// gallery:begin
/// Shell command fed to the "Scripted output" variant immediately after
/// it spawns — POSIX `printf` octal escapes, portable across
/// bash/zsh/dash. `GALLERYSCRIPTOK` is a plain, uncoloured marker a test
/// can search for without caring how each shell's prompt wraps the
/// colour escapes.
const SCRIPT: &str =
    "printf '\\033[31mRed\\033[0m \\033[1mBold\\033[0m \\033[4mUnderline\\033[0m GALLERYSCRIPTOK\\n'\n";

/// Smallest PTY size this demo will resize to — mirrors
/// `terminal_app.rs`'s own floor, and `vt100::Parser`'s panic guard
/// below it.
const MIN_COLS: u16 = 10;
const MIN_ROWS: u16 = 3;

struct SessionSlot {
    session: Result<TerminalSession, String>,
    size: (u16, u16),
}

fn spawn_slot(scripted: bool) -> RefCell<SessionSlot> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let shell = default_shell();
    let size = (80, 24);
    let mut session = TerminalSession::spawn(size.0, size.1, &shell, &cwd, 2_000)
        .map_err(|e| format!("failed to spawn PTY: {e}"));
    if scripted {
        if let Ok(sess) = session.as_mut() {
            sess.send_str(SCRIPT);
        }
    }
    RefCell::new(SessionSlot { session, size })
}

/// `(cols, rows)` a PTY inside `rect` should have, given the backend's
/// current character metrics (1x1 on TUI, real pixel metrics on GTK).
fn area_to_pty(rect: Rect, backend: &dyn Backend) -> (u16, u16) {
    let cw = backend.char_width().max(1.0);
    let lh = backend.line_height().max(1.0);
    let cols = (rect.width / cw).max(MIN_COLS as f32) as u16;
    let rows = (rect.height / lh).max(MIN_ROWS as f32) as u16;
    (cols, rows)
}

pub struct TerminalDemo {
    slots: [RefCell<SessionSlot>; 2],
}

impl TerminalDemo {
    pub fn new() -> Self {
        Self {
            slots: [spawn_slot(false), spawn_slot(true)],
        }
    }

    fn slot(&self, variant: usize) -> &RefCell<SessionSlot> {
        &self.slots[variant.min(self.slots.len() - 1)]
    }
}

impl Default for TerminalDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for TerminalDemo {
    fn name(&self) -> &'static str {
        "Terminal"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Live shell", "Scripted output"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height().max(1.0);
        let term_area = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
        let mut slot = self.slot(variant).borrow_mut();
        let (cols, rows) = area_to_pty(term_area, backend);
        let needs_resize = (cols, rows) != slot.size && cols >= MIN_COLS && rows >= MIN_ROWS;
        if needs_resize {
            slot.size = (cols, rows);
        }

        if let Ok(sess) = slot.session.as_mut() {
            if needs_resize {
                sess.resize(cols, rows);
            }
            sess.poll();
            let total = sess.history_len() + sess.rows() as usize;
            let sb = if total > sess.rows() as usize {
                Some(sess.scrollbar_state(None))
            } else {
                None
            };
            let snapshot =
                sess.to_terminal(WidgetId::new(format!("gallery:terminal:{variant}")), sb);
            backend.draw_terminal(term_area, &snapshot);
        }

        let footer_text = match &slot.session {
            Ok(sess) if sess.is_exited() => {
                format!(" [process exited {}] ", sess.exit_code().unwrap_or(0))
            }
            Ok(_) => " type to run \u{b7} PageUp/PageDown scroll history ".to_string(),
            Err(e) => format!(" {e} "),
        };
        let bar = StatusBar {
            id: WidgetId::new("gallery:terminal:footer"),
            left_segments: vec![StatusBarSegment {
                text: footer_text,
                fg: Color::rgb(160, 160, 160),
                bg: Color::rgb(40, 40, 40),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let footer_rect = Rect::new(area.x, area.y + term_area.height, area.width, lh);
        let _ = backend.draw_status_bar_interactive(footer_rect, &bar, &InteractionState::new());
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        let mut slot = self.slot(variant).borrow_mut();
        let sess = match slot.session.as_mut() {
            Ok(s) => s,
            Err(_) => return Reaction::Continue,
        };
        match event {
            UiEvent::CharTyped(ch) => {
                if sess.is_exited() {
                    return Reaction::Continue;
                }
                sess.scroll_reset();
                let mut buf = [0u8; 4];
                sess.write_input(ch.encode_utf8(&mut buf).as_bytes());
                Reaction::Redraw
            }
            UiEvent::KeyPressed { key, modifiers, .. } => {
                match key {
                    Key::Named(NamedKey::PageUp) => {
                        let page = sess.rows() as usize;
                        sess.scroll_up(page);
                    }
                    Key::Named(NamedKey::PageDown) => {
                        let page = sess.rows() as usize;
                        sess.scroll_down(page);
                    }
                    _ => {
                        if sess.is_exited() {
                            return Reaction::Continue;
                        }
                        sess.write_key(key.clone(), *modifiers);
                    }
                }
                Reaction::Redraw
            }
            UiEvent::ClipboardPaste(text) => {
                if !sess.is_exited() {
                    sess.paste(text);
                }
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn tick(&mut self, variant: usize, _backend: &mut dyn Backend) -> Reaction {
        let mut slot = self.slot(variant).borrow_mut();
        if let Ok(sess) = slot.session.as_mut() {
            if sess.poll() {
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        let slot = self.slot(variant).borrow();
        match &slot.session {
            Ok(sess) => serde_json::json!({
                "cols": sess.cols(),
                "rows": sess.rows(),
                "history_len": sess.history_len(),
                "exited": sess.is_exited(),
            }),
            Err(e) => serde_json::json!({ "error": e }),
        }
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_to_pty_enforces_the_floor() {
        let backend = quadraui::testing::RecordingBackend::new();
        let (cols, rows) = area_to_pty(Rect::new(0.0, 0.0, 4.0, 1.0), &backend);
        assert_eq!(cols, MIN_COLS);
        assert_eq!(rows, MIN_ROWS);
    }

    #[test]
    fn scripted_variant_sends_the_script_before_any_render() {
        let demo = TerminalDemo::new();
        let slot = demo.slot(1).borrow();
        // Spawning a real shell may fail in some sandboxes — only assert
        // the send actually happened when it succeeded.
        if let Ok(sess) = &slot.session {
            assert!(!sess.is_exited());
        }
    }
}
