"""Runnable demo of row windowing: a `list` leaf over a 1,000,000-row
collection that sends only the slice of rows around its viewport,
declares the real total in `items_window`, and answers each
`RowsRequested` with the slice it names.

Two rules this demo follows:

- Indices are absolute. `items_window: {"start", "total"}` places the
  loaded `items` at rows `start..` of `total`, and `selected_idx`,
  `scroll_offset` and the row index in every `List` event count from the
  start of the whole collection, not of the slice.
- The app sends the rows its first viewport shows itself. A windowed
  widget asks for rows only after input (a key, a click, the wheel), so a
  first frame, or a viewport that grows on resize, over rows the app has
  not sent paints placeholders until the next input asks for them.

Run headless (jumps to the end of the collection and prints the request
the list made and the screen after the app answered it):

    maturin develop
    python quadraui-py/examples/windowed_list_demo.py

Run in a real terminal (arrow keys/PageDown/End/the wheel move through
the whole collection, `q`/Escape quits):

    python quadraui-py/examples/windowed_list_demo.py --tui
"""

from __future__ import annotations

import sys
from typing import TYPE_CHECKING, Any, Dict, List

from quadraui import QuiApp

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import UiEvent

WIDTH = 30
HEIGHT = 8
TOTAL = 1_000_000
# The first slice: comfortably more than one viewport, so the first frame
# never shows a placeholder.
FIRST_SLICE = 50


class _Quit(Exception):
    """Raised from `on_event` to unwind out of `run_tui()`."""


class WindowedListDemo:
    """Owns the loaded slice (`start`, `count`), the selected row (an
    absolute index) and the requests the list made, in order."""

    def __init__(self) -> None:
        self.start = 0
        self.count = FIRST_SLICE
        self.selected = 0
        self.requests: List[Dict[str, Any]] = []

    def render(self) -> Any:
        count = min(self.count, TOTAL - self.start)
        items = [
            {"text": {"spans": [{"text": f"row {self.start + i:,}", "fg": None, "bg": None}]}}
            for i in range(count)
        ]
        return {
            "column": [
                {
                    "id": "rows",
                    "list": {
                        "id": "rows",
                        "items": items,
                        "items_window": {"start": self.start, "total": TOTAL},
                        "selected_idx": self.selected,
                        "has_focus": True,
                    },
                }
            ]
        }

    def on_event(self, event: "UiEvent") -> None:
        if not isinstance(event, dict):
            return
        if "RowsRequested" in event:
            request = event["RowsRequested"]
            self.requests.append(dict(request))
            self.start = request["start"]
            self.count = request["count"]
        elif "List" in event:
            _, list_event = event["List"]
            if isinstance(list_event, dict) and "SelectionChanged" in list_event:
                self.selected = list_event["SelectionChanged"]["idx"]
        elif "KeyPressed" in event:
            key = event["KeyPressed"]["key"]
            if key == {"Char": "q"} or key == {"Named": "Escape"}:
                raise _Quit


def build() -> "tuple[QuiApp, WindowedListDemo]":
    demo = WindowedListDemo()
    app = QuiApp(WIDTH, HEIGHT, on_event=demo.on_event, render=demo.render)
    return app, demo


def _run_headless(app: QuiApp, demo: WindowedListDemo) -> None:
    print("first frame:")
    print(app.screen())

    app.push_event(
        {"KeyPressed": {"key": {"Named": "Tab"}, "modifiers": {}, "repeat": False}}
    )
    app.push_event(
        {"KeyPressed": {"key": {"Named": "End"}, "modifiers": {}, "repeat": False}}
    )
    app.run(2)
    print(f"\nthe list asked for: {demo.requests}")
    print("\nafter End, with the requested rows sent:")
    print(app.screen())


if __name__ == "__main__":
    app, demo = build()
    if "--tui" in sys.argv[1:]:
        try:
            app.run_tui()
        except _Quit:
            pass
        print(f"windowed_list_demo: {len(demo.requests)} request(s)")
    else:
        _run_headless(app, demo)
