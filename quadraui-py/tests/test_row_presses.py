"""Black-box coverage for row presses and multi-select through the binding
engine: a right-click on a `DataTable` row asks for its context menu (two
quick right-clicks ask twice, never a double-click), a middle-click reports
its button, and a `"Multi"` table grows a selection from Ctrl/Shift clicks
and the keyboard chords, reported as `SelectionSet`. Two quick clicks on a
`Button` both reach `on_click`.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_row_presses.py
"""

from quadraui import QuiApp
from quadraui.app import App, Button
from quadraui.testing import harness

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "cmd": False}
_SHIFT = dict(_NO_MODS, shift=True)
_CTRL = dict(_NO_MODS, ctrl=True)

_COLUMNS = [{"title": "Name", "width": {"Native": 8.0}}]


def _row(name):
    return {"cells": [{"spans": [{"text": name, "fg": None, "bg": None}]}]}


def _table_app(mode="Single", list_leaf=False):
    """A one-column table (or list) whose rows sit at y == 1.0 + idx (the
    table header is row 0; a lone table slot is four cells tall, so rows
    0-2 are on screen) and which echoes `RowSelected`/`SelectionSet`
    back into its descriptor, as an app keeps it controlled."""
    app = QuiApp(24, 10)
    state = {"selected": None, "set": []}
    if list_leaf:
        items = [{"text": {"spans": [{"text": f"item{i}", "fg": None, "bg": None}]}} for i in range(5)]
        app.set_render(lambda: {"list": {"id": "list", "items": items, "selected_idx": 0}})
    else:
        app.set_render(
            lambda: {
                "data_table": {
                    "id": "table",
                    "columns": _COLUMNS,
                    "rows": [_row(f"row{i}") for i in range(5)],
                    "selected_idx": state["selected"],
                    "selection_mode": mode,
                    "selected_indices": state["set"],
                }
            }
        )
    app.screen()
    received = []

    def handle(event):
        received.append(event)
        payload = event.get("DataTable", (None, {}))[1]
        if "RowSelected" in payload:
            state["selected"] = payload["RowSelected"]["idx"]
            state["set"] = [state["selected"]]
        if "SelectionSet" in payload:
            state["set"] = payload["SelectionSet"]["indices"]

    app.set_handler(handle)
    return app, received


def _press(app, y, button="Left", mods=_NO_MODS, x=2.0):
    """Presses at (x, y). Consecutive presses less than 1.5 cells apart fold
    into a double-click, so the multi-select tests alternate `x`."""
    app.push_event(
        {"MouseDown": {"button": button, "position": {"x": x, "y": float(y)}, "modifiers": mods}}
    )
    app.push_event({"MouseUp": {"button": button, "position": {"x": x, "y": float(y)}}})
    app.run(2)


def _key(app, key, mods=_NO_MODS):
    app.push_event({"KeyPressed": {"key": key, "modifiers": mods, "repeat": False}})
    app.run(1)


def _payloads(received, kind="DataTable"):
    return [event[kind][1] for event in received if kind in event]


def test_a_right_click_on_a_table_row_requests_its_context_menu():
    app, received = _table_app()
    _press(app, 3, button="Right")  # row 2

    menus = [p["ContextMenuRequested"] for p in _payloads(received) if "ContextMenuRequested" in p]
    assert menus == [{"idx": 2, "position": {"x": 2.0, "y": 3.0}}], received
    assert not any("RowSelected" in p for p in _payloads(received)), received


def test_two_quick_right_clicks_are_two_context_menu_requests():
    app, received = _table_app()
    _press(app, 2, button="Right")
    _press(app, 2, button="Right")

    payloads = _payloads(received)
    assert sum("ContextMenuRequested" in p for p in payloads) == 2, received
    assert not any("RowDoubleClicked" in p for p in payloads), received


def test_a_middle_click_reports_its_button():
    app, received = _table_app()
    _press(app, 3, button="Middle")

    clicks = [p["RowClicked"] for p in _payloads(received) if "RowClicked" in p]
    assert clicks == [{"idx": 2, "button": "Middle", "modifiers": _NO_MODS}], received


def test_multi_table_grows_a_selection_from_clicks_and_keys():
    app, received = _table_app(mode="Multi")
    _press(app, 1)  # row 0
    _press(app, 3, mods=_SHIFT, x=6.0)  # row 2
    _press(app, 2, mods=_CTRL)  # row 1
    _key(app, {"Char": "a"}, _CTRL)

    sets = [p["SelectionSet"]["indices"] for p in _payloads(received) if "SelectionSet" in p]
    assert sets == [[0, 1, 2], [0, 2], [0, 1, 2, 3, 4]], received


def test_ctrl_space_toggles_and_plain_space_does_not():
    app, received = _table_app(mode="Multi")
    _press(app, 3)  # row 2
    _key(app, {"Char": " "})
    assert not any("SelectionSet" in p for p in _payloads(received)), received
    _key(app, {"Char": " "}, _CTRL)

    sets = [p["SelectionSet"]["indices"] for p in _payloads(received) if "SelectionSet" in p]
    assert sets == [[]], received


def test_shift_down_extends_a_multi_selection():
    app, received = _table_app(mode="Multi")
    _press(app, 1)  # row 0
    _key(app, {"Named": "Down"}, _SHIFT)
    _key(app, {"Named": "Down"}, _SHIFT)

    sets = [p["SelectionSet"]["indices"] for p in _payloads(received) if "SelectionSet" in p]
    assert sets == [[0, 1], [0, 1, 2]], received


def test_a_single_mode_table_treats_a_shift_click_as_a_plain_click():
    app, received = _table_app()
    _press(app, 1)
    _press(app, 3, mods=_SHIFT, x=6.0)

    payloads = _payloads(received)
    assert {"RowSelected": {"idx": 2}} in payloads, received
    assert not any("SelectionSet" in p for p in payloads), received


def test_a_list_reports_ctrl_click_and_right_click():
    app, received = _table_app(list_leaf=True)
    _press(app, 2, mods=_CTRL)
    _press(app, 1, button="Right")

    payloads = _payloads(received, "List")
    assert {"SelectionSet": {"indices": [0, 2]}} in payloads, received
    assert any("ContextMenuRequested" in p and p["ContextMenuRequested"]["idx"] == 1 for p in payloads)


def test_two_quick_clicks_on_a_button_count_twice():
    clicks = []
    view = lambda: Button("+", id="inc", on_click=lambda: clicks.append(1))  # noqa: E731
    h = harness(App(20, 3, view=view))
    h.click_text("+")
    h.click_text("+")
    assert len(clicks) == 2
