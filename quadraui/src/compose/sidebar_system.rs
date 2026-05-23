//! `SidebarSystem` — a composed controller for MSV sidebar panels.
//!
//! Owns the full interaction state machine: per-section scroll/selection,
//! active section cycling, scrollbar drag, keyboard navigation, and
//! two-layer click dispatch (MSV → body with coordinate translation).
//!
//! Sections may be Tree-bodied (managed by `TreeController`) or
//! Form-bodied (managed by `FormController`). The section kind is set
//! at construction time via [`SectionKind`] on [`SidebarSectionDef`].
//!
//! Two navigation modes (set via [`SidebarSystem::set_navigation_mode`]):
//! - [`NavigationMode::Scroll`] (default): Up/Down scroll the viewport.
//! - [`NavigationMode::Selection`]: Up/Down/j/k move `selected_path` to
//!   the next/previous row with scroll-to-follow. Home/End/PageUp/PageDown
//!   jump by page or extremes. Enter emits [`SidebarEvent::RowActivated`].
//!
//! Apps define section structure via [`SidebarSectionDef`], set row data
//! per frame via [`SidebarSystem::set_rows`] (tree sections) or
//! [`SidebarSystem::set_form`] (form sections), and match on
//! [`SidebarEvent`] for semantic actions.
//!
//! Scroll-wheel events follow the [`ScrollDelta`](crate::ScrollDelta)
//! sign convention: positive `delta.y` = scroll content up (decrease
//! offset). Backends normalise their native direction before emitting.

use super::focus_group::FocusGroup;
use super::form_controller::{form_click_event, FormController};
use super::tree_controller::TreeController;
use super::tree_controller::TreeControllerEvent;
use crate::primitives::form::{
    FieldKind, Form, FormEvent, FormFieldMeasure, FormItemMeasure, FormLayout,
};
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

/// Whether a sidebar section hosts a TreeView or a Form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SectionKind {
    #[default]
    Tree,
    Form,
}

/// Definition of one sidebar section (structure, not data).
#[derive(Debug, Clone)]
pub struct SidebarSectionDef {
    pub id: String,
    pub title: String,
    pub show_chevron: bool,
    pub size: SectionSize,
    pub kind: SectionKind,
}

impl SidebarSectionDef {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            show_chevron: false,
            size: SectionSize::EqualShare,
            kind: SectionKind::Tree,
        }
    }

    pub fn form(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            show_chevron: false,
            size: SectionSize::Content,
            kind: SectionKind::Form,
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
    /// A Form section emitted a form event (toggle, text input, button, etc.).
    FormEvent { section: usize, event: FormEvent },
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

enum SectionController {
    Tree(TreeController),
    Form(FormController),
}

pub struct SidebarSystem {
    defs: Vec<SidebarSectionDef>,
    sections: Vec<SectionController>,
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
    cached_form_layouts: Vec<Option<FormLayout>>,
}

impl SidebarSystem {
    pub fn new(defs: Vec<SidebarSectionDef>) -> Self {
        let n = defs.len();
        let sections = defs
            .iter()
            .map(|def| match def.kind {
                SectionKind::Tree => {
                    SectionController::Tree(TreeController::new(format!("{}-tree", def.id)))
                }
                SectionKind::Form => {
                    SectionController::Form(FormController::new(format!("{}-form", def.id)))
                }
            })
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
            cached_form_layouts: vec![None; n],
        }
    }

    // ── Per-frame data ────────────────────────────────────────────────

    pub fn set_rows(&mut self, section: usize, rows: Vec<TreeRow>) {
        if let Some(SectionController::Tree(tc)) = self.sections.get_mut(section) {
            tc.set_rows(rows);
        }
    }

    pub fn set_form(&mut self, section: usize, form: Form) {
        if let Some(SectionController::Form(fc)) = self.sections.get_mut(section) {
            fc.set_form(form);
        }
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn active_section(&self) -> Option<usize> {
        self.focus.active()
    }

    pub fn selected_path(&self, section: usize) -> Option<&TreePath> {
        match self.sections.get(section) {
            Some(SectionController::Tree(tc)) => tc.selected_path(),
            _ => None,
        }
    }

    pub fn scroll_offset(&self, section: usize) -> usize {
        match self.sections.get(section) {
            Some(SectionController::Tree(tc)) => tc.scroll_offset(),
            _ => 0,
        }
    }

    pub fn form(&self, section: usize) -> Option<&Form> {
        match self.sections.get(section) {
            Some(SectionController::Form(fc)) => fc.form(),
            _ => None,
        }
    }

    pub fn section_kind(&self, section: usize) -> Option<SectionKind> {
        self.defs.get(section).map(|d| d.kind)
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
        if let Some(SectionController::Tree(tc)) = self.sections.get_mut(section) {
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

    /// Pre-compute and cache form layouts using the backend's native
    /// measurement (Pango for GTK, char cells for TUI). Call after
    /// `render()` or `set_form()` so that [`Self::handle_cached`]
    /// uses pixel-accurate hit regions instead of the generic estimate.
    pub fn cache_form_layouts(&mut self, backend: &dyn Backend) {
        let (view, map) = self.build_view();
        let lh = backend.line_height();
        let metrics = backend.msv_metrics();
        let layout = view.layout(Rect::new(0.0, 0.0, 0.0, 0.0), metrics, |i| {
            body_measure(&view.sections[i], lh)
        });
        for (msv_idx, s_layout) in layout.sections.iter().enumerate() {
            let sidebar_idx = map[msv_idx];
            if let SectionBody::Form(ref f) = view.sections[msv_idx].body {
                let fl = backend.form_layout(s_layout.body_bounds, f);
                if sidebar_idx < self.cached_form_layouts.len() {
                    self.cached_form_layouts[sidebar_idx] = Some(fl);
                }
            }
        }
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
        self.handle_inner(event, rect, lh, &metrics, Some(&*backend))
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
        self.handle_inner(event, rect, lh, &metrics, None)
    }

    fn handle_inner(
        &mut self,
        event: &UiEvent,
        rect: Rect,
        lh: f32,
        metrics: &LayoutMetrics,
        backend: Option<&dyn Backend>,
    ) -> SidebarEvent {
        // Snap the rect to the backend's render quantum so paint and
        // hit-test agree. The TUI rasteriser routes through
        // `q_rect_to_ratatui` which rounds the rect to integer cells
        // (half-away-from-zero); SidebarSystem must mirror that or the
        // overflow / scrollbar-presence decisions drift. See the
        // `tui_handle_snaps_fractional_rect_height_to_match_paint` test
        // for the multi_tree shape that surfaces this — the
        // user-visible #241 retry symptom.
        let rect = snap_rect_to_quantum(rect, metrics.cell_quantum);
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
            } => self.click(rect, position.x, position.y, lh, metrics, backend),

            // ── Right-click ──────────────────────────────────────
            UiEvent::MouseDown {
                button: MouseButton::Right,
                position,
                ..
            } => self.right_click(rect, *position, lh, metrics),

            // ── Double-click → forward to TreeController for RowActivated
            UiEvent::DoubleClick { position, .. } => {
                self.double_click(rect, position.x, position.y, lh, metrics, backend)
            }

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
            //
            // Route to the section under the cursor rather than always
            // sending to the active section. This is the native
            // convention on all desktop platforms: scroll goes to the
            // widget under the pointer, not to the focused widget.
            //
            // Fallback: if the cursor position doesn't hit any
            // scrollable section (e.g. keyboard-driven scroll where
            // backends emit a default origin, or cursor outside the
            // MSV bounds), we fall back to scrolling the active section.
            UiEvent::Scroll {
                delta, position, ..
            } => {
                if self.scroll_mode == ScrollMode::WholePanel {
                    let dy = if delta.y > 0.0 { -lh } else { lh };
                    self.scroll_panel(rect, dy, lh, metrics);
                } else {
                    let rows = if delta.y > 0.0 { -1 } else { 1 };
                    if !self.scroll_section_at(*position, rect, rows, lh, metrics) {
                        self.scroll_active(rect, rows, lh, metrics);
                    }
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
            .is_none_or(|sc| match sc {
                SectionController::Tree(tc) => tc.vim_keys(),
                SectionController::Form(_) => false,
            });
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
        let SectionController::Tree(tc) = &mut self.sections[idx] else {
            return SidebarEvent::Ignored;
        };
        match tc.move_selection_by(delta, viewport_rows) {
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
        let SectionController::Tree(tc) = &mut self.sections[idx] else {
            return SidebarEvent::Ignored;
        };
        match tc.jump_to_edge(to_start, viewport_rows) {
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
        let SectionController::Tree(tc) = &self.sections[idx] else {
            return SidebarEvent::Ignored;
        };
        match tc.activate_selection() {
            TreeControllerEvent::RowActivated { path } => {
                SidebarEvent::RowActivated { section: idx, path }
            }
            _ => SidebarEvent::Ignored,
        }
    }

    // ── Inline editing forwarding ────────────────────────────────────

    fn editing_section(&self) -> Option<usize> {
        self.sections
            .iter()
            .position(|sc| matches!(sc, SectionController::Tree(tc) if tc.is_editing()))
    }

    fn forward_edit_char(&mut self, ch: char) -> SidebarEvent {
        let Some(idx) = self.editing_section() else {
            return SidebarEvent::Ignored;
        };
        let SectionController::Tree(tc) = &mut self.sections[idx] else {
            return SidebarEvent::Ignored;
        };
        tc.edit_insert_char_via(ch);
        self.map_tc_edit_event(idx)
    }

    fn forward_edit_paste(&mut self, text: &str) -> SidebarEvent {
        let Some(idx) = self.editing_section() else {
            return SidebarEvent::Ignored;
        };
        let SectionController::Tree(tc) = &mut self.sections[idx] else {
            return SidebarEvent::Ignored;
        };
        tc.edit_insert_str_via(text);
        self.map_tc_edit_event(idx)
    }

    fn forward_edit_key(&mut self, key: &Key, modifiers: &Modifiers) -> SidebarEvent {
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
            let title = if is_active {
                format!("▶ {}", def.title)
            } else {
                def.title.clone()
            };
            let body = match &self.sections[idx] {
                SectionController::Tree(tc) => {
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
                    SectionBody::Tree(TreeView {
                        id: WidgetId::new(format!("{}-tree", def.id)),
                        rows,
                        selection_mode: SelectionMode::Single,
                        selected_path: tc.selected_path().cloned(),
                        scroll_offset: tc.scroll_offset(),
                        style: Default::default(),
                        has_focus: is_active && self.has_focus,
                    })
                }
                SectionController::Form(fc) => {
                    let form = fc.form().cloned().unwrap_or_else(|| Form {
                        id: fc.default_form_id(),
                        fields: Vec::new(),
                        focused_field: None,
                        scroll_offset: 0,
                        has_focus: is_active && self.has_focus,
                    });
                    SectionBody::Form(Form {
                        has_focus: is_active && self.has_focus,
                        ..form
                    })
                }
            };
            sections.push(Section {
                id: def.id.clone(),
                header: SectionHeader {
                    title: StyledText::plain(title),
                    show_chevron: def.show_chevron,
                    badge: self.badges[idx].clone(),
                    ..Default::default()
                },
                body,
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
        backend: Option<&dyn Backend>,
    ) -> SidebarEvent {
        let (layout, map) = self.compute_layout(rect, metrics, lh);
        let (view, _) = self.build_view();
        match layout.hit_test(x, y) {
            MultiSectionViewHit::Header {
                section: msv_idx, ..
            } => {
                let section = map[msv_idx];
                self.focus.set_active(Some(section));
                if let SectionController::Tree(tc) = &mut self.sections[section] {
                    tc.set_selected_path(None);
                }
                if self.allow_collapse {
                    self.collapsed[section] = !self.collapsed[section];
                }
                SidebarEvent::HeaderActivated { section }
            }
            MultiSectionViewHit::Body {
                section: msv_idx, ..
            } => {
                let section = map[msv_idx];
                self.focus.set_active(Some(section));
                let body_b = layout.sections[msv_idx].body_bounds;
                match &view.sections[msv_idx].body {
                    SectionBody::Tree(t) => {
                        let tree = t.clone();
                        let inner = self.compute_tree_layout(body_b, &tree, lh);
                        match inner.hit_test(x - body_b.x, y - body_b.y) {
                            TreeViewHit::Row(idx) => {
                                let path = tree.rows[idx].path.clone();
                                if let SectionController::Tree(tc) = &mut self.sections[section] {
                                    tc.set_selected_path(Some(path.clone()));
                                }
                                SidebarEvent::RowSelected { section, path }
                            }
                            TreeViewHit::Empty => SidebarEvent::HeaderActivated { section },
                        }
                    }
                    SectionBody::Form(f) => {
                        let form_layout = if let Some(be) = backend {
                            be.form_layout(body_b, f)
                        } else if let Some(cached) = self
                            .cached_form_layouts
                            .get(section)
                            .and_then(|c| c.clone())
                        {
                            cached
                        } else {
                            let row_h = (lh * 1.4).round();
                            let char_w = lh * 0.6;
                            f.layout(body_b.width, body_b.height, |i| {
                                form_field_measure(&f.fields[i], row_h, char_w)
                            })
                        };
                        match form_layout.hit_test(x - body_b.x, y - body_b.y) {
                            crate::primitives::form::FormHit::Field(id) => {
                                let event = form_click_event(f, &id);
                                SidebarEvent::FormEvent { section, event }
                            }
                            crate::primitives::form::FormHit::Empty => {
                                SidebarEvent::HeaderActivated { section }
                            }
                        }
                    }
                    _ => SidebarEvent::Consumed,
                }
            }
            MultiSectionViewHit::Scrollbar {
                section: msv_idx,
                kind: ScrollbarHit::Thumb,
            } => {
                let section = map[msv_idx];
                let SectionController::Tree(tc) = &self.sections[section] else {
                    return SidebarEvent::Ignored;
                };
                let sb = layout.sections[msv_idx]
                    .scrollbar_bounds
                    .expect("scrollbar hit implies bounds present");
                let thumb_h = layout.sections[msv_idx]
                    .thumb_bounds
                    .map(|t| t.height)
                    .unwrap_or(sb.height);
                let body_b = layout.sections[msv_idx].body_bounds;
                let viewport_rows = self.viewport_rows_from_layout(body_b, msv_idx, lh);
                let row_count = tc.rows().len();
                let max_offset = row_count.saturating_sub(viewport_rows);
                let travel = (sb.height - thumb_h).max(0.0);
                self.scroll_drag = Some(ScrollDrag {
                    section,
                    origin_y: y,
                    origin_offset: tc.scroll_offset(),
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
                let SectionController::Tree(tc) = &mut self.sections[section] else {
                    return SidebarEvent::Ignored;
                };
                tc.page_scroll(-(viewport_rows as isize), viewport_rows);
                SidebarEvent::ScrollChanged { section }
            }
            MultiSectionViewHit::Scrollbar {
                section: msv_idx,
                kind: ScrollbarHit::TrackAfter,
            } => {
                let section = map[msv_idx];
                let body_b = layout.sections[msv_idx].body_bounds;
                let viewport_rows = self.viewport_rows_from_layout(body_b, msv_idx, lh);
                let SectionController::Tree(tc) = &mut self.sections[section] else {
                    return SidebarEvent::Ignored;
                };
                tc.page_scroll(viewport_rows as isize, viewport_rows);
                SidebarEvent::ScrollChanged { section }
            }
            MultiSectionViewHit::PanelScrollbar {
                kind: ScrollbarHit::Thumb,
            } => {
                if let (Some(sb), Some(thumb)) = (layout.panel_scrollbar, layout.panel_thumb_bounds)
                {
                    // Total content the panel scrolls through — match
                    // `MultiSectionView::layout`'s `total_content`
                    // (sections + dividers) so drag math agrees with
                    // the painted thumb position. The legacy code
                    // dropped divider sizes, breaking drag when
                    // `allow_resize=true`.
                    let sections_total: f32 = layout.sections.iter().map(|s| s.resolved_size).sum();
                    let dividers_total: f32 = layout.dividers.iter().map(|d| d.bounds.height).sum();
                    let total = sections_total + dividers_total;
                    let max_scroll = (total - rect.height).max(0.0);
                    // The layout owns the thumb rect — paint, hit-test
                    // and drag all use the same dimensions. Pre-#241-
                    // retry the drag init recomputed thumb_h here with
                    // a formula that didn't match what the painter drew
                    // (which itself differed from `layout.hit_regions`).
                    // The user dragged but saw nothing move because
                    // the three derivations disagreed on travel.
                    let travel = (sb.height - thumb.height).max(0.0);
                    let _ = (lh, metrics);
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

    fn double_click(
        &mut self,
        rect: Rect,
        x: f32,
        y: f32,
        lh: f32,
        metrics: &LayoutMetrics,
        _backend: Option<&dyn Backend>,
    ) -> SidebarEvent {
        let (layout, map) = self.compute_layout(rect, metrics, lh);
        let (view, _) = self.build_view();
        match layout.hit_test(x, y) {
            MultiSectionViewHit::Body {
                section: msv_idx, ..
            } => {
                let section = map[msv_idx];
                let body_b = layout.sections[msv_idx].body_bounds;
                if let SectionBody::Tree(t) = &view.sections[msv_idx].body {
                    let tree = t.clone();
                    let inner = self.compute_tree_layout(body_b, &tree, lh);
                    if let TreeViewHit::Row(idx) = inner.hit_test(x - body_b.x, y - body_b.y) {
                        let path = tree.rows[idx].path.clone();
                        if let SectionController::Tree(tc) = &mut self.sections[section] {
                            tc.set_selected_path(Some(path.clone()));
                        }
                        return SidebarEvent::RowActivated { section, path };
                    }
                }
                SidebarEvent::Ignored
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
                        if let SectionController::Tree(tc) = &mut self.sections[section] {
                            tc.set_selected_path(Some(path.clone()));
                        }
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
        let SectionController::Tree(tc) = &mut self.sections[section] else {
            return SidebarEvent::Ignored;
        };
        if new == tc.scroll_offset() {
            return SidebarEvent::Ignored;
        }
        tc.set_scroll_offset(new);
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
    fn scroll_section_at(
        &mut self,
        position: Point,
        rect: Rect,
        rows: isize,
        lh: f32,
        metrics: &LayoutMetrics,
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

    fn scroll_active(&mut self, rect: Rect, delta: isize, lh: f32, metrics: &LayoutMetrics) {
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

    fn select_first_of_active(&mut self) {
        let Some(idx) = self.focus.active() else {
            return;
        };
        let SectionController::Tree(tc) = &mut self.sections[idx] else {
            return;
        };
        if let Some(first) = tc.rows().first() {
            let path = first.path.clone();
            tc.set_selected_path(Some(path));
            tc.set_scroll_offset(0);
        }
    }
}

/// Snap a rect's coordinates to integer multiples of `quantum`, using
/// the same rounding (`f32::round` — half-away-from-zero) that
/// [`crate::tui::backend`]'s `q_rect_to_ratatui` applies when it
/// converts to `ratatui::layout::Rect`'s `u16` fields. Mirroring that
/// rounding here keeps SidebarSystem's hit-test layout in lock-step
/// with the rasteriser's paint layout.
///
/// When `quantum <= 0.0` (GTK / macOS — pixel coordinates) the rect is
/// passed through unchanged: those backends paint at fractional
/// precision and don't suffer the rounding drift TUI's u16 paint area
/// introduces.
fn snap_rect_to_quantum(rect: Rect, quantum: f32) -> Rect {
    if quantum <= 0.0 {
        return rect;
    }
    let snap = |v: f32| (v / quantum).round() * quantum;
    Rect::new(
        snap(rect.x.max(0.0)),
        snap(rect.y.max(0.0)),
        snap(rect.width).max(0.0),
        snap(rect.height).max(0.0),
    )
}

fn form_field_measure(
    field: &crate::primitives::form::FormField,
    row_h: f32,
    char_w: f32,
) -> FormFieldMeasure {
    match &field.kind {
        FieldKind::ToggleGroup { toggles } => {
            let label_w = field.label.visible_width() as f32 * char_w;
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let items = toggles
                .iter()
                .map(|t| FormItemMeasure {
                    id: t.id.clone(),
                    width: (t.label.chars().count() as f32 + 2.0) * char_w,
                })
                .collect();
            FormFieldMeasure::with_items(row_h, start_x, char_w, items)
        }
        FieldKind::ButtonRow { buttons } => {
            let label_w = field.label.visible_width() as f32 * char_w;
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let items = buttons
                .iter()
                .map(|b| {
                    let icon_w = b
                        .icon
                        .as_ref()
                        .map(|i| {
                            let gw = i.fallback.chars().count() as f32;
                            if b.label.is_empty() {
                                gw
                            } else {
                                gw + 1.0
                            }
                        })
                        .unwrap_or(0.0);
                    FormItemMeasure {
                        id: b.id.clone(),
                        width: (b.label.chars().count() as f32 + icon_w + 2.0) * char_w,
                    }
                })
                .collect();
            FormFieldMeasure::with_items(row_h, start_x, char_w, items)
        }
        FieldKind::SegmentedControl { options, .. } => {
            let label_w = field.label.visible_width() as f32 * char_w;
            let start_x = if label_w > 0.0 {
                label_w + char_w * 2.0
            } else {
                char_w
            };
            let items = options
                .iter()
                .enumerate()
                .map(|(idx, opt)| FormItemMeasure {
                    id: WidgetId::new(format!("{}__seg_{idx}", field.id.as_str())),
                    width: (opt.chars().count() as f32 + 2.0) * char_w,
                })
                .collect();
            FormFieldMeasure::with_items(row_h, start_x, 0.0, items)
        }
        FieldKind::TextArea { visible_rows, .. } => {
            FormFieldMeasure::new(row_h * *visible_rows as f32)
        }
        _ => FormFieldMeasure::new(row_h),
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
        let tc = match &ss.sections[0] {
            SectionController::Tree(tc) => tc,
            _ => panic!("expected tree section"),
        };
        assert!(tc.rows().is_empty());
        ss.set_rows(0, fake_rows("v", 5));
        let tc = match &ss.sections[0] {
            SectionController::Tree(tc) => tc,
            _ => panic!("expected tree section"),
        };
        assert_eq!(tc.rows().len(), 5);
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
        if let SectionController::Tree(tc) = &mut ss.sections[1] {
            tc.set_scroll_offset(3);
        }
        ss.focus.set_active(Some(1));
        ss.select_first_of_active();
        assert_eq!(ss.selected_path(1), Some(&vec![0]));
        assert_eq!(ss.scroll_offset(1), 0);
    }

    #[test]
    fn page_scroll_clamps_to_bounds() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_rows(0, fake_rows("v", 20));
        if let SectionController::Tree(tc) = &mut ss.sections[0] {
            tc.page_scroll(100, 5);
        }
        assert_eq!(ss.scroll_offset(0), 15); // 20 - 5
        if let SectionController::Tree(tc) = &mut ss.sections[0] {
            tc.page_scroll(-100, 5);
        }
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
        if let SectionController::Tree(tc) = &mut ss.sections[0] {
            tc.set_scroll_offset(2);
        }
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

    // ── Form section support ───────────────────────────────────────────

    fn mixed_defs() -> Vec<SidebarSectionDef> {
        vec![
            SidebarSectionDef::form("search", "SEARCH"),
            SidebarSectionDef::new("results", "RESULTS"),
        ]
    }

    fn sample_form() -> Form {
        use crate::primitives::form::{FieldKind, FormField};
        Form {
            id: WidgetId::new("search-form"),
            fields: vec![
                FormField {
                    id: WidgetId::new("query"),
                    label: StyledText::plain("Query"),
                    kind: FieldKind::TextInput {
                        value: String::new(),
                        placeholder: "Search...".into(),
                        cursor: None,
                        selection_anchor: None,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("case-sensitive"),
                    label: StyledText::plain("Case"),
                    kind: FieldKind::Toggle { value: false },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
            ],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        }
    }

    #[test]
    fn mixed_sidebar_creates_correct_controllers() {
        let ss = SidebarSystem::new(mixed_defs());
        assert!(matches!(ss.sections[0], SectionController::Form(_)));
        assert!(matches!(ss.sections[1], SectionController::Tree(_)));
    }

    #[test]
    fn form_def_defaults_to_content_size() {
        let def = SidebarSectionDef::form("s", "S");
        assert_eq!(def.kind, SectionKind::Form);
        assert_eq!(def.size, SectionSize::Content);
    }

    #[test]
    fn set_form_updates_form_section() {
        let mut ss = SidebarSystem::new(mixed_defs());
        assert!(ss.form(0).is_none());
        ss.set_form(0, sample_form());
        assert!(ss.form(0).is_some());
        assert_eq!(ss.form(0).unwrap().fields.len(), 2);
    }

    #[test]
    fn set_form_on_tree_section_is_noop() {
        let mut ss = SidebarSystem::new(mixed_defs());
        ss.set_form(1, sample_form());
        assert!(ss.form(1).is_none());
    }

    #[test]
    fn set_rows_on_form_section_is_noop() {
        let mut ss = SidebarSystem::new(mixed_defs());
        ss.set_rows(0, fake_rows("v", 5));
        assert_eq!(ss.scroll_offset(0), 0);
        assert_eq!(ss.selected_path(0), None);
    }

    #[test]
    fn build_view_produces_form_body() {
        let mut ss = SidebarSystem::new(mixed_defs());
        ss.set_form(0, sample_form());
        ss.set_rows(1, fake_rows("r", 3));
        let (view, _) = ss.build_view();
        assert_eq!(view.sections.len(), 2);
        assert!(matches!(view.sections[0].body, SectionBody::Form(_)));
        assert!(matches!(view.sections[1].body, SectionBody::Tree(_)));
    }

    #[test]
    fn build_view_form_section_has_focus() {
        let mut ss = SidebarSystem::new(mixed_defs());
        ss.set_form(0, sample_form());
        ss.focus.set_active(Some(0));
        let (view, _) = ss.build_view();
        match &view.sections[0].body {
            SectionBody::Form(f) => assert!(f.has_focus),
            _ => panic!("expected Form body"),
        }
    }

    #[test]
    fn build_view_empty_form_section() {
        let ss = SidebarSystem::new(mixed_defs());
        let (view, _) = ss.build_view();
        match &view.sections[0].body {
            SectionBody::Form(f) => assert!(f.fields.is_empty()),
            _ => panic!("expected Form body"),
        }
    }

    #[test]
    fn tab_cycles_through_mixed_sections() {
        let mut ss = SidebarSystem::new(mixed_defs());
        ss.set_form(0, sample_form());
        ss.set_rows(1, fake_rows("r", 3));
        ss.cycle_active(1);
        assert_eq!(ss.active_section(), Some(0));
        ss.cycle_active(1);
        assert_eq!(ss.active_section(), Some(1));
        ss.cycle_active(1);
        assert_eq!(ss.active_section(), Some(0));
    }

    #[test]
    fn section_kind_accessor() {
        let ss = SidebarSystem::new(mixed_defs());
        assert_eq!(ss.section_kind(0), Some(SectionKind::Form));
        assert_eq!(ss.section_kind(1), Some(SectionKind::Tree));
        assert_eq!(ss.section_kind(99), None);
    }

    #[test]
    fn selection_on_form_section_returns_ignored() {
        let mut ss = SidebarSystem::new(mixed_defs());
        ss.set_navigation_mode(NavigationMode::Selection);
        ss.set_form(0, sample_form());
        ss.focus.set_active(Some(0));
        let ev = ss.move_selection_by(1, 10);
        assert_eq!(ev, SidebarEvent::Ignored);
    }

    // ── Form click event dispatch ──────────────────────────────────────

    #[test]
    fn form_click_text_input_emits_focus_changed() {
        let form = sample_form();
        let ev = form_click_event(&form, &WidgetId::new("query"));
        assert_eq!(
            ev,
            FormEvent::FocusChanged {
                id: WidgetId::new("query")
            }
        );
    }

    #[test]
    fn form_click_toggle_emits_toggle_changed_with_flipped_value() {
        let form = sample_form();
        let ev = form_click_event(&form, &WidgetId::new("case-sensitive"));
        assert_eq!(
            ev,
            FormEvent::ToggleChanged {
                id: WidgetId::new("case-sensitive"),
                value: true, // was false, click flips to true
            }
        );
    }

    #[test]
    fn form_click_button_emits_button_clicked() {
        use crate::primitives::form::FormField;
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("submit"),
                label: StyledText::plain("Go"),
                kind: FieldKind::Button,
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let ev = form_click_event(&form, &WidgetId::new("submit"));
        assert_eq!(
            ev,
            FormEvent::ButtonClicked {
                id: WidgetId::new("submit")
            }
        );
    }

    #[test]
    fn form_click_toggle_group_item_emits_toggle_changed() {
        use crate::primitives::form::{FormField, ToggleGroupItem};
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("flags"),
                label: StyledText::plain("Flags"),
                kind: FieldKind::ToggleGroup {
                    toggles: vec![
                        ToggleGroupItem {
                            id: WidgetId::new("case"),
                            label: "Aa".into(),
                            value: true,
                        },
                        ToggleGroupItem {
                            id: WidgetId::new("regex"),
                            label: ".*".into(),
                            value: false,
                        },
                    ],
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let ev = form_click_event(&form, &WidgetId::new("regex"));
        assert_eq!(
            ev,
            FormEvent::ToggleChanged {
                id: WidgetId::new("regex"),
                value: true, // was false, flipped
            }
        );
    }

    #[test]
    fn form_click_button_row_item_emits_button_clicked() {
        use crate::primitives::form::{ButtonRowItem, FormField};
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("actions"),
                label: StyledText::plain(""),
                kind: FieldKind::ButtonRow {
                    buttons: vec![ButtonRowItem {
                        id: WidgetId::new("replace-all"),
                        label: "Replace All".into(),
                        disabled: false,
                        icon: None,
                    }],
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let ev = form_click_event(&form, &WidgetId::new("replace-all"));
        assert_eq!(
            ev,
            FormEvent::ButtonClicked {
                id: WidgetId::new("replace-all")
            }
        );
    }

    #[test]
    fn form_click_unknown_id_emits_focus_changed() {
        let form = sample_form();
        let ev = form_click_event(&form, &WidgetId::new("nonexistent"));
        assert_eq!(
            ev,
            FormEvent::FocusChanged {
                id: WidgetId::new("nonexistent")
            }
        );
    }

    // ── TUI scrollbar thumb drag (#241) ─────────────────────────────────
    //
    // TUI mode passes mouse coordinates in cell units (1 cell per row)
    // and `LayoutMetrics::scrollbar_size = 1.0`. The drag-travel math in
    // `drag_to` must produce a non-zero per-section scroll change for a
    // 1-cell mouse drag, the same way it does for a 1-line GTK drag.
    //
    // Pre-fix: the per-section scrollbar thumb did not move under drag in
    // TUI because the layout produced thumb_bounds with `thumb_h == sb.height`
    // (no spare travel) when section sizing rounded the resolved body
    // dimensions to integer cells.

    #[test]
    fn tui_per_section_scrollbar_thumb_drag_scrolls_active_section() {
        // TUI metrics: 1-cell rows, 1-cell scrollbar gutter, integer-cell
        // section heights. Use PerSection scroll mode so the per-section
        // gutter (and `ScrollDrag`) is exercised.
        let mut ss = SidebarSystem::new(vec![SidebarSectionDef::new("t", "T")]);
        ss.set_rows(0, fake_rows("r", 30));
        ss.set_active_section(Some(0));
        ss.set_scroll_mode(ScrollMode::PerSection);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);

        // 21-cell tall sidebar: 1 cell header + 20 cells body. Body shows
        // 20 of the 30 rows → max_offset = 10, thumb has 6 cells of travel.
        let rect = Rect::new(0.0, 0.0, 20.0, 21.0);

        // Compute layout to find the painted thumb position.
        let (layout, _map) = ss.compute_layout(rect, &metrics, 1.0);
        let sb = layout.sections[0]
            .scrollbar_bounds
            .expect("overflow → per-section scrollbar reserved");
        let thumb = layout.sections[0]
            .thumb_bounds
            .expect("Tree body → thumb bounds computed from scroll state");

        // Click on the painted thumb. Mouse coords are integer cells in TUI.
        // TUI mouse coords come in as integer cells.
        let click_x = sb.x.round();
        let click_y = thumb.y.round();
        let down = UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(click_x, click_y),
            modifiers: Modifiers::default(),
        };
        let _ = ss.handle_cached(&down, rect);

        // Drag down 1 cell. In TUI cell units the crossterm Drag event
        // delivers integer-cell positions — simulate a 1-cell vertical
        // movement.
        let move_ev = UiEvent::MouseMoved {
            position: Point::new(click_x, click_y + 1.0),
            buttons: ButtonMask {
                left: true,
                ..Default::default()
            },
        };
        let _ = ss.handle_cached(&move_ev, rect);

        // The active section's scroll_offset must have advanced. Pre-fix
        // it stayed at 0 because the cell-precise drag was lost to either
        // (a) a `travel <= 0.0` guard when `thumb_h >= sb.height`, or
        // (b) a drow that rounded to 0 rows.
        let offset = ss.scroll_offset(0);
        assert!(
            offset > 0,
            "1-cell TUI drag on per-section thumb should advance scroll \
             offset, but it stayed at {offset} (sb={sb:?}, thumb={thumb:?})"
        );
    }

    /// Repro of the `examples/common/multi_tree.rs` shape under TUI:
    /// 4 `Content`-sized sections that together overflow the viewport
    /// by just 1 row, in `WholePanel` mode. Pre-fix: a 1-cell drag on
    /// the panel thumb produced no scroll, because the painted thumb
    /// nearly filled the gutter (travel < 1 cell) AND the drag-math
    /// guard returned `Ignored` for sub-half-cell `panel_scroll` deltas.
    #[test]
    fn tui_panel_drag_works_in_multi_tree_example_shape() {
        let mut ss = SidebarSystem::new(vec![
            SidebarSectionDef::new("vars", "VARIABLES"),
            SidebarSectionDef::new("watch", "WATCH"),
            SidebarSectionDef::new("stack", "CALL STACK"),
            SidebarSectionDef::new("bps", "BREAKPOINTS"),
        ]);
        ss.set_rows(0, fake_rows("v", 12));
        ss.set_rows(1, fake_rows("w", 8));
        ss.set_rows(2, fake_rows("frame", 5));
        ss.set_rows(3, fake_rows("bp", 0));
        ss.set_scroll_mode(ScrollMode::WholePanel);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);
        // 30-row terminal − 2-row status bar = 28 rows. Total content
        // (13+9+6+1) = 29 → 1 row of overflow.
        let rect = Rect::new(0.0, 0.0, 30.0, 28.0);

        let (layout, _) = ss.compute_layout(rect, &metrics, 1.0);
        let panel_sb = layout.panel_scrollbar.expect("overflowing → panel sb");

        // Click the painted thumb (top of gutter when panel_scroll=0).
        let click_x = panel_sb.x.round();
        let click_y = panel_sb.y.round();
        let _ = ss.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(click_x, click_y),
                modifiers: Modifiers::default(),
            },
            rect,
        );
        let _ = ss.handle_cached(
            &UiEvent::MouseMoved {
                position: Point::new(click_x, click_y + 1.0),
                buttons: ButtonMask {
                    left: true,
                    ..Default::default()
                },
            },
            rect,
        );
        let after = ss.panel_scroll();
        assert!(
            after > 0.0,
            "multi_tree-shaped 1-cell panel drag should scroll, got {after} \
             (panel_sb={panel_sb:?})"
        );
    }

    /// Regression for #241: when the painted thumb fills nearly the whole
    /// gutter (small overflow), the drag-`travel` value approaches zero
    /// and the consumer's mouse cell coordinates can no longer move
    /// `scroll_offset`. Repro: 22 rows in a body that fits 20 rows.
    #[test]
    fn tui_per_section_drag_works_with_small_overflow() {
        let mut ss = SidebarSystem::new(vec![SidebarSectionDef::new("t", "T")]);
        ss.set_rows(0, fake_rows("r", 22));
        ss.set_active_section(Some(0));
        ss.set_scroll_mode(ScrollMode::PerSection);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);
        let rect = Rect::new(0.0, 0.0, 20.0, 21.0);

        let (layout, _) = ss.compute_layout(rect, &metrics, 1.0);
        let sb = layout.sections[0]
            .scrollbar_bounds
            .expect("scrollbar reserved");
        let thumb = layout.sections[0].thumb_bounds.expect("thumb computed");

        // TUI mouse coords come in as integer cells.
        let click_x = sb.x.round();
        let click_y = thumb.y.round();
        let _ = ss.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(click_x, click_y),
                modifiers: Modifiers::default(),
            },
            rect,
        );
        let _ = ss.handle_cached(
            &UiEvent::MouseMoved {
                position: Point::new(click_x, click_y + 1.0),
                buttons: ButtonMask {
                    left: true,
                    ..Default::default()
                },
            },
            rect,
        );
        let offset = ss.scroll_offset(0);
        assert!(
            offset > 0,
            "small-overflow 1-cell drag should advance scroll, got {offset} \
             (sb={sb:?}, thumb={thumb:?})"
        );
    }

    /// Regression for #241: in TUI the panel scrollbar's layout used a
    /// pixel-shaped `min_thumb = 8.0`, painting a 2-cell thumb while
    /// the layout reserved an 8-cell hit region. A click at a row
    /// *below* the painted thumb (in cells 2..8) was treated as a
    /// thumb-drag start instead of a `TrackAfter` page-scroll, so the
    /// user "dragged" what looked like empty track and `panel_scroll`
    /// jumped unexpectedly. Post-fix `panel_thumb_min` honours
    /// `cell_quantum` (1 cell in TUI), so the layout's thumb hit
    /// region matches the painted thumb's cells.
    #[test]
    fn tui_panel_thumb_hit_region_matches_painted_thumb_cells() {
        let mut ss = SidebarSystem::new(vec![SidebarSectionDef::new("t", "T")]);
        // Huge overflow → painted thumb is the minimum size in cells.
        ss.set_rows(0, fake_rows("r", 200));
        ss.set_scroll_mode(ScrollMode::WholePanel);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        let rect = Rect::new(0.0, 0.0, 30.0, 20.0);
        let (layout, _) = ss.compute_layout(rect, &metrics, 1.0);
        let panel_sb = layout.panel_scrollbar.expect("overflow → panel sb");

        // The painted thumb (per `paint_panel_scrollbar`) uses
        // `ceil(height * visible_frac).max(1)`. With height=20, total≈201,
        // visible_frac≈0.0995, painted thumb_h = ceil(1.99) = 2 cells.
        // Pre-fix the layout's thumb hit region was 8 cells; post-fix
        // it should be 2.
        let mut found_thumb_h: Option<f32> = None;
        for (rect, hit) in &layout.hit_regions {
            if matches!(
                hit,
                MultiSectionViewHit::PanelScrollbar {
                    kind: ScrollbarHit::Thumb
                }
            ) {
                found_thumb_h = Some(rect.height);
                break;
            }
        }
        let thumb_h = found_thumb_h.expect("layout must publish a Thumb hit region");
        assert!(
            thumb_h <= 4.0,
            "TUI panel thumb hit region was {thumb_h} cells (expected ~2 \
             to match the painted thumb). Pre-fix value was 8.0 cells, \
             which made the empty track below the painted thumb behave \
             like a thumb-drag. panel_sb={panel_sb:?}"
        );
    }

    /// Regression for #241: when the natural thumb height is smaller
    /// than the layout's `min_thumb` (8.0 cells in TUI), the painted
    /// thumb travels a *short* distance per row of scroll. Pre-fix the
    /// drag init used a different `min` (`lh = 1.0`), so the drag
    /// produced ~8× too much scroll for each cell of mouse motion —
    /// quickly clamping to `max_scroll` and visually appearing as
    /// "thumb jumps to the end" instead of "thumb follows the cursor".
    /// Post-fix, dragging 1 cell at a time produces a monotonically
    /// increasing thumb position rather than instant clamp.
    #[test]
    fn tui_panel_drag_matches_painted_thumb_under_large_overflow() {
        let mut ss = SidebarSystem::new(vec![SidebarSectionDef::new("t", "T")]);
        // Very tall content forces the natural thumb < min_thumb=8.
        ss.set_rows(0, fake_rows("r", 200));
        ss.set_scroll_mode(ScrollMode::WholePanel);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);
        let rect = Rect::new(0.0, 0.0, 30.0, 20.0);

        let (layout, _) = ss.compute_layout(rect, &metrics, 1.0);
        let panel_sb = layout.panel_scrollbar.expect("overflow → panel sb");

        // Click top of panel thumb (panel_scroll=0 → thumb at sb.y).
        let click_x = panel_sb.x.round();
        let click_y = panel_sb.y.round();
        let _ = ss.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(click_x, click_y),
                modifiers: Modifiers::default(),
            },
            rect,
        );

        // Drag 1 cell, then a second cell. The scroll must increase
        // monotonically AND not immediately jump to max_scroll on the
        // first cell — that's the pre-fix smell of using `lh=1` as the
        // min_thumb while the layout uses 8.0.
        let _ = ss.handle_cached(
            &UiEvent::MouseMoved {
                position: Point::new(click_x, click_y + 1.0),
                buttons: ButtonMask {
                    left: true,
                    ..Default::default()
                },
            },
            rect,
        );
        let after_one = ss.panel_scroll();

        let _ = ss.handle_cached(
            &UiEvent::MouseMoved {
                position: Point::new(click_x, click_y + 2.0),
                buttons: ButtonMask {
                    left: true,
                    ..Default::default()
                },
            },
            rect,
        );
        let after_two = ss.panel_scroll();

        let total = 200.0_f32 + 1.0; // 200 body rows + 1 header row
        let max_scroll = (total - rect.height).max(0.0);
        assert!(
            after_one > 0.0,
            "1-cell drag should advance panel_scroll (got {after_one})"
        );
        assert!(
            after_one < max_scroll,
            "1-cell drag should not instantly clamp to max_scroll \
             (got {after_one}, max_scroll={max_scroll}) — pre-fix sign \
             that drag travel was computed against the wrong min_thumb"
        );
        assert!(
            after_two > after_one,
            "2-cell drag should advance past 1-cell drag \
             (got after_two={after_two} vs after_one={after_one})"
        );
    }

    #[test]
    fn tui_panel_scrollbar_thumb_drag_scrolls_panel() {
        // Panel-level scrollbar in WholePanel mode under TUI metrics.
        // Multiple sections sized by Content overflow the rect → panel
        // scrollbar reserved on the trailing edge.
        let mut ss = SidebarSystem::new(vec![
            SidebarSectionDef::new("a", "A"),
            SidebarSectionDef::new("b", "B"),
            SidebarSectionDef::new("c", "C"),
        ]);
        ss.set_rows(0, fake_rows("a", 12));
        ss.set_rows(1, fake_rows("b", 12));
        ss.set_rows(2, fake_rows("c", 12));
        ss.set_scroll_mode(ScrollMode::WholePanel);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);
        let rect = Rect::new(0.0, 0.0, 20.0, 21.0);

        let (layout, _) = ss.compute_layout(rect, &metrics, 1.0);
        let panel_sb = layout
            .panel_scrollbar
            .expect("overflowing panel → panel scrollbar reserved");

        // Click somewhere in the panel scrollbar (thumb starts at the top
        // when panel_scroll == 0). Drag down 1 cell.
        // TUI mouse coords come in as integer cells.
        let click_x = panel_sb.x.round();
        let click_y = panel_sb.y.round();
        let down = UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(click_x, click_y),
            modifiers: Modifiers::default(),
        };
        let _ = ss.handle_cached(&down, rect);

        let move_ev = UiEvent::MouseMoved {
            position: Point::new(click_x, click_y + 1.0),
            buttons: ButtonMask {
                left: true,
                ..Default::default()
            },
        };
        let _ = ss.handle_cached(&move_ev, rect);

        let after = ss.panel_scroll();
        assert!(
            after > 0.0,
            "1-cell TUI drag on panel thumb should advance panel scroll, \
             but it stayed at {after} (panel_sb={panel_sb:?})"
        );
    }

    /// Regression for #241 (retry): the real-world `examples/common/multi_tree.rs`
    /// shape reserves a 1.5-cell status bar (`STATUS_BAR_LINES = 1.5`),
    /// leaving the sidebar rect with a fractional height. On a 30-cell tall
    /// terminal that's `30 - 1.5 = 28.5`.
    ///
    /// The TUI rasteriser converts the rect via `q_rect_to_ratatui` which
    /// rounds half-away-from-zero — so paint receives `area.height = 29`.
    /// Pre-fix `SidebarSystem::compute_layout` consumed the raw 28.5
    /// directly, so its overflow check `total > bounds.height` saw
    /// `29 > 28.5 = true` and reserved a panel scrollbar in the right
    /// column. Paint, working from `bounds.height = 29`, saw
    /// `29 > 29 = false` and painted NO scrollbar.
    ///
    /// The visible symptom: clicking the right column starts a
    /// `PanelScrollbar` drag against a thumb the user can't see — and
    /// the drag arithmetic computes against the hit-test's tiny 0.5-cell
    /// travel, so `panel_scroll` jumps around with no painted feedback.
    /// Closes the "thumb drag not working" issue at the actual
    /// integration shape, not just round-number bounds.
    #[test]
    fn tui_handle_snaps_fractional_rect_height_to_match_paint() {
        let mut ss = SidebarSystem::new(vec![
            SidebarSectionDef::new("vars", "VARIABLES"),
            SidebarSectionDef::new("watch", "WATCH"),
            SidebarSectionDef::new("stack", "CALL STACK"),
            SidebarSectionDef::new("bps", "BREAKPOINTS"),
        ]);
        ss.set_rows(0, fake_rows("v", 12));
        ss.set_rows(1, fake_rows("w", 8));
        ss.set_rows(2, fake_rows("frame", 5));
        ss.set_rows(3, fake_rows("bp", 0));
        ss.set_scroll_mode(ScrollMode::WholePanel);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);

        // multi_tree.rs reserves 1.5 cells for the status bar on a
        // 30-cell terminal → 28.5-cell sidebar rect. Paint (via
        // q_rect_to_ratatui) rounds this to 29; SidebarSystem must do
        // the same so its layout agrees with what the user sees.
        let rect = Rect::new(0.0, 0.0, 30.0, 28.5);

        // 4 sections, 13+9+6+1 = 29 cells of natural content. With
        // paint's effective height = 29, sections fit exactly: NO
        // panel scrollbar should be reserved. With the pre-fix raw
        // 28.5, the hit-test reported a scrollbar in the right
        // column even though paint never drew one.
        //
        // Click in the right column (where a scrollbar WOULD live if
        // there was overflow) and assert no `PanelScrollbar` drag
        // gets initiated — i.e. nothing happens or it routes to the
        // section under the click.
        let click = UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(29.0, 0.5), // rightmost column, top
            modifiers: Modifiers::default(),
        };
        let _ = ss.handle_cached(&click, rect);

        let drag = UiEvent::MouseMoved {
            position: Point::new(29.0, 5.0),
            buttons: ButtonMask {
                left: true,
                ..Default::default()
            },
        };
        let _ = ss.handle_cached(&drag, rect);

        // panel_scroll must stay at 0 — no overflow means no scrolling.
        // Pre-fix the 4.5-cell drag against the bogus 0.5-cell travel
        // produced `panel_scroll ≈ 4.5/0.5 * 0.5 = 4.5`, clamped to
        // max_scroll = 0.5. Visible: status reported "scrollbar→..."
        // and nothing painted moved.
        assert_eq!(
            ss.panel_scroll(),
            0.0,
            "with paint-effective bounds=29 and content=29 there is NO \
             overflow, so a drag in the rightmost column must NOT advance \
             panel_scroll. Pre-fix the fractional 28.5 fooled hit-test \
             into reserving a phantom scrollbar."
        );
    }

    /// Companion to the fractional-rect test above: when the rect's
    /// fractional part DOES push the snapped bounds past content,
    /// drag must still work normally. With `rect.height = 27.5` (≈ 28-cell
    /// effective) and the same 29-cell content, paint reserves a panel
    /// scrollbar (29 > 28) and a 1-cell drag must move `panel_scroll`.
    /// Locks the fix to a "snap, don't disable" semantic — we mirror
    /// paint's bounds, we don't drop hit-testing.
    #[test]
    fn tui_panel_drag_still_works_when_snapped_bounds_have_overflow() {
        let mut ss = SidebarSystem::new(vec![
            SidebarSectionDef::new("vars", "VARIABLES"),
            SidebarSectionDef::new("watch", "WATCH"),
            SidebarSectionDef::new("stack", "CALL STACK"),
            SidebarSectionDef::new("bps", "BREAKPOINTS"),
        ]);
        ss.set_rows(0, fake_rows("v", 12));
        ss.set_rows(1, fake_rows("w", 8));
        ss.set_rows(2, fake_rows("frame", 5));
        ss.set_rows(3, fake_rows("bp", 0));
        ss.set_scroll_mode(ScrollMode::WholePanel);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);

        // 27.5 rounds to 28 → 29 > 28 → real overflow, scrollbar painted.
        let rect = Rect::new(0.0, 0.0, 30.0, 27.5);
        let (layout, _) = ss.compute_layout(rect, &metrics, 1.0);
        let panel_sb = layout
            .panel_scrollbar
            .expect("snapped bounds=28 with content=29 should reserve scrollbar");

        let click_x = panel_sb.x.round();
        let click_y = panel_sb.y.round();
        let _ = ss.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(click_x, click_y),
                modifiers: Modifiers::default(),
            },
            rect,
        );
        let _ = ss.handle_cached(
            &UiEvent::MouseMoved {
                position: Point::new(click_x, click_y + 1.0),
                buttons: ButtonMask {
                    left: true,
                    ..Default::default()
                },
            },
            rect,
        );
        assert!(
            ss.panel_scroll() > 0.0,
            "real-overflow drag must still advance panel_scroll \
             (got {}), panel_sb={panel_sb:?}",
            ss.panel_scroll()
        );
    }

    /// Regression for #241 (second retry): when overflow is small
    /// (≤ 1 cell), the pre-fix painter rounded the panel thumb's
    /// height UP via `ceil(height * height/total)`, filling the whole
    /// gutter — no painted travel possible. The drag handler still
    /// advanced `panel_scroll`, but the user saw nothing move because
    /// the painted thumb stayed pinned to the top.
    ///
    /// Post-fix `compute_panel_thumb_bounds` always reserves at least
    /// `cell_quantum` (1 cell in TUI) of unused track when there's
    /// overflow, so the painted thumb visibly moves under drag.
    /// Locks the invariant: `panel_thumb_bounds.height <= panel_sb.height - cell_quantum`
    /// whenever overflow exists.
    #[test]
    fn tui_panel_thumb_always_reserves_cell_of_travel_under_small_overflow() {
        let mut ss = SidebarSystem::new(vec![
            SidebarSectionDef::new("a", "A"),
            SidebarSectionDef::new("b", "B"),
        ]);
        // 20 + 7 = 27 body rows + 2 headers = 29 rows of content.
        ss.set_rows(0, fake_rows("a", 20));
        ss.set_rows(1, fake_rows("b", 7));
        ss.set_scroll_mode(ScrollMode::WholePanel);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);
        // 28-cell viewport vs 29-cell content = 1 cell of overflow.
        let rect = Rect::new(0.0, 0.0, 30.0, 28.0);

        let (layout, _) = ss.compute_layout(rect, &metrics, 1.0);
        let panel_sb = layout.panel_scrollbar.expect("overflow → panel sb");
        let thumb = layout
            .panel_thumb_bounds
            .expect("panel_scrollbar present → panel_thumb_bounds also present");

        // Pre-fix: layout had no `panel_thumb_bounds`; painter computed
        // `ceil(28 * 28/29) = 28` cells of thumb → thumb_track = 0,
        // painted thumb pinned to top regardless of `panel_scroll`.
        assert!(
            thumb.height <= panel_sb.height - 1.0 + 0.01,
            "panel thumb must leave ≥1 cell of travel for the user to \
             drag visibly; got thumb.height={} vs panel_sb.height={} \
             (overflow=1 cell). Pre-fix the painter's `ceil` rounded \
             the thumb to fill the entire gutter.",
            thumb.height,
            panel_sb.height
        );
        assert!(
            thumb.height >= 1.0,
            "thumb.height = {} (expected ≥1 cell minimum)",
            thumb.height
        );
    }

    /// Regression for #241 (second retry): paint, hit-test, and drag
    /// math must all reference the same panel-thumb rect. Pre-fix
    /// they each re-derived dimensions and disagreed when overflow
    /// was small — the user dragged the thumb a cell, the scroll
    /// state advanced, but the painted thumb stayed pinned at the top
    /// because the painter computed a different (larger) thumb size
    /// than the drag handler did. Post-fix `MultiSectionViewLayout`
    /// owns `panel_thumb_bounds`; painter and drag handler both
    /// consume it verbatim, so a 1-cell drag advances both `panel_scroll`
    /// AND the painted thumb position.
    #[test]
    fn tui_panel_thumb_bounds_paint_hittest_drag_agree() {
        let mut ss = SidebarSystem::new(vec![
            SidebarSectionDef::new("a", "A"),
            SidebarSectionDef::new("b", "B"),
        ]);
        ss.set_rows(0, fake_rows("a", 20));
        ss.set_rows(1, fake_rows("b", 7));
        ss.set_scroll_mode(ScrollMode::WholePanel);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);
        let rect = Rect::new(0.0, 0.0, 30.0, 28.0);

        let (layout_before, _) = ss.compute_layout(rect, &metrics, 1.0);
        let panel_sb = layout_before.panel_scrollbar.expect("overflow → panel sb");
        let thumb_before = layout_before.panel_thumb_bounds.expect("panel thumb");

        // Click the top of the painted thumb.
        let click_x = panel_sb.x.round();
        let click_y = thumb_before.y.round();
        let _ = ss.handle_cached(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(click_x, click_y),
                modifiers: Modifiers::default(),
            },
            rect,
        );
        // Drag 1 cell down.
        let _ = ss.handle_cached(
            &UiEvent::MouseMoved {
                position: Point::new(click_x, click_y + 1.0),
                buttons: ButtonMask {
                    left: true,
                    ..Default::default()
                },
            },
            rect,
        );

        // Scroll state advanced.
        assert!(
            ss.panel_scroll() > 0.0,
            "1-cell drag must advance panel_scroll (got {})",
            ss.panel_scroll()
        );

        // After the drag, recompute layout. The painted thumb position
        // must have moved — same source as paint reads from.
        let (layout_after, _) = ss.compute_layout(rect, &metrics, 1.0);
        let thumb_after = layout_after.panel_thumb_bounds.expect("panel thumb");
        assert!(
            thumb_after.y > thumb_before.y,
            "painted thumb must move down after 1-cell drag — pre-fix \
             the painter's thumb stayed pinned at sb.y because its \
             internal `thumb_track` was 0 cells even though the drag \
             handler computed > 0 travel. before.y={} after.y={}",
            thumb_before.y,
            thumb_after.y
        );
    }

    /// Per-section regression for the same bug class: with 22 rows in
    /// a 20-cell body, the pre-fix `compute_thumb_bounds` quantised
    /// the thumb to fill the whole gutter (`ceil` rounded the raw
    /// 18.18 cells up to 19 then up further with no max-len cap).
    /// Post-fix it always reserves at least 1 cell of travel.
    #[test]
    fn tui_per_section_thumb_reserves_cell_of_travel_under_small_overflow() {
        let mut ss = SidebarSystem::new(vec![SidebarSectionDef::new("t", "T")]);
        ss.set_rows(0, fake_rows("r", 22));
        ss.set_active_section(Some(0));
        ss.set_scroll_mode(ScrollMode::PerSection);

        let metrics = LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        };
        ss.set_backend_info(1.0, metrics);
        let rect = Rect::new(0.0, 0.0, 20.0, 21.0);

        let (layout, _) = ss.compute_layout(rect, &metrics, 1.0);
        let sb = layout.sections[0]
            .scrollbar_bounds
            .expect("overflow → per-section sb");
        let thumb = layout.sections[0]
            .thumb_bounds
            .expect("Tree body → per-section thumb");

        assert!(
            thumb.height <= sb.height - 1.0 + 0.01,
            "per-section thumb must leave ≥1 cell of travel; got \
             thumb.height={} vs sb.height={}",
            thumb.height,
            sb.height
        );
    }

    // ── Header row click precision (#110) ──────────────────────────────

    #[test]
    fn header_row_bottom_pixel_hits_header_not_child() {
        let lh: f32 = 16.0;
        let header_h = (lh * 1.2).round(); // 19.0

        let tree = TreeView {
            id: WidgetId::new("t"),
            rows: vec![
                TreeRow {
                    path: vec![0],
                    indent: 0,
                    icon: None,
                    text: StyledText::plain("src/"),
                    badge: None,
                    is_expanded: Some(true),
                    decoration: Decoration::Header,
                    edit: None,
                },
                TreeRow {
                    path: vec![0, 0],
                    indent: 1,
                    icon: None,
                    text: StyledText::plain("main.rs"),
                    badge: None,
                    is_expanded: None,
                    decoration: Decoration::Normal,
                    edit: None,
                },
            ],
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: Default::default(),
            has_focus: true,
        };

        let body_b = Rect::new(0.0, 0.0, 200.0, 200.0);
        let ss = SidebarSystem::new(vec![SidebarSectionDef::new("t", "T")]);
        let layout = ss.compute_tree_layout(body_b, &tree, lh);

        // Header row spans [0, header_h). Click at header_h - 0.5 (bottom pixel)
        // should still land on the header (row 0), not the child (row 1).
        let bottom_of_header = header_h - 0.5;
        match layout.hit_test(5.0, bottom_of_header) {
            TreeViewHit::Row(idx) => assert_eq!(
                idx, 0,
                "click at y={bottom_of_header} (bottom of header row) hit row {idx}, expected 0"
            ),
            other => panic!(
                "click at y={bottom_of_header} returned {:?}, expected Row(0)",
                other
            ),
        }

        // First pixel of child row should hit row 1.
        let top_of_child = header_h + 0.5;
        match layout.hit_test(5.0, top_of_child) {
            TreeViewHit::Row(idx) => assert_eq!(
                idx, 1,
                "click at y={top_of_child} (top of child row) hit row {idx}, expected 1"
            ),
            other => panic!(
                "click at y={top_of_child} returned {:?}, expected Row(1)",
                other
            ),
        }
    }

    // ── Form item-level click dispatch (#112) ──────────────────────────

    #[test]
    fn form_field_measure_populates_toggle_group_items() {
        use crate::primitives::form::{FormField, ToggleGroupItem};
        let field = FormField {
            id: WidgetId::new("flags"),
            label: StyledText::plain("Flags"),
            kind: FieldKind::ToggleGroup {
                toggles: vec![
                    ToggleGroupItem {
                        id: WidgetId::new("case"),
                        label: "Aa".into(),
                        value: false,
                    },
                    ToggleGroupItem {
                        id: WidgetId::new("regex"),
                        label: ".*".into(),
                        value: false,
                    },
                ],
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        };
        let m = form_field_measure(&field, 20.0, 10.0);
        assert_eq!(m.item_measures.len(), 2);
        assert_eq!(m.item_measures[0].id, WidgetId::new("case"));
        assert_eq!(m.item_measures[1].id, WidgetId::new("regex"));
        assert!(m.items_start_x > 0.0);
    }

    #[test]
    fn form_field_measure_populates_button_row_items() {
        use crate::primitives::form::{ButtonRowItem, FormField};
        let field = FormField {
            id: WidgetId::new("actions"),
            label: StyledText::plain(""),
            kind: FieldKind::ButtonRow {
                buttons: vec![
                    ButtonRowItem {
                        id: WidgetId::new("replace"),
                        label: "Replace".into(),
                        disabled: false,
                        icon: None,
                    },
                    ButtonRowItem {
                        id: WidgetId::new("replace-all"),
                        label: "Replace All".into(),
                        disabled: false,
                        icon: None,
                    },
                ],
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        };
        let m = form_field_measure(&field, 20.0, 10.0);
        assert_eq!(m.item_measures.len(), 2);
        assert_eq!(m.item_measures[0].id, WidgetId::new("replace"));
        assert_eq!(m.item_measures[1].id, WidgetId::new("replace-all"));
    }

    #[test]
    fn form_layout_hit_test_returns_individual_toggle_id() {
        use crate::primitives::form::{FormField, FormHit, ToggleGroupItem};
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("flags"),
                label: StyledText::plain(""),
                kind: FieldKind::ToggleGroup {
                    toggles: vec![
                        ToggleGroupItem {
                            id: WidgetId::new("case"),
                            label: "Aa".into(),
                            value: false,
                        },
                        ToggleGroupItem {
                            id: WidgetId::new("regex"),
                            label: ".*".into(),
                            value: false,
                        },
                    ],
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let lh: f32 = 16.0;
        let row_h = (lh * 1.4).round();
        let char_w = lh * 0.6;
        let layout = form.layout(200.0, 100.0, |i| {
            form_field_measure(&form.fields[i], row_h, char_w)
        });
        let m = &layout.visible_fields[0];
        assert!(
            !m.item_bounds.is_empty(),
            "item_bounds should be populated for ToggleGroup"
        );
        let (first_id, first_rect) = &m.item_bounds[0];
        assert_eq!(first_id, &WidgetId::new("case"));
        let hit = layout.hit_test(first_rect.x + 1.0, first_rect.y + 1.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("case")));
    }

    #[test]
    fn form_click_event_dispatches_individual_toggle() {
        use crate::primitives::form::{FormField, ToggleGroupItem};
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("flags"),
                label: StyledText::plain(""),
                kind: FieldKind::ToggleGroup {
                    toggles: vec![ToggleGroupItem {
                        id: WidgetId::new("case"),
                        label: "Aa".into(),
                        value: true,
                    }],
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let ev = form_click_event(&form, &WidgetId::new("case"));
        assert_eq!(
            ev,
            FormEvent::ToggleChanged {
                id: WidgetId::new("case"),
                value: false,
            }
        );
    }

    // ── Header click-to-collapse tests ──────────────────────────────────

    /// Rect, line-height, and metrics used by all header-click tests.
    fn collapse_click_setup() -> (Rect, f32, LayoutMetrics) {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 600.0,
        };
        let lh = 20.0;
        // header_size matches lh so section-0 header occupies y ∈ [0, 20).
        let metrics = LayoutMetrics {
            header_size: 20.0,
            divider_size: 0.0,
            scrollbar_size: 0.0,
            cell_quantum: 0.0,
        };
        (rect, lh, metrics)
    }

    #[test]
    fn header_click_toggles_collapsed_when_allow_collapse_true() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_allow_collapse(true);
        ss.set_rows(0, fake_rows("v", 3));
        let (rect, lh, metrics) = collapse_click_setup();
        assert!(!ss.is_collapsed(0));
        // Click centre of section-0 header.
        ss.click(rect, 5.0, 10.0, lh, &metrics, None);
        assert!(ss.is_collapsed(0), "first click should collapse section 0");
    }

    #[test]
    fn header_click_does_not_toggle_when_allow_collapse_false() {
        let mut ss = SidebarSystem::new(sample_defs());
        // allow_collapse is false by default — make it explicit.
        ss.set_allow_collapse(false);
        ss.set_rows(0, fake_rows("v", 3));
        let (rect, lh, metrics) = collapse_click_setup();
        ss.click(rect, 5.0, 10.0, lh, &metrics, None);
        assert!(
            !ss.is_collapsed(0),
            "click without allow_collapse must not toggle collapsed state"
        );
    }

    #[test]
    fn header_click_emits_header_activated_after_toggle() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_allow_collapse(true);
        ss.set_rows(0, fake_rows("v", 3));
        let (rect, lh, metrics) = collapse_click_setup();
        let ev = ss.click(rect, 5.0, 10.0, lh, &metrics, None);
        assert_eq!(
            ev,
            SidebarEvent::HeaderActivated { section: 0 },
            "HeaderActivated must still be emitted even when allow_collapse toggles state"
        );
    }

    #[test]
    fn header_click_multiple_toggles_persist_state() {
        let mut ss = SidebarSystem::new(sample_defs());
        ss.set_allow_collapse(true);
        ss.set_rows(0, fake_rows("v", 3));
        let (rect, lh, metrics) = collapse_click_setup();
        // false → true
        ss.click(rect, 5.0, 10.0, lh, &metrics, None);
        assert!(ss.is_collapsed(0), "after 1st click: should be collapsed");
        // true → false
        ss.click(rect, 5.0, 10.0, lh, &metrics, None);
        assert!(!ss.is_collapsed(0), "after 2nd click: should be expanded");
        // false → true again
        ss.click(rect, 5.0, 10.0, lh, &metrics, None);
        assert!(
            ss.is_collapsed(0),
            "after 3rd click: should be collapsed again"
        );
    }

    // ── Scroll-wheel position routing (#240) ───────────────────────────────
    //
    // Helpers shared by the scroll routing tests below.

    fn scroll_routing_sidebar() -> SidebarSystem {
        // Two sections, each with 50 rows so there's plenty of room to
        // scroll (viewport ≈ 19 rows → max_scroll ≈ 31).
        let defs = vec![
            SidebarSectionDef::new("top", "TOP"),
            SidebarSectionDef::new("bottom", "BOTTOM"),
        ];
        let mut ss = SidebarSystem::new(defs);
        ss.set_rows(0, fake_rows("t", 50));
        ss.set_rows(1, fake_rows("b", 50));
        // Start with section 0 active — this is the "wrong" section for
        // the scroll-routing tests that hover over section 1.
        ss.set_active_section(Some(0));
        // Provide backend info so handle_cached works.
        // lh=1.0, default LayoutMetrics (header_size=1.0, scrollbar_size=1.0).
        ss.set_backend_info(1.0, LayoutMetrics::default());
        ss
    }

    /// Compute which y-coordinate lies inside the body of a given section
    /// for a known rect and layout metrics, so we can construct correct
    /// scroll-event positions for the routing tests.
    fn section_body_y(ss: &SidebarSystem, section_idx: usize, rect: Rect) -> f32 {
        let lh = 1.0_f32;
        let metrics = LayoutMetrics::default();
        let (layout, map) = ss.compute_layout(rect, &metrics, lh);
        let msv_idx = map.iter().position(|&s| s == section_idx).unwrap();
        let body_b = layout.sections[msv_idx].body_bounds;
        // Return a y-coordinate in the middle of the body.
        body_b.y + body_b.height / 2.0
    }

    #[test]
    fn scroll_routes_to_section_under_cursor_not_active_section() {
        // Section 0 is active, cursor is over section 1 body.
        // Scroll down should advance section 1's offset, not section 0's.
        let mut ss = scroll_routing_sidebar();
        let rect = Rect::new(0.0, 0.0, 20.0, 40.0);
        let y1 = section_body_y(&ss, 1, rect);

        let ev = UiEvent::Scroll {
            widget: None,
            delta: crate::ScrollDelta::new(0.0, -1.0), // negative y → scroll down
            position: Point::new(5.0, y1),
        };
        let result = ss.handle_cached(&ev, rect);
        assert_eq!(result, SidebarEvent::Consumed);

        // Section 1 should have scrolled; section 0 should not have moved.
        assert_eq!(ss.scroll_offset(0), 0, "active section 0 should not scroll");
        assert!(
            ss.scroll_offset(1) > 0,
            "section 1 under cursor should have scrolled, offset={}",
            ss.scroll_offset(1)
        );
    }

    #[test]
    fn scroll_routes_to_section_0_when_cursor_over_it() {
        // Cursor is over section 0. Even though section 1 is active,
        // section 0 should scroll.
        let defs = vec![
            SidebarSectionDef::new("top", "TOP"),
            SidebarSectionDef::new("bottom", "BOTTOM"),
        ];
        let mut ss = SidebarSystem::new(defs);
        ss.set_rows(0, fake_rows("t", 20));
        ss.set_rows(1, fake_rows("b", 20));
        ss.set_active_section(Some(1)); // section 1 is active
        ss.set_backend_info(1.0, LayoutMetrics::default());

        let rect = Rect::new(0.0, 0.0, 20.0, 40.0);
        let y0 = section_body_y(&ss, 0, rect);

        let ev = UiEvent::Scroll {
            widget: None,
            delta: crate::ScrollDelta::new(0.0, -1.0),
            position: Point::new(5.0, y0),
        };
        ss.handle_cached(&ev, rect);

        assert!(
            ss.scroll_offset(0) > 0,
            "section 0 under cursor should scroll"
        );
        assert_eq!(ss.scroll_offset(1), 0, "active section 1 should not scroll");
    }

    #[test]
    fn scroll_falls_back_to_active_section_when_position_outside_msv() {
        // Position (0, 0) lies outside the MSV bounds (rect starts at 10, 10).
        // The fallback path kicks in and scrolls the active section.
        let mut ss = scroll_routing_sidebar();
        let rect = Rect::new(10.0, 10.0, 20.0, 40.0);
        ss.set_active_section(Some(0));

        let ev = UiEvent::Scroll {
            widget: None,
            delta: crate::ScrollDelta::new(0.0, -1.0),
            position: Point::new(0.0, 0.0), // outside rect
        };
        ss.handle_cached(&ev, rect);
        // Active section 0 receives the scroll via the fallback path.
        assert!(
            ss.scroll_offset(0) > 0,
            "fallback should scroll active section 0"
        );
        assert_eq!(ss.scroll_offset(1), 0);
    }

    #[test]
    fn scroll_up_decrements_offset_in_section_under_cursor() {
        let mut ss = scroll_routing_sidebar();
        let rect = Rect::new(0.0, 0.0, 20.0, 40.0);
        let y1 = section_body_y(&ss, 1, rect);

        // First scroll section 1 down by 5.
        if let SectionController::Tree(tc) = &mut ss.sections[1] {
            tc.set_scroll_offset(5);
        }

        // Now scroll up with cursor over section 1.
        let ev = UiEvent::Scroll {
            widget: None,
            delta: crate::ScrollDelta::new(0.0, 1.0), // positive y → scroll up
            position: Point::new(5.0, y1),
        };
        ss.handle_cached(&ev, rect);
        assert_eq!(ss.scroll_offset(1), 4, "scroll up should decrement offset");
        assert_eq!(ss.scroll_offset(0), 0, "section 0 should be untouched");
    }

    #[cfg(feature = "gtk")]
    #[test]
    fn scroll_event_carries_cursor_position_via_gdk_scroll_to_uievent() {
        // Unit-test that `gdk_scroll_to_uievent` populates the position field
        // correctly. This is the translation function called by the fixed GTK
        // runner and `wire_da_events` — the real integration depends on the
        // shared cursor_pos state, but we can verify the helper signature here.
        use crate::gtk::events::gdk_scroll_to_uievent;
        let ev = gdk_scroll_to_uievent(0.0, 1.0, 42.0, 100.0);
        match ev {
            UiEvent::Scroll {
                position, delta, ..
            } => {
                assert_eq!(position.x, 42.0, "x from cursor should propagate");
                assert_eq!(position.y, 100.0, "y from cursor should propagate");
                // Negative dy convention is still applied (GTK positive = down).
                assert_eq!(delta.y, -1.0);
            }
            _ => panic!("expected Scroll event"),
        }
    }
}
