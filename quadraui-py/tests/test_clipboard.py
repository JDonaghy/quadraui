"""Black-box coverage for `QuiApp.clipboard_write_text` /
`clipboard_read_text`.

Unlike `run_tui()` (see `test_run_tui.py`), these calls never touch the
controlling terminal's raw mode, so they are safe to exercise in-process:
`quadraui::tui::services::TuiClipboard::write_text` only (1) best-effort
tries the local `arboard` clipboard, (2) emits an OSC 52 escape sequence to
stdout and, on Unix, `/dev/tty` (both skipped silently if unavailable), and
(3) best-effort spawns a native CLI tool (`wl-copy`/`xclip`/`xsel`) on a
detached background thread. None of those legs block or require a real
terminal, and all degrade silently on failure.

A fully headless CI runner without `arboard`/X11/Wayland support may not
actually retain what gets written, so the roundtrip assertion tolerates a
`None` read-back -- the behavioral guarantee under test is that both calls
complete without raising, not that every environment has a working system
clipboard.
"""

import pytest

from quadraui import InternalError, InvalidDimensionsError, QuiApp


def test_clipboard_bindings_are_present():
    app = QuiApp(40, 20)
    assert hasattr(app, "clipboard_write_text")
    assert hasattr(app, "clipboard_read_text")


def test_clipboard_write_text_does_not_raise():
    app = QuiApp(40, 20)
    app.clipboard_write_text("quadraui-py clipboard smoke test")


def test_clipboard_read_text_returns_str_or_none():
    app = QuiApp(40, 20)
    result = app.clipboard_read_text()
    assert result is None or isinstance(result, str)


def test_clipboard_write_then_read_roundtrip_or_none():
    app = QuiApp(40, 20)
    written = "quadraui-py clipboard roundtrip marker"
    app.clipboard_write_text(written)
    read_back = app.clipboard_read_text()
    # A working system clipboard (arboard, or the OSC 52 leg on a terminal
    # that echoes it back) returns exactly what was written; an
    # environment with no clipboard backend at all returns `None` instead
    # of raising -- both are acceptable here.
    assert read_back is None or read_back == written


def test_clipboard_backend_persists_across_calls():
    # The `clipboard` field on `QuiApp` is constructed once and reused
    # (see its doc comment in `src/lib.rs`): the arboard handle and any
    # clipboard-serving background thread must stay alive for the
    # lifetime of the app, not be torn down and rebuilt on every call.
    # Multiple calls in a row should not raise or panic.
    app = QuiApp(40, 20)
    app.clipboard_write_text("first")
    app.clipboard_write_text("second")
    result = app.clipboard_read_text()
    assert result is None or isinstance(result, str)


# ── Richer clipboard formats ─────────────────────────────────────────────
#
# `clipboard_backend` always wraps the headless `TuiBackend`'s
# `TuiClipboard`, which only overrides `Clipboard::read_text`/`write_text`
# (see `quadraui/src/tui/services.rs`). Every other `Clipboard` method —
# `read_image`/`write_image`/`write_html`/`read_file_list`/
# `read_primary_selection` — therefore takes its trait default here
# (`Err(Unsupported)` or `None`), deterministically, regardless of the host
# environment. These assertions are exact, not "or None" like the plain-text
# tests above, for exactly that reason.


def test_richer_clipboard_bindings_are_present():
    app = QuiApp(40, 20)
    assert hasattr(app, "clipboard_read_image")
    assert hasattr(app, "clipboard_write_image")
    assert hasattr(app, "clipboard_write_html")
    assert hasattr(app, "clipboard_read_file_list")
    assert hasattr(app, "clipboard_read_primary_selection")


def test_clipboard_read_image_is_none_on_a_text_only_backend():
    app = QuiApp(40, 20)
    assert app.clipboard_read_image() is None


def test_clipboard_write_image_raises_internal_error_on_unsupported_backend():
    app = QuiApp(40, 20)
    with pytest.raises(InternalError):
        app.clipboard_write_image(1, 1, b"\xff\x00\x00\xff")


def test_clipboard_write_image_rejects_a_mismatched_pixel_buffer():
    app = QuiApp(40, 20)
    with pytest.raises(InvalidDimensionsError):
        # 2x2 RGBA8 needs 16 bytes; this is deliberately short.
        app.clipboard_write_image(2, 2, b"\x00" * 4)


def test_clipboard_write_html_raises_internal_error_on_unsupported_backend():
    # `alt_text` exists so a text-only clipboard COULD degrade to writing
    # it via `write_text` instead of failing outright -- quadraui's "TUI is
    # a first-class backend" rule points that way. That degrade belongs in
    # `quadraui::tui::services::TuiClipboard` itself, not here; this test
    # locks in today's actual behavior (the trait's `Unsupported` default)
    # and will need rewriting once TUI grows that fallback.
    app = QuiApp(40, 20)
    with pytest.raises(InternalError):
        app.clipboard_write_html("<b>hi</b>", "hi")


def test_clipboard_read_file_list_is_none_on_a_text_only_backend():
    app = QuiApp(40, 20)
    assert app.clipboard_read_file_list() is None


def test_clipboard_read_primary_selection_is_none_on_a_text_only_backend():
    app = QuiApp(40, 20)
    assert app.clipboard_read_primary_selection() is None
