// Runs the parity app (`todo.tsx`) headless on the TUI backend through the
// input script given on the command line, and prints one JSON trace: after
// the first paint and after every step, the rendered cell grid (glyph, fg
// and bg of every cell) and the app updates that step fired. The Rust and
// Python tracers print the same shape; `scripts/parity_check.py` diffs them.
//
//     node examples/parity/node/trace.ts examples/parity/script.json

const fs = require("node:fs");
const path = require("node:path");
const { load, SUBPATHS } = require("../../hero/node/load_jsx.ts");

const { Todo } = load(path.join(__dirname, "todo.tsx"));
const { App } = require(SUBPATHS["@quadraui/node/app"]);
const { harness } = require(SUBPATHS["@quadraui/node/testing"]);

const WIDTH = 40;
const HEIGHT = 12;
const hex = (rgb: number[]) => rgb.map((c) => c.toString(16).padStart(2, "0")).join("");

function frame(h: any, app: any, todo: any, input: unknown) {
  const rows = h.screen().replace(/\n$/, "").split("\n");
  const styles: string[][] = [];
  for (let y = 0; y < HEIGHT; y++) {
    const row: string[] = [];
    for (let x = 0; x < WIDTH; x++) {
      const [fg, bg] = h.qui.cellColors(x, y);
      row.push(`${hex(fg)}/${hex(bg)}`);
    }
    styles.push(row);
  }
  const events = todo.log.splice(0);
  if (app.step() === false) events.push("quit");
  return { input, rows, styles, events };
}

const script = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const todo = new Todo();
const app = new App(WIDTH, HEIGHT, { view: () => todo.view() });
const h = harness(app);
const trace = [frame(h, app, todo, null)];
for (const step of script) {
  if ("click" in step) h.clickAt(step.click[0], step.click[1]);
  else if ("type" in step) h.type(step.type);
  else if ("key" in step) h.press(step.key);
  else if ("ctrl" in step) h.press(step.ctrl, { ctrl: true });
  else throw new Error(`unknown step ${JSON.stringify(step)}`);
  trace.push(frame(h, app, todo, step));
}
process.stdout.write(JSON.stringify(trace));
