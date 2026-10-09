//! `CommandLine` demo — adapted from
//! `quadraui/examples/common/command_line_selection_demo.rs`.
//!
//! Two variants show the vim-style `:`/`/` command-line bar in its two
//! paint modes: plain text (`Backend::draw_command_line`) and with a
//! selection highlight behind part of the text
//! (`Backend::draw_command_line_selection`).

use quadraui::{Backend, BackendCaps, CommandLine, Reaction, Rect, UiEvent, WidgetId};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("command_line.rs");

// gallery:begin
pub struct CommandLineDemo {
    text: String,
    /// Byte-offset `(start, end)` of the word the "Selection" variant
    /// highlights — computed once from `text` so it can't drift out of
    /// range if the text above ever changes.
    word_range: (usize, usize),
}

impl CommandLineDemo {
    pub fn new() -> Self {
        let text = ":select-me".to_string();
        let word_start = text.find("select-me").expect("literal is present");
        let word_range = (word_start, text.len());
        Self { text, word_range }
    }

    fn command_line(&self) -> CommandLine {
        CommandLine {
            id: WidgetId::new("gallery:command-line"),
            text: self.text.clone(),
            cursor_offset: Some(self.text.len()),
            right_align: false,
        }
    }
}

impl Default for CommandLineDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for CommandLineDemo {
    fn name(&self) -> &'static str {
        "Command Line"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Plain", "Selection highlight"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let rect = Rect::new(area.x, area.y, area.width, lh);
        let cmd = self.command_line();
        if variant == 1 {
            backend.draw_command_line_selection(rect, &cmd, Some(self.word_range));
        } else {
            backend.draw_command_line(rect, &cmd);
        }
    }

    fn handle(
        &mut self,
        _variant: usize,
        _event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        Reaction::Continue
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "command_line": self.command_line(),
            "selection": if variant == 1 {
                Some(self.word_range)
            } else {
                None
            },
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_range_covers_the_literal_select_me() {
        let demo = CommandLineDemo::new();
        let (start, end) = demo.word_range;
        assert_eq!(&demo.text[start..end], "select-me");
    }

    #[test]
    fn variants_lists_plain_and_selection() {
        let demo = CommandLineDemo::new();
        assert_eq!(demo.variants(), &["Plain", "Selection highlight"]);
    }
}
