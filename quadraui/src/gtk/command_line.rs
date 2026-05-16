//! GTK rasteriser for [`crate::primitives::command_line::CommandLine`].

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

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
    pcfn::show_layout(cr, layout);

    if let Some(offset) = cmd.cursor_offset {
        let anchor = &cmd.text[..offset.min(cmd.text.len())];
        layout.set_text(anchor);
        let (text_w, _) = layout.pixel_size();
        let cursor_color = cairo_rgb(theme.cursor);
        cr.set_source_rgb(cursor_color.0, cursor_color.1, cursor_color.2);
        cr.rectangle(x + text_w as f64, y, 2.0, line_height);
        cr.fill().ok();
    }
}
