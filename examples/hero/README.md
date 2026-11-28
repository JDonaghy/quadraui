# The hero app: one counter, three languages

The README's first screen has to show a seasoned developer, in about 30
seconds, that a quadraui app has the shape they already know. This directory
holds that app, a counter with a label and `-`/`+` buttons, written once in
each language:

| | Program | Run it live | Run it headless |
|---|---|---|---|
| Rust | [`rust/src/main.rs`](rust/src/main.rs) | `cargo run -p quadraui-hero --features tui --bin counter` | `cargo test -p quadraui-hero --features tui` |
| Python | [`python/counter.py`](python/counter.py) | `python examples/hero/python/counter.py` | `python examples/hero/python/run_headless.py` |
| Node (TSX) | [`node/counter.tsx`](node/counter.tsx) | `node examples/hero/node/run_tui.ts` | `node examples/hero/node/run_headless.ts` |

A native window: `cargo run -p quadraui-hero --features tui,macos --bin
counter-native` (or `tui,gtk` / `tui,win`) runs the same Rust `Counter`
through `run_native`. In
Python and Node, swap `run_tui()` for `run_native(...)`. `q` quits all three.
`node examples/hero/node/run_tui.ts path/to/app.tsx` runs a JSX file of your
own the same way, which is how the Node quick start runs a reader's app until
`@quadraui/node` is on npm (#3079).

## The shared shape

All three are MVU (the Elm architecture), the model behind Elm, iced,
Textual and React. Each one is a plain object with three parts:

| Part | Rust | Python | Node / JSX |
|---|---|---|---|
| **State** | `struct Counter { count: i32 }` | `self.count = 0` | `count = 0` |
| **View**: state to a widget tree, rebuilt each frame | `fn view(&self) -> Node<Msg>` returning `column([...])` | `def view(self)` returning `Column(...)` | `view()` returning `<column>...</column>` |
| **Update**: events change state | `.on_click(Msg::Inc)` on the nodes, applied by `fn update(&mut self, msg: Msg)` | `on_click=self.inc` callbacks on the nodes | `onClick={() => this.count++}` callbacks on the elements |
| **Run** | `run_tui(Counter::default())` | `App(30, 5, view=counter.view).run_tui()` | `new App(30, 5, { view }).runTui()` |

None of them bind data, mark anything dirty, or ask for a redraw. A changed
`count` appears because the next frame calls `view()` again. Widget ids
(`count`, `dec`, `inc`) are the same in every language, and so is the click
script the headless runners drive (`+`, `-`, `+`, expecting `Count: 1`).

The one real difference is update. Python and Node attach callbacks to nodes.
A Rust node maps its events to a message value, and one `update` applies it,
the way Elm and iced do. Both are idiomatic for their language. The README
can put them side by side under one heading: "state, view, update".

The Rust view also sizes its children (`.content()`, `.gap(..)`): the label
takes one line and the buttons their own width. Without those calls it lays
out exactly as the Python and Node versions do, each child an equal share.

## How each is checked

- **Rust**: `rust/tests/headless.rs` runs the click script on the TUI backend
  through `TuiDriver`. CI's `tui` job runs it as part of
  `cargo test --features tui --workspace`. `rust/tests/headless_macos.rs`
  runs the same script on the macOS backend: Core Graphics and Core Text
  paint into an off-screen bitmap and clicks are given in pixels. This
  test runs only on a macOS host with `--features tui,macos`.
- **Python**: `quadraui-py/tests/test_hero_counter.py` runs
  `run_headless.py`, so the `quadraui-py-tests` job covers it.
- **Node**: `quadraui-node/tests/hero_counter.test.ts` runs
  `run_headless.ts` (`npm test`). `npm run typecheck:jsx` type-checks
  `counter.tsx` under `tsc --strict` (it is listed in
  `quadraui-node/tsconfig.jsx.json`).

Verified on 2026-11-27 against develop `6f501cdd`:
- All three headless runners pass on Linux (precision), and Python and Node
  render the identical frame.
- All three run live in a real terminal (tmux) and quit cleanly on `q`.
- The Rust version passes the macOS-backend test on the Mac.
- A live native window was not checked by eye: the Mac's display was off,
  so the screenshot came back black.

## Screenshots

The README and site images (`site/src/images/hero-*.png`) are this Rust
`Counter` painted off screen by each backend's headless test driver:
`scripts/capture_hero.sh tui,gtk` on Linux, `tui,macos` on a Mac and
`tui,win` on Windows (`src/bin/capture.rs`). No window opens, so it works
over ssh. `build.rs` embeds the Common-Controls v6 manifest that any
`win`-feature binary needs to start on Windows.

## Gaps this exposed

Rust gaps first, then the ones shared by all three languages, then the
binding-only ones.

1. **Rust had no declarative layout** (closed). The app layer
   (`quadraui::app`) gives it `column`/`row` nodes laid out by the core
   stack solver, with per-child sizes.
2. **Rust built its view twice** (closed). `Run`, the app layer's
   `AppLogic` adapter, builds the view once per frame and routes against
   the frame it kept, so the app no longer stashes the cell size.
3. **One-line text labels (#3107).** Rust has `text("...")` in the app
   layer and `TextDisplayLine::plain(s)`, Python `Text("...")`, and Node
   `<text>` and `Text()`. A span's `fg`/`bg` are optional in every language.
5. **Quitting (#3107).** `App.quit()` is safe from any callback in
   both bindings (in Node, `app.stopLive()` inside a handler quits rather
   than throwing). Ctrl-C always quits, and `q`/Esc quit when no
   focused widget used the key; `quit_keys=False` / `quitKeys: false` turns
   that off. Rust quits the same way by default (`quits_on` opts out), or
   when `update` returns `Command::Exit`.
6. **`App(width, height)` is optional (#3107)** in Python and Node
   (80x24); the size only sizes a headless run.
7. **Focus renders the same in all three languages.** A focused `[ + ]`
   keeps both brackets: they turn the accent colour and the ring
   underlines the button at its natural width. The parity check (#3077)
   compares focus states cell for cell.
8. **Per-child sizes (#3106, #3107).** Every app-layer node takes a
   size and `min`/`max` in all three languages (Rust: `.content()`,
   `.fixed(px)`, `.weight(w)`, ...; Python and Node: `size=`/`min=`/`max=`),
   and text labels default to their content size, so the label takes one
   row.
9. **Node can't run JSX directly.** `node/load_jsx.ts` compiles
   `counter.tsx` in memory with esbuild and maps `@quadraui/node/*` to this
   checkout's compiled `dist/`. An app built on the published package
   compiles its JSX with `tsc`, esbuild or Bun;
   `quadraui-node/scripts/pack_smoke.ts` does that with `tsc` against
   `npm pack` tarballs.
