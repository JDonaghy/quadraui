//! Direct2D / DirectWrite layout + [`RawFormSurface`] for
//! [`crate::Form`] (issue #26; field-kind painting moved to the shared
//! [`crate::primitives::form::paint`] in #808).
//!
//! [`win_form_layout`] computes one [`crate::FormLayout`] — the same
//! "one layout, paint and hit-test both consume it" contract
//! `win::tab_bar` / `win::status_bar` established — consumed both by
//! `WinBackend::form_layout` (hit-testing) and by `WinBackend::draw_form`
//! (which feeds it to `paint`).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod form;` and `backend.rs`'s module
//! docs. See `win::status_bar`'s module doc for why colours come from
//! `Theme::default()` rather than a live `WinBackend` theme field.
//!
//! `FieldKind::Toolbar` is still painted here rather than by `paint`
//! (see that fn's doc for why) — as plain per-button text via
//! `toolbar_item`/`WinBackend::draw_form`, not `win::toolbar`'s full
//! chrome; unifying that with the GTK/macOS twins (which do delegate to
//! their full toolbar rasteriser) is follow-up scope, not part of #808.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::native_surface::NativeSurface;
use crate::primitives::form::{Form, FormLayout};
use crate::primitives::layout_metrics::TextMeasure;
use crate::primitives::toolbar::ToolbarButton;
use crate::theme::Theme;
use crate::types::WidgetId;

/// Minimal [`NativeSurface`] adapter over a raw `(&ID2D1RenderTarget,
/// &DWrite)` pair, for [`crate::primitives::form::paint`] call sites
/// that have only those — not a live [`super::WinBackend`] — such as
/// [`crate::win::multi_section_view`]'s embedded-`Form` section body.
///
/// Frame-lifecycle / metrics verbs are unreachable from a raw target
/// (there is no backend to ask) so they panic if ever called — `paint`
/// never calls them (it only fills, draws text, and measures text), so
/// this is a latent contract, not a live gap.
pub(crate) struct RawFormSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
    pub(crate) dwrite: &'a DWrite,
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
        self.dwrite.measure_text(text).unwrap_or((0.0, 0.0))
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: crate::Color) {
        let _ = fill_rect(self.target, rect, color);
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: crate::Color, stroke_width: f32) {
        let _ = super::text::stroke_rect(self.target, rect, color, stroke_width);
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: crate::Color) {
        let _ = self.dwrite.draw_text(self.target, text, rect, color);
    }

    fn surface_draw_line(
        &mut self,
        from: crate::Point,
        to: crate::Point,
        color: crate::Color,
        stroke_width: f32,
    ) {
        let _ =
            super::text::draw_line(self.target, from.x, from.y, to.x, to.y, color, stroke_width);
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        super::text::push_clip(self.target, rect);
    }

    fn surface_pop_clip(&mut self) {
        super::text::pop_clip(self.target);
    }

    fn surface_draw_image(
        &mut self,
        _rect: Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        crate::backend::ImagePaintResult::Unsupported
    }
}

/// Deprecated free-function shim (#808, CLAUDE.md rule 8): this module's
/// `draw_form` used to match every `FieldKind` directly. Painting now
/// goes through [`crate::primitives::form::paint`] via
/// [`RawFormSurface`]; this wrapper reproduces the old signature exactly
/// for any external caller that held a direct `quadraui::win::draw_form`
/// reference rather than going through [`crate::Backend::draw_form`] —
/// the sanctioned entry point, and the one every in-tree call site
/// already uses, which is why this shim has no in-repo caller left to
/// trip the `-D warnings`-denied `deprecated` lint. `FieldKind::Toolbar`
/// still renders as plain per-button text here, matching pre-#808
/// behaviour, for the same reason `WinBackend::draw_form` does (see that
/// method's doc).
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_form` (or `crate::primitives::form::paint` with a `RawFormSurface`) instead — this free function is a compatibility shim over the shared #808 implementation"
)]
pub fn draw_form(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    form: &Form,
    line_height: f32,
) -> FormLayout {
    let flayout = win_form_layout(dwrite, rect, form, line_height);
    let theme = Theme::default();
    let origin = crate::Point::new(rect.x, rect.y);
    let mut surface = RawFormSurface { target, dwrite };
    crate::primitives::form::paint(form, &flayout, &mut surface, &theme, origin);

    for vf in &flayout.visible_fields {
        let Some(field) = form.fields.get(vf.field_idx) else {
            continue;
        };
        let crate::FieldKind::Toolbar(toolbar) = &field.kind else {
            continue;
        };
        let field_fg = if field.disabled {
            theme.muted_fg
        } else {
            theme.foreground
        };
        for (item_id, item_rect) in &vf.item_bounds {
            let btn = toolbar.buttons.iter().find_map(|b| {
                toolbar_item(&field.id, b)
                    .filter(|(id, _)| id == item_id)
                    .map(|_| b)
            });
            let r = Rect::new(
                origin.x + item_rect.x,
                origin.y + item_rect.y,
                item_rect.width,
                item_rect.height,
            );
            match btn {
                Some(ToolbarButton::Action { label, enabled, .. }) => {
                    let fg = if *enabled { field_fg } else { theme.muted_fg };
                    let (tw, th) = dwrite.measure_text(label).unwrap_or((0.0, 0.0));
                    let ty = r.y + (r.height - th) / 2.0;
                    let _ = dwrite.draw_text(target, label, Rect::new(r.x, ty, tw, th), fg);
                }
                Some(ToolbarButton::Label { text, fg }) => {
                    let color = fg.unwrap_or(field_fg);
                    let (tw, th) = dwrite.measure_text(text).unwrap_or((0.0, 0.0));
                    let ty = r.y + (r.height - th) / 2.0;
                    let _ = dwrite.draw_text(target, text, Rect::new(r.x, ty, tw, th), color);
                }
                _ => {}
            }
        }
    }

    flayout
}

pub(crate) fn toolbar_item(field_id: &WidgetId, btn: &ToolbarButton) -> Option<(WidgetId, String)> {
    match btn {
        ToolbarButton::Action { id, label, .. } => Some((id.clone(), label.clone())),
        ToolbarButton::Label { text, .. } => Some((field_id.clone(), text.clone())),
        ToolbarButton::Separator => None,
    }
}

/// Adapts a live [`DWrite`] handle to the shared [`TextMeasure`] trait
/// so [`crate::primitives::layout_metrics::form_field_measure`] never
/// has to name a DirectWrite type — mirrors `macos::form::CtFontMeasure`,
/// which exists for exactly this reason.
struct DWriteMeasure<'a>(&'a DWrite);

impl TextMeasure for DWriteMeasure<'_> {
    fn width_of(&self, text: &str) -> f32 {
        self.0.measure_text(text).map(|(w, _)| w).unwrap_or(0.0)
    }
}

/// Compute a [`Form`]'s layout without painting — the DirectWrite twin
/// of [`draw_form`]'s internal layout call.
///
/// Thin wrapper over [`crate::primitives::layout_metrics::form_row_height`]
/// / [`crate::primitives::layout_metrics::form_field_measure`] (#499,
/// adopted for `win/` by #701), via [`DWriteMeasure`] — the same
/// per-field-kind measurement math `macos::form::mac_form_layout` uses.
pub fn win_form_layout(dwrite: &DWrite, rect: Rect, form: &Form, line_height: f32) -> FormLayout {
    let row_h = crate::primitives::layout_metrics::form_row_height(line_height as f64);
    let measure = DWriteMeasure(dwrite);
    form.layout(rect.width, rect.height, |i| {
        crate::primitives::layout_metrics::form_field_measure(&form.fields[i], row_h, &measure)
    })
}

/// Cursor width in DIPs for the settings-chrome search row — matches the
/// GTK/macOS twins' 1.5px caret.
const SETTINGS_CURSOR_W: f32 = 1.5;
/// Prefix rendered before the settings search query — same string the
/// GTK/macOS twins use.
const SETTINGS_SEARCH_PREFIX: &str = " /  ";

/// Draw settings-panel chrome: a 2-row strip with a header row and a
/// search input row, designed to sit immediately above a [`Form`] body.
///
/// Port of [`crate::gtk::form::draw_settings_chrome`] (and
/// [`crate::macos::form::draw_settings_chrome`]) — same two-row layout,
/// same `" /  "` prefix, same placeholder rule (shown only when the
/// query is empty *and* the row is inactive), same accent caret when
/// active.
///
/// Chrome only: the form body and any scrollbar layered below are
/// painted separately by the caller. `rect.height` is not consulted —
/// total chrome height is always `2 * line_height`, matching both twins.
#[allow(clippy::too_many_arguments)]
pub fn draw_settings_chrome(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    line_height: f32,
    header_text: &str,
    query: &str,
    placeholder: &str,
    active: bool,
) {
    if rect.width <= 0.0 || line_height <= 0.0 {
        return;
    }

    let theme = Theme::default();

    // Row 0: header bar.
    let _ = fill_rect(
        target,
        Rect::new(rect.x, rect.y, rect.width, line_height),
        theme.header_bg,
    );
    let (header_w, header_h) = dwrite.measure_text(header_text).unwrap_or((0.0, 0.0));
    let header_y = rect.y + (line_height - header_h) / 2.0;
    let _ = dwrite.draw_text(
        target,
        header_text,
        Rect::new(rect.x + 2.0, header_y, header_w, header_h),
        theme.header_fg,
    );

    // Row 1: search input.
    let search_y = rect.y + line_height;
    let row_bg = if active {
        theme.selected_bg
    } else {
        theme.tab_bar_bg
    };
    let _ = fill_rect(
        target,
        Rect::new(rect.x, search_y, rect.width, line_height),
        row_bg,
    );

    let (prefix_w, prefix_h) = dwrite
        .measure_text(SETTINGS_SEARCH_PREFIX)
        .unwrap_or((0.0, 0.0));
    let prefix_y = search_y + (line_height - prefix_h) / 2.0;
    let _ = dwrite.draw_text(
        target,
        SETTINGS_SEARCH_PREFIX,
        Rect::new(rect.x + 2.0, prefix_y, prefix_w, prefix_h),
        theme.muted_fg,
    );

    let q_x = rect.x + 2.0 + prefix_w;
    let show_placeholder = query.is_empty() && !placeholder.is_empty() && !active;
    let (text, color) = if show_placeholder {
        (placeholder, theme.muted_fg)
    } else if query.is_empty() {
        (query, theme.muted_fg)
    } else {
        (query, theme.foreground)
    };
    let (text_w, text_h) = dwrite.measure_text(text).unwrap_or((0.0, 0.0));
    let text_y = search_y + (line_height - text_h) / 2.0;
    let _ = dwrite.draw_text(target, text, Rect::new(q_x, text_y, text_w, text_h), color);

    if active {
        let (query_w, _) = dwrite.measure_text(query).unwrap_or((0.0, 0.0));
        let cur_x = q_x + if query.is_empty() { 0.0 } else { query_w };
        let _ = fill_rect(
            target,
            Rect::new(cur_x, search_y + 2.0, SETTINGS_CURSOR_W, line_height - 4.0),
            theme.accent_fg,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::form::{FieldKind, FormHit, ToggleGroupItem};
    use crate::types::StyledText;
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 300.0;
    const H: f32 = 200.0;
    const LINE_HEIGHT: f32 = 14.0;

    /// Paint `form` via the shared [`crate::primitives::form::paint`]
    /// through a [`RawFormSurface`] over `surface`'s headless target —
    /// the same adapter `win::multi_section_view`'s embedded-`Form` body
    /// uses, exercised here instead of a live `WinBackend` since these
    /// tests predate #808 and only ever needed `target`/`dwrite`.
    fn paint(surface: &HeadlessSurface, dwrite: &DWrite, rect: Rect, form: &Form) -> FormLayout {
        let layout = win_form_layout(dwrite, rect, form, LINE_HEIGHT);
        let theme = Theme::default();
        let origin = crate::Point::new(rect.x, rect.y);
        surface
            .paint(|target| {
                let mut raw = RawFormSurface { target, dwrite };
                crate::primitives::form::paint(form, &layout, &mut raw, &theme, origin);
            })
            .expect("paint form");
        layout
    }

    fn field(id: &str, label: &str, kind: FieldKind) -> crate::primitives::form::FormField {
        crate::primitives::form::FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label.to_string()),
            kind,
            hint: StyledText::plain(""),
            disabled: false,
            validation: None,
        }
    }

    fn make_form(fields: Vec<crate::primitives::form::FormField>) -> Form {
        Form {
            id: WidgetId::new("form"),
            fields,
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
        }
    }

    /// Paint↔click round trip across a mix of field kinds, including a
    /// `ToggleGroup` whose per-item hit regions must land on the
    /// correct toggle id.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let form = make_form(vec![
            field(
                "name",
                "Name",
                FieldKind::TextInput {
                    value: "quadraui".into(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    selection_anchor: None,
                },
            ),
            field(
                "flags",
                "Flags",
                FieldKind::ToggleGroup {
                    toggles: vec![
                        ToggleGroupItem {
                            id: WidgetId::new("flags:a"),
                            label: "A".into(),
                            value: true,
                        },
                        ToggleGroupItem {
                            id: WidgetId::new("flags:b"),
                            label: "B".into(),
                            value: false,
                        },
                    ],
                },
            ),
        ]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &form);

        assert_eq!(layout.visible_fields.len(), 2);
        for vf in &layout.visible_fields {
            let cx = vf.bounds.x + 1.0;
            let cy = vf.bounds.y + vf.bounds.height / 2.0;
            assert_eq!(layout.hit_test(cx, cy), FormHit::Field(vf.id.clone()));
        }

        let toggle_field = &layout.visible_fields[1];
        assert_eq!(toggle_field.item_bounds.len(), 2);
        for (id, item_rect) in &toggle_field.item_bounds {
            let cx = item_rect.x + item_rect.width / 2.0;
            let cy = item_rect.y + item_rect.height / 2.0;
            assert_eq!(layout.hit_test(cx, cy), FormHit::Field(id.clone()));
        }
    }

    /// The focused row paints `selected_bg` at its own bounds.
    #[test]
    fn focused_row_paints_selected_bg() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let mut form = make_form(vec![
            field("a", "A", FieldKind::Toggle { value: false }),
            field("b", "B", FieldKind::Toggle { value: true }),
        ]);
        form.focused_field = Some(WidgetId::new("b"));
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &form);

        let theme = Theme::default();
        let bounds = layout.visible_fields[1].bounds;
        let px = surface.pixel_at((bounds.x + 1.0) as u32, (bounds.y + 1.0) as u32);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            )
        );
    }

    /// #808: `paint` reads geometry only from the `FormLayout` it's
    /// given — it can no longer disagree with a "no-paint" call to
    /// `win_form_layout` for the same inputs, because both now *are*
    /// the same call. This is the structural version of what used to be
    /// a byte-for-byte equality assertion between two independently
    /// computed layouts; kept as a smoke test that painting a
    /// `TextInput` + `PasswordInput` pair (with cursors set) doesn't
    /// panic.
    #[test]
    fn paint_with_cursors_does_not_panic() {
        let form = make_form(vec![
            field(
                "name",
                "Name",
                FieldKind::TextInput {
                    value: "hi".into(),
                    placeholder: String::new(),
                    cursor: None,
                    selection_anchor: None,
                },
            ),
            field(
                "pw",
                "Password",
                FieldKind::PasswordInput {
                    value: "secret".into(),
                    placeholder: String::new(),
                    cursor: Some(3),
                    mask_char: '*',
                },
            ),
        ]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let _ = paint(&surface, &dwrite, rect, &form);
    }

    /// [`draw_settings_chrome`]'s two rows must paint at the geometry the
    /// GTK twin (`gtk::form::draw_settings_chrome`) uses for the same
    /// inputs: row 0 (`y in [0, line_height)`) is `header_bg`, row 1
    /// (`y in [line_height, 2*line_height)`) is `tab_bar_bg` when
    /// `active` is `false`. Probed away from the left-aligned text (issue
    /// #734).
    #[test]
    fn settings_chrome_paints_header_and_inactive_search_rows() {
        let surface = HeadlessSurface::new(200, 60).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, 200.0, 60.0);
        let line_height = 18.0_f32;

        surface
            .paint(|target| {
                draw_settings_chrome(
                    target,
                    &dwrite,
                    rect,
                    line_height,
                    "Settings",
                    "",
                    "search…",
                    false,
                );
            })
            .expect("paint settings chrome");

        let theme = Theme::default();
        let header_px = surface.pixel_at(150, 2);
        assert_eq!(
            (header_px.r, header_px.g, header_px.b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
            "header row paints header_bg"
        );

        let search_px = surface.pixel_at(150, line_height as u32 + 2);
        assert_eq!(
            (search_px.r, search_px.g, search_px.b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "inactive search row paints tab_bar_bg"
        );
    }

    /// The search row switches to `selected_bg` while `active` — same
    /// active/inactive contract as `gtk::form::draw_settings_chrome` and
    /// `macos::form::draw_settings_chrome`.
    #[test]
    fn settings_chrome_active_search_row_paints_selected_bg() {
        let surface = HeadlessSurface::new(200, 60).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, 200.0, 60.0);
        let line_height = 18.0_f32;

        surface
            .paint(|target| {
                draw_settings_chrome(
                    target,
                    &dwrite,
                    rect,
                    line_height,
                    "Settings",
                    "op",
                    "",
                    true,
                );
            })
            .expect("paint settings chrome");

        let theme = Theme::default();
        let search_px = surface.pixel_at(150, line_height as u32 + 2);
        assert_eq!(
            (search_px.r, search_px.g, search_px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "active search row paints selected_bg"
        );
    }
}
