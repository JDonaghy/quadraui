//! Backend-agnostic [`Form`] demo exercising all three [`ControlWidth`]
//! modes on the same field kind (`TextInput`), so the three rows can be
//! compared directly against each other.
//!
//! Static display only — no field in this form needs interactive state
//! for the parity check, so `handle` only wires up quit.
//!
//! Shape: three text inputs sharing one label column —
//! - "Fill" — form-wide default (`ControlWidth::Fill`): the box stretches
//!   to the row's right edge.
//! - "Fixed" — `control_width: Fixed(20.0)`: an exact width regardless of
//!   content or row width.
//! - "Content" — `control_width: Content`: sized to the value text, no
//!   stretch — the pre-D1 look, just repositioned to start right after
//!   the label instead of hugging the row's right edge.
//!
//! All three controls start at the *same* column, right after the shared
//! label column — that shared starting edge, not the differing widths, is
//! what this demo is really showing off.

use quadraui::{
    AppLogic, Backend, ControlWidth, FieldKind, Form, FormField, Key, NamedKey, Reaction, Rect,
    StyledText, UiEvent, WidgetId,
};

pub struct FormControlWidthApp;

impl FormControlWidthApp {
    pub fn new() -> Self {
        Self
    }

    fn text_field(
        id: &str,
        label: &str,
        value: &str,
        control_width: Option<ControlWidth>,
    ) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label),
            kind: FieldKind::TextInput {
                value: value.to_string(),
                placeholder: String::new(),
                cursor: None,
                selection_anchor: None,
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width,
            min_control_width: None,
            row_align: None,
        }
    }

    fn build_form() -> Form {
        Form {
            id: WidgetId::new("control-width-demo"),
            fields: vec![
                Self::text_field("fill", "Fill", "stretches", None),
                Self::text_field(
                    "fixed",
                    "Fixed",
                    "exact width",
                    Some(ControlWidth::Fixed(20.0)),
                ),
                Self::text_field(
                    "content",
                    "Content",
                    "sized to value",
                    Some(ControlWidth::Content),
                ),
            ],
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: ControlWidth::Fill,
            min_control_width: None,
            spacing: Default::default(),
        }
    }
}

impl Default for FormControlWidthApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormControlWidthApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        backend.draw_form(
            Rect::new(0.0, 0.0, vp.width, vp.height),
            &Self::build_form(),
        );
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
