#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `DataTable` on the widget contract: the
//! table reaches the frame as a type-erased `Surface::Widget`, paints
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree`, so every click, key and wheel
//! notch below goes through the shared routing engine into
//! `Interactive::handle`, and comes back out as the `DataTableEvent` this
//! test's own `apply` writes back onto the owned descriptor.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    Column, ColumnAlign, ColumnWidth, DataRow, DataTable, DataTableEvent, Modifiers, MouseButton,
    NamedKey, Point, Rect, ScrollDelta, StyledText, UiEvent, WidgetId,
};

const COLS: u16 = 44;
const ROWS: u16 = 12;
/// Header on row 1, body rows 2..=10 (nine rows), scrollbar in column 40.
const TABLE: Rect = Rect::new(1.0, 1.0, 40.0, 10.0);
const N_ROWS: usize = 30;

fn column(title: &str) -> Column {
    Column {
        title: title.to_string(),
        width: ColumnWidth::Flex(1.0),
        align: ColumnAlign::Left,
    }
}

fn table() -> DataTable {
    let rows = (0..N_ROWS)
        .map(|i| DataRow {
            cells: vec![
                StyledText::plain(format!("row-{i:02}")),
                StyledText::plain(if i % 2 == 0 { "even" } else { "odd" }),
                StyledText::plain(format!("{}", i * 10)),
            ],
            decoration: Default::default(),
        })
        .collect();
    DataTable {
        id: WidgetId::new("table:contract"),
        columns: vec![column("Name"), column("Kind"), column("Size")],
        rows,
        selected_idx: None,
        scroll_offset: 0,
        items_window: None,
        scroll_px: None,
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
    }
}

type TableApp = one_widget::OneWidgetApp<DataTable>;

fn apply(table: &mut DataTable, event: &UiEvent) -> bool {
    let UiEvent::DataTable(id, ev) = event else {
        return false;
    };
    if *id != table.id {
        return false;
    }
    match ev {
        DataTableEvent::RowSelected { idx } => table.selected_idx = Some(*idx),
        DataTableEvent::ScrollChanged { scroll_offset } => table.scroll_offset = *scroll_offset,
        DataTableEvent::ColumnResized { col, width } => {
            if table.column_overrides.len() != table.columns.len() {
                table.column_overrides = vec![None; table.columns.len()];
            }
            table.column_overrides[*col] = Some(*width);
        }
        _ => {}
    }
    true
}

fn driver() -> TuiDriver<TableApp> {
    one_widget::driver(table(), TABLE, COLS, ROWS, apply)
}

/// Screen row of body row `idx` at the app's current scroll offset.
fn row_y(driver: &TuiDriver<TableApp>, idx: usize) -> f32 {
    TABLE.y + 1.0 + (idx - driver.app().widget.scroll_offset) as f32
}

fn applied(driver: &TuiDriver<TableApp>) -> Vec<DataTableEvent> {
    let id = driver.app().widget.id;
    driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::DataTable(from, ev) if *from == id => Some(ev.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn surface_widget_table_paints_its_header_and_rows() {
    let driver = driver();
    assert!(driver.screen_contains("Name"), "{}", driver.screen());
    assert!(driver.screen_contains("row-00"), "{}", driver.screen());
    assert!(driver.screen_contains("row-08"), "{}", driver.screen());
    assert!(!driver.screen_contains("row-09"), "{}", driver.screen());
}

#[test]
fn clicking_a_row_selects_it_and_paints_the_selection() {
    let mut driver = driver();
    let y = row_y(&driver, 3);
    driver.click(TABLE.x + 2.0, y);
    assert_eq!(driver.app().widget.selected_idx, Some(3));
    let theme = quadraui::Theme::default();
    let style = driver.style_at(TABLE.x as u16 + 2, y as u16).unwrap();
    assert_eq!(
        style.bg,
        quadraui::tui::ratatui_color(theme.selection_bg),
        "the selected row paints on the selection background"
    );
}

#[test]
fn arrow_keys_move_the_cursor_and_scroll_it_into_view() {
    let mut driver = driver();
    let y = row_y(&driver, 0);
    driver.click(TABLE.x + 2.0, y);
    for _ in 0..9 {
        driver.press_named(NamedKey::Down);
    }
    assert_eq!(driver.app().widget.selected_idx, Some(9));
    assert_eq!(
        driver.app().widget.scroll_offset,
        1,
        "row 9 is one past the nine-row viewport"
    );
    assert!(driver.screen_contains("row-09"), "{}", driver.screen());
    driver.press_named(NamedKey::End);
    assert_eq!(driver.app().widget.selected_idx, Some(N_ROWS - 1));
    assert_eq!(driver.app().widget.scroll_offset, N_ROWS - 9);
    driver.press_named(NamedKey::Home);
    assert_eq!(driver.app().widget.selected_idx, Some(0));
    assert_eq!(driver.app().widget.scroll_offset, 0);
}

#[test]
fn up_with_nothing_selected_selects_the_last_row() {
    let mut driver = driver();
    // Focus the table without selecting a row: a header click.
    driver.click(TABLE.x + 20.0, TABLE.y);
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().widget.selected_idx, Some(N_ROWS - 1));
}

#[test]
fn enter_and_double_click_activate_a_row() {
    let mut driver = driver();
    let y = row_y(&driver, 2);
    driver.click(TABLE.x + 2.0, y);
    driver.press_named(NamedKey::Enter);
    assert!(applied(&driver).contains(&DataTableEvent::RowActivated { idx: 2 }));
    let y = row_y(&driver, 4);
    driver.double_click(TABLE.x + 2.0, y);
    assert!(applied(&driver).contains(&DataTableEvent::RowDoubleClicked { idx: 4 }));
}

#[test]
fn header_click_requests_a_sort_on_that_column() {
    let mut driver = driver();
    driver.click(TABLE.x + 20.0, TABLE.y);
    assert_eq!(
        applied(&driver).last(),
        Some(&DataTableEvent::HeaderClicked { col: 1 })
    );
}

#[test]
fn dragging_a_header_divider_resizes_the_column() {
    let mut driver = driver();
    let divider_x = driver.find_bounds("Kind").unwrap().x - 1.0;
    driver.drag(divider_x, TABLE.y, divider_x + 4.0, TABLE.y);
    let resized: Vec<_> = applied(&driver)
        .iter()
        .filter_map(|e| match e {
            DataTableEvent::ColumnResized { col, width } => Some((*col, *width)),
            _ => None,
        })
        .collect();
    let (col, width) = *resized.last().expect("a divider drag resizes");
    assert_eq!(col, 0);
    assert!(
        width > 13.0,
        "the column grew past its 13-cell flex share: {width}"
    );
}

#[test]
fn ctrl_and_shift_clicks_grow_a_multi_selection() {
    let mut driver = driver();
    let y = row_y(&driver, 1);
    driver.click(TABLE.x + 2.0, y);
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    let y = row_y(&driver, 3);
    driver.click_with(MouseButton::Left, ctrl, TABLE.x + 2.0, y);
    assert_eq!(
        applied(&driver).last(),
        Some(&DataTableEvent::SelectionSet {
            indices: vec![1, 3]
        })
    );
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    let y = row_y(&driver, 5);
    driver.click_with(MouseButton::Left, shift, TABLE.x + 2.0, y);
    assert_eq!(
        applied(&driver).last(),
        Some(&DataTableEvent::SelectionSet {
            indices: vec![3, 4, 5]
        })
    );
}

#[test]
fn repeated_shift_down_keeps_extending_the_selection_and_scrolls_with_it() {
    let mut driver = driver();
    let y = row_y(&driver, 2);
    driver.click(TABLE.x + 2.0, y);
    let shift_down = || UiEvent::KeyPressed {
        key: quadraui::Key::Named(NamedKey::Down),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
    };
    for _ in 0..3 {
        driver.dispatch(shift_down());
    }
    assert_eq!(
        applied(&driver).last(),
        Some(&DataTableEvent::SelectionSet {
            indices: vec![2, 3, 4, 5]
        })
    );
    // The moving end, not the descriptor's `selected_idx`, drives the
    // scroll: row 9 sits one past the nine-row viewport.
    for _ in 0..4 {
        driver.dispatch(shift_down());
    }
    assert_eq!(
        applied(&driver).last(),
        Some(&DataTableEvent::SelectionSet {
            indices: (2..=9).collect()
        })
    );
    assert_eq!(driver.app().widget.scroll_offset, 1);
    // A plain arrow collapses the extension one row past its moving end.
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().widget.selected_idx, Some(10));
}

#[test]
fn a_plain_arrow_back_onto_the_selected_row_collapses_the_extension() {
    let mut driver = driver();
    let y = row_y(&driver, 2);
    driver.click(TABLE.x + 2.0, y);
    driver.dispatch(UiEvent::KeyPressed {
        key: quadraui::Key::Named(NamedKey::Down),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    driver.press_named(NamedKey::Up);
    assert_eq!(
        applied(&driver).last(),
        Some(&DataTableEvent::RowSelected { idx: 2 }),
        "Up lands on the descriptor's own selected row but still collapses [2, 3]"
    );
}

#[test]
fn typing_jumps_to_the_first_matching_row() {
    let mut driver = driver();
    let y = row_y(&driver, 0);
    driver.click(TABLE.x + 2.0, y);
    for c in "row-2".chars() {
        driver.type_char(c);
    }
    assert_eq!(driver.app().widget.selected_idx, Some(20));
}

#[test]
fn wheel_scrolls_and_the_scrollbar_track_pages() {
    let mut driver = driver();
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, -3.0),
        position: Point::new(TABLE.x + 2.0, TABLE.y + 3.0),
    });
    assert_eq!(driver.app().widget.scroll_offset, 3);
    // Below the thumb on the track: one page down.
    let track_x = TABLE.x + TABLE.width - 1.0;
    driver.click(track_x, TABLE.y + TABLE.height - 1.0);
    assert_eq!(driver.app().widget.scroll_offset, 12);
    assert_eq!(
        driver.app().widget.selected_idx,
        None,
        "a track click never selects the row painted beside it"
    );
}

#[test]
fn right_click_requests_the_rows_context_menu() {
    let mut driver = driver();
    let y = row_y(&driver, 2);
    driver.right_click(TABLE.x + 2.0, y);
    assert!(matches!(
        applied(&driver).last(),
        Some(DataTableEvent::ContextMenuRequested { idx: 2, .. })
    ));
}

#[test]
fn striped_descriptor_tints_every_other_row() {
    let mut driver = driver();
    driver.app_mut().widget.striped = true;
    driver.render();
    let theme = quadraui::Theme::default();
    let x = TABLE.x as u16 + 2;
    let even = driver.style_at(x, row_y(&driver, 0) as u16).unwrap();
    let odd = driver.style_at(x, row_y(&driver, 1) as u16).unwrap();
    assert_eq!(even.bg, quadraui::tui::ratatui_color(theme.background));
    assert_eq!(
        odd.bg,
        quadraui::tui::ratatui_color(theme.row_alt_bg(theme.background))
    );
}
