#![cfg(feature = "tui")]
//! Black-box `TuiDriver` tests for `Chat` on the widget contract: the
//! chat reaches the frame as a type-erased `Surface::Widget` and paints
//! each part through its own widget — the status strip, transcript and
//! hint as `MessageList`s, the input as a `TextInput`, the Send/Stop
//! segment as a `Button` — and is registered in the app's `FrameTree`,
//! so pointer, wheel and key input route through the shared engine into
//! `Interactive::handle`.
//!
//! The chat is controlled: this app applies every `ChatEvent` with
//! `Chat::apply`, and the next frame paints the result.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Chat, ChatEvent, ChatRole, ChatTurn, FrameTree, Key, Point, Reaction, Rect,
    ScreenLayout, ScrollDelta, StyledText, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 60;
const ROWS: u16 = 18;
/// A non-zero origin, so every pointer event's chat-local translation is
/// exercised.
const AREA: Rect = Rect::new(2.0, 1.0, 56.0, 16.0);

fn chat_id() -> WidgetId {
    WidgetId::new("chat")
}

fn turn(role: ChatRole, text: &str) -> ChatTurn {
    ChatTurn {
        role,
        text: StyledText::plain(text),
        timestamp_unix: None,
        line_scales: Vec::new(),
    }
}

/// The app that owns the chat and applies what it reports.
struct ChatApp {
    chat: Chat,
    events: Vec<ChatEvent>,
}

impl AppLogic for ChatApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.chat.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.chat.id, AREA, Box::new(self.chat.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::Chat(id, event) = &event {
            assert_eq!(*id, chat_id());
            self.chat.apply(event);
            self.events.push(event.clone());
        }
        Reaction::Redraw
    }
}

fn chat() -> Chat {
    let mut chat = Chat::new(chat_id());
    chat.status = StyledText::plain("Reviewing #42");
    chat.model_label = "opus".into();
    chat.hint = Some(StyledText::plain("Ctrl+S sends"));
    chat.transcript = vec![
        turn(ChatRole::User, "Hello there"),
        turn(ChatRole::Assistant, "General Kenobi"),
    ];
    chat
}

fn driver_with(chat: Chat) -> TuiDriver<ChatApp> {
    TuiDriver::new(
        ChatApp {
            chat,
            events: Vec::new(),
        },
        COLS,
        ROWS,
    )
}

fn driver() -> TuiDriver<ChatApp> {
    driver_with(chat())
}

fn at(d: &TuiDriver<ChatApp>, needle: &str) -> (f32, f32) {
    let b = d
        .find_bounds(needle)
        .unwrap_or_else(|| panic!("{needle} on screen:\n{}", d.screen()));
    (b.x, b.y)
}

/// Click the input box, so it holds focus.
fn focus_input(d: &mut TuiDriver<ChatApp>) {
    let (x, y) = at(d, "Type a message");
    d.click(x + 2.0, y);
}

#[test]
fn every_part_paints_through_its_own_widget() {
    let d = driver();
    for needle in [
        "Reviewing #42",
        "[opus]",
        "You",
        "Hello there",
        "General Kenobi",
        "Ctrl+S sends",
        "Type a message",
        "Send",
    ] {
        let (x, y) = at(&d, needle);
        assert!(AREA.contains(Point::new(x, y)), "{needle} inside the chat");
    }
}

#[test]
fn typing_edits_the_input_and_ctrl_s_submits_it() {
    let mut d = driver();
    focus_input(&mut d);
    for c in "hi you".chars() {
        d.type_char(c);
    }
    assert_eq!(d.app().chat.input_text(), "hi you");
    assert!(d.screen_contains("hi you"), "{}", d.screen());
    d.ctrl_char('s');
    assert!(d.app().events.contains(&ChatEvent::Submit {
        text: "hi you".into()
    }));
}

/// Ctrl+A selects the whole input, the same as in every other text
/// widget, so the next keystroke replaces it.
#[test]
fn ctrl_a_selects_all_so_typing_replaces_the_input() {
    let mut d = driver();
    focus_input(&mut d);
    for c in "one\ntwo".chars() {
        if c == '\n' {
            d.press(Key::Named(quadraui::NamedKey::Enter));
        } else {
            d.type_char(c);
        }
    }
    assert_eq!(d.app().chat.input_text(), "one\ntwo");
    d.ctrl_char('a');
    d.type_char('x');
    assert_eq!(d.app().chat.input_text(), "x");
}

#[test]
fn the_send_button_submits_and_the_stop_button_stops() {
    let mut d = driver();
    // Two presses on the same cell are two clicks, not a double-click.
    d.set_double_click_folding(false);
    focus_input(&mut d);
    d.type_char('q');
    let (x, y) = at(&d, "Send");
    d.click(x + 1.0, y);
    assert!(d
        .app()
        .events
        .contains(&ChatEvent::Submit { text: "q".into() }));

    d.app_mut().chat.busy = true;
    d.render();
    let (x, y) = at(&d, "Stop");
    d.click(x + 1.0, y);
    assert_eq!(d.app().events.last(), Some(&ChatEvent::StopRequested));
}

#[test]
fn clicking_a_turn_reports_it_and_focuses_it() {
    let mut d = driver();
    let (x, y) = at(&d, "General Kenobi");
    d.click(x + 1.0, y);
    assert!(
        d.app().events.contains(&ChatEvent::TurnClicked {
            turn_idx: 1,
            row_in_turn: 1,
        }),
        "{:?}",
        d.app().events
    );
    assert_eq!(d.app().chat.focused_turn, Some(1));
    assert!(!d.app().chat.input_has_focus);
}

#[test]
fn the_wheel_scrolls_the_transcript_off_its_tail() {
    let mut chat = chat();
    chat.transcript = (0..30)
        .map(|i| turn(ChatRole::User, &format!("line {i}")))
        .collect();
    let mut d = driver_with(chat);
    let (x, y) = (AREA.x + 4.0, AREA.y + 3.0);
    d.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, 1.0),
        position: Point::new(x, y),
    });
    let scrolled = d.app().events.iter().find_map(|e| match e {
        ChatEvent::TranscriptScrolled {
            scroll_top,
            stuck_to_bottom,
        } => Some((*scroll_top, *stuck_to_bottom)),
        _ => None,
    });
    let (top, stuck) = scrolled.unwrap_or_else(|| panic!("{:?}", d.app().events));
    assert!(!stuck, "scrolling up leaves the tail");
    assert!(top > 0);
    assert!(!d.app().chat.stuck_to_bottom);
}
