//! Render-to-frame assembly: paints either the six-fixed-slot
//! [`RenderFrame`]/[`RenderRefs`] shape ([`draw_bands`]) or a decoded
//! [`crate::layout::LayoutNode`] tree ([`paint_layout`]) into a
//! [`quadraui::Backend`], recording a [`PaintedHit`] per primitive painted
//! so a later pointer event can be hit-tested against this frame.

use std::collections::HashMap;
use std::rc::Rc;

use serde::{Deserialize, Deserializer};

use quadraui::{
    Backend, Button, DataTable, InteractionState, ListView, Rect, Scrollbar, Split, SplitDirection,
    SplitHit, SplitLayout, TabBar, TabChrome, TextDisplay, TextDisplayLayout, TextRegion, TreeView,
    WidgetId,
};

use crate::extra_paint::ExtraPaint;
use crate::hit::PaintedHit;
use crate::layout::{
    quantise_horizontal, quantise_vertical, LayoutContainer, LayoutLeaf, LayoutNode, LeafDescriptor,
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
                selected_idx: list.selected_idx,
                total_items: list.items.len(),
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
                focused_index: None,
                tab_count: tab_bar.tabs.len(),
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
            for (child, rect) in container.children.iter().zip(split_rects_with_gap(
                area,
                container.children.len(),
                true,
                container.spacing.gap,
                char_width,
                line_height,
            )) {
                paint_layout(backend, rect, child, hits, chrome_interaction);
            }
        }
        LayoutNode::Column(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = inset_for_spacing(area, container, char_width, line_height);
            for (child, rect) in container.children.iter().zip(split_rects_with_gap(
                area,
                container.children.len(),
                false,
                container.spacing.gap,
                char_width,
                line_height,
            )) {
                paint_layout(backend, rect, child, hits, chrome_interaction);
            }
        }
        LayoutNode::Leaf(leaf) => paint_leaf(backend, area, leaf, hits, chrome_interaction),
        LayoutNode::Split(split) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = split.spacing.inset(area, char_width, line_height);
            // `split.split.first_min`/`second_min` are decoded as portable
            // line counts (see `crate::wire::decode_layout_node`'s split
            // decode); scale them into the backend's native unit here,
            // right before painting, so every backend gets a minimum pane
            // size of the same visual weight from the same decoded number.
            let native_split = Split {
                first_min: split.split.first_min * line_height,
                second_min: split.split.second_min * line_height,
                ..split.split.clone()
            };
            let horizontal = matches!(native_split.direction, SplitDirection::Horizontal);
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

/// Shrinks `area`'s main-axis extent by `gap`, anchored at the same
/// origin — the rect [`paint_layout`]'s `Split` arm feeds `Backend::draw_split`
/// so `Split::layout`'s own ratio/min-size clamping evaluates against the
/// space actually available once the gap between panes is carved out,
/// matching `Split::first_min`/`second_min`'s "a minimum applies to the
/// pane" contract. [`apply_split_gap`] reinserts the carved-out space
/// after painting.
fn shrink_main_axis(area: Rect, horizontal: bool, gap: f32) -> Rect {
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
fn apply_split_gap(layout: &mut SplitLayout, horizontal: bool, gap: f32) {
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
            hits.push(PaintedHit::List {
                id,
                area,
                layout,
                selected_idx: list.selected_idx,
                total_items: list.items.len(),
            });
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
}
