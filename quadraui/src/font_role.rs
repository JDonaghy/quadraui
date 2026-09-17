//! Chrome-vs-editor font classification (issue #1003).
//!
//! "Which font does this primitive paint in?" is a property of the
//! *primitive*, not of the rasteriser — a tree is chrome on every
//! backend, a menu bar is chrome on every backend, an editor is not.
//! Before this module existed, each backend re-decided that fact by hand
//! at its own `draw_*` call sites, and the answer drifted: GTK's
//! `gtk::backend::GtkBackend` swapped in the chrome font
//! (`chrome_font_description(&self.ui_font)`) at 16 call sites; macOS's
//! `macos::backend::MacBackend` only did it at 2 (`draw_status_bar_interactive`
//! and, per the #1003 audit, not even `draw_activity_bar_with_style`
//! despite the issue crediting it — see that method's own doc), leaving
//! the rest — `draw_tree` included, the most visible — painting the host
//! chrome in the *editor's* monospace font.
//!
//! [`ChromePrimitive`] is the single place that answers the question
//! from now on. A backend's `draw_*`/`*_layout` method for one of these
//! primitives resolves its font by consulting this list rather than
//! re-deciding — see `gtk::backend::GtkBackend`'s `ui_font_desc` call
//! sites and `macos::backend::MacBackend`'s `chrome_font` field uses for
//! the two backends' call-site conventions. Everything **not** listed
//! here paints in the editor font (or has no font at all: dividers,
//! scrollbar tracks, images).
//!
//! TUI is deliberately absent from this picture — a terminal cell grid
//! has one font by definition, so `FontRole` has nothing to say there
//! (`quadraui::tui` has no `ui_font` concept at all, and correctly so).
//!
//! ## Why this module is not under `primitives/`
//!
//! It classifies primitives; it isn't one. There is no `FontRole`
//! widget struct, no `*Layout`, no `hit_test`, no `Backend::draw_*`
//! entry point — the same test `crate::A11yInfo` (#835) is held to. Its
//! module would otherwise be counted as a 41st widget primitive by
//! `readme_truth.rs`'s `lib_doc_states_the_real_primitive_count` /
//! `root_readme_states_the_real_primitive_count`, which derive the
//! crate's advertised primitive count from `primitives/mod.rs`'s
//! `pub mod` list — making both docs claim a primitive that ships no
//! widget. Keep new cross-cutting policy modules at the crate root.

/// Which font a primitive paints in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRole {
    /// The host chrome / UI font — [`crate::Backend::set_ui_font`].
    Chrome,
    /// The user's configured editor font — [`crate::Backend::set_editor_font`].
    Editor,
}

/// Every primitive family with a `Backend::draw_*` entry point that
/// paints in the **chrome** font, on every pixel backend, by definition.
///
/// A primitive with several `draw_*`/`*_layout` entry points (e.g. the
/// tab bar's icon-less/icon/chrome-frame/layout-returning variants, all
/// of which paint the same tab labels) still gets exactly one variant
/// here — the font-role question has one answer per primitive, not per
/// method, and most of those variants delegate to one root method
/// anyway (see `Backend::draw_tab_bar_with_chrome`'s default body).
///
/// `#[non_exhaustive]`: adding a primitive here is exactly the kind of
/// change downstream consumers never need to match on (see
/// `CLAUDE.md`'s public-API rules), and it keeps this enum growable
/// without a breaking change every time a backend gains a new
/// chrome-painting primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChromePrimitive {
    Tree,
    List,
    MenuBar,
    ContextMenu,
    Dialog,
    RichTextPopup,
    CommandCenter,
    MultiSectionView,
    SidebarPanel,
    StatusBar,
    ActivityBar,
    Toolbar,
    TabBar,
}

impl ChromePrimitive {
    /// Every classified chrome primitive.
    ///
    /// Intended to be walked by a cross-backend conformance test so a
    /// new entry added here gets checked against every registered pixel
    /// backend instead of just documented — but that test does not
    /// exist yet. Milestone #11 is the natural home for it (see issue
    /// #1003); until it lands, this array is only exercised by the
    /// unit tests in this module (duplicate-check, name round-trip),
    /// which do not paint anything or compare against a second font.
    /// Don't read this doc as "already enforced."
    pub const ALL: [ChromePrimitive; 13] = [
        ChromePrimitive::Tree,
        ChromePrimitive::List,
        ChromePrimitive::MenuBar,
        ChromePrimitive::ContextMenu,
        ChromePrimitive::Dialog,
        ChromePrimitive::RichTextPopup,
        ChromePrimitive::CommandCenter,
        ChromePrimitive::MultiSectionView,
        ChromePrimitive::SidebarPanel,
        ChromePrimitive::StatusBar,
        ChromePrimitive::ActivityBar,
        ChromePrimitive::Toolbar,
        ChromePrimitive::TabBar,
    ];

    /// Every [`ChromePrimitive`] is [`FontRole::Chrome`] by construction
    /// — membership in this enum *is* the classification (see the
    /// module doc). This accessor exists so call sites read intent
    /// (`primitive.font_role() == FontRole::Chrome`) instead of an
    /// unnamed fact, and so a future primitive whose role needs to be
    /// context-dependent has a real match arm to grow instead of a new
    /// backend reinventing the question from scratch.
    pub const fn font_role(self) -> FontRole {
        FontRole::Chrome
    }

    /// Short, stable name for diagnostics (conformance-test failure
    /// messages, debug output) — not for parsing.
    pub const fn name(self) -> &'static str {
        match self {
            ChromePrimitive::Tree => "tree",
            ChromePrimitive::List => "list",
            ChromePrimitive::MenuBar => "menu_bar",
            ChromePrimitive::ContextMenu => "context_menu",
            ChromePrimitive::Dialog => "dialog",
            ChromePrimitive::RichTextPopup => "rich_text_popup",
            ChromePrimitive::CommandCenter => "command_center",
            ChromePrimitive::MultiSectionView => "multi_section_view",
            ChromePrimitive::SidebarPanel => "sidebar_panel",
            ChromePrimitive::StatusBar => "status_bar",
            ChromePrimitive::ActivityBar => "activity_bar",
            ChromePrimitive::Toolbar => "toolbar",
            ChromePrimitive::TabBar => "tab_bar",
        }
    }
}

impl std::fmt::Display for ChromePrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chrome_primitive_is_chrome() {
        for p in ChromePrimitive::ALL {
            assert_eq!(p.font_role(), FontRole::Chrome, "{p}");
        }
    }

    #[test]
    fn all_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for p in ChromePrimitive::ALL {
            assert!(
                seen.insert(p),
                "duplicate entry in ChromePrimitive::ALL: {p}"
            );
        }
    }

    #[test]
    fn name_round_trips_through_display() {
        for p in ChromePrimitive::ALL {
            assert_eq!(p.to_string(), p.name());
        }
    }
}
