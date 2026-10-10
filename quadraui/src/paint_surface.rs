//! `PaintSurface` — the low-level drawing-verb trait underneath the
//! three pixel backends (issue #807, Phase 1 of the `PaintSurface`
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
//! `PaintSurface` is that shape: ~15 backend-primitive verbs, every one
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
//! `PaintSurface` instead of their own private helpers. Every existing
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
//! # Public — the unsealed paint seam
//!
//! The bulk of `Backend`'s `draw_*` methods route through
//! `primitives::<name>::native_surface_paint` helpers that take `&mut dyn
//! PaintSurface` (see each primitive module's own doc for which ones;
//! `tab_bar` is the one documented holdout), so this trait is the real
//! paint seam.
//!
//! That seam is `pub`: the ~15 verbs below are this crate's
//! public paint-primitive surface, exposed off [`crate::Backend`] via
//! [`crate::Backend::paint_surface`] — mirroring how [`crate::Backend::window`]
//! exposes [`crate::WindowControl`] and [`crate::Backend::services`] exposes
//! [`crate::PlatformServices`]. A future pixel backend (macOS/Windows today;
//! any later platform) now only needs to implement this ~15-verb trait, plus
//! `Services`/`WindowHost`, to paint every primitive whose rasteriser has
//! already migrated onto it — it does not need to reimplement any of
//! `Backend`'s 71 `draw_*`/`*_layout` methods for those primitives, since
//! the shared `native_surface_paint::paint` helpers do that work once,
//! generically, against `&mut dyn PaintSurface`.
//!
//! `Backend` itself stays sealed
//! (`sealed::Sealed`, still `pub(crate)`) — only this one piece of it is
//! unsealed.
//!
//! Not yet covered: the remaining
//! `draw_*` methods whose primitives haven't grown a `native_surface_paint`
//! module yet (`command_center`, `completions`, `editor`, `minimap`,
//! `spinner`, plus the documented `tab_bar` holdout) still paint directly,
//! and the *rest* of `Backend` (frame lifecycle, per-primitive `draw_*`/
//! `*_layout`, focus ring, modal stack) is not split into further public
//! pieces — only the paint-verb layer is.

use crate::backend::ImagePaintResult;
use crate::{Color, Image, Point, Rect, Viewport};

/// A single segment of the open or closed polyline [`PaintSurface::surface_draw_path`]
/// strokes.
///
/// Deliberately straight-segments-only — no `ArcTo`/`CurveTo` variant
/// yet. None of the overlay primitives this verb is wired up for
/// (`Palette`'s chevron, `Dialog`'s callout pointer) need a curve; a
/// future caller that does gets its own variant added the same
/// additive way every other [`PaintSurface`] verb grew bold/scale_x/role
/// parameters over time, not by generalizing this enum speculatively
/// ahead of a real caller.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum PathVerb {
    /// Start a new subpath at `Point`, with no segment drawn to it.
    MoveTo(Point),
    /// Draw a straight segment from the current point to `Point`.
    LineTo(Point),
    /// Draw a straight segment from the current point back to the most
    /// recent [`PathVerb::MoveTo`] point. A no-op if no `MoveTo` has
    /// been seen yet.
    Close,
}

/// The ~15 drawing verbs shared by every pixel backend, extracted from
/// helpers each of [`crate::gtk::backend::GtkBackend`],
/// [`crate::macos::backend::MacBackend`] and
/// [`crate::win::backend::WinBackend`] already had privately. See the
/// module doc for scope, naming, and why TUI does not implement this.
///
/// This is the public backend-paint seam (see the module doc's *Public*
/// section). A caller outside this crate can write a function generic over
/// `&mut dyn PaintSurface` the same way every `primitives::*::native_surface_paint`
/// helper in this crate already does.
pub trait PaintSurface {
    // ─── Frame + viewport ──────────────────────────────────────────────
    /// Begin a frame at `viewport`. Phase 1 implementations simply
    /// forward to [`crate::Backend::begin_frame`] — this verb exists so
    /// a future call site that only holds `&mut dyn PaintSurface` (a
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

    /// [`Self::surface_measure_text`] with an optional bold weight —
    /// added alongside [`Self::surface_draw_text_run_styled`] (#810) so a
    /// caller measuring a segment it's about to paint bold (e.g.
    /// `primitives::status_bar::native_surface_paint::paint`, #860)
    /// measures the *same* weight it renders, rather than always the
    /// regular one.
    ///
    /// Defaults to ignoring `bold` and forwarding to
    /// [`Self::surface_measure_text`] — correct only for a surface that
    /// can't tell bold and regular text apart. No implementor actually
    /// wants the full default, mirroring [`Self::surface_draw_text_run_styled`]'s
    /// per-backend split: [`crate::gtk::backend::GtkBackend`] overrides
    /// with a Pango bold `AttrList` weight; [`crate::win::backend::WinBackend`]
    /// overrides via `DWrite::measure_text_styled`;
    /// [`crate::macos::backend::MacBackend`] takes the default as-is —
    /// this backend's pre-#860 `status_bar` rasteriser measured (and
    /// rendered) every segment at the same weight regardless of `bold`
    /// (see that module's doc), so the default reproduces its existing
    /// behaviour exactly rather than granting it new bold-measurement
    /// capability.
    fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
        let _ = bold;
        self.surface_measure_text(text)
    }

    // ─── Fills + strokes ────────────────────────────────────────────────
    /// Fill `rect` with a solid `color`.
    fn surface_fill_rect(&mut self, rect: Rect, color: Color);

    /// Fill `rect` with `color`, corners rounded to `radius` — the shape
    /// every chrome background/border rasteriser not yet ported onto
    /// `PaintSurface` needs (dialogs, context menus, buttons, the
    /// bordered `ListView`/command-center panels — see
    /// `crate::gtk::rounded_rect_path`'s call sites for the current,
    /// per-backend-private equivalents) and the reason none of them
    /// could move onto this trait before now (issue #1073).
    ///
    /// `radius` is clamped to half of `rect`'s shorter side (and floored
    /// at `0.0`) by every implementation — a radius larger than that
    /// would overlap the opposite corner's arc, which every one of the
    /// three native 2D APIs this trait sits on (Cairo, CoreGraphics,
    /// Direct2D) either refuses outright or renders as an unintended
    /// lens shape rather than a rounded rectangle, and a negative radius
    /// has no rounding to give. Clamping instead of asserting keeps a
    /// caller that wants "as round as this box allows" (radius >= half
    /// the shorter side, e.g. a pill-shaped badge) a one-line call
    /// instead of its own `min()` — mirrors `radius.min(rect.width /
    /// 2.0).min(rect.height / 2.0).max(0.0)`, computed once per
    /// implementation rather than once per call site.
    ///
    /// No default: unlike the style/measurement verbs above, there is no
    /// backend-agnostic way to approximate a rounded corner out of the
    /// other verbs on this trait (`surface_fill_rect` is axis-aligned
    /// only, and there is no arc/path verb to build one from) — every
    /// implementor draws a real rounded rect, honouring `color.a` the
    /// same way [`Self::surface_fill_rect`] already does on every
    /// backend.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color);

    /// [`Self::surface_fill_rect`] with `color`'s own alpha channel
    /// overridden by `alpha` (`0.0`..=`1.0`, clamped) — the explicit,
    /// discoverable "fill translucently" entry point every large
    /// primitive not yet ported onto `PaintSurface` needs (issue
    /// #1073; see `primitives::chart`'s module doc, "The crosshair" and
    /// "The grid line color", for two spots that pre-#1073 approximated
    /// this with a CPU-side [`Color::blend`] against a hardcoded
    /// destination colour instead of a real alpha composite, because
    /// this verb didn't exist to reach for).
    ///
    /// Provided as a default rather than requiring an override: every
    /// [`Self::surface_fill_rect`] implementation on every one of the
    /// three pixel backends already honours `color.a` with a real,
    /// native alpha blend against whatever is already on the target
    /// (Cairo's `set_source_rgba`, CoreGraphics' `CGContextSetRGBFillColor`,
    /// Direct2D's `CreateSolidColorBrush` — see `crate::gtk::set_source_rgba`'s
    /// doc for the one of the three that needed a dedicated fix,
    /// issue #811, before that was true everywhere), so there is no
    /// backend-specific behaviour left to add here beyond overriding
    /// `color`'s own alpha with `alpha`. A caller that already has an
    /// alpha-carrying `Color` can call [`Self::surface_fill_rect`]
    /// directly; this exists for the (more common) case of an opaque
    /// theme colour that needs a one-off translucency at a single call
    /// site, without constructing a throwaway [`Color::with_alpha`]'d
    /// copy first.
    ///
    /// **One documented exception:** [`crate::gtk::surface::CairoSurface`]
    /// constructed with `translucent_fill: false` (`form`, `sidebar_panel`,
    /// `split`, `split_tree`'s adapters — see that module's own doc, "a
    /// deliberate, documented divergence") calls Cairo's opaque-only
    /// `set_source` from *its* [`Self::surface_fill_rect`] override, so
    /// routing this default through one of those four adapters silently
    /// paints fully opaque instead of blending, with no panic. No call
    /// site does that today (those four adapters never call
    /// `surface_fill_rect_alpha`), but a future one that does must either
    /// pre-blend the colour itself or use a `translucent_fill: true`
    /// adapter instead.
    fn surface_fill_rect_alpha(&mut self, rect: Rect, color: Color, alpha: f32) {
        self.surface_fill_rect(rect, color.with_alpha(alpha as f64));
    }

    /// Stroke the outline of `rect` in `color` at `stroke_width`. Every
    /// backend's underlying primitive insets the stroke so it lands
    /// fully inside `rect` rather than straddling its boundary — see the
    /// crate-private `win::text::stroke_rect`'s doc comment for the
    /// geometry this convention exists to keep crisp.
    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32);

    /// [`Self::surface_stroke_rect`]'s rounded-corner twin — the border
    /// every overlay styled with
    /// [`crate::Style::corner_radius`] strokes its box outline with,
    /// mirroring how [`Self::surface_fill_rounded_rect`] is
    /// [`Self::surface_fill_rect`]'s rounded-corner twin.
    ///
    /// `radius` is clamped the same way, and for the same reason, as
    /// [`Self::surface_fill_rounded_rect`]'s own doc describes. The
    /// stroke itself lands fully inside `rect`, matching
    /// [`Self::surface_stroke_rect`]'s own inset convention.
    ///
    /// No default: same reasoning as [`Self::surface_fill_rounded_rect`]
    /// — there is no backend-agnostic way to approximate a rounded
    /// stroke out of the other verbs on this trait. Every implementor
    /// strokes a real rounded-rect path rather than a plain rectangle.
    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    );

    /// Paint `text` at `rect`'s top-left corner in `color`, using this
    /// surface's current font.
    ///
    /// "This surface's current font" is an implementor's choice, not a
    /// fixed rule, for every primitive-generic adapter that paints more
    /// than one kind of primitive through the same `&mut dyn
    /// PaintSurface` (every pixel backend implementing this trait
    /// directly on itself, rather than on a role-dedicated wrapper): the
    /// convention such an adapter should follow is to default to
    /// [`crate::FontRole::Chrome`], since most primitives reaching this
    /// method are chrome, and give the few genuinely editor-class callers
    /// (see `crate::font_role::EditorClassPrimitive`) their own
    /// role-dedicated adapter instead, mirroring
    /// [`crate::macos::backend::MacBackend`]'s `ChromeSurface`/
    /// `EditorSurface` pair — see `crate::font_role`'s module doc for the
    /// fuller account of why a primitive-generic adapter defaulting to
    /// the *editor* font instead is the wrong shape.
    /// [`Self::surface_draw_text_run_with_role`] is for the rarer case of
    /// a single call site needing to choose per-call rather than
    /// per-adapter.
    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color);

    /// [`Self::surface_draw_text_run`] with optional bold/italic/underline
    /// styling and horizontal scale — added in #810 (Phase 2c)
    /// specifically so `primitives::terminal::paint`'s per-cell glyph
    /// styling (ANSI bold/italic/underline) and wide-glyph advance fix
    /// (#439/#500/#703: stretch/shrink a CJK/emoji glyph so it fills its
    /// two-column cell box exactly) don't have to regress just because
    /// this trait's original ~15 verbs (#807, Phase 1) didn't anticipate
    /// either.
    ///
    /// `scale_x` of `1.0` means "natural width, no scaling" — matches
    /// every caller that doesn't need the wide-glyph fix.
    ///
    /// Defaults to dropping `bold`/`italic`/`underline`/`scale_x`
    /// entirely and forwarding to [`Self::surface_draw_text_run`] —
    /// correct only for a surface that can style *and* scale text no
    /// better than that. No implementor actually wants the full default:
    /// [`crate::gtk::backend::GtkBackend`] overrides for all three style
    /// flags (Pango `AttrList`) plus `scale_x` (`cr.scale`);
    /// [`crate::win::backend::WinBackend`] overrides for `bold` only
    /// (`DWrite::draw_text_styled`) plus `scale_x`
    /// (`with_horizontal_scale`), silently dropping `italic`/`underline`
    /// (its own pre-#810 documented limitation);
    /// [`crate::macos::backend::MacBackend`] overrides for `scale_x`
    /// only (`draw_text_scaled_x`), dropping every style flag (its own
    /// pre-#810 documented "not rendered yet" posture) — so macOS gains
    /// no new styling capability here, but keeps the wide-glyph fix it
    /// already had.
    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let _ = (bold, italic, underline, scale_x);
        self.surface_draw_text_run(rect, text, color);
    }

    /// [`Self::surface_draw_text_run_styled`] with an explicit
    /// [`crate::FontRole`] instead of "this surface's current font" —
    /// the last prerequisite (issue #1073, alongside
    /// [`Self::surface_fill_rounded_rect`]/[`Self::surface_fill_rect_alpha`])
    /// blocking the remaining chrome paint code from moving onto this
    /// trait. Before this verb, a primitive migrated to take
    /// `&mut dyn PaintSurface` could not paint one label in the chrome
    /// font and another in the editor font from the same call site —
    /// the concrete backends resolve that today only by handing a
    /// *different, backend-specific* adapter to each caller (e.g.
    /// [`crate::macos::backend::ChromeSurface`] vs `&mut MacBackend`
    /// itself; [`crate::win::backend::WinBackend`] has an unused
    /// `chrome_dwrite` `IDWriteTextFormat` sitting idle because nothing
    /// selects it — see that field's own doc), which only works because
    /// every existing caller already knows which role it wants at
    /// construction time, not per draw call.
    ///
    /// Defaults to ignoring `role` and forwarding to
    /// [`Self::surface_draw_text_run_styled`] with `bold`/`underline`
    /// left off and `scale_x` at `1.0` — correct only for a surface that
    /// has exactly one font to offer regardless of which role is asked
    /// for (every primitive-generic adapter: [`crate::gtk::surface::CairoSurface`],
    /// [`crate::macos::surface::CgSurface`], [`crate::win::surface::D2dSurface`] —
    /// each is already constructed against one specific, caller-chosen
    /// font, so "which role" is answered before this verb is ever
    /// reached). [`crate::gtk::backend::GtkBackend`],
    /// [`crate::macos::backend::MacBackend`] and
    /// [`crate::win::backend::WinBackend`] all override this for real —
    /// see each backend's own impl for how it resolves `role` against
    /// its live chrome/editor font state.
    fn surface_draw_text_run_with_role(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        role: crate::FontRole,
        italic: bool,
    ) {
        let _ = role;
        self.surface_draw_text_run_styled(rect, text, color, false, italic, false, 1.0)
    }

    /// Paint `text` (conventionally one glyph from an icon font's
    /// Private-Use-Area range, e.g. a Nerd Font codepoint) at `rect`'s
    /// top-left corner in `color`, resolving through this surface's
    /// icon-glyph fallback family — [`crate::Backend::set_nerd_font_fallback`]
    /// (issue #929) — rather than whatever font
    /// [`Self::surface_draw_text_run`] would otherwise use alone.
    ///
    /// Defaults to forwarding straight to [`Self::surface_draw_text_run`]
    /// — correct exactly when the surface's current font already
    /// resolves the fallback family itself, which is true for every
    /// primitive-generic adapter (see [`Self::surface_draw_text_run_with_role`]'s
    /// doc: each is constructed against a caller-chosen font, and it is
    /// that constructor's responsibility to have already applied a
    /// fallback-aware description if the primitive paints icon glyphs)
    /// and, on two of the three real backends, already true of their
    /// live font state too:
    /// [`crate::macos::backend::MacBackend::set_current_font`]/`set_chrome_font`
    /// re-apply [`crate::macos::backend::MacBackend::set_nerd_font_fallback`]'s
    /// cascade to `current_font`/`chrome_font` themselves, and Win-GUI's
    /// `DWrite::new` bakes the equivalent `IDWriteFontFallback` into
    /// every `IDWriteTextFormat` it builds — so [`crate::macos::backend::MacBackend`]
    /// and [`crate::win::backend::WinBackend`] both override this
    /// verb only to document that the default already does the right
    /// thing for them, not to change its behaviour.
    /// [`crate::gtk::backend::GtkBackend`] is the one real exception: its
    /// per-frame editor `pango::Layout` (built fresh every frame from
    /// `editor_font_pango_string`, see `gtk/run.rs::render_frame`) never
    /// gets [`crate::gtk::with_nerd_font_fallback`] applied — only
    /// individual chrome call sites wrap their *own* one-off
    /// `FontDescription` with it today — so `GtkBackend` overrides this
    /// verb for real, temporarily swapping in a fallback-wrapped
    /// description for the one glyph being painted.
    fn surface_draw_icon_glyph(&mut self, rect: Rect, text: &str, color: Color) {
        self.surface_draw_text_run(rect, text, color)
    }

    /// Stroke a line segment from `from` to `to` in `color` at
    /// `stroke_width`.
    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32);

    // ─── Shadow + path ─────────────────────────────────────────────────
    /// Paint an elevation-based drop shadow behind the rounded box
    /// described by `rect`/`radius`, in `color` (typically a near-black
    /// translucent tint the caller already has to hand — this trait has
    /// no dedicated "shadow color" of its own, deliberately: see
    /// [`crate::Style`]'s module doc for why `Theme` doesn't grow a new
    /// field just for this). `elevation` is [`crate::Style::shadow_elevation`]'s
    /// own `0`–`3` scale: `0` paints nothing; `1`–`3` paint a
    /// progressively larger, softer, more vertically-offset shadow —
    /// VS Code's own hover/menu/modal elevation tiers. Values above `3`
    /// are clamped to `3`.
    ///
    /// Default: composed entirely out of [`Self::surface_fill_rounded_rect`]
    /// — three concentric translucent layers, widest-and-faintest first,
    /// narrowest-and-most-opaque last, each wider than `rect` by a
    /// `spread` that shrinks per layer while `color`'s own alpha grows,
    /// approximating a soft blur with no blur primitive at all. This is
    /// correct (not just a stand-in) for every pixel backend today:
    /// `GtkBackend`/`MacBackend`/`WinBackend` all already implement
    /// `surface_fill_rounded_rect` for real, so the composed shadow
    /// paints identically — and deliberately so, rather than each
    /// backend reaching for its own native shadow primitive (Cairo has
    /// none; CoreGraphics' `CGContextSetShadow` and Direct2D's
    /// `ID2D1Effect` blur both exist but would make the *same* elevation
    /// look different per platform, which is exactly the inconsistency
    /// a fixed VS-Code-style token set exists to avoid). No implementor
    /// overrides this.
    fn surface_draw_shadow(&mut self, rect: Rect, radius: f32, elevation: u8, color: Color) {
        let elevation = elevation.min(3);
        if elevation == 0 {
            return;
        }
        let elevation = f32::from(elevation);
        let base_alpha = color.a as f64 / 255.0;
        const LAYERS: u8 = 3;
        for layer in (1..=LAYERS).rev() {
            let t = f32::from(layer) / f32::from(LAYERS);
            let spread = elevation * t * 1.5;
            let offset_y = elevation * t * 2.0;
            let alpha = base_alpha * f64::from(1.0 - t) * 0.35 + base_alpha * 0.05;
            let shadow_rect = Rect::new(
                rect.x - spread,
                rect.y - spread + offset_y,
                rect.width + spread * 2.0,
                rect.height + spread * 2.0,
            );
            self.surface_fill_rounded_rect(shadow_rect, radius + spread, color.with_alpha(alpha));
        }
    }

    /// Stroke the open or closed polyline described by `verbs` in
    /// `color` at `stroke_width` — the shape every chevron/arrow/
    /// pointer-triangle rasteriser not yet ported onto `PaintSurface`
    /// needs (`Palette`'s dropdown chevron, `Dialog`'s callout pointer,
    /// a menu's submenu arrow), none of which are axis-aligned rects or
    /// single line segments.
    ///
    /// Default: decomposed entirely into [`Self::surface_draw_line`]
    /// calls, one per `LineTo`/`Close` verb — correct because
    /// [`PathVerb`] is deliberately straight-segments-only today (no
    /// arc verb yet; see [`PathVerb`]'s own doc for why that's a
    /// separate, later addition rather than scope creep here). No
    /// implementor overrides this.
    fn surface_draw_path(&mut self, verbs: &[PathVerb], color: Color, stroke_width: f32) {
        let mut current: Option<Point> = None;
        let mut start: Option<Point> = None;
        for verb in verbs {
            match *verb {
                PathVerb::MoveTo(p) => {
                    current = Some(p);
                    start = Some(p);
                }
                PathVerb::LineTo(p) => {
                    if let Some(from) = current {
                        self.surface_draw_line(from, p, color, stroke_width);
                    }
                    current = Some(p);
                }
                PathVerb::Close => {
                    if let (Some(from), Some(to)) = (current, start) {
                        self.surface_draw_line(from, to, color, stroke_width);
                        current = Some(to);
                    }
                }
            }
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal mock recording every call it receives — proves the
    /// *default* decompositions of [`PaintSurface::surface_draw_shadow`]
    /// and [`PaintSurface::surface_draw_path`] call the right lower-level
    /// verbs the right number of times, independent of any real pixel
    /// backend (those get their own paint-and-probe proof in
    /// `gtk::backend::tests`/`win::backend::tests`).
    #[derive(Default)]
    struct RecordingSurface {
        fill_rounded_rects: Vec<(Rect, f32, Color)>,
        lines: Vec<(Point, Point, Color, f32)>,
    }

    impl PaintSurface for RecordingSurface {
        fn surface_begin_frame(&mut self, _viewport: Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> Viewport {
            Viewport::new(0.0, 0.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            0.0
        }
        fn surface_char_width(&self) -> f32 {
            0.0
        }
        fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
            (0.0, 0.0)
        }
        fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
        fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
            self.fill_rounded_rects.push((rect, radius, color));
        }
        fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
        fn surface_stroke_rounded_rect(
            &mut self,
            _rect: Rect,
            _radius: f32,
            _color: Color,
            _stroke_width: f32,
        ) {
        }
        fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
        fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
            self.lines.push((from, to, color, stroke_width));
        }
        fn surface_push_clip(&mut self, _rect: Rect) {}
        fn surface_pop_clip(&mut self) {}
        fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
            ImagePaintResult::Unsupported
        }
    }

    /// Elevation `0` is the explicit "no shadow" tier — the default must
    /// return before calling `surface_fill_rounded_rect` at all.
    #[test]
    fn surface_draw_shadow_elevation_zero_calls_nothing() {
        let mut surface = RecordingSurface::default();
        surface.surface_draw_shadow(Rect::new(0.0, 0.0, 10.0, 10.0), 4.0, 0, Color::rgb(0, 0, 0));
        assert!(
            surface.fill_rounded_rects.is_empty(),
            "elevation 0 must not paint any shadow layer"
        );
    }

    /// A nonzero elevation paints a fixed three-layer stack, widest
    /// first — the shape the module doc promises.
    #[test]
    fn surface_draw_shadow_nonzero_elevation_paints_three_layers() {
        let mut surface = RecordingSurface::default();
        surface.surface_draw_shadow(
            Rect::new(10.0, 10.0, 20.0, 20.0),
            6.0,
            2,
            Color::rgba(0, 0, 0, 128),
        );
        assert_eq!(
            surface.fill_rounded_rects.len(),
            3,
            "every nonzero elevation paints exactly three layers"
        );
        // Widest-and-faintest layer first.
        let (first_rect, _first_radius, first_color) = surface.fill_rounded_rects[0];
        let (last_rect, _last_radius, last_color) = surface.fill_rounded_rects[2];
        assert!(
            first_rect.width > last_rect.width,
            "the first layer must be wider than the last — widest first"
        );
        assert!(
            first_color.a < last_color.a,
            "the first (widest) layer must be the faintest"
        );
    }

    /// Elevation is clamped to `3` — a caller passing e.g. `200` must
    /// paint the same three layers an elevation of exactly `3` does, not
    /// scale up without bound.
    #[test]
    fn surface_draw_shadow_clamps_elevation_above_three() {
        let mut clamped = RecordingSurface::default();
        clamped.surface_draw_shadow(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            4.0,
            200,
            Color::rgb(0, 0, 0),
        );
        let mut exact = RecordingSurface::default();
        exact.surface_draw_shadow(Rect::new(0.0, 0.0, 10.0, 10.0), 4.0, 3, Color::rgb(0, 0, 0));
        assert_eq!(clamped.fill_rounded_rects, exact.fill_rounded_rects);
    }

    /// `MoveTo`/`LineTo`/`Close` must each decompose into exactly one
    /// `surface_draw_line` call, in order, closing back to the most
    /// recent `MoveTo` point.
    #[test]
    fn surface_draw_path_decomposes_into_lines() {
        let mut surface = RecordingSurface::default();
        let a = Point::new(0.0, 0.0);
        let b = Point::new(10.0, 0.0);
        let c = Point::new(10.0, 10.0);
        surface.surface_draw_path(
            &[
                PathVerb::MoveTo(a),
                PathVerb::LineTo(b),
                PathVerb::LineTo(c),
                PathVerb::Close,
            ],
            Color::rgb(0, 255, 0),
            2.0,
        );
        assert_eq!(
            surface.lines,
            vec![
                (a, b, Color::rgb(0, 255, 0), 2.0),
                (b, c, Color::rgb(0, 255, 0), 2.0),
                (c, a, Color::rgb(0, 255, 0), 2.0),
            ],
            "MoveTo draws no segment; each LineTo draws from the current point; \
             Close draws back to the last MoveTo"
        );
    }

    /// A lone `LineTo`/`Close` with no preceding `MoveTo` has no current
    /// point to draw from, so it must be silently skipped rather than
    /// panicking.
    #[test]
    fn surface_draw_path_without_a_moveto_draws_nothing() {
        let mut surface = RecordingSurface::default();
        surface.surface_draw_path(
            &[PathVerb::LineTo(Point::new(5.0, 5.0)), PathVerb::Close],
            Color::rgb(0, 255, 0),
            1.0,
        );
        assert!(surface.lines.is_empty());
    }
}
