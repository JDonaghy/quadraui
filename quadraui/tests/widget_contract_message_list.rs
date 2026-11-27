#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `MessageList` on the widget contract:
//! the list reaches the frame as a type-erased `Surface::Widget`, paints
//! its rows through `Primitive::paint_cells`, and the wheel routes through
//! `Interactive::handle` back out as `UiEvent::MessageList` — with
//! `at_bottom` for a host that follows the tail. `scroll_top` stays the
//! app's (controlled mode).

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    Color, MessageList, MessageListEvent, MessageRow, Point, Rect, ScrollDelta, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 10;
const AREA: Rect = Rect::new(1.0, 1.0, 30.0, 6.0);

fn list_id() -> WidgetId {
    WidgetId::new("messages:chat")
}

fn list(rows: usize) -> MessageList {
    MessageList {
        id: list_id(),
        rows: (0..rows)
            .map(|i| MessageRow::new(format!("message {i}"), Color::rgb(220, 220, 220), 0.0))
            .collect(),
        scroll_top: 0,
    }
}

type ChatApp = one_widget::OneWidgetApp<MessageList>;

fn apply(list: &mut MessageList, event: &UiEvent) -> bool {
    match event {
        UiEvent::MessageList(id, MessageListEvent::Scrolled { scroll_top, .. })
            if *id == list.id =>
        {
            list.scroll_top = *scroll_top;
            true
        }
        _ => false,
    }
}

fn driver(rows: usize) -> TuiDriver<ChatApp> {
    one_widget::driver(list(rows), AREA, COLS, ROWS, apply)
}

fn events(driver: &TuiDriver<ChatApp>) -> Vec<MessageListEvent> {
    driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::MessageList(id, ev) if *id == list_id() => Some(*ev),
            _ => None,
        })
        .collect()
}

/// Whether the last scroll left the list following its newest row.
fn following(driver: &TuiDriver<ChatApp>) -> bool {
    events(driver)
        .last()
        .is_some_and(|MessageListEvent::Scrolled { at_bottom, .. }| *at_bottom)
}

fn wheel(driver: &mut TuiDriver<ChatApp>, dy: f32) {
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, dy),
        position: Point::new(AREA.x + 2.0, AREA.y + 2.0),
    });
}

#[test]
fn surface_widget_list_paints_its_visible_rows() {
    let driver = driver(10);
    assert!(driver.screen_contains("message 0"), "{}", driver.screen());
    assert!(driver.screen_contains("message 5"), "{}", driver.screen());
    assert!(!driver.screen_contains("message 6"), "{}", driver.screen());
}

#[test]
fn the_wheel_scrolls_and_reports_reaching_the_bottom() {
    let mut driver = driver(10);
    wheel(&mut driver, -1.0);
    wheel(&mut driver, -1.0);
    assert_eq!(
        events(&driver),
        vec![
            MessageListEvent::Scrolled {
                scroll_top: 3,
                at_bottom: false
            },
            MessageListEvent::Scrolled {
                scroll_top: 4,
                at_bottom: true
            },
        ]
    );
    assert!(following(&driver));
    assert!(driver.screen_contains("message 9"), "{}", driver.screen());
    // At the bottom a further notch down changes nothing.
    wheel(&mut driver, -1.0);
    assert_eq!(events(&driver).len(), 2);
    // Scrolling back up leaves the tail.
    wheel(&mut driver, 1.0);
    assert!(!following(&driver));
    assert_eq!(driver.app().widget.scroll_top, 1);
}

#[test]
fn a_list_that_fits_does_not_scroll() {
    let mut driver = driver(3);
    wheel(&mut driver, -1.0);
    assert!(events(&driver).is_empty());
}
