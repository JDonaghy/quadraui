//! `CellGrid` demo — a tiny budget sheet: a bold, frozen header row
//! ("Item"/"Qty"/"Price"/"Total") plus four data rows, painted entirely
//! through [`quadraui::DrawsPrimitives::draw_primitive`] — no
//! `Backend::draw_cell_grid` method exists or is needed.
//!
//! A click on a cell moves the cursor there; the four arrow keys move it
//! by one row/column, clamped to the sheet's edges; Shift+arrow extends
//! the rectangular selection instead of moving the cursor alone. The
//! status bar reports the active cell's spreadsheet reference (e.g.
//! `B3`) and its own already-formatted text.

use quadraui::{
    Backend, BackendCaps, Cell, CellGrid, CellGridHit, Color, DrawsPrimitives, Key, LayoutCtx,
    MouseButton, NamedKey, Primitive, Reaction, Rect, StatusBar, StatusBarSegment, Style, UiEvent,
    WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("cell_grid.rs");

/// `Style`'s chrome-metric `token`, floored at the chrome line height on
/// a pixel backend, or the plain chrome line height on TUI's fixed cell
/// grid. Mirrors every other demo's own copy of this carve (e.g.
/// `icon_grid.rs`'s `chrome_strip_height`).
fn chrome_strip_height(backend: &dyn Backend, token: impl Fn(&Style) -> f32) -> f32 {
    let lh = backend.chrome_line_height();
    if lh <= 1.0 {
        backend.snap_height(lh)
    } else {
        backend.snap_height(lh.max(token(&backend.style())))
    }
}

// gallery:begin

/// Bold header cell, plain data cell — a small helper so
/// [`CellGridDemo::new`]'s sheet literal reads as data, not `CellStyle`
/// boilerplate.
fn header_cell(text: &str) -> Cell {
    let mut cell = Cell::plain(text);
    cell.style.bold = Some(true);
    cell
}

fn row(cells: [&str; 4]) -> Vec<Cell> {
    cells.into_iter().map(Cell::plain).collect()
}

pub struct CellGridDemo {
    cells: Vec<Vec<Cell>>,
    cursor: (usize, usize),
    selection_anchor: Option<(usize, usize)>,
    has_focus: bool,
}

impl CellGridDemo {
    pub fn new() -> Self {
        let cells = vec![
            vec![
                header_cell("Item"),
                header_cell("Qty"),
                header_cell("Price"),
                header_cell("Total"),
            ],
            row(["Widget", "3", "$4.00", "$12.00"]),
            row(["Gadget", "5", "$2.50", "$12.50"]),
            row(["Gizmo", "2", "$9.99", "$19.98"]),
            row(["Thingamajig", "7", "$1.25", "$8.75"]),
        ];
        Self {
            cells,
            cursor: (1, 0),
            selection_anchor: None,
            has_focus: true,
        }
    }

    fn row_count(&self) -> usize {
        self.cells.len()
    }

    fn col_count(&self) -> usize {
        self.cells.first().map_or(0, Vec::len)
    }

    /// `(row_height, col_width)` this demo asks for, derived from the
    /// backend's own chrome metrics — cell-quantised on TUI
    /// (`chrome_line_height() <= 1.0`), pixel-scaled everywhere else.
    /// `CellGrid::default_row_height`/`default_col_width` are raw
    /// surface-native units (unlike `DataTable`'s unit-aware
    /// `ColumnWidth`), so a demo painting on a pixel backend has to pick
    /// real pixel sizes itself rather than reusing the TUI-shaped `1.0`
    /// row height a cell-grid backend wants.
    fn cell_size(backend: &dyn Backend) -> (f32, f32) {
        let lh = backend.chrome_line_height();
        if lh <= 1.0 {
            (1.0, 12.0)
        } else {
            (lh, backend.char_width() * 10.0)
        }
    }

    fn grid(&self, backend: &dyn Backend) -> CellGrid {
        let (row_height, col_width) = Self::cell_size(backend);
        CellGrid {
            id: WidgetId::new("gallery:cell-grid"),
            row_count: self.row_count(),
            col_count: self.col_count(),
            frozen_rows: 1,
            frozen_cols: 0,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: row_height,
            default_col_width: col_width,
            row_heights: Default::default(),
            col_widths: Default::default(),
            show_row_headers: true,
            show_col_headers: true,
            row_labels: Default::default(),
            col_labels: Default::default(),
            cursor: self.cursor,
            selection_anchor: self.selection_anchor,
            cells: self.cells.clone(),
            has_focus: self.has_focus,
            show_scrollbar: false,
        }
    }

    /// The resolved rectangular selection, without needing a `Backend`
    /// to build a full [`CellGrid`] first — see [`CellGrid::selection_bounds`],
    /// which this mirrors exactly (it depends only on `cursor`/
    /// `selection_anchor`, never on pixel sizes).
    fn selection_bounds(&self) -> (usize, usize, usize, usize) {
        let (cr, cc) = self.cursor;
        let (ar, ac) = self.selection_anchor.unwrap_or(self.cursor);
        (cr.min(ar), cc.min(ac), cr.max(ar), cc.max(ac))
    }

    fn grid_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = chrome_strip_height(backend, |s| s.status_bar_height);
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = chrome_strip_height(backend, |s| s.status_bar_height);
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn cell_text(&self, row: usize, col: usize) -> &str {
        self.cells
            .get(row)
            .and_then(|r| r.get(col))
            .map(|c| c.text.as_str())
            .unwrap_or("")
    }

    fn status(&self) -> StatusBar {
        let (r, c) = self.cursor;
        let text = format!(
            " {}{}: {} — arrows move, Shift+arrow selects, click a cell ",
            quadraui::column_letters(c),
            r + 1,
            self.cell_text(r, c),
        );
        StatusBar {
            id: WidgetId::new("gallery:cell-grid:status"),
            left_segments: vec![StatusBarSegment {
                text,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 60, 90),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    /// Moves [`Self::cursor`] by `(dr, dc)`, clamped to the sheet's own
    /// bounds — never off the top/left edge (`saturating_sub`) or past
    /// the bottom/right one. `extend_selection` keeps (or starts) the
    /// selection anchor at the cursor's pre-move position; otherwise the
    /// selection collapses back to just the new cursor cell.
    fn move_cursor(&mut self, dr: isize, dc: isize, extend_selection: bool) {
        let (r, c) = self.cursor;
        let new_r = (r as isize + dr).clamp(0, self.row_count() as isize - 1) as usize;
        let new_c = (c as isize + dc).clamp(0, self.col_count() as isize - 1) as usize;
        if extend_selection {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.cursor);
            }
        } else {
            self.selection_anchor = None;
        }
        self.cursor = (new_r, new_c);
    }
}

impl Default for CellGridDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for CellGridDemo {
    fn name(&self) -> &'static str {
        "Cell Grid"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::grid_rect(area, backend);
        let grid = self.grid(backend);
        backend.draw_primitive(rect, &grid);

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(),
            &quadraui::InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let rect = Self::grid_rect(area, backend);
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let ctx = LayoutCtx {
                    line_height: backend.chrome_line_height(),
                    char_width: backend.char_width(),
                    nerd_fonts_enabled: backend.nerd_fonts_enabled(),
                };
                let grid = self.grid(backend);
                let layout = Primitive::layout(&grid, &ctx, rect);
                match layout.hit_test(position.x, position.y) {
                    CellGridHit::Cell(r, c) => {
                        self.cursor = (r, c);
                        self.selection_anchor = None;
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                }
            }
            UiEvent::KeyPressed {
                key: Key::Named(named),
                modifiers,
                ..
            } => {
                let delta = match named {
                    NamedKey::Up => Some((-1, 0)),
                    NamedKey::Down => Some((1, 0)),
                    NamedKey::Left => Some((0, -1)),
                    NamedKey::Right => Some((0, 1)),
                    _ => None,
                };
                match delta {
                    Some((dr, dc)) => {
                        self.move_cursor(dr, dc, modifiers.shift);
                        Reaction::Redraw
                    }
                    None => Reaction::Continue,
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "cursor": self.cursor,
            "selection_bounds": self.selection_bounds(),
            "row_count": self.row_count(),
            "col_count": self.col_count(),
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_keys_move_the_cursor_and_clear_any_selection() {
        let mut demo = CellGridDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.cursor, (2, 0));
        assert_eq!(demo.selection_anchor, None);
    }

    #[test]
    fn cursor_is_clamped_at_the_sheets_edges() {
        let mut demo = CellGridDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        demo.cursor = (0, 0);
        let left = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Left),
            modifiers: Default::default(),
            repeat: false,
        };
        demo.handle(0, &left, &mut backend, area);
        assert_eq!(demo.cursor, (0, 0), "cursor must not move past column 0");

        let up = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Default::default(),
            repeat: false,
        };
        demo.handle(0, &up, &mut backend, area);
        assert_eq!(demo.cursor, (0, 0), "cursor must not move past row 0");
    }

    #[test]
    fn shift_arrow_extends_the_selection_instead_of_moving_alone() {
        let mut demo = CellGridDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        demo.cursor = (1, 1);
        let shift_right = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: quadraui::Modifiers {
                shift: true,
                ..Default::default()
            },
            repeat: false,
        };
        demo.handle(0, &shift_right, &mut backend, area);
        assert_eq!(demo.cursor, (1, 2));
        assert_eq!(demo.selection_anchor, Some((1, 1)));
        assert_eq!(demo.selection_bounds(), (1, 1, 1, 2));
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn clicking_a_body_cell_moves_the_cursor_there() {
        let mut demo = CellGridDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let rect = CellGridDemo::grid_rect(area, &backend);
        let ctx = LayoutCtx {
            line_height: backend.chrome_line_height(),
            char_width: backend.char_width(),
            nerd_fonts_enabled: backend.nerd_fonts_enabled(),
        };
        let grid = demo.grid(&backend);
        let layout = Primitive::layout(&grid, &ctx, rect);
        // Row 1 ("Widget", the first data row) is a frozen-plus-scrolled
        // row right after the header, column 0.
        let target = layout
            .layout
            .rows
            .iter()
            .find(|r| r.index == 1)
            .zip(layout.layout.cols.iter().find(|c| c.index == 0))
            .expect("row 1 / col 0 must be visible");
        let event = UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: quadraui::Point::new(
                rect.x + target.1.start + 0.5,
                rect.y + target.0.start + 0.5,
            ),
            modifiers: Default::default(),
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.cursor, (1, 0));
    }
}
