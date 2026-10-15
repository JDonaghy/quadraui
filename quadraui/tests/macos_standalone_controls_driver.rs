#![cfg(all(feature = "macos", target_os = "macos"))]
//! Headless `MacDriver` conformance test for the standalone `Button`,
//! `RadioGroup`, and `Switch` primitives: focus traversal, keyboard and
//! pointer activation, all driven through the real `event -> handle ->
//! render` path on the in-memory macOS (CoreGraphics) backend.
//!
//! Unlike the TUI twin (`tests/standalone_controls_driver.rs`), which paints
//! a `TextDisplay` status line because TUI has no dedicated painter for
//! these three controls, this test paints each control through its real
//! `Backend::draw_button` / `draw_radio_group` / `draw_switch` method and
//! hit-tests clicks through the matching backend-agnostic
//! `Backend::button_layout` / `radio_group_layout` / `switch_layout` query —
//! the same split a live click router uses (CLAUDE.md "Layout helpers go
//! through `Backend` too; consumer click routers stay backend-agnostic").

use quadraui::macos::testing::MacDriver;
use quadraui::{
    AppLogic, Backend, Button, ButtonEvent, Key, Modifiers, MouseButton, NamedKey, Point,
    RadioGroup, RadioGroupEvent, RadioOption, Reaction, Rect, Switch, SwitchEvent, UiEvent,
    WidgetId,
};

/// Which of the three standalone controls currently holds keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Button,
    Radios,
    Switch,
}

const BUTTON_BOUNDS: Rect = Rect {
    x: 8.0,
    y: 8.0,
    width: 120.0,
    height: 24.0,
};
const RADIOS_BOUNDS: Rect = Rect {
    x: 8.0,
    y: 40.0,
    width: 120.0,
    height: 48.0,
};
const SWITCH_BOUNDS: Rect = Rect {
    x: 8.0,
    y: 96.0,
    width: 40.0,
    height: 24.0,
};

const CANVAS_W: u32 = 160;
const CANVAS_H: u32 = 140;

/// Minimal `AppLogic` wiring the three standalone controls together with
/// an app-owned `Focus` cursor and an event log, used only by this
/// conformance test (not a demo).
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
        backend.draw_button(BUTTON_BOUNDS, &self.button);
        backend.draw_radio_group(RADIOS_BOUNDS, &self.radios);
        backend.draw_switch(SWITCH_BOUNDS, &self.switch);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
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
                let button_layout = backend.button_layout(BUTTON_BOUNDS, &self.button);
                let radios_layout = backend.radio_group_layout(RADIOS_BOUNDS, &self.radios);
                let switch_layout = backend.switch_layout(SWITCH_BOUNDS, &self.switch);

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

fn find(driver: &MacDriver<StandaloneControlsApp>, needle: &str) -> (f32, f32) {
    driver
        .find(needle)
        .unwrap_or_else(|| panic!("expected to find {needle:?} painted on screen"))
}

#[test]
fn starts_with_button_focused_and_unselected_state() {
    let driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);
    assert_eq!(driver.app().focus, Focus::Button);
    assert_eq!(driver.app().radios.selected, None);
    assert!(!driver.app().switch.value);
    assert!(driver.screen_contains("Save"));
    assert!(driver.screen_contains("Light"));
    assert!(driver.screen_contains("Dark"));
}

#[test]
fn tab_cycles_focus_across_all_three_controls() {
    let mut driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);
    assert_eq!(driver.app().focus, Focus::Button);

    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().focus, Focus::Radios);

    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().focus, Focus::Switch);

    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().focus, Focus::Button);
}

#[test]
fn enter_activates_the_focused_button_and_logs_the_event() {
    let mut driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);
    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().events, vec!["Activated(demo:save)"]);
}

#[test]
fn arrow_keys_move_the_radio_cursor_once_focused() {
    let mut driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().radios.selected, Some(0));

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().radios.selected, Some(1));
    assert_eq!(
        driver.app().events,
        vec!["SelectionChanged(0)", "SelectionChanged(1)"]
    );
}

#[test]
fn enter_toggles_the_switch_once_focused() {
    let mut driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);

    assert!(driver.app().switch.value);
    assert_eq!(driver.app().events, vec!["Toggled(demo:wrap, true)"]);
}

#[test]
fn unfocused_controls_ignore_their_own_activation_keys() {
    let mut driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);
    // Focus starts on the button; Down should not move the (unfocused)
    // radio cursor, and no event should be logged.
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().radios.selected, None);
    assert!(driver.app().events.is_empty());
}

#[test]
fn mouse_click_activates_the_button_without_requiring_focus() {
    let mut driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);
    // Move focus off the button first; a pointer click doesn't need it.
    driver.press_named(NamedKey::Tab);
    let (x, y) = find(&driver, "Save");
    driver.click(x, y);
    assert_eq!(driver.app().events, vec!["Activated(demo:save)"]);
}

#[test]
fn mouse_click_selects_a_radio_option_without_requiring_focus() {
    let mut driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);
    let (x, y) = find(&driver, "Dark");
    driver.click(x, y);
    assert_eq!(driver.app().radios.selected, Some(1));
    assert_eq!(driver.app().events, vec!["SelectionChanged(1)"]);
}

#[test]
fn mouse_click_toggles_the_switch_without_requiring_focus() {
    let mut driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);
    // `Switch` paints only shapes (track + thumb), no text, so there is
    // nothing to `find`; click its known bounds directly instead.
    driver.dispatch(UiEvent::MouseDown {
        widget: None,
        button: MouseButton::Left,
        position: Point::new(SWITCH_BOUNDS.x + 4.0, SWITCH_BOUNDS.y + 4.0),
        modifiers: Modifiers::default(),
    });
    assert!(driver.app().switch.value);
    assert_eq!(driver.app().events, vec!["Toggled(demo:wrap, true)"]);
}
