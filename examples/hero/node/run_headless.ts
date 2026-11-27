// Drives the hero counter headless on the TUI backend: click `+`, `-`, `+`
// by their on-screen labels, then check the rendered frame. The Rust
// test (`examples/hero/rust/tests/headless.rs`) and the Python runner
// (`examples/hero/python/run_headless.py`) drive the identical script.
//
// `counter.tsx` is JSX; `load_jsx.ts` compiles it in memory.
//
//     node examples/hero/node/run_headless.ts     (from the repo root; Node >= 22.6)

const { loadHero, SUBPATHS } = require("./load_jsx.ts");

const { Counter } = loadHero();
const { App } = require(SUBPATHS["@quadraui/node/app"]);
const { harness } = require(SUBPATHS["@quadraui/node/testing"]);

const counter = new Counter();
const h = harness(new App(30, 5, { view: () => counter.view() }));
if (!h.contains("Count: 0")) throw new Error(h.screen());
// `+ - +`, not `+ + -`: two clicks on one spot this fast fold into a
// DoubleClick, which a Button does not treat as a second activation (see
// examples/hero/README.md, "Gaps").
for (const label of ["+", "-", "+"]) h.clickText(label);
console.log(h.screen());
if (!h.contains("Count: 1")) throw new Error(h.screen());
if (counter.count !== 1) throw new Error(`count == ${counter.count}`);
console.log("ok: node hero counter");
