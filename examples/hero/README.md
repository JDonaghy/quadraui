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
| **Run** | `run_tui(Counter::default())` | `App(view=counter.view).run_tui()` | `new App({ view }).runTui()` |

None of them bind data, mark anything dirty, or ask for a redraw. A changed
`count` appears because the next frame calls `view()` again. Widget ids
(`count`, `dec`, `inc`) are the same in every language, and so is the click
script the headless runners drive (`+`, `+`, `-`, expecting `Count: 2` and
then `Count: 1`).

The one real difference is update. Python and Node attach callbacks to nodes.
A Rust node maps its events to a message value, and one `update` applies it,
the way Elm and iced do. Both are idiomatic for their language. The README
can put them side by side under one heading: "state, view, update".

Layout is the same too: `text`/`Text`/`<text>` is sized to its one line,
the row of buttons asks for its content height, and the two buttons share
its width equally, so all three render the identical frame.

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

Verified on 2026-11-28 against `hand-erg-integ`:
- All three headless runners pass on Linux (precision) and render the
  identical frame, and the parity check (`scripts/parity_ci.sh`) passes.
- The Rust version passes the macOS-backend test on the Mac and the TUI
  test on Windows.
- The captures under `site/src/images/hero-*.png` were redrawn from this
  layout on all four backends; a live native window was not checked by
  eye.

## Screenshots

The README and site images (`site/src/images/hero-*.png`) are this Rust
`Counter` painted off screen by each backend's headless test driver:
`scripts/capture_hero.sh tui,gtk` on Linux, `tui,macos` on a Mac and
`tui,win` on Windows (`src/bin/capture.rs`). No window opens, so it works
over ssh. `build.rs` embeds the Common-Controls v6 manifest that any
`win`-feature binary needs to start on Windows.

## Gaps this exposed, and what closed them

The first version of this app showed where the three languages differed.
Each gap is now closed:

1. **Rust had no declarative layout** and built its view twice. The app
   layer (`quadraui::app`: `View`, `Node`, `Run`) gives it `column`/`row`
   nodes laid out by the core stack solver, built once per frame (#3106).
2. **No one-line text label.** Rust has `text("...")`, Python
   `Text("...")` and Node `<text>`/`Text()`, all sized to their content; a
   span's `fg`/`bg` are optional everywhere (#3107).
3. **Fast clicks on one spot were lost** to double-click folding. A widget
   with no double-click meaning now sees both presses, so `+ +` counts
   twice (#3074).
4. **Quitting differed.** All three quit on Ctrl-C, and on `q`/Esc that no
   focused widget used; `App.quit()` is safe from any Python or Node
   callback (#3107).
5. **`App(width, height)` was required** in Python and Node; it now
   defaults to 80x24, which only a headless run uses (#3107).
6. **The TUI focus ring covered the button label.** A focused `[ + ]` keeps
   both brackets in the accent colour, underlined at its natural width, in
   all three languages; the parity check compares focus states (#3108).
7. **Python and Node children always took equal shares.** Every node takes
   `size`/`min`/`max` (#3107), matching Rust's `.content()`/`.fixed()`/...
8. **Node could not load from `node_modules`.** `@quadraui/node` compiles
   to `dist/` with typed exports, and `quadraui-node/scripts/pack_smoke.ts`
   runs this counter from packed tarballs (#2801, #3079).
   `node/load_jsx.ts` still compiles `counter.tsx` in memory with esbuild
   for the in-repo runners.
