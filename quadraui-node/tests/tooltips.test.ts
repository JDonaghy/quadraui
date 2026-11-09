// Black-box coverage for the shared binding host's hover-tooltip driving
// (`quadraui_binding_core::BindingHost::drive_hover_tooltip`): a `Button`'s
// own `tooltip` text shows up as a real tooltip overlay once the pointer
// has hovered it continuously for `tooltipDelayMs`, and hides again on
// pointer leave, the next click, or the auto-dismiss timeout -- with no
// JS-side wiring beyond `insertButton`/`setHandler`/`pushEvent`/`step`,
// since the *host* (`BindingHost::drive_hover_tooltip`) drives
// `HoverTooltipTracker` itself. Ports `quadraui-py/tests/test_tooltips.py`
// to this binding.
//
// `setTestClockMs` pins the headless clock `BindingHost::now_ms()` reads
// to an absolute value this file picks, so every assertion below is
// driven by an exact, deterministic millisecond count rather than a real
// timer racing against `tooltipDelayMs`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test

const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

// Small values are fine -- nothing here waits in real time, so the clock
// jumps straight to whatever absolute millisecond each test needs.
const DELAY_MS = 20;
const AUTO_DISMISS_MS = 30;

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

function makeApp(autoDismissMs = 0): any {
  const app = new QuiApp(40, 10);
  app.setHandler(() => undefined);
  app.setStyle({
    tooltip_delay_ms: DELAY_MS,
    tooltip_auto_dismiss_ms: autoDismissMs,
  });
  app.setTestClockMs(0);
  app.insertButton({ id: "save", label: "Save", tooltip: "Save the file" });
  assert.ok(app.screenContains("Save"));
  return app;
}

test("tooltip appears only after the hover delay", () => {
  const app = makeApp();
  const [row, col] = locate(app, "Save");

  // Landing on the button is not enough by itself -- the delay hasn't
  // elapsed yet, so nothing shows.
  moveTo(app, row, col);
  assert.ok(!app.screenContains("Save the file"));

  // One millisecond short of the delay: still nothing.
  app.setTestClockMs(DELAY_MS - 1);
  app.step();
  assert.ok(!app.screenContains("Save the file"));

  // No further event queued: the next `step()` ticks the app instead of
  // dispatching, which is what notices the delay has elapsed with the
  // pointer held still.
  app.setTestClockMs(DELAY_MS);
  app.step();
  assert.ok(app.screenContains("Save the file"));
});

test("tooltip hides on pointer leave", () => {
  const app = makeApp();
  const [row, col] = locate(app, "Save");

  moveTo(app, row, col);
  app.setTestClockMs(DELAY_MS);
  app.step();
  assert.ok(app.screenContains("Save the file"));

  // Moving off the button (to the far bottom-right corner, well clear of
  // a single lone button painted near the top-left) hides it
  // immediately -- no further delay.
  moveTo(app, 9, 39);
  assert.ok(!app.screenContains("Save the file"));
});

test("tooltip hides on click then reappears if the pointer holds still", () => {
  const app = makeApp();
  const [row, col] = locate(app, "Save");

  moveTo(app, row, col);
  app.setTestClockMs(DELAY_MS);
  app.step();
  assert.ok(app.screenContains("Save the file"));

  // A click dismisses it immediately, same as `showTooltip`'s own
  // explicit-dismiss contract.
  click(app, row, col);
  assert.ok(!app.screenContains("Save the file"));

  // Still hovering the very same button, unmoved: holding still past
  // another full delay re-shows it rather than leaving the tracker stuck
  // believing a tooltip only it still thinks is open.
  app.setTestClockMs(2 * DELAY_MS);
  app.step();
  assert.ok(app.screenContains("Save the file"));
});

test("tooltip hides on auto-dismiss timeout", () => {
  const app = makeApp(AUTO_DISMISS_MS);
  const [row, col] = locate(app, "Save");

  moveTo(app, row, col);
  app.setTestClockMs(DELAY_MS);
  app.step();
  assert.ok(app.screenContains("Save the file"));

  // One millisecond short of the auto-dismiss deadline: still showing.
  app.setTestClockMs(DELAY_MS + AUTO_DISMISS_MS - 1);
  app.step();
  assert.ok(app.screenContains("Save the file"));

  // At the deadline, with the pointer never having moved: it auto-hides.
  app.setTestClockMs(DELAY_MS + AUTO_DISMISS_MS);
  app.step();
  assert.ok(!app.screenContains("Save the file"));
});

test("a button with no tooltip never shows one", () => {
  const app = new QuiApp(40, 10);
  app.setHandler(() => undefined);
  app.setStyle({ tooltip_delay_ms: DELAY_MS });
  app.setTestClockMs(0);
  app.insertButton({ id: "plain", label: "Plain" });
  const [row, col] = locate(app, "Plain");

  const before = app.screen();

  moveTo(app, row, col);
  app.setTestClockMs(DELAY_MS);
  app.step();

  // A widget with no `tooltip` set never arms the tracker at all, so the
  // screen is pixel-for-pixel identical to before the hover -- not just
  // "the button's own label still paints somewhere", which would also
  // pass if a spurious tooltip appeared.
  assert.equal(app.screen(), before);
});
