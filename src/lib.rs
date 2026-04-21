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

pub use primitives::activity_bar::{ActivityBar, ActivityBarEvent, ActivityItem};
pub use primitives::form::{FieldKind, Form, FormEvent, FormField};
pub use primitives::list::{ListItem, ListView, ListViewEvent};
pub use primitives::palette::{Palette, PaletteEvent, PaletteItem};
pub use primitives::status_bar::{StatusBar, StatusBarEvent, StatusBarHitRegion, StatusBarSegment};
pub use primitives::tab_bar::{TabBar, TabBarEvent, TabBarSegment, TabItem};
pub use primitives::terminal::{Terminal, TerminalCell, TerminalEvent};
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
    fn status_bar_roundtrip_serde() {
        use primitives::status_bar::StatusBarSegment;
        let bar = StatusBar {
            id: WidgetId::new("editor-status"),
            left_segments: vec![
                StatusBarSegment {
                    text: " NORMAL ".to_string(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(30, 30, 30),
                    bold: true,
                    action_id: None,
                },
                StatusBarSegment {
                    text: " main.rs".to_string(),
                    fg: Color::rgb(200, 200, 200),
                    bg: Color::rgb(30, 30, 30),
                    bold: true,
                    action_id: None,
                },
            ],
            right_segments: vec![
                StatusBarSegment {
                    text: " rust ".to_string(),
                    fg: Color::rgb(200, 200, 200),
                    bg: Color::rgb(30, 30, 30),
                    bold: false,
                    action_id: Some(WidgetId::new("status:change_language")),
                },
                StatusBarSegment {
                    text: " Ln 12, Col 4 ".to_string(),
                    fg: Color::rgb(200, 200, 200),
                    bg: Color::rgb(30, 30, 30),
                    bold: false,
                    action_id: Some(WidgetId::new("status:goto_line")),
                },
            ],
        };
        let json = serde_json::to_string(&bar).unwrap();
        let back: StatusBar = serde_json::from_str(&json).unwrap();
        assert_eq!(bar, back);
    }

    #[test]
    fn status_bar_hit_regions() {
        use primitives::status_bar::StatusBarSegment;
        // Bar width 30: left " LEFT " (6 chars, clickable "left") +
        // right " R " (3 chars, clickable "right") right-aligned at col 27.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![StatusBarSegment {
                text: " LEFT ".to_string(),
                fg: Color::rgb(0, 0, 0),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                action_id: Some(WidgetId::new("left")),
            }],
            right_segments: vec![StatusBarSegment {
                text: " R ".to_string(),
                fg: Color::rgb(0, 0, 0),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                action_id: Some(WidgetId::new("right")),
            }],
        };
        let regions = bar.hit_regions(30);
        assert_eq!(regions.len(), 2);
        // Left starts at col 0, width 6
        assert_eq!(regions[0].col, 0);
        assert_eq!(regions[0].width, 6);
        assert_eq!(regions[0].id.as_str(), "left");
        // Right starts at col 27, width 3
        assert_eq!(regions[1].col, 27);
        assert_eq!(regions[1].width, 3);
        assert_eq!(regions[1].id.as_str(), "right");

        // Click resolution
        assert_eq!(
            bar.resolve_click(3, 30).as_ref().map(|w| w.as_str()),
            Some("left")
        );
        assert_eq!(
            bar.resolve_click(28, 30).as_ref().map(|w| w.as_str()),
            Some("right")
        );
        assert_eq!(bar.resolve_click(15, 30), None); // gap between segments
    }

    #[test]
    fn terminal_roundtrip_serde() {
        use primitives::terminal::TerminalCell;
        let term = Terminal {
            id: WidgetId::new("terminal-0"),
            cells: vec![
                vec![
                    TerminalCell {
                        ch: '$',
                        fg: Color::rgb(200, 200, 200),
                        bg: Color::rgb(20, 20, 20),
                        bold: true,
                        italic: false,
                        underline: false,
                        selected: false,
                        is_cursor: false,
                        is_find_match: false,
                        is_find_active: false,
                    },
                    TerminalCell {
                        ch: ' ',
                        fg: Color::rgb(200, 200, 200),
                        bg: Color::rgb(20, 20, 20),
                        bold: false,
                        italic: false,
                        underline: false,
                        selected: false,
                        is_cursor: true,
                        is_find_match: false,
                        is_find_active: false,
                    },
                ],
                vec![TerminalCell {
                    ch: 'm',
                    fg: Color::rgb(255, 100, 50),
                    bg: Color::rgb(20, 20, 20),
                    bold: false,
                    italic: false,
                    underline: true,
                    selected: true,
                    is_cursor: false,
                    is_find_match: true,
                    is_find_active: false,
                }],
            ],
        };
        let json = serde_json::to_string(&term).unwrap();
        let back: Terminal = serde_json::from_str(&json).unwrap();
        assert_eq!(term, back);
    }

    #[test]
    fn terminal_event_roundtrip_serde() {
        let events = vec![
            TerminalEvent::KeyPressed {
                key: "a".to_string(),
                modifiers: Modifiers::default(),
            },
            TerminalEvent::SelectStart { row: 5, col: 10 },
            TerminalEvent::SelectExtend { row: 6, col: 20 },
            TerminalEvent::SelectEnd,
            TerminalEvent::Scroll { delta: -3 },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: TerminalEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    #[test]
    fn activity_bar_roundtrip_serde() {
        use primitives::activity_bar::ActivityItem;
        let bar = ActivityBar {
            id: WidgetId::new("main-activity-bar"),
            top_items: vec![
                ActivityItem {
                    id: WidgetId::new("activity:explorer"),
                    icon: "\u{f07c}".to_string(),
                    tooltip: "Explorer".to_string(),
                    is_active: true,
                    is_keyboard_selected: false,
                },
                ActivityItem {
                    id: WidgetId::new("activity:search"),
                    icon: "\u{f422}".to_string(),
                    tooltip: "Search".to_string(),
                    is_active: false,
                    is_keyboard_selected: true,
                },
            ],
            bottom_items: vec![ActivityItem {
                id: WidgetId::new("activity:settings"),
                icon: "\u{f013}".to_string(),
                tooltip: "Settings".to_string(),
                is_active: false,
                is_keyboard_selected: false,
            }],
            active_accent: Some(Color::rgb(120, 180, 255)),
            selection_bg: Some(Color::rgb(80, 80, 80)),
        };
        let json = serde_json::to_string(&bar).unwrap();
        let back: ActivityBar = serde_json::from_str(&json).unwrap();
        assert_eq!(bar, back);
    }

    #[test]
    fn activity_bar_event_roundtrip_serde() {
        let events = vec![
            ActivityBarEvent::ItemClicked {
                id: WidgetId::new("activity:git"),
            },
            ActivityBarEvent::KeyPressed {
                key: "Escape".to_string(),
                modifiers: Modifiers::default(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: ActivityBarEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    #[test]
    fn tab_bar_roundtrip_serde() {
        use primitives::tab_bar::{TabBarSegment, TabItem};
        let bar = TabBar {
            id: WidgetId::new("group-0-tabs"),
            tabs: vec![
                TabItem {
                    label: " 1: main.rs ".to_string(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                },
                TabItem {
                    label: " 2: lib.rs ".to_string(),
                    is_active: false,
                    is_dirty: true,
                    is_preview: false,
                },
                TabItem {
                    label: " 3: render.rs ".to_string(),
                    is_active: false,
                    is_dirty: false,
                    is_preview: true,
                },
            ],
            scroll_offset: 0,
            right_segments: vec![
                TabBarSegment {
                    text: "2 of 5 ".to_string(),
                    width_cells: 7,
                    id: None,
                    is_active: false,
                },
                TabBarSegment {
                    text: " ← ".to_string(),
                    width_cells: 3,
                    id: Some(WidgetId::new("tab:diff_prev")),
                    is_active: false,
                },
                TabBarSegment {
                    text: " … ".to_string(),
                    width_cells: 3,
                    id: Some(WidgetId::new("tab:action_menu")),
                    is_active: false,
                },
            ],
            active_accent: Some(Color::rgb(100, 200, 255)),
        };
        let json = serde_json::to_string(&bar).unwrap();
        let back: TabBar = serde_json::from_str(&json).unwrap();
        assert_eq!(bar, back);
    }

    #[test]
    fn tab_bar_event_roundtrip_serde() {
        let events = vec![
            TabBarEvent::TabActivated { index: 2 },
            TabBarEvent::TabClosed { index: 0 },
            TabBarEvent::ButtonClicked {
                id: WidgetId::new("tab:split_right"),
            },
            TabBarEvent::KeyPressed {
                key: "F1".to_string(),
                modifiers: Modifiers::default(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: TabBarEvent = serde_json::from_str(&json).unwrap();
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
