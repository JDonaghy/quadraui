// Runs the README hero counter (`examples/hero/node/run_headless.ts`) so the
// JSX sample stays executable, and checks that `run_tui.ts` runs a file of
// the reader's own, as the Node quick start tells them to. `npm run
// typecheck:jsx` type-checks the counter. The Rust and Python versions of
// the app are checked by `examples/hero/rust/tests/headless.rs` and
// `quadraui-py/tests/test_hero_counter.py`.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const HERO = path.join(__dirname, "..", "..", "examples", "hero", "node");

test("the hero counter runs headless and counts + - + to 1", () => {
  const runner = path.join(HERO, "run_headless.ts");
  const result = spawnSync(process.execPath, [...process.execArgv, runner], { encoding: "utf8", timeout: 60_000 });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /Count: 1/);
  assert.match(result.stdout, /ok: node hero counter/);
});

test("a test in its own file loads the app's .tsx file and passes", () => {
  const result = spawnSync(process.execPath, [...process.execArgv, path.join(HERO, "run_tui.ts"), path.join(HERO, "counter.test.tsx")], {
    encoding: "utf8",
    timeout: 60_000,
  });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /^ok$/m);
});

test("run_tui.ts runs the .tsx file it is given, outside the repo", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "qu-run-tui-"));
  const app = path.join(dir, "app.tsx");
  fs.writeFileSync(
    app,
    [
      'const { App } = require("@quadraui/node/app");',
      'const { harness } = require("@quadraui/node/testing");',
      "const item = (text: string) => ({ text: { spans: [{ text, fg: null, bg: null }] } });",
      "if (require.main === module) {",
      '  const h = harness(new App(30, 5, { view: () => <list id="l" items={[item("from my file")]} /> }));',
      '  console.log(h.contains("from my file") ? "ok: own file" : h.screen());',
      "}",
    ].join("\n"),
  );
  try {
    const result = spawnSync(process.execPath, [...process.execArgv, path.join(HERO, "run_tui.ts"), app], {
      encoding: "utf8",
      timeout: 60_000,
    });
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /ok: own file/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
