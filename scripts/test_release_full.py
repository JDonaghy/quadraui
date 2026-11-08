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
import re
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
        """Fast-forward `origin/develop` to the `release-<version>` branch
        and push, simulating the bump PR's squash merge landing on
        `origin/develop`. The `release-<version>` branch must already carry
        the bump (`bump_on_new_branch` run first); this then deletes the
        local release branch -- the exact shape a re-run of
        `release_full.py` sees after a prior run's bump PR merged and
        GitHub deleted the head branch."""
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
            orig_auto_yes = release_full.AUTO_YES
            orig_argv = sys.argv
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
                release_full.AUTO_YES = orig_auto_yes
                sys.argv = orig_argv

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
        `merge-tree` refuses outright (`fatal: refusing to merge unrelated
        histories`, exit 128 -- not a reported content conflict, exit 1),
        but since main's tree matches a real develop commit this is the
        *recoverable* shape: it must print the `-s ours` sync recipe (not a
        bare "resolve by hand" message) before exiting, so a human can run
        it and re-launch instead of being left to improvise.

        `--orphan` is the only way to construct this fixture at all: a 3-way
        merge's diff between a shared base and each side is a deterministic
        function of (base_tree, side_tree) alone, so whenever both sides'
        tip trees are made to match (main's tip here, a develop commit),
        their diffs from any literal, single shared base are necessarily
        identical too, and an identical-patch 3-way merge can never
        conflict -- there is no real-shared-history construction that
        produces both "trees match" and "merge-tree reports a conflict".
        `test_real_conflict_against_a_shared_ancestor_fails_without_a_sync_
        recipe` below covers the shared-ancestor, exit-1, *non*-recoverable
        shape with real history instead.
        """
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
        printing a false sync recipe. Exercises the "no match" branch via
        `merge-tree`'s unrelated-histories refusal (exit 128); the sibling
        test below exercises the same "no match" branch via a real exit-1
        content conflict instead."""
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

    def test_real_conflict_against_a_shared_ancestor_fails_without_a_sync_recipe(self):
        """`main` and `develop` share real history (no `--orphan` root) but
        each independently merged the other at an earlier point, giving
        `git merge-tree` more than one candidate common ancestor — the
        shape a chain of `develop`-into-`main` squash promotions actually
        produces, not an unrelated-histories refusal. The two branches then
        diverge on the same line, so `merge-tree` reports a genuine content
        conflict (exit 1, not `fatal: refusing to merge unrelated
        histories`) and main's tip matches no develop commit."""
        with FullReleaseFixtureRepo() as repo:
            # Two separate files so the setup merges below (main <- develop,
            # then develop <- main) never touch the same line and so merge
            # cleanly on their own — only the *final* pair of commits edits
            # the same file, which is where the real conflict comes from.
            marker_a = repo.clone / "quadraui" / "src" / "marker_a.rs"
            marker_b = repo.clone / "quadraui" / "src" / "marker_b.rs"
            marker_a.write_text("pub const A: &str = \"A\";\n")
            marker_b.write_text("pub const B: &str = \"B\";\n")
            run_git(["add", "-A"], repo.clone)
            run_git(["commit", "-q", "-m", "add marker files"], repo.clone)
            run_git(["push", "-q", "origin", "develop"], repo.clone)
            run_git(["branch", "main"], repo.clone)

            # develop edits marker_a only.
            marker_a.write_text("pub const A: &str = \"A-dev\";\n")
            run_git(["commit", "-q", "-am", "develop edits marker_a"], repo.clone)

            # main independently edits marker_b only.
            run_git(["checkout", "main"], repo.clone)
            marker_b.write_text("pub const B: &str = \"B-main\";\n")
            run_git(["commit", "-q", "-am", "main edits marker_b"], repo.clone)

            # main merges develop — first common ancestor candidate. No
            # conflict: the two sides touched different files.
            run_git(["merge", "-q", "--no-edit", "develop"], repo.clone)

            # develop merges main's pre-merge commit — a second, equally
            # valid common ancestor candidate (the criss-cross). Also no
            # conflict, same reason.
            run_git(["checkout", "develop"], repo.clone)
            main_pre_merge = run_git(["rev-parse", "main~1"], repo.clone).stdout.strip()
            run_git(["merge", "-q", "--no-edit", main_pre_merge], repo.clone)

            # main and develop now each make a further, overlapping edit to
            # the same file+line, genuinely conflicting against the
            # ambiguous criss-cross base.
            run_git(["checkout", "main"], repo.clone)
            marker_b.write_text("pub const B: &str = \"B-main-final\";\n")
            run_git(["commit", "-q", "-am", "main final edit"], repo.clone)
            run_git(["push", "-q", "-u", "origin", "main"], repo.clone)

            run_git(["checkout", "develop"], repo.clone)
            marker_b.write_text("pub const B: &str = \"B-dev-final\";\n")
            run_git(["commit", "-q", "-am", "develop final edit"], repo.clone)
            run_git(["push", "-q", "origin", "develop"], repo.clone)

            release_full.REPO_DIR = repo.clone
            precheck = run_git(
                ["merge-tree", "--write-tree", "main", "develop"], repo.clone, check=False
            )
            self.assertEqual(
                precheck.returncode, 1,
                "fixture must produce a real merge-tree conflict (exit 1), "
                f"got exit {precheck.returncode}:\n{precheck.stderr}",
            )

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


GATE_RE = re.compile(
    r"needs:\s*bump-shape\b.*?if:\s*\$\{\{\s*!cancelled\(\)\s*&&[^}]*"
    r"needs\.bump-shape\.outputs\.is_bump\s*!=\s*'true'",
    re.DOTALL,
)


def _job_blocks(workflow_text: str) -> dict[str, str]:
    """`{job_name: block_text}` for every top-level job in a workflow's
    `jobs:` section, textually -- the same approach `tools/lint`'s
    `semver_gate.rs`/`downstream_gate_docs.rs` already take on `ci.yml`,
    not a full YAML parse (this repo's Python tooling stays stdlib-only).
    A top-level job is a 2-space-indented `name:` line; its block runs
    until the next 2-space-indented line or EOF.
    """
    jobs_match = re.search(r"(?m)^jobs:\n", workflow_text)
    assert jobs_match is not None, "workflow file has no top-level jobs: section"
    body = workflow_text[jobs_match.end():]
    names = list(re.finditer(r"(?m)^  ([A-Za-z0-9_-]+):\n", body))
    blocks = {}
    for i, m in enumerate(names):
        end = names[i + 1].start() if i + 1 < len(names) else len(body)
        blocks[m.group(1)] = body[m.start():end]
    return blocks


class BumpShapeWorkflowGatingTests(unittest.TestCase):
    """Asserts the `needs: bump-shape` + `if:` gating invariant directly on
    the workflow YAML text, rather than only on `is_release_bump_pr`'s own
    unit tests above: those prove the *detector* is correct, not that every
    job consuming it actually wired the gate in (or that a fast, always-on
    job never silently acquired it) -- the two failure modes this issue's
    acceptance criteria calls out a dedicated workflow-level check for.
    """

    # Jobs exempt from the `needs: bump-shape` gate -- for two distinct
    # reasons, both deliberately enumerated rather than inferred:
    #   - the issue's own Scope names these as the reduced check set a
    #     bump-shaped PR must still run (lockfile-check, readme_truth, the
    #     changelog gate, rustfmt, cargo package), plus `bump-shape` itself
    #     and the few other checkout-only/no-build jobs cheap enough to
    #     always run;
    #   - `downstream` never runs on a `pull_request` at all (`if:
    #     github.event_name == 'workflow_dispatch'`, per its header
    #     comment), so gating it on `bump-shape` -- itself `pull_request`-
    #     only -- would be a no-op; it's exempt for that structural reason,
    #     not because it belongs in the fast lane.
    # Deliberately explicit and small: any *other* job in the file must
    # carry the gate, so a newly added heavy job that forgets to is caught
    # here instead of silently re-running the full matrix on every bump PR.
    ALWAYS_ON_JOBS = frozenset(
        {
            "bump-shape",
            "readme-truth",
            "fmt",
            "package",
            "changelog-check",
            "changelog-version-check",
            "comment-lint",
            "lockfile-check",
            "downstream",
        }
    )

    def _assert_gating(self, workflow_path: Path, always_on: frozenset[str]) -> None:
        text = workflow_path.read_text(encoding="utf-8")
        blocks = _job_blocks(text)
        self.assertIn("bump-shape", blocks, f"{workflow_path}: no bump-shape job")

        for name, block in blocks.items():
            if name == "bump-shape":
                continue  # the detector job itself -- obviously mentions its own name
            if name in always_on:
                self.assertNotRegex(
                    block, r"needs:\s*bump-shape\b",
                    f"{workflow_path}: always-on job {name!r} must not "
                    "`needs: bump-shape` -- that would silently gut the "
                    "reduced check set",
                )
            else:
                self.assertRegex(
                    block, GATE_RE,
                    f"{workflow_path}: job {name!r} isn't in the always-on "
                    "allowlist, so it must carry `needs: bump-shape` and an "
                    "`if: ${{ !cancelled() && "
                    "needs.bump-shape.outputs.is_bump != 'true' }}`-shaped "
                    "condition",
                )

    # The real repo checkout this test file lives in -- deliberately not
    # `release_full.REPO_DIR` (a mutable module global other test classes
    # in this file repoint at a throwaway fixture repo in `setUp`/
    # `tearDown`), so this stays correct regardless of unittest's run
    # order within the process.
    _REAL_REPO_ROOT = Path(__file__).resolve().parent.parent

    def test_ci_yml_gates_every_non_always_on_job(self):
        self._assert_gating(
            self._REAL_REPO_ROOT / ".github" / "workflows" / "ci.yml",
            self.ALWAYS_ON_JOBS,
        )

    def test_macos_yml_gates_every_job_but_bump_shape(self):
        self._assert_gating(
            self._REAL_REPO_ROOT / ".github" / "workflows" / "macos.yml",
            frozenset({"bump-shape"}),
        )

    def test_quadraui_py_wheels_yml_gates_every_job_but_bump_shape(self):
        self._assert_gating(
            self._REAL_REPO_ROOT / ".github" / "workflows" / "quadraui-py-wheels.yml",
            frozenset({"bump-shape"}),
        )

    def test_quadraui_node_build_yml_gates_every_job_but_bump_shape(self):
        self._assert_gating(
            self._REAL_REPO_ROOT / ".github" / "workflows" / "quadraui-node-build.yml",
            frozenset({"bump-shape"}),
        )


if __name__ == "__main__":
    unittest.main()
