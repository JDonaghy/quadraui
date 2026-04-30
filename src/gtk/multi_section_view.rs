//! GTK rasteriser for [`crate::MultiSectionView`].
//!
//! Paints the full chrome (per-section headers, optional aux rows,
//! per-section scrollbars, optional dividers) onto a [`Context`] and
//! dispatches each section's body to the appropriate quadraui body
//! rasteriser (`draw_tree`, `draw_list`, etc.) using the body bounds
//! returned by the primitive's [`crate::MultiSectionView::layout`].
//!
//! Vertical-only in v1 (per #294 / D-003 in `quadraui/docs/DECISIONS.md`);
//! horizontal sections fall through to a no-op.
//!
//! # Why one source of truth
//!
//! The #281 smoke wave surfaced four classes of paint/click drift in the
//! debug-sidebar GTK port — every one a "paint and click computed
//! layout from different sources." This rasteriser asks the primitive
//! for one [`crate::MultiSectionViewLayout`] and consumes it verbatim
//! for paint; the host's click handler asks the same primitive (with
//! the same metrics) for the same layout and consumes its
//! `hit_test`. Discrepancy is impossible by construction.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::{cairo_rgb, draw_form, draw_list, draw_message_list, draw_tree};
use crate::event::Rect as QRect;
use crate::primitives::multi_section_view::{
    Axis, EmptyBody, LayoutMetrics, MultiSectionView, MultiSectionViewLayout, SectionAux,
    SectionBody, SectionHeader, SectionMeasure,
};
use crate::theme::Theme;
use crate::types::StyledText;

/// Compute the GTK metrics for a `MultiSectionView` from a
/// `line_height`. Backends call this AND the primitive's `layout()`
/// with the same metrics so paint and click resolve to the same bounds.
pub fn metrics_for(line_height: f64, allow_resize: bool) -> LayoutMetrics {
    LayoutMetrics {
        header_size: (line_height * 1.4) as f32,
        divider_size: if allow_resize { 1.0 } else { 0.0 },
        scrollbar_size: 4.0,
    }
}

/// Compute the layout for a `MultiSectionView` using the GTK metrics
/// that the rasteriser would use itself. Hosts call this to drive
/// hit-testing without re-computing or re-measuring — paint AND click
/// share this single layout per frame.
pub fn layout_for(
    view: &MultiSectionView,
    bounds: QRect,
    line_height: f64,
) -> MultiSectionViewLayout {
    let metrics = metrics_for(line_height, view.allow_resize);
    view.layout(bounds, metrics, |i| {
        body_measure(&view.sections[i].body, &view.sections[i].aux, line_height)
    })
}

fn body_measure(body: &SectionBody, aux: &Option<SectionAux>, line_height: f64) -> SectionMeasure {
    let aux_size = if aux.is_some() {
        // Inline inputs and toolbars match leaf-row height in GTK
        // conventions.
        (line_height * 1.4) as f32
    } else {
        0.0
    };
    let item_h = (line_height * 1.4) as f32;
    let content_size = match body {
        SectionBody::Tree(t) => {
            // Mirror the GTK tree row convention: headers 1.0×,
            // others 1.4×.
            let mut total = 0.0_f32;
            for row in &t.rows {
                let is_header = matches!(row.decoration, crate::types::Decoration::Header);
                total += if is_header {
                    line_height as f32
                } else {
                    item_h
                };
            }
            total
        }
        SectionBody::List(l) => {
            let title_h = if l.title.is_some() {
                line_height as f32
            } else {
                0.0
            };
            title_h + l.items.len() as f32 * item_h
        }
        SectionBody::Form(f) => f.fields.len() as f32 * item_h,
        SectionBody::MessageList(m) => {
            // 1 header row + body lines per message.
            m.rows
                .iter()
                .map(|r| {
                    let lines = r.text.lines().count().max(1) as f32;
                    line_height as f32 + lines * line_height as f32
                })
                .sum()
        }
        SectionBody::Terminal(_) => 0.0,
        SectionBody::Text(lines) => lines.len() as f32 * line_height as f32,
        SectionBody::Empty(_) => item_h * 4.0, // icon + text + hint + action
        SectionBody::Custom(_) => 0.0,
    };
    SectionMeasure {
        content_size,
        aux_size,
    }
}

/// Draw a [`MultiSectionView`] into `(x, y, w, h)` on `cr`.
#[allow(clippy::too_many_arguments)]
pub fn draw_multi_section_view(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    view: &MultiSectionView,
    theme: &Theme,
    line_height: f64,
    nerd_fonts_enabled: bool,
) {
    if w <= 0.0 || h <= 0.0 || view.axis == Axis::Horizontal {
        return;
    }

    let bg = cairo_rgb(theme.background);
    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();
    layout.set_attributes(None);

    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let view_layout = layout_for(view, bounds, line_height);

    for s_layout in &view_layout.sections {
        let section = &view.sections[s_layout.section_idx];

        paint_header(
            cr,
            layout,
            s_layout.header_bounds,
            &section.header,
            section.collapsed,
            theme,
        );

        if !s_layout.collapsed {
            if let (Some(aux), Some(aux_b)) = (&section.aux, s_layout.aux_bounds) {
                paint_aux(cr, layout, aux_b, aux, theme);
            }

            paint_body(
                cr,
                layout,
                s_layout.body_bounds,
                &section.body,
                theme,
                line_height,
                nerd_fonts_enabled,
            );

            if let Some(sb_b) = s_layout.scrollbar_bounds {
                paint_scrollbar(cr, sb_b, theme);
            }
        }
    }

    if view.allow_resize {
        for d in &view_layout.dividers {
            paint_divider(cr, d.bounds, theme);
        }
    }

    // Panel-level scrollbar (WholePanel mode when content overflows).
    if let Some(panel_sb) = view_layout.panel_scrollbar {
        paint_scrollbar(cr, panel_sb, theme);
    }
}

// ── Section paint helpers ──────────────────────────────────────────────────

fn paint_header(
    cr: &Context,
    layout: &pango::Layout,
    bounds: QRect,
    header: &SectionHeader,
    collapsed: bool,
    theme: &Theme,
) {
    let bg = cairo_rgb(theme.header_bg);
    let fg = cairo_rgb(theme.header_fg);
    let dim = cairo_rgb(theme.muted_fg);

    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(bx, by, bw, bh);
    cr.fill().ok();

    layout.set_attributes(None);
    let mut left_x = bx + 4.0;

    if header.show_chevron {
        let chevron = if collapsed { "▸" } else { "▾" };
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        layout.set_text(chevron);
        let (cw, ch) = layout.pixel_size();
        cr.move_to(left_x, by + (bh - ch as f64) / 2.0);
        pcfn::show_layout(cr, layout);
        left_x += cw as f64 + 4.0;
    }

    // Right-aligned actions, right-to-left.
    let mut right_x = bx + bw - 4.0;
    for action in header.actions.iter().rev() {
        let glyph = action.icon.fallback.as_str();
        let action_fg = if action.enabled { fg } else { dim };
        layout.set_text(glyph);
        let (gw, gh) = layout.pixel_size();
        right_x -= gw as f64;
        if right_x < left_x {
            break;
        }
        cr.set_source_rgb(action_fg.0, action_fg.1, action_fg.2);
        cr.move_to(right_x, by + (bh - gh as f64) / 2.0);
        pcfn::show_layout(cr, layout);
        right_x -= 8.0; // gap between actions
    }

    // Title text.
    let title_text: String = header.title.spans.iter().map(|s| s.text.as_str()).collect();
    if !title_text.is_empty() {
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        layout.set_text(&title_text);
        let (tw, th) = layout.pixel_size();
        let max_w = (right_x - left_x).max(0.0);
        if max_w > 0.0 {
            cr.move_to(left_x, by + (bh - th as f64) / 2.0);
            // Pango clips automatically when we don't set width; sub-row
            // truncation is handled by the user-visible row width.
            pcfn::show_layout(cr, layout);
            let mut after_title_x = left_x + (tw as f64).min(max_w);

            // Badge after title.
            if let Some(badge) = &header.badge {
                let badge_text: String = badge.spans.iter().map(|s| s.text.as_str()).collect();
                if !badge_text.is_empty() {
                    after_title_x += 6.0;
                    if after_title_x < right_x {
                        cr.set_source_rgb(dim.0, dim.1, dim.2);
                        layout.set_text(&badge_text);
                        let (_, bh_text) = layout.pixel_size();
                        cr.move_to(after_title_x, by + (bh - bh_text as f64) / 2.0);
                        pcfn::show_layout(cr, layout);
                    }
                }
            }
        }
    }
}

fn paint_aux(cr: &Context, layout: &pango::Layout, bounds: QRect, aux: &SectionAux, theme: &Theme) {
    let bg = cairo_rgb(theme.input_bg);
    let fg = cairo_rgb(theme.foreground);
    let dim = cairo_rgb(theme.muted_fg);

    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(bx, by, bw, bh);
    cr.fill().ok();
    layout.set_attributes(None);

    match aux {
        SectionAux::Input(input) | SectionAux::Search(input) => {
            let display: &str = if input.text.is_empty() && !input.has_focus {
                input.placeholder.as_deref().unwrap_or("")
            } else {
                input.text.as_str()
            };
            let text_fg = if input.text.is_empty() && !input.has_focus {
                dim
            } else {
                fg
            };
            cr.set_source_rgb(text_fg.0, text_fg.1, text_fg.2);
            layout.set_text(display);
            let (_, th) = layout.pixel_size();
            cr.move_to(bx + 4.0, by + (bh - th as f64) / 2.0);
            pcfn::show_layout(cr, layout);

            // Caret as a 1-cell-wide vertical bar at the caret column.
            if input.has_focus {
                let prefix: String = input.text.chars().take(input.caret).collect();
                layout.set_text(&prefix);
                let (cx_off, _) = layout.pixel_size();
                let caret_x = bx + 4.0 + cx_off as f64;
                cr.set_source_rgb(fg.0, fg.1, fg.2);
                cr.rectangle(caret_x, by + 2.0, 1.0, bh - 4.0);
                cr.fill().ok();
            }
        }
        SectionAux::Toolbar(actions) => {
            let mut x = bx + 4.0;
            for a in actions {
                let glyph = a.icon.fallback.as_str();
                let action_fg = if a.enabled { fg } else { dim };
                cr.set_source_rgb(action_fg.0, action_fg.1, action_fg.2);
                layout.set_text(glyph);
                let (gw, gh) = layout.pixel_size();
                cr.move_to(x, by + (bh - gh as f64) / 2.0);
                pcfn::show_layout(cr, layout);
                x += gw as f64 + 8.0;
            }
        }
        SectionAux::Custom(_) => {
            // Host paints; we cleared the bg already.
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_body(
    cr: &Context,
    layout: &pango::Layout,
    bounds: QRect,
    body: &SectionBody,
    theme: &Theme,
    line_height: f64,
    nerd_fonts_enabled: bool,
) {
    let x = bounds.x as f64;
    let y = bounds.y as f64;
    let w = bounds.width as f64;
    let h = bounds.height as f64;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    match body {
        SectionBody::Tree(t) => {
            draw_tree(
                cr,
                layout,
                x,
                y,
                w,
                h,
                t,
                theme,
                line_height,
                nerd_fonts_enabled,
            );
        }
        SectionBody::List(l) => {
            draw_list(
                cr,
                layout,
                x,
                y,
                w,
                h,
                l,
                theme,
                line_height,
                nerd_fonts_enabled,
            );
        }
        SectionBody::Form(f) => {
            draw_form(cr, layout, x, y, w, h, f, theme, line_height);
        }
        SectionBody::MessageList(m) => {
            draw_message_list(cr, layout, m, x, y, w, y + h, line_height);
        }
        SectionBody::Terminal(_) => {
            // No standalone Terminal rasteriser uses this signature today;
            // host paints Terminal cells themselves.
        }
        SectionBody::Text(lines) => {
            paint_text_lines(cr, layout, x, y, w, h, lines, theme, line_height);
        }
        SectionBody::Empty(empty) => {
            paint_empty_body(cr, layout, x, y, w, h, empty, theme, line_height);
        }
        SectionBody::Custom(_) => {
            // Host paints in the body bounds.
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_text_lines(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    lines: &[StyledText],
    theme: &Theme,
    line_height: f64,
) {
    let bg = cairo_rgb(theme.background);
    let fg = cairo_rgb(theme.foreground);
    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();
    cr.set_source_rgb(fg.0, fg.1, fg.2);
    layout.set_attributes(None);

    let mut row_y = y;
    for line in lines {
        if row_y + line_height > y + h {
            break;
        }
        let text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
        layout.set_text(&text);
        let (_, th) = layout.pixel_size();
        cr.move_to(x + 4.0, row_y + (line_height - th as f64) / 2.0);
        pcfn::show_layout(cr, layout);
        row_y += line_height;
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_empty_body(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    empty: &EmptyBody,
    theme: &Theme,
    line_height: f64,
) {
    let bg = cairo_rgb(theme.background);
    let fg = cairo_rgb(theme.foreground);
    let dim = cairo_rgb(theme.muted_fg);
    let accent = cairo_rgb(theme.accent_fg);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();
    layout.set_attributes(None);

    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let mut blocks: Vec<(String, (f64, f64, f64))> = Vec::new();
    if let Some(icon) = &empty.icon {
        blocks.push((icon.fallback.clone(), fg));
    }
    let primary: String = empty.text.spans.iter().map(|s| s.text.as_str()).collect();
    if !primary.is_empty() {
        blocks.push((primary, fg));
    }
    if let Some(hint) = &empty.hint {
        let hint_str: String = hint.spans.iter().map(|s| s.text.as_str()).collect();
        if !hint_str.is_empty() {
            blocks.push((hint_str, dim));
        }
    }
    if let Some(action) = &empty.action {
        let label = action
            .tooltip
            .clone()
            .unwrap_or_else(|| action.icon.fallback.clone());
        blocks.push((format!("[ {label} ]"), accent));
    }

    if blocks.is_empty() {
        return;
    }

    let total_h = blocks.len() as f64 * line_height;
    let mut block_y = y + (h - total_h).max(0.0) / 2.0;
    for (text, color) in &blocks {
        layout.set_text(text);
        let (tw, th) = layout.pixel_size();
        let block_x = x + (w - tw as f64).max(0.0) / 2.0;
        cr.set_source_rgb(color.0, color.1, color.2);
        cr.move_to(block_x, block_y + (line_height - th as f64) / 2.0);
        pcfn::show_layout(cr, layout);
        block_y += line_height;
    }
}

fn paint_scrollbar(cr: &Context, bounds: QRect, theme: &Theme) {
    let track = cairo_rgb(theme.scrollbar_track);
    let thumb = cairo_rgb(theme.scrollbar_thumb);

    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    cr.set_source_rgba(track.0, track.1, track.2, 0.3);
    cr.rectangle(bx, by, bw, bh);
    cr.fill().ok();

    // Default 1-cell thumb at top — backends with real scroll geometry
    // overlay a `Scrollbar` primitive on top to refine.
    let thumb_h = (bh * 0.2).max(20.0).min(bh);
    cr.set_source_rgba(thumb.0, thumb.1, thumb.2, 0.7);
    cr.rectangle(bx, by, bw, thumb_h);
    cr.fill().ok();
}

fn paint_divider(cr: &Context, bounds: QRect, theme: &Theme) {
    let sep = cairo_rgb(theme.separator);
    cr.set_source_rgb(sep.0, sep.1, sep.2);
    cr.rectangle(
        bounds.x as f64,
        bounds.y as f64,
        bounds.width as f64,
        bounds.height as f64,
    );
    cr.fill().ok();
}
