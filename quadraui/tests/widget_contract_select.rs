#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Select` on the widget contract: the
//! select paints the chosen option through the form's own dropdown
//! painter, and Down/Up and a press move the choice — each reaching this
//! app as the `FormEvent::DropdownChanged` it applies onto
//! `Select::selected_idx`.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::{FormEvent, NamedKey, Rect, Select, UiEvent, WidgetId};

fn apply(select: &mut Select, event: &UiEvent) -> bool {
    match event {
        UiEvent::Form(_, FormEvent::DropdownChanged { selected_idx, .. }) => {
            select.selected_idx = *selected_idx;
            true
        }
        _ => false,
    }
}

const AREA: Rect = Rect::new(0.0, 0.0, 16.0, 1.0);

#[test]
fn a_press_and_the_arrow_keys_move_the_painted_choice() {
    let select = Select::new(
        WidgetId::new("theme"),
        vec!["Light".into(), "Dark".into(), "Contrast".into()],
    );
    let mut driver = one_widget::driver(select, AREA, 16, 1, apply);
    assert!(driver.screen_contains("Light"), "{}", driver.screen());

    driver.click(2.0, 0.0);
    assert_eq!(driver.app().widget.selected_idx, 1);
    assert!(driver.screen_contains("Dark"), "{}", driver.screen());

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().widget.selected_idx, 2);
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().widget.selected_idx, 2);
    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().widget.selected_idx, 1);
}
