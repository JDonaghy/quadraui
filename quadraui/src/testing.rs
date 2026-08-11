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

/// Semantic paint inventory for one rendered frame.
///
/// This is the minimal slice needed to back [`ConformanceDriver::inventory`]
/// today: `text_runs` is populated from each backend's existing text
/// search (TUI's cell-grid scan, GTK's `painted_text` map). `zones` is
/// reserved for the widget-zone contract `docs/SMELL_AUDIT_2026-07.md`
/// §6.2/B3 describes (`FrameHitMap` / layout-return provenance) and is
/// empty until that recording lands — declared here so the field exists
/// on the wire and callers don't need a breaking change when it's filled
/// in.
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

    /// Semantic paint inventory for the last rendered frame.
    fn inventory(&self) -> FrameInventory;

    /// True if any painted text contains `needle`.
    fn screen_has(&self, needle: &str) -> bool;

    /// Whether the app has returned [`crate::Reaction::Exit`].
    fn exited(&self) -> bool;
}
