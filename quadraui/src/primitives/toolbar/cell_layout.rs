//! Cell-grid layout for [`Toolbar`] — the measure/pack half of this
//! widget's TUI story, written in whole terminal cells.
//!
//! Every entry point here funnels into
//! [`Toolbar::layout_with_options`]/[`Toolbar::layout_with_overflow`]
//! with [`super::cell_item_width`] as the measurer, so a terminal and a
//! pixel backend share one packing algorithm and differ only in the unit
//! they measure in. [`super::cell_paint`] paints what these resolve, and
//! [`crate::primitive::Primitive::layout`]'s own
//! [`crate::primitive::Units::Cells`] branch resolves through
//! [`tui_toolbar_layout_with_options`] — so paint, hit-test and the
//! widget contract can never disagree about where an item sits.
//!
//! `#[cfg(feature = "tui")]`: the geometry is expressed in
//! [`ratatui::layout::Rect`]'s whole-cell `u16`s, the same unit
//! [`super::cell_paint`] writes glyphs in.

use ratatui::layout::Rect;

use super::{
    Toolbar, ToolbarButton, ToolbarItemMeasure, ToolbarLayout, ToolbarLayoutOptions,
    ToolbarOrientation, ToolbarOverflow, ToolbarOverflowPriorities, ToolbarVAlign,
};

/// Resolve which row within `[area.y, area.y + area.height)` button /
/// label text paints on, per `valign`. TUI's own cell-grid
/// twin of [`super::valign_offset_y`] — kept
/// separate because a terminal row is already a quantized unit, so
/// folding this into the pixel-unit `f32` formula would just re-round
/// it back.
///
/// `Center`'s formula places text on the row above centre for even
/// heights, matching the pixel backends' `(slot_height - content_height) / 2.0`
/// integer-truncating behaviour.
pub(crate) fn tui_text_row(valign: ToolbarVAlign, area: Rect) -> u16 {
    match valign {
        ToolbarVAlign::Top => area.y,
        ToolbarVAlign::Center => area.y + area.height.saturating_sub(1) / 2,
        ToolbarVAlign::Bottom => area.y + area.height.saturating_sub(1),
    }
}

/// The cell-unit width of a single toolbar item, shared with every
/// other cell-grid caller — see [`super::cell_item_width`].
pub(crate) use super::cell_item_width as tui_item_width;

/// How a horizontal [`ToolbarButton::Group`] degrades when the bar is too
/// narrow to paint it at full size — tried in this order by
/// [`choose_group_tier`]:
///
/// 1. [`Self::Full`]: every child keeps its icon, label and key hint.
/// 2. [`Self::IconOnly`]: child labels/key-hints drop, icons stay — same
///    `[ icon ]` compaction a standalone icon-only `Action` already gets
///    from [`tui_item_width`].
/// 3. [`Self::Flat`]: the group itself disappears — its children splice
///    into the bar as ordinary top-level items, each painting its own
///    `[ … ]` bracket, with no shared border or divider at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupDegradeTier {
    Full,
    IconOnly,
    Flat,
}

/// [`GroupDegradeTier::IconOnly`]'s per-child transform: clear `label`/
/// `key_hint` on an [`ToolbarButton::Action`] that has an icon to fall
/// back to (an icon-less child can't shrink further, so it's left
/// untouched and stays at [`GroupDegradeTier::Full`] size even inside an
/// `IconOnly` group).
fn tui_group_child_icon_only(btn: &ToolbarButton) -> ToolbarButton {
    match btn {
        ToolbarButton::Action {
            id,
            icon: Some(icon),
            enabled,
            is_active,
            tooltip,
            spec,
            ..
        } => ToolbarButton::Action {
            id: *id,
            label: String::new(),
            icon: Some(icon.clone()),
            key_hint: None,
            enabled: *enabled,
            is_active: *is_active,
            tooltip: tooltip.clone(),
            spec: spec.clone(),
        },
        other => other.clone(),
    }
}

/// Build `bar.buttons` as it paints at `tier`, plus a map from each
/// original top-level index to its new index (`None` when that index no
/// longer names a single item — only possible for a flattened `Group`,
/// which has no slot of its own once its children splice into the bar).
fn tui_tiered_buttons(
    buttons: &[ToolbarButton],
    tier: GroupDegradeTier,
) -> (Vec<ToolbarButton>, Vec<Option<usize>>) {
    match tier {
        GroupDegradeTier::Full => (buttons.to_vec(), (0..buttons.len()).map(Some).collect()),
        GroupDegradeTier::IconOnly => {
            let out = buttons
                .iter()
                .map(|b| match b {
                    ToolbarButton::Group {
                        id,
                        items,
                        exclusive,
                        label,
                    } => ToolbarButton::Group {
                        id: *id,
                        items: items.iter().map(tui_group_child_icon_only).collect(),
                        exclusive: *exclusive,
                        label: label.clone(),
                    },
                    other => other.clone(),
                })
                .collect();
            (out, (0..buttons.len()).map(Some).collect())
        }
        GroupDegradeTier::Flat => {
            let mut out = Vec::with_capacity(buttons.len());
            let mut map = Vec::with_capacity(buttons.len());
            for b in buttons {
                match b {
                    ToolbarButton::Group { items, .. } => {
                        map.push(None);
                        out.extend(items.iter().cloned());
                    }
                    other => {
                        map.push(Some(out.len()));
                        out.push(other.clone());
                    }
                }
            }
            (out, map)
        }
    }
}

/// Pick the narrowest-fitting [`GroupDegradeTier`] for `bar` inside
/// `width` cells — [`GroupDegradeTier::Full`] when it already fits,
/// otherwise [`GroupDegradeTier::IconOnly`], otherwise
/// [`GroupDegradeTier::Flat`]. A bar with no [`ToolbarButton::Group`]
/// is always [`GroupDegradeTier::Full`] (nothing to degrade).
fn choose_group_tier(bar: &Toolbar, width: f32) -> GroupDegradeTier {
    let total_width =
        |buttons: &[ToolbarButton]| -> f32 { buttons.iter().map(tui_item_width).sum() };
    if total_width(&bar.buttons) <= width {
        return GroupDegradeTier::Full;
    }
    let (icon_only, _) = tui_tiered_buttons(&bar.buttons, GroupDegradeTier::IconOnly);
    if total_width(&icon_only) <= width {
        return GroupDegradeTier::IconOnly;
    }
    GroupDegradeTier::Flat
}

/// `bar` as it actually paints inside `width` cells: unchanged when
/// everything fits, otherwise degraded per [`choose_group_tier`], with
/// [`Toolbar::focused_index`] remapped to match (or cleared, for a
/// flattened `Group`'s own now-nonexistent slot — a `Group` was never a
/// keyboard focus target to begin with, see [`Toolbar::focused_index`]'s
/// doc).
fn tui_toolbar_effective(bar: &Toolbar, width: f32) -> Toolbar {
    if !bar
        .buttons
        .iter()
        .any(|b| matches!(b, ToolbarButton::Group { .. }))
    {
        return bar.clone();
    }
    let tier = choose_group_tier(bar, width);
    if tier == GroupDegradeTier::Full {
        return bar.clone();
    }
    let (buttons, index_map) = tui_tiered_buttons(&bar.buttons, tier);
    let focused_index = bar
        .focused_index
        .and_then(|i| index_map.get(i).copied().flatten());
    let mut effective = bar.clone();
    effective.buttons = buttons;
    effective.focused_index = focused_index;
    effective
}

/// [`tui_toolbar_effective`] plus the layout it produces, computed once so
/// every caller indexes the same (possibly degraded) button list the
/// layout's `item_idx`s refer to.
pub(super) fn tui_toolbar_effective_layout(bar: &Toolbar, area: Rect) -> (Toolbar, ToolbarLayout) {
    let effective = tui_toolbar_effective(bar, area.width as f32);
    let layout = effective.layout(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height.max(1) as f32,
        |btn| ToolbarItemMeasure::new(tui_item_width(btn)),
    );
    (effective, layout)
}

/// Compute the layout the rasteriser would produce. No paint side-effects.
///
/// When `bar` has a [`ToolbarButton::Group`] too wide for `area.width`,
/// this is the layout of the *degraded* bar — see [`tui_toolbar_effective`]
/// — so a caller using this purely for hit-testing (e.g. the backend's
/// click router) agrees with whatever [`draw_toolbar_with_options`] last
/// painted at the same `area`.
pub fn tui_toolbar_layout(bar: &Toolbar, area: Rect) -> ToolbarLayout {
    tui_toolbar_effective_layout(bar, area).1
}

/// [`tui_toolbar_layout`], plus [`ToolbarLayoutOptions`]: a
/// [`ToolbarOrientation::Vertical`] bar measures every item at a fixed
/// 1-row height — the "one item per row" degrade this rasteriser uses for
/// a vertical strip, since a terminal cell has no sub-row granularity to
/// spend on a taller/shorter item the way a pixel backend could.
pub fn tui_toolbar_layout_with_options(
    bar: &Toolbar,
    area: Rect,
    options: ToolbarLayoutOptions,
) -> ToolbarLayout {
    match options.orientation {
        ToolbarOrientation::Horizontal => tui_toolbar_layout(bar, area),
        ToolbarOrientation::Vertical => bar.layout_with_options(
            area.x as f32,
            area.y as f32,
            area.width.max(1) as f32,
            area.height as f32,
            options,
            |_btn| ToolbarItemMeasure::new(1.0),
        ),
    }
}

/// [`ToolbarLayoutOptions`] for the vertical icon-rail shape
/// [`crate::primitives::activity_bar::ActivityBar::to_toolbar_parts`]
/// produces: one item per row,
/// [`ToolbarLayoutOptions::with_pin_trailing_overflow_to_origin`] set —
/// the same collision rule [`crate::primitives::activity_bar::ActivityBar::layout`]
/// itself uses for its bottom-pinned group. Shared by
/// [`draw_toolbar_with_rail_style`] (painting, at the real `area` origin)
/// and `ActivityBar`'s own TUI rasteriser (hit regions, at origin
/// `(0.0, 0.0)`) so both agree on exactly the same geometry.
pub(crate) fn rail_layout_options() -> ToolbarLayoutOptions {
    ToolbarLayoutOptions::new()
        .with_orientation(ToolbarOrientation::Vertical)
        .with_pin_trailing_overflow_to_origin(true)
}

/// Compute the layout [`draw_toolbar_with_overflow`] would paint. No
/// paint side-effects — same split [`tui_toolbar_layout`] keeps from
/// [`draw_toolbar`], so a click router can hit-test without a `Buffer`.
///
/// [`ToolbarOverflow::Wrap`] always measures each wrapped line at exactly
/// one terminal row — a terminal cell has no sub-row granularity to spend
/// on a taller line the way a pixel backend could — regardless of
/// `area.height`; [`ToolbarLayout::cross_extent`] on the result reports
/// how many rows the wrapped content actually used, so a caller sizes
/// `area` from that before the next [`draw_toolbar_with_overflow`] call.
/// [`ToolbarOverflow::Clip`]/[`ToolbarOverflow::Menu`] keep `area.height`
/// as the single line's row count, same as [`tui_toolbar_layout`].
///
/// A host that skips the resize step above still gets every wrapped line
/// back in `visible_items` (so a 1-row probe `area` can still discover
/// `cross_extent`), but any item on a row at or past `area.height` is
/// marked un-clickable, matching [`draw_toolbar_with_overflow`]'s own clip
/// at `area_bottom` — otherwise [`ToolbarLayout::hit_test`] would report a
/// hit on a button never actually painted.
pub fn tui_toolbar_layout_with_overflow(
    bar: &Toolbar,
    area: Rect,
    options: ToolbarLayoutOptions,
    priorities: &ToolbarOverflowPriorities,
) -> ToolbarLayout {
    // `Clip` matches [`tui_toolbar_layout_with_options`] exactly, group
    // degrade tiers included — compute it that way instead of re-deriving.
    if options.overflow == ToolbarOverflow::Clip {
        return tui_toolbar_layout_with_options(bar, area, options);
    }

    let line_height: f32 = match options.overflow {
        ToolbarOverflow::Wrap => 1.0,
        _ => area.height.max(1) as f32,
    };
    let mut layout = match options.orientation {
        ToolbarOrientation::Horizontal => bar.layout_with_overflow(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            line_height,
            options,
            priorities,
            |btn| ToolbarItemMeasure::new(tui_item_width(btn)),
        ),
        ToolbarOrientation::Vertical => bar.layout_with_overflow(
            area.x as f32,
            area.y as f32,
            area.width.max(1) as f32,
            area.height as f32,
            options,
            priorities,
            |_btn| ToolbarItemMeasure::new(1.0),
        ),
    };

    layout.demote_lines_past(options, area.y as f32 + area.height as f32);

    layout
}
