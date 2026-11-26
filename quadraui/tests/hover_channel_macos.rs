#![cfg(all(feature = "macos", target_os = "macos"))]
//! Black-box hover/press test on `MacDriver`: every widget on
//! [`hover_board::HoverBoard`] is painted through `draw_dyn(.., &())`,
//! the pointer moves and presses through the driver's production input
//! path, and each assertion reads the CoreGraphics bitmap's pixels back — see
//! `hover_board::check` for the steps.
//!
//! Twins: `hover_channel_tui.rs`, `hover_channel_gtk.rs`,
//! `hover_channel_win.rs`.

#[path = "common/hover_board.rs"]
mod hover_board;

use hover_board::{BoardLayout, Grab, HoverBoard, HoverDriver, Target};
use quadraui::macos::testing::MacDriver;
use quadraui::{Backend, Color, Rect, Theme};

struct Mac(MacDriver<HoverBoard>);

impl HoverDriver for Mac {
    type Sample = Color;
    const INSET: f32 = 2.0;

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

    fn grab(&mut self) -> Grab<Color> {
        let (w, h) = (BoardLayout::PIXEL_WIDTH, BoardLayout::PIXEL_HEIGHT);
        let mut samples = Vec::with_capacity(w as usize * h as usize);
        for y in 0..h {
            for x in 0..w {
                let (r, g, b, _) = self.0.pixel(x, y);
                samples.push(Color::rgb(r, g, b));
            }
        }
        Grab {
            width: w as usize,
            height: h as usize,
            samples,
        }
    }

    fn differs(a: Color, b: Color) -> bool {
        hover_board::pixels_differ(a, b)
    }

    fn rgb(sample: Color) -> Option<Color> {
        Some(sample)
    }
}

fn run(target: Target) {
    hover_board::fresh_pointer();
    let layout = BoardLayout::pixels();
    let mut driver = Mac(MacDriver::new(
        HoverBoard::new(layout),
        BoardLayout::PIXEL_WIDTH,
        BoardLayout::PIXEL_HEIGHT,
    ));
    hover_board::check(&mut driver, layout.off, target);
}

#[test]
fn hover_channel_macos_status_bar() {
    run(hover_board::status_bar());
}

#[test]
fn hover_channel_macos_toolbar_action() {
    run(hover_board::toolbar_action());
}

#[test]
fn hover_channel_macos_toolbar_group_child() {
    run(hover_board::toolbar_group_child());
}

#[test]
fn hover_channel_macos_toolbar_menu_button() {
    run(hover_board::toolbar_menu_button());
}

#[test]
fn hover_channel_macos_sidebar_header() {
    run(hover_board::sidebar_header());
}

#[test]
fn hover_channel_macos_menu_bar() {
    run(hover_board::menu_bar());
}

#[test]
fn hover_channel_macos_context_menu() {
    run(hover_board::context_menu());
}

#[test]
fn hover_channel_macos_palette_create_row() {
    run(hover_board::palette_create_row());
}

#[test]
fn hover_channel_macos_activity_bar() {
    run(hover_board::activity_bar_item());
}

#[test]
fn hover_channel_macos_button() {
    run(hover_board::button());
}

#[test]
fn hover_channel_macos_tab_bar() {
    run(hover_board::tab_bar());
}
