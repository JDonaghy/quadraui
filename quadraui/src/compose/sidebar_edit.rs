//! Inline row renaming on a [`SidebarSystem`]'s tree sections.
//!
//! A rename session is the row's [`TreeRowEditState`]: the tree's own
//! `Interactive` edits it and reports each keystroke, which the
//! controller stores. The session is kept here as well, so rows pushed
//! by [`SidebarSystem::set_rows`] mid-rename keep it.

use super::sidebar_system::SidebarSystem;
use crate::{TreePath, TreeRowEditState};

impl SidebarSystem {
    /// Begin inline editing of the row at `path` in tree section
    /// `section`, selecting it.
    ///
    /// `cursor` and `selection_anchor` are byte offsets into
    /// `initial_text`. For select-all: `cursor = len, anchor = Some(0)`.
    pub fn start_editing(
        &mut self,
        section: usize,
        path: TreePath,
        initial_text: String,
        cursor: usize,
        selection_anchor: Option<usize>,
        placeholder: Option<String>,
    ) {
        let edit = TreeRowEditState {
            text: initial_text,
            cursor,
            selection_anchor,
            placeholder,
        };
        let Some(tree) = self.sidebar.tree_mut(section) else {
            return;
        };
        for row in &mut tree.rows {
            row.edit = (row.path == path).then(|| edit.clone());
        }
        tree.selected_path = Some(path.clone());
        self.editing[section] = Some((path, edit));
    }

    /// Cancel editing in `section` programmatically.
    pub fn cancel_editing(&mut self, section: usize) {
        if let Some(slot) = self.editing.get_mut(section) {
            *slot = None;
        }
        if let Some(tree) = self.sidebar.tree_mut(section) {
            for row in &mut tree.rows {
                row.edit = None;
            }
        }
    }

    /// Whether any tree section is mid-rename.
    pub fn is_editing(&self) -> bool {
        self.editing.iter().any(Option::is_some)
    }
}
