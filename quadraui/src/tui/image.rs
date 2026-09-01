//! TUI rasteriser for [`crate::Image`] (#662).
//!
//! TUI has no pixel grid to paint raster image content into, so there is
//! no ASCII-art decoder here — that would be a second, much larger
//! primitive (`primitives::image` module docs' scope guard says so
//! explicitly). Instead this paints [`Image::fallback_text`], centered
//! in the target rect, and always reports
//! [`ImagePaintResult::Unsupported`] — never [`ImagePaintResult::Painted`]
//! — so a host can tell "TUI painted the fallback text" apart from "GTK
//! painted real pixels" without inspecting the screen itself (#507).

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
