"""Black-box coverage for `QuiApp.actions()`'s `QuiAppActions` handle: the
way a registered event handler queues `show_message_dialog`/`show_toast`/
`show_tooltip`/`send_notification`/`open_path`/`reveal_in_file_manager`/
`move_to_trash`/`restore_from_trash` (the methods `QuiApp`
itself restricts to top-level script code, since a handler already holds
this app's one mutable borrow for the whole dispatch that invoked it) and
has it applied right after that dispatch finishes, instead of raising
"already borrowed" the way calling the matching `QuiApp` method directly
from inside the handler would. `apply_tab_activated` is the one method here
with no restricted `QuiApp` counterpart at all -- see its own doc.

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


def test_show_tooltip_queued_from_a_handler_is_painted_after_step():
    app = QuiApp(40, 20)
    actions = app.actions()

    def on_event(_event):
        actions.show_tooltip(
            {"id": "tip:hint", "text": "Hello tip"}, (0.0, 2.0, 10.0, 1.0)
        )

    app.set_handler(on_event)
    assert not app.screen_contains("Hello tip")

    # A direct `app.show_tooltip(...)` call from inside `on_event` would hit
    # the PyO3 "already borrowed" wall instead.
    app.push_key_event(ord("a"))
    app.run(1)

    assert app.screen_contains("Hello tip")


def test_send_notification_queued_from_a_handler_does_not_raise():
    app = QuiApp(40, 20)
    actions = app.actions()

    def on_event(_event):
        actions.send_notification("Title", "Body", urgent=True, silent=True, tag="t1")

    app.set_handler(on_event)

    # No observable error surface for `send_notification` -- a direct
    # `app.send_notification(...)` call from inside `on_event` would hit the
    # PyO3 "already borrowed" wall instead; this only proves the deferred
    # path doesn't.
    app.push_key_event(ord("a"))
    app.run(1)


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


def test_restore_from_trash_queued_from_a_handler_actually_restores_the_file(
    tmp_path,
):
    victim = tmp_path / "doomed.txt"
    victim.write_text("bye")
    app = QuiApp(10, 10)
    actions = app.actions()

    # Trash it first via a direct `QuiApp` call (no handler borrow in play
    # yet at this point), then queue the restore from inside a handler --
    # exactly the undo-toast shape `restore_from_trash` exists for.
    app.move_to_trash(str(victim))
    assert not victim.exists()

    def on_event(_event):
        actions.restore_from_trash(str(victim))

    app.set_handler(on_event)
    app.push_key_event(ord("a"))
    app.run(1)

    assert victim.exists()
    assert victim.read_text() == "bye"


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


def test_apply_tab_activated_persists_without_a_click_or_pinning_the_descriptor():
    """`actions.apply_tab_activated(id, index)` makes `index` the engine's
    own stored active tab for `id`, the same persisted change a real
    `TabBar`/`TabActivated` click makes -- without a click, and without the
    render callback ever marking any tab `is_active` (it never does here,
    so the descriptor stays uncontrolled throughout, the shape
    `examples/file_manager.py`'s own `_details_pane` relies on)."""

    def render():
        return {
            "tab_bar": {
                "id": "deferred-test:tabs",
                "tabs": [{"label": "Tab 1"}, {"label": "Tab 2"}],
            }
        }

    app = QuiApp(40, 20, render=render)
    actions = app.actions()
    # A `TabBar` slot is tracked (and so present in the snapshot) from its
    # very first render, same as every other widget kind -- but with
    # nothing active yet, since the descriptor above never marks one.
    assert app.widget_state("deferred-test:tabs").get("active_tab") is None

    def on_event(_event):
        actions.apply_tab_activated("deferred-test:tabs", 1)

    app.set_handler(on_event)
    app.push_key_event(ord("a"))
    app.run(1)

    assert app.widget_state("deferred-test:tabs")["active_tab"] == 1
