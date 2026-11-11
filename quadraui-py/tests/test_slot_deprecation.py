"""Black-box coverage for the slot/extra API's deprecation warnings: every
`insert_*`/`insert_extra`/`clear_extra`/`slot_clear` method still works
exactly as before, but each call raises a `DeprecationWarning` naming the
inline layout-leaf kind that replaces it.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_slot_deprecation.py
"""

import pytest

from quadraui import QuiApp


def _descriptor_for(method: str) -> dict:
    """A minimal valid descriptor for each deprecated `insert_*` method."""
    return {
        "insert_button": {"id": "demo:btn", "label": "OK"},
        "insert_text_display": {"id": "demo:text", "lines": []},
        "insert_list": {
            "id": "demo:list",
            "items": [],
            "selected_idx": 0,
            "scroll_offset": 0,
            "has_focus": False,
            "show_v_scrollbar": False,
        },
        "insert_tree": {
            "id": "demo:tree",
            "rows": [],
            "selection_mode": "Single",
            "selected_path": [],
            "has_focus": False,
            "style": {
                "indent": 2,
                "show_chevrons": True,
                "chevron_expanded": "v",
                "chevron_collapsed": ">",
                "row_height": 20,
            },
        },
        "insert_data_table": {"id": "demo:table", "columns": [], "rows": []},
        "insert_tab_bar": {"id": "demo:tabs", "tabs": [], "active_tab": 0},
        "insert_progress": {"id": "demo:progress", "label": "Working", "value": 0.5},
        "insert_spinner": {"id": "demo:spinner", "label": "Loading"},
        "insert_image": {
            "id": "demo:image",
            "source": {"Path": "logo.png"},
            "intrinsic_size": None,
            "fit": "Contain",
            "fallback_text": "[LOGO]",
        },
    }[method]


@pytest.mark.parametrize(
    "method, leaf_kind",
    [
        ("insert_button", "button"),
        ("insert_text_display", "text_display"),
        ("insert_list", "list"),
        ("insert_tree", "tree"),
        ("insert_data_table", "data_table"),
        ("insert_tab_bar", "tab_bar"),
        ("insert_progress", "progress"),
        ("insert_spinner", "spinner"),
        ("insert_image", "image"),
    ],
)
def test_insert_method_warns_once_naming_the_inline_replacement(method, leaf_kind):
    app = QuiApp(40, 20)
    descriptor = _descriptor_for(method)
    with pytest.warns(DeprecationWarning, match=rf"{method}.*`{leaf_kind}`"):
        getattr(app, method)(descriptor)


def test_insert_extra_warns_naming_the_requested_kind_as_the_leaf():
    app = QuiApp(40, 20)
    with pytest.warns(DeprecationWarning, match=r"insert_extra.*`progress`"):
        app.insert_extra(
            "progress", {"id": "demo:progress2", "label": "Working", "value": 0.5}
        )


def test_clear_extra_warns_naming_omission_from_render_as_the_replacement():
    app = QuiApp(40, 20)
    app.insert_progress({"id": "demo:progress", "label": "Working", "value": 0.5})
    with pytest.warns(DeprecationWarning, match=r"clear_extra.*`progress`"):
        app.clear_extra("progress")


def test_slot_clear_warns_naming_the_fixed_slots_leaf_kind():
    app = QuiApp(40, 20)
    slot = app.insert_button({"id": "demo:btn", "label": "OK"})
    with pytest.warns(DeprecationWarning, match=r"slot_clear.*`button`"):
        app.slot_clear(slot)


def test_insert_still_works_despite_the_warning():
    """The slot/extra API is deprecated, not removed: the call underneath
    the warning still has its normal effect."""
    app = QuiApp(40, 20)
    with pytest.warns(DeprecationWarning):
        slot = app.insert_button({"id": "demo:btn", "label": "OK"})
    assert slot == 0
    assert app.screen_contains("OK")
