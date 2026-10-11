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


class FixBodyTest(unittest.TestCase):
    def test_leading_issue_tag_is_stripped_and_capitalized(self):
        body = "#1630 review: tightened the bound check"
        self.assertEqual(lint.fix_body(body), "Tightened the bound check")

    def test_ref_only_body_is_deleted(self):
        self.assertIsNone(lint.fix_body("see #883"))

    def test_safety_comment_is_left_unchanged(self):
        body = "SAFETY: caller guarantees the pointer in #42 is valid"
        self.assertEqual(lint.fix_body(body), body)

    def test_leading_paren_ref_is_left_for_hand_rewrite(self):
        # No preceding text on the line -- same caveat as
        # code-coordinator's comment_cruft.py: at the start of a line a
        # parenthetical can't be told apart from load-bearing content.
        body = "(#1005) keep this guard"
        self.assertEqual(lint.fix_body(body), body)

    def test_trailing_paren_ref_after_text_is_stripped(self):
        self.assertEqual(lint.fix_body("keep this guard (#1005)"), "keep this guard")

    def test_history_prose_is_left_unchanged(self):
        body = "this used to return early before #99 landed"
        self.assertEqual(lint.fix_body(body), body)


class FixFileTest(unittest.TestCase):
    def _write(self, tmp: str, text: str, suffix: str = ".rs") -> Path:
        path = Path(tmp) / f"sample{suffix}"
        path.write_text(text)
        return path

    def test_own_line_leading_tag_comment_is_rewritten_in_place(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write(tmp, "fn f() {\n    // #1630 review: tightened the bound check\n    1\n}\n")
            changed, remaining = lint.fix_file(path, {2})
            self.assertTrue(changed)
            self.assertEqual(remaining, [])
            self.assertIn("// Tightened the bound check\n", path.read_text())

    def test_ref_only_own_line_comment_is_deleted(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write(tmp, "fn f() {\n    // see #883\n    1\n}\n")
            changed, remaining = lint.fix_file(path, {2})
            self.assertTrue(changed)
            self.assertEqual(remaining, [])
            text = path.read_text()
            self.assertNotIn("#883", text)
            self.assertEqual(text.count("\n"), 3)

    def test_trailing_comment_on_code_keeps_the_code(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write(tmp, "let x = 1; // see #883\n")
            changed, remaining = lint.fix_file(path, {1})
            self.assertTrue(changed)
            self.assertEqual(remaining, [])
            self.assertEqual(path.read_text(), "let x = 1;\n")

    def test_history_prose_is_reported_as_remaining_and_untouched(self):
        with tempfile.TemporaryDirectory() as tmp:
            text = "fn f() {\n    // this used to return early before #99 landed\n    1\n}\n"
            path = self._write(tmp, text)
            changed, remaining = lint.fix_file(path, {2})
            self.assertFalse(changed)
            self.assertEqual([r[0] for r in remaining], [2])
            self.assertEqual(path.read_text(), text)

    def test_toml_ref_only_comment_is_deleted(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write(tmp, 'name = "x"\n# see #883\nversion = "1"\n', suffix=".toml")
            changed, remaining = lint.fix_file(path, {2})
            self.assertTrue(changed)
            self.assertEqual(remaining, [])
            text = path.read_text()
            self.assertNotIn("#883", text)

    def test_toml_leading_tag_comment_is_rewritten_in_place(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write(tmp, '# #1630 review: tightened the bound check\nname = "x"\n', suffix=".toml")
            changed, remaining = lint.fix_file(path, {1})
            self.assertTrue(changed)
            self.assertEqual(remaining, [])
            self.assertIn("# Tightened the bound check\n", path.read_text())


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

    def test_fix_with_explicit_path_cleans_a_scratch_file_under_the_real_tree(self):
        # --fix walks the real SCAN_ROOTS (iter_scan_files isn't
        # parameterized the way group_for is), so this plants a throwaway
        # file under the real repo's quadraui/src/ rather than a fake root,
        # and removes it again whether or not the assertions pass.
        scratch = lint.REPO_ROOT / "quadraui" / "src" / "zz_fix_test_scratch.rs"
        scratch.write_text("fn f() {\n    // #1630 review: tightened the bound check\n    1\n}\n")
        try:
            rc = lint.main(["--fix", "--report-only", str(scratch)])
            self.assertEqual(rc, 0)
            self.assertIn("// Tightened the bound check\n", scratch.read_text())
        finally:
            scratch.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
