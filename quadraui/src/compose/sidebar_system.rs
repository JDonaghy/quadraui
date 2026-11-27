//! The sidebar: a stack of collapsible sections, each a tree or a form,
//! as one composite widget ([`Sidebar`]) and its controller
//! ([`SidebarSystem`]).
//!
//! [`Sidebar`] is a controlled descriptor: section collapse, visibility,
//! badges, the active section, each tree's rows, selection and scroll
//! offset, and the panel scroll are its fields, and every change a
//! gesture asks for comes back as a [`SidebarAction`] the app writes
//! onto them ([`Sidebar::apply`] does that). It is laid out and painted
//! by [`MultiSectionView`]'s own `Primitive`; each section body is a
//! [`TreeView`] or [`Form`] painted through its own `Primitive`, and
//! every event that lands on a body goes to that body's own
//! `Interactive`. [`SidebarState`] holds only the view's gesture in
//! flight and each body widget's own state.
//!
//! [`SidebarSystem`] holds a [`Sidebar`] plus its [`SidebarState`],
//! applies every action itself, and answers [`SidebarEvent`]s.
//!
//! Two navigation modes ([`Sidebar::navigation_mode`]):
//! - [`NavigationMode::Scroll`] (default): Up/Down scroll the active
//!   section (or the panel, in [`ScrollMode::WholePanel`]), Enter selects
//!   its first row and Space expands or collapses the selected branch.
//! - [`NavigationMode::Selection`]: keys go to the active section's tree,
//!   which moves its selection with scroll-to-follow, activates on Enter
//!   and toggles a branch on Space.
//!
//! Tab and Shift+Tab move the active section through the visible ones.
//!
//! Scroll-wheel events follow the [`ScrollDelta`](crate::ScrollDelta)
//! sign convention: positive `delta.y` = scroll content up (decrease
//! offset). Backends normalise their native direction before emitting.

use crate::compose::Handled;
use crate::interactive::WidgetAction;
use crate::primitive::LayoutCtx;
use crate::primitives::form::{Form, FormEvent};
use crate::{
    Backend, MsvAxis, MultiSectionView, Point, Rect, ScrollMode, Section, SectionBody,
    SectionHeader, SectionSize, SelectionMode, StyledText, TreePath, TreeRow, TreeRowEditState,
    TreeView, UiEvent, WidgetId,
};
use serde::{Deserialize, Serialize};

/// How Up/Down keys behave in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NavigationMode {
    /// Up/Down scroll the viewport.
    #[default]
    Scroll,
    /// Up/Down move the selection to the next/previous row; the
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

// ── Descriptor ─────────────────────────────────────────────────────────────────

/// A sidebar section's body.
#[derive(Debug, Clone)]
pub enum SidebarBody {
    /// A tree. Its `has_focus` is derived from the sidebar's active
    /// section when the sidebar is laid out.
    Tree(TreeView),
    /// A form.
    Form(Form),
}

/// One section of a [`Sidebar`].
#[derive(Debug, Clone)]
pub struct SidebarSection {
    pub id: String,
    pub title: String,
    pub show_chevron: bool,
    pub size: SectionSize,
    pub body: SidebarBody,
    pub collapsed: bool,
    /// A hidden section is left out of the view entirely.
    pub visible: bool,
    pub badge: Option<StyledText>,
    /// For a tree: whether a right-click on the empty space below its
    /// rows requests a context menu, with an empty `path`.
    pub empty_space_context_menu: bool,
    /// For a form: paint it focused even while another section is the
    /// active one.
    pub form_focused: bool,
}

impl SidebarSection {
    /// A visible, expanded section from `def`, with `body`.
    pub fn new(def: &SidebarSectionDef, body: SidebarBody) -> Self {
        Self {
            id: def.id.clone(),
            title: def.title.clone(),
            show_chevron: def.show_chevron,
            size: def.size,
            body,
            collapsed: false,
            visible: true,
            badge: None,
            empty_space_context_menu: false,
            form_focused: false,
        }
    }

    /// The section's tree, if it hosts one.
    pub fn tree(&self) -> Option<&TreeView> {
        match &self.body {
            SidebarBody::Tree(t) => Some(t),
            SidebarBody::Form(_) => None,
        }
    }

    /// The section's tree, mutably, if it hosts one.
    pub fn tree_mut(&mut self) -> Option<&mut TreeView> {
        match &mut self.body {
            SidebarBody::Tree(t) => Some(t),
            SidebarBody::Form(_) => None,
        }
    }

    /// The section's form, if it hosts one.
    pub fn form(&self) -> Option<&Form> {
        match &self.body {
            SidebarBody::Form(f) => Some(f),
            SidebarBody::Tree(_) => None,
        }
    }
}

/// A multi-section sidebar panel. See the module doc.
#[derive(Debug, Clone)]
pub struct Sidebar {
    pub id: WidgetId,
    pub sections: Vec<SidebarSection>,
    /// The section keyboard navigation addresses; its title carries the
    /// active marker.
    pub active_section: Option<usize>,
    /// Whether the sidebar holds keyboard focus: the active section's
    /// tree paints focused only while it does.
    pub has_focus: bool,
    /// Whether a header click collapses or expands its section.
    pub allow_collapse: bool,
    pub navigation_mode: NavigationMode,
    pub scroll_mode: ScrollMode,
    /// The panel scroll offset in [`ScrollMode::WholePanel`], in
    /// main-axis units.
    pub panel_scroll: f32,
}

/// What a [`Sidebar`] reports.
///
/// Row, form and edit events name the section they came from by its
/// index in [`Sidebar::sections`]. The rest are controlled-state
/// write-backs for the app to store ([`Sidebar::apply`] stores them all).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SidebarAction {
    /// A header was clicked, or the empty space of a section body: the
    /// section becomes the active one, with no row selected.
    HeaderActivated { section: usize },
    /// Tab or Shift+Tab moved the active section.
    ActiveSectionChanged { section: Option<usize> },
    /// A header click collapsed or expanded its section.
    CollapseChanged { section: usize, collapsed: bool },
    /// A tree row was clicked or selected from the keyboard: the section
    /// becomes active with `path` selected.
    RowSelected { section: usize, path: TreePath },
    /// A row was double-clicked, or Enter pressed on the selected row.
    RowActivated { section: usize, path: TreePath },
    /// A branch row's chevron was clicked, or Space (or Left/Right)
    /// pressed on it. The app toggles its expand state and pushes fresh
    /// rows.
    RowToggleExpand { section: usize, path: TreePath },
    /// A right-click on a row — or, with
    /// [`SidebarSection::empty_space_context_menu`] set, on the empty
    /// space below a tree's rows, with an empty `path`. `position` is the
    /// pointer position, for placing the menu.
    ContextMenuRequested {
        section: usize,
        path: TreePath,
        position: Point,
    },
    /// A tree section scrolled to row `offset`.
    ScrollChanged { section: usize, offset: usize },
    /// The panel scrolled to `offset` ([`ScrollMode::WholePanel`]).
    PanelScrolled { offset: f32 },
    /// A keystroke changed the row being renamed: its new text, caret
    /// and selection anchor (byte offsets).
    EditChanged {
        section: usize,
        path: TreePath,
        text: String,
        cursor: usize,
        selection_anchor: Option<usize>,
    },
    /// Enter confirmed a rename.
    EditConfirmed {
        section: usize,
        path: TreePath,
        new_text: String,
    },
    /// Escape cancelled a rename.
    EditCancelled { section: usize, path: TreePath },
    /// A form section's form reported `event`.
    Form { section: usize, event: FormEvent },
}

impl WidgetAction for SidebarAction {
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::Sidebar(id, self)
    }
}

impl Sidebar {
    /// An empty sidebar with id `id`: no sections, no active section,
    /// focused, per-section scrolling.
    pub fn new(id: impl Into<WidgetId>) -> Self {
        Self {
            id: id.into(),
            sections: Vec::new(),
            active_section: None,
            has_focus: true,
            allow_collapse: false,
            navigation_mode: NavigationMode::default(),
            scroll_mode: ScrollMode::PerSection,
            panel_scroll: 0.0,
        }
    }

    /// Store an action's new field values.
    pub fn apply(&mut self, action: &SidebarAction) {
        match action {
            SidebarAction::HeaderActivated { section } => {
                self.active_section = Some(*section);
                if let Some(tree) = self.tree_mut(*section) {
                    tree.selected_path = None;
                }
            }
            SidebarAction::ActiveSectionChanged { section } => self.active_section = *section,
            SidebarAction::CollapseChanged { section, collapsed } => {
                if let Some(s) = self.sections.get_mut(*section) {
                    s.collapsed = *collapsed;
                }
            }
            SidebarAction::RowSelected { section, path }
            | SidebarAction::RowActivated { section, path }
            | SidebarAction::RowToggleExpand { section, path } => {
                self.active_section = Some(*section);
                if let Some(tree) = self.tree_mut(*section) {
                    tree.selected_path = Some(path.clone());
                }
            }
            SidebarAction::ContextMenuRequested { section, path, .. } => {
                self.active_section = Some(*section);
                if path.is_empty() {
                    return;
                }
                if let Some(tree) = self.tree_mut(*section) {
                    tree.selected_path = Some(path.clone());
                }
            }
            SidebarAction::ScrollChanged { section, offset } => {
                if let Some(tree) = self.tree_mut(*section) {
                    tree.scroll_offset = *offset;
                }
            }
            SidebarAction::PanelScrolled { offset } => self.panel_scroll = offset.max(0.0),
            SidebarAction::EditChanged {
                section,
                path,
                text,
                cursor,
                selection_anchor,
            } => {
                let row = self
                    .tree_mut(*section)
                    .and_then(|t| t.rows.iter_mut().find(|r| &r.path == path));
                if let Some(edit) = row.and_then(|r| r.edit.as_mut()) {
                    edit.text = text.clone();
                    edit.cursor = *cursor;
                    edit.selection_anchor = *selection_anchor;
                }
            }
            SidebarAction::EditConfirmed { section, path, .. }
            | SidebarAction::EditCancelled { section, path } => {
                let row = self
                    .tree_mut(*section)
                    .and_then(|t| t.rows.iter_mut().find(|r| &r.path == path));
                if let Some(row) = row {
                    row.edit = None;
                }
            }
            SidebarAction::Form { section, .. } => self.active_section = Some(*section),
        }
    }

    /// Section `section`'s tree, mutably.
    pub fn tree_mut(&mut self, section: usize) -> Option<&mut TreeView> {
        self.sections.get_mut(section).and_then(|s| s.tree_mut())
    }

    /// Section `section`'s tree.
    pub fn tree(&self, section: usize) -> Option<&TreeView> {
        self.sections.get(section).and_then(|s| s.tree())
    }

    /// The [`MultiSectionView`] this sidebar lays out and paints as,
    /// under the sidebar's own id, with `marker` before the active
    /// section's title, plus each view section's index in
    /// [`Self::sections`].
    pub fn view(&self, marker: &str) -> (MultiSectionView, Vec<usize>) {
        let mut map = Vec::new();
        let mut sections = Vec::new();
        for (idx, s) in self.sections.iter().enumerate() {
            if !s.visible {
                continue;
            }
            let active = self.active_section == Some(idx);
            let title = if active {
                format!("{marker} {}", s.title)
            } else {
                s.title.clone()
            };
            let body = match &s.body {
                SidebarBody::Tree(t) => SectionBody::Tree(TreeView {
                    has_focus: active && self.has_focus,
                    ..t.clone()
                }),
                SidebarBody::Form(f) => SectionBody::Form(Form {
                    has_focus: s.form_focused || (active && self.has_focus),
                    ..f.clone()
                }),
            };
            sections.push(Section {
                id: s.id.clone(),
                header: SectionHeader {
                    title: StyledText::plain(title),
                    show_chevron: s.show_chevron,
                    badge: s.badge.clone(),
                    ..Default::default()
                },
                body,
                aux: None,
                size: s.size,
                collapsed: s.collapsed,
                min_size: None,
                max_size: None,
            });
            map.push(idx);
        }
        let active_section = self
            .active_section
            .and_then(|a| map.iter().position(|&s| s == a));
        let view = MultiSectionView {
            id: self.id,
            sections,
            active_section,
            axis: MsvAxis::Vertical,
            allow_resize: false,
            allow_collapse: self.allow_collapse,
            scroll_mode: self.scroll_mode,
            has_focus: self.has_focus,
            panel_scroll: self.panel_scroll,
        };
        (view, map)
    }
}

/// The widget contract and the controller, on the builds that have the
/// `MultiSectionView` contract they are laid out through.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
mod contract {
    use super::super::widget_host::{self, ChildHost};
    use super::*;
    use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
    use crate::behaviour::nav::TreeNavCore;
    use crate::event::Key;
    use crate::interactive::{
        EventCx, FocusSpec, FocusStop, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    };
    use crate::primitive::{PaintEnv, Primitive};
    use crate::primitives::multi_section_view::{
        MultiSectionViewEvent, MultiSectionViewPrimitiveLayout, MultiSectionViewSubPart,
        SectionBodyLayout,
    };
    use crate::primitives::scrollbar::wheel_row_delta;
    use crate::primitives::tree::TreeEvent;
    use crate::{MouseButton, NamedKey};

    /// The active-section marker a cell grid paints before the title.
    const CELL_ACTIVE_MARKER: &str = "▶";
    /// The codicon chevron a pixel surface paints instead; every GUI backend
    /// registers the codicon font.
    const PIXEL_ACTIVE_MARKER: &str = "\u{eab6}";

    impl Sidebar {
        /// The view's section index for sidebar section `section`, if it is
        /// visible.
        fn view_index(&self, section: usize) -> Option<usize> {
            let visible = self.sections.get(section)?.visible;
            visible.then(|| {
                self.sections[..section]
                    .iter()
                    .filter(|s| s.visible)
                    .count()
            })
        }

        /// The next visible section from the active one, `delta` steps on,
        /// wrapping.
        fn cycled_section(&self, delta: isize) -> Option<usize> {
            let n = self.sections.len();
            if n == 0 {
                return None;
            }
            let mut idx = match self.active_section {
                Some(a) => a as isize,
                None if delta >= 0 => -1,
                None => n as isize,
            };
            for _ in 0..n {
                idx = (idx + delta).rem_euclid(n as isize);
                if self.sections[idx as usize].visible {
                    return Some(idx as usize);
                }
            }
            None
        }
    }

    // ── Layout and state ───────────────────────────────────────────────────────────

    /// [`Primitive::Layout`] for [`Sidebar`]: the [`MultiSectionView`] the
    /// sidebar is laid out as, its layout, and each view section's index in
    /// [`Sidebar::sections`].
    #[derive(Debug, Clone)]
    pub struct SidebarLayout {
        /// The view, with the active marker the layout's surface paints.
        pub view: MultiSectionView,
        /// The view's layout.
        pub msv: MultiSectionViewPrimitiveLayout,
        /// `map[i]` is view section `i`'s index in [`Sidebar::sections`].
        pub map: Vec<usize>,
    }

    impl SidebarLayout {
        /// Sidebar section `section`'s view index.
        pub(crate) fn view_index(&self, section: usize) -> Option<usize> {
            self.map.iter().position(|&s| s == section)
        }

        /// How many rows of sidebar section `section`'s tree fit its body.
        pub fn viewport_rows(&self, section: usize) -> usize {
            let body = self.view_index(section).and_then(|i| self.msv.body(i));
            match body.map(|b| &b.layout) {
                Some(SectionBodyLayout::Tree(l)) => l.viewport_rows,
                _ => 0,
            }
        }
    }

    // ── Primitive ──────────────────────────────────────────────────────────────────

    impl A11yPrimitive for Sidebar {
        fn a11y_info(&self) -> A11yInfo {
            self.view(CELL_ACTIVE_MARKER).0.a11y_info()
        }

        fn a11y_children(&self) -> Vec<A11yChild> {
            self.view(CELL_ACTIVE_MARKER).0.a11y_children()
        }
    }

    impl Sidebar {
        /// `env` with this sidebar's view state in place of its own.
        fn view_env<'a>(&self, env: &PaintEnv<'a>) -> PaintEnv<'a> {
            let msv = env
                .widget_state_as::<SidebarState>()
                .map(|s| &s.msv as &dyn std::any::Any);
            env.with_widget_state(msv)
        }
    }

    impl Primitive for Sidebar {
        type Layout = SidebarLayout;
        type Event = SidebarAction;

        fn id(&self) -> Option<&WidgetId> {
            Some(&self.id)
        }

        fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> SidebarLayout {
            let marker = match ctx.units() {
                crate::primitive::Units::Cells => CELL_ACTIVE_MARKER,
                _ => PIXEL_ACTIVE_MARKER,
            };
            let (view, map) = self.view(marker);
            let msv = Primitive::layout(&view, ctx, rect);
            SidebarLayout { view, msv, map }
        }

        fn paint(
            &self,
            layout: &SidebarLayout,
            surface: &mut dyn crate::paint_surface::PaintSurface,
            env: &PaintEnv,
        ) {
            Primitive::paint(&layout.view, &layout.msv, surface, &self.view_env(env));
        }

        #[cfg(feature = "tui")]
        fn paint_cells(
            &self,
            layout: &SidebarLayout,
            cells: &mut dyn crate::CellSurface,
            env: &PaintEnv,
        ) {
            Primitive::paint_cells(&layout.view, &layout.msv, cells, &self.view_env(env));
        }
    }

    // ── Interactive ────────────────────────────────────────────────────────────────

    impl Sidebar {
        /// The sidebar action a view event means, `None` for one with no
        /// sidebar meaning.
        fn map_view_event(
            &self,
            event: MultiSectionViewEvent,
            map: &[usize],
        ) -> Option<SidebarAction> {
            let section = |i: usize| map.get(i).copied();
            match event {
                MultiSectionViewEvent::HeaderActivated { section: i, .. } => {
                    Some(SidebarAction::HeaderActivated {
                        section: section(i)?,
                    })
                }
                MultiSectionViewEvent::CollapseChanged {
                    section: i,
                    collapsed,
                } => Some(SidebarAction::CollapseChanged {
                    section: section(i)?,
                    collapsed,
                }),
                MultiSectionViewEvent::ScrollChanged {
                    section: Some(i),
                    offset,
                } => Some(SidebarAction::ScrollChanged {
                    section: section(i)?,
                    offset: offset.max(0.0).round() as usize,
                }),
                MultiSectionViewEvent::ScrollChanged {
                    section: None,
                    offset,
                } => Some(SidebarAction::PanelScrolled { offset }),
                MultiSectionViewEvent::Body(i, event) => self.map_body_event(section(i)?, *event),
                _ => None,
            }
        }

        /// The sidebar action section `section`'s body widget event means.
        fn map_body_event(&self, section: usize, event: UiEvent) -> Option<SidebarAction> {
            match event {
                UiEvent::Tree(_, event) => self.map_tree_event(section, event),
                UiEvent::Form(_, event) => Some(SidebarAction::Form { section, event }),
                _ => None,
            }
        }

        fn map_tree_event(&self, section: usize, event: TreeEvent) -> Option<SidebarAction> {
            Some(match event {
                TreeEvent::RowClicked { path, .. } | TreeEvent::SelectionChanged { path } => {
                    SidebarAction::RowSelected { section, path }
                }
                TreeEvent::RowDoubleClicked { path } | TreeEvent::RowActivated { path } => {
                    SidebarAction::RowActivated { section, path }
                }
                TreeEvent::RowToggleExpand { path } => {
                    SidebarAction::RowToggleExpand { section, path }
                }
                TreeEvent::ScrollChanged { scroll_offset } => SidebarAction::ScrollChanged {
                    section,
                    offset: scroll_offset,
                },
                TreeEvent::ContextMenuRequested { path, position } => {
                    SidebarAction::ContextMenuRequested {
                        section,
                        path,
                        position,
                    }
                }
                TreeEvent::RowRenameChanged {
                    path,
                    text,
                    cursor,
                    selection_anchor,
                } => SidebarAction::EditChanged {
                    section,
                    path,
                    text,
                    cursor,
                    selection_anchor,
                },
                TreeEvent::RowRenameCommitted { path, text } => SidebarAction::EditConfirmed {
                    section,
                    path,
                    new_text: text,
                },
                TreeEvent::RowRenameCancelled { path } => {
                    SidebarAction::EditCancelled { section, path }
                }
                _ => return None,
            })
        }

        /// The sidebar section an emitted body event's widget id belongs to.
        fn section_of(&self, id: &WidgetId) -> Option<usize> {
            self.sections.iter().position(|s| match &s.body {
                SidebarBody::Tree(t) => t.id == *id,
                SidebarBody::Form(f) => f.id == *id,
            })
        }

        /// Re-report what the view and its bodies emitted while handling an
        /// event as sidebar actions; anything else passes through.
        fn forward_emitted(&self, emitted: Vec<UiEvent>, map: &[usize], cx: &mut EventCx) {
            for event in emitted {
                let action = match event {
                    UiEvent::MultiSectionView(id, ev) if id == self.id => {
                        self.map_view_event(ev, map)
                    }
                    UiEvent::Tree(id, ev) => match self.section_of(&id) {
                        Some(section) => self.map_tree_event(section, ev),
                        None => {
                            cx.emit(UiEvent::Tree(id, ev));
                            continue;
                        }
                    },
                    UiEvent::Form(id, ev) => match self.section_of(&id) {
                        Some(section) => Some(SidebarAction::Form { section, event: ev }),
                        None => {
                            cx.emit(UiEvent::Form(id, ev));
                            continue;
                        }
                    },
                    other => {
                        cx.emit(other);
                        continue;
                    }
                };
                if let Some(action) = action {
                    cx.emit(action.into_ui_event(self.id));
                }
            }
        }

        /// Run the view's own `handle`, mapping what it answers and emits.
        fn handle_view(
            &self,
            state: &mut SidebarState,
            event: &RoutedEvent,
            layout: &SidebarLayout,
            cx: &mut EventCx,
        ) -> Handled<SidebarAction> {
            let (handled, emitted) = {
                let mut host = ChildHost::same(cx).collecting();
                let handled = {
                    let mut view_cx = EventCx::new(self.id, &mut host);
                    Interactive::handle(
                        &layout.view,
                        &mut state.msv,
                        event,
                        &layout.msv,
                        &mut view_cx,
                    )
                };
                (handled, host.take_collected())
            };
            self.forward_emitted(emitted, &layout.map, cx);
            match handled {
                Handled::Event(event) => match self.map_view_event(event, &layout.map) {
                    Some(action) => Handled::Event(action),
                    None => Handled::Consumed,
                },
                Handled::Consumed => Handled::Consumed,
                Handled::Ignored => Handled::Ignored,
            }
        }

        /// The view section whose body the sidebar-local point `local` is
        /// on, and whether it is on one of that body's own parts (a row or
        /// a field) rather than its empty space.
        fn body_hit(&self, layout: &SidebarLayout, local: Point) -> Option<(usize, bool)> {
            let mut body = None;
            for (rect, sub) in Interactive::sub_regions(&layout.view, &layout.msv) {
                if !rect.contains(local) {
                    continue;
                }
                if let Some(MultiSectionViewSubPart::Body(i, child)) =
                    MultiSectionViewSubPart::from_sub_target(sub)
                {
                    if child.is_some() {
                        return Some((i, true));
                    }
                    body = body.or(Some((i, false)));
                }
            }
            body
        }

        /// Tab / Shift+Tab: the next visible section becomes active, and in
        /// whole-panel mode the panel scrolls to show it.
        fn cycle(
            &self,
            delta: isize,
            layout: &SidebarLayout,
            cx: &mut EventCx,
        ) -> Handled<SidebarAction> {
            let Some(next) = self.cycled_section(delta) else {
                return Handled::Ignored;
            };
            if self.scroll_mode == ScrollMode::WholePanel {
                if let Some(offset) = self.reveal_section_scroll(next, layout) {
                    cx.emit(SidebarAction::PanelScrolled { offset }.into_ui_event(self.id));
                }
            }
            Handled::Event(SidebarAction::ActiveSectionChanged {
                section: Some(next),
            })
        }

        /// Whether a tree row in any section is being renamed.
        fn is_renaming(&self) -> bool {
            self.sections.iter().any(|s| {
                s.tree()
                    .is_some_and(|t| t.rows.iter().any(|r| r.edit.is_some()))
            })
        }

        /// A key in [`NavigationMode::Scroll`]: Up/Down scroll, Enter selects
        /// the active section's first row, Space toggles the selected branch.
        fn scroll_mode_key(
            &self,
            key: &KeyEvent,
            layout: &SidebarLayout,
            cx: &mut EventCx,
        ) -> Handled<SidebarAction> {
            let Some(section) = self.active_section else {
                return Handled::Ignored;
            };
            let whole = self.scroll_mode == ScrollMode::WholePanel;
            match &key.key {
                Key::Named(NamedKey::Up) if whole => {
                    self.scroll_panel(-layout.msv.line_height, layout)
                }
                Key::Named(NamedKey::Down) if whole => {
                    self.scroll_panel(layout.msv.line_height, layout)
                }
                Key::Named(NamedKey::Up) => self.scroll_section(section, -1, layout),
                Key::Named(NamedKey::Down) => self.scroll_section(section, 1, layout),
                Key::Named(NamedKey::Enter) => {
                    let Some(first) = self.tree(section).and_then(|t| t.rows.first()) else {
                        return Handled::Consumed;
                    };
                    if self.tree(section).is_some_and(|t| t.scroll_offset != 0) {
                        cx.emit(
                            SidebarAction::ScrollChanged { section, offset: 0 }
                                .into_ui_event(self.id),
                        );
                    }
                    Handled::Event(SidebarAction::RowSelected {
                        section,
                        path: first.path.clone(),
                    })
                }
                Key::Char(' ') => self.toggle_selected(section),
                _ => Handled::Ignored,
            }
        }

        fn handle_key(
            &self,
            state: &mut SidebarState,
            event: &RoutedEvent,
            key: &KeyEvent,
            layout: &SidebarLayout,
            cx: &mut EventCx,
        ) -> Handled<SidebarAction> {
            let m = key.modifiers;
            if !self.is_renaming() && !m.ctrl && !m.alt {
                match key.key {
                    Key::Named(NamedKey::Tab) if !m.shift => return self.cycle(1, layout, cx),
                    Key::Named(NamedKey::Tab) | Key::Named(NamedKey::BackTab) => {
                        return self.cycle(-1, layout, cx)
                    }
                    _ => {}
                }
            }
            if self.is_renaming() {
                return self.handle_view(state, event, layout, cx);
            }
            match self.navigation_mode {
                NavigationMode::Scroll => self.scroll_mode_key(key, layout, cx),
                NavigationMode::Selection => {
                    let vim = self
                        .active_section
                        .and_then(|s| self.tree(s))
                        .is_some_and(|t| t.style.vim_keys);
                    // The tree body claims its navigation keys, Left/Right
                    // included; every other key, type-ahead letters among
                    // them, stays the app's shortcut.
                    if !TreeNavCore::claims_key(key, vim) {
                        return Handled::Ignored;
                    }
                    self.handle_view(state, event, layout, cx)
                }
            }
        }
    }

    impl Interactive for Sidebar {
        type State = SidebarState;
        type Action = SidebarAction;

        /// One stop per visible section header, then the active section's
        /// body stops.
        fn focus(&self) -> FocusSpec {
            let mut stops = Vec::new();
            let mut view_idx = 0;
            for (idx, s) in self.sections.iter().enumerate() {
                if !s.visible {
                    continue;
                }
                stops.push(FocusStop {
                    sub: MultiSectionViewSubPart::Header(view_idx).sub_target(),
                });
                if self.active_section == Some(idx) && !s.collapsed {
                    let body = match &s.body {
                        SidebarBody::Tree(t) => Interactive::focus(t),
                        SidebarBody::Form(f) => Interactive::focus(f),
                    };
                    let part = |child| FocusStop {
                        sub: MultiSectionViewSubPart::Body(view_idx, child).sub_target(),
                    };
                    match body {
                        FocusSpec::None => {}
                        FocusSpec::Stop => stops.push(part(None)),
                        FocusSpec::Children(children) => {
                            stops.extend(children.into_iter().map(|c| part(Some(c.sub))))
                        }
                    }
                }
                view_idx += 1;
            }
            if stops.is_empty() {
                FocusSpec::None
            } else {
                FocusSpec::Children(stops)
            }
        }

        fn sub_regions(&self, layout: &SidebarLayout) -> Vec<(Rect, SubTarget)> {
            Interactive::sub_regions(&layout.view, &layout.msv)
        }

        fn handle(
            &self,
            state: &mut SidebarState,
            event: &RoutedEvent,
            layout: &SidebarLayout,
            cx: &mut EventCx,
        ) -> Handled<SidebarAction> {
            match event {
                RoutedEvent::Key(key) => self.handle_key(state, event, key, layout, cx),
                RoutedEvent::Wheel { dy, .. } => match self.handle_view(state, event, layout, cx) {
                    // A wheel over no section scrolls the active one.
                    Handled::Ignored if self.scroll_mode == ScrollMode::PerSection => {
                        match self.active_section {
                            Some(section) => {
                                self.scroll_section(section, wheel_row_delta(*dy, 1.0), layout)
                            }
                            None => Handled::Ignored,
                        }
                    }
                    handled => handled,
                },
                RoutedEvent::Pointer { kind, local, .. } => {
                    let handled = self.handle_view(state, event, layout, cx);
                    if !matches!(handled, Handled::Consumed | Handled::Ignored) {
                        return handled;
                    }
                    let Some((view_idx, on_part)) = self.body_hit(layout, *local) else {
                        return handled;
                    };
                    let Some(&section) = layout.map.get(view_idx) else {
                        return handled;
                    };
                    match kind {
                        // A press on a body's empty space activates its
                        // section, the same as its header.
                        PointerKind::Down(MouseButton::Left)
                            if !on_part && !state.is_dragging() =>
                        {
                            Handled::Event(SidebarAction::HeaderActivated { section })
                        }
                        PointerKind::Down(MouseButton::Right)
                            if !on_part
                                && self.sections[section].empty_space_context_menu
                                && self.tree(section).is_some() =>
                        {
                            let o = layout.msv.chrome.bounds;
                            Handled::Event(SidebarAction::ContextMenuRequested {
                                section,
                                path: TreePath::new(),
                                position: Point::new(local.x + o.x, local.y + o.y),
                            })
                        }
                        _ => handled,
                    }
                }
                _ => self.handle_view(state, event, layout, cx),
            }
        }
    }

    impl SidebarSystem {
        /// Select `path` within `section`, expand the section if it is
        /// collapsed, and scroll its viewport so `path` ends up visible.
        ///
        /// No-op if `section` is out of range, hosts a Form, `path` isn't
        /// among the section's rows, or [`Self::set_backend_info`] hasn't
        /// been called yet (the viewport's row capacity is unknown without
        /// it). A collapsed section is expanded *before* the viewport is
        /// measured.
        pub fn reveal(&mut self, section: usize, path: &TreePath, rect: Rect) {
            let Some(info) = self.backend_info else {
                return;
            };
            let Some(row) = self
                .sidebar
                .tree(section)
                .and_then(|t| t.rows.iter().position(|r| &r.path == path))
            else {
                return;
            };
            self.sidebar.sections[section].collapsed = false;
            let viewport =
                Primitive::layout(&self.sidebar, &info.ctx(), rect).viewport_rows(section);
            if let Some(tree) = self.sidebar.tree_mut(section) {
                tree.selected_path = Some(path.clone());
                tree.scroll_offset =
                    crate::behaviour::scroll::ensure_visible(tree.scroll_offset, viewport, row);
            }
        }

        // ── Render ────────────────────────────────────────────────────────

        pub fn render(&self, backend: &mut dyn Backend, rect: Rect) {
            use crate::backend::DrawsPrimitives;
            backend.draw_dyn(rect, &self.sidebar, &());
        }

        // ── Handle ────────────────────────────────────────────────────────

        pub fn handle(
            &mut self,
            event: &UiEvent,
            backend: &mut dyn Backend,
            rect: Rect,
        ) -> SidebarEvent {
            use crate::backend::DrawsPrimitives;
            let layout = backend.layout_primitive(rect, &self.sidebar);
            let units = crate::backend::primitive_layout_ctx(&*backend).units();
            self.drive(event, rect, &layout, units)
        }

        /// Backend-free event handler. Requires [`Self::set_backend_info`]
        /// (or [`Self::cache_form_layouts`]) called first. Returns
        /// [`SidebarEvent::Ignored`] if backend info is not set.
        pub fn handle_cached(&mut self, event: &UiEvent, rect: Rect) -> SidebarEvent {
            let Some(info) = self.backend_info else {
                return SidebarEvent::Ignored;
            };
            let ctx = info.ctx();
            let layout = Primitive::layout(&self.sidebar, &ctx, rect);
            self.drive(event, rect, &layout, ctx.units())
        }

        fn drive(
            &mut self,
            event: &UiEvent,
            rect: Rect,
            layout: &SidebarLayout,
            units: crate::primitive::Units,
        ) -> SidebarEvent {
            let subs = Interactive::sub_regions(&self.sidebar, layout);
            let Some(routed) = widget_host::routed_event(event, rect, &subs) else {
                return SidebarEvent::Ignored;
            };
            // The keyboard addresses the active section's body.
            let focused = self
                .sidebar
                .active_section
                .and_then(|s| self.sidebar.view_index(s))
                .map(|i| MultiSectionViewSubPart::Body(i, None).sub_target());
            let driven = widget_host::drive_focused(
                &self.sidebar,
                self.sidebar.id,
                &mut self.state,
                &routed,
                layout,
                units,
                focused,
            );
            let mut answer = None;
            for emitted in &driven.emitted {
                if let UiEvent::Sidebar(id, action) = emitted {
                    if *id == self.sidebar.id {
                        let event = self.apply(action);
                        answer = answer.or(Some(event));
                    }
                }
            }
            match driven.handled {
                Handled::Event(action) => self.apply(&action),
                Handled::Consumed => answer.unwrap_or(SidebarEvent::Consumed),
                Handled::Ignored => answer.unwrap_or(SidebarEvent::Ignored),
            }
        }

        /// Store `action` and answer the event it means.
        fn apply(&mut self, action: &SidebarAction) -> SidebarEvent {
            let event = match action.clone() {
                SidebarAction::HeaderActivated { section } => {
                    SidebarEvent::HeaderActivated { section }
                }
                SidebarAction::ActiveSectionChanged { .. }
                | SidebarAction::CollapseChanged { .. }
                | SidebarAction::PanelScrolled { .. } => SidebarEvent::StateChanged,
                SidebarAction::RowSelected { section, path } => {
                    SidebarEvent::RowSelected { section, path }
                }
                SidebarAction::RowActivated { section, path } => {
                    SidebarEvent::RowActivated { section, path }
                }
                SidebarAction::RowToggleExpand { section, path } => {
                    SidebarEvent::RowToggleExpand { section, path }
                }
                SidebarAction::ContextMenuRequested {
                    section,
                    path,
                    position,
                } => SidebarEvent::ContextMenuRequested {
                    section,
                    path,
                    position,
                },
                SidebarAction::ScrollChanged { section, .. } => {
                    SidebarEvent::ScrollChanged { section }
                }
                SidebarAction::EditChanged {
                    section,
                    path,
                    text,
                    cursor,
                    selection_anchor,
                } => {
                    let changed = self.editing[section]
                        .as_ref()
                        .is_none_or(|(_, e)| e.text != text);
                    if let Some((_, edit)) = &mut self.editing[section] {
                        edit.text = text.clone();
                        edit.cursor = cursor;
                        edit.selection_anchor = selection_anchor;
                    }
                    if changed {
                        SidebarEvent::EditChanged {
                            section,
                            path,
                            text,
                        }
                    } else {
                        SidebarEvent::Consumed
                    }
                }
                SidebarAction::EditConfirmed {
                    section,
                    path,
                    new_text,
                } => {
                    self.editing[section] = None;
                    SidebarEvent::EditConfirmed {
                        section,
                        path,
                        new_text,
                    }
                }
                SidebarAction::EditCancelled { section, path } => {
                    self.editing[section] = None;
                    SidebarEvent::EditCancelled { section, path }
                }
                SidebarAction::Form { section, event } => {
                    SidebarEvent::FormEvent { section, event }
                }
            };
            self.sidebar.apply(action);
            event
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::primitives::form::{FieldKind, FormField};
        use crate::{ButtonMask, Decoration, Modifiers, ScrollDelta};

        fn sample_defs() -> Vec<SidebarSectionDef> {
            vec![
                SidebarSectionDef::new("variables", "VARIABLES"),
                SidebarSectionDef::new("watch", "WATCH"),
                SidebarSectionDef::new("breakpoints", "BREAKPOINTS"),
            ]
        }

        fn row(path: u16, text: &str, indent: u16, expanded: Option<bool>) -> TreeRow {
            TreeRow {
                path: vec![path],
                indent,
                icon: None,
                text: StyledText::plain(text),
                badge: None,
                is_expanded: expanded,
                decoration: Decoration::Normal,
                edit: None,
            }
        }

        fn fake_rows(prefix: &str, n: usize) -> Vec<TreeRow> {
            (0..n)
                .map(|i| row(i as u16, &format!("{prefix}{i}"), 0, None))
                .collect()
        }

        fn branch_rows() -> Vec<TreeRow> {
            vec![
                row(0, "src", 0, Some(true)),
                row(1, "tests", 0, Some(false)),
                row(2, "main.rs", 1, None),
            ]
        }

        /// Cell metrics, so `handle_cached` lays the sidebar out on a
        /// terminal grid.
        fn cells(ss: &mut SidebarSystem) {
            ss.set_backend_info(1.0);
        }

        fn layout(ss: &SidebarSystem, rect: Rect) -> SidebarLayout {
            Primitive::layout(&ss.sidebar, &LayoutCtx::new(1.0, 1.0, false), rect)
        }

        /// The first body row of sidebar section `section`.
        fn body_top(ss: &SidebarSystem, section: usize, rect: Rect) -> Rect {
            let l = layout(ss, rect);
            let i = l.view_index(section).expect("section visible");
            l.msv.chrome.sections[i].body_bounds
        }

        fn header_of(ss: &SidebarSystem, section: usize, rect: Rect) -> Rect {
            let l = layout(ss, rect);
            let i = l.view_index(section).expect("section visible");
            l.msv.chrome.sections[i].header_bounds
        }

        fn key(k: Key) -> UiEvent {
            UiEvent::KeyPressed {
                key: k,
                modifiers: Modifiers::default(),
                repeat: false,
            }
        }

        fn named(k: NamedKey) -> UiEvent {
            key(Key::Named(k))
        }

        fn press(button: MouseButton, x: f32, y: f32) -> UiEvent {
            UiEvent::MouseDown {
                widget: None,
                button,
                position: Point::new(x, y),
                modifiers: Modifiers::default(),
            }
        }

        fn wheel(dy: f32, x: f32, y: f32) -> UiEvent {
            UiEvent::Scroll {
                widget: None,
                delta: ScrollDelta::new(0.0, dy),
                position: Point::new(x, y),
            }
        }

        const RECT: Rect = Rect::new(0.0, 0.0, 30.0, 24.0);

        // ── Construction and accessors ──────────────────────────────────

        #[test]
        fn new_starts_with_no_active_section() {
            let ss = SidebarSystem::new(sample_defs());
            assert_eq!(ss.active_section(), None);
            assert!(ss.has_focus());
            assert_eq!(ss.navigation_mode(), NavigationMode::Scroll);
            assert_eq!(ss.scroll_mode(), ScrollMode::PerSection);
        }

        #[test]
        fn set_rows_updates_section_data_and_reads_back() {
            let mut ss = SidebarSystem::new(sample_defs());
            ss.set_rows(1, fake_rows("w", 4));
            assert_eq!(ss.rows(1).len(), 4);
            assert!(ss.rows(0).is_empty());
            assert_eq!(ss.selected_path(1), None);
        }

        #[test]
        fn set_panel_scroll_clamps_negative() {
            let mut ss = SidebarSystem::new(sample_defs());
            ss.set_panel_scroll(-5.0);
            assert_eq!(ss.panel_scroll(), 0.0);
        }

        // ── The view ────────────────────────────────────────────────────

        #[test]
        fn view_marks_the_active_section_and_skips_hidden_ones() {
            let mut ss = SidebarSystem::new(sample_defs());
            ss.set_active_section(Some(2));
            ss.set_section_visible(1, false);
            ss.set_section_badge(0, Some(StyledText::plain("3")));
            let (view, map) = ss.sidebar.view(CELL_ACTIVE_MARKER);
            assert_eq!(map, vec![0, 2]);
            assert_eq!(view.active_section, Some(1));
            assert!(view.sections[1].header.title.spans[0]
                .text
                .starts_with(CELL_ACTIVE_MARKER));
            assert_eq!(
                view.sections[0].header.badge.as_ref().unwrap().spans[0].text,
                "3"
            );
        }

        #[test]
        fn a_pixel_layout_marks_the_active_section_with_the_codicon_chevron() {
            let mut ss = SidebarSystem::new(sample_defs());
            ss.set_active_section(Some(0));
            let pixel = Primitive::layout(&ss.sidebar, &LayoutCtx::new(20.0, 8.0, false), RECT);
            let title = &pixel.view.sections[0].header.title.spans[0].text;
            assert!(title.starts_with(PIXEL_ACTIVE_MARKER), "{title:?}");
            let cell = layout(&ss, RECT);
            let title = &cell.view.sections[0].header.title.spans[0].text;
            assert!(title.starts_with(CELL_ACTIVE_MARKER), "{title:?}");
        }

        #[test]
        fn the_view_shares_the_sidebar_id_so_hover_reaches_its_bodies() {
            let ss = SidebarSystem::new(sample_defs());
            let (view, _) = ss.sidebar.view(CELL_ACTIVE_MARKER);
            assert_eq!(view.id, ss.sidebar.id);
        }

        // ── Tab cycling ─────────────────────────────────────────────────

        #[test]
        fn tab_cycles_the_active_section_and_wraps() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            for expected in [0, 1, 2, 0] {
                assert_eq!(
                    ss.handle_cached(&named(NamedKey::Tab), RECT),
                    SidebarEvent::StateChanged
                );
                assert_eq!(ss.active_section(), Some(expected));
            }
            ss.handle_cached(&named(NamedKey::BackTab), RECT);
            assert_eq!(ss.active_section(), Some(2));
        }

        #[test]
        fn tab_skips_a_hidden_section() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_section_visible(1, false);
            ss.set_active_section(Some(0));
            ss.handle_cached(&named(NamedKey::Tab), RECT);
            assert_eq!(ss.active_section(), Some(2));
        }

        // ── Scroll navigation mode ──────────────────────────────────────

        #[test]
        fn scroll_mode_down_scrolls_the_active_section_and_clamps() {
            let mut ss = SidebarSystem::new(vec![SidebarSectionDef::new("t", "T")]);
            cells(&mut ss);
            ss.set_rows(0, fake_rows("r", 30));
            ss.set_active_section(Some(0));
            let rect = Rect::new(0.0, 0.0, 20.0, 11.0);
            ss.handle_cached(&named(NamedKey::Down), rect);
            assert_eq!(ss.scroll_offset(0), 1);
            for _ in 0..40 {
                ss.handle_cached(&named(NamedKey::Down), rect);
            }
            assert_eq!(ss.scroll_offset(0), 20, "clamped to the last page");
            ss.handle_cached(&named(NamedKey::Up), rect);
            assert_eq!(ss.scroll_offset(0), 19);
        }

        #[test]
        fn scroll_mode_enter_selects_the_first_row_and_resets_scroll() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, fake_rows("v", 30));
            ss.set_active_section(Some(0));
            ss.sidebar.tree_mut(0).unwrap().scroll_offset = 4;
            let ev = ss.handle_cached(&named(NamedKey::Enter), RECT);
            assert_eq!(
                ev,
                SidebarEvent::RowSelected {
                    section: 0,
                    path: vec![0]
                }
            );
            assert_eq!(ss.selected_path(0), Some(&vec![0]));
            assert_eq!(ss.scroll_offset(0), 0);
        }

        #[test]
        fn space_on_a_selected_branch_toggles_it_and_ignores_a_leaf() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, branch_rows());
            ss.set_active_section(Some(0));
            ss.set_selected_path(0, Some(vec![1]));
            assert_eq!(
                ss.handle_cached(&key(Key::Char(' ')), RECT),
                SidebarEvent::RowToggleExpand {
                    section: 0,
                    path: vec![1]
                }
            );
            ss.set_selected_path(0, Some(vec![2]));
            assert_eq!(
                ss.handle_cached(&key(Key::Char(' ')), RECT),
                SidebarEvent::Ignored
            );
            assert_eq!(ss.toggle_expand_selected(), SidebarEvent::Ignored);
        }

        #[test]
        fn whole_panel_mode_scrolls_the_panel_by_one_row_per_key() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            for i in 0..3 {
                ss.set_rows(i, fake_rows("r", 20));
            }
            ss.set_scroll_mode(ScrollMode::WholePanel);
            ss.set_active_section(Some(0));
            let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
            ss.handle_cached(&named(NamedKey::Down), rect);
            assert_eq!(ss.panel_scroll(), 1.0);
        }

        // ── Selection navigation mode ───────────────────────────────────

        fn selection_sidebar() -> SidebarSystem {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, fake_rows("v", 30));
            ss.set_rows(1, fake_rows("w", 3));
            ss.set_navigation_mode(NavigationMode::Selection);
            ss.set_active_section(Some(0));
            ss
        }

        fn down(ss: &mut SidebarSystem) -> SidebarEvent {
            ss.handle_cached(&named(NamedKey::Down), RECT)
        }

        #[test]
        fn selection_down_selects_the_first_row_then_advances() {
            let mut ss = selection_sidebar();
            assert_eq!(
                down(&mut ss),
                SidebarEvent::RowSelected {
                    section: 0,
                    path: vec![0]
                }
            );
            assert_eq!(
                down(&mut ss),
                SidebarEvent::RowSelected {
                    section: 0,
                    path: vec![1]
                }
            );
            ss.handle_cached(&named(NamedKey::Up), RECT);
            assert_eq!(ss.selected_path(0), Some(&vec![0]));
        }

        #[test]
        fn selection_home_end_and_enter() {
            let mut ss = selection_sidebar();
            ss.handle_cached(&named(NamedKey::End), RECT);
            assert_eq!(ss.selected_path(0), Some(&vec![29]));
            ss.handle_cached(&named(NamedKey::Home), RECT);
            assert_eq!(ss.selected_path(0), Some(&vec![0]));
            assert_eq!(
                ss.handle_cached(&named(NamedKey::Enter), RECT),
                SidebarEvent::RowActivated {
                    section: 0,
                    path: vec![0]
                }
            );
        }

        #[test]
        fn selection_scroll_follows_a_row_below_the_viewport() {
            let mut ss = selection_sidebar();
            ss.handle_cached(&named(NamedKey::End), RECT);
            let viewport = layout(&ss, RECT).viewport_rows(0);
            assert!(viewport > 0);
            assert_eq!(ss.scroll_offset(0), 30 - viewport);
        }

        #[test]
        fn selection_in_a_collapsed_or_missing_section_is_ignored() {
            let mut ss = selection_sidebar();
            ss.set_collapsed(0, true);
            assert_eq!(down(&mut ss), SidebarEvent::Ignored);
            ss.set_collapsed(0, false);
            ss.set_active_section(None);
            assert_eq!(down(&mut ss), SidebarEvent::Ignored);
        }

        #[test]
        fn selection_leaves_unbound_keys_to_the_app() {
            let mut ss = selection_sidebar();
            assert_eq!(
                ss.handle_cached(&key(Key::Char('q')), RECT),
                SidebarEvent::Ignored
            );
            // A letter that would type-ahead to a row is still the app's.
            assert_eq!(
                ss.handle_cached(&key(Key::Char('v')), RECT),
                SidebarEvent::Ignored
            );
            assert_eq!(ss.selected_path(0), None);
        }

        #[test]
        fn selection_left_and_right_fold_the_selected_branch() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, branch_rows());
            ss.set_navigation_mode(NavigationMode::Selection);
            ss.set_active_section(Some(0));
            ss.set_selected_path(0, Some(vec![1]));
            assert_eq!(
                ss.handle_cached(&named(NamedKey::Right), RECT),
                SidebarEvent::RowToggleExpand {
                    section: 0,
                    path: vec![1]
                }
            );
            ss.set_selected_path(0, Some(vec![0]));
            assert_eq!(
                ss.handle_cached(&named(NamedKey::Left), RECT),
                SidebarEvent::RowToggleExpand {
                    section: 0,
                    path: vec![0]
                }
            );
        }

        // ── Pointer ─────────────────────────────────────────────────────

        #[test]
        fn clicking_a_row_selects_it_and_activates_its_section() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(1, fake_rows("w", 3));
            let body = body_top(&ss, 1, RECT);
            let ev = ss.handle_cached(&press(MouseButton::Left, body.x + 4.0, body.y + 1.0), RECT);
            assert_eq!(
                ev,
                SidebarEvent::RowSelected {
                    section: 1,
                    path: vec![1]
                }
            );
            assert_eq!(ss.active_section(), Some(1));
            assert_eq!(ss.selected_path(1), Some(&vec![1]));
        }

        /// On a cell grid the chevron is the two cells before a branch's
        /// title, so a click on the title selects the row rather than
        /// expanding it.
        #[cfg(feature = "tui")]
        #[test]
        fn clicking_a_branch_title_on_cells_selects_rather_than_toggles() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, branch_rows());
            let body = body_top(&ss, 0, RECT);
            let ev = ss.handle_cached(&press(MouseButton::Left, body.x + 4.0, body.y + 1.0), RECT);
            assert_eq!(
                ev,
                SidebarEvent::RowSelected {
                    section: 0,
                    path: vec![1]
                }
            );
            let ev = ss.handle_cached(&press(MouseButton::Left, body.x, body.y + 1.0), RECT);
            assert_eq!(
                ev,
                SidebarEvent::RowToggleExpand {
                    section: 0,
                    path: vec![1]
                }
            );
        }

        #[test]
        fn double_clicking_a_row_activates_it() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, fake_rows("v", 3));
            let body = body_top(&ss, 0, RECT);
            let ev = ss.handle_cached(
                &UiEvent::DoubleClick {
                    widget: None,
                    position: Point::new(body.x + 3.0, body.y + 2.0),
                },
                RECT,
            );
            assert_eq!(
                ev,
                SidebarEvent::RowActivated {
                    section: 0,
                    path: vec![2]
                }
            );
        }

        #[test]
        fn clicking_a_body_below_its_rows_activates_the_section() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, fake_rows("v", 2));
            ss.set_selected_path(0, Some(vec![1]));
            let body = body_top(&ss, 0, RECT);
            let ev = ss.handle_cached(&press(MouseButton::Left, body.x + 3.0, body.y + 4.0), RECT);
            assert_eq!(ev, SidebarEvent::HeaderActivated { section: 0 });
            assert_eq!(ss.selected_path(0), None);
        }

        #[test]
        fn right_clicking_a_row_requests_its_context_menu() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, fake_rows("v", 3));
            let body = body_top(&ss, 0, RECT);
            let at = Point::new(body.x + 3.0, body.y);
            let ev = ss.handle_cached(&press(MouseButton::Right, at.x, at.y), RECT);
            assert_eq!(
                ev,
                SidebarEvent::ContextMenuRequested {
                    section: 0,
                    path: vec![0],
                    position: at,
                }
            );
            assert_eq!(ss.selected_path(0), Some(&vec![0]));
        }

        #[test]
        fn empty_space_context_menu_is_opt_in_per_section() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, fake_rows("v", 2));
            let body = body_top(&ss, 0, RECT);
            let at = Point::new(body.x + 3.0, body.y + 4.0);
            assert_eq!(
                ss.handle_cached(&press(MouseButton::Right, at.x, at.y), RECT),
                SidebarEvent::Ignored
            );
            ss.set_empty_space_context_menu(0, true);
            assert_eq!(
                ss.handle_cached(&press(MouseButton::Right, at.x, at.y), RECT),
                SidebarEvent::ContextMenuRequested {
                    section: 0,
                    path: TreePath::new(),
                    position: at,
                }
            );
        }

        // ── Headers ─────────────────────────────────────────────────────

        #[test]
        fn a_header_click_activates_and_collapses_only_when_allowed() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            let h = header_of(&ss, 1, RECT);
            let ev = ss.handle_cached(&press(MouseButton::Left, h.x + 2.0, h.y), RECT);
            assert_eq!(ev, SidebarEvent::HeaderActivated { section: 1 });
            assert_eq!(ss.active_section(), Some(1));
            assert!(!ss.is_collapsed(1));

            ss.set_allow_collapse(true);
            ss.handle_cached(&press(MouseButton::Left, h.x + 2.0, h.y), RECT);
            assert!(ss.is_collapsed(1));
            let h = header_of(&ss, 1, RECT);
            ss.handle_cached(&press(MouseButton::Left, h.x + 2.0, h.y), RECT);
            assert!(!ss.is_collapsed(1));
        }

        // ── Wheel ───────────────────────────────────────────────────────

        fn scroll_routing_sidebar() -> SidebarSystem {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            for i in 0..3 {
                ss.set_rows(i, fake_rows("r", 30));
            }
            ss.set_active_section(Some(0));
            ss
        }

        #[test]
        fn the_wheel_scrolls_the_section_under_the_pointer() {
            let mut ss = scroll_routing_sidebar();
            let body = body_top(&ss, 2, RECT);
            ss.handle_cached(&wheel(-1.0, body.x + 2.0, body.y + 1.0), RECT);
            assert!(ss.scroll_offset(2) > 0);
            assert_eq!(ss.scroll_offset(0), 0);
        }

        #[test]
        fn the_wheel_outside_every_section_scrolls_the_active_one() {
            let mut ss = scroll_routing_sidebar();
            ss.handle_cached(&wheel(-1.0, 500.0, 500.0), RECT);
            assert_eq!(ss.scroll_offset(0), 1);
        }

        // ── Scrollbars ──────────────────────────────────────────────────

        #[test]
        fn a_one_cell_drag_on_a_section_thumb_scrolls_it() {
            let mut ss = SidebarSystem::new(vec![SidebarSectionDef::new("t", "T")]);
            cells(&mut ss);
            ss.set_rows(0, fake_rows("r", 30));
            ss.set_active_section(Some(0));
            let rect = Rect::new(0.0, 0.0, 20.0, 21.0);
            let l = layout(&ss, rect);
            let sb = l.msv.chrome.sections[0].scrollbar_bounds.expect("gutter");
            let thumb = l.msv.chrome.sections[0].thumb_bounds.expect("thumb");
            ss.handle_cached(&press(MouseButton::Left, sb.x, thumb.y), rect);
            ss.handle_cached(
                &UiEvent::MouseMoved {
                    position: Point::new(sb.x, thumb.y + 1.0),
                    buttons: ButtonMask {
                        left: true,
                        ..Default::default()
                    },
                },
                rect,
            );
            assert!(ss.scroll_offset(0) > 0);
        }

        #[test]
        fn a_panel_thumb_drag_scrolls_the_panel() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            for i in 0..3 {
                ss.set_rows(i, fake_rows("r", 8));
            }
            for s in &mut ss.sidebar.sections {
                s.size = SectionSize::Content;
            }
            ss.set_scroll_mode(ScrollMode::WholePanel);
            let rect = Rect::new(0.0, 0.0, 20.0, 12.0);
            let l = layout(&ss, rect);
            let thumb = l.msv.chrome.panel_scrollbar_thumb.expect("panel overflows");
            ss.handle_cached(&press(MouseButton::Left, thumb.x, thumb.y), rect);
            ss.handle_cached(
                &UiEvent::MouseMoved {
                    position: Point::new(thumb.x, thumb.y + 2.0),
                    buttons: ButtonMask {
                        left: true,
                        ..Default::default()
                    },
                },
                rect,
            );
            assert!(ss.panel_scroll() > 0.0);
        }

        // ── Form sections ───────────────────────────────────────────────

        fn field(id: &str, label: &str, kind: FieldKind) -> FormField {
            FormField {
                id: WidgetId::new(id),
                label: StyledText::plain(label),
                kind,
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }
        }

        fn form(fields: Vec<FormField>) -> Form {
            Form {
                fields,
                ..empty_form("search")
            }
        }

        fn mixed() -> SidebarSystem {
            let mut ss = SidebarSystem::new(vec![
                SidebarSectionDef::form("search", "SEARCH"),
                SidebarSectionDef::new("results", "RESULTS"),
            ]);
            cells(&mut ss);
            ss
        }

        #[test]
        fn form_and_tree_setters_only_reach_their_own_kind() {
            let mut ss = mixed();
            assert_eq!(ss.section_kind(0), Some(SectionKind::Form));
            assert_eq!(ss.section_kind(1), Some(SectionKind::Tree));
            assert!(ss.form(0).is_none());
            ss.set_form(1, form(Vec::new()));
            assert!(ss.form(1).is_none());
            ss.set_rows(0, fake_rows("x", 2));
            assert!(ss.rows(0).is_empty());
            ss.set_form(
                0,
                form(vec![field("q", "Q", FieldKind::Toggle { value: false })]),
            );
            assert_eq!(ss.form(0).unwrap().fields.len(), 1);
        }

        #[test]
        fn focus_form_paints_a_form_focused_while_a_tree_section_is_active() {
            let mut ss = mixed();
            ss.set_form(0, form(Vec::new()));
            ss.set_active_section(Some(1));
            assert!(!ss.form_has_focus(0));
            ss.focus_form(0, true);
            assert!(ss.form_has_focus(0));
            let (view, _) = ss.sidebar.view(CELL_ACTIVE_MARKER);
            let SectionBody::Form(f) = &view.sections[0].body else {
                panic!("form body");
            };
            assert!(f.has_focus);
            ss.focus_form(1, true);
            assert!(!ss.form_has_focus(1), "a tree section has no form focus");
        }

        #[test]
        fn clicking_a_toggle_in_a_form_section_flips_it_through_the_form() {
            let mut ss = mixed();
            ss.set_form(
                0,
                form(vec![field(
                    "case",
                    "Case",
                    FieldKind::Toggle { value: false },
                )]),
            );
            let body = body_top(&ss, 0, RECT);
            let ev = ss.handle_cached(&press(MouseButton::Left, body.x + 1.0, body.y), RECT);
            assert_eq!(
                ev,
                SidebarEvent::FormEvent {
                    section: 0,
                    event: FormEvent::ToggleChanged {
                        id: WidgetId::new("case"),
                        value: true,
                    },
                }
            );
            assert_eq!(ss.active_section(), Some(0));
        }

        /// A press on a number input's increment stepper in a form section
        /// steps it, through the form's own handling of the press.
        #[cfg(feature = "tui")]
        #[test]
        fn a_number_input_stepper_in_a_form_section_steps_the_value() {
            let number_sidebar = || {
                let mut count = crate::primitives::number_input::NumberInput::new(
                    WidgetId::new("count"),
                    0.0,
                    10.0,
                );
                count.value = 5.0;
                let mut ss = mixed();
                ss.set_form(
                    0,
                    form(vec![field("count", "Count", FieldKind::NumberInput(count))]),
                );
                ss
            };
            let probe = number_sidebar();
            let body = body_top(&probe, 0, RECT);
            let stepped = (0..body.width as usize).any(|dx| {
                let mut ss = number_sidebar();
                let ev =
                    ss.handle_cached(&press(MouseButton::Left, body.x + dx as f32, body.y), RECT);
                matches!(
                    ev,
                    SidebarEvent::FormEvent {
                        section: 0,
                        event: FormEvent::NumberChanged { value, .. },
                    } if value == 6.0
                )
            });
            assert!(stepped, "no press on the number row stepped it up");
        }

        #[test]
        fn selection_keys_in_a_form_section_are_ignored() {
            let mut ss = mixed();
            ss.set_form(
                0,
                form(vec![field(
                    "case",
                    "Case",
                    FieldKind::Toggle { value: false },
                )]),
            );
            ss.set_navigation_mode(NavigationMode::Selection);
            ss.set_active_section(Some(0));
            assert_eq!(
                ss.handle_cached(&named(NamedKey::Down), RECT),
                SidebarEvent::Ignored
            );
        }

        // ── Reveal and rename ───────────────────────────────────────────

        #[test]
        fn reveal_expands_selects_and_scrolls_to_the_row() {
            let mut ss = SidebarSystem::new(vec![SidebarSectionDef::new("t", "T")]);
            ss.set_rows(0, fake_rows("r", 40));
            ss.set_collapsed(0, true);
            let rect = Rect::new(0.0, 0.0, 20.0, 11.0);
            ss.reveal(0, &vec![30], rect);
            assert_eq!(ss.selected_path(0), None, "no backend info, no reveal");

            cells(&mut ss);
            ss.reveal(0, &vec![30], rect);
            assert!(!ss.is_collapsed(0));
            assert_eq!(ss.selected_path(0), Some(&vec![30]));
            let viewport = layout(&ss, rect).viewport_rows(0);
            let offset = ss.scroll_offset(0);
            assert!(
                offset <= 30 && 30 < offset + viewport,
                "{offset} {viewport}"
            );
        }

        #[test]
        fn a_rename_survives_set_rows_and_reports_each_edit() {
            let mut ss = SidebarSystem::new(sample_defs());
            cells(&mut ss);
            ss.set_rows(0, fake_rows("v", 3));
            ss.set_active_section(Some(0));
            ss.start_editing(0, vec![1], "ab".into(), 2, None, None);
            ss.set_rows(0, fake_rows("v", 3));
            assert!(ss.is_editing());
            assert_eq!(
                ss.handle_cached(&UiEvent::CharTyped('c'), RECT),
                SidebarEvent::EditChanged {
                    section: 0,
                    path: vec![1],
                    text: "abc".into(),
                }
            );
            assert_eq!(
                ss.handle_cached(&named(NamedKey::Enter), RECT),
                SidebarEvent::EditConfirmed {
                    section: 0,
                    path: vec![1],
                    new_text: "abc".into(),
                }
            );
            assert!(!ss.is_editing());
        }

        #[test]
        fn cancel_editing_ends_the_session() {
            let mut ss = SidebarSystem::new(sample_defs());
            ss.set_rows(0, fake_rows("v", 3));
            ss.start_editing(0, vec![0], "x".into(), 1, None, None);
            ss.cancel_editing(0);
            assert!(!ss.is_editing());
            assert!(ss.rows(0).iter().all(|r| r.edit.is_none()));
        }

        // ── Widget contract ─────────────────────────────────────────────

        #[test]
        fn focus_lists_every_header_then_the_active_body() {
            let mut ss = SidebarSystem::new(sample_defs());
            ss.set_rows(1, fake_rows("w", 2));
            ss.set_active_section(Some(1));
            let FocusSpec::Children(stops) = Interactive::focus(&ss.sidebar) else {
                panic!("children");
            };
            let parts: Vec<_> = stops
                .iter()
                .map(|s| MultiSectionViewSubPart::from_sub_target(s.sub).unwrap())
                .collect();
            assert_eq!(
                parts,
                vec![
                    MultiSectionViewSubPart::Header(0),
                    MultiSectionViewSubPart::Header(1),
                    MultiSectionViewSubPart::Body(1, None),
                    MultiSectionViewSubPart::Header(2),
                ]
            );
        }

        #[test]
        fn apply_writes_every_controlled_field() {
            let mut sidebar = SidebarSystem::new(sample_defs()).sidebar;
            sidebar.tree_mut(0).unwrap().rows = fake_rows("v", 3);
            sidebar.apply(&SidebarAction::RowSelected {
                section: 0,
                path: vec![2],
            });
            sidebar.apply(&SidebarAction::ScrollChanged {
                section: 0,
                offset: 1,
            });
            sidebar.apply(&SidebarAction::CollapseChanged {
                section: 2,
                collapsed: true,
            });
            sidebar.apply(&SidebarAction::PanelScrolled { offset: 3.0 });
            assert_eq!(sidebar.active_section, Some(0));
            assert_eq!(sidebar.tree(0).unwrap().selected_path, Some(vec![2]));
            assert_eq!(sidebar.tree(0).unwrap().scroll_offset, 1);
            assert!(sidebar.sections[2].collapsed);
            assert_eq!(sidebar.panel_scroll, 3.0);
            sidebar.apply(&SidebarAction::HeaderActivated { section: 0 });
            assert_eq!(sidebar.tree(0).unwrap().selected_path, None);
        }

        #[test]
        fn sidebar_action_round_trips_through_serde() {
            let action = SidebarAction::ContextMenuRequested {
                section: 1,
                path: vec![0, 2],
                position: Point::new(3.0, 4.0),
            };
            let json = serde_json::to_string(&action).unwrap();
            assert_eq!(
                serde_json::from_str::<SidebarAction>(&json).unwrap(),
                action
            );
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use contract::SidebarLayout;

// ── Controller ─────────────────────────────────────────────────────────────────

/// The metrics [`SidebarSystem::handle_cached`] lays the sidebar out
/// with when no backend is in reach.
#[cfg_attr(
    not(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )),
    allow(dead_code)
)]
#[derive(Debug, Clone, Copy)]
struct BackendInfo {
    line_height: f32,
    char_width: f32,
}

#[cfg_attr(
    not(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )),
    allow(dead_code)
)]
impl BackendInfo {
    fn ctx(&self) -> LayoutCtx<'static> {
        LayoutCtx::new(self.line_height, self.char_width, false)
    }
}

/// A [`Sidebar`] plus its [`SidebarState`], driven by raw [`UiEvent`]s
/// and painted through a [`Backend`]. The controller applies every
/// [`SidebarAction`] itself and answers [`SidebarEvent`]s.
pub struct SidebarSystem {
    pub(crate) sidebar: Sidebar,
    kinds: Vec<SectionKind>,
    /// Whether each form section has had a form set.
    form_set: Vec<bool>,
    /// The rename session each tree section holds, re-applied to the
    /// rows every [`Self::set_rows`] pushes.
    pub(crate) editing: Vec<Option<(TreePath, TreeRowEditState)>>,
    #[cfg_attr(
        not(any(
            feature = "gtk",
            feature = "tui",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )),
        allow(dead_code)
    )]
    state: SidebarState,
    backend_info: Option<BackendInfo>,
}

/// What happened after [`SidebarSystem::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
pub enum SidebarEvent {
    /// Header clicked — section activated.
    HeaderActivated { section: usize },
    /// Tree row clicked — section activated + row selected.
    RowSelected { section: usize, path: TreePath },
    /// Enter pressed on the selected row (Selection mode), or a row
    /// double-clicked.
    RowActivated { section: usize, path: TreePath },
    /// The expand/collapse chevron of a branch row was clicked, or Space
    /// was pressed while a branch row is selected. The app owns per-row
    /// expand state and should toggle it, then push fresh `TreeRow`s.
    RowToggleExpand { section: usize, path: TreePath },
    /// Right-click on a body row, or on a tree's empty space when the
    /// section opts in ([`SidebarSystem::set_empty_space_context_menu`]),
    /// with an empty `path`. `position` is the click position in the
    /// backend's native coordinates.
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

/// [`Interactive::State`] for [`Sidebar`]: the view's gesture in flight
/// and each body widget's own state.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct SidebarState {
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    msv: crate::primitives::multi_section_view::MultiSectionViewState,
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl SidebarState {
    /// Whether a scrollbar thumb or body gesture holds the pointer.
    pub fn is_dragging(&self) -> bool {
        self.msv.is_dragging()
    }
}

impl Sidebar {
    /// Space on section `section`'s selected row: toggle it when it is a
    /// branch.
    fn toggle_selected(&self, section: usize) -> Handled<SidebarAction> {
        let Some(tree) = self.tree(section) else {
            return Handled::Ignored;
        };
        let row = tree
            .selected_path
            .as_ref()
            .and_then(|p| tree.rows.iter().find(|r| &r.path == p));
        match row {
            Some(row) if row.is_expanded.is_some() => {
                Handled::Event(SidebarAction::RowToggleExpand {
                    section,
                    path: row.path.clone(),
                })
            }
            _ => Handled::Ignored,
        }
    }
}

impl SidebarSystem {
    pub fn new(defs: Vec<SidebarSectionDef>) -> Self {
        let mut sidebar = Sidebar::new(WidgetId::new("sidebar-system"));
        sidebar.sections = defs
            .iter()
            .map(|def| {
                let body = match def.kind {
                    SectionKind::Tree => SidebarBody::Tree(TreeView {
                        id: WidgetId::new(format!("{}-tree", def.id)),
                        rows: Vec::new(),
                        selection_mode: SelectionMode::Single,
                        selected_path: None,
                        scroll_offset: 0,
                        style: Default::default(),
                        has_focus: false,
                    }),
                    SectionKind::Form => SidebarBody::Form(empty_form(&def.id)),
                };
                SidebarSection::new(def, body)
            })
            .collect();
        let n = defs.len();
        Self {
            sidebar,
            kinds: defs.iter().map(|d| d.kind).collect(),
            form_set: vec![false; n],
            editing: vec![None; n],
            state: SidebarState::default(),
            backend_info: None,
        }
    }

    /// The descriptor this controller paints and drives.
    pub fn sidebar(&self) -> &Sidebar {
        &self.sidebar
    }

    // ── Per-frame data ────────────────────────────────────────────────

    pub fn set_rows(&mut self, section: usize, mut rows: Vec<TreeRow>) {
        if let Some(Some((path, edit))) = self.editing.get(section) {
            if let Some(row) = rows.iter_mut().find(|r| &r.path == path) {
                row.edit = Some(edit.clone());
            }
        }
        if let Some(tree) = self.sidebar.tree_mut(section) {
            tree.rows = rows;
        }
    }

    /// Section `section`'s tree rows.
    pub fn rows(&self, section: usize) -> &[TreeRow] {
        self.sidebar.tree(section).map_or(&[], |t| &t.rows)
    }

    pub fn set_form(&mut self, section: usize, form: Form) {
        if let Some(SidebarSection {
            body: SidebarBody::Form(f),
            ..
        }) = self.sidebar.sections.get_mut(section)
        {
            *f = form;
            self.form_set[section] = true;
        }
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn active_section(&self) -> Option<usize> {
        self.sidebar.active_section
    }

    pub fn selected_path(&self, section: usize) -> Option<&TreePath> {
        self.sidebar.tree(section)?.selected_path.as_ref()
    }

    pub fn scroll_offset(&self, section: usize) -> usize {
        self.sidebar.tree(section).map_or(0, |t| t.scroll_offset)
    }

    pub fn form(&self, section: usize) -> Option<&Form> {
        if !self.form_set.get(section).copied().unwrap_or(false) {
            return None;
        }
        self.sidebar.sections[section].form()
    }

    pub fn section_kind(&self, section: usize) -> Option<SectionKind> {
        self.kinds.get(section).copied()
    }

    pub fn is_collapsed(&self, section: usize) -> bool {
        self.sidebar
            .sections
            .get(section)
            .is_some_and(|s| s.collapsed)
    }

    pub fn is_section_visible(&self, section: usize) -> bool {
        self.sidebar.sections.get(section).is_none_or(|s| s.visible)
    }

    pub fn section_badge(&self, section: usize) -> Option<&StyledText> {
        self.sidebar.sections.get(section)?.badge.as_ref()
    }

    // ── Programmatic state control ────────────────────────────────────

    pub fn set_active_section(&mut self, section: Option<usize>) {
        self.sidebar.active_section = section.filter(|s| *s < self.sidebar.sections.len());
    }

    /// Mark a Form section as focused *for rendering purposes only*,
    /// without changing the active (nav) section — so a form field (e.g.
    /// a FILTER box) shows its caret while a different section's tree
    /// drives navigation.
    ///
    /// No-op if `section` is out of range or hosts a Tree, not a Form.
    pub fn focus_form(&mut self, section: usize, focused: bool) {
        if let Some(s) = self.sidebar.sections.get_mut(section) {
            if s.form().is_some() {
                s.form_focused = focused;
            }
        }
    }

    /// Whether the Form section at `section` is render-focused via
    /// [`Self::focus_form`]. Returns `false` for Tree sections, empty
    /// sections, or out-of-range indices.
    pub fn form_has_focus(&self, section: usize) -> bool {
        self.sidebar
            .sections
            .get(section)
            .is_some_and(|s| s.form().is_some() && s.form_focused)
    }

    pub fn set_selected_path(&mut self, section: usize, path: Option<TreePath>) {
        if let Some(tree) = self.sidebar.tree_mut(section) {
            tree.selected_path = path;
        }
    }

    pub fn set_collapsed(&mut self, section: usize, collapsed: bool) {
        if let Some(s) = self.sidebar.sections.get_mut(section) {
            s.collapsed = collapsed;
        }
    }

    pub fn set_section_visible(&mut self, section: usize, visible: bool) {
        if let Some(s) = self.sidebar.sections.get_mut(section) {
            s.visible = visible;
        }
    }

    pub fn set_section_badge(&mut self, section: usize, badge: Option<StyledText>) {
        if let Some(s) = self.sidebar.sections.get_mut(section) {
            s.badge = badge;
        }
    }

    /// Whether a right-click on the empty space below tree section
    /// `section`'s rows reports [`SidebarEvent::ContextMenuRequested`]
    /// with an empty `path` (default `false`: the click is ignored).
    pub fn set_empty_space_context_menu(&mut self, section: usize, enabled: bool) {
        if let Some(s) = self.sidebar.sections.get_mut(section) {
            s.empty_space_context_menu = enabled;
        }
    }

    pub fn has_focus(&self) -> bool {
        self.sidebar.has_focus
    }

    pub fn set_has_focus(&mut self, has_focus: bool) {
        self.sidebar.has_focus = has_focus;
    }

    pub fn set_allow_collapse(&mut self, allow: bool) {
        self.sidebar.allow_collapse = allow;
    }

    pub fn navigation_mode(&self) -> NavigationMode {
        self.sidebar.navigation_mode
    }

    pub fn set_navigation_mode(&mut self, mode: NavigationMode) {
        self.sidebar.navigation_mode = mode;
    }

    pub fn scroll_mode(&self) -> ScrollMode {
        self.sidebar.scroll_mode
    }

    pub fn set_scroll_mode(&mut self, mode: ScrollMode) {
        self.sidebar.scroll_mode = mode;
    }

    pub fn panel_scroll(&self) -> f32 {
        self.sidebar.panel_scroll
    }

    pub fn set_panel_scroll(&mut self, offset: f32) {
        self.sidebar.panel_scroll = offset.max(0.0);
    }

    /// Cache the line height [`Self::handle_cached`] lays the sidebar out
    /// with; the header/divider/gutter metrics derive from it.
    pub fn set_backend_info(&mut self, line_height: f32) {
        let char_width = if line_height <= 1.0 {
            1.0
        } else {
            line_height * 0.6
        };
        self.backend_info = Some(BackendInfo {
            line_height,
            char_width,
        });
    }

    /// Cache `backend`'s own line height and character advance, so
    /// [`Self::handle_cached`] lays the sidebar out exactly as
    /// [`Self::render`] paints it.
    pub fn cache_form_layouts(&mut self, backend: &dyn Backend) {
        let ctx = crate::backend::primitive_layout_ctx(backend);
        self.backend_info = Some(BackendInfo {
            line_height: ctx.line_height,
            char_width: ctx.char_width,
        });
    }

    /// Emit [`SidebarEvent::RowToggleExpand`] for the selected row of the
    /// active section, if that row is a branch (`is_expanded.is_some()`).
    pub fn toggle_expand_selected(&self) -> SidebarEvent {
        let Some(section) = self.sidebar.active_section else {
            return SidebarEvent::Ignored;
        };
        match self.sidebar.toggle_selected(section) {
            Handled::Event(SidebarAction::RowToggleExpand { section, path }) => {
                SidebarEvent::RowToggleExpand { section, path }
            }
            _ => SidebarEvent::Ignored,
        }
    }
}

/// The empty form a form section shows until the app sets one.
fn empty_form(section_id: &str) -> Form {
    Form {
        id: WidgetId::new(format!("{section_id}-form")),
        fields: Vec::new(),
        focused_field: None,
        scroll_offset: 0,
        has_focus: false,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    }
}

/// Without a backend feature there is no widget contract to lay the
/// sidebar out through, so it neither paints nor handles events.
#[cfg(not(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
)))]
impl SidebarSystem {
    pub fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        let _ = (backend, rect);
    }

    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> SidebarEvent {
        let _ = (event, backend, rect);
        SidebarEvent::Ignored
    }

    pub fn handle_cached(&mut self, event: &UiEvent, rect: Rect) -> SidebarEvent {
        let _ = (event, rect);
        SidebarEvent::Ignored
    }

    pub fn reveal(&mut self, section: usize, path: &TreePath, rect: Rect) {
        let _ = (section, path, rect);
    }
}
