"""Drives the hero counter headless on the TUI backend: click `+`, `+`, `-`
by their on-screen labels (the two `+` clicks land on one spot inside the
double-click interval, and each one counts), then check the rendered frame. The Rust
test (`examples/hero/rust/tests/headless.rs`) and the Node runner
(`examples/hero/node/run_headless.ts`) drive the identical script.

    python examples/hero/python/run_headless.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from counter import Counter  # noqa: E402
from quadraui.app import App  # noqa: E402
from quadraui.testing import harness  # noqa: E402

counter = Counter()
h = harness(App(30, 5, view=counter.view))
assert h.contains("Count: 0"), h.screen()
for label in ("+", "+"):
    h.click_text(label)
assert h.contains("Count: 2"), h.screen()
h.click_text("-")
print(h.screen())
assert h.contains("Count: 1"), h.screen()
assert counter.count == 1
print("ok: python hero counter")
