//! GTK entry points for [`crate::MultiSectionView`]: paint it onto a
//! Cairo [`Context`] through the widget's own pixel painter, and resolve
//! the chrome layout that paint uses for hit-testing.

use gtk4::cairo::Context;
use gtk4::pango;

use super::surface::{CairoSurface, PangoMeasure};
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
pub fn metrics_for(line_height: f64, allow_resize: bool) -> MsvLayoutMetrics {
    msv_metrics(line_height, allow_resize)
}

/// The chrome layout [`draw_multi_section_view`] paints `view` with in
/// `bounds` at `line_height`, for hit-testing what it painted.
pub fn gtk_msv_layout(
    view: &MultiSectionView,
    bounds: QRect,
    line_height: f64,
) -> MultiSectionViewLayout {
    msv_layout(view, bounds, line_height)
}

/// Paint `view` into `(x, y, w, h)` on `cr`, measuring text with
/// `layout`'s current font; each section body paints through its own
/// widget. `caret_visible` is the blink phase of a focused aux input's
/// caret.
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
    style: &Style,
    line_height: f64,
    nerd_fonts_enabled: bool,
    caret_visible: bool,
) {
    layout.set_attributes(None);
    layout.set_text("M");
    let char_width = layout.pixel_size().0.max(1) as f64;
    let measure = PangoMeasure {
        pango_layout: Some(layout),
        char_width,
    };
    let measurer = |text: &str, _role: crate::FontRole| measure.width_of(text);
    let ctx = LayoutCtx::new(line_height as f32, char_width as f32, nerd_fonts_enabled)
        .with_measurer(&measurer)
        .with_style(style);
    let rect = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let view_layout = Primitive::layout(view, &ctx, rect);
    let mut surface = CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    crate::primitives::multi_section_view::native_surface_paint::paint(
        view,
        &view_layout,
        &mut surface,
        &crate::PaintEnv::new(theme, style),
        caret_visible,
    );
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
        Axis, EmptyBody, SectionAux, SectionBody, SectionHeader,
    };
    use crate::primitives::multi_section_view::{
        InlineInput, MultiSectionViewHit, ScrollMode, ScrollbarHit, Section, SectionSize,
    };
    use crate::primitives::tree::{TreeRow, TreeView};
    use crate::types::StyledText;
    use crate::types::{Color, Decoration, SelectionMode, WidgetId};
    use pangocairo::cairo::{Format, ImageSurface};

    /// Surface canvas size: wide enough for chevron + title + (optional)
    /// scrollbar gutter; tall enough for several `EqualShare` sections.
    const W: i32 = 200;
    const H: i32 = 200;

    /// Standard line-height the GTK rasteriser is parameterised by.
    /// Header rows render at `line_height * 1.4`; body rows at the
    /// same (per `crate::primitives::multi_section_view::msv_body_measure`).
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

    /// Single-section view with a focused, empty `Search` aux row — the
    /// caret sits at `aux.x + 4` (text is empty, `caret: 0`), so a
    /// single pixel probe distinguishes "caret painted" from "caret
    /// skipped" without measuring glyph widths. Mirrors macOS's
    /// `search_section` test fixture.
    fn search_section() -> MultiSectionView {
        view_with(vec![Section {
            id: "search".into(),
            header: SectionHeader {
                title: StyledText::plain("Search"),
                show_chevron: false,
                ..Default::default()
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
                style: Default::default(),
                has_focus: false,
            }),
            size: SectionSize::EqualShare,
            collapsed: false,
            min_size: None,
            max_size: None,
        }])
    }

    /// Paint `view` into a fresh surface; return (surface, layout).
    /// Hit-test uses the SAME layout the rasteriser used internally —
    /// that's the source-of-truth contract `gtk_msv_layout` enforces.
    /// Paints with the caret phase on, same as
    /// [`paint_then_layout_with_caret`] called with `true`; most tests
    /// here don't exercise `SectionAux::Input`/`Search` caret painting.
    fn paint_then_layout(view: &MultiSectionView) -> (ImageSurface, MultiSectionViewLayout) {
        paint_then_layout_with_caret(view, true)
    }

    /// Same as [`paint_then_layout`], with an explicit caret-blink
    /// phase threaded into the aux-row text-input caret — mirrors
    /// macOS's `paint_with_caret_phase` test helper.
    fn paint_then_layout_with_caret(
        view: &MultiSectionView,
        caret_visible: bool,
    ) -> (ImageSurface, MultiSectionViewLayout) {
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
                &Style::default(),
                LINE_HEIGHT,
                /* nerd_fonts */ false,
                caret_visible,
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

    /// Panel-level (`WholePanel`) scrollbar: the painted thumb pixel
    /// matches `theme.scrollbar_thumb`, the track pixel below it matches
    /// `theme.scrollbar_track`, and hit-testing the painted thumb pixel
    /// agrees with the layout. Closes the paint/layout/hit-test round
    /// trip for the GTK panel scrollbar the same way
    /// `gtk_scrollbar_column_hits_scrollbar_not_body_when_overflowing`
    /// does for per-section scrollbars.
    ///
    /// Verifies that paint and hit-test both read the same `Rect` from
    /// `panel_scrollbar_thumb`.
    #[test]
    fn gtk_panel_scrollbar_thumb_paints_at_layout_position() {
        let items = [
            "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10",
        ];
        let v = MultiSectionView {
            id: WidgetId::new("v"),
            sections: vec![tree_section("a", &items), tree_section("b", &items)],
            active_section: None,
            axis: Axis::Vertical,
            allow_resize: true,
            allow_collapse: true,
            scroll_mode: ScrollMode::WholePanel,
            has_focus: true,
            panel_scroll: 0.0,
        };
        let (mut surface, layout) = paint_then_layout(&v);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let panel_sb = layout
            .panel_scrollbar
            .expect("11-row sections in a 200px canvas should overflow");
        let thumb = layout
            .panel_scrollbar_thumb
            .expect("panel_scrollbar present implies panel_scrollbar_thumb present");
        assert!(
            thumb.height < panel_sb.height - 4.0,
            "test assumes the thumb doesn't fill the whole track (thumb={thumb:?}, \
             track={panel_sb:?})"
        );

        let theme = test_theme();
        let gx = (panel_sb.x + panel_sb.width / 2.0).floor() as i32;

        // A pixel well inside the painted thumb is the thumb colour.
        let thumb_y = (thumb.y + 3.0).floor() as i32;
        assert_eq!(
            pixel(&data, stride, gx, thumb_y),
            (
                theme.scrollbar_thumb.r,
                theme.scrollbar_thumb.g,
                theme.scrollbar_thumb.b
            ),
            "pixel inside panel_scrollbar_thumb should be the thumb colour"
        );

        // A pixel in the track below the thumb is the track colour.
        let track_y = (panel_sb.y + panel_sb.height - 3.0).floor() as i32;
        assert_eq!(
            pixel(&data, stride, gx, track_y),
            (
                theme.scrollbar_track.r,
                theme.scrollbar_track.g,
                theme.scrollbar_track.b
            ),
            "pixel in the track below the thumb should be the track colour"
        );

        // hit_test at the sampled thumb pixel agrees with paint.
        match layout.hit_test(gx as f32 + 0.5, thumb_y as f32 + 0.5) {
            MultiSectionViewHit::PanelScrollbar {
                kind: ScrollbarHit::Thumb,
            } => {}
            other => panic!(
                "hit at painted panel-thumb pixel ({}, {}) returned {:?}",
                gx, thumb_y, other
            ),
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

    /// Tree body round-trip with mixed Decoration::Header and Normal
    /// rows. Header rows are shorter (1.2× line_height) than normal
    /// rows (1.4× line_height). Verifies that clicking in the bottom
    /// pixel of a header row hit-tests to the header row, not the
    /// child below it.
    #[test]
    fn gtk_header_decoration_row_boundary_round_trip() {
        use crate::primitives::tree::TreeViewHit;

        let rows = vec![
            TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("src/main.rs"),
                badge: None,
                is_expanded: Some(true),
                decoration: Decoration::Header,
                edit: None,
            },
            TreeRow {
                path: vec![0, 0],
                indent: 1,
                icon: None,
                text: StyledText::plain("line 12: fn main()"),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            },
            TreeRow {
                path: vec![0, 1],
                indent: 1,
                icon: None,
                text: StyledText::plain("line 45: let config"),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            },
        ];

        let tree = TreeView {
            id: WidgetId::new("results"),
            rows,
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: Default::default(),
            has_focus: true,
        };

        let section = Section {
            id: "results".into(),
            header: SectionHeader {
                title: StyledText::plain("RESULTS"),
                show_chevron: false,
                ..Default::default()
            },
            body: SectionBody::Tree(tree),
            aux: None,
            size: SectionSize::EqualShare,
            collapsed: false,
            min_size: None,
            max_size: None,
        };

        let view = view_with(vec![section]);
        let bounds = QRect::new(0.0, 0.0, W as f32, H as f32);
        let msv_layout = gtk_msv_layout(&view, bounds, LINE_HEIGHT);

        let s = &msv_layout.sections[0];
        let body_b = s.body_bounds;

        let tree_ref = match &view.sections[0].body {
            SectionBody::Tree(t) => t,
            _ => panic!("expected tree body"),
        };
        let tree_layout = crate::primitives::layout_metrics::pixel_tree_layout(
            tree_ref,
            body_b,
            LINE_HEIGHT,
            8.0,
        );

        let header_h = (LINE_HEIGHT * 1.2).round();
        let item_h = (LINE_HEIGHT * 1.4).round();

        // Row 0 (Header) should span [0, header_h).
        // Row 1 (Normal) should span [header_h, header_h + item_h).
        assert_eq!(tree_layout.visible_rows.len(), 3);

        let r0 = &tree_layout.visible_rows[0];
        let r1 = &tree_layout.visible_rows[1];
        assert!(
            (r0.bounds.height as f64 - header_h).abs() < 0.01,
            "header row height {}, expected {}",
            r0.bounds.height,
            header_h
        );
        assert!(
            (r1.bounds.y as f64 - header_h).abs() < 0.01,
            "child row starts at {}, expected {}",
            r1.bounds.y,
            header_h
        );

        // Hit-test bottom pixel of header row → Row(0) or Chevron(0).
        // The header is a branch row so x=5 may fall in the chevron region — both
        // Chevron(0) and Row(0) satisfy the row-discrimination requirement.
        let bottom_of_header = header_h as f32 - 0.5;
        match tree_layout.hit_test(5.0, bottom_of_header) {
            TreeViewHit::Row(0) | TreeViewHit::Chevron(0) => {}
            other => panic!(
                "click at y={bottom_of_header} returned {:?}, expected Row(0) or Chevron(0)",
                other
            ),
        }

        // Hit-test top pixel of child row → Row(1).
        let top_of_child = header_h as f32 + 0.5;
        match tree_layout.hit_test(5.0, top_of_child) {
            TreeViewHit::Row(idx) => assert_eq!(
                idx, 1,
                "click at y={top_of_child} (top of child) hit row {idx}, expected 1"
            ),
            other => panic!(
                "click at y={top_of_child} returned {:?}, expected Row(1)",
                other
            ),
        }

        // Verify msv_body_measure content_size matches the tree's total row heights.
        let expected_content = header_h as f32 + 2.0 * item_h as f32;
        let actual_content = crate::primitives::multi_section_view::msv_body_measure(
            &view.sections[0].body,
            &view.sections[0].aux,
            LINE_HEIGHT,
        );
        assert!(
            (actual_content.content_size - expected_content).abs() < 0.01,
            "msv_body_measure content_size {}, expected {} (header_h={}, item_h={})",
            actual_content.content_size,
            expected_content,
            header_h,
            item_h
        );
    }

    /// `msv_layout` is documented **ABSOLUTE**: section
    /// header/body bounds must start at the view's own origin, not
    /// (0, 0) — the case that hides a LOCAL/ABSOLUTE mixup.
    fn header_hit_round_trip_at(x: f32, y: f32) {
        let view = view_with(vec![tree_section("alpha", &["a1", "a2"])]);
        let bounds = QRect::new(x, y, 100.0, 60.0);
        let layout = gtk_msv_layout(&view, bounds, LINE_HEIGHT);

        let sl = &layout.sections[0];
        assert_eq!(sl.header_bounds.x, x);
        assert_eq!(sl.header_bounds.y, y);

        let hx = sl.header_bounds.x + 5.0;
        let hy = sl.header_bounds.y + sl.header_bounds.height / 2.0;
        match layout.hit_test(hx, hy) {
            MultiSectionViewHit::Header { section, .. } => assert_eq!(section, 0),
            other => panic!("expected Header hit at ({hx}, {hy}), got {other:?}"),
        }

        let bx = sl.body_bounds.x + 1.0;
        let by = sl.body_bounds.y + 1.0;
        match layout.hit_test(bx, by) {
            MultiSectionViewHit::Body { section } => assert_eq!(section, 0),
            other => panic!("expected Body hit at ({bx}, {by}), got {other:?}"),
        }
    }

    #[test]
    fn header_hit_round_trip() {
        header_hit_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn header_hit_round_trip_at_nonzero_origin() {
        header_hit_round_trip_at(7.0, 13.0);
    }

    /// GTK's `paint_header` must clip the title paint, unlike pre-port
    /// behavior where a title wider than its title/badge region bled ink
    /// straight past the header's own right margin. This section has no
    /// chevron and no actions, so the only thing that can ever legitimately
    /// paint in the header's last 4px (the margin `push_header_hits`/`paint_header`
    /// both reserve at the trailing edge) is an unclipped title overrunning
    /// its bounds.
    #[test]
    fn gtk_header_clips_long_title_before_right_margin() {
        let section = Section {
            id: "s".into(),
            header: SectionHeader {
                icon: None,
                title: StyledText::plain("W".repeat(200)),
                badge: None,
                actions: vec![],
                show_chevron: false,
            },
            body: SectionBody::Empty(EmptyBody {
                text: StyledText::plain(""),
                ..Default::default()
            }),
            aux: None,
            size: SectionSize::EqualShare,
            collapsed: false,
            min_size: None,
            max_size: None,
        };
        let v = view_with(vec![section]);
        let (mut surface, layout) = paint_then_layout(&v);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let hdr = layout.sections[0].header_bounds;
        // 2px inside the header's reserved trailing margin — well clear
        // of the clip boundary's own antialiased edge on one side and
        // the header's true right edge on the other.
        let probe_x = (hdr.x + hdr.width - 2.0) as i32;
        let probe_y = (hdr.y + hdr.height / 2.0) as i32;
        let (r, g, b) = pixel(&data, stride, probe_x, probe_y);
        let bg = Theme::default().header_bg;
        assert_eq!(
            (r, g, b),
            (bg.r, bg.g, bg.b),
            "an overlong title must not bleed ink into the header's trailing margin \
             at ({probe_x}, {probe_y}); got ({r}, {g}, {b}), expected header_bg {bg:?}",
        );
    }

    // ── Caret blink threading ────────────────────────────────────────────
    //
    // `caret_visible` flows from `GtkBackend::caret_blink`
    // (`crate::caret_blink::CaretBlinkState`) through
    // `draw_multi_section_view`/`paint_aux` into
    // `native_surface_paint::paint_aux`. These tests pin the phase at
    // the paint-function boundary (bypassing the timer entirely) and
    // assert the single caret pixel flips between `theme.foreground`
    // and `theme.input_bg` — mirrors macOS's
    // `caret_visible_true_paints_foreground_at_caret_position`/
    // `caret_visible_false_leaves_caret_column_as_input_bg`/
    // `unfocused_input_skips_caret_regardless_of_phase` tests.

    /// Pixel the empty, focused `search_section` input's caret paints
    /// at: `aux.x + 4` (text is empty so there's no glyph run before
    /// it), vertically centered in the aux row.
    fn caret_pixel(layout: &MultiSectionViewLayout) -> (i32, i32) {
        let aux = layout.sections[0]
            .aux_bounds
            .expect("search_section always has an aux row");
        ((aux.x + 4.0) as i32, (aux.y + aux.height / 2.0) as i32)
    }

    #[test]
    fn gtk_caret_visible_true_paints_foreground_at_caret_position() {
        let v = search_section();
        let (mut surface, layout) = paint_then_layout_with_caret(&v, true);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (x, y) = caret_pixel(&layout);
        let fg = test_theme().foreground;
        assert_eq!(
            pixel(&data, stride, x, y),
            (fg.r, fg.g, fg.b),
            "caret_visible=true must paint the caret at ({x}, {y})",
        );
    }

    #[test]
    fn gtk_caret_visible_false_leaves_caret_column_as_input_bg() {
        let v = search_section();
        let (mut surface, layout) = paint_then_layout_with_caret(&v, false);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (x, y) = caret_pixel(&layout);
        let input_bg = test_theme().input_bg;
        assert_eq!(
            pixel(&data, stride, x, y),
            (input_bg.r, input_bg.g, input_bg.b),
            "caret_visible=false must skip the caret fill at ({x}, {y})",
        );
    }

    #[test]
    fn gtk_unfocused_input_skips_caret_regardless_of_phase() {
        let mut v = search_section();
        if let Some(SectionAux::Search(input)) = v.sections[0].aux.as_mut() {
            input.has_focus = false;
        } else {
            panic!("search_section must build a Search aux");
        }
        let (mut surface, layout) = paint_then_layout_with_caret(&v, true);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (x, y) = caret_pixel(&layout);
        let input_bg = test_theme().input_bg;
        assert_eq!(
            pixel(&data, stride, x, y),
            (input_bg.r, input_bg.g, input_bg.b),
            "an unfocused input must never paint a caret, even with caret_visible=true",
        );
    }
}
