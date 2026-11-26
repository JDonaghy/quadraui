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
use quadraui::{Backend, Color, Rect, Theme};

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
