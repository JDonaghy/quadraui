//! The sidebar: a stack of collapsible sections, each a tree or a form,
//! as one composite widget ([`Sidebar`]).
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
//! A host that routes the sidebar through its own `FrameTree` gets its
//! events from the runtime; one that drives it with raw [`UiEvent`]s
//! holds a [`SidebarState`] and calls [`Sidebar::handle_event`], then
//! applies each action it answers.
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
use crate::primitives::form::{Form, FormEvent};
use crate::{
    MsvAxis, MultiSectionView, Point, ScrollMode, Section, SectionBody, SectionHeader, SectionSize,
    SelectionMode, StyledText, TreePath, TreeRow, TreeRowEditState, TreeView, UiEvent, WidgetId,
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
    /// The rename session of each section that has one, kept apart from
    /// the rows so it outlives a [`Sidebar::set_rows`] whose rows do not
    /// (yet) hold the renamed path.
    renames: Vec<(usize, TreePath, TreeRowEditState)>,
}

/// What a [`Sidebar`] reports.
///
/// Row, form and edit events name the section they came from by its
/// index in [`Sidebar::sections`]. The rest are controlled-state
/// write-backs for the app to store ([`Sidebar::apply`] stores them all).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
            renames: Vec::new(),
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
                if let Some((_, _, edit)) = self
                    .renames
                    .iter_mut()
                    .find(|(s, p, _)| s == section && p == path)
                {
                    edit.text = text.clone();
                    edit.cursor = *cursor;
                    edit.selection_anchor = *selection_anchor;
                }
            }
            SidebarAction::EditConfirmed { section, path, .. }
            | SidebarAction::EditCancelled { section, path } => {
                self.renames
                    .retain(|(s, p, _)| !(s == section && p == path));
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

impl Sidebar {
    /// A sidebar with id `id` and one visible, expanded section per
    /// `defs` entry: an empty tree for a tree section, an empty form for
    /// a form section. Each body widget's id is the section id plus
    /// `-tree` or `-form`.
    pub fn from_defs(id: impl Into<WidgetId>, defs: &[SidebarSectionDef]) -> Self {
        let mut sidebar = Self::new(id);
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
        sidebar
    }

    /// Replace tree section `section`'s rows. The section's rename
    /// session lands on the row holding its path whenever the new rows
    /// hold it, so an app that rebuilds its rows every frame does not end
    /// the rename, and one that starts a rename before pushing the row
    /// (a new file's placeholder) sees it once the row arrives.
    pub fn set_rows(&mut self, section: usize, mut rows: Vec<TreeRow>) {
        let rename = self
            .renames
            .iter()
            .find(|(s, _, _)| *s == section)
            .map(|(_, path, edit)| (path.clone(), edit.clone()));
        let Some(tree) = self.tree_mut(section) else {
            return;
        };
        if let Some((path, edit)) = rename {
            if let Some(row) = rows.iter_mut().find(|r| r.path == path) {
                row.edit = Some(edit);
            }
        } else if let Some(editing) = tree.rows.iter().find(|r| r.edit.is_some()) {
            if let Some(row) = rows
                .iter_mut()
                .find(|r| r.path == editing.path && r.edit.is_none())
            {
                row.edit = editing.edit.clone();
            }
        }
        tree.rows = rows;
    }

    /// Section `section`'s form.
    pub fn form(&self, section: usize) -> Option<&Form> {
        self.sections.get(section).and_then(|s| s.form())
    }

    /// Section `section`'s form, mutably.
    pub fn form_mut(&mut self, section: usize) -> Option<&mut Form> {
        match &mut self.sections.get_mut(section)?.body {
            SidebarBody::Form(f) => Some(f),
            SidebarBody::Tree(_) => None,
        }
    }

    /// Begin renaming the row at `path` in tree section `section`,
    /// selecting it. `cursor` and `selection_anchor` are byte offsets
    /// into `initial_text`; select-all is `cursor = len, anchor =
    /// Some(0)`. The tree's own `Interactive` edits the row from then on
    /// and reports each keystroke as [`SidebarAction::EditChanged`].
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
        let Some(tree) = self.tree_mut(section) else {
            return;
        };
        for row in &mut tree.rows {
            row.edit = (row.path == path).then(|| edit.clone());
        }
        tree.selected_path = Some(path.clone());
        self.renames.retain(|(s, _, _)| *s != section);
        self.renames.push((section, path, edit));
    }

    /// End any rename in section `section` without reporting it.
    pub fn cancel_editing(&mut self, section: usize) {
        self.renames.retain(|(s, _, _)| *s != section);
        if let Some(tree) = self.tree_mut(section) {
            for row in &mut tree.rows {
                row.edit = None;
            }
        }
    }

    /// Whether a tree row in any section is being renamed.
    pub fn is_editing(&self) -> bool {
        !self.renames.is_empty()
            || self.sections.iter().any(|s| {
                s.tree()
                    .is_some_and(|t| t.rows.iter().any(|r| r.edit.is_some()))
            })
    }

    /// The action toggling the active section's selected row, when that
    /// row is a branch (`is_expanded.is_some()`) — what Space does in
    /// [`NavigationMode::Scroll`].
    pub fn toggle_expand_selected(&self) -> Option<SidebarAction> {
        match self.toggle_selected(self.active_section?) {
            Handled::Event(action) => Some(action),
            _ => None,
        }
    }
}

/// The widget contract and its runtime-free host, on the builds that have the
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
    use crate::primitive::LayoutCtx;
    use crate::primitive::{PaintEnv, Primitive};
    use crate::primitives::multi_section_view::{
        MultiSectionViewEvent, MultiSectionViewPrimitiveLayout, MultiSectionViewSubPart,
        SectionBodyLayout,
    };
    use crate::primitives::scrollbar::wheel_row_delta;
    use crate::primitives::tree::TreeEvent;
    use crate::{Backend, Rect};
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
            if !self.is_editing() && !m.ctrl && !m.alt {
                match key.key {
                    Key::Named(NamedKey::Tab) if !m.shift => return self.cycle(1, layout, cx),
                    Key::Named(NamedKey::Tab) | Key::Named(NamedKey::BackTab) => {
                        return self.cycle(-1, layout, cx)
                    }
                    _ => {}
                }
            }
            if self.is_editing() {
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

    // ── Scroll arithmetic ──────────────────────────────────────────────────────────

    impl Sidebar {
        /// The scroll action moving section `section`'s tree by `rows`,
        /// clamped to its last page.
        fn scroll_section(
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
        fn scroll_panel(&self, delta: f32, layout: &SidebarLayout) -> Handled<SidebarAction> {
            let (_, max) = layout.msv.panel_extent();
            let offset = (self.panel_scroll + delta).clamp(0.0, max);
            if (offset - self.panel_scroll).abs() < f32::EPSILON {
                return Handled::Consumed;
            }
            Handled::Event(SidebarAction::PanelScrolled { offset })
        }

        /// The panel scroll that brings sidebar section `section` into
        /// view, when it is not already.
        fn reveal_section_scroll(&self, section: usize, layout: &SidebarLayout) -> Option<f32> {
            let s = layout.msv.section(layout.view_index(section)?)?;
            let bounds = layout.msv.chrome.bounds;
            let (_, max) = layout.msv.panel_extent();
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

    // ── Hosting without the runtime ────────────────────────────────────────────────

    impl Sidebar {
        /// Select `path` within `section`, expand the section if it is
        /// collapsed, and scroll its tree so `path` is visible, laid out
        /// at `rect` the way `backend` paints it.
        ///
        /// No-op if `section` is out of range, hosts a form, or `path` is
        /// not among its rows. The section is expanded before its
        /// viewport is measured.
        pub fn reveal(
            &mut self,
            section: usize,
            path: &TreePath,
            backend: &dyn Backend,
            rect: Rect,
        ) {
            use crate::backend::DrawsPrimitives;
            self.reveal_with(section, path, |s| backend.layout_primitive(rect, s));
        }

        /// [`Self::reveal`], laid out with `ctx` rather than a backend's
        /// metrics.
        pub fn reveal_in(&mut self, section: usize, path: &TreePath, ctx: &LayoutCtx, rect: Rect) {
            self.reveal_with(section, path, |s| Primitive::layout(s, ctx, rect));
        }

        fn reveal_with(
            &mut self,
            section: usize,
            path: &TreePath,
            layout: impl Fn(&Sidebar) -> SidebarLayout,
        ) {
            let Some(row) = self
                .tree(section)
                .and_then(|t| t.rows.iter().position(|r| &r.path == path))
            else {
                return;
            };
            self.sections[section].collapsed = false;
            let viewport = layout(self).viewport_rows(section);
            if let Some(tree) = self.tree_mut(section) {
                tree.selected_path = Some(path.clone());
                tree.scroll_offset =
                    crate::behaviour::scroll::ensure_visible(tree.scroll_offset, viewport, row);
            }
        }

        /// Drive the sidebar with a raw [`UiEvent`], laid out at `rect` the
        /// way `backend` paints it, for a host that does not route it
        /// through the runtime's `FrameTree`. The keyboard addresses the
        /// active section's body.
        ///
        /// Answers every [`SidebarAction`] the event produced, in order,
        /// for the host to [`Self::apply`]; [`Handled::Consumed`] when it
        /// was the sidebar's but changed nothing the host stores, and
        /// [`Handled::Ignored`] when it was not the sidebar's.
        pub fn handle_event(
            &self,
            state: &mut SidebarState,
            event: &UiEvent,
            backend: &dyn Backend,
            rect: Rect,
        ) -> Handled<Vec<SidebarAction>> {
            use crate::backend::DrawsPrimitives;
            let layout = backend.layout_primitive(rect, self);
            let units = crate::backend::primitive_layout_ctx(backend).units();
            self.drive(state, event, rect, &layout, units)
        }

        /// [`Self::handle_event`], laid out with `ctx` rather than a
        /// backend's metrics.
        pub fn handle_event_in(
            &self,
            state: &mut SidebarState,
            event: &UiEvent,
            ctx: &LayoutCtx,
            rect: Rect,
        ) -> Handled<Vec<SidebarAction>> {
            let layout = Primitive::layout(self, ctx, rect);
            self.drive(state, event, rect, &layout, ctx.units())
        }

        fn drive(
            &self,
            state: &mut SidebarState,
            event: &UiEvent,
            rect: Rect,
            layout: &SidebarLayout,
            units: crate::primitive::Units,
        ) -> Handled<Vec<SidebarAction>> {
            let subs = Interactive::sub_regions(self, layout);
            let Some(routed) = widget_host::routed_event(event, rect, &subs) else {
                return Handled::Ignored;
            };
            let focused = self
                .active_section
                .and_then(|s| self.view_index(s))
                .map(|i| MultiSectionViewSubPart::Body(i, None).sub_target());
            let driven =
                widget_host::drive_focused(self, self.id, state, &routed, layout, units, focused);
            let mut actions: Vec<SidebarAction> = driven
                .emitted
                .into_iter()
                .filter_map(|emitted| match emitted {
                    UiEvent::Sidebar(id, action) if id == self.id => Some(action),
                    _ => None,
                })
                .collect();
            match driven.handled {
                Handled::Event(action) => actions.push(action),
                Handled::Consumed if actions.is_empty() => return Handled::Consumed,
                Handled::Ignored if actions.is_empty() => return Handled::Ignored,
                _ => {}
            }
            Handled::Event(actions)
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

        /// Terminal-grid metrics.
        fn cells() -> LayoutCtx<'static> {
            LayoutCtx::new(1.0, 1.0, false)
        }

        /// A sidebar plus its state, driven through
        /// [`Sidebar::handle_event_in`] on a cell grid, applying every
        /// action it answers the way an app does.
        struct Host {
            sidebar: Sidebar,
            state: SidebarState,
        }

        impl Host {
            fn new(defs: Vec<SidebarSectionDef>) -> Self {
                Self {
                    sidebar: Sidebar::from_defs(WidgetId::new("sidebar"), &defs),
                    state: SidebarState::default(),
                }
            }

            fn handle(&mut self, event: &UiEvent, rect: Rect) -> Handled<Vec<SidebarAction>> {
                let handled = self
                    .sidebar
                    .handle_event_in(&mut self.state, event, &cells(), rect);
                if let Handled::Event(actions) = &handled {
                    for action in actions {
                        self.sidebar.apply(action);
                    }
                }
                handled
            }

            fn send(&mut self, event: &UiEvent, rect: Rect) {
                let _ = self.handle(event, rect);
            }

            /// The last action `event` produced, if any.
            fn last(&mut self, event: &UiEvent, rect: Rect) -> Option<SidebarAction> {
                match self.handle(event, rect) {
                    Handled::Event(mut actions) => actions.pop(),
                    _ => None,
                }
            }

            fn selected(&self, section: usize) -> Option<&TreePath> {
                self.sidebar.tree(section)?.selected_path.as_ref()
            }

            fn scroll(&self, section: usize) -> usize {
                self.sidebar.tree(section).map_or(0, |t| t.scroll_offset)
            }
        }

        fn layout(sidebar: &Sidebar, rect: Rect) -> SidebarLayout {
            Primitive::layout(sidebar, &cells(), rect)
        }

        /// The first body row of sidebar section `section`.
        fn body_top(sidebar: &Sidebar, section: usize, rect: Rect) -> Rect {
            let l = layout(sidebar, rect);
            let i = l.view_index(section).expect("section visible");
            l.msv.chrome.sections[i].body_bounds
        }

        fn header_of(sidebar: &Sidebar, section: usize, rect: Rect) -> Rect {
            let l = layout(sidebar, rect);
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

        fn selected_action(section: usize, path: TreePath) -> Option<SidebarAction> {
            Some(SidebarAction::RowSelected { section, path })
        }

        const RECT: Rect = Rect::new(0.0, 0.0, 30.0, 24.0);

        // ── Construction ────────────────────────────────────────────────

        #[test]
        fn from_defs_starts_with_no_active_section_and_one_body_per_def() {
            let sidebar = Sidebar::from_defs(
                WidgetId::new("s"),
                &[
                    SidebarSectionDef::new("a", "A"),
                    SidebarSectionDef::form("b", "B"),
                ],
            );
            assert_eq!(sidebar.active_section, None);
            assert!(sidebar.has_focus);
            assert_eq!(sidebar.navigation_mode, NavigationMode::Scroll);
            assert_eq!(sidebar.scroll_mode, ScrollMode::PerSection);
            assert_eq!(sidebar.tree(0).unwrap().id, WidgetId::new("a-tree"));
            assert_eq!(sidebar.form(1).unwrap().id, WidgetId::new("b-form"));
        }

        #[test]
        fn set_rows_updates_section_data() {
            let mut sidebar = Sidebar::from_defs(WidgetId::new("s"), &sample_defs());
            sidebar.set_rows(1, fake_rows("w", 4));
            assert_eq!(sidebar.tree(1).unwrap().rows.len(), 4);
            assert!(sidebar.tree(0).unwrap().rows.is_empty());
            assert_eq!(sidebar.tree(1).unwrap().selected_path, None);
        }

        #[test]
        fn a_panel_scroll_action_clamps_negative() {
            let mut sidebar = Sidebar::from_defs(WidgetId::new("s"), &sample_defs());
            sidebar.apply(&SidebarAction::PanelScrolled { offset: -5.0 });
            assert_eq!(sidebar.panel_scroll, 0.0);
        }

        // ── The view ────────────────────────────────────────────────────

        #[test]
        fn view_marks_the_active_section_and_skips_hidden_ones() {
            let mut sidebar = Sidebar::from_defs(WidgetId::new("s"), &sample_defs());
            sidebar.active_section = Some(2);
            sidebar.sections[1].visible = false;
            sidebar.sections[0].badge = Some(StyledText::plain("3"));
            let (view, map) = sidebar.view(CELL_ACTIVE_MARKER);
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
            let mut sidebar = Sidebar::from_defs(WidgetId::new("s"), &sample_defs());
            sidebar.active_section = Some(0);
            let pixel = Primitive::layout(&sidebar, &LayoutCtx::new(20.0, 8.0, false), RECT);
            let title = &pixel.view.sections[0].header.title.spans[0].text;
            assert!(title.starts_with(PIXEL_ACTIVE_MARKER), "{title:?}");
            let cell = layout(&sidebar, RECT);
            let title = &cell.view.sections[0].header.title.spans[0].text;
            assert!(title.starts_with(CELL_ACTIVE_MARKER), "{title:?}");
        }

        #[test]
        fn the_view_shares_the_sidebar_id_so_hover_reaches_its_bodies() {
            let sidebar = Sidebar::from_defs(WidgetId::new("s"), &sample_defs());
            let (view, _) = sidebar.view(CELL_ACTIVE_MARKER);
            assert_eq!(view.id, sidebar.id);
        }

        // ── Tab cycling ─────────────────────────────────────────────────

        #[test]
        fn tab_cycles_the_active_section_and_wraps() {
            let mut h = Host::new(sample_defs());
            for expected in [0, 1, 2, 0] {
                assert_eq!(
                    h.last(&named(NamedKey::Tab), RECT),
                    Some(SidebarAction::ActiveSectionChanged {
                        section: Some(expected)
                    })
                );
                assert_eq!(h.sidebar.active_section, Some(expected));
            }
            h.send(&named(NamedKey::BackTab), RECT);
            assert_eq!(h.sidebar.active_section, Some(2));
        }

        #[test]
        fn tab_skips_a_hidden_section() {
            let mut h = Host::new(sample_defs());
            h.sidebar.sections[1].visible = false;
            h.sidebar.active_section = Some(0);
            h.send(&named(NamedKey::Tab), RECT);
            assert_eq!(h.sidebar.active_section, Some(2));
        }

        // ── Scroll navigation mode ──────────────────────────────────────

        #[test]
        fn scroll_mode_down_scrolls_the_active_section_and_clamps() {
            let mut h = Host::new(vec![SidebarSectionDef::new("t", "T")]);
            h.sidebar.set_rows(0, fake_rows("r", 30));
            h.sidebar.active_section = Some(0);
            let rect = Rect::new(0.0, 0.0, 20.0, 11.0);
            h.send(&named(NamedKey::Down), rect);
            assert_eq!(h.scroll(0), 1);
            for _ in 0..40 {
                h.send(&named(NamedKey::Down), rect);
            }
            assert_eq!(h.scroll(0), 20, "clamped to the last page");
            assert_eq!(h.handle(&named(NamedKey::Down), rect), Handled::Consumed);
            h.send(&named(NamedKey::Up), rect);
            assert_eq!(h.scroll(0), 19);
        }

        #[test]
        fn scroll_mode_enter_selects_the_first_row_and_resets_scroll() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, fake_rows("v", 30));
            h.sidebar.active_section = Some(0);
            h.sidebar.tree_mut(0).unwrap().scroll_offset = 4;
            assert_eq!(
                h.last(&named(NamedKey::Enter), RECT),
                selected_action(0, vec![0])
            );
            assert_eq!(h.selected(0), Some(&vec![0]));
            assert_eq!(h.scroll(0), 0);
        }

        #[test]
        fn space_on_a_selected_branch_toggles_it_and_ignores_a_leaf() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, branch_rows());
            h.sidebar.active_section = Some(0);
            h.sidebar.tree_mut(0).unwrap().selected_path = Some(vec![1]);
            let toggle = Some(SidebarAction::RowToggleExpand {
                section: 0,
                path: vec![1],
            });
            assert_eq!(h.sidebar.toggle_expand_selected(), toggle);
            assert_eq!(h.last(&key(Key::Char(' ')), RECT), toggle);
            h.sidebar.tree_mut(0).unwrap().selected_path = Some(vec![2]);
            assert_eq!(h.handle(&key(Key::Char(' ')), RECT), Handled::Ignored);
            assert_eq!(h.sidebar.toggle_expand_selected(), None);
        }

        #[test]
        fn whole_panel_mode_scrolls_the_panel_by_one_row_per_key() {
            let mut h = Host::new(sample_defs());
            for i in 0..3 {
                h.sidebar.set_rows(i, fake_rows("r", 20));
            }
            h.sidebar.scroll_mode = ScrollMode::WholePanel;
            h.sidebar.active_section = Some(0);
            let rect = Rect::new(0.0, 0.0, 20.0, 10.0);
            h.send(&named(NamedKey::Down), rect);
            assert_eq!(h.sidebar.panel_scroll, 1.0);
        }

        // ── Selection navigation mode ───────────────────────────────────

        fn selection_sidebar() -> Host {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, fake_rows("v", 30));
            h.sidebar.set_rows(1, fake_rows("w", 3));
            h.sidebar.navigation_mode = NavigationMode::Selection;
            h.sidebar.active_section = Some(0);
            h
        }

        fn down(h: &mut Host) -> Option<SidebarAction> {
            h.last(&named(NamedKey::Down), RECT)
        }

        #[test]
        fn selection_down_selects_the_first_row_then_advances() {
            let mut h = selection_sidebar();
            assert_eq!(down(&mut h), selected_action(0, vec![0]));
            assert_eq!(down(&mut h), selected_action(0, vec![1]));
            h.send(&named(NamedKey::Up), RECT);
            assert_eq!(h.selected(0), Some(&vec![0]));
        }

        #[test]
        fn selection_home_end_and_enter() {
            let mut h = selection_sidebar();
            h.send(&named(NamedKey::End), RECT);
            assert_eq!(h.selected(0), Some(&vec![29]));
            h.send(&named(NamedKey::Home), RECT);
            assert_eq!(h.selected(0), Some(&vec![0]));
            assert_eq!(
                h.last(&named(NamedKey::Enter), RECT),
                Some(SidebarAction::RowActivated {
                    section: 0,
                    path: vec![0]
                })
            );
        }

        #[test]
        fn selection_scroll_follows_a_row_below_the_viewport() {
            let mut h = selection_sidebar();
            h.send(&named(NamedKey::End), RECT);
            let viewport = layout(&h.sidebar, RECT).viewport_rows(0);
            assert!(viewport > 0);
            assert_eq!(h.scroll(0), 30 - viewport);
        }

        #[test]
        fn selection_in_a_collapsed_or_missing_section_is_ignored() {
            let mut h = selection_sidebar();
            h.sidebar.sections[0].collapsed = true;
            assert_eq!(h.handle(&named(NamedKey::Down), RECT), Handled::Ignored);
            h.sidebar.sections[0].collapsed = false;
            h.sidebar.active_section = None;
            assert_eq!(h.handle(&named(NamedKey::Down), RECT), Handled::Ignored);
        }

        #[test]
        fn selection_leaves_unbound_keys_to_the_app() {
            let mut h = selection_sidebar();
            assert_eq!(h.handle(&key(Key::Char('q')), RECT), Handled::Ignored);
            // A letter that would type-ahead to a row is still the app's.
            assert_eq!(h.handle(&key(Key::Char('v')), RECT), Handled::Ignored);
            assert_eq!(h.selected(0), None);
        }

        #[test]
        fn selection_left_and_right_fold_the_selected_branch() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, branch_rows());
            h.sidebar.navigation_mode = NavigationMode::Selection;
            h.sidebar.active_section = Some(0);
            h.sidebar.tree_mut(0).unwrap().selected_path = Some(vec![1]);
            assert_eq!(
                h.last(&named(NamedKey::Right), RECT),
                Some(SidebarAction::RowToggleExpand {
                    section: 0,
                    path: vec![1]
                })
            );
            h.sidebar.tree_mut(0).unwrap().selected_path = Some(vec![0]);
            assert_eq!(
                h.last(&named(NamedKey::Left), RECT),
                Some(SidebarAction::RowToggleExpand {
                    section: 0,
                    path: vec![0]
                })
            );
        }

        // ── Pointer ─────────────────────────────────────────────────────

        #[test]
        fn clicking_a_row_selects_it_and_activates_its_section() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(1, fake_rows("w", 3));
            let body = body_top(&h.sidebar, 1, RECT);
            let ev = h.last(&press(MouseButton::Left, body.x + 4.0, body.y + 1.0), RECT);
            assert_eq!(ev, selected_action(1, vec![1]));
            assert_eq!(h.sidebar.active_section, Some(1));
            assert_eq!(h.selected(1), Some(&vec![1]));
        }

        /// On a cell grid the chevron is the two cells before a branch's
        /// title, so a click on the title selects the row rather than
        /// expanding it.
        #[cfg(feature = "tui")]
        #[test]
        fn clicking_a_branch_title_on_cells_selects_rather_than_toggles() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, branch_rows());
            let body = body_top(&h.sidebar, 0, RECT);
            let ev = h.last(&press(MouseButton::Left, body.x + 4.0, body.y + 1.0), RECT);
            assert_eq!(ev, selected_action(0, vec![1]));
            let ev = h.last(&press(MouseButton::Left, body.x, body.y + 1.0), RECT);
            assert_eq!(
                ev,
                Some(SidebarAction::RowToggleExpand {
                    section: 0,
                    path: vec![1]
                })
            );
        }

        #[test]
        fn double_clicking_a_row_activates_it() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, fake_rows("v", 3));
            let body = body_top(&h.sidebar, 0, RECT);
            let ev = h.last(
                &UiEvent::DoubleClick {
                    widget: None,
                    button: crate::MouseButton::Left,
                    position: Point::new(body.x + 3.0, body.y + 2.0),
                },
                RECT,
            );
            assert_eq!(
                ev,
                Some(SidebarAction::RowActivated {
                    section: 0,
                    path: vec![2]
                })
            );
        }

        #[test]
        fn clicking_a_body_below_its_rows_activates_the_section() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, fake_rows("v", 2));
            h.sidebar.tree_mut(0).unwrap().selected_path = Some(vec![1]);
            let body = body_top(&h.sidebar, 0, RECT);
            let ev = h.last(&press(MouseButton::Left, body.x + 3.0, body.y + 4.0), RECT);
            assert_eq!(ev, Some(SidebarAction::HeaderActivated { section: 0 }));
            assert_eq!(h.selected(0), None);
        }

        #[test]
        fn right_clicking_a_row_requests_its_context_menu() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, fake_rows("v", 3));
            let body = body_top(&h.sidebar, 0, RECT);
            let at = Point::new(body.x + 3.0, body.y);
            assert_eq!(
                h.last(&press(MouseButton::Right, at.x, at.y), RECT),
                Some(SidebarAction::ContextMenuRequested {
                    section: 0,
                    path: vec![0],
                    position: at,
                })
            );
            assert_eq!(h.selected(0), Some(&vec![0]));
        }

        #[test]
        fn empty_space_context_menu_is_opt_in_per_section() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, fake_rows("v", 2));
            let body = body_top(&h.sidebar, 0, RECT);
            let at = Point::new(body.x + 3.0, body.y + 4.0);
            assert_eq!(h.last(&press(MouseButton::Right, at.x, at.y), RECT), None);
            h.sidebar.sections[0].empty_space_context_menu = true;
            assert_eq!(
                h.last(&press(MouseButton::Right, at.x, at.y), RECT),
                Some(SidebarAction::ContextMenuRequested {
                    section: 0,
                    path: TreePath::new(),
                    position: at,
                })
            );
        }

        // ── Headers ─────────────────────────────────────────────────────

        #[test]
        fn a_header_click_activates_and_collapses_only_when_allowed() {
            let mut h = Host::new(sample_defs());
            let hdr = header_of(&h.sidebar, 1, RECT);
            let ev = h.last(&press(MouseButton::Left, hdr.x + 2.0, hdr.y), RECT);
            assert_eq!(ev, Some(SidebarAction::HeaderActivated { section: 1 }));
            assert_eq!(h.sidebar.active_section, Some(1));
            assert!(!h.sidebar.sections[1].collapsed);

            h.sidebar.allow_collapse = true;
            h.send(&press(MouseButton::Left, hdr.x + 2.0, hdr.y), RECT);
            assert!(h.sidebar.sections[1].collapsed);
            let hdr = header_of(&h.sidebar, 1, RECT);
            h.send(&press(MouseButton::Left, hdr.x + 2.0, hdr.y), RECT);
            assert!(!h.sidebar.sections[1].collapsed);
        }

        // ── Wheel ───────────────────────────────────────────────────────

        fn scroll_routing_sidebar() -> Host {
            let mut h = Host::new(sample_defs());
            for i in 0..3 {
                h.sidebar.set_rows(i, fake_rows("r", 30));
            }
            h.sidebar.active_section = Some(0);
            h
        }

        #[test]
        fn the_wheel_scrolls_the_section_under_the_pointer() {
            let mut h = scroll_routing_sidebar();
            let body = body_top(&h.sidebar, 2, RECT);
            h.send(&wheel(-1.0, body.x + 2.0, body.y + 1.0), RECT);
            assert!(h.scroll(2) > 0);
            assert_eq!(h.scroll(0), 0);
        }

        #[test]
        fn the_wheel_outside_every_section_scrolls_the_active_one() {
            let mut h = scroll_routing_sidebar();
            h.send(&wheel(-1.0, 500.0, 500.0), RECT);
            assert_eq!(h.scroll(0), 1);
        }

        // ── Scrollbars ──────────────────────────────────────────────────

        #[test]
        fn a_one_cell_drag_on_a_section_thumb_scrolls_it() {
            let mut h = Host::new(vec![SidebarSectionDef::new("t", "T")]);
            h.sidebar.set_rows(0, fake_rows("r", 30));
            h.sidebar.active_section = Some(0);
            let rect = Rect::new(0.0, 0.0, 20.0, 21.0);
            let l = layout(&h.sidebar, rect);
            let sb = l.msv.chrome.sections[0].scrollbar_bounds.expect("gutter");
            let thumb = l.msv.chrome.sections[0].thumb_bounds.expect("thumb");
            h.send(&press(MouseButton::Left, sb.x, thumb.y), rect);
            h.send(
                &UiEvent::MouseMoved {
                    position: Point::new(sb.x, thumb.y + 1.0),
                    buttons: ButtonMask {
                        left: true,
                        ..Default::default()
                    },
                },
                rect,
            );
            assert!(h.scroll(0) > 0);
        }

        #[test]
        fn a_panel_thumb_drag_scrolls_the_panel() {
            let mut h = Host::new(sample_defs());
            for i in 0..3 {
                h.sidebar.set_rows(i, fake_rows("r", 8));
            }
            for s in &mut h.sidebar.sections {
                s.size = SectionSize::Content;
            }
            h.sidebar.scroll_mode = ScrollMode::WholePanel;
            let rect = Rect::new(0.0, 0.0, 20.0, 12.0);
            let l = layout(&h.sidebar, rect);
            let thumb = l.msv.chrome.panel_scrollbar_thumb.expect("panel overflows");
            h.send(&press(MouseButton::Left, thumb.x, thumb.y), rect);
            h.send(
                &UiEvent::MouseMoved {
                    position: Point::new(thumb.x, thumb.y + 2.0),
                    buttons: ButtonMask {
                        left: true,
                        ..Default::default()
                    },
                },
                rect,
            );
            assert!(h.sidebar.panel_scroll > 0.0);
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

        fn mixed() -> Host {
            Host::new(vec![
                SidebarSectionDef::form("search", "SEARCH"),
                SidebarSectionDef::new("results", "RESULTS"),
            ])
        }

        #[test]
        fn form_and_tree_setters_only_reach_their_own_kind() {
            let mut h = mixed();
            assert!(h.sidebar.form(0).is_some());
            assert!(h.sidebar.form_mut(1).is_none());
            h.sidebar.set_rows(0, fake_rows("x", 2));
            assert!(h.sidebar.tree(0).is_none());
            *h.sidebar.form_mut(0).unwrap() =
                form(vec![field("q", "Q", FieldKind::Toggle { value: false })]);
            assert_eq!(h.sidebar.form(0).unwrap().fields.len(), 1);
        }

        #[test]
        fn a_focused_form_paints_focused_while_a_tree_section_is_active() {
            let mut h = mixed();
            h.sidebar.active_section = Some(1);
            let form_focus = |s: &Sidebar| {
                let (view, _) = s.view(CELL_ACTIVE_MARKER);
                let SectionBody::Form(f) = &view.sections[0].body else {
                    panic!("form body");
                };
                f.has_focus
            };
            assert!(!form_focus(&h.sidebar));
            h.sidebar.sections[0].form_focused = true;
            assert!(form_focus(&h.sidebar));
        }

        #[test]
        fn clicking_a_toggle_in_a_form_section_flips_it_through_the_form() {
            let mut h = mixed();
            *h.sidebar.form_mut(0).unwrap() = form(vec![field(
                "case",
                "Case",
                FieldKind::Toggle { value: false },
            )]);
            let body = body_top(&h.sidebar, 0, RECT);
            let ev = h.last(&press(MouseButton::Left, body.x + 1.0, body.y), RECT);
            assert_eq!(
                ev,
                Some(SidebarAction::Form {
                    section: 0,
                    event: FormEvent::ToggleChanged {
                        id: WidgetId::new("case"),
                        value: true,
                    },
                })
            );
            assert_eq!(h.sidebar.active_section, Some(0));
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
                let mut h = mixed();
                *h.sidebar.form_mut(0).unwrap() =
                    form(vec![field("count", "Count", FieldKind::NumberInput(count))]);
                h
            };
            let probe = number_sidebar();
            let body = body_top(&probe.sidebar, 0, RECT);
            let stepped = (0..body.width as usize).any(|dx| {
                let mut h = number_sidebar();
                let ev = h.last(&press(MouseButton::Left, body.x + dx as f32, body.y), RECT);
                matches!(
                    ev,
                    Some(SidebarAction::Form {
                        section: 0,
                        event: FormEvent::NumberChanged { value, .. },
                    }) if value == 6.0
                )
            });
            assert!(stepped, "no press on the number row stepped it up");
        }

        #[test]
        fn selection_keys_in_a_form_section_are_ignored() {
            let mut h = mixed();
            *h.sidebar.form_mut(0).unwrap() = form(vec![field(
                "case",
                "Case",
                FieldKind::Toggle { value: false },
            )]);
            h.sidebar.navigation_mode = NavigationMode::Selection;
            h.sidebar.active_section = Some(0);
            assert_eq!(h.handle(&named(NamedKey::Down), RECT), Handled::Ignored);
        }

        // ── Reveal and rename ───────────────────────────────────────────

        #[test]
        fn reveal_expands_selects_and_scrolls_to_the_row() {
            let mut sidebar =
                Sidebar::from_defs(WidgetId::new("s"), &[SidebarSectionDef::new("t", "T")]);
            sidebar.set_rows(0, fake_rows("r", 40));
            sidebar.sections[0].collapsed = true;
            let rect = Rect::new(0.0, 0.0, 20.0, 11.0);
            sidebar.reveal_in(0, &vec![30], &cells(), rect);
            assert!(!sidebar.sections[0].collapsed);
            let tree = sidebar.tree(0).unwrap();
            assert_eq!(tree.selected_path, Some(vec![30]));
            let viewport = layout(&sidebar, rect).viewport_rows(0);
            let offset = tree.scroll_offset;
            assert!(
                offset <= 30 && 30 < offset + viewport,
                "{offset} {viewport}"
            );
        }

        #[test]
        fn reveal_of_a_missing_row_changes_nothing() {
            let mut sidebar =
                Sidebar::from_defs(WidgetId::new("s"), &[SidebarSectionDef::new("t", "T")]);
            sidebar.set_rows(0, fake_rows("r", 3));
            sidebar.sections[0].collapsed = true;
            sidebar.reveal_in(0, &vec![9], &cells(), RECT);
            assert!(sidebar.sections[0].collapsed);
            assert_eq!(sidebar.tree(0).unwrap().selected_path, None);
        }

        #[test]
        fn a_rename_survives_set_rows_and_reports_each_edit() {
            let mut h = Host::new(sample_defs());
            h.sidebar.set_rows(0, fake_rows("v", 3));
            h.sidebar.active_section = Some(0);
            h.sidebar
                .start_editing(0, vec![1], "ab".into(), 2, None, None);
            h.sidebar.set_rows(0, fake_rows("v", 3));
            assert!(h.sidebar.is_editing());
            assert_eq!(
                h.last(&UiEvent::CharTyped('c'), RECT),
                Some(SidebarAction::EditChanged {
                    section: 0,
                    path: vec![1],
                    text: "abc".into(),
                    cursor: 3,
                    selection_anchor: None,
                })
            );
            h.sidebar.set_rows(0, fake_rows("v", 3));
            assert_eq!(
                h.last(&named(NamedKey::Enter), RECT),
                Some(SidebarAction::EditConfirmed {
                    section: 0,
                    path: vec![1],
                    new_text: "abc".into(),
                })
            );
            assert!(!h.sidebar.is_editing());
        }

        #[test]
        fn a_rename_started_before_its_row_exists_lands_when_the_row_arrives() {
            let mut sidebar = Sidebar::from_defs(WidgetId::new("s"), &sample_defs());
            sidebar.set_rows(0, fake_rows("v", 1));
            sidebar.start_editing(0, vec![1], String::new(), 0, None, Some("name".into()));
            assert!(sidebar.is_editing());
            sidebar.set_rows(0, fake_rows("v", 2));
            let row = &sidebar.tree(0).unwrap().rows[1];
            assert_eq!(row.path, vec![1]);
            assert_eq!(
                row.edit,
                Some(TreeRowEditState {
                    text: String::new(),
                    cursor: 0,
                    selection_anchor: None,
                    placeholder: Some("name".into()),
                })
            );
            sidebar.set_rows(0, fake_rows("v", 1));
            sidebar.set_rows(0, fake_rows("v", 2));
            assert!(
                sidebar.tree(0).unwrap().rows[1].edit.is_some(),
                "a frame that briefly leaves the row out keeps the rename"
            );
        }

        #[test]
        fn cancel_editing_ends_the_session() {
            let mut sidebar = Sidebar::from_defs(WidgetId::new("s"), &sample_defs());
            sidebar.set_rows(0, fake_rows("v", 3));
            sidebar.start_editing(0, vec![0], "x".into(), 1, None, None);
            sidebar.cancel_editing(0);
            assert!(!sidebar.is_editing());
            assert!(sidebar
                .tree(0)
                .unwrap()
                .rows
                .iter()
                .all(|r| r.edit.is_none()));
        }

        // ── Widget contract ─────────────────────────────────────────────

        #[test]
        fn focus_lists_every_header_then_the_active_body() {
            let mut sidebar = Sidebar::from_defs(WidgetId::new("s"), &sample_defs());
            sidebar.set_rows(1, fake_rows("w", 2));
            sidebar.active_section = Some(1);
            let FocusSpec::Children(stops) = Interactive::focus(&sidebar) else {
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
            let mut sidebar = Sidebar::from_defs(WidgetId::new("s"), &sample_defs());
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
