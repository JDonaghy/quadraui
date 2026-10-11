//! Accessibility data-field groundwork.
//!
//! This module provides the data-field half of the accessibility plan:
//! `a11y_role` and `a11y_label` on every primitive. AT backend integration
//! (UI Automation / NSAccessibility / AT-SPI) is deferred and separate.
//!
//! # Why this is a standalone type, not a field on every primitive
//!
//! Adding new required fields directly to [`crate::TabBar`],
//! [`crate::ActivityBar`], [`crate::Spinner`], etc. would break every
//! call site: these are exhaustive structs with all-`pub` fields built
//! via exhaustive literals without `..Default::default()` spread.
//!
//! The preferred shape for additive data that cannot be a defaulted field
//! is a **sidecar type** — the same pattern this crate uses for
//! [`crate::ActivityBarStyle`], [`crate::TabChrome`], and
//! [`crate::TooltipChrome`]. [`A11yInfo`] follows that precedent.
//! It is marked `#[non_exhaustive]` so later additions (focus order,
//! `described_by`, live-region politeness) remain additive rather than
//! breaking. It is not yet wired into any primitive's struct or the
//! `Backend` trait — each integration is a breaking change requiring
//! its own PR and migration.
//!
//! Field names match `docs/UI_CRATE_DESIGN.md` decision #6 (`a11y_role`,
//! `a11y_label`) rather than the shorter `role`/`label`, so the audit
//! command that identified this gap (`grep -rn a11y_role quadraui/src`)
//! now finds hits.

use serde::{Deserialize, Serialize};

/// Accessibility metadata a future per-primitive field or `Backend`
/// sidecar parameter would carry: an AT role hint plus an accessible
/// name. Both are free-form strings rather than a closed enum —
/// picking a role vocabulary (ARIA-style, AccessKit's `Role`, or a
/// crate-local taxonomy) will be chosen when an AT backend is wired up.
///
/// `#[non_exhaustive]`: later additions (focus order, `described_by`,
/// live-region politeness) are additive rather than breaking. Construct
/// with [`A11yInfo::new`] / [`A11yInfo::default`] and the `with_*`
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

    /// `#[serde(default)]` allows JSON missing these fields (or `{}`) to
    /// deserialize successfully, preserving backward compatibility with
    /// data serialized before these fields existed.
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
