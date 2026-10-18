#![cfg(feature = "tui")]
//! Headless `TuiDriver` coverage for the f32 scroll seam at the TUI paint
//! boundary.
//!
//! Scroll offsets are continuous (`f32`) in native units, but a terminal
//! can only start painting at a whole row. These tests drive the real
//! `event -> handle -> render` path and assert the two halves of that
//! contract:
//!
//! * a sub-row offset is *rounded* when it reaches the rasteriser — the
//!   painted top row is `scroll_rows(offset)`, never a truncated or
//!   doubly-rounded index;
//! * the `f32` offset remains the source of truth between frames — a
//!   gesture smaller than one row accumulates instead of being discarded,
//!   so repeated sub-row scrolling eventually crosses a row boundary.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    scroll_by_clamped_f32, wheel_delta, AppLogic, Backend, Decoration, ListItem, ListView,
    Reaction, Rect, StyledText, UiEvent, WidgetId,
};

const VIEWPORT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 20.0,
    height: 5.0,
};
const ROW_COUNT: usize = 20;

/// Minimal `AppLogic` holding one `ListView` whose `scroll_offset` is a
/// continuous `f32`. Wheel events move it by a caller-chosen fraction of a
/// row so the test can park the offset between two rows.
struct ListApp {
    list: ListView,
    /// Rows moved by one notch of `ScrollDelta::y`. Fractional values
    /// produce sub-row offsets.
    rows_per_notch: f32,
}

impl ListApp {
    fn new(rows_per_notch: f32) -> Self {
        let list = ListView {
            id: WidgetId::new("test:list"),
            items: (0..ROW_COUNT)
                .map(|i| ListItem {
                    text: StyledText::plain(format!("Row {i:02}")),
                    icon: None,
                    detail: None,
                    decoration: Decoration::default(),
                })
                .collect(),
            selected_idx: 0,
            scroll_offset: 0.0,
            has_focus: true,
            title: None,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        };
        Self {
            list,
            rows_per_notch,
        }
    }

    fn scroll_offset(&self) -> f32 {
        self.list.scroll_offset
    }

    fn set_scroll_offset(&mut self, offset: f32) {
        self.list.scroll_offset = offset;
    }
}

impl AppLogic for ListApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_list(VIEWPORT, &self.list);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Scroll { delta, .. } => {
                let dy = wheel_delta(delta.y, self.rows_per_notch);
                self.list.scroll_offset =
                    scroll_by_clamped_f32(self.list.scroll_offset, dy, (ROW_COUNT - 1) as f32);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

/// Drive a driver parked at `offset` and return its rendered screen.
fn screen_at(offset: f32) -> String {
    let mut driver = TuiDriver::new(ListApp::new(1.0), 20, 5);
    driver.app_mut().set_scroll_offset(offset);
    driver.render();
    driver.screen()
}

#[test]
fn whole_row_offset_paints_from_that_row() {
    let screen = screen_at(2.0);
    assert!(
        screen.contains("Row 02"),
        "offset 2.0 must paint from row 2:\n{screen}"
    );
    assert!(
        !screen.contains("Row 01"),
        "rows above the offset stay clipped:\n{screen}"
    );
}

#[test]
fn sub_row_offset_below_half_rounds_down_to_the_row_it_sits_on() {
    let screen = screen_at(2.4);
    assert!(
        screen.contains("Row 02"),
        "offset 2.4 rounds down to row 2:\n{screen}"
    );
    assert!(
        !screen.contains("Row 01"),
        "rounding must not expose the row above:\n{screen}"
    );
}

#[test]
fn sub_row_offset_at_or_above_half_rounds_up_to_the_next_row() {
    for offset in [2.5_f32, 2.6, 2.9] {
        let screen = screen_at(offset);
        assert!(
            screen.contains("Row 03"),
            "offset {offset} rounds up to row 3:\n{screen}"
        );
        assert!(
            !screen.contains("Row 02"),
            "offset {offset} must not still paint row 2:\n{screen}"
        );
    }
}

#[test]
fn negative_and_non_finite_offsets_paint_from_the_top() {
    for offset in [-1.0_f32, f32::NAN, f32::INFINITY] {
        let screen = screen_at(offset);
        assert!(
            screen.contains("Row 00"),
            "degenerate offset {offset} must paint from the top:\n{screen}"
        );
    }
}

#[test]
fn sub_row_wheel_gestures_accumulate_until_they_cross_a_row_boundary() {
    // Each notch moves a quarter of a row, so the painted top row must
    // stay put for the first gesture and only advance once the
    // accumulated f32 offset rounds to the next row.
    let mut driver = TuiDriver::new(ListApp::new(0.25), 20, 5);
    driver.render();
    assert!(driver.screen_contains("Row 00"));

    // 0.0 -> 0.25: rounds back to row 0, nothing moves on screen...
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: quadraui::ScrollDelta::new(0.0, -1.0),
        position: quadraui::Point::new(2.0, 2.0),
    });
    driver.render();
    assert_eq!(driver.app().scroll_offset(), 0.25);
    assert!(
        driver.screen_contains("Row 00"),
        "a quarter-row gesture must not jump a whole row:\n{}",
        driver.screen()
    );

    // ...but the fraction is kept, so 0.25 -> 0.5 crosses the boundary.
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: quadraui::ScrollDelta::new(0.0, -1.0),
        position: quadraui::Point::new(2.0, 2.0),
    });
    driver.render();
    assert_eq!(driver.app().scroll_offset(), 0.5);
    assert!(
        driver.screen_contains("Row 01") && !driver.screen_contains("Row 00"),
        "accumulated sub-row offsets must eventually scroll:\n{}",
        driver.screen()
    );
}

#[test]
fn the_f32_offset_not_the_painted_row_is_the_source_of_truth() {
    // Two offsets that paint the same top row still differ in state, so a
    // later gesture resumes from where the user actually left off rather
    // than from the rounded row.
    let mut driver = TuiDriver::new(ListApp::new(0.25), 20, 5);
    driver.app_mut().set_scroll_offset(3.4);
    driver.render();
    assert!(driver.screen_contains("Row 03"));

    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: quadraui::ScrollDelta::new(0.0, -1.0),
        position: quadraui::Point::new(2.0, 2.0),
    });
    assert_eq!(
        driver.app().scroll_offset(),
        3.65,
        "the fractional part survives the frame"
    );
    driver.render();
    assert!(
        driver.screen_contains("Row 04"),
        "crossing the half-row mark advances the painted row:\n{}",
        driver.screen()
    );
}
