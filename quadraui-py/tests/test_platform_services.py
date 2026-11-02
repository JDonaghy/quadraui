"""Black-box coverage for `QuiApp`'s platform-services surface:
`show_file_open_dialog`/`show_file_save_dialog`/`show_folder_dialog` (plus
the `queue_dialog_events` scripting hook they share), `system_theme`, and
`platform_name`.

Every one of these resolves `quadraui::PlatformServices` off this `QuiApp`'s
own headless `TuiBackend` (never a `run_native` window), so every assertion
below is reachable with no real terminal, no real desktop session, and no
native dialog -- the same "auto / headless oracle" story the issue tracking
this file explains. `send_notification`/`open_url`/`reveal_in_file_manager`/
`open_path`/`move_to_trash`/`beep` are fire-and-forget native effects with no
assertable return value; this file only proves they're callable and don't
raise on an obviously-benign input -- their actual visible effect needs a
human at a real terminal, noted in this PR's SMOKE_TESTS instead of asserted
here.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_platform_services.py
"""

import pytest

from quadraui import QuiApp


def _key_char(ch):
    return {
        "KeyPressed": {
            "key": {"Char": ch},
            "modifiers": {"shift": False, "ctrl": False, "alt": False, "cmd": False},
            "repeat": False,
        }
    }


def _key_enter():
    return {
        "KeyPressed": {
            "key": {"Named": "Enter"},
            "modifiers": {"shift": False, "ctrl": False, "alt": False, "cmd": False},
            "repeat": False,
        }
    }


def _key_escape():
    return {
        "KeyPressed": {
            "key": {"Named": "Escape"},
            "modifiers": {"shift": False, "ctrl": False, "alt": False, "cmd": False},
            "repeat": False,
        }
    }


# ─── Folder picker (the issue's named acceptance case) ───────────────────


def test_show_folder_dialog_confirms_a_selected_path(tmp_path):
    (tmp_path / "targetdir").mkdir()
    app = QuiApp(60, 20)

    # Filter down to the one subdirectory whose name contains "targ", then
    # confirm it -- same script shape as the Rust-side
    # `show_folder_open_dialog_confirms_a_selected_path` test this mirrors.
    app.queue_dialog_events(
        [_key_char("t"), _key_char("a"), _key_char("r"), _key_char("g"), _key_enter()]
    )

    result = app.show_folder_dialog(initial_dir=str(tmp_path))

    assert result == str(tmp_path / "targetdir")


def test_show_folder_dialog_cancelled_returns_none(tmp_path):
    (tmp_path / "targetdir").mkdir()
    app = QuiApp(60, 20)

    app.queue_dialog_events([_key_escape()])

    assert app.show_folder_dialog(initial_dir=str(tmp_path)) is None


def test_show_folder_dialog_with_nothing_queued_cancels_immediately(tmp_path):
    # No `queue_dialog_events` call at all -- the nested loop's very first
    # synthesized event is Escape, so this must resolve to `None` rather
    # than hang.
    app = QuiApp(10, 10)
    assert app.show_folder_dialog(initial_dir=str(tmp_path)) is None


# ─── File open/save pickers ───────────────────────────────────────────────


def test_show_file_open_dialog_confirms_a_selected_path(tmp_path):
    (tmp_path / "report.txt").write_text("hi")
    app = QuiApp(60, 20)

    app.queue_dialog_events(
        [
            _key_char("r"),
            _key_char("e"),
            _key_char("p"),
            _key_char("o"),
            _key_char("r"),
            _key_char("t"),
            _key_enter(),
        ]
    )

    result = app.show_file_open_dialog(initial_dir=str(tmp_path))

    assert result == str(tmp_path / "report.txt")


def test_show_file_save_dialog_seeds_the_initial_filename(tmp_path):
    app = QuiApp(60, 20)
    app.queue_dialog_events([_key_enter()])

    result = app.show_file_save_dialog(
        initial_dir=str(tmp_path), initial_filename="untitled.txt"
    )

    assert result == str(tmp_path / "untitled.txt")


# ─── System theme / platform identity ──────────────────────────────────────


def test_system_theme_reports_dark_from_colorfgbg(monkeypatch):
    monkeypatch.setenv("COLORFGBG", "15;0")
    app = QuiApp(10, 10)

    theme = app.system_theme()

    assert theme["dark"] is True
    assert theme["accent"] is None
    assert theme["high_contrast"] is False


def test_system_theme_reports_light_from_colorfgbg(monkeypatch):
    monkeypatch.setenv("COLORFGBG", "0;15")
    app = QuiApp(10, 10)

    theme = app.system_theme()

    assert theme["dark"] is False


def test_system_theme_raises_when_colorfgbg_is_unset(monkeypatch):
    monkeypatch.delenv("COLORFGBG", raising=False)
    app = QuiApp(10, 10)

    with pytest.raises(Exception):
        app.system_theme()


def test_platform_name_is_tui():
    app = QuiApp(10, 10)
    assert app.platform_name() == "tui"


# ─── Fire-and-forget native effects ────────────────────────────────────────
#
# No native desktop/terminal session is guaranteed under `pytest` -- these
# only prove the call is reachable and well-typed, not that anything
# actually appeared on screen. See this module's own doc and the owning
# PR's SMOKE_TESTS for the manual-verification story.


def test_send_notification_does_not_raise():
    app = QuiApp(10, 10)
    app.send_notification("Title", "Body")
    app.send_notification("Title", "Body", urgent=True, silent=True, tag="t1")


def test_beep_does_not_raise():
    app = QuiApp(10, 10)
    app.beep()


def test_move_to_trash_moves_a_real_file(tmp_path):
    victim = tmp_path / "doomed.txt"
    victim.write_text("bye")
    app = QuiApp(10, 10)

    app.move_to_trash(str(victim))

    assert not victim.exists()
