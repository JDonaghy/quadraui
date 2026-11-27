//! Direct2D / DirectWrite entry points for [`crate::MultiSectionView`]:
//! paint it onto an `ID2D1RenderTarget` through the widget's own pixel
//! painter, and resolve the chrome layout that paint uses for
//! hit-testing.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitive::{LayoutCtx, Primitive};
use crate::primitives::layout_metrics::TextMeasure;
use crate::primitives::multi_section_view::{
    msv_layout, msv_metrics, MsvLayoutMetrics, MultiSectionView, MultiSectionViewLayout,
};
use crate::theme::Theme;

/// The pixel metrics a view lays out with at `line_height` — see
/// [`msv_metrics`].
pub fn win_msv_metrics(line_height: f32, allow_resize: bool) -> MsvLayoutMetrics {
    msv_metrics(line_height as f64, allow_resize)
}

/// The chrome layout [`draw_multi_section_view`] paints `view` with in
/// `bounds` at `line_height`, for hit-testing what it painted.
pub fn win_msv_layout(
    view: &MultiSectionView,
    bounds: Rect,
    line_height: f32,
) -> MultiSectionViewLayout {
    msv_layout(view, bounds, line_height as f64)
}

/// Paint `view` into `rect` (DIPs) on `target` in the default theme; each
/// section body paints through its own widget. `caret_visible` is the
/// blink phase of a focused aux input's caret.
pub fn draw_multi_section_view(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    view: &MultiSectionView,
    line_height: f32,
    char_width: f32,
    caret_visible: bool,
) {
    let theme = Theme::default();
    let style = crate::Style::default();
    let measurer = |text: &str, _role: crate::FontRole| dwrite.width_of(text);
    let ctx = LayoutCtx::new(line_height, char_width, false)
        .with_measurer(&measurer)
        .with_style(&style);
    let view_layout = Primitive::layout(view, &ctx, rect);
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::multi_section_view::native_surface_paint::paint(
        view,
        &view_layout,
        &mut surface,
        &crate::PaintEnv::new(&theme, &style),
        caret_visible,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::multi_section_view::{Axis, SectionBody};
    use crate::primitives::multi_section_view::{
        InlineInput, MultiSectionViewHit, ScrollMode, ScrollbarHit, Section, SectionAux,
        SectionHeader, SectionSize,
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

    /// Paints with the caret phase on, same as [`paint_via_with_caret`]
    /// called with `true`; most tests here don't exercise
    /// `SectionAux::Input`/`Search` caret painting.
    fn paint_via(view: &MultiSectionView) -> (HeadlessSurface, MultiSectionViewLayout) {
        paint_via_with_caret(view, true)
    }

    /// Same as [`paint_via`], with an explicit caret-blink phase
    /// threaded into the aux-row text-input caret — mirrors
    /// `gtk::multi_section_view::tests::paint_then_layout_with_caret`.
    fn paint_via_with_caret(
        view: &MultiSectionView,
        caret_visible: bool,
    ) -> (HeadlessSurface, MultiSectionViewLayout) {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = surface
            .paint(|target| {
                draw_multi_section_view(
                    target,
                    &dwrite,
                    rect,
                    view,
                    LINE_HEIGHT,
                    8.0,
                    caret_visible,
                );
            })
            .map(|_| win_msv_layout(view, rect, LINE_HEIGHT))
            .expect("paint msv");
        (surface, layout)
    }

    /// Single-section view with a focused, empty `Search` aux row — the
    /// caret sits at `aux.x + 4` (text is empty, `caret: 0`), so a
    /// single pixel probe distinguishes "caret painted" from "caret
    /// skipped" without measuring glyph widths. Mirrors
    /// `gtk::multi_section_view::tests::search_section`.
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
        }
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

    /// Panel-level (`WholePanel`) scrollbar: the painted thumb pixel
    /// matches `theme.scrollbar_thumb`, the track pixel below it matches
    /// `theme.scrollbar_track`, and hit-testing the painted thumb pixel
    /// agrees with the layout. Win-side mirror of GTK's
    /// `gtk_panel_scrollbar_thumb_paints_at_layout_position`.
    #[test]
    fn panel_scrollbar_thumb_paints_at_layout_position() {
        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![tree_section("alpha", 11), tree_section("beta", 11)],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: true,
            allow_collapse: true,
            scroll_mode: ScrollMode::WholePanel,
            has_focus: true,
            panel_scroll: 0.0,
        };
        let (surface, layout) = paint_via(&view);
        let theme = Theme::default();

        let panel_sb = layout
            .panel_scrollbar
            .expect("two 11-row sections in a 320px-tall canvas should overflow");
        let thumb = layout
            .panel_scrollbar_thumb
            .expect("panel_scrollbar present implies panel_scrollbar_thumb present");
        assert!(
            thumb.height < panel_sb.height - 4.0,
            "test assumes the thumb doesn't fill the whole track (thumb={thumb:?}, \
             track={panel_sb:?})"
        );

        let px = (panel_sb.x + panel_sb.width / 2.0) as u32;

        // A pixel well inside the painted thumb is the thumb colour.
        let thumb_py = (thumb.y + 3.0) as u32;
        let thumb_c = surface.pixel_at(px, thumb_py);
        assert_eq!(
            (thumb_c.r, thumb_c.g, thumb_c.b),
            (
                theme.scrollbar_thumb.r,
                theme.scrollbar_thumb.g,
                theme.scrollbar_thumb.b
            ),
            "pixel inside panel_scrollbar_thumb should be the thumb colour"
        );

        // A pixel in the track below the thumb is the track colour.
        let track_py = (panel_sb.y + panel_sb.height - 3.0) as u32;
        let track_c = surface.pixel_at(px, track_py);
        assert_eq!(
            (track_c.r, track_c.g, track_c.b),
            (
                theme.scrollbar_track.r,
                theme.scrollbar_track.g,
                theme.scrollbar_track.b
            ),
            "pixel in the track below the thumb should be the track colour"
        );

        // hit_test at the sampled thumb pixel agrees with paint.
        match layout.hit_test(px as f32 + 0.5, thumb_py as f32 + 0.5) {
            MultiSectionViewHit::PanelScrollbar {
                kind: ScrollbarHit::Thumb,
            } => {}
            other => panic!(
                "hit at painted panel-thumb pixel ({}, {}) returned {:?}",
                px, thumb_py, other
            ),
        }
    }

    #[test]
    fn focused_input_paints_caret_bar() {
        let view = search_section();
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

    // ── Caret blink threading ────────────────────────────────────────────
    //
    // `caret_visible` flows from `WinBackend::caret_visible`
    // (`crate::caret_blink::CaretBlinkState`) through
    // `draw_multi_section_view`/`paint_aux` into
    // `native_surface_paint::paint_aux`. These tests pin the phase at the
    // paint-function boundary (bypassing the timer entirely) and assert
    // the single caret pixel flips between `theme.foreground` and
    // `theme.input_bg` — mirrors `gtk::multi_section_view::tests`'
    // `gtk_caret_visible_true_paints_foreground_at_caret_position`/
    // `gtk_caret_visible_false_leaves_caret_column_as_input_bg`/
    // `gtk_unfocused_input_skips_caret_regardless_of_phase` tests.

    /// Pixel the empty, focused `search_section` input's caret paints
    /// at: `aux.x + 4` (text is empty so there's no glyph run before
    /// it), vertically centered in the aux row.
    fn caret_pixel(layout: &MultiSectionViewLayout) -> (u32, u32) {
        let aux = layout.sections[0]
            .aux_bounds
            .expect("search_section always has an aux row");
        ((aux.x + 4.0) as u32, (aux.y + aux.height / 2.0) as u32)
    }

    #[test]
    fn win_caret_visible_true_paints_foreground_at_caret_position() {
        let view = search_section();
        let (surface, layout) = paint_via_with_caret(&view, true);
        let (x, y) = caret_pixel(&layout);
        let fg = Theme::default().foreground;
        let c = surface.pixel_at(x, y);
        assert_eq!(
            (c.r, c.g, c.b),
            (fg.r, fg.g, fg.b),
            "caret_visible=true must paint the caret at ({x}, {y})",
        );
    }

    #[test]
    fn win_caret_visible_false_leaves_caret_column_as_input_bg() {
        let view = search_section();
        let (surface, layout) = paint_via_with_caret(&view, false);
        let (x, y) = caret_pixel(&layout);
        let input_bg = Theme::default().input_bg;
        let c = surface.pixel_at(x, y);
        assert_eq!(
            (c.r, c.g, c.b),
            (input_bg.r, input_bg.g, input_bg.b),
            "caret_visible=false must skip the caret fill at ({x}, {y})",
        );
    }

    #[test]
    fn win_unfocused_input_skips_caret_regardless_of_phase() {
        let mut view = search_section();
        if let Some(SectionAux::Search(input)) = view.sections[0].aux.as_mut() {
            input.has_focus = false;
        } else {
            panic!("search_section must build a Search aux");
        }
        let (surface, layout) = paint_via_with_caret(&view, true);
        let (x, y) = caret_pixel(&layout);
        let input_bg = Theme::default().input_bg;
        let c = surface.pixel_at(x, y);
        assert_eq!(
            (c.r, c.g, c.b),
            (input_bg.r, input_bg.g, input_bg.b),
            "an unfocused input must never paint a caret, even with caret_visible=true",
        );
    }

    /// `win_msv_layout`'s body measurement for a `MessageList` section
    /// must agree with [`crate::primitives::multi_section_view::msv_body_measure`],
    /// ensuring a `SectionSize::Content` section's `resolved_size`
    /// (`header_size + body content_size`) matches the shared function.
    #[test]
    fn msv_body_measure_message_list_matches_shared_layout_metrics() {
        use crate::primitives::message_list::{MessageList, MessageRow};
        use crate::types::Color;

        let body = SectionBody::MessageList(MessageList {
            id: WidgetId::new("messages"),
            rows: vec![
                MessageRow::new("line one\nline two", Color::rgb(255, 255, 255), 0.0),
                MessageRow::new("single line", Color::rgb(255, 255, 255), 0.0),
            ],
            scroll_top: 0,
        });

        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![Section {
                id: "messages".into(),
                header: SectionHeader {
                    icon: None,
                    title: StyledText::plain("Messages"),
                    badge: None,
                    actions: vec![],
                    show_chevron: true,
                },
                body,
                aux: None,
                size: SectionSize::Content,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };

        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_msv_layout(&view, rect, LINE_HEIGHT);

        let shared_measure = crate::primitives::multi_section_view::msv_body_measure(
            &view.sections[0].body,
            &view.sections[0].aux,
            LINE_HEIGHT as f64,
        );
        let metrics = win_msv_metrics(LINE_HEIGHT, view.allow_resize);
        let expected_resolved = metrics.header_size + shared_measure.content_size;

        assert!(
            shared_measure.content_size > 0.0,
            "MessageList section must measure non-zero height, got {}",
            shared_measure.content_size
        );
        assert!(
            (layout.sections[0].resolved_size - expected_resolved).abs() < 0.01,
            "win_msv_layout resolved_size {} did not match \
             header_size + layout_metrics::msv_body_measure content_size {} \
             (header_size={}, content_size={})",
            layout.sections[0].resolved_size,
            expected_resolved,
            metrics.header_size,
            shared_measure.content_size,
        );
    }

    /// `SectionBody::MessageList` is painted through
    /// `super::message_list::draw_message_list`, which records a text run
    /// for each row, matching the `inventory().text_runs()` contract.
    #[test]
    fn message_list_section_paints_row_text() {
        use crate::primitives::message_list::{MessageList, MessageRow};
        use crate::types::Color;

        let body = SectionBody::MessageList(MessageList {
            id: WidgetId::new("messages"),
            rows: vec![MessageRow::new(
                "hello from win msv",
                Color::rgb(255, 255, 255),
                0.0,
            )],
            scroll_top: 0,
        });
        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![Section {
                id: "messages".into(),
                header: SectionHeader {
                    icon: None,
                    title: StyledText::plain("Messages"),
                    badge: None,
                    actions: vec![],
                    show_chevron: true,
                },
                body,
                aux: None,
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };

        let previous = crate::testing::install_text_run_sink();
        let (_surface, _layout) = paint_via(&view);
        let runs = crate::testing::take_text_run_sink(previous);

        assert!(
            runs.iter().any(|r| r.text.contains("hello from win msv")),
            "expected a text run for the MessageList row, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
    }

    /// `SectionBody::Terminal` is painted through
    /// `crate::primitives::terminal::paint`, which records a text run
    /// for each non-blank cell.
    #[test]
    fn terminal_section_paints_cell_glyphs() {
        use crate::primitives::terminal::{Terminal, TerminalCell, TerminalCursorShape};
        use crate::types::Color;

        fn cell(ch: char) -> TerminalCell {
            TerminalCell {
                text: ch.to_string(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                selected: false,
                is_cursor: false,
                is_find_match: false,
                is_find_active: false,
                cursor_shape: TerminalCursorShape::Block,
                cursor_blinking: false,
            }
        }

        let body = SectionBody::Terminal(Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('Q'), cell('X')]],
            scrollbar: None,
        });
        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![Section {
                id: "term".into(),
                header: SectionHeader {
                    icon: None,
                    title: StyledText::plain("Terminal"),
                    badge: None,
                    actions: vec![],
                    show_chevron: true,
                },
                body,
                aux: None,
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };

        let previous = crate::testing::install_text_run_sink();
        let (_surface, _layout) = paint_via(&view);
        let runs = crate::testing::take_text_run_sink(previous);

        assert!(
            runs.iter().any(|r| r.text.contains('Q')),
            "expected a text run for the Terminal cell glyphs, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
    }
}
