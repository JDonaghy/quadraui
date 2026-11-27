//! The cell painter for [`Canvas`]: the braille degrade of every
//! [`DrawOp`] described in the canvas module doc (design decision D-014).
//!
//! # Z-order: shapes under text, not strict declared order
//!
//! Every pixel surface paints `ops` in exactly the order the app declared
//! them. A cell grid cannot do that cheaply: packing one cell's braille
//! glyph needs that cell's *final* dot state, which isn't known until
//! every shape op has been walked, so shape ops are accumulated into one
//! dot grid first and rendered in a single pass; text and image ops paint
//! afterwards, in their own declared order, on top of that pass. A
//! background fill declared *before* a label renders correctly (the
//! common pattern); a shape declared *after* a label, intended to cover
//! it, does not cover it here the way it would on a pixel surface. D-014's
//! bar is "same information", not "identical z-order".
//!
//! Off-canvas line/path endpoints clamp to the dot grid's edge. A
//! line/path segment merges through a full-size scratch grid per segment
//! (`interpolate_dots` into a throwaway grid, then copied cell-by-cell
//! through the shared clip/colour bookkeeping) — simple and correct, not
//! tuned for a canvas with hundreds of segments.

use super::{
    arc_polyline, bezier_polyline, dash_segments, ellipse_polygon, polygon_fill_spans, Canvas,
    CanvasHit, CanvasLayout, DrawOp,
};
use crate::cell_surface::{solid_style, CellSurface};
use crate::event::{Point, Rect};
use crate::theme::Theme;
use crate::tui::braille::{interpolate_dots, pack_braille_cell};
use crate::types::Color;

/// Scanline band height (in canvas-local, cell-fractional units) used to
/// lower [`DrawOp::Polygon`]/[`DrawOp::Ellipse`] onto the dot grid — one
/// dot row (a quarter of a cell's height), the finest granularity this
/// backend can actually light up (module doc's degrade table).
const TUI_POLYGON_ROW_HEIGHT: f32 = 0.25;
/// Sample counts for [`DrawOp::Ellipse`]/[`DrawOp::Arc`]/[`DrawOp::Bezier`]
/// on the dot grid — coarser than the pixel backends'
/// (`native_surface_paint::{ELLIPSE,ARC,BEZIER}_SEGMENTS`) because a dot
/// is a much larger unit than a pixel; same degrade bar either way (D-014).
const TUI_ELLIPSE_SEGMENTS: usize = 32;
const TUI_ARC_SEGMENTS: usize = 24;
const TUI_BEZIER_SEGMENTS: usize = 12;

/// Axis-aligned clip in whole cells, local to a `Canvas` (`0..cols`,
/// `0..rows`) or to an absolute screen-cell rect ([`Self::from_rrect`]) —
/// the cell-quantised degrade for [`DrawOp::PushClip`]/[`DrawOp::PopClip`]
/// (module doc), reused by the TUI backend's own clip stack.
#[derive(Clone, Copy)]
pub(crate) struct CellClip {
    pub(crate) x0: i32,
    pub(crate) y0: i32,
    pub(crate) x1: i32,
    pub(crate) y1: i32,
}

impl CellClip {
    pub(crate) fn full(cols: i32, rows: i32) -> Self {
        Self {
            x0: 0,
            y0: 0,
            x1: cols,
            y1: rows,
        }
    }

    /// Bound by a whole-cell `rect`, no quantisation.
    pub(crate) fn from_cells(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x0: x as i32,
            y0: y as i32,
            x1: x as i32 + width as i32,
            y1: y as i32 + height as i32,
        }
    }

    /// Quantise `rect` (same units as `self`'s own bounds) outward to
    /// whole cells and intersect with `self`.
    pub(crate) fn push(&self, rect: Rect) -> Self {
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

    pub(crate) fn contains_cell(&self, cx: i32, cy: i32) -> bool {
        cx >= self.x0 && cx < self.x1 && cy >= self.y0 && cy < self.y1
    }
}

/// The clip active while op `i` executes, for every op in `ops` — a
/// single linear replay of the push/pop stack, shared by the shape pass
/// and the text/image pass so neither re-derives it differently.
pub(crate) fn clip_per_op(ops: &[DrawOp], cols: i32, rows: i32) -> Vec<CellClip> {
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

/// Paint `text` at `rect`'s cell-quantised top-left corner (canvas-local
/// cells, offset by `origin`), clipped to `clip` and to the canvas's own
/// `cols`×`rows`.
#[allow(clippy::too_many_arguments)]
fn paint_cell_quantised_text(
    cells: &mut dyn CellSurface,
    origin: (u16, u16),
    size: (i32, i32),
    clip: &CellClip,
    rect: Rect,
    text: &str,
    fg: Color,
    bg: Color,
) {
    if text.is_empty() {
        return;
    }
    let (cols, rows) = size;
    let cell_x = rect.x.round() as i32;
    let cell_y = rect.y.round() as i32;
    if cell_y < clip.y0 || cell_y >= clip.y1 || cell_y < 0 || cell_y >= rows {
        return;
    }
    let start_x = cell_x.max(clip.x0).max(0);
    let end_x = cols.min(clip.x1);
    if start_x >= end_x {
        return;
    }
    cells.put_str_clipped(
        origin.0 + start_x as u16,
        origin.1 + cell_y as u16,
        text,
        (end_x - start_x) as u16,
        solid_style(fg, bg),
    );
}

/// Paint `canvas`'s `ops` into `layout.bounds` on `cells` — the braille
/// shape pass, then text and image ops on top in declared order. An
/// image op paints real pixels when the surface has a graphics channel
/// ([`CellSurface::emit_graphics`]) and its fallback text otherwise.
pub(crate) fn paint(
    canvas: &Canvas,
    layout: &CanvasLayout,
    cells: &mut dyn CellSurface,
    theme: &Theme,
) {
    let b = layout.bounds;
    let (ax, ay) = (b.x.max(0.0) as u16, b.y.max(0.0) as u16);
    let (aw, ah) = (b.width.max(0.0) as u16, b.height.max(0.0) as u16);
    if aw == 0 || ah == 0 {
        return;
    }

    let cols = aw as i32;
    let rows = ah as i32;
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
            DrawOp::Polygon { points, color } => {
                for span in polygon_fill_spans(points, TUI_POLYGON_ROW_HEIGHT) {
                    accumulate_rect(&mut dots, &mut cell_color, clip, span, *color, dot_w, dot_h);
                }
            }
            DrawOp::Ellipse {
                center,
                radius_x,
                radius_y,
                color,
            } => {
                let polygon = ellipse_polygon(*center, *radius_x, *radius_y, TUI_ELLIPSE_SEGMENTS);
                for span in polygon_fill_spans(&polygon, TUI_POLYGON_ROW_HEIGHT) {
                    accumulate_rect(&mut dots, &mut cell_color, clip, span, *color, dot_w, dot_h);
                }
            }
            DrawOp::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                color,
                ..
            } => {
                let polyline =
                    arc_polyline(*center, *radius, *start_angle, *end_angle, TUI_ARC_SEGMENTS);
                for pair in polyline.windows(2) {
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
            }
            DrawOp::Bezier {
                from,
                control1,
                control2,
                to,
                color,
                ..
            } => {
                let polyline =
                    bezier_polyline(*from, *control1, *control2, *to, TUI_BEZIER_SEGMENTS);
                for pair in polyline.windows(2) {
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
            }
            DrawOp::DashedLine {
                from,
                to,
                color,
                dash_len,
                gap_len,
                ..
            } => {
                for (seg_from, seg_to) in dash_segments(*from, *to, *dash_len, *gap_len) {
                    accumulate_segment(
                        &mut dots,
                        &mut cell_color,
                        clip,
                        seg_from,
                        seg_to,
                        *color,
                        dot_w,
                        dot_h,
                    );
                }
            }
            DrawOp::StrokeRect { rect, color, .. } => {
                let tl = Point::new(rect.x, rect.y);
                let tr = Point::new(rect.x + rect.width, rect.y);
                let br = Point::new(rect.x + rect.width, rect.y + rect.height);
                let bl = Point::new(rect.x, rect.y + rect.height);
                for (a, b) in [(tl, tr), (tr, br), (br, bl), (bl, tl)] {
                    accumulate_segment(
                        &mut dots,
                        &mut cell_color,
                        clip,
                        a,
                        b,
                        *color,
                        dot_w,
                        dot_h,
                    );
                }
            }
            DrawOp::TextRun { .. }
            | DrawOp::Image { .. }
            | DrawOp::PushClip { .. }
            | DrawOp::PopClip
            | DrawOp::StyledTextRun { .. } => {}
        }
    }

    let bg = theme.background;
    for cell_row in 0..rows as usize {
        for cell_col in 0..cols as usize {
            let ch = pack_braille_cell(|dr, dc| dots[cell_row * 4 + dr][cell_col * 2 + dc]);
            if ch != '\u{2800}' {
                if let Some(color) = cell_color[cell_row][cell_col] {
                    let x = ax + cell_col as u16;
                    let y = ay + cell_row as u16;
                    cells.put_char(x, y, ch, solid_style(color, bg));
                }
            }
        }
    }

    // ── Pass 2: text/image ops paint on top, in declared order.
    for (op, clip) in canvas.ops.iter().zip(&clips) {
        match op {
            DrawOp::TextRun { rect, text, color } => {
                paint_cell_quantised_text(
                    cells,
                    (ax, ay),
                    (cols, rows),
                    clip,
                    *rect,
                    text,
                    *color,
                    theme.background,
                );
            }
            DrawOp::Image { rect, image } => {
                if let Some(op_area) = image_op_cells((ax, ay), (cols, rows), clip, *rect) {
                    if cells.emit_graphics(op_area, image) {
                        continue;
                    }
                }
                paint_cell_quantised_text(
                    cells,
                    (ax, ay),
                    (cols, rows),
                    clip,
                    *rect,
                    &image.fallback_text,
                    theme.foreground,
                    theme.background,
                );
            }
            DrawOp::StyledTextRun {
                rect, text, color, ..
            } => {
                // `role`/`italic`/`size` drop entirely — a terminal cell
                // grid has exactly one font (module doc's degrade table).
                paint_cell_quantised_text(
                    cells,
                    (ax, ay),
                    (cols, rows),
                    clip,
                    *rect,
                    text,
                    *color,
                    theme.background,
                );
            }
            _ => {}
        }
    }
}

/// The absolute whole cells an image op covers: its canvas-local `rect`
/// rounded to whole cells, clipped by `clip` and the canvas's own
/// `cols`×`rows`, offset by `origin`. `None` when nothing is left.
pub(crate) fn image_op_cells(
    origin: (u16, u16),
    size: (i32, i32),
    clip: &CellClip,
    rect: Rect,
) -> Option<Rect> {
    let (cols, rows) = size;
    let cell_x = rect.x.round() as i32;
    let cell_y = rect.y.round() as i32;
    let cell_w = rect.width.round().max(0.0) as i32;
    let cell_h = rect.height.round().max(0.0) as i32;
    let x0 = cell_x.max(clip.x0).max(0);
    let y0 = cell_y.max(clip.y0).max(0);
    let x1 = (cell_x + cell_w).min(clip.x1).min(cols);
    let y1 = (cell_y + cell_h).min(clip.y1).min(rows);
    (x0 < x1 && y0 < y1).then(|| {
        Rect::new(
            (origin.0 as i32 + x0) as f32,
            (origin.1 as i32 + y0) as f32,
            (x1 - x0) as f32,
            (y1 - y0) as f32,
        )
    })
}

/// Cell-quantised [`CanvasLayout::region_hit_test`]: each region's
/// canvas-local `rect` rounds outward to whole cells through the same
/// [`CellClip::push`] math [`DrawOp::PushClip`] rounds through, so a
/// region's box covers every cell its fractional rect touches. `(x, y)`
/// is absolute, in cells; regions are tested last-declared-first.
pub(crate) fn region_hit_test(canvas: &Canvas, layout: &CanvasLayout, x: f32, y: f32) -> CanvasHit {
    if layout.hit_test(x, y) == CanvasHit::Outside {
        return CanvasHit::Outside;
    }
    let b = layout.bounds;
    let outer = CellClip::from_cells(
        b.x.max(0.0) as u16,
        b.y.max(0.0) as u16,
        b.width.max(0.0) as u16,
        b.height.max(0.0) as u16,
    );
    let cell_x = x.floor() as i32;
    let cell_y = y.floor() as i32;
    for region in canvas.regions.iter().rev() {
        let abs_rect = Rect::new(
            b.x + region.rect.x,
            b.y + region.rect.y,
            region.rect.width,
            region.rect.height,
        );
        if outer.push(abs_rect).contains_cell(cell_x, cell_y) {
            return CanvasHit::Region { id: region.id };
        }
    }
    CanvasHit::Inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::canvas::CanvasRegion;
    use crate::primitives::image::{Image, ImageFit, ImageSource};
    use crate::tui::BufferCellSurface;
    use crate::types::WidgetId;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as RRect;

    fn layout_over(c: &Canvas, area: RRect) -> CanvasLayout {
        c.layout(Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ))
    }

    /// Lay `c` out over `area` and paint it into `buf`, the way the TUI
    /// backend does.
    fn draw_canvas(buf: &mut Buffer, area: RRect, c: &Canvas, theme: &Theme) -> CanvasLayout {
        let layout = layout_over(c, area);
        paint(c, &layout, &mut BufferCellSurface::new(buf), theme);
        layout
    }

    fn tui_canvas_region_hit_test(c: &Canvas, area: RRect, x: f32, y: f32) -> CanvasHit {
        region_hit_test(c, &layout_over(c, area), x, y)
    }

    fn canvas(ops: Vec<DrawOp>) -> Canvas {
        Canvas {
            id: WidgetId::new("canvas"),
            ops,
            regions: Vec::new(),
            focusable: false,
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

    fn region(id: &str, rect: Rect) -> CanvasRegion {
        CanvasRegion {
            id: WidgetId::new(id),
            rect,
            cursor: None,
        }
    }

    #[test]
    fn tui_canvas_region_hit_test_resolves_a_click_inside_a_region_to_its_id() {
        let area = RRect::new(0, 0, 10, 10);
        let mut c = canvas(vec![]);
        c.regions = vec![region("btn", Rect::new(1.0, 1.0, 3.0, 2.0))];
        assert_eq!(
            tui_canvas_region_hit_test(&c, area, 2.0, 1.5),
            CanvasHit::Region {
                id: WidgetId::new("btn")
            }
        );
    }

    #[test]
    fn tui_canvas_region_hit_test_quantises_a_fractional_region_outward_to_whole_cells() {
        let area = RRect::new(0, 0, 10, 10);
        let mut c = canvas(vec![]);
        // LOCAL rect spans x in [1.4, 3.2) -- the same floor/ceil-outward
        // rounding `CellClip::push` uses for `PushClip` widens this to
        // cells 1, 2, 3, so a click at the very start of cell 1 and the
        // very end of cell 3 both resolve to the region, not just a
        // fractional interior a terminal grid can't address.
        c.regions = vec![region("btn", Rect::new(1.4, 1.0, 1.8, 1.0))];
        assert_eq!(
            tui_canvas_region_hit_test(&c, area, 1.0, 1.5),
            CanvasHit::Region {
                id: WidgetId::new("btn")
            }
        );
        assert_eq!(
            tui_canvas_region_hit_test(&c, area, 3.9, 1.5),
            CanvasHit::Region {
                id: WidgetId::new("btn")
            }
        );
    }

    #[test]
    fn tui_canvas_region_hit_test_falls_back_to_plain_inside_outside_the_declared_regions() {
        let area = RRect::new(0, 0, 10, 10);
        let mut c = canvas(vec![]);
        c.regions = vec![region("btn", Rect::new(1.0, 1.0, 2.0, 2.0))];
        assert_eq!(
            tui_canvas_region_hit_test(&c, area, 8.0, 8.0),
            CanvasHit::Inside
        );
        assert_eq!(
            tui_canvas_region_hit_test(&c, area, 50.0, 50.0),
            CanvasHit::Outside
        );
    }

    #[test]
    fn tui_canvas_region_hit_test_respects_a_nonzero_area_origin() {
        let area = RRect::new(5, 6, 10, 10);
        let mut c = canvas(vec![]);
        c.regions = vec![region("btn", Rect::new(1.0, 1.0, 2.0, 2.0))];
        assert_eq!(
            tui_canvas_region_hit_test(&c, area, 6.0, 7.0),
            CanvasHit::Region {
                id: WidgetId::new("btn")
            }
        );
        assert_eq!(
            tui_canvas_region_hit_test(&c, area, 0.0, 0.0),
            CanvasHit::Outside
        );
    }

    #[test]
    fn tui_canvas_region_hit_test_prefers_the_last_declared_region_on_overlap() {
        let area = RRect::new(0, 0, 10, 10);
        let mut c = canvas(vec![]);
        c.regions = vec![
            region("under", Rect::new(0.0, 0.0, 10.0, 10.0)),
            region("over", Rect::new(2.0, 2.0, 4.0, 4.0)),
        ];
        assert_eq!(
            tui_canvas_region_hit_test(&c, area, 3.0, 3.0),
            CanvasHit::Region {
                id: WidgetId::new("over")
            }
        );
        assert_eq!(
            tui_canvas_region_hit_test(&c, area, 8.0, 8.0),
            CanvasHit::Region {
                id: WidgetId::new("under")
            }
        );
    }

    /// Non-zero-origin regression guard (see `LESSONS.md`).
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

    #[test]
    fn polygon_op_degrades_to_braille_fill() {
        let area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::Polygon {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(6.0, 0.0),
                Point::new(6.0, 6.0),
                Point::new(0.0, 6.0),
            ],
            color: Color::rgb(10, 20, 30),
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_ne!(cell_char(&buf, 2, 2), ' ');
        assert_eq!(cell_char(&buf, 8, 8), ' ');
    }

    #[test]
    fn ellipse_op_degrades_to_an_n_gon_braille_fill() {
        let area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::Ellipse {
            center: Point::new(5.0, 5.0),
            radius_x: 3.0,
            radius_y: 3.0,
            color: Color::rgb(1, 2, 3),
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_ne!(cell_char(&buf, 5, 5), ' ');
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn arc_op_degrades_to_a_sampled_braille_polyline() {
        let area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::Arc {
            center: Point::new(5.0, 5.0),
            radius: 4.0,
            start_angle: 0.0,
            end_angle: std::f32::consts::PI,
            color: Color::rgb(1, 2, 3),
            stroke_width: 1.0,
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_ne!(cell_char(&buf, 9, 5), ' ');
        assert_ne!(cell_char(&buf, 1, 5), ' ');
    }

    #[test]
    fn bezier_op_degrades_to_a_sampled_braille_polyline() {
        let area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::Bezier {
            from: Point::new(0.0, 0.0),
            control1: Point::new(0.0, 8.0),
            control2: Point::new(8.0, 8.0),
            to: Point::new(8.0, 0.0),
            color: Color::rgb(1, 2, 3),
            stroke_width: 1.0,
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_ne!(cell_char(&buf, 0, 0), ' ');
        assert_ne!(cell_char(&buf, 8, 0), ' ');
    }

    #[test]
    fn dashed_line_op_leaves_gaps_unlit() {
        let area = RRect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::DashedLine {
            from: Point::new(0.0, 0.0),
            to: Point::new(10.0, 0.0),
            color: Color::rgb(1, 2, 3),
            stroke_width: 1.0,
            dash_len: 2.0,
            gap_len: 2.0,
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        // dash_len == gap_len == 2.0 over a 10-unit line: dash 0-2, gap
        // 2-4, dash 4-6, gap 6-8, dash 8-10 — cell 0 falls in the first
        // dash, cell 3 falls in the first gap.
        assert_ne!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(
            cell_char(&buf, 3, 0),
            ' ',
            "a gap between dashes must stay unlit, proving the dash pattern survived the degrade"
        );
    }

    #[test]
    fn stroke_rect_op_degrades_to_four_perimeter_segments() {
        let area = RRect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::StrokeRect {
            rect: Rect::new(1.0, 1.0, 6.0, 6.0),
            color: Color::rgb(1, 2, 3),
            stroke_width: 2.0,
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_ne!(cell_char(&buf, 1, 1), ' ');
        assert_ne!(cell_char(&buf, 7, 1), ' ');
        assert_ne!(cell_char(&buf, 1, 7), ' ');
        assert_ne!(cell_char(&buf, 7, 7), ' ');
        assert_eq!(
            cell_char(&buf, 4, 4),
            ' ',
            "the rect's interior must stay unlit — only the perimeter strokes"
        );
    }

    #[test]
    fn styled_text_run_op_degrades_identically_to_text_run() {
        let area = RRect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::StyledTextRun {
            rect: Rect::new(2.3, 1.0, 10.0, 1.0),
            text: "hi".into(),
            color: Color::rgb(255, 255, 255),
            role: crate::primitives::canvas::CanvasFontRole::Editor,
            italic: true,
            size: Some(24.0),
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        // `role`/`italic`/`size` are dropped — identical to the plain
        // `TextRun` degrade above.
        assert_eq!(cell_char(&buf, 2, 1), 'h');
        assert_eq!(cell_char(&buf, 3, 1), 'i');
    }
}
