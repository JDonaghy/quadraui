#![cfg(feature = "tui")]
//! Black-box `TuiDriver` tests for the focus ring on TUI: a focused button
//! keeps its `[ Label ]` glyphs, the ring fits the button rather than the
//! layout cell it sits in, plain text is not a Tab stop, and a Rust app
//! that routes its widgets through `AppLogic::frame_tree` alone shows the
//! same ring the bindings do.
//!
//! The app mirrors the hero counter's shape: a text readout above two
//! buttons, each button laid out in a cell three rows tall and much wider
//! than its label, the equal-share cells the Python and Node `Row` hand it.

use quadraui::tui::testing::{Modifier, TuiDriver};
use quadraui::{
    AppLogic, Backend, Button, FrameTree, NamedKey, Reaction, Rect, ScreenLayout, StyledSpan,
    Surface, TextDisplay, TextDisplayLine, UiEvent, WidgetId,
};

const COLS: u16 = 30;
const ROWS: u16 = 6;
const TEXT: Rect = Rect::new(0.0, 0.0, 30.0, 2.0);
const DEC: Rect = Rect::new(0.0, 2.0, 15.0, 3.0);
const INC: Rect = Rect::new(15.0, 2.0, 15.0, 3.0);

struct Counter {
    count: i32,
    /// Whether the app hands its stops to Tab cycling, or routes its
    /// widgets through `frame_tree` alone.
    tab_stops: bool,
}

impl Counter {
    fn new(tab_stops: bool) -> Self {
        Self {
            count: 0,
            tab_stops,
        }
    }

    fn view(&self) -> ScreenLayout {
        let mut label = TextDisplay::new(WidgetId::new("count"));
        label.lines = vec![TextDisplayLine {
            spans: vec![StyledSpan::plain(format!("Count: {}", self.count))],
            decoration: Default::default(),
            timestamp: None,
        }];
        let mut ui = ScreenLayout::new();
        ui.push(Surface::Widget {
            rect: TEXT,
            widget: Box::new(label),
        });
        ui.push(Surface::Widget {
            rect: DEC,
            widget: Box::new(Button::new(WidgetId::new("dec"), "-")),
        });
        ui.push(Surface::Widget {
            rect: INC,
            widget: Box::new(Button::new(WidgetId::new("inc"), "+")),
        });
        ui
    }
}

impl AppLogic for Counter {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        self.view().draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        self.view().into_frame_tree()
    }

    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        if self.tab_stops {
            self.view().tab_stops()
        } else {
            Vec::new()
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let _ = backend;
        match event {
            UiEvent::ButtonActivated { widget } if widget.as_str() == "inc" => self.count += 1,
            UiEvent::ButtonActivated { widget } if widget.as_str() == "dec" => self.count -= 1,
            _ => return Reaction::Continue,
        }
        Reaction::Redraw
    }
}

fn row(driver: &TuiDriver<Counter>, y: u16) -> String {
    driver
        .screen()
        .lines()
        .nth(y as usize)
        .unwrap_or("")
        .to_string()
}

fn underlined(driver: &TuiDriver<Counter>, x: u16, y: u16) -> bool {
    driver
        .style_at(x, y)
        .is_some_and(|s| s.modifiers.contains(Modifier::UNDERLINED))
}

/// Clicking a button focuses it; the ring underlines `[ - ]` and leaves
/// both brackets and the cell's empty rows alone.
#[test]
fn a_focused_button_keeps_both_brackets() {
    let mut driver = TuiDriver::new(Counter::new(true), COLS, ROWS);
    let (x, y) = driver.find("[ - ]").expect("the button paints");
    driver.click(x + 2.0, y);
    driver.render();

    let label_row = y as u16;
    assert!(
        row(&driver, label_row).starts_with("[ - ]"),
        "{}",
        driver.screen()
    );
    for y in 2..5 {
        let r = row(&driver, y);
        assert!(
            !r.contains(['╭', '╮', '╰', '╯', '│']),
            "row {y} carries ring glyphs over the button's cell:\n{}",
            driver.screen()
        );
    }
    assert!(
        (0..5).all(|x| underlined(&driver, x, label_row)),
        "the ring underlines the button's label"
    );
}

/// The ring fits the button's natural size, not the 15-column cell it was
/// laid out in.
#[test]
fn the_ring_fits_the_button_not_its_layout_cell() {
    let mut driver = TuiDriver::new(Counter::new(true), COLS, ROWS);
    let (x, y) = driver.find("[ - ]").expect("the button paints");
    driver.click(x + 2.0, y);
    driver.render();

    let label_row = y as u16;
    assert!(underlined(&driver, 4, label_row));
    assert!(
        (5..15).all(|x| !underlined(&driver, x, label_row)),
        "the ring stops at the button's closing bracket"
    );
}

/// The text readout is not in the Tab order: Tab from nothing lands on
/// the first button.
#[test]
fn plain_text_is_not_a_tab_stop() {
    let mut driver = TuiDriver::new(Counter::new(true), COLS, ROWS);
    let stops = driver.app().view().tab_stops();
    assert!(
        stops.iter().all(|(id, _)| id.as_str() != "count"),
        "{stops:?}"
    );

    driver.press_named(NamedKey::Tab);
    driver.render();
    assert_eq!(
        driver.backend().focus_tree().focused_id(),
        Some(&WidgetId::new("dec"))
    );
    assert!(
        row(&driver, 0).starts_with("Count: 0"),
        "{}",
        driver.screen()
    );
}

/// A focused button's brackets switch to the accent colour, the half of
/// its focus a glyph/colour comparison (the cross-language parity check)
/// sees.
#[test]
fn a_focused_buttons_brackets_take_the_accent_colour() {
    let mut driver = TuiDriver::new(Counter::new(true), COLS, ROWS);
    let (x, y) = driver.find("[ + ]").expect("the button paints");
    let (bx, by) = (x as u16, y as u16);
    let before = driver.style_at(bx, by).expect("cell").fg;
    driver.click(x + 2.0, y);
    driver.render();
    let after = driver.style_at(bx, by).expect("cell").fg;
    assert_ne!(before, after, "focus recolours the bracket");
    assert_eq!(after, driver.style_at(bx + 4, by).expect("cell").fg);
}

/// An app that routes its widgets through `frame_tree` and declares no
/// `extra_tab_stops` still shows the ring on the widget a click focused,
/// as the Python and Node hosts do.
#[test]
fn a_frame_tree_app_shows_the_ring_on_the_clicked_button() {
    let mut driver = TuiDriver::new(Counter::new(false), COLS, ROWS);
    let (x, y) = driver.find("[ + ]").expect("the button paints");
    driver.click(x + 2.0, y);
    driver.render();

    assert_eq!(driver.app().count, 1);
    assert!(underlined(&driver, x as u16, y as u16));
}
