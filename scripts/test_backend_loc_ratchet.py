#!/usr/bin/env python3
"""Tests for `scripts/backend_loc_ratchet.py` (#2373).

Stdlib-only (`unittest`), same reasoning as the repo's other script tests
(`scripts/test_release_decide.py`, `tools/test_comment_history_lint.py`): no
pytest dependency just to cover one script. Run with:

    python3 scripts/test_backend_loc_ratchet.py
"""
from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import backend_loc_ratchet as ratchet  # noqa: E402


class CountLinesTest(unittest.TestCase):
    def test_counts_lines_across_rust_files_recursively(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "sub").mkdir()
            (root / "a.rs").write_text("line1\nline2\nline3\n")
            (root / "sub" / "b.rs").write_text("line1\nline2\n")
            self.assertEqual(ratchet.count_lines(root), 5)

    def test_ignores_non_rust_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "a.rs").write_text("line1\n")
            (root / "notes.md").write_text("line1\nline2\nline3\n")
            self.assertEqual(ratchet.count_lines(root), 1)

    def test_missing_directory_counts_as_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(ratchet.count_lines(Path(tmp) / "does_not_exist"), 0)


class BuildReportTest(unittest.TestCase):
    def test_growth_past_allowance_fails(self):
        counts = {"tui": 150}
        baseline = {"tui": 100}
        any_over, report = ratchet.build_report(counts, baseline, allowance=40)
        self.assertTrue(any_over)
        self.assertIn("OVER", report)

    def test_growth_within_allowance_passes(self):
        counts = {"tui": 120}
        baseline = {"tui": 100}
        any_over, report = ratchet.build_report(counts, baseline, allowance=40)
        self.assertFalse(any_over)
        self.assertIn("ok", report)

    def test_shrink_passes_and_prints_the_suggested_update(self):
        counts = {"tui": 80}
        baseline = {"tui": 100}
        any_over, report = ratchet.build_report(counts, baseline, allowance=40)
        self.assertFalse(any_over)
        self.assertIn("shrink", report)
        self.assertIn("baseline=80", report)

    def test_no_change_passes(self):
        counts = {"tui": 100}
        baseline = {"tui": 100}
        any_over, report = ratchet.build_report(counts, baseline, allowance=40)
        self.assertFalse(any_over)

    def test_missing_baseline_entry_is_unthresholded(self):
        counts = {"tui": 999999}
        baseline: dict[str, int] = {}
        any_over, report = ratchet.build_report(counts, baseline, allowance=40)
        self.assertFalse(any_over)
        self.assertIn("no baseline", report)

    def test_one_dir_over_does_not_mask_another_dir_ok(self):
        counts = {"tui": 200, "gtk": 100}
        baseline = {"tui": 100, "gtk": 100}
        any_over, report = ratchet.build_report(counts, baseline, allowance=40)
        self.assertTrue(any_over)
        lines = {line.split()[0]: line for line in report.splitlines()}
        self.assertIn("OVER", lines["tui"])
        self.assertIn("ok", lines["gtk"])


class LoadBaselineTest(unittest.TestCase):
    def test_missing_file_is_empty_baseline(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing = Path(tmp) / "does_not_exist.json"
            self.assertEqual(ratchet.load_baseline(missing), {})

    def test_loads_json_mapping(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "baseline.json"
            path.write_text(json.dumps({"tui": 10}))
            self.assertEqual(ratchet.load_baseline(path), {"tui": 10})


class MainExitStatusTest(unittest.TestCase):
    def _make_tree(self, tmp: str, counts: dict[str, int]) -> Path:
        src_root = Path(tmp) / "src"
        for name, n in counts.items():
            d = src_root / name
            d.mkdir(parents=True)
            (d / "mod.rs").write_text("\n".join(f"line{i}" for i in range(n)) + "\n")
        return src_root

    def test_report_only_always_exits_zero_even_when_over(self):
        with tempfile.TemporaryDirectory() as tmp:
            src_root = self._make_tree(tmp, {"tui": 200})
            baseline = Path(tmp) / "baseline.json"
            baseline.write_text(json.dumps({"tui": 100}))
            rc = ratchet.main(
                [
                    "--report-only",
                    "--src-root",
                    str(src_root),
                    "--baseline",
                    str(baseline),
                    "--allowance",
                    "10",
                ]
            )
            self.assertEqual(rc, 0)

    def test_growth_past_allowance_fails_the_gate(self):
        with tempfile.TemporaryDirectory() as tmp:
            src_root = self._make_tree(tmp, {"tui": 200})
            baseline = Path(tmp) / "baseline.json"
            baseline.write_text(json.dumps({"tui": 100}))
            rc = ratchet.main(
                [
                    "--src-root",
                    str(src_root),
                    "--baseline",
                    str(baseline),
                    "--allowance",
                    "10",
                ]
            )
            self.assertEqual(rc, 1)

    def test_shrink_passes_the_gate(self):
        with tempfile.TemporaryDirectory() as tmp:
            src_root = self._make_tree(tmp, {"tui": 50})
            baseline = Path(tmp) / "baseline.json"
            baseline.write_text(json.dumps({"tui": 100}))
            rc = ratchet.main(
                [
                    "--src-root",
                    str(src_root),
                    "--baseline",
                    str(baseline),
                    "--allowance",
                    "10",
                ]
            )
            self.assertEqual(rc, 0)

    def test_escape_hatch_raising_the_baseline_passes_the_gate(self):
        # Simulates a PR that legitimately needs to grow a backend directory
        # past the old baseline + allowance: it edits the baseline JSON in
        # the same PR, so the gate passes and the increase is visible in the
        # diff, not silently ignored.
        with tempfile.TemporaryDirectory() as tmp:
            src_root = self._make_tree(tmp, {"tui": 500})
            baseline = Path(tmp) / "baseline.json"
            baseline.write_text(json.dumps({"tui": 100}))

            rc_before = ratchet.main(
                [
                    "--report-only",
                    "--src-root",
                    str(src_root),
                    "--baseline",
                    str(baseline),
                    "--allowance",
                    "10",
                ]
            )
            # Sanity check: --report-only always exits 0, so verify the
            # underlying gate would have failed via a non-report-only run.
            rc_gate_before = ratchet.main(
                [
                    "--src-root",
                    str(src_root),
                    "--baseline",
                    str(baseline),
                    "--allowance",
                    "10",
                ]
            )
            self.assertEqual(rc_before, 0)
            self.assertEqual(rc_gate_before, 1)

            # The escape hatch: raise the baseline itself.
            baseline.write_text(json.dumps({"tui": 500}))
            rc_after = ratchet.main(
                [
                    "--src-root",
                    str(src_root),
                    "--baseline",
                    str(baseline),
                    "--allowance",
                    "10",
                ]
            )
            self.assertEqual(rc_after, 0)

    def test_missing_baseline_file_is_unthresholded(self):
        with tempfile.TemporaryDirectory() as tmp:
            src_root = self._make_tree(tmp, {"tui": 999999})
            missing = Path(tmp) / "does_not_exist.json"
            rc = ratchet.main(["--src-root", str(src_root), "--baseline", str(missing)])
            self.assertEqual(rc, 0)


if __name__ == "__main__":
    unittest.main()
