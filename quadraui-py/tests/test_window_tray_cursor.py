"""Black-box coverage for `QuiApp`'s window-control/tray/pointer-cursor
surface: `set_title`/`set_document_edited`/`set_represented_filename`
(`quadraui::WindowControl`), `set_tray_icon`/`set_tray_tooltip`/
`set_tray_menu` (`quadraui::backend::TrayService`), and `set_cursor`
(`quadraui::Backend::set_cursor`) -- plus their `QuiAppActions` deferred
counterparts, for use from inside a registered handler.

Every one of these resolves off this `QuiApp`'s own headless `TuiBackend`
(never a `run_native` window). `TuiBackend` genuinely implements
`set_title` (OSC 0/2) and `set_cursor` genuinely always answers `False`
(no native pointer concept) -- both asserted directly here. Every other
`WindowControl` method, and the whole `TrayService` surface (TUI has no
tray/status-bar concept at all), inherit `Err(BackendError::Unsupported)`
on this backend -- also asserted directly, the same "lock in the current
behavior" shape `test_accelerators.py`'s
`test_register_global_shortcut_raises_internal_error_on_this_binding` uses
for an analogous backend gap. The *visible* effect of a real window
title/tray icon/pointer glyph needs a `run_native` window and a human at
a real desktop session -- noted in this PR's SMOKE_TESTS instead of
asserted here.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_window_tray_cursor.py
"""

import pytest

from quadraui import InternalError, InvalidDescriptorError, QuiApp


def _key_char(ch):
    return {
        "KeyPressed": {
            "key": {"Char": ch},
            "modifiers": {"shift": False, "ctrl": False, "alt": False, "cmd": False},
            "repeat": False,
        }
    }


# ─── WindowControl ─────────────────────────────────────────────────────


def test_set_title_succeeds_on_the_headless_backend():
    # TUI's one genuine `WindowControl` capability: `set_title` emits OSC
    # 0/2 and does not raise, unlike every other method on this surface.
    app = QuiApp(10, 10)
    app.set_title("quadraui-files")


def test_set_document_edited_raises_internal_error_on_this_binding():
    app = QuiApp(10, 10)
    with pytest.raises(InternalError):
        app.set_document_edited(True)


def test_set_represented_filename_raises_internal_error_on_this_binding():
    app = QuiApp(10, 10)
    with pytest.raises(InternalError):
        app.set_represented_filename("/tmp/report.pdf")


def test_set_represented_filename_accepts_none_to_clear():
    app = QuiApp(10, 10)
    with pytest.raises(InternalError):
        app.set_represented_filename(None)


# ─── TrayService ───────────────────────────────────────────────────────


def test_set_tray_icon_raises_internal_error_on_this_binding():
    # `Backend::tray` stays `None` forever on `TuiBackend` -- a terminal has
    # no notification-area/menu-bar concept at all.
    app = QuiApp(10, 10)
    with pytest.raises(InternalError):
        app.set_tray_icon({"Path": "/tmp/icon.png"})


def test_set_tray_icon_rejects_a_malformed_descriptor():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.set_tray_icon({"NotAVariant": "oops"})


def test_set_tray_tooltip_raises_internal_error_on_this_binding():
    app = QuiApp(10, 10)
    with pytest.raises(InternalError):
        app.set_tray_tooltip("quadraui-files")


def _tray_menu_items():
    return [
        {"id": "quit", "label": {"spans": [{"text": "Quit", "fg": None, "bg": None}]}},
    ]


def test_set_tray_menu_raises_internal_error_on_this_binding():
    app = QuiApp(10, 10)
    with pytest.raises(InternalError):
        app.set_tray_menu(_tray_menu_items())


def test_set_tray_menu_rejects_a_malformed_descriptor():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.set_tray_menu([{"not": "an item"}])


# ─── Pointer cursor ─────────────────────────────────────────────────────


def test_set_cursor_returns_false_on_the_headless_backend():
    # No native pointer concept on TUI -- `Backend::set_cursor` always
    # answers `False`, not an error, same as a windowed backend before its
    # window exists.
    app = QuiApp(10, 10)
    assert app.set_cursor("pointer") is False
    assert app.set_cursor("grabbing") is False
    assert app.set_cursor("resize", edge="east") is False


def test_set_cursor_rejects_an_unknown_shape():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.set_cursor("not-a-shape")


def test_set_cursor_resize_without_an_edge_raises():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.set_cursor("resize")


def test_set_cursor_rejects_an_unknown_edge():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.set_cursor("resize", edge="not-an-edge")


# ─── `QuiAppActions` deferred counterparts ───────────────────────────────


def test_set_title_queued_from_a_handler_does_not_raise_already_borrowed():
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.set_title("quadraui-files: report.pdf")

    app.set_handler(on_event)

    # A direct `app.set_title(...)` call from inside `on_event` would hit
    # the PyO3 "already borrowed" wall instead -- this only proves the
    # deferred path doesn't.
    app.push_key_event(ord("a"))
    app.run(1)


def test_set_cursor_queued_from_a_handler_does_not_raise():
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.set_cursor("col_resize")

    app.set_handler(on_event)
    app.push_key_event(ord("a"))
    app.run(1)


def test_set_document_edited_queued_from_a_handler_reraises_its_error():
    # `with_window`/`with_tray`'s `Err(BackendError::Unsupported)` on this
    # backend still surfaces -- `flush_deferred_actions` records it onto
    # `pending_error`, and `run` re-raises it once the dispatch that
    # queued it finishes, same as a failing `open_path`/`move_to_trash`.
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.set_document_edited(True)

    app.set_handler(on_event)
    app.push_key_event(ord("a"))
    with pytest.raises(InternalError):
        app.run(1)


def test_set_tray_menu_queued_from_a_handler_rejects_a_malformed_descriptor():
    # Decoding happens synchronously inside the queueing call itself, from
    # the handler's own call frame -- a malformed descriptor still raises
    # right there, not deferred to `flush_deferred_actions`.
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        with pytest.raises(InvalidDescriptorError):
            actions.set_tray_menu([{"not": "an item"}])

    app.set_handler(on_event)
    app.push_key_event(ord("a"))
    app.run(1)
