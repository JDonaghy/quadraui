"""Black-box coverage for mouse text selection + Ctrl-C copy against a
`message_list` descriptor.

Mirrors `test_text_selection.py`'s `text_display` coverage: `ExtraPaint for
MessageList`'s `paint` (`quadraui-binding-core/src/extra_paint.rs`) registers
a `TextRegion` for the painted transcript, so a drag across it begins a
`DragTarget::TextSelection` and a following `MouseMoved` emits
`TextSelectionChanged`; `Ctrl-C` with an active selection emits `TextCopied`.
"""

from quadraui import QuiApp

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "cmd": False}


def _insert_message_list(app):
    app.insert_extra(
        "message_list",
        {
            "id": "messages:1",
            "rows": [
                {
                    "text": f"Row {i}",
                    "fg": {"r": 200, "g": 200, "b": 200, "a": 255},
                }
                for i in range(3)
            ],
        },
    )


def test_drag_over_message_list_emits_text_selection_changed():
    app = QuiApp(40, 20)
    _insert_message_list(app)

    received = []
    app.set_handler(lambda event: received.append(event))

    # Start drag at (2, 0), move to (2, 2) -- inside the 3-row band
    # `insert_extra` reserves at the top of the viewport for the first
    # (and here, only) additive primitive.
    app.push_event(
        {
            "MouseDown": {
                "button": "Left",
                "position": {"x": 2.0, "y": 0.0},
                "modifiers": _NO_MODS,
            }
        }
    )
    app.push_event(
        {
            "MouseMoved": {
                "position": {"x": 2.0, "y": 2.0},
                "buttons": {"left": True, "right": False, "middle": False},
            }
        }
    )
    app.run(2)

    changes = [
        event["TextSelectionChanged"]
        for event in received
        if "TextSelectionChanged" in event
    ]
    assert len(changes) == 1, received
    assert changes[0]["region"] == "messages:1"
    assert changes[0]["anchor"]["y"] < 1.0


def test_ctrl_c_with_active_message_list_selection_emits_text_copied():
    app = QuiApp(40, 20)
    _insert_message_list(app)

    received = []
    app.set_handler(lambda event: received.append(event))

    app.push_event(
        {
            "MouseDown": {
                "button": "Left",
                "position": {"x": 0.0, "y": 0.0},
                "modifiers": _NO_MODS,
            }
        }
    )
    app.push_event(
        {
            "MouseMoved": {
                "position": {"x": 0.0, "y": 1.0},
                "buttons": {"left": True, "right": False, "middle": False},
            }
        }
    )
    app.push_event(
        {
            "MouseUp": {
                "button": "Left",
                "position": {"x": 0.0, "y": 1.0},
            }
        }
    )
    app.push_event(
        {
            "KeyPressed": {
                "key": {"Char": "c"},
                "modifiers": {"shift": False, "ctrl": True, "alt": False, "cmd": False},
                "repeat": False,
            }
        }
    )
    app.run(4)

    copied = [event["TextCopied"] for event in received if "TextCopied" in event]
    assert len(copied) == 1, received
    # Same line-wise selection rules as `test_text_selection.py`'s own
    # Ctrl-C test: anchor (0, 0) to focus (0, 1) selects row 0 in full and
    # just row 1's first character.
    assert copied[0] == "Row 0\nR"


def test_ctrl_c_without_message_list_selection_does_not_emit_text_copied():
    app = QuiApp(40, 20)
    _insert_message_list(app)

    received = []
    app.set_handler(lambda event: received.append(event))

    app.push_event(
        {
            "KeyPressed": {
                "key": {"Char": "c"},
                "modifiers": {"shift": False, "ctrl": True, "alt": False, "cmd": False},
                "repeat": False,
            }
        }
    )
    app.run(1)

    assert not any("TextCopied" in event for event in received)
    assert any("KeyPressed" in event for event in received)
