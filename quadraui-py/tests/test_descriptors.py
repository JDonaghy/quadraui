"""Black-box coverage for `QuiApp`: insert each of the six core primitive
descriptors from a plain Python dict and assert the rendered screen shows
it.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_descriptors.py
"""

import pytest

from quadraui import (
    InvalidDescriptorError,
    InvalidDimensionsError,
    QuiApp,
    SlotOccupiedError,
    UnknownSlotError,
)


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


def test_text_display_wheel_scroll_emits_scrolled():
    # `insert_text_display`'s own render already populated the hit map, so
    # the display is hit-testable the moment it's inserted. Five lines into
    # a 3-line viewport (the fixed-slot band height every `QuiApp` default
    # render reserves) leaves two lines of scroll room; a wheel notch
    # forward (`delta.y < 0`, per `wheel_row_delta`'s convention) must move
    # `scroll_offset` from 0 to 1, landing on `TextDisplayEvent::Scrolled`
    # rather than a raw `Scroll` with `widget: None`.
    app = QuiApp(40, 20)
    app.insert_text_display(
        {
            "id": "text:1",
            "lines": [
                {"spans": [{"text": f"Line {i}", "fg": None, "bg": None}]}
                for i in range(5)
            ],
            "auto_scroll": False,
        }
    )

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {
            "Scroll": {
                "delta": {"x": 0.0, "y": -1.0},
                "position": {"x": 1.0, "y": 1.0},
            }
        }
    )
    app.run(1)

    assert len(received) == 1
    widget_id, text_display_event = received[0]["TextDisplay"]
    assert widget_id == "text:1"
    assert text_display_event == {"Scrolled": {"new_offset": 1}}


def test_text_display_with_title_wheel_scroll_emits_scrolled():
    # Every backend's text-display layout lays the body out from `0.0` and
    # leaves the title row to be painted separately above it, so a hit on
    # the *last* visible body row (one row lower than an untitled display's
    # own last row) is the regression shape: before the hit-test accounted
    # for the title's body offset, a wheel notch there missed the layout
    # entirely and no `Scrolled` event was emitted at all.
    app = QuiApp(40, 20)
    app.insert_text_display(
        {
            "id": "text:1",
            "title": {"spans": [{"text": "Logs", "fg": None, "bg": None}]},
            "lines": [
                {"spans": [{"text": f"Line {i}", "fg": None, "bg": None}]}
                for i in range(5)
            ],
            "auto_scroll": False,
        }
    )

    received = []
    app.set_handler(lambda event: received.append(event))
    app.push_event(
        {
            "Scroll": {
                "delta": {"x": 0.0, "y": -1.0},
                # y=2.0: one row for the title, one more for the first body
                # line, landing on the last body row the 3-line band leaves
                # room for once the title has taken its own row.
                "position": {"x": 1.0, "y": 2.0},
            }
        }
    )
    app.run(1)

    assert len(received) == 1
    widget_id, text_display_event = received[0]["TextDisplay"]
    assert widget_id == "text:1"
    assert text_display_event == {"Scrolled": {"new_offset": 1}}


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


def test_insert_tree_with_codicon_icon():
    """A tree row's `icon` accepts `{"codicon": "...", "fallback": "..."}`
    in place of `{"glyph": "...", "fallback": "..."}` — the wire shape
    `Icon`'s `Deserialize` impl resolves through the bundled codicon name
    table. `QuiApp`'s headless driver defaults to nerd fonts off, so the
    rendered row shows the icon's plain-text fallback.
    """
    app = QuiApp(40, 20)
    app.insert_tree(
        {
            "id": "tree:1",
            "rows": [
                {
                    "path": [0],
                    "indent": 0,
                    "icon": {"codicon": "folder", "fallback": "DIR"},
                    "text": {"spans": [{"text": "src", "fg": None, "bg": None}]},
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
    assert app.screen_contains("DIR")
    assert app.screen_contains("src")


def test_insert_tree_with_unknown_codicon_name_raises():
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.insert_tree(
            {
                "id": "tree:1",
                "rows": [
                    {
                        "path": [0],
                        "indent": 0,
                        "icon": {"codicon": "not-a-real-codicon-name", "fallback": "?"},
                        "text": {"spans": [{"text": "src", "fg": None, "bg": None}]},
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
    with pytest.raises(SlotOccupiedError):
        app.insert_button({"id": "demo:ok2", "label": "OK2"})

    assert app.slot_clear(0) is True
    assert not app.screen_contains("OK")
    slot = app.insert_button({"id": "demo:ok2", "label": "OK2"})
    assert slot == 0
    assert app.screen_contains("OK2")


def test_invalid_descriptor_raises_invalid_descriptor_error():
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDescriptorError):
        app.insert_button({"not": "a button"})


def test_zero_sized_app_raises_invalid_dimensions_error():
    with pytest.raises(InvalidDimensionsError):
        QuiApp(0, 0)
    with pytest.raises(InvalidDimensionsError):
        QuiApp(40, 0)
    with pytest.raises(InvalidDimensionsError):
        QuiApp(0, 20)


def test_slot_clear_unknown_slot_raises_unknown_slot_error():
    app = QuiApp(40, 20)
    with pytest.raises(UnknownSlotError):
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
