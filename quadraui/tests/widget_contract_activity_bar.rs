#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `ActivityBar` on the widget contract:
//! the rail reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input routes through the shared
//! widget-contract engine — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s routing step (hit test,
//! click-to-focus, `Interactive::handle`) and back out as the
//! `UiEvent::ActivityBar(_, ActivityBarEvent::ItemClicked { .. })` this
//! test's own `AppLogic::handle` records.
//!
//! Never a direct call into `ActivityBar::handle`/`Primitive::layout`
//! that would skip the routing engine.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    ActivityBar, ActivityBarEvent, ActivityItem, AppLogic, Backend, FrameTree, NamedKey, Reaction,
    Rect, ScreenLayout, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 30;
const ROWS: u16 = 10;
/// A 4-column rail down the screen's left edge: 1 accent + 2 icon + 1
/// separator, the cell-grid convention.
const AREA: Rect = Rect::new(0.0, 0.0, 4.0, ROWS as f32);

fn explorer_id() -> WidgetId {
    WidgetId::new("activity:explorer")
}

fn search_id() -> WidgetId {
    WidgetId::new("activity:search")
}

fn settings_id() -> WidgetId {
    WidgetId::new("activity:settings")
}

fn item(id: WidgetId, fallback: &str) -> ActivityItem {
    ActivityItem {
        id,
        icon: fallback.into(),
        tooltip: String::new(),
        is_active: false,
        is_keyboard_selected: false,
    }
}

/// One activity bar routed through the generic `Interactive`/`FrameTree`
/// engine, plus a log of what the app saw.
struct ActivityBarApp {
    bar: ActivityBar,
    activated: Vec<WidgetId>,
    events: Vec<UiEvent>,
}

impl ActivityBarApp {
    fn new() -> Self {
        Self {
            bar: ActivityBar {
                id: WidgetId::new("activity:main"),
                top_items: vec![item(explorer_id(), "E"), item(search_id(), "S")],
                bottom_items: vec![item(settings_id(), "G")],
                active_accent: None,
                selection_bg: None,
                is_keyboard_focused: false,
                style: Default::default(),
                hovered: None,
            },
            activated: Vec::new(),
            events: Vec::new(),
        }
    }
}

impl AppLogic for ActivityBarApp {
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
        if let UiEvent::ActivityBar(_, ActivityBarEvent::ItemClicked { id }) = &event {
            self.activated.push(*id);
        }
        self.events.push(event);
        Reaction::Redraw
    }
}

/// The icon column of the rail — one in from the accent column.
const ICON_X: f32 = 1.5;

#[test]
fn surface_widget_activity_bar_paints_both_groups_in_their_own_rows() {
    let driver = TuiDriver::new(ActivityBarApp::new(), COLS, ROWS);

    let row = |y: usize| -> String {
        driver
            .screen()
            .lines()
            .nth(y)
            .map(|l| l.chars().take(AREA.width as usize).collect())
            .unwrap_or_default()
    };

    assert!(row(0).contains('E'), "{}", driver.screen());
    assert!(row(1).contains('S'), "{}", driver.screen());
    assert!(
        row(ROWS as usize - 1).contains('G'),
        "the bottom group pins to the rail's last row: {}",
        driver.screen()
    );
}

#[test]
fn clicking_a_row_activates_that_item() {
    let mut driver = TuiDriver::new(ActivityBarApp::new(), COLS, ROWS);

    driver.click(ICON_X, 1.0);

    assert_eq!(driver.app().activated, vec![search_id()]);
}

/// The bottom-pinned group lives at the far end of the rail, and
/// `visible_items` lists it first — so this only resolves to `settings`
/// if that order is translated back into the bar's logical sequence.
#[test]
fn clicking_the_bottom_pinned_row_activates_the_bottom_item() {
    let mut driver = TuiDriver::new(ActivityBarApp::new(), COLS, ROWS);

    driver.click(ICON_X, (ROWS - 1) as f32);

    assert_eq!(driver.app().activated, vec![settings_id()]);
}

#[test]
fn a_click_on_a_blank_rail_row_activates_nothing() {
    let mut driver = TuiDriver::new(ActivityBarApp::new(), COLS, ROWS);

    driver.click(ICON_X, 5.0);

    assert!(
        driver.app().activated.is_empty(),
        "a click between the two groups must activate nothing: {:?}",
        driver.app().activated
    );
}

/// Down/Up walk the rail's own cursor once a click has given it keyboard
/// focus, and Enter activates whatever the cursor landed on.
#[test]
fn arrow_keys_walk_the_rail_and_enter_activates_the_cursor() {
    let mut driver = TuiDriver::new(ActivityBarApp::new(), COLS, ROWS);

    // Click the first row: activates `explorer` and, because the rail is
    // a focus stop, hands it keyboard focus with the cursor parked there.
    driver.click(ICON_X, 0.0);
    assert_eq!(driver.app().activated, vec![explorer_id()]);

    driver.press_named(NamedKey::Down);
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        driver.app().activated,
        vec![explorer_id(), search_id()],
        "Down should move the cursor to the second item"
    );

    driver.press_named(NamedKey::Up);
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        driver.app().activated,
        vec![explorer_id(), search_id(), explorer_id()],
        "Up should move the cursor back"
    );

    // Up at the first item saturates rather than wrapping to the last.
    driver.press_named(NamedKey::Up);
    driver.press_named(NamedKey::Enter);
    assert_eq!(
        driver.app().activated,
        vec![explorer_id(), search_id(), explorer_id(), explorer_id()],
        "Up must saturate at the first item, not wrap to the bottom group"
    );
}

/// A key a host maps to its own engine (vim-style `j`) is never claimed
/// by the rail, so it still bubbles to the app.
#[test]
fn an_unmapped_key_still_reaches_the_app() {
    let mut driver = TuiDriver::new(ActivityBarApp::new(), COLS, ROWS);
    driver.click(ICON_X, 0.0);

    driver.press(quadraui::Key::Char('j'));

    assert!(
        driver.app().events.iter().any(
            |e| matches!(e, UiEvent::KeyPressed { key, .. } if *key == quadraui::Key::Char('j'))
        ),
        "the raw keystroke must bubble to the app: {:?}",
        driver.app().events
    );
}
