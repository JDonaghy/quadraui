//! `SidebarSystem` — a composed controller for MSV + TreeView sidebar
//! panels.
//!
//! Owns the full interaction state machine: per-section scroll/selection,
//! active section cycling, scrollbar drag, keyboard navigation, and
//! two-layer click dispatch (MSV → TreeView with coordinate translation).
//!
//! Two navigation modes (set via [`SidebarSystem::set_navigation_mode`]):
//! - [`NavigationMode::Scroll`] (default): Up/Down scroll the viewport.
//! - [`NavigationMode::Selection`]: Up/Down/j/k move `selected_path` to
//!   the next/previous row with scroll-to-follow. Home/End/PageUp/PageDown
//!   jump by page or extremes. Enter emits [`SidebarEvent::RowActivated`].
//!
//! Apps define section structure via [`SidebarSectionDef`], set row data
//! per frame via [`SidebarSystem::set_rows`], and match on
//! [`SidebarEvent`] for semantic actions.

use super::tree_controller::TreeController;
use crate::{
    Backend, ButtonMask, Key, MouseButton, MsvAxis, MultiSectionView, MultiSectionViewHit,
    NamedKey, Point, Rect, ScrollMode, ScrollbarHit, Section, SectionBody, SectionHeader,
    SectionSize, SelectionMode, StyledText, TreePath, TreeRow, TreeView, TreeViewHit, UiEvent,
    WidgetId,
};

/// How Up/Down keys behave in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavigationMode {
    /// Up/Down scroll the viewport (legacy behaviour).
    #[default]
    Scroll,
    /// Up/Down move `selected_path` to the next/previous row; the
    /// viewport scrolls to keep the selection visible.
    Selection,
}

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
    /// Enter pressed on the currently selected row (Selection mode).
    /// Distinct from `RowSelected` (click-driven) — lets apps
    /// distinguish keyboard activation from mouse selection.
    RowActivated { section: usize, path: TreePath },
    /// Right-click on a body row. `position` is the click position
    /// in the backend's native coordinates (for context menu placement).
    ContextMenuRequested {
        section: usize,
        path: TreePath,
        position: Point,
    },
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
    sections: Vec<TreeController>,
    active_section: Option<usize>,
    collapsed: Vec<bool>,
    scroll_drag: Option<ScrollDrag>,
    has_focus: bool,
    allow_collapse: bool,
    navigation_mode: NavigationMode,
    cached_viewport_rows: Option<(usize, usize)>,
}

impl SidebarSystem {
    pub fn new(defs: Vec<SidebarSectionDef>) -> Self {
        let n = defs.len();
        let sections = defs
            .iter()
            .map(|def| TreeController::new(format!("{}-tree", def.id)))
            .collect();
        Self {
            defs,
            sections,
            active_section: None,
            collapsed: vec![false; n],
            scroll_drag: None,
            has_focus: true,
            allow_collapse: false,
            navigation_mode: NavigationMode::default(),
            cached_viewport_rows: None,
        }
    }

    // ── Per-frame data ────────────────────────────────────────────────

    pub fn set_rows(&mut self, section: usize, rows: Vec<TreeRow>) {
        if let Some(tc) = self.sections.get_mut(section) {
            tc.set_rows(rows);
        }
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn active_section(&self) -> Option<usize> {
        self.active_section
    }

    pub fn selected_path(&self, section: usize) -> Option<&TreePath> {
        self.sections.get(section).and_then(|tc| tc.selected_path())
    }

    pub fn scroll_offset(&self, section: usize) -> usize {
        self.sections
            .get(section)
            .map(|tc| tc.scroll_offset())
            .unwrap_or(0)
    }

    pub fn is_collapsed(&self, section: usize) -> bool {
        self.collapsed.get(section).copied().unwrap_or(false)
    }

    // ── Programmatic state control ────────────────────────────────────

    pub fn set_active_section(&mut self, section: Option<usize>) {
        self.active_section = section;
    }

    pub fn set_selected_path(&mut self, section: usize, path: Option<TreePath>) {
        if let Some(tc) = self.sections.get_mut(section) {
            tc.set_selected_path(path);
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

    pub fn navigation_mode(&self) -> NavigationMode {
        self.navigation_mode
    }

    pub fn set_navigation_mode(&mut self, mode: NavigationMode) {
        self.navigation_mode = mode;
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
        self.cached_viewport_rows = None;
        match event {
            // ── Mouse click ───────────────────────────────────────
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.click(backend, rect, position.x, position.y),

            // ── Right-click ──────────────────────────────────────
            UiEvent::MouseDown {
                button: MouseButton::Right,
                position,
                ..
            } => self.right_click(backend, rect, *position),

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
            UiEvent::KeyPressed { key, .. } => self.handle_key(key, backend, rect),

            _ => SidebarEvent::Ignored,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn handle_key(&mut self, key: &Key, backend: &mut dyn Backend, rect: Rect) -> SidebarEvent {
        match self.navigation_mode {
            NavigationMode::Scroll => self.handle_key_scroll(key, backend, rect),
            NavigationMode::Selection => self.handle_key_selection(key, backend, rect),
        }
    }

    fn handle_key_scroll(
        &mut self,
        key: &Key,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> SidebarEvent {
        match key {
            Key::Named(NamedKey::Up) => {
                self.scroll_active(backend, rect, -1);
                SidebarEvent::Consumed
            }
            Key::Named(NamedKey::Down) => {
                self.scroll_active(backend, rect, 1);
                SidebarEvent::Consumed
            }
            Key::Named(NamedKey::Enter) => {
                self.select_first_of_active();
                SidebarEvent::StateChanged
            }
            _ => SidebarEvent::Ignored,
        }
    }

    fn handle_key_selection(
        &mut self,
        key: &Key,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> SidebarEvent {
        match key {
            Key::Named(NamedKey::Up) | Key::Char('k') => self.move_selection(-1, backend, rect),
            Key::Named(NamedKey::Down) | Key::Char('j') => self.move_selection(1, backend, rect),
            Key::Named(NamedKey::Home) => self.jump_selection_to_edge(true, backend, rect),
            Key::Named(NamedKey::End) => self.jump_selection_to_edge(false, backend, rect),
            Key::Named(NamedKey::PageUp) => {
                let vr = self.active_viewport_rows(backend, rect);
                self.move_selection_by(-((vr.max(1) - 1).max(1) as isize), vr)
            }
            Key::Named(NamedKey::PageDown) => {
                let vr = self.active_viewport_rows(backend, rect);
                self.move_selection_by((vr.max(1) - 1).max(1) as isize, vr)
            }
            Key::Named(NamedKey::Enter) => self.activate_selection(),
            _ => SidebarEvent::Ignored,
        }
    }

    fn move_selection(
        &mut self,
        delta: isize,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> SidebarEvent {
        let vr = self.active_viewport_rows(backend, rect);
        self.move_selection_by(delta, vr)
    }

    fn move_selection_by(&mut self, delta: isize, viewport_rows: usize) -> SidebarEvent {
        let Some(idx) = self.active_section else {
            return SidebarEvent::Ignored;
        };
        if self.collapsed[idx] {
            return SidebarEvent::Ignored;
        }
        use super::tree_controller::TreeControllerEvent;
        match self.sections[idx].move_selection_by(delta, viewport_rows) {
            TreeControllerEvent::RowSelected { path } => {
                SidebarEvent::RowSelected { section: idx, path }
            }
            TreeControllerEvent::Consumed => SidebarEvent::Consumed,
            _ => SidebarEvent::Ignored,
        }
    }

    fn jump_selection_to_edge(
        &mut self,
        to_start: bool,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> SidebarEvent {
        let vr = self.active_viewport_rows(backend, rect);
        self.jump_selection_to_edge_by(to_start, vr)
    }

    fn jump_selection_to_edge_by(&mut self, to_start: bool, viewport_rows: usize) -> SidebarEvent {
        let Some(idx) = self.active_section else {
            return SidebarEvent::Ignored;
        };
        use super::tree_controller::TreeControllerEvent;
        match self.sections[idx].jump_to_edge(to_start, viewport_rows) {
            TreeControllerEvent::RowSelected { path } => {
                SidebarEvent::RowSelected { section: idx, path }
            }
            _ => SidebarEvent::Ignored,
        }
    }

    fn activate_selection(&self) -> SidebarEvent {
        let Some(idx) = self.active_section else {
            return SidebarEvent::Ignored;
        };
        use super::tree_controller::TreeControllerEvent;
        match self.sections[idx].activate_selection() {
            TreeControllerEvent::RowActivated { path } => {
                SidebarEvent::RowActivated { section: idx, path }
            }
            _ => SidebarEvent::Ignored,
        }
    }

    fn active_viewport_rows(&mut self, backend: &mut dyn Backend, rect: Rect) -> usize {
        let Some(idx) = self.active_section else {
            return 0;
        };
        if let Some((cached_section, cached_vr)) = self.cached_viewport_rows {
            if cached_section == idx {
                return cached_vr;
            }
        }
        let vr = self.section_viewport_rows(idx, backend, rect);
        self.cached_viewport_rows = Some((idx, vr));
        vr
    }

    fn section_viewport_rows(
        &self,
        section: usize,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> usize {
        let view = self.build_view();
        let layout = backend.msv_layout(rect, &view);
        let body_b = layout.sections[section].body_bounds;
        self.viewport_rows(backend, body_b, section, &view)
    }

    fn build_view(&self) -> MultiSectionView {
        let sections: Vec<Section> = self
            .defs
            .iter()
            .enumerate()
            .map(|(idx, def)| {
                let is_active = self.active_section == Some(idx);
                let tc = &self.sections[idx];
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
                        rows: tc.rows().to_vec(),
                        selection_mode: SelectionMode::Single,
                        selected_path: tc.selected_path().cloned(),
                        scroll_offset: tc.scroll_offset(),
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

    fn click(&mut self, backend: &mut dyn Backend, rect: Rect, x: f32, y: f32) -> SidebarEvent {
        let view = self.build_view();
        let layout = backend.msv_layout(rect, &view);
        match layout.hit_test(x, y) {
            MultiSectionViewHit::Header { section, .. } => {
                self.active_section = Some(section);
                self.sections[section].set_selected_path(None);
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
                        self.sections[section].set_selected_path(Some(path.clone()));
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
                let row_count = self.sections[section].rows().len();
                let max_offset = row_count.saturating_sub(viewport_rows);
                let travel = (sb.height - thumb_h).max(0.0);
                self.scroll_drag = Some(ScrollDrag {
                    section,
                    origin_y: y,
                    origin_offset: self.sections[section].scroll_offset(),
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
                self.sections[section].page_scroll(-(viewport_rows as isize), viewport_rows);
                SidebarEvent::ScrollChanged { section }
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackAfter,
            } => {
                let body_b = layout.sections[section].body_bounds;
                let viewport_rows = self.viewport_rows(backend, body_b, section, &view);
                self.sections[section].page_scroll(viewport_rows as isize, viewport_rows);
                SidebarEvent::ScrollChanged { section }
            }
            _ => SidebarEvent::Ignored,
        }
    }

    fn right_click(
        &mut self,
        backend: &mut dyn Backend,
        rect: Rect,
        position: Point,
    ) -> SidebarEvent {
        let view = self.build_view();
        let layout = backend.msv_layout(rect, &view);
        match layout.hit_test(position.x, position.y) {
            MultiSectionViewHit::Body { section } => {
                let body_b = layout.sections[section].body_bounds;
                let tree = match &view.sections[section].body {
                    SectionBody::Tree(t) => t.clone(),
                    _ => return SidebarEvent::Ignored,
                };
                let inner = backend.tree_layout(body_b, &tree);
                match inner.hit_test(position.x - body_b.x, position.y - body_b.y) {
                    TreeViewHit::Row(idx) => {
                        let path = tree.rows[idx].path.clone();
                        self.active_section = Some(section);
                        self.sections[section].set_selected_path(Some(path.clone()));
                        SidebarEvent::ContextMenuRequested {
                            section,
                            path,
                            position,
                        }
                    }
                    TreeViewHit::Empty => SidebarEvent::Ignored,
                }
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
        if new == self.sections[section].scroll_offset() {
            return SidebarEvent::Ignored;
        }
        self.sections[section].set_scroll_offset(new);
        SidebarEvent::Consumed
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
        let row_count = self.sections[idx].rows().len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.sections[idx].scroll_offset() as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.sections[idx].set_scroll_offset(new);
    }

    fn select_first_of_active(&mut self) {
        let Some(idx) = self.active_section else {
            return;
        };
        if let Some(first) = self.sections[idx].rows().first() {
            let path = first.path.clone();
            self.sections[idx].set_selected_path(Some(path));
            self.sections[idx].set_scroll_offset(0);
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
        assert!(ss.sections[0].rows().is_empty());
        ss.set_rows(0, fake_rows("v", 5));
        assert_eq!(ss.sections[0].rows().len(), 5);
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
        ss.sections[1].set_scroll_offset(3);
        ss.active_section = Some(1);
        ss.select_first_of_active();
        assert_eq!(ss.selected_path(1), Some(&vec![0]));
        assert_eq!(ss.scroll_offset(1), 0);
    }

    #[test]
    fn page_scroll_clamps_to_bounds() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 20));
        ss.sections[0].page_scroll(100, 5);
        assert_eq!(ss.scroll_offset(0), 15); // 20 - 5
        ss.sections[0].page_scroll(-100, 5);
        assert_eq!(ss.scroll_offset(0), 0);
    }

    #[test]
    fn build_view_produces_correct_sections() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 3));
        ss.active_section = Some(0);
        let view = ss.build_view();
        assert_eq!(view.sections.len(), 3);
        assert!(view.sections[0].header.title.spans[0].text.starts_with("▶"));
        assert_eq!(view.active_section, Some(0));
    }

    // ── Selection-mode tests ────────────────────────────────────────

    fn selection_sidebar() -> SidebarSystem {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_navigation_mode(NavigationMode::Selection);
        ss.set_rows(0, fake_rows("v", 5));
        ss.set_rows(1, fake_rows("w", 3));
        ss.set_rows(2, fake_rows("b", 4));
        ss.active_section = Some(0);
        ss
    }

    #[test]
    fn selection_down_selects_first_row_when_none_selected() {
        let mut ss = selection_sidebar();
        let ev = ss.move_selection_by(1, 10);
        assert_eq!(ss.selected_path(0), Some(&vec![0]));
        assert!(matches!(ev, SidebarEvent::RowSelected { section: 0, .. }));
    }

    #[test]
    fn selection_down_advances_to_next_row() {
        let mut ss = selection_sidebar();
        ss.set_selected_path(0, Some(vec![0]));
        let ev = ss.move_selection_by(1, 10);
        assert_eq!(ss.selected_path(0), Some(&vec![1]));
        assert!(matches!(ev, SidebarEvent::RowSelected { section: 0, .. }));
    }

    #[test]
    fn selection_up_moves_to_previous_row() {
        let mut ss = selection_sidebar();
        ss.set_selected_path(0, Some(vec![2]));
        ss.move_selection_by(-1, 10);
        assert_eq!(ss.selected_path(0), Some(&vec![1]));
    }

    #[test]
    fn selection_clamps_at_first_row() {
        let mut ss = selection_sidebar();
        ss.set_selected_path(0, Some(vec![0]));
        let ev = ss.move_selection_by(-1, 10);
        assert_eq!(ss.selected_path(0), Some(&vec![0]));
        assert_eq!(ev, SidebarEvent::Consumed);
    }

    #[test]
    fn selection_clamps_at_last_row() {
        let mut ss = selection_sidebar();
        ss.set_selected_path(0, Some(vec![4]));
        let ev = ss.move_selection_by(1, 10);
        assert_eq!(ss.selected_path(0), Some(&vec![4]));
        assert_eq!(ev, SidebarEvent::Consumed);
    }

    #[test]
    fn selection_up_with_no_selection_selects_last_row() {
        let mut ss = selection_sidebar();
        ss.move_selection_by(-1, 10);
        assert_eq!(ss.selected_path(0), Some(&vec![4]));
    }

    #[test]
    fn selection_home_jumps_to_first_row() {
        let mut ss = selection_sidebar();
        ss.set_selected_path(0, Some(vec![3]));
        ss.jump_selection_to_edge_by(true, 10);
        assert_eq!(ss.selected_path(0), Some(&vec![0]));
    }

    #[test]
    fn selection_end_jumps_to_last_row() {
        let mut ss = selection_sidebar();
        ss.set_selected_path(0, Some(vec![0]));
        ss.jump_selection_to_edge_by(false, 10);
        assert_eq!(ss.selected_path(0), Some(&vec![4]));
    }

    #[test]
    fn selection_page_down_jumps_by_viewport() {
        let mut ss = selection_sidebar();
        ss.set_selected_path(0, Some(vec![0]));
        let jump = (3_usize.max(1) - 1).max(1) as isize; // viewport=3 → jump=2
        ss.move_selection_by(jump, 3);
        assert_eq!(ss.selected_path(0), Some(&vec![2]));
    }

    #[test]
    fn selection_enter_emits_row_activated() {
        let mut ss = selection_sidebar();
        ss.set_selected_path(0, Some(vec![2]));
        let ev = ss.activate_selection();
        assert_eq!(
            ev,
            SidebarEvent::RowActivated {
                section: 0,
                path: vec![2]
            }
        );
    }

    #[test]
    fn selection_enter_with_no_selection_returns_ignored() {
        let ss = selection_sidebar();
        let ev = ss.activate_selection();
        assert_eq!(ev, SidebarEvent::Ignored);
    }

    #[test]
    fn selection_scroll_follows_when_row_below_viewport() {
        let mut ss = selection_sidebar();
        // 5 rows, viewport=3, start at row 0 with offset 0.
        ss.set_selected_path(0, Some(vec![0]));
        // Move down 4 times: 0→1→2→3→4
        for _ in 0..4 {
            ss.move_selection_by(1, 3);
        }
        assert_eq!(ss.selected_path(0), Some(&vec![4]));
        // scroll_offset should have followed: row 4 visible in 3-row viewport
        // means offset must be at least 2.
        assert!(ss.scroll_offset(0) >= 2, "offset={}", ss.scroll_offset(0));
        assert!(ss.scroll_offset(0) + 3 > 4);
    }

    #[test]
    fn selection_scroll_follows_when_row_above_viewport() {
        let mut ss = selection_sidebar();
        ss.set_selected_path(0, Some(vec![4]));
        ss.sections[0].set_scroll_offset(2);
        // Move up to row 0.
        for _ in 0..4 {
            ss.move_selection_by(-1, 3);
        }
        assert_eq!(ss.selected_path(0), Some(&vec![0]));
        assert_eq!(ss.scroll_offset(0), 0);
    }

    #[test]
    fn selection_collapsed_section_returns_ignored() {
        let mut ss = selection_sidebar();
        ss.set_collapsed(0, true);
        let ev = ss.move_selection_by(1, 10);
        assert_eq!(ev, SidebarEvent::Ignored);
    }

    #[test]
    fn selection_no_active_section_returns_ignored() {
        let mut ss = selection_sidebar();
        ss.active_section = None;
        let ev = ss.move_selection_by(1, 10);
        assert_eq!(ev, SidebarEvent::Ignored);
    }

    #[test]
    fn navigation_mode_defaults_to_scroll() {
        let ss = SidebarSystem::new(sample_defs());
        assert_eq!(ss.navigation_mode(), NavigationMode::Scroll);
    }
}
