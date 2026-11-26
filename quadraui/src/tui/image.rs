//! TUI fallback rasteriser for [`crate::Image`]: [`draw_image`] paints
//! [`Image::fallback_text`] centered in the target rect through the
//! widget's own cell painter and reports
//! [`ImagePaintResult::Unsupported`]. Real pixels reach a terminal through
//! the cell surface's graphics channel instead (see
//! `crate::tui::cell_surface`'s module doc).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::backend::ImagePaintResult;
use crate::primitives::image::Image;
use crate::theme::Theme;

/// Paint `image`'s fallback text into `area`, centered horizontally on
/// the vertical middle row. Always returns
/// [`ImagePaintResult::Unsupported`] — see the module docs.
pub fn draw_image(buf: &mut Buffer, area: Rect, image: &Image, theme: &Theme) -> ImagePaintResult {
    let mut cells = super::cell_surface::BufferCellSurface::new(buf);
    let area = crate::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );
    image.paint_fallback_cells(area, &mut cells, theme);
    ImagePaintResult::Unsupported
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
