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
//!
//! Scroll-wheel events follow the [`ScrollDelta`](crate::ScrollDelta)
//! sign convention: positive `delta.y` = scroll content up (decrease
//! offset). Backends normalise their native direction before emitting.

use super::focus_group::FocusGroup;
use super::tree_controller::TreeController;
use super::tree_controller::TreeControllerEvent;
use crate::primitives::multi_section_view::{
    LayoutMetrics, MultiSectionViewLayout, SectionMeasure,
};
use crate::primitives::tree::TreeRowMeasure;
use crate::{
    Backend, ButtonMask, Key, Modifiers, MouseButton, MsvAxis, MultiSectionView,
    MultiSectionViewHit, NamedKey, Point, Rect, ScrollMode, ScrollbarHit, Section, SectionBody,
    SectionHeader, SectionSize, SelectionMode, StyledText, TreePath, TreeRow, TreeRowEditState,
    TreeView, TreeViewHit, UiEvent, WidgetId,
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
    /// The user confirmed an inline edit (pressed Enter).
    EditConfirmed {
        section: usize,
        path: TreePath,
        new_text: String,
    },
    /// The user cancelled an inline edit (pressed Escape).
    EditCancelled { section: usize, path: TreePath },
    /// The text buffer changed during inline editing.
    EditChanged {
        section: usize,
        path: TreePath,
        text: String,
    },
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

struct PanelScrollDrag {
    origin_y: f32,
    origin_scroll: f32,
    travel: f32,
    max_scroll: f32,
}

struct BackendInfo {
    line_height: f32,
    metrics: LayoutMetrics,
}

pub struct SidebarSystem {
    defs: Vec<SidebarSectionDef>,
    sections: Vec<TreeController>,
    focus: FocusGroup,
    collapsed: Vec<bool>,
    visible: Vec<bool>,
    badges: Vec<Option<StyledText>>,
    scroll_drag: Option<ScrollDrag>,
    panel_drag: Option<PanelScrollDrag>,
    has_focus: bool,
    allow_collapse: bool,
    navigation_mode: NavigationMode,
    cached_viewport_rows: Option<(usize, usize)>,
    scroll_mode: ScrollMode,
    panel_scroll: f32,
    backend_info: Option<BackendInfo>,
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
            focus: FocusGroup::new(n),
            collapsed: vec![false; n],
            visible: vec![true; n],
            badges: vec![None; n],
            scroll_drag: None,
            panel_drag: None,
            has_focus: true,
            allow_collapse: false,
            navigation_mode: NavigationMode::default(),
            cached_viewport_rows: None,
            scroll_mode: ScrollMode::PerSection,
            panel_scroll: 0.0,
            backend_info: None,
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
        self.focus.active()
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

    pub fn is_section_visible(&self, section: usize) -> bool {
        self.visible.get(section).copied().unwrap_or(true)
    }

    pub fn section_badge(&self, section: usize) -> Option<&StyledText> {
        self.badges.get(section).and_then(|b| b.as_ref())
    }

    // ── Programmatic state control ────────────────────────────────────

    pub fn set_active_section(&mut self, section: Option<usize>) {
        self.focus.set_active(section);
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

    pub fn set_section_visible(&mut self, section: usize, visible: bool) {
        if section < self.visible.len() {
            self.visible[section] = visible;
        }
    }

    pub fn set_section_badge(&mut self, section: usize, badge: Option<StyledText>) {
        if section < self.badges.len() {
            self.badges[section] = badge;
        }
    }

    pub fn has_focus(&self) -> bool {
        self.has_focus
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

    pub fn scroll_mode(&self) -> ScrollMode {
        self.scroll_mode
    }

    pub fn set_scroll_mode(&mut self, mode: ScrollMode) {
        self.scroll_mode = mode;
    }

    pub fn panel_scroll(&self) -> f32 {
        self.panel_scroll
    }

    pub fn set_panel_scroll(&mut self, offset: f32) {
        self.panel_scroll = offset.max(0.0);
    }

    /// Cache backend-specific layout info so [`Self::handle_cached`] can
    /// compute layouts without a Backend reference. Call once at init, or
    /// again if `line_height` changes (font/DPI change).
    pub fn set_backend_info(&mut self, line_height: f32, metrics: LayoutMetrics) {
        self.backend_info = Some(BackendInfo {
            line_height,
            metrics,
        });
    }

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
        if let Some(tc) = self.sections.get_mut(section) {
            tc.start_editing(path, initial_text, cursor, selection_anchor, placeholder);
        }
    }

    pub fn cancel_editing(&mut self, section: usize) {
        if let Some(tc) = self.sections.get_mut(section) {
            tc.cancel_editing();
        }
    }

    pub fn is_editing(&self) -> bool {
        self.sections.iter().any(|tc| tc.is_editing())
    }

    // ── Render ────────────────────────────────────────────────────────

    pub fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        let (view, _) = self.build_view();
        backend.draw_multi_section_view(rect, &view);
    }

    // ── Handle ────────────────────────────────────────────────────────

    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> SidebarEvent {
        let lh = backend.line_height();
        let metrics = backend.msv_metrics();
        self.handle_inner(event, rect, lh, &metrics)
    }

    /// Backend-free event handler. Requires [`Self::set_backend_info`]
    /// called first. Returns [`SidebarEvent::Ignored`] if backend info
    /// is not set.
    pub fn handle_cached(&mut self, event: &UiEvent, rect: Rect) -> SidebarEvent {
        let Some(ref info) = self.backend_info else {
            return SidebarEvent::Ignored;
        };
        let lh = info.line_height;
        let metrics = info.metrics;
        self.handle_inner(event, rect, lh, &metrics)
    }

    fn handle_inner(
        &mut self,
        event: &UiEvent,
        rect: Rect,
        lh: f32,
        metrics: &LayoutMetrics,
    ) -> SidebarEvent {
        self.cached_viewport_rows = None;

        // Route text input events to the editing section's TreeController.
        if self.is_editing() {
            match event {
                UiEvent::CharTyped(ch) => return self.forward_edit_char(*ch),
                UiEvent::ClipboardPaste(text) => return self.forward_edit_paste(text),
                _ => {}
            }
        }

        match event {
            // ── Mouse click ───────────────────────────────────────
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.click(rect, position.x, position.y, lh, metrics),

            // ── Right-click ──────────────────────────────────────
            UiEvent::MouseDown {
                button: MouseButton::Right,
                position,
                ..
            } => self.right_click(rect, *position, lh, metrics),

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
                self.panel_drag = None;
                SidebarEvent::Ignored
            }

            // ── Scroll wheel ──────────────────────────────────────
            UiEvent::Scroll { delta, .. } => {
                if self.scroll_mode == ScrollMode::WholePanel {
                    let dy = if delta.y > 0.0 { -lh } else { lh };
                    self.scroll_panel(rect, dy, lh, metrics);
                } else {
                    let rows = if delta.y > 0.0 { -1 } else { 1 };
                    self.scroll_active(rect, rows, lh, metrics);
                }
                SidebarEvent::Consumed
            }

            // ── Keyboard ──────────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.cycle_active(1);
                if self.scroll_mode == ScrollMode::WholePanel {
                    self.scroll_to_active_section(rect, lh, metrics);
                }
                SidebarEvent::StateChanged
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::BackTab),
                ..
            } => {
                self.cycle_active(-1);
                if self.scroll_mode == ScrollMode::WholePanel {
                    self.scroll_to_active_section(rect, lh, metrics);
                }
                SidebarEvent::StateChanged
            }
            UiEvent::KeyPressed { key, modifiers, .. } => {
                if self.is_editing() {
                    return self.forward_edit_key(key, modifiers);
                }
                self.handle_key(key, rect, lh, metrics)
            }

            _ => SidebarEvent::Ignored,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn compute_layout(
        &self,
        rect: Rect,
        metrics: &LayoutMetrics,
        lh: f32,
    ) -> (MultiSectionViewLayout, Vec<usize>) {
        let (view, map) = self.build_view();
        let layout = view.layout(rect, *metrics, |i| body_measure(&view.sections[i], lh));
        (layout, map)
    }

    fn compute_tree_layout(
        &self,
        body_b: Rect,
        tree: &TreeView,
        lh: f32,
    ) -> crate::primitives::tree::TreeViewLayout {
        let header_h = (lh * 1.2).round();
        let item_h = (lh * 1.4).round();
        tree.layout(body_b.width, body_b.height, |i| {
            let is_header = matches!(tree.rows[i].decoration, crate::types::Decoration::Header);
            TreeRowMeasure::new(if is_header { header_h } else { item_h })
        })
    }

    fn handle_key(
        &mut self,
        key: &Key,
        rect: Rect,
        lh: f32,
        metrics: &LayoutMetrics,
    ) -> SidebarEvent {
        match self.navigation_mode {
            NavigationMode::Scroll => self.handle_key_scroll(key, rect, lh, metrics),
            NavigationMode::Selection => self.handle_key_selection(key, rect, lh, metrics),
        }
    }

    fn handle_key_scroll(
        &mut self,
        key: &Key,
        rect: Rect,
        lh: f32,
        metrics: &LayoutMetrics,
    ) -> SidebarEvent {
        match key {
            Key::Named(NamedKey::Up) => {
                if self.scroll_mode == ScrollMode::WholePanel {
                    self.scroll_panel(rect, -lh, lh, metrics);
                } else {
                    self.scroll_active(rect, -1, lh, metrics);
                }
                SidebarEvent::Consumed
            }
            Key::Named(NamedKey::Down) => {
                if self.scroll_mode == ScrollMode::WholePanel {
                    self.scroll_panel(rect, lh, lh, metrics);
                } else {
                    self.scroll_active(rect, 1, lh, metrics);
                }
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
        rect: Rect,
        lh: f32,
        metrics: &LayoutMetrics,
    ) -> SidebarEvent {
        let vim = self
            .focus
            .active()
            .and_then(|i| self.sections.get(i))
            .is_none_or(|tc| tc.vim_keys());
        let vim_up = vim && matches!(key, Key::Char('k'));
        let vim_down = vim && matches!(key, Key::Char('j'));
        match key {
            Key::Named(NamedKey::Up) => self.move_selection(-1, rect, lh, metrics),
            _ if vim_up => self.move_selection(-1, rect, lh, metrics),
            Key::Named(NamedKey::Down) => self.move_selection(1, rect, lh, metrics),
            _ if vim_down => self.move_selection(1, rect, lh, metrics),
            Key::Named(NamedKey::Home) => self.jump_selection_to_edge(true, rect, lh, metrics),
            Key::Named(NamedKey::End) => self.jump_selection_to_edge(false, rect, lh, metrics),
            Key::Named(NamedKey::PageUp) => {
                let vr = self.active_viewport_rows(rect, lh, metrics);
                self.move_selection_by(-((vr.max(1) - 1).max(1) as isize), vr)
            }
            Key::Named(NamedKey::PageDown) => {
                let vr = self.active_viewport_rows(rect, lh, metrics);
                self.move_selection_by((vr.max(1) - 1).max(1) as isize, vr)
            }
            Key::Named(NamedKey::Enter) => self.activate_selection(),
            _ => SidebarEvent::Ignored,
        }
    }

    fn move_selection(
        &mut self,
        delta: isize,
        rect: Rect,
        lh: f32,
        metrics: &LayoutMetrics,
    ) -> SidebarEvent {
        let vr = self.active_viewport_rows(rect, lh, metrics);
        self.move_selection_by(delta, vr)
    }

    fn move_selection_by(&mut self, delta: isize, viewport_rows: usize) -> SidebarEvent {
        let Some(idx) = self.focus.active() else {
            return SidebarEvent::Ignored;
        };
        if self.collapsed[idx] {
            return SidebarEvent::Ignored;
        }
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
        rect: Rect,
        lh: f32,
        metrics: &LayoutMetrics,
    ) -> SidebarEvent {
        let vr = self.active_viewport_rows(rect, lh, metrics);
        self.jump_selection_to_edge_by(to_start, vr)
    }

    fn jump_selection_to_edge_by(&mut self, to_start: bool, viewport_rows: usize) -> SidebarEvent {
        let Some(idx) = self.focus.active() else {
            return SidebarEvent::Ignored;
        };
        match self.sections[idx].jump_to_edge(to_start, viewport_rows) {
            TreeControllerEvent::RowSelected { path } => {
                SidebarEvent::RowSelected { section: idx, path }
            }
            _ => SidebarEvent::Ignored,
        }
    }

    fn activate_selection(&self) -> SidebarEvent {
        let Some(idx) = self.focus.active() else {
            return SidebarEvent::Ignored;
        };
        match self.sections[idx].activate_selection() {
            TreeControllerEvent::RowActivated { path } => {
                SidebarEvent::RowActivated { section: idx, path }
            }
            _ => SidebarEvent::Ignored,
        }
    }

    // ── Inline editing forwarding ────────────────────────────────────

    fn editing_section(&self) -> Option<usize> {
        self.sections.iter().position(|tc| tc.is_editing())
    }

    fn forward_edit_char(&mut self, ch: char) -> SidebarEvent {
        let Some(idx) = self.editing_section() else {
            return SidebarEvent::Ignored;
        };
        let tc = &mut self.sections[idx];
        tc.edit_insert_char_via(ch);
        self.map_tc_edit_event(idx)
    }

    fn forward_edit_paste(&mut self, text: &str) -> SidebarEvent {
        let Some(idx) = self.editing_section() else {
            return SidebarEvent::Ignored;
        };
        let tc = &mut self.sections[idx];
        tc.edit_insert_str_via(text);
        self.map_tc_edit_event(idx)
    }

    fn forward_edit_key(&mut self, key: &Key, modifiers: &Modifiers) -> SidebarEvent {
        let Some(idx) = self.editing_section() else {
            return SidebarEvent::Ignored;
        };
        let ev = self.sections[idx].handle_edit_key_via(key, modifiers);
        Self::map_tc_event(idx, ev)
    }

    fn map_tc_edit_event(&self, idx: usize) -> SidebarEvent {
        let tc = &self.sections[idx];
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

    fn active_viewport_rows(&mut self, rect: Rect, lh: f32, metrics: &LayoutMetrics) -> usize {
        let Some(idx) = self.focus.active() else {
            return 0;
        };
        if let Some((cached_section, cached_vr)) = self.cached_viewport_rows {
            if cached_section == idx {
                return cached_vr;
            }
        }
        let vr = self.section_viewport_rows(idx, rect, lh, metrics);
        self.cached_viewport_rows = Some((idx, vr));
        vr
    }

    fn section_viewport_rows(
        &self,
        section: usize,
        rect: Rect,
        lh: f32,
        metrics: &LayoutMetrics,
    ) -> usize {
        let (layout, map) = self.compute_layout(rect, metrics, lh);
        let Some(msv_idx) = map.iter().position(|&s| s == section) else {
            return 0;
        };
        let body_b = layout.sections[msv_idx].body_bounds;
        self.viewport_rows_from_layout(body_b, msv_idx, lh)
    }

    fn viewport_rows_from_layout(&self, body_b: Rect, msv_section: usize, lh: f32) -> usize {
        let (view, _) = self.build_view();
        let SectionBody::Tree(t) = &view.sections[msv_section].body else {
            return 0;
        };
        let mut shadow = t.clone();
        shadow.scroll_offset = 0;
        let inner = self.compute_tree_layout(body_b, &shadow, lh);
        inner.visible_rows.len()
    }

    fn build_view(&self) -> (MultiSectionView, Vec<usize>) {
        let mut msv_to_sidebar: Vec<usize> = Vec::new();
        let mut sections: Vec<Section> = Vec::new();
        let active = self.focus.active();
        for (idx, def) in self.defs.iter().enumerate() {
            if !self.visible[idx] {
                continue;
            }
            let is_active = active == Some(idx);
            let tc = &self.sections[idx];
            let title = if is_active {
                format!("▶ {}", def.title)
            } else {
                def.title.clone()
            };
            sections.push(Section {
                id: def.id.clone(),
                header: SectionHeader {
                    title: StyledText::plain(title),
                    show_chevron: def.show_chevron,
                    badge: self.badges[idx].clone(),
                    ..Default::default()
                },
                body: SectionBody::Tree({
                    let mut rows = tc.rows().to_vec();
                    if let Some(editing_path) = tc.editing_path() {
                        if let Some(row) = rows.iter_mut().find(|r| &r.path == editing_path) {
                            row.edit = Some(TreeRowEditState {
                                text: tc.editing_text().unwrap_or("").to_string(),
                                cursor: tc.editing_cursor(),
                                selection_anchor: tc.editing_selection_anchor(),
                                placeholder: tc.editing_placeholder().map(String::from),
                            });
                        }
                    }
                    TreeView {
                        id: WidgetId::new(format!("{}-tree", def.id)),
                        rows,
                        selection_mode: SelectionMode::Single,
                        selected_path: tc.selected_path().cloned(),
                        scroll_offset: tc.scroll_offset(),
                        style: Default::default(),
                        has_focus: is_active && self.has_focus,
                    }
                }),
                aux: None,
                size: def.size,
                collapsed: self.collapsed[idx],
                min_size: None,
                max_size: None,
            });
            msv_to_sidebar.push(idx);
        }
        let msv_active = active.and_then(|a| msv_to_sidebar.iter().position(|&s| s == a));
        let view = MultiSectionView {
            id: WidgetId::new("sidebar-system"),
            sections,
            active_section: msv_active,
            axis: MsvAxis::Vertical,
            allow_resize: false,
            allow_collapse: self.allow_collapse,
            scroll_mode: self.scroll_mode,
            has_focus: self.has_focus,
            panel_scroll: self.panel_scroll,
        };
        (view, msv_to_sidebar)
    }

    fn click(
        &mut self,
        rect: Rect,
        x: f32,
        y: f32,
        lh: f32,
        metrics: &LayoutMetrics,
    ) -> SidebarEvent {
        let (layout, map) = self.compute_layout(rect, metrics, lh);
        let (view, _) = self.build_view();
        match layout.hit_test(x, y) {
            MultiSectionViewHit::Header {
                section: msv_idx, ..
            } => {
                let section = map[msv_idx];
                self.focus.set_active(Some(section));
                self.sections[section].set_selected_path(None);
                SidebarEvent::HeaderActivated { section }
            }
            MultiSectionViewHit::Body {
                section: msv_idx, ..
            } => {
                let section = map[msv_idx];
                self.focus.set_active(Some(section));
                let body_b = layout.sections[msv_idx].body_bounds;
                let tree = match &view.sections[msv_idx].body {
                    SectionBody::Tree(t) => t.clone(),
                    _ => return SidebarEvent::Consumed,
                };
                let inner = self.compute_tree_layout(body_b, &tree, lh);
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
                section: msv_idx,
                kind: ScrollbarHit::Thumb,
            } => {
                let section = map[msv_idx];
                let sb = layout.sections[msv_idx]
                    .scrollbar_bounds
                    .expect("scrollbar hit implies bounds present");
                let thumb_h = layout.sections[msv_idx]
                    .thumb_bounds
                    .map(|t| t.height)
                    .unwrap_or(sb.height);
                let body_b = layout.sections[msv_idx].body_bounds;
                let viewport_rows = self.viewport_rows_from_layout(body_b, msv_idx, lh);
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
                section: msv_idx,
                kind: ScrollbarHit::TrackBefore,
            } => {
                let section = map[msv_idx];
                let body_b = layout.sections[msv_idx].body_bounds;
                let viewport_rows = self.viewport_rows_from_layout(body_b, msv_idx, lh);
                self.sections[section].page_scroll(-(viewport_rows as isize), viewport_rows);
                SidebarEvent::ScrollChanged { section }
            }
            MultiSectionViewHit::Scrollbar {
                section: msv_idx,
                kind: ScrollbarHit::TrackAfter,
            } => {
                let section = map[msv_idx];
                let body_b = layout.sections[msv_idx].body_bounds;
                let viewport_rows = self.viewport_rows_from_layout(body_b, msv_idx, lh);
                self.sections[section].page_scroll(viewport_rows as isize, viewport_rows);
                SidebarEvent::ScrollChanged { section }
            }
            MultiSectionViewHit::PanelScrollbar {
                kind: ScrollbarHit::Thumb,
            } => {
                if let Some(sb) = layout.panel_scrollbar {
                    let total: f32 = layout.sections.iter().map(|s| s.resolved_size).sum();
                    let max_scroll = (total - rect.height).max(0.0);
                    let thumb_frac = rect.height / total;
                    let thumb_h = (sb.height * thumb_frac).max(lh);
                    let travel = (sb.height - thumb_h).max(0.0);
                    self.panel_drag = Some(PanelScrollDrag {
                        origin_y: y,
                        origin_scroll: self.panel_scroll,
                        travel,
                        max_scroll,
                    });
                    SidebarEvent::Consumed
                } else {
                    SidebarEvent::Ignored
                }
            }
            MultiSectionViewHit::PanelScrollbar {
                kind: ScrollbarHit::TrackBefore,
            } => {
                let total: f32 = layout.sections.iter().map(|s| s.resolved_size).sum();
                let max_scroll = (total - rect.height).max(0.0);
                self.panel_scroll = (self.panel_scroll - rect.height).clamp(0.0, max_scroll);
                SidebarEvent::Consumed
            }
            MultiSectionViewHit::PanelScrollbar {
                kind: ScrollbarHit::TrackAfter,
            } => {
                let total: f32 = layout.sections.iter().map(|s| s.resolved_size).sum();
                let max_scroll = (total - rect.height).max(0.0);
                self.panel_scroll = (self.panel_scroll + rect.height).clamp(0.0, max_scroll);
                SidebarEvent::Consumed
            }
            _ => SidebarEvent::Ignored,
        }
    }

    fn right_click(
        &mut self,
        rect: Rect,
        position: Point,
        lh: f32,
        metrics: &LayoutMetrics,
    ) -> SidebarEvent {
        let (layout, map) = self.compute_layout(rect, metrics, lh);
        let (view, _) = self.build_view();
        match layout.hit_test(position.x, position.y) {
            MultiSectionViewHit::Body {
                section: msv_idx, ..
            } => {
                let section = map[msv_idx];
                let body_b = layout.sections[msv_idx].body_bounds;
                let tree = match &view.sections[msv_idx].body {
                    SectionBody::Tree(t) => t.clone(),
                    _ => return SidebarEvent::Ignored,
                };
                let inner = self.compute_tree_layout(body_b, &tree, lh);
                match inner.hit_test(position.x - body_b.x, position.y - body_b.y) {
                    TreeViewHit::Row(idx) => {
                        let path = tree.rows[idx].path.clone();
                        self.focus.set_active(Some(section));
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

    fn drag_to(&mut self, y: f32) -> SidebarEvent {
        if let Some(drag) = &self.panel_drag {
            if drag.travel <= 0.0 || drag.max_scroll <= 0.0 {
                return SidebarEvent::Ignored;
            }
            let dy = y - drag.origin_y;
            let new = drag.origin_scroll + dy / drag.travel * drag.max_scroll;
            let new = new.clamp(0.0, drag.max_scroll);
            if (new - self.panel_scroll).abs() < 0.5 {
                return SidebarEvent::Ignored;
            }
            self.panel_scroll = new;
            return SidebarEvent::Consumed;
        }
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
        let n = self.defs.len();
        if n == 0 {
            return;
        }
        self.focus.cycle(delta);
        // Skip invisible sections.
        for _ in 0..n {
            if let Some(idx) = self.focus.active() {
                if self.visible[idx] {
                    return;
                }
                self.focus.cycle(delta);
            } else {
                return;
            }
        }
    }

    fn scroll_panel(&mut self, rect: Rect, dy: f32, lh: f32, metrics: &LayoutMetrics) {
        let (layout, _) = self.compute_layout(rect, metrics, lh);
        let total: f32 = layout.sections.iter().map(|s| s.resolved_size).sum();
        let max = (total - rect.height).max(0.0);
        self.panel_scroll = (self.panel_scroll + dy).clamp(0.0, max);
    }

    fn scroll_to_active_section(&mut self, rect: Rect, lh: f32, metrics: &LayoutMetrics) {
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

    fn scroll_active(&mut self, rect: Rect, delta: isize, lh: f32, metrics: &LayoutMetrics) {
        let Some(idx) = self.focus.active() else {
            return;
        };
        let (layout, map) = self.compute_layout(rect, metrics, lh);
        let Some(msv_idx) = map.iter().position(|&s| s == idx) else {
            return;
        };
        let body_b = layout.sections[msv_idx].body_bounds;
        let viewport_rows = self.viewport_rows_from_layout(body_b, msv_idx, lh);
        let row_count = self.sections[idx].rows().len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.sections[idx].scroll_offset() as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.sections[idx].set_scroll_offset(new);
    }

    fn select_first_of_active(&mut self) {
        let Some(idx) = self.focus.active() else {
            return;
        };
        if let Some(first) = self.sections[idx].rows().first() {
            let path = first.path.clone();
            self.sections[idx].set_selected_path(Some(path));
            self.sections[idx].set_scroll_offset(0);
        }
    }
}

fn body_measure(section: &Section, lh: f32) -> SectionMeasure {
    let aux_size = if section.aux.is_some() {
        (lh * 1.4).round()
    } else {
        0.0
    };
    let item_h = (lh * 1.4).round();
    let content_size = match &section.body {
        SectionBody::Tree(t) => {
            let header_h = (lh * 1.2).round();
            t.rows
                .iter()
                .map(|r| {
                    if matches!(r.decoration, crate::types::Decoration::Header) {
                        header_h
                    } else {
                        item_h
                    }
                })
                .sum()
        }
        SectionBody::List(l) => {
            let title_h = if l.title.is_some() { lh } else { 0.0 };
            title_h + l.items.len() as f32 * item_h
        }
        SectionBody::Form(f) => f.fields.len() as f32 * item_h,
        _ => 0.0,
    };
    SectionMeasure {
        content_size,
        aux_size,
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
                edit: None,
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
        ss.focus.set_active(Some(1));
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
        ss.focus.set_active(Some(0));
        let (view, _) = ss.build_view();
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
        ss.focus.set_active(Some(0));
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
        ss.focus.set_active(None);
        let ev = ss.move_selection_by(1, 10);
        assert_eq!(ev, SidebarEvent::Ignored);
    }

    #[test]
    fn navigation_mode_defaults_to_scroll() {
        let ss = SidebarSystem::new(sample_defs());
        assert_eq!(ss.navigation_mode(), NavigationMode::Scroll);
    }

    #[test]
    fn scroll_mode_defaults_to_per_section() {
        let ss = SidebarSystem::new(sample_defs());
        assert_eq!(ss.scroll_mode(), ScrollMode::PerSection);
        assert_eq!(ss.panel_scroll(), 0.0);
    }

    #[test]
    fn set_scroll_mode_whole_panel() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_scroll_mode(ScrollMode::WholePanel);
        assert_eq!(ss.scroll_mode(), ScrollMode::WholePanel);
    }

    #[test]
    fn set_panel_scroll_clamps_negative() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_scroll_mode(ScrollMode::WholePanel);
        ss.set_panel_scroll(-10.0);
        assert_eq!(ss.panel_scroll(), 0.0);
    }

    #[test]
    fn build_view_uses_scroll_mode() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 5));
        ss.set_scroll_mode(ScrollMode::WholePanel);
        ss.set_panel_scroll(10.0);
        let (view, _) = ss.build_view();
        assert_eq!(view.scroll_mode, ScrollMode::WholePanel);
        assert_eq!(view.panel_scroll, 10.0);
    }

    // ── Section visibility ──────────────────────────────────────────

    #[test]
    fn sections_visible_by_default() {
        let ss = SidebarSystem::new(sample_defs());
        assert!(ss.is_section_visible(0));
        assert!(ss.is_section_visible(1));
    }

    #[test]
    fn hidden_section_excluded_from_view() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 3));
        ss.set_rows(1, fake_rows("w", 2));
        ss.set_section_visible(0, false);
        let (view, map) = ss.build_view();
        assert_eq!(view.sections.len(), 2);
        assert_eq!(view.sections[0].id, "watch");
        assert_eq!(view.sections[1].id, "breakpoints");
        assert_eq!(map, vec![1, 2]);
    }

    #[test]
    fn tab_skips_hidden_section() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 3));
        ss.set_rows(1, fake_rows("w", 2));
        ss.set_active_section(Some(0));
        ss.set_section_visible(1, false);
        ss.cycle_active(1);
        // Should skip section 1 (hidden) and land on section 0 (wraps).
        assert_ne!(ss.active_section(), Some(1));
    }

    // ── Section badges ──────────────────────────────────────────────

    #[test]
    fn badge_defaults_to_none() {
        let ss = SidebarSystem::new(sample_defs());
        assert_eq!(ss.section_badge(0), None);
    }

    #[test]
    fn set_badge_appears_in_view() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 3));
        ss.set_section_badge(0, Some(StyledText::plain("(3)")));
        let (view, _) = ss.build_view();
        assert_eq!(
            view.sections[0].header.badge,
            Some(StyledText::plain("(3)"))
        );
    }
}
