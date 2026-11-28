#![cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos")
))]
//! Black-box tests for the row widgets on the widget contract under a
//! host-owned pixel scroll: each widget is painted through
//! `DrawsPrimitives::draw_dyn` with a paint state whose `scroll_px` is a
//! fractional number of rows, registered in the app's `FrameTree`, and
//! clicked on a painted row through the real routing engine. The click
//! must select the row painted under the pointer, so layout, routing and
//! paint all resolve the same scroll.
//!
//! `DataTable`, `ListView`, `TreeView` and `TreeTable` each get a TUI leg
//! (whole-row scroll: the fraction is quantised) and a pixel leg (the
//! fraction shifts every row up by a partial row).

use quadraui::interactive::Interactive;
use quadraui::{
    AppLogic, Backend, Column, ColumnAlign, ColumnWidth, DataRow, DataTable, DataTableEvent,
    Decoration, DrawsPrimitives, FrameTree, ListItem, ListPaintState, ListView, ListViewEvent,
    Reaction, Rect, SelectionMode, StyledText, TreeEvent, TreeRow, TreeStyle, TreeTable,
    TreeTableEvent, TreeTableNode, TreeView, UiEvent, WidgetId,
};

const ROW_COUNT: usize = 30;

fn label(i: usize) -> String {
    format!("row{i:02}")
}

fn list_view() -> ListView {
    ListView {
        id: WidgetId::new("rows:list"),
        title: None,
        items: (0..ROW_COUNT)
            .map(|i| ListItem {
                text: StyledText::plain(label(i)),
                icon: None,
                detail: None,
                decoration: Decoration::Normal,
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
    }
}

fn tree_view() -> TreeView {
    TreeView {
        id: WidgetId::new("rows:tree"),
        rows: (0..ROW_COUNT)
            .map(|i| TreeRow {
                path: vec![i as u16],
                indent: 0,
                icon: None,
                text: StyledText::plain(label(i)),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            })
            .collect(),
        selection_mode: SelectionMode::Single,
        selected_path: None,
        scroll_offset: 0,
        items_window: None,
        scroll_px: None,
        style: TreeStyle {
            show_scrollbar: false,
            ..TreeStyle::default()
        },
        has_focus: false,
    }
}

fn data_table() -> DataTable {
    DataTable {
        id: WidgetId::new("rows:table"),
        columns: vec![Column {
            title: "Name".into(),
            width: ColumnWidth::Flex(1.0),
            align: ColumnAlign::Left,
        }],
        rows: (0..ROW_COUNT)
            .map(|i| DataRow {
                cells: vec![StyledText::plain(label(i))],
                decoration: Decoration::Normal,
            })
            .collect(),
        selected_idx: None,
        scroll_offset: 0,
        items_window: None,
        scroll_px: None,
        sort: None,
        has_focus: false,
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

fn tree_table() -> TreeTable {
    let mut table = data_table();
    table.id = WidgetId::new("rows:tree_table");
    let tree = (0..ROW_COUNT)
        .map(|i| TreeTableNode::new(vec![i as u16], 0))
        .collect();
    TreeTable::new(table, tree)
}

/// The row a routed event selects, for whichever row widget sent it.
fn selected_row(event: &UiEvent) -> Option<usize> {
    match event {
        UiEvent::List(_, ListViewEvent::SelectionChanged { idx }) => Some(*idx),
        UiEvent::Tree(_, TreeEvent::RowClicked { path, .. })
        | UiEvent::Tree(_, TreeEvent::SelectionChanged { path }) => {
            path.first().map(|i| *i as usize)
        }
        UiEvent::DataTable(_, DataTableEvent::RowSelected { idx }) => Some(*idx),
        UiEvent::TreeTable(_, TreeTableEvent::RowSelected { idx, .. }) => Some(*idx),
        _ => None,
    }
}

/// One row widget painted with a host-owned `scroll_px` and routed
/// through the app's `FrameTree`, recording every event that comes back.
struct ScrolledApp<W> {
    widget: W,
    rect: Rect,
    scroll_px: f32,
    events: Vec<UiEvent>,
}

impl<W> ScrolledApp<W> {
    fn new(widget: W, rect: Rect, scroll_px: f32) -> Self {
        Self {
            widget,
            rect,
            scroll_px,
            events: Vec::new(),
        }
    }

    fn last_selected(&self) -> Option<usize> {
        self.events.iter().rev().find_map(selected_row)
    }
}

impl<W: Interactive + Clone + 'static> AppLogic for ScrolledApp<W> {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let state = ListPaintState::default().with_scroll_px(Some(self.scroll_px));
        backend.draw_dyn(self.rect, &self.widget, &state);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        let id = *self.widget.id().expect("a row widget names itself");
        tree.push(id, self.rect, Box::new(self.widget.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Continue
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;

    const RECT: Rect = Rect::new(0.0, 0.0, 30.0, 10.0);
    /// Three and a half rows: a cell grid paints from row 3.
    const SCROLL_PX: f32 = 3.5;

    /// Paint `widget` scrolled by [`SCROLL_PX`], check the painted window
    /// starts at row 3, then click row 5's label and return the row the
    /// click selected.
    fn click_row_5<W: Interactive + Clone + 'static>(widget: W) -> Option<usize> {
        let mut driver = TuiDriver::new(ScrolledApp::new(widget, RECT, SCROLL_PX), 30, 10);
        assert!(
            !driver.screen_contains(&label(2)) && driver.screen_contains(&label(3)),
            "the painted window starts at row 3:\n{}",
            driver.screen()
        );
        let (x, y) = driver.find(&label(5)).expect("row 5 painted");
        driver.click(x, y);
        driver.app().last_selected()
    }

    #[test]
    fn list_view_click_selects_the_row_painted_under_a_scroll_px() {
        assert_eq!(click_row_5(list_view()), Some(5));
    }

    #[test]
    fn tree_view_click_selects_the_row_painted_under_a_scroll_px() {
        assert_eq!(click_row_5(tree_view()), Some(5));
    }

    #[test]
    fn data_table_click_selects_the_row_painted_under_a_scroll_px() {
        assert_eq!(click_row_5(data_table()), Some(5));
    }

    #[test]
    fn tree_table_click_selects_the_row_painted_under_a_scroll_px() {
        assert_eq!(click_row_5(tree_table()), Some(5));
    }
}

/// The pixel legs: a scroll of a fraction of a row moves every row up by
/// less than a row, so row 0 is gone, row 5 sits between two whole-row
/// positions, and a click on its label must still select it.
#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
const PIXEL_RECT: Rect = Rect::new(0.0, 0.0, 300.0, 400.0);
/// Fifty pixels: more than one row and not a whole number of rows at any
/// row pitch the pixel backends resolve.
#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
const PIXEL_SCROLL_PX: f32 = 50.0;

#[cfg(feature = "gtk")]
mod gtk {
    use super::*;
    use quadraui::gtk::testing::GtkDriver;

    fn click_row_5<W: Interactive + Clone + 'static>(widget: W) -> Option<usize> {
        let app = ScrolledApp::new(widget, PIXEL_RECT, PIXEL_SCROLL_PX);
        let mut driver = GtkDriver::new(app, 320, 420);
        assert!(
            !driver.screen_contains(&label(0)),
            "a 50px scroll moves row 0 out of view"
        );
        let (x, y) = driver.find(&label(5)).expect("row 5 painted");
        driver.click(x, y);
        driver.app().last_selected()
    }

    #[test]
    fn list_view_click_selects_the_row_painted_under_a_scroll_px_on_gtk() {
        assert_eq!(click_row_5(list_view()), Some(5));
    }

    #[test]
    fn tree_view_click_selects_the_row_painted_under_a_scroll_px_on_gtk() {
        assert_eq!(click_row_5(tree_view()), Some(5));
    }

    #[test]
    fn data_table_click_selects_the_row_painted_under_a_scroll_px_on_gtk() {
        assert_eq!(click_row_5(data_table()), Some(5));
    }

    #[test]
    fn tree_table_click_selects_the_row_painted_under_a_scroll_px_on_gtk() {
        assert_eq!(click_row_5(tree_table()), Some(5));
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod macos {
    use super::*;
    use quadraui::macos::testing::MacDriver;

    fn click_row_5<W: Interactive + Clone + 'static>(widget: W) -> Option<usize> {
        let app = ScrolledApp::new(widget, PIXEL_RECT, PIXEL_SCROLL_PX);
        let mut driver = MacDriver::new(app, 320, 420);
        assert!(
            !driver.screen_contains(&label(0)),
            "a 50px scroll moves row 0 out of view"
        );
        let (x, y) = driver.find(&label(5)).expect("row 5 painted");
        driver.click(x, y);
        driver.app().last_selected()
    }

    #[test]
    fn list_view_click_selects_the_row_painted_under_a_scroll_px_on_macos() {
        assert_eq!(click_row_5(list_view()), Some(5));
    }

    #[test]
    fn tree_view_click_selects_the_row_painted_under_a_scroll_px_on_macos() {
        assert_eq!(click_row_5(tree_view()), Some(5));
    }

    #[test]
    fn data_table_click_selects_the_row_painted_under_a_scroll_px_on_macos() {
        assert_eq!(click_row_5(data_table()), Some(5));
    }

    #[test]
    fn tree_table_click_selects_the_row_painted_under_a_scroll_px_on_macos() {
        assert_eq!(click_row_5(tree_table()), Some(5));
    }
}
