//! Pointer presses on rows and buttons, driven through the real backend
//! input path (`TuiDriver`, and `GtkDriver` for the pixel backend) so the
//! shared `DoubleClickDetector` sees every press the way it does live:
//!
//! - two quick clicks on a `Button` activate it twice;
//! - two quick right-clicks on a row are two context-menu requests, never a
//!   double-click;
//! - a middle press on a row reports its button;
//! - `SelectionMode::Multi` turns Ctrl/Cmd and Shift clicks and the
//!   keyboard chords into `SelectionSet`, and `Single` does not.
#![cfg(feature = "tui")]

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Button, Column, DataRow, DataTable, DataTableEvent, Decoration,
    DrawsPrimitives, FrameTree, ListItem, ListView, ListViewEvent, Modifiers, MouseButton,
    Reaction, Rect, SelectionMode, StyledText, TreeEvent, TreeRow, TreeStyle, TreeView, UiEvent,
    WidgetId,
};

const W: u16 = 40;
const H: u16 = 10;

fn table(mode: SelectionMode) -> DataTable {
    DataTable {
        id: WidgetId::new("table"),
        columns: vec![Column {
            title: "Name".to_string(),
            width: Default::default(),
            align: Default::default(),
        }],
        rows: (0..5)
            .map(|i| DataRow {
                cells: vec![StyledText::plain(format!("row{i}"))],
                decoration: Decoration::Normal,
            })
            .collect(),
        selected_idx: None,
        selection_mode: mode,
        selected_indices: Vec::new(),
        scroll_offset: 0,
        sort: None,
        has_focus: true,
        show_scrollbar: false,
        min_total_width: None,
        h_scroll: 0.0,
        column_overrides: vec![],
        footer: None,
        bordered: false,
        striped: false,
        glyphs: None,
    }
}

fn list() -> ListView {
    ListView {
        id: WidgetId::new("list"),
        title: None,
        items: (0..4)
            .map(|i| ListItem {
                text: StyledText::plain(format!("item{i}")),
                icon: None,
                detail: None,
                decoration: Decoration::Normal,
            })
            .collect(),
        selected_idx: 0,
        scroll_offset: 0,
        has_focus: true,
        bordered: false,
        h_scroll: 0,
        max_content_width: None,
        show_v_scrollbar: false,
        striped: false,
    }
}

fn tree() -> TreeView {
    TreeView {
        id: WidgetId::new("tree"),
        rows: (0..3u16)
            .map(|i| TreeRow {
                path: vec![i],
                indent: 0,
                icon: None,
                text: StyledText::plain(format!("node{i}")),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            })
            .collect(),
        selection_mode: SelectionMode::Single,
        selected_path: None,
        scroll_offset: 0,
        style: TreeStyle::default(),
        has_focus: true,
    }
}

enum Widget {
    Button(Button),
    Table(DataTable),
    List(ListView),
    Tree(TreeView),
}

/// Paints one widget across the viewport and records every event the
/// routing engine hands back. A `DataTable` echoes `RowSelected` /
/// `SelectionSet`, a `ListView` and a `TreeView` their `SelectionChanged`,
/// into the descriptor, the way an app keeps it controlled.
struct WidgetApp {
    widget: Widget,
    rect: Rect,
    events: Vec<UiEvent>,
}

impl WidgetApp {
    fn new(widget: Widget, width: f32, height: f32) -> Self {
        Self {
            widget,
            rect: Rect::new(0.0, 0.0, width, height),
            events: Vec::new(),
        }
    }

    fn table_events(&self) -> Vec<DataTableEvent> {
        self.events
            .iter()
            .filter_map(|e| match e {
                UiEvent::DataTable(_, ev) => Some(ev.clone()),
                _ => None,
            })
            .collect()
    }
}

impl AppLogic for WidgetApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let rect = self.rect;
        match &self.widget {
            Widget::Button(b) => {
                backend.draw_primitive(rect, b);
            }
            Widget::Table(t) => {
                backend.draw_primitive(rect, t);
            }
            Widget::List(l) => {
                backend.draw_primitive(rect, l);
            }
            Widget::Tree(t) => {
                backend.draw_primitive(rect, t);
            }
        }
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        match &self.widget {
            Widget::Button(b) => tree.push(b.id, self.rect, Box::new(b.clone())),
            Widget::Table(t) => tree.push(t.id, self.rect, Box::new(t.clone())),
            Widget::List(l) => tree.push(l.id, self.rect, Box::new(l.clone())),
            Widget::Tree(t) => tree.push(t.id, self.rect, Box::new(t.clone())),
        }
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let (Widget::Table(t), UiEvent::DataTable(_, ev)) = (&mut self.widget, &event) {
            match ev {
                DataTableEvent::RowSelected { idx } => {
                    t.selected_idx = Some(*idx);
                    t.selected_indices = vec![*idx];
                }
                DataTableEvent::SelectionSet { indices } => {
                    t.selected_indices = indices.clone();
                }
                _ => {}
            }
        }
        match (&mut self.widget, &event) {
            (Widget::List(l), UiEvent::List(_, ListViewEvent::SelectionChanged { idx })) => {
                l.selected_idx = *idx;
            }
            (Widget::Tree(t), UiEvent::Tree(_, TreeEvent::SelectionChanged { path })) => {
                t.selected_path = Some(path.clone());
            }
            _ => {}
        }
        self.events.push(event);
        Reaction::Redraw
    }
}

fn tui(widget: Widget) -> TuiDriver<WidgetApp> {
    let mut driver = TuiDriver::new(WidgetApp::new(widget, W as f32, H as f32), W, H);
    driver.render();
    driver
}

fn at(driver: &TuiDriver<WidgetApp>, label: &str) -> (f32, f32) {
    driver
        .find(label)
        .unwrap_or_else(|| panic!("{label:?} not on screen:\n{}", driver.screen()))
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Default::default()
    }
}

fn ctrl() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Default::default()
    }
}

#[test]
fn two_quick_clicks_on_a_button_activate_it_twice() {
    let mut driver = tui(Widget::Button(Button::new(WidgetId::new("plus"), "+")));
    let (x, y) = at(&driver, "+");
    driver.click(x, y);
    driver.click(x, y);
    let activations = driver
        .app()
        .events
        .iter()
        .filter(|e| matches!(e, UiEvent::ButtonActivated { .. }))
        .count();
    assert_eq!(activations, 2, "{:?}", driver.app().events);
}

#[test]
fn two_quick_right_clicks_on_a_table_row_are_two_context_menu_requests() {
    let mut driver = tui(Widget::Table(table(SelectionMode::Single)));
    let (x, y) = at(&driver, "row2");
    driver.right_click(x, y);
    driver.right_click(x, y);
    let events = driver.app().table_events();
    let menus = events
        .iter()
        .filter(|e| matches!(e, DataTableEvent::ContextMenuRequested { idx: 2, .. }))
        .count();
    assert_eq!(menus, 2, "{events:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DataTableEvent::RowDoubleClicked { .. })),
        "{events:?}"
    );
}

#[test]
fn a_left_double_click_on_a_table_row_still_double_clicks() {
    let mut driver = tui(Widget::Table(table(SelectionMode::Single)));
    let (x, y) = at(&driver, "row1");
    driver.click(x, y);
    driver.click(x, y);
    assert!(
        driver
            .app()
            .table_events()
            .contains(&DataTableEvent::RowDoubleClicked { idx: 1 }),
        "{:?}",
        driver.app().events
    );
}

#[test]
fn a_middle_click_on_a_table_row_reports_its_button() {
    let mut driver = tui(Widget::Table(table(SelectionMode::Single)));
    let (x, y) = at(&driver, "row3");
    driver.middle_click(x, y);
    assert_eq!(
        driver.app().table_events(),
        vec![DataTableEvent::RowClicked {
            idx: 3,
            button: MouseButton::Middle,
            modifiers: Modifiers::default(),
        }]
    );
}

#[test]
fn multi_mode_table_grows_and_toggles_a_selection() {
    let mut driver = tui(Widget::Table(table(SelectionMode::Multi)));
    driver.set_double_click_folding(false);
    let (x1, y1) = at(&driver, "row1");
    let (x2, y2) = at(&driver, "row2");
    let (x3, y3) = at(&driver, "row3");
    driver.click(x1, y1);
    driver.click_with(MouseButton::Left, shift(), x3, y3);
    driver.click_with(MouseButton::Left, ctrl(), x2, y2);
    driver.ctrl_char('a');
    let sets: Vec<Vec<usize>> = driver
        .app()
        .table_events()
        .into_iter()
        .filter_map(|e| match e {
            DataTableEvent::SelectionSet { indices } => Some(indices),
            _ => None,
        })
        .collect();
    assert_eq!(
        sets,
        vec![vec![1, 2, 3], vec![1, 3], vec![0, 1, 2, 3, 4]],
        "{:?}",
        driver.app().events
    );
}

#[test]
fn ctrl_space_toggles_the_cursor_row_and_plain_space_does_not() {
    let mut driver = tui(Widget::Table(table(SelectionMode::Multi)));
    let (x, y) = at(&driver, "row2");
    driver.click(x, y);
    driver.press(quadraui::Key::Char(' '));
    assert!(
        !driver
            .app()
            .table_events()
            .iter()
            .any(|e| matches!(e, DataTableEvent::SelectionSet { .. })),
        "plain Space must not toggle: {:?}",
        driver.app().events
    );
    driver.dispatch(UiEvent::KeyPressed {
        key: quadraui::Key::Char(' '),
        modifiers: ctrl(),
        repeat: false,
    });
    assert_eq!(
        driver.app().table_events().last(),
        Some(&DataTableEvent::SelectionSet { indices: vec![] })
    );
}

#[test]
fn single_mode_table_treats_a_shift_click_as_a_plain_click() {
    let mut driver = tui(Widget::Table(table(SelectionMode::Single)));
    driver.set_double_click_folding(false);
    let (x1, y1) = at(&driver, "row1");
    let (x3, y3) = at(&driver, "row3");
    driver.click(x1, y1);
    driver.click_with(MouseButton::Left, shift(), x3, y3);
    assert_eq!(
        driver.app().table_events(),
        vec![
            DataTableEvent::RowSelected { idx: 1 },
            DataTableEvent::RowSelected { idx: 3 },
        ]
    );
}

#[test]
fn list_right_and_middle_clicks_report_their_buttons() {
    let mut driver = tui(Widget::List(list()));
    let (x, y) = at(&driver, "item2");
    driver.right_click(x, y);
    driver.right_click(x, y);
    driver.middle_click(x, y);
    let events: Vec<ListViewEvent> = driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::List(_, ev) => Some(ev.clone()),
            _ => None,
        })
        .collect();
    assert!(
        matches!(
            events.as_slice(),
            [
                ListViewEvent::ContextMenuRequested { idx: 2, .. },
                ListViewEvent::ContextMenuRequested { idx: 2, .. },
                ListViewEvent::ItemClicked {
                    idx: 2,
                    button: MouseButton::Middle,
                    ..
                },
            ]
        ),
        "{events:?}"
    );
}

#[test]
fn tree_middle_click_reports_its_button_and_right_clicks_never_double_click() {
    let mut driver = tui(Widget::Tree(tree()));
    let (x, y) = at(&driver, "node1");
    driver.middle_click(x, y);
    driver.right_click(x, y);
    driver.right_click(x, y);
    let events: Vec<TreeEvent> = driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Tree(_, ev) => Some(ev.clone()),
            _ => None,
        })
        .collect();
    assert!(
        matches!(
            events.as_slice(),
            [
                TreeEvent::RowClicked {
                    button: MouseButton::Middle,
                    ..
                },
                TreeEvent::ContextMenuRequested { .. },
                TreeEvent::ContextMenuRequested { .. },
            ]
        ),
        "{events:?}"
    );
}

fn last_selection_set(driver: &TuiDriver<WidgetApp>) -> Option<UiEvent> {
    driver
        .app()
        .events
        .iter()
        .rev()
        .find(|e| {
            matches!(
                e,
                UiEvent::List(_, ListViewEvent::SelectionSet { .. })
                    | UiEvent::Tree(_, TreeEvent::SelectionSet { .. })
            )
        })
        .cloned()
}

#[test]
fn a_ctrl_click_after_arrow_keys_grows_the_list_selection_from_the_cursor_row() {
    let mut driver = tui(Widget::List(list()));
    let (x, y) = at(&driver, "item0");
    driver.click(x, y);
    driver.press_named(quadraui::NamedKey::Down);
    driver.press_named(quadraui::NamedKey::Down);
    let (x, y) = at(&driver, "item3");
    driver.click_with(MouseButton::Left, ctrl(), x, y);
    assert!(
        matches!(
            last_selection_set(&driver),
            Some(UiEvent::List(_, ListViewEvent::SelectionSet { ref indices })) if indices == &[2, 3]
        ),
        "{:?}",
        driver.app().events
    );
}

#[test]
fn a_ctrl_click_after_an_arrow_key_grows_the_tree_selection_from_the_cursor_row() {
    let mut multi = tree();
    multi.selection_mode = SelectionMode::Multi;
    let mut driver = tui(Widget::Tree(multi));
    let (x, y) = at(&driver, "node0");
    driver.click(x, y);
    driver.press_named(quadraui::NamedKey::Down);
    let (x, y) = at(&driver, "node2");
    driver.click_with(MouseButton::Left, ctrl(), x, y);
    assert!(
        matches!(
            last_selection_set(&driver),
            Some(UiEvent::Tree(_, TreeEvent::SelectionSet { ref paths })) if paths == &[vec![1], vec![2]]
        ),
        "{:?}",
        driver.app().events
    );
}

#[cfg(feature = "gtk")]
#[test]
fn gtk_two_quick_right_clicks_on_a_table_row_are_two_context_menu_requests() {
    use quadraui::gtk::testing::GtkDriver;
    use quadraui::Point;

    let mut driver = GtkDriver::new(
        WidgetApp::new(Widget::Table(table(SelectionMode::Single)), 400.0, 300.0),
        400,
        300,
    );
    driver.render();
    let (x, y) = driver
        .find("row2")
        .unwrap_or_else(|| panic!("row2 not painted: {:?}", driver.painted_texts()));
    for _ in 0..2 {
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Right,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        });
    }
    let events = driver.app().table_events();
    let menus = events
        .iter()
        .filter(|e| matches!(e, DataTableEvent::ContextMenuRequested { idx: 2, .. }))
        .count();
    assert_eq!(menus, 2, "{events:?}");
}
