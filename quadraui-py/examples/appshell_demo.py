"""AppShell-shaped demo: one `ListView` holding 10,000 rows, scrolled
smoothly from Python.

`AppShellDemo` holds `scroll_offset`/`selected_idx` as plain Python
attributes and registers a `render` callback that rebuilds the `list`
layout leaf from them on every frame -- the same "state lives in Python,
`render` rebuilds the descriptor from scratch" shape `tui_render_state_demo.py`
uses. The scroll loop itself is a plain Python `for` loop driving it one
page at a time, standing in for whatever drives real key/scroll-wheel
events (a bound handler must not call back into the same `QuiApp` it was
invoked from -- `QuiApp` rejects that reentrant borrow -- so this demo calls
`scroll()` directly rather than through `set_handler`).

Run headless:

    maturin develop
    python quadraui-py/examples/appshell_demo.py
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Dict

from quadraui import QuiApp

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import ListItemDescriptor

WIDTH = 80
HEIGHT = 24
ROW_COUNT = 10_000
PAGE = 250


def make_rows(count: int) -> "list[ListItemDescriptor]":
    return [
        {"text": {"spans": [{"text": f"Row {i:05d}", "fg": None, "bg": None}]}}
        for i in range(count)
    ]


class AppShellDemo:
    """Owns the plain-Python `scroll_offset`/`selected_idx` state a `render`
    callback reads to rebuild the `list` layout leaf on every frame."""

    def __init__(self, app: QuiApp, rows: "list[ListItemDescriptor]") -> None:
        self.app = app
        self.rows = rows
        self.selected_idx = 0
        self.scroll_offset = 0
        app.set_render(self._render)

    def _render(self) -> "Dict[str, Any]":
        return {
            "list": {
                "id": "list:appshell",
                "items": self.rows,
                "selected_idx": self.selected_idx,
                "scroll_offset": self.scroll_offset,
                "has_focus": True,
                "show_v_scrollbar": True,
            }
        }

    def scroll(self, delta: int) -> None:
        last = len(self.rows) - 1
        self.selected_idx = max(0, min(last, self.selected_idx + delta))
        self.scroll_offset = max(0, min(last, self.scroll_offset + delta))


def run_demo(steps: int = 20) -> "tuple[QuiApp, AppShellDemo, list[int]]":
    """Build the demo app, scroll down `steps` times, and return
    `(app, shell, offsets)` -- `offsets[0]` is the initial `scroll_offset`,
    `offsets[i]` the value after the i-th scroll step."""
    app = QuiApp(WIDTH, HEIGHT)
    shell = AppShellDemo(app, make_rows(ROW_COUNT))

    offsets = [shell.scroll_offset]
    for _ in range(steps):
        shell.scroll(PAGE)
        offsets.append(shell.scroll_offset)

    return app, shell, offsets


if __name__ == "__main__":
    app, shell, offsets = run_demo()
    print(app.screen())
    print(f"scroll offsets after each step: {offsets}")
