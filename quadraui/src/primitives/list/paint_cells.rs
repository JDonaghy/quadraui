//! Cell-grid painter behind [`ListView`]'s `Primitive::paint_cells`.
//!
//! Paints through [`CellSurface`] verbs only: every glyph is a
//! [`CellSurface::put_char`]/[`CellSurface::put_wide`] with a plain
//! fg/bg [`CellStyle`], and the overflow ellipsis reads the cell it
//! replaces back with [`CellSurface::get`] so per-span colouring
//! survives the clamp.

use crate::cell_surface::{solid_style, CellStyle, CellSurface};
use crate::event::Rect;
use crate::primitives::list::ListView;
use crate::primitives::scrollbar::{cell_paint as scrollbar_cells, ScrollbarLayout};
use crate::text_util::{char_cell_width, display_width};
use crate::theme::Theme;
use crate::types::{Color, Decoration, StyledText};
use crate::CellOverflow;

/// One painted row's cell band: the columns `[x, x + w)` on row `y`.
#[derive(Clone, Copy)]
struct Band {
    x: u16,
    w: u16,
    y: u16,
}

/// Paint `list` into the cell rect `area` on `surface`.
///
/// # Visual contract
///
/// - **Bordered:** filled with [`Theme::surface_bg`], rounded
///   `╭─╮ │ ╰─╯` glyphs in [`Theme::border_fg`], optional title on the
///   top border in [`Theme::title_fg`].
/// - **Non-bordered with title:** the title row is a flat
///   [`Theme::header_bg`] / [`Theme::header_fg`] strip.
/// - **Selected row** (only while [`ListView::has_focus`]):
///   [`Theme::selected_bg`] and a `▶` selection prefix.
/// - **Per-item decoration → fg:** `Error → error_fg`, `Warning →
///   warning_fg`, `Muted → muted_fg`, others → [`Theme::surface_fg`].
/// - **Detail span:** right-aligned in [`Theme::muted_fg`], pinned to the
///   viewport, skipped when there isn't room past the main text.
/// - **Striping** (`striped` or [`ListView::striped`]): every other
///   non-selected, non-header row on the absolute item index takes
///   [`Theme::row_alt_bg`].
///
/// `scroll_px` wins over [`ListView::scroll_offset`] but is quantised to
/// a whole row: a cell grid never paints a partial row. `overflow` cuts a
/// row too long for its band with an `…` in the last cell
/// ([`CellOverflow::Ellipsis`]) or nothing at all ([`CellOverflow::Clip`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint(
    surface: &mut dyn CellSurface,
    area: Rect,
    list: &ListView,
    theme: &Theme,
    nerd_fonts_enabled: bool,
    scroll_px: Option<f32>,
    striped: bool,
    overflow: CellOverflow,
) {
    let cell = |v: f32| v.max(0.0).round() as u16;
    let (ax, ay, aw, ah) = (
        cell(area.x),
        cell(area.y),
        cell(area.width),
        cell(area.height),
    );
    // Floored to a whole cell up front so the scrollbar thumb and the row
    // layout agree on the same quantised position.
    let scroll_px = scroll_px.map(f32::floor);
    if aw == 0 || ah == 0 {
        return;
    }

    let fg = theme.surface_fg;
    let row_bg = if list.bordered {
        theme.surface_bg
    } else {
        theme.background
    };
    let dim_fg = theme.muted_fg;
    let h_scroll = list.h_scroll;
    // A terminal quantised to a low colour depth rounds the stripe back to
    // `row_bg`'s own nearest colour, which drops the stripe for free.
    let alt_bg = theme.row_alt_bg(row_bg);

    let layout = list.resolve_layout(
        crate::primitive::Units::Cells,
        (aw as f32, ah as f32),
        1.0,
        1.0,
        0.0,
        scroll_px,
    );

    // The scrollbar geometry comes from `ListView::hscrollbar` /
    // `ListView::vscrollbar`, the same values `ListView`'s primitive
    // layout reports for hit-testing, so the painted thumb and the
    // draggable one never drift. Bordered, the bars sit inside the box;
    // flat, they take the outermost row/column.
    let area_f = Rect::new(ax as f32, ay as f32, aw as f32, ah as f32);
    let hscrollbar = list.hscrollbar(area_f, 1.0);
    let v_area_f = if hscrollbar.is_some() {
        Rect::new(
            area_f.x,
            area_f.y,
            area_f.width,
            (area_f.height - 1.0).max(0.0),
        )
    } else {
        area_f
    };
    let vscrollbar = list.vscrollbar_with_scroll_px(v_area_f, 1.0, 1.0, scroll_px);

    if list.bordered {
        paint_border(surface, (ax, ay, aw, ah), list, theme, row_bg);
    } else if let (Some(title_bounds), Some(title)) = (layout.title_bounds, list.title.as_ref()) {
        let y = ay + title_bounds.y.round() as u16;
        let header = solid_style(theme.header_fg, theme.header_bg);
        for x in ax..ax + aw {
            surface.put_char(x, y, ' ', header);
        }
        put_styled_text(
            surface,
            Band { x: ax, w: aw, y },
            1,
            title,
            (theme.header_fg, theme.header_bg),
            Decoration::Normal,
            dim_fg,
        );
    }

    for visible_item in &layout.visible_items {
        let item = &list.items[visible_item.item_idx];
        let band = Band {
            x: ax + visible_item.bounds.x.round() as u16,
            w: visible_item.bounds.width.round() as u16,
            y: ay + visible_item.bounds.y.round() as u16,
        };
        let is_selected = visible_item.item_idx == list.selected_idx && list.has_focus;
        // Striped on the absolute item index, not the on-screen position,
        // so a row's shading survives scrolling. Selection takes priority
        // over the stripe; a header-decorated row is never striped.
        let is_striped_row = (striped || list.striped)
            && !is_selected
            && !matches!(item.decoration, Decoration::Header)
            && visible_item.item_idx % 2 == 1;
        let bg = if is_selected {
            theme.selected_bg
        } else if is_striped_row {
            alt_bg
        } else {
            row_bg
        };
        let decoration_fg = match item.decoration {
            Decoration::Error => theme.error_fg,
            Decoration::Warning => theme.warning_fg,
            Decoration::Muted => dim_fg,
            _ => fg,
        };
        let row_style = solid_style(decoration_fg, bg);

        for x in band.x..band.x + band.w {
            surface.put_char(x, band.y, ' ', row_style);
        }

        // `vcol` is the virtual display-cell offset within the full item
        // content (prefix + icon + text): a CJK/emoji glyph advances it by
        // 2. Content wholly before `h_scroll` is skipped; the rest maps to
        // band column `vcol - h_scroll`.
        let mut vcol: usize = 0;
        let prefix = if is_selected { "▶ " } else { "  " };
        for ch in prefix.chars() {
            vcol = put_char_scrolled(surface, band, vcol, h_scroll, ch, row_style);
        }
        if let Some(ref icon) = item.icon {
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            for ch in glyph.chars() {
                vcol = put_char_scrolled(surface, band, vcol, h_scroll, ch, row_style);
            }
            vcol = put_char_scrolled(surface, band, vcol, h_scroll, ' ', row_style);
        }

        let text_start_vcol = vcol;
        let mut vcol_after_text = vcol;
        for span in &item.text.spans {
            let span_fg = span_fg(span.fg, item.decoration, decoration_fg, dim_fg);
            let style = solid_style(span_fg, span.bg.unwrap_or(bg));
            for ch in span.text.chars() {
                vcol_after_text =
                    put_char_scrolled(surface, band, vcol_after_text, h_scroll, ch, style);
            }
        }

        // The item's own right edge (not `h_scroll`'s left edge) cut the
        // text mid-glyph: under `CellOverflow::Ellipsis`, the last visible
        // column becomes `…`, keeping that cell's own fg/bg.
        if overflow == CellOverflow::Ellipsis
            && band.w > 0
            && vcol_after_text > h_scroll + band.w as usize
            && text_start_vcol < h_scroll + band.w as usize
        {
            paint_ellipsis(surface, band, row_style);
        }

        // Band column past the last text character, so the detail never
        // overwrites the main text.
        let text_col_end = if vcol_after_text > h_scroll {
            (vcol_after_text - h_scroll).min(band.w as usize)
        } else {
            0
        };

        // The detail is right-aligned and pinned to the viewport: it does
        // not scroll with `h_scroll`, so it stays readable at any offset.
        if let Some(ref detail) = item.detail {
            let detail_w: usize = detail.spans.iter().map(|s| display_width(&s.text)).sum();
            let start = (band.w as usize).saturating_sub(detail_w + 1);
            if start > text_col_end + 1 {
                put_styled_text(
                    surface,
                    band,
                    start,
                    detail,
                    (dim_fg, bg),
                    Decoration::Muted,
                    dim_fg,
                );
            }
        }
    }

    // Scrollbars paint after the rows so they overlay the bottom row's and
    // rightmost column's fill; horizontal first, then vertical.
    for bar in [hscrollbar, vscrollbar].into_iter().flatten() {
        scrollbar_cells::paint(
            &ScrollbarLayout::resolve(&bar, bar.track),
            surface,
            theme,
            row_bg,
        );
    }
}

/// The rounded box border, its title and the blank interior of a
/// bordered list occupying the cell rect `(x, y, w, h)`.
fn paint_border(
    surface: &mut dyn CellSurface,
    (x0, y0, w, h): (u16, u16, u16, u16),
    list: &ListView,
    theme: &Theme,
    row_bg: Color,
) {
    let border = solid_style(theme.border_fg, row_bg);
    let edge = |col: u16, left: char, right: char| {
        if col == 0 {
            left
        } else if col + 1 == w {
            right
        } else {
            '─'
        }
    };
    for col in 0..w {
        surface.put_char(x0 + col, y0, edge(col, '╭', '╮'), border);
    }
    if let Some(ref title) = list.title {
        let title_text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
        let label = format!(" {} ", title_text.trim());
        let title_style = solid_style(theme.title_fg, row_bg);
        let mut vcol: u16 = 0;
        for ch in label.chars() {
            let cw = char_cell_width(ch);
            let cx = x0 + 2 + vcol;
            if cx + cw >= x0 + w {
                break;
            }
            if cw == 2 {
                surface.put_wide(cx, y0, ch, title_style);
            } else {
                surface.put_char(cx, y0, ch, title_style);
            }
            vcol += cw;
        }
    }
    if h >= 2 {
        let bot_y = y0 + h - 1;
        for col in 0..w {
            surface.put_char(x0 + col, bot_y, edge(col, '╰', '╯'), border);
        }
    }
    let interior = solid_style(theme.surface_fg, row_bg);
    for row in (y0 + 1)..(y0 + h - 1) {
        surface.put_char(x0, row, '│', border);
        surface.put_char(x0 + w - 1, row, '│', border);
        for col in 1..(w - 1) {
            surface.put_char(x0 + col, row, ' ', interior);
        }
    }
}

/// A span's foreground: its own colour, else the muted colour on a
/// `Muted` row, else the row's default.
fn span_fg(own: Option<Color>, decoration: Decoration, default_fg: Color, dim_fg: Color) -> Color {
    match own {
        Some(c) => c,
        None if matches!(decoration, Decoration::Muted) => dim_fg,
        None => default_fg,
    }
}

/// Overwrite the band's last column with `…`, keeping that cell's fg/bg.
///
/// When the last column is the trailing half of a double-width glyph,
/// the `…` goes on the glyph's own cell instead and the trailing half is
/// blanked, so no half glyph is left beside the ellipsis.
fn paint_ellipsis(surface: &mut dyn CellSurface, band: Band, fallback: CellStyle) {
    let last_col = band.x + band.w - 1;
    let last_is_continuation = last_col > band.x
        && surface
            .get(last_col - 1, band.y)
            .is_some_and(|(ch, _)| char_cell_width(ch) == 2);
    let ellipsis_x = if last_is_continuation {
        last_col - 1
    } else {
        last_col
    };
    let style = surface
        .get(ellipsis_x, band.y)
        .map(|(_, existing)| solid_style_of(existing))
        .unwrap_or(fallback);
    surface.put_char(ellipsis_x, band.y, '…', style);
    if ellipsis_x != last_col {
        surface.put_char(last_col, band.y, ' ', style);
    }
}

/// `existing`'s colours with no modifiers — what a plain glyph written
/// over that cell keeps.
fn solid_style_of(existing: CellStyle) -> CellStyle {
    CellStyle {
        fg: existing.fg,
        bg: existing.bg,
        ..CellStyle::default()
    }
}

/// Write `ch` at virtual column `vcol` (a display-cell offset) of a
/// horizontally scrolled row band. A glyph wholly before `h_scroll` or
/// past the band is dropped; a double-width glyph straddling either edge
/// is blanked rather than half-painted. Returns the next virtual column.
fn put_char_scrolled(
    surface: &mut dyn CellSurface,
    band: Band,
    vcol: usize,
    h_scroll: usize,
    ch: char,
    style: CellStyle,
) -> usize {
    let w = char_cell_width(ch) as usize;
    if vcol + w <= h_scroll {
        return vcol + w;
    }
    if vcol < h_scroll {
        if band.w > 0 {
            surface.put_char(band.x, band.y, ' ', style);
        }
        return vcol + w;
    }
    let col = (vcol - h_scroll) as u16;
    if col >= band.w {
        return vcol + w;
    }
    if w == 2 {
        if col + 1 < band.w {
            surface.put_wide(band.x + col, band.y, ch, style);
        } else {
            surface.put_char(band.x + col, band.y, ' ', style);
        }
    } else {
        surface.put_char(band.x + col, band.y, ch, style);
    }
    vcol + w
}

/// Write `text`'s spans from band column `start_col`, clipped to the
/// band, returning the column past the last glyph. A span's own colours
/// win; otherwise `(fg, bg)` apply, with `decoration` `Muted` dimming the
/// foreground to `dim_fg`.
fn put_styled_text(
    surface: &mut dyn CellSurface,
    band: Band,
    start_col: usize,
    text: &StyledText,
    (fg, bg): (Color, Color),
    decoration: Decoration,
    dim_fg: Color,
) -> usize {
    let mut col = start_col;
    let width = band.w as usize;
    for span in &text.spans {
        let style = solid_style(
            span_fg(span.fg, decoration, fg, dim_fg),
            span.bg.unwrap_or(bg),
        );
        for ch in span.text.chars() {
            if col >= width {
                return col;
            }
            let w = char_cell_width(ch) as usize;
            if w == 2 && col + 1 < width {
                surface.put_wide(band.x + col as u16, band.y, ch, style);
            } else {
                surface.put_char(band.x + col as u16, band.y, ch, style);
            }
            col += w;
        }
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::list::{ListItem, ListPaintState};
    use crate::tui::{ratatui_color, BufferCellSurface};
    use crate::types::{StyledSpan, WidgetId};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn rect_of(area: Rect) -> crate::event::Rect {
        crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        )
    }

    fn paint_list_with_scroll_px(
        buf: &mut Buffer,
        area: Rect,
        list: &ListView,
        theme: &Theme,
        nerd_fonts_enabled: bool,
        scroll_px: Option<f32>,
    ) {
        paint(
            &mut BufferCellSurface::new(buf),
            rect_of(area),
            list,
            theme,
            nerd_fonts_enabled,
            scroll_px,
            false,
            CellOverflow::Ellipsis,
        );
    }

    fn paint_list(
        buf: &mut Buffer,
        area: Rect,
        list: &ListView,
        theme: &Theme,
        nerd_fonts_enabled: bool,
    ) {
        paint_list_with_scroll_px(buf, area, list, theme, nerd_fonts_enabled, None);
    }

    fn paint_list_with_state(
        buf: &mut Buffer,
        area: Rect,
        list: &ListView,
        theme: &Theme,
        nerd_fonts_enabled: bool,
        state: &ListPaintState,
    ) {
        paint(
            &mut BufferCellSurface::new(buf),
            rect_of(area),
            list,
            theme,
            nerd_fonts_enabled,
            state.scroll_px,
            state.striped,
            CellOverflow::Ellipsis,
        );
    }

    fn item(text: &str, dec: Decoration) -> ListItem {
        ListItem {
            text: StyledText {
                spans: vec![StyledSpan::plain(text)],
            },
            detail: None,
            icon: None,
            decoration: dec,
        }
    }

    fn make_list(selected: usize) -> ListView {
        ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![
                item("alpha", Decoration::Normal),
                item("beta", Decoration::Normal),
                item("gamma", Decoration::Normal),
            ],
            selected_idx: selected,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_three_items_with_selection_marker() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let list = make_list(1);
        paint_list(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &list,
            &Theme::default(),
            false,
        );

        // Selection marker '▶' is on row 1 (the second item).
        assert_eq!(cell_char(&buf, 0, 1), '▶');
        // First and third rows show ' ' selection placeholder.
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(cell_char(&buf, 0, 2), ' ');
    }

    /// `ListPaintState::striped: true` tints every other non-selected
    /// row with `theme.row_alt_bg`; the selected row keeps
    /// `theme.selected_bg` regardless of its parity, and `striped: false`
    /// (the default) paints every row with the plain background.
    #[test]
    fn striped_tints_alternate_rows_selection_wins_over_stripe() {
        let mut list = make_list(1); // selected row 1 (odd) — would be striped if not selected
        list.items.push(item("delta", Decoration::Normal));
        let area = Rect::new(0, 0, 20, 5);
        let theme = Theme::default();

        let mut unstriped = Buffer::empty(area);
        paint_list(&mut unstriped, area, &list, &theme, false);

        let mut striped_buf = Buffer::empty(area);
        let state = crate::primitives::list::ListPaintState::default().with_striped(true);
        paint_list_with_state(&mut striped_buf, area, &list, &theme, false, &state);

        let plain_bg = ratatui_color(theme.background);
        let alt_bg = ratatui_color(theme.row_alt_bg(theme.background));
        let sel_bg = ratatui_color(theme.selected_bg);
        assert_ne!(
            alt_bg, plain_bg,
            "fixture theme must make the stripe visible"
        );

        // Row 0 (even, not selected): plain either way.
        assert_eq!(unstriped[(0, 0)].bg, plain_bg);
        assert_eq!(striped_buf[(0, 0)].bg, plain_bg);

        // Row 1 (odd, selected): selection colour either way — the
        // stripe never overrides the selected row.
        assert_eq!(unstriped[(0, 1)].bg, sel_bg);
        assert_eq!(striped_buf[(0, 1)].bg, sel_bg);

        // Row 2 (even, not selected): unaffected by striping.
        assert_eq!(striped_buf[(0, 2)].bg, plain_bg);

        // Row 3 (odd, not selected): only tinted when striped.
        assert_eq!(unstriped[(0, 3)].bg, plain_bg);
        assert_eq!(striped_buf[(0, 3)].bg, alt_bg);
    }

    /// The descriptor's own `striped` tints the same rows as a paint
    /// state's, with no paint state at all.
    #[test]
    fn descriptor_striped_tints_alternate_rows_without_a_paint_state() {
        let mut list = make_list(1);
        list.items.push(item("delta", Decoration::Normal));
        list.striped = true;
        let area = Rect::new(0, 0, 20, 5);
        let theme = Theme::default();

        let mut buf = Buffer::empty(area);
        paint_list(&mut buf, area, &list, &theme, false);

        assert_eq!(buf[(0, 2)].bg, ratatui_color(theme.background));
        assert_eq!(
            buf[(0, 3)].bg,
            ratatui_color(theme.row_alt_bg(theme.background))
        );
    }

    #[test]
    fn no_selection_marker_when_unfocused() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let mut list = make_list(1);
        list.has_focus = false;
        paint_list(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &list,
            &Theme::default(),
            false,
        );
        // Row 1 should NOT have the '▶' marker.
        for y in 0..3 {
            assert_ne!(cell_char(&buf, 0, y), '▶');
        }
    }

    #[test]
    fn bordered_paints_corner_glyphs() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let mut list = make_list(0);
        list.bordered = true;
        list.title = Some(StyledText {
            spans: vec![StyledSpan::plain("Picker")],
        });
        paint_list(
            &mut buf,
            Rect::new(0, 0, 10, 5),
            &list,
            &Theme::default(),
            false,
        );

        assert_eq!(cell_char(&buf, 0, 0), '╭');
        assert_eq!(cell_char(&buf, 9, 0), '╮');
        assert_eq!(cell_char(&buf, 0, 4), '╰');
        assert_eq!(cell_char(&buf, 9, 4), '╯');
    }

    #[test]
    fn decoration_error_uses_error_fg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![item("oops", Decoration::Error)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        let theme = Theme {
            error_fg: Color::rgb(255, 0, 0),
            ..Theme::default()
        };
        paint_list(&mut buf, Rect::new(0, 0, 20, 3), &list, &theme, false);
        // The 'o' of "oops" should be drawn in error_fg.
        // Selection marker is ' ' (no focus); icon prefix runs cols 0..2;
        // text starts at col 2.
        assert_eq!(cell_char(&buf, 2, 0), 'o');
        let fg = buf[(2u16, 0u16)].fg;
        assert_eq!(fg, ratatui::style::Color::Rgb(255, 0, 0));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let list = make_list(0);
        paint_list(
            &mut buf,
            Rect::new(0, 0, 0, 5),
            &list,
            &Theme::default(),
            false,
        );
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    /// `scroll_px` wins over `scroll_offset`, but on
    /// TUI's cell grid (`row_height == 1.0`) it quantises to the same
    /// whole row a GUI backend's sub-row remainder would float between
    /// — the "TUI quantises scroll_px to whole rows" half of the
    /// issue's acceptance criterion.
    #[test]
    fn scroll_px_quantises_to_a_whole_row_on_tui() {
        let items: Vec<ListItem> = (0..50)
            .map(|i| item(&format!("row-{i}"), Decoration::Normal))
            .collect();
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            items,
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        let area = Rect::new(0, 0, 20, 5);

        let mut buf = Buffer::empty(area);
        paint_list_with_scroll_px(&mut buf, area, &list, &Theme::default(), false, Some(10.7));
        let first_row: String = (0..9).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            first_row.contains("row-10"),
            "scroll_px=10.7 should quantise to row 10 at the top, got: {first_row}"
        );

        // A whole-row scroll_offset of 10 paints byte-for-byte the same
        // first row — scroll_px degrades to the pre-existing
        // row-quantised behaviour, it does not add a new visual.
        let mut via_offset = list.clone();
        via_offset.scroll_offset = 10;
        let mut buf2 = Buffer::empty(area);
        paint_list(&mut buf2, area, &via_offset, &Theme::default(), false);
        let first_row_via_offset: String = (0..9).map(|x| cell_char(&buf2, x, 0)).collect();
        assert_eq!(first_row, first_row_via_offset);
    }

    // ── h_scroll tests ───────────────────────────────────────────────────────

    #[test]
    fn hscroll_zero_renders_identically_to_default() {
        // h_scroll=0 with max_content_width=None must produce the same output
        // as the original no-scroll rasteriser.
        let mut buf_ref = Buffer::empty(Rect::new(0, 0, 20, 3));
        let mut buf_new = Buffer::empty(Rect::new(0, 0, 20, 3));

        let list_ref = make_list(0); // h_scroll=0, max_content_width=None by construction
        let list_new = ListView {
            h_scroll: 0,
            max_content_width: None,
            ..make_list(0)
        };

        paint_list(
            &mut buf_ref,
            Rect::new(0, 0, 20, 3),
            &list_ref,
            &Theme::default(),
            false,
        );
        paint_list(
            &mut buf_new,
            Rect::new(0, 0, 20, 3),
            &list_new,
            &Theme::default(),
            false,
        );

        assert_eq!(buf_ref, buf_new, "h_scroll=0 must produce identical output");
        // Sanity: selected row 0 still has '▶' at col 0, 'a' of "alpha" at col 2.
        assert_eq!(cell_char(&buf_new, 0, 0), '▶');
        assert_eq!(cell_char(&buf_new, 2, 0), 'a');
    }

    #[test]
    fn hscroll_skips_prefix_and_reveals_text_at_col_zero() {
        // With h_scroll=2 the two-char "  " prefix of a non-selected item
        // is entirely scrolled off; the first char of the item text appears
        // at buffer column 0.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let mut list = make_list(1); // item 0 is NOT selected, no focus marker
        list.h_scroll = 2;
        paint_list(
            &mut buf,
            Rect::new(0, 0, 20, 3),
            &list,
            &Theme::default(),
            false,
        );

        // Row 0: item "alpha" — "  " prefix (vcol 0,1) scrolled off → 'a' at buf col 0.
        assert_eq!(
            cell_char(&buf, 0, 0),
            'a',
            "first text char at buf col 0 after skipping prefix"
        );
        // Row 1: item "beta" is selected (selected_idx=1), prefix '▶'(vcol 0) is
        // scrolled off, ' '(vcol 1) is scrolled off → 'b' of "beta" at buf col 0.
        assert_eq!(
            cell_char(&buf, 0, 1),
            'b',
            "selected item text visible at buf col 0"
        );
    }

    #[test]
    fn hscroll_partial_prefix_visible() {
        // h_scroll=1: first prefix char scrolled off, second (space) appears at col 0.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let mut list = make_list(1); // row 0 not selected
        list.h_scroll = 1;
        paint_list(
            &mut buf,
            Rect::new(0, 0, 20, 3),
            &list,
            &Theme::default(),
            false,
        );

        // Row 0 non-selected: prefix "  " → vcol 0 skipped, vcol 1 (' ') at buf col 0.
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        // 'a' of "alpha" at buf col 1.
        assert_eq!(cell_char(&buf, 1, 0), 'a');
    }

    #[test]
    fn hscrollbar_visible_when_content_wider_than_area() {
        // max_content_width=20 with area width=10 → scrollbar appears on last row.
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let mut list = make_list(0);
        list.max_content_width = Some(20);
        paint_list(
            &mut buf,
            Rect::new(0, 0, 10, 5),
            &list,
            &Theme::default(),
            false,
        );

        // Bottom row (y=4) must contain at least one scrollbar glyph.
        let has_sb = (0..10u16).any(|x| matches!(cell_char(&buf, x, 4), '▁' | '▄'));
        assert!(
            has_sb,
            "horizontal scrollbar expected on last row when content overflows"
        );
        // Items still appear: row 0 should have the selection marker.
        assert_eq!(cell_char(&buf, 0, 0), '▶');
    }

    #[test]
    fn hscrollbar_hidden_when_content_fits() {
        // max_content_width=5 with area width=20 → no scrollbar.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let mut list = make_list(0);
        list.max_content_width = Some(5);
        paint_list(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &list,
            &Theme::default(),
            false,
        );

        let has_sb = (0..20u16).any(|x| matches!(cell_char(&buf, x, 4), '▁' | '▄'));
        assert!(
            !has_sb,
            "no horizontal scrollbar expected when content fits in viewport"
        );
    }

    #[test]
    fn hscrollbar_inside_bordered_box() {
        // In bordered mode the scrollbar sits at area.height-2 (above bottom border).
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 6));
        let mut list = make_list(0);
        list.bordered = true;
        list.max_content_width = Some(30);
        paint_list(
            &mut buf,
            Rect::new(0, 0, 10, 6),
            &list,
            &Theme::default(),
            false,
        );

        // Bottom border corners still intact.
        assert_eq!(cell_char(&buf, 0, 5), '╰');
        assert_eq!(cell_char(&buf, 9, 5), '╯');
        // Scrollbar at y=4 in inner columns (x=1..9).
        let has_sb = (1..9u16).any(|x| matches!(cell_char(&buf, x, 4), '▁' | '▄'));
        assert!(
            has_sb,
            "horizontal scrollbar expected at y=4 inside bordered box"
        );
    }

    #[test]
    fn detail_pinned_when_hscroll_active() {
        // With h_scroll > 0 the main text scrolls but the detail span must
        // remain right-aligned in the viewport — it is pinned, not scrolled.
        // This test also exercises the `text_buf_col_end` overlap guard:
        // the detail must not overwrite the last visible chars of main text.
        //
        // Layout at width=20, h_scroll=8:
        //   prefix "  " → vcol 0,1 (both scrolled off)
        //   text "abcdefghijklmnop" (16 chars) → vcol 2..17
        //   With h_scroll=8: visible from vcol 8 onwards → buf col 0..11
        //   buf col 0 = vcol 8 = 'g' (a=vcol2, b=vcol3, …, g=vcol8)
        //   detail "det" (3 chars): start = 20 - 3 - 1 = 16; buf cols 16..19
        //   text_buf_col_end = vcol_after_text(18) - h_scroll(8) = 10; detail start(16) > 11 ✓
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![ListItem {
                text: StyledText {
                    spans: vec![StyledSpan::plain("abcdefghijklmnop")],
                },
                detail: Some(StyledText {
                    spans: vec![StyledSpan::plain("det")],
                }),
                icon: None,
                decoration: Decoration::Normal,
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 8,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        paint_list(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &list,
            &Theme::default(),
            false,
        );

        // Main text: vcol 8 = text char at index 8-2=6 → 'g' (a=0,b=1,…,g=6)
        // appears at buf col 0.
        assert_eq!(
            cell_char(&buf, 0, 0),
            'g',
            "first visible main-text char at buf col 0"
        );
        // Detail "det" must be pinned at buf cols 16..18 regardless of h_scroll.
        assert_eq!(cell_char(&buf, 16, 0), 'd', "detail 'd' at col 16");
        assert_eq!(cell_char(&buf, 17, 0), 'e', "detail 'e' at col 17");
        assert_eq!(cell_char(&buf, 18, 0), 't', "detail 't' at col 18");
        // The cell just before the detail must NOT be 'p' (last char of main
        // text) — text ends at buf col 10, so col 15 should be blank padding.
        assert_ne!(
            cell_char(&buf, 15, 0),
            'p',
            "main text must not bleed into detail area"
        );
    }

    #[test]
    fn hscrollbar_max_content_none_never_shows_scrollbar() {
        // max_content_width=None → never show scrollbar regardless of actual content.
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 3));
        let mut list = make_list(0);
        list.max_content_width = None;
        // Item "alpha" (5 chars + 2 prefix = 7) would overflow the 5-wide area,
        // but without max_content_width set the rasteriser can't know that.
        paint_list(
            &mut buf,
            Rect::new(0, 0, 5, 3),
            &list,
            &Theme::default(),
            false,
        );

        let has_sb = (0..5u16).any(|x| matches!(cell_char(&buf, x, 2), '▁' | '▄'));
        assert!(!has_sb, "no scrollbar when max_content_width is None");
    }

    // ── v_scrollbar rasteriser tests ─────────────────────────────────────────

    fn make_vlist(n: usize, scroll_offset: usize) -> ListView {
        ListView {
            items: (0..n)
                .map(|i| item(&format!("item {}", i), Decoration::Normal))
                .collect(),
            scroll_offset,
            show_v_scrollbar: true,
            ..make_list(0)
        }
    }

    #[test]
    fn vscrollbar_visible_when_items_exceed_height() {
        // 20 items in a 5-row area → scrollbar in rightmost column.
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let list = make_vlist(20, 0);
        paint_list(
            &mut buf,
            Rect::new(0, 0, 10, 5),
            &list,
            &Theme::default(),
            false,
        );

        // Rightmost column (x=9) must contain at least one scrollbar glyph.
        let has_sb = (0..5u16).any(|y| matches!(cell_char(&buf, 9, y), '█' | '░'));
        assert!(
            has_sb,
            "vertical scrollbar expected in rightmost column when items overflow"
        );
        // Selection marker still appears in the items area.
        assert_eq!(
            cell_char(&buf, 0, 0),
            '▶',
            "selection marker must remain intact"
        );
    }

    #[test]
    fn vscrollbar_hidden_when_show_false() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let mut list = make_vlist(20, 0);
        list.show_v_scrollbar = false;
        paint_list(
            &mut buf,
            Rect::new(0, 0, 10, 5),
            &list,
            &Theme::default(),
            false,
        );

        let has_sb = (0..5u16).any(|y| matches!(cell_char(&buf, 9, y), '█' | '░'));
        assert!(!has_sb, "no vertical scrollbar when show_v_scrollbar=false");
    }

    #[test]
    fn vscrollbar_items_do_not_paint_into_scrollbar_column() {
        // Items must stay within [0, area.width-2] when v_scrollbar is active.
        let long_text = "abcdefghijklmnop";
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let list = ListView {
            items: (0..20)
                .map(|_| item(long_text, Decoration::Normal))
                .collect(),
            show_v_scrollbar: true,
            ..make_list(0)
        };
        paint_list(
            &mut buf,
            Rect::new(0, 0, 10, 5),
            &list,
            &Theme::default(),
            false,
        );

        // Column 9 must be a scrollbar glyph, not item text.
        let col9_is_sb = (0..5u16).any(|y| matches!(cell_char(&buf, 9, y), '█' | '░'));
        assert!(col9_is_sb, "rightmost column should be the scrollbar");
        assert_eq!(
            cell_char(&buf, 0, 0),
            '▶',
            "selection marker at col 0 must be intact"
        );
    }

    #[test]
    fn vscrollbar_hidden_when_all_items_fit() {
        // 3 items in a 5-row viewport → all fit, no scrollbar shown.
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let list = make_vlist(3, 0);
        paint_list(
            &mut buf,
            Rect::new(0, 0, 10, 5),
            &list,
            &Theme::default(),
            false,
        );

        let has_sb = (0..5u16).any(|y| matches!(cell_char(&buf, 9, y), '█' | '░'));
        assert!(!has_sb, "no scrollbar when all items fit");
    }

    #[test]
    fn vscrollbar_thumb_moves_down_as_scroll_offset_increases() {
        // PRIMITIVE_RULES.md Rule 6: assert the PAINTED thumb tracks
        // scroll_offset (read the rasterised output), not just the geometry
        // method. 30 items in a 10-row area; paint at offset 0 vs near-max and
        // confirm the '█' thumb's topmost cell moves DOWN.
        let area = Rect::new(0, 0, 10, 10);
        let mut buf_top = Buffer::empty(area);
        let mut buf_bot = Buffer::empty(area);
        paint_list(
            &mut buf_top,
            area,
            &make_vlist(30, 0),
            &Theme::default(),
            false,
        );
        paint_list(
            &mut buf_bot,
            area,
            &make_vlist(30, 20),
            &Theme::default(),
            false,
        );

        let first_thumb = |buf: &Buffer| (0..10u16).find(|&y| cell_char(buf, 9, y) == '█');
        let thumb_top = first_thumb(&buf_top).expect("thumb '█' must paint at scroll_offset=0");
        let thumb_bot = first_thumb(&buf_bot).expect("thumb '█' must paint at scroll_offset=20");
        assert!(
            thumb_bot > thumb_top,
            "v-scrollbar thumb must move DOWN as scroll_offset grows \
             (got top={thumb_top}, bottom={thumb_bot}) — guards the \
             'thumb hardcoded at gutter top' failure class (Rule 6)",
        );
    }

    #[test]
    fn overlong_row_text_ellipsizes_instead_of_clipping_mid_glyph() {
        // A 20-column area leaves well under 20 columns for the item's
        // own text after the selection prefix; a 50-char label must end
        // in `…`, not stop abruptly on a hard character boundary.
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let mut list = make_list(0);
        list.items = vec![item(&"x".repeat(50), Decoration::Normal)];
        paint_list(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &list,
            &Theme::default(),
            false,
        );

        let row: String = (0..20).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            row.trim_end().ends_with('…'),
            "overlong row text must end in an ellipsis, got {row:?}"
        );
    }

    // ── Wide-glyph (CJK/emoji) display-cell tests ──────────────────────────

    /// A 6-cell item band leaves room for exactly two whole 2-cell CJK
    /// glyphs after the 2-cell unfocused prefix. The third glyph sits
    /// entirely past the item's own right edge, so the clamp overwrites
    /// the second glyph with `…` and never paints a half glyph or a
    /// character past `item_x + item_w`.
    #[test]
    fn wide_glyph_item_paints_whole_glyphs_and_ellipsizes_within_item_band() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        // Sentinel just past the item band — paint_list must never touch it.
        buf[(6u16, 0u16)].set_char('Z');
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![item("日本語日本語", Decoration::Normal)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        paint_list(
            &mut buf,
            Rect::new(0, 0, 6, 1),
            &list,
            &Theme::default(),
            false,
        );

        // Prefix "  " at cols 0,1; whole glyphs land at even columns 2
        // and (pre-clamp) 4, each followed by an empty continuation cell.
        assert_eq!(cell_char(&buf, 2, 0), '日');
        assert_eq!(buf[(3u16, 0u16)].symbol(), "");
        // The clamp lands on the second glyph's continuation cell (col 5),
        // so it backs up and overwrites the glyph's own cell (col 4) with
        // `…`, clearing the now-stale continuation.
        assert_eq!(cell_char(&buf, 4, 0), '…');
        assert_eq!(buf[(5u16, 0u16)].symbol(), " ");
        // Nothing painted at or past item_x + item_w (= 6).
        assert_eq!(cell_char(&buf, 6, 0), 'Z');
    }

    /// Mirrors `tree.rs`'s
    /// `ellipsis_overwrite_backs_up_off_a_wide_glyphs_continuation_cell`:
    /// when the item-width clamp boundary lands exactly on the
    /// continuation cell of a wide glyph, the ellipsis must overwrite the
    /// glyph's own cell (clearing the continuation to a blank), never the
    /// continuation cell alone.
    #[test]
    fn ellipsis_overwrite_backs_up_off_a_wide_glyphs_continuation_cell() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            // Unfocused prefix "  " (2 cells) + 3 ascii fillers reach col
            // 5; "字" (2 cells) then occupies cols 5-6 exactly — col 6 is
            // the item's own last column at item_w=7 — before the
            // trailing "X" is pushed entirely past the right edge.
            items: vec![item("aaa字X", Decoration::Normal)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        paint_list(
            &mut buf,
            Rect::new(0, 0, 7, 1),
            &list,
            &Theme::default(),
            false,
        );

        assert_eq!(
            cell_char(&buf, 5, 0),
            '…',
            "the ellipsis must overwrite the wide glyph's own cell"
        );
        assert_eq!(
            buf[(6u16, 0u16)].symbol(),
            " ",
            "the glyph's now-stale continuation cell must be cleared to a blank cell"
        );
        // The ascii filler before the glyph is undisturbed by the backed-up
        // overwrite.
        assert_eq!(cell_char(&buf, 4, 0), 'a');
    }

    /// Detail text is measured and right-aligned by **display width**
    /// (`text_util::display_width`), not `chars().count()` — a 2-char CJK
    /// detail is 4 display cells, not 2; the latter would place it two
    /// columns too far right.
    #[test]
    fn detail_right_aligned_by_display_width_for_cjk() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            items: vec![ListItem {
                text: StyledText {
                    spans: vec![StyledSpan::plain("hi")],
                },
                detail: Some(StyledText {
                    spans: vec![StyledSpan::plain("新着")],
                }),
                icon: None,
                decoration: Decoration::Normal,
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        paint_list(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &list,
            &Theme::default(),
            false,
        );

        // display_width("新着") == 4 → start = item_w(20) - 4 - 1 = 15.
        assert_eq!(cell_char(&buf, 15, 0), '新');
        assert_eq!(buf[(16u16, 0u16)].symbol(), "");
        assert_eq!(cell_char(&buf, 17, 0), '着');
        assert_eq!(buf[(18u16, 0u16)].symbol(), "");
    }

    /// A 2-cell CJK glyph whose virtual column straddles `h_scroll`'s left
    /// edge must never be half-painted: the visible half is blanked
    /// instead of showing a stray leading/trailing byte of the glyph.
    #[test]
    fn hscroll_wide_glyph_straddling_left_edge_paints_no_half_glyph() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let list = ListView {
            id: WidgetId::new("list"),
            title: None,
            // Unfocused prefix "  " (vcol 0,1) + "AB" (vcol 2,3) + "日"
            // (vcol 4..6, a 2-cell glyph) + "CD" (vcol 6,7). h_scroll=5
            // lands in the middle of "日"'s 2-cell span.
            items: vec![item("AB日CD", Decoration::Normal)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
            bordered: false,
            h_scroll: 5,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        };
        paint_list(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &list,
            &Theme::default(),
            false,
        );

        assert_eq!(
            cell_char(&buf, 0, 0),
            ' ',
            "glyph straddling h_scroll's left edge must be blanked, not half-painted"
        );
        assert_eq!(
            cell_char(&buf, 1, 0),
            'C',
            "first fully-visible char after the blanked straddle"
        );
        assert_eq!(cell_char(&buf, 2, 0), 'D');
    }
}

/// Multi-select keyboard wiring over [`crate::tui::testing::TuiDriver`].
///
/// `ListView` stays purely declarative (no selection-set field of its
/// own — see `crate::types::apply_selection_gesture`'s doc), so the app
/// owns the selected-index set, the keyboard cursor, and the gesture
/// anchor. TUI has no mouse, so unlike the click-driven
/// `crate::gtk::list::multi_select_driver_tests` / `crate::win::list`
/// counterparts, a plain Up/Down only moves the cursor and leaves the
/// selection untouched; Shift+Up/Down extends a range from the anchor via
/// [`SelectionGesture::from_modifiers`], and Space toggles the cursor row
/// without moving it.
#[cfg(test)]
mod multi_select_driver_tests {
    use crate::primitives::list::{ListItem, ListView};
    use crate::runner::{AppLogic, Reaction};
    use crate::tui::testing::TuiDriver;
    use crate::{
        apply_selection_gesture, Backend, Decoration, DrawsPrimitives, Key, ListViewEvent,
        Modifiers, NamedKey, Rect, SelectionGesture, StyledText, UiEvent, WidgetId,
    };

    const VIEWPORT_W: u16 = 40;
    const VIEWPORT_H: u16 = 10;

    fn item(label: &str) -> ListItem {
        ListItem {
            text: StyledText::plain(label.to_string()),
            icon: None,
            detail: None,
            decoration: Decoration::Normal,
        }
    }

    fn sample_list() -> ListView {
        ListView {
            id: WidgetId::new("l"),
            title: None,
            items: (0..5).map(|i| item(&format!("row {i}"))).collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
            striped: false,
        }
    }

    struct ListApp {
        list: ListView,
        cursor: usize,
        selected: Vec<usize>,
        anchor: usize,
        events: Vec<ListViewEvent>,
    }

    impl ListApp {
        fn new() -> Self {
            Self {
                list: sample_list(),
                cursor: 0,
                selected: Vec::new(),
                anchor: 0,
                events: Vec::new(),
            }
        }
    }

    impl AppLogic for ListApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let _ = backend.draw_dyn(
                Rect::new(0.0, 0.0, VIEWPORT_W as f32, VIEWPORT_H as f32),
                &self.list,
                &(),
            );
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            let UiEvent::KeyPressed { key, modifiers, .. } = event else {
                return Reaction::Continue;
            };
            let last = self.list.items.len().saturating_sub(1);
            match key {
                Key::Named(NamedKey::Down) | Key::Named(NamedKey::Up) => {
                    let delta: isize = if key == Key::Named(NamedKey::Down) {
                        1
                    } else {
                        -1
                    };
                    self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
                    if !modifiers.shift {
                        // A plain move repositions the cursor only; the
                        // selection changes explicitly via Space/Shift.
                        return Reaction::Redraw;
                    }
                    let indices = apply_selection_gesture(
                        &self.selected,
                        self.anchor,
                        self.cursor,
                        SelectionGesture::Range,
                    );
                    self.selected = indices.clone();
                    self.events.push(ListViewEvent::SelectionSet { indices });
                    Reaction::Redraw
                }
                Key::Char(' ') => {
                    let indices = apply_selection_gesture(
                        &self.selected,
                        self.anchor,
                        self.cursor,
                        SelectionGesture::Toggle,
                    );
                    self.selected = indices.clone();
                    self.anchor = self.cursor;
                    self.events.push(ListViewEvent::SelectionSet { indices });
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            }
        }
    }

    fn key_event(key: Key, modifiers: Modifiers) -> UiEvent {
        UiEvent::KeyPressed {
            key,
            modifiers,
            repeat: false,
        }
    }

    fn driver() -> TuiDriver<ListApp> {
        TuiDriver::new(ListApp::new(), VIEWPORT_W, VIEWPORT_H)
    }

    #[test]
    fn plain_arrow_moves_the_cursor_without_changing_selection() {
        let mut driver = driver();
        driver.dispatch(key_event(Key::Named(NamedKey::Down), Modifiers::default()));
        assert_eq!(driver.app().cursor, 1);
        assert_eq!(driver.app().selected, Vec::<usize>::new());
    }

    #[test]
    fn space_toggles_rows_into_a_multi_selection() {
        let mut driver = driver();
        driver.dispatch(key_event(Key::Char(' '), Modifiers::default()));
        driver.dispatch(key_event(Key::Named(NamedKey::Down), Modifiers::default()));
        driver.dispatch(key_event(Key::Named(NamedKey::Down), Modifiers::default()));
        driver.dispatch(key_event(Key::Char(' '), Modifiers::default()));
        assert_eq!(driver.app().selected, vec![0, 2]);
        // Space toggles the same row back off.
        driver.dispatch(key_event(Key::Named(NamedKey::Up), Modifiers::default()));
        driver.dispatch(key_event(Key::Named(NamedKey::Up), Modifiers::default()));
        driver.dispatch(key_event(Key::Char(' '), Modifiers::default()));
        assert_eq!(driver.app().selected, vec![2]);
        assert_eq!(
            driver.app().events.last(),
            Some(&ListViewEvent::SelectionSet { indices: vec![2] })
        );
    }

    #[test]
    fn shift_arrow_extends_a_range_from_the_anchor() {
        let mut driver = driver();
        driver.dispatch(key_event(Key::Char(' '), Modifiers::default()));
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        driver.dispatch(key_event(Key::Named(NamedKey::Down), shift));
        driver.dispatch(key_event(Key::Named(NamedKey::Down), shift));
        driver.dispatch(key_event(Key::Named(NamedKey::Down), shift));
        assert_eq!(driver.app().selected, vec![0, 1, 2, 3]);
        assert_eq!(
            driver.app().events.last(),
            Some(&ListViewEvent::SelectionSet {
                indices: vec![0, 1, 2, 3]
            })
        );
    }
}
