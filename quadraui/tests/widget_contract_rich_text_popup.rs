#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `RichTextPopup` on the widget
//! contract: the popup reaches the frame as a type-erased
//! `Surface::Widget`, is painted through `DynWidget::paint` →
//! `Primitive::paint_cells`, and is registered in the app's `FrameTree`
//! so input is routed through the shared widget-contract engine —
//! `UiEvent` into `quadraui::runtime::preprocess_event`'s widget routing
//! (hit test, click-to-focus, `Interactive::handle`) and back out as the
//! `UiEvent` this test's own `AppLogic::handle` applies onto the owned
//! `RichTextPopup`, which the next `render` paints.
//!
//! Never a direct call into `RichTextPopup::handle`/`Primitive::layout`
//! that would skip the routing engine. The wheel arm is the one the
//! engine cannot deliver: `route_widget_event`'s `UiEvent` match has no
//! `Scroll` arm, so no `RoutedEvent::Wheel` reaches a widget through it —
//! `primitives::rich_text_popup`'s own unit tests drive that arm against
//! a `RoutedEvent` directly, and Page keys exercise the same
//! scroll path end to end here.

use quadraui::tui::ratatui_color;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, NamedKey, Reaction, Rect, RichTextLink, RichTextPopup,
    ScreenLayout, StyledText, Surface, Theme, UiEvent, WidgetId,
};

const COLS: u16 = 44;
const ROWS: u16 = 16;
/// Six rows tall: one border row either side leaves four body rows, so a
/// twelve-line body overflows and pages.
const POPUP: Rect = Rect::new(2.0, 2.0, 30.0, 6.0);

/// One rich-text popup, drawn and routed as a `Surface::Widget`, plus a
/// record of what the host was told to do with it.
struct PopupApp {
    popup: RichTextPopup,
    opened: Vec<String>,
}

impl PopupApp {
    fn new(lines: usize, links: Vec<RichTextLink>) -> Self {
        let line_text: Vec<String> = (0..lines).map(|i| format!("row-{i:02}")).collect();
        let popup = RichTextPopup {
            id: WidgetId::new("popup:hover"),
            lines: line_text
                .iter()
                .map(|l| StyledText::plain(l.clone()))
                .collect(),
            line_text,
            line_scales: Vec::new(),
            scroll_top: 0,
            max_visible_rows: 4,
            has_focus: true,
            selection: None,
            links,
            focused_link: None,
            placement: Default::default(),
            padding: 0.0,
            fg: None,
            bg: None,
        };
        Self {
            popup,
            opened: Vec::new(),
        }
    }
}

impl AppLogic for PopupApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: POPUP,
            widget: Box::new(self.popup.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.popup.id, POPUP, Box::new(self.popup.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::RichTextLinkClicked(id, url) if id == self.popup.id => {
                self.opened.push(url);
                Reaction::Redraw
            }
            UiEvent::ScrollOffsetChanged { widget, new_offset } if widget == self.popup.id => {
                self.popup.scroll_top = new_offset;
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

fn driver(lines: usize) -> TuiDriver<PopupApp> {
    TuiDriver::new(PopupApp::new(lines, Vec::new()), COLS, ROWS)
}

fn linked_driver() -> TuiDriver<PopupApp> {
    let links = vec![RichTextLink {
        line: 1,
        start_byte: 0,
        end_byte: 6,
        url: "https://example.com/docs".into(),
    }];
    TuiDriver::new(PopupApp::new(12, links), COLS, ROWS)
}

fn row(driver: &TuiDriver<PopupApp>, y: usize) -> Vec<char> {
    driver
        .screen()
        .lines()
        .nth(y)
        .map(|l| l.chars().collect())
        .unwrap_or_default()
}

#[test]
fn a_surface_widget_popup_paints_a_closed_box_around_its_body() {
    let driver = driver(3);
    let (x0, y0) = (POPUP.x as usize, POPUP.y as usize);
    let (x1, y1) = (
        (POPUP.x + POPUP.width) as usize - 1,
        (POPUP.y + POPUP.height) as usize - 1,
    );
    assert_eq!(row(&driver, y0)[x0], '┌', "{}", driver.screen());
    assert_eq!(row(&driver, y0)[x1], '┐', "{}", driver.screen());
    assert_eq!(row(&driver, y1)[x0], '└', "{}", driver.screen());
    assert_eq!(row(&driver, y1)[x1], '┘', "{}", driver.screen());
    assert_eq!(row(&driver, y0 + 1)[x0], '│', "{}", driver.screen());
}

#[test]
fn the_body_rows_paint_inside_the_border() {
    let driver = driver(3);
    let first = driver.find_bounds("row-00").expect("line 0 visible");
    assert_eq!(first.x, POPUP.x + 1.0, "{}", driver.screen());
    assert_eq!(first.y, POPUP.y + 1.0, "{}", driver.screen());
    let second = driver.find_bounds("row-01").expect("line 1 visible");
    assert_eq!(second.y, first.y + 1.0, "{}", driver.screen());
}

/// Four body rows fit inside the box, so a twelve-line body shows a
/// window and a thumb scrollbar in the right border column.
#[test]
fn an_overflowing_body_paints_a_thumb_scrollbar() {
    let driver = driver(12);
    assert!(driver.screen_contains("row-03"), "{}", driver.screen());
    assert!(!driver.screen_contains("row-04"), "{}", driver.screen());

    let track: Vec<char> = (1..5)
        .map(|r| row(&driver, POPUP.y as usize + r)[(POPUP.x + POPUP.width) as usize - 1])
        .collect();
    assert!(
        track.contains(&'█'),
        "thumb: {track:?}\n{}",
        driver.screen()
    );
    assert!(
        track.contains(&'░'),
        "track: {track:?}\n{}",
        driver.screen()
    );
}

#[test]
fn a_focused_popup_tints_its_border_with_the_link_colour() {
    let mut driver = driver(3);
    let style = driver
        .style_at(POPUP.x as u16, POPUP.y as u16)
        .expect("border cell");
    assert_eq!(style.fg, ratatui_color(Theme::default().link_fg));

    driver.app_mut().popup.has_focus = false;
    driver.render();
    let style = driver
        .style_at(POPUP.x as u16, POPUP.y as u16)
        .expect("border cell");
    assert_eq!(style.fg, ratatui_color(Theme::default().hover_border));
}

#[test]
fn page_keys_scroll_the_body_through_the_real_routing_engine() {
    let mut driver = linked_driver();
    // Click the body once so the widget-contract click-to-focus step
    // runs — nothing routes a key to the popup until then.
    let (x, y) = driver.find("row-00").expect("line 0 visible");
    driver.click(x, y);

    driver.press_named(NamedKey::PageDown);
    assert_eq!(driver.app().popup.scroll_top, 4);
    assert!(driver.screen_contains("row-04"), "{}", driver.screen());
    assert!(!driver.screen_contains("row-00"), "{}", driver.screen());

    driver.press_named(NamedKey::PageUp);
    assert_eq!(driver.app().popup.scroll_top, 0);
    assert!(driver.screen_contains("row-00"), "{}", driver.screen());
}

/// Scrolling stops at the last full screen rather than running past the
/// end into blank rows.
#[test]
fn paging_past_the_end_pins_to_the_last_full_screen() {
    let mut driver = linked_driver();
    let (x, y) = driver.find("row-00").expect("line 0 visible");
    driver.click(x, y);
    for _ in 0..6 {
        driver.press_named(NamedKey::PageDown);
    }
    assert_eq!(driver.app().popup.scroll_top, 8);
    assert!(driver.screen_contains("row-11"), "{}", driver.screen());
}

#[test]
fn a_click_on_a_link_reports_its_url_to_the_host() {
    let mut driver = linked_driver();
    let (x, y) = driver.find("row-01").expect("the linked line is visible");
    driver.click(x, y);
    assert_eq!(driver.app().opened, vec!["https://example.com/docs"]);
}

#[test]
fn a_click_on_plain_body_text_opens_nothing() {
    let mut driver = linked_driver();
    let (x, y) = driver.find("row-02").expect("line 2 visible");
    driver.click(x, y);
    assert!(driver.app().opened.is_empty());
}

/// Escape is not the popup's to claim — dismissing it is the host's
/// policy, so the keystroke keeps travelling and the popup stays up.
#[test]
fn escape_is_left_to_the_host() {
    let mut driver = linked_driver();
    let (x, y) = driver.find("row-00").expect("line 0 visible");
    driver.click(x, y);
    driver.press_named(NamedKey::Escape);
    assert!(driver.screen_contains("row-00"), "{}", driver.screen());
    assert!(driver.app().opened.is_empty());
}
