//! # quadraui
//!
//! Cross-platform UI primitives for keyboard-driven desktop and terminal apps.
//!
//! Targets four rendering backends — Windows (Direct2D + DirectWrite),
//! Linux (GTK4 + Cairo + Pango), macOS (Core Graphics + Core Text, v1.x),
//! and TUI (ratatui + crossterm) — with a single declarative API.
//!
//! See `docs/UI_CRATE_DESIGN.md` in the vimcode repository for the full design.
//!
//! **Status:** Phase A.0 — workspace scaffold. No primitives yet.
//! Primitives begin landing in Phase A.1 (`TreeView` for the source-control panel).

/// Crate version, sourced from `Cargo.toml`.
///
/// Used by dependents as a link-time smoke test until real primitives land.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_cargo_toml() {
        assert_eq!(VERSION, "0.1.0");
    }
}
