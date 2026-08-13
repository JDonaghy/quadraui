//! Conformance scenario **runner** (quadraui#491, audit §6.4/§6.6).
//!
//! Replays a [`Scenario`]'s steps against one backend driver and reports a
//! [`Outcome`]. The top-level `tests/conformance.rs` crosses every scenario
//! file with every registered [`BackendReg`] and prints the resulting
//! scenario × backend matrix.
//!
//! Three moving parts, kept small on purpose:
//!
//! - [`DynDriver`] — an object-safe view of
//!   [`quadraui::testing::ConformanceDriver`] (which is `Sized`, so it can't
//!   be a trait object itself). One blanket impl covers every present and
//!   future backend driver; the runner only ever holds `Box<dyn DynDriver>`.
//! - [`DriverFactory`] — the *one driver impl* a new backend writes. It
//!   turns any `AppLogic` fixture plus a [`LogicalViewport`] into a boxed
//!   driver, which is what lets the string-keyed fixture registry stay
//!   backend-agnostic.
//! - [`BackendReg`] — the *one registration line* a new backend adds,
//!   pairing a display name and a declared capability set with
//!   `fixtures::build::<TheirFactory>`.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use quadraui::testing::{Anchor, ConformanceDriver, FrameInventory, LogicalViewport};
use quadraui::{AppLogic, Backend, BackendCaps, Reaction, UiEvent, WidgetId};

use super::schema::{parse_named_key, Scenario, Step};

// ─── Object-safe driver view ────────────────────────────────────────────

/// Object-safe mirror of [`ConformanceDriver`]'s act/observe surface.
///
/// `ConformanceDriver` is `Sized` (its `new_fixture` returns `Self`), so it
/// cannot be used as `dyn ConformanceDriver`. The runner needs a trait
/// object because a scenario's fixture — and therefore the driver's `App`
/// type parameter — is chosen at runtime from a string. Construction stays
/// on the concrete side ([`DriverFactory`]); everything after it goes
/// through this trait.
pub trait DynDriver {
    fn press_named(&mut self, key: quadraui::NamedKey);
    fn type_char(&mut self, c: char);
    fn type_text(&mut self, s: &str);
    fn ctrl_char(&mut self, c: char);
    fn click_text_at(&mut self, needle: &str, at: Anchor);
    fn drag_text(&mut self, from: &str, to: &str);
    fn scroll_at(&mut self, needle: &str, lines: i32);
    fn inventory(&self) -> FrameInventory;
    fn backend_caps(&self) -> BackendCaps;
    fn screen_has(&self, needle: &str) -> bool;
    fn exited(&self) -> bool;
}

impl<D: ConformanceDriver> DynDriver for D {
    fn press_named(&mut self, key: quadraui::NamedKey) {
        ConformanceDriver::press_named(self, key)
    }
    fn type_char(&mut self, c: char) {
        ConformanceDriver::type_char(self, c)
    }
    fn type_text(&mut self, s: &str) {
        ConformanceDriver::type_text(self, s)
    }
    fn ctrl_char(&mut self, c: char) {
        ConformanceDriver::ctrl_char(self, c)
    }
    fn click_text_at(&mut self, needle: &str, at: Anchor) {
        ConformanceDriver::click_text_at(self, needle, at)
    }
    fn drag_text(&mut self, from: &str, to: &str) {
        ConformanceDriver::drag_text(self, from, to)
    }
    fn scroll_at(&mut self, needle: &str, lines: i32) {
        ConformanceDriver::scroll_at(self, needle, lines)
    }
    fn inventory(&self) -> FrameInventory {
        ConformanceDriver::inventory(self)
    }
    fn backend_caps(&self) -> BackendCaps {
        ConformanceDriver::backend_caps(self)
    }
    fn screen_has(&self, needle: &str) -> bool {
        ConformanceDriver::screen_has(self, needle)
    }
    fn exited(&self) -> bool {
        ConformanceDriver::exited(self)
    }
}

/// The single per-backend adapter a new backend author writes.
///
/// Deliberately an *associated* function with no `self`: that makes
/// `fixtures::build::<MyFactory>` a plain `fn` pointer, which is what
/// [`BackendReg`] stores, so registering a backend is one line rather than
/// one line per fixture.
pub trait DriverFactory {
    fn make<A: AppLogic + 'static>(app: A, viewport: LogicalViewport) -> Box<dyn DynDriver>;

    /// What this backend declares it can do — read off a real instance of
    /// the backend, via [`DynDriver::backend_caps`].
    ///
    /// **Do not override this.** The default body is the whole point
    /// (quadraui#492 review): it builds a throwaway driver over a
    /// paint-nothing [`AppLogic`] and asks the backend itself, so
    /// registering a backend cannot restate — and therefore cannot
    /// contradict — its own `Backend::backend_caps`. Overriding it would
    /// reintroduce exactly the hand-maintained second list
    /// (`TUI_CAPS`/`GTK_CAPS`) this replaced. Provided as a defaulted
    /// trait method rather than a free function only so the "add a
    /// backend = one `DriverFactory` impl + one `push`" contract holds
    /// with no extra line at either site.
    fn caps() -> BackendCaps {
        /// Renders nothing and handles nothing: capabilities are a
        /// property of the *backend*, not of whatever app is on top of
        /// it, so the cheapest possible app is the honest probe.
        struct CapsProbe;
        impl AppLogic for CapsProbe {
            type AreaId = ();
            fn render(&self, _backend: &mut dyn Backend, _area: ()) {}
            fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
                Reaction::Continue
            }
        }
        // Small on purpose — nothing is painted, so the viewport only has
        // to be large enough for the backend to construct a surface.
        Self::make(CapsProbe, LogicalViewport::new(10, 5)).backend_caps()
    }
}

/// Builds a driver for a named fixture, or `None` if this build has no such
/// fixture. Always `fixtures::build::<F>` for some [`DriverFactory`] `F`.
pub type BuildFn = fn(&str, LogicalViewport) -> Option<Box<dyn DynDriver>>;

/// One registered backend: display name, declared capabilities, builder.
pub struct BackendReg {
    pub name: &'static str,
    /// Capabilities this backend declares it supports. A scenario whose
    /// `requires` list mentions anything absent here is **skipped with the
    /// missing capability named** — never silently passed (audit §6.6).
    ///
    /// This is [`quadraui::Backend::backend_caps`]'s own return value,
    /// obtained through [`DriverFactory::caps`] — *not* a list restated
    /// beside the registration (quadraui#492 review). One vocabulary,
    /// one source.
    pub caps: BackendCaps,
    pub build: BuildFn,
}

impl BackendReg {
    /// Register backend `name` from its [`DriverFactory`] alone.
    ///
    /// Generic over `F` rather than taking `caps` and `build` as separate
    /// arguments so the three cannot disagree: the name is the only thing
    /// the caller supplies, and both the capability set and the fixture
    /// builder are derived from the same `F`. This is the whole of "add a
    /// backend = one registration line".
    pub fn register<F: DriverFactory>(name: &'static str) -> Self {
        Self {
            name,
            caps: F::caps(),
            build: super::fixtures::build::<F>,
        }
    }

    /// The first capability in `requires` this backend has not declared.
    fn missing_cap(&self, requires: &[String]) -> Option<String> {
        requires.iter().find(|r| !self.caps.has(r)).cloned()
    }
}

// ─── Outcomes ───────────────────────────────────────────────────────────

/// Result of one (scenario, backend) cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    /// Skipped because the backend did not declare a required capability.
    /// The reason is mandatory — an unexplained skip is not expressible.
    Skip {
        missing_cap: String,
    },
    Fail {
        step: usize,
        reason: String,
    },
}

impl Outcome {
    pub fn symbol(&self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Skip { .. } => "skip",
            Outcome::Fail { .. } => "FAIL",
        }
    }
}

/// One row of the matrix: a scenario and its outcome per backend.
pub struct MatrixRow {
    pub id: String,
    pub tier: u8,
    pub cells: Vec<(&'static str, Outcome)>,
}

// ─── Execution ──────────────────────────────────────────────────────────

/// Build a driver for `scenario` on `backend` and replay every step.
///
/// Returns [`Outcome::Skip`] before constructing anything if the backend
/// lacks a required capability, so a capability gate never pays for a
/// fixture build.
pub fn run_scenario(scenario: &Scenario, backend: &BackendReg) -> Outcome {
    if let Some(missing_cap) = backend.missing_cap(&scenario.requires) {
        return Outcome::Skip { missing_cap };
    }
    let Some(mut driver) = (backend.build)(&scenario.fixture, scenario.viewport.into()) else {
        return Outcome::Fail {
            step: 0,
            reason: format!(
                "unknown fixture {:?} — add it to tests/conformance/fixtures.rs",
                scenario.fixture
            ),
        };
    };
    for (i, step) in scenario.steps.iter().enumerate() {
        if let Err(reason) = run_step(driver.as_mut(), step) {
            return Outcome::Fail { step: i, reason };
        }
    }
    Outcome::Pass
}

/// Every zone id `backend` registers at any point while replaying
/// `scenario` — the first frame plus one observation after each step.
///
/// `None` when the backend skips the scenario (missing capability) or has
/// no such fixture; those cells have nothing to say about zones.
///
/// Step failures are deliberately ignored here: this exists to answer the
/// single question "does this zone id ever get registered?", which is the
/// difference between a step that is *failing* and a step that is
/// *unsatisfiable*. `conformance_matrix` still reports the failure itself.
pub fn zones_seen(scenario: &Scenario, backend: &BackendReg) -> Option<BTreeSet<String>> {
    if backend.missing_cap(&scenario.requires).is_some() {
        return None;
    }
    let mut driver = (backend.build)(&scenario.fixture, scenario.viewport.into())?;
    let mut seen = BTreeSet::new();
    fn observe(d: &dyn DynDriver, seen: &mut BTreeSet<String>) {
        for z in d.inventory().zones() {
            seen.insert(z.id.as_str().to_string());
        }
    }
    observe(driver.as_ref(), &mut seen);
    for step in &scenario.steps {
        let _ = run_step(driver.as_mut(), step);
        observe(driver.as_ref(), &mut seen);
    }
    Some(seen)
}

/// Execute one step. `Err(reason)` is a scenario failure, not a panic —
/// act steps pre-check that their target is painted so a missing needle
/// reports "not painted" with the frame's runs rather than unwinding out
/// of the driver.
fn run_step(d: &mut dyn DynDriver, step: &Step) -> Result<(), String> {
    match step {
        Step::Note(_) => {}

        Step::Press(name) => {
            let key = parse_named_key(name)?;
            d.press_named(key);
        }
        Step::TypeChar(c) => d.type_char(*c),
        Step::TypeText(s) => d.type_text(s),
        Step::CtrlChar(c) => d.ctrl_char(*c),

        Step::ClickText(text) => {
            require_painted(d, text)?;
            d.click_text_at(text, Anchor::Center);
        }
        Step::ClickTextAt { text, anchor } => {
            require_painted(d, text)?;
            d.click_text_at(text, (*anchor).into());
        }
        Step::DragText { from, to } => {
            require_painted(d, from)?;
            require_painted(d, to)?;
            d.drag_text(from, to);
        }
        Step::ScrollAt { target, lines } => {
            require_painted(d, target)?;
            d.scroll_at(target, *lines);
        }

        Step::AssertScreenHas(text) => {
            if !d.screen_has(text) {
                return Err(format!("expected {text:?} on screen; {}", painted(d)));
            }
        }
        Step::AssertAbsent(text) => {
            if d.screen_has(text) {
                return Err(format!("expected {text:?} to be absent, but it is painted"));
            }
        }
        Step::AssertCount { text, count } => {
            let got = d.inventory().count(text);
            if got != *count {
                return Err(format!(
                    "expected {count} painted run(s) containing {text:?}, found {got}"
                ));
            }
        }
        Step::AssertLeftOf { a, b } => {
            let inv = d.inventory();
            if !inv.left_of(a, b) {
                return Err(format!(
                    "expected {a:?} entirely left of {b:?} ({}); {}",
                    relation_hint(&inv, a, b),
                    painted(d)
                ));
            }
        }
        Step::AssertAbove { a, b } => {
            let inv = d.inventory();
            if !inv.above(a, b) {
                return Err(format!(
                    "expected {a:?} entirely above {b:?} ({}); {}",
                    relation_hint(&inv, a, b),
                    painted(d)
                ));
            }
        }
        Step::AssertInside { a, zone } => {
            let inv = d.inventory();
            if !inv.inside(a, &WidgetId::new(zone.clone())) {
                return Err(inside_failure(&inv, a, zone));
            }
        }
        Step::AssertExited(want) => {
            let got = d.exited();
            if got != *want {
                return Err(format!("expected exited={want}, got exited={got}"));
            }
        }
    }
    Ok(())
}

/// Guard for act steps: the driver's `click_text`/`drag_text`/`scroll_at`
/// panic when a needle was never painted (that's the right behaviour for a
/// hand-written Rust body). A declarative scenario wants a diagnosis
/// instead, so check first and turn the miss into a `Fail` row.
fn require_painted(d: &dyn DynDriver, needle: &str) -> Result<(), String> {
    if d.screen_has(needle) {
        Ok(())
    } else {
        Err(format!(
            "cannot act on {needle:?}: not painted; {}",
            painted(d)
        ))
    }
}

/// Which of the two needles a relational assertion actually found — the
/// usual cause of a `left_of`/`above` failure is a typo'd needle, not a
/// layout regression, and the vocabulary answers `false` for both.
fn relation_hint(inv: &FrameInventory, a: &str, b: &str) -> String {
    match (inv.screen_has(a), inv.screen_has(b)) {
        (true, true) => "both painted, but the relation does not hold".into(),
        (false, true) => format!("{a:?} was never painted"),
        (true, false) => format!("{b:?} was never painted"),
        (false, false) => "neither was painted".into(),
    }
}

/// Why an `assert_inside` failed. The three causes need three different
/// fixes, and conflating them is what makes a zone-backed step look like
/// a layout bug when it is really a missing `Backend::register_zone` call
/// (or vice versa), so each is reported in its own words.
fn inside_failure(inv: &FrameInventory, needle: &str, zone: &str) -> String {
    let mut ids: Vec<&str> = inv.zones().iter().map(|z| z.id.as_str()).collect();
    ids.sort_unstable();
    let run_bounds = inv
        .text_runs()
        .iter()
        .find(|r| r.text.contains(needle))
        .map(|r| r.bounds);
    let zone_bounds = inv
        .zones()
        .iter()
        .find(|z| z.id.as_str() == zone)
        .map(|z| z.bounds);
    match (run_bounds, zone_bounds) {
        (_, None) => format!(
            "zone {zone:?} was never registered this frame, so this step can never pass on \
             this backend — nothing called `Backend::register_zone` for it. Either wire the \
             registration at the paint site (see `AppShell::register_chrome_zones`) or drop \
             the step and record the gap under `docs/TESTING.md` → *Known coordinate-free \
             gaps*. Registered zones: {ids:?}"
        ),
        (None, Some(_)) => format!(
            "zone {zone:?} is registered, but {needle:?} was never painted; {}",
            painted_runs(inv)
        ),
        (Some(r), Some(z)) => format!(
            "{needle:?} is painted at {r:?}, which is not contained by zone {zone:?} at {z:?}"
        ),
    }
}

/// A compact, deduplicated dump of what *was* painted, for failure text.
/// Sorted and capped so a 40-row frame doesn't drown the report.
fn painted(d: &dyn DynDriver) -> String {
    painted_runs(&d.inventory())
}

fn painted_runs(inv: &FrameInventory) -> String {
    let runs: BTreeSet<String> = inv
        .text_runs()
        .iter()
        .map(|r| r.text.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let shown: Vec<&str> = runs.iter().map(|s| s.as_str()).take(40).collect();
    let more = runs.len().saturating_sub(shown.len());
    if more > 0 {
        format!("painted runs (first 40 of {}): {shown:?}", runs.len())
    } else {
        format!("painted runs: {shown:?}")
    }
}

// ─── Matrix report ──────────────────────────────────────────────────────

/// Render the scenario × backend matrix the audit (§6.6) calls for: one
/// row per scenario, one column per registered backend, `pass` / `skip` /
/// `FAIL` per cell, followed by the detail for every non-pass cell.
///
/// CI uploads this as an artifact; for a new backend it *is* the
/// implementation checklist.
pub fn render_matrix(rows: &[MatrixRow], backends: &[&'static str]) -> String {
    let id_w = rows
        .iter()
        .map(|r| r.id.len())
        .chain(std::iter::once("scenario".len()))
        .max()
        .unwrap_or(8);
    let col_w = |name: &str| name.len().max("FAIL".len());

    let mut out = String::new();
    out.push_str("\nConformance matrix (scenario × backend)\n");

    let mut header = format!("{:<id_w$}  tier", "scenario", id_w = id_w);
    for b in backends {
        let _ = write!(header, "  {:<w$}", b, w = col_w(b));
    }
    out.push_str(&header);
    out.push('\n');
    out.push_str(&"-".repeat(header.len()));
    out.push('\n');

    for row in rows {
        let mut line = format!("{:<id_w$}  {:>4}", row.id, row.tier, id_w = id_w);
        for b in backends {
            let cell = row
                .cells
                .iter()
                .find(|(n, _)| n == b)
                .map(|(_, o)| o.symbol())
                .unwrap_or("-");
            let _ = write!(line, "  {:<w$}", cell, w = col_w(b));
        }
        out.push_str(&line);
        out.push('\n');
    }

    let mut details = String::new();
    for row in rows {
        for (backend, outcome) in &row.cells {
            match outcome {
                Outcome::Pass => {}
                Outcome::Skip { missing_cap } => {
                    let _ = writeln!(
                        details,
                        "  skip {}/{}: backend does not declare capability {:?}",
                        row.id, backend, missing_cap
                    );
                }
                Outcome::Fail { step, reason } => {
                    let _ = writeln!(
                        details,
                        "  FAIL {}/{} at step {}: {}",
                        row.id, backend, step, reason
                    );
                }
            }
        }
    }
    if !details.is_empty() {
        out.push_str("\nDetail:\n");
        out.push_str(&details);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::Rect;

    const NO_CAPS: BackendCaps = BackendCaps::empty();
    const SOME_CAPS: BackendCaps = BackendCaps {
        text_selection: true,
        ..BackendCaps::empty()
    };

    fn never_builds(_: &str, _: LogicalViewport) -> Option<Box<dyn DynDriver>> {
        None
    }

    /// A `BackendReg` built by hand rather than through
    /// [`BackendReg::register`]: these tests are about `missing_cap`'s
    /// gate, so they need to pin an arbitrary capability set without
    /// standing up a backend to declare it.
    fn stub(caps: BackendCaps) -> BackendReg {
        BackendReg {
            name: "stub",
            caps,
            build: never_builds,
        }
    }

    fn scenario_requiring(caps: &[&str]) -> Scenario {
        Scenario {
            id: "x.y".into(),
            fixture: "nope".into(),
            tier: 1,
            viewport: super::super::schema::ViewportSpec { cols: 10, rows: 10 },
            requires: caps.iter().map(|s| s.to_string()).collect(),
            steps: vec![],
        }
    }

    fn inv(runs: &[(&str, Rect)], zones: &[(&str, Rect)]) -> FrameInventory {
        use quadraui::testing::{TextRun, ZoneRec};
        FrameInventory {
            text_runs: runs
                .iter()
                .map(|(text, bounds)| TextRun {
                    text: (*text).into(),
                    bounds: *bounds,
                })
                .collect(),
            zones: zones
                .iter()
                .map(|(id, bounds)| ZoneRec {
                    id: WidgetId::new(*id),
                    bounds: *bounds,
                })
                .collect(),
        }
    }

    /// An `assert_inside` failure must name *which* of the three causes
    /// it hit. The unregistered-zone case is the one that matters most:
    /// `FrameInventory::inside` returns `false` for it exactly as it does
    /// for a genuine geometry miss, so without distinct wording an
    /// unsatisfiable step reads as a layout regression and gets "fixed"
    /// in the wrong place.
    #[test]
    fn inside_failure_distinguishes_its_three_causes() {
        let zone = Rect::new(0.0, 0.0, 10.0, 10.0);
        let inside = Rect::new(1.0, 1.0, 3.0, 1.0);
        let outside = Rect::new(50.0, 1.0, 3.0, 1.0);

        // 1. Zone never registered — the step can never pass.
        let msg = inside_failure(&inv(&[("row", inside)], &[]), "row", "sidebar");
        assert!(
            msg.contains("never registered") && msg.contains("register_zone"),
            "unregistered-zone failure must say so and name the fix: {msg}"
        );

        // 2. Zone registered, needle never painted.
        let msg = inside_failure(
            &inv(&[("other", inside)], &[("sidebar", zone)]),
            "row",
            "sidebar",
        );
        assert!(
            msg.contains("never painted") && !msg.contains("never registered"),
            "missing-needle failure must blame the needle, not the zone: {msg}"
        );

        // 3. Both present, geometry does not hold.
        let msg = inside_failure(
            &inv(&[("row", outside)], &[("sidebar", zone)]),
            "row",
            "sidebar",
        );
        assert!(
            msg.contains("not contained") && !msg.contains("never"),
            "geometry failure must report both rects: {msg}"
        );
    }

    /// A backend that hasn't declared a capability skips — and the skip
    /// carries the missing capability's name. "Silence is impossible."
    #[test]
    fn missing_capability_skips_with_a_named_reason() {
        let backend = stub(NO_CAPS);
        let outcome = run_scenario(&scenario_requiring(&["text_selection"]), &backend);
        assert_eq!(
            outcome,
            Outcome::Skip {
                missing_cap: "text_selection".into()
            }
        );
    }

    /// A declared capability does *not* skip — it goes on to build (and
    /// here, fail on the deliberately-unknown fixture).
    #[test]
    fn declared_capability_runs_the_scenario() {
        let backend = stub(SOME_CAPS);
        let outcome = run_scenario(&scenario_requiring(&["text_selection"]), &backend);
        assert!(
            matches!(&outcome, Outcome::Fail { reason, .. } if reason.contains("unknown fixture")),
            "expected an unknown-fixture failure, got {outcome:?}"
        );
    }

    /// The gate reads `BackendCaps::has`, so a `requires` entry that is
    /// not in the vocabulary at all can never be satisfied — by any
    /// backend, however capable. That is deliberate (a typo must not
    /// silently pass), and it is why
    /// `conformance::every_requires_names_a_known_capability` exists to
    /// catch the typo by name rather than leaving it as a permanent skip.
    #[test]
    fn a_capability_outside_the_vocabulary_can_never_be_satisfied() {
        // Written out exhaustively (no `..empty()`) so a new capability
        // field is a compile error here rather than quietly weakening
        // "maximally capable" to "capable of the fields that existed when
        // this was written".
        let every_cap = BackendCaps {
            mouse: true,
            scroll: true,
            drag: true,
            text_selection: true,
            native_menu: true,
            window_chrome: true,
            pointer_cursor: true,
            ime: true,
            file_dialogs: true,
            notifications: true,
        };
        assert_eq!(
            every_cap.names(),
            BackendCaps::vocabulary(),
            "this value is supposed to declare the entire vocabulary"
        );
        let backend = stub(every_cap);
        assert_eq!(
            run_scenario(&scenario_requiring(&["teleportation"]), &backend),
            Outcome::Skip {
                missing_cap: "teleportation".into()
            },
            "a maximally-capable backend must still skip an unknown capability name"
        );
    }

    #[test]
    fn matrix_renders_a_cell_per_backend_and_details_every_non_pass() {
        let rows = vec![
            MatrixRow {
                id: "a.one".into(),
                tier: 1,
                cells: vec![("tui", Outcome::Pass), ("gtk", Outcome::Pass)],
            },
            MatrixRow {
                id: "b.two".into(),
                tier: 1,
                cells: vec![
                    ("tui", Outcome::Pass),
                    (
                        "gtk",
                        Outcome::Skip {
                            missing_cap: "text_selection".into(),
                        },
                    ),
                ],
            },
        ];
        let table = render_matrix(&rows, &["tui", "gtk"]);
        assert!(table.contains("a.one"));
        assert!(table.contains("b.two"));
        assert!(table.contains("pass"));
        assert!(table.contains("skip"));
        assert!(
            table.contains("text_selection"),
            "a skip must name its missing capability in the detail block:\n{table}"
        );
    }
}
