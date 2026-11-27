# Quick start: Node and TypeScript

You need Node.js 22 or newer. `@quadraui/node` ships the Rust renderer as a
prebuilt native addon for macOS (arm64), Linux (x64, glibc) and Windows
(x64).

> **Status.** `@quadraui/node` is first published to npm with quadraui 0.12
> ([#3079](https://github.com/JDonaghy/quadraui/issues/3079)), and the
> `@quadraui/node/app` and JSX entry points need
> [#2801](https://github.com/JDonaghy/quadraui/issues/2801) to load from
> `node_modules`. Until both land, use the [from a clone](#from-a-clone)
> steps at the end of this page, which CI runs today.

## 1. Install

```sh
mkdir counter && cd counter && npm init -y
npm install @quadraui/node
npm install --save-dev tsx typescript
```

JSX needs a compiler. Add a `tsconfig.json` that points JSX at quadraui:

```json
{
  "compilerOptions": {
    "strict": true,
    "module": "commonjs",
    "target": "es2020",
    "jsx": "react-jsx",
    "jsxImportSource": "@quadraui/node"
  }
}
```

## 2. Write the app

Save this as `counter.tsx`. It is the counter from the README, and CI runs
and type-checks this exact file (`examples/hero/node/counter.tsx`):

<!-- sample: examples/hero/node/counter.tsx -->
```tsx
const { App, keyName } = require("@quadraui/node/app");

class Counter {
  count = 0;

  view() {
    return (
      <column>
        <textDisplay id="count" lines={[{ spans: [{ text: `Count: ${this.count}`, fg: null, bg: null }] }]} />
        <row>
          <button id="dec" label="-" onClick={() => this.count--} />
          <button id="inc" label="+" onClick={() => this.count++} />
        </row>
      </column>
    );
  }

  onKey(event: unknown, quit: () => void) {
    if (keyName(event) === "q") quit();
  }
}

if (require.main === module) {
  const counter = new Counter();
  const app = new App(30, 5, { view: () => counter.view(), onEvent: (e: unknown) => counter.onKey(e, quit) });
  const live = app.runTui(); // or app.runNative("Counter"): same Counter
  // Ending a live session needs both calls, deferred out of the handler (examples/hero/README.md, "Gaps").
  const quit = () => setImmediate(() => (live.stop(), app.stopLive()));
}
module.exports = { Counter };
```

The three parts:

- **State** is the `Counter` object's fields.
- **View** is `view()`: it returns JSX (`<column>`, `<row>`, `<button>`)
  and runs again after every event, so a changed `count` shows up without
  `setState` or a redraw call.
- **Update** is the handlers: `onClick` on an element, and `onEvent` for
  input no single widget owns, such as the `q` key.

Lowercase tags are quadraui's built-in widgets, typed from the same
generated schema as the Rust side, so a misspelled prop fails `tsc`.

## 3. Run it in the terminal

```sh
npx tsx counter.tsx
```

Click `+` and `-`. `q` quits.

## 4. Run it in a native window

Change `app.runTui()` to `app.runNative("Counter")`. The prebuilt addons
from npm are terminal-only, so build the addon with a native backend
first. That needs a Rust toolchain (and `libgtk-4-dev` on Linux):

```sh
git clone https://github.com/JDonaghy/quadraui
cd quadraui/quadraui-node && npm install
npx napi build --platform --release --features macos   # or gtk, win
```

Then run the app against that checkout, as in [from a clone](#from-a-clone).

## 5. Test it headless

`@quadraui/node/testing` renders the app off screen and clicks what was
painted, by its text. From `examples/hero/node/run_headless.ts`:

<!-- sample: examples/hero/node/run_headless.ts -->
```ts
const counter = new Counter();
const h = harness(new App(30, 5, { view: () => counter.view() }));
if (!h.contains("Count: 0")) throw new Error(h.screen());
```

## From a clone

This path builds the addon from source and runs the same `counter.tsx`. It
is what CI runs:

```sh
git clone https://github.com/JDonaghy/quadraui
cd quadraui/quadraui-node && npm install && npm run build:debug && cd ..
node examples/hero/node/run_tui.ts        # live, in this terminal
node examples/hero/node/run_headless.ts   # the headless check
```

`run_tui.ts` compiles the JSX in memory with esbuild, so there is no build
step for your own code.

## Next

- [The shape of an app](../concepts/index.md), and how it maps onto React,
  Elm, iced and Textual.
- A larger app: `quadraui-node/examples/sample_app.ts` (list, form, confirm
  dialog, status bar) and `file_manager_app_jsx.tsx`.
- `App.enableHotReload(...)` reloads your view code and keeps state.
- [The same app in Rust and Python](../bindings.md).
