#![cfg(feature = "tui")]
//! Headless `TuiDriver` tests for the row-windowing seam
//! (`quadraui::behaviour::row_window`): a `ListView`, `DataTable`,
//! `TreeView` or `TextDisplay` whose descriptor sets `items_window`
//! carries only a slice of a 100,000-row collection, and the widget
//! itself paints placeholders for the rest, moves its cursor and scroll
//! over the real extent, and asks the app for the rows it needs through
//! `UiEvent::RowsRequested` — once per new range.
//!
//! The app here is uncontrolled: it never echoes a selection or scroll
//! back into the descriptor, so every move the widgets make is resolved
//! from their own state. It answers `RowsRequested` only when a test says
//! so ([`WindowApp::load`]), so a test can count the requests first.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Column, DataRow, DataTable, DataTableEvent, DrawsPrimitives, FrameTree,
    ItemsWindow, ListItem, ListView, ListViewEvent, NamedKey, Reaction, Rect, SelectionMode,
    StyledSpan, StyledText, TextDisplay, TextDisplayEvent, TextDisplayLine, TreeEvent, TreeRow,
    TreeStyle, TreeView, UiEvent, WidgetId,
};

const TOTAL: usize = 100_000;
const RECT: Rect = Rect::new(0.0, 0.0, 40.0, 10.0);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    List,
    Table,
    Tree,
    Text,
}

fn label(abs: usize) -> String {
    format!("row-{abs}")
}

fn tree_path(abs: usize) -> Vec<u16> {
    vec![(abs / 60_000) as u16, (abs % 60_000) as u16]
}

struct WindowApp {
    kind: Kind,
    id: WidgetId,
    /// The loaded slice, `[start, start + len)`.
    start: usize,
    len: usize,
    requests: Vec<(usize, usize)>,
    events: Vec<UiEvent>,
}

impl WindowApp {
    fn new(kind: Kind, start: usize, len: usize) -> Self {
        Self {
            kind,
            id: WidgetId::new("rows"),
            start,
            len,
            requests: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Answer the last `RowsRequested`: load the rows it asked for.
    fn load_requested(&mut self) {
        let (start, count) = *self.requests.last().expect("a request to answer");
        self.start = start;
        self.len = count;
    }

    fn window(&self) -> Option<ItemsWindow> {
        Some(ItemsWindow::new(self.start, TOTAL))
    }

    fn loaded(&self) -> std::ops::Range<usize> {
        self.start..(self.start + self.len).min(TOTAL)
    }

    fn widget(&self) -> Box<dyn quadraui::primitive::DynWidget> {
        match self.kind {
            Kind::List => Box::new(ListView {
                id: self.id,
                title: None,
                items: self
                    .loaded()
                    .map(|abs| ListItem {
                        text: StyledText::plain(label(abs)),
                        icon: None,
                        detail: None,
                        decoration: Default::default(),
                    })
                    .collect(),
                selected_idx: 0,
                scroll_offset: 0,
                items_window: self.window(),
                scroll_px: None,
                has_focus: true,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: true,
                striped: false,
            }),
            Kind::Table => Box::new(DataTable {
                id: self.id,
                columns: vec![Column {
                    title: "Name".into(),
                    width: Default::default(),
                    align: Default::default(),
                }],
                rows: self
                    .loaded()
                    .map(|abs| DataRow {
                        cells: vec![StyledText::plain(label(abs))],
                        decoration: Default::default(),
                    })
                    .collect(),
                selected_idx: None,
                scroll_offset: 0,
                items_window: self.window(),
                scroll_px: None,
                sort: None,
                has_focus: true,
                show_scrollbar: true,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: Vec::new(),
                footer: None,
                bordered: false,
                striped: false,
                glyphs: None,
            }),
            Kind::Tree => Box::new(TreeView {
                id: self.id,
                rows: self
                    .loaded()
                    .map(|abs| TreeRow {
                        path: tree_path(abs),
                        indent: 0,
                        icon: None,
                        text: StyledText::plain(label(abs)),
                        badge: None,
                        is_expanded: None,
                        decoration: Default::default(),
                        edit: None,
                    })
                    .collect(),
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                items_window: self.window(),
                scroll_px: None,
                style: TreeStyle::default(),
                has_focus: true,
            }),
            Kind::Text => {
                let mut td = TextDisplay::new(self.id);
                td.auto_scroll = false;
                td.show_scrollbar = true;
                td.items_window = self.window();
                td.lines = self
                    .loaded()
                    .map(|abs| TextDisplayLine {
                        spans: vec![StyledSpan::plain(label(abs))],
                        decoration: Default::default(),
                        timestamp: None,
                    })
                    .collect();
                Box::new(td)
            }
        }
    }
}

impl AppLogic for WindowApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let widget = self.widget();
        let _ = backend.draw_dyn(RECT, widget.as_ref(), &());
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.id, RECT, self.widget());
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::RowsRequested { id, start, count } = &event {
            assert_eq!(*id, self.id);
            self.requests.push((*start, *count));
        }
        self.events.push(event);
        Reaction::Redraw
    }
}

fn driver(kind: Kind, start: usize, len: usize) -> TuiDriver<WindowApp> {
    let mut driver = TuiDriver::new(WindowApp::new(kind, start, len), 60, 12);
    // Focus the widget: every kind is one tab stop.
    driver.click(2.0, 3.0);
    driver.app_mut().events.clear();
    driver
}

fn placeholders_paint_where_rows_are_not_loaded(kind: Kind) {
    let mut driver = TuiDriver::new(WindowApp::new(kind, 5, 10), 60, 12);
    let screen = driver.screen();
    assert!(
        !screen.contains("row-0") && !screen.contains("row-4"),
        "{kind:?}: rows 0-4 are not loaded, so nothing paints for them:\n{screen}"
    );
    assert!(
        screen.contains("row-5"),
        "{kind:?}: the loaded slice paints at its absolute position:\n{screen}"
    );
    driver.app_mut().start = 0;
    driver.render();
    assert!(
        driver.screen_contains("row-0"),
        "{kind:?}: loading the rows repaints them"
    );
}

fn end_requests_the_tail_once_and_loading_repaints_it(kind: Kind) {
    let mut driver = driver(kind, 0, 30);
    driver.press_named(NamedKey::End);
    assert_eq!(
        driver.app().requests.len(),
        1,
        "{kind:?}: End leaves the loaded slice, so exactly one request: {:?}",
        driver.app().events
    );
    let (start, count) = driver.app().requests[0];
    assert!(
        start + count == TOTAL && count < 100,
        "{kind:?}: the request covers the tail, bounded by the viewport: {start}+{count}"
    );
    assert!(
        !driver.screen_contains("row-99999"),
        "{kind:?}: the tail is not loaded yet"
    );
    driver.press_named(NamedKey::End);
    assert_eq!(
        driver.app().requests.len(),
        1,
        "{kind:?}: the same range is never asked for twice"
    );

    driver.app_mut().load_requested();
    driver.render();
    assert!(
        driver.screen_contains("row-99999"),
        "{kind:?}: loading the requested rows repaints them:\n{}",
        driver.screen()
    );
    driver.press_named(NamedKey::Home);
    assert_eq!(
        driver.app().requests.len(),
        2,
        "{kind:?}: back to the top is a new range"
    );
}

/// The cursor (or, for a text display, the scroll) the End key reached,
/// read off the widget's own event.
fn end_moves_the_cursor_past_the_loaded_slice(kind: Kind) {
    let mut driver = driver(kind, 0, 30);
    driver.press_named(NamedKey::End);
    let events = &driver.app().events;
    match kind {
        Kind::List => assert!(events.contains(&UiEvent::List(
            driver.app().id,
            ListViewEvent::SelectionChanged { idx: TOTAL - 1 }
        ))),
        Kind::Table => assert!(events.contains(&UiEvent::DataTable(
            driver.app().id,
            DataTableEvent::RowSelected { idx: TOTAL - 1 }
        ))),
        // A windowed tree reports a path only for a loaded row; its cursor
        // still moved, which the next request and the next key show.
        Kind::Tree => assert!(!events
            .iter()
            .any(|e| matches!(e, UiEvent::Tree(_, TreeEvent::SelectionChanged { .. })))),
        Kind::Text => assert!(events.iter().any(|e| matches!(
            e,
            UiEvent::TextDisplay(_, TextDisplayEvent::Scrolled { new_offset })
                if *new_offset > TOTAL - 20
        ))),
    }
    if kind == Kind::Tree {
        driver.app_mut().load_requested();
        driver.render();
        driver.app_mut().events.clear();
        driver.press_named(NamedKey::Up);
        assert!(
            driver.app().events.contains(&UiEvent::Tree(
                driver.app().id,
                TreeEvent::SelectionChanged {
                    path: tree_path(TOTAL - 2)
                }
            )),
            "the tree's cursor sat on the unloaded last row: {:?}",
            driver.app().events
        );
    }
}

macro_rules! row_window_tests {
    ($($name:ident => $kind:expr),* $(,)?) => {
        $(
            mod $name {
                use super::*;

                #[test]
                fn placeholders_paint_where_rows_are_not_loaded() {
                    super::placeholders_paint_where_rows_are_not_loaded($kind);
                }

                #[test]
                fn end_requests_the_tail_once_and_loading_repaints_it() {
                    super::end_requests_the_tail_once_and_loading_repaints_it($kind);
                }

                #[test]
                fn end_moves_the_cursor_past_the_loaded_slice() {
                    super::end_moves_the_cursor_past_the_loaded_slice($kind);
                }
            }
        )*
    };
}

row_window_tests! {
    list => Kind::List,
    data_table => Kind::Table,
    tree => Kind::Tree,
    text_display => Kind::Text,
}

#[test]
fn a_ten_billion_row_total_allocates_only_the_band() {
    let mut app = WindowApp::new(Kind::List, 0, 10);
    app.kind = Kind::List;
    let mut list = match app.widget().as_ref().downcast_ref::<ListView>() {
        Some(list) => list.clone(),
        None => unreachable!(),
    };
    list.items_window = Some(ItemsWindow::new(0, 10_000_000_000));
    list.scroll_offset = 5_000_000_000;
    let ctx = quadraui::primitive::LayoutCtx::new(1.0, 1.0, false);
    let layout = quadraui::primitive::Primitive::layout(&list, &ctx, RECT);
    let window = layout.window.expect("a windowed layout");
    assert!(window.live.items.len() <= 32, "the band is viewport-sized");
    assert_eq!(window.band.start, 5_000_000_000);
    assert_eq!(window.total, 10_000_000_000);
}
