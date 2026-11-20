//! `CellGridController` demo: a virtual quarterly-sales sheet navigated
//! the way a spreadsheet user expects — arrows, Tab/Enter, Page
//! Up/Down, Home/End, Ctrl+Home/End, Ctrl+Arrow, Shift to extend, click
//! and click-drag range selection, row/column-header and corner clicks,
//! plus in-cell editing (typing a character or F2 starts an edit at the
//! cursor cell, Esc cancels, Enter/Tab/the arrow keys commit and move).
//!
//! The sheet is `ROW_COUNT` × `COL_COUNT` — far larger than any
//! terminal or window shows at once — and every cell's text is computed
//! on demand from its own `(row, col)` ([`Self::default_cell`]), never
//! materialised up front: the only cells that exist as real values are
//! the ones a user has actually edited ([`Self::edits`]).
//!
//! Four bands, stacked top to bottom and all pushed through one
//! [`ScreenLayout`] so the whole screen — tabs, formula bar, grid, and
//! status bar — is agent-visible in the a11y tree the same `draw` call
//! captures for every other example:
//! - a [`TabBar`] of sheet tabs — each one its own independent
//!   [`CellGridController`] with its own cursor, selection, scroll
//!   position and edits, so switching tabs is a real sheet switch, not
//!   a cosmetic label change;
//! - a [`TextField`] formula bar mirroring the active sheet's cursor
//!   cell, wired through [`CellGridController::sync_formula_bar`] and
//!   [`CellGridController::handle_formula_bar_key`] — clicking it opens
//!   the same in-progress edit the grid's own inline editor would, just
//!   with the caret rendered in the formula bar instead of over the
//!   cell;
//! - the [`CellGrid`] body itself;
//! - a [`StatusBar`] reporting the cursor/selection in spreadsheet
//!   notation (`B3`, `B3:D7`) plus — for a selection small enough to be
//!   worth the arithmetic — the sum of its numeric cells, so the
//!   semantic outcome of every key and click is readable on screen
//!   rather than inferred from highlight colours.
//!
//! The controller owns the cursor, the selection anchor, the scroll
//! offsets, the frozen panes and (while active) the in-progress edit's
//! text buffer; this `AppLogic` owns the cell *content* and nothing
//! else, which is the whole split `CellGrid` is built around (see
//! `quadraui::primitives::cell_grid`'s module doc, "the grid only
//! presents"). Each frame it asks the controller which absolute rows
//! and columns are on screen (`CellGridController::layout`), fills only
//! those cells, and paints the resulting descriptor through
//! `Surface::CellGrid` — `CellGrid` has no bespoke `Backend::draw_cell_grid`
//! method on any backend, pixel or terminal.
//!
//! Committing an edit hands the raw text to this demo's own toy "engine"
//! (`Self::evaluate`): a leading `=` sums `+`-separated integers (e.g.
//! `=1+2` → `3`); anything else is shown verbatim. A real spreadsheet
//! app wires the same `CellEditCommitted` text into its own engine
//! (`cell.set`) and supplies the resulting formatted value next frame.

use std::collections::HashMap;

use quadraui::primitives::cell_grid::{column_letters, Cell, CellAlign, CellStyle};
use quadraui::{
    AppLogic, Backend, CellGridController, CellGridControllerEvent, Color, Key, MouseButton,
    NamedKey, Reaction, Rect, ScreenLayout, SegmentMeasure, StatusBar, StatusBarSegment, Surface,
    TabBar, TabBarHit, TabItem, TabMeasure, TextField, UiEvent, WidgetId,
};

/// Rows in the virtual sheet — far more than any terminal or window
/// shows at once, so scrolling to a distant row is a real exercise of
/// the grid's virtualisation (`CellGrid::cells` is always just the
/// visible window, never the whole sheet).
pub const ROW_COUNT: usize = 100_000;
/// Columns in the virtual sheet, likewise wider than any viewport.
pub const COL_COUNT: usize = 200;
/// Leading rows that never scroll: the metric-name header row.
pub const FROZEN_ROWS: usize = 1;
/// Leading columns that never scroll: the region-name column.
pub const FROZEN_COLS: usize = 1;

/// Region names cycled down the frozen first column, so a scrolled row
/// is still identifiable by content in a test or on screen.
const REGIONS: [&str; 6] = ["North", "South", "East", "West", "Central", "Pacific"];

/// Sheet tabs, each its own independent [`CellGridController`].
const SHEET_NAMES: [&str; 3] = ["Sales", "Inventory", "Notes"];

/// A selection larger than this many cells skips the sum entirely — an
/// honest "too big to bother" rather than quietly stalling a redraw on,
/// say, a Ctrl+A over the full 20,000,000-cell sheet.
const SUM_CELL_CAP: usize = 10_000;

/// One sheet tab's state: its own navigation/selection/scroll/edit
/// state plus the raw text a user has committed for any of its cells.
struct Sheet {
    grid: CellGridController,
    edits: HashMap<(usize, usize), String>,
}

impl Sheet {
    fn new(id: &str) -> Self {
        let mut grid = CellGridController::new(id);
        grid.set_dimensions(ROW_COUNT, COL_COUNT);
        grid.set_frozen(FROZEN_ROWS, FROZEN_COLS);
        grid.set_show_scrollbar(true);
        Self {
            grid,
            edits: HashMap::new(),
        }
    }
}

/// The four stacked bands this demo paints into, top to bottom. A pure
/// function of the backend's viewport/line-height, called identically
/// by `render` (to paint) and `handle` (to hit-test) — unlike caching a
/// rect computed once in `render`, this can never let the two drift
/// apart, because there is only one computation to begin with.
struct Bands {
    tabs: Rect,
    formula: Rect,
    grid: Rect,
    status: Rect,
}

fn bands(backend: &dyn Backend) -> Bands {
    let vp = backend.viewport();
    let lh = backend.line_height();
    let tab_h = (lh * 1.5).round().min(vp.height);
    let formula_h = lh.min((vp.height - tab_h).max(0.0));
    let status_h = lh.min((vp.height - tab_h - formula_h).max(0.0));
    let grid_h = (vp.height - tab_h - formula_h - status_h).max(0.0);
    Bands {
        tabs: Rect::new(0.0, 0.0, vp.width, tab_h),
        formula: Rect::new(0.0, tab_h, vp.width, formula_h),
        grid: Rect::new(0.0, tab_h + formula_h, vp.width, grid_h),
        status: Rect::new(0.0, tab_h + formula_h + grid_h, vp.width, status_h),
    }
}

/// Quarterly-sales sheet driven entirely by [`CellGridController`]s.
pub struct CellGridDemo {
    sheets: Vec<Sheet>,
    active_sheet: usize,
    /// Whether the formula bar (rather than the grid itself) currently
    /// owns the in-progress edit's caret. Always `false` when no edit
    /// is active — set by a click on the formula bar, cleared the
    /// moment that edit commits or cancels (from *either* the formula
    /// bar or the grid's own inline editor; they share one edit).
    formula_focused: bool,
}

impl CellGridDemo {
    pub fn new() -> Self {
        Self {
            sheets: SHEET_NAMES
                .iter()
                .enumerate()
                .map(|(i, name)| Sheet::new(&format!("sheet-{i}-{name}")))
                .collect(),
            active_sheet: 0,
            formula_focused: false,
        }
    }

    fn sheet(&self) -> &Sheet {
        &self.sheets[self.active_sheet]
    }

    fn sheet_mut(&mut self) -> &mut Sheet {
        &mut self.sheets[self.active_sheet]
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
            _ => Cell::plain(format!("${}", Self::default_numeric(row, col) as i64)),
        }
    }

    /// [`Self::default_cell`]'s underlying number, before the `$`
    /// display formatting — what [`Self::cell_numeric_value`] sums,
    /// computed directly rather than by parsing the formatted text back
    /// out (the grid never parses display text; neither does its host).
    fn default_numeric(row: usize, col: usize) -> f64 {
        ((row * 100 + col * 7) * 3) as f64
    }

    /// The raw text [`Self::start_editing`] seeds F2/Enter editing with
    /// — the user's own committed text if there is one, else the
    /// placeholder's displayed text.
    fn raw_text(edits: &HashMap<(usize, usize), String>, row: usize, col: usize) -> String {
        edits
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
    fn formatted_cell(edits: &HashMap<(usize, usize), String>, row: usize, col: usize) -> Cell {
        match edits.get(&(row, col)) {
            Some(raw) => Cell::plain(Self::evaluate(raw)),
            None => Self::default_cell(row, col),
        }
    }

    /// This cell's numeric value for the status bar's sum, or `None`
    /// for a cell that isn't numeric at all: the frozen header row and
    /// region column are always text, and a user edit that didn't
    /// evaluate to a plain integer (e.g. typed prose) contributes
    /// nothing rather than erroring.
    fn cell_numeric_value(
        edits: &HashMap<(usize, usize), String>,
        row: usize,
        col: usize,
    ) -> Option<f64> {
        if row < FROZEN_ROWS || col < FROZEN_COLS {
            return None;
        }
        match edits.get(&(row, col)) {
            Some(raw) => Self::evaluate(raw).parse().ok(),
            None => Some(Self::default_numeric(row, col)),
        }
    }

    /// Cursor and selection in spreadsheet notation — `B3` for a single
    /// cell, `B3:D7` for a range.
    fn selection_label(&self) -> String {
        let (row, col) = self.sheet().grid.cursor();
        let a1 = |r: usize, c: usize| format!("{}{}", column_letters(c), r + 1);
        let (top, left, bottom, right) = self.sheet().grid.selection_bounds();
        if top == bottom && left == right {
            a1(row, col)
        } else {
            format!("{}:{}", a1(top, left), a1(bottom, right))
        }
    }

    /// `(cell count, sum)` for the current selection — `sum` is `None`
    /// when the selection has no numeric cells at all, or is bigger
    /// than [`SUM_CELL_CAP`] is worth summing every frame.
    fn selection_stats(&self) -> (usize, Option<f64>) {
        let (top, left, bottom, right) = self.sheet().grid.selection_bounds();
        let count = (bottom - top + 1) * (right - left + 1);
        if count > SUM_CELL_CAP {
            return (count, None);
        }
        let edits = &self.sheet().edits;
        let mut total = 0.0;
        let mut any = false;
        for row in top..=bottom {
            for col in left..=right {
                if let Some(v) = Self::cell_numeric_value(edits, row, col) {
                    total += v;
                    any = true;
                }
            }
        }
        (count, any.then_some(total))
    }

    fn tab_bar(&self) -> TabBar {
        let tabs = SHEET_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| TabItem {
                label: format!(" {name} "),
                is_active: i == self.active_sheet,
                is_dirty: false,
                is_preview: false,
                is_closable: false,
            })
            .collect();
        TabBar {
            id: WidgetId::new("cell-grid-tabs"),
            tabs,
            scroll_offset: 0,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: false,
            compact: true,
        }
    }

    fn tab_bar_hit(&self, backend: &dyn Backend, rect: Rect, x: f32, y: f32) -> Option<usize> {
        let cw = backend.char_width();
        let bar = self.tab_bar();
        let layout = bar.layout(
            rect.width,
            rect.height,
            cw * 2.0,
            |i| TabMeasure::new(bar.tabs[i].label.chars().count() as f32 * cw, 0.0),
            |_| SegmentMeasure::new(0.0),
        );
        match layout.hit_test(x - rect.x, y - rect.y) {
            TabBarHit::Tab(idx) => Some(idx),
            _ => None,
        }
    }

    fn formula_field(&self) -> TextField {
        let mut field = TextField::new(WidgetId::new("cell-grid-formula-bar"));
        field.placeholder = "Formula".into();
        let (row, col) = self.sheet().grid.cursor();
        field.value = Self::raw_text(&self.sheet().edits, row, col);
        self.sheet().grid.sync_formula_bar(&mut field);
        field.has_focus = self.formula_focused;
        field
    }

    fn status_bar(&self) -> StatusBar {
        let fg = Color::rgb(225, 225, 225);
        let bg = Color::rgb(35, 45, 65);
        let (n, sum) = self.selection_stats();
        let sum_suffix = sum.map(|s| format!(", sum {s:.0}")).unwrap_or_default();
        StatusBar {
            id: WidgetId::new("cell-grid-status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " Sel {} ({n} cell{}{sum_suffix}) ",
                    self.selection_label(),
                    if n == 1 { "" } else { "s" }
                ),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " F2/formula bar edit | Esc cancel | q quit ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }

    /// Apply one [`CellGridControllerEvent`] from either the grid's own
    /// `handle` or the formula bar's `handle_formula_bar_key` — both
    /// commit/cancel the exact same in-progress edit, so both end it
    /// here the same way.
    fn apply_controller_event(&mut self, event: CellGridControllerEvent) -> Reaction {
        match event {
            CellGridControllerEvent::Ignored => Reaction::Continue,
            // The controller has no cell content of its own (see the
            // module doc), so F2 only reports the request — seed the
            // editor with this cell's current raw text ourselves.
            CellGridControllerEvent::CellEditRequested { row, col } => {
                let text = Self::raw_text(&self.sheet().edits, row, col);
                self.sheet_mut().grid.start_editing(text);
                Reaction::Redraw
            }
            // Hand the committed raw text to the toy "engine"
            // (`Self::evaluate`) — a real app calls its own `cell.set`
            // here and supplies the resulting formatted value next
            // frame via `Self::formatted_cell`.
            CellGridControllerEvent::CellEditCommitted { row, col, text } => {
                self.sheet_mut().edits.insert((row, col), text);
                self.formula_focused = false;
                Reaction::Redraw
            }
            CellGridControllerEvent::CellEditCancelled { .. } => {
                self.formula_focused = false;
                Reaction::Redraw
            }
            _ => Reaction::Redraw,
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
        let bands = bands(backend);

        // Ask the controller which absolute rows/columns are on screen,
        // then fill exactly those — `CellGrid::cells` is a window, not
        // the whole sheet.
        let layout = self.sheet().grid.layout(backend, bands.grid).layout;
        let edits = &self.sheet().edits;
        let cells: Vec<Vec<Cell>> = layout
            .rows
            .iter()
            .map(|r| {
                layout
                    .cols
                    .iter()
                    .map(|c| Self::formatted_cell(edits, r.index, c.index))
                    .collect()
            })
            .collect();
        let grid = self.sheet().grid.grid(cells);

        let mut frame = ScreenLayout::new();
        frame.push(Surface::TabBar {
            rect: bands.tabs,
            bar: self.tab_bar(),
            hovered_close: None,
        });
        frame.push(Surface::TextField {
            rect: bands.formula,
            tf: self.formula_field(),
        });
        frame.push(Surface::CellGrid {
            rect: bands.grid,
            grid,
        });
        frame.push(Surface::StatusBar {
            rect: bands.status,
            bar: self.status_bar(),
            hovered: None,
            pressed: None,
        });
        let _ = frame.draw(backend);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let b = bands(backend);

        if let UiEvent::MouseDown {
            button: MouseButton::Left,
            position,
            ..
        } = &event
        {
            if b.tabs.contains(*position) {
                if let Some(idx) = self.tab_bar_hit(backend, b.tabs, position.x, position.y) {
                    if idx != self.active_sheet {
                        self.sheet_mut().grid.cancel_editing();
                        self.active_sheet = idx;
                        self.formula_focused = false;
                    }
                }
                return Reaction::Redraw;
            }
            if b.formula.contains(*position) {
                if !self.sheet().grid.is_editing() {
                    let (row, col) = self.sheet().grid.cursor();
                    let text = Self::raw_text(&self.sheet().edits, row, col);
                    self.sheet_mut().grid.start_editing(text);
                }
                self.formula_focused = true;
                return Reaction::Redraw;
            }
        }

        // `q`/Escape quit only outside an active edit — while editing,
        // both must reach the controller instead: `q` is a character to
        // insert, and Escape cancels the edit rather than the app.
        if !self.sheet().grid.is_editing() {
            if let UiEvent::KeyPressed { key, .. } = &event {
                if matches!(key, Key::Char('q') | Key::Named(NamedKey::Escape)) {
                    return Reaction::Exit;
                }
            }
        }

        if self.formula_focused {
            return match &event {
                UiEvent::KeyPressed { key, modifiers, .. } => {
                    let ev = self
                        .sheet_mut()
                        .grid
                        .handle_formula_bar_key(key, modifiers, backend, b.grid);
                    self.apply_controller_event(ev)
                }
                _ => Reaction::Continue,
            };
        }

        let ev = self.sheet_mut().grid.handle(&event, backend, b.grid);
        self.apply_controller_event(ev)
    }
}
