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

pub use primitives::form::{FieldKind, Form, FormEvent, FormField};
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

    #[test]
    fn form_roundtrip_serde() {
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![
                FormField {
                    id: WidgetId::new("header"),
                    label: StyledText::plain("Editor"),
                    kind: FieldKind::Label,
                    hint: StyledText::default(),
                    disabled: false,
                },
                FormField {
                    id: WidgetId::new("line-numbers"),
                    label: StyledText::plain("Show line numbers"),
                    kind: FieldKind::Toggle { value: true },
                    hint: StyledText::default(),
                    disabled: false,
                },
                FormField {
                    id: WidgetId::new("tabstop"),
                    label: StyledText::plain("Tab width"),
                    kind: FieldKind::TextInput {
                        value: "4".to_string(),
                        placeholder: "2".to_string(),
                    },
                    hint: StyledText::plain("Number of spaces per tab"),
                    disabled: false,
                },
                FormField {
                    id: WidgetId::new("save"),
                    label: StyledText::plain("Save settings"),
                    kind: FieldKind::Button,
                    hint: StyledText::default(),
                    disabled: false,
                },
            ],
            focused_field: Some(WidgetId::new("line-numbers")),
            scroll_offset: 0,
            has_focus: true,
        };
        let json = serde_json::to_string(&form).unwrap();
        let back: Form = serde_json::from_str(&json).unwrap();
        assert_eq!(form, back);
    }

    #[test]
    fn form_event_roundtrip_serde() {
        let events = vec![
            FormEvent::ToggleChanged {
                id: WidgetId::new("line-numbers"),
                value: false,
            },
            FormEvent::TextInputChanged {
                id: WidgetId::new("tabstop"),
                value: "8".to_string(),
            },
            FormEvent::TextInputCommitted {
                id: WidgetId::new("tabstop"),
                value: "8".to_string(),
            },
            FormEvent::FocusChanged {
                id: WidgetId::new("save"),
            },
            FormEvent::ButtonClicked {
                id: WidgetId::new("save"),
            },
            FormEvent::KeyPressed {
                key: "Escape".to_string(),
                modifiers: Modifiers::default(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: FormEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }
}
