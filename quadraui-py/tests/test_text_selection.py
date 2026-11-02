"""Black-box coverage for mouse text selection + Ctrl-C copy against a
`text_display` descriptor.

`insert_text_display`'s render path registers a `TextRegion` for the
painted area (`quadraui-binding-core/src/render.rs`'s `draw_text_display_hit`),
so a drag across it begins a `DragTarget::TextSelection` and a following
`MouseMoved` emits `TextSelectionChanged`; `Ctrl-C` with an active selection
emits `TextCopied` instead of forwarding the raw key press. This mirrors
`quadraui::tui::run`'s own `drag_over_text_region_emits_selection_changed` /
`ctrl_c_with_selection_emits_text_copied` Rust tests, through the Python
surface.
"""

from quadraui import QuiApp

_NO_MODS = {"shift": False, "ctrl": False, "alt": False, "cmd": False}


def _insert_text_display(app):
    app.insert_text_display(
        {
            "id": "text:1",
            "lines": [
                {"spans": [{"text": f"Line {i}", "fg": None, "bg": None}]}
                for i in range(3)
            ],
        }
    )


def test_drag_over_text_display_emits_text_selection_changed():
    app = QuiApp(40, 20)
    _insert_text_display(app)

    received = []
    app.set_handler(lambda event: received.append(event))

    # Start drag at (2, 0), move to (2, 2) -- inside the 3-row band
    # `insert_text_display`'s fixed-slot render reserves at the top of the
    # viewport.
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
    assert changes[0]["region"] == "text:1"
    # The anchor should still be close to where the drag started (row 0).
    assert changes[0]["anchor"]["y"] < 1.0


def test_ctrl_c_with_active_selection_emits_text_copied():
    app = QuiApp(40, 20)
    _insert_text_display(app)

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
    assert isinstance(copied[0], str)
    assert copied[0] != ""


def test_ctrl_c_without_selection_does_not_emit_text_copied():
    app = QuiApp(40, 20)
    _insert_text_display(app)

    received = []
    app.set_handler(lambda event: received.append(event))

    # No drag -- no active selection -- Ctrl-C stays a raw KeyPressed.
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
