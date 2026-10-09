//! Backend-agnostic Form scroll demo showcasing `FormController` with
//! built-in scrollbar support.
//!
//! Single [`AppLogic`] impl drives both the TUI runner and the GTK
//! runner. The thin shells in `examples/{tui,gtk}_form_scroll.rs` are
//! each ~10 lines.
//!
//! Shape: a settings panel with one `TextInput` field followed by 20
//! toggle fields — enough to overflow any reasonable viewport and
//! trigger the scrollbar. FormController owns scroll state, renders the
//! scrollbar, handles scroll wheel / scrollbar click / thumb-drag, and
//! keyboard focus traversal and text editing (see
//! `compose::form_controller`'s module doc). Note that `handle` below
//! has no key-decoding arm of its own for the `name` field:
//! `fc.handle_cached` does all of it, demonstrating "no app-side key
//! plumbing is needed to edit a field".
//!
//! Controls:
//! - mouse click on toggle  → flip value
//! - `Tab` / `Shift+Tab`    → move focus between fields
//! - typing (name focused)  → edit the name field
//! - `Enter` (name focused) → commit the name field
//! - scroll wheel           → scroll form
//! - scrollbar drag         → scroll form
//! - `q` / `Esc`            → quit

use quadraui::{
    AppLogic, Backend, Color, FieldKind, Form, FormController, FormControllerEvent, FormField,
    InteractionState, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

pub struct FormScrollApp {
    fc: FormController,
    name: String,
    toggles: Vec<bool>,
    focused: Option<WidgetId>,
    last_action: String,
}

impl FormScrollApp {
    pub fn new() -> Self {
        Self {
            fc: FormController::new("settings".into()),
            name: String::new(),
            toggles: vec![false; 20],
            focused: Some(WidgetId::new("name")),
            last_action: "—".into(),
        }
    }

    fn build_form(&self) -> Form {
        let mut fields = vec![FormField {
            id: WidgetId::new("name"),
            label: StyledText::plain("Name"),
            kind: FieldKind::TextInput {
                value: self.name.clone(),
                placeholder: "Your name…".into(),
                cursor: None,
                selection_anchor: None,
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }];
        fields.extend(self.toggles.iter().enumerate().map(|(i, &val)| FormField {
            id: WidgetId::new(format!("toggle-{i}")),
            label: StyledText::plain(format!("Setting {}", i + 1)),
            kind: FieldKind::Toggle { value: val },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }));

        Form {
            id: WidgetId::new("settings-form"),
            fields,
            focused_field: self.focused.clone(),
            scroll_offset: 0,
            has_focus: true,
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
                text: format!(
                    " scroll={} | {} ",
                    self.fc.scroll_offset(),
                    self.last_action
                ),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " Tab focus / type / Enter / scroll / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FormScrollApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormScrollApp {
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
        // `q` quits — unless the name field has keyboard focus, in which
        // case it's a character the user is typing into it.
        let editing_name = self.focused.as_ref().map(WidgetId::as_str) == Some("name");
        match &event {
            UiEvent::KeyPressed {
                key: quadraui::Key::Named(quadraui::NamedKey::Escape),
                ..
            } => return Reaction::Exit,
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('q'),
                ..
            } if !editing_name => return Reaction::Exit,
            UiEvent::WindowResized { .. } => return Reaction::Redraw,
            _ => {}
        }

        self.fc.set_form(self.build_form());
        self.fc.set_backend_info(backend.line_height());
        let rect = Self::form_rect(backend);
        match self.fc.handle_cached(&event, rect) {
            FormControllerEvent::FormAction(fe) => {
                match fe {
                    quadraui::FormEvent::ToggleChanged { ref id, value } => {
                        let prefix = "toggle-";
                        if let Ok(idx) = id
                            .as_str()
                            .strip_prefix(prefix)
                            .unwrap_or("")
                            .parse::<usize>()
                        {
                            if idx < self.toggles.len() {
                                self.toggles[idx] = value;
                            }
                        }
                        self.last_action = format!("{} = {}", id.as_str(), value);
                    }
                    // No app-side key plumbing: FormController decoded
                    // the keystroke into the new text and the cursor
                    // position it needs to keep editing — the app only
                    // persists the resulting `value` string, same as it
                    // already does for a toggle flip above.
                    quadraui::FormEvent::TextInputChanged { id, value }
                        if id.as_str() == "name" =>
                    {
                        self.name = value;
                        self.last_action = format!("name = {:?}", self.name);
                    }
                    quadraui::FormEvent::TextInputCommitted { id, value }
                        if id.as_str() == "name" =>
                    {
                        self.name = value;
                        self.last_action = format!("name committed: {:?}", self.name);
                    }
                    quadraui::FormEvent::FocusChanged { id } => {
                        self.last_action = format!("focus → {}", id.as_str());
                        self.focused = Some(id);
                    }
                    _ => {
                        self.last_action = format!("{:?}", fe);
                    }
                }
                Reaction::Redraw
            }
            FormControllerEvent::ScrollChanged | FormControllerEvent::Consumed => Reaction::Redraw,
            FormControllerEvent::Ignored => Reaction::Continue,
        }
    }
}
