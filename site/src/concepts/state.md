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

To read widget state you did not control, ask for it by id:
`app.widget_state("tasks:list")` in Python, `app.widgetState(...)` in Node.

In Rust the split is in the widget contract itself: anything the app sets
is a field on the widget struct, and the widget's private
`Interactive::State` lives in the runtime's per-id store.

## If you know...

- **React:** exactly controlled versus uncontrolled inputs. There are no
  hooks; component-local state comes from `Component` (Python, Node).
- **Elm:** your model is the state. Elm has no widget-owned state; quadraui
  adds it so you do not have to model scroll positions.
- **iced:** like the `State` a stateful iced widget keeps in its tree, but
  you never thread it through `view`.
- **Textual:** like reactive attributes on a widget, except your data stays
  on your own object and the widget's own data is managed for you.
