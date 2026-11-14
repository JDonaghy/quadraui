"""Black-box acceptance test for `examples/appshell_demo.py`: drives the
10,000-row `AppShellDemo` headless and asserts it scrolls through every
step without dropping or duplicating one, within a bounded step budget.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_appshell_demo.py
"""

import importlib.util
import pathlib
import time

_EXAMPLE_PATH = (
    pathlib.Path(__file__).resolve().parent.parent / "examples" / "appshell_demo.py"
)

# Headless scrolling through the full demo is not expected to be fast in
# any absolute sense -- this is a guard against an accidental quadratic
# blowup (e.g. an `O(rows * steps)` descriptor rebuild), not a
# timing-precision assertion. `_scroll_and_render_each_step` -- not
# `run_demo` -- is what this budget actually times: `run_demo` only
# mutates `shell.scroll_offset`/`selected_idx` (plain Python ints) on
# every step and never renders until the test's own later
# `screen_contains` call, so timing `run_demo` itself would measure a
# bare int-arithmetic loop rather than the render cost this budget exists
# to bound.
STEP_BUDGET_SECONDS = 10.0


def _scroll_and_render_each_step(appshell_demo, steps):
    """Same stepping loop as `appshell_demo.run_demo`, but forces a render
    after every step (`app.screen()`) instead of once at the end -- so a
    render that becomes `O(rows)` per call shows up as `O(rows * steps)`
    total, the blowup `STEP_BUDGET_SECONDS` guards against."""
    app = appshell_demo.QuiApp(appshell_demo.WIDTH, appshell_demo.HEIGHT)
    shell = appshell_demo.AppShellDemo(app, appshell_demo.make_rows(appshell_demo.ROW_COUNT))
    offsets = [shell.scroll_offset]
    app.screen()
    for _ in range(steps):
        shell.scroll(appshell_demo.PAGE)
        offsets.append(shell.scroll_offset)
        app.screen()
    return app, shell, offsets


def _load_appshell_demo():
    spec = importlib.util.spec_from_file_location("appshell_demo", _EXAMPLE_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_initial_screen_shows_the_first_rows():
    appshell_demo = _load_appshell_demo()
    app, _shell, _offsets = appshell_demo.run_demo(steps=0)
    assert app.screen_contains("Row 00000")


def test_scrolling_advances_every_step_without_dropping_one():
    appshell_demo = _load_appshell_demo()
    steps = 20

    start = time.monotonic()
    app, shell, offsets = _scroll_and_render_each_step(appshell_demo, steps)
    elapsed = time.monotonic() - start

    # One recorded offset per step, plus the initial value.
    assert len(offsets) == steps + 1
    # Every step lands, in order, none skipped and none repeated: each
    # consecutive pair differs by exactly one page.
    deltas = {b - a for a, b in zip(offsets, offsets[1:])}
    assert deltas == {appshell_demo.PAGE}

    last_offset = offsets[-1]
    assert app.screen_contains(f"Row {last_offset:05d}")
    assert elapsed < STEP_BUDGET_SECONDS


def test_scrolling_reaches_and_clamps_at_the_last_row():
    appshell_demo = _load_appshell_demo()
    # Enough pages to run past the end of the 10k rows, to exercise the
    # clamp rather than stopping short of it.
    steps = (appshell_demo.ROW_COUNT // appshell_demo.PAGE) + 5

    start = time.monotonic()
    app, shell, offsets = _scroll_and_render_each_step(appshell_demo, steps)
    elapsed = time.monotonic() - start

    last_row = len(shell.rows) - 1
    assert offsets[-1] == last_row
    assert shell.selected_idx == last_row
    assert app.screen_contains(f"Row {last_row:05d}")
    assert elapsed < STEP_BUDGET_SECONDS
