#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Checkbox` on the widget contract: the
//! checkbox reaches the frame as a `Surface::Widget`, paints through the
//! form's own toggle painter, and every gesture routes through the shared
//! widget-contract engine and back out as the `FormEvent::ToggleChanged`
//! this app applies onto `Checkbox::value`.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::{Checkbox, FormEvent, NamedKey, Rect, UiEvent, WidgetId};

fn apply(checkbox: &mut Checkbox, event: &UiEvent) -> bool {
    match event {
        UiEvent::Form(_, FormEvent::ToggleChanged { value, .. }) => {
            checkbox.value = *value;
            true
        }
        _ => false,
    }
}

const AREA: Rect = Rect::new(1.0, 1.0, 4.0, 1.0);

#[test]
fn a_click_and_space_flip_the_box_and_repaint_its_glyph() {
    let mut driver = one_widget::driver(Checkbox::new(WidgetId::new("wrap")), AREA, 10, 3, apply);
    assert!(driver.screen_contains("[ ]"), "{}", driver.screen());

    driver.click(AREA.x + 1.0, AREA.y);
    assert!(driver.app().widget.value);
    assert!(driver.screen_contains("[x]"), "{}", driver.screen());

    driver.type_char(' ');
    assert!(!driver.app().widget.value);
    driver.press_named(NamedKey::Enter);
    assert!(driver.app().widget.value);
    assert!(driver.screen_contains("[x]"), "{}", driver.screen());
}

#[test]
fn a_disabled_checkbox_ignores_a_click() {
    let mut checkbox = Checkbox::new(WidgetId::new("wrap"));
    checkbox.disabled = true;
    let mut driver = one_widget::driver(checkbox, AREA, 10, 3, apply);
    driver.click(AREA.x + 1.0, AREA.y);
    assert!(driver
        .app()
        .events
        .iter()
        .all(|e| !matches!(e, UiEvent::Form(..))));
}
