#!/usr/bin/env python3
"""Every code sample in the README and the hand-written site pages is either
copied from a file CI builds or runs, or says why it is not.

A fenced block in Rust, Python, TypeScript or JavaScript must sit directly
under a marker comment:

    <!-- sample: examples/hero/python/counter.py -->
    ```python
    ...
    ```

The block's lines (trailing whitespace ignored) must appear, in order and
contiguous, in the named file, which the repo's CI compiles or runs. An
excerpt is fine; an edited copy is not. A sample that cannot be checked that
way says so instead:

    <!-- sample: untested — the reason -->

Shell, TOML and other non-program blocks are exempt. Generated pages
(`site/src/gallery/`, `site/src/SUMMARY.md`) are skipped: their code comes
straight from the gallery's sources.

Stdlib-only (`unittest`), like the other `tools/test_*.py`. Run with:

    python3 tools/test_doc_samples.py
"""
from __future__ import annotations

import re
import unittest
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

CODE_LANGS = {"rust", "python", "py", "ts", "tsx", "typescript", "js", "javascript", "jsx"}
MARKER_RE = re.compile(r"^<!--\s*sample:\s*(.+?)\s*-->$")
FENCE_RE = re.compile(r"^(\s*)(`{3,})\s*([\w+-]*)")


@dataclass
class Sample:
    doc: Path
    line: int
    lang: str
    marker: str | None
    body: list[str]


def doc_files() -> list[Path]:
    site = REPO_ROOT / "site" / "src"
    pages = [
        p
        for p in sorted(site.rglob("*.md"))
        if "gallery" not in p.relative_to(site).parts and p.name != "SUMMARY.md"
    ]
    return [REPO_ROOT / "README.md", *pages]


def samples(doc: Path) -> list[Sample]:
    lines = doc.read_text().splitlines()
    found: list[Sample] = []
    i = 0
    while i < len(lines):
        m = FENCE_RE.match(lines[i])
        if not m:
            i += 1
            continue
        indent, fence, info = m.group(1), m.group(2), m.group(3)
        lang = info.split(",")[0].lower()
        j = i + 1
        while j < len(lines) and not lines[j].strip().startswith(fence):
            j += 1
        body = [line[len(indent):] if line.startswith(indent) else line for line in lines[i + 1 : j]]
        if lang in CODE_LANGS:
            k = i - 1
            while k >= 0 and not lines[k].strip():
                k -= 1
            marker = None
            if k >= 0:
                mm = MARKER_RE.match(lines[k].strip())
                marker = mm.group(1) if mm else None
            found.append(Sample(doc, i + 1, lang, marker, body))
        i = j + 1
    return found


def contains_contiguous(haystack: list[str], needle: list[str]) -> bool:
    hay = [line.rstrip() for line in haystack]
    nee = [line.rstrip() for line in needle]
    while nee and not nee[-1]:
        nee.pop()
    if not nee:
        return True
    n = len(nee)
    return any(hay[i : i + n] == nee for i in range(len(hay) - n + 1))


def check(sample: Sample) -> str | None:
    where = f"{sample.doc.relative_to(REPO_ROOT)}:{sample.line}"
    if sample.marker is None:
        return (
            f"{where}: {sample.lang} sample has no `<!-- sample: <file> -->` marker "
            "(or `<!-- sample: untested — <reason> -->`) on the line above it"
        )
    if sample.marker.startswith("untested"):
        reason = sample.marker[len("untested") :].strip(" —-:")
        return None if reason else f"{where}: `untested` marker gives no reason"
    source = REPO_ROOT / sample.marker
    if not source.is_file():
        return f"{where}: sample marker names {sample.marker}, which does not exist"
    if not contains_contiguous(source.read_text().splitlines(), sample.body):
        return (
            f"{where}: sample no longer matches {sample.marker}; copy the current "
            "code from that file (the file is what CI builds)"
        )
    return None


class DocSamplesTest(unittest.TestCase):
    def test_every_sample_is_checked_or_says_why_not(self):
        problems = [p for doc in doc_files() for s in samples(doc) if (p := check(s))]
        self.assertEqual(problems, [], "\n".join(problems))

    def test_the_readme_hero_samples_are_checked_against_the_hero_files(self):
        readme = samples(REPO_ROOT / "README.md")
        markers = {s.marker for s in readme}
        for path in (
            "examples/hero/rust/src/main.rs",
            "examples/hero/python/counter.py",
            "examples/hero/node/counter.tsx",
        ):
            self.assertIn(path, markers)


class MatcherTest(unittest.TestCase):
    def test_excerpt_matches_and_edited_copy_does_not(self):
        source = ["a", "b  ", "c", "d"]
        self.assertTrue(contains_contiguous(source, ["b", "c"]))
        self.assertFalse(contains_contiguous(source, ["b", "d"]))
        self.assertFalse(contains_contiguous(source, ["b", "C"]))

    def test_marker_must_be_directly_above_the_fence(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            doc = Path(tmp) / "x.md"
            doc.write_text("<!-- sample: untested — demo -->\n\n```rust\nfn x() {}\n```\n\ntext\n\n```python\npass\n```\n")
            found = samples(doc)
            self.assertEqual([s.marker for s in found], ["untested — demo", None])


if __name__ == "__main__":
    unittest.main()
