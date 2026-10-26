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
very first step is `enable_raw_mode()`, which on Unix opens the fd it
operates on via crossterm's `tty_fd()` (`terminal/sys/file_descriptor.rs`
in the `crossterm` crate) -- `STDIN_FILENO` when it `isatty()`, falling
back to `open("/dev/tty")` otherwise. A piped/`/dev/null` stdin alone does
NOT make that fallback fail: `open("/dev/tty")` still succeeds and hands
back the *same* controlling terminal the parent shell owns, so without
extra care a subprocess run from an interactive shell would seize and
raw-mode the developer's real terminal instead of failing fast. The fix
is to drop the controlling terminal entirely before `exec`: `_run_script`
passes `start_new_session=True`, which (POSIX `setsid()`) detaches the
child into its own session with no controlling terminal at all, so
`open("/dev/tty")` fails with `ENXIO` and `enable_raw_mode()` fails fast
with a plain `io::Error` -- deterministic and host-independent, since it
no longer depends on whether a controlling terminal happens to be present.
Actually driving a live keypress through a real tty would need a full
terminal emulator responding to crossterm's own terminal queries (e.g.
the cursor position report `ESC[6n`), which a bare pty slave does not do
on its own, so that path is left to a human running the terminal demo in
`examples/tui_interactive_demo.py` instead.
"""

import subprocess
import sys

import pytest

from quadraui import QuiApp


def test_run_tui_binding_is_present():
    # `run_tui` is always compiled in -- confirm the binding itself didn't
    # regress to being feature-gated before relying on that in every other
    # test here. A real test function (rather than a bare module-level
    # `assert`) turns a regression into a reported failure instead of a
    # silent collection error, and still runs under `python -O`.
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
            # Detach from the controlling terminal (POSIX `setsid`) so
            # crossterm's `open("/dev/tty")` fallback has nothing to open.
            # Without this, a piped/`/dev/null` stdin does not stop
            # `enable_raw_mode()` from reaching for -- and raw-moding --
            # whatever real terminal invoked this test.
            start_new_session=True,
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


def test_run_tui_accepts_mouse_false(tmp_path):
    # `mouse=False` selects `quadraui::tui::RunConfig::no_mouse()` instead
    # of the default config, which only changes what gets negotiated with
    # the terminal *after* `enable_raw_mode()` already succeeded -- so
    # without a real tty this still fails at the same first step, with the
    # same `InternalError`. That is enough to prove the keyword argument is
    # accepted and plumbed through to `TuiRunner::new_with` without
    # panicking or raising `TypeError` on the call itself.
    body = (
        "from quadraui import QuiApp, InternalError\n"
        "app = QuiApp(40, 20)\n"
        "try:\n"
        "    app.run_tui(mouse=False)\n"
        '    print("RUN_TUI_DID_NOT_RAISE")\n'
        "except InternalError as exc:\n"
        '    print(f"RUN_TUI_RAISED_INTERNAL:{exc}")\n'
    )
    result = _run_script(tmp_path, "run_tui_no_mouse.py", body)

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
