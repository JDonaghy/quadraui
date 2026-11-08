//! Backend-agnostic [`Form`] demo exercising [`FormSpacing`] (padding +
//! `item_gap`) and [`RowAlign`] together.
//!
//! Static display only — no field in this form needs interactive state
//! for the parity check, so `handle` only wires up quit.
//!
//! Shape: two text inputs followed by a dialog-footer-style
//! `FieldKind::ButtonRow` ("Cancel" / "OK") that sets
//! `row_align: Some(RowAlign::End)`, so the pair sits flush against the
//! form's right edge instead of hugging the left like every other
//! `ButtonRow` in this crate's other demos. `Form::spacing.padding`
//! insets the whole form a few cells/pixels in from the surface it
//! paints into, and `Form::spacing.item_gap` widens the gap between the
//! two buttons beyond whatever gap the backend would otherwise use.
use quadraui::{
    AppLogic, Backend, ButtonRowItem, FieldKind, Form, FormField, FormSpacing, Key, NamedKey,
    Reaction, Rect, RowAlign, StyledText, UiEvent, WidgetId,
};

pub struct FormSpacingApp;

impl FormSpacingApp {
    pub fn new() -> Self {
        Self
    }

    fn text_field(id: &str, label: &str, value: &str) -> FormField {
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
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn footer_buttons() -> FormField {
        FormField {
            id: WidgetId::new("footer"),
            label: StyledText::default(),
            kind: FieldKind::ButtonRow {
                buttons: vec![
                    ButtonRowItem {
                        id: WidgetId::new("cancel"),
                        label: "Cancel".into(),
                        disabled: false,
                        icon: None,
                        spec: None,
                    },
                    ButtonRowItem {
                        id: WidgetId::new("ok"),
                        label: "OK".into(),
                        disabled: false,
                        icon: None,
                        spec: None,
                    },
                ],
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            // Right-aligns the footer against the (padded) row's right
            // edge — the behaviour this demo is showing off.
            row_align: Some(RowAlign::End),
        }
    }

    fn build_form() -> Form {
        Form {
            id: WidgetId::new("spacing-demo"),
            fields: vec![
                Self::text_field("name", "Name", ""),
                Self::text_field("email", "Email", ""),
                Self::footer_buttons(),
            ],
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: FormSpacing {
                padding: 2.0,
                item_gap: Some(3.0),
                row_align: RowAlign::Start,
            },
        }
    }
}

impl Default for FormSpacingApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormSpacingApp {
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
