"""Black-box coverage for the virtualised-collection seam a real Python app
sees: a `data_table` leaf that sends only a 50-row slice of a 100,000-row
collection (the `items_window` extension key) paints a scrollbar extent for
the whole collection, scrolls by absolute row index, and -- the half this
test exists for -- asks the app for the rows a scroll brought into view, as
a synthesized `{"RowsRequested": {"id", "start", "count"}}` event delivered
to the same registered handler every `UiEvent` reaches.

Nothing in the app computes that request: `InteractionEngine::handle` does,
from the window the leaf's own decode recorded plus the offset the scroll
resolved to, and it never re-asks for a slice the app already supplied.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_windowed_rows_requested.py
"""

from quadraui import QuiApp

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "cmd": False}
_END = {"KeyPressed": {"key": {"Named": "End"}, "modifiers": _NO_MODS, "repeat": False}}
_TAB = {"KeyPressed": {"key": {"Named": "Tab"}, "modifiers": _NO_MODS, "repeat": False}}

TOTAL = 100_000
WINDOW_LEN = 50


def _cell(text):
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def _windowed_app(state):
    """A `QuiApp` whose render callback sends only `state["start"]`'s own
    slice of a `TOTAL`-row table, declaring the real total in
    `items_window`."""
    app = QuiApp(24, 8)

    def render():
        start = state["start"]
        length = min(WINDOW_LEN, TOTAL - start)
        rows = [{"cells": [_cell(f"row-{start + i}")]} for i in range(length)]
        return {
            "column": [
                {
                    "id": "table",
                    "data_table": {
                        "id": "table",
                        "columns": [{"title": "Row"}],
                        "rows": rows,
                        "items_window": {"start": start, "total": TOTAL},
                    },
                }
            ]
        }

    app.set_render(render)
    # Force the first paint so the table's own hit (and its window) are on
    # the hit map before any interaction below.
    app.screen()
    return app


def test_scrolling_past_the_loaded_window_asks_the_app_for_more_rows():
    state = {"start": 0}
    app = _windowed_app(state)

    received = []
    app.set_handler(lambda event: received.append(event))
    # Focus the table, then jump to the end of the whole 100,000-row
    # collection -- far outside the 50 rows the app has loaded.
    app.push_event(_TAB)
    app.push_event(_END)
    app.run(2)

    requests = [event["RowsRequested"] for event in received if "RowsRequested" in event]
    assert len(requests) == 1, f"expected exactly one RowsRequested: {received}"
    request = requests[0]
    assert request["id"] == "table"
    assert request["start"] + request["count"] >= TOTAL, request
    # Bounded: a 100,000-row collection is never requested whole.
    assert request["count"] < WINDOW_LEN * 10, request


def test_answering_the_request_paints_the_requested_rows():
    state = {"start": 0}
    app = _windowed_app(state)

    def handle(event):
        request = event.get("RowsRequested")
        if request is not None:
            # Exactly what a real app does with one: load that slice and
            # repaint.
            state["start"] = request["start"]
            return True
        return None

    app.set_handler(handle)
    app.push_event(_TAB)
    app.push_event(_END)
    app.run(2)

    # The app answered with the tail of the collection, so the rows that
    # paint are the ones around the absolute offset the table is actually
    # scrolled to -- not row 0's slice, and not blank placeholders.
    assert state["start"] > TOTAL - WINDOW_LEN * 10, state
    offset = app.widget_state("table")["scroll_offset"]
    assert offset > TOTAL - WINDOW_LEN * 10, offset
    screen = app.screen()
    # The leftmost column starts under the table's own border, so the row
    # label's first character is clipped out of the painted cell.
    assert f"ow-{offset}" in screen, screen


def test_a_covered_window_is_never_re_requested():
    state = {"start": 0}
    app = _windowed_app(state)

    received = []

    def handle(event):
        request = event.get("RowsRequested")
        if request is not None:
            received.append(request)
            state["start"] = request["start"]
            return True
        return None

    app.set_handler(handle)
    app.push_event(_TAB)
    app.push_event(_END)
    app.run(2)
    # The same key again, with the answered window now covering the
    # viewport: nothing left to ask for.
    app.push_event(_END)
    app.run(1)

    assert len(received) == 1, received
