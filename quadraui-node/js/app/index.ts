// A thin app layer over `QuiApp`'s render/handle seam: typed *nodes*
// instead of raw wire objects, *callbacks* on those nodes instead of one
// app-wide event switch, and *components* that bundle a node tree with its
// own local state. Builds on `QuiApp`'s MVU discipline (state -> view ->
// update, via `setRender`/`setHandler`) rather than replacing it -- this is
// not MVVM: nothing here binds a value two ways, and nothing here hides
// which direction data flows. Mirrors `quadraui-py`'s own `quadraui.app`
// (`quadraui-py/python/quadraui/app/__init__.py`) one concept at a time, in
// TypeScript idioms where the two languages differ -- see each divergence
// called out below.
//
// **Nodes.** `Split`, `Column`, `Tree`, `Table`, `Button`, and friends
// (`./nodes.ts`) are small functions that build a `Node` -- the same
// information a raw `{tree: {...}}` wire object carries, plus whichever
// `on*` callbacks that element's constructor accepts. A `view()` returns a
// tree of these (mixed freely with raw wire objects, `null`/`undefined`,
// and arrays, for escaping to the lower-level API wherever convenient);
// `App` walks it once per render into the wire shape `QuiApp` already
// expects. Every id-bearing constructor's `id` option is optional
// (`Column`/`Row` have no `id` at all -- there's nothing on the wire to
// address): a node whose caller didn't give it an explicit `id` gets a
// stable one derived from its position in the tree -- stable, that is, as
// long as the tree's own shape (sibling count, slot names) doesn't change
// between renders, the same assumption a bare array index relies on
// anywhere else. `Toolbar` is the one exception -- see its own doc for why
// it falls back to a weaker, call-order-based default instead.
//
// *Idiom divergence 1: options objects, not keyword arguments.* Python's
// constructors take `id=`/`on_click=`/... as keyword-only parameters;
// TypeScript has no equivalent syntax, so every constructor here takes a
// single trailing options object instead (`Button("Save", {onClick: ...})`
// -- the issue's own worked example). `Column`/`Row` take their items as a
// plain array (`Column([a, b])`) rather than Python's variadic `*items`,
// since a rest parameter can't be followed by a later options parameter;
// this also makes every list-of-children constructor here consistent with
// `Tabs`/`Tree`/`Table`, which already took an array on both sides.
//
// **Callbacks.** `Button("Save", {onClick: self.save})`, `Tree(rows,
// {onRowClick: self.select, onActivate: self.open})` -- each node's own
// constructor documents which raw `UiEvent` shape each `on*` option
// answers (a tree row click, a double-click, …). `App` rebuilds an
// id -> callback registry on every render and dispatches a later event by
// looking its own widget id up in whichever registry the render that
// preceded it produced; a single `App._handle` does the looking up, so an
// app's own code never pattern-matches `UiEvent` by hand for anything a
// node callback already covers (one still works, via `onEvent`, for truly
// global input like an app-wide keyboard shortcut no single widget owns).
//
// **Components.** Subclass `Component`, override `initialState()` and
// `view(state)`, and use an instance of it as a node anywhere one is
// accepted. A fresh instance is typically built every frame, the same way
// a node constructor is called every frame -- but its *state* lives in a
// plain object `App` keys by this component's position in the tree (or
// its explicit `key`) and threads back into every later `view(state)` call
// unchanged, so state a callback mutates survives to the component's next
// render exactly like a plain app's own instance fields would.
//
// *Idiom divergence 2: no `Proxy`, no descriptor.* Python's own module doc
// flags this as the one place the two layers might need a different
// mechanism (`Proxy` vs. descriptors) to notice a mutation and schedule a
// redraw. Neither language needs either: `QuiApp` already repaints once
// per `step()` whenever a render callback is registered (see
// `../index.ts`'s own `run`/`pump` doc), so a plain mutable object -- a
// dict in Python, a plain object here -- is already enough in both. There
// is no "mark dirty" step for either layer to intercept a write for.
//
// **Redraw.** Nothing here schedules one explicitly, for the reason the
// paragraph above already gives: the mutation a callback makes is already
// visible by the time the `step()` that ran it returns. There is no
// reaction value to return and no manual "mark dirty" call in the common
// case.
//
// **Hot reload.** `App.enableHotReload([require.resolve("./view.ts")],
// {preserve: [myApp]})` re-requires edited source modules while the app
// runs and keeps component state, the state on a preserved app object, and
// framework widget state; a broken edit leaves the last good code running
// and is reported on `App.reloadError`. `./hotReload.ts` documents exactly
// what survives -- and the one real divergence from Python, which is that
// a JS view must call *through* the preserved object or the module
// exports (`{view: () => myApp.view()}`) rather than capture a function.
//
// **Testing.** `require("../testing.ts").harness(app)` wraps an `App` for
// a test that queries by widget id/role/text, synthesises the clicks and
// keys a user performs, and asserts on the rendered frame -- without a
// test ever spelling out a wire event or a coordinate. See that module's
// own doc; it mirrors `quadraui.testing` the same way this file mirrors
// `quadraui.app`.
//
// Runtime exports go through `exports.X = ...` rather than `export`, the
// same reason `../index.ts`'s own header comment gives.

const nodes = require("./nodes.ts");
const { Component } = require("./component.ts");
const { App, keyName } = require("./runtime.ts");

exports.App = App;
exports.keyName = keyName;
exports.Component = Component;
exports.Split = nodes.Split;
exports.Column = nodes.Column;
exports.Row = nodes.Row;
exports.SidebarPanel = nodes.SidebarPanel;
exports.Tab = nodes.Tab;
exports.Tabs = nodes.Tabs;
exports.Tree = nodes.Tree;
exports.Table = nodes.Table;
exports.StatusBar = nodes.StatusBar;
exports.TextDisplay = nodes.TextDisplay;
exports.Button = nodes.Button;
exports.Toolbar = nodes.Toolbar;

export type { Node, Renderable, ComponentLike } from "./nodes";
export type {
  SplitOptions,
  ColumnOptions,
  RowOptions,
  SidebarPanelOptions,
  TabOptions,
  TabsOptions,
  TreeOptions,
  TableOptions,
  StatusBarOptions,
  TextDisplayOptions,
  ButtonOptions,
  ToolbarOptions,
} from "./nodes";
export type { ComponentState } from "./component";
export type { AppOptions, HotReloadOptions } from "./runtime";
