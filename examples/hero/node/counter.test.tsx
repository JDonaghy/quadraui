// A test of the hero counter in a file of its own, the shape the Node quick
// start gives a reader for their app: plain imports, the app loaded from its
// own file, run with `node examples/hero/node/run_tui.ts <this file>`.

import { App } from "@quadraui/node/app";
import { harness } from "@quadraui/node/testing";

const { Counter } = require("./counter.tsx");

const counter = new Counter();
const h = harness(new App(30, 5, { view: () => counter.view() }));
h.clickText("+");
h.clickText("+");
h.clickText("-");
if (!h.contains("Count: 1")) throw new Error(h.screen());
console.log("ok");
