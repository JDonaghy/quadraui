#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Switch` on the widget contract: the
//! switch reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so every gesture routes through
//! the shared widget-contract engine — hit test, click-to-focus,
//! `Interactive::handle` — and comes back out as a `UiEvent::SwitchToggled`
//! this app applies to its own `Switch::value`.
//!
//! This is the runtime leg of the slice's three-way event-parity test.
//! The direct-`Interactive::handle` leg (an out-of-process binding host's
//! dispatch shape) and the `a11y::route_widget_action` leg (the
//! assistive-technology dispatch path) live in
//! `src/primitives/switch.rs`'s own `contract_tests` module, and all
//! three must resolve the same gesture to the same `UiEvent`.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, NamedKey, Reaction, Rect, ScreenLayout, Surface, Switch, UiEvent,
    WidgetId,
};

const COLS: u16 = 20;
const ROWS: u16 = 3;
/// The switch's own rect — four cells wide, one tall, same shape its
/// `measure()` reports in cell units.
const AREA: Rect = Rect::new(2.0, 1.0, 4.0, 1.0);

fn switch_id() -> WidgetId {
    WidgetId::new("settings:wrap")
}

/// One switch, drawn and routed as a `Surface::Widget`. `toggles`
/// records every `UiEvent::SwitchToggled` value the app has seen, and
/// the app applies it onto `switch.value` the way a real host would.
struct SwitchApp {
    switch: Switch,
    toggles: Vec<bool>,
}

impl SwitchApp {
    fn new() -> Self {
        Self {
            switch: Switch::new(switch_id()),
            toggles: Vec::new(),
        }
    }
}

impl AppLogic for SwitchApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.switch.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.switch.id, AREA, Box::new(self.switch.clone()));
        tree
    }

    /// Makes the switch reachable by Tab before any click has focused
    /// it — the same seam [`quadraui::AppLogic::extra_tab_stops`]'s own
    /// doc describes a host using to feed a widget-contract widget into
    /// Tab/Shift+Tab cycling.
    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        vec![(self.switch.id, AREA)]
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::SwitchToggled { widget, value } = event {
            assert_eq!(widget, switch_id(), "the switch's own id rides the event");
            self.toggles.push(value);
            self.switch.value = value;
        }
        Reaction::Continue
    }
}

/// The four cells the switch's own track occupies, as painted.
fn track(driver: &TuiDriver<SwitchApp>) -> String {
    driver
        .screen()
        .lines()
        .nth(AREA.y as usize)
        .map(|line| {
            line.chars()
                .skip(AREA.x as usize)
                .take(AREA.width as usize)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn surface_widget_switch_paints_a_bracketed_track_off_by_default() {
    let driver = TuiDriver::new(SwitchApp::new(), COLS, ROWS);
    let painted = track(&driver);
    assert!(
        painted.starts_with('['),
        "track opens with a bracket: {painted:?}\n{}",
        driver.screen()
    );
    assert!(
        painted.ends_with(']'),
        "track closes with a bracket: {painted:?}\n{}",
        driver.screen()
    );
    assert!(
        painted.contains('●'),
        "the knob glyph is painted somewhere in the track: {painted:?}"
    );
}

#[test]
fn clicking_the_track_toggles_the_switch_on_then_off() {
    let mut driver = TuiDriver::new(SwitchApp::new(), COLS, ROWS);
    // Two single clicks at the same spot, not a double click.
    driver.set_double_click_folding(false);

    driver.click(AREA.x + 1.0, AREA.y);
    assert_eq!(driver.app().toggles, vec![true]);
    assert!(driver.app().switch.value);

    driver.click(AREA.x + 1.0, AREA.y);
    assert_eq!(driver.app().toggles, vec![true, false]);
    assert!(!driver.app().switch.value);
}

#[test]
fn tab_then_enter_toggles_the_focused_switch() {
    let mut driver = TuiDriver::new(SwitchApp::new(), COLS, ROWS);

    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);

    assert_eq!(driver.app().toggles, vec![true]);
}

#[test]
fn space_also_toggles_the_focused_switch() {
    let mut driver = TuiDriver::new(SwitchApp::new(), COLS, ROWS);

    driver.press_named(NamedKey::Tab);
    driver.press(quadraui::Key::Char(' '));

    assert_eq!(driver.app().toggles, vec![true]);
}

#[test]
fn a_disabled_switch_ignores_clicks_and_keys() {
    let mut app = SwitchApp::new();
    app.switch.disabled = true;
    let mut driver = TuiDriver::new(app, COLS, ROWS);

    driver.click(AREA.x + 1.0, AREA.y);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);

    assert!(driver.app().toggles.is_empty());
}
