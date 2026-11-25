#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Button` on the widget contract: the
//! button reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so every gesture routes through
//! the shared widget-contract engine — hit test, click-to-focus,
//! `Interactive::handle` — and comes back out as a
//! `UiEvent::ButtonActivated` this app records.
//!
//! This is the runtime leg of the slice's three-way event-parity test.
//! The direct-`Interactive::handle` leg (an out-of-process binding
//! host's dispatch shape) and the `a11y::route_widget_action` leg (the
//! assistive-technology dispatch path) live in
//! `src/primitives/button.rs`'s own `contract_tests` module, and all
//! three must resolve the same gesture to the same `UiEvent`.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Button, FrameTree, NamedKey, Reaction, Rect, ScreenLayout, Surface, UiEvent,
    WidgetId,
};

const COLS: u16 = 20;
const ROWS: u16 = 3;
/// Wide enough for `[OK]`'s bracket chrome plus a cell of slack.
const AREA: Rect = Rect::new(2.0, 1.0, 6.0, 1.0);

fn button_id() -> WidgetId {
    WidgetId::new("dialog:ok")
}

/// One button, drawn and routed as a `Surface::Widget`. `activations`
/// records every `UiEvent::ButtonActivated` the app has seen.
struct ButtonApp {
    button: Button,
    activations: Vec<WidgetId>,
}

impl ButtonApp {
    fn new() -> Self {
        Self {
            button: Button::new(button_id(), "OK"),
            activations: Vec::new(),
        }
    }
}

impl AppLogic for ButtonApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.button.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.button.id, AREA, Box::new(self.button.clone()));
        tree
    }

    /// Makes the button reachable by Tab before any click has focused
    /// it — the same seam [`quadraui::AppLogic::extra_tab_stops`]'s own
    /// doc describes a host using to feed a widget-contract widget into
    /// Tab/Shift+Tab cycling. A `Button` is a single [`crate::interactive::FocusSpec::Stop`]
    /// (no children), so — unlike `RadioGroup`'s sub-targeted stops —
    /// this flat `(id, None)` address stays valid across the engine's
    /// per-event `FocusTree::sync`.
    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        vec![(self.button.id, AREA)]
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::ButtonActivated { widget } = event {
            self.activations.push(widget);
        }
        Reaction::Continue
    }
}

#[test]
fn surface_widget_button_paints_its_bracketed_label() {
    let driver = TuiDriver::new(ButtonApp::new(), COLS, ROWS);
    assert!(driver.screen_contains("OK"), "{}", driver.screen());
    assert!(driver.screen_contains("["), "{}", driver.screen());
    assert!(driver.screen_contains("]"), "{}", driver.screen());
}

#[test]
fn clicking_the_button_activates_it() {
    let mut driver = TuiDriver::new(ButtonApp::new(), COLS, ROWS);
    driver.set_double_click_folding(false);

    driver.click(AREA.x + 1.0, AREA.y);
    assert_eq!(driver.app().activations, vec![button_id()]);

    driver.click(AREA.x + 1.0, AREA.y);
    assert_eq!(
        driver.app().activations,
        vec![button_id(), button_id()],
        "every click activates again, there's no toggle state"
    );
}

#[test]
fn tab_then_enter_activates_the_focused_button() {
    let mut driver = TuiDriver::new(ButtonApp::new(), COLS, ROWS);

    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);

    assert_eq!(driver.app().activations, vec![button_id()]);
}

#[test]
fn space_also_activates_the_focused_button() {
    let mut driver = TuiDriver::new(ButtonApp::new(), COLS, ROWS);

    driver.press_named(NamedKey::Tab);
    driver.press(quadraui::Key::Char(' '));

    assert_eq!(driver.app().activations, vec![button_id()]);
}

#[test]
fn a_disabled_button_ignores_clicks_and_keys() {
    let mut app = ButtonApp::new();
    app.button.disabled = true;
    let mut driver = TuiDriver::new(app, COLS, ROWS);

    driver.click(AREA.x + 1.0, AREA.y);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Enter);

    assert!(driver.app().activations.is_empty());
}
