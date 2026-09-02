//! Direct2D / DirectWrite rasteriser for [`crate::Terminal`] cell grids
//! (issue #30).
//!
//! Mirrors [`crate::macos::terminal::draw_terminal_cells`]'s structure:
//! walk `term.cells[row][col]`, fill each cell's background, then paint
//! the glyph (skipped for `' '`/`'\0'`). Overlay flags (`is_cursor`,
//! `is_find_active`, `is_find_match`, `selected`) override the cell's
//! `bg`/`fg`, matching every other backend's contract. Unlike the macOS
//! twin — which defers bold/italic/underline entirely — this rasteriser
//! paints `bold` cells through [`DWrite::draw_text_styled`], since that
//! variant already exists on this backend's `DWrite` (#25). `italic` /
//! `underline` are not yet applied: `DWrite` has no italic text format
//! or underline attribute wired up today (only GTK's Pango `AttrList`
//! does), and double-width (CJK/emoji) glyph handling (GTK's #439) is
//! also out of scope for this initial rasteriser — both are follow-ups
//! for whichever consumer needs them.
//!
//! GTK's #417 dirty-row repaint cache is a follow-up optimisation, not
//! required for parity — this rasteriser repaints the whole grid every
//! frame, the same posture `macos::terminal` shipped with.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod terminal;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::terminal::Terminal;
use crate::theme::Theme;
use crate::types::Color;

/// Draw `term`'s cell grid into the rectangular region starting at
/// `(x, y)` on `target`. `cell_area_w` clips per-row painting — cells
/// past the right edge stop being drawn rather than wrapping.
/// `cell_area_h` clips per-column painting vertically — rows whose top
/// falls at or below the pane bottom stop being drawn rather than
/// bleeding into whatever sits below the terminal (mirrors
/// `gtk`/`macos`'s quadraui#437 fix). `line_height` and `char_width` are
/// the per-cell dimensions in DIPs.
///
/// Callers that render with a scrollbar pass `cell_area_w = rect.width -
/// scrollbar_width` so the cell grid stops at the scrollbar gutter; the
/// gutter itself is painted by [`super::scrollbar::draw_scrollbar`] from
/// [`super::backend::WinBackend::draw_terminal`].
#[allow(clippy::too_many_arguments)]
pub fn draw_terminal_cells(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    term: &Terminal,
    x: f32,
    y: f32,
    cell_area_w: f32,
    cell_area_h: f32,
    line_height: f32,
    char_width: f32,
    theme: &Theme,
) {
    if cell_area_w <= 0.0 || cell_area_h <= 0.0 || line_height <= 0.0 || char_width <= 0.0 {
        return;
    }
    for (row_idx, row) in term.cells.iter().enumerate() {
        let row_y = y + row_idx as f32 * line_height;
        // Stop once a row's top has reached the pane bottom — such a row
        // belongs to a taller (pre-resize) grid and would bleed past the
        // pane. A row that merely straddles the bottom edge is still drawn
        // (and clipped by whatever the host paints over it).
        if row_y >= y + cell_area_h {
            break;
        }
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
            let _ = fill_rect(
                target,
                Rect::new(cell_x, row_y, char_width, line_height),
                cell_bg,
            );

            if cell.ch != ' ' && cell.ch != '\0' {
                let cell_fg = if cell.is_cursor {
                    cell.bg
                } else if cell.is_find_active {
                    Color::rgb(0, 0, 0)
                } else {
                    cell.fg
                };
                let s = cell.ch.to_string();
                let _ = dwrite.draw_text_styled(
                    target,
                    &s,
                    Rect::new(cell_x, row_y, char_width, line_height),
                    cell_fg,
                    cell.bold,
                );
            }

            cell_x += char_width;
        }
    }
}

/// Draw a vertical divider line for a terminal split pane. Paints a
/// 1-DIP-wide line at `rect.x` from `rect.y` to `rect.y + rect.height`
/// using `theme.separator` — `rect.width` is ignored, matching
/// [`crate::Backend::draw_terminal_divider`]'s documented contract.
pub fn draw_terminal_divider(target: &ID2D1RenderTarget, rect: Rect, theme: &Theme) {
    let _ = fill_rect(
        target,
        Rect::new(rect.x, rect.y, 1.0, rect.height),
        theme.separator,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 120;
    const CHAR_W: f32 = 10.0;
    const LINE_H: f32 = 20.0;

    fn cell(ch: char, fg: Color, bg: Color) -> crate::primitives::terminal::TerminalCell {
        crate::primitives::terminal::TerminalCell {
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

    fn dwrite() -> DWrite {
        DWrite::new("Segoe UI", 10.0).expect("create DWrite").0
    }

    /// A cell's background must paint at its own grid position before
    /// the glyph is drawn — probing a corner the glyph doesn't reach
    /// isolates the fill from font rasterisation.
    #[test]
    fn cell_bg_paints_at_its_grid_position() {
        let magenta = Color::rgb(200, 30, 200);
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('A', Color::rgb(255, 255, 255), magenta)]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (magenta.r, magenta.g, magenta.b));
    }

    /// The cursor overlay swaps fg/bg: the cell background paints in
    /// the cell's *foreground* colour.
    #[test]
    fn cursor_cell_inverts_fg_bg() {
        let fg = Color::rgb(10, 220, 30);
        let bg = Color::rgb(40, 40, 40);
        let mut c = cell('X', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (fg.r, fg.g, fg.b));
    }

    /// A find-active match paints its cell background orange —
    /// matching the GTK/macOS overlay convention.
    #[test]
    fn find_active_cell_paints_orange() {
        let mut c = cell('z', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.is_find_active = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (255, 165, 0));
    }

    /// A selected cell uses `theme.selection_bg`.
    #[test]
    fn selected_cell_uses_theme_selection_bg() {
        let mut c = cell('x', Color::rgb(255, 255, 255), Color::rgb(10, 10, 10));
        c.selected = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            )
        );
    }

    /// Rows past the pane bottom must be clipped, not bled — the
    /// interactive-resize regression `gtk`/`macos` already guard
    /// (quadraui#437 / #484).
    #[test]
    fn rows_past_the_pane_bottom_are_clipped() {
        let magenta = Color::rgb(200, 30, 200);
        let bg_sentinel = Color::rgb(1, 2, 3);
        let cells: Vec<Vec<_>> = (0..60)
            .map(|_| {
                (0..20)
                    .map(|_| cell(' ', Color::rgb(255, 255, 255), magenta))
                    .collect()
            })
            .collect();
        let term = Terminal {
            id: WidgetId::new("term"),
            cells,
            scrollbar: None,
        };
        let pane_h = 60.0_f32;
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                let _ = fill_rect(target, Rect::new(0.0, 0.0, W as f32, H as f32), bg_sentinel);
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, pane_h, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let inside = surface.pixel_at(4, 4);
        assert_eq!(
            (inside.r, inside.g, inside.b),
            (magenta.r, magenta.g, magenta.b)
        );

        let below = surface.pixel_at(4, H - 4);
        assert_eq!(
            (below.r, below.g, below.b),
            (bg_sentinel.r, bg_sentinel.g, bg_sentinel.b),
            "rows below the pane bottom must be clipped, not bled"
        );
    }

    /// A bold cell paints via `DWrite::draw_text_styled` rather than
    /// silently dropping the attribute — smoke-tested by confirming the
    /// paint call succeeds and the cell's own bg still lands correctly
    /// (glyph shape/weight isn't independently probable via pixel
    /// colour without OCR, so this only guards the plumbing).
    #[test]
    fn bold_cell_paints_without_error() {
        let mut c = cell('B', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.bold = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                draw_terminal_cells(
                    target, &dwrite, &term, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, &theme,
                );
            })
            .expect("paint");

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (20, 20, 20));
    }

    /// [`draw_terminal_divider`] paints a 1-DIP line at `rect.x`,
    /// ignoring `rect.width`.
    #[test]
    fn divider_paints_a_one_dip_line() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let theme = Theme::default();
        surface
            .paint(|target| {
                let _ = fill_rect(
                    target,
                    Rect::new(0.0, 0.0, W as f32, H as f32),
                    Color::rgb(0, 0, 0),
                );
                draw_terminal_divider(target, Rect::new(50.0, 0.0, 999.0, H as f32), &theme);
            })
            .expect("paint");

        let on_line = surface.pixel_at(50, 10);
        let off_line = surface.pixel_at(52, 10);
        assert_eq!(
            (on_line.r, on_line.g, on_line.b),
            (theme.separator.r, theme.separator.g, theme.separator.b)
        );
        assert_eq!((off_line.r, off_line.g, off_line.b), (0, 0, 0));
    }
}
