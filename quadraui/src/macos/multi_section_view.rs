//! macOS entry points for [`crate::MultiSectionView`]: paint it onto a
//! `CGContextRef` through the widget's own pixel painter, and resolve the
//! chrome layout that paint uses for hit-testing.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::event::Rect as QRect;
use crate::primitive::{LayoutCtx, Primitive};
use crate::primitives::layout_metrics::TextMeasure;
use crate::primitives::multi_section_view::{
    msv_layout, msv_metrics, MsvLayoutMetrics, MultiSectionView, MultiSectionViewLayout,
};
use crate::style::Style;
use crate::theme::Theme;

/// The pixel metrics a view lays out with at `line_height` — see
/// [`msv_metrics`].
pub fn mac_msv_metrics(line_height: f64, allow_resize: bool) -> MsvLayoutMetrics {
    msv_metrics(line_height, allow_resize)
}

/// The chrome layout [`draw_multi_section_view`] paints `view` with in
/// `bounds` at `line_height`, for hit-testing what it painted.
pub fn mac_msv_layout(
    view: &MultiSectionView,
    bounds: QRect,
    line_height: f64,
) -> MultiSectionViewLayout {
    msv_layout(view, bounds, line_height)
}

/// Paint `view` into `(x, y, w, h)` on `ctx` in `font`; each section body
/// paints through its own widget. `caret_visible` is the blink phase of
/// a focused aux input's caret.
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
    style: &Style,
    line_height: f64,
    char_width: f64,
    caret_visible: bool,
) {
    let measurer = |text: &str, _role: crate::FontRole| font.width_of(text);
    let layout_ctx = LayoutCtx::new(line_height as f32, char_width as f32, false)
        .with_measurer(&measurer)
        .with_style(style);
    let rect = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let view_layout = Primitive::layout(view, &layout_ctx, rect);
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    crate::primitives::multi_section_view::native_surface_paint::paint(
        view,
        &view_layout,
        &mut surface,
        &crate::PaintEnv::new(theme, style),
        caret_visible,
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
        Axis, EmptyBody, SectionAux, SectionBody, SectionHeader,
    };
    use crate::primitives::multi_section_view::{
        MultiSectionViewHit, ScrollMode, Section, SectionHeader, SectionSize,
    };
    use crate::primitives::tree::{TreeRow, TreeView};
    use crate::theme::Theme;
    use crate::types::StyledText;
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
        // Regression test: layout helpers must return coords in the same frame
        // across backends. `mac_msv_layout` bakes `bounds.x`/`bounds.y` into
        // every returned Rect (absolute frame). Test at a non-zero origin to
        // ensure both header and section body clicks resolve correctly.
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

    /// An overflowing `Tree` section shows one scrollbar: the section's
    /// gutter paints it, and the tree's rows run up to the gutter without
    /// a scrollbar of their own.
    #[test]
    fn tree_section_paints_one_scrollbar_in_the_section_gutter() {
        let mut view = two_section_view();
        view.sections[0] = tree_section("alpha", 50);
        let (surface, layout) = paint_via_backend(&view);
        let t = Theme::default();
        let bg = (t.background.r, t.background.g, t.background.b);
        let s0 = &layout.sections[0];
        let sb = s0.scrollbar_bounds.expect("50 rows must overflow");
        let y = (sb.y + 2.0) as u32;
        let rgb = |x: f32| {
            let (r, g, b, _) = surface.pixel(x as u32, y);
            (r, g, b)
        };
        assert_ne!(
            rgb(sb.x + sb.width / 2.0),
            bg,
            "the gutter paints the scrollbar"
        );
        let body_right = s0.body_bounds.x + s0.body_bounds.width;
        assert_eq!(
            rgb(body_right - 2.0),
            rgb(body_right - 30.0),
            "no second scrollbar at the tree's right edge"
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

    // InlineInput caret-blink tests

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
