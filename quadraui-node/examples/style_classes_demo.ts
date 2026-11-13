// Runnable demo of named style sets: `app.defineStyle(name, overrides)`
// plus the "class" key any layout-tree node may carry -- the CSS-class
// analogue of the per-node "style" key `style_override_demo.ts` shows.
// Mirrors `quadraui-py/examples/style_classes_demo.py`'s headless path 1:1
// (this binding's live-session model is non-blocking `startTui`, not a
// blocking `run_tui` call, so only the headless path is ported here).
//
// A "row" of four buttons, each pulling its look from a *name* rather than
// from inline tokens: "Save" uses the built-in "primary" set, "Discard" a
// host-defined "brand" set, "Reset" the two of them together ("brand
// danger" -- later name wins the tokens both name), and "Quiet" the
// built-in "subtle" set.
//
// Redefining "brand" repaints every node that names it, with no change to
// the render callback at all -- which is the whole point of a named set
// over inline tokens.
//
// Run headless (prints the screen before and after redefining "brand"):
//
//     node examples/style_classes_demo.ts

import type { LayoutLeafNode } from "../js/wire";

const { QuiApp } = require("../js/index.ts");

const WIDTH = 48;
const HEIGHT = 1;

const BRAND_TEAL = "#1f6f6f";
const BRAND_PLUM = "#6f1f6f";

/** One Secondary button pulling its look from `classes`. `size: "Content"`
 * sizes it to its own natural label width instead of an equal four-way
 * share of WIDTH, so no "[ ... ]" chrome clips. */
function button(leafId: string, label: string, classes: string): LayoutLeafNode {
  return {
    id: leafId,
    button: { id: leafId, label, variant: "Secondary" },
    size: "Content",
    class: classes,
  };
}

/** Owns the one piece of state this demo needs: which colour the
 * host-defined "brand" set currently holds. The render callback never
 * mentions a colour at all, only class names. */
class StyleClassesDemo {
  app: InstanceType<typeof QuiApp>;
  brandBg = BRAND_TEAL;

  constructor() {
    this.app = new QuiApp(WIDTH, HEIGHT);
    this.app.setRender(() => this._render());
    // "surface_bg" is the alias a Secondary button's fill actually reads
    // -- a plain "bg" would be a silent no-op on it, see
    // docs/bindings-wire-format.md's own per-leaf token table.
    this.app.defineStyle("brand", { surface_bg: this.brandBg });
  }

  private _render(): Record<string, unknown> {
    return {
      row: [
        button("save", "Save", "primary"),
        button("discard", "Discard", "brand"),
        // Both sets name surface_bg; the later one in the list wins it,
        // while "brand"'s other tokens still apply.
        button("reset", "Reset", "brand danger"),
        button("quiet", "Quiet", "subtle"),
      ],
      gap: 1,
    };
  }

  /** Swaps what "brand" means. Every node naming it repaints on the next
   * frame; the render callback is untouched. */
  redefineBrand(): void {
    this.brandBg = this.brandBg === BRAND_TEAL ? BRAND_PLUM : BRAND_TEAL;
    this.app.defineStyle("brand", { surface_bg: this.brandBg });
    this.app.step();
  }
}

function runDemo(): StyleClassesDemo {
  const demo = new StyleClassesDemo();
  demo.app.step();
  return demo;
}

exports.StyleClassesDemo = StyleClassesDemo;
exports.runDemo = runDemo;
exports.WIDTH = WIDTH;
exports.HEIGHT = HEIGHT;

if (require.main === module) {
  const demo = runDemo();
  console.log(`registered style sets: ${demo.app.styleSetNames().join(", ")}`);
  console.log(`\nwith "brand" = ${BRAND_TEAL}:`);
  console.log(demo.app.screen());
  demo.redefineBrand();
  console.log(`\nafter redefining "brand" to ${demo.brandBg}:`);
  console.log(demo.app.screen());
}
