#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `RadioGroup` on the widget contract:
//! the group reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so every gesture routes through
//! the shared widget-contract engine — hit test, click-to-focus,
//! `Interactive::handle`, `FocusSpec::Children` sub-target cycling — and
//! comes back out as a `UiEvent::RadioGroupSelectionChanged` this app
//! applies to its own `RadioGroup::selected`.
//!
//! This is the runtime leg of the slice's three-way event-parity test.
//! The direct-`Interactive::handle` leg (an out-of-process binding
//! host's dispatch shape) and the `a11y::route_widget_action` leg (the
//! assistive-technology dispatch path) live in
//! `src/primitives/radio_group.rs`'s own `contract_tests` module, and
//! all three must resolve the same gesture to the same `UiEvent`.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, NamedKey, RadioGroup, RadioOption, Reaction, Rect, ScreenLayout,
    Surface, UiEvent, WidgetId,
};

const COLS: u16 = 20;
const ROWS: u16 = 4;
/// One row per option, same shape `RadioGroup`'s own cell `measure()`
/// reports.
const AREA: Rect = Rect::new(1.0, 1.0, 12.0, 2.0);

fn group_id() -> WidgetId {
    WidgetId::new("settings:theme")
}

fn light_id() -> WidgetId {
    WidgetId::new("settings:theme:light")
}

fn dark_id() -> WidgetId {
    WidgetId::new("settings:theme:dark")
}

/// One radio group, drawn and routed as a `Surface::Widget`. `changes`
/// records every `UiEvent::RadioGroupSelectionChanged` index seen, and
/// the app applies it onto `group.selected` the way a real host would.
struct RadioGroupApp {
    group: RadioGroup,
    changes: Vec<usize>,
}

impl RadioGroupApp {
    fn new() -> Self {
        Self {
            group: RadioGroup::new(
                group_id(),
                vec![
                    RadioOption::new(light_id(), "Light"),
                    RadioOption::new(dark_id(), "Dark"),
                ],
            ),
            changes: Vec::new(),
        }
    }
}

impl AppLogic for RadioGroupApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.group.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.group.id, AREA, Box::new(self.group.clone()));
        tree
    }

    /// Makes the group reachable by Tab before any click has focused
    /// it — the same seam [`quadraui::AppLogic::extra_tab_stops`]'s own
    /// doc describes a host using to feed a widget-contract widget into
    /// Tab/Shift+Tab cycling.
    fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        vec![(self.group.id, AREA)]
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::RadioGroupSelectionChanged { widget, index } = event {
            assert_eq!(widget, group_id(), "the group's own id rides the event");
            self.changes.push(index);
            self.group.selected = Some(index);
        }
        Reaction::Continue
    }
}

#[test]
fn surface_widget_radio_group_paints_one_row_per_option() {
    let driver = TuiDriver::new(RadioGroupApp::new(), COLS, ROWS);
    assert!(driver.screen_contains("Light"), "{}", driver.screen());
    assert!(driver.screen_contains("Dark"), "{}", driver.screen());
    let light = driver.find_bounds("Light").expect("Light row");
    let dark = driver.find_bounds("Dark").expect("Dark row");
    assert!(
        light.y < dark.y,
        "Light must paint above Dark: {}",
        driver.screen()
    );
}

#[test]
fn clicking_a_row_selects_that_option() {
    let mut driver = TuiDriver::new(RadioGroupApp::new(), COLS, ROWS);

    driver.click(AREA.x, AREA.y + 1.0);
    assert_eq!(driver.app().changes, vec![1]);
    assert_eq!(driver.app().group.selected, Some(1));

    driver.set_double_click_folding(false);
    driver.click(AREA.x, AREA.y);
    assert_eq!(driver.app().changes, vec![1, 0]);
    assert_eq!(driver.app().group.selected, Some(0));
}

/// Arrow keys walk the group's own cursor once a click has given it
/// keyboard focus with a real sub-target — the same
/// click-then-arrow-keys shape `tests/widget_contract_activity_bar.rs`
/// exercises for its own `FocusSpec::Children` rail.
#[test]
fn arrow_keys_walk_the_selection_once_a_click_has_focused_it() {
    let mut driver = TuiDriver::new(RadioGroupApp::new(), COLS, ROWS);

    driver.click(AREA.x, AREA.y);
    assert_eq!(driver.app().changes, vec![0]);

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().changes, vec![0, 1]);

    driver.press_named(NamedKey::Up);
    assert_eq!(driver.app().changes, vec![0, 1, 0]);
}

/// `FocusSpec::Children` collapses to exactly one `Tab` stop for the
/// whole group ([`crate::frame`]'s own
/// `widget_tab_stops_collapses_focus_spec_children_to_one_stop` test
/// covers that collapse in isolation) — this confirms the host-facing
/// half: `Tab` alone, with no prior click, actually lands keyboard focus
/// on the group (landing with `sub: None`, not any one option), and the
/// arrow keys that reach it afterward still walk the cursor exactly as
/// [`arrow_keys_walk_the_selection_once_a_click_has_focused_it`] already
/// shows for the click-focused case — `RadioGroup::handle`'s `Key` arm
/// seeds off `RadioGroup::selected` alone, never `sub`.
#[test]
fn tab_into_the_group_then_arrow_walks_the_selection() {
    let mut driver = TuiDriver::new(RadioGroupApp::new(), COLS, ROWS);

    driver.press_named(NamedKey::Tab);
    assert!(
        driver.app().changes.is_empty(),
        "Tab alone must not select an option"
    );

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().changes, vec![0]);

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().changes, vec![0, 1]);
}

#[test]
fn a_disabled_option_still_paints_but_ignores_its_click() {
    let mut app = RadioGroupApp::new();
    app.group.options[1].disabled = true;
    let mut driver = TuiDriver::new(app, COLS, ROWS);

    driver.click(AREA.x, AREA.y + 1.0);

    assert!(driver.app().changes.is_empty());
}
