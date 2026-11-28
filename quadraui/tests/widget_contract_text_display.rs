#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `TextDisplay` on the widget contract:
//! the pane reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input routes through the shared
//! widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s routing step (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the
//! `UiEvent::TextDisplay(_, TextDisplayEvent::Scrolled { .. })` this
//! test's own `AppLogic::handle` applies.
//!
//! Never a direct call into `TextDisplay::handle`/`Primitive::layout`
//! that would skip the routing engine.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Decoration, FrameTree, NamedKey, Reaction, Rect, ScreenLayout, StyledSpan,
    Surface, TextDisplay, TextDisplayEvent, TextDisplayLine, UiEvent, WidgetId,
};

const COLS: u16 = 24;
const ROWS: u16 = 6;
/// A five-row pane, so one page of scrolling is five lines.
const AREA: Rect = Rect::new(0.0, 0.0, 20.0, 5.0);

fn display_id() -> WidgetId {
    WidgetId::new("display:log")
}

fn line(text: &str) -> TextDisplayLine {
    TextDisplayLine {
        spans: vec![StyledSpan::plain(text)],
        decoration: Decoration::Normal,
        timestamp: None,
    }
}

/// One read-only log pane routed through the generic
/// `Interactive`/`FrameTree` engine, plus a log of what the app saw.
struct DisplayApp {
    display: TextDisplay,
    offsets: Vec<usize>,
    keys: Vec<String>,
}

impl DisplayApp {
    fn new(count: usize, show_scrollbar: bool) -> Self {
        Self {
            display: TextDisplay {
                id: display_id(),
                lines: (0..count).map(|i| line(&format!("L{i:02}"))).collect(),
                scroll_offset: 0,
                auto_scroll: false,
                max_lines: 0,
                has_focus: true,
                title: None,
                show_scrollbar,
            },
            offsets: Vec::new(),
            keys: Vec::new(),
        }
    }
}

impl AppLogic for DisplayApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.display.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(display_id(), AREA, Box::new(self.display.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::TextDisplay(_, TextDisplayEvent::Scrolled { new_offset }) => {
                self.offsets.push(new_offset);
                // The app owns the offset, as the layout's own
                // `resolved_scroll_offset` contract says.
                self.display.scroll_offset = new_offset;
            }
            UiEvent::TextDisplay(_, TextDisplayEvent::KeyPressed { key, .. }) => {
                self.keys.push(key)
            }
            _ => {}
        }
        Reaction::Redraw
    }
}

/// Click a text row, which focuses the pane without scrolling it.
fn focus_the_pane(driver: &mut TuiDriver<DisplayApp>) {
    driver.click(AREA.x + 1.0, AREA.y + 1.0);
    assert!(
        driver.app().offsets.is_empty(),
        "a click on a text row must not scroll: {:?}",
        driver.app().offsets
    );
}

#[test]
fn surface_widget_text_display_paints_its_lines_top_to_bottom() {
    let driver = TuiDriver::new(DisplayApp::new(3, false), COLS, ROWS);

    for (row, text) in ["L00", "L01", "L02"].iter().enumerate() {
        let bounds = driver
            .find_bounds(text)
            .unwrap_or_else(|| panic!("{text} on screen: {}", driver.screen()));
        assert_eq!(bounds.y, AREA.y + row as f32, "{}", driver.screen());
    }
}

#[test]
fn surface_widget_text_display_paints_its_scrollbar_gutter_and_thumb() {
    let driver = TuiDriver::new(DisplayApp::new(20, true), COLS, ROWS);

    let gutter_x = (AREA.x + AREA.width) as usize - 1;
    let rows: Vec<Vec<char>> = driver
        .screen()
        .lines()
        .map(|l| l.chars().collect())
        .collect();
    assert_eq!(rows[0][gutter_x], '█', "{}", driver.screen());
    assert!(
        rows.iter()
            .take(AREA.height as usize)
            .any(|r| r[gutter_x] == '░'),
        "the track must show below the thumb: {}",
        driver.screen()
    );
}

#[test]
fn page_down_scrolls_a_viewport_and_repaints_the_next_lines() {
    let mut driver = TuiDriver::new(DisplayApp::new(20, true), COLS, ROWS);
    focus_the_pane(&mut driver);

    driver.press_named(NamedKey::PageDown);

    assert_eq!(driver.app().offsets, vec![5]);
    assert!(driver.screen_contains("L05"), "{}", driver.screen());
    assert!(!driver.screen_contains("L00"), "{}", driver.screen());
}

#[test]
fn the_arrow_keys_scroll_one_line_at_a_time() {
    let mut driver = TuiDriver::new(DisplayApp::new(20, true), COLS, ROWS);
    focus_the_pane(&mut driver);

    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Up);

    assert_eq!(driver.app().offsets, vec![1, 2, 1]);
}

#[test]
fn end_scrolls_to_the_last_page_and_home_back_to_the_top() {
    let mut driver = TuiDriver::new(DisplayApp::new(20, true), COLS, ROWS);
    focus_the_pane(&mut driver);

    driver.press_named(NamedKey::End);
    assert!(driver.screen_contains("L19"), "{}", driver.screen());

    driver.press_named(NamedKey::Home);
    assert_eq!(driver.app().offsets, vec![15, 0]);
    assert!(driver.screen_contains("L00"), "{}", driver.screen());
}

#[test]
fn an_unhandled_key_reaches_the_app_as_a_key_passthrough() {
    let mut driver = TuiDriver::new(DisplayApp::new(3, true), COLS, ROWS);
    focus_the_pane(&mut driver);

    driver.type_char('?');

    assert_eq!(driver.app().keys.len(), 1, "{:?}", driver.app().keys);
    assert!(
        driver.app().keys[0].contains('?'),
        "the passthrough names the key: {:?}",
        driver.app().keys
    );
}

#[test]
fn clicking_the_scrollbar_track_below_the_thumb_pages_down() {
    let mut driver = TuiDriver::new(DisplayApp::new(20, true), COLS, ROWS);

    let gutter_x = AREA.x + AREA.width - 1.0;
    driver.click(gutter_x, AREA.y + AREA.height - 1.0);

    assert_eq!(driver.app().offsets, vec![5]);
}

/// A pane with no scrollbar is plain text: it takes no keyboard focus, so
/// the keys that scroll a focused pane do nothing.
#[test]
fn a_pane_without_a_scrollbar_is_not_a_keyboard_stop() {
    let mut driver = TuiDriver::new(DisplayApp::new(20, false), COLS, ROWS);
    focus_the_pane(&mut driver);

    driver.press_named(NamedKey::PageDown);

    assert!(
        driver.app().offsets.is_empty(),
        "{:?}",
        driver.app().offsets
    );
    assert!(driver.screen_contains("L00"), "{}", driver.screen());
}
