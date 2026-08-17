//! **Conformance suite runner** (quadraui#491, epic #480; audit §6.4–§6.6).
//!
//! Crosses every scenario file under
//! `tests/conformance/scenarios/**/*.scn.json` with every registered
//! backend driver and prints a scenario × backend matrix of
//! pass / skip / FAIL.
//!
//! ```sh
//! cargo test --features tui  --test conformance -- --nocapture
//! cargo test --features gtk  --test conformance -- --nocapture
//! cargo test --features tui,gtk --test conformance -- --nocapture
//! ```
//!
//! The matrix is also written to `target/conformance-matrix.txt` so CI can
//! upload it as an artifact. For a backend that doesn't exist yet
//! (Windows, macOS) that artifact **is** the implementation checklist.
//!
//! ## The two costs this file fixes
//!
//! - **Adding a backend = 1 driver impl + 1 registration line.** The impl
//!   is a [`DriverFactory`] (three lines: box the driver); the
//!   registration is one `push` in [`backends`] behind that backend's
//!   feature gate. Nothing else in the suite mentions a backend by name.
//! - **Adding a scenario = 1 JSON file, no Rust.** Scenario files are
//!   discovered from disk at run time, so a new `.scn.json` is picked up
//!   with no registration anywhere.
//!
//! ## Capabilities and skips
//!
//! Each backend declares a capability list. A scenario whose `requires`
//! names a capability the backend hasn't declared is **skipped, with the
//! missing capability printed** — the audit's "silence is impossible"
//! rule. A skip is never a silent pass, and a backend cannot skip a
//! scenario without having declared the gap up front.
//!
//! That list is not written here. It is
//! [`quadraui::Backend::backend_caps`] on the real backend instance,
//! reached through [`runner::DriverFactory::caps`] (quadraui#492): there
//! used to be a hand-maintained `TUI_CAPS`/`GTK_CAPS` array beside each
//! registration, and nothing tied it to what the backend actually
//! implements, so the two vocabularies could drift in both directions.
//! Three tests hold the single vocabulary together now:
//!
//! - [`every_requires_names_a_known_capability`] — a scenario cannot
//!   `require` a name [`quadraui::BackendCaps`] has no field for.
//! - [`every_capability_is_required_by_some_scenario_or_named_as_unused`]
//!   — the reverse direction: a `BackendCaps` field no scenario can
//!   reference is either a gap to fill or a fact to write down.
//! - [`caps::backends_declare_only_what_they_override`] — the honesty
//!   check: a declared capability whose `Backend` methods are still the
//!   trait's no-op default is a lie, and so is an undeclared one whose
//!   methods are overridden.

// Under a feature set with no C0 driver backend — e.g. `--features macos`
// alone (`quadraui::macos` itself needs `target_os = "macos"` too, so the
// feature is a no-op on other hosts), which is what
// `.github/workflows/macos.yml`'s legs see, and it sets
// `RUSTFLAGS: -D warnings` — the whole C0 harness (`Case`, `DriverFactory`,
// `c0::run`, `fixtures::build`, …) has no caller and every item in it reads
// as dead. It is not dead; it is unreachable *from this feature set*, which
// is the same reason `backends()` is empty here. Scoped to exactly that
// case so a genuinely-unused item still surfaces on the tui / gtk builds
// that own this suite. `MacFactory` (quadraui#493) registers for the
// Tier-1 scenario suite only — it never touches the Tier-0 harness in
// `c0.rs` (see `c0_paint_smoke`'s doc below: `MacBackend` is deliberately
// not in that tier's columns yet) — so a real `--features macos` build on
// `target_os = "macos"` still has no Tier-0 caller and stays on the
// dead-code-allowed side until the Tier-0 macOS follow-up lands.
// (quadraui#484.)
#![cfg_attr(not(any(feature = "tui", feature = "gtk")), allow(dead_code))]

#[path = "../examples/common/mod.rs"]
mod common;

#[path = "conformance/c0.rs"]
mod c0;
#[path = "conformance/caps.rs"]
mod caps;
#[path = "conformance/fixtures.rs"]
mod fixtures;
#[path = "conformance/runner.rs"]
mod runner;
#[path = "conformance/schema.rs"]
mod schema;

use std::fs;
use std::path::{Path, PathBuf};

use runner::{render_matrix, run_scenario, zones_seen, BackendReg, MatrixRow, Outcome};
use schema::{Scenario, Step};

// ─── Backend registrations ──────────────────────────────────────────────
//
// One `DriverFactory` impl + one `backends()` line per backend. Both are
// gated on that backend's feature, so a TUI-only build never links GTK.

#[cfg(feature = "tui")]
struct TuiFactory;

#[cfg(feature = "tui")]
impl runner::DriverFactory for TuiFactory {
    fn make<A: quadraui::AppLogic + 'static>(
        app: A,
        viewport: quadraui::testing::LogicalViewport,
    ) -> Box<dyn runner::DynDriver> {
        use quadraui::testing::ConformanceDriver;
        Box::new(quadraui::tui::testing::TuiDriver::new_fixture(
            app, viewport,
        ))
    }
}

#[cfg(feature = "gtk")]
struct GtkFactory;

#[cfg(feature = "gtk")]
impl runner::DriverFactory for GtkFactory {
    fn make<A: quadraui::AppLogic + 'static>(
        app: A,
        viewport: quadraui::testing::LogicalViewport,
    ) -> Box<dyn runner::DynDriver> {
        use quadraui::testing::ConformanceDriver;
        Box::new(quadraui::gtk::testing::GtkDriver::new_fixture(
            app, viewport,
        ))
    }
}

// `target_os = "macos"` as well as the feature: `macos` is a no-op flag on
// non-macOS hosts (see `Cargo.toml`'s comment on the feature), so
// `quadraui::macos` itself only compiles under both — this mirrors that
// gate rather than fighting it.
#[cfg(all(feature = "macos", target_os = "macos"))]
struct MacFactory;

#[cfg(all(feature = "macos", target_os = "macos"))]
impl runner::DriverFactory for MacFactory {
    fn make<A: quadraui::AppLogic + 'static>(
        app: A,
        viewport: quadraui::testing::LogicalViewport,
    ) -> Box<dyn runner::DynDriver> {
        use quadraui::testing::ConformanceDriver;
        Box::new(quadraui::macos::testing::MacDriver::new_fixture(
            app, viewport,
        ))
    }
}

/// Every backend compiled into this build. **This is the registration
/// point** — a new backend adds exactly one `push` here.
///
/// Note what is *not* here: a capability list. `BackendReg::register`
/// reads it off the backend itself (quadraui#492), so a registration
/// cannot claim a capability the backend doesn't declare, and a backend
/// cannot declare one the runner ignores.
// `vec_init_then_push`: each push is behind its own `#[cfg]`, so the
// `vec![…]` form clippy suggests would need the cfg attributes on macro
// arguments — less legible than the one-push-per-backend shape this
// file's whole "add a backend = one line here" contract rests on.
#[allow(clippy::vec_init_then_push)]
fn backends() -> Vec<BackendReg> {
    #[allow(unused_mut)]
    let mut regs: Vec<BackendReg> = Vec::new();
    #[cfg(feature = "tui")]
    regs.push(BackendReg::register::<TuiFactory>("tui"));
    #[cfg(feature = "gtk")]
    regs.push(BackendReg::register::<GtkFactory>("gtk"));
    #[cfg(all(feature = "macos", target_os = "macos"))]
    regs.push(BackendReg::register::<MacFactory>("macos"));
    regs
}

// ─── Scenario discovery ─────────────────────────────────────────────────

fn scenarios_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance/scenarios")
}

/// Every `*.scn.json` under `scenarios/`, recursively, sorted by path so
/// the matrix is stable across runs and filesystems.
fn scenario_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("conformance: cannot read {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("conformance: unreadable dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().ends_with(".scn.json") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Load and validate every scenario file. A parse error, or an `id` that
/// disagrees with the filename, fails here rather than producing a
/// mislabelled matrix row.
fn load_scenarios() -> Vec<Scenario> {
    let dir = scenarios_dir();
    let files = scenario_files(&dir);
    assert!(
        !files.is_empty(),
        "conformance: no *.scn.json under {}",
        dir.display()
    );
    files
        .iter()
        .map(|path| {
            let src = fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("conformance: cannot read {}: {e}", path.display()));
            let scenario = Scenario::from_json(&path.display().to_string(), &src)
                .unwrap_or_else(|e| panic!("conformance: {e}"));
            let stem = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".scn.json"))
                .expect("conformance: *.scn.json filename");
            assert_eq!(
                scenario.id,
                stem,
                "conformance: {} declares id {:?} but is named {:?} — the two must match so a \
                 matrix row is greppable back to its file",
                path.display(),
                scenario.id,
                stem
            );
            scenario
        })
        .collect()
}

// ─── The suite ──────────────────────────────────────────────────────────

/// Run every scenario against every registered backend, print the matrix,
/// and fail if any cell failed.
#[test]
fn conformance_matrix() {
    let scenarios = load_scenarios();
    let backends = backends();

    let rows: Vec<MatrixRow> = scenarios
        .iter()
        .map(|s| MatrixRow {
            id: s.id.clone(),
            tier: s.tier,
            cells: backends
                .iter()
                .map(|b| (b.name, run_scenario(s, b)))
                .collect(),
        })
        .collect();

    let names: Vec<&'static str> = backends.iter().map(|b| b.name).collect();
    let table = render_matrix(&rows, &names);
    println!("{table}");
    write_artifact(&table);

    let failures: Vec<String> = rows
        .iter()
        .flat_map(|r| {
            r.cells
                .iter()
                .filter_map(move |(backend, outcome)| match outcome {
                    Outcome::Fail { step, reason } => {
                        Some(format!("{}/{} step {}: {}", r.id, backend, step, reason))
                    }
                    _ => None,
                })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} conformance cell(s) failed:\n{}\n{table}",
        failures.len(),
        failures.join("\n")
    );
}

/// Tier 0 — C0 paint smoke (quadraui#492, epic #480). Runs ahead of every
/// scenario-based tier: `c0::CASES` is a canned, minimal descriptor per
/// primitive, and this asserts `begin_frame` → draw → `end_frame` neither
/// panics nor produces a frame the inventory can't see at all (contract
/// §5b — "compiles must stop implying renders").
///
/// A registered backend with nothing to say for a given primitive prints
/// as a row here, same as a tier-1 `FAIL` — never silence (#492's second
/// acceptance bullet). Backends this build doesn't compile in (Win,
/// macOS) simply have no column, matching how `conformance_matrix`
/// already treats an unregistered backend: the *absence* of a Win/macOS
/// column in this artifact **is** the gap enumerated, not a hidden pass.
///
/// ## What this tier does *not* yet catch, stated plainly
///
/// quadraui#492's Problem section names the macOS `draw_diff_view` fake
/// (`macos/backend.rs`) as the motivating bug. **This test would not have
/// caught it.** `MacBackend` gained a `ConformanceDriver` (`MacDriver`,
/// quadraui#493) and a `backends()` row for the Tier-1 scenario suite
/// above, but is deliberately *not* added to this tier's `columns` below:
/// `draw_diff_view`'s known fake would turn straight into a hard
/// `c0_paint_smoke` failure the moment a real macOS host ran it, which is
/// its own follow-up rather than something to paper over here by, say,
/// special-casing that one row. `WinBackend` still has no
/// `ConformanceDriver` at all, so it has nothing to run on any host either
/// way. The `draw_diff_view` row below is proven on TUI and GTK only.
///
/// That is a pre-existing limitation inherited from #491's tier-1 suite
/// rather than something this tier introduced, and it is why the
/// capability half of #492 was deliberately built to read *source*
/// instead (`caps.rs`): `MacBackend`/`WinBackend` are checked there on
/// every run, on every platform. Closing the paint half needs a macOS/Win
/// `ConformanceDriver`, which is the follow-up — until it lands, C0's
/// coverage claim is "every primitive, on every backend that has a
/// driver", not "on every backend".
// `vec_init_then_push`: each push is behind its own `#[cfg]`, so the
// `vec![…]` form clippy suggests would need the cfg attributes on macro
// arguments — same trade-off `backends()` above already makes.
#[allow(clippy::vec_init_then_push)]
#[test]
fn c0_paint_smoke() {
    struct Column {
        name: &'static str,
        outcomes: Vec<c0::CaseOutcome>,
    }

    // `mut` is unused when neither push below is compiled in — see the
    // skip arm underneath.
    #[allow(unused_mut)]
    let mut columns: Vec<Column> = Vec::new();
    #[cfg(feature = "tui")]
    columns.push(Column {
        name: "tui",
        outcomes: c0::run::<TuiFactory>(),
    });
    #[cfg(feature = "gtk")]
    columns.push(Column {
        name: "gtk",
        outcomes: c0::run::<GtkFactory>(),
    });

    // With a C0 driver compiled in, an empty column set means the
    // registration above broke — still a hard failure, and still the
    // guard against a vacuous pass.
    #[cfg(any(feature = "tui", feature = "gtk"))]
    assert!(
        !columns.is_empty(),
        "c0_paint_smoke: no backend feature enabled — run with --features tui,gtk"
    );

    // Without one, there is no driver to smoke and nothing to prove
    // vacuously. `--features macos` alone is that feature set:
    // `MacBackend` has no `ConformanceDriver` (see `caps::ACCEPTED_DEFAULTS`
    // → `macos/register_zone` for why), and `macos.yml` runs
    // `cargo test -p quadraui --features macos`, where a hard failure here
    // would be this tier reporting on a backend it does not cover.
    // The gap itself is not silent: `caps::backends_declare_only_what_they
    // _override` prints the macOS column on every run. (quadraui#484.)
    if columns.is_empty() {
        println!(
            "c0_paint_smoke: SKIPPED — no C0 driver backend in this feature set. \
             Build with --features tui and/or gtk to run tier 0."
        );
        return;
    }

    assert!(
        !c0::CASES.is_empty(),
        "c0_paint_smoke: the descriptor table is empty, so this tier would pass vacuously"
    );

    let method_w = c0::CASES.iter().map(|c| c.method.len()).max().unwrap_or(0);
    let mut table = format!("{:<method_w$}", "primitive (tier 0)");
    for col in &columns {
        table.push_str(&format!("  {:<6}", col.name));
    }
    table.push('\n');

    let mut gaps: Vec<String> = Vec::new();
    for (i, case) in c0::CASES.iter().enumerate() {
        table.push_str(&format!("{:<method_w$}", case.method));
        for col in &columns {
            let outcome = &col.outcomes[i];
            let verdict = if !outcome.survived {
                "PANIC"
            } else if !outcome.text_ok || !outcome.observable {
                "FAIL"
            } else {
                "pass"
            };
            table.push_str(&format!("  {verdict:<6}"));
            if verdict != "pass" {
                let why = if !outcome.survived {
                    "panicked mid-paint".to_string()
                } else if !outcome.text_ok {
                    format!(
                        "handed {:?} and the frame does not report it — {}",
                        case.needle, outcome.reported
                    )
                } else {
                    format!(
                        "reported neither a text run nor a zone — {}",
                        outcome.reported
                    )
                };
                gaps.push(format!("{}/{}: {why}", col.name, case.method));
            }
        }
        table.push('\n');
    }
    println!("{table}");

    assert!(
        gaps.is_empty(),
        "{} C0 paint-smoke gap(s) — a primitive either panicked, dropped its text, or left the \
         frame unobservable, which contract §5b (tests/acceptance/ms-11/contract.md) treats as \
         indistinguishable from the trait's no-op default:\n{}\n{table}",
        gaps.len(),
        gaps.join("\n")
    );
}

/// Drop the matrix where CI can pick it up as an artifact. Best-effort:
/// an unwritable target directory must not fail the suite.
///
/// Honours `CARGO_TARGET_DIR` so a shared/relocated target directory
/// still gets the file; otherwise falls back to the workspace's own
/// `target/` (which `.gitignore` already covers).
fn write_artifact(table: &str) {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../target"));
    let path = target.join("conformance-matrix.txt");
    let _ = fs::create_dir_all(&target);
    let _ = fs::write(&path, table);
}

/// Every scenario names a fixture the registry knows. Catches a typo in a
/// new scenario file even in a no-backend build, where the runner would
/// otherwise have no driver to discover it with.
#[test]
fn every_scenario_names_a_registered_fixture() {
    for s in load_scenarios() {
        assert!(
            fixtures::FIXTURES.contains(&s.fixture.as_str()),
            "scenario {:?} names fixture {:?}, which is not in fixtures::FIXTURES {:?}",
            s.id,
            s.fixture,
            fixtures::FIXTURES
        );
    }
}

/// `FIXTURES` and `build`'s match arms must not drift: every advertised
/// name has to actually construct. Needs a backend to build *with*, so it
/// runs on TUI (the cheapest one) — the registry itself is backend-neutral,
/// so proving it once is enough.
#[cfg(feature = "tui")]
#[test]
fn every_advertised_fixture_builds() {
    use quadraui::testing::LogicalViewport;
    for name in fixtures::FIXTURES {
        assert!(
            fixtures::build::<TuiFactory>(name, LogicalViewport::new(80, 24)).is_some(),
            "fixtures::FIXTURES advertises {name:?} but fixtures::build has no arm for it"
        );
    }
    assert!(
        fixtures::build::<TuiFactory>("no_such_fixture", LogicalViewport::new(80, 24)).is_none(),
        "an unregistered fixture name must be None, not a panic"
    );
}

/// Every zone a scenario names in an `assert_inside` step is actually
/// registered by every backend that runs that scenario.
///
/// `FrameInventory::inside` answers `false` for an unregistered zone
/// exactly as it does for a needle that landed outside one, so without
/// this guard a step naming a zone **no backend ever registers** — a typo,
/// or an assertion written ahead of the `Backend::register_zone` call it
/// depends on — is an *unsatisfiable* step that reads in the matrix like
/// an ordinary layout failure. This test names the cause directly:
/// "nothing registers this id", separately from "the geometry doesn't
/// hold". Zone registration is per paint site (today: the shell chrome
/// regions and activity-bar items `AppShell::render` records — see
/// `docs/TESTING.md` → *Zone-backed assertions*), so a scenario reaching
/// for a not-yet-wired primitive fails here, at the step that reached,
/// rather than silently.
#[test]
fn every_asserted_zone_is_registered_by_every_backend() {
    let backends = backends();
    let mut problems: Vec<String> = Vec::new();

    for scenario in load_scenarios() {
        let wanted: Vec<&str> = scenario
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::AssertInside { zone, .. } => Some(zone.as_str()),
                _ => None,
            })
            .collect();
        if wanted.is_empty() {
            continue;
        }
        for backend in &backends {
            // `None` = this backend skips the scenario (declared capability
            // gap, already visible in the matrix) — nothing to check.
            let Some(seen) = zones_seen(&scenario, backend) else {
                continue;
            };
            for zone in &wanted {
                if !seen.contains(*zone) {
                    let seen: Vec<&str> = seen.iter().map(|s| s.as_str()).collect();
                    problems.push(format!(
                        "{}/{}: asserts `inside` zone {:?}, which {} never registers during \
                         the scenario (zones seen: {:?})",
                        scenario.id, backend.name, zone, backend.name, seen
                    ));
                }
            }
        }
    }

    assert!(
        problems.is_empty(),
        "{} unsatisfiable `assert_inside` step(s) — each names a zone no \
         `Backend::register_zone` call produces, so it can never pass:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// `caps.rs` reads each backend's declaration out of its **source**,
/// because "is this trait method overridden or defaulted?" is not a
/// question a running program can ask. This is the guard that keeps that
/// technique honest: for every backend this build actually compiles in,
/// the source-parsed declaration must equal the one the *running*
/// backend returns from `Backend::backend_caps()`.
///
/// Without it, a rustfmt change or an impl-header rename would degrade
/// `caps.rs` to parsing nothing — and a parse of nothing clears every
/// backend of every claim, i.e. the honesty check would go green exactly
/// when it stopped working. Here that failure is loud, and it names the
/// two lists that disagree.
///
/// Only asserts over compiled-in backends by construction (Win/macOS have
/// no driver to run), which is precisely why `caps.rs` keeps checking
/// those two from source on every platform.
#[allow(clippy::vec_init_then_push)]
#[test]
fn source_parsed_caps_match_the_running_backend() {
    #[allow(unused_mut)]
    let mut live: Vec<(&'static str, quadraui::BackendCaps)> = Vec::new();
    #[cfg(feature = "tui")]
    live.push(("tui", <TuiFactory as runner::DriverFactory>::caps()));
    #[cfg(feature = "gtk")]
    live.push(("gtk", <GtkFactory as runner::DriverFactory>::caps()));

    for (name, caps) in live {
        let running: std::collections::BTreeSet<&str> = caps.names().into_iter().collect();
        let parsed = caps::declared_in_source(name);
        assert_eq!(
            parsed, running,
            "{name}: `caps.rs` parsed {parsed:?} out of the backend's source, but the running \
             `Backend::backend_caps()` says {running:?}. The source parser is stale — fix it \
             before trusting any other result in `caps.rs`, including the Win/macOS rows it is \
             the only check for (quadraui#492)."
        );
    }
}

/// Every `requires` entry names a capability [`quadraui::BackendCaps`]
/// actually has a field for.
///
/// quadraui#492 review: `requires` used to be matched against a
/// hand-maintained `&[&str]` per backend, so a name outside *that* list
/// was an unsatisfiable gate that silently skipped forever — and a
/// `BackendCaps` field outside it was unreachable from any scenario. Now
/// there is one vocabulary, and this is the direction that catches a
/// typo: `BackendCaps::has` deliberately answers `false` for an unknown
/// name rather than panicking (so a real capability gap reads as a skip),
/// which means the typo has to be caught here, by name, or not at all.
///
/// Runs with no backend feature enabled too — the vocabulary is a
/// property of the library, not of whichever backends this build links.
#[test]
fn every_requires_names_a_known_capability() {
    let vocabulary = quadraui::BackendCaps::vocabulary();
    let mut problems: Vec<String> = Vec::new();
    for scenario in load_scenarios() {
        for cap in &scenario.requires {
            if !vocabulary.contains(&cap.as_str()) {
                problems.push(format!("{}: requires {cap:?}", scenario.id));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{} scenario `requires` entr(ies) name a capability `BackendCaps` has no field for, so \
         no backend can ever declare them and the scenario would skip everywhere, forever. \
         Either fix the typo or add the field (quadraui#492). Known capabilities: \
         {vocabulary:?}\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// The reverse direction: every capability in the vocabulary is either
/// exercised by a scenario or explicitly written down as not-yet-gated.
///
/// Without this, `BackendCaps` grows fields no `requires` list ever
/// references — the exact half of the drift the review named
/// ("`BackendCaps` has `native_menu`/`window_chrome`/… that no scenario
/// `requires` list can reference today"). The point is not to force a
/// scenario per capability, which would be busywork; it is that an
/// ungated capability has to be an acknowledged fact in
/// [`UNGATED_CAPS`] rather than an unnoticed one.
#[test]
fn every_capability_is_required_by_some_scenario_or_named_as_unused() {
    /// Capabilities no scenario gates on yet, each with the reason. Every
    /// entry here is a checklist item, not an excuse: deleting one and
    /// watching this test go red is how you find out a scenario now
    /// covers it.
    const UNGATED_CAPS: &[(&str, &str)] = &[
        (
            "native_menu",
            "only macOS declares it, and macOS has no ConformanceDriver yet (#493) — a \
             scenario gating on it would skip on every column this build has",
        ),
        (
            "window_chrome",
            "drag-to-move / double-click-maximize / edge-resize act on a real toplevel; \
             GtkDriver renders to an offscreen ImageSurface with no window to drive",
        ),
        (
            "pointer_cursor",
            "`set_cursor` changes the OS pointer glyph, which no headless driver observes — \
             `FrameInventory` has no notion of the cursor",
        ),
        (
            "ime",
            "no backend declares it (there is no backend-level IME method yet — see \
             `BackendCaps::ime`), so a gate on it would skip everywhere",
        ),
        (
            "file_dialogs",
            "a modal native picker cannot run headless; the `file_dialog_demo` fixture \
             exercises the app-side flow instead",
        ),
        (
            "notifications",
            "fire-and-forget to a system daemon — nothing paints, so no assertion in this \
             suite's vocabulary can observe it",
        ),
    ];

    let required: std::collections::BTreeSet<String> = load_scenarios()
        .iter()
        .flat_map(|s| s.requires.clone())
        .collect();
    let vocabulary = quadraui::BackendCaps::vocabulary();

    let unexplained: Vec<&str> = vocabulary
        .iter()
        .copied()
        .filter(|cap| !required.contains(*cap))
        .filter(|cap| !UNGATED_CAPS.iter().any(|(name, _)| name == cap))
        .collect();
    assert!(
        unexplained.is_empty(),
        "{} capability(ies) that no scenario `requires` and that `UNGATED_CAPS` does not \
         explain: {unexplained:?}. Add a scenario that gates on it, or add it to \
         `UNGATED_CAPS` with the reason it cannot be gated here (quadraui#492).",
        unexplained.len()
    );

    let stale: Vec<&str> = UNGATED_CAPS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| required.contains(*name) || !vocabulary.contains(name))
        .collect();
    assert!(
        stale.is_empty(),
        "`UNGATED_CAPS` still excuses {stale:?}, but each is now either gated by a scenario or \
         no longer a `BackendCaps` field — delete the stale entr(ies)"
    );
}

/// Tier-1 is the mandatory interaction core (audit §6.6 C1). Guard the
/// count so the "first 10 Tier-1 scenarios" this suite ships with can only
/// grow, never silently shrink when a file is deleted or retiered.
#[test]
fn ships_at_least_ten_tier_one_scenarios() {
    let tier1 = load_scenarios().iter().filter(|s| s.tier == 1).count();
    assert!(
        tier1 >= 10,
        "expected at least 10 Tier-1 scenarios (audit §6.6 C1), found {tier1}"
    );
}
