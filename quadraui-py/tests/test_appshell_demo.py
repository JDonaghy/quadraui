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
# timing-precision assertion.
STEP_BUDGET_SECONDS = 10.0


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
    app, shell, offsets = appshell_demo.run_demo(steps=steps)
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
    app, shell, offsets = appshell_demo.run_demo(steps=steps)
    elapsed = time.monotonic() - start

    last_row = len(shell.rows) - 1
    assert offsets[-1] == last_row
    assert shell.selected_idx == last_row
    assert app.screen_contains(f"Row {last_row:05d}")
    assert elapsed < STEP_BUDGET_SECONDS
