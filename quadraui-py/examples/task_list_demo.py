"""The Audit 5 sample app: a task list, a detail form, a confirm dialog
and a status bar, all on `quadraui.app`'s typed nodes -- no raw wire
dict, no `on_event` branch for `List`/`Form`/`MessageDialogResolved`.
Up/Down (or a click) move the cursor, the form edits the selected task,
`d` (or the Delete button) confirms before removing it, `q` quits.

    python quadraui-py/examples/task_list_demo.py [--tui]
"""

from __future__ import annotations

import sys
from typing import Any, Dict, List as ListOf

from quadraui.app import App, Button, Column, Form, List, Row, StatusBar, TextInputField, ToggleField, key_name

WIDTH, HEIGHT = 60, 14
_DELETE_BUTTONS = [
    {"id": "delete", "label": "Delete", "is_default": True},
    {"id": "cancel", "label": "Cancel", "is_cancel": True},
]


class _Quit(Exception):
    """Raised from `on_event` to unwind out of `run_tui()`."""


class TaskListDemo:
    def __init__(self) -> None:
        self.tasks: ListOf[Dict[str, Any]] = [
            {"title": "Write report", "done": False},
            {"title": "Review PR", "done": True},
            {"title": "Ship release", "done": False},
        ]
        self.selected_idx = 0
        self.message = ""
        self.app = App(WIDTH, HEIGHT, view=self.view, on_event=self.on_event)

    def _confirm_delete(self) -> None:
        if not self.tasks:
            return
        title = self.tasks[self.selected_idx]["title"]

        def resolve(button_id: str) -> None:
            if button_id != "delete":
                return
            del self.tasks[self.selected_idx]
            self.selected_idx = max(0, min(len(self.tasks) - 1, self.selected_idx))
            self.message = "Deleted"

        self.app.confirm("Delete task", f'Delete "{title}"?', _DELETE_BUTTONS, on_resolve=resolve)

    def _set(self, field: str, value: Any) -> None:
        self.tasks[self.selected_idx][field] = value

    def view(self) -> Any:
        task = self.tasks[self.selected_idx] if self.tasks else None
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
        fields = [
            TextInputField(task["title"], id="tasks:form:title", label="Title", on_change=lambda v: self._set("title", v)),
            ToggleField(task["done"], id="tasks:form:done", label="Done", on_change=lambda v: self._set("done", v)),
        ] if task is not None else []
        detail = Column(Form(fields, id="tasks:form"), Button("Delete", id="tasks:delete", on_click=self._confirm_delete))
        return Column(
            Row(task_list, detail),
            StatusBar(
                [
                    {
                        "text": f" {len(self.tasks)} task(s)  {self.message} ",
                        "fg": {"r": 220, "g": 220, "b": 230, "a": 255},
                        "bg": {"r": 30, "g": 32, "b": 48, "a": 255},
                    }
                ],
                [],
                id="tasks:status",
            ),
        )

    def _select(self, idx: int) -> None:
        self.selected_idx = idx

    def on_event(self, event: Any) -> None:
        name = key_name(event)
        if name is None:
            return
        self.message = ""
        last = len(self.tasks) - 1
        if name in ("Down", "Up"):
            self.selected_idx = max(0, min(last, self.selected_idx + (1 if name == "Down" else -1)))
        elif name == "d":
            self._confirm_delete()
        elif name in ("q", "Escape"):
            raise _Quit


def build() -> "tuple[App, TaskListDemo]":
    demo = TaskListDemo()
    return demo.app, demo


if __name__ == "__main__":
    app, demo = build()
    if "--tui" in sys.argv[1:]:
        try:
            app.run_tui()
        except _Quit:
            pass
        print(f"task_list_demo: {len(demo.tasks)} task(s) left")
    else:
        app.step()
        print(app.screen())
