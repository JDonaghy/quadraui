//! Compile-time guard for the *Downstream consumers* policy:
//! these three `quadraui` public primitives — `TextInput`, `Toolbar`,
//! `Editor` — are `#[non_exhaustive]` and built via builders, not
//! exhaustive struct literals. This file asserts the builders cover
//! every field that external consumers' original literal-based code
//! used to set, so migrations off literals never omit a field.
//! ```text
//! $ grep -rn 'TextInput {' ~/src/vimcode/src ~/src/coord-tui/src
//! /home/john/src/vimcode/src/render.rs:13998:    TextInput {
//! ```

use quadraui::WidgetId;

/// `TextInput`'s builder chain covers every field that external
/// consumers' struct literals set before `#[non_exhaustive]` was applied.
#[test]
fn text_input_builder_covers_every_consumer_set_field() {
    use quadraui::TextInput;

    let ti = TextInput::new(WidgetId::new("sc:commit_input"))
        .with_lines(vec![
            "subject".to_string(),
            String::new(),
            "body".to_string(),
        ])
        .with_cursor_line(2)
        .with_cursor_col(4)
        .with_placeholder("Message (press c)")
        .with_scroll_offset(0)
        .with_scroll_col(0)
        .with_has_focus(true);

    assert_eq!(ti.lines.len(), 3);
    assert_eq!((ti.cursor_line, ti.cursor_col), (2, 4));
    assert_eq!(ti.placeholder.as_deref(), Some("Message (press c)"));
    assert!(ti.has_focus);
}

/// The editing state is reachable via `TextEditor` wrapping a
/// builder-constructed `TextInput` without changes to the input's field list.
#[test]
fn editing_is_available_without_new_text_input_fields() {
    use quadraui::{EditOp, TextEditor, TextInput};

    let mut ed = TextEditor::new(
        TextInput::new(WidgetId::new("sc:commit_input"))
            .with_lines(vec!["hello".to_string()])
            .with_has_focus(true),
    );

    // Select "he", type over it, then undo — all three capabilities the
    // issue asked for, none of them a new `TextInput` field.
    ed.apply(EditOp::MoveRight { extend: true });
    ed.apply(EditOp::MoveRight { extend: true });
    assert_eq!(ed.selected_text().as_deref(), Some("he"));

    ed.apply(EditOp::InsertChar('X'));
    assert_eq!(ed.lines, vec!["Xllo".to_string()]);

    assert!(ed.apply(EditOp::Undo));
    assert_eq!(ed.lines, vec!["hello".to_string()]);
    assert_eq!(ed.selection_range(), Some(((0, 0), (0, 2))));

    // `Deref` means the wrapper is still a `TextInput` everywhere a
    // rasteriser wants one.
    let painted: &TextInput = &ed;
    assert_eq!(painted.id, WidgetId::new("sc:commit_input"));
}

/// `Toolbar`'s builder chain covers every field that external
/// consumers' struct literals set before `#[non_exhaustive]` was applied.
/// ```text
/// $ grep -rn 'Toolbar {' ~/src/coord-tui/src ~/src/vimcode/src
/// /home/john/src/coord-tui/src/app/sidebar.rs:66:            toolbar: Some(Toolbar {
/// /home/john/src/coord-tui/src/app/sidebar.rs:368:        Some(Toolbar {
/// /home/john/src/coord-tui/src/app/pipeline.rs:7434:        Some(Toolbar {
/// /home/john/src/coord-tui/src/app/dialogs.rs:6311:            let toolbar = Toolbar {
/// /home/john/src/vimcode/src/render.rs:9089:    Toolbar {
/// /home/john/src/vimcode/src/render.rs:13824:    Toolbar {
/// ```
#[test]
fn toolbar_builder_covers_every_consumer_set_field() {
    use quadraui::{Toolbar, ToolbarButton};

    let bar = Toolbar::new(WidgetId::new("sidebar-action-bar")).with_buttons(vec![
        ToolbarButton::Action {
            id: WidgetId::new("sidebar:refresh"),
            label: "Refresh".to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        },
    ]);

    assert_eq!(bar.buttons.len(), 1);
    assert_eq!(bar.focused_index, None);
}

/// Nerd-Font glyph + ASCII fallback pairs are reachable via
/// `ToolbarIcons` alongside a builder-constructed `Toolbar`.
#[test]
fn nerd_font_fallbacks_are_available_without_new_toolbar_fields() {
    use quadraui::{Icon, Toolbar, ToolbarButton, ToolbarIcons};

    let bar = Toolbar::new(WidgetId::new("sidebar-action-bar")).with_buttons(vec![
        ToolbarButton::Action {
            id: WidgetId::new("sidebar:refresh"),
            label: "Refresh".to_string(),
            icon: Some("~".to_string()),
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        },
    ]);
    let icons =
        ToolbarIcons::new().with(WidgetId::new("sidebar:refresh"), Icon::new("\u{f021}", "R"));

    let icon_of = |bar: &Toolbar| match &bar.buttons[0] {
        ToolbarButton::Action { icon, .. } => icon.clone(),
        _ => None,
    };

    assert_eq!(
        icon_of(&icons.apply(&bar, true)).as_deref(),
        Some("\u{f021}")
    );
    assert_eq!(icon_of(&icons.apply(&bar, false)).as_deref(), Some("R"));
    // The resolved value is a plain `Toolbar`, so it goes straight into
    // every existing `Backend::draw_toolbar*` / `SidebarPanel` slot.
    let _: Toolbar = icons.apply(&bar, true);
    // Registering nothing leaves the bar exactly as built.
    assert_eq!(ToolbarIcons::new().apply(&bar, true), bar);
}

/// `Editor`'s builder chain covers every field that external
/// consumers' struct literals set before `#[non_exhaustive]` was applied.
/// Fields at their default (`lines`, `extra_cursors`, `selection`,
/// `extra_selections`, `yank_highlight`, `scroll_top`, `scroll_left`,
/// `total_lines`, `max_col`, `show_active_bg`, `has_git_diff`,
/// `has_breakpoints`, `diagnostic_gutter`, `code_action_lines`,
/// `bracket_match_positions`, `active_indent_col`, `tabstop`) need no
/// `with_*` call — only non-default fields do.
/// ```text
/// $ grep -n 'Editor {$' ~/src/vimcode/src/render.rs
/// 19236:    quadraui::Editor {
/// ```
#[test]
fn editor_builder_covers_every_consumer_set_field() {
    use quadraui::{Editor, EditorCursor, EditorCursorPos, EditorCursorShape, Rect};

    let ed = Editor::new(WidgetId::new("editor:0"), Rect::new(0.0, 0.0, 80.0, 24.0))
        .with_cursor(EditorCursor {
            pos: EditorCursorPos {
                view_line: 0,
                col: 0,
            },
            shape: EditorCursorShape::Bar,
        })
        .with_gutter_char_width(4)
        .with_is_active(true)
        .with_cursorline(true)
        .with_lightbulb_glyph('!');

    assert!(ed.is_active);
    assert_eq!(ed.tabstop, 4);
    assert_eq!(ed.gutter_char_width, 4);
    assert!(ed.cursorline);
    assert_eq!(ed.lightbulb_glyph, '!');
}

/// `AppShellLayout` remains an exhaustive struct, built with struct
/// literals rather than a builder. This test enforces that the
/// literal pattern — no `..base`, no `Default` impl — still compiles,
/// guarding against future field additions that would break it.
/// ```text
/// $ grep -rn "AppShellLayout {" ~/src/vimcode/src ~/src/coord-tui/src
/// /home/john/src/vimcode/src/render.rs:22108:    quadraui::AppShellLayout {
/// /home/john/src/vimcode/src/tui_main/shell_app.rs:1844:    quadraui::AppShellLayout {
/// ```
#[test]
fn app_shell_layout_exhaustive_struct_literal_still_compiles() {
    use quadraui::{AppShellLayout, Rect};

    let layout = AppShellLayout {
        window_bounds: Rect::new(0.0, 0.0, 1400.0, 900.0),
        title_bar_bounds: None,
        activity_bar_bounds: Rect::default(),
        sidebar_header_bounds: None,
        sidebar_content_bounds: None,
        divider_bounds: None,
        main_content_bounds: Rect::new(0.0, 0.0, 1400.0, 900.0),
        bottom_panel_bounds: None,
        command_line_bounds: None,
        status_bar_bounds: None,
    };

    assert_eq!(layout.main_content_bounds.width, 1400.0);
    assert!(layout.bottom_panel_bounds.is_none());
}

// ── `Backend::draw_toolbar*` call shapes ───────────────────────────────

/// The call shapes a known consumer uses to paint a `Toolbar`.
/// A function breaks a consumer by growing its parameters (`E0061`)
/// as readily as a struct breaks by growing fields. This test catches
/// any future parameter additions to `draw_toolbar_interactive`.
/// ```text
/// $ grep -rn 'draw_toolbar' ~/src/coord-tui/src ~/src/vimcode/src
/// /home/john/src/coord-tui/src/app/render.rs:396:   backend.draw_toolbar_interactive(bar_rect, &toolbar, &InteractionState::from_parts(..))
/// /home/john/src/coord-tui/src/app/dialogs.rs:6435: backend.draw_toolbar_interactive(bar_rect, &toolbar, &InteractionState::new())
/// ```
#[cfg(feature = "tui")]
mod toolbar_paint_call_shapes {
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{
        AppLogic, Backend, InteractionState, Reaction, Rect, Toolbar, ToolbarButton,
        ToolbarPaintOptions, ToolbarVAlign, UiEvent, WidgetId,
    };

    /// Paints one toolbar into a 3-row slot, through whichever of the
    /// two public entry points `call_shape` selects.
    struct ToolbarPainter {
        call_shape: CallShape,
    }

    #[derive(Clone, Copy)]
    enum CallShape {
        /// coord-tui's shape: `(rect, bar, &InteractionState)`.
        Interactive,
        /// The new method with `ToolbarPaintOptions`.
        WithOptions(ToolbarVAlign),
    }

    fn bar() -> Toolbar {
        Toolbar::new(WidgetId::new("downstream-bar")).with_buttons(vec![ToolbarButton::Action {
            id: WidgetId::new("downstream:go"),
            label: "Go".to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }])
    }

    impl AppLogic for ToolbarPainter {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            // A 3-row slot at the top of the screen: tall enough that
            // `Top` and `Bottom` land on different rows.
            let rect = Rect::new(0.0, 0.0, backend.viewport().width, 3.0);
            let bar = bar();
            let _ = match self.call_shape {
                CallShape::Interactive => {
                    backend.draw_toolbar_interactive(rect, &bar, &InteractionState::new())
                }
                CallShape::WithOptions(valign) => backend.draw_toolbar_with_options(
                    rect,
                    &bar,
                    &InteractionState::new(),
                    ToolbarPaintOptions { valign },
                ),
            };
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    fn painted_row(call_shape: CallShape) -> u16 {
        let mut driver = TuiDriver::new(ToolbarPainter { call_shape }, 24, 6);
        driver.render();
        let (_x, y) = driver
            .find("Go")
            .unwrap_or_else(|| panic!("toolbar button never painted:\n{}", driver.screen()));
        y as u16
    }

    /// coord-tui's three-argument call still compiles **and** paints —
    /// a forwarding shim that silently stopped painting would pass a
    /// compile-only guard.
    #[test]
    fn coord_tui_three_arg_interactive_call_still_paints() {
        assert_eq!(painted_row(CallShape::Interactive), 0);
    }

    /// The new method moves the painted row, defaulting to `ToolbarVAlign::Top`.
    #[test]
    fn with_options_moves_the_painted_row_and_defaults_to_top() {
        assert_eq!(painted_row(CallShape::WithOptions(ToolbarVAlign::Top)), 0);
        assert_eq!(
            painted_row(CallShape::WithOptions(ToolbarVAlign::Center)),
            1
        );
        assert_eq!(
            painted_row(CallShape::WithOptions(ToolbarVAlign::Bottom)),
            2
        );
    }
}
