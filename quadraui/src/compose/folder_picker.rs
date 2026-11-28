//! [`FolderPicker`] — the [`PathPicker`] kind that browses directories.
//!
//! The picker composes [`crate::Palette`]: it paints through the
//! palette's own [`crate::primitive::Primitive`] and routes input through
//! the palette's [`crate::interactive::Interactive`], which owns query
//! editing and list navigation. What [`FolderKind`] adds is what the rows
//! mean: only directories (plus any
//! [`crate::compose::WalkOptions::extra_file_names`]) are listed, a `.`
//! row names the browsing root itself, and Enter confirms the highlighted
//! folder rather than browsing into it.
//!
//! # Relation to vimcode
//!
//! Extracted from vimcode's TUI-local folder-picker state
//! (`src/tui_main/mod.rs`, ~285 lines). vimcode's TUI and GTK
//! `OpenFolderDialog` are the migration target.
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! // Instantiate when the user triggers "Open Folder":
//! let mut picker = FolderPicker::rooted_at(
//!     std::env::current_dir().unwrap(),
//!     vec![],       // no extra file names to surface
//! );                 // hidden dirs stay off by default; chain
//!                    // `.with_show_hidden(true)` to surface them
//!
//! // In AppLogic::render:
//! backend.draw_dyn(rect, &picker, &());
//!
//! // In AppLogic::handle:
//! let visible_rows = /* rect.height as usize - PALETTE_CHROME_ROWS */;
//! match picker.handle_event(&event, visible_rows) {
//!     Handled::Event(PathPickerAction::Confirmed { path }) => { /* open workspace */ },
//!     Handled::Event(PathPickerAction::Cancelled)          => { /* dismiss modal */ },
//!     Handled::Event(change) => picker.apply(&change, visible_rows),
//!     _                                                    => {},
//! }
//! ```
//!
//! An app on the runtime's routing engine pushes the [`FolderPicker`]
//! itself as a `Surface::Widget`; its actions arrive as
//! [`crate::UiEvent::PathPicker`], and the app hands each to
//! [`PathPicker::apply`].

use std::path::PathBuf;

use super::path_picker::{PathPicker, PathPickerAction, PickerKind};
use crate::compose::Handled;
use crate::Icon;

/// The [`PickerKind`] of a [`FolderPicker`]: directories only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderKind;

/// A directory-browsing picker, painted as a [`crate::Palette`].
pub type FolderPicker = PathPicker<FolderKind>;

impl PickerKind for FolderKind {
    const DEFAULT_ID: &'static str = "folder_picker";
    /// The `.` row lets the user open the current directory directly.
    const ROOT_ROW: bool = true;

    fn lists_file(&self, _name: &str) -> bool {
        false
    }

    /// The folder picker's query is never a destination filename, so the
    /// vim shortcuts always take priority over typing them.
    fn vim_keys(&self) -> bool {
        true
    }

    fn title(&self) -> &'static str {
        "Open Folder"
    }

    /// ⚙ — a surfaced extra file is a marker, not a folder.
    fn file_icon(&self) -> Icon {
        Icon::codicon("gear", "\u{2699}").unwrap_or_else(|| Icon::new("\u{2699}", "\u{2699}"))
    }

    fn create_label(&self, _query: &str) -> Option<String> {
        None
    }

    /// `..` browses up; any other row confirms that folder (or surfaced
    /// file).
    fn confirm(picker: &FolderPicker) -> Handled<PathPickerAction> {
        match picker.paths.highlighted() {
            Some(rel) if rel.as_os_str() == ".." => match picker.paths.parent() {
                Some(root) => Handled::Event(PathPickerAction::Navigated { root }),
                None => Handled::Consumed,
            },
            Some(_) => match picker.selected_path() {
                Some(path) => Handled::Event(PathPickerAction::Confirmed { path }),
                None => Handled::Consumed,
            },
            None => Handled::Consumed,
        }
    }
}

impl PathPicker<FolderKind> {
    /// A folder picker rooted at `root`, walked with the default options.
    /// Hidden directories stay hidden unless surfaced with
    /// [`Self::with_show_hidden`].
    ///
    /// - `root` — initial browsing directory (typically
    ///   `env::current_dir()` or the engine's active workspace root).
    /// - `extra_file_names` — dotfile names to surface even with hidden
    ///   entries hidden (e.g. `[".vimcode-workspace".to_string()]`).
    ///   Pass an empty `Vec` for the common case.
    pub fn rooted_at(root: impl Into<PathBuf>, extra_file_names: Vec<String>) -> Self {
        let picker = Self::with_kind(FolderKind, root);
        if extra_file_names.is_empty() {
            picker
        } else {
            picker.with_extra_file_names(extra_file_names)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::path_picker::{drive, settle};
    use crate::{Key, Modifiers, NamedKey, Rect, UiEvent};

    /// Enter's resolution, applied the way a host applies it.
    fn confirm_applied<K: PickerKind>(picker: &mut PathPicker<K>) -> Handled<PathPickerAction> {
        let handled = picker.confirm();
        settle(picker, handled, 0)
    }

    /// Number of child directories [`picker_root`] creates. Chosen so
    /// `filtered()` (which also carries `".."` and `"."`) holds well over
    /// ten entries, the largest selection index any test below drives.
    const PICKER_ROOT_CHILDREN: usize = 12;

    /// A throwaway browsing root holding [`PICKER_ROOT_CHILDREN`] empty
    /// child directories.
    ///
    /// Every controller test roots its picker here rather than at the
    /// ambient `env::temp_dir()`: `FolderPicker::new` walks the
    /// root recursively to `DEFAULT_MAX_DEPTH`, so a shared system temp
    /// directory makes both the entry list and the walk's cost depend on
    /// whatever else happens to live there. The returned `TempDir` must
    /// stay bound for the lifetime of the picker — dropping it deletes
    /// the directory.
    fn picker_root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("create picker tempdir");
        for i in 0..PICKER_ROOT_CHILDREN {
            std::fs::create_dir(dir.path().join(format!("child{i:02}"))).expect("create child dir");
        }
        dir
    }

    // Fuzzy-scoring and filtering live in `path_picker` now — see
    // `super::path_picker::tests` for their coverage. This module only
    // tests `FolderPicker`'s own behaviour.

    // ── FolderPicker behaviour ────────────────────────────────────────

    #[test]
    fn new_sets_dotdot_and_dot_entries() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPicker::rooted_at(tmp, vec![]);
        // ".." and "." should always be the first two entries.
        assert!(picker.paths.all_entries.contains(&PathBuf::from("..")));
        assert!(picker.paths.all_entries.contains(&PathBuf::from(".")));
        // An isolated root makes the whole walk deterministic: "..", "."
        // and one entry per child directory, nothing else.
        assert_eq!(
            picker.paths.filtered.len(),
            PICKER_ROOT_CHILDREN + 2,
            "isolated root must yield exactly \"..\", \".\" and one entry per child"
        );
    }

    #[test]
    fn with_show_hidden_surfaces_dot_directories() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        std::fs::create_dir(tmp.join(".hidden-child")).expect("create hidden child dir");

        let hidden_off = FolderPicker::rooted_at(tmp.clone(), vec![]);
        assert!(
            !hidden_off
                .paths
                .all_entries
                .contains(&PathBuf::from(".hidden-child")),
            "hidden directories stay hidden by default"
        );

        let hidden_on = FolderPicker::rooted_at(tmp, vec![]).with_show_hidden(true);
        assert!(
            hidden_on
                .paths
                .all_entries
                .contains(&PathBuf::from(".hidden-child")),
            "with_show_hidden(true) surfaces hidden directories"
        );
    }

    #[test]
    fn move_up_clamps_at_zero() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        assert_eq!(picker.paths.selected, 0);
        picker.paths.move_up();
        assert_eq!(picker.paths.selected, 0);
    }

    #[test]
    fn move_down_advances_selection() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        assert!(picker.paths.filtered.len() >= 2);
        picker.paths.move_down();
        assert_eq!(picker.paths.selected, 1);
    }

    #[test]
    fn move_down_clamps_at_last() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        let last = picker.paths.filtered.len().saturating_sub(1);
        for _ in 0..last + 5 {
            picker.paths.move_down();
        }
        assert_eq!(picker.paths.selected, last);
    }

    #[test]
    fn sync_scroll_brings_selected_into_view() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        // Force a large selection index.
        assert!(picker.paths.filtered.len() >= 10);
        picker.paths.selected = 9;
        picker.paths.sync_scroll(5);
        assert!(picker.paths.scroll_top + 5 > picker.paths.selected);
        assert!(picker.paths.scroll_top <= picker.paths.selected);
    }

    #[test]
    fn sync_scroll_scrolls_up_when_needed() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        assert!(picker.paths.filtered.len() >= 5);
        picker.paths.scroll_top = 5;
        picker.paths.selected = 2; // above scroll_top
        picker.paths.sync_scroll(3);
        assert_eq!(picker.paths.scroll_top, 2);
    }

    #[test]
    fn escape_event_returns_cancelled() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(
            drive(&mut picker, &ev, 10),
            Handled::Event(PathPickerAction::Cancelled)
        );
    }

    #[test]
    fn enter_on_dotdot_navigates_up() {
        // Use tempfile so parallel test runs don't race on a fixed name and
        // a crashed prior run can't leak a stale marker directory.
        let parent = tempfile::tempdir().expect("create parent tempdir");
        let sub = parent.path().join("nav_up_child");
        std::fs::create_dir_all(&sub).expect("create child dir");
        let mut picker = FolderPicker::rooted_at(sub.clone(), vec![]);
        // Select ".." (index 0, always prepended when parent exists).
        picker.paths.selected = 0;
        assert_eq!(picker.paths.filtered.first(), Some(&PathBuf::from("..")));
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = drive(&mut picker, &ev, 10);
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.paths.root, parent.path());
    }

    #[test]
    fn enter_on_dot_returns_confirmed_with_root() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp.clone(), vec![]);
        // "." is always at index 1 (after "..") when parent exists.
        let dot_idx = picker
            .paths
            .filtered
            .iter()
            .position(|p| p.as_os_str() == ".")
            .expect("'.' should always be in filtered");
        picker.paths.selected = dot_idx;
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = drive(&mut picker, &ev, 10);
        assert_eq!(
            result,
            Handled::Event(PathPickerAction::Confirmed { path: tmp })
        );
    }

    #[test]
    fn up_down_key_moves_selection() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        assert!(picker.paths.filtered.len() >= 2);
        let ev_down = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = drive(&mut picker, &ev_down, 10);
        assert_eq!(picker.paths.selected, 1);

        let ev_up = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = drive(&mut picker, &ev_up, 10);
        assert_eq!(picker.paths.selected, 0);
    }

    #[test]
    fn key_char_appends_to_query() {
        // Query characters arrive via `Key::Char` in `KeyPressed`, *not* via
        // `CharTyped`. The latter is intentionally ignored to avoid
        // double-insert when a future backend emits both for the same input.
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('s'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = drive(&mut picker, &ev, 10);
        assert_eq!(picker.paths.query, "s");
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend, so it must not be typed into the filter query or trigger
    // a vim-style navigation shortcut.
    #[test]
    fn cmd_char_is_ignored_not_typed_into_query() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('j'),
            modifiers: Modifiers {
                cmd: true,
                ..Default::default()
            },
            repeat: false,
        };
        let result = drive(&mut picker, &ev, 10);
        assert_eq!(result, Handled::Ignored);
        assert_eq!(picker.paths.query, "");
    }

    #[test]
    fn char_typed_is_ignored() {
        // Documented contract: `CharTyped` is not the canonical typing
        // source; only `Key::Char` in `KeyPressed` mutates the query.
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        let ev = UiEvent::CharTyped('s');
        let result = drive(&mut picker, &ev, 10);
        assert_eq!(result, Handled::Ignored);
        assert_eq!(picker.paths.query, "");
    }

    #[test]
    fn backspace_removes_last_char() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPicker::rooted_at(tmp, vec![]);
        picker.paths.set_query("sr".into());
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Backspace),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = drive(&mut picker, &ev, 10);
        assert_eq!(picker.paths.query, "s");
    }

    #[test]
    fn selected_path_resolves_dot_to_root() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPicker::rooted_at(tmp.clone(), vec![]);
        let dot_idx = picker
            .paths
            .filtered
            .iter()
            .position(|p| p.as_os_str() == ".")
            .unwrap();
        let mut picker = picker;
        picker.paths.selected = dot_idx;
        assert_eq!(picker.selected_path(), Some(tmp));
    }

    #[test]
    fn dash_key_navigates_up() {
        let parent = tempfile::tempdir().expect("create parent tempdir");
        let sub = parent.path().join("dash_nav_child");
        std::fs::create_dir_all(&sub).expect("create child dir");
        let mut picker = FolderPicker::rooted_at(sub.clone(), vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('-'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = drive(&mut picker, &ev, 10);
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.paths.root, parent.path());
    }

    #[test]
    fn build_palette_id_is_folder_picker() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPicker::rooted_at(tmp, vec![]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.palette(rect.width, picker.paths.query.len());
        assert_eq!(palette.id.as_str(), "folder_picker");
    }

    #[test]
    fn build_palette_title_contains_open_folder() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPicker::rooted_at(tmp, vec![]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.palette(rect.width, picker.paths.query.len());
        assert!(palette.title.starts_with("Open Folder"));
    }

    #[test]
    fn build_palette_items_match_filtered() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPicker::rooted_at(tmp, vec![]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.palette(rect.width, picker.paths.query.len());
        assert_eq!(palette.items.len(), picker.paths.filtered.len());
    }

    #[test]
    fn extra_file_names_gets_file_icon() {
        // Isolate the marker file in a fresh tempdir so concurrent test
        // runs don't race and a crashed prior run can't leak state.
        let root = tempfile::tempdir().expect("create marker tempdir");
        let marker = root.path().join(".my-workspace");
        std::fs::write(&marker, b"").expect("write marker file");
        let picker =
            FolderPicker::rooted_at(root.path().to_path_buf(), vec![".my-workspace".to_string()]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.palette(rect.width, picker.paths.query.len());
        // The marker file should appear in items with the ⚙ icon.
        let has_marker = palette.items.iter().any(|item| {
            item.text
                .spans
                .first()
                .map(|s| s.text.contains(".my-workspace"))
                .unwrap_or(false)
        });
        assert!(has_marker, "marker file should appear in palette items");
    }

    #[test]
    fn build_palette_handles_non_ascii_root_path() {
        // `build_palette` must handle non-ASCII paths correctly: it must
        // validate UTF-8 char boundaries when slicing the root path to fit
        // the viewport width, rather than panicking on multi-byte characters.
        let long_japanese = PathBuf::from(format!("/a/{}", "ト".repeat(50)));
        let picker = FolderPicker::rooted_at(long_japanese, vec![]);
        // Narrow rect forces truncation: max = max(80-30, 10) = 50 bytes.
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        // Must not panic.
        let palette = picker.palette(rect.width, picker.paths.query.len());
        assert!(palette.title.starts_with("Open Folder "));
        // Sanity: ellipsis should be present and the suffix should still
        // be valid UTF-8 (implicit — `format!` would have panicked otherwise).
        assert!(palette.title.contains('…'));
    }

    #[test]
    fn build_palette_handles_accented_root_path() {
        // Regression: shorter multi-byte (Latin-1 / Latin-Extended) paths
        // should also be safe.
        let path = PathBuf::from(
            "/home/utilisateur/répertoire/sous-répertoire/encore-plus-profond/données",
        );
        let picker = FolderPicker::rooted_at(path, vec![]);
        let rect = Rect::new(0.0, 0.0, 60.0, 24.0);
        let palette = picker.palette(rect.width, picker.paths.query.len());
        assert!(palette.title.starts_with("Open Folder "));
    }

    // ── Builder-method overrides ──────────────────────────────────────

    #[test]
    fn with_id_overrides_widget_id() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPicker::rooted_at(tmp, vec![]).with_id("picker_b");
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        assert_eq!(
            picker
                .palette(rect.width, picker.paths.query.len())
                .id
                .as_str(),
            "picker_b"
        );
    }

    #[test]
    fn with_ignore_dirs_replaces_default_list() {
        let root = tempfile::tempdir().expect("tempdir");
        // Create `target` and `keep_me`; default list skips `target`.
        std::fs::create_dir_all(root.path().join("target")).unwrap();
        std::fs::create_dir_all(root.path().join("keep_me")).unwrap();
        // With an empty override list, `target` is no longer skipped.
        let picker =
            FolderPicker::rooted_at(root.path().to_path_buf(), vec![]).with_ignore_dirs(vec![]);
        let has_target = picker
            .paths
            .all_entries
            .iter()
            .any(|p| p == &PathBuf::from("target"));
        let has_keep = picker
            .paths
            .all_entries
            .iter()
            .any(|p| p == &PathBuf::from("keep_me"));
        assert!(
            has_target,
            "target should be present after with_ignore_dirs(empty)"
        );
        assert!(has_keep);
    }

    #[test]
    fn with_max_depth_limits_recursion() {
        let root = tempfile::tempdir().expect("tempdir");
        // Build a nested chain: depth1/depth2/depth3
        let d3 = root.path().join("d1").join("d2").join("d3");
        std::fs::create_dir_all(&d3).unwrap();
        // depth=0 should only emit the top-level "d1".
        let picker = FolderPicker::rooted_at(root.path().to_path_buf(), vec![]).with_max_depth(0);
        let has_d1 = picker.paths.all_entries.iter().any(|p| p.ends_with("d1"));
        let has_d2 = picker.paths.all_entries.iter().any(|p| p.ends_with("d2"));
        assert!(has_d1);
        assert!(!has_d2, "depth=0 should not surface nested subdirs");
    }

    // ── Drift fixes shared through `PathPicker` ───────────────────────

    #[test]
    fn navigating_clears_the_query_caret_and_anchor() {
        use crate::compose::path_picker::PathPickerEditState;
        let parent = tempfile::tempdir().expect("create parent tempdir");
        let sub = parent.path().join("caret_child");
        std::fs::create_dir_all(&sub).expect("create child dir");
        let mut picker = FolderPicker::rooted_at(sub, vec![]);
        picker.paths.set_query("ab".into());
        let mut state = PathPickerEditState::default();
        state.palette.cursor = Some(1);
        state.palette.anchor = Some(2);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('-'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let handled = crate::compose::widget_host::drive_in_cells(
            &picker,
            &mut state,
            &ev,
            Rect::new(0.0, 0.0, 80.0, 14.0),
        )
        .expect("a key press routes")
        .handled;
        assert_eq!(
            handled,
            Handled::Event(PathPickerAction::Navigated {
                root: parent.path().to_path_buf()
            })
        );
        assert_eq!(
            state,
            PathPickerEditState::default(),
            "the new root's empty query starts with a fresh caret"
        );
    }

    #[test]
    fn show_hidden_keeps_the_extra_file_names() {
        let dir = picker_root();
        std::fs::write(dir.path().join(".my-workspace"), b"").expect("write marker");
        std::fs::create_dir(dir.path().join(".hidden-child")).expect("create hidden dir");
        let picker = FolderPicker::rooted_at(dir.path(), vec![".my-workspace".to_string()])
            .with_show_hidden(true);
        assert!(picker
            .paths
            .all_entries
            .contains(&PathBuf::from(".hidden-child")));
        assert!(
            picker
                .paths
                .all_entries
                .contains(&PathBuf::from(".my-workspace")),
            "surfacing hidden directories must not drop the extra files"
        );
    }

    #[test]
    fn ignore_dirs_skips_only_directories() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("target"), b"").expect("write marker file");
        let picker = FolderPicker::rooted_at(root.path(), vec!["target".to_string()]);
        assert!(
            picker.paths.all_entries.contains(&PathBuf::from("target")),
            "a surfaced file named like an ignored directory is still listed"
        );
    }

    #[test]
    fn enter_with_no_rows_is_consumed() {
        let dir = picker_root();
        let mut picker = FolderPicker::rooted_at(dir.path(), vec![]);
        picker.paths.filtered.clear();
        assert_eq!(confirm_applied(&mut picker), Handled::Consumed);
    }
}
