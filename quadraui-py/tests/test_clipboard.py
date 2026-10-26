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

from quadraui import QuiApp


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
