//! TUI rasteriser for [`crate::TextInput`].
//!
//! Paints a 1-cell border, the visible lines, and a block cursor
//! (inverted fg/bg on the cell at `cursor_bounds`). Placeholder
//! renders in `theme.muted_fg` when no content is present.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{qc, set_cell};
use crate::primitives::text_input::{TextInput, TextInputLayout, TextInputMeasure};
use crate::theme::Theme;

pub fn tui_text_input_layout(ti: &TextInput, rect: Rect) -> TextInputLayout {
    let r = crate::event::Rect::new(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );
    ti.layout(r, TextInputMeasure::new(1.0, 1.0))
}

pub fn draw_text_input(
    buf: &mut Buffer,
    rect: Rect,
    ti: &TextInput,
    theme: &Theme,
) -> TextInputLayout {
    let layout = tui_text_input_layout(ti, rect);
    if rect.width == 0 || rect.height == 0 {
        return layout;
    }

    let bg = qc(theme.background);
    let fg = qc(theme.foreground);
    let muted = qc(theme.muted_fg);
    let border_fg = if ti.has_focus {
        qc(theme.accent_fg)
    } else {
        qc(theme.border_fg)
    };

    // Fill background + border.
    let x0 = rect.x;
    let y0 = rect.y;
    let x1 = rect.x + rect.width.saturating_sub(1);
    let y1 = rect.y + rect.height.saturating_sub(1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            set_cell(buf, x, y, ' ', fg, bg);
        }
    }
    // Top + bottom border.
    for x in x0..=x1 {
        set_cell(buf, x, y0, '─', border_fg, bg);
        if y1 != y0 {
            set_cell(buf, x, y1, '─', border_fg, bg);
        }
    }
    // Left + right border.
    for y in y0..=y1 {
        set_cell(buf, x0, y, '│', border_fg, bg);
        if x1 != x0 {
            set_cell(buf, x1, y, '│', border_fg, bg);
        }
    }
    // Corners.
    set_cell(buf, x0, y0, '┌', border_fg, bg);
    set_cell(buf, x1, y0, '┐', border_fg, bg);
    set_cell(buf, x0, y1, '└', border_fg, bg);
    set_cell(buf, x1, y1, '┘', border_fg, bg);

    // Content.
    if layout.placeholder_active {
        if let Some(text) = ti.placeholder.as_ref() {
            if let Some(first) = layout.visible_lines.first() {
                let row_x = first.bounds.x as u16;
                let row_y = first.bounds.y as u16;
                let max_w = first.bounds.width as u16;
                for (x, ch) in (row_x..).zip(text.chars().take(max_w as usize)) {
                    set_cell(buf, x, row_y, ch, muted, bg);
                }
            }
        }
    } else {
        let h_scroll = layout.resolved_scroll_col;
        for vis in &layout.visible_lines {
            let line = ti.lines.get(vis.line_idx).map(String::as_str).unwrap_or("");
            let row_x = vis.bounds.x as u16;
            let row_y = vis.bounds.y as u16;
            let max_w = vis.bounds.width as u16;
            for (x, ch) in (row_x..).zip(line.chars().skip(h_scroll).take(max_w as usize)) {
                set_cell(buf, x, row_y, ch, fg, bg);
            }
        }
    }

    // Cursor — block cursor by inverting fg/bg on the cell.
    if ti.has_focus {
        if let Some(cb) = layout.cursor_bounds {
            let cx = cb.x as u16;
            let cy = cb.y as u16;
            let area = buf.area;
            if cx < area.x + area.width && cy < area.y + area.height {
                let cell = &mut buf[(cx, cy)];
                let old_fg = cell.fg;
                let old_bg = cell.bg;
                cell.set_fg(old_bg).set_bg(old_fg);
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::primitives::text_input::TextInputHit;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_input(lines: Vec<&str>) -> TextInput {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = lines.into_iter().map(String::from).collect();
        ti.has_focus = true;
        ti
    }

    // Parametrized over the area's origin: layout helpers must return
    // coords in the same frame across backends. `tui_text_input_layout`
    // bakes `rect.x`/`rect.y` straight into `ti.layout`'s returned bounds
    // (absolute frame: `TextInput::layout` computes
    // `content_x = rect.x + border + pad_x`), so this test verifies a full
    // paint+hit_test round trip at a non-zero origin.
    fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let rect = Rect::new(origin_x, origin_y, 10, 4);
        let mut buf = Buffer::empty(rect);
        let ti = mk_input(vec!["hello"]);
        let layout = draw_text_input(&mut buf, rect, &ti, &Theme::default());

        // Border top-left corner paints at the absolute origin.
        assert_eq!(cell_char(&buf, origin_x, origin_y), '┌');

        // Content starts after the 1-cell border + 1-char horizontal
        // padding, at the absolute screen position.
        let content_x = origin_x + 2;
        let content_y = origin_y + 1;
        assert_eq!(cell_char(&buf, content_x, content_y), 'h');

        // hit_test: a click at the first content cell (absolute coords)
        // should resolve to the (only) visible line.
        let p = Point::new(content_x as f32 + 0.5, content_y as f32 + 0.5);
        let hit = layout
            .hit_regions
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, h)| h.clone());
        assert_eq!(hit, Some(TextInputHit::Line { line_idx: 0 }));
    }

    #[test]
    fn paint_and_click_round_trip() {
        paint_and_click_round_trip_at(0, 0);
    }

    /// Paint and hit-test must work correctly when the input is not at the
    /// screen origin.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        paint_and_click_round_trip_at(7, 13);
    }

    /// TUI's focus treatment for a `TextInput`: the border tints to
    /// `theme.accent_fg` while `has_focus` is `true`, and back to
    /// `theme.border_fg` while it's `false`. This is the "existing
    /// treatment" the primitive's focus-ring seam defers to rather than
    /// adding a TUI-side ring stroke.
    #[test]
    fn border_tints_to_accent_when_focused_and_back_when_not() {
        let theme = Theme::default();
        let rect = Rect::new(0, 0, 10, 4);

        let mut focused = mk_input(vec!["hi"]);
        focused.has_focus = true;
        let mut buf_focused = Buffer::empty(rect);
        draw_text_input(&mut buf_focused, rect, &focused, &theme);

        let mut unfocused = mk_input(vec!["hi"]);
        unfocused.has_focus = false;
        let mut buf_unfocused = Buffer::empty(rect);
        draw_text_input(&mut buf_unfocused, rect, &unfocused, &theme);

        // Sample the top border edge (not a corner, not content) in both
        // buffers.
        let focused_border_fg = buf_focused[(1, 0)].fg;
        let unfocused_border_fg = buf_unfocused[(1, 0)].fg;

        assert_eq!(focused_border_fg, qc(theme.accent_fg));
        assert_eq!(unfocused_border_fg, qc(theme.border_fg));
        assert_ne!(
            focused_border_fg, unfocused_border_fg,
            "focus must change the painted border colour"
        );
    }
}
