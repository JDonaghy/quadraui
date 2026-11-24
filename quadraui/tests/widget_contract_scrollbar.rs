#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Scrollbar` on the widget contract: the
//! bar reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so every gesture routes through
//! the shared widget-contract engine — hit test, pointer capture,
//! `Interactive::handle` — and comes back out as a
//! `UiEvent::ScrollbarMoved` this app applies to its own scroll state.
//!
//! This is the runtime leg of the slice's event-parity test. The other
//! leg — a direct `Interactive::handle` call, the dispatch shape an
//! out-of-process binding host takes — lives in
//! `src/primitives/scrollbar.rs`'s own `interactive_tests` module, and
//! the two must resolve the same gesture to the same fraction. A
//! scrollbar advertises no `a11y_actions`, so there is no
//! assistive-technology leg to compare: it is chrome for a scrollable
//! region whose own keyboard handling is the accessible path.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, Point, Reaction, Rect, ScreenLayout, ScrollDelta, Scrollbar,
    Surface, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 20;
/// A one-column gutter at the right edge, 20 rows tall.
const TRACK: Rect = Rect::new(39.0, 0.0, 1.0, ROWS as f32);
/// 80 rows of content, 20 visible — a quarter, so `fit_thumb` sizes the
/// thumb at 5 cells and leaves 15 cells of travel.
const TOTAL: f32 = 80.0;
const VISIBLE: f32 = 20.0;

/// One scrollbar, drawn and routed as a `Surface::Widget`. `fraction`
/// holds whatever the last `UiEvent::ScrollbarMoved` reported, the way a
/// host maps the widget's geometry-only answer onto its own scroll state.
struct ScrollbarApp {
    id: WidgetId,
    bar: Scrollbar,
    fractions: Vec<f32>,
}

impl ScrollbarApp {
    fn new() -> Self {
        let id = WidgetId::new("sb:editor");
        Self {
            id,
            bar: Scrollbar::vertical(id, TRACK, 0.0, TOTAL, VISIBLE, 1.0),
            fractions: Vec::new(),
        }
    }

    /// The scroll offset `fraction` names in this app's own content
    /// units — the mapping every host owns, since the descriptor never
    /// carries the content extent.
    fn offset_rows(fraction: f32) -> f32 {
        fraction * (TOTAL - VISIBLE)
    }
}

impl AppLogic for ScrollbarApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: TRACK,
            widget: Box::new(self.bar.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.id, TRACK, Box::new(self.bar.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::ScrollbarMoved { widget, fraction } = event {
            assert_eq!(widget, self.id, "the bar's own id rides the event");
            self.fractions.push(fraction);
            // Apply it the way a host would: re-fit the thumb for the
            // offset the fraction names.
            self.bar = Scrollbar::vertical(
                self.id,
                TRACK,
                Self::offset_rows(fraction),
                TOTAL,
                VISIBLE,
                1.0,
            );
        }
        Reaction::Continue
    }
}

/// The glyphs painted down the gutter column, top to bottom.
fn gutter(driver: &TuiDriver<ScrollbarApp>) -> Vec<char> {
    (0..ROWS)
        .map(|y| {
            driver
                .screen()
                .lines()
                .nth(y as usize)
                .and_then(|line| line.chars().nth(TRACK.x as usize))
                .unwrap_or(' ')
        })
        .collect()
}

fn last_fraction(driver: &TuiDriver<ScrollbarApp>) -> f32 {
    *driver
        .app()
        .fractions
        .last()
        .unwrap_or_else(|| panic!("no ScrollbarMoved reached the app\n{}", driver.screen()))
}

#[test]
fn surface_widget_scrollbar_paints_a_thumb_run_over_a_track() {
    let driver = TuiDriver::new(ScrollbarApp::new(), COLS, ROWS);
    let column = gutter(&driver);
    assert_eq!(
        column.iter().filter(|c| **c == '█').count(),
        5,
        "a quarter-height thumb is 5 of 20 cells: {column:?}\n{}",
        driver.screen()
    );
    assert_eq!(column[0], '█', "thumb starts at the top at offset 0");
    assert_eq!(column[5], '░', "bare track below it");
    assert_eq!(column[19], '░');
}

#[test]
fn dragging_the_thumb_to_the_bottom_scrolls_to_the_end_and_repaints_there() {
    let mut driver = TuiDriver::new(ScrollbarApp::new(), COLS, ROWS);
    // Grab the thumb at the very top of the track and drag past the end
    // of its travel; the fraction clamps rather than overshooting.
    driver.mouse_down(TRACK.x, TRACK.y);
    driver.mouse_move(TRACK.x, TRACK.y + 19.0);
    driver.mouse_up(TRACK.x, TRACK.y + 19.0);
    assert_eq!(last_fraction(&driver), 1.0);

    driver.render();
    let column = gutter(&driver);
    assert_eq!(column[19], '█', "thumb now sits at the bottom: {column:?}");
    assert_eq!(column[0], '░');
}

#[test]
fn a_drag_keeps_routing_to_the_bar_after_the_pointer_leaves_its_column() {
    let mut driver = TuiDriver::new(ScrollbarApp::new(), COLS, ROWS);
    driver.mouse_down(TRACK.x, TRACK.y);
    // Well outside the one-column track — the capture taken on press has
    // to keep the move routed to the bar.
    driver.mouse_move(TRACK.x - 20.0, TRACK.y + 15.0);
    assert_eq!(last_fraction(&driver), 1.0);

    driver.mouse_up(TRACK.x - 20.0, TRACK.y + 15.0);
    // With the capture released, a later move no longer scrolls.
    let before = driver.app().fractions.len();
    driver.mouse_move(TRACK.x - 20.0, TRACK.y + 2.0);
    assert_eq!(driver.app().fractions.len(), before);
}

#[test]
fn clicking_the_bare_track_pages_by_one_viewport() {
    let mut driver = TuiDriver::new(ScrollbarApp::new(), COLS, ROWS);
    // Below a 5-cell thumb sitting at the top: one page forward.
    driver.click(TRACK.x, TRACK.y + 12.0);
    let travel = TRACK.height - 5.0;
    assert_eq!(last_fraction(&driver), 5.0 / travel);

    // And back up again, from the thumb's new position.
    driver.click(TRACK.x, TRACK.y);
    assert_eq!(last_fraction(&driver), 0.0);
}

#[test]
fn a_wheel_notch_over_the_bar_scrolls_a_fraction_of_a_viewport() {
    let mut driver = TuiDriver::new(ScrollbarApp::new(), COLS, ROWS);
    // Negative dy scrolls the content down, i.e. the thumb forward.
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, -1.0),
        position: Point::new(TRACK.x, TRACK.y + 10.0),
    });
    let travel = TRACK.height - 5.0;
    assert_eq!(last_fraction(&driver), 5.0 * 0.25 / travel);

    // Scrolling back up past the top clamps at the start of the track.
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, 4.0),
        position: Point::new(TRACK.x, TRACK.y + 10.0),
    });
    assert_eq!(last_fraction(&driver), 0.0);
}

#[test]
fn a_scrollbar_is_not_a_keyboard_tab_stop() {
    let mut driver = TuiDriver::new(ScrollbarApp::new(), COLS, ROWS);
    driver.click(TRACK.x, TRACK.y);
    assert_eq!(
        driver.backend().focus_tree().focused(),
        None,
        "pressing chrome must not move keyboard focus"
    );
}
