//! Guard against the documentation-vs-code drift that issue #798 fixed.
//!
//! #487 did a manual "truth pass" over quadraui's docs in July and it had
//! drifted again by September: the root README claimed Windows was "not
//! implemented yet" while CI blocked merges on a real `windows-latest`
//! build; `quadraui/src/lib.rs` (what docs.rs actually renders) still said
//! "nine primitives" and "v0.1.x" against a crate that had 40 primitive
//! modules and version `0.0.1`; `docs/UI_CRATE_DESIGN.md` claimed a11y
//! fields shipped that don't exist; `docs/BACKEND.md` documented a
//! `BackendError` type with zero occurrences anywhere in `src/`.
//!
//! A manual correction rots the instant the crate changes shape again. This
//! test derives the ground truth (version, primitive count, per-backend
//! reality) from the crate itself — `Cargo.toml`/`env!("CARGO_PKG_VERSION")`,
//! `src/primitives/mod.rs`'s module list, `src/lib.rs`'s own `cfg` gates,
//! `src/backend.rs` — and fails if the docs stop agreeing with it. Whoever
//! adds primitive #41, bumps the version, or finally ships Windows
//! rasterisers is the one who has to touch the docs, in the same PR,
//! instead of leaving it for the next audit.
//!
//! What this cannot do: notice a NEW kind of drift nobody anticipated. It
//! only re-checks the specific facts #798 corrected. Extend it when the
//! next drift is found rather than trusting a truth pass to hold forever.

use std::fs;
use std::path::PathBuf;

/// Repo root — `quadraui/`'s parent, where the root `README.md` lives.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("quadraui crate dir always has a parent (the repo root)")
        .to_path_buf()
}

/// `quadraui/`'s own crate dir (this test's `CARGO_MANIFEST_DIR`).
fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: PathBuf) -> String {
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

fn root_readme() -> String {
    read(repo_root().join("README.md"))
}

fn lib_rs() -> String {
    read(crate_root().join("src/lib.rs"))
}

fn primitives_mod_rs() -> String {
    read(crate_root().join("src/primitives/mod.rs"))
}

fn backend_rs() -> String {
    read(crate_root().join("src/backend.rs"))
}

fn backend_md() -> String {
    read(crate_root().join("docs/BACKEND.md"))
}

fn ui_crate_design_md() -> String {
    read(crate_root().join("docs/UI_CRATE_DESIGN.md"))
}

/// Number of primitive modules declared in `src/primitives/mod.rs`. This is
/// the crate's own definition of "how many primitives exist" — the same
/// thing a `pub mod` grep would show a human, just automated so the docs
/// can be checked against it mechanically.
fn primitive_module_count() -> usize {
    let src = primitives_mod_rs();
    let count = src
        .lines()
        .filter(|l| l.trim_start().starts_with("pub mod "))
        .count();
    assert!(
        count > 0,
        "src/primitives/mod.rs has no `pub mod` declarations — either the \
         module emptied out or this test's parsing broke. Either way, \
         investigate before trusting the count below."
    );
    count
}

// ── Version ──────────────────────────────────────────────────────────────

/// The version string every doc's "pre-1.0" status claim must agree with,
/// e.g. `0.0.1` -> `0.0.x`. Derived from the crate's own `Cargo.toml` via
/// `CARGO_PKG_VERSION` (set by cargo at compile time) rather than
/// re-parsing the TOML — this test *is* part of the `quadraui` package, so
/// it already sees exactly what a consumer's `Cargo.lock` would resolve.
fn major_minor_x() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut parts = version.split('.');
    let major = parts.next().expect("CARGO_PKG_VERSION has a major segment");
    let minor = parts.next().expect("CARGO_PKG_VERSION has a minor segment");
    format!("{major}.{minor}.x")
}

#[test]
fn readme_and_lib_doc_state_the_real_version() {
    let want = major_minor_x();
    let readme = root_readme();
    let lib = lib_rs();

    assert!(
        readme.contains(&format!("`{want}`")),
        "root README.md's Status section doesn't say `{want}` (derived from \
         CARGO_PKG_VERSION = {}). Either the crate version moved and the \
         README wasn't updated, or the README's version claim was edited \
         wrong. Update README.md's Status section to match Cargo.toml.",
        env!("CARGO_PKG_VERSION")
    );
    assert!(
        lib.contains(&format!("`{want}`")),
        "quadraui/src/lib.rs's crate doc (what docs.rs renders) doesn't say \
         `{want}` (derived from CARGO_PKG_VERSION = {}). This is the exact \
         drift #798 fixed (`v0.1.x` claimed against a 0.0.1 crate) — update \
         the `## Status` section in src/lib.rs's doc comment.",
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn readme_does_not_claim_a_resolvable_published_version() {
    // quadraui is not published to crates.io (see CLAUDE.md's *Downstream
    // consumers*): both real consumers pin a git rev or a relative path.
    // A `quadraui = { version = "..." }` dependency snippet in the README
    // would tell a reader to write a Cargo.toml line that cannot resolve.
    let readme = root_readme();
    assert!(
        !readme.contains("quadraui = { version ="),
        "root README.md shows a `quadraui = {{ version = \"...\" }}` \
         dependency snippet. quadraui is not published to crates.io, so \
         this cannot resolve for any consumer. Show a `git` (pinned rev) \
         or `path` dependency instead — see CLAUDE.md's *Downstream \
         consumers* table for the two real shapes."
    );
}

// ── Primitive count ──────────────────────────────────────────────────────

#[test]
fn lib_doc_states_the_real_primitive_count() {
    let count = primitive_module_count();
    let lib = lib_rs();
    let needle = format!("{count} primitive");
    assert!(
        lib.contains(&needle),
        "quadraui/src/lib.rs's crate doc doesn't contain \"{needle}\", but \
         src/primitives/mod.rs declares {count} `pub mod` primitive \
         modules. This is the exact drift #798 fixed (\"Nine primitives\" \
         claimed against a 40-module crate) — a primitive was added or \
         removed without updating src/lib.rs's doc comment (what docs.rs \
         renders), or the doc's count was hand-edited to a wrong number."
    );
}

#[test]
fn root_readme_states_the_real_primitive_count() {
    let count = primitive_module_count();
    let readme = root_readme();
    let needle = format!("{count} primitive");
    assert!(
        readme.contains(&needle),
        "root README.md doesn't contain \"{needle}\", but \
         src/primitives/mod.rs declares {count} `pub mod` primitive \
         modules. Update the README's Primitives section count."
    );
}

// ── Per-backend status ───────────────────────────────────────────────────

#[test]
fn windows_backend_is_not_described_as_unimplemented() {
    // `src/win/{backend,run}.rs` compile on every host (only the real WinAPI
    // calls inside are `cfg(target_os = "windows")`-gated, with a `todo!()`
    // fallback) — unlike `macos`, `win`'s `pub mod` in lib.rs carries no
    // `target_os` gate at all. That's the structural fact backing "Windows
    // infrastructure is real, not just designed" — verify it still holds
    // before trusting the doc text that depends on it.
    let lib = lib_rs();
    let win_mod_idx = lib
        .find("pub mod win;")
        .expect("quadraui::win module must still exist — see lib.rs");
    let preceding = &lib[..win_mod_idx];
    let win_cfg_start = preceding
        .rfind("#[cfg(feature = \"win\")]")
        .expect("`pub mod win;` must be gated on `feature = \"win\"`");
    let win_cfg_block = &preceding[win_cfg_start..];
    assert!(
        !win_cfg_block.contains("target_os"),
        "`pub mod win;` in lib.rs is now gated on `target_os`, unlike when \
         #798 verified it compiles on every host. If that changed \
         deliberately, `win` is no longer real infrastructure on \
         non-Windows hosts and the README/lib.rs Windows status text \
         (which says its window/event/platform-services layer is real, \
         not just designed) needs re-verifying, not just this test."
    );

    for (doc_name, doc) in [("README.md", root_readme()), ("src/lib.rs", lib)] {
        assert!(
            !doc.to_lowercase().contains("not implemented yet")
                && !doc.to_lowercase().contains("not-yet-implemented"),
            "{doc_name} describes some backend as \"not implemented yet\" / \
             \"not-yet-implemented\". #798 removed that claim for Windows \
             because its window/event/platform-services infrastructure is \
             real and CI-blocking on windows-latest — if this fired for a \
             different backend, verify it the same way (grep CI, grep \
             src/) before restating it; if it fired for Windows again, the \
             phrase crept back in."
        );
    }
}

#[test]
fn macos_backend_is_described_as_shipped_not_planned() {
    // `mod macos` is gated on `all(feature = "macos", target_os = "macos")`
    // — real, target-specific code, not a stub. `macos.yml` builds and
    // tests it for real on `macos-latest`. Docs claiming it's merely
    // "planned" contradict both facts.
    let lib = lib_rs();
    assert!(
        lib.contains("all(feature = \"macos\", target_os = \"macos\")"),
        "expected `mod macos` in lib.rs to stay gated on \
         `all(feature = \"macos\", target_os = \"macos\")` — if that gate \
         changed, re-verify the macOS status text in README.md/lib.rs \
         rather than assuming this test's premise still holds."
    );

    for (doc_name, doc) in [("README.md", root_readme()), ("src/lib.rs", lib)] {
        let lower = doc.to_lowercase();
        assert!(
            !lower.contains("macos") || !lower.contains("planned"),
            "{doc_name} mentions both \"macOS\" and \"planned\" — #798 \
             corrected a claim that the macOS backend was merely \
             \"planned, v1.x\" when it already implements the whole \
             `Backend` trait and is built/tested for real on macos-latest \
             CI. If a *different* macOS feature is legitimately still \
             planned, reword to avoid the bare \"planned\" collision with \
             this check."
        );
    }
}

#[test]
fn shipped_primitives_and_backend_trait_are_not_called_roadmapped() {
    // docs/APP_ARCHITECTURE.md used to say Panel/MenuBar/Backend-trait were
    // "roadmapped but not yet shipped". Verify they exist for real, then
    // verify the doc no longer contradicts that.
    let primitives = primitives_mod_rs();
    assert!(
        primitives.contains("pub mod panel;"),
        "expected `panel` primitive module — if it was renamed/removed, \
         re-verify docs/APP_ARCHITECTURE.md's Panel claim independently."
    );
    assert!(
        primitives.contains("pub mod menu_bar;"),
        "expected `menu_bar` primitive module — if it was renamed/removed, \
         re-verify docs/APP_ARCHITECTURE.md's MenuBar claim independently."
    );
    let backend = backend_rs();
    assert!(
        backend.contains("pub trait Backend"),
        "expected `pub trait Backend` in src/backend.rs — if it moved or \
         was renamed, re-verify docs/APP_ARCHITECTURE.md's Backend-trait \
         claim independently."
    );

    let app_arch = read(crate_root().join("docs/APP_ARCHITECTURE.md"));
    assert!(
        !app_arch.contains("roadmapped but not yet shipped"),
        "docs/APP_ARCHITECTURE.md still says something is \"roadmapped but \
         not yet shipped\" — #798 corrected this for Panel/MenuBar/Backend \
         trait, which all exist in src/ today. If new text reintroduced \
         this exact phrase for something genuinely unshipped, reword to \
         avoid tripping this guard, or extend it to name-check the new \
         claim specifically."
    );
}

// ── a11y and BackendError: doc claims that must track source reality ────

#[test]
fn a11y_fields_absence_is_still_disclosed_accurately() {
    // docs/UI_CRATE_DESIGN.md's decision #6 claimed a11y-ready fields
    // (`a11y_role`, `a11y_label`) ship on every primitive. They never did.
    // If they ever get added for real, this test starts failing — that's
    // the point: whoever ships them must come back and remove the
    // now-stale disclaimer instead of leaving both claims in the repo.
    let has_a11y_fields = fs::read_dir(crate_root().join("src/primitives"))
        .expect("src/primitives exists")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "rs"))
        .any(|e| fs::read_to_string(e.path()).is_ok_and(|s| s.contains("a11y_role")));

    let design_doc = ui_crate_design_md();
    if has_a11y_fields {
        panic!(
            "a11y_role now appears under src/primitives/ — accessibility \
             fields shipped! Remove the \"did not [ship]\" / \"no primitive \
             has any such field\" disclaimers this test currently checks \
             for in docs/UI_CRATE_DESIGN.md (decision #6 and the risk \
             table), and delete this branch of the test."
        );
    }
    assert!(
        design_doc.contains("no primitive has any such field"),
        "docs/UI_CRATE_DESIGN.md's accessibility disclaimer (added by \
         #798) is missing, but src/primitives/ still has no a11y_role \
         field anywhere. Decision #6 in that doc reads as a shipped ✅ \
         without the disclaimer, which is exactly the drift #798 fixed — \
         restore the correction rather than deleting it silently."
    );
}

#[test]
fn backend_error_doc_claim_tracks_source_reality() {
    // docs/BACKEND.md used to document `BackendError`, `Backend::last_error`,
    // and `_result`-suffixed `PlatformServices` methods as design-only,
    // with a "None of them exist in `src/`" disclaimer #798 added to
    // correct drift the other way (the doc claimed shipped API that
    // wasn't there). Issue #805 shipped the minimal error channel D-009
    // designed — `BackendError`, `Backend::last_error`, and
    // `Clipboard::write_text_result` (the three `PlatformServices`
    // dialog `_result` twins remain follow-up scope, not shipped). If
    // `BackendError` ever disappears from src/ again (a revert), this
    // test starts failing — that's the signal to restore #798's
    // "design only" framing instead of leaving a doc that describes API
    // nothing implements.
    let src_dir = crate_root().join("src");
    let backend_error_in_src = walk_rs_files(&src_dir)
        .into_iter()
        .any(|p| fs::read_to_string(&p).is_ok_and(|s| s.contains("BackendError")));

    let backend_doc = backend_md();
    assert!(
        backend_error_in_src,
        "`BackendError` no longer appears anywhere under src/, but \
         docs/BACKEND.md (updated by #805) describes it as shipped API — \
         restore #798's \"design only\" / \"None of them exist in \
         `src/`\" framing instead of leaving stale documentation for API \
         that no longer exists."
    );
    assert!(
        !backend_doc.contains("None of them exist in `src/`"),
        "docs/BACKEND.md still carries #798's \"design only\" disclaimer, \
         but `BackendError` now exists in src/ (issue #805 shipped it) — \
         update the doc to describe the real, shipped API instead of \
         contradicting the code."
    );
}

fn walk_rs_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_rs_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out
}

// ── One root README ──────────────────────────────────────────────────────

#[test]
fn quadraui_crate_has_no_duplicate_readme() {
    // #798 deleted quadraui/README.md in favour of one root README.md —
    // two READMEs is how the two drifted apart from each other (and from
    // reality) in the first place.
    let dup = crate_root().join("README.md");
    assert!(
        !dup.exists(),
        "quadraui/README.md exists again at {} — #798 deleted it in favour \
         of a single root README.md specifically because having two copies \
         is how they drifted apart before. Fold any new content into the \
         root README.md instead of recreating this one.",
        dup.display()
    );
}
