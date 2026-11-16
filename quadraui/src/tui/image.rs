//! TUI rasteriser for [`crate::Image`].
//!
//! [`draw_image`] — [`Image::fallback_text`] centered in the target rect,
//! reporting [`ImagePaintResult::Unsupported`] — is the universal
//! fallback: it runs whenever the terminal hasn't confirmed Kitty
//! graphics support, isn't truecolor, the `tui-images` Cargo feature is
//! off, or a real decode fails. There is no ASCII-art decoder as a
//! further fallback under that — that would be a second, much larger
//! primitive (`primitives::image` module docs' scope guard says so
//! explicitly).
//!
//! On a confirmed Kitty-graphics terminal (`tui-images` feature,
//! [`crate::tui::backend::TuiBackend::kitty_graphics`] returning `Some`,
//! truecolor), [`draw_image_kitty`] paints real pixels instead: decode +
//! downscale through [`crate::tui::image_decode`], fill the placed
//! cells with [`crate::tui::kitty_graphics`]'s Unicode placeholder
//! content, and queue that image's transmit/placement escape bytes on
//! [`KittyImageState::pending_writes`] for
//! [`crate::tui::run::flush_kitty_graphics_writes`] to write to the real
//! terminal once `terminal.draw` returns — raw APC bytes can't go
//! through `ratatui::buffer::Buffer`'s cell grid the way the placeholder
//! glyphs themselves do. Reports [`ImagePaintResult::Painted`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{draw_styled_text, qc};
use crate::backend::ImagePaintResult;
use crate::primitives::image::Image;
use crate::theme::Theme;
use crate::types::{Decoration, StyledText};

/// Paint `image`'s fallback text into `area`, centered horizontally on
/// the vertical middle row. Always returns
/// [`ImagePaintResult::Unsupported`] — see the module docs.
pub fn draw_image(buf: &mut Buffer, area: Rect, image: &Image, theme: &Theme) -> ImagePaintResult {
    if area.width > 0 && area.height > 0 && !image.fallback_text.is_empty() {
        let text = StyledText::plain(image.fallback_text.as_str());
        let visible = text.visible_width();
        let start_col = (area.width as usize).saturating_sub(visible) / 2;
        let y = area.y + area.height / 2;
        draw_styled_text(
            buf,
            area,
            y,
            start_col,
            &text,
            qc(theme.foreground),
            qc(theme.background),
            Decoration::Normal,
            qc(theme.foreground),
        );
    }
    ImagePaintResult::Unsupported
}

/// Per-session state [`draw_image_kitty`] threads across frames —
/// owned by [`crate::tui::backend::TuiBackend`], one instance per
/// backend, never per-call: the decode cache and id registry only pay
/// off when the same backend paints the same image again on a later
/// frame.
#[cfg(feature = "tui-images")]
pub(crate) struct KittyImageState {
    /// Decoded-and-downscaled pixels, keyed by source + target cell size
    /// — see [`crate::tui::image_decode`]'s module doc, "Decode cache"
    /// section.
    pub(crate) cache: crate::image_cache::ImageCache<crate::tui::image_decode::DecodedImage>,
    /// Which kitty image ids are currently placed, and for how many
    /// frames each has gone untouched — see
    /// [`crate::tui::kitty_graphics::IdRegistry`].
    pub(crate) registry: crate::tui::kitty_graphics::IdRegistry,
    /// Transmit/placement/delete escape bytes queued this frame,
    /// drained by [`crate::tui::run::flush_kitty_graphics_writes`] once
    /// `terminal.draw` returns.
    pub(crate) pending_writes: Vec<u8>,
}

#[cfg(feature = "tui-images")]
impl KittyImageState {
    pub(crate) fn new() -> Self {
        Self {
            cache: crate::image_cache::ImageCache::default(),
            registry: crate::tui::kitty_graphics::IdRegistry::new(),
            pending_writes: Vec::new(),
        }
    }
}

/// Paint `image` for real through the Kitty graphics protocol: resolve
/// [`Image::layout`] against `area` scaled into pixel space by
/// `cell_px` (terminal cells aren't square, and `Image::intrinsic_size`
/// is in pixels, so the fit math has to run in the same units), scale
/// the result back down into cell units and clip to whole cells,
/// decode and downscale to that placement's pixel size
/// (`cols * cell_px.0` × `rows * cell_px.1`), fill the placed cells
/// with Unicode placeholder content colored to carry this image's id,
/// and queue transmit + placement bytes on `state.pending_writes` for
/// an id not already live this session. Falls back to [`draw_image`]
/// (and its [`ImagePaintResult::Unsupported`]) when there is nothing to
/// place — a zero-size `area`/`cell_px`, a zero-size clipped placement,
/// or a decode failure.
#[cfg(feature = "tui-images")]
pub(crate) fn draw_image_kitty(
    buf: &mut Buffer,
    area: Rect,
    image: &Image,
    theme: &Theme,
    cell_px: (u16, u16),
    state: &mut KittyImageState,
) -> ImagePaintResult {
    if area.width == 0 || area.height == 0 || cell_px.0 == 0 || cell_px.1 == 0 {
        return draw_image(buf, area, image, theme);
    }
    // Clip against `buf`'s own extent too, same defensive posture as
    // `TuiBackend::register_opaque_rect` — `area` is normally already
    // inside `buf.area`, but a stale caller-supplied rect must never
    // index past the buffer's real size.
    let buf_area = buf.area;
    let clip_x0 = (area.x as i32).max(buf_area.x as i32);
    let clip_y0 = (area.y as i32).max(buf_area.y as i32);
    let clip_x1 =
        (area.x as i32 + area.width as i32).min(buf_area.x as i32 + buf_area.width as i32);
    let clip_y1 =
        (area.y as i32 + area.height as i32).min(buf_area.y as i32 + buf_area.height as i32);

    // Resolve `Image::layout` in pixel space, not cell space:
    // `Image::intrinsic_size` (and therefore the aspect-ratio math
    // behind `ImageFit::Contain`/`Cover`/`None`) is in pixels, and
    // terminal cells are not square — scaling `area` up by `cell_px`
    // keeps the fit math correct, then the result is scaled back down
    // into cell units before any cell-grid indexing happens.
    let bounds_px = crate::event::Rect::new(
        area.x as f32 * cell_px.0 as f32,
        area.y as f32 * cell_px.1 as f32,
        area.width as f32 * cell_px.0 as f32,
        area.height as f32 * cell_px.1 as f32,
    );
    let placed_px = image.layout(bounds_px).bounds;
    let placed = crate::event::Rect::new(
        placed_px.x / cell_px.0 as f32,
        placed_px.y / cell_px.1 as f32,
        placed_px.width / cell_px.0 as f32,
        placed_px.height / cell_px.1 as f32,
    );
    let x0 = (placed.x.round() as i32).max(clip_x0);
    let y0 = (placed.y.round() as i32).max(clip_y0);
    let x1 = ((placed.x + placed.width).round() as i32).min(clip_x1);
    let y1 = ((placed.y + placed.height).round() as i32).min(clip_y1);
    let cols = (x1 - x0).max(0) as u32;
    let rows = (y1 - y0).max(0) as u32;
    if cols == 0 || rows == 0 {
        return draw_image(buf, area, image, theme);
    }

    let target_px = (cols * cell_px.0 as u32, rows * cell_px.1 as u32);
    let decoded = match crate::tui::image_decode::decode_and_downscale_cached(
        &image.source,
        target_px,
        1.0,
        &mut state.cache,
    ) {
        Ok(decoded) => decoded,
        Err(_) => return draw_image(buf, area, image, theme),
    };

    let key = crate::tui::kitty_graphics::ImageKey::new(&image.source, cols, rows);
    let id = match state.registry.touch(key) {
        crate::tui::kitty_graphics::IdLookup::New(id) => {
            for seq in crate::tui::kitty_graphics::transmit_rgba(
                id,
                decoded.width,
                decoded.height,
                &decoded.rgba,
            ) {
                state.pending_writes.extend(seq.into_bytes());
            }
            state
                .pending_writes
                .extend(crate::tui::kitty_graphics::place_virtual(id, cols, rows).into_bytes());
            id
        }
        crate::tui::kitty_graphics::IdLookup::Live(id) => id,
    };

    let fg = qc(crate::tui::kitty_graphics::placeholder_fg_color(id));
    for row in 0..rows {
        for col in 0..cols {
            let Some(text) = crate::tui::kitty_graphics::placeholder_cell_text(row, col) else {
                // Past the diacritic table's range — see
                // `ROWCOL_DIACRITICS`'s doc. Leave the cell whatever it
                // was; a placement this large is already far past any
                // real terminal's viewport.
                continue;
            };
            let x = x0 as u16 + col as u16;
            let y = y0 as u16 + row as u16;
            let cell = &mut buf[(x, y)];
            cell.set_symbol(&text);
            cell.set_fg(fg);
        }
    }
    ImagePaintResult::Painted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::image::{ImageFit, ImageSource};
    use crate::types::WidgetId;

    fn image(fallback_text: &str) -> Image {
        Image {
            id: WidgetId::new("logo"),
            source: ImageSource::Path("/tmp/logo.png".into()),
            intrinsic_size: Some((16, 16)),
            fit: ImageFit::Contain,
            fallback_text: fallback_text.into(),
        }
    }

    fn screen_text(buf: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
        }
        out
    }

    #[test]
    fn draw_image_always_reports_unsupported() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let theme = Theme::default();
        let result = draw_image(&mut buf, Rect::new(0, 0, 20, 3), &image("[LOGO]"), &theme);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn draw_image_paints_fallback_text_into_the_rect() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        draw_image(&mut buf, area, &image("[LOGO]"), &theme);
        assert!(
            screen_text(&buf, area).contains("[LOGO]"),
            "fallback text should be painted somewhere in the rect:\n{}",
            screen_text(&buf, area)
        );
    }

    #[test]
    fn empty_fallback_text_paints_nothing() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let result = draw_image(&mut buf, area, &image(""), &theme);
        assert_eq!(result, ImagePaintResult::Unsupported);
        assert!(screen_text(&buf, area).trim().is_empty());
    }

    #[test]
    fn zero_size_rect_does_not_panic() {
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        let theme = Theme::default();
        let result = draw_image(&mut buf, area, &image("[LOGO]"), &theme);
        assert_eq!(result, ImagePaintResult::Unsupported);
    }
}

#[cfg(all(test, feature = "tui-images"))]
mod kitty_tests {
    use super::*;
    use crate::primitives::image::{ImageFit, ImageSource};
    use crate::types::WidgetId;

    /// A tiny solid-colour PNG, generated at test time — same approach as
    /// `image_decode`'s own test helper.
    fn tiny_png_bytes() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("encode tiny test PNG");
        out
    }

    fn image(source: ImageSource) -> Image {
        Image {
            id: WidgetId::new("logo"),
            source,
            intrinsic_size: Some((4, 4)),
            fit: ImageFit::Fill,
            fallback_text: "[LOGO]".into(),
        }
    }

    const CELL_PX: (u16, u16) = (8, 16);

    #[test]
    fn paints_placeholder_cells_and_reports_painted() {
        let area = Rect::new(0, 0, 4, 4);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut state = KittyImageState::new();
        let img = image(ImageSource::Bytes(tiny_png_bytes()));

        let result = draw_image_kitty(&mut buf, area, &img, &theme, CELL_PX, &mut state);

        assert_eq!(result, ImagePaintResult::Painted);
        // Every cell in the placed rect carries the placeholder base
        // character, not the fallback text.
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                assert!(
                    buf[(x, y)].symbol().starts_with('\u{10EEEE}'),
                    "cell ({x},{y}) should carry the Unicode placeholder base char, got {:?}",
                    buf[(x, y)].symbol()
                );
            }
        }
        assert!(
            !state.pending_writes.is_empty(),
            "a newly transmitted image must queue transmit + placement bytes"
        );
    }

    #[test]
    fn same_image_twice_transmits_only_once() {
        let area = Rect::new(0, 0, 4, 4);
        let theme = Theme::default();
        let mut state = KittyImageState::new();
        let img = image(ImageSource::Bytes(tiny_png_bytes()));

        let mut buf1 = Buffer::empty(area);
        draw_image_kitty(&mut buf1, area, &img, &theme, CELL_PX, &mut state);
        let first_len = state.pending_writes.len();
        state.registry.begin_frame();

        let mut buf2 = Buffer::empty(area);
        let result = draw_image_kitty(&mut buf2, area, &img, &theme, CELL_PX, &mut state);

        assert_eq!(result, ImagePaintResult::Painted);
        assert_eq!(
            state.pending_writes.len(),
            first_len,
            "an id already live must not be re-transmitted"
        );
    }

    #[test]
    fn decode_failure_falls_back_to_fallback_text() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut state = KittyImageState::new();
        let img = image(ImageSource::Bytes(vec![0, 1, 2, 3]));

        let result = draw_image_kitty(&mut buf, area, &img, &theme, CELL_PX, &mut state);

        assert_eq!(result, ImagePaintResult::Unsupported);
        assert!(state.pending_writes.is_empty());
        let mut text = String::new();
        for x in area.left()..area.right() {
            text.push_str(buf[(x, area.y + area.height / 2)].symbol());
        }
        assert!(text.contains("[LOGO]"));
    }

    #[test]
    fn zero_size_placement_falls_back_without_panicking() {
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        let theme = Theme::default();
        let mut state = KittyImageState::new();
        let img = image(ImageSource::Bytes(tiny_png_bytes()));

        let result = draw_image_kitty(&mut buf, area, &img, &theme, CELL_PX, &mut state);

        assert_eq!(result, ImagePaintResult::Unsupported);
    }

    #[test]
    fn contain_resolves_in_pixel_space_against_non_square_cells() {
        // 200x100 (2:1) source into a 20x10-cell rect on 8x16 cells: the
        // bounds in pixels is 160x160 (square), so `Contain`'s
        // aspect-preserving fit picks the width-limited scale
        // (160/200 = 0.8) and letterboxes to 20 cols x 5 rows, not the
        // whole 20x10 area a cell-unit (unconverted) layout would give.
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut state = KittyImageState::new();
        let img = Image {
            id: WidgetId::new("logo"),
            source: ImageSource::Bytes(tiny_png_bytes()),
            intrinsic_size: Some((200, 100)),
            fit: ImageFit::Contain,
            fallback_text: "[LOGO]".into(),
        };

        let result = draw_image_kitty(&mut buf, area, &img, &theme, CELL_PX, &mut state);

        assert_eq!(result, ImagePaintResult::Painted);
        let mut painted_rows = std::collections::BTreeSet::new();
        let mut painted_cols = std::collections::BTreeSet::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if buf[(x, y)].symbol().starts_with('\u{10EEEE}') {
                    painted_rows.insert(y);
                    painted_cols.insert(x);
                }
            }
        }
        assert_eq!(
            painted_rows.len(),
            5,
            "letterboxed placement should be 5 cell rows tall, got rows {painted_rows:?}"
        );
        assert_eq!(
            painted_cols.len(),
            20,
            "placement should span the full 20-cell width, got cols {painted_cols:?}"
        );
    }

    #[test]
    fn none_resolves_intrinsic_pixel_size_into_cells_not_whole_area() {
        // Intrinsic 16x32 px on 8x16 cells is exactly 2 cols x 2 rows —
        // `ImageFit::None` must place that small footprint anchored at
        // the top-left, not the literal pixel count's worth of cells
        // clipped down to the whole area, which is what resolving the
        // layout against cell units (instead of pixels) would give.
        let area = Rect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut state = KittyImageState::new();
        let img = Image {
            id: WidgetId::new("logo"),
            source: ImageSource::Bytes(tiny_png_bytes()),
            intrinsic_size: Some((16, 32)),
            fit: ImageFit::None,
            fallback_text: "[LOGO]".into(),
        };

        let result = draw_image_kitty(&mut buf, area, &img, &theme, CELL_PX, &mut state);

        assert_eq!(result, ImagePaintResult::Painted);
        let mut painted = 0;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if buf[(x, y)].symbol().starts_with('\u{10EEEE}') {
                    painted += 1;
                    assert!(
                        x < 2 && y < 2,
                        "None should anchor within a 2x2-cell footprint, painted ({x},{y})"
                    );
                }
            }
        }
        assert_eq!(
            painted, 4,
            "None should place exactly intrinsic_size/cell_px = 2x2 cells, got {painted}"
        );
    }
}
