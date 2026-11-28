import { App, TextInputField } from "@quadraui/node/app";

const item = (text: string) => ({ text: { spans: [{ text }] } });
const grey = { r: 220, g: 220, b: 220, a: 255 };
const navy = { r: 40, g: 40, b: 60, a: 255 };
const segment = (text: string) => ({ text: ` ${text} `, fg: grey, bg: navy });

export class Todo {
  draft = "";
  todos = ["Write the README", "Ship 0.12"];
  selected = 0;
  log: string[] = []; // every update, in order: what the parity runner compares

  view() {
    const draft = TextInputField("New", this.draft, { id: "draft", onChange: (t: string) => this.edit(t), onSubmit: () => this.add() });
    return (
      <column>
        <form id="form" size="content" fields={[draft]} />
        <list id="todos" items={this.todos.map(item)} onSelect={(i) => this.select(i)} />
        <row>
          <button id="add" label="Add" onClick={() => this.add()} />
          <button id="clear" label="Clear" onClick={() => this.clear()} />
        </row>
        <statusBar id="status" leftSegments={[segment(`${this.todos.length} todos`)]} rightSegments={[segment(`selected ${this.selected}`)]} />
      </column>
    );
  }

  edit(text: string) {
    this.log.push(`edit ${text}`);
    this.draft = text;
  }

  add() {
    this.log.push(`add ${this.draft}`);
    if (this.draft) {
      this.todos.push(this.draft);
      this.draft = "";
    }
  }

  select(index: number) {
    this.log.push(`select ${index}`);
    this.selected = index;
  }

  clear() {
    this.log.push("clear");
    this.todos = [];
    this.selected = 0;
  }
}

if (require.main === module) {
  const todo = new Todo();
  new App({ view: () => todo.view() }).runTui(); // Esc quits
}
