// Runnable demo of `../js/app/widget.ts`'s `defineWidget`: a 5-star rating
// control with its own click regions, keyboard handling and accessibility
// -- a script-authored widget, not a composition of existing nodes (see
// `../js/app/index.ts`'s own module doc -- this is exactly the case
// composition can't cover). Mirrors `quadraui-py/examples/rating.py`
// concept for concept.
//
// Run headless (clicks the fifth star, then nudges it with the keyboard,
// printing what the app's own `onRate` callback recorded after each):
//
//     node examples/rating.ts
//
// Run in a real terminal (click a star, or focus the control with Tab and
// use Left/Right to change the rating; `q`/Escape quits):
//
//     node examples/rating.ts --tui

import type { ComponentLike } from "../js/app/nodes";
import type {
  WidgetA11yNode,
  WidgetConstraints,
  WidgetEvent,
  WidgetLayoutCtx,
  WidgetOptions,
  WidgetPaint,
  WidgetRect,
  WidgetSize,
  WidgetSpec,
} from "../js/app/widget";

const appModule = require("../js/app/index.ts");
const { App, keyName } = appModule;
/**
 * `../js/app/index.ts`'s own `defineWidget` re-export, typed by hand --
 * `require`'s result is otherwise `any`, and TS rejects a type argument
 * (`defineWidget<RatingProps>`, below) on an untyped call.
 */
const defineWidget: <P extends object, S = Record<string, unknown>>(
  spec: WidgetSpec<P, S>,
) => (options: WidgetOptions<P>) => ComponentLike = appModule.defineWidget;

const WIDTH = 30;
const HEIGHT = 1;
const MAX_STARS = 5;
exports.WIDTH = WIDTH;
exports.HEIGHT = HEIGHT;
exports.MAX_STARS = MAX_STARS;

const FILLED = { r: 230, g: 180, b: 20, a: 255 };
const EMPTY = { r: 120, g: 120, b: 120, a: 255 };

/**
 * `RatingControl`'s own domain props: `value` (0..=`max`) is the current
 * rating; `onRate`, if given, is called with the new value on a star
 * click or an arrow key while this control holds focus.
 */
export interface RatingProps {
  value: number;
  max?: number;
  onRate?: (value: number) => void;
}

function rate(value: number, props: RatingProps): void {
  props.onRate?.(value);
}

/**
 * Exported on its own (not just wrapped in `RatingControl` below) so a
 * test can call `measure`/`paint`/`onEvent`/`a11y` directly -- the same
 * "exercise the method, not a running app" shape
 * `quadraui-py/tests/test_rating.py`'s own `test_a11y_reports_a_slider_role_and_the_current_value`
 * uses, since no a11y query API exists end to end yet on either binding.
 */
const ratingWidgetSpec: WidgetSpec<RatingProps> = {
  measure(ctx: WidgetLayoutCtx, props: RatingProps, constraints: WidgetConstraints): WidgetSize {
    const maxStars = props.max ?? MAX_STARS;
    let width = maxStars * 2 * ctx.charWidth;
    if (constraints.maxWidth !== null) {
      width = Math.min(width, constraints.maxWidth);
    }
    return { width, height: ctx.lineHeight };
  },

  paint(_ctx: WidgetLayoutCtx, props: RatingProps, rect: WidgetRect): WidgetPaint {
    const maxStars = props.max ?? MAX_STARS;
    const value = props.value;
    const starWidth = rect.width / maxStars;
    const ops: WidgetPaint["ops"] = [];
    const regions: NonNullable<WidgetPaint["regions"]> = [];
    for (let i = 0; i < maxStars; i++) {
      const x = i * starWidth;
      const filled = i < value;
      ops.push({
        TextRun: {
          rect: { x, y: 0, width: starWidth, height: rect.height },
          text: filled ? "★" : "☆",
          color: filled ? FILLED : EMPTY,
        },
      } as WidgetPaint["ops"][number]);
      regions.push({ id: `star-${i}`, rect: { x, y: 0, width: starWidth, height: rect.height } });
    }
    return { ops, regions };
  },

  onEvent(event: WidgetEvent, props: RatingProps): boolean {
    if ("Canvas" in event) {
      const canvas = event.Canvas as { RegionClicked?: { region: string } } & {
        RegionDoubleClicked?: { region: string };
      };
      const clicked = canvas.RegionClicked ?? canvas.RegionDoubleClicked;
      if (clicked === undefined) {
        return false;
      }
      const star = Number(clicked.region.split("-")[1]);
      rate(star + 1, props);
      return true;
    }
    if ("Key" in event) {
      const name = keyName({ KeyPressed: { key: event.Key } });
      const max = props.max ?? MAX_STARS;
      if (name === "Right") {
        rate(Math.min(max, props.value + 1), props);
        return true;
      }
      if (name === "Left") {
        rate(Math.max(0, props.value - 1), props);
        return true;
      }
    }
    return false;
  },

  a11y(props: RatingProps): WidgetA11yNode {
    const max = props.max ?? MAX_STARS;
    return {
      info: {
        a11y_role: "slider",
        a11y_label: "Rating",
        value: `${props.value} of ${max} stars`,
        value_range: { min: 0.0, max, current: props.value },
      },
      children: [],
    };
  },
};
exports.ratingWidgetSpec = ratingWidgetSpec;

/**
 * A fresh `RatingControl({...})` call is typically made every `view()`
 * call, the same way any other node constructor is -- this widget is
 * stateless itself (no `initialState`); the actual rating lives on
 * whichever app object owns it (`RatingDemo` below), threaded back in as
 * `props.value` every frame.
 */
const RatingControl = defineWidget<RatingProps>(ratingWidgetSpec);
exports.RatingControl = RatingControl;

/** Owns the one piece of state this demo needs (the current rating) and the `App` it drives. */
class RatingDemo {
  rating = 3;
  app: InstanceType<typeof App>;

  constructor(width = WIDTH, height = HEIGHT) {
    this.app = new App(width, height, { view: () => this.view(), onEvent: (e: unknown) => this.onEvent(e) });
  }

  private onRate = (value: number): void => {
    this.rating = value;
  };

  view(): unknown {
    // The sole root node, filling the whole viewport -- see
    // `RatingControl`'s own `measure`'s doc for why this demo keeps it
    // that way rather than placing it alongside a text label in a
    // `Column` (every `Column` child defaults to an equal share of the
    // remaining space, not its own measured size, unless it opts into
    // `"size": "Content"` itself -- a composition wrinkle this
    // single-widget demo sidesteps rather than explains).
    return RatingControl({ id: "rating:stars", value: this.rating, focusable: true, onRate: this.onRate });
  }

  private onEvent(event: unknown): void {
    const name = keyName(event);
    if (name === "q" || name === "Escape") {
      this.app.stopLive();
    }
  }
}
exports.RatingDemo = RatingDemo;

function runHeadless(demo: RatingDemo): void {
  console.log("before any interaction:");
  console.log(demo.app.screen());
  console.log(`rating == ${demo.rating}`);

  // The control is the sole root node, so it fills the whole
  // `WIDTH`-wide viewport: each of `MAX_STARS` stars is `WIDTH /
  // MAX_STARS` cells wide. Click the fifth star (index 4).
  const starWidth = WIDTH / MAX_STARS;
  const x = starWidth * 4 + starWidth / 2;
  demo.app.pushEvent({ MouseDown: { button: "Left", position: { x, y: 0.0 }, modifiers: {} } });
  demo.app.step();
  console.log(`\nafter clicking star 5, rating == ${demo.rating}:`);
  console.log(demo.app.screen());

  // The click above already focused the control (its only tab stop), so
  // a plain `KeyPressed` routes straight to it.
  demo.app.pushEvent({ KeyPressed: { key: { Named: "Left" }, modifiers: {}, repeat: false } });
  demo.app.step();
  console.log(`\nafter pressing Left, rating == ${demo.rating}:`);
  console.log(demo.app.screen());
}

if (require.main === module) {
  const demo = new RatingDemo();
  if (new Set(process.argv.slice(2)).has("--tui")) {
    demo.app.runTui();
  } else {
    runHeadless(demo);
  }
}
