//! TUI rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! Paints a single-row strip of `[icon label (key)]` cells with hit
//! zones per `Action`. Separators render as a dim ` │ ` cell; non-
//! clickable `Label` items paint their text in `theme.muted_fg`. A
//! `Group` paints one shared `[ … ]` bracket pair around its children,
//! with a `│` divider between them and no bracket of its own per child —
//! see `draw_group_horizontal`. When the bar is narrower than a `Group`
//! needs, it degrades through `GroupDegradeTier`: full labels, then
//! icon-only children, then flat (ungrouped) items.
//!
//! ## Per-state colouring
//!
//! Priority (highest first): pressed → hovered → focused → is_active → enabled.
//!
//! | State              | Foreground             | Background          |
//! |--------------------|------------------------|---------------------|
//! | Action, enabled    | `theme.foreground`     | `bar_bg`            |
//! | Action, disabled   | `theme.muted_fg`       | `bar_bg`            |
//! | Action, is_active  | `theme.foreground`     | `theme.selected_bg` |
//! | Action, focused    | `theme.foreground`     | `theme.accent_bg`   |
//! | Action, hovered    | `theme.hover_fg`       | `theme.hover_bg`    |
//! | Action, pressed    | `theme.foreground`     | `theme.selected_bg` |
//! | Separator          | `theme.muted_fg`       | `bar_bg`            |
//! | Label              | `Label.fg` or `muted_fg`| `bar_bg`           |
//!
//! `bar_bg` is `Toolbar.bg.unwrap_or(theme.header_bg)`.
//!
//! Focus is set via [`crate::primitives::toolbar::Toolbar::focused_index`].
//! The host app advances focus with Tab / Shift-Tab and activates with
//! Enter / Space; this rasteriser is purely declarative.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::text::{char_cell_width, display_width};
use super::{qc, set_cell, set_cell_wide};
use crate::primitives::toolbar::{
    Toolbar, ToolbarButton, ToolbarItemKind, ToolbarItemMeasure, ToolbarLayout,
    ToolbarLayoutOptions, ToolbarOrientation, ToolbarOverflow, ToolbarOverflowPriorities,
    ToolbarPaintOptions, ToolbarRailStyle, ToolbarVAlign, VisibleToolbarItem,
};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Resolve which row within `[area.y, area.y + area.height)` button /
/// label text paints on, per `valign`. TUI's own cell-grid
/// twin of [`crate::primitives::toolbar::valign_offset_y`] — kept
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

/// Compute the TUI cell-unit width of a single toolbar item.
///
/// Action layout:
/// - icon-only (label empty): `[ {icon} ]` = 4 + icon_w cells
/// - normal: `[ {icon }{label}{ (key)} ]` = 4 + icon_block + label_w + hint_block
///
/// Separator: 2 cells (` │`). Label: text width (double-width aware).
pub(crate) fn tui_item_width(btn: &ToolbarButton) -> f32 {
    match btn {
        ToolbarButton::Action {
            label,
            icon,
            key_hint,
            ..
        } => {
            let label_w = display_width(label);
            let icon_w = icon.as_ref().map(|s| display_width(s)).unwrap_or(0);
            // Icon-only: drop the trailing space after the icon — pack
            // glyph snug between the brackets.
            let icon_block = if icon.is_some() && label_w > 0 {
                icon_w + 1
            } else {
                icon_w
            };
            let hint_w = key_hint
                .as_ref()
                .map(|s| display_width(s) + 3) // " (xxx)"
                .unwrap_or(0);
            // "[ " + icon_block + label + hint_w + " ]"
            (4 + icon_block + label_w + hint_w) as f32
        }
        ToolbarButton::Separator => 2.0,
        ToolbarButton::Label { text, .. } => display_width(text) as f32,
        // Never consulted by `tui_toolbar_layout` (`Spacer`'s slot is
        // whatever room is left over) — zero here so a caller that
        // measures one directly gets an inert result.
        ToolbarButton::Spacer => 0.0,
        // Reserves exactly as much room as `n` standalone, fully
        // bracketed children would need, plus `n + 1` 1-cell gaps —
        // matching `layout_group_children`'s even split of
        // `measure(group) - sum(measure(child))`. `draw_group_horizontal`
        // then paints only one shared `[`/`]` pair and a `│` divider
        // into those gaps rather than per-child brackets, so each
        // child's own slot carries a little extra blank padding where
        // its own `[ `/` ]` would otherwise have gone — the same
        // trade `measure_button`'s `Group` arm above makes for the
        // pixel backends (each child still reserves its own full
        // width; only the *painted* chrome is shared).
        ToolbarButton::Group { items, .. } => {
            let child_sum: f32 = items.iter().map(tui_item_width).sum();
            child_sum + items.len() as f32 + 1.0
        }
    }
}

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
fn tui_toolbar_effective_layout(bar: &Toolbar, area: Rect) -> (Toolbar, ToolbarLayout) {
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

/// Char sequence for one button's `icon label (key hint)` content, with
/// the same icon-only compaction [`tui_item_width`]'s `Action` arm
/// measures against (drop the trailing space after the icon when there's
/// no label to separate it from). Shared by every call site that paints
/// this exact content — a flat `Action`, a `Group` child, and the
/// vertical rail's own one-row `Action` — so the three can't drift.
/// Callers wrap the result with their own leading/trailing cells: a
/// `[`/`]` bracket pair for a standalone button, or two blank cells for a
/// `Group` child painting inside the group's own shared bracket.
fn action_content_cells(icon: Option<&str>, label: &str, key_hint: Option<&str>) -> Vec<char> {
    let label_w = display_width(label);
    let icon_only = icon.is_some() && label_w == 0;
    let mut cells = Vec::new();
    if let Some(icon) = icon {
        for ch in icon.chars() {
            cells.push(ch);
        }
        if !icon_only {
            cells.push(' ');
        }
    }
    for ch in label.chars() {
        cells.push(ch);
    }
    if let Some(hint) = key_hint {
        cells.push(' ');
        cells.push('(');
        for ch in hint.chars() {
            cells.push(ch);
        }
        cells.push(')');
    }
    cells
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

/// Draw a [`Toolbar`] into `area` on `buf`. Returns the layout for host
/// click dispatch.
///
/// Equivalent to [`draw_toolbar_with_options`] with
/// [`ToolbarPaintOptions::default()`] — kept as a separate, **unchanged**
/// function (rather than growing this one's argument list) so every
/// existing caller of this re-exported `pub fn` keeps compiling
/// untouched. See [`ToolbarPaintOptions`] and `CLAUDE.md`'s *Downstream
/// consumers* rule 2.
pub fn draw_toolbar(
    buf: &mut Buffer,
    area: Rect,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> ToolbarLayout {
    draw_toolbar_with_options(
        buf,
        area,
        bar,
        theme,
        hovered_id,
        pressed_id,
        ToolbarPaintOptions::default(),
    )
}

/// [`draw_toolbar`], plus [`ToolbarPaintOptions`]: `options.valign`
/// resolves which row button/label text paints on when
/// `area.height > 1` — see [`tui_text_row`].
pub fn draw_toolbar_with_options(
    buf: &mut Buffer,
    area: Rect,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    options: ToolbarPaintOptions,
) -> ToolbarLayout {
    if area.width == 0 || area.height == 0 {
        // A zero-height area should not produce clickable buttons for a row
        // that was never painted; return an empty layout instead.
        return ToolbarLayout::empty(crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ));
    }

    let (effective, layout) = tui_toolbar_effective_layout(bar, area);

    let bar_bg = qc(bar.bg.unwrap_or(theme.header_bg));
    let fg = qc(theme.foreground);
    let muted = qc(theme.muted_fg);
    let hover_bg = qc(theme.hover_bg);
    let hover_fg = qc(theme.hover_fg);
    let active_bg = qc(theme.selected_bg);
    let focus_bg = qc(theme.accent_bg);

    // Resolve the text row per `options.valign`. On a
    // 1-row slot every variant resolves to `area.y`.
    let text_row = tui_text_row(options.valign, area);

    // Fill every row of the bar background so multi-row hosts (Gap 1)
    // get a uniform-coloured strip rather than 1 row painted + N-1
    // rows of stale buffer.
    for dy in 0..area.height {
        for dx in 0..area.width {
            set_cell(buf, area.x + dx, area.y + dy, ' ', fg, bar_bg);
        }
    }

    for vis in &layout.visible_items {
        let item_x = vis.bounds.x.round() as u16;
        let item_w = vis.bounds.width.round() as u16;
        if item_w == 0 {
            continue;
        }
        let btn = &effective.buttons[vis.item_idx];

        match btn {
            ToolbarButton::Action {
                id,
                label,
                icon,
                key_hint,
                enabled,
                is_active,
                spec,
                ..
            } => {
                let is_hovered = *enabled && hovered_id == Some(id);
                let is_pressed = *enabled && pressed_id == Some(id);
                let is_focused = *enabled && effective.focused_index == Some(vis.item_idx);
                // An explicit `spec` override's `variant` tints the
                // resting foreground the same way `crate::tui::button`
                // paints a standalone `Danger`/`Warning` `Button` — see
                // that module's doc. Lowest priority: pressed/hovered/
                // focused/is_active all still win, same as a focused
                // `Button` wins over its own intent colour. Only
                // `variant` is honoured here today — `icon`/
                // `icon_position`/`icon_gap`/`tooltip`/`disabled` on the
                // spec have no TUI toolbar effect yet; this button keeps
                // painting through its own legacy `icon`/`enabled` fields.
                let intent_fg = spec.as_ref().and_then(|s| match s.variant {
                    crate::primitives::button::ButtonVariant::Danger => Some(theme.error_fg),
                    crate::primitives::button::ButtonVariant::Warning => Some(theme.warning_fg),
                    _ => None,
                });
                // Priority: pressed > hovered > focused > is_active > enabled > intent.
                let (cell_fg, cell_bg) = if !*enabled {
                    (muted, bar_bg)
                } else if is_pressed || *is_active {
                    (fg, active_bg)
                } else if is_hovered {
                    (hover_fg, hover_bg)
                } else if is_focused {
                    (fg, focus_bg)
                } else if let Some(intent) = intent_fg {
                    (qc(intent), bar_bg)
                } else {
                    (fg, bar_bg)
                };

                // Build the rendered cells. Icon-only buttons compact
                // to `[ icon ]` (no trailing space after the icon).
                let mut cells: Vec<char> = Vec::with_capacity(item_w as usize);
                cells.push('[');
                cells.push(' ');
                cells.extend(action_content_cells(
                    icon.as_deref(),
                    label,
                    key_hint.as_deref(),
                ));
                cells.push(' ');
                cells.push(']');

                // Paint background across every row of the item so the
                // hover/active highlight reads as one solid button on
                // multi-row toolbars (Gap 1).
                for dy in 0..area.height {
                    for dx in 0..item_w {
                        set_cell(buf, item_x + dx, area.y + dy, ' ', cell_fg, cell_bg);
                    }
                }

                // Paint text on the vertically-centred row, advancing
                // by the wide-char-aware cell width of each glyph.
                let mut col: u16 = 0;
                for ch in cells {
                    let cw = char_cell_width(ch);
                    if col + cw > item_w {
                        break;
                    }
                    set_cell(buf, item_x + col, text_row, ch, cell_fg, cell_bg);
                    col += cw;
                }
            }
            ToolbarButton::Separator => {
                // 2 cells: ` │`, drawn through every row of the bar so
                // the divider reads as a single full-height rule on
                // multi-row toolbars.
                for dy in 0..area.height {
                    if item_w >= 1 {
                        set_cell(buf, item_x, area.y + dy, ' ', muted, bar_bg);
                    }
                    if item_w >= 2 {
                        set_cell(buf, item_x + 1, area.y + dy, '│', muted, bar_bg);
                    }
                }
            }
            ToolbarButton::Label { text, fg: label_fg } => {
                let color = label_fg.map(qc).unwrap_or(muted);
                let mut col: u16 = 0;
                for ch in text.chars() {
                    let cw = char_cell_width(ch);
                    if col + cw > item_w {
                        break;
                    }
                    set_cell(buf, item_x + col, text_row, ch, color, bar_bg);
                    col += cw;
                }
            }
            // A `Spacer`'s slot is painted already (the bar-background
            // fill loop above), and it is never clickable, so there is
            // nothing further to draw here.
            ToolbarButton::Spacer => {}
            ToolbarButton::Group { items, .. } => {
                draw_group_horizontal(
                    buf,
                    area.y..area.y + area.height,
                    vis,
                    items,
                    hovered_id,
                    pressed_id,
                    text_row,
                    bar_bg,
                    fg,
                    muted,
                    hover_bg,
                    hover_fg,
                    active_bg,
                );
            }
        }
    }

    layout
}

/// Paint one [`ToolbarButton::Group`] slot (`vis`, whose `kind` is
/// [`ToolbarItemKind::Group`]) as one shared `[ a │ b │ c ]` frame: the
/// group's own leading/trailing bracket cells, plus a `│` divider
/// between children — no child draws a bracket of its own, matching
/// `crate::tui::form`'s `draw_field_segmented_control`.
/// `vis.group_children`'s bounds already reserve exactly these gaps (see
/// [`tui_item_width`]'s `Group` arm), so this paints into them rather
/// than re-deriving positions. Each child keeps the per-state colouring
/// priority order (`enabled`/`is_active`/`hovered`)
/// [`ToolbarButton::Action`]'s own arm above uses, scoped to
/// `child.action_id` instead of `bar.focused_index` (a `Group` child is
/// never a keyboard focus target today).
#[allow(clippy::too_many_arguments)]
fn draw_group_horizontal(
    buf: &mut Buffer,
    rows: std::ops::Range<u16>,
    vis: &VisibleToolbarItem,
    items: &[ToolbarButton],
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    text_row: u16,
    bar_bg: ratatui::style::Color,
    fg: ratatui::style::Color,
    muted: ratatui::style::Color,
    hover_bg: ratatui::style::Color,
    hover_fg: ratatui::style::Color,
    active_bg: ratatui::style::Color,
) {
    let item_x = vis.bounds.x.round() as u16;
    let item_w = vis.bounds.width.round() as u16;
    if item_w == 0 {
        return;
    }

    // The group's own leading/trailing bracket cells.
    for y in rows.clone() {
        set_cell(buf, item_x, y, '[', muted, bar_bg);
        if item_w >= 2 {
            set_cell(buf, item_x + item_w - 1, y, ']', muted, bar_bg);
        }
    }

    for child in &vis.group_children {
        let cx = child.bounds.x.round() as u16;
        let cw = child.bounds.width.round() as u16;
        if cw == 0 {
            continue;
        }

        // Divider cell immediately before this child (every child but
        // the first).
        if child.item_idx > 0 && cx > item_x {
            for y in rows.clone() {
                set_cell(buf, cx - 1, y, '│', muted, bar_bg);
            }
        }

        let Some(btn) = items.get(child.item_idx) else {
            continue;
        };
        let (label, icon, key_hint, enabled): (&str, Option<&str>, Option<&str>, bool) = match btn {
            ToolbarButton::Action {
                label,
                icon,
                key_hint,
                enabled,
                ..
            } => (
                label.as_str(),
                icon.as_deref(),
                key_hint.as_deref(),
                *enabled,
            ),
            ToolbarButton::Label { text, .. } => (text.as_str(), None, None, true),
            _ => continue,
        };

        let is_hovered = enabled && child.action_id.as_ref() == hovered_id;
        let is_pressed = enabled && child.action_id.as_ref() == pressed_id;
        let (cell_fg, cell_bg) = if !enabled {
            (muted, bar_bg)
        } else if is_pressed || child.is_active {
            (fg, active_bg)
        } else if is_hovered {
            (hover_fg, hover_bg)
        } else {
            (fg, bar_bg)
        };

        for y in rows.clone() {
            for dx in 0..cw {
                set_cell(buf, cx + dx, y, ' ', cell_fg, cell_bg);
            }
        }

        // A `Label` child paints bare (no padding, matching the flat
        // `Label` arm above); an `Action` child paints the same content a
        // standalone button would, but with two blank cells where its
        // own `[ `/` ]` would otherwise go — the group's single shared
        // bracket pair (painted above) and the `│` divider between
        // children are the only brackets drawn, matching
        // `crate::tui::form`'s `draw_field_segmented_control`.
        let is_action = matches!(btn, ToolbarButton::Action { .. });
        let mut cells: Vec<char> = Vec::with_capacity(cw as usize);
        if is_action {
            cells.push(' ');
            cells.push(' ');
            cells.extend(action_content_cells(icon, label, key_hint));
            cells.push(' ');
            cells.push(' ');
        } else {
            cells.extend(label.chars());
        }

        let mut col: u16 = 0;
        for ch in cells {
            let cw2 = char_cell_width(ch);
            if col + cw2 > cw {
                break;
            }
            set_cell(buf, cx + col, text_row, ch, cell_fg, cell_bg);
            col += cw2;
        }
    }
}

/// [`draw_toolbar`], plus [`ToolbarLayoutOptions`]: `options.orientation`
/// picks whether buttons flow left-to-right (delegates straight to
/// [`draw_toolbar`]) or top-to-bottom, one item per terminal row (see
/// [`ToolbarOrientation::Vertical`] and [`tui_toolbar_layout_with_options`]).
///
/// No `ToolbarPaintOptions`/`valign` parameter: a [`ToolbarVAlign`] choice
/// only affects a slot taller than one text row, and every vertical item
/// here is exactly one row tall, so threading it through would be a
/// knob with no effect — a caller that wants `valign` on a `Horizontal`
/// bar calls [`draw_toolbar_with_options`] directly instead of this fn.
///
/// The vertical degrade paints the same `[ icon label (key) ]` bracket
/// cells and per-state colouring [`draw_toolbar_with_options`] uses —
/// just one full-width row per item instead of one column-packed cell
/// run — so a host that switches a toolbar's orientation keeps its
/// existing hover/pressed/focused/active/disabled visuals.
pub fn draw_toolbar_oriented(
    buf: &mut Buffer,
    area: Rect,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    layout_options: ToolbarLayoutOptions,
) -> ToolbarLayout {
    if layout_options.orientation == ToolbarOrientation::Horizontal {
        return draw_toolbar(buf, area, bar, theme, hovered_id, pressed_id);
    }

    if area.width == 0 || area.height == 0 {
        return ToolbarLayout::empty(crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ));
    }

    let layout = tui_toolbar_layout_with_options(bar, area, layout_options);

    let bar_bg = qc(bar.bg.unwrap_or(theme.header_bg));
    let fg = qc(theme.foreground);
    let muted = qc(theme.muted_fg);
    let hover_bg = qc(theme.hover_bg);
    let hover_fg = qc(theme.hover_fg);
    let active_bg = qc(theme.selected_bg);
    let focus_bg = qc(theme.accent_bg);

    for dy in 0..area.height {
        for dx in 0..area.width {
            set_cell(buf, area.x + dx, area.y + dy, ' ', fg, bar_bg);
        }
    }

    for vis in &layout.visible_items {
        let row = vis.bounds.y.round() as u16;
        let row_h = vis.bounds.height.round() as u16;
        if row_h == 0 {
            continue;
        }
        let btn = &bar.buttons[vis.item_idx];

        match btn {
            ToolbarButton::Action {
                id,
                label,
                icon,
                key_hint,
                enabled,
                is_active,
                spec,
                ..
            } => {
                let is_hovered = *enabled && hovered_id == Some(id);
                let is_pressed = *enabled && pressed_id == Some(id);
                let is_focused = *enabled && bar.focused_index == Some(vis.item_idx);
                let intent_fg = spec.as_ref().and_then(|s| match s.variant {
                    crate::primitives::button::ButtonVariant::Danger => Some(theme.error_fg),
                    crate::primitives::button::ButtonVariant::Warning => Some(theme.warning_fg),
                    _ => None,
                });
                let (cell_fg, cell_bg) = if !*enabled {
                    (muted, bar_bg)
                } else if is_pressed || *is_active {
                    (fg, active_bg)
                } else if is_hovered {
                    (hover_fg, hover_bg)
                } else if is_focused {
                    (fg, focus_bg)
                } else if let Some(intent) = intent_fg {
                    (qc(intent), bar_bg)
                } else {
                    (fg, bar_bg)
                };

                let mut cells: Vec<char> = Vec::new();
                cells.push('[');
                cells.push(' ');
                cells.extend(action_content_cells(
                    icon.as_deref(),
                    label,
                    key_hint.as_deref(),
                ));
                cells.push(' ');
                cells.push(']');

                for dx in 0..area.width {
                    set_cell(buf, area.x + dx, row, ' ', cell_fg, cell_bg);
                }
                let mut col: u16 = 0;
                for ch in cells {
                    let cw = char_cell_width(ch);
                    if col + cw > area.width {
                        break;
                    }
                    set_cell(buf, area.x + col, row, ch, cell_fg, cell_bg);
                    col += cw;
                }
            }
            ToolbarButton::Separator => {
                for dx in 0..area.width {
                    set_cell(buf, area.x + dx, row, '─', muted, bar_bg);
                }
            }
            ToolbarButton::Label { text, fg: label_fg } => {
                let color = label_fg.map(qc).unwrap_or(muted);
                let mut col: u16 = 0;
                for ch in text.chars() {
                    let cw = char_cell_width(ch);
                    if col + cw > area.width {
                        break;
                    }
                    set_cell(buf, area.x + col, row, ch, color, bar_bg);
                    col += cw;
                }
            }
            // The spacer's rows are already bar-background from the fill
            // loop above; nothing more to paint, and it's never clickable.
            ToolbarButton::Spacer => {}
            ToolbarButton::Group { items, .. } => {
                draw_group_horizontal(
                    buf,
                    row..row + row_h,
                    vis,
                    items,
                    hovered_id,
                    pressed_id,
                    row,
                    bar_bg,
                    fg,
                    muted,
                    hover_bg,
                    hover_fg,
                    active_bg,
                );
            }
        }
    }

    layout
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
pub fn tui_toolbar_layout_with_overflow(
    bar: &Toolbar,
    area: Rect,
    options: ToolbarLayoutOptions,
    priorities: &ToolbarOverflowPriorities,
) -> ToolbarLayout {
    let line_height: f32 = match options.overflow {
        ToolbarOverflow::Wrap => 1.0,
        _ => area.height.max(1) as f32,
    };
    match options.orientation {
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
    }
}

/// [`draw_toolbar_with_options`], plus [`ToolbarOverflow`]:
/// `options.overflow` picks how items that don't fit are handled.
///
/// - [`ToolbarOverflow::Clip`] paints identically to
///   [`draw_toolbar_with_options`] (same single-row geometry).
/// - [`ToolbarOverflow::Wrap`] paints each wrapped line on its own
///   terminal row below `area.y`, clipped to `area.height` — call
///   [`tui_toolbar_layout_with_overflow`] first and grow `area` to its
///   `ToolbarLayout::cross_extent` so every row actually paints.
/// - [`ToolbarOverflow::Menu`] paints a trailing `[ » ]` cell in the same
///   bracket format as a plain `Action` — the `»` affordance has no
///   `WidgetId` a host can compare against `hovered_id`/`pressed_id`, so
///   it always paints in the resting `fg`/`bar_bg` pair. A click on it
///   hit-tests to [`crate::primitives::toolbar::ToolbarHit::Overflow`];
///   open a [`crate::ContextMenu`] built from
///   [`ToolbarLayout::overflowed_items`].
///
/// `priorities` is read only when `options.overflow` is
/// [`ToolbarOverflow::Menu`] — pass [`ToolbarOverflowPriorities::new`]
/// for the other two variants.
#[allow(clippy::too_many_arguments)]
pub fn draw_toolbar_with_overflow(
    buf: &mut Buffer,
    area: Rect,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    options: ToolbarLayoutOptions,
    priorities: &ToolbarOverflowPriorities,
) -> ToolbarLayout {
    if area.width == 0 || area.height == 0 {
        return ToolbarLayout::empty(crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ));
    }

    let layout = tui_toolbar_layout_with_overflow(bar, area, options, priorities);

    let bar_bg = qc(bar.bg.unwrap_or(theme.header_bg));
    let fg = qc(theme.foreground);
    let muted = qc(theme.muted_fg);
    let hover_bg = qc(theme.hover_bg);
    let hover_fg = qc(theme.hover_fg);
    let active_bg = qc(theme.selected_bg);
    let focus_bg = qc(theme.accent_bg);

    let area_bottom = area.y + area.height;
    for dy in area.y..area_bottom {
        for dx in area.x..area.x + area.width {
            set_cell(buf, dx, dy, ' ', fg, bar_bg);
        }
    }

    for vis in &layout.visible_items {
        let item_x = vis.bounds.x.round() as u16;
        let item_y = vis.bounds.y.round() as u16;
        let item_w = vis.bounds.width.round() as u16;
        let item_h = (vis.bounds.height.round() as u16).max(1);
        if item_w == 0 || item_y >= area_bottom {
            continue;
        }
        let row_h = item_h.min(area_bottom - item_y);
        let text_row = item_y;

        if vis.kind == ToolbarItemKind::Overflow {
            for dy in 0..row_h {
                for dx in 0..item_w {
                    set_cell(buf, item_x + dx, item_y + dy, ' ', fg, bar_bg);
                }
            }
            let mut col: u16 = 0;
            for ch in ['[', ' ', '»', ' ', ']'] {
                let cw = char_cell_width(ch);
                if col + cw > item_w {
                    break;
                }
                set_cell(buf, item_x + col, text_row, ch, fg, bar_bg);
                col += cw;
            }
            continue;
        }

        let Some(btn) = bar.buttons.get(vis.item_idx) else {
            continue;
        };
        match btn {
            ToolbarButton::Action {
                id,
                label,
                icon,
                key_hint,
                enabled,
                is_active,
                spec,
                ..
            } => {
                let is_hovered = *enabled && hovered_id == Some(id);
                let is_pressed = *enabled && pressed_id == Some(id);
                let is_focused = *enabled && bar.focused_index == Some(vis.item_idx);
                let intent_fg = spec.as_ref().and_then(|s| match s.variant {
                    crate::primitives::button::ButtonVariant::Danger => Some(theme.error_fg),
                    crate::primitives::button::ButtonVariant::Warning => Some(theme.warning_fg),
                    _ => None,
                });
                let (cell_fg, cell_bg) = if !*enabled {
                    (muted, bar_bg)
                } else if is_pressed || *is_active {
                    (fg, active_bg)
                } else if is_hovered {
                    (hover_fg, hover_bg)
                } else if is_focused {
                    (fg, focus_bg)
                } else if let Some(intent) = intent_fg {
                    (qc(intent), bar_bg)
                } else {
                    (fg, bar_bg)
                };

                let mut cells: Vec<char> = Vec::new();
                cells.push('[');
                cells.push(' ');
                cells.extend(action_content_cells(
                    icon.as_deref(),
                    label,
                    key_hint.as_deref(),
                ));
                cells.push(' ');
                cells.push(']');

                for dy in 0..row_h {
                    for dx in 0..item_w {
                        set_cell(buf, item_x + dx, item_y + dy, ' ', cell_fg, cell_bg);
                    }
                }
                let mut col: u16 = 0;
                for ch in cells {
                    let cw = char_cell_width(ch);
                    if col + cw > item_w {
                        break;
                    }
                    set_cell(buf, item_x + col, text_row, ch, cell_fg, cell_bg);
                    col += cw;
                }
            }
            ToolbarButton::Separator => {
                for dy in 0..row_h {
                    if item_w >= 1 {
                        set_cell(buf, item_x, item_y + dy, ' ', muted, bar_bg);
                    }
                    if item_w >= 2 {
                        set_cell(buf, item_x + 1, item_y + dy, '│', muted, bar_bg);
                    }
                }
            }
            ToolbarButton::Label { text, fg: label_fg } => {
                let color = label_fg.map(qc).unwrap_or(muted);
                let mut col: u16 = 0;
                for ch in text.chars() {
                    let cw = char_cell_width(ch);
                    if col + cw > item_w {
                        break;
                    }
                    set_cell(buf, item_x + col, text_row, ch, color, bar_bg);
                    col += cw;
                }
            }
            // Already painted by the bar-background fill loop above, and
            // never clickable.
            ToolbarButton::Spacer => {}
            ToolbarButton::Group { items, .. } => {
                draw_group_horizontal(
                    buf,
                    item_y..item_y + row_h,
                    vis,
                    items,
                    hovered_id,
                    pressed_id,
                    text_row,
                    bar_bg,
                    fg,
                    muted,
                    hover_bg,
                    hover_fg,
                    active_bg,
                );
            }
        }
    }

    layout
}

/// Draw a [`Toolbar`] shaped as a vertical icon rail — the shape
/// [`crate::primitives::activity_bar::ActivityBar::to_toolbar_parts`]
/// produces — honouring its [`ToolbarRailStyle`] sidecar: the active-row
/// background fill, the leading-edge `'▎'` accent glyph, the explicit (or
/// `Modifier::REVERSED` fallback) keyboard-selection fill, and the
/// trailing-edge separator column. None of those four are something
/// [`draw_toolbar_with_options`] knows about — a plain `Toolbar` has no
/// concept of a row fill, an accent line, a selection fill distinct from
/// hover, or a separator column.
///
/// Always lays out [`ToolbarOrientation::Vertical`], one item per row,
/// via [`rail_layout_options`] — the fixed shape every rail-style
/// `Toolbar` uses — so this function takes no `ToolbarLayoutOptions` of
/// its own. `bar.focused_index` is read as the keyboard-selected item;
/// `hovered_id` and each button's own `is_active` resolve the rest of
/// the per-row state. Only [`ToolbarButton::Action`] rows paint anything
/// beyond the bar background; a [`ToolbarButton::Spacer`] row (the only
/// other variant a rail-shaped `Toolbar` contains) is left as the plain
/// background fill.
///
/// `bar.bg` is the rail's background (`None` falls back to
/// `theme.header_bg`, same default [`draw_toolbar_with_options`] uses).
/// The hover tint lightens that background by 10% — the TUI activity-bar
/// painter's own `theme.tab_bar_bg.lighten(0.10)` formula, carried
/// through unchanged since `bar.bg` is `theme.tab_bar_bg` for every
/// caller today.
///
/// The leading column (`area.x`) is always reserved for the accent glyph,
/// whether or not [`ToolbarRailStyle::active_accent`] is set — same
/// unconditional reservation as the trailing separator column, when
/// [`ToolbarRailStyle::trailing_separator`] is `Some`. Icon glyphs centre
/// within the room left between those two columns (or between the
/// leading column and the bar's trailing edge, when no separator is
/// requested).
pub fn draw_toolbar_with_rail_style(
    buf: &mut Buffer,
    area: Rect,
    bar: &Toolbar,
    rail_style: &ToolbarRailStyle,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
) -> ToolbarLayout {
    if area.width == 0 || area.height == 0 {
        return ToolbarLayout::empty(crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ));
    }

    let layout = tui_toolbar_layout_with_options(bar, area, rail_layout_options());

    let bg = qc(bar.bg.unwrap_or(theme.header_bg));
    let sep_color = rail_style.trailing_separator.map(qc);
    let accent = rail_style.active_accent.map(qc);
    let active_bg = rail_style.active_bg.map(qc);
    let active_fg = qc(theme.foreground);
    let inactive_fg = qc(theme.inactive_fg);
    let hover_bg = qc(theme.tab_bar_bg.lighten(0.10));
    // Explicit keyboard-selection background, when the sidecar sets one.
    // `None` falls back to `Modifier::REVERSED` after the icon paint —
    // terminal-agnostic inversion that works in every colour scheme.
    let sel_explicit_bg = rail_style.selection_bg.map(qc);

    // Fill background across the whole area.
    for row in area.y..area.y + area.height {
        for col in area.x..area.x + area.width {
            set_cell(buf, col, row, ' ', bg, bg);
        }
    }

    // Trailing-edge separator column — only when the sidecar requests one.
    let sep_col = sep_color.map(|_| area.x + area.width - 1);
    if let (Some(col), Some(color)) = (sep_col, sep_color) {
        for row in area.y..area.y + area.height {
            set_cell(buf, col, row, '│', color, bg);
        }
    }
    let content_start = area.x + 1; // after the leading accent column
    let content_end = sep_col.unwrap_or(area.x + area.width);

    for vis in &layout.visible_items {
        if vis.kind != ToolbarItemKind::Action || vis.bounds.height <= 0.0 {
            continue;
        }
        let y = vis.bounds.y.round() as u16;
        if y >= area.y + area.height {
            continue;
        }
        let (id, icon, is_active) = match &bar.buttons[vis.item_idx] {
            ToolbarButton::Action {
                id,
                icon,
                is_active,
                ..
            } => (id, icon, *is_active),
            _ => continue,
        };

        let is_hovered = hovered_id == Some(id);
        let is_keyboard_selected = bar.focused_index == Some(vis.item_idx);

        // Effective row background priority: explicit keyboard-selection >
        // hover > the active-row fill. When keyboard-selected with no
        // explicit `selection_bg`, the row keeps the default/hover
        // background and `Modifier::REVERSED` is applied after the icon
        // paint instead (so every cell inverts, giving a clear cursor in
        // any terminal colour scheme).
        let row_bg = if is_keyboard_selected {
            sel_explicit_bg.unwrap_or(if is_hovered { hover_bg } else { bg })
        } else if is_hovered {
            hover_bg
        } else if is_active {
            active_bg.unwrap_or(bg)
        } else {
            bg
        };
        let fg = if is_active || is_hovered || is_keyboard_selected {
            active_fg
        } else {
            inactive_fg
        };

        // Row background fill (hover tint, explicit keyboard-selection
        // bg, or the active-row fill) — only when there is actually a
        // tint to apply over the plain bar background.
        if is_hovered
            || (is_keyboard_selected && sel_explicit_bg.is_some())
            || (is_active && active_bg.is_some())
        {
            for col in area.x..content_end {
                set_cell(buf, col, y, ' ', fg, row_bg);
            }
        }

        // Leading-edge accent glyph for active items — only when the
        // sidecar opts in via `active_accent`. `None` paints zero
        // accent-line pixels.
        if is_active {
            if let Some(ac) = accent {
                set_cell(buf, area.x, y, '▎', ac, row_bg);
            }
        }

        // Icon glyph — centred in the room between the leading accent
        // column and the trailing edge (separator column, when present).
        let icon_str = icon.as_deref().unwrap_or("");
        let icon_ch = icon_str.chars().next().unwrap_or(' ');
        let content_w = content_end.saturating_sub(content_start);
        let icon_w = char_cell_width(icon_ch).max(1);
        if content_w >= icon_w {
            let icon_x = content_start + (content_w - icon_w) / 2;
            if icon_w >= 2 {
                set_cell_wide(buf, icon_x, y, icon_ch, fg, row_bg);
            } else {
                set_cell(buf, icon_x, y, icon_ch, fg, row_bg);
            }
        }

        // Keyboard-selection highlight — `Modifier::REVERSED` fallback
        // when the sidecar sets no explicit `selection_bg`. Applied last
        // so it overrides any earlier colour.
        if is_keyboard_selected && sel_explicit_bg.is_none() {
            for col in area.x..content_end {
                let cell = &mut buf[(col, y)];
                cell.modifier |= Modifier::REVERSED;
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::{ToolbarButton, ToolbarHit};
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
        ToolbarButton::Action {
            spec: None,
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn mk_action_with_icon(id: &str, label: &str, icon: &str) -> ToolbarButton {
        ToolbarButton::Action {
            spec: None,
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: Some(icon.to_string()),
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    #[test]
    fn draws_action_buttons_with_brackets() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Refine", true)],
            bg: None,
            focused_index: None,
        };
        let _layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // First two cells should be `[` then ` `.
        assert_eq!(cell_char(&buf, 0, 0), '[');
        assert_eq!(cell_char(&buf, 1, 0), ' ');
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `tui_toolbar_layout` bakes `area.x`/`area.y` straight into
    // `bar.layout`'s returned bounds (absolute frame), so the regression
    // guard is a full paint+hit_test round trip re-run at a non-zero
    // origin.
    fn click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Refine", true), mk_action("b", "Drop", true)],
            bg: None,
            focused_index: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // Click inside the first button.
        let b = layout.visible_items[0].bounds;
        assert_eq!(
            b.x, origin_x as f32,
            "bounds should be absolute (area.x baked in)"
        );
        assert_eq!(
            b.y, origin_y as f32,
            "bounds should be absolute (area.y baked in)"
        );
        let hit = layout.hit_test(b.x + 1.0, b.y);
        match hit {
            ToolbarHit::Button(id) => assert_eq!(id.as_str(), "a"),
            _ => panic!("expected button-a hit"),
        }
        // Click inside the second.
        let b = layout.visible_items[1].bounds;
        let hit = layout.hit_test(b.x + 1.0, b.y);
        match hit {
            ToolbarHit::Button(id) => assert_eq!(id.as_str(), "b"),
            _ => panic!("expected button-b hit"),
        }
    }

    #[test]
    fn click_round_trip_through_layout() {
        click_round_trip_at(0, 0);
    }

    /// Verify that the same click round trip holds when the toolbar isn't
    /// painted at the screen origin.
    #[test]
    fn click_round_trip_through_layout_at_nonzero_origin() {
        click_round_trip_at(7, 13);
    }

    /// The painted glyphs of row `y`, `width` cells starting at `x` —
    /// trimmed of trailing spaces so a test's expected string doesn't
    /// have to pad out to the full row width.
    fn row_text(buf: &Buffer, x: u16, y: u16, width: u16) -> String {
        (x..x + width)
            .map(|cx| cell_char(buf, cx, y))
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn cell_bg(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
        buf[(x, y)]
            .style()
            .bg
            .unwrap_or(ratatui::style::Color::Reset)
    }

    fn mk_group(id: &str, exclusive: bool, items: Vec<ToolbarButton>) -> ToolbarButton {
        ToolbarButton::Group {
            id: WidgetId::new(id),
            exclusive,
            label: None,
            items,
        }
    }

    /// A `[ a │ b │ c ]`-style exclusive group: one shared bracket pair
    /// around the whole cluster, `│` dividers between children, a click
    /// on the third segment's own body resolves through `hit_test` to
    /// *its* id (never the group's), and the active child paints
    /// `theme.selected_bg` while its siblings don't.
    #[test]
    fn group_click_round_trip_resolves_to_the_clicked_child_and_paints_it_active() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_group(
                "view",
                true,
                vec![
                    mk_action("icons", "Icons", true),
                    mk_action("list", "List", true),
                    mk_action("columns", "Columns", true),
                ],
            )],
            bg: None,
            focused_index: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &theme, None, None);

        let group = &layout.visible_items[0];
        assert_eq!(group.group_children.len(), 3);
        let group_x = group.bounds.x.round() as u16;
        let group_right = group_x + group.bounds.width.round() as u16 - 1;
        assert_eq!(
            cell_char(&buf, group_x, 0),
            '[',
            "group's own leading bracket"
        );
        assert_eq!(
            cell_char(&buf, group_right, 0),
            ']',
            "group's own trailing bracket"
        );

        let columns = &group.group_children[2];
        let divider_x = columns.bounds.x.round() as u16 - 1;
        assert_eq!(
            cell_char(&buf, divider_x, 0),
            '│',
            "divider before the third segment"
        );

        // The whole row is one shared bracket pair with one `│` between
        // each child and no bracket of any child's own — not
        // `[[ Icons ]│[ List ]│[ Columns ]]`.
        assert_eq!(
            row_text(&buf, 0, 0, area.width),
            "[  Icons  │  List  │  Columns  ]"
        );

        // A click anywhere in "Columns"'s own body resolves to *its* id,
        // not the group's ("view").
        let hit = layout.hit_test(columns.bounds.x + 1.0, columns.bounds.y);
        match hit {
            ToolbarHit::Button(id) => assert_eq!(id.as_str(), "columns"),
            _ => panic!("expected the third segment's own id, got {:?}", hit),
        }

        // No child carries `is_active` yet, so none of them paint
        // `selected_bg`.
        assert_ne!(
            cell_bg(&buf, columns.bounds.x.round() as u16, 0),
            qc(theme.selected_bg)
        );

        // Re-render with "Columns" the only active child (the host's own
        // response to the click above) and confirm it now paints
        // reversed/selected while "Icons" no longer does.
        let mut buf2 = Buffer::empty(area);
        let bar_active = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_group(
                "view",
                true,
                vec![
                    {
                        let mut a = mk_action("icons", "Icons", true);
                        if let ToolbarButton::Action { is_active, .. } = &mut a {
                            *is_active = false;
                        }
                        a
                    },
                    mk_action("list", "List", true),
                    {
                        let mut a = mk_action("columns", "Columns", true);
                        if let ToolbarButton::Action { is_active, .. } = &mut a {
                            *is_active = true;
                        }
                        a
                    },
                ],
            )],
            bg: None,
            focused_index: None,
        };
        let layout2 = draw_toolbar(&mut buf2, area, &bar_active, &theme, None, None);
        let columns2 = &layout2.visible_items[0].group_children[2];
        let icons2 = &layout2.visible_items[0].group_children[0];
        assert_eq!(
            cell_bg(&buf2, columns2.bounds.x.round() as u16, 0),
            qc(theme.selected_bg),
            "the now-active \"Columns\" segment must paint theme.selected_bg"
        );
        assert_ne!(
            cell_bg(&buf2, icons2.bounds.x.round() as u16, 0),
            qc(theme.selected_bg),
            "\"Icons\" must lose the active fill once it's no longer the active child"
        );
    }

    #[test]
    fn disabled_action_not_clickable_after_paint() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Refine", false)],
            bg: None,
            focused_index: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        let b = layout.visible_items[0].bounds;
        assert_eq!(layout.hit_test(b.x + 1.0, b.y), ToolbarHit::Empty);
    }

    #[test]
    fn separator_draws_pipe() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Separator],
            bg: None,
            focused_index: None,
        };
        let _layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // First cell is space, second is the pipe char.
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(cell_char(&buf, 1, 0), '│');
    }

    #[test]
    fn label_draws_text() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Label {
                text: "2/5".into(),
                fg: None,
            }],
            bg: None,
            focused_index: None,
        };
        let _layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(cell_char(&buf, 0, 0), '2');
        assert_eq!(cell_char(&buf, 1, 0), '/');
        assert_eq!(cell_char(&buf, 2, 0), '5');
    }

    #[test]
    fn zero_size_area_is_noop() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "X", true)],
            bg: None,
            focused_index: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert!(layout.visible_items.is_empty());
        assert_eq!(layout.hit_test(0.0, 0.0), ToolbarHit::Empty);
    }

    /// A zero-height area must not produce hit-testable buttons. Even though
    /// `tui_toolbar_layout` forces `bar_height` to `area.height.max(1)`, a
    /// `height == 0` area should return an empty layout with no clickable
    /// buttons, not invisible-but-clickable buttons for a row that was never painted.
    #[test]
    fn zero_height_area_has_no_hit_testable_buttons() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 10, 0);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "X", true)],
            bg: None,
            focused_index: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert!(layout.visible_items.is_empty());
        // Pre-fix, the forced `bar_height.max(1)` row band at y == 0
        // would hit-test the button here even though nothing painted.
        assert_eq!(layout.hit_test(1.0, 0.0), ToolbarHit::Empty);
    }

    // ── Gap 1: multi-row rasteriser ─────────────────────────────────────

    #[test]
    fn multi_row_fills_background_across_all_rows() {
        // 3-row bar with a single action button. Every cell of every
        // row must be painted (not just row 0).
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Go", true)],
            bg: None,
            focused_index: None,
        };
        let _ = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);

        // Row 0 (top): button text painted in centre — but the bg
        // outside the button text still fills.
        // Row 1 (middle): button text is here (centred vertically).
        // Row 2 (bottom): background fill only.
        //
        // Pre-Gap-1 behaviour would leave row 1 and row 2 untouched
        // (default Buffer cell symbol = " " but with default colours
        // not the bar's). Use any non-text cell (last column past the
        // button) — it should carry the bar bg every row.
        for y in 0..area.height {
            let cell = &buf[(area.width - 1, y)];
            assert_eq!(
                cell.symbol(),
                " ",
                "row {} last column not background-filled",
                y
            );
        }
    }

    #[test]
    fn multi_row_center_valign_centres_button_text_vertically() {
        // Height 3, `ToolbarVAlign::Center` → text on row 1 (middle).
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Go", true)],
            bg: None,
            focused_index: None,
        };
        let options = ToolbarPaintOptions {
            valign: ToolbarVAlign::Center,
        };
        let _ =
            draw_toolbar_with_options(&mut buf, area, &bar, &Theme::default(), None, None, options);
        // Row 1 col 0 should be `[`.
        assert_eq!(cell_char(&buf, 0, 1), '[');
        // Rows 0 and 2 should not have the bracket.
        assert_ne!(cell_char(&buf, 0, 0), '[');
        assert_ne!(cell_char(&buf, 0, 2), '[');
    }

    #[test]
    fn multi_row_default_valign_is_top() {
        // `ToolbarPaintOptions::default()` → `ToolbarVAlign::Top` → text
        // on row 0, padding below (the default).
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Go", true)],
            bg: None,
            focused_index: None,
        };
        let _ = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(cell_char(&buf, 0, 0), '[');
        assert_ne!(cell_char(&buf, 0, 1), '[');
        assert_ne!(cell_char(&buf, 0, 2), '[');
    }

    #[test]
    fn multi_row_bottom_valign_puts_text_on_last_row() {
        // `ToolbarVAlign::Bottom` → text on the last row, padding above.
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Go", true)],
            bg: None,
            focused_index: None,
        };
        let options = ToolbarPaintOptions {
            valign: ToolbarVAlign::Bottom,
        };
        let _ =
            draw_toolbar_with_options(&mut buf, area, &bar, &Theme::default(), None, None, options);
        assert_eq!(cell_char(&buf, 0, 2), '[');
        assert_ne!(cell_char(&buf, 0, 0), '[');
        assert_ne!(cell_char(&buf, 0, 1), '[');
    }

    #[test]
    fn one_row_slot_every_valign_resolves_identically() {
        // On a 1-row slot there's only one row to paint on — Top,
        // Center, and Bottom must all resolve to it, so this is
        // purely a multi-row concern (per `ToolbarVAlign`'s own doc).
        for valign in [
            ToolbarVAlign::Top,
            ToolbarVAlign::Center,
            ToolbarVAlign::Bottom,
        ] {
            let area = Rect::new(0, 0, 12, 1);
            let mut buf = Buffer::empty(area);
            let bar = Toolbar {
                id: WidgetId::new("tb"),
                buttons: vec![mk_action("a", "Go", true)],
                bg: None,
                focused_index: None,
            };
            let _ = draw_toolbar_with_options(
                &mut buf,
                area,
                &bar,
                &Theme::default(),
                None,
                None,
                ToolbarPaintOptions { valign },
            );
            assert_eq!(cell_char(&buf, 0, 0), '[', "valign={valign:?}");
        }
    }

    #[test]
    fn toolbar_valign_defaults_to_top_when_omitted_from_serde() {
        // An object that omits `valign` must deserialise to `Top`.
        #[derive(serde::Deserialize)]
        struct Wrapper {
            #[serde(default)]
            valign: ToolbarVAlign,
        }
        let w: Wrapper = serde_json::from_str("{}").unwrap();
        assert_eq!(w.valign, ToolbarVAlign::Top);

        // Serialises back to a tagged string the same wrapper can
        // round-trip.
        let json = serde_json::to_string(&ToolbarVAlign::Bottom).unwrap();
        let back: ToolbarVAlign = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ToolbarVAlign::Bottom);
    }

    #[test]
    fn multi_row_click_in_any_row_hits_button() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Go", true)],
            bg: None,
            focused_index: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        let r = layout.visible_items[0].bounds;
        // Click in each row of the button bounds — every row should
        // resolve to the button.
        for dy in 0..3 {
            let hit = layout.hit_test(r.x + 1.0, r.y + dy as f32);
            match hit {
                ToolbarHit::Button(ref id) => assert_eq!(id.as_str(), "a"),
                _ => panic!("row {dy} did not hit button"),
            }
        }
    }

    #[test]
    fn multi_row_separator_extends_full_height() {
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Separator],
            bg: None,
            focused_index: None,
        };
        let _ = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // The │ glyph should appear on every row at column 1.
        for y in 0..area.height {
            assert_eq!(cell_char(&buf, 1, y), '│', "row {y} missing pipe");
        }
    }

    // ── Gap 4: icon-only buttons + wide-glyph icons ─────────────────────

    #[test]
    fn icon_only_button_compacts_to_three_plus_icon() {
        // Icon-only button: `[ X ]` = 5 cells (4 wrapper + 1-cell icon).
        // The pre-Gap-4 layout would emit `[ X  ]` = 6 cells (trailing
        // space after the icon).
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                spec: None,
                id: WidgetId::new("a"),
                label: "".into(),
                icon: Some("X".into()),
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(layout.visible_items[0].bounds.width, 5.0);
        // Painted: `[ X ]`
        assert_eq!(cell_char(&buf, 0, 0), '[');
        assert_eq!(cell_char(&buf, 1, 0), ' ');
        assert_eq!(cell_char(&buf, 2, 0), 'X');
        assert_eq!(cell_char(&buf, 3, 0), ' ');
        assert_eq!(cell_char(&buf, 4, 0), ']');
    }

    #[test]
    fn double_width_icon_reserves_two_cells() {
        // CJK or emoji glyphs occupy 2 cells. The measurer must
        // reserve that width so the right bracket lands at the
        // expected column. Use 一 (U+4E00) — explicit 2-cell width.
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                spec: None,
                id: WidgetId::new("a"),
                label: "Go".into(),
                icon: Some("一".into()),
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
        };
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        // "[ " (2) + icon (2 cells) + " " (1) + "Go" (2) + " ]" (2) = 9
        assert_eq!(layout.visible_items[0].bounds.width, 9.0);
    }

    fn cell_fg(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
        buf[(x, y)]
            .style()
            .fg
            .unwrap_or(ratatui::style::Color::Reset)
    }

    /// An `Action` with an explicit `Danger` spec paints its resting
    /// foreground in `theme.error_fg` instead of the plain
    /// `theme.foreground` — same intent-colour convention
    /// `crate::tui::button` uses for a standalone `Button`.
    #[test]
    fn action_with_danger_spec_paints_error_fg_at_rest() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut spec = crate::primitives::button::ButtonSpec::new("Delete");
        spec.variant = crate::primitives::button::ButtonVariant::Danger;
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                spec: Some(spec),
                id: WidgetId::new("a"),
                label: "Delete".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
        };

        draw_toolbar(&mut buf, area, &bar, &theme, None, None);

        assert_eq!(cell_fg(&buf, 2, 0), qc(theme.error_fg));
    }

    /// A legacy-only `Action` (`spec: None`, every pre-existing
    /// construction site) keeps painting `theme.foreground` at rest,
    /// unaffected by the intent-colour path above.
    #[test]
    fn action_without_spec_paints_plain_foreground_at_rest() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Delete", true)],
            bg: None,
            focused_index: None,
        };

        draw_toolbar(&mut buf, area, &bar, &theme, None, None);

        assert_eq!(cell_fg(&buf, 2, 0), qc(theme.foreground));
    }

    // ── `ToolbarOrientation::Vertical` / `ToolbarButton::Spacer` ───────

    /// `draw_toolbar_oriented` with the default `Horizontal` orientation
    /// paints byte-for-byte the same buffer as `draw_toolbar` — no
    /// behaviour change for every existing horizontal caller.
    #[test]
    fn oriented_horizontal_matches_plain_draw_toolbar() {
        let area = Rect::new(0, 0, 40, 1);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Refine", true), mk_action("b", "Drop", true)],
            bg: None,
            focused_index: None,
        };

        let mut plain_buf = Buffer::empty(area);
        let plain_layout = draw_toolbar(&mut plain_buf, area, &bar, &Theme::default(), None, None);

        let mut oriented_buf = Buffer::empty(area);
        let oriented_layout = draw_toolbar_oriented(
            &mut oriented_buf,
            area,
            &bar,
            &Theme::default(),
            None,
            None,
            ToolbarLayoutOptions::default(),
        );

        assert_eq!(plain_buf, oriented_buf);
        assert_eq!(plain_layout, oriented_layout);
    }

    /// `ToolbarOrientation::Vertical` degrades to one item per terminal
    /// row — the first action's bracket cells paint on row 0, the
    /// second's on row 1.
    #[test]
    fn vertical_degrades_to_one_item_per_row() {
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Run", true), mk_action("b", "Stop", true)],
            bg: None,
            focused_index: None,
        };
        let options = ToolbarLayoutOptions::new().with_orientation(ToolbarOrientation::Vertical);

        let layout =
            draw_toolbar_oriented(&mut buf, area, &bar, &Theme::default(), None, None, options);

        assert_eq!(layout.visible_items.len(), 2);
        assert_eq!(layout.visible_items[0].bounds.y, 0.0);
        assert_eq!(layout.visible_items[1].bounds.y, 1.0);
        // Every row spans the full area width.
        assert_eq!(layout.visible_items[0].bounds.width, 20.0);

        assert_eq!(cell_char(&buf, 0, 0), '[');
        assert_eq!(cell_char(&buf, 0, 1), '[');
        // Unused rows stay blank (no stray brackets below the items).
        assert_ne!(cell_char(&buf, 0, 2), '[');

        let hit = layout.hit_test(1.0, 1.0);
        match hit {
            ToolbarHit::Button(id) => assert_eq!(id.as_str(), "b"),
            _ => panic!("expected row 1 to hit button b, got {hit:?}"),
        }
    }

    /// A `Spacer` in a vertical bar still pushes the trailing item to the
    /// bottom row, same push semantics as the horizontal case.
    #[test]
    fn vertical_spacer_pushes_trailing_item_to_the_bottom_row() {
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("a", "Top", true),
                ToolbarButton::Spacer,
                mk_action("b", "Bottom", true),
            ],
            bg: None,
            focused_index: None,
        };
        let options = ToolbarLayoutOptions::new().with_orientation(ToolbarOrientation::Vertical);

        let layout =
            draw_toolbar_oriented(&mut buf, area, &bar, &Theme::default(), None, None, options);

        assert_eq!(layout.visible_items[0].bounds.y, 0.0);
        assert_eq!(
            layout.visible_items[2].bounds.y, 4.0,
            "flush to the last row"
        );
        assert_eq!(cell_char(&buf, 0, 4), '[');
    }

    /// A `Group` too wide for its area degrades in steps: full labels
    /// when there's room, icon-only children when there isn't, and flat
    /// (ungrouped) items — each its own independently bracketed, flush
    /// top-level item — as the last resort. Never just clipped mid-glyph.
    #[test]
    fn narrow_group_degrades_icon_only_then_flat() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_group(
                "nav",
                false,
                vec![
                    mk_action_with_icon("back", "Back", "◀"),
                    mk_action_with_icon("forward", "Forward", "▶"),
                ],
            )],
            bg: None,
            focused_index: None,
        };

        // Full width (26 cells needed): both children keep their labels
        // inside one shared group.
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(layout.visible_items.len(), 1);
        assert_eq!(layout.visible_items[0].kind, ToolbarItemKind::Group);
        let row = row_text(&buf, 0, 0, area.width);
        assert!(row.contains("Back"), "{row:?}");
        assert!(row.contains("Forward"), "{row:?}");

        // Narrower (13 cells needed at icon-only, 26 at full): labels
        // drop, icons stay, still one shared group.
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(layout.visible_items.len(), 1);
        assert_eq!(layout.visible_items[0].kind, ToolbarItemKind::Group);
        let row = row_text(&buf, 0, 0, area.width);
        assert!(!row.contains("Back"), "label should drop: {row:?}");
        assert!(row.contains('◀'), "icon should stay: {row:?}");

        // Narrower still (below the 13-cell icon-only floor): the group
        // itself disappears — each child becomes its own flat,
        // independently bracketed top-level item.
        let area = Rect::new(0, 0, 12, 1);
        let mut buf = Buffer::empty(area);
        let layout = draw_toolbar(&mut buf, area, &bar, &Theme::default(), None, None);
        assert_eq!(
            layout.visible_items.len(),
            2,
            "the group's 2 children flatten into 2 top-level items"
        );
        assert_eq!(layout.visible_items[0].kind, ToolbarItemKind::Action);
        assert_eq!(layout.visible_items[1].kind, ToolbarItemKind::Action);
        assert_eq!(
            cell_char(&buf, 0, 0),
            '[',
            "the first flattened child gets its own bracket"
        );
        match layout.hit_test(1.0, 0.0) {
            ToolbarHit::Button(id) => assert_eq!(id.as_str(), "back"),
            other => panic!("expected the flattened child's own id, got {other:?}"),
        }
    }

    /// A zero-height vertical area returns an empty, non-hit-testable
    /// layout — same degenerate-area contract `draw_toolbar_with_options`
    /// already has for a zero-height horizontal bar.
    #[test]
    fn vertical_zero_height_area_is_noop() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 10, 0);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "X", true)],
            bg: None,
            focused_index: None,
        };
        let options = ToolbarLayoutOptions::new().with_orientation(ToolbarOrientation::Vertical);

        let layout =
            draw_toolbar_oriented(&mut buf, area, &bar, &Theme::default(), None, None, options);

        assert!(layout.visible_items.is_empty());
        assert_eq!(layout.hit_test(0.0, 0.0), ToolbarHit::Empty);
    }

    // ── ToolbarOverflow ─────────────────────────────────────────────────

    /// `ToolbarOverflow::Wrap` on a bar too narrow for one row paints the
    /// overflow buttons on additional terminal rows below the first, and
    /// `cross_extent` reports exactly how many rows that took — the value
    /// a host reads before growing `area` to fit.
    #[test]
    fn overflow_wrap_paints_extra_terminal_rows() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("a", "AAAA", true),
                mk_action("b", "BBBB", true),
                mk_action("c", "CCCC", true),
            ],
            bg: None,
            focused_index: None,
        };
        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Wrap);
        let priorities = ToolbarOverflowPriorities::new();

        // Each `[ XXXX ]` button is 8 cells; a 10-cell-wide bar fits one
        // per row, so 3 buttons need 3 rows.
        let probe_area = Rect::new(0, 0, 10, 1);
        let probe = tui_toolbar_layout_with_overflow(&bar, probe_area, options, &priorities);
        let rows_needed = probe.cross_extent(ToolbarOrientation::Horizontal) as u16;
        assert_eq!(rows_needed, 3, "one button per row at this width");

        let area = Rect::new(0, 0, 10, rows_needed);
        let mut buf = Buffer::empty(area);
        let layout = draw_toolbar_with_overflow(
            &mut buf,
            area,
            &bar,
            &Theme::default(),
            None,
            None,
            options,
            &priorities,
        );

        assert_eq!(cell_char(&buf, 0, 0), '[', "row 0 has a button");
        assert_eq!(cell_char(&buf, 0, 1), '[', "row 1 has a button");
        assert_eq!(cell_char(&buf, 0, 2), '[', "row 2 has a button");
        match layout.hit_test(1.0, 2.0) {
            ToolbarHit::Button(id) => assert_eq!(id.as_str(), "c"),
            other => panic!("expected the third button on row 2, got {other:?}"),
        }
    }

    /// `ToolbarOverflow::Menu` on a bar too narrow for every button paints
    /// a trailing `[ » ]` cell in place of whatever it dropped, and a
    /// click there hit-tests to `ToolbarHit::Overflow`.
    #[test]
    fn overflow_menu_paints_bracket_and_hit_tests_overflow() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("a", "AAAA", true),
                mk_action("b", "BBBB", true),
                mk_action("c", "CCCC", true),
            ],
            bg: None,
            focused_index: None,
        };
        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Menu);
        let priorities = ToolbarOverflowPriorities::new();

        // Each `[ XXXX ]` button is 8 cells and the `[ » ]` affordance is
        // 5 — only one button plus the affordance fits in 14 cells, so
        // two of the three buttons must drop.
        let area = Rect::new(0, 0, 14, 1);
        let mut buf = Buffer::empty(area);
        let layout = draw_toolbar_with_overflow(
            &mut buf,
            area,
            &bar,
            &Theme::default(),
            None,
            None,
            options,
            &priorities,
        );

        let overflow_vis = layout
            .visible_items
            .iter()
            .find(|v| v.kind == crate::primitives::toolbar::ToolbarItemKind::Overflow)
            .expect("overflow affordance present");
        assert_eq!(
            overflow_vis.item_idx,
            crate::primitives::toolbar::TOOLBAR_OVERFLOW_ITEM_IDX
        );
        let ox = overflow_vis.bounds.x.round() as u16;
        assert_eq!(cell_char(&buf, ox, 0), '[');
        assert_eq!(cell_char(&buf, ox + 1, 0), ' ');
        assert_eq!(cell_char(&buf, ox + 2, 0), '»');
        assert_eq!(
            layout.hit_test(overflow_vis.bounds.x, overflow_vis.bounds.y),
            ToolbarHit::Overflow
        );
        assert_eq!(layout.overflowed_items().len(), 2);
    }
}
