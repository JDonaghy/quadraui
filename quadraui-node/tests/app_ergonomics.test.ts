// `App.quit()`, the default quit keys, the optional `App` size, `Text` and
// `<text>`, optional `fg`/`bg` on a span, and per-child `size`/`min`/`max`
// on `Column`/`Row` children. Mirrors
// `quadraui-py/tests/test_app_ergonomics.py` case for case.
//
// Run with `npm run build:debug` (or `npm run build`) then `npm test`.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { App, Button, Column, Form, List, Row, Text, TextDisplay, TextInputField } = require("@quadraui/node/app");
const { run } = require("@quadraui/node");
const { harness } = require("@quadraui/node/testing");
const { load } = require("../../examples/hero/node/load_jsx.ts");

const NO_MODS = { shift: false, ctrl: false, alt: false, cmd: false };

function key(name: string, mods: Record<string, boolean> = {}) {
  const k = name.length > 1 ? { Named: name } : { Char: name };
  return { KeyPressed: { key: k, modifiers: { ...NO_MODS, ...mods }, repeat: false } };
}

function counterApp(options: Record<string, unknown> = {}) {
  const clicks: number[] = [];
  const app = new App(20, 4, {
    view: () =>
      Column([
        Button("Quit", { id: "quit", onClick: () => app.quit() }),
        Button("Count", { id: "count", onClick: () => clicks.push(1) }),
      ]),
    ...options,
  });
  return { app, clicks };
}

const line = (text: string) => ({ spans: [{ text }] });

// --- App.quit() ---------------------------------------------------------------

test("quit() from a node callback ends the step that ran it", () => {
  const { app } = counterApp();
  harness(app).find({ id: "quit" }).click();
  assert.equal(app.step(), false);
});

test("quit() from onEvent ends the step that ran it", () => {
  const app = new App(20, 4, {
    view: () => Text("hi"),
    onEvent: (event: unknown) => {
      if ((event as { User?: unknown }).User === "stop") app.quit();
    },
  });
  assert.equal(app.step(), true);
  app.qui.waker().post("stop");
  assert.equal(app.step(), false);
});

test("quit() outside any callback ends the next step", () => {
  const { app } = counterApp();
  assert.equal(app.step(), true);
  app.quit();
  assert.equal(app.step(), false);
});

test("stopLive() inside onEvent quits instead of throwing", () => {
  const app = new App(20, 4, {
    view: () => Text("hi"),
    onEvent: (event: unknown) => {
      if ((event as { User?: unknown }).User === "stop") app.stopLive();
    },
  });
  app.qui.waker().post("stop");
  assert.equal(app.step(), false);
});

test("run(app) stops its loop once a callback quits", async () => {
  const { app } = counterApp();
  let steps = 0;
  const counted = {
    step: () => {
      steps += 1;
      return app.step();
    },
    stopLive: () => app.stopLive(),
    armWakeNotify: (callback: () => void) => app.armWakeNotify(callback),
    disarmWakeNotify: () => app.disarmWakeNotify(),
  };
  run(counted, 5);
  app.quit();
  await new Promise((resolve) => setTimeout(resolve, 100));
  const settled = steps;
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(steps, settled, "the loop kept stepping after quit()");
});

test("a running app keeps running without quit()", () => {
  const { app, clicks } = counterApp();
  harness(app).find({ id: "count" }).click();
  assert.deepEqual(clicks, [1]);
  assert.equal(app.step(), true);
});

// --- default quit keys ----------------------------------------------------------

for (const [label, event] of [
  ["q", key("q")],
  ["Escape", key("Escape")],
  ["Ctrl-C", key("c", { ctrl: true })],
] as const) {
  test(`${label} quits by default`, () => {
    const { app } = counterApp();
    assert.equal(app.step(), true);
    app.pushEvent(event);
    assert.equal(app.step(), false);
  });
}

test("Ctrl-C quits even inside a focused widget", () => {
  const app = new App(20, 6, { view: () => List([{ text: line("a") }], { id: "items" }) });
  app.pushEvent({ List: ["items", { KeyPressed: { key: "c", modifiers: { ctrl: true } } }] });
  assert.equal(app.step(), false);
});

test("q consumed by a focused widget does not quit", () => {
  const app = new App(20, 6, { view: () => List([{ text: line("a") }], { id: "items" }) });
  app.pushEvent({ List: ["items", { KeyPressed: { key: "q", modifiers: {} } }] });
  assert.equal(app.step(), true);
});

function draftFormApp() {
  const draft = { text: "" };
  const app = new App(30, 4, {
    view: () =>
      Form([TextInputField("New", draft.text, { id: "draft", onChange: (t: string) => (draft.text = t) })], { id: "form" }),
  });
  const h = harness(app);
  h.clickAt(10, 0);
  return { app, h, draft };
}

test("q typed into a focused text field edits it and does not quit", () => {
  const { app, h, draft } = draftFormApp();
  h.type("aq");
  assert.equal(draft.text, "aq");
  assert.equal(app.step(), true);
});

test("Escape a focused text field leaves unconsumed still quits", () => {
  const { app, h } = draftFormApp();
  h.press("Escape");
  assert.equal(app.step(), false);
});

test("other keys and modified q do not quit", () => {
  const { app } = counterApp();
  for (const event of [key("x"), key("Enter"), key("q", { alt: true }), key("q", { ctrl: true })]) {
    app.pushEvent(event);
    assert.equal(app.step(), true, JSON.stringify(event));
  }
});

test("onEvent returning true keeps a quit key", () => {
  const seen: unknown[] = [];
  const app = new App(20, 4, {
    view: () => Text("hi"),
    onEvent: (event: unknown) => {
      seen.push(event);
      return true;
    },
  });
  app.pushEvent(key("q"));
  assert.equal(app.step(), true);
  assert.deepEqual(seen.at(-1), key("q"));
});

test("onEvent sees a quit key before the app quits", () => {
  const seen: unknown[] = [];
  const app = new App(20, 4, { view: () => Text("hi"), onEvent: (event: unknown) => void seen.push(event) });
  app.pushEvent(key("Escape"));
  assert.equal(app.step(), false);
  assert.deepEqual(seen.at(-1), key("Escape"));
});

test("quitKeys: false turns every default quit key off", () => {
  const { app } = counterApp({ quitKeys: false });
  for (const event of [key("q"), key("Escape"), key("c", { ctrl: true })]) {
    app.pushEvent(event);
    assert.equal(app.step(), true, JSON.stringify(event));
  }
});

// --- optional size ---------------------------------------------------------------

test("new App({view}) defaults to 80x24", () => {
  const app = new App({ view: () => Text("hi") });
  const rows = app.screen().replace(/\n$/, "").split("\n");
  assert.equal(rows.length, 24);
  assert.ok(rows.every((row: string) => row.length === 80));
});

// --- Text, <text>, optional fg/bg -----------------------------------------------

function wire(node: unknown) {
  const app = new App(20, 4, { view: () => Column([node]) });
  return (app._render() as { column: Record<string, unknown>[] }).column[0];
}

test("Text builds a text_display with one plain line per line, sized to content", () => {
  const built = wire(Text("one\ntwo", { id: "t" }));
  assert.deepEqual((built.text_display as { lines: unknown }).lines, [line("one"), line("two")]);
  assert.equal(built.size, "Content");
});

test("Text renders and takes one row in a column", () => {
  const app = new App(20, 6, { view: () => Column([Text("Count: 3"), Text("below", { size: "EqualShare" })]) });
  const rows = app.screen().split("\n");
  assert.match(rows[0], /Count: 3/);
  assert.match(rows[1], /below/);
});

test("<text> joins its children into one Text, and spans need no fg/bg", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "qu-text-jsx-"));
  const file = path.join(dir, "label.tsx");
  fs.writeFileSync(
    file,
    [
      "const n = 3;",
      'module.exports.view = () => (<column><text id="label">Count: {n}</text><textDisplay id="plain" lines={[{ spans: [{ text: "no colours" }] }]} /></column>);',
    ].join("\n"),
  );
  try {
    const { view } = load(file);
    const app = new App(20, 6, { view });
    const rows = app.screen().split("\n");
    assert.match(rows[0], /Count: 3/);
    assert.match(app.screen(), /no colours/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

// --- per-child size / min / max -------------------------------------------------
// A number size, `min` and `max` are portable pixels; a TUI cell is 8 px wide
// and 16 px tall.

function rowOf(screen: string, needle: string): number {
  return screen.split("\n").findIndex((row) => row.includes(needle));
}

test("Column children share space equally by default", () => {
  const app = new App(20, 8, { view: () => Column([TextDisplay([line("top")]), TextDisplay([line("bottom")])]) });
  assert.equal(rowOf(app.screen(), "bottom"), 4);
});

test("a fixed size child takes exactly that many pixels", () => {
  const app = new App(20, 8, { view: () => Column([TextDisplay([line("top")], { size: 32 }), TextDisplay([line("bottom")])]) });
  assert.equal(rowOf(app.screen(), "bottom"), 2);
});

test("max clamps an equal share child", () => {
  const app = new App(20, 8, { view: () => Column([TextDisplay([line("top")], { max: 48 }), TextDisplay([line("bottom")])]) });
  assert.equal(rowOf(app.screen(), "bottom"), 3);
});

test("min floors a child", () => {
  const app = new App(20, 8, { view: () => Column([Text("top", { min: 80 }), TextDisplay([line("bottom")])]) });
  assert.equal(rowOf(app.screen(), "bottom"), 5);
});

test("Row children take their size along the width", () => {
  const app = new App(20, 3, { view: () => Row([Text("ab", { size: 32 }), Text("cd", { size: "EqualShare" })]) });
  assert.equal(app.screen().split("\n")[0].indexOf("cd"), 4);
});

test("a content-sized row takes its tallest child's height", () => {
  const app = new App(20, 6, { view: () => Column([Row([Text("ab"), Text("cd")], { size: "content" }), Text("below", { size: "EqualShare" })]) });
  assert.equal(rowOf(app.screen(), "ab"), 0);
  assert.equal(rowOf(app.screen(), "cd"), 0);
  assert.equal(rowOf(app.screen(), "below"), 1);
});

test("content-sized Text in a row shows its whole line", () => {
  const app = new App(30, 2, { view: () => Row([Text("Name:"), Text("Ada Lovelace")]) });
  const first = app.screen().split("\n")[0];
  assert.ok(first.includes("Name:"), first);
  assert.ok(first.includes("Ada Lovelace"), first);
});

test("a content-sized column adds its children and gaps", () => {
  const app = new App(20, 8, { view: () => Column([Column([Text("a"), Text("b")], { gap: 16, size: "content" }), Text("below", { size: "EqualShare" })]) });
  assert.equal(rowOf(app.screen(), "b"), 2);
  assert.equal(rowOf(app.screen(), "below"), 3);
});

test("<button size> in JSX reaches the wire", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "qu-size-jsx-"));
  const file = path.join(dir, "sized.tsx");
  fs.writeFileSync(file, 'module.exports.view = () => (<column><button id="b" label="b" size="2fr" min={16} /></column>);');
  try {
    const { view } = load(file);
    const built = wire(view());
    const child = (built.column as Record<string, unknown>[])[0];
    assert.deepEqual([child.size, child.min], [{ Weight: 2 }, 16]);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

for (const [size, expected] of [
  [3, { Fixed: 3 }],
  ["30%", { Percent: 0.3 }],
  ["2fr", { Weight: 2 }],
  ["content", "Content"],
  [{ Weight: 1.5 }, { Weight: 1.5 }],
] as const) {
  test(`size ${JSON.stringify(size)} maps to the wire SectionSize`, () => {
    const built = wire(Button("b", { id: "b", size, min: 1, max: 9 }));
    assert.deepEqual([built.size, built.min, built.max], [expected, 1, 9]);
  });
}

for (const size of ["wide", "x%", 1.5, true]) {
  test(`an unknown size spelling ${JSON.stringify(size)} throws`, () => {
    assert.throws(() => Button("b", { size: size as never }), TypeError);
  });
}

test("a child without size carries no layout keys", () => {
  assert.deepEqual(Object.keys(wire(Button("b", { id: "b" }))).sort(), ["button", "id"]);
});

test("a padded content-sized row keeps its children", () => {
  const app = new App(30, 8, {
    view: () =>
      Column([
        Row([Button("ok", { id: "ok", size: "content" })], { padding: 16, size: "content" }),
        Text("below", { size: "EqualShare" }),
      ]),
  });
  const screen = app.screen();
  assert.ok(screen.includes("[ ok ]"), screen);
  assert.ok(rowOf(screen, "[ ok ]") >= 1, screen);
  assert.ok(rowOf(screen, "below") > rowOf(screen, "[ ok ]") + 1, screen);
});

test("Ctrl-C typed into a focused text field is the field's and does not quit", () => {
  const { app, h } = draftFormApp();
  h.press("c", { ctrl: true });
  assert.equal(app.step(), true);
});

test("focusing a form sends typed keys to its field, not the last-clicked button", () => {
  const draft = { text: "" };
  const clicks: number[] = [];
  const app = new App(30, 6, {
    view: () =>
      Column([
        Button("Go", { id: "go", size: "content", onClick: () => clicks.push(1) }),
        Form([TextInputField("New", draft.text, { id: "draft", onChange: (t: string) => (draft.text = t) })], {
          id: "form",
          size: "content",
        }),
      ]),
  });
  const h = harness(app);
  h.clickText("Go");
  h.find({ id: "form" }).focus();
  h.type("hi");
  h.press("Enter");
  assert.equal(draft.text, "hi");
  assert.deepEqual(clicks, [1]);
});

test("a Ctrl-click after arrow keys grows the selection from the cursor row", () => {
  const picked: number[][] = [];
  const items = [0, 1, 2, 3].map((i) => ({ text: { spans: [{ text: `item${i}` }] } }));
  const app = new App(24, 6, { view: () => List(items, { id: "items", onSelection: (s: number[]) => picked.push(s) }) });
  const h = harness(app);
  h.clickText("item0");
  h.press("Down");
  h.press("Down");
  const [x, y] = h.locateAll("item3")[0];
  const position = { x, y };
  h.dispatch({ MouseDown: { button: "Left", position, modifiers: { ...NO_MODS, ctrl: true } } });
  h.dispatch({ MouseUp: { button: "Left", position } });
  assert.deepEqual(picked[picked.length - 1], [2, 3]);
});
