// Black-box coverage for `../js/testing.ts`, the app-author test harness:
// the query/act/assert API a consumer writes their own app's tests
// against. Mirrors `quadraui-py/tests/test_testing_harness.py`.
//
// Each test below drives a small real app through the harness and checks
// the harness reported what the app actually rendered -- the harness is
// only worth anything if a green assertion means the app really is in
// that state, so nothing here inspects the app object.

const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");
const {
  App,
  Button,
  Column,
  SidebarPanel,
  Split,
  Tab,
  Table,
  Tabs,
  TextDisplay,
  Toolbar,
  Tree,
  keyName,
} = require("../js/app/index.ts");
const { Harness, WidgetNotFoundError, harness } = require("../js/testing.ts");

function line(text: string) {
  return { spans: [{ text, fg: null, bg: null }] };
}

function treeRow(path: number[], text: string, expanded = false) {
  return {
    path,
    indent: path.length - 1,
    icon: null,
    text: line(text),
    badge: null,
    is_expanded: expanded,
  };
}

function tableRow(...cells: string[]) {
  return { cells: cells.map(line) };
}

/** A tree over a table, a tab strip and a toolbar -- one of every shape the harness has a verb for. */
class Demo {
  log: unknown[] = [];
  keys: string[] = [];
  openTabs: string[] = ["one"];
  app: any;

  constructor(width = 70, height = 24) {
    this.app = new App(width, height, {
      view: () => this.view(),
      onEvent: (event: unknown) => {
        const name = keyName(event);
        if (name !== null) {
          this.keys.push(name);
        }
      },
    });
  }

  view(): unknown {
    return Split(
      Tree(
        [treeRow([0], "alpha", true), treeRow([0, 0], "alpha-child"), treeRow([1], "beta")],
        {
          id: "d:tree",
          selectedPath: [0],
          onRowClick: (path: number[]) => this.log.push(["click", path]),
          onSelect: (path: number[]) => this.log.push(["select", path]),
          onActivate: (path: number[]) => this.log.push(["activate", path]),
          onToggleExpand: (path: number[]) => this.log.push(["toggle", path]),
        },
      ),
      Tabs(
        this.openTabs.map((title, i) =>
          Tab(
            Table([{ title: "Name" }, { title: "Kind" }], [tableRow("a1.txt", "file"), tableRow("sub", "folder")], {
              id: `d:grid:${i}`,
              onSelect: (idx: number) => this.log.push(["row-select", idx]),
              onActivate: (idx: number) => this.log.push(["row-activate", idx]),
            }),
            { title },
          ),
        ),
        {
          id: "d:tabs",
          onActivate: (index: number) => this.log.push(["tab", index]),
          onClose: (index: number) => this.log.push(["close", index]),
        },
      ),
      { id: "d:split", orientation: "horizontal", ratio: 0.4 },
    );
  }
}

function demoHarness(): [Demo, any] {
  const demo = new Demo();
  return [demo, harness(demo.app)];
}

// --- querying ----------------------------------------------------------------

test("find locates a widget by id, role or text", () => {
  const [, h] = demoHarness();

  assert.equal(h.find({ id: "d:tree" }).role, "tree");
  assert.equal(h.find({ role: "data_table" }).id, "d:grid:0");
  assert.equal(h.find({ role: "tree", text: "beta" }).id, "d:tree");
  assert.equal(h.root.role, "split");
});

test("find throws a readable error when nothing matches", () => {
  const [, h] = demoHarness();

  assert.throws(() => h.find({ role: "combo_box" }), { name: "WidgetNotFoundError", message: /role="combo_box"/ });
  assert.equal(h.query({ role: "combo_box" }), null);
  assert.equal(h.exists({ role: "combo_box" }), false);
  assert.equal(h.exists({ role: "tree" }), true);
});

test("a widget's find searches only its own subtree", () => {
  const [, h] = demoHarness();
  const tabs = h.find({ role: "tabs" });

  assert.equal(tabs.find({ role: "data_table" }).id, "d:grid:0");
  assert.throws(() => tabs.find({ role: "tree" }), WidgetNotFoundError);
});

test("rows report their text, cells and expansion", () => {
  const [, h] = demoHarness();
  const tree = h.find({ role: "tree" });

  assert.deepEqual(tree.rowTexts, ["alpha", "alpha-child", "beta"]);
  assert.equal(tree.row("alpha").expanded, true);
  assert.equal(tree.row("beta").expanded, false);
  assert.deepEqual(tree.row("beta").path, [1]);
  assert.equal(tree.selectedRow.text, "alpha");

  const table = h.find({ role: "data_table" });
  assert.deepEqual(
    table.rows.map((row: any) => row.cells),
    [
      ["a1.txt", "file"],
      ["sub", "folder"],
    ],
  );
});

test("row lookup reports what is actually there", () => {
  const [, h] = demoHarness();
  const tree = h.find({ role: "tree" });

  assert.equal(tree.row({ index: 2 }).text, "beta");
  assert.equal(tree.row({ path: [0, 0] }).text, "alpha-child");
  assert.throws(() => tree.row("nonexistent"), /alpha-child/);
});

test("a hidden tab is invisible to a query until asked for", () => {
  const demo = new Demo();
  demo.openTabs = ["one", "two"];
  const h = harness(demo.app);

  assert.equal(h.findAll({ role: "data_table" }).length, 1);
  assert.equal(h.findAll({ role: "data_table", includeHidden: true }).length, 2);
  assert.equal(h.find({ role: "tabs" }).activeIndex, 0);
  assert.deepEqual(
    h.find({ role: "tabs" }).tabs.map((tab: any) => tab.title),
    ["one", "two"],
  );
});

// --- acting ------------------------------------------------------------------

test("row actions dispatch the event each gesture delivers", () => {
  const [demo, h] = demoHarness();
  const tree = h.find({ role: "tree" });

  tree.row("beta").click();
  tree.row("beta").select();
  tree.row("beta").doubleClick();
  tree.row("alpha").toggle();

  assert.deepEqual(demo.log, [
    ["click", [1]],
    ["select", [1]],
    ["activate", [1]],
    ["toggle", [0]],
  ]);
});

test("table row actions separate select from activate", () => {
  const [demo, h] = demoHarness();
  const table = h.find({ role: "data_table" });

  table.row("sub").click();
  table.row("sub").doubleClick();
  table.row("a1.txt").activate();

  assert.deepEqual(demo.log, [
    ["row-select", 1],
    ["row-activate", 1],
    ["row-activate", 0],
  ]);
});

test("tab click and close reach the tab strip callbacks", () => {
  const [demo, h] = demoHarness();
  const tabs = h.find({ role: "tabs" });

  tabs.tabs[0].click();
  tabs.tabs[0].close();

  assert.deepEqual(demo.log, [
    ["tab", 0],
    ["close", 0],
  ]);
});

test("press and type deliver plain top-level keys", () => {
  const [demo, h] = demoHarness();

  h.press("Down");
  h.press("F2");
  h.type("hi");

  assert.deepEqual(demo.keys, ["Down", "F2", "h", "i"]);
});

test("press rejects a name that is not a key", () => {
  const [, h] = demoHarness();

  assert.throws(() => h.press("Dowm"), /not a key name/);
});

test("a widget's press delivers the focus-scoped shape", () => {
  // A focused widget's keys arrive scoped to it on a real backend; the
  // harness sends that shape, and the app's own `keyName` still reads it.
  const [demo, h] = demoHarness();

  h.find({ role: "tree" }).press("Down");

  assert.deepEqual(demo.keys, ["Down"]);
});

test("a button is clicked through the engine's own hit test", () => {
  const clicks: string[] = [];
  const app = new App(30, 6, {
    view: () =>
      Column([
        Button("Save", { id: "t:save", onClick: () => clicks.push("save") }),
        TextDisplay([line("idle")], { id: "t:out" }),
      ]),
  });
  const h = harness(app);

  h.find({ role: "button" }).click();

  assert.deepEqual(clicks, ["save"]);
});

test("clickText reports where a label is not on screen", () => {
  const h = harness(new App(30, 6, { view: () => TextDisplay([line("hello")], { id: "t:out" }) }));

  assert.deepEqual(h.locateAll("hello"), [[2.5, 0]]);
  assert.throws(() => h.clickText("goodbye"), /appears 0 time/);
});

test("a split divider drag persists in the store, not the descriptor", () => {
  const [, h] = demoHarness();
  assert.ok(Math.abs((h.find({ id: "d:split" }).get("ratio") as number) - 0.4) < 1e-6);

  h.find({ id: "d:split" }).dragDivider(0.75);

  assert.ok(Math.abs(h.find({ id: "d:split" }).ratio - 0.75) < 1e-4);
  // The app keeps declaring its own literal; the store holds the drag.
  assert.ok(Math.abs((h.find({ id: "d:split" }).get("ratio") as number) - 0.4) < 1e-6);
});

test("actions on the wrong role say so", () => {
  const [, h] = demoHarness();

  assert.throws(() => h.find({ role: "tabs" }).rows, /has no rows/);
  assert.throws(() => h.find({ role: "tree" }).dragDivider(0.5), /has no divider/);
  assert.throws(() => h.find({ role: "data_table" }).row({ index: 0 }).toggle(), /no "toggle" action/);
  assert.throws(() => h.find({ role: "tree" }).click(), /not clicked as a whole/);
});

// --- asserting ----------------------------------------------------------------

test("assertText prints the frame when it fails", () => {
  const [, h] = demoHarness();

  h.assertText("alpha", "beta", "a1.txt");
  h.assertNotText("gamma");

  assert.throws(
    () => h.assertText("gamma"),
    (error: Error) => error.message.includes("gamma") && error.message.includes("alpha"),
  );
  assert.throws(() => h.assertNotText("alpha"), /unexpectedly on screen/);
});

test("screen and contains read the rendered frame", () => {
  const [, h] = demoHarness();

  assert.equal(h.contains("alpha"), true);
  assert.equal(h.contains("gamma"), false);
  assert.ok(h.screen().includes("alpha"));
  assert.ok(h.screenLines().some((line_: string) => line_.includes("alpha")));
});

test("a toolbar reports its buttons", () => {
  const app = new App(30, 6, {
    view: () =>
      SidebarPanel(TextDisplay([line("body")], { id: "t:body" }), {
        id: "t:chrome",
        toolbar: Toolbar([{ Action: { id: "t:open", label: "Open" } }, "Separator"], { id: "t:bar" }),
      }),
  });
  const h = harness(app);
  const toolbar = h.find({ role: "toolbar" });

  assert.deepEqual(toolbar.buttonLabels, ["Open"]);
  assert.equal(toolbar.button("Open").id, "t:open");
  assert.throws(() => toolbar.button("Close"), /no toolbar button/);
});

// --- wrapping a plain QuiApp ---------------------------------------------------

test("a plain QuiApp is driven through the same API", () => {
  // The harness is not tied to the app layer: a raw `QuiApp` works too,
  // given the same render callable the app was built with.
  const state = { count: 0 };
  const render = () => ({ id: "q:out", text_display: { id: "q:out", lines: [line(`count ${state.count}`)] } });

  const app = new QuiApp(30, 4);
  app.setRender(render);
  app.setHandler((event: unknown) => {
    if (keyName(event) === "Up") {
      state.count += 1;
    }
  });
  const h = harness(app, { render });

  assert.equal(h.find({ role: "text_display" }).text, "count 0");
  h.press("Up");
  assert.equal(h.find({ role: "text_display" }).text, "count 1");
  h.assertText("count 1");
});

test("wrapping a plain QuiApp without its render is refused", () => {
  assert.throws(() => new Harness(new QuiApp(10, 3)), /render/);
});
