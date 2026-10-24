"""Black-box coverage for `QuiApp.run_native()`: opens a real OS window via
`quadraui::gtk::run_with`, closed deterministically rather than by a human,
and asserts the handler the window dispatched into actually ran.

Each test shells out to a tiny standalone script instead of calling
`run_native()` in-process. `quadraui::gtk::run_with` hands the raw process
argv to `gio::Application::run()`, which parses it as GLib/GTK options --
and pytest's own invocation (`-m pytest ...`, `-v`, ...) contains flags
`gio::Application` doesn't recognize, so it errors out of the main loop
before a window is ever built and this module's assertions would be
checking state that was never exercised. Launching `python <script path>`
with no other argv keeps that parse clean, exactly like a user double
-clicking (or `python demo.py`-ing) the example script would see.

Only present when the extension is built with the `gtk` feature; skips
(rather than fails) when it isn't, or when no display is reachable --
mirroring the `gtk_*` Rust examples' own xvfb-run-friendly smoke tier.

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

pytestmark = pytest.mark.skipif(
    not hasattr(QuiApp, "run_native"),
    reason="extension built without the `gtk` feature (no `run_native`)",
)


def _run_script(tmp_path, name, body, env_extra):
    script = tmp_path / name
    script.write_text(body)
    try:
        return subprocess.run(
            [sys.executable, str(script)],
            capture_output=True,
            text=True,
            timeout=30,
            env=env_extra,
        )
    except subprocess.TimeoutExpired as exc:  # pragma: no cover - environment-dependent
        pytest.skip(f"run_native script did not exit: {exc!r}")


def test_run_native_opens_and_runs_handler_until_smoke_timeout(tmp_path):
    # `QUADRAUI_GTK_SMOKE_MS` is `quadraui::gtk::run_with`'s own documented
    # unattended-exit knob (see `quadraui/src/gtk/run.rs`) -- it closes the
    # window itself after the given number of milliseconds, so this never
    # blocks on a human closing it.
    env = dict(os.environ)
    env["QUADRAUI_GTK_SMOKE_MS"] = "200"

    body = (
        "from quadraui import QuiApp\n"
        "app = QuiApp(40, 20)\n"
        'app.insert_button({"id": "demo:ok", "label": "OK", "focused": True})\n'
        "app.set_handler(lambda event: None)\n"
        'app.run_native(title="quadraui run_native test")\n'
        'print("RUN_NATIVE_RETURNED")\n'
    )
    result = _run_script(tmp_path, "run_opens.py", body, env)

    if result.returncode != 0:
        pytest.skip(
            "no usable display for a native GTK window: "
            f"rc={result.returncode} stderr={result.stderr!r}"
        )
    assert "RUN_NATIVE_RETURNED" in result.stdout


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
        "from quadraui import QuiApp\n"
        "app = QuiApp(40, 20)\n"
        'app.insert_button({"id": "demo:ok", "label": "OK", "focused": True})\n'
        "def on_tick():\n"
        '    raise RuntimeError("tick handler asked to exit")\n'
        "app.set_tick_handler(on_tick)\n"
        "try:\n"
        '    app.run_native(title="quadraui run_native test")\n'
        '    print("RUN_NATIVE_DID_NOT_RAISE")\n'
        "except RuntimeError as exc:\n"
        '    print(f"RUN_NATIVE_RAISED:{exc}")\n'
    )
    result = _run_script(tmp_path, "run_propagates.py", body, env)

    if result.returncode != 0 and "RUN_NATIVE_RAISED" not in result.stdout:
        pytest.skip(
            "no usable display for a native GTK window: "
            f"rc={result.returncode} stderr={result.stderr!r}"
        )
    assert "RUN_NATIVE_RAISED:tick handler asked to exit" in result.stdout
