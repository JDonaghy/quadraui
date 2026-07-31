//! Integration tests for `.githooks/` — the versioned git-hooks bootstrap
//! that makes the graphify knowledge graph usable from a *linked worktree*
//! (#512, ported from claude-coordinator PRs #1613 / #1614).
//!
//! These drive the *real* hooks through *real* git: build a throwaway repo,
//! copy in the actual `.githooks/` from this checkout, wire up
//! `core.hooksPath`, and exercise `git worktree add` / `git checkout`.
//!
//! Every "X does not happen" assertion here is paired, in the *same*
//! worktree, with proof that the hook mechanism actually ran — two of the
//! equivalent tests in claude-coordinator passed for weeks with the hook
//! never firing at all, because a relative `core.hooksPath` resolves against
//! the *invoking* directory: if `.githooks/` isn't committed (and therefore
//! checked out into the new worktree), in-worktree checkouts never run the
//! hook, and "nothing happened" looks identical to "nothing happened because
//! the condition was false". See #512.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// Root of the actual quadraui checkout this test binary was built from —
/// one level up from the `quadraui` crate's manifest dir.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("quadraui crate has a parent directory")
        .to_path_buf()
}

fn real_githooks_dir() -> PathBuf {
    repo_root().join(".githooks")
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {args:?} in {}: {e}", dir.display()))
}

fn run_ok(dir: &Path, args: &[&str]) -> Output {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {} failed:\nstdout: {}\nstderr: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

fn combined_output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Copies the actual `.githooks/` directory into `dest`, preserving the
/// executable bit on the hooks themselves. That bit is exactly what's under
/// test in `hooks_are_committed_executable` below — if the copy silently
/// dropped it, every *other* test here would also fail (git ignores
/// non-executable hooks), which is the point: they all depend on it.
fn copy_real_githooks_into(dest: &Path) {
    let src = real_githooks_dir();
    assert!(
        src.is_dir(),
        "expected {:?} to exist — run from a checkout with .githooks/ present",
        src
    );
    let target = dest.join(".githooks");
    fs::create_dir_all(&target).unwrap();
    for entry in fs::read_dir(&src).unwrap() {
        let entry = entry.unwrap();
        let contents = fs::read(entry.path()).unwrap();
        let dest_path = target.join(entry.file_name());
        fs::write(&dest_path, &contents).unwrap();
        let mode = fs::metadata(entry.path()).unwrap().permissions();
        fs::set_permissions(&dest_path, mode).unwrap();
    }
}

/// Sets up a bare-bones "base checkout" repo with `.githooks/` committed,
/// `core.hooksPath` wired, and `graphify-out/.gitignore` tracked — mirroring
/// this repo's real setup, where only the `.gitignore` under `graphify-out/`
/// is ever committed. If `seed_graph` is `Some`, also writes a (deliberately
/// untracked, as in reality) `graphify-out/graph.json` with those contents.
fn setup_base_repo(seed_graph: Option<&str>) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().join("base");
    fs::create_dir_all(&base).unwrap();

    run_ok(&base, &["init", "-q"]);
    run_ok(&base, &["config", "user.name", "Test"]);
    run_ok(&base, &["config", "user.email", "test@example.com"]);
    run_ok(&base, &["config", "core.hooksPath", ".githooks"]);

    copy_real_githooks_into(&base);

    fs::create_dir_all(base.join("graphify-out")).unwrap();
    fs::write(base.join("graphify-out/.gitignore"), "*\n!.gitignore\n").unwrap();
    fs::write(base.join("README.md"), "base repo\n").unwrap();

    run_ok(&base, &["add", "."]);
    run_ok(&base, &["commit", "-q", "-m", "initial"]);

    if let Some(contents) = seed_graph {
        fs::write(base.join("graphify-out/graph.json"), contents).unwrap();
    }

    tmp
}

/// `Some(target)` if `path` is a symlink, `None` otherwise (including
/// nonexistent paths).
fn symlink_target(path: &Path) -> Option<PathBuf> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.file_type().is_symlink())
        .and_then(|_| fs::read_link(path).ok())
}

/// The checked-in hooks must be mode `100755` — git silently ignores a
/// non-executable hook (an advice hint at most), so a mode regression
/// disables the whole bootstrap with no error anywhere.
#[test]
fn hooks_are_committed_executable() {
    // Reads THIS repo's actual index, not a copy — this is what ships.
    let root = repo_root();
    let out = run_ok(&root, &["ls-files", "-s", ".githooks"]);
    let listing = String::from_utf8_lossy(&out.stdout);

    let mut modes = HashMap::new();
    for line in listing.lines() {
        // Format: "<mode> <sha> <stage>\t<path>"
        let mut parts = line.splitn(2, '\t');
        let meta = parts.next().unwrap_or_default();
        let path = parts.next().unwrap_or_default();
        let mode = meta.split_whitespace().next().unwrap_or_default();
        modes.insert(path.to_string(), mode.to_string());
    }
    assert!(
        !modes.is_empty(),
        ".githooks does not appear to be tracked in the index yet"
    );

    for hook in ["post-checkout", "post-commit", "post-merge"] {
        let path = format!(".githooks/{hook}");
        assert_eq!(
            modes.get(&path).map(String::as_str),
            Some("100755"),
            "{path} must be committed mode 100755 (got {:?}) — a non-executable \
             hook is silently ignored by git",
            modes.get(&path)
        );
    }
}

/// `git worktree add` with a base graph present must symlink the new
/// worktree's `graphify-out/graph.json` to the base checkout's graph.
/// `graphify-out/` itself must stay a real, git-clean directory — never a
/// symlink (#512 / claude-coordinator#1617: an earlier version replaced the
/// whole directory with a symlink, which required deleting the tracked
/// `graphify-out/.gitignore` out from under git first).
#[test]
fn worktree_add_links_to_base_graph() {
    let tmp = setup_base_repo(Some("BASE-GRAPH-V1"));
    let base = tmp.path().join("base");
    let wt = tmp.path().join("wt");

    let out = run_ok(
        &base,
        &["worktree", "add", wt.to_str().unwrap(), "-b", "feature-1"],
    );
    let combined = combined_output(&out);
    assert!(
        combined.contains("[graphify] linked graphify-out"),
        "hook did not report linking graphify-out; it may not have run at all:\n{combined}"
    );

    let go = wt.join("graphify-out");
    assert!(
        go.is_dir() && symlink_target(&go).is_none(),
        "graphify-out itself must stay a real directory, not become a symlink"
    );

    let graph_link = go.join("graph.json");
    assert!(
        symlink_target(&graph_link).is_some(),
        "graphify-out/graph.json in the worktree should be a symlink"
    );
    let resolved = fs::canonicalize(&graph_link).unwrap();
    let expected = fs::canonicalize(base.join("graphify-out/graph.json")).unwrap();
    assert_eq!(
        resolved, expected,
        "worktree graphify-out/graph.json symlink must resolve to the base checkout's graph.json"
    );

    let graph = fs::read_to_string(&graph_link).unwrap();
    assert_eq!(graph, "BASE-GRAPH-V1");
}

/// The acceptance bar for #512 / claude-coordinator#1617: a fresh linked
/// worktree must be `git status --porcelain` clean immediately after
/// `git worktree add` runs the hook. The original bug showed up here as a
/// deleted tracked `graphify-out/.gitignore` plus a new untracked,
/// machine-local, absolute-path symlink — both invisible to any check that
/// only looks at what the symlink points to, which is why this specific
/// assertion is the whole point of the issue.
#[test]
fn worktree_add_git_status_is_empty() {
    let tmp = setup_base_repo(Some("BASE-GRAPH-V1"));
    let base = tmp.path().join("base");
    let wt = tmp.path().join("wt");

    run_ok(
        &base,
        &["worktree", "add", wt.to_str().unwrap(), "-b", "feature-status"],
    );

    let out = run_ok(&wt, &["status", "--porcelain"]);
    let status = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        status, "",
        "expected a clean worktree immediately after `git worktree add`, got:\n{status}"
    );
}

/// `graphify-out/.gitignore` is tracked, so `git worktree add` materialises
/// a non-empty stub directory. The hook must add symlinks alongside it
/// without ever deleting or shadowing the tracked file.
#[test]
fn worktree_link_preserves_the_tracked_gitignore() {
    let tmp = setup_base_repo(Some("BASE-GRAPH-V1"));
    let base = tmp.path().join("base");
    let wt = tmp.path().join("wt");

    run_ok(
        &base,
        &["worktree", "add", wt.to_str().unwrap(), "-b", "feature-gitignore"],
    );

    let gitignore = wt.join("graphify-out/.gitignore");
    assert!(
        gitignore.is_file() && symlink_target(&gitignore).is_none(),
        "graphify-out/.gitignore must remain a real, tracked file, not a symlink"
    );
    let contents = fs::read_to_string(&gitignore).unwrap();
    assert!(contents.contains("!.gitignore"));

    let out = run_ok(&wt, &["ls-files", "graphify-out/.gitignore"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "graphify-out/.gitignore",
        "graphify-out/.gitignore must still be tracked in the worktree"
    );

    assert!(wt.join("graphify-out/graph.json").is_file());
}

/// #512 / claude-coordinator#1295: the per-entry symlinks must never cause
/// worktree cleanup to reach into the base checkout — `git worktree remove`
/// must leave the base graph (including nested files) untouched.
#[test]
fn worktree_remove_leaves_base_graph_intact() {
    let tmp = setup_base_repo(Some("BASE-GRAPH-V1"));
    let base = tmp.path().join("base");
    let wt = tmp.path().join("wt");

    let cache_dir = base.join("graphify-out/cache");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(cache_dir.join("foo"), "x").unwrap();

    run_ok(
        &base,
        &["worktree", "add", wt.to_str().unwrap(), "-b", "feature-remove"],
    );
    assert!(wt.join("graphify-out/cache/foo").is_file());

    run_ok(&base, &["worktree", "remove", "--force", wt.to_str().unwrap()]);

    let base_out = base.join("graphify-out");
    assert!(base_out.join("graph.json").is_file());
    assert!(base_out.join("cache/foo").is_file());
    assert!(symlink_target(&base_out).is_none());
}

/// When the base checkout has no `graph.json`, the worktree must NOT get a
/// (dangling) symlink — it keeps the plain directory `git worktree add`
/// materialised from the tracked `.gitignore`, with no `graph.json` link
/// inside it.
///
/// Anti-vacuity: after asserting "no symlink", this test seeds a base graph
/// and triggers another checkout *in the same worktree*, and requires the
/// symlink to now appear. Without that second half, "no symlink" would pass
/// identically whether the hook correctly declined to link, or never ran at
/// all — which is exactly the failure mode #512 calls out.
#[test]
fn worktree_add_without_base_graph_makes_no_symlink() {
    let tmp = setup_base_repo(None);
    let base = tmp.path().join("base");
    let wt = tmp.path().join("wt");

    run_ok(
        &base,
        &["worktree", "add", wt.to_str().unwrap(), "-b", "feature-2"],
    );

    let go = wt.join("graphify-out");
    assert!(
        go.is_dir(),
        "graphify-out should exist as a plain dir (tracked .gitignore checked out)"
    );
    assert!(
        symlink_target(&go).is_none(),
        "graphify-out must not be a symlink"
    );
    let graph_link = go.join("graph.json");
    assert!(!graph_link.exists());
    assert!(
        symlink_target(&graph_link).is_none(),
        "graphify-out/graph.json must not be a symlink when the base checkout has no graph.json"
    );

    // The worktree must also be git-status clean with no graph present.
    let status_out = run_ok(&wt, &["status", "--porcelain"]);
    assert_eq!(
        String::from_utf8_lossy(&status_out.stdout),
        "",
        "worktree must be git-clean even when no base graph exists to link"
    );

    // --- anti-vacuity half ---
    fs::write(base.join("graphify-out/graph.json"), "BASE-GRAPH-V2").unwrap();
    let out = run_ok(&wt, &["checkout", "-b", "feature-2-bump"]);
    let combined = combined_output(&out);
    assert!(
        combined.contains("[graphify] linked graphify-out"),
        "hook should report linking once a base graph exists; if this doesn't \
         fire, the earlier 'no symlink' result was vacuous (hook never ran):\n{combined}"
    );
    assert!(symlink_target(&graph_link).is_some());
    assert_eq!(
        fs::read_to_string(&graph_link).unwrap(),
        "BASE-GRAPH-V2"
    );
}

/// A real graph already present in the worktree (e.g. from a manual
/// `/graphify` run there) must never be clobbered by the base-graph
/// symlink, even once the base checkout gains/updates its own graph.
#[test]
fn real_worktree_graph_is_never_clobbered() {
    let tmp = setup_base_repo(Some("BASE-GRAPH-V1"));
    let base = tmp.path().join("base");
    let wt = tmp.path().join("wt");

    // First checkout doubles as the reachability proof: base has a graph,
    // so the hook must symlink and announce it.
    let out = run_ok(
        &base,
        &["worktree", "add", wt.to_str().unwrap(), "-b", "feature-3"],
    );
    assert!(combined_output(&out).contains("[graphify] linked graphify-out"));
    let go = wt.join("graphify-out");
    let graph_link = go.join("graph.json");
    assert!(
        symlink_target(&graph_link).is_some(),
        "sanity: hook should have symlinked graph.json before we replace it with a real graph"
    );

    // Simulate a real graph subsequently being built in this worktree,
    // replacing the linked graph.json — exactly what a manual `/graphify`
    // run would do. `remove_file` on a symlink removes only the link, never
    // the shared base file it points at.
    fs::remove_file(&graph_link).unwrap();
    fs::write(&graph_link, "REAL-WORKTREE-GRAPH").unwrap();

    // Base's graph changes too, so a clobber would be visible either as a
    // re-created symlink or as base's content leaking in.
    fs::write(
        base.join("graphify-out/graph.json"),
        "BASE-GRAPH-V2-CHANGED",
    )
    .unwrap();

    run_ok(&wt, &["checkout", "-b", "feature-3-bump"]);

    assert!(
        go.is_dir() && symlink_target(&go).is_none(),
        "graphify-out must remain a real directory throughout"
    );
    assert!(
        symlink_target(&graph_link).is_none(),
        "a real local graph.json must never be replaced by a symlink"
    );
    assert_eq!(
        fs::read_to_string(&graph_link).unwrap(),
        "REAL-WORKTREE-GRAPH",
        "real local graph.json content must be left untouched"
    );
}
