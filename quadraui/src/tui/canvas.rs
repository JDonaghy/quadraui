//! TUI rasteriser for [`crate::Canvas`] (issue #1102).
//!
//! See `crate::primitives::canvas`'s module doc for the full per-`DrawOp`
//! degrade table (issue #1097/D-014). This file implements it:
//!
//! - [`crate::DrawOp::Rect`] / [`crate::DrawOp::RoundedRect`] /
//!   [`crate::DrawOp::Line`] / [`crate::DrawOp::Path`] rasterise into the
//!   same braille sub-cell dot grid [`super::chart`]'s line charts use
//!   ([`super::braille`]).
//! - [`crate::DrawOp::TextRun`] / [`crate::DrawOp::Image`] snap to the
//!   nearest whole cell (text can't live inside a dot cell) — `Image`
//!   paints [`crate::Image::fallback_text`], same posture as
//!   [`super::image::draw_image`].
//! - [`crate::DrawOp::PushClip`] / [`crate::DrawOp::PopClip`] quantise
//!   their rect outward to whole cells.
//!
//! # Z-order: shapes under text, not strict declared order
//!
//! Every pixel backend paints `ops` in exactly the order the app
//! declared them (`primitives::canvas::native_surface_paint::paint`) —
//! a later op visually covers an earlier one at the same spot. TUI
//! cannot do that cheaply: packing one cell's braille glyph needs that
//! cell's *final* dot state, which isn't known until every shape op has
//! been walked, so shape ops are accumulated into one dot grid first and
//! rendered in a single pass; text/image ops paint directly onto the
//! cell buffer afterwards, in their own declared order, on top of that
//! pass. The practical effect: a background fill declared *before* a
//! label still renders correctly (the overwhelmingly common pattern —
//! the alternative would make every label fight its own background);
//! the uncommon reverse — a shape declared *after* a label, intended to
//! cover it — does not cover it on TUI the way it would on a pixel
//! backend. This is a real, documented divergence from "same ops, same
//! backend-relative result" (D-014's bar is "same information", not
//! "identical z-order on a backend with no sub-cell text at all") —
//! named here rather than discovered by a future bug report.
//!
//! Off-canvas line/path endpoints clamp to the dot grid's edge rather
//! than draw into negative space — there is nothing past the grid to
//! draw into. A line/path segment merges through a full-size scratch
//! grid per segment (`interpolate_dots` into a throwaway grid, then
//! copied cell-by-cell through the shared clip/colour bookkeeping) —
//! simple and correct, not tuned for a canvas with hundreds of
//! segments; a future pass could rasterise directly with clipping built
//! into the stepper instead.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RRect;

use super::braille::{interpolate_dots, pack_braille_cell};
use super::{draw_styled_text, qc, set_cell};
use crate::event::{Point, Rect};
use crate::primitives::canvas::{Canvas, CanvasLayout, DrawOp};
use crate::theme::Theme;
use crate::types::{Color, Decoration, StyledText};

/// Compute the TUI cell-unit layout for a [`Canvas`] without painting.
pub fn tui_canvas_layout(canvas: &Canvas, area: RRect) -> CanvasLayout {
    canvas.layout(Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ))
}

/// Axis-aligned clip in whole cells, local to the canvas (`0..cols`,
/// `0..rows`) — the cell-quantised degrade for
/// [`DrawOp::PushClip`]/[`DrawOp::PopClip`] (module doc).
#[derive(Clone, Copy)]
struct CellClip {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl CellClip {
    fn full(cols: i32, rows: i32) -> Self {
        Self {
            x0: 0,
            y0: 0,
            x1: cols,
            y1: rows,
        }
    }

    /// Quantise `rect` (local canvas units) outward to whole cells and
    /// intersect with `self`.
    fn push(&self, rect: Rect) -> Self {
        let x0 = rect.x.floor() as i32;
        let y0 = rect.y.floor() as i32;
        let x1 = (rect.x + rect.width).ceil() as i32;
        let y1 = (rect.y + rect.height).ceil() as i32;
        Self {
            x0: self.x0.max(x0),
            y0: self.y0.max(y0),
            x1: self.x1.min(x1),
            y1: self.y1.min(y1),
        }
    }

    fn contains_cell(&self, cx: i32, cy: i32) -> bool {
        cx >= self.x0 && cx < self.x1 && cy >= self.y0 && cy < self.y1
    }
}

/// The clip active while op `i` executes, for every op in `ops` — a
/// single linear replay of the push/pop stack, shared by the shape pass
/// and the text/image pass so neither re-derives it differently.
fn clip_per_op(ops: &[DrawOp], cols: i32, rows: i32) -> Vec<CellClip> {
    let mut stack = vec![CellClip::full(cols, rows)];
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        out.push(*stack.last().unwrap());
        match op {
            DrawOp::PushClip { rect } => {
                let top = *stack.last().unwrap();
                stack.push(top.push(*rect));
            }
            DrawOp::PopClip if stack.len() > 1 => {
                stack.pop();
            }
            _ => {}
        }
    }
    out
}

/// Scale a canvas-local x/y (in cell units, fractional) to the dot grid
/// (2 dots wide, 4 dots tall per cell).
fn rect_to_dot_bounds(rect: Rect) -> (i32, i32, i32, i32) {
    let x0 = (rect.x * 2.0).round() as i32;
    let y0 = (rect.y * 4.0).round() as i32;
    let x1 = ((rect.x + rect.width) * 2.0).round() as i32;
    let y1 = ((rect.y + rect.height) * 4.0).round() as i32;
    (x0, y0, x1, y1)
}

fn point_to_dot(p: Point) -> (i32, i32) {
    ((p.x * 2.0).round() as i32, (p.y * 4.0).round() as i32)
}

/// Set one dot, honouring `clip` (cell-granularity) and recording
/// `color` as the cell's current colour (last writer wins, same as
/// every other overlapping-paint primitive in this crate).
fn set_dot(
    dots: &mut [Vec<bool>],
    cell_color: &mut [Vec<Option<Color>>],
    clip: &CellClip,
    dx: i32,
    dy: i32,
    color: Color,
) {
    if dx < 0 || dy < 0 {
        return;
    }
    let dot_h = dots.len() as i32;
    let dot_w = dots.first().map(|r| r.len()).unwrap_or(0) as i32;
    if dx >= dot_w || dy >= dot_h {
        return;
    }
    let cell_x = dx / 2;
    let cell_y = dy / 4;
    if !clip.contains_cell(cell_x, cell_y) {
        return;
    }
    dots[dy as usize][dx as usize] = true;
    cell_color[cell_y as usize][cell_x as usize] = Some(color);
}

fn accumulate_rect(
    dots: &mut [Vec<bool>],
    cell_color: &mut [Vec<Option<Color>>],
    clip: &CellClip,
    rect: Rect,
    color: Color,
    dot_w: i32,
    dot_h: i32,
) {
    let (x0, y0, x1, y1) = rect_to_dot_bounds(rect);
    for dy in y0.max(0)..y1.min(dot_h) {
        for dx in x0.max(0)..x1.min(dot_w) {
            set_dot(dots, cell_color, clip, dx, dy, color);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn accumulate_segment(
    dots: &mut [Vec<bool>],
    cell_color: &mut [Vec<Option<Color>>],
    clip: &CellClip,
    from: Point,
    to: Point,
    color: Color,
    dot_w: i32,
    dot_h: i32,
) {
    let (x0, y0) = point_to_dot(from);
    let (x1, y1) = point_to_dot(to);
    // Endpoints explicitly — `interpolate_dots` no-ops when they
    // coincide (module doc), and clamping below can otherwise lose an
    // endpoint that lands exactly on the grid's far edge.
    set_dot(dots, cell_color, clip, x0, y0, color);
    set_dot(dots, cell_color, clip, x1, y1, color);
    if (x0, y0) == (x1, y1) || dot_w <= 0 || dot_h <= 0 {
        return;
    }
    let cx0 = x0.clamp(0, dot_w - 1) as usize;
    let cy0 = y0.clamp(0, dot_h - 1) as usize;
    let cx1 = x1.clamp(0, dot_w - 1) as usize;
    let cy1 = y1.clamp(0, dot_h - 1) as usize;
    let mut scratch = vec![vec![false; dot_w as usize]; dot_h as usize];
    interpolate_dots(&mut scratch, cx0, cy0, cx1, cy1);
    for (sy, row) in scratch.iter().enumerate() {
        for (sx, &on) in row.iter().enumerate() {
            if on {
                set_dot(dots, cell_color, clip, sx as i32, sy as i32, color);
            }
        }
    }
}

/// Paint `text` at `rect`'s cell-quantised top-left corner, clipped to
/// `clip` and to `area`'s own width/height.
#[allow(clippy::too_many_arguments)]
fn paint_cell_quantised_text(
    buf: &mut Buffer,
    area: RRect,
    clip: &CellClip,
    rect: Rect,
    text: &str,
    fg: Color,
    bg: Color,
) {
    if text.is_empty() {
        return;
    }
    let cell_x = rect.x.round() as i32;
    let cell_y = rect.y.round() as i32;
    if cell_y < clip.y0 || cell_y >= clip.y1 || cell_y < 0 || cell_y >= area.height as i32 {
        return;
    }
    let start_x = cell_x.max(clip.x0).max(0);
    let end_x = (area.width as i32).min(clip.x1);
    if start_x >= end_x {
        return;
    }
    let text_area = RRect::new(
        area.x + start_x as u16,
        area.y + cell_y as u16,
        (end_x - start_x) as u16,
        1,
    );
    let styled = StyledText::plain(text);
    let fg = qc(fg);
    let bg = qc(bg);
    draw_styled_text(
        buf,
        text_area,
        text_area.y,
        0,
        &styled,
        fg,
        bg,
        Decoration::Normal,
        fg,
    );
}

/// Draw a [`Canvas`]'s `ops` into `area` on `buf`. Returns the layout for
/// host click dispatch — see [`crate::CanvasHit`] for why there is
/// nothing finer-grained to return.
pub fn draw_canvas(buf: &mut Buffer, area: RRect, canvas: &Canvas, theme: &Theme) -> CanvasLayout {
    let layout = tui_canvas_layout(canvas, area);
    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let cols = area.width as i32;
    let rows = area.height as i32;
    let dot_w = cols * 2;
    let dot_h = rows * 4;
    let clips = clip_per_op(&canvas.ops, cols, rows);

    // ── Pass 1: accumulate every shape op into one dot grid (module
    // doc's "Z-order" section — shapes always render as one unit,
    // regardless of how they interleave with text/image ops below).
    let mut dots = vec![vec![false; dot_w as usize]; dot_h as usize];
    let mut cell_color: Vec<Vec<Option<Color>>> = vec![vec![None; cols as usize]; rows as usize];
    for (op, clip) in canvas.ops.iter().zip(&clips) {
        match op {
            DrawOp::Rect { rect, color } | DrawOp::RoundedRect { rect, color, .. } => {
                accumulate_rect(
                    &mut dots,
                    &mut cell_color,
                    clip,
                    *rect,
                    *color,
                    dot_w,
                    dot_h,
                );
            }
            DrawOp::Line {
                from, to, color, ..
            } => {
                accumulate_segment(
                    &mut dots,
                    &mut cell_color,
                    clip,
                    *from,
                    *to,
                    *color,
                    dot_w,
                    dot_h,
                );
            }
            DrawOp::Path {
                points,
                color,
                closed,
                ..
            } => {
                for pair in points.windows(2) {
                    accumulate_segment(
                        &mut dots,
                        &mut cell_color,
                        clip,
                        pair[0],
                        pair[1],
                        *color,
                        dot_w,
                        dot_h,
                    );
                }
                if *closed && points.len() > 1 {
                    accumulate_segment(
                        &mut dots,
                        &mut cell_color,
                        clip,
                        points[points.len() - 1],
                        points[0],
                        *color,
                        dot_w,
                        dot_h,
                    );
                }
            }
            DrawOp::TextRun { .. }
            | DrawOp::Image { .. }
            | DrawOp::PushClip { .. }
            | DrawOp::PopClip => {}
        }
    }

    let bg = qc(theme.background);
    for cell_row in 0..rows as usize {
        for cell_col in 0..cols as usize {
            let ch = pack_braille_cell(|dr, dc| dots[cell_row * 4 + dr][cell_col * 2 + dc]);
            if ch != '\u{2800}' {
                if let Some(color) = cell_color[cell_row][cell_col] {
                    let x = area.x + cell_col as u16;
                    let y = area.y + cell_row as u16;
                    set_cell(buf, x, y, ch, qc(color), bg);
                }
            }
        }
    }

    // ── Pass 2: text/image ops paint on top, in declared order.
    for (op, clip) in canvas.ops.iter().zip(&clips) {
        match op {
            DrawOp::TextRun { rect, text, color } => {
                paint_cell_quantised_text(buf, area, clip, *rect, text, *color, theme.background);
            }
            DrawOp::Image { rect, image } if !image.fallback_text.is_empty() => {
                paint_cell_quantised_text(
                    buf,
                    area,
                    clip,
                    *rect,
                    &image.fallback_text,
                    theme.foreground,
                    theme.background,
                );
            }
            _ => {}
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::canvas::CanvasHit;
    use crate::primitives::image::{Image, ImageFit, ImageSource};
    use crate::types::WidgetId;

    fn canvas(ops: Vec<DrawOp>) -> Canvas {
        Canvas {
            id: WidgetId::new("canvas"),
            ops,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn rect_op_paints_a_nonblank_braille_cell_inside_and_nothing_outside() {
        let area = RRect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::Rect {
            rect: Rect::new(0.0, 0.0, 2.0, 2.0),
            color: Color::rgb(200, 0, 0),
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());

        assert_ne!(cell_char(&buf, 0, 0), ' ');
        assert_ne!(cell_char(&buf, 1, 1), ' ');
        assert_eq!(cell_char(&buf, 5, 3), ' ');
    }

    #[test]
    fn rounded_rect_degrades_to_a_plain_filled_rect_on_tui() {
        let area = RRect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::RoundedRect {
            rect: Rect::new(0.0, 0.0, 2.0, 2.0),
            radius: 50.0,
            color: Color::rgb(0, 200, 0),
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        // Same cell the plain-rect test above covers — radius is dropped,
        // not refused (module doc's degrade table).
        assert_ne!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn line_op_lights_a_dot_near_its_far_endpoint() {
        let area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::Line {
            from: Point::new(0.0, 0.0),
            to: Point::new(5.0, 5.0),
            color: Color::rgb(0, 0, 200),
            stroke_width: 1.0,
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_ne!(cell_char(&buf, 0, 0), ' ');
        assert_ne!(cell_char(&buf, 5, 5), ' ');
    }

    #[test]
    fn closed_path_connects_the_last_point_back_to_the_first() {
        let area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::Path {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(6.0, 0.0),
                Point::new(6.0, 6.0),
            ],
            color: Color::rgb(10, 10, 10),
            stroke_width: 1.0,
            closed: true,
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        // The closing segment runs from (6,6) back to (0,0) — a dot
        // somewhere along that diagonal (e.g. near (3, 3)) proves the
        // extra segment was actually drawn, not just the two open-path
        // legs.
        assert_ne!(cell_char(&buf, 3, 3), ' ');
    }

    #[test]
    fn text_run_paints_cell_quantised_at_the_rects_rounded_position() {
        let area = RRect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::TextRun {
            rect: Rect::new(2.3, 1.0, 10.0, 1.0),
            text: "hi".into(),
            color: Color::rgb(255, 255, 255),
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        // 2.3 rounds to cell 2.
        assert_eq!(cell_char(&buf, 2, 1), 'h');
        assert_eq!(cell_char(&buf, 3, 1), 'i');
    }

    #[test]
    fn image_op_paints_fallback_text() {
        let area = RRect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::Image {
            rect: Rect::new(0.0, 0.0, 10.0, 1.0),
            image: Image {
                id: WidgetId::new("img"),
                source: ImageSource::Path("/tmp/x.png".into()),
                intrinsic_size: Some((10, 10)),
                fit: ImageFit::Contain,
                fallback_text: "[X]".into(),
            },
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), '[');
        assert_eq!(cell_char(&buf, 1, 0), 'X');
        assert_eq!(cell_char(&buf, 2, 0), ']');
    }

    #[test]
    fn push_clip_confines_a_rect_fill_to_the_clipped_cells() {
        let area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![
            DrawOp::PushClip {
                rect: Rect::new(0.0, 0.0, 2.0, 2.0),
            },
            DrawOp::Rect {
                rect: Rect::new(0.0, 0.0, 8.0, 8.0),
                color: Color::rgb(50, 50, 50),
            },
            DrawOp::PopClip,
        ]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_ne!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(
            cell_char(&buf, 5, 5),
            ' ',
            "the rect fill reaches cell (5, 5) but the pushed 2x2-cell clip must confine it"
        );
    }

    #[test]
    fn shapes_paint_under_text_regardless_of_declared_order() {
        // Module doc's "Z-order" section: a label declared *before* its
        // background rect must still read clearly — the common pattern
        // this degrade is tuned for.
        let area = RRect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![
            DrawOp::TextRun {
                rect: Rect::new(0.0, 0.0, 5.0, 1.0),
                text: "X".into(),
                color: Color::rgb(255, 255, 255),
            },
            DrawOp::Rect {
                rect: Rect::new(0.0, 0.0, 5.0, 1.0),
                color: Color::rgb(0, 0, 0),
            },
        ]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), 'X');
    }

    #[test]
    fn hit_test_round_trips_with_the_returned_layout() {
        let area = RRect::new(3, 4, 10, 10);
        let mut buf = Buffer::empty(RRect::new(0, 0, 20, 20));
        let c = canvas(vec![]);
        let layout = draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_eq!(layout.hit_test(5.0, 5.0), CanvasHit::Inside);
        assert_eq!(layout.hit_test(0.0, 0.0), CanvasHit::Outside);
    }

    /// Non-zero-origin regression guard (issue #505/LESSONS.md).
    #[test]
    fn rect_op_paints_at_the_right_cell_with_a_nonzero_area_origin() {
        let area = RRect::new(7, 3, 10, 10);
        let mut buf = Buffer::empty(RRect::new(0, 0, 20, 20));
        let c = canvas(vec![DrawOp::Rect {
            rect: Rect::new(0.0, 0.0, 1.0, 1.0),
            color: Color::rgb(1, 2, 3),
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_ne!(cell_char(&buf, 7, 3), ' ');
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn zero_size_area_does_not_panic() {
        let area = RRect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(RRect::new(0, 0, 5, 5));
        let c = canvas(vec![DrawOp::Rect {
            rect: Rect::new(0.0, 0.0, 1.0, 1.0),
            color: Color::rgb(1, 2, 3),
        }]);
        let _ = draw_canvas(&mut buf, area, &c, &Theme::default());
    }

    #[test]
    fn empty_ops_paints_nothing() {
        let area = RRect::new(0, 0, 5, 5);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        for y in 0..5 {
            for x in 0..5 {
                assert_eq!(cell_char(&buf, x, y), ' ');
            }
        }
    }
}
