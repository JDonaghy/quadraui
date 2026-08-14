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

Exit status:
    0 — every example has its required driver-suite coverage.
    1 — at least one example is missing coverage (see the printed matrix).

Run from anywhere; paths are resolved relative to this script's location.
"""
from __future__ import annotations

import re
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


def main() -> int:
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
    return 1 if any_missing else 0


if __name__ == "__main__":
    sys.exit(main())
