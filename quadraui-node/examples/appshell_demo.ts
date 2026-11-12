// AppShell-shaped demo: one `ListView` holding 10,000 rows, scrolled
// smoothly from JavaScript/TypeScript. Mirrors
// `quadraui-py/examples/appshell_demo.py` 1:1.
//
// `QuiApp` has no in-place "update a slot" call -- a descriptor is immutable
// once inserted, so advancing the scroll position means clearing the slot
// and re-inserting the list with a new `scroll_offset`/`selected_idx` on
// every "frame". `AppShellDemo` below owns that dance; the scroll loop
// itself is a plain `for` loop driving it one page at a time, standing in
// for whatever drives real key/scroll-wheel events (a bound handler must
// not call back into the same `QuiApp` it was invoked from -- `QuiApp`
// rejects that reentrant borrow -- so this demo calls `scroll()` directly
// rather than through `setHandler`).
//
// Run headless:
//
//     node examples/appshell_demo.ts

const { QuiApp } = require("../js/index.ts");

import type { ListItemDescriptor, ListViewDescriptor } from "../js/wire";

const WIDTH = 80;
const HEIGHT = 24;
const ROW_COUNT = 10_000;
const PAGE = 250;

function makeRows(count: number): ListItemDescriptor[] {
  const rows: ListItemDescriptor[] = [];
  for (let i = 0; i < count; i++) {
    rows.push({ text: { spans: [{ text: `Row ${String(i).padStart(5, "0")}`, fg: null, bg: null }] } });
  }
  return rows;
}

/**
 * Drives the single `ListView` slot of a `QuiApp`, re-inserting it with an
 * updated `scroll_offset`/`selected_idx` each time the list scrolls.
 */
class AppShellDemo {
  app: InstanceType<typeof QuiApp>;
  rows: ListItemDescriptor[];
  selectedIdx: number;
  scrollOffset: number;
  slot: number;

  constructor(app: InstanceType<typeof QuiApp>, rows: ListItemDescriptor[]) {
    this.app = app;
    this.rows = rows;
    this.selectedIdx = 0;
    this.scrollOffset = 0;
    this.slot = app.insertList(this._descriptor());
  }

  private _descriptor(): ListViewDescriptor {
    return {
      id: "list:appshell",
      items: this.rows,
      selected_idx: this.selectedIdx,
      scroll_offset: this.scrollOffset,
      has_focus: true,
      show_v_scrollbar: true,
    };
  }

  scroll(delta: number): void {
    const last = this.rows.length - 1;
    this.selectedIdx = Math.max(0, Math.min(last, this.selectedIdx + delta));
    this.scrollOffset = Math.max(0, Math.min(last, this.scrollOffset + delta));
    this.app.slotClear(this.slot);
    this.slot = this.app.insertList(this._descriptor());
  }
}

/**
 * Builds the demo app, scrolls down `steps` times, and returns
 * `{app, shell, offsets}` -- `offsets[0]` is the initial `scroll_offset`,
 * `offsets[i]` the value after the i-th scroll step.
 */
function runDemo(steps = 20): { app: InstanceType<typeof QuiApp>; shell: AppShellDemo; offsets: number[] } {
  const app = new QuiApp(WIDTH, HEIGHT);
  const shell = new AppShellDemo(app, makeRows(ROW_COUNT));

  const offsets = [shell.scrollOffset];
  for (let i = 0; i < steps; i++) {
    shell.scroll(PAGE);
    offsets.push(shell.scrollOffset);
  }

  return { app, shell, offsets };
}

exports.AppShellDemo = AppShellDemo;
exports.makeRows = makeRows;
exports.runDemo = runDemo;
exports.WIDTH = WIDTH;
exports.HEIGHT = HEIGHT;
exports.ROW_COUNT = ROW_COUNT;
exports.PAGE = PAGE;

if (require.main === module) {
  const { app, offsets } = runDemo();
  console.log(app.screen());
  console.log(`scroll offsets after each step: ${JSON.stringify(offsets)}`);
}
