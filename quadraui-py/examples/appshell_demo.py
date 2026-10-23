"""AppShell-shaped demo: one `ListView` holding 10,000 rows, scrolled
smoothly from Python.

`QuiApp` has no in-place "update a slot" call -- a descriptor is immutable
once inserted, so advancing the scroll position means clearing the slot and
re-inserting the list with a new `scroll_offset`/`selected_idx` on every
"frame". `AppShellDemo` below owns that dance; the scroll loop itself is a
plain Python `for` loop driving it one page at a time, standing in for
whatever drives real key/scroll-wheel events (a bound handler must not call
back into the same `QuiApp` it was invoked from -- `QuiApp` rejects that
reentrant borrow -- so this demo calls `scroll()` directly rather than
through `set_handler`).

Run headless:

    maturin develop
    python quadraui-py/examples/appshell_demo.py
"""

from quadraui import QuiApp

WIDTH = 80
HEIGHT = 24
ROW_COUNT = 10_000
PAGE = 250


def make_rows(count):
    return [
        {"text": {"spans": [{"text": f"Row {i:05d}", "fg": None, "bg": None}]}}
        for i in range(count)
    ]


class AppShellDemo:
    """Drives the single `ListView` slot of a `QuiApp`, re-inserting it with
    an updated `scroll_offset`/`selected_idx` each time the list scrolls."""

    def __init__(self, app, rows):
        self.app = app
        self.rows = rows
        self.selected_idx = 0
        self.scroll_offset = 0
        self.slot = app.insert_list(self._descriptor())

    def _descriptor(self):
        return {
            "id": "list:appshell",
            "items": self.rows,
            "selected_idx": self.selected_idx,
            "scroll_offset": self.scroll_offset,
            "has_focus": True,
            "show_v_scrollbar": True,
        }

    def scroll(self, delta):
        last = len(self.rows) - 1
        self.selected_idx = max(0, min(last, self.selected_idx + delta))
        self.scroll_offset = max(0, min(last, self.scroll_offset + delta))
        self.app.slot_clear(self.slot)
        self.slot = self.app.insert_list(self._descriptor())


def run_demo(steps=20):
    """Build the demo app, scroll down `steps` times, and return
    `(app, shell, offsets)` -- `offsets[0]` is the initial `scroll_offset`,
    `offsets[i]` the value after the i-th scroll step."""
    app = QuiApp(WIDTH, HEIGHT)
    shell = AppShellDemo(app, make_rows(ROW_COUNT))

    offsets = [shell.scroll_offset]
    for _ in range(steps):
        shell.scroll(PAGE)
        offsets.append(shell.scroll_offset)

    return app, shell, offsets


if __name__ == "__main__":
    app, shell, offsets = run_demo()
    print(app.screen())
    print(f"scroll offsets after each step: {offsets}")
