# State

There are two kinds of state, and quadraui keeps them apart.

**Your state** is your data: the count, the list of tasks, which task is
selected. It lives in your own object. The view reads it; your event
handlers change it.

**Widget state** is what only the widget cares about: how far a list is
scrolled, where a text cursor is, a drag in progress, which tree rows are
expanded. The runtime keeps it for you, keyed by the widget's id, so you
do not store a scroll offset just to give it back next frame.

## Uncontrolled by default, controlled when you supply it

These are React's terms. A widget is **uncontrolled** when you leave a
value out and let the widget manage it, and **controlled** when you pass
the value every frame and update it from the widget's events. The task list
example controls its selection, because the detail form shows the selected
task:

<!-- sample: quadraui-py/examples/task_list_demo.py -->
```python
        task_list = List(
            [
                {"text": {"spans": [{"text": ("[x] " if t["done"] else "[ ] ") + t["title"], "fg": None, "bg": None}]}}
                for t in self.tasks
            ],
            id="tasks:list",
            selected=self.selected_idx,
            has_focus=True,
            on_select=self._select,
        )
```

A list, tree, table, text display or tab bar follows a value you pass
only when it changes. Passing the same `selected` every frame therefore
never fights the keyboard: the widget moves, reports the move, and goes
where you send it next.

To read widget state you did not control, ask for it by id:
`app.widget_state("tasks:list")` in Python, `app.widgetState(...)` in Node.

In Rust the split is in the widget contract itself: anything the app sets
is a field on the widget struct, and the widget's private
`Interactive::State` lives in the runtime's per-id store.

## Collections too large to send

A list, tree, table or text display can hold a million rows without the
app sending them all. Send a slice and say where it sits with
`items_window: {"start": ..., "total": ...}` (`Some(ItemsWindow::new(start,
total))` in Rust). The widget scrolls over the whole `total`, paints
placeholders for rows it does not have, and when input brings them into
view it sends `{"RowsRequested": {"id", "start", "count"}}`
(`UiEvent::RowsRequested` in Rust); answer by sending those rows next
frame. Every index stays absolute. `quadraui-py/examples/windowed_list_demo.py`
is a complete example, and the
[wire format](https://github.com/JDonaghy/quadraui/blob/develop/docs/bindings-wire-format.md)
has the rules.

## If you know...

- **React:** exactly controlled versus uncontrolled inputs. There are no
  hooks; component-local state comes from `Component` (Python, Node),
  shown below.
- **Elm:** your model is the state. Elm has no widget-owned state; quadraui
  adds it so you do not have to model scroll positions.
- **iced:** like the `State` a stateful iced widget keeps in its tree, but
  you never thread it through `view`.
- **Textual:** like reactive attributes on a widget, except your data stays
  on your own object and the widget's own data is managed for you.

## Component-local state

State that belongs to one piece of the view, rather than to your app
object, goes in a `Component` (`from quadraui.app import Component`; Node:
`Component` from `@quadraui/node/app`). `initial_state()` gives its shape
the first time it renders, and `view(state)` reads and changes it; the
runtime keeps the dict per component, by its place in the tree or its
`key=`. From `quadraui-py/examples/hot_reload/notes_view.py`:

<!-- sample: quadraui-py/examples/hot_reload/notes_view.py -->
```python
class Counter(Component):
    """Component-local state: survives every reload of this file."""

    def initial_state(self) -> Dict[str, Any]:
        return {"count": 0}

    def view(self, state: Dict[str, Any]) -> Any:
        def bump() -> None:
            state["count"] += 1

        return Column(
            TextDisplay([_display_line(f"Counter: {state['count']}")], id="hr:count"),
            Button("Add one", id="hr:add", on_click=bump),
        )
```

Use it like any node: `Column(Counter(), ...)`.
