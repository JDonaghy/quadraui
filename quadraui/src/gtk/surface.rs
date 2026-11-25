//! The shared [`PaintSurface`] adapter over a bare Cairo context.
//! Every primitive that uses this adapter is listed below, grouped by
//! two characteristics that vary per-primitive:
//!
//! 1. **`layout` presence.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!    [`crate::Split`] and [`crate::SplitTree`] paint with `cr` alone (no
//!    text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//!    [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//!    [`crate::ToastOverlay`] and [`crate::TextInput`]
//!    paint text too and need a live `pango::Layout`.
//!    [`CairoSurface::layout`] is `Option` so one struct covers both —
//!    [`Self::layout_or_panic`] panics only if a text verb is reached with
//!    `layout: None`.
//! 2. **Fill translucency.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!    `gtk::status_bar`, `gtk::diff_view` and `gtk::toast`
//!    fill with `set_source_rgba` (honouring `color.a`); [`crate::Form`],
//!    [`crate::Split`], [`crate::SplitTree`] and
//!    [`crate::TextInput`] fill with `set_source` (opaque-only).
//!    [`CairoSurface::translucent_fill`] carries that choice per call site.
//!
//! Every other verb (`surface_stroke_rect`, `surface_draw_text_run`,
//! `surface_draw_line`, `surface_push_clip`/`surface_pop_clip`,
//! `surface_draw_image`) is implemented unconditionally; primitives that
//! never call a given verb simply never reach that code.
//! `surface_draw_text_run_styled`/`surface_measure_text_styled` (bold-aware
//! Pango `AttrList`) are implemented here unconditionally; only `StatusBar`
//! uses them, making this inert for other primitives.
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly; no primitive that uses this adapter ever paints an image.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::backend::ImagePaintResult;
use crate::paint_surface::PaintSurface;
use crate::{Color, Image, Point, Rect, Viewport};

/// See the module doc for the two fields that carry genuine
/// per-call-site behaviour ([`Self::layout`], [`Self::translucent_fill`]);
/// every other [`PaintSurface`] verb below is one shared implementation.
pub(crate) struct CairoSurface<'a> {
    pub(crate) cr: &'a Context,
    /// `None` for primitives that never paint text through this adapter
    /// (`Scrollbar`, `DropOverlay`, `Split`, `SplitTree`) — see module doc
    /// point 1.
    pub(crate) layout: Option<&'a pango::Layout>,
    /// `true` selects `set_source_rgba` (honours `color.a`); `false`
    /// selects `set_source` (opaque-only) — see module doc point 2.
    pub(crate) translucent_fill: bool,
}

impl<'a> CairoSurface<'a> {
    /// Returns the live layout or panics — only reachable if a text verb
    /// is called on an adapter constructed with `layout: None`, which no
    /// current primitive does (see module doc point 1).
    fn layout_or_panic(&self) -> &'a pango::Layout {
        self.layout
            .expect("CairoSurface: text verb called without a pango::Layout")
    }
}

impl PaintSurface for CairoSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: Viewport) {
        unreachable!("CairoSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("CairoSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> Viewport {
        unreachable!("CairoSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("CairoSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("CairoSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let layout = self.layout_or_panic();
        layout.set_text(text);
        layout.set_attributes(None);
        let (w, h) = layout.pixel_size();
        (w as f32, h as f32)
    }

    fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
        let layout = self.layout_or_panic();
        layout.set_text(text);
        if bold {
            let attrs = pango::AttrList::new();
            attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
            layout.set_attributes(Some(&attrs));
        } else {
            layout.set_attributes(None);
        }
        let (w, h) = layout.pixel_size();
        layout.set_attributes(None);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        if self.translucent_fill {
            super::set_source_rgba(self.cr, color);
        } else {
            super::set_source(self.cr, color);
        }
        self.cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        self.cr.fill().ok();
    }

    /// Same clamp/fill recipe as `GtkBackend`'s own override,
    /// honouring [`Self::translucent_fill`] the same way
    /// [`Self::surface_fill_rect`] above does.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        if self.translucent_fill {
            super::set_source_rgba(self.cr, color);
        } else {
            super::set_source(self.cr, color);
        }
        let r = (radius as f64)
            .min(rect.width as f64 / 2.0)
            .min(rect.height as f64 / 2.0)
            .max(0.0);
        super::rounded_rect_path(
            self.cr,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            r,
        );
        self.cr.fill().ok();
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
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

    /// Same clamp/stroke recipe as `GtkBackend`'s own override.
    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    ) {
        super::set_source(self.cr, color);
        self.cr.set_line_width(stroke_width as f64);
        let r = (radius as f64)
            .min(rect.width as f64 / 2.0)
            .min(rect.height as f64 / 2.0)
            .max(0.0);
        super::rounded_rect_path(
            self.cr,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            r,
        );
        self.cr.stroke().ok();
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let layout = self.layout_or_panic();
        layout.set_text(text);
        layout.set_attributes(None);
        super::set_source(self.cr, color);
        self.cr.move_to(rect.x as f64, rect.y as f64);
        super::painted_text::show_layout(self.cr, layout);
    }

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
        let layout = self.layout_or_panic();
        layout.set_text(text);
        let attrs = pango::AttrList::new();
        if bold {
            attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
        }
        if italic {
            attrs.insert(pango::AttrInt::new_style(pango::Style::Italic));
        }
        if underline {
            attrs.insert(pango::AttrInt::new_underline(pango::Underline::Single));
        }
        layout.set_attributes(Some(&attrs));
        super::set_source(self.cr, color);
        if (scale_x - 1.0).abs() > f32::EPSILON {
            self.cr.save().ok();
            self.cr.translate(rect.x as f64, rect.y as f64);
            self.cr.scale(scale_x as f64, 1.0);
            self.cr.move_to(0.0, 0.0);
            super::painted_text::show_layout(self.cr, layout);
            self.cr.restore().ok();
        } else {
            self.cr.move_to(rect.x as f64, rect.y as f64);
            super::painted_text::show_layout(self.cr, layout);
        }
        layout.set_attributes(None);
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
        super::set_source(self.cr, color);
        self.cr.set_line_width(stroke_width as f64);
        self.cr.move_to(from.x as f64, from.y as f64);
        self.cr.line_to(to.x as f64, to.y as f64);
        self.cr.stroke().ok();
    }

    fn surface_push_clip(&mut self, rect: Rect) {
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

    fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
        ImagePaintResult::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    fn probe_pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    /// `CairoSurface::surface_fill_rounded_rect` must reach the
    /// same real rounded-corner Cairo path `GtkBackend`'s own override
    /// does (`gtk_backend_paint_surface_fill_rounded_rect_clips_the_corners`'s
    /// twin, at the adapter level) — proven by a corner staying
    /// untouched while the box's centre paints solid.
    #[test]
    fn cairo_surface_fill_rounded_rect_clips_the_corners() {
        let mut surface =
            ImageSurface::create(Format::ARgb32, 40, 40).expect("create ImageSurface");
        let red = Color::rgb(200, 20, 20);

        {
            let cr = Context::new(&surface).expect("Context::new");
            let mut adapter = CairoSurface {
                cr: &cr,
                layout: None,
                translucent_fill: true,
            };
            adapter.surface_fill_rounded_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 15.0, red);
        }

        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        assert_eq!(
            probe_pixel(&data, stride, 20, 20),
            (red.r, red.g, red.b),
            "surface_fill_rounded_rect must paint the solid color at the box's centre"
        );
        assert_eq!(
            probe_pixel(&data, stride, 1, 1),
            (0, 0, 0),
            "a corner pixel well inside a radius-15 fillet on a 40x40 box must stay \
             untouched"
        );
    }

    /// `CairoSurface::surface_stroke_rounded_rect` must reach the
    /// same real rounded-corner Cairo path `GtkBackend`'s own override
    /// does — the adapter-level twin of
    /// `gtk_backend_paint_surface_stroke_rounded_rect_clips_the_corners`.
    #[test]
    fn cairo_surface_stroke_rounded_rect_clips_the_corners() {
        let mut surface =
            ImageSurface::create(Format::ARgb32, 40, 40).expect("create ImageSurface");
        let blue = Color::rgb(60, 120, 220);

        {
            let cr = Context::new(&surface).expect("Context::new");
            let mut adapter = CairoSurface {
                cr: &cr,
                layout: None,
                translucent_fill: true,
            };
            adapter.surface_stroke_rounded_rect(Rect::new(0.0, 0.0, 40.0, 40.0), 15.0, blue, 4.0);
        }

        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        assert_eq!(
            probe_pixel(&data, stride, 20, 1),
            (blue.r, blue.g, blue.b),
            "the straight middle of the top edge must land the stroke colour"
        );
        assert_eq!(
            probe_pixel(&data, stride, 1, 1),
            (0, 0, 0),
            "a corner pixel well inside a radius-15 fillet on a 40x40 box must stay \
             untouched"
        );
        assert_eq!(
            probe_pixel(&data, stride, 20, 20),
            (0, 0, 0),
            "the box's centre, far from any stroke, must stay untouched"
        );
    }
}

/// The terminal's shared pixel painter, driven through `GtkBackend`'s
/// widget-contract paint against a Cairo `ImageSurface` and read back
/// pixel by pixel — terminal-cell background fill, wide glyphs and cursor
/// accents, without a display.
#[cfg(test)]
mod terminal_paint_tests {
    use crate::event::{Rect as QRect, Viewport};
    use crate::gtk::backend::GtkBackend;
    use crate::primitives::terminal::{Terminal, TerminalCell, TerminalCursorShape};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    const W: i32 = 200;
    const H: i32 = 40;
    const CHAR_W: f64 = 10.0;
    const LINE_H: f64 = 20.0;

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    /// ARgb32 stores each pixel as [B, G, R, A] in native (little-endian)
    /// byte order; `stride` is in bytes and may include padding.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

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

    fn paint(term: &Terminal) -> ImageSurface {
        let mut backend = GtkBackend::new();
        backend.set_current_theme(Theme::default());
        backend.set_current_line_height(LINE_H);
        backend.set_current_char_width(CHAR_W);
        Backend::begin_frame(&mut backend, Viewport::new(W as f32, H as f32, 1.0));
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            backend.enter_frame_scope(&cr, &pango_layout, |b| {
                crate::DrawsPrimitives::draw_primitive(
                    b,
                    QRect::new(0.0, 0.0, W as f32, H as f32),
                    term,
                );
            });
        }
        surface
    }

    /// A double-width glyph (CJK) followed by vt100's
    /// blank continuation cell must have its background span both
    /// columns — the continuation cell's own (different) background must
    /// NOT paint over the second half of the wide glyph's cell.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        // '日' is a double-width CJK character. Its vt100-derived
        // continuation cell carries a *different* background (cyan) to
        // prove the rasteriser doesn't just get lucky on matching colours.
        //
        // The wide glyph's foreground is set equal to its background
        // (magenta on magenta) so the glyph is invisible. This isolates the
        // thing under test — the two-column *background* fill — from the
        // glyph raster itself: with wide glyphs now scaled to fill the full
        // two-cell box (`wide_glyph_x_scale`), antialiased glyph ink reaches
        // deep into the second column and its exact colour depends on which
        // fallback font Pango picks, which varies by machine. A white glyph
        // here made the probe read a magenta/white blend on some hosts and
        // fail spuriously. Painting the glyph in the background colour keeps
        // the probe a pure, deterministic background read on every host,
        // while the glyph-scaling maths stays covered by the dedicated
        // `wide_glyph_x_scale` unit tests in `terminal_style`.
        let row = vec![cell('日', magenta, magenta), cell(' ', magenta, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Probe just past the first single-cell-width boundary, still
        // within the wide glyph's two-column span: must be magenta, not
        // cyan.
        let probe_x = (CHAR_W * 1.5) as i32;
        let (r, g, b) = pixel(&data, stride, probe_x, 5);
        assert_eq!(
            (r, g, b),
            (magenta.r, magenta.g, magenta.b),
            "wide cell's background should span both columns, not be \
             overpainted by the continuation cell's background"
        );
    }

    /// Companion regression: ordinary narrow (single-width) cells must
    /// still advance by exactly `char_width` — the wide-cell fix must not
    /// widen unrelated cells.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        let white = Color::rgb(255, 255, 255);
        // 'B' (the probed cell) paints its glyph in its own background
        // colour so antialiased glyph ink can't corrupt the background
        // probe; 'A' keeps a visible (white) glyph as it isn't probed.
        // This tests the *advance* logic — that 'A' occupies exactly one
        // char_width and doesn't bleed into 'B''s column — independent of
        // glyph rendering.
        let row = vec![cell('A', white, magenta), cell('B', cyan, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Just past the single-cell-width boundary: second cell's own
        // background (cyan) should already be showing.
        let probe_x = (CHAR_W * 1.5) as i32;
        let (r, g, b) = pixel(&data, stride, probe_x, 5);
        assert_eq!(
            (r, g, b),
            (cyan.r, cyan.g, cyan.b),
            "narrow cells must still advance by exactly char_width"
        );
    }

    // ── Cursor shape accent ─────────────────────────────────────────────
    //
    // These paint through the same shared `primitives::terminal::paint`
    // every pixel backend's terminal paint uses — see that
    // fn's own `cursor_accent_visible`/`cursor_accent_rect` call for the
    // logic under test. The probed cell's text is a blank space so no
    // glyph ink can land on the probed pixels (matching this module's
    // other pixel-probe tests' convention), isolating the accent fill
    // from glyph rasterisation.

    /// A `Bar` cursor paints a thin accent stripe at the cell's *left*
    /// edge in the cell's own foreground colour, and does **not** invert
    /// the rest of the cell (unlike `Block`).
    #[test]
    fn bar_cursor_paints_a_left_edge_accent_stripe() {
        let fg = Color::rgb(255, 0, 0);
        let bg = Color::rgb(0, 0, 0);
        let mut c = cell(' ', fg, bg);
        c.is_cursor = true;
        c.cursor_shape = TerminalCursorShape::Bar;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Inside the ~2px accent stripe at the cell's left edge.
        let (r, g, b) = pixel(&data, stride, 1, 5);
        assert_eq!(
            (r, g, b),
            (fg.r, fg.g, fg.b),
            "bar cursor accent should paint at the cell's left edge"
        );
        // Well past the stripe, still inside the (10px-wide) cell: the
        // cell's own (uninverted) background, not the accent colour.
        let (r2, g2, b2) = pixel(&data, stride, 8, 5);
        assert_eq!(
            (r2, g2, b2),
            (bg.r, bg.g, bg.b),
            "bar cursor must not invert the rest of the cell"
        );
    }

    /// An `Underline` cursor paints a thin accent stripe at the cell's
    /// *bottom* edge, and likewise does not invert the rest of the cell.
    #[test]
    fn underline_cursor_paints_a_bottom_edge_accent_stripe() {
        let fg = Color::rgb(0, 255, 0);
        let bg = Color::rgb(0, 0, 0);
        let mut c = cell(' ', fg, bg);
        c.is_cursor = true;
        c.cursor_shape = TerminalCursorShape::Underline;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        // Inside the ~2px accent stripe at the cell's bottom edge
        // (LINE_H = 20.0).
        let (r, g, b) = pixel(&data, stride, 5, 19);
        assert_eq!(
            (r, g, b),
            (fg.r, fg.g, fg.b),
            "underline cursor accent should paint at the cell's bottom edge"
        );
        // Near the top of the same cell: the cell's own background.
        let (r2, g2, b2) = pixel(&data, stride, 5, 2);
        assert_eq!(
            (r2, g2, b2),
            (bg.r, bg.g, bg.b),
            "underline cursor must not invert the rest of the cell"
        );
    }

    /// A `Block` cursor (the default shape) still inverts the
    /// whole cell — no separate accent stripe.
    #[test]
    fn block_cursor_still_inverts_whole_cell() {
        let fg = Color::rgb(255, 0, 0);
        let bg = Color::rgb(0, 0, 0);
        let mut c = cell(' ', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let mut s = paint(&term);
        s.flush();
        let stride = s.stride() as usize;
        let data = s.data().expect("surface data");

        let (r, g, b) = pixel(&data, stride, 5, 10);
        assert_eq!(
            (r, g, b),
            (fg.r, fg.g, fg.b),
            "block cursor should invert: the whole cell shows the cell's own fg as its background"
        );
    }

    // The paint-level contract of repainting only the rows in `dirty_rows`
    // is covered by `primitives::terminal::native_surface_paint::tests::
    // dirty_rows_filter_skips_untouched_rows`, which runs on every backend.
}
