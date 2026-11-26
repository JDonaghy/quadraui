#![cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos")
))]
//! Black-box driver tests for `BottomPanel` on the widget contract: the
//! panel reaches the frame as a type-erased `Surface::Widget`, its tab
//! strip paints through its `TabBar` child's own `Primitive`, and the panel
//! sits in the app's `FrameTree`, so a click on the strip routes through
//! the shared widget-contract engine and back out as the
//! `UiEvent::BottomPanel` intents this app applies (controlled mode).
//!
//! The panel keeps the wire ids the bindings address it by: the strip is
//! `bottom-panel:tabs` and its maximise toggle `bp:maximise`.

use quadraui::primitive::painted_sub_center;
use quadraui::primitives::tab_bar::{tab_part_center, TabBarSubPart};
use quadraui::testing::DriverInput;
use quadraui::{
    AppLogic, Backend, BottomPanel, BottomPanelEvent, BottomPanelItem, FrameTree, Reaction, Rect,
    ScreenLayout, Surface, UiEvent, WidgetId,
};

fn panel_id() -> WidgetId {
    WidgetId::new("bottom-panel:tabs")
}

fn panel() -> BottomPanel {
    BottomPanel::new(
        vec![
            BottomPanelItem::new("bp:terminal", "TERMINAL"),
            BottomPanelItem::new("bp:output", "OUTPUT").with_closable(true),
            BottomPanelItem::new("bp:problems", "PROBLEMS")
                .with_closable(true)
                .with_badge(Some("3".to_string())),
        ],
        "bp:terminal",
    )
}

/// A host that owns one panel descriptor, paints it over the whole
/// viewport and applies every intent it reports.
struct PanelApp {
    panel: BottomPanel,
    /// The rect the last frame painted into, which the frame tree routes
    /// against.
    painted: std::cell::Cell<Rect>,
    events: Vec<BottomPanelEvent>,
}

impl PanelApp {
    fn new() -> Self {
        Self {
            panel: panel(),
            painted: std::cell::Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            events: Vec::new(),
        }
    }
}

impl AppLogic for PanelApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let area = Rect::new(0.0, 0.0, vp.width, vp.height);
        self.painted.set(area);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: area,
            widget: Box::new(self.panel.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(
            self.panel.id,
            self.painted.get(),
            Box::new(self.panel.clone()),
        );
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::BottomPanel(id, ev) = event {
            assert_eq!(id, panel_id(), "intents carry the panel's own id");
            match &ev {
                BottomPanelEvent::TabActivated(id) => self.panel.active_tab_id = id.clone(),
                BottomPanelEvent::TabClosed(id) => self.panel.tabs.retain(|t| &t.id != id),
                BottomPanelEvent::MaximiseToggled => self.panel.maximised = !self.panel.maximised,
                BottomPanelEvent::Resized(_) => {}
            }
            self.events.push(ev);
        }
        Reaction::Redraw
    }
}

fn click_at<D: DriverInput>(driver: &mut D, (x, y): (f32, f32)) {
    driver.mouse_down(x, y);
    driver.mouse_up(x, y);
}

fn click<D: DriverInput>(driver: &mut D, part: TabBarSubPart) {
    let at = tab_part_center(&panel_id(), part)
        .unwrap_or_else(|| panic!("{part:?} painted in the panel strip"));
    click_at(driver, at);
}

fn tab_click_activates<D: DriverInput>(driver: &mut D, app: impl Fn(&D) -> &PanelApp) {
    click(driver, TabBarSubPart::Tab(1));
    assert_eq!(
        app(driver).events,
        vec![BottomPanelEvent::TabActivated("bp:output".to_string())]
    );
    assert_eq!(app(driver).panel.active_tab_id, "bp:output");
}

fn close_click_closes<D: DriverInput>(driver: &mut D, app: impl Fn(&D) -> &PanelApp) {
    click(driver, TabBarSubPart::TabClose(2));
    assert_eq!(
        app(driver).events,
        vec![BottomPanelEvent::TabClosed("bp:problems".to_string())]
    );
}

fn maximise_click_toggles<D: DriverInput>(driver: &mut D, app: impl Fn(&D) -> &PanelApp) {
    let toggle = painted_sub_center(&panel_id(), TabBarSubPart::RightSegment(0).sub_target())
        .expect("the maximise toggle painted");
    click_at(driver, toggle);
    assert_eq!(app(driver).events, vec![BottomPanelEvent::MaximiseToggled]);
    assert!(app(driver).panel.maximised);
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;

    fn driver() -> TuiDriver<PanelApp> {
        quadraui::interaction::reset_ambient_interaction();
        TuiDriver::new(PanelApp::new(), 60, 8)
    }

    #[test]
    fn surface_widget_bottom_panel_paints_its_strip_in_the_top_row() {
        let driver = driver();
        for label in ["TERMINAL", "OUTPUT", "PROBLEMS (3)", "^"] {
            let b = driver
                .find_bounds(label)
                .unwrap_or_else(|| panic!("{label} on screen:\n{}", driver.screen()));
            assert_eq!(b.y, 0.0, "{label} sits in the strip row");
        }
    }

    #[test]
    fn clicking_a_tab_reports_its_activation() {
        let mut driver = driver();
        tab_click_activates(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_the_active_tab_reports_nothing() {
        let mut driver = driver();
        click(&mut driver, TabBarSubPart::Tab(0));
        assert!(driver.app().events.is_empty());
    }

    #[test]
    fn clicking_a_close_glyph_reports_the_close() {
        let mut driver = driver();
        close_click_closes(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_the_maximise_toggle_flips_its_glyph() {
        let mut driver = driver();
        maximise_click_toggles(&mut driver, |d| d.app());
        assert!(driver.find_bounds("v").is_some(), "{}", driver.screen());
    }

    #[test]
    fn a_press_in_the_content_area_is_the_hosts() {
        let mut driver = driver();
        click_at(&mut driver, (5.0, 4.0));
        assert!(driver.app().events.is_empty());
    }
}

#[cfg(feature = "gtk")]
mod gtk {
    use super::*;
    use quadraui::gtk::testing::GtkDriver;

    fn driver() -> GtkDriver<PanelApp> {
        quadraui::interaction::reset_ambient_interaction();
        GtkDriver::new(PanelApp::new(), 700, 200)
    }

    #[test]
    fn clicking_a_tab_reports_its_activation() {
        let mut driver = driver();
        tab_click_activates(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_a_close_glyph_reports_the_close() {
        let mut driver = driver();
        close_click_closes(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_the_maximise_toggle_toggles() {
        let mut driver = driver();
        maximise_click_toggles(&mut driver, |d| d.app());
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod macos {
    use super::*;
    use quadraui::macos::testing::MacDriver;

    fn driver() -> MacDriver<PanelApp> {
        quadraui::interaction::reset_ambient_interaction();
        MacDriver::new(PanelApp::new(), 700, 200)
    }

    #[test]
    fn clicking_a_tab_reports_its_activation() {
        let mut driver = driver();
        tab_click_activates(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_a_close_glyph_reports_the_close() {
        let mut driver = driver();
        close_click_closes(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_the_maximise_toggle_toggles() {
        let mut driver = driver();
        maximise_click_toggles(&mut driver, |d| d.app());
    }
}
