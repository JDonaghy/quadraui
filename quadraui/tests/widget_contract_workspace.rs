#![cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos")
))]
//! Black-box driver tests for `Workspace` on the widget contract: the
//! workspace reaches the frame as a type-erased `Surface::Widget`, its
//! strip paints through its `TabBar` child's own `Primitive`, and the
//! workspace sits in the app's `FrameTree`, so a click on the strip routes
//! through the shared widget-contract engine and back out as the
//! `UiEvent::Workspace` intents this app applies (controlled mode).
//!
//! The same scenarios run on the TUI driver and on the pixel drivers
//! (GTK, macOS) the build enables.

use quadraui::primitives::tab_bar::{tab_part_center, TabBarSubPart};
use quadraui::testing::DriverInput;
use quadraui::{
    AppLogic, Backend, FrameTree, Reaction, Rect, ScreenLayout, Surface, UiEvent, WidgetId,
    Workspace, WorkspaceDoc, WorkspaceEvent,
};

fn ws_id() -> WidgetId {
    WidgetId::new("test:workspace")
}

/// Three documents, `alpha` active and `gamma` the preview.
fn workspace() -> Workspace {
    Workspace::new(ws_id())
        .with_docs(vec![
            WorkspaceDoc::new("doc:a", "alpha"),
            WorkspaceDoc::new("doc:b", "beta"),
            WorkspaceDoc::new("doc:c", "gamma"),
        ])
        .with_active(Some(0))
        .with_preview(Some("doc:c".to_string()))
}

/// A host that owns one workspace descriptor, paints it over the whole
/// viewport and applies every intent it reports.
struct WorkspaceApp {
    ws: Workspace,
    /// The rect the last frame painted into, which the frame tree routes
    /// against.
    painted: std::cell::Cell<Rect>,
    events: Vec<WorkspaceEvent>,
}

impl WorkspaceApp {
    fn new() -> Self {
        Self {
            ws: workspace(),
            painted: std::cell::Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            events: Vec::new(),
        }
    }

    fn area(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        Rect::new(0.0, 0.0, vp.width, vp.height)
    }

    fn apply(&mut self, event: &WorkspaceEvent) {
        match event {
            WorkspaceEvent::Activated { index, .. } => self.ws.active = Some(*index),
            WorkspaceEvent::Closed { index, .. } => {
                self.ws.docs.remove(*index);
                self.ws.active = self
                    .ws
                    .active
                    .filter(|_| !self.ws.docs.is_empty())
                    .map(|a| a.min(self.ws.docs.len() - 1));
            }
            WorkspaceEvent::Promoted { .. } => self.ws.preview = None,
            _ => {}
        }
    }
}

impl AppLogic for WorkspaceApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let area = Self::area(backend);
        self.painted.set(area);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: area,
            widget: Box::new(self.ws.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.ws.id, self.painted.get(), Box::new(self.ws.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::Workspace(id, ev) = event {
            assert_eq!(id, ws_id(), "intents carry the workspace's own id");
            self.apply(&ev);
            self.events.push(ev);
        }
        Reaction::Redraw
    }
}

/// Press and release the left button on `part` of the strip.
fn click<D: DriverInput>(driver: &mut D, part: TabBarSubPart) {
    let (x, y) = tab_part_center(&ws_id(), part)
        .unwrap_or_else(|| panic!("{part:?} painted in the workspace strip"));
    driver.mouse_down(x, y);
    driver.mouse_up(x, y);
}

fn activated(id: &str, index: usize) -> WorkspaceEvent {
    WorkspaceEvent::Activated {
        id: id.to_string(),
        index,
    }
}

/// Clicking a permanent tab's body reports one activation.
fn tab_click_activates<D: DriverInput>(driver: &mut D, app: impl Fn(&D) -> &WorkspaceApp) {
    click(driver, TabBarSubPart::Tab(1));
    assert_eq!(app(driver).events, vec![activated("doc:b", 1)]);
    assert_eq!(app(driver).ws.active, Some(1));
}

/// Clicking the preview tab activates it and then promotes it.
fn preview_click_activates_then_promotes<D: DriverInput>(
    driver: &mut D,
    app: impl Fn(&D) -> &WorkspaceApp,
) {
    click(driver, TabBarSubPart::Tab(2));
    assert_eq!(
        app(driver).events,
        vec![
            activated("doc:c", 2),
            WorkspaceEvent::Promoted {
                id: "doc:c".to_string()
            },
        ]
    );
    assert_eq!(app(driver).ws.preview, None);
}

/// Clicking a close glyph reports that document's close.
fn close_click_closes<D: DriverInput>(driver: &mut D, app: impl Fn(&D) -> &WorkspaceApp) {
    click(driver, TabBarSubPart::TabClose(1));
    assert_eq!(
        app(driver).events,
        vec![WorkspaceEvent::Closed {
            id: "doc:b".to_string(),
            index: 1,
        }]
    );
    assert_eq!(app(driver).ws.docs.len(), 2);
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{Key, Modifiers, MouseButton, NamedKey, Point};

    fn driver() -> TuiDriver<WorkspaceApp> {
        quadraui::interaction::reset_ambient_interaction();
        TuiDriver::new(WorkspaceApp::new(), 60, 8)
    }

    #[test]
    fn surface_widget_workspace_paints_every_tab_in_the_top_row() {
        let driver = driver();
        for label in ["alpha", "beta", "gamma"] {
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
    fn clicking_the_preview_tab_activates_then_promotes_it() {
        let mut driver = driver();
        preview_click_activates_then_promotes(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_a_close_glyph_reports_the_close() {
        let mut driver = driver();
        close_click_closes(&mut driver, |d| d.app());
    }

    #[test]
    fn a_middle_click_anywhere_on_a_tab_closes_it() {
        let mut driver = driver();
        let (x, y) = tab_part_center(&ws_id(), TabBarSubPart::Tab(0)).expect("tab 0 painted");
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Middle,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        });
        assert_eq!(
            driver.app().events,
            vec![WorkspaceEvent::Closed {
                id: "doc:a".to_string(),
                index: 0,
            }]
        );
    }

    #[test]
    fn ctrl_page_down_on_the_focused_strip_activates_the_next_document() {
        let mut driver = driver();
        // A click focuses the strip (and activates tab 1).
        click(&mut driver, TabBarSubPart::Tab(1));
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::PageDown),
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        });
        assert_eq!(
            driver.app().events,
            vec![activated("doc:b", 1), activated("doc:c", 2)]
        );
    }

    #[test]
    fn a_press_below_the_strip_is_the_hosts() {
        let mut driver = driver();
        driver.mouse_down(5.0, 4.0);
        driver.mouse_up(5.0, 4.0);
        assert!(driver.app().events.is_empty());
    }
}

#[cfg(feature = "gtk")]
mod gtk {
    use super::*;
    use quadraui::gtk::testing::GtkDriver;

    fn driver() -> GtkDriver<WorkspaceApp> {
        quadraui::interaction::reset_ambient_interaction();
        GtkDriver::new(WorkspaceApp::new(), 600, 200)
    }

    #[test]
    fn clicking_a_tab_reports_its_activation() {
        let mut driver = driver();
        tab_click_activates(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_the_preview_tab_activates_then_promotes_it() {
        let mut driver = driver();
        preview_click_activates_then_promotes(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_a_close_glyph_reports_the_close() {
        let mut driver = driver();
        close_click_closes(&mut driver, |d| d.app());
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod macos {
    use super::*;
    use quadraui::macos::testing::MacDriver;

    fn driver() -> MacDriver<WorkspaceApp> {
        quadraui::interaction::reset_ambient_interaction();
        MacDriver::new(WorkspaceApp::new(), 600, 200)
    }

    #[test]
    fn clicking_a_tab_reports_its_activation() {
        let mut driver = driver();
        tab_click_activates(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_the_preview_tab_activates_then_promotes_it() {
        let mut driver = driver();
        preview_click_activates_then_promotes(&mut driver, |d| d.app());
    }

    #[test]
    fn clicking_a_close_glyph_reports_the_close() {
        let mut driver = driver();
        close_click_closes(&mut driver, |d| d.app());
    }
}
