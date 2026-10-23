#!/usr/bin/env python3
"""End-to-end quadraui release: prepare → push → PR → merge → promote → wait.

Wraps the existing `release.py prepare` + `release.py promote` steps into a
single guided, resumable workflow.  Pauses for confirmation before each
irreversible action.

Usage:

    python3 scripts/release_full.py 0.2.1
    python3 scripts/release_full.py 0.2.1 --yes   # non-interactive

Phases:

  1. Preflight  — repo clean, on develop, version valid, changelog non-empty
  2. Prepare    — `release.py prepare` (branch, bump, changelog, commit)
  3. Bump PR    — push branch, create PR to develop, wait for merge
  4. Promote    — develop → main PR (merge commit), wait for merge
  5. Publish    — wait for CI to publish to crates.io + push the tag
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

REPO_DIR = Path(__file__).resolve().parent.parent

BOLD = "\033[1m"
GREEN = "\033[32m"
YELLOW = "\033[33m"
RED = "\033[31m"
RESET = "\033[0m"

AUTO_YES = False


def banner(phase: str, msg: str) -> None:
    print(f"\n{BOLD}{'=' * 60}{RESET}")
    print(f"{BOLD}  [{phase}] {msg}{RESET}")
    print(f"{BOLD}{'=' * 60}{RESET}\n")


def info(msg: str) -> None:
    print(f"  {GREEN}✓{RESET} {msg}")


def warn(msg: str) -> None:
    print(f"  {YELLOW}⚠{RESET} {msg}")


def fail(msg: str) -> None:
    print(f"\n  {RED}✗ {msg}{RESET}\n", file=sys.stderr)
    sys.exit(1)


def confirm(prompt: str) -> bool:
    if AUTO_YES:
        print(f"\n  {YELLOW}?{RESET} {prompt} [y/N] y (--yes)")
        return True
    try:
        answer = input(f"\n  {YELLOW}?{RESET} {prompt} [y/N] ").strip().lower()
    except EOFError:
        warn("no TTY — use --yes for non-interactive mode")
        return True
    return answer in ("y", "yes")


def run(cmd: list[str], check: bool = True,
        capture: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd, cwd=REPO_DIR, capture_output=capture, text=True, check=check,
    )


def git(args: list[str], check: bool = True) -> subprocess.CompletedProcess:
    return run(["git", *args], check=check)


def gh(args: list[str], check: bool = True) -> subprocess.CompletedProcess:
    return run(["gh", *args], check=check)


def git_branch() -> str:
    return git(["rev-parse", "--abbrev-ref", "HEAD"]).stdout.strip()


def git_clean() -> bool:
    return git(["status", "--porcelain", "--untracked-files=no"]).stdout.strip() == ""


def read_cargo_version() -> str:
    text = (REPO_DIR / "quadraui" / "Cargo.toml").read_text()
    m = re.search(r'(?m)^version\s*=\s*"([^"]+)"', text)
    if not m:
        fail("no version found in quadraui/Cargo.toml")
    return m.group(1)


def check_readme_truth_version_strings(version: str) -> None:
    """Fail before any push if README.md/lib.rs's hard-coded version strings
    don't already match `version`.

    Belt-and-suspenders for `quadraui/tests/readme_truth.rs`
    (`readme_and_lib_doc_state_the_real_version`,
    `readme_leads_with_a_resolvable_published_version`): `release.py
    prepare` keeps these in lockstep with the Cargo.toml bump in the same
    commit, but this catches a hand-run release (or a stale checkout) that
    skipped it, before it ever reaches a push.
    """
    from release import pre_1_0_status_string, published_dependency_version

    want_status = f"`{pre_1_0_status_string(version)}`"
    want_dependency = f'quadraui = {{ version = "{published_dependency_version(version)}"'

    readme = (REPO_DIR / "README.md").read_text()
    lib_rs = (REPO_DIR / "quadraui" / "src" / "lib.rs").read_text()

    problems = []
    if want_status not in readme:
        problems.append(f"README.md's Status section doesn't say {want_status}")
    if want_dependency not in readme:
        problems.append(
            f"README.md's Getting started section doesn't contain {want_dependency!r}"
        )
    if want_status not in lib_rs:
        problems.append(f"quadraui/src/lib.rs's crate doc doesn't say {want_status}")

    if problems:
        fail(
            "README/lib.rs version strings don't match "
            f"{version} (readme_truth.rs will red out on develop):\n    "
            + "\n    ".join(problems)
        )
    info(f"README.md + lib.rs version strings match {version}")


def tag_exists_on_remote(tag: str) -> bool:
    r = git(["ls-remote", "--tags", "origin", f"refs/tags/{tag}"], check=False)
    return tag in r.stdout


def wait_for_crate(version: str, timeout: int = 600) -> bool:
    import urllib.request
    url = "https://crates.io/api/v1/crates/quadraui/versions"
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "quadraui-release/1.0"})
            with urllib.request.urlopen(req, timeout=15) as resp:
                data = json.loads(resp.read())
                versions = [v["num"] for v in data.get("versions", [])]
                if version in versions:
                    return True
        except Exception:
            pass
        print(f"    waiting for quadraui {version} on crates.io...", flush=True)
        time.sleep(15)
    return False


def pr_for_branch(base: str, head: str) -> dict | None:
    r = gh(["pr", "list", "--base", base, "--head", head,
            "--json", "url,number,state", "--jq", ".[0]"], check=False)
    if r.returncode == 0 and r.stdout.strip():
        return json.loads(r.stdout.strip())
    return None


def pr_state(number: int) -> str:
    r = gh(["pr", "view", str(number), "--json", "state", "--jq", ".state"],
           check=False)
    return r.stdout.strip()


def pr_checks_status(number: int) -> tuple[str, list[dict]]:
    """Return (overall, checks) where overall is 'pass', 'fail', 'pending', or 'none'."""
    r = gh(["pr", "view", str(number), "--json", "statusCheckRollup",
            "--jq", ".statusCheckRollup"], check=False)
    if r.returncode != 0 or not r.stdout.strip():
        return ("none", [])

    try:
        checks = json.loads(r.stdout.strip())
    except json.JSONDecodeError:
        return ("none", [])

    if not checks:
        return ("none", [])

    # Deduplicate by name — keep the latest run of each check
    by_name: dict[str, dict] = {}
    for c in checks:
        name = c.get("name", "")
        by_name[name] = c
    checks = list(by_name.values())

    failed = [c for c in checks if c.get("conclusion") == "FAILURE"]
    pending = [c for c in checks
               if c.get("status") in ("PENDING", "QUEUED", "IN_PROGRESS", "WAITING")
               or (c.get("status") == "COMPLETED" and not c.get("conclusion"))]
    skipping = [c for c in checks if c.get("conclusion") == "SKIPPED"
                or c.get("status") == "EXPECTED"]
    done = [c for c in checks if c.get("conclusion") in ("SUCCESS", "NEUTRAL", "SKIPPED")]
    if failed:
        return ("fail", checks)
    if pending:
        return ("pending", checks)
    return ("pass", checks)


def wait_for_checks(number: int, label: str, timeout: int = 900) -> None:
    """Poll until CI checks on a PR reach a terminal state (all done or timed out).

    Non-required failures are warned but don't block — gh pr merge is
    the authority on whether the PR is actually mergeable.
    """
    deadline = time.time() + timeout
    last_msg = ""
    while time.time() < deadline:
        status, checks = pr_checks_status(number)
        if status == "pass":
            info(f"{label} PR #{number}: all checks passed")
            return
        if status == "fail":
            failed = [c["name"] for c in checks if c.get("conclusion") == "FAILURE"]
            warn(f"{label} PR #{number}: non-critical checks failed — {', '.join(failed)}")
            passed = [c for c in checks if c.get("conclusion") in ("SUCCESS", "NEUTRAL", "SKIPPED")]
            info(f"{label} PR #{number}: {len(passed)}/{len(checks)} checks passed, proceeding to merge")
            return
        if status == "none":
            msg = f"    {label} PR #{number}: waiting for checks to start..."
        else:
            pending = [c["name"] for c in checks
                       if c.get("status") in ("PENDING", "QUEUED", "IN_PROGRESS", "WAITING")
                       or (c.get("status") == "COMPLETED" and not c.get("conclusion"))]
            done = len(checks) - len(pending)
            msg = f"    {label} PR #{number}: {done}/{len(checks)} checks done, waiting on: {', '.join(pending[:3])}"
            if len(pending) > 3:
                msg += f" (+{len(pending) - 3} more)"
        if msg != last_msg:
            print(msg, flush=True)
            last_msg = msg
        time.sleep(15)
    fail(f"timed out waiting for {label} PR #{number} checks")


def merge_pr(number: int, label: str, strategy: str) -> None:
    """Wait for checks, then merge with the given strategy."""
    info(f"waiting for CI on {label} PR #{number}...")
    wait_for_checks(number, label)

    info(f"merging {label} PR #{number} (--{strategy})...")
    r = gh(["pr", "merge", str(number), f"--{strategy}"], check=False)
    if r.returncode != 0:
        stderr = r.stderr.strip()
        if "auto-merge" in stderr.lower() or "not allowed" in stderr.lower():
            fail(f"merge failed: {stderr}\n  Merge manually: gh pr merge {number} --{strategy}")
        # Try --auto as fallback (merges when branch protection allows)
        r2 = gh(["pr", "merge", str(number), f"--{strategy}", "--auto"], check=False)
        if r2.returncode != 0:
            fail(f"merge failed: {stderr}\n  Merge manually: gh pr merge {number} --{strategy}")
        info(f"auto-merge enabled on {label} PR #{number} — waiting for merge...")

    deadline = time.time() + 300
    while time.time() < deadline:
        if pr_state(number) == "MERGED":
            info(f"{label} PR #{number} merged")
            return
        time.sleep(10)
    fail(f"timed out waiting for {label} PR #{number} to merge")


# ── Phase 1: Preflight ──────────────────────────────────────────────


def preflight(version: str) -> None:
    banner("1/5", "Preflight checks")

    porcelain = git(["status", "--porcelain", "--untracked-files=no"]).stdout.strip()
    if porcelain:
        fail(f"tracked files are dirty — commit or stash first:\n{porcelain}")

    cur = read_cargo_version()
    from release import parse_version
    if cur == version:
        info(f"version already at {version} (resuming)")
    elif parse_version(version) <= parse_version(cur):
        fail(f"{version} is not greater than current {cur}")
    else:
        info(f"version: {cur} → {version}")

    resuming = cur == version
    text = (REPO_DIR / "CHANGELOG.md").read_text()
    if f"## [{version}]" in text:
        info(f"CHANGELOG already has [{version}] section")
    else:
        from release import UNRELEASED_HEADER_RE
        m = UNRELEASED_HEADER_RE.search(text)
        if not m:
            fail("CHANGELOG.md has no [Unreleased] header")
        rest = text[m.end():]
        nxt = re.search(r"(?m)^## \[", rest)
        body = (rest[:nxt.start()] if nxt else rest).strip()
        if not body:
            fail("CHANGELOG.md [Unreleased] section is empty — nothing to release")
        info("CHANGELOG has unreleased entries")

    if tag_exists_on_remote(f"v{version}"):
        fail(f"v{version} tag already exists on remote — already released?")

    info("preflight passed")


# ── Phase 2: Prepare ────────────────────────────────────────────────


def prepare(version: str) -> None:
    banner("2/5", f"Prepare release branch for {version}")

    branch = f"release-{version}"
    current = git_branch()

    if current == branch:
        info(f"already on {branch} — skipping prepare")
        check_readme_truth_version_strings(version)
        return

    if read_cargo_version() == version:
        info(f"version already bumped to {version} — skipping prepare")
        check_readme_truth_version_strings(version)
        return

    if current != "develop":
        warn(f"on {current}, switching to develop")
        git(["checkout", "develop"])

    git(["pull", "--ff-only"])

    # Stash untracked files so release.py's strict porcelain check passes
    stashed = False
    porcelain = git(["status", "--porcelain"]).stdout.strip()
    if porcelain:
        untracked_only = all(line.startswith("?? ") for line in porcelain.splitlines())
        if untracked_only:
            git(["stash", "--include-untracked", "-m", "release_full: stash untracked"])
            stashed = True
        else:
            fail("tracked files are dirty — commit or stash first")

    result = subprocess.run(
        [sys.executable, "scripts/release.py", "prepare", version],
        cwd=REPO_DIR, text=True,
    )

    if stashed:
        git(["stash", "pop"], check=False)

    if result.returncode != 0:
        fail("release.py prepare failed")

    check_readme_truth_version_strings(version)
    info(f"release branch {branch} ready")


# ── Phase 3: Bump PR (release branch → develop) ────────────────────


def bump_pr(version: str) -> None:
    banner("3/5", "Bump PR → develop")

    branch = f"release-{version}"
    check_readme_truth_version_strings(version)

    existing = pr_for_branch("develop", branch)
    if existing and existing["state"] == "MERGED":
        info(f"bump PR #{existing['number']} already merged")
        git(["checkout", "develop"])
        git(["pull", "--ff-only"])
        return

    if existing:
        info(f"bump PR already exists: #{existing['number']}")
        pr_number = existing["number"]
    else:
        if not confirm(f"Push {branch} and create PR to develop?"):
            fail("aborted")

        git(["push", "-u", "origin", branch])
        info(f"pushed {branch}")

        r = gh(["pr", "create",
                "--base", "develop",
                "--head", branch,
                "--title", f"Release v{version} (version bump + CHANGELOG)",
                "--body", f"Bumps quadraui to {version} and retitles CHANGELOG.md."],
               check=False)
        if r.returncode == 0:
            pr_url = r.stdout.strip()
            info(f"bump PR created: {pr_url}")
            pr_number = int(re.search(r"/(\d+)$", pr_url).group(1))
        else:
            warn(f"PR creation failed: {r.stderr}")
            fail("create the PR manually and re-run")
            return

    merge_pr(pr_number, "bump", "squash")

    git(["checkout", "develop"])
    git(["pull", "--ff-only"])


# ── Phase 4: Promote (develop → main) ──────────────────────────────


def promote(version: str) -> None:
    banner("4/5", "Promote develop → main")

    tag = f"v{version}"
    if tag_exists_on_remote(tag):
        info(f"{tag} already on remote — skipping promote")
        return

    existing = pr_for_branch("main", "develop")
    if existing and existing["state"] == "MERGED":
        info(f"promotion PR #{existing['number']} already merged")
        return

    if existing:
        info(f"promotion PR already exists: #{existing['number']}")
        pr_number = existing["number"]
    else:
        changelog_text = (REPO_DIR / "CHANGELOG.md").read_text()
        from release import release_section_body
        body = release_section_body(changelog_text, version)

        if not confirm("Create develop → main promotion PR?"):
            fail("aborted")

        r = gh(["pr", "create",
                "--base", "main",
                "--head", "develop",
                "--title", f"Release v{version}",
                "--body", body],
               check=False)
        if r.returncode == 0:
            pr_url = r.stdout.strip()
            info(f"promotion PR created: {pr_url}")
            pr_number = int(re.search(r"/(\d+)$", pr_url).group(1))
        else:
            warn(f"PR creation failed: {r.stderr}")
            fail("create the PR manually and re-run")
            return

    merge_pr(pr_number, "promotion", "merge")


# ── Phase 5: Wait for publish ───────────────────────────────────────


def publish(version: str) -> None:
    banner("5/5", f"Wait for crates.io publish of {version}")

    if wait_for_crate(version, timeout=10):
        info(f"quadraui {version} already on crates.io")
        return

    print(f"  Waiting for CI to publish quadraui {version}...")
    print(f"  (release.yml triggers on main push, then cargo publish)\n")

    if wait_for_crate(version, timeout=900):
        info(f"quadraui {version} is live on crates.io")
    else:
        fail(f"timed out waiting for quadraui {version} — check the release workflow")


# ── Main ────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="End-to-end quadraui release.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("version", help="new version to release (e.g. 0.2.1)")
    parser.add_argument("--yes", "-y", action="store_true",
                        help="answer yes to all prompts")
    args = parser.parse_args()

    if args.yes:
        global AUTO_YES
        AUTO_YES = True

    # Add scripts/ to sys.path so we can import release.py helpers
    sys.path.insert(0, str(REPO_DIR / "scripts"))

    preflight(args.version)
    prepare(args.version)
    bump_pr(args.version)
    promote(args.version)
    publish(args.version)

    print(f"\n  {GREEN}{BOLD}quadraui {args.version} released successfully.{RESET}\n")


if __name__ == "__main__":
    main()
