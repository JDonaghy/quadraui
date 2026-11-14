// Runnable demo of the `Scroll` node constructor: 40 buttons stacked in a
// `Column`, wrapped in a `Scroll` viewport that only shows 10 rows at a
// time -- the issue's own "40 buttons in a 10-row viewport" shape.
// Mirrors `quadraui-py/examples/scroll_demo.py` 1:1.
//
// Run headless (drives the scroll with synthetic wheel/click events and
// prints the before/after screens, plus which button a click on the
// viewport's top row now resolves to):
//
//     node examples/scroll_demo.ts
//
// Run in a real terminal (mouse wheel/PageDown/PageUp/scrollbar-drag
// actually scroll it, clicking a button prints its id below the viewport,
// `q`/Escape quits):
//
//     node examples/scroll_demo.ts --tui
//
// Either way this is an *uncontrolled* `Scroll` -- `offset` is left unset
// on every render, so `quadraui`'s own `InteractionEngine` owns the
// position across frames; see `ScrollOptions`' own doc for the controlled
// alternative (pinning `offset` from app state on every render).

const { App, Button, Column, Scroll, keyName } = require("../js/app/index.ts");

import type { Renderable } from "../js/app/index";

const WIDTH = 40;
const HEIGHT = 10;
const BUTTON_COUNT = 40;

/** Owns the one piece of state this demo needs: which buttons have been clicked, in click order. */
class ScrollDemo {
  clicked: string[] = [];

  private _onClick(buttonId: string): void {
    this.clicked.push(buttonId);
  }

  view(): Renderable {
    const buttons: Renderable[] = [];
    for (let i = 0; i < BUTTON_COUNT; i++) {
      const buttonId = `btn:${i}`;
      buttons.push(
        Button(`Item${String(i).padStart(2, "0")}`, {
          id: buttonId,
          onClick: () => this._onClick(buttonId),
        }),
      );
    }
    return Scroll(Column(buttons), { id: "scroll:demo" });
  }
}

/** The demo's `App`, built the same way `hot_reload_demo.ts`'s own `build` is. */
function build(): { app: InstanceType<typeof App>; demo: ScrollDemo } {
  const demo = new ScrollDemo();
  const app = new App(WIDTH, HEIGHT, {
    view: () => demo.view(),
    onEvent: (event: unknown) => {
      const name = keyName(event);
      if (name === "q" || name === "Escape") {
        app.stopLive();
      }
    },
  });
  return { app, demo };
}

function runHeadless(app: InstanceType<typeof App>, demo: ScrollDemo): void {
  console.log("before scrolling:");
  console.log(app.screen());

  // A wheel notch landing on the scrollbar's own track/thumb -- the
  // rightmost viewport column, reserved once content overflows it --
  // moves the container the same way a real mouse wheel would.
  app.pushEvent({ Scroll: { delta: { x: 0.0, y: 10.0 }, position: { x: WIDTH - 1.0, y: 1.0 } } });
  app.run(1);
  console.log("\nafter scrolling 10 rows down:");
  console.log(app.screen());

  // The hit map moved with the paint, not just the pixels: clicking the
  // viewport's top row resolves to the button the scroll brought there
  // ("btn:10"), not the button that painted there before the scroll
  // ("btn:0").
  app.pushEvent({ MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} } });
  app.run(1);
  console.log(`\nclicking the top row now reports: ${JSON.stringify(demo.clicked)}`);
}

exports.ScrollDemo = ScrollDemo;
exports.build = build;
exports.WIDTH = WIDTH;
exports.HEIGHT = HEIGHT;
exports.BUTTON_COUNT = BUTTON_COUNT;

if (require.main === module) {
  const flags = new Set(process.argv.slice(2));
  const { app, demo } = build();
  if (flags.has("--tui")) {
    app.runTui();
  } else {
    runHeadless(app, demo);
  }
}
