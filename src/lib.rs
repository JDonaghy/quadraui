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
pub use primitives::list::{ListItem, ListView, ListViewEvent};
pub use primitives::palette::{Palette, PaletteEvent, PaletteItem};
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
                        cursor: Some(1),
                        selection_anchor: None,
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

    #[test]
    fn list_view_roundtrip_serde() {
        let list = ListView {
            id: WidgetId::new("quickfix"),
            title: Some(StyledText::plain("QUICKFIX (3 items)")),
            items: vec![
                ListItem {
                    text: StyledText::plain("src/main.rs:12: unused variable"),
                    icon: None,
                    detail: None,
                    decoration: Decoration::Warning,
                },
                ListItem {
                    text: StyledText::plain("src/lib.rs:4: missing import"),
                    icon: None,
                    detail: Some(StyledText::plain("E0425")),
                    decoration: Decoration::Error,
                },
            ],
            selected_idx: 1,
            scroll_offset: 0,
            has_focus: true,
        };
        let json = serde_json::to_string(&list).unwrap();
        let back: ListView = serde_json::from_str(&json).unwrap();
        assert_eq!(list, back);
    }

    #[test]
    fn palette_roundtrip_serde() {
        let palette = Palette {
            id: WidgetId::new("cmd-palette"),
            title: "Commands".to_string(),
            query: "open".to_string(),
            query_cursor: 4,
            items: vec![
                PaletteItem {
                    text: StyledText::plain("Open File"),
                    detail: Some(StyledText::plain("Ctrl+O")),
                    icon: None,
                    match_positions: vec![0, 1, 2, 3],
                },
                PaletteItem {
                    text: StyledText::plain("Open Recent"),
                    detail: None,
                    icon: None,
                    match_positions: vec![0, 1, 2, 3],
                },
            ],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 42,
            has_focus: true,
        };
        let json = serde_json::to_string(&palette).unwrap();
        let back: Palette = serde_json::from_str(&json).unwrap();
        assert_eq!(palette, back);
    }

    #[test]
    fn palette_event_roundtrip_serde() {
        let events = vec![
            PaletteEvent::QueryChanged {
                value: "foo".to_string(),
            },
            PaletteEvent::SelectionChanged { idx: 3 },
            PaletteEvent::ItemConfirmed { idx: 0 },
            PaletteEvent::Closed,
            PaletteEvent::KeyPressed {
                key: "Ctrl+P".to_string(),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: PaletteEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    #[test]
    fn text_input_cursor_and_selection_serde() {
        // Round-trip a TextInput variant with explicit cursor + selection state.
        let field = FormField {
            id: WidgetId::new("name"),
            label: StyledText::plain("Name"),
            kind: FieldKind::TextInput {
                value: "hello world".to_string(),
                placeholder: String::new(),
                cursor: Some(5),
                selection_anchor: Some(0),
            },
            hint: StyledText::default(),
            disabled: false,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);

        // Legacy shape without cursor/selection_anchor also deserializes
        // (new fields default to None) — ensures the extension is
        // backward-compatible with pre-A.3d serialised forms.
        let legacy = r#"{
            "id": "legacy",
            "label": {"spans":[{"text":"Legacy","fg":null,"bg":null}]},
            "kind": {"TextInput": {"value": "x"}},
            "hint": {"spans":[]}
        }"#;
        let parsed: FormField = serde_json::from_str(legacy).unwrap();
        match parsed.kind {
            FieldKind::TextInput {
                cursor,
                selection_anchor,
                ..
            } => {
                assert_eq!(cursor, None);
                assert_eq!(selection_anchor, None);
            }
            other => panic!("unexpected kind: {:?}", other),
        }
    }
}
