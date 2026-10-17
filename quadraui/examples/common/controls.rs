//! Backend-agnostic app code for the button + radio group + switch
//! example ([`tui_controls`] / [`gtk_controls`]).
//!
//! [`ControlsApp`] demonstrates a [`Button`], a [`RadioGroup`] and a
//! [`Switch`] stacked vertically, each clickable with the mouse and
//! reachable from the keyboard.
//!
//! Controls:
//! - Tab         move focus to the next control
//! - Space/Enter activate the focused control
//! - Up/Down     move the radio group cursor (while it has focus)
//! - q / Esc     quit

use quadraui::{
    AppLogic, Backend, Button, ButtonHit, Color, Key, Modifiers, NamedKey, RadioGroup,
    RadioGroupHit, RadioOption, Reaction, Rect, StatusBar, StatusBarSegment, Switch, SwitchHit,
    UiEvent, WidgetId,
};

const FOCUS_ORDER: [Focus; 3] = [Focus::Button, Focus::RadioGroup, Focus::Switch];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Button,
    RadioGroup,
    Switch,
}

pub struct ControlsApp {
    focus: Focus,
    clicks: u32,
    selected: Option<usize>,
    wrap_enabled: bool,
    last_message: String,
}

impl ControlsApp {
    pub fn new() -> Self {
        Self {
            focus: Focus::Button,
            clicks: 0,
            selected: Some(0),
            wrap_enabled: false,
            last_message: "Tab=focus Space=activate Up/Down=radio".into(),
        }
    }

    fn button(&self) -> Button {
        let mut button = Button::new(
            WidgetId::new("controls:button"),
            format!("Clicked {}x", self.clicks),
        );
        button.focused = self.focus == Focus::Button;
        button
    }

    fn radio_group(&self) -> RadioGroup {
        let mut group = RadioGroup::new(
            WidgetId::new("controls:radio"),
            vec![
                RadioOption::new(WidgetId::new("controls:radio:light"), "Light"),
                RadioOption::new(WidgetId::new("controls:radio:dark"), "Dark"),
                RadioOption::new(WidgetId::new("controls:radio:auto"), "Auto"),
            ],
        );
        group.selected = self.selected;
        group.focused = self.focus == Focus::RadioGroup;
        group
    }

    fn switch(&self) -> Switch {
        let mut switch = Switch::new(WidgetId::new("controls:switch"));
        switch.value = self.wrap_enabled;
        switch.focused = self.focus == Focus::Switch;
        switch
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn advance_focus(&mut self) {
        let idx = FOCUS_ORDER
            .iter()
            .position(|f| *f == self.focus)
            .unwrap_or(0);
        self.focus = FOCUS_ORDER[(idx + 1) % FOCUS_ORDER.len()];
    }

    fn activate_focused(&mut self) {
        match self.focus {
            Focus::Button => {
                if self.button().click().is_some() {
                    self.clicks += 1;
                    self.last_message = "Button clicked".into();
                }
            }
            Focus::Switch => {
                if let Some(event) = self.switch().click() {
                    let quadraui::SwitchEvent::Toggled { value, .. } = event else {
                        return;
                    };
                    self.wrap_enabled = value;
                    self.last_message = format!("Switch -> {}", value);
                }
            }
            Focus::RadioGroup => {}
        }
    }
}

impl Default for ControlsApp {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlsApp {
    fn layout_rects(&self, backend: &dyn Backend) -> (Rect, Rect, Rect, Rect) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let button_rect = Rect::new(1.0, lh, 16.0, lh);
        let radio_rect = Rect::new(1.0, lh * 3.0, viewport.width - 2.0, lh * 3.0);
        let switch_rect = Rect::new(1.0, lh * 7.0, 24.0, lh);
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        (button_rect, radio_rect, switch_rect, status_rect)
    }
}

impl AppLogic for ControlsApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let (button_rect, radio_rect, switch_rect, status_rect) = self.layout_rects(backend);

        let _ = backend.draw_button(button_rect, &self.button());
        let _ = backend.draw_radio_group(radio_rect, &self.radio_group());
        let _ = backend.draw_switch(switch_rect, &self.switch());
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &quadraui::InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.advance_focus();
                self.last_message = "Focus moved".into();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char(' '),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } => {
                self.activate_focused();
                Reaction::Redraw
            }
            UiEvent::KeyPressed { key, .. }
                if self.focus == Focus::RadioGroup
                    && matches!(
                        key,
                        Key::Named(NamedKey::Up)
                            | Key::Named(NamedKey::Down)
                            | Key::Named(NamedKey::Left)
                            | Key::Named(NamedKey::Right)
                    ) =>
            {
                let group = self.radio_group();
                if let Some(quadraui::RadioGroupEvent::SelectionChanged { index }) =
                    group.handle_key(&key, &Modifiers::default())
                {
                    self.selected = Some(index);
                    self.last_message = format!("Radio -> {}", index);
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }
            UiEvent::MouseDown { position, .. } => {
                let (button_rect, radio_rect, switch_rect, _status_rect) =
                    self.layout_rects(backend);

                let button_layout = backend.button_layout(button_rect, &self.button());
                if matches!(button_layout.hit_test(position), ButtonHit::Body) {
                    self.focus = Focus::Button;
                    self.activate_focused();
                    return Reaction::Redraw;
                }

                let radio_layout = backend.radio_group_layout(radio_rect, &self.radio_group());
                if let RadioGroupHit::Option(index) = radio_layout.hit_test(position) {
                    self.focus = Focus::RadioGroup;
                    self.selected = Some(index);
                    self.last_message = format!("Radio -> {}", index);
                    return Reaction::Redraw;
                }

                let switch_layout = backend.switch_layout(switch_rect, &self.switch());
                if matches!(switch_layout.hit_test(position), SwitchHit::Body) {
                    self.focus = Focus::Switch;
                    self.activate_focused();
                    return Reaction::Redraw;
                }

                Reaction::Continue
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
