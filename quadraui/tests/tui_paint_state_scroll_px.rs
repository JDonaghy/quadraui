#![cfg(feature = "tui")]
//! Headless `TuiDriver` coverage for the A3c pixel-smooth-scrolling seam:
//! `scroll_px` sourced from a per-frame `*PaintState` (not a descriptor
//! field) and fed into `Backend::draw_{list,tree,data_table}_with_state`.
//!
//! Each test accumulates a fractional wheel delta into a `scroll_px`
//! carried by the widget's paint state, confirms the accumulator keeps
//! its sub-row precision between frames, and confirms TUI quantises the
//! painted first visible row to a whole one (never a partial row) —
//! `ListView`/`TreeView`/`DataTable::scroll_offset` itself is untouched
//! by this, consistent with the seam being additive-only.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    scroll_by_clamped_f32, wheel_delta, AppLogic, Backend, Column, ColumnAlign, ColumnWidth,
    DataRow, DataTable, DataTablePaintState, Decoration, ListItem, ListPaintState, ListView,
    Reaction, Rect, StyledText, TreeController, TreePath, TreeRow, UiEvent, WidgetId,
};

const VIEWPORT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 20.0,
    height: 5.0,
};
const ROW_COUNT: usize = 20;

fn scroll_event(dy: f32) -> UiEvent {
    UiEvent::Scroll {
        widget: None,
        delta: quadraui::ScrollDelta::new(0.0, dy),
        position: quadraui::Point::new(2.0, 2.0),
    }
}

// ── ListView ────────────────────────────────────────────────────────────

struct ListApp {
    list: ListView,
    scroll_px: Option<f32>,
}

impl ListApp {
    fn new() -> Self {
        Self {
            list: ListView {
                id: WidgetId::new("test:list"),
                items: (0..ROW_COUNT)
                    .map(|i| ListItem {
                        text: StyledText::plain(format!("Row {i:02}")),
                        icon: None,
                        detail: None,
                        decoration: Decoration::default(),
                    })
                    .collect(),
                selected_idx: 0,
                scroll_offset: 0,
                has_focus: true,
                title: None,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: false,
            },
            scroll_px: None,
        }
    }
}

impl AppLogic for ListApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let state = ListPaintState::default().with_scroll_px(self.scroll_px);
        backend.draw_list_with_state(VIEWPORT, &self.list, &state, false);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Scroll { delta, .. } => {
                let dy = wheel_delta(delta.y, 0.25);
                let current = self.scroll_px.unwrap_or(0.0);
                self.scroll_px = Some(scroll_by_clamped_f32(current, dy, (ROW_COUNT - 1) as f32));
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

#[test]
fn list_scroll_px_accumulates_sub_row_then_quantises_on_tui() {
    let mut driver = TuiDriver::new(ListApp::new(), 20, 5);
    driver.render();
    assert!(driver.screen_contains("Row 00"));

    // A quarter-row gesture must not jump a whole row: `scroll_px`
    // carries the fraction, but the TUI-rendered first row stays put.
    driver.dispatch(scroll_event(-1.0));
    assert_eq!(driver.app().scroll_px, Some(0.25));
    driver.render();
    assert!(
        driver.screen_contains("Row 00"),
        "a quarter-row gesture must not jump a whole row:\n{}",
        driver.screen()
    );

    // Three more identical gestures (1.0 row total) cross the boundary;
    // TUI paints the quantised whole row, never a partial one.
    for _ in 0..3 {
        driver.dispatch(scroll_event(-1.0));
    }
    assert_eq!(driver.app().scroll_px, Some(1.0));
    driver.render();
    assert!(
        driver.screen_contains("Row 01") && !driver.screen_contains("Row 00"),
        "four quarter-row gestures should add up to one whole row:\n{}",
        driver.screen()
    );
}

// ── TreeView (via TreeController) ───────────────────────────────────────

fn tree_rows(n: usize) -> Vec<TreeRow> {
    (0..n)
        .map(|i| TreeRow {
            path: vec![i as u16] as TreePath,
            indent: 0,
            icon: None,
            text: StyledText::plain(format!("Node {i:02}")),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        })
        .collect()
}

struct TreeApp {
    controller: TreeController,
}

impl TreeApp {
    fn new() -> Self {
        let mut controller = TreeController::new("test:tree");
        controller.set_rows(tree_rows(ROW_COUNT));
        controller.set_show_scrollbar(false);
        Self { controller }
    }
}

impl AppLogic for TreeApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        self.controller.render(backend, VIEWPORT);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        self.controller.handle(&event, backend, VIEWPORT);
        Reaction::Redraw
    }
}

#[test]
fn tree_controller_scroll_px_accumulates_sub_row_then_quantises_on_tui() {
    let mut driver = TuiDriver::new(TreeApp::new(), 20, 5);
    driver.render();
    assert!(driver.screen_contains("Node 00"));

    // `TreeController::handle` itself accumulates the fractional wheel
    // delta into `scroll_px` — see `scroll_by_wheel` — rather than
    // snapping straight to a whole row.
    for _ in 0..4 {
        driver.dispatch(scroll_event(-0.2));
    }
    assert!(
        driver.app().controller.scroll_px().unwrap_or(0.0) > 0.0,
        "the controller should still be holding a sub-row scroll_px"
    );
    driver.render();
    assert!(
        driver.screen_contains("Node 00"),
        "sub-row scroll_px must not have scrolled the painted row yet:\n{}",
        driver.screen()
    );

    // A fifth gesture crosses the boundary; TUI paints the quantised
    // whole row.
    driver.dispatch(scroll_event(-0.2));
    driver.render();
    assert!(
        driver.screen_contains("Node 01") && !driver.screen_contains("Node 00"),
        "five 0.2-row gestures should add up to one whole row:\n{}",
        driver.screen()
    );
}

// ── DataTable ────────────────────────────────────────────────────────────

fn data_table(n: usize) -> DataTable {
    DataTable {
        id: WidgetId::new("test:table"),
        columns: vec![Column {
            title: "Name".to_string(),
            width: ColumnWidth::Flex(1.0),
            align: ColumnAlign::Left,
        }],
        rows: (0..n)
            .map(|i| DataRow {
                cells: vec![StyledText::plain(format!("item-{i:02}"))],
                decoration: Decoration::Normal,
            })
            .collect(),
        selected_idx: None,
        scroll_offset: 0,
        sort: None,
        has_focus: false,
        show_scrollbar: false,
        min_total_width: None,
        h_scroll: 0.0,
        column_overrides: Vec::new(),
        footer: None,
    }
}

struct DataTableApp {
    table: DataTable,
    scroll_px: Option<f32>,
}

impl DataTableApp {
    fn new() -> Self {
        Self {
            table: data_table(ROW_COUNT),
            scroll_px: None,
        }
    }
}

impl AppLogic for DataTableApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let state = DataTablePaintState::default().with_scroll_px(self.scroll_px);
        backend.draw_data_table_with_state(VIEWPORT, &self.table, &state);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Scroll { delta, .. } => {
                let dy = wheel_delta(delta.y, 0.25);
                let current = self.scroll_px.unwrap_or(0.0);
                self.scroll_px = Some(scroll_by_clamped_f32(current, dy, (ROW_COUNT - 1) as f32));
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

#[test]
fn data_table_scroll_px_accumulates_sub_row_then_quantises_on_tui() {
    let mut driver = TuiDriver::new(DataTableApp::new(), 20, 5);
    driver.render();
    assert!(driver.screen_contains("item-00"));

    for _ in 0..3 {
        driver.dispatch(scroll_event(-1.0));
    }
    assert_eq!(driver.app().scroll_px, Some(0.75));
    driver.render();
    assert!(
        driver.screen_contains("item-00"),
        "three quarter-row gestures (0.75 total) must not have scrolled yet:\n{}",
        driver.screen()
    );

    driver.dispatch(scroll_event(-1.0));
    assert_eq!(driver.app().scroll_px, Some(1.0));
    driver.render();
    assert!(
        driver.screen_contains("item-01") && !driver.screen_contains("item-00"),
        "four quarter-row gestures should add up to one whole row:\n{}",
        driver.screen()
    );
}
