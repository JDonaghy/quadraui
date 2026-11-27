#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `DiffView` on the widget contract: the
//! diff reaches the frame as a type-erased `Surface::Widget`, paints
//! through `Primitive::paint_cells`, and the wheel, PageUp/PageDown, Tab
//! and pane clicks route through `Interactive::handle` back out as
//! `UiEvent::DiffView` — `scroll_offset` and `focused_pane` staying the
//! app's (controlled mode).

#[path = "common/one_widget.rs"]
mod one_widget;

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    DiffEditability, DiffHunk, DiffMode, DiffPane, DiffRow, DiffRowKind, DiffView, DiffViewEvent,
    NamedKey, Point, Rect, ScrollDelta, UiEvent, WidgetId,
};

const COLS: u16 = 64;
const ROWS: u16 = 12;
/// Ten rows: a one-row header and nine of content.
const AREA: Rect = Rect::new(1.0, 1.0, 60.0, 10.0);

fn diff_id() -> WidgetId {
    WidgetId::new("diff:review")
}

/// One 30-row hunk: every fifth row changed, the rest the same.
fn view(mode: DiffMode) -> DiffView {
    let rows: Vec<DiffRow> = (0..30)
        .map(|i| {
            let old = format!("old line {i}");
            if i % 5 == 0 {
                DiffRow {
                    left: Some(old),
                    right: Some(format!("new line {i}")),
                    kind: DiffRowKind::Changed,
                }
            } else {
                DiffRow {
                    left: Some(old.clone()),
                    right: Some(old),
                    kind: DiffRowKind::Same,
                }
            }
        })
        .collect();
    DiffView {
        id: diff_id(),
        left: String::new(),
        right: String::new(),
        left_label: Some("a/file".to_string()),
        right_label: Some("b/file".to_string()),
        hunks: vec![DiffHunk {
            left_start: 1,
            right_start: 1,
            rows,
        }],
        mode,
        editability: DiffEditability::ReadOnly,
        scroll_offset: 0,
        focused_pane: DiffPane::Left,
        has_focus: false,
    }
}

type DiffApp = one_widget::OneWidgetApp<DiffView>;

fn apply(view: &mut DiffView, event: &UiEvent) -> bool {
    match event {
        UiEvent::DiffView(id, ev) if *id == view.id => {
            match ev {
                DiffViewEvent::Scrolled { scroll_offset } => view.scroll_offset = *scroll_offset,
                DiffViewEvent::PaneFocused { pane } => view.focused_pane = *pane,
            }
            true
        }
        _ => false,
    }
}

fn driver(mode: DiffMode) -> TuiDriver<DiffApp> {
    one_widget::driver(view(mode), AREA, COLS, ROWS, apply)
}

fn events(driver: &TuiDriver<DiffApp>) -> Vec<DiffViewEvent> {
    driver
        .app()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::DiffView(id, ev) if *id == diff_id() => Some(*ev),
            _ => None,
        })
        .collect()
}

fn wheel(driver: &mut TuiDriver<DiffApp>, dy: f32) {
    driver.dispatch(UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(0.0, dy),
        position: Point::new(AREA.x + 5.0, AREA.y + 4.0),
    });
}

#[test]
fn surface_widget_diff_paints_labels_and_rows() {
    let driver = driver(DiffMode::SideBySide);
    for needle in ["a/file", "b/file", "old line 1", "new line 0"] {
        assert!(
            driver.screen_contains(needle),
            "{needle}:\n{}",
            driver.screen()
        );
    }
}

#[test]
fn the_wheel_scrolls_three_rows_a_notch_and_stops_at_the_end() {
    let mut driver = driver(DiffMode::SideBySide);
    wheel(&mut driver, -1.0);
    assert_eq!(
        events(&driver),
        vec![DiffViewEvent::Scrolled { scroll_offset: 3 }]
    );
    assert!(driver.screen_contains("old line 3"), "{}", driver.screen());
    assert!(
        !driver.screen_contains("old line 1 "),
        "{}",
        driver.screen()
    );
    for _ in 0..20 {
        wheel(&mut driver, -1.0);
    }
    let last = driver.app().widget.scroll_offset;
    // 30 rows, 9 visible: the furthest offset keeps the last row on screen.
    assert_eq!(last, 21);
    wheel(&mut driver, 1.0);
    assert_eq!(driver.app().widget.scroll_offset, 18);
}

#[test]
fn page_keys_scroll_by_the_visible_window() {
    let mut driver = driver(DiffMode::SideBySide);
    driver.click(AREA.x + 5.0, AREA.y + 3.0);
    driver.press_named(NamedKey::PageDown);
    driver.press_named(NamedKey::PageUp);
    assert_eq!(
        events(&driver),
        vec![
            DiffViewEvent::Scrolled { scroll_offset: 9 },
            DiffViewEvent::Scrolled { scroll_offset: 0 },
        ]
    );
}

#[test]
fn tab_and_clicks_move_focus_between_panes() {
    let mut driver = driver(DiffMode::SideBySide);
    // A click in the left pane, already focused, reports nothing.
    driver.click(AREA.x + 5.0, AREA.y + 3.0);
    driver.press_named(NamedKey::Tab);
    driver.click(AREA.x + 8.0, AREA.y + 5.0);
    driver.click(AREA.x + 45.0, AREA.y + 3.0);
    assert_eq!(
        events(&driver),
        vec![
            DiffViewEvent::PaneFocused {
                pane: DiffPane::Right
            },
            DiffViewEvent::PaneFocused {
                pane: DiffPane::Left
            },
            DiffViewEvent::PaneFocused {
                pane: DiffPane::Right
            },
        ]
    );
}

#[test]
fn tab_in_unified_mode_bubbles() {
    let mut driver = driver(DiffMode::Unified);
    driver.click(AREA.x + 5.0, AREA.y + 3.0);
    driver.press_named(NamedKey::Tab);
    assert!(events(&driver).is_empty(), "{:?}", events(&driver));
}
