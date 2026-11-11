"""A thin app layer over `quadraui.QuiApp`'s render/handle seam: typed
*nodes* instead of raw wire dicts, *callbacks* on those nodes instead of
one app-wide event switch, and *components* that bundle a node tree with
its own local state. Builds on `QuiApp`'s MVU discipline (state -> view ->
update, via `render`/`handle`) rather than replacing it -- this is not
MVVM: nothing here binds a value two ways, and nothing here hides which
direction data flows. #2331's TypeScript app layer mirrors every concept
below one for one.

**Nodes.** `Split`, `Column`, `Tree`, `Table`, `Button`, and friends
(`quadraui.app.nodes`) are small functions that build a `Node` -- the same
information a raw `{"tree": {...}}` wire dict carries, plus whichever
`on_*` callbacks that element's constructor accepts. A `view()` returns a
tree of these (mixed freely with raw wire dicts, `None`, and lists, for
escaping to the lower-level API wherever convenient); `App` walks it once
per render into the wire shape `QuiApp` already expects. Every id-bearing
constructor's `id=` is optional (`Column`/`Row` have no `id` at all --
there's nothing on the wire to address): a node whose caller didn't give
it an explicit `id=` gets a stable one derived from its position in the
tree -- stable, that is, as long as the tree's own shape (sibling count,
slot names) doesn't change between renders, the same assumption a bare
list index relies on anywhere else. `Toolbar` is the one exception --
see its own docstring for why it falls back to a weaker, call-order-based
default instead.

**Callbacks.** `Button("Save", on_click=self.save)`, `Tree(rows,
on_row_click=self.select, on_activate=self.open)` -- each node's own
constructor documents which raw `UiEvent` shape each `on_*` keyword
answers (a tree row click, a double-click, …). `App` rebuilds an
id -> callback registry on every render and dispatches a later event by
looking its own widget id up in whichever registry the render that
preceded it produced; a single `App._handle` does the looking up, so an
app's own code never pattern-matches `UiEvent` by hand for anything a node
callback already covers (one still works, via `on_event=`, for truly
global input like an app-wide keyboard shortcut no single widget owns).

**Components.** Subclass `Component`, override `initial_state()` and
`view(state)`, and use an instance of it as a node anywhere one is
accepted. A fresh instance is typically built every frame, the same way a
`Node` constructor is called every frame -- but its *state* lives in a
dict `App` keys by this component's position in the tree (or its explicit
`key=`) and threads back into every later `view(state)` call unchanged, so
state a callback mutates survives to the component's next render exactly
like a plain app's own instance attributes would.

**Redraw.** Nothing here schedules one explicitly, because nothing needs
to: `QuiApp` already repaints once per `step()` whenever a render callback
is registered (see `QuiApp`'s own module doc), so a callback mutating
app/component state has already made that state visible by the time the
`step()` that ran it returns. There is no `Reaction` value to return and
no manual "mark dirty" call in the common case.

**Hot reload.** `App.enable_hot_reload(module, ...)` re-imports edited
source modules while the app runs and keeps component state, the state on
a bound-method view's own object, and framework widget state; a broken
edit leaves the last good code running and is reported on
`App.reload_error`. `quadraui.app.hot_reload` documents exactly what
survives a reload.
"""

from .component import Component
from .nodes import (
    Button,
    Column,
    Node,
    Row,
    SidebarPanel,
    Split,
    StatusBar,
    Tab,
    Table,
    Tabs,
    TextDisplay,
    Toolbar,
    Tree,
)
from .runtime import App, key_name

__all__ = [
    "App",
    "Node",
    "Component",
    "key_name",
    "Split",
    "Column",
    "Row",
    "SidebarPanel",
    "Tab",
    "Tabs",
    "Tree",
    "Table",
    "StatusBar",
    "TextDisplay",
    "Button",
    "Toolbar",
]
