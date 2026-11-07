"""Black-box coverage for the platform-services surface this binding exposes
as thin PyO3 wrappers over `quadraui_binding_core::services`: `QuiApp`'s
auto-update (`check_for_update`/`download_and_stage`/`apply_on_quit`),
power-save blocker (`begin_power_save_block`/`end_power_save_block`),
launch-at-login (`set_login_item`/`login_item_enabled`), protocol
registration (`set_as_default_protocol_client`/`is_default_protocol_client`/
`remove_as_default_protocol_client`), drag-out (`begin_drag_out`), recent
docs/dock menu/jump list (`add_recent_document`/`clear_recent_documents`/
`set_dock_menu`/`set_jump_list`) methods, their `QuiAppActions` mirrors, and
the three module-level, app-less functions (`quadraui.app_path`,
`quadraui.set_app_identifier`, `quadraui.install_crash_handler`).

Every `QuiApp` method here resolves `quadraui::PlatformServices` off this
app's own headless `TuiBackend`. `TuiBackend` implements only
`begin_power_save_block`/`end_power_save_block` (a genuine, if no-op,
bookkeeping inhibitor) and `login_item_enabled` (always reports `False`,
truthfully, since this backend itself never registers a login item) for
real; every other method here falls through to `BackendError::Unsupported`
on this backend, surfaced as `quadraui.UnsupportedError` rather than a panic
or a silent no-op -- this file's main acceptance case.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_platform_parity.py
"""

import pytest

from quadraui import InvalidDescriptorError, QuiApp, UnsupportedError

# ─── Auto-update ────────────────────────────────────────────────────────────


def test_check_for_update_raises_unsupported_on_tui():
    app = QuiApp(10, 10)
    with pytest.raises(UnsupportedError):
        app.check_for_update("https://example.test/appcast.xml")


def test_download_and_stage_raises_unsupported_on_tui():
    app = QuiApp(10, 10)
    info = {
        "version": "1.2.3",
        "notes": "notes",
        "url": "https://example.test/v1.2.3",
        "signature": "sig",
    }
    with pytest.raises(UnsupportedError):
        app.download_and_stage(info)


def test_apply_on_quit_rejects_an_unknown_handle():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.apply_on_quit(999999)


# ─── Power-save blocker ─────────────────────────────────────────────────────
#
# The one real (non-degraded) `PlatformServices` implementation covered in
# this file: `TuiBackend` genuinely tracks an inhibitor handle, it just
# never calls out to the OS to back it with anything.


def test_power_save_block_begin_and_end_round_trip():
    app = QuiApp(10, 10)

    handle = app.begin_power_save_block("prevent_display_sleep")

    app.end_power_save_block(handle)


def test_begin_power_save_block_rejects_an_unknown_kind():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.begin_power_save_block("not_a_real_kind")


def test_end_power_save_block_rejects_an_unknown_handle():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.end_power_save_block(999999)


# ─── Launch at login ────────────────────────────────────────────────────────


def test_login_item_enabled_reports_false_on_tui():
    app = QuiApp(10, 10)
    assert app.login_item_enabled() is False


def test_set_login_item_raises_unsupported_on_tui():
    app = QuiApp(10, 10)
    with pytest.raises(UnsupportedError):
        app.set_login_item(True)


# ─── Protocol registration ──────────────────────────────────────────────────


def test_protocol_client_methods_raise_unsupported_on_tui():
    app = QuiApp(10, 10)

    with pytest.raises(UnsupportedError):
        app.set_as_default_protocol_client("myapp")

    with pytest.raises(UnsupportedError):
        app.is_default_protocol_client("myapp")

    with pytest.raises(UnsupportedError):
        app.remove_as_default_protocol_client("myapp")


# ─── Drag-out ────────────────────────────────────────────────────────────────


def test_begin_drag_out_raises_unsupported_on_tui():
    app = QuiApp(10, 10)
    with pytest.raises(UnsupportedError):
        app.begin_drag_out(files=["/tmp/a.txt"])


# ─── Recent docs / dock menu / jump list ─────────────────────────────────────


def test_add_recent_document_raises_unsupported_on_tui():
    app = QuiApp(10, 10)
    with pytest.raises(UnsupportedError):
        app.add_recent_document("/tmp/a.txt")


def test_clear_recent_documents_raises_unsupported_on_tui():
    app = QuiApp(10, 10)
    with pytest.raises(UnsupportedError):
        app.clear_recent_documents()


def test_set_dock_menu_raises_unsupported_on_tui():
    app = QuiApp(10, 10)
    with pytest.raises(UnsupportedError):
        app.set_dock_menu([{"label": {"spans": []}}])


def test_set_dock_menu_rejects_a_malformed_items_value():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.set_dock_menu("not a list")


def test_set_jump_list_raises_unsupported_on_tui():
    app = QuiApp(10, 10)
    with pytest.raises(UnsupportedError):
        app.set_jump_list([{"title": "Recent file"}])


def test_set_jump_list_rejects_a_malformed_items_value():
    app = QuiApp(10, 10)
    with pytest.raises(InvalidDescriptorError):
        app.set_jump_list("not a list")


# ─── App standard paths / app identifier / crash handler (module-level) ────


def test_app_path_returns_a_real_path():
    import quadraui

    assert quadraui.app_path("temp")


def test_app_path_rejects_an_unknown_kind():
    import quadraui

    with pytest.raises(InvalidDescriptorError):
        quadraui.app_path("not_a_real_kind")


def test_set_app_identifier_changes_app_scoped_paths():
    import quadraui

    quadraui.set_app_identifier("quadraui-py-parity-test")
    path = quadraui.app_path("app_config")

    assert "quadraui-py-parity-test" in path


def test_install_crash_handler_accepts_a_config(tmp_path):
    import quadraui

    quadraui.install_crash_handler({"log_dir": str(tmp_path)})


def test_install_crash_handler_rejects_a_malformed_config():
    import quadraui

    with pytest.raises(InvalidDescriptorError):
        quadraui.install_crash_handler("not a dict")


# ─── QuiAppActions mirrors ───────────────────────────────────────────────────
#
# Every void-returning method above has a matching `QuiAppActions` method,
# queued from inside a handler and applied right after that dispatch
# finishes -- see `test_deferred_actions.py`'s module doc for why that
# matters. Each of these only proves the deferred path is reachable and
# eventually raises (recorded as `app.run`'s own `pending_error`) the same
# `UnsupportedError`/`InvalidDescriptorError` the matching `QuiApp` method
# would raise synchronously.


def test_actions_apply_on_quit_surfaces_its_error_after_dispatch():
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.apply_on_quit(999999)

    app.set_handler(on_event)
    app.push_key_event(ord("a"))

    with pytest.raises(InvalidDescriptorError):
        app.run(1)


def test_actions_end_power_save_block_surfaces_its_error_after_dispatch():
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.end_power_save_block(999999)

    app.set_handler(on_event)
    app.push_key_event(ord("a"))

    with pytest.raises(InvalidDescriptorError):
        app.run(1)


def test_actions_set_login_item_surfaces_unsupported_after_dispatch():
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.set_login_item(True)

    app.set_handler(on_event)
    app.push_key_event(ord("a"))

    with pytest.raises(UnsupportedError):
        app.run(1)


def test_actions_set_dock_menu_surfaces_unsupported_after_dispatch():
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.set_dock_menu([{"label": {"spans": []}}])

    app.set_handler(on_event)
    app.push_key_event(ord("a"))

    with pytest.raises(UnsupportedError):
        app.run(1)


def test_actions_set_dock_menu_surfaces_a_malformed_items_value_after_dispatch():
    # Unlike `set_tray_menu`, `QuiAppActions.set_dock_menu` only converts
    # `items` to JSON at queue time (a bare string is already valid JSON);
    # the actual item-list shape is decoded by
    # `quadraui_binding_core::set_dock_menu` when `flush_deferred_actions`
    # applies it -- see `DeferredAction`'s own doc for why these ten
    # platform-services variants validate at flush time rather than queue
    # time, the same `Theme` exception already carves out.
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.set_dock_menu("not a list")

    app.set_handler(on_event)
    app.push_key_event(ord("a"))

    with pytest.raises(InvalidDescriptorError):
        app.run(1)


def test_actions_begin_drag_out_surfaces_unsupported_after_dispatch():
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.begin_drag_out(files=["/tmp/a.txt"])

    app.set_handler(on_event)
    app.push_key_event(ord("a"))

    with pytest.raises(UnsupportedError):
        app.run(1)


def test_actions_clear_recent_documents_surfaces_unsupported_after_dispatch():
    app = QuiApp(10, 10)
    actions = app.actions()

    def on_event(_event):
        actions.clear_recent_documents()

    app.set_handler(on_event)
    app.push_key_event(ord("a"))

    with pytest.raises(UnsupportedError):
        app.run(1)
