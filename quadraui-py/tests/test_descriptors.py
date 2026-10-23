"""Black-box coverage for `QuiApp`: insert each of the six core primitive
descriptors from a plain Python dict and assert the rendered screen shows
it.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_descriptors.py
"""

import pytest

from quadraui import QuiApp


def test_insert_button():
    app = QuiApp(40, 20)
    slot = app.insert_button({"id": "demo:ok", "label": "OK"})
    assert slot == 0
    assert app.screen_contains("OK")


def test_insert_text_display():
    app = QuiApp(40, 20)
    app.insert_text_display(
        {
            "id": "text:1",
            "lines": [{"spans": [{"text": "Hello", "fg": None, "bg": None}]}],
        }
    )
    assert app.screen_contains("Hello")


def test_insert_list():
    app = QuiApp(40, 20)
    app.insert_list(
        {
            "id": "list:1",
            "items": [
                {"text": {"spans": [{"text": "Row 1", "fg": None, "bg": None}]}}
            ],
            "selected_idx": 0,
        }
    )
    assert app.screen_contains("Row 1")


def test_insert_tree():
    app = QuiApp(40, 20)
    app.insert_tree(
        {
            "id": "tree:1",
            "rows": [
                {
                    "path": [0],
                    "indent": 0,
                    "icon": None,
                    "text": {"spans": [{"text": "Alpha", "fg": None, "bg": None}]},
                    "badge": None,
                    "is_expanded": None,
                }
            ],
            "selection_mode": "Single",
            "selected_path": None,
            "style": {
                "indent": 2,
                "show_chevrons": True,
                "chevron_expanded": "v",
                "chevron_collapsed": ">",
            },
        }
    )
    assert app.screen_contains("Alpha")


def test_insert_data_table():
    app = QuiApp(40, 20)
    app.insert_data_table(
        {
            "id": "table:1",
            "columns": [{"title": "Name"}],
            "rows": [
                {"cells": [{"spans": [{"text": "Row1", "fg": None, "bg": None}]}]}
            ],
        }
    )
    assert app.screen_contains("Row1")


def test_insert_tab_bar():
    app = QuiApp(40, 20)
    app.insert_tab_bar({"id": "tabbar:1", "tabs": [{"label": "Tab 1"}]})
    assert app.screen_contains("Tab 1")


def test_slot_occupied_then_cleared():
    app = QuiApp(40, 20)
    app.insert_button({"id": "demo:ok", "label": "OK"})
    with pytest.raises(RuntimeError):
        app.insert_button({"id": "demo:ok2", "label": "OK2"})

    assert app.slot_clear(0) is True
    assert not app.screen_contains("OK")
    slot = app.insert_button({"id": "demo:ok2", "label": "OK2"})
    assert slot == 0
    assert app.screen_contains("OK2")


def test_invalid_descriptor_raises_value_error():
    app = QuiApp(40, 20)
    with pytest.raises(ValueError):
        app.insert_button({"not": "a button"})


def test_zero_sized_app_raises_value_error():
    with pytest.raises(ValueError):
        QuiApp(0, 0)
    with pytest.raises(ValueError):
        QuiApp(40, 0)
    with pytest.raises(ValueError):
        QuiApp(0, 20)


def test_slot_clear_unknown_slot_raises_value_error():
    app = QuiApp(40, 20)
    with pytest.raises(ValueError):
        app.slot_clear(999)


def test_slot_clear_empty_slot_returns_false():
    app = QuiApp(40, 20)
    assert app.slot_clear(0) is False


def test_screen_reflects_inserted_descriptors():
    app = QuiApp(40, 20)
    app.insert_button({"id": "demo:ok", "label": "OK"})
    screen = app.screen()
    assert isinstance(screen, str)
    assert "OK" in screen
    # One line per row of the cell grid.
    assert screen.count("\n") >= 19
