#![cfg(feature = "tui")]
//! Black-box tests for the painted-layout record — the backend-neutral
//! store `DrawsPrimitives::draw_dyn` fills for every widget-contract
//! widget it paints (`quadraui::primitive::PaintedWidgets`).
//!
//! Everything here drives the real `event -> handle -> render` path
//! through the headless `TuiDriver`, so what gets asserted is what a
//! live frame actually recorded: the record is written by `draw_dyn`,
//! cleared by the next frame's begin-frame step, skipped for a widget
//! with no id, and resolves both a whole widget and the sub-part under a
//! point through one shared hit-test.
//!
//! The driver half of the same seam — `TuiDriver::tab_center` /
//! `tab_close_center` resolving a bar that only ever painted through
//! `draw_dyn`, with no per-backend layout cache behind it — is
//! the last test in this file. The GTK driver resolves through the same
//! shared helper (`primitives::tab_bar::tab_part_center`), and the
//! workspace demo's end-to-end tab clicks live in
//! `tests/tui_example_driver.rs`.

use quadraui::interactive::SubTarget;
use quadraui::primitive::{painted_hit, with_painted_widgets, LayoutCtx, Primitive};
use quadraui::primitives::tab_bar::{tab_part_center, TabBarSubPart};
use quadraui::{
    A11yInfo, A11yPrimitive, AppLogic, Backend, DrawsPrimitives, FrameTree, Reaction, Rect, TabBar,
    TabBarEvent, TabBarPrimitiveLayout, TabItem, UiEvent, WidgetId,
};

const BAR_RECT: Rect = Rect::new(3.0, 2.0, 50.0, 1.0);

fn bar_id() -> WidgetId {
    WidgetId::new("tabs:record")
}

/// An `AppLogic` that paints one `TabBar` through `draw_dyn` — the widget
/// contract's paint path, with no per-backend rasteriser anywhere,
/// so the only geometry anything can read back is the painted-layout
/// record.
struct RecordApp {
    bar: TabBar,
    /// When `false`, `render` paints nothing at all, which proves the
    /// record is per-frame rather than sticky.
    paint_bar: bool,
    /// When `true`, `render` also paints an id-less widget.
    paint_anonymous: bool,
    activated: Vec<usize>,
    closed: Vec<usize>,
}

impl RecordApp {
    fn new() -> Self {
        let bar = TabBar {
            id: bar_id(),
            tabs: ["alpha", "beta", "gamma"]
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: label.to_string(),
                    is_active: i == 0,
                    is_closable: true,
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
            bar,
            paint_bar: true,
            paint_anonymous: false,
            activated: Vec::new(),
            closed: Vec::new(),
        }
    }
}

impl AppLogic for RecordApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        if self.paint_bar {
            backend.draw_dyn(BAR_RECT, &self.bar, &());
        }
        if self.paint_anonymous {
            backend.draw_dyn(Rect::new(0.0, 6.0, 10.0, 1.0), &Anonymous, &());
        }
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        if self.paint_bar {
            tree.push(self.bar.id, BAR_RECT, Box::new(self.bar.clone()));
        }
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::TabBar(id, bar_event) = &event else {
            return Reaction::Continue;
        };
        if *id != self.bar.id {
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
            _ => {}
        }
        Reaction::Redraw
    }
}

/// A widget with no `WidgetId` of its own — `Primitive::id` keeps its
/// `None` default, so `draw_dyn` has nothing to record it under.
struct Anonymous;

impl A11yPrimitive for Anonymous {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::default()
    }
}

impl Primitive for Anonymous {
    type Layout = Rect;
    type Event = ();

    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
        rect
    }

    fn paint(
        &self,
        _layout: &Rect,
        _surface: &mut dyn quadraui::PaintSurface,
        _env: &quadraui::PaintEnv,
    ) {
    }
}

quadraui::non_interactive!(Anonymous);

/// The painted rect and layout the record holds for the bar, as
/// absolute, downcast-to-concrete values.
fn recorded_bar() -> Option<(Rect, TabBarPrimitiveLayout)> {
    with_painted_widgets(|painted| {
        let (rect, layout) = painted.painted_layout(&bar_id())?;
        Some((
            rect,
            layout
                .downcast_ref::<TabBarPrimitiveLayout>()
                .expect("a painted TabBar records its own TabBarPrimitiveLayout")
                .clone(),
        ))
    })
}

#[test]
fn draw_dyn_records_the_painted_rect_and_a_downcastable_layout() {
    let driver = quadraui::tui::testing::TuiDriver::new(RecordApp::new(), 60, 10);

    let (rect, layout) = recorded_bar().expect("the bar painted through draw_dyn this frame");
    assert_eq!(
        rect, BAR_RECT,
        "the record holds the absolute rect draw_dyn painted into"
    );
    assert_eq!(
        layout.area, BAR_RECT,
        "and the recorded layout is the one that painted"
    );
    assert_eq!(
        layout.layout.visible_tabs.len(),
        3,
        "all three tabs fit in a 50-cell bar:\n{}",
        driver.screen()
    );
}

#[test]
fn the_record_only_describes_the_frame_now_on_screen() {
    let mut driver = quadraui::tui::testing::TuiDriver::new(RecordApp::new(), 60, 10);
    assert!(recorded_bar().is_some(), "frame 1 painted the bar");

    driver.app_mut().paint_bar = false;
    driver.render();

    assert!(
        recorded_bar().is_none(),
        "a widget that stops painting stops being addressable — the \
         record is cleared at the start of every frame"
    );
    assert!(
        with_painted_widgets(|painted| painted.is_empty()),
        "nothing else lingers from the previous frame either"
    );
}

#[test]
fn a_widget_with_no_id_is_not_recorded() {
    let mut app = RecordApp::new();
    app.paint_bar = false;
    app.paint_anonymous = true;
    let _driver = quadraui::tui::testing::TuiDriver::new(app, 60, 10);

    assert!(
        with_painted_widgets(|painted| painted.is_empty()),
        "draw_dyn records a widget only when DynWidget::id answers Some — \
         nothing could address an anonymous widget afterwards"
    );
}

#[test]
fn painted_hit_resolves_the_tab_body_the_close_button_and_dead_space() {
    let _driver = quadraui::tui::testing::TuiDriver::new(RecordApp::new(), 60, 10);

    let (body_x, body_y) =
        tab_part_center(&bar_id(), TabBarSubPart::Tab(1)).expect("tab 1 painted a body");
    let (id, sub) = painted_hit(body_x, body_y).expect("the bar is under its own tab 1");
    assert_eq!(id, bar_id());
    assert_eq!(
        sub.and_then(TabBarSubPart::from_sub_target),
        Some(TabBarSubPart::Tab(1)),
        "a point on tab 1's body resolves to that tab"
    );

    let (close_x, close_y) = tab_part_center(&bar_id(), TabBarSubPart::TabClose(1))
        .expect("tab 1 painted a close button");
    let (id, sub) = painted_hit(close_x, close_y).expect("the bar is under tab 1's close button");
    assert_eq!(id, bar_id());
    assert_eq!(
        sub.and_then(TabBarSubPart::from_sub_target),
        Some(TabBarSubPart::TabClose(1)),
        "a point on a close button resolves to the close button, not the \
         tab body around it"
    );

    // The bar is 50 cells wide and three short tabs don't fill it, so its
    // right edge is dead space: inside the widget, on no sub-part.
    let right_edge = BAR_RECT.x + BAR_RECT.width - 1.0;
    assert_eq!(
        painted_hit(right_edge, BAR_RECT.y),
        Some((bar_id(), None)),
        "dead space inside the bar is the bar with no sub-part"
    );
    assert_eq!(
        painted_hit(BAR_RECT.x, BAR_RECT.y + 4.0),
        None,
        "a point outside every painted widget hits nothing"
    );
}

#[test]
fn a_sub_target_from_another_widget_decodes_to_no_tab_bar_part() {
    assert_eq!(TabBarSubPart::from_sub_target(SubTarget(0)), None);
    assert_eq!(
        TabBarSubPart::from_sub_target(TabBarSubPart::RightSegment(7).sub_target()),
        Some(TabBarSubPart::RightSegment(7)),
        "every part this widget does produce round-trips"
    );
}

#[test]
fn tui_driver_targets_a_draw_dyn_painted_bar_with_no_legacy_cache() {
    let mut driver = quadraui::tui::testing::TuiDriver::new(RecordApp::new(), 60, 10);

    let (x, y) = driver
        .tab_center(&bar_id(), 1)
        .expect("tab_center resolves from the painted-layout record");
    driver.click(x, y);
    assert_eq!(
        driver.app().activated,
        vec![1],
        "clicking the record's own tab center activates that tab:\n{}",
        driver.screen()
    );

    let (cx, cy) = driver
        .tab_close_center(&bar_id(), 2)
        .expect("tab_close_center resolves from the same record");
    driver.click(cx, cy);
    assert_eq!(
        driver.app().closed,
        vec![2],
        "and the close button closes that tab rather than activating it:\n{}",
        driver.screen()
    );
}

/// Painting the same id twice in one frame — a two-pass paint whose
/// first pass corrects its own scroll offset — must leave the record
/// describing the *second* pass, the one on screen.
#[test]
fn a_second_paint_of_the_same_id_wins() {
    struct TwoPass(TabBar);

    impl AppLogic for TwoPass {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_dyn(BAR_RECT, &self.0, &());
            backend.draw_dyn(Rect::new(0.0, 5.0, 50.0, 1.0), &self.0, &());
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    let _driver = quadraui::tui::testing::TuiDriver::new(TwoPass(RecordApp::new().bar), 60, 10);

    let (rect, _) = recorded_bar().expect("the bar painted twice");
    assert_eq!(
        rect,
        Rect::new(0.0, 5.0, 50.0, 1.0),
        "the last paint of an id is the one on screen, so it wins the record"
    );
    assert_eq!(
        with_painted_widgets(|painted| painted.len()),
        2,
        "both passes are still recorded in paint order"
    );
}
