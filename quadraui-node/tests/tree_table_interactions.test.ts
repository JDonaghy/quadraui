// Black-box coverage for `TreeTable`, wired through `quadraui-binding-core`
// as an additive primitive (`"tree_table" => TreeTable` in `extra_paint.rs`'s
// `extra_primitives!` table): a plain JS object decodes into a real
// `TreeTable` (a `DataTable` plus per-row tree metadata), renders its branch
// row's chevron and both rows' labels, and coexists with a fixed-slot leaf
// inside a `row` container -- no new napi method needed, since
// `EXTRA_PRIMITIVE_KINDS`'s generic registration already covers it. Ports
// `quadraui-py/tests/test_tree_table_interactions.py`.
//
// `TreeTable::hit_test` (the `ExtraPaint` impl) is coarse -- a click
// anywhere in the body still reaches the handler as a raw `MouseDown`,
// since a chevron-click `ToggleExpanded` semantic event isn't wired yet
// (see `extra_paint.rs`'s `TreeTable::hit_test` doc).
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

function cell(text: string) {
  return { spans: [{ text, fg: null, bg: null }] };
}

function row(text: string) {
  return { cells: [cell(text)] };
}

function treeTableDict() {
  return {
    table: {
      id: "files",
      columns: [{ title: "Name", width: { Fixed: 20.0 } }],
      rows: [row("src"), row("main.rs")],
      has_focus: true,
    },
    tree: [
      { path: [0], indent: 0, is_expanded: true, icon: null },
      { path: [0, 0], indent: 1, is_expanded: null, icon: null },
    ],
  };
}

test("tree_table renders the branch chevron and both labels", () => {
  const app = new QuiApp(40, 10);
  app.setRender(() => ({ tree_table: treeTableDict() }));
  app.screen();

  // Default `TreeStyle` chevrons -- TUI paints them verbatim, no codicon
  // substitution (that's a GTK/macOS/Win-only swap).
  assert.ok(app.screenContains("▾"), app.screen()); // "▾" expanded chevron
  assert.ok(app.screenContains("src"));
  assert.ok(app.screenContains("main.rs"));
});

test("tree_table as a row leaf coexists with a fixed slot", () => {
  const app = new QuiApp(40, 10);
  app.setRender(() => ({
    row: [
      { id: "btn:1", button: { id: "btn:1", label: "OK" } },
      { id: "tt:1", tree_table: treeTableDict() },
    ],
  }));

  assert.ok(app.screenContains("OK"));
  assert.ok(app.screenContains("src"));
});

test("a click on the table body reaches the handler as a raw MouseDown", () => {
  const app = new QuiApp(40, 10);
  app.setRender(() => ({ tree_table: treeTableDict() }));
  app.screen();

  const received: any[] = [];
  app.setHandler((event: unknown) => received.push(event));
  // Row 0 ("src", the expanded branch) sits just below the one-row
  // header, at viewport y == 1.0.
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 2.0, y: 1.0 }, modifiers: {} },
  });
  app.run(1);

  assert.ok(
    received.some((event) => "MouseDown" in event),
    `expected a raw MouseDown to reach the handler, got: ${JSON.stringify(received)}`,
  );
});
