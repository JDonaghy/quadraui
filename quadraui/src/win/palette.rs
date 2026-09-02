//! Direct2D / DirectWrite rasteriser for [`crate::Palette`] (issue #28).
//!
//! Mirrors `gtk::palette`'s structure: [`Palette::layout`] (the D6
//! layout API) does the vertical positioning (title / query / item rows
//! / create row / preview pane); this module supplies a uniform
//! per-item row height (`line_height`, same shortcut `gtk::draw_palette`
//! takes — real per-item width doesn't affect layout, only row count)
//! and paints the resolved layout.
//!
//! Per-item `match_positions` (byte offsets into the item's concatenated
//! span text) are highlighted by splitting the label into contiguous
//! highlighted/non-highlighted runs and painting each run in
//! [`Theme::match_fg`] or the row's normal foreground — the DirectWrite
//! equivalent of `gtk::palette`'s per-character Pango `AttrColor` spans
//! (DirectWrite has no ready analogue to a Pango `AttrList`, so runs are
//! painted as separate `DrawText` calls instead of one attributed run).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod palette;` and `backend.rs`'s
//! module docs.

use std::collections::HashSet;

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::palette::{Palette, PaletteItemMeasure, PaletteLayout, PaletteMode};
use crate::theme::Theme;
use crate::types::Color;

/// Compute a [`Palette`]'s layout at `(rect.x, rect.y)` without
/// painting, clamping `scroll_offset` so the selected row stays visible
/// (same visibility clamp `gtk::draw_palette` applies to its local
/// clone before calling `Palette::layout`).
pub fn win_palette_layout(rect: Rect, palette: &Palette, line_height: f32) -> PaletteLayout {
    let visible_rows = if line_height > 0.0 {
        (rect.height / line_height) as usize
    } else {
        0
    };
    let total = palette.items.len();
    let max_offset = total.saturating_sub(visible_rows);
    let effective_offset = if visible_rows == 0 {
        0
    } else if palette.selected_idx < palette.scroll_offset {
        palette.selected_idx
    } else if palette.selected_idx >= palette.scroll_offset + visible_rows {
        palette.selected_idx + 1 - visible_rows
    } else {
        palette.scroll_offset
    };
    let effective_offset = effective_offset.min(max_offset);

    let mut local = palette.clone();
    local.scroll_offset = effective_offset;

    let title_h = if !palette.title.is_empty() {
        line_height
    } else {
        0.0
    };
    let query_h = if palette.show_query { line_height } else { 0.0 };

    local.layout(rect.width, rect.height, title_h, query_h, 6.0, 8.0, |_| {
        PaletteItemMeasure::new(line_height)
    })
}

/// Split `text` into contiguous `(run, highlighted)` chunks based on
/// `match_positions` (byte offsets, one per highlighted character).
fn matched_runs(text: &str, match_positions: &[usize]) -> Vec<(String, bool)> {
    if match_positions.is_empty() {
        return vec![(text.to_string(), false)];
    }
    let matches: HashSet<usize> = match_positions.iter().copied().collect();
    let mut runs: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut cur_hi = false;
    let mut first = true;
    for (byte_idx, ch) in text.char_indices() {
        let hi = matches.contains(&byte_idx);
        if first {
            cur_hi = hi;
            first = false;
        } else if hi != cur_hi {
            runs.push((std::mem::take(&mut cur), cur_hi));
            cur_hi = hi;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        runs.push((cur, cur_hi));
    }
    runs
}

/// Paint `text` starting at `row.x, row.y` (single line, `row.height`
/// tall), colouring highlighted runs (per [`matched_runs`]) in
/// `match_fg` and the rest in `fg`. Returns the total painted width
/// (DIPs).
fn draw_matched_text(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    text: &str,
    match_positions: &[usize],
    row: Rect,
    fg: Color,
    match_fg: Color,
) -> f32 {
    let mut cursor_x = row.x;
    for (run, hi) in matched_runs(text, match_positions) {
        if run.is_empty() {
            continue;
        }
        let (w, _) = dwrite.measure_text(&run).unwrap_or((0.0, 0.0));
        let rect = Rect::new(cursor_x, row.y, w.max(1.0), row.height);
        let color = if hi { match_fg } else { fg };
        let _ = dwrite.draw_text(target, &run, rect, color);
        cursor_x += w;
    }
    cursor_x - row.x
}

/// Draw a [`Palette`] modal into `rect`. Backend-internal layout (see
/// [`win_palette_layout`]) — `Palette` has no layout-passthrough trait
/// method (unlike `ContextMenu`/`Dialog`), matching
/// [`crate::Backend::draw_palette`]'s signature.
pub fn draw_palette(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    palette: &Palette,
    line_height: f32,
) {
    if rect.width < 20.0 || rect.height < line_height * 4.0 {
        return;
    }

    let theme = Theme::default();
    let bg = theme.surface_bg;
    let fg = theme.surface_fg;
    let border = theme.border_fg;
    let title_fg = theme.title_fg;
    let query_fg = theme.query_fg;
    let match_fg = theme.match_fg;
    let sel = theme.selected_bg;
    let dim = theme.muted_fg;
    let accent = theme.accent_fg;

    let _ = fill_rect(target, rect, bg);
    let _ = stroke_rect(target, rect, border, 1.0);

    let layout = win_palette_layout(rect, palette, line_height);

    // `Palette::layout` returns bounds relative to a `(0, 0)` origin
    // (see its doc) — offset every layout-derived rect by `rect.x` /
    // `rect.y` before painting, mirroring `gtk::draw_palette`'s
    // `x + title_bounds.x` / `y + title_bounds.y` treatment.
    let off = |r: Rect| Rect::new(rect.x + r.x, rect.y + r.y, r.width, r.height);

    if let Some(tb) = layout.title_bounds.map(off) {
        let title_text = if palette.total_count > 0 {
            format!(
                " {}  {}/{} ",
                palette.title,
                palette.items.len(),
                palette.total_count
            )
        } else {
            format!(" {} ", palette.title)
        };
        let _ = dwrite.draw_text(target, &title_text, tb, title_fg);
    }

    if let Some(qb) = layout.query_bounds.map(off) {
        let prompt = "> ";
        let (prompt_w, _) = dwrite.measure_text(prompt).unwrap_or((0.0, 0.0));
        let prompt_rect = Rect::new(qb.x + 8.0, qb.y, prompt_w.max(1.0), qb.height);
        let _ = dwrite.draw_text(target, prompt, prompt_rect, query_fg);
        let query_rect = Rect::new(
            qb.x + 8.0 + prompt_w,
            qb.y,
            (qb.width - 8.0 - prompt_w).max(1.0),
            qb.height,
        );
        let _ = dwrite.draw_text(target, &palette.query, query_rect, query_fg);
    }

    if palette.mode == PaletteMode::Input {
        return;
    }

    for vis in &layout.visible_items {
        let bounds = off(vis.bounds);
        let item = &palette.items[vis.item_idx];
        let is_selected = vis.item_idx == palette.selected_idx && palette.has_focus;
        if is_selected {
            let _ = fill_rect(target, bounds, sel);
        }

        let full_text: String = item.text.spans.iter().map(|s| s.text.as_str()).collect();
        let prefix = if is_selected { "\u{25b6} " } else { "  " };
        let (prefix_w, _) = dwrite.measure_text(prefix).unwrap_or((0.0, 0.0));
        let prefix_rect = Rect::new(bounds.x + 8.0, bounds.y, prefix_w.max(1.0), bounds.height);
        let _ = dwrite.draw_text(target, prefix, prefix_rect, fg);

        let text_x = bounds.x + 8.0 + prefix_w;
        let text_row = Rect::new(
            text_x,
            bounds.y,
            (bounds.width - (text_x - bounds.x)).max(1.0),
            bounds.height,
        );
        let _ = draw_matched_text(
            target,
            dwrite,
            &full_text,
            &item.match_positions,
            text_row,
            fg,
            match_fg,
        );

        if let Some(ref detail) = item.detail {
            let detail_text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
            let (dw, _) = dwrite.measure_text(&detail_text).unwrap_or((0.0, 0.0));
            let dx = bounds.x + bounds.width - dw - 8.0;
            let detail_rect = Rect::new(dx, bounds.y, dw.max(1.0), bounds.height);
            let _ = dwrite.draw_text(target, &detail_text, detail_rect, dim);
        }
    }

    if let Some(sb) = layout.scrollbar {
        let _ = fill_rect(
            target,
            Rect::new(
                rect.x + sb.track.x,
                rect.y + sb.track.y,
                sb.track.width,
                sb.track.height,
            ),
            Color::rgb(
                (bg.r as f32 * 0.7) as u8,
                (bg.g as f32 * 0.7) as u8,
                (bg.b as f32 * 0.7) as u8,
            ),
        );
        let _ = fill_rect(
            target,
            Rect::new(
                rect.x + sb.thumb.x,
                rect.y + sb.thumb.y,
                sb.thumb.width,
                sb.thumb.height,
            ),
            border,
        );
    }

    if let (Some(cb), Some(label)) = (layout.create_bounds, palette.create_label.as_ref()) {
        let prefix_rect = Rect::new(rect.x + cb.x + 8.0, rect.y + cb.y, 20.0, cb.height);
        let _ = dwrite.draw_text(target, "+ ", prefix_rect, accent);
        let label_rect = Rect::new(
            rect.x + cb.x + 28.0,
            rect.y + cb.y,
            (cb.width - 28.0).max(1.0),
            cb.height,
        );
        let _ = dwrite.draw_text(target, label, label_rect, accent);
    }

    if let (Some(pb), Some(preview)) = (layout.preview_bounds, palette.preview.as_ref()) {
        let preview_rect = Rect::new(rect.x + pb.x, rect.y + pb.y, pb.width, pb.height);
        let mut row_y = preview_rect.y;
        if let Some(ref title) = preview.title {
            let tr = Rect::new(
                preview_rect.x + 8.0,
                row_y,
                (pb.width - 8.0).max(1.0),
                line_height,
            );
            let _ = dwrite.draw_text(target, title, tr, dim);
            row_y += line_height;
        }
        for (vi, line) in preview.lines.iter().skip(preview.scroll_offset).enumerate() {
            let ly = row_y + vi as f32 * line_height;
            if ly + line_height > preview_rect.y + preview_rect.height {
                break;
            }
            let line_idx = preview.scroll_offset + vi;
            if preview.highlight_line == Some(line_idx) {
                let _ = fill_rect(
                    target,
                    Rect::new(preview_rect.x, ly, pb.width, line_height),
                    sel,
                );
            }
            let text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
            let lr = Rect::new(
                preview_rect.x + 8.0,
                ly,
                (pb.width - 8.0).max(1.0),
                line_height,
            );
            let _ = dwrite.draw_text(target, &text, lr, fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;

    fn item(text: &str, match_positions: Vec<usize>) -> crate::primitives::palette::PaletteItem {
        crate::primitives::palette::PaletteItem {
            text: StyledText::plain(text),
            detail: None,
            icon: None,
            match_positions,
            depth: 0,
            expandable: false,
            expanded: false,
        }
    }

    fn palette() -> Palette {
        Palette {
            id: WidgetId::new("pal"),
            title: "Commands".into(),
            query: "op".into(),
            query_cursor: 2,
            items: vec![item("open file", vec![0, 1]), item("close", vec![])],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 0,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::List,
        }
    }

    #[test]
    fn renders_and_highlights_match_positions() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let first_item = layout.visible_items[0].bounds;

        let match_fg = Theme::default().match_fg;
        // Scan the first item's row for the match-highlighted colour —
        // matched runs ('o', 'p' at byte offsets 0, 1 of "open file")
        // paint in `match_fg`, distinct from the row's normal foreground.
        let y = (first_item.y + first_item.height / 2.0) as u32;
        let found = (first_item.x as u32..(first_item.x + first_item.width) as u32).any(|x| {
            let px = surface.pixel_at(x, y);
            (px.r, px.g, px.b) == (match_fg.r, match_fg.g, match_fg.b)
        });
        assert!(
            found,
            "expected to find match_fg-coloured pixels on the matched item's row"
        );
    }

    #[test]
    fn selected_row_paints_selection_bg() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let first_item = layout.visible_items[0].bounds;
        let sel = Theme::default().selected_bg;
        let px = surface.pixel_at(
            (first_item.x + first_item.width - 2.0) as u32,
            (first_item.y + 1.0) as u32,
        );
        assert_eq!((px.r, px.g, px.b), (sel.r, sel.g, sel.b));
    }
}
