#![cfg(feature = "tui")]
//! Headless `TuiDriver` test for `TabBar`'s own `Interactive` impl: tab
//! click-to-activate, close-glyph click, and `Left`/`Right`/`Enter`
//! keyboard navigation, all driven through the real `event -> handle ->
//! render` path — `UiEvent` into `quadraui::runtime::preprocess_event`'s
//! widget-contract routing (hit test, click-to-focus,
//! `Interactive::handle`) and back out as a `UiEvent::TabBar` this test's
//! own `AppLogic::handle` applies onto the owned `TabBar`, then the next
//! `render` paints it — never a direct call into
//! `TabBar::handle`/`Primitive::layout` that would skip the routing
//! engine entirely.
//!
//! This is the `TuiDriver` leg of this slice's three-way event-parity
//! test. The other two legs — a direct `Interactive::handle` call (the
//! shape a binding host's own dispatch takes) and
//! `crate::a11y::route_widget_action` (the assistive-technology path) —
//! live as in-crate unit tests in `src/primitives/tab_bar.rs`'s own
//! `interactive_tests` module: `route_widget_action` is not re-exported
//! past this crate's private `a11y` module, so it cannot be named from
//! here.
//!
//! `TabBar` still has its own bespoke `Backend::draw_tab_bar*`, but this
//! test routes it through `Backend::draw_primitive`, the same generic
//! path every other `Primitive`-only widget goes through once it also
//! joins `Interactive` — proving the paint half and the behaviour half
//! agree on one `TabBar`, not just byte-matching rasterisers.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, DrawsPrimitives, FrameTree, NamedKey, Reaction, Rect, TabBar, TabBarEvent,
    TabItem, UiEvent, WidgetId,
};

const TAB_BAR_RECT: Rect = Rect::new(0.0, 0.0, 60.0, 1.0);

/// Minimal `AppLogic` wiring one `TabBar` through the generic
/// `Interactive`/`FrameTree` routing engine — used only by this test.
struct TabBarDriverApp {
    id: WidgetId,
    bar: TabBar,
    activated: Vec<usize>,
    closed: Vec<usize>,
    focus_moved: Vec<usize>,
}

impl TabBarDriverApp {
    fn new() -> Self {
        let id = WidgetId::new("tabs:editor");
        let bar = TabBar {
            id,
            tabs: ["main.rs", "lib.rs", "mod.rs"]
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: label.to_string(),
                    is_active: i == 0,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
            ..Default::default()
        };
        Self {
            id,
            bar,
            activated: Vec::new(),
            closed: Vec::new(),
            focus_moved: Vec::new(),
        }
    }
}

impl AppLogic for TabBarDriverApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_primitive(TAB_BAR_RECT, &self.bar);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.id, TAB_BAR_RECT, Box::new(self.bar.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::TabBar(id, bar_event) = &event else {
            return Reaction::Continue;
        };
        if *id != self.id {
            return Reaction::Continue;
        }
        match bar_event {
            TabBarEvent::TabActivated { index } => {
                for (i, tab) in self.bar.tabs.iter_mut().enumerate() {
                    tab.is_active = i == *index;
                }
                self.activated.push(*index);
            }
            TabBarEvent::TabClosed { index } => self.closed.push(*index),
            TabBarEvent::FocusMoved { index } => self.focus_moved.push(*index),
            TabBarEvent::ButtonClicked { .. } | TabBarEvent::KeyPressed { .. } => {}
            _ => {}
        }
        Reaction::Redraw
    }
}

#[test]
fn renders_every_tab_label() {
    let driver = TuiDriver::new(TabBarDriverApp::new(), 60, 10);
    assert!(driver.screen_contains("main.rs"));
    assert!(driver.screen_contains("lib.rs"));
    assert!(driver.screen_contains("mod.rs"));
}

#[test]
fn clicking_a_tab_body_activates_it_through_the_real_routing_engine() {
    let mut driver = TuiDriver::new(TabBarDriverApp::new(), 60, 10);

    let (x, y) = driver.find("lib.rs").expect("lib.rs tab visible");
    driver.click(x, y);

    assert_eq!(driver.app().activated, vec![1]);
    assert!(driver.app().bar.tabs[1].is_active);
    assert!(!driver.app().bar.tabs[0].is_active);
}

#[test]
fn clicking_a_close_glyph_emits_tab_closed_not_tab_activated() {
    let mut driver = TuiDriver::new(TabBarDriverApp::new(), 60, 10);

    // The close glyph (`×`) sits immediately after a tab's label — find
    // the label, then click one cell past its last character.
    let bounds = driver.find_bounds("mod.rs").expect("mod.rs tab visible");
    let close_x = bounds.x + bounds.width;
    driver.click(close_x, bounds.y);

    assert_eq!(driver.app().closed, vec![2]);
    assert!(driver.app().activated.is_empty());
}

#[test]
fn right_arrow_then_enter_moves_focus_and_activates_through_the_real_routing_engine() {
    let mut driver = TuiDriver::new(TabBarDriverApp::new(), 60, 10);

    // Click first to give the bar keyboard focus — `TabBar::focus` is
    // `FocusSpec::Stop`, so nothing routes a key to it until the
    // click-to-focus step runs.
    let (x, y) = driver.find("main.rs").expect("main.rs tab visible");
    driver.click(x, y);

    driver.press_named(NamedKey::Right);
    assert_eq!(driver.app().focus_moved, vec![1]);

    driver.press_named(NamedKey::Enter);
    // `activated` also carries the initial click's own `TabActivated { index:
    // 0 }` (clicking a tab always activates it, even one that's already
    // active) — `Enter`'s own activation of the now-focused tab 1 is the
    // trailing entry.
    assert_eq!(driver.app().activated, vec![0, 1]);
    assert!(driver.app().bar.tabs[1].is_active);
}
