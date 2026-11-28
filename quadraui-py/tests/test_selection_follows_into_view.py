"""Black-box coverage for a selection the app moves itself: a `list` or
`data_table` leaf whose render callback sets `selected_idx` but leaves
`scroll_offset` alone scrolls that row into view on the next paint, the
same way a selection the widget's own keys moved does. A `scroll_offset`
the app pins stays where the app put it. A windowed leaf follows an
absolute selection the same way, and a windowed tree's keyboard cursor
moves to a `selected_path` the app jumps to and keeps its row's path when
the app re-flattens the rows above it.
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


# ── windowed leaves: the selection is an absolute row index ──

WINDOW_TOTAL = 1000
LOADED = 50

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "cmd": False}


def _key(name):
    return {"KeyPressed": {"key": {"Named": name}, "modifiers": _NO_MODS, "repeat": False}}


def _windowed_list_app(state):
    app = QuiApp(24, 8)

    def render():
        return {
            "column": [
                {
                    "id": "list",
                    "list": {
                        "id": "list",
                        "items": [{"text": _cell(f"item-{i}")} for i in range(LOADED)],
                        "items_window": {"start": 0, "total": WINDOW_TOTAL},
                        "selected_idx": state["selected"],
                    },
                }
            ]
        }

    app.set_render(render)
    return app


def _windowed_table_app(state):
    app = QuiApp(24, 8)

    def render():
        return {
            "column": [
                {
                    "id": "table",
                    "data_table": {
                        "id": "table",
                        "columns": [{"title": "Row"}],
                        "rows": [{"cells": [_cell(f"row-{i}")]} for i in range(LOADED)],
                        "items_window": {"start": 0, "total": WINDOW_TOTAL},
                        "selected_idx": state["selected"],
                    },
                }
            ]
        }

    app.set_render(render)
    return app


def test_a_windowed_list_selection_the_app_moves_scrolls_into_view():
    state = {"selected": 0}
    app = _windowed_list_app(state)
    assert app.screen_contains("item-0")
    state["selected"] = 40
    screen = app.screen()
    assert "item-40" in screen, screen
    assert app.widget_state("list")["scroll_offset"] > 0


def test_a_windowed_data_table_selection_the_app_moves_scrolls_into_view():
    state = {"selected": None}
    app = _windowed_table_app(state)
    assert app.screen_contains("row-0")
    state["selected"] = 40
    screen = app.screen()
    assert "row-40" in screen, screen
    assert app.widget_state("table")["scroll_offset"] > 0


def _tree_row(path, text, expanded=None):
    return {
        "path": path,
        "indent": len(path) - 1,
        "icon": None,
        "text": _cell(text),
        "badge": None,
        "is_expanded": expanded,
    }


def _windowed_tree_app(state, activated):
    """A windowed tree the app flattens itself: branch `[0]` (five
    children while expanded), then leaves `[1]..[40]`. The app echoes every
    `SelectionChanged` into `state["selected"]` and records each
    `RowActivated` path in `activated`."""
    app = QuiApp(24, 8)

    def rows():
        out = [_tree_row([0], "branch", expanded=not state["collapsed"])]
        if not state["collapsed"]:
            out += [_tree_row([0, i], f"child-{i}") for i in range(5)]
        out += [_tree_row([i], f"leaf-{i}") for i in range(1, 41)]
        return out

    def render():
        loaded = rows()
        return {
            "column": [
                {
                    "id": "tree",
                    "tree": {
                        "id": "tree",
                        "rows": loaded,
                        "items_window": {"start": 0, "total": len(loaded)},
                        "selection_mode": "Single",
                        "selected_path": state["selected"],
                        "style": {
                            "indent": 2,
                            "show_chevrons": True,
                            "chevron_expanded": "v",
                            "chevron_collapsed": ">",
                        },
                    },
                }
            ]
        }

    def on_event(event):
        if "Tree" not in event:
            return
        _, tree_event = event["Tree"]
        if "SelectionChanged" in tree_event:
            state["selected"] = tree_event["SelectionChanged"]["path"]
        elif "RowActivated" in tree_event:
            activated.append(tree_event["RowActivated"]["path"])

    app.set_render(render)
    app.set_handler(on_event)
    app.screen()
    return app


def _press(app, *names):
    for name in names:
        app.push_event(_key(name))
        app.run(1)


def test_a_windowed_tree_follows_a_selection_the_app_jumps_to():
    state = {"selected": None, "collapsed": False}
    activated = []
    app = _windowed_tree_app(state, activated)
    _press(app, "Tab", "Down", "Down", "Down")
    assert state["selected"] == [0, 1], state
    state["selected"] = [5]
    app.screen()
    assert app.widget_state("tree")["selected_path"] == [5]
    _press(app, "Enter")
    assert activated == [[5]], activated


def test_a_windowed_tree_cursor_keeps_its_path_when_rows_above_it_collapse():
    state = {"selected": None, "collapsed": False}
    activated = []
    app = _windowed_tree_app(state, activated)
    _press(app, "Tab", *(["Down"] * 9))
    assert state["selected"] == [3], state
    state["collapsed"] = True
    app.screen()
    assert app.widget_state("tree")["selected_path"] == [3]
    _press(app, "Enter")
    assert activated == [[3]], activated
