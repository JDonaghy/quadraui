// Black-box coverage for `QuiApp` through the published JS surface
// (`js/index.ts`), run with `npm test` -> `node --test tests/*.test.ts`.
// Node's own built-in test runner and native TypeScript support, so no
// extra test-framework dependency.
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

// `QuiApp.screen()` ends every row (including the last) with a trailing
// `\n` -- see `quadraui::tui::testing::TuiDriver::screen`'s doc -- so a
// plain `split("\n")` leaves one extra empty string at the end; this
// splits on each row's own trailing newline instead.
function screenLines(screen: string): string[] {
  return screen.split("\n").slice(0, -1);
}

test("constructs on a cols x rows grid and renders a blank screen", () => {
  const app = new QuiApp(10, 4);
  const lines = screenLines(app.screen());

  assert.equal(lines.length, 4);
  for (const line of lines) {
    assert.equal(line.trim(), "", `expected a blank line, got ${JSON.stringify(line)}`);
  }
});

test("rejects a zero width or height", () => {
  assert.throws(() => new QuiApp(0, 4));
  assert.throws(() => new QuiApp(10, 0));
});

test("step/run keep the app running with nothing queued", () => {
  const app = new QuiApp(8, 3);
  assert.equal(app.step(), true);
  assert.equal(app.run(3), true);
});

test("pushKeyEvent queues an event step consumes", () => {
  const app = new QuiApp(8, 3);
  app.pushKeyEvent("x".codePointAt(0));
  assert.equal(app.step(), true);
});

test("resize changes the rendered grid's row count", () => {
  const app = new QuiApp(5, 2);
  app.resize(5, 6);
  const lines = screenLines(app.screen());
  assert.equal(lines.length, 6);
});

test("slotClear on an unknown slot id raises", () => {
  const app = new QuiApp(5, 5);
  assert.throws(() => app.slotClear(999));
});

test("slotClear on a never-occupied known slot returns false", () => {
  const app = new QuiApp(5, 5);
  // Slot 0 is `SLOT_BUTTON` -- see `quadraui-node/src/lib.rs`'s doc; a
  // never-populated app has nothing in it yet, so this is a no-op. See
  // `tests/descriptors.test.ts` for `insertButton` occupying/clearing it.
  assert.equal(app.slotClear(0), false);
});
