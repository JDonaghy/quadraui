//! Accessibility data-field groundwork (#835).
//!
//! `docs/UI_CRATE_DESIGN.md` decision #6 said v1 would ship "a11y-ready
//! data fields on every primitive (`a11y_role`, `a11y_label`, focus
//! order)", with platform AT wiring (UI Automation / NSAccessibility /
//! AT-SPI) deferred to v1.1. As of 2026-09-05 neither had landed —
//! `grep -rn a11y_role quadraui/src` found nothing. This module is the
//! "data fields" half of that decision. **No AT backend integration is
//! attempted here** — that remains a separate, multi-backend,
//! months-scale programme; this is groundwork only.
//!
//! # Why this is a standalone type, not a field on every primitive
//!
//! The obvious shape — `pub a11y_role: ...` / `pub a11y_label: ...`
//! fields added directly to [`crate::TabBar`], [`crate::ActivityBar`],
//! [`crate::Spinner`], etc. — was tried first and reverted. Every
//! top-level primitive descriptor in this crate is a plain,
//! non-`#[non_exhaustive]`, all-`pub`-field struct, and **every one of
//! them is already constructed via exhaustive literals** — no
//! `..Default::default()` spread — both downstream and in this repo's
//! own sealed acceptance suite:
//!
//! - `tests/acceptance/ms-11/c0_paint_smoke.rs` (sealed — workers may
//!   not edit it, see `quadraui/tests/acceptance.rs`'s module doc)
//!   builds `StatusBar`, `TabBar`, `CommandLine`, `MessageList`,
//!   `TextDisplay`, `ListView`, `TreeView`, `PipelineView`, `DiffView`,
//!   `Tooltip`, `Spinner`, `ProgressBar`, `Scrollbar` and `DropOverlay`
//!   with full field lists. A new required field on any of those is an
//!   `error[E0063]: missing fields` there, unconditionally.
//! - `~/src/vimcode/src/{render.rs,app.rs,gtk/util.rs,core/window.rs}`
//!   and `~/src/coord-tui/src/app/{mod.rs,render.rs}` construct
//!   essentially every remaining descriptor
//!   (`ActivityBar`/`BoardCard`/`BoardColumn`/`BoardModel`/`Chart`/
//!   `CommandCenter`/`Completions`/`ContextMenu`/`DataTable`/`Dialog`/
//!   `FindReplacePanel`/`Form`/`Image`/`MenuBar`/`Minimap`/
//!   `MultiSectionView`/`Palette`/`RichTextPopup`/`SidebarPanel`/
//!   `SplitTree`/`Toolbar`/…) the same exhaustive way — confirmed by
//!   grep, zero `..Default::default()` hits in the coord-tui files that
//!   do the constructing. `vimcode` tracks this repo's `develop` tip
//!   directly (see the *Downstream consumers* section of the repo
//!   root `CLAUDE.md`), so any one of those breaks would turn its CI
//!   red on this PR's `downstream` job, not at some future pin bump.
//!
//! Per `CLAUDE.md`'s public-API rule 2, the preferred shape for
//! additive data that can't be a defaulted field on an
//! already-exhaustive struct is a **sidecar type** — exactly the
//! pattern this crate already uses three times for the identical
//! problem: [`crate::ActivityBarStyle`], [`crate::TabChrome`] and
//! [`crate::TooltipChrome`]. [`A11yInfo`] follows that precedent. It
//! is brand new, so `#[non_exhaustive]` costs nothing today, and it is
//! not wired into any primitive's struct or the `Backend` trait in
//! this change — doing that for one primitive is itself a breaking
//! change (a new required field, or a new `Backend` method parameter)
//! that needs its own one-at-a-time PR and, for widely-adopted
//! primitives, a paired `vimcode`/`coord-tui` migration, per rule 4
//! ("one breaking change per PR"). This change only lands the shared
//! vocabulary + shape so that follow-up work has something to build
//! on instead of inventing its own `A11yRole` per primitive.
//!
//! Field names deliberately match `docs/UI_CRATE_DESIGN.md`'s
//! decision #6 wording (`a11y_role`, `a11y_label`) rather than the
//! shorter `role`/`label`, so the exact audit command that flagged
//! this gap (`grep -rn a11y_role quadraui/src`) now finds a real hit.

use serde::{Deserialize, Serialize};

/// Accessibility metadata a future per-primitive field or `Backend`
/// sidecar parameter would carry: an AT role hint plus an accessible
/// name. Both are free-form strings rather than a closed enum —
/// picking a role vocabulary (ARIA-style, AccessKit's `Role`, or a
/// crate-local taxonomy) is a real design decision best made once an
/// actual AT backend is being wired up, not guessed at here.
///
/// `#[non_exhaustive]`: brand new this PR, so marking it costs no
/// consumer anything today, and it means later additions (focus
/// order, `described_by`, live-region politeness, …) are additive
/// rather than another breaking field. Construct with
/// [`A11yInfo::new`] / [`A11yInfo::default`] and the `with_*`
/// builders.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct A11yInfo {
    /// AT role hint (e.g. `"button"`, `"listitem"`, `"tab"`). `None`
    /// (the default) means "no role asserted yet" — today's behaviour
    /// for every primitive, since none carry this data at all.
    #[serde(default)]
    pub a11y_role: Option<String>,
    /// Accessible name an AT would announce for this element. `None`
    /// (the default) means "no explicit label" — a future backend
    /// would need to fall back to visible text, same as today's
    /// AT-less behaviour.
    #[serde(default)]
    pub a11y_label: Option<String>,
}

impl A11yInfo {
    /// An empty [`A11yInfo`] — equivalent to [`A11yInfo::default`],
    /// spelled as a constructor for symmetry with the `with_*`
    /// builders.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the AT role hint.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.a11y_role = Some(role.into());
        self
    }

    /// Sets the accessible label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.a11y_label = Some(label.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let info = A11yInfo::default();
        assert_eq!(info.a11y_role, None);
        assert_eq!(info.a11y_label, None);
    }

    #[test]
    fn builders_set_fields() {
        let info = A11yInfo::new().with_role("button").with_label("Save");
        assert_eq!(info.a11y_role.as_deref(), Some("button"));
        assert_eq!(info.a11y_label.as_deref(), Some("Save"));
    }

    /// `#[serde(default)]` lets old JSON (or `{}`) missing these
    /// fields still deserialize — the "no existing construction
    /// breaks" half of #835's acceptance bar, for the deserialization
    /// path this type actually supports (Rust struct-literal
    /// construction has no such escape hatch, which is exactly why
    /// this type isn't a field on an existing primitive yet — see the
    /// module doc).
    #[test]
    fn deserializes_from_empty_json() {
        let info: A11yInfo = serde_json::from_str("{}").unwrap();
        assert_eq!(info, A11yInfo::default());
    }

    #[test]
    fn round_trips_through_json() {
        let info = A11yInfo::new().with_role("tab").with_label("Untitled-1");
        let json = serde_json::to_string(&info).unwrap();
        let back: A11yInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }
}
