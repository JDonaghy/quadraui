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
    AppLogic, Backend, Button, ButtonEvent, ButtonHit, ButtonVariant, Icon, InteractionState, Key,
    Modifiers, MouseButton, NamedKey, Point, RadioGroup, RadioGroupEvent, RadioOption, Reaction,
    Rect, Switch, SwitchEvent, UiEvent, WidgetId,
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
/// A static `Danger` button, painted but not reachable from focus/click
/// — probed only for its fill colour against [`BUTTON_BOUNDS`]'s
/// `Primary` one.
const DANGER_BOUNDS: Rect = Rect {
    x: 8.0,
    y: 128.0,
    width: 120.0,
    height: 24.0,
};
/// A static `Warning` button, same role as [`DANGER_BOUNDS`].
const WARNING_BOUNDS: Rect = Rect {
    x: 8.0,
    y: 160.0,
    width: 120.0,
    height: 24.0,
};

const CANVAS_W: u32 = 160;
const CANVAS_H: u32 = 192;

/// Minimal `AppLogic` wiring the three standalone controls together with
/// an app-owned `Focus` cursor and an event log, used only by this
/// conformance test (not a demo).
struct StandaloneControlsApp {
    focus: Focus,
    button: Button,
    radios: RadioGroup,
    switch: Switch,
    /// `Danger`/`Warning` siblings of `button`, painted at fixed bounds
    /// outside the focus/click wiring — probed only for their fill
    /// colour (see `danger_and_warning_fills_differ_from_primary`).
    danger: Button,
    warning: Button,
    events: Vec<String>,
    /// Hover/pressed state for `button`, fed from `MouseMoved` below —
    /// `button` is the only control here painted via
    /// `Backend::draw_button_with_state` (B3's pointer-enter hover
    /// story).
    interaction: InteractionState,
}

impl StandaloneControlsApp {
    fn new() -> Self {
        let mut button = Button::new(WidgetId::new("demo:save"), "Save");
        button.focused = true;
        // A leading icon, `6.0` px clear of the label — exercises
        // `Button::icon`/`icon_position`/`icon_gap` on the one control
        // whose ordering/spacing this driver actually probes.
        button.icon = Some(Icon::codicon("save", "S").expect("\"save\" is a real codicon name"));
        button.icon_gap = 6.0;
        let mut danger = Button::new(WidgetId::new("demo:delete"), "Delete");
        danger.variant = ButtonVariant::Danger;
        let mut warning = Button::new(WidgetId::new("demo:overwrite"), "Overwrite");
        warning.variant = ButtonVariant::Warning;
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
            danger,
            warning,
            events: Vec::new(),
            interaction: InteractionState::new(),
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
        backend.draw_button_with_state(BUTTON_BOUNDS, &self.button, &self.interaction);
        backend.draw_button(DANGER_BOUNDS, &self.danger);
        backend.draw_button(WARNING_BOUNDS, &self.warning);
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
            UiEvent::MouseMoved { position, .. } => {
                let button_layout = backend.button_layout(BUTTON_BOUNDS, &self.button);
                let hovered = matches!(button_layout.hit_test(position), ButtonHit::Body)
                    .then_some(self.button.id);
                if self.interaction.set_hovered(hovered) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
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

/// A pixel offset from each button's own left edge, at mid-height, that
/// always lands on the fill rather than its centred label/icon text:
/// `Style::default().padding == 8.0` is the inset
/// `native_surface_paint::paint` never draws label/icon content inside
/// of, and `6 < 8` sits inside that padding band.
const FILL_PROBE_X: f32 = 6.0;

fn fill_pixel(driver: &MacDriver<StandaloneControlsApp>, bounds: Rect) -> (u8, u8, u8, u8) {
    driver.pixel(
        (bounds.x + FILL_PROBE_X) as u32,
        (bounds.y + bounds.height / 2.0) as u32,
    )
}

/// `Danger`/`Warning` fill with `Theme::button_danger_bg`/
/// `button_warning_bg`, visibly distinct from `Primary`'s `accent_bg` —
/// the intent-colouring half of this issue's acceptance.
#[test]
fn danger_and_warning_fills_differ_from_primary() {
    let driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);

    let primary = fill_pixel(&driver, BUTTON_BOUNDS);
    let danger = fill_pixel(&driver, DANGER_BOUNDS);
    let warning = fill_pixel(&driver, WARNING_BOUNDS);

    assert_ne!(danger, primary, "Danger must not fill the same as Primary");
    assert_ne!(
        warning, primary,
        "Warning must not fill the same as Primary"
    );
    assert_ne!(danger, warning, "Danger and Warning must look distinct");
}

/// `Backend::draw_button_with_state` repaints the fill on pointer enter
/// — the hover half of this issue's acceptance.
#[test]
fn hover_changes_the_button_fill_on_pointer_enter() {
    let mut driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);

    let resting = fill_pixel(&driver, BUTTON_BOUNDS);
    driver.mouse_move(BUTTON_BOUNDS.x + FILL_PROBE_X, BUTTON_BOUNDS.y + 12.0);
    let hovered = fill_pixel(&driver, BUTTON_BOUNDS);

    assert_ne!(
        resting, hovered,
        "hovering the button must repaint its fill"
    );

    // Moving off the button restores the resting fill.
    driver.mouse_move(1.0, 1.0);
    let after = fill_pixel(&driver, BUTTON_BOUNDS);
    assert_eq!(
        after, resting,
        "moving off the button must clear the hover fill"
    );
}

/// `Button::icon` with the default `Leading` position paints before the
/// label, `icon_gap` clear of it — the icon-placement half of this
/// issue's acceptance.
#[test]
fn leading_icon_paints_before_the_label_icon_gap_apart() {
    let driver = MacDriver::new(StandaloneControlsApp::new(), CANVAS_W, CANVAS_H);

    let save_glyph = Icon::codicon("save", "S")
        .expect("\"save\" is a real codicon name")
        .glyph;
    let icon_bounds = driver
        .find_bounds(&save_glyph)
        .expect("the Save button's leading icon must paint a text run");
    let label_bounds = driver
        .find_bounds("Save")
        .expect("the Save button's label must paint a text run");

    assert!(
        icon_bounds.x < label_bounds.x,
        "a Leading icon must paint before the label: icon.x={}, label.x={}",
        icon_bounds.x,
        label_bounds.x,
    );
    let gap = label_bounds.x - (icon_bounds.x + icon_bounds.width);
    assert!(
        (gap - driver.app().button.icon_gap).abs() < 0.5,
        "expected icon_gap ({}) clear of the icon, got {gap}",
        driver.app().button.icon_gap,
    );
}
