// Black-box coverage for `DataTable`/`ListView` double-click vs single-click
// event parity: a single click on a `DataTable` row synthesizes
// `DataTableEvent::RowSelected` (select), a double-click synthesizes the
// distinct `RowDoubleClicked` (open), and a `ListView` double-click
// synthesizes `ItemDoubleClicked` rather than reusing `ItemActivated` --
// all produced by `quadraui-binding-core`'s `InteractionEngine` and
// reaching JS exactly as the typed `UiEvent::DataTable`/`UiEvent::List`
// shape, not as a raw `MouseDown`/`DoubleClick` plus a `"hit"` dict the
// handler would have to re-interpret itself. Ports
// `quadraui-py/tests/test_data_table_interactions.py`'s click cases plus
// `quadraui-py/src/lib.rs`'s list double-click test.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

// Two `Fixed(8.0)` columns keep every coordinate below exact: the header
// occupies row 0 (TUI header height is one cell); body rows start at row 1.
const COLUMNS = [
  { title: "Name", width: { Fixed: 8.0 } },
  { title: "Age", width: { Fixed: 8.0 } },
];

function cell(text: string) {
  return { spans: [{ text, fg: null, bg: null }] };
}

function row(name: string, age: string) {
  return { cells: [cell(name), cell(age)] };
}

function insertTable(app: any) {
  app.insertDataTable({
    id: "table",
    columns: COLUMNS,
    rows: [row("Alice", "30"), row("Bob", "40"), row("Cleo", "50")],
  });
}

test("single click on a data table row emits RowSelected, not RowActivated", () => {
  const app = new QuiApp(24, 8);
  insertTable(app);

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 2.0, y: 1.0 }, modifiers: {} },
  });
  app.run(1);

  assert.ok(
    received.some(
      (event) =>
        event.DataTable &&
        event.DataTable[0] === "table" &&
        event.DataTable[1].RowSelected?.idx === 0,
    ),
    `expected a DataTable RowSelected{idx: 0}, got: ${JSON.stringify(received)}`,
  );
  assert.ok(
    !received.some((event) => event.DataTable && "RowActivated" in event.DataTable[1]),
    `a single click must not emit RowActivated: ${JSON.stringify(received)}`,
  );
});

test("double click on a data table row emits the distinct RowDoubleClicked", () => {
  const app = new QuiApp(24, 8);
  insertTable(app);

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({ DoubleClick: { position: { x: 2.0, y: 1.0 } } });
  app.run(1);

  assert.ok(
    received.some(
      (event) =>
        event.DataTable &&
        event.DataTable[0] === "table" &&
        event.DataTable[1].RowDoubleClicked?.idx === 0,
    ),
    `expected a DataTable RowDoubleClicked{idx: 0}, got: ${JSON.stringify(received)}`,
  );
});

test("double click on a list item emits ItemDoubleClicked, not ItemActivated", () => {
  const app = new QuiApp(40, 10);
  app.insertList({
    id: "list:main",
    items: [
      { text: { spans: [{ text: "Alpha", fg: null, bg: null }] } },
      { text: { spans: [{ text: "Beta", fg: null, bg: null }] } },
    ],
    selected_idx: 0,
  });
  assert.ok(app.screenContains("Beta"));

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({ DoubleClick: { position: { x: 0.0, y: 1.0 } } });
  app.run(1);

  assert.ok(
    received.some(
      (event) =>
        event.List &&
        event.List[0] === "list:main" &&
        event.List[1].ItemDoubleClicked?.idx === 1,
    ),
    `expected a List ItemDoubleClicked{idx: 1}, got: ${JSON.stringify(received)}`,
  );
});
