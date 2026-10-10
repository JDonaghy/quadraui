#!/usr/bin/env python3
"""Generate the mdBook gallery pages for the quadraui website (#1349).

Joins `quadraui-gallery`'s own registry dump (`--dump-registry`, see
`quadraui-gallery/src/site_export.rs`) against one or more capture
directories' `manifest.json` (`--capture`, see
`quadraui-gallery/src/capture.rs`) to write one mdBook page per demo —
its code sample (lifted verbatim from the `// gallery:begin` /
`// gallery:end` region the gallery's own Code tab uses) and a
backend-support grid (TUI / GTK / macOS / Windows, one column per
backend, one row per variant). No per-widget content here is
hand-maintained: everything comes from those two generated inputs.

Usage:
    tools/site_gen.py \\
        --registry /path/to/registry.json \\
        --capture /path/to/capture-dir [--capture /path/to/other-capture-dir ...] \\
        --out site/src

`--capture` is repeatable because each backend/OS combination's capture
run writes its own directory + `manifest.json` (see `run_capture`'s own
module doc for why) — a Linux CI leg's TUI-only manifest and a macOS
leg's manifest both get merged here, keyed by
`(demo, group, variant, backend)`, preferring any row with a real
`path` over a `status`-only row from another run.

Writes `<out>/gallery/index.md`, `<out>/gallery/<slug>.md` per demo,
copies every referenced image into `<out>/gallery/images/`, and
(re)writes `<out>/SUMMARY.md` in full — the generated half of the site;
`<out>/introduction.md` and `<out>/getting-started.md` are
hand-maintained chapters this script only *links to*, never edits.

Run from anywhere; paths are resolved relative to the current working
directory (all arguments are plain paths, no repo-root assumption).
"""
from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
from pathlib import Path
from typing import Any

# Canonical group order, mirroring `quadraui-gallery/src/app.rs`'s
# `GROUPS` constant. A group name not in this list (e.g. a fixture's
# made-up group name in a test, or a future sixth activity-bar group)
# still gets a section — just appended alphabetically after the known
# ones, rather than silently dropped.
KNOWN_GROUP_ORDER = ["Content", "Chrome", "Containers", "Overlays", "Data"]

BACKENDS = ["tui", "gtk", "macos", "win"]
BACKEND_LABELS = {"tui": "TUI", "gtk": "GTK", "macos": "macOS", "win": "Windows"}

# `"unsupported"` means "this particular build/host didn't capture this
# backend" (`quadraui-gallery/src/capture.rs`'s own doc is explicit about
# that), never "quadraui doesn't support this backend". On a public
# landing site whose whole pitch is four backends, labelling every
# macOS/Windows cell "Unsupported" (as a `tui`-only CI build necessarily
# does) reads as the opposite of the truth, so the label names the build,
# not the project.
STATUS_LABELS = {
    "unsupported": "Not captured in this build",
    "capture-pending": "Pending",
}


def slug(s: str) -> str:
    """Filesystem/URL-safe fragment: lowercase, non-alphanumeric runs
    collapsed to a single `-`. Mirrors `quadraui-gallery/src/capture.rs`'s
    own `slug` helper (kept as a separate implementation since this
    script has no Rust to import, but the same algorithm) so a demo's
    page filename reads the same as its capture filenames' own slugged
    segments.
    """
    out = []
    last_was_dash = False
    for ch in s:
        if ch.isalnum() and ch.isascii():
            out.append(ch.lower())
            last_was_dash = False
        elif not last_was_dash and out:
            out.append("-")
            last_was_dash = True
    while out and out[-1] == "-":
        out.pop()
    return "".join(out) or "demo"


def read_json_array(path: Path, what: str) -> list[dict[str, Any]]:
    """Read `path` as a JSON array, raising a message that names the file
    and what it was expected to be instead of a bare traceback — both
    "file doesn't exist" and "file isn't the shape we expected" are
    equally easy mistakes to make when wiring this script into a new CI
    job, and both should read as "fix your invocation", not a stack trace.
    """
    if not path.is_file():
        raise FileNotFoundError(f"{path}: no such file ({what})")
    entries = json.loads(path.read_text())
    if not isinstance(entries, list):
        raise ValueError(f"{path}: expected a JSON array ({what})")
    return entries


def load_registry(path: Path) -> list[dict[str, Any]]:
    return read_json_array(path, "registry.json from --dump-registry")


def manifest_key(row: dict[str, Any]) -> tuple[str, str, str, str]:
    return (row["demo"], row["group"], row["variant"], row["backend"])


def merge_manifests(capture_dirs: list[Path]) -> dict[tuple[str, str, str, str], dict[str, Any]]:
    """Load `manifest.json` from each of `capture_dirs` and merge rows
    keyed by `(demo, group, variant, backend)`.

    A row carrying a real `path` always wins over a `status`-only row
    for the same key, regardless of which directory it came from, so a
    Linux leg's `"unsupported"` macOS rows don't shadow a macOS leg's
    real captures when both manifests are passed together. Between two
    `path`-bearing rows for the same key, the first one encountered
    wins (deterministic, and in practice each backend's row only ever
    comes from the one capture dir that built that backend). Each row
    is annotated with `_capture_dir` so [`copy_images`] knows which
    directory to resolve its `path` against.
    """
    merged: dict[tuple[str, str, str, str], dict[str, Any]] = {}
    for capture_dir in capture_dirs:
        manifest_path = capture_dir / "manifest.json"
        rows = read_json_array(manifest_path, "manifest.json from --capture")
        for row in rows:
            key = manifest_key(row)
            row = dict(row, _capture_dir=capture_dir)
            existing = merged.get(key)
            if existing is None or (row.get("path") and not existing.get("path")):
                merged[key] = row
    return merged


def copy_images(
    merged: dict[tuple[str, str, str, str], dict[str, Any]], images_out: Path
) -> None:
    """Copy every merged row's `path` image from its own capture dir into
    `images_out`, creating the directory if needed. Rows with no `path`
    (a `status`-only row) have nothing to copy.
    """
    images_out.mkdir(parents=True, exist_ok=True)
    for key, row in merged.items():
        rel_path = row.get("path")
        if not rel_path:
            continue
        capture_dir = row["_capture_dir"].resolve()
        src = (capture_dir / rel_path).resolve()
        if capture_dir not in (src, *src.parents):
            raise ValueError(
                f"manifest row for {key} has an unsafe path outside its "
                f"capture dir: {rel_path!r}"
            )
        dest = images_out / Path(rel_path).name
        shutil.copyfile(src, dest)


def group_sort_key(group: str) -> tuple[int, str]:
    try:
        return (KNOWN_GROUP_ORDER.index(group), group)
    except ValueError:
        return (len(KNOWN_GROUP_ORDER), group)


def cell_markdown(row: dict[str, Any] | None, backend: str) -> str:
    """The backend-grid table cell for one (demo, variant, backend) —
    an embedded image link for a real capture, a labelled status
    string for `unsupported`/`capture-pending`/`error`, or "No data"
    when the manifest has no row at all for this key (a capture
    directory that doesn't cover every demo, e.g. a hand-run partial
    capture).
    """
    if row is None:
        return "No data"
    if row.get("path"):
        filename = Path(row["path"]).name
        return f"![{BACKEND_LABELS[backend]}](images/{filename})"
    status = row.get("status") or "unknown"
    if status == "error":
        note = row.get("note") or "unknown error"
        return f"Error: {table_cell_text(note)}"
    return STATUS_LABELS.get(status, status)


def table_cell_text(text: str) -> str:
    """Make free-form text (a capture's panic/IO `note`) safe to embed in
    a markdown table cell: collapse embedded newlines (a multi-line
    message would otherwise terminate the row early) and escape `|` (a
    literal pipe would otherwise be read as a column separator), either
    of which would silently corrupt the rest of the table.
    """
    return " ".join(text.split()).replace("|", "\\|")


def code_fence(source: str) -> str:
    """A backtick fence long enough that nothing inside `source` can close
    it early. CommonMark lets a fence be 3+ backticks, and closes on the
    first line whose own backtick run is at least as long as the
    opening one — so a demo whose own `source` embeds a fenced code
    sample (e.g. a markdown-adapter demo's own ```rust ``` example) can
    close our wrapping fence prematurely if we always open with exactly
    three. Scanning `source` for its longest run of backticks and adding
    one more guarantees no line inside it can ever match or exceed our
    opening fence's length.
    """
    longest = max((len(run) for run in re.findall(r"`+", source)), default=0)
    return "`" * max(longest + 1, 3)


def render_demo_page(
    entry: dict[str, Any], merged: dict[tuple[str, str, str, str], dict[str, Any]]
) -> str:
    demo = entry["demo"]
    group = entry["group"]
    variants = entry["variants"]
    source = entry["source"]
    fence = code_fence(source)

    # `rust,noplayground`: a bare `rust` info string gets mdBook's Rust
    # Playground "Run" button, but every sample here is a fragment lifted
    # from inside a larger file (a `gallery:begin`/`end` region) and
    # cannot compile standalone, so that button would just error.
    lines = [
        f"# {demo}",
        "",
        f"**Group:** {group}",
        "",
        "## Code",
        "",
        f"{fence}rust,noplayground",
        source,
        fence,
        "",
    ]

    lines.append("## Backends")
    lines.append("")
    header = ["Variant"] + [BACKEND_LABELS[b] for b in BACKENDS]
    lines.append("| " + " | ".join(header) + " |")
    lines.append("|" + "|".join(["---"] * len(header)) + "|")
    for variant in variants:
        cells = [variant]
        for backend in BACKENDS:
            row = merged.get((demo, group, variant, backend))
            cells.append(cell_markdown(row, backend))
        lines.append("| " + " | ".join(cells) + " |")
    lines.append("")
    return "\n".join(lines)


def render_gallery_index(entries: list[dict[str, Any]]) -> str:
    by_group: dict[str, list[dict[str, Any]]] = {}
    for entry in entries:
        by_group.setdefault(entry["group"], []).append(entry)

    lines = ["# Gallery", "", "Every quadraui primitive demo, generated from the gallery's own", "registry — see `tools/site_gen.py`.", ""]
    for group in sorted(by_group, key=group_sort_key):
        lines.append(f"## {group}")
        lines.append("")
        for entry in sorted(by_group[group], key=lambda e: e["demo"]):
            demo_slug = slug(entry["demo"])
            lines.append(f"- [{entry['demo']}]({demo_slug}.md)")
        lines.append("")
    return "\n".join(lines)


def render_summary(entries: list[dict[str, Any]]) -> str:
    by_group: dict[str, list[dict[str, Any]]] = {}
    for entry in entries:
        by_group.setdefault(entry["group"], []).append(entry)

    lines = [
        "# Summary",
        "",
        "[Introduction](introduction.md)",
        "[Getting Started](getting-started.md)",
        "[Gallery Overview](gallery/index.md)",
        "",
    ]
    for group in sorted(by_group, key=group_sort_key):
        lines.append(f"# {group}")
        lines.append("")
        for entry in sorted(by_group[group], key=lambda e: e["demo"]):
            demo_slug = slug(entry["demo"])
            lines.append(f"- [{entry['demo']}](gallery/{demo_slug}.md)")
        lines.append("")
    return "\n".join(lines)


def generate(registry_path: Path, capture_dirs: list[Path], out_dir: Path) -> None:
    entries = load_registry(registry_path)
    merged = merge_manifests(capture_dirs)

    seen_slugs: dict[str, str] = {}
    for entry in entries:
        demo_slug = slug(entry["demo"])
        collision = seen_slugs.get(demo_slug)
        if collision is not None and collision != entry["demo"]:
            raise ValueError(
                f"demo names {collision!r} and {entry['demo']!r} both slug "
                f"to {demo_slug!r} — their gallery pages would overwrite "
                "each other"
            )
        seen_slugs[demo_slug] = entry["demo"]

    gallery_dir = out_dir / "gallery"
    if gallery_dir.exists():
        shutil.rmtree(gallery_dir)
    gallery_dir.mkdir(parents=True)

    copy_images(merged, gallery_dir / "images")

    for entry in entries:
        page = render_demo_page(entry, merged)
        (gallery_dir / f"{slug(entry['demo'])}.md").write_text(page)

    (gallery_dir / "index.md").write_text(render_gallery_index(entries))
    (out_dir / "SUMMARY.md").write_text(render_summary(entries))


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Generate the quadraui mdBook gallery site (#1349).")
    parser.add_argument("--registry", required=True, type=Path, help="registry.json from --dump-registry")
    parser.add_argument(
        "--capture",
        required=True,
        action="append",
        type=Path,
        dest="capture_dirs",
        help="a capture directory containing manifest.json (repeatable)",
    )
    parser.add_argument("--out", required=True, type=Path, help="mdBook src directory to write gallery/ and SUMMARY.md into")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    generate(args.registry, args.capture_dirs, args.out)
    print(f"site_gen: wrote gallery pages + SUMMARY.md into {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
