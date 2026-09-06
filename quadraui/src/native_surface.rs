//! `NativeSurface` — the low-level drawing-verb trait underneath the
//! three pixel backends (issue #807, Phase 1 of the `NativeSurface`
//! milestone; child of #785; `docs/SMELL_AUDIT_2026-07.md` §5).
//!
//! # The problem this starts to fix
//!
//! [`crate::Backend`] fuses two different jobs into one trait: "what to
//! paint" (71 `draw_*`/`*_layout` methods, one per primitive) and "how
//! the platform draws at all" (frame lifecycle, measurement, and a
//! handful of drawing verbs every one of those 71 methods ultimately
//! bottoms out in). Each pixel backend already has its own *private*
//! copy of that second job — GTK's `set_source` / `cr.rectangle` /
//! `gtk::painted_text::show_layout` (all crate-private), macOS's
//! per-file `fill_rect`/`color_to_cg` duplicated across ~30 rasteriser
//! modules plus [`crate::macos::text::draw_text`]/[`crate::macos::text::measure_text`],
//! and Windows's `win::text::fill_rect`/`stroke_rect`/`push_clip`/
//! `DWrite::draw_text` (also crate-private) — but nothing names it as a
//! shared shape, so nothing can be written once against it.
//!
//! `NativeSurface` is that shape: ~15 backend-primitive verbs, every one
//! `Rect`/`Point`-taking rather than a loose bag of `x`/`y`/`w`/`h`
//! scalars (the shape change that also lets a future phase drop the
//! `#[allow(clippy::too_many_arguments)]`s scattered across `gtk::*`,
//! `macos::*` and `win::*` — see this issue's PR description for the
//! measured before/after; **Phase 1 does not remove any of them itself**,
//! since no call site is migrated yet — see *Scope* below).
//!
//! # Scope — Phase 1 of three
//!
//! This phase only:
//! 1. Defines the trait (this file).
//! 2. Implements it for [`crate::gtk::backend::GtkBackend`],
//!    [`crate::macos::backend::MacBackend`] and
//!    [`crate::win::backend::WinBackend`] — TUI is deliberately excluded
//!    (see *Why TUI stays out* below).
//!
//! It does **not**: migrate any of the 71 `Backend::draw_*`/`*_layout`
//! methods, or any per-primitive rasteriser module, to call through
//! `NativeSurface` instead of their own private helpers. Every existing
//! call site is untouched, so this phase changes zero paint/click
//! behaviour — it only proves the trait is implementable, honestly,
//! against what each backend already has. Wiring `Backend`'s `draw_*`
//! methods (and eventually the ~30 duplicated macOS `fill_rect`/
//! `color_to_cg` copies) through this trait instead is Phase 2; dropping
//! the accumulated `too_many_arguments` allows as call sites migrate to
//! `Rect`-taking signatures is Phase 3.
//!
//! # Why every method is `surface_`-prefixed
//!
//! [`crate::Backend`] already has methods named `begin_frame`,
//! `end_frame`, `viewport`, `line_height` and `char_width` — the exact
//! concepts this trait's frame/measurement verbs cover, on the *same*
//! concrete types ([`GtkBackend`](crate::gtk::backend::GtkBackend) etc.
//! implement both traits). Naming this trait's methods identically would
//! make every existing `backend.line_height()` call site in this crate
//! ambiguous the moment both traits are in scope together (Rust's method
//! resolution doesn't prefer one trait over another same-arity-name
//! sibling), forcing fully-qualified syntax everywhere `Backend` is used
//! today — a real behaviour-affecting change this phase promises not to
//! make. The `surface_` prefix sidesteps that entirely: no existing call
//! site anywhere in the crate needs to change for this trait to exist.
//!
//! # Why TUI stays out
//!
//! TUI paints a cell grid, not a pixel canvas — there is no sub-cell
//! `Rect`, no fractional `stroke_width`, no clip rect narrower than a
//! whole cell. Forcing [`crate::tui::backend::TuiBackend`] to implement
//! this trait would mean either lying about sub-cell precision or
//! growing every verb an extra "how do I round this to cells" branch,
//! which is exactly the kind of leaky abstraction `Backend` itself
//! already avoids by keeping TUI a first-class separate implementation
//! (see `Backend`'s own module doc, and this issue's description).
//!
//! # Sealed to this crate
//!
//! `pub(crate)`, not `pub`: this is an internal decomposition of
//! `Backend`'s existing (sealed, in-tree-only) implementors, not new
//! public API. Nothing outside this crate can see or implement it, so
//! adding, removing, or reshaping a verb here is never a breaking change
//! to `coord-tui`/`vimcode` — see `CLAUDE.md`'s *Downstream consumers*
//! section; this file adds zero surface those blast-radius rules apply
//! to.

use crate::backend::ImagePaintResult;
use crate::{Color, Image, Point, Rect, Viewport};

/// The ~15 drawing verbs shared by every pixel backend, extracted from
/// helpers each of [`crate::gtk::backend::GtkBackend`],
/// [`crate::macos::backend::MacBackend`] and
/// [`crate::win::backend::WinBackend`] already had privately. See the
/// module doc for scope, naming, and why TUI does not implement this.
///
/// `#[allow(dead_code)]`: Phase 1's whole point is that no production call
/// site is wired up yet (see the module doc's *Scope* section) — every
/// method is exercised by each backend's own test suite (proving the
/// implementations are real, not just type-checked stubs) but nothing in
/// the shipped library calls through this trait until Phase 2 migrates a
/// `Backend::draw_*` method onto it. Without this, `-D warnings` fails the
/// ordinary (non-test) build the moment this file lands, for a gap that is
/// this phase's entire scope, not a bug — see this issue's PR description
/// for why that tradeoff is deliberate here rather than pulled forward.
///
/// "Each backend's own test suite" runs on different hosts, not this one:
/// `gtk::backend`'s and `win::backend`'s `native_surface_*` tests execute
/// on this dev machine (the win ones behind a further `target_os =
/// "windows"` per-test gate, since `HeadlessSurface` needs a real
/// Direct2D device); `macos::backend`'s can only execute on `macos.yml`'s
/// `macos-latest` runner, because `mod macos` itself only compiles under
/// `#[cfg(all(feature = "macos", target_os = "macos"))]` (see `lib.rs`).
/// A Linux `cargo check --features macos --target aarch64-apple-darwin`
/// type-checks the mac tests but never runs them.
#[allow(dead_code)]
pub(crate) trait NativeSurface {
    // ─── Frame + viewport ──────────────────────────────────────────────
    /// Begin a frame at `viewport`. Phase 1 implementations simply
    /// forward to [`crate::Backend::begin_frame`] — this verb exists so
    /// a future call site that only holds `&mut dyn NativeSurface` (a
    /// primitive rasteriser that has been migrated off direct cairo/
    /// CoreGraphics/Direct2D access) doesn't need `Backend` in scope too.
    fn surface_begin_frame(&mut self, viewport: Viewport);

    /// Flush the current frame. See [`Self::surface_begin_frame`]'s doc
    /// for why this forwards to [`crate::Backend::end_frame`] rather
    /// than duplicating its logic.
    fn surface_end_frame(&mut self);

    /// Current viewport, scale included ([`Viewport::scale`] carries the
    /// DPI ratio already — see that type's docs — so this single verb
    /// covers "viewport/scale" as one call, not two).
    fn surface_viewport(&self) -> Viewport;

    // ─── Measurement ───────────────────────────────────────────────────
    /// Height of one standard text row in surface-native units. Mirrors
    /// [`crate::Backend::line_height`].
    fn surface_line_height(&self) -> f32;

    /// Approximate monospace character width in surface-native units.
    /// Mirrors [`crate::Backend::char_width`].
    fn surface_char_width(&self) -> f32;

    /// `(width, height)` of `text` laid out against this surface's
    /// current font, in surface-native units.
    fn surface_measure_text(&self, text: &str) -> (f32, f32);

    // ─── Fills + strokes ────────────────────────────────────────────────
    /// Fill `rect` with a solid `color`.
    fn surface_fill_rect(&mut self, rect: Rect, color: Color);

    /// Stroke the outline of `rect` in `color` at `stroke_width`. Every
    /// backend's underlying primitive insets the stroke so it lands
    /// fully inside `rect` rather than straddling its boundary — see the
    /// crate-private `win::text::stroke_rect`'s doc comment for the
    /// geometry this convention exists to keep crisp.
    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32);

    /// Paint `text` at `rect`'s top-left corner in `color`, using this
    /// surface's current font.
    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color);

    /// Stroke a line segment from `from` to `to` in `color` at
    /// `stroke_width`.
    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32);

    // ─── Clipping ──────────────────────────────────────────────────────
    /// Push an axis-aligned clip rect. Every push must be balanced by a
    /// [`Self::surface_pop_clip`] — see the crate-private
    /// `win::text::push_clip`'s doc comment for the rationale (content
    /// rasterisers that paint wider than their own bounds, e.g. a
    /// horizontally-scrolled row, use this pair to keep scrolled-off
    /// content from bleeding into neighbours).
    fn surface_push_clip(&mut self, rect: Rect);

    /// Pop the clip most recently pushed by [`Self::surface_push_clip`].
    fn surface_pop_clip(&mut self);

    // ─── Images ────────────────────────────────────────────────────────
    /// Paint `image` into `rect`. Phase 1 implementations forward to
    /// [`crate::Backend::draw_image`] — see that method's doc comment
    /// for the per-backend decode contract (GTK: `gdk_pixbuf`; macOS:
    /// categorically [`ImagePaintResult::Unsupported`] until a real
    /// `NSImage` decoder lands, #802; Win-GUI: Direct2D bitmap decode).
    fn surface_draw_image(&mut self, rect: Rect, image: &Image) -> ImagePaintResult;
}
