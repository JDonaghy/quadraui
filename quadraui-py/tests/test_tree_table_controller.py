"""Black-box coverage for the `"tree_table_controller"` leaf through
`quadraui-py`: the node tree is content the app sends from `render` every
frame, lazily loaded children included, while the user's expand,
selection, scroll and sort state stays with the widget across frames.
Every event reaches the registered handler as
`{"TreeTableController": (id, event)}`, the same tuple shape a flat
`TreeTable`'s own `{"TreeTable": (table_id, event)}` takes (see
`test_tree_table_interactions.py`).

The app answers `ChildrenRequested` from inside its own handler by
recording the children in its own model; the next `render` sends them.
A host that orders rows itself sends them in its own order with
`"sort_override": True`.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_tree_table_controller.py
"""

from quadraui import QuiApp

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "meta": False}
_TAB = {"KeyPressed": {"key": {"Named": "Tab"}, "modifiers": _NO_MODS, "repeat": False}}
_DOWN = {"KeyPressed": {"key": {"Named": "Down"}, "modifiers": _NO_MODS, "repeat": False}}
_LEFT = {"KeyPressed": {"key": {"Named": "Left"}, "modifiers": _NO_MODS, "repeat": False}}
_RIGHT = {"KeyPressed": {"key": {"Named": "Right"}, "modifiers": _NO_MODS, "repeat": False}}
_SPACE = {"KeyPressed": {"key": {"Char": " "}, "modifiers": _NO_MODS, "repeat": False}}
_ENTER = {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": _NO_MODS, "repeat": False}}
_HEADER_CLICK = {
    "MouseDown": {"button": "Left", "position": {"x": 5.0, "y": 0.0}, "modifiers": {}}
}

# What the app "loads" for each branch, in a deliberately unsorted order.
_TREE = {
    "root": [("zebra_dir", True), ("apple_file", False)],
    "zebra_dir": [("z_leaf", False), ("a_leaf", False)],
}


def _cell(text):
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


class _Files:
    """The app's own model: the children it has loaded so far, keyed by
    branch id, and the order it sorts the root's children in when it
    sorts them itself."""

    def __init__(self, *, answer_requests=True):
        self.answer_requests = answer_requests
        self.loaded = {}
        self.root_order = None
        self.received = []

    def node(self, node_id, has_children):
        node = {"id": node_id, "cells": [_cell(node_id)]}
        if has_children:
            node["has_children"] = True
            children = self.loaded.get(node_id)
            if node_id == "root" and children is not None and self.root_order is not None:
                children = sorted(children, key=lambda c: self.root_order.index(c[0]))
            if children is not None:
                node["children"] = [self.node(c, h) for c, h in children]
        return node

    def render(self):
        descriptor = {
            "id": "tt",
            "columns": [{"title": "Name", "width": {"Flex": 1.0}}],
            "nodes": [self.node("root", True)],
            "selected_id": "root",
            "sort_override": self.root_order is not None,
        }
        # A layout-tree leaf fills the whole area its container gives it,
        # which a tree deeper than one visible row needs.
        return {"row": [{"id": "tt", "tree_table_controller": descriptor}]}

    def handle(self, event):
        self.received.append(event)
        if "TreeTableController" not in event or not self.answer_requests:
            return
        _, payload = event["TreeTableController"]
        if "ChildrenRequested" in payload:
            node_id = payload["ChildrenRequested"]["id"]
            self.loaded[node_id] = _TREE[node_id]

    def events(self, event):
        return [
            entry["TreeTableController"]
            for entry in self.received
            if "TreeTableController" in entry and entry["TreeTableController"] == ("tt", event)
        ]


def _app(files):
    app = QuiApp(40, 12, render=files.render)
    app.screen()
    app.set_handler(files.handle)
    return app


def _press(app, event):
    app.push_event(event)
    app.run(1)


def _locate(screen, label):
    """`(row, column)` of `label`'s first occurrence in a rendered
    `screen()` string, or `(None, None)` if absent."""
    for row, line in enumerate(screen.splitlines()):
        col = line.find(label)
        if col != -1:
            return row, col
    return None, None


def test_three_level_lazy_tree_expand_sort_and_selection_round_trip():
    files = _Files()
    app = _app(files)

    # Tab focuses the sole tab stop; Space on the selected lazy root
    # expands it and requests its children, which the handler loads and
    # the next frame sends.
    _press(app, _TAB)
    _press(app, _SPACE)
    assert len(files.events({"ChildrenRequested": {"id": "root"}})) == 1, files.received
    assert app.screen_contains("zebra_dir")
    assert app.screen_contains("apple_file")

    # Down selects the first (still unsorted) child, so the selection
    # surviving the sort below is an id-keyed survival.
    _press(app, _DOWN)
    assert files.events({"RowSelected": {"id": "zebra_dir"}}), files.received

    _press(app, _SPACE)
    assert len(files.events({"ChildrenRequested": {"id": "zebra_dir"}})) == 1, files.received
    assert app.screen_contains("z_leaf")
    assert app.screen_contains("a_leaf")

    _press(app, _DOWN)
    assert files.events({"RowSelected": {"id": "z_leaf"}}), files.received

    # A header click sorts every loaded level's children, keeping the
    # hierarchy: "apple_file" above "zebra_dir", "a_leaf" above "z_leaf",
    # both leaves still nested under "zebra_dir".
    _press(app, _HEADER_CLICK)
    assert files.events({"SortChanged": {"col": 0, "ascending": True}}), files.received

    screen = app.screen()
    apple_row, _ = _locate(screen, "apple_file")
    zebra_row, zebra_indent = _locate(screen, "zebra_dir")
    a_leaf_row, a_leaf_indent = _locate(screen, "a_leaf")
    z_leaf_row, z_leaf_indent = _locate(screen, "z_leaf")
    assert None not in (apple_row, zebra_row, a_leaf_row, z_leaf_row), screen
    assert apple_row < zebra_row, screen
    assert a_leaf_indent > zebra_indent, screen
    assert z_leaf_indent > zebra_indent, screen
    assert zebra_row < a_leaf_row < z_leaf_row, screen

    # The selection survives the sort by node id: Enter activates
    # "z_leaf", which moved from first to second among its siblings.
    files.received.clear()
    _press(app, _ENTER)
    assert files.events({"RowActivated": {"id": "z_leaf"}}), files.received


def test_an_unanswered_lazy_branch_shows_a_loading_row_and_is_asked_once():
    files = _Files(answer_requests=False)
    app = _app(files)
    _press(app, _TAB)
    _press(app, _SPACE)
    assert app.screen_contains("…"), app.screen()

    # Collapsing and expanding again before the app answers asks nothing
    # more.
    _press(app, _LEFT)
    _press(app, _RIGHT)
    assert len(files.events({"ChildrenRequested": {"id": "root"}})) == 1, files.received


def test_sort_override_paints_the_order_the_app_sends():
    files = _Files()
    app = _app(files)
    _press(app, _TAB)
    _press(app, _SPACE)

    # The header click reports the sort; this app orders the root's
    # children itself, reverse-alphabetically, and says so with
    # `sort_override`, so the widget paints them as sent.
    _press(app, _HEADER_CLICK)
    assert files.events({"SortChanged": {"col": 0, "ascending": True}}), files.received
    files.root_order = ["zebra_dir", "apple_file"]

    screen = app.screen()
    apple_row, _ = _locate(screen, "apple_file")
    zebra_row, _ = _locate(screen, "zebra_dir")
    assert None not in (apple_row, zebra_row), screen
    assert zebra_row < apple_row, screen


def test_the_imperative_tree_table_methods_are_gone():
    app = QuiApp(40, 12)
    for name in ("set_tree_table_children", "set_tree_table_sort_override"):
        assert not hasattr(app, name), name
        assert not hasattr(app.actions(), name), name
