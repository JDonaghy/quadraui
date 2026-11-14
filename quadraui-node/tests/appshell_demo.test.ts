// Black-box acceptance test for `examples/appshell_demo.ts`: drives the
// 10,000-row `AppShellDemo` headless and asserts it scrolls through every
// step without dropping or duplicating one, within a bounded step budget.
// Mirrors `quadraui-py/tests/test_appshell_demo.py` 1:1.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test

const { test } = require("node:test");
const assert = require("node:assert/strict");

const appshellDemo = require("../examples/appshell_demo.ts");

// Headless scrolling through the full demo is not expected to be fast in
// any absolute sense -- this is a guard against an accidental quadratic
// blowup (e.g. an `O(rows * steps)` descriptor rebuild), not a
// timing-precision assertion.
const STEP_BUDGET_MS = 10_000;

test("the initial screen shows the first rows", () => {
  const { app } = appshellDemo.runDemo(0);
  assert.ok(app.screenContains("Row 00000"));
});

test("scrolling advances every step without dropping one", () => {
  const steps = 20;

  const start = Date.now();
  const { app, offsets } = appshellDemo.runDemo(steps);
  const elapsed = Date.now() - start;

  // One recorded offset per step, plus the initial value.
  assert.equal(offsets.length, steps + 1);
  // Every step lands, in order, none skipped and none repeated: each
  // consecutive pair differs by exactly one page.
  const deltas = new Set(offsets.slice(1).map((value: number, i: number) => value - offsets[i]));
  assert.deepEqual(deltas, new Set([appshellDemo.PAGE]));

  const lastOffset = offsets[offsets.length - 1];
  assert.ok(app.screenContains(`Row ${String(lastOffset).padStart(5, "0")}`));
  assert.ok(elapsed < STEP_BUDGET_MS);
});

test("scrolling reaches and clamps at the last row", () => {
  // Enough pages to run past the end of the 10k rows, to exercise the
  // clamp rather than stopping short of it.
  const steps = Math.floor(appshellDemo.ROW_COUNT / appshellDemo.PAGE) + 5;

  const start = Date.now();
  const { app, shell, offsets } = appshellDemo.runDemo(steps);
  const elapsed = Date.now() - start;

  const lastRow = shell.rows.length - 1;
  assert.equal(offsets[offsets.length - 1], lastRow);
  assert.equal(shell.selectedIdx, lastRow);
  assert.ok(app.screenContains(`Row ${String(lastRow).padStart(5, "0")}`));
  assert.ok(elapsed < STEP_BUDGET_MS);
});
