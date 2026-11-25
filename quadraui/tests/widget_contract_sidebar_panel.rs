#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `SidebarPanel` on the widget contract:
//! the panel reaches the frame as a type-erased `Surface::Widget`, its
//! header is painted through `DynWidget::paint` → `Primitive::paint_cells`,
//! and the panel is registered in the app's `FrameTree` so a click on a
//! header button routes through the shared widget-contract engine (hit
//! test, `Interactive::handle`) and back out as the
//! `UiEvent::Toolbar(toolbar_id, ButtonClicked { .. })` this app records.
//!
//! This is the runtime leg of the three-way event-parity check. The
//! assistive-technology leg (`a11y::route_widget_action`, crate-private)
//! lives in `src/primitives/sidebar_panel.rs`'s own tests, and the
//! binding-host leg (`quadraui-binding-core`'s `InteractionEngine`
//! painting a `"sidebar_panel"` layout node) in that crate's engine
//! tests; all three resolve a click on a header button to the same
//! `UiEvent::Toolbar(toolbar_id, ButtonClicked { .. })`.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, Reaction, Rect, ScreenLayout, SidebarPanel, Surface, Toolbar,
    ToolbarButton, ToolbarEvent, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 10;
/// A non-zero origin, so the panel-local translation of a click is
/// actually exercised.
const AREA: Rect = Rect::new(2.0, 1.0, 30.0, 8.0);

fn panel_id() -> WidgetId {
    WidgetId::new("sidebar:explorer")
}

fn toolbar_id() -> WidgetId {
    WidgetId::new("sidebar:explorer:toolbar")
}

fn action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
    ToolbarButton::Action {
        spec: None,
        id: WidgetId::new(id),
        label: label.to_string(),
        icon: None,
        key_hint: None,
        enabled,
        is_active: false,
        tooltip: String::new(),
    }
}

fn panel() -> SidebarPanel {
    SidebarPanel {
        id: panel_id(),
        toolbar: Some(Toolbar::new(toolbar_id()).with_buttons(vec![
            action("refresh", "Refresh", true),
            action("collapse", "Collapse", true),
            action("off", "Off", false),
        ])),
        toolbar_height: None,
    }
}

fn clicked(id: &str) -> UiEvent {
    UiEvent::Toolbar(
        toolbar_id(),
        ToolbarEvent::ButtonClicked {
            id: WidgetId::new(id),
            is_active: true,
        },
    )
}

/// One sidebar panel routed through the generic `Interactive`/`FrameTree`
/// engine, plus a log of every event the app saw.
struct SidebarPanelApp {
    panel: SidebarPanel,
    events: Vec<UiEvent>,
}

impl SidebarPanelApp {
    fn new() -> Self {
        Self {
            panel: panel(),
            events: Vec::new(),
        }
    }

    fn toolbar_events(&self) -> Vec<&UiEvent> {
        self.events
            .iter()
            .filter(|e| matches!(e, UiEvent::Toolbar(..)))
            .collect()
    }
}

impl AppLogic for SidebarPanelApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.panel.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.panel.id, AREA, Box::new(self.panel.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Redraw
    }
}

/// Press and release the left button on the first cell of `label`'s
/// painted run.
fn click_label(driver: &mut TuiDriver<SidebarPanelApp>, label: &str) {
    let bounds = driver
        .find_bounds(label)
        .unwrap_or_else(|| panic!("{label} on screen:\n{}", driver.screen()));
    driver.mouse_down(bounds.x + 0.5, bounds.y);
    driver.mouse_up(bounds.x + 0.5, bounds.y);
}

#[test]
fn surface_widget_sidebar_panel_paints_its_header_in_the_top_row() {
    let driver = TuiDriver::new(SidebarPanelApp::new(), COLS, ROWS);
    for label in ["Refresh", "Collapse", "Off"] {
        let b = driver
            .find_bounds(label)
            .unwrap_or_else(|| panic!("{label} on screen:\n{}", driver.screen()));
        assert_eq!(b.y, AREA.y, "{label} sits in the header row");
        assert!(b.x >= AREA.x, "{label} sits inside the panel");
    }
}

#[test]
fn clicking_a_header_button_reports_the_header_toolbars_event() {
    let mut driver = TuiDriver::new(SidebarPanelApp::new(), COLS, ROWS);

    click_label(&mut driver, "Collapse");

    assert_eq!(driver.app().toolbar_events(), vec![&clicked("collapse")]);
}

#[test]
fn clicking_a_disabled_header_button_reports_nothing() {
    let mut driver = TuiDriver::new(SidebarPanelApp::new(), COLS, ROWS);

    click_label(&mut driver, "Off");

    assert!(driver.app().toolbar_events().is_empty());
}

/// A press in the content area is the host's: the panel doesn't claim
/// it, so the plain mouse event bubbles to the app.
#[test]
fn a_press_in_the_content_area_bubbles_to_the_app() {
    let mut driver = TuiDriver::new(SidebarPanelApp::new(), COLS, ROWS);

    let (x, y) = (AREA.x + 3.0, AREA.y + 4.0);
    driver.mouse_down(x, y);
    driver.mouse_up(x, y);

    assert!(driver.app().toolbar_events().is_empty());
    assert!(
        driver.app().events.iter().any(|e| matches!(
            e,
            UiEvent::MouseDown { position, .. } if position.y == y
        )),
        "the content-area press must bubble: {:?}",
        driver.app().events
    );
}

/// The runtime leg of the three-way event parity: a mouse click on the
/// first enabled header button resolves to exactly the event the
/// a11y and binding-host legs assert.
#[test]
fn a_mouse_click_on_the_first_header_button_matches_the_parity_event() {
    let mut driver = TuiDriver::new(SidebarPanelApp::new(), COLS, ROWS);
    click_label(&mut driver, "Refresh");
    assert_eq!(driver.app().toolbar_events(), vec![&clicked("refresh")]);
}
