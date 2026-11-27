#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `TreeTable` on the widget contract: the
//! table reaches the frame as a type-erased `Surface::Widget`, paints
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree`, so every click, key and wheel
//! notch below goes through the shared routing engine into
//! `Interactive::handle`, and comes back out as the `TreeTableEvent` this
//! test's own `apply` writes back onto the owned descriptor — re-flattening
//! the rows the way a host owning the tree would.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    Column, ColumnAlign, ColumnWidth, DataRow, DataTable, Decoration, NamedKey, Point, Rect,
    ScrollDelta, SortDirection, StyledText, TreeTable, TreeTableEvent, TreeTableNode, UiEvent,
    WidgetId,
};

const COLS: u16 = 44;
const ROWS: u16 = 14;
/// Header on row 1, body rows 2..=12 (eleven rows), scrollbar in column 40.
const TABLE: Rect = Rect::new(1.0, 1.0, 40.0, 12.0);
const N_FILES: usize = 28;

fn row(label: &str, size: &str, decoration: Decoration) -> DataRow {
    DataRow {
        cells: vec![StyledText::plain(label), StyledText::plain(size)],
        decoration,
    }
}

/// `src` (collapsed, children known), `remote` (collapsed, children not
/// loaded yet), then plain files.
fn tree_table() -> TreeTable {
    let mut rows = vec![
        row("src", "", Decoration::Normal),
        row("remote", "", Decoration::Normal),
    ];
    let mut tree = vec![
        TreeTableNode::new(vec![0], 0).with_expanded(false),
        TreeTableNode::new(vec![1], 0)
            .with_expanded(false)
            .with_children_pending(),
    ];
    for i in 0..N_FILES {
        rows.push(row(&format!("file-{i:02}"), "1k", Decoration::Normal));
        tree.push(TreeTableNode::new(vec![i as u16 + 2], 0));
    }
    TreeTable::new(
        DataTable {
            id: WidgetId::new("tree_table:contract"),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Size".into(),
                    width: ColumnWidth::FixedCells(6.0),
                    align: ColumnAlign::Right,
                },
            ],
            rows,
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: true,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            bordered: false,
            striped: false,
            glyphs: None,
        },
        tree,
    )
}

type TreeTableApp = one_widget::OneWidgetApp<TreeTable>;

/// Insert `children` (label, path) below row `idx`, one level deeper.
fn insert_children(tt: &mut TreeTable, idx: usize, children: Vec<(DataRow, TreeTableNode)>) {
    for (offset, (row, node)) in children.into_iter().enumerate() {
        tt.table.rows.insert(idx + 1 + offset, row);
        tt.tree.insert(idx + 1 + offset, node);
    }
}

fn apply(tt: &mut TreeTable, event: &UiEvent) -> bool {
    let UiEvent::TreeTable(id, ev) = event else {
        return false;
    };
    if *id != tt.table.id {
        return false;
    }
    match ev {
        TreeTableEvent::RowSelected { idx, .. } => tt.table.selected_idx = Some(*idx),
        TreeTableEvent::ScrollChanged { scroll_offset } => tt.table.scroll_offset = *scroll_offset,
        TreeTableEvent::SortChanged { col, ascending } => {
            let dir = if *ascending {
                SortDirection::Ascending
            } else {
                SortDirection::Descending
            };
            tt.table.sort = Some((*col, dir));
        }
        TreeTableEvent::ToggleExpanded { path, expand } => {
            let Some(idx) = tt.tree.iter().position(|n| &n.path == path) else {
                return false;
            };
            tt.tree[idx].is_expanded = Some(*expand);
            if *expand {
                let indent = tt.tree[idx].indent + 1;
                let child = |i: u16, label: &str| {
                    let mut p = path.clone();
                    p.push(i);
                    (
                        row(label, "", Decoration::Normal),
                        TreeTableNode::new(p, indent),
                    )
                };
                let children = if tt.tree[idx].children_pending {
                    let (row, node) = child(0, "…");
                    vec![(
                        DataRow {
                            decoration: Decoration::Muted,
                            ..row
                        },
                        node.as_placeholder(),
                    )]
                } else {
                    vec![child(0, "main.rs"), child(1, "lib.rs")]
                };
                insert_children(tt, idx, children);
            } else {
                let depth = path.len();
                while tt
                    .tree
                    .get(idx + 1)
                    .is_some_and(|n| n.path.len() > depth && n.path[..depth] == path[..])
                {
                    tt.tree.remove(idx + 1);
                    tt.table.rows.remove(idx + 1);
                }
            }
        }
        _ => {}
    }
    true
}

fn driver() -> TuiDriver<TreeTableApp> {
    one_widget::driver(tree_table(), TABLE, COLS, ROWS, apply)
}

/// Screen row of body row `idx` at the app's current scroll offset.
fn row_y(driver: &TuiDriver<TreeTableApp>, idx: usize) -> f32 {
    TABLE.y + 1.0 + (idx - driver.app().widget.table.scroll_offset) as f32
}

fn applied(driver: &TuiDriver<TreeTableApp>) -> Vec<TreeTableEvent> {
    let id = driver.app().widget.table.id;
    driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::TreeTable(from, ev) if *from == id => Some(ev.clone()),
            _ => None,
        })
        .collect()
}

fn labels(driver: &TuiDriver<TreeTableApp>) -> Vec<String> {
    driver
        .app()
        .widget
        .table
        .rows
        .iter()
        .map(|r| r.cells[0].spans.iter().map(|s| s.text.as_str()).collect())
        .collect()
}

#[test]
fn surface_widget_tree_table_paints_its_header_chevrons_and_rows() {
    let driver = driver();
    assert!(driver.screen_contains("Name"), "{}", driver.screen());
    assert!(driver.screen_contains("▸ src"), "{}", driver.screen());
    assert!(driver.screen_contains("▸ remote"), "{}", driver.screen());
    assert!(driver.screen_contains("file-00"), "{}", driver.screen());
    assert!(!driver.screen_contains("file-10"), "{}", driver.screen());
}

#[test]
fn clicking_a_chevron_expands_the_branch_without_selecting_it() {
    let mut driver = driver();
    let y = row_y(&driver, 0);
    driver.click(TABLE.x, y);
    assert_eq!(
        applied(&driver),
        vec![TreeTableEvent::ToggleExpanded {
            path: vec![0],
            expand: true,
        }]
    );
    assert_eq!(driver.app().widget.table.selected_idx, None);
    assert!(driver.screen_contains("▾ src"), "{}", driver.screen());
    assert!(driver.screen_contains("main.rs"), "{}", driver.screen());
}

#[test]
fn keyboard_navigation_walks_the_tree_and_left_right_fold_it() {
    let mut driver = driver();
    driver.click(TABLE.x + 10.0, row_y(&driver, 0));
    assert_eq!(driver.app().widget.table.selected_idx, Some(0));
    driver.press_named(NamedKey::Right);
    assert_eq!(&labels(&driver)[..3], ["src", "main.rs", "lib.rs"]);
    driver.press_named(NamedKey::Right);
    assert_eq!(
        driver.app().widget.table.selected_idx,
        Some(1),
        "Right on an expanded branch selects its first child"
    );
    driver.press_named(NamedKey::Left);
    assert_eq!(
        driver.app().widget.table.selected_idx,
        Some(0),
        "Left on a leaf selects its parent"
    );
    driver.press_named(NamedKey::Left);
    assert_eq!(&labels(&driver)[..2], ["src", "remote"], "Left collapses");
    driver.press_named(NamedKey::End);
    let last = driver.app().widget.table.rows.len() - 1;
    assert_eq!(driver.app().widget.table.selected_idx, Some(last));
    assert!(
        driver.screen_contains(&format!("file-{:02}", N_FILES - 1)),
        "the selection scrolled into view: {}",
        driver.screen()
    );
}

#[test]
fn expanding_a_lazy_branch_requests_its_children_once_and_skips_the_placeholder() {
    let mut driver = driver();
    driver.click(TABLE.x + 10.0, row_y(&driver, 1));
    driver.press_named(NamedKey::Right);
    let requested = |driver: &TuiDriver<TreeTableApp>| {
        applied(driver)
            .iter()
            .filter(|e| matches!(e, TreeTableEvent::ChildrenRequested { .. }))
            .count()
    };
    assert_eq!(requested(&driver), 1);
    assert!(
        applied(&driver).contains(&TreeTableEvent::ChildrenRequested {
            idx: 1,
            path: vec![1],
        })
    );
    assert!(driver.screen_contains("…"), "{}", driver.screen());
    // Down skips the placeholder straight to the next real row.
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().widget.table.selected_idx, Some(3));
    // Collapsing and expanding again before the app answers asks nothing
    // more.
    driver.press_named(NamedKey::Up);
    driver.press_named(NamedKey::Left);
    driver.press_named(NamedKey::Right);
    assert_eq!(requested(&driver), 1);
}

#[test]
fn header_click_asks_for_the_next_sort_direction() {
    let mut driver = driver();
    let (x, y) = (TABLE.x + 10.0, TABLE.y);
    driver.click(x, y);
    assert_eq!(
        applied(&driver).last(),
        Some(&TreeTableEvent::SortChanged {
            col: 0,
            ascending: true,
        })
    );
    driver.click(x, y);
    assert_eq!(
        applied(&driver).last(),
        Some(&TreeTableEvent::SortChanged {
            col: 0,
            ascending: false,
        })
    );
}

#[test]
fn wheel_scrolls_and_the_scrollbar_track_pages() {
    let mut driver = driver();
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, -3.0),
        position: Point::new(TABLE.x + 2.0, TABLE.y + 3.0),
    });
    assert_eq!(driver.app().widget.table.scroll_offset, 3);
    assert!(driver.screen_contains("file-01"), "{}", driver.screen());
    let track_x = TABLE.x + TABLE.width - 1.0;
    driver.click(track_x, TABLE.y + TABLE.height - 1.0);
    assert_eq!(driver.app().widget.table.scroll_offset, 14);
    assert_eq!(
        driver.app().widget.table.selected_idx,
        None,
        "a track click never selects the row painted beside it"
    );
}

#[test]
fn vim_keys_move_the_selection_when_the_descriptor_turns_them_on() {
    let mut driver = driver();
    driver.app_mut().widget.vim_keys = true;
    driver.render();
    driver.click(TABLE.x + 10.0, row_y(&driver, 2));
    driver.type_char('j');
    assert_eq!(driver.app().widget.table.selected_idx, Some(3));
    driver.type_char('k');
    assert_eq!(driver.app().widget.table.selected_idx, Some(2));
}

#[test]
fn double_click_and_enter_activate_a_row() {
    let mut driver = driver();
    let y = row_y(&driver, 4);
    driver.double_click(TABLE.x + 10.0, y);
    assert!(
        applied(&driver).contains(&TreeTableEvent::RowDoubleClicked {
            idx: 4,
            path: vec![4],
        })
    );
    driver.click(TABLE.x + 10.0, y);
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        applied(&driver).last(),
        Some(&TreeTableEvent::RowActivated {
            idx: 4,
            path: vec![4],
        })
    );
}
