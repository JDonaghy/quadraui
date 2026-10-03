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
//! [`crate::native_surface`]'s module doc, "Why TUI stays out", for the
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
//! # Scope — first token only
//!
//! This is deliberately a **one-token start**, not the full tokens
//! `CLAUDE.md` eventually wants (padding, corner radius, border width,
//! per-role font size, …). [`Self::focus_ring_width`] is first because
//! it is the one geometry value all three pixel backends agree on
//! byte-for-byte, and it is painted through the shared
//! [`crate::native_surface::NativeSurface`] trait, so a single call
//! site reaches `GtkBackend`, `MacBackend` and `WinBackend` at once.
//! Each further token — padding, corner radius, border width on the
//! primitives that still hardcode them — is its own follow-up PR, one
//! token/primitive pair at a time, the same discipline
//! `docs/decisions/DECISIONS.md` already applies to the `*_layout`
//! coordinate-frame conversions (D-005) and the LOCAL→ABSOLUTE
//! migration (D-016's point 4): batching every hardcoded literal in the
//! codebase into one PR is what `PRIMITIVE_RULES.md` rule 4 forbids, at
//! a much larger scale.
//!
//! See `docs/decisions/DECISIONS.md` D-017 for the full design — token
//! catalogue, why a separate struct, and the TUI-mapping rule — this
//! module implements only the first slice of it.

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
    /// backend. Consumed via [`crate::native_surface::NativeSurface::surface_stroke_rect`]
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
}

impl Default for Style {
    fn default() -> Self {
        Self {
            focus_ring_width: 2.0,
        }
    }
}

impl Style {
    /// Set [`Self::focus_ring_width`]. The only constructor this struct
    /// needs today — every field so far has a sensible default, so
    /// there is no `Style::new(required…)` the way `TextInput::new(id)`
    /// needs one; a consumer starts from [`Style::default`] and chains
    /// `with_*` builders, e.g. `Style::default().with_focus_ring_width(4.0)`.
    #[must_use]
    pub fn with_focus_ring_width(mut self, focus_ring_width: f32) -> Self {
        self.focus_ring_width = focus_ring_width;
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
}
