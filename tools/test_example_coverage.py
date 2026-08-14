#!/usr/bin/env python3
"""Tests for `tools/example_coverage.py`'s delta-detection (#311).

Stdlib-only (`unittest`) on purpose: the repo has no Python test harness, so
this avoids adding a pytest dependency just to cover one script. Run with:

    python3 tools/test_example_coverage.py

The load-bearing case here is the rename blind spot: `git diff
--diff-filter=A` alone treats a copy-then-edit of an existing example as a
rename (`R`), not an addition (`A`), so a genuinely new, uncovered example
authored by copying an existing one (the exact workflow CLAUDE.md documents)
would silently pass `--fail-on-gap`. `new_tui_example_stems` must pass
`--no-renames` to close that hole; this test fails loudly if that flag is
ever dropped.
"""
from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import example_coverage  # noqa: E402


def run_git(args: list[str], cwd: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=True,
    )


class TempGitRepo:
    """A throwaway git repo with a `quadraui/examples/` dir, for diff tests."""

    def __init__(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.examples_dir = self.root / "quadraui" / "examples"
        self.examples_dir.mkdir(parents=True)
        run_git(["init", "-q", "-b", "main"], self.root)
        run_git(["config", "user.email", "test@example.com"], self.root)
        run_git(["config", "user.name", "Test"], self.root)
        # Git doesn't track empty directories, and an empty `examples/`
        # would make the very first commit have nothing to add. Seed a
        # harmless root-level file so `commit()` always has something to
        # record even when `examples/` itself is empty at that point.
        (self.root / "README.md").write_text("placeholder\n")

    def __enter__(self) -> "TempGitRepo":
        return self

    def __exit__(self, *exc) -> None:
        self._tmp.cleanup()

    def commit(self, message: str) -> str:
        run_git(["add", "-A"], self.root)
        run_git(["commit", "-q", "-m", message], self.root)
        return run_git(["rev-parse", "HEAD"], self.root).stdout.strip()

    def write_example(self, name: str, body: str) -> None:
        (self.examples_dir / name).write_text(body)


HSCROLL_BODY = """\
//! TUI horizontal-scroll smoke test.
#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::HScrollEditor::new())
}
"""


class NewTuiExampleStemsTest(unittest.TestCase):
    def test_plain_addition_is_detected(self):
        with TempGitRepo() as repo:
            base = repo.commit("base: empty examples dir")
            repo.write_example("tui_new.rs", HSCROLL_BODY)
            repo.commit("add tui_new.rs")

            stems = example_coverage.new_tui_example_stems(
                base, repo_root=repo.root, examples_dir=repo.examples_dir
            )
            self.assertEqual(stems, {"tui_new"})

    def test_copy_of_existing_example_is_not_a_rename_blind_spot(self):
        """Copying an existing example to a new filename (with a small edit)
        must still be reported as new by `new_tui_example_stems`, even
        though git's default rename detection would classify it as `R`
        rather than `A`.
        """
        with TempGitRepo() as repo:
            repo.write_example("tui_hscroll.rs", HSCROLL_BODY)
            base = repo.commit("base: existing uncovered example")

            # Copy with a trivial edit -- similar enough that git's default
            # (on, 50% threshold) rename detection kicks in.
            copy_body = HSCROLL_BODY + "// a new comment line\n"
            repo.write_example("tui_hscroll_copy_zzz.rs", copy_body)
            (repo.examples_dir / "tui_hscroll.rs").unlink()
            repo.commit("copy tui_hscroll.rs -> tui_hscroll_copy_zzz.rs")

            # Sanity check: confirm git really does see this as a rename
            # when rename detection isn't disabled -- otherwise this test
            # wouldn't actually be exercising the blind spot.
            status = run_git(
                ["diff", "--name-status", "-M", f"{base}...HEAD"], repo.root
            ).stdout
            self.assertIn("R", status.splitlines()[0].split()[0])

            stems = example_coverage.new_tui_example_stems(
                base, repo_root=repo.root, examples_dir=repo.examples_dir
            )
            self.assertIn(
                "tui_hscroll_copy_zzz",
                stems,
                "renamed/copied example must still count as new -- "
                "--no-renames must be passed to git diff",
            )
            # The old (still-uncovered, pre-existing) name is gone, so it
            # must not be reported as a "new" example either.
            self.assertNotIn("tui_hscroll", stems)

    def test_gtk_examples_are_excluded(self):
        with TempGitRepo() as repo:
            base = repo.commit("base: empty examples dir")
            repo.write_example("gtk_new.rs", HSCROLL_BODY)
            repo.commit("add gtk_new.rs")

            stems = example_coverage.new_tui_example_stems(
                base, repo_root=repo.root, examples_dir=repo.examples_dir
            )
            self.assertEqual(stems, set())

    def test_no_new_examples_yields_empty_set(self):
        with TempGitRepo() as repo:
            repo.write_example("tui_existing.rs", HSCROLL_BODY)
            base = repo.commit("base: one pre-existing example")
            (repo.root / "README.md").write_text("unrelated change\n")
            repo.commit("touch an unrelated file outside examples/")

            stems = example_coverage.new_tui_example_stems(
                base, repo_root=repo.root, examples_dir=repo.examples_dir
            )
            self.assertEqual(stems, set())


if __name__ == "__main__":
    unittest.main()
