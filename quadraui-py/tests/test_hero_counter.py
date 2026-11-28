"""Runs the README hero counter (`examples/hero/python/run_headless.py`) so
the sample stays executable. The Rust and Node versions of the same app are
checked by `examples/hero/rust/tests/headless.rs` and
`quadraui-node/tests/hero_counter.test.ts`."""

import pathlib
import subprocess
import sys

_RUNNER = pathlib.Path(__file__).resolve().parents[2] / "examples" / "hero" / "python" / "run_headless.py"


def test_hero_counter_runs_headless_and_counts():
    result = subprocess.run([sys.executable, str(_RUNNER)], capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "Count: 1" in result.stdout
    assert "ok: python hero counter" in result.stdout
