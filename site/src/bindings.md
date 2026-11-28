# One app, three languages

`quadraui-py` (PyPI: `quadraui`) and `@quadraui/node` are thin layers over
the Rust core. Layout, painting, hit-testing, focus and widget state all
run in Rust, so a widget looks and behaves the same from every language.
The [hero counter](introduction.md#the-same-counter-three-ways) is written
once per language with the same widget ids, and CI clicks the same script
through all three.

## Side by side

| | Rust | Python | Node / TSX |
|---|---|---|---|
| Import | `use quadraui::prelude::*` | `from quadraui.app import App, ...` | `require("@quadraui/node/app")` |
| State | struct fields | object attributes | class fields |
| View | `fn view(&self)` returning `column([...])` | `def view(self)` returning `Column(...)` | `view()` returning `<column>...</column>` |
| Update | `.on_click(Msg::Inc)` + `fn update(&mut self, Msg)` | `on_click=self.inc` | `onClick={() => this.count++}` |
| Global keys | `fn on_event(&self, &UiEvent)` | `on_event=` + `key_name(event)` | `onEvent:` + `keyName(event)` |
| Run, terminal | `run_tui(app)` | `App(...).run_tui()` | `new App(...).runTui()` |
| Run, window | `run_native(app)` | `.run_native(title=...)` | `.runNative(title)` |
| Quit | `q`/Escape/Ctrl-C, or return `Command::Exit` | raise `SystemExit` | stop the live handle |
| Test headless | `TuiDriver` | `quadraui.testing.harness` | `@quadraui/node/testing` |
| Reusable parts | your own `Primitive` | `Component`, `Widget` | `Component`, `defineWidget` |
| Hot reload | rebuild | `App.enable_hot_reload` | `App.enableHotReload` |

Names follow each language: `on_select` in Python is `onSelect` in Node,
and both answer the Rust `UiEvent` for that widget. [Events](concepts/events.md)
has the table.

## Where they differ today

- **Update style.** Python and Node put callbacks on nodes. A Rust node
  maps its events to a message value, and one `update` applies it, as Elm
  and iced do, which keeps `&mut self` borrows out of the view.
- **Child sizes.** Rust nodes take `.content()`, `.fixed(px)`,
  `.weight(w)`, `.min(px)` and `.max(px)`, and containers `.gap(px)`;
  Python and Node children share their container equally.
- **Quitting.** Rust quits on `q`, Escape or Ctrl-C when no widget took
  the key (`quits_on` opts out). There is no default quit key in Python or
  Node. Python
  raises `SystemExit` from `on_event`. Node cannot stop a live session from
  inside a handler (`app.stopLive()` throws there), so the counter's
  `quit()` defers `live.stop()` and `app.stopLive()` with `setImmediate`.
  An `App.quit()` safe to call from a handler is proposed.
- **Node from npm.** `@quadraui/node` is first published with 0.12, and its
  app and JSX entry points need #2801 to load from `node_modules`; until
  then Node apps run from a clone ([Node quick start](quick-start/node.md)).
- **Text.** Python lets a span leave out `fg`/`bg`; the TypeScript type
  requires both (`null` for the theme's colour). Rust's `text("...")` is
  one plain line.
- **Native windows.** The Linux Python wheel and the npm addons are
  terminal-only; see [Getting started](getting-started.md#which-backends-you-get).

## Reference

The wire types (every layout node, widget descriptor and event, with
their fields) are generated from one Rust schema,
[`wire_schema.rs`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui-binding-core/src/wire_schema.rs),
by `scripts/gen_wire_types.py`, and CI fails if they drift:

- Python: [`quadraui/wire.pyi`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui-py/python/quadraui/wire.pyi)
  (`TypedDict`s for your type checker).
- TypeScript: [`wire.d.ts`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui-node/js/wire.d.ts).
- The app-layer nodes and their callbacks:
  [`quadraui/app/nodes.py`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui-py/python/quadraui/app/nodes.py),
  [`js/app/nodes.ts`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui-node/js/app/nodes.ts).

**Versions.** The bindings' minor version always equals the Rust crate's:
`quadraui` 0.N.x on PyPI pairs with crate 0.N.x, and so will
`@quadraui/node` from 0.12, its first npm release. The last release is
0.11; these pages track `develop`, which becomes 0.12. The policy is
[`BINDING_VERSION_POLICY.md`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/docs/decisions/BINDING_VERSION_POLICY.md).
