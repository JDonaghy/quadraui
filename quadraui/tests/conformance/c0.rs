//! Tier C0 — auto-generated per-primitive paint smoke (quadraui#492,
//! epic #480 acceptance: "Compiles must stop implying renders").
//!
//! `Backend` gives several `draw_*`-adjacent methods a no-op (or `false`)
//! default so a new backend compiles before every optional surface is
//! wired up. That silence is the bug: a backend can implement every
//! `draw_*` method and still discard the caller's content (the macOS
//! `draw_diff_view` fake #492 names), and nothing in a green build says
//! so. C0 is the boot tier that closes that gap: for every primitive in
//! [`CASES`], `begin_frame` → draw → `end_frame` must not panic, and the
//! resulting frame must be **observable** — a non-empty
//! `FrameInventory::text_runs()` or a non-empty `zones()` — on every
//! registered backend.
//!
//! [`CASES`] *is* the generator: [`run`] is generic over
//! [`super::runner::DriverFactory`], so adding a backend costs nothing
//! here (same "one factory, no per-backend code" shape as
//! `fixtures::build`), and adding a primitive is one more entry in the
//! table rather than one more hand-written test function per backend.
//!
//! Deliberately weak, matching contract §5b (`tests/acceptance/ms-11/
//! contract.md`): this is "the primitive draws *something* attributable
//! to it", not a rendering assertion — C1/#491's scenario suite is where
//! layout and interaction get checked.
//!
//! [`CASES`] covers every `draw_*` method [`quadraui::Backend`] declares
//! (quadraui#492 review: a 15-of-38 sample let a backend with a
//! completely broken `draw_editor` — arguably the single highest-stakes
//! primitive in the library — pass this tier silently). That claim is not
//! just asserted in prose: `tests::cases_cover_every_draw_method_on_the_trait`
//! below parses `backend.rs`'s own source for every `fn draw_*` trait
//! item and fails if `CASES` and the trait ever disagree, in either
//! direction — a new primitive added to the trait without a matching
//! entry here, or a stale entry naming a method that no longer exists.

use quadraui::{
    compute_hunks, ActivityBar, ActivityItem, AppLogic, Backend, BadgeStatus, BoardCard,
    BoardColumn, BoardModel, CardBadge, Chart, ChartKind, Color, Column, ColumnAlign, ColumnWidth,
    CommandCenter, CommandLine, CompletionItem, CompletionItemMeasure, Completions, ContextMenu,
    ContextMenuItem, ContextMenuItemMeasure, ContextMenuPlacement, DataRow, DataTable, Decoration,
    Dialog, DialogButton, DialogMeasure, DiffEditability, DiffMode, DiffPane, DiffView,
    DropOverlay, Editor, EditorCursor, EditorCursorPos, EditorCursorShape, EditorLine, EditorStyle,
    EditorStyledSpan, FieldKind, FindReplacePanel, Form, FormField, ListItem, ListView, MenuBar,
    MenuBarItem, MessageList, MessageRow, MsvAxis, MultiSectionView, Palette, PaletteItem,
    PaletteMode, Panel, PipelineStage, PipelineView, PopupPlacement, ProgressBar, Reaction, Rect,
    RichTextPopup, RichTextPopupMeasure, ScrollAxis, ScrollMode, Scrollbar, Section, SectionBody,
    SectionHeader, SectionSize, SelectionMode, Series, SidebarPanel, Spinner, Split,
    SplitDirection, SplitTree, StageStatus, StatusBar, StatusBarSegment, StyledSpan, StyledText,
    TabBar, TabItem, Terminal, TerminalCell, TextDisplay, TextDisplayLine, TextInput, ToastCorner,
    ToastItem, ToastSeverity, ToastStack, Toolbar, ToolbarButton, ToolbarItemMeasure, Tooltip,
    TooltipBorder, TooltipMeasure, TooltipPlacement, TreeRow, TreeStyle, TreeView, UiEvent,
    WidgetId,
};

use super::runner::{DriverFactory, DynDriver};

/// Backend-neutral viewport, generous enough that no descriptor below
/// clips for want of room — a C0 smoke that failed on fixture size would
/// be reporting the fixture, not the backend.
const VIEWPORT: quadraui::testing::LogicalViewport =
    quadraui::testing::LogicalViewport::new(80, 24);

fn id(suffix: &str) -> WidgetId {
    WidgetId::new(format!("c0:{suffix}"))
}

/// One primitive's C0 case.
pub struct Case {
    /// The `Backend` method this case exercises — named in failure text
    /// so a red row points straight at the trait method to fix.
    pub method: &'static str,
    /// The text this descriptor hands the backend, or `None` for a
    /// chrome-only primitive that paints no text at all.
    pub needle: Option<&'static str>,
    /// Paints the descriptor into the given viewport rect, in the
    /// backend's own units (never literal cells or pixels).
    pub paint: fn(&mut dyn Backend, Rect),
}

/// One exemplar per primitive, built from public quadraui API only.
///
/// **Exhaustive, and mechanically held so**: every `fn draw_*` on
/// [`quadraui::Backend`] has exactly one entry here, and
/// [`tests::cases_cover_every_draw_method_on_the_trait`] fails if that
/// ever stops being true in either direction. Adding a primitive to the
/// trait is therefore one more entry here, not an optional one.
pub const CASES: &[Case] = &[
    Case {
        method: "draw_status_bar",
        needle: Some("c0stat"),
        paint: |b, area| {
            let lh = b.line_height();
            let bar = StatusBar {
                id: id("status-bar"),
                left_segments: vec![StatusBarSegment {
                    text: " c0stat ".to_string(),
                    fg: Color::rgb(220, 220, 220),
                    bg: Color::rgb(37, 37, 38),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let _ = b.draw_status_bar(Rect::new(0.0, 0.0, area.width, lh), &bar, None, None);
        },
    },
    Case {
        method: "draw_tab_bar",
        needle: Some("c0tabs"),
        paint: |b, area| {
            let lh = b.line_height();
            let bar = TabBar {
                id: id("tab-bar"),
                tabs: vec![TabItem {
                    label: " c0tabs ".to_string(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: false,
                }],
                scroll_offset: 0,
                right_segments: vec![],
                active_accent: None,
                show_tab_close: false,
                compact: false,
            };
            let _ = b.draw_tab_bar(Rect::new(0.0, 0.0, area.width, lh), &bar, None);
        },
    },
    Case {
        method: "draw_command_line",
        needle: Some("c0cmd"),
        paint: |b, area| {
            let lh = b.line_height();
            let cmd = CommandLine {
                id: id("command-line"),
                text: ":c0cmd".to_string(),
                cursor_offset: None,
                right_align: false,
            };
            b.draw_command_line(Rect::new(0.0, 0.0, area.width, lh), &cmd);
        },
    },
    Case {
        method: "draw_message_list",
        needle: Some("c0msg"),
        paint: |b, area| {
            let list = MessageList {
                id: id("message-list"),
                rows: vec![MessageRow::new("c0msg", Color::rgb(220, 220, 220), 0.0)],
                scroll_top: 0,
            };
            b.draw_message_list(area, &list);
        },
    },
    Case {
        method: "draw_text_display",
        needle: Some("c0text"),
        paint: |b, area| {
            let td = TextDisplay {
                id: id("text-display"),
                lines: vec![TextDisplayLine {
                    spans: vec![StyledSpan::plain("c0text")],
                    decoration: Decoration::Normal,
                    timestamp: None,
                }],
                scroll_offset: 0,
                auto_scroll: true,
                max_lines: 0,
                has_focus: false,
                title: None,
                show_scrollbar: false,
            };
            b.draw_text_display(area, &td);
        },
    },
    Case {
        method: "draw_list",
        needle: Some("c0list"),
        paint: |b, area| {
            let list = ListView {
                id: id("list"),
                title: None,
                items: vec![ListItem {
                    text: StyledText::plain("c0list"),
                    icon: None,
                    detail: None,
                    decoration: Decoration::Normal,
                }],
                selected_idx: 0,
                scroll_offset: 0,
                has_focus: false,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: false,
            };
            b.draw_list(area, &list);
        },
    },
    Case {
        method: "draw_tree",
        needle: Some("c0tree"),
        paint: |b, area| {
            let tree = TreeView {
                id: id("tree"),
                rows: vec![TreeRow {
                    path: vec![0],
                    indent: 0,
                    icon: None,
                    text: StyledText::plain("c0tree"),
                    badge: None,
                    is_expanded: None,
                    decoration: Decoration::Normal,
                    edit: None,
                }],
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: TreeStyle::default(),
                has_focus: false,
            };
            b.draw_tree(area, &tree);
        },
    },
    Case {
        method: "draw_pipeline_view",
        needle: Some("c0pipe"),
        paint: |b, area| {
            let view = PipelineView {
                id: id("pipeline"),
                stages: vec![PipelineStage {
                    label: "c0pipe".to_string(),
                    status: StageStatus::Active,
                    action: None,
                }],
                focused_stage: None,
            };
            let _ = b.draw_pipeline_view(area, &view);
        },
    },
    Case {
        method: "draw_diff_view",
        needle: Some("c0diff"),
        paint: |b, area| {
            // The needle lives in the row content, not a pane label, so a
            // backend that paints the chrome and drops the diff body
            // (the macOS fake #492 names) still fails.
            let left = "c0diff-old".to_string();
            let right = "c0diff-new".to_string();
            let hunks = compute_hunks(&left, &right);
            let view = DiffView {
                id: id("diff-view"),
                left,
                right,
                left_label: None,
                right_label: None,
                hunks,
                mode: DiffMode::Unified,
                editability: DiffEditability::ReadOnly,
                scroll_offset: 0,
                focused_pane: DiffPane::Left,
                has_focus: false,
            };
            let _ = b.draw_diff_view(area, &view);
        },
    },
    Case {
        method: "draw_tooltip",
        needle: Some("c0tip"),
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            let anchor = Rect::new(0.0, 0.0, area.width, lh);
            let tooltip =
                Tooltip::new(id("tooltip"), "c0tip").with_placement(TooltipPlacement::Bottom);
            // Room for a border on all sides plus the whole label — a box
            // measured to exactly `border + text + border` leaves no
            // padding and clips the last glyph.
            let measure = TooltipMeasure::new(cw * 9.0, lh * 3.0);
            let layout = tooltip
                .layout(anchor, area, measure, lh)
                .with_border(TooltipBorder::default());
            b.draw_tooltip(&tooltip, &layout);
        },
    },
    Case {
        method: "draw_spinner",
        needle: Some("c0spin"),
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            let spinner = Spinner {
                id: id("spinner"),
                label: "c0spin".to_string(),
                frame_idx: 0,
                accent: None,
            };
            let _ = b.draw_spinner(Rect::new(0.0, 0.0, cw * 20.0, lh), &spinner);
            let _ = area;
        },
    },
    Case {
        method: "draw_progress",
        needle: Some("c0prog"),
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            let bar = ProgressBar {
                id: id("progress"),
                label: "c0prog".to_string(),
                value: Some(0.5),
                frame_idx: 0,
                cancellable: false,
                accent: None,
            };
            let _ = b.draw_progress(Rect::new(0.0, 0.0, cw * 30.0, lh * 2.0), &bar);
            let _ = area;
        },
    },
    // ── Chrome-only primitives — no text, so the only way the frame can
    // report them at all is a registered zone (contract §5b's sharp end).
    Case {
        method: "draw_scrollbar",
        needle: None,
        paint: |b, area| {
            let cw = b.char_width();
            let track = Rect::new(area.width - cw, 0.0, cw, area.height);
            let sb = Scrollbar {
                id: id("scrollbar"),
                axis: ScrollAxis::Vertical,
                track,
                thumb_start: 0.0,
                thumb_len: area.height / 2.0,
                hovered: false,
                dragging: false,
            };
            b.draw_scrollbar(track, &sb);
        },
    },
    Case {
        method: "draw_terminal_divider",
        needle: None,
        paint: |b, area| {
            let lh = b.line_height();
            b.draw_terminal_divider(Rect::new(0.0, lh * 2.0, area.width, lh));
        },
    },
    Case {
        method: "draw_drop_overlay",
        needle: None,
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            let overlay = DropOverlay {
                highlight: Some(Rect::new(cw * 2.0, lh * 2.0, cw * 20.0, lh * 6.0)),
                insertion_bar: None,
                ghost_position: None,
            };
            b.draw_drop_overlay(&overlay);
            let _ = area;
        },
    },
    // ── The rest of the trait surface (quadraui#492 review) ───────────
    //
    // The 15 cases above were the original sample; everything below was
    // added to close that gap. `tests::cases_cover_every_draw_method_on_
    // the_trait` (bottom of this file) keeps the two in sync from here on.
    Case {
        method: "draw_data_table",
        needle: Some("c0dtbl"),
        paint: |b, area| {
            let table = DataTable {
                id: id("data-table"),
                columns: vec![Column {
                    title: "c0dtbl".to_string(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                }],
                rows: vec![DataRow {
                    cells: vec![StyledText::plain("row")],
                    decoration: Decoration::Normal,
                }],
                selected_idx: None,
                scroll_offset: 0,
                sort: None,
                has_focus: false,
                show_scrollbar: false,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: vec![],
                footer: None,
            };
            let _ = b.draw_data_table(area, &table, None);
        },
    },
    Case {
        method: "draw_form",
        needle: Some("c0form"),
        paint: |b, area| {
            let form = Form {
                id: id("form"),
                fields: vec![FormField {
                    id: id("form-field"),
                    label: StyledText::plain("c0form"),
                    kind: FieldKind::ReadOnly {
                        value: StyledText::plain("value"),
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                }],
                focused_field: None,
                scroll_offset: 0,
                has_focus: false,
            };
            b.draw_form(area, &form);
        },
    },
    Case {
        method: "draw_palette",
        needle: Some("c0palt"),
        paint: |b, area| {
            let palette = Palette {
                id: id("palette"),
                title: "Palette".to_string(),
                query: String::new(),
                query_cursor: 0,
                items: vec![PaletteItem {
                    text: StyledText::plain("c0palt"),
                    detail: None,
                    icon: None,
                    match_positions: vec![],
                    depth: 0,
                    expandable: false,
                    expanded: false,
                }],
                selected_idx: 0,
                scroll_offset: 0,
                total_count: 0,
                has_focus: false,
                show_query: true,
                create_label: None,
                preview: None,
                mode: PaletteMode::List,
            };
            b.draw_palette(area, &palette);
        },
    },
    Case {
        method: "draw_settings_chrome",
        needle: Some("c0sett"),
        paint: |b, area| {
            b.draw_settings_chrome(area, "c0sett", "", "search…", false);
        },
    },
    Case {
        method: "draw_activity_bar",
        // TUI paints only the icon's *first* character
        // (`item.icon.chars().next()`), while GTK paints the whole
        // `icon` string — an inherent cross-backend asymmetry in this
        // primitive's own design, not a #492 gap, so no single needle
        // matches both verbatim. Observability still comes from the
        // icon glyph itself: a backend that painted nothing would leave
        // both `text_runs()` and `zones()` empty.
        needle: None,
        paint: |b, area| {
            let lh = b.line_height();
            let bar = ActivityBar {
                id: id("activity-bar"),
                top_items: vec![ActivityItem {
                    id: id("activity-item"),
                    icon: "c0acty".to_string(),
                    tooltip: String::new(),
                    is_active: true,
                    is_keyboard_selected: false,
                }],
                bottom_items: vec![],
                active_accent: None,
                selection_bg: None,
                is_keyboard_focused: false,
            };
            let _ = b.draw_activity_bar(Rect::new(0.0, 0.0, lh * 3.0, area.height), &bar, None);
        },
    },
    Case {
        method: "draw_terminal",
        // GTK's rasteriser paints cell glyphs via a raw
        // `pangocairo::functions::show_layout` call, bypassing the
        // `painted_text` tracking every other primitive's rasteriser
        // routes through (see the `#492` comment on
        // `GtkBackend::draw_terminal`) — so an exact-text needle can't
        // be asserted across backends today. Observability instead
        // comes from the zone `draw_terminal` now registers (TUI's
        // buffer scan sees the glyphs directly either way).
        needle: None,
        paint: |b, area| {
            let cell = |ch: char| TerminalCell {
                ch,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                italic: false,
                underline: false,
                selected: false,
                is_cursor: false,
                is_find_match: false,
                is_find_active: false,
            };
            let row: Vec<TerminalCell> = "c0term".chars().map(cell).collect();
            let term = Terminal {
                id: id("terminal"),
                cells: vec![row],
                scrollbar: None,
            };
            b.draw_terminal(area, &term);
        },
    },
    Case {
        method: "draw_text_input",
        needle: Some("c0tin"),
        paint: |b, area| {
            let ti = TextInput {
                id: id("text-input"),
                lines: vec!["c0tin".to_string()],
                cursor_line: 0,
                cursor_col: 0,
                placeholder: None,
                scroll_offset: 0,
                scroll_col: 0,
                has_focus: true,
            };
            let _ = b.draw_text_input(area, &ti);
        },
    },
    Case {
        method: "draw_context_menu",
        needle: Some("c0ctxm"),
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            let menu = ContextMenu {
                id: id("context-menu"),
                items: vec![ContextMenuItem {
                    id: Some(id("context-menu-item")),
                    label: StyledText::plain("c0ctxm"),
                    detail: None,
                    disabled: false,
                    key_equivalent: None,
                    checked: None,
                    submenu: None,
                }],
                selected_idx: 0,
                bg: None,
                placement: ContextMenuPlacement::AnchorPoint,
            };
            let layout = menu.layout(0.0, 0.0, area, cw * 20.0, |_| {
                ContextMenuItemMeasure::new(lh)
            });
            let _ = b.draw_context_menu(&menu, &layout);
        },
    },
    Case {
        method: "draw_dialog",
        needle: Some("c0dlgb"),
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            let dialog = Dialog {
                id: id("dialog"),
                title: StyledText::plain("Dialog"),
                body: vec![StyledText::plain("c0dlgb")],
                buttons: vec![DialogButton {
                    id: id("dialog-ok"),
                    label: "OK".to_string(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                }],
                severity: None,
                vertical_buttons: false,
                table: None,
                input: None,
            };
            let measure = DialogMeasure {
                width: cw * 40.0,
                title_height: lh,
                body_height: lh * 3.0,
                table_height: 0.0,
                input_height: 0.0,
                button_row_height: lh,
                button_width: cw * 10.0,
                button_gap: cw,
                padding: cw,
            };
            let layout = dialog.layout(area, measure, |_| ToolbarItemMeasure::new(0.0));
            let _ = b.draw_dialog(&dialog, &layout);
        },
    },
    Case {
        method: "draw_multi_section_view",
        needle: Some("c0msv"),
        paint: |b, area| {
            let view = MultiSectionView {
                id: id("msv"),
                sections: vec![Section {
                    id: "c0msv-section".to_string(),
                    header: SectionHeader {
                        title: StyledText::plain("c0msv"),
                        ..Default::default()
                    },
                    body: SectionBody::Text(vec![StyledText::plain("body")]),
                    aux: None,
                    size: SectionSize::EqualShare,
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                }],
                active_section: None,
                axis: MsvAxis::Vertical,
                allow_resize: false,
                allow_collapse: true,
                scroll_mode: ScrollMode::PerSection,
                has_focus: false,
                panel_scroll: 0.0,
            };
            b.draw_multi_section_view(area, &view);
        },
    },
    Case {
        method: "draw_editor",
        needle: Some("c0edit"),
        paint: |b, area| {
            let fg = Color::rgb(220, 220, 220);
            let text = "c0edit".to_string();
            let len = text.len();
            let line = EditorLine {
                raw_text: text,
                gutter_text: "   1".to_string(),
                spans: vec![EditorStyledSpan {
                    start_byte: 0,
                    end_byte: len,
                    style: EditorStyle {
                        fg,
                        bg: None,
                        bold: false,
                        italic: false,
                        font_scale: 1.0,
                    },
                }],
                line_idx: 0,
                is_current_line: true,
                is_fold_header: false,
                folded_line_count: 0,
                git_diff: None,
                diff_status: None,
                diagnostics: vec![],
                spell_errors: vec![],
                is_breakpoint: false,
                is_conditional_bp: false,
                is_dap_current: false,
                is_wrap_continuation: false,
                segment_col_offset: 0,
                annotation: None,
                ghost_suffix: None,
                is_ghost_continuation: false,
                indent_guides: vec![],
                colorcolumns: vec![],
            };
            let editor = Editor {
                id: id("editor"),
                rect: area,
                lines: vec![line],
                cursor: Some(EditorCursor {
                    pos: EditorCursorPos {
                        view_line: 0,
                        col: 0,
                    },
                    shape: EditorCursorShape::Block,
                }),
                extra_cursors: vec![],
                selection: None,
                extra_selections: vec![],
                yank_highlight: None,
                scroll_top: 0,
                scroll_left: 0,
                total_lines: 1,
                max_col: 6,
                gutter_char_width: 4,
                is_active: true,
                show_active_bg: false,
                has_git_diff: false,
                has_breakpoints: false,
                diagnostic_gutter: Default::default(),
                code_action_lines: Default::default(),
                bracket_match_positions: vec![],
                active_indent_col: None,
                tabstop: 4,
                cursorline: true,
                lightbulb_glyph: '\0',
            };
            let _ = b.draw_editor(area, &editor);
        },
    },
    Case {
        method: "draw_rich_text_popup",
        needle: Some("c0rtp"),
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            let popup = RichTextPopup {
                id: id("rich-text-popup"),
                lines: vec![StyledText::plain("c0rtp")],
                line_text: vec!["c0rtp".to_string()],
                line_scales: vec![],
                scroll_top: 0,
                max_visible_rows: 10,
                has_focus: true,
                selection: None,
                links: vec![],
                focused_link: None,
                placement: PopupPlacement::Below,
                padding: 1.0,
                fg: None,
                bg: None,
            };
            let measure = RichTextPopupMeasure::new(cw * 20.0, lh);
            let layout = popup.layout(0.0, 0.0, area, measure, |_, start, end| {
                (end - start) as f32 * cw
            });
            b.draw_rich_text_popup(&popup, &layout);
        },
    },
    Case {
        method: "draw_find_replace",
        needle: Some("c0find"),
        paint: |b, area| {
            // `hit_regions` isn't just click-dispatch metadata here — the
            // rasteriser paints the query/replace field *text* by walking
            // it (`paint_input` in `tui::find_replace`), so an empty list
            // paints a border with nothing inside. Must be the real
            // output of `compute_find_replace_hit_regions`, the same
            // helper a real caller uses.
            let (hit_regions, _input_width) =
                quadraui::compute_find_replace_hit_regions(40, false, "", 2, 2);
            let panel = FindReplacePanel {
                query: "c0find".to_string(),
                replacement: String::new(),
                show_replace: false,
                focus: 0,
                cursor: 6,
                sel_anchor: None,
                match_info: String::new(),
                case_sensitive: false,
                whole_word: false,
                use_regex: false,
                preserve_case: false,
                in_selection: false,
                group_bounds: area,
                panel_width: 40,
                replace_one_glyph: "R1".to_string(),
                replace_all_glyph: "R*".to_string(),
                hit_regions,
            };
            b.draw_find_replace(area, &panel);
        },
    },
    Case {
        method: "draw_completions",
        needle: Some("c0cmpl"),
        paint: |b, area| {
            let lh = b.line_height();
            let cw = b.char_width();
            let completions = Completions {
                id: id("completions"),
                items: vec![CompletionItem {
                    label: StyledText::plain("c0cmpl"),
                    detail: None,
                    documentation: None,
                    kind: Default::default(),
                    icon: None,
                }],
                selected_idx: 0,
                scroll_offset: 0,
                has_focus: true,
            };
            let layout = completions.layout(0.0, 0.0, lh, area, cw * 20.0, lh * 6.0, |_| {
                CompletionItemMeasure::new(lh)
            });
            b.draw_completions(&completions, &layout);
        },
    },
    Case {
        method: "draw_menu_bar",
        needle: Some("c0menu"),
        paint: |b, area| {
            let lh = b.line_height();
            let bar = MenuBar {
                id: id("menu-bar"),
                items: vec![MenuBarItem {
                    id: id("menu-bar-item"),
                    label: "c0menu".to_string(),
                    disabled: false,
                    submenu: None,
                }],
                open_item: None,
                focused_item: None,
            };
            let _ = b.draw_menu_bar(Rect::new(0.0, 0.0, area.width, lh), &bar);
        },
    },
    // ── More chrome-only primitives ────────────────────────────────────
    Case {
        method: "draw_split",
        needle: None,
        paint: |b, area| {
            let split = Split {
                id: id("split"),
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first_min: 0.0,
                second_min: 0.0,
            };
            let _ = b.draw_split(area, &split);
        },
    },
    Case {
        method: "draw_split_tree",
        needle: None,
        paint: |b, area| {
            let tree = SplitTree::split(
                SplitDirection::Horizontal,
                0.5,
                SplitTree::leaf(id("split-tree-a")),
                SplitTree::leaf(id("split-tree-b")),
            );
            let _ = b.draw_split_tree(area, &tree);
        },
    },
    Case {
        method: "draw_panel",
        needle: Some("c0panl"),
        paint: |b, area| {
            let panel = Panel {
                id: id("panel"),
                title: Some(StyledText::plain("c0panl")),
                actions: vec![],
                accent: None,
                collapsed: false,
            };
            let _ = b.draw_panel(area, &panel);
        },
    },
    Case {
        method: "draw_toast_stack",
        needle: Some("c0tost"),
        paint: |b, area| {
            let stack = ToastStack {
                id: id("toast-stack"),
                corner: ToastCorner::BottomRight,
                toasts: vec![ToastItem {
                    id: id("toast"),
                    title: "c0tost".to_string(),
                    body: String::new(),
                    severity: ToastSeverity::Info,
                    action: None,
                    accent: None,
                }],
            };
            let _ = b.draw_toast_stack(area, &stack);
        },
    },
    Case {
        method: "draw_command_center",
        needle: Some("c0ccnt"),
        paint: |b, area| {
            let cc = CommandCenter {
                id: id("command-center"),
                back_enabled: true,
                forward_enabled: true,
                search_label: "c0ccnt".to_string(),
            };
            let _ = b.draw_command_center(area, &cc);
        },
    },
    Case {
        method: "draw_toolbar",
        needle: Some("c0tbar"),
        paint: |b, area| {
            let lh = b.line_height();
            let bar = Toolbar {
                id: id("toolbar"),
                buttons: vec![ToolbarButton::Action {
                    id: id("toolbar-button"),
                    label: "c0tbar".to_string(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                }],
                bg: None,
                focused_index: None,
            };
            let _ = b.draw_toolbar(Rect::new(0.0, 0.0, area.width, lh), &bar, None, None);
        },
    },
    Case {
        method: "draw_sidebar_panel",
        needle: Some("c0sbpn"),
        paint: |b, area| {
            let toolbar = Toolbar {
                id: id("sidebar-toolbar"),
                buttons: vec![ToolbarButton::Action {
                    id: id("sidebar-toolbar-button"),
                    label: "c0sbpn".to_string(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                }],
                bg: None,
                focused_index: None,
            };
            let panel = SidebarPanel {
                id: id("sidebar-panel"),
                toolbar: Some(toolbar),
                toolbar_height: None,
            };
            let _ = b.draw_sidebar_panel(area, &panel, None, None);
        },
    },
    Case {
        method: "draw_chart",
        needle: Some("c0chrt"),
        paint: |b, area| {
            let chart = Chart {
                id: id("chart"),
                kind: ChartKind::Line,
                series: vec![Series {
                    label: "c0chrt".to_string(),
                    data: vec![1.0, 2.0, 3.0, 2.0, 4.0],
                    color: None,
                    fill: false,
                }],
                x_label: None,
                y_label: None,
                y_range: None,
                x_range: None,
                show_legend: true,
                y_ticks: None,
                x_ticks: None,
                show_grid: false,
            };
            let _ = b.draw_chart(area, &chart, None, None);
        },
    },
    Case {
        method: "draw_board",
        needle: Some("c0board"),
        paint: |b, area| {
            let model = BoardModel {
                id: id("board"),
                columns: vec![BoardColumn {
                    id: id("board-column"),
                    title: "c0board".to_string(),
                    cards: vec![BoardCard {
                        id: id("card-1"),
                        title: "card".to_string(),
                        labels: vec![],
                        badges: vec![CardBadge {
                            label: "P".to_string(),
                            status: BadgeStatus::Passed,
                        }],
                        hint: None,
                    }],
                    scroll_offset: 0,
                }],
                selected_card_id: None,
                col_scroll_offset: 0,
            };
            let _ = b.draw_board(area, &model);
        },
    },
];

/// An `AppLogic` whose whole render pass is one primitive's canned
/// descriptor.
struct Fixture {
    paint: fn(&mut dyn Backend, Rect),
}

impl AppLogic for Fixture {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        (self.paint)(backend, Rect::new(0.0, 0.0, vp.width, vp.height));
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// One case's outcome on one backend. Positional: `run()` returns exactly
/// one `CaseOutcome` per [`CASES`] entry, in the same order, so callers
/// zip the two rather than this type repeating `Case::method`.
pub struct CaseOutcome {
    /// `false` means the primitive panicked mid-paint — a worse failure
    /// than painting nothing, and the one C0's boot half exists to catch.
    pub survived: bool,
    /// `true` when the descriptor had no needle, or the needle it handed
    /// the backend came back in `text_runs()`.
    pub text_ok: bool,
    /// `true` when the frame reports a non-empty `text_runs()` or a
    /// non-empty `zones()` — contract §5b's observability floor.
    pub observable: bool,
    /// A compact dump of what the frame reported, for failure text.
    pub reported: String,
}

/// Run every [`CASES`] entry against one backend, built via `F`.
///
/// Generic over [`DriverFactory`] rather than duplicated per backend, so
/// a shared body can't accidentally assert something on one backend it
/// forgot to check on another.
pub fn run<F: DriverFactory>() -> Vec<CaseOutcome> {
    CASES
        .iter()
        .map(|case| {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let driver: Box<dyn DynDriver> = F::make(Fixture { paint: case.paint }, VIEWPORT);
                let inv = driver.inventory();
                let text_ok = match case.needle {
                    Some(needle) => driver.screen_has(needle),
                    None => true,
                };
                let observable = !inv.text_runs().is_empty() || !inv.zones().is_empty();
                let runs: Vec<String> = inv
                    .text_runs()
                    .iter()
                    .map(|r| r.text.clone())
                    .filter(|t| !t.trim().is_empty())
                    .take(10)
                    .collect();
                let zones: Vec<String> = inv
                    .zones()
                    .iter()
                    .map(|z| z.id.as_str().to_string())
                    .collect();
                (
                    text_ok,
                    observable,
                    format!("text_runs: {runs:?}; zones: {zones:?}"),
                )
            }));
            match outcome {
                Ok((text_ok, observable, reported)) => CaseOutcome {
                    survived: true,
                    text_ok,
                    observable,
                    reported,
                },
                Err(_) => CaseOutcome {
                    survived: false,
                    text_ok: false,
                    observable: false,
                    reported: "panicked mid-paint".to_string(),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::CASES;

    /// quadraui#492 review, blocking finding 1: `CASES` must name *every*
    /// `draw_*` method on `Backend`, not a hand-picked sample — a 15-of-38
    /// table let a backend ship a completely broken `draw_editor` and
    /// still get a fully green C0 row. Rather than trust that claim to
    /// stay true by convention, parse `backend.rs`'s own source for every
    /// `fn draw_*` trait item (skipping the doc-comment example inside the
    /// "Implementations are thin wrappers" blurb, which is indented as
    /// prose rather than a real trait item — `fn` there does not start
    /// the trimmed line) and assert the two lists agree exactly. A new
    /// primitive added to the trait without a matching `CASES` entry
    /// fails here, at the source of the drift, instead of shipping an
    /// untested `draw_*` method with a silently-passing C0 row.
    #[test]
    fn cases_cover_every_draw_method_on_the_trait() {
        const BACKEND_SRC: &str = include_str!("../../src/backend.rs");
        let trait_methods: Vec<&str> = BACKEND_SRC
            .lines()
            .filter(|line| line.trim_start().starts_with("fn draw_"))
            .map(|line| {
                line.trim_start()
                    .trim_start_matches("fn ")
                    .split(['(', '<'])
                    .next()
                    .unwrap()
                    .trim()
            })
            .collect();
        assert!(
            trait_methods.len() >= 30,
            "sanity check failed: expected dozens of `fn draw_*` trait methods in backend.rs, \
             found only {} — the line-based parser above likely broke against a reformatted \
             file rather than the trait actually shrinking: {trait_methods:?}",
            trait_methods.len()
        );

        let case_methods: std::collections::BTreeSet<&str> =
            CASES.iter().map(|c| c.method).collect();

        let missing: Vec<&str> = trait_methods
            .iter()
            .copied()
            .filter(|m| !case_methods.contains(m))
            .collect();
        assert!(
            missing.is_empty(),
            "{} `Backend::draw_*` method(s) have no `CASES` entry, so C0 says nothing about \
             them (quadraui#492): {missing:?}",
            missing.len()
        );

        let stale: Vec<&str> = case_methods
            .iter()
            .copied()
            .filter(|m| !trait_methods.contains(m))
            .collect();
        assert!(
            stale.is_empty(),
            "CASES names method(s) not found on the `Backend` trait today — likely a typo, or \
             a renamed/removed method CASES was not updated for: {stale:?}"
        );
    }
}
