#![cfg(feature = "tui")]
//! Black-box `TuiDriver` tests for `DropOverlay` on the widget contract.
//!
//! The overlay reaches the frame two ways, and both are driven here
//! through the running app rather than by calling a painter directly:
//!
//! - pushed by a host as a type-erased `Surface::Widget` at
//!   `DropOverlay::bounds()`, painted by `DynWidget::paint` →
//!   `Primitive::paint_cells`;
//! - painted by `TabGroupController::render` while a tab drag hovers a
//!   drop zone, which also registers the `DROP_OVERLAY_ZONE_ID` zone so the
//!   text-free overlay is attributable in the frame inventory.
//!
//! The overlay is `non_interactive!`: a click over it reaches the app as
//! the plain mouse event and takes no focus.

use std::cell::RefCell;

use quadraui::testing::ConformanceDriver;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, BackendWidget, Button, DropOverlay, PaneTab, Reaction, Rect, ScreenLayout,
    SplitDirection, Surface, TabGroupController, Theme, UiEvent, WidgetId, DROP_OVERLAY_ZONE_ID,
};

const COLS: u16 = 60;
const ROWS: u16 = 12;

/// The highlight tint the cell painter applies: `theme.accent_fg`
/// lightened so glyphs on top of it stay legible.
fn highlight_tint(theme: &Theme) -> quadraui::Color {
    let accent = theme.accent_fg;
    quadraui::Color::rgb(
        accent.r.saturating_add(30),
        accent.g.saturating_add(30),
        accent.b.saturating_add(60),
    )
}

fn tinted<A: AppLogic>(driver: &TuiDriver<A>, x: u16, y: u16) -> bool {
    let tint = quadraui::tui::ratatui_color(highlight_tint(&driver.backend().theme()));
    driver.style_at(x, y).map(|s| s.bg) == Some(tint)
}

// ── A host pushing the overlay as a `Surface::Widget` ─────────────────────

/// Paints a button and, on top of it, one drop overlay as a
/// `Surface::Widget` — the shape any host that computes its own overlay
/// uses.
struct OverlayApp {
    overlay: DropOverlay,
    events: Vec<UiEvent>,
}

impl OverlayApp {
    fn new(overlay: DropOverlay) -> Self {
        Self {
            overlay,
            events: Vec::new(),
        }
    }
}

impl AppLogic for OverlayApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: Rect::new(4.0, 3.0, 10.0, 1.0),
            widget: Box::new(Button::new(WidgetId::new("under"), "under")),
        });
        frame.push(Surface::Widget {
            rect: self.overlay.bounds(),
            widget: Box::new(self.overlay.clone()),
        });
        frame.draw(backend);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Continue
    }
}

#[test]
fn surface_widget_highlight_tints_every_cell_in_bounds_and_keeps_glyphs() {
    let overlay = DropOverlay {
        highlight: Some(Rect::new(3.0, 2.0, 8.0, 4.0)),
        insertion_bar: None,
        ghost_position: None,
    };
    let driver = TuiDriver::new(OverlayApp::new(overlay), COLS, ROWS);

    for y in 2..6 {
        for x in 3..11 {
            assert!(
                tinted(&driver, x, y),
                "cell ({x},{y}) is tinted\n{}",
                driver.screen()
            );
        }
    }
    assert!(!tinted(&driver, 2, 2), "left of the highlight is untouched");
    assert!(!tinted(&driver, 11, 2), "the right edge is exclusive");
    assert!(!tinted(&driver, 3, 6), "the bottom edge is exclusive");
    assert!(
        driver.screen_contains("under"),
        "the tint keeps the glyphs underneath:\n{}",
        driver.screen()
    );
}

#[test]
fn surface_widget_insertion_bar_paints_a_vertical_rule() {
    let overlay = DropOverlay {
        highlight: None,
        insertion_bar: Some(Rect::new(20.0, 1.0, 1.0, 3.0)),
        ghost_position: None,
    };
    let driver = TuiDriver::new(OverlayApp::new(overlay), COLS, ROWS);
    let screen = driver.screen();
    let rows: Vec<&str> = screen.lines().collect();
    for row in rows.iter().take(4).skip(1) {
        assert_eq!(row.chars().nth(20), Some('│'), "{screen}");
    }
    assert_ne!(rows[4].chars().nth(20), Some('│'), "{screen}");
    assert_eq!(
        driver.style_at(20, 1).map(|s| s.fg),
        Some(quadraui::tui::ratatui_color(
            driver.backend().theme().accent_fg
        ))
    );
}

#[test]
fn clicking_an_overlay_reaches_the_app_and_takes_no_focus() {
    let overlay = DropOverlay {
        highlight: Some(Rect::new(3.0, 2.0, 8.0, 4.0)),
        insertion_bar: None,
        ghost_position: None,
    };
    let mut driver = TuiDriver::new(OverlayApp::new(overlay), COLS, ROWS);
    driver.click(5.0, 3.0);

    let events = &driver.app().events;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, UiEvent::MouseDown { .. })),
        "the plain mouse-down must reach the app: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, UiEvent::FocusChanged(Some(_)))),
        "a drop overlay must not take focus: {events:?}"
    );
}

// ── `TabGroupController` painting the overlay during a tab drag ───────────

struct Blank;

impl BackendWidget for Blank {
    fn render(&self, _backend: &mut dyn Backend, _rect: Rect) {}
}

fn tab(id: &str, label: &str) -> PaneTab {
    PaneTab {
        id: id.to_string(),
        label: label.to_string(),
        closable: true,
        content: Box::new(Blank),
    }
}

/// Two side-by-side panes; the test starts and moves a tab drag through
/// the controller's own drag API, then re-renders.
struct TabGroupApp {
    ctrl: RefCell<TabGroupController>,
}

impl TabGroupApp {
    fn new() -> Self {
        let mut ctrl = TabGroupController::with_pane(
            "left",
            vec![tab("a", "alpha.rs"), tab("b", "beta.rs")],
            "a",
            SplitDirection::Horizontal,
        );
        ctrl.add_pane_with_tab("right", tab("c", "gamma.rs"));
        Self {
            ctrl: RefCell::new(ctrl),
        }
    }
}

impl AppLogic for TabGroupApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        self.ctrl
            .borrow_mut()
            .render(backend, Rect::new(0.0, 0.0, COLS as f32, ROWS as f32));
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// Start dragging `beta.rs` and hover the middle of the right pane's
/// content area, then repaint.
fn drag_into_right_pane() -> TuiDriver<TabGroupApp> {
    let mut driver = TuiDriver::new(TabGroupApp::new(), COLS, ROWS);
    let (bx, by) = driver.find("beta.rs").expect("beta.rs tab painted");
    let gamma = driver
        .find_bounds("gamma.rs")
        .expect("gamma.rs tab painted");
    {
        let mut ctrl = driver.app().ctrl.borrow_mut();
        assert!(
            ctrl.handle_tab_drag_start(bx, by),
            "drag starts on a tab body"
        );
        let target_x = gamma.x + 4.0;
        let target_y = (ROWS / 2) as f32;
        assert!(ctrl.handle_tab_drag_move(target_x, target_y).is_some());
    }
    driver.render();
    driver
}

#[test]
fn tab_drag_paints_the_highlight_over_the_hovered_pane() {
    let driver = drag_into_right_pane();
    let gamma = driver
        .find_bounds("gamma.rs")
        .expect("gamma.rs tab painted");
    let probe_x = (gamma.x + 4.0) as u16;
    let probe_y = ROWS / 2;
    assert!(
        tinted(&driver, probe_x, probe_y),
        "the hovered drop zone is tinted\n{}",
        driver.screen()
    );
    assert!(
        !tinted(&driver, 1, probe_y),
        "the left pane, which is not a drop target, is untouched"
    );
}

#[test]
fn tab_drag_overlay_is_attributed_by_its_zone() {
    let driver = drag_into_right_pane();
    let inventory = driver.inventory();
    let zone = inventory
        .zones()
        .iter()
        .find(|z| z.id.as_str() == DROP_OVERLAY_ZONE_ID)
        .unwrap_or_else(|| {
            panic!(
                "the overlay registers {DROP_OVERLAY_ZONE_ID:?}; zones: {:?}",
                inventory
                    .zones()
                    .iter()
                    .map(|z| z.id.as_str())
                    .collect::<Vec<_>>()
            )
        });
    let probe = quadraui::Point::new(zone.bounds.x + 1.0, zone.bounds.y + 1.0);
    assert!(zone.bounds.contains(probe));
    assert!(tinted(&driver, probe.x as u16, probe.y as u16));
}

#[test]
fn no_overlay_and_no_zone_without_a_drag() {
    let driver = TuiDriver::new(TabGroupApp::new(), COLS, ROWS);
    assert!(
        !driver
            .inventory()
            .zones()
            .iter()
            .any(|z| z.id.as_str() == DROP_OVERLAY_ZONE_ID),
        "the overlay zone is only registered while a drag is active"
    );
    for y in 0..ROWS {
        for x in 0..COLS {
            assert!(!tinted(&driver, x, y), "cell ({x},{y}) tinted with no drag");
        }
    }
}

// ── Observability on every registered driver ──────────────────────────────

/// A host that paints one overlay with `draw_dyn` and registers its zone,
/// exactly as `TabGroupController::render` does, in a fixture-sized
/// frame. The overlay hands the backend no text, so the zone is the only
/// thing in the frame inventory that attributes it.
struct ZoneFixture;

impl AppLogic for ZoneFixture {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        use quadraui::DrawsPrimitives;
        let cw = backend.char_width();
        let lh = backend.line_height();
        let overlay = DropOverlay {
            highlight: Some(Rect::new(cw * 2.0, lh * 2.0, cw * 20.0, lh * 6.0)),
            insertion_bar: None,
            ghost_position: None,
        };
        let bounds = overlay.bounds();
        let _ = backend.draw_dyn(bounds, &overlay, &());
        backend.register_zone(WidgetId::new(DROP_OVERLAY_ZONE_ID), bounds);
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

fn overlay_zone<D: ConformanceDriver<App = ZoneFixture>>() -> Option<Rect> {
    let inventory =
        D::new_fixture(ZoneFixture, quadraui::testing::LogicalViewport::new(80, 24)).inventory();
    inventory
        .zones()
        .iter()
        .find(|z| z.id.as_str() == DROP_OVERLAY_ZONE_ID)
        .map(|z| z.bounds)
}

#[test]
fn tui_driver_inventory_attributes_a_painted_overlay() {
    assert_eq!(
        overlay_zone::<TuiDriver<_>>(),
        Some(Rect::new(2.0, 2.0, 20.0, 6.0))
    );
}

#[cfg(feature = "gtk")]
#[test]
fn gtk_driver_inventory_attributes_a_painted_overlay() {
    let zone = overlay_zone::<quadraui::gtk::testing::GtkDriver<_>>()
        .expect("the GTK frame inventory reports the overlay zone");
    assert!(zone.width > 0.0 && zone.height > 0.0, "{zone:?}");
}
