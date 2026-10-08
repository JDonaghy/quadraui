//! `# Examples` doctest coverage ratchet for `quadraui/src/primitives/`
//! (issue #1352, part of the consumability epic #1340).
//!
//! docs.rs is where a Rust developer looks first, and before #1352 most
//! primitives shipped prose with no compiling example. Every primitive
//! module under `src/primitives/` declares exactly one "the primitive"
//! declarative struct/enum — [`PRIMITIVES`] below mirrors
//! `src/primitives/mod.rs`'s module list, pairing each module with that
//! type's name (`layout_metrics` is the one module with no such type —
//! it is shared pixel-layout *math*, not a widget; see its own module
//! doc — so it is absent from the table rather than padded with a
//! placeholder).
//!
//! This check reads each primitive's source file, finds that type's own
//! doc comment (the contiguous `///` block directly above `pub struct
//! Name` / `pub enum Name`, skipping over any `#[attr]` lines in
//! between), and asserts the block contains a `# Examples` heading.
//! Whether the example actually *compiles* is enforced separately by
//! `cargo test --doc` — this check only verifies the heading exists, so
//! it's cheap enough to run on every `cargo run -p quadraui-repo-lint`
//! without building the crate's doctest harness.
//!
//! ## The ratchet
//!
//! [`NOT_YET_COVERED`] is the explicit allowlist the issue asked for:
//! primitives that don't have an `# Examples` section yet. It is empty
//! today (#1352 landed all 41 in one pass), but the shape stays so a
//! future primitive addition can list itself here instead of failing
//! CI outright while its doctest is being written, same pattern as
//! `docs/decisions` exemptions elsewhere in this repo.
//!
//! Two assertions keep the list honest in both directions:
//! - [`every_primitive_struct_has_an_examples_section`] fails if a
//!   primitive not on the allowlist lacks the heading — the forward
//!   ratchet ("don't regress").
//! - [`allowlist_has_no_stale_entries`] fails if an allowlisted
//!   primitive *does* have the heading — it should have been removed
//!   from the list in the same PR that added the doctest, so the list
//!   shrinks to empty over time instead of silently accumulating
//!   already-fixed entries that no one remembers to delete.

use std::fs;

use crate::common::quadraui_dir;

/// `(module file stem, primitive type name)` for every primitive module
/// in `src/primitives/mod.rs`. Order matches that file. `layout_metrics`
/// is deliberately absent — see this module's doc.
const PRIMITIVES: &[(&str, &str)] = &[
    ("activity_bar", "ActivityBar"),
    ("board", "BoardModel"),
    ("canvas", "Canvas"),
    ("chart", "Chart"),
    ("command_center", "CommandCenter"),
    ("command_line", "CommandLine"),
    ("completions", "Completions"),
    ("context_menu", "ContextMenu"),
    ("data_table", "DataTable"),
    ("dialog", "Dialog"),
    ("diff_view", "DiffView"),
    ("drop_zone", "DropZone"),
    ("editor", "Editor"),
    ("find_replace", "FindReplacePanel"),
    ("float", "Float"),
    ("form", "Form"),
    ("image", "Image"),
    ("list", "ListView"),
    ("menu_bar", "MenuBar"),
    ("message_list", "MessageList"),
    ("minimap", "Minimap"),
    ("multi_section_view", "MultiSectionView"),
    ("palette", "Palette"),
    ("panel", "Panel"),
    ("pipeline_view", "PipelineView"),
    ("progress", "ProgressBar"),
    ("rich_text_popup", "RichTextPopup"),
    ("scrollbar", "Scrollbar"),
    ("sidebar_panel", "SidebarPanel"),
    ("spinner", "Spinner"),
    ("split", "Split"),
    ("split_tree", "SplitTree"),
    ("status_bar", "StatusBar"),
    ("tab_bar", "TabBar"),
    ("terminal", "Terminal"),
    ("text_display", "TextDisplay"),
    ("text_input", "TextInput"),
    ("toast", "Toast"),
    ("toolbar", "Toolbar"),
    ("tooltip", "Tooltip"),
    ("tree", "TreeView"),
];

/// Primitives that don't yet have an `# Examples` section. See this
/// module's doc for the ratchet contract. Empty today — #1352 covered
/// every entry in [`PRIMITIVES`] in one pass.
const NOT_YET_COVERED: &[&str] = &[];

/// Walk back from the `pub struct Name` / `pub enum Name` definition
/// line to the start of its doc comment, skipping `#[attr]` lines in
/// between (e.g. `#[non_exhaustive]` sitting between the doc block and
/// the item, as on `TextInput` / `Editor` / `Toolbar`). Returns the
/// collected `///` lines (without the leading `///`), outermost-first.
fn doc_lines_for_item(source: &str, type_name: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = source.lines().collect();
    let struct_needle = format!("pub struct {type_name}");
    let enum_needle = format!("pub enum {type_name}");

    let item_idx = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        (trimmed.starts_with(&struct_needle)
            && trimmed[struct_needle.len()..]
                .chars()
                .next()
                .is_none_or(|c| c == ' ' || c == '{' || c == '<' || c == '('))
            || (trimmed.starts_with(&enum_needle)
                && trimmed[enum_needle.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| c == ' ' || c == '{' || c == '<'))
    })?;

    let mut doc: Vec<String> = Vec::new();
    let mut i = item_idx;
    while i > 0 {
        i -= 1;
        let trimmed = lines[i].trim_start();
        if let Some(rest) = trimmed.strip_prefix("///") {
            doc.push(rest.trim_start().to_string());
        } else if trimmed.starts_with('#') {
            // Attribute between the doc block and the item — keep
            // walking up through it.
            continue;
        } else {
            break;
        }
    }
    doc.reverse();
    Some(doc)
}

/// Does `type_name`'s own doc comment in `file` contain a `# Examples`
/// heading?
///
/// Returns `None` if the type definition itself couldn't be found at
/// all — treated as a hard failure by the caller, distinct from "found
/// it, no heading", since a missing definition means [`PRIMITIVES`] has
/// drifted from the actual source rather than the primitive lacking
/// docs.
fn has_examples_heading(file: &str, type_name: &str) -> Option<bool> {
    let doc = doc_lines_for_item(file, type_name)?;
    Some(doc.iter().any(|line| line.trim() == "# Examples"))
}

/// The forward ratchet: every primitive not on [`NOT_YET_COVERED`] must
/// have an `# Examples` section on its own doc comment.
pub fn every_primitive_struct_has_an_examples_section() {
    let dir = quadraui_dir().join("src").join("primitives");
    let mut missing: Vec<String> = Vec::new();
    let mut drifted: Vec<String> = Vec::new();

    for (module, type_name) in PRIMITIVES {
        if NOT_YET_COVERED.contains(module) {
            continue;
        }
        let path = dir.join(format!("{module}.rs"));
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        match has_examples_heading(&source, type_name) {
            None => drifted.push(format!(
                "{module}.rs: no `pub struct {type_name}` / `pub enum {type_name}` found — \
                 PRIMITIVES in tools/lint/src/primitive_doc_examples.rs has drifted from the \
                 actual type name"
            )),
            Some(false) => missing.push(format!("{module}.rs ({type_name})")),
            Some(true) => {}
        }
    }

    assert!(
        drifted.is_empty(),
        "primitive_doc_examples.rs's PRIMITIVES table is stale:\n  {}",
        drifted.join("\n  ")
    );
    assert!(
        missing.is_empty(),
        "these primitives have no `# Examples` rustdoc section (issue #1352):\n  {}\n\n\
         Add one to the type's own doc comment: construct it with realistic \
         fields, call its `*_layout` method if it has one, and assert \
         something about the result — see any primitive in this list's \
         siblings for the pattern. If you can't finish this pass, add the \
         module name to NOT_YET_COVERED in \
         tools/lint/src/primitive_doc_examples.rs instead of leaving this \
         check red.",
        missing.join("\n  ")
    );
}

/// The reverse ratchet: an allowlisted primitive that already has the
/// heading should have been removed from [`NOT_YET_COVERED`] in the same
/// PR that added it, so the list can only shrink.
pub fn allowlist_has_no_stale_entries() {
    let dir = quadraui_dir().join("src").join("primitives");
    let by_module: std::collections::HashMap<&str, &str> = PRIMITIVES.iter().copied().collect();
    let mut stale: Vec<String> = Vec::new();

    for module in NOT_YET_COVERED {
        let Some(type_name) = by_module.get(module) else {
            panic!(
                "NOT_YET_COVERED entry {module:?} doesn't match any module in PRIMITIVES — \
                 fix the typo or remove the stale entry"
            );
        };
        let path = dir.join(format!("{module}.rs"));
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        if has_examples_heading(&source, type_name) == Some(true) {
            stale.push(module.to_string());
        }
    }

    assert!(
        stale.is_empty(),
        "these NOT_YET_COVERED entries in tools/lint/src/primitive_doc_examples.rs already \
         have an `# Examples` section — remove them from the allowlist so it keeps shrinking:\n  {}",
        stale.join("\n  ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_examples_heading_past_an_intervening_attribute() {
        let src = "\
/// Some intro prose.
///
/// # Examples
///
/// ```
/// let x = 1;
/// ```
#[non_exhaustive]
#[derive(Debug)]
pub struct Widget {
    pub id: u32,
}
";
        assert_eq!(has_examples_heading(src, "Widget"), Some(true));
    }

    #[test]
    fn reports_missing_heading_distinctly_from_missing_type() {
        let src = "\
/// Some intro prose, no examples section.
pub struct Widget {
    pub id: u32,
}
";
        assert_eq!(has_examples_heading(src, "Widget"), Some(false));
        assert_eq!(has_examples_heading(src, "NoSuchType"), None);
    }

    #[test]
    fn stops_the_upward_walk_at_a_blank_line() {
        // A blank line separates an unrelated preceding doc block (e.g.
        // a free function's docs) from this struct's own — only the
        // contiguous block directly above counts.
        let src = "\
/// Docs for an unrelated item above, which happens to mention # Examples.
///
/// # Examples
fn unrelated() {}

/// This struct's own docs, no heading.
pub struct Widget {
    pub id: u32,
}
";
        assert_eq!(has_examples_heading(src, "Widget"), Some(false));
    }

    #[test]
    fn does_not_match_a_type_name_that_is_a_prefix_of_another() {
        let src = "\
/// Docs for the longer name.
///
/// # Examples
pub struct WidgetExtra {
    pub id: u32,
}

/// Docs for the short name, no heading here.
pub struct Widget {
    pub id: u32,
}
";
        assert_eq!(has_examples_heading(src, "Widget"), Some(false));
    }
}
