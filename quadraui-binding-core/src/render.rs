//! Render-to-frame assembly: paints either the six-fixed-slot
//! [`RenderFrame`]/[`RenderRefs`] shape ([`draw_bands`]) or a decoded
//! [`crate::layout::LayoutNode`] tree ([`paint_layout`]) into a
//! [`quadraui::Backend`], recording a [`PaintedHit`] per primitive painted
//! so a later pointer event can be hit-tested against this frame.

use std::collections::HashMap;
use std::rc::Rc;

use serde::{Deserialize, Deserializer};

use quadraui::{
    Backend, Button, DataTable, FontRole, InteractionState, ListView, Rect, Scrollbar, SectionSize,
    Split, SplitDirection, SplitHit, SplitLayout, TabBar, TabChrome, TextDisplay,
    TextDisplayLayout, TextRegion, TreeView, WidgetId,
};

use crate::extra_paint::ExtraPaint;
use crate::hit::PaintedHit;
use crate::layout::{
    align_cross, justify_starts, quantise_horizontal, quantise_vertical, resolve_main_axis_sizes,
    Align, LayoutContainer, LayoutLeaf, LayoutNode, LeafDescriptor, MainAxisSpec,
};

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
    /// Optional pixel-precise scroll position lifted out of the `"list"`
    /// slot's raw dict by [`crate::scroll_px_from_value`] — `None` unless
    /// the dict set a numeric `"scroll_px"` key, in which case it wins
    /// over [`ListView::scroll_offset`] in [`draw_bands`].
    pub list_scroll_px: Option<f32>,
    /// Same as `list_scroll_px`, for the `"tree"` slot.
    pub tree_scroll_px: Option<f32>,
    /// Same as `list_scroll_px`, for the `"data_table"` slot.
    pub data_table_scroll_px: Option<f32>,
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
            list_scroll_px: self.list_scroll_px,
            tree_scroll_px: self.tree_scroll_px,
            data_table_scroll_px: self.data_table_scroll_px,
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
    /// See [`RenderFrame::list_scroll_px`]. `None` for a `RenderRefs` a
    /// binding builds by hand from its own static fields (no dict to
    /// lift a `scroll_px` out of) — [`draw_bands`] paints exactly as
    /// before in that case.
    pub list_scroll_px: Option<f32>,
    /// See [`RenderFrame::tree_scroll_px`].
    pub tree_scroll_px: Option<f32>,
    /// See [`RenderFrame::data_table_scroll_px`].
    pub data_table_scroll_px: Option<f32>,
}

/// Allocates the next `lines`-tall band (scaled by `line_height`),
/// starting at the mutable cursor `*y` and never exceeding `viewport`'s
/// remaining height — the sequential top-to-bottom allocation [`draw_bands`]
/// uses for each of its six fixed slots plus every additive primitive.
/// [`crate::host::resolve_render_refs`] calls this directly too, so it
/// computes each occupied slot's pre-paint rect from the exact same band
/// order and sizes `draw_bands` itself paints into, rather than a second,
/// independently-drifting copy of this sequence. Returns `None` once the
/// viewport has run out of vertical room; `*y` is only advanced when a
/// band is actually returned.
pub(crate) fn next_band(
    y: &mut f32,
    viewport_width: f32,
    viewport_height: f32,
    line_height: f32,
    lines: f32,
) -> Option<Rect> {
    let remaining = viewport_height - *y;
    if remaining <= 0.0 {
        return None;
    }
    let band_height = (lines * line_height).min(remaining);
    let rect = Rect::new(0.0, *y, viewport_width, band_height);
    *y += band_height;
    Some(rect)
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
    let mut band = |lines: f32| -> Option<Rect> {
        next_band(&mut y, viewport.width, viewport.height, line_height, lines)
    };

    if let Some(button) = refs.button {
        if let Some(rect) = band(3.0) {
            let layout = backend.draw_button(rect, button);
            hits.push(PaintedHit::Button {
                id: button.id,
                layout,
            });
        }
    }
    if let Some(text_display) = refs.text_display {
        if let Some(rect) = band(3.0) {
            hits.push(draw_text_display_hit(
                backend,
                text_display.id,
                rect,
                text_display,
            ));
        }
    }
    if let Some(list) = refs.list {
        if let Some(rect) = band(4.0) {
            let state = quadraui::ListPaintState::default().with_scroll_px(refs.list_scroll_px);
            backend.draw_list_with_state(rect, list, &state, false);
            let layout = backend.list_layout(rect, list);
            let viewport_rows = list_viewport_rows(backend, rect, list);
            hits.push(PaintedHit::List {
                id: list.id,
                area: rect,
                layout,
                selected_idx: list.selected_idx,
                total_items: list.items.len(),
                scroll_offset: list.scroll_offset,
                viewport_rows,
            });
        }
    }
    if let Some(tree) = refs.tree {
        if let Some(rect) = band(4.0) {
            let (content_rect, scrollbar, viewport_rows) =
                draw_tree_with_scrollbar_and_state(backend, rect, tree, refs.tree_scroll_px);
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
        if let Some(rect) = band(4.0) {
            let state =
                quadraui::DataTablePaintState::default().with_scroll_px(refs.data_table_scroll_px);
            let layout = backend.draw_data_table_with_state(rect, data_table, &state);
            let viewport_rows = layout.visible_rows;
            hits.push(PaintedHit::DataTable {
                id: data_table.id,
                area: rect,
                layout,
                scroll_offset: data_table.scroll_offset,
                total_rows: data_table.rows.len(),
                selected_idx: data_table.selected_idx,
                viewport_rows,
            });
        }
    }
    if let Some(tab_bar) = refs.tab_bar {
        if let Some(rect) = band(3.0) {
            let layout = backend.draw_tab_bar_layout(rect, tab_bar, None);
            hits.push(PaintedHit::TabBar {
                id: tab_bar.id,
                area: rect,
                layout,
                focused_index: None,
                tab_count: tab_bar.tabs.len(),
            });
        }
    }
    // Additive primitives, painted in canonical `EXTRA_PRIMITIVES` order so
    // the output is deterministic regardless of `HashMap` iteration order.
    for kind in crate::extra_paint::EXTRA_PRIMITIVE_KINDS {
        if let Some(extra) = refs.extra.get(kind) {
            if let Some(rect) = band(3.0) {
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

/// Paints a layout tree into `area`. A `Row`/`Column` first shrinks `area`
/// by its own `padding`/`margin` and reserves `gap` between children (see
/// [`inset_for_spacing`]/[`split_rects_with_gap`] — a no-spacing container
/// behaves exactly as before these existed), then divides what's left into
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
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = inset_for_spacing(area, container, char_width, line_height);
            let rects = layout_container_rects(backend, area, container, true);
            for (child, rect) in container.children.iter().zip(rects) {
                paint_layout(backend, rect, &child.node, hits, chrome_interaction);
            }
        }
        LayoutNode::Column(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = inset_for_spacing(area, container, char_width, line_height);
            let rects = layout_container_rects(backend, area, container, false);
            for (child, rect) in container.children.iter().zip(rects) {
                paint_layout(backend, rect, &child.node, hits, chrome_interaction);
            }
        }
        LayoutNode::Leaf(leaf) => paint_leaf(backend, area, leaf, hits, chrome_interaction),
        LayoutNode::Split(split) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = split.spacing.inset(area, char_width, line_height);
            let horizontal = matches!(split.split.direction, SplitDirection::Horizontal);
            // `split.split.first_min`/`second_min` are decoded in the same
            // portable surface-native-pixel unit `Spacing`'s own fields
            // use — see `crate::wire::decode_layout_node`'s split decode —
            // so they're quantised into the backend's native unit here,
            // right before painting, exactly like `gap` just below:
            // `quantise_horizontal` (against `char_width`) for a
            // `Horizontal` split, where `first_min`/`second_min` bound the
            // pane's *width*; `quantise_vertical` (against `line_height`)
            // for a `Vertical` one, where they bound its *height*. Using
            // the wrong axis's metric would scale a minimum meant for one
            // dimension by the other backend's font metric for the other.
            let (min_metric, quantise_min): (f32, fn(f32, f32) -> f32) = if horizontal {
                (char_width, quantise_horizontal)
            } else {
                (line_height, quantise_vertical)
            };
            let native_split = Split {
                first_min: quantise_min(split.split.first_min, min_metric),
                second_min: quantise_min(split.split.second_min, min_metric),
                ..split.split.clone()
            };
            let gap = if horizontal {
                quantise_horizontal(split.spacing.gap, char_width)
            } else {
                quantise_vertical(split.spacing.gap, line_height)
            };
            // `Split::layout`'s own min-size clamping evaluates against
            // whatever rect it's handed, so the gap is carved out of
            // `area` BEFORE calling `draw_split` -- `first_min`/
            // `second_min` apply to the pane after the gap is already
            // subtracted, not before.
            let draw_area = shrink_main_axis(area, horizontal, gap);
            let mut layout = backend.draw_split(draw_area, &native_split);
            apply_split_gap(&mut layout, horizontal, gap);
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
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = tabs.spacing.inset(area, char_width, line_height);
            let strip_height = (3.0 * backend.line_height()).min(area.height);
            let strip_rect = Rect::new(area.x, area.y, area.width, strip_height);
            // `focused_index` threads through `TabChrome` rather than
            // `tabs.tab_bar` itself, so `None` (the common case) composes
            // to `TabChrome::default()`. Neither `Backend` the bindings
            // actually render with (`TuiBackend`, `GtkBackend`) takes the
            // trait-default `draw_tab_bar_with_chrome_layout` passthrough
            // to `draw_tab_bar_layout` — both override it with their own
            // measurer and paint call. `TuiBackend`'s override reproduces
            // plain `draw_tab_bar_layout` exactly for `TabChrome::default()`
            // — see `tui::backend::tests::draw_tab_bar_with_chrome_layout_default_chrome_matches_draw_tab_bar_layout`,
            // which asserts this at the trait-method level this call site
            // uses, not just at the free-function level.
            // `GtkBackend`'s equivalent is unverified by an equivalent
            // test; its own `draw_tab_bar_with_chrome_layout` is at
            // `gtk/backend.rs`.
            let chrome = TabChrome::default().with_focused_index(tabs.focused_index);
            let layout =
                backend.draw_tab_bar_with_chrome_layout(strip_rect, &tabs.tab_bar, None, &chrome);
            hits.push(PaintedHit::TabBar {
                id: tabs.tab_bar.id,
                area: strip_rect,
                layout,
                focused_index: tabs.focused_index,
                tab_count: tabs.tab_bar.tabs.len(),
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
        #[allow(deprecated)] // `Panel` is `#[deprecated]`; this arm is its own plumbing.
        LayoutNode::Panel(panel_node) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = panel_node.spacing.inset(area, char_width, line_height);
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
                toolbar: sidebar_node.sidebar_panel.toolbar.clone(),
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

/// A node's intrinsic `(width, height)`, each `None` when no cheap
/// measurement exists for that axis on this node kind — see
/// [`natural_size`], the only producer of this type.
struct NaturalSize {
    width: Option<f32>,
    height: Option<f32>,
}

impl NaturalSize {
    /// The extent along whichever axis is `horizontal`'s container's main
    /// axis (width for a `Row`, height for a `Column`).
    fn main(&self, horizontal: bool) -> Option<f32> {
        if horizontal {
            self.width
        } else {
            self.height
        }
    }

    /// [`Self::main`]'s counterpart on the container's cross axis.
    fn cross(&self, horizontal: bool) -> Option<f32> {
        if horizontal {
            self.height
        } else {
            self.width
        }
    }
}

/// Best-effort intrinsic size of `node`, consulted by
/// [`layout_container_rects`] for a [`quadraui::SectionSize::Content`]
/// child's main-axis size and for a non-`Stretch`
/// [`crate::layout::Align`]'s cross-axis size. Measured directly off the
/// descriptor already decoded for painting: `Button`/`TabBar` via
/// `Backend::measure_text` (the same measurer `quadraui::compose::
/// menu_system` already leans on for its own label widths), `TextDisplay`
/// via its own line count/widest line, `List`/`Tree`/`DataTable` via their
/// own row count (height only — none of the three has a cheap intrinsic
/// width). Every other node kind (a nested `Row`/`Column`/`Split`/`Tabs`/
/// `Panel`/`SidebarPanel`/`SplitTree`, or an `Extra` additive primitive)
/// has no cheap measurement at this layer and reports [`None`] on both
/// axes — see [`NaturalSize::main`]/[`NaturalSize::cross`]'s callers for
/// how each degrades a `None`.
fn natural_size(backend: &mut dyn Backend, node: &LayoutNode, line_height: f32) -> NaturalSize {
    let LayoutNode::Leaf(leaf) = node else {
        return NaturalSize {
            width: None,
            height: None,
        };
    };
    match &leaf.descriptor {
        LeafDescriptor::Button(button) => {
            let (label_w, label_h) = backend.measure_text(&button.label, FontRole::Chrome);
            // `measure_text` reports the bare label; every non-`Icon`
            // variant paints chrome around it too (TUI's `"[ Label ]"`
            // bracket chrome — see `quadraui::tui::button` — a pixel
            // backend's own left/right `Button` padding). Omitting this
            // would undersize the natural width enough to clip the
            // chrome under `Align::Start`/`Center`/`End` or a `Content`
            // main-axis size, not just misalign it. `char_width <= 1.0`
            // is the same cell-grid-backend test `layout::native_units`
            // uses; the pixel-backend branch is a nominal estimate, not
            // a real `Style::padding` read (this layer has no `Style`
            // handle), so it's the shakier of the two.
            let chrome_w = if backend.char_width() <= 1.0 {
                4.0
            } else {
                24.0
            };
            NaturalSize {
                width: Some(label_w + chrome_w),
                height: Some(label_h),
            }
        }
        LeafDescriptor::TextDisplay(text) => {
            let width = text
                .lines
                .iter()
                .map(|line| {
                    let line_text: String =
                        line.spans.iter().map(|span| span.text.as_str()).collect();
                    // `TextDisplay` is an `EditorClassPrimitive` (see
                    // `quadraui::font_role`) -- its own lines paint in
                    // `FontRole::Editor`, not the chrome font a `Button`/
                    // `TabBar` label uses.
                    backend.measure_text(&line_text, FontRole::Editor).0
                })
                .fold(0.0_f32, f32::max);
            NaturalSize {
                width: Some(width),
                height: Some(text.lines.len() as f32 * line_height),
            }
        }
        LeafDescriptor::List(list) => NaturalSize {
            width: None,
            height: Some(list.items.len() as f32 * line_height),
        },
        LeafDescriptor::Tree(tree) => NaturalSize {
            width: None,
            height: Some(tree.rows.len() as f32 * line_height),
        },
        LeafDescriptor::DataTable(table) => NaturalSize {
            width: None,
            height: Some(table.rows.len() as f32 * line_height),
        },
        LeafDescriptor::TabBar(tab_bar) => {
            let width = tab_bar
                .tabs
                .iter()
                .map(|tab| backend.measure_text(&tab.label, FontRole::Chrome).0)
                .sum::<f32>();
            // Top border + one row of labels + bottom border, TUI's own
            // `quadraui::tui::tab_bar` chrome frame -- three line-heights
            // regardless of backend, since no pixel backend reports a
            // narrower natural height for the same chrome today.
            const BORDER_LABEL_BORDER_ROWS: f32 = 3.0;
            NaturalSize {
                width: Some(width),
                height: Some(BORDER_LABEL_BORDER_ROWS * line_height),
            }
        }
        LeafDescriptor::Extra(_) => NaturalSize {
            width: None,
            height: None,
        },
    }
}

/// Resolves a [`LayoutContainer`]'s children into their final paint rects:
/// each child's [`quadraui::SectionSize`] main-axis size (clamped by its
/// own `min`/`max`, both quantised into native units here — the same
/// treatment [`crate::layout::Spacing`]'s own fields get), the container's
/// [`crate::layout::Justify`] distributing any leftover main-axis space,
/// and the container's [`crate::layout::Align`] sizing/positioning each
/// child on the cross axis. `area` is already the container's own inset
/// area (post `padding`/`margin` — see [`inset_for_spacing`]); `horizontal`
/// is `true` for a `Row` (main axis = width), `false` for a `Column` (main
/// axis = height). The one call site each in [`paint_layout`] and
/// `crate::host::resolve_layout_node` share this so a click always
/// resolves against the exact geometry just painted.
pub fn layout_container_rects(
    backend: &mut dyn Backend,
    area: Rect,
    container: &LayoutContainer,
    horizontal: bool,
) -> Vec<Rect> {
    let n = container.children.len();
    if n == 0 {
        return Vec::new();
    }
    let char_width = backend.char_width();
    let line_height = backend.line_height();
    let (main_metric, quantise_main): (f32, fn(f32, f32) -> f32) = if horizontal {
        (char_width, quantise_horizontal)
    } else {
        (line_height, quantise_vertical)
    };
    let gap = quantise_main(container.spacing.gap, main_metric);
    let container_main = if horizontal { area.width } else { area.height };
    let usable_main = (container_main - gap * (n - 1) as f32).max(0.0);

    let specs: Vec<MainAxisSpec> = container
        .children
        .iter()
        .map(|child| {
            // `SectionSize::Fixed`'s own `u16` is decoded in the same
            // portable surface-native-pixel unit as `min`/`max` below —
            // quantise it into native units the same way before
            // `resolve_main_axis_sizes` ever sees it.
            let size = match child.size {
                SectionSize::Fixed(raw) => {
                    SectionSize::Fixed(quantise_main(raw as f32, main_metric).round() as u16)
                }
                other => other,
            };
            let content = if matches!(size, SectionSize::Content) {
                natural_size(backend, &child.node, line_height)
                    .main(horizontal)
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            MainAxisSpec {
                size,
                min: child.min.map(|v| quantise_main(v, main_metric)),
                max: child.max.map(|v| quantise_main(v, main_metric)),
                content,
            }
        })
        .collect();

    let sizes = resolve_main_axis_sizes(&specs, container_main, usable_main);
    // `justify_starts` wants the container's full main-axis extent, not
    // `usable_main` (already gap-reduced for `resolve_main_axis_sizes`'s
    // own budget) — it reinserts the full gap total itself when computing
    // its own `leftover`, so passing the gap-reduced value here would
    // subtract the gap twice.
    let starts = justify_starts(&sizes, container_main, gap, container.justify);

    let cross_extent = if horizontal { area.height } else { area.width };
    container
        .children
        .iter()
        .zip(sizes.iter())
        .zip(starts.iter())
        .map(|((child, &size), &start)| {
            // `align_cross` ignores `natural_cross` entirely under the
            // default `Align::Stretch` -- skip the measurement (a `String`
            // allocation plus a `Backend::measure_text` call for a
            // `TextDisplay`) so the pre-`align` default path stays free of
            // it, the same way the main-axis `Content` size already skips
            // `natural_size` for every other `SectionSize`.
            let natural_cross = if container.align == Align::Stretch {
                None
            } else {
                natural_size(backend, &child.node, line_height).cross(horizontal)
            };
            let (cross_offset, cross_size) =
                align_cross(cross_extent, natural_cross, container.align);
            if horizontal {
                Rect::new(area.x + start, area.y + cross_offset, size, cross_size)
            } else {
                Rect::new(area.x + cross_offset, area.y + start, cross_size, size)
            }
        })
        .collect()
}

/// Shrinks `area`'s main-axis extent by `gap`, anchored at the same
/// origin — the rect [`paint_layout`]'s `Split` arm feeds `Backend::draw_split`
/// so `Split::layout`'s own ratio/min-size clamping evaluates against the
/// space actually available once the gap between panes is carved out,
/// matching `Split::first_min`/`second_min`'s "a minimum applies to the
/// pane" contract. [`apply_split_gap`] reinserts the carved-out space
/// after painting.
pub(crate) fn shrink_main_axis(area: Rect, horizontal: bool, gap: f32) -> Rect {
    if horizontal {
        Rect::new(area.x, area.y, (area.width - gap).max(0.0), area.height)
    } else {
        Rect::new(area.x, area.y, area.width, (area.height - gap).max(0.0))
    }
}

/// Reinserts `gap` between a [`SplitLayout`]'s two panes after
/// `Backend::draw_split` painted it into [`shrink_main_axis`]'s
/// gap-reduced rect: the divider and first pane stay exactly where they
/// were painted (so the divider's own visuals/hit region are untouched),
/// and the second pane — its `second_bounds` plus its `SplitHit::SecondPane`
/// hit region — shifts forward by `gap` along the main axis, so the two
/// panes end up visually separated by blank space rather than both
/// touching the divider. A no-op when `gap` is `0.0`, the overwhelmingly
/// common case.
pub(crate) fn apply_split_gap(layout: &mut SplitLayout, horizontal: bool, gap: f32) {
    if gap <= 0.0 {
        return;
    }
    let shift = |rect: Rect| -> Rect {
        if horizontal {
            Rect::new(rect.x + gap, rect.y, rect.width, rect.height)
        } else {
            Rect::new(rect.x, rect.y + gap, rect.width, rect.height)
        }
    };
    layout.second_bounds = shift(layout.second_bounds);
    for (rect, hit) in layout.hit_regions.iter_mut() {
        if matches!(hit, SplitHit::SecondPane(_)) {
            *rect = shift(*rect);
        }
    }
    // Widen `bounds` back out so it reports the real (pre-shrink) area
    // this split occupies, not the gap-reduced rect fed to `draw_split`.
    layout.bounds = if horizontal {
        Rect::new(
            layout.bounds.x,
            layout.bounds.y,
            layout.bounds.width + gap,
            layout.bounds.height,
        )
    } else {
        Rect::new(
            layout.bounds.x,
            layout.bounds.y,
            layout.bounds.width,
            layout.bounds.height + gap,
        )
    };
}

/// Shrinks `area` by `container.spacing` — see [`crate::layout::Spacing::inset`]
/// for the margin-then-padding rule and why `char_width`/`line_height` (the
/// painting backend's own `Backend::char_width()`/`Backend::line_height()`)
/// are needed. Kept as its own name for every `Row`/`Column` call site;
/// `Split`/`Tabs`/`Panel` call `Spacing::inset` directly since they have no
/// [`LayoutContainer`] to unwrap it from.
fn inset_for_spacing(
    area: Rect,
    container: &LayoutContainer,
    char_width: f32,
    line_height: f32,
) -> Rect {
    container.spacing.inset(area, char_width, line_height)
}

/// Divides `area` into `n` equal shares, side by side when `horizontal` or
/// stacked top-to-bottom otherwise, with `gap` native units of space
/// between adjacent shares — quantised via [`quantise_horizontal`] (using
/// `char_width`) when `horizontal`, or [`crate::layout::quantise_vertical`]
/// (using `line_height`) otherwise, so the *distance* between two adjacent
/// shares is always a whole number of TUI cells. The shares themselves
/// stay fractional (`(area's main-axis extent - total_gap) / n`), so an
/// individual share's own start position is not guaranteed to land on a
/// cell boundary — only the gap carved out of the space between shares
/// is quantised. Returns an empty `Vec` for `n == 0`. `gap: 0.0` behaves
/// exactly like the equal-share split this replaced. When `gap` alone
/// consumes the whole of `area` (so every share clamps to `0.0`), every
/// returned rect's origin is clamped to `area`'s own far edge instead of
/// marching past it — a share is always zero-width/height in that case,
/// so nothing paints either way, but the geometry itself never escapes
/// `area`.
pub fn split_rects_with_gap(
    area: Rect,
    n: usize,
    horizontal: bool,
    gap: f32,
    char_width: f32,
    line_height: f32,
) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let gap = if horizontal {
        quantise_horizontal(gap, char_width)
    } else {
        quantise_vertical(gap, line_height)
    };
    let n_f = n as f32;
    let total_gap = gap * (n.saturating_sub(1)) as f32;
    (0..n)
        .map(|i| {
            let i_f = i as f32;
            if horizontal {
                let share = ((area.width - total_gap) / n_f).max(0.0);
                let x = (area.x + (share + gap) * i_f).min(area.x + area.width);
                Rect::new(x, area.y, share, area.height)
            } else {
                let share = ((area.height - total_gap) / n_f).max(0.0);
                let y = (area.y + (share + gap) * i_f).min(area.y + area.height);
                Rect::new(area.x, y, area.width, share)
            }
        })
        .collect()
}

/// Divides `area` into `n` equal shares with no gap between them —
/// [`split_rects_with_gap`] with `gap: 0.0`, kept as its own name for
/// every caller that has no [`LayoutContainer`] spacing to thread
/// through. `gap: 0.0` never consults either metric, so `char_width`/
/// `line_height` here are nominal.
pub fn split_rects(area: Rect, n: usize, horizontal: bool) -> Vec<Rect> {
    split_rects_with_gap(area, n, horizontal, 0.0, 1.0, 1.0)
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

/// How many items of `list` fit in `rect` at a zero scroll offset — the
/// viewport's true row capacity, mirroring [`tree_viewport_rows`] for the
/// identical reason: `ListViewLayout::visible_items.len()` at `list`'s
/// ACTUAL offset can be shorter near the end of a long list. Paints into
/// no backend at all — only queries `Backend::list_layout` against a
/// scratch clone — so computing this never disturbs the real paint.
pub fn list_viewport_rows(backend: &mut dyn Backend, rect: Rect, list: &ListView) -> usize {
    let mut shadow = list.clone();
    shadow.scroll_offset = 0;
    backend.list_layout(rect, &shadow).visible_items.len()
}

/// How many rows of `table` fit in `rect` at a zero scroll offset,
/// mirroring [`tree_viewport_rows`]/[`list_viewport_rows`] for the
/// identical reason: [`quadraui::DataTableLayout::visible_rows`] at
/// `table`'s ACTUAL offset can be shorter near the end of a long table.
/// Paints into no backend at all — only queries `Backend::data_table_layout`
/// against a scratch clone — so computing this never disturbs the real
/// paint.
pub fn data_table_viewport_rows(backend: &mut dyn Backend, rect: Rect, table: &DataTable) -> usize {
    let mut shadow = table.clone();
    shadow.scroll_offset = 0;
    backend.data_table_layout(rect, &shadow).visible_rows
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
    draw_tree_with_scrollbar_and_state(backend, area, tree, None)
}

/// [`draw_tree_with_scrollbar`], plus an optional pixel-precise
/// `scroll_px` (lifted out of the leaf's raw descriptor dict by
/// [`crate::scroll_px_from_value`]) painted via
/// [`Backend::draw_tree_with_state`] instead of [`Backend::draw_tree`].
/// `scroll_px: None` paints exactly as [`draw_tree_with_scrollbar`]
/// already does; `Some(px)` also moves the scrollbar thumb to the same
/// continuous position [`crate::primitives::TreeController`]'s own
/// scrollbar does (`px / line_height`, not quantised to a whole row).
pub fn draw_tree_with_scrollbar_and_state(
    backend: &mut dyn Backend,
    area: Rect,
    tree: &TreeView,
    scroll_px: Option<f32>,
) -> (Rect, Option<Scrollbar>, usize) {
    let state = quadraui::TreePaintState::default().with_scroll_px(scroll_px);
    if backend.tree_paints_own_scrollbar() {
        backend.draw_tree_with_state(area, tree, &state, false);
        let scrollbar = backend.tree_vscrollbar(area, tree);
        let viewport_rows = tree_viewport_rows(backend, area, tree);
        return (area, scrollbar, viewport_rows);
    }
    let track_width = backend.line_height();
    if area.width <= track_width {
        backend.draw_tree_with_state(area, tree, &state, false);
        let viewport_rows = tree_viewport_rows(backend, area, tree);
        return (area, None, viewport_rows);
    }
    let content_rect = Rect::new(area.x, area.y, area.width - track_width, area.height);
    let viewport_rows = tree_viewport_rows(backend, content_rect, tree);
    if tree.rows.len() <= viewport_rows {
        backend.draw_tree_with_state(area, tree, &state, false);
        return (area, None, viewport_rows);
    }
    backend.draw_tree_with_state(content_rect, tree, &state, false);
    let track_rect = Rect::new(
        area.x + content_rect.width,
        area.y,
        track_width,
        area.height,
    );
    let line_height = backend.line_height();
    let thumb_offset = match scroll_px {
        Some(px) if line_height > 0.0 => px / line_height,
        _ => tree.scroll_offset as f32,
    };
    let scrollbar = Scrollbar::vertical(
        format!("{}-scrollbar", tree.id),
        track_rect,
        thumb_offset,
        tree.rows.len() as f32,
        viewport_rows as f32,
        track_width.max(1.0),
    );
    backend.draw_scrollbar(track_rect, &scrollbar);
    (content_rect, Some(scrollbar), viewport_rows)
}

/// Walks `node`'s layout tree collecting the tree-address `id` ->
/// `Button::tooltip` text of every `Button` leaf that sets one — the
/// layout-tree counterpart of a `BindingHost`'s own `button` slot read.
/// [`paint_leaf`] keys a layout-tree leaf's [`PaintedHit::Button`] by this
/// same tree-address id (not the descriptor's own embedded `Button::id`,
/// which a render callback may leave at its default and reuse across
/// leaves — see [`paint_leaf`]'s own doc), so a binding that paints
/// through [`paint_layout`] calls this once per render and feeds the
/// result to `crate::host::BindingHost::set_leaf_tooltips`: one
/// `crate::host::BindingHost::hover_tooltip_text` lookup then resolves a
/// hovered id from either paint path. A tab not currently selected still
/// contributes its own leaves here even though [`paint_layout`] never
/// paints them — harmless, since no [`PaintedHit`] for an unpainted leaf
/// ever exists for [`crate::host::BindingHost::hover_anchor`] to find.
pub fn collect_leaf_tooltips(node: &LayoutNode, out: &mut HashMap<WidgetId, String>) {
    match node {
        LayoutNode::Row(container) | LayoutNode::Column(container) => {
            for child in &container.children {
                collect_leaf_tooltips(&child.node, out);
            }
        }
        LayoutNode::Leaf(leaf) => {
            if let LeafDescriptor::Button(button) = &leaf.descriptor {
                if let Some(tooltip) = &button.tooltip {
                    out.insert(WidgetId::new(leaf.id.clone()), tooltip.clone());
                }
            }
        }
        LayoutNode::Split(split) => {
            collect_leaf_tooltips(&split.first, out);
            collect_leaf_tooltips(&split.second, out);
        }
        LayoutNode::Tabs(tabs) => {
            for entry in &tabs.entries {
                collect_leaf_tooltips(&entry.content, out);
            }
        }
        LayoutNode::Panel(panel_node) => collect_leaf_tooltips(&panel_node.content, out),
        LayoutNode::SidebarPanel(sidebar_node) => collect_leaf_tooltips(&sidebar_node.content, out),
        LayoutNode::SplitTree(split_tree) => {
            for leaf in &split_tree.leaves {
                collect_leaf_tooltips(leaf, out);
            }
        }
    }
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
            let state = quadraui::ListPaintState::default().with_scroll_px(leaf.scroll_px);
            backend.draw_list_with_state(area, list, &state, false);
            let layout = backend.list_layout(area, list);
            let viewport_rows = list_viewport_rows(backend, area, list);
            hits.push(PaintedHit::List {
                id,
                area,
                layout,
                selected_idx: list.selected_idx,
                total_items: list.items.len(),
                scroll_offset: list.scroll_offset,
                viewport_rows,
            });
        }
        LeafDescriptor::Tree(tree) => {
            let (content_rect, scrollbar, viewport_rows) =
                draw_tree_with_scrollbar_and_state(backend, area, tree, leaf.scroll_px);
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
            let state = quadraui::DataTablePaintState::default().with_scroll_px(leaf.scroll_px);
            let layout = backend.draw_data_table_with_state(area, data_table, &state);
            let viewport_rows = layout.visible_rows;
            hits.push(PaintedHit::DataTable {
                id,
                area,
                layout,
                scroll_offset: data_table.scroll_offset,
                total_rows: data_table.rows.len(),
                selected_idx: data_table.selected_idx,
                viewport_rows,
            });
        }
        LeafDescriptor::TabBar(tab_bar) => {
            let layout = backend.draw_tab_bar_layout(area, tab_bar, None);
            hits.push(PaintedHit::TabBar {
                id,
                area,
                layout,
                focused_index: None,
                tab_count: tab_bar.tabs.len(),
            });
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
mod tests {
    use super::*;
    use crate::layout::{EdgeInsets, Spacing};

    /// A real font's reported char/line metrics (GTK/macOS/Win) — any
    /// value `> 1.0` takes [`crate::layout::quantise_horizontal`]/
    /// [`quantise_vertical`]'s pixel-backend passthrough branch, so these
    /// particular numbers aren't load-bearing, just plausible ones.
    const PIXEL_CHAR_WIDTH: f32 = 8.0;
    const PIXEL_LINE_HEIGHT: f32 = 16.0;

    /// `TuiBackend::char_width`/`line_height`'s own values — the
    /// architectural `1.0` that flags a cell-grid backend.
    const TUI_CHAR_WIDTH: f32 = 1.0;
    const TUI_LINE_HEIGHT: f32 = 1.0;

    fn container(children: usize, spacing: Spacing) -> LayoutContainer {
        LayoutContainer::new(
            (0..children)
                .map(|_| LayoutNode::Row(LayoutContainer::new(Vec::new(), Spacing::default())))
                .collect(),
            spacing,
        )
    }

    #[test]
    fn inset_for_spacing_applies_margin_then_padding_on_a_pixel_backend() {
        let area = Rect::new(0.0, 0.0, 100.0, 50.0);
        let c = container(
            0,
            Spacing {
                padding: EdgeInsets::uniform(5.0),
                margin: 3.0,
                gap: 0.0,
            },
        );
        let inset = inset_for_spacing(area, &c, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        // margin (3) then padding (5) on every side: 8 total inset, in
        // surface-native pixels, unchanged by the pixel-backend passthrough.
        assert_eq!(inset.x, 8.0);
        assert_eq!(inset.y, 8.0);
        assert_eq!(inset.width, 100.0 - 16.0);
        assert_eq!(inset.height, 50.0 - 16.0);
    }

    #[test]
    fn inset_for_spacing_drops_a_sub_cell_margin_on_a_pixel_backend() {
        let area = Rect::new(0.0, 0.0, 100.0, 50.0);
        let c = container(
            0,
            Spacing {
                padding: EdgeInsets::default(),
                margin: 0.5,
                gap: 0.0,
            },
        );
        let inset = inset_for_spacing(area, &c, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        assert_eq!(inset, area, "a margin under one native unit drops to zero");
    }

    /// The blocking regression this module exists to pin down: a pixel
    /// padding value shrinks into a *small* number of TUI cells instead of
    /// being read as that many whole cells outright (`padding: 12` must
    /// not mean "12-cell inset" on a cell-grid backend).
    #[test]
    fn inset_for_spacing_shrinks_a_pixel_padding_into_whole_cells_on_tui() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let c = container(
            0,
            Spacing {
                padding: EdgeInsets::uniform(12.0),
                margin: 0.0,
                gap: 0.0,
            },
        );
        let inset = inset_for_spacing(area, &c, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        // ceil(12 / 8) = 2 columns each side, ceil(12 / 16) = 1 row each side.
        assert_eq!(inset.x, 2.0);
        assert_eq!(inset.y, 1.0);
        assert_eq!(inset.width, 80.0 - 4.0);
        assert_eq!(inset.height, 24.0 - 2.0);
    }

    /// The literal acceptance scenario the blocking review flagged: a
    /// `padding: 12, gap: 8` row of three children on a stock 80×24
    /// terminal must still leave real room to paint into, not clip to
    /// zero height.
    #[test]
    fn split_rects_with_gap_does_not_clip_a_terminal_sized_row() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let c = container(
            3,
            Spacing {
                padding: EdgeInsets::uniform(12.0),
                margin: 0.0,
                gap: 8.0,
            },
        );
        let inset = inset_for_spacing(area, &c, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        assert!(inset.width > 0.0, "row must not clip to zero width");
        assert!(inset.height > 0.0, "row must not clip to zero height");
        let rects = split_rects_with_gap(
            inset,
            c.children.len(),
            true,
            c.spacing.gap,
            TUI_CHAR_WIDTH,
            TUI_LINE_HEIGHT,
        );
        assert_eq!(rects.len(), 3);
        for rect in &rects {
            assert!(
                rect.width > 0.0,
                "every child must get real width: {rect:?}"
            );
            assert!(
                rect.height > 0.0,
                "every child must get real height: {rect:?}"
            );
        }
    }

    /// When `gap` alone consumes the whole area, every share clamps to
    /// `0.0` but the geometry itself must still stay inside `area` — see
    /// [`split_rects_with_gap`]'s doc for why marching origins past the
    /// far edge is a latent hit-test/overdraw hazard even though nothing
    /// actually paints into a zero-sized rect.
    #[test]
    fn split_rects_with_gap_clamps_origins_when_gap_exceeds_the_area() {
        let area = Rect::new(0.0, 0.0, 10.0, 10.0);
        let rects = split_rects_with_gap(area, 5, true, 4.0, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        assert_eq!(rects.len(), 5);
        for rect in &rects {
            assert_eq!(rect.width, 0.0);
            assert!(
                rect.x <= area.x + area.width,
                "origin must not escape area: {rect:?}"
            );
        }
    }

    #[test]
    fn split_rects_with_gap_leaves_whole_unit_gaps_between_shares() {
        let area = Rect::new(0.0, 0.0, 33.0, 10.0);
        let rects = split_rects_with_gap(area, 3, true, 3.0, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
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
            split_rects_with_gap(area, 3, true, 0.0, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT),
            split_rects(area, 3, true)
        );
    }

    #[test]
    fn split_rects_with_gap_quantises_a_fractional_gap_up() {
        let area = Rect::new(0.0, 0.0, 20.0, 10.0);
        let rects = split_rects_with_gap(area, 2, true, 0.4, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        // 0.4 rounds up to a whole unit of gap (1.0).
        assert_eq!(rects[1].x - (rects[0].x + rects[0].width), 1.0);
    }

    /// The same fractional gap from a cell-grid backend's own metrics:
    /// `0.4` surface-native pixels is nowhere near one nominal 8px-wide
    /// cell, so it still rounds up to exactly one whole TUI cell of gap —
    /// same outcome as the pixel-backend case above, by a different route.
    #[test]
    fn split_rects_with_gap_quantises_a_fractional_gap_up_on_tui() {
        let area = Rect::new(0.0, 0.0, 20.0, 10.0);
        let rects = split_rects_with_gap(area, 2, true, 0.4, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        assert_eq!(rects[1].x - (rects[0].x + rects[0].width), 1.0);
    }

    // ── Split/Tabs/Panel spacing (issue #2025) ──────────────────────────

    use quadraui::SplitMeasure;

    /// A `"split"` node's `margin`/`padding` inset the whole split area
    /// before `Split::layout` divides it — the exact [`Spacing::inset`]
    /// call `paint_layout`'s `LayoutNode::Split` arm makes, on a pixel
    /// backend where the padding/margin values pass straight through.
    #[test]
    fn split_spacing_insets_the_whole_area_on_a_pixel_backend() {
        let area = Rect::new(0.0, 0.0, 100.0, 50.0);
        let spacing = Spacing {
            padding: EdgeInsets::uniform(5.0),
            margin: 3.0,
            gap: 0.0,
        };
        let inset = spacing.inset(area, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        assert_eq!(inset, Rect::new(8.0, 8.0, 100.0 - 16.0, 50.0 - 16.0));
    }

    /// The same inset on TUI shrinks the pixel padding/margin into whole
    /// cells, same rule as a `"row"`/`"column"` node's own spacing.
    #[test]
    fn split_spacing_insets_the_whole_area_on_tui() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let spacing = Spacing {
            padding: EdgeInsets::uniform(12.0),
            margin: 0.0,
            gap: 0.0,
        };
        let inset = spacing.inset(area, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        // ceil(12 / 8) = 2 columns each side, ceil(12 / 16) = 1 row each side.
        assert_eq!(inset, Rect::new(2.0, 1.0, 80.0 - 4.0, 24.0 - 2.0));
    }

    /// [`shrink_main_axis`]/[`apply_split_gap`] together implement a
    /// split's `gap`: the available space `Split::layout` divides is
    /// reduced by the gap first (so `first_min`/`second_min` are evaluated
    /// against the space left AFTER the gap, not before), and the second
    /// pane is shifted forward by exactly the gap afterwards, landing it
    /// flush with the original area's far edge.
    #[test]
    fn split_gap_is_carved_out_before_dividing_then_reinserted() {
        let area = Rect::new(0.0, 0.0, 100.0, 40.0);
        let gap = 10.0;
        let draw_area = shrink_main_axis(area, true, gap);
        assert_eq!(draw_area, Rect::new(0.0, 0.0, 90.0, 40.0));

        let split = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let mut layout = split.layout(draw_area, SplitMeasure::new(1.0));
        let first_width_before = layout.first_bounds.width;
        let second_x_before = layout.second_bounds.x;

        apply_split_gap(&mut layout, true, gap);

        // The divider/first pane painted into `draw_area` stay exactly
        // where they were -- only the second pane and its hit region move.
        assert_eq!(layout.first_bounds.width, first_width_before);
        assert_eq!(layout.second_bounds.x, second_x_before + gap);
        // The second pane now reaches exactly the far edge of the
        // original (pre-shrink) area -- the gap didn't leak room outside
        // it, and didn't leave any slack short of it either.
        assert_eq!(
            layout.second_bounds.x + layout.second_bounds.width,
            area.x + area.width
        );
        let second_hit = layout
            .hit_regions
            .iter()
            .find(|(_, hit)| matches!(hit, SplitHit::SecondPane(_)))
            .expect("a second-pane hit region");
        assert_eq!(second_hit.0.x, second_x_before + gap);
    }

    /// `gap: 0.0` is a no-op — [`apply_split_gap`] leaves every rect and
    /// hit region exactly as `Backend::draw_split` returned it.
    #[test]
    fn split_gap_zero_is_a_no_op() {
        let area = Rect::new(0.0, 0.0, 100.0, 40.0);
        let split = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let mut layout = split.layout(area, SplitMeasure::new(1.0));
        let before = layout.clone();
        apply_split_gap(&mut layout, true, 0.0);
        assert_eq!(layout, before);
    }

    /// Minimal [`quadraui::AppLogic`] around a `LayoutNode::Split` of two
    /// empty `Row` children, with `first_min`/`second_min` both set to
    /// `min` — drives [`paint_layout`]'s `Split` arm through a real
    /// [`quadraui::tui::testing::TuiDriver`] frame (the same headless
    /// harness CLAUDE.md's testing doc calls for), the one real exercise
    /// of `crate::wire::decode_layout_node`'s documented unit for these
    /// two fields (the same portable surface-native-pixel unit
    /// `padding`/`margin`/`gap` use).
    struct MinSplitLogic {
        direction: SplitDirection,
        min: f32,
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for MinSplitLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let empty_row =
                || LayoutNode::Row(LayoutContainer::new(Vec::new(), Spacing::default()));
            let node = LayoutNode::Split(crate::layout::LayoutSplit {
                split: Split {
                    id: WidgetId::new("s"),
                    direction: self.direction,
                    ratio: 0.01,
                    first_min: self.min,
                    second_min: self.min,
                },
                first: Box::new(empty_row()),
                second: Box::new(empty_row()),
                spacing: Spacing::default(),
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
            self.engine.set_hits(hits);
        }

        fn handle(
            &mut self,
            _event: quadraui::UiEvent,
            _backend: &mut dyn Backend,
        ) -> quadraui::Reaction {
            quadraui::Reaction::Continue
        }
    }

    fn paint_split_with_min(
        direction: SplitDirection,
        min: f32,
        width: u16,
        height: u16,
    ) -> (Rect, Rect) {
        use quadraui::tui::testing::TuiDriver;

        let logic = MinSplitLogic {
            direction,
            min,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, width, height);
        driver.render();
        let layout = driver
            .app()
            .engine
            .hit_map()
            .iter()
            .find_map(|hit| match hit {
                PaintedHit::Split { layout } => Some(layout.clone()),
                _ => None,
            })
            .expect("the split should have painted");
        (layout.first_bounds, layout.second_bounds)
    }

    /// A `Horizontal` split's `first_min`/`second_min` bound the pane's
    /// *width*, so they quantise against `char_width` — `10` (portable
    /// px) quantises to `ceil(10 / 8) = 2` cells on `TuiBackend`, not the
    /// `ceil(10 / 16) = 1` a `line_height`-keyed quantisation (the pre-fix
    /// behaviour, which also skipped quantising altogether) would've
    /// produced.
    #[test]
    fn horizontal_split_min_quantises_against_char_width() {
        let (first, _second) = paint_split_with_min(SplitDirection::Horizontal, 10.0, 100, 10);
        assert_eq!(first.width, 2.0);
    }

    /// [`horizontal_split_min_quantises_against_char_width`]'s `Vertical`
    /// counterpart: `first_min`/`second_min` bound the pane's *height*
    /// there, so the same `10` quantises against `line_height` instead —
    /// `ceil(10 / 16) = 1` cell.
    #[test]
    fn vertical_split_min_quantises_against_line_height() {
        let (first, _second) = paint_split_with_min(SplitDirection::Vertical, 10.0, 10, 100);
        assert_eq!(first.height, 1.0);
    }

    /// The acceptance scenario for `"split"`: `padding: 12, margin: 4,
    /// gap: 8` on a stock 80×24 terminal must still leave both panes real
    /// room to paint into, and the second pane must never overflow past
    /// the original area's edge.
    #[test]
    fn split_spacing_does_not_clip_or_overflow_a_terminal_sized_area() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let spacing = Spacing {
            padding: EdgeInsets::uniform(12.0),
            margin: 4.0,
            gap: 8.0,
        };
        let inset = spacing.inset(area, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        let gap = quantise_horizontal(spacing.gap, TUI_CHAR_WIDTH);
        let draw_area = shrink_main_axis(inset, true, gap);

        let split = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let mut layout = split.layout(draw_area, SplitMeasure::new(1.0));
        apply_split_gap(&mut layout, true, gap);

        assert!(layout.first_bounds.width > 0.0, "first pane must not clip");
        assert!(
            layout.second_bounds.width > 0.0,
            "second pane must not clip"
        );
        assert!(
            layout.second_bounds.x + layout.second_bounds.width <= inset.x + inset.width + 0.001,
            "second pane must not overflow the inset area: {:?}",
            layout.second_bounds
        );
    }

    /// A `"tabs"` node's `margin`/`padding` inset the whole node's area —
    /// strip and content both — before the strip band is reserved,
    /// exactly like a `"row"`/`"column"` node's own [`Spacing::inset`]
    /// call. `padding: 12` on an 80×24 terminal must leave real room for
    /// both the strip and the content beneath it.
    #[test]
    fn tabs_spacing_insets_the_whole_area_and_does_not_clip_on_tui() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let spacing = Spacing {
            padding: EdgeInsets::uniform(12.0),
            margin: 0.0,
            gap: 0.0,
        };
        let inset = spacing.inset(area, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        assert_eq!(inset, Rect::new(2.0, 1.0, 80.0 - 4.0, 24.0 - 2.0));

        // Mirrors `paint_layout`'s `LayoutNode::Tabs` arm: a 3-line-high
        // strip reserved from the (already inset) area, clamped to its
        // height.
        let strip_height = (3.0 * TUI_LINE_HEIGHT).min(inset.height);
        let content_height = (inset.height - strip_height).max(0.0);
        assert!(strip_height > 0.0, "tab strip must not clip to zero height");
        assert!(
            content_height > 0.0,
            "tab content must not clip to zero height"
        );
        assert_eq!(strip_height + content_height, inset.height);
    }

    /// The same inset on a pixel backend passes the padding/margin values
    /// straight through, same as every other spacing-aware node kind.
    #[test]
    fn tabs_spacing_insets_the_whole_area_on_a_pixel_backend() {
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let spacing = Spacing {
            padding: EdgeInsets::uniform(5.0),
            margin: 3.0,
            gap: 0.0,
        };
        let inset = spacing.inset(area, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        assert_eq!(inset, Rect::new(8.0, 8.0, 200.0 - 16.0, 100.0 - 16.0));
    }

    /// A `"panel"` node's `margin`/`padding` inset the whole panel area —
    /// chrome and content both — before `Backend::draw_panel` paints it,
    /// the same [`Spacing::inset`] call every other spacing-aware node
    /// kind makes. `padding: 12` on an 80×24 terminal must leave real room
    /// to paint the panel into.
    #[test]
    fn panel_spacing_insets_the_whole_area_and_does_not_clip_on_tui() {
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let spacing = Spacing {
            padding: EdgeInsets::uniform(12.0),
            margin: 0.0,
            gap: 0.0,
        };
        let inset = spacing.inset(area, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
        assert_eq!(inset, Rect::new(2.0, 1.0, 80.0 - 4.0, 24.0 - 2.0));
        assert!(inset.width > 0.0, "panel must not clip to zero width");
        assert!(inset.height > 0.0, "panel must not clip to zero height");
    }

    /// The same inset on a pixel backend.
    #[test]
    fn panel_spacing_insets_the_whole_area_on_a_pixel_backend() {
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let spacing = Spacing {
            padding: EdgeInsets::uniform(5.0),
            margin: 3.0,
            gap: 0.0,
        };
        let inset = spacing.inset(area, PIXEL_CHAR_WIDTH, PIXEL_LINE_HEIGHT);
        assert_eq!(inset, Rect::new(8.0, 8.0, 200.0 - 16.0, 100.0 - 16.0));
    }

    // ── `layout_container_rects`: per-child size/min/max/justify/align ──
    //
    // Headless render tests driven straight through `quadraui::tui::
    // TuiBackend` -- the real `char_width`/`line_height`/`measure_text`
    // path every TUI render callback paints through, exercised at two
    // terminal sizes per mode so a mode's own math isn't accidentally
    // pinned to one viewport.

    use crate::layout::{Align, Justify, LayoutChild};
    use quadraui::tui::TuiBackend;

    fn tui() -> TuiBackend {
        TuiBackend::new()
    }

    fn leaf_child(id: &str, size: SectionSize, min: Option<f32>, max: Option<f32>) -> LayoutChild {
        LayoutChild {
            node: LayoutNode::Leaf(LayoutLeaf {
                id: id.to_string(),
                descriptor: LeafDescriptor::Button(Button::new(WidgetId::new(id), id)),
                scroll_px: None,
            }),
            size,
            min,
            max,
        }
    }

    fn row(children: Vec<LayoutChild>, justify: Justify, align: Align) -> LayoutContainer {
        LayoutContainer {
            children,
            spacing: Spacing::default(),
            justify,
            align,
        }
    }

    #[test]
    fn fixed_plus_weight_mix_at_two_terminal_sizes() {
        for width in [80.0_f32, 40.0] {
            let area = Rect::new(0.0, 0.0, width, 10.0);
            let c = row(
                vec![
                    // `SectionSize::Fixed` is decoded in the same portable
                    // surface-native-pixel unit as `padding`/`margin`/
                    // `gap`; `80` quantises to exactly `10` cells on
                    // `TuiBackend` (`ceil(80 / 8) = 10`).
                    leaf_child("fixed", SectionSize::Fixed(80), None, None),
                    leaf_child("w2", SectionSize::Weight(2.0), None, None),
                    leaf_child("w1", SectionSize::Weight(1.0), None, None),
                ],
                Justify::Start,
                Align::Stretch,
            );
            let rects = layout_container_rects(&mut tui(), area, &c, true);
            assert_eq!(rects[0].width, 10.0);
            let remainder = width - 10.0;
            assert!((rects[1].width - remainder * 2.0 / 3.0).abs() < 0.01);
            assert!((rects[2].width - remainder * 1.0 / 3.0).abs() < 0.01);
            // Every child still fills the full row height -- `Align::Stretch`.
            for r in &rects {
                assert_eq!(r.height, 10.0);
            }
            // The three children tile the row with no gap/overlap.
            assert_eq!(rects[0].x, 0.0);
            assert_eq!(rects[1].x, rects[0].width);
            assert_eq!(rects[2].x, rects[0].width + rects[1].width);
        }
    }

    #[test]
    fn percent_at_two_terminal_sizes() {
        for width in [80.0_f32, 40.0] {
            let area = Rect::new(0.0, 0.0, width, 10.0);
            let c = row(
                vec![
                    leaf_child("quarter", SectionSize::Percent(0.25), None, None),
                    leaf_child("rest", SectionSize::EqualShare, None, None),
                ],
                Justify::Start,
                Align::Stretch,
            );
            let rects = layout_container_rects(&mut tui(), area, &c, true);
            assert_eq!(rects[0].width, width * 0.25);
            assert_eq!(rects[1].width, width * 0.75);
        }
    }

    #[test]
    fn content_sized_leaf_uses_its_natural_main_axis_extent() {
        for width in [80.0_f32, 40.0] {
            let area = Rect::new(0.0, 0.0, width, 10.0);
            let c = row(
                vec![
                    // A `Button`'s natural width is its label's measured
                    // width (`measure_text` reports a label's display
                    // width directly on `TuiBackend`, so a 4-character
                    // label measures 4.0 cells) plus the `"[ " + " ]"`
                    // bracket chrome TUI paints around it (4.0 more).
                    leaf_child("abcd", SectionSize::Content, None, None),
                    leaf_child("rest", SectionSize::EqualShare, None, None),
                ],
                Justify::Start,
                Align::Stretch,
            );
            let rects = layout_container_rects(&mut tui(), area, &c, true);
            assert_eq!(rects[0].width, 8.0);
            assert_eq!(rects[1].width, width - 8.0);
        }
    }

    #[test]
    fn min_max_clamp_at_two_terminal_sizes() {
        for width in [80.0_f32, 40.0] {
            let area = Rect::new(0.0, 0.0, width, 10.0);
            let c = row(
                vec![
                    // A tiny `Fixed` floored by `min` (both in the same
                    // portable px unit: `120` quantises to `15` cells,
                    // `ceil(120 / 8) = 15`).
                    leaf_child("floored", SectionSize::Fixed(1), Some(120.0), None),
                    // The only other (flex) child, capped by `max` (`40`
                    // quantises to `5` cells) -- same single-pass clamp
                    // `quadraui::MultiSectionView`'s own per-section
                    // layout uses, so the space a `max` clamps away isn't
                    // redistributed to a sibling; it goes unused, same as
                    // a `Weight`/`EqualShare` child with no min/max at all
                    // when nothing else absorbs the remainder.
                    leaf_child("capped", SectionSize::EqualShare, None, Some(40.0)),
                ],
                Justify::Start,
                Align::Stretch,
            );
            let rects = layout_container_rects(&mut tui(), area, &c, true);
            assert_eq!(rects[0].width, 15.0);
            assert_eq!(rects[1].width, 5.0);
        }
    }

    #[test]
    fn justify_every_value_at_two_terminal_sizes() {
        for width in [80.0_f32, 40.0] {
            let area = Rect::new(0.0, 0.0, width, 10.0);
            // `80` quantises to exactly `10` cells on `TuiBackend`.
            let fixed = || {
                vec![
                    leaf_child("a", SectionSize::Fixed(80), None, None),
                    leaf_child("b", SectionSize::Fixed(80), None, None),
                ]
            };
            let leftover = width - 20.0;

            let start = row(fixed(), Justify::Start, Align::Stretch);
            let rects = layout_container_rects(&mut tui(), area, &start, true);
            assert_eq!(rects[0].x, 0.0);
            assert_eq!(rects[1].x, 10.0);

            let center = row(fixed(), Justify::Center, Align::Stretch);
            let rects = layout_container_rects(&mut tui(), area, &center, true);
            assert_eq!(rects[0].x, leftover / 2.0);
            assert_eq!(rects[1].x, leftover / 2.0 + 10.0);

            let end = row(fixed(), Justify::End, Align::Stretch);
            let rects = layout_container_rects(&mut tui(), area, &end, true);
            assert_eq!(rects[0].x, leftover);
            assert_eq!(rects[1].x, leftover + 10.0);

            let space_between = row(fixed(), Justify::SpaceBetween, Align::Stretch);
            let rects = layout_container_rects(&mut tui(), area, &space_between, true);
            assert_eq!(rects[0].x, 0.0);
            assert_eq!(rects[1].x, width - 10.0);
        }
    }

    /// `justify` must dispose of the leftover main-axis space against the
    /// container's *full* extent, not an already gap-reduced one — a
    /// non-zero `gap` is exactly the case that caught a regression where
    /// the gap total got subtracted twice (once by `layout_container_rects`
    /// feeding an already-reduced extent to `justify_starts`, once more by
    /// `justify_starts` itself). Both `End` and `SpaceBetween` must still
    /// land the last child flush with the container's far edge.
    #[test]
    fn justify_with_a_nonzero_gap_still_reaches_the_far_edge() {
        let width = 40.0_f32;
        let area = Rect::new(0.0, 0.0, width, 10.0);
        // `80` quantises to `10` cells; `"gap": 8` quantises to `1` cell.
        let fixed_with_gap = || LayoutContainer {
            children: vec![
                leaf_child("a", SectionSize::Fixed(80), None, None),
                leaf_child("b", SectionSize::Fixed(80), None, None),
            ],
            spacing: Spacing {
                gap: 8.0,
                ..Spacing::default()
            },
            justify: Justify::End,
            align: Align::Stretch,
        };

        let end = fixed_with_gap();
        let rects = layout_container_rects(&mut tui(), area, &end, true);
        assert_eq!(rects[1].x + rects[1].width, width);

        let space_between = LayoutContainer {
            justify: Justify::SpaceBetween,
            ..fixed_with_gap()
        };
        let rects = layout_container_rects(&mut tui(), area, &space_between, true);
        assert_eq!(rects[0].x, 0.0);
        assert_eq!(rects[1].x + rects[1].width, width);
    }

    #[test]
    fn align_every_value_at_two_terminal_sizes() {
        for height in [24.0_f32, 10.0] {
            let area = Rect::new(0.0, 0.0, 80.0, height);
            // `Button::new`'s label "x" measures to a natural height of
            // `1.0` on `TuiBackend` (the bracket chrome only widens it,
            // never heightens it) -- its natural cross-axis (height)
            // extent for a `Row` is `1.0`.
            let one = || {
                vec![LayoutChild::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "x".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(WidgetId::new("x"), "x")),
                    scroll_px: None,
                }))]
            };

            let stretch = row(one(), Justify::Start, Align::Stretch);
            let rects = layout_container_rects(&mut tui(), area, &stretch, true);
            assert_eq!(rects[0].y, 0.0);
            assert_eq!(rects[0].height, height);

            let start = row(one(), Justify::Start, Align::Start);
            let rects = layout_container_rects(&mut tui(), area, &start, true);
            assert_eq!(rects[0].y, 0.0);
            assert_eq!(rects[0].height, 1.0);

            let center = row(one(), Justify::Start, Align::Center);
            let rects = layout_container_rects(&mut tui(), area, &center, true);
            assert_eq!(rects[0].y, (height - 1.0) / 2.0);
            assert_eq!(rects[0].height, 1.0);

            let end = row(one(), Justify::Start, Align::End);
            let rects = layout_container_rects(&mut tui(), area, &end, true);
            assert_eq!(rects[0].y, height - 1.0);
            assert_eq!(rects[0].height, 1.0);
        }
    }

    /// A pre-per-child-sizing tree (every child defaults to `EqualShare`,
    /// no min/max, `Justify::Start`/`Align::Stretch`) divides the row the
    /// same way [`split_rects_with_gap`]'s plain equal-share split always
    /// did — the backward-compatibility contract every default above
    /// rests on. Compared with a small tolerance rather than exact
    /// equality: the two now reach the same share width by different
    /// floating-point paths (a weight-ratio division here, a direct
    /// `(width - gap) / n` there), which can differ by a ULP or two
    /// without being visually or behaviourally different.
    #[test]
    fn default_container_matches_the_pre_per_child_sizing_equal_split() {
        for width in [80.0_f32, 40.0] {
            let area = Rect::new(0.0, 0.0, width, 10.0);
            let c = LayoutContainer::new(
                vec![
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "a".to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(WidgetId::new("a"), "a")),
                        scroll_px: None,
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "b".to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(WidgetId::new("b"), "b")),
                        scroll_px: None,
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "c".to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(WidgetId::new("c"), "c")),
                        scroll_px: None,
                    }),
                ],
                Spacing::default(),
            );
            let got = layout_container_rects(&mut tui(), area, &c, true);
            let want = split_rects_with_gap(area, 3, true, 0.0, TUI_CHAR_WIDTH, TUI_LINE_HEIGHT);
            assert_eq!(got.len(), want.len());
            for (g, w) in got.iter().zip(want.iter()) {
                assert!((g.x - w.x).abs() < 0.001);
                assert!((g.y - w.y).abs() < 0.001);
                assert!((g.width - w.width).abs() < 0.001);
                assert!((g.height - w.height).abs() < 0.001);
            }
        }
    }
}
