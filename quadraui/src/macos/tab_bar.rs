//! macOS rasteriser for [`crate::TabBar`]. [`CtFontMeasure`] feeds the
//! shared [`crate::primitives::layout_metrics::pixel_tab_bar_layout_fitting_boundary`];
//! painting goes through the shared
//! [`crate::primitives::tab_bar::native_surface_paint::paint`].

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::surface::CgSurface;
use super::text::measure_text;
use crate::backend::tab_bar_hits_from_layout;
use crate::primitives::layout_metrics::{
    pixel, pixel_tab_bar_layout_fitting_boundary, TextMeasure,
};
use crate::primitives::tab_bar::native_surface_paint::{self, TabBarPaintState};
use crate::primitives::tab_bar::{tab_icon_at, TabBarGlyphs, TabBarLayout, TabChrome, TabIcon};
#[allow(deprecated)]
use crate::primitives::tab_bar::{TabBar, TabBarHits};
use crate::style::Style;
use crate::theme::Theme;

/// `layout` translated by `dy` on the y-axis.
///
/// Baking `y_offset` into the layout's own `Rect`s (rather than issuing
/// a `CGContextTranslateCTM`) keeps every recorded text run honest:
/// macOS's text-run recording (`super::text::record_if_active`) captures
/// the raw `x`/`y` a draw call is given, with no view onto the context's
/// current transform — unlike GTK's `painted_text::show_layout`, which
/// converts through `Context::user_to_device`. A CTM-based shift would
/// paint pixels at the right place while recording them at `y_offset`
/// too low.
fn shift_layout_y(mut layout: TabBarLayout, dy: f32) -> TabBarLayout {
    if dy != 0.0 {
        for vt in &mut layout.visible_tabs {
            vt.bounds.y += dy;
            if let Some(cb) = &mut vt.close_bounds {
                cb.y += dy;
            }
        }
        for vs in &mut layout.visible_segments {
            vs.bounds.y += dy;
        }
        for r in layout
            .scroll_left
            .iter_mut()
            .chain(&mut layout.scroll_right)
        {
            r.y += dy;
        }
        for (r, _) in &mut layout.hit_regions {
            r.y += dy;
        }
    }
    layout
}

/// [`TextMeasure`] over a live [`CTFont`].
struct CtFontMeasure<'a>(&'a CTFont);
impl TextMeasure for CtFontMeasure<'_> {
    fn width_of(&self, text: &str) -> f32 {
        measure_text(self.0, text).0 as f32
    }
}

/// The [`TabBarLayout`] a paint call produces for `bar` at `(width,
/// height)`, without painting. Returns `(layout, corrected_scroll_offset,
/// available_cols)`.
pub fn mac_tab_bar_pixel_layout(
    font: &CTFont,
    width: f32,
    height: f32,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    style: &Style,
) -> (TabBarLayout, usize, usize) {
    let (tab_pad, tab_inner_gap, tab_outer_gap) = if bar.compact {
        (
            pixel::TAB_PAD_COMPACT,
            pixel::TAB_INNER_GAP_COMPACT,
            pixel::TAB_OUTER_GAP_COMPACT,
        )
    } else {
        (
            style.tab_padding_x,
            pixel::TAB_INNER_GAP,
            pixel::TAB_OUTER_GAP,
        )
    };
    let measure = CtFontMeasure(font);
    let tab_name_widths: Vec<f32> = bar
        .tabs
        .iter()
        .map(|t| measure.width_of(&t.label))
        .collect();
    // `0.0` for a tab without an icon, so `&[]` reproduces icon-less geometry.
    let icon_extras: Vec<f32> = (0..bar.tabs.len())
        .map(|i| match tab_icon_at(icons, i) {
            Some(icon) => measure_text(font, &icon.glyph).0 as f32 + pixel::TAB_ICON_GAP,
            None => 0.0,
        })
        .collect();
    let (layout, corrected_scroll_offset, available_cols, _fitted) =
        pixel_tab_bar_layout_fitting_boundary(
            bar,
            width,
            height,
            tab_pad,
            tab_inner_gap,
            tab_outer_gap,
            &tab_name_widths,
            &icon_extras,
            &TabChrome::default(),
            &measure,
        );
    (layout, corrected_scroll_offset, available_cols)
}

/// Paint `bar` into `(0, y_offset, width, row_height)` on `ctx`; returns
/// [`TabBarHits`] (bar-relative — callers shift by `rect.x`).
/// # Safety
/// `ctx` must be a valid, non-null `CGContextRef` for the call's duration.
#[allow(clippy::too_many_arguments)]
#[allow(deprecated)]
pub unsafe fn draw_tab_bar_icons_with_glyphs(
    ctx: CGContextRef,
    font: &CTFont,
    width: f64,
    _line_height: f64, // derived from `font` — see `CgSurface::surface_line_height`
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    theme: &Theme,
    style: &Style,
    hovered_close_tab: Option<usize>,
    icons: &[Option<TabIcon>],
    glyphs: Option<&TabBarGlyphs>,
    nerd_fonts_enabled: bool,
    hovered_tab: Option<usize>,
    pressed_tab: Option<usize>,
    focused_index: Option<usize>,
) -> TabBarHits {
    let (layout, corrected_scroll_offset, available_cols) =
        mac_tab_bar_pixel_layout(font, width as f32, row_height as f32, bar, icons, style);

    let mut hits = tab_bar_hits_from_layout(&layout, bar);
    hits.correct_scroll_offset = corrected_scroll_offset;
    hits.available_cols = available_cols;

    let layout = shift_layout_y(layout, y_offset as f32);
    let mut surface = CgSurface {
        ctx,
        font: Some(font),
    };
    native_surface_paint::paint(
        bar,
        &layout,
        icons,
        glyphs,
        nerd_fonts_enabled,
        TabBarPaintState {
            hovered_tab,
            pressed_tab,
            hovered_close_tab,
            focused_index,
        },
        &mut surface,
        theme,
        style,
        y_offset as f32,
    );

    hits
}

/// [`draw_tab_bar_icons_with_glyphs`] with no glyph override and no
/// hover/press/focus state.
/// # Safety
/// Same contract as [`draw_tab_bar_icons_with_glyphs`].
#[allow(clippy::too_many_arguments)]
#[allow(deprecated)]
pub unsafe fn draw_tab_bar_icons(
    ctx: CGContextRef,
    font: &CTFont,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    theme: &Theme,
    style: &Style,
    hovered_close_tab: Option<usize>,
    icons: &[Option<TabIcon>],
) -> TabBarHits {
    draw_tab_bar_icons_with_glyphs(
        ctx,
        font,
        width,
        line_height,
        y_offset,
        row_height,
        bar,
        theme,
        style,
        hovered_close_tab,
        icons,
        None,
        false,
        None,
        None,
        None,
    )
}

/// [`draw_tab_bar_icons`] with an empty icon sidecar.
/// # Safety
/// Same contract as [`draw_tab_bar_icons_with_glyphs`].
#[allow(clippy::too_many_arguments)]
#[allow(deprecated)]
pub unsafe fn draw_tab_bar(
    ctx: CGContextRef,
    font: &CTFont,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    theme: &Theme,
    style: &Style,
    hovered_close_tab: Option<usize>,
) -> TabBarHits {
    draw_tab_bar_icons(
        ctx,
        font,
        width,
        line_height,
        y_offset,
        row_height,
        bar,
        theme,
        style,
        hovered_close_tab,
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::tab_bar::{TabBar, TabBarSegment, TabIcon, TabItem};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;

    const W: u32 = 480;
    const H: u32 = 28;
    const FONT_SIZE: f64 = 14.0;

    fn font() -> CTFont {
        make_font("Menlo", FONT_SIZE).expect("Menlo installed")
    }

    /// A backend whose *editor* and *chrome* fonts are both this module's
    /// `font()` — the tab bar paints/measures through `MacBackend::chrome_font`.
    fn backend_with_test_fonts() -> MacBackend {
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_chrome_font(font());
        backend
    }

    fn sample_bar() -> TabBar {
        TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![
                TabItem {
                    label: "main.rs".into(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
                TabItem {
                    label: "lib.rs".into(),
                    is_active: false,
                    is_dirty: true,
                    is_preview: false,
                    is_closable: true,
                },
            ],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: Some(Color::rgb(80, 140, 255)),
            show_tab_close: true,
            compact: false,
        }
    }

    /// Drive a paint through the full `MacBackend::draw_tab_bar` path and
    /// return `(surface, hits)` for inspection.
    #[allow(deprecated)]
    fn paint_via_backend(
        bar: &TabBar,
        hovered_close: Option<usize>,
    ) -> (BitmapSurface, TabBarHits) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(0.0, 0.0, W as f32, H as f32), bar, hovered_close);
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        (surface, hits.into_inner().unwrap())
    }

    #[test]
    #[allow(deprecated)]
    fn active_tab_paints_active_bg() {
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();

        let (start, end) = hits.slot_positions[0];
        assert!(end > start, "active tab slot must have non-zero width");

        let probe_x = (start + 2.0) as u32;
        let probe_y = H / 2;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        let expected = theme.tab_active_bg;
        assert_eq!(
            (r, g, b),
            (expected.r, expected.g, expected.b),
            "active tab bg at ({probe_x}, {probe_y}) should be tab_active_bg",
        );
    }

    /// The active tab's chip is inset by `pixel::TAB_CHIP_INSET_Y` from
    /// both the top and bottom of the row.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits`
    fn active_chip_is_inset_from_top_and_bottom_of_the_row() {
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        let active_bg = theme.tab_active_bg;

        let (start, _) = hits.slot_positions[0];
        let probe_x = (start + pixel::TAB_CHIP_RADIUS as f64 + 4.0) as u32;

        let is_active = |y: u32| {
            let (r, g, b, _) = surface.pixel(probe_x, y);
            (r, g, b) == (active_bg.r, active_bg.g, active_bg.b)
        };
        assert!(
            !is_active(0),
            "top scanline of the row must be outside the inset chip"
        );
        assert!(
            !is_active(H - 1),
            "bottom scanline of the row must be outside the inset chip"
        );
        assert!(
            is_active(H / 2),
            "chip must paint the active fill in the middle of the row"
        );
    }

    /// The active-tab chip is *rounded*, not just inset — probe all four
    /// corners of the chip's bounding rect: a rounded corner leaves them
    /// painted the bar background, not the active fill.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits`
    fn active_chip_corners_are_bar_background_not_active_fill() {
        // No `active_accent` here, unlike `sample_bar()` — the accent
        // strip paints the chip's full inset top edge including its top
        // corners, which would otherwise overwrite exactly the pixels
        // this test probes.
        let mut bar = sample_bar();
        bar.active_accent = None;
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        let bar_bg = theme.tab_bar_bg;

        // The shared painter's chip spans the tab's full slot width,
        // including `pixel::TAB_OUTER_GAP`'s share of it (unlike
        // `gtk::tab_bar`, which subtracts the gap) — see
        // `native_surface_paint::paint`'s doc.
        let (slot_x, slot_end) = hits.slot_positions[0];
        let chip_x0 = slot_x as u32;
        let chip_x1 = slot_end as u32 - 1;
        let chip_y0 = pixel::TAB_CHIP_INSET_Y as u32;
        let chip_y1 = H - pixel::TAB_CHIP_INSET_Y as u32 - 1;

        for (x, y) in [
            (chip_x0, chip_y0),
            (chip_x1, chip_y0),
            (chip_x0, chip_y1),
            (chip_x1, chip_y1),
        ] {
            let (r, g, b, _) = surface.pixel(x, y);
            assert_eq!(
                (r, g, b),
                (bar_bg.r, bar_bg.g, bar_bg.b),
                "chip bounding-rect corner ({x},{y}) should be bar background \
                 (rounded off), not the active fill",
            );
        }
    }

    #[test]
    #[allow(deprecated)]
    fn active_accent_paints_at_top_of_active_tab_chip() {
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);

        let (start, _) = hits.slot_positions[0];
        let probe_x = (start + 4.0) as u32;
        let probe_y = pixel::TAB_CHIP_INSET_Y as u32;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        let accent = bar.active_accent.unwrap();
        assert_eq!(
            (r, g, b),
            (accent.r, accent.g, accent.b),
            "accent strip at the chip's inset top edge should match TabBar.active_accent",
        );

        let (r0, g0, b0, _) = surface.pixel(probe_x, 0);
        assert_ne!(
            (r0, g0, b0),
            (accent.r, accent.g, accent.b),
            "accent strip must not paint at the strip's own top edge once inset"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn close_bounds_round_trip_via_hits_struct() {
        let bar = sample_bar();
        let (_surface, hits) = paint_via_backend(&bar, None);

        let close = hits.close_bounds[0].expect("active tab has close bounds");
        let mid_x = (close.0 + close.1) / 2.0;
        assert!(
            mid_x >= close.0 && mid_x < close.1,
            "midpoint must be inside close bounds [{}, {})",
            close.0,
            close.1,
        );
        let (slot_start, slot_end) = hits.slot_positions[0];
        assert!(
            close.0 >= slot_start && close.1 <= slot_end,
            "close bounds [{}, {}) must be inside tab slot [{}, {})",
            close.0,
            close.1,
            slot_start,
            slot_end,
        );
    }

    #[test]
    #[allow(deprecated)]
    fn dirty_tab_uses_filled_circle_glyph() {
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);

        let active_close = hits.close_bounds[0].unwrap();
        let dirty_close = hits.close_bounds[1].unwrap();

        fn ink_density(surface: &BitmapSurface, x: u32, y0: u32, y1: u32, bg: Color) -> u32 {
            (y0..y1)
                .filter(|&y| {
                    let (r, g, b, _) = surface.pixel(x, y);
                    !(r == bg.r && g == bg.g && b == bg.b)
                })
                .count() as u32
        }
        let theme = Theme::default();
        let chip_y0 = pixel::TAB_CHIP_INSET_Y as u32;
        let chip_y1 = H - pixel::TAB_CHIP_INSET_Y as u32;
        let active_ink = ink_density(
            &surface,
            ((active_close.0 + active_close.1) / 2.0) as u32,
            chip_y0,
            chip_y1,
            theme.tab_active_bg,
        );
        let dirty_ink = ink_density(
            &surface,
            ((dirty_close.0 + dirty_close.1) / 2.0) as u32,
            chip_y0,
            chip_y1,
            theme.tab_bar_bg,
        );
        assert!(
            dirty_ink > active_ink,
            "dirty glyph should ink more rows ({dirty_ink}) than the active close glyph ({active_ink})",
        );
    }

    #[test]
    #[allow(deprecated)]
    fn non_closable_tab_has_no_close_bounds_even_when_bar_show_close_is_true() {
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![
                TabItem {
                    label: "tab.rs".into(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
                TabItem {
                    label: "tab.rs".into(),
                    is_active: false,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: false,
                },
            ],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let (_surface, hits) = paint_via_backend(&bar, None);
        assert!(
            hits.close_bounds[0].is_some(),
            "closable tab must have close bounds",
        );
        assert!(
            hits.close_bounds[1].is_none(),
            "non-closable tab must have no close bounds even when bar.show_tab_close is true",
        );
        let (s0, e0) = hits.slot_positions[0];
        let (s1, e1) = hits.slot_positions[1];
        let closable_slot = e0 - s0;
        let pinned_slot = e1 - s1;
        assert!(
            closable_slot > pinned_slot,
            "closable tab slot ({closable_slot}) should reserve more width than pinned slot ({pinned_slot})",
        );
    }

    #[test]
    #[allow(deprecated)]
    fn empty_bar_paints_only_tab_bar_bg() {
        let bar = TabBar {
            id: WidgetId::new("empty"),
            tabs: vec![],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();

        let (r, g, b, _) = surface.pixel(W / 2, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b)
        );
        assert!(hits.slot_positions.is_empty());
        assert!(hits.close_bounds.is_empty());
    }

    /// Like [`paint_via_backend`], but through
    /// `MacBackend::draw_tab_bar_with_state` so a test can probe the
    /// whole-tab hover/pressed treatment.
    #[allow(deprecated)]
    fn paint_via_backend_with_state(
        bar: &TabBar,
        hovered_tab: Option<usize>,
        pressed_tab: Option<usize>,
    ) -> (BitmapSurface, TabBarHits) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar_with_state(
                QRect::new(0.0, 0.0, W as f32, H as f32),
                bar,
                None,
                hovered_tab,
                pressed_tab,
            );
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        (surface, hits.into_inner().unwrap())
    }

    #[test]
    #[allow(deprecated)]
    fn hovered_tab_tints_the_whole_background_not_just_the_close_glyph() {
        let bar = sample_bar();
        let theme = Theme::default();
        let (_, hits) = paint_via_backend(&bar, None);
        let (start, end) = hits.slot_positions[1];
        assert!(end > start, "second tab slot must have non-zero width");
        let probe_x = (start + 2.0) as u32;
        let probe_y = H / 2;

        let (resting, _) = paint_via_backend_with_state(&bar, None, None);
        let (r, g, b, _) = resting.pixel(probe_x, probe_y);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "with no hovered_tab, tab 1's background must stay tab_bar_bg",
        );

        let (hovered, _) = paint_via_backend_with_state(&bar, Some(1), None);
        let (r, g, b, _) = hovered.pixel(probe_x, probe_y);
        let expected = theme.list_hover_bg(theme.tab_bar_bg);
        assert_eq!(
            (r, g, b),
            (expected.r, expected.g, expected.b),
            "hovered_tab == Some(1) must tint tab 1's background toward list_hover_bg",
        );
    }

    #[test]
    #[allow(deprecated)]
    fn pressed_tab_tints_deeper_than_hovered_tab() {
        let bar = sample_bar();
        let theme = Theme::default();
        let (_, hits) = paint_via_backend(&bar, None);
        let (start, _) = hits.slot_positions[1];
        let probe_x = (start + 2.0) as u32;
        let probe_y = H / 2;

        let (pressed, _) = paint_via_backend_with_state(&bar, Some(1), Some(1));
        let (r, g, b, _) = pressed.pixel(probe_x, probe_y);
        let expected = theme.list_pressed_bg(theme.tab_bar_bg);
        assert_eq!(
            (r, g, b),
            (expected.r, expected.g, expected.b),
            "pressed_tab == Some(1) must tint tab 1's background toward list_pressed_bg, \
             even though hovered_tab also names it",
        );

        let hover_expected = theme.list_hover_bg(theme.tab_bar_bg);
        assert_ne!(
            (expected.r, expected.g, expected.b),
            (hover_expected.r, hover_expected.g, hover_expected.b),
            "pressed and hover tints must read as visibly different colours",
        );
    }

    #[test]
    #[allow(deprecated)]
    fn mouse_leaving_the_tab_clears_the_hover_tint() {
        let bar = sample_bar();
        let theme = Theme::default();
        let (_, hits) = paint_via_backend(&bar, None);
        let (start, _) = hits.slot_positions[1];
        let probe_x = (start + 2.0) as u32;
        let probe_y = H / 2;

        let (after_leave, _) = paint_via_backend_with_state(&bar, None, None);
        let (r, g, b, _) = after_leave.pixel(probe_x, probe_y);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "hovered_tab: None must repaint tab 1 back to the untinted tab_bar_bg",
        );
    }

    /// `Backend::tab_bar_layout` must return exactly what `draw_tab_bar`
    /// painted — both route through [`mac_tab_bar_pixel_layout`].
    #[test]
    #[allow(deprecated)]
    fn layout_twin_matches_the_painted_hits() {
        let bar = sample_bar();
        let (_surface, painted) = paint_via_backend(&bar, None);

        let backend = backend_with_test_fonts();
        let computed = backend.tab_bar_layout(QRect::new(0.0, 0.0, W as f32, H as f32), &bar);

        assert_eq!(painted.slot_positions, computed.slot_positions);
        assert_eq!(painted.close_bounds, computed.close_bounds);
        assert_eq!(painted.right_segment_bounds, computed.right_segment_bounds);
        assert_eq!(painted.available_cols, computed.available_cols);
        assert_eq!(
            painted.correct_scroll_offset,
            computed.correct_scroll_offset
        );
    }

    /// A click at the centre of a reported close box lands on the close
    /// glyph the rasteriser actually drew.
    #[test]
    #[allow(deprecated)]
    fn close_box_centre_is_where_the_glyph_was_painted() {
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        let (lo, hi) = hits.close_bounds[0].expect("tab 0 is closable");
        let bg = theme.tab_active_bg;
        let chip_y0 = pixel::TAB_CHIP_INSET_Y as u32;
        let chip_y1 = H - pixel::TAB_CHIP_INSET_Y as u32;

        let inked = (lo.ceil() as u32..hi.floor() as u32).any(|x| {
            (chip_y0..chip_y1).any(|y| {
                let (r, g, b, _) = surface.pixel(x.min(W - 1), y);
                (r, g, b) != (bg.r, bg.g, bg.b)
            })
        });
        assert!(
            inked,
            "close box [{lo}, {hi}] contains no painted glyph — hits and paint have drifted",
        );
    }

    /// A helper for icon tests: drive a paint through the full
    /// `MacBackend::draw_tab_bar_icons` path with a real sidecar and
    /// return `(surface, hits)`.
    #[allow(deprecated)]
    fn paint_icons_via_backend(
        bar: &TabBar,
        icons: &[Option<TabIcon>],
        hovered_close: Option<usize>,
    ) -> (BitmapSurface, TabBarHits) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar_icons(
                QRect::new(0.0, 0.0, W as f32, H as f32),
                bar,
                icons,
                hovered_close,
            );
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        (surface, hits.into_inner().unwrap())
    }

    /// An ASCII icon on tab 0 only — ASCII on purpose, a Nerd Font
    /// codepoint measures as tofu under Menlo. Tab 1 is left `None` so
    /// every test below also covers the mixed-sidecar case.
    fn sample_icons() -> Vec<Option<TabIcon>> {
        vec![
            Some(TabIcon {
                glyph: "R".into(),
                color: Color::rgb(220, 120, 60),
            }),
            None,
        ]
    }

    #[test]
    #[allow(deprecated)]
    fn draw_tab_bar_icons_with_non_empty_sidecar_does_not_abort() {
        let bar = sample_bar();
        let icons = sample_icons();

        let (_surface, hits) = paint_icons_via_backend(&bar, &icons, None);
        let (_icon_less_surface, icon_less_hits) = paint_via_backend(&bar, None);

        // The decorated tab is wider than its icon-less self by exactly
        // the CoreText-measured glyph width plus `pixel::TAB_ICON_GAP`.
        let expected_extra = measure_text(&font(), "R").0 + pixel::TAB_ICON_GAP as f64;
        assert!(
            expected_extra > pixel::TAB_ICON_GAP as f64,
            "an ASCII icon glyph must measure non-zero under Menlo (got {expected_extra})",
        );

        let (lo, hi) = hits.slot_positions[0];
        let (plain_lo, plain_hi) = icon_less_hits.slot_positions[0];
        assert_eq!(lo, plain_lo, "the first tab still starts at the bar origin");
        assert!(
            ((hi - lo) - ((plain_hi - plain_lo) + expected_extra)).abs() < 0.01,
            "decorated slot width {} should be the icon-less width {} plus the \
             reservation {expected_extra}",
            hi - lo,
            plain_hi - plain_lo,
        );

        let (t1_lo, t1_hi) = hits.slot_positions[1];
        let (p1_lo, p1_hi) = icon_less_hits.slot_positions[1];
        assert!(
            ((t1_hi - t1_lo) - (p1_hi - p1_lo)).abs() < 0.01,
            "an undecorated tab must keep its icon-less width",
        );
        assert!(
            ((t1_lo - p1_lo) - expected_extra).abs() < 0.01,
            "an undecorated tab shifts right by exactly the preceding tab's \
             icon reservation",
        );
    }

    /// The layout-only twins must not abort, and must agree with the
    /// *painted* geometry once a sidecar is present.
    #[test]
    #[allow(deprecated)]
    fn layout_only_icon_twins_agree_with_the_icon_paint() {
        let bar = sample_bar();
        let icons = sample_icons();
        let rect = QRect::new(0.0, 0.0, W as f32, H as f32);
        let (_surface, painted) = paint_icons_via_backend(&bar, &icons, None);

        let mut backend = backend_with_test_fonts();

        let icon_hits = backend.tab_bar_layout_icons(rect, &bar, &icons);
        assert_eq!(icon_hits.slot_positions, painted.slot_positions);
        assert_eq!(icon_hits.close_bounds, painted.close_bounds);
        let plain_hits = backend.tab_bar_layout(rect, &bar);
        assert_ne!(
            icon_hits.slot_positions, plain_hits.slot_positions,
            "a non-empty sidecar must change the reported geometry",
        );

        let icon_layout = backend.resolve_tab_bar_layout_icons(rect, &bar, &icons);
        let as_hits = crate::backend::tab_bar_hits_from_layout(&icon_layout, &bar);
        assert_eq!(
            as_hits.slot_positions, painted.slot_positions,
            "the layout and hits spellings of the icon geometry must agree",
        );
        assert_eq!(as_hits.close_bounds, painted.close_bounds);

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let drawn = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *drawn.borrow_mut() = Some(b.draw_tab_bar_icons_layout(rect, &bar, &icons, None));
        });
        backend.end_frame();
        assert_eq!(
            drawn.into_inner().unwrap().visible_tabs,
            icon_layout.visible_tabs,
        );
    }

    #[test]
    #[allow(deprecated)]
    fn close_box_still_lands_on_the_close_glyph_with_an_icon_present() {
        let bar = sample_bar();
        let icons = sample_icons();
        let (surface, hits) = paint_icons_via_backend(&bar, &icons, None);
        let (_plain_surface, plain_hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();

        let (lo, hi) = hits.close_bounds[0].expect("tab 0 is closable");
        let (plain_lo, _) = plain_hits.close_bounds[0].expect("tab 0 is closable");
        assert!(
            lo > plain_lo,
            "the icon must push the close box right (icon {lo} vs icon-less {plain_lo})",
        );

        let (slot_lo, slot_hi) = hits.slot_positions[0];
        assert!(
            lo >= slot_lo && hi <= slot_hi,
            "close box [{lo}, {hi}) must stay inside tab slot [{slot_lo}, {slot_hi})",
        );

        let bg = theme.tab_active_bg;
        let chip_y0 = pixel::TAB_CHIP_INSET_Y as u32;
        let chip_y1 = H - pixel::TAB_CHIP_INSET_Y as u32;
        let inked = (lo.ceil() as u32..hi.floor() as u32).any(|x| {
            (chip_y0..chip_y1).any(|y| {
                let (r, g, b, _) = surface.pixel(x.min(W - 1), y);
                (r, g, b) != (bg.r, bg.g, bg.b)
            })
        });
        assert!(
            inked,
            "close box [{lo}, {hi}) contains no painted glyph — the icon \
             reservation moved the hit box off the × it draws",
        );
    }

    #[test]
    #[allow(deprecated)]
    fn icon_glyph_paints_before_the_label_in_its_own_colour() {
        let bar = sample_bar();
        let icons = sample_icons();
        let icon_color = icons[0].as_ref().unwrap().color;
        let (surface, hits) = paint_icons_via_backend(&bar, &icons, None);

        let (slot_lo, _) = hits.slot_positions[0];
        let icon_extra = measure_text(&font(), "R").0 + pixel::TAB_ICON_GAP as f64;
        let glyph_lo = slot_lo + Style::default().tab_padding_x as f64;
        let glyph_hi = glyph_lo + icon_extra - pixel::TAB_ICON_GAP as f64;
        assert!(glyph_hi > glyph_lo, "icon span must be non-empty");

        let bg = Theme::default().tab_active_bg;
        let dist = |a: (u8, u8, u8), c: Color| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
            d(a.0, c.r) + d(a.1, c.g) + d(a.2, c.b)
        };
        let chip_y0 = pixel::TAB_CHIP_INSET_Y as u32;
        let chip_y1 = H - pixel::TAB_CHIP_INSET_Y as u32;
        let found = (glyph_lo.floor() as u32..glyph_hi.ceil() as u32).any(|x| {
            (chip_y0..chip_y1).any(|y| {
                let (r, g, b, _) = surface.pixel(x.min(W - 1), y);
                dist((r, g, b), icon_color) < dist((r, g, b), bg)
            })
        });
        assert!(
            found,
            "no pixel in the icon span [{glyph_lo}, {glyph_hi}) leans towards \
             TabIcon::color {icon_color:?} — the glyph was not painted, or was \
             painted in the tab foreground",
        );
    }

    #[test]
    #[allow(deprecated)]
    fn empty_sidecar_is_pixel_identical_to_the_icon_less_paint() {
        let bar = sample_bar();
        let (icon_surface, icon_hits) = paint_icons_via_backend(&bar, &[], None);
        let (plain_surface, plain_hits) = paint_via_backend(&bar, None);

        assert_eq!(icon_hits.slot_positions, plain_hits.slot_positions);
        assert_eq!(icon_hits.close_bounds, plain_hits.close_bounds);
        assert_eq!(icon_hits.available_cols, plain_hits.available_cols);
        for y in 0..H {
            for x in 0..W {
                assert_eq!(
                    icon_surface.pixel(x, y),
                    plain_surface.pixel(x, y),
                    "pixel ({x}, {y}) differs with an empty sidecar",
                );
            }
        }
    }

    /// A sidecar shorter (or longer) than `bar.tabs` must not panic.
    #[test]
    #[allow(deprecated)]
    fn short_and_overlong_sidecars_are_tolerated() {
        let bar = sample_bar();
        let f = font();
        let style = Style::default();
        let layout_of = |icons: &[Option<TabIcon>]| {
            mac_tab_bar_pixel_layout(&f, W as f32, H as f32, &bar, icons, &style).0
        };

        let short = vec![Some(TabIcon {
            glyph: "R".into(),
            color: Color::rgb(220, 120, 60),
        })];
        let short_layout = layout_of(&short);
        let full_layout = layout_of(&sample_icons());
        assert_eq!(
            short_layout.visible_tabs.len(),
            full_layout.visible_tabs.len()
        );
        assert_eq!(
            short_layout.visible_tabs[0].bounds, full_layout.visible_tabs[0].bounds,
            "a short sidecar decorates the entries it does have",
        );

        let long = vec![
            None,
            None,
            Some(TabIcon {
                glyph: "R".into(),
                color: Color::rgb(220, 120, 60),
            }),
        ];
        let long_layout = layout_of(&long);
        let plain_layout = layout_of(&[]);
        assert_eq!(
            long_layout.visible_tabs, plain_layout.visible_tabs,
            "an overlong sidecar whose in-range entries are all None must \
             reproduce the icon-less geometry",
        );
    }

    /// The scroll-offset correction has to see the icon reservation too —
    /// a decorated tab is wider, so fewer tabs fit and the offset that
    /// keeps the active tab visible can differ.
    #[test]
    fn icon_reservation_feeds_the_scroll_offset_correction() {
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: (0..6)
                .map(|i| TabItem {
                    label: format!(" a_rather_long_file_name_{i}.rs "),
                    is_active: i == 5,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let icons: Vec<Option<TabIcon>> = (0..6)
            .map(|_| {
                Some(TabIcon {
                    glyph: "RR".into(),
                    color: Color::rgb(220, 120, 60),
                })
            })
            .collect();
        let f = font();
        let style = Style::default();

        let (plain, plain_corrected, _) =
            mac_tab_bar_pixel_layout(&f, W as f32, H as f32, &bar, &[], &style);
        let (decorated, decorated_corrected, _) =
            mac_tab_bar_pixel_layout(&f, W as f32, H as f32, &bar, &icons, &style);

        let slot_w = |l: &TabBarLayout| l.visible_tabs[0].bounds.width;
        assert!(
            slot_w(&decorated) > slot_w(&plain),
            "decorated slots must be wider than icon-less ones ({} vs {})",
            slot_w(&decorated),
            slot_w(&plain),
        );
        assert!(
            decorated_corrected >= plain_corrected,
            "wider tabs can only need the same or a later scroll offset \
             (icon-less {plain_corrected}, decorated {decorated_corrected})",
        );
        assert!(
            decorated.visible_tabs.len() <= plain.visible_tabs.len(),
            "decorated tabs are wider, so no more of them can fit \
             (icon-less {}, decorated {})",
            plain.visible_tabs.len(),
            decorated.visible_tabs.len(),
        );
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits`
    fn tab_bar_paints_at_rect_x_not_at_window_origin() {
        const SIDEBAR_W: f32 = 120.0;
        let canvas_w = SIDEBAR_W as u32 + W;
        let surface = BitmapSurface::new(canvas_w, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let bar = TabBar {
            id: WidgetId::new("empty"),
            tabs: vec![],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(canvas_w as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let _ = b.draw_tab_bar(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar, None);
        });
        backend.end_frame();

        let (_, _, _, a) = surface.pixel(4, H / 2);
        assert_eq!(
            a, 0,
            "sidebar column (x=4) must stay untouched by the tab bar fill",
        );

        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(SIDEBAR_W as u32 + 4, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "tab bar background should start at rect.x, not the window origin",
        );
    }

    /// [`tab_bar_paints_at_rect_x_not_at_window_origin`]'s y-axis twin: a
    /// strip below a toolbar (`rect.y != 0`) must paint its background at
    /// `rect.y`, not at the window's own top edge — `draw_tab_bar_icons_with_glyphs`
    /// bakes `y_offset` into every `Rect` in the layout rather than
    /// translating the CGContext's CTM (see `shift_layout_y`'s doc), so
    /// the shared painter's own `(0, 0)`-anchored background fill needs
    /// that same offset passed in separately as `bar_y`.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits`
    fn tab_bar_background_paints_at_rect_y_not_at_window_origin() {
        const TOOLBAR_H: f32 = 32.0;
        let canvas_h = TOOLBAR_H as u32 + H;
        let surface = BitmapSurface::new(W, canvas_h);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let bar = TabBar {
            id: WidgetId::new("empty"),
            tabs: vec![],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(W as f32, canvas_h as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let _ = b.draw_tab_bar(QRect::new(0.0, TOOLBAR_H, W as f32, H as f32), &bar, None);
        });
        backend.end_frame();

        let (_, _, _, a) = surface.pixel(W / 2, 4);
        assert_eq!(
            a, 0,
            "the row above the strip (toolbar area, y=4) must stay untouched by the tab bar fill",
        );

        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(W / 2, TOOLBAR_H as u32 + 4);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "tab bar background should start at rect.y, not the window origin",
        );
        let (r, g, b, _) = surface.pixel(W / 2, canvas_h - 1);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "tab bar background should cover down to the bottom of the strip",
        );
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits`
    fn tab_bar_hits_are_absolute_at_nonzero_rect_x() {
        const SIDEBAR_W: f32 = 120.0;
        let canvas_w = SIDEBAR_W as u32 + W;
        let surface = BitmapSurface::new(canvas_w, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let bar = sample_bar();
        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(canvas_w as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar, None);
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        let hits = hits.into_inner().unwrap();

        let (first_start, _) = hits.slot_positions[0];
        assert!(
            first_start >= SIDEBAR_W as f64,
            "first tab slot should start at/after rect.x={SIDEBAR_W}, got {first_start}",
        );
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits`
    fn layout_twin_matches_the_painted_hits_at_nonzero_origin() {
        const SIDEBAR_W: f32 = 120.0;
        let canvas_w = SIDEBAR_W as u32 + W;
        let surface = BitmapSurface::new(canvas_w, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let bar = sample_bar();
        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(canvas_w as f32, H as f32, 1.0));
        let painted = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar, None);
            *painted.borrow_mut() = Some(h);
        });
        backend.end_frame();
        let painted = painted.into_inner().unwrap();

        let computed = backend.tab_bar_layout(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar);

        assert_eq!(painted.slot_positions, computed.slot_positions);
        assert_eq!(painted.close_bounds, computed.close_bounds);
        assert_eq!(painted.right_segment_bounds, computed.right_segment_bounds);
        assert_eq!(painted.available_cols, computed.available_cols);
        assert_eq!(
            painted.correct_scroll_offset,
            computed.correct_scroll_offset
        );
        assert!(painted.slot_positions[0].0 >= SIDEBAR_W as f64);
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits`
    fn right_segments_are_shifted_by_rect_x_like_the_tabs() {
        const SIDEBAR_W: f32 = 120.0;
        let canvas_w = SIDEBAR_W as u32 + W;
        let surface = BitmapSurface::new(canvas_w, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut bar = sample_bar();
        bar.right_segments = vec![TabBarSegment {
            text: "\u{22ef}".into(), // "⋯" overflow glyph
            width_cells: 3,
            id: Some(WidgetId::new("tb:overflow")),
            is_active: false,
        }];

        // Bar-relative geometry, before any `rect.x` shift.
        let (bar_relative, _, _) =
            mac_tab_bar_pixel_layout(&font(), W as f32, H as f32, &bar, &[], &Style::default());
        assert_eq!(
            bar_relative.visible_segments.len(),
            1,
            "fixture bar should have exactly one right segment",
        );
        let seg = bar_relative.visible_segments[0].bounds;
        let (rel_start, rel_end) = (seg.x as f64, (seg.x + seg.width) as f64);

        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(canvas_w as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar, None);
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        let hits = hits.into_inner().unwrap();

        assert_eq!(hits.right_segment_bounds.len(), 1);
        assert_eq!(
            hits.right_segment_bounds[0],
            (rel_start + SIDEBAR_W as f64, rel_end + SIDEBAR_W as f64),
            "right segment bounds should be the bar-relative geometry shifted by rect.x={SIDEBAR_W}",
        );

        let computed = backend.tab_bar_layout(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar);
        assert_eq!(hits.right_segment_bounds, computed.right_segment_bounds);
    }

    /// The active tab's chip must fully enclose the painted label — icon,
    /// text and the close `×` — not just the label's *measured* width, on
    /// a row only just tall enough for the chrome font's own line height
    /// (the `tui_tab_icons`/`macos_tab_icons` demo's real row height).
    #[test]
    #[allow(deprecated)]
    fn active_chip_fully_encloses_the_painted_label_at_the_real_chrome_font() {
        let icons = vec![
            Some(TabIcon {
                glyph: "R".into(),
                color: Color::rgb(222, 165, 132),
            }),
            Some(TabIcon {
                glyph: "T".into(),
                color: Color::rgb(160, 190, 210),
            }),
            Some(TabIcon {
                glyph: "M".into(),
                color: Color::rgb(120, 170, 240),
            }),
        ];
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: [" main.rs ", " Cargo.toml ", " README.md "]
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == 1,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };

        const WIDE_W: u32 = 600;
        const DEMO_ROW_H: u32 = 16;
        let surface = BitmapSurface::new(WIDE_W, DEMO_ROW_H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.begin_frame(Viewport::new(WIDE_W as f32, DEMO_ROW_H as f32, 1.0));
        let (_, chrome_line_height) = backend.measure_text("M", crate::FontRole::Chrome);
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar_icons(
                QRect::new(0.0, 0.0, WIDE_W as f32, DEMO_ROW_H as f32),
                &bar,
                &icons,
                None,
            );
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        let hits = hits.into_inner().unwrap();

        let theme = Theme::default();
        let (slot_x, slot_end) = hits.slot_positions[1];
        let chip_x1 = slot_end - pixel::TAB_OUTER_GAP as f64;

        fn luma(c: Color) -> f64 {
            0.299 * c.r as f64 + 0.587 * c.g as f64 + 0.114 * c.b as f64
        }
        let ink_luma_threshold = ((luma(theme.tab_bar_bg).max(luma(theme.tab_active_bg))
            + luma(theme.tab_active_fg))
            / 2.0)
            .max(luma(theme.tab_bar_bg).max(luma(theme.tab_active_bg)) + 20.0);
        let inset = (pixel::TAB_CHIP_RADIUS as u32) + 6;
        let probe_x0 = (slot_x as u32) + inset;
        let probe_x1 = (chip_x1 as u32).saturating_sub(inset).max(probe_x0);

        let bar_bg = theme.tab_bar_bg;
        const BG_DIFF_EPSILON: f64 = 4.0;

        let mut violations: Vec<u32> = vec![];
        for y in 0..DEMO_ROW_H {
            let mut row_has_ink = false;
            let mut bg_color_counts: std::collections::HashMap<(u8, u8, u8), u32> =
                std::collections::HashMap::new();
            for x in probe_x0..probe_x1 {
                let (r, g, b, _) = surface.pixel(x, y);
                let pixel_luma = 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
                if pixel_luma > ink_luma_threshold {
                    row_has_ink = true;
                } else {
                    *bg_color_counts.entry((r, g, b)).or_insert(0) += 1;
                }
            }
            let row_has_chip_coverage = bg_color_counts
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .is_some_and(|((r, g, b), _)| {
                    (r as f64 - bar_bg.r as f64).abs() > BG_DIFF_EPSILON
                        || (g as f64 - bar_bg.g as f64).abs() > BG_DIFF_EPSILON
                        || (b as f64 - bar_bg.b as f64).abs() > BG_DIFF_EPSILON
                });
            if row_has_ink && !row_has_chip_coverage {
                violations.push(y);
            }
        }

        assert!(
            violations.is_empty(),
            "label ink painted in a row with no chip colour beneath it \
             (chrome_line_height={chrome_line_height}, demo_row_h={DEMO_ROW_H}): \
             rows={violations:?}"
        );
    }

    #[test]
    fn close_dirty_glyph_override_paints_app_glyph_default_paints_codicon() {
        let bar = sample_bar();
        let chrome = TabChrome::default();
        let rect = QRect::new(0.0, 0.0, W as f32, H as f32);

        let glyphs = TabBarGlyphs::new()
            .with_close_glyph("x")
            .with_dirty_glyph("*");

        let paint = |glyphs: Option<&TabBarGlyphs>| -> Vec<crate::testing::TextRun> {
            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = backend_with_test_fonts();
            backend.set_painted_text_recording(true);
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                let _ = b.draw_tab_bar_with_chrome_and_glyphs(rect, &bar, None, &chrome, glyphs);
            });
            let runs = backend.text_runs().to_vec();
            backend.end_frame();
            runs
        };

        let overridden = paint(Some(&glyphs));
        assert!(
            overridden.iter().any(|r| r.text == "*"),
            "expected the overridden dirty glyph among painted runs, got {:?}",
            overridden
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(!overridden
            .iter()
            .any(|r| r.text.contains(crate::codicon::DIRTY)));
        assert!(
            overridden.iter().any(|r| r.text == "x"),
            "expected the overridden close glyph among painted runs, got {:?}",
            overridden
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(!overridden
            .iter()
            .any(|r| r.text.contains(crate::codicon::CLOSE)));

        let default_runs = paint(None);
        assert!(
            default_runs
                .iter()
                .any(|r| r.text.contains(crate::codicon::DIRTY)),
            "expected the default codicon dirty glyph among painted runs, got {:?}",
            default_runs
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            default_runs
                .iter()
                .any(|r| r.text.contains(crate::codicon::CLOSE)),
            "expected the default codicon close glyph among painted runs, got {:?}",
            default_runs
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
        );
    }

    /// `TabChrome::focused_index` paints a focus-visible ring on the
    /// named tab — present after keyboard navigation, absent after a
    /// click (`focused_index: None`).
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits`
    fn focused_index_paints_a_ring_after_keyboard_nav_and_none_after_a_click() {
        let bar = sample_bar();
        let rect = QRect::new(0.0, 0.0, W as f32, H as f32);
        let style = Style::default().with_focus_ring_width(4.0);
        let theme = Theme::default();
        let ring_color = theme.focus_ring_color();
        let bar_bg = theme.tab_bar_bg;

        let paint = |focused_index: Option<usize>| -> (BitmapSurface, TabBarHits) {
            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = backend_with_test_fonts();
            backend.set_style(style.clone());
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            let hits = std::cell::RefCell::new(None);
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                let chrome = TabChrome::default().with_focused_index(focused_index);
                let h = b.draw_tab_bar_with_chrome_and_glyphs(rect, &bar, None, &chrome, None);
                *hits.borrow_mut() = Some(h);
            });
            backend.end_frame();
            (surface, hits.into_inner().unwrap())
        };

        let (focused_surface, focused_hits) = paint(Some(0));
        let (slot_x, slot_end) = focused_hits.slot_positions[0];
        let tab_content_w = slot_end - slot_x - pixel::TAB_OUTER_GAP as f64;
        let ring_width = style.focus_ring_width as f64;
        let inset = ring_width / 2.0;
        let left = (slot_x + inset).round() as u32;
        let right = (slot_x + tab_content_w - inset).round() as u32;
        let top = inset.round() as u32;
        let bottom = (H as f64 - inset).round() as u32;
        let mid_x = (slot_x + tab_content_w / 2.0).round() as u32;
        let mid_y = H / 2;

        let active_bg = theme.tab_active_bg;
        let probes = [
            ("left", left, mid_y, active_bg),
            ("right", right, mid_y, active_bg),
            ("top", mid_x, top, bar_bg),
            ("bottom", mid_x, bottom, bar_bg),
        ];
        for (name, px, py, _) in probes {
            let (r, g, b, _) = focused_surface.pixel(px, py);
            assert_eq!(
                (r, g, b),
                (ring_color.r, ring_color.g, ring_color.b),
                "expected the focus ring colour at the {name} edge ({px}, {py}) \
                 when TabChrome::focused_index names tab 0",
            );
        }

        let (clicked_surface, _clicked_hits) = paint(None);
        for (name, px, py, expected) in probes {
            let (r0, g0, b0, _) = clicked_surface.pixel(px, py);
            assert_eq!(
                (r0, g0, b0),
                (expected.r, expected.g, expected.b),
                "expected no focus ring at the {name} edge ({px}, {py}) when \
                 TabChrome::focused_index is None (click-suppressed)",
            );
        }
    }

    /// `Backend::draw_tab_bar_with_chrome`/`draw_tab_bar_with_chrome_layout`
    /// must both paint `chrome.focused_index`'s ring too, not just the
    /// `_and_glyphs` spelling.
    #[test]
    fn draw_tab_bar_with_chrome_and_its_layout_twin_both_paint_the_focus_ring() {
        let bar = sample_bar();
        let rect = QRect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();
        let ring_color = theme.focus_ring_color();
        let chrome = TabChrome::default().with_focused_index(Some(0));
        let probe_x = 1;
        let probe_y = H / 2;

        let paint_via = |f: fn(&mut MacBackend, QRect, &TabBar, Option<usize>, &TabChrome)| {
            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = backend_with_test_fonts();
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                f(b, rect, &bar, None, &chrome);
            });
            backend.end_frame();
            surface
        };

        #[allow(deprecated)] // `draw_tab_bar_with_chrome` returns the deprecated `TabBarHits`
        let via_chrome = paint_via(|b, rect, bar, hovered, chrome| {
            let _ = b.draw_tab_bar_with_chrome(rect, bar, hovered, chrome);
        });
        let (r, g, b) = {
            let (r, g, b, _) = via_chrome.pixel(probe_x, probe_y);
            (r, g, b)
        };
        assert_eq!(
            (r, g, b),
            (ring_color.r, ring_color.g, ring_color.b),
            "draw_tab_bar_with_chrome must paint chrome.focused_index's ring",
        );

        let via_chrome_layout = paint_via(|b, rect, bar, hovered, chrome| {
            let _ = b.draw_tab_bar_with_chrome_layout(rect, bar, hovered, chrome);
        });
        let (r2, g2, b2) = {
            let (r, g, b, _) = via_chrome_layout.pixel(probe_x, probe_y);
            (r, g, b)
        };
        assert_eq!(
            (r2, g2, b2),
            (ring_color.r, ring_color.g, ring_color.b),
            "draw_tab_bar_with_chrome_layout must paint chrome.focused_index's ring",
        );
    }

    /// `Style::tab_padding_x` sets the label's leading inset: with no icon
    /// decorating the tab, the label text run's painted `x` origin is
    /// exactly `slot_x + tab_padding_x`, not a fixed literal.
    #[test]
    #[allow(deprecated)]
    fn tab_padding_x_sets_the_labels_leading_inset() {
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![TabItem {
                label: "main.rs".into(),
                is_active: false,
                is_dirty: false,
                is_preview: false,
                is_closable: false,
            }],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: false,
            compact: false,
        };

        let label_x_for_style = |style: Style| -> (f64, f64) {
            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = backend_with_test_fonts();
            backend.set_style(style);
            backend.set_painted_text_recording(true);
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            let hits = std::cell::RefCell::new(None);
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                let h = b.draw_tab_bar(QRect::new(0.0, 0.0, W as f32, H as f32), &bar, None);
                *hits.borrow_mut() = Some(h);
            });
            let slot_x = hits.into_inner().unwrap().slot_positions[0].0;
            let runs = backend.text_runs().to_vec();
            backend.end_frame();
            let label_run = runs
                .iter()
                .find(|r| r.text == "main.rs")
                .expect("label must have painted a text run");
            (label_run.bounds.x as f64, slot_x)
        };

        let default_style = Style::default();
        let (default_label_x, default_slot_x) = label_x_for_style(default_style);
        assert!(
            (default_label_x - (default_slot_x + default_style.tab_padding_x as f64)).abs()
                < 0.01,
            "label x {default_label_x} should sit tab_padding_x={} in from slot edge {default_slot_x}",
            default_style.tab_padding_x,
        );

        let wide_style = Style::default().with_tab_padding_x(26.0);
        let (wide_label_x, wide_slot_x) = label_x_for_style(wide_style);
        assert!(
            (wide_label_x - (wide_slot_x + wide_style.tab_padding_x as f64)).abs() < 0.01,
            "label x {wide_label_x} should sit tab_padding_x={} in from slot edge {wide_slot_x}",
            wide_style.tab_padding_x,
        );
        assert!(
            (wide_label_x - default_label_x - 16.0).abs() < 0.01,
            "widening tab_padding_x by 16.0 (10.0 -> 26.0) should shift the label \
             by exactly 16.0 (default {default_label_x}, wide {wide_label_x})",
        );
    }

    /// A label wider than the tab strip it has to fit in is ellipsis-fitted
    /// instead of being dropped from the strip. The close glyph's hit box
    /// — and the glyph itself — must still land fully inside the narrow
    /// strip.
    #[test]
    #[allow(deprecated)]
    fn overlong_label_in_a_narrow_strip_ends_in_ellipsis_and_close_glyph_stays_fully_visible() {
        const NARROW_W: f64 = 110.0;

        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![TabItem {
                label: "a_very_long_filename_for_this_single_tab.rs".into(),
                is_active: true,
                is_dirty: false,
                is_preview: false,
                is_closable: true,
            }],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };

        let surface = BitmapSurface::new(NARROW_W as u32, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = backend_with_test_fonts();
        backend.set_painted_text_recording(true);
        backend.begin_frame(Viewport::new(NARROW_W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(0.0, 0.0, NARROW_W as f32, H as f32), &bar, None);
            *hits.borrow_mut() = Some(h);
        });
        let hits = hits.into_inner().unwrap();
        let runs = backend.text_runs().to_vec();
        backend.end_frame();

        assert_eq!(
            hits.slot_positions.len(),
            1,
            "the narrow strip must still show the tab (ellipsis-fitted), not drop it"
        );

        let label_run = runs
            .iter()
            .find(|r| r.text != crate::codicon::CLOSE.to_string() && r.text.contains('…'))
            .unwrap_or_else(|| {
                panic!(
                    "expected an ellipsis-truncated label run among painted runs, got {:?}",
                    runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
                )
            });
        assert!(
            label_run.text.ends_with('…'),
            "label run {:?} must end in an ellipsis",
            label_run.text
        );
        assert!(
            (label_run.text.chars().count() as f64) < bar.tabs[0].label.chars().count() as f64,
            "the painted label {:?} must be shorter than the original {:?}",
            label_run.text,
            bar.tabs[0].label,
        );

        let (close_lo, close_hi) =
            hits.close_bounds[0].expect("a closable tab must still report a close-button hit box");
        assert!(
            close_hi <= NARROW_W,
            "close hit box [{close_lo}, {close_hi}] must stay inside the {NARROW_W}-wide strip"
        );

        let close_run = runs
            .iter()
            .find(|r| r.text.contains(crate::codicon::CLOSE))
            .expect("the close glyph must still be painted");
        assert!(
            (close_run.bounds.x as f64) >= 0.0
                && (close_run.bounds.x + close_run.bounds.width) as f64 <= NARROW_W,
            "close glyph run {close_run:?} must be fully visible inside the {NARROW_W}-wide strip"
        );
    }

    /// A label that genuinely fits its slot must paint unchanged.
    #[test]
    #[allow(deprecated)]
    fn fully_fitting_labels_never_get_an_ellipsis() {
        for len in 1..=30usize {
            let label = "a".repeat(len);
            let bar = TabBar {
                id: WidgetId::new("tabs"),
                tabs: vec![TabItem {
                    label: label.clone(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                }],
                scroll_offset: 0,
                right_segments: vec![],
                active_accent: None,
                show_tab_close: true,
                compact: false,
            };

            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = backend_with_test_fonts();
            backend.set_painted_text_recording(true);
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                b.draw_tab_bar(QRect::new(0.0, 0.0, W as f32, H as f32), &bar, None);
            });
            let runs = backend.text_runs().to_vec();
            backend.end_frame();

            let label_run = runs
                .iter()
                .find(|r| r.text != crate::codicon::CLOSE.to_string())
                .unwrap_or_else(|| panic!("a {len}-char label must be painted, got {runs:?}"));
            assert_eq!(
                label_run.text, label,
                "a {len}-char label fitting a {W}-wide strip must paint unchanged, got {:?}",
                label_run.text
            );
        }
    }

    /// A label truncated by the overflow path, on a tab decorated with an
    /// icon, must reserve the icon's width identically on the hit-box side
    /// and the paint side.
    #[test]
    #[allow(deprecated)]
    fn overlong_label_with_icon_in_a_narrow_strip_reserves_icon_width_on_both_paint_and_hit_sides()
    {
        const NARROW_W: f64 = 150.0;

        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![TabItem {
                label: "a_very_long_filename_for_this_single_tab.rs".into(),
                is_active: true,
                is_dirty: false,
                is_preview: false,
                is_closable: true,
            }],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let icons = vec![Some(TabIcon {
            glyph: "R".into(),
            color: Color::rgb(220, 120, 60),
        })];

        let surface = BitmapSurface::new(NARROW_W as u32, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = backend_with_test_fonts();
        backend.set_painted_text_recording(true);
        backend.begin_frame(Viewport::new(NARROW_W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar_icons(
                QRect::new(0.0, 0.0, NARROW_W as f32, H as f32),
                &bar,
                &icons,
                None,
            );
            *hits.borrow_mut() = Some(h);
        });
        let hits = hits.into_inner().unwrap();
        let runs = backend.text_runs().to_vec();
        backend.end_frame();

        assert_eq!(
            hits.slot_positions.len(),
            1,
            "the narrow strip must still show the decorated tab"
        );

        let icon_run = runs
            .iter()
            .find(|r| r.text == "R")
            .expect("the icon glyph must be painted");
        let label_run = runs
            .iter()
            .find(|r| {
                r.text != "R" && r.text != crate::codicon::CLOSE.to_string() && r.text.contains('…')
            })
            .unwrap_or_else(|| {
                panic!(
                    "expected an ellipsis-truncated label run among painted runs, got {:?}",
                    runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
                )
            });
        assert!(
            label_run.text.ends_with('…'),
            "label run {:?} must end in an ellipsis",
            label_run.text
        );
        assert!(
            (label_run.bounds.x as f64) > (icon_run.bounds.x + icon_run.bounds.width) as f64,
            "label run {:?} must start after the icon run {:?}",
            label_run,
            icon_run,
        );

        let (close_lo, close_hi) =
            hits.close_bounds[0].expect("a closable tab must still report a close-button hit box");
        assert!(
            close_hi <= NARROW_W,
            "close hit box [{close_lo}, {close_hi}] must stay inside the {NARROW_W}-wide strip"
        );
        let close_run = runs
            .iter()
            .find(|r| r.text.contains(crate::codicon::CLOSE))
            .expect("the close glyph must still be painted");
        assert!(
            (close_run.bounds.x as f64) >= 0.0
                && (close_run.bounds.x + close_run.bounds.width) as f64 <= NARROW_W,
            "close glyph run {close_run:?} must be fully visible inside the {NARROW_W}-wide strip"
        );
    }
}
