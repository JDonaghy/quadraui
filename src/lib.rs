//! # quadraui
//!
//! Cross-platform UI primitives for keyboard-driven desktop and terminal apps.
//!
//! Targets four rendering backends with a single declarative API:
//! - **Windows** (Direct2D + DirectWrite) — `windows-rs`
//! - **Linux** (GTK4 + Cairo + Pango) — `gtk4`
//! - **macOS** (Core Graphics + Core Text) — *planned, v1.x*
//! - **TUI** (ratatui + crossterm) — works everywhere as a fallback
//!
//! ## What's in the box
//!
//! Nine primitives, each declarative + serde-friendly so apps and Lua
//! plugins can describe UI as data:
//!
//! | Primitive | Use for |
//! |-----------|---------|
//! | [`TreeView`] | File explorers, source-control panels, hierarchical lists |
//! | [`ListView`] | Quickfix, search results, flat selectable lists |
//! | [`Form`] | Settings panels, config editors |
//! | [`Palette`] | Command palettes, fuzzy pickers |
//! | [`StatusBar`] | Mode/file/cursor strips, footer bars |
//! | [`TabBar`] | Editor tabs, document tabs |
//! | [`ActivityBar`] | Vertical icon strips (VSCode-style) |
//! | [`Terminal`] | Cell grids for terminal emulators |
//! | [`TextDisplay`] | Streaming logs, AI chat output |
//!
//! ## How it works
//!
//! Apps build primitive descriptions from their own state. Backends consume
//! the descriptions and rasterise them. Events flow back as `*Event` enums
//! that reference primitives by [`WidgetId`] (owned strings — plugin-safe,
//! no `&'static str`).
//!
//! ```ignore
//! // App (your code) — state-driven, declarative:
//! let bar = quadraui::StatusBar {
//!     id: WidgetId::new("status:editor"),
//!     left_segments: vec![mode_segment(), filename_segment()],
//!     right_segments: vec![lsp_segment(), cursor_segment()],
//! };
//!
//! // Backend (yours or one of the existing ones) — measure + paint:
//! draw_status_bar(cr, &bar, &theme);
//! ```
//!
//! ## Documentation
//!
//! - **`README.md`** (in this crate) — quick start, primitive guide.
//! - **`BACKEND.md`** — implementing a new render backend: mental
//!   model, the three contracts (owned data, measurer-parameterised
//!   algorithms, per-primitive contracts), two-pass paint pattern,
//!   click-intercept hierarchy, implementer checklist.
//! - **`examples/tui_demo.rs`** — runnable ratatui example that
//!   exercises the TabBar + StatusBar contracts end-to-end (cell
//!   units). `cargo run --example tui_demo`.
//! - **`examples/gtk_demo.rs`** — same demo rendered with GTK4 +
//!   Cairo + Pango (pixel units, two-pass paint). Requires the
//!   `gtk-example` feature: `cargo run --example gtk_demo
//!   --features gtk-example`.
//! - **`docs/UI_CRATE_DESIGN.md`** — full design rationale and the §10
//!   plugin invariants every primitive must honour.
//! - **`docs/DECISIONS.md`** — running log of API decisions
//!   (which primitives, why this shape, what was deferred).
//!
//! ## Status
//!
//! Pre-1.0 (`v0.1.x`). API will stabilise before publishing to crates.io.
//! All nine primitives shipped; the TUI and GTK backends are battle-tested
//! by vimcode (5000+ tests), the Win-GUI backend ships SC + explorer panel
//! migrations and is queued for tab/status/activity bar parity. macOS is
//! v1.x.
//!
//! ## Plugin invariants (briefly)
//!
//! From `docs/UI_CRATE_DESIGN.md` §10 — applies to every primitive:
//! 1. [`WidgetId`] is owned (`String`) — not `&'static str`.
//! 2. Events are plain data — no Rust closures.
//! 3. Primitives implement `Serialize + Deserialize` — Lua tables map via JSON.
//! 4. WidgetIds are namespaced (e.g. `"plugin:my-ext:send"`).
//! 5. No global event handlers — every event references a `WidgetId`.
//! 6. Primitives don't borrow app state — owned data or explicit `'a`.
//!
//! Verify all six when adding a new primitive or extending an existing one.

pub mod primitives;
pub mod types;

// ── Phase B.1: Backend trait + UiEvent + Accelerator ────────────────────────
// See quadraui/docs/BACKEND_TRAIT_PROPOSAL.md for design. These modules add
// the unified cross-backend surface alongside the existing per-backend
// free-function draw pattern; no migration yet (that's Phase B.2).
pub mod accelerator;
pub mod backend;
pub mod event;

// ── Phase B.4: cross-backend event routing ──────────────────────────────────
// ModalStack + dispatch free functions. Backends hold one ModalStack and
// call into dispatch to translate raw mouse events into Vec<UiEvent>
// without each backend reimplementing modal-precedence / backdrop-dismiss.
pub mod dispatch;
pub mod modal_stack;

pub use primitives::activity_bar::{
    ActivityBar, ActivityBarEvent, ActivityBarHit, ActivityBarLayout, ActivityItem, ActivitySide,
    VisibleActivityItem,
};
pub use primitives::completions::{
    CompletionItem, CompletionItemMeasure, CompletionKind, Completions, CompletionsEvent,
    CompletionsHit, CompletionsLayout, CompletionsPlacement, VisibleCompletion,
};
pub use primitives::context_menu::{
    ContextMenu, ContextMenuEvent, ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure,
    ContextMenuLayout, VisibleContextMenuItem,
};
pub use primitives::dialog::{
    Dialog, DialogButton, DialogEvent, DialogHit, DialogInput, DialogLayout, DialogMeasure,
    DialogSeverity, VisibleDialogButton,
};
pub use primitives::form::{
    FieldKind, Form, FormEvent, FormField, FormFieldMeasure, FormHit, FormLayout, VisibleFormField,
};
pub use primitives::list::{
    ListItem, ListItemMeasure, ListView, ListViewEvent, ListViewHit, ListViewLayout,
    VisibleListItem,
};
pub use primitives::menu_bar::{
    MenuBar, MenuBarEvent, MenuBarHit, MenuBarItem, MenuBarItemMeasure, MenuBarLayout,
    VisibleMenuBarItem,
};
pub use primitives::modal::{Modal, ModalEvent, ModalHit, ModalLayout};
pub use primitives::palette::{
    Palette, PaletteEvent, PaletteHit, PaletteItem, PaletteItemMeasure, PaletteLayout,
    VisiblePaletteItem,
};
pub use primitives::panel::{
    Panel, PanelAction, PanelEvent, PanelHit, PanelLayout, PanelMeasure, VisiblePanelAction,
};
pub use primitives::progress::{
    ProgressBar, ProgressBarEvent, ProgressBarHit, ProgressBarLayout, ProgressBarMeasure,
};
pub use primitives::rich_text_popup::{
    PopupPlacement, PopupScrollbar, RichTextLink, RichTextPopup, RichTextPopupEvent,
    RichTextPopupHit, RichTextPopupLayout, RichTextPopupMeasure, TextSelection,
    VisibleRichTextLine,
};
pub use primitives::spinner::{Spinner, SpinnerEvent, SpinnerHit, SpinnerLayout, SpinnerMeasure};
pub use primitives::split::{
    Split, SplitDirection, SplitEvent, SplitHit, SplitLayout, SplitMeasure,
};
pub use primitives::status_bar::{
    StatusBar, StatusBarEvent, StatusBarHit, StatusBarHitRegion, StatusBarLayout, StatusBarSegment,
    StatusSegmentMeasure, StatusSegmentSide, VisibleStatusSegment,
};
pub use primitives::tab_bar::{
    SegmentMeasure, TabBar, TabBarEvent, TabBarHit, TabBarLayout, TabBarSegment, TabItem,
    TabMeasure, VisibleSegment, VisibleTab,
};
pub use primitives::terminal::{
    Terminal, TerminalCell, TerminalCellSize, TerminalEvent, TerminalHit, TerminalLayout,
};
pub use primitives::text_display::{
    TextDisplay, TextDisplayEvent, TextDisplayHit, TextDisplayLayout, TextDisplayLine,
    TextDisplayLineMeasure, VisibleTextDisplayLine,
};
pub use primitives::toast::{
    ToastAction, ToastCorner, ToastEvent, ToastHit, ToastItem, ToastMeasure, ToastSeverity,
    ToastStack, ToastStackLayout, VisibleToast,
};
pub use primitives::tooltip::{
    ResolvedPlacement, Tooltip, TooltipEvent, TooltipHit, TooltipLayout, TooltipMeasure,
    TooltipPlacement,
};
pub use primitives::tree::{
    TreeEvent, TreeRow, TreeRowMeasure, TreeView, TreeViewHit, TreeViewLayout, VisibleTreeRow,
};
pub use types::{
    Badge, Color, Decoration, Icon, Modifiers, SelectionMode, StyledSpan, StyledText, TreePath,
    TreeStyle, WidgetId,
};

// Phase B.1 re-exports.
pub use accelerator::{
    parse_key_binding, render_accelerator, render_binding, Accelerator, AcceleratorId,
    AcceleratorScope, KeyBinding, ParsedBinding, Platform,
};
pub use backend::{Backend, Clipboard, FileDialogOptions, Notification, PlatformServices};
pub use event::{
    BackendNativeEvent, ButtonMask, Key, MouseButton, NamedKey, Point, Rect, ScrollDelta, UiEvent,
    Viewport,
};

// Phase B.4 re-exports.
pub use dispatch::{
    dispatch_mouse_down, dispatch_mouse_drag, dispatch_mouse_up, DragState, DragTarget,
};
pub use modal_stack::{ModalEntry, ModalStack};

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
            bordered: false,
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
    fn status_bar_fit_right_start_chars() {
        use primitives::status_bar::StatusBarSegment;
        let mk = |text: &str, id: &str| StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(0, 0, 0),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            action_id: Some(WidgetId::new(id)),
        };
        // Left 5 chars, right = 4 low-priority (lo0..lo3) + cursor (always kept).
        // Right segments total: 3+3+3+3+11 = 23 chars
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![mk(" LEFT", "left")],
            right_segments: vec![
                mk(" a ", "lo0"),
                mk(" b ", "lo1"),
                mk(" c ", "lo2"),
                mk(" d ", "lo3"),
                mk(" Ln 1,Col 1", "cursor"),
            ],
        };

        // Plenty of room (left 5 + gap 2 + right 23 = 30 <= 40) → nothing dropped.
        assert_eq!(bar.fit_right_start_chars(40, 2), 0);

        // Exact fit (30) → still 0 dropped.
        assert_eq!(bar.fit_right_start_chars(30, 2), 0);

        // bar_width 29: need max_right = 29 - 5 - 2 = 22. Total 23 > 22, drop lo0 (3).
        // After dropping lo0, remaining = 20 <= 22, keep rest.
        assert_eq!(bar.fit_right_start_chars(29, 2), 1);

        // bar_width 20: max_right = 13. Must drop lo0(3), lo1(3), lo2(3), lo3(3)
        // → remaining = 11 <= 13. Keep only cursor.
        assert_eq!(bar.fit_right_start_chars(20, 2), 4);

        // Tiny bar: left(5)+gap(2)=7 already >= bar. max_right=0. Even cursor
        // (11) doesn't fit — but we always keep the last segment.
        assert_eq!(bar.fit_right_start_chars(5, 2), 4);

        // Empty right side.
        let empty_right = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![mk(" X", "x")],
            right_segments: vec![],
        };
        assert_eq!(empty_right.fit_right_start_chars(10, 2), 0);
    }

    #[test]
    fn status_bar_fit_right_start_generic_pixel_measurer() {
        // Proves the fit algorithm is unit-agnostic: a backend can supply
        // its own measurer (e.g. Pango pixel widths for GTK) and the same
        // drop-by-priority logic applies. Each char here = 10 "px".
        use primitives::status_bar::StatusBarSegment;
        let mk = |text: &str, id: &str| StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(0, 0, 0),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            action_id: Some(WidgetId::new(id)),
        };
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![mk("LL", "left")], // 20 px
            right_segments: vec![
                mk("aaa", "lo"),    // 30 px (lowest priority)
                mk("bbbb", "mid"),  // 40 px
                mk("cursor", "hi"), // 60 px (highest priority)
            ],
        };
        let measure_px = |seg: &StatusBarSegment| seg.text.chars().count() * 10;

        // 200 px: 20 + 16 (gap) + 130 = 166 <= 200, no drop.
        assert_eq!(bar.fit_right_start(200, 16, measure_px), 0);

        // 150 px: 20 + 16 + 130 = 166 > 150. Drop "aaa" (30): 20+16+100=136 <= 150.
        assert_eq!(bar.fit_right_start(150, 16, measure_px), 1);

        // 100 px: drop "aaa" (30) + "bbbb" (40), keep "cursor": 20+16+60=96 <= 100.
        assert_eq!(bar.fit_right_start(100, 16, measure_px), 2);

        // 30 px: even cursor doesn't fit alone, but algorithm always keeps last.
        assert_eq!(bar.fit_right_start(30, 16, measure_px), 2);

        // Bold-aware: a measurer that adds 5 px for bold segments yields a
        // different fit. Verifies the closure can vary by segment style.
        let bold = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![StatusBarSegment {
                text: "BOLD".to_string(),
                fg: Color::rgb(0, 0, 0),
                bg: Color::rgb(0, 0, 0),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![mk("xx", "a"), mk("yy", "b")],
        };
        let measure_with_bold =
            |seg: &StatusBarSegment| seg.text.chars().count() * 10 + if seg.bold { 5 } else { 0 };
        // Left: 4*10 + 5 (bold) = 45. Right total: 20 + 20 = 40. Gap 5.
        // 45 + 5 + 40 = 90 <= 90 → no drop.
        assert_eq!(bold.fit_right_start(90, 5, measure_with_bold), 0);
        // 89: drop one — first ("xx").
        assert_eq!(bold.fit_right_start(89, 5, measure_with_bold), 1);
    }

    #[test]
    fn status_bar_resolve_click_fit_chars_skips_dropped() {
        use primitives::status_bar::StatusBarSegment;
        let mk = |text: &str, id: &str| StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(0, 0, 0),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            action_id: Some(WidgetId::new(id)),
        };
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![mk(" L ", "left")],
            right_segments: vec![mk(" drop ", "drop"), mk(" keep ", "keep")],
        };

        // bar_width 20 fits both on the right (3+12=15 <= 20-0=20 with gap 2): left_w=3, gap=2, total_r=12, 3+2+12=17 <= 20.
        // No drop; keep starts at col 14 (20-6), drop at col 8 (20-12).
        assert_eq!(
            bar.resolve_click_fit_chars(10, 20, 2)
                .as_ref()
                .map(|w| w.as_str()),
            Some("drop")
        );

        // Narrow bar: 3 + 2 + 12 = 17 > 15. Drop " drop " (6). Remaining " keep " (6) fits (3+2+6=11<=15).
        // Now visible right: just "keep" at col 15-6=9.
        // Click at col 10 → hits "keep".
        assert_eq!(
            bar.resolve_click_fit_chars(10, 15, 2)
                .as_ref()
                .map(|w| w.as_str()),
            Some("keep")
        );
        // Click at col 3 (where "drop" used to be) → no segment.
        assert_eq!(bar.resolve_click_fit_chars(3, 15, 2), None);
    }

    #[test]
    fn text_display_append_and_cap() {
        use primitives::text_display::TextDisplayLine;
        let mut td = TextDisplay::new(WidgetId::new("logs"));
        td.set_max_lines(3);

        let mk = |text: &str| TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        };

        td.append_line(mk("a"));
        td.append_line(mk("b"));
        td.append_line(mk("c"));
        assert_eq!(td.lines.len(), 3);

        // Fourth append evicts the oldest.
        td.append_line(mk("d"));
        assert_eq!(td.lines.len(), 3);
        assert_eq!(td.lines.first().unwrap().spans[0].text, "b");
        assert_eq!(td.lines.last().unwrap().spans[0].text, "d");

        // Lower the cap → trims oldest.
        td.set_max_lines(2);
        assert_eq!(td.lines.len(), 2);
        assert_eq!(td.lines.first().unwrap().spans[0].text, "c");

        td.clear();
        assert_eq!(td.lines.len(), 0);
        assert_eq!(td.scroll_offset, 0);
    }

    #[test]
    fn text_display_roundtrip_serde() {
        use primitives::text_display::TextDisplayLine;
        let td = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![
                TextDisplayLine {
                    spans: vec![StyledSpan::plain("hello")],
                    decoration: Decoration::Normal,
                    timestamp: Some("12:00:00".to_string()),
                },
                TextDisplayLine {
                    spans: vec![
                        StyledSpan::plain("error: "),
                        StyledSpan::with_fg("not found", Color::rgb(255, 80, 80)),
                    ],
                    decoration: Decoration::Error,
                    timestamp: None,
                },
            ],
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 1000,
            has_focus: true,
        };
        let json = serde_json::to_string(&td).unwrap();
        let back: TextDisplay = serde_json::from_str(&json).unwrap();
        assert_eq!(td, back);
    }

    #[test]
    fn text_display_event_roundtrip_serde() {
        let events = vec![
            TextDisplayEvent::Scrolled { new_offset: 42 },
            TextDisplayEvent::AutoScrollToggled { enabled: false },
            TextDisplayEvent::Copied {
                text: "selected line".to_string(),
            },
            TextDisplayEvent::KeyPressed {
                key: "G".to_string(),
                modifiers: Modifiers::default(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: TextDisplayEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
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
    fn tab_bar_fit_active_scroll_offset() {
        // 10 tabs, each measures 100 units. Width 250 fits 2 full tabs.
        let measure = |_i: usize| 100usize;

        // Active 0 fits at offset 0.
        assert_eq!(TabBar::fit_active_scroll_offset(0, 10, 250, measure), 0);
        // Active 1 fits at offset 0 (tabs 0,1 fit in 250).
        assert_eq!(TabBar::fit_active_scroll_offset(1, 10, 250, measure), 0);
        // Active 2 doesn't fit at offset 0; walk back from 2 → tabs 1,2 fit in 250 → offset 1.
        assert_eq!(TabBar::fit_active_scroll_offset(2, 10, 250, measure), 1);
        // Active 9 (last) → tabs 8,9 fit → offset 8.
        assert_eq!(TabBar::fit_active_scroll_offset(9, 10, 250, measure), 8);

        // Variable widths: GTK-like scenario where each tab has ~50 units of
        // padding overhead the engine's char-based estimate would miss.
        let varied = [60, 80, 70, 90, 100, 50, 120, 75, 65, 85];
        let measure_varied = |i: usize| varied[i];
        // Width 200, active 9 (last). Walk back: 85+65=150, +75=225 > 200 → break.
        // best_offset stays at 8 (only tabs 8,9 fit).
        assert_eq!(
            TabBar::fit_active_scroll_offset(9, 10, 200, measure_varied),
            8
        );

        // Edge case: tab wider than width → keep that tab as the only visible one.
        // Active 5 (width 50) at width 30. Walk back from 5: tw=50 > 30 → break.
        // best_offset stays at 5 (initial). Renders tab 5 alone, clipped on right.
        assert_eq!(
            TabBar::fit_active_scroll_offset(5, 10, 30, measure_varied),
            5
        );

        // Edge cases: empty + out-of-bounds active.
        assert_eq!(TabBar::fit_active_scroll_offset(0, 0, 100, measure), 0);
        assert_eq!(TabBar::fit_active_scroll_offset(99, 5, 100, measure), 0);
    }

    // ── MenuBar primitive tests ───────────────────────────────────────

    fn mk_menu_item(id: &str, label: &str) -> MenuBarItem {
        MenuBarItem {
            id: WidgetId::new(id),
            label: label.to_string(),
            disabled: false,
        }
    }

    #[test]
    fn menu_bar_layout_flat_items() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![
                mk_menu_item("file", "&File"),
                mk_menu_item("edit", "&Edit"),
                mk_menu_item("view", "&View"),
            ],
            open_item: None,
            focused_item: None,
        };
        let bounds = Rect::new(0.0, 0.0, 800.0, 20.0);
        let layout = bar.layout(bounds, |_| MenuBarItemMeasure::new(60.0));
        assert_eq!(layout.visible_items.len(), 3);
        assert_eq!(layout.visible_items[0].bounds.x, 0.0);
        assert_eq!(layout.visible_items[1].bounds.x, 60.0);
        assert_eq!(layout.visible_items[2].bounds.x, 120.0);
        // Click on Edit.
        match layout.hit_test(70.0, 10.0) {
            MenuBarHit::Item(1) => {}
            other => panic!("expected Item(1), got {other:?}"),
        }
    }

    #[test]
    fn menu_bar_alt_target_resolution() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![
                mk_menu_item("file", "&File"),
                mk_menu_item("edit", "&Edit"),
                mk_menu_item("view", "&View"),
            ],
            open_item: None,
            focused_item: None,
        };
        assert_eq!(bar.find_alt_target('f'), Some(0));
        assert_eq!(bar.find_alt_target('E'), Some(1));
        assert_eq!(bar.find_alt_target('v'), Some(2));
        assert_eq!(bar.find_alt_target('x'), None);
    }

    #[test]
    fn menu_bar_disabled_items_not_clickable() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![
                mk_menu_item("file", "&File"),
                MenuBarItem {
                    id: WidgetId::new("tools"),
                    label: "&Tools".to_string(),
                    disabled: true,
                },
            ],
            open_item: None,
            focused_item: None,
        };
        let bounds = Rect::new(0.0, 0.0, 800.0, 20.0);
        let layout = bar.layout(bounds, |_| MenuBarItemMeasure::new(60.0));
        assert!(!layout.visible_items[1].clickable);
        // Click on the disabled Tools item → Bar (not Item).
        assert_eq!(layout.hit_test(70.0, 10.0), MenuBarHit::Bar);
        // Alt+t skips disabled.
        assert_eq!(bar.find_alt_target('t'), None);
    }

    #[test]
    fn menu_bar_click_outside() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![mk_menu_item("file", "File")],
            open_item: None,
            focused_item: None,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 20.0);
        let layout = bar.layout(bounds, |_| MenuBarItemMeasure::new(50.0));
        assert_eq!(layout.hit_test(100.0, 50.0), MenuBarHit::Outside);
    }

    // ── Modal primitive tests ─────────────────────────────────────────

    #[test]
    fn modal_layout_centers_content() {
        let m = Modal {
            id: WidgetId::new("m"),
            content_width: 400,
            content_height: 300,
            backdrop_color: None,
            dismiss_on_backdrop: true,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = m.layout(viewport);
        assert_eq!(layout.backdrop_bounds, viewport);
        // Centered: x = (800 - 400)/2 = 200; y = (600 - 300)/2 = 150
        assert_eq!(layout.content_bounds.x, 200.0);
        assert_eq!(layout.content_bounds.y, 150.0);
        assert_eq!(layout.content_bounds.width, 400.0);
        assert_eq!(layout.content_bounds.height, 300.0);
    }

    #[test]
    fn modal_hit_test_content_vs_backdrop() {
        let m = Modal {
            id: WidgetId::new("m"),
            content_width: 200,
            content_height: 100,
            backdrop_color: None,
            dismiss_on_backdrop: true,
        };
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let layout = m.layout(viewport);
        // Click inside content.
        match layout.hit_test(200.0, 150.0) {
            ModalHit::Content(id) => assert_eq!(id.as_str(), "m"),
            _ => panic!("expected Content hit"),
        }
        // Click on backdrop.
        match layout.hit_test(10.0, 10.0) {
            ModalHit::Backdrop(id) => assert_eq!(id.as_str(), "m"),
            _ => panic!("expected Backdrop hit"),
        }
    }

    #[test]
    fn modal_content_clamped_to_viewport() {
        // Requested size bigger than viewport — content should clamp.
        let m = Modal {
            id: WidgetId::new("m"),
            content_width: 2000,
            content_height: 2000,
            backdrop_color: None,
            dismiss_on_backdrop: true,
        };
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let layout = m.layout(viewport);
        assert_eq!(layout.content_bounds.width, 400.0);
        assert_eq!(layout.content_bounds.height, 300.0);
    }

    // ── Split primitive tests ─────────────────────────────────────────

    #[test]
    fn split_layout_horizontal_even() {
        let s = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let bounds = Rect::new(0.0, 0.0, 202.0, 100.0);
        let layout = s.layout(bounds, SplitMeasure::new(2.0));
        // available = 200, first = 100, second = 100.
        assert_eq!(layout.first_bounds.width, 100.0);
        assert_eq!(layout.divider_bounds.x, 100.0);
        assert_eq!(layout.divider_bounds.width, 2.0);
        assert_eq!(layout.second_bounds.x, 102.0);
        assert_eq!(layout.second_bounds.width, 100.0);
    }

    #[test]
    fn split_layout_vertical_with_min() {
        let s = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Vertical,
            ratio: 0.1, // too small; clamped up to first_min
            first_min: 30.0,
            second_min: 20.0,
        };
        let bounds = Rect::new(0.0, 0.0, 100.0, 101.0);
        let layout = s.layout(bounds, SplitMeasure::new(1.0));
        // available = 100. Raw first = 10; clamped to first_min = 30.
        assert_eq!(layout.first_bounds.height, 30.0);
        assert_eq!(layout.second_bounds.height, 70.0);
        assert_eq!(layout.divider_bounds.y, 30.0);
    }

    #[test]
    fn split_hit_test_regions() {
        let s = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
        let layout = s.layout(bounds, SplitMeasure::new(2.0));
        // Click in first pane.
        match layout.hit_test(50.0, 50.0) {
            SplitHit::FirstPane(id) => assert_eq!(id.as_str(), "s"),
            _ => panic!(),
        }
        // Click on divider (x = 99.0..101.0).
        match layout.hit_test(99.5, 50.0) {
            SplitHit::Divider(id) => assert_eq!(id.as_str(), "s"),
            _ => panic!(),
        }
        // Click in second pane.
        match layout.hit_test(150.0, 50.0) {
            SplitHit::SecondPane(id) => assert_eq!(id.as_str(), "s"),
            _ => panic!(),
        }
    }

    #[test]
    fn split_layout_resolved_ratio() {
        let s = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.0, // would collapse first pane
            first_min: 50.0,
            second_min: 0.0,
        };
        let bounds = Rect::new(0.0, 0.0, 201.0, 100.0);
        let layout = s.layout(bounds, SplitMeasure::new(1.0));
        // first_size clamped to 50; resolved_ratio = 50/200 = 0.25.
        assert!((layout.resolved_ratio - 0.25).abs() < 0.001);
    }

    // ── Panel primitive tests ─────────────────────────────────────────

    fn mk_panel_action(id: &str, icon: char) -> PanelAction {
        PanelAction {
            id: WidgetId::new(id),
            icon: icon.to_string(),
            tooltip: String::new(),
            is_active: false,
        }
    }

    #[test]
    fn panel_layout_with_title_and_actions() {
        let p = Panel {
            id: WidgetId::new("terminal"),
            title: Some(StyledText::plain("Terminal")),
            actions: vec![
                mk_panel_action("term:split", '+'),
                mk_panel_action("term:max", '□'),
                mk_panel_action("term:close", '×'),
            ],
            accent: None,
            collapsed: false,
        };
        let bounds = Rect::new(0.0, 0.0, 400.0, 200.0);
        let layout = p.layout(bounds, PanelMeasure::new(24.0));
        assert!(layout.title_bar_bounds.is_some());
        let tb = layout.title_bar_bounds.unwrap();
        assert_eq!(tb.height, 24.0);
        // Actions right-aligned: close (last in actions) at rightmost.
        assert_eq!(layout.visible_actions.len(), 3);
        assert_eq!(layout.visible_actions[0].id.as_str(), "term:split");
        // Rightmost action is first in iteration (right-to-left placement).
        assert_eq!(
            layout.visible_actions[0].bounds.x + layout.visible_actions[0].bounds.width,
            400.0
        );
        // Content region below title bar.
        assert_eq!(layout.content_bounds.y, 24.0);
        assert_eq!(layout.content_bounds.height, 200.0 - 24.0);
    }

    #[test]
    fn panel_layout_no_title() {
        let p = Panel {
            id: WidgetId::new("p"),
            title: None,
            actions: vec![],
            accent: None,
            collapsed: false,
        };
        let bounds = Rect::new(10.0, 20.0, 300.0, 100.0);
        let layout = p.layout(bounds, PanelMeasure::new(24.0));
        assert!(layout.title_bar_bounds.is_none());
        // Content fills the full panel.
        assert_eq!(layout.content_bounds.y, 20.0);
        assert_eq!(layout.content_bounds.height, 100.0);
    }

    #[test]
    fn panel_layout_collapsed() {
        let p = Panel {
            id: WidgetId::new("p"),
            title: Some(StyledText::plain("Collapsed")),
            actions: vec![],
            accent: None,
            collapsed: true,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 150.0);
        let layout = p.layout(bounds, PanelMeasure::new(20.0));
        // Title bar still rendered; content region has zero size.
        assert!(layout.title_bar_bounds.is_some());
        assert_eq!(layout.content_bounds.width, 0.0);
        assert_eq!(layout.content_bounds.height, 0.0);
    }

    #[test]
    fn panel_layout_hit_test_dispatches_correctly() {
        let p = Panel {
            id: WidgetId::new("p"),
            title: Some(StyledText::plain("T")),
            actions: vec![mk_panel_action("close", '×')],
            accent: None,
            collapsed: false,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
        let layout = p.layout(bounds, PanelMeasure::new(20.0));
        // Click on the close button (rightmost in title bar).
        let close = &layout.visible_actions[0];
        let cx = close.bounds.x + close.bounds.width / 2.0;
        let cy = close.bounds.y + close.bounds.height / 2.0;
        match layout.hit_test(cx, cy) {
            PanelHit::Action(id) => assert_eq!(id.as_str(), "close"),
            _ => panic!("expected Action(close)"),
        }
        // Click on title bar body.
        match layout.hit_test(20.0, 10.0) {
            PanelHit::TitleBar(id) => assert_eq!(id.as_str(), "p"),
            _ => panic!("expected TitleBar"),
        }
        // Click on content region.
        match layout.hit_test(100.0, 50.0) {
            PanelHit::Content(id) => assert_eq!(id.as_str(), "p"),
            _ => panic!("expected Content"),
        }
        // Click outside.
        assert_eq!(layout.hit_test(500.0, 500.0), PanelHit::Outside);
    }

    // ── Dialog primitive tests ────────────────────────────────────────

    fn mk_dialog_button(id: &str, label: &str) -> DialogButton {
        DialogButton {
            id: WidgetId::new(id),
            label: label.to_string(),
            is_default: false,
            is_cancel: false,
            tint: None,
        }
    }

    #[test]
    fn dialog_layout_centered_horizontal_buttons() {
        let mut ok = mk_dialog_button("ok", "OK");
        ok.is_default = true;
        let mut cancel = mk_dialog_button("cancel", "Cancel");
        cancel.is_cancel = true;
        let d = Dialog {
            id: WidgetId::new("confirm"),
            title: StyledText::plain("Close unsaved file?"),
            body: StyledText::plain("You have unsaved changes. Close anyway?"),
            buttons: vec![cancel, ok],
            severity: Some(DialogSeverity::Question),
            vertical_buttons: false,
            input: None,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let measure = DialogMeasure {
            width: 400.0,
            title_height: 24.0,
            body_height: 40.0,
            input_height: 0.0,
            button_row_height: 32.0,
            button_width: 80.0,
            button_gap: 8.0,
            padding: 16.0,
        };
        let layout = d.layout(viewport, measure);

        // Centered: x = (800 - 400)/2 = 200
        assert_eq!(layout.bounds.x, 200.0);
        // total_h = 24 + 40 + 32 + 32 = 128; y = (600 - 128)/2 = 236
        assert_eq!(layout.bounds.y, 236.0);

        assert!(layout.title_bounds.is_some());
        assert_eq!(layout.visible_buttons.len(), 2);
        // Right-aligned: last button's right edge = content right edge.
        let last = &layout.visible_buttons[1];
        assert_eq!(
            last.bounds.x + last.bounds.width,
            layout.bounds.x + measure.width - measure.padding
        );
    }

    #[test]
    fn dialog_layout_vertical_buttons() {
        let d = Dialog {
            id: WidgetId::new("code-actions"),
            title: StyledText::plain("Code actions"),
            body: StyledText::default(),
            buttons: vec![
                mk_dialog_button("a1", "Add import"),
                mk_dialog_button("a2", "Use fully qualified path"),
                mk_dialog_button("a3", "Ignore"),
            ],
            severity: None,
            vertical_buttons: true,
            input: None,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let measure = DialogMeasure {
            width: 300.0,
            title_height: 20.0,
            body_height: 0.0,
            input_height: 0.0,
            button_row_height: 90.0,
            button_width: 280.0,
            button_gap: 0.0,
            padding: 10.0,
        };
        let layout = d.layout(viewport, measure);
        assert_eq!(layout.visible_buttons.len(), 3);
        // Stacked: each 30px tall (90/3); widths equal content_w = 300-20=280.
        for b in &layout.visible_buttons {
            assert_eq!(b.bounds.width, 280.0);
            assert_eq!(b.bounds.height, 30.0);
        }
        // Button 0 y < button 1 y < button 2 y.
        assert!(layout.visible_buttons[0].bounds.y < layout.visible_buttons[1].bounds.y);
    }

    #[test]
    fn dialog_hit_test_on_button_returns_id() {
        let d = Dialog {
            id: WidgetId::new("confirm"),
            title: StyledText::plain("T"),
            body: StyledText::plain("B"),
            buttons: vec![
                mk_dialog_button("cancel", "Cancel"),
                mk_dialog_button("ok", "OK"),
            ],
            severity: None,
            vertical_buttons: false,
            input: None,
        };
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let measure = DialogMeasure {
            width: 200.0,
            title_height: 20.0,
            body_height: 20.0,
            input_height: 0.0,
            button_row_height: 20.0,
            button_width: 60.0,
            button_gap: 10.0,
            padding: 10.0,
        };
        let layout = d.layout(viewport, measure);
        // Click on the second (right) button — OK.
        let ok_btn = &layout.visible_buttons[1];
        let cx = ok_btn.bounds.x + ok_btn.bounds.width / 2.0;
        let cy = ok_btn.bounds.y + ok_btn.bounds.height / 2.0;
        match layout.hit_test(cx, cy) {
            DialogHit::Button(id) => assert_eq!(id.as_str(), "ok"),
            _ => panic!(),
        }
        // Click on body (not on button) → Body.
        assert_eq!(
            layout.hit_test(layout.body_bounds.x + 5.0, layout.body_bounds.y + 5.0),
            DialogHit::Body
        );
        // Click outside dialog → Outside.
        assert_eq!(layout.hit_test(0.0, 0.0), DialogHit::Outside);
    }

    #[test]
    fn dialog_with_input_field_layout() {
        // Rename-style dialog: title + short body + input + buttons.
        let ok = mk_dialog_button("ok", "OK");
        let cancel = mk_dialog_button("cancel", "Cancel");
        let d = Dialog {
            id: WidgetId::new("rename"),
            title: StyledText::plain("Rename"),
            body: StyledText::plain("New name:"),
            buttons: vec![cancel, ok],
            severity: None,
            vertical_buttons: false,
            input: Some(DialogInput {
                value: "old_name.rs".to_string(),
                placeholder: String::new(),
                cursor: Some(11),
            }),
        };
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let measure = DialogMeasure {
            width: 200.0,
            title_height: 20.0,
            body_height: 20.0,
            input_height: 24.0,
            button_row_height: 20.0,
            button_width: 60.0,
            button_gap: 10.0,
            padding: 10.0,
        };
        let layout = d.layout(viewport, measure);
        assert!(layout.input_bounds.is_some());
        let ib = layout.input_bounds.unwrap();
        // Input sits between body and buttons.
        assert_eq!(ib.y, layout.body_bounds.y + layout.body_bounds.height);
        assert_eq!(ib.height, 24.0);
        // Buttons pushed down by input_height.
        assert_eq!(layout.button_row_bounds.y, ib.y + ib.height);
    }

    #[test]
    fn dialog_default_and_cancel_resolution() {
        let mut ok = mk_dialog_button("ok", "OK");
        ok.is_default = true;
        let mut cancel = mk_dialog_button("cancel", "Cancel");
        cancel.is_cancel = true;
        let d = Dialog {
            id: WidgetId::new("d"),
            title: StyledText::default(),
            body: StyledText::default(),
            buttons: vec![cancel, ok],
            severity: None,
            vertical_buttons: false,
            input: None,
        };
        assert_eq!(d.default_button_id().unwrap().as_str(), "ok");
        assert_eq!(d.cancel_button_id().unwrap().as_str(), "cancel");
    }

    // ── Form field primitive tests (#143 Slider/ColorPicker/Dropdown) ─

    #[test]
    fn form_slider_field_serde() {
        let field = FormField {
            id: WidgetId::new("font-size"),
            label: StyledText::plain("Font size"),
            kind: FieldKind::Slider {
                value: 14.0,
                min: 8.0,
                max: 32.0,
                step: 1.0,
            },
            hint: StyledText::plain("Editor font size in px"),
            disabled: false,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);
    }

    #[test]
    fn form_color_picker_field_serde() {
        let field = FormField {
            id: WidgetId::new("accent"),
            label: StyledText::plain("Accent colour"),
            kind: FieldKind::ColorPicker {
                value: Color::rgb(0x78, 0xb4, 0xff),
            },
            hint: StyledText::default(),
            disabled: false,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);
    }

    #[test]
    fn form_dropdown_field_serde() {
        let field = FormField {
            id: WidgetId::new("theme"),
            label: StyledText::plain("Theme"),
            kind: FieldKind::Dropdown {
                options: vec![
                    StyledText::plain("One Dark"),
                    StyledText::plain("Solarized Light"),
                    StyledText::plain("Monokai"),
                ],
                selected_idx: 0,
            },
            hint: StyledText::default(),
            disabled: false,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);
    }

    #[test]
    fn form_slider_legacy_deserialize_with_default_step() {
        // A client might omit `step` — it defaults to 1.0 via serde.
        let json = r#"{
            "id": "x",
            "label": {"spans":[{"text":"X","fg":null,"bg":null}]},
            "kind": {"Slider": {"value": 5.0, "min": 0.0, "max": 10.0}},
            "hint": {"spans":[]}
        }"#;
        let field: FormField = serde_json::from_str(json).unwrap();
        match field.kind {
            FieldKind::Slider { step, .. } => assert_eq!(step, 1.0),
            _ => panic!("expected Slider"),
        }
    }

    // ── Completions primitive tests (D6) ──────────────────────────────

    fn make_completion(label: &str, kind: CompletionKind) -> CompletionItem {
        CompletionItem {
            label: StyledText::plain(label),
            detail: None,
            documentation: None,
            kind,
            icon: None,
        }
    }

    #[test]
    fn completions_layout_below_cursor() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: vec![
                make_completion("fn main", CompletionKind::Function),
                make_completion("fn map", CompletionKind::Function),
                make_completion("Vec", CompletionKind::Struct),
            ],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = c.layout(100.0, 50.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.placement, CompletionsPlacement::Below);
        assert_eq!(layout.bounds.x, 100.0);
        // Popup y = cursor_y + line_height = 50 + 18 = 68
        assert_eq!(layout.bounds.y, 68.0);
        assert_eq!(layout.visible_items.len(), 3);
        assert_eq!(layout.visible_items[0].bounds.y, 68.0);
        // Click on 2nd item.
        match layout.hit_test(150.0, 90.0) {
            CompletionsHit::Item(idx) => assert_eq!(idx, 1),
            _ => panic!(),
        }
    }

    #[test]
    fn completions_layout_flips_above_when_below_overflows() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: (0..5)
                .map(|i| make_completion(&format!("x{i}"), CompletionKind::Variable))
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        // Cursor near the bottom of a small viewport.
        let viewport = Rect::new(0.0, 0.0, 800.0, 150.0);
        // Cursor at y=120, line_height=18 → below_y = 138, need 100 px (5 × 20)
        // → bottom edge = 238, > 150 viewport. Flip above.
        let layout = c.layout(100.0, 120.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.placement, CompletionsPlacement::Above);
        // Above: y = cursor - content_h = 120 - 100 = 20
        assert_eq!(layout.bounds.y, 20.0);
    }

    #[test]
    fn completions_layout_shifts_left_when_right_overflows() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: vec![make_completion("item", CompletionKind::Text)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 400.0, 600.0);
        // Cursor near right edge — popup_width 300, cursor_x 200 → right would be 500 > 400.
        let layout = c.layout(200.0, 50.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 100.0); // 400 - 300
    }

    #[test]
    fn completions_layout_scroll_offset_applies() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: (0..10)
                .map(|i| make_completion(&format!("x{i}"), CompletionKind::Variable))
                .collect(),
            selected_idx: 0,
            scroll_offset: 5,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = c.layout(0.0, 0.0, 18.0, viewport, 200.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        // Visible items start at index 5.
        assert_eq!(layout.visible_items[0].item_idx, 5);
    }

    // ── ContextMenu primitive tests (D6) ──────────────────────────────

    fn cm_action(id: &str, label: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(label),
            detail: None,
            disabled: false,
        }
    }

    fn cm_separator() -> ContextMenuItem {
        ContextMenuItem {
            id: None,
            label: StyledText::default(),
            detail: None,
            disabled: false,
        }
    }

    #[test]
    fn context_menu_layout_flat() {
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![
                cm_action("cut", "Cut"),
                cm_action("copy", "Copy"),
                cm_separator(),
                cm_action("paste", "Paste"),
            ],
            selected_idx: 0,
            bg: None,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = menu.layout(100.0, 100.0, viewport, 160.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 100.0);
        assert_eq!(layout.bounds.y, 100.0);
        assert_eq!(layout.bounds.width, 160.0);
        assert_eq!(layout.bounds.height, 80.0); // 4 × 20
        assert_eq!(layout.visible_items.len(), 4);
        // Separator at index 2 is visually present, non-clickable.
        assert!(layout.visible_items[2].is_separator);
        assert!(!layout.visible_items[2].clickable);
        // Hit-test on Copy (2nd item, y=120..140).
        match layout.hit_test(120.0, 125.0) {
            ContextMenuHit::Item(id) => assert_eq!(id.as_str(), "copy"),
            _ => panic!("expected Item(copy)"),
        }
        // Hit-test on separator (y=140..160) → Inert.
        assert_eq!(layout.hit_test(120.0, 150.0), ContextMenuHit::Inert);
        // Hit-test far outside → Empty.
        assert_eq!(layout.hit_test(500.0, 500.0), ContextMenuHit::Empty);
    }

    #[test]
    fn context_menu_layout_shifts_left_when_overflow() {
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![cm_action("a", "A")],
            selected_idx: 0,
            bg: None,
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        // Anchor at x=180, menu_width=100 → right edge would be 280 > 200.
        let layout = menu.layout(180.0, 50.0, viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 100.0); // 200 - 100 = 100
    }

    #[test]
    fn context_menu_layout_shifts_up_when_overflow() {
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![cm_action("a", "A"), cm_action("b", "B")],
            selected_idx: 0,
            bg: None,
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 100.0);
        // Anchor at y=80, 2 items × 20 = 40, bottom would be 120 > 100.
        let layout = menu.layout(10.0, 80.0, viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.y, 60.0); // 100 - 40
    }

    #[test]
    fn context_menu_layout_disabled_items_inert() {
        let mut menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![cm_action("delete", "Delete")],
            selected_idx: 0,
            bg: None,
        };
        menu.items[0].disabled = true;
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = menu.layout(10.0, 10.0, viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert!(!layout.visible_items[0].clickable);
        // Click on disabled item → Inert, not Item.
        assert_eq!(layout.hit_test(50.0, 15.0), ContextMenuHit::Inert);
    }

    // ── Tooltip primitive tests (D6 shape) ────────────────────────────

    #[test]
    fn tooltip_layout_prefers_bottom_when_room() {
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Hello".to_string(),
            placement: TooltipPlacement::Bottom,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let anchor = Rect::new(100.0, 50.0, 40.0, 20.0);
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(60.0, 16.0), 4.0);
        assert_eq!(layout.resolved_placement, ResolvedPlacement::Bottom);
        // Bottom placement: y = anchor.y + anchor.height + margin = 50 + 20 + 4 = 74
        assert_eq!(layout.bounds.y, 74.0);
        // Centered horizontally on anchor: x = 100 + (40 - 60)/2 = 90
        assert_eq!(layout.bounds.x, 90.0);
    }

    #[test]
    fn tooltip_layout_flips_to_opposite_when_overflow() {
        // Anchor near bottom of viewport — preferred Bottom would overflow.
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Hello".to_string(),
            placement: TooltipPlacement::Bottom,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 100.0);
        let anchor = Rect::new(100.0, 80.0, 40.0, 16.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(60.0, 16.0), 4.0);
        // Bottom would put tooltip at y=80+16+4=100 which exceeds viewport_height 100.
        // Flip to Top.
        assert_eq!(layout.resolved_placement, ResolvedPlacement::Top);
        // Top: y = anchor.y - margin - vh = 80 - 4 - 16 = 60
        assert_eq!(layout.bounds.y, 60.0);
    }

    #[test]
    fn tooltip_layout_clamped_when_neither_fits() {
        // Tiny viewport, anchor in middle — both Top and Bottom overflow
        // (viewport is shorter than anchor + margin + tooltip).
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "…".to_string(),
            placement: TooltipPlacement::Bottom,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let viewport = Rect::new(0.0, 0.0, 100.0, 30.0);
        let anchor = Rect::new(10.0, 10.0, 20.0, 10.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(40.0, 20.0), 4.0);
        // Preferred (Bottom) clamped: y would be 10+10+4=24; tooltip_h=20 →
        // bottom edge at 44, > viewport 30. Doesn't fit. Try Top: y = 10-4-20=-14 → doesn't fit.
        // Clamp preferred (Bottom): y clamped to viewport.height - vh = 30 - 20 = 10.
        assert_eq!(layout.resolved_placement, ResolvedPlacement::Bottom);
        assert!(layout.bounds.y <= 10.0);
    }

    #[test]
    fn tooltip_layout_does_not_panic_when_wider_than_viewport() {
        // Regression for #213: when content width exceeds viewport width
        // the legacy clamp produced max < min and `f32::clamp` panicked.
        // The fix pins the tooltip to the viewport edge.
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Very wide".to_string(),
            placement: TooltipPlacement::Top,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let anchor = Rect::new(50.0, 50.0, 10.0, 10.0);
        let viewport = Rect::new(0.0, 0.0, 100.0, 100.0);
        // Content much wider than viewport (300 vs 100).
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(300.0, 16.0), 4.0);
        // Pinned to viewport.x rather than panicking; bounds carries the
        // requested size so the consumer can see the overflow.
        assert_eq!(layout.bounds.x, 0.0);
        assert_eq!(layout.bounds.width, 300.0);
    }

    #[test]
    fn tooltip_layout_does_not_panic_when_taller_than_viewport() {
        // Symmetric case for the y-axis.
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Very tall".to_string(),
            placement: TooltipPlacement::Left,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let anchor = Rect::new(50.0, 50.0, 10.0, 10.0);
        let viewport = Rect::new(0.0, 0.0, 100.0, 100.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(50.0, 500.0), 4.0);
        assert_eq!(layout.bounds.y, 0.0);
        assert_eq!(layout.bounds.height, 500.0);
    }

    #[test]
    fn tooltip_layout_hit_test() {
        let t = Tooltip {
            id: WidgetId::new("tip"),
            text: "Hover".to_string(),
            placement: TooltipPlacement::Right,
            styled_lines: None,
            bg: None,
            fg: None,
        };
        let anchor = Rect::new(100.0, 100.0, 20.0, 20.0);
        let viewport = Rect::new(0.0, 0.0, 400.0, 400.0);
        let layout = t.layout(anchor, viewport, TooltipMeasure::new(80.0, 16.0), 4.0);
        let center_x = layout.bounds.x + 10.0;
        let center_y = layout.bounds.y + 5.0;
        match layout.hit_test(center_x, center_y, &t.id) {
            TooltipHit::Body(id) => assert_eq!(id.as_str(), "tip"),
            _ => panic!(),
        }
        assert_eq!(layout.hit_test(0.0, 0.0, &t.id), TooltipHit::Empty);
    }

    // ── RichTextPopup primitive tests (#214) ─────────────────────────────

    fn make_rich_popup(lines: usize, max_visible: usize, scroll: usize) -> RichTextPopup {
        let line_text: Vec<String> = (0..lines).map(|i| format!("line {i:02}")).collect();
        let lines_styled: Vec<StyledText> = line_text
            .iter()
            .map(|s| StyledText::plain(s.clone()))
            .collect();
        RichTextPopup {
            id: WidgetId::new("hover"),
            lines: lines_styled,
            line_text,
            line_scales: Vec::new(),
            scroll_top: scroll,
            max_visible_rows: max_visible,
            has_focus: false,
            selection: None,
            links: Vec::new(),
            focused_link: None,
            placement: PopupPlacement::Above,
            padding: 1.0,
            fg: None,
            bg: None,
        }
    }

    #[test]
    fn rich_text_popup_layout_visible_lines_window() {
        let p = make_rich_popup(30, 10, 5);
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            10.0,
            100.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        // Visible window starts at scroll_top=5 and shows 10 rows (capped by total).
        assert_eq!(layout.visible_lines.len(), 10);
        assert_eq!(layout.visible_lines[0].line_idx, 5);
        assert_eq!(layout.visible_lines[9].line_idx, 14);
        assert_eq!(layout.resolved_scroll_offset, 5);
    }

    #[test]
    fn rich_text_popup_layout_clamps_scroll_past_end() {
        // 30 lines, 10 visible at a time → max scroll is 20.
        // Asking for scroll=999 should clamp.
        let p = make_rich_popup(30, 10, 999);
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            0.0,
            0.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        assert_eq!(layout.resolved_scroll_offset, 20);
        assert_eq!(layout.visible_lines.first().unwrap().line_idx, 20);
        assert_eq!(layout.visible_lines.last().unwrap().line_idx, 29);
    }

    #[test]
    fn rich_text_popup_scrollbar_present_when_overflow() {
        let p = make_rich_popup(50, 10, 0);
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            0.0,
            0.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        let sb = layout.scrollbar.expect("scrollbar should exist");
        // Thumb size proportional to visible/total = 10/50 = 1/5 of track.
        assert!(sb.thumb.height > 0.0);
        assert!(sb.thumb.height < sb.track.height);
        // No scrollbar when content fits.
        let p2 = make_rich_popup(5, 10, 0);
        let layout2 = p2.layout(
            0.0,
            0.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        assert!(layout2.scrollbar.is_none());
    }

    #[test]
    fn rich_text_popup_link_hit_regions_for_visible_lines() {
        let mut p = make_rich_popup(20, 10, 5);
        // Add a link on visible line 7 (visible_lines[2]).
        p.links.push(RichTextLink {
            line: 7,
            start_byte: 5,
            end_byte: 10,
            url: "https://example.com".to_string(),
        });
        // Add a link on hidden line 2 (above scroll_top=5).
        p.links.push(RichTextLink {
            line: 2,
            start_byte: 0,
            end_byte: 4,
            url: "off-screen".to_string(),
        });
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = p.layout(
            10.0,
            100.0,
            viewport,
            RichTextPopupMeasure::new(80.0, 1.0),
            |_, s, e| (e - s) as f32,
        );
        // Only the visible link gets a hit region.
        assert_eq!(layout.link_hit_regions.len(), 1);
        let (_, idx) = &layout.link_hit_regions[0];
        assert_eq!(*idx, 0);
    }

    #[test]
    fn text_selection_contains_single_and_multi_line() {
        let single = TextSelection {
            start_line: 3,
            start_col: 5,
            end_line: 3,
            end_col: 10,
        };
        assert!(single.contains(3, 5));
        assert!(single.contains(3, 9));
        assert!(!single.contains(3, 10));
        assert!(!single.contains(2, 5));
        assert!(!single.contains(4, 5));

        let multi = TextSelection {
            start_line: 2,
            start_col: 4,
            end_line: 5,
            end_col: 3,
        };
        assert!(!multi.contains(2, 3));
        assert!(multi.contains(2, 4));
        assert!(multi.contains(3, 0));
        assert!(multi.contains(4, 100));
        assert!(multi.contains(5, 0));
        assert!(multi.contains(5, 2));
        assert!(!multi.contains(5, 3));
        assert!(!multi.contains(6, 0));
    }

    // ── Spinner + ProgressBar primitive tests (D6, #142) ──────────────

    #[test]
    fn spinner_layout_bounds() {
        let s = Spinner {
            id: WidgetId::new("installing"),
            label: "Installing rust-analyzer…".to_string(),
            frame_idx: 42,
            accent: None,
        };
        let layout = s.layout(10.0, 20.0, SpinnerMeasure::new(200.0, 16.0));
        assert_eq!(layout.bounds.x, 10.0);
        assert_eq!(layout.bounds.y, 20.0);
        assert_eq!(layout.bounds.width, 200.0);
        // Hit-test on bounds returns Body(id).
        match layout.hit_test(100.0, 28.0, &s.id) {
            SpinnerHit::Body(id) => assert_eq!(id.as_str(), "installing"),
            _ => panic!(),
        }
        assert_eq!(layout.hit_test(500.0, 28.0, &s.id), SpinnerHit::Empty);
    }

    #[test]
    fn progress_bar_layout_determinate() {
        let p = ProgressBar {
            id: WidgetId::new("download"),
            label: "Downloading…".to_string(),
            value: Some(0.4),
            frame_idx: 0,
            cancellable: false,
            accent: None,
        };
        let layout = p.layout(0.0, 0.0, ProgressBarMeasure::new(200.0, 8.0));
        assert_eq!(layout.bounds.width, 200.0);
        let fill = layout.fill_bounds.unwrap();
        assert_eq!(fill.width, 80.0); // 0.4 * 200
        assert!(layout.cancel_bounds.is_none());
    }

    #[test]
    fn progress_bar_layout_indeterminate() {
        let p = ProgressBar {
            id: WidgetId::new("op"),
            label: String::new(),
            value: None,
            frame_idx: 5,
            cancellable: false,
            accent: None,
        };
        let layout = p.layout(0.0, 0.0, ProgressBarMeasure::new(100.0, 4.0));
        assert!(layout.fill_bounds.is_none());
    }

    #[test]
    fn progress_bar_layout_cancellable() {
        let p = ProgressBar {
            id: WidgetId::new("install"),
            label: "Installing…".to_string(),
            value: Some(0.5),
            frame_idx: 0,
            cancellable: true,
            accent: None,
        };
        let layout = p.layout(
            0.0,
            0.0,
            ProgressBarMeasure {
                width: 200.0,
                height: 8.0,
                cancel_width: 20.0,
            },
        );
        // Fill uses the bar area minus cancel width.
        let fill = layout.fill_bounds.unwrap();
        assert_eq!(fill.width, (200.0 - 20.0) * 0.5); // 90
        let cancel = layout.cancel_bounds.unwrap();
        assert_eq!(cancel.x, 180.0);
        assert_eq!(cancel.width, 20.0);
        // Click on cancel → Cancel(id).
        match layout.hit_test(190.0, 4.0) {
            ProgressBarHit::Cancel(id) => assert_eq!(id.as_str(), "install"),
            _ => panic!("expected Cancel hit"),
        }
        // Click on bar body (before cancel) → Body(id).
        match layout.hit_test(50.0, 4.0) {
            ProgressBarHit::Body(id) => assert_eq!(id.as_str(), "install"),
            _ => panic!("expected Body hit"),
        }
    }

    #[test]
    fn progress_bar_value_clamped() {
        let p = ProgressBar {
            id: WidgetId::new("overrun"),
            label: String::new(),
            value: Some(1.5), // > 1.0
            frame_idx: 0,
            cancellable: false,
            accent: None,
        };
        let layout = p.layout(0.0, 0.0, ProgressBarMeasure::new(100.0, 4.0));
        // Clamped to 1.0 → full width.
        assert_eq!(layout.fill_bounds.unwrap().width, 100.0);
    }

    // ── Toast primitive tests (D6 shape, new B.3 primitive) ───────────

    fn make_toast(id: &str, title: &str) -> ToastItem {
        ToastItem {
            id: WidgetId::new(id),
            title: title.to_string(),
            body: String::new(),
            severity: ToastSeverity::Info,
            action: None,
            accent: None,
        }
    }

    fn make_toast_stack(corner: ToastCorner, toasts: Vec<ToastItem>) -> ToastStack {
        ToastStack {
            id: WidgetId::new("toasts"),
            corner,
            toasts,
        }
    }

    #[test]
    fn toast_layout_empty() {
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![]);
        let layout = stack.layout(800.0, 600.0, 16.0, 8.0, |_| ToastMeasure::new(300.0, 64.0));
        assert_eq!(layout.visible_toasts.len(), 0);
        assert_eq!(layout.hit_test(100.0, 100.0), ToastHit::Empty);
    }

    #[test]
    fn toast_layout_bottom_right_newest_at_bottom() {
        let stack = make_toast_stack(
            ToastCorner::BottomRight,
            vec![
                make_toast("first", "First"),
                make_toast("second", "Second"),
                make_toast("third", "Third"),
            ],
        );
        let layout = stack.layout(800.0, 600.0, 16.0, 8.0, |_| ToastMeasure::new(300.0, 64.0));
        assert_eq!(layout.visible_toasts.len(), 3);
        // Newest (idx=2, "third") pinned at the bottom.
        let newest = &layout.visible_toasts[0];
        assert_eq!(newest.toast_idx, 2);
        assert_eq!(newest.id.as_str(), "third");
        // Newest bottom = viewport_height (600) - margin (16) - toast_height (64) = 520
        assert_eq!(newest.bounds.y, 520.0);
        // Right-aligned: x = 800 - 16 - 300 = 484
        assert_eq!(newest.bounds.x, 484.0);
        // Second-newest above with gap.
        assert_eq!(layout.visible_toasts[1].id.as_str(), "second");
        assert_eq!(layout.visible_toasts[1].bounds.y, 520.0 - 8.0 - 64.0);
    }

    #[test]
    fn toast_layout_top_left_newest_at_top() {
        let stack = make_toast_stack(
            ToastCorner::TopLeft,
            vec![make_toast("a", "A"), make_toast("b", "B")],
        );
        let layout = stack.layout(800.0, 600.0, 10.0, 5.0, |_| ToastMeasure::new(200.0, 50.0));
        assert_eq!(layout.visible_toasts.len(), 2);
        // Iteration is oldest-first for top corners.
        let first = &layout.visible_toasts[0];
        assert_eq!(first.id.as_str(), "a");
        assert_eq!(first.bounds.x, 10.0);
        assert_eq!(first.bounds.y, 10.0);
        let second = &layout.visible_toasts[1];
        assert_eq!(second.bounds.y, 10.0 + 50.0 + 5.0);
    }

    #[test]
    fn toast_layout_action_and_dismiss_regions() {
        let mut toast = make_toast("t1", "Build failed");
        toast.action = Some(ToastAction {
            id: WidgetId::new("open_log"),
            label: "Open log".to_string(),
        });
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![toast]);
        let layout = stack.layout(800.0, 600.0, 16.0, 8.0, |_| ToastMeasure {
            width: 300.0,
            height: 64.0,
            dismiss_width: 24.0,
            action_width: 80.0,
        });
        let v = &layout.visible_toasts[0];
        assert!(v.dismiss_bounds.is_some());
        assert!(v.action_bounds.is_some());
        let db = v.dismiss_bounds.unwrap();
        let ab = v.action_bounds.unwrap();
        // Dismiss at trailing edge.
        assert_eq!(db.x + db.width, v.bounds.x + v.bounds.width);
        // Action left of dismiss.
        assert_eq!(ab.x + ab.width, db.x);

        // Hit-test on dismiss.
        match layout.hit_test(db.x + 5.0, db.y + 10.0) {
            ToastHit::Dismiss(id) => assert_eq!(id.as_str(), "t1"),
            _ => panic!("expected Dismiss hit"),
        }
        // Hit-test on action.
        match layout.hit_test(ab.x + 5.0, ab.y + 10.0) {
            ToastHit::Action(id) => assert_eq!(id.as_str(), "open_log"),
            _ => panic!("expected Action hit"),
        }
        // Hit-test on body (left part of toast, not on action/dismiss).
        match layout.hit_test(v.bounds.x + 5.0, v.bounds.y + 10.0) {
            ToastHit::Body(id) => assert_eq!(id.as_str(), "t1"),
            _ => panic!("expected Body hit"),
        }
    }

    #[test]
    fn toast_layout_stack_clips_when_out_of_room() {
        // 5 toasts of 64px each, but viewport only has 200 px from margin
        // to top. Should render as many as fit.
        let stack = make_toast_stack(
            ToastCorner::BottomRight,
            (0..5)
                .map(|i| make_toast(&format!("t{i}"), &format!("T{i}")))
                .collect(),
        );
        let layout = stack.layout(800.0, 200.0, 10.0, 8.0, |_| ToastMeasure::new(300.0, 64.0));
        // Bottom stack. Newest at y = 200 - 10 - 64 = 126. Each subsequent
        // goes up 64+8=72. Next: 126-72=54. Next: 54-72=-18 (would be off-top).
        // So only 2-3 fit. Specifically we break when y_cursor <= 0.
        assert!(layout.visible_toasts.len() >= 2);
        assert!(layout.visible_toasts.len() <= 3);
    }

    // ── D6 Terminal layout API tests ──────────────────────────────────

    fn make_term(rows: usize, cols: usize) -> Terminal {
        let cell = primitives::terminal::TerminalCell {
            ch: ' ',
            fg: Color::rgb(200, 200, 200),
            bg: Color::rgb(20, 20, 20),
            bold: false,
            italic: false,
            underline: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
        };
        Terminal {
            id: WidgetId::new("term"),
            cells: (0..rows).map(|_| vec![cell.clone(); cols]).collect(),
        }
    }

    #[test]
    fn terminal_layout_tui_cells() {
        let term = make_term(24, 80);
        let layout = term.layout(80.0, 24.0, 1.0, 1.0);
        assert_eq!(layout.grid_rows, 24);
        assert_eq!(layout.grid_cols, 80);
        // Click at (5, 3) → cell (row=3, col=5).
        match layout.hit_test(5.5, 3.5) {
            TerminalHit::Cell { row, col } => {
                assert_eq!(row, 3);
                assert_eq!(col, 5);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn terminal_layout_pixel_cells() {
        // Native: 800x600 viewport, 8 px × 16 px cells = 100 cols × 37 rows.
        let term = make_term(37, 100);
        let layout = term.layout(800.0, 600.0, 8.0, 16.0);
        assert_eq!(layout.grid_cols, 100);
        assert_eq!(layout.grid_rows, 37); // 600/16 = 37.5 → 37
                                          // Click at (160, 48) → col=20, row=3.
        match layout.hit_test(160.0, 48.0) {
            TerminalHit::Cell { row, col } => {
                assert_eq!(col, 20);
                assert_eq!(row, 3);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn terminal_layout_hit_test_outside() {
        let term = make_term(10, 20);
        let layout = term.layout(20.0, 10.0, 1.0, 1.0);
        assert_eq!(layout.hit_test(-1.0, 5.0), TerminalHit::Empty);
        assert_eq!(layout.hit_test(5.0, -1.0), TerminalHit::Empty);
        assert_eq!(layout.hit_test(100.0, 5.0), TerminalHit::Empty);
    }

    #[test]
    fn terminal_layout_cell_bounds() {
        let term = make_term(10, 20);
        let layout = term.layout(20.0, 10.0, 1.0, 1.0);
        let r = layout.cell_bounds(3, 5).unwrap();
        assert_eq!(r.x, 5.0);
        assert_eq!(r.y, 3.0);
        assert_eq!(r.width, 1.0);
        assert_eq!(r.height, 1.0);
        // Out of range → None.
        assert!(layout.cell_bounds(99, 0).is_none());
        assert!(layout.cell_bounds(0, 99).is_none());
    }

    // ── D6 TextDisplay layout API tests ───────────────────────────────

    fn make_td_line(text: &str) -> primitives::text_display::TextDisplayLine {
        primitives::text_display::TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn make_td(
        lines: Vec<primitives::text_display::TextDisplayLine>,
        scroll: usize,
        auto: bool,
    ) -> TextDisplay {
        TextDisplay {
            id: WidgetId::new("td"),
            lines,
            scroll_offset: scroll,
            auto_scroll: auto,
            max_lines: 0,
            has_focus: true,
        }
    }

    #[test]
    fn text_display_layout_empty() {
        let td = make_td(vec![], 0, true);
        let layout = td.layout(40.0, 10.0, |_| TextDisplayLineMeasure::new(1.0));
        assert_eq!(layout.visible_lines.len(), 0);
        assert_eq!(layout.hit_test(5.0, 5.0), TextDisplayHit::Empty);
    }

    #[test]
    fn text_display_layout_manual_scroll() {
        let td = make_td(
            (0..10).map(|i| make_td_line(&format!("l{i}"))).collect(),
            3,
            false,
        );
        let layout = td.layout(40.0, 5.0, |_| TextDisplayLineMeasure::new(1.0));
        // scroll_offset honoured verbatim; 5 lines visible from offset 3.
        assert_eq!(layout.resolved_scroll_offset, 3);
        assert_eq!(layout.visible_lines.len(), 5);
        assert_eq!(layout.visible_lines[0].line_idx, 3);
    }

    #[test]
    fn text_display_layout_auto_scroll_pins_bottom() {
        // 10 lines, viewport fits 5 lines, auto_scroll true. Layout
        // should pick offset 5 so lines 5..10 are visible — ignoring
        // whatever scroll_offset was in the primitive.
        let td = make_td(
            (0..10).map(|i| make_td_line(&format!("l{i}"))).collect(),
            0, // stored scroll_offset overridden by auto-scroll
            true,
        );
        let layout = td.layout(40.0, 5.0, |_| TextDisplayLineMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 5);
        assert_eq!(layout.visible_lines.len(), 5);
        assert_eq!(layout.visible_lines[0].line_idx, 5);
        assert_eq!(layout.visible_lines[4].line_idx, 9);
    }

    #[test]
    fn text_display_layout_auto_scroll_short_stream() {
        // Only 3 lines, viewport fits 5. Auto-scroll pins bottom but
        // there's nothing to scroll past — offset should stay at 0.
        let td = make_td(
            (0..3).map(|i| make_td_line(&format!("l{i}"))).collect(),
            0,
            true,
        );
        let layout = td.layout(40.0, 5.0, |_| TextDisplayLineMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 0);
        assert_eq!(layout.visible_lines.len(), 3);
    }

    #[test]
    fn text_display_layout_wrap_heights() {
        // Simulate wrap: line 0 wraps to 3 rows, line 1 fits in 1 row,
        // line 2 wraps to 2 rows. Viewport 5 rows. Lines 0 + 1 take
        // rows 0..4; line 2 starts at y=4 and clips to 1 row.
        let td = make_td(
            (0..3).map(|i| make_td_line(&format!("l{i}"))).collect(),
            0,
            false,
        );
        let heights = [3.0, 1.0, 2.0];
        let layout = td.layout(40.0, 5.0, |i| TextDisplayLineMeasure::new(heights[i]));
        assert_eq!(layout.visible_lines.len(), 3);
        assert_eq!(layout.visible_lines[0].bounds.height, 3.0);
        assert_eq!(layout.visible_lines[1].bounds.y, 3.0);
        assert_eq!(layout.visible_lines[1].bounds.height, 1.0);
        // Third line clipped to the remaining 1 row of viewport.
        assert_eq!(layout.visible_lines[2].bounds.y, 4.0);
        assert_eq!(layout.visible_lines[2].bounds.height, 1.0);
    }

    // ── D6 Form layout API tests ──────────────────────────────────────

    fn make_form_field(id: &str, label: &str, kind: FieldKind) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label),
            kind,
            hint: StyledText::default(),
            disabled: false,
        }
    }

    fn make_form(fields: Vec<FormField>, scroll: usize) -> Form {
        Form {
            id: WidgetId::new("f"),
            fields,
            focused_field: None,
            scroll_offset: scroll,
            has_focus: true,
        }
    }

    #[test]
    fn form_layout_empty() {
        let f = make_form(vec![], 0);
        let layout = f.layout(40.0, 20.0, |_| FormFieldMeasure::new(1.0));
        assert_eq!(layout.visible_fields.len(), 0);
        assert_eq!(layout.hit_test(5.0, 5.0), FormHit::Empty);
    }

    #[test]
    fn form_layout_stacks_fields() {
        let f = make_form(
            vec![
                make_form_field("header", "Editor", FieldKind::Label),
                make_form_field("toggle1", "Line numbers", FieldKind::Toggle { value: true }),
                make_form_field("btn", "Save", FieldKind::Button),
            ],
            0,
        );
        let layout = f.layout(40.0, 10.0, |_| FormFieldMeasure::new(1.0));
        assert_eq!(layout.visible_fields.len(), 3);
        assert_eq!(layout.visible_fields[0].bounds.y, 0.0);
        assert_eq!(layout.visible_fields[1].bounds.y, 1.0);
        assert_eq!(layout.visible_fields[2].bounds.y, 2.0);
        match layout.hit_test(10.0, 1.5) {
            FormHit::Field(id) => assert_eq!(id.as_str(), "toggle1"),
            _ => panic!("expected Field(toggle1)"),
        }
    }

    #[test]
    fn form_layout_hit_carries_widget_id_not_index() {
        // Adding fields in arbitrary order — hit_test returns the id,
        // not the flat index, so apps don't care about ordering.
        let f = make_form(
            vec![
                make_form_field("zebra", "Zebra", FieldKind::Button),
                make_form_field("alpha", "Alpha", FieldKind::Button),
            ],
            0,
        );
        let layout = f.layout(40.0, 5.0, |_| FormFieldMeasure::new(1.0));
        match layout.hit_test(10.0, 0.5) {
            FormHit::Field(id) => assert_eq!(id.as_str(), "zebra"),
            _ => panic!(),
        }
        match layout.hit_test(10.0, 1.5) {
            FormHit::Field(id) => assert_eq!(id.as_str(), "alpha"),
            _ => panic!(),
        }
    }

    #[test]
    fn form_layout_scroll_offset_skips() {
        let f = make_form(
            (0..5)
                .map(|i| make_form_field(&format!("f{i}"), &format!("F{i}"), FieldKind::Button))
                .collect(),
            2,
        );
        let layout = f.layout(40.0, 10.0, |_| FormFieldMeasure::new(1.0));
        assert_eq!(layout.visible_fields[0].field_idx, 2);
        assert_eq!(layout.visible_fields[0].id.as_str(), "f2");
    }

    #[test]
    fn form_layout_varying_heights_by_kind() {
        // Fields with hints are taller; Label rows can be shorter.
        let fields = vec![
            make_form_field("hdr", "Header", FieldKind::Label),
            make_form_field(
                "txt",
                "Name",
                FieldKind::TextInput {
                    value: "John".to_string(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    selection_anchor: None,
                },
            ),
        ];
        let f = make_form(fields.clone(), 0);
        let layout = f.layout(40.0, 10.0, |i| {
            // Pretend TextInput fields are 2 rows tall (room for hint), Label is 1.
            match fields[i].kind {
                FieldKind::TextInput { .. } => FormFieldMeasure::new(2.0),
                _ => FormFieldMeasure::new(1.0),
            }
        });
        assert_eq!(layout.visible_fields[0].bounds.height, 1.0);
        assert_eq!(layout.visible_fields[1].bounds.y, 1.0);
        assert_eq!(layout.visible_fields[1].bounds.height, 2.0);
    }

    // ── D6 Palette layout API tests ───────────────────────────────────

    fn make_palette_item(text: &str) -> primitives::palette::PaletteItem {
        primitives::palette::PaletteItem {
            text: StyledText::plain(text),
            detail: None,
            icon: None,
            match_positions: vec![],
        }
    }

    fn make_palette(
        title: &str,
        query: &str,
        items: Vec<primitives::palette::PaletteItem>,
        selected: usize,
        scroll: usize,
    ) -> Palette {
        Palette {
            id: WidgetId::new("p"),
            title: title.to_string(),
            query: query.to_string(),
            query_cursor: 0,
            items,
            selected_idx: selected,
            scroll_offset: scroll,
            total_count: 0,
            has_focus: true,
        }
    }

    #[test]
    fn palette_layout_empty() {
        let p = make_palette("Commands", "", vec![], 0, 0);
        let layout = p.layout(40.0, 20.0, 0.0, 0.0, |_| PaletteItemMeasure::new(1.0));
        assert!(layout.title_bounds.is_none());
        assert!(layout.query_bounds.is_none());
        assert_eq!(layout.visible_items.len(), 0);
        assert_eq!(layout.hit_test(10.0, 5.0), PaletteHit::Empty);
    }

    #[test]
    fn palette_layout_stacks_title_query_items() {
        let p = make_palette(
            "Commands",
            "open",
            (0..3)
                .map(|i| make_palette_item(&format!("cmd{i}")))
                .collect(),
            0,
            0,
        );
        let layout = p.layout(40.0, 10.0, 1.0, 1.0, |_| PaletteItemMeasure::new(1.0));
        // Title at y=0 (h=1), query at y=1 (h=1), items at y=2,3,4.
        assert_eq!(layout.title_bounds.unwrap().y, 0.0);
        assert_eq!(layout.query_bounds.unwrap().y, 1.0);
        assert_eq!(layout.visible_items[0].bounds.y, 2.0);
        assert_eq!(layout.visible_items[2].bounds.y, 4.0);
        // Hit-tests.
        assert_eq!(layout.hit_test(10.0, 0.5), PaletteHit::Title);
        assert_eq!(layout.hit_test(10.0, 1.5), PaletteHit::Query);
        assert_eq!(layout.hit_test(10.0, 2.5), PaletteHit::Item(0));
    }

    #[test]
    fn palette_layout_no_title_query_only() {
        let p = make_palette("", "", vec![make_palette_item("a")], 0, 0);
        let layout = p.layout(40.0, 10.0, 0.0, 1.0, |_| PaletteItemMeasure::new(1.0));
        assert!(layout.title_bounds.is_none());
        assert_eq!(layout.query_bounds.unwrap().y, 0.0);
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
    }

    #[test]
    fn palette_layout_scroll_offset_skips_items() {
        let p = make_palette(
            "",
            "",
            (0..5)
                .map(|i| make_palette_item(&format!("i{i}")))
                .collect(),
            0,
            2,
        );
        let layout = p.layout(40.0, 10.0, 0.0, 1.0, |_| PaletteItemMeasure::new(1.0));
        // Query at y=0, items from offset 2.
        assert_eq!(layout.visible_items[0].item_idx, 2);
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
    }

    #[test]
    fn palette_layout_pixel_units() {
        // GTK-style: 32 px title, 40 px query, 24 px item rows.
        let p = make_palette(
            "Commands",
            "",
            (0..3)
                .map(|i| make_palette_item(&format!("c{i}")))
                .collect(),
            0,
            0,
        );
        let layout = p.layout(400.0, 300.0, 32.0, 40.0, |_| PaletteItemMeasure::new(24.0));
        assert_eq!(layout.title_bounds.unwrap().height, 32.0);
        assert_eq!(layout.query_bounds.unwrap().y, 32.0);
        assert_eq!(layout.query_bounds.unwrap().height, 40.0);
        assert_eq!(layout.visible_items[0].bounds.y, 72.0);
        assert_eq!(layout.visible_items[0].bounds.height, 24.0);
    }

    // ── D6 ActivityBar layout API tests ───────────────────────────────

    fn make_activity_item(id: &str, icon: char) -> primitives::activity_bar::ActivityItem {
        primitives::activity_bar::ActivityItem {
            id: WidgetId::new(id),
            icon: icon.to_string(),
            tooltip: String::new(),
            is_active: false,
            is_keyboard_selected: false,
        }
    }

    #[test]
    fn activity_bar_layout_empty() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
        };
        let layout = bar.layout(3.0, 20.0, 1.0);
        assert_eq!(layout.visible_items.len(), 0);
        assert_eq!(layout.hit_test(1.0, 5.0), ActivityBarHit::Empty);
    }

    #[test]
    fn activity_bar_layout_top_only() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![
                make_activity_item("activity:explorer", 'E'),
                make_activity_item("activity:search", 'S'),
            ],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
        };
        let layout = bar.layout(3.0, 10.0, 1.0);
        assert_eq!(layout.visible_items.len(), 2);
        assert_eq!(layout.visible_items[0].side, ActivitySide::Top);
        assert_eq!(layout.visible_items[0].bounds.y, 0.0);
        assert_eq!(layout.visible_items[1].bounds.y, 1.0);
        match layout.hit_test(1.0, 0.5) {
            ActivityBarHit::Item(id) => assert_eq!(id.as_str(), "activity:explorer"),
            _ => panic!("expected explorer hit"),
        }
    }

    #[test]
    fn activity_bar_layout_bottom_pinned() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![make_activity_item("activity:settings", 'G')],
            active_accent: None,
            selection_bg: None,
        };
        // Viewport 10, items 1 each. Top at y=0, bottom at y=9.
        let layout = bar.layout(3.0, 10.0, 1.0);
        assert_eq!(layout.visible_items.len(), 2);
        let top = layout
            .visible_items
            .iter()
            .find(|v| v.side == ActivitySide::Top)
            .unwrap();
        let bot = layout
            .visible_items
            .iter()
            .find(|v| v.side == ActivitySide::Bottom)
            .unwrap();
        assert_eq!(top.bounds.y, 0.0);
        assert_eq!(bot.bounds.y, 9.0);
        // Click near top → explorer. Click near bottom → settings.
        match layout.hit_test(1.0, 0.5) {
            ActivityBarHit::Item(id) => assert_eq!(id.as_str(), "activity:explorer"),
            _ => panic!(),
        }
        match layout.hit_test(1.0, 9.5) {
            ActivityBarHit::Item(id) => assert_eq!(id.as_str(), "activity:settings"),
            _ => panic!(),
        }
    }

    #[test]
    fn activity_bar_layout_bottom_wins_on_collision() {
        // 5 top items + 3 bottom items, item_height=1, viewport=6.
        // Bottom reserves [3, 6). Top stops at y=3 → only 3 top items fit.
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: (0..5)
                .map(|i| make_activity_item(&format!("top:{i}"), 'T'))
                .collect(),
            bottom_items: (0..3)
                .map(|i| make_activity_item(&format!("bot:{i}"), 'B'))
                .collect(),
            active_accent: None,
            selection_bg: None,
        };
        let layout = bar.layout(3.0, 6.0, 1.0);
        let top_count = layout
            .visible_items
            .iter()
            .filter(|v| v.side == ActivitySide::Top)
            .count();
        let bot_count = layout
            .visible_items
            .iter()
            .filter(|v| v.side == ActivitySide::Bottom)
            .count();
        assert_eq!(bot_count, 3, "all bottom items visible");
        assert_eq!(
            top_count, 3,
            "top truncated to fit above bottom reserved area"
        );
    }

    #[test]
    fn activity_bar_layout_pixel_units() {
        // GTK-style: 48 px item height, 200 px strip.
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: (0..3)
                .map(|i| make_activity_item(&format!("top:{i}"), 'T'))
                .collect(),
            bottom_items: vec![make_activity_item("activity:settings", 'G')],
            active_accent: None,
            selection_bg: None,
        };
        let layout = bar.layout(48.0, 200.0, 48.0);
        // Top items at y = 0, 48, 96. Settings at y = 200 - 48 = 152.
        let top0 = layout
            .visible_items
            .iter()
            .find(|v| v.side == ActivitySide::Top && v.item_idx == 0)
            .unwrap();
        assert_eq!(top0.bounds.y, 0.0);
        assert_eq!(top0.bounds.height, 48.0);
        let bot0 = layout
            .visible_items
            .iter()
            .find(|v| v.side == ActivitySide::Bottom)
            .unwrap();
        assert_eq!(bot0.bounds.y, 152.0);
    }

    // ── D6 ListView layout API tests ──────────────────────────────────

    fn make_list_item(text: &str) -> primitives::list::ListItem {
        primitives::list::ListItem {
            text: StyledText::plain(text),
            icon: None,
            detail: None,
            decoration: Decoration::Normal,
        }
    }

    fn make_list(
        title: Option<&str>,
        items: Vec<primitives::list::ListItem>,
        selected: usize,
        scroll: usize,
    ) -> ListView {
        ListView {
            id: WidgetId::new("l"),
            title: title.map(StyledText::plain),
            items,
            selected_idx: selected,
            scroll_offset: scroll,
            has_focus: true,
            bordered: false,
        }
    }

    #[test]
    fn list_view_layout_empty() {
        let list = make_list(None, vec![], 0, 0);
        let layout = list.layout(40.0, 10.0, 0.0, |_| ListItemMeasure::new(1.0));
        assert_eq!(layout.visible_items.len(), 0);
        assert!(layout.title_bounds.is_none());
        assert_eq!(layout.hit_test(5.0, 5.0), ListViewHit::Empty);
    }

    #[test]
    fn list_view_layout_title_reserves_first_row() {
        let list = make_list(
            Some("QUICKFIX"),
            (0..3)
                .map(|i| make_list_item(&format!("item{i}")))
                .collect(),
            0,
            0,
        );
        let layout = list.layout(40.0, 10.0, 1.0, |_| ListItemMeasure::new(1.0));
        assert!(layout.title_bounds.is_some());
        let tb = layout.title_bounds.unwrap();
        assert_eq!(tb.y, 0.0);
        assert_eq!(tb.height, 1.0);
        // Items start at y=1 (after title).
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
        assert_eq!(layout.visible_items[0].item_idx, 0);
        // Click on title → ListViewHit::Title.
        assert_eq!(layout.hit_test(10.0, 0.5), ListViewHit::Title);
        // Click on first item row.
        assert_eq!(layout.hit_test(10.0, 1.5), ListViewHit::Item(0));
    }

    #[test]
    fn list_view_layout_no_title_starts_at_zero() {
        let list = make_list(
            None,
            (0..2).map(|i| make_list_item(&format!("i{i}"))).collect(),
            0,
            0,
        );
        let layout = list.layout(40.0, 10.0, 0.0, |_| ListItemMeasure::new(1.0));
        assert!(layout.title_bounds.is_none());
        assert_eq!(layout.visible_items[0].bounds.y, 0.0);
        assert_eq!(layout.hit_test(10.0, 0.5), ListViewHit::Item(0));
    }

    #[test]
    fn list_view_layout_scroll_offset_skips_items_not_title() {
        let list = make_list(
            Some("HEADER"),
            (0..5).map(|i| make_list_item(&format!("i{i}"))).collect(),
            0,
            2, // skip first 2 items
        );
        let layout = list.layout(40.0, 10.0, 1.0, |_| ListItemMeasure::new(1.0));
        // Title still pinned at top.
        assert_eq!(layout.title_bounds.unwrap().y, 0.0);
        // First visible item is items[2].
        assert_eq!(layout.visible_items[0].item_idx, 2);
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
    }

    #[test]
    fn list_view_layout_viewport_overflow_clips_last() {
        let list = make_list(
            None,
            (0..10).map(|i| make_list_item(&format!("i{i}"))).collect(),
            0,
            0,
        );
        // 10 items × 2.0; viewport 5.0 → 3 rows fit (last clipped to 1.0).
        let layout = list.layout(40.0, 5.0, 0.0, |_| ListItemMeasure::new(2.0));
        assert_eq!(layout.visible_items.len(), 3);
        assert_eq!(layout.visible_items[2].bounds.height, 1.0);
    }

    #[test]
    fn list_view_layout_pixel_units_with_title() {
        // GTK-style: title row 20 px, items 18.5 px each.
        let list = make_list(
            Some("DIAGNOSTICS"),
            (0..5).map(|i| make_list_item(&format!("d{i}"))).collect(),
            0,
            0,
        );
        let layout = list.layout(300.0, 100.0, 20.0, |_| ListItemMeasure::new(18.5));
        let tb = layout.title_bounds.unwrap();
        assert_eq!(tb.height, 20.0);
        // First item starts at y=20.
        assert_eq!(layout.visible_items[0].bounds.y, 20.0);
        assert_eq!(layout.visible_items[0].bounds.height, 18.5);
        // Hit-test lands on correct row with fractional coords.
        assert_eq!(layout.hit_test(100.0, 29.0), ListViewHit::Item(0));
        assert_eq!(layout.hit_test(100.0, 39.0), ListViewHit::Item(1));
    }

    #[test]
    fn list_view_layout_bordered_insets_items() {
        // Bordered: items inset by 1 cell on each side, viewport
        // height reduced by 2 (top + bottom border rows). Title (when
        // present) overlays the top border, so item area starts at y=1.
        let mut list = make_list(
            Some("Open Tabs"),
            (0..3).map(|i| make_list_item(&format!("tab{i}"))).collect(),
            0,
            0,
        );
        list.bordered = true;
        let layout = list.layout(20.0, 6.0, 1.0, |_| ListItemMeasure::new(1.0));
        // Title overlay covers the full top border row (y=0).
        let tb = layout.title_bounds.unwrap();
        assert_eq!(tb.y, 0.0);
        assert_eq!(tb.width, 20.0);
        // Items inset by 1 cell horizontally, start at y=1.
        let i0 = layout.visible_items[0].bounds;
        assert_eq!(i0.x, 1.0);
        assert_eq!(i0.y, 1.0);
        assert_eq!(i0.width, 18.0);
        // Bottom row (y=5) is reserved for the border — only 3 item
        // rows fit between y=1 and y=5 (inclusive of y=4).
        assert!(layout.visible_items.iter().all(|v| v.bounds.y < 5.0));
    }

    #[test]
    fn list_view_layout_bordered_no_title_starts_at_one() {
        let mut list = make_list(
            None,
            (0..3).map(|i| make_list_item(&format!("r{i}"))).collect(),
            0,
            0,
        );
        list.bordered = true;
        let layout = list.layout(10.0, 6.0, 0.0, |_| ListItemMeasure::new(1.0));
        assert!(layout.title_bounds.is_none());
        // Without title, items still start at y=1 (top border).
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
    }

    // ── D6 TreeView layout API tests ──────────────────────────────────

    fn make_tree_row(path: &[u16], indent: u16, label: &str) -> primitives::tree::TreeRow {
        primitives::tree::TreeRow {
            path: path.to_vec(),
            indent,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
        }
    }

    fn make_tree(rows: Vec<primitives::tree::TreeRow>, scroll: usize) -> TreeView {
        TreeView {
            id: WidgetId::new("t"),
            rows,
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: scroll,
            style: TreeStyle::default(),
            has_focus: true,
        }
    }

    #[test]
    fn tree_view_layout_empty() {
        let tree = make_tree(vec![], 0);
        let layout = tree.layout(40.0, 20.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.visible_rows.len(), 0);
        assert_eq!(layout.hit_regions.len(), 0);
        assert_eq!(layout.resolved_scroll_offset, 0);
        assert_eq!(layout.hit_test(5.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_all_rows_fit() {
        let tree = make_tree(
            (0..3)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].bounds.y, 0.0);
        assert_eq!(layout.visible_rows[1].bounds.y, 1.0);
        assert_eq!(layout.visible_rows[2].bounds.y, 2.0);
        // Hit-test each row by y coord.
        assert_eq!(layout.hit_test(10.0, 0.5), TreeViewHit::Row(0));
        assert_eq!(layout.hit_test(10.0, 1.5), TreeViewHit::Row(1));
        assert_eq!(layout.hit_test(10.0, 2.5), TreeViewHit::Row(2));
        // Below last row → Empty.
        assert_eq!(layout.hit_test(10.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_scroll_offset_applies() {
        let tree = make_tree(
            (0..5)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            2, // skip first 2
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].row_idx, 2);
        assert_eq!(layout.visible_rows[1].row_idx, 3);
        assert_eq!(layout.visible_rows[2].row_idx, 4);
    }

    #[test]
    fn tree_view_layout_viewport_overflow_clips() {
        // 10 rows of height 2.0 each; viewport 5.0 tall → only 3 rows
        // fit (one partially clipped).
        let tree = make_tree(
            (0..10)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(40.0, 5.0, |_| TreeRowMeasure::new(2.0));
        // Rows 0 (y=0..2), 1 (y=2..4), 2 (y=4..5 clipped) — three visible.
        assert_eq!(layout.visible_rows.len(), 3);
        // Last row clipped to height 1.0 (remaining = 5 - 4 = 1).
        assert_eq!(layout.visible_rows[2].bounds.height, 1.0);
        // A click below all visible rows returns Empty.
        assert_eq!(layout.hit_test(10.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_varying_row_heights() {
        // Branches (is_expanded != None) get height 1.4 * base; leaves get
        // 1.0. Proves the measurer can consult row state.
        let mut rows = vec![
            make_tree_row(&[0], 0, "branch"),
            make_tree_row(&[0, 0], 1, "leaf0"),
            make_tree_row(&[0, 1], 1, "leaf1"),
        ];
        rows[0].is_expanded = Some(true);
        let tree = make_tree(rows.clone(), 0);
        let layout = tree.layout(40.0, 10.0, |i| {
            let h = if rows[i].is_expanded.is_some() {
                1.4
            } else {
                1.0
            };
            TreeRowMeasure::new(h)
        });
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].bounds.height, 1.4);
        assert_eq!(layout.visible_rows[1].bounds.y, 1.4);
        assert_eq!(layout.visible_rows[1].bounds.height, 1.0);
        assert!((layout.visible_rows[2].bounds.y - 2.4).abs() < 0.001);
    }

    #[test]
    fn tree_view_layout_pixel_units_fractional() {
        // GTK-style: line_height = 18.5 px leaf, 25.9 px branch. Proves
        // fractional row heights flow through correctly.
        let tree = make_tree(
            (0..5)
                .map(|i| make_tree_row(&[i], 0, &format!("r{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(300.0, 60.0, |_| TreeRowMeasure::new(18.5));
        // 3 full rows fit (55.5), 4th starts at y=55.5 and gets clipped to 4.5 px.
        assert_eq!(layout.visible_rows.len(), 4);
        assert!((layout.visible_rows[3].bounds.height - 4.5).abs() < 0.001);
    }

    #[test]
    fn tree_view_layout_scroll_offset_clamped() {
        // scroll_offset beyond rows.len() — resolved to rows.len()-1 so
        // the single remaining row is visible.
        let tree = make_tree(
            (0..3)
                .map(|i| make_tree_row(&[i], 0, &format!("r{i}")))
                .collect(),
            99,
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.visible_rows.len(), 1);
        assert_eq!(layout.visible_rows[0].row_idx, 2);
    }

    // ── D6 StatusBar layout API tests ─────────────────────────────────

    fn make_status_seg(
        text: &str,
        id: Option<&str>,
        bold: bool,
    ) -> primitives::status_bar::StatusBarSegment {
        primitives::status_bar::StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(255, 255, 255),
            bg: Color::rgb(30, 30, 30),
            bold,
            action_id: id.map(WidgetId::new),
        }
    }

    #[test]
    fn status_bar_layout_empty() {
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![],
            right_segments: vec![],
        };
        let layout = bar.layout(30.0, 1.0, 2.0, |_| StatusSegmentMeasure::new(0.0));
        assert_eq!(layout.visible_segments.len(), 0);
        assert_eq!(layout.hit_regions.len(), 0);
        assert_eq!(layout.resolved_right_start, 0);
        assert_eq!(layout.hit_test(5.0, 0.5), StatusBarHit::Empty);
    }

    #[test]
    fn status_bar_layout_left_only() {
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![
                make_status_seg(" NORMAL ", None, true),
                make_status_seg(" main.rs", Some("filename"), false),
            ],
            right_segments: vec![],
        };
        let layout = bar.layout(50.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        assert_eq!(layout.visible_segments.len(), 2);
        assert_eq!(layout.visible_segments[0].bounds.x, 0.0);
        assert_eq!(layout.visible_segments[0].bounds.width, 8.0); // " NORMAL "
        assert_eq!(layout.visible_segments[0].side, StatusSegmentSide::Left);
        assert!(!layout.visible_segments[0].clickable);
        assert_eq!(layout.visible_segments[1].bounds.x, 8.0);
        assert_eq!(layout.visible_segments[1].side, StatusSegmentSide::Left);
        assert!(layout.visible_segments[1].clickable);

        // Click on non-clickable → Empty. Click on clickable → the id.
        assert_eq!(layout.hit_test(3.0, 0.5), StatusBarHit::Empty);
        match layout.hit_test(10.0, 0.5) {
            StatusBarHit::Segment(id) => assert_eq!(id.as_str(), "filename"),
            other => panic!("expected Segment(filename), got {other:?}"),
        }
    }

    #[test]
    fn status_bar_layout_right_aligned() {
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg(" NORMAL", None, true)],
            right_segments: vec![
                make_status_seg(" rust ", Some("lang"), false),
                make_status_seg(" Ln 1,Col 1 ", Some("cursor"), false),
            ],
        };
        // Bar 40 chars. Right segs total 18; left 7; gap min 2. 7+2+18=27<=40.
        // No drop. Right starts at 40 - 18 = 22.
        let layout = bar.layout(40.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        assert_eq!(layout.resolved_right_start, 0);
        assert_eq!(layout.visible_segments.len(), 3);
        // Right side starts at bar_width - total_visible_right = 40 - 18 = 22
        let lang = &layout.visible_segments[1];
        assert_eq!(lang.side, StatusSegmentSide::Right);
        assert_eq!(lang.bounds.x, 22.0);
        assert_eq!(lang.bounds.width, 6.0);
        let cursor = &layout.visible_segments[2];
        assert_eq!(cursor.bounds.x, 28.0);

        // Hit-test the right-side cursor segment.
        match layout.hit_test(30.0, 0.5) {
            StatusBarHit::Segment(id) => assert_eq!(id.as_str(), "cursor"),
            other => panic!("expected Segment(cursor), got {other:?}"),
        }
    }

    #[test]
    fn status_bar_layout_priority_drop() {
        // Right segments ordered least-important first. A narrow bar should
        // drop the low-priority ones and preserve the cursor segment.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg(" LEFT", None, false)],
            right_segments: vec![
                make_status_seg(" a ", Some("lo0"), false),            // 3
                make_status_seg(" b ", Some("lo1"), false),            // 3
                make_status_seg(" c ", Some("lo2"), false),            // 3
                make_status_seg(" Ln 1,Col 1", Some("cursor"), false), // 11
            ],
        };
        // bar=20, left=5, gap=2 → max_right=13. Sum=20 > 13. Drop lo0 (3).
        // Remaining 17 > 13. Drop lo1. Remaining 14 > 13. Drop lo2. Remaining 11 ≤ 13.
        // resolved_right_start = 3 (cursor only).
        let layout = bar.layout(20.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        assert_eq!(layout.resolved_right_start, 3);
        // Visible: 1 left + 1 right = 2
        assert_eq!(layout.visible_segments.len(), 2);
        let surviving_right = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Right)
            .unwrap();
        assert_eq!(surviving_right.segment_idx, 3);
        assert_eq!(surviving_right.bounds.width, 11.0);

        // Hit-test the dropped-segment columns: no action fires.
        assert_eq!(layout.hit_test(7.0, 0.5), StatusBarHit::Empty);
    }

    #[test]
    fn status_bar_layout_pixel_units_fractional() {
        // Native-style measurement: fractional pixel widths, proportional
        // font. Proves the unit-agnostic contract (north-star goal).
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg("NORMAL", None, true)],
            right_segments: vec![make_status_seg("Ln 1,Col 1", Some("cursor"), false)],
        };
        // Non-uniform widths — pretend each char is ~7.3 px average, bold +5.
        let measure = |seg: &StatusBarSegment| {
            let w = seg.text.chars().count() as f32 * 7.3 + if seg.bold { 5.0 } else { 0.0 };
            StatusSegmentMeasure::new(w)
        };
        let layout = bar.layout(400.0, 22.0, 16.0, measure);
        assert_eq!(layout.resolved_right_start, 0);
        assert_eq!(layout.visible_segments.len(), 2);
        assert_eq!(layout.visible_segments[0].side, StatusSegmentSide::Left);
        assert_eq!(layout.visible_segments[0].bounds.x, 0.0);
        assert!((layout.visible_segments[0].bounds.width - (6.0 * 7.3 + 5.0)).abs() < 0.01);
        // Right segment right-aligned.
        let right = &layout.visible_segments[1];
        let right_w = 10.0 * 7.3;
        assert!((right.bounds.x - (400.0 - right_w)).abs() < 0.01);
    }

    #[test]
    fn status_bar_layout_always_keeps_last_right_segment() {
        // Even if the last (highest-priority) segment alone doesn't fit,
        // the layout keeps it rather than rendering an empty right half.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg("LEFT_MORE", None, false)],
            right_segments: vec![make_status_seg("cursor_info", Some("cursor"), false)],
        };
        // bar=10, left=9, gap=2 → max_right=0 (well, negative → clamped to 0).
        // Single segment, alone overflow → keep it anyway.
        let layout = bar.layout(10.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        assert_eq!(layout.resolved_right_start, 0);
        let r = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Right);
        assert!(
            r.is_some(),
            "last segment should survive even when too wide"
        );
    }

    // ── D6 layout API tests (per-primitive Layout + hit_test) ────────────
    //
    // These exercise the unit-agnostic contract: a TUI-style measurer
    // (char counts, integer-valued f32) and a pixel-style measurer
    // (fractional f32 from proportional-font metrics) must both produce
    // consistent layouts. The cross-backend correctness of this
    // abstraction is the whole point — see north-star goal in PLAN.md.

    fn make_tab(label: &str, is_active: bool) -> primitives::tab_bar::TabItem {
        primitives::tab_bar::TabItem {
            label: label.to_string(),
            is_active,
            is_dirty: false,
            is_preview: false,
        }
    }

    fn make_bar(tabs: Vec<primitives::tab_bar::TabItem>) -> TabBar {
        TabBar {
            id: WidgetId::new("t"),
            tabs,
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
        }
    }

    #[test]
    fn tab_bar_layout_empty() {
        let bar = make_bar(vec![]);
        let layout = bar.layout(
            100.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 0);
        assert_eq!(layout.visible_segments.len(), 0);
        assert!(layout.scroll_left.is_none());
        assert!(layout.scroll_right.is_none());
        assert_eq!(layout.hit_regions.len(), 0);
        assert_eq!(layout.resolved_scroll_offset, 0);
        assert_eq!(layout.hit_test(5.0, 1.0), TabBarHit::Empty);
    }

    #[test]
    fn tab_bar_layout_single_tab_fits() {
        let bar = make_bar(vec![make_tab("main.rs", true)]);
        // Width 20; tab is 10 wide; plenty of room; no scroll arrows needed.
        let layout = bar.layout(
            20.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 2.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 1);
        assert_eq!(layout.visible_tabs[0].tab_idx, 0);
        assert_eq!(layout.visible_tabs[0].bounds.x, 0.0);
        assert_eq!(layout.visible_tabs[0].bounds.width, 10.0);
        assert!(layout.visible_tabs[0].close_bounds.is_some());
        let cb = layout.visible_tabs[0].close_bounds.unwrap();
        assert_eq!(cb.x, 8.0); // 10 - 2
        assert_eq!(cb.width, 2.0);
        assert!(layout.scroll_left.is_none());
        assert!(layout.scroll_right.is_none());

        // Hit-test: click on tab body returns Tab(0).
        assert_eq!(layout.hit_test(5.0, 1.0), TabBarHit::Tab(0));
        // Click on close area returns TabClose(0), not Tab(0).
        assert_eq!(layout.hit_test(9.0, 1.0), TabBarHit::TabClose(0));
        // Click past the tab returns Empty.
        assert_eq!(layout.hit_test(15.0, 1.0), TabBarHit::Empty);
    }

    #[test]
    fn tab_bar_layout_tui_char_units() {
        // Classic TUI scenario: 3 tabs each 10 cells wide; bar is 30 cells.
        // All fit, no scroll.
        let bar = make_bar(vec![
            make_tab("a", true),
            make_tab("b", false),
            make_tab("c", false),
        ]);
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 3);
        assert_eq!(layout.visible_tabs[0].bounds.x, 0.0);
        assert_eq!(layout.visible_tabs[1].bounds.x, 10.0);
        assert_eq!(layout.visible_tabs[2].bounds.x, 20.0);
        assert!(layout.scroll_left.is_none());
        assert!(layout.scroll_right.is_none());
        assert_eq!(layout.resolved_scroll_offset, 0);
    }

    #[test]
    fn tab_bar_layout_pixel_units_fractional() {
        // Proves the unit-agnostic contract: fractional pixel widths (as
        // Pango would return) produce a consistent layout. Same 3 tabs but
        // each measured at 87.5 px; bar is 400 px.
        let bar = make_bar(vec![
            make_tab("a", true),
            make_tab("b", false),
            make_tab("c", false),
        ]);
        let layout = bar.layout(
            400.0,
            22.0,
            16.0,
            |_| TabMeasure::new(87.5, 18.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 3);
        assert_eq!(layout.visible_tabs[0].bounds.x, 0.0);
        assert_eq!(layout.visible_tabs[1].bounds.x, 87.5);
        assert_eq!(layout.visible_tabs[2].bounds.x, 175.0);
        assert_eq!(layout.visible_tabs[0].bounds.height, 22.0);
        let cb = layout.visible_tabs[0].close_bounds.unwrap();
        assert_eq!(cb.x, 87.5 - 18.0);
        assert_eq!(cb.width, 18.0);
    }

    #[test]
    fn tab_bar_layout_overflow_active_in_middle() {
        // 10 tabs × 10 cells; bar 30 cells → ~2 tabs fit after reserving
        // 2*2=4 cells for scroll arrows. Active is tab 5 (middle). Both
        // scroll arrows should appear.
        let bar = make_bar(
            (0..10)
                .map(|i| make_tab(&format!("t{i}"), i == 5))
                .collect(),
        );
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(layout.scroll_left.is_some());
        assert!(layout.scroll_right.is_some());
        // Active tab must be among visible.
        let visible_indices: Vec<usize> = layout.visible_tabs.iter().map(|v| v.tab_idx).collect();
        assert!(
            visible_indices.contains(&5),
            "active tab 5 not visible: {:?}",
            visible_indices
        );
        // Scroll offset > 0 (we walked back from the active tab).
        assert!(layout.resolved_scroll_offset > 0);
        // First visible tab starts after the left arrow.
        assert_eq!(layout.visible_tabs[0].bounds.x, 2.0);
    }

    #[test]
    fn tab_bar_layout_overflow_active_at_start() {
        // 10 tabs × 10; bar 30; active is tab 0. Only right arrow needed.
        let bar = make_bar(
            (0..10)
                .map(|i| make_tab(&format!("t{i}"), i == 0))
                .collect(),
        );
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(layout.scroll_left.is_none()); // offset 0
        assert!(layout.scroll_right.is_some()); // tabs to the right
        assert_eq!(layout.resolved_scroll_offset, 0);
    }

    #[test]
    fn tab_bar_layout_overflow_active_at_end() {
        // 10 tabs × 10; bar 30; active is tab 9 (last). Only left arrow.
        let bar = make_bar(
            (0..10)
                .map(|i| make_tab(&format!("t{i}"), i == 9))
                .collect(),
        );
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(layout.scroll_left.is_some());
        assert!(layout.scroll_right.is_none()); // nothing past active
        assert!(layout.resolved_scroll_offset > 0);
        // Active tab is among visible.
        assert!(layout.visible_tabs.iter().any(|v| v.tab_idx == 9));
    }

    #[test]
    fn tab_bar_layout_right_segments_fit() {
        use primitives::tab_bar::TabBarSegment;
        let bar = TabBar {
            id: WidgetId::new("t"),
            tabs: vec![make_tab("a", true)],
            scroll_offset: 0,
            right_segments: vec![
                TabBarSegment {
                    text: " ← ".to_string(),
                    width_cells: 3,
                    id: Some(WidgetId::new("prev")),
                    is_active: false,
                },
                TabBarSegment {
                    text: " → ".to_string(),
                    width_cells: 3,
                    id: Some(WidgetId::new("next")),
                    is_active: false,
                },
            ],
            active_accent: None,
        };
        // Bar 100 wide. Tab 10 wide. Right segments: 6 wide total.
        let layout = bar.layout(
            100.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |i| {
                let w = [3.0, 3.0][i];
                SegmentMeasure::new(w)
            },
        );
        assert_eq!(layout.visible_segments.len(), 2);
        // Right segments start at bar_width - total_width = 100 - 6 = 94.
        assert_eq!(layout.visible_segments[0].bounds.x, 94.0);
        assert_eq!(layout.visible_segments[1].bounds.x, 97.0);
        // Hit-test on first segment.
        let hit = layout.hit_test(95.0, 1.0);
        match hit {
            TabBarHit::RightSegment(id) => assert_eq!(id.as_str(), "prev"),
            other => panic!("expected RightSegment(prev), got {other:?}"),
        }
    }

    #[test]
    fn tab_bar_layout_right_segments_dropped_when_too_wide() {
        use primitives::tab_bar::TabBarSegment;
        let bar = TabBar {
            id: WidgetId::new("t"),
            tabs: vec![make_tab("a", true)],
            scroll_offset: 0,
            right_segments: vec![TabBarSegment {
                text: "many many pixels".to_string(),
                width_cells: 60,
                id: Some(WidgetId::new("huge")),
                is_active: false,
            }],
            active_accent: None,
        };
        // Bar 50 wide. Segment is 60 wide; literally doesn't fit → drop.
        let layout = bar.layout(
            50.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(60.0),
        );
        assert_eq!(layout.visible_segments.len(), 0);
        // No hit-region for the dropped segment.
        for (_, hit) in &layout.hit_regions {
            assert!(!matches!(hit, TabBarHit::RightSegment(_)));
        }

        // But a segment that fits, even narrowly, renders. 60 ≤ 100 → keep.
        let bar2 = bar.clone();
        let layout2 = bar2.layout(
            100.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(60.0),
        );
        assert_eq!(layout2.visible_segments.len(), 1);
    }

    #[test]
    fn tab_bar_layout_stale_scroll_offset_corrected() {
        // App stored scroll_offset 0 but active is tab 9 — layout should
        // correct by returning a non-zero `resolved_scroll_offset`. This is
        // the "write this back to storage" signal for the two-pass-paint
        // pattern.
        let tabs: Vec<_> = (0..10)
            .map(|i| make_tab(&format!("t{i}"), i == 9))
            .collect();
        let mut bar = make_bar(tabs);
        bar.scroll_offset = 0; // stale
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(
            layout.resolved_scroll_offset > 0,
            "expected correction but got {}",
            layout.resolved_scroll_offset
        );
    }

    #[test]
    fn tab_bar_layout_no_close_button_when_close_width_zero() {
        // A pinned / preview tab style — backend passes close_width = 0
        // to suppress the close button.
        let bar = make_bar(vec![make_tab("pinned.rs", true)]);
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 0.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 1);
        assert!(layout.visible_tabs[0].close_bounds.is_none());
        // Hit-test anywhere on the tab returns Tab(0), never TabClose.
        assert_eq!(layout.hit_test(5.0, 0.5), TabBarHit::Tab(0));
        assert_eq!(layout.hit_test(9.5, 0.5), TabBarHit::Tab(0));
    }

    #[test]
    fn tab_bar_layout_hit_test_outside_bar() {
        let bar = make_bar(vec![make_tab("a", true)]);
        let layout = bar.layout(
            20.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 2.0),
            |_| SegmentMeasure::new(0.0),
        );
        // Well past the bar.
        assert_eq!(layout.hit_test(100.0, 1.0), TabBarHit::Empty);
        // Below the bar.
        assert_eq!(layout.hit_test(5.0, 100.0), TabBarHit::Empty);
        // Negative coords (robust to weird backends).
        assert_eq!(layout.hit_test(-1.0, 1.0), TabBarHit::Empty);
    }

    #[test]
    fn tab_bar_layout_scroll_disabled_tabs_clip() {
        // scroll_arrow_width = 0.0 disables scroll arrows; tabs that don't
        // fit are silently clipped. Useful for backends that don't want
        // scroll affordances yet.
        let bar = make_bar(
            (0..10)
                .map(|i| make_tab(&format!("t{i}"), i == 0))
                .collect(),
        );
        let layout = bar.layout(
            30.0,
            1.0,
            0.0, // scroll disabled
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        // Exactly 3 tabs fit (no arrow reservation).
        assert_eq!(layout.visible_tabs.len(), 3);
        assert!(layout.scroll_left.is_none());
        assert!(layout.scroll_right.is_none());
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
