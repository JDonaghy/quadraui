//! [`Style`] — non-colour visual tokens (spacing, radius, border width,
//! focus-ring weight, …), the geometry half of theming that
//! [`crate::Theme`] deliberately does not cover.
//!
//! # Why this is a separate struct, not new `Theme` fields
//!
//! `Theme`'s own module doc already states the rule: adding a `pub`
//! field to `Theme` is a breaking change, because `coord-tui` builds
//! three of its four palettes with exhaustive struct literals and no
//! `..Default::default()` spread. A flat colour struct and a flat
//! geometry struct would share that exact liability, for no benefit —
//! colour and geometry are independent axes an app may want to override
//! separately (a high-contrast *colour* theme with the *same* spacing,
//! or a compact-density *geometry* preset layered under three different
//! colour themes). Keeping them as two independently defaulted types
//! means a consumer who only ever wants to touch colours — today, both
//! of them — never has to look at this file at all, and a later field
//! goes through the ordinary additive path this file sets up from its
//! first commit.
//!
//! # Where each token maps on TUI
//!
//! D-014 in `docs/decisions/DECISIONS.md` requires every new capability
//! to declare its TUI story. Every [`Style`] token's is the same shape,
//! stated once here rather than per-field: TUI paints a whole-cell
//! grid, so a sub-cell geometry token has nothing to refine — see
//! [`crate::paint_surface`]'s module doc, "Why TUI stays out", for the
//! identical reasoning applied to the drawing-verb trait this struct's
//! tokens are consumed through. Concretely:
//!
//! - **No-op** — the token has no effect because the TUI rasteriser
//!   for that primitive doesn't read [`Style`] at all. [`Self::focus_ring_width`]
//!   is this: [`crate::tui::draw_focus_ring`] always paints a 1-cell
//!   box-drawing border regardless of what a GUI backend's style carries
//!   (its own doc already says so — this struct doesn't change that).
//! - **Cell-quantised** — a future spacing/padding token would round to
//!   whole cells rather than being dropped outright, the same way
//!   `LayoutMetrics::cell_quantum` already quantises hit-test geometry
//!   (`docs/decisions/DECISIONS.md` D-016's "Paint/click drift" note).
//!   No token is shaped that way yet — see *Scope* below.
//!
//! No [`Style`] token is physically absent on TUI the way a tray icon
//! or dock badge is (D-014's **N/A** tier) — every one of them is a
//! pixel-only *refinement* with a well-defined "coarsest cell" fallback,
//! so **no-op**/**cell-quantised** cover the whole token set by
//! construction, not by accident.
//!
//! # Scope — the fixed VS Code token set
//!
//! This struct carries [`Self::focus_ring_width`] plus the rest of the
//! catalogue `CLAUDE.md` wants — padding, corner radius, border width,
//! shadow elevation and control height — as one fixed, VS-Code-shaped
//! set rather than a per-widget style sheet: every overlay primitive
//! reads the *same* five tokens, the same way every one of them reads
//! the same five-ish [`crate::Theme`] colours, instead of each carrying
//! its own bespoke radius/padding constant. [`Self::default`]'s values
//! are chosen to match VS Code's own chrome metrics (hover cards, the
//! quick-pick/command-palette widget, notifications, menus) — see each
//! field's own doc for the specific VS Code surface it mirrors.
//!
//! Each further token lands as its own additive field, the same shape
//! [`Self::focus_ring_width`] already has — `docs/decisions/DECISIONS.md`
//! D-017's one-token/primitive-at-a-time discipline applies to *adding*
//! a token, not to how many existing tokens a single consuming change
//! may read; `Toast`/`Tooltip`/`ContextMenu`/`Palette`/`Dialog`/
//! `Spinner`/`ProgressBar` all read these five together because that is
//! the one set of overlays the owner decision names, not a batch of
//! unrelated hardcoded-literal cleanups (`PRIMITIVE_RULES.md` rule 4 is
//! about unrelated changes riding one PR, not about a single design
//! decision's consistent rollout).
//!
//! See `docs/decisions/DECISIONS.md` D-017 for why this is a separate
//! struct and the TUI-mapping rule, and D-020 for the fixed token-set
//! catalogue and the verbs each token is painted through.

use serde::{Deserialize, Serialize};

/// Non-colour visual tokens shared by every pixel backend. See the
/// module doc for why this is a separate type from [`crate::Theme`],
/// which rasterisers that want styling read alongside it the same way
/// [`crate::Backend::style`] pairs with [`crate::Backend::theme`].
///
/// `#[non_exhaustive]`, from this struct's first commit: every future
/// token (padding, corner radius, border width, …) lands as a purely
/// additive field, with no retrofit later needed — the same
/// `Default`/`with_*` shape `TextInput`, `Toolbar` and `Editor` carry
/// (see [`Style::default`] and
/// [`Style::with_focus_ring_width`]). Unlike a plain struct,
/// `#[non_exhaustive]` blocks *any* struct-literal construction from
/// outside this crate — including `Style { focus_ring_width: 4.0,
/// ..Default::default() }` — so a downstream consumer always goes
/// through `Default::default()` plus a `with_*` builder, which is the
/// construction shape that keeps compiling as fields are added. Compare
/// `Theme`'s own doc, which explains why it *can't* adopt this attribute
/// while `coord-tui` builds it with exhaustive literals — `Style`
/// avoids that trap only because it is new.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Style {
    /// Stroke width, in surface-native pixels, of the focus ring
    /// [`crate::Backend::draw_focus_ring`] paints on every pixel
    /// backend. Consumed via [`crate::paint_surface::PaintSurface::surface_stroke_rect`]
    /// — see that trait's module doc for why this reaches `GtkBackend`,
    /// `MacBackend` and `WinBackend` from one call site.
    ///
    /// **TUI story: no-op.** [`crate::tui::draw_focus_ring`] always
    /// paints a 1-cell box-drawing border; there is no sub-cell stroke
    /// weight on a terminal grid to refine. See the module doc's *Where
    /// each token maps on TUI* section.
    ///
    /// Default `2.0` — the one stroke width all three pixel backends
    /// agree on, so [`Style::default()`] paints the stock focus ring.
    pub focus_ring_width: f32,

    /// Inner spacing, in surface-native pixels, between an overlay's
    /// border and its content — the gap `Toast`, `Tooltip`,
    /// `ContextMenu`, `Palette` and `Dialog` reserve on every edge
    /// before laying out their label/list/button content.
    ///
    /// **TUI story: cell-quantised.** A sub-cell padding has no grid
    /// position to paint into; a TUI rasteriser that wants to honour
    /// this token rounds it up to whole cells first (`(padding /
    /// cell_width).ceil()`), the same way `LayoutMetrics::cell_quantum`
    /// already quantises hit-test geometry. No TUI primitive reads this
    /// field yet — every one of the six overlay primitives' TUI
    /// rasterisers keeps its own pre-existing fixed cell gutter — so
    /// today this is a documented fallback rule, not yet a live call
    /// site.
    ///
    /// Default `8.0` — VS Code's own hover-card/quick-pick body padding.
    pub padding: f32,

    /// Corner radius, in surface-native pixels, every rounded-rect
    /// overlay rasterises its box with — fed to
    /// [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`]/
    /// [`crate::paint_surface::PaintSurface::surface_stroke_rounded_rect`].
    ///
    /// **TUI story: no-op.** A terminal cell grid has no sub-cell corner
    /// to round — the TUI rasteriser for every primitive this token
    /// touches keeps painting its square box-drawing border exactly as
    /// before. See the module doc's *Where each token maps on TUI*.
    ///
    /// Default `6.0` — VS Code's own widget corner radius (hover,
    /// quick-pick, notifications, context menus).
    pub corner_radius: f32,

    /// Border stroke width, in surface-native pixels, every bordered
    /// overlay strokes its box outline with — independent of
    /// [`Self::focus_ring_width`], which is specifically the *focus
    /// ring* weight, not a widget's resting border.
    ///
    /// **TUI story: no-op.** [`crate::tui::backend::TuiBackend`]'s
    /// box-drawing borders are always exactly one cell wide; there is no
    /// sub-cell stroke weight to refine.
    ///
    /// Default `1.0` — VS Code's own widget border width.
    pub border_width: f32,

    /// Drop-shadow elevation, `0`–`3`, every overlay passes to
    /// [`crate::paint_surface::PaintSurface::surface_draw_shadow`]: `0`
    /// paints no shadow at all, `1`–`3` paint a progressively larger,
    /// softer, more-offset shadow — VS Code's own hover (`1`) vs.
    /// quick-pick/menu (`2`) vs. modal dialog (`3`) elevation tiers.
    /// Values above `3` are clamped by every
    /// [`crate::paint_surface::PaintSurface::surface_draw_shadow`]
    /// implementation (see that method's doc).
    ///
    /// **TUI story: no-op.** A terminal has no pixel-level blur/offset
    /// to approximate a shadow with — `crate::tui`'s rasterisers paint
    /// no shadow at any elevation, matching
    /// [`crate::paint_surface::PaintSurface::surface_draw_shadow`]'s own
    /// module doc ("Why TUI stays out").
    ///
    /// Default `1` — the lightest tier, matching a hover-class overlay;
    /// primitives that want the heavier modal/menu tiers override it
    /// per call with [`Self::with_shadow_elevation`].
    pub shadow_elevation: u8,

    /// Height, in surface-native pixels, of a standard single-line
    /// control (a button, a text input, a list row) — the metric
    /// `Spinner`/`ProgressBar` and the other overlays size their own
    /// single-line chrome against instead of each hardcoding their own
    /// row height.
    ///
    /// **TUI story: cell-quantised.** A terminal row is always exactly
    /// one cell tall; a TUI rasteriser that wants to honour this token
    /// rounds it up to the nearest whole cell. No TUI primitive reads
    /// this field yet, for the same reason as [`Self::padding`] above.
    ///
    /// Default `26.0` — VS Code's own standard control height (input
    /// boxes, compact buttons, list rows).
    pub control_height: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            focus_ring_width: 2.0,
            padding: 8.0,
            corner_radius: 6.0,
            border_width: 1.0,
            shadow_elevation: 1,
            control_height: 26.0,
        }
    }
}

impl Style {
    /// Set [`Self::focus_ring_width`]. A consumer starts from
    /// [`Style::default`] and chains `with_*` builders, e.g.
    /// `Style::default().with_focus_ring_width(4.0)` — see the module
    /// doc for why `#[non_exhaustive]` makes this the only construction
    /// shape.
    #[must_use]
    pub fn with_focus_ring_width(mut self, focus_ring_width: f32) -> Self {
        self.focus_ring_width = focus_ring_width;
        self
    }

    /// Set [`Self::padding`].
    #[must_use]
    pub fn with_padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    /// Set [`Self::corner_radius`].
    #[must_use]
    pub fn with_corner_radius(mut self, corner_radius: f32) -> Self {
        self.corner_radius = corner_radius;
        self
    }

    /// Set [`Self::border_width`].
    #[must_use]
    pub fn with_border_width(mut self, border_width: f32) -> Self {
        self.border_width = border_width;
        self
    }

    /// Set [`Self::shadow_elevation`]. Not clamped here — every
    /// [`crate::paint_surface::PaintSurface::surface_draw_shadow`]
    /// implementation clamps its own `elevation` argument to `0..=3`
    /// (see that method's doc), so a caller that sets a larger value is
    /// harmless, just redundant.
    #[must_use]
    pub fn with_shadow_elevation(mut self, shadow_elevation: u8) -> Self {
        self.shadow_elevation = shadow_elevation;
        self
    }

    /// Set [`Self::control_height`].
    #[must_use]
    pub fn with_control_height(mut self, control_height: f32) -> Self {
        self.control_height = control_height;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the stock `2.0` stroke width every pixel backend paints, so
    /// a future edit to this default is a deliberate, reviewed visual
    /// change — not an accidental one.
    #[test]
    fn default_focus_ring_width_matches_pre_style_constant() {
        assert_eq!(Style::default().focus_ring_width, 2.0);
    }

    /// The `Default` + `with_*` construction shape the module doc
    /// promises stays compiling as fields are added — unlike a struct
    /// literal, unaffected by `#[non_exhaustive]`.
    #[test]
    fn with_focus_ring_width_builder_overrides_only_that_field() {
        let style = Style::default().with_focus_ring_width(4.0);
        assert_eq!(style.focus_ring_width, 4.0);
    }

    /// Pins the VS-Code-matching defaults for these five tokens, so a
    /// future edit to any of them is a deliberate, reviewed visual
    /// change — not an accidental one. See each field's own doc comment
    /// for which VS Code surface it mirrors.
    #[test]
    fn defaults_match_vs_code_token_set() {
        let style = Style::default();
        assert_eq!(style.padding, 8.0);
        assert_eq!(style.corner_radius, 6.0);
        assert_eq!(style.border_width, 1.0);
        assert_eq!(style.shadow_elevation, 1);
        assert_eq!(style.control_height, 26.0);
    }

    /// Every one of these `with_*` builders overrides only its own
    /// field — the same proof `with_focus_ring_width_builder_overrides_only_that_field`
    /// already gives for the original token, extended to the rest of
    /// the set so a copy/paste mistake between builders (e.g. two of
    /// them writing the same field) would fail here.
    #[test]
    fn style_token_builders_each_override_only_their_own_field() {
        let defaults = Style::default();

        let padding = defaults.with_padding(20.0);
        assert_eq!(padding.padding, 20.0);
        assert_eq!(padding.corner_radius, defaults.corner_radius);

        let corner_radius = defaults.with_corner_radius(12.0);
        assert_eq!(corner_radius.corner_radius, 12.0);
        assert_eq!(corner_radius.border_width, defaults.border_width);

        let border_width = defaults.with_border_width(3.0);
        assert_eq!(border_width.border_width, 3.0);
        assert_eq!(border_width.shadow_elevation, defaults.shadow_elevation);

        let shadow_elevation = defaults.with_shadow_elevation(3);
        assert_eq!(shadow_elevation.shadow_elevation, 3);
        assert_eq!(shadow_elevation.control_height, defaults.control_height);

        let control_height = defaults.with_control_height(32.0);
        assert_eq!(control_height.control_height, 32.0);
        assert_eq!(control_height.focus_ring_width, defaults.focus_ring_width);
    }
}
