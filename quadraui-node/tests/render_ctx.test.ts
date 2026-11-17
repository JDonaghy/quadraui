// Black-box coverage for the render context a script `view()` callback
// gets through `quadraui-node`'s `App` -- `RenderCtx`'s `measureText`/
// `lineHeight`/`charWidth`/`cellGrid`/`scale`/`themeToken`/`styleToken`,
// `getCurrentCtx()`, and the `App` wrapper's own unconditional forwarding
// of it (no arity detection needed -- see `js/app/runtime.ts`'s own doc),
// so a pre-existing zero-argument `view()` keeps working unchanged.
// Mirrors `quadraui-py/tests/test_render_ctx.py` 1:1 where the two
// bindings' own shapes allow it.
//
// Run with `npm run build:debug` (or `npm run build`) then `npm test`.
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { getCurrentCtx, StaleRenderCtxError } = require("../js/index.ts");
const { App, Button } = require("../js/app/index.ts");

test("a view with no ctx parameter still renders unchanged", () => {
  // The original zero-argument `view()` shape every script written before
  // this existed uses must keep working exactly as before.
  const app = new App(20, 5, { view: () => Button("Save", { onClick: () => {} }) });
  assert.equal(app.screenContains("Save"), true);
});

test("a view declaring ctx receives a RenderCtx instance each render", () => {
  const captured: unknown[] = [];
  const app = new App(20, 5, {
    view(ctx: unknown) {
      captured.push(ctx);
      return Button("Save", { onClick: () => {} });
    },
  });
  // `App`'s constructor already forces one eager render (see its own
  // doc), so by the time `screenContains` triggers a second one,
  // `captured` has two entries -- assert on the fact that matters (every
  // entry is a real `RenderCtx`) rather than an exact count tied to that
  // detail.
  assert.equal(app.screenContains("Save"), true);
  assert.ok(captured.length > 0);
  for (const ctx of captured) {
    assert.equal((ctx as { constructor: { name: string } }).constructor.name, "RenderCtx");
  }
});

test("ctx reports the headless TUI backend as a cell grid", () => {
  // Every `ctx.*` call below runs *inside* `view` -- a `RenderCtx` is only
  // valid for the duration of the render call that handed it out (see
  // `RenderCtx`'s own doc, `../js/index.ts`) -- so `captured` stashes
  // results, not the `ctx` object itself, for the assertions after
  // `screen()`.
  const captured: Record<string, unknown> = {};
  const app = new App(20, 5, {
    view(ctx: { cellGrid(): boolean; scale(): number }) {
      captured.cellGrid = ctx.cellGrid();
      captured.scale = ctx.scale();
      return {};
    },
  });
  app.screen();
  assert.equal(captured.cellGrid, true);
  assert.equal(captured.scale, 1);
});

test("ctx measureText counts display width in cells", () => {
  const captured: Record<string, unknown> = {};
  const app = new App(20, 5, {
    view(ctx: {
      measureText(text: string): [number, number];
      lineHeight(): number;
      charWidth(): number;
    }) {
      captured.measured = ctx.measureText("hello");
      captured.lineHeight = ctx.lineHeight();
      captured.charWidth = ctx.charWidth();
      return {};
    },
  });
  app.screen();
  assert.deepEqual(captured.measured, [5, 1]);
  assert.equal(captured.lineHeight, 1);
  assert.equal(captured.charWidth, 1);
});

test("ctx themeToken and styleToken look up fields by name", () => {
  const captured: Record<string, unknown> = {};
  const app = new App(20, 5, {
    view(ctx: { themeToken(name: string): unknown; styleToken(name: string): unknown }) {
      captured.accentFg = ctx.themeToken("accent_fg");
      captured.unknownTheme = ctx.themeToken("not_a_real_field");
      captured.padding = ctx.styleToken("padding");
      captured.unknownStyle = ctx.styleToken("not_a_real_field");
      return {};
    },
  });
  app.screen();
  assert.deepEqual(
    Object.keys(captured.accentFg as object).sort(),
    ["a", "b", "g", "r"],
  );
  assert.equal(captured.unknownTheme, null);
  assert.equal(typeof captured.padding, "number");
  assert.equal(captured.unknownStyle, null);
});

test("a RenderCtx raises StaleRenderCtxError once the render call that handed it out returns", () => {
  let stashed: { measureText(text: string): unknown } | undefined;
  const app = new App(20, 5, {
    view(ctx: { measureText(text: string): unknown }) {
      stashed = ctx;
      return {};
    },
  });
  app.screen();
  assert.throws(() => stashed!.measureText("x"), StaleRenderCtxError);
});

test("getCurrentCtx() is None outside of a render call", () => {
  assert.equal(getCurrentCtx(), null);
});
