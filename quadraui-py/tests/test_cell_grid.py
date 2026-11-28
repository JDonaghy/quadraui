"""Black-box coverage for the `cell_grid` `WIDGET_KINDS` seam exposed
through `quadraui-py`: a plain Python dict decodes straight into
`quadraui::CellGrid` (`WIDGET_KINDS`' generic registration already covers
the descriptor shape -- no new pymethod needed for that half), and the
cursor/selection/inline-edit/virtualisation round trip -- click a cell ->
`Navigated` -> page past the loaded window ->
`CellsRequested` -> answer with a freshly loaded block -> type + Enter ->
`CellEditCommitted` -- reaches the registered handler as `{"CellGrid":
(id, event)}`, mirroring `test_tree_table_controller.py`'s own
`{"TreeTableController": (id, event)}` shape.

Unlike the old per-id-cached-controller design, `CellGrid` carries no
state of its own across frames: `cursor`/`selection_anchor`/`row_scroll`/
`col_scroll`/`editing` all come from whatever `_descriptor` returns *this*
frame, so the test's own `state` dict has to apply every `Navigated`/
`CellEditStarted`/`CellEditChanged`/`CellEditCommitted`/`CellEditCancelled`
event back onto itself before the next render, the same way a real app's
host state must. Mirrors `quadraui-binding-core::engine`'s own
`CellGridLogic::apply` Rust test harness, driven end-to-end through this
binding's Python surface instead.

`CellsRequested` is answered the same way `test_windowed_rows_requested.py`
answers a `RowsRequested`: there is no separate `set_*` host call -- the
engine reconciles a `"cell_grid"` leaf's `cells`/`loaded_row_start`/
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
# A bare character key with no active edit starts one (`CellGrid::handle`'s
# own `RoutedEvent::Key` arm for a plain char) -- the synthesized
# `CharTyped` event only ever reaches `CellGrid::handle` once an edit is
# already under way (that type's own doc), so this is `KeyPressed`, not
# `CharTyped`, mirroring `TuiDriver::type_char`'s own `Key::Char` shape.
_TYPE_X = {"KeyPressed": {"key": {"Char": "x"}, "modifiers": _NO_MODS, "repeat": False}}
_CLICK_FIRST_CELL = {
    "MouseDown": {"button": "Left", "position": {"x": 9.0, "y": 1.5}, "modifiers": {}}
}
# Far enough from `_CLICK_FIRST_CELL` (> 1.5 cells) that `TuiBackend`'s
# `DoubleClickDetector` (400ms/1.5-cell window) doesn't fold the two
# consecutive `MouseDown`s below into a `DoubleClick`, which
# `CellGrid::handle` has no arm for.
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


def _initial_state():
    """A 20x5 sheet with only `LOADED_ROWS` rows loaded, starting at
    row 0 -- deliberately fewer rows than the 10-cell-tall viewport
    `_render` below paints into, so a `CellsRequested` is always one
    `click`/page away. Mirrors `quadraui-binding-core::engine`'s own
    `small_window_cell_grid`."""
    return {
        "cells": _cells(0, 0, LOADED_ROWS, COL_COUNT),
        "loaded_row_start": 0,
        "loaded_col_start": 0,
        "cursor": [0, 0],
        "selection_anchor": None,
        "row_scroll": 0,
        "col_scroll": 0,
        "editing": None,
    }


def _descriptor(state):
    return {
        "id": "cg",
        "row_count": ROW_COUNT,
        "col_count": COL_COUNT,
        "default_row_height": 1.0,
        "default_col_width": 8.0,
        "cells": state["cells"],
        "loaded_row_start": state["loaded_row_start"],
        "loaded_col_start": state["loaded_col_start"],
        "cursor": state["cursor"],
        "selection_anchor": state["selection_anchor"],
        "row_scroll": state["row_scroll"],
        "col_scroll": state["col_scroll"],
        "editing": state["editing"],
        "has_focus": True,
        "show_scrollbar": False,
    }


def _render(state):
    # A single-leaf `"row"` layout tree -- fills the whole viewport, the
    # same shape `test_tree_table_controller.py`'s own `render` uses and
    # for the same reason (the flat six-slot-plus-extras system's fixed
    # 3-line band is far too short for a sheet).
    return {"row": [{"id": "cg", "cell_grid": _descriptor(state)}]}


def _cell_grid_events(received, id_):
    return [
        entry["CellGrid"][1]
        for entry in received
        if "CellGrid" in entry and entry["CellGrid"][0] == id_
    ]


def _apply(state, event):
    """Writes one `CellGridEvent` back onto `state` the way a real host's
    own `cursor`/`selection_anchor`/`row_scroll`/`col_scroll`/`editing`
    fields must track it -- `CellGrid` itself is stateless across frames,
    so skipping this leaves the next render starting over at `(0, 0)` with
    no active edit, matching `CellGridLogic::apply`'s own Rust contract."""
    navigated = event.get("Navigated")
    if navigated is not None:
        state["cursor"] = navigated["cursor"]
        state["selection_anchor"] = navigated["selection_anchor"]
        state["row_scroll"] = navigated["row_scroll"]
        state["col_scroll"] = navigated["col_scroll"]
    started = event.get("CellEditStarted") or event.get("CellEditChanged")
    if started is not None:
        state["editing"] = started["edit"]
    if "CellEditCommitted" in event or "CellEditCancelled" in event:
        state["editing"] = None


def test_cell_grid_click_scroll_request_and_edit_round_trip():
    state = _initial_state()
    app = QuiApp(20, 10, render=lambda: _render(state))
    app.screen()

    received = []

    def on_event(event):
        received.append(event)
        if "CellGrid" not in event:
            return None
        _, payload = event["CellGrid"]
        _apply(state, payload)
        requested = payload.get("CellsRequested")
        if requested is None:
            return True
        # Exactly what a real app does with a `CellsRequested`: load that
        # absolute block and repaint -- there is no `set_*` call for this.
        row_start = requested["row_start"]
        rows = requested["rows"]
        cols = requested["cols"]
        state["cells"] = _cells(row_start, requested["col_start"], rows, cols)
        state["loaded_row_start"] = row_start
        state["loaded_col_start"] = requested["col_start"]
        return True

    app.set_handler(on_event)

    # A click on the first body cell both click-focuses "cg" and reports a
    # `Navigated` moving the cursor to (0, 0) -- `CellGrid::handle` reports
    # every click/key navigation as `Navigated`, never `CursorMoved` (that
    # variant is a host-derived event this leaf never actually emits, see
    # `CellGridEvent`'s own Rust doc).
    app.push_event(_CLICK_FIRST_CELL)
    app.run(1)
    navigated = [
        ev["Navigated"] for ev in _cell_grid_events(received, "cg") if "Navigated" in ev
    ]
    assert navigated and navigated[0]["cursor"] == (0, 0), received

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
    assert any("Navigated" in ev for ev in events), events

    # Typing "x" starts an inline edit at the cursor; Enter commits it.
    received.clear()
    app.push_event(_TYPE_X)
    app.push_event(_ENTER)
    app.run(2)
    committed = [
        ev["CellEditCommitted"] for ev in _cell_grid_events(received, "cg") if "CellEditCommitted" in ev
    ]
    assert len(committed) == 1, received
    assert committed[0]["text"] == "x", committed
