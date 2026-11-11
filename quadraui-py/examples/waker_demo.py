"""Demonstrates `QuiApp.waker()`: a background thread (standing in for a
slow network call, an asyncio loop running elsewhere, a subprocess watcher,
...) delivers a result into a real `run_tui()` terminal loop without ever
blocking the terminal's own event loop on it.

    maturin develop
    python quadraui-py/examples/waker_demo.py

The handler's own job stays on the terminal thread; "fetching" happens on a
plain `threading.Thread`. `QuiAppWaker.post` is the only bridge between the
two -- no locks, no polling, no change to `run_tui()`'s own event loop. The
delivered value arrives as a `{"User": value}` event, same shape any other
event takes.

Press `q` or Escape to quit before the background thread finishes -- a
terminal has no window manager "close" event the way a GTK window does, so
a quit keystroke raising from inside the handler is the only way to ask
`run_tui()` to return.
"""

from __future__ import annotations

import threading
import time
from typing import TYPE_CHECKING, Any, Callable, Dict, List

from quadraui import QuiApp, QuiAppWaker

if TYPE_CHECKING:
    # `quadraui.wire` is a stub-only module -- see its own module docstring --
    # so this import only runs under a type checker.
    from quadraui.wire import Key, UiEvent

WIDTH = 80
HEIGHT = 24


class _Quit(Exception):
    """Sentinel raised from the handler to unwind out of `run_tui()`."""


def _render() -> "Dict[str, Any]":
    return {
        "text_display": {
            "id": "demo:status",
            "lines": [
                {
                    "spans": [
                        {
                            "text": "waker_demo: waiting on a background thread... (q quits)",
                            "fg": None,
                            "bg": None,
                        }
                    ]
                }
            ],
        }
    }


def make_app() -> QuiApp:
    app = QuiApp(WIDTH, HEIGHT)
    app.set_render(_render)
    return app


def _is_quit_key(key: "Key") -> bool:
    return key == {"Char": "q"} or key == {"Named": "Escape"}


def fetch_in_background(waker: QuiAppWaker) -> None:
    """Stands in for a slow network call / subprocess / asyncio task that
    must not block the terminal's own event loop."""
    time.sleep(2)
    waker.post({"status": "fetched", "value": 42})


def make_handler(results: "List[Any]") -> "Callable[[UiEvent], None]":
    """Builds a handler closing over a plain Python list used as an
    out-of-band result slot, since the handler cannot write back to the
    `QuiApp` it was invoked from."""

    def handler(event: "UiEvent") -> None:
        if not isinstance(event, dict):
            return
        if "KeyPressed" in event and _is_quit_key(event["KeyPressed"]["key"]):
            raise _Quit
        if "User" in event:
            results.append(event["User"])
            raise _Quit

    return handler


if __name__ == "__main__":
    app = make_app()
    # Obtained before `run_tui()` starts -- see `QuiApp.waker()`'s doc for
    # why that's the one reachable way to get a handle that keeps
    # delivering once the real terminal loop takes over.
    waker = app.waker()
    results: List[Any] = []
    app.set_handler(make_handler(results))

    worker = threading.Thread(target=fetch_in_background, args=(waker,), daemon=True)
    worker.start()
    try:
        app.run_tui()
    except _Quit:
        pass
    if results:
        print(f"waker_demo: background thread delivered {results[0]!r}")
    else:
        print("waker_demo: quit before the background thread finished")
