//! Screen pins for the gallery's `Form` demo "Scrolling" variant: the
//! 20-toggle settings form is driven through wheel notches, a toggle
//! click, a scrollbar-thumb drag and track paging, and every step's
//! painted cells (glyphs and styles) must match the fixture under
//! `tests/fixtures/form_demo_pins/`.
#![cfg(feature = "tui")]

#[path = "../../quadraui/tests/common/screen_pin.rs"]
mod screen_pin;

use quadraui::tui::testing::{driver_with_shell, TuiDriver};
use quadraui::{AppLogic, Point, ScrollDelta, UiEvent};
use quadraui_gallery::GalleryApp;
use screen_pin::ScreenPins;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/form_demo_pins");

fn click_text<A: AppLogic>(driver: &mut TuiDriver<A>, text: &str) {
    let (x, y) = driver
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} not painted:\n{}", driver.screen()));
    driver.click(x, y);
}

#[test]
fn form_demo_scrolling_variant_paints_the_pinned_screens() {
    let mut driver = driver_with_shell(GalleryApp::new(), GalleryApp::config(), 100, 16);
    driver.set_double_click_folding(false);
    click_text(&mut driver, "Form");
    click_text(&mut driver, "Scrolling");
    let mut pins = ScreenPins::new(FIXTURES, "gallery_form_scrolling");
    pins.step("initial", &driver);

    let (sx, sy) = driver.find("Setting 3").expect("Setting 3 painted");
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, -3.0),
        position: Point::new(sx, sy),
    });
    pins.step("wheel_down", &driver);
    click_text(&mut driver, "Setting 9");
    pins.step("click_toggle", &driver);

    let thumb_x = 99;
    let thumb_y = driver
        .screen()
        .lines()
        .position(|line| line.chars().nth(thumb_x) == Some('█'))
        .expect("a scrollbar thumb in the last column");
    let (x, y) = (thumb_x as f32, thumb_y as f32);
    driver.mouse_down(x, y);
    driver.mouse_move(x, y + 4.0);
    pins.step("thumb_dragging", &driver);
    driver.mouse_up(x, y + 4.0);
    pins.step("thumb_released", &driver);
    driver.click(x, sy);
    pins.step("track_page", &driver);
    pins.finish();
}
