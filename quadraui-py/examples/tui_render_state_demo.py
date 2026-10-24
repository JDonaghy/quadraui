"""Shows the `render` callback's reason for existing: a `set_handler`
callback runs while the run loop holds the one mutable borrow of this
app's `QuiApp` for the whole step, so it cannot call back into
`insert_list`/`slot_clear` on that same instance to show the new
selection -- the way `appshell_demo.py` drives its scroll loop from
outside `run()`/`step()` entirely, never from a bound handler.

The `render` callback is the way around that. It takes no arguments and
is called once per frame -- the same frame the runner already repaints
after every dispatched event -- and returns a `dict` mapping slot names to
descriptors. Here, a plain Python `dict` (`_state`, never touching
`QuiApp`) holds the one thing that changes, `selected_idx`; the event
handler mutates it directly on every `Down`/`Up` keystroke, and the
render callback rebuilds the `list` descriptor from it from scratch every
frame. Neither callback calls back into `QuiApp` -- the handler only
mutates `_state` and returns, the render callback only reads `_state` and
returns a `dict`.

Run headless:

    maturin develop
    python quadraui-py/examples/tui_render_state_demo.py
"""

from quadraui import QuiApp

WIDTH = 20
HEIGHT = 10
# Fits entirely within the `list` slot's fixed-height band with no
# scrolling, so every row stays visible across the whole demo -- scroll
# management is its own concern (`appshell_demo.py` owns it explicitly) and
# orthogonal to what this demo is about.
ROW_COUNT = 4

_DOWN_KEY = {"Named": "Down"}
_UP_KEY = {"Named": "Up"}
_NO_MODIFIERS = {"shift": False, "ctrl": False, "alt": False, "meta": False}


def _key_event(key):
    return {
        "KeyPressed": {
            "key": key,
            "modifiers": _NO_MODIFIERS,
            "repeat": False,
        }
    }


def make_rows(count):
    return [
        {"text": {"spans": [{"text": f"Row {i}", "fg": None, "bg": None}]}}
        for i in range(count)
    ]


class RenderStateDemo:
    """Owns the plain-Python `state` dict a `render` callback reads and a
    `set_handler` handler mutates -- the two callbacks never touch each
    other or `QuiApp` directly, only this shared `state`."""

    def __init__(self, rows):
        self.rows = rows
        self.state = {"selected_idx": 0}
        self.app = QuiApp(WIDTH, HEIGHT, render=self._render)
        self.app.set_handler(self._handle)

    def _render(self):
        return {
            "list": {
                "id": "list:render_state_demo",
                "items": self.rows,
                "selected_idx": self.state["selected_idx"],
                "scroll_offset": 0,
                "has_focus": True,
                "show_v_scrollbar": False,
            }
        }

    def _handle(self, event):
        if not (isinstance(event, dict) and "KeyPressed" in event):
            return
        key = event["KeyPressed"]["key"]
        last = len(self.rows) - 1
        if key == _DOWN_KEY:
            self.state["selected_idx"] = min(last, self.state["selected_idx"] + 1)
        elif key == _UP_KEY:
            self.state["selected_idx"] = max(0, self.state["selected_idx"] - 1)

    def press_down(self):
        self.app.push_event(_key_event(_DOWN_KEY))
        self.app.step()

    def press_up(self):
        self.app.push_event(_key_event(_UP_KEY))
        self.app.step()


def run_demo():
    """Builds the demo, presses Down `ROW_COUNT - 1` times, and returns
    `(demo, selections)` -- `selections[i]` is `state["selected_idx"]`
    after the i-th Down press."""
    demo = RenderStateDemo(make_rows(ROW_COUNT))
    selections = []
    for _ in range(ROW_COUNT - 1):
        demo.press_down()
        selections.append(demo.state["selected_idx"])
    return demo, selections


if __name__ == "__main__":
    demo, selections = run_demo()
    print(demo.app.screen())
    print(f"selected_idx after each Down press: {selections}")
