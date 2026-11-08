#!/usr/bin/env python3
"""Tests for `scripts/release_full.py`'s resume logic (#1694).

Stdlib-only (`unittest`), matching `scripts/test_release.py`'s own rationale
for avoiding a pytest dependency. Run with:

    python3 scripts/test_release_full.py

Focuses on the "release branch already exists" resume path:
`find_and_checkout_bumped_release_branch` must find a `release-<version>`
branch — local or remote-only — whose tip already carries the version bump,
and check it out, so `prepare()` can resume from *any* starting branch
instead of calling `release.py prepare`'s `checkout -b`, which aborts when
the branch already exists.

Exercises real `git` subprocess calls against a throwaway local "remote"
(a bare repo) plus a clone, the same shape `test_release_decide.py`'s
`TempRemoteRepo` uses — `find_and_checkout_bumped_release_branch` shells out
to `git ls-remote`/`git fetch`, so a mocked remote wouldn't cover the actual
code path.
"""
from __future__ import annotations

import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_full  # noqa: E402


def _hermetic_git_env() -> dict[str, str]:
    """See `scripts/test_release_decide.py`'s `_hermetic_git_env` for why."""
    env = dict(os.environ)
    env["GIT_CONFIG_GLOBAL"] = "/dev/null"
    env["GIT_CONFIG_SYSTEM"] = "/dev/null"
    return env


def run_git(args: list[str], cwd: Path, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=check,
        env=_hermetic_git_env(),
    )


class FullReleaseFixtureRepo:
    """A bare "origin" repo plus a clone laid out like quadraui, on `develop`.

    Mirrors `test_release.py`'s `ReleaseFixtureRepo`, but adds a real
    `origin` remote so `find_and_checkout_bumped_release_branch`'s
    `git ls-remote`/`git fetch` calls have something to query.
    """

    def __init__(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.remote = self.root / "remote.git"
        self.clone = self.root / "clone"

        run_git(["init", "-q", "--bare", "-b", "develop", str(self.remote)], self.root)
        run_git(["clone", "-q", str(self.remote), str(self.clone)], self.root)
        run_git(["config", "user.email", "test@example.com"], self.clone)
        run_git(["config", "user.name", "Test"], self.clone)

        (self.clone / "quadraui" / "src").mkdir(parents=True)
        (self.clone / "quadraui" / "Cargo.toml").write_text(
            '[package]\nname = "quadraui"\nversion = "0.1.0"\nedition = "2021"\n'
        )
        (self.clone / "quadraui" / "src" / "lib.rs").write_text(
            "//! ## Status\n"
            "//!\n"
            "//! `0.1.x` — pre-1.0, published on crates.io.\n"
            "pub fn hello() {}\n"
        )
        (self.clone / "README.md").write_text(
            "# quadraui\n"
            "\n"
            "## Status\n"
            "\n"
            "`0.1.x` — pre-1.0, published on crates.io.\n"
            "\n"
            "## Getting started\n"
            "\n"
            "```toml\n"
            'quadraui = { version = "0.1", features = ["tui", "gtk"] }\n'
            "```\n"
        )
        (self.clone / "CHANGELOG.md").write_text(
            "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- a new thing\n"
        )
        run_git(["add", "-A"], self.clone)
        run_git(["commit", "-q", "-m", "init"], self.clone)
        run_git(["push", "-q", "origin", "develop"], self.clone)

    def __enter__(self) -> "FullReleaseFixtureRepo":
        return self

    def __exit__(self, *exc) -> None:
        self._tmp.cleanup()

    def current_branch(self) -> str:
        return run_git(["branch", "--show-current"], self.clone).stdout.strip()

    def bump_on_new_branch(self, version: str, *, push: bool = False) -> None:
        """Simulate a prior `release.py prepare` run: create
        `release-<version>`, bump `quadraui/Cargo.toml` plus the
        README.md/lib.rs version strings `check_readme_truth_version_strings`
        enforces and `CHANGELOG.md`'s `[Unreleased]` -> `[<version>]`
        retitle, commit — optionally pushing it to `origin` and returning to
        `develop` locally, to exercise the remote-only branch path."""
        branch = f"release-{version}"
        run_git(["checkout", "-b", branch], self.clone)

        cargo_toml = self.clone / "quadraui" / "Cargo.toml"
        cargo_toml.write_text(
            cargo_toml.read_text().replace('version = "0.1.0"', f'version = "{version}"')
        )

        major, minor, _ = version.split(".")
        status = f"{major}.{minor}.x"
        readme = self.clone / "README.md"
        readme.write_text(
            readme.read_text()
            .replace("`0.1.x`", f"`{status}`")
            .replace('version = "0.1"', f'version = "{major}.{minor}"')
        )
        lib_rs = self.clone / "quadraui" / "src" / "lib.rs"
        lib_rs.write_text(lib_rs.read_text().replace("`0.1.x`", f"`{status}`"))

        changelog = self.clone / "CHANGELOG.md"
        changelog.write_text(
            changelog.read_text().replace(
                "## [Unreleased]\n\n### Added\n\n- a new thing\n",
                f"## [Unreleased]\n\n## [{version}] - 2026-11-07\n\n"
                "### Added\n\n- a new thing\n",
            )
        )

        run_git(["commit", "-a", "-m", f"Release v{version}"], self.clone)
        if push:
            run_git(["push", "-q", "-u", "origin", branch], self.clone)
            run_git(["checkout", "develop"], self.clone)
            run_git(["branch", "-D", branch], self.clone)

    def merge_bump_into_develop(self, version: str) -> None:
        """Simulate the bump PR's squash merge landing on `origin/develop`
        the `release-<version>` branch must already carry the
        bump (`bump_on_new_branch` run first), and this fast-forwards
        `develop` to it and pushes, then deletes the local release branch
        -- the exact shape a re-run of `release_full.py` sees after a prior
        run's bump PR merged and GitHub deleted the head branch."""
        branch = f"release-{version}"
        run_git(["checkout", "develop"], self.clone)
        run_git(["merge", "--ff-only", branch], self.clone)
        run_git(["push", "-q", "origin", "develop"], self.clone)
        run_git(["branch", "-D", branch], self.clone)


class ResumeBranchDetectionTests(unittest.TestCase):
    def setUp(self):
        self._orig_repo_dir = release_full.REPO_DIR

    def tearDown(self):
        release_full.REPO_DIR = self._orig_repo_dir

    def test_no_branch_found_returns_false(self):
        with FullReleaseFixtureRepo() as repo:
            release_full.REPO_DIR = repo.clone
            self.assertFalse(
                release_full.find_and_checkout_bumped_release_branch("0.2.0")
            )
            self.assertEqual(repo.current_branch(), "develop")

    def test_finds_and_checks_out_local_bumped_branch(self):
        with FullReleaseFixtureRepo() as repo:
            repo.bump_on_new_branch("0.2.0")
            run_git(["checkout", "develop"], repo.clone)
            self.assertEqual(repo.current_branch(), "develop")

            release_full.REPO_DIR = repo.clone
            found = release_full.find_and_checkout_bumped_release_branch("0.2.0")
            self.assertTrue(found)
            self.assertEqual(repo.current_branch(), "release-0.2.0")

    def test_local_branch_with_wrong_bump_is_not_reused(self):
        with FullReleaseFixtureRepo() as repo:
            repo.bump_on_new_branch("0.3.0")
            run_git(
                ["branch", "-m", "release-0.3.0", "release-0.2.0"], repo.clone
            )
            run_git(["checkout", "develop"], repo.clone)

            release_full.REPO_DIR = repo.clone
            found = release_full.find_and_checkout_bumped_release_branch("0.2.0")
            self.assertFalse(found)
            # Left on develop — no partial checkout of the mismatched branch.
            self.assertEqual(repo.current_branch(), "develop")

    def test_finds_and_checks_out_remote_only_bumped_branch(self):
        """The branch `release_full.py` created got pushed, then the local
        copy went away (e.g. a fresh checkout resuming on another machine,
        or a re-run that went back to develop and deleted it) — only
        `origin/release-<version>` still has it."""
        with FullReleaseFixtureRepo() as repo:
            repo.bump_on_new_branch("0.2.0", push=True)
            self.assertEqual(repo.current_branch(), "develop")
            local = run_git(
                ["branch", "--list", "release-0.2.0"], repo.clone
            ).stdout.strip()
            self.assertEqual(local, "")

            release_full.REPO_DIR = repo.clone
            found = release_full.find_and_checkout_bumped_release_branch("0.2.0")
            self.assertTrue(found)
            self.assertEqual(repo.current_branch(), "release-0.2.0")

    def test_prepare_resumes_from_develop_after_branch_already_exists(self):
        """End-to-end: `prepare()` itself, called from `develop` after a
        `release-<version>` branch with the bump already exists, checks it
        out instead of re-invoking `release.py prepare` (whose `checkout -b`
        would abort)."""
        with FullReleaseFixtureRepo() as repo:
            repo.bump_on_new_branch("0.2.0")
            run_git(["checkout", "develop"], repo.clone)

            release_full.REPO_DIR = repo.clone
            release_full.prepare("0.2.0")
            self.assertEqual(repo.current_branch(), "release-0.2.0")


class VersionAlreadyLandedOnDevelopTests(unittest.TestCase):
    """`version_already_landed_on_develop`: a re-run must recognize
    a bump PR that already merged into `origin/develop` and skip straight
    to `promote`, instead of re-running `prepare`/`bump_pr` and opening a
    second, empty bump PR."""

    def setUp(self):
        self._orig_repo_dir = release_full.REPO_DIR

    def tearDown(self):
        release_full.REPO_DIR = self._orig_repo_dir

    def test_false_before_any_bump(self):
        with FullReleaseFixtureRepo() as repo:
            release_full.REPO_DIR = repo.clone
            self.assertFalse(
                release_full.version_already_landed_on_develop("0.2.0")
            )

    def test_false_while_bump_only_lives_on_the_release_branch(self):
        """The bump exists, but only on `release-<version>` — its PR into
        `develop` hasn't merged yet, so a re-run must still go through the
        ordinary `prepare`/`bump_pr` resume path, not skip it."""
        with FullReleaseFixtureRepo() as repo:
            repo.bump_on_new_branch("0.2.0", push=True)

            release_full.REPO_DIR = repo.clone
            self.assertFalse(
                release_full.version_already_landed_on_develop("0.2.0")
            )

    def test_true_after_the_bump_pr_merges_into_develop(self):
        with FullReleaseFixtureRepo() as repo:
            repo.bump_on_new_branch("0.2.0")
            repo.merge_bump_into_develop("0.2.0")

            release_full.REPO_DIR = repo.clone
            self.assertTrue(
                release_full.version_already_landed_on_develop("0.2.0")
            )

    def test_main_skips_prepare_and_bump_pr_when_already_landed(self):
        """End-to-end: `main()`'s own resume branch checks out `develop`
        and heads straight to `promote`/`publish` — it never calls
        `prepare()` (which would try `release.py prepare` again) or
        `bump_pr()` (which would open a second bump PR)."""
        with FullReleaseFixtureRepo() as repo:
            repo.bump_on_new_branch("0.2.0")
            repo.merge_bump_into_develop("0.2.0")
            run_git(["checkout", "develop"], repo.clone)

            release_full.REPO_DIR = repo.clone
            calls: list[str] = []
            orig_prepare = release_full.prepare
            orig_bump_pr = release_full.bump_pr
            orig_promote = release_full.promote
            orig_publish = release_full.publish
            release_full.prepare = lambda v: calls.append("prepare")
            release_full.bump_pr = lambda v: calls.append("bump_pr")
            release_full.promote = lambda v: calls.append("promote")
            release_full.publish = lambda v: calls.append("publish")
            try:
                release_full.AUTO_YES = True
                sys.argv = ["release_full.py", "0.2.0", "--yes"]
                release_full.main()
            finally:
                release_full.prepare = orig_prepare
                release_full.bump_pr = orig_bump_pr
                release_full.promote = orig_promote
                release_full.publish = orig_publish

            self.assertNotIn("prepare", calls)
            self.assertNotIn("bump_pr", calls)
            self.assertEqual(calls, ["promote", "publish"])
            self.assertEqual(repo.current_branch(), "develop")


class MainDevelopDivergenceGuardTests(unittest.TestCase):
    """`check_main_develop_divergence`: preflight must catch a
    `main`/`develop` history split before the long CI waits start, not
    after a promotion PR's `merge-tree` conflict surprises a human partway
    through."""

    def setUp(self):
        self._orig_repo_dir = release_full.REPO_DIR

    def tearDown(self):
        release_full.REPO_DIR = self._orig_repo_dir

    def _push_main(self, repo: "FullReleaseFixtureRepo") -> None:
        run_git(["branch", "main"], repo.clone)
        run_git(["push", "-q", "origin", "main"], repo.clone)

    def test_clean_merge_passes_without_a_fail(self):
        with FullReleaseFixtureRepo() as repo:
            self._push_main(repo)
            # develop has moved ahead of main with an ordinary commit —
            # merge-tree reports no conflicts.
            (repo.clone / "quadraui" / "src" / "extra.rs").write_text("pub fn x() {}\n")
            run_git(["add", "-A"], repo.clone)
            run_git(["commit", "-q", "-m", "an ordinary change"], repo.clone)
            run_git(["push", "-q", "origin", "develop"], repo.clone)

            release_full.REPO_DIR = repo.clone
            release_full.check_main_develop_divergence()  # must not raise/exit

    def test_identical_tree_diverged_history_prints_sync_recipe(self):
        """`main` is a squash-merged, same-tree-but-different-history copy
        of a develop commit — a recoverable, same-tree-different-history shape.
        `merge-tree` reports conflicts, but since main's tree matches a
        real develop commit this is the *recoverable* shape: it must print
        the `-s ours` sync recipe (not a bare "resolve by hand" message)
        before exiting, so a human can run it and re-launch instead of
        being left to improvise."""
        with FullReleaseFixtureRepo() as repo:
            # Diverge main from develop's history (an unrelated root commit
            # on main with develop's current tree squashed onto it), so
            # merge-tree sees no common ancestor content to cleanly merge
            # but the resulting tree is identical to a develop commit.
            run_git(["checkout", "--orphan", "main"], repo.clone)
            run_git(["commit", "-q", "-m", "squashed copy of develop"], repo.clone)
            run_git(["push", "-q", "-u", "origin", "main"], repo.clone)
            run_git(["checkout", "develop"], repo.clone)

            release_full.REPO_DIR = repo.clone
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                with self.assertRaises(SystemExit):
                    release_full.check_main_develop_divergence()
            self.assertIn("git merge -s ours origin/main", buf.getvalue())

    def test_genuinely_diverged_content_fails_without_a_sync_recipe(self):
        """`main` and `develop` have real, unrelated content — no develop
        commit's tree matches main's — so this must hard-fail without
        printing a false sync recipe."""
        with FullReleaseFixtureRepo() as repo:
            run_git(["checkout", "--orphan", "main"], repo.clone)
            (repo.clone / "only-on-main.txt").write_text("unrelated content\n")
            run_git(["add", "-A"], repo.clone)
            run_git(["commit", "-q", "-m", "genuinely unrelated main"], repo.clone)
            run_git(["push", "-q", "-u", "origin", "main"], repo.clone)
            run_git(["checkout", "develop"], repo.clone)

            release_full.REPO_DIR = repo.clone
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                with self.assertRaises(SystemExit):
                    release_full.check_main_develop_divergence()
            self.assertNotIn("git merge -s ours", buf.getvalue())


class CheckTimeoutTests(unittest.TestCase):
    def test_default_check_timeout_is_raised_past_900(self):
        self.assertGreater(release_full.CHECK_TIMEOUT, 900)

    def test_wait_for_checks_uses_module_default_when_timeout_omitted(self):
        orig = release_full.CHECK_TIMEOUT
        try:
            release_full.CHECK_TIMEOUT = 0
            # status "none" with an already-elapsed deadline falls straight
            # through to the timeout `fail()` — confirms the function reads
            # the *current* module global rather than a value baked in at
            # `def` time.
            release_full.pr_checks_status = lambda number: ("none", [])
            with self.assertRaises(SystemExit):
                release_full.wait_for_checks(1, "test")
        finally:
            release_full.CHECK_TIMEOUT = orig
            del release_full.pr_checks_status

    def test_check_timeout_cli_flag_overrides_default(self):
        result = subprocess.run(
            [sys.executable, str(Path(release_full.__file__)), "--help"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--check-timeout", result.stdout)


if __name__ == "__main__":
    unittest.main()
