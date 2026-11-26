//! The pixel painter for [`Editor`], written against [`PaintSurface`]:
//! the window background, selections, each visible line's text in the
//! editor font, and the primary caret.

use super::{CursorShape, Editor, EditorLayout, EditorSelection};
use crate::paint_surface::PaintSurface;
use crate::theme::Theme;
use crate::types::Color;
use crate::Rect;

/// Paint `editor` into the rects `layout` resolved.
pub(crate) fn paint(
    editor: &Editor,
    layout: &EditorLayout,
    surface: &mut dyn PaintSurface,
    theme: &Theme,
) {
    let mut paint_in = |s: &mut dyn PaintSurface| paint_body(editor, layout, s, theme);
    if !surface.surface_with_editor_font(&mut paint_in) {
        paint_body(editor, layout, surface, theme);
    }
}

fn paint_body(
    editor: &Editor,
    layout: &EditorLayout,
    surface: &mut dyn PaintSurface,
    theme: &Theme,
) {
    let window_bg = if editor.show_active_bg {
        theme.editor_active_background
    } else {
        theme.background
    };
    surface.surface_fill_rect(layout.bounds, window_bg);

    let text = layout.text_bounds;
    let cw = layout.cell_width;
    let lh = layout.line_height;
    surface.surface_push_clip(text);

    for sel in editor
        .selection
        .iter()
        .chain(editor.extra_selections.iter())
    {
        paint_selection(editor, layout, surface, sel, theme.selection);
    }

    for (row, line) in editor.lines.iter().enumerate().take(layout.visible_lines) {
        let y = text.y + row as f32 * lh;
        let x0 = text.x - layout.scroll_left as f32 * cw;
        let raw = line.raw_text.trim_end_matches(['\n', '\r']);
        if line.spans.is_empty() {
            surface.surface_draw_text_run(
                Rect::new(x0, y, text.width + layout.scroll_left as f32 * cw, lh),
                raw,
                theme.foreground,
            );
            continue;
        }
        for span in &line.spans {
            let start = crate::text_util::snap_to_char_boundary(raw, span.start_byte);
            let end = crate::text_util::snap_to_char_boundary(raw, span.end_byte.min(raw.len()));
            if start >= end {
                continue;
            }
            let col = raw[..start].chars().count() as f32;
            surface.surface_draw_text_run_styled(
                Rect::new(x0 + col * cw, y, text.width, lh),
                &raw[start..end],
                span.style.fg,
                span.style.bold,
                span.style.italic,
                false,
                1.0,
            );
        }
    }

    if let Some(cursor) = &editor.cursor {
        if cursor.pos.view_line < layout.visible_lines {
            let x = layout.x_at_col(editor, cursor.pos.view_line, cursor.pos.col);
            let y = text.y + cursor.pos.view_line as f32 * lh;
            let rect = match cursor.shape {
                CursorShape::Block => Rect::new(x, y, cw, lh),
                CursorShape::Bar => Rect::new(x, y, 2.0, lh),
                CursorShape::Underline => Rect::new(x, y + lh - 2.0, cw, 2.0),
            };
            surface.surface_fill_rect(rect, theme.cursor);
        }
    }

    surface.surface_pop_clip();
}

/// Fill the cells `sel` covers on each visible row.
fn paint_selection(
    editor: &Editor,
    layout: &EditorLayout,
    surface: &mut dyn PaintSurface,
    sel: &EditorSelection,
    color: Color,
) {
    for (row, line) in editor.lines.iter().enumerate().take(layout.visible_lines) {
        let Some(cols) = sel.cols_on(line) else {
            continue;
        };
        let x0 = layout.x_at_col(editor, row, cols.start + line.segment_col_offset);
        let x1 = layout.x_at_col(editor, row, cols.end + line.segment_col_offset);
        let y = layout.text_bounds.y + row as f32 * layout.line_height;
        surface.surface_fill_rect(
            Rect::new(x0, y, (x1 - x0).max(0.0), layout.line_height),
            color,
        );
    }
}
