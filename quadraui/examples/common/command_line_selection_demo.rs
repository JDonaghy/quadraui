//! `CommandLine` selection-highlight demo (issue #1001).
//!
//! Exercises [`Backend::draw_command_line_selection`] — the sibling of
//! [`Backend::draw_command_line`] that paints
//! [`quadraui::primitives::command_line::CommandLineLayout::selection_bounds`]'s
//! rect as a highlight. Before #1001, that geometry existed but nothing
//! painted it (GTK had no visual selection feedback at all; TUI faked one
//! by hand-inverting cells outside quadraui). Pressing `s` toggles a fixed
//! selection range on the command text so the highlight is visibly there
//! on both backends — same `AppLogic`, same call, same highlight.
//!
//! ## Controls
//!
//! | Input | Action                              |
//! |-------|--------------------------------------|
//! | `s`   | Toggle the `:select-me` selection    |
//! | `q`/Esc | Quit                                |

use quadraui::{AppLogic, Backend, Color, CommandLine, Reaction, Rect, UiEvent, WidgetId};

pub struct CommandLineSelectionDemo {
    text: String,
    /// Byte-offset `(start, end)` of the word this demo highlights when
    /// `selected` is `true` — computed once from `text` so it can't drift
    /// out of range if the text above ever changes.
    word_range: (usize, usize),
    selected: bool,
}

impl CommandLineSelectionDemo {
    pub fn new() -> Self {
        let text = ":select-me".to_string();
        let word_start = text.find("select-me").expect("literal is present");
        let word_range = (word_start, text.len());
        Self {
            text,
            word_range,
            selected: false,
        }
    }
}

impl Default for CommandLineSelectionDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for CommandLineSelectionDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let line_height = backend.line_height();

        backend.draw_solid_fill(
            Rect::new(0.0, 0.0, viewport.width, viewport.height),
            Color::rgb(20, 22, 30),
        );

        let hint_rect = Rect::new(4.0, 4.0, viewport.width - 8.0, line_height);
        let hint = CommandLine {
            id: WidgetId::new("cmdline:hint"),
            text: if self.selected {
                "selection ON  (s: toggle, q: quit)".to_string()
            } else {
                "selection OFF (s: toggle, q: quit)".to_string()
            },
            cursor_offset: None,
            right_align: false,
        };
        backend.draw_command_line(hint_rect, &hint);

        let cmd_rect = Rect::new(
            0.0,
            viewport.height - line_height,
            viewport.width,
            line_height,
        );
        let cmd = CommandLine {
            id: WidgetId::new("cmdline:demo"),
            text: self.text.clone(),
            cursor_offset: None,
            right_align: false,
        };
        let selection = self.selected.then_some(self.word_range);
        backend.draw_command_line_selection(cmd_rect, &cmd, selection);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: quadraui::Key::Named(quadraui::NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('s'),
                ..
            } => {
                self.selected = !self.selected;
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
