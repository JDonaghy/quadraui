"""Black-box acceptance test for `examples/windowed_list_demo.py`: the
first frame shows the rows the app sent itself, and End on the focused
list asks for the rows at the end of the collection, which the demo sends,
so the rows at the end paint in place of placeholders.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_windowed_list_demo.py
"""

import importlib.util
import pathlib

_DEMO = pathlib.Path(__file__).resolve().parent.parent / "examples" / "windowed_list_demo.py"
_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "cmd": False}


def _load_demo():
    spec = importlib.util.spec_from_file_location("windowed_list_demo", _DEMO)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _key(name):
    return {"KeyPressed": {"key": {"Named": name}, "modifiers": _NO_MODS, "repeat": False}}


def test_the_first_frame_shows_the_rows_the_app_sent():
    app, demo = _load_demo().build()
    assert app.screen_contains("row 0")
    assert demo.requests == []


def test_end_asks_for_the_last_rows_and_paints_them():
    module = _load_demo()
    app, demo = module.build()
    app.screen()
    app.push_event(_key("Tab"))
    app.push_event(_key("End"))
    app.run(2)
    assert len(demo.requests) == 1, demo.requests
    request = demo.requests[0]
    assert request["start"] + request["count"] >= module.TOTAL, request
    assert demo.start == request["start"]
    # The last row sits under the TUI focus ring's bottom edge; the one
    # above it is the last row the list shows.
    assert app.screen_contains(f"row {module.TOTAL - 2:,}"), app.screen()
