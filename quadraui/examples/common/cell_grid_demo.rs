//! `CellGrid` demo: a virtual quarterly-sales sheet navigated the way a
//! spreadsheet user expects — arrows, Tab/Enter, Page Up/Down, Home/End,
//! Ctrl+Home/End, Ctrl+Arrow, Shift to extend, click and click-drag
//! range selection, row/column-header and corner clicks, plus in-cell
//! editing (typing a character or F2 starts an edit at the cursor cell,
//! Esc cancels, Enter/Tab/the arrow keys commit and move).
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
//!   [`CellGrid`] with its own cursor, selection, scroll position and
//!   edits, so switching tabs is a real sheet switch, not a cosmetic
//!   label change;
//! - a [`TextField`] formula bar mirroring the active sheet's cursor
//!   cell and, while an edit is in progress, the same [`CellEditState`]
//!   the grid's own inline editor is drawing from — clicking it opens
//!   the same edit the grid's F2/typing would, just with the caret
//!   rendered in the formula bar instead of over the cell;
//! - the [`CellGrid`] body itself;
//! - a [`StatusBar`] reporting the cursor/selection in spreadsheet
//!   notation (`B3`, `B3:D7`) plus — for a selection small enough to be
//!   worth the arithmetic — the sum of its numeric cells, so the
//!   semantic outcome of every key and click is readable on screen
//!   rather than inferred from highlight colours.
//!
//! This app drives [`CellGrid`] directly through its own
//! [`Interactive`] impl rather than registering it with
//! [`quadraui::AppLogic::frame_tree`]: like `data_table_app.rs`'s own
//! hand-rolled hit testing, every event this demo ever hands to
//! [`CellGrid::handle`] has already been resolved to belong to it by
//! [`Self::handle`]'s own band checks (tabs, then the formula bar, then
//! everything else), so there is no hit-test/focus-cycling router for
//! [`DemoHost`] to stand in for — only [`EventHost::emit`] carries
//! anything real, the extra [`CellGridEvent::Navigated`] a commit's
//! `cx.emit` call attaches alongside its own `CellEditCommitted`.
//!
//! `CellGrid` owns the cursor, the selection anchor, the scroll offsets,
//! the frozen panes and (while active) the in-progress edit's text
//! buffer; this `AppLogic` owns the cell *content* and nothing else,
//! which is the whole split `CellGrid` is built around (see
//! `quadraui::primitives::cell_grid`'s module doc, "the grid only
//! presents"). Each frame it resolves `CellGrid`'s own
//! [`crate::primitive::Primitive::layout`] to learn which absolute rows
//! and columns are on screen, fills only those cells, and paints the
//! resulting descriptor through `Surface::Widget` — `CellGrid` has no
//! bespoke `Backend::draw_cell_grid` method on any backend, pixel or
//! terminal.
//!
//! Committing an edit hands the raw text to this demo's own toy "engine"
//! (`Self::evaluate`): a leading `=` sums `+`-separated integers (e.g.
//! `=1+2` → `3`); anything else is shown verbatim. A real spreadsheet
//! app wires the same `CellEditCommitted` text into its own engine
//! (`cell.set`) and supplies the resulting formatted value next frame.

use std::collections::HashMap;

use quadraui::interactive::SubTarget;
use quadraui::primitive::{LayoutCtx, Primitive, Units};
use quadraui::primitives::cell_grid::{column_letters, Cell, CellAlign, CellEditState, CellStyle};
use quadraui::{
    AppLogic, Backend, ButtonMask, CellGrid, CellGridEvent, CellGridPrimitiveLayout, CellGridState,
    Color, EventCx, EventHost, Handled, Interactive, Key, KeyEvent, Modifiers, MouseButton,
    NamedKey, Point, PointerKind, PointerShape, Reaction, Rect, RoutedEvent, ScreenLayout,
    SegmentMeasure, StatusBar, StatusBarSegment, Surface, TabBar, TabBarHit, TabItem, TabMeasure,
    TextField, UiEvent, WidgetId,
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

/// Sheet tabs, each its own independent [`CellGrid`].
const SHEET_NAMES: [&str; 3] = ["Sales", "Inventory", "Notes"];

/// A selection larger than this many cells skips the sum entirely — an
/// honest "too big to bother" rather than quietly stalling a redraw on,
/// say, a Ctrl+A over the full 20,000,000-cell sheet.
const SUM_CELL_CAP: usize = 10_000;

/// One sheet tab's state: the grid descriptor plus its
/// [`Interactive::State`] (the in-progress drag anchor) and the raw text
/// a user has committed for any of its cells.
struct Sheet {
    grid: CellGrid,
    drag: CellGridState,
    edits: HashMap<(usize, usize), String>,
}

impl Sheet {
    fn new(id: &str) -> Self {
        let mut grid = CellGrid::new(WidgetId::new(id));
        grid.row_count = ROW_COUNT;
        grid.col_count = COL_COUNT;
        grid.frozen_rows = FROZEN_ROWS;
        grid.frozen_cols = FROZEN_COLS;
        grid.show_scrollbar = true;
        grid.has_focus = true;
        // Exercise both opt-in visual aids in this demo's own sheet:
        // TUI column separators and all-backend zebra striping.
        grid.column_separators = true;
        grid.striped = true;
        Self {
            grid,
            drag: CellGridState::default(),
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

/// [`LayoutCtx`] resolved from `backend` — the same chrome-metric
/// resolution [`Backend::draw_primitive`] applies before calling
/// [`Primitive::layout`], built by hand here since this demo resolves
/// `CellGrid`'s layout once per frame to window its cells rather than
/// through `draw_primitive` itself (which would also paint).
fn layout_ctx(backend: &dyn Backend) -> LayoutCtx {
    LayoutCtx {
        line_height: LayoutCtx::resolve_row_height(backend.chrome_line_height(), &backend.style()),
        char_width: backend.char_width(),
        nerd_fonts_enabled: backend.nerd_fonts_enabled(),
    }
}

/// `Primitive::layout` resolved for `grid` at `rect` — called through
/// the trait explicitly (not `grid.layout(...)`) since `CellGrid` also
/// has its own unrelated six-argument inherent `layout` method (see that
/// method's own doc) that dot-call syntax would otherwise prefer.
fn grid_layout(grid: &CellGrid, backend: &dyn Backend, rect: Rect) -> CellGridPrimitiveLayout {
    Primitive::layout(grid, &layout_ctx(backend), rect)
}

/// [`EventHost`] impl backing this demo's own hand-rolled dispatch into
/// `CellGrid::handle` — see the module doc for why `request_focus`/
/// `capture_pointer`/`release_pointer`/`set_cursor`/`request_redraw` are
/// no-ops here: there is no hit-test router for them to report back to,
/// only [`Self::emit`] (an extra [`UiEvent`] a commit attaches) matters.
struct DemoHost {
    units: Units,
    emitted: Vec<UiEvent>,
}

impl DemoHost {
    fn new(units: Units) -> Self {
        Self {
            units,
            emitted: Vec::new(),
        }
    }
}

impl EventHost for DemoHost {
    fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
        false
    }

    fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}

    fn capture_pointer(&mut self, _id: &WidgetId) {}

    fn release_pointer(&mut self) {}

    fn set_cursor(&mut self, _cursor: PointerShape) {}

    fn request_redraw(&mut self) {}

    fn emit(&mut self, event: UiEvent) {
        self.emitted.push(event);
    }

    fn units(&self) -> Units {
        self.units
    }
}

/// Quarterly-sales sheet driven entirely by [`CellGrid`]'s own
/// [`Interactive`] impl.
pub struct CellGridDemo {
    sheets: Vec<Sheet>,
    active_sheet: usize,
    /// Whether the formula bar (rather than the grid itself) currently
    /// owns the in-progress edit's caret. Always `false` when no edit
    /// is active — set by a click on the formula bar, cleared the
    /// moment that edit commits or cancels (from *either* the formula
    /// bar or the grid's own inline editor; they share one edit, held on
    /// [`CellGrid::editing`]).
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

    /// The raw text a fresh edit seeds itself with — the user's own
    /// committed text if there is one, else the placeholder's displayed
    /// text.
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
        let (row, col) = self.sheet().grid.cursor;
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
        let (row, col) = self.sheet().grid.cursor;
        field.value = Self::raw_text(&self.sheet().edits, row, col);
        match &self.sheet().grid.editing {
            Some(edit) => {
                field.value = edit.text.clone();
                field.cursor = Some(edit.cursor);
                field.selection_anchor = edit.selection_anchor;
            }
            None => {
                field.cursor = None;
                field.selection_anchor = None;
            }
        }
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

    /// Seed [`CellGrid::editing`] with `text`, cursor at its end — F2,
    /// plain typing and a formula-bar click all start an edit this way.
    fn start_editing(&mut self, text: String) {
        let cursor = text.len();
        self.sheet_mut().grid.editing = Some(CellEditState {
            text,
            cursor,
            selection_anchor: None,
            placeholder: None,
        });
    }

    /// Route one [`RoutedEvent`] into the active sheet's own
    /// [`CellGrid::handle`], applying its [`Handled::Event`] outcome (if
    /// any) and every extra event [`DemoHost::emit`] collected — see the
    /// module doc for why this demo's own [`EventHost`] impl has nothing
    /// else to do.
    fn dispatch_to_grid(
        &mut self,
        routed: RoutedEvent,
        layout: &CellGridPrimitiveLayout,
        units: Units,
    ) -> Reaction {
        let mut host = DemoHost::new(units);
        let handled = {
            let sheet = self.sheet_mut();
            let mut cx = EventCx::new(sheet.grid.id, &mut host);
            sheet.grid.handle(&mut sheet.drag, &routed, layout, &mut cx)
        };
        let mut reaction = match handled {
            Handled::Event(event) => self.apply_cell_grid_event(event),
            Handled::Consumed => Reaction::Redraw,
            Handled::Ignored => Reaction::Continue,
        };
        let grid_id = self.sheet().grid.id;
        for extra in host.emitted {
            if let UiEvent::CellGrid(id, event) = extra {
                if id == grid_id {
                    reaction = reaction.merge(self.apply_cell_grid_event(event));
                }
            }
        }
        reaction
    }

    /// Apply one [`CellGridEvent`] from [`Self::dispatch_to_grid`] — both
    /// the grid's own `handle` and a formula-bar key routed through the
    /// exact same `handle` call (see [`Self::handle`]) commit/cancel the
    /// identical in-progress edit, so both end it here the same way.
    fn apply_cell_grid_event(&mut self, event: CellGridEvent) -> Reaction {
        match event {
            CellGridEvent::Navigated {
                cursor,
                selection_anchor,
                row_scroll,
                col_scroll,
            } => {
                let grid = &mut self.sheet_mut().grid;
                grid.cursor = cursor;
                grid.selection_anchor = selection_anchor;
                grid.row_scroll = row_scroll;
                grid.col_scroll = col_scroll;
                Reaction::Redraw
            }
            // `CellGrid` has no cell content of its own (see the module
            // doc), so F2 only reports the request — seed the editor
            // with this cell's current raw text ourselves.
            CellGridEvent::CellEditRequested { row, col } => {
                let text = Self::raw_text(&self.sheet().edits, row, col);
                self.start_editing(text);
                Reaction::Redraw
            }
            CellGridEvent::CellEditStarted { edit, .. }
            | CellGridEvent::CellEditChanged { edit, .. } => {
                self.sheet_mut().grid.editing = Some(edit);
                Reaction::Redraw
            }
            // Hand the committed raw text to the toy "engine"
            // (`Self::evaluate`) — a real app calls its own `cell.set`
            // here and supplies the resulting formatted value next
            // frame via `Self::formatted_cell`.
            CellGridEvent::CellEditCommitted { row, col, text } => {
                self.sheet_mut().edits.insert((row, col), text);
                self.sheet_mut().grid.editing = None;
                self.formula_focused = false;
                Reaction::Redraw
            }
            CellGridEvent::CellEditCancelled { .. } => {
                self.sheet_mut().grid.editing = None;
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

        // Resolve the grid's own layout to learn which absolute
        // rows/columns are on screen, then fill exactly those —
        // `CellGrid::cells` is a window, not the whole sheet.
        let layout = grid_layout(&self.sheet().grid, backend, bands.grid);
        let edits = &self.sheet().edits;
        let cells: Vec<Vec<Cell>> = layout
            .layout
            .rows
            .iter()
            .map(|r| {
                layout
                    .layout
                    .cols
                    .iter()
                    .map(|c| Self::formatted_cell(edits, r.index, c.index))
                    .collect()
            })
            .collect();
        let mut grid = self.sheet().grid.clone();
        // `row_scroll`/`col_scroll` are ranks *past* the frozen prefix,
        // but `loaded_row_start`/`loaded_col_start` are absolute sheet
        // indices (see `CellGrid::cells_window`'s doc) — add the frozen
        // count back so the two conventions agree.
        grid.loaded_row_start = grid.frozen_rows.min(grid.row_count) + grid.row_scroll;
        grid.loaded_col_start = grid.frozen_cols.min(grid.col_count) + grid.col_scroll;
        grid.cells = cells;

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
        frame.push(Surface::Widget {
            rect: bands.grid,
            widget: Box::new(grid),
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
                        self.sheet_mut().grid.editing = None;
                        self.active_sheet = idx;
                        self.formula_focused = false;
                    }
                }
                return Reaction::Redraw;
            }
            if b.formula.contains(*position) {
                if self.sheet().grid.editing.is_none() {
                    let (row, col) = self.sheet().grid.cursor;
                    let text = Self::raw_text(&self.sheet().edits, row, col);
                    self.start_editing(text);
                }
                self.formula_focused = true;
                return Reaction::Redraw;
            }
        }

        // `q`/Escape quit only outside an active edit — while editing,
        // both must reach the grid instead: `q` is a character to
        // insert, and Escape cancels the edit rather than the app.
        if self.sheet().grid.editing.is_none() {
            if let UiEvent::KeyPressed { key, .. } = &event {
                if matches!(key, Key::Char('q') | Key::Named(NamedKey::Escape)) {
                    return Reaction::Exit;
                }
            }
        }

        let units = layout_ctx(backend).units();
        let layout = grid_layout(&self.sheet().grid, backend, b.grid);

        // A key reaches the grid regardless of `self.formula_focused`:
        // `CellGrid::handle` itself branches on whether
        // `CellGrid::editing` is active, the one state the formula bar
        // and the grid's own inline editor share, so routing every key
        // here keeps committing/cancelling/typing identical no matter
        // which of the two painted the caret.
        let routed = match &event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                modifiers,
                ..
            } if b.grid.contains(*position) => Some(RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(position.x - b.grid.x, position.y - b.grid.y),
                mods: *modifiers,
            }),
            UiEvent::MouseMoved {
                position,
                buttons: ButtonMask { left: true, .. },
            } => Some(RoutedEvent::Pointer {
                kind: PointerKind::Move,
                sub: None,
                local: Point::new(position.x - b.grid.x, position.y - b.grid.y),
                mods: Modifiers::default(),
            }),
            UiEvent::MouseUp {
                button: MouseButton::Left,
                position,
                ..
            } => Some(RoutedEvent::Pointer {
                kind: PointerKind::Up(MouseButton::Left),
                sub: None,
                local: Point::new(position.x - b.grid.x, position.y - b.grid.y),
                mods: Modifiers::default(),
            }),
            UiEvent::KeyPressed {
                key,
                modifiers,
                repeat,
            } => Some(RoutedEvent::Key(KeyEvent {
                key: key.clone(),
                modifiers: *modifiers,
                repeat: *repeat,
            })),
            _ => None,
        };

        match routed {
            Some(routed) => self.dispatch_to_grid(routed, &layout, units),
            None => Reaction::Continue,
        }
    }
}
