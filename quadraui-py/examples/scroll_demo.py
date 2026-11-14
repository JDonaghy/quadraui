"""Runnable demo of the `Scroll` node helper: 40 buttons stacked in a
`Column`, wrapped in a `Scroll` viewport that only shows 10 rows at a
time -- the issue's own "40 buttons in a 10-row viewport" shape.

Run headless (drives the scroll with synthetic wheel/click events and
prints the before/after screens, plus which button a click on the
viewport's top row now resolves to):

    maturin develop
    python quadraui-py/examples/scroll_demo.py

Run in a real terminal (mouse wheel/PageDown/PageUp/scrollbar-drag
actually scroll it, clicking a button prints its id below the viewport,
`q`/Escape quits):

    python quadraui-py/examples/scroll_demo.py --tui

Either way this is an *uncontrolled* `Scroll` -- `offset` is left unset
on every render, so `quadraui`'s own `InteractionEngine` owns the
position across frames; see `Scroll`'s own docstring for the controlled
alternative (pinning `offset` from app state on every render).
"""

from __future__ import annotations

import sys
from typing import TYPE_CHECKING, Any, Callable, List

from quadraui.app import App, Button, Column, Scroll

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import UiEvent

WIDTH = 40
HEIGHT = 10
BUTTON_COUNT = 40


class _Quit(Exception):
    """Raised from `_on_event` to unwind out of `run_tui()`."""


class ScrollDemo:
    """Owns the one piece of state this demo needs: which buttons have
    been clicked, in click order."""

    def __init__(self) -> None:
        self.clicked: List[str] = []

    def _on_click(self, button_id: str) -> None:
        self.clicked.append(button_id)

    def _click_handler(self, button_id: str) -> Callable[[], None]:
        def handler() -> None:
            self._on_click(button_id)

        return handler

    def view(self) -> Any:
        return Scroll(
            Column(
                *[
                    Button(
                        f"Item{i:02d}",
                        id=f"btn:{i}",
                        on_click=self._click_handler(f"btn:{i}"),
                    )
                    for i in range(BUTTON_COUNT)
                ]
            ),
            id="scroll:demo",
        )

    def on_event(self, event: "UiEvent") -> None:
        if isinstance(event, dict) and "KeyPressed" in event:
            key = event["KeyPressed"]["key"]
            if key == {"Char": "q"} or key == {"Named": "Escape"}:
                raise _Quit


def build() -> "tuple[App, ScrollDemo]":
    demo = ScrollDemo()
    app = App(WIDTH, HEIGHT, view=demo.view, on_event=demo.on_event)
    return app, demo


def _run_headless(app: App, demo: ScrollDemo) -> None:
    print("before scrolling:")
    print(app.screen())

    # A wheel notch landing on the scrollbar's own track/thumb -- the
    # rightmost viewport column, reserved once content overflows it --
    # moves the container the same way a real mouse wheel would.
    app.push_event(
        {"Scroll": {"delta": {"x": 0.0, "y": 10.0}, "position": {"x": WIDTH - 1.0, "y": 1.0}}}
    )
    app.run(1)
    print("\nafter scrolling 10 rows down:")
    print(app.screen())

    # The hit map moved with the paint, not just the pixels: clicking the
    # viewport's top row resolves to the button the scroll brought there
    # ("btn:10"), not the button that painted there before the scroll
    # ("btn:0").
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 1.0, "y": 0.0}, "modifiers": {}}}
    )
    app.run(1)
    print(f"\nclicking the top row now reports: {demo.clicked}")


if __name__ == "__main__":
    app, demo = build()
    if "--tui" in sys.argv[1:]:
        try:
            app.run_tui()
        except _Quit:
            pass
        print(f"scroll_demo: clicked {demo.clicked}")
    else:
        _run_headless(app, demo)
