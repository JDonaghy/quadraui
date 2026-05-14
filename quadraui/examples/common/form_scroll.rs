//! Backend-agnostic Form scroll demo showcasing `FormController` with
//! built-in scrollbar support.
//!
//! Single [`AppLogic`] impl drives both the TUI runner and the GTK
//! runner. The thin shells in `examples/{tui,gtk}_form_scroll.rs` are
//! each ~10 lines.
//!
//! Shape: a settings panel with 20 toggle fields — enough to overflow
//! any reasonable viewport and trigger the scrollbar. FormController
//! owns scroll state, renders the scrollbar, and handles scroll wheel /
//! scrollbar click / thumb-drag internally.
//!
//! Controls:
//! - mouse click on toggle  → flip value
//! - scroll wheel           → scroll form
//! - scrollbar drag         → scroll form
//! - `q` / `Esc`            → quit

use quadraui::{
    AppLogic, Backend, Color, FieldKind, Form, FormController, FormControllerEvent, FormField,
    Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

pub struct FormScrollApp {
    fc: FormController,
    toggles: Vec<bool>,
    last_action: String,
}

impl FormScrollApp {
    pub fn new() -> Self {
        Self {
            fc: FormController::new("settings".into()),
            toggles: vec![false; 20],
            last_action: "—".into(),
        }
    }

    fn build_form(&self) -> Form {
        let fields: Vec<FormField> = self
            .toggles
            .iter()
            .enumerate()
            .map(|(i, &val)| FormField {
                id: WidgetId::new(format!("toggle-{i}")),
                label: StyledText::plain(format!("Setting {}", i + 1)),
                kind: FieldKind::Toggle { value: val },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            })
            .collect();

        Form {
            id: WidgetId::new("settings-form"),
            fields,
            focused_field: None,
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
                text: " scroll / click / drag scrollbar / q ".into(),
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
        let _hits = backend.draw_status_bar(status_rect, &self.build_status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match &event {
            UiEvent::KeyPressed { key, .. } => match key {
                quadraui::Key::Char('q') | quadraui::Key::Named(quadraui::NamedKey::Escape) => {
                    return Reaction::Exit
                }
                _ => {}
            },
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
