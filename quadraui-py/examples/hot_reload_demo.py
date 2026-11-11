"""Hot reload with state preservation, on `quadraui.app`.

    python examples/hot_reload_demo.py --tui      # a real terminal
    python examples/hot_reload_demo.py --native   # a native window
    python examples/hot_reload_demo.py            # headless smoke run

While it runs, edit `examples/hot_reload/notes_view.py` and save: the
window repaints with the new code within half a second, and the counter
value, the note list and the list's selection all stay as they were. Save
a syntax error and the last good version keeps running, with the error in
the status bar, until the file is fixed. Press `q` or Escape (or close
the window) to quit.

The view lives in its own module because a script run directly is
`__main__`, which the import system cannot re-import.
"""

from __future__ import annotations

import pathlib
import sys
from typing import Any, Tuple

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from hot_reload import notes_view  # noqa: E402

from quadraui.app import App, key_name  # noqa: E402


class _Quit(Exception):
    """Raised from `on_event` to unwind out of `run_tui()`/`run_native()`."""


def _quit_on_q(event: Any) -> None:
    if key_name(event) in ("q", "Escape"):
        raise _Quit


def build(width: int = 60, height: int = 24, interval: float = 0.5) -> Tuple[App, notes_view.NotesApp]:
    """The demo's `App`, watching `notes_view` for edits."""
    notes = notes_view.NotesApp()
    app = App(width, height, view=notes.view, on_event=_quit_on_q)
    notes.app = app
    app.enable_hot_reload(notes_view, interval=interval)
    return app, notes


if __name__ == "__main__":
    flags = set(sys.argv[1:])
    app, _notes = build()
    try:
        if "--tui" in flags:
            app.run_tui()
        elif "--native" in flags:
            app.run_native(title="quadraui hot reload")
        else:
            app.step()
            print(app.screen())
    except _Quit:
        pass
    finally:
        app.disable_hot_reload()
