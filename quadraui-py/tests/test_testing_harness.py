"""Black-box coverage for `quadraui.testing`, the app-author test harness:
the query/act/assert API a consumer writes their own app's tests against.

Each test below drives a small real app through the harness and checks the
harness reported what the app actually rendered -- the harness is only
worth anything if a green assertion means the app really is in that state,
so nothing here inspects the app object.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_testing_harness.py
"""

import pytest

from quadraui import QuiApp
from quadraui.app import App, Button, Column, SidebarPanel, Split, Tab, Table, Tabs, TextDisplay, Toolbar, Tree, key_name
from quadraui.testing import Harness, WidgetNotFound, harness


def _line(text):
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def _tree_row(path, text, expanded=False):
    return {
        "path": list(path),
        "indent": len(path) - 1,
        "icon": None,
        "text": _line(text),
        "badge": None,
        "is_expanded": expanded,
    }


def _table_row(*cells):
    return {"cells": [_line(cell) for cell in cells]}


# --- a tiny app exercising every shape the harness knows ---------------------


class Demo:
    """A tree over a table, a tab strip, a button and a toolbar -- one of
    every shape the harness has a verb for."""

    def __init__(self, width=70, height=24):
        self.log = []
        self.open_tabs = ["one"]
        self.active = 0
        self.keys = []
        self.app = App(width, height, view=self.view, on_event=self._on_event)

    def _on_event(self, event):
        name = key_name(event)
        if name is not None:
            self.keys.append(name)

    def view(self):
        return Split(
            first=Tree(
                [
                    _tree_row([0], "alpha", expanded=True),
                    _tree_row([0, 0], "alpha-child"),
                    _tree_row([1], "beta"),
                ],
                id="d:tree",
                selected_path=[0],
                on_row_click=lambda path: self.log.append(("click", tuple(path))),
                on_select=lambda path: self.log.append(("select", tuple(path))),
                on_activate=lambda path: self.log.append(("activate", tuple(path))),
                on_toggle_expand=lambda path: self.log.append(("toggle", tuple(path))),
            ),
            second=Tabs(
                tabs=[
                    Tab(
                        Table(
                            columns=[{"title": "Name"}, {"title": "Kind"}],
                            rows=[_table_row("a1.txt", "file"), _table_row("sub", "folder")],
                            id=f"d:grid:{i}",
                            on_select=lambda idx: self.log.append(("row-select", idx)),
                            on_activate=lambda idx: self.log.append(("row-activate", idx)),
                        ),
                        title=title,
                    )
                    for i, title in enumerate(self.open_tabs)
                ],
                id="d:tabs",
                on_activate=lambda index: self.log.append(("tab", index)),
                on_close=lambda index: self.log.append(("close", index)),
            ),
            id="d:split",
            orientation="horizontal",
            ratio=0.4,
        )


@pytest.fixture
def demo():
    return Demo()


@pytest.fixture
def ui(demo):
    return harness(demo.app)


# --- querying ----------------------------------------------------------------


def test_find_locates_a_widget_by_id_role_or_text(ui):
    assert ui.find(id="d:tree").role == "tree"
    assert ui.find(role="data_table").id == "d:grid:0"
    assert ui.find(role="tree", text="beta").id == "d:tree"
    assert ui.root.role == "split"


def test_find_raises_a_readable_error_when_nothing_matches(ui):
    with pytest.raises(WidgetNotFound, match="role='combo_box'"):
        ui.find(role="combo_box")
    assert ui.query(role="combo_box") is None
    assert ui.exists(role="combo_box") is False
    assert ui.exists(role="tree") is True


def test_widget_find_searches_only_its_own_subtree(ui):
    tabs = ui.find(role="tabs")
    assert tabs.find(role="data_table").id == "d:grid:0"
    with pytest.raises(WidgetNotFound):
        tabs.find(role="tree")


def test_rows_report_their_text_cells_and_expansion(ui):
    tree = ui.find(role="tree")
    assert tree.row_texts == ["alpha", "alpha-child", "beta"]
    assert tree.row("alpha").expanded is True
    assert tree.row("beta").expanded is False
    assert tree.row("beta").path == [1]
    assert tree.selected_row.text == "alpha"

    table = ui.find(role="data_table")
    assert [row.cells for row in table.rows] == [["a1.txt", "file"], ["sub", "folder"]]


def test_row_lookup_reports_what_is_actually_there(ui):
    tree = ui.find(role="tree")
    assert tree.row(index=2).text == "beta"
    assert tree.row(path=[0, 0]).text == "alpha-child"
    with pytest.raises(WidgetNotFound, match=r"alpha-child"):
        tree.row("nonexistent")


def test_a_hidden_tab_is_invisible_to_a_query_until_asked_for():
    demo = Demo()
    demo.open_tabs = ["one", "two"]
    ui = harness(demo.app)

    assert len(ui.find_all(role="data_table")) == 1
    assert len(ui.find_all(role="data_table", include_hidden=True)) == 2
    assert ui.find(role="tabs").active_index == 0
    assert [tab.title for tab in ui.find(role="tabs").tabs] == ["one", "two"]


# --- acting ------------------------------------------------------------------


def test_row_actions_dispatch_the_event_each_gesture_delivers(demo, ui):
    tree = ui.find(role="tree")
    tree.row("beta").click()
    tree.row("beta").select()
    tree.row("beta").double_click()
    tree.row("alpha").toggle()

    assert demo.log == [
        ("click", (1,)),
        ("select", (1,)),
        ("activate", (1,)),
        ("toggle", (0,)),
    ]


def test_table_row_actions_separate_select_from_activate(demo, ui):
    table = ui.find(role="data_table")
    table.row("sub").click()
    table.row("sub").double_click()
    table.row("a1.txt").activate()

    assert demo.log == [("row-select", 1), ("row-activate", 1), ("row-activate", 0)]


def test_tab_click_and_close_reach_the_tab_strip_callbacks(demo, ui):
    tabs = ui.find(role="tabs")
    tabs.tabs[0].click()
    tabs.tabs[0].close()

    assert demo.log == [("tab", 0), ("close", 0)]


def test_press_and_type_deliver_plain_top_level_keys(demo, ui):
    ui.press("Down")
    ui.press("F2")
    ui.type("hi")

    assert demo.keys == ["Down", "F2", "h", "i"]


def test_press_rejects_a_name_that_is_not_a_key(ui):
    with pytest.raises(ValueError, match="not a key name"):
        ui.press("Dowm")


def test_widget_press_delivers_the_focus_scoped_shape(demo, ui):
    """A focused widget's keys arrive scoped to it on a real backend; the
    harness sends that shape, and the app's own `key_name` still reads it."""
    ui.find(role="tree").press("Down")

    assert demo.keys == ["Down"]


def test_a_button_is_clicked_through_the_engines_own_hit_test():
    clicks = []

    def view():
        return Column(
            Button("Save", id="t:save", on_click=lambda: clicks.append("save")),
            TextDisplay([_line("idle")], id="t:out"),
        )

    ui = harness(App(30, 6, view=view))
    ui.find(role="button").click()

    assert clicks == ["save"]


def test_click_text_reports_where_a_label_is_not_on_screen():
    ui = harness(App(30, 6, view=lambda: TextDisplay([_line("hello")], id="t:out")))

    assert ui.locate_all("hello") == [(2.5, 0.0)]
    with pytest.raises(WidgetNotFound, match="appears 0 time"):
        ui.click_text("goodbye")


def test_focus_moves_focus_and_a_split_divider_drag_persists(ui):
    split = ui.find(id="d:split")
    assert split["ratio"] == pytest.approx(0.4)

    split.drag_divider(0.75)

    assert ui.find(id="d:split").ratio == pytest.approx(0.75)
    # The app keeps declaring its own literal; the store holds the drag.
    assert ui.find(id="d:split")["ratio"] == pytest.approx(0.4)


def test_actions_on_the_wrong_role_say_so(ui):
    with pytest.raises(TypeError, match="has no rows"):
        ui.find(role="tabs").rows
    with pytest.raises(TypeError, match="has no divider"):
        ui.find(role="tree").drag_divider(0.5)
    with pytest.raises(TypeError, match="no 'toggle' action"):
        ui.find(role="data_table").row(index=0).toggle()
    with pytest.raises(TypeError, match="not clicked as a whole"):
        ui.find(role="tree").click()


# --- asserting ----------------------------------------------------------------


def test_assert_text_prints_the_frame_when_it_fails(ui):
    ui.assert_text("alpha", "beta", "a1.txt")
    ui.assert_not_text("gamma")

    with pytest.raises(AssertionError) as missing:
        ui.assert_text("gamma")
    assert "gamma" in str(missing.value)
    assert "alpha" in str(missing.value), "the failure message must include the frame"

    with pytest.raises(AssertionError, match="unexpectedly on screen"):
        ui.assert_not_text("alpha")


def test_screen_and_contains_read_the_rendered_frame(ui):
    assert ui.contains("alpha")
    assert not ui.contains("gamma")
    assert "alpha" in ui.screen()
    assert any("alpha" in line for line in ui.screen_lines())


def test_a_toolbar_reports_its_buttons():
    def view():
        return SidebarPanel(
            TextDisplay([_line("body")], id="t:body"),
            id="t:chrome",
            toolbar=Toolbar([{"Action": {"id": "t:open", "label": "Open"}}, "Separator"], id="t:bar"),
        )

    ui = harness(App(30, 6, view=view))
    toolbar = ui.find(role="toolbar")

    assert toolbar.button_labels == ["Open"]
    assert toolbar.button("Open").id == "t:open"
    with pytest.raises(WidgetNotFound, match="no toolbar button"):
        toolbar.button("Close")


# --- wrapping a plain QuiApp ---------------------------------------------------


def test_a_plain_qui_app_is_driven_through_the_same_api():
    """The harness is not tied to `quadraui.app`: a raw `QuiApp` works too,
    given the same render callable the app was built with."""
    state = {"count": 0}

    def render():
        return {"id": "q:out", "text_display": {"id": "q:out", "lines": [_line(f"count {state['count']}")]}}

    app = QuiApp(30, 4, render=render)

    def handle(event):
        if key_name(event) == "Up":
            state["count"] += 1

    app.set_handler(handle)
    ui = harness(app, render=render)

    assert ui.find(role="text_display").text == "count 0"
    ui.press("Up")
    assert ui.find(role="text_display").text == "count 1"
    ui.assert_text("count 1")


def test_wrapping_a_plain_qui_app_without_its_render_is_refused():
    with pytest.raises(TypeError, match="render="):
        Harness(QuiApp(10, 3))
