//! TUI rasteriser for [`crate::ListView`].
//!
//! Per D6: this function asks the primitive for a [`crate::ListViewLayout`]
//! (one cell per item; title row 1 cell when present) and paints the
//! resolved positions verbatim. Apps that need their own measurer
//! (variable-height items, e.g.) can compute the layout externally —
//! this rasteriser computes it inline because TUI list rows are
//! always uniform 1 cell tall.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color as RatatuiColor;

use super::{draw_styled_text, ratatui_color, set_cell, set_cell_wide};
use crate::primitives::list::{ListItemMeasure, ListView, ListViewLayout};
use crate::text_util::{char_cell_width, display_width};
use crate::theme::Theme;
use crate::types::{Decoration, StyledText};

/// Compute the TUI cell-unit layout for a [`ListView`] without painting —
/// the same viewport reservation (h/v scrollbar rows/columns, title row)
/// [`draw_list`] applies internally before calling [`ListView::layout`].
/// `draw_list` calls this exact function so paint and no-paint hit-testing
/// can never drift apart (`PRIMITIVE_RULES.md` rule 5).
///
/// Coordinate frame: **LOCAL** — relative to `area`'s origin, `(0, 0)` at
/// `area`'s top-left, matching `gtk_list_layout` / `win_list_layout` /
/// `mac_list_layout`. Does **not** account for `ListView::bordered`'s
/// 1-cell border inset — same as every other backend's list-layout
/// helper; a bordered list's caller adds that inset itself.
pub fn tui_list_layout(area: Rect, list: &ListView) -> ListViewLayout {
    tui_list_layout_with_scroll_px(area, list, None)
}

/// Like [`tui_list_layout`], but with an explicit pixel-precise scroll
/// position (`scroll_px`) instead of only [`ListView::scroll_offset`]'s
/// whole-row value — see [`draw_list_with_scroll_px`].
pub fn tui_list_layout_with_scroll_px(
    area: Rect,
    list: &ListView,
    scroll_px: Option<f32>,
) -> ListViewLayout {
    // Floor to a whole cell *before* resolving: TUI's row height is
    // always 1.0, so a non-whole `scroll_px` would otherwise leave a
    // non-zero `scroll_sub_px` remainder that nothing here clips away
    // (unlike a GUI surface's `surface_push_clip`), painting a
    // negative-offset row that collides with its neighbour. Flooring
    // first guarantees `scroll_sub_px == 0.0` and TUI never paints a
    // partial row.
    let scroll_px = scroll_px.map(f32::floor);
    let area_f = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    let hscrollbar = list.hscrollbar(area_f, 1.0);
    let needs_hscrollbar = hscrollbar.is_some();

    let v_area_f = if needs_hscrollbar {
        crate::event::Rect::new(
            area_f.x,
            area_f.y,
            area_f.width,
            (area_f.height - 1.0).max(0.0),
        )
    } else {
        area_f
    };
    let vscrollbar = list.vscrollbar(v_area_f, 1.0);
    let needs_vscrollbar = vscrollbar.is_some();

    let viewport_h = if needs_hscrollbar {
        (area.height as f32 - 1.0).max(0.0)
    } else {
        area.height as f32
    };

    let viewport_w = if needs_vscrollbar {
        (area.width as f32 - 1.0).max(0.0)
    } else {
        area.width as f32
    };

    let title_h = if list.title.is_some() { 1.0 } else { 0.0 };
    list.layout_with_scroll_px(viewport_w, viewport_h, title_h, scroll_px, |_| {
        ListItemMeasure::new(1.0)
    })
}

/// Draw a [`ListView`] into `area` on `buf`. Honours
/// [`ListView::bordered`] (rounded box border + title overlay) and
/// [`ListView::has_focus`] (the selected row only highlights when the
/// list has focus).
///
/// # Visual contract
///
/// - **Bordered:** filled with [`Theme::surface_bg`], rounded
///   `╭─╮ │ ╰─╯` glyphs in [`Theme::border_fg`], optional title
///   centred-ish on the top border in [`Theme::title_fg`].
/// - **Non-bordered with title:** the title row is painted as a flat
///   [`Theme::header_bg`] / [`Theme::header_fg`] strip.
/// - **Selected row:** [`Theme::selected_bg`] background and a `▶`
///   selection prefix in the row's foreground.
/// - **Per-item decoration → fg:** `Error → error_fg`, `Warning →
///   warning_fg`, `Muted → muted_fg`, others → [`Theme::surface_fg`].
/// - **Detail span:** right-aligned in [`Theme::muted_fg`], skipped
///   when there isn't room past the main text.
///
/// `nerd_fonts_enabled` controls which icon variant gets painted —
/// pass the consumer's own Nerd Font resolver (there is no `icons`
/// module in this crate; `crate::icons` is `vimcode`'s own capability
/// probe, not something quadraui ships), or forward the value most
/// recently given to [`crate::Backend::set_nerd_fonts`], or `false` to
/// always use ASCII fallbacks.
pub fn draw_list(
    buf: &mut Buffer,
    area: Rect,
    list: &ListView,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    draw_list_with_scroll_px(buf, area, list, theme, nerd_fonts_enabled, None)
}

/// Like [`draw_list`], but with an explicit pixel-precise scroll
/// position (`scroll_px`) from a
/// [`crate::primitives::list::ListPaintState`] instead of only
/// [`ListView::scroll_offset`]'s whole-row value.
///
/// TUI is a cell grid: `scroll_px` wins over `scroll_offset` (see
/// [`crate::primitives::scrollbar::resolve_scroll_px`]) but is quantised
/// to a whole row — the sub-row remainder is computed and then simply
/// never read, so TUI never paints a partial row. `None` (what
/// [`draw_list`] passes) resolves to exactly the pre-existing whole-row
/// behaviour.
pub fn draw_list_with_scroll_px(
    buf: &mut Buffer,
    area: Rect,
    list: &ListView,
    theme: &Theme,
    nerd_fonts_enabled: bool,
    scroll_px: Option<f32>,
) {
    // Floor to a whole cell up front — see `tui_list_layout_with_scroll_px`'s
    // doc for why — so the scrollbar thumb below and the row layout
    // agree on the same quantised position.
    let scroll_px = scroll_px.map(f32::floor);
    if area.width == 0 || area.height == 0 {
        return;
    }

    let hdr_fg = ratatui_color(theme.header_fg);
    let hdr_bg = ratatui_color(theme.header_bg);
    let fg = ratatui_color(theme.surface_fg);
    let sel_bg = ratatui_color(theme.selected_bg);
    let row_bg = if list.bordered {
        ratatui_color(theme.surface_bg)
    } else {
        ratatui_color(theme.background)
    };
    let dim_fg = ratatui_color(theme.muted_fg);
    let error_fg = ratatui_color(theme.error_fg);
    let warn_fg = ratatui_color(theme.warning_fg);
    let border_fg = ratatui_color(theme.border_fg);
    let title_fg = ratatui_color(theme.title_fg);
    let h_scroll = list.h_scroll;

    // Reserve the bottom row for a horizontal scrollbar and/or the
    // rightmost column for a vertical scrollbar when content overflows.
    // The track + thumb geometry comes from `ListView::hscrollbar` /
    // `ListView::vscrollbar` — the single source of truth also used by
    // consumers for thumb hit-testing (via `Backend::list_hscrollbar` /
    // `Backend::list_vscrollbar`), so the painted thumb and the draggable
    // thumb can never drift apart. In bordered mode the scrollbars sit
    // inside the box; in flat mode they occupy the outermost row/column.
    let layout = tui_list_layout_with_scroll_px(area, list, scroll_px);

    // Re-derive the scrollbar geometry for painting (cheap — pure
    // functions of `list` + `area`). `tui_list_layout` only needs to know
    // *whether* a scrollbar reserves space, not its track/thumb geometry.
    let area_f = crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    let hscrollbar = list.hscrollbar(area_f, 1.0);
    let v_area_f = if hscrollbar.is_some() {
        crate::event::Rect::new(
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
        let top_y = area.y;
        for col in 0..area.width {
            let cx = area.x + col;
            let ch = if col == 0 {
                '╭'
            } else if col + 1 == area.width {
                '╮'
            } else {
                '─'
            };
            set_cell(buf, cx, top_y, ch, border_fg, row_bg);
        }
        if let Some(ref title) = list.title {
            let title_text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
            let label = format!(" {} ", title_text.trim());
            let mut vcol: u16 = 0;
            for ch in label.chars() {
                let w = char_cell_width(ch);
                let cx = area.x + 2 + vcol;
                if cx + w >= area.x + area.width {
                    break;
                }
                if w == 2 {
                    set_cell_wide(buf, cx, top_y, ch, title_fg, row_bg);
                } else {
                    set_cell(buf, cx, top_y, ch, title_fg, row_bg);
                }
                vcol += w;
            }
        }
        if area.height >= 2 {
            let bot_y = area.y + area.height - 1;
            for col in 0..area.width {
                let cx = area.x + col;
                let ch = if col == 0 {
                    '╰'
                } else if col + 1 == area.width {
                    '╯'
                } else {
                    '─'
                };
                set_cell(buf, cx, bot_y, ch, border_fg, row_bg);
            }
        }
        for row in (area.y + 1)..(area.y + area.height - 1) {
            set_cell(buf, area.x, row, '│', border_fg, row_bg);
            set_cell(buf, area.x + area.width - 1, row, '│', border_fg, row_bg);
            for col in 1..(area.width - 1) {
                set_cell(buf, area.x + col, row, ' ', fg, row_bg);
            }
        }
    } else if let Some(title_bounds) = layout.title_bounds {
        if let Some(ref title) = list.title {
            let y = area.y + title_bounds.y.round() as u16;
            for x in area.x..area.x + area.width {
                set_cell(buf, x, y, ' ', hdr_fg, hdr_bg);
            }
            draw_styled_text(
                buf,
                area,
                y,
                1,
                title,
                hdr_fg,
                hdr_bg,
                Decoration::Normal,
                dim_fg,
            );
        }
    }

    for visible_item in &layout.visible_items {
        let item = &list.items[visible_item.item_idx];
        let y = area.y + visible_item.bounds.y.round() as u16;
        let item_x = area.x + visible_item.bounds.x.round() as u16;
        let item_w = visible_item.bounds.width.round() as u16;
        let item_area = Rect {
            x: item_x,
            y,
            width: item_w,
            height: 1,
        };
        let is_selected = visible_item.item_idx == list.selected_idx && list.has_focus;
        let bg = if is_selected { sel_bg } else { row_bg };
        let decoration_fg = match item.decoration {
            Decoration::Error => error_fg,
            Decoration::Warning => warn_fg,
            Decoration::Muted => dim_fg,
            _ => fg,
        };

        // Fill the entire visible row with the background colour first.
        for x in item_x..item_x + item_w {
            set_cell(buf, x, y, ' ', decoration_fg, bg);
        }

        // `vcol` is the virtual display-cell offset within the full item
        // content (prefix + icon + text) — a CJK/emoji glyph advances it
        // by 2, matching `char_cell_width`, not 1 per `char`. Content
        // whose full width sits before `h_scroll` is skipped; the rest
        // maps to buf col (vcol - h_scroll). See `put_char_scrolled`.
        let mut vcol: usize = 0;

        // Selection prefix "▶ " (selected) or "  " (not selected).
        let prefix = if is_selected { "▶ " } else { "  " };
        for ch in prefix.chars() {
            vcol = put_char_scrolled(
                buf,
                vcol,
                h_scroll,
                item_x,
                item_w,
                y,
                ch,
                decoration_fg,
                bg,
            );
        }

        // Optional icon glyph + trailing space.
        if let Some(ref icon) = item.icon {
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            for ch in glyph.chars() {
                vcol = put_char_scrolled(
                    buf,
                    vcol,
                    h_scroll,
                    item_x,
                    item_w,
                    y,
                    ch,
                    decoration_fg,
                    bg,
                );
            }
            vcol = put_char_scrolled(
                buf,
                vcol,
                h_scroll,
                item_x,
                item_w,
                y,
                ' ',
                decoration_fg,
                bg,
            );
        }

        // Main item text — rendered with h_scroll awareness.
        let text_start_vcol = vcol;
        let vcol_after_text = render_spans_scrolled(
            buf,
            vcol,
            h_scroll,
            item_x,
            item_w,
            y,
            &item.text,
            decoration_fg,
            bg,
            item.decoration,
            dim_fg,
        );

        // The item's own right edge (not `h_scroll`'s left edge) cut the
        // text off mid-glyph — overwrite the last visible column with
        // `…`, reusing whatever fg/bg that cell already has so per-span
        // colouring survives the clamp untouched (same technique as
        // `Tree`'s TUI rasteriser).
        if item_w > 0
            && vcol_after_text > h_scroll + item_w as usize
            && text_start_vcol < h_scroll + item_w as usize
        {
            let last_col = item_x + item_w - 1;
            // `last_col` may be the empty *continuation* cell of a wide
            // glyph `render_spans_scrolled` already painted one column
            // to its left — writing `…` straight into it would either
            // get swallowed or leave a half-painted glyph next to the
            // ellipsis. Detect that case (empty symbol) and back up to
            // overwrite the glyph's own cell instead, clearing its
            // now-stale continuation.
            let last_is_continuation = buf
                .cell((last_col, y))
                .map(|c| c.symbol().is_empty())
                .unwrap_or(false);
            let ellipsis_x = if last_is_continuation && last_col > item_x {
                last_col - 1
            } else {
                last_col
            };
            let (ellipsis_fg, ellipsis_bg) = buf
                .cell((ellipsis_x, y))
                .map(|existing| (existing.fg, existing.bg))
                .unwrap_or((decoration_fg, bg));
            set_cell(buf, ellipsis_x, y, '…', ellipsis_fg, ellipsis_bg);
            if ellipsis_x != last_col {
                set_cell(buf, last_col, y, ' ', ellipsis_fg, ellipsis_bg);
            }
        }

        // Buffer column past the last text character (clamped to item_w).
        // Used to ensure detail text doesn't overwrite main text.
        let text_buf_col_end = if vcol_after_text > h_scroll {
            (vcol_after_text - h_scroll).min(item_w as usize)
        } else {
            0
        };

        // Detail text — right-aligned and pinned to the visible viewport
        // (does not scroll with h_scroll, so it remains readable at all offsets).
        if let Some(ref detail) = item.detail {
            let detail_w: usize = detail.spans.iter().map(|s| display_width(&s.text)).sum();
            let start = (item_w as usize).saturating_sub(detail_w + 1);
            if start > text_buf_col_end + 1 {
                draw_styled_text(
                    buf,
                    item_area,
                    y,
                    start,
                    detail,
                    dim_fg,
                    bg,
                    Decoration::Muted,
                    dim_fg,
                );
            }
        }
    }

    // ── Horizontal scrollbar ──────────────────────────────────────────────
    // Drawn after items so it overlays the bottom row's background fill.
    // Track + thumb geometry was resolved above by `ListView::hscrollbar`;
    // we just paint it here. Consumers obtain the same `Scrollbar` via
    // `Backend::list_hscrollbar` and hit-test its thumb for mouse-drag.
    if let Some(ref hsb) = hscrollbar {
        let hsb_bg = if list.bordered {
            theme.surface_bg
        } else {
            theme.background
        };
        super::draw_scrollbar(buf, hsb, theme, hsb_bg);
    }

    // ── Vertical scrollbar ────────────────────────────────────────────────
    // Drawn after items (and the h-scrollbar) so it occupies the rightmost
    // column without being overwritten. Track + thumb geometry was resolved
    // above by `ListView::vscrollbar`; consumers obtain the same `Scrollbar`
    // via `Backend::list_vscrollbar` for thumb hit-testing.
    if let Some(ref vsb) = vscrollbar {
        let vsb_bg = if list.bordered {
            theme.surface_bg
        } else {
            theme.background
        };
        super::draw_scrollbar(buf, vsb, theme, vsb_bg);
    }
}

// ── Private rendering helpers ─────────────────────────────────────────────

/// Write a single character at virtual column `vcol` (a **display-cell**
/// offset, not a char index — see [`char_cell_width`]) within a
/// horizontally-scrolled item row. Characters whose full width is
/// `vcol < h_scroll` are silently dropped; the rest are mapped to buffer
/// column `vcol - h_scroll` inside the item's `[item_x, item_x + item_w)`
/// band.
///
/// A double-width glyph straddling either edge — `h_scroll`'s left edge
/// or the item's right edge — is never half-painted: the visible half is
/// blanked instead, matching [`set_cell_wide`]'s all-or-nothing contract.
///
/// Returns `vcol + char_cell_width(ch)` so callers can do
/// `vcol = put_char_scrolled(...)`.
#[allow(clippy::too_many_arguments)]
fn put_char_scrolled(
    buf: &mut Buffer,
    vcol: usize,
    h_scroll: usize,
    item_x: u16,
    item_w: u16,
    y: u16,
    ch: char,
    fg: RatatuiColor,
    bg: RatatuiColor,
) -> usize {
    let w = char_cell_width(ch) as usize;
    if vcol + w <= h_scroll {
        // Entirely scrolled off past the left edge.
        return vcol + w;
    }
    if vcol < h_scroll {
        // Straddles `h_scroll`'s left edge: only the glyph's trailing
        // half would land at buffer column 0. Blank it rather than
        // paint half a glyph.
        if item_w > 0 {
            set_cell(buf, item_x, y, ' ', fg, bg);
        }
        return vcol + w;
    }
    let buf_col = (vcol - h_scroll) as u16;
    if buf_col >= item_w {
        // Entirely past the item's right edge.
        return vcol + w;
    }
    if w == 2 {
        if buf_col + 1 < item_w {
            set_cell_wide(buf, item_x + buf_col, y, ch, fg, bg);
        } else {
            // Straddles the item's right edge: only the glyph's leading
            // half fits. Blank it rather than paint half a glyph.
            set_cell(buf, item_x + buf_col, y, ' ', fg, bg);
        }
    } else {
        set_cell(buf, item_x + buf_col, y, ch, fg, bg);
    }
    vcol + w
}

/// Render a [`StyledText`]'s spans starting at virtual column `vcol`,
/// skipping the first `h_scroll` virtual columns. Returns the virtual column
/// past the last character of `text` (i.e. the next `vcol` for subsequent
/// content in the same item row).
///
/// Span foreground / background and `decoration` semantics mirror
/// [`super::draw_styled_text`].
#[allow(clippy::too_many_arguments)]
fn render_spans_scrolled(
    buf: &mut Buffer,
    vcol_start: usize,
    h_scroll: usize,
    item_x: u16,
    item_w: u16,
    y: u16,
    text: &StyledText,
    default_fg: RatatuiColor,
    bg: RatatuiColor,
    decoration: Decoration,
    dim_fg: RatatuiColor,
) -> usize {
    let mut vcol = vcol_start;
    for span in &text.spans {
        let span_fg = if let Some(c) = span.fg {
            ratatui_color(c)
        } else if matches!(decoration, Decoration::Muted) {
            dim_fg
        } else {
            default_fg
        };
        let span_bg = span.bg.map(ratatui_color).unwrap_or(bg);
        for ch in span.text.chars() {
            vcol = put_char_scrolled(buf, vcol, h_scroll, item_x, item_w, y, ch, span_fg, span_bg);
        }
    }
    vcol
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::list::{ListItem, ListView};
    use crate::types::{Color, StyledSpan, StyledText, WidgetId};

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
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_three_items_with_selection_marker() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let list = make_list(1);
        draw_list(
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

    #[test]
    fn no_selection_marker_when_unfocused() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let mut list = make_list(1);
        list.has_focus = false;
        draw_list(
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
        draw_list(
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
        };
        let theme = Theme {
            error_fg: Color::rgb(255, 0, 0),
            ..Theme::default()
        };
        draw_list(&mut buf, Rect::new(0, 0, 20, 3), &list, &theme, false);
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
        draw_list(
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
        };
        let area = Rect::new(0, 0, 20, 5);

        let mut buf = Buffer::empty(area);
        draw_list_with_scroll_px(&mut buf, area, &list, &Theme::default(), false, Some(10.7));
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
        draw_list(&mut buf2, area, &via_offset, &Theme::default(), false);
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

        draw_list(
            &mut buf_ref,
            Rect::new(0, 0, 20, 3),
            &list_ref,
            &Theme::default(),
            false,
        );
        draw_list(
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
        draw_list(
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
        draw_list(
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
        draw_list(
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
        draw_list(
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
        draw_list(
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
        };
        draw_list(
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
        draw_list(
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
        draw_list(
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
        draw_list(
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
        draw_list(
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
        draw_list(
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
        draw_list(
            &mut buf_top,
            area,
            &make_vlist(30, 0),
            &Theme::default(),
            false,
        );
        draw_list(
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
        draw_list(
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
        // Sentinel just past the item band — draw_list must never touch it.
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
        };
        draw_list(
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
        };
        draw_list(
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
        };
        draw_list(
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
        };
        draw_list(
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
        apply_selection_gesture, Backend, Decoration, Key, ListViewEvent, Modifiers, NamedKey,
        Rect, SelectionGesture, StyledText, UiEvent, WidgetId,
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
            backend.draw_list(
                Rect::new(0.0, 0.0, VIEWPORT_W as f32, VIEWPORT_H as f32),
                &self.list,
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
