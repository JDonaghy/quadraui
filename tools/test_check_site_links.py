#!/usr/bin/env python3
"""Unit tests for `tools/check_site_links.py`. Stdlib-only; run with:

    python3 tools/test_check_site_links.py
"""
from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_site_links  # noqa: E402


class BrokenLinksTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.book = Path(self.tmp.name)
        (self.book / "img").mkdir()
        (self.book / "img" / "a.png").write_bytes(b"")
        (self.book / "sub").mkdir()
        (self.book / "sub" / "page.html").write_text('<h2 id="here">x</h2><a href="../index.html#top">up</a>')

    def write_index(self, body: str) -> None:
        (self.book / "index.html").write_text(f'<div id="top"></div>{body}')

    def test_good_links_pass(self):
        self.write_index(
            '<a href="sub/page.html#here">p</a><img src="img/a.png">'
            '<a href="https://example.com/x">ext</a><a href="#top">self</a>'
        )
        self.assertEqual(check_site_links.broken_links(self.book), [])

    def test_missing_page_and_image_are_reported(self):
        self.write_index('<a href="nope.html">p</a><img src="img/missing.png">')
        problems = check_site_links.broken_links(self.book)
        self.assertEqual(len(problems), 2, problems)

    def test_missing_anchor_is_reported(self):
        self.write_index('<a href="sub/page.html#gone">p</a>')
        problems = check_site_links.broken_links(self.book)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("gone", problems[0])


if __name__ == "__main__":
    unittest.main()
