// Unit tests for `js/app/`: id auto-assignment stability across re-render,
// callback dispatch (`Tree`, `Table`, `List`, `Tabs`, `Button`), component
// state survival, and redraw-on-mutation -- the four properties the app layer's
// own module doc (`js/app/index.ts`) promises and `examples/file_manager_app.ts`
// relies on wholesale. Mirrors `quadraui-py/tests/test_app_layer.py` 1:1.
//
// Run with `npm run build:debug` (or `npm run build`) then `npm test`.
const { test } = require("node:test");
const assert = require("node:assert/strict");

const {
  App,
  Button,
  Checkbox,
  Column,
  Component,
  DropdownField,
  Form,
  List,
  Password,
  PasswordField,
  Select,
  SidebarPanel,
  Slider,
  SliderField,
  Split,
  StatusBar,
  Table,
  Tabs,
  Tab,
  TextArea,
  TextAreaField,
  TextDisplay,
  TextInputField,
  ToggleField,
  Toolbar,
  Tree,
} = require("../js/app/index.ts");

const NO_MODS = { shift: false, ctrl: false, alt: false, cmd: false };

function keyEvent(key: unknown) {
  return { KeyPressed: { key, modifiers: NO_MODS, repeat: false } };
}

// --- id auto-assignment -----------------------------------------------------

test("auto-assigned ids are stable across a re-render at the same position", () => {
  const calls: number[] = [];
  const app = new App(20, 5, { view: () => Button("Save", { onClick: () => calls.push(1) }) });

  const idsBefore = new Set(Object.keys(app._registry));
  assert.ok(app.screenContains("Save"));
  app.step();
  const idsAfter = new Set(Object.keys(app._registry));
  assert.deepEqual(idsBefore, idsAfter);
  assert.equal(idsBefore.size, 1);
});

test("auto ids differ by structural position", () => {
  const app = new App(20, 5, {
    view: () => Column([Button("A", { onClick: () => {} }), Button("B", { onClick: () => {} })]),
  });
  assert.ok(app.screenContains("A"));
  const ids = Object.keys(app._registry).sort();
  assert.equal(ids.length, 2);
  assert.notEqual(ids[0], ids[1]);
});

test("every id-bearing node constructor accepts an omitted id", () => {
  const app = new App(20, 5, {
    view: () => {
      const tree = Tree([], { onActivate: () => {} });
      const table = Table([], []);
      const lst = List([], { onSelect: () => {} });
      const status = StatusBar([], []);
      const text = TextDisplay([]);
      const tabs = Tabs([Tab(text, { title: "one" })]);
      const panel = SidebarPanel(Column([tree, table, lst, status, tabs]));
      return Split(panel, text);
    },
  });
  assert.notEqual(app._render(), null);
});

test("auto ids from every node constructor are stable across re-render", () => {
  const app = new App(20, 5, {
    view: () =>
      Split(Tree([], { onRowClick: () => {} }), Table([], [], { onSelect: () => {} })),
  });
  const idsBefore = Object.keys(app._registry).sort();
  app.step();
  const idsAfter = Object.keys(app._registry).sort();
  assert.deepEqual(idsBefore, idsAfter);
  assert.equal(idsBefore.length, 2);
});

test("Toolbar without an explicit id still builds a wire object", () => {
  const toolbar = Toolbar([{ Action: { id: "a", label: "Go" } }]);
  assert.equal(typeof toolbar.id, "string");
  assert.deepEqual(toolbar.buttons, [{ Action: { id: "a", label: "Go" } }]);

  const other = Toolbar([]);
  assert.notEqual(other.id, toolbar.id);
});

// --- callback dispatch -------------------------------------------------------

test("tree row click and toggle-expand dispatch to their own callback", () => {
  const clicked: unknown[] = [];
  const toggled: unknown[] = [];

  const app = new App(20, 5, {
    view: () =>
      Tree(
        [{ path: [0], indent: 0, icon: null, text: { spans: [] }, badge: null, is_expanded: false }],
        {
          id: "t:tree",
          onRowClick: (path: unknown) => clicked.push(path),
          onToggleExpand: (path: unknown) => toggled.push(path),
        },
      ),
  });

  app.pushEvent({ Tree: ["t:tree", { RowClicked: { path: [0], modifiers: NO_MODS } }] });
  app.step();
  assert.deepEqual(clicked, [[0]]);
  assert.deepEqual(toggled, []);

  app.pushEvent({ Tree: ["t:tree", { RowToggleExpand: { path: [0] } }] });
  app.step();
  assert.deepEqual(toggled, [[0]]);
});

test("table row double-clicked dispatches onActivate, not onSelect", () => {
  const activated: unknown[] = [];
  const selected: unknown[] = [];

  const app = new App(20, 5, {
    view: () =>
      Table(
        [{ title: "Name", width: { Flex: 1.0 }, align: "Left" }],
        [{ cells: [{ spans: [{ text: "a", fg: null, bg: null }] }] }],
        {
          id: "t:grid",
          onActivate: (idx: unknown) => activated.push(idx),
          onSelect: (idx: unknown) => selected.push(idx),
        },
      ),
  });

  app.pushEvent({ DataTable: ["t:grid", { RowSelected: { idx: 0 } }] });
  app.step();
  assert.deepEqual(selected, [0]);
  assert.deepEqual(activated, []);

  app.pushEvent({ DataTable: ["t:grid", { RowDoubleClicked: { idx: 0 } }] });
  app.step();
  assert.deepEqual(activated, [0]);
});

test("list selection changed dispatches onSelect, not onActivate", () => {
  const selected: unknown[] = [];
  const activated: unknown[] = [];

  const app = new App(20, 5, {
    view: () =>
      List([{ text: { spans: [{ text: "a", fg: null, bg: null }] } }], {
        id: "t:list",
        onSelect: (idx: unknown) => selected.push(idx),
        onActivate: (idx: unknown) => activated.push(idx),
      }),
  });

  app.pushEvent({ List: ["t:list", { SelectionChanged: { idx: 0 } }] });
  app.step();
  assert.deepEqual(selected, [0]);
  assert.deepEqual(activated, []);
});

test("list item double-clicked and item activated both dispatch onActivate", () => {
  const activated: unknown[] = [];

  const app = new App(20, 5, {
    view: () =>
      List([{ text: { spans: [{ text: "a", fg: null, bg: null }] } }], {
        id: "t:list",
        onActivate: (idx: unknown) => activated.push(idx),
      }),
  });

  app.pushEvent({ List: ["t:list", { ItemDoubleClicked: { idx: 0 } }] });
  app.step();
  assert.deepEqual(activated, [0]);

  app.pushEvent({ List: ["t:list", { ItemActivated: { idx: 0 } }] });
  app.step();
  assert.deepEqual(activated, [0, 0]);
});

test("list without selected defaults selected_idx to zero", () => {
  const app = new App(20, 5, { view: () => List([], { id: "t:list" }) });
  const wire = app._render() as Record<string, Record<string, unknown>>;
  assert.equal(wire.list.selected_idx, 0);
});

test("list selected/title/bordered reach the payload when passed", () => {
  const app = new App(20, 5, {
    view: () =>
      List([], {
        id: "t:list",
        selected: 2,
        title: { spans: [{ text: "Files", fg: null, bg: null }] },
        bordered: true,
      }),
  });
  const payload = (app._render() as Record<string, Record<string, unknown>>).list;
  assert.equal(payload.selected_idx, 2);
  assert.deepEqual(payload.title, { spans: [{ text: "Files", fg: null, bg: null }] });
  assert.equal(payload.bordered, true);
});

test("list title and bordered are omitted when not passed", () => {
  const app = new App(20, 5, { view: () => List([], { id: "t:list" }) });
  const payload = (app._render() as Record<string, Record<string, unknown>>).list;
  assert.ok(!("title" in payload));
  assert.ok(!("bordered" in payload));
});

test("list KeyPressed falls through to onEvent", () => {
  // A focus-scoped `{List: [id, {KeyPressed: ...}]}` is not one of `List`'s
  // own callback variants (`SelectionChanged`/`ItemDoubleClicked`/
  // `ItemActivated`), so it must still reach the global `onEvent` escape
  // hatch rather than being silently swallowed.
  const fallenThrough: unknown[] = [];

  const app = new App(20, 5, {
    view: () => List([], { id: "t:list", onSelect: () => {} }),
    onEvent: (event: unknown) => fallenThrough.push(event),
  });
  app.pushEvent({ List: ["t:list", { KeyPressed: { key: "j", modifiers: NO_MODS } }] });
  app.step();
  assert.equal(fallenThrough.length, 1);
  const [widgetId, activation] = (fallenThrough[0] as Record<string, unknown>).List as [string, unknown];
  assert.equal(widgetId, "t:list");
  assert.deepEqual(activation, { KeyPressed: { key: "j", modifiers: NO_MODS } });
});

test("list SelectionChanged with no onSelect falls through to onEvent", () => {
  const fallenThrough: unknown[] = [];

  const app = new App(20, 5, {
    view: () => List([], { id: "t:list" }),
    onEvent: (event: unknown) => fallenThrough.push(event),
  });
  app.pushEvent({ List: ["t:list", { SelectionChanged: { idx: 0 } }] });
  app.step();
  assert.equal(fallenThrough.length, 1);
  const [widgetId, activation] = (fallenThrough[0] as Record<string, unknown>).List as [string, unknown];
  assert.equal(widgetId, "t:list");
  assert.deepEqual(activation, { SelectionChanged: { idx: 0 } });
});

// --- form / form fields -----------------------------------------------

test("TextInputField dispatches onChange per keystroke and onSubmit on commit", () => {
  const changes: string[] = [];
  const submits: string[] = [];

  const app = new App(40, 5, {
    view: () =>
      Form(
        [
          TextInputField("Name", "Alice", {
            id: "f:name",
            onChange: (value: string) => changes.push(value),
            onSubmit: (value: string) => submits.push(value),
          }),
        ],
        { id: "t:settings" },
      ),
  });

  // The outer tuple id is the `Form`'s own id (`t:settings`), never the
  // field's -- `FormEvent`'s own `id` (here `f:name`) carries the field.
  app.pushEvent({ Form: ["t:settings", { TextInputChanged: { id: "f:name", value: "Alicia" } }] });
  app.step();
  app.pushEvent({ Form: ["t:settings", { TextInputCommitted: { id: "f:name", value: "Alicia" } }] });
  app.step();
  assert.deepEqual(changes, ["Alicia"]);
  assert.deepEqual(submits, ["Alicia"]);
});

test("ToggleField dispatches onChange with the new value", () => {
  const changes: boolean[] = [];

  const app = new App(40, 5, {
    view: () =>
      Form([ToggleField("Enabled", false, { id: "f:enabled", onChange: (value: boolean) => changes.push(value) })], {
        id: "t:settings",
      }),
  });

  app.pushEvent({ Form: ["t:settings", { ToggleChanged: { id: "f:enabled", value: true } }] });
  app.step();
  assert.deepEqual(changes, [true]);
});

test("DropdownField dispatches onChange with the selected index", () => {
  const changes: number[] = [];
  const choices = [{ spans: [{ text: "A", fg: null, bg: null }] }, { spans: [{ text: "B", fg: null, bg: null }] }];

  const app = new App(40, 5, {
    view: () =>
      Form([DropdownField("Role", choices, 0, { id: "f:role", onChange: (idx: number) => changes.push(idx) })], {
        id: "t:settings",
      }),
  });

  app.pushEvent({ Form: ["t:settings", { DropdownChanged: { id: "f:role", selected_idx: 1 } }] });
  app.step();
  assert.deepEqual(changes, [1]);
});

test("SliderField dispatches onChange with the new value", () => {
  const changes: number[] = [];

  const app = new App(40, 5, {
    view: () =>
      Form([SliderField("Volume", 5, 0, 10, { id: "f:vol", onChange: (value: number) => changes.push(value) })], {
        id: "t:settings",
      }),
  });

  app.pushEvent({ Form: ["t:settings", { SliderChanged: { id: "f:vol", value: 7 } }] });
  app.step();
  assert.deepEqual(changes, [7]);
});

test("PasswordField dispatches onChange per keystroke and onSubmit on commit", () => {
  const changes: string[] = [];
  const submits: string[] = [];

  const app = new App(40, 5, {
    view: () =>
      Form(
        [
          PasswordField("Secret", "", {
            id: "f:pwd",
            onChange: (value: string) => changes.push(value),
            onSubmit: (value: string) => submits.push(value),
          }),
        ],
        { id: "t:settings" },
      ),
  });

  app.pushEvent({ Form: ["t:settings", { TextInputChanged: { id: "f:pwd", value: "s3cr3t" } }] });
  app.step();
  app.pushEvent({ Form: ["t:settings", { TextInputCommitted: { id: "f:pwd", value: "s3cr3t" } }] });
  app.step();
  assert.deepEqual(changes, ["s3cr3t"]);
  assert.deepEqual(submits, ["s3cr3t"]);
});

test("TextAreaField dispatches onChange but never onSubmit (Enter inserts a newline, not a commit)", () => {
  const changes: string[] = [];

  const app = new App(40, 5, {
    view: () =>
      Form([TextAreaField("Notes", "hi", { id: "f:notes", onChange: (value: string) => changes.push(value) })], {
        id: "t:settings",
      }),
  });

  app.pushEvent({ Form: ["t:settings", { TextInputChanged: { id: "f:notes", value: "hi\nthere" } }] });
  app.step();
  assert.deepEqual(changes, ["hi\nthere"]);
  // No `FORM_FIELD_EVENT_MAP` entry claims `TextInputCommitted` under
  // `on_submit` for this field -- `TextAreaField` never registers one --
  // so sending it anyway must fall through rather than throw.
  app.pushEvent({ Form: ["t:settings", { TextInputCommitted: { id: "f:notes", value: "hi\nthere" } }] });
  app.step();
  assert.deepEqual(changes, ["hi\nthere"]);
});

test("Form resolves an unlabeled field's own id from its position, like Tabs' own tabs", () => {
  const changes: boolean[] = [];

  const app = new App(40, 5, {
    view: () =>
      Form(
        [
          TextInputField("Name", "Alice"),
          ToggleField("Enabled", false, { onChange: (value: boolean) => changes.push(value) }),
        ],
        { id: "t:form" },
      ),
  });

  const wire = app._render() as Record<string, Record<string, unknown>>;
  const fields = wire.form.fields as Array<Record<string, unknown>>;
  assert.equal(fields[0].id, "t:form:field:0");
  assert.equal(fields[1].id, "t:form:field:1");

  app.pushEvent({ Form: ["t:form", { ToggleChanged: { id: "t:form:field:1", value: true } }] });
  app.step();
  assert.deepEqual(changes, [true]);
});

test("a Form field with no onChange falls through to onEvent", () => {
  const fallenThrough: unknown[] = [];

  const app = new App(40, 5, {
    view: () => Form([ToggleField("Enabled", false, { id: "f:enabled" })], { id: "t:settings" }),
    onEvent: (event: unknown) => fallenThrough.push(event),
  });

  app.pushEvent({ Form: ["t:settings", { ToggleChanged: { id: "f:enabled", value: true } }] });
  app.step();
  assert.equal(fallenThrough.length, 1);
});

test("standalone Checkbox/Slider/Select/Password/TextArea leaves dispatch onChange the same way a Form field does", () => {
  const changes: Record<string, unknown[]> = { checkbox: [], slider: [], select: [], password: [], textarea: [] };

  const app = new App(40, 5, {
    view: () =>
      Column([
        Checkbox(false, { id: "l:checkbox", onChange: (v: boolean) => changes.checkbox.push(v) }),
        Slider(0, 0, 10, { id: "l:slider", onChange: (v: number) => changes.slider.push(v) }),
        Select(["One", "Two"], { id: "l:select", onChange: (v: number) => changes.select.push(v) }),
        Password("", { id: "l:password", onChange: (v: string) => changes.password.push(v) }),
        TextArea("", { id: "l:textarea", onChange: (v: string) => changes.textarea.push(v) }),
      ]),
  });

  app.pushEvent({ Form: ["l:checkbox", { ToggleChanged: { id: "l:checkbox", value: true } }] });
  app.step();
  app.pushEvent({ Form: ["l:slider", { SliderChanged: { id: "l:slider", value: 3 } }] });
  app.step();
  app.pushEvent({ Form: ["l:select", { DropdownChanged: { id: "l:select", selected_idx: 1 } }] });
  app.step();
  app.pushEvent({ Form: ["l:password", { TextInputChanged: { id: "l:password", value: "hi" } }] });
  app.step();
  app.pushEvent({ Form: ["l:textarea", { TextInputChanged: { id: "l:textarea", value: "hi" } }] });
  app.step();

  assert.deepEqual(changes, {
    checkbox: [true],
    slider: [3],
    select: [1],
    password: ["hi"],
    textarea: ["hi"],
  });
});

test("standalone Password dispatches onSubmit on TextInputCommitted", () => {
  const submits: string[] = [];

  const app = new App(40, 5, {
    view: () => Password("", { id: "l:password", onSubmit: (v: string) => submits.push(v) }),
  });

  app.pushEvent({ Form: ["l:password", { TextInputCommitted: { id: "l:password", value: "hi" } }] });
  app.step();
  assert.deepEqual(submits, ["hi"]);
});

test("a Form field's on_change is keyed off the field's own id, not the wrapping Form's", () => {
  // `FormEvent`'s own `id` is the field's, never the wrapping `Form`'s
  // (see `quadraui::primitives::form::FormEvent`'s own doc) -- so a
  // field's `onChange` must be looked up by its own id, and a payload
  // naming the `Form`'s id instead must not accidentally match it.
  const seen: boolean[] = [];

  const app = new App(20, 5, {
    view: () => Form([ToggleField("Enabled", true, { id: "f:toggle", onChange: (v: boolean) => seen.push(v) })], { id: "t:form" }),
  });

  app.pushEvent({ Form: ["t:form", { ToggleChanged: { id: "t:form", value: false } }] });
  app.step();
  assert.deepEqual(seen, [], "the Form's own id must not be mistaken for the field's");

  app.pushEvent({ Form: ["t:form", { ToggleChanged: { id: "f:toggle", value: false } }] });
  app.step();
  assert.deepEqual(seen, [false]);
});

test("Checkbox onChange fires on a real hit-tested click, through the engine's own dispatch", () => {
  // End-to-end, through the real engine's own hit-test and semantic
  // dispatch -- not a hand-built `Form` event -- the same way "button
  // onClick fires on a hit-tested MouseDown" proves `Button`'s own
  // wiring.
  const seen: boolean[] = [];

  const app = new App(20, 5, {
    view: () => Checkbox(true, { id: "t:cb", onChange: (v: boolean) => seen.push(v) }),
  });

  clickAt(app);
  assert.deepEqual(seen, [false]);
});

test("tabs closed dispatches onClose with the index", () => {
  const closed: unknown[] = [];

  const app = new App(20, 5, {
    view: () =>
      Tabs([Tab(TextDisplay([], { id: "t:c0" }), { title: "one" })], {
        id: "t:tabs",
        onClose: (idx: unknown) => closed.push(idx),
      }),
  });

  app.pushEvent({ TabBar: ["t:tabs", { TabClosed: { index: 0 } }] });
  app.step();
  assert.deepEqual(closed, [0]);
});

function clickAt(app: any, x = 0, y = 0) {
  app.pushEvent({ MouseDown: { button: "Left", position: { x, y }, modifiers: {} } });
  app.step();
}

test("button onClick fires on a hit-tested MouseDown", () => {
  const clicks: number[] = [];
  const app = new App(20, 5, { view: () => Button("Save", { id: "t:save", onClick: () => clicks.push(1) }) });
  clickAt(app);
  assert.deepEqual(clicks, [1]);
});

test("button onClick fires on Enter while focused", () => {
  const clicks: number[] = [];
  const app = new App(20, 5, {
    view: () => Button("Save", { id: "t:save", onClick: () => clicks.push(1), focused: true }),
  });
  app.pushEvent({ FocusChanged: "t:save" });
  app.step();
  app.pushEvent(keyEvent({ Named: "Enter" }));
  app.step();
  assert.deepEqual(clicks, [1]);
});

test("button onClick fires on Enter after a click moved real focus to it", () => {
  // A click leaves the *backend's* own focus on the button, not just this
  // layer's `_focusedId`, so the Enter that follows resolves to the button's
  // semantic `Form`/`ButtonClicked` activation instead of a raw `KeyPressed`.
  // Both routes must reach the same `onClick`.
  const clicks: number[] = [];
  const fallback: unknown[] = [];
  const app = new App(20, 5, {
    view: () => Button("Save", { id: "t:save", onClick: () => clicks.push(1) }),
    onEvent: (event: unknown) => fallback.push(event),
  });

  clickAt(app);
  assert.deepEqual(clicks, [1], "the click itself must fire onClick exactly once");
  assert.ok(
    !fallback.some((event) => typeof event === "object" && event !== null && "Form" in event),
    "a button's own activation must never reach the onEvent escape hatch",
  );

  app.pushEvent(keyEvent({ Named: "Enter" }));
  app.step();
  assert.deepEqual(clicks, [1, 1], "Enter on the clicked (and so really focused) button must fire too");
});

// --- confirm / alert ---------------------------------------------------------

function locateText(app: any, needle: string): [number, number] {
  const lines = app.screen().split("\n");
  for (let row = 0; row < lines.length; row++) {
    const col = lines[row].indexOf(needle);
    if (col !== -1) {
      return [row, col];
    }
  }
  throw new Error(`${JSON.stringify(needle)} not found on screen`);
}

function clickText(app: any, needle: string) {
  const [row, col] = locateText(app, needle);
  clickAt(app, col, row);
}

test("confirm resolves with the id of whichever button is clicked", async () => {
  const app = new App(40, 10, { view: () => TextDisplay([], { id: "t:text" }) });
  const pending = app.confirm("Unsaved changes", "Do you want to save?", [
    { id: "save", label: "Save", is_default: true },
    { id: "discard", label: "Discard" },
    { id: "cancel", label: "Cancel", is_cancel: true },
  ]);
  app.step();
  assert.ok(app.screenContains("Save"), "the dialog must be painted once step() applies the queued action");

  clickText(app, "Discard");
  assert.equal(await pending, "discard");
});

test("confirm resolves the default button on Enter", async () => {
  const app = new App(40, 10, { view: () => TextDisplay([], { id: "t:text" }) });
  const pending = app.confirm("Unsaved changes", "Do you want to save?", [
    { id: "save", label: "Save", is_default: true },
    { id: "cancel", label: "Cancel", is_cancel: true },
  ]);
  app.step();
  app.pushEvent(keyEvent({ Named: "Enter" }));
  app.step();
  assert.equal(await pending, "save");
});

test("confirm resolves the cancel button on Escape", async () => {
  const app = new App(40, 10, { view: () => TextDisplay([], { id: "t:text" }) });
  const pending = app.confirm("Unsaved changes", "Do you want to save?", [
    { id: "save", label: "Save", is_default: true },
    { id: "cancel", label: "Cancel", is_cancel: true },
  ]);
  app.step();
  app.pushEvent(keyEvent({ Named: "Escape" }));
  app.step();
  assert.equal(await pending, "cancel");
});

test("alert resolves with ok (its default button id) from any dismissal route", async () => {
  const app = new App(40, 10, { view: () => TextDisplay([], { id: "t:text" }) });
  const pending = app.alert("Heads up", "Something happened.");
  app.step();
  assert.ok(app.screenContains("OK"));
  app.pushEvent(keyEvent({ Named: "Enter" }));
  app.step();
  assert.equal(await pending, "ok");
});

test("alert's button id/label are overridable", async () => {
  const app = new App(40, 10, { view: () => TextDisplay([], { id: "t:text" }) });
  const pending = app.alert("Heads up", "Something happened.", { id: "dismiss", label: "Got it" });
  app.step();
  assert.ok(app.screenContains("Got it"));
  clickText(app, "Got it");
  assert.equal(await pending, "dismiss");
});

test("a MessageDialogResolved with no pending confirm/alert falls through to onEvent", () => {
  const seen: unknown[] = [];
  const app = new App(40, 10, {
    view: () => TextDisplay([], { id: "t:text" }),
    onEvent: (event: unknown) => seen.push(event),
  });
  // Opened through the lower-level `actions` escape hatch directly, not
  // through `confirm`/`alert` -- its resolution must not be swallowed.
  app.actions.showMessageDialog("Title", "Body", [{ id: "ok", label: "OK", is_default: true }], undefined);
  app.step();
  app.pushEvent(keyEvent({ Named: "Enter" }));
  app.step();
  assert.deepEqual(seen, [{ MessageDialogResolved: "ok" }]);
});

test("an unmatched event falls through to onEvent", () => {
  const seen: unknown[] = [];
  const { keyName } = require("../js/app/index.ts");
  const app = new App(20, 5, {
    view: () => TextDisplay([], { id: "t:text" }),
    onEvent: (event: unknown) => seen.push(event),
  });
  app.pushEvent(keyEvent({ Named: "Down" }));
  app.step();
  assert.equal(seen.length, 1);
  assert.equal(keyName(seen[0]), "Down");
});

// --- strict / onUnknownKey ----------------------------------------------------

test("strict:true raises on a typo'd optional key in view()'s own tree", () => {
  const { UnknownKeyError } = require("../js/index.ts");
  // `App`'s constructor already forces one real render (see its own doc),
  // so the typo is caught there, not on a later `step()`.
  assert.throws(
    () =>
      new App(20, 5, {
        strict: true,
        // "focussed" is a typo for "focused_index" on a tabs descriptor.
        view: () => ({ tabs: { id: "frm", tabs: [], focussed: 0 } }),
      }),
    (err: unknown) => {
      assert.ok(err instanceof UnknownKeyError);
      assert.match((err as Error).message, /focussed/);
      return true;
    },
  );
});

test("onUnknownKey reports a typo'd optional key once, without strict", () => {
  const calls: Array<{ key: string; path: string }> = [];
  const app = new App(20, 5, {
    onUnknownKey: (info) => calls.push(info),
    view: () => ({ tabs: { id: "frm", tabs: [], focussed: 0 } }),
  });
  app.step();
  app.step();
  assert.deepEqual(calls, [{ key: "focussed", path: "render › tabs#frm" }]);
});

// --- components --------------------------------------------------------------

class Counter extends Component {
  initialState() {
    return { count: 0 };
  }
  view(state: Record<string, unknown>) {
    return Button(`count=${state.count}`, {
      focused: true,
      onClick: () => {
        state.count = (state.count as number) + 1;
      },
    });
  }
}

function label(app: any): string {
  return app._render().button.label;
}

test("component state survives re-render and a mutation is visible next render", () => {
  const app = new App(20, 5, { view: () => new Counter() });
  assert.equal(label(app), "count=0");

  const buttonId = Object.keys(app._registry)[0];
  app.pushEvent({ FocusChanged: buttonId });
  app.step();

  for (const expected of ["count=1", "count=2", "count=3"]) {
    app.pushEvent(keyEvent({ Named: "Enter" }));
    app.step();
    assert.equal(label(app), expected);
    assert.ok(app.screenContains(expected));
  }
});

test("two components in a list keep independent state by position", () => {
  const app = new App(20, 5, { view: () => Column([new Counter(), new Counter()]) });
  const ids = Object.keys(app._registry).sort();
  assert.equal(ids.length, 2);

  app._registry[ids[0]].on_click();
  app.step();

  const screen = app.screen();
  assert.ok(screen.includes("count=1"));
  assert.ok(screen.includes("count=0"));
});

test("closing and reopening a row does not resurrect the old state", () => {
  let counters = [new Counter(), new Counter()];
  const app = new App(20, 5, { view: () => Column(counters) });
  const ids = Object.keys(app._registry).sort();
  assert.equal(ids.length, 2);

  for (let i = 0; i < 2; i++) {
    app._registry[ids[1]].on_click();
  }
  assert.ok(app.screen().includes("count=2"));
  assert.equal(app._componentStates.size, 2);

  // Render with only position 0 present -- position 1's slot must be
  // pruned, since nothing visited it this render.
  counters = [counters[0]];
  app.screen();
  assert.equal(app._componentStates.size, 1);

  // A brand-new `Counter` rendered back at position 1 starts fresh.
  counters = [counters[0], new Counter()];
  const screen = app.screen();
  assert.ok(!screen.includes("count=2"));
  assert.equal(screen.split("count=0").length - 1, 2);
});

// --- wire_version --------------------------------------------------------

test("rendered tree carries the current wire_version", () => {
  // Every tree this layer renders gets a `"wire_version"` sibling key added
  // automatically (see `runtime.ts`'s `_renderOnce`) -- a plain `Button`
  // tree with no version opinion of its own still decodes and renders, the
  // same way a hand-built wire object carrying the current
  // `runtime.WIRE_VERSION` already does for `QuiApp` directly.
  const runtime = require("../js/app/runtime.ts");
  const app = new App(20, 5, { view: () => Button("OK", { onClick: () => {} }) });
  assert.ok(app.screenContains("OK"));
  assert.equal((app._render() as Record<string, unknown>).wire_version, runtime.WIRE_VERSION);
});

test("a raw escape-hatch object that sets its own wire_version wins", () => {
  // A `view()` that returns a raw wire object (the escape hatch `Node`
  // constructors bypass) and already set its own `"wire_version"` is left
  // untouched -- this layer only fills the key in when it's missing.
  const app = new App(20, 5, {
    view: () => ({ wire_version: 1, button: { id: "b", label: "OK" } }),
  });
  assert.equal((app._render() as Record<string, unknown>).wire_version, 1);
});

test("forcing a wire_version newer than this host understands raises", () => {
  // Bumping `runtime.WIRE_VERSION` past what the native extension's own
  // `wire_schema::WIRE_VERSION` accepts makes the host's version gate
  // reject the next render with `VersionMismatchError`, exactly as it
  // would for a hand-built wire object carrying the same inflated value.
  const runtime = require("../js/app/runtime.ts");
  const { VersionMismatchError } = require("../js/index.ts");
  const app = new App(20, 5, { view: () => Button("OK", { onClick: () => {} }) });
  try {
    runtime.WIRE_VERSION = 999;
    assert.throws(
      () => app.screen(),
      (err: unknown) => {
        assert.ok(err instanceof VersionMismatchError);
        assert.match((err as Error).message, /wire_version/);
        return true;
      },
    );
  } finally {
    runtime.WIRE_VERSION = 1;
  }
});
