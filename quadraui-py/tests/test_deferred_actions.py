"""Black-box coverage for `QuiApp.actions()`'s `QuiAppActions` handle: the
way a registered event handler queues `show_message_dialog`/`show_toast`/
`open_path`/`reveal_in_file_manager`/`move_to_trash` (the methods `QuiApp`
itself restricts to top-level script code, since a handler already holds
this app's one mutable borrow for the whole dispatch that invoked it) and
has it applied right after that dispatch finishes, instead of raising
"already borrowed" the way calling the matching `QuiApp` method directly
from inside the handler would.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_deferred_actions.py
"""

from quadraui import QuiApp


def _key_char(ch):
    return {
        "KeyPressed": {
            "key": {"Char": ch},
            "modifiers": {"shift": False, "ctrl": False, "alt": False, "cmd": False},
            "repeat": False,
        }
    }


def test_show_message_dialog_queued_from_a_handler_is_painted_after_step():
    app = QuiApp(40, 20)
    actions = app.actions()

    def on_event(_event):
        actions.show_message_dialog(
            "Confirm", "Really?", [{"id": "ok", "label": "OK"}]
        )

    app.set_handler(on_event)
    assert not app.screen_contains("Confirm")

    # Nothing in this app's own dispatch raises -- a direct
    # `app.show_message_dialog(...)` call from inside `on_event` would hit
    # the PyO3 "already borrowed" wall instead.
    app.push_key_event(ord("a"))
    app.run(1)

    assert app.screen_contains("Confirm")
    assert app.screen_contains("OK")


def test_show_toast_queued_from_a_handler_is_painted_after_step():
    app = QuiApp(40, 20)
    actions = app.actions()

    def on_event(_event):
        actions.show_toast({"id": "toast:saved", "title": "Saved", "body": ""})

    app.set_handler(on_event)
    app.push_key_event(ord("a"))
    app.run(1)

    assert app.screen_contains("Saved")


def test_move_to_trash_queued_from_a_handler_actually_moves_the_file(tmp_path):
    victim = tmp_path / "doomed.txt"
    victim.write_text("bye")
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.move_to_trash(str(victim))

    app.set_handler(on_event)
    app.push_key_event(ord("a"))
    app.run(1)

    assert not victim.exists()


def test_multiple_actions_queued_from_one_dispatch_all_apply():
    app = QuiApp(40, 20)
    actions = app.actions()

    def on_event(_event):
        actions.show_toast({"id": "toast:a", "title": "Alpha", "body": ""})
        actions.show_toast({"id": "toast:b", "title": "Bravo", "body": ""})

    app.set_handler(on_event)
    app.push_key_event(ord("a"))
    app.run(1)

    assert app.screen_contains("Alpha")
    assert app.screen_contains("Bravo")
