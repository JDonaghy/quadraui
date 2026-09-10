//! GTK settings-chrome rasteriser for [`crate::Form`], plus
//! [`RawFormSurface`].
//!
//! Field-kind *painting* moved to the shared
//! [`crate::primitives::form::paint`] (#808, NativeSurface Phase 2a) —
//! this module now only carries `draw_settings_chrome` (unrelated: form
//! *body* chrome, not field painting) and `RawFormSurface`, the
//! [`crate::native_surface::NativeSurface`] adapter over a raw
//! `(&Context, &pango::Layout)` pair for call sites that have only
//! those — not a live [`super::backend::GtkBackend`] — such as
//! [`crate::gtk::multi_section_view`]'s embedded-`Form` section body.
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
use crate::native_surface::NativeSurface;
use crate::theme::Theme;
use crate::Form;

/// See this module's doc.
///
/// Frame-lifecycle / metrics verbs are unreachable from a raw
/// `(&Context, &pango::Layout)` pair (there is no backend to ask);
/// `paint` never calls them (it only fills, draws text, and measures
/// text), so they panic if ever called — a latent contract, not a live
/// gap.
pub(crate) struct RawFormSurface<'a> {
    pub(crate) cr: &'a Context,
    pub(crate) layout: &'a pango::Layout,
}

impl NativeSurface for RawFormSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawFormSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawFormSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawFormSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawFormSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawFormSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.layout.set_text(text);
        self.layout.set_attributes(None);
        let (w, h) = self.layout.pixel_size();
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        super::set_source(self.cr, color);
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.fill().ok();
    }

    fn surface_stroke_rect(&mut self, rect: crate::Rect, color: crate::Color, stroke_width: f32) {
        super::set_source(self.cr, color);
        self.cr.set_line_width(stroke_width as f64);
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.stroke().ok();
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        self.layout.set_text(text);
        self.layout.set_attributes(None);
        super::set_source(self.cr, color);
        self.cr.move_to(rect.x as f64, rect.y as f64);
        super::painted_text::show_layout(self.cr, self.layout);
    }

    fn surface_draw_line(
        &mut self,
        from: crate::Point,
        to: crate::Point,
        color: crate::Color,
        stroke_width: f32,
    ) {
        super::set_source(self.cr, color);
        self.cr.set_line_width(stroke_width as f64);
        self.cr.move_to(from.x as f64, from.y as f64);
        self.cr.line_to(to.x as f64, to.y as f64);
        self.cr.stroke().ok();
    }

    fn surface_push_clip(&mut self, rect: crate::Rect) {
        self.cr.save().ok();
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.clip();
    }

    fn surface_pop_clip(&mut self) {
        self.cr.restore().ok();
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        crate::backend::ImagePaintResult::Unsupported
    }
}

/// Deprecated free-function shim (#808, CLAUDE.md rule 8): `draw_form`
/// used to be this module's whole reason to exist — every `FieldKind`
/// match arm lived directly in its body. Painting now goes through
/// [`crate::primitives::form::paint`] via [`RawFormSurface`]; this
/// wrapper reproduces the old signature exactly (same geometry, same
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
    note = "call `Backend::draw_form` (or `crate::primitives::form::paint` with a `RawFormSurface`) instead — this free function is a compatibility shim over the shared #808 implementation"
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
    // `false`: this is the deprecated `draw_form` shim, which reproduces
    // the pre-#808 signature exactly and predates `nerd_fonts_enabled`
    // entirely — there's no flag for a caller of this shim to have
    // passed. The live path is `GtkBackend::form_layout`, which forwards
    // its own `self.nerd_fonts_enabled` (issue #913 review fix).
    let flayout = form.layout(w as f32, h as f32, |i| {
        crate::primitives::layout_metrics::form_field_measure(
            &form.fields[i],
            row_h,
            &measure,
            false,
        )
    });
    let origin = crate::Point::new(x as f32, y as f32);
    let mut surface = RawFormSurface { cr, layout };
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
            // `false`: this is the deprecated `draw_form` shim, which
            // reproduces the pre-#808 signature exactly and predates
            // `nerd_fonts_enabled` entirely — there's no flag for a
            // caller of this shim to have passed. The live path is
            // `GtkBackend::draw_form`, which forwards its own
            // `self.nerd_fonts_enabled` (issue #913 review fix).
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
                false,
            );
            layout.set_attributes(None);
        }
    }
}

/// Settings panel chrome: a 2-row strip with a header row and a search
/// input row, designed to sit immediately above a [`Form`] body.
///
/// Total chrome height = `2 * line_height` pixels — the first
/// `line_height` is the header (`header_bg` / `header_fg`), the second
/// is the search input (full-width tinted `selected_bg` when `active`,
/// otherwise the panel `tab_bar_bg`). Layout from left to right inside
/// the search row: ` /  ` prefix in `muted_fg`, then either `query` (in
/// `foreground`) or `placeholder` (in `muted_fg`) when the query is
/// empty + inactive. A 1.5px-wide `accent_fg` cursor follows the query
/// when `active`.
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

    // Row 1: search input.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gtk::toolbar::PangoMeasure;
    use crate::primitives::form::{FieldKind, Form, FormField};
    use crate::types::{StyledText, WidgetId};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    /// Paint `form` via the shared [`crate::primitives::form::paint`]
    /// through a [`RawFormSurface`] over a fresh in-memory
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
            crate::primitives::layout_metrics::form_field_measure(
                &form.fields[i],
                row_h,
                &measure,
                false,
            )
        });
        let mut raw = RawFormSurface {
            cr: &cr,
            layout: &pango_layout,
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
                    false,
                )
            });
            let mut raw = RawFormSurface {
                cr: &cr,
                layout: &pango_layout,
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
}
