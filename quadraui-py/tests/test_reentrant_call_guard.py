"""Black-box coverage for three ergonomics fixes to `QuiApp`'s error
surface, all exercised through `pytest.raises`/direct assertions rather
than inspecting any Rust-internal state:

1. Every `QuiApp` method restricted to top-level script code (the ones
   `QuiAppActions` mirrors -- `show_message_dialog`, `set_title`, ...)
   raises `ReentrantCallError`, with a message naming the method and
   pointing at `actions()`, instead of PyO3's raw
   `RuntimeError: Already borrowed`, when called from inside this app's
   own registered handler.
2. A `render` callback that returns something other than a `dict` (a
   `list`, a `str`, `None`, ...) raises `InvalidDescriptorError` with the
   message `"render callback must return a dict"` -- see
   `test_render_output_from_callback.py`-style coverage for the dict-shape
   decode path; this file only covers the top-level-shape gate.
3. `step()`/`run()` with nothing queued (no event, no tick handler that
   requests a redraw) still renders once, so a `render` callback that
   decodes badly raises immediately rather than only on the next
   `screen()`/`screen_contains()` call.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_reentrant_call_guard.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp, ReentrantCallError


def _key_char(ch):
    return {
        "KeyPressed": {
            "key": {"Char": ch},
            "modifiers": {"shift": False, "ctrl": False, "alt": False, "cmd": False},
            "repeat": False,
        }
    }


@pytest.mark.parametrize(
    ("method", "args"),
    [
        ("show_message_dialog", ("Title", "Body", [{"id": "ok", "label": "OK"}])),
        ("show_context_menu", ([{"id": "a", "label": "A"}], (0.0, 0.0))),
        ("show_toast", ({"id": "t1", "title": "Hi", "body": "there"},)),
        ("show_tooltip", ({"id": "tip", "text": "hint"}, (0.0, 0.0, 1.0, 1.0))),
        ("hide_tooltip", ()),
        ("send_notification", ("Title", "Body")),
        ("open_path", ("/tmp",)),
        ("reveal_in_file_manager", ("/tmp",)),
        ("move_to_trash", ("/tmp/does-not-exist-quadraui-test",)),
        ("restore_from_trash", ("/tmp/does-not-exist-quadraui-test",)),
        ("set_title", ("New Title",)),
        ("set_document_edited", (True,)),
        ("set_represented_filename", ("/tmp/report.pdf",)),
        ("set_tray_icon", ({"Path": "/tmp/icon.png"},)),
        ("set_tray_tooltip", ("hint",)),
        ("set_tray_menu", ([{"id": "a", "label": "A"}],)),
        ("set_cursor", ("default",)),
        ("set_theme", ({"fg": "#ffffff"},)),
        ("define_style", ("card", {"bg": "#445566"})),
        ("add_recent_document", ("/tmp/does-not-exist-quadraui-test",)),
        ("clear_recent_documents", ()),
        ("set_dock_menu", ([{"id": "a", "label": "A"}],)),
        ("set_jump_list", ([{"title": "t", "program": "p"}],)),
        ("set_login_item", (True,)),
        ("set_as_default_protocol_client", ("quadraui-test",)),
        ("remove_as_default_protocol_client", ("quadraui-test",)),
        ("begin_drag_out", (["/tmp/does-not-exist-quadraui-test"],)),
        ("end_power_save_block", (1,)),
        ("apply_on_quit", (1,)),
    ],
)
def test_restricted_method_called_from_a_handler_raises_reentrant_call_error(method, args):
    app = QuiApp(40, 20)
    caught = []

    def on_event(_event):
        try:
            getattr(app, method)(*args)
        except Exception as exc:  # noqa: BLE001 -- re-raised as an assertion below
            caught.append(exc)

    app.set_handler(on_event)
    app.push_event(_key_char("a"))
    app.step()

    assert len(caught) == 1, f"{method} did not raise when called re-entrantly"
    err = caught[0]
    assert isinstance(err, ReentrantCallError)
    assert isinstance(err, Exception)
    assert str(err) == (
        f"app.{method} can't be called from inside a handler; use the "
        "actions() handle passed to / available in the handler"
    )


def test_restricted_method_still_works_normally_from_top_level_script_code():
    # The reentrancy guard must not get in the way of the ordinary,
    # non-reentrant call every other test in this package relies on.
    app = QuiApp(40, 20)
    app.set_title("hello")
    app.show_message_dialog("Confirm", "Really?", [{"id": "ok", "label": "OK"}])
    assert app.screen_contains("Confirm")


def test_render_callback_returning_a_list_raises_invalid_descriptor_error():
    app = QuiApp(10, 5)
    app.set_render(lambda: [{"button": {"id": "x", "label": "x"}}])
    with pytest.raises(InvalidDescriptorError, match="render callback must return a dict"):
        app.screen()


def test_idle_step_with_nothing_queued_still_renders_and_surfaces_a_render_error():
    app = QuiApp(10, 5)
    calls = []
    app.set_render(lambda: calls.append(None) or [{"button": {"id": "x", "label": "x"}}])

    with pytest.raises(InvalidDescriptorError, match="render callback must return a dict"):
        app.step()

    assert len(calls) == 1, "an idle step() must invoke the render callback"


def test_idle_step_with_nothing_queued_still_renders_a_valid_frame():
    # `screen()`/`screen_contains()` always force their own render, so this
    # counts calls directly rather than inspecting painted content --
    # that's the only way to observe specifically `step()`'s own render
    # without a second call muddying the count.
    app = QuiApp(20, 5)
    calls = []
    app.set_render(lambda: calls.append(None) or {"button": {"id": "demo:btn", "label": "OK"}})
    assert len(calls) == 0

    # No event, no tick handler requesting a redraw -- still re-renders.
    app.step()
    assert len(calls) == 1
