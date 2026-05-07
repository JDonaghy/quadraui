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
        // 8px gives a visible scrollbar against typical dark sidebar
        // backgrounds; the previous 4px was easy to miss. Hosts that
        // want a thinner scrollbar can compose `Scrollbar` directly.
        scrollbar_size: 8.0,
        // GTK paints at sub-pixel precision via Cairo; no quantization.
        cell_quantum: 0.0,
    }
}

/// Compute the layout for a `MultiSectionView` using the GTK metrics
/// that the rasteriser would use itself. Hosts call this to drive
/// hit-testing without re-computing or re-measuring — paint AND click
/// share this single layout per frame. Mirrors TUI's [`crate::tui::tui_msv_layout`]
/// in spirit: one source-of-truth layout produced by one set of
/// metrics, consumed by both paint and hit-test.
pub fn gtk_msv_layout(
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
    let view_layout = gtk_msv_layout(view, bounds, line_height);

    // Clip everything painted below to the panel area. Sections with
    // negative-y bounds (scrolled past the viewport top) extend beyond
    // the visible window — Cairo's clip silently drops the off-screen
    // portion so the next section's body doesn't get overpainted by a
    // tree from a previous section. Mirrors the TUI rasteriser's
    // `clip_to_viewport`.
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();

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
                paint_scrollbar(cr, sb_b, s_layout.thumb_bounds, theme);
            }
        }
    }

    if view.allow_resize {
        for d in &view_layout.dividers {
            paint_divider(cr, d.bounds, theme);
        }
    }

    // Restore the unclipped region so the panel-level scrollbar paints
    // on top without being itself clipped.
    cr.restore().ok();

    // Panel-level scrollbar (WholePanel mode when content overflows).
    if let Some(panel_sb) = view_layout.panel_scrollbar {
        let total_content: f32 = view_layout.sections.iter().map(|s| s.resolved_size).sum();
        paint_panel_scrollbar(cr, panel_sb, view.panel_scroll, total_content, theme);
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
    let by = bounds.y.round() as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height.round() as f64;

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
        cr.move_to(left_x, (by + (bh - ch as f64) * 0.4).round());
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
        cr.move_to(right_x, (by + (bh - gh as f64) * 0.4).round());
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
            cr.move_to(left_x, (by + (bh - th as f64) * 0.4).round());
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
    // Clip to body bounds so inner primitives (tree, list) can't
    // paint past the section boundary into the next header.
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();
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
    cr.restore().ok();
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

fn paint_scrollbar(cr: &Context, gutter: QRect, thumb_bounds: Option<QRect>, theme: &Theme) {
    let track = cairo_rgb(theme.scrollbar_track);
    let thumb = cairo_rgb(theme.scrollbar_thumb);

    let bx = gutter.x as f64;
    let by = gutter.y as f64;
    let bw = gutter.width as f64;
    let bh = gutter.height as f64;

    cr.set_source_rgba(track.0, track.1, track.2, 0.5);
    cr.rectangle(bx, by, bw, bh);
    cr.fill().ok();

    // Thumb at the layout-computed position when the body's scroll
    // state was introspectable (`Tree`, `List`). Falls back to a
    // 20%-tall top-anchored thumb for overflowing bodies without
    // row-based scroll — visual continuity with pre-#9. Per
    // *Primitive Authoring Rule #6*: thumb position is state-derived
    // and lives on the layout, not the rasteriser.
    let (ty, th) = match thumb_bounds {
        Some(t) => (t.y as f64, (t.height as f64).max(1.0)),
        None => (by, (bh * 0.2).max(20.0).min(bh)),
    };
    cr.set_source_rgba(thumb.0, thumb.1, thumb.2, 0.9);
    cr.rectangle(bx, ty, bw, th);
    cr.fill().ok();
}

/// Panel-level scrollbar with thumb size + position derived from
/// `panel_scroll` and the total content height. Painted with solid
/// colours (no alpha) and at the full `metrics.scrollbar_size` width
/// so it's actually visible against dark sidebar backgrounds; alpha
/// blending against unknown panel backgrounds was washing the thumb
/// out in onedark.
fn paint_panel_scrollbar(cr: &Context, bounds: QRect, scroll: f32, total: f32, theme: &Theme) {
    let track = cairo_rgb(theme.scrollbar_track);
    let thumb = cairo_rgb(theme.scrollbar_thumb);

    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;
    if bh <= 0.0 || total <= 0.0 {
        return;
    }

    cr.set_source_rgb(track.0, track.1, track.2);
    cr.rectangle(bx, by, bw, bh);
    cr.fill().ok();

    let visible_frac = (bh / total as f64).min(1.0);
    let scroll_frac = if total as f64 > bh {
        scroll as f64 / (total as f64 - bh)
    } else {
        0.0
    };
    let thumb_h = (bh * visible_frac).max(20.0);
    let thumb_track = (bh - thumb_h).max(0.0);
    let thumb_y = by + thumb_track * scroll_frac;
    cr.set_source_rgb(thumb.0, thumb.1, thumb.2);
    cr.rectangle(bx, thumb_y, bw, thumb_h);
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

// ── Tests ──────────────────────────────────────────────────────────────────
//
// Paint↔click round-trip harness for the GTK rasteriser. Mirrors the
// TUI harness pattern in `tui::multi_section_view::tests` but paints
// into a `cairo::ImageSurface` instead of a ratatui `Buffer` and
// inspects pixels rather than glyphs.
//
// The bug class this catches: paint position derived from one set of
// bounds while hit-test consumes another. On GTK the typical drift
// vectors are subpixel rounding (paint snaps to integer pixels while
// hit_test consumes fractional bounds) and font-metric quirks (the
// rasteriser uses `line_height * 1.4` for body row pitch while a
// drifting copy might use `line_height`).
//
// Tests are gated on `#[cfg(all(test, feature = "gtk"))]` so they
// only run under `cargo test --features gtk`. They don't need a real
// display — `cairo::ImageSurface` is pure memory; Pango uses
// fontconfig and works headless.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::multi_section_view::{
        MultiSectionViewHit, ScrollMode, Section, SectionSize,
    };
    use crate::primitives::tree::{TreeRow, TreeView};
    use crate::types::{Color, Decoration, SelectionMode, WidgetId};
    use pangocairo::cairo::{Format, ImageSurface};

    /// Surface canvas size: wide enough for chevron + title + (optional)
    /// scrollbar gutter; tall enough for several `EqualShare` sections.
    const W: i32 = 200;
    const H: i32 = 200;

    /// Standard line-height the GTK rasteriser is parameterised by.
    /// Header rows render at `line_height * 1.4`; body rows at the
    /// same (per `body_measure`).
    const LINE_HEIGHT: f64 = 14.0;

    /// Build a [`Theme`] where the canonical background is pure white
    /// (RGB 255,255,255) so any non-white pixel in the surface came
    /// from `draw_multi_section_view` — header/aux fills, scrollbar
    /// track/thumb, body fills, or rendered text. Other colors stay
    /// at their defaults so the painted regions are visibly inked.
    fn test_theme() -> Theme {
        Theme {
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        }
    }

    fn tree_section(id: &str, items: &[&str]) -> Section {
        let rows: Vec<TreeRow> = items
            .iter()
            .enumerate()
            .map(|(i, t)| TreeRow {
                path: vec![i as u16],
                indent: 0,
                icon: None,
                text: StyledText::plain((*t).to_string()),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            })
            .collect();
        Section {
            id: id.into(),
            header: SectionHeader {
                title: StyledText::plain(id.to_uppercase()),
                show_chevron: false,
                ..Default::default()
            },
            body: SectionBody::Tree(TreeView {
                id: WidgetId::new(format!("{}-tree", id)),
                rows,
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: Default::default(),
                has_focus: true,
            }),
            aux: None,
            size: SectionSize::EqualShare,
            collapsed: false,
            min_size: None,
            max_size: None,
        }
    }

    fn view_with(sections: Vec<Section>) -> MultiSectionView {
        MultiSectionView {
            id: WidgetId::new("v"),
            sections,
            active_section: None,
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        }
    }

    /// Paint `view` into a fresh surface; return (surface, layout).
    /// Hit-test uses the SAME layout the rasteriser used internally —
    /// that's the source-of-truth contract `gtk_msv_layout` enforces.
    fn paint_then_layout(view: &MultiSectionView) -> (ImageSurface, MultiSectionViewLayout) {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        // Clear surface to white so non-white pixels uniquely identify
        // painted regions.
        {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let layout = pangocairo::functions::create_layout(&cr);
            draw_multi_section_view(
                &cr,
                &layout,
                0.0,
                0.0,
                W as f64,
                H as f64,
                view,
                &test_theme(),
                LINE_HEIGHT,
                /* nerd_fonts */ false,
            );
        }
        let bounds = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = gtk_msv_layout(view, bounds, LINE_HEIGHT);
        (surface, layout)
    }

    /// Read pixel at (x, y) as (r, g, b). Cairo ARGB32 byte order on
    /// little-endian is BGRA, so byte[0]=B, byte[1]=G, byte[2]=R.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    fn is_painted(data: &[u8], stride: usize, x: i32, y: i32) -> bool {
        let (r, g, b) = pixel(data, stride, x, y);
        !(r == 255 && g == 255 && b == 255)
    }

    /// Find any painted pixel within (x_range, y_range). Returns
    /// (x, y) of the first non-white pixel, scanning row-major.
    fn first_painted_in(
        data: &[u8],
        stride: usize,
        x_range: (i32, i32),
        y_range: (i32, i32),
    ) -> Option<(i32, i32)> {
        for y in y_range.0..y_range.1 {
            for x in x_range.0..x_range.1 {
                if x < 0 || y < 0 || x >= W || y >= H {
                    continue;
                }
                if is_painted(data, stride, x, y) {
                    return Some((x, y));
                }
            }
        }
        None
    }

    /// Header round-trip: paint a section, find a painted pixel in
    /// its header band, hit_test that exact pixel, assert the hit
    /// identifies the same section's `Header`. Catches drift between
    /// paint and layout for header bounds.
    #[test]
    fn gtk_header_round_trip_paint_to_pixel_to_hit_test() {
        let v = view_with(vec![
            tree_section("alpha", &["a0", "a1", "a2"]),
            tree_section("beta", &["b0", "b1", "b2"]),
            tree_section("gamma", &["g0", "g1", "g2"]),
        ]);
        let (mut surface, layout) = paint_then_layout(&v);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        for s in &layout.sections {
            let hb = s.header_bounds;
            // Interior-pixel scan: skip the 1px boundary on each edge
            // because Cairo anti-aliases fractional bounds onto integer
            // pixels (cell_quantum: 0.0 on GTK). A boundary pixel is
            // mixed-ink by design — the contract is that *interior*
            // pixels paint the section's ink AND hit_test there
            // returns that section. The TUI analogue (cell_quantum:
            // 1.0) snaps bounds to integers and avoids the AA region;
            // GTK accepts AA and asserts at interior pixels only.
            let x_range = (
                (hb.x + 1.0).floor() as i32,
                (hb.x + hb.width - 1.0).floor() as i32,
            );
            let y_range = (
                (hb.y + 1.0).floor() as i32,
                (hb.y + hb.height - 1.0).floor() as i32,
            );
            let painted = first_painted_in(&data, stride, x_range, y_range).unwrap_or_else(|| {
                panic!(
                    "section {} interior header bounds (x {}..{}, y {}..{}) contained no painted pixel",
                    s.section_idx, x_range.0, x_range.1, y_range.0, y_range.1
                )
            });
            let hit = layout.hit_test(painted.0 as f32 + 0.5, painted.1 as f32 + 0.5);
            match hit {
                MultiSectionViewHit::Header { section, .. } => assert_eq!(
                    section, s.section_idx,
                    "pixel ({}, {}) painted in section {} header but hit_test returned section {}",
                    painted.0, painted.1, s.section_idx, section
                ),
                other => panic!(
                    "pixel ({}, {}) painted in section {} header but hit_test returned {:?}",
                    painted.0, painted.1, s.section_idx, other
                ),
            }
        }
    }

    /// Body round-trip: each section's body bounds contain at least
    /// one painted pixel; hit_test at that pixel returns Body for the
    /// same section. Catches "body painted into the wrong y-range" and
    /// "hit_test at painted body row returns Header of next section".
    #[test]
    fn gtk_body_round_trip_paint_to_pixel_to_hit_test() {
        let v = view_with(vec![
            tree_section("alpha", &["a0", "a1", "a2"]),
            tree_section("beta", &["b0", "b1", "b2"]),
            tree_section("gamma", &["g0", "g1", "g2"]),
        ]);
        let (mut surface, layout) = paint_then_layout(&v);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        for s in &layout.sections {
            let bb = s.body_bounds;
            if bb.height < 3.0 || bb.width < 3.0 {
                continue;
            }
            // Interior-pixel scan; see header test for the AA rationale.
            let x_range = (
                (bb.x + 1.0).floor() as i32,
                (bb.x + bb.width - 1.0).floor() as i32,
            );
            let y_range = (
                (bb.y + 1.0).floor() as i32,
                (bb.y + bb.height - 1.0).floor() as i32,
            );
            let painted = first_painted_in(&data, stride, x_range, y_range).unwrap_or_else(|| {
                panic!(
                    "section {} interior body bounds (x {}..{}, y {}..{}) contained no painted pixel",
                    s.section_idx, x_range.0, x_range.1, y_range.0, y_range.1
                )
            });
            let hit = layout.hit_test(painted.0 as f32 + 0.5, painted.1 as f32 + 0.5);
            match hit {
                MultiSectionViewHit::Body { section } => assert_eq!(
                    section, s.section_idx,
                    "pixel ({}, {}) painted in section {} body but hit_test returned Body{{{}}}",
                    painted.0, painted.1, s.section_idx, section
                ),
                other => panic!(
                    "pixel ({}, {}) painted in section {} body but hit_test returned {:?}",
                    painted.0, painted.1, s.section_idx, other
                ),
            }
        }
    }

    /// Overflowing section reserves a scrollbar gutter on the trailing
    /// edge. Click in the gutter → `Scrollbar`, NOT `Body`. Click left
    /// of the gutter → `Body`. Mirror of TUI's
    /// `scrollbar_column_hits_scrollbar_not_body_when_section_overflows`.
    #[test]
    fn gtk_scrollbar_column_hits_scrollbar_not_body_when_overflowing() {
        // 1 section with enough rows that body overflows.
        let v = view_with(vec![tree_section(
            "lots",
            &[
                "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12",
                "r13", "r14", "r15",
            ],
        )]);
        let (mut surface, layout) = paint_then_layout(&v);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let s = &layout.sections[0];
        let sb = s
            .scrollbar_bounds
            .expect("overflowing section must reserve a scrollbar gutter (paint↔click contract)");

        // Hit_test the centre of the gutter — should be Scrollbar.
        let click_x = sb.x + sb.width / 2.0;
        let click_y = sb.y + sb.height / 2.0;
        match layout.hit_test(click_x, click_y) {
            MultiSectionViewHit::Scrollbar { section, .. } => assert_eq!(section, 0),
            other => panic!(
                "click at gutter centre ({:.1}, {:.1}) returned {:?}",
                click_x, click_y, other
            ),
        }

        // Pixel inside the gutter must be painted (track or thumb).
        let gx = sb.x.floor() as i32 + 1;
        let gy = sb.y.floor() as i32 + 5;
        if gx < W && gy < H {
            assert!(
                is_painted(&data, stride, gx, gy),
                "scrollbar gutter at pixel ({}, {}) was not painted",
                gx,
                gy
            );
        }

        // Hit_test left of the gutter — should be Body.
        let body_b = s.body_bounds;
        if body_b.width >= 2.0 {
            let click = layout.hit_test(body_b.x + 1.0, body_b.y + 1.0);
            assert!(
                matches!(click, MultiSectionViewHit::Body { section: 0 }),
                "click at body interior returned {:?}; expected Body{{0}}",
                click
            );
        }
    }

    /// Subpixel safety: a fractional `EqualShare` distribution
    /// produces section bounds with non-integer y/height. Paint and
    /// hit_test must agree on integer pixel y. For each section, find
    /// a painted pixel inside its header band, hit_test, assert the
    /// section index matches.
    ///
    /// GTK keeps `cell_quantum: 0.0` (no integer snap; Cairo paints at
    /// fractional coords directly). The contract: hit_test on integer
    /// pixel coords routes to whichever section's logical bounds
    /// contain that y. A pixel painted at y=N is logically inside
    /// whichever section spans y=N..N+1.
    #[test]
    fn gtk_subpixel_section_bounds_round_trip() {
        // 3 sections in a height that doesn't divide evenly. With
        // line_height=14, header=14*1.4=19.6, this exercises
        // fractional bounds.
        let v = view_with(vec![
            tree_section("alpha", &["a0", "a1"]),
            tree_section("beta", &["b0", "b1"]),
            tree_section("gamma", &["g0", "g1"]),
        ]);
        let (mut surface, layout) = paint_then_layout(&v);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        // For each section, find an interior painted pixel inside its
        // y-span (skipping the 1px AA boundary on each edge).
        for s in &layout.sections {
            let section_top = s.header_bounds.y;
            let section_bot = section_top + s.resolved_size;
            let y_top = (section_top + 1.0).floor() as i32;
            let y_bot = (section_bot - 1.0).floor() as i32;
            let painted = first_painted_in(&data, stride, (1, W - 1), (y_top, y_bot.min(H)))
                .unwrap_or_else(|| {
                    panic!(
                        "section {} interior (y={}..{}) contained no painted pixel",
                        s.section_idx, y_top, y_bot
                    )
                });
            let hit = layout.hit_test(painted.0 as f32 + 0.5, painted.1 as f32 + 0.5);
            let hit_section = match hit {
                MultiSectionViewHit::Header { section, .. } => section,
                MultiSectionViewHit::Body { section } => section,
                MultiSectionViewHit::Scrollbar { section, .. } => section,
                other => panic!(
                    "section {} (y={}..{}) painted at pixel ({}, {}) but hit_test returned {:?}",
                    s.section_idx, y_top, y_bot, painted.0, painted.1, other
                ),
            };
            assert_eq!(
                hit_section, s.section_idx,
                "pixel ({}, {}) painted in section {} (y range {}..{}) but hit_test returned section {}",
                painted.0, painted.1, s.section_idx, y_top, y_bot, hit_section
            );
        }
    }
}
