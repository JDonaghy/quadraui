#!/usr/bin/env python3
"""Cross-language parity check: the same app in Rust, Python and Node must
render the same TUI frames and fire the same updates.

Runs `examples/parity/{rust,python,node}` headless on the TUI backend, each
driven through the one input script `examples/parity/script.json` (clicks
at cell coordinates, typed text, named keys). Every tracer prints a JSON
trace: the frame after the first paint and after every step, as the glyph
plus fg/bg colour of every cell, and the app-level updates the step fired.
This script compares the traces step by step and exits non-zero on the
first difference, printing both frames side by side and every differing
cell.

Python is the reference the other two are compared against. An entry in
`examples/parity/known_divergences.json` allows a recorded difference: only
at its listed steps, only inside its cell region (and in the fired updates
only when it says `events`). A listed step that no longer differs there
fails the check, so a fix has to remove the entry.

    scripts/parity_check.py                      # run all three (Rust via cargo run)
    scripts/parity_check.py --rust-bin PATH      # a prebuilt parity-trace
    scripts/parity_check.py --save DIR           # also write each trace to DIR

Prerequisites: the `quadraui` Python extension importable by `python3` (see
scripts/test_quadraui_py.sh), the Node addon built in quadraui-node/ (`npm run
build:debug`) with its devDependencies installed, and `cargo` on PATH unless
`--rust-bin` is given.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from typing import Any, Dict, List, Optional, Tuple

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PARITY = os.path.join(ROOT, "examples", "parity")
SCRIPT = os.path.join(PARITY, "script.json")
KNOWN = os.path.join(PARITY, "known_divergences.json")
REFERENCE = "python"


def tracer_commands(rust_bin: Optional[str]) -> Dict[str, List[str]]:
    rust = (
        [rust_bin]
        if rust_bin
        else ["cargo", "run", "-q", "-p", "quadraui-parity", "--features", "tui", "--bin", "parity-trace", "--"]
    )
    return {
        "python": [sys.executable, os.path.join(PARITY, "python", "trace.py")],
        "rust": rust,
        "node": ["node", os.path.join(PARITY, "node", "trace.ts")],
    }


def run_tracer(lang: str, command: List[str], script: str) -> List[Dict[str, Any]]:
    proc = subprocess.run(command + [script], cwd=ROOT, capture_output=True, text=True)
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"parity: the {lang} tracer failed (exit {proc.returncode}): {' '.join(command)}")
    return json.loads(proc.stdout)


def describe(step: Any) -> str:
    return "first paint" if step is None else json.dumps(step)


def cell(frame: Dict[str, Any], x: int, y: int) -> Optional[str]:
    """One cell as `glyph fg/bg`, or None off the grid."""
    rows, styles = frame["rows"], frame["styles"]
    if y >= len(styles) or x >= len(styles[y]):
        return None
    glyph = rows[y][x] if y < len(rows) and x < len(rows[y]) else " "
    return f"{glyph!r} {styles[y][x]}"


def differing_cells(ref: Dict[str, Any], other: Dict[str, Any]) -> List[Tuple[int, int, Optional[str], Optional[str]]]:
    height = max(len(ref["styles"]), len(other["styles"]))
    width = max((len(r) for r in ref["styles"] + other["styles"]), default=0)
    out = []
    for y in range(height):
        for x in range(width):
            a, b = cell(ref, x, y), cell(other, x, y)
            if a != b:
                out.append((x, y, a, b))
    return out


def inside(x: int, y: int, region: List[int]) -> bool:
    rx, ry, rw, rh = region
    return rx <= x < rx + rw and ry <= y < ry + rh


def compare(ref: Dict[str, Any], other: Dict[str, Any], allowed: List[Dict[str, Any]]) -> Tuple[List[str], List[Dict[str, Any]]]:
    """Differences between two frames that no `allowed` divergence covers,
    as readable lines, plus the allowed divergences that did not actually
    show up in this frame (stale)."""
    cells = differing_cells(ref, other)
    events_differ = ref["events"] != other["events"]
    problems: List[str] = []
    if events_differ and not any(d.get("events") for d in allowed):
        problems.append(f"  updates fired: {json.dumps(ref['events'])} != {json.dumps(other['events'])}")
    for x, y, a, b in cells:
        if not any(inside(x, y, d["region"]) for d in allowed):
            problems.append(f"  cell ({x},{y}): {a} != {b}")
    stale = [
        d
        for d in allowed
        if not any(inside(x, y, d["region"]) for x, y, _, _ in cells) and not (d.get("events") and events_differ)
    ]
    return problems, stale


def side_by_side(left: str, ref: Dict[str, Any], right: str, other: Dict[str, Any]) -> str:
    rows_a, rows_b = ref["rows"], other["rows"]
    width = max((len(r) for r in rows_a), default=0)
    lines = [f"   {left:<{width + 2}}  {right}"]
    for y in range(max(len(rows_a), len(rows_b))):
        a = rows_a[y] if y < len(rows_a) else ""
        b = rows_b[y] if y < len(rows_b) else ""
        mark = "*" if a != b else " "
        lines.append(f"{mark}{y:>2}|{a:<{width}}|  |{b}|")
    return "\n".join(lines)


def load_known() -> List[Dict[str, Any]]:
    if not os.path.exists(KNOWN):
        return []
    return json.load(open(KNOWN))["divergences"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--script", default=SCRIPT, help="input script (default: examples/parity/script.json)")
    parser.add_argument("--rust-bin", help="a prebuilt parity-trace binary instead of `cargo run`")
    parser.add_argument("--save", metavar="DIR", help="also write each language's trace to DIR/<lang>.json")
    parser.add_argument("--keep-going", action="store_true", help="report every differing step, not just the first")
    parser.add_argument("--only", choices=["rust", "node"], action="append", help="compare only these against python")
    args = parser.parse_args()

    commands = tracer_commands(args.rust_bin)
    langs = [REFERENCE] + (args.only or ["rust", "node"])
    traces = {lang: run_tracer(lang, commands[lang], args.script) for lang in langs}
    if args.save:
        os.makedirs(args.save, exist_ok=True)
        for lang, trace in traces.items():
            with open(os.path.join(args.save, f"{lang}.json"), "w") as f:
                json.dump(trace, f, indent=1)

    known = load_known()
    ref = traces[REFERENCE]
    failures: List[str] = []
    for lang in langs[1:]:
        other = traces[lang]
        if len(other) != len(ref):
            failures.append(f"{lang}: {len(other)} frames, {REFERENCE}: {len(ref)}")
            continue
        for i, (a, b) in enumerate(zip(ref, other)):
            allowed = [d for d in known if d["lang"] == lang and i in d["steps"]]
            problems, stale = compare(a, b, allowed)
            for d in stale:
                failures.append(
                    f"{lang} step {i} ({describe(a['input'])}) no longer shows the known divergence "
                    f"{d['id']!r} ({d['reason']}). If it was fixed, drop step {i} from that entry in "
                    f"examples/parity/known_divergences.json (and the entry once it lists no steps)."
                )
            for d in allowed:
                if d not in stale:
                    print(f"known divergence {d['id']!r}: {lang} step {i} ({describe(a['input'])})")
            if problems:
                failures.append(
                    f"{lang} differs from {REFERENCE} at step {i} ({describe(a['input'])}):\n"
                    + side_by_side(REFERENCE, a, lang, b)
                    + "\n"
                    + "\n".join(problems[:40])
                    + (f"\n  ... {len(problems) - 40} more" if len(problems) > 40 else "")
                )
                if not args.keep_going:
                    break  # later steps of this language only repeat the drift
    if failures:
        print("\nPARITY FAILED\n\n" + "\n\n".join(failures))
        return 1
    steps = len(ref) - 1
    allowed = sorted({d["id"] for d in known if d["lang"] in langs})
    caveat = f", apart from the recorded divergences {', '.join(allowed)}" if allowed else ""
    print(f"parity ok: {', '.join(langs)} rendered the same frames and fired the same updates over {steps} steps{caveat}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
