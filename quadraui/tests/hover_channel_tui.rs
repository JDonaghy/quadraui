#![cfg(feature = "tui")]
//! Black-box hover/press test on `TuiDriver`: every widget on
//! [`hover_board::HoverBoard`] is painted through `draw_dyn(.., &())`,
//! the pointer moves and presses through the driver's production input
//! path, and each assertion reads the cell styles the terminal buffer
//! holds — see `hover_board::check` for the steps.
//!
//! Twins: `hover_channel_gtk.rs`, `hover_channel_macos.rs`,
//! `hover_channel_win.rs`.

#[path = "common/hover_board.rs"]
mod hover_board;

use hover_board::{BoardLayout, Grab, HoverBoard, HoverDriver, Target};
use quadraui::tui::testing::{CellStyle, TuiDriver};
use quadraui::{
    AppLogic, Backend, Button, ButtonMask, Color, DrawsPrimitives, Point, Reaction, Rect, Theme,
    UiEvent, WidgetId,
};

struct Tui(TuiDriver<HoverBoard>);

impl HoverDriver for Tui {
    type Sample = CellStyle;
    const INSET: f32 = 0.0;

    fn move_to(&mut self, x: f32, y: f32) {
        self.0.mouse_move(x, y);
    }

    fn press(&mut self, x: f32, y: f32) {
        self.0.mouse_down(x, y);
    }

    fn release(&mut self, x: f32, y: f32) {
        self.0.mouse_up(x, y);
    }

    fn text_bounds(&mut self, needle: &str) -> Option<Rect> {
        self.0.find_bounds(needle)
    }

    fn theme(&self) -> Theme {
        self.0.backend().theme()
    }

    fn grab(&mut self) -> Grab<CellStyle> {
        let (w, h) = (BoardLayout::CELL_COLS, BoardLayout::CELL_ROWS);
        let mut samples = Vec::with_capacity(w as usize * h as usize);
        for y in 0..h {
            for x in 0..w {
                samples.push(self.0.style_at(x, y).expect("cell inside the grid"));
            }
        }
        Grab {
            width: w as usize,
            height: h as usize,
            samples,
        }
    }

    fn differs(a: CellStyle, b: CellStyle) -> bool {
        a != b
    }

    /// Cell tints are terminal colours chosen per widget, not the pixel
    /// backends' theme blends, so a cell probe asserts change, not colour.
    fn rgb(_: CellStyle) -> Option<Color> {
        None
    }
}

fn run(target: Target) {
    hover_board::fresh_pointer();
    let layout = BoardLayout::cells();
    let mut driver = Tui(TuiDriver::new(
        HoverBoard::new(layout),
        BoardLayout::CELL_COLS,
        BoardLayout::CELL_ROWS,
    ));
    hover_board::check(&mut driver, layout.off, target);
}

#[test]
fn hover_channel_tui_status_bar() {
    run(hover_board::status_bar());
}

#[test]
fn hover_channel_tui_toolbar_action() {
    run(hover_board::toolbar_action());
}

#[test]
fn hover_channel_tui_toolbar_group_child() {
    run(hover_board::toolbar_group_child());
}

#[test]
fn hover_channel_tui_toolbar_menu_button() {
    run(hover_board::toolbar_menu_button());
}

#[test]
fn hover_channel_tui_sidebar_header() {
    run(hover_board::sidebar_header());
}

#[test]
fn hover_channel_tui_menu_bar() {
    run(hover_board::menu_bar());
}

#[test]
fn hover_channel_tui_context_menu() {
    run(hover_board::context_menu());
}

#[test]
fn hover_channel_tui_palette_create_row() {
    run(hover_board::palette_create_row());
}

#[test]
fn hover_channel_tui_activity_bar() {
    run(hover_board::activity_bar_item());
}

#[test]
fn hover_channel_tui_button() {
    run(hover_board::button());
}

#[test]
fn hover_channel_tui_tab_bar() {
    run(hover_board::tab_bar());
}

/// The cell styles across `needle`'s painted bounds.
fn styles_over(driver: &TuiDriver<HoverBoard>, needle: &str) -> Vec<CellStyle> {
    let r = driver.find_bounds(needle).expect("the label is painted");
    (r.x as u16..(r.x + r.width) as u16)
        .map(|x| {
            driver
                .style_at(x, r.y as u16)
                .expect("cell inside the grid")
        })
        .collect()
}

fn hover_board_driver() -> TuiDriver<HoverBoard> {
    hover_board::fresh_pointer();
    TuiDriver::new(
        HoverBoard::new(BoardLayout::cells()),
        BoardLayout::CELL_COLS,
        BoardLayout::CELL_ROWS,
    )
}

/// A move with no button held.
fn bare_move(x: f32, y: f32) -> UiEvent {
    UiEvent::MouseMoved {
        position: Point::new(x, y),
        buttons: ButtonMask::default(),
    }
}

/// A press whose release this window never sees (the button let go
/// outside it) ends at the next move with no button held: back over the
/// button, it paints the hover tint, not the press look.
#[test]
fn hover_channel_tui_a_release_outside_the_window_ends_the_press() {
    let mut driver = hover_board_driver();
    let (x, y) = driver.find("Build").expect("the toolbar action is painted");
    let off = BoardLayout::cells().off;
    driver.dispatch(bare_move(x, y));
    let hovered = styles_over(&driver, "Build");
    driver.mouse_down(x, y);
    let pressed = styles_over(&driver, "Build");
    assert_ne!(pressed, hovered, "the press paints its own look");

    driver.dispatch(bare_move(off.0, off.1));
    driver.dispatch(bare_move(x, y));
    assert_eq!(styles_over(&driver, "Build"), hovered);
}

/// The second press of a double-click paints the press look, the same as
/// a single press.
#[test]
fn hover_channel_tui_a_double_click_shows_the_press_look() {
    let mut driver = hover_board_driver();
    let (x, y) = driver.find("Build").expect("the toolbar action is painted");
    driver.dispatch(bare_move(x, y));
    driver.mouse_down(x, y);
    let pressed = styles_over(&driver, "Build");
    driver.mouse_up(x, y);
    let hovered = styles_over(&driver, "Build");
    assert_ne!(pressed, hovered);

    driver.double_click(x, y);
    assert_eq!(styles_over(&driver, "Build"), pressed);
}

/// A button that a scroll moves out from under a still pointer.
struct Shifting {
    button: Button,
    row: f32,
}

impl AppLogic for Shifting {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_dyn(Rect::new(2.0, self.row, 12.0, 1.0), &self.button, &());
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Scroll { .. } => {
                self.row = 6.0;
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

/// Content that moves under a still pointer takes the hover tint with
/// it only while it is still under the pointer: once the scroll has moved
/// the button away, it paints at rest.
#[test]
fn hover_channel_tui_a_scroll_moving_content_leaves_the_tint_under_the_pointer() {
    hover_board::fresh_pointer();
    let button = || Button::new(WidgetId::new("hover:shifting"), "Deploy");
    let mut rest = TuiDriver::new(
        Shifting {
            button: button(),
            row: 6.0,
        },
        20,
        10,
    );
    rest.render();
    let at_rest = rest.style_at(6, 6).expect("cell inside the grid");

    hover_board::fresh_pointer();
    let mut driver = TuiDriver::new(
        Shifting {
            button: button(),
            row: 2.0,
        },
        20,
        10,
    );
    driver.render();
    driver.dispatch(bare_move(6.5, 2.5));
    assert_ne!(
        driver.style_at(6, 2).expect("cell inside the grid"),
        at_rest,
        "the pointer hovers the button"
    );
    driver.dispatch(quadraui::scroll(0.0, 1.0, 6.5, 2.5));
    assert!(driver.find_bounds("Deploy").is_some_and(|r| r.y == 6.0));
    assert_eq!(
        driver.style_at(6, 6).expect("cell inside the grid"),
        at_rest
    );
}
