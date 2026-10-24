"""Black-box coverage for `QuiApp.run_tui()`: takes over the controlling
terminal via a real `quadraui::tui::TuiRunner`, as opposed to the headless
`step()` / `screen()` path the rest of this package's tests exercise.

Each test shells out to a tiny standalone script instead of calling
`run_tui()` in-process, so one test's terminal takeover attempt can never
leak into the next.

`run_tui` is always compiled in (no Cargo feature gates it -- the `tui`
feature quadraui-py depends on unconditionally already compiles in both
the headless `TuiDriver` and the real `TuiRunner`), so unlike
`test_run_native.py` these tests are not skipped based on `hasattr`.

Deterministic coverage here is deliberately narrow: `TuiRunner::new`'s
very first step is `enable_raw_mode()`, which fails fast with a plain
`io::Error` (not a panic, not a hang) whenever stdin/stdout isn't a real
tty -- exactly the case for a subprocess run with its stdin piped or
redirected from `/dev/null`, as every test here does. This is the only
`run_tui` failure mode that is both fast and host-independent; actually
driving a live keypress through a real tty would need a full terminal
emulator responding to crossterm's own terminal queries (e.g. the cursor
position report `ESC[6n`), which a bare pty slave does not do on its own,
so that path is left to a human running the terminal demo in
`examples/tui_interactive_demo.py` instead.
"""

import subprocess
import sys

import pytest

from quadraui import InternalError, QuiApp

# `run_tui` is always present -- confirm the binding itself didn't regress
# to being feature-gated before relying on that in every other test here.
assert hasattr(QuiApp, "run_tui")


def _run_script(tmp_path, name, body):
    script = tmp_path / name
    script.write_text(body)
    try:
        return subprocess.run(
            [sys.executable, str(script)],
            capture_output=True,
            text=True,
            timeout=30,
            stdin=subprocess.DEVNULL,
        )
    except subprocess.TimeoutExpired as exc:  # pragma: no cover - environment-dependent
        pytest.fail(f"run_tui script did not exit: {exc!r}")


def test_run_tui_raises_internal_error_without_a_real_tty(tmp_path):
    body = (
        "from quadraui import QuiApp, InternalError\n"
        "app = QuiApp(40, 20)\n"
        "try:\n"
        "    app.run_tui()\n"
        '    print("RUN_TUI_DID_NOT_RAISE")\n'
        "except InternalError as exc:\n"
        '    print(f"RUN_TUI_RAISED_INTERNAL:{exc}")\n'
    )
    result = _run_script(tmp_path, "run_tui_no_tty.py", body)

    assert result.returncode == 0, (
        f"stdout={result.stdout!r} stderr={result.stderr!r}"
    )
    assert "RUN_TUI_RAISED_INTERNAL:run_tui:" in result.stdout, result.stdout


def test_run_tui_does_not_consume_the_headless_app_on_failure(tmp_path):
    # `run_tui` moves this app's `PyAppLogic` out of the headless driver (the
    # same `mem::take` `run_native` uses) before attempting the takeover, so
    # even when the takeover itself fails fast, the headless driver must be
    # left in a usable (if emptied) state afterwards -- `screen()` should
    # still work rather than panicking or poisoning the app.
    body = (
        "from quadraui import QuiApp, InternalError\n"
        "app = QuiApp(40, 20)\n"
        'app.insert_button({"id": "demo:ok", "label": "OK", "focused": True})\n'
        "try:\n"
        "    app.run_tui()\n"
        "except InternalError:\n"
        "    pass\n"
        "app.screen()\n"
        'print("SCREEN_STILL_WORKS")\n'
    )
    result = _run_script(tmp_path, "run_tui_recovers.py", body)

    assert result.returncode == 0, (
        f"stdout={result.stdout!r} stderr={result.stderr!r}"
    )
    assert "SCREEN_STILL_WORKS" in result.stdout, result.stdout
