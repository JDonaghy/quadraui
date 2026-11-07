//! Backend-agnostic app code for the button + radio group + switch
//! example ([`tui_controls`] / [`gtk_controls`]).
//!
//! [`ControlsApp`] demonstrates every [`ButtonVariant`] — `Primary`,
//! `Secondary`, `Danger`, `Warning`, `Icon` — stacked vertically above a
//! [`RadioGroup`] and a [`Switch`], each clickable with the mouse and
//! reachable from the keyboard. Every non-`Icon` button can show (or
//! hide) a leading [`Icon`] via the `i` key, so every variant is visible
//! both with and without an icon; `n` toggles whether that icon paints
//! its Nerd Font glyph or its plain-text fallback. Whichever button the
//! pointer is currently over also tints on hover/press via
//! [`InteractionState`] and [`Backend::draw_button_with_state`] —
//! independent of keyboard focus.
//!
//! Controls:
//! - Tab         move focus to the next control
//! - Space/Enter activate the focused control
//! - Up/Down     move the radio group cursor (while it has focus)
//! - i           toggle every button's icon on/off
//! - n           toggle Nerd Font glyphs vs. plain-text fallbacks
//! - Hover       tints whichever button the pointer is over
//! - q / Esc     quit

use quadraui::{
    AppLogic, Backend, Button, ButtonHit, ButtonVariant, Color, Icon, InteractionState, Key,
    Modifiers, NamedKey, RadioGroup, RadioGroupHit, RadioOption, Reaction, Rect, StatusBar,
    StatusBarSegment, Switch, SwitchHit, UiEvent, WidgetId,
};

/// One row per [`ButtonVariant`] — `Primary`, `Secondary`, `Danger`,
/// `Warning`, `Icon` — the full set this example exists to show off.
const BUTTON_COUNT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Button(usize),
    RadioGroup,
    Switch,
}

pub struct ControlsApp {
    focus: Focus,
    clicks: [u32; BUTTON_COUNT],
    selected: Option<usize>,
    wrap_enabled: bool,
    /// Whether every non-`Icon` button's [`Button::icon`] is set — `i`
    /// toggles this, so the same running demo shows every variant both
    /// with and without an icon.
    icons_enabled: bool,
    /// Forwarded to `Backend::set_nerd_fonts` every frame; `n` toggles
    /// it, picking `Icon::glyph` vs. `Icon::fallback` for whichever
    /// button currently carries an icon.
    nerd_fonts: bool,
    last_message: String,
    /// Hover/pressed state for whichever button the pointer is
    /// currently over, keyed by its `WidgetId` — fed from
    /// `MouseMoved`/`MouseDown`/`MouseUp` in `handle` below.
    interaction: InteractionState,
}

/// Static per-row metadata: variant, label, and the icon's Nerd Font
/// glyph / plain-text fallback (ignored for [`ButtonVariant::Icon`],
/// which never reads [`Button::icon`] here).
fn button_spec(idx: usize) -> (ButtonVariant, &'static str, &'static str, &'static str) {
    match idx {
        0 => (ButtonVariant::Primary, "OK", "\u{f00c}", "v"),
        1 => (ButtonVariant::Secondary, "Cancel", "\u{f00d}", "x"),
        2 => (ButtonVariant::Danger, "Delete", "\u{f071}", "!"),
        3 => (ButtonVariant::Warning, "Overwrite", "\u{f071}", "!"),
        _ => (ButtonVariant::Icon, "*", "\u{f013}", "*"),
    }
}

impl ControlsApp {
    pub fn new() -> Self {
        Self {
            focus: Focus::Button(0),
            clicks: [0; BUTTON_COUNT],
            selected: Some(0),
            wrap_enabled: false,
            icons_enabled: true,
            nerd_fonts: false,
            last_message: "Tab=focus Space=activate i=icons n=nerd-fonts".into(),
            interaction: InteractionState::new(),
        }
    }

    fn button(&self, idx: usize) -> Button {
        let (variant, label, glyph, fallback) = button_spec(idx);
        let mut button = Button::new(
            WidgetId::new(format!("controls:button:{idx}")),
            if self.clicks[idx] > 0 {
                format!("{label} ({}x)", self.clicks[idx])
            } else {
                label.to_string()
            },
        );
        button.variant = variant;
        button.focused = self.focus == Focus::Button(idx);
        if self.icons_enabled && !matches!(variant, ButtonVariant::Icon) {
            button.icon = Some(Icon::new(glyph, fallback));
        }
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
                text: format!(
                    " icons={} nerd-fonts={} q=quit ",
                    if self.icons_enabled { "on" } else { "off" },
                    if self.nerd_fonts { "on" } else { "off" }
                ),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn advance_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Button(i) if i + 1 < BUTTON_COUNT => Focus::Button(i + 1),
            Focus::Button(_) => Focus::RadioGroup,
            Focus::RadioGroup => Focus::Switch,
            Focus::Switch => Focus::Button(0),
        };
    }

    fn activate_focused(&mut self) {
        match self.focus {
            Focus::Button(idx) => {
                if self.button(idx).click().is_some() {
                    self.clicks[idx] += 1;
                    self.last_message = format!("Button {idx} clicked");
                }
            }
            Focus::Switch => {
                if let Some(event) = self.switch().click() {
                    let quadraui::SwitchEvent::Toggled { value, .. } = event;
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
    fn button_rect(&self, backend: &dyn Backend, idx: usize) -> Rect {
        let lh = backend.line_height();
        Rect::new(1.0, lh * (1.0 + idx as f32), 24.0, lh)
    }

    fn radio_rect(&self, backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        Rect::new(
            1.0,
            lh * (2.0 + BUTTON_COUNT as f32),
            viewport.width - 2.0,
            lh * 3.0,
        )
    }

    /// Sits on the first line below the radio group, so the whole stack
    /// — buttons, radio group, switch — still clears the bottom status
    /// bar on a short surface (a 12-row terminal).
    fn switch_rect(&self, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        let radio = self.radio_rect(backend);
        Rect::new(1.0, radio.y + radio.height, 24.0, lh)
    }

    fn status_rect(&self, backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, viewport.height - lh, viewport.width, lh)
    }
}

impl AppLogic for ControlsApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.set_nerd_fonts(self.nerd_fonts);

        for idx in 0..BUTTON_COUNT {
            let rect = self.button_rect(backend, idx);
            let _ = backend.draw_button_with_state(rect, &self.button(idx), &self.interaction);
        }
        let _ = backend.draw_radio_group(self.radio_rect(backend), &self.radio_group());
        let _ = backend.draw_switch(self.switch_rect(backend), &self.switch());
        let _ = backend.draw_status_bar_interactive(
            self.status_rect(backend),
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
                key: Key::Char('i'),
                ..
            } => {
                self.icons_enabled = !self.icons_enabled;
                self.last_message =
                    format!("Icons {}", if self.icons_enabled { "on" } else { "off" });
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('n'),
                ..
            } => {
                self.nerd_fonts = !self.nerd_fonts;
                self.last_message = format!(
                    "Nerd-Font icons {}",
                    if self.nerd_fonts { "on" } else { "off" }
                );
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
            UiEvent::MouseMoved { position, .. } => {
                let hovered = (0..BUTTON_COUNT).find_map(|idx| {
                    let rect = self.button_rect(backend, idx);
                    let layout = backend.button_layout(rect, &self.button(idx));
                    matches!(layout.hit_test(position), ButtonHit::Body)
                        .then(|| self.button(idx).id)
                });
                if self.interaction.set_hovered(hovered) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            UiEvent::MouseDown { position, .. } => {
                for idx in 0..BUTTON_COUNT {
                    let rect = self.button_rect(backend, idx);
                    let layout = backend.button_layout(rect, &self.button(idx));
                    if matches!(layout.hit_test(position), ButtonHit::Body) {
                        self.focus = Focus::Button(idx);
                        self.interaction.set_pressed(Some(self.button(idx).id));
                        self.activate_focused();
                        return Reaction::Redraw;
                    }
                }

                let radio_layout =
                    backend.radio_group_layout(self.radio_rect(backend), &self.radio_group());
                if let RadioGroupHit::Option(index) = radio_layout.hit_test(position) {
                    self.focus = Focus::RadioGroup;
                    self.selected = Some(index);
                    self.last_message = format!("Radio -> {}", index);
                    return Reaction::Redraw;
                }

                let switch_layout =
                    backend.switch_layout(self.switch_rect(backend), &self.switch());
                if matches!(switch_layout.hit_test(position), SwitchHit::Body) {
                    self.focus = Focus::Switch;
                    self.activate_focused();
                    return Reaction::Redraw;
                }

                Reaction::Continue
            }
            UiEvent::MouseUp { .. } => {
                if self.interaction.set_pressed(None) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
