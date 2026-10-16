//! TUI rasteriser for [`crate::ContextMenu`].
//!
//! Box-bordered popup with one row per item. Selected clickable items
//! render inverted (fg/bg swapped); separators draw as a horizontal
//! dash; disabled items render dimmed. Items with `checked == Some(true)`
//! show a leading `✓` glyph in the row's left-padding column. Items with
//! `submenu.is_some()` show a `▶` glyph at the far-right column instead
//! of a keyboard-shortcut hint. The right-aligned shortcut hint is sourced
//! from `item.detail` if set (back-compat), otherwise from
//! `render_accelerator(item.key_equivalent, Platform::Tui)`.
//!
//! [`draw_context_menu_with_submenus`] extends the basic rasteriser to
//! paint the full stack of open nested (pull-right) submenus in one call.

use ratatui::buffer::Buffer;

use super::{ratatui_color, set_cell};
use crate::accelerator::{render_accelerator, Platform};
use crate::primitives::context_menu::{
    ContextMenu, ContextMenuItem, ContextMenuItemMeasure, ContextMenuLayout, ContextMenuPlacement,
};
use crate::theme::Theme;

/// Cells [`draw_context_menu`] paints its box border on, outside the
/// item region, on each side. This is what
/// [`crate::tui::backend::TuiBackend`] reports as its
/// [`crate::Backend::context_menu_border_inset`], so the reservation a
/// caller makes and the chrome this rasteriser paints come from one
/// constant.
///
/// Re-exported as `quadraui::tui::CONTEXT_MENU_BORDER_INSET` for callers
/// that drive this rasteriser without a [`crate::Backend`] handle: inset
/// the viewport by it on all four sides before laying the menu out, the
/// way [`crate::Backend::context_menu_viewport`] does.
pub const BORDER_INSET: f32 = 1.0;

/// Returns the right-aligned shortcut text for `item`, sourced from
/// `item.detail` (preferred, back-compat) or rendered from
/// `item.key_equivalent`. Returns `None` if neither is set.
fn shortcut_text(item: &ContextMenuItem, platform: Platform) -> Option<String> {
    if let Some(ref det) = item.detail {
        return det.spans.first().map(|s| s.text.clone());
    }
    item.key_equivalent
        .as_ref()
        .map(|acc| render_accelerator(acc, platform))
}

/// Draw a [`ContextMenu`] popup.
///
/// `layout.bounds` is the **inner** items region; the box border paints
/// on a whole cell *outside* it on every side. For that border to have a
/// cell to occupy when the menu is anchored flush against a viewport
/// edge, the caller reserves it up front by passing
/// [`crate::Backend::context_menu_viewport`] — the surface inset on all
/// four sides by [`crate::Backend::context_menu_border_inset`] — as the
/// `viewport` of [`ContextMenu::layout`] / [`ContextMenu::layout_at`].
/// Reserving the cell in the layout, rather than shifting only where
/// this function paints, keeps painted rows and the rows
/// [`ContextMenuLayout::hit_test`] reports in the same coordinate space.
///
/// That inset is a *recommendation*, not a precondition: a layout built
/// from a non-inset viewport (so `bounds.x` or `bounds.y` rounds to `0`)
/// still renders, and still paints every item on the row its own
/// `bounds` reports. The consequence is cosmetic and confined to the
/// near edge — the border column/row has nowhere to go outside the
/// buffer, so it falls on the first item column/row, which is painted
/// after it and wins. Nothing here panics or wraps on such a layout.
pub fn draw_context_menu(
    buf: &mut Buffer,
    menu: &ContextMenu,
    layout: &ContextMenuLayout,
    theme: &Theme,
) {
    let bg = ratatui_color(theme.tab_bar_bg);
    let fg = ratatui_color(theme.foreground);
    let sep_fg = ratatui_color(theme.muted_fg);
    let dim_fg = ratatui_color(theme.muted_fg);

    let inner_x = layout.bounds.x.round() as u16;
    let inner_y = layout.bounds.y.round() as u16;
    let inner_w = layout.bounds.width.round() as u16;
    let inner_h = layout.bounds.height.round() as u16;
    if inner_w == 0 || inner_h == 0 {
        return;
    }
    // Border box: the ring of cells immediately outside the items
    // region. `bounds` comes from caller-supplied `f32` geometry, so a
    // viewport that was not inset by `Backend::context_menu_viewport`
    // can put the items region at column/row 0, leaving the near border
    // no cell inside the buffer to occupy. Saturate instead of
    // underflowing: the near border then shares the first item
    // column/row, which is painted afterwards and wins, so the only
    // consequence is a missing edge. Items always paint at the
    // coordinates `bounds` reports, keeping paint and `hit_test` in
    // agreement whichever way this lands.
    let left = inner_x.saturating_sub(1);
    let top = inner_y.saturating_sub(1);
    let right = inner_x.saturating_add(inner_w);
    let bottom = inner_y.saturating_add(inner_h);

    for cy in top..=bottom {
        for cx in left..=right {
            let ch = if cy == top {
                if cx == left {
                    '┌'
                } else if cx == right {
                    '┐'
                } else {
                    '─'
                }
            } else if cy == bottom {
                if cx == left {
                    '└'
                } else if cx == right {
                    '┘'
                } else {
                    '─'
                }
            } else if cx == left || cx == right {
                '│'
            } else {
                ' '
            };
            set_cell(buf, cx, cy, ch, fg, bg);
        }
    }

    for vis in &layout.visible_items {
        let item = &menu.items[vis.item_idx];
        let row_y = vis.bounds.y.round() as u16;
        if vis.is_separator {
            for dx in 0..inner_w {
                set_cell(buf, inner_x.saturating_add(dx), row_y, '─', sep_fg, bg);
            }
            continue;
        }
        let is_selected = vis.item_idx == menu.selected_idx;
        let (item_fg, item_bg) = if is_selected && vis.clickable {
            (bg, fg) // inverted
        } else if !vis.clickable {
            (dim_fg, bg)
        } else {
            (fg, bg)
        };
        for dx in 0..inner_w {
            set_cell(
                buf,
                inner_x.saturating_add(dx),
                row_y,
                ' ',
                item_fg,
                item_bg,
            );
        }
        // Check prefix — sits in the left-padding column at inner_x.
        // `Some(false)` reserves the slot without filling it (so a
        // column of mixed checked/unchecked items aligns visually).
        if matches!(item.checked, Some(true)) {
            set_cell(buf, inner_x, row_y, '✓', item_fg, item_bg);
        }
        let label = item
            .label
            .spans
            .first()
            .map(|s| s.text.as_str())
            .unwrap_or("");
        for (i, ch) in label.chars().enumerate() {
            let col = inner_x.saturating_add(1).saturating_add(i as u16);
            if col >= right {
                break;
            }
            set_cell(buf, col, row_y, ch, item_fg, item_bg);
        }
        if item.submenu.is_some() {
            // Submenu-parent: show ▶ at the far-right column.
            // Keyboard shortcuts are not rendered for submenu parents
            // (they open a child menu rather than dispatching an action).
            let arrow_fg = if is_selected && vis.clickable {
                item_fg
            } else {
                dim_fg
            };
            set_cell(buf, right.saturating_sub(1), row_y, '▶', arrow_fg, item_bg);
        } else if let Some(shortcut) = shortcut_text(item, Platform::Tui) {
            let sc_w = shortcut.chars().count().min(u16::MAX as usize) as u16;
            let sc_start = inner_x.saturating_add(inner_w.saturating_sub(sc_w.saturating_add(1)));
            let sc_fg = if is_selected && vis.clickable {
                item_fg
            } else {
                dim_fg
            };
            for (i, ch) in shortcut.chars().enumerate() {
                let col = sc_start.saturating_add(i as u16);
                if col >= right {
                    break;
                }
                set_cell(buf, col, row_y, ch, sc_fg, item_bg);
            }
        }
    }
}

#[doc(hidden)]
/// Draw a [`ContextMenu`] and any nested submenus that are currently open
/// within it (cascading submenus anchored to the right of their parent).
///
/// This function operates directly on a raw `ratatui::buffer::Buffer` and
/// bypasses the [`crate::backend::Backend`] trait. It exists as a lower-level
/// building block for unit tests; production rendering goes through
/// [`crate::compose::MenuSystem::render`] → `backend.draw_context_menu()`
/// per level. Consumers should use `MenuSystem::render` instead.
///
/// # Arguments
///
/// * `root_menu` / `root_layout` — the top-level menu, as produced by
///   [`ContextMenu::layout`].
/// * `submenu_path` — depth-first path of open submenus.
///   `submenu_path[0]` = `item_idx` in the root menu whose submenu is open;
///   `submenu_path[1]` = `item_idx` in *that* submenu whose sub-submenu is open;
///   etc. Empty slice → draw only the root.
/// * `selected_at_depth` — `selected_at_depth[d]` = selected item index
///   inside the submenu at depth `d` (depth 0 = first child of root).
/// * `viewport` — full paint surface; used for right-edge overflow detection.
///
/// Each child popup anchors its left edge to the parent popup's right
/// border column. If that would overflow the viewport's right edge the
/// popup flips to the left of the parent instead.
pub fn draw_context_menu_with_submenus(
    buf: &mut Buffer,
    root_menu: &ContextMenu,
    root_layout: &ContextMenuLayout,
    submenu_path: &[usize],
    selected_at_depth: &[usize],
    viewport: crate::event::Rect,
    theme: &Theme,
) {
    draw_context_menu(buf, root_menu, root_layout, theme);

    // Child layouts are computed here rather than by the caller, so this
    // function owns the border reservation for them: inset `viewport` on
    // all four sides by the cell `draw_context_menu` paints its box
    // border on, exactly as `Backend::context_menu_viewport` does for the
    // root. Every placement branch below reads `vp`, not `viewport`, so
    // neither the pull-left flip nor the clamp fallback can land a
    // child's inner region flush at the buffer's near corner.
    let vp = crate::event::Rect::new(
        viewport.x + BORDER_INSET,
        viewport.y + BORDER_INSET,
        (viewport.width - BORDER_INSET * 2.0).max(0.0),
        (viewport.height - BORDER_INSET * 2.0).max(0.0),
    );

    // Walk the open submenu chain.  We keep owned copies so Rust's borrow
    // checker lets us update `parent_*` at the end of each iteration.
    let mut parent_items: Vec<ContextMenuItem> = root_menu.items.clone();
    let mut parent_bounds = root_layout.bounds;
    let mut parent_vis: Vec<crate::primitives::context_menu::VisibleContextMenuItem> =
        root_layout.visible_items.clone();

    // Fixed geometry per level: 1 cell high per item (TUI cells, not pixels).
    const MENU_WIDTH: f32 = 20.0;

    for (depth, &path_idx) in submenu_path.iter().enumerate() {
        // Resolve submenu items at this depth.
        let sub_items = match parent_items
            .get(path_idx)
            .and_then(|item| item.submenu.clone())
        {
            Some(items) => items,
            None => break,
        };

        // Preferred anchor: right border of parent popup + 1 column.
        // Parent inner right = parent_bounds.x + parent_bounds.width
        // Parent right border column = inner_right (draw_context_menu places it at
        // inner_x + inner_w, i.e. bounds.x + bounds.width).
        // We start the child's inner region one column further right.
        let preferred_x = parent_bounds.x + parent_bounds.width + 1.0;

        // Fallback: flip to the left of the parent popup when the right side
        // would overflow the viewport.
        //   Child inner left = parent_bounds.x - MENU_WIDTH - 1
        //   (the −1 places the child's right border at parent_bounds.x − 1,
        //    leaving the parent's left border intact)
        let flipped_x = parent_bounds.x - MENU_WIDTH - 1.0;

        let anchor_x = if preferred_x + MENU_WIDTH <= vp.x + vp.width {
            preferred_x
        } else if flipped_x >= vp.x {
            flipped_x
        } else {
            // Neither side has enough room — clamp to viewport right.
            (vp.x + vp.width - MENU_WIDTH).max(vp.x)
        };

        // Anchor y = top of the row that triggered this submenu.
        let anchor_y = parent_vis
            .iter()
            .find(|v| v.item_idx == path_idx)
            .map(|v| v.bounds.y)
            .unwrap_or(parent_bounds.y);

        let selected = selected_at_depth.get(depth).copied().unwrap_or(0);

        let sub_menu = ContextMenu {
            id: crate::types::WidgetId::new("tui-submenu"),
            items: sub_items.clone(),
            selected_idx: selected,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        };

        // In TUI every row (item or separator) is exactly 1 cell tall.
        let sub_layout = sub_menu.layout(anchor_x, anchor_y, vp, MENU_WIDTH, |_| {
            ContextMenuItemMeasure::new(1.0)
        });

        draw_context_menu(buf, &sub_menu, &sub_layout, theme);

        // Advance to next depth.
        parent_items = sub_items;
        parent_bounds = sub_layout.bounds;
        parent_vis = sub_layout.visible_items;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::context_menu::{
        ContextMenu, ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure,
    };
    use crate::types::{StyledSpan, StyledText, WidgetId};
    use ratatui::layout::Rect;

    fn item(label: &str, clickable: bool) -> ContextMenuItem {
        ContextMenuItem {
            id: if clickable {
                Some(WidgetId::new(label))
            } else {
                None
            },
            label: StyledText {
                spans: vec![StyledSpan::plain(label)],
            },
            disabled: !clickable,
            ..Default::default()
        }
    }

    fn make_menu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![
                item("Open", true),
                item("Open to Side", true),
                // Separator: id = None.
                ContextMenuItem::default(),
                item("Delete", true),
            ],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_corner_glyphs_and_items() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        // Border corners around the inner items region (inset by 1).
        let bx = layout.bounds.x.round() as u16 - 1;
        let by = layout.bounds.y.round() as u16 - 1;
        let bw = layout.bounds.width.round() as u16 + 2;
        let bh = layout.bounds.height.round() as u16 + 2;
        assert_eq!(cell_char(&buf, bx, by), '┌');
        assert_eq!(cell_char(&buf, bx + bw - 1, by), '┐');
        assert_eq!(cell_char(&buf, bx, by + bh - 1), '└');
        assert_eq!(cell_char(&buf, bx + bw - 1, by + bh - 1), '┘');
    }

    #[test]
    fn separator_paints_horizontal_dashes() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        // The third visible item is a separator — find a row that's all '─'.
        let mut found_sep_row = false;
        for vis in &layout.visible_items {
            if vis.is_separator {
                let row_y = vis.bounds.y.round() as u16;
                let inner_x = layout.bounds.x.round() as u16;
                let inner_w = layout.bounds.width.round() as u16;
                let row: String = (inner_x..inner_x + inner_w)
                    .map(|x| cell_char(&buf, x, row_y))
                    .collect();
                assert!(row.chars().all(|c| c == '─'), "separator row: {:?}", row);
                found_sep_row = true;
                break;
            }
        }
        assert!(found_sep_row, "expected at least one separator row");
    }

    #[test]
    fn selected_clickable_inverted() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu(); // selected_idx = 0 → "Open"
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        let theme = Theme {
            tab_bar_bg: crate::types::Color::rgb(0, 0, 0),
            foreground: crate::types::Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        draw_context_menu(&mut buf, &menu, &layout, &theme);

        // Find the "Open" row's first cell (inner_x). The selected row has
        // inverted bg = foreground colour.
        let inner_x = layout.bounds.x.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        let bg = buf[(inner_x, row_y)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(255, 255, 255));
    }

    #[test]
    fn checked_item_paints_check_glyph_in_left_padding() {
        use crate::primitives::context_menu::ContextMenuItem;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![
                ContextMenuItem {
                    id: Some(WidgetId::new("toggle-sidebar")),
                    label: StyledText {
                        spans: vec![StyledSpan::plain("Toggle Sidebar")],
                    },
                    checked: Some(true),
                    ..Default::default()
                },
                ContextMenuItem {
                    id: Some(WidgetId::new("toggle-panel")),
                    label: StyledText {
                        spans: vec![StyledSpan::plain("Toggle Panel")],
                    },
                    checked: Some(false),
                    ..Default::default()
                },
            ],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        let inner_x = layout.bounds.x.round() as u16;
        let checked_row = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x, checked_row),
            '✓',
            "checked=Some(true) item should paint ✓ at inner_x",
        );
        let unchecked_row = layout.visible_items[1].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x, unchecked_row),
            ' ',
            "checked=Some(false) item should leave the slot blank",
        );
    }

    #[test]
    fn key_equivalent_renders_as_right_aligned_shortcut() {
        // key_equivalent set but detail unset → shortcut text comes
        // from render_accelerator(Platform::Tui).
        use crate::accelerator::{Accelerator, AcceleratorId, AcceleratorScope, KeyBinding};
        use crate::primitives::context_menu::ContextMenuItem;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("save")),
                label: StyledText {
                    spans: vec![StyledSpan::plain("Save")],
                },
                key_equivalent: Some(Accelerator {
                    id: AcceleratorId::new("editor.save"),
                    binding: KeyBinding::Save,
                    scope: AcceleratorScope::Global,
                    label: None,
                }),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        // Platform::Tui renders Save as "Ctrl+S" — look for that suffix
        // near the right edge of the row.
        let inner_x = layout.bounds.x.round() as u16;
        let inner_w = layout.bounds.width.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        let row: String = (inner_x..inner_x + inner_w)
            .map(|x| cell_char(&buf, x, row_y))
            .collect();
        assert!(
            row.contains("Ctrl+S"),
            "row should contain rendered key_equivalent: row was {row:?}",
        );
    }

    #[test]
    fn detail_wins_over_key_equivalent_for_back_compat() {
        // Pre-existing apps that set `detail` directly should keep
        // seeing that text — even when key_equivalent is also set.
        use crate::accelerator::{Accelerator, AcceleratorId, AcceleratorScope, KeyBinding};
        use crate::primitives::context_menu::ContextMenuItem;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("save")),
                label: StyledText {
                    spans: vec![StyledSpan::plain("Save")],
                },
                detail: Some(StyledText {
                    spans: vec![StyledSpan::plain("⌘S-legacy")],
                }),
                key_equivalent: Some(Accelerator {
                    id: AcceleratorId::new("editor.save"),
                    binding: KeyBinding::Save,
                    scope: AcceleratorScope::Global,
                    label: None,
                }),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        let inner_x = layout.bounds.x.round() as u16;
        let inner_w = layout.bounds.width.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        let row: String = (inner_x..inner_x + inner_w)
            .map(|x| cell_char(&buf, x, row_y))
            .collect();
        assert!(
            row.contains("legacy"),
            "detail string should win over key_equivalent: row was {row:?}",
        );
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        let menu = make_menu();
        let layout = menu.layout(
            0.0,
            0.0,
            crate::event::Rect::new(0.0, 0.0, 0.0, 0.0),
            0.0,
            |_| ContextMenuItemMeasure::new(0.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    // ── Submenu (▶ affordance + pull-right popup) ────────────────────────

    fn submenu_parent_menu() -> ContextMenu {
        use crate::types::{StyledSpan, StyledText, WidgetId};
        let sub_item = ContextMenuItem {
            id: Some(WidgetId::new("sub-a")),
            label: StyledText {
                spans: vec![StyledSpan::plain("SubItem")],
            },
            ..Default::default()
        };
        ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![
                ContextMenuItem {
                    id: Some(WidgetId::new("parent")),
                    label: StyledText {
                        spans: vec![StyledSpan::plain("Parent")],
                    },
                    submenu: Some(vec![sub_item]),
                    ..Default::default()
                },
                ContextMenuItem {
                    id: Some(WidgetId::new("leaf")),
                    label: StyledText {
                        spans: vec![StyledSpan::plain("Leaf")],
                    },
                    ..Default::default()
                },
            ],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
        }
    }

    #[test]
    fn submenu_parent_paints_arrow_affordance() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
        let menu = submenu_parent_menu();
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 40.0, 10.0),
            16.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        // The ▶ appears at the rightmost inner column of the parent item's row.
        let inner_x = layout.bounds.x.round() as u16;
        let inner_w = layout.bounds.width.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x + inner_w - 1, row_y),
            '▶',
            "submenu-parent item should show ▶ at rightmost inner column",
        );

        // A plain leaf item (no submenu) must not show ▶.
        let leaf_row_y = layout.visible_items[1].bounds.y.round() as u16;
        assert_ne!(
            cell_char(&buf, inner_x + inner_w - 1, leaf_row_y),
            '▶',
            "plain leaf item must not show ▶",
        );
    }

    #[test]
    fn submenu_popup_anchored_at_right_edge() {
        // Root menu inner at x=2, width=16.
        // Viewport 50 wide — plenty of room to the right.
        // Expected: child inner left = 2 + 16 + 1 = 19.
        //           child left border (┌) at (18, anchor_y − 1).
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 10));
        let menu = submenu_parent_menu();
        let viewport = crate::event::Rect::new(0.0, 0.0, 50.0, 10.0);
        let layout = menu.layout(2.0, 1.0, viewport, 16.0, |_| {
            ContextMenuItemMeasure::new(1.0)
        });

        // submenu_path = [0] means item 0 (the submenu-parent) is open.
        // selected_at_depth = [0] → first child selected.
        draw_context_menu_with_submenus(
            &mut buf,
            &menu,
            &layout,
            &[0],
            &[0],
            viewport,
            &Theme::default(),
        );

        // Root menu ┌ is at (inner_x − 1, inner_y − 1) = (1, 0).
        assert_eq!(cell_char(&buf, 1, 0), '┌', "root menu top-left corner");

        // Child menu: preferred_x = 2 + 16 + 1 = 19 (inner left).
        // Child left border (┌) at inner_x − 1 = 18.
        // anchor_y = bounds.y of visible_items[0] = 1.0
        // by = anchor_y − 1 = 0.
        assert_eq!(
            cell_char(&buf, 18, 0),
            '┌',
            "child popup top-left corner should be at x=18 (right-edge anchor)",
        );
    }

    #[test]
    fn submenu_overflow_flips_left() {
        // Viewport 50 wide.  Root menu inner at x=25, width=10.
        // preferred_x = 25 + 10 + 1 = 36; 36 + 20 = 56 > 50 → overflow.
        // flipped_x   = 25 − 20 − 1 = 4 ≥ 0 → use 4.
        // Child inner left = 4 → child ┌ at (3, anchor_y − 1).
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 10));
        let menu = submenu_parent_menu();
        let viewport = crate::event::Rect::new(0.0, 0.0, 50.0, 10.0);
        let layout = menu.layout(25.0, 1.0, viewport, 10.0, |_| {
            ContextMenuItemMeasure::new(1.0)
        });

        draw_context_menu_with_submenus(
            &mut buf,
            &menu,
            &layout,
            &[0],
            &[0],
            viewport,
            &Theme::default(),
        );

        // anchor_y = 1.0, child border-top at y = 0.
        assert_eq!(
            cell_char(&buf, 3, 0),
            '┌',
            "flipped child popup top-left corner should be at x=3 (left-flip)",
        );
    }

    #[test]
    fn top_border_survives_when_menu_anchored_at_viewport_top_row() {
        // Anchored at y=0 (e.g. a right-click on a tab bar rendered at the
        // very top row of the viewport). Callers lay a menu out inside
        // `Backend::context_menu_viewport` — the surface inset by one cell
        // on every side on TUI, where the box border sits outside the item
        // region. This test passes that inset surface directly, so
        // `layout_at`'s own clamping puts `bounds.y` on row 1 and leaves
        // row 0 free for the border.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            2.0,
            0.0,
            crate::event::Rect::new(1.0, 1.0, 29.0, 9.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        let inner_x = layout.bounds.x.round() as u16;
        let bx = inner_x - 1;
        // Top border must still be drawn, and on a different row than the
        // first item's label.
        assert_eq!(
            cell_char(&buf, bx, 0),
            '┌',
            "top-left corner must be visible when anchored at y=0"
        );
        assert_eq!(
            cell_char(&buf, inner_x + 1, 1),
            'O',
            "first item (\"Open\") must render one row below the top border"
        );
    }

    #[test]
    fn left_border_survives_when_menu_anchored_at_viewport_left_edge() {
        // Symmetric case: anchored at x=0 inside the same inset surface
        // (see the top-row test above), so `layout_at` puts `bounds.x` on
        // column 1, leaving column 0 free for the border.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            0.0,
            1.0,
            crate::event::Rect::new(1.0, 1.0, 29.0, 9.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, 0, row_y),
            '│',
            "left border must be visible when anchored at x=0"
        );
        assert_eq!(
            cell_char(&buf, 2, row_y),
            'O',
            "first item (\"Open\") label must still start after the left border"
        );
    }

    #[test]
    fn clicks_land_on_painted_row_when_menu_anchored_at_viewport_corner() {
        // Paint↔click round-trip: a menu anchored flush against the
        // top-left corner of `Backend::context_menu_viewport` must paint
        // its first item on the exact row/column `hit_test` reports for
        // that item's `WidgetId`. The border occupies the row and column
        // outside `bounds`, so neither moves.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            0.0,
            0.0,
            crate::event::Rect::new(1.0, 1.0, 29.0, 9.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        let first = &layout.visible_items[0];
        let item_x = first.bounds.x;
        let row_y = first.bounds.y;

        // Sanity: the painted label starts one column right of the item's
        // left edge (the check-mark/padding column), on the item's row.
        assert_eq!(
            cell_char(&buf, item_x.round() as u16 + 1, row_y.round() as u16),
            'O',
            "\"Open\" must be painted at the row/column its own bounds report"
        );

        // The border occupies the row/column directly outside `bounds` —
        // hit-testing that cell must miss the menu, not resolve to the item.
        assert_eq!(
            layout.hit_test(item_x - 1.0, row_y - 1.0),
            ContextMenuHit::Empty,
            "the border cell must not be classified as the first item"
        );

        // Hit-testing the exact cell the item was painted at must resolve
        // to that item's id: reserving the border in the viewport the
        // layout is built from is what keeps paint and hit-test in one
        // coordinate space.
        assert_eq!(
            layout.hit_test(item_x, row_y),
            ContextMenuHit::Item(WidgetId::new("Open")),
            "clicking the painted row of the first item must hit \"Open\""
        );
    }

    /// Row `y` of `buf`, as a string, for substring assertions.
    fn buf_row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| cell_char(buf, x, y)).collect()
    }

    #[test]
    fn zero_origin_layout_paints_without_overflow() {
        // A caller that lays the menu out in the raw viewport instead of
        // `Backend::context_menu_viewport` gets an item region on column
        // and row 0, so the near border has no cell inside the buffer to
        // occupy. That must stay a cosmetic degrade: the `u16` border
        // arithmetic may not underflow (a panic under `overflow-checks`,
        // a wrap to 65535 without), and every item must still paint on
        // the row and column its own bounds report.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            0.0,
            0.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        assert_eq!(
            (
                layout.bounds.x.round() as u16,
                layout.bounds.y.round() as u16
            ),
            (0, 0),
            "test precondition: a non-inset viewport puts the item region at the origin"
        );

        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        let first = &layout.visible_items[0];
        assert_eq!(
            cell_char(
                &buf,
                first.bounds.x.round() as u16 + 1,
                first.bounds.y.round() as u16
            ),
            'O',
            "\"Open\" must paint at the row/column its own bounds report:\n{}",
            buf_row(&buf, first.bounds.y.round() as u16)
        );
        assert_eq!(
            layout.hit_test(first.bounds.x + 1.0, first.bounds.y),
            ContextMenuHit::Item(WidgetId::new("Open")),
            "paint and hit-test must agree even when the border has no cell"
        );

        // The edges that do have room are unaffected: the border still
        // closes on the right and the bottom.
        let right = layout.bounds.width.round() as u16;
        let bottom = layout.bounds.height.round() as u16;
        assert_eq!(
            cell_char(&buf, right, bottom),
            '┘',
            "bottom-right corner must still be painted"
        );
    }

    #[test]
    fn submenu_child_keeps_its_border_at_zero_origin_viewport() {
        // Same non-inset viewport, but through the submenu rasteriser:
        // the parent sits at the origin, so the child's own `layout`
        // would clamp to row 0 too. `draw_context_menu_with_submenus`
        // insets the viewport it lays children out in, which keeps the
        // child's chrome on-screen; either way the `u16` arithmetic must
        // not overflow and the child's items must still render.
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 10));
        let menu = submenu_parent_menu();
        let viewport = crate::event::Rect::new(0.0, 0.0, 50.0, 10.0);
        let layout = menu.layout(0.0, 0.0, viewport, 16.0, |_| {
            ContextMenuItemMeasure::new(1.0)
        });
        assert_eq!(
            layout.bounds.y.round() as u16,
            0,
            "test precondition: the parent menu's item region starts on row 0"
        );

        draw_context_menu_with_submenus(
            &mut buf,
            &menu,
            &layout,
            &[0],
            &[0],
            viewport,
            &Theme::default(),
        );

        let screen: String = (0..buf.area.height)
            .map(|y| buf_row(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            screen.contains("SubItem"),
            "the child menu's item must render:\n{screen}"
        );
        assert!(
            screen.lines().any(|line| {
                matches!((line.find('┌'), line.rfind('┐')), (Some(l), Some(r)) if r > l)
            }),
            "the child popup's top border must be fully on-screen:\n{screen}"
        );
    }

    #[test]
    fn submenu_left_flip_against_left_edge_does_not_overflow() {
        // The left-flip branch subtracts the child width from the
        // parent's left edge, which goes negative when the parent is
        // anchored near column 0 in a narrow viewport. The child is
        // clamped back inside instead of wrapping, and still paints.
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 8));
        let menu = submenu_parent_menu();
        let viewport = crate::event::Rect::new(0.0, 0.0, 24.0, 8.0);
        let layout = menu.layout(1.0, 0.0, viewport, 16.0, |_| {
            ContextMenuItemMeasure::new(1.0)
        });

        draw_context_menu_with_submenus(
            &mut buf,
            &menu,
            &layout,
            &[0],
            &[0],
            viewport,
            &Theme::default(),
        );

        let screen: String = (0..buf.area.height)
            .map(|y| buf_row(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            screen.contains("SubItem"),
            "the flipped child menu's item must render inside the buffer:\n{screen}"
        );
    }
}
