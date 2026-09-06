//! macOS layout + settings-chrome rasteriser for [`crate::Form`].
//!
//! Field-kind *painting* moved to the shared
//! [`crate::primitives::form::paint`] (#808, NativeSurface Phase 2a) —
//! `mac_form_layout` here only computes geometry (used both for
//! hit-testing via `MacBackend::form_layout` and to feed `paint` from
//! `MacBackend::draw_form`). Before #808, this module's own `draw_form`
//! matched only 10 of 14 `FieldKind` variants and silently fell through
//! on `Slider` / `ColorPicker` / `Dropdown` / `TextArea`; the shared
//! `paint` handles all 14, so that gap can't recur here.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as QRect;
use crate::native_surface::NativeSurface;
use crate::primitives::form::{Form, FormLayout};
use crate::primitives::layout_metrics::TextMeasure;
use crate::theme::Theme;
use crate::types::Color;

/// Adapts a live `CTFont` to the shared [`TextMeasure`] trait so
/// [`crate::primitives::layout_metrics::form_field_measure`] never has
/// to name a Core Text type.
struct CtFontMeasure<'a>(&'a CTFont);

impl TextMeasure for CtFontMeasure<'_> {
    fn width_of(&self, text: &str) -> f32 {
        measure_text(self.0, text).0 as f32
    }
}

/// Compute the layout the macOS rasteriser would produce for `form`
/// in `area` at `line_height`. Rows are `(line_height * 1.4).round()`
/// tall.
///
/// Per-item measurement (`ToggleGroup`, `SegmentedControl`,
/// `ButtonRow`) uses `font` so paint and hit-test agree on each
/// item's x position. Mirrors the per-`FieldKind` measurement in
/// [`crate::gtk::backend`]'s `form_layout` impl.
///
/// Coordinate frame: `visible_fields.bounds`, `item_bounds`, and
/// `hit_regions` are in **form-local** coords (origin at 0, 0),
/// matching `gtk_form_layout` and the `tree_layout` contract. Hosts
/// (e.g. `SidebarSystem`, `FormController`) subtract `area.x`/`area.y`
/// from absolute click coords before calling
/// [`FormLayout::hit_test`].
pub fn mac_form_layout(form: &Form, area: QRect, line_height: f64, font: &CTFont) -> FormLayout {
    let row_h = crate::primitives::layout_metrics::form_row_height(line_height);
    let measure = CtFontMeasure(font);
    form.layout(area.width, area.height, |i| {
        crate::primitives::layout_metrics::form_field_measure(&form.fields[i], row_h, &measure)
    })
}

/// Minimal [`NativeSurface`] adapter over a raw `(CGContextRef, &CTFont)`
/// pair, for [`crate::primitives::form::paint`] call sites that have
/// only those — not a live [`super::MacBackend`] — such as
/// [`crate::macos::multi_section_view`]'s embedded-`Form` section body.
///
/// Frame-lifecycle / measurement-metrics verbs are unreachable from a
/// bare `CGContextRef` (there is no backend to ask), so they panic if
/// ever called — `paint` never calls them (it only fills, draws text,
/// and measures text), so this is a latent contract, not a live gap.
pub(crate) struct RawFormSurface<'a> {
    pub(crate) ctx: CGContextRef,
    pub(crate) font: &'a CTFont,
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
        let (w, h) = measure_text(self.font, text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's
        // paint pass — see this struct's construction sites.
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
    }

    fn surface_stroke_rect(&mut self, rect: crate::Rect, color: Color, stroke_width: f32) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_stroke_rect(self.ctx, rect, color, stroke_width as f64) };
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: Color) {
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            draw_text(
                self.ctx,
                self.font,
                text,
                rect.x as f64,
                rect.y as f64,
                super::backend::ns_color_to_cg(color),
            );
        }
    }

    fn surface_draw_line(
        &mut self,
        from: crate::Point,
        to: crate::Point,
        color: Color,
        stroke_width: f32,
    ) {
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            super::backend::ns_draw_line(
                self.ctx,
                from.x as f64,
                from.y as f64,
                to.x as f64,
                to.y as f64,
                color,
                stroke_width as f64,
            );
        }
    }

    fn surface_push_clip(&mut self, rect: crate::Rect) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_push_clip(self.ctx, rect) };
    }

    fn surface_pop_clip(&mut self) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_pop_clip(self.ctx) };
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        // Matches `MacBackend::draw_image`: no `NSImage` decoder wired
        // up yet (#802).
        crate::backend::ImagePaintResult::Unsupported
    }
}

/// Deprecated free-function shim (#808, CLAUDE.md rule 8): this module's
/// `draw_form` used to match every `FieldKind` directly, and matched
/// only 10 of 14 (see the module doc). Painting now goes through
/// [`crate::primitives::form::paint`] via [`RawFormSurface`]; this
/// wrapper reproduces the old signature exactly for any external caller
/// that held a direct `quadraui::macos::draw_form` reference rather than
/// going through [`crate::Backend::draw_form`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the
/// `-D warnings`-denied `deprecated` lint. `FieldKind::Toolbar` renders
/// through the full toolbar rasteriser here too, matching pre-#808
/// behaviour, for the same reason `MacBackend::draw_form` does (see that
/// method's doc).
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_form` (or `crate::primitives::form::paint` with a `RawFormSurface`) instead — this free function is a compatibility shim over the shared #808 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_form(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    form: &Form,
    theme: &Theme,
    line_height: f64,
) -> FormLayout {
    let area = QRect::new(x as f32, y as f32, w as f32, h as f32);
    let flayout = mac_form_layout(form, area, line_height, font);
    if w <= 0.0 || h <= 0.0 {
        return flayout;
    }

    let origin = crate::Point::new(x as f32, y as f32);
    let mut surface = RawFormSurface { ctx, font };
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
            // SAFETY: `ctx` is valid for the duration of this call, per
            // this fn's own contract.
            super::toolbar::draw_toolbar(
                ctx, font, toolbar_x, row_y, toolbar_w, row_h, toolbar, theme, None, None,
            );
        }
    }

    flayout
}

/// Cursor width in points for the settings-chrome search row. Matches
/// the GTK twin's 1.5px caret.
const SETTINGS_CURSOR_W: f64 = 1.5;
/// Prefix rendered before the settings search query.
const SETTINGS_SEARCH_PREFIX: &str = " /  ";

/// Draw settings-panel chrome: a 2-row strip with a header row and a
/// search input row, designed to sit immediately above a [`Form`] body.
///
/// Port of [`crate::gtk::form::draw_settings_chrome`] — same two-row
/// layout, same `" /  "` prefix, same placeholder rule (shown only when
/// the query is empty *and* the row is inactive), same accent caret when
/// active.
///
/// Chrome only: the form body and any scrollbar layered below are painted
/// separately by the caller.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call (typical: the frame-scope pointer stashed on [`super::MacBackend`]).
/// Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_settings_chrome(
    ctx: CGContextRef,
    font: &CTFont,
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

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, line_height * 2.0));

    // Row 0: header bar.
    fill_rect(ctx, x, y, w, line_height, theme.header_bg);
    draw_text(
        ctx,
        font,
        header_text,
        x + 2.0,
        y,
        color_to_cg(theme.header_fg),
    );

    // Row 1: search input.
    let search_y = y + line_height;
    let row_bg = if active {
        theme.selected_bg
    } else {
        theme.tab_bar_bg
    };
    fill_rect(ctx, x, search_y, w, line_height, row_bg);

    draw_text(
        ctx,
        font,
        SETTINGS_SEARCH_PREFIX,
        x + 2.0,
        search_y,
        color_to_cg(theme.muted_fg),
    );
    let (prefix_w, _) = measure_text(font, SETTINGS_SEARCH_PREFIX);
    let q_x = x + 2.0 + prefix_w;

    let show_placeholder = query.is_empty() && !placeholder.is_empty() && !active;
    let (text, color) = if show_placeholder {
        (placeholder, theme.muted_fg)
    } else if query.is_empty() {
        (query, theme.muted_fg)
    } else {
        (query, theme.foreground)
    };
    draw_text(ctx, font, text, q_x, search_y, color_to_cg(color));

    if active {
        let (q_w, _) = measure_text(font, query);
        let cur_x = q_x + if query.is_empty() { 0.0 } else { q_w };
        fill_rect(
            ctx,
            cur_x,
            search_y + 2.0,
            SETTINGS_CURSOR_W,
            line_height - 4.0,
            theme.accent_fg,
        );
    }

    CGContextRestoreGState(ctx);
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::Viewport;
    use crate::primitives::form::{ButtonRowItem, FieldKind, FormField, FormHit, ToggleGroupItem};
    use crate::theme::Theme;
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 160;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn label(id: &str, text: &str) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(text),
            kind: FieldKind::Label,
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn text_input(id: &str, label_text: &str, value: &str) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label_text),
            kind: FieldKind::TextInput {
                value: value.into(),
                placeholder: String::new(),
                cursor: Some(value.len()),
                selection_anchor: None,
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn toggle(id: &str, label_text: &str, value: bool) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label_text),
            kind: FieldKind::Toggle { value },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn sample_form() -> Form {
        Form {
            id: WidgetId::new("settings"),
            fields: vec![
                label("hdr", "General"),
                text_input("name", "Name", "alice"),
                toggle("enabled", "Enabled", true),
            ],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
        }
    }

    fn paint_via_backend(form: &Form) -> (BitmapSurface, FormLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_form(QRect::new(0.0, 0.0, W as f32, H as f32), form);
            let l = super::mac_form_layout(
                form,
                QRect::new(0.0, 0.0, W as f32, H as f32),
                b.line_height() as f64,
                &font(),
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn focused_field_paints_selected_bg() {
        let form = sample_form();
        let (surface, layout) = paint_via_backend(&form);
        let theme = Theme::default();
        let name = layout
            .visible_fields
            .iter()
            .find(|v| v.id == WidgetId::new("name"))
            .expect("name field visible");
        // Probe near right edge to avoid the label glyph "Name".
        let px = (name.bounds.x + name.bounds.width - 4.0) as u32;
        let py = (name.bounds.y + name.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        // Focused field row gets selected_bg.
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "focused TextInput row should paint selected_bg",
        );
    }

    #[test]
    fn header_field_paints_header_bg() {
        let form = sample_form();
        let (surface, layout) = paint_via_backend(&form);
        let theme = Theme::default();
        let hdr = layout
            .visible_fields
            .iter()
            .find(|v| v.id == WidgetId::new("hdr"))
            .expect("header visible");
        let px = (hdr.bounds.x + hdr.bounds.width - 4.0) as u32;
        let py = (hdr.bounds.y + hdr.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
        );
    }

    #[test]
    fn hit_test_resolves_field_at_painted_centre() {
        let form = sample_form();
        let (_surface, layout) = paint_via_backend(&form);
        for vis in &layout.visible_fields {
            let cx = vis.bounds.x + vis.bounds.width * 0.5;
            let cy = vis.bounds.y + vis.bounds.height * 0.5;
            assert_eq!(
                layout.hit_test(cx, cy),
                FormHit::Field(vis.id.clone()),
                "field {:?} hit-test",
                vis.id,
            );
        }
    }

    #[test]
    fn layout_returns_local_coords_when_area_offset() {
        // Cross-backend contract: visible_fields.bounds, item_bounds,
        // and hit_regions are in form-local coords (origin 0, 0),
        // regardless of where `area` lives. Hosts (SidebarSystem,
        // FormController, AppLogic) subtract area.x/area.y from
        // absolute click coords before hit_test. Matches the
        // `gtk_form_layout` and `mac_tree_layout` contract.
        //
        // Regression for #44 macos_sidebar_search "Find click selects
        // row above": prior to the fix, mac_form_layout shifted
        // hit_regions to absolute coords, causing AppLogic that
        // localised position (per the documented contract) to hit the
        // field at `position.y - 2*area.y` instead of the one under
        // the cursor.
        let form = sample_form();
        // Area offset by (0, 40) — typical when a form lives below an
        // MSV section header.
        let area = QRect::new(0.0, 40.0, 320.0, 120.0);
        let layout = mac_form_layout(&form, area, 16.0, &font());
        // Locality: first field's bounds.y must be 0, not 40.
        let first = &layout.visible_fields[0];
        assert_eq!(
            first.bounds.y, 0.0,
            "visible_fields.bounds.y must be local (0.0), got {}",
            first.bounds.y,
        );
        // Round-trip: simulate a click at the absolute centre of each
        // painted field, localise the way the AppLogic does, and assert
        // it hits the right field. Pre-fix this returned the field N
        // positions earlier.
        for vis in &layout.visible_fields {
            let abs_x = area.x + vis.bounds.x + vis.bounds.width * 0.5;
            let abs_y = area.y + vis.bounds.y + vis.bounds.height * 0.5;
            let local_x = abs_x - area.x;
            let local_y = abs_y - area.y;
            assert_eq!(
                layout.hit_test(local_x, local_y),
                FormHit::Field(vis.id.clone()),
                "field {:?} click → wrong hit (coord-frame drift)",
                vis.id,
            );
        }
    }

    // ── #189 ToggleGroup / SegmentedControl / ButtonRow / PasswordInput ──

    fn toggle_group_field(id: &str, toggles: Vec<ToggleGroupItem>) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(""),
            kind: FieldKind::ToggleGroup { toggles },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn search_flags_form() -> Form {
        // Mirrors the `macos_sidebar_search` shape — three toggles with
        // the middle one ON, no label so items start at the row's left.
        Form {
            id: WidgetId::new("search"),
            fields: vec![toggle_group_field(
                "flags",
                vec![
                    ToggleGroupItem {
                        id: WidgetId::new("case"),
                        label: "Aa".into(),
                        value: false,
                    },
                    ToggleGroupItem {
                        id: WidgetId::new("regex"),
                        label: ".*".into(),
                        value: true,
                    },
                    ToggleGroupItem {
                        id: WidgetId::new("word"),
                        label: "W".into(),
                        value: false,
                    },
                ],
            )],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        }
    }

    #[test]
    fn toggle_group_layout_resolves_per_item_bounds() {
        // ToggleGroup must populate item_bounds so per-toggle clicks
        // dispatch the right `ToggleGroupItem` id rather than the
        // parent field id.
        let form = search_flags_form();
        let (_surface, layout) = paint_via_backend(&form);
        let vis = &layout.visible_fields[0];
        assert_eq!(
            vis.item_bounds.len(),
            3,
            "ToggleGroup should resolve 3 item_bounds",
        );
        let ids: Vec<&WidgetId> = vis.item_bounds.iter().map(|(id, _)| id).collect();
        assert_eq!(
            ids,
            vec![
                &WidgetId::new("case"),
                &WidgetId::new("regex"),
                &WidgetId::new("word"),
            ],
        );
    }

    /// True when at least one pixel *strictly inside* `rect` is exactly
    /// `color`.
    ///
    /// Scanned rather than probed at one hard-coded offset: the only
    /// pixels a solid fill leaves un-blended are the ones no glyph's
    /// antialiasing touches, and where those sit inside an item's rect
    /// depends on the host's font metrics and Core Text's rasteriser —
    /// not on anything this crate controls. A single-offset probe
    /// therefore asserts "this exact pixel is glyph-free" (a fact about
    /// Menlo) on top of the fact under test ("the fill happened"), and
    /// fails on the former the moment the painter nudges a glyph by a
    /// fraction of a point. Same lesson as the macOS selection-highlight
    /// probe.
    ///
    /// The rect is inset one pixel on every side so a *neighbouring*
    /// item's fill (segments butt up against each other with no gap)
    /// can never leak in through a boundary pixel.
    fn region_has_color(surface: &BitmapSurface, rect: &QRect, color: Color) -> bool {
        let x0 = rect.x.ceil() as u32 + 1;
        let x1 = ((rect.x + rect.width).floor() as u32)
            .saturating_sub(1)
            .min(W);
        let y0 = rect.y.ceil() as u32 + 1;
        let y1 = ((rect.y + rect.height).floor() as u32)
            .saturating_sub(1)
            .min(H);
        for y in y0..y1 {
            for x in x0..x1 {
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) == (color.r, color.g, color.b) {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn toggle_group_on_item_paints_selected_bg() {
        // The "on" toggle (regex / `.*`) must paint `selected_bg`
        // behind its rect so it is visually distinguishable from
        // the off toggles at a glance. Regression for #808: the shared
        // `primitives::form::paint` was written from the Windows copy,
        // which never painted this pill, so unifying the three
        // rasterisers silently dropped macOS's on-state affordance.
        let form = search_flags_form();
        let (surface, layout) = paint_via_backend(&form);
        let theme = Theme::default();
        let vis = &layout.visible_fields[0];
        let rect_for = |name: &str| {
            vis.item_bounds
                .iter()
                .find(|(id, _)| id == &WidgetId::new(name))
                .map(|(_, r)| *r)
                .unwrap_or_else(|| panic!("{name} toggle in layout"))
        };
        assert!(
            region_has_color(&surface, &rect_for("regex"), theme.selected_bg),
            "on-toggle should paint selected_bg behind its rect",
        );
        for off in ["case", "word"] {
            assert!(
                !region_has_color(&surface, &rect_for(off), theme.selected_bg),
                "off-toggle {off:?} must not paint the on-state pill",
            );
        }
    }

    #[test]
    fn toggle_group_click_dispatches_individual_toggle_id() {
        // Round-trip the acceptance criteria: click at a known
        // on-toggle's centre, assert hit_test returns the toggle's
        // own WidgetId (not the parent field's id).
        let form = search_flags_form();
        let (_surface, layout) = paint_via_backend(&form);
        for (id, rect) in &layout.visible_fields[0].item_bounds {
            let cx = rect.x + rect.width * 0.5;
            let cy = rect.y + rect.height * 0.5;
            assert_eq!(
                layout.hit_test(cx, cy),
                FormHit::Field(id.clone()),
                "click on toggle {id:?} should resolve to its own id",
            );
        }
    }

    #[test]
    fn segmented_control_selected_paints_selected_bg() {
        let form = Form {
            id: WidgetId::new("seg-form"),
            fields: vec![FormField {
                id: WidgetId::new("scope"),
                label: StyledText::plain(""),
                kind: FieldKind::SegmentedControl {
                    options: vec!["File".into(), "Folder".into(), "Project".into()],
                    selected_idx: 1,
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let (surface, layout) = paint_via_backend(&form);
        let theme = Theme::default();
        let vis = &layout.visible_fields[0];
        // Synthetic per-segment ids `<field>__seg_<idx>` — assert
        // the SELECTED segment paints selected_bg and its neighbours
        // don't (regression for #808: the shared painter filled
        // `hover_bg` here, a hover cue on a segment nothing is
        // hovering).
        let rect_for = |id: &str| {
            vis.item_bounds
                .iter()
                .find(|(item, _)| item.as_str() == id)
                .map(|(_, r)| *r)
                .unwrap_or_else(|| panic!("{id} in layout"))
        };
        assert!(
            region_has_color(&surface, &rect_for("scope__seg_1"), theme.selected_bg),
            "selected segment should paint selected_bg behind its rect",
        );
        for other in ["scope__seg_0", "scope__seg_2"] {
            assert!(
                !region_has_color(&surface, &rect_for(other), theme.selected_bg),
                "unselected segment {other:?} must not paint the selection pill",
            );
        }
    }

    #[test]
    fn button_row_layout_resolves_per_item_bounds() {
        let form = Form {
            id: WidgetId::new("actions-form"),
            fields: vec![FormField {
                id: WidgetId::new("actions"),
                label: StyledText::plain(""),
                kind: FieldKind::ButtonRow {
                    buttons: vec![
                        ButtonRowItem {
                            id: WidgetId::new("find"),
                            label: "Find".into(),
                            disabled: false,
                            icon: None,
                        },
                        ButtonRowItem {
                            id: WidgetId::new("replace"),
                            label: "Replace".into(),
                            disabled: false,
                            icon: None,
                        },
                    ],
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let (_surface, layout) = paint_via_backend(&form);
        let vis = &layout.visible_fields[0];
        assert_eq!(vis.item_bounds.len(), 2);
        // Each item rect should be wide enough to span the bracketed
        // label (e.g. "[Find]" is 6 monospace chars wide).
        for (id, rect) in &vis.item_bounds {
            assert!(
                rect.width >= 6.0 * 4.0,
                "button {id:?} rect width {} too narrow",
                rect.width,
            );
            let hit = layout.hit_test(rect.x + 1.0, rect.y + rect.height * 0.5);
            assert_eq!(
                hit,
                FormHit::Field(id.clone()),
                "click at button {id:?} should hit its own id",
            );
        }
    }

    #[test]
    fn password_input_paints_with_mask_char() {
        // PasswordInput should render `mask_char` repeated `value.chars().count()`
        // times, NOT the plaintext. Verify by computing the expected
        // masked-text width and asserting paint geometry agrees.
        let form = Form {
            id: WidgetId::new("auth-form"),
            fields: vec![FormField {
                id: WidgetId::new("pw"),
                label: StyledText::plain(""),
                kind: FieldKind::PasswordInput {
                    value: "hunter2".into(),
                    placeholder: String::new(),
                    cursor: Some(7),
                    mask_char: '•',
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: Some(WidgetId::new("pw")),
            scroll_offset: 0,
            has_focus: true,
        };
        let (surface, layout) = paint_via_backend(&form);
        // Locate the field bounds.
        let vis = &layout.visible_fields[0];
        // Probe inside the field row at the very centre Y. Pre-fix
        // this pixel would have been background (label-only fallback);
        // post-fix the row contains painted `[`, masked text, `]`.
        // Cheap shape-check: at least ONE pixel inside the row should
        // be foreground (mask glyphs) rather than the focused-row bg.
        let theme = Theme::default();
        let mut saw_glyph = false;
        let y = (vis.bounds.y + vis.bounds.height / 2.0) as u32;
        for x in (vis.bounds.x as u32)..(vis.bounds.x + vis.bounds.width) as u32 {
            let (r, g, b, _) = surface.pixel(x, y);
            // Foreground-ish pixel: differs from both selected_bg (focused row)
            // and pure transparent surface clear.
            let is_bg = (r, g, b)
                == (
                    theme.selected_bg.r,
                    theme.selected_bg.g,
                    theme.selected_bg.b,
                );
            if !is_bg && !(r == 0 && g == 0 && b == 0) {
                saw_glyph = true;
                break;
            }
        }
        assert!(
            saw_glyph,
            "PasswordInput row should paint masked glyphs (none found)",
        );
    }

    /// Regression for issue #503: `TextInput.cursor` is a host-supplied
    /// byte offset with no guarantee it lands on a char boundary —
    /// `&shown[..prefix_byte]` used to panic the moment a multibyte
    /// character sat left of the cursor.
    #[test]
    fn text_input_with_multibyte_cursor_does_not_panic() {
        // "café🎉" — byte 4 sits inside the 2-byte 'é' (starts at byte 3).
        let value = "café🎉";
        assert!(!value.is_char_boundary(4));
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("name"),
                label: StyledText::plain("Name"),
                kind: FieldKind::TextInput {
                    value: value.into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    selection_anchor: None,
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
        let _ = paint_via_backend(&form);
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
        let _ = paint_via_backend(&form);
    }

    // ── Settings chrome (quadraui#484) ──────────────────────────────────

    /// Paint the settings chrome through the real
    /// `Backend::draw_settings_chrome` path at an arbitrary origin.
    fn paint_settings_chrome_at(
        origin: QRect,
        header: &str,
        query: &str,
        placeholder: &str,
        active: bool,
    ) -> (BitmapSurface, f64) {
        let surface = BitmapSurface::new(W, H);
        // Known non-theme background so "did the chrome paint here?" is
        // answerable per pixel.
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let lh = backend.line_height() as f64;
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_settings_chrome(origin, header, query, placeholder, active);
        });
        backend.end_frame();
        (surface, lh)
    }

    #[test]
    fn settings_chrome_paints_header_then_search_row() {
        let (surface, lh) = paint_settings_chrome_at(
            QRect::new(0.0, 0.0, W as f32, H as f32),
            "Settings",
            "",
            "type to filter",
            false,
        );
        let theme = Theme::default();

        // Row 0 = header strip. Probe clear of the "Settings" glyphs.
        let (r, g, b, _) = surface.pixel(W - 4, (lh * 0.5) as u32);
        assert_eq!(
            (r, g, b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
            "row 0 should be the header strip",
        );

        // Row 1 = search input, inactive → `tab_bar_bg`.
        let (r, g, b, _) = surface.pixel(W - 4, (lh * 1.5) as u32);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "row 1 inactive should be the panel background",
        );

        // Row 2 is the form body's job — chrome must not paint it.
        let (r, g, b, _) = surface.pixel(W - 4, (lh * 2.5) as u32);
        assert_eq!(
            (r, g, b),
            (255, 255, 255),
            "settings chrome paints exactly two rows",
        );
    }

    #[test]
    fn settings_chrome_active_row_uses_selected_bg_and_paints_a_caret() {
        let (surface, lh) = paint_settings_chrome_at(
            QRect::new(0.0, 0.0, W as f32, H as f32),
            "Settings",
            "",
            "type to filter",
            true,
        );
        let theme = Theme::default();

        let (r, g, b, _) = surface.pixel(W - 4, (lh * 1.5) as u32);
        assert_eq!(
            (r, g, b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "an active search row uses `selected_bg`",
        );

        // With an empty query the caret sits right after the " /  "
        // prefix. Scan that row for the accent stroke.
        let (prefix_w, _) = measure_text(&font(), SETTINGS_SEARCH_PREFIX);
        let caret_x = (2.0 + prefix_w) as u32;
        let row = (lh * 1.5) as u32;
        let accent = (theme.accent_fg.r, theme.accent_fg.g, theme.accent_fg.b);
        let found = (caret_x.saturating_sub(1)..=caret_x + 2).any(|x| {
            let (r, g, b, _) = surface.pixel(x.min(W - 1), row);
            (r, g, b) == accent
        });
        assert!(
            found,
            "active chrome should paint an accent caret at x≈{caret_x}"
        );
    }

    /// Non-zero-origin regression guard (LESSONS.md:159-181).
    #[test]
    fn settings_chrome_paints_at_a_nonzero_origin_only() {
        let ox = 24.0_f32;
        let oy = 30.0_f32;
        let (surface, lh) = paint_settings_chrome_at(
            QRect::new(ox, oy, W as f32 - ox, H as f32 - oy),
            "Settings",
            "",
            "",
            false,
        );
        let theme = Theme::default();

        let (r, g, b, _) = surface.pixel(W - 4, oy as u32 + (lh * 0.5) as u32);
        assert_eq!(
            (r, g, b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
            "header row should follow the requested origin",
        );
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(4, oy as u32 + (lh * 0.5) as u32);
                (r, g, b)
            },
            (255, 255, 255),
            "nothing should paint left of the origin",
        );
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(W - 4, oy as u32 - 4);
                (r, g, b)
            },
            (255, 255, 255),
            "nothing should paint above the origin",
        );
    }

    #[test]
    fn settings_chrome_zero_width_is_a_no_op() {
        let (surface, _lh) = paint_settings_chrome_at(
            QRect::new(0.0, 0.0, 0.0, H as f32),
            "Settings",
            "",
            "",
            true,
        );
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(1, 1);
                (r, g, b)
            },
            (255, 255, 255),
        );
    }
}
