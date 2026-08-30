//! CLAUDE.md's "Quality Gate" block must stay in sync with the commands
//! `.github/workflows/ci.yml` actually runs (#19 follow-up).
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
