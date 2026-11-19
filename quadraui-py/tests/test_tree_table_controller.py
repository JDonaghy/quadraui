"""Black-box coverage for the `TreeTableController` per-widget-id seam
exposed through `quadraui-py`: a plain Python dict decodes into a
`"tree_table_controller"` extra (`EXTRA_PRIMITIVE_KINDS`' generic
registration already covers the descriptor shape -- no new pymethod needed
for that half), and the full lazy-loaded hierarchy round trip -- expand a
lazy branch -> `ChildrenRequested` -> `QuiApp.set_tree_table_children` ->
header-click sort (every loaded level re-ordered, hierarchy preserved) ->
selection survives the sort by node id, not position -- reaches the
registered handler as `{"TreeTableController": (id, event)}`, the same
`{"TreeTable": (table_id, event)}` tuple shape a flat-slot `TreeTable`'s own
`TreeTableEvent` already reaches it as (see `test_tree_table_interactions.py`).

Mirrors `quadraui-binding-core::engine`'s own
`tree_table_controller_expand_fetch_sort_and_selection_round_trip` Rust
test, driven end-to-end through this binding's Python surface instead, at
three hierarchy levels (root -> directory -> file) rather than two, so both
`QuiApp.set_tree_table_children` calls answer a genuinely nested lazy load.

The round-trip test above calls `QuiApp.set_tree_table_children`/
`set_tree_table_sort_override` from top-level script code, after `_press`
returns -- never from inside the registered handler, since both raise
`ReentrantCallError` there (the handler already holds this app's one
mutable borrow for the whole dispatch). `ChildrenRequested`/`SortChanged`
are in fact only ever delivered to that same handler, so
`test_children_requested_answered_from_inside_the_handler` below answers
one for real: via `QuiAppActions.set_tree_table_children`/
`set_tree_table_sort_override` (obtained once from `QuiApp.actions()`),
which queue the write instead of applying it immediately, applied right
after that same dispatch finishes.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_tree_table_controller.py
"""

from quadraui import QuiApp

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "meta": False}
_TAB = {"KeyPressed": {"key": {"Named": "Tab"}, "modifiers": _NO_MODS, "repeat": False}}
_DOWN = {"KeyPressed": {"key": {"Named": "Down"}, "modifiers": _NO_MODS, "repeat": False}}
_SPACE = {"KeyPressed": {"key": {"Char": " "}, "modifiers": _NO_MODS, "repeat": False}}
_ENTER = {"KeyPressed": {"key": {"Named": "Enter"}, "modifiers": _NO_MODS, "repeat": False}}


def _cell(text):
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def _node(node_id, label, *, has_children=False):
    node = {"id": node_id, "cells": [_cell(label)]}
    if has_children:
        node["has_children"] = True
    return node


def _descriptor():
    """A single lazy-branch root (`TreeTableNodeData::lazy_branch`'s own
    "has_children, no loaded children yet" shape). The engine seeds a
    fresh `TreeTableController` from this exactly once -- the first frame
    `"tt"` is painted -- then leaves the controller's own live expand/
    selection/sort state alone on every later frame, so returning this
    same dict unchanged from every `render` call below is enough; it is
    never re-seeded."""
    return {
        "id": "tt",
        "columns": [{"title": "Name", "width": {"Flex": 1.0}}],
        "nodes": [_node("root", "root", has_children=True)],
        "selected_id": "root",
        "has_focus": True,
        "show_scrollbar": True,
        "bordered": False,
        "folders_first": False,
    }


def _press(app, event):
    app.push_event(event)
    app.run(1)


def _locate(screen, label):
    """`(row, column)` of `label`'s first occurrence in a rendered
    `screen()` string, or `(None, None)` if absent. `QuiApp` (unlike
    `quadraui.testing`'s `Harness`) exposes no structural `find`, so a
    row-order/indentation check on this binding's bare surface reads the
    rendered text directly -- a plain, row-major grid, one line per
    screen row."""
    for row, line in enumerate(screen.splitlines()):
        col = line.find(label)
        if col != -1:
            return row, col
    return None, None


def _tree_table_controller_events(received, id_, event):
    return [
        entry["TreeTableController"]
        for entry in received
        if "TreeTableController" in entry and entry["TreeTableController"] == (id_, event)
    ]


def _render():
    # A single-leaf `"row"` layout tree, not the bare flat `{"tree_table_controller":
    # ...}` top-level key: the flat six-slot-plus-extras system gives every
    # additive primitive a fixed 3-line band (`draw_bands`'s own `band(3.0)`
    # calls), which is the right size for a `Switch`/`ProgressBar` but far
    # too short for a tree with more than one visible row. A layout-tree
    # leaf fills the whole area its container gives it instead -- the same
    # shape `tree_table_controller_expand_fetch_sort_and_selection_round_trip`
    # (`quadraui-binding-core::engine`) drives its own `TreeTableController`
    # through.
    return {"row": [{"id": "tt", "tree_table_controller": _descriptor()}]}


def test_three_level_lazy_tree_expand_sort_and_selection_round_trip():
    app = QuiApp(40, 12, render=_render)
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))

    # Tab lands keyboard focus on the sole tab stop -- nothing is focused
    # on a freshly built `QuiApp`, so the `KeyPressed` routing below (which
    # requires engine-level focus, not just the descriptor's own
    # `has_focus` cosmetic flag) has nothing to dispatch through yet.
    _press(app, _TAB)

    # Space on the selected lazy-branch root expands it -- no loaded
    # children yet, so this requests them exactly once.
    _press(app, _SPACE)
    assert (
        len(_tree_table_controller_events(received, "tt", {"ChildrenRequested": {"id": "root"}}))
        == 1
    ), received

    # Answer the request: two children, one itself a lazy branch (the
    # tree's second level), in an intentionally unsorted order.
    app.set_tree_table_children(
        "tt",
        "root",
        [
            _node("zebra_dir", "zebra_dir", has_children=True),
            _node("apple_file", "apple_file"),
        ],
    )
    app.screen()
    assert app.screen_contains("zebra_dir")
    assert app.screen_contains("apple_file")

    # Down from "root" selects its first (still unsorted) child,
    # "zebra_dir" -- picked deliberately so "selection survives the sort"
    # below is a genuine id-keyed survival, not a lucky positional
    # coincidence.
    received.clear()
    _press(app, _DOWN)
    assert _tree_table_controller_events(received, "tt", {"RowSelected": {"id": "zebra_dir"}}), (
        received
    )

    # Space on "zebra_dir" expands the tree's third level -- another
    # not-yet-loaded lazy branch, so this requests its own children too.
    received.clear()
    _press(app, _SPACE)
    assert (
        len(
            _tree_table_controller_events(
                received, "tt", {"ChildrenRequested": {"id": "zebra_dir"}}
            )
        )
        == 1
    ), received

    app.set_tree_table_children(
        "tt",
        "zebra_dir",
        [
            _node("z_leaf", "z_leaf"),
            _node("a_leaf", "a_leaf"),
        ],
    )
    app.screen()
    assert app.screen_contains("z_leaf")
    assert app.screen_contains("a_leaf")

    # Down from "zebra_dir" selects its first (still unsorted) child,
    # "z_leaf".
    received.clear()
    _press(app, _DOWN)
    assert _tree_table_controller_events(received, "tt", {"RowSelected": {"id": "z_leaf"}}), (
        received
    )

    # A header click sorts every loaded level's children ascending --
    # both "root"'s two children and "zebra_dir"'s two children -- without
    # flattening the hierarchy: "apple_file" moves ahead of "zebra_dir" at
    # the root level, and "a_leaf" ahead of "z_leaf" under "zebra_dir",
    # each staying nested under its own parent.
    #
    # Not `app.find("Name")`: the generic per-runner TUI focus ring
    # (`crate::tui::draw_focus_ring`, drawn for any focused tab-stop-
    # resolvable widget) overwrites the whole top row of a focused extra's
    # rect with border glyphs, so the header's own "Name" text is no
    # longer on screen once "tt" is focused -- but the header row is
    # still row 0 of this leaf's rect, which fills the whole viewport, so
    # clicking (x=5, y=0) still lands on column 0's header exactly as
    # `TreeTableHit::Header` resolves it.
    received.clear()
    app.push_event(
        {
            "MouseDown": {
                "button": "Left",
                "position": {"x": 5.0, "y": 0.0},
                "modifiers": {},
            }
        }
    )
    app.run(1)
    assert _tree_table_controller_events(
        received, "tt", {"SortChanged": {"col": 0, "ascending": True}}
    ), received

    screen = app.screen()
    apple_row, _ = _locate(screen, "apple_file")
    zebra_row, zebra_indent = _locate(screen, "zebra_dir")
    a_leaf_row, a_leaf_indent = _locate(screen, "a_leaf")
    z_leaf_row, z_leaf_indent = _locate(screen, "z_leaf")
    assert None not in (apple_row, zebra_row, a_leaf_row, z_leaf_row), screen
    # "apple_file" now renders above "zebra_dir" at the root level.
    assert apple_row < zebra_row, screen
    # "a_leaf"/"z_leaf" are still nested under "zebra_dir" -- indented
    # further right than it, and painted below it -- with "a_leaf" now
    # first among them.
    assert a_leaf_indent > zebra_indent, screen
    assert z_leaf_indent > zebra_indent, screen
    assert zebra_row < a_leaf_row < z_leaf_row, screen

    # Selection survives the sort by node id, not position: "z_leaf" is
    # still selected even though it moved from index 0 to index 1 among
    # "zebra_dir"'s children -- Enter activates it, not "a_leaf".
    received.clear()
    _press(app, _ENTER)
    assert _tree_table_controller_events(received, "tt", {"RowActivated": {"id": "z_leaf"}}), (
        received
    )


def test_children_requested_answered_from_inside_the_handler():
    """`ChildrenRequested` is only ever actually delivered to the
    registered handler -- unlike the round trip above, which answers it
    with `QuiApp.set_tree_table_children` from top-level script code,
    after `_press` returns. A direct `app.set_tree_table_children(...)`
    call from inside the handler itself raises `ReentrantCallError`
    instead (the handler already holds this app's one mutable borrow for
    the whole dispatch) -- `test_reentrant_call_guard.py` covers that wall
    for the whole restricted-method family. This answers the request the
    way a real `run_tui`/`run_native` app must: via
    `QuiAppActions.set_tree_table_children`, obtained once up front from
    `QuiApp.actions()`, and applied by `flush_deferred_actions` right
    after this same dispatch finishes -- so the newly loaded children are
    already in the cached controller by the time `_press` returns."""
    app = QuiApp(40, 12, render=_render)
    app.screen()
    actions = app.actions()

    def on_event(event):
        if "TreeTableController" not in event:
            return
        table_id, payload = event["TreeTableController"]
        if "ChildrenRequested" in payload:
            actions.set_tree_table_children(
                table_id,
                payload["ChildrenRequested"]["id"],
                [
                    _node("zebra_dir", "zebra_dir", has_children=True),
                    _node("apple_file", "apple_file"),
                ],
            )

    app.set_handler(on_event)

    # Tab focuses the sole tab stop; Space on the selected lazy-branch
    # root expands it, requesting its children -- answered above, from
    # inside `on_event` itself, not after `_press` returns.
    _press(app, _TAB)
    _press(app, _SPACE)

    assert app.screen_contains("zebra_dir")
    assert app.screen_contains("apple_file")


def test_set_tree_table_sort_override_applies_a_host_chosen_order():
    """The positive path `test_set_tree_table_children_is_a_no_op_for_an_unpainted_id`
    below doesn't exercise: a host order that disagrees with the engine's
    own default ascending comparator -- "zebra_dir" before "apple_file",
    the reverse of alphabetical -- is what actually renders, proving the
    override wins rather than the header click's own default sort
    silently winning instead."""
    app = QuiApp(40, 12, render=_render)
    app.screen()

    app.set_handler(lambda _event: None)
    _press(app, _TAB)
    _press(app, _SPACE)

    app.set_tree_table_children(
        "tt",
        "root",
        [
            _node("apple_file", "apple_file"),
            _node("zebra_dir", "zebra_dir", has_children=True),
        ],
    )

    # A header click applies the engine's own default ascending sort --
    # already "apple_file" first here, so this is a no-op on the visible
    # order, exercised only so the override below has a real `SortChanged`
    # dispatch (and its default application) to override.
    app.push_event(
        {
            "MouseDown": {
                "button": "Left",
                "position": {"x": 5.0, "y": 0.0},
                "modifiers": {},
            }
        }
    )
    app.run(1)

    app.set_tree_table_sort_override(
        "tt",
        [
            _node("zebra_dir", "zebra_dir", has_children=True),
            _node("apple_file", "apple_file"),
        ],
    )

    screen = app.screen()
    apple_row, _ = _locate(screen, "apple_file")
    zebra_row, _ = _locate(screen, "zebra_dir")
    assert None not in (apple_row, zebra_row), screen
    assert zebra_row < apple_row, screen


def test_set_tree_table_children_is_a_no_op_for_an_unpainted_id():
    """`QuiApp.set_tree_table_children`/`set_tree_table_sort_override`
    silently no-op for an id with no cached controller -- it hasn't been
    painted this frame, or at all -- mirroring
    `InteractionEngine::set_tree_table_children`'s own doc. Neither raises,
    and a later real call for the real id still behaves normally."""
    app = QuiApp(40, 12, render=_render)
    app.screen()

    app.set_tree_table_children("no-such-id", "root", [_node("x", "x")])
    app.set_tree_table_sort_override("no-such-id", [_node("x", "x")])

    received = []
    app.set_handler(lambda event: received.append(event))
    _press(app, _TAB)
    _press(app, _SPACE)
    assert _tree_table_controller_events(received, "tt", {"ChildrenRequested": {"id": "root"}})
