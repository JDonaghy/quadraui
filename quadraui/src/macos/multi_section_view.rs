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
//! `quadraui/docs/decisions/DECISIONS.md`); horizontal sections fall through to
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

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::cg::*;
use super::text::measure_text;
use crate::event::Rect as QRect;
use crate::primitives::multi_section_view::{
    Axis, EmptyBody, MsvLayoutMetrics, MultiSectionView, MultiSectionViewLayout, SectionAux,
    SectionBody, SectionHeader,
};
use crate::theme::Theme;
use crate::types::StyledText;

/// Compute the macOS metrics for a `MultiSectionView` from a
/// `line_height`. Hosts call this and the primitive's `layout()`
/// with the same metrics so paint and click resolve to the same bounds.
///
/// Thin wrapper over [`crate::primitives::layout_metrics::msv_metrics`]
/// (#499) — identical math across every pixel backend.
pub fn mac_msv_metrics(line_height: f64, allow_resize: bool) -> MsvLayoutMetrics {
    crate::primitives::layout_metrics::msv_metrics(line_height, allow_resize)
}

/// Compute the layout for a `MultiSectionView` using the macOS metrics
/// the rasteriser would use itself. Hosts call this to drive hit-
/// testing without re-computing — paint and click share this single
/// layout per frame.
///
/// Thin wrapper over [`crate::primitives::layout_metrics::msv_layout`]
/// (#499). Before #499 this backend's own `body_measure` returned
/// `0.0` for `SectionBody::MessageList` (a copy-paste drift from GTK's
/// version, which measured real content height) — sharing one
/// function fixes it: MessageList sections now measure correctly on
/// macOS too. See `crate::primitives::layout_metrics::msv_body_measure`'s
/// doc and its regression test.
pub fn mac_msv_layout(
    view: &MultiSectionView,
    bounds: QRect,
    line_height: f64,
) -> MultiSectionViewLayout {
    crate::primitives::layout_metrics::msv_layout(view, bounds, line_height)
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
    CGContextClipToRect(ctx, rect(x, y, w, h));
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
    // panel clip so it isn't itself clipped. Thumb geometry comes
    // straight from `view_layout.panel_scrollbar_thumb` (computed once,
    // via `fit_thumb`, in `MultiSectionView::layout`) — see
    // `paint_panel_scrollbar`'s doc for the quadraui#820-class bug this
    // fixes: pre-#1074 this call re-derived thumb geometry from
    // `(scroll, total_content)` with a formula that disagreed with the
    // layout's own, the same bug GTK/Win already had fixed.
    if let Some(panel_sb) = view_layout.panel_scrollbar {
        paint_panel_scrollbar(ctx, panel_sb, view_layout.panel_scrollbar_thumb, theme);
    }
}

// #1074 (NativeSurface Phase 4): header/aux/text/empty/scrollbar/divider
// chrome painting moved to the shared
// [`crate::primitives::multi_section_view::native_surface_paint`] — these
// wrappers just build a [`super::surface::CgSurface`] adapter and forward.
// `paint_body` (below) stays here: `Tree`/`List` bodies still dispatch to
// this backend's own `super::tree::draw_tree`/`super::list::draw_list`,
// which take a raw `(CGContextRef, &CTFont)` pair, not `&mut dyn
// NativeSurface`.
//
// Pre-port, this rasteriser already clipped the title paint the same way
// the shared [`crate::primitives::multi_section_view::native_surface_paint::paint_header`]
// does — see that function's doc for the GTK-only drift this port fixes
// elsewhere, not here.

unsafe fn paint_header(
    ctx: CGContextRef,
    font: &CTFont,
    bounds: QRect,
    header: &SectionHeader,
    collapsed: bool,
    theme: &Theme,
) {
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_header(
        &mut surface,
        bounds,
        header,
        collapsed,
        theme,
    );
}

unsafe fn paint_aux(
    ctx: CGContextRef,
    font: &CTFont,
    bounds: QRect,
    aux: &SectionAux,
    theme: &Theme,
    caret_visible: bool,
) {
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    // `caret_visible` threads macOS's own blink-timer phase (#188)
    // straight through to the shared implementation, preserving this
    // backend's blink-aware caret exactly.
    crate::primitives::multi_section_view::native_surface_paint::paint_aux(
        &mut surface,
        bounds,
        aux,
        theme,
        caret_visible,
    );
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
    CGContextClipToRect(ctx, rect(bx, by, bw, bh));

    match body {
        SectionBody::Tree(t) => {
            // #804 fixed the nerd-fonts no-op in `macos::tree::draw_tree`
            // itself; wiring `nerd_fonts_enabled` through
            // `draw_multi_section_view`'s own call chain (it isn't a
            // parameter here yet, unlike TUI/GTK's MSV) is separate,
            // unstarted scope — passing `false` preserves today's
            // fallback-only behaviour for tree bodies nested in an MSV.
            super::tree::draw_tree(ctx, font, bx, by, bw, bh, t, theme, line_height, false);
        }
        SectionBody::List(l) => {
            super::list::draw_list(ctx, font, bx, by, bw, bh, l, theme, line_height);
        }
        SectionBody::Form(f) => {
            draw_form_body(ctx, font, bx, by, bw, bh, f, theme, line_height);
        }
        SectionBody::Chart(c) => {
            // #810: painting moved to the shared
            // `crate::primitives::chart::paint`; this raw
            // `(CGContextRef, &CTFont)` call site (no live `MacBackend`
            // on hand) reuses the shared `super::surface::CgSurface`
            // adapter (#1072).
            let chart_layout =
                super::chart::mac_chart_layout(c, bx, by, bw, bh, line_height, char_width);
            let mut surface = super::surface::CgSurface {
                ctx,
                font: Some(font),
            };
            crate::primitives::chart::paint(c, &chart_layout, &mut surface, theme, None, None);
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

/// Paint an embedded [`crate::Form`] section body. #808: field-kind
/// painting goes through the shared [`crate::primitives::form::paint`]
/// via [`super::surface::CgSurface`] (this call site has only a raw
/// `CGContextRef`, not a live [`super::MacBackend`]) — `FieldKind::
/// Toolbar` is painted separately below, same as
/// `MacBackend::draw_form`, for the same reason (see that fn's doc).
#[allow(clippy::too_many_arguments)]
unsafe fn draw_form_body(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    form: &crate::Form,
    theme: &Theme,
    line_height: f64,
) {
    let area = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let flayout = super::form::mac_form_layout(form, area, line_height, font);
    let origin = crate::Point::new(x as f32, y as f32);
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    crate::primitives::form::paint(form, &flayout, &mut surface, theme, origin);

    for vf in &flayout.visible_fields {
        let Some(field) = form.fields.get(vf.field_idx) else {
            continue;
        };
        let crate::FieldKind::Toolbar(toolbar) = &field.kind else {
            continue;
        };
        let label_text: String = field.label.spans.iter().map(|s| s.text.as_str()).collect();
        let no_label = label_text.is_empty();
        let (label_w, _) = measure_text(font, &label_text);
        let row_x = x + vf.bounds.x as f64;
        let row_y = y + vf.bounds.y as f64;
        let row_w = vf.bounds.width as f64;
        let row_h = vf.bounds.height as f64;
        let toolbar_x = if no_label {
            row_x + 6.0
        } else {
            row_x + 6.0 + label_w + 12.0
        };
        let toolbar_w = row_x + row_w - toolbar_x;
        if toolbar_w > 0.0 {
            super::toolbar::draw_toolbar(
                ctx, font, toolbar_x, row_y, toolbar_w, row_h, toolbar, theme, None, None,
            );
        }
    }
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
    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_text_lines(
        &mut surface,
        bounds,
        lines,
        theme,
        line_height as f32,
    );
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
    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_empty_body(
        &mut surface,
        bounds,
        empty,
        theme,
        line_height as f32,
    );
}

/// Per-section scrollbar gutter — 50%-alpha track, 90%-alpha thumb, both
/// real alpha blends against whatever is already painted underneath.
/// [`super::surface::CgSurface::surface_fill_rect`] has honoured real
/// alpha since before #1072 (see that adapter's module doc: "no
/// fill-translucency divergence to preserve"), so routing this through
/// the shared, real-alpha
/// [`crate::primitives::multi_section_view::native_surface_paint::paint_scrollbar`]
/// is not a behaviour change here.
unsafe fn paint_section_scrollbar(
    ctx: CGContextRef,
    gutter: QRect,
    thumb_bounds: Option<QRect>,
    theme: &Theme,
) {
    let mut surface = super::surface::CgSurface { ctx, font: None };
    crate::primitives::multi_section_view::native_surface_paint::paint_scrollbar(
        &mut surface,
        gutter,
        thumb_bounds,
        theme,
    );
}

/// Panel-level scrollbar (`ScrollMode::WholePanel`) — opaque track and
/// thumb. Thumb geometry comes from `thumb_bounds` — computed once by
/// [`crate::primitives::multi_section_view::MultiSectionView::layout`]
/// (`fit_thumb`) and published as
/// [`crate::primitives::multi_section_view::MultiSectionViewLayout::panel_scrollbar_thumb`].
///
/// Pre-#1074 this function recomputed thumb size/position itself from
/// `(scroll, total_content)` — the same quadraui#820 class of bug
/// already fixed on GTK/Win: a hardcoded `20.0`-pixel minimum thumb here
/// disagreed with `panel_thumb_min`'s
/// `metrics.scrollbar_size.max(8.0)` used everywhere else, and the
/// caller's `total_content` summed only section sizes, silently
/// dropping divider strips. Consuming the layout's own `thumb_bounds`
/// closes both gaps at once. Mirrors [`paint_section_scrollbar`]'s
/// (per-section) pattern of consuming pre-computed bounds instead of
/// re-deriving them.
unsafe fn paint_panel_scrollbar(
    ctx: CGContextRef,
    bounds: QRect,
    thumb_bounds: Option<QRect>,
    theme: &Theme,
) {
    let mut surface = super::surface::CgSurface { ctx, font: None };
    crate::primitives::multi_section_view::native_surface_paint::paint_panel_scrollbar(
        &mut surface,
        bounds,
        thumb_bounds,
        theme,
    );
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
    fn hit_test_resolves_header_and_body_at_nonzero_origin() {
        // Regression for quadraui#494 / LESSONS.md "Layout helpers must
        // return coords in the same frame across backends": the sibling
        // at-origin test above only exercises (0, 0), where an origin
        // bug is invisible. `mac_msv_layout` bakes `bounds.x`/`bounds.y`
        // straight into every returned Rect (absolute frame, matching
        // the GTK/TUI twins) — call it directly (pure fn, no paint
        // needed) at a non-zero origin and round-trip both a header
        // click AND a section body click through `hit_test` (the
        // existing at-origin test only covers a header click).
        let view = two_section_view();
        let origin = QRect::new(7.0, 13.0, W as f32, H as f32);
        let layout = mac_msv_layout(&view, origin, 16.0);

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
