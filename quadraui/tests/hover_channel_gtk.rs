#![cfg(feature = "gtk")]
//! Black-box hover/press test on `GtkDriver`: every widget on
//! [`hover_board::HoverBoard`] is painted through `draw_dyn(.., &())`,
//! the pointer moves and presses through the driver's production input
//! path, and each assertion reads the Cairo surface's pixels back — see
//! `hover_board::check` for the steps.
//!
//! Twins: `hover_channel_tui.rs`, `hover_channel_macos.rs`,
//! `hover_channel_win.rs`.

#[path = "common/hover_board.rs"]
mod hover_board;

use hover_board::{BoardLayout, Grab, HoverBoard, HoverDriver, Target};
use quadraui::gtk::testing::GtkDriver;
use quadraui::{Backend, Color, Rect, Theme};

struct Gtk(GtkDriver<HoverBoard>);

impl HoverDriver for Gtk {
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

    /// Cairo `ARgb32` is `[B, G, R, A]` per pixel in memory.
    fn grab(&mut self) -> Grab<Color> {
        let (w, h) = (
            BoardLayout::PIXEL_WIDTH as usize,
            BoardLayout::PIXEL_HEIGHT as usize,
        );
        let stride = self.0.stride() as usize;
        let data = self.0.screen();
        let mut samples = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let o = y * stride + x * 4;
                samples.push(Color::rgb(data[o + 2], data[o + 1], data[o]));
            }
        }
        Grab {
            width: w,
            height: h,
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
    let mut driver = Gtk(GtkDriver::new(
        HoverBoard::new(layout),
        BoardLayout::PIXEL_WIDTH as i32,
        BoardLayout::PIXEL_HEIGHT as i32,
    ));
    hover_board::check(&mut driver, layout.off, target);
}

#[test]
fn hover_channel_gtk_status_bar() {
    run(hover_board::status_bar());
}

#[test]
fn hover_channel_gtk_toolbar_action() {
    run(hover_board::toolbar_action());
}

#[test]
fn hover_channel_gtk_toolbar_group_child() {
    run(hover_board::toolbar_group_child());
}

#[test]
fn hover_channel_gtk_toolbar_menu_button() {
    run(hover_board::toolbar_menu_button());
}

#[test]
fn hover_channel_gtk_sidebar_header() {
    run(hover_board::sidebar_header());
}

#[test]
fn hover_channel_gtk_menu_bar() {
    run(hover_board::menu_bar());
}

#[test]
fn hover_channel_gtk_context_menu() {
    run(hover_board::context_menu());
}

#[test]
fn hover_channel_gtk_palette_create_row() {
    run(hover_board::palette_create_row());
}

#[test]
fn hover_channel_gtk_activity_bar() {
    run(hover_board::activity_bar_item());
}

#[test]
fn hover_channel_gtk_button() {
    run(hover_board::button());
}

#[test]
fn hover_channel_gtk_tab_bar() {
    run(hover_board::tab_bar());
}
