#![cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos")
))]
//! Black-box driver tests for `ShellView` on the widget contract: the
//! shell reaches the frame as a type-erased `Surface::Widget`, its
//! activity bar and sidebar header paint through their own `Primitive`s,
//! and the shell sits in the app's `FrameTree`, so a click on an activity
//! row or a drag on the sidebar divider routes through the shared
//! widget-contract engine and back out as the `UiEvent::AppShell` intents
//! this app applies (controlled mode).
//!
//! `draw_dyn` publishes the shell's named sub-regions as frame-inventory
//! zones under `app-shell:<region>`, so its chrome stays observable to a
//! driver even though the shell paints its children itself; the zone set
//! agrees across every driver the build enables.

use std::collections::BTreeSet;

use quadraui::primitive::painted_sub_center;
use quadraui::testing::{ConformanceDriver, DriverInput, LogicalViewport};
use quadraui::{
    AppLogic, AppShellEvent, Backend, FrameTree, PanelDefinition, Reaction, Rect, ScreenLayout,
    ShellRegion, ShellView, Surface, UiEvent, WidgetId,
};

fn shell_id() -> WidgetId {
    WidgetId::new("app-shell")
}

fn panel(id: &str, icon: &str, title: &str) -> PanelDefinition {
    PanelDefinition {
        id: WidgetId::new(id),
        icon: icon.to_string(),
        tooltip: title.to_string(),
        title: title.to_uppercase(),
    }
}

fn shell() -> ShellView {
    let mut shell = ShellView::new(
        vec![
            panel("panel:explorer", "E", "Explorer"),
            panel("panel:search", "S", "Search"),
        ],
        18.0,
    );
    shell.has_status_bar = true;
    shell
}

/// A host that owns one shell descriptor, paints it over the whole
/// viewport and applies every intent it reports.
struct ShellApp {
    shell: ShellView,
    /// The rect the last frame painted into, which the frame tree routes
    /// against.
    painted: std::cell::Cell<Rect>,
    events: Vec<AppShellEvent>,
}

impl ShellApp {
    fn new() -> Self {
        Self {
            shell: shell(),
            painted: std::cell::Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            events: Vec::new(),
        }
    }
}

impl AppLogic for ShellApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let area = Rect::new(0.0, 0.0, vp.width, vp.height);
        self.painted.set(area);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: area,
            widget: Box::new(self.shell.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(
            self.shell.id,
            self.painted.get(),
            Box::new(self.shell.clone()),
        );
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::AppShell(id, ev) = event {
            assert_eq!(id, shell_id(), "intents carry the shell's own id");
            match &ev {
                AppShellEvent::PanelChanged { panel_id } => {
                    self.shell.active_panel =
                        self.shell.panels.iter().position(|p| p.id == *panel_id);
                    self.shell.sidebar_visible = true;
                }
                AppShellEvent::SidebarHidden => self.shell.sidebar_visible = false,
                AppShellEvent::SidebarResized { .. } => {}
                _ => {}
            }
            self.events.push(ev);
        }
        Reaction::Redraw
    }
}

/// The `app-shell:*` zones a driver's frame inventory reports.
fn shell_zones<D: ConformanceDriver<App = ShellApp>>() -> BTreeSet<String> {
    quadraui::interaction::reset_ambient_interaction();
    D::new_fixture(ShellApp::new(), LogicalViewport::new(80, 24))
        .inventory()
        .zones()
        .iter()
        .map(|z| z.id.as_str().to_string())
        .filter(|id| id.starts_with("app-shell:"))
        .collect()
}

fn expected_zones() -> BTreeSet<String> {
    [
        "window",
        "activity-bar",
        "sidebar-header",
        "sidebar-content",
        "divider",
        "main-content",
        "status-bar",
    ]
    .into_iter()
    .map(|name| format!("app-shell:{name}"))
    .collect()
}

fn region_center(region: ShellRegion) -> (f32, f32) {
    painted_sub_center(&shell_id(), region.sub_target())
        .unwrap_or_else(|| panic!("{region:?} painted"))
}

fn click_at<D: DriverInput>(driver: &mut D, (x, y): (f32, f32)) {
    driver.mouse_down(x, y);
    driver.mouse_up(x, y);
}

/// Clicking another panel's activity row switches to it; clicking the
/// active panel's row again hides the sidebar.
fn activity_clicks_switch_then_hide<D: DriverInput>(driver: &mut D, app: impl Fn(&D) -> &ShellApp) {
    click_at(driver, region_center(ShellRegion::ActivityItem(1)));
    assert_eq!(
        app(driver).events,
        vec![AppShellEvent::PanelChanged {
            panel_id: WidgetId::new("panel:search")
        }]
    );
    click_at(driver, region_center(ShellRegion::ActivityItem(1)));
    assert_eq!(
        app(driver).events.last(),
        Some(&AppShellEvent::SidebarHidden)
    );
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;

    fn driver() -> TuiDriver<ShellApp> {
        quadraui::interaction::reset_ambient_interaction();
        TuiDriver::new(ShellApp::new(), 80, 24)
    }

    #[test]
    fn draw_dyn_publishes_the_shell_chrome_as_zones() {
        assert_eq!(shell_zones::<TuiDriver<_>>(), expected_zones());
    }

    #[test]
    fn the_sidebar_header_is_titled_from_the_active_panel() {
        let driver = driver();
        let header = driver
            .find_bounds("EXPLORER")
            .unwrap_or_else(|| panic!("header on screen:\n{}", driver.screen()));
        assert_eq!(header.y, 0.0, "the header sits in the top row");
    }

    #[test]
    fn activity_clicks_switch_the_panel_then_hide_the_sidebar() {
        let mut driver = driver();
        activity_clicks_switch_then_hide(&mut driver, |d| d.app());
        assert!(
            driver.find_bounds("SEARCH").is_none(),
            "the hidden sidebar paints no header:\n{}",
            driver.screen()
        );
    }

    #[test]
    fn dragging_the_divider_reports_a_sidebar_resize() {
        let mut driver = driver();
        let (x, y) = region_center(ShellRegion::Divider);
        driver.drag(x, y, x + 10.0, y);
        assert!(
            driver
                .app()
                .events
                .iter()
                .any(|e| matches!(e, AppShellEvent::SidebarResized { .. })),
            "{:?}",
            driver.app().events
        );
    }

    #[test]
    fn a_press_in_the_main_content_is_the_hosts() {
        let mut driver = driver();
        click_at(&mut driver, region_center(ShellRegion::MainContent));
        assert!(driver.app().events.is_empty());
    }
}

#[cfg(feature = "gtk")]
mod gtk {
    use super::*;
    use quadraui::gtk::testing::GtkDriver;

    #[test]
    fn draw_dyn_publishes_the_shell_chrome_as_zones() {
        assert_eq!(shell_zones::<GtkDriver<_>>(), expected_zones());
    }

    #[test]
    fn activity_clicks_switch_the_panel_then_hide_the_sidebar() {
        quadraui::interaction::reset_ambient_interaction();
        let mut driver = GtkDriver::new(ShellApp::new(), 900, 600);
        activity_clicks_switch_then_hide(&mut driver, |d| d.app());
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod macos {
    use super::*;
    use quadraui::macos::testing::MacDriver;

    #[test]
    fn draw_dyn_publishes_the_shell_chrome_as_zones() {
        assert_eq!(shell_zones::<MacDriver<_>>(), expected_zones());
    }

    #[test]
    fn activity_clicks_switch_the_panel_then_hide_the_sidebar() {
        quadraui::interaction::reset_ambient_interaction();
        let mut driver = MacDriver::new(ShellApp::new(), 900, 600);
        activity_clicks_switch_then_hide(&mut driver, |d| d.app());
    }
}
