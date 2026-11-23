#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Float` on the widget contract: the
//! float reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input is routed through the
//! shared widget-contract engine. A float is `non_interactive!`, so a
//! click on it must take no focus, emit no widget event, and still reach
//! the app as the plain mouse event.
//!
//! The float is placed with `Float::layout` against its anchor, the way a
//! host positions one, and its content — here a borderless tooltip, also
//! a `Surface::Widget` — is painted by the host into the float's
//! `content_bounds`, which `Primitive::layout` derives from the same rect.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    Anchor, AppLogic, Backend, Float, FloatMeasure, FrameTree, LayoutCtx, Primitive, Reaction,
    Rect, ScreenLayout, Side, Surface, Tooltip, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 12;

/// One anchored float with a line of content inside it, drawn and routed
/// as `Surface::Widget`s, plus a log of every event the runtime hands to
/// `handle`.
struct FloatApp {
    float: Float,
    rect: Rect,
    /// Painted *before* the float when the float is transparent (so it
    /// must stay visible through it) and after it otherwise (as the
    /// float's own content).
    content: Tooltip,
    events: Vec<UiEvent>,
}

impl FloatApp {
    fn new(transparent: bool) -> Self {
        let anchor = Anchor::new(Rect::new(5.0, 1.0, 4.0, 1.0), Side::Bottom);
        let mut float = Float::new(WidgetId::new("keyhints"), anchor);
        float.transparent = transparent;
        let viewport = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);
        let rect = float.layout(viewport, FloatMeasure::new(16.0, 4.0)).bounds;
        Self {
            float,
            rect,
            content: Tooltip::new(WidgetId::new("keyhints:body"), "g  goto"),
            events: Vec::new(),
        }
    }

    fn content_rect(&self) -> Rect {
        let ctx = LayoutCtx {
            line_height: 1.0,
            char_width: 1.0,
            nerd_fonts_enabled: false,
        };
        let layout = Primitive::layout(&self.float, &ctx, self.rect);
        // One content row, two columns in from the float's own border.
        Rect::new(
            layout.content_bounds.x + 1.0,
            layout.content_bounds.y + 1.0,
            layout.content_bounds.width - 2.0,
            1.0,
        )
    }
}

impl AppLogic for FloatApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let content = Surface::Widget {
            rect: self.content_rect(),
            widget: Box::new(self.content.clone()),
        };
        let float = Surface::Widget {
            rect: self.rect,
            widget: Box::new(self.float.clone()),
        };
        let mut frame = ScreenLayout::new();
        if self.float.transparent {
            frame.push(content);
            frame.push(float);
        } else {
            frame.push(float);
            frame.push(content);
        }
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.float.id, self.rect, Box::new(self.float.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Continue
    }
}

fn cell(driver: &TuiDriver<FloatApp>, x: f32, y: f32) -> char {
    driver
        .screen()
        .lines()
        .nth(y as usize)
        .and_then(|line| line.chars().nth(x as usize))
        .unwrap_or(' ')
}

#[test]
fn surface_widget_float_paints_a_bordered_box_at_its_anchor() {
    let driver = TuiDriver::new(FloatApp::new(false), COLS, ROWS);
    let r = driver.app().rect;
    let (right, bottom) = (r.x + r.width - 1.0, r.y + r.height - 1.0);
    assert_eq!(cell(&driver, r.x, r.y), '┌', "{}", driver.screen());
    assert_eq!(cell(&driver, right, r.y), '┐', "{}", driver.screen());
    assert_eq!(cell(&driver, r.x, bottom), '└', "{}", driver.screen());
    assert_eq!(cell(&driver, right, bottom), '┘', "{}", driver.screen());
    assert_eq!(cell(&driver, r.x, r.y + 1.0), '│', "{}", driver.screen());
    // Bottom placement: the float's top border sits just below the anchor
    // row.
    assert_eq!(r.y, 2.0);
}

#[test]
fn host_content_lands_inside_the_float_border() {
    let driver = TuiDriver::new(FloatApp::new(false), COLS, ROWS);
    let r = driver.app().rect;
    let text = driver
        .find_bounds("g  goto")
        .expect("float content on screen");
    assert!(
        text.x > r.x && text.x + text.width < r.x + r.width - 1.0,
        "content must sit inside the side borders: {text:?} in {r:?}\n{}",
        driver.screen()
    );
    assert!(
        text.y > r.y && text.y < r.y + r.height - 1.0,
        "content must sit between the top and bottom borders: {text:?} in {r:?}\n{}",
        driver.screen()
    );
}

#[test]
fn transparent_float_keeps_what_was_painted_under_it_visible() {
    let driver = TuiDriver::new(FloatApp::new(true), COLS, ROWS);
    let r = driver.app().rect;
    assert!(
        driver.screen_contains("g  goto"),
        "a transparent float must not paint over the layer below it:\n{}",
        driver.screen()
    );
    assert_eq!(cell(&driver, r.x, r.y), '┌', "{}", driver.screen());
}

#[test]
fn click_on_a_float_is_routed_but_not_consumed() {
    let mut driver = TuiDriver::new(FloatApp::new(false), COLS, ROWS);
    let r = driver.app().rect;
    let (x, y) = (r.x + 0.5, r.y + 0.5);
    driver.click(x, y);

    let events = &driver.app().events;
    assert!(
        events.iter().any(
            |e| matches!(e, UiEvent::MouseDown { position, .. } if position.x == x && position.y == y)
        ),
        "the plain mouse-down must reach the app: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, UiEvent::FocusChanged(Some(_)))),
        "a non-interactive float must not take focus: {events:?}"
    );
}
