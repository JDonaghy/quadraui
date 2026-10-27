//! Backend-agnostic Form demo showcasing per-field `label_position` /
//! `label_align` / `label_width` and a custom `tab_index` order.
//!
//! [`AppLogic`] impl driving the TUI runner via the thin
//! `examples/tui_form_label_placement.rs` shell (~10 lines). The TUI
//! backend renders every `LabelPosition` this demo exercises; other
//! backends compile against the new `FormField` fields but don't yet
//! have their own rasteriser support for them.
//!
//! Shape: a "server profile" settings form with five fields, each using a
//! different label placement, and a declared `tab_index` that does *not*
//! match declaration order:
//! - "Role" (`TextInput`)   — `label_position: Top`, tab_index 2
//! - "ID" (`ReadOnly`)      — form-level default `Left`, tab_index 0
//! - "Owner" (`TextInput`)  — form-level default `Left`, tab_index 1
//!   ("ID" and "Owner" share one aligned label column via the form-level
//!   `label_width`.)
//! - "Notes" (`TextInput`)  — `label_position: Inline`, tab_index 3
//! - "Internal key" (`ReadOnly`) — `label_position: Hidden`, tab_index 4
//!
//! Tabbing from "Role" cycles ID → Owner → Role → Notes → Internal key →
//! ID, i.e. declared order "Role, ID, Owner, Notes, Internal key" but
//! visited in `tab_index` order "ID, Owner, Role, Notes, Internal key".
//!
//! Controls:
//! - `Tab` / `Shift+Tab`    → move focus in `tab_index` order
//! - typing (focused field) → edit the focused text field
//! - `Enter` (focused field) → commit the focused text field
//! - `Esc`                  → quit
//! - `q` (no field focused) → quit

use quadraui::{
    AppLogic, Backend, Color, FieldKind, Form, FormController, FormControllerEvent, FormField,
    InteractionState, LabelAlign, LabelPosition, Reaction, Rect, StatusBar, StatusBarSegment,
    StyledText, UiEvent, WidgetId,
};

pub struct FormLabelPlacementApp {
    fc: FormController,
    role: String,
    id: String,
    owner: String,
    notes: String,
    internal_key: String,
    focused: Option<WidgetId>,
    last_action: String,
}

impl FormLabelPlacementApp {
    pub fn new() -> Self {
        let mut fc = FormController::new("profile".into());
        fc.set_has_focus(true);
        let mut app = Self {
            fc,
            role: "Operator".into(),
            id: "srv-042".into(),
            owner: "ops-team".into(),
            notes: String::new(),
            internal_key: "xk-9f21".into(),
            focused: Some(WidgetId::new("id")),
            last_action: "—".into(),
        };
        // `FormController::render` paints its own internally cached
        // `Form`, populated only by `set_form` — without this, the very
        // first frame (painted before any `handle` call) would render
        // blank.
        app.fc.set_form(app.build_form());
        app
    }

    fn text_field(
        &self,
        id: &str,
        label: &str,
        value: &str,
        label_position: Option<LabelPosition>,
        tab_index: i32,
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
            label_position,
            label_align: None,
            label_width: None,
            tab_index: Some(tab_index),
        }
    }

    fn read_only_field(
        &self,
        id: &str,
        label: &str,
        value: &str,
        label_position: Option<LabelPosition>,
        tab_index: i32,
    ) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label),
            kind: FieldKind::ReadOnly {
                value: StyledText::plain(value),
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position,
            label_align: None,
            label_width: None,
            tab_index: Some(tab_index),
        }
    }

    fn build_form(&self) -> Form {
        let fields = vec![
            self.text_field("role", "Role", &self.role, Some(LabelPosition::Top), 2),
            self.read_only_field("id", "ID", &self.id, None, 0),
            self.text_field("owner", "Owner", &self.owner, None, 1),
            self.text_field(
                "notes",
                "Notes",
                &self.notes,
                Some(LabelPosition::Inline),
                3,
            ),
            self.read_only_field(
                "internal-key",
                "Internal key",
                &self.internal_key,
                Some(LabelPosition::Hidden),
                4,
            ),
        ];

        Form {
            id: WidgetId::new("profile-form"),
            fields,
            focused_field: self.focused,
            scroll_offset: 0,
            has_focus: true,
            label_position: LabelPosition::Left,
            label_align: LabelAlign::End,
            label_width: Some(14.0),
        }
    }

    fn form_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let status_h = backend.line_height() * 1.5;
        Rect::new(0.0, 0.0, vp.width, (vp.height - status_h).max(0.0))
    }

    fn status_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let status_h = backend.line_height() * 1.5;
        Rect::new(0.0, (vp.height - status_h).max(0.0), vp.width, status_h)
    }

    fn build_status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_action),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " Tab / type / Enter / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FormLabelPlacementApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormLabelPlacementApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let form_rect = Self::form_rect(backend);
        let status_rect = Self::status_rect(backend);
        self.fc.render(backend, form_rect);
        let _hits = backend.draw_status_bar_interactive(
            status_rect,
            &self.build_status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let editing = self.focused.is_some();
        match &event {
            UiEvent::KeyPressed {
                key: quadraui::Key::Named(quadraui::NamedKey::Escape),
                ..
            } => return Reaction::Exit,
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('q'),
                ..
            } if !editing => return Reaction::Exit,
            UiEvent::WindowResized { .. } => return Reaction::Redraw,
            _ => {}
        }

        self.fc.set_form(self.build_form());
        self.fc.set_backend_info(backend.line_height());
        let rect = Self::form_rect(backend);
        match self.fc.handle_cached(&event, rect) {
            FormControllerEvent::FormAction(fe) => {
                match fe {
                    quadraui::FormEvent::TextInputChanged { id, value } => {
                        match id.as_str() {
                            "role" => self.role = value.clone(),
                            "owner" => self.owner = value.clone(),
                            "notes" => self.notes = value.clone(),
                            _ => {}
                        }
                        self.last_action = format!("{} = {:?}", id.as_str(), value);
                    }
                    quadraui::FormEvent::TextInputCommitted { id, value } => {
                        match id.as_str() {
                            "role" => self.role = value.clone(),
                            "owner" => self.owner = value.clone(),
                            "notes" => self.notes = value.clone(),
                            _ => {}
                        }
                        self.last_action = format!("{} committed: {:?}", id.as_str(), value);
                    }
                    quadraui::FormEvent::FocusChanged { id } => {
                        self.last_action = format!("focus → {}", id.as_str());
                        self.focused = Some(id);
                    }
                    _ => {
                        self.last_action = format!("{:?}", fe);
                    }
                }
                self.fc.set_form(self.build_form());
                Reaction::Redraw
            }
            FormControllerEvent::ScrollChanged | FormControllerEvent::Consumed => Reaction::Redraw,
            FormControllerEvent::Ignored => Reaction::Continue,
        }
    }
}
