//! Render-to-frame assembly: paints either the six-fixed-slot
//! [`RenderFrame`]/[`RenderRefs`] shape ([`draw_bands`]) or a decoded
//! [`crate::layout::LayoutNode`] tree ([`paint_layout`]) into a
//! [`quadraui::Backend`], recording a [`PaintedHit`] per primitive painted
//! so a later pointer event can be hit-tested against this frame.

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

use serde::{Deserialize, Deserializer};

use quadraui::{
    Backend, Button, DataRow, DataTable, Decoration, FontRole, InteractionState, ListItem,
    ListView, ListViewHit, ListViewLayout, Rect, ScrollAxis, ScrollView, Scrollbar, SectionSize,
    Split, SplitDirection, SplitHit, SplitLayout, Style, StyledText, TabBar, TabChrome,
    TextDisplay, TextDisplayHit, TextDisplayLayout, TextDisplayLine, TextRegion, Theme, TreePath,
    TreeRow, TreeTable, TreeView, TreeViewHit, TreeViewLayout, WidgetId,
};

use crate::extra_paint::{extra_as, ExtraPaint};
use crate::hit::{LoadedRows, PaintedHit, ScrollChildHit, TreeTableHitState};
use crate::layout::{
    align_cross, apply_min_max, grid_cell_rects, justify_starts, quantise_horizontal,
    quantise_vertical, resolve_main_axis_sizes, Align, LayoutChild, LayoutContainer, LayoutLeaf,
    LayoutNode, LayoutScroll, LayoutStyled, LeafDescriptor, MainAxisSpec,
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
///
/// Builds a fresh warn-mode [`crate::wire::DecodeDiagnostics`] per call
/// (unlike [`crate::wire::decode_render_output_with_styles`], which takes a
/// session-scoped one), so an unknown optional key found through this impl
/// can never warn-once across calls or honour a binding's `strict` setting.
/// No binding calls this `Deserialize` impl directly today — each decodes
/// through `decode_render_output_with_styles` instead — so the gap is
/// latent, not currently user-visible.
impl<'de> Deserialize<'de> for RenderFrame {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let diagnostics = crate::wire::DecodeDiagnostics::warn();
        crate::wire::render_frame_from_value("render", &value, &diagnostics)
            .map_err(serde::de::Error::custom)
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
                clip: rect,
                layout,
                selected_idx: list.selected_idx,
                total_items: list.items.len(),
                scroll_offset: list.scroll_offset,
                viewport_rows,
                // The flat six-slot path carries no `ItemsWindow`: the
                // only decode that records one is a layout-tree leaf's
                // (`crate::wire::decode_leaf`), so every slot painted here
                // holds its whole collection.
                loaded_rows: None,
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
                clip: content_rect,
                layout,
                row_paths: tree.rows.iter().map(|row| row.path.clone()).collect(),
                scrollbar,
                scroll_offset: tree.scroll_offset,
                total_rows: tree.rows.len(),
                viewport_rows,
                editing: tree
                    .rows
                    .iter()
                    .find_map(|row| row.edit.clone().map(|edit| (row.path.clone(), edit))),
                // See the `List` slot above.
                loaded_rows: None,
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
                clip: rect,
                layout,
                scroll_offset: data_table.scroll_offset,
                total_rows: data_table.rows.len(),
                selected_idx: data_table.selected_idx,
                viewport_rows,
                // See the `List` slot above.
                loaded_rows: None,
            });
        }
    }
    if let Some(tab_bar) = refs.tab_bar {
        if let Some(rect) = band(3.0) {
            let layout = backend.draw_tab_bar_layout(rect, tab_bar, None);
            hits.push(PaintedHit::TabBar {
                id: tab_bar.id,
                area: rect,
                clip: rect,
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
                let (chrome_hits, tree_table_hit) = paint_extra(backend, extra, rect, &interaction);
                hits.push(PaintedHit::Extra {
                    id: extra.id(),
                    area: rect,
                    extra: Rc::clone(extra),
                    chrome_hits,
                    tree_table_hit,
                });
            }
        }
    }
}

/// Paints one additive primitive and returns the `(chrome_hits,
/// tree_table_hit)` pair its [`PaintedHit::Extra`] entry carries. A
/// [`TreeTable`] paints through [`Backend::draw_tree_table`] directly
/// (not the generic [`ExtraPaint::paint_chrome`], whose `Vec<(Rect,
/// WidgetId)>` return shape has no room for the resolved layout) so its
/// chevron can be hit-tested precisely later without a live `Backend` —
/// see [`PaintedHit::Extra::tree_table_hit`]'s own doc.
fn paint_extra(
    backend: &mut dyn Backend,
    extra: &Rc<dyn ExtraPaint>,
    area: Rect,
    interaction: &InteractionState,
) -> (Vec<(Rect, WidgetId)>, Option<TreeTableHitState>) {
    if let Some(tree_table) = extra_as::<TreeTable>(extra) {
        let layout = backend.draw_tree_table(area, tree_table, None);
        let scroll_offset = tree_table.table.scroll_offset;
        let total_rows = tree_table.table.rows.len();
        (Vec::new(), Some((layout, scroll_offset, total_rows)))
    } else {
        (extra.paint_chrome(backend, area, interaction), None)
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
        clip: area,
        layout,
        body_offset,
        total_lines: td.lines.len(),
        viewport_lines,
        // Overwritten by `paint_leaf_windowed` for a windowed leaf; `None`
        // is right for every other caller (`draw_bands`, an unwindowed
        // layout-tree leaf).
        loaded_rows: None,
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
/// `Split` whose panes are each a `Tabs`. A `SidebarPanel` paints its
/// chrome (toolbar) via `Backend::draw_sidebar_panel_interactive` and
/// recurses into `content_bounds` with its one content subtree; a
/// `SplitTree` paints its dividers via `Backend::draw_split_tree` and
/// recurses into each resolved leaf rect. All three record a [`PaintedHit`]
/// for their own chrome (divider / toolbar button), same as `Split`,
/// so a later pointer event can resolve against it. A `Grid` resolves its
/// `columns`/`rows` tracks into cell rects via
/// [`crate::layout::grid_cell_rects`] and recurses into each with the
/// matching row-major child; it paints no chrome of its own, so it records
/// no hit. Per-child flex weights on `Row`/`Column` are a follow-up; every
/// `Row`/`Column` child gets the same share for now.
pub fn paint_layout(
    backend: &mut dyn Backend,
    area: Rect,
    node: &LayoutNode,
    hits: &mut Vec<PaintedHit>,
    chrome_interaction: &dyn Fn(WidgetId) -> InteractionState,
) {
    paint_layout_windowed(
        backend,
        area,
        node,
        hits,
        chrome_interaction,
        &HashMap::new(),
    );
}

/// [`paint_layout`], additionally honoring each windowed leaf's `{start,
/// total}` from `windows` — an id-keyed map built from
/// [`crate::wire::DecodeDiagnostics::items_windows`] against the same
/// decode that produced `node`. Honoring it means [`paint_leaf_windowed`]
/// paints that leaf from the band of its collection the viewport actually
/// shows (see [`WindowedBand`]), reports the collection's real extent on
/// the leaf's own [`PaintedHit`] rather than the painted band's length, and
/// leaves an empty placeholder at every band position the app hasn't
/// supplied a row for yet. [`paint_layout`] itself is this function with an
/// empty `windows` map, so a caller that never decodes an `"items_window"`
/// key paints exactly as before.
pub fn paint_layout_windowed(
    backend: &mut dyn Backend,
    area: Rect,
    node: &LayoutNode,
    hits: &mut Vec<PaintedHit>,
    chrome_interaction: &dyn Fn(WidgetId) -> InteractionState,
    windows: &HashMap<String, ItemsWindow>,
) {
    match node {
        LayoutNode::Row(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = inset_for_spacing(area, container, char_width, line_height);
            let rects = layout_container_rects(backend, area, container, true);
            for (child, rect) in container.children.iter().zip(rects) {
                paint_layout_windowed(
                    backend,
                    rect,
                    &child.node,
                    hits,
                    chrome_interaction,
                    windows,
                );
            }
        }
        LayoutNode::Column(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = inset_for_spacing(area, container, char_width, line_height);
            let rects = layout_container_rects(backend, area, container, false);
            for (child, rect) in container.children.iter().zip(rects) {
                paint_layout_windowed(
                    backend,
                    rect,
                    &child.node,
                    hits,
                    chrome_interaction,
                    windows,
                );
            }
        }
        LayoutNode::Grid(grid) => {
            let rects = grid_cell_rects(grid, area, backend.char_width(), backend.line_height());
            for (child, rect) in grid.children.iter().zip(rects) {
                paint_layout_windowed(backend, rect, child, hits, chrome_interaction, windows);
            }
        }
        LayoutNode::Leaf(leaf) => {
            paint_leaf_windowed(backend, area, leaf, hits, chrome_interaction, windows)
        }
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
            paint_layout_windowed(
                backend,
                layout.first_bounds,
                &split.first,
                hits,
                chrome_interaction,
                windows,
            );
            paint_layout_windowed(
                backend,
                layout.second_bounds,
                &split.second,
                hits,
                chrome_interaction,
                windows,
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
                clip: strip_rect,
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
                paint_layout_windowed(
                    backend,
                    content_rect,
                    &selected.content,
                    hits,
                    chrome_interaction,
                    windows,
                );
            }
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
            paint_layout_windowed(
                backend,
                layout.content_bounds,
                &sidebar_node.content,
                hits,
                chrome_interaction,
                windows,
            );
        }
        LayoutNode::SplitTree(split_tree) => {
            let layout = backend.draw_split_tree(area, &split_tree.tree);
            hits.push(PaintedHit::SplitTree {
                id: WidgetId::new(split_tree.id.as_str()),
                layout: layout.clone(),
            });
            for ((_, rect), content) in layout.leaves.iter().zip(split_tree.leaves.iter()) {
                paint_layout_windowed(backend, *rect, content, hits, chrome_interaction, windows);
            }
        }
        LayoutNode::Scroll(scroll_node) => {
            paint_scroll(
                backend,
                area,
                scroll_node,
                hits,
                chrome_interaction,
                windows,
            );
        }
        LayoutNode::Styled(styled) => {
            paint_styled(backend, area, styled, hits, chrome_interaction, windows);
        }
    }
}

/// Paints a [`LayoutNode::Styled`] wrapper: pushes the derived `Theme`/
/// `Style` via [`push_styled_override`] for the wrapped subtree's own
/// paint, then restores the parent's values via [`pop_styled_override`]
/// once that subtree has finished painting — the same "vary the theme
/// per-pane" bracketing `quadraui::Backend::set_theme`'s own doc
/// describes, just driven once per `Styled` node instead of once per
/// host-level render pass. The restore runs even though nothing here can
/// panic partway through today, matching every other `paint_*` function's
/// own "always finish painting the whole subtree" contract.
fn paint_styled(
    backend: &mut dyn Backend,
    area: Rect,
    styled: &LayoutStyled,
    hits: &mut Vec<PaintedHit>,
    chrome_interaction: &dyn Fn(WidgetId) -> InteractionState,
    windows: &HashMap<String, ItemsWindow>,
) {
    let push = push_styled_override(backend, &styled.style);
    paint_layout_windowed(
        backend,
        area,
        &styled.node,
        hits,
        chrome_interaction,
        windows,
    );
    pop_styled_override(backend, push);
}

/// What [`push_styled_override`] actually changed on `backend`, so
/// [`pop_styled_override`] restores only that -- never a blind
/// `set_theme`/`set_style` back to the parent's values, which would cost a
/// backend like GTK's (`Backend::set_theme` there also reloads a CSS
/// provider string and flips `gtk-application-prefer-dark-theme`) a second
/// unnecessary reload even when this override only ever touched the other
/// domain (a metric-only `"style"` key touches neither).
struct StyledPush {
    parent_theme: Option<Theme>,
    parent_style: Option<Style>,
}

/// Applies `style` onto `backend`'s current theme/style, merged via
/// [`crate::layout::StyleOverride::apply_theme`]/`apply_style`, skipping
/// the `Backend::set_theme`/`set_style` call entirely when the derived
/// value equals what's already active -- both `Theme` and `Style`
/// implement `PartialEq`, so an override that only names metric tokens (or
/// only colour tokens) never touches the other domain's backend call, and
/// an override whose named tokens already match the parent's own values
/// (e.g. a nested `Styled` re-stating a colour an ancestor already set)
/// touches neither. Returns what (if anything) was actually pushed,
/// for [`pop_styled_override`] to restore.
fn push_styled_override(
    backend: &mut dyn Backend,
    style: &crate::layout::StyleOverride,
) -> StyledPush {
    let current_theme = backend.theme();
    let derived_theme = style.apply_theme(current_theme);
    let parent_theme = if derived_theme != current_theme {
        backend.set_theme(derived_theme);
        Some(current_theme)
    } else {
        None
    };
    let current_style = backend.style();
    let derived_style = style.apply_style(current_style);
    let parent_style = if derived_style != current_style {
        backend.set_style(derived_style);
        Some(current_style)
    } else {
        None
    };
    StyledPush {
        parent_theme,
        parent_style,
    }
}

/// Restores whatever [`push_styled_override`] actually changed -- a no-op
/// `Backend::set_theme`/`set_style` call for whichever domain (or both)
/// that push skipped.
fn pop_styled_override(backend: &mut dyn Backend, push: StyledPush) {
    if let Some(theme) = push.parent_theme {
        backend.set_theme(theme);
    }
    if let Some(style) = push.parent_style {
        backend.set_style(style);
    }
}

/// [`push_styled_override`]'s multi-layer counterpart: walks through every
/// nested [`LayoutNode::Styled`] wrapper directly wrapping `content` (not
/// recursing into row/column children -- only the chain of `Styled` layers
/// sitting directly on top of a `Scroll`'s own content node), pushing each
/// one's own override in outer-to-inner order -- the same composition
/// order [`paint_layout`]'s own recursive `Styled` dispatch produces.
/// [`paint_scroll`] needs this because its `Children`/`Atomic` arms paint
/// straight off `content.unstyled()`'s own children/leaf rather than
/// recursing through `paint_layout` on `content` itself, which is the one
/// call site that would otherwise apply a wrapping `Styled` node's own
/// bracket. Pop with [`pop_styled_scope`] once that painting is done.
fn push_styled_scope(backend: &mut dyn Backend, content: &LayoutNode) -> Vec<StyledPush> {
    let mut pushes = Vec::new();
    let mut node = content;
    while let LayoutNode::Styled(styled) = node {
        pushes.push(push_styled_override(backend, &styled.style));
        node = &styled.node;
    }
    pushes
}

/// [`push_styled_scope`]'s restore: pops every pushed layer in
/// inner-to-outer order (the reverse of how they were pushed), so each
/// [`pop_styled_override`] call restores onto the state its own matching
/// push saw, not the final outer parent's.
fn pop_styled_scope(backend: &mut dyn Backend, pushes: Vec<StyledPush>) {
    for push in pushes.into_iter().rev() {
        pop_styled_override(backend, push);
    }
}

/// Paints a [`LayoutNode::Scroll`]: measures `scroll_node.content`'s
/// natural length along its own axis ([`natural_main_length`]), resolves
/// the scrollbar gutter + content viewport ([`scroll_viewport_and_track`]),
/// clips to that viewport via `Backend::push_clip`/`Backend::pop_clip`
/// (belt-and-suspenders against a partially-visible boundary slot's own
/// chrome overflowing by a sub-unit rounding — [`scroll_content_slots`]'s
/// own per-slot culling already keeps a fully-invisible slot from being
/// painted, hence from getting a [`PaintedHit`], at all), and paints the
/// shared scrollbar ([`quadraui::ScrollView::scrollbar`] +
/// `Backend::draw_scrollbar`, the same geometry
/// `crate::extra_paint`'s standalone `ScrollView` leaf paints into its own
/// fixed `line_height`-wide track). `scroll_node.offset` is read as-is —
/// already resolved against [`crate::widget_state::WidgetState`] by
/// [`crate::host::resolve_layout_node`], which must run on this tree before
/// this function does (same ordering [`crate::layout`]'s own `Split`/
/// `Tabs` resolution already requires).
fn paint_scroll(
    backend: &mut dyn Backend,
    area: Rect,
    scroll_node: &LayoutScroll,
    hits: &mut Vec<PaintedHit>,
    chrome_interaction: &dyn Fn(WidgetId) -> InteractionState,
    windows: &HashMap<String, ItemsWindow>,
) {
    let metrics = ScrollMetrics {
        char_width: backend.char_width(),
        line_height: backend.line_height(),
    };
    let inner = scroll_node
        .spacing
        .inset(area, metrics.char_width, metrics.line_height);
    let (viewport, track) = scroll_viewport_and_track(inner, scroll_node.axis, metrics.line_height);
    let content_len = natural_main_length(backend, &scroll_node.content, scroll_node.axis, metrics);
    let mut scroll_view = scroll_view_at(
        WidgetId::new(scroll_node.id.as_str()),
        scroll_node.axis,
        viewport,
        content_len,
        scroll_node.offset.unwrap_or(0.0),
    );
    // Clamped here (not just relied on from `crate::host::resolve_layout_node`,
    // which already clamps before writing `scroll_node.offset` back) so a
    // caller that paints straight off a raw, never-resolved offset (a bare
    // `paint_layout` call with no `resolve_layout_node` pass first, e.g. a
    // `render` that doesn't go through `crate::host::BindingHost` at all)
    // still gets a `scroll_content_slots` call whose own visibility checks
    // (which already clamp internally -- `ScrollView::is_visible`'s own
    // `clamped_offset()`) and whose rect positioning (which does not --
    // see that function's own doc) agree on the same offset.
    scroll_view.clamp_offset();
    let offset = scroll_view.scroll_offset;
    let scrollbar = scroll_view.scrollbar(track, metrics.line_height);

    let slots = scroll_content_slots(
        backend,
        viewport,
        scroll_node.axis,
        offset,
        &scroll_node.content,
        content_len,
        metrics,
    );
    // One `ScrollChildHit` per direct `Leaf` child, read straight off
    // `slots` before it's consumed below — see `PaintedHit::Scroll`'s own
    // `children` doc for why this has to come from the slot (its
    // pre-offset, pre-clamp `start`/`len`) rather than the child's own
    // painted rect.
    let children: Vec<ScrollChildHit> = match (&slots, scroll_node.content.unstyled()) {
        (
            ScrollSlots::Children(slots),
            LayoutNode::Row(container) | LayoutNode::Column(container),
        ) => container
            .children
            .iter()
            .zip(slots.iter())
            .filter_map(|(child, slot)| match child.node.unstyled() {
                LayoutNode::Leaf(leaf) => Some(ScrollChildHit {
                    id: WidgetId::new(leaf.id.as_str()),
                    start: slot.start,
                    len: slot.len,
                }),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };

    hits.push(PaintedHit::Scroll {
        id: scroll_view.id,
        viewport,
        axis: scroll_node.axis,
        content_len,
        offset,
        scrollbar: scrollbar.clone(),
        children,
    });

    backend.push_clip(viewport);
    // Everything pushed from here down is this scroll's own descendant
    // content, painted inside `viewport` -- `hits_before` lets
    // `crate::hit::clip_hits_to_viewport` narrow exactly that slice
    // afterwards, intersecting each entry's own bounds with `viewport`
    // (dropping it entirely when the intersection is empty) so a hit
    // whose geometry wasn't itself clipped above (every kind but the
    // `cell_grid` slot clipping `scroll_content_slots` just applied) can
    // never resolve to a point outside this scroll's own viewport, nor
    // mask a sibling painted earlier at that same screen position.
    let hits_before = hits.len();
    // `scroll_node.content` may itself be wrapped in one or more
    // `LayoutNode::Styled` layers (a `"style"` key on the `"scroll"`
    // node's own `"content"` dict) -- unlike every other node kind, the
    // arms below paint straight off `content.unstyled()`'s own children/
    // leaf rather than recursing through `paint_layout` on `content`
    // itself, so the wrapper's own bracket has to be applied explicitly
    // here via `push_styled_scope`/`pop_styled_scope` instead of riding
    // `paint_layout`'s usual `LayoutNode::Styled` dispatch.
    let styled_scope = push_styled_scope(backend, scroll_node.content.as_ref());
    let content = scroll_node.content.unstyled();
    match slots {
        ScrollSlots::Children(slots) => {
            if let LayoutNode::Row(container) | LayoutNode::Column(container) = content {
                for (child, slot) in container.children.iter().zip(slots) {
                    if slot.visible {
                        paint_layout_windowed(
                            backend,
                            slot.rect,
                            &child.node,
                            hits,
                            chrome_interaction,
                            windows,
                        );
                    }
                }
            }
        }
        ScrollSlots::Atomic(rect, visible) => {
            if visible {
                // `TextDisplay`/`List`/`Tree`/`DataTable` each render their
                // own `scroll_offset..` window into their data regardless
                // of where their rect sits on screen (see
                // `offset_windowed_leaf_scrolled_to_offset`'s own doc) --
                // shifting their rect by `-offset` the way every other
                // `Atomic` leaf kind is positioned below would just
                // repaint from row/line `0` wherever that shifted rect
                // landed, never the `offset`th one, on every backend.
                // Paint a shadow clone with `scroll_offset` advanced to
                // match instead, into `rect` (already the viewport itself
                // here -- see `scroll_content_slots`'s own
                // `is_offset_windowed_leaf` branch).
                match offset_windowed_leaf_scrolled_to_offset(content, offset, metrics.line_height)
                {
                    Some(shadow) => {
                        paint_layout_windowed(
                            backend,
                            rect,
                            &shadow,
                            hits,
                            chrome_interaction,
                            windows,
                        );
                    }
                    None => {
                        paint_layout_windowed(
                            backend,
                            rect,
                            content,
                            hits,
                            chrome_interaction,
                            windows,
                        );
                    }
                }
            }
        }
    }
    pop_styled_scope(backend, styled_scope);
    crate::hit::clip_hits_to_viewport(hits, hits_before, viewport);
    backend.pop_clip();

    if let Some(scrollbar) = &scrollbar {
        backend.draw_scrollbar(track, scrollbar);
    }
}

/// `content`, when it is a bare `TextDisplay`/`List`/`Tree`/`DataTable`
/// leaf — the same four kinds [`scroll_content_slots`]'s own
/// `is_offset_windowed_leaf` singles out — with a locally mutated clone of
/// its own descriptor whose `scroll_offset` is advanced to the whole row/
/// line `offset` native units lands on (`TextDisplay` also gets
/// `auto_scroll` disabled, so it can't override that). `None` for any
/// other leaf kind, or a non-leaf node (a `Row`/`Column` reaches
/// [`ScrollSlots::Children`] instead, never `Atomic`).
///
/// Each of these four kinds renders its own `scroll_offset`-relative
/// window into its data regardless of where its own rect sits on screen
/// (`quadraui::TextDisplay`/`ListView`/`TreeView`/`DataTable`'s own doc:
/// render `items`/`rows`/`lines[scroll_offset..]`) — unlike every other
/// `Atomic` leaf kind, whose rect `scroll_content_slots` positions by
/// shifting it `-offset` along the scroll axis, repainting a shadow of
/// this shape instead is what lets [`paint_scroll`]'s own
/// `ScrollSlots::Atomic` arm show the right row/line first. `leaf`'s own
/// `id`/`scroll_px` are threaded through unchanged, so the shadow node
/// keeps the same hit-map id and (for `List`/`Tree`/`DataTable`, whose
/// `with_scroll_px` paint states already read it) the same pixel-smooth
/// sub-row remainder a plain, unscrolled leaf would get. Matches via
/// [`LayoutNode::unstyled`], so a `Styled` leaf gets the same shadow
/// treatment as an unstyled one -- the returned shadow carries no
/// `Styled` wrapper of its own, since [`paint_scroll`]'s own
/// `push_styled_scope` call already has the matching override active on
/// `backend` by the time this shadow paints.
fn offset_windowed_leaf_scrolled_to_offset(
    content: &LayoutNode,
    offset: f32,
    line_height: f32,
) -> Option<LayoutNode> {
    let LayoutNode::Leaf(leaf) = content.unstyled() else {
        return None;
    };
    let row = (offset / line_height.max(1.0)).floor().max(0.0) as usize;
    let descriptor = match &leaf.descriptor {
        LeafDescriptor::TextDisplay(td) => {
            let mut shadow = td.clone();
            shadow.scroll_offset = row;
            shadow.auto_scroll = false;
            LeafDescriptor::TextDisplay(shadow)
        }
        LeafDescriptor::List(list) => {
            let mut shadow = list.clone();
            shadow.scroll_offset = row;
            LeafDescriptor::List(shadow)
        }
        LeafDescriptor::Tree(tree) => {
            let mut shadow = tree.clone();
            shadow.scroll_offset = row;
            LeafDescriptor::Tree(shadow)
        }
        LeafDescriptor::DataTable(table) => {
            let mut shadow = table.clone();
            shadow.scroll_offset = row;
            LeafDescriptor::DataTable(shadow)
        }
        LeafDescriptor::Button(_) | LeafDescriptor::TabBar(_) | LeafDescriptor::Extra(_) => {
            return None
        }
    };
    Some(LayoutNode::Leaf(LayoutLeaf {
        id: leaf.id.clone(),
        descriptor,
        scroll_px: leaf.scroll_px,
    }))
}

/// Builds a [`quadraui::ScrollView`] with every field this crate needs to
/// set — [`quadraui::ScrollView`] is `#[non_exhaustive]`, so an external
/// crate (this one) cannot use its own struct-literal syntax even listing
/// every current field; [`quadraui::ScrollView::vertical`]/
/// [`quadraui::ScrollView::horizontal`] are the only constructors, so
/// `scroll_offset` is set via its own `pub` field afterwards (mutating an
/// already-constructed instance's field is unaffected by
/// `#[non_exhaustive]`, unlike construction syntax). `pub(crate)` so
/// [`crate::drag::scroll_view_at`] can share this construction rather than
/// re-deriving it from a [`PaintedHit::Scroll`]'s own fields.
pub(crate) fn scroll_view_at(
    id: WidgetId,
    axis: ScrollAxis,
    viewport: Rect,
    content_len: f32,
    offset: f32,
) -> ScrollView {
    let mut scroll_view = match axis {
        ScrollAxis::Vertical => ScrollView::vertical(id, viewport, content_len),
        ScrollAxis::Horizontal => ScrollView::horizontal(id, viewport, content_len),
    };
    scroll_view.scroll_offset = offset;
    scroll_view
}

/// Splits a [`LayoutNode::Scroll`]'s own inset area into the content
/// viewport and the scrollbar gutter: a fixed `line_height`-wide strip
/// along the scrolling axis's trailing edge, same convention
/// `crate::extra_paint`'s standalone `ScrollView` leaf already paints its
/// own track into. Reserved unconditionally (not only when
/// [`quadraui::ScrollView::scrollbar`] actually returns `Some`) so the
/// viewport a frame paints into never changes shape between a frame with
/// nothing to scroll and one with something to — [`ScrollView::scrollbar`]
/// itself still returns `None` for the former, so nothing paints into the
/// reserved strip either way.
pub(crate) fn scroll_viewport_and_track(
    inner: Rect,
    axis: ScrollAxis,
    line_height: f32,
) -> (Rect, Rect) {
    match axis {
        ScrollAxis::Vertical => {
            let track_width = line_height.max(0.0).min(inner.width);
            let track = Rect::new(
                inner.x + inner.width - track_width,
                inner.y,
                track_width,
                inner.height,
            );
            let viewport = Rect::new(inner.x, inner.y, inner.width - track_width, inner.height);
            (viewport, track)
        }
        ScrollAxis::Horizontal => {
            let track_height = line_height.max(0.0).min(inner.height);
            let track = Rect::new(
                inner.x,
                inner.y + inner.height - track_height,
                inner.width,
                track_height,
            );
            let viewport = Rect::new(inner.x, inner.y, inner.width, inner.height - track_height);
            (viewport, track)
        }
    }
}

/// The two per-backend metrics every `Scroll`-content measurement/layout
/// function needs ([`Backend::char_width`]/[`Backend::line_height`]),
/// bundled into one `Copy` value so a function threading both through a
/// recursive call stays under clippy's `too_many_arguments` limit.
#[derive(Clone, Copy)]
pub(crate) struct ScrollMetrics {
    pub(crate) char_width: f32,
    pub(crate) line_height: f32,
}

/// Natural main-axis length of `node` along `axis`: for a `Leaf`, its own
/// per-kind intrinsic size via [`natural_size`] (falling back to one
/// `line_height`/`char_width`-scaled unit when that reports `None` — a
/// `List`/`Tree`/`DataTable`'s width, and whichever axis an `Extra`
/// leaf's own `ExtraPaint::natural_size` has no cheap measurement for);
/// for a `Row`/`Column` whose own main axis matches `axis` (a `Column` under a
/// vertical scroll, a `Row` under a horizontal one), the sum of every
/// child's own natural length along `axis` plus the quantised gap between
/// them — this container's `padding`/`margin`/`justify`/`align` are NOT
/// applied here, unlike [`layout_container_rects`]'s equal-share division: a
/// `Scroll`'s content stacks every child at its own natural size, not
/// flexed to fill a fixed viewport, which is the entire point of putting it
/// in a `Scroll`. A child declaring [`SectionSize::Fixed`] on that matching
/// main axis is the one `size`/`min`/`max` exception: see
/// [`matching_axis_child_length`]'s own doc for why its declared width
/// wins over [`natural_size`]'s content-based guess — every other
/// `SectionSize` is still a relative share with no intrinsic length of its
/// own, so it keeps falling through to [`natural_size`]. For a `Row`/
/// `Column` on the cross axis, the max over its children's own natural
/// length along `axis`. For anything else (`Split`/`Tabs`/
/// `SidebarPanel`/`SplitTree`, or a nested `Scroll`), the same one-unit
/// fallback a `Leaf` with no intrinsic size gets — deeper measurement of
/// those node kinds as `Scroll` content is unimplemented today. `node` is
/// matched via [`LayoutNode::unstyled`] first -- a `"style"` key is purely
/// cosmetic, so a `Styled(Row|Column)` content node measures exactly like
/// an unstyled one, rather than falling through to the one-unit fallback
/// every unmeasurable node kind gets.
pub(crate) fn natural_main_length(
    backend: &mut dyn Backend,
    node: &LayoutNode,
    axis: ScrollAxis,
    metrics: ScrollMetrics,
) -> f32 {
    let horizontal = matches!(axis, ScrollAxis::Horizontal);
    let fallback = if horizontal {
        metrics.char_width.max(1.0) * 4.0
    } else {
        metrics.line_height.max(1.0)
    };
    match node.unstyled() {
        LayoutNode::Row(container) => {
            container_natural_main_length(backend, container, true, axis, metrics)
        }
        LayoutNode::Column(container) => {
            container_natural_main_length(backend, container, false, axis, metrics)
        }
        _ => natural_size(backend, node, metrics.line_height)
            .main(horizontal)
            .unwrap_or(fallback),
    }
}

/// [`natural_main_length`]'s `Row`/`Column` case: `container_is_row` is
/// `true` for a `Row` (own main axis = width), `false` for a `Column` (own
/// main axis = height) — compared against `axis` to decide "stack and sum"
/// (own main axis matches `axis`) vs. "cross-stack and take the max" (it
/// doesn't).
///
/// On the "stack and sum" branch, each child's length comes from
/// [`matching_axis_child_length`] rather than a bare recursive
/// [`natural_main_length`] call — see that function's own doc for why a
/// [`SectionSize::Fixed`] child needs the detour.
fn container_natural_main_length(
    backend: &mut dyn Backend,
    container: &LayoutContainer,
    container_is_row: bool,
    axis: ScrollAxis,
    metrics: ScrollMetrics,
) -> f32 {
    if container.children.is_empty() {
        return 0.0;
    }
    let horizontal = matches!(axis, ScrollAxis::Horizontal);
    let on_main_axis = container_is_row == horizontal;
    let lengths: Vec<f32> = container
        .children
        .iter()
        .map(|child| {
            if on_main_axis {
                matching_axis_child_length(backend, child, axis, metrics)
            } else {
                natural_main_length(backend, &child.node, axis, metrics)
            }
        })
        .collect();
    if on_main_axis {
        let main_metric = if horizontal {
            metrics.char_width
        } else {
            metrics.line_height
        };
        let quantise_main: fn(f32, f32) -> f32 = if horizontal {
            quantise_horizontal
        } else {
            quantise_vertical
        };
        let gap = quantise_main(container.spacing.gap, main_metric);
        lengths.iter().sum::<f32>() + gap * (lengths.len() - 1) as f32
    } else {
        lengths.into_iter().fold(0.0_f32, f32::max)
    }
}

/// A `Row`/`Column` child's own main-axis length along `axis`, when that
/// container's main axis matches `axis` (the "stack and sum" case both
/// [`container_natural_main_length`] and [`scroll_content_slots`] walk — a
/// `Row` directly or nested under a horizontal `Scroll`, a `Column` under a
/// vertical one). A child declaring [`SectionSize::Fixed`] uses that
/// declared size directly — quantised into native units and clamped by its
/// own `min`/`max` via [`apply_min_max`], the same treatment
/// [`layout_container_rects`] gives it when actually painting — instead of
/// [`natural_main_length`]'s content-based measurement. A `List`/
/// `DataTable`/`Tree` leaf's [`natural_size`] never reports a width (only a
/// content-driven height), so without this detour, a `Fixed`-width `list`/
/// `data_table` child inside a `scroll`+`row` fell back to
/// [`natural_main_length`]'s one-unit placeholder on the horizontal axis —
/// the `Row`'s total declared width collapsed to a handful of cells
/// regardless of how many `Fixed` columns it held, so the `Scroll`'s own
/// `content_len` never exceeded the viewport and `offset` had nothing to
/// scroll, and (via [`scroll_content_slots`]) each child's own painted rect
/// shrank the same way even when `content_len` happened to be big enough.
/// Every other `SectionSize` (`Percent`/`Weight`/`EqualShare`/`Content`) is
/// a relative share, not an intrinsic length, so it keeps falling through
/// to [`natural_main_length`] unchanged.
fn matching_axis_child_length(
    backend: &mut dyn Backend,
    child: &LayoutChild,
    axis: ScrollAxis,
    metrics: ScrollMetrics,
) -> f32 {
    let horizontal = matches!(axis, ScrollAxis::Horizontal);
    if let SectionSize::Fixed(raw) = child.size {
        let main_metric = if horizontal {
            metrics.char_width
        } else {
            metrics.line_height
        };
        let quantise_main: fn(f32, f32) -> f32 = if horizontal {
            quantise_horizontal
        } else {
            quantise_vertical
        };
        let spec = MainAxisSpec {
            size: child.size,
            min: child.min.map(|v| quantise_main(v, main_metric)),
            max: child.max.map(|v| quantise_main(v, main_metric)),
            content: 0.0,
        };
        return apply_min_max(&spec, quantise_main(raw as f32, main_metric));
    }
    natural_main_length(backend, &child.node, axis, metrics)
}

/// One [`LayoutNode::Scroll`]'s content, resolved into paintable/
/// hit-testable slots by [`scroll_content_slots`] — see that function's
/// own doc.
pub(crate) enum ScrollSlots {
    /// `content` is a `Column` (vertical axis) or `Row` (horizontal axis)
    /// matching the scroll's own axis: one [`ScrollChildSlot`] per
    /// top-level child, in the same order as [`LayoutContainer::children`].
    Children(Vec<ScrollChildSlot>),
    /// Anything else: the whole subtree is one atomic unit sized to its
    /// own overall natural length (`content_len`) — same visibility
    /// semantics as [`Self::Children`]'s own [`ScrollChildSlot::visible`].
    Atomic(Rect, bool),
}

/// One [`ScrollSlots::Children`] slot: a top-level child's own painted
/// `rect`, sized to its own natural main-axis length and positioned by the
/// running sum of every earlier sibling's — never
/// [`layout_container_rects`]'s equal-share division, which would defeat
/// natural-size scrolling (see [`natural_main_length`]'s own doc).
/// `visible` is whether [`quadraui::ScrollView::is_visible`] says this
/// slot overlaps the current scroll position — `false` means skip
/// painting/recursing/resolving it entirely, which is this container's
/// whole mechanism for "no cell or pixel outside the viewport is painted"
/// and "anything scrolled out of view gets no hits": an invisible slot
/// never gets a [`PaintedHit`] (or a resolved `WidgetState` entry) in the
/// first place, rather than one that exists and is masked out afterwards.
/// `start`/`len` are this slot's own main-axis span within the scroll's
/// content, BEFORE `offset` is subtracted and before a cell-grid backend's
/// [`clip_main_axis`] may have clamped `rect` to the viewport — the
/// un-clamped span [`PaintedHit::Scroll::children`] needs to scroll a
/// child that just received focus fully into view, which `rect` alone
/// can't recover once it's been clamped.
pub(crate) struct ScrollChildSlot {
    pub(crate) rect: Rect,
    pub(crate) visible: bool,
    pub(crate) start: f32,
    pub(crate) len: f32,
}

/// Resolves a [`LayoutNode::Scroll`]'s content into paintable/
/// hit-testable [`ScrollSlots`], at `offset` inside `viewport` — the
/// shared geometry [`paint_scroll`] and
/// [`crate::host::resolve_layout_node`]'s own `LayoutNode::Scroll` arm both
/// walk, so a click always resolves against the exact rects just painted
/// (mirroring [`layout_container_rects`]'s own shared-geometry role for a
/// plain `Row`/`Column`). `content_len` is the same value
/// [`natural_main_length`] already computed for `content` — passed in
/// rather than recomputed, since the caller needs it before this call to
/// build the `ScrollView` the offset/visibility checks run against.
pub(crate) fn scroll_content_slots(
    backend: &mut dyn Backend,
    viewport: Rect,
    axis: ScrollAxis,
    offset: f32,
    content: &LayoutNode,
    content_len: f32,
    metrics: ScrollMetrics,
) -> ScrollSlots {
    let horizontal = matches!(axis, ScrollAxis::Horizontal);
    // The same cell-grid-backend test `natural_size`'s `Button` arm and
    // `crate::layout::native_units` already use. A cell-grid `draw_*`
    // (every TUI one, via `q_rect_to_ratatui`) clamps a negative `x`/`y`
    // to `0.0` without shrinking the matching `width`/`height`, so a slot
    // positioned with a negative main-axis origin repaints its own top
    // content at the viewport's edge, full length, overlapping whatever
    // comes after it, instead of being clipped away — [`clip_main_axis`]
    // is this branch's fix. A pixel backend keeps the unclipped rect: it
    // already paints a negative origin correctly and clips it via
    // `Backend::push_clip`, so clipping here too would just double the
    // same result through a second code path.
    let cell_grid = metrics.char_width <= 1.0;
    let scroll = scroll_view_at(WidgetId::new(""), axis, viewport, content_len, offset);
    let matching_container = match (axis, content.unstyled()) {
        (ScrollAxis::Vertical, LayoutNode::Column(c)) => Some(c),
        (ScrollAxis::Horizontal, LayoutNode::Row(c)) => Some(c),
        _ => None,
    };
    if let Some(container) = matching_container {
        let gap_metric = if horizontal {
            metrics.char_width
        } else {
            metrics.line_height
        };
        let gap = if horizontal {
            quantise_horizontal(container.spacing.gap, gap_metric)
        } else {
            quantise_vertical(container.spacing.gap, gap_metric)
        };
        let mut cursor = 0.0_f32;
        let mut rects = Vec::with_capacity(container.children.len());
        for child in &container.children {
            let len = matching_axis_child_length(backend, child, axis, metrics);
            let start = cursor;
            cursor += len + gap;
            let visible = scroll.is_visible(start, len);
            let rect = if horizontal {
                let raw_x = viewport.x + start - offset;
                let (x, width) = if cell_grid {
                    clip_main_axis(raw_x, len, viewport.x, viewport.width)
                } else {
                    (raw_x, len)
                };
                Rect::new(x, viewport.y, width, viewport.height)
            } else {
                let raw_y = viewport.y + start - offset;
                let (y, height) = if cell_grid {
                    clip_main_axis(raw_y, len, viewport.y, viewport.height)
                } else {
                    (raw_y, len)
                };
                Rect::new(viewport.x, y, viewport.width, height)
            };
            rects.push(ScrollChildSlot {
                rect,
                visible,
                start,
                len,
            });
        }
        ScrollSlots::Children(rects)
    } else {
        let visible = scroll.is_visible(0.0, content_len);
        // `TextDisplay`/`List`/`Tree`/`DataTable` each render their own
        // `scroll_offset..` window into their data regardless of where
        // their own rect sits on screen (see
        // `offset_windowed_leaf_scrolled_to_offset`'s own doc) — shifting
        // their rect by `-offset` the way every other `Atomic` leaf kind
        // is positioned below would just repaint from row/line `0`
        // wherever that shifted rect landed, never the `offset`th one, on
        // every backend. Hand them the viewport itself instead;
        // `paint_scroll`'s own `ScrollSlots::Atomic` arm is what actually
        // advances a painted shadow clone's `scroll_offset` to match.
        if is_offset_windowed_leaf(content) {
            return ScrollSlots::Atomic(viewport, visible);
        }
        let rect = if horizontal {
            let raw_x = viewport.x - offset;
            let (x, width) = if cell_grid {
                clip_main_axis(raw_x, content_len, viewport.x, viewport.width)
            } else {
                (raw_x, content_len)
            };
            Rect::new(x, viewport.y, width, viewport.height)
        } else {
            let raw_y = viewport.y - offset;
            let (y, height) = if cell_grid {
                clip_main_axis(raw_y, content_len, viewport.y, viewport.height)
            } else {
                (raw_y, content_len)
            };
            Rect::new(viewport.x, y, viewport.width, height)
        };
        ScrollSlots::Atomic(rect, visible)
    }
}

/// Whether `content` is a bare leaf kind that renders its own
/// `scroll_offset`-relative window into its data regardless of where its
/// own rect sits on screen — a `TextDisplay`/`List`/`Tree`/`DataTable`.
/// [`scroll_content_slots`]'s own `Atomic` branch hands these the
/// viewport itself rather than a rect shifted `-offset` along the scroll
/// axis (which would just repaint from the first row/line every time);
/// [`offset_windowed_leaf_scrolled_to_offset`] is what actually advances
/// the shadow clone `paint_scroll` paints into it to show the right one.
/// Matches via [`LayoutNode::unstyled`] so a `"style"` key on this leaf
/// never changes which scroll strategy it gets.
fn is_offset_windowed_leaf(content: &LayoutNode) -> bool {
    let LayoutNode::Leaf(leaf) = content.unstyled() else {
        return false;
    };
    matches!(
        leaf.descriptor,
        LeafDescriptor::TextDisplay(_)
            | LeafDescriptor::List(_)
            | LeafDescriptor::Tree(_)
            | LeafDescriptor::DataTable(_)
    )
}

/// Clips a scroll slot's main-axis span (`origin`, `extent`) to fit
/// inside `[viewport_origin, viewport_origin + viewport_extent)`,
/// shrinking from whichever edge(s) stick out — never widening. Only
/// [`scroll_content_slots`]'s own `cell_grid` branches call this; see
/// that function's doc for why a pixel backend keeps the unclipped span
/// instead.
fn clip_main_axis(
    origin: f32,
    extent: f32,
    viewport_origin: f32,
    viewport_extent: f32,
) -> (f32, f32) {
    let end = (origin + extent).min(viewport_origin + viewport_extent);
    let clipped_origin = origin.max(viewport_origin);
    (clipped_origin, (end - clipped_origin).max(0.0))
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
/// width), and an `Extra` additive primitive via its own
/// `crate::extra_paint::ExtraPaint::natural_size` (most additive
/// primitives have a real answer on at least one axis — see that
/// method's own impls — the handful of viewport-filling/multi-cell-grid
/// ones that don't fall back to that method's own `(None, None)`
/// default). Every other node kind (a nested `Row`/`Column`/`Split`/
/// `Tabs`/`SidebarPanel`/`SplitTree`) has no cheap measurement
/// at this layer and reports [`None`] on both axes — see
/// [`NaturalSize::main`]/[`NaturalSize::cross`]'s callers for how each
/// degrades a `None`.
fn natural_size(backend: &mut dyn Backend, node: &LayoutNode, line_height: f32) -> NaturalSize {
    // A `Styled` wrapper carries no geometry of its own -- unwrap to
    // whatever it wraps, but push the override first via the same
    // `push_styled_override`/`pop_styled_override` bracket `paint_styled`
    // brackets its own recursive paint with. A leaf arm below that reads
    // `Backend::style()` (the `Button` chrome inset) sees the merged
    // value for the duration of this call, so a `padding` override on
    // this wrapper changes what gets measured here, not just how the
    // subtree later paints.
    if let LayoutNode::Styled(styled) = node {
        let push = push_styled_override(backend, &styled.style);
        let size = natural_size(backend, &styled.node, line_height);
        pop_styled_override(backend, push);
        return size;
    }
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
            // a real `Style::padding` read.
            //
            // A `"style": {"padding": ...}` override changes this: rather
            // than re-deriving the base chrome width from
            // `Backend::style().padding` outright (which would also shift
            // every unstyled button whose host sets a non-default global
            // `Style`), only the *delta* from `Style::default().padding`
            // is added on each side — zero whenever the active style's
            // padding equals the default, which is every call site that
            // never pushed an override. On a cell-grid backend the delta
            // is rounded to whole cells first, the same
            // `quantise_horizontal` rule `crate::layout::Spacing::inset`
            // already applies to a container's own padding.
            let style = backend.style();
            let char_width = backend.char_width();
            let default_padding = Style::default().padding;
            let chrome_w = if char_width <= 1.0 {
                let extra = quantise_horizontal(style.padding, char_width)
                    - quantise_horizontal(default_padding, char_width);
                4.0 + 2.0 * extra
            } else {
                24.0 + 2.0 * (style.padding - default_padding)
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
        LeafDescriptor::Extra(extra) => {
            let natural = extra.natural_size(backend);
            NaturalSize {
                width: natural.width,
                height: natural.height,
            }
        }
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
///
/// `container.wrap` delegates the whole resolution to
/// [`layout_container_rects_wrapped`] instead — see that function's own
/// doc for the multi-line shape.
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
    if container.wrap {
        return layout_container_rects_wrapped(backend, area, container, horizontal);
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

/// [`layout_container_rects`]'s multi-line path for a `wrap: true`
/// container: greedily packs children onto main-axis lines (a line always
/// takes at least one child, even one wider than the container on its
/// own — CSS flex-wrap never drops an item for being too big to fit),
/// then resolves each line's own main-axis sizes/positions through the
/// same [`resolve_main_axis_sizes`]/[`justify_starts`] the single-line
/// path uses — `justify` distributes each *line's* own leftover space,
/// not the whole container's. Lines stack along the cross axis, each
/// sized to the tallest (`Row`) / widest (`Column`) child it holds —
/// [`Align::Stretch`] fills that line's own extent, not the container's,
/// since a wrapped container's total cross extent is however many lines
/// it took, not a fixed budget to divide up front. Lines are separated by
/// the same `spacing.gap` the main axis uses between children — there is
/// no separate "row gap"/"column gap" wire key.
///
/// A child's own [`quadraui::SectionSize`] still governs its *size*
/// within whichever line it lands on (`Fixed`/`Percent`/`Content` resolve
/// exactly, `Weight`/`EqualShare` share that line's own leftover space),
/// but the line-assignment pass bins by each child's natural extent
/// instead: `Fixed`/`Percent`/`Content` use the same value they go on to
/// resolve to, while `Weight`/`EqualShare` use [`natural_size`], falling
/// back to one `main_metric` unit with no cheap measurement — there is no
/// "leftover space" to bin a `Weight`/`EqualShare` child against before
/// the lines themselves exist.
fn layout_container_rects_wrapped(
    backend: &mut dyn Backend,
    area: Rect,
    container: &LayoutContainer,
    horizontal: bool,
) -> Vec<Rect> {
    let n = container.children.len();
    let char_width = backend.char_width();
    let line_height = backend.line_height();
    let (main_metric, cross_metric, quantise_main): (f32, f32, fn(f32, f32) -> f32) = if horizontal
    {
        (char_width, line_height, quantise_horizontal)
    } else {
        (line_height, char_width, quantise_vertical)
    };
    let quantise_cross: fn(f32, f32) -> f32 = if horizontal {
        quantise_vertical
    } else {
        quantise_horizontal
    };
    let gap = quantise_main(container.spacing.gap, main_metric);
    // The distance between lines runs along the *cross* axis (vertically
    // between rows of a wrapped `Row`, horizontally between columns of a
    // wrapped `Column`), so it is quantised by the cross-axis rule against
    // the cross metric -- not `quantise_main`/`main_metric`, which govern
    // the gap *within* a line, between a line's own children.
    let cross_gap = quantise_cross(container.spacing.gap, cross_metric);
    let container_main = if horizontal { area.width } else { area.height };

    // Each child's main-axis spec (same shape the single-line path builds)
    // plus its own natural cross size, computed once up front and reused
    // by both the line-assignment pass below and the per-line
    // `resolve_main_axis_sizes`/`align_cross` calls.
    let mut specs: Vec<MainAxisSpec> = Vec::with_capacity(n);
    let mut natural_cross: Vec<Option<f32>> = Vec::with_capacity(n);
    let mut bin_hints: Vec<f32> = Vec::with_capacity(n);
    for child in &container.children {
        let natural = natural_size(backend, &child.node, line_height);
        let size = match child.size {
            SectionSize::Fixed(raw) => {
                SectionSize::Fixed(quantise_main(raw as f32, main_metric).round() as u16)
            }
            other => other,
        };
        let content = if matches!(size, SectionSize::Content) {
            natural.main(horizontal).unwrap_or(0.0)
        } else {
            0.0
        };
        let spec = MainAxisSpec {
            size,
            min: child.min.map(|v| quantise_main(v, main_metric)),
            max: child.max.map(|v| quantise_main(v, main_metric)),
            content,
        };
        let hint = match size {
            SectionSize::Fixed(raw) => raw as f32,
            SectionSize::Content => spec.content,
            SectionSize::Percent(p) => p.clamp(0.0, 1.0) * container_main,
            SectionSize::Weight(_) | SectionSize::EqualShare => {
                natural.main(horizontal).unwrap_or(main_metric)
            }
        };
        bin_hints.push(apply_min_max(&spec, hint));
        natural_cross.push(natural.cross(horizontal));
        specs.push(spec);
    }

    // Greedy line assignment: a line always holds at least its first
    // child regardless of overflow.
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut line_main = 0.0_f32;
    for (i, &hint) in bin_hints.iter().enumerate() {
        let starts_new_line = match lines.last() {
            Some(last) if !last.is_empty() => line_main + gap + hint > container_main,
            _ => true,
        };
        if starts_new_line {
            lines.push(Vec::new());
            line_main = 0.0;
        }
        let current = lines.last_mut().expect("a line was just pushed if empty");
        if !current.is_empty() {
            line_main += gap;
        }
        current.push(i);
        line_main += hint;
    }

    let mut rects = vec![Rect::new(0.0, 0.0, 0.0, 0.0); n];
    let mut cross_pos = 0.0_f32;
    for line in &lines {
        let line_usable_main = (container_main - gap * (line.len() - 1) as f32).max(0.0);
        let line_specs: Vec<MainAxisSpec> = line.iter().map(|&i| specs[i]).collect();
        let sizes = resolve_main_axis_sizes(&line_specs, container_main, line_usable_main);
        let starts = justify_starts(&sizes, container_main, gap, container.justify);
        let line_cross = line
            .iter()
            .map(|&i| natural_cross[i].unwrap_or(cross_metric))
            .fold(0.0_f32, f32::max);
        for (local, &i) in line.iter().enumerate() {
            let (cross_offset, cross_size) =
                align_cross(line_cross, natural_cross[i], container.align);
            let main_start = starts[local];
            let main_size = sizes[local];
            rects[i] = if horizontal {
                Rect::new(
                    area.x + main_start,
                    area.y + cross_pos + cross_offset,
                    main_size,
                    cross_size,
                )
            } else {
                Rect::new(
                    area.x + cross_pos + cross_offset,
                    area.y + main_start,
                    cross_size,
                    main_size,
                )
            };
        }
        cross_pos += line_cross + cross_gap;
    }

    rects
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
/// `Split`/`Tabs` call `Spacing::inset` directly since they have no
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

/// A `{start, total}` virtualised window lifted out of a `"list"`/`"tree"`/
/// `"data_table"`/`"text_display"` leaf's own descriptor dict by
/// [`items_window_from_value`] — `start` is the index, into the widget's
/// true row/item/line count `total`, that this frame's `items`/`rows`/
/// `lines` payload begins at. Like [`crate::scroll_px_from_value`], this is
/// **not** a field any of `ListView`/`TreeView`/`DataTable`/`TextDisplay`
/// declares: a render callback that never sets `"items_window"` is
/// completely unaffected, and one that does only sends the slice of rows
/// currently visible (plus whatever prefetch margin it chooses) instead of
/// every row up front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemsWindow {
    pub start: usize,
    pub total: usize,
}

/// Lifts an optional `"items_window": {"start": ..., "total": ...}` object
/// out of a `list`/`tree`/`data_table`/`text_display` leaf's own descriptor
/// dict (the nested value under its `LEAF_KIND_KEYS` key, e.g. the `{...}`
/// in `{"id": ..., "data_table": {...}}`) — mirrors
/// [`crate::scroll_px_from_value`]'s own extraction, reading straight out of
/// the raw [`serde_json::Value`] a binding already holds rather than
/// requiring a dedicated struct field. Returns `None` when the key is
/// absent, isn't an object, or either `start`/`total` is missing or not a
/// non-negative integer — a malformed or missing `items_window` degrades to
/// "this frame's payload holds every row" rather than failing the leaf's
/// decode.
pub fn items_window_from_value(value: &serde_json::Value) -> Option<ItemsWindow> {
    let window = value.as_object()?.get("items_window")?.as_object()?;
    let start = usize::try_from(window.get("start")?.as_u64()?).ok()?;
    let total = usize::try_from(window.get("total")?.as_u64()?).ok()?;
    Some(ItemsWindow { start, total })
}

/// The `{id, start, count}` a binding should ask its app's render callback
/// for next, computed by [`rows_requested_for_window`] once scrolling moves
/// outside the slice an [`ItemsWindow`] says is currently loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowsRequestedEvent {
    pub id: WidgetId,
    pub start: usize,
    pub count: usize,
}

/// Computes the [`RowsRequestedEvent`] a virtualised `list`/`tree`/
/// `data_table`/`text_display` leaf needs for its next frame, given
/// `window` (this frame's [`ItemsWindow`]), `loaded_len` (how many rows the
/// current payload actually carries — `ListView::items.len()`,
/// `TreeView::rows.len()`, `DataTable::rows.len()`, or
/// `TextDisplay::lines.len()`), and `scroll_offset`/`viewport_rows`.
///
/// `viewport_rows` is the viewport's true row capacity. A windowed leaf's
/// own `PaintedHit::*.viewport_rows` field is that capacity directly —
/// its band is painted from offset `0` (see [`WindowedLeaf`]), so even
/// `PaintedHit::DataTable`'s `DataTableLayout::visible_rows` is measured
/// against a band that starts at the first visible row rather than
/// degrading toward zero near the end of the collection. For an
/// UNWINDOWED leaf, whose descriptor is painted at its real offset, that
/// same field does degrade near the end, so a caller passing one must use
/// [`list_viewport_rows`]/[`tree_viewport_rows`]/[`data_table_viewport_rows`]
/// (the capacity at a *zero* offset) instead, or the margin below starves
/// into a near-empty request.
///
/// `scroll_offset` is **absolute**: an index into the widget's full
/// `window.total` collection, not the loaded payload alone — the same
/// value [`paint_layout_windowed`] leaves on this leaf's `PaintedHit` this
/// frame (e.g. `PaintedHit::DataTable::scroll_offset`), which is absolute
/// whether or not a window is in play. [`crate::InteractionEngine::handle`]
/// is what pairs the two in production: it reads this leaf's
/// [`crate::hit::LoadedRows`] off the hit the last frame painted and calls
/// this with the absolute offset the scroll input it just handled resolved
/// to.
///
/// Returns `None` once the loaded slice `[window.start, window.start +
/// loaded_len)` already covers the visible range `[scroll_offset,
/// scroll_offset + viewport_rows)` — a render callback that already sent
/// enough rows re-renders unchanged. Otherwise returns a request covering
/// the visible range plus one viewport's worth of prefetch margin on each
/// side, clamped to `[0, window.total)`: bounded by a small multiple of
/// `viewport_rows`, never by `window.total`, so a 100k-row window never
/// asks for more than a few hundred rows at once. Returns `None` instead of
/// a zero-`count` request when the clamp leaves nothing to ask for (e.g.
/// `scroll_offset >= window.total`).
pub fn rows_requested_for_window(
    id: WidgetId,
    window: ItemsWindow,
    loaded_len: usize,
    scroll_offset: usize,
    viewport_rows: usize,
) -> Option<RowsRequestedEvent> {
    let visible_start = scroll_offset.min(window.total);
    let visible_end = visible_start
        .saturating_add(viewport_rows)
        .min(window.total);
    let loaded_end = window.start.saturating_add(loaded_len);
    let covers = window.start <= visible_start && loaded_end >= visible_end;
    if covers {
        return None;
    }
    let margin = viewport_rows.max(1);
    let start = visible_start.saturating_sub(margin);
    let end = visible_end.saturating_add(margin).min(window.total);
    let count = end.saturating_sub(start);
    if count == 0 {
        return None;
    }
    Some(RowsRequestedEvent { id, start, count })
}

/// Rows a windowed leaf's band materialises beyond its viewport's own
/// measured capacity (see [`windowed_band_rows`]) — a small margin so a
/// partially-visible last row, a title/header row the capacity probe
/// resolves differently, or a backend's own sub-row pixel shift always
/// still has a row to land on.
const WINDOWED_BAND_PAD_ROWS: usize = 8;

/// The contiguous slice of a windowed collection that one frame actually
/// materialises for painting: `len` rows starting at absolute index
/// `start`, which is the row painted at the very top of the viewport.
///
/// Virtualisation exists to make a large collection cheap, so the band is
/// sized off the viewport — NOT off `ItemsWindow::total`, which is both
/// unbounded (a wire field: a `"total": 10000000000` payload would attempt
/// a multi-hundred-GB allocation, and Rust aborts on allocation failure)
/// and, at a 100,000-row scale, far more per-frame allocation and cloning
/// than painting a few dozen visible rows can justify. The collection's
/// real extent reaches the scrollbar arithmetic and the keyboard navigation
/// through [`crate::hit::PaintedHit`]'s own `total_rows`/`total_items`/
/// `total_lines` + [`crate::hit::LoadedRows`] fields instead, never through
/// the materialised `Vec`'s own `len()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowedBand {
    start: usize,
    len: usize,
}

/// How many rows a leaf painted into `area` needs materialised: an upper
/// bound on the viewport's own row capacity (`area.height` over
/// [`Backend::line_height`], which is what every `*_viewport_rows` probe
/// resolves against) plus [`WINDOWED_BAND_PAD_ROWS`].
fn windowed_band_rows(backend: &mut dyn Backend, area: Rect) -> usize {
    band_rows_for(area.height, backend.line_height())
}

/// [`windowed_band_rows`]'s own arithmetic, over plain numbers.
fn band_rows_for(height: f32, line_height: f32) -> usize {
    let rows = if line_height > 0.0 {
        (height.max(0.0) / line_height).ceil() as usize
    } else {
        0
    };
    rows.saturating_add(WINDOWED_BAND_PAD_ROWS)
}

/// The band `band_rows` rows wide that paints a windowed collection
/// scrolled to absolute `scroll_offset`.
///
/// `start` is `scroll_offset` clamped into `[0, real_total)` — the same
/// clamp every primitive's own layout applies to a descriptor's
/// `scroll_offset` (`ListViewLayout::resolved_scroll_offset`'s own doc),
/// so a band near the end of the collection paints the same short tail an
/// unwindowed one does. `real_total` widens past `window.total` when a
/// stale or malformed total understates `window.start + loaded_len`, so a
/// bad total never truncates real loaded data.
fn windowed_band(
    window: ItemsWindow,
    loaded_len: usize,
    scroll_offset: usize,
    band_rows: usize,
) -> WindowedBand {
    let real_total = window.total.max(window.start.saturating_add(loaded_len));
    let start = scroll_offset.min(real_total.saturating_sub(1));
    let len = band_rows.max(1).min(real_total.saturating_sub(start));
    WindowedBand { start, len }
}

/// Everything painting one windowed leaf needs that its descriptor can't
/// carry: which band to materialise, and the absolute geometry its
/// [`crate::hit::PaintedHit`] must report in place of the band-relative
/// values the backend sees.
///
/// A windowed leaf is painted from a descriptor whose `items`/`rows`/
/// `lines` is only [`Self::band`] (see [`WindowedBand`]'s own doc for why),
/// so its own `scroll_offset` is rewritten to `0` and every index the
/// backend's returned layout reports is band-relative. [`shift_list_layout`]/
/// [`shift_tree_layout`]/[`shift_text_display_layout`] shift those indices
/// back by `band.start` before they reach a hit, so nothing downstream of
/// paint ever sees a band-relative index.
#[derive(Debug, Clone, Copy)]
struct WindowedLeaf {
    band: WindowedBand,
    /// The absolute row index the band's own row `0` sits at — what the
    /// leaf's hit reports as its `scroll_offset`.
    scroll_offset: usize,
    /// `LayoutLeaf::scroll_px` rebased onto the band: the sub-row pixel
    /// remainder of the absolute pixel offset, so a pixel-smooth backend
    /// still paints a partially-clipped first row.
    scroll_px: Option<f32>,
    /// The collection's real extent — `ItemsWindow::total`, widened to
    /// cover the loaded slice when a stale total understates it.
    total: usize,
    loaded: LoadedRows,
}

/// Resolves a windowed leaf's band and absolute geometry, from `window`,
/// the loaded payload's own length, the descriptor's own `scroll_offset`,
/// and the leaf's optional pixel-precise `scroll_px`.
///
/// `scroll_px`, when set, is the authority on where the leaf is scrolled to
/// (it is what every backend's own layout resolves its first visible row
/// from, ignoring `scroll_offset`), so the absolute row index comes from
/// `scroll_px / line_height` in that case and only the sub-row remainder is
/// handed on to the backend.
fn windowed_leaf(
    window: ItemsWindow,
    loaded_len: usize,
    descriptor_offset: usize,
    scroll_px: Option<f32>,
    line_height: f32,
    band_rows: usize,
) -> WindowedLeaf {
    let (scroll_offset, scroll_px) = match scroll_px {
        Some(px) if line_height > 0.0 => {
            let rows = (px / line_height).floor().max(0.0);
            (rows as usize, Some(px - rows * line_height))
        }
        other => (descriptor_offset, other),
    };
    let band = windowed_band(window, loaded_len, scroll_offset, band_rows);
    WindowedLeaf {
        band,
        scroll_offset: band.start,
        scroll_px,
        total: window.total.max(window.start.saturating_add(loaded_len)),
        loaded: LoadedRows {
            start: window.start,
            len: loaded_len,
        },
    }
}

/// Materialises `band` out of `loaded` — this frame's payload, covering the
/// half-open absolute range `[window.start, window.start + loaded.len())` —
/// filling every band position the payload has no row for with
/// `placeholder(absolute_index)`.
///
/// Every one of `ListView::items`/`TreeView::rows`/`DataTable::rows`/
/// `TextDisplay::lines` needs this same shape before it reaches a
/// `Backend::draw_*` call: each backend's own paint loop indexes straight
/// into the `Vec` at whatever row its layout decides is visible, so a
/// position past the loaded slice needs an empty placeholder row to land on
/// instead of indexing off the end.
///
/// The returned `Vec` is exactly `band.len` long — a viewport's worth (see
/// [`WindowedBand`]'s own doc), never `window.total` — so the band's row
/// `0` is the collection's row `band.start`, not its row `0`. A caller
/// needs the collection's true extent (a scrollbar's thumb size, Home/End
/// navigation, the next `RowsRequestedEvent`) read off
/// [`WindowedLeaf::total`]/[`WindowedLeaf::loaded`] rather than this
/// `Vec`'s own `len()`.
fn windowed_vec<T: Clone>(
    loaded: &[T],
    window: ItemsWindow,
    band: WindowedBand,
    mut placeholder: impl FnMut(usize) -> T,
) -> Vec<T> {
    let mut out = Vec::with_capacity(band.len);
    for i in 0..band.len {
        let absolute = band.start.saturating_add(i);
        match absolute
            .checked_sub(window.start)
            .and_then(|loaded_idx| loaded.get(loaded_idx))
        {
            Some(row) => out.push(row.clone()),
            None => out.push(placeholder(absolute)),
        }
    }
    out
}

/// [`windowed_vec`] for a [`DataTable`]'s own `rows` — an empty-`cells`
/// [`DataRow`] at every band position outside the loaded slice. Safe
/// because every cell read goes through `Vec::get` (see `DataTableLayout`'s
/// own paint code), never a direct index, so a short `cells` paints as
/// blank columns rather than panicking.
///
/// `selected_idx` is rebased onto the band and dropped outright when the
/// selected row isn't in it — a selection outside the painted band has no
/// row to highlight.
fn windowed_data_table(table: &DataTable, window: ItemsWindow, leaf: WindowedLeaf) -> DataTable {
    let rows = windowed_vec(&table.rows, window, leaf.band, |_| DataRow {
        cells: Vec::new(),
        decoration: Decoration::default(),
    });
    DataTable {
        rows,
        scroll_offset: 0,
        selected_idx: band_relative(table.selected_idx, leaf.band),
        ..table.clone()
    }
}

/// [`windowed_vec`] for a [`ListView`]'s own `items` — an empty-text
/// [`ListItem`] at every band position outside the loaded slice.
///
/// `ListView::selected_idx` is a plain `usize` with no "nothing selected"
/// value, so a selection outside the band is rebased to `band.len` — one
/// past the last materialised row, an index no painted row ever matches.
fn windowed_list(list: &ListView, window: ItemsWindow, leaf: WindowedLeaf) -> ListView {
    let items = windowed_vec(&list.items, window, leaf.band, |_| ListItem {
        text: StyledText::plain(""),
        icon: None,
        detail: None,
        decoration: Decoration::default(),
    });
    ListView {
        items,
        scroll_offset: 0,
        selected_idx: band_relative(Some(list.selected_idx), leaf.band).unwrap_or(leaf.band.len),
        ..list.clone()
    }
}

/// [`windowed_vec`] for a [`TreeView`]'s own `rows` — an empty-text leaf
/// [`TreeRow`] at every band position outside the loaded slice, each
/// carrying a synthetic single-level path built from its own absolute
/// index.
///
/// That path is **not** a stable identity: `TreePath` is an app-chosen
/// `Vec<u16>` (`quadraui::types::TreePath`) with no value range reserved
/// for this module, so a synthetic `[idx]` can and does collide with a
/// real loaded row's own path (a tree whose top-level node `0` expands 100
/// children puts a real row at path `[1]` sitting at flattened index 101,
/// colliding with the placeholder generated for index 1 — and every index
/// past `u16::MAX` saturates to the same `[u16::MAX]`). Painting a
/// collided path is harmless (nothing compares two placeholders' paths
/// against each other), but [`paint_leaf_windowed`] must never let one of
/// these synthetic paths escape into [`crate::hit::PaintedHit::Tree::row_paths`]
/// as if it identified a real row — see that field's own doc for how only
/// the loaded slice's own paths are recorded instead.
///
/// `selected_path` needs no rebasing: it is matched by value against the
/// band's own rows, so a selection outside the band simply matches
/// nothing.
fn windowed_tree(tree: &TreeView, window: ItemsWindow, leaf: WindowedLeaf) -> TreeView {
    let rows = windowed_vec(&tree.rows, window, leaf.band, |idx| TreeRow {
        path: vec![u16::try_from(idx).unwrap_or(u16::MAX)] as TreePath,
        indent: 0,
        icon: None,
        text: StyledText::plain(""),
        badge: None,
        is_expanded: None,
        decoration: Decoration::default(),
        edit: None,
    });
    TreeView {
        rows,
        scroll_offset: 0,
        ..tree.clone()
    }
}

/// [`windowed_vec`] for a [`TextDisplay`]'s own `lines` — an empty-spans
/// [`TextDisplayLine`] at every band position outside the loaded slice.
///
/// `auto_scroll` is left exactly as the app set it: the band a windowed
/// display paints with it set is the collection's own tail (see
/// [`windowed_text_display_band`]), so "keep the last line visible"
/// resolves against the real last line rather than the band's.
fn windowed_text_display(td: &TextDisplay, window: ItemsWindow, leaf: WindowedLeaf) -> TextDisplay {
    let lines = windowed_vec(&td.lines, window, leaf.band, |_| TextDisplayLine {
        spans: Vec::new(),
        decoration: Decoration::default(),
        timestamp: None,
    });
    TextDisplay {
        lines,
        scroll_offset: 0,
        ..td.clone()
    }
}

/// [`windowed_leaf`] for a [`TextDisplay`], which has one scroll input
/// neither of the other three kinds does: [`TextDisplay::auto_scroll`]
/// pins the viewport to the collection's last line regardless of
/// `scroll_offset`. The band is therefore anchored at the tail — the last
/// `band_rows` rows of the real extent — so the primitive's own
/// "last line visible" resolution lands on the collection's final line and
/// the rows requested next are the tail's, not row `0`'s.
fn windowed_text_display_band(
    td: &TextDisplay,
    window: ItemsWindow,
    scroll_px: Option<f32>,
    line_height: f32,
    band_rows: usize,
) -> WindowedLeaf {
    let loaded_len = td.lines.len();
    if !td.auto_scroll {
        return windowed_leaf(
            window,
            loaded_len,
            td.scroll_offset,
            scroll_px,
            line_height,
            band_rows,
        );
    }
    let real_total = window.total.max(window.start.saturating_add(loaded_len));
    let tail_start = real_total.saturating_sub(band_rows.max(1));
    windowed_leaf(window, loaded_len, tail_start, None, line_height, band_rows)
}

/// `idx` rebased from an absolute collection index onto `band`, or `None`
/// when it isn't inside the band at all.
fn band_relative(idx: Option<usize>, band: WindowedBand) -> Option<usize> {
    idx?.checked_sub(band.start).filter(|rel| *rel < band.len)
}

/// Shifts every row index [`Backend::list_layout`] resolved against a
/// windowed list's band back into the collection's own absolute indexing,
/// so a hit test or a selection read downstream of paint never sees a
/// band-relative index — see [`WindowedLeaf`]'s own doc.
fn shift_list_layout(layout: &mut ListViewLayout, by: usize) {
    for item in &mut layout.visible_items {
        item.item_idx = item.item_idx.saturating_add(by);
    }
    for (_, hit) in &mut layout.hit_regions {
        if let ListViewHit::Item(idx) = hit {
            *idx = idx.saturating_add(by);
        }
    }
    layout.resolved_scroll_offset = layout.resolved_scroll_offset.saturating_add(by);
}

/// [`shift_list_layout`] for a windowed tree's [`Backend::tree_layout`].
fn shift_tree_layout(layout: &mut TreeViewLayout, by: usize) {
    for row in &mut layout.visible_rows {
        row.row_idx = row.row_idx.saturating_add(by);
    }
    for (_, hit) in &mut layout.hit_regions {
        match hit {
            TreeViewHit::Row(idx) | TreeViewHit::Chevron(idx) => {
                *idx = idx.saturating_add(by);
            }
            TreeViewHit::Empty => {}
        }
    }
    layout.resolved_scroll_offset = layout.resolved_scroll_offset.saturating_add(by);
}

/// [`shift_list_layout`] for a windowed display's
/// [`Backend::text_display_layout`].
fn shift_text_display_layout(layout: &mut TextDisplayLayout, by: usize) {
    for line in &mut layout.visible_lines {
        line.line_idx = line.line_idx.saturating_add(by);
    }
    for (_, hit) in &mut layout.hit_regions {
        if let TextDisplayHit::Line(idx) = hit {
            *idx = idx.saturating_add(by);
        }
    }
    layout.resolved_scroll_offset = layout.resolved_scroll_offset.saturating_add(by);
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
///
/// [`draw_tree_with_scrollbar_and_state_windowed`] with `extent: None` — a
/// tree with no [`ItemsWindow`] in play sizes its scrollbar off
/// `tree.rows.len()` exactly as this always has.
pub fn draw_tree_with_scrollbar_and_state(
    backend: &mut dyn Backend,
    area: Rect,
    tree: &TreeView,
    scroll_px: Option<f32>,
) -> (Rect, Option<Scrollbar>, usize) {
    draw_tree_with_scrollbar_and_state_windowed(backend, area, tree, scroll_px, None)
}

/// A windowed tree's real scrollbar geometry, which its painted descriptor
/// can no longer carry: `total_rows` is the collection's own extent
/// (`ItemsWindow::total`) rather than the band's length, and
/// `scroll_offset` is the band's own absolute start rather than the `0` the
/// band-relative descriptor paints from — see [`WindowedBand`]'s own doc
/// for why the painted `rows` is only a band.
#[derive(Debug, Clone, Copy)]
pub struct WindowedTreeExtent {
    pub total_rows: usize,
    pub scroll_offset: usize,
}

/// [`draw_tree_with_scrollbar_and_state`], plus an optional
/// [`WindowedTreeExtent`] — [`paint_leaf_windowed`] passes a windowed
/// tree's own real total and absolute offset here, every other caller
/// passes `None`.
///
/// This is the hook a windowed tree's scrollbar needs: a windowed tree's
/// painted `rows` is only the band it actually shows (see
/// [`WindowedBand`]'s own doc), so sizing the thumb off `tree.rows.len()`
/// and positioning it off `tree.scroll_offset` the way an unwindowed tree
/// always has would shrink the thumb to a viewport's worth and pin it to
/// the top — `extent` lets the thumb, its position, and the "does this
/// even need a scrollbar" check below all use the collection's real
/// numbers instead.
///
/// On a backend that paints its own scrollbar overlay inside `draw_tree`
/// ([`Backend::tree_paints_own_scrollbar`] — GTK, macOS, Win), `extent`
/// has no effect: [`Backend::tree_vscrollbar`] still sizes its own thumb
/// off `tree.rows.len()` internally, so a windowed tree's thumb on those
/// backends reflects the painted band, not the real total, until `Backend`
/// grows its own windowed-extent hook — tracked as a per-backend
/// follow-up, not fixed here (TUI, the backend this module's own tests
/// exercise, takes the explicit-gutter branch below and is unaffected).
pub fn draw_tree_with_scrollbar_and_state_windowed(
    backend: &mut dyn Backend,
    area: Rect,
    tree: &TreeView,
    scroll_px: Option<f32>,
    extent: Option<WindowedTreeExtent>,
) -> (Rect, Option<Scrollbar>, usize) {
    let total_rows = extent.map_or(tree.rows.len(), |extent| extent.total_rows);
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
    if total_rows <= viewport_rows {
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
    let thumb_offset = match (extent, scroll_px) {
        // A windowed tree's absolute offset is whole rows (its band starts
        // at a row boundary and the sub-row remainder is what reaches the
        // backend), so the pixel-precise branch below has nothing to add.
        (Some(extent), _) => extent.scroll_offset as f32,
        (None, Some(px)) if line_height > 0.0 => px / line_height,
        _ => tree.scroll_offset as f32,
    };
    let scrollbar = Scrollbar::vertical(
        format!("{}-scrollbar", tree.id),
        track_rect,
        thumb_offset,
        total_rows as f32,
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
        LayoutNode::Grid(grid) => {
            for child in &grid.children {
                collect_leaf_tooltips(child, out);
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
        LayoutNode::SidebarPanel(sidebar_node) => collect_leaf_tooltips(&sidebar_node.content, out),
        LayoutNode::SplitTree(split_tree) => {
            for leaf in &split_tree.leaves {
                collect_leaf_tooltips(leaf, out);
            }
        }
        LayoutNode::Scroll(scroll_node) => collect_leaf_tooltips(&scroll_node.content, out),
        LayoutNode::Styled(styled) => collect_leaf_tooltips(&styled.node, out),
    }
}

/// Paints one leaf's descriptor into the whole `area` its container gave
/// it. Appends a [`PaintedHit`] for it, keyed by the leaf's own tree-address
/// `id` (not the descriptor's embedded `id`, which a render callback may
/// leave at its default and reuse across leaves) so a later pointer event
/// can be routed back to this exact slot in the layout tree.
///
/// [`paint_leaf_windowed`] with an empty `windows` map, so a leaf whose
/// decode never recorded an `ItemsWindow` paints exactly as this always
/// has.
pub fn paint_leaf(
    backend: &mut dyn Backend,
    area: Rect,
    leaf: &LayoutLeaf,
    hits: &mut Vec<PaintedHit>,
    chrome_interaction: &dyn Fn(WidgetId) -> InteractionState,
) {
    paint_leaf_windowed(
        backend,
        area,
        leaf,
        hits,
        chrome_interaction,
        &HashMap::new(),
    );
}

/// [`paint_leaf`], additionally painting this leaf's `List`/`Tree`/
/// `DataTable`/`TextDisplay` from the band of its collection the viewport
/// actually shows, when `windows` (keyed by this leaf's own `id`) has an
/// entry for it — see [`WindowedBand`]'s own doc for why only a band is
/// ever materialised, and [`WindowedLeaf`]'s for how the collection's real
/// extent reaches the hit instead. A leaf with no entry in `windows` (every
/// leaf, on a tree decoded without any `"items_window"` key) paints
/// `leaf.descriptor` verbatim, identically to [`paint_leaf`].
pub fn paint_leaf_windowed(
    backend: &mut dyn Backend,
    area: Rect,
    leaf: &LayoutLeaf,
    hits: &mut Vec<PaintedHit>,
    chrome_interaction: &dyn Fn(WidgetId) -> InteractionState,
    windows: &HashMap<String, ItemsWindow>,
) {
    let id = WidgetId::new(leaf.id.clone());
    let window = windows.get(&leaf.id).copied();
    match &leaf.descriptor {
        LeafDescriptor::Button(button) => {
            let layout = backend.draw_button(area, button);
            hits.push(PaintedHit::Button { id, layout });
        }
        LeafDescriptor::TextDisplay(text_display) => {
            let windowed = window.map(|w| {
                windowed_text_display_band(
                    text_display,
                    w,
                    leaf.scroll_px,
                    backend.line_height(),
                    windowed_band_rows(backend, area),
                )
            });
            let painted: Cow<TextDisplay> = match (window, windowed) {
                (Some(w), Some(windowed)) => {
                    Cow::Owned(windowed_text_display(text_display, w, windowed))
                }
                _ => Cow::Borrowed(text_display),
            };
            let mut hit = draw_text_display_hit(backend, id, area, painted.as_ref());
            // `draw_text_display_hit` reports `td.lines.len()` and a
            // layout resolved against the painted descriptor — both correct
            // for an unwindowed leaf, but a windowed one only ever paints
            // its own band (see [`WindowedBand`]'s doc), so the real total
            // and the band's absolute offset are threaded in here instead.
            if let (
                Some(windowed),
                PaintedHit::TextDisplay {
                    layout,
                    total_lines,
                    loaded_rows,
                    ..
                },
            ) = (windowed, &mut hit)
            {
                shift_text_display_layout(layout, windowed.band.start);
                *total_lines = windowed.total;
                *loaded_rows = Some(windowed.loaded);
            }
            hits.push(hit);
        }
        LeafDescriptor::List(list) => {
            let windowed = window.map(|w| {
                windowed_leaf(
                    w,
                    list.items.len(),
                    list.scroll_offset,
                    leaf.scroll_px,
                    backend.line_height(),
                    windowed_band_rows(backend, area),
                )
            });
            let painted: Cow<ListView> = match (window, windowed) {
                (Some(w), Some(windowed)) => Cow::Owned(windowed_list(list, w, windowed)),
                _ => Cow::Borrowed(list),
            };
            let scroll_px = windowed.map_or(leaf.scroll_px, |windowed| windowed.scroll_px);
            let painted_list = painted.as_ref();
            let state = quadraui::ListPaintState::default().with_scroll_px(scroll_px);
            backend.draw_list_with_state(area, painted_list, &state, false);
            let mut layout = backend.list_layout(area, painted_list);
            let viewport_rows = list_viewport_rows(backend, area, painted_list);
            // The collection's own numbers, not the painted band's — see
            // [`WindowedLeaf`]'s own doc.
            if let Some(windowed) = windowed {
                shift_list_layout(&mut layout, windowed.band.start);
            }
            hits.push(PaintedHit::List {
                id,
                area,
                clip: area,
                layout,
                selected_idx: list.selected_idx,
                total_items: windowed.map_or(painted_list.items.len(), |windowed| windowed.total),
                scroll_offset: windowed.map_or(painted_list.scroll_offset, |windowed| {
                    windowed.scroll_offset
                }),
                viewport_rows,
                loaded_rows: windowed.map(|windowed| windowed.loaded),
            });
        }
        LeafDescriptor::Tree(tree) => {
            let windowed = window.map(|w| {
                windowed_leaf(
                    w,
                    tree.rows.len(),
                    tree.scroll_offset,
                    leaf.scroll_px,
                    backend.line_height(),
                    windowed_band_rows(backend, area),
                )
            });
            let painted: Cow<TreeView> = match (window, windowed) {
                (Some(w), Some(windowed)) => Cow::Owned(windowed_tree(tree, w, windowed)),
                _ => Cow::Borrowed(tree),
            };
            let painted_tree = painted.as_ref();
            let scroll_px = windowed.map_or(leaf.scroll_px, |windowed| windowed.scroll_px);
            let (content_rect, scrollbar, viewport_rows) =
                draw_tree_with_scrollbar_and_state_windowed(
                    backend,
                    area,
                    painted_tree,
                    scroll_px,
                    windowed.map(|windowed| WindowedTreeExtent {
                        total_rows: windowed.total,
                        scroll_offset: windowed.scroll_offset,
                    }),
                );
            let mut layout = backend.tree_layout(content_rect, painted_tree);
            if let Some(windowed) = windowed {
                shift_tree_layout(&mut layout, windowed.band.start);
            }
            // Only the rows this frame actually loaded, in visual order —
            // a placeholder's own synthetic path (see [`windowed_tree`]'s
            // own doc for why it can collide with a real row's path) never
            // escapes into this field, so a path-keyed lookup downstream
            // can't resolve a click or a cursor to the wrong row. Taken
            // from the ORIGINAL descriptor, not the painted band, since the
            // loaded slice and the band only partly overlap.
            let row_paths: Vec<TreePath> = tree.rows.iter().map(|row| row.path.clone()).collect();
            hits.push(PaintedHit::Tree {
                id,
                area: content_rect,
                clip: content_rect,
                layout,
                row_paths,
                scrollbar,
                scroll_offset: windowed.map_or(painted_tree.scroll_offset, |windowed| {
                    windowed.scroll_offset
                }),
                total_rows: windowed.map_or(painted_tree.rows.len(), |windowed| windowed.total),
                viewport_rows,
                editing: tree
                    .rows
                    .iter()
                    .find_map(|row| row.edit.clone().map(|edit| (row.path.clone(), edit))),
                loaded_rows: windowed.map(|windowed| windowed.loaded),
            });
        }
        LeafDescriptor::DataTable(data_table) => {
            let windowed = window.map(|w| {
                windowed_leaf(
                    w,
                    data_table.rows.len(),
                    data_table.scroll_offset,
                    leaf.scroll_px,
                    backend.line_height(),
                    windowed_band_rows(backend, area),
                )
            });
            let painted: Cow<DataTable> = match (window, windowed) {
                (Some(w), Some(windowed)) => {
                    Cow::Owned(windowed_data_table(data_table, w, windowed))
                }
                _ => Cow::Borrowed(data_table),
            };
            let painted_table = painted.as_ref();
            let scroll_px = windowed.map_or(leaf.scroll_px, |windowed| windowed.scroll_px);
            let state = quadraui::DataTablePaintState::default().with_scroll_px(scroll_px);
            // `DataTableLayout` carries no row indices of its own — a row
            // hit resolves through `DataTableLayout::hit_test`'s own
            // `scroll_offset`/`total_rows` arguments, which a caller reads
            // off this hit's absolute fields — so unlike the `List`/`Tree`/
            // `TextDisplay` arms there is no layout index to shift back.
            let layout = backend.draw_data_table_with_state(area, painted_table, &state);
            let viewport_rows = layout.visible_rows;
            hits.push(PaintedHit::DataTable {
                id,
                area,
                clip: area,
                layout,
                scroll_offset: windowed.map_or(painted_table.scroll_offset, |windowed| {
                    windowed.scroll_offset
                }),
                total_rows: windowed.map_or(painted_table.rows.len(), |windowed| windowed.total),
                selected_idx: data_table.selected_idx,
                viewport_rows,
                loaded_rows: windowed.map(|windowed| windowed.loaded),
            });
        }
        LeafDescriptor::TabBar(tab_bar) => {
            let layout = backend.draw_tab_bar_layout(area, tab_bar, None);
            hits.push(PaintedHit::TabBar {
                id,
                area,
                clip: area,
                layout,
                focused_index: None,
                tab_count: tab_bar.tabs.len(),
            });
        }
        LeafDescriptor::Extra(extra) => {
            let interaction = chrome_interaction(id);
            let (chrome_hits, tree_table_hit) = paint_extra(backend, extra, area, &interaction);
            hits.push(PaintedHit::Extra {
                id,
                area,
                extra: Rc::clone(extra),
                chrome_hits,
                tree_table_hit,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::*;
    use crate::layout::{EdgeInsets, Spacing, StyleOverride};
    use quadraui::{Decoration, StyledSpan, TextDisplayLine};

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

    // ── Split/Tabs spacing ───────────────────────────────────────────────

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

    /// Minimal [`quadraui::AppLogic`] around a `Row` of three `Button`
    /// leaves, the middle one wrapped in its own [`LayoutNode::Styled`]
    /// nested inside an outer [`LayoutNode::Styled`] wrapping the whole
    /// row — the acceptance scenario for per-node style overrides: a
    /// `"style"` override changes exactly the subtree it wraps, a nested
    /// override composes with (and wins over) an ancestor's, and an
    /// unstyled sibling is left alone.
    struct StyledRowLogic {
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for StyledRowLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let leaf = |id: &str, label: &str| {
                LayoutNode::Leaf(LayoutLeaf {
                    id: id.to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(WidgetId::new(id), label)),
                    scroll_px: None,
                })
            };
            let row = LayoutNode::Row(LayoutContainer::new(
                vec![
                    leaf("a", "Alpha"),
                    LayoutNode::Styled(crate::layout::LayoutStyled {
                        style: Box::new(StyleOverride {
                            fg: Some(quadraui::Color::rgb(0, 255, 0)),
                            ..StyleOverride::default()
                        }),
                        node: Box::new(leaf("b", "Bravo")),
                    }),
                    leaf("c", "Charlie"),
                ],
                Spacing::default(),
            ));
            let node = LayoutNode::Styled(crate::layout::LayoutStyled {
                style: Box::new(StyleOverride {
                    fg: Some(quadraui::Color::rgb(255, 0, 0)),
                    ..StyleOverride::default()
                }),
                node: Box::new(row),
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

    #[test]
    fn styled_wrapper_scopes_an_override_to_its_own_subtree_and_composes_with_nesting() {
        use quadraui::tui::testing::TuiDriver;

        let logic = StyledRowLogic {
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 42, 3);
        driver.render();

        let (ax, ay) = driver.find("Alpha").expect("Alpha should paint");
        let (bx, by) = driver.find("Bravo").expect("Bravo should paint");
        let (cx, cy) = driver.find("Charlie").expect("Charlie should paint");

        let alpha_fg = driver
            .style_at(ax as u16, ay as u16)
            .expect("style at Alpha")
            .fg;
        let bravo_fg = driver
            .style_at(bx as u16, by as u16)
            .expect("style at Bravo")
            .fg;
        let charlie_fg = driver
            .style_at(cx as u16, cy as u16)
            .expect("style at Charlie")
            .fg;

        let red = quadraui::tui::ratatui_color(quadraui::Color::rgb(255, 0, 0));
        let green = quadraui::tui::ratatui_color(quadraui::Color::rgb(0, 255, 0));
        assert_eq!(
            alpha_fg, red,
            "Alpha has no override of its own, so it inherits the outer row's"
        );
        assert_eq!(
            bravo_fg, green,
            "Bravo's own nested override wins over the outer row's"
        );
        assert_eq!(
            charlie_fg, red,
            "Charlie (Bravo's sibling) is unaffected by Bravo's own override, \
             and still inherits the outer row's"
        );
    }

    /// [`StyledRowLogic`]'s own scenario, but for `"bg"` instead of
    /// `"fg"`: three `ButtonVariant::Icon` leaves (TUI's `tui::button`
    /// reads `Theme::background` for that one variant -- see
    /// [`crate::layout::StyleOverride`]'s own doc table), the middle one
    /// nested-overridden inside an outer row-level override. Covers the
    /// acceptance bar's own "overriding bg ... on a container changes
    /// exactly that subtree's cells" case, which the matching `"fg"` test
    /// above doesn't exercise.
    struct StyledBgRowLogic {
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for StyledBgRowLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let icon_button = |id: &str, label: &str| {
                let mut button = Button::new(WidgetId::new(id), label);
                button.variant = quadraui::ButtonVariant::Icon;
                LayoutNode::Leaf(LayoutLeaf {
                    id: id.to_string(),
                    descriptor: LeafDescriptor::Button(button),
                    scroll_px: None,
                })
            };
            let row = LayoutNode::Row(LayoutContainer::new(
                vec![
                    icon_button("a", "Alpha"),
                    LayoutNode::Styled(crate::layout::LayoutStyled {
                        style: Box::new(StyleOverride {
                            bg: Some(quadraui::Color::rgb(0, 255, 0)),
                            ..StyleOverride::default()
                        }),
                        node: Box::new(icon_button("b", "Bravo")),
                    }),
                    icon_button("c", "Charlie"),
                ],
                Spacing::default(),
            ));
            let node = LayoutNode::Styled(crate::layout::LayoutStyled {
                style: Box::new(StyleOverride {
                    bg: Some(quadraui::Color::rgb(255, 0, 0)),
                    ..StyleOverride::default()
                }),
                node: Box::new(row),
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

    #[test]
    fn styled_wrapper_scopes_a_bg_override_to_its_own_subtree_and_composes_with_nesting() {
        use quadraui::tui::testing::TuiDriver;

        let logic = StyledBgRowLogic {
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 42, 3);
        driver.render();

        let (ax, ay) = driver.find("Alpha").expect("Alpha should paint");
        let (bx, by) = driver.find("Bravo").expect("Bravo should paint");
        let (cx, cy) = driver.find("Charlie").expect("Charlie should paint");

        let alpha_bg = driver
            .style_at(ax as u16, ay as u16)
            .expect("style at Alpha")
            .bg;
        let bravo_bg = driver
            .style_at(bx as u16, by as u16)
            .expect("style at Bravo")
            .bg;
        let charlie_bg = driver
            .style_at(cx as u16, cy as u16)
            .expect("style at Charlie")
            .bg;

        let red = quadraui::tui::ratatui_color(quadraui::Color::rgb(255, 0, 0));
        let green = quadraui::tui::ratatui_color(quadraui::Color::rgb(0, 255, 0));
        assert_eq!(
            alpha_bg, red,
            "Alpha has no override of its own, so it inherits the outer row's"
        );
        assert_eq!(
            bravo_bg, green,
            "Bravo's own nested override wins over the outer row's"
        );
        assert_eq!(
            charlie_bg, red,
            "Charlie (Bravo's sibling) is unaffected by Bravo's own override, \
             and still inherits the outer row's"
        );
    }

    /// The headline case the `"style"` spec page calls out: a plain `bg`
    /// override does **not** reach a `Button`'s own fill on any variant
    /// but `Icon` (see the test above) -- `Secondary`/`Danger`/`Warning`/
    /// disabled read `Theme::surface_bg`, `Primary` reads
    /// `Theme::accent_bg`, never `Theme::background`. `surface_bg`/
    /// `accent_bg` are the aliases that actually reach it.
    #[test]
    fn surface_bg_and_accent_bg_are_the_aliases_a_button_fill_actually_reads() {
        use quadraui::tui::testing::TuiDriver;

        struct Logic {
            engine: crate::engine::InteractionEngine,
        }
        impl quadraui::AppLogic for Logic {
            type AreaId = ();
            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                // `Button::new` defaults to `ButtonVariant::Primary`,
                // which TUI's `tui::button` fills with `Theme::accent_bg`.
                let primary = |id: &str, label: &str| {
                    LayoutNode::Leaf(LayoutLeaf {
                        id: id.to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(WidgetId::new(id), label)),
                        scroll_px: None,
                    })
                };
                let row = LayoutNode::Row(LayoutContainer::new(
                    vec![
                        primary("unstyled", "Unstyled"),
                        LayoutNode::Styled(crate::layout::LayoutStyled {
                            style: Box::new(StyleOverride {
                                bg: Some(quadraui::Color::rgb(0, 255, 0)),
                                ..StyleOverride::default()
                            }),
                            node: Box::new(primary("bg", "BgOnly")),
                        }),
                        LayoutNode::Styled(crate::layout::LayoutStyled {
                            style: Box::new(StyleOverride {
                                accent_bg: Some(quadraui::Color::rgb(0, 255, 0)),
                                ..StyleOverride::default()
                            }),
                            node: Box::new(primary("accent_bg", "AccentBg")),
                        }),
                    ],
                    Spacing::default(),
                ));
                let mut hits = Vec::new();
                paint_layout(backend, area, &row, &mut hits, &|id| {
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

        let logic = Logic {
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 60, 3);
        driver.render();

        let (ux, uy) = driver.find("Unstyled").expect("Unstyled should paint");
        let (bx, by) = driver.find("BgOnly").expect("BgOnly should paint");
        let (acx, acy) = driver.find("AccentBg").expect("AccentBg should paint");

        let unstyled_bg = driver.style_at(ux as u16, uy as u16).expect("style").bg;
        let bg_only_bg = driver.style_at(bx as u16, by as u16).expect("style").bg;
        let accent_bg_bg = driver.style_at(acx as u16, acy as u16).expect("style").bg;

        let green = quadraui::tui::ratatui_color(quadraui::Color::rgb(0, 255, 0));
        assert_eq!(
            bg_only_bg, unstyled_bg,
            "a plain \"bg\" override is a no-op on a Primary button's fill -- \
             it reads Theme::accent_bg, not Theme::background"
        );
        assert_eq!(
            accent_bg_bg, green,
            "\"accent_bg\" is the alias that actually reaches a Primary button's fill"
        );
    }

    /// End to end for the `"class"` key: a tree carrying class names,
    /// decoded against a [`crate::layout::StyleSets`] registry and painted
    /// through [`paint_layout`], reaches the same cells an inline
    /// `"style"` would -- with the registry's own precedence (later class
    /// wins, inline `"style"` wins over every class, unknown name
    /// ignored) visible in the painted colours rather than only in the
    /// decoded [`StyleOverride`].
    #[test]
    fn class_names_resolve_through_the_registry_into_the_painted_cells() {
        use quadraui::tui::testing::TuiDriver;

        struct Logic {
            engine: crate::engine::InteractionEngine,
            sets: crate::layout::StyleSets,
        }
        impl quadraui::AppLogic for Logic {
            type AreaId = ();
            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let value = serde_json::json!({
                    "row": [
                        {"id": "plain", "button": {"id": "plain", "label": "Plain"}},
                        {
                            "id": "one",
                            "button": {"id": "one", "label": "OneClass"},
                            "class": "brand",
                        },
                        {
                            "id": "two",
                            "button": {"id": "two", "label": "TwoClass"},
                            "class": ["brand", "alert"],
                        },
                        {
                            "id": "inline",
                            "button": {"id": "inline", "label": "InlineWins"},
                            "class": "brand",
                            "style": {"accent_bg": "#0000ff"},
                        },
                        {
                            "id": "unknown",
                            "button": {"id": "unknown", "label": "Unknown"},
                            "class": "no-such-set",
                        },
                    ],
                });
                let theme = backend.theme();
                let node = crate::wire::decode_layout_node_with_styles(
                    "test",
                    &value,
                    crate::wire::StyleContext::new(&self.sets, &theme),
                    &crate::wire::DecodeDiagnostics::warn(),
                )
                .expect("valid tree");
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

        let mut sets = crate::layout::StyleSets::new();
        sets.define(
            "brand",
            StyleOverride {
                accent_bg: Some(quadraui::Color::rgb(0, 255, 0)),
                ..StyleOverride::default()
            },
        );
        sets.define(
            "alert",
            StyleOverride {
                accent_bg: Some(quadraui::Color::rgb(255, 0, 0)),
                ..StyleOverride::default()
            },
        );
        let logic = Logic {
            engine: crate::engine::InteractionEngine::new(),
            sets,
        };
        let mut driver = TuiDriver::new(logic, 80, 3);
        driver.render();

        let bg_at = |driver: &mut TuiDriver<Logic>, label: &str| {
            let (x, y) = driver
                .find(label)
                .unwrap_or_else(|| panic!("{label} should paint"));
            driver
                .style_at(x as u16, y as u16)
                .unwrap_or_else(|| panic!("style at {label}"))
                .bg
        };

        let plain = bg_at(&mut driver, "Plain");
        let green = quadraui::tui::ratatui_color(quadraui::Color::rgb(0, 255, 0));
        let red = quadraui::tui::ratatui_color(quadraui::Color::rgb(255, 0, 0));
        let blue = quadraui::tui::ratatui_color(quadraui::Color::rgb(0, 0, 255));
        assert_eq!(
            bg_at(&mut driver, "OneClass"),
            green,
            "one class resolves to that set's own tokens"
        );
        assert_eq!(
            bg_at(&mut driver, "TwoClass"),
            red,
            "the later class in the list wins a token both name"
        );
        assert_eq!(
            bg_at(&mut driver, "InlineWins"),
            blue,
            "the node's own inline \"style\" beats every class it names"
        );
        assert_eq!(
            bg_at(&mut driver, "Unknown"),
            plain,
            "an unknown class name resolves to nothing, leaving the node unstyled"
        );
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
            wrap: false,
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

    /// A `"style": {"padding": ...}` override on a `Content`-sized
    /// `Button` leaf changes that leaf's own measured rect, not just how
    /// it later paints — the `padding` override [`StyleOverride`]'s own
    /// doc describes under "Metric overrides feed layout".
    #[test]
    fn a_styled_padding_override_widens_a_content_sized_leaf_and_narrows_its_sibling() {
        let width = 80.0_f32;
        let area = Rect::new(0.0, 0.0, width, 10.0);
        let baseline = layout_container_rects(
            &mut tui(),
            area,
            &row(
                vec![
                    leaf_child("abcd", SectionSize::Content, None, None),
                    leaf_child("rest", SectionSize::EqualShare, None, None),
                ],
                Justify::Start,
                Align::Stretch,
            ),
            true,
        );
        assert_eq!(baseline[0].width, 8.0);

        // Doubling `Style::default().padding` (`8.0` -> `16.0`) adds one
        // whole cell of chrome on each side on `TuiBackend`
        // (`ceil(16/8) - ceil(8/8) = 1`), widening the leaf by 2 cells
        // total and narrowing its `EqualShare` sibling by the same
        // amount — the row's own full width never changes.
        let unstyled_leaf = leaf_child("abcd", SectionSize::Content, None, None);
        let styled_first = LayoutChild {
            node: LayoutNode::Styled(LayoutStyled {
                style: Box::new(crate::layout::StyleOverride {
                    padding: Some(16.0),
                    ..Default::default()
                }),
                node: Box::new(unstyled_leaf.node),
            }),
            size: unstyled_leaf.size,
            min: unstyled_leaf.min,
            max: unstyled_leaf.max,
        };
        let styled = row(
            vec![
                styled_first,
                leaf_child("rest", SectionSize::EqualShare, None, None),
            ],
            Justify::Start,
            Align::Stretch,
        );
        let overridden = layout_container_rects(&mut tui(), area, &styled, true);
        assert_eq!(overridden[0].width, 10.0);
        assert_eq!(overridden[1].width, width - 10.0);
        assert_eq!(overridden[0].height, baseline[0].height);
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
            wrap: false,
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

    /// A `wrap: true` row flows its children onto a second line once a
    /// third `SectionSize::Fixed(80)` (10 cells on `TuiBackend`) child no
    /// longer fits beside the first two inside a 25-cell-wide area, then
    /// unwraps back onto a single line once the area widens to fit all
    /// three — the headless acceptance case this feature exists for.
    #[test]
    fn wrap_flows_overflowing_children_onto_a_new_line_and_unwraps_when_widened() {
        let children = || {
            vec![
                leaf_child("a", SectionSize::Fixed(80), None, None),
                leaf_child("b", SectionSize::Fixed(80), None, None),
                leaf_child("c", SectionSize::Fixed(80), None, None),
            ]
        };
        let wrapping = LayoutContainer {
            wrap: true,
            ..row(children(), Justify::Start, Align::Stretch)
        };

        // 25 cells: the first two 10-cell children fit on one line, the
        // third does not and wraps onto a second line.
        let narrow = Rect::new(0.0, 0.0, 25.0, 10.0);
        let rects = layout_container_rects(&mut tui(), narrow, &wrapping, true);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].y, 0.0);
        assert_eq!(rects[1].y, 0.0);
        assert_eq!(rects[2].y, 1.0);
        assert_eq!(rects[0].x, 0.0);
        assert_eq!(rects[1].x, 10.0);
        assert_eq!(rects[2].x, 0.0);
        for rect in &rects {
            assert_eq!(rect.width, 10.0);
        }

        // Widened to 30 cells: all three now fit on a single line.
        let wide = Rect::new(0.0, 0.0, 30.0, 10.0);
        let rects = layout_container_rects(&mut tui(), wide, &wrapping, true);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].y, 0.0);
        assert_eq!(rects[1].y, 0.0);
        assert_eq!(rects[2].y, 0.0);
        assert_eq!(rects[0].x, 0.0);
        assert_eq!(rects[1].x, 10.0);
        assert_eq!(rects[2].x, 20.0);
    }

    /// The distance between a wrapped row's lines is a *cross-axis* (here,
    /// vertical) distance, so it is quantised by `quantise_vertical`
    /// against `line_height`, not `quantise_horizontal` against
    /// `char_width` the way the gap *within* each line is — `"gap": 16`
    /// quantises to `2` cells horizontally (`ceil(16 / 8)`) but only `1`
    /// cell vertically (`ceil(16 / 16)`) on [`TuiBackend`], so the two
    /// gaps in this test must differ.
    #[test]
    fn wrap_line_gap_is_quantised_by_the_cross_axis_rule_not_the_main_axis_one() {
        let children = vec![
            leaf_child("a", SectionSize::Fixed(80), None, None),
            leaf_child("b", SectionSize::Fixed(80), None, None),
            leaf_child("c", SectionSize::Fixed(80), None, None),
        ];
        let wrapping = LayoutContainer {
            wrap: true,
            spacing: Spacing {
                gap: 16.0,
                ..Spacing::default()
            },
            ..row(children, Justify::Start, Align::Stretch)
        };

        // 25 cells: `a`/`b` are 10 cells each plus a 2-cell horizontal
        // gap (22 total) fit; a third 10-cell child would need 34 and
        // wraps onto a second line.
        let area = Rect::new(0.0, 0.0, 25.0, 10.0);
        let rects = layout_container_rects(&mut tui(), area, &wrapping, true);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].x, 0.0);
        assert_eq!(rects[1].x, 12.0, "main-axis gap quantises to 2 cells");
        assert_eq!(rects[0].y, 0.0);
        assert_eq!(rects[1].y, 0.0);
        // The first line is 1 cell tall; the second line starts after it
        // plus a 1-cell *cross*-axis gap, not the 2-cell main-axis one.
        assert_eq!(
            rects[2].y, 2.0,
            "cross-axis line gap quantises to 1 cell, not the main-axis rule's 2"
        );
    }

    /// `wrap: true` on a `Column` still wraps along its own main axis
    /// (vertical), but the resulting lines stack along the *cross* axis —
    /// horizontally, since a `Column`'s cross axis is its width. Two
    /// one-cell-tall `Fixed(16)` children inside a one-cell-tall area
    /// cannot share a line, so the second lands in a new line placed
    /// beside (not below) the first.
    #[test]
    fn wrap_on_a_column_stacks_new_lines_along_its_horizontal_cross_axis() {
        let children = vec![
            leaf_child("a", SectionSize::Fixed(16), None, None),
            leaf_child("b", SectionSize::Fixed(16), None, None),
        ];
        let wrapping = LayoutContainer {
            wrap: true,
            ..row(children, Justify::Start, Align::Stretch)
        };
        let narrow = Rect::new(0.0, 0.0, 10.0, 1.0);
        let rects = layout_container_rects(&mut tui(), narrow, &wrapping, false);
        // Both start at the top of their own line (`y`)...
        assert_eq!(rects[0].y, 0.0);
        assert_eq!(rects[1].y, 0.0);
        // ...but the second's line is offset along `x`, not stacked
        // underneath the first.
        assert_eq!(rects[0].x, 0.0);
        assert!(rects[1].x > rects[0].x);
    }

    /// Every one of the additive (`LeafDescriptor::Extra`) leaf kinds
    /// overriding `ExtraPaint::natural_size` sizes to its own content
    /// under a non-`Stretch` `Align`, exactly like a `Button`/
    /// `TextDisplay`/`TabBar` fixed leaf already does in
    /// [`align_every_value_at_two_terminal_sizes`] above — the behaviour
    /// this trait method exists to extend past the six fixed slots.
    /// `Align::Start`/`Center`/`End` each size the child to its reported
    /// natural height (well under the tall `area` below, so "non-
    /// stretched" is unambiguous) and position it at the top, middle,
    /// or bottom of the container respectively; only `Align::Stretch`
    /// (checked once up front) still fills the whole extent.
    #[test]
    fn align_sizes_every_extra_leaf_kind_to_its_own_natural_height() {
        use crate::extra_paint::{Checkbox, PasswordInput, Select, Slider, TextArea};
        use quadraui::{
            ComboBox, Image, ImageFit, ImageSource, ProgressBar, RadioGroup, RadioOption, Spinner,
            Switch, TextField, TextInput,
        };

        let kinds: Vec<(&str, Rc<dyn ExtraPaint>)> = vec![
            (
                "progress",
                Rc::new(ProgressBar {
                    id: WidgetId::new("progress"),
                    label: String::new(),
                    value: Some(0.5),
                    frame_idx: 0,
                    cancellable: false,
                    accent: None,
                }),
            ),
            (
                "spinner",
                Rc::new(Spinner {
                    id: WidgetId::new("spinner"),
                    label: String::new(),
                    frame_idx: 0,
                    accent: None,
                }),
            ),
            (
                "image",
                Rc::new(Image {
                    id: WidgetId::new("image"),
                    source: ImageSource::Path(std::path::PathBuf::new()),
                    intrinsic_size: None,
                    fit: ImageFit::Contain,
                    fallback_text: "img".to_string(),
                }),
            ),
            (
                "checkbox",
                Rc::new(Checkbox {
                    id: WidgetId::new("checkbox"),
                    value: false,
                    disabled: false,
                    focused: false,
                }),
            ),
            ("switch", Rc::new(Switch::new(WidgetId::new("switch")))),
            (
                "radio_group",
                Rc::new(RadioGroup::new(
                    WidgetId::new("radio_group"),
                    vec![
                        RadioOption::new(WidgetId::new("radio_group:a"), "A"),
                        RadioOption::new(WidgetId::new("radio_group:b"), "B"),
                    ],
                )),
            ),
            (
                "slider",
                Rc::new(Slider {
                    id: WidgetId::new("slider"),
                    value: 0.0,
                    min: 0.0,
                    max: 1.0,
                    step: 1.0,
                    disabled: false,
                    focused: false,
                }),
            ),
            (
                "select",
                Rc::new(Select {
                    id: WidgetId::new("select"),
                    options: vec!["a".to_string(), "b".to_string()],
                    selected_idx: 0,
                    disabled: false,
                    focused: false,
                }),
            ),
            (
                "combo_box",
                Rc::new(ComboBox::new(
                    WidgetId::new("combo_box"),
                    vec!["a".to_string(), "b".to_string()],
                )),
            ),
            (
                "text_input",
                Rc::new(TextInput::new(WidgetId::new("text_input"))),
            ),
            (
                "text_field",
                Rc::new(TextField::new(WidgetId::new("text_field"))),
            ),
            (
                "password",
                Rc::new(PasswordInput {
                    inner: TextInput::new(WidgetId::new("password")),
                    mask_char: '*',
                }),
            ),
            (
                "textarea",
                Rc::new(TextArea {
                    inner: TextInput::new(WidgetId::new("textarea")),
                    visible_rows: 3,
                }),
            ),
        ];
        assert!(
            kinds.len() >= 10,
            "this test's own point is coverage across at least 10 leaf kinds"
        );

        let height = 24.0_f32;
        let area = Rect::new(0.0, 0.0, 80.0, height);
        for (name, extra) in kinds {
            let child = || {
                vec![LayoutChild::new(LayoutNode::Leaf(LayoutLeaf {
                    id: name.to_string(),
                    descriptor: LeafDescriptor::Extra(extra.clone()),
                    scroll_px: None,
                }))]
            };

            let natural = extra.natural_size(&mut tui()).height.unwrap_or_else(|| {
                panic!("{name} should report a natural height for this test to be meaningful")
            });
            assert!(
                natural < height,
                "{name}'s natural height {natural} should be well under the container's \
                 {height} for Start/Center/End to visibly differ from Stretch"
            );

            let stretch = row(child(), Justify::Start, Align::Stretch);
            let rects = layout_container_rects(&mut tui(), area, &stretch, true);
            assert_eq!(
                rects[0].height, height,
                "{name}: Stretch should still fill the extent"
            );

            let start = row(child(), Justify::Start, Align::Start);
            let rects = layout_container_rects(&mut tui(), area, &start, true);
            assert_eq!(rects[0].y, 0.0, "{name}: Start should anchor at the top");
            assert_eq!(rects[0].height, natural, "{name}: Start should not stretch");

            let center = row(child(), Justify::Start, Align::Center);
            let rects = layout_container_rects(&mut tui(), area, &center, true);
            assert_eq!(
                rects[0].y,
                (height - natural) / 2.0,
                "{name}: Center should center on the cross axis"
            );
            assert_eq!(
                rects[0].height, natural,
                "{name}: Center should not stretch"
            );

            let end = row(child(), Justify::Start, Align::End);
            let rects = layout_container_rects(&mut tui(), area, &end, true);
            assert_eq!(
                rects[0].y,
                height - natural,
                "{name}: End should anchor at the bottom"
            );
            assert_eq!(rects[0].height, natural, "{name}: End should not stretch");
        }
    }

    /// [`Switch`]'s and [`RadioGroup`]'s cross-axis (width) natural size on
    /// a cell-grid backend has to match the cell count their own TUI
    /// rasteriser actually paints, not just [`Checkbox`]'s shape: a
    /// `Switch` under-reporting its `4`-cell `[● ]`/`[ ●]` track collapses
    /// `draw_switch`'s knob column so on/off paint identically, and a
    /// `RadioGroup` under-reporting its `"(x) label"` chrome by even one
    /// cell gets its last label character truncated by `draw_radio_group`
    /// under a non-`Stretch` `Column` align.
    #[test]
    fn switch_and_radio_group_natural_width_matches_what_tui_paints() {
        use quadraui::{RadioGroup, RadioOption, Switch};

        let mut backend = tui();
        let char_width = backend.char_width();

        let switch = Switch::new(WidgetId::new("switch"));
        let switch_width = switch
            .natural_size(&mut backend)
            .width
            .expect("Switch reports a natural width");
        assert_eq!(
            switch_width,
            4.0 * char_width,
            "Switch's natural width should match draw_switch's 4-cell track"
        );

        let group = RadioGroup::new(
            WidgetId::new("radio_group"),
            vec![
                RadioOption::new(WidgetId::new("radio_group:a"), "Ab"),
                RadioOption::new(WidgetId::new("radio_group:b"), "C"),
            ],
        );
        let group_width = group
            .natural_size(&mut backend)
            .width
            .expect("RadioGroup reports a natural width");
        // `draw_radio_group` paints `"({glyph}) {label}"`: `'('`, glyph,
        // `')'`, a gap, then the widest label -- 4 cells of chrome plus
        // the label's own width.
        assert_eq!(
            group_width,
            4.0 * char_width + 2.0 * char_width,
            "RadioGroup's natural width should fit its widest label without truncation"
        );
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

    /// A column of `count` buttons, labelled `"0"`..`"{count - 1}"` by
    /// index, each a one-line-tall `LayoutLeaf` — the shared content
    /// builder every `LayoutNode::Scroll` acceptance test below stacks.
    fn button_column(count: usize) -> LayoutNode {
        LayoutNode::Column(LayoutContainer::new(
            (0..count)
                .map(|i| {
                    let label = i.to_string();
                    LayoutNode::Leaf(LayoutLeaf {
                        id: label.clone(),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new(label.clone()),
                            label,
                        )),
                        scroll_px: None,
                    })
                })
                .collect(),
            Spacing::default(),
        ))
    }

    /// Minimal [`quadraui::AppLogic`] around a `LayoutNode::Scroll` hosting
    /// [`button_column`] — drives [`paint_layout`]'s `Scroll` arm through a
    /// real [`quadraui::tui::testing::TuiDriver`] frame, the issue's own
    /// acceptance scenario: a column of 40 buttons in a 10-row viewport, at
    /// a pinned (controlled) offset.
    struct ScrollButtonsLogic {
        button_count: usize,
        offset: f32,
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for ScrollButtonsLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: Some(self.offset),
                content: Box::new(button_column(self.button_count)),
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

    fn painted_button_ids(hits: &[PaintedHit]) -> Vec<String> {
        hits.iter()
            .filter_map(|hit| match hit {
                PaintedHit::Button { id, .. } => Some(id.as_str().to_string()),
                _ => None,
            })
            .collect()
    }

    /// The issue's own acceptance scenario: a column of 40 buttons in a
    /// 10-row viewport at offset 20 -- the button at the top of the
    /// viewport is id `"20"`, and nothing outside `[20, 30)` ever gets a
    /// [`PaintedHit`] at all (this container's whole mechanism for "no
    /// cell or pixel outside the viewport is painted" / "anything
    /// scrolled out of view gets no hits" -- see [`ScrollSlots`]'s own
    /// doc).
    #[test]
    fn scroll_at_offset_twenty_paints_exactly_the_ten_visible_buttons() {
        use quadraui::tui::testing::TuiDriver;

        let logic = ScrollButtonsLogic {
            button_count: 40,
            offset: 20.0,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let ids = painted_button_ids(&hits);
        let want: Vec<String> = (20..30).map(|i| i.to_string()).collect();
        assert_eq!(ids, want);
    }

    /// A click at the top-left cell of the viewport -- where button `"20"`
    /// is now painted -- hits id `"20"`, not whatever would have been
    /// there unscrolled (button `"0"`).
    #[test]
    fn scroll_at_offset_twenty_click_at_viewport_top_hits_button_twenty() {
        use quadraui::tui::testing::TuiDriver;

        let logic = ScrollButtonsLogic {
            button_count: 40,
            offset: 20.0,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let found = hit_test_painted_for_test(&hits, quadraui::Point { x: 0.0, y: 0.0 });
        assert_eq!(
            found,
            Some((WidgetId::new("20"), "button_body".to_string()))
        );
    }

    /// Thin wrapper around [`crate::hit::hit_test_painted`] that stringifies
    /// the matched [`crate::hit::HitDetail`] by its own serde `"kind"` tag,
    /// so this test module doesn't need to match every `HitDetail` variant
    /// by hand just to assert which one came back.
    fn hit_test_painted_for_test(
        hits: &[PaintedHit],
        point: quadraui::Point,
    ) -> Option<(WidgetId, String)> {
        crate::hit::hit_test_painted(hits, point).map(|(id, detail)| {
            let value = serde_json::to_value(&detail).expect("HitDetail serializes");
            let kind = value
                .get("kind")
                .and_then(|k| k.as_str())
                .expect("HitDetail has a \"kind\" tag")
                .to_string();
            (id, kind)
        })
    }

    /// Scrolled all the way to `max_offset` (`30`, since `40 - 10 = 30`):
    /// the last ten buttons (`30`..`39`) paint, and nothing past the clamp
    /// is reachable even if a caller pins an offset beyond it.
    #[test]
    fn scroll_offset_beyond_max_still_paints_exactly_the_last_ten_buttons() {
        use quadraui::tui::testing::TuiDriver;

        let logic = ScrollButtonsLogic {
            button_count: 40,
            // Pinned well past `max_offset()` (30.0) -- `ScrollView`'s own
            // clamp (not this module's) is what keeps this from painting
            // past the end.
            offset: 999.0,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let ids = painted_button_ids(&hits);
        let want: Vec<String> = (30..40).map(|i| i.to_string()).collect();
        assert_eq!(ids, want);
    }

    /// At `offset: 0.0` (nothing scrolled), the same column paints its
    /// first ten buttons -- the baseline this module's offset math is
    /// measured against.
    #[test]
    fn scroll_at_offset_zero_paints_the_first_ten_buttons() {
        use quadraui::tui::testing::TuiDriver;

        let logic = ScrollButtonsLogic {
            button_count: 40,
            offset: 0.0,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 10);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let ids = painted_button_ids(&hits);
        let want: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        assert_eq!(ids, want);
    }

    /// A two-line `TextDisplay`, labelled `id` — the shared multi-row
    /// (but non-reflowing: it always paints from its own line `0`,
    /// however many rows the rect it's given can fit) child the non-
    /// boundary scroll test below stacks, unlike [`button_column`]'s
    /// one-line-tall buttons.
    fn two_line_text_display(id: &str) -> LayoutNode {
        let mut td = TextDisplay::new(WidgetId::new(id));
        for text in ["a", "b"] {
            td.append_line(TextDisplayLine {
                spans: vec![StyledSpan::plain(text)],
                decoration: Decoration::default(),
                timestamp: None,
            });
        }
        td.auto_scroll = false;
        LayoutNode::Leaf(LayoutLeaf {
            id: id.to_string(),
            descriptor: LeafDescriptor::TextDisplay(td),
            scroll_px: None,
        })
    }

    fn text_display_column(count: usize) -> LayoutNode {
        LayoutNode::Column(LayoutContainer::new(
            (0..count)
                .map(|i| two_line_text_display(&i.to_string()))
                .collect(),
            Spacing::default(),
        ))
    }

    struct ScrollTextColumnLogic {
        child_count: usize,
        offset: f32,
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for ScrollTextColumnLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: Some(self.offset),
                content: Box::new(text_display_column(self.child_count)),
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

    fn painted_text_displays(hits: &[PaintedHit]) -> Vec<(String, Rect)> {
        hits.iter()
            .filter_map(|hit| match hit {
                PaintedHit::TextDisplay { id, area, .. } => Some((id.as_str().to_string(), *area)),
                _ => None,
            })
            .collect()
    }

    /// A column of five 2-line `TextDisplay`s (ids `"0"`..`"4"`) in a
    /// 5-row viewport at offset `3` — a non-boundary offset, since each
    /// child is 2 rows tall and `3` isn't a multiple of `2`. Child `"1"`
    /// (rows `[2, 4)` globally) straddles the viewport's own top edge
    /// (global row `3`): its painted `area` is cut to the one row still
    /// inside the viewport, not the full two, and doesn't overlap `"2"`'s
    /// own area painted right after it — `clip_main_axis`'s own
    /// `cell_grid` clamp-and-shrink, not just a plain clamp to the
    /// viewport's top edge, which would paint `"1"` at its full two-row
    /// height there and overlap `"2"`.
    #[test]
    fn scroll_multi_row_child_at_non_boundary_offset_is_cut_not_overlapping() {
        use quadraui::tui::testing::TuiDriver;

        let logic = ScrollTextColumnLogic {
            child_count: 5,
            offset: 3.0,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let painted = painted_text_displays(&hits);

        // `width` is 19, not the driver's own 20 -- `scroll_viewport_and_track`
        // reserves a one-cell scrollbar gutter on the trailing edge of a
        // vertical scroll's own cross axis (width) whenever there's
        // anything to scroll, which this 10-row content in a 5-row
        // viewport always has.
        let want = vec![
            ("1".to_string(), Rect::new(0.0, 0.0, 19.0, 1.0)),
            ("2".to_string(), Rect::new(0.0, 1.0, 19.0, 2.0)),
            ("3".to_string(), Rect::new(0.0, 3.0, 19.0, 2.0)),
        ];
        assert_eq!(painted, want);

        for i in 1..painted.len() {
            let prev = painted[i - 1].1;
            let next = painted[i].1;
            assert!(
                prev.y + prev.height <= next.y,
                "{:?} overlaps {:?}",
                prev,
                next
            );
        }
    }

    struct ScrollTextAtomicLogic {
        offset: f32,
        line_count: usize,
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for ScrollTextAtomicLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut td = TextDisplay::new(WidgetId::new("log"));
            for i in 0..self.line_count {
                td.append_line(TextDisplayLine {
                    spans: vec![StyledSpan::plain(format!("line {i}"))],
                    decoration: Decoration::default(),
                    timestamp: None,
                });
            }
            td.auto_scroll = false;
            let node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: Some(self.offset),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "log".to_string(),
                    descriptor: LeafDescriptor::TextDisplay(td),
                    scroll_px: None,
                })),
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

    /// A single 20-line `TextDisplay` AS the scroll's entire content (not
    /// wrapped in a `Row`/`Column` — `scroll_content_slots`'s `Atomic`
    /// branch) in a 5-row viewport at offset `7`: a cell-grid backend's
    /// `Atomic` leaf rect clamps a negative main-axis origin to `0`
    /// without shrinking (`q_rect_to_ratatui`'s own `.max(0.0)`), so
    /// shifting the rect by `-offset` the way a plain leaf is positioned
    /// would always repaint line `0` at the viewport's top regardless of
    /// `offset`. `TextDisplay`'s own backend contract
    /// (`quadraui::TextDisplay`'s doc: render `lines[scroll_offset..]`) is
    /// what lets the shadow clone `offset_windowed_leaf_scrolled_to_offset`
    /// builds show the right line instead.
    #[test]
    fn scroll_atomic_text_display_at_offset_shows_the_offset_line_first() {
        use quadraui::tui::testing::TuiDriver;

        let logic = ScrollTextAtomicLogic {
            offset: 7.0,
            line_count: 20,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let first_visible_line = hits.iter().find_map(|hit| match hit {
            PaintedHit::TextDisplay { layout, .. } => {
                layout.visible_lines.first().map(|l| l.line_idx)
            }
            _ => None,
        });
        assert_eq!(first_visible_line, Some(7));
    }

    /// [`ScrollTextAtomicLogic`]'s `List` counterpart: a 20-item `List` AS
    /// the scroll's entire content, in a 5-row viewport at offset `7` —
    /// the issue's own acceptance scenario for the kind it names
    /// explicitly. Mirrors [`scroll_atomic_text_display_at_offset_shows_the_offset_line_first`]
    /// for the same reason: `ListView`'s own backend contract renders
    /// `items[scroll_offset..]` regardless of where its rect sits on
    /// screen, so the shadow clone must show item `7` first, not item `0`.
    struct ScrollListAtomicLogic {
        offset: f32,
        item_count: usize,
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for ScrollListAtomicLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let list = ListView {
                id: WidgetId::new("list"),
                title: None,
                items: (0..self.item_count)
                    .map(|i| quadraui::ListItem {
                        text: quadraui::StyledText::plain(format!("item {i}")),
                        icon: None,
                        detail: None,
                        decoration: Decoration::default(),
                    })
                    .collect(),
                selected_idx: 0,
                scroll_offset: 0,
                has_focus: true,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: false,
            };
            let node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: Some(self.offset),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "list".to_string(),
                    descriptor: LeafDescriptor::List(list),
                    scroll_px: None,
                })),
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

    #[test]
    fn scroll_atomic_list_at_offset_shows_the_offset_item_first() {
        use quadraui::tui::testing::TuiDriver;

        let logic = ScrollListAtomicLogic {
            offset: 7.0,
            item_count: 20,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let painted_scroll_offset = hits.iter().find_map(|hit| match hit {
            PaintedHit::List { scroll_offset, .. } => Some(*scroll_offset),
            _ => None,
        });
        assert_eq!(painted_scroll_offset, Some(7));
        let first_visible_item = hits.iter().find_map(|hit| match hit {
            PaintedHit::List { layout, .. } => {
                layout.visible_items.first().map(|item| item.item_idx)
            }
            _ => None,
        });
        assert_eq!(first_visible_item, Some(7));
    }

    /// [`ScrollListAtomicLogic`]'s `Tree` counterpart — same scenario, a
    /// 20-row `Tree` AS the scroll's entire content, in a 5-row viewport
    /// at offset `7`.
    struct ScrollTreeAtomicLogic {
        offset: f32,
        row_count: usize,
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for ScrollTreeAtomicLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let tree = TreeView {
                id: WidgetId::new("tree"),
                rows: (0..self.row_count)
                    .map(|i| quadraui::TreeRow {
                        path: vec![i as u16],
                        indent: 0,
                        icon: None,
                        text: quadraui::StyledText::plain(format!("row {i}")),
                        badge: None,
                        is_expanded: None,
                        decoration: Decoration::default(),
                        edit: None,
                    })
                    .collect(),
                selection_mode: quadraui::SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: quadraui::TreeStyle::default(),
                has_focus: true,
            };
            let node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: Some(self.offset),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "tree".to_string(),
                    descriptor: LeafDescriptor::Tree(tree),
                    scroll_px: None,
                })),
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

    #[test]
    fn scroll_atomic_tree_at_offset_shows_the_offset_row_first() {
        use quadraui::tui::testing::TuiDriver;

        let logic = ScrollTreeAtomicLogic {
            offset: 7.0,
            row_count: 20,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let painted_scroll_offset = hits.iter().find_map(|hit| match hit {
            PaintedHit::Tree { scroll_offset, .. } => Some(*scroll_offset),
            _ => None,
        });
        assert_eq!(painted_scroll_offset, Some(7));
        let first_visible_row_idx = hits.iter().find_map(|hit| match hit {
            PaintedHit::Tree { layout, .. } => layout.visible_rows.first().map(|r| r.row_idx),
            _ => None,
        });
        assert_eq!(first_visible_row_idx, Some(7));
    }

    /// [`ScrollListAtomicLogic`]'s `DataTable` counterpart — same scenario,
    /// a 20-row `DataTable` AS the scroll's entire content, in a 5-row
    /// viewport at offset `7`.
    struct ScrollDataTableAtomicLogic {
        offset: f32,
        row_count: usize,
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for ScrollDataTableAtomicLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            use quadraui::{Column, ColumnWidth, DataRow};

            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let table = DataTable {
                id: WidgetId::new("table"),
                columns: vec![Column {
                    title: "col".to_string(),
                    width: ColumnWidth::FixedCells(10.0),
                    align: Default::default(),
                }],
                rows: (0..self.row_count)
                    .map(|i| DataRow {
                        cells: vec![quadraui::StyledText::plain(format!("row {i}"))],
                        decoration: Decoration::default(),
                    })
                    .collect(),
                selected_idx: None,
                scroll_offset: 0,
                sort: None,
                has_focus: true,
                show_scrollbar: false,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: vec![],
                footer: None,
                bordered: false,
            };
            let node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: Some(self.offset),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "table".to_string(),
                    descriptor: LeafDescriptor::DataTable(table),
                    scroll_px: None,
                })),
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

    #[test]
    fn scroll_atomic_data_table_at_offset_shows_the_offset_row_first() {
        use quadraui::tui::testing::TuiDriver;

        let logic = ScrollDataTableAtomicLogic {
            offset: 7.0,
            row_count: 20,
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();
        let hits = driver.app().engine.hit_map();
        let painted_scroll_offset = hits.iter().find_map(|hit| match hit {
            PaintedHit::DataTable { scroll_offset, .. } => Some(*scroll_offset),
            _ => None,
        });
        assert_eq!(painted_scroll_offset, Some(7));
    }

    /// A single-line `TextDisplay` child, labelled `id` — `natural_size`'s
    /// `TextDisplay` arm sizes it to `lines.len() as f32 * line_height`
    /// using the `line_height` it's PASSED (unlike its `Button`/`TabBar`
    /// arms, which measure through `Backend::measure_text` against
    /// whatever real backend is driving the call) — so under a faked
    /// pixel `ScrollMetrics` this is `1.0 * 16.0 = 16.0` regardless of
    /// `tui()`'s own real `line_height` of `1.0`.
    fn one_line_text_display(id: &str) -> LayoutNode {
        let mut td = TextDisplay::new(WidgetId::new(id));
        td.append_line(TextDisplayLine {
            spans: vec![StyledSpan::plain("x")],
            decoration: Decoration::default(),
            timestamp: None,
        });
        LayoutNode::Leaf(LayoutLeaf {
            id: id.to_string(),
            descriptor: LeafDescriptor::TextDisplay(td),
            scroll_px: None,
        })
    }

    /// A pixel backend's own metrics (`char_width`/`line_height` both
    /// `> 1.0`) keep [`scroll_content_slots`]'s unclipped, negative-origin
    /// rect for a `Children` slot — the contract
    /// [`crate::hit::clip_hits_to_viewport`]'s own pixel-metrics
    /// regression test (`hit::tests::clip_hits_to_viewport_keeps_a_local_frame_hits_translation_origin_unmoved`)
    /// relies on `scroll_content_slots` itself upholding. A `Column` of
    /// two 16px-tall children in a 50px viewport at `offset: 10.0`: the
    /// second child's rect keeps its real (not yet negative, but still
    /// un-clamped) `y`, `16.0 - 10.0 = 6.0`, computed the same way
    /// whether or not it happens to land inside `viewport` — a
    /// `cell_grid` backend's slot would instead clamp/shrink a span that
    /// does stick out past the viewport's own edge, which this one, at
    /// this particular offset, does not; `scroll_multi_row_child_at_non_boundary_offset_is_cut_not_overlapping`
    /// above already covers that clamp on the `cell_grid` side.
    #[test]
    fn scroll_content_slots_keeps_the_unclipped_rect_on_a_pixel_backend() {
        let viewport = Rect::new(0.0, 0.0, 100.0, 50.0);
        let metrics = ScrollMetrics {
            char_width: 8.0,
            line_height: 16.0,
        };
        let content = LayoutNode::Column(LayoutContainer::new(
            vec![one_line_text_display("a"), one_line_text_display("b")],
            Spacing::default(),
        ));
        let mut tui = tui();
        let content_len = natural_main_length(&mut tui, &content, ScrollAxis::Vertical, metrics);
        assert_eq!(content_len, 32.0);
        let slots = scroll_content_slots(
            &mut tui,
            viewport,
            ScrollAxis::Vertical,
            10.0,
            &content,
            content_len,
            metrics,
        );
        let ScrollSlots::Children(slots) = slots else {
            panic!("a Column matching the scroll's own axis yields Children slots");
        };
        assert_eq!(slots[1].rect, Rect::new(0.0, 6.0, 100.0, 16.0));
    }

    /// A `List` leaf's own `natural_size` never reports a width (only a
    /// row-count-driven height), so a `Row` matching a horizontal
    /// `Scroll`'s axis, holding two `SectionSize::Fixed` `List` children,
    /// must still measure `content_len` as the sum of their two declared
    /// `Fixed` widths — not `natural_main_length`'s one-unit placeholder
    /// for an unmeasurable leaf, which would collapse the whole row far
    /// enough below the viewport that nothing is left to scroll to. Two
    /// `Fixed(500)` `List`s plus a `20`-wide gap must report `1020.0`, well
    /// past the `100`-wide viewport this test's `Scroll` would sit inside.
    #[test]
    fn container_natural_main_length_honors_a_fixed_list_childs_own_width() {
        fn fixed_list_child(id: &str, width: u16) -> LayoutChild {
            LayoutChild {
                node: LayoutNode::Leaf(LayoutLeaf {
                    id: id.to_string(),
                    descriptor: LeafDescriptor::List(ListView {
                        id: WidgetId::new(id),
                        title: None,
                        items: vec![quadraui::ListItem {
                            text: quadraui::StyledText::plain("item"),
                            icon: None,
                            detail: None,
                            decoration: Decoration::default(),
                        }],
                        selected_idx: 0,
                        scroll_offset: 0,
                        has_focus: false,
                        bordered: false,
                        h_scroll: 0,
                        max_content_width: None,
                        show_v_scrollbar: false,
                    }),
                    scroll_px: None,
                }),
                size: SectionSize::Fixed(width),
                min: None,
                max: None,
            }
        }

        let metrics = ScrollMetrics {
            char_width: 8.0,
            line_height: 16.0,
        };
        let content = LayoutNode::Row(LayoutContainer {
            children: vec![
                fixed_list_child("left", 500),
                fixed_list_child("right", 500),
            ],
            spacing: Spacing {
                padding: EdgeInsets::default(),
                margin: 0.0,
                gap: 20.0,
            },
            justify: Justify::default(),
            align: Align::default(),
            wrap: false,
        });
        let mut tui = tui();
        let content_len = natural_main_length(&mut tui, &content, ScrollAxis::Horizontal, metrics);
        assert_eq!(content_len, 1020.0);

        let viewport = Rect::new(0.0, 0.0, 100.0, 20.0);
        let slots = scroll_content_slots(
            &mut tui,
            viewport,
            ScrollAxis::Horizontal,
            0.0,
            &content,
            content_len,
            metrics,
        );
        let ScrollSlots::Children(slots) = slots else {
            panic!("a Row matching the scroll's own axis yields Children slots");
        };
        assert_eq!(slots[0].len, 500.0);
        assert_eq!(slots[1].len, 500.0);
        assert_eq!(slots[1].start, 520.0);
    }
    /// Minimal [`quadraui::AppLogic`] that decodes a `"grid"` wire node
    /// dict and paints it -- the end-to-end path a Python/TypeScript
    /// render callback returning that dict actually takes
    /// (`decode_layout_node` -> [`paint_layout`]), rather than a
    /// hand-built [`LayoutNode::Grid`].
    struct GridWireLogic {
        engine: crate::engine::InteractionEngine,
    }

    impl quadraui::AppLogic for GridWireLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let value = serde_json::json!({
                "grid": {
                    "columns": [{"Weight": 1.0}, {"Weight": 1.0}],
                    "rows": [{"Weight": 1.0}, {"Weight": 1.0}],
                    "cells": [
                        {"id": "nw", "button": {"id": "nw", "label": "Northwest"}},
                        {"id": "ne", "button": {"id": "ne", "label": "Northeast"}},
                        {"id": "sw", "button": {"id": "sw", "label": "Southwest"}},
                        {"id": "se", "button": {"id": "se", "label": "Southeast"}},
                    ],
                }
            });
            let node = crate::wire::decode_layout_node("render", value)
                .expect("the grid dict should decode");
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

    /// A `"grid"` wire node's four cells tile into two equal columns and
    /// two equal rows: each cell's label lands in its own quadrant, the
    /// two rows share one column pitch, and the two columns share one row
    /// pitch. Asserted relatively (pitch equality and ordering), never
    /// against hardcoded cell coordinates, so it survives a viewport or
    /// button-chrome change while still failing on a track-math
    /// regression.
    #[test]
    fn a_grid_wire_node_tiles_its_cells_into_equal_columns_and_rows() {
        use quadraui::tui::testing::TuiDriver;

        let logic = GridWireLogic {
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 60, 10);
        driver.render();

        let nw = driver.find("Northwest").expect("Northwest should paint");
        let ne = driver.find("Northeast").expect("Northeast should paint");
        let sw = driver.find("Southwest").expect("Southwest should paint");
        let se = driver.find("Southeast").expect("Southeast should paint");

        assert!(
            nw.0 < ne.0,
            "the first cell of a row sits left of the second"
        );
        assert!(sw.0 < se.0, "and so does the second row's");
        assert!(nw.1 < sw.1, "the first row sits above the second");
        assert!(ne.1 < se.1, "for both columns");
        assert_eq!(
            ne.0 - nw.0,
            se.0 - sw.0,
            "both rows share the same column pitch -- equal `Weight` columns"
        );
        assert_eq!(
            sw.1 - nw.1,
            se.1 - ne.1,
            "both columns share the same row pitch -- equal `Weight` rows"
        );
    }

    // ── virtualised `items_window` decode + `rows_requested_for_window` ──

    /// [`items_window_from_value`]'s documented `None` paths -- mirrors
    /// `scroll_px_tests`'s own coverage of
    /// [`crate::scroll_px_from_value`]'s identical set of degrade-to-`None`
    /// cases for the sibling extension key.
    #[test]
    fn items_window_from_value_none_paths() {
        use serde_json::json;

        assert_eq!(
            items_window_from_value(&json!({"rows": []})),
            None,
            "key absent"
        );
        assert_eq!(
            items_window_from_value(&json!({"items_window": "not an object"})),
            None,
            "items_window present but not an object"
        );
        assert_eq!(
            items_window_from_value(&json!({"items_window": {"total": 10}})),
            None,
            "start missing"
        );
        assert_eq!(
            items_window_from_value(&json!({"items_window": {"start": 0}})),
            None,
            "total missing"
        );
        assert_eq!(
            items_window_from_value(&json!({"items_window": {"start": -1, "total": 10}})),
            None,
            "start is negative, not a non-negative integer"
        );
        assert_eq!(
            items_window_from_value(&json!({"items_window": {"start": 0, "total": 1.5}})),
            None,
            "total is not an integer"
        );
        assert_eq!(
            items_window_from_value(&json!([1, 2, 3])),
            None,
            "non-object value"
        );
        assert_eq!(
            items_window_from_value(&json!({"items_window": {"start": 0, "total": 10}})),
            Some(ItemsWindow {
                start: 0,
                total: 10
            }),
            "the happy path still decodes"
        );
    }

    /// The app-side state the windowed-leaf driver tests below share: the
    /// `start` of the slice the "app" currently has loaded, and every
    /// [`RowsRequestedEvent`] the engine has asked it for.
    ///
    /// Shared by `Rc` with [`WindowedRequestSink`], which is what actually
    /// answers a request — a real binding's own handler mutates its app
    /// state from inside `EventSink::dispatch_rows_requested` the same way,
    /// so the next frame renders the newly requested slice.
    #[derive(Clone)]
    struct WindowedHarness {
        window_start: Rc<Cell<usize>>,
        requests: Rc<RefCell<Vec<RowsRequestedEvent>>>,
        /// Every `UiEvent` the engine dispatched, so a test can assert on
        /// a click resolving to the right ABSOLUTE row of a windowed
        /// collection.
        dispatched: Rc<RefCell<Vec<quadraui::UiEvent>>>,
    }

    impl WindowedHarness {
        fn new(window_start: usize) -> Self {
            Self {
                window_start: Rc::new(Cell::new(window_start)),
                requests: Rc::new(RefCell::new(Vec::new())),
                dispatched: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn sink(&self) -> WindowedRequestSink {
            WindowedRequestSink {
                harness: self.clone(),
            }
        }

        /// The one request the engine made, panicking if it made none or
        /// more than one — "scrolling past the loaded slice emits exactly
        /// one `RowsRequested`" is the acceptance bar, so a duplicate is a
        /// failure, not noise.
        fn only_request(&self) -> RowsRequestedEvent {
            let requests = self.requests.borrow();
            assert_eq!(
                requests.len(),
                1,
                "a scroll past the loaded slice must emit exactly one \
                 RowsRequested: {requests:?}"
            );
            requests[0].clone()
        }
    }

    /// Stands in for a binding's own event callback: records every
    /// `UiEvent`, and answers a [`RowsRequestedEvent`] by moving the app's
    /// loaded window to the rows the engine asked for.
    struct WindowedRequestSink {
        harness: WindowedHarness,
    }

    impl crate::engine::EventSink for WindowedRequestSink {
        fn dispatch(
            &mut self,
            event: &quadraui::UiEvent,
            _hit_detail: Option<&crate::hit::HitDetail>,
        ) -> quadraui::Reaction {
            self.harness.dispatched.borrow_mut().push(event.clone());
            quadraui::Reaction::Continue
        }

        fn dispatch_rows_requested(&mut self, event: RowsRequestedEvent) -> quadraui::Reaction {
            self.harness.window_start.set(event.start);
            self.harness.requests.borrow_mut().push(event);
            quadraui::Reaction::Redraw
        }
    }

    /// A `data_table` leaf whose `items_window` says its 50 `rows` are only
    /// a slice of a 100,000-row total: [`paint_layout_windowed`] threads the
    /// real total through to the leaf's own hit, so the scrollbar extent
    /// ([`PaintedHit::DataTable::total_rows`]) reflects `100_000`, not the
    /// 50 rows this frame's payload actually carries. A real keyboard
    /// scroll (`End`, dispatched through the exact same
    /// `InteractionEngine::handle`/`crate::host::resolve_layout_node` path
    /// a binding's own render loop drives — unmodified production code,
    /// not a hand-poked field) that lands past the loaded slice produces
    /// exactly one [`rows_requested_for_window`] request, and once the
    /// harness answers it with a freshly windowed payload, the row
    /// actually scrolled to renders its own absolute content.
    #[test]
    fn windowed_data_table_scrollbar_extent_and_real_scroll_request_more_rows() {
        use quadraui::tui::testing::TuiDriver;
        use quadraui::{Key, Modifiers, NamedKey};

        const TOTAL: usize = 100_000;
        const WINDOW_LEN: usize = 50;
        const WINDOW_START: usize = 35;

        fn leaf_value(window_start: usize) -> serde_json::Value {
            // Never claims more rows than actually remain in the
            // collection — a window near the very end of `TOTAL` sends a
            // shorter-than-`WINDOW_LEN` tail, matching what a real app's
            // own data source would hand back.
            let len = WINDOW_LEN.min(TOTAL.saturating_sub(window_start));
            let rows: Vec<_> = (0..len)
                .map(|i| {
                    let absolute = window_start + i;
                    serde_json::json!({"cells": [{"spans": [{"text": format!("row-{absolute}")}]}]})
                })
                .collect();
            serde_json::json!({
                "id": "table",
                "data_table": {
                    "id": "table",
                    "columns": [{"title": "col"}],
                    "rows": rows,
                    "items_window": {"start": window_start, "total": TOTAL},
                },
            })
        }

        fn decode_table(value: &serde_json::Value) -> DataTable {
            let node = crate::wire::decode_layout_node("test", value)
                .expect("a data_table leaf with items_window must decode");
            let LayoutNode::Leaf(leaf) = node else {
                panic!("expected a leaf node");
            };
            let LeafDescriptor::DataTable(table) = leaf.descriptor else {
                panic!("expected a DataTable descriptor");
            };
            table
        }

        fn row_text(table: &DataTable, row: usize) -> &str {
            &table.rows[row].cells[0].spans[0].text
        }

        let value = leaf_value(WINDOW_START);
        let table = decode_table(&value);
        assert_eq!(
            table.rows.len(),
            WINDOW_LEN,
            "the decoded payload must stay bounded to the loaded window, \
             never the full 100k-row total"
        );
        assert_eq!(
            row_text(&table, 0),
            format!("row-{WINDOW_START}"),
            "the first loaded row is the window's own start, not row 0"
        );

        struct WindowedTableLogic {
            harness: WindowedHarness,
            engine: crate::engine::InteractionEngine,
        }

        impl quadraui::AppLogic for WindowedTableLogic {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let value = leaf_value(self.harness.window_start.get());
                let diagnostics = crate::wire::DecodeDiagnostics::warn();
                let sets = crate::layout::StyleSets::new();
                let theme = Theme::default();
                let mut node = crate::wire::decode_layout_node_with_styles(
                    "test",
                    &value,
                    crate::wire::StyleContext::new(&sets, &theme),
                    &diagnostics,
                )
                .expect("a data_table leaf with items_window must decode");
                // Resolves the uncontrolled cursor/scroll this frame's
                // keyboard navigation already moved in `self.engine`'s own
                // `WidgetState` -- the same call a binding's render loop
                // makes before `paint_layout_windowed`, unmodified by
                // windowing.
                crate::host::resolve_layout_node(
                    self.engine.widget_state(),
                    backend,
                    area,
                    &mut node,
                );
                let windows = diagnostics.items_windows();
                let mut hits = Vec::new();
                paint_layout_windowed(
                    backend,
                    area,
                    &node,
                    &mut hits,
                    &|id| self.engine.chrome_interaction(id),
                    &windows,
                );
                self.engine.set_hits(hits);
            }

            fn handle(
                &mut self,
                event: quadraui::UiEvent,
                backend: &mut dyn Backend,
            ) -> quadraui::Reaction {
                self.engine.handle(event, backend, &mut self.harness.sink())
            }
        }

        fn painted_data_table(driver: &TuiDriver<WindowedTableLogic>) -> (usize, usize, usize) {
            driver
                .app()
                .engine
                .hit_map()
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::DataTable {
                        scroll_offset,
                        total_rows,
                        viewport_rows,
                        ..
                    } => Some((*scroll_offset, *total_rows, *viewport_rows)),
                    _ => None,
                })
                .expect("the data_table should have painted a hit")
        }

        let harness = WindowedHarness::new(WINDOW_START);
        let logic = WindowedTableLogic {
            harness: harness.clone(),
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();

        let (_, total_rows, viewport_rows) = painted_data_table(&driver);
        assert_eq!(
            total_rows, TOTAL,
            "the scrollbar extent must reflect the full collection, not the \
             50 loaded rows"
        );
        assert!(viewport_rows > 0, "a 5-row area must fit at least one row");

        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("table"))),
                "the data table is a tab stop and must accept focus"
            );
        }

        // Real scroll input: `End` jumps the uncontrolled cursor straight to
        // the bottom of the full `TOTAL`-row collection.
        // `crate::host::apply_data_table_nav_key` (unmodified production
        // code, reached through `InteractionEngine::handle`) clamps against
        // `total_rows` read straight off this frame's own
        // `PaintedHit::DataTable` -- `TOTAL`, not the 50 loaded rows.
        driver.dispatch(quadraui::UiEvent::KeyPressed {
            key: Key::Named(NamedKey::End),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        driver.render();

        let (scroll_offset, total_rows, viewport_rows) = painted_data_table(&driver);
        assert_eq!(total_rows, TOTAL);
        assert!(
            scroll_offset + viewport_rows >= TOTAL,
            "End must scroll all the way to the bottom of the full {TOTAL}-row \
             collection, not just the loaded window: scroll_offset={scroll_offset} \
             viewport_rows={viewport_rows}"
        );

        // The ENGINE asked the app for the rows that scroll brought into
        // view, through `EventSink::dispatch_rows_requested` -- nothing in
        // this test computes the request itself.
        let request = harness.only_request();
        assert_eq!(request.id, WidgetId::new("table"));
        assert!(
            request.count < WINDOW_LEN * 10,
            "a bounded request must stay a small multiple of the viewport, \
             nowhere near the widget's own {TOTAL}-row total -- got {}",
            request.count
        );
        assert!(
            request.start + request.count >= TOTAL,
            "the requested window must reach the very end of the collection"
        );

        // End-to-end: the harness answered the request from inside
        // `dispatch_rows_requested` (the way a real app's handler does),
        // so the next frame renders the right absolute content at the row
        // actually scrolled to.
        assert_eq!(
            harness.window_start.get(),
            request.start,
            "answering the request moves the app's own loaded window"
        );
        driver.render();

        let (scroll_offset_after, total_rows_after, _) = painted_data_table(&driver);
        assert_eq!(total_rows_after, TOTAL);
        let new_table = decode_table(&leaf_value(request.start));
        assert_eq!(
            row_text(&new_table, scroll_offset_after - request.start),
            format!("row-{scroll_offset_after}"),
            "the row actually scrolled to must render its own absolute content \
             after the window update, not stale or wrong-offset content"
        );
        assert!(
            driver.screen_contains(&format!("row-{scroll_offset_after}")),
            "the requested rows must actually paint at the scrolled-to \
             position:\n{}",
            driver.screen()
        );

        // Re-dispatching the same key at the same position asks for
        // nothing further: the answered window now covers the viewport.
        driver.dispatch(quadraui::UiEvent::KeyPressed {
            key: Key::Named(NamedKey::End),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert_eq!(
            harness.requests.borrow().len(),
            1,
            "a covered viewport must never re-request a slice it already has"
        );
    }

    /// [`windowed_data_table_scrollbar_extent_and_real_scroll_request_more_rows`],
    /// for a `list` leaf instead of a `data_table` one — the same windowed
    /// paint, scrollbar extent, and real-`End`-key `RowsRequested` round
    /// trip, driven through `crate::host::apply_list_nav_key` (unmodified
    /// production code) instead of `apply_data_table_nav_key`.
    #[test]
    fn windowed_list_scrollbar_extent_and_real_scroll_request_more_rows() {
        use quadraui::tui::testing::TuiDriver;
        use quadraui::{Key, Modifiers, NamedKey};

        const TOTAL: usize = 100_000;
        const WINDOW_LEN: usize = 50;
        const WINDOW_START: usize = 35;

        fn leaf_value(window_start: usize) -> serde_json::Value {
            let len = WINDOW_LEN.min(TOTAL.saturating_sub(window_start));
            let items: Vec<_> = (0..len)
                .map(|i| {
                    let absolute = window_start + i;
                    serde_json::json!({"text": {"spans": [{"text": format!("row-{absolute}")}]}})
                })
                .collect();
            serde_json::json!({
                "id": "list",
                "list": {
                    "id": "list",
                    "items": items,
                    "selected_idx": 0,
                    "items_window": {"start": window_start, "total": TOTAL},
                },
            })
        }

        struct WindowedListLogic {
            harness: WindowedHarness,
            engine: crate::engine::InteractionEngine,
        }

        impl quadraui::AppLogic for WindowedListLogic {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let value = leaf_value(self.harness.window_start.get());
                let diagnostics = crate::wire::DecodeDiagnostics::warn();
                let sets = crate::layout::StyleSets::new();
                let theme = Theme::default();
                let mut node = crate::wire::decode_layout_node_with_styles(
                    "test",
                    &value,
                    crate::wire::StyleContext::new(&sets, &theme),
                    &diagnostics,
                )
                .expect("a list leaf with items_window must decode");
                crate::host::resolve_layout_node(
                    self.engine.widget_state(),
                    backend,
                    area,
                    &mut node,
                );
                let windows = diagnostics.items_windows();
                let mut hits = Vec::new();
                paint_layout_windowed(
                    backend,
                    area,
                    &node,
                    &mut hits,
                    &|id| self.engine.chrome_interaction(id),
                    &windows,
                );
                self.engine.set_hits(hits);
            }

            fn handle(
                &mut self,
                event: quadraui::UiEvent,
                backend: &mut dyn Backend,
            ) -> quadraui::Reaction {
                self.engine.handle(event, backend, &mut self.harness.sink())
            }
        }

        fn painted_list(driver: &TuiDriver<WindowedListLogic>) -> (usize, usize, usize) {
            driver
                .app()
                .engine
                .hit_map()
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::List {
                        scroll_offset,
                        total_items,
                        viewport_rows,
                        ..
                    } => Some((*scroll_offset, *total_items, *viewport_rows)),
                    _ => None,
                })
                .expect("the list should have painted a hit")
        }

        let harness = WindowedHarness::new(WINDOW_START);
        let logic = WindowedListLogic {
            harness: harness.clone(),
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();

        let (_, total_items, viewport_rows) = painted_list(&driver);
        assert_eq!(
            total_items, TOTAL,
            "the scrollbar extent must reflect the full collection, not the \
             50 loaded items"
        );
        assert!(viewport_rows > 0, "a 5-row area must fit at least one item");

        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("list"))),
                "the list is a tab stop and must accept focus"
            );
        }

        driver.dispatch(quadraui::UiEvent::KeyPressed {
            key: Key::Named(NamedKey::End),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        driver.render();

        let (scroll_offset, total_items, viewport_rows) = painted_list(&driver);
        assert_eq!(total_items, TOTAL);
        assert!(
            scroll_offset + viewport_rows >= TOTAL,
            "End must scroll all the way to the bottom of the full {TOTAL}-item \
             collection, not just the loaded window"
        );

        // The ENGINE asked the app for the items that scroll brought into
        // view -- see the `data_table` test above.
        let request = harness.only_request();
        assert_eq!(request.id, WidgetId::new("list"));
        assert!(
            request.start + request.count >= TOTAL,
            "the requested window must reach the very end of the collection"
        );
        assert_eq!(harness.window_start.get(), request.start);

        driver.render();

        let (scroll_offset_after, total_items_after, _) = painted_list(&driver);
        assert_eq!(total_items_after, TOTAL);
        assert!(
            driver.screen_contains(&format!("row-{scroll_offset_after}")),
            "the requested items must actually paint at the scrolled-to \
             position:\n{}",
            driver.screen()
        );

        // A click on a visible item resolves to its ABSOLUTE index -- the
        // `ListViewLayout` indices the backend reports against the painted
        // band are shifted back into the collection's own indexing before
        // they reach the hit.
        let (click_point, clicked_idx) = {
            let hit_map = driver.app().engine.hit_map();
            let (area, layout) = hit_map
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::List { area, layout, .. } => Some((*area, layout.clone())),
                    _ => None,
                })
                .expect("the list should have painted a hit");
            let item = layout
                .visible_items
                .first()
                .expect("a scrolled-to list paints at least one item");
            assert_eq!(
                item.item_idx, scroll_offset_after,
                "the first visible item's index must be the ABSOLUTE item \
                 painted at the top of the viewport"
            );
            (
                (area.x + item.bounds.x + 1.0, area.y + item.bounds.y + 0.5),
                item.item_idx,
            )
        };
        harness.dispatched.borrow_mut().clear();
        driver.click(click_point.0, click_point.1);
        assert!(
            harness.dispatched.borrow().iter().any(|event| matches!(
                event,
                quadraui::UiEvent::List(id, quadraui::ListViewEvent::SelectionChanged { idx })
                    if *id == WidgetId::new("list") && *idx == clicked_idx
            )),
            "clicking the top visible item of a windowed list must resolve \
             to absolute item {clicked_idx}: {:?}",
            harness.dispatched.borrow()
        );
    }

    /// A `tree` leaf whose `items_window` says its 50 `rows` are only a
    /// slice of a 100,000-row total: [`paint_layout_windowed`] threads the
    /// real total through to the scrollbar (`PaintedHit::Tree::total_rows`),
    /// not the loaded slice's own length, and a real `End` keypress scrolls
    /// all the way to the bottom of the full collection.
    ///
    /// Driven entirely through production code — `InteractionEngine::handle`
    /// for the key (which routes a windowed tree to
    /// [`crate::host::apply_tree_nav_key_windowed`] itself, the
    /// absolute-index counterpart `apply_tree_nav_key` can't stand in for
    /// because a placeholder row's path has no identity stable across a
    /// window update) and [`crate::host::resolve_layout_node_windowed`]
    /// for the resolve that paints the moved cursor. The request reaching
    /// the app comes from the engine's own
    /// `EventSink::dispatch_rows_requested` call, not from this test.
    #[test]
    fn windowed_tree_scrollbar_extent_and_real_scroll_request_more_rows() {
        use quadraui::tui::testing::TuiDriver;
        use quadraui::{Key, Modifiers, NamedKey};

        const TOTAL: usize = 100_000;
        const WINDOW_LEN: usize = 50;
        const WINDOW_START: usize = 35;

        fn leaf_value(window_start: usize) -> serde_json::Value {
            let len = WINDOW_LEN.min(TOTAL.saturating_sub(window_start));
            let rows: Vec<_> = (0..len)
                .map(|i| {
                    let absolute = window_start + i;
                    // `TreePath` is `Vec<u16>` -- `TOTAL` is well past
                    // `u16::MAX`, so each real row's path is split across
                    // two `u16` levels (`absolute` fits in neither alone)
                    // rather than truncating. Only needs to stay unique
                    // per row; nothing downstream reads it as a real
                    // hierarchy.
                    serde_json::json!({
                        "path": [absolute / 60_000, absolute % 60_000],
                        "indent": 0,
                        "icon": null,
                        "text": {"spans": [{"text": format!("row-{absolute}")}]},
                        "badge": null,
                        "is_expanded": null,
                    })
                })
                .collect();
            serde_json::json!({
                "id": "tree",
                "tree": {
                    "id": "tree",
                    "rows": rows,
                    "selection_mode": "Single",
                    "selected_path": null,
                    "style": {
                        "indent": 2,
                        "show_chevrons": true,
                        "chevron_expanded": "v",
                        "chevron_collapsed": ">",
                    },
                    "items_window": {"start": window_start, "total": TOTAL},
                },
            })
        }

        struct WindowedTreeLogic {
            harness: WindowedHarness,
            engine: crate::engine::InteractionEngine,
        }

        impl quadraui::AppLogic for WindowedTreeLogic {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let value = leaf_value(self.harness.window_start.get());
                let diagnostics = crate::wire::DecodeDiagnostics::warn();
                let sets = crate::layout::StyleSets::new();
                let theme = Theme::default();
                let mut node = crate::wire::decode_layout_node_with_styles(
                    "test",
                    &value,
                    crate::wire::StyleContext::new(&sets, &theme),
                    &diagnostics,
                )
                .expect("a tree leaf with items_window must decode");
                let windows = diagnostics.items_windows();
                // The windowed resolve a binding's own render loop makes
                // before `paint_layout_windowed` -- unmodified production
                // code, with no hand-poked cursor state.
                crate::host::resolve_layout_node_windowed(
                    self.engine.widget_state(),
                    backend,
                    area,
                    &mut node,
                    &windows,
                );
                let mut hits = Vec::new();
                paint_layout_windowed(
                    backend,
                    area,
                    &node,
                    &mut hits,
                    &|id| self.engine.chrome_interaction(id),
                    &windows,
                );
                self.engine.set_hits(hits);
            }

            fn handle(
                &mut self,
                event: quadraui::UiEvent,
                backend: &mut dyn Backend,
            ) -> quadraui::Reaction {
                self.engine.handle(event, backend, &mut self.harness.sink())
            }
        }

        fn painted_tree(driver: &TuiDriver<WindowedTreeLogic>) -> (usize, usize, usize) {
            driver
                .app()
                .engine
                .hit_map()
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::Tree {
                        scroll_offset,
                        total_rows,
                        viewport_rows,
                        ..
                    } => Some((*scroll_offset, *total_rows, *viewport_rows)),
                    _ => None,
                })
                .expect("the tree should have painted a hit")
        }

        let harness = WindowedHarness::new(WINDOW_START);
        let logic = WindowedTreeLogic {
            harness: harness.clone(),
            engine: crate::engine::InteractionEngine::new(),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);
        driver.render();

        let (_, total_rows, viewport_rows) = painted_tree(&driver);
        assert_eq!(
            total_rows, TOTAL,
            "the scrollbar extent must reflect the full collection, not the \
             50 loaded rows"
        );
        assert!(viewport_rows > 0, "a 5-row area must fit at least one row");

        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("tree"))),
                "the tree is a tab stop and must accept focus"
            );
        }

        driver.dispatch(quadraui::UiEvent::KeyPressed {
            key: Key::Named(NamedKey::End),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        driver.render();

        let (scroll_offset, total_rows, viewport_rows) = painted_tree(&driver);
        assert_eq!(total_rows, TOTAL);
        assert!(
            scroll_offset + viewport_rows >= TOTAL,
            "End must scroll all the way to the bottom of the full {TOTAL}-row \
             collection, not just the loaded window"
        );

        // The ENGINE asked the app for the rows that keypress brought
        // into view, through `EventSink::dispatch_rows_requested`.
        let request = harness.only_request();
        assert_eq!(request.id, WidgetId::new("tree"));
        assert!(
            request.start + request.count >= TOTAL,
            "the requested window must reach the very end of the collection"
        );
        assert!(
            request.count < WINDOW_LEN * 10,
            "a bounded request must stay a small multiple of the viewport, \
             nowhere near the widget's own {TOTAL}-row total -- got {}",
            request.count
        );
        assert_eq!(harness.window_start.get(), request.start);

        // End-to-end: the harness answered the request from inside
        // `dispatch_rows_requested` (the way a real app's handler does),
        // so the next frame renders the right absolute content at the row
        // actually scrolled to.
        driver.render();

        let (scroll_offset_after, total_rows_after, _) = painted_tree(&driver);
        assert_eq!(total_rows_after, TOTAL);
        assert!(
            scroll_offset_after + viewport_rows >= TOTAL,
            "the absolute scroll position must survive the window update"
        );
        assert!(
            driver.screen_contains(&format!("row-{scroll_offset_after}")),
            "the requested rows must actually paint at the scrolled-to \
             position:\n{}",
            driver.screen()
        );

        // A click on a visible row resolves to that row's ABSOLUTE path,
        // not a band-relative one: `paint_leaf_windowed` shifts every
        // index the backend's own layout reports back into the
        // collection's indexing before it reaches the hit.
        let (click_point, clicked_row) = {
            let hit_map = driver.app().engine.hit_map();
            let (area, layout) = hit_map
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::Tree { area, layout, .. } => Some((*area, layout.clone())),
                    _ => None,
                })
                .expect("the tree should have painted a hit");
            let row = layout
                .visible_rows
                .first()
                .expect("a scrolled-to tree paints at least one row");
            assert_eq!(
                row.row_idx, scroll_offset_after,
                "the first visible row's index must be the ABSOLUTE row \
                 painted at the top of the viewport"
            );
            (
                quadraui::Point {
                    x: area.x + row.bounds.x + 1.0,
                    y: area.y + row.bounds.y + 0.5,
                },
                row.row_idx,
            )
        };
        harness.dispatched.borrow_mut().clear();
        driver.click(click_point.x, click_point.y);
        let clicked_path: Vec<u16> =
            vec![(clicked_row / 60_000) as u16, (clicked_row % 60_000) as u16];
        assert!(
            harness.dispatched.borrow().iter().any(|event| matches!(
                event,
                quadraui::UiEvent::Tree(id, quadraui::TreeEvent::RowClicked { path, .. })
                    if *id == WidgetId::new("tree") && *path == clicked_path
            )),
            "clicking the top visible row of a windowed tree must resolve to \
             absolute row {clicked_row}: {:?}",
            harness.dispatched.borrow()
        );

        // `Home` goes back to the very first row, which this window no
        // longer holds -- so the engine asks for row 0's slice too.
        driver.dispatch(quadraui::UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Home),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        driver.render();
        assert_eq!(
            harness.requests.borrow().len(),
            2,
            "Home at the far end of a windowed tree must request the head \
             of the collection"
        );
        assert_eq!(
            harness.requests.borrow()[1].start,
            0,
            "Home's request must start at the collection's own first row"
        );
        let (scroll_offset_home, _, _) = painted_tree(&driver);
        assert_eq!(
            scroll_offset_home, 0,
            "Home must scroll a windowed tree back to its first row"
        );
        assert!(
            driver.screen_contains("row-0"),
            "the head of the collection must paint after Home:\n{}",
            driver.screen()
        );

        // Pointer input reaches the same request dispatch the keyboard
        // does: `InteractionEngine::handle` translates a wheel notch over
        // the tree body into an absolute `ScrollOffsetChanged` against the
        // full extent, then asks for the rows that offset landed on.
        const WHEEL_ROWS: usize = 120;
        let wheel_point = {
            let hit_map = driver.app().engine.hit_map();
            let (area, layout) = hit_map
                .iter()
                .find_map(|hit| match hit {
                    PaintedHit::Tree { area, layout, .. } => Some((*area, layout.clone())),
                    _ => None,
                })
                .expect("the tree should have painted a hit");
            let row = layout
                .visible_rows
                .first()
                .expect("a tree at its head paints at least one row");
            quadraui::Point {
                x: area.x + row.bounds.x + 1.0,
                y: area.y + row.bounds.y + 0.5,
            }
        };
        driver.dispatch(quadraui::UiEvent::Scroll {
            widget: None,
            delta: quadraui::ScrollDelta::new(0.0, -(WHEEL_ROWS as f32)),
            position: wheel_point,
        });
        let wheel_request = harness
            .requests
            .borrow()
            .last()
            .cloned()
            .expect("a wheel past the loaded window must request rows");
        assert_eq!(
            harness.requests.borrow().len(),
            3,
            "a wheel notch landing outside the loaded window must produce \
             its own request"
        );
        assert!(
            wheel_request.start <= WHEEL_ROWS
                && WHEEL_ROWS < wheel_request.start + wheel_request.count,
            "the wheeled-to row {WHEEL_ROWS} must fall inside the requested \
             window: {wheel_request:?}"
        );
        assert!(
            wheel_request.count < WINDOW_LEN * 10,
            "a wheel's request must stay bounded too -- got {}",
            wheel_request.count
        );
    }

    /// A `text_display` leaf shares the exact same windowed paint path
    /// every other windowed kind goes through ([`paint_leaf_windowed`]'s
    /// `LeafDescriptor::TextDisplay` arm calls the same
    /// [`windowed_text_display`]/[`windowed_vec`] every other kind does),
    /// so its own `PaintedHit::TextDisplay::total_lines` reflects
    /// `window.total` rather than the 10 lines this frame's payload
    /// actually carries — no keyboard-nav-key story to exercise here
    /// (`TextDisplay` has none, controlled or not), so this only covers the
    /// paint-time extent; a windowed display's own wheel scroll reaches the
    /// same `RowsRequested` dispatch every other kind does, through
    /// `TextDisplayEvent::Scrolled`.
    #[test]
    fn windowed_text_display_total_lines_reflects_window_total() {
        use quadraui::tui::testing::TuiDriver;

        const TOTAL: usize = 5_000;
        const WINDOW_LEN: usize = 10;
        const WINDOW_START: usize = 3;

        let lines: Vec<_> = (0..WINDOW_LEN)
            .map(|i| {
                let absolute = WINDOW_START + i;
                serde_json::json!({"spans": [{"text": format!("line-{absolute}")}]})
            })
            .collect();
        let value = serde_json::json!({
            "id": "log",
            "text_display": {
                "id": "log",
                "lines": lines,
                "items_window": {"start": WINDOW_START, "total": TOTAL},
            },
        });

        struct WindowedTextDisplayLogic {
            value: serde_json::Value,
        }

        impl quadraui::AppLogic for WindowedTextDisplayLogic {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let diagnostics = crate::wire::DecodeDiagnostics::warn();
                let sets = crate::layout::StyleSets::new();
                let theme = Theme::default();
                let node = crate::wire::decode_layout_node_with_styles(
                    "test",
                    &self.value,
                    crate::wire::StyleContext::new(&sets, &theme),
                    &diagnostics,
                )
                .expect("a text_display leaf with items_window must decode");
                let windows = diagnostics.items_windows();
                let mut hits = Vec::new();
                paint_layout_windowed(
                    backend,
                    area,
                    &node,
                    &mut hits,
                    &|_| InteractionState::default(),
                    &windows,
                );
                assert!(
                    hits.iter().any(|hit| matches!(
                        hit,
                        PaintedHit::TextDisplay { total_lines, .. } if *total_lines == TOTAL
                    )),
                    "the text_display's own total_lines must reflect window.total, got: {:?}",
                    hits.iter().find_map(|hit| match hit {
                        PaintedHit::TextDisplay { total_lines, .. } => Some(*total_lines),
                        _ => None,
                    })
                );
            }

            fn handle(
                &mut self,
                _event: quadraui::UiEvent,
                _backend: &mut dyn Backend,
            ) -> quadraui::Reaction {
                quadraui::Reaction::Continue
            }
        }

        let mut driver = TuiDriver::new(WindowedTextDisplayLogic { value }, 20, 5);
        driver.render();
    }

    /// [`windowed_vec`]'s own documented shape: the band it materialises
    /// starts at the absolute row the viewport is scrolled to, carries
    /// this frame's loaded rows wherever the two overlap, and a
    /// placeholder everywhere else.
    #[test]
    fn windowed_vec_materialises_the_band_with_the_loaded_slice_spliced_in() {
        let loaded = vec!["a", "b", "c"];
        let window = ItemsWindow {
            start: 5,
            total: 12,
        };
        let band = windowed_band(window, loaded.len(), 4, 5);
        assert_eq!(band, WindowedBand { start: 4, len: 5 });
        let out = windowed_vec(&loaded, window, band, |_| "_");
        assert_eq!(out, vec!["_", "a", "b", "c", "_"]);
    }

    /// A band near the end of the collection is short rather than
    /// over-running it — the same clamp a primitive's own layout applies
    /// to a descriptor's `scroll_offset`.
    #[test]
    fn windowed_band_clamps_to_the_collections_own_tail() {
        let window = ItemsWindow {
            start: 90,
            total: 100,
        };
        let band = windowed_band(window, 10, 97, 32);
        assert_eq!(band, WindowedBand { start: 97, len: 3 });
        // An offset past the end still lands on a real row.
        let band = windowed_band(window, 10, 500, 32);
        assert_eq!(band, WindowedBand { start: 99, len: 1 });
    }

    /// A `window.total` that understates `window.start + loaded.len()`
    /// never truncates the real loaded payload — the extent widens to fit
    /// it instead.
    #[test]
    fn windowed_vec_widens_past_a_stale_undersized_total() {
        let loaded = vec![1, 2, 3];
        let window = ItemsWindow { start: 2, total: 3 };
        let band = windowed_band(window, loaded.len(), 0, 8);
        assert_eq!(band, WindowedBand { start: 0, len: 5 });
        let out = windowed_vec(&loaded, window, band, |_| 0);
        assert_eq!(out, vec![0, 0, 1, 2, 3]);
    }

    /// Neither an adversarial `window.total` nor a scroll offset deep
    /// inside one drives the materialised `Vec` past a viewport's worth of
    /// rows — the whole point of materialising a band instead of padding
    /// out to `window.total`, and what keeps `Vec::with_capacity` off an
    /// unbounded wire field.
    #[test]
    fn windowed_vec_never_materialises_more_than_the_band() {
        let loaded = vec![0_u8; 10];
        let window = ItemsWindow {
            start: 0,
            total: 10_000_000_000,
        };
        for scroll_offset in [0, 9_999_999_000] {
            let band = windowed_band(window, loaded.len(), scroll_offset, 32);
            assert_eq!(band.len, 32, "band stays bounded at {scroll_offset}");
            let out = windowed_vec(&loaded, window, band, |_| 1_u8);
            assert_eq!(out.len(), 32, "materialised rows at {scroll_offset}");
        }
    }

    /// [`windowed_band_rows`] bounds the band by the viewport's own row
    /// capacity plus [`WINDOWED_BAND_PAD_ROWS`], so a 100,000-row
    /// collection materialises a few dozen rows per frame rather than
    /// 100,000 — on a cell-grid backend (TUI, `line_height == 1.0`) and a
    /// pixel one alike.
    #[test]
    fn windowed_band_rows_track_the_viewport_not_the_collection() {
        assert_eq!(
            band_rows_for(24.0, TUI_LINE_HEIGHT),
            24 + WINDOWED_BAND_PAD_ROWS
        );
        assert_eq!(
            band_rows_for(24.0 * PIXEL_LINE_HEIGHT, PIXEL_LINE_HEIGHT),
            24 + WINDOWED_BAND_PAD_ROWS
        );
        // A degenerate `line_height` can't divide by zero into an
        // unbounded band.
        assert_eq!(band_rows_for(1_000.0, 0.0), WINDOWED_BAND_PAD_ROWS);
    }
}
