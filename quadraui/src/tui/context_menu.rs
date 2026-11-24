//! TUI entry point for [`crate::ContextMenu`] — a thin delegate onto the
//! widget's own cell painter,
//! [`crate::primitives::context_menu::cell_paint`], which documents the
//! box chrome, row content and glyph-override rules. This module only
//! adapts a raw ratatui [`Buffer`] into the [`crate::CellSurface`] that
//! painter takes, and stacks the open submenu levels for
//! [`draw_context_menu_with_submenus`].

use ratatui::buffer::Buffer;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::primitives::context_menu::{
    cell_paint, ContextMenu, ContextMenuGlyphs, ContextMenuItem, ContextMenuItemMeasure,
    ContextMenuLayout, ContextMenuPlacement, CELL_BORDER_INSET,
};
use crate::theme::Theme;

/// Cells [`draw_context_menu`] paints its box border on, outside the
/// item region, on each side — the cell-grid reservation
/// [`crate::primitives::context_menu::CELL_BORDER_INSET`] names, which
/// [`crate::tui::backend::TuiBackend`] reports as its
/// [`crate::Backend::context_menu_border_inset`], so the reservation a
/// caller makes and the chrome this rasteriser paints come from one
/// constant. Re-exported as `quadraui::tui::CONTEXT_MENU_BORDER_INSET`
/// for callers with no [`crate::Backend`] handle: inset the viewport by
/// it on all four sides before laying the menu out, the way
/// [`crate::Backend::context_menu_viewport`] does.
pub const BORDER_INSET: f32 = CELL_BORDER_INSET;

/// Draw a [`ContextMenu`] popup at its resolved layout. `layout` must be
/// the same [`ContextMenuLayout`] the host hit-tests against, so paint
/// and click-routing can never disagree.
///
/// Paints the menu's own [`ContextMenu::glyphs`] at the ASCII half of
/// each [`crate::types::Icon`]; a caller holding a live
/// [`crate::Backend::nerd_fonts_enabled`] calls
/// [`draw_context_menu_with_glyphs`] instead.
pub fn draw_context_menu(
    buf: &mut Buffer,
    menu: &ContextMenu,
    layout: &ContextMenuLayout,
    theme: &Theme,
) {
    draw_context_menu_with_glyphs(buf, menu, layout, theme, None, false);
}

/// [`draw_context_menu`] with a caller-supplied [`ContextMenuGlyphs`]
/// in place of the menu's own [`ContextMenu::glyphs`], plus the live
/// [`crate::Backend::nerd_fonts_enabled`] flag.
///
/// `glyphs: None` falls back to that descriptor field, so a menu naming
/// no glyphs renders the default `✓`/`▶` marks unchanged. A `Some` set
/// overrides only the glyphs it names; any slot left `None` keeps its
/// default. Each slot is a single cell, so only the override glyph's
/// first character is used, mirroring the tab strip's close/dirty
/// overrides. `nerd_fonts_enabled` selects which half of each override
/// [`crate::types::Icon`] — and of each item's own `icon` — renders.
pub fn draw_context_menu_with_glyphs(
    buf: &mut Buffer,
    menu: &ContextMenu,
    layout: &ContextMenuLayout,
    theme: &Theme,
    glyphs: Option<&ContextMenuGlyphs>,
    nerd_fonts_enabled: bool,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(
        menu,
        layout,
        &mut surface,
        theme,
        glyphs,
        nerd_fonts_enabled,
    );
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
/// Each child popup is anchored through
/// [`ContextMenuLayout::submenu_anchor`], the shared pull-right /
/// flip-left rule every menu in this crate positions a nested popup
/// with, and carries a clone of the root's own [`ContextMenu::glyphs`],
/// so one override set paints the whole cascade.
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
    // all four sides by the cell the painter paints its box border on,
    // exactly as `Backend::context_menu_viewport` does for the root.
    // Every placement below reads `vp`, not `viewport`, so neither the
    // pull-left flip nor the clamp fallback can land a child's inner
    // region flush at the buffer's near corner.
    let vp = crate::event::Rect::new(
        viewport.x + BORDER_INSET,
        viewport.y + BORDER_INSET,
        (viewport.width - BORDER_INSET * 2.0).max(0.0),
        (viewport.height - BORDER_INSET * 2.0).max(0.0),
    );

    // Walk the open submenu chain. Owned copies, so each iteration can
    // replace the parent it anchored against.
    let mut parent_items: Vec<ContextMenuItem> = root_menu.items.clone();
    let mut parent_layout = root_layout.clone();

    // Fixed geometry per level: one cell high per item (TUI cells, not
    // pixels).
    const MENU_WIDTH: f32 = 20.0;

    for (depth, &path_idx) in submenu_path.iter().enumerate() {
        let sub_items = match parent_items
            .get(path_idx)
            .and_then(|item| item.submenu.clone())
        {
            Some(items) => items,
            None => break,
        };

        let anchor = parent_layout.submenu_anchor(path_idx, MENU_WIDTH, vp);
        let selected = selected_at_depth.get(depth).copied().unwrap_or(0);

        let sub_menu = ContextMenu {
            id: crate::types::WidgetId::new("tui-submenu"),
            items: sub_items.clone(),
            selected_idx: selected,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
            glyphs: root_menu.glyphs.clone(),
        };

        let sub_layout = sub_menu.layout(anchor.x, anchor.y, vp, MENU_WIDTH, |_| {
            ContextMenuItemMeasure::new(1.0)
        });

        draw_context_menu(buf, &sub_menu, &sub_layout, theme);

        parent_items = sub_items;
        parent_layout = sub_layout;
    }
}
