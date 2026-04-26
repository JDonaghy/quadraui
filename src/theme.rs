//! Backend-agnostic [`Theme`] struct consumed by the per-backend rasterisers
//! in [`crate::tui`] and [`crate::gtk`].
//!
//! `Theme` is intentionally small. Apps with rich theme systems (vimcode's
//! `render::Theme` carries dozens of LSP / git / markdown colours; kubeui
//! has its own palette) build a `quadraui::Theme` at the call site by
//! picking the relevant fields out of their app-specific theme. Adding a
//! field here means every primitive rasteriser can read it; field count
//! grows as more primitives migrate from vimcode-private rasterisers into
//! `quadraui::tui` / `quadraui::gtk`.
//!
//! This is the **first** field set — driven by the StatusBar pilot
//! (#223). Future migrations (TabBar, ListView, TreeView, …) will
//! extend the struct.

use crate::types::Color;
use serde::{Deserialize, Serialize};

/// Minimal backend-agnostic colour palette consumed by the public
/// `quadraui::tui` / `quadraui::gtk` rasterisers.
///
/// Apps that want the rasterisers to draw with their own colours
/// build a `Theme` at the call site (vimcode does this from
/// `render::Theme`; kubeui does it from its own palette). All fields
/// are required so every rasteriser has a reasonable fallback for
/// regions a primitive doesn't fully cover (e.g. the `StatusBar`
/// background fill when no segments are present).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    /// Default surface background. Used as a fallback fill when the
    /// primitive has no opinion (e.g. an empty `StatusBar`).
    pub background: Color,
    /// Default surface foreground. Available for primitives that need
    /// a generic text colour; not yet consumed by `StatusBar` (every
    /// segment carries its own `fg`).
    pub foreground: Color,
}

impl Default for Theme {
    /// Neutral dark palette so the rasterisers produce something visible
    /// when an app forgets to populate the theme. Apps almost always
    /// override this.
    fn default() -> Self {
        Self {
            background: Color::rgb(20, 22, 30),
            foreground: Color::rgb(220, 220, 220),
        }
    }
}
