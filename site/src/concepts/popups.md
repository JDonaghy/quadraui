# Popups

Anything drawn above the normal layout is a popup: a dropdown list, a date
picker's calendar, a context menu, a tooltip, a dialog. The runtime paints
popups over everything else, hit-tests them first, and closes them on an
outside click or Escape. In a terminal they are drawn into the cell grid;
on the desktop backends menus and file dialogs use the platform's native
ones where it has them.

## Widgets that open their own

A dropdown (`Select` in Python and Node, `ComboBox` in Rust) or a
`DatePicker` opens its popup itself; you only see the result
(`on_change`). In Rust, such a widget sets
`Interactive::OPENS_POPUPS` and answers `popups()` with what is open; the
details are in
[`PRIMITIVE_RULES.md`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/docs/PRIMITIVE_RULES.md#popups).

## Dialogs from your code

Python and Node apps open a modal confirm or alert with one call and get
the chosen button back. From `quadraui-py/examples/task_list_demo.py`:

<!-- sample: quadraui-py/examples/task_list_demo.py -->
```python
        def resolve(button_id: str) -> None:
            if button_id != "delete":
                return
            del self.tasks[self.selected_idx]
            self.selected_idx = max(0, min(len(self.tasks) - 1, self.selected_idx))
            self.message = "Deleted"

        self.app.confirm("Delete task", f'Delete "{title}"?', _DELETE_BUTTONS, on_resolve=resolve)
```

`App.confirm` and `App.alert` in Node return a promise instead. With
asyncio, Python's `confirm` can return a future too.

Rust apps use the `MessageDialog` controller for the same thing, and
`ContextMenuController` for a context menu that is native where the
platform has one and painted where it does not. Larger overlays (a command
palette, a file picker) are compose controllers too; see
[`COMPOSE.md`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui/docs/COMPOSE.md).

## If you know...

- **React:** popups are like portals: declared by the widget that owns
  them, rendered above the app. You do not manage a portal root.
- **iced / egui:** like `overlay` / `Area`, with dismissal handled for you.
- **Textual:** like a `ModalScreen` for dialogs; `confirm` is the
  `push_screen(..., callback)` pattern in one call.
