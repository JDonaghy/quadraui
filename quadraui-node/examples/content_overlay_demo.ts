// Runnable demo of the two content-bearing overlay kinds: a popover and a
// modal dialog that each carry a whole layout subtree, not just chrome.
// `showOverlay("popover", ...)` / `showOverlay("layout_dialog", ...)` take
// a `{float|dialog, content}` descriptor whose `content` is any layout
// node -- here a button above a `form` with a text field and a toggle --
// and the leaves inside it take focus, route clicks and emit the same
// semantic events they would in the main tree.
//
// The demo drives it headlessly: it prints the base screen, the popover
// over it, the events a click on a nested toggle produced, the dialog
// over the same base, and what Escape resolves that dialog to.
//
// Run headless:
//
//     node examples/content_overlay_demo.ts

// Named apart from the other examples' own top-level bindings: Node runs
// each of these as a plain CommonJS script, but `npm run
// typecheck:examples` type-checks them in one `tsc --strict
// examples/*.ts` pass, where a script file's top-level `const`s all share
// one global scope.
const { QuiApp: OverlayQuiApp } = require("../js/index.ts");

const OVERLAY_DEMO_WIDTH = 60;
const OVERLAY_DEMO_HEIGHT = 20;

/** The `form` leaf both overlays embed: one text field and one toggle. */
function settingsForm(): Record<string, unknown> {
  return {
    id: "settings",
    form: {
      id: "settings",
      fields: [
        {
          id: "name",
          label: { spans: [{ text: "Name", fg: null, bg: null }] },
          kind: { TextInput: { value: "", placeholder: "", cursor: 0 } },
        },
        {
          id: "wrap",
          label: { spans: [{ text: "Word wrap", fg: null, bg: null }] },
          kind: { Toggle: { value: false } },
        },
      ],
    },
  };
}

// The button is the subtree's first tab stop deliberately: opening a
// content-bearing overlay seeds keyboard focus onto that stop, and the TUI
// focus ring paints a border box over the focused widget's own edge rows,
// which would otherwise sit on top of the form's first field row.
function overlayContent(buttonId: string, label: string): Record<string, unknown> {
  return {
    column: [
      { id: buttonId, button: { id: buttonId, label } },
      settingsForm(),
    ],
  };
}

/** Owns the demo's app plus every event its overlays emitted, in order. */
class ContentOverlayDemo {
  app: InstanceType<typeof OverlayQuiApp>;
  received: unknown[] = [];

  constructor() {
    this.app = new OverlayQuiApp(OVERLAY_DEMO_WIDTH, OVERLAY_DEMO_HEIGHT);
    this.app.setHandler((event: unknown) => {
      this.received.push(event);
    });
    this.app.setRender(() => ({
      column: [
        { id: "open", button: { id: "open", label: "Open settings" } },
      ],
    }));
    this.app.step();
  }

  /** Opens the popover, anchored under the base screen's own button. */
  openPopover(): void {
    this.app.showOverlay(
      "popover",
      {
        float: {
          id: "popover:settings",
          anchor: {
            rect: { x: 2.0, y: 1.0, width: 16.0, height: 1.0 },
            preferred: "Bottom",
            margin: 0.0,
          },
        },
        content: overlayContent("apply", "Apply"),
      },
      { width: 30.0, height: 8.0 },
    );
  }

  /** Opens the modal dialog, chrome and buttons around the same subtree. */
  openDialog(): void {
    this.app.showOverlay(
      "layout_dialog",
      {
        dialog: {
          id: "dialog:settings",
          title: "Settings",
          buttons: [
            { id: "save", label: "Save", is_default: true },
            { id: "cancel", label: "Cancel", is_cancel: true },
          ],
        },
        content: overlayContent("reset", "Reset"),
      },
      { width: 40.0, height: 6.0 },
    );
  }

  /** Clicks whatever cell `needle` starts at, and pumps the event. */
  clickOn(needle: string): void {
    const lines: string[] = this.app.screen().split("\n");
    for (let row = 0; row < lines.length; row++) {
      const col = lines[row].indexOf(needle);
      if (col !== -1) {
        this.app.pushEvent({
          MouseDown: {
            button: "Left",
            position: { x: col, y: row },
            modifiers: {},
          },
        });
        this.app.run(1);
        return;
      }
    }
    throw new Error(`${JSON.stringify(needle)} not found on screen`);
  }

  /** Sends Escape, which dismisses a popover and resolves a dialog. */
  escape(): void {
    this.app.pushEvent({
      KeyPressed: { key: { Named: "Escape" }, modifiers: {}, repeat: false },
    });
    this.app.run(1);
  }
}

exports.ContentOverlayDemo = ContentOverlayDemo;
exports.WIDTH = OVERLAY_DEMO_WIDTH;
exports.HEIGHT = OVERLAY_DEMO_HEIGHT;

if (require.main === module) {
  const demo = new ContentOverlayDemo();
  console.log("base screen:");
  console.log(demo.app.screen());

  demo.openPopover();
  console.log("\npopover with a button + form subtree inside it:");
  console.log(demo.app.screen());

  demo.clickOn("Word wrap");
  console.log("\nevents from clicking the nested toggle:");
  console.log(JSON.stringify(demo.received, null, 2));
  demo.escape();

  demo.openDialog();
  console.log("\nmodal dialog with the same subtree in its body:");
  console.log(demo.app.screen());

  demo.received.length = 0;
  demo.escape();
  console.log("\nevents from pressing Escape on the dialog:");
  console.log(JSON.stringify(demo.received, null, 2));
  console.log("\nscreen after the dialog resolved:");
  console.log(demo.app.screen());
}
