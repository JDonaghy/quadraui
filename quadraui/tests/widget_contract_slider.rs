#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Slider` on the widget contract: the
//! slider paints through the form's own slider painter, a press maps to
//! the value under the pointer on the painted track, a drag keeps
//! tracking it, and the keys step — each reaching this app as the
//! `FormEvent::SliderChanged` it applies onto `Slider::value`.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::{FormEvent, NamedKey, Rect, Slider, UiEvent, WidgetId};

fn apply(slider: &mut Slider, event: &UiEvent) -> bool {
    match event {
        UiEvent::Form(_, FormEvent::SliderChanged { value, .. }) => {
            slider.value = *value;
            true
        }
        _ => false,
    }
}

const AREA: Rect = Rect::new(0.0, 0.0, 24.0, 1.0);

fn track_cols(driver_screen: &str) -> (usize, usize) {
    let row = driver_screen.lines().next().unwrap_or("");
    let open = row.find('[').expect("track opens with '['");
    let close = row.find(']').expect("track closes with ']'");
    (open + 1, close)
}

#[test]
fn a_press_on_the_track_sets_the_painted_value_and_a_drag_follows() {
    let mut driver = one_widget::driver(
        Slider::new(WidgetId::new("vol"), 0.0, 0.0, 10.0),
        AREA,
        24,
        1,
        apply,
    );
    let (start, end) = track_cols(&driver.screen());
    let mid = (start + end) / 2;

    driver.mouse_down(mid as f32, 0.0);
    let pressed = driver.app().widget.value;
    assert!(pressed > 3.0 && pressed < 7.0, "{pressed}");

    driver.mouse_move(end as f32 + 5.0, 0.0);
    assert_eq!(driver.app().widget.value, 10.0);
    driver.mouse_up(end as f32 + 5.0, 0.0);
    assert!(driver.screen_contains("10.00"), "{}", driver.screen());

    driver.press_named(NamedKey::Left);
    assert_eq!(driver.app().widget.value, 9.0);
    driver.press_named(NamedKey::Home);
    assert_eq!(driver.app().widget.value, 0.0);
}
