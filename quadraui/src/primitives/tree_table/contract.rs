//! [`TreeTable`] on the widget contract: its `Primitive` layout and
//! paint, and its controlled `Interactive` behaviour.
//!
//! Tree-specific gestures — keyboard navigation through
//! [`TreeNavCore`], chevrons, lazy-load requests, the scrollbar and the
//! wheel — are handled here. Every other pointer gesture (header clicks,
//! divider drags, row selection, context menus) and every key the tree
//! navigation leaves alone (type-to-jump, select-all) is forwarded to the
//! wrapped [`DataTable`]'s own `Interactive` impl, and its answer is
//! re-addressed as a [`TreeTableEvent`].

use super::{TreeTable, TreeTableEvent, TreeTableHit, TreeTableLayout};
use crate::behaviour::nav::{HorizontalNav, NavOutcome, ScrollbarPress, TreeKey, TreeNavCore};
use crate::event::Rect;
use crate::interactive::{
    EventCx, EventHost, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    WidgetAction,
};
use crate::primitive::{LayoutCtx, Primitive, Units};
use crate::primitives::data_table::{
    DataTableEvent, DataTableHit, DataTablePaintState, DataTablePrimitiveLayout, DataTableState,
    DataTableSubPart, SortDirection,
};
use crate::types::{SelectionGesture, TreePath, WidgetId};
use crate::{Handled, PaintEnv, PointerShape, UiEvent};

/// [`Primitive::Layout`] for [`TreeTable`]: the resolved
/// [`TreeTableLayout`] paired with the absolute area it was laid out
/// into plus the row count and scroll offset
/// [`TreeTableLayout::hit_test`] needs but doesn't itself carry.
///
/// `#[non_exhaustive]`: backend-returned, never constructed by a
/// consumer via struct literal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TreeTablePrimitiveLayout {
    /// Absolute area the table was laid out into; the table-local
    /// [`Self::layout`] is relative to its origin.
    pub area: Rect,
    /// Row height resolved from the [`LayoutCtx`] the layout was called
    /// with.
    pub line_height: f32,
    /// Nerd-Font toggle resolved the same way.
    pub nerd_fonts_enabled: bool,
    /// The table-local layout itself.
    pub layout: TreeTableLayout,
    /// The first painted row: `table.effective_scroll_offset` for the
    /// paint state's `scroll_px` at layout time.
    pub scroll_offset: usize,
    /// `table.rows.len()` at layout time.
    pub total_rows: usize,
}

impl TreeTablePrimitiveLayout {
    /// Hit-test at an absolute `(x, y)` — subtracts [`Self::area`]'s
    /// origin before delegating to [`TreeTableLayout::hit_test`].
    pub fn hit_test(&self, x: f32, y: f32) -> TreeTableHit {
        self.layout.hit_test(
            x - self.area.x,
            y - self.area.y,
            self.scroll_offset,
            self.total_rows,
        )
    }

    /// The wrapped [`crate::DataTable`]'s own primitive layout, which its
    /// `Interactive` impl handles a forwarded event against.
    pub fn table_layout(&self) -> DataTablePrimitiveLayout {
        DataTablePrimitiveLayout {
            area: self.area,
            line_height: self.line_height,
            nerd_fonts_enabled: self.nerd_fonts_enabled,
            layout: self.layout.table.clone(),
            scroll_offset: self.scroll_offset,
            total_rows: self.total_rows,
        }
    }

    /// Rows that fit the body.
    fn visible_rows(&self) -> usize {
        self.layout.table.visible_rows
    }
}

/// One addressable part of a laid-out [`TreeTable`], packed into a
/// [`SubTarget`]: one of the wrapped table's own parts, or a row's
/// chevron.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeTableSubPart {
    /// A part of the wrapped [`crate::DataTable`], encoded the way it
    /// encodes it.
    Table(DataTableSubPart),
    /// Row `idx`'s expand/collapse chevron.
    Chevron(usize),
}

/// The [`SubTarget`] kind a chevron is packed under, clear of every kind
/// [`DataTableSubPart`] uses.
const CHEVRON_KIND: u64 = 16;

impl TreeTableSubPart {
    /// This part as a [`SubTarget`].
    pub fn sub_target(self) -> SubTarget {
        match self {
            Self::Table(part) => part.sub_target(),
            Self::Chevron(i) => SubTarget((CHEVRON_KIND << 32) | (i as u64 & 0xffff_ffff)),
        }
    }

    /// The part `sub` names, or `None` for a target this widget never
    /// produces.
    pub fn from_sub_target(sub: SubTarget) -> Option<Self> {
        if sub.0 >> 32 == CHEVRON_KIND {
            return Some(Self::Chevron((sub.0 & 0xffff_ffff) as usize));
        }
        DataTableSubPart::from_sub_target(sub).map(Self::Table)
    }

    /// The body row this part belongs to, if any.
    pub fn row(self) -> Option<usize> {
        match self {
            Self::Table(part) => part.row(),
            Self::Chevron(i) => Some(i),
        }
    }
}

impl TreeTable {
    /// The row paint state for one paint: the wrapped table's own (hover
    /// and press from the runtime, stripes from the descriptor), with a
    /// hovered or pressed chevron counting as its row.
    fn row_paint_state(&self, env: &PaintEnv) -> DataTablePaintState {
        let chevron_row =
            |sub: Option<SubTarget>| match sub.and_then(TreeTableSubPart::from_sub_target) {
                Some(TreeTableSubPart::Chevron(i)) if i < self.table.rows.len() => Some(i),
                _ => None,
            };
        let mut state = self.table.row_paint_state(env);
        if let Some(i) = chevron_row(env.hovered_sub(&self.table.id)) {
            state = state.with_hovered_idx(Some(i));
        }
        if let Some(i) = chevron_row(env.pressed_sub(&self.table.id)) {
            state = state.with_pressed_idx(Some(i));
        }
        state
    }
}

impl Primitive for TreeTable {
    type Layout = TreeTablePrimitiveLayout;
    type Event = TreeTableEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.table.id)
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        // The paint state's `scroll_px` places rows and chevrons where
        // the painters draw them.
        let scroll_px = ctx.paint_state().scroll_px();
        let layout = if ctx.units() == Units::Cells {
            super::layout::cell_layout(self, rect.width, rect.height, scroll_px)
        } else {
            // Titles and chevrons measure through the backend's own text
            // measurer, the metric the painter positions text with.
            let measure = crate::primitives::layout_metrics::LayoutCtxMeasure {
                ctx,
                role: crate::FontRole::Chrome,
            };
            super::layout::pixel_layout(
                self,
                rect.width,
                rect.height,
                ctx.line_height,
                &measure,
                scroll_px,
            )
        };
        TreeTablePrimitiveLayout {
            area: rect,
            line_height: ctx.line_height,
            nerd_fonts_enabled: ctx.nerd_fonts_enabled,
            layout,
            scroll_offset: self
                .table
                .effective_scroll_offset(ctx.line_height, scroll_px),
            total_rows: self.table.rows.len(),
        }
    }

    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &PaintEnv,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let state = self.row_paint_state(env);
            super::native_surface_paint::paint_laid_out(
                self,
                surface,
                env.theme(),
                layout.area,
                layout.line_height,
                &state,
                layout.nerd_fonts_enabled,
                layout.layout.clone(),
            );
        }
        // A `tui`-only build has no `PaintSurface` to paint into; cells
        // paint through `Self::paint_cells`.
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, env);
        }
    }

    /// Paints through `DataTable`'s cell rasteriser.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn crate::CellSurface,
        env: &PaintEnv,
    ) {
        let state = self.row_paint_state(env);
        super::paint_cells::paint_laid_out(
            cells,
            crate::tui::backend::q_rect_to_ratatui(layout.area),
            self,
            env.theme(),
            &state,
            layout.nerd_fonts_enabled,
            layout.layout.clone(),
        );
    }
}

// ── Interactive ────────────────────────────────────────────────────────────
//
// Controlled mode: the rows (with each branch's expand state), the
// selection, the scroll offset and the sort stay descriptor fields the
// app owns. `handle` computes the next value from the descriptor and
// reports it as a `TreeTableEvent`; `TreeTableState` holds only gesture
// data.

impl WidgetAction for TreeTableEvent {
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::TreeTable(id, self)
    }
}

/// [`Interactive::State`] for [`TreeTable`].
///
/// `nav` is the navigation scratch every keyboard move is computed in and
/// carries a scrollbar thumb drag; it is re-seeded from the descriptor's
/// selection and scroll offset whenever those differ from what it last
/// produced. `table` is the wrapped table's own gesture state for the
/// events forwarded to it.
///
/// `#[non_exhaustive]`: a future gesture field is additive.
#[non_exhaustive]
#[derive(Debug, Default)]
pub struct TreeTableState {
    /// Selection/scroll scratch and the scrollbar thumb drag.
    pub nav: TreeNavCore,
    /// The wrapped table's gesture state: multi-selection, type-to-jump
    /// buffer and an in-progress column resize.
    pub table: DataTableState,
    /// Pending branches whose children were already requested, so a
    /// repeated expand does not ask again before the app answers.
    pub requested: Vec<TreePath>,
}

impl TreeTableState {
    /// Re-seed the navigation core from `tt`'s controlled fields and drop
    /// requests the app has answered.
    fn sync(&mut self, tt: &TreeTable) {
        let selected = tt
            .table
            .selected_idx
            .and_then(|idx| tt.tree.get(idx))
            .map(|n| n.path.clone());
        if self.nav.selected_path() != selected.as_ref() {
            self.nav.set_selected_path(selected);
        }
        if self.nav.scroll_offset() != tt.table.scroll_offset && !self.nav.is_dragging() {
            self.nav.set_scroll_offset(tt.table.scroll_offset);
        }
        self.requested.retain(|path| {
            tt.tree
                .iter()
                .any(|n| &n.path == path && n.children_pending)
        });
    }
}

/// The [`EventHost`] the wrapped table handles a forwarded event against:
/// focus and capture go to this widget, and the table's emitted events
/// are kept so they can be re-addressed as [`TreeTableEvent`]s.
struct TableHost<'a, 'b> {
    parent: &'a mut EventCx<'b>,
    emitted: Vec<UiEvent>,
}

impl EventHost for TableHost<'_, '_> {
    fn is_focused(&self, _id: &WidgetId, sub: Option<SubTarget>) -> bool {
        self.parent.is_focused(sub)
    }
    fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
        self.parent.request_focus(sub);
    }
    fn capture_pointer(&mut self, _id: &WidgetId) {
        self.parent.capture_pointer();
    }
    fn release_pointer(&mut self) {
        self.parent.release_pointer();
    }
    fn set_cursor(&mut self, cursor: PointerShape) {
        self.parent.set_cursor(cursor);
    }
    fn request_redraw(&mut self) {
        self.parent.request_redraw();
    }
    fn emit(&mut self, event: UiEvent) {
        self.emitted.push(event);
    }
    fn units(&self) -> Units {
        self.parent.units()
    }
}

impl TreeTable {
    /// Whether row `idx` is a loading placeholder.
    fn is_placeholder(&self, idx: usize) -> bool {
        self.tree.get(idx).is_some_and(|n| n.placeholder)
    }

    /// Row `idx`'s tree path; a row past `tree`'s end has the empty path.
    fn path_of(&self, idx: usize) -> TreePath {
        self.tree
            .get(idx)
            .map(|n| n.path.clone())
            .unwrap_or_default()
    }

    /// The row index of `path`.
    fn index_of(&self, path: &TreePath) -> Option<usize> {
        self.tree.iter().position(|n| &n.path == path)
    }

    /// The rows keyboard navigation walks: every row but the loading
    /// placeholders.
    fn nav_rows(&self) -> Vec<super::TreeTableNode> {
        self.tree
            .iter()
            .filter(|n| !n.placeholder)
            .cloned()
            .collect()
    }

    /// Report a scroll offset change, or just consume when it did not
    /// move.
    fn scroll_outcome(&self, before: usize, state: &TreeTableState) -> Handled<TreeTableEvent> {
        let after = state.nav.scroll_offset();
        if after == before {
            Handled::Consumed
        } else {
            Handled::Event(TreeTableEvent::ScrollChanged {
                scroll_offset: after,
            })
        }
    }

    /// Select row `idx` and bring it into view, reporting the scroll move
    /// alongside the selection.
    fn select_idx(
        &self,
        state: &mut TreeTableState,
        idx: usize,
        layout: &TreeTablePrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<TreeTableEvent> {
        let path = self.path_of(idx);
        state.nav.set_selected_path(Some(path.clone()));
        state
            .table
            .selection
            .on_gesture(Some(idx), idx, SelectionGesture::Replace);
        state.table.cursor = Some(idx);
        let scroll_offset = crate::behaviour::scroll::ensure_visible(
            self.table.scroll_offset,
            layout.visible_rows(),
            idx,
        );
        state.nav.set_scroll_offset(scroll_offset);
        if scroll_offset != self.table.scroll_offset {
            cx.emit(TreeTableEvent::ScrollChanged { scroll_offset }.into_ui_event(self.table.id));
        }
        if self.table.selected_idx == Some(idx) {
            Handled::Consumed
        } else {
            Handled::Event(TreeTableEvent::RowSelected { idx, path })
        }
    }

    /// Expand or collapse the branch at `path`. Expanding a branch whose
    /// children are pending also requests them, once.
    fn toggle(
        &self,
        state: &mut TreeTableState,
        path: TreePath,
        expand: bool,
        cx: &mut EventCx,
    ) -> Handled<TreeTableEvent> {
        let Some(idx) = self.index_of(&path) else {
            return Handled::Ignored;
        };
        let node = &self.tree[idx];
        if node.placeholder || node.is_expanded.is_none() {
            return Handled::Consumed;
        }
        if expand && node.children_pending && !state.requested.contains(&path) {
            state.requested.push(path.clone());
            cx.emit(
                TreeTableEvent::ChildrenRequested {
                    idx,
                    path: path.clone(),
                }
                .into_ui_event(self.table.id),
            );
        }
        Handled::Event(TreeTableEvent::ToggleExpanded { path, expand })
    }

    /// One keystroke while the table has focus: tree navigation first,
    /// then whatever the wrapped table does with it.
    fn handle_key(
        &self,
        state: &mut TreeTableState,
        event: &KeyEvent,
        layout: &TreeTablePrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<TreeTableEvent> {
        let rows = self.nav_rows();
        let before = state.nav.scroll_offset();
        match state
            .nav
            .handle_key(&rows, event, layout.visible_rows(), self.vim_keys)
        {
            TreeKey::Moved(NavOutcome::Selected(path)) => {
                // The core scrolled against the placeholder-free rows;
                // `select_idx` scrolls against the real row index.
                state.nav.set_scroll_offset(before);
                match self.index_of(&path) {
                    Some(idx) => self.select_idx(state, idx, layout, cx),
                    None => Handled::Ignored,
                }
            }
            TreeKey::Moved(NavOutcome::Unchanged) => Handled::Consumed,
            TreeKey::Moved(NavOutcome::Empty) => Handled::Ignored,
            TreeKey::Horizontal(HorizontalNav::Collapse(path)) => {
                self.toggle(state, path, false, cx)
            }
            TreeKey::Horizontal(HorizontalNav::Expand(path)) => self.toggle(state, path, true, cx),
            TreeKey::Horizontal(
                HorizontalNav::SelectParent(path) | HorizontalNav::SelectFirstChild(path),
            ) => match self.index_of(&path) {
                Some(idx) => self.select_idx(state, idx, layout, cx),
                None => Handled::Ignored,
            },
            TreeKey::Horizontal(HorizontalNav::None) => Handled::Consumed,
            TreeKey::Activate(Some(path)) => match self.index_of(&path) {
                Some(idx) => Handled::Event(TreeTableEvent::RowActivated { idx, path }),
                None => Handled::Ignored,
            },
            TreeKey::Activate(None) => Handled::Ignored,
            TreeKey::ToggleExpand(Some(path)) => {
                let expanded = self
                    .index_of(&path)
                    .and_then(|i| self.tree[i].is_expanded)
                    .unwrap_or(false);
                self.toggle(state, path, !expanded, cx)
            }
            TreeKey::ToggleExpand(None) => Handled::Consumed,
            TreeKey::Unhandled => self.forward(state, &RoutedEvent::Key(event.clone()), layout, cx),
        }
    }

    /// Left press on the scrollbar track, at `y` in widget-local space.
    fn press_scrollbar(
        &self,
        state: &mut TreeTableState,
        y: f32,
        layout: &TreeTablePrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<TreeTableEvent> {
        let Some(track) = layout.layout.table.v_scrollbar_track else {
            return Handled::Consumed;
        };
        let total = self.table.rows.len();
        let visible = layout.visible_rows();
        let sb = crate::primitives::row_stack::vscrollbar_thumb(
            self.table.id,
            track,
            self.table.scroll_offset,
            None,
            total,
            layout.layout.table.row_height,
            visible as f32,
        );
        let before = state.nav.scroll_offset();
        match state
            .nav
            .press_scrollbar(sb.press_zone(y), y, sb.travel(), total, visible)
        {
            ScrollbarPress::DragStarted => {
                cx.capture_pointer();
                Handled::Consumed
            }
            ScrollbarPress::Paged => self.scroll_outcome(before, state),
            ScrollbarPress::Ignored => Handled::Consumed,
        }
    }

    /// Hand `event` to the wrapped table's own `Interactive` impl and
    /// re-address what it reports.
    fn forward(
        &self,
        state: &mut TreeTableState,
        event: &RoutedEvent,
        layout: &TreeTablePrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<TreeTableEvent> {
        let table_layout = layout.table_layout();
        let (handled, emitted) = {
            let mut host = TableHost {
                parent: cx,
                emitted: Vec::new(),
            };
            let handled = {
                let mut child = EventCx::new(self.table.id, &mut host);
                self.table
                    .handle(&mut state.table, event, &table_layout, &mut child)
            };
            (handled, host.emitted)
        };
        for emitted in emitted {
            match emitted {
                UiEvent::DataTable(_, ev) => {
                    if let Handled::Event(ev) = self.tree_event_for(state, ev) {
                        cx.emit(ev.into_ui_event(self.table.id));
                    }
                }
                other => cx.emit(other),
            }
        }
        match handled {
            Handled::Event(ev) => self.tree_event_for(state, ev),
            Handled::Consumed => Handled::Consumed,
            Handled::Ignored => Handled::Ignored,
        }
    }

    /// The tree-table event a wrapped-table event becomes. A header click
    /// becomes the sort it asks for; a row event gains its tree path; an
    /// event on a loading placeholder is swallowed.
    fn tree_event_for(
        &self,
        state: &mut TreeTableState,
        event: DataTableEvent,
    ) -> Handled<TreeTableEvent> {
        let row_event = |idx: usize, make: fn(usize, TreePath) -> TreeTableEvent| {
            if self.is_placeholder(idx) {
                Handled::Consumed
            } else {
                Handled::Event(make(idx, self.path_of(idx)))
            }
        };
        match event {
            DataTableEvent::HeaderClicked { col } => Handled::Event(TreeTableEvent::SortChanged {
                col,
                ascending: self.table.sort != Some((col, SortDirection::Ascending)),
            }),
            DataTableEvent::RowSelected { idx } => {
                if !self.is_placeholder(idx) {
                    state.nav.set_selected_path(Some(self.path_of(idx)));
                }
                row_event(idx, |idx, path| TreeTableEvent::RowSelected { idx, path })
            }
            DataTableEvent::RowActivated { idx } => {
                row_event(idx, |idx, path| TreeTableEvent::RowActivated { idx, path })
            }
            DataTableEvent::RowDoubleClicked { idx } => row_event(idx, |idx, path| {
                TreeTableEvent::RowDoubleClicked { idx, path }
            }),
            DataTableEvent::ContextMenuRequested { idx, position } => {
                if self.is_placeholder(idx) {
                    Handled::Consumed
                } else {
                    Handled::Event(TreeTableEvent::ContextMenuRequested {
                        idx,
                        path: self.path_of(idx),
                        position,
                    })
                }
            }
            DataTableEvent::SelectionSet { indices } => {
                let indices: Vec<usize> = indices
                    .into_iter()
                    .filter(|&i| !self.is_placeholder(i))
                    .collect();
                let paths = indices.iter().map(|&i| self.path_of(i)).collect();
                Handled::Event(TreeTableEvent::SelectionSet { indices, paths })
            }
            DataTableEvent::Scroll { delta, modifiers } => {
                Handled::Event(TreeTableEvent::Scroll { delta, modifiers })
            }
            DataTableEvent::ColumnResized { col, width } => {
                Handled::Event(TreeTableEvent::ColumnResized { col, width })
            }
            DataTableEvent::ScrollChanged { scroll_offset } => {
                state.nav.set_scroll_offset(scroll_offset);
                Handled::Event(TreeTableEvent::ScrollChanged { scroll_offset })
            }
            DataTableEvent::RowClicked {
                idx,
                button,
                modifiers,
            } if !self.is_placeholder(idx) => Handled::Event(TreeTableEvent::RowClicked {
                idx,
                path: self.path_of(idx),
                button,
                modifiers,
            }),
            DataTableEvent::RowClicked { .. } => Handled::Consumed,
            DataTableEvent::ColumnFocused { .. } => Handled::Consumed,
        }
    }
}

impl Interactive for TreeTable {
    type State = TreeTableState;
    type Action = TreeTableEvent;

    /// One focus stop as a whole — rows are navigated inside
    /// [`Self::handle`].
    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    /// Every visible chevron, then the wrapped table's own sub-regions
    /// (headers, dividers, rows, scrollbar), in widget-local
    /// coordinates — chevrons first, so a point on one resolves to it
    /// rather than to its row.
    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, SubTarget)> {
        let l = &layout.layout.table;
        let x0 = l.inset_x;
        let x1 = (l.viewport_width - l.inset_x - l.scrollbar_width).max(x0);
        let mut out: Vec<(Rect, SubTarget)> = layout
            .layout
            .chevrons
            .iter()
            .filter_map(|(idx, b)| {
                let x = (x0 + b.x - l.h_scroll).max(x0);
                let right = (x0 + b.x - l.h_scroll + b.width).min(x1);
                (right > x).then(|| {
                    (
                        Rect::new(x, b.y, right - x, b.height),
                        TreeTableSubPart::Chevron(*idx).sub_target(),
                    )
                })
            })
            .collect();
        out.extend(self.table.sub_regions(&layout.table_layout()));
        out
    }

    fn handle(
        &self,
        state: &mut TreeTableState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<TreeTableEvent> {
        use crate::a11y::A11yAction;
        use crate::event::MouseButton;

        state.sync(self);
        let hit = |local: &crate::event::Point| {
            layout
                .layout
                .hit_test(local.x, local.y, layout.scroll_offset, layout.total_rows)
        };
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => match hit(local) {
                TreeTableHit::Chevron(idx) => {
                    cx.request_focus(None);
                    let node = &self.tree[idx];
                    match node.is_expanded {
                        Some(expanded) => self.toggle(state, node.path.clone(), !expanded, cx),
                        None => Handled::Consumed,
                    }
                }
                TreeTableHit::Table(DataTableHit::Scrollbar) => {
                    cx.request_focus(None);
                    self.press_scrollbar(state, local.y, layout, cx)
                }
                TreeTableHit::Table(DataTableHit::Row { idx }) if self.is_placeholder(idx) => {
                    cx.request_focus(None);
                    Handled::Consumed
                }
                TreeTableHit::Table(_) => self.forward(state, event, layout, cx),
            },
            RoutedEvent::Pointer {
                kind: PointerKind::DoubleClick(MouseButton::Left),
                local,
                ..
            } => match hit(local) {
                TreeTableHit::Chevron(idx) | TreeTableHit::Table(DataTableHit::Row { idx }) => {
                    if self.is_placeholder(idx) {
                        Handled::Consumed
                    } else {
                        Handled::Event(TreeTableEvent::RowDoubleClicked {
                            idx,
                            path: self.path_of(idx),
                        })
                    }
                }
                _ => Handled::Ignored,
            },
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } if state.nav.is_dragging() => match state.nav.drag_to(local.y) {
                Some(scroll_offset) => {
                    Handled::Event(TreeTableEvent::ScrollChanged { scroll_offset })
                }
                None => Handled::Consumed,
            },
            RoutedEvent::Pointer {
                kind: PointerKind::Up(MouseButton::Left),
                ..
            } if state.nav.is_dragging() => {
                state.nav.end_scrollbar_drag();
                cx.release_pointer();
                Handled::Consumed
            }
            RoutedEvent::Pointer { .. } => self.forward(state, event, layout, cx),
            RoutedEvent::Wheel { dy, .. } => {
                let before = state.nav.scroll_offset();
                state.nav.scroll_by_wheel(
                    *dy,
                    layout.layout.table.row_height,
                    self.table.rows.len(),
                    layout.visible_rows(),
                );
                self.scroll_outcome(before, state)
            }
            RoutedEvent::Key(key_event) => self.handle_key(state, key_event, layout, cx),
            RoutedEvent::Action(A11yAction::Click) => match self.table.selected_idx {
                Some(idx) if idx < self.table.rows.len() && !self.is_placeholder(idx) => {
                    Handled::Event(TreeTableEvent::RowActivated {
                        idx,
                        path: self.path_of(idx),
                    })
                }
                _ => Handled::Ignored,
            },
            RoutedEvent::Action(action @ (A11yAction::Expand | A11yAction::Collapse)) => {
                let want_expanded = *action == A11yAction::Expand;
                let node = self.table.selected_idx.and_then(|i| self.tree.get(i));
                match node {
                    Some(node) if node.is_expanded == Some(!want_expanded) => {
                        self.toggle(state, node.path.clone(), want_expanded, cx)
                    }
                    Some(_) => Handled::Consumed,
                    None => Handled::Ignored,
                }
            }
            RoutedEvent::Action(A11yAction::Focus) => {
                cx.request_focus(None);
                Handled::Consumed
            }
            RoutedEvent::Action(A11yAction::ScrollIntoView) => {
                let Some(idx) = self.table.selected_idx else {
                    return Handled::Ignored;
                };
                let before = state.nav.scroll_offset();
                state.nav.scroll_to_visible(idx, layout.visible_rows());
                self.scroll_outcome(before, state)
            }
            _ => Handled::Ignored,
        }
    }

    fn a11y_actions(&self, _state: &TreeTableState) -> Vec<crate::a11y::A11yAction> {
        use crate::a11y::A11yAction;
        if self.table.rows.is_empty() {
            Vec::new()
        } else {
            vec![
                A11yAction::Click,
                A11yAction::Focus,
                A11yAction::Expand,
                A11yAction::Collapse,
                A11yAction::ScrollIntoView,
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, NamedKey, Point};
    use crate::primitives::data_table::{Column, ColumnAlign, ColumnWidth, DataRow, DataTable};
    use crate::primitives::tree_table::TreeTableNode;
    use crate::types::{Decoration, Modifiers, StyledText};

    #[derive(Default)]
    struct MockHost {
        emitted: Vec<UiEvent>,
        captured: bool,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            true
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}
        fn capture_pointer(&mut self, _id: &WidgetId) {
            self.captured = true;
        }
        fn release_pointer(&mut self) {
            self.captured = false;
        }
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, event: UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    /// `dir` (a pending branch, expanded, with its placeholder), then
    /// `n - 2` leaves.
    fn tree_table(n: usize) -> TreeTable {
        let mut rows = vec!["dir".to_string(), "…".to_string()];
        rows.extend((2..n).map(|i| format!("item{i}")));
        let mut tree = vec![
            TreeTableNode::new(vec![0], 0)
                .with_expanded(true)
                .with_children_pending(),
            TreeTableNode::new(vec![0, 0], 1).as_placeholder(),
        ];
        tree.extend((2..n).map(|i| TreeTableNode::new(vec![i as u16 - 1], 0)));
        TreeTable::new(
            DataTable {
                selection_mode: Default::default(),
                selected_indices: Vec::new(),
                id: WidgetId::new("tt:unit"),
                columns: vec![
                    Column {
                        title: "Name".into(),
                        width: ColumnWidth::Flex(1.0),
                        align: ColumnAlign::Left,
                    },
                    Column {
                        title: "Size".into(),
                        width: ColumnWidth::FixedCells(4.0),
                        align: ColumnAlign::Right,
                    },
                ],
                rows: rows
                    .into_iter()
                    .map(|label| DataRow {
                        cells: vec![StyledText::plain(label), StyledText::plain("1")],
                        decoration: Decoration::Normal,
                    })
                    .collect(),
                selected_idx: None,
                scroll_offset: 0,
                sort: None,
                has_focus: true,
                show_scrollbar: true,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: vec![],
                footer: None,
                bordered: false,
                striped: false,
                glyphs: None,
            },
            tree,
        )
    }

    fn layout_for(tt: &TreeTable, height: f32) -> TreeTablePrimitiveLayout {
        Primitive::layout(
            tt,
            &LayoutCtx::new(1.0, 1.0, false),
            Rect::new(0.0, 0.0, 30.0, height),
        )
    }

    fn run(
        tt: &TreeTable,
        state: &mut TreeTableState,
        event: RoutedEvent,
        height: f32,
    ) -> (Handled<TreeTableEvent>, MockHost) {
        let layout = layout_for(tt, height);
        let mut host = MockHost::default();
        let handled = {
            let mut cx = EventCx::new(tt.table.id, &mut host);
            Interactive::handle(tt, state, &event, &layout, &mut cx)
        };
        (handled, host)
    }

    fn key(key: Key) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn press(x: f32, y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind: PointerKind::Down(crate::event::MouseButton::Left),
            sub: None,
            local: Point::new(x, y),
            mods: Modifiers::default(),
        }
    }

    fn emitted_tree_events(host: &MockHost) -> Vec<TreeTableEvent> {
        host.emitted
            .iter()
            .filter_map(|e| match e {
                UiEvent::TreeTable(_, ev) => Some(ev.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sub_parts_round_trip_through_their_sub_targets() {
        for part in [
            TreeTableSubPart::Chevron(0),
            TreeTableSubPart::Chevron(41),
            TreeTableSubPart::Table(DataTableSubPart::Row(3)),
            TreeTableSubPart::Table(DataTableSubPart::Header(1)),
            TreeTableSubPart::Table(DataTableSubPart::Scrollbar),
        ] {
            assert_eq!(
                TreeTableSubPart::from_sub_target(part.sub_target()),
                Some(part)
            );
        }
    }

    #[test]
    fn sub_regions_put_each_chevron_ahead_of_its_row() {
        let tt = tree_table(4);
        let layout = layout_for(&tt, 10.0);
        let regions = tt.sub_regions(&layout);
        let chevron = regions
            .iter()
            .position(|(_, s)| *s == TreeTableSubPart::Chevron(0).sub_target())
            .expect("the branch's chevron is a sub-region");
        let row = regions
            .iter()
            .position(|(_, s)| *s == TreeTableSubPart::Table(DataTableSubPart::Row(0)).sub_target())
            .expect("the branch's row is a sub-region");
        assert!(chevron < row);
    }

    #[test]
    fn down_skips_the_loading_placeholder() {
        let mut tt = tree_table(5);
        tt.table.selected_idx = Some(0);
        let mut state = TreeTableState::default();
        let (handled, _) = run(&tt, &mut state, key(Key::Named(NamedKey::Down)), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::RowSelected {
                idx: 2,
                path: vec![1],
            })
        );
    }

    #[test]
    fn page_down_moves_by_one_viewport_less_one_row() {
        let mut tt = tree_table(40);
        tt.table.selected_idx = Some(2);
        let mut state = TreeTableState::default();
        // Height 10: one header row, nine body rows.
        let (handled, host) = run(&tt, &mut state, key(Key::Named(NamedKey::PageDown)), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::RowSelected {
                idx: 10,
                path: vec![9],
            })
        );
        assert_eq!(
            emitted_tree_events(&host),
            vec![TreeTableEvent::ScrollChanged { scroll_offset: 2 }]
        );
    }

    #[test]
    fn vim_keys_follow_the_descriptor_flag() {
        let mut tt = tree_table(5);
        tt.table.selected_idx = Some(2);
        let mut state = TreeTableState::default();
        let (handled, _) = run(&tt, &mut state, key(Key::Char('j')), 10.0);
        assert!(
            !matches!(
                handled,
                Handled::Event(TreeTableEvent::RowSelected { idx: 3, .. })
            ),
            "with vim keys off, j is a type-to-jump character"
        );
        tt.vim_keys = true;
        let mut state = TreeTableState::default();
        let (handled, _) = run(&tt, &mut state, key(Key::Char('j')), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::RowSelected {
                idx: 3,
                path: vec![2],
            })
        );
    }

    #[test]
    fn expanding_a_pending_branch_requests_its_children_once() {
        let mut tt = tree_table(4);
        tt.tree[0].is_expanded = Some(false);
        tt.table.selected_idx = Some(0);
        let mut state = TreeTableState::default();
        let (handled, host) = run(&tt, &mut state, key(Key::Named(NamedKey::Right)), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::ToggleExpanded {
                path: vec![0],
                expand: true,
            })
        );
        assert_eq!(
            emitted_tree_events(&host),
            vec![TreeTableEvent::ChildrenRequested {
                idx: 0,
                path: vec![0],
            }]
        );
        let (_, host) = run(&tt, &mut state, key(Key::Char(' ')), 10.0);
        assert!(
            emitted_tree_events(&host).is_empty(),
            "a second expand before the app answers asks nothing more"
        );
        // Once the app supplies the children the guard clears.
        tt.tree[0].children_pending = false;
        let (_, host) = run(&tt, &mut state, key(Key::Char(' ')), 10.0);
        assert!(emitted_tree_events(&host).is_empty());
        assert!(state.requested.is_empty());
    }

    #[test]
    fn a_chevron_press_toggles_without_selecting() {
        let tt = tree_table(4);
        let mut state = TreeTableState::default();
        let (x, y) = {
            let layout = layout_for(&tt, 10.0);
            let b = layout.layout.chevrons[0].1;
            (b.x, b.y)
        };
        let (handled, _) = run(&tt, &mut state, press(x, y), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::ToggleExpanded {
                path: vec![0],
                expand: false,
            })
        );
    }

    #[test]
    fn a_header_press_asks_for_the_next_sort() {
        let mut tt = tree_table(4);
        let mut state = TreeTableState::default();
        let (handled, _) = run(&tt, &mut state, press(1.0, 0.0), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::SortChanged {
                col: 0,
                ascending: true,
            })
        );
        tt.table.sort = Some((0, SortDirection::Ascending));
        let (handled, _) = run(&tt, &mut state, press(1.0, 0.0), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::SortChanged {
                col: 0,
                ascending: false,
            })
        );
    }

    #[test]
    fn a_row_press_selects_through_the_wrapped_table_with_its_path() {
        let tt = tree_table(4);
        let mut state = TreeTableState::default();
        let (handled, _) = run(&tt, &mut state, press(8.0, 3.0), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::RowSelected {
                idx: 2,
                path: vec![1],
            })
        );
        let (handled, _) = run(&tt, &mut state, press(8.0, 2.0), 10.0);
        assert_eq!(handled, Handled::Consumed, "the placeholder never selects");
    }

    #[test]
    fn the_scrollbar_track_pages_and_its_thumb_captures() {
        let tt = tree_table(40);
        let layout = layout_for(&tt, 10.0);
        let track = layout
            .layout
            .table
            .v_scrollbar_track
            .expect("rows overflow");
        let mut state = TreeTableState::default();
        let (handled, _) = run(
            &tt,
            &mut state,
            press(track.x, track.y + track.height - 1.0),
            10.0,
        );
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::ScrollChanged { scroll_offset: 9 })
        );
        let mut state = TreeTableState::default();
        let (handled, host) = run(&tt, &mut state, press(track.x, track.y), 10.0);
        assert_eq!(handled, Handled::Consumed);
        assert!(host.captured, "a thumb press captures the pointer");
        assert!(state.nav.is_dragging());
    }

    #[test]
    fn the_wheel_reports_a_scroll() {
        let tt = tree_table(40);
        let mut state = TreeTableState::default();
        let (handled, _) = run(
            &tt,
            &mut state,
            RoutedEvent::Wheel {
                dx: 0.0,
                dy: -3.0,
                local: Point::new(2.0, 3.0),
                mods: Modifiers::default(),
            },
            10.0,
        );
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::ScrollChanged { scroll_offset: 3 })
        );
    }

    #[test]
    fn enter_activates_the_selected_row() {
        let mut tt = tree_table(4);
        tt.table.selected_idx = Some(3);
        let mut state = TreeTableState::default();
        let (handled, _) = run(&tt, &mut state, key(Key::Named(NamedKey::Enter)), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeTableEvent::RowActivated {
                idx: 3,
                path: vec![2],
            })
        );
    }

    /// Event parity for the input routes: a `Collapse` on the selected
    /// expanded branch, routed as an assistive-technology action through
    /// the live widget (`a11y::route_widget_action`), straight into
    /// `Interactive::handle`, and as the Left key, resolves to the same
    /// `ToggleExpanded`; an `Expand` on that already-expanded branch
    /// resolves to no event on the action routes.
    #[test]
    fn expand_and_collapse_agree_across_the_a11y_route_direct_handle_and_keys() {
        let mut tt = tree_table(4);
        tt.table.selected_idx = Some(0);
        let id = tt.table.id;
        let area = Rect::new(0.0, 0.0, 30.0, 10.0);
        let collapse = vec![UiEvent::TreeTable(
            id,
            TreeTableEvent::ToggleExpanded {
                path: vec![0],
                expand: false,
            },
        )];

        let into_events = |handled: Handled<TreeTableEvent>, host: MockHost| {
            let mut events = host.emitted;
            if let Handled::Event(ev) = handled {
                events.push(crate::interactive::WidgetAction::into_ui_event(ev, id));
            }
            events
        };
        let direct = |event: RoutedEvent| {
            let mut state = TreeTableState::default();
            let (handled, host) = run(&tt, &mut state, event, area.height);
            into_events(handled, host)
        };
        let live = |action| {
            let mut frame = crate::frame_tree::FrameTree::new();
            frame.push(id, area, Box::new(tt.clone()));
            let mut states = crate::widget_state::WidgetStateStore::new();
            crate::a11y::route_widget_action(
                &frame,
                &mut states,
                &LayoutCtx::new(1.0, 1.0, false),
                id,
                action,
            )
        };

        assert_eq!(
            direct(RoutedEvent::Action(crate::a11y::A11yAction::Collapse)),
            collapse
        );
        assert_eq!(live(crate::a11y::A11yAction::Collapse), collapse);
        assert_eq!(direct(key(Key::Named(NamedKey::Left))), collapse);
        assert_eq!(
            direct(RoutedEvent::Action(crate::a11y::A11yAction::Expand)),
            Vec::new()
        );
        assert_eq!(live(crate::a11y::A11yAction::Expand), Vec::new());
    }
}
