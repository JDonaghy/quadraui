// Hot reload with state preservation, on `js/app/index.ts`. Mirrors
// `quadraui-py/examples/hot_reload_demo.py`.
//
//     node examples/hot_reload_demo.ts --tui      # a real terminal
//     node examples/hot_reload_demo.ts --native   # a native window
//     node examples/hot_reload_demo.ts            # headless smoke run
//
// While it runs, edit `examples/hot_reload/notes_view.ts` and save: the
// window repaints with the new code within half a second, and the counter
// value, the note list and the list's selection all stay as they were.
// Save a syntax error and the last good version keeps running, with the
// error in the status bar, until the file is fixed. Press `q` or Escape
// (or close the window) to quit.
//
// The view lives in its own module because a reload re-requires a file by
// path, and the watch list is built with this module's own
// `require.resolve` so the relative path below is resolved the way an
// ordinary `require` of it would be.
//
// Note `{view: () => notes.view()}`, not `{view: notes.view}`: the
// closure reads `.view` off the preserved object at call time, so the
// prototype swap a reload performs makes the next render run the edited
// method. See `js/app/hotReload.ts`'s own doc for why a captured function
// cannot work in JS.
//
// Runtime exports go through `exports.X = ...` rather than `export`, the
// same reason `../js/index.ts`'s own header comment gives.

const { App, keyName } = require("../js/app/index.ts");
const notesView = require("./hot_reload/notes_view.ts");

const VIEW_MODULE = require.resolve("./hot_reload/notes_view.ts");

/** The demo's `App`, watching `notes_view.ts` for edits. */
function build(width = 60, height = 24, intervalMs: number | null = 500): [any, any] {
  const notes = new notesView.NotesApp();
  const app = new App(width, height, {
    view: () => notes.view(),
    onEvent: (event: unknown) => {
      const name = keyName(event);
      if (name === "q" || name === "Escape") {
        app.stopLive();
      }
    },
  });
  notes.app = app;
  app.enableHotReload([VIEW_MODULE], { interval: intervalMs, preserve: [notes] });
  return [app, notes];
}

exports.build = build;
exports.VIEW_MODULE = VIEW_MODULE;

if (require.main === module) {
  const flags = new Set(process.argv.slice(2));
  const [app] = build();
  if (flags.has("--tui")) {
    app.runTui();
  } else if (flags.has("--native")) {
    app.runNative("quadraui hot reload");
  } else {
    app.step();
    console.log(app.screen());
    app.disableHotReload();
  }
}
