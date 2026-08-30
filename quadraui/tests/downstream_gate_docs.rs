//! `.github/workflows/ci.yml`'s `downstream` job — the consumer
//! compile-truth gate (#528) — must keep saying the same thing CLAUDE.md's
//! *Downstream consumers* section says, and must keep the three properties
//! CLAUDE.md calls load-bearing.
//!
//! Why this is a *test* and not a review checklist, exactly as with
//! `quality_gate_docs.rs`: that job's failure mode is silence. Every one of
//! its checks is `continue-on-error: true` by design (a consumer may be red
//! for its own reasons), so the job's own conclusion is computed by a final
//! `Evaluate` step from step outcomes. A step that never runs — because it
//! `cd`s into a directory that no longer exists, or resolves the dependency
//! to the wrong source — contributes no failure and the job goes green while
//! proving nothing. That is not hypothetical: on 2026-08-29
//! `claude-coordinator#2899` moved the `coord-tui` crate out to its own
//! repo, and this job kept pointing at `claude-coordinator/tui` — a
//! directory that still exists in that repo but now holds only
//! `tests/acceptance/`.
//!
//! **What this test cannot do:** notice that a consumer moved. Nothing in
//! this repo knows that. What it *can* do is stop ci.yml and CLAUDE.md from
//! disagreeing about who the consumers are and how they are checked, so
//! that when a human or agent updates one, the other cannot silently rot —
//! and so the three "do not tidy this away" properties are enforced by
//! something that runs, not by a comment asking nicely.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// Repo root — `quadraui/`'s parent, where `CLAUDE.md` and `.github/` live.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("quadraui crate dir always has a parent (the repo root)")
        .to_path_buf()
}

fn ci_yml() -> String {
    fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect(".github/workflows/ci.yml exists at repo root")
}

fn claude_md() -> String {
    fs::read_to_string(repo_root().join("CLAUDE.md")).expect("CLAUDE.md exists at repo root")
}

/// The `downstream:` job's own lines, from its key to the next job key at
/// the same (4-space) indent, or EOF. Line-based rather than YAML-parsed on
/// purpose: this crate has no YAML dev-dependency, and the properties below
/// are all textual anyway.
fn downstream_job(ci: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut inside = false;
    for line in ci.lines() {
        let is_job_key = line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
            && !line.trim_start().starts_with('#');
        if is_job_key {
            inside = line.trim() == "downstream:";
            if inside {
                continue;
            }
        }
        if inside {
            lines.push(line);
        }
    }
    assert!(
        !lines.is_empty(),
        "no `downstream:` job found in ci.yml — it was renamed or removed. \
         That job is the only thing standing between a breaking `pub` change \
         and two consumers' CI (CLAUDE.md, *Downstream consumers*); fix this \
         parser or restore the job rather than deleting this test."
    );
    lines
}

/// `JDonaghy/<repo>` names mentioned in `text`, ignoring `quadraui` itself.
fn consumer_repos(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (idx, _) in text.match_indices("JDonaghy/") {
        let rest = &text[idx + "JDonaghy/".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .collect();
        if !name.is_empty() && name != "quadraui" {
            found.insert(name);
        }
    }
    found
}

#[test]
fn downstream_job_and_claude_md_name_the_same_consumers() {
    let ci = ci_yml();
    let job: String = downstream_job(&ci).join("\n");

    // CLAUDE.md's *Downstream consumers* section, up to the next `## `.
    let md = claude_md();
    let start = md
        .find("## Downstream consumers")
        .expect("CLAUDE.md still has a `## Downstream consumers` section");
    let section = &md[start..];
    let end = section[3..]
        .find("\n## ")
        .map(|i| i + 3)
        .unwrap_or(section.len());
    let section = &section[..end];

    let in_ci = consumer_repos(&job);
    let in_doc = consumer_repos(section);

    assert!(
        !in_ci.is_empty(),
        "the `downstream` job clones no JDonaghy consumer repo at all — the \
         gate is inert. Parsed job:\n{job}"
    );

    assert_eq!(
        in_ci, in_doc,
        "ci.yml's `downstream` job and CLAUDE.md's *Downstream consumers* \
         section disagree about which consumer repos exist.\n  \
         cloned by ci.yml: {in_ci:?}\n  documented in CLAUDE.md: {in_doc:?}\n\n\
         A consumer that ci.yml checks but the doc omits is one nobody \
         remembers to grep before a breaking change; a consumer the doc \
         names but ci.yml never clones is a gate with a hole in it. Update \
         whichever is stale — and if a consumer repo moved, fix the clone \
         URL rather than dropping the leg."
    );
}

#[test]
fn downstream_job_keeps_its_three_load_bearing_properties() {
    let ci = ci_yml();
    let lines = downstream_job(&ci);
    let job: String = lines.join("\n");

    // (3) `RUSTFLAGS: ""` overrides the workflow-level `-D warnings`, so a
    // rule-8 `#[deprecated]` shim stays green downstream.
    assert!(
        job.contains(r#"RUSTFLAGS: """#),
        "the `downstream` job no longer sets `RUSTFLAGS: \"\"`. It would then \
         inherit the workflow-level `-D warnings`, turning a rule-8 \
         `#[deprecated]` shim — the correct way to break an API — into a red \
         consumer build, while a reckless hard break (no warning to deny) \
         stays green. See CLAUDE.md, *Downstream consumers*."
    );

    // (1) Every cargo step `cd`s into the consumer; none uses
    // `--manifest-path` from the workspace root, which would make cargo miss
    // the `.cargo/config.toml` `paths` override it discovers by walking up
    // from the process CWD.
    for line in &lines {
        let code = line.split('#').next().unwrap_or(line);
        assert!(
            !code.contains("--manifest-path"),
            "the `downstream` job invokes cargo with `--manifest-path`:\n  \
             {line}\nCargo discovers `.cargo/config.toml` by walking up from \
             the process CWD, not from the manifest's directory, so this form \
             silently discards the coord-tui `paths` override and checks the \
             pinned git rev instead of this PR. Use `working-directory:` (or \
             `cd`) into the consumer."
        );
    }

    // (2) `--all-targets`, since `tui::testing::{TuiDriver, driver_with_shell}`
    // is referenced only from coord-tui's test targets.
    let cargo_checks: Vec<&&str> = lines
        .iter()
        .filter(|l| l.contains("run: cargo check"))
        .collect();
    assert!(
        cargo_checks.len() >= 2,
        "expected at least the two PR-side `cargo check` steps in the \
         `downstream` job, found {}: {cargo_checks:?}",
        cargo_checks.len()
    );
    for line in &cargo_checks {
        assert!(
            line.contains("--all-targets"),
            "`downstream` job step runs a bare cargo check:\n  {line}\nA bare \
             check compiles only lib/bin targets, so it never sees \
             coord-tui's 242 `TuiDriver`/`driver_with_shell` call sites — all \
             of them in test targets. Keep `--all-targets`."
        );
    }

    // …and each of those checks really does run with its CWD inside the
    // consumer: a `working-directory:` between the step's `- name:` and its
    // `run:`.
    for (idx, line) in lines.iter().enumerate() {
        if !line.contains("run: cargo check") {
            continue;
        }
        let step_start = lines[..idx]
            .iter()
            .rposition(|l| l.trim_start().starts_with("- name:"))
            .unwrap_or(0);
        let has_wd = lines[step_start..idx]
            .iter()
            .any(|l| l.trim_start().starts_with("working-directory:"));
        assert!(
            has_wd,
            "`downstream` job step running `{}` has no `working-directory:` — \
             it would run from $GITHUB_WORKSPACE (this repo), where cargo \
             finds neither the consumer's manifest nor its `paths` override.",
            line.trim()
        );
    }
}
