//! Render-to-frame assembly: paints either the six-fixed-slot
//! [`RenderFrame`]/[`RenderRefs`] shape ([`draw_bands`]) or a decoded
//! [`crate::layout::LayoutNode`] tree ([`paint_layout`]) into a
//! [`quadraui::Backend`], recording a [`PaintedHit`] per primitive painted
//! so a later pointer event can be hit-tested against this frame.

use std::collections::HashMap;
use std::rc::Rc;

use quadraui::{
    Backend, Button, DataTable, InteractionState, ListView, Rect, Scrollbar, Split, TabBar,
    TextDisplay, TreeView, WidgetId,
};

use crate::extra_paint::ExtraPaint;
use crate::hit::PaintedHit;
use crate::layout::{LayoutLeaf, LayoutNode, LeafDescriptor};

/// Owned decode of a render callback's returned descriptor set — one
/// optional descriptor per slot, keyed by `"button"`, `"text_display"`,
/// `"list"`, `"tree"`, `"data_table"`, and `"tab_bar"`. A binding decodes
/// its own wire format (a Python `dict`, a JS object, ...) into this type;
/// [`RenderFrame::as_refs`] is the one shape [`draw_bands`] paints from,
/// regardless of which path produced it.
#[derive(Default)]
pub struct RenderFrame {
    pub button: Option<Button>,
    pub text_display: Option<TextDisplay>,
    pub list: Option<ListView>,
    pub tree: Option<TreeView>,
    pub data_table: Option<DataTable>,
    pub tab_bar: Option<TabBar>,
    /// Additive primitive slots present in this frame, keyed by kind name.
    pub extra: HashMap<String, Rc<dyn ExtraPaint>>,
}

impl RenderFrame {
    pub fn as_refs(&self) -> RenderRefs<'_> {
        RenderRefs {
            button: self.button.as_ref(),
            text_display: self.text_display.as_ref(),
            list: self.list.as_ref(),
            tree: self.tree.as_ref(),
            data_table: self.data_table.as_ref(),
            tab_bar: self.tab_bar.as_ref(),
            extra: &self.extra,
        }
    }
}

/// One descriptor reference per slot, borrowed either from a binding's own
/// static fields or from a [`RenderFrame`] just decoded from a render
/// callback's return value — the one shape [`draw_bands`] paints from,
/// regardless of which path produced it.
pub struct RenderRefs<'a> {
    pub button: Option<&'a Button>,
    pub text_display: Option<&'a TextDisplay>,
    pub list: Option<&'a ListView>,
    pub tree: Option<&'a TreeView>,
    pub data_table: Option<&'a DataTable>,
    pub tab_bar: Option<&'a TabBar>,
    /// Additive primitive slots, keyed by kind name. Painted after the six
    /// fixed bands above, in a binding's own `EXTRA_PRIMITIVES` order.
    pub extra: &'a HashMap<String, Rc<dyn ExtraPaint>>,
}

/// Paints each present slot top-to-bottom in a fixed-height band, skipping
/// any slot once the viewport runs out of room. Appends a [`PaintedHit`] to
/// `hits` for every slot painted, so a later pointer event can be routed to
/// it.
pub fn draw_bands(backend: &mut dyn Backend, refs: RenderRefs<'_>, hits: &mut Vec<PaintedHit>) {
    let viewport = backend.viewport();
    // Band heights below are given in text LINES, not raw viewport units: a
    // button is 3 lines tall, a tree 4. `viewport` and `line_height` share
    // the backend's own unit (cells on TUI, where `line_height` is 1.0;
    // device pixels on the native backends, where it is the font's pixel line
    // height), so scaling the line count by `line_height` lays the bands out
    // correctly on every backend instead of collapsing them to a few pixels
    // tall on native.
    let line_height = backend.line_height();
    let mut y = 0.0_f32;
    let mut next_band = |lines: f32| -> Option<Rect> {
        let remaining = viewport.height - y;
        if remaining <= 0.0 {
            return None;
        }
        let band_height = (lines * line_height).min(remaining);
        let rect = Rect::new(0.0, y, viewport.width, band_height);
        y += band_height;
        Some(rect)
    };

    if let Some(button) = refs.button {
        if let Some(rect) = next_band(3.0) {
            let layout = backend.draw_button(rect, button);
            hits.push(PaintedHit::Button {
                id: button.id,
                layout,
            });
        }
    }
    if let Some(text_display) = refs.text_display {
        if let Some(rect) = next_band(3.0) {
            backend.draw_text_display(rect, text_display);
        }
    }
    if let Some(list) = refs.list {
        if let Some(rect) = next_band(4.0) {
            backend.draw_list(rect, list);
            let layout = backend.list_layout(rect, list);
            hits.push(PaintedHit::List {
                id: list.id,
                area: rect,
                layout,
            });
        }
    }
    if let Some(tree) = refs.tree {
        if let Some(rect) = next_band(4.0) {
            let (content_rect, scrollbar, viewport_rows) =
                draw_tree_with_scrollbar(backend, rect, tree);
            let layout = backend.tree_layout(content_rect, tree);
            hits.push(PaintedHit::Tree {
                id: tree.id,
                area: content_rect,
                layout,
                row_paths: tree.rows.iter().map(|row| row.path.clone()).collect(),
                scrollbar,
                scroll_offset: tree.scroll_offset,
                viewport_rows,
            });
        }
    }
    if let Some(data_table) = refs.data_table {
        if let Some(rect) = next_band(4.0) {
            let layout = backend.draw_data_table(rect, data_table, None);
            hits.push(PaintedHit::DataTable {
                id: data_table.id,
                area: rect,
                layout,
                scroll_offset: data_table.scroll_offset,
                total_rows: data_table.rows.len(),
            });
        }
    }
    if let Some(tab_bar) = refs.tab_bar {
        if let Some(rect) = next_band(3.0) {
            let layout = backend.draw_tab_bar_layout(rect, tab_bar, None);
            hits.push(PaintedHit::TabBar {
                id: tab_bar.id,
                area: rect,
                layout,
            });
        }
    }
    // Additive primitives, painted in canonical `EXTRA_PRIMITIVES` order so
    // the output is deterministic regardless of `HashMap` iteration order.
    for kind in crate::extra_paint::EXTRA_PRIMITIVE_KINDS {
        if let Some(extra) = refs.extra.get(kind) {
            if let Some(rect) = next_band(3.0) {
                extra.paint(backend, rect);
                hits.push(PaintedHit::Extra {
                    id: extra.id(),
                    area: rect,
                    extra: Rc::clone(extra),
                });
            }
        }
    }
}

/// Paints a layout tree into `area`. A `Row`/`Column` divides its area into
/// equal shares — one per child, side by side or stacked — and recurses;
/// equal shares (rather than `draw_bands`'s fixed line-count bands) fill
/// the whole area regardless of viewport size. A `Leaf` paints its one
/// descriptor into the whole area its container gave it. A `Split` divides
/// its area into two unequal panes at `ratio` via `Backend::draw_split`
/// (the same divider paint/hit-test every backend already shares for the
/// Rust-side `Split` primitive) and recurses into each with its resolved
/// bounds. A `Tabs` reserves a top band for its tab strip (painted via
/// `Backend::draw_tab_bar_layout`, the same strip every backend already
/// shares for the Rust-side `TabBar` primitive) and recurses into the
/// remaining area with the selected tab's content, so a tab group is just a
/// `Split` whose panes are each a `Tabs`. Per-child flex weights on
/// `Row`/`Column` are a follow-up; every `Row`/`Column` child gets the same
/// share for now.
pub fn paint_layout(
    backend: &mut dyn Backend,
    area: Rect,
    node: &LayoutNode,
    hits: &mut Vec<PaintedHit>,
) {
    match node {
        LayoutNode::Row(children) => {
            for (child, rect) in children.iter().zip(split_rects(area, children.len(), true)) {
                paint_layout(backend, rect, child, hits);
            }
        }
        LayoutNode::Column(children) => {
            for (child, rect) in children
                .iter()
                .zip(split_rects(area, children.len(), false))
            {
                paint_layout(backend, rect, child, hits);
            }
        }
        LayoutNode::Leaf(leaf) => paint_leaf(backend, area, leaf, hits),
        LayoutNode::Split(split) => {
            // `split.split.first_min`/`second_min` are decoded as portable
            // line counts (see a binding's own split decode, e.g.
            // `layout_split_from_dict` in `quadraui-py`); scale them into
            // the backend's native unit here, right before painting, so
            // every backend gets a minimum pane size of the same visual
            // weight from the same binding-supplied number.
            let line_height = backend.line_height();
            let native_split = Split {
                first_min: split.split.first_min * line_height,
                second_min: split.split.second_min * line_height,
                ..split.split.clone()
            };
            let layout = backend.draw_split(area, &native_split);
            // Pushed before recursing so the divider/pane hit only wins
            // over a descendant primitive's own, more specific hit when
            // nothing painted inside either pane claims the point first —
            // `hit_test_painted` checks in reverse paint order.
            hits.push(PaintedHit::Split {
                layout: layout.clone(),
            });
            paint_layout(backend, layout.first_bounds, &split.first, hits);
            paint_layout(backend, layout.second_bounds, &split.second, hits);
        }
        LayoutNode::Tabs(tabs) => {
            let strip_height = (3.0 * backend.line_height()).min(area.height);
            let strip_rect = Rect::new(area.x, area.y, area.width, strip_height);
            let layout = backend.draw_tab_bar_layout(strip_rect, &tabs.tab_bar, None);
            hits.push(PaintedHit::TabBar {
                id: tabs.tab_bar.id,
                area: strip_rect,
                layout,
            });
            let content_rect = Rect::new(
                area.x,
                area.y + strip_height,
                area.width,
                (area.height - strip_height).max(0.0),
            );
            if let Some(selected) = tabs.entries.get(tabs.selected) {
                paint_layout(backend, content_rect, &selected.content, hits);
            }
        }
        LayoutNode::Panel(panel_node) => {
            let layout = backend.draw_panel(area, &panel_node.panel);
            paint_layout(backend, layout.content_bounds, &panel_node.content, hits);
        }
        LayoutNode::SidebarPanel(sidebar_node) => {
            let layout = backend.draw_sidebar_panel_interactive(
                area,
                &sidebar_node.sidebar_panel,
                &InteractionState::default(),
            );
            paint_layout(backend, layout.content_bounds, &sidebar_node.content, hits);
        }
        LayoutNode::SplitTree(split_tree) => {
            let layout = backend.draw_split_tree(area, &split_tree.tree);
            for ((_, rect), content) in layout.leaves.iter().zip(split_tree.leaves.iter()) {
                paint_layout(backend, *rect, content, hits);
            }
        }
    }
}

/// Divides `area` into `n` equal shares, side by side when `horizontal` or
/// stacked top-to-bottom otherwise. Returns an empty `Vec` for `n == 0`.
pub fn split_rects(area: Rect, n: usize, horizontal: bool) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let n_f = n as f32;
    (0..n)
        .map(|i| {
            let i_f = i as f32;
            if horizontal {
                let share = area.width / n_f;
                Rect::new(area.x + share * i_f, area.y, share, area.height)
            } else {
                let share = area.height / n_f;
                Rect::new(area.x, area.y + share * i_f, area.width, share)
            }
        })
        .collect()
}

/// How many rows of `tree` fit in `rect` at a zero scroll offset — the
/// viewport's true row capacity, as distinct from `TreeViewLayout`'s own
/// `visible_rows.len()` at the tree's ACTUAL offset, which can be shorter
/// near the end of a long list. Paints into a scratch clone rather than
/// `tree` itself, so computing this never disturbs the real paint below.
pub fn tree_viewport_rows(backend: &mut dyn Backend, rect: Rect, tree: &TreeView) -> usize {
    let mut shadow = tree.clone();
    shadow.scroll_offset = 0;
    backend.tree_layout(rect, &shadow).visible_rows.len()
}

/// Paints `tree` into `area`, reserving a right-hand gutter for its
/// vertical scrollbar when one is needed, and returns `(content_rect,
/// scrollbar, viewport_rows)` so the caller can hit-test against the SAME
/// geometry that was actually painted (mirrors
/// `quadraui::compose::TreeController::split_rect`, minus its drag/selection
/// state) and drive drag/page/wheel scrolling without re-touching the
/// backend.
///
/// On a backend that paints its own scrollbar overlay inside `draw_tree`
/// ([`Backend::tree_paints_own_scrollbar`] — GTK, macOS, Win), `tree` is
/// painted into the full `area` and the returned `Scrollbar` exists purely
/// for hit-testing; painting it again here would double it. On a backend
/// with no overlay (TUI), a `line_height`-wide column is reserved on the
/// right, `tree` is painted into the narrowed rect, and the scrollbar is
/// built and painted explicitly into the reserved column. Either way,
/// `None` is returned (and the full `area` painted) when the rows fit
/// without scrolling.
pub fn draw_tree_with_scrollbar(
    backend: &mut dyn Backend,
    area: Rect,
    tree: &TreeView,
) -> (Rect, Option<Scrollbar>, usize) {
    if backend.tree_paints_own_scrollbar() {
        backend.draw_tree(area, tree);
        let scrollbar = backend.tree_vscrollbar(area, tree);
        let viewport_rows = tree_viewport_rows(backend, area, tree);
        return (area, scrollbar, viewport_rows);
    }
    let track_width = backend.line_height();
    if area.width <= track_width {
        backend.draw_tree(area, tree);
        let viewport_rows = tree_viewport_rows(backend, area, tree);
        return (area, None, viewport_rows);
    }
    let content_rect = Rect::new(area.x, area.y, area.width - track_width, area.height);
    let viewport_rows = tree_viewport_rows(backend, content_rect, tree);
    if tree.rows.len() <= viewport_rows {
        backend.draw_tree(area, tree);
        return (area, None, viewport_rows);
    }
    backend.draw_tree(content_rect, tree);
    let track_rect = Rect::new(
        area.x + content_rect.width,
        area.y,
        track_width,
        area.height,
    );
    let scrollbar = Scrollbar::vertical(
        format!("{}-scrollbar", tree.id),
        track_rect,
        tree.scroll_offset as f32,
        tree.rows.len() as f32,
        viewport_rows as f32,
        track_width.max(1.0),
    );
    backend.draw_scrollbar(track_rect, &scrollbar);
    (content_rect, Some(scrollbar), viewport_rows)
}

/// Paints one leaf's descriptor into the whole `area` its container gave
/// it. Appends a [`PaintedHit`] for it, keyed by the leaf's own tree-address
/// `id` (not the descriptor's embedded `id`, which a render callback may
/// leave at its default and reuse across leaves) so a later pointer event
/// can be routed back to this exact slot in the layout tree.
pub fn paint_leaf(
    backend: &mut dyn Backend,
    area: Rect,
    leaf: &LayoutLeaf,
    hits: &mut Vec<PaintedHit>,
) {
    let id = WidgetId::new(leaf.id.clone());
    match &leaf.descriptor {
        LeafDescriptor::Button(button) => {
            let layout = backend.draw_button(area, button);
            hits.push(PaintedHit::Button { id, layout });
        }
        LeafDescriptor::TextDisplay(text_display) => {
            backend.draw_text_display(area, text_display);
        }
        LeafDescriptor::List(list) => {
            backend.draw_list(area, list);
            let layout = backend.list_layout(area, list);
            hits.push(PaintedHit::List { id, area, layout });
        }
        LeafDescriptor::Tree(tree) => {
            let (content_rect, scrollbar, viewport_rows) =
                draw_tree_with_scrollbar(backend, area, tree);
            let layout = backend.tree_layout(content_rect, tree);
            hits.push(PaintedHit::Tree {
                id,
                area: content_rect,
                layout,
                row_paths: tree.rows.iter().map(|row| row.path.clone()).collect(),
                scrollbar,
                scroll_offset: tree.scroll_offset,
                viewport_rows,
            });
        }
        LeafDescriptor::DataTable(data_table) => {
            let layout = backend.draw_data_table(area, data_table, None);
            hits.push(PaintedHit::DataTable {
                id,
                area,
                layout,
                scroll_offset: data_table.scroll_offset,
                total_rows: data_table.rows.len(),
            });
        }
        LeafDescriptor::TabBar(tab_bar) => {
            let layout = backend.draw_tab_bar_layout(area, tab_bar, None);
            hits.push(PaintedHit::TabBar { id, area, layout });
        }
        LeafDescriptor::Extra(extra) => {
            extra.paint(backend, area);
            hits.push(PaintedHit::Extra {
                id,
                area,
                extra: Rc::clone(extra),
            });
        }
    }
}
