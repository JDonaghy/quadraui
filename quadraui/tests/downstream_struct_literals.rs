//! Compile-time guard for the *Downstream consumers* policy in
//! `CLAUDE.md` / `docs/PRIMITIVE_RULES.md` rule 8.
//!
//! ## Why this file exists
//!
//! Several of quadraui's public primitives are all-`pub`-field "paint-time
//! snapshot" structs, and external consumers build them with **exhaustive
//! struct literals** — no `..base`, no `..Default::default()`. The live
//! example that motivated this file is `vimcode`'s
//! `src/render.rs::sc_commit_message_to_text_input()`:
//!
//! ```text
//! $ grep -rn 'TextInput {' ~/src/vimcode/src ~/src/coord-tui/src
//! /home/john/src/vimcode/src/render.rs:13998:    TextInput {
//! ```
//!
//! ```ignore
//! TextInput {
//!     id: WidgetId::new("sc:commit_input"),
//!     lines,
//!     cursor_line,
//!     cursor_col,
//!     placeholder: ...,
//!     scroll_offset: 0,
//!     scroll_col: 0,
//!     has_focus: sc.commit_input_active,
//! }
//! ```
//!
//! Rust gives no way to grow such a struct without breaking that call site:
//!
//! * adding a **private** field makes every external `TextInput { .. }`
//!   literal fail with `E0451`, and `..Default::default()` does *not*
//!   rescue it;
//! * adding a **public** field makes the same literal fail with `E0063`
//!   (`missing field`).
//!
//! Issue #833 hit both variants in succession, each time discovered only
//! at review. An integration test is the right home for the guard because
//! `tests/` compiles as a **separate crate** — exactly the position a
//! downstream consumer is in — so it catches the private-field (`E0451`)
//! case too, which an in-crate `#[cfg(test)] mod tests` cannot see.
//!
//! ## How to react when this file stops compiling
//!
//! It is not a test to "fix" by adding the new field here. A compile error
//! here means the change is **breaking for real consumers**. Follow rule 8:
//! prefer a non-breaking shape (put the new state on a wrapper type, as
//! `TextEditor` does for #833's selection anchor and undo history); if it
//! genuinely must break, land the consumer migration alongside it and put
//! the `grep` output in the PR's `## Downstream impact` section.

use quadraui::{TextInput, WidgetId};

/// `TextInput`'s exhaustive struct literal, transcribed from vimcode's
/// `sc_commit_message_to_text_input()` — field-for-field, and pointedly
/// with no `..base`. If this stops compiling, so does that consumer.
#[test]
fn text_input_exhaustive_struct_literal_still_compiles() {
    let ti = TextInput {
        id: WidgetId::new("sc:commit_input"),
        lines: vec!["subject".to_string(), String::new(), "body".to_string()],
        cursor_line: 2,
        cursor_col: 4,
        placeholder: Some("Message (press c)".to_string()),
        scroll_offset: 0,
        scroll_col: 0,
        has_focus: true,
    };

    assert_eq!(ti.lines.len(), 3);
    assert_eq!((ti.cursor_line, ti.cursor_col), (2, 4));
    assert!(ti.has_focus);
}

/// The editing state #833 added is reachable **without** touching
/// `TextInput`'s field list: an external crate wraps the same
/// exhaustively-constructed value in a `TextEditor` and edits it.
///
/// This is the other half of the guard above — it proves the wrapper is a
/// real substitute for the fields that were *not* added, rather than the
/// literal above being kept alive by amputating the feature.
#[test]
fn editing_is_available_without_new_text_input_fields() {
    use quadraui::{EditOp, TextEditor};

    let mut ed = TextEditor::new(TextInput {
        id: WidgetId::new("sc:commit_input"),
        lines: vec!["hello".to_string()],
        cursor_line: 0,
        cursor_col: 0,
        placeholder: None,
        scroll_offset: 0,
        scroll_col: 0,
        has_focus: true,
    });

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

/// `Toolbar`'s exhaustive struct literal, transcribed from `coord-tui`'s
/// `src/app/sidebar.rs::sidebar_panel()` — again pointedly with no
/// `..base`. Four such literals live in `coord-tui` and two in `vimcode`:
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
/// Issue #913 first tried to grow `Toolbar` by an `icon_overrides` field
/// and then, when that broke those literals with `E0063`, to mark the
/// struct `#[non_exhaustive]` — which breaks them with `E0639` instead
/// (`#[non_exhaustive]` rejects bare struct-literal syntax outright,
/// `..Default::default()` included). CI's *downstream consumers* job
/// caught the second attempt; this test is what catches the next one
/// here, before it costs a merge-gate round trip.
#[test]
fn toolbar_exhaustive_struct_literal_still_compiles() {
    use quadraui::{Toolbar, ToolbarButton};

    let bar = Toolbar {
        id: WidgetId::new("sidebar-action-bar"),
        buttons: vec![ToolbarButton::Action {
            id: WidgetId::new("sidebar:refresh"),
            label: "Refresh".to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }],
        bg: None,
        focused_index: None,
    };

    assert_eq!(bar.buttons.len(), 1);
    assert_eq!(bar.focused_index, None);
}

/// #913's Nerd-Font glyph + ASCII fallback pairs are reachable **without**
/// touching `Toolbar`'s field list: an external crate keeps building the
/// same exhaustive literal above and composes a `ToolbarIcons` table
/// beside it.
///
/// This is the other half of the guard — it proves the side table is a
/// real substitute for the field that was *not* added, rather than the
/// literal above being kept alive by dropping the feature.
#[test]
fn nerd_font_fallbacks_are_available_without_new_toolbar_fields() {
    use quadraui::{Icon, Toolbar, ToolbarButton, ToolbarIcons};

    let bar = Toolbar {
        id: WidgetId::new("sidebar-action-bar"),
        buttons: vec![ToolbarButton::Action {
            id: WidgetId::new("sidebar:refresh"),
            label: "Refresh".to_string(),
            icon: Some("~".to_string()),
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }],
        bg: None,
        focused_index: None,
    };
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
