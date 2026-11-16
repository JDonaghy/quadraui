//! `TextField` demo — the standalone single-line framed field, plain and
//! as a Finder-style search box (leading magnifier icon + clear button).
//!
//! Every edit goes through `TextField::apply_edit_op(EditOp)`, never
//! hand-rolled insert/backspace/cursor logic — the same discipline
//! `TextInputDemo` follows for the multi-line primitive.

use quadraui::{
    Backend, BackendCaps, EditOp, Icon, Reaction, Rect, TextField, TextFieldHit, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("text_field.rs");

// gallery:begin
pub struct TextFieldDemo {
    /// `fields[0]` is the "Plain" variant, `fields[1]` is the "Search"
    /// variant (leading magnifier icon + clear button) — independent so
    /// typing in one never leaks into the other when the gallery's
    /// variant picker switches.
    fields: Vec<TextField>,
}

impl TextFieldDemo {
    pub fn new() -> Self {
        let plain = TextField::new(WidgetId::new("gallery:text-field:plain"))
            .with_placeholder("Type something…")
            .with_has_focus(true);

        let search = TextField::new(WidgetId::new("gallery:text-field:search"))
            .with_placeholder("Search")
            .with_leading_icon(
                Icon::codicon("search", "?").unwrap_or_else(|| Icon::new("\u{1F50D}", "?")),
            )
            .with_clearable(true)
            .with_value("quadraui")
            .with_cursor(Some("quadraui".len()))
            .with_has_focus(true);

        Self {
            fields: vec![plain, search],
        }
    }

    fn field(&self, variant: usize) -> &TextField {
        &self.fields[variant.min(self.fields.len() - 1)]
    }

    fn field_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let h = backend.line_height().max(1.0);
        Rect::new(
            area.x,
            area.y,
            area.width.min(40.0 * backend.char_width()),
            h,
        )
    }
}

impl Default for TextFieldDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for TextFieldDemo {
    fn name(&self) -> &'static str {
        "Text Field"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Plain", "Search"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::field_rect(area, backend);
        backend.draw_text_field(rect, self.field(variant));
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let idx = variant.min(self.fields.len() - 1);
        match event {
            UiEvent::MouseDown { position, .. } => {
                let rect = Self::field_rect(area, backend);
                let layout = backend.text_field_layout(rect, &self.fields[idx]);
                match layout.hit_test(position.x, position.y) {
                    TextFieldHit::Clear => {
                        self.fields[idx].clear();
                        Reaction::Redraw
                    }
                    TextFieldHit::Text => {
                        self.fields[idx].has_focus = true;
                        Reaction::Redraw
                    }
                    TextFieldHit::Miss => Reaction::Continue,
                }
            }
            UiEvent::KeyPressed { key, modifiers, .. } => {
                if matches!(key, quadraui::Key::Char(_)) && (modifiers.ctrl || modifiers.cmd) {
                    return Reaction::Continue;
                }
                match EditOp::from_key(key, *modifiers) {
                    Some(op) => {
                        if self.fields[idx].apply_edit_op(op) {
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
        serde_json::to_value(self.field(variant)).unwrap_or(serde_json::Value::Null)
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
    fn plain_variant_starts_empty_with_a_placeholder() {
        let demo = TextFieldDemo::new();
        assert_eq!(demo.fields[0].value, "");
        assert!(!demo.fields[0].placeholder.is_empty());
        assert!(!demo.fields[0].clearable);
    }

    #[test]
    fn search_variant_has_icon_clearable_and_a_prefilled_value() {
        let demo = TextFieldDemo::new();
        assert!(demo.fields[1].leading_icon.is_some());
        assert!(demo.fields[1].clearable);
        assert_eq!(demo.fields[1].value, "quadraui");
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
        let mut demo = TextFieldDemo::new();
        demo.fields[0].cursor = Some(0);
        let event = UiEvent::KeyPressed {
            key: quadraui::Key::Char('x'),
            modifiers: Default::default(),
            repeat: false,
        };
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 3.0);
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.fields[0].value, "x");
    }

    #[test]
    fn clicking_the_clear_button_empties_the_search_field() {
        let mut demo = TextFieldDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 40.0, 3.0);
        let rect = TextFieldDemo::field_rect(area, &backend);
        let layout = backend.text_field_layout(rect, &demo.fields[1]);
        let clear = layout.clear_rect.expect("search variant is clearable");
        let event = quadraui::mouse_down(
            quadraui::MouseButton::Left,
            clear.x + 0.5,
            clear.y + 0.5,
            Default::default(),
        );
        let reaction = demo.handle(1, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.fields[1].value, "");
    }
}
