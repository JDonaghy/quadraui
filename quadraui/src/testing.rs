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

// Only pulled in by the paint-time text-run recording sink below, which
// is itself gated to the pixel backends that need it (`gtk`/`win`/`macos`
// — see that section's doc) — TUI has no analogous need (its `find` scans
// a character grid instead), so this import must be gated in lock-step or
// a `--features tui` build (no pixel backend) flags it unused under
// `-D warnings`.
#[cfg(any(feature = "gtk", feature = "win", feature = "macos"))]
use std::cell::RefCell;

use crate::runner::{AppLogic, Reaction};
use crate::{Key, Modifiers, NamedKey, Point, Rect, ScrollDelta, UiEvent, WidgetId};

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

// ── Shared paint-time text-run recording sink (quadraui#721) ───────────
//
// GTK (`gtk::painted_text::show_layout`, quadraui#489) and macOS
// (`macos::text::draw_text`, quadraui#493) each need to record a
// `(text, bounds)` run at the exact point their backend paints text, so
// `find`/`find_bounds`/`inventory`/`screen_has` can locate any
// text-bearing primitive without a per-primitive opt-in. Neither call
// site has a `&mut Backend` in scope to push the run onto — GTK's
// rasterisers are free functions handed only a `&cairo::Context`,
// macOS's `draw_text` is handed only a borrowed `CGContextRef` (see that
// module's "Why direct CoreGraphics FFI" doc) — so both back onto a
// thread-local sink instead. Win-GUI's `DWrite::draw_text` (win/text.rs)
// is the same shape: a `&ID2D1RenderTarget`, no backend handle.
//
// This was a thread-local *duplicated* in `gtk::painted_text` and
// `macos::text` until quadraui#721 lifted it here — the one shared
// implementation every backend's paint-time recorder now wraps, per
// `docs/PRIMITIVE_RULES.md`'s primitive-first rule (#713): a shared
// concern gets one implementation with per-backend adapters over it, not
// N independent copies.
//
// Thread-local rather than a static: GTK painting is single-threaded and
// this pairs with `cargo test`'s one-thread-per-test model exactly as
// `gtk::painted_text`'s original doc explained — each headless driver
// (`GtkDriver`/`MacDriver`/`WinDriver`) lives on its own test thread, so
// one sink per thread is exactly right and needs no locking.
//
// Two distinct cfg predicates gate this section, not one, because the
// three backends reach it through call sites with different OS gating:
//
// - `WinBackend::begin_frame`/`end_frame` (`win/backend.rs`) call
//   `install_text_run_sink`/`take_text_run_sink` unconditionally under
//   `feature = "win"` — those two methods are never `cfg(target_os)`-gated
//   themselves (only the real Direct2D calls inside their bodies are), so
//   `--features win` alone reaches them on *any* host, Linux included
//   (`ci.yml`'s ubuntu-only "Compile check (win feature)" step).
// - `gtk::painted_text::show_layout` / `macos::text::draw_text` /
//   `win::text::draw_text` call `text_run_sink_active`/`record_text_run`
//   — but `win::text` is itself `cfg(target_os = "windows")`-gated (see
//   its module doc), so under `--features win` on a non-Windows host
//   those two calls don't exist anywhere in the crate.
//
// A single shared predicate would therefore leave `text_run_sink_active`/
// `record_text_run` `dead_code` (→ build failure under this crate's
// workflow-wide `-D warnings`) on that ubuntu leg specifically, even
// though `install_text_run_sink`/`take_text_run_sink` are genuinely used
// there. TUI needs neither group at all — its `find` scans a character
// grid instead of a sink.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
thread_local! {
    static TEXT_RUN_SINK: RefCell<Option<Vec<TextRun>>> = const { RefCell::new(None) };
}

/// Install a fresh recording sink, returning the previous one so a
/// (theoretically) nested paint scope can restore it. Pair with
/// [`take_text_run_sink`]. `pub(crate)`: each backend's own paint-time
/// recording module (`gtk::painted_text`, `macos::text`, `win::text`)
/// wraps this rather than exposing the raw sink outside the crate.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) fn install_text_run_sink() -> Option<Vec<TextRun>> {
    TEXT_RUN_SINK.with(|s| s.borrow_mut().replace(Vec::new()))
}

/// Take everything recorded since [`install_text_run_sink`] and restore
/// `previous` as the active sink (`None` = recording off again).
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) fn take_text_run_sink(previous: Option<Vec<TextRun>>) -> Vec<TextRun> {
    TEXT_RUN_SINK.with(|s| {
        let mut slot = s.borrow_mut();
        let recorded = slot.take().unwrap_or_default();
        *slot = previous;
        recorded
    })
}

/// Whether a sink is currently installed. Backends that would otherwise
/// do real measurement work before recording a run (GTK: converting
/// Cairo's current point + Pango pixel size to device coordinates; macOS:
/// `measure_text`) check this first so that work is skipped entirely
/// when recording is off — Win-GUI does too (its bounds are already in
/// hand, but the check keeps this function's reachability, and therefore
/// its `cfg`, identical to [`record_text_run`]'s — see this section's
/// top-level comment for why that has to match exactly).
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
pub(crate) fn text_run_sink_active() -> bool {
    TEXT_RUN_SINK.with(|s| s.borrow().is_some())
}

/// Append one run to the active sink. No-op when recording is off.
///
/// Skips whitespace-only `text` (alignment pads, blank rows, selection
/// prefixes): those can never be a useful `find` needle and would bury
/// the real labels in `painted_texts()` output.
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
pub(crate) fn record_text_run(text: &str, bounds: Rect) {
    if text.trim().is_empty() {
        return;
    }
    TEXT_RUN_SINK.with(|s| {
        if let Some(sink) = s.borrow_mut().as_mut() {
            sink.push(TextRun {
                text: text.to_string(),
                bounds,
            });
        }
    });
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

/// Shared "act" surface for headless [`AppLogic`] test drivers
/// (quadraui#708, issue #708's "Problem 2" — `testing.rs` is a 3-way
/// copy).
///
/// `press`/`type_char`/`press_named`/`ctrl_char`/`click`/`drag` reduce to
/// the exact same body on every backend driver once [`Self::dispatch`]
/// and the three raw mouse primitives exist — only the *how* of
/// `dispatch`/`mouse_down`/`mouse_move`/`mouse_up` differs per backend
/// (compare `GtkDriver::mouse_down`, which routes through
/// `crate::dispatch::dispatch_click` and drag-state tracking, against
/// `TuiDriver::mouse_down`, a bare `UiEvent::MouseDown` dispatch). Before
/// this trait existed, every method built *on top of* those four
/// primitives was transcribed byte-for-byte into `GtkDriver`'s,
/// `MacDriver`'s, and `TuiDriver`'s own inherent impl blocks — the
/// gtk↔macos overlap alone measured 26 duplicated lines in the
/// 2026-09-03 function-level duplication re-audit that opened #708.
///
/// Each driver keeps its own same-named **inherent** method (so no
/// caller needs to import this trait to call `driver.press(..)` — see
/// e.g. `GtkDriver::press`), whose body now just forwards to
/// `DriverInput::method(self, …)`.
pub trait DriverInput: Sized {
    /// Feed one synthetic event through this driver's production
    /// dispatch path. See the concrete driver's own `dispatch` doc for
    /// what "production" means there — it's the one genuinely
    /// per-backend piece (extra macOS caret-blink handles, TUI's
    /// `translate_injected` preprocessing, …), which is exactly why it's
    /// `dispatch` itself that's the required method here and not shared.
    fn dispatch(&mut self, event: UiEvent) -> Reaction;

    /// Press the left mouse button down at `(x, y)`.
    fn mouse_down(&mut self, x: f32, y: f32) -> Reaction;
    /// Move the cursor to `(x, y)` with the left button held.
    fn mouse_move(&mut self, x: f32, y: f32) -> Reaction;
    /// Release the left mouse button at `(x, y)`.
    fn mouse_up(&mut self, x: f32, y: f32) -> Reaction;

    /// Press a key (no modifiers).
    fn press(&mut self, key: Key) -> Reaction {
        self.dispatch(UiEvent::KeyPressed {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// Type a single character key (no modifiers).
    fn type_char(&mut self, c: char) -> Reaction {
        self.press(Key::Char(c))
    }

    /// Press a named (non-printable) key, e.g. [`NamedKey::Enter`].
    fn press_named(&mut self, key: NamedKey) -> Reaction {
        self.press(Key::Named(key))
    }

    /// Press a character key with Ctrl held (e.g. `ctrl_char('c')` to
    /// trigger the runner's copy-on-selection path).
    fn ctrl_char(&mut self, c: char) -> Reaction {
        self.dispatch(UiEvent::KeyPressed {
            key: Key::Char(c),
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        })
    }

    /// Left-click at `(x, y)`. The default is a bare press-down with no
    /// release — the behaviour `GtkDriver`, `MacDriver`, and `TuiDriver`
    /// all already shared before this trait existed. Override this (as
    /// `WinDriver` does) if a backend's real `click` needs a release too.
    fn click(&mut self, x: f32, y: f32) -> Reaction {
        self.mouse_down(x, y)
    }

    /// Left-button drag from `(x0, y0)` to `(x1, y1)`: down → move → up.
    fn drag(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) -> Reaction {
        self.mouse_down(x0, y0);
        self.mouse_move(x1, y1);
        self.mouse_up(x1, y1)
    }
}

/// Shared [`ConformanceDriver::click_text_at`] /
/// [`ConformanceDriver::drag_text`] / [`ConformanceDriver::scroll_at`]
/// bodies for pixel-unit backends (quadraui#708).
///
/// GTK, macOS, and Win-GUI all locate a painted run's bounds and click
/// `bounds.x + 1.0` / `bounds.x + bounds.width - 1.0` for
/// `LeftEdge`/`RightEdge`; only TUI (cell-unit, `+0.5`/`-0.5`, and a
/// `self.screen()` dump folded into its panic messages) diverges enough
/// to keep its own bespoke `ConformanceDriver` impl rather than adopt
/// this trait. The 2026-09-03 duplication audit measured this block at 8
/// duplicated lines between `GtkDriver` and `MacDriver` alone.
pub trait PixelClickConformance: DriverInput {
    /// This driver's type name, for panic messages (`"GtkDriver"`, …) —
    /// mirrors what each driver's hand-written panic message used to
    /// hardcode.
    const NAME: &'static str;

    /// Pixel bounds of the first painted run containing `needle`.
    fn find_bounds(&self, needle: &str) -> Option<Rect>;
    /// Center pixel coordinates of the first painted run containing
    /// `needle`.
    fn find(&self, needle: &str) -> Option<(f32, f32)>;
    /// This backend's current line height, for [`Self::scroll_at`]'s
    /// `ScrollDelta`.
    fn conformance_line_height(&self) -> f32;

    fn click_text_at(&mut self, needle: &str, at: Anchor) {
        let bounds = self
            .find_bounds(needle)
            .unwrap_or_else(|| panic!("{}: {needle:?} not painted", Self::NAME));
        let y = bounds.y + bounds.height / 2.0;
        let x = match at {
            Anchor::Center => bounds.x + bounds.width / 2.0,
            Anchor::LeftEdge => bounds.x + 1.0,
            Anchor::RightEdge => bounds.x + bounds.width - 1.0,
        };
        self.click(x, y);
    }

    fn drag_text(&mut self, from: &str, to: &str) {
        let (x0, y0) = self
            .find(from)
            .unwrap_or_else(|| panic!("{}: {from:?} not painted", Self::NAME));
        let (x1, y1) = self
            .find(to)
            .unwrap_or_else(|| panic!("{}: {to:?} not painted", Self::NAME));
        self.drag(x0, y0, x1, y1);
    }

    fn scroll_at(&mut self, needle: &str, lines: i32) {
        let (x, y) = self
            .find(needle)
            .unwrap_or_else(|| panic!("{}: {needle:?} not painted", Self::NAME));
        let line_height = self.conformance_line_height();
        self.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, lines as f32 * line_height),
            position: Point::new(x, y),
        });
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

    // ─── Shared text-run recording sink (quadraui#721) ────────────────────
    //
    // Gated to match `record_text_run`/`text_run_sink_active`'s own cfg
    // (the narrower of the sink module's two predicates — see its
    // top-level comment): every function this sub-module exercises must
    // actually exist under whichever single-backend feature combination
    // is compiling it, and `install_text_run_sink`/`take_text_run_sink`
    // are available under strictly more feature combinations than that
    // (`--features win` alone on a non-Windows host, in particular) — so
    // gating on the narrower predicate is what guarantees all four names
    // resolve together.
    #[cfg(any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    ))]
    mod text_run_sink {
        use super::*;

        /// Recording is off by default: [`record_text_run`] must be a
        /// no-op until [`install_text_run_sink`] has been called — the "a
        /// live app never pays for this" contract every backend's own
        /// recording toggle (`GtkBackend::set_painted_text_recording`,
        /// etc.) relies on.
        #[test]
        fn record_is_a_no_op_with_no_sink_installed() {
            // No `install_text_run_sink()` call — proves recording
            // doesn't leak across tests via some global default, and
            // that a run painted with nothing installed is silently
            // dropped, not buffered somewhere it could resurface later.
            record_text_run("stray", Rect::new(0.0, 0.0, 1.0, 1.0));
            let prev = install_text_run_sink();
            let runs = take_text_run_sink(prev);
            assert!(
                runs.is_empty(),
                "a run recorded before any sink existed must not appear once one is installed: {runs:?}"
            );
        }

        #[test]
        fn install_then_record_then_take_round_trips_one_run() {
            assert!(!text_run_sink_active());
            let prev = install_text_run_sink();
            assert!(text_run_sink_active());

            record_text_run("hello", Rect::new(1.0, 2.0, 3.0, 4.0));

            let runs = take_text_run_sink(prev);
            assert_eq!(
                runs,
                vec![TextRun {
                    text: "hello".into(),
                    bounds: Rect::new(1.0, 2.0, 3.0, 4.0),
                }]
            );
            assert!(
                !text_run_sink_active(),
                "take_text_run_sink(None) must leave recording off again"
            );
        }

        /// Whitespace-only runs (alignment pads, blank rows, selection
        /// prefixes) are dropped — they can never be a useful `find`
        /// needle.
        #[test]
        fn whitespace_only_runs_are_not_recorded() {
            let prev = install_text_run_sink();
            for text in ["", "   ", "real"] {
                record_text_run(text, Rect::new(0.0, 0.0, 1.0, 1.0));
            }
            let runs = take_text_run_sink(prev);
            assert_eq!(
                runs.len(),
                1,
                "only the non-blank run should record: {runs:?}"
            );
            assert_eq!(runs[0].text, "real");
        }

        /// [`install_text_run_sink`] returns the previous sink so a
        /// (theoretically) nested paint scope can restore it exactly —
        /// [`take_text_run_sink`]'s `previous` parameter — rather than
        /// clobbering an outer scope's in-flight recording.
        #[test]
        fn nested_install_restores_the_outer_sink_on_take() {
            let outer_prev = install_text_run_sink();
            record_text_run("outer-before", Rect::new(0.0, 0.0, 1.0, 1.0));

            let inner_prev = install_text_run_sink();
            record_text_run("inner", Rect::new(0.0, 0.0, 1.0, 1.0));
            let inner_runs = take_text_run_sink(inner_prev);
            assert_eq!(inner_runs.len(), 1);
            assert_eq!(inner_runs[0].text, "inner");

            // The outer sink is active again and still carries its own
            // earlier run — the inner scope's recording didn't bleed
            // into it.
            assert!(text_run_sink_active());
            record_text_run("outer-after", Rect::new(0.0, 0.0, 1.0, 1.0));
            let outer_runs = take_text_run_sink(outer_prev);
            assert_eq!(
                outer_runs
                    .iter()
                    .map(|r| r.text.as_str())
                    .collect::<Vec<_>>(),
                vec!["outer-before", "outer-after"]
            );
        }
    }

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

    // ─── quadraui#708: DriverInput / PixelClickConformance defaults ───────
    //
    // Every real backend driver now delegates `press`/`type_char`/
    // `press_named`/`ctrl_char`/`click`/`drag` (and, for the pixel-unit
    // backends, `click_text_at`/`drag_text`/`scroll_at`) to these traits'
    // default method bodies — they're the single source of truth the
    // gtk/tui/mac/win driver tests exercise indirectly. This fixture
    // exercises the default bodies *directly*, with no backend feature at
    // all, so a future edit to a default here has a test right beside it.

    use crate::MouseButton;

    /// Minimal fake driver recording every dispatched [`UiEvent`] plus a
    /// hand-built `needle -> bounds` map.
    struct FakeDriver {
        events: Vec<UiEvent>,
        painted: Vec<(&'static str, Rect)>,
        line_height: f32,
    }

    impl FakeDriver {
        fn new() -> Self {
            Self {
                events: Vec::new(),
                painted: vec![("Toggle", Rect::new(10.0, 20.0, 40.0, 8.0))],
                line_height: 16.0,
            }
        }
    }

    impl DriverInput for FakeDriver {
        fn dispatch(&mut self, event: UiEvent) -> Reaction {
            self.events.push(event);
            Reaction::Redraw
        }

        fn mouse_down(&mut self, x: f32, y: f32) -> Reaction {
            self.dispatch(UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(x, y),
                modifiers: Modifiers::default(),
            })
        }

        fn mouse_move(&mut self, x: f32, y: f32) -> Reaction {
            self.dispatch(UiEvent::MouseMoved {
                position: Point::new(x, y),
                buttons: crate::ButtonMask {
                    left: true,
                    ..crate::ButtonMask::default()
                },
            })
        }

        fn mouse_up(&mut self, x: f32, y: f32) -> Reaction {
            self.dispatch(UiEvent::MouseUp {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(x, y),
            })
        }
    }

    impl PixelClickConformance for FakeDriver {
        const NAME: &'static str = "FakeDriver";

        fn find_bounds(&self, needle: &str) -> Option<Rect> {
            self.painted
                .iter()
                .find(|(t, _)| *t == needle)
                .map(|(_, b)| *b)
        }

        fn find(&self, needle: &str) -> Option<(f32, f32)> {
            self.find_bounds(needle)
                .map(|b| (b.x + b.width / 2.0, b.y + b.height / 2.0))
        }

        fn conformance_line_height(&self) -> f32 {
            self.line_height
        }
    }

    #[test]
    fn driver_input_press_family_dispatches_expected_key_events() {
        let mut d = FakeDriver::new();
        d.press(Key::Char('a'));
        d.type_char('b');
        d.press_named(NamedKey::Enter);
        d.ctrl_char('c');

        assert_eq!(
            d.events,
            vec![
                UiEvent::KeyPressed {
                    key: Key::Char('a'),
                    modifiers: Modifiers::default(),
                    repeat: false,
                },
                UiEvent::KeyPressed {
                    key: Key::Char('b'),
                    modifiers: Modifiers::default(),
                    repeat: false,
                },
                UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Enter),
                    modifiers: Modifiers::default(),
                    repeat: false,
                },
                UiEvent::KeyPressed {
                    key: Key::Char('c'),
                    modifiers: Modifiers {
                        ctrl: true,
                        ..Modifiers::default()
                    },
                    repeat: false,
                },
            ]
        );
    }

    #[test]
    fn driver_input_click_is_a_bare_mouse_down_by_default() {
        let mut d = FakeDriver::new();
        d.click(3.0, 4.0);
        assert_eq!(
            d.events,
            vec![UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(3.0, 4.0),
                modifiers: Modifiers::default(),
            }],
            "the default `click` is a bare press-down — WinDriver overrides \
             this to add a release, everyone else keeps the default"
        );
    }

    #[test]
    fn driver_input_drag_is_down_then_move_then_up() {
        let mut d = FakeDriver::new();
        d.drag(1.0, 2.0, 5.0, 6.0);
        assert_eq!(d.events.len(), 3);
        assert!(matches!(d.events[0], UiEvent::MouseDown { .. }));
        assert!(matches!(d.events[1], UiEvent::MouseMoved { .. }));
        assert!(matches!(d.events[2], UiEvent::MouseUp { .. }));
    }

    #[test]
    fn pixel_click_conformance_click_text_at_resolves_each_anchor() {
        // "Toggle" bounds: x=10, y=20, w=40, h=8 → center (30, 24),
        // left-edge (11, 24), right-edge (49, 24).
        let mut d = FakeDriver::new();
        PixelClickConformance::click_text_at(&mut d, "Toggle", Anchor::Center);
        PixelClickConformance::click_text_at(&mut d, "Toggle", Anchor::LeftEdge);
        PixelClickConformance::click_text_at(&mut d, "Toggle", Anchor::RightEdge);

        let positions: Vec<(f32, f32)> = d
            .events
            .iter()
            .map(|e| match e {
                UiEvent::MouseDown { position, .. } => (position.x, position.y),
                other => panic!("expected MouseDown, got {other:?}"),
            })
            .collect();
        assert_eq!(positions, vec![(30.0, 24.0), (11.0, 24.0), (49.0, 24.0)]);
    }

    #[test]
    #[should_panic(expected = "FakeDriver: \"nope\" not painted")]
    fn pixel_click_conformance_click_text_at_panics_naming_the_driver_when_not_painted() {
        let mut d = FakeDriver::new();
        PixelClickConformance::click_text_at(&mut d, "nope", Anchor::Center);
    }

    #[test]
    fn pixel_click_conformance_scroll_at_dispatches_scroll_scaled_by_line_height() {
        let mut d = FakeDriver::new();
        PixelClickConformance::scroll_at(&mut d, "Toggle", 2);
        assert_eq!(d.events.len(), 1);
        match &d.events[0] {
            UiEvent::Scroll { delta, .. } => assert_eq!(delta.y, 2.0 * 16.0),
            other => panic!("expected Scroll, got {other:?}"),
        }
    }
}
