#![cfg(feature = "tui")]
//! Headless `TuiDriver` conformance test for the generic `ScrollView`
//! primitive: wheel scroll, keyboard scroll (arrow/page/home/end), and
//! clipping of arbitrary child content, all driven through the real
//! `event -> handle -> render` path on the in-memory TUI backend.
//!
//! `ScrollView` has no `Backend` painter of its own (none is in scope for
//! this issue — it is shared logic only), so this test's `render` paints
//! only the rows `ScrollView::is_visible` reports as inside the current
//! visible range, via `TextDisplay`. Each row is arbitrary child content
//! the view knows nothing about: plain text here, but it could just as
//! well be a form section or a help panel, since `ScrollView` clips by
//! content-unit arithmetic, not by the content's own type.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Decoration, NamedKey, Point, Reaction, Rect, ScrollDelta, ScrollView,
    ScrollViewEvent, StyledSpan, TextDisplay, TextDisplayLine, UiEvent, WidgetId,
};

const VIEWPORT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 20.0,
    height: 5.0,
};
const ROW_COUNT: usize = 20;

/// Minimal `AppLogic` wrapping a single focused `ScrollView` over 20 rows
/// of plain-text content, used only by this conformance test (not a demo
/// — this issue ships no backend painter).
struct ScrollViewApp {
    view: ScrollView,
    rows: Vec<String>,
}

impl ScrollViewApp {
    fn new() -> Self {
        let mut view = ScrollView::new(WidgetId::new("test:panel"), VIEWPORT, ROW_COUNT as f32);
        view.focused = true;
        let rows = (0..ROW_COUNT).map(|i| format!("Row {i}")).collect();
        Self { view, rows }
    }

    fn apply(&mut self, event: Option<ScrollViewEvent>) -> Reaction {
        match event {
            Some(ScrollViewEvent::Scrolled { offset }) => {
                self.view.scroll_offset = offset;
                Reaction::Redraw
            }
            Some(_) => Reaction::Redraw,
            None => Reaction::Continue,
        }
    }
}

impl AppLogic for ScrollViewApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut display = TextDisplay::new(WidgetId::new("test:display"));
        for (index, row) in self.rows.iter().enumerate() {
            if self.view.is_visible(index as f32, 1.0) {
                display.append_line(TextDisplayLine {
                    spans: vec![StyledSpan::plain(row.clone())],
                    decoration: Decoration::default(),
                    timestamp: None,
                });
            }
        }
        backend.draw_text_display(VIEWPORT, &display);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Scroll { delta, .. } => {
                let reaction_event = self.view.handle_wheel(delta, 1.0);
                self.apply(reaction_event)
            }
            UiEvent::KeyPressed { key, modifiers, .. } => {
                let reaction_event = self.view.handle_key(&key, &modifiers, 1.0);
                self.apply(reaction_event)
            }
            _ => Reaction::Continue,
        }
    }
}

#[test]
fn starts_scrolled_to_top_showing_only_the_first_viewport_rows() {
    let driver = TuiDriver::new(ScrollViewApp::new(), 20, 5);
    assert!(driver.screen_contains("Row 0"));
    assert!(driver.screen_contains("Row 4"));
    // Row 5 is outside the 5-row viewport at offset 0 — clipped, not painted.
    assert!(!driver.screen_contains("Row 5"));
    assert!(!driver.screen_contains("Row 19"));
}

#[test]
fn wheel_scroll_moves_the_visible_window_and_clips_scrolled_past_rows() {
    let mut driver = TuiDriver::new(ScrollViewApp::new(), 20, 5);
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, 3.0),
        position: Point::new(1.0, 1.0),
    });
    assert_eq!(driver.app().view.scroll_offset, 3.0);
    assert!(driver.screen_contains("Row 3"));
    assert!(driver.screen_contains("Row 7"));
    // Scrolled past the top: Row 0 is clipped out of the viewport now.
    assert!(!driver.screen_contains("Row 0"));
    assert!(!driver.screen_contains("Row 8"));
}

#[test]
fn down_and_up_keys_scroll_by_one_row_when_focused() {
    let mut driver = TuiDriver::new(ScrollViewApp::new(), 20, 5);
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().view.scroll_offset, 1.0);
    assert!(driver.screen_contains("Row 5"));
    assert!(!driver.screen_contains("Row 0"));

    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().view.scroll_offset, 0.0);
    assert!(driver.screen_contains("Row 0"));
}

#[test]
fn page_down_and_page_up_scroll_by_a_full_viewport() {
    let mut driver = TuiDriver::new(ScrollViewApp::new(), 20, 5);
    driver.press_named(NamedKey::PageDown);
    assert_eq!(driver.app().view.scroll_offset, 5.0);
    assert!(driver.screen_contains("Row 5"));
    assert!(driver.screen_contains("Row 9"));
    assert!(!driver.screen_contains("Row 4"));

    driver.press_named(NamedKey::PageUp);
    assert_eq!(driver.app().view.scroll_offset, 0.0);
}

#[test]
fn home_and_end_jump_to_the_bounds_and_clamp_to_max_offset() {
    let mut driver = TuiDriver::new(ScrollViewApp::new(), 20, 5);
    driver.press_named(NamedKey::End);
    // max_offset == content_len (20) - visible_len (5) == 15.
    assert_eq!(driver.app().view.scroll_offset, 15.0);
    assert!(driver.screen_contains("Row 15"));
    assert!(driver.screen_contains("Row 19"));
    assert!(!driver.screen_contains("Row 14"));

    driver.press_named(NamedKey::Home);
    assert_eq!(driver.app().view.scroll_offset, 0.0);
    assert!(driver.screen_contains("Row 0"));
}

#[test]
fn unfocused_view_ignores_keyboard_scroll() {
    let mut driver = TuiDriver::new(ScrollViewApp::new(), 20, 5);
    driver.app_mut().view.focused = false;
    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().view.scroll_offset, 0.0);
    assert!(driver.screen_contains("Row 0"));
}
