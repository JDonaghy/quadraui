"""Black-box coverage for a selection the app moves itself: a `list` or
`data_table` leaf whose render callback sets `selected_idx` but leaves
`scroll_offset` alone scrolls that row into view on the next paint, the
same way a selection the widget's own keys moved does. A `scroll_offset`
the app pins stays where the app put it.
"""

from quadraui import QuiApp

ROWS = 200


def _cell(text):
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def _list_app(state):
    app = QuiApp(24, 8)

    def render():
        leaf = {
            "id": "list",
            "items": [{"text": _cell(f"item-{i}")} for i in range(ROWS)],
            "selected_idx": state["selected"],
        }
        if "scroll" in state:
            leaf["scroll_offset"] = state["scroll"]
        return {"column": [{"id": "list", "list": leaf}]}

    app.set_render(render)
    return app


def _table_app(state):
    app = QuiApp(24, 8)

    def render():
        return {
            "column": [
                {
                    "id": "table",
                    "data_table": {
                        "id": "table",
                        "columns": [{"title": "Row"}],
                        "rows": [{"cells": [_cell(f"row-{i}")]} for i in range(ROWS)],
                        "selected_idx": state["selected"],
                    },
                }
            ]
        }

    app.set_render(render)
    return app


def test_a_list_selection_the_app_moves_scrolls_into_view():
    state = {"selected": 0}
    app = _list_app(state)
    assert app.screen_contains("item-0")
    state["selected"] = 150
    screen = app.screen()
    assert "item-150" in screen
    assert app.widget_state("list")["scroll_offset"] > 0


def test_a_list_scroll_the_app_pins_stays_put():
    state = {"selected": 150, "scroll": 10}
    app = _list_app(state)
    screen = app.screen()
    assert "item-10" in screen
    assert "item-150" not in screen


def test_a_data_table_selection_the_app_moves_scrolls_into_view():
    state = {"selected": None}
    app = _table_app(state)
    assert app.screen_contains("row-0")
    state["selected"] = 120
    screen = app.screen()
    assert "row-120" in screen
    assert app.widget_state("table")["scroll_offset"] > 0
