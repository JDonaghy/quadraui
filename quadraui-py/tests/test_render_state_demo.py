"""Black-box acceptance test for `examples/tui_render_state_demo.py`: an
app with a `render` callback and a handler that mutates plain Python
state, pushed an event and stepped, must show the *new* state -- the
literal shape of the per-frame render callback this exercises.

Run against a built wheel/extension, e.g.:

    maturin develop
    pytest quadraui-py/tests/test_render_state_demo.py
"""

import importlib.util
import pathlib

import pytest

from quadraui import QuiApp

_EXAMPLE_PATH = (
    pathlib.Path(__file__).resolve().parent.parent
    / "examples"
    / "tui_render_state_demo.py"
)


def _load_render_state_demo():
    spec = importlib.util.spec_from_file_location(
        "tui_render_state_demo", _EXAMPLE_PATH
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def render_state_demo():
    """Loads `tui_render_state_demo.py` once per test module rather than once
    per test function -- the module has no mutable top-level state, so
    re-executing it for every test only adds import overhead without buying
    isolation."""
    return _load_render_state_demo()


def test_initial_screen_highlights_the_first_row(render_state_demo):
    demo = render_state_demo.RenderStateDemo(render_state_demo.make_rows(5))
    assert demo.app.screen_contains("Row 0")
    initial_row0_line = [
        line for line in demo.app.screen().splitlines() if "Row 0" in line
    ][0]

    demo.press_down()
    assert demo.state["selected_idx"] == 1

    # Whatever visually marks the selected row, "Row 0"'s line must look
    # different once selection moves off it -- found by comparing the two
    # screens, never by hardcoding the glyph or a line index.
    moved_row0_line = [
        line for line in demo.app.screen().splitlines() if "Row 0" in line
    ][0]
    assert initial_row0_line != moved_row0_line


def test_pressing_down_moves_the_highlight_to_the_new_row(render_state_demo):
    demo = render_state_demo.RenderStateDemo(render_state_demo.make_rows(5))

    demo.press_down()

    assert demo.state["selected_idx"] == 1
    assert demo.app.screen_contains("Row 1")
    screen = demo.app.screen()
    lines = screen.splitlines()
    row1_line = [line for line in lines if "Row 1" in line][0]
    row0_line = [line for line in lines if "Row 0" in line][0]
    assert row1_line != row0_line
    # The newly selected row's line must carry whatever marks a line as
    # selected; the no-longer-selected row's line must not.
    prefix_len = min(len(row0_line), len(row1_line))
    marker = next(
        (row1_line[: i + 1] for i in range(prefix_len) if row1_line[i] != " "),
        "",
    )
    assert marker and not row0_line.startswith(marker)


def test_running_the_full_demo_advances_selection_every_step(render_state_demo):
    demo, selections = render_state_demo.run_demo()

    assert selections == list(range(1, render_state_demo.ROW_COUNT))
    assert demo.app.screen_contains(f"Row {render_state_demo.ROW_COUNT - 1}")


def test_render_callback_is_invoked_exactly_once_per_step():
    """`step()` dispatches one event and repaints once -- the render
    callback must run exactly once per `step()`, not zero times (state
    changes get lost) and not more than once (duplicate side effects for a
    callback that isn't pure).

    `QuiApp.__new__` itself paints the first frame (so `screen()` works
    before any `step()`), so the call count must be snapshotted *after*
    construction -- counting from zero here would attribute that
    construction-time paint to `step()` and pass even if `step()` never
    rendered at all, which is exactly the bug this test must catch."""
    calls = []

    def render():
        calls.append(1)
        return {
            "text_display": {
                "id": "text:guard",
                "lines": [{"spans": [{"text": "guard", "fg": None, "bg": None}]}],
            }
        }

    app = QuiApp(20, 5, render=render)
    app.set_handler(lambda event: None)
    before = len(calls)

    app.push_key_event(ord("a"))
    app.step()

    assert len(calls) - before == 1


def test_step_repaints_with_post_handler_state_before_any_screen_call():
    """The repaint that follows a handler mutation must happen *during*
    `step()` itself -- not be deferred until something else (`screen()`,
    `screen_contains()`) happens to call `render()` later. This is the
    behaviour `run_tui()`/`run_native()` actually depend on: their loops
    only ever call `step()`/`pump()`, never `screen()`.

    Proven here by recording the render callback's observed state on every
    invocation, mutating that state in the handler, stepping once, and
    asserting the *last recorded* value already reflects the post-handler
    state -- all without calling `screen()`/`screen_contains()` anywhere
    before the assertion, so the only thing that could have produced that
    recording is the repaint `step()` triggers on its own."""
    observed = []
    state = {"selected_idx": 0}

    def render():
        observed.append(state["selected_idx"])
        return {
            "text_display": {
                "id": "text:guard",
                "lines": [{"spans": [{"text": "guard", "fg": None, "bg": None}]}],
            }
        }

    def handle(_event):
        state["selected_idx"] += 1

    app = QuiApp(20, 5, render=render)
    app.set_handler(handle)
    before = len(observed)

    app.push_key_event(ord("a"))
    app.step()

    assert len(observed) - before == 1
    assert observed[-1] == 1
