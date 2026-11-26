//! The one shared [`PaintSurface`] adapter over a bare
//! `&ID2D1RenderTarget`.
//!
//! This adapter varies on one axis: **`dwrite` presence.**
//! [`crate::Scrollbar`], [`crate::DropOverlay`],
//! [`crate::Split`] and [`crate::SplitTree`] paint with `target` alone
//! (no text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//! [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//! [`crate::ToastOverlay`] paint text too and need a
//! live [`DWrite`]. [`D2dSurface::dwrite`] is `Option` so one struct
//! covers both — [`Self::dwrite_or_panic`] panics only if a text verb
//! is reached with `dwrite: None`, exactly mirroring what the primitives'
//! `paint` fns guarantee: they never call a text verb without text measurement.
//!
//! All other verbs (`surface_stroke_rect`, `surface_draw_text_run`,
//! `surface_draw_line`, `surface_push_clip`/`surface_pop_clip`,
//! `surface_draw_image`) are implemented unconditionally; primitives that
//! never call a given verb simply never reach that code.
//!
//! `surface_measure_text_styled`/`surface_draw_text_run_styled` handle bold
//! and `scale_x` scaling (needed by `win::multi_section_view`'s embedded
//! Terminal section body); at `scale_x == 1.0` the scaling branch is skipped.
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly; no primitive that uses this adapter ever paints an image.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::backend::ImagePaintResult;
use crate::paint_surface::PaintSurface;
use crate::{Color, Image, Point, Rect, Viewport};

use super::text::{
    draw_line, fill_rect, fill_rounded_rect, pop_clip, push_clip, stroke_rect, stroke_rounded_rect,
    with_horizontal_scale, DWrite,
};

/// See the module doc for [`Self::dwrite`], the one field that carries
/// genuine per-call-site behaviour; every other [`PaintSurface`] verb
/// below is one shared implementation.
pub(crate) struct D2dSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
    /// `None` for primitives that never paint text through this adapter
    /// (`Scrollbar`, `DropOverlay`, `Split`, `SplitTree`) — see module
    /// doc.
    pub(crate) dwrite: Option<&'a DWrite>,
}

impl<'a> D2dSurface<'a> {
    /// Returns the live `DWrite` handle or panics — only reachable if a
    /// text verb is called on an adapter constructed with
    /// `dwrite: None`, which no current primitive does (see module doc).
    fn dwrite_or_panic(&self) -> &'a DWrite {
        self.dwrite
            .expect("D2dSurface: text verb called without a DWrite instance")
    }
}

impl PaintSurface for D2dSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: Viewport) {
        unreachable!("D2dSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("D2dSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> Viewport {
        unreachable!("D2dSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("D2dSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("D2dSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.dwrite_or_panic()
            .measure_text(text)
            .unwrap_or((0.0, 0.0))
    }

    fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
        self.dwrite_or_panic()
            .measure_text_styled(text, bold)
            .unwrap_or((0.0, 0.0))
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        let _ = fill_rect(self.target, rect, color);
    }

    /// `super::text::fill_rounded_rect`'s `ID2D1RenderTarget`
    /// twin of [`Self::surface_fill_rect`] above.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        let _ = fill_rounded_rect(self.target, rect, radius, color);
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        let _ = stroke_rect(self.target, rect, color, stroke_width);
    }

    /// `super::text::stroke_rounded_rect`'s `ID2D1RenderTarget`
    /// twin of [`Self::surface_stroke_rect`] above.
    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    ) {
        let _ = stroke_rounded_rect(self.target, rect, radius, color, stroke_width);
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let _ = self
            .dwrite_or_panic()
            .draw_text(self.target, text, rect, color);
    }

    /// See module doc: this is `win::form`'s pre-existing bold +
    /// `scale_x` implementation, generalised to every call site. At
    /// `scale_x == 1.0` it reproduces the plain bold-only override exactly (the `with_horizontal_scale` branch is
    /// skipped, falling through to the same `draw_text_styled` call).
    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let _ = (italic, underline);
        let dwrite = self.dwrite_or_panic();
        if (scale_x - 1.0).abs() > f32::EPSILON {
            with_horizontal_scale(self.target, scale_x, rect.x, || {
                let _ = dwrite.draw_text_styled(self.target, text, rect, color, bold);
            });
        } else {
            let _ = dwrite.draw_text_styled(self.target, text, rect, color, bold);
        }
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
        let _ = draw_line(self.target, from.x, from.y, to.x, to.y, color, stroke_width);
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        push_clip(self.target, rect);
    }

    fn surface_pop_clip(&mut self) {
        pop_clip(self.target);
    }

    fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
        ImagePaintResult::Unsupported
    }
}

/// The terminal's shared pixel painter through [`D2dSurface`] against a
/// headless Direct2D target — cell backgrounds, wide glyphs, overlays,
/// clipping and the split divider.
#[cfg(test)]
mod tests {
    use crate::event::Rect;
    use crate::primitives::terminal::{Terminal, TerminalCell, TerminalCursorShape};
    use crate::terminal_style::wide_glyph_x_scale;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::{fill_rect, DWrite};

    const W: u32 = 200;
    const H: u32 = 120;
    const CHAR_W: f32 = 10.0;
    const LINE_H: f32 = 20.0;

    fn cell(ch: char, fg: Color, bg: Color) -> TerminalCell {
        TerminalCell {
            text: ch.to_string(),
            fg,
            bg,
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

    fn dwrite() -> DWrite {
        DWrite::new("Segoe UI", 10.0, None)
            .expect("create DWrite")
            .0
    }

    /// Paints `term` via [`super::D2dSurface`] — the same adapter
    /// `win::multi_section_view`'s embedded `Terminal` section body
    /// uses — so these tests keep controlling `char_width`/`line_height`
    /// explicitly instead of depending on a live `WinBackend`'s measured
    /// font metrics.
    fn paint(surface: &HeadlessSurface, dwrite: &DWrite, term: &Terminal, theme: &Theme) {
        surface
            .paint(|target| {
                let mut raw = super::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::terminal::paint(
                    term, &mut raw, theme, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, None,
                );
            })
            .expect("paint");
    }

    /// A cell's background must paint at its own grid position before
    /// the glyph is drawn — probing a corner the glyph doesn't reach
    /// isolates the fill from font rasterisation.
    #[test]
    fn cell_bg_paints_at_its_grid_position() {
        let magenta = Color::rgb(200, 30, 200);
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('A', Color::rgb(255, 255, 255), magenta)]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (magenta.r, magenta.g, magenta.b));
    }

    /// A double-width glyph (CJK) followed by vt100's blank continuation cell
    /// must have its background span both columns — the continuation cell's own
    /// background must NOT paint over the second half of the wide glyph's cell.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        // The wide glyph's foreground is set equal to its background
        // (magenta on magenta) so antialiased glyph ink can't shift the
        // probed colour — same trick as the GTK/macOS twin tests.
        let row = vec![cell('日', magenta, magenta), cell(' ', magenta, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        // Probe just past the first single-cell-width boundary, still
        // within the wide glyph's two-column span: must be magenta, not
        // cyan.
        let probe_x = (CHAR_W * 1.5) as u32;
        let px = surface.pixel_at(probe_x, 5);
        assert_eq!(
            (px.r, px.g, px.b),
            (magenta.r, magenta.g, magenta.b),
            "wide cell's background should span both columns, not be \
             overpainted by the continuation cell's background"
        );
    }

    /// Mirrors `terminal_style`'s `narrow_wide_glyph_is_stretched_to_fill_two_cells`
    /// — every backend now shares `wide_glyph_x_scale`, so a CJK glyph
    /// measuring 15px in an 18px (2 × 9px) box scales 1.2x here too.
    #[test]
    fn narrow_wide_glyph_is_stretched_to_fill_two_cells() {
        let cell_w = 18.0;
        let scale = wide_glyph_x_scale(15.0, cell_w);
        assert!(
            (scale - 1.2).abs() < 1e-9,
            "15px glyph in an 18px box should scale 1.2x, got {scale}"
        );
        assert!((15.0 * scale - cell_w).abs() < 1e-9);
    }

    /// Mirrors `terminal_style`'s `exact_fit_wide_glyph_is_not_scaled`.
    #[test]
    fn exact_fit_wide_glyph_is_not_scaled() {
        assert_eq!(wide_glyph_x_scale(18.0, 18.0), 1.0);
    }

    /// Mirrors `terminal_style`'s `over_wide_glyph_is_shrunk_into_box`.
    #[test]
    fn over_wide_glyph_is_shrunk_into_box() {
        let scale = wide_glyph_x_scale(24.0, 18.0);
        assert!(
            scale < 1.0,
            "24px glyph in 18px box should shrink, got {scale}"
        );
        assert!((24.0 * scale - 18.0).abs() < 1e-9);
    }

    /// Companion regression, mirroring the `gtk`/`macos` surface tests'
    /// `narrow_cells_advance_by_single_char_width`: ordinary narrow
    /// (single-width) cells must still advance by exactly `char_width` —
    /// the wide-cell fix must not widen unrelated cells.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        let white = Color::rgb(255, 255, 255);
        let row = vec![cell('A', white, magenta), cell('B', cyan, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let probe_x = (CHAR_W * 1.5) as u32;
        let px = surface.pixel_at(probe_x, 5);
        assert_eq!(
            (px.r, px.g, px.b),
            (cyan.r, cyan.g, cyan.b),
            "narrow cells must still advance by exactly char_width"
        );
    }

    /// The cursor overlay swaps fg/bg: the cell background paints in
    /// the cell's *foreground* colour.
    #[test]
    fn cursor_cell_inverts_fg_bg() {
        let fg = Color::rgb(10, 220, 30);
        let bg = Color::rgb(40, 40, 40);
        let mut c = cell('X', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (fg.r, fg.g, fg.b));
    }

    /// A find-active match paints its cell background orange —
    /// matching the GTK/macOS overlay convention.
    #[test]
    fn find_active_cell_paints_orange() {
        let mut c = cell('z', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.is_find_active = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (255, 165, 0));
    }

    /// A selected cell uses `theme.selection_bg`.
    #[test]
    fn selected_cell_uses_theme_selection_bg() {
        let mut c = cell('x', Color::rgb(255, 255, 255), Color::rgb(10, 10, 10));
        c.selected = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            )
        );
    }

    /// Rows past the pane bottom must be clipped, not bled — the
    /// interactive-resize regression `gtk`/`macos` already guard
    #[test]
    fn rows_past_the_pane_bottom_are_clipped() {
        let magenta = Color::rgb(200, 30, 200);
        let bg_sentinel = Color::rgb(1, 2, 3);
        let cells: Vec<Vec<_>> = (0..60)
            .map(|_| {
                (0..20)
                    .map(|_| cell(' ', Color::rgb(255, 255, 255), magenta))
                    .collect()
            })
            .collect();
        let term = Terminal {
            id: WidgetId::new("term"),
            cells,
            scrollbar: None,
        };
        let pane_h = 60.0_f32;
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        surface
            .paint(|target| {
                let _ = fill_rect(target, Rect::new(0.0, 0.0, W as f32, H as f32), bg_sentinel);
                let mut raw = super::D2dSurface {
                    target,
                    dwrite: Some(&dwrite),
                };
                crate::primitives::terminal::paint(
                    &term, &mut raw, &theme, 0.0, 0.0, W as f32, pane_h, LINE_H, CHAR_W, None,
                );
            })
            .expect("paint");

        let inside = surface.pixel_at(4, 4);
        assert_eq!(
            (inside.r, inside.g, inside.b),
            (magenta.r, magenta.g, magenta.b)
        );

        let below = surface.pixel_at(4, H - 4);
        assert_eq!(
            (below.r, below.g, below.b),
            (bg_sentinel.r, bg_sentinel.g, bg_sentinel.b),
            "rows below the pane bottom must be clipped, not bled"
        );
    }

    /// A bold cell paints via `DWrite::draw_text_styled` rather than
    /// silently dropping the attribute — smoke-tested by confirming the
    /// paint call succeeds and the cell's own bg still lands correctly
    /// (glyph shape/weight isn't independently probable via pixel
    /// colour without OCR, so this only guards the plumbing).
    #[test]
    fn bold_cell_paints_without_error() {
        let mut c = cell('B', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.bold = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let dwrite = dwrite();
        let theme = Theme::default();
        paint(&surface, &dwrite, &term, &theme);

        let px = surface.pixel_at(1, 1);
        assert_eq!((px.r, px.g, px.b), (20, 20, 20));
    }

    /// [`crate::primitives::terminal::paint_divider`] paints a 1-DIP
    /// line at `x`.
    #[test]
    fn divider_paints_a_one_dip_line() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let theme = Theme::default();
        surface
            .paint(|target| {
                let _ = fill_rect(
                    target,
                    Rect::new(0.0, 0.0, W as f32, H as f32),
                    Color::rgb(0, 0, 0),
                );
                let dwrite = dwrite();
                let mut raw = super::D2dSurface {
                    target,
                    dwrite: Some(&dwrite),
                };
                crate::primitives::terminal::paint_divider(&mut raw, 50.0, 0.0, H as f32, &theme);
            })
            .expect("paint");

        let on_line = surface.pixel_at(50, 10);
        let off_line = surface.pixel_at(52, 10);
        assert_eq!(
            (on_line.r, on_line.g, on_line.b),
            (theme.separator.r, theme.separator.g, theme.separator.b)
        );
        assert_eq!((off_line.r, off_line.g, off_line.b), (0, 0, 0));
    }
}

/// Form and settings-chrome paint through [`D2dSurface`] on a headless
/// Direct2D target.
#[cfg(test)]
mod form_paint_tests {
    use super::D2dSurface;
    use crate::primitives::form::{FieldKind, Form, FormHit, FormLayout, ToggleGroupItem};
    use crate::theme::Theme;
    use crate::types::{Color, StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;
    use crate::Rect;

    const W: f32 = 300.0;
    const H: f32 = 200.0;
    const LINE_HEIGHT: f32 = 14.0;

    /// The pixel form layout at `line_height`, measured through `dwrite`
    /// — the geometry a pixel form paints from.
    fn win_form_layout(dwrite: &DWrite, rect: Rect, form: &Form, line_height: f32) -> FormLayout {
        let row_h = crate::primitives::layout_metrics::form_row_height(line_height as f64);
        form.layout(rect.width, rect.height, |i| {
            crate::primitives::layout_metrics::form_field_measure(&form.fields[i], row_h, dwrite)
        })
    }

    /// Paint the settings chrome through the shared
    /// [`crate::compose::sidebar_panel_body::paint_settings_chrome`] on a
    /// [`D2dSurface`] over `target`.
    #[allow(clippy::too_many_arguments)]
    fn draw_settings_chrome(
        target: &windows::Win32::Graphics::Direct2D::ID2D1RenderTarget,
        dwrite: &DWrite,
        rect: Rect,
        line_height: f32,
        header_text: &str,
        query: &str,
        placeholder: &str,
        active: bool,
    ) {
        let mut raw = D2dSurface {
            target,
            dwrite: Some(dwrite),
        };
        crate::compose::sidebar_panel_body::paint_settings_chrome(
            &mut raw,
            &Theme::default(),
            rect,
            line_height,
            header_text,
            query,
            placeholder,
            active,
        );
    }

    /// Paint `form` via the shared [`crate::primitives::form::paint`]
    /// through a [`D2dSurface`] over `surface`'s headless target —
    /// the same adapter `win::multi_section_view`'s embedded-`Form` body uses.
    fn paint(surface: &HeadlessSurface, dwrite: &DWrite, rect: Rect, form: &Form) -> FormLayout {
        let layout = win_form_layout(dwrite, rect, form, LINE_HEIGHT);
        let theme = Theme::default();
        let origin = crate::Point::new(rect.x, rect.y);
        surface
            .paint(|target| {
                let mut raw = D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
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
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    fn make_form(fields: Vec<crate::primitives::form::FormField>) -> Form {
        Form {
            id: WidgetId::new("form"),
            fields,
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    /// Paint↔click round trip across a mix of field kinds, including a
    /// `ToggleGroup` whose per-item hit regions must land on the
    /// correct toggle id.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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
            assert_eq!(layout.hit_test(cx, cy), FormHit::Field(vf.id));
        }

        let toggle_field = &layout.visible_fields[1];
        assert_eq!(toggle_field.item_bounds.len(), 2);
        for (id, item_rect) in &toggle_field.item_bounds {
            let cx = item_rect.x + item_rect.width / 2.0;
            let cy = item_rect.y + item_rect.height / 2.0;
            assert_eq!(layout.hit_test(cx, cy), FormHit::Field(*id));
        }
    }

    /// The focused row paints `selected_bg` at its own bounds.
    #[test]
    fn focused_row_paints_selected_bg() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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

    /// Both `paint` and `win_form_layout` consume the same layout computation,
    /// ensuring they cannot disagree on geometry. This is a smoke test that
    /// painting a `TextInput` + `PasswordInput` pair (with cursors set) doesn't panic.
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let _ = paint(&surface, &dwrite, rect, &form);
    }

    /// [`draw_settings_chrome`]'s two rows must paint at the shared
    /// painter's geometry: row 0 (`y in [0, line_height)`) is `header_bg`, row 1
    /// (`y in [line_height, 2*line_height)`) is `tab_bar_bg` when
    /// `active` is `false`.
    #[test]
    fn settings_chrome_paints_header_and_inactive_search_rows() {
        let surface = HeadlessSurface::new(200, 60).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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

    /// The search row switches to `selected_bg` while `active`.
    #[test]
    fn settings_chrome_active_search_row_paints_selected_bg() {
        let surface = HeadlessSurface::new(200, 60).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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

    /// A `rect.height` of exactly one `line_height` (the shape
    /// [`crate::compose::sidebar_panel_body::SidebarPanelChrome::Header`]
    /// reserves) must not paint the search row — only the header row paints.
    #[test]
    fn settings_chrome_one_row_height_paints_no_search_row() {
        let surface = HeadlessSurface::new(200, 60).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let line_height = 18.0_f32;
        // Sentinel distinct from every colour this fn itself paints
        // (header_bg/tab_bar_bg/selected_bg), so a search-row pixel
        // reading the sentinel unambiguously means "nothing painted
        // here".
        let sentinel = Color::rgb(1, 2, 3);
        surface
            .fill_rect(Rect::new(0.0, 0.0, 200.0, 60.0), sentinel)
            .expect("fill sentinel");

        let one_row = Rect::new(0.0, 0.0, 200.0, line_height);
        surface
            .paint(|target| {
                draw_settings_chrome(
                    target,
                    &dwrite,
                    one_row,
                    line_height,
                    "HEADER",
                    "",
                    "placeholder",
                    false,
                );
            })
            .expect("paint settings chrome");

        let theme = Theme::default();
        let header_px = surface.pixel_at(150, 2);
        assert_eq!(
            (header_px.r, header_px.g, header_px.b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
            "the header row must still paint even when the search row doesn't"
        );

        let search_px = surface.pixel_at(150, line_height as u32 + 2);
        assert_eq!(
            (search_px.r, search_px.g, search_px.b),
            (sentinel.r, sentinel.g, sentinel.b),
            "a 1-row-tall chrome rect must not paint a search row past its own height"
        );
    }
}

/// Palette paint through [`D2dSurface`] on a headless Direct2D target.
#[cfg(test)]
mod palette_paint_tests {
    use super::D2dSurface;
    use crate::primitives::palette::{
        native_surface_paint, pixel_layout, Palette, PaletteLayout, PaletteMode,
    };
    use crate::theme::Theme;
    use crate::types::{Color, StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;
    use crate::Rect;
    use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

    /// The pixel palette layout at `line_height` — the geometry the
    /// palette paints from.
    fn win_palette_layout(rect: Rect, palette: &Palette, line_height: f32) -> PaletteLayout {
        pixel_layout(rect.width, rect.height, palette, line_height).0
    }

    /// Paint `palette` into `rect` through the shared pixel painter on a
    /// [`D2dSurface`] over `target`, with `Theme::default()`.
    fn draw_palette(
        target: &ID2D1RenderTarget,
        dwrite: &DWrite,
        rect: Rect,
        palette: &Palette,
        line_height: f32,
        nerd_fonts_enabled: bool,
    ) {
        let (palette_layout, rows_h) = pixel_layout(rect.width, rect.height, palette, line_height);
        let mut surface = D2dSurface {
            target,
            dwrite: Some(dwrite),
        };
        native_surface_paint::paint(
            palette,
            rect,
            &palette_layout,
            rows_h,
            line_height,
            nerd_fonts_enabled,
            &mut surface,
            &Theme::default(),
            &crate::style::Style::default(),
            crate::primitives::context_menu::PointerTint::default(),
        );
    }

    fn item(text: &str, match_positions: Vec<usize>) -> crate::primitives::palette::PaletteItem {
        crate::primitives::palette::PaletteItem {
            text: StyledText::plain(text),
            detail: None,
            icon: None,
            match_positions,
            depth: 0,
            expandable: false,
            expanded: false,
        }
    }

    fn palette() -> Palette {
        Palette {
            id: WidgetId::new("pal"),
            title: "Commands".into(),
            query: "op".into(),
            query_cursor: 2,
            items: vec![item("open file", vec![0, 1]), item("close", vec![])],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 0,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::List,
        }
    }

    #[test]
    fn renders_and_highlights_match_positions() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let first_item = layout.visible_items[0].bounds;

        let theme = Theme::default();
        // Scan the whole first-item row box for match-highlighted ink —
        // matched runs ('o', 'p' at byte offsets 0, 1 of "open file")
        // paint in `match_fg`, warm and far from every other colour on
        // that row. Two deliberate weakenings versus an exact-equality
        // probe at a guessed coordinate: the scan covers the row's full
        // height (real DirectWrite glyph placement inside the row box
        // isn't predictable ahead of a live Windows font pass), and a
        // pixel counts when it is *nearer* `match_fg` than the row's
        // other two colours rather than exactly equal to it, since
        // glyph edges are antialiased blends over the selection fill.
        let dist2 = |px: Color, c: Color| {
            let dr = px.r as i32 - c.r as i32;
            let dg = px.g as i32 - c.g as i32;
            let db = px.b as i32 - c.b as i32;
            dr * dr + dg * dg + db * db
        };
        let found = (first_item.y as u32..(first_item.y + first_item.height) as u32).any(|y| {
            (first_item.x as u32..(first_item.x + first_item.width) as u32).any(|x| {
                let px = surface.pixel_at(x, y);
                let d_match = dist2(px, theme.match_fg);
                d_match < dist2(px, theme.surface_fg) && d_match < dist2(px, theme.selected_bg)
            })
        });
        assert!(
            found,
            "expected to find match_fg-coloured pixels on the matched item's row"
        );
    }

    /// The shared `paint` gives every GUI backend, including Windows, a
    /// thin bar caret (not a full-glyph-width block) in `theme.cursor`.
    #[test]
    fn query_row_paints_a_cursor_bar() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let qb = layout.query_bounds.expect("query bounds present");
        let theme = Theme::default();
        // A cursor bar is a *solid fill* spanning the row's full height —
        // unlike glyph ink (which never covers a whole text-row column
        // solidly, since real fonts leave leading above ascenders and
        // below descenders) — but only ~2px wide, not a full-glyph-width
        // block. Scan for the column with the highest fraction of
        // cursor-near pixels and require it to be nearly fully covered,
        // and require most columns in the query row to NOT be covered
        // (ruling out a block spanning the whole glyph).
        //
        // `draw_palette` always paints with `Theme::default()` (it takes
        // no theme parameter — see that fn's doc), whose dark-mode
        // `cursor` and `query_fg` happen to be the same `rgb(220, 220,
        // 220)`, so probing `theme.cursor` below doesn't by itself
        // distinguish a caret from glyph ink; `fully_covered_cols <= 4`
        // (a bar is ~2px wide, a full-glyph block is not) is the
        // assertion that actually tells them apart.
        let dist2 = |px: Color, c: Color| {
            let dr = px.r as i32 - c.r as i32;
            let dg = px.g as i32 - c.g as i32;
            let db = px.b as i32 - c.b as i32;
            dr * dr + dg * dg + db * db
        };
        let rows: Vec<u32> = (qb.y as u32..(qb.y + qb.height) as u32).collect();
        let coverage: Vec<f32> = (qb.x as u32..(qb.x + qb.width) as u32)
            .map(|x| {
                let covered = rows
                    .iter()
                    .filter(|&&y| dist2(surface.pixel_at(x, y), theme.cursor) < 400)
                    .count();
                covered as f32 / rows.len().max(1) as f32
            })
            .collect();
        let best_coverage = coverage.iter().cloned().fold(0.0_f32, f32::max);
        let fully_covered_cols = coverage.iter().filter(|&&c| c > 0.9).count();
        assert!(
            best_coverage > 0.9,
            "expected a full-row-height cursor bar; best column coverage was {best_coverage}"
        );
        assert!(
            fully_covered_cols <= 4,
            "expected a thin ~2px bar caret, not a full-glyph-width block; \
             {fully_covered_cols} columns were fully covered"
        );
    }

    /// The query row height uses a formula that reserves space for a
    /// separator pixel at the bottom: `line_height + 1.0`. The separator
    /// stroke sits in the query band's last pixel, and the first item row
    /// moves down by 1.0 pixel accordingly.
    ///
    /// Paints through the real Direct2D `HeadlessSurface` and pins all
    /// three rows: the reserved separator pixel carries border ink, the
    /// query text row above it does not, and the first item row below it
    /// carries the selected row's fill — i.e. the separator neither
    /// climbs into the text row nor steals (and get overpainted by) the
    /// first item row.
    #[test]
    fn separator_paints_at_corrected_row_not_drifted_one() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0_f32;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let theme = Theme::default();
        let layout = win_palette_layout(rect, &p, line_height);
        let qb = layout.query_bounds.expect("query bounds present");
        assert_eq!(
            qb.height,
            line_height + 1.0,
            "query band must reserve line_height + 1.0 (the +1 is the separator)",
        );

        let title_h = line_height; // native_surface_paint::layout: title_h = line_height
        let sep_y = (qb.y + qb.height - 1.0) as u32; // corrected: the reserved pixel
        let first_row_y = (qb.y + qb.height) as u32; // This was sep_y
        assert_eq!(sep_y, (title_h + line_height) as u32);
        assert_eq!(first_row_y, (title_h + line_height + 1.0) as u32);
        assert_eq!(
            layout.visible_items[0].bounds.y,
            qb.y + qb.height,
            "first item row starts immediately below the query band",
        );

        // `p`'s row 0 is the selected one, so the first item row paints
        // `selected_bg` — the fill that must not overwrite the separator.
        let probe_x = 150_u32;
        let is_border = |y: u32| {
            let px = surface.pixel_at(probe_x, y);
            (px.r, px.g, px.b) == (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b)
        };
        assert!(
            is_border(sep_y),
            "expected separator ink in the query band's reserved pixel, row {sep_y}",
        );
        assert!(
            !is_border(sep_y - 1),
            "separator climbed into the query *text* row {} — it belongs in the \
             single pixel query_bounds reserves for it",
            sep_y - 1,
        );
        let first_row_px = surface.pixel_at(probe_x, first_row_y);
        assert_eq!(
            (first_row_px.r, first_row_px.g, first_row_px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "row {first_row_y} is the first item row (selected), not the separator's",
        );
    }

    /// The item list flooring: when available item-list height is not an
    /// exact multiple of the resolved row height, the row count is floored
    /// and a reduced `viewport_height` is used to prevent partial-height
    /// last rows. The rect height (200) and `line_height` (18) are chosen
    /// so the available item area (159px after title/query/bottom-inset)
    /// is not an exact multiple of the row pitch — the condition that
    /// produces a partial last row without flooring.
    #[test]
    fn item_list_never_shows_a_partial_last_row() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut p = palette();
        p.items = (0..40)
            .map(|i| item(&format!("item {i}"), vec![]))
            .collect();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0_f32;
        let row_height = native_surface_paint::item_row_height(line_height);

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let last = layout
            .visible_items
            .last()
            .expect("some items should be visible");
        assert_eq!(
            last.bounds.height, row_height,
            "last visible row must be full height — a partial row means the \
             unfloored-viewport drift regressed",
        );

        // Driver-tier: paint through the real Direct2D backend and
        // confirm no ink bleeds into the row immediately below the
        // floored boundary — a partial 9th row would have painted glyph
        // ink there.
        let theme = Theme::default();
        let boundary_y = (last.bounds.y + last.bounds.height + 1.0) as u32;
        let probe_x = (last.bounds.x + 4.0) as u32;
        let px = surface.pixel_at(probe_x, boundary_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
            "expected plain surface_bg just below the floored last row, not a \
             partially-clipped row's content",
        );
    }

    #[test]
    fn selected_row_paints_selection_bg() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = palette();
        let rect = Rect::new(0.0, 0.0, 300.0, 200.0);
        let line_height = 18.0;

        surface
            .paint(|target| {
                draw_palette(target, &dwrite, rect, &p, line_height, false);
            })
            .expect("paint palette");

        let layout = win_palette_layout(rect, &p, line_height);
        let first_item = layout.visible_items[0].bounds;
        let sel = Theme::default().selected_bg;
        let px = surface.pixel_at(
            (first_item.x + first_item.width - 2.0) as u32,
            (first_item.y + 1.0) as u32,
        );
        assert_eq!((px.r, px.g, px.b), (sel.r, sel.g, sel.b));
    }
}
