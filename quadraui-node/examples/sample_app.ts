// The Audit 5 sample app (list + detail form + confirm dialog + status
// bar + a keyboard shortcut) on `js/app/index.ts`'s typed nodes: no raw
// wire dict, no `onEvent` branch for `List`/`Form`/`MessageDialogResolved`.
// Up/Down (or a click) move the cursor, edit the title or toggle "Done",
// `d` (or "Delete") confirms before removing a task, `q`/Escape quits.
//     node examples/sample_app.ts [--tui]

const { App, Button, Column, Form, List, Row, StatusBar, TextInputField, ToggleField, keyName } = require("../js/app/index.ts");
import type { Renderable } from "../js/app/index";
const plain = (s: string) => ({ spans: [{ text: s, fg: null, bg: null }] });
class TaskApp {
  tasks: { title: string; done: boolean }[] = [
    { title: "Write report", done: false },
    { title: "Review PR", done: true },
    { title: "Ship release", done: false },
  ];
  selectedIdx = 0;
  message = "";
  app: InstanceType<typeof App>;
  constructor(width = 60, height = 14) {
    this.app = new App(width, height, { view: () => this._view(), onEvent: (e: unknown) => this._onEvent(e) });
  }
  private _confirmDelete(): void {
    const task = this.tasks[this.selectedIdx];
    if (!task) return;
    const buttons = [{ id: "delete", label: "Delete", is_default: true }, { id: "cancel", label: "Cancel", is_cancel: true }];
    this.app.confirm("Delete task", `Delete "${task.title}"?`, buttons).then((choice: string) => {
      if (choice !== "delete") return;
      this.tasks.splice(this.selectedIdx, 1);
      this.selectedIdx = Math.max(0, Math.min(this.tasks.length - 1, this.selectedIdx));
      this.message = "Deleted";
    });
  }

  private _view(): Renderable {
    const task = this.tasks[this.selectedIdx];
    const items = this.tasks.map((t) => ({ text: plain(t.done ? `[x] ${t.title}` : `[ ] ${t.title}`) }));
    const list = List(items, { id: "tasks:list", selected: this.selectedIdx, hasFocus: true, onSelect: (i: number) => (this.selectedIdx = i) });
    const fields = task
      ? [
          TextInputField("Title", task.title, { id: "tasks:form:title", onChange: (v: string) => (task.title = v) }),
          ToggleField("Done", task.done, { id: "tasks:form:done", onChange: (v: boolean) => (task.done = v) }),
        ]
      : [];
    const deleteBtn = Button("Delete", { id: "tasks:delete", onClick: () => this._confirmDelete() });
    const detail = Column([Form(fields, { id: "tasks:form" }), deleteBtn]);
    const segment = { text: ` ${this.tasks.length} task(s)  ${this.message} `, fg: { r: 220, g: 220, b: 230, a: 255 }, bg: { r: 30, g: 32, b: 48, a: 255 }, bold: false };
    return Column([Row([list, detail]), StatusBar([segment], [], { id: "tasks:status" })]);
  }

  private _onEvent(event: unknown): void {
    const name = keyName(event);
    if (name === null) return;
    this.message = "";
    if (name === "Down" || name === "Up") {
      this.selectedIdx = Math.max(0, Math.min(this.tasks.length - 1, this.selectedIdx + (name === "Down" ? 1 : -1)));
    } else if (name === "d") this._confirmDelete();
    else if (name === "q" || name === "Escape") this.app.stopLive();
  }
}

exports.TaskApp = TaskApp;
if (require.main === module) {
  const app = new TaskApp();
  if (new Set(process.argv.slice(2)).has("--tui")) app.app.runTui();
  else {
    app.app.step();
    console.log(app.app.screen());
  }
}
