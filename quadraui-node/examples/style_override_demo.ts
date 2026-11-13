// Runnable demo of the per-node "style" wire key: a "row" of three
// buttons ("Save" / "Discard" / "Cancel"), where only "Discard" carries
// its own `style: { surface_bg: ... }` override -- a scoped re-colour of
// exactly that button's own fill, not the row's, not its siblings'.
// Mirrors `quadraui-py/examples/style_override_demo.py`'s headless path
// 1:1 (this binding's live-session model is non-blocking `startTui`, not
// a blocking `run_tui` call, so only the headless path is ported here --
// see `theme.test.ts`'s own doc for that split).
//
// Toggling the override proves two things at once:
//
// - "surface_bg" is the semantic alias that actually reaches a
//   Secondary-variant button's fill -- a plain "bg" override, which only
//   ever reaches Theme.background, is a silent no-op on it (see
//   docs/bindings-wire-format.md's own per-leaf token table).
// - The override is purely per-node: "Save"/"Cancel" paint with the
//   default surface_bg on every frame, whether or not "Discard"'s own
//   override is active.
//
// Run headless (prints the screen before and after toggling the
// override):
//
//     node examples/style_override_demo.ts

const { QuiApp } = require("../js/index.ts");

const WIDTH = 32;
const HEIGHT = 1;

const DANGER_BG = "#aa2222";

/** Owns the one piece of state this demo needs: whether "Discard"'s own "style" override is active. */
class StyleOverrideDemo {
  app: InstanceType<typeof QuiApp>;
  discardStyled = false;

  constructor() {
    this.app = new QuiApp(WIDTH, HEIGHT);
    this.app.setRender(() => this._render());
  }

  private _render(): Record<string, unknown> {
    // `size: "Content"` sizes each button to its own natural label width
    // instead of an equal three-way share of WIDTH -- so "Discard"'s own
    // "[ Discard ]" chrome never clips.
    const discardLeaf: Record<string, unknown> = {
      id: "discard",
      button: { id: "discard", label: "Discard", variant: "Secondary" },
      size: "Content",
    };
    if (this.discardStyled) {
      discardLeaf.style = { surface_bg: DANGER_BG };
    }
    return {
      row: [
        { id: "save", button: { id: "save", label: "Save", variant: "Secondary" }, size: "Content" },
        discardLeaf,
        { id: "cancel", button: { id: "cancel", label: "Cancel", variant: "Secondary" }, size: "Content" },
      ],
      gap: 1,
    };
  }

  toggle(): void {
    this.discardStyled = !this.discardStyled;
    this.app.step();
  }
}

function runDemo(): StyleOverrideDemo {
  const demo = new StyleOverrideDemo();
  demo.app.step();
  return demo;
}

exports.StyleOverrideDemo = StyleOverrideDemo;
exports.runDemo = runDemo;
exports.WIDTH = WIDTH;
exports.HEIGHT = HEIGHT;

if (require.main === module) {
  const demo = runDemo();
  console.log('before toggling "Discard"\'s own style override:');
  console.log(demo.app.screen());
  demo.toggle();
  console.log('\nafter toggling "Discard"\'s own style override on:');
  console.log(demo.app.screen());
}
