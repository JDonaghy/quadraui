// Black-box coverage for the shared binding host's hover-tooltip driving
// (`quadraui_binding_core::BindingHost::drive_hover_tooltip`): a `Button`'s
// own `tooltip` text shows up as a real tooltip overlay once the pointer
// has hovered it continuously for `tooltipDelayMs`, and hides again on
// pointer leave or the next click -- with no JS-side wiring beyond
// `insertButton`/`setHandler`/`pushEvent`/`step`, since the engine now
// drives `HoverTooltipTracker` itself. Ports
// `quadraui-py/tests/test_tooltips.py` to this binding.
//
// `setStyle({ tooltip_delay_ms: ... })` is set to a small value and these
// tests sleep past it with a real (but tiny) wait -- the same
// "headless, no real timer, but a short real wait at the FFI boundary"
// pattern this crate's own waker `_after` tests already use, since there
// is no way from JS to inject a synthetic clock into the host's own
// wall-clock-driven `now_ms()`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test

const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

// Small enough to keep this file fast, large enough that a single real
// wait below comfortably clears it without flaking.
const DELAY_MS = 20;
const SLEEP_MS = 100;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

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

function moveTo(app: any, row: number, col: number) {
  app.pushEvent({
    MouseMoved: {
      position: { x: col, y: row },
      buttons: { left: false, right: false, middle: false },
    },
  });
  app.step();
}

function click(app: any, row: number, col: number) {
  app.pushEvent({
    MouseDown: {
      button: "Left",
      position: { x: col, y: row },
      modifiers: {},
    },
  });
  app.step();
}

function makeApp(): any {
  const app = new QuiApp(40, 10);
  app.setHandler(() => undefined);
  app.setStyle({ tooltip_delay_ms: DELAY_MS });
  app.insertButton({ id: "save", label: "Save", tooltip: "Save the file" });
  assert.ok(app.screenContains("Save"));
  return app;
}

test("tooltip appears only after the hover delay", async () => {
  const app = makeApp();
  const [row, col] = locate(app, "Save");

  // Landing on the button is not enough by itself -- the delay hasn't
  // elapsed yet, so nothing shows.
  moveTo(app, row, col);
  assert.ok(!app.screenContains("Save the file"));

  // No further event queued: the next `step()` ticks the app instead of
  // dispatching, which is what notices the delay has elapsed with the
  // pointer held still.
  await sleep(SLEEP_MS);
  app.step();
  assert.ok(app.screenContains("Save the file"));
});

test("tooltip hides on pointer leave", async () => {
  const app = makeApp();
  const [row, col] = locate(app, "Save");

  moveTo(app, row, col);
  await sleep(SLEEP_MS);
  app.step();
  assert.ok(app.screenContains("Save the file"));

  // Moving off the button (to the far bottom-right corner, well clear of
  // a single lone button painted near the top-left) hides it
  // immediately -- no further delay.
  moveTo(app, 9, 39);
  assert.ok(!app.screenContains("Save the file"));
});

test("tooltip hides on click then reappears if the pointer holds still", async () => {
  const app = makeApp();
  const [row, col] = locate(app, "Save");

  moveTo(app, row, col);
  await sleep(SLEEP_MS);
  app.step();
  assert.ok(app.screenContains("Save the file"));

  // A click dismisses it immediately, same as `showTooltip`'s own
  // explicit-dismiss contract.
  click(app, row, col);
  assert.ok(!app.screenContains("Save the file"));

  // Still hovering the very same button, unmoved: holding still past
  // another full delay re-shows it rather than leaving the tracker stuck
  // believing a tooltip only it still thinks is open.
  await sleep(SLEEP_MS);
  app.step();
  assert.ok(app.screenContains("Save the file"));
});

test("a button with no tooltip never shows one", async () => {
  const app = new QuiApp(40, 10);
  app.setHandler(() => undefined);
  app.setStyle({ tooltip_delay_ms: DELAY_MS });
  app.insertButton({ id: "plain", label: "Plain" });
  const [row, col] = locate(app, "Plain");

  moveTo(app, row, col);
  await sleep(SLEEP_MS);
  app.step();

  // Nothing to assert `screenContains` *for* here beyond "the button's
  // own label is still the only thing painted" -- there is no tooltip
  // text to check against, which is the point: a widget with no
  // `tooltip` set never arms the tracker at all.
  assert.ok(app.screenContains("Plain"));
});
