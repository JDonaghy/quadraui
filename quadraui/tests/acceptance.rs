//! Sealed acceptance entrypoint for the oracle loop (quadraui#556).
//!
//! Modelled on `claude-coordinator`'s `tui/tests/acceptance.rs`. This is
//! the file `acceptance.drivers.quadraui` in the fleet config (`kind:
//! tui-tuidriver`) actually runs:
//!
//! ```sh
//! RUSTC_BOOTSTRAP=1 cargo test --test acceptance --features tui,gtk \
//!     -- -Z unstable-options --format json
//! ```
//!
//! Two things live in this file, deliberately kept apart:
//!
//! 1. **The seam** (above the `SEALED` marker below) — fixture imports plus
//!    a test per backend that proves this *external* integration-test
//!    crate can build a driver from an example `AppLogic` and run it, with
//!    no in-crate `#[cfg(test)]` access. Workers may extend this part
//!    (e.g. adding a new `#[path]` include for a fixture a slice needs).
//! 2. **The sealed block** (below the marker) — `include!`s for each
//!    milestone's independently-authored acceptance slices
//!    (`tests/acceptance/<ms>/<name>.rs`, repo-root-relative). Workers may
//!    NOT edit anything below the marker — see CLAUDE.md.
//!
//! No `test-support` feature is needed here (unlike coord-tui, which
//! needed one because its fixtures were `#[cfg(test)]`-private). quadraui's
//! example `AppLogic` fixtures are already reachable from any integration
//! test via the canonical `#[path = "../examples/common/<name>.rs"]`
//! include the two existing example-driver suites
//! (`tests/tui_example_driver.rs`, `tests/gtk_example_driver.rs`) already
//! use — this file pulls in the whole `examples/common` module the same
//! way `examples/*.rs` do (`#[path = "common/mod.rs"] mod common;`), so an
//! independently-authored slice can name any fixture (`common::<module>::
//! <Type>`) without editing this file's imports.
//!
//! Feature-gating is per backend, not on the file: this module itself
//! carries no `#![cfg(feature = "...")]`, so `cargo check --test
//! acceptance` (no features) still type-checks the doc comments and module
//! wiring. Each backend-specific item — the fixture-driving use, the seam
//! test, and (later) any GTK-only slice — carries its own `#[cfg(feature =
//! "tui")]` / `#[cfg(feature = "gtk")]`, so a TUI-only slice never forces a
//! GTK build.

#[path = "../examples/common/mod.rs"]
mod common;

#[cfg(feature = "tui")]
use quadraui::tui::testing::TuiDriver;

#[cfg(feature = "gtk")]
use quadraui::gtk::testing::GtkDriver;

// ─── Seam tests ──────────────────────────────────────────────────────────
//
// Each proves the same thing for its backend: an example `AppLogic`
// fixture, pulled in only via the `#[path]` include above, drives a real
// driver end to end (setup → paint → event → re-paint) from *this* crate
// — i.e. the oracle's `tui-tuidriver` acceptance kind has a reachable,
// buildable, runnable target for quadraui. If either of these fails to
// compile or run, the sealed suite below has no foundation to build on.

#[cfg(feature = "tui")]
#[test]
fn seam_tui_driver_builds_and_runs_an_example_app_logic() {
    let mut driver = TuiDriver::new(common::mini_app::MiniApp::new(), 40, 10);
    assert!(
        driver.screen_contains("quadraui::run demo"),
        "seam: MiniApp's title segment should paint:\n{}",
        driver.screen()
    );
    assert!(
        driver.screen_contains("keys: 0"),
        "seam: key counter should start at 0:\n{}",
        driver.screen()
    );

    driver.type_char('a');

    assert!(
        driver.screen_contains("keys: 1"),
        "seam: key counter should bump after a real event round-trip:\n{}",
        driver.screen()
    );
}

#[cfg(feature = "gtk")]
#[test]
fn seam_gtk_driver_builds_and_runs_an_example_app_logic() {
    let mut driver = GtkDriver::new(common::mini_app::MiniApp::new(), 320, 60);
    assert!(
        driver.screen_contains("quadraui::run demo"),
        "seam: MiniApp's title segment should paint"
    );
    assert!(
        driver.screen_contains("keys: 0"),
        "seam: key counter should start at 0"
    );

    driver.type_char('a');

    assert!(
        driver.screen_contains("keys: 1"),
        "seam: key counter should bump after a real event round-trip"
    );
}

// ============================================================================
// SEALED — oracle-authored acceptance slices only. Workers may not add,
// remove, or edit `include!`s below this line, and may not edit anything
// under `tests/acceptance/**` at the repo root (see CLAUDE.md). Slices are
// authored independently per milestone and pulled in here, exactly
// mirroring coord-tui's `tui/tests/acceptance.rs` shape. Paths are
// relative to this file's directory (`quadraui/tests/`), so the repo root
// is `../../`:
//
//   include!("../../tests/acceptance/ms-11/<name>.rs");
//
// No slices exist yet — this issue (#556) only builds the road; ms-11
// Gate A (epic #480) authors the first ones. Until a slice lands here,
// `cargo test --test acceptance --features tui,gtk` exercises only the
// seam tests above.
// ============================================================================

// ms-11 (epic #480) — issue #554: tab labels measured and painted in
// display columns, not chars. Contract: tests/acceptance/ms-11/contract.md §3.
include!("../../tests/acceptance/ms-11/wide_tab_labels.rs");

// ms-11 (epic #480) — issue #542: structural parity tier, so a backend that
// silently drops chrome (borders, titles, scrollbars) fails instead of
// passing on text presence alone. Contract: tests/acceptance/ms-11/contract.md §4.
include!("../../tests/acceptance/ms-11/structural_parity.rs");
