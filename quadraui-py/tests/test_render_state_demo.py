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


def test_initial_screen_highlights_the_first_row():
    render_state_demo = _load_render_state_demo()
    demo = render_state_demo.RenderStateDemo(render_state_demo.make_rows(5))
    assert demo.app.screen_contains("Row 0")
    screen = demo.app.screen()
    first_line = screen.splitlines()[0]
    assert first_line.startswith("▶")


def test_pressing_down_moves_the_highlight_to_the_new_row():
    render_state_demo = _load_render_state_demo()
    demo = render_state_demo.RenderStateDemo(render_state_demo.make_rows(5))

    demo.press_down()

    assert demo.state["selected_idx"] == 1
    screen = demo.app.screen()
    lines = screen.splitlines()
    assert lines[0].startswith("▶") is False
    assert lines[1].startswith("▶")
    assert "Row 1" in lines[1]


def test_running_the_full_demo_advances_selection_every_step():
    render_state_demo = _load_render_state_demo()
    demo, selections = render_state_demo.run_demo()

    assert selections == list(range(1, render_state_demo.ROW_COUNT))
    assert demo.app.screen_contains(f"Row {render_state_demo.ROW_COUNT - 1}")


def test_render_callback_is_invoked_exactly_once_per_step():
    """`step()` dispatches one event and repaints once -- the render
    callback must run exactly once per `step()`, not zero times (state
    changes get lost) and not more than once (duplicate side effects for a
    callback that isn't pure)."""
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

    app.push_key_event(ord("a"))
    app.step()

    assert len(calls) == 1
