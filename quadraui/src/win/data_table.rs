//! Direct2D / DirectWrite rasteriser for [`crate::DataTable`] (issue #26).
//!
//! Mirrors `gtk::data_table`'s structure: [`DataTable::layout`] (the D6
//! layout API) resolves column positions, row/header/footer heights,
//! and scrollbar reservations; this module measures column titles (via
//! DirectWrite) and paints (via [`super::text::fill_rect`] +
//! [`DWrite::draw_text`]/`draw_text_styled`). Paint and hit-test both
//! derive from one [`win_data_table_layout`] call.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod data_table;` and `backend.rs`'s
//! module docs. See `win::status_bar`'s module doc for why colours come
//! from `Theme::default()` rather than a live `WinBackend` theme field.
//!
//! # Scope for #26
//!
//! Scrollbar track/thumb paint as flat fills (no `win::draw_scrollbar`
//! dependency — that trait method is still a `todo!()` stub). Row
//! selection/hover tint is computed as a CPU-side RGB blend
//! ([`blend`]) rather than an alpha-blended `FillRectangle`, since the
//! shared [`super::text::fill_rect`] helper takes an opaque [`crate::Color`].

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{blend, fill_rect, pop_clip, push_clip, DWrite};
use crate::event::Rect;
use crate::primitives::data_table::{ColumnAlign, ColumnMeasure, DataTable, SortDirection};
use crate::primitives::scrollbar::Scrollbar;
use crate::theme::Theme;
use crate::types::Decoration;
use crate::DataTableLayout;

const SCROLLBAR_WIDTH: f32 = 8.0;

/// Compute a [`DataTable`]'s layout without painting — the DirectWrite
/// twin of [`draw_data_table`]'s internal layout call.
pub fn win_data_table_layout(
    dwrite: &DWrite,
    rect: Rect,
    table: &DataTable,
    line_height: f32,
) -> DataTableLayout {
    let header_height = (line_height * 1.2).round();
    let measure = |col: &crate::primitives::data_table::Column| -> ColumnMeasure {
        let (w, _) = dwrite.measure_text(&col.title).unwrap_or((0.0, 0.0));
        ColumnMeasure::new(w)
    };
    table.layout(
        rect.width,
        rect.height,
        line_height,
        header_height,
        SCROLLBAR_WIDTH,
        measure,
    )
}

/// Draw a [`DataTable`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`DataTableLayout`] for host click dispatch.
///
/// `hovered_idx` tints the hovered body row (skipped when it's also
/// the selected row).
///
/// # Visual contract
///
/// - **Header:** `Theme::tab_bar_bg`, bold title + sort-direction
///   suffix (`▲`/`▼`), column separators in `Theme::separator`.
/// - **Selected row:** `Theme::selection_bg` blended over the row's own
///   background at `Theme::selection_alpha`.
/// - **Hovered row:** `Theme::tab_bar_bg` blended at `0.5`.
/// - **Muted rows:** cell text in `Theme::muted_fg` regardless of any
///   per-span colour override.
/// - **Footer:** a `Theme::separator` divider rule, `Theme::tab_bar_bg`
///   background, bold cell text.
pub fn draw_data_table(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    table: &DataTable,
    line_height: f32,
    hovered_idx: Option<usize>,
) -> DataTableLayout {
    let theme = Theme::default();
    let layout = win_data_table_layout(dwrite, rect, table, line_height);
    let h_off = table.h_scroll;

    push_clip(target, rect);

    // ── Header ───────────────────────────────────────────────────────
    let _ = fill_rect(
        target,
        Rect::new(rect.x, rect.y, rect.width, layout.header_height),
        theme.tab_bar_bg,
    );

    for (col_idx, rc) in layout.columns.iter().enumerate() {
        let Some(col) = table.columns.get(col_idx) else {
            break;
        };
        if rc.width <= 0.0 {
            continue;
        }
        let sort_suffix = match &table.sort {
            Some((si, dir)) if *si == col_idx => match dir {
                SortDirection::Ascending => " \u{25B2}",
                SortDirection::Descending => " \u{25BC}",
            },
            _ => "",
        };
        let title = format!("{}{}", col.title, sort_suffix);
        let col_x = rect.x + rc.x - h_off;
        let col_rect = Rect::new(col_x, rect.y, rc.width, layout.header_height);
        push_clip(target, col_rect);
        let (tw, th) = dwrite
            .measure_text_styled(&title, true)
            .unwrap_or((0.0, 0.0));
        let text_x = align_text_x(col_x, rc.width, tw, col.align);
        let _ = dwrite.draw_text_styled(
            target,
            &title,
            Rect::new(text_x, rect.y, tw, th),
            theme.foreground,
            true,
        );
        pop_clip(target);
    }

    for (col_idx, rc) in layout.columns.iter().enumerate() {
        if col_idx + 1 >= layout.columns.len() {
            break;
        }
        let sep_x = rect.x + rc.x + rc.width - h_off;
        let _ = fill_rect(
            target,
            Rect::new(sep_x, rect.y, 1.0, layout.header_height),
            theme.separator,
        );
    }

    // ── Body ─────────────────────────────────────────────────────────
    let body_y = rect.y + layout.header_height;
    let visible = layout
        .visible_rows
        .min(table.rows.len().saturating_sub(table.scroll_offset));

    for row_idx in 0..visible {
        let abs_idx = table.scroll_offset + row_idx;
        let row = &table.rows[abs_idx];
        let row_y = body_y + row_idx as f32 * line_height;
        let is_selected = table.selected_idx == Some(abs_idx);
        let is_hovered = hovered_idx == Some(abs_idx) && !is_selected;
        let is_muted = row.decoration == Decoration::Muted;

        let row_bg = if is_selected {
            blend(theme.background, theme.selection_bg, theme.selection_alpha)
        } else if is_hovered {
            blend(theme.background, theme.tab_bar_bg, 0.5)
        } else {
            theme.background
        };
        let _ = fill_rect(
            target,
            Rect::new(rect.x, row_y, rect.width, line_height),
            row_bg,
        );

        for (col_idx, rc) in layout.columns.iter().enumerate() {
            let Some(styled) = row.cells.get(col_idx).filter(|c| !c.spans.is_empty()) else {
                continue;
            };
            if rc.width <= 0.0 {
                continue;
            }
            let col_x = rect.x + rc.x - h_off;
            let col_rect = Rect::new(col_x, row_y, rc.width, line_height);
            push_clip(target, col_rect);

            let full_text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
            let (tw, th) = dwrite.measure_text(&full_text).unwrap_or((0.0, 0.0));
            let align = table
                .columns
                .get(col_idx)
                .map(|c| c.align)
                .unwrap_or(ColumnAlign::Left);
            let text_x = align_text_x(col_x, rc.width, tw, align);

            if is_muted {
                let _ = dwrite.draw_text(
                    target,
                    &full_text,
                    Rect::new(text_x, row_y, tw, th),
                    theme.muted_fg,
                );
            } else {
                // Per-span colour runs, painted left to right from
                // `text_x` (only meaningful for `ColumnAlign::Left`;
                // centre/right alignment still anchors the whole run at
                // `text_x` — matches `gtk::data_table`'s single anchor
                // point for a multi-span cell).
                let mut run_x = text_x;
                for span in &styled.spans {
                    let (sw, sh) = dwrite.measure_text(&span.text).unwrap_or((0.0, 0.0));
                    let fg = span.fg.unwrap_or(theme.foreground);
                    let _ =
                        dwrite.draw_text(target, &span.text, Rect::new(run_x, row_y, sw, sh), fg);
                    run_x += sw;
                }
            }
            pop_clip(target);
        }

        for (col_idx, rc) in layout.columns.iter().enumerate() {
            if col_idx + 1 >= layout.columns.len() || rc.width <= 0.0 {
                continue;
            }
            let sep_x = rect.x + rc.x + rc.width - h_off;
            let _ = fill_rect(
                target,
                Rect::new(sep_x, row_y, 1.0, line_height),
                theme.separator,
            );
        }
    }

    // ── Scrollbars ───────────────────────────────────────────────────
    let footer_h = layout.footer_height;
    if table.show_scrollbar
        && table.rows.len() > layout.visible_rows
        && layout.scrollbar_width > 0.0
    {
        let sb_x = rect.x + rect.width - layout.scrollbar_width;
        let track = Rect::new(
            sb_x,
            rect.y + layout.header_height,
            layout.scrollbar_width,
            (rect.height - layout.header_height - footer_h).max(0.0),
        );
        let sb = Scrollbar::vertical(
            table.id.clone(),
            track,
            table.scroll_offset as f32,
            table.rows.len() as f32,
            layout.visible_rows as f32,
            line_height,
        );
        paint_scrollbar(target, &sb, &theme);
    }
    if layout.h_scrollbar_height > 0.0 && layout.content_width > 0.0 {
        let hsb_y = rect.y + rect.height - footer_h - layout.h_scrollbar_height;
        let track_w = (rect.width - layout.scrollbar_width).max(1.0);
        let track = Rect::new(rect.x, hsb_y, track_w, layout.h_scrollbar_height);
        let sb = Scrollbar::horizontal(
            table.id.clone(),
            track,
            table.h_scroll,
            layout.content_width,
            track_w,
            line_height,
        );
        paint_scrollbar(target, &sb, &theme);
    }

    // ── Footer ───────────────────────────────────────────────────────
    if let Some(footer) = &table.footer {
        if footer_h > 0.0 {
            let footer_band_top = rect.y + rect.height - footer_h;
            let footer_y = rect.y + rect.height - line_height;
            let _ = fill_rect(
                target,
                Rect::new(rect.x, footer_band_top - 0.5, rect.width, 1.0),
                theme.separator,
            );
            let _ = fill_rect(
                target,
                Rect::new(rect.x, footer_band_top, rect.width, footer_h),
                theme.tab_bar_bg,
            );

            for (col_idx, rc) in layout.columns.iter().enumerate() {
                let Some(styled) = footer.cells.get(col_idx).filter(|c| !c.spans.is_empty()) else {
                    continue;
                };
                if rc.width <= 0.0 {
                    continue;
                }
                let col_x = rect.x + rc.x - h_off;
                push_clip(target, Rect::new(col_x, footer_y, rc.width, line_height));
                let full_text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
                let (tw, th) = dwrite
                    .measure_text_styled(&full_text, true)
                    .unwrap_or((0.0, 0.0));
                let align = table
                    .columns
                    .get(col_idx)
                    .map(|c| c.align)
                    .unwrap_or(ColumnAlign::Left);
                let text_x = align_text_x(col_x, rc.width, tw, align);
                let _ = dwrite.draw_text_styled(
                    target,
                    &full_text,
                    Rect::new(text_x, footer_y, tw, th),
                    theme.foreground,
                    true,
                );
                pop_clip(target);
            }
        }
    }

    pop_clip(target);
    layout
}

fn align_text_x(col_x: f32, col_w: f32, text_w: f32, align: ColumnAlign) -> f32 {
    match align {
        ColumnAlign::Left => col_x,
        ColumnAlign::Center => col_x + (col_w - text_w) / 2.0,
        ColumnAlign::Right => col_x + col_w - text_w,
    }
}

/// Paint `sb`'s track (`scrollbar_track`) and thumb (`scrollbar_thumb`)
/// as flat fills — see this module's "Scope for #26" doc for why this
/// doesn't delegate to a shared `draw_scrollbar` (still a `todo!()`
/// stub on `WinBackend`).
fn paint_scrollbar(target: &ID2D1RenderTarget, sb: &Scrollbar, theme: &Theme) {
    let _ = fill_rect(target, sb.track, theme.scrollbar_track);
    let thumb = match sb.axis {
        crate::primitives::scrollbar::ScrollAxis::Vertical => Rect::new(
            sb.track.x,
            sb.track.y + sb.thumb_start,
            sb.track.width,
            sb.thumb_len,
        ),
        crate::primitives::scrollbar::ScrollAxis::Horizontal => Rect::new(
            sb.track.x + sb.thumb_start,
            sb.track.y,
            sb.thumb_len,
            sb.track.height,
        ),
    };
    let _ = fill_rect(target, thumb, theme.scrollbar_thumb);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::data_table::{Column, ColumnWidth, DataRow, DataTableHit};
    use crate::types::{StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 300.0;
    const H: f32 = 100.0;
    const LINE_HEIGHT: f32 = 16.0;

    fn table(rows: Vec<DataRow>) -> DataTable {
        DataTable {
            id: WidgetId::new("table"),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Size".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Right,
                },
            ],
            rows,
            selected_idx: Some(0),
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        }
    }

    fn row(name: &str, size: &str) -> DataRow {
        DataRow {
            cells: vec![
                StyledText::plain(name.to_string()),
                StyledText::plain(size.to_string()),
            ],
            decoration: Decoration::Normal,
        }
    }

    /// Paint↔click round trip: the selected row's blended background
    /// must be painted at its own bounds, and clicking each row/header
    /// resolves to the expected `DataTableHit`.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let table = table(vec![
            row("a.txt", "1kb"),
            row("b.txt", "2kb"),
            row("c.txt", "3kb"),
        ]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_data_table(target, &dwrite, rect, &table, LINE_HEIGHT, None);
            })
            .map(|_| win_data_table_layout(&dwrite, rect, &table, LINE_HEIGHT))
            .expect("paint data table");

        // Header click.
        let hit = layout.hit_test(1.0, 1.0, table.scroll_offset, table.rows.len());
        assert_eq!(hit, DataTableHit::Header { col: 0 });

        // Row click (row 1, below the header).
        let row_y = layout.header_height + line_height_mid(LINE_HEIGHT);
        let hit = layout.hit_test(1.0, row_y, table.scroll_offset, table.rows.len());
        assert_eq!(hit, DataTableHit::Row { idx: 0 });

        // Selected row (idx 0) painted a blended background distinct
        // from the plain body background.
        let theme = Theme::default();
        let sample_y = (layout.header_height + 2.0) as u32;
        let px = surface.pixel_at(2, sample_y);
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "selected row should paint a tinted background distinct from the plain body bg"
        );
    }

    fn line_height_mid(h: f32) -> f32 {
        h / 2.0
    }

    /// Scroll-offset round trip: with `scroll_offset = 1`, a click on
    /// the first body row resolves to row index 1.
    #[test]
    fn scroll_offset_hit_test_agrees() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let mut t = table(vec![row("a", "1"), row("b", "2"), row("c", "3")]);
        t.scroll_offset = 1;
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_data_table_layout(&dwrite, rect, &t, LINE_HEIGHT);
        let row_y = layout.header_height + line_height_mid(LINE_HEIGHT);
        let hit = layout.hit_test(1.0, row_y, t.scroll_offset, t.rows.len());
        assert_eq!(hit, DataTableHit::Row { idx: 1 });
    }

    /// No-paint layout must agree byte-for-byte with what
    /// `draw_data_table` painted.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let table = table(vec![row("a.txt", "1kb"), row("b.txt", "2kb")]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_data_table(target, &dwrite, rect, &table, LINE_HEIGHT, None);
            })
            .map(|_| win_data_table_layout(&dwrite, rect, &table, LINE_HEIGHT))
            .expect("paint");
        let no_paint = win_data_table_layout(&dwrite, rect, &table, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }
}
