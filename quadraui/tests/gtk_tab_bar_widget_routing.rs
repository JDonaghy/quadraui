#![cfg(feature = "gtk")]
//! Headless `GtkDriver` test that a pixel-backend click lands on the tab
//! the frame actually painted.
//!
//! The GTK leg of `tab_bar_interactive_driver.rs`: a `TabBar` painted
//! through `DrawsPrimitives::draw_dyn` and clicked through the real
//! `UiEvent → preprocess_event → Interactive::handle` routing engine, on
//! a backend whose chrome font is proportional. The click point comes
//! from the frame's painted-layout record (`GtkDriver::tab_center`), so
//! this only passes when the layout the router hit-tests against is the
//! same one that was painted — i.e. when both measure labels through
//! `LayoutCtx::text_width` rather than one of them approximating every
//! glyph as `char_width` wide.
//!
//! The labels are chosen so that divergence accumulates past a whole
//! tab slot (see [`LABELS`]), making a router built on the approximation
//! land on the wrong tab rather than merely a few pixels off-centre.

use quadraui::gtk::testing::GtkDriver;
use quadraui::primitive::{LayoutCtx, Primitive};
use quadraui::{
    AppLogic, Backend, DrawsPrimitives, FrameTree, Reaction, Rect, TabBar, TabBarEvent, TabBarHit,
    TabItem, UiEvent, WidgetId,
};

const TAB_BAR_RECT: Rect = Rect::new(0.0, 0.0, 900.0, 35.0);

/// Four narrow labels then a wide one. In a proportional chrome font
/// `i` measures far narrower than one nominal `char_width` and `W` far
/// wider, so the drift between a real measurement and the
/// `char_width * chars` approximation accumulates across the narrow run:
/// by the last tab it exceeds a whole tab slot, which is what makes a
/// click at the painted centre land on a different tab (or on nothing)
/// if the router resolves its own layout from the approximation.
const LABELS: [&str; 5] = [
    "iiiiiiiiiii1",
    "iiiiiiiiiii2",
    "iiiiiiiiiii3",
    "iiiiiiiiiii4",
    "WWWWWWWWWWW5",
];

/// Minimal `AppLogic` wiring one `TabBar` through the generic
/// `Interactive`/`FrameTree` routing engine — the GTK twin of
/// `tab_bar_interactive_driver.rs`'s own driver app. Paints through
/// `draw_dyn` (not `draw_primitive`) because that is the call that
/// registers the bar in the frame's painted-layout record, which is what
/// `GtkDriver::tab_center` reads back.
struct TabBarRoutingApp {
    id: WidgetId,
    bar: TabBar,
    activated: Vec<usize>,
}

impl TabBarRoutingApp {
    fn new() -> Self {
        let id = WidgetId::new("tabs:gtk-routing");
        let bar = TabBar {
            id,
            tabs: LABELS
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == 0,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        Self {
            id,
            bar,
            activated: Vec::new(),
        }
    }
}

impl AppLogic for TabBarRoutingApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_dyn(TAB_BAR_RECT, &self.bar, &());
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.id, TAB_BAR_RECT, Box::new(self.bar.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::TabBar(id, TabBarEvent::TabActivated { index }) = &event else {
            return Reaction::Continue;
        };
        if *id != self.id {
            return Reaction::Continue;
        }
        for (i, tab) in self.bar.tabs.iter_mut().enumerate() {
            tab.is_active = i == *index;
        }
        self.activated.push(*index);
        Reaction::Redraw
    }
}

/// Clicking the painted centre of each tab in turn activates that tab,
/// not a neighbour.
#[test]
fn clicking_each_painted_tab_centre_activates_that_tab_on_gtk() {
    for tab_idx in 0..LABELS.len() {
        let mut driver = GtkDriver::new(TabBarRoutingApp::new(), 900, 60);
        let id = driver.app().id;

        let (x, y) = driver
            .tab_center(&id, tab_idx)
            .unwrap_or_else(|| panic!("tab {tab_idx} must be painted and in view"));
        driver.click(x, y);

        assert_eq!(
            driver.app().activated,
            vec![tab_idx],
            "a click at the painted centre of tab {tab_idx} ({x}, {y}) must route to that \
             tab — the router and the painter must resolve the same layout"
        );
        assert!(driver.app().bar.tabs[tab_idx].is_active);
    }
}

/// Guards the premise [`LABELS`] is chosen for: on this backend's chrome
/// font the drift between a real measurement and the `char_width * chars`
/// approximation really does exceed a whole tab slot, so the test above
/// would fail — land on a *different* tab, not merely a few pixels off
/// its centre — if the router resolved its layout without a measurer.
///
/// Without this, a font change that shrank the drift to a couple of
/// pixels would silently turn the test above into a far weaker check
/// that still passed.
#[test]
fn the_measurer_free_layout_puts_a_different_tab_under_the_painted_centres() {
    let mut driver = GtkDriver::new(TabBarRoutingApp::new(), 900, 60);
    let id = driver.app().id;
    driver.render();

    let painted_centres: Vec<(f32, f32)> = (0..LABELS.len())
        .map(|i| {
            driver
                .tab_center(&id, i)
                .unwrap_or_else(|| panic!("tab {i} must be painted and in view"))
        })
        .collect();

    // The same shared layout the router runs, minus the measurer: every
    // other input (rect, padding constants, the bar itself) is identical,
    // so any disagreement below is glyph measurement alone.
    let backend = driver.backend();
    let ctx = LayoutCtx::new(
        LayoutCtx::resolve_row_height(backend.chrome_line_height(), &backend.style()),
        backend.char_width(),
        backend.nerd_fonts_enabled(),
    );
    let approximated = Primitive::layout(&driver.app().bar, &ctx, TAB_BAR_RECT);

    let (x, y) = painted_centres[LABELS.len() - 1];
    let hit = approximated
        .layout
        .hit_test(x - TAB_BAR_RECT.x, y - TAB_BAR_RECT.y);
    assert_ne!(
        hit,
        TabBarHit::Tab(LABELS.len() - 1),
        "the painted centre of the last tab ({x}, {y}) must fall outside that tab in the \
         measurer-free layout — otherwise the drift these labels are chosen for no longer \
         exceeds a tab slot and the routing test above proves little; approximated hit was \
         {hit:?}"
    );
}
