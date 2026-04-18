//! # quadraui
//!
//! Cross-platform UI primitives for keyboard-driven desktop and terminal apps.
//!
//! Targets four rendering backends — Windows (Direct2D + DirectWrite),
//! Linux (GTK4 + Cairo + Pango), macOS (Core Graphics + Core Text, v1.x),
//! and TUI (ratatui + crossterm) — with a single declarative API.
//!
//! See `docs/UI_CRATE_DESIGN.md` in the vimcode repository for the full
//! design, resolved decisions, and plugin-friendly invariants.
//!
//! **Status:** Phase A.1a — `TreeView` primitive defined; TUI backend
//! landing next within this stage; GTK (A.1b) and Win-GUI (A.1c) follow.

pub mod primitives;
pub mod types;

pub use primitives::tree::{TreeEvent, TreeRow, TreeView};
pub use types::{
    Badge, Color, Decoration, Icon, Modifiers, SelectionMode, StyledSpan, StyledText, TreePath,
    TreeStyle, WidgetId,
};

/// Crate version, sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_cargo_toml() {
        assert_eq!(VERSION, "0.1.0");
    }

    #[test]
    fn color_from_hex_rgb() {
        let c = Color::from_hex("#1a2b3c").unwrap();
        assert_eq!(c, Color::rgb(0x1a, 0x2b, 0x3c));
    }

    #[test]
    fn color_from_hex_rgba() {
        let c = Color::from_hex("#1a2b3c80").unwrap();
        assert_eq!(c, Color::rgba(0x1a, 0x2b, 0x3c, 0x80));
    }

    #[test]
    fn color_from_hex_invalid() {
        assert!(Color::from_hex("not a hex").is_none());
        assert!(Color::from_hex("#xyz").is_none());
        assert!(Color::from_hex("#12345").is_none()); // wrong length
    }

    #[test]
    fn widget_id_roundtrip() {
        let id = WidgetId::new("sc-tree");
        assert_eq!(id.as_str(), "sc-tree");
        let id2: WidgetId = "sc-tree".into();
        assert_eq!(id, id2);
    }

    #[test]
    fn styled_text_visible_width() {
        let text = StyledText {
            spans: vec![
                StyledSpan::plain("hello "),
                StyledSpan::with_fg("world", Color::rgb(255, 0, 0)),
            ],
        };
        assert_eq!(text.visible_width(), 11);
    }

    #[test]
    fn tree_view_roundtrip_serde() {
        let tree = TreeView {
            id: WidgetId::new("sc"),
            rows: vec![TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("Staged Changes"),
                badge: Some(Badge::plain("3")),
                is_expanded: Some(true),
                decoration: Decoration::Normal,
            }],
            selection_mode: SelectionMode::Single,
            selected_path: Some(vec![0]),
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: true,
        };
        let json = serde_json::to_string(&tree).unwrap();
        let back: TreeView = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
    }
}
