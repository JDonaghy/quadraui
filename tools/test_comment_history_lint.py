#!/usr/bin/env python3
"""Tests for `tools/comment_history_lint.py` (#1112).

Stdlib-only (`unittest`), same reasoning as `test_example_coverage.py`: no
pytest dependency just to cover one script. Run with:

    python3 tools/test_comment_history_lint.py
"""
from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import comment_history_lint as lint  # noqa: E402


class CommentTextTest(unittest.TestCase):
    def test_rust_line_comment_is_found(self):
        self.assertEqual(lint.comment_text("let x = 1; // hello", ".rs"), "// hello")

    def test_rust_doc_comment_is_found(self):
        self.assertEqual(lint.comment_text("/// does a thing", ".rs"), "/// does a thing")

    def test_rust_url_inside_comment_does_not_truncate_it(self):
        line = "// see https://example.com/foo for details"
        self.assertEqual(lint.comment_text(line, ".rs"), line[line.index("//") :])

    def test_rust_code_only_line_has_no_comment(self):
        self.assertIsNone(lint.comment_text("let x = 1;", ".rs"))

    def test_toml_comment_is_found(self):
        self.assertEqual(lint.comment_text('name = "x" # a comment', ".toml"), "# a comment")

    def test_unknown_suffix_returns_none(self):
        self.assertIsNone(lint.comment_text("// hello", ".md"))


class ScanFileTest(unittest.TestCase):
    def _scan(self, text: str, suffix: str = ".rs") -> list[lint.Finding]:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / f"sample{suffix}"
            path.write_text(text)
            return lint.scan_file(path)

    def test_issue_reference_is_flagged(self):
        findings = self._scan("// workaround for #1234\nfn f() {}\n")
        self.assertEqual(len(findings), 1)
        self.assertTrue(findings[0].is_issue_ref)
        self.assertEqual(findings[0].phrases, [])

    def test_history_phrase_is_flagged_case_insensitively(self):
        findings = self._scan("// This Used To return None\nfn f() {}\n")
        self.assertEqual(len(findings), 1)
        self.assertFalse(findings[0].is_issue_ref)
        self.assertEqual(findings[0].phrases, ["used to"])

    def test_plain_comment_is_not_flagged(self):
        findings = self._scan("// returns the current value\nfn f() {}\n")
        self.assertEqual(findings, [])

    def test_both_issue_ref_and_phrase_on_one_line_is_one_finding(self):
        findings = self._scan("// no longer needed, was added for #42\n")
        self.assertEqual(len(findings), 1)
        self.assertTrue(findings[0].is_issue_ref)
        self.assertEqual(findings[0].phrases, ["no longer"])

    def test_non_comment_line_with_hash_like_code_is_ignored_for_rust(self):
        # Rust has no '#' comment marker, so a line with a literal '#'
        # outside any '//' must not be flagged.
        findings = self._scan('let s = "#1234 not a comment";\n')
        self.assertEqual(findings, [])


class GroupForTest(unittest.TestCase):
    def test_top_level_src_file_is_src_group(self):
        root = Path("/repo")
        path = root / "quadraui" / "src" / "lib.rs"
        self.assertEqual(lint.group_for(path, repo_root=root), "src")

    def test_tabled_subdir_file_is_its_own_group(self):
        root = Path("/repo")
        path = root / "quadraui" / "src" / "tui" / "mod.rs"
        self.assertEqual(lint.group_for(path, repo_root=root), "tui")

    def test_untabled_subdir_falls_back_to_src(self):
        root = Path("/repo")
        path = root / "quadraui" / "src" / "diff" / "mod.rs"
        self.assertEqual(lint.group_for(path, repo_root=root), "src")

    def test_root_cargo_toml_is_src_group(self):
        root = Path("/repo")
        self.assertEqual(lint.group_for(root / "Cargo.toml", repo_root=root), "src")

    def test_quadraui_cargo_toml_is_src_group(self):
        root = Path("/repo")
        path = root / "quadraui" / "Cargo.toml"
        self.assertEqual(lint.group_for(path, repo_root=root), "src")

    def test_example_file_is_other_group(self):
        root = Path("/repo")
        path = root / "quadraui" / "examples" / "hello.rs"
        self.assertEqual(lint.group_for(path, repo_root=root), "other")


class MainExitStatusTest(unittest.TestCase):
    def test_report_only_always_exits_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            thresholds = Path(tmp) / "thresholds.json"
            thresholds.write_text("{}")
            rc = lint.main(["--report-only", "--thresholds", str(thresholds)])
            self.assertEqual(rc, 0)

    def test_missing_thresholds_file_is_treated_as_unthresholded(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing = Path(tmp) / "does_not_exist.json"
            rc = lint.main(["--thresholds", str(missing)])
            self.assertEqual(rc, 0)

    def test_group_over_its_threshold_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            thresholds = Path(tmp) / "thresholds.json"
            # Every real group has far more than 0 flagged lines today, so a
            # 0 threshold on every gated group guarantees at least one is
            # over -- this exercises the gate without hardcoding today's
            # repo-wide counts into a test (which would churn every time the
            # baseline is ratcheted down by a module pass).
            thresholds.write_text(
                '{"src": 0, "compose": 0, "primitives": 0, "tui": 0, '
                '"gtk": 0, "macos": 0, "win": 0}'
            )
            rc = lint.main(["--thresholds", str(thresholds)])
            self.assertEqual(rc, 1)


if __name__ == "__main__":
    unittest.main()
