#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `DiffView` on the widget contract: the
//! diff reaches the frame as a type-erased `Surface::Widget`, paints
//! through `Primitive::paint_cells`, and the wheel, PageUp/PageDown, Tab
//! and pane clicks route through `Interactive::handle` back out as
//! `UiEvent::DiffView` — `scroll_offset` and `focused_pane` staying the
//! app's (controlled mode).

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, DiffEditability, DiffHunk, DiffMode, DiffPane, DiffRow, DiffRowKind,
    DiffView, DiffViewEvent, FrameTree, NamedKey, Point, Reaction, Rect, ScreenLayout, ScrollDelta, Surface,
    UiEvent, WidgetId,
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

struct DiffApp {
    view: DiffView,
    events: Vec<DiffViewEvent>,
}

impl AppLogic for DiffApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.view.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.view.id, AREA, Box::new(self.view.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::DiffView(id, event) = event {
            if id == diff_id() {
                match event {
                    DiffViewEvent::Scrolled { scroll_offset } => {
                        self.view.scroll_offset = scroll_offset
                    }
                    DiffViewEvent::PaneFocused { pane } => self.view.focused_pane = pane,
                }
                self.events.push(event);
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }
}

fn driver(mode: DiffMode) -> TuiDriver<DiffApp> {
    TuiDriver::new(
        DiffApp {
            view: view(mode),
            events: Vec::new(),
        },
        COLS,
        ROWS,
    )
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
        assert!(driver.screen_contains(needle), "{needle}:\n{}", driver.screen());
    }
}

#[test]
fn the_wheel_scrolls_three_rows_a_notch_and_stops_at_the_end() {
    let mut driver = driver(DiffMode::SideBySide);
    wheel(&mut driver, -1.0);
    assert_eq!(
        driver.app().events,
        vec![DiffViewEvent::Scrolled { scroll_offset: 3 }]
    );
    assert!(driver.screen_contains("old line 3"), "{}", driver.screen());
    assert!(!driver.screen_contains("old line 1 "), "{}", driver.screen());
    for _ in 0..20 {
        wheel(&mut driver, -1.0);
    }
    let last = driver.app().view.scroll_offset;
    // 30 rows, 9 visible: the furthest offset keeps the last row on screen.
    assert_eq!(last, 21);
    wheel(&mut driver, 1.0);
    assert_eq!(driver.app().view.scroll_offset, 18);
}

#[test]
fn page_keys_scroll_by_the_visible_window() {
    let mut driver = driver(DiffMode::SideBySide);
    driver.click(AREA.x + 5.0, AREA.y + 3.0);
    driver.press_named(NamedKey::PageDown);
    driver.press_named(NamedKey::PageUp);
    assert_eq!(
        driver.app().events,
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
        driver.app().events,
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
    assert!(driver.app().events.is_empty(), "{:?}", driver.app().events);
}
