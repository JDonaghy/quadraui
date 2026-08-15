//! GTK rasteriser for [`crate::primitives::command_line::CommandLine`].

use gtk4::cairo::Context;
use gtk4::pango;

use super::cairo_rgb;
use crate::primitives::command_line::CommandLine;
use crate::theme::Theme;

#[allow(clippy::too_many_arguments)]
pub fn draw_command_line(
    cr: &Context,
    layout: &pango::Layout,
    cmd: &CommandLine,
    theme: &Theme,
    x: f64,
    y: f64,
    width: f64,
    line_height: f64,
) {
    let bg = cairo_rgb(theme.command_line_bg);
    let fg = cairo_rgb(theme.command_line_fg);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, width, line_height);
    cr.fill().ok();

    if cmd.text.is_empty() {
        return;
    }

    layout.set_text(&cmd.text);
    layout.set_attributes(None);
    cr.set_source_rgb(fg.0, fg.1, fg.2);

    if cmd.right_align {
        let (text_w, _) = layout.pixel_size();
        cr.move_to(x + width - text_w as f64, y);
    } else {
        cr.move_to(x, y);
    }
    super::painted_text::show_layout(cr, layout);

    if let Some(offset) = cmd.cursor_offset {
        let anchor = crate::text_util::safe_prefix(&cmd.text, offset);
        layout.set_text(anchor);
        let (text_w, _) = layout.pixel_size();
        let cursor_color = cairo_rgb(theme.cursor);
        cr.set_source_rgb(cursor_color.0, cursor_color.1, cursor_color.2);
        cr.rectangle(x + text_w as f64, y, 2.0, line_height);
        cr.fill().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    /// Regression for issue #503: typing `:éditer` (or any command
    /// with a multibyte char left of the cursor) used to panic
    /// `&cmd.text[..offset.min(cmd.text.len())]` when `cursor_offset`
    /// landed mid-character.
    #[test]
    fn draw_command_line_with_multibyte_cursor_does_not_panic() {
        let surface = ImageSurface::create(Format::ARgb32, 400, 30).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);
        let theme = Theme::default();

        // ":éditer" — byte 2 sits inside the 2-byte 'é' (starts at byte 1).
        let text = ":éditer";
        assert!(!text.is_char_boundary(2));
        let cmd = CommandLine {
            id: WidgetId::new("cmdline"),
            text: text.into(),
            cursor_offset: Some(2),
            right_align: false,
        };

        // Must not panic.
        draw_command_line(&cr, &pango_layout, &cmd, &theme, 0.0, 0.0, 400.0, 20.0);
    }
}
