//! CLAUDE.md's copy-paste command blocks must stay true: the "Quality Gate"
//! block in sync with the commands `.github/workflows/ci.yml` actually runs
//! (#19 follow-up), and the "Win-GUI" block carrying every environment
//! variable the cross-compiled Windows run needs (#832 follow-up) — along
//! with `tools/win-test.sh`, the wrapper that exists so those variables do
//! not have to be copied by hand at all, and which must therefore keep
//! setting all of them and keep running the command the doc says it runs.
//!
//! Why this is a *test* and not a review checklist: the gate block is the
//! first thing every agent and every human copies before committing, and it
//! is pure prose — nothing compiles it, so it rots silently and is still
//! trusted while it rots.
//!
//! That already cost real time. CLAUDE.md documented a bare
//! `cargo test --features tui` at the workspace root long after ci.yml had
//! moved to `cargo test --features tui --workspace --exclude kubeui-gtk`.
//! The bare form selects *every* workspace member, so it drags in
//! `kubeui-gtk` → `gtk4` → `glib-sys` → `pkg-config`. On any machine
//! without pkg-config and the GTK4 `-dev` packages that dies in a build
//! script before compiling a single line of quadraui — a hard failure that
//! says nothing at all about the diff under test. Issue #19's smoke test
//! was reported failing twice for exactly that reason while the code was
//! green the whole time.
//!
//! The invariant is deliberately one-directional: every `cargo` line in
//! CLAUDE.md's gate must appear in ci.yml, but *not* the reverse. ci.yml
//! legitimately runs many more steps than a human is asked to run locally
//! (per-example builds, conformance-matrix uploads, the downstream-consumer
//! job). Requiring the reverse containment would turn every new CI step
//! into a forced CLAUDE.md edit, which is not the failure being guarded.

use std::fs;
use std::path::PathBuf;

/// Repo root — `quadraui/`'s parent, where `CLAUDE.md` and `.github/` live.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("quadraui crate dir always has a parent (the repo root)")
        .to_path_buf()
}

/// Collapse runs of whitespace to single spaces so the doc block may align
/// its commands into columns (`cargo build  --features …`) without that
/// cosmetic padding being mistaken for a drift from ci.yml.
fn normalise(cmd: &str) -> String {
    cmd.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every `cargo …` line inside the fenced ```bash block that follows the
/// `## Quality Gate` heading in CLAUDE.md, comments and blanks dropped.
fn documented_gate_commands(claude_md: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut in_section = false;
    let mut in_fence = false;

    for raw in claude_md.lines() {
        let line = raw.trim();

        if line.starts_with("## ") {
            // The gate section ends at the next heading of any kind.
            in_section = line == "## Quality Gate";
            continue;
        }
        if !in_section {
            continue;
        }
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("cargo ") {
            commands.push(normalise(line));
        }
    }

    commands
}

#[test]
fn claude_md_quality_gate_commands_are_all_run_by_ci() {
    let root = repo_root();
    let claude_md =
        fs::read_to_string(root.join("CLAUDE.md")).expect("CLAUDE.md exists at repo root");
    let ci_yml = fs::read_to_string(root.join(".github/workflows/ci.yml"))
        .expect(".github/workflows/ci.yml exists at repo root");

    // Normalise ci.yml the same way, line by line, so a `run:` prefix or
    // YAML indentation doesn't defeat the substring match.
    let ci_lines: Vec<String> = ci_yml.lines().map(normalise).collect();

    let documented = documented_gate_commands(&claude_md);

    // Guard the parser itself: if the heading is ever renamed or the fence
    // reformatted, this test must fail loudly rather than vacuously pass on
    // an empty command list.
    assert!(
        documented.len() >= 6,
        "parsed only {} command(s) out of CLAUDE.md's Quality Gate block — \
         the block or its ```bash fence probably moved; fix this parser \
         rather than deleting the assertion. Parsed: {documented:#?}",
        documented.len(),
    );

    let missing: Vec<&String> = documented
        .iter()
        .filter(|cmd| !ci_lines.iter().any(|line| line.contains(cmd.as_str())))
        .collect();

    assert!(
        missing.is_empty(),
        "CLAUDE.md's Quality Gate documents command(s) that .github/workflows/ci.yml \
         does not run:\n{missing:#?}\n\n\
         Either the gate drifted from CI, or CI changed without updating the doc. \
         Make them match — an out-of-date gate block sends every worker down a \
         path CI never validates (see this file's module docs for the \
         pkg-config failure this guards).",
    );
}

#[test]
fn claude_md_quality_gate_never_recommends_a_bare_workspace_test() {
    let claude_md = fs::read_to_string(repo_root().join("CLAUDE.md")).expect("CLAUDE.md exists");
    let documented = documented_gate_commands(&claude_md);

    // The specific regression: a root-level build/test/clippy with no
    // package selection pulls kubeui-gtk (and thus GTK + pkg-config) into a
    // gate that has no business needing them.
    for cmd in &documented {
        let selects_packages = cmd.contains("--workspace") || cmd.contains("-p ");
        let is_fmt = cmd.starts_with("cargo fmt");
        assert!(
            selects_packages || is_fmt,
            "Quality Gate command `{cmd}` selects no packages, so it runs against \
             every workspace member — including kubeui-gtk, whose unconditional \
             gtk4 dependency needs pkg-config and libgtk-4-dev. Add an explicit \
             `--workspace --exclude <member>` or `-p <member>`, matching ci.yml.",
        );
    }
}

/// The blank-line-separated chunks of the fenced ```bash block(s) under
/// CLAUDE.md's `## Win-GUI: building and testing for real` heading. Each
/// chunk is one copy-pasteable invocation — its `\`-continued env prefix
/// plus the `cargo` line — with comments and blank lines dropped.
fn win_gui_invocations(claude_md: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut in_fence = false;

    for raw in claude_md.lines() {
        let line = raw.trim();

        if !in_fence && line.starts_with("## ") {
            in_section = line == "## Win-GUI: building and testing for real";
            continue;
        }
        if !in_section {
            continue;
        }
        if line.starts_with("```") {
            in_fence = !in_fence;
            if !current.is_empty() {
                chunks.push(current.join(" "));
                current.clear();
            }
            continue;
        }
        if !in_fence {
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            if !current.is_empty() {
                chunks.push(current.join(" "));
                current.clear();
            }
            continue;
        }
        current.push(normalise(line.trim_end_matches('\\').trim()));
    }
    if !current.is_empty() {
        chunks.push(current.join(" "));
    }

    chunks
}

/// Every environment variable the documented `cargo xwin test` line must
/// carry, paired with what silently breaks when it is missing. All three
/// failure modes are silent-or-misleading, which is why they are asserted
/// rather than trusted to review — see CLAUDE.md's numbered trap list.
const REQUIRED_XWIN_TEST_ENV: &[(&str, &str)] = &[
    (
        "RUSTFLAGS=\"-C target-feature=+crt-static\"",
        "the host has no vcruntime140.dll, so every test .exe exits 53 with \
         completely empty output — indistinguishable from a program that ran \
         and printed nothing (trap #1)",
    ),
    (
        "RUSTDOCFLAGS=\"-C target-feature=+crt-static\"",
        "rustdoc compiles each doctest itself and does NOT inherit RUSTFLAGS, \
         so the integration tests all pass and only the `Doc-tests quadraui` \
         leg fails — with exit 53 blamed on the library docs rather than on \
         the missing flag (trap #1, second half)",
    ),
    (
        "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env",
        "cargo xwin injects a `wine` runner and dies with \
         `could not execute process 'wine ...'`, even though binfmt_misc can \
         exec the PE directly and wine is never needed (trap #2)",
    ),
];

#[test]
fn claude_md_win_gui_test_command_carries_every_required_env_var() {
    let claude_md = fs::read_to_string(repo_root().join("CLAUDE.md")).expect("CLAUDE.md exists");
    let invocations = win_gui_invocations(&claude_md);

    // Guard the parser: a renamed heading or reformatted fence must fail
    // loudly here rather than vacuously pass on an empty chunk list.
    assert!(
        !invocations.is_empty(),
        "parsed no commands out of CLAUDE.md's `## Win-GUI: building and \
         testing for real` block — the heading or its ```bash fence probably \
         moved; fix this parser rather than deleting the assertion."
    );

    let test_cmds: Vec<&String> = invocations
        .iter()
        .filter(|c| c.contains("cargo xwin test"))
        .collect();
    assert_eq!(
        test_cmds.len(),
        1,
        "expected exactly one documented `cargo xwin test` invocation in \
         CLAUDE.md's Win-GUI block, found {}: {test_cmds:#?}",
        test_cmds.len(),
    );
    let cmd = test_cmds[0];

    for (var, consequence) in REQUIRED_XWIN_TEST_ENV {
        assert!(
            cmd.contains(var),
            "CLAUDE.md's documented Windows test command is missing \
             `{var}`:\n  {cmd}\n\nWithout it, {consequence}.\n\nEvery one of \
             these has already cost a session, because none of them produces \
             an error that names its own cause. Restore the variable rather \
             than relaxing this assertion."
        );
    }
}

/// Path of the wrapper that exists precisely so none of
/// [`REQUIRED_XWIN_TEST_ENV`] has to be remembered.
const WIN_TEST_SCRIPT: &str = "tools/win-test.sh";

fn win_test_script() -> String {
    fs::read_to_string(repo_root().join(WIN_TEST_SCRIPT))
        .unwrap_or_else(|e| panic!("{WIN_TEST_SCRIPT} is readable: {e}"))
}

/// The `cargo xwin test …` portion of a single command, from `cargo`
/// onwards, with `"$@"` (the script's argument forwarding) dropped and
/// cosmetic whitespace collapsed. `None` if there is no such command.
fn cargo_xwin_test_tail(command: &str) -> Option<String> {
    let start = command.find("cargo xwin test")?;
    Some(normalise(&command[start..].replace("\"$@\"", "")))
}

/// The command [`WIN_TEST_SCRIPT`] actually executes — the `exec` line, not
/// the several mentions of `cargo xwin test` in its explanatory header, which
/// a naive "first match" search would pick up instead.
fn scripted_win_test_command(script: &str) -> Option<String> {
    script
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .find(|line| line.starts_with("exec ") && line.contains("cargo xwin test"))
        .and_then(cargo_xwin_test_tail)
}

/// The wrapper must set every variable the raw command needs. This is the
/// same list the CLAUDE.md assertion uses, so the doc and the script cannot
/// drift apart: adding a fourth trap to `REQUIRED_XWIN_TEST_ENV` fails both
/// until both are updated.
#[test]
fn win_test_script_sets_every_required_env_var() {
    let script = win_test_script();

    for (var, consequence) in REQUIRED_XWIN_TEST_ENV {
        assert!(
            script.contains(var),
            "{WIN_TEST_SCRIPT} does not set `{var}`.\n\nWithout it, \
             {consequence}.\n\nThe whole point of this wrapper is that a \
             worker (or a smoke run) cannot lose one of these by copying the \
             command out of a doc — #832's smoke run lost one and reported \
             `9 of 11 doctests failed` as if the diff were at fault."
        );
    }

    assert!(
        script.contains("\"$@\""),
        "{WIN_TEST_SCRIPT} must forward its arguments verbatim (`\"$@\"`) so \
         `--doc`, `--test <name>` and `-- --nocapture` still work; otherwise \
         anyone needing to narrow the run drops back to the raw command and \
         straight back into the missing-env-var trap."
    );
}

/// The wrapper and the spelled-out command in CLAUDE.md must run the *same*
/// cargo invocation. If they diverge, the doc stops describing the script and
/// one of the two silently tests something else (a different target triple, a
/// different feature set, a different package).
#[test]
fn win_test_script_and_claude_md_run_the_same_cargo_command() {
    let claude_md = fs::read_to_string(repo_root().join("CLAUDE.md")).expect("CLAUDE.md exists");

    let documented = win_gui_invocations(&claude_md)
        .iter()
        .find_map(|c| cargo_xwin_test_tail(c))
        .expect(
            "CLAUDE.md's Win-GUI block still spells out one `cargo xwin test` \
             invocation — the wrapper documents what it runs, it does not \
             replace the explanation",
        );

    let scripted = scripted_win_test_command(&win_test_script()).unwrap_or_else(|| {
        panic!(
            "{WIN_TEST_SCRIPT} no longer ends in an `exec cargo xwin test …` \
             line — if the wrapper's shape changed, teach this parser the new \
             shape rather than deleting the assertion."
        )
    });

    assert_eq!(
        scripted, documented,
        "\n{WIN_TEST_SCRIPT} runs:\n  {scripted}\nbut CLAUDE.md documents:\n  \
         {documented}\n\nThese must match. Whichever one you meant to change, \
         change the other too — a wrapper that quietly tests a different \
         target or feature set than the doc claims is worse than no wrapper."
    );
}

/// A wrapper nobody can execute is a wrapper nobody uses. `cfg(unix)` because
/// this test target is also compiled and run for `x86_64-pc-windows-msvc`
/// (that is the very run this script drives), where the mode bits are absent
/// and meaningless.
#[test]
#[cfg(unix)]
fn win_test_script_is_executable() {
    use std::os::unix::fs::PermissionsExt;

    let path = repo_root().join(WIN_TEST_SCRIPT);
    let mode = fs::metadata(&path)
        .unwrap_or_else(|e| panic!("{WIN_TEST_SCRIPT} exists: {e}"))
        .permissions()
        .mode();

    assert!(
        mode & 0o111 != 0,
        "{WIN_TEST_SCRIPT} is not executable (mode {mode:o}); CLAUDE.md tells \
         the reader to run `{WIN_TEST_SCRIPT}` directly. Restore the bit with \
         `git update-index --chmod=+x {WIN_TEST_SCRIPT}`."
    );
}
