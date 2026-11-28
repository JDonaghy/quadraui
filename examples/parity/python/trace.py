"""Runs the parity app (`todo.py`) headless on the TUI backend through the
input script given on the command line, and prints one JSON trace: after
the first paint and after every step, the rendered cell grid (glyph, fg
and bg of every cell) and the app updates that step fired. The Rust and
Node tracers print the same shape; `scripts/parity_check.py` diffs them.

    python examples/parity/python/trace.py examples/parity/script.json
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from todo import Todo  # noqa: E402
from quadraui.app import App  # noqa: E402
from quadraui.testing import harness  # noqa: E402

WIDTH, HEIGHT = 40, 12


def hex_rgb(rgb):
    return "%02x%02x%02x" % tuple(rgb)


def frame(h, todo, input_step):
    rows = h.screen().rstrip("\n").split("\n")
    styles = []
    for y in range(HEIGHT):
        row = []
        for x in range(WIDTH):
            fg, bg = h.qui.cell_colors(x, y)
            row.append(hex_rgb(fg) + "/" + hex_rgb(bg))
        styles.append(row)
    events, todo.log[:] = list(todo.log), []
    return {"input": input_step, "rows": rows, "styles": styles, "events": events}


def main():
    script = json.load(open(sys.argv[1]))
    todo = Todo()
    h = harness(App(WIDTH, HEIGHT, view=todo.view))
    trace = [frame(h, todo, None)]
    for step in script:
        if "click" in step:
            h.click_at(*step["click"])
        elif "type" in step:
            h.type(step["type"])
        elif "key" in step:
            h.press(step["key"])
        else:
            raise ValueError(f"unknown step {step!r}")
        trace.append(frame(h, todo, step))
    json.dump(trace, sys.stdout)


if __name__ == "__main__":
    main()
