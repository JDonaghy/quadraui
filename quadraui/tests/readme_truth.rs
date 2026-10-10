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
//!
//! #1107 ("truth pass II") found it had rotted again: the root README's
//! `## Features` section only ever named `tui`/`gtk`, even after `terminal`,
//! `win`, and `macos` features existed in `Cargo.toml`; README.md's Windows
//! status paragraph still said "most `Backend::draw_*`/`*_layout` methods
//! on `WinBackend` are `todo!()` stubs" against a `src/win/backend.rs` with
//! zero non-comment `todo!()` calls left; and the README's sample
//! `Cargo.toml` snippet labelled the `path =` dependency shape "vimcode's
//! approach" after vimcode had already moved to a pinned `git`+`rev`
//! dependency (vimcode#691, quadraui#795 — see the root `README-PATCH.md`).
//! The tests below derive each of those three facts from `Cargo.toml` /
//! `src/win/backend.rs` instead of restating a snapshot, the same strategy
//! #798's tests already use for version and primitive count.

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

fn cargo_toml() -> String {
    read(crate_root().join("Cargo.toml"))
}

fn win_backend_rs() -> String {
    read(crate_root().join("src/win/backend.rs"))
}

/// Feature names declared in `Cargo.toml`'s `[features]` table — the
/// crate's own definition of "what features exist", parsed the same
/// mechanical way [`primitive_module_count`] derives primitive count,
/// rather than hand-listed.
fn declared_feature_names() -> Vec<String> {
    let toml = cargo_toml();
    let start = toml
        .find("\n[features]")
        .expect("Cargo.toml has a [features] table")
        + 1;
    let after = &toml[start..];
    let body_start = after
        .find('\n')
        .expect("[features] header has a newline after it")
        + 1;
    let body = &after[body_start..];
    // The table ends at the next top-level `[section]` header.
    let end = body.find("\n[").unwrap_or(body.len());
    let body = &body[..end];

    let names: Vec<String> = body
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.starts_with('#') || l.is_empty() {
                return None;
            }
            // A feature declaration line looks like `name = [...]` or
            // `name = ["..."]` possibly spanning multiple lines — only the
            // first line (with the `=`) carries the name.
            let (name, rest) = l.split_once('=')?;
            let name = name.trim();
            let is_ident = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            (is_ident && rest.trim_start().starts_with('[')).then(|| name.to_string())
        })
        .collect();
    assert!(
        !names.is_empty(),
        "Cargo.toml's [features] table parsed to zero feature names — \
         either the table emptied out or this test's parsing broke \
         (it expects `name = [...]` lines). Investigate before trusting \
         the names below."
    );
    names
}

/// The root README's `## Features` section body, up to the next `##`
/// heading — so a feature name mentioned elsewhere in the README (e.g. in
/// prose) doesn't count as "listed".
fn readme_features_section() -> String {
    let readme = root_readme();
    let start = readme
        .find("## Features")
        .expect("root README.md has a `## Features` section");
    let after = &readme[start..];
    let end = after[2..]
        .find("\n## ")
        .map(|i| i + 2)
        .unwrap_or(after.len());
    after[..end].to_string()
}

fn backend_md() -> String {
    read(crate_root().join("docs/BACKEND.md"))
}

fn ui_crate_design_md() -> String {
    read(crate_root().join("docs/UI_CRATE_DESIGN.md"))
}

fn clipboard_md() -> String {
    read(crate_root().join("docs/CLIPBOARD.md"))
}

fn tui_services_rs() -> String {
    read(crate_root().join("src/tui/services.rs"))
}

fn compose_mod_rs() -> String {
    read(crate_root().join("src/compose/mod.rs"))
}

fn compose_md() -> String {
    read(crate_root().join("docs/COMPOSE.md"))
}

fn canvas_rs() -> String {
    read(crate_root().join("src/primitives/canvas.rs"))
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
fn lib_doc_does_not_claim_still_unpublished() {
    // #1355: v0.1.0 shipped to crates.io on 2026-10-07. `src/lib.rs`'s
    // crate doc is what docs.rs actually renders as the package's front
    // page, and it kept saying "prepared for its first publish ... but
    // not yet published" well after the publish happened. This fix only
    // reaches docs.rs on the *next* published release (0.1.1), since
    // docs.rs renders whatever `src/lib.rs` looked like at the version
    // it built — see the CHANGELOG `[Unreleased]` entry.
    let lib = lib_rs();
    assert!(
        !lib.contains("not yet published"),
        "quadraui/src/lib.rs's crate doc still says \"not yet published\", \
         but CARGO_PKG_VERSION = {} has already been released to \
         crates.io (see CHANGELOG.md). Update the `## Status` section to \
         describe the published state instead.",
        env!("CARGO_PKG_VERSION")
    );
}

/// The crates.io version requirement every published-dependency snippet
/// must show, e.g. `0.1.0` -> `0.1`. Derived from `CARGO_PKG_VERSION` the
/// same mechanical way [`major_minor_x`] derives the pre-1.0 status
/// string, so a release bump that moves the minor version doesn't leave a
/// stale snippet a reader copies and cannot resolve.
fn major_minor() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut parts = version.split('.');
    let major = parts.next().expect("CARGO_PKG_VERSION has a major segment");
    let minor = parts.next().expect("CARGO_PKG_VERSION has a minor segment");
    format!("{major}.{minor}")
}

#[test]
fn readme_leads_with_a_resolvable_published_version() {
    // #1355: v0.1.0 published to crates.io on 2026-10-07. The root
    // README's Getting started section is the first thing a prospective
    // consumer reads, and it must lead with a `quadraui = { version =
    // "..." }` dependency a plain `cargo add`/`cargo build` can actually
    // resolve, not just the git-rev pin the in-house consumers use to
    // track unreleased work.
    let want = major_minor();
    let readme = root_readme();
    let needle = format!("quadraui = {{ version = \"{want}\"");
    assert!(
        readme.contains(&needle),
        "root README.md's Getting started section doesn't contain \
         \"{needle}\" (derived from CARGO_PKG_VERSION = {}). quadraui has \
         been published to crates.io since v0.1.0 (#1355) — the README \
         should lead with a resolvable `quadraui = {{ version = \"{want}\", \
         features = [...] }}` dependency snippet, not a git-rev pin.",
        env!("CARGO_PKG_VERSION")
    );
}

// ── Consumer pin shape (#1107, inverted by #1355) ────────────────────────

#[test]
fn readme_does_not_attribute_the_path_dependency_shape_to_vimcode() {
    // #1107: README's sample Cargo.toml block labelled the `path =`
    // dependency snippet "vimcode's approach". vimcode moved to a pinned
    // `git` + `rev` dependency in vimcode#691 — the same shape coord-tui
    // uses — and no longer path-deps a sibling checkout; the vendored
    // vt100 patch that the old path shape depended on is gone too
    // (quadraui#795, see root `README-PATCH.md`'s regression note). A
    // reader who copies the labelled snippet today gets a dependency
    // shape neither real consumer actually uses.
    let readme = root_readme();
    assert!(
        !readme.to_lowercase().contains("vimcode's approach"),
        "root README.md still labels a Cargo.toml dependency snippet \
         \"vimcode's approach\". vimcode pins quadraui via `git` + `rev` \
         now (vimcode#691), not a `path =` sibling-checkout dependency — \
         re-verify which shape (if any) vimcode actually uses before \
         attributing a snippet to it again."
    );
}

#[test]
fn readme_still_mentions_git_rev_pin_for_unreleased_work() {
    // #1355: now that v0.1.0 is on crates.io, the git+rev pin is no
    // longer the *lead* Getting started snippet (see
    // `readme_leads_with_a_resolvable_published_version` above) — but it
    // is still real: both coord-tui and vimcode can use it to track
    // unreleased work ahead of the next crates.io release, and the
    // README should keep saying so, just not first.
    let readme = root_readme();
    assert!(
        readme.contains("rev = \"<commit-sha>\"") || readme.contains("rev = \"<pinned sha>\""),
        "root README.md no longer mentions a `git` + `rev` pin anywhere. \
         That shape is still how a consumer tracks unreleased work ahead \
         of the next crates.io release (both coord-tui and vimcode use \
         it) — keep it mentioned in the README, just not as the lead \
         Getting started snippet now that a resolvable published version \
         exists."
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

#[test]
fn canvas_doc_states_the_real_other_primitive_count() {
    // #1393: `Canvas` is itself one of the `pub mod` entries
    // `primitive_module_count` counts, so its own module doc's "the N
    // other shipped primitives" phrasing must say one less than the
    // total — it went stale at "40 other" against a 42-module crate (41
    // others) after #798/#1107 kept `lib.rs`/README.md in sync but never
    // taught this file about `canvas.rs`'s own doc comment.
    let others = primitive_module_count() - 1;
    let canvas = canvas_rs();
    let needle = format!("{others} other shipped primitive");
    assert!(
        canvas.contains(&needle),
        "quadraui/src/primitives/canvas.rs's module doc doesn't contain \
         \"{needle}\", but src/primitives/mod.rs declares {} `pub mod` \
         primitive modules total ({others} other than `canvas` itself). \
         A primitive was added or removed without updating canvas.rs's \
         doc comment, or its count was hand-edited to a wrong number.",
        others + 1
    );
}

// ── Feature list (#1107) ─────────────────────────────────────────────────

#[test]
fn root_readme_features_section_lists_every_cargo_feature() {
    // #1107: README's `## Features` section said only `tui`/`gtk` while
    // Cargo.toml had grown `terminal`, `win`, and `macos` on top of those.
    // Derive the real feature set from Cargo.toml instead of re-pinning a
    // hand-written list that will drift the next time a feature is added.
    let features = declared_feature_names();
    let section = readme_features_section();
    for name in &features {
        let needle = format!("`{name}`");
        assert!(
            section.contains(&needle),
            "root README.md's `## Features` section doesn't mention \
             \"{needle}\", but Cargo.toml declares a `{name}` feature. \
             Add a bullet for it (see the other entries for the format) \
             — this is the exact drift #1107 fixed (the section only \
             named `tui`/`gtk` after `terminal`/`win`/`macos` existed)."
        );
    }
}

#[test]
fn root_readme_does_not_list_a_feature_cargo_toml_no_longer_has() {
    // The inverse direction: a feature bullet left behind after a feature
    // was renamed or removed tells a reader to enable something that no
    // longer exists.
    let features = declared_feature_names();
    let section = readme_features_section();
    for bullet_name in ["terminal", "tui", "gtk", "win", "macos"] {
        let mentioned = section.contains(&format!("`{bullet_name}`"));
        let declared = features.iter().any(|f| f == bullet_name);
        assert!(
            !mentioned || declared,
            "root README.md's `## Features` section mentions `{bullet_name}`, \
             but Cargo.toml's [features] table no longer declares it. \
             Remove the stale bullet."
        );
    }
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

/// Number of non-comment `todo!()` macro calls in `src/win/backend.rs` —
/// a crude but mechanical line-based check (skip lines whose trimmed start
/// is a `//` comment) rather than a real Rust parser, same trade-off
/// [`primitive_module_count`]'s `pub mod` grep makes.
fn win_backend_real_todo_call_count() -> usize {
    win_backend_rs()
        .lines()
        .filter(|l| {
            let trimmed = l.trim_start();
            !trimmed.starts_with("//") && l.contains("todo!()")
        })
        .count()
}

#[test]
fn win_backend_todo_claim_tracks_source_reality() {
    // #1107: README.md said "most `Backend::draw_*`/`*_layout` methods on
    // `WinBackend` are `todo!()` stubs" after src/win/backend.rs had
    // already been filled in for real (0 non-comment `todo!()` calls left).
    // Same shape as `backend_error_doc_claim_tracks_source_reality` below:
    // check both directions so neither "claims a stub gap that's gone" nor
    // "silently overclaims completeness" can land unnoticed.
    //
    // #1355: `src/lib.rs`'s `## Status` (docs.rs's front page) made the
    // exact same claim independently of README.md, and rotted the same
    // way — so it's checked here too, not just the README.
    let real_todo_calls = win_backend_real_todo_call_count();

    for (doc_name, doc) in [("README.md", root_readme()), ("src/lib.rs", lib_rs())] {
        let lower = doc.to_lowercase();
        let claims_todo_stubs = lower.contains("todo!()` stubs") || lower.contains("todo!() stubs");

        if real_todo_calls == 0 {
            assert!(
                !claims_todo_stubs,
                "{doc_name} still claims WinBackend draw_*/*_layout \
                 methods are `todo!()` stubs, but src/win/backend.rs has \
                 zero non-comment `todo!()` macro calls left. Update the \
                 Windows status paragraph to describe the real remaining \
                 gap (its conformance-matrix burn-down status in \
                 tests/conformance.rs) instead of a stub count that no \
                 longer exists — this is the exact drift #1107 fixed \
                 (and #1355 found it had crept back into lib.rs too)."
            );
        } else {
            assert!(
                claims_todo_stubs,
                "src/win/backend.rs has {real_todo_calls} real `todo!()` \
                 macro call(s) left, but {doc_name} no longer mentions \
                 `todo!()` stubs for the Windows backend — restore an \
                 accurate claim instead of silently overclaiming \
                 completeness."
            );
        }
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

// ── Clipboard docs vs. the clipboard code (#331) ─────────────────────────
//
// #331 was reported as "Ctrl-A + Ctrl-C doesn't reach the clipboard inside
// tmux". The code was already right — `TuiClipboard::write_text` detects
// `$TMUX` and emits a DCS-passthrough-wrapped OSC 52 alongside the raw one
// — but the tmux configuration both forms depend on (`set-clipboard on` /
// `allow-passthrough on`) was documented only in a private module comment,
// so from a user's seat the feature was indistinguishable from broken.
// `docs/CLIPBOARD.md` is the fix. These tests keep it pinned to the code
// it describes, so a future change to the emission strategy can't leave
// the troubleshooting steps quietly pointing at the wrong knob.

#[test]
fn clipboard_doc_names_both_tmux_knobs_the_code_depends_on() {
    let doc = clipboard_md();
    for knob in ["set-clipboard on", "allow-passthrough on"] {
        assert!(
            doc.contains(knob),
            "docs/CLIPBOARD.md no longer mentions `{knob}`. quadraui emits \
             BOTH a raw OSC 52 sequence (gated by tmux's `set-clipboard`) \
             and a DCS-passthrough-wrapped copy (gated by \
             `allow-passthrough`), so a user whose copy silently fails \
             needs both names to diagnose it. If the emission strategy \
             changed so one of these is genuinely no longer required, \
             update the doc's tmux section deliberately rather than \
             dropping the name."
        );
    }
}

#[test]
fn tui_clipboard_still_emits_the_tmux_passthrough_the_doc_promises() {
    // The two halves of the tmux story in `docs/CLIPBOARD.md`: detection
    // off `$TMUX`, and the DCS passthrough wrapper it enables. If either
    // disappears from the code, the doc is lying about what happens
    // inside tmux and #331 regresses into an undiagnosable bug report.
    let src = tui_services_rs();
    assert!(
        src.contains("\"TMUX\""),
        "src/tui/services.rs no longer reads $TMUX. docs/CLIPBOARD.md tells \
         users the tmux passthrough copy is emitted automatically when \
         `$TMUX` is set — if detection moved elsewhere, repoint this test \
         and the doc at wherever it lives now."
    );
    assert!(
        src.contains("Ptmux;"),
        "src/tui/services.rs no longer emits tmux's DCS passthrough \
         introducer (`ESC P tmux ;`). That wrapper is the leg that works \
         under `allow-passthrough on`, and docs/CLIPBOARD.md documents it \
         as one of the two forms every copy sends (#331)."
    );
}

#[test]
fn clipboard_doc_lists_every_native_clipboard_tool_the_code_tries() {
    // The doc's troubleshooting step 5 tells users to check for these
    // tools by name. A tool added to (or dropped from) the candidate list
    // without a doc update sends people looking for the wrong binary.
    let src = tui_services_rs();
    let doc = clipboard_md();
    for tool in ["wl-copy", "xclip", "xsel"] {
        assert_eq!(
            src.contains(tool),
            doc.contains(tool),
            "`{tool}` appears in exactly one of src/tui/services.rs and \
             docs/CLIPBOARD.md. The doc's native-tool leg table and its \
             troubleshooting steps name these binaries so a user can check \
             `which {tool}` — they have to be the same set the code \
             actually spawns."
        );
    }
}

// ── Compose controllers (#1128) ──────────────────────────────────────────
//
// #1128's audit of `develop @ ed402b4` found the root README's "Compose
// Helpers" section naming three of fourteen-plus `src/compose/` modules —
// the controllers own real state machines and emit semantic events, and
// the audit rated them the best-and-least-documented part of the crate.
// The fix is `docs/COMPOSE.md` (one section per module) plus the tests
// below, which derive "every compose module" from `src/compose/mod.rs`'s
// own `pub mod` list — the same mechanical strategy
// [`primitive_module_count`] already uses for primitives — so adding
// module #24 without documenting it fails CI instead of waiting for the
// next manual audit.

/// Every `pub mod <name>;` declared in `src/compose/mod.rs`, in file
/// order. This is the crate's own definition of "what compose modules
/// exist" — a human doing `ls src/compose/*.rs` would see the same set.
fn compose_module_names() -> Vec<String> {
    let src = compose_mod_rs();
    let names: Vec<String> = src
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("pub mod ")?;
            let name = rest.trim_end_matches(';').trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect();
    assert!(
        !names.is_empty(),
        "src/compose/mod.rs has no `pub mod` declarations — either the \
         module emptied out or this test's parsing broke. Investigate \
         before trusting the list below."
    );
    names
}

/// The root README's "Compose controllers" table body, from the
/// `**Compose controllers**` paragraph up to (not including) the
/// `**Platform services:**` paragraph that follows it — so a controller
/// name mentioned elsewhere in the README doesn't count as "listed here".
fn readme_compose_section() -> String {
    let readme = root_readme();
    let start = readme
        .find("**Compose controllers**")
        .expect("root README.md has a `**Compose controllers**` paragraph");
    let after = &readme[start..];
    let end = after.find("**Platform services:**").expect(
        "root README.md has a `**Platform services:**` paragraph after Compose controllers",
    );
    after[..end].to_string()
}

/// For a `src/compose/<stem>.rs` module, the public type name(s) it is
/// expected to expose, by the convention nearly every compose module
/// follows: `PascalCase(stem)` directly (`app_shell` -> `AppShell`) or
/// `PascalCase(stem) + "Controller"` (`workspace` -> `WorkspaceController`,
/// `tab_group` -> `TabGroupController`). A handful of modules don't fit
/// either shape — because they expose more than one primary type
/// (`help_layer` -> `HelpRegistry` + `HelpOverlayController`) or because
/// they're a utility rather than a stateful controller (`markdown`,
/// `notification`) — and are listed here explicitly instead of guessed at.
/// Adding a *conventionally-named* module needs no change here: the
/// fallback derives its expected name automatically.
fn expected_compose_type_names(stem: &str) -> Vec<String> {
    fn pascal_case(stem: &str) -> String {
        stem.split('_')
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect()
    }

    match stem {
        "help_layer" => vec![
            "HelpRegistry".to_string(),
            "HelpOverlayController".to_string(),
        ],
        // Utility modules, not stateful controllers — the README's prose
        // after the table names them without wrapping a type name in
        // backticks, so these two are matched as plain substrings by
        // `readme_compose_table_names_every_compose_controller` below
        // rather than `` `Name` ``.
        "markdown" => vec!["Markdown".to_string()],
        "notification" => vec!["notify_or_toast".to_string()],
        _ => {
            let base = pascal_case(stem);
            let mut names = vec![base.clone()];
            if !base.ends_with("Controller") {
                names.push(format!("{base}Controller"));
            }
            names
        }
    }
}

#[test]
fn docs_compose_md_covers_every_compose_module() {
    let doc = compose_md();
    for stem in compose_module_names() {
        let needle = format!("src/compose/{stem}.rs");
        assert!(
            doc.contains(&needle),
            "quadraui/docs/COMPOSE.md has no section referencing `{needle}`. \
             #1128 asks for one section per compose controller — add one \
             (what it owns, the events it emits, the smallest wiring \
             example, and which example file shows it) naming its source \
             file so this check can find it again."
        );
    }
}

#[test]
fn readme_compose_table_names_every_compose_controller() {
    let section = readme_compose_section();
    let plain_substring_match = ["markdown", "notification"];
    for stem in compose_module_names() {
        let candidates = expected_compose_type_names(&stem);
        let found = if plain_substring_match.contains(&stem.as_str()) {
            candidates.iter().any(|name| section.contains(name))
        } else {
            candidates
                .iter()
                .any(|name| section.contains(&format!("`{name}`")))
        };
        assert!(
            found,
            "root README.md's Compose controllers table doesn't mention \
             any of {candidates:?} (derived from `src/compose/{stem}.rs`). \
             This is the exact drift #1128 fixed (README named 3 of 14+ \
             compose controllers) — add a row for it, or if it's a \
             non-conventionally-named module, update \
             `expected_compose_type_names` in this test alongside \
             `docs/COMPOSE.md`."
        );
    }
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

// ── Test count (#1393) ──────────────────────────────────────────────────

/// Every `#[test]`-attributed item under this crate's own `src/` and
/// `tests/` — the same mechanical "count the real thing instead of
/// re-pinning a snapshot" strategy [`primitive_module_count`] already uses.
/// Deliberately a line-level `starts_with("#[test]")` check (not a plain
/// substring `contains`): a doc comment that merely *mentions* `#[test]`
/// in prose (this very file has several) must not inflate the count.
///
/// Scope: only `quadraui/src` and `quadraui/tests` — not `kubeui*` or
/// `quadraui-gallery`, even though the root README's "Heavily tested"
/// bullet this backs reads as a claim about the whole workspace. Widening
/// this walk to match that literally would move the count by enough to
/// jump a bucket on its own; keep the walk narrow and treat the bullet's
/// "Around N" wording as deliberately approximate rather than "fixing" it
/// by widening the walk.
fn test_fn_count() -> usize {
    fn walk(dir: &std::path::Path, total: &mut usize) {
        let entries = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("failed to read dir {}: {e}", dir.display()));
        for entry in entries {
            let entry = entry.expect("dir entry readable");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, total);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = read(path);
                *total += text
                    .lines()
                    .filter(|l| l.trim_start().starts_with("#[test]"))
                    .count();
            }
        }
    }
    let mut total = 0;
    walk(&crate_root().join("src"), &mut total);
    walk(&crate_root().join("tests"), &mut total);
    assert!(
        total > 0,
        "found zero `#[test]` items under quadraui/src and quadraui/tests — \
         either the crate's tests emptied out or this test's parsing broke. \
         Either way, investigate before trusting the count below."
    );
    total
}

/// Round `n` down to the nearest `step`. The root README's "Heavily
/// tested" bullet states an approximate count ("Around N tests"), not an
/// exact one — rounding both the stated claim and the real count to the
/// same bucket means this test only needs re-running (and the bullet
/// re-editing) once the real count crosses into the next bucket, not on
/// every single `#[test]` a PR adds.
fn round_down(n: usize, step: usize) -> usize {
    (n / step) * step
}

/// Render a bucketed test count the way the README spells it, e.g.
/// `5100` -> `"5,100"`. The loop below groups from the right in
/// threes regardless of digit count, so it handles any `n` this crate
/// could plausibly reach, not just today's 4-digit range.
fn with_thousands_separator(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[test]
fn root_readme_states_the_real_test_count_ballpark() {
    // #1393: the "Heavily tested" bullet said "Around 4,000 tests" against
    // a crate with over 5,000 — stale since whenever that bullet was last
    // hand-edited. `TEST_COUNT_BUCKET` is the granularity both this test
    // and the README text round to; bump the README bullet (and nothing
    // else) when this test starts failing.
    const TEST_COUNT_BUCKET: usize = 100;
    let actual = test_fn_count();
    let bucket = round_down(actual, TEST_COUNT_BUCKET);
    let readme = root_readme();
    let needle = format!("Around {} tests", with_thousands_separator(bucket));
    assert!(
        readme.contains(&needle),
        "root README.md's \"Heavily tested\" bullet doesn't contain \
         \"{needle}\", but this crate has {actual} `#[test]` items under \
         src/ and tests/ (rounded down to the nearest {TEST_COUNT_BUCKET}). \
         This is the exact drift #1393 fixed (\"Around 4,000 tests\" \
         claimed against a crate with over 5,000) — update the bullet's \
         count to match."
    );
}
