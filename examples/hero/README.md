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
counter-native` (or `tui,gtk` / `tui,win`) runs the same Rust `Counter`. In
Python and Node, swap `run_tui()` for `run_native(...)`. `q` quits all three.
`node examples/hero/node/run_tui.ts path/to/app.tsx` runs a JSX file of your
own the same way, which is how the Node quick start runs a reader's app until
`@quadraui/node` loads from `node_modules` (#2801).

## The shared shape

All three are MVU (the Elm architecture), the model behind Elm, iced,
Textual and React. Each one is a plain object with three parts:

| Part | Rust | Python | Node / JSX |
|---|---|---|---|
| **State** | `struct Counter { count: i32 }` | `self.count = 0` | `count = 0` |
| **View**: state to a widget tree, rebuilt each frame | `fn view(&self) -> ScreenLayout` | `def view(self)` returning `Column(...)` | `view()` returning `<column>...</column>` |
| **Update**: events change state | `fn handle(&mut self, event) -> Reaction`, matching `ButtonActivated { widget }` by id | `on_click=self.inc` callbacks on the nodes | `onClick={() => this.count++}` callbacks on the elements |
| **Run** | `quadraui::tui::run(Counter::default())` | `App(30, 5, view=counter.view).run_tui()` | `new App(30, 5, { view }).runTui()` |

None of them bind data, mark anything dirty, or ask for a redraw. A changed
`count` appears because the next frame calls `view()` again. Widget ids
(`count`, `dec`, `inc`) are the same in every language, and so is the click
script the headless runners drive (`+`, `-`, `+`, expecting `Count: 1`).

The one real difference is update. Python and Node attach callbacks to nodes.
Rust matches events by widget id in one `handle`, the way Elm's `update`
matches messages. Both are idiomatic for their language. The README can put
them side by side under one heading: "state, view, update".

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

1. **Rust has no declarative layout, so its view is twice as long.** Python
   and Node return `Column(label, Row(dec, inc))` and the engine lays it
   out. Rust has no `Column`/`Row`: the app hand-places a `Rect` for each
   widget, in character-cell units, inside a `ScreenLayout`. The row and
   column solver exists, but only in `quadraui-binding-core::layout`
   (`LayoutNode::{Row, Column, Leaf, ...}`), where Rust apps can't reach it.
2. **Rust builds the view twice, and the second build can't see the
   backend.** `AppLogic::render` paints. `AppLogic::frame_tree` rebuilds the
   same widgets so the runtime can route clicks to them. `frame_tree` gets
   no `Backend`, so the hero stores the last-seen cell size in a
   `Cell<(f32, f32)>` during `render`, purely so the second build can lay
   out the same way. A reader will see this as noise.

   **The smallest fix covers gaps 1 and 2 (proposal only; not built here).**
   The widget-contract plan already says binding-core keeps no layout solver
   of its own (#2828 rule 7), so the solver is moving into `quadraui` anyway.
   Move `LayoutNode` (`Row`/`Column`/`Leaf`) and its solver into `quadraui`,
   then add one adapter:

   ```rust
   pub trait View {
       fn view(&self) -> LayoutNode;
       fn update(&mut self, event: UiEvent) -> Reaction;
   }
   // quadraui::app::Run<V: View> implements AppLogic: render() resolves and
   // paints the tree and caches the resolved FrameTree; frame_tree() returns
   // that cache. A View then runs on every backend's existing runner.
   ```

   The Rust hero would then be:

   ```rust
   impl View for Counter {
       fn view(&self) -> LayoutNode {
           column([text(format!("Count: {}", self.count)).id("count"),
                   row([button("-").id("dec"), button("+").id("inc")])])
       }
       fn update(&mut self, event: UiEvent) -> Reaction { /* the same match */ }
   }
   fn main() -> Result<(), RunError> { quadraui::tui::run(Run(Counter::default())) }
   ```

   That is the same length and shape as the Python and Node versions. It
   adds no callbacks or closures to Rust. `update` stays an Elm-style match
   on widget ids, which avoids the borrow and lifetime problems that
   callbacks capturing `&mut self` would cause.
3. **One-line text labels (#3107).** Rust has
   `TextDisplayLine::plain(s)`, Python `Text("...")`, and Node `<text>` and
   `Text()`. A span's `fg`/`bg` are optional in every language.
4. **Fast clicks on the same spot are lost.** Two clicks at one position
   inside the double-click interval fold into a `DoubleClick`, and a
   `Button` does not activate on it. Clicking `+` twice quickly counts
   once. This happens in Rust (`TuiDriver`), Python and Node. The runners
   click `+ - +` to work around it. A button should activate on every
   press.
5. **Quitting (#3107).** `App.quit()` is safe from any callback in
   both bindings (in Node, `app.stopLive()` inside a handler quits rather
   than throwing). Ctrl-C always quits, and `q`/Esc quit when no
   focused widget used the key; `quit_keys=False` / `quitKeys: false` turns
   that off. Rust returns `Reaction::Exit`.
6. **`App(width, height)` is optional (#3107)** in Python and Node
   (80x24); the size only sizes a headless run.
7. **The TUI focus ring covers the focused button's label** in Python and
   Node: `[ + ]` renders as `│ + ]`. The Rust `AppLogic` version shows no
   focus ring at all. So a cross-language parity test that compares frames
   cell for cell (#3077) would fail on focus styling before it gets to
   anything else.
8. **Per-child sizes (#3107), Python and Node only.** Every app-layer
   node takes `size`/`min`/`max`, and `Text`/`<text>` default to their
   content size, so the label takes one row. The Rust hero still splits
   equally.
9. **Node can't run JSX directly.** `node/load_jsx.ts` compiles
   `counter.tsx` in memory with esbuild and maps `@quadraui/node/*` to this
   checkout. An app built on the published package needs `tsc`, esbuild or
   Bun, and the `app`/`jsx-runtime` subpaths still export `.ts` source
   (#2801).
