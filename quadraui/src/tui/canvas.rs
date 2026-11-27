//! TUI entry points for [`crate::Canvas`]: the cell painter and the
//! cell-quantised region hit test live with the widget
//! (`primitives::canvas`); these keep the backend's existing
//! `draw_canvas`/`canvas_layout` paths reaching them. Also home to the
//! backend's own clip stack ([`TuiClipFrame`]), which quantises through
//! the widget's [`CellClip`], and the Kitty-graphics upgrade pass the
//! backend runs over a canvas's image ops.

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect as RRect;

use crate::event::Rect;
use crate::primitives::canvas::paint_cells::CellClip;
#[cfg(feature = "tui-images")]
use crate::primitives::canvas::paint_cells::{clip_per_op, image_op_cells};
#[cfg(feature = "tui-images")]
use crate::primitives::canvas::DrawOp;
use crate::primitives::canvas::{Canvas, CanvasHit, CanvasLayout};
use crate::theme::Theme;

/// Compute the TUI cell-unit layout for a [`Canvas`] without painting.
pub fn tui_canvas_layout(canvas: &Canvas, area: RRect) -> CanvasLayout {
    canvas.layout(Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ))
}

/// Cell-quantised hit-test for [`Canvas::regions`] on TUI — see
/// [`crate::primitives::canvas::paint_cells::region_hit_test`]. `(x, y)`
/// is in absolute screen cells.
pub fn tui_canvas_region_hit_test(canvas: &Canvas, area: RRect, x: f32, y: f32) -> CanvasHit {
    crate::primitives::canvas::paint_cells::region_hit_test(
        canvas,
        &tui_canvas_layout(canvas, area),
        x,
        y,
    )
}

/// Draw a [`Canvas`]'s `ops` into `area` on `buf`. Returns the layout for
/// host click dispatch.
pub fn draw_canvas(buf: &mut Buffer, area: RRect, canvas: &Canvas, theme: &Theme) -> CanvasLayout {
    let layout = tui_canvas_layout(canvas, area);
    crate::primitives::canvas::paint_cells::paint(
        canvas,
        &layout,
        &mut super::BufferCellSurface::new(buf),
        theme,
    );
    layout
}

/// One entry in [`crate::tui::backend::TuiBackend`]'s
/// [`crate::Backend::push_clip`]/[`crate::Backend::pop_clip`] stack.
///
/// TUI has no per-cell scissor every `draw_*` call site checks, so this
/// is a retroactive undo rather than a real filter: [`Self::push`]
/// snapshots the region a push is about to put off-limits (`outer`),
/// lets painting run unclipped, and [`Self::pop`] restores whatever part
/// of `outer` still sits outside the narrowed `clip`. A `draw_*` that
/// reads the buffer between push and pop (a translucent fill blending
/// against what's "underneath", a test asserting mid-frame) sees
/// unclipped content — this degrade is only sound for paint that writes
/// once and is read back after the matching `pop`.
///
/// Expected call frequency: one push per visible viewport (a scroll
/// container clipping one child), not one per row — each push clones a
/// full `outer`-sized cell snapshot, so a per-row push pays that cost
/// once per row.
pub(crate) struct TuiClipFrame {
    /// Absolute screen-cell area active just before this push — what
    /// [`Self::pop`] restores, outside `clip`, to its pre-push contents.
    outer: RRect,
    /// This push's own narrowed clip: `rect` quantised outward to whole
    /// cells (same rounding as [`DrawOp::PushClip`], coarser than the
    /// `.round()` every `draw_*` rect otherwise gets — a fractional
    /// clip can let a child bleed up to one cell past it) and
    /// intersected with `outer`.
    clip: RRect,
    /// `outer`'s cells, snapshotted at push time, row-major
    /// (`outer.width` cells per row).
    snapshot: Vec<Cell>,
}

impl TuiClipFrame {
    /// Snapshot `outer`'s cells from `buf`, then quantise `rect`
    /// (absolute screen-cell units — the same space every
    /// `Backend::draw_*` rect already uses) outward to whole cells and
    /// intersect with `outer`, reusing [`CellClip`]'s own math rather
    /// than a second copy of it.
    pub(crate) fn push(buf: &Buffer, outer: RRect, rect: Rect) -> Self {
        let bound = CellClip::from_cells(outer.x, outer.y, outer.width, outer.height).push(rect);
        let clip = RRect::new(
            bound.x0 as u16,
            bound.y0 as u16,
            bound.x1.saturating_sub(bound.x0).max(0) as u16,
            bound.y1.saturating_sub(bound.y0).max(0) as u16,
        );
        let mut snapshot = Vec::with_capacity(outer.area() as usize);
        for y in outer.y..outer.y.saturating_add(outer.height) {
            for x in outer.x..outer.x.saturating_add(outer.width) {
                snapshot.push(buf[(x, y)].clone());
            }
        }
        Self {
            outer,
            clip,
            snapshot,
        }
    }

    /// This push's own narrowed clip — what a nested push intersects
    /// against, and what [`crate::Backend::push_clip`]'s caller sees as
    /// "the active clip" until the matching [`Self::pop`].
    pub(crate) fn active(&self) -> RRect {
        self.clip
    }

    /// Restore `buf`'s cells inside `outer` but outside `clip` to their
    /// snapshotted contents — the donut a clipped-out paint could have
    /// reached — leaving cells inside `clip` exactly as whatever painted
    /// there left them.
    pub(crate) fn pop(self, buf: &mut Buffer) {
        let clip =
            CellClip::from_cells(self.clip.x, self.clip.y, self.clip.width, self.clip.height);
        let mut i = 0;
        for y in self.outer.y..self.outer.y.saturating_add(self.outer.height) {
            for x in self.outer.x..self.outer.x.saturating_add(self.outer.width) {
                if !clip.contains_cell(x as i32, y as i32) {
                    buf[(x, y)] = self.snapshot[i].clone();
                }
                i += 1;
            }
        }
    }
}

/// Upgrade every [`DrawOp::Image`] in `canvas` to real Kitty-graphics
/// pixels, overwriting whatever fallback-text cells [`draw_canvas`]
/// already painted for it. Each op's canvas-local `rect` is rounded to
/// whole cells and clipped by the same per-op [`CellClip`] stack the
/// widget's cell painter replays, and handed to
/// [`crate::tui::cell_surface::draw_image_kitty`] — the same decode-cache,
/// id-registry, and transmit/placement-byte-queueing path the cell
/// surface's graphics channel drives for the standalone `Image`
/// primitive, so an image reached through either path shares one id if
/// its source and placed cell size match. An op the Kitty path cannot
/// place or decode gets the standalone primitive's centered fallback
/// text instead.
///
/// Called by `TuiBackend::draw_canvas` only when
/// [`crate::tui::backend::TuiBackend::kitty_graphics`] is `Some` and
/// the backend is truecolor — see that trait method's body. Behind the
/// `tui-images` Cargo feature, same as [`crate::tui::cell_surface::draw_image_kitty`]
/// itself.
#[cfg(feature = "tui-images")]
pub(crate) fn draw_canvas_kitty_images(
    buf: &mut Buffer,
    area: RRect,
    canvas: &Canvas,
    theme: &Theme,
    cell_px: (u16, u16),
    state: &mut crate::tui::cell_surface::KittyImageState,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cols = area.width as i32;
    let rows = area.height as i32;
    let clips = clip_per_op(&canvas.ops, cols, rows);
    for (op, clip) in canvas.ops.iter().zip(&clips) {
        let DrawOp::Image { rect, image } = op else {
            continue;
        };
        let Some(fallback_area) = image_op_cells((area.x, area.y), (cols, rows), clip, *rect)
        else {
            continue;
        };
        let op_area = super::backend::q_rect_to_ratatui(fallback_area);
        if !crate::tui::cell_surface::draw_image_kitty(buf, op_area, image, cell_px, state) {
            image.paint_fallback_cells(
                fallback_area,
                &mut crate::tui::BufferCellSurface::new(buf),
                theme,
            );
        }
    }
}

#[cfg(all(test, feature = "tui-images"))]
mod tests {
    use super::*;
    use crate::primitives::image::{Image, ImageFit, ImageSource};
    use crate::types::WidgetId;

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
    fn draw_canvas_kitty_images_upgrades_an_image_op_past_its_fallback_text() {
        fn tiny_png_bytes() -> Vec<u8> {
            let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 200, 255]));
            let mut out = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .expect("encode tiny test PNG");
            out
        }

        let area = RRect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let c = canvas(vec![DrawOp::Image {
            rect: Rect::new(0.0, 0.0, 4.0, 2.0),
            image: Image {
                id: WidgetId::new("img"),
                source: ImageSource::Bytes(tiny_png_bytes()),
                intrinsic_size: Some((4, 4)),
                fit: ImageFit::Fill,
                fallback_text: "[X]".into(),
            },
        }]);
        draw_canvas(&mut buf, area, &c, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), '[', "fallback text paints first");

        let mut state = crate::tui::cell_surface::KittyImageState::new();
        draw_canvas_kitty_images(&mut buf, area, &c, &Theme::default(), (8, 16), &mut state);

        assert_eq!(
            cell_char(&buf, 0, 0),
            '\u{10EEEE}',
            "a successful real-pixel decode must overwrite the fallback text with placeholder cells"
        );
        assert!(!state.pending_writes.is_empty());
    }
}
