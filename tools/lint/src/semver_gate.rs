//! `.github/workflows/ci.yml`'s `semver-checks` job — the public-API gate
//! against the published crates.io release (#1368) — classifies a public-API
//! diff two ways: advisory everywhere, required on the `develop` -> `main`
//! release promotion PR. These checks guard *how* that split is implemented,
//! because the obvious implementation is silently wrong in one direction and
//! the mistake is invisible in a green workflow run.
//!
//! A job-level `continue-on-error: ${{ <not the promotion PR> }}` reads
//! exactly like the policy it is meant to express, and it does keep the
//! overall *workflow run* green. But the job's own **check run** is still
//! reported to the Checks API with conclusion `failure`. Branch protection,
//! required-checks lists and the coordinator's merge gate all read check
//! runs, not the run conclusion — so under that shape every `develop` PR
//! that happens to carry an accumulated API break (the routine case between
//! releases, which is what `CHANGELOG.md`'s `[Unreleased]` section and
//! CLAUDE.md rule 3 exist to manage) is reported as a red, merge-blocking
//! check. That is the opposite of "advisory", and nothing in the workflow
//! log says so.
//!
//! The shape that works is per *step*: `continue-on-error: true` on the
//! action step, so its failure never reaches the job's conclusion, plus a
//! classification step that reads `steps.<id>.outcome` and decides whether
//! to emit a `::warning::` (advisory) or an `::error::` and `exit 1`
//! (required). These checks assert that shape is still in place, and that
//! the required half still actually hard-fails on a `pull_request` whose
//! `base_ref` is `main` — an advisory-only gate would let a breaking change
//! reach `release.yml`, and publishing to crates.io is irreversible.

use std::fs;

use crate::common::repo_root;

fn ci_yml() -> String {
    fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect(".github/workflows/ci.yml exists at repo root")
}

/// The `semver-checks:` job's own lines, from its key to the next job key at
/// the same (2-space) indent, or EOF. Line-based rather than YAML-parsed for
/// the same reason as `downstream_gate_docs`: this crate has no YAML
/// dependency, and these properties are textual.
fn semver_job(ci: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut inside = false;
    for line in ci.lines() {
        let is_job_key = line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
            && !line.trim_start().starts_with('#');
        if is_job_key {
            inside = line.trim() == "semver-checks:";
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
        "no `semver-checks:` job found in ci.yml — it was renamed or removed. \
         That job is the only thing that compares quadraui's public API \
         against the version already on crates.io before a release promotion \
         merges (publishing is irreversible; yank only). Fix this parser or \
         restore the job rather than deleting this check."
    );
    lines
}

/// Strips a line's trailing `# comment`, so a property named only in prose
/// doesn't read as if it were configured.
fn code_of(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

pub fn semver_job_advisory_split_is_step_level() {
    let ci = ci_yml();
    let lines = semver_job(&ci);

    // A job-level property sits at exactly 4 spaces: job keys are at 2,
    // step list items at 6, step properties at 8.
    for line in &lines {
        let code = code_of(line);
        let job_level = code.starts_with("    ")
            && !code.starts_with("     ")
            && code.trim_start().starts_with("continue-on-error:");
        assert!(
            !job_level,
            "the `semver-checks` job sets a job-level `continue-on-error`:\n  \
             {line}\nThat keeps the workflow run green but still reports this \
             job's check run as `failure`, so every `develop` PR carrying an \
             accumulated API break looks merge-blocking to branch protection \
             and to the coordinator's merge gate. Put `continue-on-error: \
             true` on the cargo-semver-checks *step* and let the \
             classification step after it decide warning vs. hard failure."
        );
    }

    let action_idx = lines
        .iter()
        .position(|l| code_of(l).contains("cargo-semver-checks-action"))
        .expect(
            "the `semver-checks` job no longer runs \
             `obi1kenobi/cargo-semver-checks-action` — the gate is inert",
        );

    // The action step's own properties: from its `- uses:` line to the next
    // list item at the same indent.
    let step_indent = lines[action_idx].len() - lines[action_idx].trim_start().len();
    let mut step: Vec<&str> = vec![lines[action_idx]];
    for line in &lines[action_idx + 1..] {
        let indent = line.len() - line.trim_start().len();
        if indent == step_indent && line.trim_start().starts_with("- ") {
            break;
        }
        step.push(line);
    }
    let step_text = step.join("\n");
    let step_code: String = step
        .iter()
        .map(|l| code_of(l))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        step_code.contains("continue-on-error: true"),
        "the cargo-semver-checks step has no `continue-on-error: true`, so an \
         advisory API diff fails the whole job — and therefore its check run \
         — on ordinary `develop` PRs. Parsed step:\n{step_text}"
    );
    assert!(
        step.iter().any(|l| code_of(l).contains("id:")),
        "the cargo-semver-checks step has no `id:`, so the classification \
         step cannot read its `outcome` and the advisory/required split has \
         nothing to switch on. Parsed step:\n{step_text}"
    );
}

pub fn semver_job_still_blocks_the_release_promotion() {
    let ci = ci_yml();
    let lines = semver_job(&ci);
    let job: String = lines.join("\n");
    let code: String = lines
        .iter()
        .map(|l| code_of(l))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        code.contains("github.base_ref == 'main'"),
        "the `semver-checks` job no longer mentions `github.base_ref == \
         'main'`, so nothing distinguishes the `develop` -> `main` release \
         promotion PR from an ordinary one and the gate is advisory \
         everywhere. A breaking change would then reach `release.yml` and be \
         published to crates.io, which cannot be undone (yank only). Parsed \
         job:\n{job}"
    );
    assert!(
        code.contains("steps.semver.outcome"),
        "no step reads `steps.semver.outcome`, so the cargo-semver-checks \
         step's `continue-on-error: true` swallows its failure outright and \
         the job is green even on the release promotion PR. Parsed \
         job:\n{job}"
    );
    assert!(
        code.contains("exit 1"),
        "the `semver-checks` job's classification step never `exit 1`s, so \
         the `required on main` half of the policy cannot fail the job. \
         Parsed job:\n{job}"
    );
    assert!(
        code.contains("::error"),
        "the `semver-checks` job emits no `::error` annotation, so the \
         release-blocking case is reported only as an exit code with no \
         annotation naming the offending API items. Parsed job:\n{job}"
    );
    assert!(
        code.contains("::warning"),
        "the `semver-checks` job emits no `::warning` annotation, so the \
         advisory case is invisible: a green check run and nothing on the PR \
         saying an API break is accumulating for the next release. Parsed \
         job:\n{job}"
    );
}
