//! GTK rasteriser for [`crate::ListView`].
//!
//! Content painting (background, title, rows, h/v scrollbars) moved to
//! the shared [`crate::primitives::list::native_surface_paint::paint`]
//! (#1075, `NativeSurface` Phase 4 slice 2/8) — see that fn's module
//! doc for what's shared and what stays per-backend. This module still
//! owns the [`ListView::bordered`] frame itself: a rounded-rectangle
//! clip + stroke (3px corner radius) around the shared content paint,
//! since no `NativeSurface` verb exists for a rounded stroke and GTK's
//! rounded frame is a real, documented visual divergence from Windows'
//! square one and macOS's absent one (see the shared `paint`'s module
//! doc).
//!
//! [`gtk_list_layout`] and the deprecated [`draw_list`] compatibility
//! shim live over the shared [`super::surface::CairoSurface`] adapter
//! (#1072).

use gtk4::cairo::Context;
use gtk4::pango;

use super::{cairo_rgb, rounded_rect_path};
use crate::primitives::list::{ListView, ListViewLayout};
use crate::theme::Theme;

/// Compute the GTK pixel-unit layout for a [`ListView`] without painting —
/// the same viewport reservation (border inset, h-scrollbar row)
/// [`draw_list`] applies internally before calling [`ListView::layout`].
/// `draw_list` calls this exact function (with its own live-measured
/// `char_width`) so paint and no-paint hit-testing can never drift apart
/// (`PRIMITIVE_RULES.md` rule 5).
///
/// The reservation math itself lives in
/// [`crate::primitives::layout_metrics::list_layout`] (#712) — shared
/// with `mac_list_layout` so both backends reserve the h-scrollbar row
/// identically instead of each carrying its own copy that can drift.
///
/// `char_width` is used only for the h-scrollbar-overflow threshold check
/// (`ListView::max_content_width` is in character columns); pass
/// [`crate::Backend::char_width`]'s cached value when no live
/// `pango::Layout` is available — the same approximation
/// `GtkBackend::list_hscrollbar` already uses for this exact check.
///
/// Coordinate frame: **LOCAL** — relative to `(0, 0)`, matching
/// `tui_list_layout` / `win_list_layout` / `mac_list_layout`. Does **not**
/// account for [`ListView::bordered`]'s 1px border inset — same as every
/// other backend's list-layout helper; a bordered list's caller adds that
/// inset itself (see `draw_list`'s `item_x_offset` / `item_y_offset`).
pub fn gtk_list_layout(
    w: f64,
    h: f64,
    list: &ListView,
    line_height: f64,
    char_width: f64,
) -> ListViewLayout {
    let border_inset: f64 = if list.bordered { 1.0 } else { 0.0 };
    crate::primitives::layout_metrics::list_layout(
        list,
        w,
        h,
        line_height,
        char_width,
        border_inset,
    )
}

/// Draw a [`ListView`] into `(x, y, w, h)` on `cr`.
///
/// Caller owns `layout`'s font choice — the rasteriser doesn't
/// switch fonts. `nerd_fonts_enabled` controls which icon variant
/// the consumer's icon registry exposes; pass `false` to always
/// use the ASCII fallback.
///
/// # Visual contract
///
/// - **Background:** [`Theme::background`] (matches the editor surface
///   the list is embedded in).
/// - **Optional title:** painted as a flat [`Theme::header_bg`] /
///   [`Theme::header_fg`] strip at the top.
/// - **Selected row:** [`Theme::selected_bg`] background and a `▶`
///   selection prefix.
/// - **Header decoration:** items with [`crate::types::Decoration::Header`]
///   use [`Theme::header_bg`] / [`Theme::header_fg`] (used by the source
///   control panel for section titles).
/// - **Per-item decoration → fg:** `Error → error_fg`, `Warning →
///   warning_fg`, `Muted → muted_fg`, `Header → header_fg`, others
///   → [`Theme::surface_fg`].
/// - **Detail span:** right-aligned in [`Theme::muted_fg`], skipped
///   when there isn't room past the main text.
/// - **Vertical / horizontal scrollbar:** painted via
///   [`crate::primitives::scrollbar::native_surface_paint::paint`] when
///   [`ListView::show_v_scrollbar`] / an overflowing
///   [`ListView::max_content_width`] call for one (#1075).
#[allow(clippy::too_many_arguments)]
pub fn draw_list(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    list: &ListView,
    theme: &Theme,
    line_height: f64,
    nerd_fonts_enabled: bool,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    layout.set_attributes(None);

    // Measure a reference glyph for char-to-pixel conversion.  `h_scroll` is
    // expressed in character columns (same unit as TUI cells); GTK works in
    // pixels, so the shared paint multiplies by this before subtracting from
    // cursor_x — mirrored here only for `gtk_list_layout`'s own char-width
    // parameter, which must agree with what the shared paint measures.
    layout.set_text("M");
    let (cw_px, _) = layout.pixel_size();
    let char_w = cw_px.max(1) as f64;

    let list_layout = gtk_list_layout(w, h, list, line_height, char_w);

    if list.bordered {
        cr.save().ok();
        rounded_rect_path(cr, x, y, w, h, 3.0);
        cr.clip();
    }

    let area = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    crate::primitives::list::native_surface_paint::paint(
        list,
        area,
        &list_layout,
        line_height as f32,
        nerd_fonts_enabled,
        /* supports_border */ true,
        /* supports_hscrollbar */ true,
        &mut surface,
        theme,
    );

    if list.bordered {
        cr.restore().ok();
        rounded_rect_path(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 3.0);
        let border_color = cairo_rgb(theme.border_fg);
        cr.set_source_rgb(border_color.0, border_color.1, border_color.2);
        cr.set_line_width(1.0);
        cr.stroke().ok();
    }

    layout.set_attributes(None);
}

// ── Tests ──────────────────────────────────────────────────────────────────
//
// #1075 regression: before the `native_surface_paint` migration,
// `draw_list` never painted `ListView::show_v_scrollbar`'s track/thumb
// even though `GtkBackend::list_vscrollbar` already exposed real
// geometry for hit-testing/dragging (`list.vscrollbar(...)` — see that
// method's own doc). This test paints through the real `draw_list` into
// a Cairo `ImageSurface` and probes a pixel inside the resolved track
// rect — it would have failed (background colour, nothing painted)
// against the pre-#1075 body.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::list::ListItem;
    use crate::types::{Color, Decoration, StyledText, WidgetId};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    const W: i32 = 120;
    const H: i32 = 100;
    const LINE_HEIGHT: f64 = 10.0;

    fn test_theme() -> Theme {
        Theme {
            background: Color::rgb(255, 255, 255),
            surface_bg: Color::rgb(255, 255, 255),
            ..Theme::default()
        }
    }

    fn item(label: &str) -> ListItem {
        ListItem {
            text: StyledText::plain(label.to_string()),
            icon: None,
            detail: None,
            decoration: Decoration::Normal,
        }
    }

    fn vlist(n: usize) -> ListView {
        ListView {
            id: WidgetId::new("l"),
            title: None,
            items: (0..n).map(|i| item(&format!("row {i}"))).collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: true,
        }
    }

    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    #[test]
    fn gtk_paints_vertical_scrollbar_track_when_enabled() {
        let list = vlist(30); // 30 rows in a 100px / 10px viewport overflow.
        let area = crate::event::Rect::new(0.0, 0.0, W as f32, H as f32);
        let expected = list
            .vscrollbar(area, LINE_HEIGHT as f32)
            .expect("30 rows in a 10-row viewport must need a v-scrollbar");

        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_list(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                W as f64,
                H as f64,
                &list,
                &test_theme(),
                LINE_HEIGHT,
                false,
            );
        }
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let probe_x = (expected.track.x + expected.track.width / 2.0).round() as i32;
        let probe_y = (expected.track.y + expected.track.height / 2.0).round() as i32;
        let (r, g, b) = pixel(
            &data,
            stride,
            probe_x.clamp(0, W - 1),
            probe_y.clamp(0, H - 1),
        );
        assert_ne!(
            (r, g, b),
            (255, 255, 255),
            "expected the v-scrollbar track at ({probe_x}, {probe_y}) to be painted \
             (non-background), got white — the #1075 regression this test guards"
        );
    }
}
