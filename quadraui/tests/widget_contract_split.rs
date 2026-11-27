#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Split` on the widget contract: the
//! split reaches the frame as a type-erased `Surface::Widget`, paints its
//! divider through `Primitive::paint_cells`, and a drag on the divider
//! routes through `Interactive::handle` back out as the
//! `UiEvent::SplitDividerDragged` this app writes onto `ratio`.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::tui::testing::TuiDriver;
use quadraui::{Rect, Split, SplitDirection, UiEvent, WidgetId};

const COLS: u16 = 50;
const ROWS: u16 = 12;
/// 41 columns: 40 to share once the one-cell divider is taken.
const AREA: Rect = Rect::new(3.0, 1.0, 41.0, 8.0);

fn split_id() -> WidgetId {
    WidgetId::new("split:main")
}

type SplitApp = one_widget::OneWidgetApp<Split>;

fn apply(split: &mut Split, event: &UiEvent) -> bool {
    match event {
        UiEvent::SplitDividerDragged {
            tree, new_ratio, ..
        } if *tree == split.id => {
            split.ratio = *new_ratio;
            true
        }
        _ => false,
    }
}

fn driver(direction: SplitDirection) -> TuiDriver<SplitApp> {
    one_widget::driver(
        Split {
            id: split_id(),
            direction,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        },
        AREA,
        COLS,
        ROWS,
        apply,
    )
}

fn drags(driver: &TuiDriver<SplitApp>) -> Vec<(usize, f32)> {
    driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::SplitDividerDragged {
                tree,
                split_index,
                new_ratio,
            } if *tree == split_id() => Some((*split_index, *new_ratio)),
            _ => None,
        })
        .collect()
}

fn cell(driver: &TuiDriver<SplitApp>, x: u16, y: u16) -> char {
    driver.styled_row(y)[x as usize].0
}

#[test]
fn surface_widget_split_paints_its_divider_column() {
    let driver = driver(SplitDirection::Horizontal);
    // 0.5 of 40 shared cells puts the divider 20 cells in.
    let x = AREA.x as u16 + 20;
    for y in AREA.y as u16..(AREA.y + AREA.height) as u16 {
        assert_eq!(cell(&driver, x, y), '│', "{}", driver.screen());
    }
    assert_eq!(cell(&driver, x - 1, AREA.y as u16), ' ');
    assert_eq!(cell(&driver, x + 1, AREA.y as u16), ' ');
}

#[test]
fn dragging_the_divider_lands_it_under_the_pointer() {
    let mut driver = driver(SplitDirection::Horizontal);
    let divider_x = AREA.x + 20.0;
    let y = AREA.y + 3.0;
    driver.mouse_down(divider_x, y);
    driver.mouse_move(AREA.x + 30.0, y);
    driver.mouse_up(AREA.x + 30.0, y);

    assert_eq!(drags(&driver).len(), 1, "{:?}", drags(&driver));
    let (split_index, ratio) = drags(&driver)[0];
    assert_eq!(split_index, 0);
    assert!((ratio - 0.75).abs() < 1e-4, "ratio {ratio}");
    driver.render();
    assert_eq!(
        cell(&driver, AREA.x as u16 + 30, y as u16),
        '│',
        "{}",
        driver.screen()
    );
}

#[test]
fn a_stacked_split_drags_along_y() {
    let mut driver = driver(SplitDirection::Vertical);
    // 8 rows less the divider leaves 7: 0.5 truncates to row 3.
    let divider_y = AREA.y + 3.0;
    assert_eq!(cell(&driver, AREA.x as u16, divider_y as u16), '─');
    driver.mouse_down(AREA.x + 5.0, divider_y);
    driver.mouse_move(AREA.x + 5.0, AREA.y + 6.0);
    driver.mouse_up(AREA.x + 5.0, AREA.y + 6.0);
    driver.render();
    assert_eq!(
        cell(&driver, AREA.x as u16, AREA.y as u16 + 6),
        '─',
        "{}",
        driver.screen()
    );
}

#[test]
fn a_press_inside_a_pane_does_not_drag() {
    let mut driver = driver(SplitDirection::Horizontal);
    driver.mouse_down(AREA.x + 5.0, AREA.y + 2.0);
    driver.mouse_move(AREA.x + 30.0, AREA.y + 2.0);
    driver.mouse_up(AREA.x + 30.0, AREA.y + 2.0);
    assert!(drags(&driver).is_empty());
}

#[test]
fn a_move_after_release_drags_nothing() {
    let mut driver = driver(SplitDirection::Horizontal);
    let y = AREA.y + 3.0;
    driver.mouse_down(AREA.x + 20.0, y);
    driver.mouse_up(AREA.x + 20.0, y);
    driver.mouse_move(AREA.x + 10.0, y);
    assert!(drags(&driver).is_empty());
}
