// Black-box coverage for the `"tree_table_controller"` leaf through
// `quadraui-node`: the node tree is content the app sends from `render`
// every frame, lazily loaded children included, while the user's expand,
// selection, scroll and sort state stays with the widget across frames.
// Every event reaches the registered handler as
// `{"TreeTableController": [id, event]}`, the same array shape a flat
// `TreeTable`'s own `{"TreeTable": [tableId, event]}` takes (see
// `tree_table_interactions.test.ts`).
//
// The app answers `ChildrenRequested` from inside its own handler by
// recording the children in its own model; the next `render` sends them.
// A host that orders rows itself sends them in its own order with
// `sort_override: true`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { isDeepStrictEqual } = require("node:util");

const { QuiApp } = require("@quadraui/node");

const NO_MODS = { shift: false, ctrl: false, alt: false, cmd: false };
const key = (k: unknown) => ({ KeyPressed: { key: k, modifiers: NO_MODS, repeat: false } });
const TAB = key({ Named: "Tab" });
const DOWN = key({ Named: "Down" });
const LEFT = key({ Named: "Left" });
const RIGHT = key({ Named: "Right" });
const SPACE = key({ Char: " " });
const ENTER = key({ Named: "Enter" });
const HEADER_CLICK = {
  MouseDown: { button: "Left", position: { x: 5.0, y: 0.0 }, modifiers: {} },
};

// What the app "loads" for each branch, in a deliberately unsorted order.
const TREE: Record<string, [string, boolean][]> = {
  root: [
    ["zebra_dir", true],
    ["apple_file", false],
  ],
  zebra_dir: [
    ["z_leaf", false],
    ["a_leaf", false],
  ],
};

function cell(text: string) {
  return { spans: [{ text, fg: null, bg: null }] };
}

// The app's own model: the children it has loaded so far, keyed by branch
// id, and the order it sorts the root's children in when it sorts them
// itself.
class Files {
  loaded: Record<string, [string, boolean][]> = {};
  rootOrder: string[] | null = null;
  received: any[] = [];

  answerRequests: boolean;

  constructor(answerRequests = true) {
    this.answerRequests = answerRequests;
  }

  node(id: string, hasChildren: boolean): Record<string, unknown> {
    const n: Record<string, unknown> = { id, cells: [cell(id)] };
    if (hasChildren) {
      n.has_children = true;
      let children = this.loaded[id];
      if (id === "root" && children !== undefined && this.rootOrder !== null) {
        const order = this.rootOrder;
        children = [...children].sort((a, b) => order.indexOf(a[0]) - order.indexOf(b[0]));
      }
      if (children !== undefined) {
        n.children = children.map(([c, h]) => this.node(c, h));
      }
    }
    return n;
  }

  render() {
    const descriptor = {
      id: "tt",
      columns: [{ title: "Name", width: { Flex: 1.0 } }],
      nodes: [this.node("root", true)],
      selected_id: "root",
      sort_override: this.rootOrder !== null,
    };
    // A layout-tree leaf fills the whole area its container gives it,
    // which a tree deeper than one visible row needs.
    return { row: [{ id: "tt", tree_table_controller: descriptor }] };
  }

  handle(event: any) {
    this.received.push(event);
    if (!("TreeTableController" in event) || !this.answerRequests) {
      return;
    }
    const payload = event.TreeTableController[1];
    if ("ChildrenRequested" in payload) {
      const id = payload.ChildrenRequested.id;
      this.loaded[id] = TREE[id];
    }
  }

  events(event: unknown) {
    return this.received.filter(
      (entry) =>
        "TreeTableController" in entry &&
        entry.TreeTableController[0] === "tt" &&
        isDeepStrictEqual(entry.TreeTableController[1], event),
    );
  }
}

function makeApp(files: Files) {
  const app = new QuiApp(40, 12);
  app.setRender(() => files.render());
  app.screen();
  app.setHandler((event: any) => files.handle(event));
  return app;
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

test("three-level lazy tree: expand, fetch, sort, and selection round trip", () => {
  const files = new Files();
  const app = makeApp(files);

  // Tab focuses the sole tab stop; Space on the selected lazy root expands
  // it and requests its children, which the handler loads and the next
  // frame sends.
  press(app, TAB);
  press(app, SPACE);
  assert.equal(
    files.events({ ChildrenRequested: { id: "root" } }).length,
    1,
    JSON.stringify(files.received),
  );
  assert.ok(app.screenContains("zebra_dir"));
  assert.ok(app.screenContains("apple_file"));

  // Down selects the first (still unsorted) child, so the selection
  // surviving the sort below is an id-keyed survival.
  press(app, DOWN);
  assert.ok(files.events({ RowSelected: { id: "zebra_dir" } }).length > 0);

  press(app, SPACE);
  assert.equal(files.events({ ChildrenRequested: { id: "zebra_dir" } }).length, 1);
  assert.ok(app.screenContains("z_leaf"));
  assert.ok(app.screenContains("a_leaf"));

  press(app, DOWN);
  assert.ok(files.events({ RowSelected: { id: "z_leaf" } }).length > 0);

  // A header click sorts every loaded level's children, keeping the
  // hierarchy: "apple_file" above "zebra_dir", "a_leaf" above "z_leaf",
  // both leaves still nested under "zebra_dir".
  press(app, HEADER_CLICK);
  assert.ok(files.events({ SortChanged: { col: 0, ascending: true } }).length > 0);

  const screen = app.screen();
  const [appleRow] = locate(screen, "apple_file");
  const [zebraRow, zebraIndent] = locate(screen, "zebra_dir");
  const [aLeafRow, aLeafIndent] = locate(screen, "a_leaf");
  const [zLeafRow] = locate(screen, "z_leaf");
  assert.ok(
    appleRow !== undefined &&
      zebraRow !== undefined &&
      aLeafRow !== undefined &&
      zLeafRow !== undefined,
    screen,
  );
  assert.ok(appleRow! < zebraRow!, screen);
  assert.ok(aLeafIndent! > zebraIndent!, screen);
  assert.ok(zebraRow! < aLeafRow! && aLeafRow! < zLeafRow!, screen);

  // The selection survives the sort by node id: Enter activates "z_leaf",
  // which moved from first to second among its siblings.
  files.received.length = 0;
  press(app, ENTER);
  assert.ok(files.events({ RowActivated: { id: "z_leaf" } }).length > 0);
});

test("an unanswered lazy branch shows a loading row and is asked once", () => {
  const files = new Files(false);
  const app = makeApp(files);
  press(app, TAB);
  press(app, SPACE);
  assert.ok(app.screenContains("…"), app.screen());

  // Collapsing and expanding again before the app answers asks nothing
  // more.
  press(app, LEFT);
  press(app, RIGHT);
  assert.equal(files.events({ ChildrenRequested: { id: "root" } }).length, 1);
});

test("sort_override paints the order the app sends", () => {
  const files = new Files();
  const app = makeApp(files);
  press(app, TAB);
  press(app, SPACE);

  // The header click reports the sort; this app orders the root's children
  // itself, reverse-alphabetically, and says so with `sort_override`.
  press(app, HEADER_CLICK);
  assert.ok(files.events({ SortChanged: { col: 0, ascending: true } }).length > 0);
  files.rootOrder = ["zebra_dir", "apple_file"];

  const screen = app.screen();
  const [appleRow] = locate(screen, "apple_file");
  const [zebraRow] = locate(screen, "zebra_dir");
  assert.ok(appleRow !== undefined && zebraRow !== undefined, screen);
  assert.ok(zebraRow! < appleRow!, screen);
});

test("the imperative tree-table methods are gone", () => {
  const app = new QuiApp(40, 12) as any;
  for (const name of ["setTreeTableChildren", "setTreeTableSortOverride"]) {
    assert.equal(typeof app[name], "undefined", name);
  }
});
