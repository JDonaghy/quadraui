// Black-box coverage for `js/app/runtime.ts`'s hot reload: an app whose
// view code lives in a real module on disk keeps running while that file
// is rewritten, picks the new code up, and keeps component state, the
// state on a preserved app object, and framework widget state; a broken
// edit leaves the last good code running. Mirrors
// `quadraui-py/tests/test_hot_reload.py`.
//
// The app is driven through `../js/testing.ts` -- the app-author test
// harness -- so a click is a click and a keypress is a keypress, with no
// wire event spelled out here. Only the reload calls themselves go
// straight to `App`: `enableHotReload`/`reloadIfChanged` are the app's own
// lifecycle API, not something a user does to a widget.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { App } = require("../js/app/index.ts");
const { harness } = require("../js/testing.ts");

const createdTmpDirs: string[] = [];
const builtHarnesses: any[] = [];
after(() => {
  // `close()` stops a watcher timer the test enabled, so no suite below
  // leaves one running for the rest of the process.
  for (const built of builtHarnesses) {
    built.close();
  }
  for (const dir of createdTmpDirs) {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

/** `harness(app)`, registered for the `close()` the `after` hook above runs. */
function wrap(app: any): any {
  const built = harness(app);
  builtHarnesses.push(built);
  return built;
}

/** A throwaway module written to a temp dir and required for real. */
class Fixture {
  dir: string;
  path: string;
  private tick = 0;

  constructor(source: string) {
    this.dir = fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-hot-reload-"));
    createdTmpDirs.push(this.dir);
    this.path = path.join(this.dir, "fixture_view.ts");
    this.write(source);
  }

  write(source: string): void {
    fs.writeFileSync(this.path, source);
    // A distinct mtime per write, so change detection never depends on the
    // filesystem's timestamp granularity.
    this.tick += 10;
    const stamp = new Date(Date.now() + this.tick * 1000);
    fs.utimesSync(this.path, stamp, stamp);
  }

  get exports(): any {
    return require(this.path);
  }
}

const APP_DIR = path.join(__dirname, "..", "js", "app", "index.ts");

const COUNTER_V1 = `
const { Button, Column, Component, TextDisplay } = require(${JSON.stringify(APP_DIR)});

function line(text) {
  return { spans: [{ text, fg: null, bg: null, bold: false, italic: false, underline: false }] };
}

class Counter extends Component {
  initialState() {
    return { count: 0 };
  }
  view(state) {
    return Column([
      TextDisplay([line("v1 count=" + state.count)], { id: "t:text" }),
      Button("Plus", { id: "b:plus", onClick: () => { state.count += 1; } }),
    ]);
  }
}
exports.Counter = Counter;

class Shell {
  constructor() {
    this.title = "first";
  }
  view() {
    return Column([TextDisplay([line("title=" + this.title)], { id: "t:title" }), new Counter()]);
  }
}
exports.Shell = Shell;
`;

const COUNTER_V2 = COUNTER_V1.replace("v1 count=", "v2 count=")
  .replace("return { count: 0 };", "return { count: 0, step: 5 };")
  .replace('"v2 count=" + state.count', '"v2 count=" + state.count + " step=" + state.step')
  .replace('"title=" + this.title', '"new title=" + this.title');

/** Activates a button the way a keyboard user does: focus it, press Enter. */
function click(ui: any, widget: string): void {
  ui.focus(widget);
  ui.press("Enter");
}

function build(fixture: Fixture, options: any = {}): [any, any] {
  const shell = new fixture.exports.Shell();
  const app = new App(60, 10, { view: () => shell.view() });
  app.enableHotReload([fixture.path], { interval: null, preserve: [shell], ...options });
  return [wrap(app), shell];
}

test("reload picks up the edited view and keeps component and app-object state", () => {
  const fixture = new Fixture(COUNTER_V1);
  const [ui, shell] = build(fixture);
  assert.ok(ui.contains("v1 count=0"));

  click(ui, "b:plus");
  click(ui, "b:plus");
  shell.title = "edited-at-runtime";
  ui.step();
  assert.ok(ui.contains("v1 count=2"), ui.screen());

  fixture.write(COUNTER_V2);
  assert.equal(ui.app.reloadIfChanged(), true);
  ui.step();

  // New code renders, the component's count survived, the key the new
  // initialState() introduced was merged in, and the preserved object kept
  // its property while running the new method body.
  assert.ok(ui.contains("v2 count=2 step=5"), ui.screen());
  assert.ok(ui.contains("new title=edited-at-runtime"), ui.screen());
  assert.equal(Object.getPrototypeOf(shell), fixture.exports.Shell.prototype);
  assert.equal(ui.app.reloadError, null);
  assert.equal(ui.app.reloadCount, 1);

  // Callbacks registered by the new code still drive the same state.
  click(ui, "b:plus");
  assert.ok(ui.contains("v2 count=3 step=5"), ui.screen());
});

test("reloadIfChanged is a no-op without an edit", () => {
  const fixture = new Fixture(COUNTER_V1);
  const [ui] = build(fixture);

  assert.equal(ui.app.reloadIfChanged(), false);
  assert.equal(ui.app.reloadCount, 0);
});

test("a syntax error keeps the last good code running", () => {
  const fixture = new Fixture(COUNTER_V1);
  const [ui] = build(fixture);
  click(ui, "b:plus");

  fixture.write(`${COUNTER_V1}\nfunction broken( {\n`);
  assert.equal(ui.app.reloadIfChanged(), false);
  assert.ok(ui.app.reloadError instanceof Error);
  ui.step();
  assert.ok(ui.contains("v1 count=1"), ui.screen());
  click(ui, "b:plus");
  assert.ok(ui.contains("v1 count=2"), ui.screen());

  // Fixing the file recovers and clears the error, state intact.
  fixture.write(COUNTER_V2);
  assert.equal(ui.app.reloadIfChanged(), true);
  ui.step();
  assert.equal(ui.app.reloadError, null);
  assert.ok(ui.contains("v2 count=2 step=5"), ui.screen());
});

test("a view that throws after a reload rolls back to the previous code", () => {
  const fixture = new Fixture(COUNTER_V1);
  const [ui] = build(fixture);
  click(ui, "b:plus");

  fixture.write(COUNTER_V1.replace("  view() {\n", '  view() {\n    throw new Error("boom");\n'));
  assert.equal(ui.app.reloadIfChanged(), true);
  ui.step();

  assert.ok(ui.contains("v1 count=1"), ui.screen());
  assert.ok(ui.app.reloadError instanceof Error);
  assert.ok(ui.contains("title=first"), ui.screen());
  click(ui, "b:plus");
  assert.ok(ui.contains("v1 count=2"), ui.screen());
});

test("framework widget state survives a reload", () => {
  const source = `
const { Tree } = require(${JSON.stringify(APP_DIR)});

function row(i, label) {
  return { path: [i], indent: 0, icon: null,
           text: { spans: [{ text: label + " row " + i, fg: null, bg: null,
                             bold: false, italic: false, underline: false }] },
           badge: null, is_expanded: false };
}

exports.view = () => Tree([0, 1, 2].map((i) => row(i, "%LABEL%")), { id: "t:tree" });
`;
  const fixture = new Fixture(source.replace("%LABEL%", "old"));
  const app = new App(40, 6, { view: () => fixture.exports.view() });
  app.enableHotReload([fixture.path], { interval: null });
  const ui = wrap(app);
  assert.ok(ui.contains("old row 0"));

  ui.find({ id: "t:tree" }).row("old row 2").click();
  const before = ui.state("t:tree");

  fixture.write(source.replace("%LABEL%", "new"));
  assert.equal(ui.app.reloadIfChanged(), true);
  ui.step();

  assert.ok(ui.contains("new row 2"), ui.screen());
  assert.deepEqual(ui.state("t:tree"), before);
});

test("the onReload hook runs after a successful reload", () => {
  const fixture = new Fixture(COUNTER_V1);
  const seen: number[] = [];
  const [ui] = build(fixture, { onReload: (app: any) => seen.push(app.reloadCount) });

  fixture.write(COUNTER_V2);
  ui.app.reloadIfChanged();

  assert.deepEqual(seen, [0]);
});

test("the watcher timer wakes the event loop on an edit", async () => {
  const fixture = new Fixture(COUNTER_V1);
  const shell = new fixture.exports.Shell();
  const unclaimed: unknown[] = [];
  const app = new App(60, 10, { view: () => shell.view(), onEvent: (event: unknown) => unclaimed.push(event) });
  app.enableHotReload([fixture.path], { interval: 20, preserve: [shell] });
  const ui = wrap(app);
  try {
    fixture.write(COUNTER_V2);
    const deadline = Date.now() + 5000;
    while (app.reloadCount === 0 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 10));
      app.run(1);
    }
    assert.equal(app.reloadCount, 1);
    assert.ok(ui.contains("v2 count=0 step=5"), ui.screen());
    assert.deepEqual(unclaimed, [], "the reload wake must not reach onEvent");
  } finally {
    app.disableHotReload();
  }
});

test("an unresolvable watch path is rejected with a clear error", () => {
  const app = new App(10, 3, { view: () => null });

  assert.throws(() => app.enableHotReload(["./definitely-not-a-module"]), /cannot hot-reload/);
  assert.throws(() => app.enableHotReload([{} as any]), /not a module path/);
  assert.throws(() => app.reloadIfChanged(), /enableHotReload\(\) has not been called/);
});

// --- examples/hot_reload_demo.ts -----------------------------------------

/** Copies the demo and its view module into a temp dir (so the test can rewrite the view) and requires the copy. */
function loadDemo(): [any, string] {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-hot-reload-demo-"));
  createdTmpDirs.push(dir);
  const examples = path.join(__dirname, "..", "examples");
  fs.mkdirSync(path.join(dir, "hot_reload"));
  // The copies live two directories deeper than the originals did relative
  // to `js/`, so their own `require("../../js/...")` paths are rewritten to
  // absolute ones rather than left to resolve against the temp dir.
  const jsDir = path.join(__dirname, "..", "js");
  const rewrite = (source: string) =>
    source
      .replaceAll('require("../../js/', `require(${JSON.stringify(jsDir)} + "/`)
      .replaceAll('require("../js/', `require(${JSON.stringify(jsDir)} + "/`);
  fs.writeFileSync(
    path.join(dir, "hot_reload", "notes_view.ts"),
    rewrite(fs.readFileSync(path.join(examples, "hot_reload", "notes_view.ts"), "utf8")),
  );
  fs.writeFileSync(
    path.join(dir, "hot_reload_demo.ts"),
    rewrite(fs.readFileSync(path.join(examples, "hot_reload_demo.ts"), "utf8")),
  );
  return [require(path.join(dir, "hot_reload_demo.ts")), path.join(dir, "hot_reload", "notes_view.ts")];
}

function bumpMtime(file: string): void {
  const stamp = new Date(Date.now() + 30000);
  fs.utimesSync(file, stamp, stamp);
}

test("a demo edit repaints and keeps the counter, notes and selection", () => {
  const [demo, viewFile] = loadDemo();
  const [app] = demo.build(60, 24, null);
  const ui = wrap(app);
  try {
    assert.ok(ui.contains("Hot reload demo"));
    click(ui, "hr:add");
    click(ui, "hr:add");
    click(ui, "hr:note");
    ui.find({ id: "hr:notes" }).row({ index: 1 }).click();
    assert.ok(ui.contains("Counter: 2"), ui.screen());
    assert.ok(ui.contains("note 4"), ui.screen());
    const selection = ui.state("hr:notes");

    fs.writeFileSync(viewFile, fs.readFileSync(viewFile, "utf8").replace('"Hot reload demo"', '"Edited title"'));
    bumpMtime(viewFile);
    assert.equal(app.reloadIfChanged(), true);
    ui.step();

    assert.ok(ui.contains("Edited title"), ui.screen());
    assert.ok(ui.contains("Counter: 2"), ui.screen());
    assert.ok(ui.contains("note 4"), ui.screen());
    assert.ok(ui.contains("reloads: 1"), ui.screen());
    assert.deepEqual(ui.state("hr:notes"), selection);
  } finally {
    app.disableHotReload();
  }
});

test("the demo shows a broken edit in the status bar", () => {
  const [demo, viewFile] = loadDemo();
  const [app] = demo.build(60, 24, null);
  const ui = wrap(app);
  try {
    click(ui, "hr:add");
    fs.writeFileSync(viewFile, `${fs.readFileSync(viewFile, "utf8")}\nfunction broken( {\n`);
    bumpMtime(viewFile);
    assert.equal(app.reloadIfChanged(), false);
    ui.step();
    assert.ok(ui.contains("reload failed"), ui.screen());
    assert.ok(ui.contains("Counter: 1"), ui.screen());
  } finally {
    app.disableHotReload();
  }
});
