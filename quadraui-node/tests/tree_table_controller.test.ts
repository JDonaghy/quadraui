// Black-box coverage for the `TreeTableController` per-widget-id seam
// exposed through `quadraui-node`: a plain JS object decodes into a
// `"tree_table_controller"` extra (`EXTRA_PRIMITIVE_KINDS`'s generic
// registration already covers the descriptor shape -- no new napi method
// needed for that half), and the full lazy-loaded hierarchy round trip --
// expand a lazy branch -> `ChildrenRequested` -> `QuiApp.setTreeTableChildren`
// -> header-click sort (every loaded level re-ordered, hierarchy preserved)
// -> selection survives the sort by node id, not position -- reaches the
// registered handler as `{"TreeTableController": [id, event]}`, the same
// `{"TreeTable": [tableId, event]}` array shape a flat-slot `TreeTable`'s own
// `TreeTableEvent` already reaches it as (see
// `tree_table_interactions.test.ts`).
//
// Mirrors `quadraui-binding-core::engine`'s own
// `tree_table_controller_expand_fetch_sort_and_selection_round_trip` Rust
// test (and `quadraui-py/tests/test_tree_table_controller.py`), driven
// end-to-end through this binding's JS surface instead, at three hierarchy
// levels (root -> directory -> file) so both `QuiApp.setTreeTableChildren`
// calls answer a genuinely nested lazy load.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { isDeepStrictEqual } = require("node:util");

const { QuiApp } = require("../js/index.ts");

const NO_MODS = { shift: false, ctrl: false, alt: false, cmd: false };
const TAB = { KeyPressed: { key: { Named: "Tab" }, modifiers: NO_MODS, repeat: false } };
const DOWN = { KeyPressed: { key: { Named: "Down" }, modifiers: NO_MODS, repeat: false } };
const SPACE = { KeyPressed: { key: { Char: " " }, modifiers: NO_MODS, repeat: false } };
const ENTER = { KeyPressed: { key: { Named: "Enter" }, modifiers: NO_MODS, repeat: false } };

function cell(text: string) {
  return { spans: [{ text, fg: null, bg: null }] };
}

function node(id: string, label: string, hasChildren = false) {
  const n: Record<string, unknown> = { id, cells: [cell(label)] };
  if (hasChildren) {
    n.has_children = true;
  }
  return n;
}

// A single lazy-branch root (`TreeTableNodeData::lazy_branch`'s own
// "has_children, no loaded children yet" shape). The engine seeds a fresh
// `TreeTableController` from this exactly once -- the first frame "tt" is
// painted -- then leaves the controller's own live expand/selection/sort
// state alone on every later frame, so returning this same object unchanged
// from every render call below is enough; it is never re-seeded.
function descriptor() {
  return {
    id: "tt",
    columns: [{ title: "Name", width: { Flex: 1.0 } }],
    nodes: [node("root", "root", true)],
    selected_id: "root",
    has_focus: true,
    show_scrollbar: true,
    bordered: false,
    folders_first: false,
  };
}

function render() {
  // A single-leaf "row" layout tree, not the bare flat
  // `{tree_table_controller: ...}` top-level key -- a layout-tree leaf fills
  // the whole area its container gives it, which a hierarchy deeper than
  // one visible row needs (the flat six-slot-plus-extras system's fixed
  // 3-line band is far too short).
  return { row: [{ id: "tt", tree_table_controller: descriptor() }] };
}

function press(app: any, event: unknown) {
  app.pushEvent(event);
  app.run(1);
}

// `(row, column)` of `label`'s first occurrence in a rendered `screen()`
// string, or `[undefined, undefined]` if absent.
function locate(screen: string, label: string): [number | undefined, number | undefined] {
  const lines = screen.split("\n");
  for (let row = 0; row < lines.length; row++) {
    const col = lines[row].indexOf(label);
    if (col !== -1) {
      return [row, col];
    }
  }
  return [undefined, undefined];
}

function treeTableControllerEvents(received: any[], id: string, event: unknown) {
  return received.filter(
    (entry) =>
      "TreeTableController" in entry &&
      entry.TreeTableController[0] === id &&
      isDeepStrictEqual(entry.TreeTableController[1], event),
  );
}

test("three-level lazy tree: expand, fetch, sort, and selection round trip", () => {
  const app = new QuiApp(40, 12);
  app.setRender(render);
  app.screen();

  const received: any[] = [];
  app.setHandler((event: any) => received.push(event));

  // Tab lands keyboard focus on the sole tab stop -- nothing is focused on
  // a freshly built `QuiApp`, so the `KeyPressed` routing below (which
  // requires engine-level focus, not just the descriptor's own `has_focus`
  // cosmetic flag) has nothing to dispatch through yet.
  press(app, TAB);

  // Space on the selected lazy-branch root expands it -- no loaded children
  // yet, so this requests them exactly once.
  press(app, SPACE);
  assert.equal(
    treeTableControllerEvents(received, "tt", { ChildrenRequested: { id: "root" } }).length,
    1,
    JSON.stringify(received),
  );

  // Answer the request: two children, one itself a lazy branch (the tree's
  // second level), in an intentionally unsorted order.
  app.setTreeTableChildren("tt", "root", [
    node("zebra_dir", "zebra_dir", true),
    node("apple_file", "apple_file"),
  ]);
  app.screen();
  assert.ok(app.screenContains("zebra_dir"));
  assert.ok(app.screenContains("apple_file"));

  // Down from "root" selects its first (still unsorted) child, "zebra_dir"
  // -- picked deliberately so "selection survives the sort" below is a
  // genuine id-keyed survival, not a lucky positional coincidence.
  received.length = 0;
  press(app, DOWN);
  assert.ok(
    treeTableControllerEvents(received, "tt", { RowSelected: { id: "zebra_dir" } }).length > 0,
    JSON.stringify(received),
  );

  // Space on "zebra_dir" expands the tree's third level -- another
  // not-yet-loaded lazy branch, so this requests its own children too.
  received.length = 0;
  press(app, SPACE);
  assert.equal(
    treeTableControllerEvents(received, "tt", { ChildrenRequested: { id: "zebra_dir" } }).length,
    1,
    JSON.stringify(received),
  );

  app.setTreeTableChildren("tt", "zebra_dir", [node("z_leaf", "z_leaf"), node("a_leaf", "a_leaf")]);
  app.screen();
  assert.ok(app.screenContains("z_leaf"));
  assert.ok(app.screenContains("a_leaf"));

  // Down from "zebra_dir" selects its first (still unsorted) child, "z_leaf".
  received.length = 0;
  press(app, DOWN);
  assert.ok(
    treeTableControllerEvents(received, "tt", { RowSelected: { id: "z_leaf" } }).length > 0,
    JSON.stringify(received),
  );

  // A header click sorts every loaded level's children ascending -- both
  // "root"'s two children and "zebra_dir"'s two children -- without
  // flattening the hierarchy: "apple_file" moves ahead of "zebra_dir" at the
  // root level, and "a_leaf" ahead of "z_leaf" under "zebra_dir", each
  // staying nested under its own parent.
  //
  // Not a structural `find`: the generic per-runner TUI focus ring overwrites
  // the whole top row of a focused extra's rect with border glyphs, so the
  // header's own "Name" text is no longer on screen once "tt" is focused --
  // but the header row is still row 0 of this leaf's rect, which fills the
  // whole viewport, so clicking (x=5, y=0) still lands on column 0's header
  // exactly as `TreeTableHit::Header` resolves it.
  received.length = 0;
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 5.0, y: 0.0 }, modifiers: {} },
  });
  app.run(1);
  assert.ok(
    treeTableControllerEvents(received, "tt", { SortChanged: { col: 0, ascending: true } })
      .length > 0,
    JSON.stringify(received),
  );

  const screen = app.screen();
  const [appleRow, appleIndent] = locate(screen, "apple_file");
  const [zebraRow, zebraIndent] = locate(screen, "zebra_dir");
  const [aLeafRow, aLeafIndent] = locate(screen, "a_leaf");
  const [zLeafRow] = locate(screen, "z_leaf");
  assert.ok(
    appleRow !== undefined && zebraRow !== undefined && aLeafRow !== undefined && zLeafRow !== undefined,
    screen,
  );
  // "apple_file" now renders above "zebra_dir" at the root level.
  assert.ok(appleRow! < zebraRow!, screen);
  // "a_leaf"/"z_leaf" are still nested under "zebra_dir" -- indented further
  // right than it, and painted below it -- with "a_leaf" now first among
  // them.
  assert.ok(aLeafIndent! > zebraIndent!, screen);
  assert.ok(zebraRow! < aLeafRow! && aLeafRow! < zLeafRow!, screen);

  // Selection survives the sort by node id, not position: "z_leaf" is still
  // selected even though it moved from index 0 to index 1 among
  // "zebra_dir"'s children -- Enter activates it, not "a_leaf".
  received.length = 0;
  press(app, ENTER);
  assert.ok(
    treeTableControllerEvents(received, "tt", { RowActivated: { id: "z_leaf" } }).length > 0,
    JSON.stringify(received),
  );
});

test("setTreeTableChildren/setTreeTableSortOverride are a no-op for an unpainted id", () => {
  const app = new QuiApp(40, 12);
  app.setRender(render);
  app.screen();

  app.setTreeTableChildren("no-such-id", "root", [node("x", "x")]);
  app.setTreeTableSortOverride("no-such-id", [node("x", "x")]);

  const received: any[] = [];
  app.setHandler((event: any) => received.push(event));
  press(app, TAB);
  press(app, SPACE);
  assert.ok(
    treeTableControllerEvents(received, "tt", { ChildrenRequested: { id: "root" } }).length > 0,
    JSON.stringify(received),
  );
});
