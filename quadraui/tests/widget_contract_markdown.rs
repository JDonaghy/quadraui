#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Markdown` on the widget contract: the
//! rendered source paints through the composed `TextDisplay`, a press on
//! a link line reports `UiEvent::RichTextLinkClicked`, and the wheel
//! scrolls through the display's own `TextDisplayEvent::Scrolled`, which
//! this app applies onto `Markdown::scroll_offset`.

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::{Markdown, Point, Rect, ScrollDelta, TextDisplayEvent, UiEvent, WidgetId};

fn apply(md: &mut Markdown, event: &UiEvent) -> bool {
    match event {
        UiEvent::TextDisplay(_, TextDisplayEvent::Scrolled { new_offset }) => {
            md.scroll_offset = *new_offset;
            true
        }
        _ => false,
    }
}

const AREA: Rect = Rect::new(0.0, 0.0, 40.0, 6.0);

fn row_of(screen: &str, needle: &str) -> Option<usize> {
    screen.lines().position(|line| line.contains(needle))
}

#[test]
fn a_press_on_a_link_reports_its_url() {
    let md = Markdown::new(
        WidgetId::new("readme"),
        "Intro text\n\nRead [the docs](https://example.com/docs) now\n",
    );
    let mut driver = one_widget::driver(md, AREA, 40, 6, apply);
    let screen = driver.screen();
    assert!(screen.contains("Intro text"), "{screen}");
    let row = row_of(&screen, "the docs").expect("link line is painted");

    driver.click(2.0, row as f32);
    assert!(
        driver.app().events.iter().any(|e| matches!(
            e,
            UiEvent::RichTextLinkClicked(_, url) if url == "https://example.com/docs"
        )),
        "{:?}",
        driver.app().events
    );
}

#[test]
fn the_wheel_scrolls_the_rendered_document() {
    let source: String = (0..30).map(|i| format!("line {i}\n\n")).collect();
    let mut driver = one_widget::driver(
        Markdown::new(WidgetId::new("readme"), source),
        AREA,
        40,
        6,
        apply,
    );
    assert!(driver.screen_contains("line 0"), "{}", driver.screen());
    for _ in 0..4 {
        driver.dispatch(UiEvent::Scroll {
            widget: None,
            position: Point::new(2.0, 2.0),
            delta: ScrollDelta { x: 0.0, y: -1.0 },
        });
    }
    assert_eq!(driver.app().widget.scroll_offset, 4);
    assert!(!driver.screen_contains("line 0"), "{}", driver.screen());
}
