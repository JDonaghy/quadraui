"""Real-terminal analog of `appshell_demo.py`: the same `QuiApp` surface,
but shown in the actual controlling terminal via `run_tui()` -- raw mode,
alternate screen, live redraw on real crossterm keyboard/resize events --
instead of read back headlessly via `screen()`.

No extra feature flag is needed; just build and run in a real terminal
(not a pipe, not a CI runner with no tty):

    maturin develop
    python quadraui-py/examples/tui_interactive_demo.py

Interaction is keyboard-only, for the same reason `native_window_demo.py`
is: `quadraui` never turns a raw `MouseDown` into a "button click" on its
own, and Python has no access to the layout rects a Rust `render()` would
hit-test against, so this demo sticks to a focused button's `Enter`/`Space`
activation instead.

A bound handler must not call back into the `QuiApp` it was invoked from --
the same reentrant-borrow rule `appshell_demo.py` documents. That means
this handler can observe events and record them, but it cannot re-insert
the button/text-display with updated state while the terminal loop is
running; what's on screen is whatever was inserted before `run_tui()` was
called.

Press Enter or Space to "click" the button (recorded, printed after exit),
`q` or Escape to quit -- a terminal has no window manager "close" event the
way a GTK window does, so a quit keystroke raising from inside the handler
is the only way to ask `run_tui()` to return.
"""

from quadraui import QuiApp

# Only used to seed the headless `TuiDriver` this demo builds against before
# `run_tui()` takes over; `run_tui()` itself ignores both and sizes the real
# viewport from the actual terminal's current dimensions instead.
WIDTH = 80
HEIGHT = 24


class _Quit(Exception):
    """Sentinel raised from the handler to unwind out of `run_tui()`."""


def make_app():
    app = QuiApp(WIDTH, HEIGHT)
    app.insert_button(
        {
            "id": "demo:ok",
            "label": "OK (Enter/Space)",
            "focused": True,
        }
    )
    app.insert_text_display(
        {
            "id": "demo:label",
            "lines": [
                {
                    "spans": [
                        {
                            "text": "tui_interactive_demo: Enter/Space clicks, q quits",
                            "fg": None,
                            "bg": None,
                        }
                    ]
                }
            ],
        }
    )
    return app


def _is_quit_key(key):
    return key == {"Char": "q"} or key == {"Named": "Escape"}


def _is_activate_key(key):
    return key == {"Char": " "} or key == {"Named": "Enter"}


def make_handler(clicks):
    """Builds a handler closing over a plain Python list used as an
    out-of-band click counter, since the handler cannot write back to the
    `QuiApp` it was invoked from."""

    def handler(event):
        if isinstance(event, dict) and "KeyPressed" in event:
            key = event["KeyPressed"]["key"]
            if _is_quit_key(key):
                raise _Quit
            if _is_activate_key(key):
                clicks.append(1)

    return handler


if __name__ == "__main__":
    app = make_app()
    clicks = []
    app.set_handler(make_handler(clicks))
    try:
        app.run_tui()
    except _Quit:
        pass
    print(f"tui_interactive_demo: closed after {len(clicks)} click(s)")
