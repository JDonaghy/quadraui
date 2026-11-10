"""Black-box coverage for `QuiApp.widget_state` and the engine's own
widget-state store resolving a `Tree`/`Split` descriptor's cursor, scroll,
and divider ratio when a render callback leaves them uncontrolled --
the same store `examples/file_manager.py` relies on to drop its own
hand-managed `scroll_offset`/split-ratio tracking (see that file's own
module doc) and the more general "fully uncontrolled cursor" case it
doesn't itself need (it keeps its own cursor via `selected_path`, only
`scroll_offset` is left uncontrolled there).

Exercises both render-callback shapes a binding decodes
(`quadraui_binding_core::wire::RenderOutput`): a bare flat `{"tree": ...}`
dict (`RenderOutput::Flat`) and a layout tree wrapping the same leaf in a
`"row"` (`RenderOutput::Tree`) -- `resolve_render_frame_state`/
`resolve_layout_node_state` are two separate call sites in
`quadraui-py/src/lib.rs`, so each needs its own coverage.
"""

import pytest

from quadraui import QuiApp

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "meta": False}
_DOWN = {"KeyPressed": {"key": {"Named": "Down"}, "modifiers": _NO_MODS, "repeat": False}}
# Nothing is focused on a freshly built `QuiApp` (`FocusManager::new` starts
# empty -- see `quadraui::focus`'s own doc); a focused tree's own arrow-key
# navigation (`apply_tree_nav_key`, gated on `backend.focus_manager()`)
# never runs until something is. Tab is the one event
# `quadraui::runtime::preprocess_event` reserves for cycling it onto the
# first tab stop, mirroring the real terminal/native keyboard story.
_TAB = {"KeyPressed": {"key": {"Named": "Tab"}, "modifiers": _NO_MODS, "repeat": False}}

_TREE_STYLE = {
    "indent": 2,
    "show_chevrons": False,
    "chevron_expanded": "v",
    "chevron_collapsed": ">",
    "row_height": 1,
}


def _styled_text(text: str) -> dict:
    return {"spans": [{"text": text, "fg": None, "bg": None}]}


def _tree_rows(n: int) -> list[dict]:
    return [
        {
            "path": [i],
            "indent": 0,
            "icon": None,
            "text": _styled_text(f"row {i}"),
            "badge": None,
            "is_expanded": None,
        }
        for i in range(n)
    ]


def _tree_descriptor(rows: list[dict], *, scroll_offset: int | None = None) -> dict:
    """A folders-only-style tree descriptor with no `selected_path` of its
    own -- uncontrolled, same as `examples/file_manager.py`'s `tree_leaf`
    now builds -- and `scroll_offset` only set at all when `scroll_offset`
    is given (its own `#[serde(default)]` makes an omitted key decode to
    `0`, the uncontrolled sentinel)."""
    tree = {
        "id": "widget-state-test:tree",
        "rows": rows,
        "selection_mode": "Single",
        "selected_path": None,
        "has_focus": True,
        "style": _TREE_STYLE,
    }
    if scroll_offset is not None:
        tree["scroll_offset"] = scroll_offset
    return tree


def _press(app: QuiApp, event: dict) -> None:
    app.push_event(event)
    app.step()


# `PyAppLogic::handle` skips `InteractionEngine` (and so every `apply_*`
# call the engine's own key/drag/click routing makes) entirely when no
# handler is registered at all, so a `QuiApp` driving any of these tests'
# key events needs one -- a no-op is enough; `examples/file_manager.py`
# always has a real one (`set_handler`), so this is purely a test-harness
# detail, not something the real example needs to work around.
_NOOP_HANDLER = lambda event: None  # noqa: E731


def test_widget_state_is_none_for_an_untracked_id():
    app = QuiApp(20, 5, render=lambda: {"text_display": {"id": "t", "lines": []}})
    app.step()
    assert app.widget_state("widget-state-test:nothing-here") is None


def test_uncontrolled_tree_cursor_moves_and_scrolls_via_the_layout_tree_path():
    """A layout-tree render callback (`RenderOutput::Tree`) that never sets
    `selected_path`/`scroll_offset` still has arrow keys move the cursor
    and the view scroll to keep it visible -- with no app code tracking
    either, purely via `resolve_layout_node_state`."""
    rows = _tree_rows(30)

    def render():
        return {"row": [{"id": "widget-state-test:tree", "tree": _tree_descriptor(rows)}]}

    app = QuiApp(20, 6, render=render, on_event=_NOOP_HANDLER)
    _press(app, _TAB)  # focus the tree -- the only tab stop
    # The first Down from no selection lands on row 0; each of the other 9
    # moves by one -- 10 presses land on row index 9.
    for _ in range(10):
        _press(app, _DOWN)

    state = app.widget_state("widget-state-test:tree")
    assert state["selected_path"] == [9]
    # The engine's own `TreeController::scroll_to_visible` keeps the
    # selected row within one viewport of the scroll offset; a border this
    # tiny app's own focus ring overlays on top of the painted band (see
    # `quadraui::Backend::draw_focus_ring`) shrinks what's cosmetically
    # *visible* without changing this invariant, so this checks the
    # relationship rather than a literal screen row.
    assert 0 <= state["selected_path"][0] - state["scroll_offset"]
    assert state["scroll_offset"] > 0


def test_uncontrolled_tree_cursor_moves_and_scrolls_via_the_flat_frame_path():
    """The same behavior as the layout-tree test above, through the flat
    six-slot render shape (`RenderOutput::Flat`) instead --
    `resolve_render_frame_state`'s own call site."""
    rows = _tree_rows(30)

    def render():
        return {"tree": _tree_descriptor(rows)}

    app = QuiApp(20, 6, render=render, on_event=_NOOP_HANDLER)
    _press(app, _TAB)  # focus the tree -- the only tab stop
    # Same "first Down lands on row 0" counting as the layout-tree test
    # above -- 5 presses land on row index 4.
    for _ in range(5):
        _press(app, _DOWN)

    state = app.widget_state("widget-state-test:tree")
    assert state["selected_path"] == [4]
    assert 0 <= state["selected_path"][0] - state["scroll_offset"]


def test_controlled_scroll_offset_still_pins_the_scroll():
    """A render callback that keeps pinning `scroll_offset` to a fixed,
    non-zero value every frame wins outright over the store's own
    scroll-to-visible, even while the cursor keeps moving uncontrolled --
    `resolve_tree_cursor`'s own controlled/uncontrolled contract, and the
    acceptance bar "existing examples keep working unchanged" relies on."""
    rows = _tree_rows(30)

    def render():
        return {"row": [{"id": "widget-state-test:tree", "tree": _tree_descriptor(rows, scroll_offset=5)}]}

    app = QuiApp(20, 6, render=render, on_event=_NOOP_HANDLER)
    _press(app, _TAB)  # focus the tree -- the only tab stop
    # Same "first Down lands on row 0" counting as the two tests above --
    # 10 presses land on row index 9.
    for _ in range(10):
        _press(app, _DOWN)

    state = app.widget_state("widget-state-test:tree")
    assert state["selected_path"] == [9]
    assert state["scroll_offset"] == 5


def test_split_ratio_drag_persists_with_no_app_code():
    """A render callback that always resends the same literal `ratio` --
    the shape an app tracking no divider state of its own would send --
    still has a drag persist across every later render, purely via
    `resolve_split_ratio`."""

    def render():
        return {
            "split": {
                "id": "widget-state-test:split",
                "orientation": "horizontal",
                "ratio": 0.4,
                "first": {"id": "a", "text_display": {"id": "a", "lines": []}},
                "second": {"id": "b", "text_display": {"id": "b", "lines": []}},
            }
        }

    app = QuiApp(20, 6, render=render, on_event=_NOOP_HANDLER)
    assert app.widget_state("widget-state-test:split")["ratio"] == pytest.approx(0.4)

    _press(
        app,
        {
            "SplitDividerDragged": {
                "tree": "widget-state-test:split",
                "split_index": 0,
                "new_ratio": 0.7,
            }
        },
    )

    state = app.widget_state("widget-state-test:split")
    assert state["ratio"] == pytest.approx(0.7)
