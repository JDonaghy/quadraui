"""Black-box coverage for the `CellGridController` per-widget-id seam
exposed through `quadraui-py`: a plain Python dict decodes into a
`"cell_grid"` extra (`EXTRA_PRIMITIVE_KINDS`' generic registration already
covers the descriptor shape -- no new pymethod needed for that half), and
the cursor/selection/inline-edit/virtualisation round trip -- click a cell
-> `CursorMoved` -> page past the loaded window -> `CellsRequested` ->
answer with a freshly loaded block -> type + Enter -> `CellEditCommitted`
-- reaches the registered handler as `{"CellGridController": (id,
event)}`, mirroring `test_tree_table_controller.py`'s own
`{"TreeTableController": (id, event)}` shape.

Mirrors `quadraui-binding-core::engine`'s own
`cell_grid_controller_cells_requested_does_not_repeat_after_host_loads_window`/
`cell_grid_controller_typed_edit_commits_on_enter` Rust tests, driven
end-to-end through this binding's Python surface instead.

`CellsRequested` is answered the same way `test_windowed_rows_requested.py`
answers a `RowsRequested`: there is no separate `set_*` host call -- the
engine reconciles a `"cell_grid"` extra's `cells`/`loaded_row_start`/
`loaded_col_start` fresh every frame, so mutating the render callback's own
state and returning `True` (redraw) from the handler is enough.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_cell_grid.py
"""

from quadraui import QuiApp

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "cmd": False}
_PAGE_DOWN = {
    "KeyPressed": {"key": {"Named": "PageDown"}, "modifiers": _NO_MODS, "repeat": False}
}
_ENTER = {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": _NO_MODS, "repeat": False}}
# A bare character key with no active edit starts one
# (`CellGridController::handle_key`'s `Key::Char(c) if !jump =>
# self.begin_typing(*c)` arm) -- the synthesized `CharTyped` event only
# ever reaches `CellGridController::handle` once an edit is already under
# way (that type's own doc), so this is `KeyPressed`, not `CharTyped`,
# mirroring `TuiDriver::type_char`'s own `Key::Char` shape.
_TYPE_X = {"KeyPressed": {"key": {"Char": "x"}, "modifiers": _NO_MODS, "repeat": False}}
_CLICK_FIRST_CELL = {
    "MouseDown": {"button": "Left", "position": {"x": 9.0, "y": 1.5}, "modifiers": {}}
}
# Far enough from `_CLICK_FIRST_CELL` (> 1.5 cells) that `TuiBackend`'s
# `DoubleClickDetector` (400ms/1.5-cell window) doesn't fold the two
# consecutive `MouseDown`s below into a `DoubleClick`, which
# `CellGridController::handle` has no arm for.
_CLICK_LAST_VISIBLE_ROW = {
    "MouseDown": {"button": "Left", "position": {"x": 9.0, "y": 8.5}, "modifiers": {}}
}

ROW_COUNT = 20
COL_COUNT = 5
LOADED_ROWS = 2


def _cell(text):
    return {"text": text, "style": {}}


def _cells(row_start, col_start, rows, cols):
    return [
        [_cell(f"r{row_start + r}c{col_start + c}") for c in range(cols)] for r in range(rows)
    ]


def _descriptor(state):
    """A 20x5 sheet with only `state["cells"]`' own row count loaded,
    starting at `state["loaded_row_start"]` -- deliberately fewer rows
    than the 10-cell-tall viewport `_render` below paints into, so a
    `CellsRequested` is always one `click`/page away. Mirrors
    `quadraui-binding-core::engine`'s own `small_window_cell_grid_descriptor`."""
    return {
        "id": "cg",
        "row_count": ROW_COUNT,
        "col_count": COL_COUNT,
        "cells": state["cells"],
        "loaded_row_start": state["loaded_row_start"],
        "loaded_col_start": 0,
        "has_focus": True,
        "show_scrollbar": False,
    }


def _render(state):
    # A single-leaf `"row"` layout tree -- fills the whole viewport, the
    # same shape `test_tree_table_controller.py`'s own `_render` uses and
    # for the same reason (the flat six-slot-plus-extras system's fixed
    # 3-line band is far too short for a sheet).
    return {"row": [{"id": "cg", "cell_grid": _descriptor(state)}]}


def _cell_grid_events(received, id_):
    return [
        entry["CellGridController"][1]
        for entry in received
        if "CellGridController" in entry and entry["CellGridController"][0] == id_
    ]


def test_cell_grid_controller_click_scroll_request_and_edit_round_trip():
    state = {
        "cells": _cells(0, 0, LOADED_ROWS, COL_COUNT),
        "loaded_row_start": 0,
    }
    app = QuiApp(20, 10, render=lambda: _render(state))
    app.screen()

    received = []

    def on_event(event):
        received.append(event)
        if "CellGridController" not in event:
            return None
        _, payload = event["CellGridController"]
        requested = payload.get("CellsRequested")
        if requested is None:
            return None
        # Exactly what a real app does with a `CellsRequested`: load that
        # absolute block and repaint -- there is no `set_*` call for this,
        # unlike `set_tree_table_children`.
        row_start = requested["row_start"]
        rows = requested["rows"]
        cols = requested["cols"]
        state["cells"] = _cells(row_start, requested["col_start"], rows, cols)
        state["loaded_row_start"] = row_start
        return True

    app.set_handler(on_event)

    # A click on the first body cell both click-focuses "cg" and reports
    # a `CursorMoved` to (0, 0).
    app.push_event(_CLICK_FIRST_CELL)
    app.run(1)
    assert {"CursorMoved": {"row": 0, "col": 0}} in _cell_grid_events(received, "cg"), received

    # Paging down past the 2 loaded rows scrolls the cursor beyond what's
    # loaded, requesting (and -- via `on_event` above -- immediately
    # getting answered with) a fresh block.
    received.clear()
    app.push_event(_PAGE_DOWN)
    app.run(1)
    cells_requested = [
        ev["CellsRequested"] for ev in _cell_grid_events(received, "cg") if "CellsRequested" in ev
    ]
    assert len(cells_requested) == 1, received
    assert cells_requested[0]["row_start"] > 0, cells_requested

    # The engine never re-asks for a block the host just loaded: a second
    # click onto an already-loaded cell reports only a cursor move.
    received.clear()
    app.push_event(_CLICK_LAST_VISIBLE_ROW)
    app.run(1)
    events = _cell_grid_events(received, "cg")
    assert not any("CellsRequested" in ev for ev in events), events
    assert any("CursorMoved" in ev for ev in events), events

    # Typing "x" starts an inline edit at the cursor; Enter commits it.
    received.clear()
    app.push_event(_TYPE_X)
    app.push_event(_ENTER)
    app.run(2)
    committed = [
        ev["CellEditCommitted"]
        for ev in _cell_grid_events(received, "cg")
        if "CellEditCommitted" in ev
    ]
    assert len(committed) == 1, received
    assert committed[0]["text"] == "x", committed
