#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `ScrollView` on the widget contract: the
//! view reaches the frame as a `Surface::Widget`, paints its scrollbar
//! through `Scrollbar`'s cell painter, and turns wheel, keys and thumb
//! drags into the `UiEvent::ScrollView(_, Scrolled { .. })` this app
//! writes back onto the descriptor's offset.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, NamedKey, Point, Reaction, Rect, ScreenLayout, ScrollDelta,
    ScrollView, ScrollViewEvent, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 20;
const ROWS: u16 = 12;
/// Ten columns by eight rows; 32 rows of content.
const AREA: Rect = Rect::new(2.0, 1.0, 10.0, 8.0);
const CONTENT: f32 = 32.0;

fn view_id() -> WidgetId {
    WidgetId::new("scroll:help")
}

struct ScrollApp {
    view: ScrollView,
    offsets: Vec<f32>,
}

impl AppLogic for ScrollApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.view.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.view.id, AREA, Box::new(self.view.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::ScrollView(id, ScrollViewEvent::Scrolled { offset }) = event {
            if id == view_id() {
                self.view.scroll_offset = offset;
                self.offsets.push(offset);
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }
}

fn driver() -> TuiDriver<ScrollApp> {
    TuiDriver::new(
        ScrollApp {
            view: ScrollView::new(view_id(), AREA, CONTENT),
            offsets: Vec::new(),
        },
        COLS,
        ROWS,
    )
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
    driver.app_mut().view.content_len = 4.0;
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
    assert_eq!(driver.app().offsets, vec![2.0, 1.0]);
}

#[test]
fn focused_keys_page_and_jump() {
    let mut driver = driver();
    driver.click(AREA.x + 1.0, AREA.y + 1.0);
    driver.press_named(NamedKey::PageDown);
    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::End);
    driver.press_named(NamedKey::Home);
    assert_eq!(driver.app().offsets, vec![8.0, 9.0, 24.0, 0.0]);
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
    assert_eq!(driver.app().offsets, vec![12.0, 24.0]);
}
