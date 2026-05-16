//! macOS rasteriser for [`crate::MultiSectionView`].
//!
//! Paints the full chrome (per-section headers, optional aux rows,
//! per-section scrollbars, optional dividers) onto a `CGContextRef`
//! and dispatches each section's body to the appropriate quadraui
//! body rasteriser (`draw_tree`, `draw_list`, `draw_form`, …) using
//! the body bounds returned by the primitive's
//! [`crate::MultiSectionView::layout`].
//!
//! Vertical-only in v1 (per #294 / D-003 in
//! `quadraui/docs/DECISIONS.md`); horizontal sections fall through to
//! a no-op.
//!
//! Mirrors [`crate::gtk::multi_section_view`] in shape:
//! [`mac_msv_metrics`] computes the layout metrics for a given
//! `line_height`, [`mac_msv_layout`] returns the resolved chrome
//! layout, and [`draw_multi_section_view`] consumes the same layout
//! for paint. Apps call `mac_msv_layout` for hit-testing so paint and
//! click share one source of truth.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Terminal section bodies** — `Terminal` rasteriser lands in #43.
//!   `SectionBody::Terminal` paints the bg only for now.
//! - **MessageList section bodies** — same; `MessageList` lands in #43.
//! - **Custom-icon empty bodies** — placeholder text rendering matches
//!   GTK but the `EmptyBody::action` button is rendered as plain
//!   centred text (no clickable button chrome yet).

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as QRect;
use crate::primitives::multi_section_view::{
    Axis, EmptyBody, LayoutMetrics, MultiSectionView, MultiSectionViewLayout, SectionAux,
    SectionBody, SectionHeader, SectionMeasure,
};
use crate::theme::Theme;
use crate::types::{Color, StyledText};

/// Compute the macOS metrics for a `MultiSectionView` from a
/// `line_height`. Hosts call this and the primitive's `layout()`
/// with the same metrics so paint and click resolve to the same bounds.
pub fn mac_msv_metrics(line_height: f64, allow_resize: bool) -> LayoutMetrics {
    LayoutMetrics {
        header_size: (line_height * 1.4) as f32,
        divider_size: if allow_resize { 1.0 } else { 0.0 },
        // Matches GTK: 8 px gives a visible track against dark sidebars.
        scrollbar_size: 8.0,
        // CG paints at sub-pixel precision; no quantization.
        cell_quantum: 0.0,
    }
}

/// Compute the layout for a `MultiSectionView` using the macOS metrics
/// the rasteriser would use itself. Hosts call this to drive hit-
/// testing without re-computing — paint and click share this single
/// layout per frame.
pub fn mac_msv_layout(
    view: &MultiSectionView,
    bounds: QRect,
    line_height: f64,
) -> MultiSectionViewLayout {
    let metrics = mac_msv_metrics(line_height, view.allow_resize);
    view.layout(bounds, metrics, |i| {
        body_measure(&view.sections[i].body, &view.sections[i].aux, line_height)
    })
}

fn body_measure(body: &SectionBody, aux: &Option<SectionAux>, line_height: f64) -> SectionMeasure {
    let item_h = (line_height * 1.4).round() as f32;
    let aux_size = if aux.is_some() { item_h } else { 0.0 };
    let content_size = match body {
        SectionBody::Tree(t) => {
            let header_h = (line_height * 1.2).round() as f32;
            let mut total = 0.0_f32;
            for row in &t.rows {
                let is_header = matches!(row.decoration, crate::types::Decoration::Header);
                total += if is_header { header_h } else { item_h };
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
        SectionBody::Chart(c) => {
            if matches!(c.kind, crate::primitives::chart::ChartKind::Sparkline) {
                line_height as f32
            } else {
                item_h * 8.0
            }
        }
        SectionBody::MessageList(_) | SectionBody::Terminal(_) => 0.0,
        SectionBody::Text(lines) => lines.len() as f32 * line_height as f32,
        SectionBody::Empty(_) => item_h * 4.0,
        SectionBody::Custom(_) => 0.0,
    };
    SectionMeasure {
        content_size,
        aux_size,
    }
}

/// Paint `view` into `(x, y, w, h)` on `ctx`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_multi_section_view(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    view: &MultiSectionView,
    theme: &Theme,
    line_height: f64,
    char_width: f64,
    caret_visible: bool,
) {
    if w <= 0.0 || h <= 0.0 || view.axis == Axis::Horizontal {
        return;
    }

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));
    fill_rect(ctx, x, y, w, h, theme.background);

    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let view_layout = mac_msv_layout(view, bounds, line_height);

    for s_layout in &view_layout.sections {
        let section = &view.sections[s_layout.section_idx];

        paint_header(
            ctx,
            font,
            s_layout.header_bounds,
            &section.header,
            section.collapsed,
            theme,
        );

        if !s_layout.collapsed {
            if let (Some(aux), Some(aux_b)) = (&section.aux, s_layout.aux_bounds) {
                paint_aux(ctx, font, aux_b, aux, theme, caret_visible);
            }

            paint_body(
                ctx,
                font,
                s_layout.body_bounds,
                &section.body,
                theme,
                line_height,
                char_width,
            );

            if let Some(sb_b) = s_layout.scrollbar_bounds {
                paint_section_scrollbar(ctx, sb_b, s_layout.thumb_bounds, theme);
            }
        }
    }

    if view.allow_resize {
        for d in &view_layout.dividers {
            fill_rect(
                ctx,
                d.bounds.x as f64,
                d.bounds.y as f64,
                d.bounds.width as f64,
                d.bounds.height as f64,
                theme.separator,
            );
        }
    }

    CGContextRestoreGState(ctx);

    // Panel-level scrollbar (WholePanel mode) painted outside the
    // panel clip so it isn't itself clipped.
    if let Some(panel_sb) = view_layout.panel_scrollbar {
        let total_content: f32 = view_layout.sections.iter().map(|s| s.resolved_size).sum();
        paint_panel_scrollbar(ctx, panel_sb, view.panel_scroll, total_content, theme);
    }
}

unsafe fn paint_header(
    ctx: CGContextRef,
    font: &CTFont,
    bounds: QRect,
    header: &SectionHeader,
    collapsed: bool,
    theme: &Theme,
) {
    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    fill_rect(ctx, bx, by, bw, bh, theme.header_bg);

    let mut left_x = bx + 4.0;
    let row_text_y = |th: f64| (by + (bh - th) * 0.4).round();

    if header.show_chevron {
        let chevron = if collapsed { "▸" } else { "▾" };
        let (cw, ch) = measure_text(font, chevron);
        draw_text(
            ctx,
            font,
            chevron,
            left_x,
            row_text_y(ch),
            color_to_cg(theme.header_fg),
        );
        left_x += cw + 4.0;
    }

    // Right-aligned actions, right-to-left.
    let mut right_x = bx + bw - 4.0;
    for action in header.actions.iter().rev() {
        let glyph = action.icon.fallback.as_str();
        let (gw, gh) = measure_text(font, glyph);
        right_x -= gw;
        if right_x < left_x {
            break;
        }
        let action_fg = if action.enabled {
            theme.header_fg
        } else {
            theme.muted_fg
        };
        draw_text(
            ctx,
            font,
            glyph,
            right_x,
            row_text_y(gh),
            color_to_cg(action_fg),
        );
        right_x -= 8.0;
    }

    // Title text + badge.
    let title_text: String = header.title.spans.iter().map(|s| s.text.as_str()).collect();
    if !title_text.is_empty() {
        let (tw, th) = measure_text(font, &title_text);
        let max_w = (right_x - left_x).max(0.0);
        if max_w > 0.0 {
            // Clip title to the header's title region.
            CGContextSaveGState(ctx);
            CGContextClipToRect(ctx, CGRect::new_xywh(left_x, by, max_w, bh));
            draw_text(
                ctx,
                font,
                &title_text,
                left_x,
                row_text_y(th),
                color_to_cg(theme.header_fg),
            );
            CGContextRestoreGState(ctx);
            let after_title_x = left_x + tw.min(max_w);

            if let Some(badge) = &header.badge {
                let badge_text: String = badge.spans.iter().map(|s| s.text.as_str()).collect();
                if !badge_text.is_empty() {
                    let badge_x = after_title_x + 6.0;
                    if badge_x < right_x {
                        let (_, bth) = measure_text(font, &badge_text);
                        draw_text(
                            ctx,
                            font,
                            &badge_text,
                            badge_x,
                            by + (bh - bth) / 2.0,
                            color_to_cg(theme.muted_fg),
                        );
                    }
                }
            }
        }
    }
}

unsafe fn paint_aux(
    ctx: CGContextRef,
    font: &CTFont,
    bounds: QRect,
    aux: &SectionAux,
    theme: &Theme,
    caret_visible: bool,
) {
    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    fill_rect(ctx, bx, by, bw, bh, theme.input_bg);

    match aux {
        SectionAux::Input(input) | SectionAux::Search(input) => {
            let display: &str = if input.text.is_empty() && !input.has_focus {
                input.placeholder.as_deref().unwrap_or("")
            } else {
                input.text.as_str()
            };
            let text_fg = if input.text.is_empty() && !input.has_focus {
                theme.muted_fg
            } else {
                theme.foreground
            };
            let (_, th) = measure_text(font, display);
            draw_text(
                ctx,
                font,
                display,
                bx + 4.0,
                by + (bh - th) / 2.0,
                color_to_cg(text_fg),
            );

            // Caret as a thin vertical bar at the caret column.
            // Painted only when the InlineInput is focused AND the
            // current blink phase is "on" — the run-loop blink timer
            // toggles `caret_visible` ~530 ms (#188).
            if input.has_focus && caret_visible {
                let prefix: String = input.text.chars().take(input.caret).collect();
                let (cx_off, _) = measure_text(font, &prefix);
                let caret_x = bx + 4.0 + cx_off;
                fill_rect(ctx, caret_x, by + 2.0, 1.0, bh - 4.0, theme.foreground);
            }
        }
        SectionAux::Toolbar(actions) => {
            let mut tx = bx + 4.0;
            for a in actions {
                let glyph = a.icon.fallback.as_str();
                let action_fg = if a.enabled {
                    theme.foreground
                } else {
                    theme.muted_fg
                };
                let (gw, gh) = measure_text(font, glyph);
                draw_text(
                    ctx,
                    font,
                    glyph,
                    tx,
                    by + (bh - gh) / 2.0,
                    color_to_cg(action_fg),
                );
                tx += gw + 8.0;
            }
        }
        SectionAux::Custom(_) => {
            // Host paints; we cleared the bg already.
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn paint_body(
    ctx: CGContextRef,
    font: &CTFont,
    bounds: QRect,
    body: &SectionBody,
    theme: &Theme,
    line_height: f64,
    char_width: f64,
) {
    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;
    if bw <= 0.0 || bh <= 0.0 {
        return;
    }
    // Clip to body bounds so inner primitives can't paint past the
    // section boundary.
    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(bx, by, bw, bh));

    match body {
        SectionBody::Tree(t) => {
            super::tree::draw_tree(ctx, font, bx, by, bw, bh, t, theme, line_height);
        }
        SectionBody::List(l) => {
            super::list::draw_list(ctx, font, bx, by, bw, bh, l, theme, line_height);
        }
        SectionBody::Form(f) => {
            super::form::draw_form(ctx, font, bx, by, bw, bh, f, theme, line_height);
        }
        SectionBody::Chart(c) => {
            super::chart::draw_chart(
                ctx,
                font,
                bx,
                by,
                bw,
                bh,
                c,
                theme,
                line_height,
                char_width,
                None,
                None,
            );
        }
        SectionBody::Terminal(_) | SectionBody::MessageList(_) => {
            // Lands in #43 — paint the bg only for now.
            fill_rect(ctx, bx, by, bw, bh, theme.background);
        }
        SectionBody::Text(lines) => {
            paint_text_lines(ctx, font, bx, by, bw, bh, lines, theme, line_height);
        }
        SectionBody::Empty(empty) => {
            paint_empty_body(ctx, font, bx, by, bw, bh, empty, theme, line_height);
        }
        SectionBody::Custom(_) => {
            // Host paints in body bounds.
        }
    }
    CGContextRestoreGState(ctx);
}

#[allow(clippy::too_many_arguments)]
unsafe fn paint_text_lines(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    lines: &[StyledText],
    theme: &Theme,
    line_height: f64,
) {
    fill_rect(ctx, x, y, w, h, theme.background);
    let mut row_y = y;
    for line in lines {
        if row_y + line_height > y + h {
            break;
        }
        let text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
        let (_, th) = measure_text(font, &text);
        draw_text(
            ctx,
            font,
            &text,
            x + 4.0,
            row_y + (line_height - th) / 2.0,
            color_to_cg(theme.foreground),
        );
        row_y += line_height;
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn paint_empty_body(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    empty: &EmptyBody,
    theme: &Theme,
    line_height: f64,
) {
    fill_rect(ctx, x, y, w, h, theme.background);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let mut blocks: Vec<(String, Color)> = Vec::new();
    if let Some(icon) = &empty.icon {
        blocks.push((icon.fallback.clone(), theme.foreground));
    }
    let primary: String = empty.text.spans.iter().map(|s| s.text.as_str()).collect();
    if !primary.is_empty() {
        blocks.push((primary, theme.foreground));
    }
    if let Some(hint) = &empty.hint {
        let hint_str: String = hint.spans.iter().map(|s| s.text.as_str()).collect();
        if !hint_str.is_empty() {
            blocks.push((hint_str, theme.muted_fg));
        }
    }
    if let Some(action) = &empty.action {
        let label = action
            .tooltip
            .clone()
            .unwrap_or_else(|| action.icon.fallback.clone());
        blocks.push((format!("[ {label} ]"), theme.accent_fg));
    }
    if blocks.is_empty() {
        return;
    }
    let total_h = blocks.len() as f64 * line_height;
    let mut block_y = y + (h - total_h).max(0.0) / 2.0;
    for (text, color) in &blocks {
        let (tw, th) = measure_text(font, text);
        let block_x = x + (w - tw).max(0.0) / 2.0;
        draw_text(
            ctx,
            font,
            text,
            block_x,
            block_y + (line_height - th) / 2.0,
            color_to_cg(*color),
        );
        block_y += line_height;
    }
}

unsafe fn paint_section_scrollbar(
    ctx: CGContextRef,
    gutter: QRect,
    thumb_bounds: Option<QRect>,
    theme: &Theme,
) {
    let bx = gutter.x as f64;
    let by = gutter.y as f64;
    let bw = gutter.width as f64;
    let bh = gutter.height as f64;

    fill_rect(ctx, bx, by, bw, bh, with_alpha(theme.scrollbar_track, 0.5));

    let (ty, th) = match thumb_bounds {
        Some(t) => (t.y as f64, (t.height as f64).max(1.0)),
        None => (by, (bh * 0.2).max(20.0).min(bh)),
    };
    fill_rect(ctx, bx, ty, bw, th, with_alpha(theme.scrollbar_thumb, 0.9));
}

unsafe fn paint_panel_scrollbar(
    ctx: CGContextRef,
    bounds: QRect,
    scroll: f32,
    total: f32,
    theme: &Theme,
) {
    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;
    if bh <= 0.0 || total <= 0.0 {
        return;
    }

    fill_rect(ctx, bx, by, bw, bh, theme.scrollbar_track);

    let visible_frac = (bh / total as f64).min(1.0);
    let scroll_frac = if total as f64 > bh {
        scroll as f64 / (total as f64 - bh)
    } else {
        0.0
    };
    let thumb_h = (bh * visible_frac).max(20.0);
    let thumb_track = (bh - thumb_h).max(0.0);
    let thumb_y = by + thumb_track * scroll_frac;
    fill_rect(ctx, bx, thumb_y, bw, thumb_h, theme.scrollbar_thumb);
}

fn with_alpha(c: Color, alpha: f64) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: (255.0 * alpha).round().clamp(0.0, 255.0) as u8,
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
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::multi_section_view::{
        MultiSectionViewHit, ScrollMode, Section, SectionHeader, SectionSize,
    };
    use crate::primitives::tree::{TreeRow, TreeView};
    use crate::theme::Theme;
    use crate::types::{Decoration, SelectionMode, StyledText, TreeStyle, WidgetId};
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 320;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn leaf(idx: u16, label: &str) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn tree_section(name: &str, n: usize) -> Section {
        Section {
            id: name.into(),
            header: SectionHeader {
                icon: None,
                title: StyledText::plain(name),
                badge: None,
                actions: vec![],
                show_chevron: true,
            },
            body: SectionBody::Tree(TreeView {
                id: WidgetId::new(format!("tree:{}", name)),
                rows: (0..n)
                    .map(|i| leaf(i as u16, &format!("{}-{}", name, i)))
                    .collect(),
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: TreeStyle::default(),
                has_focus: false,
            }),
            aux: None,
            size: SectionSize::EqualShare,
            collapsed: false,
            min_size: None,
            max_size: None,
        }
    }

    fn two_section_view() -> MultiSectionView {
        MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![tree_section("alpha", 5), tree_section("beta", 3)],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        }
    }

    fn paint_via_backend(view: &MultiSectionView) -> (BitmapSurface, MultiSectionViewLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_multi_section_view(QRect::new(0.0, 0.0, W as f32, H as f32), view);
            *layout.borrow_mut() =
                Some(b.msv_layout(QRect::new(0.0, 0.0, W as f32, H as f32), view));
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn header_strip_paints_header_bg() {
        let view = two_section_view();
        let (surface, layout) = paint_via_backend(&view);
        let theme = Theme::default();
        let hdr = layout.sections[0].header_bounds;
        // Probe near right edge of the first header (past the chevron
        // and title glyphs).
        let px = (hdr.x + hdr.width - 4.0) as u32;
        let py = (hdr.y + hdr.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
        );
    }

    #[test]
    fn two_sections_stack_vertically_without_overlap() {
        let view = two_section_view();
        let (_surface, layout) = paint_via_backend(&view);
        let s0 = &layout.sections[0];
        let s1 = &layout.sections[1];
        let s0_bottom = s0.body_bounds.y + s0.body_bounds.height;
        assert!(
            s1.header_bounds.y >= s0_bottom - 0.5,
            "section 1 must stack below section 0; s0_bottom={}, s1_header_y={}",
            s0_bottom,
            s1.header_bounds.y,
        );
    }

    #[test]
    fn hit_test_resolves_header_click_to_section() {
        let view = two_section_view();
        let (_surface, layout) = paint_via_backend(&view);
        let hdr = layout.sections[0].header_bounds;
        let cx = hdr.x + hdr.width * 0.5;
        let cy = hdr.y + hdr.height * 0.5;
        let hit = layout.hit_test(cx, cy);
        // Header click on section 0 should resolve to a Header hit
        // carrying section 0.
        assert!(
            matches!(hit, MultiSectionViewHit::Header { section: 0, .. }),
            "header click hit was {:?}",
            hit,
        );
    }

    #[test]
    fn collapsed_section_zero_body_height() {
        let mut view = two_section_view();
        view.sections[0].collapsed = true;
        let (_surface, layout) = paint_via_backend(&view);
        let s0 = &layout.sections[0];
        assert_eq!(
            s0.body_bounds.height, 0.0,
            "collapsed section must report zero body height",
        );
    }

    #[test]
    fn metrics_match_gtk_convention() {
        let m = mac_msv_metrics(16.0, false);
        // line_height * 1.4 = 22.4, header_size matches the convention.
        assert!((m.header_size - 22.4).abs() < 0.01);
        assert_eq!(m.scrollbar_size, 8.0);
        assert_eq!(m.divider_size, 0.0);
        let m_resize = mac_msv_metrics(16.0, true);
        assert_eq!(m_resize.divider_size, 1.0);
    }

    // ── #188 InlineInput caret-blink ─────────────────────────────────

    use crate::primitives::multi_section_view::InlineInput;

    fn search_section() -> MultiSectionView {
        MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![Section {
                id: "search".into(),
                header: SectionHeader {
                    icon: None,
                    title: StyledText::plain("Search"),
                    badge: None,
                    actions: vec![],
                    show_chevron: true,
                },
                aux: Some(SectionAux::Search(InlineInput {
                    id: WidgetId::new("query"),
                    text: String::new(), // empty so the caret sits at x=4
                    caret: 0,
                    placeholder: None,
                    has_focus: true,
                })),
                body: SectionBody::Tree(TreeView {
                    id: WidgetId::new("tree:search"),
                    rows: vec![],
                    selection_mode: SelectionMode::Single,
                    selected_path: None,
                    scroll_offset: 0,
                    style: TreeStyle::default(),
                    has_focus: false,
                }),
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: false,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        }
    }

    fn paint_with_caret_phase(
        view: &MultiSectionView,
        caret_visible: bool,
    ) -> (BitmapSurface, MultiSectionViewLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_caret_visible(caret_visible);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_multi_section_view(QRect::new(0.0, 0.0, W as f32, H as f32), view);
            *layout.borrow_mut() =
                Some(b.msv_layout(QRect::new(0.0, 0.0, W as f32, H as f32), view));
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// X coordinate the caret bar paints at when `text` is empty: a
    /// 1-pixel-wide stroke at `aux.x + 4.0`. Mirrors the constant in
    /// `paint_aux`'s SectionAux::Input branch.
    fn caret_pixel(aux: crate::event::Rect) -> (u32, u32) {
        let x = (aux.x + 4.0) as u32;
        // Probe vertically inside the caret stroke (+2..+bh-2 band).
        let y = (aux.y + aux.height / 2.0) as u32;
        (x, y)
    }

    #[test]
    fn caret_visible_true_paints_foreground_at_caret_position() {
        let view = search_section();
        let (surface, layout) = paint_with_caret_phase(&view, true);
        let theme = Theme::default();
        let aux = layout.sections[0].aux_bounds.expect("aux bounds present");
        let (px, py) = caret_pixel(aux);
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.foreground.r, theme.foreground.g, theme.foreground.b),
            "caret_visible=true should paint theme.foreground at the caret column",
        );
    }

    #[test]
    fn caret_visible_false_leaves_caret_column_as_input_bg() {
        // The blink "off" phase — the caret bar should be skipped, so
        // the pixel at the caret column is the input row's bg
        // (theme.input_bg), not theme.foreground.
        let view = search_section();
        let (surface, layout) = paint_with_caret_phase(&view, false);
        let theme = Theme::default();
        let aux = layout.sections[0].aux_bounds.expect("aux bounds present");
        let (px, py) = caret_pixel(aux);
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.input_bg.r, theme.input_bg.g, theme.input_bg.b),
            "caret_visible=false should leave the caret column blank (input_bg)",
        );
    }

    #[test]
    fn unfocused_input_skips_caret_regardless_of_phase() {
        // has_focus=false → caret never paints, even if caret_visible
        // happens to be true. Documents the existing precondition.
        let mut view = search_section();
        if let Some(SectionAux::Search(ref mut input)) = view.sections[0].aux {
            input.has_focus = false;
        }
        let (surface, layout) = paint_with_caret_phase(&view, true);
        let theme = Theme::default();
        let aux = layout.sections[0].aux_bounds.expect("aux bounds present");
        let (px, py) = caret_pixel(aux);
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.input_bg.r, theme.input_bg.g, theme.input_bg.b),
            "unfocused input should never paint the caret bar",
        );
    }
}
