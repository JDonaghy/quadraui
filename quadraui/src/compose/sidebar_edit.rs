//! Inline-editing forwarders for [`crate::compose::SidebarSystem`]'s
//! Tree-bodied sections.
//!
//! An `impl SidebarSystem` block in its own file, not a separate type:
//! none of these methods need state beyond what `SidebarSystem` already
//! owns (`sections`), so there is nothing to extract into a sidecar
//! struct — only the methods move. `sections`/[`SectionController`] are
//! `pub(crate)` on `SidebarSystem` precisely so this sibling module can
//! reach them; see that field's own doc for why.

use super::sidebar_system::{SectionController, SidebarEvent, SidebarSystem};
use super::tree_controller::TreeControllerEvent;
use crate::{Key, Modifiers, TreePath};

impl SidebarSystem {
    // ── Inline editing ───────────────────────────────────────────────

    pub fn start_editing(
        &mut self,
        section: usize,
        path: TreePath,
        initial_text: String,
        cursor: usize,
        selection_anchor: Option<usize>,
        placeholder: Option<String>,
    ) {
        if let Some(SectionController::Tree(tc)) = self.sections.get_mut(section) {
            tc.start_editing(path, initial_text, cursor, selection_anchor, placeholder);
        }
    }

    pub fn cancel_editing(&mut self, section: usize) {
        if let Some(SectionController::Tree(tc)) = self.sections.get_mut(section) {
            tc.cancel_editing();
        }
    }

    pub fn is_editing(&self) -> bool {
        self.sections
            .iter()
            .any(|sc| matches!(sc, SectionController::Tree(tc) if tc.is_editing()))
    }

    // ── Inline editing forwarding ────────────────────────────────────

    fn editing_section(&self) -> Option<usize> {
        self.sections
            .iter()
            .position(|sc| matches!(sc, SectionController::Tree(tc) if tc.is_editing()))
    }

    /// `pub(crate)`: called from `handle_inner` in `sidebar_system.rs`, a
    /// sibling module's method — see [`SidebarSystem::sections`]'s doc for
    /// why that needs crate-wide visibility rather than private.
    pub(crate) fn forward_edit_char(&mut self, ch: char) -> SidebarEvent {
        let Some(idx) = self.editing_section() else {
            return SidebarEvent::Ignored;
        };
        let SectionController::Tree(tc) = &mut self.sections[idx] else {
            return SidebarEvent::Ignored;
        };
        tc.edit_insert_char_via(ch);
        self.map_tc_edit_event(idx)
    }

    /// `pub(crate)`: see [`Self::forward_edit_char`]'s doc.
    pub(crate) fn forward_edit_paste(&mut self, text: &str) -> SidebarEvent {
        let Some(idx) = self.editing_section() else {
            return SidebarEvent::Ignored;
        };
        let SectionController::Tree(tc) = &mut self.sections[idx] else {
            return SidebarEvent::Ignored;
        };
        tc.edit_insert_str_via(text);
        self.map_tc_edit_event(idx)
    }

    /// `pub(crate)`: see [`Self::forward_edit_char`]'s doc.
    pub(crate) fn forward_edit_key(&mut self, key: &Key, modifiers: &Modifiers) -> SidebarEvent {
        let Some(idx) = self.editing_section() else {
            return SidebarEvent::Ignored;
        };
        let SectionController::Tree(tc) = &mut self.sections[idx] else {
            return SidebarEvent::Ignored;
        };
        let ev = tc.handle_edit_key_via(key, modifiers);
        Self::map_tc_event(idx, ev)
    }

    fn map_tc_edit_event(&self, idx: usize) -> SidebarEvent {
        let SectionController::Tree(tc) = &self.sections[idx] else {
            return SidebarEvent::Consumed;
        };
        if let Some(path) = tc.editing_path() {
            SidebarEvent::EditChanged {
                section: idx,
                path: path.clone(),
                text: tc.editing_text().unwrap_or_default().to_string(),
            }
        } else {
            SidebarEvent::Consumed
        }
    }

    fn map_tc_event(idx: usize, ev: TreeControllerEvent) -> SidebarEvent {
        match ev {
            TreeControllerEvent::EditConfirmed { path, new_text } => SidebarEvent::EditConfirmed {
                section: idx,
                path,
                new_text,
            },
            TreeControllerEvent::EditCancelled { path } => {
                SidebarEvent::EditCancelled { section: idx, path }
            }
            TreeControllerEvent::EditChanged { path, text } => SidebarEvent::EditChanged {
                section: idx,
                path,
                text,
            },
            TreeControllerEvent::Consumed => SidebarEvent::Consumed,
            _ => SidebarEvent::Ignored,
        }
    }
}
