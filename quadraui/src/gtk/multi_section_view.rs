//! GTK rasteriser for [`crate::MultiSectionView`].
//!
//! Paints the full chrome (per-section headers, optional aux rows,
//! per-section scrollbars, optional dividers) onto a [`Context`] and
//! dispatches each section's body to the appropriate quadraui body
//! rasteriser (`draw_tree`, `draw_list`, etc.) using the body bounds
//! returned by the primitive's [`crate::MultiSectionView::layout`].
//!
//! Vertical-only in v1 (per #294 / D-003 in `quadraui/docs/decisions/DECISIONS.md`);
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

use super::{cairo_rgb, draw_list, draw_message_list, draw_tree};
use crate::event::Rect as QRect;
use crate::primitives::multi_section_view::{
    Axis, EmptyBody, MsvLayoutMetrics, MultiSectionView, MultiSectionViewLayout, SectionAux,
    SectionBody, SectionHeader,
};
use crate::theme::Theme;
use crate::types::StyledText;

/// Compute the GTK metrics for a `MultiSectionView` from a
/// `line_height`. Backends call this AND the primitive's `layout()`
/// with the same metrics so paint and click resolve to the same bounds.
///
/// Thin wrapper over [`crate::primitives::layout_metrics::msv_metrics`]
/// (#499) — identical math across every pixel backend.
pub fn metrics_for(line_height: f64, allow_resize: bool) -> MsvLayoutMetrics {
    crate::primitives::layout_metrics::msv_metrics(line_height, allow_resize)
}

/// Compute the layout for a `MultiSectionView` using the GTK metrics
/// that the rasteriser would use itself. Hosts call this to drive
/// hit-testing without re-computing or re-measuring — paint AND click
/// share this single layout per frame. Mirrors TUI's [`crate::tui::tui_msv_layout`]
/// in spirit: one source-of-truth layout produced by one set of
/// metrics, consumed by both paint and hit-test.
///
/// Thin wrapper over [`crate::primitives::layout_metrics::msv_layout`]
/// (#499).
pub fn gtk_msv_layout(
    view: &MultiSectionView,
    bounds: QRect,
    line_height: f64,
) -> MultiSectionViewLayout {
    crate::primitives::layout_metrics::msv_layout(view, bounds, line_height)
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
    // Thumb geometry comes straight from `view_layout.panel_scrollbar_thumb`
    // (computed once, via `fit_thumb`, in `MultiSectionView::layout`) —
    // see `paint_panel_scrollbar`'s doc for why this used to be a third,
    // disagreeing formula (quadraui#820).
    if let Some(panel_sb) = view_layout.panel_scrollbar {
        paint_panel_scrollbar(cr, panel_sb, view_layout.panel_scrollbar_thumb, theme);
    }
}

// ── Section paint helpers ──────────────────────────────────────────────────
//
// #1074 (NativeSurface Phase 4): header/aux/text/empty/scrollbar/divider
// chrome painting moved to the shared
// [`crate::primitives::multi_section_view::native_surface_paint`] — these
// wrappers just build a [`super::surface::CairoSurface`] adapter and
// forward. `paint_body` (below) stays here: `Tree`/`List`/`MessageList`
// bodies still dispatch to this backend's own `draw_tree`/`draw_list`/
// `draw_message_list`, which take a raw `(&Context, &pango::Layout)` pair,
// not `&mut dyn NativeSurface`.

/// #1074: GTK's header row painted at an integer-rounded `y`/`height`
/// pre-port (`by`/`bh` were `.round()`ed; `bx`/`bw` were not) — a
/// GTK-specific crispness tweak the shared
/// [`crate::primitives::multi_section_view::native_surface_paint::paint_header`]
/// doesn't itself apply (Win/macOS never rounded either). Rounding the
/// `Rect` here, before handing it to the shared function, reproduces
/// that exact pre-port behaviour instead of silently dropping it.
fn paint_header(
    cr: &Context,
    layout: &pango::Layout,
    bounds: QRect,
    header: &SectionHeader,
    collapsed: bool,
    theme: &Theme,
) {
    let rounded = QRect::new(
        bounds.x,
        bounds.y.round(),
        bounds.width,
        bounds.height.round(),
    );
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_header(
        &mut surface,
        rounded,
        header,
        collapsed,
        theme,
    );
}

fn paint_aux(cr: &Context, layout: &pango::Layout, bounds: QRect, aux: &SectionAux, theme: &Theme) {
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    // `caret_visible: true` — GTK has no caret-blink timer for MSV aux
    // inputs (unlike macOS's #188), so the caret always paints while
    // focused, matching this rasteriser's pre-port behaviour exactly.
    crate::primitives::multi_section_view::native_surface_paint::paint_aux(
        &mut surface,
        bounds,
        aux,
        theme,
        true,
    );
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
            draw_form_body(cr, layout, x, y, w, h, f, theme, line_height);
        }
        SectionBody::Chart(c) => {
            // #810: painting moved to the shared
            // `crate::primitives::chart::paint`; this raw
            // `(&Context, &pango::Layout)` call site (no live
            // `GtkBackend` on hand) uses the same shared
            // `super::surface::CairoSurface` adapter (#1072)
            // `form::draw_form`'s deprecated shim uses.
            layout.set_text("M");
            let char_width = layout.pixel_size().0 as f64;
            let chart_layout =
                super::gtk_chart_layout(c, x, y, w, h, line_height, char_width.max(1.0));
            let mut surface = super::surface::CairoSurface {
                cr,
                layout: Some(layout),
                translucent_fill: false,
            };
            crate::primitives::chart::paint(c, &chart_layout, &mut surface, theme, None, None);
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

/// Paint an embedded [`crate::Form`] section body. #808: field-kind
/// painting goes through the shared [`crate::primitives::form::paint`]
/// via [`super::surface::CairoSurface`] (this call site has only a raw
/// `cr`/`layout`, not a live [`super::backend::GtkBackend`]) —
/// `FieldKind::Toolbar` is painted separately below, same as
/// `GtkBackend::draw_form`, for the same reason (see that fn's doc).
#[allow(clippy::too_many_arguments)]
fn draw_form_body(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    form: &crate::Form,
    theme: &Theme,
    line_height: f64,
) {
    let row_h = crate::primitives::layout_metrics::form_row_height(line_height);
    let measure = super::toolbar::PangoMeasure {
        pango_layout: Some(layout),
        char_width: 8.0,
    };
    let flayout = form.layout(w as f32, h as f32, |i| {
        crate::primitives::layout_metrics::form_field_measure(&form.fields[i], row_h, &measure)
    });
    let origin = crate::Point::new(x as f32, y as f32);
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: false,
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
        layout.set_text(&label_text);
        let (label_w, _) = layout.pixel_size();
        let row_x = x + vf.bounds.x as f64;
        let row_y = y + vf.bounds.y as f64;
        let row_w = vf.bounds.width as f64;
        let toolbar_row_h = vf.bounds.height as f64;
        let toolbar_x = if no_label {
            row_x + 6.0
        } else {
            row_x + 6.0 + label_w as f64 + 12.0
        };
        let toolbar_w = row_x + row_w - toolbar_x;
        if toolbar_w > 0.0 {
            super::toolbar::draw_toolbar(
                cr,
                layout,
                toolbar_x,
                row_y,
                toolbar_w,
                toolbar_row_h,
                toolbar,
                theme,
                None,
                None,
            );
            layout.set_attributes(None);
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
    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
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
    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_empty_body(
        &mut surface,
        bounds,
        empty,
        theme,
        line_height as f32,
    );
}

fn paint_scrollbar(cr: &Context, gutter: QRect, thumb_bounds: Option<QRect>, theme: &Theme) {
    // Thumb at the layout-computed position when the body's scroll
    // state was introspectable (`Tree`, `List`). Falls back to a
    // 20%-tall top-anchored thumb for overflowing bodies without
    // row-based scroll — visual continuity with pre-#9. Per
    // *Primitive Authoring Rule #6*: thumb position is state-derived
    // and lives on the layout, not the rasteriser.
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: None,
        translucent_fill: true,
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_scrollbar(
        &mut surface,
        gutter,
        thumb_bounds,
        theme,
    );
}

/// Panel-level scrollbar. Paints the track, then the thumb at
/// `thumb_bounds` — geometry computed once by
/// [`crate::primitives::multi_section_view::MultiSectionView::layout`]
/// (`fit_thumb`) and published as
/// [`crate::primitives::multi_section_view::MultiSectionViewLayout::panel_scrollbar_thumb`].
///
/// Pre-quadraui#820 this function computed thumb size/position itself
/// from `(scroll, total_content)` with its own formula that disagreed
/// with the layout's `fit_thumb`-based one in two ways: the caller
/// passed `total_content` as *just* the summed section sizes, silently
/// dropping divider strips (so the painted thumb size drifted from the
/// hit-tested thumb whenever `allow_resize` was on and dividers were
/// present); and even with matching totals, a hardcoded `20.0`-pixel
/// minimum thumb here disagreed with `panel_thumb_min`'s
/// `metrics.scrollbar_size.max(8.0)` used everywhere else. Mirrors
/// [`paint_scrollbar`]'s (per-section) pattern of consuming
/// pre-computed bounds instead of re-deriving them.
fn paint_panel_scrollbar(cr: &Context, bounds: QRect, thumb_bounds: Option<QRect>, theme: &Theme) {
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: None,
        translucent_fill: true,
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_panel_scrollbar(
        &mut surface,
        bounds,
        thumb_bounds,
        theme,
    );
}

fn paint_divider(cr: &Context, bounds: QRect, theme: &Theme) {
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: None,
        translucent_fill: true,
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_divider(
        &mut surface,
        bounds,
        theme,
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
        MultiSectionViewHit, ScrollMode, ScrollbarHit, Section, SectionSize,
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
    /// same (per `crate::primitives::layout_metrics::msv_body_measure`).
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

    /// Panel-level (`WholePanel`) scrollbar: the painted thumb pixel
    /// matches `theme.scrollbar_thumb`, the track pixel below it matches
    /// `theme.scrollbar_track`, and hit-testing the painted thumb pixel
    /// agrees with the layout. Closes the paint/layout/hit-test round
    /// trip for the GTK panel scrollbar the same way
    /// `gtk_scrollbar_column_hits_scrollbar_not_body_when_overflowing`
    /// does for per-section scrollbars.
    ///
    /// Regression coverage for quadraui#820: pre-fix, GTK's
    /// `paint_panel_scrollbar` recomputed thumb geometry itself from
    /// `layout.sections.iter().map(|s| s.resolved_size).sum()` (dropping
    /// the divider strip from the total) and a hardcoded `20.0`-pixel
    /// minimum thumb, instead of reusing `panel_scrollbar_thumb` — the
    /// same geometry `fit_thumb` + `panel_thumb_min` already computed
    /// for hit-testing. Both formulas' *outputs* were close enough in
    /// pixel terms on this shape that a numeric magnitude assertion
    /// (like the TUI regression test's cell-precision one) isn't
    /// reliable here; what this test locks down instead is that paint
    /// and hit-test now provably read the same `Rect`.
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
        use crate::gtk::tree::gtk_tree_layout;
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
        let tree_layout = gtk_tree_layout(tree_ref, body_b, LINE_HEIGHT);

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
        let actual_content = crate::primitives::layout_metrics::msv_body_measure(
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

    /// `msv_layout` is documented **ABSOLUTE** (issue #505): section
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

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn header_hit_round_trip_at_nonzero_origin() {
        header_hit_round_trip_at(7.0, 13.0);
    }

    /// Regression for #1074's drift note: pre-port, GTK's `paint_header`
    /// never clipped the title paint, unlike Win/macOS — a title wider
    /// than its title/badge region bled ink straight past the header's
    /// own right margin. This section has no chevron and no actions, so
    /// the only thing that can ever legitimately paint in the header's
    /// last 4px (the margin `push_header_hits`/`paint_header` both
    /// reserve at the trailing edge, see `right_x`'s initial value) is
    /// an unclipped title overrunning its bounds.
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
}
