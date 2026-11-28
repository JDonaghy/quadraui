//! Screen pins for every shipped `Form` / `FormDialog` demo: each demo is
//! driven through a fixed script of keys, clicks, wheel notches and
//! scrollbar gestures, and every step's painted cells (glyphs and styles)
//! must match the fixture under `tests/fixtures/form_demo_pins/`.
//!
//! The behavioural assertions for these demos live in
//! `tests/tui_example_driver.rs`; these pins catch any change in what
//! the demos paint along the way.
#![cfg(feature = "tui")]

#[path = "common/screen_pin.rs"]
mod screen_pin;

#[path = "../examples/common/form_combo_box.rs"]
mod form_combo_box;
#[path = "../examples/common/form_date_picker.rs"]
mod form_date_picker;
#[path = "../examples/common/form_dialog_demo.rs"]
mod form_dialog_demo;
#[path = "../examples/common/form_dropdown.rs"]
mod form_dropdown;
#[path = "../examples/common/form_label_placement.rs"]
mod form_label_placement;
#[path = "../examples/common/form_scroll.rs"]
mod form_scroll;
#[path = "../examples/common/form_visibility.rs"]
mod form_visibility;

use quadraui::tui::testing::TuiDriver;
use quadraui::{AppLogic, NamedKey, Point, ScrollDelta, UiEvent};
use screen_pin::ScreenPins;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/form_demo_pins");

/// One wheel notch at `(x, y)`; a negative `dy` scrolls the content up
/// (towards later rows).
fn wheel<A: AppLogic>(driver: &mut TuiDriver<A>, x: f32, y: f32, dy: f32) {
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, dy),
        position: Point::new(x, y),
    });
}

/// The first row whose cell in column `x` paints `glyph`.
fn row_with_glyph_at<A: AppLogic>(driver: &TuiDriver<A>, x: usize, glyph: char) -> Option<usize> {
    driver
        .screen()
        .lines()
        .position(|line| line.chars().nth(x) == Some(glyph))
}

fn click_text<A: AppLogic>(driver: &mut TuiDriver<A>, text: &str) {
    let (x, y) = driver
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} not painted:\n{}", driver.screen()));
    driver.click(x, y);
}

#[test]
fn form_scroll_demo_paints_the_pinned_screens() {
    let mut driver = TuiDriver::new(form_scroll::FormScrollApp::new(), 100, 14);
    driver.set_double_click_folding(false);
    let mut pins = ScreenPins::new(FIXTURES, "form_scroll");
    pins.step("initial", &driver);
    for c in "Joe".chars() {
        driver.type_char(c);
    }
    pins.step("typed", &driver);
    driver.press_named(NamedKey::Backspace);
    driver.press_named(NamedKey::Enter);
    pins.step("committed", &driver);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Tab);
    pins.step("tabbed", &driver);
    driver.type_char(' ');
    pins.step("space_toggles", &driver);
    wheel(&mut driver, 40.0, 5.0, -3.0);
    pins.step("wheel_down", &driver);
    wheel(&mut driver, 40.0, 5.0, 1.0);
    pins.step("wheel_up", &driver);
    click_text(&mut driver, "Setting 9");
    pins.step("click_toggle", &driver);

    let thumb = row_with_glyph_at(&driver, 99, '█').expect("a scrollbar thumb in column 99");
    driver.mouse_down(99.0, thumb as f32);
    driver.mouse_move(99.0, thumb as f32 + 6.0);
    pins.step("thumb_dragging", &driver);
    driver.mouse_up(99.0, thumb as f32 + 6.0);
    pins.step("thumb_released", &driver);
    driver.click(99.0, 0.0);
    pins.step("track_page_up", &driver);
    driver.click(99.0, 11.0);
    pins.step("track_page_down", &driver);
    pins.finish();
}

#[test]
fn form_visibility_demo_paints_the_pinned_screens() {
    let mut driver = TuiDriver::new(form_visibility::FormVisibilityApp::new(), 100, 24);
    driver.set_double_click_folding(false);
    let mut pins = ScreenPins::new(FIXTURES, "form_visibility");
    pins.step("initial", &driver);
    click_text(&mut driver, "Ship to a different address");
    pins.step("ship_on", &driver);
    driver.press_named(NamedKey::Tab);
    for c in "1 Main St".chars() {
        driver.type_char(c);
    }
    pins.step("address_typed", &driver);
    click_text(&mut driver, "Submit");
    pins.step("submit_disabled", &driver);
    click_text(&mut driver, "I agree to the terms");
    pins.step("agreed", &driver);
    click_text(&mut driver, "Submit");
    pins.step("submitted", &driver);
    click_text(&mut driver, "Ship to a different address");
    pins.step("ship_off", &driver);
    driver.press_named(NamedKey::BackTab);
    pins.step("backtab", &driver);
    pins.finish();
}

#[test]
fn form_combo_box_demo_paints_the_pinned_screens() {
    let mut driver = TuiDriver::new(form_combo_box::FormComboBoxApp::new(), 100, 24);
    let mut pins = ScreenPins::new(FIXTURES, "form_combo_box");
    pins.step("initial", &driver);
    driver.type_char('a');
    pins.step("typed_open", &driver);
    driver.press_named(NamedKey::Down);
    pins.step("highlight", &driver);
    driver.press_named(NamedKey::Enter);
    pins.step("committed", &driver);
    driver.type_char('c');
    pins.step("retyped", &driver);
    driver.press_named(NamedKey::Escape);
    pins.step("cancelled", &driver);
    pins.finish();
}

#[test]
fn form_dropdown_demo_paints_the_pinned_screens() {
    let mut driver = TuiDriver::new(form_dropdown::FormDropdownApp::new(), 100, 24);
    let mut pins = ScreenPins::new(FIXTURES, "form_dropdown");
    pins.step("initial", &driver);
    driver.press_named(NamedKey::Down);
    pins.step("down", &driver);
    driver.press_named(NamedKey::Down);
    pins.step("down_again", &driver);
    driver.press_named(NamedKey::Up);
    pins.step("up", &driver);
    pins.finish();
}

#[test]
fn form_date_picker_demo_paints_the_pinned_screens() {
    let mut driver = TuiDriver::new(form_date_picker::FormDatePickerApp::new(), 100, 24);
    let mut pins = ScreenPins::new(FIXTURES, "form_date_picker");
    pins.step("initial", &driver);
    driver.press_named(NamedKey::Right);
    driver.press_named(NamedKey::Down);
    pins.step("scrubbed", &driver);
    driver.press_named(NamedKey::PageDown);
    pins.step("next_month", &driver);
    driver.press_named(NamedKey::Enter);
    pins.step("confirmed", &driver);
    pins.finish();
}

#[test]
fn form_label_placement_demo_paints_the_pinned_screens() {
    let mut driver = TuiDriver::new(form_label_placement::FormLabelPlacementApp::new(), 100, 24);
    driver.set_double_click_folding(false);
    let mut pins = ScreenPins::new(FIXTURES, "form_label_placement");
    pins.step("initial", &driver);
    driver.type_char('7');
    pins.step("typed", &driver);
    driver.press_named(NamedKey::Tab);
    driver.press_named(NamedKey::Tab);
    pins.step("tabbed", &driver);
    for c in "hi".chars() {
        driver.type_char(c);
    }
    pins.step("typed_again", &driver);
    driver.press_named(NamedKey::BackTab);
    pins.step("backtab", &driver);
    pins.finish();
}

#[test]
fn form_dialog_demo_paints_the_pinned_screens() {
    let mut driver = TuiDriver::new(form_dialog_demo::FormDialogApp::new(), 100, 24);
    driver.set_double_click_folding(false);
    let mut pins = ScreenPins::new(FIXTURES, "form_dialog");
    pins.step("initial", &driver);
    for c in "M1".chars() {
        driver.type_char(c);
    }
    pins.step("name_typed", &driver);
    driver.press_named(NamedKey::Tab);
    for c in "first".chars() {
        driver.type_char(c);
    }
    pins.step("description_typed", &driver);
    driver.press_named(NamedKey::Tab);
    pins.step("on_cancel", &driver);
    driver.press_named(NamedKey::Tab);
    pins.step("on_create", &driver);
    driver.press_named(NamedKey::Tab);
    pins.step("wrapped_to_name", &driver);
    driver.press_named(NamedKey::BackTab);
    pins.step("backtab_to_create", &driver);
    driver.press_named(NamedKey::Enter);
    pins.step("created", &driver);
    click_text(&mut driver, "Description");
    pins.step("clicked_description", &driver);
    pins.finish();
}
