#![cfg(feature = "tui")]
//! Headless `TuiDriver` test for `ListView`'s own `Interactive` impl:
//! keyboard navigation, Enter-to-activate, and single-click/modified-click
//! selection, all driven through the real `event -> handle -> render`
//! path — `UiEvent` into `quadraui::runtime::preprocess_event`'s
//! widget-contract routing (hit test, click-to-focus,
//! `Interactive::handle`) and back out as a `UiEvent::List` this test's
//! own `AppLogic::handle` applies onto the owned `ListView`, then the
//! next `render` paints it — never a direct call into
//! `ListView::handle`/`Primitive::layout` that would skip the routing
//! engine entirely.
//!
//! Double-click isn't covered here: `route_widget_event`'s own
//! `UiEvent` match has no `DoubleClick` arm yet, so no `PointerKind`
//! this routing engine produces ever carries
//! [`quadraui::interactive::PointerKind::DoubleClick`] — see
//! `primitives::list`'s own unit tests for `ListView::handle`'s
//! double-click arm exercised directly against a `RoutedEvent`.
//!
//! This test routes `ListView` through `Backend::draw_primitive`, the
//! same generic path every widget goes through — proving the paint half
//! and the behaviour half agree on one `ListView`.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, DrawsPrimitives, FrameTree, ListItem, ListView, ListViewEvent, Modifiers,
    NamedKey, Reaction, Rect, StyledText, UiEvent, WidgetId,
};

const LIST_RECT: Rect = Rect::new(0.0, 0.0, 40.0, 10.0);

/// Minimal `AppLogic` wiring one `ListView` through the generic
/// `Interactive`/`FrameTree` routing engine — used only by this test.
struct ListDriverApp {
    id: WidgetId,
    list: ListView,
    selected: Vec<usize>,
    activated: Vec<usize>,
}

impl ListDriverApp {
    fn new() -> Self {
        let id = WidgetId::new("quickfix");
        let list = ListView {
            id,
            title: None,
            items: ["Alpha", "Beta", "Gamma", "Delta"]
                .iter()
                .map(|s| ListItem {
                    text: StyledText::plain(*s),
                    icon: None,
                    detail: None,
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            items_window: None,
            scroll_px: None,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        Self {
            id,
            list,
            selected: Vec::new(),
            activated: Vec::new(),
        }
    }
}

impl AppLogic for ListDriverApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_primitive(LIST_RECT, &self.list);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.id, LIST_RECT, Box::new(self.list.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::List(id, list_event) = &event else {
            return Reaction::Continue;
        };
        if *id != self.id {
            return Reaction::Continue;
        }
        self.list.has_focus = true;
        match list_event {
            ListViewEvent::SelectionChanged { idx } => {
                self.list.selected_idx = *idx;
                self.selected = vec![*idx];
            }
            ListViewEvent::SelectionSet { indices } => {
                self.selected = indices.clone();
                if let Some(&idx) = indices.first() {
                    self.list.selected_idx = idx;
                }
            }
            ListViewEvent::ItemActivated { idx } => self.activated.push(*idx),
            ListViewEvent::ItemDoubleClicked { .. } | ListViewEvent::KeyPressed { .. } => {}
        }
        Reaction::Redraw
    }
}

#[test]
fn renders_every_item() {
    let driver = TuiDriver::new(ListDriverApp::new(), 60, 15);
    assert!(driver.screen_contains("Alpha"));
    assert!(driver.screen_contains("Gamma"));
}

#[test]
fn arrow_keys_move_the_selection_through_the_real_routing_engine() {
    let mut driver = TuiDriver::new(ListDriverApp::new(), 60, 15);

    // Click row 0 first — `ListView::focus` is `FocusSpec::Stop`, so
    // nothing routes a key to it until the click-to-focus step runs.
    let (x, y) = driver.find("Alpha").expect("row 0 visible");
    driver.click(x, y);
    assert_eq!(driver.app().list.selected_idx, 0);

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().list.selected_idx, 1);
    assert_eq!(driver.app().selected, vec![1]);

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().list.selected_idx, 2);

    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().list.selected_idx, 1);
}

#[test]
fn home_and_end_jump_to_either_edge() {
    let mut driver = TuiDriver::new(ListDriverApp::new(), 60, 15);
    let (x, y) = driver.find("Beta").expect("row 1 visible");
    driver.click(x, y);
    assert_eq!(driver.app().list.selected_idx, 1);

    driver.press_named(NamedKey::End);
    assert_eq!(driver.app().list.selected_idx, 3);

    driver.press_named(NamedKey::Home);
    assert_eq!(driver.app().list.selected_idx, 0);
}

#[test]
fn enter_activates_the_current_selection() {
    let mut driver = TuiDriver::new(ListDriverApp::new(), 60, 15);
    let (x, y) = driver.find("Gamma").expect("row 2 visible");
    driver.click(x, y);

    driver.press_named(NamedKey::Enter);
    assert_eq!(driver.app().activated, vec![2]);
}

#[test]
fn shift_arrow_extends_the_selection_into_a_selection_set() {
    let mut driver = TuiDriver::new(ListDriverApp::new(), 60, 15);
    let (x, y) = driver.find("Alpha").expect("row 0 visible");
    driver.click(x, y);

    driver.dispatch(UiEvent::KeyPressed {
        key: quadraui::Key::Named(NamedKey::Down),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
    });

    assert_eq!(driver.app().selected, vec![0, 1]);
}

#[test]
fn ctrl_click_toggles_a_row_into_the_selection() {
    let mut driver = TuiDriver::new(ListDriverApp::new(), 60, 15);
    let (x0, y0) = driver.find("Alpha").expect("row 0 visible");
    driver.click(x0, y0);
    assert_eq!(driver.app().selected, vec![0]);

    let (x1, y1) = driver.find("Gamma").expect("row 2 visible");
    driver.click_with(
        quadraui::MouseButton::Left,
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        },
        x1,
        y1,
    );
    assert_eq!(driver.app().selected, vec![0, 2]);
}
