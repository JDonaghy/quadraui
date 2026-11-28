# Quick start: Node and TypeScript

You need Node.js 22.18 or newer (22.6 to 22.17 work with
`--experimental-strip-types`), because the runners below are `.ts` files
that Node runs directly. `@quadraui/node` carries the Rust renderer as a
native addon.

> **Status: run it from a clone today.** `@quadraui/node` is first
> published to npm with quadraui 0.12
> ([#3079](https://github.com/JDonaghy/quadraui/issues/3079)). Until then
> this page runs your app through the clone's own loader, which CI uses.
> [From npm](#from-npm-after-012) shows the setup for an installed package;
> CI already runs it against `npm pack` tarballs of the clone.

## 1. Get quadraui

```sh
git clone https://github.com/JDonaghy/quadraui
cd quadraui/quadraui-node && npm ci && npm run build:debug && npm run build:dist && cd ..
```

`npm ci` installs the exact locked versions without touching
`package-lock.json`. npm 11 may warn that esbuild's install script was
blocked by `allow-scripts`; the loader works anyway, so you can ignore it.
`build:debug` compiles the native addon (it needs a Rust toolchain), and
`build:dist` compiles the package's TypeScript to `dist/`, where its
entry points live.

## 2. Write the app

Save this anywhere, for example `~/counter/counter.tsx`. It is the counter
from the README, and CI runs and type-checks this exact file
(`examples/hero/node/counter.tsx`):

<!-- sample: examples/hero/node/counter.tsx -->
```tsx
import { App } from "@quadraui/node/app";

class Counter {
  count = 0;

  view() {
    return (
      <column>
        <text id="count">Count: {this.count}</text>
        <row>
          <button id="dec" label="-" onClick={() => this.count--} />
          <button id="inc" label="+" onClick={() => this.count++} />
        </row>
      </column>
    );
  }
}

if (require.main === module) {
  const counter = new Counter();
  new App({ view: () => counter.view() }).runTui(); // q quits; or .runNative("Counter"): same Counter
}
module.exports = { Counter };
```

The three parts:

- **State** is the `Counter` object's fields.
- **View** is `view()`: it returns JSX (`<column>`, `<row>`, `<button>`)
  and runs again after every event, so a changed `count` shows up without
  `setState` or a redraw call.
- **Update** is the handlers: `onClick` on an element, and `onEvent` for
  input no single widget owns, such as a global shortcut.

Lowercase tags are quadraui's built-in widgets, typed from the same
generated schema as the Rust side, so a misspelled prop fails `tsc`, and
so does a wrong argument to `App`.

Some details the sample does not explain:

- **Text.** `<text>` is one line of plain text, sized to its content. For
  colours, use a `<textDisplay>`: it takes a list of lines, each a list of
  styled spans with optional `fg` and `bg`.
- **`new App(...)`** takes an optional size in character cells, `new
  App(30, 5, { view })`, for a headless run such as the test in step 5; it
  defaults to 80x24. A live `runTui()` fills the terminal and `runNative()`
  opens a window, and both ignore it.
- **Layout.** `<column>` and `<row>` split their space equally between the
  children that do not ask for a size. `<text>` asks for its content size,
  so the label takes one row and the buttons the rest. Any element takes
  `size` (pixels as a number, `"30%"`, `"2fr"` or `"content"`) and
  `min`/`max` ([Layout](../concepts/layout.md)).
- **Quitting.** Ctrl-C always quits, and `q` or Escape quit unless a
  focused widget used the key (a text field takes `q` as a letter). Call
  `app.quit()` from any handler to quit yourself, and pass `quitKeys:
  false` to turn the keys off.

## 3. Run it in the terminal

From the clone's root:

```sh
node examples/hero/node/run_tui.ts ~/counter/counter.tsx
```

`run_tui.ts` compiles the file's JSX in memory with esbuild, points
`@quadraui/node/*` at the clone, and runs it as the main module. With no
argument it runs the clone's own copy of the counter. Click `+` and `-`, or
Tab to a button and press Enter. `q` quits.

Run it in a real terminal (tmux and ssh are fine). The terminal backend
asks the terminal for its cursor position and capabilities at startup, so
it fails or hangs when input is piped or comes from `script`.

## 4. Run it in a native window

Change `.runTui()` to `.runNative("Counter")`, and build the addon
with a native backend (`libgtk-4-dev` is needed on Linux):

```sh
cd quadraui-node && npx napi build --platform --release --features macos && cd ..   # or gtk, win
node examples/hero/node/run_tui.ts ~/counter/counter.tsx
```

## 5. Test it headless

`@quadraui/node/testing` renders the app off screen. You click what was
painted, by its text, and read the frame back. From
`examples/hero/node/run_headless.ts`, which loads `counter.tsx` and the
two modules through the clone's loader:

<!-- sample: examples/hero/node/run_headless.ts -->
```ts
const { loadHero, SUBPATHS } = require("./load_jsx.ts");

const { Counter } = loadHero();
const { App } = require(SUBPATHS["@quadraui/node/app"]);
const { harness } = require(SUBPATHS["@quadraui/node/testing"]);

const counter = new Counter();
const h = harness(new App(30, 5, { view: () => counter.view() }));
if (!h.contains("Count: 0")) throw new Error(h.screen());
for (const label of ["+", "+"]) h.clickText(label);
if (!h.contains("Count: 2")) throw new Error(h.screen());
h.clickText("-");
console.log(h.screen());
if (!h.contains("Count: 1")) throw new Error(h.screen());
```

In a file you run with `run_tui.ts`, the same two modules are plain
`import { App } from "@quadraui/node/app"` and
`import { harness } from "@quadraui/node/testing"`.
The harness methods you will use most:

| Method | Does |
|---|---|
| `clickText(text, nth = 0)` | Clicks the `nth` place `text` is painted. |
| `clickAt(x, y)` | Clicks a cell. |
| `press(key, { shift, ctrl, alt, meta })` | Presses a named key (`"Enter"`, `"Escape"`, `"Down"`) or one character. |
| `type(text)` | Types each character in turn. |
| `contains(text)`, `assertText(...texts)` | Checks the frame. |
| `screen()`, `screenLines()` | The frame as text. |
| `find({ id, role, text })` | A widget, to read or act on (`.click()`, `.focus()`, `.row(...)`). |

**Two quick clicks are two clicks.** Clicks less than 400 ms and 1.5
cells apart fold into a double-click, which a list, table or tree row
treats as "open"; a button, or any widget with no double-click meaning,
sees both presses, so the two quick `+` clicks above count twice. Only the
left button folds: two quick right-clicks are two context-menu requests.

## 6. Next: a text field and a list

The counter uses two widgets. The
[parity app](https://github.com/JDonaghy/quadraui/tree/develop/examples/parity),
a todo list CI runs in all three languages, adds a text field, a list and
a status bar. From `examples/parity/node/todo.tsx`:

<!-- sample: examples/parity/node/todo.tsx -->
```tsx
const item = (text: string) => ({ text: { spans: [{ text }] } });
```

<!-- sample: examples/parity/node/todo.tsx -->
```tsx
  view() {
    const draft = TextInputField("New", this.draft, { id: "draft", onChange: (t: string) => this.edit(t), onSubmit: () => this.add() });
    return (
      <column>
        <form id="form" fields={[draft]} />
        <list id="todos" items={this.todos.map(item)} onSelect={(i) => this.select(i)} />
        <row>
          <button id="add" label="Add" onClick={() => this.add()} />
          <button id="clear" label="Clear" onClick={() => this.clear()} />
        </row>
        <statusBar id="status" leftSegments={[segment(`${this.todos.length} todos`)]} rightSegments={[segment(`selected ${this.selected}`)]} />
      </column>
    );
  }
```

- A list item is one line (`{ text: { spans: [...] } }`), unlike a
  `<textDisplay>`, which takes a list of lines.
- A text field is a `TextInputField(label, value, options)` from
  `@quadraui/node/app`, placed in a `<form>`'s `fields`. `onChange(text)`
  gets the whole new value after each keystroke; `onSubmit(text)` fires on
  Enter.
- `onSelect(index)` gets the selected row's index. Pass `selected` to pin
  the selection yourself ([State](../concepts/state.md)).
- The file's `log` records each update for the parity check; your app does
  not need it.

## Without JSX

Every tag is also a function exported by `@quadraui/node/app`. Content
comes first, positionally, and an options object second. Children are an
array:

| JSX | Function |
|---|---|
| `<column>...</column>` | `Column([...children], { gap, padding })` |
| `<row>...</row>` | `Row([...children], { gap, padding })` |
| `<button id="b" label="Go" onClick={f} />` | `Button("Go", { id: "b", onClick: f })` |
| `<text id="t">Hello</text>` | `Text("Hello", { id: "t" })` |
| `<textDisplay id="t" lines={lines} />` | `TextDisplay(lines, { id: "t" })` |
| `<list id="l" items={items} onSelect={f} />` | `List(items, { id: "l", onSelect: f })` |
| `<form id="f" fields={fields} />` | `Form(fields, { id: "f" })` |
| `<statusBar id="s" leftSegments={l} rightSegments={r} />` | `StatusBar(l, r, { id: "s" })` |

The node constructors take positional arguments, not a React-style props
object: `Button("+", { id: "inc", onClick })` and `Column([a, b])`, and
`tsc` rejects `Button({ id, label })`. A whole app in this style is
`quadraui-node/examples/sample_app.ts`.

## From npm (after 0.12)

Once `@quadraui/node` is on npm, a project of your own looks like this.
Until then, `npm pack` in the clone's `quadraui-node/` (and in
`quadraui-node/npm/<your platform>/`, with the built `.node` file copied
in) gives you tarballs to `npm install` instead.

```sh
mkdir counter && cd counter && npm init -y
npm install @quadraui/node
npm install --save-dev tsx typescript @types/node
```

```json
{
  "compilerOptions": {
    "strict": true,
    "module": "node16",
    "target": "es2020",
    "types": ["node"],
    "jsx": "react-jsx",
    "jsxImportSource": "@quadraui/node"
  }
}
```

`"module": "node16"` (or `"nodenext"`, or `"bundler"` with a bundler) is
what makes `tsc` read the package's `exports` map, so
`@quadraui/node/app` and the JSX runtime resolve with their types.
`@types/node` and `"types": ["node"]` let `tsc` see `require.main` and
`module`. Then `npx tsx counter.tsx` runs the app.

## More

- [The shape of an app](../concepts/index.md), and how it maps onto React,
  Elm, iced and Textual.
- [Events](../concepts/events.md): every callback, and the key names
  `keyName` returns.
- A larger app: `quadraui-node/examples/sample_app.ts` (list, form,
  confirm dialog, status bar) and `file_manager_app_jsx.tsx`.
- `App.enableHotReload(...)` reloads your view code and keeps state.
- [The same app in Rust and Python](../bindings.md).
