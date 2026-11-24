//! TUI entry point for [`crate::ActivityBar`] — the adapter onto
//! [`super::toolbar::draw_toolbar_with_rail_style`], the one cell-grid
//! icon-rail rasteriser this bar shares with
//! [`crate::primitives::toolbar::Toolbar`]. It lives here, not in the
//! widget module, because that painter is crate-private to `crate::tui`:
//! this module maps the bar to the vertical-rail `Toolbar` shape through
//! [`ActivityBar::to_toolbar_parts`], paints that, then turns the same
//! layout into this type's own bar-relative [`ActivityBarRowHit`] list
//! via [`ActivityBar::row_hits_from_toolbar_layout`].
//!
//! [`crate::primitives::activity_bar::ActivityBar`]'s
//! [`crate::primitive::Primitive::paint_cells`] reaches
//! [`draw_activity_bar_with_style`] for the same reason, and the paint
//! coverage for both lives in that widget module.
//!
//! Activity bar width is caller-determined — typically
//! [`crate::primitives::activity_bar::CELL_BAR_WIDTH`] cells: 1 accent +
//! 2 icon + 1 separator.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::toolbar::{
    draw_toolbar_with_rail_style, rail_layout_options, tui_toolbar_layout_with_options,
};
use crate::primitives::activity_bar::{ActivityBar, ActivityBarRowHit, ActivityBarStyle};
use crate::theme::Theme;

/// Equivalent to [`draw_activity_bar_with_style`] with
/// `ActivityBarStyle::default()`, i.e. no active-row fill — see that
/// function for the full behaviour.
///
/// `nerd_fonts_enabled` picks which half of each item's [`crate::Icon`]
/// paints — `glyph` when `true`, `fallback` when `false`.
/// Pass the backend's own flag (`Backend::set_nerd_fonts`), same as
/// [`super::list::draw_list`].
#[allow(clippy::too_many_arguments)]
pub fn draw_activity_bar(
    buf: &mut Buffer,
    area: Rect,
    bar: &ActivityBar,
    theme: &Theme,
    hovered_idx: Option<usize>,
    nerd_fonts_enabled: bool,
) -> Vec<ActivityBarRowHit> {
    draw_activity_bar_with_style(
        buf,
        area,
        bar,
        &ActivityBarStyle::default(),
        theme,
        hovered_idx,
        nerd_fonts_enabled,
    )
}

/// [`draw_activity_bar`] with an explicit [`ActivityBarStyle`] request.
/// `ActivityBarStyle::default()` reproduces [`draw_activity_bar`]
/// cell for cell. See [`draw_activity_bar`] for `nerd_fonts_enabled`.
#[allow(clippy::too_many_arguments)]
pub fn draw_activity_bar_with_style(
    buf: &mut Buffer,
    area: Rect,
    bar: &ActivityBar,
    style: &ActivityBarStyle,
    theme: &Theme,
    hovered_idx: Option<usize>,
    nerd_fonts_enabled: bool,
) -> Vec<ActivityBarRowHit> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }

    let (toolbar, rail_style) = bar.to_toolbar_parts(style, theme, nerd_fonts_enabled);

    // Bar-relative row hits, built at origin (0, 0) per
    // `row_hits_from_toolbar_layout`'s own precondition — this is also
    // the enumeration `hovered_idx` indexes into (bottom-pinned items
    // first, then top-pinned), so it doubles as the lookup table below.
    let zero_origin_layout = tui_toolbar_layout_with_options(
        &toolbar,
        Rect::new(0, 0, area.width, area.height),
        rail_layout_options(),
    );
    let row_hits = bar.row_hits_from_toolbar_layout(&zero_origin_layout);

    let hovered_id = hovered_idx.and_then(|i| row_hits.get(i)).map(|h| h.id);

    let _ =
        draw_toolbar_with_rail_style(buf, area, &toolbar, &rail_style, theme, hovered_id.as_ref());

    row_hits
}
