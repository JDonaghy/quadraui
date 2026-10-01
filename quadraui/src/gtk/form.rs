//! GTK settings-chrome rasteriser for [`crate::Form`].
//!
//! Field-kind *painting* moved to the shared
//! [`crate::primitives::form::paint`] (#808, NativeSurface Phase 2a) —
//! this module now only carries `draw_settings_chrome` (unrelated: form
//! *body* chrome, not field painting) and the deprecated [`draw_form`]
//! shim, both over the shared [`super::surface::CairoSurface`] adapter
//! (#1072 — consolidated from this module's own private
//! `RawFormSurface`, which also served call sites with only a raw
//! `(&Context, &pango::Layout)` pair — not a live
//! [`super::backend::GtkBackend`] — such as
//! [`crate::gtk::multi_section_view`]'s embedded-`Form` section body;
//! those now build a [`super::surface::CairoSurface`] directly).
//!
//! Before #808, this module's own `draw_form` painted from an ad-hoc
//! running cursor independent of the shared [`crate::Form::layout`]
//! geometry `GtkBackend::form_layout` used for hit-testing, and left
//! `Slider` / `ColorPicker` / `Dropdown` blank. The shared `paint`
//! fixes both: it paints from the same `FormLayout` hit-testing uses,
//! and handles all 14 `FieldKind` variants.

use gtk4::cairo::Context;
use gtk4::pango;

use super::cairo_rgb;
use crate::theme::Theme;
use crate::Form;

/// Deprecated free-function shim (#808, CLAUDE.md rule 8): `draw_form`
/// used to be this module's whole reason to exist — every `FieldKind`
/// match arm lived directly in its body. Painting now goes through
/// [`crate::primitives::form::paint`] via [`super::surface::CairoSurface`];
/// this wrapper reproduces the old signature exactly (same geometry, same
/// paint contract) for any external caller that held a direct
/// `quadraui::gtk::draw_form` reference rather than going through
/// [`crate::Backend::draw_form`] — the sanctioned entry point, and the
/// one every in-tree call site already uses, which is why this shim has
/// no in-repo caller left to trip the `-D warnings`-denied `deprecated`
/// lint. `FieldKind::Toolbar` renders through the full toolbar
/// rasteriser here too, matching pre-#808 behaviour, for the same
/// reason `GtkBackend::draw_form` does (see that method's doc).
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_form` (or `crate::primitives::form::paint` with a `super::surface::CairoSurface`) instead — this free function is a compatibility shim over the shared #808 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub fn draw_form(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    form: &Form,
    theme: &Theme,
    line_height: f64,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
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

/// Settings panel chrome: a header row and, when `height` leaves room
/// for it, a search input row beneath it — designed to sit immediately
/// above a [`Form`] body.
///
/// The header row always paints at `line_height` tall. The search row
/// paints only when `height >= line_height * 1.5` (issue #1041 review):
/// a caller that reserves a single row (e.g.
/// [`crate::compose::sidebar_panel_body::SidebarPanelChrome::Header`])
/// gets a header-only strip instead of a second, unrequested row
/// overpainting whatever the caller placed directly beneath it. When
/// the search row does paint, its full-width tinted background is
/// `selected_bg` when `active`, otherwise the panel `tab_bar_bg`.
/// Layout from left to right inside the search row: ` /  ` prefix in
/// `muted_fg`, then either `query` (in `foreground`) or `placeholder`
/// (in `muted_fg`) when the query is empty + inactive. A 1.5px-wide
/// `accent_fg` cursor follows the query when `active`.
///
/// Chrome only — the form body and any scrollbar layered below are
/// painted separately by the caller.
#[allow(clippy::too_many_arguments)]
pub fn draw_settings_chrome(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    height: f64,
    line_height: f64,
    header_text: &str,
    query: &str,
    placeholder: &str,
    active: bool,
    theme: &Theme,
) {
    if w <= 0.0 || line_height <= 0.0 {
        return;
    }

    let bg = cairo_rgb(theme.tab_bar_bg);
    let hdr_bg = cairo_rgb(theme.header_bg);
    let hdr_fg = cairo_rgb(theme.header_fg);
    let fg = cairo_rgb(theme.foreground);
    let dim = cairo_rgb(theme.muted_fg);
    let sel = cairo_rgb(theme.selected_bg);
    let accent = cairo_rgb(theme.accent_fg);

    layout.set_attributes(None);

    // Row 0: header bar.
    cr.set_source_rgb(hdr_bg.0, hdr_bg.1, hdr_bg.2);
    cr.rectangle(x, y, w, line_height);
    cr.fill().ok();
    cr.set_source_rgb(hdr_fg.0, hdr_fg.1, hdr_fg.2);
    layout.set_text(header_text);
    let (_, header_lh) = layout.pixel_size();
    cr.move_to(
        x + 2.0,
        (y + (line_height - header_lh as f64) / 2.0).round(),
    );
    super::painted_text::show_layout(cr, layout);

    // Row 1: search input — only when `height` leaves room for it (see
    // this fn's doc). Skipped for a header-only strip instead of
    // overpainting whatever the caller placed directly beneath it.
    if height >= line_height * 1.5 {
        let search_y = y + line_height;
        let (sb_r, sb_g, sb_b) = if active { sel } else { bg };
        cr.set_source_rgb(sb_r, sb_g, sb_b);
        cr.rectangle(x, search_y, w, line_height);
        cr.fill().ok();

        let prefix = " /  ";
        cr.set_source_rgb(dim.0, dim.1, dim.2);
        layout.set_text(prefix);
        let (prefix_w, _) = layout.pixel_size();
        cr.move_to(
            x + 2.0,
            (search_y + (line_height - header_lh as f64) / 2.0).round(),
        );
        super::painted_text::show_layout(cr, layout);

        let q_x = x + 2.0 + prefix_w as f64;
        let show_placeholder = query.is_empty() && !placeholder.is_empty() && !active;
        let (text, color) = if show_placeholder {
            (placeholder, dim)
        } else if query.is_empty() {
            (query, dim)
        } else {
            (query, fg)
        };
        cr.set_source_rgb(color.0, color.1, color.2);
        layout.set_text(text);
        let (q_w, _) = layout.pixel_size();
        cr.move_to(
            q_x,
            (search_y + (line_height - header_lh as f64) / 2.0).round(),
        );
        super::painted_text::show_layout(cr, layout);

        if active {
            let cur_x = q_x + if query.is_empty() { 0.0 } else { q_w as f64 };
            cr.set_source_rgb(accent.0, accent.1, accent.2);
            cr.rectangle(cur_x, search_y + 2.0, 1.5, line_height - 4.0);
            cr.fill().ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gtk::toolbar::PangoMeasure;
    use crate::primitives::form::{FieldKind, Form, FormField};
    use crate::types::{StyledText, WidgetId};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    /// Paint `form` via the shared [`crate::primitives::form::paint`]
    /// through a [`crate::gtk::surface::CairoSurface`] over a fresh in-memory
    /// `ImageSurface` — the same adapter `gtk::multi_section_view`'s
    /// embedded-`Form` body uses. Builds its own [`crate::FormLayout`]
    /// with the shared `form_field_measure` (the same measurer
    /// `GtkBackend::form_layout` uses) so painted rows match what
    /// hit-testing would resolve.
    fn paint(form: &Form) {
        let surface = ImageSurface::create(Format::ARgb32, 320, 160).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);
        let theme = Theme::default();
        let row_h = crate::primitives::layout_metrics::form_row_height(14.0);
        let measure = PangoMeasure {
            pango_layout: Some(&pango_layout),
            char_width: 8.0,
        };
        let flayout = form.layout(320.0, 160.0, |i| {
            crate::primitives::layout_metrics::form_field_measure(&form.fields[i], row_h, &measure)
        });
        let mut raw = crate::gtk::surface::CairoSurface {
            cr: &cr,
            layout: Some(&pango_layout),
            translucent_fill: false,
        };
        crate::primitives::form::paint(
            form,
            &flayout,
            &mut raw,
            &theme,
            crate::Point::new(0.0, 0.0),
        );
    }

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    /// Cairo's `ARgb32` stores each pixel as four bytes in native
    /// (little-endian) byte order: [B, G, R, A] — mirrors
    /// `gtk::terminal`/`gtk::activity_bar`'s test-local helper of the
    /// same name.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    /// Regression for issue #710's secondary finding: `draw_form` used
    /// to iterate `form.fields.iter().skip(form.scroll_offset)` on the
    /// **raw** `scroll_offset`, while `Form::layout` (the hit-test path
    /// `GtkBackend::form_layout` drives) clamps it via
    /// `clamp_scroll_offset`. At an out-of-range offset the two
    /// disagreed about which field (if any) occupies row 0: the raw
    /// `.skip()` walked past every field and painted nothing, while the
    /// clamped layout still resolved row 0 to the last field.
    #[test]
    fn draw_form_clamps_out_of_range_scroll_offset_like_form_layout() {
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![
                FormField {
                    id: WidgetId::new("hdr"),
                    label: StyledText::plain("Header"),
                    kind: FieldKind::Label,
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("enabled"),
                    label: StyledText::plain("Enabled"),
                    kind: FieldKind::Toggle { value: true },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
            ],
            // Focused so the resolved field paints `selected_bg` —
            // distinct from both the plain background and the header's
            // `header_bg`, so row 0's colour unambiguously identifies
            // which field (if any) painted there.
            focused_field: Some(WidgetId::new("enabled")),
            scroll_offset: 50, // far past `fields.len()` == 2
            has_focus: true,
        };

        let mut surface =
            ImageSurface::create(Format::ARgb32, 320, 160).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            let theme = Theme::default();
            let row_h = crate::primitives::layout_metrics::form_row_height(14.0);
            let measure = PangoMeasure {
                pango_layout: Some(&pango_layout),
                char_width: 8.0,
            };
            let flayout = form.layout(320.0, 160.0, |i| {
                crate::primitives::layout_metrics::form_field_measure(
                    &form.fields[i],
                    row_h,
                    &measure,
                )
            });
            let mut raw = crate::gtk::surface::CairoSurface {
                cr: &cr,
                layout: Some(&pango_layout),
                translucent_fill: false,
            };
            crate::primitives::form::paint(
                &form,
                &flayout,
                &mut raw,
                &theme,
                crate::Point::new(0.0, 0.0),
            );
        }

        let theme = Theme::default();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (r, g, b) = pixel(&data, stride, 4, 4);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "row 0 should paint the focused 'enabled' field (the field \
             `clamp_scroll_offset` resolves to), not leave the background \
             untouched the way the pre-fix raw `.skip()` did",
        );
    }

    /// Regression for issue #503: `TextInput.cursor`/`selection_anchor`
    /// are host-supplied byte offsets with no guarantee they land on a
    /// char boundary — `&shown[lo..hi]` / `&shown[..cur]` used to panic
    /// the moment a multibyte character sat inside the range.
    #[test]
    fn text_input_with_multibyte_cursor_and_selection_does_not_panic() {
        // "café🎉" — byte 4 sits inside 'é' (starts at byte 3); byte 7
        // sits inside the emoji (starts at byte 6).
        let value = "café🎉";
        assert!(!value.is_char_boundary(4));
        assert!(!value.is_char_boundary(7));
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("name"),
                label: StyledText::plain("Name"),
                kind: FieldKind::TextInput {
                    value: value.into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    selection_anchor: Some(7),
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
        };

        // Must not panic.
        paint(&form);
    }

    /// Regression for issue #503: same class of bug in the
    /// `PasswordInput` byte-cursor-to-mask-char-position conversion.
    #[test]
    fn password_input_with_multibyte_cursor_does_not_panic() {
        let value = "café🎉";
        assert!(!value.is_char_boundary(4));
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("pw"),
                label: StyledText::plain("Password"),
                kind: FieldKind::PasswordInput {
                    value: value.into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    mask_char: '*',
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: Some(WidgetId::new("pw")),
            scroll_offset: 0,
            has_focus: true,
        };

        // Must not panic.
        paint(&form);
    }

    /// Regression for issue #503: `TextArea` clamps `cursor` against
    /// `first_line.len()` but the earlier code sliced `shown` (which may
    /// be `first_line` or a differently-lengthed placeholder) without a
    /// char-boundary snap.
    #[test]
    fn text_area_with_multibyte_cursor_does_not_panic() {
        let value = "café🎉\nsecond line";
        assert!(!value.is_char_boundary(4));
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("notes"),
                label: StyledText::plain("Notes"),
                kind: FieldKind::TextArea {
                    value: value.into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    visible_rows: 3,
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: Some(WidgetId::new("notes")),
            scroll_offset: 0,
            has_focus: true,
        };

        // Must not panic.
        paint(&form);
    }

    // ─── issue #1041 review: height-aware `draw_settings_chrome` ────────

    const CHROME_W: i32 = 200;
    const CHROME_H: i32 = 60;
    const LINE_HEIGHT: f64 = 18.0;
    // Distinct from every colour `draw_settings_chrome` itself paints
    // (header_bg/tab_bar_bg/selected_bg), so a search-row pixel that
    // still reads this sentinel unambiguously means "nothing painted
    // here", not "painted a colour that happens to match by chance."
    const SENTINEL: (u8, u8, u8) = (1, 2, 3);

    /// Paint `draw_settings_chrome` into a `CHROME_W`x`CHROME_H` surface
    /// pre-filled with [`SENTINEL`], at the given `height`, and return
    /// the raw pixel buffer + stride for probing.
    fn paint_chrome(height: f64) -> (Vec<u8>, usize) {
        let mut surface =
            ImageSurface::create(Format::ARgb32, CHROME_W, CHROME_H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let (sr, sg, sb) = (
                SENTINEL.0 as f64 / 255.0,
                SENTINEL.1 as f64 / 255.0,
                SENTINEL.2 as f64 / 255.0,
            );
            cr.set_source_rgb(sr, sg, sb);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let theme = Theme::default();
            draw_settings_chrome(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                CHROME_W as f64,
                height,
                LINE_HEIGHT,
                "HEADER",
                "",
                "placeholder",
                false,
                &theme,
            );
        }
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data").to_vec();
        (data, stride)
    }

    /// A 1-row-tall `height` (the shape
    /// [`crate::compose::sidebar_panel_body::SidebarPanelChrome::Header`]
    /// reserves) must leave the would-be search row untouched — the
    /// issue #1041 review finding: GTK used to always paint a second row
    /// regardless of the rect it was given, overpainting whatever the
    /// caller placed directly beneath a "header-only" chrome strip.
    #[test]
    fn draw_settings_chrome_one_row_height_paints_no_search_row() {
        let (data, stride) = paint_chrome(LINE_HEIGHT);

        // Probe x=150, clear of the left-aligned "HEADER" glyphs (same
        // convention `settings_chrome_paints_header_and_inactive_search_rows`
        // in `win::form`'s tests uses).
        let probe_x = 150;

        // Sample inside where row 1 (search) would have started —
        // comfortably past the header row, comfortably before the
        // surface's bottom edge.
        let probe_y = (LINE_HEIGHT * 1.5) as i32;
        assert_eq!(
            pixel(&data, stride, probe_x, probe_y),
            SENTINEL,
            "a 1-row-tall chrome rect must not paint a search row past its own height"
        );

        // The header row itself must still have painted.
        let theme = Theme::default();
        assert_eq!(
            pixel(&data, stride, probe_x, (LINE_HEIGHT / 2.0) as i32),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
            "the header row must still paint even when the search row doesn't"
        );
    }

    /// A full 2-row-tall `height` (the shape
    /// [`crate::compose::sidebar_panel_body::SidebarPanelChrome::HeaderAndSearch`]
    /// reserves) must still paint the search row — the height-aware
    /// fix must not regress the documented 2-row shape.
    #[test]
    fn draw_settings_chrome_two_row_height_paints_search_row() {
        let (data, stride) = paint_chrome(LINE_HEIGHT * 2.0);

        let theme = Theme::default();
        let probe_y = (LINE_HEIGHT * 1.5) as i32;
        assert_eq!(
            pixel(&data, stride, 4, probe_y),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "a 2-row-tall chrome rect must still paint the inactive search row"
        );
    }
}
