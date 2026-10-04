//! Cross-compiling CI jobs must install their `rust-std` the way the
//! toolchain pin can actually see it (#1131 follow-up).
//!
//! `rust-toolchain.toml` pins this workspace to a fixed channel, and rustup's
//! per-directory override resolution honors that file for *every* `cargo` /
//! `rustup` invocation made from inside the checkout — regardless of what
//! `dtolnay/rust-toolchain@stable` installed as the machine default. That pin
//! is deliberate (see the file's own header comment, and #544).
//!
//! The trap: `dtolnay/rust-toolchain`'s `targets:` input installs each target's
//! `rust-std` **onto the toolchain the action itself installs** — `stable`. The
//! pinned toolchain is a *different* toolchain, auto-installed by rustup with
//! only the `components` the pin file lists, so it gets no cross-target std at
//! all. Every build then resolves to the pin, can't find `core` for the
//! requested target, and dies with
//!
//! ```text
//! error[E0463]: can't find crate for `core`
//! note: the `x86_64-unknown-linux-musl` target may not be installed
//! ```
//!
//! …which looks like a code problem and is not one. It cost #1131's
//! `musl-static` job a red run on its very first CI execution, after the same
//! build had been verified green by hand locally (where the target happened to
//! be installed for the pinned toolchain already).
//!
//! A plain `run: rustup target add <triple>` step has no such split: it runs
//! inside the checkout, so the directory override applies and the std lands on
//! the pinned toolchain, which is where cargo will look for it. ci.yml's
//! `src/macos/` cross-target compile check has always done it that way; the
//! `musl-static` job now does too. These two checks keep it that way.

use std::fs;

use crate::common::repo_root;

/// Workflow files this module inspects, as `(path relative to repo root,
/// contents)`.
fn workflows() -> Vec<(String, String)> {
    let dir = repo_root().join(".github/workflows");
    let mut out = Vec::new();
    let entries =
        fs::read_dir(&dir).unwrap_or_else(|e| panic!("{} is readable: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("workflow dir entry is readable").path();
        let is_yaml = path
            .extension()
            .map(|e| e == "yml" || e == "yaml")
            .unwrap_or(false);
        if !is_yaml {
            continue;
        }
        let body = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()));
        out.push((
            format!(
                ".github/workflows/{}",
                path.file_name().unwrap().to_string_lossy()
            ),
            body,
        ));
    }
    assert!(
        !out.is_empty(),
        "found no workflow files under {} — fix this lint's path rather than \
         letting it pass vacuously",
        dir.display(),
    );
    out
}

/// Is this a YAML comment line (so `--target` / `targets:` inside it is prose,
/// not configuration)? Both checks below reason about *effective* workflow
/// config only; this repo's workflows carry long explanatory comments that
/// quote the very commands being linted.
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// The channel pinned by `rust-toolchain.toml`, if any. Both checks here exist
/// *because* of that pin, so its absence means this module's premise is gone
/// and the module should be revisited rather than silently kept.
fn pinned_channel() -> Option<String> {
    let path = repo_root().join("rust-toolchain.toml");
    let body = fs::read_to_string(&path).ok()?;
    for raw in body.lines() {
        let line = raw.trim();
        if is_comment(line) {
            continue;
        }
        if let Some(rest) = line.strip_prefix("channel") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

pub fn rust_toolchain_pin_still_exists() {
    assert!(
        pinned_channel().is_some(),
        "rust-toolchain.toml no longer pins a `channel`. Both checks in \
         tools/lint/src/cross_target_toolchain.rs exist only because the pin \
         makes `dtolnay/rust-toolchain`'s `targets:` input install cross-target \
         std onto the wrong toolchain. If the pin is intentionally gone, revisit \
         that module (and #544) deliberately instead of leaving assertions \
         standing on a premise that no longer holds.",
    );
}

pub fn no_workflow_installs_cross_targets_via_the_action_input() {
    let mut offenders: Vec<String> = Vec::new();

    for (name, body) in workflows() {
        // Track the most recent `uses:` so a bare `targets:` key is attributed
        // to the action it configures — `targets:` is `dtolnay/rust-toolchain`'s
        // input, but another action could legitimately have a key of the same
        // name.
        let mut last_uses = String::new();
        for (i, raw) in body.lines().enumerate() {
            let line = raw.trim();
            if is_comment(line) {
                continue;
            }
            if let Some(rest) = line.strip_prefix("- uses:") {
                last_uses = rest.trim().to_string();
                continue;
            }
            if line.starts_with("uses:") {
                last_uses = line.trim_start_matches("uses:").trim().to_string();
                continue;
            }
            if line.starts_with("targets:") && last_uses.contains("rust-toolchain") {
                offenders.push(format!("{name}:{} — {line}", i + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "workflow step(s) install a cross-compilation target via \
         `dtolnay/rust-toolchain`'s `targets:` input:\n{offenders:#?}\n\n\
         That installs the target's rust-std onto the toolchain the action \
         installs (`stable`), not onto the `rust-toolchain.toml`-pinned \
         toolchain every cargo invocation in this checkout actually resolves \
         to — so the build fails with `error[E0463]: can't find crate for \
         `core``. Use a `run: rustup target add <triple>` step instead, which \
         runs inside the checkout and therefore honors the pin. See \
         tools/lint/src/cross_target_toolchain.rs's module docs.",
    );
}

/// Every `--target <triple>` a workflow actually passes to cargo, as
/// `(workflow name, line number, triple)`.
fn cross_target_invocations(name: &str, body: &str) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    for (i, raw) in body.lines().enumerate() {
        let line = raw.trim();
        if is_comment(line) {
            continue;
        }
        let mut words = line.split_whitespace().peekable();
        while let Some(word) = words.next() {
            if word == "--target" {
                if let Some(triple) = words.peek() {
                    out.push((name.to_string(), i + 1, (*triple).to_string()));
                }
            } else if let Some(triple) = word.strip_prefix("--target=") {
                out.push((name.to_string(), i + 1, triple.to_string()));
            }
        }
    }
    out
}

pub fn every_cross_target_build_installs_its_target_with_rustup() {
    // Triples a workflow may pass without an accompanying `rustup target add`,
    // declared in that workflow with a `# repo-lint: target-preinstalled
    // <triple>` comment — for a runner whose *host* triple is the target, or a
    // wrapper (cargo-xwin) that installs the std itself. Explicit and
    // reviewable, rather than a hardcoded allowlist in this file.
    let mut checked = 0usize;
    let mut missing: Vec<String> = Vec::new();

    for (name, body) in workflows() {
        let preinstalled: Vec<String> = body
            .lines()
            .filter_map(|l| l.split("repo-lint: target-preinstalled").nth(1))
            .map(|rest| rest.trim().to_string())
            .collect();

        let installs: Vec<String> = body
            .lines()
            .filter(|l| !is_comment(l))
            .filter_map(|l| l.split("rustup target add").nth(1))
            .map(|rest| rest.trim().to_string())
            .collect();

        for (workflow, line, triple) in cross_target_invocations(&name, &body) {
            checked += 1;
            let installed = installs.iter().any(|t| t == &triple);
            let waived = preinstalled.iter().any(|t| t == &triple);
            if !installed && !waived {
                missing.push(format!("{workflow}:{line} — --target {triple}"));
            }
        }
    }

    // Parser guard: these workflows do cross-compile (musl, aarch64-apple-
    // darwin), so finding nothing means the scan broke, not that the repo
    // stopped cross-compiling.
    assert!(
        checked >= 2,
        "found only {checked} `--target` invocation(s) across the workflows — \
         this lint's parser probably broke. Fix it rather than deleting the \
         assertion.",
    );

    assert!(
        missing.is_empty(),
        "workflow step(s) cross-compile to a target the same workflow never \
         installs with `rustup target add`:\n{missing:#?}\n\n\
         Without it the pinned toolchain (rust-toolchain.toml) has no rust-std \
         for that target and the build dies with `error[E0463]: can't find \
         crate for `core``. Add a `run: rustup target add <triple>` step \
         (which honors the pin, unlike the toolchain action's `targets:` \
         input), or — if the target genuinely needs no install, e.g. it is the \
         runner's own host triple — say so in that workflow with a \
         `# repo-lint: target-preinstalled <triple>` comment.",
    );
}
