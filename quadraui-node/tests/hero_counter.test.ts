// Runs the README hero counter (`examples/hero/node/run_headless.ts`) so the
// JSX sample stays executable; `npm run typecheck:jsx` type-checks the same
// file. The Rust and Python versions of the app are checked by
// `examples/hero/rust/tests/headless.rs` and
// `quadraui-py/tests/test_hero_counter.py`.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const path = require("node:path");

const RUNNER = path.join(__dirname, "..", "..", "examples", "hero", "node", "run_headless.ts");

test("the hero counter runs headless and counts + - + to 1", () => {
  const result = spawnSync(process.execPath, [...process.execArgv, RUNNER], { encoding: "utf8", timeout: 60_000 });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /Count: 1/);
  assert.match(result.stdout, /ok: node hero counter/);
});
