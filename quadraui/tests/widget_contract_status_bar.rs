#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `StatusBar` on the widget contract: the
//! bar reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input routes through the shared
//! widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s routing step (hit test,
//! pointer capture, `Interactive::handle`) and back out as the
//! `UiEvent::StatusBar(_, StatusBarEvent::SegmentClicked { .. })` this
//! test's own `AppLogic::handle` records.
//!
//! Never a direct call into `StatusBar::handle`/`Primitive::layout` that
//! would skip the routing engine.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Color, FrameTree, Reaction, Rect, ScreenLayout, StatusBar, StatusBarEvent,
    StatusBarSegment, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 6;
/// The bar occupies the screen's bottom row — a non-zero origin, so a
/// click's bar-local translation is actually exercised.
const AREA: Rect = Rect::new(0.0, (ROWS - 1) as f32, COLS as f32, 1.0);

const MODE: &str = "NORMAL";
const PLAIN: &str = " main.rs ";
const CURSOR: &str = " 1:1 ";

fn mode_id() -> WidgetId {
    WidgetId::new("status:mode")
}

fn cursor_id() -> WidgetId {
    WidgetId::new("status:cursor")
}

fn segment(text: &str, action: Option<WidgetId>) -> StatusBarSegment {
    StatusBarSegment {
        text: text.to_string(),
        fg: Color::rgb(230, 230, 230),
        bg: Color::rgb(40, 80, 120),
        bold: false,
        action_id: action,
    }
}

/// One status bar routed through the generic `Interactive`/`FrameTree`
/// engine, plus a log of what the app saw.
struct StatusBarApp {
    bar: StatusBar,
    clicked: Vec<WidgetId>,
    events: Vec<UiEvent>,
}

impl StatusBarApp {
    fn new() -> Self {
        Self {
            bar: StatusBar {
                id: WidgetId::new("status:editor"),
                left_segments: vec![segment(MODE, Some(mode_id())), segment(PLAIN, None)],
                right_segments: vec![segment(CURSOR, Some(cursor_id()))],
            },
            clicked: Vec::new(),
            events: Vec::new(),
        }
    }
}

impl AppLogic for StatusBarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.bar.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.bar.id, AREA, Box::new(self.bar.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::StatusBar(_, StatusBarEvent::SegmentClicked { id }) = &event {
            self.clicked.push(*id);
        }
        self.events.push(event);
        Reaction::Redraw
    }
}

/// Press and release the left button on the first cell of `label`'s
/// painted run — the status bar only activates a segment when the press
/// and the release land on the same one.
fn click_segment(driver: &mut TuiDriver<StatusBarApp>, label: &str) {
    let bounds = driver
        .find_bounds(label)
        .unwrap_or_else(|| panic!("{label} on screen"));
    let (x, y) = (bounds.x + 0.5, bounds.y);
    driver.mouse_down(x, y);
    driver.mouse_up(x, y);
}

#[test]
fn surface_widget_status_bar_paints_both_segment_groups() {
    let driver = TuiDriver::new(StatusBarApp::new(), COLS, ROWS);

    assert!(driver.screen_contains(MODE), "{}", driver.screen());
    assert!(driver.screen_contains("main.rs"), "{}", driver.screen());
    assert!(driver.screen_contains("1:1"), "{}", driver.screen());

    // The left group starts flush at the bar's own left edge and the right
    // group is right-aligned against its right edge — both on the bar's
    // own row, not anywhere else on screen.
    let mode = driver.find_bounds(MODE).expect("mode segment on screen");
    assert_eq!(mode.x, 0.0, "{}", driver.screen());
    assert_eq!(mode.y, AREA.y, "{}", driver.screen());

    let cursor = driver.find_bounds("1:1").expect("cursor segment on screen");
    assert_eq!(cursor.y, AREA.y, "{}", driver.screen());
    assert!(
        cursor.x + cursor.width >= COLS as f32 - 1.0,
        "the right group should sit against the bar's right edge: {}",
        driver.screen()
    );
}

#[test]
fn clicking_a_clickable_segment_reports_that_segment() {
    let mut driver = TuiDriver::new(StatusBarApp::new(), COLS, ROWS);

    click_segment(&mut driver, MODE);

    assert_eq!(driver.app().clicked, vec![mode_id()]);
}

/// Each group resolves its own segment — the right group's bar-local
/// offset has to survive the round trip for this to land on `cursor`
/// rather than on `mode`.
#[test]
fn clicking_the_right_group_reports_the_right_groups_segment() {
    let mut driver = TuiDriver::new(StatusBarApp::new(), COLS, ROWS);

    click_segment(&mut driver, "1:1");

    assert_eq!(driver.app().clicked, vec![cursor_id()]);
}

#[test]
fn clicking_a_segment_with_no_action_id_reports_nothing() {
    let mut driver = TuiDriver::new(StatusBarApp::new(), COLS, ROWS);

    click_segment(&mut driver, "main.rs");

    assert!(
        driver.app().clicked.is_empty(),
        "a non-clickable segment must not report a click: {:?}",
        driver.app().clicked
    );
}

/// A press that drifts off its segment before the release activates
/// nothing — the release still reaches the bar (it captured the pointer),
/// it just resolves to a different segment.
#[test]
fn a_press_released_on_another_segment_reports_nothing() {
    let mut driver = TuiDriver::new(StatusBarApp::new(), COLS, ROWS);
    let mode = driver.find_bounds(MODE).expect("mode segment on screen");
    let cursor = driver.find_bounds("1:1").expect("cursor segment on screen");

    driver.mouse_down(mode.x + 0.5, mode.y);
    driver.mouse_up(cursor.x + 0.5, cursor.y);

    assert!(
        driver.app().clicked.is_empty(),
        "a press that drifts off its segment must activate nothing: {:?}",
        driver.app().clicked
    );
}

/// A press anywhere above the bar's own row never reaches it — the plain
/// mouse event still bubbles to the app.
#[test]
fn a_press_outside_the_bar_bubbles_to_the_app_unclaimed() {
    let mut driver = TuiDriver::new(StatusBarApp::new(), COLS, ROWS);

    driver.mouse_down(2.0, 0.0);
    driver.mouse_up(2.0, 0.0);

    assert!(driver.app().clicked.is_empty());
    assert!(
        driver.app().events.iter().any(|e| matches!(
            e,
            UiEvent::MouseDown { position, .. } if position.y == 0.0
        )),
        "the plain mouse-down must bubble to the app: {:?}",
        driver.app().events
    );
}
