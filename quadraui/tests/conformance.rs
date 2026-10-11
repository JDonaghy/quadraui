//! **Conformance suite runner**.
//!
//! Crosses every scenario file under
//! `tests/conformance/scenarios/**/*.scn.json` with every registered
//! backend driver and prints a scenario × backend matrix of
//! pass / skip / FAIL.
//! ```sh
//! cargo test --features tui  --test conformance -- --nocapture
//! cargo test --features gtk  --test conformance -- --nocapture
//! cargo test --features tui,gtk --test conformance -- --nocapture
//! cargo test --features tui,terminal --test conformance -- --nocapture
//! ```
//! The Tier-1 matrix (`conformance_matrix`) is written to
//! `target/conformance-matrix.txt`, and the Tier-0 grid (`c0_paint_smoke`)
//! to `target/conformance-matrix-c0.txt` for CI artifacts. For a backend
//! not yet implemented, that artifact serves as the implementation checklist.
//!
//! ## Gating vs. reporting
//!
//! [`runner::Gating`] distinguishes backends under construction: `BackendReg::register`
//! is blocking, `BackendReg::register_burn_down` reports without gating, and
//! [`runner::verdict`] fails the suite if a burn-down column stops failing.
//! See `docs/TESTING.md` → *Burn-down columns* for details.
//!
//! ## Two TUI observers, one row each
//!
//! `--features tui,terminal` additionally registers `"tui-vt100"`
//! alongside `"tui"`: the same `AppLogic` fixtures and scenario files,
//! but painted through `ratatui::backend::CrosstermBackend` into a real ANSI
//! byte stream and read back with `vt100` — see `quadraui::tui::vt_testing` —
//! instead of `TestBackend`'s in-memory buffer. A draw-time paint inventory
//! can only answer "what did the rasteriser ask for"; it cannot answer
//! "what would a real terminal show", and those can diverge. Only scenarios
//! that opt in via `Scenario::text_fidelity` run this observer.
//! See `docs/TESTING.md` → *TUI: two observers* for details.
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
//! missing capability printed**. A skip is never a silent pass.
//!
//! That list is [`quadraui::Backend::backend_caps`] on the real backend
//! instance, reached through [`runner::DriverFactory::caps`]. Three tests
//! hold the single vocabulary together:
//!
//! - [`every_requires_names_a_known_capability`] — a scenario cannot
//!   `require` a name [`quadraui::BackendCaps`] has no field for.
//! - [`every_capability_is_required_by_some_scenario_or_named_as_unused`]
//!   — the reverse direction: a `BackendCaps` field no scenario can
//!   reference is either a gap to fill or a fact to write down.
//! - [`caps::backends_declare_only_what_they_override`] — the honesty
//!   check: a declared capability whose `Backend` methods are the
//!   trait's no-op default is a lie, and so is an undeclared one that
//!   is overridden.

// Under a feature set with no C0 driver backend — e.g. `--features macos`
// alone — the whole C0 harness has no caller and reads as dead, but is
// unreachable *from this feature set*. Scoped to exactly that case so a
// genuinely-unused item still surfaces on builds that own this suite.
#![cfg_attr(not(any(feature = "tui", feature = "gtk")), allow(dead_code))]

#[path = "../examples/common/mod.rs"]
mod common;

#[path = "conformance/c0.rs"]
mod c0;
#[path = "conformance/c2.rs"]
mod c2;
#[path = "conformance/caps.rs"]
mod caps;
// Selection column-range conformance — pure
// `EditorSelection::cols_on` matrix, no backend feature required (see
// this module's doc for why it isn't gated behind `tui`/`gtk` like the
// rest of this file).
#[path = "conformance/editor.rs"]
mod editor;
#[path = "conformance/fixtures.rs"]
mod fixtures;
#[path = "conformance/runner.rs"]
mod runner;
#[path = "conformance/schema.rs"]
mod schema;

use std::fs;
use std::path::{Path, PathBuf};

use runner::{
    backend_applies_to, burn_down_legend, render_matrix, run_scenario, verdict, zones_seen,
    BackendReg, Gating, MatrixRow,
};
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

/// The vt100/ANSI-byte-stream TUI observer — same
/// `AppLogic` fixtures and scenario files as `TuiFactory`, but painted
/// through `CrosstermBackend` into a real ANSI byte stream and read back
/// with `vt100` instead of `TestBackend`'s in-memory buffer. Needs `vt100`
/// itself, which only the `terminal` feature pulls in, so this is gated on
/// both features — same as `tests/tui_pty_smoke.rs`.
///
/// Registered under a *different* name (`"tui-vt100"`, not `"tui"`) so the
/// matrix reports the two TUI observers as distinct rows/columns rather
/// than silently overwriting one another — see
/// `runner::backend_applies_to` for why not every scenario runs against
/// this column.
#[cfg(all(feature = "tui", feature = "terminal"))]
struct TuiVtFactory;

#[cfg(all(feature = "tui", feature = "terminal"))]
impl runner::DriverFactory for TuiVtFactory {
    fn make<A: quadraui::AppLogic + 'static>(
        app: A,
        viewport: quadraui::testing::LogicalViewport,
    ) -> Box<dyn runner::DynDriver> {
        use quadraui::testing::ConformanceDriver;
        Box::new(quadraui::tui::vt_testing::TuiVtDriver::new_fixture(
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

// Win-GUI: `feature = "win"` compiles `quadraui::win` with fallbacks for
// real WinAPI/Direct2D calls elsewhere, so `WinDriver::new_fixture` type-checks
// on Linux too. This registration is gated to `target_os = "windows"` because
// the test needs a real Direct2D surface (`HeadlessSurface::new` always errs
// off Windows), so it stays inert except on `windows-latest`.
//
// Registered **blocking, not burn-down**: the windows-latest leg has been
// blocking since #674, and `WinBackend` has no rasterisers left to build
// for this Tier-1 scenario suite. A regression now reds the Windows column
// of every unrelated PR. `c0_paint_smoke`'s column list is separate and
// still keeps a `win` burn-down entry.
#[cfg(all(feature = "win", target_os = "windows"))]
struct WinFactory;

#[cfg(all(feature = "win", target_os = "windows"))]
impl runner::DriverFactory for WinFactory {
    fn make<A: quadraui::AppLogic + 'static>(
        app: A,
        viewport: quadraui::testing::LogicalViewport,
    ) -> Box<dyn runner::DynDriver> {
        use quadraui::testing::ConformanceDriver;
        Box::new(quadraui::win::testing::WinDriver::new_fixture(
            app, viewport,
        ))
    }
}

/// Every backend compiled into this build. **This is the registration
/// point** — a new backend adds exactly one `push` here.
///
/// Note what is *not* here: a capability list. `BackendReg::register`
/// reads it off the backend itself, so a registration
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
    #[cfg(all(feature = "tui", feature = "terminal"))]
    regs.push(BackendReg::register::<TuiVtFactory>("tui-vt100"));
    #[cfg(feature = "gtk")]
    regs.push(BackendReg::register::<GtkFactory>("gtk"));
    #[cfg(all(feature = "macos", target_os = "macos"))]
    regs.push(BackendReg::register::<MacFactory>("macos"));
    #[cfg(all(feature = "win", target_os = "windows"))]
    regs.push(BackendReg::register::<WinFactory>("win"));
    regs
}

/// The names of every [`Gating::BurnDown`] column in this build — what
/// [`verdict`] and [`burn_down_legend`] key off. Empty on every leg
/// today; a newly added backend starts here until its column passes.
fn burn_down_backends(backends: &[BackendReg]) -> std::collections::BTreeSet<&'static str> {
    backends
        .iter()
        .filter(|b| b.gating == Gating::BurnDown)
        .map(|b| b.name)
        .collect()
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
/// and fail if any *gating* cell failed.
///
/// "Gating" is not "all": a [`Gating::BurnDown`] column —
/// today only `win` — has its failures printed in the matrix, in the detail
/// block, and in the CI artifact, but doesn't fail the run. See
/// [`runner::Gating`] for why registering a half-built backend and gating
/// on it are separate decisions, and note that the burn-down state is
/// self-expiring: a burn-down column that stops failing fails *this* test
/// until it is promoted.
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
                // A scenario that hasn't opted into the vt100 observer
                // has no cell for it at all — renders as
                // `-`, not `skip` — rather than paying to build and replay
                // a driver it was never asked to run against.
                .filter(|b| backend_applies_to(b.name, s))
                .map(|b| (b.name, run_scenario(s, b)))
                .collect(),
        })
        .collect();

    let names: Vec<&'static str> = backends.iter().map(|b| b.name).collect();
    let burn_down = burn_down_backends(&backends);
    // The legend goes into the printed table *and* the CI artifact, not
    // just this test's stdout: the artifact is what a reader consults to
    // decide whether a `FAIL` is a regression or a checklist item.
    let table = format!(
        "{}{}",
        render_matrix(&rows, &names),
        burn_down_legend(&burn_down)
    );
    println!("{table}");
    write_artifact(&table, "conformance-matrix.txt");

    let judged = verdict(&rows, &burn_down);
    assert!(
        judged.blocking.is_empty(),
        "{} conformance cell(s) failed:\n{}\n{table}",
        judged.blocking.len(),
        judged.blocking.join("\n")
    );
    assert!(
        judged.promotable.is_empty(),
        "burn-down backend(s) {:?} failed no scenario — promote them from \
         `BackendReg::register_burn_down` to `BackendReg::register` in `backends()` so their \
         column gates again (quadraui#708).\n{table}",
        judged.promotable
    );
}

/// One case's classification on one column: the table cell to print, and
/// — for anything but a clean pass — the `col_name/method: why` line ready
/// to push into whichever gap list `c0_paint_smoke`'s `Gating` split
/// selects.
///
/// Pulled out of `c0_paint_smoke`'s loop body so this
/// tier's primitive-axis gating split has a unit test of its own,
/// mirroring `runner::verdict`'s scenario-axis equivalent — which
/// `runner.rs`'s own test module already covers (e.g.
/// `register_burn_down_marks_the_registration_non_gating`). Before this,
/// the split logic below was only ever exercised by `c0_paint_smoke`
/// itself actually running for real; if the two gating-split
/// implementations ever drift, this is what would catch it.
struct C0CaseVerdict {
    /// `"pass"`, `"FAIL"`, or `"PANIC"`.
    cell: &'static str,
    /// `None` for a clean pass; otherwise the fully-formatted gap line.
    gap_line: Option<String>,
}

fn classify_c0_case(
    outcome: &c0::CaseOutcome,
    method: &str,
    needle: Option<&str>,
    col_name: &str,
) -> C0CaseVerdict {
    if !outcome.survived {
        return C0CaseVerdict {
            cell: "PANIC",
            gap_line: Some(format!("{col_name}/{method}: panicked mid-paint")),
        };
    }
    if !outcome.text_ok {
        return C0CaseVerdict {
            cell: "FAIL",
            gap_line: Some(format!(
                "{col_name}/{method}: handed {needle:?} and the frame does not report it — {}",
                outcome.reported
            )),
        };
    }
    if !outcome.observable {
        return C0CaseVerdict {
            cell: "FAIL",
            gap_line: Some(format!(
                "{col_name}/{method}: reported neither a text run nor a zone — {}",
                outcome.reported
            )),
        };
    }
    C0CaseVerdict {
        cell: "pass",
        gap_line: None,
    }
}

/// Tier 0 — C0 paint smoke. Runs ahead of every scenario-based tier:
/// `c0::CASES` is a canned, minimal descriptor per primitive, and this
/// asserts `begin_frame` → draw → `end_frame` neither panics nor produces
/// a frame the inventory can't see at all.
///
/// A registered backend with nothing to say for a given primitive prints
/// as a row here, same as a tier-1 `FAIL` — never silence. A backend this
/// build doesn't compile in simply has no column. `win` is registered as
/// [`runner::Gating::BurnDown`] rather than blocking; see `docs/TESTING.md`
/// → *Burn-down columns* for why a half-built backend gets a column.
///
/// ## What this tier does *not* yet catch
///
/// `MacBackend` is deliberately *not* added to this tier's `columns`:
/// it lacks a real `ConformanceDriver` and its known fakes would turn
/// straight into failures on macOS, which is its own follow-up.
/// The `draw_diff_view` row is proven on TUI and GTK only. Until the macOS
/// `ConformanceDriver` lands, C0's coverage claim is "every primitive, on
/// every backend wired into this tier", not "on every backend".
// `vec_init_then_push`: each push is behind its own `#[cfg]`, so the
// `vec![…]` form clippy suggests would need the cfg attributes on macro
// arguments — same trade-off `backends()` above already makes.
#[allow(clippy::vec_init_then_push)]
#[test]
fn c0_paint_smoke() {
    struct Column {
        name: &'static str,
        outcomes: Vec<c0::CaseOutcome>,
        /// See [`runner::Gating`] — a `BurnDown` column's `PANIC`/`FAIL`
        /// rows are printed and named exactly like a blocking column's,
        /// but do not fail this test.
        gating: Gating,
    }

    // `mut` is unused when none of the pushes below is compiled in — see
    // the skip arm underneath.
    #[allow(unused_mut)]
    let mut columns: Vec<Column> = Vec::new();
    #[cfg(feature = "tui")]
    columns.push(Column {
        name: "tui",
        outcomes: c0::run::<TuiFactory>(),
        gating: Gating::Blocking,
    });
    #[cfg(feature = "gtk")]
    columns.push(Column {
        name: "gtk",
        outcomes: c0::run::<GtkFactory>(),
        gating: Gating::Blocking,
    });
    // Win-GUI: registered burn-down, not blocking. `WinFactory`/`WinDriver`
    // exist, but `WinBackend`'s painted-text-run recording and `register_zone`
    // are stubs and several `draw_*` methods are still `todo!()`. Gating on
    // that would red windows-latest for every unrelated PR; the row detail
    // below is the burn-down checklist this registration produces.
    #[cfg(all(feature = "win", target_os = "windows"))]
    columns.push(Column {
        name: "win",
        outcomes: c0::run::<WinFactory>(),
        gating: Gating::BurnDown,
    });

    // With a C0 driver compiled in, an empty column set means the
    // registration above broke — still a hard failure, and still the
    // guard against a vacuous pass.
    #[cfg(any(
        feature = "tui",
        feature = "gtk",
        all(feature = "win", target_os = "windows")
    ))]
    assert!(
        !columns.is_empty(),
        "c0_paint_smoke: no backend feature enabled — run with --features tui,gtk (or --features \
         win on a windows-latest host)"
    );

    // Without a C0 driver compiled in, there is no driver to smoke. `--features
    // macos` alone has no `MacBackend` `ConformanceDriver`, so a hard failure
    // would mean reporting on a backend this tier does not cover. The gap
    // itself is not silent: `caps::backends_declare_only_what_they_override`
    // prints the macOS column on every run.
    if columns.is_empty() {
        println!(
            "c0_paint_smoke: SKIPPED — no C0 driver backend in this feature set. \
             Build with --features tui, gtk, and/or win (on Windows) to run tier 0."
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

    // Split by gating exactly as `verdict` does for the Tier-1 matrix
    //: a `BurnDown` column's gaps are printed, named, and
    // folded into the promotable check below, but never fail this test.
    let mut blocking_gaps: Vec<String> = Vec::new();
    let mut burn_down_gaps: Vec<String> = Vec::new();
    // Which burn-down columns failed at least one case. Every case in
    // `CASES` always runs here — there is no capability-skip concept at
    // this tier — so unlike `verdict`'s `ran` set, "did it run" is not in
    // question; only "did it ever fail" is, which is exactly what decides
    // promotability.
    let mut burn_down_failed: std::collections::BTreeSet<&'static str> =
        std::collections::BTreeSet::new();

    for (i, case) in c0::CASES.iter().enumerate() {
        table.push_str(&format!("{:<method_w$}", case.method));
        for col in &columns {
            let outcome = &col.outcomes[i];
            let v = classify_c0_case(outcome, case.method, case.needle, col.name);
            table.push_str(&format!("  {:<6}", v.cell));
            if let Some(line) = v.gap_line {
                if col.gating == Gating::BurnDown {
                    burn_down_failed.insert(col.name);
                    burn_down_gaps.push(line);
                } else {
                    blocking_gaps.push(line);
                }
            }
        }
        table.push('\n');
    }

    let burn_down: std::collections::BTreeSet<&'static str> = columns
        .iter()
        .filter(|c| c.gating == Gating::BurnDown)
        .map(|c| c.name)
        .collect();
    // Same legend `conformance_matrix` prints under its own table — a
    // reader must never mistake a non-gating `FAIL`/`PANIC` row here for a
    // regression.
    table.push_str(&burn_down_legend(&burn_down));
    println!("{table}");
    // Same artifact mechanism `conformance_matrix` uses; the `win` burn-down
    // grid is invisible on a routine green CI run otherwise, but survives in
    // the artifact for CI to upload. See `.github/workflows/ci.yml`'s
    // "Conformance matrix (win)" step.
    write_artifact(&table, "conformance-matrix-c0.txt");

    if !burn_down_gaps.is_empty() {
        println!(
            "{} non-gating C0 gap(s) on burn-down column(s) {:?} — implementation checklist, \
             not a regression, and does not fail this test:\n{}",
            burn_down_gaps.len(),
            burn_down,
            burn_down_gaps.join("\n")
        );
    }

    assert!(
        blocking_gaps.is_empty(),
        "{} C0 paint-smoke gap(s) — a primitive either panicked, dropped its text, or left the \
         frame unobservable, which contract §5b (tests/acceptance/ms-11/contract.md) treats as \
         indistinguishable from the trait's no-op default:\n{}\n{table}",
        blocking_gaps.len(),
        blocking_gaps.join("\n")
    );

    // Self-expiring, same as `verdict`'s `promotable` check:
    // a burn-down column that stops failing is a column nobody has any
    // reason not to gate on, so leaving it non-gating forever would let a
    // green column silently stop protecting anything.
    let promotable: Vec<&'static str> = burn_down.difference(&burn_down_failed).copied().collect();
    assert!(
        promotable.is_empty(),
        "C0 burn-down backend(s) {:?} failed no case — promote them from `Gating::BurnDown` to \
         `Gating::Blocking` in `c0_paint_smoke` so their column gates again (quadraui#708).\n{table}",
        promotable
    );
}

/// Tier C2 — event-emission conformance. See `c2.rs`'s module doc for what
/// this proves and why it's distinct from C0 (paint) and C1 (behaviour).
/// Covers mouse/key/scroll/resize core plus `WindowClose` on windowed
/// backends (GTK, Win). The `win` column here runs on `ubuntu-latest
/// --features win`, unlike the C0/C1 `WinFactory` which stays `target_os =
/// "windows"`-gated because it drives a live `Backend`.
///
/// `DoubleClick`/`Accelerator`/`ClipboardPaste` have declared `c2::DISPATCH_ROWS`
/// entries on every column, printing `pass*` with a footnote naming the
/// `dispatch_event` hook that produces them. Promoting them to real assertions
/// needs a dispatch-level fixture. TUI's `clipboard_paste` asserts for real.
/// `TextCopied` is not required and has no row; `docs/BACKEND.md` is the
/// source of truth for production emission status.
#[test]
// See the `columns` builder below for why this can't be a `vec![…]`.
#[allow(clippy::vec_init_then_push)]
fn c2_event_parity() {
    struct Column {
        name: &'static str,
        rows: Vec<&'static str>,
        case: fn(&str) -> c2::CaseOutcome,
    }

    // Every push below is `cfg`-gated, so a single-backend feature set —
    // `--features win`, exactly what the ubuntu win leg builds — leaves
    // one push, making the `mut` look redundant and the builder look like
    // it could be a `vec![…]` literal (hence this fn's
    // `allow(clippy::vec_init_then_push)`). It can't: which columns exist
    // is decided by feature flags at compile time, not by this expression.
    #[allow(unused_mut)]
    let mut columns: Vec<Column> = Vec::new();
    #[cfg(feature = "tui")]
    columns.push(Column {
        name: "tui",
        rows: c2::CORE_ROWS
            .iter()
            .chain(c2::DISPATCH_ROWS)
            .copied()
            .collect(),
        case: c2::tui_case,
    });
    #[cfg(feature = "gtk")]
    columns.push(Column {
        name: "gtk",
        rows: c2::CORE_ROWS
            .iter()
            .chain(c2::WINDOWED_ROWS)
            .chain(c2::DISPATCH_ROWS)
            .copied()
            .collect(),
        case: c2::gtk_case,
    });
    // Not `target_os = "windows"`-gated — see this test's doc and
    // `c2.rs`'s module doc for why `win_case` is host-independent.
    #[cfg(feature = "win")]
    columns.push(Column {
        name: "win",
        rows: c2::CORE_ROWS
            .iter()
            .chain(c2::WINDOWED_ROWS)
            .chain(c2::DISPATCH_ROWS)
            .copied()
            .collect(),
        case: c2::win_case,
    });

    #[cfg(any(feature = "tui", feature = "gtk", feature = "win"))]
    assert!(
        !columns.is_empty(),
        "c2_event_parity: no backend feature enabled — run with --features tui,gtk,win"
    );

    if columns.is_empty() {
        println!(
            "c2_event_parity: SKIPPED — no C2 driver backend in this feature set. \
             Build with --features tui, gtk, and/or win to run tier 2."
        );
        return;
    }

    // Every row that at least one column declares, in a stable order:
    // `CORE_ROWS` first (shared), then `WINDOWED_ROWS`, then
    // `DISPATCH_ROWS`. A column that doesn't declare a row prints `n/a`
    // for it rather than a fabricated pass/fail — `WindowClose` genuinely
    // does not apply to TUI (D-010). Note `n/a` means *inapplicable*, not
    // *unmeasured*: an event that applies but can't be asserted at this
    // tier is a declared `pass*` placeholder row instead, which is why
    // `DISPATCH_ROWS` is declared by every column.
    let mut all_rows: Vec<&'static str> = c2::CORE_ROWS.to_vec();
    all_rows.extend(c2::WINDOWED_ROWS);
    all_rows.extend(c2::DISPATCH_ROWS);

    let row_w = all_rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut table = format!("{:<row_w$}", "event (tier 2)");
    for col in &columns {
        table.push_str(&format!("  {:<6}", col.name));
    }
    table.push('\n');

    let mut gaps: Vec<String> = Vec::new();
    // Placeholder rows always report `pass` but aren't backed by real assertions;
    // collected here so the table can foot-note them rather than read as
    // identical to verified passes.
    let mut placeholders: Vec<String> = Vec::new();
    for row in &all_rows {
        table.push_str(&format!("{row:<row_w$}"));
        for col in &columns {
            if !col.rows.contains(row) {
                table.push_str(&format!("  {:<6}", "n/a"));
                continue;
            }
            let outcome = (col.case)(row);
            let cell = if !outcome.pass {
                "FAIL"
            } else if outcome.placeholder {
                "pass*"
            } else {
                "pass"
            };
            table.push_str(&format!("  {cell:<6}"));
            if !outcome.pass {
                gaps.push(format!("{}/{row}: {}", col.name, outcome.detail));
            } else if outcome.placeholder {
                placeholders.push(format!("{}/{row}: {}", col.name, outcome.detail));
            }
        }
        table.push('\n');
    }
    if !placeholders.is_empty() {
        table.push_str("* placeholder — not a live assertion at this tier:\n");
        for p in &placeholders {
            table.push_str(&format!("  {p}\n"));
        }
    }
    println!("{table}");

    assert!(
        gaps.is_empty(),
        "{} C2 event-parity gap(s) — a backend's native→UiEvent translation for a required \
         event didn't produce the expected shape:\n{}\n{table}",
        gaps.len(),
        gaps.join("\n")
    );
}

/// Drop a matrix where CI can pick it up as an artifact. Best-effort:
/// an unwritable target directory must not fail the suite.
///
/// Honours `CARGO_TARGET_DIR` so a shared/relocated target directory
/// still gets the file; otherwise falls back to the workspace's own
/// `target/` (which `.gitignore` already covers).
///
/// Takes an explicit `filename` rather than a single
/// hardcoded `conformance-matrix.txt`: `conformance_matrix` (Tier-1,
/// scenario × backend) and `c0_paint_smoke` (Tier-0, primitive × backend)
/// both call this, and `cargo test` runs test functions concurrently by
/// default — two tests racing to write the *same* path would make whichever
/// finished last silently clobber the other's artifact, so each tier gets
/// its own file instead of sharing one.
fn write_artifact(table: &str, filename: &str) {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../target"));
    let path = target.join(filename);
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
/// exactly as it does for a needle outside one, so a step naming a zone
/// no backend ever registers — a typo or premature assertion — reads as
/// an ordinary layout failure. This test names the cause directly.
/// Zone registration is per paint site (shell chrome regions and activity-bar
/// items), so a scenario reaching for an unregistered primitive fails here.
///
/// Burn-down columns are exempt: a backend with no shell chrome registers
/// *no* zones, so every `assert_inside` step would fail here — true of the
/// backend, not the scenario. Their gap stays visible as the `FAIL` row.
#[test]
fn every_asserted_zone_is_registered_by_every_backend() {
    let backends = backends();
    let burn_down = burn_down_backends(&backends);
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
            // Same opt-in filter `conformance_matrix` applies — a scenario
            // that never runs against `tui-vt100` has nothing to check
            // there either.
            if !backend_applies_to(backend.name, &scenario) {
                continue;
            }
            // Non-gating column — see this test's doc comment.
            if burn_down.contains(backend.name) {
                continue;
            }
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
/// There is one vocabulary now. `BackendCaps::has` answers `false` for an
/// unknown name so a real capability gap reads as a skip, which means a
/// typo must be caught here or not at all.
///
/// Runs with no backend feature enabled — the vocabulary is a property
/// of the library, not of whichever backends this build links.
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
            "window_control",
            "every `WindowControl` method (issue #950; `is_maximized`/`set_decorated` added by \
             #1022) acts on a real OS toplevel — title, size, position, fullscreen, \
             maximized-state, decoration, minimize/restore/hide/show/focus — the same \
             `GtkDriver`-renders-to-an-offscreen-`ImageSurface`-with-no-window-to-drive gap \
             `window_chrome` above already names, and nothing paints, so `FrameInventory` has \
             no observable to assert on even where a window did exist",
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
            "folder_dialogs",
            "same as `file_dialogs` — a modal native directory chooser cannot run headless \
             (quadraui#935)",
        ),
        (
            "native_dialogs",
            "same as `file_dialogs` — a modal native alert cannot run headless; \
             `GtkDriver` sees Cairo paint, not native windows (quadraui#666), so its \
             visibility has no automated coverage here, only the smoke item that gap \
             calls out",
        ),
        (
            "notifications",
            "fire-and-forget to a system daemon — nothing paints, so no assertion in this \
             suite's vocabulary can observe it",
        ),
        (
            "app_font_registration",
            "declared by GTK (which already had a working fallback before #929 and overrides \
             `set_nerd_font_fallback` for portability) plus macOS/Win-GUI, neither of which has \
             a `ConformanceDriver` (#493) — and even GTK's own coverage would need per-glyph \
             font-resolution inspection `FrameInventory` doesn't do (it records painted text \
             runs, not which family resolved each character), so there is no headless \
             assertion this suite's vocabulary can gate on",
        ),
        (
            "generic_font_families",
            "declared by GTK, macOS and Win-GUI (issue #1023) — GTK has a `ConformanceDriver` \
             but the same gap `app_font_registration` above already names blocks it here too: \
             telling \"resolved the generic token to the right native font\" apart from \
             \"resolved it to something, or fell back to the same default it always used\" \
             needs per-glyph font-resolution inspection `FrameInventory` doesn't do (it records \
             painted text runs, not which family resolved each character). macOS/Win-GUI have \
             no `ConformanceDriver` at all (#493), same as `app_font_registration`",
        ),
        (
            "tray",
            "only macOS declares it (issue #953), and macOS has no `ConformanceDriver` yet \
             (#493) — same gap `native_menu` above already names. Even with one, a tray icon \
             lives outside any window `FrameInventory` observes (the OS menu bar / \
             notification area, not the app's own render surface), so there is nothing this \
             suite's paint-and-click vocabulary could assert on even then",
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

/// Unit coverage for `classify_c0_case` — the
/// primitive-axis gating split `c0_paint_smoke` applies to `c0::CaseOutcome`.
/// Direct calls rather than a full `c0_paint_smoke` run: no backend driver
/// needed, and each case below pins one branch of the split so a future
/// edit can't silently change which failures land in `blocking_gaps` vs.
/// `burn_down_gaps` without a red test naming exactly which branch moved.
#[cfg(test)]
mod c0_verdict_tests {
    use super::{c0, classify_c0_case};

    fn outcome(survived: bool, text_ok: bool, observable: bool) -> c0::CaseOutcome {
        c0::CaseOutcome {
            survived,
            text_ok,
            observable,
            reported: "reported: <nothing>".to_string(),
        }
    }

    #[test]
    fn survived_text_ok_observable_is_a_clean_pass() {
        let v = classify_c0_case(&outcome(true, true, true), "draw_panel", None, "win");
        assert_eq!(v.cell, "pass");
        assert!(v.gap_line.is_none());
    }

    #[test]
    fn a_panic_is_named_by_column_and_method_regardless_of_the_rest() {
        // `survived: false` must win even when the other two flags claim
        // success — a panic mid-paint means those flags describe a frame
        // that was never actually produced.
        let v = classify_c0_case(&outcome(false, true, true), "draw_editor", None, "win");
        assert_eq!(v.cell, "PANIC");
        assert_eq!(
            v.gap_line.as_deref(),
            Some("win/draw_editor: panicked mid-paint")
        );
    }

    #[test]
    fn a_missing_needle_fails_and_names_what_was_handed() {
        let v = classify_c0_case(
            &outcome(true, false, true),
            "draw_status_bar",
            Some("hi"),
            "win",
        );
        assert_eq!(v.cell, "FAIL");
        let line = v.gap_line.expect("text_ok=false must produce a gap line");
        assert!(line.starts_with("win/draw_status_bar: "));
        assert!(line.contains("Some(\"hi\")"));
    }

    #[test]
    fn an_unobservable_frame_fails_even_with_the_right_text() {
        let v = classify_c0_case(&outcome(true, true, false), "draw_toolbar", None, "win");
        assert_eq!(v.cell, "FAIL");
        assert_eq!(
            v.gap_line.as_deref(),
            Some("win/draw_toolbar: reported neither a text run nor a zone — reported: <nothing>")
        );
    }
}
