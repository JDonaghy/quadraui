//! `TextInput` demo — adapted from
//! `quadraui/examples/common/text_input_demo.rs`.
//!
//! Two independent `TextEditor` buffers (one per variant) exercise
//! cursor movement, line wrap on Enter, Shift+arrow selection and
//! click-to-position via `TextInputLayout::hit_test` — every edit goes
//! through `TextEditor::apply(EditOp)` (quadraui#833), never hand-rolled
//! insert/backspace/cursor logic.

use quadraui::{
    Backend, BackendCaps, Color, EditOp, InteractionState, MouseButton, Reaction, Rect, StatusBar,
    StatusBarSegment, TextEditor, TextInput, TextInputHit, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("text_input.rs");

// gallery:begin
pub struct TextInputDemo {
    /// `inputs[0]` is the "Empty" variant's buffer, `inputs[1]` is the
    /// "Prefilled" variant's — kept independent so typing in one never
    /// leaks into the other when the gallery's variant picker switches.
    inputs: Vec<TextEditor>,
}

impl TextInputDemo {
    pub fn new() -> Self {
        let mut empty = TextInput::new(WidgetId::new("gallery:text-input:empty"));
        empty.placeholder = Some(
            "Type something. Enter for newline, arrows/Home/End to move, Shift to select.".into(),
        );
        empty.has_focus = true;

        let mut prefilled = TextInput::new(WidgetId::new("gallery:text-input:prefilled"))
            .with_lines(vec![
                "fix: handle empty selection range".to_string(),
                String::new(),
                "Click to move the cursor, or select and retype.".to_string(),
            ]);
        prefilled.has_focus = true;

        Self {
            inputs: vec![TextEditor::new(empty), TextEditor::new(prefilled)],
        }
    }

    fn editor(&self, variant: usize) -> &TextEditor {
        &self.inputs[variant.min(self.inputs.len() - 1)]
    }

    fn status(&self, variant: usize) -> StatusBar {
        let editor = self.editor(variant);
        let cursor = format!(
            " line {} col {} ",
            editor.cursor_line + 1,
            editor.cursor_col + 1,
        );
        StatusBar {
            id: WidgetId::new("gallery:text-input:status"),
            left_segments: vec![StatusBarSegment {
                text: " Type to edit — Shift+arrows to select ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: cursor,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn input_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }
}

impl Default for TextInputDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for TextInputDemo {
    fn name(&self) -> &'static str {
        "Text Input"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Empty", "Prefilled"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let input_rect = Self::input_rect(area, backend);
        backend.draw_text_input(input_rect, self.editor(variant));

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(variant),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let idx = variant.min(self.inputs.len() - 1);
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = Self::input_rect(area, backend);
                let layout = backend.text_input_layout(rect, &self.inputs[idx]);
                let op = match layout.hit_test(position.x, position.y) {
                    TextInputHit::Line { line_idx } => EditOp::SetCursor {
                        line: line_idx,
                        col: self.inputs[idx].cursor_col,
                        extend: false,
                    },
                    TextInputHit::EmptyArea => {
                        let last = self.inputs[idx].lines.len().saturating_sub(1);
                        EditOp::SetCursor {
                            line: last,
                            col: usize::MAX,
                            extend: false,
                        }
                    }
                };
                if self.inputs[idx].apply(op) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            UiEvent::KeyPressed { key, modifiers, .. } => {
                if matches!(key, quadraui::Key::Char(_)) && (modifiers.ctrl || modifiers.cmd) {
                    return Reaction::Continue;
                }
                match EditOp::from_key(key, *modifiers) {
                    Some(op) => {
                        if self.inputs[idx].apply(op) {
                            Reaction::Redraw
                        } else {
                            Reaction::Continue
                        }
                    }
                    None => Reaction::Continue,
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::to_value(&self.editor(variant).input).unwrap_or(serde_json::Value::Null)
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
    fn empty_variant_starts_with_a_placeholder_and_no_text() {
        let demo = TextInputDemo::new();
        assert_eq!(demo.inputs[0].lines, vec![String::new()]);
        assert!(demo.inputs[0].placeholder.is_some());
    }

    #[test]
    fn prefilled_variant_starts_with_sample_lines() {
        let demo = TextInputDemo::new();
        assert!(demo.inputs[1].lines[0].contains("empty selection range"));
    }
}

// `TuiBackend` only exists under `feature = "tui"`, and this crate has
// no default features: the `macos` and `win` gates compile these test
// targets with `tui` off, so the module itself must carry the feature.
#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn typing_a_character_inserts_it_at_the_cursor() {
        let mut demo = TextInputDemo::new();
        let event = UiEvent::KeyPressed {
            key: quadraui::Key::Char('x'),
            modifiers: Default::default(),
            repeat: false,
        };
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 10.0);
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.inputs[0].lines[0], "x");
    }
}
