#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `PasswordInput` on the widget contract:
//! typed text reaches this app as `FormEvent::TextInputChanged`, the
//! screen only ever shows the mask glyph, Enter commits, and a copy chord
//! never puts the value on the clipboard.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::{FormEvent, Key, Modifiers, NamedKey, PasswordInput, Rect, UiEvent, WidgetId};

fn apply(input: &mut PasswordInput, event: &UiEvent) -> bool {
    match event {
        UiEvent::Form(_, FormEvent::TextInputChanged { value, .. }) => {
            input.value = value.clone();
            input.cursor = Some(value.len());
            true
        }
        _ => false,
    }
}

const AREA: Rect = Rect::new(0.0, 0.0, 16.0, 1.0);

#[test]
fn typing_is_masked_and_copy_never_leaks_the_value() {
    let mut input = PasswordInput::new(WidgetId::new("pw"));
    input.mask_char = '*';
    let mut driver = one_widget::driver(input, AREA, 16, 1, apply);
    driver.click(2.0, 0.0);
    for c in "abc".chars() {
        driver.type_char(c);
    }
    assert_eq!(driver.app().widget.value, "abc");
    assert!(driver.screen_contains("***"), "{}", driver.screen());
    assert!(!driver.screen_contains("abc"), "{}", driver.screen());

    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    for c in ['a', 'c'] {
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Char(c),
            modifiers: ctrl,
            repeat: false,
        });
    }
    assert!(
        driver
            .app()
            .events
            .iter()
            .all(|e| !matches!(e, UiEvent::TextCopied(_))),
        "{:?}",
        driver.app().events
    );

    driver.press_named(NamedKey::Enter);
    assert!(driver.app().events.iter().any(|e| matches!(
        e,
        UiEvent::Form(_, FormEvent::TextInputCommitted { value, .. }) if value == "abc"
    )));
}
