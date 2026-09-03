//! GTK rasteriser for [`crate::primitives::command_line::CommandLine`].

use gtk4::cairo::Context;
use gtk4::pango;

use super::cairo_rgb;
use crate::primitives::command_line::{CommandLine, CommandLineLayout, CommandLineMeasure};
use crate::theme::Theme;

/// Compute [`CommandLineLayout`] for `cmd` painted at
/// `(x, y, width, line_height)`, using the backend's monospace
/// `char_width` (issue #705).
pub fn gtk_command_line_layout(
    cmd: &CommandLine,
    x: f64,
    y: f64,
    width: f64,
    line_height: f64,
    char_width: f32,
) -> CommandLineLayout {
    let rect = crate::event::Rect::new(x as f32, y as f32, width as f32, line_height as f32);
    cmd.layout(rect, CommandLineMeasure::new(char_width))
}

/// Paint `cmd` into `cr` at `(x, y, width, line_height)` and return the
/// [`CommandLineLayout`] used to place its glyphs — the same value
/// `gtk_command_line_layout` would compute for the same arguments, handed
/// back so callers (and tests) never have to re-derive it and risk it
/// drifting from what was actually painted (issue #705 review: the
/// paint/click round-trip test below reads this back instead of
/// asserting a formula in isolation).
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
    char_width: f32,
) -> CommandLineLayout {
    let cmd_layout = gtk_command_line_layout(cmd, x, y, width, line_height, char_width);
    let bg = cairo_rgb(theme.command_line_bg);
    let fg = cairo_rgb(theme.command_line_fg);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, width, line_height);
    cr.fill().ok();

    if cmd.text.is_empty() {
        return cmd_layout;
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

    cmd_layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Color, WidgetId};
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
        draw_command_line(&cr, &pango_layout, &cmd, &theme, 0.0, 0.0, 400.0, 20.0, 8.0);
    }

    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        // Cairo ARGB32 byte order on little-endian is BGRA.
        (data[off + 2], data[off + 1], data[off])
    }

    fn is_painted(data: &[u8], stride: usize, x: i32, y: i32) -> bool {
        let (r, g, b) = pixel(data, stride, x, y);
        !(r == 255 && g == 255 && b == 255)
    }

    /// Find any painted pixel within (x_range, y_range), row-major.
    fn first_painted_in(
        data: &[u8],
        stride: usize,
        w: i32,
        h: i32,
        x_range: (i32, i32),
        y_range: (i32, i32),
    ) -> Option<(i32, i32)> {
        for y in y_range.0..y_range.1 {
            for x in x_range.0..x_range.1 {
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                if is_painted(data, stride, x, y) {
                    return Some((x, y));
                }
            }
        }
        None
    }

    /// Paint/click round-trip (`docs/TESTING.md` coverage-taxonomy row 1):
    /// paint via the real `draw_command_line` rasteriser into a headless
    /// Cairo surface, find the actual painted (non-background) pixel for
    /// each of two characters, then `hit_test` those exact pixels and
    /// assert they resolve to the correct byte offsets. Unlike a test
    /// that calls `gtk_command_line_layout` in isolation and asserts
    /// formula-predicted x-positions, this catches `draw_command_line`'s
    /// real Pango glyph placement drifting away from
    /// `CommandLine::layout`'s fixed-advance column formula (e.g. a
    /// future prompt gutter added to one but not the other) — see #705
    /// review.
    ///
    /// A monospace font is forced so the fixed-advance grid
    /// `CommandLine::layout` assumes actually matches what Pango paints;
    /// `char_width` is measured from that same font rather than assumed,
    /// so the test doesn't encode a magic constant that only happens to
    /// work.
    ///
    /// #505: a LOCAL/ABSOLUTE mixup is invisible at `x == 0`, so this is
    /// exercised at a nonzero origin too.
    #[test]
    fn gtk_command_line_paint_and_click_round_trip_at_nonzero_origin() {
        const W: i32 = 400;
        const H: i32 = 40;
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        let font = pango::FontDescription::from_string("Monospace 12");
        let theme = Theme {
            command_line_bg: Color::rgb(255, 255, 255),
            command_line_fg: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let cmd = CommandLine {
            id: WidgetId::new("cmdline"),
            text: ":wq".into(),
            cursor_offset: None,
            right_align: false,
        };
        let (x, y, width, line_height) = (40.0, 12.0, 200.0, 20.0_f64);

        // All painting happens in this block, so `cr` (and the
        // `pango::Layout`s it owns a reference through) is dropped
        // before `surface.data()` needs exclusive access below.
        let (char_width, layout) = {
            let cr = Context::new(&surface).expect("Context::new");
            // White background so any non-white pixel uniquely identifies
            // painted ink (mirrors `gtk::tree`'s round-trip harness).
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();

            // Measure the font's real per-character advance with a
            // throwaway layout, distinct from the one `draw_command_line`
            // paints with — so a bug that corrupts the paint layout's
            // state can't accidentally make the "expected" and "actual"
            // values agree for the wrong reason (mirrors `gtk::editor`'s
            // truth/probe-layout split).
            let measure_layout = pangocairo::functions::create_layout(&cr);
            measure_layout.set_font_description(Some(&font));
            measure_layout.set_text(":wq");
            let char_width = measure_layout.index_to_pos(0).width() as f32 / pango::SCALE as f32;
            assert!(
                char_width > 1.0,
                "monospace char_width should be several px"
            );

            let pango_layout = pangocairo::functions::create_layout(&cr);
            pango_layout.set_font_description(Some(&font));
            let layout = draw_command_line(
                &cr,
                &pango_layout,
                &cmd,
                &theme,
                x,
                y,
                width,
                line_height,
                char_width,
            );
            (char_width, layout)
        };

        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        // Column 0 (':') interior — inset 1px from each edge to dodge
        // antialiasing at the cell boundary (see `gtk::multi_section_view`'s
        // header round-trip for the same AA rationale).
        let col_y_range = (y as i32 + 1, (y + line_height) as i32 - 1);
        let col0_x_range = (x as i32 + 1, (x + char_width as f64).floor() as i32);
        let (px0, _) = first_painted_in(&data, stride, W, H, col0_x_range, col_y_range)
            .unwrap_or_else(|| panic!("column 0 (':') painted no pixel in {col0_x_range:?}"));
        assert_eq!(layout.hit_test(px0 as f32), 0);

        // Column 1 ('w').
        let col1_x_range = (
            (x + char_width as f64).ceil() as i32 + 1,
            (x + 2.0 * char_width as f64).floor() as i32,
        );
        let (px1, _) = first_painted_in(&data, stride, W, H, col1_x_range, col_y_range)
            .unwrap_or_else(|| panic!("column 1 ('w') painted no pixel in {col1_x_range:?}"));
        assert_eq!(layout.hit_test(px1 as f32), 1);

        // A click left of the bar clamps to the first column's byte offset.
        assert_eq!(layout.hit_test(0.0), 0);
        assert_eq!(layout.hit_test(x as f32), 0);
    }
}
