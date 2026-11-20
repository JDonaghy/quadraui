//! `CellGridController` demo: a quarterly-sales sheet navigated the way
//! a spreadsheet user expects — arrows, Tab/Enter, Page Up/Down,
//! Home/End, Ctrl+Home/End, Ctrl+Arrow, Shift to extend, click and
//! click-drag range selection, row/column-header and corner clicks, plus
//! in-cell editing (typing a character or F2 starts an edit at the
//! cursor cell, Esc cancels, Enter/Tab/the arrow keys commit and move).
//!
//! The controller owns the cursor, the selection anchor, the scroll
//! offsets, the frozen panes and (while active) the in-progress edit's
//! text buffer; this `AppLogic` owns the cell *content* and nothing
//! else, which is the whole split `CellGrid` is built around (see
//! `quadraui::primitives::cell_grid`'s module doc, "the grid only
//! presents"). Each frame it asks the controller which absolute rows and
//! columns are on screen (`CellGridController::layout`), fills only
//! those cells, and paints the resulting descriptor through
//! `DrawsPrimitives::draw_primitive` — `CellGrid` has no bespoke
//! `Backend::draw_cell_grid` method.
//!
//! Committing an edit hands the raw text to this demo's own toy "engine"
//! (`Self::evaluate`): a leading `=` sums `+`-separated integers (e.g.
//! `=1+2` → `3`); anything else is shown verbatim. A real spreadsheet
//! app wires the same `CellEditCommitted` text into its own engine
//! (`cell.set`) and supplies the resulting formatted value next frame.
//!
//! A status bar reports the cursor cell and the selection's extent in
//! spreadsheet notation (`B3`, `B3:D7`), so the semantic outcome of
//! every key and click is readable on screen rather than inferred from
//! highlight colours.

use std::cell::Cell as StdCell;
use std::collections::HashMap;

use quadraui::primitives::cell_grid::{column_letters, Cell, CellAlign, CellStyle};
use quadraui::{
    AppLogic, Backend, CellGridController, CellGridControllerEvent, Color, DrawsPrimitives,
    InteractionState, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent,
    WidgetId,
};

/// Rows in the demo sheet — more than any terminal shows at once, so
/// paging and scroll-to-follow are exercised for real.
pub const ROW_COUNT: usize = 48;
/// Columns in the demo sheet, likewise wider than the viewport.
pub const COL_COUNT: usize = 14;
/// Leading rows that never scroll: the metric-name header row.
pub const FROZEN_ROWS: usize = 1;
/// Leading columns that never scroll: the region-name column.
pub const FROZEN_COLS: usize = 1;

/// Region names cycled down the frozen first column, so a scrolled row
/// is still identifiable by content in a test or on screen.
const REGIONS: [&str; 6] = ["North", "South", "East", "West", "Central", "Pacific"];

/// Quarterly-sales sheet driven entirely by a `CellGridController`.
pub struct CellGridDemo {
    grid: CellGridController,
    /// The grid's rect as `render` last laid it out. `handle` needs the
    /// same rect the painter used, and `AppLogic::render` takes `&self`
    /// — recomputing it from the backend viewport in `handle` would
    /// silently drift if `render`'s layout ever changed.
    last_grid_rect: StdCell<Rect>,
    /// Raw text a user has committed for a given absolute cell,
    /// overriding `Self::default_cell`'s deterministic placeholder. A
    /// cell with no entry here still paints its placeholder — editing
    /// one cell never requires having "loaded" any others.
    edits: HashMap<(usize, usize), String>,
}

impl CellGridDemo {
    pub fn new() -> Self {
        let mut grid = CellGridController::new("sales-sheet");
        grid.set_dimensions(ROW_COUNT, COL_COUNT);
        grid.set_frozen(FROZEN_ROWS, FROZEN_COLS);
        grid.set_has_focus(true);
        grid.set_show_scrollbar(true);
        Self {
            grid,
            last_grid_rect: StdCell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            edits: HashMap::new(),
        }
    }

    /// Already-formatted placeholder text for one absolute cell, before
    /// any user edit. The frozen top row holds metric names, the frozen
    /// first column holds region names, and the body holds a
    /// deterministic currency figure.
    fn default_cell(row: usize, col: usize) -> Cell {
        match (row, col) {
            (0, 0) => Cell {
                text: "Region".into(),
                style: CellStyle {
                    bold: Some(true),
                    ..CellStyle::default()
                },
            },
            (0, _) => Cell {
                text: format!("Q{} {}", (col - 1) % 4 + 1, 2024 + (col - 1) / 4),
                style: CellStyle {
                    bold: Some(true),
                    align: Some(CellAlign::Center),
                    ..CellStyle::default()
                },
            },
            (_, 0) => Cell::plain(format!("{} {row}", REGIONS[(row - 1) % REGIONS.len()])),
            _ => Cell::plain(format!("${}", (row * 100 + col * 7) * 3)),
        }
    }

    /// The raw text [`Self::start_editing`] seeds F2/Enter editing with
    /// — the user's own committed text if there is one, else the
    /// placeholder's displayed text.
    fn raw_text(&self, row: usize, col: usize) -> String {
        self.edits
            .get(&(row, col))
            .cloned()
            .unwrap_or_else(|| Self::default_cell(row, col).text)
    }

    /// A leading `=` sums the `+`-separated integers that follow (the
    /// toy "engine" this demo stands in for GridCraft's own `cell.set`);
    /// anything else displays verbatim. Non-numeric or malformed terms
    /// are dropped rather than erroring — fine for a demo, not a real
    /// spreadsheet's error-reporting contract.
    fn evaluate(raw: &str) -> String {
        match raw.strip_prefix('=') {
            Some(expr) => {
                let sum: i64 = expr
                    .split('+')
                    .filter_map(|term| term.trim().parse::<i64>().ok())
                    .sum();
                sum.to_string()
            }
            None => raw.to_string(),
        }
    }

    /// The cell actually painted this frame: a user edit's evaluated
    /// value if one was committed, else [`Self::default_cell`]'s own
    /// placeholder (keeping its header/region styling).
    fn formatted_cell(&self, row: usize, col: usize) -> Cell {
        match self.edits.get(&(row, col)) {
            Some(raw) => Cell::plain(Self::evaluate(raw)),
            None => Self::default_cell(row, col),
        }
    }

    /// The grid's rect: the whole viewport minus a one-row status bar.
    fn grid_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let status_h = backend.line_height();
        Rect::new(0.0, 0.0, vp.width, (vp.height - status_h).max(0.0))
    }

    /// Cursor and selection in spreadsheet notation — `B3` for a single
    /// cell, `B3:D7` for a range.
    pub fn selection_label(&self) -> String {
        let (row, col) = self.grid.cursor();
        let a1 = |r: usize, c: usize| format!("{}{}", column_letters(c), r + 1);
        let (top, left, bottom, right) = self.grid.selection_bounds();
        if top == bottom && left == right {
            a1(row, col)
        } else {
            format!("{}:{}", a1(top, left), a1(bottom, right))
        }
    }

    /// How many cells the current selection covers.
    pub fn selected_cell_count(&self) -> usize {
        let (top, left, bottom, right) = self.grid.selection_bounds();
        (bottom - top + 1) * (right - left + 1)
    }

    fn status_bar(&self) -> StatusBar {
        let fg = Color::rgb(225, 225, 225);
        let bg = Color::rgb(35, 45, 65);
        let n = self.selected_cell_count();
        StatusBar {
            id: WidgetId::new("cell-grid-status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " Sel {} ({n} cell{}) ",
                    self.selection_label(),
                    if n == 1 { "" } else { "s" }
                ),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " type/F2 edit | Enter/Tab/arrows commit+move | Esc cancel | q=quit ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for CellGridDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for CellGridDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let rect = Self::grid_rect(backend);
        self.last_grid_rect.set(rect);

        // Ask the controller which absolute rows/columns are on screen,
        // then fill exactly those — `CellGrid::cells` is a window, not
        // the whole sheet.
        let layout = self.grid.layout(backend, rect).layout;
        let cells: Vec<Vec<Cell>> = layout
            .rows
            .iter()
            .map(|r| {
                layout
                    .cols
                    .iter()
                    .map(|c| self.formatted_cell(r.index, c.index))
                    .collect()
            })
            .collect();

        backend.draw_primitive(rect, &self.grid.grid(cells));

        let vp = backend.viewport();
        let status_h = backend.line_height();
        let status_rect = Rect::new(0.0, rect.height, vp.width, status_h.min(vp.height));
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        // `q`/Escape quit only outside an active edit — while editing,
        // both must reach the controller instead: `q` is a character to
        // insert, and Escape cancels the edit rather than the app.
        if !self.grid.is_editing() {
            if let UiEvent::KeyPressed { key, .. } = &event {
                if matches!(key, Key::Char('q') | Key::Named(NamedKey::Escape)) {
                    return Reaction::Exit;
                }
            }
        }
        let rect = self.last_grid_rect.get();
        match self.grid.handle(&event, backend, rect) {
            CellGridControllerEvent::Ignored => Reaction::Continue,
            // The controller has no cell content of its own (see the
            // module doc), so F2 only reports the request — seed the
            // editor with this cell's current raw text ourselves.
            CellGridControllerEvent::CellEditRequested { row, col } => {
                self.grid.start_editing(self.raw_text(row, col));
                Reaction::Redraw
            }
            // Hand the committed raw text to the toy "engine"
            // (`Self::evaluate`) — a real app calls its own `cell.set`
            // here and supplies the resulting formatted value next
            // frame via `Self::formatted_cell`.
            CellGridControllerEvent::CellEditCommitted { row, col, text } => {
                self.edits.insert((row, col), text);
                Reaction::Redraw
            }
            _ => Reaction::Redraw,
        }
    }
}
