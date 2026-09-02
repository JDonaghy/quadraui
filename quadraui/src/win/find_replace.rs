//! Direct2D / DirectWrite rasteriser for [`crate::FindReplacePanel`]
//! (issue #28).
//!
//! Mirrors `gtk::find_replace`'s structure: `panel.hit_regions` (built
//! once via [`crate::primitives::find_replace::compute_hit_regions`] at
//! panel construction) drives both paint and click hit-test, so the two
//! can't drift apart. `char_width` / `line_height` are the editor's
//! monospace cell dimensions (DIPs); cell-unit hit-region coordinates
//! are scaled by these to absolute Direct2D coordinates.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod find_replace;` and `backend.rs`'s
//! module docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::find_replace::{FindReplaceClickTarget, FindReplacePanel};
use crate::theme::Theme;

/// Draw a [`FindReplacePanel`] anchored at the top-right of
/// `panel.group_bounds`. Walks `panel.hit_regions` so paint and click
/// hit-test agree by construction (mirrors `gtk::draw_find_replace`).
pub fn draw_find_replace(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    panel: &FindReplacePanel,
    line_height: f32,
    char_width: f32,
) {
    use FindReplaceClickTarget as T;

    let cw = char_width.max(1.0);
    let lh = line_height.max(1.0);
    let theme = Theme::default();

    let popup_w = panel.panel_width as f32 * cw;
    let row_count = if panel.show_replace { 2.0 } else { 1.0 };
    let popup_h = (row_count + 2.0) * lh;

    let gb = panel.group_bounds;
    let popup_x = ((gb.x + gb.width) - popup_w - 10.0).max(gb.x);
    let popup_y = gb.y + 2.0;
    let popup_rect = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let _ = fill_rect(target, popup_rect, theme.surface_bg);
    let _ = stroke_rect(target, popup_rect, theme.separator, 1.0);

    let content_x = popup_x + cw;
    let content_y = popup_y + lh;

    let paint_toggle = |col: u16, row: u16, width: u16, label: &str, active: bool| {
        let bx = content_x + col as f32 * cw;
        let by = content_y + row as f32 * lh;
        let bw = width as f32 * cw;
        let rect = Rect::new(bx, by, bw.max(1.0), lh);
        let fg = if active {
            let _ = fill_rect(target, rect, theme.accent_bg);
            theme.background
        } else {
            let _ = stroke_rect(target, rect, theme.separator, 0.5);
            theme.foreground
        };
        let _ = dwrite.draw_text(target, label, rect, fg);
    };

    let paint_glyph = |col: u16, row: u16, width: u16, label: &str, active: bool| {
        let bx = content_x + col as f32 * cw;
        let by = content_y + row as f32 * lh;
        let bw = width as f32 * cw;
        let rect = Rect::new(bx, by, bw.max(1.0), lh);
        let fg = if active {
            let _ = fill_rect(target, rect, theme.accent_bg);
            theme.background
        } else {
            theme.foreground
        };
        let _ = dwrite.draw_text(target, label, rect, fg);
    };

    let paint_input = |col: u16,
                       row: u16,
                       width: u16,
                       text: &str,
                       is_focused: bool,
                       cursor: usize,
                       sel_anchor: Option<usize>| {
        let bx = content_x + col as f32 * cw;
        let by = content_y + row as f32 * lh;
        let bw = width as f32 * cw;
        let rect = Rect::new(bx, by, bw.max(1.0), lh);
        let _ = fill_rect(target, rect, theme.background);
        let _ = stroke_rect(target, rect, theme.separator, 0.5);

        let text_rect = Rect::new(bx + 4.0, by, (bw - 4.0).max(1.0), lh);
        let _ = dwrite.draw_text(target, text, text_rect, theme.foreground);

        if !is_focused {
            return;
        }

        let char_x = |col: usize| -> f32 {
            let prefix_end = text
                .char_indices()
                .nth(col)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            let (w, _) = dwrite
                .measure_text(&text[..prefix_end])
                .unwrap_or((0.0, 0.0));
            bx + 4.0 + w
        };

        if let Some(anchor) = sel_anchor {
            let s = anchor.min(cursor);
            let e = anchor.max(cursor);
            if s != e {
                let sx = char_x(s);
                let ex = char_x(e);
                let sel_rect = Rect::new(sx, by, (ex - sx).max(1.0), lh);
                let _ = fill_rect(target, sel_rect, theme.selection_bg);
            }
        }

        let cx = char_x(cursor);
        let cursor_rect = Rect::new(cx, by + 2.0, 2.0, (lh - 4.0).max(1.0));
        let _ = fill_rect(target, cursor_rect, theme.foreground);
    };

    let mut regex_end_col: Option<u16> = None;
    let mut prev_match_col: Option<u16> = None;

    for (region, target_kind) in &panel.hit_regions {
        match target_kind {
            T::Chevron => {
                let chevron = if panel.show_replace {
                    "\u{25bc}"
                } else {
                    "\u{25b6}"
                };
                let bx = content_x + region.col as f32 * cw;
                let by = content_y + region.row as f32 * lh;
                let rect = Rect::new(bx, by, (region.width as f32 * cw).max(1.0), lh);
                let _ = dwrite.draw_text(target, chevron, rect, theme.foreground);
            }
            T::FindInput(_) => {
                paint_input(
                    region.col,
                    region.row,
                    region.width,
                    &panel.query,
                    panel.focus == 0,
                    panel.cursor,
                    panel.sel_anchor,
                );
            }
            T::ReplaceInput(_) => {
                paint_input(
                    region.col,
                    region.row,
                    region.width,
                    &panel.replacement,
                    panel.focus == 1,
                    panel.cursor,
                    panel.sel_anchor,
                );
            }
            T::ToggleCase => paint_toggle(
                region.col,
                region.row,
                region.width,
                "Aa",
                panel.case_sensitive,
            ),
            T::ToggleWholeWord => {
                paint_toggle(region.col, region.row, region.width, "ab", panel.whole_word)
            }
            T::ToggleRegex => {
                paint_toggle(region.col, region.row, region.width, ".*", panel.use_regex);
                regex_end_col = Some(region.col + region.width);
            }
            T::PrevMatch => {
                paint_glyph(region.col, region.row, region.width, "\u{2191}", false);
                prev_match_col.get_or_insert(region.col);
            }
            T::NextMatch => paint_glyph(region.col, region.row, region.width, "\u{2193}", false),
            T::ToggleInSelection => paint_glyph(
                region.col,
                region.row,
                region.width,
                "\u{2261}",
                panel.in_selection,
            ),
            T::Close => paint_glyph(region.col, region.row, region.width, "\u{00d7}", false),
            T::TogglePreserveCase => paint_toggle(
                region.col,
                region.row,
                region.width,
                "AB",
                panel.preserve_case,
            ),
            T::ReplaceCurrent => paint_glyph(
                region.col,
                region.row,
                region.width,
                &panel.replace_one_glyph,
                false,
            ),
            T::ReplaceAll => paint_glyph(
                region.col,
                region.row,
                region.width,
                &panel.replace_all_glyph,
                false,
            ),
        }
    }

    if let (Some(start_col), Some(end_col)) = (regex_end_col, prev_match_col) {
        let info_col = start_col + 1;
        if end_col > info_col + 1 {
            let px = content_x + info_col as f32 * cw;
            let py = content_y;
            let rect = Rect::new(px, py, ((end_col - info_col) as f32 * cw).max(1.0), lh);
            let _ = dwrite.draw_text(target, &panel.match_info, rect, theme.foreground);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::find_replace::compute_hit_regions;
    use crate::win::testing::HeadlessSurface;

    fn sample_panel(query: &str, cursor: usize, sel_anchor: Option<usize>) -> FindReplacePanel {
        let (hit_regions, _input_width) = compute_hit_regions(50, false, "1 of 3", 2, 2);
        FindReplacePanel {
            query: query.into(),
            replacement: String::new(),
            show_replace: false,
            focus: 0,
            cursor,
            sel_anchor,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds: Rect::new(0.0, 0.0, 600.0, 200.0),
            panel_width: 50,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions,
        }
    }

    #[test]
    fn paints_panel_background_and_border() {
        let surface = HeadlessSurface::new(600, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let panel = sample_panel("needle", 3, None);
        let theme = Theme::default();

        surface
            .paint(|target| {
                draw_find_replace(target, &dwrite, &panel, 16.0, 8.0);
            })
            .expect("paint find/replace");

        let popup_w = panel.panel_width as f32 * 8.0;
        let popup_h = 3.0 * 16.0;
        let popup_x = (panel.group_bounds.x + panel.group_bounds.width - popup_w - 10.0)
            .max(panel.group_bounds.x);
        let popup_y = panel.group_bounds.y + 2.0;

        let inner = surface.pixel_at(
            (popup_x + popup_w - 4.0) as u32,
            (popup_y + popup_h - 4.0) as u32,
        );
        assert_eq!(
            (inner.r, inner.g, inner.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b)
        );
    }

    /// Regression twin of `gtk::find_replace`'s #503 multibyte-cursor
    /// test: `cursor`/`sel_anchor` are char offsets that may not land on
    /// a byte boundary-adjacent split without care — this must not panic.
    #[test]
    fn draw_find_replace_with_multibyte_query_does_not_panic() {
        let surface = HeadlessSurface::new(600, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let panel = sample_panel("caf\u{e9}\u{1f389}\u{4e2d}\u{6587}", 3, Some(6));

        surface
            .paint(|target| {
                draw_find_replace(target, &dwrite, &panel, 16.0, 8.0);
            })
            .expect("paint find/replace");
    }
}
