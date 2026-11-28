"""Checks an installed Linux ``quadraui`` wheel against what the host should get.

Run by ``.github/workflows/quadraui-py-wheels.yml``'s ``linux-smoke`` job
inside clean distro containers, after ``pip install``-ing from a directory
that holds both Linux wheels (the TUI-only ``manylinux_2_17`` one and the
GTK-enabled ``manylinux_2_39`` one), so pip's own tag selection is part of
what gets checked. Exits non-zero on the first mismatch.

    python linux_wheel_smoke.py gtk           # GTK 4 installed, glibc >= 2.39
    python linux_wheel_smoke.py missing-gtk   # glibc >= 2.39, no GTK 4
    python linux_wheel_smoke.py tui-only      # glibc < 2.39

``gtk`` needs a reachable display (the workflow starts ``Xvfb``) and opens a
real window through ``run_native``, closing it with ``GtkRunner``'s own
``QUADRAUI_GTK_SMOKE_MS`` timer.
"""

import os
import subprocess
import sys
import textwrap

import quadraui


def check(condition: bool, message: str) -> None:
    if not condition:
        print(f"FAIL: {message}", file=sys.stderr)
        sys.exit(1)
    print(f"ok: {message}")


def headless_renders() -> None:
    app = quadraui.QuiApp(40, 5)
    app.set_render(lambda: {"button": {"id": "smoke:ok", "label": "OK"}})
    check(app.screen_contains("OK"), "headless TUI driver renders a button")


def run_tui_reaches_terminal_runner() -> None:
    # No controlling terminal in a CI container, so `run_tui` gets as far as
    # crossterm's raw-mode setup and raises `InternalError` there: proof the
    # terminal runner is compiled in and callable, without needing a pty that
    # answers crossterm's terminal queries.
    body = textwrap.dedent(
        """
        from quadraui import QuiApp, InternalError
        app = QuiApp(40, 5)
        app.set_render(lambda: {"button": {"id": "smoke:ok", "label": "OK"}})
        try:
            app.run_tui()
        except InternalError:
            print("RUN_TUI_REACHED_RUNNER")
        """
    )
    result = subprocess.run(
        [sys.executable, "-c", body],
        capture_output=True,
        text=True,
        timeout=60,
        stdin=subprocess.DEVNULL,
        start_new_session=True,
    )
    check(
        "RUN_TUI_REACHED_RUNNER" in result.stdout,
        f"run_tui reaches the terminal runner (stdout={result.stdout!r} stderr={result.stderr!r})",
    )


def run_native_raises(needle: str) -> None:
    try:
        quadraui.QuiApp(40, 5).run_native()
    except quadraui.UnsupportedError as exc:
        print(f"run_native: {exc}")
        check(needle in str(exc), f"run_native's UnsupportedError names {needle!r}")
        return
    check(False, "run_native raises UnsupportedError")


def run_native_paints() -> None:
    os.environ["QUADRAUI_GTK_SMOKE_MS"] = "1500"
    renders: list[int] = []

    def render() -> dict[str, object]:
        renders.append(1)
        return {"button": {"id": "smoke:ok", "label": "OK", "focused": True}}

    app = quadraui.QuiApp(40, 20)
    app.set_render(render)
    app.set_handler(lambda event: None)
    before = len(renders)
    app.run_native(title="quadraui Linux wheel smoke")
    check(len(renders) > before, f"run_native painted {len(renders) - before} frame(s) in a GTK window")


def main(mode: str) -> None:
    ext = quadraui._quadraui
    print(f"quadraui {quadraui.__version__} from {os.path.dirname(quadraui.__file__)}")
    print(f"extension module: {ext.__name__}, native backend: {ext._NATIVE_BACKEND}")
    package_dir = os.path.dirname(quadraui.__file__)
    ships_gtk = any(name.startswith("_quadraui_gtk.") for name in os.listdir(package_dir))

    if mode == "gtk":
        check(ships_gtk, "pip picked the GTK-enabled wheel")
        check(ext.__name__ == "quadraui._quadraui_gtk", "the GTK extension variant loaded")
        check(ext._NATIVE_BACKEND == "gtk", "native backend is gtk")
        headless_renders()
        run_tui_reaches_terminal_runner()
        run_native_paints()
    elif mode == "missing-gtk":
        check(ships_gtk, "pip picked the GTK-enabled wheel")
        check(ext.__name__ == "quadraui._quadraui", "fell back to the GTK-free extension")
        check(ext._NATIVE_BACKEND is None, "no native backend")
        headless_renders()
        run_tui_reaches_terminal_runner()
        run_native_raises("libgtk-4.so.1")
    elif mode == "tui-only":
        check(not ships_gtk, "pip picked the TUI-only wheel")
        check(ext._NATIVE_BACKEND is None, "no native backend")
        headless_renders()
        run_tui_reaches_terminal_runner()
        run_native_raises("glibc 2.39")
    else:
        sys.exit(f"unknown mode {mode!r}")


if __name__ == "__main__":
    main(sys.argv[1])
