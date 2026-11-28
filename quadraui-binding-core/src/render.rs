//! Render-to-frame assembly: paints either a flat [`RenderFrame`]
//! ([`draw_bands`]) or a decoded
//! [`crate::layout::LayoutNode`] tree ([`paint_layout`]) into a
//! [`quadraui::Backend`], recording a [`PaintedHit`] per primitive painted
//! so a later pointer event can be hit-tested against this frame.

use std::collections::HashMap;
use std::rc::Rc;

use serde::{Deserialize, Deserializer};

use quadraui::{
    Backend, Button, DataTable, ListView, Rect, ScrollAxis, ScrollView, SectionSize, Split,
    SplitDirection, Style, TabBar, TabChrome, TextDisplay, Theme, TreeView, WidgetId,
};

use quadraui::primitive::{DynWidget, LayoutCtx as DynLayoutCtx, Size as DynSize};
use quadraui::DrawsPrimitives;

use crate::hit::{PaintedHit, ScrollChildHit};
use crate::layout::{
    LayoutChild, LayoutContainer, LayoutLeaf, LayoutNode, LayoutScroll, LayoutStyled,
    LeafDescriptor,
};
use quadraui::layout::stack::{
    apply_min_max, apply_split_gap, quantise_horizontal, quantise_vertical, shrink_main_axis,
    stack_rects, MainAxisSpec, StackChild, StackStyle,
};
use quadraui::layout::NaturalSize;

/// Owned decode of a flat render callback dict: one optional descriptor
/// per widget kind, keyed by kind name. A binding decodes its own wire
/// format (a Python `dict`, a JS object, ...) into this type, and
/// [`draw_bands`] paints [`Self::widgets`].
#[derive(Default)]
pub struct RenderFrame {
    /// The widget kinds present in this frame, keyed by kind name.
    pub widgets: HashMap<String, Rc<dyn DynWidget>>,
}

/// Decodes a render callback's returned `dict` into a [`RenderFrame`]:
/// each [`crate::widget_kinds::widget_leaf_kinds`] key holds that kind's
/// descriptor; a missing key or a `null` value both mean "draw nothing in
/// this slot this frame". Every error is labeled `"render"`.
///
/// Builds a fresh warn-mode [`crate::wire::DecodeDiagnostics`] per call
/// (unlike [`crate::wire::decode_render_output_with_styles`], which takes a
/// session-scoped one), so an unknown optional key found through this impl
/// never warns once across calls or honours a binding's `strict` setting.
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

/// Paints a [`LayoutNode::Tabs`]'s strip into `rect` through the widget
/// contract and records it as a [`PaintedHit::Widget`], so a click or key
/// on it routes through [`TabBar`]'s own [`quadraui::Interactive`] impl.
fn paint_tab_bar(
    backend: &mut dyn Backend,
    rect: Rect,
    tab_bar: TabBar,
    hits: &mut Vec<PaintedHit>,
) {
    let id = tab_bar.id;
    let widget: Rc<dyn DynWidget> = Rc::new(tab_bar);
    let _ = backend.draw_dyn(rect, widget.as_ref(), &());
    hits.push(PaintedHit::Widget {
        id,
        area: rect,
        widget,
    });
}

/// Paints `panel`'s header through the widget contract and records it as
/// a [`PaintedHit::Widget`] covering the header slot only, so a click on a
/// header button routes through `SidebarPanel`'s own
/// [`quadraui::Interactive`] impl while a click in the content area falls
/// through to whatever the content subtree paints. Returns the content
/// rect the subtree paints into.
fn paint_sidebar_panel(
    backend: &mut dyn Backend,
    area: Rect,
    panel: &quadraui::SidebarPanel,
    hits: &mut Vec<PaintedHit>,
) -> Rect {
    let resolved = backend.draw_dyn(area, panel, &());
    let Some(layout) = resolved.downcast_ref::<quadraui::SidebarPanelLayout>() else {
        return area;
    };
    if let Some(header) = layout.toolbar_bounds {
        // The recorded widget is re-laid-out against `header` alone at
        // click time, so it carries the painted header height explicitly
        // and the click layout's toolbar matches the painted one exactly.
        hits.push(PaintedHit::Widget {
            id: panel.id,
            area: header,
            widget: Rc::new(quadraui::SidebarPanel {
                toolbar_height: Some(header.height),
                ..panel.clone()
            }),
        });
    }
    layout.content_bounds
}

/// Allocates the next `lines`-tall band (scaled by `line_height`),
/// starting at the mutable cursor `*y` and never exceeding `viewport`'s
/// remaining height — the sequential top-to-bottom allocation [`draw_bands`]
/// uses for each present kind. Returns `None` once the viewport has run out
/// of vertical room; `*y` is only advanced when a band is actually returned.
fn next_band(
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

/// How many text lines tall [`draw_bands`] makes `kind`'s band: the row
/// collections get four, every other kind three.
fn flat_band_lines(kind: &str) -> f32 {
    if matches!(kind, "list" | "tree" | "data_table") {
        4.0
    } else {
        3.0
    }
}

/// Paints each present kind of a flat frame top-to-bottom in a
/// fixed-height band ([`flat_band_lines`]), in
/// [`crate::widget_kinds::widget_leaf_kinds`] order so the output is
/// deterministic regardless of `HashMap` iteration order, skipping any
/// kind once the viewport runs out of room. Appends a
/// [`PaintedHit::Widget`] to `hits` for every widget painted, keyed by the
/// widget's own id (or its kind, for a widget with no id), so a later
/// pointer event can be routed to it.
pub fn draw_bands(
    backend: &mut dyn Backend,
    widgets: &HashMap<String, Rc<dyn DynWidget>>,
    hits: &mut Vec<PaintedHit>,
) {
    let viewport = backend.viewport();
    // Band heights are given in text LINES, not raw viewport units:
    // `viewport` and `line_height` share the backend's own unit (cells on
    // TUI, where `line_height` is 1.0; device pixels on the native
    // backends), so scaling the line count by `line_height` lays the bands
    // out correctly on every backend.
    let line_height = backend.line_height();
    let mut y = 0.0_f32;
    for kind in crate::widget_kinds::widget_leaf_kinds() {
        let Some(widget) = widgets.get(kind) else {
            continue;
        };
        let Some(rect) = next_band(
            &mut y,
            viewport.width,
            viewport.height,
            line_height,
            flat_band_lines(kind),
        ) else {
            continue;
        };
        if paint_widget_leaf(backend, rect, widget.as_ref()) {
            let id = widget.id().cloned().unwrap_or_else(|| WidgetId::new(kind));
            hits.push(PaintedHit::Widget {
                id,
                area: rect,
                widget: Rc::clone(widget),
            });
        }
    }
}

/// The per-call paint state a generic leaf paints with: whether it holds
/// keyboard focus, as [`quadraui::primitive::PaintState::focus_ring_visible`]. A
/// widget whose look follows focus (a form's focused-row highlight) reads
/// it there, so the host's focus shows without the app echoing it onto
/// the descriptor.
struct LeafFocus(bool);

impl quadraui::primitive::PaintState for LeafFocus {
    fn focus_ring_visible(&self) -> bool {
        self.0
    }
}

/// Paints one generic leaf at `area`, unless the platform presents it
/// itself ([`DynWidget::native_presentation`], a native menu bar), in
/// which case nothing paints in the window. Returns whether it painted,
/// so a natively presented leaf records no hit to route window input to.
fn paint_widget_leaf(backend: &mut dyn Backend, area: Rect, widget: &dyn DynWidget) -> bool {
    if widget.native_presentation(backend, area) {
        return false;
    }
    let focused = widget
        .id()
        .is_some_and(|id| backend.focus_tree().focused_id() == Some(id));
    backend.draw_dyn(area, widget, &LeafFocus(focused));
    true
}

/// Paints a layout tree into `area`. A `Row`/`Column` first shrinks `area`
/// by its own `padding`/`margin` and reserves `gap` between children (see
/// [`inset_for_spacing`]/[`quadraui::layout::stack::split_rects_with_gap`] — a no-spacing container
/// behaves exactly as before these existed), then divides what's left into
/// equal shares — one per child, side by side or stacked — and recurses;
/// equal shares (rather than `draw_bands`'s fixed line-count bands) fill
/// the whole area regardless of viewport size. A `Leaf` paints its one
/// descriptor into the whole area its container gave it. A `Split` divides
/// its area into two unequal panes at `ratio` via `draw_primitive`
/// (the same divider paint/hit-test every backend already shares for the
/// Rust-side `Split` primitive) and recurses into each with its resolved
/// bounds. A `Tabs` reserves a top band for its tab strip (painted via
/// [`paint_tab_bar`], the `TabBar` widget contract every backend already
/// shares for the Rust-side `TabBar` primitive) and recurses into the
/// remaining area with the selected tab's content, so a tab group is just a
/// `Split` whose panes are each a `Tabs`. A `SidebarPanel` paints its
/// chrome (toolbar) through the `SidebarPanel` widget contract
/// ([`paint_sidebar_panel`]) and recurses into `content_bounds` with its
/// one content subtree; a
/// `SplitTree` paints its dividers via `draw_primitive` and
/// recurses into each resolved leaf rect. All three record a [`PaintedHit`]
/// for their own chrome (divider / toolbar button), same as `Split`,
/// so a later pointer event can resolve against it. A `Grid` resolves its
/// `columns`/`rows` tracks into cell rects via
/// [`LayoutGrid::cell_rects`](crate::layout::LayoutGrid::cell_rects) and recurses into each with the
/// matching row-major child; it paints no chrome of its own, so it records
/// no hit. Per-child flex weights on `Row`/`Column` are a follow-up; every
/// `Row`/`Column` child gets the same share for now.
pub fn paint_layout(
    backend: &mut dyn Backend,
    area: Rect,
    node: &LayoutNode,
    hits: &mut Vec<PaintedHit>,
) {
    match node {
        LayoutNode::Row(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = inset_for_spacing(area, container, char_width, line_height);
            let rects = layout_container_rects(backend, area, container, true);
            for (child, rect) in container.children.iter().zip(rects) {
                paint_layout(backend, rect, &child.node, hits);
            }
        }
        LayoutNode::Column(container) => {
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = inset_for_spacing(area, container, char_width, line_height);
            let rects = layout_container_rects(backend, area, container, false);
            for (child, rect) in container.children.iter().zip(rects) {
                paint_layout(backend, rect, &child.node, hits);
            }
        }
        LayoutNode::Grid(grid) => {
            let rects = grid.cell_rects(area, backend.char_width(), backend.line_height());
            for (child, rect) in grid.children.iter().zip(rects) {
                paint_layout(backend, rect, child, hits);
            }
        }
        LayoutNode::Leaf(leaf) => paint_leaf(backend, area, leaf, hits),
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
            // `area` BEFORE laying the split out -- `first_min`/
            // `second_min` apply to the pane after the gap is already
            // subtracted, not before.
            let draw_area = shrink_main_axis(area, horizontal, gap);
            let mut layout = backend.draw_primitive(draw_area, &native_split);
            apply_split_gap(&mut layout, horizontal, gap);
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
            let char_width = backend.char_width();
            let line_height = backend.line_height();
            let area = tabs.spacing.inset(area, char_width, line_height);
            let strip_height = (3.0 * backend.line_height()).min(area.height);
            let strip_rect = Rect::new(area.x, area.y, area.width, strip_height);
            // `focused_index` threads through the bar's `TabChrome`
            // rather than the wire `tabs.tab_bar` itself, so `None` (the
            // common case) composes to `TabChrome::default()`.
            let bar = TabBar {
                chrome: TabChrome::default().with_focused_index(tabs.focused_index),
                ..tabs.tab_bar.clone()
            };
            paint_tab_bar(backend, strip_rect, bar, hits);
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
        LayoutNode::SidebarPanel(sidebar_node) => {
            let panel = &sidebar_node.sidebar_panel;
            let content_bounds = paint_sidebar_panel(backend, area, panel, hits);
            paint_layout(backend, content_bounds, &sidebar_node.content, hits);
        }
        LayoutNode::SplitTree(split_tree) => {
            let layout = backend.draw_primitive(area, &split_tree.tree);
            hits.push(PaintedHit::SplitTree {
                id: WidgetId::new(split_tree.id.as_str()),
                layout: layout.clone(),
            });
            for ((_, rect), content) in layout.leaves.iter().zip(split_tree.leaves.iter()) {
                paint_layout(backend, *rect, content, hits);
            }
        }
        LayoutNode::Scroll(scroll_node) => {
            paint_scroll(backend, area, scroll_node, hits);
        }
        LayoutNode::Styled(styled) => {
            paint_styled(backend, area, styled, hits);
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
) {
    let push = push_styled_override(backend, &styled.style);
    paint_layout(backend, area, &styled.node, hits);
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
/// shared scrollbar ([`quadraui::ScrollView::scrollbar`] painted through
/// `draw_dyn`, the same geometry a standalone `"scroll_view"` leaf paints
/// into its own fixed `line_height`-wide track). `scroll_node.offset` is
/// read as-is — already resolved against [`quadraui::WidgetStateStore`] by
/// [`crate::host::resolve_layout_node`], which must run on this tree before
/// this function does (same ordering [`crate::layout`]'s own `Split`/
/// `Tabs` resolution already requires).
fn paint_scroll(
    backend: &mut dyn Backend,
    area: Rect,
    scroll_node: &LayoutScroll,
    hits: &mut Vec<PaintedHit>,
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
                        paint_layout(backend, slot.rect, &child.node, hits);
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
                        paint_layout(backend, rect, &shadow, hits);
                    }
                    None => {
                        paint_layout(backend, rect, content, hits);
                    }
                }
            }
        }
    }
    pop_styled_scope(backend, styled_scope);
    crate::hit::clip_hits_to_viewport(hits, hits_before, viewport);
    backend.pop_clip();

    if let Some(scrollbar) = &scrollbar {
        let _ = backend.draw_dyn(scrollbar.track, scrollbar, &());
        backend.register_zone(scrollbar.id, scrollbar.track);
    }
}

/// `content`, when it is a bare text display, list, tree or table leaf —
/// the kinds [`is_offset_windowed_leaf`] singles out — with a clone of its
/// widget whose `scroll_offset` is advanced to the whole row/line `offset`
/// native units lands on (a text display also gets `auto_scroll`
/// disabled, so it can't override that). `None` for any other leaf kind,
/// or a non-leaf node (a `Row`/`Column` reaches [`ScrollSlots::Children`]
/// instead, never `Atomic`).
///
/// Each of these kinds renders its own `scroll_offset`-relative window
/// into its data regardless of where its own rect sits on screen — unlike
/// every other `Atomic` leaf kind, whose rect `scroll_content_slots`
/// positions by shifting it `-offset` along the scroll axis, repainting a
/// clone scrolled this way is what lets [`paint_scroll`]'s own
/// `ScrollSlots::Atomic` arm show the right row/line first. The leaf's
/// `id` is kept, so the clone has the same hit-map id. Matches via
/// [`LayoutNode::unstyled`], so a `Styled` leaf gets the same treatment as
/// an unstyled one -- the returned node carries no `Styled` wrapper of its
/// own, since [`paint_scroll`]'s own `push_styled_scope` call already has
/// the matching override active on `backend` by the time it paints.
fn offset_windowed_leaf_scrolled_to_offset(
    content: &LayoutNode,
    offset: f32,
    line_height: f32,
) -> Option<LayoutNode> {
    let LayoutNode::Leaf(leaf) = content.unstyled() else {
        return None;
    };
    let LeafDescriptor::Widget { kind, widget } = &leaf.descriptor;
    let row = (offset / line_height.max(1.0)).floor().max(0.0) as usize;
    let widget: Rc<dyn DynWidget> = if let Some(td) = widget.downcast_ref::<TextDisplay>() {
        Rc::new(TextDisplay {
            scroll_offset: row,
            auto_scroll: false,
            ..td.clone()
        })
    } else if let Some(list) = widget.downcast_ref::<ListView>() {
        Rc::new(ListView {
            scroll_offset: row,
            ..list.clone()
        })
    } else if let Some(tree) = widget.downcast_ref::<TreeView>() {
        Rc::new(TreeView {
            scroll_offset: row,
            ..tree.clone()
        })
    } else if let Some(table) = widget.downcast_ref::<DataTable>() {
        Rc::new(DataTable {
            scroll_offset: row,
            ..table.clone()
        })
    } else {
        return None;
    };
    Some(LayoutNode::Leaf(LayoutLeaf {
        id: leaf.id.clone(),
        descriptor: LeafDescriptor::Widget { kind, widget },
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
/// a standalone `"scroll_view"` leaf already paints its own track into. Reserved unconditionally (not only when
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
/// `List`/`Tree`/`DataTable`'s width, and whichever axis a generic
/// widget's own `Primitive::measure` leaves open);
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
            floor: 0.0,
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
/// never gets a [`PaintedHit`] (or a resolved `WidgetStateStore` entry) in the
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
    // The same cell-grid-backend test `crate::layout::native_units`
    // already uses. A cell-grid `draw_*`
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
    let LeafDescriptor::Widget { widget, .. } = &leaf.descriptor;
    widget.downcast_ref::<TextDisplay>().is_some()
        || widget.downcast_ref::<ListView>().is_some()
        || widget.downcast_ref::<TreeView>().is_some()
        || widget.downcast_ref::<DataTable>().is_some()
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

/// Best-effort intrinsic size of `node`, consulted by
/// [`layout_container_rects`] for a [`quadraui::SectionSize::Content`]
/// child's main-axis size and for a non-`Stretch`
/// [`quadraui::Align`]'s cross-axis size, and by [`natural_main_length`]
/// for a `Scroll`'s content. A leaf measures through its widget's own
/// `Primitive::measure`; every other node kind (a nested `Row`/`Column`/
/// `Split`/`Tabs`/`SidebarPanel`/`SplitTree`) has no cheap measurement at
/// this layer and reports [`None`] on both axes — see
/// [`NaturalSize::main`]/[`NaturalSize::cross`]'s callers for how each
/// degrades a `None`.
fn natural_size(backend: &mut dyn Backend, node: &LayoutNode, line_height: f32) -> NaturalSize {
    // A `Styled` wrapper carries no geometry of its own -- unwrap to
    // whatever it wraps, but push the override first via the same
    // `push_styled_override`/`pop_styled_override` bracket `paint_styled`
    // brackets its own recursive paint with, so a `padding` override on
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
    let LeafDescriptor::Widget { widget, .. } = &leaf.descriptor;
    // No final rect exists yet at measurement time, so there is no
    // `avail` to pass: a widget reports its content's own size, and the
    // container clamps it.
    let ctx = DynLayoutCtx::new(
        line_height,
        backend.char_width(),
        backend.nerd_fonts_enabled(),
    );
    let natural = widget.measure(&ctx, DynSize::new(0.0, 0.0));
    NaturalSize {
        width: natural.width,
        height: natural.height,
    }
}

/// Resolves a [`LayoutContainer`]'s children into their final paint rects
/// through the core stack solver [`stack_rects`], measuring each child's
/// natural size on demand with [`natural_size`]. `area` is the
/// container's own inset area (post `padding`/`margin` — see
/// [`inset_for_spacing`]); `horizontal` is `true` for a `Row`, `false` for
/// a `Column`. [`paint_layout`] and `crate::host::resolve_layout_node`
/// share this so a click resolves against the exact geometry just painted.
pub fn layout_container_rects(
    backend: &mut dyn Backend,
    area: Rect,
    container: &LayoutContainer,
    horizontal: bool,
) -> Vec<Rect> {
    let char_width = backend.char_width();
    let line_height = backend.line_height();
    let children: Vec<StackChild> = container
        .children
        .iter()
        .map(|child| StackChild {
            size: child.size,
            min: child.min,
            max: child.max,
        })
        .collect();
    let style = StackStyle {
        gap: container.spacing.gap,
        justify: container.justify,
        align: container.align,
        wrap: container.wrap,
    };
    stack_rects(
        area,
        &children,
        style,
        horizontal,
        char_width,
        line_height,
        &mut |i| natural_size(backend, &container.children[i].node, line_height),
    )
}

/// Shrinks `area` by `container.spacing` — see [`quadraui::Spacing::inset`]
/// for the margin-then-padding rule and why `char_width`/`line_height` (the
/// painting backend's own `Backend::char_width()`/`Backend::line_height()`)
/// are needed. `Split`/`Tabs` call `Spacing::inset` directly since they
/// have no [`LayoutContainer`] to unwrap it from.
fn inset_for_spacing(
    area: Rect,
    container: &LayoutContainer,
    char_width: f32,
    line_height: f32,
) -> Rect {
    container.spacing.inset(area, char_width, line_height)
}

/// Walks `node`'s layout tree collecting the tree-address `id` ->
/// `Button::tooltip` text of every `"button"`-kind
/// [`LeafDescriptor::Widget`] leaf that sets one. [`paint_leaf`] keys a
/// layout-tree leaf's [`PaintedHit::Widget`] by this same tree-address id
/// (not the descriptor's own embedded `Button::id`, which a render
/// callback may leave at its default and reuse across leaves — see
/// [`paint_leaf`]'s own doc), so a binding that paints
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
            let LeafDescriptor::Widget { kind, widget } = &leaf.descriptor;
            if *kind == "button" {
                if let Some(button) = widget.as_ref().downcast_ref::<Button>() {
                    if let Some(tooltip) = &button.tooltip {
                        out.insert(WidgetId::new(leaf.id.clone()), tooltip.clone());
                    }
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

/// Paints one leaf's widget into the whole `area` its container gave it.
/// Appends a [`PaintedHit::Widget`] for it, keyed by the leaf's own
/// tree-address `id` (not the descriptor's embedded `id`, which a render
/// callback may leave at its default and reuse across leaves) so a later
/// pointer event can be routed back to this exact slot in the layout tree.
pub fn paint_leaf(
    backend: &mut dyn Backend,
    area: Rect,
    leaf: &LayoutLeaf,
    hits: &mut Vec<PaintedHit>,
) {
    let LeafDescriptor::Widget { widget, .. } = &leaf.descriptor;
    if paint_widget_leaf(backend, area, widget.as_ref()) {
        hits.push(PaintedHit::Widget {
            id: WidgetId::new(leaf.id.clone()),
            area,
            widget: Rc::clone(widget),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::layout::StyleOverride;
    use quadraui::layout::stack::split_rects_with_gap;
    use quadraui::{Decoration, StyledSpan, TextDisplayLine};
    use quadraui::{EdgeInsets, Spacing};

    /// A real font's reported char/line metrics (GTK/macOS/Win) — any
    /// value `> 1.0` takes [`quadraui::layout::stack::quantise_horizontal`]/
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

    // ── Split/Tabs spacing ───────────────────────────────────────────────

    use quadraui::SplitMeasure;

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
            paint_layout(backend, area, &node, &mut hits);
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
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(WidgetId::new(id), label)),
                    },
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
            paint_layout(backend, area, &node, &mut hits);
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
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(button),
                    },
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
            paint_layout(backend, area, &node, &mut hits);
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
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new(id), label)),
                        },
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
                paint_layout(backend, area, &row, &mut hits);
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
                paint_layout(backend, area, &node, &mut hits);
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

    use crate::layout::LayoutChild;
    use quadraui::tui::TuiBackend;
    use quadraui::{Align, Justify};

    fn tui() -> TuiBackend {
        TuiBackend::new()
    }

    fn leaf_child(id: &str, size: SectionSize, min: Option<f32>, max: Option<f32>) -> LayoutChild {
        LayoutChild {
            node: LayoutNode::Leaf(LayoutLeaf {
                id: id.to_string(),
                descriptor: LeafDescriptor::Widget {
                    kind: "button",
                    widget: Rc::new(Button::new(WidgetId::new(id), id)),
                },
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
                    descriptor: LeafDescriptor::Widget {
                        kind: "button",
                        widget: Rc::new(Button::new(WidgetId::new("x"), "x")),
                    },
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

    /// Every one of the text-entry leaf kinds sizes to its own content
    /// under a non-`Stretch` `Align`, exactly like a `Button`/
    /// `TextDisplay`/`TabBar` leaf already does in
    /// [`align_every_value_at_two_terminal_sizes`] above, through its own
    /// `Primitive::measure`.
    /// `Align::Start`/`Center`/`End` each size the child to its reported
    /// natural height (well under the tall `area` below, so "non-
    /// stretched" is unambiguous) and position it at the top, middle,
    /// or bottom of the container respectively; only `Align::Stretch`
    /// (checked once up front) still fills the whole extent.
    #[test]
    fn align_sizes_every_text_entry_leaf_kind_to_its_own_natural_height() {
        use quadraui::primitive::DynWidget;
        use quadraui::{PasswordInput, TextArea, TextField, TextInput};

        let kinds: Vec<(&'static str, Rc<dyn DynWidget>)> = vec![
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
                Rc::new(PasswordInput::new(WidgetId::new("password"))),
            ),
            (
                "textarea",
                Rc::new(TextArea::new(WidgetId::new("textarea"))),
            ),
        ];

        let height = 24.0_f32;
        let area = Rect::new(0.0, 0.0, 80.0, height);
        for (name, widget) in kinds {
            let leaf = || {
                LayoutNode::Leaf(LayoutLeaf {
                    id: name.to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: name,
                        widget: widget.clone(),
                    },
                })
            };
            let child = || vec![LayoutChild::new(leaf())];

            let mut backend = tui();
            let line_height = backend.line_height();
            let natural = natural_size(&mut backend, &leaf(), line_height)
                .height
                .unwrap_or_else(|| {
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
    /// painter actually paints, not just [`Checkbox`]'s shape: a
    /// `Switch` under-reporting its `4`-cell `[● ]`/`[ ●]` track collapses
    /// its own cell painter's knob column so on/off paint identically, and
    /// a `RadioGroup` under-reporting its `"(x) label"` chrome by even one
    /// cell truncates the last label character under a non-`Stretch`
    /// `Column` align. Both resolve through
    /// [`quadraui::primitive::Primitive::measure`] (the generic
    /// [`crate::widget_kinds::WIDGET_KINDS`] seam), so this test builds a
    /// cells-mode `LayoutCtx` from the same backend's own metrics.
    #[test]
    fn switch_and_radio_group_natural_width_matches_what_tui_paints() {
        use quadraui::primitive::{LayoutCtx, Primitive};
        use quadraui::{RadioGroup, RadioOption, Switch};

        let backend = tui();
        let char_width = backend.char_width();
        let ctx = LayoutCtx::new(backend.chrome_line_height(), char_width, false);

        let switch = Switch::new(WidgetId::new("switch"));
        let switch_width =
            Primitive::measure(&switch, &ctx, quadraui::primitive::Size::new(0.0, 0.0))
                .width
                .expect("Switch reports a natural width");
        assert_eq!(
            switch_width, 4.0,
            "Switch's natural width should match its own cell painter's 4-cell track"
        );

        let group = RadioGroup::new(
            WidgetId::new("radio_group"),
            vec![
                RadioOption::new(WidgetId::new("radio_group:a"), "Ab"),
                RadioOption::new(WidgetId::new("radio_group:b"), "C"),
            ],
        );
        let group_width =
            Primitive::measure(&group, &ctx, quadraui::primitive::Size::new(0.0, 0.0))
                .width
                .expect("RadioGroup reports a natural width");
        // `cell_paint::paint` paints `"({glyph}) {label}"`: `'('`, glyph,
        // `')'`, a gap, then the widest label -- 4 cells of chrome plus
        // the label's own width.
        assert_eq!(
            group_width,
            4.0 + 2.0,
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
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new("a"), "a")),
                        },
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "b".to_string(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new("b"), "b")),
                        },
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "c".to_string(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new("c"), "c")),
                        },
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
                        descriptor: LeafDescriptor::Widget {
                            kind: "button",
                            widget: Rc::new(Button::new(WidgetId::new(label.clone()), label)),
                        },
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
            paint_layout(backend, area, &node, &mut hits);
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
                PaintedHit::Widget { id, widget, .. } if widget.as_any().is::<Button>() => {
                    Some(id.as_str().to_string())
                }
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
            Some((WidgetId::new("20"), "widget_body".to_string()))
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
            paint_layout(backend, area, &node, &mut hits);
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

    struct ScrollTextAtomicLogic {
        offset: f32,
        line_count: usize,
        engine: crate::engine::InteractionEngine,
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

    /// [`ScrollListAtomicLogic`]'s `Tree` counterpart — same scenario, a
    /// 20-row `Tree` AS the scroll's entire content, in a 5-row viewport
    /// at offset `7`.
    struct ScrollTreeAtomicLogic {
        offset: f32,
        row_count: usize,
        engine: crate::engine::InteractionEngine,
    }

    /// [`ScrollListAtomicLogic`]'s `DataTable` counterpart — same scenario,
    /// a 20-row `DataTable` AS the scroll's entire content, in a 5-row
    /// viewport at offset `7`.
    struct ScrollDataTableAtomicLogic {
        offset: f32,
        row_count: usize,
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
                    descriptor: LeafDescriptor::Widget {
                        kind: "text_display",
                        widget: Rc::new(td),
                    },
                })),
                spacing: Spacing::default(),
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
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
                items_window: None,
                scroll_px: None,
                has_focus: true,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: false,
                striped: false,
            };
            let node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: Some(self.offset),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "list".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "list",
                        widget: Rc::new(list),
                    },
                })),
                spacing: Spacing::default(),
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
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
                items_window: None,
                scroll_px: None,
                style: quadraui::TreeStyle::default(),
                has_focus: true,
            };
            let node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: Some(self.offset),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "tree".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "tree",
                        widget: Rc::new(tree),
                    },
                })),
                spacing: Spacing::default(),
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
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
                items_window: None,
                scroll_px: None,
                sort: None,
                has_focus: true,
                show_scrollbar: false,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: vec![],
                footer: None,
                bordered: false,
                striped: false,
                glyphs: None,
            };
            let node = LayoutNode::Scroll(LayoutScroll {
                id: "scroll".to_string(),
                axis: ScrollAxis::Vertical,
                offset: Some(self.offset),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "table".to_string(),
                    descriptor: LeafDescriptor::Widget {
                        kind: "data_table",
                        widget: Rc::new(table),
                    },
                })),
                spacing: Spacing::default(),
            });
            let mut hits = Vec::new();
            paint_layout(backend, area, &node, &mut hits);
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

    /// A 20-line/row collection AS a `Scroll`'s entire content (not
    /// wrapped in a `Row`/`Column` — `scroll_content_slots`'s `Atomic`
    /// branch) in a 5-row viewport at offset `7`: a cell-grid backend's
    /// `Atomic` leaf rect clamps a negative main-axis origin to `0`
    /// without shrinking, so shifting the rect by `-offset` the way a
    /// plain leaf is positioned would always repaint row `0` at the
    /// viewport's top regardless of `offset`. Each of these kinds renders
    /// its own `scroll_offset..` window, which is what lets the clone
    /// `offset_windowed_leaf_scrolled_to_offset` builds show row `7`
    /// first instead.
    fn assert_scrolled_to_row_seven<A: quadraui::AppLogic<AreaId = ()>>(logic: A, label: &str) {
        let mut driver = quadraui::tui::testing::TuiDriver::new(logic, 20, 5);
        driver.render();
        assert!(
            driver.screen_contains(&format!("{label} 7")),
            "row 7 must paint inside the viewport:\n{}",
            driver.screen()
        );
        assert!(
            !driver.screen_contains(&format!("{label} 0")),
            "row 0 is scrolled out of view:\n{}",
            driver.screen()
        );
    }

    #[test]
    fn scroll_atomic_text_display_at_offset_shows_the_offset_line_first() {
        let logic = ScrollTextAtomicLogic {
            offset: 7.0,
            line_count: 20,
            engine: crate::engine::InteractionEngine::new(),
        };
        assert_scrolled_to_row_seven(logic, "line");
    }

    #[test]
    fn scroll_atomic_list_at_offset_shows_the_offset_item_first() {
        let logic = ScrollListAtomicLogic {
            offset: 7.0,
            item_count: 20,
            engine: crate::engine::InteractionEngine::new(),
        };
        assert_scrolled_to_row_seven(logic, "item");
    }

    #[test]
    fn scroll_atomic_tree_at_offset_shows_the_offset_row_first() {
        let logic = ScrollTreeAtomicLogic {
            offset: 7.0,
            row_count: 20,
            engine: crate::engine::InteractionEngine::new(),
        };
        assert_scrolled_to_row_seven(logic, "row");
    }

    #[test]
    fn scroll_atomic_data_table_at_offset_shows_the_offset_row_first() {
        let logic = ScrollDataTableAtomicLogic {
            offset: 7.0,
            row_count: 20,
            engine: crate::engine::InteractionEngine::new(),
        };
        assert_scrolled_to_row_seven(logic, "row");
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
            paint_layout(backend, area, &node, &mut hits);
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

    /// A `"text_display"` leaf `id` holding one line per entry of `texts`.
    fn text_display_leaf(id: &str, texts: &[&str]) -> LayoutNode {
        let mut td = TextDisplay::new(WidgetId::new(id));
        for text in texts {
            td.append_line(TextDisplayLine {
                spans: vec![StyledSpan::plain(*text)],
                decoration: Decoration::default(),
                timestamp: None,
            });
        }
        td.auto_scroll = false;
        LayoutNode::Leaf(LayoutLeaf {
            id: id.to_string(),
            descriptor: LeafDescriptor::Widget {
                kind: "text_display",
                widget: Rc::new(td),
            },
        })
    }

    fn two_line_text_display(id: &str) -> LayoutNode {
        text_display_leaf(id, &["a", "b"])
    }

    fn one_line_text_display(id: &str) -> LayoutNode {
        text_display_leaf(id, &["x"])
    }

    /// The `(leaf id, area)` of every text display painted into `hits`.
    fn painted_text_displays(hits: &[PaintedHit]) -> Vec<(String, Rect)> {
        hits.iter()
            .filter_map(|hit| match hit {
                PaintedHit::Widget { id, area, widget }
                    if widget.downcast_ref::<TextDisplay>().is_some() =>
                {
                    Some((id.as_str().to_string(), *area))
                }
                _ => None,
            })
            .collect()
    }

    fn text_display_column(count: usize) -> LayoutNode {
        LayoutNode::Column(LayoutContainer::new(
            (0..count)
                .map(|i| two_line_text_display(&i.to_string()))
                .collect(),
            Spacing::default(),
        ))
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
}
