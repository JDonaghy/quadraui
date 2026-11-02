"""Black-box coverage for `QuiApp`'s accelerator / global-shortcut surface:
`register_accelerator`, `unregister_accelerator`, `register_global_shortcut`,
`unregister_global_shortcut`, `unregister_all_global_shortcuts`.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_accelerators.py
"""

import pytest

from quadraui import InternalError, InvalidDescriptorError, QuiApp

_CTRL = {"shift": False, "ctrl": True, "alt": False, "cmd": False}


def _ctrl_s_event():
    return {
        "KeyPressed": {
            "key": {"Char": "s"},
            "modifiers": _CTRL,
            "repeat": False,
        }
    }


def test_registered_accelerator_fires_an_accelerator_event_for_its_chord():
    app = QuiApp(10, 10)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.register_accelerator(
        {"id": "app.save", "binding": "Save", "scope": "Global", "label": None}
    )
    app.push_event(_ctrl_s_event())
    app.run(1)

    assert len(received) == 1
    accel_id, modifiers = received[0]["Accelerator"]
    assert accel_id == "app.save"
    assert modifiers == _CTRL


def test_literal_vim_style_binding_matches_the_same_chord_as_the_universal_save():
    app = QuiApp(10, 10)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.register_accelerator(
        {
            "id": "app.save.literal",
            "binding": {"Literal": "<C-s>"},
            "scope": "Global",
            "label": None,
        }
    )
    app.push_event(_ctrl_s_event())
    app.run(1)

    assert len(received) == 1
    accel_id, _ = received[0]["Accelerator"]
    assert accel_id == "app.save.literal"


def test_unregistered_accelerator_leaves_the_key_press_as_a_plain_key_pressed():
    app = QuiApp(10, 10)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.register_accelerator(
        {"id": "app.save", "binding": "Save", "scope": "Global", "label": None}
    )
    app.unregister_accelerator("app.save")
    app.push_event(_ctrl_s_event())
    app.run(1)

    assert len(received) == 1
    assert "KeyPressed" in received[0]
    assert "Accelerator" not in received[0]


def test_unregister_accelerator_is_idempotent_on_an_unknown_id():
    app = QuiApp(10, 10)
    # Never registered -- must not raise.
    app.unregister_accelerator("never.registered")


def test_registering_the_same_id_again_replaces_its_binding_wholesale():
    app = QuiApp(10, 10)
    received = []
    app.set_handler(lambda event: received.append(event))

    app.register_accelerator(
        {"id": "app.action", "binding": "Save", "scope": "Global", "label": None}
    )
    # Re-registering the same id swaps in a new binding, dropping the old
    # one entirely rather than keeping both live.
    app.register_accelerator(
        {
            "id": "app.action",
            "binding": {"Literal": "<C-q>"},
            "scope": "Global",
            "label": None,
        }
    )

    app.push_event(_ctrl_s_event())
    app.run(1)
    assert len(received) == 1
    assert "KeyPressed" in received[0], (
        "the replaced Ctrl+S binding must no longer fire"
    )

    received.clear()
    app.push_event(
        {
            "KeyPressed": {
                "key": {"Char": "q"},
                "modifiers": _CTRL,
                "repeat": False,
            }
        }
    )
    app.run(1)
    assert len(received) == 1
    accel_id, _ = received[0]["Accelerator"]
    assert accel_id == "app.action"


def test_register_accelerator_rejects_a_malformed_descriptor():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.register_accelerator({"not": "an accelerator"})


def test_register_global_shortcut_raises_internal_error_on_this_binding():
    # No in-tree `PlatformServices` override claims a real OS-wide hotkey
    # yet, so every backend this binding ships -- including the headless
    # `TuiBackend` `QuiApp` always wraps -- inherits
    # `Backend::register_global_shortcut`'s `Unsupported` default. This
    # locks in that seam's current behavior; a real success path needs
    # manual verification once a backend implements one.
    app = QuiApp(10, 10)
    with pytest.raises(InternalError):
        app.register_global_shortcut(
            {
                "id": "app.summon",
                "binding": {"Literal": "<C-S-space>"},
                "scope": "Global",
                "label": None,
            }
        )


def test_unregister_global_shortcut_raises_internal_error_on_this_binding():
    # Same rationale as `register_global_shortcut` above: this inherits
    # `Unsupported` regardless of which id is passed, since
    # `register_global_shortcut` never succeeds here to hand back a real
    # one to round-trip through this call instead.
    app = QuiApp(10, 10)
    with pytest.raises(InternalError):
        app.unregister_global_shortcut(0)


def test_unregister_all_global_shortcuts_raises_internal_error_on_this_binding():
    app = QuiApp(10, 10)
    with pytest.raises(InternalError):
        app.unregister_all_global_shortcuts()
