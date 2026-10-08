//! `quadraui-repo-lint` — maintainer-workflow guards for the quadraui repo,
//! run explicitly by CI as a binary rather than picked up by `cargo test`
//! (#1110's "move process meta-tests out of `cargo test`" half only —
//! #1110's separate "stop depending on host fonts" half, the macOS tests
//! that `expect("Menlo installed on every macOS host")`, is untouched by
//! this crate; see `quadraui/docs/TESTING.md`'s "What unit tests don't
//! cover" section for that gap).
//!
//! ## Why this crate exists
//!
//! Before #1110, four files lived in `quadraui/tests/`:
//! `quality_gate_docs.rs`, `downstream_gate_docs.rs`, `githooks_worktree.rs`,
//! and `example_manifest.rs`. Every assertion in them is about *this repo's
//! own* maintainer workflow — does CLAUDE.md's Quality Gate block match
//! ci.yml, does the `downstream` job still name the consumers CLAUDE.md
//! documents, does `.githooks/` actually symlink `graphify-out/` in a fresh
//! worktree, does every `examples/*.rs` have a `[[example]]` manifest entry —
//! not about whether `quadraui` the library behaves correctly.
//!
//! Because `quadraui/tests/*.rs` is auto-discovered by `cargo test`, all four
//! ran on *every* `cargo test` in that crate — including an outside
//! contributor's very first `cargo test --features tui` on a fork, someone
//! who has no reason to know or care what CLAUDE.md's Quality Gate section
//! says. A wording mismatch that means nothing about their diff failed their
//! build anyway. Moving these checks to a plain binary — no `#[test]`
//! functions calling into repo policy, so `cargo test` (at any scope) no
//! longer executes them at all — fixes that, while keeping the checks
//! themselves: CI's `repo-lint` job runs this binary directly, same as it
//! already runs `tools/example_coverage.py` for a related reason.
//!
//! ## Running it
//!
//! ```bash
//! cargo run -p quadraui-repo-lint
//! ```
//!
//! from anywhere inside the checkout (see `common::repo_root` for how it
//! finds the repo root regardless of the invoking CWD). Exits `0` if every
//! check passes, `1` and a `failures:` summary otherwise — the same contract
//! `cargo test`'s own harness uses, deliberately, since these checks used to
//! run under it.
//!
//! ## What's still a `#[test]` in this crate
//!
//! `example_manifest`'s TOML-stanza parser has its own pure-logic sanity
//! check (`parser_detects_missing_and_ungated_entries`) that touches no
//! filesystem and has nothing to do with quadraui's docs or CI wording —
//! that one stays a normal `#[cfg(test)] #[test]` fn, run by
//! `cargo test -p quadraui-repo-lint`. It's a unit test of this tool, not a
//! maintainer-workflow guard, so it doesn't need to move anywhere.

mod common;
mod cross_target_toolchain;
mod downstream_gate_docs;
mod example_manifest;
#[cfg(unix)]
mod githooks_worktree;
mod primitive_doc_examples;
mod quality_gate_docs;

use std::panic;

/// One named, self-contained check. `run` panics (via `assert!`/`panic!`,
/// same idiom as a `#[test]` fn body) to report failure — this harness
/// catches that panic so one failing check doesn't stop the rest from
/// running and being reported.
struct Check {
    name: &'static str,
    run: fn(),
}

macro_rules! check {
    ($f:expr) => {
        Check {
            name: stringify!($f),
            run: $f,
        }
    };
}

fn checks() -> Vec<Check> {
    #[allow(unused_mut)]
    let mut checks = vec![
        check!(quality_gate_docs::claude_md_quality_gate_commands_are_all_run_by_ci),
        check!(quality_gate_docs::claude_md_quality_gate_never_recommends_a_bare_workspace_test),
        check!(quality_gate_docs::testing_md_win_gui_test_command_carries_every_required_env_var),
        check!(quality_gate_docs::win_test_script_sets_every_required_env_var),
        check!(quality_gate_docs::win_test_script_and_testing_md_run_the_same_cargo_command),
        check!(quality_gate_docs::win_test_script_is_executable),
        check!(downstream_gate_docs::downstream_job_and_claude_md_name_the_same_consumers),
        check!(downstream_gate_docs::downstream_job_keeps_its_three_load_bearing_properties),
        check!(example_manifest::every_example_file_has_a_manifest_entry),
        check!(example_manifest::every_example_entry_requires_its_backend_feature),
        check!(cross_target_toolchain::rust_toolchain_pin_still_exists),
        check!(cross_target_toolchain::no_workflow_installs_cross_targets_via_the_action_input),
        check!(cross_target_toolchain::every_cross_target_build_installs_its_target_with_rustup),
        check!(primitive_doc_examples::every_primitive_struct_has_an_examples_section),
        check!(primitive_doc_examples::allowlist_has_no_stale_entries),
    ];

    #[cfg(unix)]
    checks.extend([
        check!(githooks_worktree::hooks_are_committed_executable),
        check!(githooks_worktree::worktree_add_links_to_base_graph),
        check!(githooks_worktree::worktree_add_git_status_is_empty),
        check!(githooks_worktree::worktree_link_preserves_the_tracked_gitignore),
        check!(githooks_worktree::worktree_remove_leaves_base_graph_intact),
        check!(githooks_worktree::worktree_add_without_base_graph_makes_no_symlink),
        check!(githooks_worktree::real_worktree_graph_is_never_clobbered),
    ]);

    checks
}

/// Best-effort extraction of a human-readable message from a caught panic
/// payload — `panic!`/`assert!` payloads are almost always `&str` or
/// `String`, but fall back to a fixed message rather than panicking again on
/// an unexpected payload type.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(panic payload was not a string)".to_string()
    }
}

fn main() {
    // Suppress the default panic handler's own stderr spew for each caught
    // panic — this harness prints its own "FAILED" line and collects the
    // message for the summary at the end, same as `cargo test`'s output
    // shape.
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let all_checks = checks();
    let total = all_checks.len();
    let mut failures: Vec<(&str, String)> = Vec::new();

    for check in all_checks {
        print!("check {} ... ", check.name);
        match panic::catch_unwind(check.run) {
            Ok(()) => println!("ok"),
            Err(payload) => {
                println!("FAILED");
                failures.push((check.name, panic_message(payload)));
            }
        }
    }

    panic::set_hook(default_hook);

    if failures.is_empty() {
        println!("\nall {total} repo-lint checks passed");
        return;
    }

    eprintln!("\nfailures:\n");
    for (name, msg) in &failures {
        eprintln!("---- {name} ----\n{msg}\n");
    }
    eprintln!(
        "repo-lint result: {} FAILED, {} passed, {total} total",
        failures.len(),
        total - failures.len()
    );
    std::process::exit(1);
}
