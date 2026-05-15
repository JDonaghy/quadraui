//! macOS rasteriser for [`crate::FindReplacePanel`].
//!
//! Mirrors [`crate::gtk::find_replace::draw_find_replace`]: panel
//! positioned at the top-right of `panel.group_bounds`, walking
//! `panel.hit_regions` for paint. Each region paints a sub-zone:
//! chevron, input fields with cursor, toggle buttons, navigation
//! glyphs, dismiss `×`.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Selection highlight in input fields** — GTK paints a Cairo
//!   rect under selected characters. Deferred with the unified text-
//!   attribute pass.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::find_replace::{FindReplaceClickTarget, FindReplacePanel};
use crate::theme::Theme;
use crate::types::Color;

/// Draw a [`FindReplacePanel`] at its anchored position.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_find_replace(
    ctx: CGContextRef,
    font: &CTFont,
    panel: &FindReplacePanel,
    theme: &Theme,
    line_height: f64,
    char_width: f64,
) {
    use FindReplaceClickTarget as T;

    let cw = char_width.max(1.0);
    let lh = line_height.max(1.0);

    let panel_w_cells: f64 = panel.panel_width as f64;
    let popup_w = panel_w_cells * cw;
    let row_count = if panel.show_replace { 2.0 } else { 1.0 };
    let popup_h = (row_count + 2.0) * lh;

    let gb = &panel.group_bounds;
    let popup_x = ((gb.x + gb.width) as f64 - popup_w - 10.0).max(gb.x as f64);
    let popup_y = gb.y as f64 + 2.0;

    fill_rect(ctx, popup_x, popup_y, popup_w, popup_h, theme.surface_bg);
    stroke_rect(
        ctx,
        popup_x,
        popup_y,
        popup_w,
        popup_h,
        theme.separator,
        1.0,
    );

    // Content origin — 1 cell inside the borders.
    let content_x = popup_x + cw;
    let content_y = popup_y + lh;

    let paint_label = |text: &str, px: f64, py: f64, fg: Color| {
        draw_text(ctx, font, text, px, py, color_to_cg(fg));
    };

    let paint_toggle = |col: u16, row: u16, width: u16, label: &str, active: bool| {
        let bx = content_x + col as f64 * cw;
        let by = content_y + row as f64 * lh;
        let bw = width as f64 * cw;
        if active {
            fill_rect(ctx, bx, by, bw, lh, theme.accent_bg);
            let (tw, _) = measure_text(font, label);
            draw_text(
                ctx,
                font,
                label,
                bx + (bw - tw) / 2.0,
                by,
                color_to_cg(theme.background),
            );
        } else {
            stroke_rect(ctx, bx, by, bw, lh, theme.separator, 0.5);
            let (tw, _) = measure_text(font, label);
            draw_text(
                ctx,
                font,
                label,
                bx + (bw - tw) / 2.0,
                by,
                color_to_cg(theme.foreground),
            );
        }
    };

    let paint_glyph = |col: u16, row: u16, width: u16, label: &str, active: bool| {
        let bx = content_x + col as f64 * cw;
        let by = content_y + row as f64 * lh;
        let bw = width as f64 * cw;
        let fg = if active {
            fill_rect(ctx, bx, by, bw, lh, theme.accent_bg);
            theme.background
        } else {
            theme.foreground
        };
        let (tw, _) = measure_text(font, label);
        draw_text(ctx, font, label, bx + (bw - tw) / 2.0, by, color_to_cg(fg));
    };

    let paint_input =
        |col: u16, row: u16, width: u16, text: &str, is_focused: bool, cursor: usize| {
            let bx = content_x + col as f64 * cw;
            let by = content_y + row as f64 * lh;
            let bw = width as f64 * cw;
            fill_rect(ctx, bx, by, bw, lh, theme.background);
            stroke_rect(ctx, bx, by, bw, lh, theme.separator, 0.5);
            paint_label(text, bx + 4.0, by, theme.foreground);
            if !is_focused {
                return;
            }
            let prefix_byte = text
                .char_indices()
                .nth(cursor)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            let prefix = &text[..prefix_byte];
            let (cpx, _) = measure_text(font, prefix);
            fill_rect(
                ctx,
                bx + 4.0 + cpx,
                by + 2.0,
                2.0,
                lh - 4.0,
                theme.foreground,
            );
        };

    let mut regex_end_col: Option<u16> = None;
    let mut prev_match_col: Option<u16> = None;

    for (region, target) in &panel.hit_regions {
        match target {
            T::Chevron => {
                let chevron = if panel.show_replace { "▼" } else { "▶" };
                let px = content_x + region.col as f64 * cw;
                let py = content_y + region.row as f64 * lh;
                paint_label(chevron, px, py, theme.foreground);
            }
            T::FindInput(_) => {
                paint_input(
                    region.col,
                    region.row,
                    region.width,
                    &panel.query,
                    panel.focus == 0,
                    panel.cursor,
                );
            }
            T::ReplaceInput(_) => {
                paint_input(
                    region.col,
                    region.row,
                    region.width,
                    &panel.replacement,
                    panel.focus == 1,
                    panel.cursor,
                );
            }
            T::ToggleCase => {
                paint_toggle(
                    region.col,
                    region.row,
                    region.width,
                    "Aa",
                    panel.case_sensitive,
                );
            }
            T::ToggleWholeWord => {
                paint_toggle(region.col, region.row, region.width, "ab", panel.whole_word);
            }
            T::ToggleRegex => {
                paint_toggle(region.col, region.row, region.width, ".*", panel.use_regex);
                regex_end_col = Some(region.col + region.width);
            }
            T::PrevMatch => {
                paint_glyph(region.col, region.row, region.width, "\u{2191}", false);
                prev_match_col.get_or_insert(region.col);
            }
            T::NextMatch => {
                paint_glyph(region.col, region.row, region.width, "\u{2193}", false);
            }
            T::ToggleInSelection => {
                paint_glyph(
                    region.col,
                    region.row,
                    region.width,
                    "\u{2261}",
                    panel.in_selection,
                );
            }
            T::Close => {
                paint_glyph(region.col, region.row, region.width, "\u{00d7}", false);
            }
            T::TogglePreserveCase => {
                paint_toggle(
                    region.col,
                    region.row,
                    region.width,
                    "AB",
                    panel.preserve_case,
                );
            }
            T::ReplaceCurrent => {
                paint_glyph(
                    region.col,
                    region.row,
                    region.width,
                    &panel.replace_one_glyph,
                    false,
                );
            }
            T::ReplaceAll => {
                paint_glyph(
                    region.col,
                    region.row,
                    region.width,
                    &panel.replace_all_glyph,
                    false,
                );
            }
        }
    }

    // Match count text between regex toggle and PrevMatch (same trick
    // GTK + TUI use — positions derived from neighbours, not a hit
    // region).
    if let (Some(start_col), Some(end_col)) = (regex_end_col, prev_match_col) {
        let info_col = start_col + 1;
        if end_col > info_col + 1 {
            let px = content_x + info_col as f64 * cw;
            let py = content_y;
            paint_label(&panel.match_info, px, py, theme.foreground);
        }
    }
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

unsafe fn stroke_rect(
    ctx: CGContextRef,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    c: Color,
    line_width: f64,
) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextStrokeRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextSetLineWidth(c: CGContextRef, w: core_graphics::base::CGFloat);
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextStrokeRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::find_replace::FindReplacePanel;
    use crate::Backend;

    const W: u32 = 600;
    const H: u32 = 200;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_panel() -> FindReplacePanel {
        let group_bounds = QRect::new(0.0, 0.0, W as f32, H as f32);
        let (hit_regions, _input_width) =
            crate::primitives::find_replace::compute_hit_regions(50, false, "", 2, 2);
        FindReplacePanel {
            query: "needle".into(),
            replacement: String::new(),
            show_replace: false,
            focus: 0,
            cursor: 6,
            sel_anchor: None,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds,
            panel_width: 50,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions,
        }
    }

    fn paint_via_backend(panel: &FindReplacePanel) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_find_replace(QRect::new(0.0, 0.0, W as f32, H as f32), panel);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn panel_paints_surface_bg() {
        let panel = sample_panel();
        let surface = paint_via_backend(&panel);
        let theme = Theme::default();
        // Panel is anchored top-right of group_bounds with a 10-px
        // inset. Probe inside the panel's left edge but past the
        // chevron.
        let panel_w_cells = panel.panel_width as f64;
        // char_width ~8.4; popup_w ≈ 420; popup_x ≈ W - 420 - 10 = 170.
        let cw = 8.4;
        let popup_w = panel_w_cells * cw;
        let popup_x = (W as f64 - popup_w - 10.0).max(0.0);
        // Probe a few px inside the right edge of the panel — outside
        // any glyph region.
        let px = (popup_x + popup_w - 4.0) as u32;
        let py = 4_u32; // top edge inside the panel
        let (r, g, b, _) = surface.pixel(px, py);
        // panel bg should differ from the surface fill (0,0,0).
        assert_ne!((r, g, b), (0, 0, 0));
        // And match theme.surface_bg (the panel bg).
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
        );
    }

    #[test]
    fn hit_regions_present_for_basic_panel() {
        let panel = sample_panel();
        assert!(
            !panel.hit_regions.is_empty(),
            "find/replace panel should expose hit regions",
        );
    }
}
