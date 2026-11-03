// Black-box coverage for `QuiApp.runShell`/`QuiShellApp` through the
// published JS surface (`js/index.ts`), run with `npm test` -> `node --test
// tests/*.test.ts`. A port of `quadraui-py/tests/test_run_shell.py`.
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

const CONFIG = {
  title: "Demo",
  panels: [{ id: "panel:files", icon: "@", tooltip: "Files", title: "FILES" }],
  bottom_panel: {
    tabs: [
      { id: "bp:a", label: "ALPHA" },
      { id: "bp:b", label: "BETA" },
    ],
    active_tab_id: "bp:a",
  },
};

function newShell(options: Record<string, unknown> = {}) {
  const { onEvent, renderContent, onShellEvent, onBottomPanelEvent, onTick } = options as {
    onEvent?: (event: unknown) => unknown;
    renderContent?: () => unknown;
    onShellEvent?: (event: unknown) => unknown;
    onBottomPanelEvent?: (event: unknown) => unknown;
    onTick?: () => unknown;
  };
  return QuiApp.runShell(80, 24, CONFIG, onEvent, renderContent, onShellEvent, onBottomPanelEvent, onTick);
}

test("initial frame shows the sidebar and active tab's content", () => {
  const app = newShell();
  const screen = app.screen();
  assert.match(screen, /FILES/);
  assert.match(screen, /ALPHA/);
  assert.match(screen, /BETA/);
  // `StaticTabContent` paints the active tab's own id into the content
  // area, so the initially-active tab ("bp:a") is visible and the other
  // ("bp:b") is not.
  assert.match(screen, /bp:a/);
  assert.doesNotMatch(screen, /bp:b/);
});

test("clicking the active panel's icon toggles the sidebar", () => {
  const events: unknown[] = [];
  const app = newShell({ onShellEvent: (event: unknown) => events.push(event) });

  const [x, y] = app.find("@");
  app.pushClick(x, y);
  app.run(2); // drain both the MouseDown and MouseUp pushClick queues

  assert.ok(!app.screenContains("FILES"), `clicking the already-active panel's icon must hide the sidebar:\n${app.screen()}`);
  assert.deepEqual(events, [{ SidebarHidden: null }]);

  // Clicking the same icon again must bring it back.
  app.pushClick(x, y);
  app.run(2);

  assert.ok(app.screenContains("FILES"));
  assert.deepEqual(events[events.length - 1], { PanelChanged: { panel_id: "panel:files" } });
});

test("switching a bottom panel tab changes the rendered frame", () => {
  const events: unknown[] = [];
  const app = newShell({ onBottomPanelEvent: (event: unknown) => events.push(event) });

  const before = app.screen();
  assert.match(before, /bp:a/);
  assert.doesNotMatch(before, /bp:b/);

  const [x, y] = app.bottomPanelTabCenter(1);
  app.pushClick(x, y);
  app.run(2);

  const after = app.screen();
  assert.match(after, /bp:b/, `switching to the second tab must show its content:\n${after}`);
  assert.doesNotMatch(after, /bp:a/, `the first tab's content must no longer be painted:\n${after}`);
  assert.deepEqual(events, [{ TabActivated: "bp:b" }]);
});

test("renderContent paints into the main content area", () => {
  const app = newShell({ renderContent: () => ["hello from node"] });
  app.run(1);
  assert.ok(app.screenContains("hello from node"));
});

test("setHandler receives events the shell chrome did not consume", () => {
  const received: unknown[] = [];
  const app = newShell();
  app.setHandler((event: unknown) => received.push(event));

  app.pushKeyEvent("x".codePointAt(0));
  app.run(1);

  assert.equal(received.length, 1);
  assert.ok(Object.prototype.hasOwnProperty.call(received[0], "KeyPressed"));
});

test("a handler's thrown exception surfaces out of run", () => {
  const app = newShell();
  app.setHandler(() => {
    throw new Error("boom");
  });
  app.pushKeyEvent("x".codePointAt(0));

  assert.throws(() => app.run(1), /boom/);
});

test("runShell rejects a zero width or height", () => {
  assert.throws(() => QuiApp.runShell(0, 24, CONFIG));
  assert.throws(() => QuiApp.runShell(80, 0, CONFIG));
});

test("runShell rejects a malformed config", () => {
  assert.throws(() => QuiApp.runShell(80, 24, { title: "Demo", panels: "not an array" }));
});

test("runShell rejects a config missing the required title", () => {
  assert.throws(() => QuiApp.runShell(80, 24, { panels: [] }));
});

test("bottomPanelTabCenter is null without a bottom_panel config", () => {
  const app = QuiApp.runShell(80, 24, { title: "Demo", panels: [] });
  assert.equal(app.bottomPanelTabCenter(0), null);
});
