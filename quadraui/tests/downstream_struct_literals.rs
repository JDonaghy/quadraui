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

/// `Toolbar` (issue #913) took the *other* branch of rule 8's decision
/// tree: rather than avoid the break (as `TextInput` above does), it
/// accepted a one-time breaking change by marking itself
/// `#[non_exhaustive]` + `Default` specifically so *no future* field
/// addition breaks external construction again — the same shape this
/// crate's own `PRIMITIVE_RULES.md` rule 8 lists as preferred, applied
/// retroactively because `Toolbar` shipped without it.
///
/// The pre-#913 exhaustive literal this repeats vimcode's and coord-tui's
/// call sites almost certainly used (`Toolbar { id, buttons, bg,
/// focused_index }`, no `icon_overrides` — that field didn't exist yet)
/// **no longer compiles** in an external crate: `#[non_exhaustive]`
/// rejects bare struct-literal syntax outright, `..Default::default()`
/// included (verified while writing this fix — see the review thread on
/// issue #913). That is `## Downstream impact`: both consumers must move
/// their `Toolbar { .. }` construction to [`Toolbar::new`] (or
/// `Toolbar::default()` + field assignment) the next time they bump
/// their pinned quadraui rev. This test guards the migration path itself
/// — the replacement every such call site needs — the same way the
/// `TextInput` tests above guard theirs.
#[test]
fn toolbar_new_constructor_replaces_the_pre_913_bare_struct_literal() {
    use quadraui::{Toolbar, ToolbarButton};

    // What `Toolbar { id, buttons, bg, focused_index }` used to look
    // like, ported to the external-crate-safe constructor.
    let mut bar = Toolbar::new(
        WidgetId::new("sc:toolbar"),
        vec![ToolbarButton::Action {
            id: WidgetId::new("sc:commit"),
            label: "Commit".to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }],
    );
    bar.bg = None;
    bar.focused_index = Some(0);

    assert_eq!(bar.buttons.len(), 1);
    assert_eq!(bar.focused_index, Some(0));
    assert!(bar.icon_overrides.is_empty());
}
