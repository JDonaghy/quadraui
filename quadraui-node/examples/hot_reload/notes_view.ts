// The view half of `examples/hot_reload_demo.ts`: edit this file while the
// demo runs (change a label, the counter step, the layout) and the running
// app picks the edit up, keeping the counter value, the note list, and the
// list's selection. A syntax error leaves the last good version on screen
// with the error in the status bar until the file is fixed.
//
// Mirrors `quadraui-py/examples/hot_reload/notes_view.py`.
//
// Runtime exports go through `exports.X = ...` rather than `export`, the
// same reason `../../js/index.ts`'s own header comment gives.

const { Button, Column, Component, StatusBar, TextDisplay, Tree } = require("../../js/app/index.ts");

function line(text: string) {
  return { spans: [{ text, fg: null, bg: null, bold: false, italic: false, underline: false }] };
}

const FG = { r: 220, g: 220, b: 230, a: 255 };
const BG = { r: 30, g: 32, b: 48, a: 255 };

function segment(text: string) {
  return { text, fg: FG, bg: BG, bold: false };
}

function row(i: number, text: string) {
  return { path: [i], indent: 0, icon: null, text: line(text), badge: null, is_expanded: false };
}

/** Component-local state: survives every reload of this file. */
class Counter extends Component {
  initialState() {
    return { count: 0 };
  }

  view(state: any) {
    return Column([
      TextDisplay([line(`Counter: ${state.count}`)], { id: "hr:count" }),
      Button("Add one", {
        id: "hr:add",
        onClick: () => {
          state.count += 1;
        },
      }),
    ]);
  }
}
exports.Counter = Counter;

/**
 * App-object state (`this.notes`) survives a reload: the object is kept and
 * only its prototype is re-pointed at the edited class, which is why the
 * demo lists it in `enableHotReload`'s `preserve` option.
 */
class NotesApp {
  notes: string[] = ["edit examples/hot_reload/notes_view.ts", "save it", "watch this window"];
  app: any = null;

  addNote(): void {
    this.notes.push(`note ${this.notes.length + 1}`);
  }

  status(): string {
    if (this.app === null) {
      return " ready ";
    }
    if (this.app.reloadError !== null) {
      return ` reload failed: ${this.app.reloadError} `;
    }
    return ` reloads: ${this.app.reloadCount} `;
  }

  view(): unknown {
    return Column([
      TextDisplay([line("Hot reload demo")], { id: "hr:title" }),
      new Counter(),
      Button("Add note", { id: "hr:note", onClick: () => this.addNote() }),
      Tree(
        this.notes.map((note, i) => row(i, note)),
        { id: "hr:notes" },
      ),
      StatusBar([segment(this.status())], [], { id: "hr:status" }),
    ]);
  }
}
exports.NotesApp = NotesApp;
