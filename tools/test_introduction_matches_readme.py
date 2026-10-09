#!/usr/bin/env python3
"""Guards `site/src/introduction.md`'s hello-world sample against drifting
from `README.md`'s.

`site/src/introduction.md` was written by hand-copying `README.md`'s own
hello-world walkthrough (the landing page's pitch, per issue #1349's own
brief: "the README's hello world"). Nothing keeps the two in sync after
that copy, so this test fails loudly the moment one changes without the
other, instead of the drift going unnoticed until a reader spots a stale
sample on the site.

Stdlib-only (`unittest`), same reasoning as `test_site_gen.py` and its
neighbors. Run with:

    python3 tools/test_introduction_matches_readme.py
"""
from __future__ import annotations

import re
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

FENCE_RE = re.compile(r"```rust\n(.*?)\n```", re.DOTALL)


def first_rust_block(text: str) -> str:
    match = FENCE_RE.search(text)
    if match is None:
        raise AssertionError("no ```rust fenced block found")
    return match.group(1)


class IntroductionMatchesReadmeTest(unittest.TestCase):
    def test_hello_world_sample_is_identical_in_both_files(self):
        readme = (REPO_ROOT / "README.md").read_text()
        introduction = (REPO_ROOT / "site" / "src" / "introduction.md").read_text()
        self.assertEqual(first_rust_block(readme), first_rust_block(introduction))


if __name__ == "__main__":
    unittest.main()
