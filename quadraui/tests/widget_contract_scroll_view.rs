#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `ScrollView` on the widget contract: the
//! view reaches the frame as a `Surface::Widget`, paints its scrollbar
//! through `Scrollbar`'s cell painter, and turns wheel, keys and thumb
//! drags into the `UiEvent::ScrollView(_, Scrolled { .. })` this app
//! writes back onto the descriptor's offset.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    NamedKey, Point, Rect, ScrollDelta, ScrollView, ScrollViewEvent, UiEvent, WidgetId,
};

const COLS: u16 = 20;
const ROWS: u16 = 12;
/// Ten columns by eight rows; 32 rows of content.
const AREA: Rect = Rect::new(2.0, 1.0, 10.0, 8.0);
const CONTENT: f32 = 32.0;

fn view_id() -> WidgetId {
    WidgetId::new("scroll:help")
}

type ScrollApp = one_widget::OneWidgetApp<ScrollView>;

fn apply(view: &mut ScrollView, event: &UiEvent) -> bool {
    match event {
        UiEvent::ScrollView(id, ScrollViewEvent::Scrolled { offset }) if *id == view.id => {
            view.scroll_offset = *offset;
            true
        }
        _ => false,
    }
}

fn driver() -> TuiDriver<ScrollApp> {
    one_widget::driver(
        ScrollView::new(view_id(), AREA, CONTENT),
        AREA,
        COLS,
        ROWS,
        apply,
    )
}

fn offsets(driver: &TuiDriver<ScrollApp>) -> Vec<f32> {
    driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::ScrollView(id, ScrollViewEvent::Scrolled { offset }) if *id == view_id() => {
                Some(*offset)
            }
            _ => None,
        })
        .collect()
}

fn cell(driver: &TuiDriver<ScrollApp>, x: u16, y: u16) -> char {
    driver.styled_row(y)[x as usize].0
}

/// The scrollbar column: the view's last column.
const BAR_X: u16 = (AREA.x + AREA.width) as u16 - 1;

#[test]
fn surface_widget_scroll_view_paints_its_scrollbar_in_the_trailing_column() {
    let driver = driver();
    let y0 = AREA.y as u16;
    // 8 of 32 rows visible: a two-cell thumb at the top of an 8-cell track.
    assert_eq!(cell(&driver, BAR_X, y0), '█', "{}", driver.screen());
    assert_eq!(cell(&driver, BAR_X, y0 + 1), '█', "{}", driver.screen());
    assert_eq!(cell(&driver, BAR_X, y0 + 7), '░', "{}", driver.screen());
    // Nothing painted left of the gutter: the host owns the content.
    assert_eq!(cell(&driver, BAR_X - 1, y0), ' ');
}

#[test]
fn content_that_fits_paints_no_scrollbar() {
    let mut driver = driver();
    driver.app_mut().widget.content_len = 4.0;
    driver.render();
    assert_eq!(
        cell(&driver, BAR_X, AREA.y as u16),
        ' ',
        "{}",
        driver.screen()
    );
}

#[test]
fn wheel_down_scrolls_forward_and_wheel_up_back() {
    let mut driver = driver();
    let at = Point::new(AREA.x + 1.0, AREA.y + 1.0);
    let wheel = |dy: f32| UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, dy),
        position: at,
    };
    // A positive delta is the wheel turned up, toward the start.
    driver.dispatch(wheel(-2.0));
    driver.dispatch(wheel(1.0));
    assert_eq!(offsets(&driver), vec![2.0, 1.0]);
}

#[test]
fn focused_keys_page_and_jump() {
    let mut driver = driver();
    driver.click(AREA.x + 1.0, AREA.y + 1.0);
    driver.press_named(NamedKey::PageDown);
    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::End);
    driver.press_named(NamedKey::Home);
    assert_eq!(offsets(&driver), vec![8.0, 9.0, 24.0, 0.0]);
}

#[test]
fn dragging_the_thumb_scrolls_proportionally() {
    let mut driver = driver();
    let y0 = AREA.y;
    driver.mouse_down(BAR_X as f32, y0);
    // The thumb (2 cells) travels 6 cells over 24 rows of offset.
    driver.mouse_move(BAR_X as f32, y0 + 3.0);
    // Past the end of the track, wherever the pointer goes.
    driver.mouse_move(BAR_X as f32 + 5.0, y0 + 40.0);
    driver.mouse_up(BAR_X as f32 + 5.0, y0 + 40.0);
    assert_eq!(offsets(&driver), vec![12.0, 24.0]);
}
