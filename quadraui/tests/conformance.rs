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

#[path = "../examples/common/mod.rs"]
mod common;

#[path = "conformance/fixtures.rs"]
mod fixtures;
#[path = "conformance/runner.rs"]
mod runner;
#[path = "conformance/schema.rs"]
mod schema;

use std::fs;
use std::path::{Path, PathBuf};

use runner::{render_matrix, run_scenario, BackendReg, MatrixRow, Outcome};
use schema::Scenario;

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

/// Capabilities the TUI backend declares. Honest, not aspirational: each
/// is exercised by at least one Tier-1 scenario, and removing one here
/// turns the scenarios that need it into visible `skip` rows rather than
/// silently-green ones.
#[cfg(feature = "tui")]
const TUI_CAPS: &[&str] = &["mouse", "scroll", "drag", "text_selection"];

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

#[cfg(feature = "gtk")]
const GTK_CAPS: &[&str] = &["mouse", "scroll", "drag", "text_selection"];

/// Every backend compiled into this build. **This is the registration
/// point** — a new backend adds exactly one `push` here.
// `vec_init_then_push`: each push is behind its own `#[cfg]`, so the
// `vec![…]` form clippy suggests would need the cfg attributes on macro
// arguments — less legible than the one-push-per-backend shape this
// file's whole "add a backend = one line here" contract rests on.
#[allow(clippy::vec_init_then_push)]
fn backends() -> Vec<BackendReg> {
    #[allow(unused_mut)]
    let mut regs: Vec<BackendReg> = Vec::new();
    #[cfg(feature = "tui")]
    regs.push(BackendReg::new(
        "tui",
        TUI_CAPS,
        fixtures::build::<TuiFactory>,
    ));
    #[cfg(feature = "gtk")]
    regs.push(BackendReg::new(
        "gtk",
        GTK_CAPS,
        fixtures::build::<GtkFactory>,
    ));
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
