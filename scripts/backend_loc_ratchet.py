#!/usr/bin/env python3
"""Per-backend line-count ratchet for `quadraui/src/{tui,macos,win,gtk}` (#2373).

Those four directories grew by roughly 4.1k lines in three days while the
consolidation epics' shared engines landed beside them instead of replacing
code inside them. This script is the CI gate that stops that from happening
silently again: it counts `.rs` lines under each backend directory and
compares the count against a committed baseline
(`scripts/backend_loc_baseline.json`), the same ratchet shape as
`tools/comment_history_lint.py`'s per-module thresholds.

A directory fails only when it grows *past its baseline by more than a
small allowance* (`--allowance`, default below) -- a one- or two-line diff
doesn't trip it. A directory that shrinks passes, and the report prints the
lower number so a human can commit it as the new baseline, tightening the
ratchet. Raising a baseline to accommodate real growth is the escape hatch:
edit `scripts/backend_loc_baseline.json` in the same PR, so the increase is
a visible line in the diff instead of something this script papers over.

Usage:
    scripts/backend_loc_ratchet.py                      # report + exit 1 on any dir over its limit
    scripts/backend_loc_ratchet.py --report-only         # report, always exit 0
    scripts/backend_loc_ratchet.py --baseline FILE        # override the default baseline file
    scripts/backend_loc_ratchet.py --allowance N          # override the default per-dir allowance
    scripts/backend_loc_ratchet.py --src-root DIR         # override `quadraui/src` (tests only)

Run from anywhere; paths are resolved relative to this script's location
unless `--src-root`/`--baseline` are given explicitly.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BASELINE_FILE = Path(__file__).resolve().parent / "backend_loc_baseline.json"
DEFAULT_SRC_ROOT = REPO_ROOT / "quadraui" / "src"

# The four per-backend directories #2373 is ratcheting. Mirrors the module
# table `tools/comment_history_lint.py` already uses for the same
# directories, minus `compose`/`primitives`, which aren't backend-specific.
BACKEND_DIRS = ("tui", "macos", "win", "gtk")

# Small per-PR wiggle room so the gate isn't tripped by an incidental
# one-line fix inside a backend directory. A real feature is expected to
# cross this within a file or two -- at that point the escape hatch (bump
# the baseline JSON) is the intended path, not raising this constant.
DEFAULT_ALLOWANCE = 40


def count_lines(directory: Path) -> int:
    """Total line count across every `.rs` file under `directory`."""
    if not directory.is_dir():
        return 0
    total = 0
    for path in sorted(directory.rglob("*.rs")):
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        total += len(text.splitlines())
    return total


def current_counts(src_root: Path, dirs: tuple[str, ...] = BACKEND_DIRS) -> dict[str, int]:
    return {d: count_lines(src_root / d) for d in dirs}


def load_baseline(path: Path) -> dict[str, int]:
    if not path.exists():
        return {}
    return json.loads(path.read_text())


def build_report(
    counts: dict[str, int], baseline: dict[str, int], allowance: int
) -> tuple[bool, str]:
    """Returns (any_over, report_text)."""
    all_dirs = sorted(set(counts) | set(baseline))
    name_width = max((len(d) for d in all_dirs), default=len("dir"))
    name_width = max(name_width, len("dir"))
    lines = [
        f"{'dir':<{name_width}}  {'lines':>7}  {'baseline':>8}  {'limit':>7}  status"
    ]
    any_over = False
    for d in all_dirs:
        count = counts.get(d, 0)
        base = baseline.get(d)
        if base is None:
            status = "(no baseline)"
            base_cell = "—"
            limit_cell = "—"
        else:
            limit = base + allowance
            base_cell = str(base)
            limit_cell = str(limit)
            if count > limit:
                status = "OVER"
                any_over = True
            elif count < base:
                status = f"ok (shrink: commit baseline={count})"
            else:
                status = "ok"
        lines.append(
            f"{d:<{name_width}}  {count:>7}  {base_cell:>8}  {limit_cell:>7}  {status}"
        )
    return any_over, "\n".join(lines)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Fail when a per-backend src directory grows past its committed baseline (#2373)."
    )
    parser.add_argument(
        "--baseline",
        type=Path,
        default=DEFAULT_BASELINE_FILE,
        metavar="FILE",
        help="JSON file mapping backend dir name -> baseline line count (default: %(default)s).",
    )
    parser.add_argument(
        "--src-root",
        type=Path,
        default=DEFAULT_SRC_ROOT,
        metavar="DIR",
        help="Directory containing the backend subdirectories (default: %(default)s).",
    )
    parser.add_argument(
        "--allowance",
        type=int,
        default=DEFAULT_ALLOWANCE,
        metavar="N",
        help="Lines a directory may exceed its baseline by before failing (default: %(default)s).",
    )
    parser.add_argument(
        "--report-only",
        action="store_true",
        help="Print the report but always exit 0 (ignore the ratchet).",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    baseline = load_baseline(args.baseline)
    counts = current_counts(args.src_root)
    any_over, report = build_report(counts, baseline, args.allowance)
    print(report)
    print(f"\nallowance: {args.allowance} line(s) past baseline before a directory fails")
    if args.report_only:
        return 0
    return 1 if any_over else 0


if __name__ == "__main__":
    sys.exit(main())
