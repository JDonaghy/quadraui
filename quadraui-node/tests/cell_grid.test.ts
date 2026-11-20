// Black-box coverage for the `CellGridController` per-widget-id seam
// exposed through `quadraui-node`: a plain JS object decodes into a
// `"cell_grid"` extra (`EXTRA_PRIMITIVE_KINDS`'s generic registration
// already covers the descriptor shape -- no new napi method needed for
// that half), and the cursor/selection/inline-edit/virtualisation round
// trip -- click a cell -> `CursorMoved` -> page past the loaded window ->
// `CellsRequested` -> answer with a freshly loaded block -> type + Enter ->
// `CellEditCommitted` -- reaches the registered handler as
// `{"CellGridController": [id, event]}`, the same `{"TreeTableController":
// [id, event]}` array shape `tree_table_controller.test.ts` already
// exercises.
//
// Mirrors `quadraui-binding-core::engine`'s own
// `cell_grid_controller_cells_requested_does_not_repeat_after_host_loads_window`/
// `cell_grid_controller_typed_edit_commits_on_enter` Rust tests (and
// `quadraui-py/tests/test_cell_grid.py`), driven end-to-end through this
// binding's JS surface instead.
//
// `CellsRequested` is answered the same way `RowsRequested` is elsewhere:
// there is no separate `set_*` host call -- the engine reconciles a
// `"cell_grid"` extra's `cells`/`loaded_row_start`/`loaded_col_start` fresh
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
// A bare character key with no active edit starts one
// (`CellGridController::handle_key`'s `Key::Char(c) if !jump =>
// self.begin_typing(*c)` arm) -- the synthesized `CharTyped` event only
// ever reaches `CellGridController::handle` once an edit is already under
// way (that type's own doc), so this is `KeyPressed`, not `CharTyped`.
const TYPE_X = { KeyPressed: { key: { Char: "x" }, modifiers: NO_MODS, repeat: false } };
const CLICK_FIRST_CELL = {
  MouseDown: { button: "Left", position: { x: 9.0, y: 1.5 }, modifiers: {} },
};
// Far enough from `CLICK_FIRST_CELL` (> 1.5 cells) that `TuiBackend`'s
// `DoubleClickDetector` (400ms/1.5-cell window) doesn't fold the two
// consecutive `MouseDown`s below into a `DoubleClick`, which
// `CellGridController::handle` has no arm for.
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

// A 20x5 sheet with only `state.cells`' own row count loaded, starting at
// `state.loadedRowStart` -- deliberately fewer rows than the 10-cell-tall
// viewport `render` below paints into, so a `CellsRequested` is always one
// click/page away. Mirrors `quadraui-binding-core::engine`'s own
// `small_window_cell_grid_descriptor`.
function descriptor(state: { cells: unknown; loadedRowStart: number }) {
  return {
    id: "cg",
    row_count: ROW_COUNT,
    col_count: COL_COUNT,
    cells: state.cells,
    loaded_row_start: state.loadedRowStart,
    loaded_col_start: 0,
    has_focus: true,
    show_scrollbar: false,
  };
}

function render(state: { cells: unknown; loadedRowStart: number }) {
  // A single-leaf `"row"` layout tree -- fills the whole viewport, the same
  // shape `tree_table_controller.test.ts`'s own `render` uses and for the
  // same reason (the flat six-slot-plus-extras system's fixed 3-line band
  // is far too short for a sheet).
  return { row: [{ id: "cg", cell_grid: descriptor(state) }] };
}

function cellGridEvents(received: any[], id: string) {
  return received
    .filter((entry) => "CellGridController" in entry && entry.CellGridController[0] === id)
    .map((entry) => entry.CellGridController[1]);
}

test("cell grid controller click, scroll request, and edit round trip", () => {
  const state = { cells: cells(0, 0, LOADED_ROWS, COL_COUNT), loadedRowStart: 0 };
  const app = new QuiApp(20, 10);
  app.setRender(() => render(state));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: any) => {
    received.push(event);
    if (!("CellGridController" in event)) {
      return undefined;
    }
    const [, payload] = event.CellGridController;
    const requested = payload.CellsRequested;
    if (requested === undefined) {
      return undefined;
    }
    // Exactly what a real app does with a `CellsRequested`: load that
    // absolute block and repaint -- there is no `set_*` call for this,
    // unlike `setTreeTableChildren`.
    state.cells = cells(requested.row_start, requested.col_start, requested.rows, requested.cols);
    state.loadedRowStart = requested.row_start;
    return true;
  });

  // A click on the first body cell both click-focuses "cg" and reports a
  // `CursorMoved` to (0, 0).
  app.pushEvent(CLICK_FIRST_CELL);
  app.run(1);
  assert.ok(
    cellGridEvents(received, "cg").some(
      (ev) => ev.CursorMoved && ev.CursorMoved.row === 0 && ev.CursorMoved.col === 0,
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
  assert.ok(afterClick.some((ev) => ev.CursorMoved), JSON.stringify(afterClick));

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
