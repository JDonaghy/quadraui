#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Terminal` on the widget contract: the
//! pane reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so every gesture routes through the
//! shared widget-contract engine — hit test, click-to-focus,
//! `Interactive::handle` — and comes back out as the
//! `UiEvent::Terminal(_, TerminalEvent::..)` this app records.
//!
//! This is the runtime leg of the three-way event-parity test. The
//! direct-`Interactive::handle` leg (a binding host's dispatch shape) and
//! the `a11y::route_widget_action` leg live in
//! `src/primitives/terminal.rs`'s `contract_tests` module, and all three
//! resolve one upward scroll to the same `UiEvent`.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Color, FrameTree, Key, Point, Reaction, Rect, ScreenLayout, ScrollDelta,
    Surface, Terminal, TerminalCell, TerminalCursorShape, TerminalDivider, TerminalEvent,
    TerminalScrollbar, UiEvent, WidgetId,
};

const COLS: u16 = 24;
const ROWS: u16 = 8;
/// A twelve-column, five-row pane whose last column is the scrollbar
/// gutter, leaving an eleven-column cell grid.
const AREA: Rect = Rect::new(2.0, 1.0, 12.0, 5.0);
/// The split divider column just right of the pane.
const DIVIDER: Rect = Rect::new(15.0, 1.0, 1.0, 5.0);

fn terminal_id() -> WidgetId {
    WidgetId::new("terminal:shell")
}

fn cell(ch: char) -> TerminalCell {
    TerminalCell {
        text: ch.to_string(),
        fg: Color::rgb(220, 220, 220),
        bg: Color::rgb(20, 20, 20),
        bold: false,
        italic: false,
        underline: false,
        dim: false,
        selected: false,
        is_cursor: false,
        cursor_shape: TerminalCursorShape::Block,
        cursor_blinking: false,
        is_find_match: false,
        is_find_active: false,
    }
}

/// One terminal pane plus a split divider, drawn and routed as
/// `Surface::Widget`s. `terminal_events` records every
/// `UiEvent::Terminal`; `keys` records every key that bubbled to the app.
struct TerminalApp {
    term: Terminal,
    divider: TerminalDivider,
    terminal_events: Vec<TerminalEvent>,
    keys: Vec<Key>,
}

impl TerminalApp {
    fn new() -> Self {
        let mut rows: Vec<Vec<TerminalCell>> = vec!["$ echo hi", "hi", "$"]
            .into_iter()
            .map(|line| line.chars().map(cell).collect())
            .collect();
        for row in &mut rows {
            row.resize(11, cell(' '));
        }
        rows.resize(5, vec![cell(' '); 11]);
        Self {
            term: Terminal {
                id: terminal_id(),
                cells: rows,
                scrollbar: Some(TerminalScrollbar {
                    total_lines: 50,
                    visible_lines: 5,
                    scroll_offset: 0,
                    inverted: true,
                    width: None,
                }),
            },
            divider: TerminalDivider::new(WidgetId::new("terminal:divider")),
            terminal_events: Vec::new(),
            keys: Vec::new(),
        }
    }
}

impl AppLogic for TerminalApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.term.clone()),
        });
        frame.push(Surface::Widget {
            rect: DIVIDER,
            widget: Box::new(self.divider),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.term.id, AREA, Box::new(self.term.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Terminal(id, ev) if id == terminal_id() => self.terminal_events.push(ev),
            UiEvent::KeyPressed { key, .. } => self.keys.push(key),
            _ => {}
        }
        Reaction::Continue
    }
}

fn row_text(driver: &TuiDriver<TerminalApp>, y: u16) -> String {
    driver.styled_row(y).into_iter().map(|(ch, _)| ch).collect()
}

#[test]
fn surface_widget_terminal_paints_its_cells_and_scrollbar() {
    let driver = TuiDriver::new(TerminalApp::new(), COLS, ROWS);
    assert!(driver.screen_contains("$ echo hi"), "{}", driver.screen());
    assert!(row_text(&driver, 2).contains("hi"), "{}", driver.screen());

    // The gutter column holds the scrollbar track and thumb, with the
    // inverted (live-at-bottom) thumb at the bottom of the track.
    let gutter_x = (AREA.x + AREA.width - 1.0) as usize;
    let gutter: Vec<char> = (1..6)
        .map(|y| row_text(&driver, y).chars().nth(gutter_x).unwrap())
        .collect();
    assert!(gutter.iter().all(|c| *c == '█' || *c == '░'), "{gutter:?}");
    assert_eq!(gutter.last(), Some(&'█'), "{gutter:?}");
}

#[test]
fn surface_widget_divider_paints_a_separator_column() {
    let driver = TuiDriver::new(TerminalApp::new(), COLS, ROWS);
    for y in 1..6 {
        assert_eq!(
            row_text(&driver, y).chars().nth(DIVIDER.x as usize),
            Some('│'),
            "{}",
            driver.screen()
        );
    }
}

#[test]
fn a_drag_across_cells_starts_extends_and_ends_a_selection() {
    let mut driver = TuiDriver::new(TerminalApp::new(), COLS, ROWS);
    driver.set_double_click_folding(false);

    driver.mouse_down(AREA.x + 2.0, AREA.y + 1.0);
    driver.mouse_move(AREA.x + 5.0, AREA.y + 2.0);
    driver.mouse_up(AREA.x + 5.0, AREA.y + 2.0);

    assert_eq!(
        driver.app().terminal_events,
        vec![
            TerminalEvent::SelectStart { row: 1, col: 2 },
            TerminalEvent::SelectExtend { row: 2, col: 5 },
            TerminalEvent::SelectEnd,
        ]
    );
}

#[test]
fn clicking_the_terminal_focuses_it_and_keys_still_reach_the_app() {
    let mut driver = TuiDriver::new(TerminalApp::new(), COLS, ROWS);
    driver.set_double_click_folding(false);

    driver.click(AREA.x + 1.0, AREA.y);
    assert_eq!(
        driver.backend().focus_tree().focused().map(|(id, _)| *id),
        Some(terminal_id())
    );

    driver.type_char('l');
    assert_eq!(driver.app().keys, vec![Key::Char('l')]);
}

/// The runtime leg of the parity test: one upward wheel notch over the
/// pane resolves to the same `Scroll { delta: -1 }` the assistive
/// `ScrollUp` action and the direct `Interactive::handle` call produce.
#[test]
fn a_wheel_notch_over_the_pane_scrolls_into_history() {
    let mut driver = TuiDriver::new(TerminalApp::new(), COLS, ROWS);
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, 1.0),
        position: Point::new(AREA.x + 3.0, AREA.y + 2.0),
    });
    assert_eq!(
        driver.app().terminal_events,
        vec![TerminalEvent::Scroll { delta: -1 }]
    );
}
