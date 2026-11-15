"""Runnable demo of the `List` node helper: a selectable list of file
names wired up with `on_select`/`on_activate`, with no raw wire dict and
no `"List"` branch in `on_event` -- the typed-node shape issue #2461 adds,
in place of the raw `{"list": {...}}` leaf `examples/appshell_demo.py` and
`examples/tui_render_state_demo.py` still hand-write.

Run headless (drives a selection move and an activate through the
headless harness, then prints what each callback recorded):

    maturin develop
    python quadraui-py/examples/list_demo.py

Run in a real terminal (arrow keys/click move the selection, Enter or a
double-click "opens" the highlighted file, `q`/Escape quits):

    python quadraui-py/examples/list_demo.py --tui
"""

from __future__ import annotations

import sys
from typing import TYPE_CHECKING, Any, List as ListOf

from quadraui.app import App, List

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import UiEvent

WIDTH = 30
HEIGHT = 8

FILES = ["README.md", "Cargo.toml", "src/main.rs", "src/lib.rs", "tests/smoke.rs"]


class _Quit(Exception):
    """Raised from `_on_event` to unwind out of `run_tui()`."""


class ListDemo:
    """Owns the one piece of state this demo needs: which file is
    selected, and which files have been opened (activated), in order."""

    def __init__(self) -> None:
        self.selected_idx = 0
        self.opened: ListOf[str] = []

    def _on_select(self, idx: int) -> None:
        self.selected_idx = idx

    def _on_activate(self, idx: int) -> None:
        self.opened.append(FILES[idx])

    def view(self) -> Any:
        return List(
            items=[{"text": {"spans": [{"text": name, "fg": None, "bg": None}]}} for name in FILES],
            id="list:demo",
            selected=self.selected_idx,
            has_focus=True,
            on_select=self._on_select,
            on_activate=self._on_activate,
        )

    def on_event(self, event: "UiEvent") -> None:
        if isinstance(event, dict) and "KeyPressed" in event:
            key = event["KeyPressed"]["key"]
            if key == {"Char": "q"} or key == {"Named": "Escape"}:
                raise _Quit


def build() -> "tuple[App, ListDemo]":
    demo = ListDemo()
    app = App(WIDTH, HEIGHT, view=demo.view, on_event=demo.on_event)
    return app, demo


def _run_headless(app: App, demo: ListDemo) -> None:
    print("before any interaction:")
    print(app.screen())

    app.push_event({"List": ["list:demo", {"SelectionChanged": {"idx": 2}}]})
    app.run(1)
    print(f"\nafter selecting row 2, selected_idx == {demo.selected_idx}:")
    print(app.screen())

    app.push_event({"List": ["list:demo", {"ItemActivated": {"idx": 2}}]})
    app.run(1)
    print(f"\nactivating the selected row opened: {demo.opened}")


if __name__ == "__main__":
    app, demo = build()
    if "--tui" in sys.argv[1:]:
        try:
            app.run_tui()
        except _Quit:
            pass
        print(f"list_demo: opened {demo.opened}")
    else:
        _run_headless(app, demo)
