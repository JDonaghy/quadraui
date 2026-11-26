// Black-box coverage for pointer hover reaching a painted widget through
// the binding: a `MouseMoved` pushed into `QuiApp` feeds the ambient
// pointer state, and the next frame paints the hovered part with its
// hover tint. Port of `quadraui-py/tests/test_hover_channel.py`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

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

function tabBarApp(): any {
  const app = new QuiApp(40, 20);
  app.setRender(() => ({
    tab_bar: {
      id: "tabs:hover",
      tabs: [{ label: "alpha", is_active: true }, { label: "beta" }],
    },
  }));
  return app;
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

function bgAt(app: any, col: number, row: number): number[] {
  const [, bg] = app.cellColors(col, row) as [number[], number[]];
  return bg;
}

test("moving over an inactive tab tints it and moving off restores it", () => {
  const app = tabBarApp();
  const [row, col] = locate(app, "beta");
  const resting = bgAt(app, col, row);

  moveTo(app, row, col);
  assert.notDeepEqual(bgAt(app, col, row), resting);

  // Row 10 is below the bar, on no painted widget.
  moveTo(app, 10, col);
  assert.deepEqual(bgAt(app, col, row), resting);
});

test("hovering one tab leaves the other at rest", () => {
  const app = tabBarApp();
  const [alphaRow, alphaCol] = locate(app, "alpha");
  const alphaResting = bgAt(app, alphaCol, alphaRow);

  const [row, col] = locate(app, "beta");
  moveTo(app, row, col);
  assert.deepEqual(bgAt(app, alphaCol, alphaRow), alphaResting);
});
