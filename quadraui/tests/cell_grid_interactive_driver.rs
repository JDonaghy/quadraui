#![cfg(feature = "tui")]
//! Headless `TuiDriver` test for `CellGrid`'s own `Interactive` impl:
//! keyboard navigation, mouse click/drag and inline editing, all driven
//! through the real `event -> handle -> render` path — `UiEvent` into
//! `quadraui::runtime::preprocess_event`'s widget-contract routing (hit
//! test, click-to-focus, `Interactive::handle`) and back out as a
//! `UiEvent::CellGrid` this test's own `AppLogic::handle` applies onto
//! the owned `CellGrid`, then the next `render` paints it — never a
//! direct call into `CellGrid::handle`/`Primitive::layout` that would
//! skip the routing engine entirely.
//!
//! `CellGrid` has no bespoke `Backend::draw_cell_grid` method (see that
//! primitive's module doc), so `render` below paints it through
//! `Backend::draw_primitive`, the same generic path every other
//! `Primitive`-only widget goes through.
//!
//! `examples/common/cell_grid_demo.rs` drives this same `Interactive`
//! impl through its own hand-rolled, band-based dispatch instead of
//! `AppLogic::frame_tree`; this test exercises the generic
//! `FrameTree`/routing-engine path a host that registers with
//! `frame_tree` gets for free.

use quadraui::primitives::cell_grid::CellEditState;
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, Cell, CellGrid, CellGridEvent, DrawsPrimitives, FrameTree, Modifiers,
    NamedKey, Reaction, Rect, UiEvent, WidgetId,
};

const GRID_RECT: Rect = Rect::new(0.0, 0.0, 40.0, 10.0);

/// Minimal `AppLogic` wiring one `CellGrid` through the generic
/// `Interactive`/`FrameTree` routing engine — used only by this test;
/// `examples/tui_cell_grid.rs` drives the same primitive through its own
/// hand-rolled dispatch instead (see this module's own doc).
struct CellGridDriverApp {
    id: WidgetId,
    grid: CellGrid,
}

impl CellGridDriverApp {
    fn new() -> Self {
        let id = WidgetId::new("grid");
        let cells = (0..8)
            .map(|r| {
                (0..8)
                    .map(|c| Cell::plain(format!("{}{}", quadraui::column_letters(c), r + 1)))
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut grid = CellGrid::new(id);
        grid.row_count = 50;
        grid.col_count = 20;
        grid.default_col_width = 4.0;
        grid.cells = cells;
        Self { id, grid }
    }
}

impl AppLogic for CellGridDriverApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_primitive(GRID_RECT, &self.grid);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.id, GRID_RECT, Box::new(self.grid.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::CellGrid(id, cell_event) = &event else {
            return Reaction::Continue;
        };
        if *id != self.id {
            return Reaction::Continue;
        }
        // Any routed `CellGrid` event implies the click-to-focus/
        // routed-to-the-focused-widget rule already picked this widget,
        // so it's safe to assume focus here without a second
        // `UiEvent::FocusChanged` round trip.
        self.grid.has_focus = true;
        match cell_event {
            CellGridEvent::Navigated {
                cursor,
                selection_anchor,
                row_scroll,
                col_scroll,
            } => {
                self.grid.cursor = *cursor;
                self.grid.selection_anchor = *selection_anchor;
                self.grid.row_scroll = *row_scroll;
                self.grid.col_scroll = *col_scroll;
            }
            CellGridEvent::CellEditRequested { row, col } => {
                let text = self
                    .grid
                    .cells
                    .get(*row)
                    .and_then(|r| r.get(*col))
                    .map(|c| c.text.clone())
                    .unwrap_or_default();
                let cursor = text.len();
                self.grid.editing = Some(CellEditState {
                    text,
                    cursor,
                    selection_anchor: None,
                    placeholder: None,
                });
            }
            CellGridEvent::CellEditStarted { edit, .. }
            | CellGridEvent::CellEditChanged { edit, .. } => {
                self.grid.editing = Some(edit.clone());
            }
            CellGridEvent::CellEditCommitted { row, col, text } => {
                if let Some(cell) = self.grid.cells.get_mut(*row).and_then(|r| r.get_mut(*col)) {
                    cell.text = text.clone();
                }
                self.grid.editing = None;
            }
            CellGridEvent::CellEditCancelled { .. } => {
                self.grid.editing = None;
            }
            _ => {}
        }
        Reaction::Redraw
    }
}

#[test]
fn renders_the_sheet_with_headers_and_cell_text() {
    let driver = TuiDriver::new(CellGridDriverApp::new(), 60, 15);
    assert!(driver.screen_contains("A1"));
    assert!(driver.screen_contains("B2"));
}

#[test]
fn arrow_keys_move_the_cursor_through_the_real_routing_engine() {
    let mut driver = TuiDriver::new(CellGridDriverApp::new(), 60, 15);

    // Click cell (0, 0) first — `CellGrid::focus` is `FocusSpec::Stop`, so
    // nothing routes a key to it until the click-to-focus step runs.
    let (x, y) = driver.find("A1").expect("header column A, row 1 visible");
    driver.click(x, y);
    assert_eq!(driver.app().grid.cursor, (0, 0));

    driver.press_named(NamedKey::Right);
    assert_eq!(driver.app().grid.cursor, (0, 1));

    driver.press_named(NamedKey::Down);
    assert_eq!(driver.app().grid.cursor, (1, 1));
}

#[test]
fn shift_arrow_extends_the_selection() {
    let mut driver = TuiDriver::new(CellGridDriverApp::new(), 60, 15);
    let (x, y) = driver.find("A1").expect("cell A1 visible");
    driver.click(x, y);

    driver.dispatch(UiEvent::KeyPressed {
        key: quadraui::Key::Named(NamedKey::Right),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
    });

    assert_eq!(driver.app().grid.cursor, (0, 1));
    assert_eq!(driver.app().grid.selection_anchor, Some((0, 0)));
}

#[test]
fn clicking_a_cell_moves_the_cursor_there() {
    let mut driver = TuiDriver::new(CellGridDriverApp::new(), 60, 15);
    let (x, y) = driver.find("C3").expect("cell C3 visible");
    driver.click(x, y);
    assert_eq!(driver.app().grid.cursor, (2, 2));
}

#[test]
fn typing_starts_an_edit_and_enter_commits_it_and_moves_down() {
    let mut driver = TuiDriver::new(CellGridDriverApp::new(), 60, 15);
    let (x, y) = driver.find("A1").expect("cell A1 visible");
    driver.click(x, y);

    driver.type_char('x');
    assert_eq!(
        driver.app().grid.editing.as_ref().map(|e| e.text.as_str()),
        Some("x")
    );

    driver.press_named(NamedKey::Enter);
    assert!(driver.app().grid.editing.is_none());
    assert_eq!(driver.app().grid.cells[0][0].text, "x");
    assert_eq!(driver.app().grid.cursor, (1, 0));
}

#[test]
fn drag_through_the_real_routing_engine_extends_the_selection_then_releases_it() {
    let mut driver = TuiDriver::new(CellGridDriverApp::new(), 60, 15);
    let (x0, y0) = driver.find("A1").expect("cell A1 visible");
    let (x1, y1) = driver.find("C3").expect("cell C3 visible");

    // `drag` is down -> move -> up, all through the real
    // `dispatch`/`preprocess_event` path — not a hand-built
    // `RoutedEvent::Pointer { kind: PointerKind::Up, .. }` the way a
    // `CellGrid::handle` unit test would, since that `Up` is exactly what
    // the runtime previously never produced for a widget-routed capture.
    driver.drag(x0, y0, x1, y1);
    assert_eq!(driver.app().grid.cursor, (2, 2));
    assert_eq!(driver.app().grid.selection_anchor, Some((0, 0)));

    // The drag's `mouse_up` must have released the pointer capture: a
    // later move with no new `mouse_down` has to fall through to plain
    // hover tracking (`route_widget_hover`), not keep routing to the grid
    // as an extending drag — otherwise the selection would follow the
    // bare cursor forever, with every other widget losing Enter/Leave
    // hover besides.
    let (x2, y2) = driver.find("A1").expect("cell A1 still visible");
    driver.mouse_move(x2, y2);
    assert_eq!(driver.app().grid.cursor, (2, 2));
    assert_eq!(driver.app().grid.selection_anchor, Some((0, 0)));
}

#[test]
fn escape_cancels_an_edit_without_moving_the_cursor_or_changing_the_text() {
    let mut driver = TuiDriver::new(CellGridDriverApp::new(), 60, 15);
    let (x, y) = driver.find("A1").expect("cell A1 visible");
    driver.click(x, y);

    driver.type_char('z');
    driver.press_named(NamedKey::Escape);

    assert!(driver.app().grid.editing.is_none());
    assert_eq!(driver.app().grid.cells[0][0].text, "A1");
    assert_eq!(driver.app().grid.cursor, (0, 0));
}

/// A wheel notch over the grid scrolls its body a row through the real
/// routing engine, leaving the cursor where it was.
#[test]
fn the_wheel_scrolls_the_body_without_moving_the_cursor() {
    let mut driver = TuiDriver::new(CellGridDriverApp::new(), 60, 15);
    let (x, y) = driver.find("B2").expect("cell B2 visible");
    let wheel = |dy: f32| UiEvent::Scroll {
        widget: None,
        delta: quadraui::ScrollDelta::new(0.0, dy),
        position: quadraui::Point::new(x, y),
    };

    driver.dispatch(wheel(-1.0));
    driver.dispatch(wheel(-1.0));
    assert_eq!(driver.app().grid.row_scroll, 2);
    assert_eq!(driver.app().grid.cursor, (0, 0));

    driver.dispatch(wheel(1.0));
    assert_eq!(driver.app().grid.row_scroll, 1);

    // Scrolling up past the top stops at the first row.
    driver.dispatch(wheel(5.0));
    assert_eq!(driver.app().grid.row_scroll, 0);
}

/// While a cell is being edited, a click on its text places the caret
/// there, and a drag across the text selects it.
#[test]
fn a_click_and_a_drag_in_the_inline_editor_place_the_caret_and_select() {
    let mut driver = TuiDriver::new(CellGridDriverApp::new(), 60, 15);
    // The clicks below land on one spot in quick succession; each is a
    // separate press, not a double-click.
    driver.set_double_click_folding(false);
    let (x, y) = driver.find("A1").expect("cell A1 visible");
    driver.click(x, y);
    driver.press_named(NamedKey::F(2));
    driver.render();
    let edit = |driver: &TuiDriver<CellGridDriverApp>| {
        let edit = driver.app().grid.editing.clone().expect("editing");
        (edit.cursor, edit.selection_anchor)
    };
    assert_eq!(edit(&driver), (2, None), "F2 seeds the caret at the end");

    let (tx, ty) = driver.find("A1").expect("the editor paints the text");
    driver.click(tx, ty);
    assert_eq!(edit(&driver), (0, None), "the click puts the caret at A");
    assert_eq!(
        driver.app().grid.cursor,
        (0, 0),
        "the cursor cell stays put"
    );

    driver.drag(tx, ty, tx + 1.0, ty);
    assert_eq!(edit(&driver), (1, Some(0)), "the drag selects A");
    assert!(driver.app().grid.editing.is_some(), "still editing");
}
