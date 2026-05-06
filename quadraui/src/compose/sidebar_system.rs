//! `SidebarSystem` — a composed controller for MSV + TreeView sidebar
//! panels.
//!
//! Owns the full interaction state machine: per-section scroll/selection,
//! active section cycling, scrollbar drag, keyboard navigation, and
//! two-layer click dispatch (MSV → TreeView with coordinate translation).
//!
//! Apps define section structure via [`SidebarSectionDef`], set row data
//! per frame via [`SidebarSystem::set_rows`], and match on
//! [`SidebarEvent`] for semantic actions.

use crate::{
    Backend, ButtonMask, Key, MouseButton, MsvAxis, MultiSectionView, MultiSectionViewHit,
    NamedKey, Rect, ScrollMode, ScrollbarHit, Section, SectionBody, SectionHeader, SectionSize,
    SelectionMode, StyledText, TreePath, TreeRow, TreeView, TreeViewHit, UiEvent, WidgetId,
};

/// Definition of one sidebar section (structure, not data).
#[derive(Debug, Clone)]
pub struct SidebarSectionDef {
    pub id: String,
    pub title: String,
    pub show_chevron: bool,
    pub size: SectionSize,
}

impl SidebarSectionDef {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            show_chevron: false,
            size: SectionSize::EqualShare,
        }
    }
}

/// What happened after [`SidebarSystem::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
pub enum SidebarEvent {
    /// Header clicked — section activated.
    HeaderActivated { section: usize },
    /// Tree row clicked — section activated + row selected.
    RowSelected { section: usize, path: TreePath },
    /// Scrollbar interaction (drag or page).
    ScrollChanged { section: usize },
    /// State changed (navigation, collapse) — app should redraw.
    StateChanged,
    /// Event consumed (drag update, hover) — app should redraw.
    Consumed,
    /// Event not relevant to the sidebar.
    Ignored,
}

struct ScrollDrag {
    section: usize,
    origin_y: f32,
    origin_offset: usize,
    travel: f32,
    max_offset: usize,
}

pub struct SidebarSystem {
    defs: Vec<SidebarSectionDef>,
    rows: Vec<Vec<TreeRow>>,
    active_section: Option<usize>,
    scroll_offsets: Vec<usize>,
    selected_paths: Vec<Option<TreePath>>,
    collapsed: Vec<bool>,
    scroll_drag: Option<ScrollDrag>,
    has_focus: bool,
    allow_collapse: bool,
}

impl SidebarSystem {
    pub fn new(defs: Vec<SidebarSectionDef>) -> Self {
        let n = defs.len();
        Self {
            defs,
            rows: vec![Vec::new(); n],
            active_section: None,
            scroll_offsets: vec![0; n],
            selected_paths: vec![None; n],
            collapsed: vec![false; n],
            scroll_drag: None,
            has_focus: true,
            allow_collapse: false,
        }
    }

    // ── Per-frame data ────────────────────────────────────────────────

    pub fn set_rows(&mut self, section: usize, rows: Vec<TreeRow>) {
        if section < self.rows.len() {
            self.rows[section] = rows;
        }
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn active_section(&self) -> Option<usize> {
        self.active_section
    }

    pub fn selected_path(&self, section: usize) -> Option<&TreePath> {
        self.selected_paths.get(section).and_then(|p| p.as_ref())
    }

    pub fn scroll_offset(&self, section: usize) -> usize {
        self.scroll_offsets.get(section).copied().unwrap_or(0)
    }

    pub fn is_collapsed(&self, section: usize) -> bool {
        self.collapsed.get(section).copied().unwrap_or(false)
    }

    // ── Programmatic state control ────────────────────────────────────

    pub fn set_active_section(&mut self, section: Option<usize>) {
        self.active_section = section;
    }

    pub fn set_selected_path(&mut self, section: usize, path: Option<TreePath>) {
        if section < self.selected_paths.len() {
            self.selected_paths[section] = path;
        }
    }

    pub fn set_collapsed(&mut self, section: usize, collapsed: bool) {
        if section < self.collapsed.len() {
            self.collapsed[section] = collapsed;
        }
    }

    pub fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    pub fn set_allow_collapse(&mut self, allow: bool) {
        self.allow_collapse = allow;
    }

    // ── Render ────────────────────────────────────────────────────────

    pub fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        let view = self.build_view();
        backend.draw_multi_section_view(rect, &view);
    }

    // ── Handle ────────────────────────────────────────────────────────

    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> SidebarEvent {
        match event {
            // ── Mouse click ───────────────────────────────────────
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.click(backend, rect, position.x, position.y),

            // ── Mouse drag ────────────────────────────────────────
            UiEvent::MouseMoved {
                position,
                buttons:
                    ButtonMask {
                        left: true,
                        middle: _,
                        right: _,
                    },
            } => self.drag_to(position.y),

            // ── Mouse up ──────────────────────────────────────────
            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                self.scroll_drag = None;
                SidebarEvent::Ignored
            }

            // ── Scroll wheel ──────────────────────────────────────
            UiEvent::Scroll { delta, .. } => {
                let rows = if delta.y > 0.0 { -1 } else { 1 };
                self.scroll_active(backend, rect, rows);
                SidebarEvent::Consumed
            }

            // ── Keyboard ──────────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.cycle_active(1);
                SidebarEvent::StateChanged
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::BackTab),
                ..
            } => {
                self.cycle_active(-1);
                SidebarEvent::StateChanged
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                self.scroll_active(backend, rect, -1);
                SidebarEvent::Consumed
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                self.scroll_active(backend, rect, 1);
                SidebarEvent::Consumed
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } => {
                self.select_first_of_active();
                SidebarEvent::StateChanged
            }

            _ => SidebarEvent::Ignored,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn build_view(&self) -> MultiSectionView {
        let sections: Vec<Section> = self
            .defs
            .iter()
            .enumerate()
            .map(|(idx, def)| {
                let is_active = self.active_section == Some(idx);
                let title = if is_active {
                    format!("▶ {}", def.title)
                } else {
                    def.title.clone()
                };
                Section {
                    id: def.id.clone(),
                    header: SectionHeader {
                        title: StyledText::plain(title),
                        show_chevron: def.show_chevron,
                        ..Default::default()
                    },
                    body: SectionBody::Tree(TreeView {
                        id: WidgetId::new(format!("{}-tree", def.id)),
                        rows: self.rows[idx].clone(),
                        selection_mode: SelectionMode::Single,
                        selected_path: self.selected_paths[idx].clone(),
                        scroll_offset: self.scroll_offsets[idx],
                        style: Default::default(),
                        has_focus: is_active && self.has_focus,
                    }),
                    aux: None,
                    size: def.size,
                    collapsed: self.collapsed[idx],
                    min_size: None,
                    max_size: None,
                }
            })
            .collect();
        MultiSectionView {
            id: WidgetId::new("sidebar-system"),
            sections,
            active_section: self.active_section,
            axis: MsvAxis::Vertical,
            allow_resize: false,
            allow_collapse: self.allow_collapse,
            scroll_mode: ScrollMode::PerSection,
            has_focus: self.has_focus,
            panel_scroll: 0.0,
        }
    }

    fn click(
        &mut self,
        backend: &mut dyn Backend,
        rect: Rect,
        x: f32,
        y: f32,
    ) -> SidebarEvent {
        let view = self.build_view();
        let layout = backend.msv_layout(rect, &view);
        match layout.hit_test(x, y) {
            MultiSectionViewHit::Header { section, .. } => {
                self.active_section = Some(section);
                self.selected_paths[section] = None;
                SidebarEvent::HeaderActivated { section }
            }
            MultiSectionViewHit::Body { section } => {
                self.active_section = Some(section);
                let body_b = layout.sections[section].body_bounds;
                let tree = match &view.sections[section].body {
                    SectionBody::Tree(t) => t.clone(),
                    _ => return SidebarEvent::Consumed,
                };
                let inner = backend.tree_layout(body_b, &tree);
                match inner.hit_test(x - body_b.x, y - body_b.y) {
                    TreeViewHit::Row(idx) => {
                        let path = tree.rows[idx].path.clone();
                        self.selected_paths[section] = Some(path.clone());
                        SidebarEvent::RowSelected { section, path }
                    }
                    TreeViewHit::Empty => SidebarEvent::HeaderActivated { section },
                }
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::Thumb,
            } => {
                let sb = layout.sections[section]
                    .scrollbar_bounds
                    .expect("scrollbar hit implies bounds present");
                let thumb_h = layout.sections[section]
                    .thumb_bounds
                    .map(|t| t.height)
                    .unwrap_or(sb.height);
                let body_b = layout.sections[section].body_bounds;
                let viewport_rows = self.viewport_rows(backend, body_b, section, &view);
                let max_offset = self.rows[section].len().saturating_sub(viewport_rows);
                let travel = (sb.height - thumb_h).max(0.0);
                self.scroll_drag = Some(ScrollDrag {
                    section,
                    origin_y: y,
                    origin_offset: self.scroll_offsets[section],
                    travel,
                    max_offset,
                });
                SidebarEvent::ScrollChanged { section }
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackBefore,
            } => {
                let body_b = layout.sections[section].body_bounds;
                let viewport_rows = self.viewport_rows(backend, body_b, section, &view);
                self.page_scroll(section, -(viewport_rows as isize), viewport_rows);
                SidebarEvent::ScrollChanged { section }
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackAfter,
            } => {
                let body_b = layout.sections[section].body_bounds;
                let viewport_rows = self.viewport_rows(backend, body_b, section, &view);
                self.page_scroll(section, viewport_rows as isize, viewport_rows);
                SidebarEvent::ScrollChanged { section }
            }
            _ => SidebarEvent::Ignored,
        }
    }

    fn viewport_rows(
        &self,
        backend: &dyn Backend,
        body_b: Rect,
        section: usize,
        view: &MultiSectionView,
    ) -> usize {
        let SectionBody::Tree(t) = &view.sections[section].body else {
            return 0;
        };
        let mut shadow = t.clone();
        shadow.scroll_offset = 0;
        let inner = backend.tree_layout(body_b, &shadow);
        inner.visible_rows.len()
    }

    fn drag_to(&mut self, y: f32) -> SidebarEvent {
        let Some(drag) = &self.scroll_drag else {
            return SidebarEvent::Ignored;
        };
        if drag.travel <= 0.0 || drag.max_offset == 0 {
            return SidebarEvent::Ignored;
        }
        let dy = y - drag.origin_y;
        let drow = dy / drag.travel * drag.max_offset as f32;
        let new = (drag.origin_offset as f32 + drow).round() as i32;
        let new = new.max(0) as usize;
        let new = new.min(drag.max_offset);
        let section = drag.section;
        if new == self.scroll_offsets[section] {
            return SidebarEvent::Ignored;
        }
        self.scroll_offsets[section] = new;
        SidebarEvent::Consumed
    }

    fn page_scroll(&mut self, section: usize, delta: isize, viewport_rows: usize) {
        let row_count = self.rows[section].len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.scroll_offsets[section] as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.scroll_offsets[section] = new;
    }

    fn cycle_active(&mut self, delta: isize) {
        let n = self.defs.len() as isize;
        if n == 0 {
            return;
        }
        let next = match self.active_section {
            Some(i) => ((i as isize + delta).rem_euclid(n)) as usize,
            None => {
                if delta >= 0 {
                    0
                } else {
                    (n - 1) as usize
                }
            }
        };
        self.active_section = Some(next);
    }

    fn scroll_active(&mut self, backend: &mut dyn Backend, rect: Rect, delta: isize) {
        let Some(idx) = self.active_section else {
            return;
        };
        let view = self.build_view();
        let layout = backend.msv_layout(rect, &view);
        let body_b = layout.sections[idx].body_bounds;
        let viewport_rows = self.viewport_rows(backend, body_b, idx, &view);
        let row_count = self.rows[idx].len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.scroll_offsets[idx] as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.scroll_offsets[idx] = new;
    }

    fn select_first_of_active(&mut self) {
        let Some(idx) = self.active_section else {
            return;
        };
        if let Some(first) = self.rows[idx].first() {
            self.selected_paths[idx] = Some(first.path.clone());
            self.scroll_offsets[idx] = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Decoration;

    fn sample_defs() -> Vec<SidebarSectionDef> {
        vec![
            SidebarSectionDef::new("variables", "VARIABLES"),
            SidebarSectionDef::new("watch", "WATCH"),
            SidebarSectionDef::new("breakpoints", "BREAKPOINTS"),
        ]
    }

    fn fake_rows(prefix: &str, n: usize) -> Vec<TreeRow> {
        (0..n)
            .map(|i| TreeRow {
                path: vec![i as u16],
                indent: 0,
                icon: None,
                text: StyledText::plain(format!("{prefix}{i}")),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
            })
            .collect()
    }

    #[test]
    fn new_starts_with_no_active_section() {
        let ss = SidebarSystem::new(sample_defs());
        assert_eq!(ss.active_section(), None);
        assert_eq!(ss.scroll_offset(0), 0);
        assert_eq!(ss.selected_path(0), None);
        assert!(!ss.is_collapsed(0));
    }

    #[test]
    fn set_rows_updates_section_data() {
        let mut ss = SidebarSystem::new(sample_defs());
        assert!(ss.rows[0].is_empty());
        ss.set_rows(0, fake_rows("v", 5));
        assert_eq!(ss.rows[0].len(), 5);
    }

    #[test]
    fn cycle_active_wraps() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.cycle_active(1);
        assert_eq!(ss.active_section(), Some(0));
        ss.cycle_active(1);
        assert_eq!(ss.active_section(), Some(1));
        ss.cycle_active(1);
        assert_eq!(ss.active_section(), Some(2));
        ss.cycle_active(1);
        assert_eq!(ss.active_section(), Some(0));
    }

    #[test]
    fn cycle_active_backward_wraps() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.cycle_active(-1);
        assert_eq!(ss.active_section(), Some(2));
        ss.cycle_active(-1);
        assert_eq!(ss.active_section(), Some(1));
    }

    #[test]
    fn set_active_and_selected_path() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 3));
        ss.set_active_section(Some(0));
        ss.set_selected_path(0, Some(vec![1]));
        assert_eq!(ss.active_section(), Some(0));
        assert_eq!(ss.selected_path(0), Some(&vec![1]));
    }

    #[test]
    fn select_first_of_active_sets_path_and_resets_scroll() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(1, fake_rows("w", 5));
        ss.scroll_offsets[1] = 3;
        ss.active_section = Some(1);
        ss.select_first_of_active();
        assert_eq!(ss.selected_path(1), Some(&vec![0]));
        assert_eq!(ss.scroll_offset(1), 0);
    }

    #[test]
    fn page_scroll_clamps_to_bounds() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 20));
        ss.page_scroll(0, 100, 5);
        assert_eq!(ss.scroll_offset(0), 15); // 20 - 5
        ss.page_scroll(0, -100, 5);
        assert_eq!(ss.scroll_offset(0), 0);
    }

    #[test]
    fn build_view_produces_correct_sections() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 3));
        ss.active_section = Some(0);
        let view = ss.build_view();
        assert_eq!(view.sections.len(), 3);
        assert!(view.sections[0].header.title.spans[0]
            .text
            .starts_with("▶"));
        assert_eq!(view.active_section, Some(0));
    }
}
