"""Unit tests for `quadraui.app`: id auto-assignment stability across
re-render, callback dispatch (`Tree`, `Table`, `Tabs`, `Button`), component
state survival, and redraw-on-mutation -- the four properties the app
layer's own module doc promises and `examples/file_manager_app.py` relies
on wholesale.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_app_layer.py
"""

from quadraui.app import App, Button, Column, Component, Table, Tabs, Tab, TextDisplay, Tree, key_name


_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "cmd": False}


def _key_event(key):
    return {"KeyPressed": {"key": key, "modifiers": _NO_MODS, "repeat": False}}


# --- id auto-assignment -------------------------------------------------


def test_auto_assigned_ids_are_stable_across_rerender_at_the_same_position():
    """A node with a callback but no explicit `id=` exposes its resolved
    auto-id through the registry; re-rendering at the same tree position
    (nothing before `Button` in the tree ever changes shape here) must
    resolve to that same id every time, or a callback registered against
    the first render's id would go stale the moment the second render's
    event needs to look it up."""
    calls = []

    def view():
        return Button("Save", on_click=lambda: calls.append(1))

    app = App(20, 5, view=view)
    ids_before = set(app._registry.keys())
    assert app.screen_contains("Save")
    app.step()
    ids_after = set(app._registry.keys())
    assert ids_before == ids_after
    assert len(ids_before) == 1


def test_auto_ids_differ_by_structural_position():
    def view():
        return Column(
            Button("A", on_click=lambda: None),
            Button("B", on_click=lambda: None),
        )

    app = App(20, 5, view=view)
    assert app.screen_contains("A")
    ids = sorted(app._registry.keys())
    assert len(ids) == 2
    assert ids[0] != ids[1]


# --- callback dispatch ---------------------------------------------------


def test_tree_row_click_and_toggle_expand_dispatch_to_their_own_callback():
    clicked = []
    toggled = []

    def view():
        return Tree(
            rows=[{"path": [0], "indent": 0, "icon": None, "text": {"spans": []}, "badge": None, "is_expanded": False}],
            id="t:tree",
            on_row_click=lambda path: clicked.append(path),
            on_toggle_expand=lambda path: toggled.append(path),
        )

    app = App(20, 5, view=view)
    app.push_event({"Tree": ["t:tree", {"RowClicked": {"path": [0], "modifiers": _NO_MODS}}]})
    app.step()
    assert clicked == [[0]]
    assert toggled == []

    app.push_event({"Tree": ["t:tree", {"RowToggleExpand": {"path": [0]}}]})
    app.step()
    assert toggled == [[0]]


def test_table_row_double_clicked_dispatches_on_activate_not_on_select():
    activated = []
    selected = []

    def view():
        return Table(
            columns=[{"title": "Name", "width": {"Flex": 1.0}, "align": "Left"}],
            rows=[{"cells": [{"spans": [{"text": "a", "fg": None, "bg": None}]}]}],
            id="t:grid",
            on_activate=lambda idx: activated.append(idx),
            on_select=lambda idx: selected.append(idx),
        )

    app = App(20, 5, view=view)
    app.push_event({"DataTable": ["t:grid", {"RowSelected": {"idx": 0}}]})
    app.step()
    assert selected == [0]
    assert activated == []

    app.push_event({"DataTable": ["t:grid", {"RowDoubleClicked": {"idx": 0}}]})
    app.step()
    assert activated == [0]


def test_tabs_closed_dispatches_on_close_with_the_index():
    closed = []

    def view():
        return Tabs(
            tabs=[Tab(TextDisplay([], id="t:c0"), title="one")],
            id="t:tabs",
            on_close=lambda idx: closed.append(idx),
        )

    app = App(20, 5, view=view)
    app.push_event({"TabBar": ["t:tabs", {"TabClosed": {"index": 0}}]})
    app.step()
    assert closed == [0]


def _click_at(app, x=0.0, y=0.0):
    """Pushes a `MouseDown` at `(x, y)` -- the real `InteractionEngine` hit-
    tests it against whatever actually painted there and attaches the
    `widget`/`hit` this layer's own `on_click` dispatch reads; a test never
    sets either by hand (there is no way to invent a correct `hit` from
    Python -- only the Rust-side layout knows what it painted where)."""
    app.push_event({"MouseDown": {"button": "Left", "position": {"x": x, "y": y}, "modifiers": {}}})
    app.step()


def test_button_on_click_fires_on_a_hit_tested_mouse_down():
    clicks = []

    def view():
        return Button("Save", id="t:save", on_click=lambda: clicks.append(1))

    app = App(20, 5, view=view)
    _click_at(app)
    assert clicks == [1]


def test_button_on_click_fires_on_enter_while_focused():
    clicks = []

    def view():
        return Button("Save", id="t:save", on_click=lambda: clicks.append(1), focused=True)

    app = App(20, 5, view=view)
    app.push_event({"FocusChanged": "t:save"})
    app.step()
    app.push_event(_key_event({"Named": "Enter"}))
    app.step()
    assert clicks == [1]


def test_unmatched_event_falls_through_to_on_event():
    seen = []

    def view():
        return TextDisplay([], id="t:text")

    app = App(20, 5, view=view, on_event=lambda event: seen.append(event))
    app.push_event(_key_event({"Named": "Down"}))
    app.step()
    assert len(seen) == 1
    assert key_name(seen[0]) == "Down"


# --- components -----------------------------------------------------------


class Counter(Component):
    """A `Button` whose label shows a count that only its own `on_click`
    ever increments -- no explicit `id=`, so two of these at different tree
    positions (`test_two_components_in_a_list_keep_independent_state_by_position`)
    resolve to two different auto-ids, and therefore two different state
    slots, with no `key=` needed."""

    def initial_state(self):
        return {"count": 0}

    def view(self, state):
        return Button(
            f"count={state['count']}",
            focused=True,
            on_click=lambda: state.__setitem__("count", state["count"] + 1),
        )


def _label(app):
    """The button's own wire-level label this frame -- asserted against
    directly (rather than `screen_contains`) so this test stays scoped to
    what the app layer itself is responsible for: resolving the right
    wire dict from the current component state. What a backend's own
    rasterizer paints for a given wire dict (hover chrome, focus rings,
    …) is that backend's concern, not this layer's."""
    return app._render()["button"]["label"]


def test_component_state_survives_rerender_and_mutation_is_visible_next_render():
    """Repeated real `Enter`-while-focused clicks (`test_button_on_click_fires_on_enter_while_focused`
    already covers that dispatch path on its own) must each land on the
    *same* component state slot and each repaint must already show the
    mutation `on_click` just made -- with no explicit redraw call
    anywhere in `Counter.view`."""
    app = App(20, 5, view=lambda: Counter())
    assert _label(app) == "count=0"

    button_id = next(iter(app._registry))
    app.push_event({"FocusChanged": button_id})
    app.step()

    for expected in ("count=1", "count=2", "count=3"):
        app.push_event(_key_event({"Named": "Enter"}))
        app.step()
        assert _label(app) == expected


def test_two_components_in_a_list_keep_independent_state_by_position():
    """Two `Counter`s in the same `Column` resolve to two distinct auto-ids
    (`test_auto_ids_differ_by_structural_position` already covers that id
    derivation on its own); invoking only the first one's own `on_click`
    straight from the registry -- bypassing mouse hit-testing, which this
    test has nothing to do with -- must leave the second one's count
    untouched."""

    def view():
        return Column(Counter(), Counter())

    app = App(20, 5, view=view)
    ids = sorted(app._registry.keys())
    assert len(ids) == 2

    app._registry[ids[0]]["on_click"]()
    app.step()

    screen = app.screen()
    assert "count=1" in screen
    assert "count=0" in screen
