//! Backend-agnostic conformance-test surface (quadraui#488).
//!
//! [`ConformanceDriver`] promotes the `ExampleDriver` trait that was
//! sketched in `docs/TESTING.md` ("Cross-backend example tests: shared
//! bodies, per-backend adapters") and re-implemented as a test-local copy
//! in `tests/cross_backend_parity.rs`, into a real, feature-independent
//! module every backend driver can implement — see
//! `docs/SMELL_AUDIT_2026-07.md` §6.3 for the full design this is the first
//! slice of.
//!
//! This module has **no `tui`/`gtk`/`macos`/`win` feature gate** — the
//! trait and its supporting types (`LogicalViewport`, `Anchor`,
//! `TextRun`, `ZoneRec`, `FrameInventory`) compile unconditionally so a
//! shared test body can be written against `ConformanceDriver` without
//! pulling in any backend. The `impl ConformanceDriver for {Tui,Gtk}Driver`
//! blocks live in `quadraui::tui::testing` / `quadraui::gtk::testing`,
//! each behind its own backend feature flag.
//!
//! ## Why a promoted trait, not the test-local one
//!
//! The test-local `ExampleDriver` in `cross_backend_parity.rs` dropped
//! `drag_text` — the one method `docs/TESTING.md`'s canonical sketch
//! includes — so no drag scenario could be shared across backends. It also
//! froze the two drivers' gratuitous API divergences (constructor units,
//! `screen()` return type, missing `GtkDriver::app_mut`, a TUI `find()`
//! that assumes one cell per character) in place, since a test-local trait
//! has no reach into the driver modules themselves to fix them.
//!
//! ## Rules for shared bodies (unchanged from `docs/TESTING.md`)
//!
//! 1. **Locate by semantics, never literal coordinates.** Use
//!    `click_text`/`click_text_at`/`drag_text`, not a hardcoded
//!    `click(12.0, 3.0)` — TUI cells and GTK pixels are different units,
//!    so a literal in a shared body would silently be wrong on one side.
//! 2. **Assert on logic/text, not pixels, in shared bodies.** `screen_has`
//!    works identically on every backend.

use crate::runner::AppLogic;
use crate::{NamedKey, Rect, WidgetId};

/// Backend-neutral viewport size for [`ConformanceDriver::new_fixture`].
///
/// Interpreted per backend: TUI treats `cols`/`rows` as terminal cells
/// directly; pixel backends (GTK, Win-GUI, macOS) scale by a nominal
/// `char_width`/`line_height` to get a device-unit surface size. Either
/// way, a shared test body never writes a pixel or cell number itself —
/// it only ever picks a `LogicalViewport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalViewport {
    pub cols: u32,
    pub rows: u32,
}

impl LogicalViewport {
    pub const fn new(cols: u32, rows: u32) -> Self {
        Self { cols, rows }
    }
}

/// Where within a located text run's bounds a click should land.
///
/// `click_text`/[`ConformanceDriver::click_text`] is `click_text_at(needle,
/// Anchor::Center)`; `LeftEdge`/`RightEdge` exist for widgets whose hit
/// regions are sensitive to which end of a label was clicked (e.g. a
/// divider immediately to the right of a column header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Center,
    LeftEdge,
    RightEdge,
}

/// One run of text painted during a frame, and where — the portable
/// equivalent of `TuiDriver::find`'s cell scan or `GtkDriver`'s
/// `(text, bounds)` map, per `docs/SMELL_AUDIT_2026-07.md` §6.2.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub bounds: Rect,
}

/// One registered widget zone (a hit-testable region) painted during a
/// frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneRec {
    pub id: WidgetId,
    pub bounds: Rect,
}

/// Semantic paint inventory for one rendered frame — the cross-backend
/// observable contract `docs/SMELL_AUDIT_2026-07.md` §6.2/B3 describes,
/// aligned with quadraweb#322's id→rect map (quadraui#490).
///
/// `text_runs` is populated from each backend's existing text search
/// (TUI's cell-grid scan, GTK's `painted_text` map) — this covers every
/// text-bearing primitive with no per-widget opt-in required.
///
/// `zones` is populated from [`crate::Backend::register_zone`] calls made
/// during the frame, and **coverage is opt-in per paint site**. Exactly
/// one composer registers zones today —
/// [`crate::compose::app_shell::AppShell::render`], via its
/// `register_chrome_zones` helper — and it contributes:
///
/// - one `app-shell:`-prefixed zone per chrome region it lays out:
///   `window`, `activity-bar`, `main-content`, and (when that region
///   exists this frame) `title-bar`, `sidebar-header`, `sidebar-content`,
///   `divider`, `bottom-panel`, `command-line`, `status-bar`;
/// - one zone per activity-bar item, keyed by that panel's own
///   `WidgetId` (e.g. `panel:explorer`) rather than an `app-shell:` name.
///
/// Every other primitive contributes **no** zone until it is wired to
/// call `register_zone` — no zone, rather than a wrong one. Because
/// [`Self::inside`] answers `false` for an unregistered zone exactly as
/// it does for a run that landed outside a registered one, a caller
/// asserting against a zone id no paint site produces gets an
/// unsatisfiable assertion, not a flaky one; the conformance suite's
/// `every_asserted_zone_is_registered_by_every_backend` guard exists to
/// catch precisely that and name it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameInventory {
    pub text_runs: Vec<TextRun>,
    pub zones: Vec<ZoneRec>,
}

impl FrameInventory {
    pub fn text_runs(&self) -> &[TextRun] {
        &self.text_runs
    }

    pub fn zones(&self) -> &[ZoneRec] {
        &self.zones
    }

    // ── Relational assertion vocabulary (quadraui#490) ──────────────────
    //
    // These are the "no hardcoded coordinates" assertions the issue calls
    // for: every one of them is computed from `Rect`s this same
    // `FrameInventory` already reports, in whichever unit the backend
    // that produced it uses (cells for TUI, pixels for GTK) — a shared
    // test body never writes a literal coordinate itself, it only ever
    // names two painted things and asks how they relate.

    /// True if any painted text run contains `needle`.
    pub fn screen_has(&self, needle: &str) -> bool {
        self.text_runs.iter().any(|r| r.text.contains(needle))
    }

    /// True if no painted text run contains `needle` — the negative
    /// counterpart to [`Self::screen_has`], for asserting something was
    /// *not* painted this frame (e.g. a closed panel's content).
    pub fn absent(&self, needle: &str) -> bool {
        !self.screen_has(needle)
    }

    /// Number of painted text runs containing `needle`. Useful for
    /// asserting a repeated element's cardinality (e.g. "3 rows contain
    /// this label") without caring where each instance landed.
    pub fn count(&self, needle: &str) -> usize {
        self.text_runs
            .iter()
            .filter(|r| r.text.contains(needle))
            .count()
    }

    /// Bounds of the first text run containing `needle`, in this frame's
    /// paint order. `None` if nothing painted matched.
    fn locate(&self, needle: &str) -> Option<Rect> {
        self.text_runs
            .iter()
            .find(|r| r.text.contains(needle))
            .map(|r| r.bounds)
    }

    /// Bounds of the zone registered under `id` via
    /// [`crate::Backend::register_zone`], if any.
    fn zone_bounds(&self, id: &WidgetId) -> Option<Rect> {
        self.zones.iter().find(|z| &z.id == id).map(|z| z.bounds)
    }

    /// True if `a`'s painted bounds lie entirely to the left of `b`'s —
    /// `a`'s right edge at or before `b`'s left edge. Panics-free: if
    /// either needle wasn't painted this frame, returns `false` (the
    /// same "didn't happen" answer a missing needle gets everywhere else
    /// in this vocabulary — callers that need to know *why* should pair
    /// this with [`Self::screen_has`]).
    pub fn left_of(&self, a: &str, b: &str) -> bool {
        match (self.locate(a), self.locate(b)) {
            (Some(ra), Some(rb)) => ra.x + ra.width <= rb.x,
            _ => false,
        }
    }

    /// True if `a`'s painted bounds lie entirely above `b`'s — `a`'s
    /// bottom edge at or before `b`'s top edge.
    pub fn above(&self, a: &str, b: &str) -> bool {
        match (self.locate(a), self.locate(b)) {
            (Some(ra), Some(rb)) => ra.y + ra.height <= rb.y,
            _ => false,
        }
    }

    /// True if `a` and `b`'s painted bounds vertically overlap — the
    /// same logical text row, even across backends whose native units
    /// (TUI cell rows vs GTK sub-pixel baselines) never line up on exact
    /// equality.
    pub fn same_row(&self, a: &str, b: &str) -> bool {
        match (self.locate(a), self.locate(b)) {
            (Some(ra), Some(rb)) => ra.y < rb.y + rb.height && rb.y < ra.y + ra.height,
            _ => false,
        }
    }

    /// True if `needle`'s painted bounds lie entirely within the zone
    /// registered under `zone_id`. `false` if either the needle wasn't
    /// painted or the zone wasn't registered this frame.
    pub fn inside(&self, needle: &str, zone_id: &WidgetId) -> bool {
        match (self.locate(needle), self.zone_bounds(zone_id)) {
            (Some(r), Some(z)) => {
                r.x >= z.x
                    && r.y >= z.y
                    && r.x + r.width <= z.x + z.width
                    && r.y + r.height <= z.y + z.height
            }
            _ => false,
        }
    }
}

/// Backend-agnostic driver surface a shared conformance-test body needs.
///
/// Implemented once per backend, behind that backend's feature flag
/// (`quadraui::tui::testing::TuiDriver`, `quadraui::gtk::testing::GtkDriver`).
/// A test body written generically over `D: ConformanceDriver` runs
/// unmodified against every backend that implements it.
pub trait ConformanceDriver: Sized {
    /// The [`AppLogic`] this driver instance wraps.
    type App: AppLogic;

    /// Build a driver for `app` on a `viewport`-sized surface, running the
    /// app's `setup` hook and painting the first frame — the
    /// [`LogicalViewport`]-aligned constructor `docs/SMELL_AUDIT_2026-07.md`
    /// §6.3 calls for, sitting alongside (not replacing) each backend's
    /// existing native-unit `new`.
    fn new_fixture(app: Self::App, viewport: LogicalViewport) -> Self;

    /// Press a named (non-printable) key, e.g. [`NamedKey::Enter`].
    fn press_named(&mut self, key: NamedKey);

    /// Type a single character key (no modifiers).
    fn type_char(&mut self, c: char);

    /// Press a character key with Ctrl held — the runner-level path that
    /// turns `Ctrl-C` over an active selection into
    /// [`crate::UiEvent::TextCopied`] rather than a raw key press. Needed
    /// by any scenario that exercises copy (audit §6.5 example 3), which
    /// is why it sits on the trait rather than only on each concrete
    /// driver.
    fn ctrl_char(&mut self, c: char);

    /// Type each character of `s` in turn (no modifiers).
    fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.type_char(c);
        }
    }

    /// Locate `needle`'s painted bounds and click its center. Equivalent
    /// to `click_text_at(needle, Anchor::Center)`.
    fn click_text(&mut self, needle: &str) {
        self.click_text_at(needle, Anchor::Center);
    }

    /// Locate `needle`'s painted bounds and click the point `at` picks out
    /// within them.
    fn click_text_at(&mut self, needle: &str, at: Anchor);

    /// Locate `from` and `to`'s painted bounds and drag from the center of
    /// one to the center of the other (down → move → up) — the method
    /// `docs/TESTING.md`'s canonical `ExampleDriver` sketch included and
    /// the test-local copy in `tests/cross_backend_parity.rs` dropped.
    fn drag_text(&mut self, from: &str, to: &str);

    /// Locate `needle`'s painted bounds and dispatch a scroll-wheel event
    /// there, `lines` deep (in this backend's `line_height` multiples;
    /// positive = scroll up, matching [`crate::ScrollDelta`]'s convention).
    fn scroll_at(&mut self, needle: &str, lines: i32);

    /// The [`crate::BackendCaps`] the backend under this driver declares
    /// — i.e. `Backend::backend_caps` on the *real* backend instance the
    /// driver wraps, never a re-statement of it.
    ///
    /// quadraui#492 review: the conformance runner used to pair each
    /// backend with a hand-maintained `&[&str]` of capability names,
    /// disconnected from what the backend itself declares, so the two
    /// could drift silently in both directions. This method is the wire
    /// between them — `BackendReg::caps` is now literally this value, so
    /// a scenario's `requires` gate can only ever match against what the
    /// backend really said. Required (no default) on purpose: there is no
    /// honest answer to guess, and inventing one is exactly the silence
    /// `BackendCaps` exists to break.
    fn backend_caps(&self) -> crate::BackendCaps;

    /// Semantic paint inventory for the last rendered frame.
    fn inventory(&self) -> FrameInventory;

    /// True if any painted text contains `needle`.
    fn screen_has(&self, needle: &str) -> bool;

    /// Whether the app has returned [`crate::Reaction::Exit`].
    fn exited(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small hand-built inventory: an activity-bar-style icon column
    /// (`"E"`, `"S"`, `"G"` stacked at x=0..2) to the left of a sidebar
    /// header/content pair (`"SOURCE CONTROL"` above `"content"` at
    /// x=10..40), plus one registered zone around the sidebar content
    /// run. Exercises the relational vocabulary with no backend at all —
    /// the cross-backend agreement itself is proven separately in
    /// `tests/cross_backend_parity.rs::frame_inventory_relations_agree_tui_and_gtk`.
    fn fixture() -> FrameInventory {
        FrameInventory {
            text_runs: vec![
                TextRun {
                    text: "E".into(),
                    bounds: Rect::new(0.0, 0.0, 1.0, 1.0),
                },
                TextRun {
                    text: "S".into(),
                    bounds: Rect::new(0.0, 1.0, 1.0, 1.0),
                },
                TextRun {
                    text: "G".into(),
                    bounds: Rect::new(0.0, 2.0, 1.0, 1.0),
                },
                TextRun {
                    text: "SOURCE CONTROL".into(),
                    bounds: Rect::new(10.0, 0.0, 14.0, 1.0),
                },
                TextRun {
                    text: "content".into(),
                    bounds: Rect::new(10.0, 1.0, 7.0, 1.0),
                },
            ],
            zones: vec![ZoneRec {
                id: WidgetId::new("sidebar-content"),
                bounds: Rect::new(10.0, 1.0, 30.0, 1.0),
            }],
        }
    }

    #[test]
    fn screen_has_and_absent() {
        let inv = fixture();
        assert!(inv.screen_has("SOURCE"));
        assert!(inv.screen_has("CONTROL"));
        assert!(inv.absent("git"));
        assert!(!inv.screen_has("git"));
    }

    #[test]
    fn count_matches_every_run_containing_needle() {
        let inv = fixture();
        // "S" appears in the "S" icon run and inside "SOURCE CONTROL".
        assert_eq!(inv.count("S"), 2);
        assert_eq!(inv.count("G"), 1);
        assert_eq!(inv.count("nope"), 0);
    }

    #[test]
    fn left_of_is_true_for_the_icon_column_vs_the_sidebar() {
        let inv = fixture();
        assert!(inv.left_of("G", "SOURCE CONTROL"));
        assert!(inv.left_of("G", "content"));
        assert!(!inv.left_of("SOURCE CONTROL", "G"), "reverse must not hold");
    }

    #[test]
    fn above_is_true_for_header_vs_content() {
        let inv = fixture();
        assert!(inv.above("SOURCE CONTROL", "content"));
        assert!(
            !inv.above("content", "SOURCE CONTROL"),
            "reverse must not hold"
        );
        // Same-column icons stack top to bottom too.
        assert!(inv.above("E", "S"));
        assert!(inv.above("S", "G"));
    }

    #[test]
    fn same_row_true_for_overlapping_ranges_false_across_rows() {
        let inv = fixture();
        assert!(inv.same_row("E", "SOURCE CONTROL"));
        assert!(inv.same_row("S", "content"));
        assert!(!inv.same_row("E", "content"));
    }

    #[test]
    fn inside_checks_containment_within_a_registered_zone() {
        let inv = fixture();
        let zone = WidgetId::new("sidebar-content");
        assert!(inv.inside("content", &zone));
        assert!(
            !inv.inside("SOURCE CONTROL", &zone),
            "the header run sits above the zone's y range, not inside it"
        );
        assert!(
            !inv.inside("content", &WidgetId::new("no-such-zone")),
            "an unregistered zone id must never report containment"
        );
    }

    #[test]
    fn relations_are_false_not_panicking_when_a_needle_never_painted() {
        let inv = fixture();
        assert!(!inv.left_of("nope", "G"));
        assert!(!inv.above("G", "nope"));
        assert!(!inv.same_row("nope", "G"));
        assert!(!inv.inside("nope", &WidgetId::new("sidebar-content")));
    }
}
