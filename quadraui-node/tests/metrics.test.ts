// Black-box coverage for `QuiApp`'s metrics/capabilities surface:
// `viewport`, `lineHeight`, `charWidth`, `listCharWidth`,
// `chromeLineHeight`, `scrollbarReserve`, `scalesTextRows`, `snapHeight`,
// `measureText`, `backendCaps`, and `cellColors` -- a port of
// `quadraui-py/tests/test_metrics.py`. All of these read the headless
// `TuiBackend` this `QuiApp` wraps, so the values asserted here are the
// fixed-cell-grid truth: a terminal cell is `1.0` wide and `1.0` tall by
// construction, no overlay scrollbar chrome, no per-row font scaling.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

test("viewport matches the constructor dimensions", () => {
  const app = new QuiApp(42, 17);
  assert.deepEqual(app.viewport(), [42, 17, 1]);
});

test("viewport follows a resize once rendered", () => {
  const app = new QuiApp(10, 10);
  app.resize(30, 8);
  app.screen();
  assert.deepEqual(app.viewport(), [30, 8, 1]);
});

test("lineHeight and charWidth are one terminal cell", () => {
  const app = new QuiApp(10, 10);
  assert.equal(app.lineHeight(), 1.0);
  assert.equal(app.charWidth(), 1.0);
  assert.equal(app.listCharWidth(), 1.0);
  assert.equal(app.chromeLineHeight(), 1.0);
});

test("scrollbarReserve and scalesTextRows are tui defaults", () => {
  const app = new QuiApp(10, 10);
  assert.equal(app.scrollbarReserve(), 0.0);
  assert.equal(app.scalesTextRows(), false);
});

test("snapHeight rounds to a whole cell", () => {
  const app = new QuiApp(10, 10);
  assert.equal(app.snapHeight(2.3), 2.0);
  assert.equal(app.snapHeight(2.7), 3.0);
});

test("measureText counts display width in cells", () => {
  const app = new QuiApp(10, 10);
  const [width, height] = app.measureText("hello");
  assert.deepEqual([width, height], [5.0, 1.0]);
  // The default role ("chrome") and the explicit "editor" role measure the
  // same on a terminal, which has one font by definition.
  assert.deepEqual(app.measureText("hello", "editor"), [width, height]);
});

test("measureText rejects an unknown role", () => {
  const app = new QuiApp(10, 10);
  assert.throws(() => app.measureText("hello", "bogus"), /InvalidDescriptorError/);
});

test("backendCaps reports the headless tui vocabulary", () => {
  const app = new QuiApp(10, 10);
  const caps = app.backendCaps();
  // Input-delivery capabilities are real on the headless driver.
  assert.equal(caps.mouse, true);
  assert.equal(caps.scroll, true);
  assert.equal(caps.drag, true);
  // No OS chrome / native dialogs on a terminal.
  assert.equal(caps.native_menu, false);
  assert.equal(caps.window_chrome, false);
  assert.equal(caps.file_dialogs, false);
  // The three fields `BackendCaps::vocabulary` excludes are still present.
  assert.equal(typeof caps.kitty_keyboard, "boolean");
  assert.equal(typeof caps.sgr_pixel_mouse, "boolean");
  assert.ok(["ansi16", "indexed256", "true_color"].includes(caps.color_depth));
});

test("cellColors reads back the painted RGB colours at a cell", () => {
  const app = new QuiApp(10, 3);
  app.insertButton({ id: "ok", label: "OK" });
  // Outside the viewport -> null, not an error.
  assert.equal(app.cellColors(100, 100), null);
  // Inside the viewport -> a `[[r,g,b],[r,g,b]]` pair, not null.
  const colors = app.cellColors(0, 0);
  assert.ok(colors !== null);
  assert.equal(colors.length, 2);
});
