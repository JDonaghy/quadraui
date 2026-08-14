#!/usr/bin/env python3
"""Driver-test coverage report for quadraui examples (#310).

Enumerates every `quadraui/examples/tui_*.rs` and `quadraui/examples/gtk_*.rs`
file, works out which `AppLogic` struct each one instantiates, and checks
whether that struct is actually wired into the corresponding
`quadraui/tests/{tui,gtk}_example_driver.rs` in-process driver suite (via its
`#[path = "../examples/common/<mod>.rs"] mod ...;` include + `use ...;`).

This is a coverage *proxy*, not a guarantee the struct has a meaningful
`#[test]` — it answers "is this example's app wired into the driver suite at
all", which is what #A1 ("every new example needs a test") needs to be
measurable. See issue #310.

Usage:
    tools/example_coverage.py
    tools/example_coverage.py --fail-on-gap [--base <git-ref>]

Exit status (no flags — full-audit mode):
    0 — every example has its required driver-suite coverage.
    1 — at least one example is missing coverage (see the printed matrix).

Exit status (--fail-on-gap — CI delta-gate mode, #311):
    The matrix always prints in full, but the exit status only reflects
    `tui_*.rs` examples ADDED relative to `--base` (default: `origin/develop`,
    matching this repo's branching policy of merging feature PRs into
    `develop`) —
    pre-existing gaps (most of GTK, and the TUI examples still mid the
    #B1-#B5 backfill) print but don't fail the run. GTK examples are never
    part of the delta gate, new or not — GTK-example coverage waits on
    `GtkDriver` (#301), same "TUI only for now" line CLAUDE.md draws.
    0 — no newly-added `tui_*.rs` example is missing its driver test.
    1 — a newly-added `tui_*.rs` example is missing its driver test.
    2 — the git diff needed to find "newly-added" couldn't be computed
        (unreachable --base, not a git repo, git not on PATH, ...). This is
        a tooling failure, not "no new examples" — never silently treated
        as a pass, since that would open exactly the hole #311 exists to
        close.

Run from anywhere; paths are resolved relative to this script's location.
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
EXAMPLES_DIR = REPO_ROOT / "quadraui" / "examples"
DRIVER_FILES = {
    "TUI": REPO_ROOT / "quadraui" / "tests" / "tui_example_driver.rs",
    "GTK": REPO_ROOT / "quadraui" / "tests" / "gtk_example_driver.rs",
}

# The app struct an example instantiates is always some capitalised
# `Type::new()` call inside `fn main`, e.g.:
#   quadraui::tui::run(common::PipelineApp::new())
#   let app = common::appshell_demo::AppShellDemo::new();
NEW_CALL_RE = re.compile(r"\b([A-Z][A-Za-z0-9_]*)::new\(\)")
MAIN_FN_RE = re.compile(r"fn main\s*\([^)]*\)[^{]*\{")


def main_body(source: str) -> str:
    """Return the text of `fn main() { ... }`, brace-matched from source."""
    m = MAIN_FN_RE.search(source)
    if not m:
        return source
    depth = 1
    i = m.end()
    start = i
    while i < len(source) and depth > 0:
        if source[i] == "{":
            depth += 1
        elif source[i] == "}":
            depth -= 1
        i += 1
    return source[start:i]


def app_type_for(example_path: Path) -> str | None:
    """The AppLogic struct name `fn main` in `example_path` instantiates."""
    source = example_path.read_text()
    body = main_body(source)
    match = NEW_CALL_RE.search(body)
    return match.group(1) if match else None


def is_covered(app_type: str, driver_source: str) -> bool:
    return re.search(rf"\b{re.escape(app_type)}\b", driver_source) is not None


def new_tui_example_stems(
    base_ref: str,
    repo_root: Path = REPO_ROOT,
    examples_dir: Path = EXAMPLES_DIR,
) -> set[str]:
    """Stems of `examples/tui_*.rs` files this branch adds relative to `base_ref`.

    Scoped to `tui_*.rs` on purpose: GTK-example coverage is out of scope for
    the #311 CI gate until #301 ships (CLAUDE.md's "TUI only for now"), so a
    newly-added `gtk_*.rs` example must never make this gate fail.

    Uses `--no-renames`: git's rename detection is on by default (similarity
    threshold 50%), and without this flag copying an existing, *uncovered*
    example to a new filename with a trivial edit gets classified as a
    rename (`R`) rather than an addition (`A`) and silently evades
    `--diff-filter=A`. `--no-renames` forces every genuinely new path to
    report as `A` regardless of how similar it is to a deleted file — which
    is exactly the copy-an-existing-example-as-a-starting-point workflow
    CLAUDE.md itself documents as the normal way to author a new example.

    `repo_root` / `examples_dir` default to the real repo paths but are
    overridable so tests can point this at a throwaway git repo.

    Raises `subprocess.CalledProcessError` / `FileNotFoundError` if the diff
    can't be computed — callers must not treat that as "no new examples"; see
    the module docstring's exit-status-2 note.
    """
    examples_pathspec = examples_dir.relative_to(repo_root).as_posix()
    result = subprocess.run(
        [
            "git",
            "diff",
            "--no-renames",
            "--name-only",
            "--diff-filter=A",
            f"{base_ref}...HEAD",
            "--",
            examples_pathspec,
        ],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=True,
    )
    stems = set()
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        p = Path(line)
        if p.parent.name == "examples" and p.name.startswith("tui_") and p.suffix == ".rs":
            stems.add(p.stem)
    return stems


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Driver-test coverage report for quadraui examples (#310)."
    )
    parser.add_argument(
        "--fail-on-gap",
        action="store_true",
        help=(
            "Only fail when a tui_*.rs example ADDED relative to --base is "
            "missing driver-suite coverage (#311); pre-existing gaps and any "
            "GTK gap still print but no longer fail the run. Without this "
            "flag, fail on ANY gap (the tool's original full-audit mode)."
        ),
    )
    parser.add_argument(
        "--base",
        default="origin/develop",
        metavar="REF",
        help=(
            "git ref to diff against when finding new examples (default: "
            "%(default)s). This repo's branching policy merges feature PRs "
            "into `develop` (main only moves via release merges from "
            "develop), and CI always diffs against the PR's actual base "
            "commit -- so `origin/develop` is the ref that matches CI for "
            "the PRs this gate exists to catch."
        ),
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)

    driver_sources = {
        kind: (path.read_text() if path.exists() else "")
        for kind, path in DRIVER_FILES.items()
    }

    rows = []  # (example_name, kind, app_type | None, covered | None)
    for path in sorted(EXAMPLES_DIR.glob("tui_*.rs")) + sorted(
        EXAMPLES_DIR.glob("gtk_*.rs")
    ):
        kind = "TUI" if path.name.startswith("tui_") else "GTK"
        app_type = app_type_for(path)
        covered = is_covered(app_type, driver_sources[kind]) if app_type else False
        rows.append((path.stem, kind, app_type, covered))

    rows.sort(key=lambda r: r[0])

    name_width = max((len(r[0]) for r in rows), default=len("Example"))
    name_width = max(name_width, len("Example"))
    header = f"{'Example':<{name_width}}  {'TUI test':<10}  {'GTK test':<10}"
    print(header)

    def cell(kind: str, row) -> str:
        _, row_kind, _app_type, covered = row
        if row_kind != kind:
            return "—"  # em dash: N/A — this example isn't of this kind
        return "✓" if covered else "✗"  # check / cross

    for row in rows:
        name = row[0]
        tui_cell = cell("TUI", row)
        gtk_cell = cell("GTK", row)
        print(f"{name:<{name_width}}  {tui_cell:<10}  {gtk_cell:<10}")

    tui_total = [r for r in rows if r[1] == "TUI"]
    gtk_total = [r for r in rows if r[1] == "GTK"]
    tui_covered = sum(1 for r in tui_total if r[3])
    gtk_covered = sum(1 for r in gtk_total if r[3])
    print(
        f"Total coverage: {tui_covered}/{len(tui_total)} TUI, "
        f"{gtk_covered}/{len(gtk_total)} GTK"
    )

    any_missing = any(not r[3] for r in rows)

    if not args.fail_on_gap:
        return 1 if any_missing else 0

    try:
        new_stems = new_tui_example_stems(args.base)
    except (subprocess.CalledProcessError, FileNotFoundError) as exc:
        print(
            f"\nerror: could not diff against '{args.base}' to find newly-added "
            f"tui_*.rs examples: {exc}\n"
            "This is the #311 CI gate — refusing to treat a failed diff as "
            "'no new examples', since that would silently let a real gap "
            "through. Make sure --base is a ref reachable from this checkout "
            "(CI fetches it with fetch-depth: 0 before calling this tool).",
            file=sys.stderr,
        )
        return 2

    if new_stems:
        print(f"\nNew tui_*.rs examples since {args.base}: {', '.join(sorted(new_stems))}")

    new_gaps = [r for r in rows if r[0] in new_stems and not r[3]]
    if new_gaps:
        print("\nNewly-added examples missing required driver-suite coverage (#311):")
        for row in new_gaps:
            print(f"  {row[0]}")
        return 1

    if any_missing:
        print(
            "\nPre-existing gaps remain above (not introduced by this change) "
            "— not failing. Tracked by the #B1-#B5 backfill."
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
