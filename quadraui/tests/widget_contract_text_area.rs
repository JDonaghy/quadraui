#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `TextArea` on the widget contract:
//! typing and Enter edit the value through the shared editing handler and
//! reach this app as `FormEvent::TextInputChanged`, and the area paints
//! its `visible_rows` rows.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::{FormEvent, NamedKey, Rect, TextArea, UiEvent, WidgetId};

fn apply(area: &mut TextArea, event: &UiEvent) -> bool {
    match event {
        UiEvent::Form(_, FormEvent::TextInputChanged { value, .. }) => {
            area.value = value.clone();
            area.cursor = Some(value.len());
            true
        }
        _ => false,
    }
}

const AREA: Rect = Rect::new(0.0, 0.0, 16.0, 3.0);

#[test]
fn typing_and_enter_edit_a_multi_line_value() {
    let mut area = TextArea::new(WidgetId::new("notes"));
    area.value = "ab".into();
    let mut driver = one_widget::driver(area, AREA, 16, 3, apply);
    driver.click(2.0, 0.0);
    driver.press_named(NamedKey::Enter);
    driver.type_char('c');
    assert_eq!(driver.app().widget.value, "ab\nc");
    assert!(driver.screen_contains("ab"), "{}", driver.screen());
}
