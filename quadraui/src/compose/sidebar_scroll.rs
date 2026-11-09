//! Scroll bookkeeping for [`crate::compose::SidebarSystem`]: whole-panel
//! scroll, scroll-to-active-section, and per-section row scroll.
//!
//! An `impl SidebarSystem` block in its own file, not a separate type —
//! every method here still needs the whole-system layout
//! ([`SidebarSystem::compute_layout`]) and section list
//! ([`SidebarSystem::sections`]), so there is no bounded sub-state to
//! extract into a sidecar struct the way [`crate::compose::DividerDrag`]
//! or `ActivityBarKeyboard` could. `sections`/`focus`/`panel_scroll` and
//! the `compute_layout`/`viewport_rows_from_layout` helpers are
//! `pub(crate)` on `SidebarSystem` precisely so this sibling module can
//! reach them; see [`SidebarSystem::sections`]'s own doc for why.

use super::sidebar_system::{SectionController, SidebarSystem};
use crate::primitives::multi_section_view::MsvLayoutMetrics;
use crate::{MultiSectionViewHit, Point, Rect};

impl SidebarSystem {
    /// `pub(crate)`: called from `handle_inner` in `sidebar_system.rs`, a
    /// sibling module's method — see [`SidebarSystem::sections`]'s doc for
    /// why that needs crate-wide visibility rather than private.
    pub(crate) fn scroll_panel(
        &mut self,
        rect: Rect,
        dy: f32,
        lh: f32,
        metrics: &MsvLayoutMetrics,
    ) {
        let (layout, _) = self.compute_layout(rect, metrics, lh);
        let total: f32 = layout.sections.iter().map(|s| s.resolved_size).sum();
        let max = (total - rect.height).max(0.0);
        self.panel_scroll = (self.panel_scroll + dy).clamp(0.0, max);
    }

    /// `pub(crate)`: see [`Self::scroll_panel`]'s doc.
    pub(crate) fn scroll_to_active_section(
        &mut self,
        rect: Rect,
        lh: f32,
        metrics: &MsvLayoutMetrics,
    ) {
        let Some(idx) = self.focus.active() else {
            return;
        };
        let (layout, map) = self.compute_layout(rect, metrics, lh);
        let Some(msv_idx) = map.iter().position(|&s| s == idx) else {
            return;
        };
        if msv_idx >= layout.sections.len() {
            return;
        }
        let total: f32 = layout.sections.iter().map(|s2| s2.resolved_size).sum();
        let max = (total - rect.height).max(0.0);
        let s = &layout.sections[msv_idx];
        let content_top = s.header_bounds.y - rect.y + self.panel_scroll;
        let content_bottom = content_top + s.resolved_size;
        if content_top < self.panel_scroll {
            self.panel_scroll = content_top.clamp(0.0, max);
        } else if content_bottom > self.panel_scroll + rect.height {
            self.panel_scroll = (content_bottom - rect.height).clamp(0.0, max);
        }
    }

    /// Route a scroll-wheel event to the tree section whose bounds
    /// contain `position`. Returns `true` when a section was hit and
    /// its scroll offset updated; returns `false` when the position
    /// lies outside all sections or over a non-tree section (caller
    /// falls back to active-section scroll).
    ///
    /// Used exclusively from the `PerSection` scroll-mode path in
    /// `handle_inner` so that hovering over section B while section A
    /// is active scrolls section B — matching the native desktop
    /// convention (scroll follows cursor, not focus).
    ///
    /// `pub(crate)`: see [`Self::scroll_panel`]'s doc.
    pub(crate) fn scroll_section_at(
        &mut self,
        position: Point,
        rect: Rect,
        rows: isize,
        lh: f32,
        metrics: &MsvLayoutMetrics,
    ) -> bool {
        let (layout, map) = self.compute_layout(rect, metrics, lh);
        let msv_idx = match layout.hit_test(position.x, position.y) {
            MultiSectionViewHit::Body { section }
            | MultiSectionViewHit::Header { section, .. }
            | MultiSectionViewHit::Scrollbar { section, .. } => section,
            _ => return false,
        };
        let section = map[msv_idx];
        let body_b = layout.sections[msv_idx].body_bounds;
        let viewport_rows = self.viewport_rows_from_layout(body_b, msv_idx, lh);
        let SectionController::Tree(tc) = &mut self.sections[section] else {
            return false;
        };
        let row_count = tc.rows().len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = tc.scroll_offset() as isize;
        let new = (cur + rows).max(0).min(max) as usize;
        tc.set_scroll_offset(new);
        true
    }

    /// `pub(crate)`: see [`Self::scroll_panel`]'s doc.
    pub(crate) fn scroll_active(
        &mut self,
        rect: Rect,
        delta: isize,
        lh: f32,
        metrics: &MsvLayoutMetrics,
    ) {
        let Some(idx) = self.focus.active() else {
            return;
        };
        if !matches!(self.sections[idx], SectionController::Tree(_)) {
            return;
        }
        let (layout, map) = self.compute_layout(rect, metrics, lh);
        let Some(msv_idx) = map.iter().position(|&s| s == idx) else {
            return;
        };
        let body_b = layout.sections[msv_idx].body_bounds;
        let viewport_rows = self.viewport_rows_from_layout(body_b, msv_idx, lh);
        let SectionController::Tree(tc) = &mut self.sections[idx] else {
            return;
        };
        let row_count = tc.rows().len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = tc.scroll_offset() as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        tc.set_scroll_offset(new);
    }
}
