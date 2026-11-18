// Acceptance tests for `../examples/rating.ts` -- the `../js/app/widget.ts`
// `defineWidget` demo. Mirrors `quadraui-py/tests/test_rating.py` concept
// for concept: render, click-a-star, focused-arrow-key, and a11y role,
// plus a `tsc --strict` type-error check for `RatingControl`'s own typed
// props (this package's acceptance bar for a typed public surface -- see
// `jsx_runtime.test.ts`'s own "wrong prop name" test for the pattern this
// one reuses against a plain, non-JSX call site).
//
// Run with `npm run build:debug` (or `npm run build`) then `npm test`.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const rating = require("../examples/rating.ts");

function starX(index: number): number {
  const starWidth = rating.WIDTH / rating.MAX_STARS;
  return starWidth * index + starWidth / 2;
}

function click(app: { pushEvent(event: unknown): void; step(): boolean }, x: number, y: number): void {
  app.pushEvent({ MouseDown: { button: "Left", position: { x, y }, modifiers: {} } });
  app.step();
}

function press(app: { pushEvent(event: unknown): void; step(): boolean }, name: string): void {
  app.pushEvent({ KeyPressed: { key: { Named: name }, modifiers: {}, repeat: false } });
  app.step();
}

function countOccurrences(haystack: string, needle: string): number {
  return haystack.split(needle).length - 1;
}

// --- render / click / keyboard -----------------------------------------

test("renders the initial rating as filled and empty stars", () => {
  const demo = new rating.RatingDemo();
  const screen = demo.app.screen();
  assert.equal(countOccurrences(screen, "★"), demo.rating);
  assert.equal(countOccurrences(screen, "☆"), rating.MAX_STARS - demo.rating);
});

test("clicking star 4 rates it and calls onRate", () => {
  const demo = new rating.RatingDemo();
  // Star index 3 -- the 4th star, 1-based, the issue's own worked example.
  click(demo.app, starX(3), 0);
  assert.equal(demo.rating, 4);
  assert.equal(countOccurrences(demo.app.screen(), "★"), 4);
});

test("clicking a star twice is idempotent and re-renders from app state", () => {
  const demo = new rating.RatingDemo();
  click(demo.app, starX(1), 0);
  assert.equal(demo.rating, 2);
  click(demo.app, starX(1), 0);
  assert.equal(demo.rating, 2);
  assert.equal(countOccurrences(demo.app.screen(), "★"), 2);
});

test("a region click calls onRate exactly once", () => {
  const calls: number[] = [];
  const RatingControl = rating.RatingControl as (options: unknown) => unknown;
  const { App } = require("../js/app/index.ts");
  const app = new App(rating.WIDTH, rating.HEIGHT, {
    view: () => RatingControl({ id: "rating:stars", value: 3, onRate: (v: number) => calls.push(v) }),
    onEvent: () => {},
  });
  click(app, starX(4), 0);
  assert.deepEqual(calls, [5]);
});

test("a focused key press calls onRate exactly once", () => {
  // Regression coverage for the duplicate-dispatch bug a sibling binding
  // hit (quadraui-py#2677): if `InteractionEngine::handle`'s own generic
  // `scripted_key_press` forwarding were ever joined by a second,
  // binding-side dispatch of the same key, `onRate` would fire twice for
  // one `Right` press -- invisible in `rating`/`screen()` assertions
  // alone (`_rate` is idempotent), so this counts invocations directly.
  const calls: number[] = [];
  const RatingControl = rating.RatingControl as (options: unknown) => unknown;
  const { App } = require("../js/app/index.ts");
  const app = new App(rating.WIDTH, rating.HEIGHT, {
    view: () => RatingControl({ id: "rating:stars", value: 3, focusable: true, onRate: (v: number) => calls.push(v) }),
    onEvent: () => {},
  });
  // Focuses the control without itself calling `onRate` (star index 2 is
  // already the current value, so `_rate`/`rate` is still invoked once
  // with that same value).
  click(app, starX(2), 0);
  calls.length = 0;
  press(app, "Right");
  assert.deepEqual(calls, [4]);
});

test("Right arrow increments the rating while the control holds focus", () => {
  const demo = new rating.RatingDemo();
  // Focuses the control -- its only tab stop -- without changing the
  // rating (star index 2 is already the current value, 3).
  click(demo.app, starX(2), 0);
  assert.equal(demo.rating, 3);

  press(demo.app, "Right");
  assert.equal(demo.rating, 4);

  press(demo.app, "Left");
  press(demo.app, "Left");
  assert.equal(demo.rating, 2);
});

test("the rating cannot go below zero or above max stars", () => {
  const demo = new rating.RatingDemo();
  demo.rating = 0;
  click(demo.app, starX(0), 0);
  assert.equal(demo.rating, 1);

  for (let i = 0; i < 10; i++) press(demo.app, "Left");
  assert.equal(demo.rating, 0);

  for (let i = 0; i < 10; i++) press(demo.app, "Right");
  assert.equal(demo.rating, rating.MAX_STARS);
});

// --- a11y ----------------------------------------------------------------
//
// No a11y query API exists yet for a running `App` (same gap
// `quadraui-py/tests/test_rating.py`'s own equivalent test calls out) --
// this exercises `ratingWidgetSpec.a11y` directly, the way a native
// accessibility backend would read it.

test("a11y reports a slider role and the current value", () => {
  const node = rating.ratingWidgetSpec.a11y({ value: 3, max: 5 }) as {
    info: Record<string, unknown>;
  };
  assert.equal(node.info.a11y_role, "Slider");
  assert.deepEqual(node.info.value_range, { min: 0, max: 5, current: 3 });
  assert.ok(String(node.info.value).includes("3"));
});

// --- `tsc --strict` acceptance: a typed public surface --------------------

function runTsc(args: string[]): { status: number | null; output: string } {
  const repoRoot = path.join(__dirname, "..");
  const tscScript = path.join(repoRoot, "node_modules", "typescript", "bin", "tsc");
  try {
    const output = execFileSync(process.execPath, [tscScript, ...args], { cwd: repoRoot, encoding: "utf8" });
    return { status: 0, output };
  } catch (error) {
    const err = error as { status: number | null; stdout?: string };
    return { status: err.status, output: err.stdout ?? "" };
  }
}

const INDEX_D_TS = path.join(__dirname, "..", "index.d.ts");

test("examples/rating.ts type-checks clean under tsc --strict", { skip: !fs.existsSync(INDEX_D_TS) }, () => {
  const result = runTsc([
    "--strict",
    "--noEmit",
    "--module",
    "commonjs",
    "--target",
    "es2020",
    "--moduleResolution",
    "node",
    "--skipLibCheck",
    "--types",
    "node",
    path.join(__dirname, "..", "examples", "rating.ts"),
  ]);
  assert.equal(result.status, 0, result.output);
});

test("a wrong prop name on RatingControl's options fails tsc --strict", { skip: !fs.existsSync(INDEX_D_TS) }, () => {
  const repoRoot = path.join(__dirname, "..");
  const originalPath = path.join(repoRoot, "examples", "rating.ts");
  const original = fs.readFileSync(originalPath, "utf8");
  assert.ok(
    original.includes('RatingControl({ id: "rating:stars", value: this.rating, focusable: true, onRate: this.onRate })'),
    "fixture source has drifted -- update this test's own substitution",
  );
  const broken = original.replace(
    'RatingControl({ id: "rating:stars", value: this.rating, focusable: true, onRate: this.onRate })',
    'RatingControl({ id: "rating:stars", valueTypo: this.rating, focusable: true, onRate: this.onRate })',
  );
  // Suffixed with this process's own pid (see the top-level `.gitignore`
  // entries for these fixtures) so two concurrent runs of this test never
  // write the same path out from under each other.
  const suffix = `.${process.pid}`;
  const brokenPath = path.join(repoRoot, "examples", `rating.broken_fixture${suffix}.ts`);
  fs.writeFileSync(brokenPath, broken);
  try {
    const result = runTsc([
      "--strict",
      "--noEmit",
      "--module",
      "commonjs",
      "--target",
      "es2020",
      "--moduleResolution",
      "node",
      "--skipLibCheck",
      "--types",
      "node",
      brokenPath,
    ]);
    assert.notEqual(result.status, 0);
    assert.match(result.output, /valueTypo/);
  } finally {
    fs.rmSync(brokenPath, { force: true });
  }
});
