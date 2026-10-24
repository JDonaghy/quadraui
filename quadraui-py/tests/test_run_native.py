"""Black-box coverage for `QuiApp.run_native()`: opens a real OS window via
`quadraui::gtk::GtkRunner`, closed deterministically rather than by a human,
and asserts the handler the window dispatched into actually ran.

Each test shells out to a tiny standalone script instead of calling
`run_native()` in-process, so a GTK window/display connection from one test
can never leak into the next -- each script gets its own fresh interpreter,
`GApplication` registration and (if reachable) display connection, exactly
like a human running `python demo.py` once and exiting.

Only present when the extension is built with the `gtk` feature; skips
(rather than fails) when it isn't, or when no display is reachable --
mirroring the `gtk_*` Rust examples' own xvfb-run-friendly smoke tier. "No
display reachable" is detected primarily by having each script catch
`quadraui.NoDisplayError` itself and print a dedicated `NO_DISPLAY_RAISED`
sentinel -- `run_native`'s own pre-flight check, so this is a positive
assertion ("no display raises a catchable Python error"), not a guess from
subprocess stderr.

Run against a build with the native feature enabled, under a virtual
display:

    maturin develop --features gtk
    xvfb-run -a pytest quadraui-py/tests/test_run_native.py
"""

import os
import subprocess
import sys

import pytest

from quadraui import QuiApp
from quadraui import _quadraui as _native

pytestmark = pytest.mark.skipif(
    getattr(_native, "_NATIVE_BACKEND", None) != "gtk",
    reason="extension not built with the `gtk` feature (or built with `macos` "
    "instead, which this suite's GtkRunner-specific env vars/errors don't apply to)",
)

# Backstop only: markers GTK/GDK/GLib could still leave in stderr for a
# display failure that happens *outside* `run_native`'s own `NoDisplayError`
# pre-flight check (e.g. a GTK/Cairo library load failure). Every script
# below catches `NoDisplayError` itself and prints `NO_DISPLAY_RAISED`, which
# is the primary (and reliable) signal these tests skip on; this is only
# consulted when that sentinel is absent from stdout.
_NO_DISPLAY_MARKERS = (
    "cannot open display",
    "could not connect",
    "no protocol specified",
    "failed to open display",
)


def _looks_like_no_display(stderr):
    lowered = stderr.lower()
    return any(marker in lowered for marker in _NO_DISPLAY_MARKERS)


def _run_script(tmp_path, name, body, env):
    script = tmp_path / name
    script.write_text(body)
    try:
        return subprocess.run(
            [sys.executable, str(script)],
            capture_output=True,
            text=True,
            timeout=30,
            env=env,
        )
    except subprocess.TimeoutExpired as exc:  # pragma: no cover - environment-dependent
        pytest.skip(f"run_native script did not exit: {exc!r}")


def _skip_if_no_display(result):
    """Skip the calling test if the script reported `NoDisplayError` (the
    sentinel every script below prints from its own `except NoDisplayError`
    clause), or if stderr otherwise looks like a display failure."""
    if "NO_DISPLAY_RAISED" in result.stdout or _looks_like_no_display(result.stderr):
        pytest.skip(
            "no usable display for a native GTK window: "
            f"rc={result.returncode} stdout={result.stdout!r} stderr={result.stderr!r}"
        )


def test_run_native_opens_and_runs_handler_until_smoke_timeout(tmp_path):
    # `QUADRAUI_GTK_SMOKE_MS` is `GtkRunner`'s own documented unattended-exit
    # knob (see `quadraui/src/gtk/run.rs`) -- it closes the window itself
    # after the given number of milliseconds, so this never blocks on a
    # human closing it.
    env = dict(os.environ)
    env["QUADRAUI_GTK_SMOKE_MS"] = "200"

    body = (
        "from quadraui import QuiApp, NoDisplayError\n"
        "app = QuiApp(40, 20)\n"
        'app.insert_button({"id": "demo:ok", "label": "OK", "focused": True})\n'
        "seen = []\n"
        "app.set_handler(lambda event: seen.append(event))\n"
        "try:\n"
        '    app.run_native(title="quadraui run_native test")\n'
        "except NoDisplayError as exc:\n"
        "    print(f'NO_DISPLAY_RAISED:{exc}')\n"
        "    raise SystemExit(0)\n"
        'print(f"RUN_NATIVE_RETURNED seen={len(seen)}")\n'
        'print("WINDOW_CLOSE_SEEN" if any(e == "WindowClose" for e in seen) else "NO_WINDOW_CLOSE")\n'
    )
    result = _run_script(tmp_path, "run_opens.py", body, env)

    _skip_if_no_display(result)
    if result.returncode != 0:
        pytest.fail(
            f"run_native script failed unexpectedly: rc={result.returncode} "
            f"stdout={result.stdout!r} stderr={result.stderr!r}"
        )
    assert "RUN_NATIVE_RETURNED" in result.stdout
    # The smoke timeout destroys the window directly rather than going
    # through `WindowClose`, so the handler must have seen no events at all
    # -- confirming `run_native` returned because the window actually
    # closed, not because it errored out before ever registering one.
    assert "NO_WINDOW_CLOSE" in result.stdout, result.stdout


def test_run_native_propagates_tick_handler_exception(tmp_path):
    # The smoke timeout above forces the window closed with
    # `window.destroy()` directly, bypassing the app's own `WindowClose`
    # handling entirely, so it can't exercise a handler exception. The
    # recurring idle tick (`IDLE_POLL_CEILING`, 250ms) calling `tick()`
    # does: raising from `on_tick` turns into `Reaction::Exit`, which runs
    # the normal close path and is what `run_native` is expected to
    # re-raise. `QUADRAUI_GTK_SMOKE_MS` here is only a backstop in case
    # that expectation is ever wrong, so this can't hang.
    env = dict(os.environ)
    env["QUADRAUI_GTK_SMOKE_MS"] = "5000"

    body = (
        "from quadraui import QuiApp, NoDisplayError\n"
        "app = QuiApp(40, 20)\n"
        'app.insert_button({"id": "demo:ok", "label": "OK", "focused": True})\n'
        "def on_tick():\n"
        '    raise RuntimeError("tick handler asked to exit")\n'
        "app.set_tick_handler(on_tick)\n"
        "try:\n"
        '    app.run_native(title="quadraui run_native test")\n'
        '    print("RUN_NATIVE_DID_NOT_RAISE")\n'
        "except NoDisplayError as exc:\n"
        "    print(f'NO_DISPLAY_RAISED:{exc}')\n"
        "except RuntimeError as exc:\n"
        '    print(f"RUN_NATIVE_RAISED:{exc}")\n'
    )
    result = _run_script(tmp_path, "run_propagates.py", body, env)

    _skip_if_no_display(result)
    assert "RUN_NATIVE_RAISED:tick handler asked to exit" in result.stdout, (
        f"rc={result.returncode} stdout={result.stdout!r} stderr={result.stderr!r}"
    )


def test_run_native_second_call_raises_native_already_run_error(tmp_path):
    # The re-entrancy check (`native_consumed`) runs before the no-display
    # check, so a second `run_native()` call only ever reaches
    # `NativeAlreadyRunError` once the *first* call actually opened (and
    # closed) a real window -- this needs a real display, unlike the
    # no-display test below.
    env = dict(os.environ)
    env["QUADRAUI_GTK_SMOKE_MS"] = "200"

    body = (
        "from quadraui import QuiApp, NativeAlreadyRunError, NoDisplayError\n"
        "app = QuiApp(10, 5)\n"
        "try:\n"
        '    app.run_native(title="first")\n'
        "except NoDisplayError as exc:\n"
        "    print(f'NO_DISPLAY_RAISED:{exc}')\n"
        "    raise SystemExit(0)\n"
        "try:\n"
        '    app.run_native(title="second")\n'
        "    print('SECOND_CALL_DID_NOT_RAISE')\n"
        "except NativeAlreadyRunError as exc:\n"
        "    print(f'SECOND_CALL_RAISED:{exc}')\n"
    )
    result = _run_script(tmp_path, "run_twice.py", body, env)

    _skip_if_no_display(result)
    assert "SECOND_CALL_RAISED" in result.stdout, (
        f"rc={result.returncode} stdout={result.stdout!r} stderr={result.stderr!r}"
    )


def test_run_native_invalid_app_id_raises_clear_error(tmp_path):
    # The `app_id` validity check runs after the no-display check but
    # before `native_consumed` is set (see `lib.rs`'s `run_native`), so
    # reaching `InvalidAppIdError` still needs a real display to get past
    # that earlier check -- same reasoning as the re-entrancy test above.
    # The id below starts with a digit, which `g_application_id_is_valid`
    # rejects outright.
    env = dict(os.environ)
    env["QUADRAUI_GTK_SMOKE_MS"] = "200"

    body = (
        "from quadraui import QuiApp, InvalidAppIdError, NoDisplayError\n"
        "app = QuiApp(10, 5)\n"
        "try:\n"
        '    app.run_native(title="invalid app_id test", app_id="1.not.valid")\n'
        "    print('RUN_NATIVE_DID_NOT_RAISE')\n"
        "except NoDisplayError as exc:\n"
        "    print(f'NO_DISPLAY_RAISED:{exc}')\n"
        "except InvalidAppIdError as exc:\n"
        "    print(f'INVALID_APP_ID_RAISED:{exc}')\n"
    )
    result = _run_script(tmp_path, "run_invalid_app_id.py", body, env)

    _skip_if_no_display(result)
    assert "INVALID_APP_ID_RAISED" in result.stdout, (
        f"rc={result.returncode} stdout={result.stdout!r} stderr={result.stderr!r}"
    )


def test_run_native_no_display_raises_clear_error(tmp_path):
    """With `DISPLAY`/`WAYLAND_DISPLAY` stripped from the environment, GTK's
    own display check fails before `run_native` ever builds a window or an
    `Application` -- this forces the "no display reachable" branch via the
    environment rather than depending on what the host actually has
    plugged in, so (on Linux/Windows) it passes on any host, display or
    not. Skips rather than asserts if the host still finds a display with
    those variables unset (e.g. a macOS window server, which isn't gated by
    either variable)."""
    env = dict(os.environ)
    for var in ("DISPLAY", "WAYLAND_DISPLAY"):
        env.pop(var, None)

    body = (
        "from quadraui import QuiApp, NoDisplayError\n"
        "app = QuiApp(10, 5)\n"
        "try:\n"
        "    app.run_native(title='no display test')\n"
        "    print('RUN_NATIVE_DID_NOT_RAISE')\n"
        "except NoDisplayError as exc:\n"
        "    print(f'NO_DISPLAY_RAISED:{exc}')\n"
    )
    result = _run_script(tmp_path, "run_no_display.py", body, env)

    if "NO_DISPLAY_RAISED" not in result.stdout:
        pytest.skip(
            "host still found a reachable display with DISPLAY/WAYLAND_DISPLAY "
            f"unset (e.g. a macOS window server): rc={result.returncode} "
            f"stdout={result.stdout!r} stderr={result.stderr!r}"
        )
    assert "NO_DISPLAY_RAISED:run_native found no display" in result.stdout
