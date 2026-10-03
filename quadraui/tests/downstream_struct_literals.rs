//! Compile-time guard for the *Downstream consumers* policy in
//! `CLAUDE.md` / `docs/PRIMITIVE_RULES.md` rule 8.
//!
//! ## Why this file exists
//!
//! Three of quadraui's public primitives — `TextInput`, `Toolbar`,
//! `Editor` — used to be all-`pub`-field "paint-time snapshot" structs,
//! and external consumers built them with **exhaustive struct
//! literals** — no `..base`, no `..Default::default()`. The live
//! example that motivated this file was `vimcode`'s
//! `src/render.rs::sc_commit_message_to_text_input()`:
//!
//! ```text
//! $ grep -rn 'TextInput {' ~/src/vimcode/src ~/src/coord-tui/src
//! /home/john/src/vimcode/src/render.rs:13998:    TextInput {
//! ```
//!
//! Rust gives no way to grow such a struct without breaking that call
//! site: adding a **private** field makes every external
//! `TextInput { .. }` literal fail with `E0451` (and `..Default::default()`
//! does *not* rescue it), and adding a **public** field makes the same
//! literal fail with `E0063` (`missing field`). Issue #833 hit both
//! variants in succession, each time discovered only at review.
//!
//! ## #1108 → #1251: builders, then `#[non_exhaustive]` for real
//!
//! #1108 gave all three types a `new(..)`/`with_*`/`Default` builder
//! trio covering every field, so both consumers could migrate off
//! exhaustive literals ahead of the break, and added an off-by-default
//! `strict-descriptors` feature a consumer could opt into early to
//! *prove* its own migration was complete. #1251 (the v0.1.0 breaking
//! batch, phase 2) is the follow-up that actually lands the break, once
//! `coord-tui`#119 and `vimcode`#1652 had merged their migrations: all
//! three structs are `#[non_exhaustive]` unconditionally now, and the
//! `strict-descriptors` feature is gone — there's nothing left to opt
//! into.
//!
//! ## What this file guards now
//!
//! An external crate (this one, compiling as a separate crate — exactly
//! the position a downstream consumer is in) can no longer build
//! `TextInput`/`Toolbar`/`Editor` with a struct literal at all, so the
//! old "literal still compiles" guard is impossible to keep. Inverted
//! per issue #1251: these tests instead assert that the `new(..)`/
//! `with_*`/`Default` builders cover every field each consumer's own
//! struct literal used to set — i.e. the migration path #1108 offered
//! was never missing a field. If a future field addition has no
//! matching `with_*`, that is caught here before it reaches a consumer
//! that has no struct-literal escape hatch left to fall back on.
//!
//! `AppShellLayout` (below) is a different primitive, untouched by
//! #1108/#1251 — it keeps its exhaustive-literal guard unchanged.

use quadraui::WidgetId;

/// `TextInput`'s builder chain, covering every field vimcode's
/// `sc_commit_message_to_text_input()` used to set via an exhaustive
/// struct literal (transcribed field-for-field from that call site)
/// before #1251 made `#[non_exhaustive]` unconditional.
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

/// The editing state #833 added is reachable **without** touching
/// `TextInput`'s field list: an external crate wraps a builder-
/// constructed value in a `TextEditor` and edits it.
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

/// `Toolbar`'s builder chain, covering every field `coord-tui`'s
/// `src/app/sidebar.rs::sidebar_panel()` used to set via an exhaustive
/// struct literal. Four such literals lived in `coord-tui` and two in
/// `vimcode`:
///
/// ```text
/// $ grep -rn 'Toolbar {' ~/src/coord-tui/src ~/src/vimcode/src
/// /home/john/src/coord-tui/src/app/sidebar.rs:66:            toolbar: Some(Toolbar {
/// /home/john/src/coord-tui/src/app/sidebar.rs:368:        Some(Toolbar {
/// /home/john/src/coord-tui/src/app/pipeline.rs:7434:        Some(Toolbar {
/// /home/john/src/coord-tui/src/app/dialogs.rs:6311:            let toolbar = Toolbar {
/// /home/john/src/vimcode/src/render.rs:9089:    Toolbar {
/// /home/john/src/vimcode/src/render.rs:13824:    Toolbar {
/// ```
///
/// Issue #913 first tried to grow `Toolbar` by an `icon_overrides`
/// field and then, when that broke those literals with `E0063`, to mark
/// the struct `#[non_exhaustive]` — which would have broken them with
/// `E0639` instead, before either consumer had a builder to fall back
/// on. Both migrated to the builder below ahead of #1251 flipping the
/// attribute on for real.
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

/// #913's Nerd-Font glyph + ASCII fallback pairs are reachable **without**
/// touching `Toolbar`'s field list: an external crate keeps building the
/// same builder-constructed value above and composes a `ToolbarIcons`
/// table beside it.
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

/// `Editor`'s builder chain, covering every field vimcode's
/// `render.rs::to_q_editor()` used to set via an exhaustive struct
/// literal:
///
/// ```text
/// $ grep -n 'Editor {$' ~/src/vimcode/src/render.rs
/// 19236:    quadraui::Editor {
/// ```
///
/// (`~/src/coord-tui/src` has zero hits — it doesn't construct `Editor`
/// at all.) #968 added scrollbar-suppression as
/// `quadraui::EditorPaintOptions` + `gtk::draw_editor_with_options`
/// *instead of* a new field directly on `Editor`, specifically to keep
/// this call site's migration a pure builder-chain swap rather than a
/// field-list change — see that primitive's module doc and
/// `docs/PRIMITIVE_RULES.md` rule 8. Fields left at [`Editor::new`]'s
/// own default (`lines`, `extra_cursors`, `selection`,
/// `extra_selections`, `yank_highlight`, `scroll_top`, `scroll_left`,
/// `total_lines`, `max_col`, `show_active_bg`, `has_git_diff`,
/// `has_breakpoints`, `diagnostic_gutter`, `code_action_lines`,
/// `bracket_match_positions`, `active_indent_col`, `tabstop`) need no
/// `with_*` call — only fields that differ from the constructor's
/// default do, same as the real migration.
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

/// `AppShellLayout`'s exhaustive struct literal, transcribed from vimcode's
/// `render.rs::bare_shell_layout()` — field-for-field, and pointedly with
/// no `..base` (`AppShellLayout` has no `Default` impl). Unlike
/// `TextInput`/`Toolbar`/`Editor` above, `AppShellLayout` was not part of
/// #1108/#1251's builder migration — it stays exhaustive-literal built,
/// so this guard is unchanged:
///
/// ```text
/// $ grep -rn "AppShellLayout {" ~/src/vimcode/src ~/src/coord-tui/src
/// /home/john/src/vimcode/src/render.rs:22108:    quadraui::AppShellLayout {
/// /home/john/src/vimcode/src/tui_main/shell_app.rs:1844:    quadraui::AppShellLayout {
/// ```
///
/// (The second hit, `shell_app.rs:1844`, uses `..layout.clone()` and is
/// unaffected by field-list growth either way — only `bare_shell_layout()`
/// is exhaustive.)
///
/// Issue #997 first tried to grow `AppShellLayout` by a
/// `bottom_band_bounds: Vec<(WidgetId, Rect)>` field for its N-independent-
/// bottom-bands feature and, caught at review, found this exact literal
/// would break with `E0063`. The fix moved bottom-band bounds off
/// `AppShellLayout` entirely — read via [`quadraui::compose::app_shell::AppShell::bottom_band_bounds`]
/// after `layout`/`render`, or painted via the new
/// `ShellApp::render_bottom_band` hook (a trait method with a default
/// no-op implementation, so existing `ShellApp` implementors are
/// unaffected) — instead of widening this struct's field list. This test
/// is what catches the next attempt to do it the breaking way, before it
/// costs a merge-gate round trip.
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

// ── `Backend::draw_toolbar*` call shapes (issue #260) ──────────────────

/// The *call shapes* a known consumer uses to paint a `Toolbar`, as an
/// external crate — the other half of the builder-coverage guards
/// above. A struct can break a consumer by growing a field; a trait
/// method or free function breaks one just as hard by growing a
/// **parameter** (`E0061`), and nothing in `tests/` caught that until
/// #260's first attempt did exactly that and turned the *downstream
/// consumers (compile truth)* CI job red:
///
/// ```text
/// $ grep -rn 'draw_toolbar' ~/src/coord-tui/src ~/src/vimcode/src
/// /home/john/src/coord-tui/src/app/render.rs:396:   backend.draw_toolbar_interactive(bar_rect, &toolbar, &InteractionState::from_parts(..))
/// /home/john/src/coord-tui/src/app/dialogs.rs:6435: backend.draw_toolbar_interactive(bar_rect, &toolbar, &InteractionState::new())
/// ```
///
/// (`vimcode`'s positional `draw_toolbar(rect, &bar, hovered, pressed)`
/// call — the third call shape this module used to cover — migrated to
/// `draw_toolbar_interactive` alongside its #1251 builder migration;
/// `Backend::draw_toolbar` itself was removed in the same issue, once
/// that was the only remaining caller.)
///
/// `ToolbarVAlign` reaches the rasteriser through
/// [`quadraui::ToolbarPaintOptions`] + `draw_toolbar_with_options`
/// instead — a *new* method beside the old ones, per `CLAUDE.md`'s
/// *Downstream consumers* rule 2 — so both call sites above keep
/// compiling untouched. This test fails to compile if a future change
/// grows either of them again.
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
        /// #260's new shape: the above plus `ToolbarPaintOptions`.
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

    /// #260's opt-in: the new method moves the painted row, and the
    /// shim above agrees with `ToolbarVAlign::Top` (the default).
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
