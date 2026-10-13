#![cfg(feature = "tui")]
//! Headless `TuiDriver` conformance test for the standalone `Button`,
//! `RadioGroup`, and `Switch` primitives: focus traversal, keyboard and
//! pointer activation, and the events each emits, all driven through the
//! real `event -> handle -> render` path on the in-memory TUI backend.
//!
//! `Button`/`RadioGroup`/`Switch` have no `Backend` painter of their own
//! (none is in scope for this issue), so this test's `render` paints a
//! status line per control via `TextDisplay`, the one primitive with an
//! already-implemented `Backend::draw_text_display`. Each primitive's own
//! `handle_key`/`click`/`select` is a pure `&self` method that returns an
//! event without mutating the struct — `handle` below applies the
//! returned event back onto the owned field itself, matching the "host
//! applies the event" split each primitive's module doc already states.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Button, ButtonEvent, Decoration, Key, Modifiers, MouseButton, NamedKey,
    Point, RadioGroup, RadioGroupEvent, RadioOption, RadioOptionMeasure, Reaction, Rect,
    StyledSpan, Switch, SwitchEvent, TextDisplay, TextDisplayLine, UiEvent, WidgetId,
};

/// Which of the three standalone controls currently holds keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Button,
    Radios,
    Switch,
}

const BUTTON_BOUNDS: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 20.0,
    height: 1.0,
};
const RADIOS_BOUNDS: Rect = Rect {
    x: 0.0,
    y: 1.0,
    width: 20.0,
    height: 2.0,
};
const SWITCH_BOUNDS: Rect = Rect {
    x: 0.0,
    y: 3.0,
    width: 20.0,
    height: 1.0,
};

fn radio_measure(_option: &RadioOption) -> RadioOptionMeasure {
    RadioOptionMeasure {
        width: 20.0,
        height: 1.0,
    }
}

/// Minimal `AppLogic` wiring the three standalone controls together with
/// an app-owned `Focus` cursor and an event log, used only by this
/// conformance test (not a demo — this issue ships no backend painter).
struct StandaloneControlsApp {
    focus: Focus,
    button: Button,
    radios: RadioGroup,
    switch: Switch,
    events: Vec<String>,
}

impl StandaloneControlsApp {
    fn new() -> Self {
        let mut button = Button::new(WidgetId::new("demo:save"), "Save");
        button.focused = true;
        let radios = RadioGroup::new(
            WidgetId::new("demo:theme"),
            vec![
                RadioOption::new(WidgetId::new("demo:theme:light"), "Light"),
                RadioOption::new(WidgetId::new("demo:theme:dark"), "Dark"),
            ],
        );
        let switch = Switch::new(WidgetId::new("demo:wrap"));
        Self {
            focus: Focus::Button,
            button,
            radios,
            switch,
            events: Vec::new(),
        }
    }

    fn sync_focus_flags(&mut self) {
        self.button.focused = self.focus == Focus::Button;
        self.radios.focused = self.focus == Focus::Radios;
        self.switch.focused = self.focus == Focus::Switch;
    }

    fn advance_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Button => Focus::Radios,
            Focus::Radios => Focus::Switch,
            Focus::Switch => Focus::Button,
        };
        self.sync_focus_flags();
    }
}

impl AppLogic for StandaloneControlsApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut display = TextDisplay::new(WidgetId::new("demo:status"));
        display.append_line(TextDisplayLine {
            spans: vec![StyledSpan::plain(format!(
                "[{}] button: {}",
                if self.focus == Focus::Button {
                    "*"
                } else {
                    " "
                },
                self.button.label
            ))],
            decoration: Decoration::default(),
            timestamp: None,
        });
        display.append_line(TextDisplayLine {
            spans: vec![StyledSpan::plain(format!(
                "[{}] theme: {:?}",
                if self.focus == Focus::Radios {
                    "*"
                } else {
                    " "
                },
                self.radios.selected
            ))],
            decoration: Decoration::default(),
            timestamp: None,
        });
        display.append_line(TextDisplayLine {
            spans: vec![StyledSpan::plain(format!(
                "[{}] wrap: {}",
                if self.focus == Focus::Switch {
                    "*"
                } else {
                    " "
                },
                self.switch.value
            ))],
            decoration: Decoration::default(),
            timestamp: None,
        });
        backend.draw_text_display(Rect::new(0.0, 0.0, 40.0, 4.0), &display);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, modifiers, .. } => {
                if key == Key::Named(NamedKey::Tab) {
                    self.advance_focus();
                    return Reaction::Redraw;
                }
                match self.focus {
                    Focus::Button => {
                        if let Some(ButtonEvent::Activated { id }) =
                            self.button.handle_key(&key, &modifiers)
                        {
                            self.events.push(format!("Activated({})", id.as_str()));
                        }
                    }
                    Focus::Radios => {
                        if let Some(RadioGroupEvent::SelectionChanged { index }) =
                            self.radios.handle_key(&key, &modifiers)
                        {
                            self.radios.selected = Some(index);
                            self.events.push(format!("SelectionChanged({index})"));
                        }
                    }
                    Focus::Switch => {
                        if let Some(SwitchEvent::Toggled { id, value }) =
                            self.switch.handle_key(&key, &modifiers)
                        {
                            self.switch.value = value;
                            self.events
                                .push(format!("Toggled({}, {value})", id.as_str()));
                        }
                    }
                }
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let button_layout = self.button.layout(BUTTON_BOUNDS);
                let radios_layout = self.radios.layout(RADIOS_BOUNDS, radio_measure);
                let switch_layout = self.switch.layout(SWITCH_BOUNDS);

                if button_layout.bounds.contains(position) {
                    if let Some(ButtonEvent::Activated { id }) = self.button.click() {
                        self.events.push(format!("Activated({})", id.as_str()));
                    }
                } else if switch_layout.bounds.contains(position) {
                    if let Some(SwitchEvent::Toggled { id, value }) = self.switch.click() {
                        self.switch.value = value;
                        self.events
                            .push(format!("Toggled({}, {value})", id.as_str()));
                    }
                } else {
                    for (index, item) in radios_layout.items.iter().enumerate() {
                        if item.contains(position) {
                            if let Some(RadioGroupEvent::SelectionChanged { index }) =
                                self.radios.select(index)
                            {
                                self.radios.selected = Some(index);
                                self.events.push(format!("SelectionChanged({index})"));
                            }
                            break;
                        }
                    }
                }
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

#[test]
fn starts_with_button_focused_and_unselected_state() {
    let driver = TuiDriver::new(StandaloneControlsApp::new(), 40, 6);
    assert!(driver.screen_contains("[*] button: Save"));
    assert!(driver.screen_contains("theme: None"));
    assert!(driver.screen_contains("wrap: false"));
}

#[test]
fn tab_cycles_focus_across_all_three_controls() {
    let mut driver = TuiDriver::new(StandaloneControlsApp::new(), 40, 6);
    assert_eq!(driver.app().focus, Focus::Button);

    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().focus, Focus::Radios);
    assert!(driver.screen_contains("[*] theme: None"));

    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().focus, Focus::Switch);
    assert!(driver.screen_contains("[*] wrap: false"));

    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().focus, Focus::Button);
    assert!(driver.screen_contains("[*] button: Save"));
}

#[test]
fn enter_activates_the_focused_button_and_logs_the_event() {
    let mut driver = TuiDriver::new(StandaloneControlsApp::new(), 40, 6);
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().events, vec!["Activated(demo:save)"]);
}

#[test]
fn arrow_keys_move_the_radio_cursor_once_focused() {
    let mut driver = TuiDriver::new(StandaloneControlsApp::new(), 40, 6);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().radios.selected, Some(0));
    assert!(driver.screen_contains("theme: Some(0)"));

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().radios.selected, Some(1));
    assert_eq!(
        driver.app().events,
        vec!["SelectionChanged(0)", "SelectionChanged(1)"]
    );
}

#[test]
fn enter_toggles_the_switch_once_focused() {
    let mut driver = TuiDriver::new(StandaloneControlsApp::new(), 40, 6);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);

    assert!(driver.app().switch.value);
    assert!(driver.screen_contains("[*] wrap: true"));
    assert_eq!(driver.app().events, vec!["Toggled(demo:wrap, true)"]);
}

#[test]
fn unfocused_controls_ignore_their_own_activation_keys() {
    let mut driver = TuiDriver::new(StandaloneControlsApp::new(), 40, 6);
    // Focus starts on the button; Down should not move the (unfocused)
    // radio cursor, and no event should be logged.
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().radios.selected, None);
    assert!(driver.app().events.is_empty());
}

#[test]
fn mouse_click_activates_the_button_without_requiring_focus() {
    let mut driver = TuiDriver::new(StandaloneControlsApp::new(), 40, 6);
    // Move focus off the button first; a pointer click doesn't need it.
    driver.press_named(NamedKey::Tab);
    driver.dispatch(UiEvent::MouseDown {
        widget: None,
        button: MouseButton::Left,
        position: Point::new(1.0, 0.0),
        modifiers: Modifiers::default(),
    });
    assert_eq!(driver.app().events, vec!["Activated(demo:save)"]);
}

#[test]
fn mouse_click_selects_a_radio_option_without_requiring_focus() {
    let mut driver = TuiDriver::new(StandaloneControlsApp::new(), 40, 6);
    driver.dispatch(UiEvent::MouseDown {
        widget: None,
        button: MouseButton::Left,
        position: Point::new(1.0, 2.0),
        modifiers: Modifiers::default(),
    });
    assert_eq!(driver.app_mut().radios.selected, Some(1));
    assert_eq!(driver.app().events, vec!["SelectionChanged(1)"]);
}

#[test]
fn mouse_click_toggles_the_switch_without_requiring_focus() {
    let mut driver = TuiDriver::new(StandaloneControlsApp::new(), 40, 6);
    driver.dispatch(UiEvent::MouseDown {
        widget: None,
        button: MouseButton::Left,
        position: Point::new(1.0, 3.0),
        modifiers: Modifiers::default(),
    });
    assert!(driver.app().switch.value);
    assert_eq!(driver.app().events, vec!["Toggled(demo:wrap, true)"]);
}
