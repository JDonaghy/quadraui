#![cfg(feature = "tui")]
//! Black-box `TuiDriver` tests for `TabGroup` on the widget contract: the
//! group reaches the frame as a type-erased `Surface::Widget`, paints its
//! dividers through its `SplitTree` child and each pane's strip through
//! that pane's `TabBar`, and is registered in the app's `FrameTree`, so
//! pointer input routes through the shared engine into
//! `Interactive::handle`.
//!
//! The group is controlled: this app applies every `TabGroupEvent` back
//! onto the descriptor it owns with `TabGroup::apply`, and the next frame
//! paints the result.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, GroupLayout, LayoutCtx, Point, Primitive, Reaction, Rect,
    ScreenLayout, SplitDirection, Surface, TabGroup, TabGroupEvent, TabGroupPane,
    TabGroupPrimitiveLayout, TabGroupTab, Theme, UiEvent, WidgetId,
};

const COLS: u16 = 64;
const ROWS: u16 = 20;
/// A non-zero origin, so the group-local and strip-local translations of
/// every pointer event are exercised.
const AREA: Rect = Rect::new(2.0, 1.0, 58.0, 17.0);

fn group_id() -> WidgetId {
    WidgetId::new("tg:editors")
}

/// `left | (top-right / bottom-right)`: alpha.rs and beta.rs on the left,
/// gamma.rs top right, delta.rs bottom right.
fn editors() -> TabGroup {
    TabGroup::from_layout(
        group_id(),
        vec![
            TabGroupPane::new(
                "left",
                vec![
                    TabGroupTab::new("a", "alpha.rs"),
                    TabGroupTab::new("b", "beta.rs"),
                ],
                "a",
            ),
            TabGroupPane::new("top-right", vec![TabGroupTab::new("c", "gamma.rs")], "c"),
            TabGroupPane::new("bottom-right", vec![TabGroupTab::new("d", "delta.rs")], "d"),
        ],
        GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(GroupLayout::Leaf(1)),
                second: Box::new(GroupLayout::Leaf(2)),
            }),
        },
        SplitDirection::Horizontal,
    )
    .expect("valid layout")
}

/// The app that owns the group and applies what it reports.
struct EditorsApp {
    group: TabGroup,
    events: Vec<TabGroupEvent>,
    applied: Vec<TabGroupEvent>,
}

impl AppLogic for EditorsApp {
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

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::TabGroup(id, event) = event {
            assert_eq!(id, group_id());
            self.events.push(event.clone());
            let applied = self.group.apply(event);
            self.applied.extend(applied);
        }
        Reaction::Redraw
    }
}

fn driver() -> TuiDriver<EditorsApp> {
    TuiDriver::new(
        EditorsApp {
            group: editors(),
            events: Vec::new(),
            applied: Vec::new(),
        },
        COLS,
        ROWS,
    )
}

/// The group's layout as the driver paints it: whole cells.
fn layout(group: &TabGroup) -> TabGroupPrimitiveLayout {
    Primitive::layout(group, &LayoutCtx::new(1.0, 1.0, false), AREA)
}

fn at(driver: &TuiDriver<EditorsApp>, needle: &str) -> (f32, f32) {
    let b = driver
        .find_bounds(needle)
        .unwrap_or_else(|| panic!("{needle} on screen:\n{}", driver.screen()));
    (b.x + 1.0, b.y)
}

fn cell(driver: &TuiDriver<EditorsApp>, x: u16, y: u16) -> char {
    driver.styled_row(y)[x as usize].0
}

#[test]
fn strips_and_dividers_paint_with_a_junction_where_dividers_meet() {
    let d = driver();
    let l = layout(&d.app().group);
    for (k, label) in ["alpha.rs", "gamma.rs", "delta.rs"].iter().enumerate() {
        let (x, y) = at(&d, label);
        assert!(
            l.panes[k].strip_bounds.contains(Point::new(x, y)),
            "{label} paints in pane {k}'s strip\n{}",
            d.screen()
        );
    }
    let column = l.dividers.dividers[0].cell_position();
    let row = l.dividers.dividers[1].cell_position();
    // The side-by-side divider runs the full height of the group.
    for y in AREA.y as u16 + 1..(AREA.y + AREA.height) as u16 {
        if y != row {
            assert_eq!(cell(&d, column, y), '│', "row {y}\n{}", d.screen());
        }
    }
    // The stacked divider tees into it rather than stopping short.
    assert_eq!(cell(&d, column, row), '├', "\n{}", d.screen());
    assert_eq!(cell(&d, column + 1, row), '─');
}

#[test]
fn clicking_an_inactive_tab_activates_it() {
    let mut d = driver();
    let (x, y) = at(&d, "beta.rs");
    d.click(x, y);
    assert_eq!(
        d.app().events,
        vec![TabGroupEvent::TabActivated {
            pane_idx: 0,
            tab_id: "b".into()
        }]
    );
    assert_eq!(d.app().group.pane(0).map(|p| p.active_tab_id()), Some("b"));
}

#[test]
fn clicking_a_close_glyph_closes_that_tab() {
    let mut d = driver();
    let l = layout(&d.app().group);
    let strip = l.panes[0].strip_bounds;
    let (cx, cy) = l.panes[0]
        .tabs
        .layout
        .tab_close_center(1)
        .expect("beta.rs paints a close glyph");
    d.click(strip.x + cx, strip.y + cy);
    assert_eq!(
        d.app().events,
        vec![TabGroupEvent::TabClosed {
            pane_idx: 0,
            tab_id: "b".into()
        }]
    );
    assert!(!d.screen_contains("beta.rs"), "{}", d.screen());
}

#[test]
fn clicking_a_strip_new_tab_button_requests_a_tab() {
    let mut d = driver();
    let l = layout(&d.app().group);
    let strip = l.panes[1].strip_bounds;
    let segment = l.panes[1]
        .tabs
        .layout
        .visible_segments
        .first()
        .expect("the + button paints")
        .bounds;
    d.click(strip.x + segment.x + 1.0, strip.y);
    assert_eq!(
        d.app().events,
        vec![TabGroupEvent::NewTabRequested { pane_idx: 1 }]
    );
}

#[test]
fn pressing_a_pane_body_focuses_that_pane_once() {
    let mut d = driver();
    let l = layout(&d.app().group);
    let body = l.panes[2].content_bounds;
    let (x, y) = (body.x + 3.0, body.y + 2.0);
    d.click(x, y);
    d.click(x, y + 1.0);
    assert_eq!(
        d.app().events,
        vec![TabGroupEvent::PaneFocused { pane_idx: 2 }]
    );
}

#[test]
fn dragging_a_divider_moves_it_under_the_pointer() {
    let mut d = driver();
    let l = layout(&d.app().group);
    let column = f32::from(l.dividers.dividers[0].cell_position());
    let y = AREA.y + 4.0;
    let target = column + 9.0;
    d.mouse_down(column, y);
    d.mouse_move(target, y);
    d.mouse_up(target, y);

    assert!(
        matches!(
            d.app().applied.as_slice(),
            [TabGroupEvent::DividerResized { divider_idx: 0, .. }, ..]
        ),
        "{:?}",
        d.app().events
    );
    let moved = layout(&d.app().group).dividers.dividers[0].cell_position();
    assert_eq!(f32::from(moved), target);
    assert_eq!(cell(&d, moved, y as u16), '│', "\n{}", d.screen());
}

/// The highlight tint the drop overlay's cell painter applies.
fn tinted(driver: &TuiDriver<EditorsApp>, x: u16, y: u16) -> bool {
    let theme: Theme = driver.backend().theme();
    let accent = theme.accent_fg;
    let tint = quadraui::Color::rgb(
        accent.r.saturating_add(30),
        accent.g.saturating_add(30),
        accent.b.saturating_add(60),
    );
    driver.style_at(x, y).map(|s| s.bg) == Some(quadraui::tui::ratatui_color(tint))
}

#[test]
fn dragging_a_tab_into_another_pane_previews_and_then_moves_it() {
    let mut d = driver();
    let l = layout(&d.app().group);
    let target = l.panes[2].content_bounds;
    let (tx, ty) = (
        target.x + target.width / 2.0,
        target.y + target.height / 2.0,
    );
    let (bx, by) = at(&d, "beta.rs");

    d.mouse_down(bx, by);
    d.mouse_move(tx, ty);
    assert!(
        tinted(&d, tx as u16, ty as u16),
        "the hovered pane shows the drop overlay\n{}",
        d.screen()
    );
    assert!(
        !tinted(&d, (AREA.x + 3.0) as u16, ty as u16),
        "the source pane is not a drop target here"
    );
    d.mouse_up(tx, ty);

    assert!(
        matches!(
            d.app().events.last(),
            Some(TabGroupEvent::TabDropped { tab_id, .. }) if tab_id == "b"
        ),
        "{:?}",
        d.app().events
    );
    assert!(
        matches!(
            d.app().applied.last(),
            Some(TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 2,
                ..
            })
        ),
        "{:?}",
        d.app().applied
    );
    let (x, y) = at(&d, "beta.rs");
    assert!(
        layout(&d.app().group).panes[2]
            .strip_bounds
            .contains(Point::new(x, y)),
        "beta.rs now paints in the bottom-right strip\n{}",
        d.screen()
    );
    assert!(!tinted(&d, tx as u16, ty as u16), "the overlay is gone");
}
