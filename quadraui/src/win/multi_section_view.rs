//! Direct2D / DirectWrite rasteriser for [`crate::MultiSectionView`]
//! (issue #27).
//!
//! Paints the full chrome (per-section headers, optional aux rows,
//! per-section scrollbars, optional dividers) onto an `ID2D1RenderTarget`
//! and dispatches each section's body to the appropriate quadraui body
//! rasteriser (`super::tree::draw_tree`, `super::list::draw_list`,
//! `super::form::draw_form`, `super::chart::draw_chart`) using the body
//! bounds returned by the primitive's [`crate::MultiSectionView::layout`].
//!
//! Mirrors [`crate::macos::multi_section_view`] in shape (the closest
//! existing pixel backend: no frame-scope requirement for layout, real
//! font measurement rather than TUI's cell grid): [`win_msv_metrics`]
//! computes the layout metrics for a given `line_height`,
//! [`win_msv_layout`] returns the resolved chrome layout, and
//! [`draw_multi_section_view`] consumes the same layout for paint.
//! `WinBackend::msv_layout` calls [`win_msv_layout`] directly so paint
//! and click share one source of truth (see `super::backend`'s
//! `msv_layout`/`msv_metrics` methods).
//!
//! Vertical-only in v1 (per #294 / D-003 in
//! `quadraui/docs/DECISIONS.md`); horizontal sections fall through to a
//! no-op, same as the GTK/macOS twins.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod multi_section_view;` and
//! `backend.rs`'s module docs for why the rest of this repo's
//! `--features win` compile gate stays meaningful without a Windows
//! host. Colours come from `Theme::default()` rather than a live
//! `WinBackend` theme field, same as every other rasteriser in this
//! module (see `win::status_bar`'s doc for why).
//!
//! # Scope omissions (follow-up, matches `crate::macos::multi_section_view`)
//!
//! - **Terminal / MessageList section bodies** — `WinBackend::draw_terminal`
//!   and `draw_message_list` are still `todo!()` stubs (see
//!   `super::backend`'s module doc); `SectionBody::Terminal` /
//!   `SectionBody::MessageList` paint the background only for now.
//! - **Custom-icon empty bodies** — the `EmptyBody::action` button is
//!   rendered as plain centred text, no clickable button chrome.
//! - **Caret blink** — `WinBackend` has no caret-blink timer
//!   infrastructure yet (unlike `macos::caret_blink`), so the
//!   `SectionAux::Input`/`Search` caret paints unconditionally whenever
//!   the input `has_focus`, matching `gtk::multi_section_view`'s
//!   simpler (non-blinking) convention rather than macOS's blink-aware
//!   one.
//! - **Translucent overlays are CPU-premixed, not native D2D alpha
//!   blending** — every other rasteriser in this module premixes
//!   translucent fills against a known base colour via
//!   [`super::text::blend`] rather than painting a partially-transparent
//!   brush directly (see that function's doc for why); the per-section
//!   and standalone scrollbar tracks/thumbs here follow the same
//!   convention instead of macOS/GTK's real alpha-blended overlay.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{blend, fill_rect, pop_clip, push_clip, DWrite};
use crate::event::Rect;
use crate::primitives::multi_section_view::{
    Axis, EmptyBody, LayoutMetrics, MultiSectionView, MultiSectionViewLayout, SectionAux,
    SectionBody, SectionHeader, SectionMeasure,
};
use crate::theme::Theme;
use crate::types::{Color, StyledText};

/// Compute the Win-GUI metrics for a `MultiSectionView` from a
/// `line_height`. Hosts call this and the primitive's `layout()` with
/// the same metrics so paint and click resolve to the same bounds.
/// Matches `mac_msv_metrics` / `gtk::multi_section_view::metrics_for`'s
/// convention exactly.
pub fn win_msv_metrics(line_height: f32, allow_resize: bool) -> LayoutMetrics {
    LayoutMetrics {
        header_size: line_height * 1.4,
        divider_size: if allow_resize { 1.0 } else { 0.0 },
        // Matches GTK/macOS: 8 DIPs gives a visible track against dark
        // sidebars.
        scrollbar_size: 8.0,
        // Direct2D paints at sub-pixel precision; no quantization.
        cell_quantum: 0.0,
    }
}

/// Compute the layout for a `MultiSectionView` using the Win-GUI
/// metrics the rasteriser would use itself. Hosts call this to drive
/// hit-testing without re-computing — paint and click share this single
/// layout per frame. No `DWrite` handle needed: body measurement below
/// is estimate-based (row counts × `line_height`), not real text
/// measurement, mirroring `mac_msv_metrics`'s twin `body_measure`.
pub fn win_msv_layout(
    view: &MultiSectionView,
    bounds: Rect,
    line_height: f32,
) -> MultiSectionViewLayout {
    let metrics = win_msv_metrics(line_height, view.allow_resize);
    view.layout(bounds, metrics, |i| {
        body_measure(&view.sections[i].body, &view.sections[i].aux, line_height)
    })
}

fn body_measure(body: &SectionBody, aux: &Option<SectionAux>, line_height: f32) -> SectionMeasure {
    let item_h = (line_height * 1.4).round();
    let aux_size = if aux.is_some() { item_h } else { 0.0 };
    let content_size = match body {
        SectionBody::Tree(t) => {
            let header_h = (line_height * 1.2).round();
            let mut total = 0.0_f32;
            for row in &t.rows {
                let is_header = matches!(row.decoration, crate::types::Decoration::Header);
                total += if is_header { header_h } else { item_h };
            }
            total
        }
        SectionBody::List(l) => {
            let title_h = if l.title.is_some() { line_height } else { 0.0 };
            title_h + l.items.len() as f32 * item_h
        }
        SectionBody::Form(f) => f.fields.len() as f32 * item_h,
        SectionBody::Chart(c) => {
            if matches!(c.kind, crate::primitives::chart::ChartKind::Sparkline) {
                line_height
            } else {
                item_h * 8.0
            }
        }
        SectionBody::MessageList(_) | SectionBody::Terminal(_) => 0.0,
        SectionBody::Text(lines) => lines.len() as f32 * line_height,
        SectionBody::Empty(_) => item_h * 4.0,
        SectionBody::Custom(_) => 0.0,
    };
    SectionMeasure {
        content_size,
        aux_size,
    }
}

/// Draw a [`MultiSectionView`] into `rect` (DIPs) on `target`.
///
/// # Visual contract
///
/// See [`crate::macos::multi_section_view::draw_multi_section_view`]'s
/// doc — this rasteriser matches its chrome layout (header/aux/body/
/// scrollbar/divider) byte-for-byte modulo the scope omissions listed
/// in this module's doc.
pub fn draw_multi_section_view(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    view: &MultiSectionView,
    line_height: f32,
    char_width: f32,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 || view.axis == Axis::Horizontal {
        return;
    }

    let theme = Theme::default();
    push_clip(target, rect);
    let _ = fill_rect(target, rect, theme.background);

    let view_layout = win_msv_layout(view, rect, line_height);

    for s_layout in &view_layout.sections {
        let section = &view.sections[s_layout.section_idx];

        paint_header(
            target,
            dwrite,
            s_layout.header_bounds,
            &section.header,
            section.collapsed,
            &theme,
        );

        if !s_layout.collapsed {
            if let (Some(aux), Some(aux_b)) = (&section.aux, s_layout.aux_bounds) {
                paint_aux(target, dwrite, aux_b, aux, &theme);
            }

            paint_body(
                target,
                dwrite,
                s_layout.body_bounds,
                &section.body,
                &theme,
                line_height,
                char_width,
            );

            if let Some(sb_b) = s_layout.scrollbar_bounds {
                paint_section_scrollbar(target, sb_b, s_layout.thumb_bounds, &theme);
            }
        }
    }

    if view.allow_resize {
        for d in &view_layout.dividers {
            let _ = fill_rect(target, d.bounds, theme.separator);
        }
    }

    pop_clip(target);

    // Panel-level scrollbar (WholePanel mode) painted outside the
    // panel clip so it isn't itself clipped — matches
    // `mac_msv`/`gtk_msv`'s posture.
    if let Some(panel_sb) = view_layout.panel_scrollbar {
        let total_content: f32 = view_layout.sections.iter().map(|s| s.resolved_size).sum();
        paint_panel_scrollbar(target, panel_sb, view.panel_scroll, total_content, &theme);
    }
}

fn paint_header(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    header: &SectionHeader,
    collapsed: bool,
    theme: &Theme,
) {
    let _ = fill_rect(target, bounds, theme.header_bg);

    let mut left_x = bounds.x + 4.0;
    let row_text_y = |th: f32| (bounds.y + (bounds.height - th) * 0.4).round();

    if header.show_chevron {
        let chevron = if collapsed { "\u{25B8}" } else { "\u{25BE}" };
        let (cw, ch) = dwrite.measure_text(chevron).unwrap_or((0.0, 0.0));
        let _ = dwrite.draw_text(
            target,
            chevron,
            Rect::new(left_x, row_text_y(ch), cw, ch),
            theme.header_fg,
        );
        left_x += cw + 4.0;
    }

    // Right-aligned actions, right-to-left.
    let mut right_x = bounds.x + bounds.width - 4.0;
    for action in header.actions.iter().rev() {
        let glyph = action.icon.fallback.as_str();
        let (gw, gh) = dwrite.measure_text(glyph).unwrap_or((0.0, 0.0));
        right_x -= gw;
        if right_x < left_x {
            break;
        }
        let action_fg = if action.enabled {
            theme.header_fg
        } else {
            theme.muted_fg
        };
        let _ = dwrite.draw_text(
            target,
            glyph,
            Rect::new(right_x, row_text_y(gh), gw, gh),
            action_fg,
        );
        right_x -= 8.0;
    }

    // Title text + badge.
    let title_text: String = header.title.spans.iter().map(|s| s.text.as_str()).collect();
    if !title_text.is_empty() {
        let (tw, th) = dwrite.measure_text(&title_text).unwrap_or((0.0, 0.0));
        let max_w = (right_x - left_x).max(0.0);
        if max_w > 0.0 {
            // Clip title to the header's title region.
            push_clip(target, Rect::new(left_x, bounds.y, max_w, bounds.height));
            let _ = dwrite.draw_text(
                target,
                &title_text,
                Rect::new(left_x, row_text_y(th), tw, th),
                theme.header_fg,
            );
            pop_clip(target);
            let after_title_x = left_x + tw.min(max_w);

            if let Some(badge) = &header.badge {
                let badge_text: String = badge.spans.iter().map(|s| s.text.as_str()).collect();
                if !badge_text.is_empty() {
                    let badge_x = after_title_x + 6.0;
                    if badge_x < right_x {
                        let (bw, bth) = dwrite.measure_text(&badge_text).unwrap_or((0.0, 0.0));
                        let _ = dwrite.draw_text(
                            target,
                            &badge_text,
                            Rect::new(badge_x, bounds.y + (bounds.height - bth) / 2.0, bw, bth),
                            theme.muted_fg,
                        );
                    }
                }
            }
        }
    }
}

fn paint_aux(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    aux: &SectionAux,
    theme: &Theme,
) {
    let _ = fill_rect(target, bounds, theme.input_bg);

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
            let (dw, dh) = dwrite.measure_text(display).unwrap_or((0.0, 0.0));
            let _ = dwrite.draw_text(
                target,
                display,
                Rect::new(
                    bounds.x + 4.0,
                    bounds.y + (bounds.height - dh) / 2.0,
                    dw,
                    dh,
                ),
                text_fg,
            );

            // Caret as a thin vertical bar at the caret column. Painted
            // whenever the input is focused — see this module's "Caret
            // blink" scope-omission note for why this doesn't gate on a
            // blink phase the way `macos::multi_section_view` does.
            if input.has_focus {
                let prefix: String = input.text.chars().take(input.caret).collect();
                let (cx_off, _) = dwrite.measure_text(&prefix).unwrap_or((0.0, 0.0));
                let caret_x = bounds.x + 4.0 + cx_off;
                let _ = fill_rect(
                    target,
                    Rect::new(caret_x, bounds.y + 2.0, 1.0, bounds.height - 4.0),
                    theme.foreground,
                );
            }
        }
        SectionAux::Toolbar(actions) => {
            let mut tx = bounds.x + 4.0;
            for a in actions {
                let glyph = a.icon.fallback.as_str();
                let action_fg = if a.enabled {
                    theme.foreground
                } else {
                    theme.muted_fg
                };
                let (gw, gh) = dwrite.measure_text(glyph).unwrap_or((0.0, 0.0));
                let _ = dwrite.draw_text(
                    target,
                    glyph,
                    Rect::new(tx, bounds.y + (bounds.height - gh) / 2.0, gw, gh),
                    action_fg,
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
fn paint_body(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    body: &SectionBody,
    theme: &Theme,
    line_height: f32,
    char_width: f32,
) {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    // Clip to body bounds so inner primitives can't paint past the
    // section boundary.
    push_clip(target, bounds);

    match body {
        SectionBody::Tree(t) => {
            let _ = super::tree::draw_tree(target, dwrite, bounds, t, line_height);
        }
        SectionBody::List(l) => {
            let _ = super::list::draw_list(target, dwrite, bounds, l, line_height);
        }
        SectionBody::Form(f) => {
            let _ = super::form::draw_form(target, dwrite, bounds, f, line_height);
        }
        SectionBody::Chart(c) => {
            let _ = super::chart::draw_chart(
                target,
                dwrite,
                bounds,
                c,
                char_width,
                line_height,
                None,
                None,
            );
        }
        SectionBody::Terminal(_) | SectionBody::MessageList(_) => {
            // Lands in a follow-up issue — paint the bg only for now.
            let _ = fill_rect(target, bounds, theme.background);
        }
        SectionBody::Text(lines) => {
            paint_text_lines(target, dwrite, bounds, lines, theme, line_height);
        }
        SectionBody::Empty(empty) => {
            paint_empty_body(target, dwrite, bounds, empty, theme, line_height);
        }
        SectionBody::Custom(_) => {
            // Host paints in body bounds.
        }
    }
    pop_clip(target);
}

fn paint_text_lines(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    lines: &[StyledText],
    theme: &Theme,
    line_height: f32,
) {
    let _ = fill_rect(target, bounds, theme.background);
    let mut row_y = bounds.y;
    for line in lines {
        if row_y + line_height > bounds.y + bounds.height {
            break;
        }
        let text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
        let (tw, th) = dwrite.measure_text(&text).unwrap_or((0.0, 0.0));
        let _ = dwrite.draw_text(
            target,
            &text,
            Rect::new(bounds.x + 4.0, row_y + (line_height - th) / 2.0, tw, th),
            theme.foreground,
        );
        row_y += line_height;
    }
}

fn paint_empty_body(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    empty: &EmptyBody,
    theme: &Theme,
    line_height: f32,
) {
    let _ = fill_rect(target, bounds, theme.background);
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
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
    let total_h = blocks.len() as f32 * line_height;
    let mut block_y = bounds.y + (bounds.height - total_h).max(0.0) / 2.0;
    for (text, color) in &blocks {
        let (tw, th) = dwrite.measure_text(text).unwrap_or((0.0, 0.0));
        let block_x = bounds.x + (bounds.width - tw).max(0.0) / 2.0;
        let _ = dwrite.draw_text(
            target,
            text,
            Rect::new(block_x, block_y + (line_height - th) / 2.0, tw, th),
            *color,
        );
        block_y += line_height;
    }
}

/// Per-section scrollbar gutter — 50%-alpha track, 90%-alpha thumb,
/// both premixed against `theme.background` via [`blend`] (see this
/// module's doc for why Direct2D fills here are opaque, not native
/// alpha blends).
fn paint_section_scrollbar(
    target: &ID2D1RenderTarget,
    gutter: Rect,
    thumb_bounds: Option<Rect>,
    theme: &Theme,
) {
    let track_color = blend(theme.background, theme.scrollbar_track, 0.5);
    let _ = fill_rect(target, gutter, track_color);

    let thumb_rect = match thumb_bounds {
        Some(t) => Rect::new(gutter.x, t.y, gutter.width, t.height.max(1.0)),
        None => Rect::new(
            gutter.x,
            gutter.y,
            gutter.width,
            (gutter.height * 0.2).max(20.0).min(gutter.height),
        ),
    };
    let thumb_color = blend(track_color, theme.scrollbar_thumb, 0.9);
    let _ = fill_rect(target, thumb_rect, thumb_color);
}

/// Panel-level scrollbar (`ScrollMode::WholePanel`) — opaque track and
/// thumb, no blending needed since it's painted outside any body clip.
fn paint_panel_scrollbar(
    target: &ID2D1RenderTarget,
    bounds: Rect,
    scroll: f32,
    total: f32,
    theme: &Theme,
) {
    if bounds.height <= 0.0 || total <= 0.0 {
        return;
    }

    let _ = fill_rect(target, bounds, theme.scrollbar_track);

    let visible_frac = (bounds.height / total).min(1.0);
    let scroll_frac = if total > bounds.height {
        scroll / (total - bounds.height)
    } else {
        0.0
    };
    let thumb_h = (bounds.height * visible_frac).max(20.0);
    let thumb_track = (bounds.height - thumb_h).max(0.0);
    let thumb_y = bounds.y + thumb_track * scroll_frac;
    let _ = fill_rect(
        target,
        Rect::new(bounds.x, thumb_y, bounds.width, thumb_h),
        theme.scrollbar_thumb,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::multi_section_view::{
        InlineInput, MultiSectionViewHit, ScrollMode, Section, SectionHeader, SectionSize,
    };
    use crate::primitives::tree::{TreeRow, TreeView};
    use crate::types::{Decoration, SelectionMode, StyledText, TreeStyle, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 240.0;
    const H: f32 = 320.0;
    const LINE_HEIGHT: f32 = 16.0;

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

    fn paint_via(view: &MultiSectionView) -> (HeadlessSurface, MultiSectionViewLayout) {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = surface
            .paint(|target| {
                draw_multi_section_view(target, &dwrite, rect, view, LINE_HEIGHT, 8.0);
            })
            .map(|_| win_msv_layout(view, rect, LINE_HEIGHT))
            .expect("paint msv");
        (surface, layout)
    }

    #[test]
    fn header_strip_paints_header_bg() {
        let view = two_section_view();
        let (surface, layout) = paint_via(&view);
        let theme = Theme::default();
        let hdr = layout.sections[0].header_bounds;
        // Probe near right edge of the first header (past chevron/title).
        let px = (hdr.x + hdr.width - 4.0) as u32;
        let py = (hdr.y + hdr.height / 2.0) as u32;
        let c = surface.pixel_at(px, py);
        assert_eq!(
            (c.r, c.g, c.b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
        );
    }

    #[test]
    fn two_sections_stack_vertically_without_overlap() {
        let view = two_section_view();
        let (_surface, layout) = paint_via(&view);
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
        let (_surface, layout) = paint_via(&view);
        let hdr = layout.sections[0].header_bounds;
        let cx = hdr.x + hdr.width * 0.5;
        let cy = hdr.y + hdr.height * 0.5;
        let hit = layout.hit_test(cx, cy);
        assert!(
            matches!(hit, MultiSectionViewHit::Header { section: 0, .. }),
            "header click hit was {:?}",
            hit,
        );
    }

    /// Paint↔click round trip at a non-zero origin — a header click AND
    /// a body click must resolve back to section 0. Regression guard for
    /// the "layout helpers must return coords in the same frame across
    /// backends" class of bug (see `macos::multi_section_view`'s
    /// analogous test).
    #[test]
    fn hit_test_resolves_header_and_body_at_nonzero_origin() {
        let view = two_section_view();
        let origin = Rect::new(7.0, 13.0, W, H);
        let layout = win_msv_layout(&view, origin, LINE_HEIGHT);

        let hdr = layout.sections[0].header_bounds;
        let cx = hdr.x + hdr.width * 0.5;
        let cy = hdr.y + hdr.height * 0.5;
        let hit = layout.hit_test(cx, cy);
        assert!(
            matches!(hit, MultiSectionViewHit::Header { section: 0, .. }),
            "header click hit was {:?}",
            hit,
        );

        let body = layout.sections[0].body_bounds;
        assert!(
            body.width > 0.0 && body.height > 0.0,
            "section 0 body must have non-zero size to round-trip a click",
        );
        let bx = body.x + body.width * 0.5;
        let by = body.y + body.height * 0.5;
        let hit = layout.hit_test(bx, by);
        assert!(
            matches!(hit, MultiSectionViewHit::Body { section: 0 }),
            "body click hit was {:?}",
            hit,
        );
    }

    #[test]
    fn collapsed_section_zero_body_height() {
        let mut view = two_section_view();
        view.sections[0].collapsed = true;
        let (_surface, layout) = paint_via(&view);
        let s0 = &layout.sections[0];
        assert_eq!(
            s0.body_bounds.height, 0.0,
            "collapsed section must report zero body height",
        );
    }

    #[test]
    fn metrics_match_gtk_macos_convention() {
        let m = win_msv_metrics(16.0, false);
        assert!((m.header_size - 22.4).abs() < 0.01);
        assert_eq!(m.scrollbar_size, 8.0);
        assert_eq!(m.divider_size, 0.0);
        let m_resize = win_msv_metrics(16.0, true);
        assert_eq!(m_resize.divider_size, 1.0);
    }

    /// No-paint layout must agree byte-for-byte with the layout used
    /// during paint — `win_msv_layout` is a pure fn, so a second call
    /// with the same inputs must produce identical bounds.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let view = two_section_view();
        let rect = Rect::new(0.0, 0.0, W, H);
        let (_surface, painted) = paint_via(&view);
        let no_paint = win_msv_layout(&view, rect, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }

    /// A section's per-section scrollbar track should paint when its
    /// tree body overflows the section's resolved height — probes a
    /// pixel inside the scrollbar gutter and checks it differs from the
    /// plain background (i.e. something painted there).
    #[test]
    fn overflowing_section_paints_a_scrollbar_track() {
        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![tree_section("alpha", 100)],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };
        let (surface, layout) = paint_via(&view);
        let theme = Theme::default();
        let sb = layout.sections[0]
            .scrollbar_bounds
            .expect("100-row tree section must overflow and get a scrollbar");
        let px = (sb.x + sb.width / 2.0) as u32;
        let py = (sb.y + 2.0) as u32;
        let c = surface.pixel_at(px, py);
        assert_ne!(
            (c.r, c.g, c.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "scrollbar gutter should paint something other than plain background",
        );
    }

    #[test]
    fn focused_input_paints_caret_bar() {
        let view = MultiSectionView {
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
                    text: String::new(),
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
        };
        let (surface, layout) = paint_via(&view);
        let theme = Theme::default();
        let aux = layout.sections[0].aux_bounds.expect("aux bounds present");
        let px = (aux.x + 4.0) as u32;
        let py = (aux.y + aux.height / 2.0) as u32;
        let c = surface.pixel_at(px, py);
        assert_eq!(
            (c.r, c.g, c.b),
            (theme.foreground.r, theme.foreground.g, theme.foreground.b),
            "focused empty input should paint the caret bar at x=aux.x+4",
        );
    }
}
