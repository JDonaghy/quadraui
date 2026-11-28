// Black-box coverage for a selection the app moves itself: a `list` or
// `data_table` leaf whose render callback sets `selected_idx` but leaves
// `scroll_offset` alone scrolls that row into view on the next paint, the
// same way a selection the widget's own keys moved does. A `scroll_offset`
// the app pins stays where the app put it. Mirrors
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
