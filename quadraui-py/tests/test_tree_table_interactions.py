"""Black-box coverage for `TreeTable`, wired through `quadraui-binding-core`
as an additive primitive (`"tree_table" => TreeTable` in `extra_paint.rs`'s
`extra_primitives!` table): a plain Python dict decodes into a real
`TreeTable` (a `DataTable` plus per-row tree metadata), renders its branch
row's chevron and both rows' labels, and its resolved layout/chevron bounds
are reachable through the same pure `TreeTable::layout`/`TreeTableLayout`
API the Rust side tests against -- no new pymethod needed, since
`EXTRA_PRIMITIVE_KINDS`'s generic registration already covers it.

A click on a branch row's chevron resolves to a typed
`TreeTableEvent::ToggleExpanded` (via the paint-time layout
`quadraui-binding-core` stores on the hit map -- see `hit.rs`'s
`PaintedHit::Extra::tree_table_hit`); a click anywhere else in the row body
has no more specific resolution and still reaches the handler as a raw
`MouseDown`, the same `ExtraBody` fallback every other additive primitive's
body click gets.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_tree_table_interactions.py
"""

from quadraui import QuiApp


def _cell(text):
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def _row(text):
    return {"cells": [_cell(text)]}


def _tree_table_dict():
    return {
        "table": {
            "id": "files",
            "columns": [{"title": "Name", "width": {"Fixed": 20.0}}],
            "rows": [_row("src"), _row("main.rs")],
            "has_focus": True,
        },
        "tree": [
            {"path": [0], "indent": 0, "is_expanded": True, "icon": None},
            {"path": [0, 0], "indent": 1, "is_expanded": None, "icon": None},
        ],
    }


def test_tree_table_renders_branch_chevron_and_both_labels():
    app = QuiApp(40, 10)
    app.set_render(lambda: {"tree_table": _tree_table_dict()})
    app.screen()

    # Default `TreeStyle` chevrons -- TUI paints them verbatim, no codicon
    # substitution (that's a GTK/macOS/Win-only swap).
    assert app.screen_contains("▾"), app.screen()  # "▾" expanded chevron
    assert app.screen_contains("src")
    assert app.screen_contains("main.rs")


def test_tree_table_as_a_row_leaf_coexists_with_a_fixed_slot():
    app = QuiApp(40, 10)

    def render():
        return {
            "row": [
                {"id": "btn:1", "button": {"id": "btn:1", "label": "OK"}},
                {"id": "tt:1", "tree_table": _tree_table_dict()},
            ]
        }

    app.set_render(render)
    assert app.screen_contains("OK")
    assert app.screen_contains("src")


def test_click_on_the_chevron_emits_toggle_expanded():
    app = QuiApp(40, 10)
    app.set_render(lambda: {"tree_table": _tree_table_dict()})
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    # Row 0 ("src", indent 0, the only expanded branch) sits just below the
    # one-row header, at viewport y == 1.0; its chevron paints at column 0.
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 0.0, "y": 1.0}, "modifiers": {}}}
    )
    app.run(1)

    assert {"TreeTable": ("files", {"ToggleExpanded": {"path": [0], "expand": False}})} in (
        received
    ), received


def test_click_on_the_table_body_reaches_the_handler_as_a_raw_mouse_down():
    app = QuiApp(40, 10)
    app.set_render(lambda: {"tree_table": _tree_table_dict()})
    app.screen()

    received = []
    app.set_handler(lambda event: received.append(event))
    # Past the chevron and label, still on "src"'s row (viewport y == 1.0):
    # no more specific hit resolution, so it reaches the handler raw.
    app.push_event(
        {"MouseDown": {"button": "Left", "position": {"x": 2.0, "y": 1.0}, "modifiers": {}}}
    )
    app.run(1)

    assert any("MouseDown" in event for event in received), received
    assert not any("TreeTable" in event for event in received), received
