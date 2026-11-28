// Black-box coverage for row presses and multi-select through the binding
// engine, the Node twin of `quadraui-py/tests/test_row_presses.py`: a
// right-click on a `DataTable` row asks for its context menu (two quick
// right-clicks ask twice, never a double-click), a middle-click reports its
// button, a `"Multi"` table grows a selection from Ctrl/Shift clicks and the
// keyboard chords, and two quick clicks on a `Button` both reach `onClick`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("@quadraui/node");
const { App, Button } = require("@quadraui/node/app");
const { harness } = require("@quadraui/node/testing");

const NO_MODS = { shift: false, ctrl: false, alt: false, cmd: false };
const SHIFT = { ...NO_MODS, shift: true };
const CTRL = { ...NO_MODS, ctrl: true };

function row(name: string) {
  return { cells: [{ spans: [{ text: name, fg: null, bg: null }] }] };
}

// A one-column table whose rows sit at y == 1 + idx (the header is row 0;
// a lone table slot is four cells tall, so rows 0-2 are on screen), echoing `RowSelected`/`SelectionSet` back into its descriptor.
function tableApp(mode: "Single" | "Multi" = "Single") {
  const app = new QuiApp(24, 10);
  const state: { selected: number | null; set: number[] } = { selected: null, set: [] };
  app.setRender(() => ({
    data_table: {
      id: "table",
      columns: [{ title: "Name", width: { Native: 8.0 } }],
      rows: [0, 1, 2, 3, 4].map((i) => row(`row${i}`)),
      selected_idx: state.selected,
      selection_mode: mode,
      selected_indices: state.set,
    },
  }));
  app.screen();
  const received: any[] = [];
  app.setHandler((event: any) => {
    received.push(event);
    const payload = event.DataTable?.[1] ?? {};
    if (payload.RowSelected) {
      state.selected = payload.RowSelected.idx;
      state.set = [payload.RowSelected.idx];
    }
    if (payload.SelectionSet) state.set = payload.SelectionSet.indices;
  });
  return { app, received };
}

// Consecutive presses less than 1.5 cells apart fold into a double-click,
// so the multi-select test alternates `x`.
function press(app: any, y: number, button = "Left", modifiers = NO_MODS, x = 2.0) {
  app.pushEvent({ MouseDown: { button, position: { x, y }, modifiers } });
  app.pushEvent({ MouseUp: { button, position: { x, y } } });
  app.run(2);
}

function key(app: any, k: unknown, modifiers = NO_MODS) {
  app.pushEvent({ KeyPressed: { key: k, modifiers, repeat: false } });
  app.run(1);
}

function payloads(received: any[]) {
  return received.filter((e) => e.DataTable).map((e) => e.DataTable[1]);
}

test("two quick right-clicks on a row are two context-menu requests", () => {
  const { app, received } = tableApp();
  press(app, 3, "Right");
  press(app, 3, "Right");
  const menus = payloads(received).filter((p) => p.ContextMenuRequested);
  assert.deepEqual(
    menus.map((p) => p.ContextMenuRequested),
    [
      { idx: 2, position: { x: 2.0, y: 3.0 } },
      { idx: 2, position: { x: 2.0, y: 3.0 } },
    ],
  );
  assert.ok(!payloads(received).some((p) => p.RowDoubleClicked || p.RowSelected), JSON.stringify(received));
});

test("a middle-click on a row reports its button", () => {
  const { app, received } = tableApp();
  press(app, 3, "Middle");
  assert.deepEqual(
    payloads(received).filter((p) => p.RowClicked),
    [{ RowClicked: { idx: 2, button: "Middle", modifiers: NO_MODS } }],
  );
});

test("a Multi table grows a selection from clicks and keys", () => {
  const { app, received } = tableApp("Multi");
  press(app, 1);
  press(app, 3, "Left", SHIFT, 6.0);
  press(app, 2, "Left", CTRL);
  key(app, { Char: "a" }, CTRL);
  key(app, { Char: " " });
  key(app, { Char: " " }, CTRL);
  const sets = payloads(received)
    .filter((p) => p.SelectionSet)
    .map((p) => p.SelectionSet.indices);
  assert.deepEqual(sets, [[0, 1, 2], [0, 2], [0, 1, 2, 3, 4], [1, 2, 3, 4]]);
});

test("two quick clicks on a Button call onClick twice", () => {
  let clicks = 0;
  const h = harness(new App(20, 3, { view: () => Button("+", { id: "inc", onClick: () => clicks++ }) }));
  h.clickText("+");
  h.clickText("+");
  assert.equal(clicks, 2);
});
