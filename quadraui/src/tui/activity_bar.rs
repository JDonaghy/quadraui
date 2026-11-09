//! TUI rasteriser for [`crate::ActivityBar`].
//!
//! Paints via [`super::toolbar::draw_toolbar_with_rail_style`]: maps this
//! bar to the vertical-icon-rail [`crate::primitives::toolbar::Toolbar`]
//! shape through [`ActivityBar::to_toolbar_parts`], paints that, then
//! turns the same layout into this type's own bar-relative
//! [`ActivityBarRowHit`] list via
//! [`ActivityBar::row_hits_from_toolbar_layout`]. Activity bar width is
//! caller-determined (typically 4 cells: 1 accent + 2 icon + 1
//! separator).

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

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::activity_bar::ActivityItem;
    use crate::tui::qc;
    use crate::types::{Color, Icon, WidgetId};

    fn item(id: &str, icon: &str) -> ActivityItem {
        ActivityItem {
            id: WidgetId::new(id),
            icon: icon.into(),
            tooltip: format!("{id} tooltip"),
            is_active: false,
            is_keyboard_selected: false,
        }
    }

    fn bar() -> ActivityBar {
        ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![item("explorer", "E"), item("search", "S"), item("git", "G")],
            bottom_items: vec![item("settings", "*")],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
        }
    }

    /// Hit regions are **bar-relative**, not absolute, so the top row
    /// starts at `0.0` no matter where the bar is painted.
    ///
    /// Looks rows up **by id** rather than by position: `visible_items`
    /// (and therefore this list) is bottom-pinned-first, not visual
    /// top-to-bottom order.
    #[test]
    fn hit_regions_are_bar_relative_not_absolute() {
        let b = bar();
        let theme = Theme::default();

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 3, 3, 10);
        let hits = draw_activity_bar(&mut buf, area, &b, &theme, None, false);

        let span = |id: &str| {
            let h = hits
                .iter()
                .find(|h| h.id.as_str() == id)
                .unwrap_or_else(|| panic!("no hit region for {id:?}"));
            (h.y_start, h.y_end)
        };

        assert_eq!(
            span("explorer"),
            (0.0, 1.0),
            "the top row must start at 0.0 relative to the bar, not at \
             area.y ({}) — folding the origin in here double-counts it \
             against AppShell's own `+ activity_bar_bounds.y` (issue #552)",
            area.y
        );
        assert_eq!(span("search"), (1.0, 2.0), "second row is one row down");
        assert_eq!(span("git"), (2.0, 3.0));
        // Bottom-pinned: `area.height` (10) - 1 row, still measured from
        // the bar's own top edge, not the screen's.
        assert_eq!(span("settings"), (9.0, 10.0));
    }

    /// The same regions must come back for *any* `area.y`. Pinning
    /// invariance rather than a single value is what stops the bug from
    /// reappearing: the pre-fix output was correct at `area.y == 0` (title
    /// bar hidden) and wrong everywhere else, which is exactly why every
    /// existing static test passed.
    #[test]
    fn hit_regions_do_not_move_when_the_bar_does() {
        let b = bar();
        let theme = Theme::default();

        let spans = |y: u16| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 40, 30));
            let hits = draw_activity_bar(&mut buf, Rect::new(0, y, 3, 10), &b, &theme, None, false);
            hits.iter()
                .map(|h| (h.y_start, h.y_end))
                .collect::<Vec<_>>()
        };

        let at_zero = spans(0);
        assert!(!at_zero.is_empty(), "sanity: some rows were painted");
        for y in [1u16, 2, 5] {
            assert_eq!(
                spans(y),
                at_zero,
                "hit regions must be independent of the bar's screen origin; \
                 they differed at area.y = {y} (issue #552)"
            );
        }
    }

    /// Painting uses the absolute row; hit regions are bar-relative.
    #[test]
    fn icons_still_paint_at_the_absolute_row() {
        let b = bar();
        let theme = Theme::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 3, 4, 10);
        draw_activity_bar(&mut buf, area, &b, &theme, None, false);

        let row_text = |y: u16| {
            (area.x..area.x + area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        };
        assert!(
            row_text(3).contains('E'),
            "first icon should paint on the bar's first *screen* row (3), \
             got {:?}",
            row_text(3)
        );
        assert!(
            row_text(4).contains('S'),
            "second icon on screen row 4, got {:?}",
            row_text(4)
        );
    }

    /// `nerd_fonts_enabled` picks `Icon::glyph` when `true` and
    /// `Icon::fallback` when `false` — the activity bar had no fallback
    /// path at all before this issue, painting whatever bare string it
    /// was handed on every backend regardless of Nerd Font availability.
    #[test]
    fn nerd_fonts_flag_selects_glyph_or_fallback() {
        let mut b = bar();
        b.top_items[0].icon = Icon::new("\u{f07c}", "E");
        let theme = Theme::default();

        let row_text = |nerd_fonts_enabled: bool| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
            let area = Rect::new(0, 0, 4, 10);
            draw_activity_bar(&mut buf, area, &b, &theme, None, nerd_fonts_enabled);
            (area.x..area.x + area.width)
                .map(|x| buf[(x, area.y)].symbol().to_string())
                .collect::<String>()
        };

        assert!(
            row_text(true).contains('\u{f07c}'),
            "nerd_fonts_enabled: true should paint the glyph half of the Icon"
        );
        assert!(
            row_text(false).contains('E'),
            "nerd_fonts_enabled: false should paint the fallback half of the Icon"
        );
    }

    /// `style.active_bg: Some(..)` with `active_accent: None` fills the
    /// active row's cells with the requested colour and paints **zero**
    /// accent-line pixels — the accent column (`area.x`) must not carry
    /// the `'▎'` glyph, only the plain fill background.
    #[test]
    fn active_bg_fills_row_cells_with_zero_accent_glyph_when_accent_is_none() {
        let mut b = bar();
        b.top_items[0].is_active = true; // "explorer"
        let theme = Theme::default();
        let fill_color = Color::rgb(49, 50, 51);
        let style = ActivityBarStyle::new().with_active_bg(fill_color);

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 0, 3, 10);
        draw_activity_bar_with_style(&mut buf, area, &b, &style, &theme, None, false);

        let fill = qc(fill_color);
        assert_ne!(
            buf[(area.x, area.y)].symbol(),
            "▎",
            "active_accent is None: the accent column must not carry the \
             accent glyph (zero accent-line pixels)"
        );
        assert_eq!(
            buf[(area.x, area.y)].bg,
            fill,
            "accent column should carry the active_bg fill, not the plain bar bg"
        );
        let icon_col = area.x + 1;
        assert_eq!(
            buf[(icon_col, area.y)].bg,
            fill,
            "icon column should also carry the active_bg fill"
        );
    }

    /// The flip side: `active_accent: Some(..)` with no `style.active_bg`
    /// still paints the traditional accent glyph, and the row keeps the
    /// plain bar background elsewhere.
    #[test]
    fn active_accent_paints_glyph_when_set_with_no_active_bg() {
        let mut b = bar();
        b.top_items[0].is_active = true; // "explorer"
        b.active_accent = Some(Color::rgb(80, 140, 255));
        let theme = Theme::default();

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 0, 3, 10);
        draw_activity_bar_with_style(
            &mut buf,
            area,
            &b,
            &ActivityBarStyle::default(),
            &theme,
            None,
            false,
        );

        assert_eq!(
            buf[(area.x, area.y)].symbol(),
            "▎",
            "active_accent is set: the accent column should carry the accent glyph"
        );
        let icon_col = area.x + 1;
        assert_eq!(
            buf[(icon_col, area.y)].bg,
            qc(theme.tab_bar_bg),
            "with no active_bg, the icon column keeps the plain bar background"
        );
    }
}
