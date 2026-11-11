// Unit tests for `js/app/`: id auto-assignment stability across re-render,
// callback dispatch (`Tree`, `Table`, `Tabs`, `Button`), component state
// survival, and redraw-on-mutation -- the four properties the app layer's
// own module doc (`js/app/index.ts`) promises and `examples/file_manager_app.ts`
// relies on wholesale. Mirrors `quadraui-py/tests/test_app_layer.py` 1:1.
//
// Run with `npm run build:debug` (or `npm run build`) then `npm test`.
const { test } = require("node:test");
const assert = require("node:assert/strict");

const {
  App,
  Button,
  Column,
  Component,
  SidebarPanel,
  Split,
  StatusBar,
  Table,
  Tabs,
  Tab,
  TextDisplay,
  Toolbar,
  Tree,
} = require("../js/app/index.ts");

const NO_MODS = { shift: false, ctrl: false, alt: false, cmd: false };

function keyEvent(key: unknown) {
  return { KeyPressed: { key, modifiers: NO_MODS, repeat: false } };
}

// --- id auto-assignment -----------------------------------------------------

test("auto-assigned ids are stable across a re-render at the same position", () => {
  const calls: number[] = [];
  const app = new App(20, 5, { view: () => Button("Save", { onClick: () => calls.push(1) }) });

  const idsBefore = new Set(Object.keys(app._registry));
  assert.ok(app.screenContains("Save"));
  app.step();
  const idsAfter = new Set(Object.keys(app._registry));
  assert.deepEqual(idsBefore, idsAfter);
  assert.equal(idsBefore.size, 1);
});

test("auto ids differ by structural position", () => {
  const app = new App(20, 5, {
    view: () => Column([Button("A", { onClick: () => {} }), Button("B", { onClick: () => {} })]),
  });
  assert.ok(app.screenContains("A"));
  const ids = Object.keys(app._registry).sort();
  assert.equal(ids.length, 2);
  assert.notEqual(ids[0], ids[1]);
});

test("every id-bearing node constructor accepts an omitted id", () => {
  const app = new App(20, 5, {
    view: () => {
      const tree = Tree([], { onActivate: () => {} });
      const table = Table([], []);
      const status = StatusBar([], []);
      const text = TextDisplay([]);
      const tabs = Tabs([Tab(text, { title: "one" })]);
      const panel = SidebarPanel(Column([tree, table, status, tabs]));
      return Split(panel, text);
    },
  });
  assert.notEqual(app._render(), null);
});

test("auto ids from every node constructor are stable across re-render", () => {
  const app = new App(20, 5, {
    view: () =>
      Split(Tree([], { onRowClick: () => {} }), Table([], [], { onSelect: () => {} })),
  });
  const idsBefore = Object.keys(app._registry).sort();
  app.step();
  const idsAfter = Object.keys(app._registry).sort();
  assert.deepEqual(idsBefore, idsAfter);
  assert.equal(idsBefore.length, 2);
});

test("Toolbar without an explicit id still builds a wire object", () => {
  const toolbar = Toolbar([{ Action: { id: "a", label: "Go" } }]);
  assert.equal(typeof toolbar.id, "string");
  assert.deepEqual(toolbar.buttons, [{ Action: { id: "a", label: "Go" } }]);

  const other = Toolbar([]);
  assert.notEqual(other.id, toolbar.id);
});

// --- callback dispatch -------------------------------------------------------

test("tree row click and toggle-expand dispatch to their own callback", () => {
  const clicked: unknown[] = [];
  const toggled: unknown[] = [];

  const app = new App(20, 5, {
    view: () =>
      Tree(
        [{ path: [0], indent: 0, icon: null, text: { spans: [] }, badge: null, is_expanded: false }],
        {
          id: "t:tree",
          onRowClick: (path: unknown) => clicked.push(path),
          onToggleExpand: (path: unknown) => toggled.push(path),
        },
      ),
  });

  app.pushEvent({ Tree: ["t:tree", { RowClicked: { path: [0], modifiers: NO_MODS } }] });
  app.step();
  assert.deepEqual(clicked, [[0]]);
  assert.deepEqual(toggled, []);

  app.pushEvent({ Tree: ["t:tree", { RowToggleExpand: { path: [0] } }] });
  app.step();
  assert.deepEqual(toggled, [[0]]);
});

test("table row double-clicked dispatches onActivate, not onSelect", () => {
  const activated: unknown[] = [];
  const selected: unknown[] = [];

  const app = new App(20, 5, {
    view: () =>
      Table(
        [{ title: "Name", width: { Flex: 1.0 }, align: "Left" }],
        [{ cells: [{ spans: [{ text: "a", fg: null, bg: null }] }] }],
        {
          id: "t:grid",
          onActivate: (idx: unknown) => activated.push(idx),
          onSelect: (idx: unknown) => selected.push(idx),
        },
      ),
  });

  app.pushEvent({ DataTable: ["t:grid", { RowSelected: { idx: 0 } }] });
  app.step();
  assert.deepEqual(selected, [0]);
  assert.deepEqual(activated, []);

  app.pushEvent({ DataTable: ["t:grid", { RowDoubleClicked: { idx: 0 } }] });
  app.step();
  assert.deepEqual(activated, [0]);
});

test("tabs closed dispatches onClose with the index", () => {
  const closed: unknown[] = [];

  const app = new App(20, 5, {
    view: () =>
      Tabs([Tab(TextDisplay([], { id: "t:c0" }), { title: "one" })], {
        id: "t:tabs",
        onClose: (idx: unknown) => closed.push(idx),
      }),
  });

  app.pushEvent({ TabBar: ["t:tabs", { TabClosed: { index: 0 } }] });
  app.step();
  assert.deepEqual(closed, [0]);
});

function clickAt(app: any, x = 0, y = 0) {
  app.pushEvent({ MouseDown: { button: "Left", position: { x, y }, modifiers: {} } });
  app.step();
}

test("button onClick fires on a hit-tested MouseDown", () => {
  const clicks: number[] = [];
  const app = new App(20, 5, { view: () => Button("Save", { id: "t:save", onClick: () => clicks.push(1) }) });
  clickAt(app);
  assert.deepEqual(clicks, [1]);
});

test("button onClick fires on Enter while focused", () => {
  const clicks: number[] = [];
  const app = new App(20, 5, {
    view: () => Button("Save", { id: "t:save", onClick: () => clicks.push(1), focused: true }),
  });
  app.pushEvent({ FocusChanged: "t:save" });
  app.step();
  app.pushEvent(keyEvent({ Named: "Enter" }));
  app.step();
  assert.deepEqual(clicks, [1]);
});

test("an unmatched event falls through to onEvent", () => {
  const seen: unknown[] = [];
  const { keyName } = require("../js/app/index.ts");
  const app = new App(20, 5, {
    view: () => TextDisplay([], { id: "t:text" }),
    onEvent: (event: unknown) => seen.push(event),
  });
  app.pushEvent(keyEvent({ Named: "Down" }));
  app.step();
  assert.equal(seen.length, 1);
  assert.equal(keyName(seen[0]), "Down");
});

// --- components --------------------------------------------------------------

class Counter extends Component {
  initialState() {
    return { count: 0 };
  }
  view(state: Record<string, unknown>) {
    return Button(`count=${state.count}`, {
      focused: true,
      onClick: () => {
        state.count = (state.count as number) + 1;
      },
    });
  }
}

function label(app: any): string {
  return app._render().button.label;
}

test("component state survives re-render and a mutation is visible next render", () => {
  const app = new App(20, 5, { view: () => new Counter() });
  assert.equal(label(app), "count=0");

  const buttonId = Object.keys(app._registry)[0];
  app.pushEvent({ FocusChanged: buttonId });
  app.step();

  for (const expected of ["count=1", "count=2", "count=3"]) {
    app.pushEvent(keyEvent({ Named: "Enter" }));
    app.step();
    assert.equal(label(app), expected);
    assert.ok(app.screenContains(expected));
  }
});

test("two components in a list keep independent state by position", () => {
  const app = new App(20, 5, { view: () => Column([new Counter(), new Counter()]) });
  const ids = Object.keys(app._registry).sort();
  assert.equal(ids.length, 2);

  app._registry[ids[0]].on_click();
  app.step();

  const screen = app.screen();
  assert.ok(screen.includes("count=1"));
  assert.ok(screen.includes("count=0"));
});

test("closing and reopening a row does not resurrect the old state", () => {
  let counters = [new Counter(), new Counter()];
  const app = new App(20, 5, { view: () => Column(counters) });
  const ids = Object.keys(app._registry).sort();
  assert.equal(ids.length, 2);

  for (let i = 0; i < 2; i++) {
    app._registry[ids[1]].on_click();
  }
  assert.ok(app.screen().includes("count=2"));
  assert.equal(app._componentStates.size, 2);

  // Render with only position 0 present -- position 1's slot must be
  // pruned, since nothing visited it this render.
  counters = [counters[0]];
  app.screen();
  assert.equal(app._componentStates.size, 1);

  // A brand-new `Counter` rendered back at position 1 starts fresh.
  counters = [counters[0], new Counter()];
  const screen = app.screen();
  assert.ok(!screen.includes("count=2"));
  assert.equal(screen.split("count=0").length - 1, 2);
});
