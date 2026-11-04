//! Render-to-frame assembly: paints either the six-fixed-slot
//! [`RenderFrame`]/[`RenderRefs`] shape ([`draw_bands`]) or a decoded
//! [`crate::layout::LayoutNode`] tree ([`paint_layout`]) into a
//! [`quadraui::Backend`], recording a [`PaintedHit`] per primitive painted
//! so a later pointer event can be hit-tested against this frame.

use std::collections::HashMap;
use std::rc::Rc;

use serde::{Deserialize, Deserializer};

use quadraui::{
    Backend, Button, DataTable, InteractionState, ListView, Rect, Scrollbar, Split, TabBar,
    TextDisplay, TextDisplayLayout, TextRegion, TreeView, WidgetId,
};

use crate::extra_paint::ExtraPaint;
use crate::hit::PaintedHit;
use crate::layout::{quantise_margin, LayoutContainer, LayoutLeaf, LayoutNode, LeafDescriptor};

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

/// Decodes a render callback's returned `dict` into a [`RenderFrame`] —
/// the flat six-slot wire shape (`"button"`, `"text_display"`, `"list"`,
/// `"tree"`, `"data_table"`, `"tab_bar"`, plus any additive primitive
/// kind) is a contract of this type, not of any one binding's own decode.
/// A missing key or a `null` value both mean "draw nothing in this slot
/// this frame"; every error is labeled `"render"` — the only context a
/// `RenderFrame` is ever decoded in.
impl<'de> Deserialize<'de> for RenderFrame {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        crate::wire::render_frame_from_value("render", &value).map_err(serde::de::Error::custom)
    }
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
/// it. `chrome_interaction` supplies the hover/pressed [`InteractionState`]
/// a chrome-bar extra (`Toolbar`'s buttons, `StatusBar`'s segments) paints
/// with, keyed by that extra's own id — a binding with no such bookkeeping
/// (or a test with no extras at all) passes `&|_| InteractionState::default()`.
pub fn draw_bands(
    backend: &mut dyn Backend,
    refs: RenderRefs<'_>,
    hits: &mut Vec<PaintedHit>,
    chrome_interaction: &dyn Fn(WidgetId) -> InteractionState,
) {
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
            hits.push(draw_text_display_hit(
                backend,
                text_display.id,
                rect,
                text_display,
            ));
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
                editing: tree
                    .rows
                    .iter()
                    .find_map(|row| row.edit.clone().map(|edit| (row.path.clone(), edit))),
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
                selected_idx: data_table.selected_idx,
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
                let interaction = chrome_interaction(extra.id());
                let chrome_hits = extra.paint_chrome(backend, rect, &interaction);
                hits.push(PaintedHit::Extra {
                    id: extra.id(),
                    area: rect,
                    extra: Rc::clone(extra),
                    chrome_hits,
                });
            }
        }
    }
}

/// How many lines of `td` fit in `rect` at a zero scroll offset — mirrors
/// [`tree_viewport_rows`] for the same reason: [`quadraui::TextDisplayLayout::visible_lines`]
/// at `td`'s ACTUAL offset can be shorter near the end of a long buffer, so
/// the wheel-scroll `max_offset` clamp needs the viewport's true line
/// capacity instead. Only queries `Backend::text_display_layout` against a
/// scratch clone — paints nothing — so `draw_text_display_hit` calling it
/// after its own real `text_display_layout` call means every frame lays
/// out `td` twice.
fn text_display_viewport_lines(backend: &mut dyn Backend, rect: Rect, td: &TextDisplay) -> usize {
    let mut shadow = td.clone();
    shadow.scroll_offset = 0;
    shadow.auto_scroll = false;
    backend
        .text_display_layout(rect, &shadow)
        .visible_lines
        .len()
}

/// Paints `td` into `area` under hit-map id `id` and records its
/// [`PaintedHit::TextDisplay`] — shared by [`draw_bands`] (which passes
/// `td.id`) and `paint_leaf`'s `LeafDescriptor::TextDisplay` arm (which,
/// like every other leaf, passes the leaf's own tree-address id instead),
/// so the two paint paths can't drift apart on what gets stored.
fn draw_text_display_hit(
    backend: &mut dyn Backend,
    id: WidgetId,
    area: Rect,
    td: &TextDisplay,
) -> PaintedHit {
    backend.draw_text_display(area, td);
    let layout = backend.text_display_layout(area, td);
    let viewport_lines = text_display_viewport_lines(backend, area, td);
    let body_offset = if td.title.is_some() {
        backend.line_height()
    } else {
        0.0
    };
    // Register a selectable text region every frame so drag-select
    // (`TextSelectionChanged`) and Ctrl-C copy (`TextCopied`) work against
    // this `TextDisplay`. `lines` is left empty — see `text_display_region`'s
    // doc for why a flattened `td.lines` can't stand in for it.
    backend.register_text_region(text_display_region(id, area, body_offset, &layout));
    PaintedHit::TextDisplay {
        id,
        area,
        layout,
        body_offset,
        total_lines: td.lines.len(),
        viewport_lines,
    }
}

/// Builds the [`TextRegion`] registered for a painted `TextDisplay`'s body.
///
/// `bounds` is `area` shifted down by `body_offset` (the title row, when
/// present) and narrowed to exclude the scrollbar gutter `layout`
/// reserves inside `area` (when `show_scrollbar` is set) — the rect
/// actually holding painted text, matching how every other caller
/// registers a region (`examples/common/panel_app.rs`,
/// `examples/common/selection_app.rs`, `quadraui-gallery`'s own
/// `text_selection` demo all register the rendered content rect, not the
/// whole slot). Without this, a drag that reaches the right edge of a
/// `show_scrollbar` display inverts the gutter's track/thumb glyphs too.
///
/// `lines` is always empty: TUI reads its live cell buffer on Ctrl-C and
/// never touches it, and there is no backend-portable flattening of
/// `td.lines` to hand pixel backends (GTK/macOS/win) instead. A wrapped
/// logical line occupies a different number of display rows under each
/// backend's own wrap width, and `layout.resolved_scroll_offset` shifts
/// which logical line is on top, so a flattening built against one
/// backend's wrap/scroll state would misalign the rows another backend
/// actually painted. Leaving `lines` empty (the documented
/// [`TextRegion::lines`] escape hatch) makes a pixel backend's Ctrl-C copy
/// nothing for this region instead of copying the wrong text.
fn text_display_region(
    id: WidgetId,
    area: Rect,
    body_offset: f32,
    layout: &TextDisplayLayout,
) -> TextRegion {
    let gutter_width = layout.scrollbar_bounds.map(|g| g.width).unwrap_or(0.0);
    TextRegion {
        id,
        bounds: Rect::new(
            area.x,
            area.y + body_offset,
            (area.width - gutter_width).max(0.0),
            (area.height - body_offset).max(0.0),
        ),
        lines: Vec::new(),
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
/// `Split` whose panes are each a `Tabs`. A `Panel`/`SidebarPanel` paints its
/// chrome (title bar + actions, or toolbar) via `Backend::draw_panel`/
/// `Backend::draw_sidebar_panel_interactive` and recurses into
/// `content_bounds` with its one content subtree; a `SplitTree` paints its
/// dividers via `Backend::draw_split_tree` and recurses into each resolved
/// leaf rect. All four record a [`PaintedHit`] for their own chrome
/// (divider / title bar / action button / toolbar button), same as `Split`,
/// so a later pointer event can resolve against it. Per-child flex weights
/// on `Row`/`Column` are a follow-up; every `Row`/`Column` child gets the
/// same share for now.
pub fn paint_layout(
    backend: &mut dyn Backend,
    area: Rect,
    node: &LayoutNode,
    hits: &mut Vec<PaintedHit>,
    chrome_interaction: &dyn Fn(WidgetId) -> InteractionState,
) {
    match node {
        LayoutNode::Row(container) => {
            let area = inset_for_spacing(area, container);
            for (child, rect) in container.children.iter().zip(split_rects_with_gap(
                area,
                container.children.len(),
                true,
                container.spacing.gap,
            )) {
                paint_layout(backend, rect, child, hits, chrome_interaction);
            }
        }
        LayoutNode::Column(container) => {
            let area = inset_for_spacing(area, container);
            for (child, rect) in container.children.iter().zip(split_rects_with_gap(
                area,
                container.children.len(),
                false,
                container.spacing.gap,
            )) {
                paint_layout(backend, rect, child, hits, chrome_interaction);
            }
        }
        LayoutNode::Leaf(leaf) => paint_leaf(backend, area, leaf, hits, chrome_interaction),
        LayoutNode::Split(split) => {
            // `split.split.first_min`/`second_min` are decoded as portable
            // line counts (see `crate::wire::decode_layout_node`'s split
            // decode); scale them into the backend's native unit here,
            // right before painting, so every backend gets a minimum pane
            // size of the same visual weight from the same decoded number.
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
            paint_layout(
                backend,
                layout.first_bounds,
                &split.first,
                hits,
                chrome_interaction,
            );
            paint_layout(
                backend,
                layout.second_bounds,
                &split.second,
                hits,
                chrome_interaction,
            );
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
                paint_layout(
                    backend,
                    content_rect,
                    &selected.content,
                    hits,
                    chrome_interaction,
                );
            }
        }
        LayoutNode::Panel(panel_node) => {
            let layout = backend.draw_panel(area, &panel_node.panel);
            // Pushed before recursing into content, same reasoning as
            // `LayoutNode::Split`'s own push above: a descendant primitive's
            // more specific hit wins over this chrome hit (title bar /
            // action button) only when nothing inside `content_bounds`
            // claims the point first.
            hits.push(PaintedHit::Panel {
                id: panel_node.panel.id,
                layout: layout.clone(),
            });
            paint_layout(
                backend,
                layout.content_bounds,
                &panel_node.content,
                hits,
                chrome_interaction,
            );
        }
        LayoutNode::SidebarPanel(sidebar_node) => {
            let layout = backend.draw_sidebar_panel_interactive(
                area,
                &sidebar_node.sidebar_panel,
                &InteractionState::default(),
            );
            hits.push(PaintedHit::SidebarPanel {
                id: sidebar_node.sidebar_panel.id,
                layout: layout.clone(),
            });
            paint_layout(
                backend,
                layout.content_bounds,
                &sidebar_node.content,
                hits,
                chrome_interaction,
            );
        }
        LayoutNode::SplitTree(split_tree) => {
            let layout = backend.draw_split_tree(area, &split_tree.tree);
            hits.push(PaintedHit::SplitTree {
                id: WidgetId::new(split_tree.id.as_str()),
                layout: layout.clone(),
            });
            for ((_, rect), content) in layout.leaves.iter().zip(split_tree.leaves.iter()) {
                paint_layout(backend, *rect, content, hits, chrome_interaction);
            }
        }
    }
}

/// Shrinks `area` by `container.spacing`'s `margin` (the space the parent
/// leaves around the whole container, inset uniformly on every side) then
/// its `padding` (the space inset from the container's own edges before
/// laying out children) — `margin` quantised via [`quantise_margin`],
/// `padding` via [`crate::layout::EdgeInsets::inset`]. The order matters:
/// margin is outside the container's own box, padding is inside it, same
/// as CSS's box model.
fn inset_for_spacing(area: Rect, container: &LayoutContainer) -> Rect {
    let margin = quantise_margin(container.spacing.margin);
    let after_margin = Rect::new(
        area.x + margin,
        area.y + margin,
        (area.width - margin * 2.0).max(0.0),
        (area.height - margin * 2.0).max(0.0),
    );
    container.spacing.padding.inset(after_margin)
}

/// Divides `area` into `n` equal shares, side by side when `horizontal` or
/// stacked top-to-bottom otherwise, with `gap` native units of space
/// between adjacent shares (quantised via
/// [`crate::layout::quantise_padding`] so a TUI `gap` always lands on a
/// whole cell boundary). Returns an empty `Vec` for `n == 0`. `gap: 0.0`
/// behaves exactly like the equal-share split this replaced.
pub fn split_rects_with_gap(area: Rect, n: usize, horizontal: bool, gap: f32) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let gap = crate::layout::quantise_padding(gap);
    let n_f = n as f32;
    let total_gap = gap * (n.saturating_sub(1)) as f32;
    (0..n)
        .map(|i| {
            let i_f = i as f32;
            if horizontal {
                let share = ((area.width - total_gap) / n_f).max(0.0);
                Rect::new(area.x + (share + gap) * i_f, area.y, share, area.height)
            } else {
                let share = ((area.height - total_gap) / n_f).max(0.0);
                Rect::new(area.x, area.y + (share + gap) * i_f, area.width, share)
            }
        })
        .collect()
}

/// Divides `area` into `n` equal shares with no gap between them —
/// [`split_rects_with_gap`] with `gap: 0.0`, kept as its own name for
/// every caller that has no [`LayoutContainer`] spacing to thread through.
pub fn split_rects(area: Rect, n: usize, horizontal: bool) -> Vec<Rect> {
    split_rects_with_gap(area, n, horizontal, 0.0)
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
    chrome_interaction: &dyn Fn(WidgetId) -> InteractionState,
) {
    let id = WidgetId::new(leaf.id.clone());
    match &leaf.descriptor {
        LeafDescriptor::Button(button) => {
            let layout = backend.draw_button(area, button);
            hits.push(PaintedHit::Button { id, layout });
        }
        LeafDescriptor::TextDisplay(text_display) => {
            hits.push(draw_text_display_hit(backend, id, area, text_display));
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
                editing: tree
                    .rows
                    .iter()
                    .find_map(|row| row.edit.clone().map(|edit| (row.path.clone(), edit))),
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
                selected_idx: data_table.selected_idx,
            });
        }
        LeafDescriptor::TabBar(tab_bar) => {
            let layout = backend.draw_tab_bar_layout(area, tab_bar, None);
            hits.push(PaintedHit::TabBar { id, area, layout });
        }
        LeafDescriptor::Extra(extra) => {
            let interaction = chrome_interaction(id);
            let chrome_hits = extra.paint_chrome(backend, area, &interaction);
            hits.push(PaintedHit::Extra {
                id,
                area,
                extra: Rc::clone(extra),
                chrome_hits,
            });
        }
    }
}

#[cfg(test)]
mod spacing_tests {
    use super::*;
    use crate::layout::{EdgeInsets, Spacing};

    fn container(children: usize, spacing: Spacing) -> LayoutContainer {
        LayoutContainer {
            children: (0..children)
                .map(|_| {
                    LayoutNode::Row(LayoutContainer {
                        children: Vec::new(),
                        spacing: Spacing::default(),
                    })
                })
                .collect(),
            spacing,
        }
    }

    #[test]
    fn inset_for_spacing_applies_margin_then_padding() {
        let area = Rect::new(0.0, 0.0, 100.0, 50.0);
        let c = container(
            0,
            Spacing {
                padding: EdgeInsets::uniform(5.0),
                margin: 3.0,
                gap: 0.0,
            },
        );
        let inset = inset_for_spacing(area, &c);
        // margin (3) then padding (5) on every side: 8 total inset.
        assert_eq!(inset.x, 8.0);
        assert_eq!(inset.y, 8.0);
        assert_eq!(inset.width, 100.0 - 16.0);
        assert_eq!(inset.height, 50.0 - 16.0);
    }

    #[test]
    fn inset_for_spacing_drops_a_sub_cell_margin() {
        let area = Rect::new(0.0, 0.0, 100.0, 50.0);
        let c = container(
            0,
            Spacing {
                padding: EdgeInsets::default(),
                margin: 0.5,
                gap: 0.0,
            },
        );
        let inset = inset_for_spacing(area, &c);
        assert_eq!(inset, area, "a margin under one native unit drops to zero");
    }

    #[test]
    fn split_rects_with_gap_leaves_whole_unit_gaps_between_shares() {
        let area = Rect::new(0.0, 0.0, 33.0, 10.0);
        let rects = split_rects_with_gap(area, 3, true, 3.0);
        assert_eq!(rects.len(), 3);
        // 33 width, 2 gaps of 3 => 27 left for 3 equal shares of 9 each.
        assert_eq!(rects[0], Rect::new(0.0, 0.0, 9.0, 10.0));
        assert_eq!(rects[1], Rect::new(12.0, 0.0, 9.0, 10.0));
        assert_eq!(rects[2], Rect::new(24.0, 0.0, 9.0, 10.0));
    }

    #[test]
    fn split_rects_with_gap_zero_matches_the_plain_equal_split() {
        let area = Rect::new(0.0, 0.0, 30.0, 10.0);
        assert_eq!(
            split_rects_with_gap(area, 3, true, 0.0),
            split_rects(area, 3, true)
        );
    }

    #[test]
    fn split_rects_with_gap_quantises_a_fractional_gap_up() {
        let area = Rect::new(0.0, 0.0, 20.0, 10.0);
        let rects = split_rects_with_gap(area, 2, true, 0.4);
        // 0.4 rounds up to a whole unit of gap (1.0).
        assert_eq!(rects[1].x - (rects[0].x + rects[0].width), 1.0);
    }
}
