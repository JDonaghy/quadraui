//! The sidebar's scroll arithmetic: a tree section's row offset, the
//! panel scroll, and the panel scroll that brings a section into view.

use super::sidebar_system::{Sidebar, SidebarAction, SidebarLayout};
use crate::compose::Handled;

impl Sidebar {
    /// The scroll action moving section `section`'s tree by `rows`,
    /// clamped to its last page.
    pub(crate) fn scroll_section(
        &self,
        section: usize,
        rows: isize,
        layout: &SidebarLayout,
    ) -> Handled<SidebarAction> {
        let Some(tree) = self.tree(section) else {
            return Handled::Ignored;
        };
        let max = tree
            .rows
            .len()
            .saturating_sub(layout.viewport_rows(section)) as isize;
        let offset = (tree.scroll_offset as isize + rows).clamp(0, max.max(0)) as usize;
        if offset == tree.scroll_offset {
            return Handled::Consumed;
        }
        Handled::Event(SidebarAction::ScrollChanged { section, offset })
    }

    /// The panel scroll action moving the panel by `delta`, clamped.
    pub(crate) fn scroll_panel(
        &self,
        delta: f32,
        layout: &SidebarLayout,
    ) -> Handled<SidebarAction> {
        let (_, max) = layout.panel_extent();
        let offset = (self.panel_scroll + delta).clamp(0.0, max);
        if (offset - self.panel_scroll).abs() < f32::EPSILON {
            return Handled::Consumed;
        }
        Handled::Event(SidebarAction::PanelScrolled { offset })
    }

    /// The panel scroll that brings sidebar section `section` into view,
    /// when it is not already.
    pub(crate) fn reveal_section_scroll(
        &self,
        section: usize,
        layout: &SidebarLayout,
    ) -> Option<f32> {
        let s = layout.msv.section(layout.view_index(section)?)?;
        let bounds = layout.msv.chrome.bounds;
        let (_, max) = layout.panel_extent();
        let top = s.header_bounds.y - bounds.y + self.panel_scroll;
        let bottom = top + s.resolved_size;
        if top < self.panel_scroll {
            Some(top.clamp(0.0, max))
        } else if bottom > self.panel_scroll + bounds.height {
            Some((bottom - bounds.height).clamp(0.0, max))
        } else {
            None
        }
    }
}
