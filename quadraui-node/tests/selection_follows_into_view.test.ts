// Black-box coverage for a selection the app moves itself: a `list` or
// `data_table` leaf whose render callback sets `selected_idx` but leaves
// `scroll_offset` alone scrolls that row into view on the next paint, the
// same way a selection the widget's own keys moved does. A `scroll_offset`
// the app pins stays where the app put it. A windowed leaf follows an
// absolute selection the same way, and a windowed tree's keyboard cursor
// moves to a `selected_path` the app jumps to and keeps its row's path when
// the app re-flattens the rows above it. Mirrors
// `quadraui-py/tests/test_selection_follows_into_view.py`.
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

const ROWS = 200;

function cell(text: string) {
  return { spans: [{ text, fg: null, bg: null }] };
}

function listApp(state: { selected: number; scroll?: number }) {
  const app = new QuiApp(24, 8);
  app.setRender(() => {
    const items = [];
    for (let i = 0; i < ROWS; i++) {
      items.push({ text: cell(`item-${i}`) });
    }
    const leaf: any = { id: "list", items, selected_idx: state.selected };
    if (state.scroll !== undefined) {
      leaf.scroll_offset = state.scroll;
    }
    return { column: [{ id: "list", list: leaf }] };
  });
  return app;
}

function tableApp(state: { selected: number | null }) {
  const app = new QuiApp(24, 8);
  app.setRender(() => {
    const rows = [];
    for (let i = 0; i < ROWS; i++) {
      rows.push({ cells: [cell(`row-${i}`)] });
    }
    return {
      column: [
        {
          id: "table",
          data_table: {
            id: "table",
            columns: [{ title: "Row" }],
            rows,
            selected_idx: state.selected,
          },
        },
      ],
    };
  });
  return app;
}

test("a list selection the app moves scrolls into view", () => {
  const state = { selected: 0 };
  const app = listApp(state);
  assert.ok(app.screen().includes("item-0"));
  state.selected = 150;
  assert.ok(app.screen().includes("item-150"));
  assert.ok((app.widgetState("list")?.scrollOffset ?? 0) > 0);
});

test("a list scroll the app pins stays put", () => {
  const app = listApp({ selected: 150, scroll: 10 });
  const screen = app.screen();
  assert.ok(screen.includes("item-10"));
  assert.ok(!screen.includes("item-150"));
});

test("a data table selection the app moves scrolls into view", () => {
  const state: { selected: number | null } = { selected: null };
  const app = tableApp(state);
  assert.ok(app.screen().includes("row-0"));
  state.selected = 120;
  assert.ok(app.screen().includes("row-120"));
  assert.ok((app.widgetState("table")?.scrollOffset ?? 0) > 0);
});

// ── windowed leaves: the selection is an absolute row index ──

const WINDOW_TOTAL = 1000;
const LOADED = 50;

function key(name: string) {
  return { KeyPressed: { key: { Named: name }, modifiers: {}, repeat: false } };
}

test("a windowed list selection the app moves scrolls into view", () => {
  const state = { selected: 0 };
  const app = new QuiApp(24, 8);
  app.setRender(() => {
    const items = [];
    for (let i = 0; i < LOADED; i++) {
      items.push({ text: cell(`item-${i}`) });
    }
    return {
      column: [
        {
          id: "list",
          list: {
            id: "list",
            items,
            items_window: { start: 0, total: WINDOW_TOTAL },
            selected_idx: state.selected,
          },
        },
      ],
    };
  });
  assert.ok(app.screen().includes("item-0"));
  state.selected = 40;
  const screen = app.screen();
  assert.ok(screen.includes("item-40"), screen);
  assert.ok((app.widgetState("list")?.scrollOffset ?? 0) > 0);
});

function treeRow(path: number[], text: string, expanded: boolean | null = null) {
  return { path, indent: path.length - 1, icon: null, text: cell(text), badge: null, is_expanded: expanded };
}

// A windowed tree the app flattens itself: branch `[0]` (five children
// while expanded), then leaves `[1]..[40]`. The app echoes every
// `SelectionChanged` and records each `RowActivated` path.
function windowedTreeApp(state: { selected: number[] | null; collapsed: boolean }, activated: number[][]) {
  const app = new QuiApp(24, 8);
  app.setRender(() => {
    const rows = [treeRow([0], "branch", !state.collapsed)];
    if (!state.collapsed) {
      for (let i = 0; i < 5; i++) rows.push(treeRow([0, i], `child-${i}`));
    }
    for (let i = 1; i <= 40; i++) rows.push(treeRow([i], `leaf-${i}`));
    return {
      column: [
        {
          id: "tree",
          tree: {
            id: "tree",
            rows,
            items_window: { start: 0, total: rows.length },
            selection_mode: "Single",
            selected_path: state.selected,
            style: { indent: 2, show_chevrons: true, chevron_expanded: "v", chevron_collapsed: ">" },
          },
        },
      ],
    };
  });
  app.setHandler((event: any) => {
    if (!event.Tree) return;
    const treeEvent = event.Tree[1];
    if (treeEvent.SelectionChanged) state.selected = treeEvent.SelectionChanged.path;
    else if (treeEvent.RowActivated) activated.push(treeEvent.RowActivated.path);
  });
  app.screen();
  return app;
}

function press(app: any, ...names: string[]) {
  for (const name of names) {
    app.pushEvent(key(name));
    app.run(1);
  }
}

test("a windowed tree follows a selection the app jumps to", () => {
  const state: { selected: number[] | null; collapsed: boolean } = { selected: null, collapsed: false };
  const activated: number[][] = [];
  const app = windowedTreeApp(state, activated);
  press(app, "Tab", "Down", "Down", "Down");
  assert.deepEqual(state.selected, [0, 1]);
  state.selected = [5];
  app.screen();
  assert.deepEqual(app.widgetState("tree")?.selectedPath, [5]);
  press(app, "Enter");
  assert.deepEqual(activated, [[5]]);
});

test("a windowed tree cursor keeps its path when rows above it collapse", () => {
  const state: { selected: number[] | null; collapsed: boolean } = { selected: null, collapsed: false };
  const activated: number[][] = [];
  const app = windowedTreeApp(state, activated);
  press(app, "Tab", ...Array(9).fill("Down"));
  assert.deepEqual(state.selected, [3]);
  state.collapsed = true;
  app.screen();
  assert.deepEqual(app.widgetState("tree")?.selectedPath, [3]);
  press(app, "Enter");
  assert.deepEqual(activated, [[3]]);
});
