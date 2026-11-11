# Building an app on `quadraui` (Python) — guide for coding agents

This file ships inside the `quadraui` wheel, so it always matches the
version you have installed. Find it, and everything else this guide points
at, with:

```bash
python -c "import quadraui, os; print(os.path.dirname(quadraui.__file__))"
```

Read it before writing code against the package. It covers how to structure
an app, how to test it, and what will bite you if you don't know about it.
The authoritative detail is in the module docstrings next to this file. Each
section below names the one to open.

## What is in the package

| Module | What it is | Read it when |
|---|---|---|
| `quadraui.app` | The app layer: typed nodes, `on_*` callbacks, `Component`s, hot reload. **Start here.** | Always |
| `quadraui.testing` | The black-box test harness, plus a `quadraui_harness` pytest fixture registered automatically | Writing tests |
| `quadraui.wire` (`wire.pyi`) | `TypedDict`s for every descriptor and event on the wire. Stub-only: import it under `TYPE_CHECKING` | Building a raw descriptor, or typing one |
| `quadraui` (`QuiApp`) | The lower-level engine `App` wraps: raw dict descriptors, one `handle(event)` switch, platform services, theme, fonts | You need something the app layer doesn't expose |
| `quadraui.app.hot_reload` | What survives a reload and what doesn't | Turning on hot reload |

The package is typed (`py.typed`). `QuiApp`'s own methods are typed
permissively as `Any`, so a type checker won't catch a misspelled
`QuiApp` method. The node constructors and `quadraui.wire` *are* typed
precisely.

## The shape of an app

Write against `quadraui.app.App`, not raw `QuiApp`, unless you have a
reason. The model is MVU: your state lives in plain Python, `view()`
returns a tree of nodes built from it every frame, and callbacks on those
nodes mutate the state. There is no data binding, no "mark dirty", and no
redraw call. `App` repaints after every event a callback handled.

```python
from quadraui.app import App, Button, Column, Component, TextDisplay, key_name


class Quit(Exception):
    pass


def line(text):
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


class Counter(Component):
    def initial_state(self):
        return {"count": 0}

    def view(self, state):
        def bump():
            state["count"] += 1

        return Column(
            TextDisplay([line(f"Count: {state['count']}")], id="count"),
            Button("Increment", id="inc", on_click=bump),
        )


class CounterApp:
    def __init__(self, width=60, height=12):
        self.app = App(width, height, view=self.view, on_event=self.on_key)

    def view(self):
        return Counter(key="main")

    def on_key(self, event):
        if key_name(event) in ("q", "Escape"):
            raise Quit


if __name__ == "__main__":
    try:
        CounterApp().app.run_tui()   # or .run_native(title="Counter")
    except Quit:
        pass
```

Rules that follow from the model:

- **Build the whole tree in `view()` every frame.** Node constructors are
  cheap. Don't cache nodes between frames, and don't try to patch a
  previous tree.
- **Keep app state on an object, and use a bound method as `view`.** That
  object survives hot reload, as in the `CounterApp` above. Use a `Component`
  for a reusable piece with its own local state. Its state dict is keyed by
  the component's position in the tree, or by `key=`. Give a `key=` to any
  component in a list that can reorder.
- **Don't track what the framework already owns.** Scroll offsets, which
  tab is active, a `Split`'s dragged ratio and focus live in the engine's
  widget-state store, keyed by widget id. Data the rows carry, such as a
  tree row's `is_expanded` or `Tree(selected_path=...)`, is yours: keep
  it in your model. Re-send the
  *same literal* `ratio=` every frame and a user's drag persists. Read it
  back with `app.widget_state(id)` if you need it. To change it from code,
  use the action handle: `app.actions.apply_tab_activated("tabs-id", i)`.
- **Give an explicit `id=` to every widget you will query, test, or keep
  state for.** Omitting it gets you an auto-id derived from the node's tree
  position. That id changes whenever the tree's shape above it changes, and
  so does the widget state keyed by it.
- **Callbacks receive the payload, not the event.** For example,
  `Tree(on_row_click=f)` calls `f(path)` and `Table(on_activate=f)` calls
  `f(row_index)`. Each constructor's docstring lists its callbacks.
  Bind per-item context with a default argument, as in
  `lambda idx, folder=folder: ...`, not by parsing ids.
- **Global input goes to `App(on_event=...)`**, the fallback for events no
  node callback claimed. Use `key_name(event)` to get a key name out of
  whatever shape the event arrived in. Quit `run_tui()`/`run_native()` by
  raising an exception of your own from a handler and catching it around
  the run call.

### Available nodes

`Split`, `Column`, `Row`, `SidebarPanel`, `Tabs`/`Tab`, `Tree`, `Table`,
`StatusBar`, `TextDisplay`, `Button`, plus `Toolbar`. `Toolbar` is not a
node: it returns a raw descriptor for `SidebarPanel(toolbar=...)`, and its
buttons do **not** raise events yet. Use `Button` for anything clickable.

Anything else in the toolkit (progress, spinner, image, list, overlays,
...) has no typed node yet. A `view()` may return a **raw wire dict**
anywhere a node goes, mixed freely with nodes. Get the shape from
`quadraui.wire`:

```python
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    from quadraui.wire import TreeRowDescriptor  # stub-only; never import at runtime
```

A malformed descriptor fails when the engine decodes it, with
`quadraui.InvalidDescriptorError`. The node layer does not validate.

## Running it

- `app.run_tui()`: takes over the terminal. Works on every wheel.
- `app.run_native(...)`: a real native window. It's built into the macOS
  (AppKit) and Windows (Direct2D) wheels. **The Linux wheel is TUI-only**:
  check `hasattr(quadraui.QuiApp, "run_native")` and fall back to
  `run_tui()`.
- Headless: `app.step()` processes pending events and renders, and
  `app.screen()` returns the frame as text. This is what tests use.

Threads: `App`/`QuiApp` is pinned to the thread that created it. From a
background thread, use `app.qui.waker()` and its
`post(value)`/`post_after(ms, value)`. The value arrives as a
`{"User": value}` event in `on_event`.

Inside a handler, change engine state through `app.actions`, not through
`QuiApp` setters (`set_theme`, fonts, ...). Those must be called outside
the callback, after `step()`/`run` returns.

Theme, fonts, platform services (auto-update, login items, recent
documents, tray, ...) and `app_path(kind)` are on `QuiApp`/`quadraui`. See
`quadraui/__init__.py`'s module docstring. A service a backend lacks raises
`UnsupportedError` rather than silently doing nothing, and the headless
backend has none of them.

## Testing — drive the app the way a user does

Every behavior you add should have a black-box test that drives the real
app and asserts on what it renders. The harness makes that the easy path.
Installing `quadraui` registers the `quadraui_harness` pytest fixture, so
no `conftest.py` is needed:

```python
from counter import CounterApp

def test_increment(quadraui_harness):
    h = quadraui_harness(CounterApp().app)
    h.assert_text("Count: 0")

    h.find(role="button", text="Increment").click()
    h.assert_text("Count: 1")
```

- **Query** with `h.find(id=..., role=..., text=...)`, which raises
  `WidgetNotFound`, an `AssertionError`. Also `find_all`, `query` (returns
  `None` when nothing matches) and `exists`. `role` is the wire kind:
  `"tree"`, `"data_table"`, `"tabs"`, `"button"`, `"text_display"`, ...
  Content of an inactive tab is hidden from queries unless you pass
  `include_hidden=True`.
- **Act** like a user would: `tree.row("alpha").click()`, `.toggle()`,
  `table.row(index=1).double_click()`, `tabs.tabs[0].close()`,
  `split.drag_divider(0.6)`, `h.press("Down")`, `h.type("hello")`,
  `widget.focus()`, `h.click_text("Save")`.
- **Assert** with `h.assert_text(...)` and `h.assert_not_text(...)`. A
  failure prints the whole frame. `h.screen()` and `h.contains()` are the
  plain versions. Engine-owned state is on `widget.state`,
  `widget.scroll_offset`, `widget.ratio` and `widget.active_index`.
- **Re-query after every action.** A `Widget`/`Row`/`Tab` is a snapshot of
  one frame.
- Never hand-build wire events or hardcode coordinates in a test. If the
  harness can't express an action, that is a gap to report, not a reason to
  drop to raw events.
- For a plain `QuiApp`, pass the render callable too:
  `quadraui_harness(qui_app, render=my_render)`.

Known issue (0.10.1, quadraui#2394): in the TUI backend, a mouse-clicked
`Button` repaints as an empty box with no label, so a
*second* `click_text(...)` on it fails. Until that's fixed, test repeated
presses through the keyboard path, which the bug doesn't affect:
`h.find(role="button").focus(); h.press("Enter")`.

## Hot reload

```python
app.enable_hot_reload(my_view_module)   # watched every 0.5 s by default
```

Edit the module and save: the app re-imports it and keeps component state,
the state on a bound-method view's object, and widget state. A broken edit
keeps the last good code running and sets `app.reload_error`. Show it in
your status bar.

- The watched code must live in an **importable module**, not the
  `__main__` script.
- Module-level globals are re-executed on reload. New attributes an edited
  `__init__` would set don't exist on an existing instance. Use
  `getattr(self, name, default)`, or set them in an `on_reload=` hook.
  Override `Component.migrate_state` if an edit renames state keys.
- In tests, the `quadraui_harness` fixture shuts the watcher thread down
  for you. With `harness(...)` directly, call `h.close()`.

## Worked examples

These are in the source repository, not the wheel. Each one has a matching
black-box test under
[`quadraui-py/tests/`](https://github.com/JDonaghy/quadraui/tree/main/quadraui-py/tests).

- [`file_manager_app.py`](https://github.com/JDonaghy/quadraui/blob/main/quadraui-py/examples/file_manager_app.py):
  a full app layer app. A lazily loaded tree, closable table tabs, a status
  bar and per-row callbacks. Its test is `test_file_manager_app.py`.
- [`hot_reload_demo.py`](https://github.com/JDonaghy/quadraui/blob/main/quadraui-py/examples/hot_reload_demo.py)
  with `hot_reload/notes_view.py`: hot reload with state carried over.
- [`file_manager.py`](https://github.com/JDonaghy/quadraui/blob/main/quadraui-py/examples/file_manager.py):
  the same app on raw `QuiApp`, for comparison with the layer below.
- `appshell_demo.py` (VS Code-style shell via `run_shell`),
  `native_window_demo.py`, `waker_demo.py` (background threads).

Report bugs and API gaps at <https://github.com/JDonaghy/quadraui/issues>.
