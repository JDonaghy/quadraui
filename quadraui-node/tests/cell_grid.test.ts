// Black-box coverage for the `cell_grid` `WIDGET_KINDS` seam exposed
// through `quadraui-node`: a plain JS object decodes straight into
// `quadraui::CellGrid` (`WIDGET_KINDS`'s generic registration already
// covers the descriptor shape -- no new napi method needed for that
// half), and the cursor/selection/inline-edit/virtualisation round trip
// -- click a cell -> `Navigated` -> page past the loaded window ->
// `CellsRequested` -> answer with a freshly loaded block -> type + Enter
// -> `CellEditCommitted` -- reaches the registered handler as
// `{"CellGrid": [id, event]}`, the same `{"TreeTableController":
// [id, event]}` array shape `tree_table_controller.test.ts` already
// exercises.
//
// Unlike the old per-id-cached-controller design, `CellGrid` carries no
// state of its own across frames: `cursor`/`selection_anchor`/
// `row_scroll`/`col_scroll`/`editing` all come from whatever `descriptor`
// returns *this* frame, so this test's own `state` object has to apply
// every `Navigated`/`CellEditStarted`/`CellEditChanged`/
// `CellEditCommitted`/`CellEditCancelled` event back onto itself before
// the next render, the same way a real app's host state must. Mirrors
// `quadraui-binding-core::engine`'s own `CellGridLogic::apply` Rust test
// harness (and `quadraui-py/tests/test_cell_grid.py`), driven end-to-end
// through this binding's JS surface instead.
//
// `CellsRequested` is answered the same way `RowsRequested` is elsewhere:
// there is no separate `set_*` host call -- the engine reconciles a
// `"cell_grid"` leaf's `cells`/`loaded_row_start`/`loaded_col_start` fresh
// every frame, so mutating the render callback's own state and returning
// `true` (redraw) from the handler is enough.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

const NO_MODS = { shift: false, ctrl: false, alt: false, cmd: false };
const PAGE_DOWN = { KeyPressed: { key: { Named: "PageDown" }, modifiers: NO_MODS, repeat: false } };
const ENTER = { KeyPressed: { key: { Named: "Enter" }, modifiers: NO_MODS, repeat: false } };
// A bare character key with no active edit starts one (`CellGrid::handle`'s
// own `RoutedEvent::Key` arm for a plain char) -- the synthesized
// `CharTyped` event only ever reaches `CellGrid::handle` once an edit is
// already under way (that type's own doc), so this is `KeyPressed`, not
// `CharTyped`.
const TYPE_X = { KeyPressed: { key: { Char: "x" }, modifiers: NO_MODS, repeat: false } };
const CLICK_FIRST_CELL = {
  MouseDown: { button: "Left", position: { x: 9.0, y: 1.5 }, modifiers: {} },
};
// Far enough from `CLICK_FIRST_CELL` (> 1.5 cells) that `TuiBackend`'s
// `DoubleClickDetector` (400ms/1.5-cell window) doesn't fold the two
// consecutive `MouseDown`s below into a `DoubleClick`, which
// `CellGrid::handle` has no arm for.
const CLICK_LAST_VISIBLE_ROW = {
  MouseDown: { button: "Left", position: { x: 9.0, y: 8.5 }, modifiers: {} },
};

const ROW_COUNT = 20;
const COL_COUNT = 5;
const LOADED_ROWS = 2;

function cell(text: string) {
  return { text, style: {} };
}

function cells(rowStart: number, colStart: number, rows: number, cols: number) {
  const out: unknown[][] = [];
  for (let r = 0; r < rows; r++) {
    const line: unknown[] = [];
    for (let c = 0; c < cols; c++) {
      line.push(cell(`r${rowStart + r}c${colStart + c}`));
    }
    out.push(line);
  }
  return out;
}

type CellGridState = {
  cells: unknown;
  loadedRowStart: number;
  loadedColStart: number;
  cursor: [number, number];
  selectionAnchor: [number, number] | null;
  rowScroll: number;
  colScroll: number;
  editing: unknown;
};

// A 20x5 sheet with only `LOADED_ROWS` rows loaded, starting at row 0 --
// deliberately fewer rows than the 10-cell-tall viewport `render` below
// paints into, so a `CellsRequested` is always one click/page away.
// Mirrors `quadraui-binding-core::engine`'s own `small_window_cell_grid`.
function initialState(): CellGridState {
  return {
    cells: cells(0, 0, LOADED_ROWS, COL_COUNT),
    loadedRowStart: 0,
    loadedColStart: 0,
    cursor: [0, 0],
    selectionAnchor: null,
    rowScroll: 0,
    colScroll: 0,
    editing: null,
  };
}

function descriptor(state: CellGridState) {
  return {
    id: "cg",
    row_count: ROW_COUNT,
    col_count: COL_COUNT,
    default_row_height: 1.0,
    default_col_width: 8.0,
    cells: state.cells,
    loaded_row_start: state.loadedRowStart,
    loaded_col_start: state.loadedColStart,
    cursor: state.cursor,
    selection_anchor: state.selectionAnchor,
    row_scroll: state.rowScroll,
    col_scroll: state.colScroll,
    editing: state.editing,
    has_focus: true,
    show_scrollbar: false,
  };
}

function render(state: CellGridState) {
  // A single-leaf `"row"` layout tree -- fills the whole viewport, the same
  // shape `tree_table_controller.test.ts`'s own render uses and for the
  // same reason (the flat six-slot-plus-extras system's fixed 3-line band
  // is far too short for a sheet).
  return { row: [{ id: "cg", cell_grid: descriptor(state) }] };
}

function cellGridEvents(received: any[], id: string) {
  return received
    .filter((entry) => "CellGrid" in entry && entry.CellGrid[0] === id)
    .map((entry) => entry.CellGrid[1]);
}

// Writes one `CellGridEvent` back onto `state` the way a real host's own
// cursor/selection/scroll/editing fields must track it -- `CellGrid` itself
// is stateless across frames, so skipping this leaves the next render
// starting over at `(0, 0)` with no active edit, matching
// `CellGridLogic::apply`'s own Rust contract.
function apply(state: CellGridState, event: any) {
  if (event.Navigated) {
    state.cursor = event.Navigated.cursor;
    state.selectionAnchor = event.Navigated.selection_anchor;
    state.rowScroll = event.Navigated.row_scroll;
    state.colScroll = event.Navigated.col_scroll;
  }
  const started = event.CellEditStarted ?? event.CellEditChanged;
  if (started) {
    state.editing = started.edit;
  }
  if (event.CellEditCommitted || event.CellEditCancelled) {
    state.editing = null;
  }
}

test("cell grid click, scroll request, and edit round trip", () => {
  const state = initialState();
  const app = new QuiApp(20, 10);
  app.setRender(() => render(state));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: any) => {
    received.push(event);
    if (!("CellGrid" in event)) {
      return undefined;
    }
    const [, payload] = event.CellGrid;
    apply(state, payload);
    const requested = payload.CellsRequested;
    if (requested === undefined) {
      return true;
    }
    // Exactly what a real app does with a `CellsRequested`: load that
    // absolute block and repaint -- there is no `set_*` call for this.
    state.cells = cells(requested.row_start, requested.col_start, requested.rows, requested.cols);
    state.loadedRowStart = requested.row_start;
    state.loadedColStart = requested.col_start;
    return true;
  });

  // A click on the first body cell both click-focuses "cg" and reports a
  // `Navigated` moving the cursor to (0, 0) -- `CellGrid::handle` reports
  // every click/key navigation as `Navigated`, never `CursorMoved` (that
  // variant is a host-derived event this leaf never actually emits, see
  // `CellGridEvent`'s own Rust doc).
  app.pushEvent(CLICK_FIRST_CELL);
  app.run(1);
  assert.ok(
    cellGridEvents(received, "cg").some(
      (ev) => ev.Navigated && ev.Navigated.cursor[0] === 0 && ev.Navigated.cursor[1] === 0,
    ),
    JSON.stringify(received),
  );

  // Paging down past the 2 loaded rows scrolls the cursor beyond what's
  // loaded, requesting (and -- via the handler above -- immediately
  // getting answered with) a fresh block.
  received.length = 0;
  app.pushEvent(PAGE_DOWN);
  app.run(1);
  const cellsRequested = cellGridEvents(received, "cg")
    .filter((ev) => ev.CellsRequested)
    .map((ev) => ev.CellsRequested);
  assert.equal(cellsRequested.length, 1, JSON.stringify(received));
  assert.ok(cellsRequested[0].row_start > 0, JSON.stringify(cellsRequested));

  // The engine never re-asks for a block the host just loaded: a second
  // click onto an already-loaded cell reports only a cursor move.
  received.length = 0;
  app.pushEvent(CLICK_LAST_VISIBLE_ROW);
  app.run(1);
  const afterClick = cellGridEvents(received, "cg");
  assert.ok(!afterClick.some((ev) => ev.CellsRequested), JSON.stringify(afterClick));
  assert.ok(afterClick.some((ev) => ev.Navigated), JSON.stringify(afterClick));

  // Typing "x" starts an inline edit at the cursor; Enter commits it.
  received.length = 0;
  app.pushEvent(TYPE_X);
  app.pushEvent(ENTER);
  app.run(2);
  const committed = cellGridEvents(received, "cg")
    .filter((ev) => ev.CellEditCommitted)
    .map((ev) => ev.CellEditCommitted);
  assert.equal(committed.length, 1, JSON.stringify(received));
  assert.equal(committed[0].text, "x", JSON.stringify(committed));
});
