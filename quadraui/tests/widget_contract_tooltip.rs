#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Tooltip` on the widget contract: the
//! tooltip reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine. A tooltip is `non_interactive!`, so a
//! click on it must take no focus, emit no widget event, and still reach
//! the app as the plain mouse event.
//!
//! The box is sized with the tooltip's own `Primitive::measure` and placed
//! with `Tooltip::layout` against an anchor, the way a host positions one.

use quadraui::primitive::Size;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, LayoutCtx, Primitive, Reaction, Rect, ScreenLayout, Surface,
    Tooltip, TooltipMeasure, TooltipPlacement, UiEvent, WidgetId,
};

const COLS: u16 = 60;
const ROWS: u16 = 16;

/// One anchored tooltip, drawn and routed as a `Surface::Widget`, plus a
/// log of every event the runtime hands to `handle`.
struct TooltipApp {
    tip: Tooltip,
    rect: Rect,
    events: Vec<UiEvent>,
}

impl TooltipApp {
    fn new(text: &str) -> Self {
        let mut tip = Tooltip::new(WidgetId::new("hover-tip"), text);
        tip.placement = TooltipPlacement::Bottom;
        let ctx = LayoutCtx {
            line_height: 1.0,
            char_width: 1.0,
            nerd_fonts_enabled: false,
        };
        let natural = tip.measure(&ctx, Size::new(COLS as f32, ROWS as f32));
        let measure = TooltipMeasure::new(natural.width.unwrap(), natural.height.unwrap());
        let anchor = Rect::new(10.0, 2.0, 6.0, 1.0);
        let viewport = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);
        let rect = tip.layout(anchor, viewport, measure, 0.0).bounds;
        Self {
            tip,
            rect,
            events: Vec::new(),
        }
    }
}

impl AppLogic for TooltipApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.rect,
            widget: Box::new(self.tip.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.tip.id, self.rect, Box::new(self.tip.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Continue
    }
}

fn row(driver: &TuiDriver<TooltipApp>, y: usize) -> Vec<char> {
    driver
        .screen()
        .lines()
        .nth(y)
        .map(|line| line.chars().collect())
        .unwrap_or_default()
}

#[test]
fn surface_widget_tooltip_paints_a_closed_box_around_its_text() {
    let driver = TuiDriver::new(TooltipApp::new("Hover hint"), COLS, ROWS);
    let text = driver
        .find_bounds("Hover hint")
        .expect("tooltip text on screen");
    let (x, y) = (text.x as usize, text.y as usize);

    // Text sits one pad column inside the left border, one row below the
    // top border; the measured box closes one column past the text.
    let text_row = row(&driver, y);
    assert_eq!(text_row[x - 2], '│', "{}", driver.screen());
    assert_eq!(text_row[x + "Hover hint".len()], '│', "{}", driver.screen());
    assert_eq!(row(&driver, y - 1)[x - 2], '┌', "{}", driver.screen());
    assert_eq!(row(&driver, y + 1)[x - 2], '└', "{}", driver.screen());
    assert_eq!(
        row(&driver, y + 1)[x + "Hover hint".len()],
        '┘',
        "{}",
        driver.screen()
    );
}

#[test]
fn surface_widget_tooltip_shows_every_line_of_a_multi_line_tip() {
    let driver = TuiDriver::new(TooltipApp::new("first line\nsecond line"), COLS, ROWS);
    let first = driver
        .find_bounds("first line")
        .expect("first line on screen");
    let second = driver
        .find_bounds("second line")
        .expect("second line on screen");
    assert_eq!(first.x, second.x, "lines share a left edge");
    assert_eq!(second.y, first.y + 1.0, "lines sit on consecutive rows");
}

#[test]
fn surface_widget_tooltip_sits_below_its_anchor() {
    let driver = TuiDriver::new(TooltipApp::new("Hover hint"), COLS, ROWS);
    let text = driver
        .find_bounds("Hover hint")
        .expect("tooltip text on screen");
    // Anchor row is 2; a bottom placement puts the top border on row 3 and
    // the text on row 4.
    assert_eq!(text.y, 4.0);
}

#[test]
fn click_on_a_tooltip_is_routed_but_not_consumed() {
    let mut driver = TuiDriver::new(TooltipApp::new("Hover hint"), COLS, ROWS);
    let (x, y) = driver.find("Hover hint").expect("tooltip text on screen");
    driver.click(x, y);

    let events = &driver.app().events;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, UiEvent::MouseDown { position, .. } if position.x == x && position.y == y)),
        "the plain mouse-down must reach the app: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, UiEvent::FocusChanged(Some(_)))),
        "a non-interactive tooltip must not take focus: {events:?}"
    );
}
