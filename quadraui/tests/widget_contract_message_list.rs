#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `MessageList` on the widget contract:
//! the list reaches the frame as a type-erased `Surface::Widget`, paints
//! its rows through `Primitive::paint_cells`, and the wheel routes through
//! `Interactive::handle` back out as `UiEvent::MessageList` — with
//! `at_bottom` for a host that follows the tail. `scroll_top` stays the
//! app's (controlled mode).

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Color, FrameTree, MessageList, MessageListEvent, MessageRow, Point,
    Reaction, Rect, ScreenLayout, ScrollDelta, Surface, UiEvent, WidgetId,
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

struct ChatApp {
    list: MessageList,
    following: bool,
    events: Vec<MessageListEvent>,
}

impl AppLogic for ChatApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.list.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.list.id, AREA, Box::new(self.list.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::MessageList(id, event) = event {
            if id == list_id() {
                let MessageListEvent::Scrolled {
                    scroll_top,
                    at_bottom,
                } = event;
                self.list.scroll_top = scroll_top;
                self.following = at_bottom;
                self.events.push(event);
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }
}

fn driver(rows: usize) -> TuiDriver<ChatApp> {
    TuiDriver::new(
        ChatApp {
            list: list(rows),
            following: false,
            events: Vec::new(),
        },
        COLS,
        ROWS,
    )
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
        driver.app().events,
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
    assert!(driver.app().following);
    assert!(driver.screen_contains("message 9"), "{}", driver.screen());
    // At the bottom a further notch down changes nothing.
    wheel(&mut driver, -1.0);
    assert_eq!(driver.app().events.len(), 2);
    // Scrolling back up leaves the tail.
    wheel(&mut driver, 1.0);
    assert!(!driver.app().following);
    assert_eq!(driver.app().list.scroll_top, 1);
}

#[test]
fn a_list_that_fits_does_not_scroll() {
    let mut driver = driver(3);
    wheel(&mut driver, -1.0);
    assert!(driver.app().events.is_empty());
}
