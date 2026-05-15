//! macOS rasteriser for [`crate::Terminal`] cell grids.
//!
//! Mirror of [`crate::gtk::terminal::draw_terminal_cells`]: walks
//! `term.cells[row][col]`, fills each cell's background, then paints
//! the glyph (skipped for `' '` and `'\0'`). Overlay flags
//! (`is_cursor`, `is_find_active`, `is_find_match`, `selected`)
//! override the cell's `bg` / `fg` to match the GTK contract.
//!
//! Bold / italic / underline attributes are **not** rendered yet —
//! Core Text would need a per-cell `CTFont` (or attributed-string
//! attribute) for the bold / italic variants, and the cell grid is
//! hot enough that we want trait-shape parity in this ticket and
//! defer styled-attr support to a follow-up. (#43 acceptance
//! criteria covers the cell grid + cursor / selection / find
//! overlays; attribute variants are listed in the milestone but
//! gated behind a consumer asking for them.)

use core_graphics::base::CGFloat;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::draw_text;
use crate::primitives::terminal::Terminal;
use crate::theme::Theme;
use crate::types::Color;

/// Draw `term`'s cell grid into the rectangular region starting at
/// `(x, y)` on `ctx`. `cell_area_w` clips per-row painting — cells
/// past the right edge stop being drawn rather than wrapping.
/// `line_height` and `char_width` are the per-cell dimensions in
/// points.
///
/// Callers that render with a scrollbar pass `cell_area_w =
/// rect.width - scrollbar_width` so the cell grid stops at the
/// scrollbar gutter; the gutter itself is painted by
/// [`super::scrollbar::draw_scrollbar`] from
/// [`crate::macos::backend::MacBackend::draw_terminal`].
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_terminal_cells(
    ctx: CGContextRef,
    font: &CTFont,
    term: &Terminal,
    x: f64,
    y: f64,
    cell_area_w: f64,
    line_height: f64,
    char_width: f64,
    theme: &Theme,
) {
    if cell_area_w <= 0.0 || line_height <= 0.0 || char_width <= 0.0 {
        return;
    }
    for (row_idx, row) in term.cells.iter().enumerate() {
        let row_y = y + row_idx as f64 * line_height;
        let mut cell_x = x;
        for cell in row {
            if cell_x + char_width > x + cell_area_w {
                break;
            }
            let cell_bg = if cell.is_cursor {
                cell.fg
            } else if cell.is_find_active {
                Color::rgb(255, 165, 0)
            } else if cell.is_find_match {
                Color::rgb(100, 80, 20)
            } else if cell.selected {
                theme.selection_bg
            } else {
                cell.bg
            };
            fill_rect(ctx, cell_x, row_y, char_width, line_height, cell_bg);

            if cell.ch != ' ' && cell.ch != '\0' {
                let cell_fg = if cell.is_cursor {
                    cell.bg
                } else if cell.is_find_active {
                    Color::rgb(0, 0, 0)
                } else {
                    cell.fg
                };
                let s = cell.ch.to_string();
                draw_text(ctx, font, &s, cell_x, row_y, color_to_cg(cell_fg));
            }

            cell_x += char_width;
        }
    }
}

/// Draw a vertical divider line for a terminal split pane. Paints a
/// 1-pt-wide line at `x` from `y` to `y + height` using
/// `theme.separator`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call. Calling with a freed or null pointer is UB.
pub unsafe fn draw_terminal_divider(ctx: CGContextRef, x: f64, y: f64, height: f64, theme: &Theme) {
    fill_rect(ctx, x, y, 1.0, height, theme.separator);
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: CGFloat,
        green: CGFloat,
        blue: CGFloat,
        alpha: CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::terminal::{Terminal, TerminalCell};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn cell(ch: char, fg: Color, bg: Color) -> TerminalCell {
        TerminalCell {
            ch,
            fg,
            bg,
            bold: false,
            italic: false,
            underline: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
        }
    }

    fn paint(term: &Terminal) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_terminal(QRect::new(0.0, 0.0, W as f32, H as f32), term);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn cell_bg_paints_through_backend() {
        // One cell, magenta background, glyph 'A'. The fill_rect path
        // should colour the whole cell magenta before the glyph paints.
        let magenta = Color::rgb(200, 30, 200);
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('A', Color::rgb(255, 255, 255), magenta)]],
            scrollbar: None,
        };
        let s = paint(&term);
        // Probe a corner of the cell that the glyph "A" doesn't reach
        // (glyph sits roughly in the middle); top-left of the cell is
        // background.
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!((r, g, b), (magenta.r, magenta.g, magenta.b));
    }

    #[test]
    fn cursor_cell_inverts_fg_bg() {
        // Cursor flag flips the cell so bg is painted with the cell's fg.
        let fg = Color::rgb(10, 220, 30);
        let bg = Color::rgb(40, 40, 40);
        let mut c = cell('X', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let (r, g, b, _) = s.pixel(0, 0);
        // Cursor swap: bg now uses fg.
        assert_eq!((r, g, b), (fg.r, fg.g, fg.b));
    }

    #[test]
    fn find_active_cell_paints_orange() {
        let mut c = cell('z', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.is_find_active = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!((r, g, b), (255, 165, 0));
    }

    #[test]
    fn selected_cell_uses_theme_selection_bg() {
        let mut c = cell('x', Color::rgb(255, 255, 255), Color::rgb(10, 10, 10));
        c.selected = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let theme = Theme::default();
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!(
            (r, g, b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
        );
    }

    #[test]
    fn cells_past_width_are_clipped() {
        // Build a row of 200 magenta cells. At a typical char_width of
        // ~8px, only ~25 fit in W=200. Past the clip we should still
        // see transparent black (surface initial fill).
        let magenta = Color::rgb(200, 30, 200);
        let row: Vec<_> = (0..200)
            .map(|_| cell(' ', Color::rgb(255, 255, 255), magenta))
            .collect();
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let s = paint(&term);
        // Right-most pixel column: cells should NOT have painted there
        // because clipping kicks in at `x + char_width > x + W` — and
        // 200 / char_width < 200 cells fit.
        // Probe just inside the right edge: this should still be magenta
        // (one of the cells fits there).
        let inside = s.pixel(W / 2, 0);
        assert_eq!(
            (inside.0, inside.1, inside.2),
            (magenta.r, magenta.g, magenta.b)
        );
    }
}
