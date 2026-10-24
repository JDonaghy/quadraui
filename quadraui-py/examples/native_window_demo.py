"""Native-window analog of `appshell_demo.py`: the same `QuiApp` surface,
but shown in a real OS window via `run_native()` instead of read back
headlessly via `screen()`.

Nothing below is backend-specific -- `run_native()` opens a GTK window on
Linux, a real AppKit window on macOS, or a real Win32 window on Windows,
depending on which feature the extension was built with (`gtk` wins over
`macos`, and both win over `win`, if more than one is enabled), and this
demo's `title`-only call works unchanged on every backend. Requires the
extension to be built with the `gtk`, `macos`, or `win` feature:

    maturin develop --features gtk     # Linux
    maturin develop --features macos   # macOS
    maturin develop --features win     # Windows
    python quadraui-py/examples/native_window_demo.py

Interaction is keyboard-only, for two reasons specific to this spike:

- `quadraui` itself never turns a raw `MouseDown` into a "button click" --
  every app does its own hit-testing against the layout rects its own
  `render()` just computed (see `quadraui/examples/common/controls.rs`).
  Python has no access to those rects, so there is nothing to hit-test
  against; a focused button's `Enter`/`Space` activation needs no rect at
  all, so that is the path this demo exercises instead.
- A bound handler must not call back into the `QuiApp` it was invoked
  from -- the same reentrant-borrow rule `appshell_demo.py` documents.
  That means this handler can observe events and print to stdout, but it
  cannot re-insert the button/list/text-display with updated state while
  the window is open; what's on screen is whatever was inserted before
  `run_native()` was called.

Press Enter or Space to "click" the button (prints a line to stdout),
`q` or Escape to close the window, or just close it from the window
manager. Closing from the window manager raises `_Quit` from inside the
handler; the `q`/Escape path instead returns `False`, the other way a
handler can ask `run_native()` to return -- both exercised here so neither
path bit-rots.

The two paths look the same on GTK and Win32 but differ visibly on macOS:
the `q`/Escape path returns from `run_native()` without tearing the window
down (AppKit's own exit route ends the whole process, so this binding
stops pumping instead), so the window stays on screen, frozen, until the
interpreter exits a moment later at the end of this script. On GTK and
Win32, that same path destroys the window immediately, same as closing
from the window manager. Closing from the window manager closes the
window first on every backend. All three end with the final
`closed after N click(s)` line on stdout.
"""

from quadraui import QuiApp

WIDTH = 80
HEIGHT = 24

ROWS = [f"item-{i:02d}" for i in range(5)]


class _Quit(Exception):
    """Sentinel raised from the handler to unwind out of `run_native()`."""


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
                            "text": "native_window_demo: Enter/Space clicks, q quits",
                            "fg": None,
                            "bg": None,
                        }
                    ]
                }
            ],
        }
    )
    app.insert_list(
        {
            "id": "demo:list",
            "items": [
                {"text": {"spans": [{"text": row, "fg": None, "bg": None}]}}
                for row in ROWS
            ],
            "selected_idx": 0,
            "scroll_offset": 0,
            "has_focus": False,
            "show_v_scrollbar": True,
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
        if event == "WindowClose":
            print("native_window_demo: window close requested")
            raise _Quit
        if isinstance(event, dict) and "KeyPressed" in event:
            key = event["KeyPressed"]["key"]
            if _is_quit_key(key):
                print("native_window_demo: quit key pressed")
                # Returning `False` is the non-exception way to ask
                # `run_native()` to exit -- no exception to catch here.
                return False
            if _is_activate_key(key):
                clicks.append(1)
                print(f"native_window_demo: button activated (click #{len(clicks)})")

    return handler


if __name__ == "__main__":
    app = make_app()
    clicks = []
    app.set_handler(make_handler(clicks))
    try:
        app.run_native(title="quadraui native window demo")
    except _Quit:
        pass
    print(f"native_window_demo: closed after {len(clicks)} click(s)")
