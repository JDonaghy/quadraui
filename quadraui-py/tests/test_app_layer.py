"""Unit tests for `quadraui.app`: id auto-assignment stability across
re-render, callback dispatch (`Tree`, `Table`, `Tabs`, `Button`, `Form`
and its field builders, and the standalone leaves that reuse `Form`'s
own event shape), component state survival, and redraw-on-mutation --
the four properties the app layer's own module doc promises and
`examples/file_manager_app.py` relies on wholesale.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_app_layer.py
"""

import pytest

from quadraui import UnknownKeyError, VersionMismatchError
from quadraui.app import (
    App,
    Button,
    Checkbox,
    Column,
    Component,
    Form,
    List,
    Password,
    PasswordField,
    Select,
    SelectField,
    SidebarPanel,
    Slider,
    SliderField,
    Split,
    StatusBar,
    Table,
    Tabs,
    Tab,
    TextArea,
    TextAreaField,
    TextDisplay,
    TextInputField,
    ToggleField,
    Toolbar,
    Tree,
    key_name,
)
from quadraui.app import runtime as _runtime


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


def test_id_bearing_node_constructors_all_accept_an_omitted_id():
    """Every constructor the reviewer flagged as still requiring `id=`
    (`Split`, `SidebarPanel`, `Tabs`, `Tree`, `Table`, `StatusBar`,
    `TextDisplay`) must build without one -- the issue's own worked
    example (`Tree(rows, on_activate=...)`) previously raised `TypeError`
    for exactly this reason. `Toolbar` is checked separately below: it
    returns a raw dict rather than a `Node`, so it never reaches `_walk`'s
    own auto-id."""

    def view():
        tree = Tree([], on_activate=lambda path: None)
        table = Table([], [])
        lst = List([], on_select=lambda idx: None)
        status = StatusBar([], [])
        text = TextDisplay([])
        tabs = Tabs([Tab(text, title="one")])
        panel = SidebarPanel(Column(tree, table, lst, status, tabs))
        return Split(panel, text)

    app = App(20, 5, view=view)
    assert app._render() is not None


def test_auto_ids_from_node_constructors_are_stable_across_rerender():
    """The same stability guarantee `test_auto_assigned_ids_are_stable_across_rerender_at_the_same_position`
    proves for `Button` holds for every other id-bearing constructor too,
    since they all resolve through the same `_walk`-driven auto-id."""

    def view():
        return Split(
            Tree([], on_row_click=lambda path: None),
            Table([], [], on_select=lambda idx: None),
        )

    app = App(20, 5, view=view)
    ids_before = sorted(app._registry.keys())
    app.step()
    ids_after = sorted(app._registry.keys())
    assert ids_before == ids_after
    assert len(ids_before) == 2


def test_toolbar_without_an_explicit_id_still_builds_a_wire_dict():
    """`Toolbar` falls back to its own call-order counter (it never passes
    through `Node`/`_walk`), but omitting `id=` must still produce a
    well-formed `ToolbarDescriptor` rather than raising."""
    toolbar = Toolbar(buttons=[{"Action": {"id": "a", "label": "Go"}}])
    assert isinstance(toolbar["id"], str)
    assert toolbar["buttons"] == [{"Action": {"id": "a", "label": "Go"}}]

    other = Toolbar(buttons=[])
    assert other["id"] != toolbar["id"]


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


def test_list_selection_changed_dispatches_on_select_not_on_activate():
    selected = []
    activated = []

    def view():
        return List(
            items=[{"text": {"spans": [{"text": "a", "fg": None, "bg": None}]}}],
            id="t:list",
            on_select=lambda idx: selected.append(idx),
            on_activate=lambda idx: activated.append(idx),
        )

    app = App(20, 5, view=view)
    app.push_event({"List": ["t:list", {"SelectionChanged": {"idx": 0}}]})
    app.step()
    assert selected == [0]
    assert activated == []


def test_list_item_double_clicked_and_item_activated_both_dispatch_on_activate():
    """`on_activate` fires on either the mouse `ItemDoubleClicked` or the
    keyboard `ItemActivated` -- the same "open this row" pairing `Table`'s
    own `on_activate` documents."""
    activated = []

    def view():
        return List(
            items=[{"text": {"spans": [{"text": "a", "fg": None, "bg": None}]}}],
            id="t:list",
            on_activate=lambda idx: activated.append(idx),
        )

    app = App(20, 5, view=view)
    app.push_event({"List": ["t:list", {"ItemDoubleClicked": {"idx": 0}}]})
    app.step()
    assert activated == [0]

    app.push_event({"List": ["t:list", {"ItemActivated": {"idx": 0}}]})
    app.step()
    assert activated == [0, 0]


def test_list_without_selected_defaults_selected_idx_to_zero():
    def view():
        return List(items=[], id="t:list")

    app = App(20, 5, view=view)
    wire = app._render()
    assert wire["list"]["selected_idx"] == 0


def test_list_selected_title_and_bordered_reach_the_payload_when_passed():
    def view():
        return List(
            items=[],
            id="t:list",
            selected=2,
            title={"spans": [{"text": "Files", "fg": None, "bg": None}]},
            bordered=True,
        )

    app = App(20, 5, view=view)
    payload = app._render()["list"]
    assert payload["selected_idx"] == 2
    assert payload["title"] == {"spans": [{"text": "Files", "fg": None, "bg": None}]}
    assert payload["bordered"] is True


def test_list_title_and_bordered_are_omitted_when_not_passed():
    def view():
        return List(items=[], id="t:list")

    app = App(20, 5, view=view)
    payload = app._render()["list"]
    assert "title" not in payload
    assert "bordered" not in payload


def test_list_key_pressed_falls_through_to_on_event():
    """A focus-scoped `{"List": [id, {"KeyPressed": ...}]}` is not one of
    `List`'s own callback variants (`SelectionChanged`/`ItemDoubleClicked`/
    `ItemActivated`), so it must still reach the global `on_event` escape
    hatch rather than being silently swallowed."""
    fallen_through = []

    def view():
        return List(items=[], id="t:list", on_select=lambda idx: None)

    app = App(20, 5, view=view, on_event=lambda event: fallen_through.append(event))
    # `ListViewEvent::KeyPressed` carries a plain string key, unlike the
    # top-level `{"KeyPressed": {"key": {...}, ...}}` shape -- see
    # `quadraui.wire.ListViewEvent`'s own `_KeyPressedStrPayload`.
    app.push_event({"List": ["t:list", {"KeyPressed": {"key": "j", "modifiers": _NO_MODS}}]})
    app.step()
    assert len(fallen_through) == 1
    widget_id, activation = fallen_through[0]["List"]
    assert widget_id == "t:list"
    assert activation == {"KeyPressed": {"key": "j", "modifiers": _NO_MODS}}


def test_list_selection_changed_with_no_on_select_falls_through_to_on_event():
    fallen_through = []

    def view():
        return List(items=[], id="t:list")

    app = App(20, 5, view=view, on_event=lambda event: fallen_through.append(event))
    app.push_event({"List": ["t:list", {"SelectionChanged": {"idx": 0}}]})
    app.step()
    assert len(fallen_through) == 1
    widget_id, activation = fallen_through[0]["List"]
    assert widget_id == "t:list"
    assert activation == {"SelectionChanged": {"idx": 0}}


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


def test_button_on_click_fires_on_enter_after_a_click_moved_real_focus_to_it():
    # A click leaves the *backend's* own focus on the button, not just this
    # layer's `_focused_id`, so the Enter that follows resolves to the
    # button's semantic `Form`/`ButtonClicked` activation instead of a raw
    # `KeyPressed`. Both routes must reach the same `on_click`.
    clicks = []
    fallback = []

    def view():
        return Button("Save", id="t:save", on_click=lambda: clicks.append(1))

    app = App(20, 5, view=view, on_event=lambda event: fallback.append(event))
    _click_at(app)
    assert clicks == [1], "the click itself must fire on_click exactly once"
    assert not any("Form" in event for event in fallback), (
        "a button's own activation must never reach the on_event escape hatch"
    )

    app.push_event(_key_event({"Named": "Enter"}))
    app.step()
    assert clicks == [1, 1], "Enter on the clicked (and so really focused) button must fire too"


def test_unmatched_event_falls_through_to_on_event():
    seen = []

    def view():
        return TextDisplay([], id="t:text")

    app = App(20, 5, view=view, on_event=lambda event: seen.append(event))
    app.push_event(_key_event({"Named": "Down"}))
    app.step()
    assert len(seen) == 1
    assert key_name(seen[0]) == "Down"


# --- Form / typed field builders -------------------------------------------


def _form_event(field_id, variant, payload):
    """A `{"Form": [widget_id, {variant: payload}]}` event -- the same
    shape a real `Form`-embedded field or a standalone leaf reusing it
    produces (see `_FORM_FIELD_EVENT_MAP`'s own doc on why dispatch keys
    off `payload["id"]`, not the wrapper's `widget_id`); the wrapper id
    below is deliberately *not* `field_id`, to prove dispatch really does
    key off the payload."""
    return {"Form": ["t:wrapper", {variant: {"id": field_id, **payload}}]}


def test_toggle_field_on_change_fires_with_the_new_value():
    seen = []

    def view():
        return Form([ToggleField(True, id="f:wrap", on_change=lambda v: seen.append(v))], id="t:form")

    app = App(20, 5, view=view)
    app.push_event(_form_event("f:wrap", "ToggleChanged", {"value": False}))
    app.step()
    assert seen == [False]


def test_checkbox_on_change_fires_with_the_new_value():
    seen = []

    def view():
        return Checkbox(True, id="t:cb", on_change=lambda v: seen.append(v))

    app = App(20, 5, view=view)
    app.push_event(_form_event("t:cb", "ToggleChanged", {"value": False}))
    app.step()
    assert seen == [False]


def test_checkbox_on_change_fires_on_a_real_hit_tested_click():
    """End-to-end, through the real engine's own hit-test and semantic
    dispatch -- not a hand-built `Form` event -- the same way
    `test_button_on_click_fires_on_a_hit_tested_mouse_down` proves
    `Button`'s own wiring."""
    seen = []

    def view():
        return Checkbox(True, id="t:cb", on_change=lambda v: seen.append(v))

    app = App(20, 5, view=view)
    _click_at(app)
    assert seen == [False]


def test_text_input_field_on_change_and_on_submit_fire_separately():
    changed = []
    submitted = []

    def view():
        return Form(
            [
                TextInputField(
                    "hi",
                    id="f:name",
                    on_change=lambda v: changed.append(v),
                    on_submit=lambda v: submitted.append(v),
                )
            ],
            id="t:form",
        )

    app = App(20, 5, view=view)
    app.push_event(_form_event("f:name", "TextInputChanged", {"value": "hi!"}))
    app.step()
    assert changed == ["hi!"]
    assert submitted == []

    app.push_event(_form_event("f:name", "TextInputCommitted", {"value": "hi!"}))
    app.step()
    assert submitted == ["hi!"]
    assert changed == ["hi!"], "a commit must not also re-fire on_change"


def test_slider_field_on_change_fires_with_the_new_value():
    # Hand-feeds the `SliderChanged` payload to pin the dispatch wiring; it
    # does not prove the engine emits one for a real drag/arrow key on a
    # `Form`-embedded slider -- see `SliderField`'s own doc for that gap.
    seen = []

    def view():
        return Form([SliderField(5.0, 0.0, 10.0, id="f:vol", on_change=lambda v: seen.append(v))], id="t:form")

    app = App(20, 5, view=view)
    app.push_event(_form_event("f:vol", "SliderChanged", {"value": 7.5}))
    app.step()
    assert seen == [7.5]


def test_slider_leaf_on_change_fires_with_the_new_value():
    seen = []

    def view():
        return Slider(5.0, 0.0, 10.0, id="t:vol", on_change=lambda v: seen.append(v))

    app = App(20, 5, view=view)
    app.push_event(_form_event("t:vol", "SliderChanged", {"value": 2.5}))
    app.step()
    assert seen == [2.5]


def test_select_field_on_change_fires_with_the_new_index():
    seen = []

    def view():
        return Form(
            [SelectField(["a", "b", "c"], 0, id="f:theme", on_change=lambda idx: seen.append(idx))], id="t:form"
        )

    app = App(20, 5, view=view)
    app.push_event(_form_event("f:theme", "DropdownChanged", {"selected_idx": 2}))
    app.step()
    assert seen == [2]


def test_select_leaf_on_change_fires_with_the_new_index():
    seen = []

    def view():
        return Select(["a", "b", "c"], id="t:theme", on_change=lambda idx: seen.append(idx))

    app = App(20, 5, view=view)
    app.push_event(_form_event("t:theme", "DropdownChanged", {"selected_idx": 1}))
    app.step()
    assert seen == [1]


def test_password_field_on_change_and_on_submit_fire_separately():
    changed = []
    submitted = []

    def view():
        return Form(
            [
                PasswordField(
                    "secret",
                    id="f:pw",
                    on_change=lambda v: changed.append(v),
                    on_submit=lambda v: submitted.append(v),
                )
            ],
            id="t:form",
        )

    app = App(20, 5, view=view)
    app.push_event(_form_event("f:pw", "TextInputChanged", {"value": "secret!"}))
    app.step()
    assert changed == ["secret!"]

    app.push_event(_form_event("f:pw", "TextInputCommitted", {"value": "secret!"}))
    app.step()
    assert submitted == ["secret!"]


def test_password_leaf_on_change_fires_with_the_new_value():
    seen = []

    def view():
        return Password("secret", id="t:pw", on_change=lambda v: seen.append(v))

    app = App(20, 5, view=view)
    app.push_event(_form_event("t:pw", "TextInputChanged", {"value": "secret!"}))
    app.step()
    assert seen == ["secret!"]


def test_text_area_field_on_change_fires_with_the_new_value():
    seen = []

    def view():
        return Form([TextAreaField("line1", id="f:notes", on_change=lambda v: seen.append(v))], id="t:form")

    app = App(20, 5, view=view)
    app.push_event(_form_event("f:notes", "TextInputChanged", {"value": "line1\nline2"}))
    app.step()
    assert seen == ["line1\nline2"]


def test_text_area_leaf_on_change_fires_with_the_new_value():
    seen = []

    def view():
        return TextArea("line1", id="t:notes", on_change=lambda v: seen.append(v))

    app = App(20, 5, view=view)
    app.push_event(_form_event("t:notes", "TextInputChanged", {"value": "line1\nline2"}))
    app.step()
    assert seen == ["line1\nline2"]


def test_form_field_callbacks_are_registered_under_the_fields_own_id_not_forms():
    """`FormEvent`'s own `id` is the field's, never the wrapping `Form`'s
    (see `quadraui::primitives::form::FormEvent`'s own doc) -- so a field's
    `on_change` must be looked up by its own id, and a payload naming the
    `Form`'s id instead must not accidentally match it."""
    seen = []

    def view():
        return Form([ToggleField(True, id="f:toggle", on_change=lambda v: seen.append(v))], id="t:form")

    app = App(20, 5, view=view)
    app.push_event(_form_event("t:form", "ToggleChanged", {"value": False}))
    app.step()
    assert seen == [], "the Form's own id must not be mistaken for the field's"


def test_form_fields_resolve_auto_ids_from_their_position_under_fields():
    def view():
        return Form(
            [ToggleField(True, on_change=lambda v: None), ToggleField(False, on_change=lambda v: None)],
            id="t:form",
        )

    app = App(20, 5, view=view)
    rendered_ids = [field["id"] for field in app._render()["form"]["fields"]]
    assert len(rendered_ids) == 2
    assert rendered_ids[0] != rendered_ids[1]
    assert set(rendered_ids) <= set(app._registry.keys())


def test_form_builds_a_well_formed_form_descriptor_with_every_field_kind():
    form_node = Form(
        [
            ToggleField(True, id="f:toggle", label="Word wrap"),
            TextInputField("hi", id="f:text", label="Name", placeholder="type a name"),
            SliderField(5.0, 0.0, 10.0, id="f:slider", step=0.5, label="Vol"),
            SelectField(["a", "b"], 1, id="f:select", label="Theme"),
            PasswordField("pw", id="f:pw", label="Password", mask_char="*"),
            TextAreaField("line1", id="f:area", label="Notes", visible_rows=4),
        ],
        id="t:form",
        has_focus=True,
    )
    app = App(80, 20, view=lambda: form_node)
    descriptor = app._render()["form"]
    assert descriptor["id"] == "t:form"
    assert descriptor["has_focus"] is True
    fields_by_id = {field["id"]: field for field in descriptor["fields"]}
    assert fields_by_id["f:toggle"]["kind"] == {"Toggle": {"value": True}}
    assert fields_by_id["f:text"]["kind"] == {"TextInput": {"value": "hi", "placeholder": "type a name"}}
    assert fields_by_id["f:slider"]["kind"] == {"Slider": {"value": 5.0, "min": 0.0, "max": 10.0, "step": 0.5}}
    assert fields_by_id["f:select"]["kind"]["Dropdown"]["selected_idx"] == 1
    assert fields_by_id["f:pw"]["kind"] == {"PasswordInput": {"value": "pw", "placeholder": "", "mask_char": "*"}}
    assert fields_by_id["f:area"]["kind"] == {"TextArea": {"value": "line1", "placeholder": "", "visible_rows": 4}}
    assert fields_by_id["f:toggle"]["label"] == {"spans": [{"text": "Word wrap", "fg": None, "bg": None}]}


def test_form_field_event_with_no_matching_callback_falls_through_to_on_event():
    fallback = []

    def view():
        return Form([ToggleField(True, id="f:toggle")], id="t:form")

    app = App(20, 5, view=view, on_event=lambda event: fallback.append(event))
    app.push_event(_form_event("f:toggle", "ToggleChanged", {"value": False}))
    app.step()
    assert len(fallback) == 1
    assert "Form" in fallback[0]


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
        assert expected in app.screen()


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


def test_closing_and_reopening_a_row_does_not_resurrect_the_old_state():
    """Two `Counter`s at positions 0 and 1; bump position 1's count, then
    render with only position 0 present (closing a tab/removing a list
    row) -- position 1's state slot must be pruned, not left behind. A
    fresh `Counter` later rendered back at position 1 must start at
    `count=0`, not inherit the pruned slot's stale count (the resurrection
    bug a position-keyed, never-pruned `_component_states` dict would
    otherwise cause for the common "one component per open tab/list row"
    shape `component.py`'s own `key=` doc anticipates)."""
    counters = [Counter(), Counter()]

    def view():
        return Column(*counters)

    app = App(20, 5, view=view)
    ids = sorted(app._registry.keys())
    assert len(ids) == 2

    for _ in range(2):
        app._registry[ids[1]]["on_click"]()
    assert "count=2" in app.screen()
    assert len(app._component_states) == 2

    # Render with only position 0 present -- position 1's slot must be
    # pruned, since nothing visited it this render. (`screen()` itself
    # triggers the render pass, same as every other assertion here.)
    counters.pop()
    app.screen()
    assert len(app._component_states) == 1

    # A brand-new `Counter` rendered back at position 1 starts fresh.
    counters.append(Counter())
    screen = app.screen()
    assert "count=2" not in screen
    assert screen.count("count=0") == 2


def test_app_strict_true_raises_on_a_typo_in_a_raw_descriptor():
    """`App(strict=True)` forwards `strict` straight to the underlying
    `QuiApp`, so a misspelt optional key on a raw-dict escape-hatch node
    (one `view()` returns unwrapped, bypassing every typed `Node`
    constructor's own fixed keyword set) raises instead of decoding
    silently -- the same policy `QuiApp(strict=True)` enforces on its own
    `render` callback."""

    def view():
        return {"button": {"id": "b", "label": "Save", "labell": "typo"}}

    app = App(20, 5, view=view, strict=True)
    with pytest.raises(UnknownKeyError, match="labell"):
        app.screen()


def test_app_set_strict_toggles_the_policy_after_construction():
    """`strict` can also be flipped at runtime through the underlying
    `QuiApp`, reachable as `app.qui.set_strict`."""

    def view():
        return {"button": {"id": "b", "label": "Save", "labell": "typo"}}

    app = App(20, 5, view=view)
    app.screen()  # no `strict` set yet -- the typo is silently ignored

    app.qui.set_strict(True)
    with pytest.raises(UnknownKeyError, match="labell"):
        app.screen()


# --- wire_version ---------------------------------------------------------


def test_rendered_tree_carries_the_current_wire_version():
    """Every tree this layer renders gets a `"wire_version"` sibling key
    added automatically (see `runtime._render_once`) -- a plain `Button`
    tree with no version opinion of its own still decodes and renders,
    the same way a hand-built wire dict carrying the current
    `runtime.WIRE_VERSION` already does for `QuiApp` directly."""

    def view():
        return Button("OK", on_click=lambda: None)

    app = App(20, 5, view=view)
    assert app.screen_contains("OK")
    assert app._render()["wire_version"] == _runtime.WIRE_VERSION


def test_a_raw_escape_hatch_dict_that_sets_its_own_wire_version_wins():
    """A `view()` that returns a raw wire dict (the escape hatch `Node`
    constructors bypass) and already set its own `"wire_version"` is left
    untouched -- this layer only fills the key in when it's missing."""

    def view():
        return {"wire_version": 1, "button": {"id": "b", "label": "OK"}}

    app = App(20, 5, view=view)
    assert app._render()["wire_version"] == 1


def test_forcing_a_wire_version_newer_than_this_host_understands_raises():
    """Bumping `runtime.WIRE_VERSION` past what the native extension's own
    `wire_schema::WIRE_VERSION` accepts makes the host's version gate
    reject the next render with `VersionMismatchError`, exactly as it
    would for a hand-built wire dict carrying the same inflated value."""

    def view():
        return Button("OK", on_click=lambda: None)

    app = App(20, 5, view=view)
    try:
        _runtime.WIRE_VERSION = 999
        with pytest.raises(VersionMismatchError, match="wire_version"):
            app.screen()
    finally:
        _runtime.WIRE_VERSION = 1
