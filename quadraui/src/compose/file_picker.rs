//! [`FilePicker`] — the [`PathPicker`] kind that browses **files** as well
//! as directories, with a save mode where the typed query doubles as the
//! destination filename.
//!
//! Shares [`crate::compose::FolderPicker`]'s fuzzy filter, walk,
//! [`crate::compose::PathPickerState`] browsing model and key map; this
//! module adds only [`FileKind`]: the extension filters, the Open/Save
//! modes and what Enter resolves to.
//!
//! `TuiPlatformServices::show_file_open_dialog` / `show_file_save_dialog`
//! drive the picker through a nested draw-and-read loop (see
//! `crate::tui::services`' module doc) so both methods return a real path
//! on TUI too — see `examples/tui_file_picker.rs` for the picker used
//! directly by an app.
//!
//! # Shares its key map with the folder picker — mode-aware
//!
//! Both pickers route keys through the same palette input, so they never
//! drift apart on the keys they share. The vim-style `-`/`j`/`k`
//! shortcuts follow the mode:
//!
//! - [`FilePickerMode::Open`]: `-`/`j`/`k` navigate (parent-dir/up/down),
//!   matching the folder picker, because there is nothing to type a name
//!   into.
//! - [`FilePickerMode::Save`]: `-`/`j`/`k` type into the query like any
//!   other character, because the query *is* the destination filename
//!   there (see "Save-mode semantics" below) and real filenames routinely
//!   contain all three (`my-notes.txt`, `journal.md`, `Makefile-k8s`).
//!
//! # Save-mode semantics
//!
//! In [`FilePickerMode::Save`], `query` is simultaneously the fuzzy
//! filter over existing entries (so the user can see what they'd
//! overwrite) *and* the destination filename. Enter on a directory row
//! navigates into it (clearing `query`, matching Open mode); Enter
//! otherwise confirms `root.join(query)` regardless of whether an entry
//! with that name currently exists — the standard "Save As" contract.
//! In [`FilePickerMode::Open`], Enter only ever confirms an existing,
//! currently-highlighted **file** row; there is nothing to "create".
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! let mut picker = FilePicker::new(
//!     FilePickerMode::Open,
//!     std::env::current_dir().unwrap(),
//!     vec![("Rust".to_string(), vec!["rs".to_string()])],
//! );
//!
//! // In AppLogic::render:
//! backend.draw_dyn(popup_rect, &picker, &());
//!
//! // In AppLogic::handle:
//! let visible_rows = popup_rect.height as usize - PALETTE_CHROME_ROWS;
//! match picker.handle_event(&event, visible_rows) {
//!     Handled::Event(PathPickerAction::Confirmed { path }) => { /* open/save `path` */ }
//!     Handled::Event(PathPickerAction::Cancelled)          => { /* dismiss modal */ }
//!     Handled::Event(change) => picker.apply(&change, visible_rows),
//!     _                                                    => {}
//! }
//! ```

use std::path::{Path, PathBuf};

use super::path_picker::{PathPicker, PathPickerAction, PickerKind};
use crate::compose::Handled;
use crate::Icon;

/// Whether a [`FilePicker`] is choosing a file to open (must exist) or a
/// destination to save to (may not exist yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePickerMode {
    /// Enter only confirms an existing, highlighted file.
    Open,
    /// Enter confirms `root.join(query)`, whether or not that name
    /// already exists — the query field doubles as the filename.
    Save,
}

/// The [`PickerKind`] of a [`FilePicker`]: its mode and the extension
/// filters its walk applies to files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileKind {
    /// Open or Save.
    pub mode: FilePickerMode,
    /// Extension filters — `(display_name, &[ext])` pairs, mirroring
    /// [`crate::backend::FileDialogOptions::filters`]. Empty means "show
    /// every file". Directories are never filtered out.
    pub filters: Vec<(String, Vec<String>)>,
}

/// A file open/save picker, painted as a [`crate::Palette`].
pub type FilePicker = PathPicker<FileKind>;

impl PickerKind for FileKind {
    const DEFAULT_ID: &'static str = "file_picker";
    /// Selecting the current directory itself is meaningless for a
    /// *file* picker.
    const ROOT_ROW: bool = false;

    fn lists_file(&self, name: &str) -> bool {
        extension_matches(&self.filters, name)
    }

    /// `Open` mode's query is never a filename, so the vim shortcuts take
    /// priority there; in `Save` mode they type.
    fn vim_keys(&self) -> bool {
        self.mode == FilePickerMode::Open
    }

    fn title(&self) -> &'static str {
        match self.mode {
            FilePickerMode::Open => "Open File",
            FilePickerMode::Save => "Save File",
        }
    }

    /// 📄 — the `file` codicon on a GUI backend, the emoji on TUI.
    fn file_icon(&self) -> Icon {
        Icon::codicon("file", "\u{1F4C4}").unwrap_or_else(|| Icon::new("\u{1F4C4}", "\u{1F4C4}"))
    }

    fn create_label(&self, query: &str) -> Option<String> {
        match self.mode {
            FilePickerMode::Save if !query.is_empty() => Some(format!("Save as \"{query}\"")),
            _ => None,
        }
    }

    /// The ".." row needs special-casing beyond "it's a directory": it's
    /// the row `selected` defaults to (index 0) whenever the entry list
    /// hasn't been touched, so treating it exactly like any other
    /// directory would mean a Save dialog opened with a pre-filled
    /// [`FilePicker::with_initial_filename`] and confirmed with an
    /// *immediate* Enter — no arrowing, no typing — would silently
    /// navigate up a directory instead of saving. A typed/seeded Save-mode
    /// filename takes priority over that unmoved default; explicitly
    /// arrowing onto a *real* directory row still browses into it
    /// regardless of mode or query, matching native save dialogs' "Enter
    /// on a folder row browses" convention.
    fn confirm(picker: &FilePicker) -> Handled<PathPickerAction> {
        let paths = &picker.paths;
        let highlighted = paths.highlighted();
        let save_as = || {
            Handled::Event(PathPickerAction::Confirmed {
                path: paths.root.join(&paths.query),
            })
        };
        let save = picker.kind.mode == FilePickerMode::Save;

        if highlighted.is_some_and(|p| p.as_os_str() == "..") {
            if save && !paths.query.is_empty() {
                return save_as();
            }
            return match paths.parent() {
                Some(root) => Handled::Event(PathPickerAction::Navigated { root }),
                None => Handled::Consumed,
            };
        }

        if let Some(rel) = highlighted {
            if picker.is_dir_entry(rel) {
                return Handled::Event(PathPickerAction::Navigated {
                    root: paths.root.join(rel),
                });
            }
        }

        match (picker.kind.mode, highlighted) {
            (FilePickerMode::Open, Some(rel)) => Handled::Event(PathPickerAction::Confirmed {
                path: paths.root.join(rel),
            }),
            (FilePickerMode::Save, _) if !paths.query.is_empty() => save_as(),
            _ => Handled::Consumed,
        }
    }
}

impl PathPicker<FileKind> {
    /// A file picker rooted at `root`.
    ///
    /// `filters` restricts which **files** are listed (directories are
    /// always shown, so the user can navigate through them regardless of
    /// filter) — `(display_name, extensions)` pairs; an empty `Vec`
    /// shows every file.
    pub fn new(
        mode: FilePickerMode,
        root: impl Into<PathBuf>,
        filters: Vec<(String, Vec<String>)>,
    ) -> Self {
        Self::with_kind(FileKind { mode, filters }, root)
    }

    /// Seed the query (the Save-mode destination filename), mirroring
    /// [`crate::backend::FileDialogOptions::initial_filename`]. The list
    /// stays unfiltered. Returns `self` for builder-style chaining.
    pub fn with_initial_filename(mut self, name: impl Into<String>) -> Self {
        self.paths.query = name.into();
        self
    }

    /// Open or Save.
    pub fn mode(&self) -> FilePickerMode {
        self.kind.mode
    }
}

// ── Filters ───────────────────────────────────────────────────────────────

/// Whether `name` passes `filters` — an empty `filters` list accepts
/// everything; otherwise `name`'s extension must match at least one
/// extension in at least one filter (via the same
/// [`crate::backend::normalize_filter_extension`] every backend's own
/// filter mapping runs its extensions through, so `".rs"`, `"rs"`, and
/// `"RS"` all mean the same filter here too), or a filter must carry the
/// wildcard `"*"`. The wildcard check runs on the *normalized* filter
/// entry, not the raw string a caller wrote, so `".*"` is a wildcard here
/// too — [`crate::backend::normalize_filter_extension`] strips every
/// leading dot before comparing, and a literal `".*"` extension would
/// never match a real file's extension (`Path::extension` never returns
/// `"*"`) otherwise.
fn extension_matches(filters: &[(String, Vec<String>)], name: &str) -> bool {
    if filters.is_empty() {
        return true;
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(crate::backend::normalize_filter_extension)
        .unwrap_or_default();
    filters.iter().any(|(_, exts)| {
        exts.iter().any(|e| {
            let e = crate::backend::normalize_filter_extension(e);
            e == "*" || e == ext
        })
    })
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

    fn scratch_dir_with(files: &[&str], dirs: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        for f in files {
            std::fs::write(tmp.path().join(f), b"").expect("write file");
        }
        for d in dirs {
            std::fs::create_dir_all(tmp.path().join(d)).expect("mkdir");
        }
        tmp
    }

    #[test]
    fn new_lists_files_and_dirs() {
        let tmp = scratch_dir_with(&["a.rs", "b.txt"], &["sub"]);
        let picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        assert!(picker.paths.all_entries.contains(&PathBuf::from("a.rs")));
        assert!(picker.paths.all_entries.contains(&PathBuf::from("b.txt")));
        assert!(picker.paths.all_entries.contains(&PathBuf::from("sub")));
    }

    /// A directory row's icon is `Icon::codicon("folder", "📁")` and a
    /// file row's is `Icon::codicon("file", "📄")` — the GUI glyph
    /// resolves to the real codicon, and the fallback is the pre-existing
    /// emoji, so a TUI host (`nerd_fonts_enabled` defaults `false`) still
    /// paints exactly what it painted before this glyph was added.
    #[test]
    fn row_icons_resolve_to_codicon_on_gui_and_emoji_fallback_on_tui() {
        let tmp = scratch_dir_with(&["a.rs"], &["sub"]);
        let picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.palette(rect.width, picker.paths.query.len());

        let file_item = palette
            .items
            .iter()
            .find(|i| i.text.spans.first().is_some_and(|s| s.text == "a.rs"))
            .expect("a.rs row present");
        let dir_item = palette
            .items
            .iter()
            .find(|i| i.text.spans.first().is_some_and(|s| s.text == "sub"))
            .expect("sub row present");

        let file_icon = file_item.icon.as_ref().expect("file row has an icon");
        let dir_icon = dir_item.icon.as_ref().expect("dir row has an icon");

        assert_eq!(file_icon.fallback, "\u{1F4C4}");
        assert_eq!(file_icon.resolve(false), "\u{1F4C4}");
        assert_eq!(file_icon.resolve(true), file_icon.glyph);
        assert_ne!(file_icon.glyph, "\u{1F4C4}");

        assert_eq!(dir_icon.fallback, "\u{1F4C1}");
        assert_eq!(dir_icon.resolve(false), "\u{1F4C1}");
        assert_eq!(dir_icon.resolve(true), dir_icon.glyph);
        assert_ne!(dir_icon.glyph, "\u{1F4C1}");
    }

    #[test]
    fn filters_restrict_files_not_dirs() {
        let tmp = scratch_dir_with(&["a.rs", "b.txt"], &["sub"]);
        let picker = FilePicker::new(
            FilePickerMode::Open,
            tmp.path(),
            vec![("Rust".to_string(), vec!["rs".to_string()])],
        );
        assert!(picker.paths.all_entries.contains(&PathBuf::from("a.rs")));
        assert!(!picker.paths.all_entries.contains(&PathBuf::from("b.txt")));
        assert!(
            picker.paths.all_entries.contains(&PathBuf::from("sub")),
            "directories bypass file filters so the user can still navigate through them"
        );
    }

    #[test]
    fn open_mode_enter_on_file_confirms_it() {
        let tmp = scratch_dir_with(&["a.rs"], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        let idx = picker
            .paths
            .filtered
            .iter()
            .position(|p| p == &PathBuf::from("a.rs"))
            .expect("a.rs listed");
        picker.paths.selected = idx;
        let result = confirm_applied(&mut picker);
        assert_eq!(
            result,
            Handled::Event(PathPickerAction::Confirmed {
                path: tmp.path().join("a.rs")
            })
        );
    }

    #[test]
    fn open_mode_enter_on_dir_navigates_into_it() {
        let tmp = scratch_dir_with(&[], &["sub"]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        let idx = picker
            .paths
            .filtered
            .iter()
            .position(|p| p == &PathBuf::from("sub"))
            .expect("sub listed");
        picker.paths.selected = idx;
        let result = confirm_applied(&mut picker);
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.paths.root, tmp.path().join("sub"));
    }

    #[test]
    fn enter_on_dotdot_navigates_up() {
        let parent = tempfile::tempdir().expect("parent tempdir");
        let sub = parent.path().join("child");
        std::fs::create_dir_all(&sub).expect("mkdir child");
        let mut picker = FilePicker::new(FilePickerMode::Open, sub.clone(), vec![]);
        picker.paths.selected = 0;
        assert_eq!(picker.paths.filtered.first(), Some(&PathBuf::from("..")));
        let result = confirm_applied(&mut picker);
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.paths.root, parent.path());
    }

    #[test]
    fn save_mode_enter_confirms_typed_name_even_if_absent() {
        let tmp = scratch_dir_with(&["existing.txt"], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Save, tmp.path(), vec![]);
        picker.paths.set_query("new".into());
        let result = confirm_applied(&mut picker);
        assert_eq!(
            result,
            Handled::Event(PathPickerAction::Confirmed {
                path: tmp.path().join("new")
            })
        );
    }

    #[test]
    fn save_mode_enter_with_empty_query_and_no_selection_is_consumed() {
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Save, tmp.path(), vec![]);
        // No entries at all (empty dir, no "..": tmp roots have a parent,
        // so ".." is still present) — clear it to hit the empty-query,
        // no-highlighted-row path directly.
        picker.paths.filtered.clear();
        let result = confirm_applied(&mut picker);
        assert_eq!(result, Handled::Consumed);
    }

    #[test]
    fn save_mode_immediate_enter_confirms_seeded_name_not_navigate_up() {
        // Regression: pressing Enter with no prior navigation/typing used
        // to always act on the highlighted row, which defaults to "..".
        // A Save dialog opened with `with_initial_filename` and confirmed
        // immediately must save under that name, not silently navigate
        // up a directory.
        let parent = tempfile::tempdir().expect("parent tempdir");
        let sub = parent.path().join("child");
        std::fs::create_dir_all(&sub).expect("mkdir child");
        let mut picker = FilePicker::new(FilePickerMode::Save, sub.clone(), vec![])
            .with_initial_filename("untitled.txt");
        assert_eq!(picker.paths.filtered.first(), Some(&PathBuf::from("..")));
        let result = confirm_applied(&mut picker);
        assert_eq!(
            result,
            Handled::Event(PathPickerAction::Confirmed {
                path: sub.join("untitled.txt")
            })
        );
        assert_eq!(picker.paths.root, sub, "must not have navigated");
    }

    #[test]
    fn save_mode_enter_on_dir_navigates_not_confirms() {
        let tmp = scratch_dir_with(&[], &["sub"]);
        let mut picker = FilePicker::new(FilePickerMode::Save, tmp.path(), vec![]);
        picker.paths.set_query("x".into()); // non-empty query — would confirm if not for the dir row
        let idx = picker
            .paths
            .all_entries
            .iter()
            .position(|p| p == &PathBuf::from("sub"))
            .expect("sub listed");
        // Force-select the dir row directly (typing "x" filtered it out of
        // `filtered`, since "sub" doesn't fuzzy-match "x"); this test only
        // cares about `confirm_selection`'s directory-vs-file branch, so
        // bypass the filter machinery.
        picker.paths.filtered = vec![picker.paths.all_entries[idx].clone()];
        picker.paths.selected = 0;
        let result = confirm_applied(&mut picker);
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.paths.root, tmp.path().join("sub"));
    }

    #[test]
    fn with_initial_filename_seeds_query() {
        let tmp = scratch_dir_with(&[], &[]);
        let picker = FilePicker::new(FilePickerMode::Save, tmp.path(), vec![])
            .with_initial_filename("untitled.txt");
        assert_eq!(picker.paths.query, "untitled.txt");
    }

    #[test]
    fn move_up_clamps_at_zero() {
        let tmp = scratch_dir_with(&["a.rs"], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        picker.paths.move_up();
        assert_eq!(picker.paths.selected, 0);
    }

    #[test]
    fn move_down_clamps_at_last() {
        let tmp = scratch_dir_with(&["a.rs", "b.rs"], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        let last = picker.paths.filtered.len().saturating_sub(1);
        for _ in 0..last + 5 {
            picker.paths.move_down();
        }
        assert_eq!(picker.paths.selected, last);
    }

    #[test]
    fn escape_cancels() {
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
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

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend, so it must not be typed into the filter query.
    #[test]
    fn cmd_char_is_ignored_not_typed_into_query() {
        let tmp = scratch_dir_with(&["a.rs"], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('f'),
            modifiers: Modifiers {
                cmd: true,
                ..Default::default()
            },
            repeat: false,
        };
        assert_eq!(drive(&mut picker, &ev, 10), Handled::Ignored);
        assert_eq!(picker.paths.query, "");
    }

    // ── Open mode: shared vim-key navigation (shared with FolderPicker) ─────────

    #[test]
    fn vim_j_and_k_move_selection_through_handle() {
        let tmp = scratch_dir_with(&["a.rs", "b.rs"], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        assert!(picker.paths.filtered.len() >= 2);

        let down = UiEvent::KeyPressed {
            key: Key::Char('j'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(drive(&mut picker, &down, 10), Handled::Consumed);
        assert_eq!(picker.paths.selected, 1);

        let up = UiEvent::KeyPressed {
            key: Key::Char('k'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(drive(&mut picker, &up, 10), Handled::Consumed);
        assert_eq!(picker.paths.selected, 0);
    }

    #[test]
    fn dash_key_navigates_up() {
        let parent = tempfile::tempdir().expect("parent tempdir");
        let sub = parent.path().join("dash_nav_child");
        std::fs::create_dir_all(&sub).expect("mkdir child");
        let mut picker = FilePicker::new(FilePickerMode::Open, sub.clone(), vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('-'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = drive(&mut picker, &ev, 10);
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.paths.root, parent.path());
    }

    // ── Save mode: vim shortcuts disabled, typing wins ──────────────────

    /// In `Save` mode, `-`/`j`/`k` must reach the query through the full
    /// `handle()` path — Save's query is
    /// the destination filename, so routing them as navigation would make
    /// `journal.md`, `my-notes.txt`, and `Makefile-k8s` untypeable and
    /// `-` actively destructive (it would call `navigate_up`, which
    /// clears the query the user had already typed).
    #[test]
    fn save_mode_vim_chars_type_into_query_through_handle() {
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Save, tmp.path(), vec![]);

        for c in "my-journal.md".chars() {
            let ev = UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers: Modifiers::default(),
                repeat: false,
            };
            assert_eq!(drive(&mut picker, &ev, 10), Handled::Consumed);
        }

        assert_eq!(picker.paths.query, "my-journal.md");
        assert_eq!(picker.paths.root, tmp.path(), "must not have navigated up");
    }

    /// Named arrow keys still navigate the selection in Save mode — only
    /// the vim letter/dash aliases are gated, not `Up`/`Down` themselves.
    #[test]
    fn save_mode_arrow_keys_still_navigate_selection() {
        let tmp = scratch_dir_with(&["a.txt", "b.txt"], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Save, tmp.path(), vec![]);
        assert!(picker.paths.filtered.len() >= 2);

        let down = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(drive(&mut picker, &down, 10), Handled::Consumed);
        assert_eq!(picker.paths.selected, 1);
    }

    #[test]
    fn fuzzy_match_finds_kube_system_style_entry() {
        // "kbs" as a non-contiguous subsequence should surface a
        // "kube-system"-shaped entry, the same ranking both pickers share
        // via `path_picker::filter_entries`.
        let tmp = scratch_dir_with(&[], &["kube-system"]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        picker.paths.set_query("kbs".into());
        assert!(
            picker
                .paths
                .filtered
                .iter()
                .any(|p| p == &PathBuf::from("kube-system")),
            "\"kbs\" should fuzzy-match \"kube-system\""
        );
    }

    #[test]
    fn build_palette_open_mode_has_no_create_label() {
        let tmp = scratch_dir_with(&["a.rs"], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        picker.paths.set_query("a".into());
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        assert!(picker
            .palette(rect.width, picker.paths.query.len())
            .create_label
            .is_none());
    }

    #[test]
    fn build_palette_save_mode_has_create_label_when_query_nonempty() {
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Save, tmp.path(), vec![]);
        picker.paths.set_query("x".into());
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let label = picker
            .palette(rect.width, picker.paths.query.len())
            .create_label;
        assert_eq!(label.as_deref(), Some("Save as \"x\""));
    }

    #[test]
    fn build_palette_title_reflects_mode() {
        let tmp = scratch_dir_with(&[], &[]);
        let open = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        let save = FilePicker::new(FilePickerMode::Save, tmp.path(), vec![]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        assert!(open
            .palette(rect.width, open.paths.query.len())
            .title
            .starts_with("Open File"));
        assert!(save
            .palette(rect.width, save.paths.query.len())
            .title
            .starts_with("Save File"));
    }

    #[test]
    fn with_id_overrides_widget_id() {
        let tmp = scratch_dir_with(&[], &[]);
        let picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]).with_id("picker_b");
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        assert_eq!(
            picker
                .palette(rect.width, picker.paths.query.len())
                .id
                .as_str(),
            "picker_b"
        );
    }

    /// A symlink pointing at a directory must still behave like a
    /// directory: listed even under an extension filter that its name
    /// fails, and navigable with Enter.
    ///
    /// This is the case [`dir_entry_is_dir`] deliberately falls back to
    /// `Path::is_dir` for — `DirEntry::file_type` describes the *link*,
    /// so consulting it alone would classify this row as a file and an
    /// active filter would then hide it. Unix-only because creating a
    /// symlink on Windows needs either developer mode or
    /// `SeCreateSymbolicLinkPrivilege`, neither of which a test may
    /// assume.
    #[cfg(unix)]
    #[test]
    fn symlinked_directory_is_listed_and_navigable_as_a_directory() {
        let tmp = scratch_dir_with(&["a.rs"], &["real_dir"]);
        std::os::unix::fs::symlink(tmp.path().join("real_dir"), tmp.path().join("link"))
            .expect("symlink");

        let mut picker = FilePicker::new(
            FilePickerMode::Open,
            tmp.path(),
            vec![("Rust".to_string(), vec!["rs".to_string()])],
        );
        assert!(
            picker.paths.all_entries.contains(&PathBuf::from("link")),
            "a symlink to a directory must bypass the file extension filter, \
             the same as a real directory"
        );

        let idx = picker
            .paths
            .filtered
            .iter()
            .position(|p| p == &PathBuf::from("link"))
            .expect("link listed");
        picker.paths.selected = idx;
        assert_eq!(confirm_applied(&mut picker), Handled::Consumed);
        assert_eq!(picker.paths.root, tmp.path().join("link"));
    }

    #[test]
    fn extension_matches_wildcard_accepts_everything() {
        assert!(extension_matches(
            &[("Any".to_string(), vec!["*".to_string()])],
            "whatever.zzz"
        ));
    }

    #[test]
    fn extension_matches_case_insensitive() {
        assert!(extension_matches(
            &[("Rust".to_string(), vec!["RS".to_string()])],
            "main.rs"
        ));
    }

    #[test]
    fn extension_matches_rejects_non_matching() {
        assert!(!extension_matches(
            &[("Rust".to_string(), vec!["rs".to_string()])],
            "main.txt"
        ));
    }

    /// `".*"` is a wildcard too, not just the bare `"*"` — the normalized
    /// comparison in [`extension_matches`] strips its leading dot before
    /// checking for the wildcard, so a filter written either way accepts
    /// everything.
    #[test]
    fn extension_matches_treats_a_dotted_wildcard_as_a_wildcard_too() {
        assert!(extension_matches(
            &[("Any".to_string(), vec![".*".to_string()])],
            "whatever.zzz"
        ));
    }

    #[test]
    fn open_mode_enter_with_no_rows_is_consumed() {
        let tmp = scratch_dir_with(&["a.rs"], &[]);
        let mut picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![]);
        picker.paths.filtered.clear();
        assert_eq!(
            confirm_applied(&mut picker),
            Handled::Consumed,
            "the modal picker owns Enter even with nothing to confirm"
        );
    }

    #[test]
    fn builders_rewalk_the_file_picker_too() {
        let tmp = scratch_dir_with(&[".env"], &["target", "d1/d2"]);
        let picker = FilePicker::new(FilePickerMode::Open, tmp.path(), vec![])
            .with_max_depth(0)
            .with_ignore_dirs(vec![])
            .with_extra_file_names(vec![".env".to_string()]);
        let all = &picker.paths.all_entries;
        assert!(all.contains(&PathBuf::from("target")));
        assert!(all.contains(&PathBuf::from(".env")));
        assert!(all.contains(&PathBuf::from("d1")));
        assert!(!all.iter().any(|p| p.ends_with("d2")));
    }

    #[test]
    fn handle_event_keeps_the_caret_for_paint_and_drops_it_on_dismissal() {
        use crate::compose::path_picker::PathPickerEditState;
        use crate::widget_state::with_widget_states;
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker =
            FilePicker::new(FilePickerMode::Save, tmp.path(), vec![]).with_id("caret_store");
        let key = |key| UiEvent::KeyPressed {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let stored = || {
            with_widget_states(|s| {
                s.get(&crate::WidgetId::new("caret_store"))
                    .and_then(|s| s.downcast_ref::<PathPickerEditState>())
                    .cloned()
            })
        };
        let _ = drive(&mut picker, &key(Key::Char('a')), 10);
        let _ = drive(&mut picker, &key(Key::Char('b')), 10);
        let _ = drive(&mut picker, &key(Key::Named(NamedKey::Left)), 10);
        assert_eq!(stored().and_then(|s| s.palette.cursor), Some(1));
        let _ = drive(&mut picker, &key(Key::Char('x')), 10);
        assert_eq!(picker.paths.query, "axb", "typing lands at the moved caret");
        assert_eq!(
            drive(&mut picker, &key(Key::Named(NamedKey::Escape)), 10),
            Handled::Event(PathPickerAction::Cancelled)
        );
        assert_eq!(stored(), None, "a dismissed picker leaves no state behind");
    }
}
