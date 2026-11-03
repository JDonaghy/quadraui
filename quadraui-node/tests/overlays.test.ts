// Black-box coverage for the in-canvas modal-overlay primitives wired onto
// `InteractionEngine`'s own anchored-overlay slot (the one
// `showMessageDialog`/`showContextMenu` already use, covered in
// `qui_app.test.ts`/`callbacks.test.ts`): `showToast`/`dismissToast`,
// `showTooltip`/`hideTooltip`, `showCompletions`/`dismissCompletions`,
// `showFloat`/`dismissFloat`, and
// `showRichTextPopup`/`dismissRichTextPopup` -- a port of
// `quadraui-py/tests/test_overlay_primitives.py`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

function locate(app: any, needle: string): [number, number] {
  const lines = app.screen().split("\n");
  for (let row = 0; row < lines.length; row++) {
    const col = lines[row].indexOf(needle);
    if (col !== -1) {
      return [row, col];
    }
  }
  throw new Error(`${JSON.stringify(needle)} not found on screen`);
}

function click(app: any, row: number, col: number) {
  app.pushEvent({
    MouseDown: {
      button: "Left",
      position: { x: col, y: row },
      modifiers: {},
    },
  });
  app.run(1);
}

function escape(app: any) {
  app.pushEvent({
    KeyPressed: { key: { Named: "Escape" }, modifiers: {}, repeat: false },
  });
  app.run(1);
}

// ── Toast ────────────────────────────────────────────────────────────────

test("showToast paints over base content and action click routes", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.insertButton({ id: "ok", label: "OK" });
  assert.ok(app.screenContains("OK"));

  app.showToast({
    id: "toast:saved",
    title: "Saved",
    body: "",
    actions: [{ id: "undo", label: "Undo" }],
  });
  assert.ok(app.screenContains("OK"));
  assert.ok(app.screenContains("Saved"));
  assert.ok(app.screenContains("Undo"));

  const [row, col] = locate(app, "Undo");
  click(app, row, col);

  assert.deepEqual(received, [{ ToastActionClicked: "undo" }]);
  assert.ok(app.screenContains("Saved"));
});

test("toast dismiss click closes with no event", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showToast({ id: "toast:saved", title: "Saved", body: "" });
  const [row, col] = locate(app, "×");

  click(app, row, col);

  assert.deepEqual(received, []);
  assert.ok(!app.screenContains("Saved"));
});

test("dismissToast by id closes without input", () => {
  const app = new QuiApp(40, 20);
  app.showToast({ id: "toast:a", title: "A", body: "" });
  app.showToast({ id: "toast:b", title: "B", body: "" });
  assert.ok(app.screenContains("A"));
  assert.ok(app.screenContains("B"));

  assert.equal(app.dismissToast("toast:a"), true);
  assert.ok(!app.screenContains("A"));
  assert.ok(app.screenContains("B"));

  assert.equal(app.dismissToast("toast:a"), false);
});

test("showToast rejects a malformed descriptor", () => {
  const app = new QuiApp(40, 20);
  assert.throws(
    () => app.showToast({ id: "toast:bad" }),
    /InvalidDescriptorError/,
  );
});

// ── Tooltip ──────────────────────────────────────────────────────────────

test("showTooltip paints over base content then auto-dismisses on click", () => {
  const app = new QuiApp(40, 20);
  app.setHandler(() => undefined);

  app.insertButton({ id: "ok", label: "OK" });
  app.showTooltip(
    { id: "tip:hint", text: "Hello tip" },
    { x: 0.0, y: 2.0, width: 10.0, height: 1.0 },
  );

  assert.ok(app.screenContains("OK"));
  assert.ok(app.screenContains("Hello tip"));

  click(app, 0, 0);

  assert.ok(!app.screenContains("Hello tip"));
});

test("hideTooltip closes explicitly", () => {
  const app = new QuiApp(40, 20);
  app.showTooltip(
    { id: "tip:hint", text: "Hello tip" },
    { x: 0.0, y: 2.0, width: 10.0, height: 1.0 },
  );
  assert.ok(app.screenContains("Hello tip"));

  assert.equal(app.hideTooltip(), true);
  assert.ok(!app.screenContains("Hello tip"));
  assert.equal(app.hideTooltip(), false);
});

// ── Completions ────────────────────────────────────────────────────────

function completionItems() {
  return [
    { label: { spans: [{ text: "map", fg: null, bg: null }] } },
    { label: { spans: [{ text: "filter", fg: null, bg: null }] } },
  ];
}

test("showCompletions then clicking an item activates it", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showCompletions(
    { id: "completions:editor", selected_idx: 0, items: completionItems() },
    { x: 2.0, y: 2.0 },
  );
  assert.ok(app.screenContains("map"));
  assert.ok(app.screenContains("filter"));

  const [row, col] = locate(app, "filter");
  click(app, row, col);

  assert.deepEqual(received, [
    { CompletionItemActivated: ["completions:editor", 1] },
  ]);
  assert.ok(!app.screenContains("filter"));
});

test("completions escape dismisses without activation", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showCompletions(
    { id: "completions:editor", selected_idx: 0, items: completionItems() },
    { x: 2.0, y: 2.0 },
  );
  escape(app);

  assert.deepEqual(received, []);
  assert.ok(!app.screenContains("map"));
});

test("dismissCompletions closes without input", () => {
  const app = new QuiApp(40, 20);
  app.showCompletions(
    { id: "completions:editor", selected_idx: 0, items: completionItems() },
    { x: 2.0, y: 2.0 },
  );
  assert.equal(app.dismissCompletions(), true);
  assert.ok(!app.screenContains("map"));
  assert.equal(app.dismissCompletions(), false);
});

// ── Float ────────────────────────────────────────────────────────────────

function anchor(x: number, y: number, width = 1.0, height = 1.0, side = "Bottom") {
  return {
    rect: { x, y, width, height },
    preferred: side,
    margin: 0.0,
  };
}

test("showFloat paints chrome then escape dismisses", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.insertButton({ id: "ok", label: "OK" });
  app.showFloat({ id: "float:hints", anchor: anchor(2.0, 2.0) }, 10.0, 4.0);

  assert.ok(app.screenContains("OK"));
  assert.ok(app.screenContains("┌"));

  escape(app);

  assert.deepEqual(received, []);
  assert.ok(!app.screenContains("┌"));
});

test("dismissFloat closes without input", () => {
  const app = new QuiApp(40, 20);
  app.showFloat({ id: "float:hints", anchor: anchor(2.0, 2.0) }, 10.0, 4.0);
  assert.equal(app.dismissFloat(), true);
  assert.equal(app.dismissFloat(), false);
});

test("a non-focusable float does not claim escape", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showFloat(
    { id: "float:hint", anchor: anchor(2.0, 2.0), focusable: false },
    10.0,
    4.0,
  );

  escape(app);

  assert.ok(received.some((event) => typeof event === "object" && event !== null && "KeyPressed" in (event as object)));
  assert.ok(app.screenContains("┌"));
});

// ── RichTextPopup ────────────────────────────────────────────────────────

test("showRichTextPopup then clicking a link routes", () => {
  const app = new QuiApp(40, 20);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));

  app.showRichTextPopup(
    {
      id: "popup:hover",
      lines: [{ spans: [{ text: "See docs", fg: null, bg: null }] }],
      line_text: ["See docs"],
      max_visible_rows: 3,
      links: [{ line: 0, start_byte: 4, end_byte: 8, url: "https://example.com" }],
    },
    { x: 2.0, y: 2.0 },
  );
  assert.ok(app.screenContains("See docs"));

  const [row, col] = locate(app, "docs");
  click(app, row, col);

  assert.deepEqual(received, [
    { RichTextLinkClicked: ["popup:hover", "https://example.com"] },
  ]);
});

test("rich text popup escape dismisses", () => {
  const app = new QuiApp(40, 20);
  app.setHandler(() => undefined);
  app.showRichTextPopup(
    {
      id: "popup:hover",
      lines: [{ spans: [{ text: "Hello", fg: null, bg: null }] }],
      line_text: ["Hello"],
      max_visible_rows: 3,
    },
    { x: 2.0, y: 2.0 },
  );
  assert.ok(app.screenContains("Hello"));

  escape(app);

  assert.ok(!app.screenContains("Hello"));
});

test("dismissRichTextPopup closes without input", () => {
  const app = new QuiApp(40, 20);
  app.showRichTextPopup(
    {
      id: "popup:hover",
      lines: [{ spans: [{ text: "Hello", fg: null, bg: null }] }],
      line_text: ["Hello"],
      max_visible_rows: 3,
    },
    { x: 2.0, y: 2.0 },
  );
  assert.equal(app.dismissRichTextPopup(), true);
  assert.equal(app.dismissRichTextPopup(), false);
});
