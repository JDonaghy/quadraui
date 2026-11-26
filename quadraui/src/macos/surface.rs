//! The one shared [`PaintSurface`] adapter over a bare `CGContextRef`.
//! Every adapter fills through `super::backend::ns_fill_rect`, which honours
//! `color.a` with a real alpha blend. This module varies only in one axis:
//!
//! - **`font` presence.** [`crate::Scrollbar`], [`crate::DropOverlay`],
//!   [`crate::Split`] and [`crate::SplitTree`] paint with `ctx` alone (no
//!   text); [`crate::Form`], `crate::primitives::sidebar_panel`,
//!   [`crate::StatusBar`], [`crate::primitives::diff_view::DiffView`],
//!   [`crate::ToastOverlay`] paint text too and need a
//!   live `&CTFont`. [`CgSurface::font`] is `Option` so one struct covers
//!   both — [`Self::font_or_panic`] panics only if a text verb is reached
//!   with `font: None`, which never happens in practice.
//!
//! Every other verb (`surface_stroke_rect`, `surface_draw_text_run`,
//! `surface_draw_line`, `surface_push_clip`/`surface_pop_clip`,
//! `surface_draw_image`) is implemented unconditionally; primitives that
//! never call a given verb simply never reach that code.
//! `surface_measure_text_styled`/`surface_draw_text_run_styled` (bold-aware)
//! take the trait's default everywhere on macOS — macOS ignores a segment's
//! own `bold` weight, unlike GTK/Win.
//!
//! [`Self::surface_line_height`] computes `ascent + descent + leading` on
//! [`Self::font`] directly via [`super::text::font_metrics`] — the same
//! derivation [`crate::macos::MacBackend::set_chrome_font`] uses to seed
//! `chrome_line_height`, and `MacBackend` has no override setter for that
//! field (unlike `current_line_height`), so the two can never disagree.
//!
//! `surface_draw_image` returns [`ImagePaintResult::Unsupported`]
//! uniformly, matching `MacBackend::draw_image`'s contract (no decoder
//! wired up yet); no primitive using this adapter paints an image.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::backend::ImagePaintResult;
use crate::paint_surface::{AlphaSheet, PaintSurface};
use crate::{Color, Image, Point, Rect, Viewport};

use super::text::{draw_text, measure_text};

/// See the module doc for [`Self::font`], the one field that carries
/// genuine per-call-site behaviour; every other [`PaintSurface`] verb
/// below is one shared implementation.
pub(crate) struct CgSurface<'a> {
    pub(crate) ctx: CGContextRef,
    /// `None` for primitives that never paint text through this adapter
    /// (`Scrollbar`, `DropOverlay`, `Split`, `SplitTree`) — see module
    /// doc.
    pub(crate) font: Option<&'a CTFont>,
}

impl<'a> CgSurface<'a> {
    /// Returns the live font or panics — only reachable if a text verb
    /// is called on an adapter constructed with `font: None`, which no
    /// current primitive does (see module doc).
    fn font_or_panic(&self) -> &'a CTFont {
        self.font
            .expect("CgSurface: text verb called without a CTFont")
    }
}

thread_local! {
    /// The last sheet [`glyph_alpha_sheet`] rendered, keyed on its family,
    /// characters and cell size, so a repeated request shapes nothing.
    static GLYPH_SHEET: std::cell::RefCell<Option<(String, String, (u32, u32), AlphaSheet)>> =
        const { std::cell::RefCell::new(None) };
}

/// [`PaintSurface::surface_glyph_alpha_sheet`] for Core Text: each char of
/// `chars` drawn bold in `family`, sized to `cell`'s height, white onto a
/// zeroed offscreen bitmap whose red channel is then the coverage.
pub(crate) fn glyph_alpha_sheet(
    family: &str,
    chars: &str,
    cell: crate::primitive::Size,
) -> Option<AlphaSheet> {
    let cell_px = (cell.width.round() as u32, cell.height.round() as u32);
    let hit = GLYPH_SHEET.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(f, ch, sz, _)| f == family && ch == chars && *sz == cell_px)
            .map(|(_, _, _, sheet)| sheet.clone())
    });
    if hit.is_some() {
        return hit;
    }
    let sheet = render_glyph_sheet(family, chars, cell_px)?;
    GLYPH_SHEET.with(|c| {
        *c.borrow_mut() = Some((
            family.to_string(),
            chars.to_string(),
            cell_px,
            sheet.clone(),
        ))
    });
    Some(sheet)
}

fn render_glyph_sheet(
    family: &str,
    chars: &str,
    (cell_w, cell_h): (u32, u32),
) -> Option<AlphaSheet> {
    use core_text::font_descriptor::kCTFontBoldTrait;
    let count = chars.chars().count() as u32;
    let (sheet_w, sheet_h) = (cell_w * count, cell_h);
    if sheet_w == 0 || sheet_h == 0 {
        return None;
    }
    let base_font = super::text::make_font(family, f64::from(cell_h) * 0.85)?;
    // A family with no bold face keeps its regular weight.
    let font = base_font
        .clone_with_symbolic_traits(kCTFontBoldTrait, kCTFontBoldTrait)
        .unwrap_or(base_font);
    let surface = super::headless::BitmapSurface::new(sheet_w, sheet_h);
    // `draw_text` records into the thread's painted-text sink; this sheet
    // is offscreen, so record into a scratch sink and discard it.
    let prev_sink = crate::testing::install_text_run_sink();
    for (i, ch) in chars.chars().enumerate() {
        // SAFETY: `surface.context_ptr()` is a live `CGContextRef` for the
        // surface's lifetime, which outlives this loop.
        unsafe {
            draw_text(
                surface.context_ptr(),
                &font,
                &ch.to_string(),
                f64::from(i as u32 * cell_w),
                0.0,
                (1.0, 1.0, 1.0, 1.0),
            );
        }
    }
    let _ = crate::testing::take_text_run_sink(prev_sink);
    let mut alpha = vec![0u8; (sheet_w * sheet_h) as usize];
    for y in 0..sheet_h {
        for x in 0..sheet_w {
            // White over transparent, premultiplied: red equals coverage.
            alpha[(y * sheet_w + x) as usize] = surface.pixel(x, y).0;
        }
    }
    Some(AlphaSheet::new(sheet_w, sheet_h, alpha))
}

/// [`PaintSurface::surface_blit_alpha`] for Core Graphics: one
/// `color`-at-coverage fill per non-zero pixel of `alpha`, each
/// `dest.width / width` by `dest.height / height` points.
///
/// # Safety
///
/// `ctx` must be a valid, live `CGContextRef` for the duration of this call.
pub(crate) unsafe fn blit_alpha(
    ctx: CGContextRef,
    dest: Rect,
    alpha: &[u8],
    width: u32,
    height: u32,
    color: Color,
) {
    use super::cg::{rect, CGContextFillRect, CGContextSetRGBFillColor};
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || alpha.len() < w * h {
        return;
    }
    let (r, g, b, _) = super::cg::color_to_cg(color);
    let px_w = f64::from(dest.width) / f64::from(width);
    let px_h = f64::from(dest.height) / f64::from(height);
    for row in 0..h {
        for col in 0..w {
            let coverage = alpha[row * w + col];
            if coverage == 0 {
                continue;
            }
            CGContextSetRGBFillColor(ctx, r, g, b, f64::from(coverage) / 255.0);
            CGContextFillRect(
                ctx,
                rect(
                    f64::from(dest.x) + col as f64 * px_w,
                    f64::from(dest.y) + row as f64 * px_h,
                    px_w,
                    px_h,
                ),
            );
        }
    }
}

impl PaintSurface for CgSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: Viewport) {
        unreachable!("CgSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("CgSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> Viewport {
        unreachable!("CgSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        super::text::font_metrics(self.font_or_panic()).line_height as f32
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("CgSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let (w, h) = measure_text(self.font_or_panic(), text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's paint
        // pass — see this struct's construction sites. `ns_fill_rect`
        // already honours `color.a` with a real alpha blend (see module
        // doc — there is no opaque-only variant to pick between here,
        // unlike `gtk::surface::CairoSurface`).
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
    }

    /// `ns_fill_rounded_rect`'s twin of [`Self::surface_fill_rect`]
    /// above — same SAFETY contract.
    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_fill_rounded_rect(self.ctx, rect, radius, color) };
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_stroke_rect(self.ctx, rect, color, stroke_width as f64) };
    }

    /// `ns_stroke_rounded_rect`'s twin of [`Self::surface_stroke_rect`]
    /// above — same SAFETY contract.
    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    ) {
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            super::backend::ns_stroke_rounded_rect(
                self.ctx,
                rect,
                radius,
                color,
                stroke_width as f64,
            )
        };
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let font = self.font_or_panic();
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            draw_text(
                self.ctx,
                font,
                text,
                rect.x as f64,
                rect.y as f64,
                super::backend::ns_color_to_cg(color),
            );
        }
    }

    fn surface_draw_text_line(
        &mut self,
        origin: Point,
        text: &str,
        runs: &[crate::TextRunStyle],
        _role: crate::FontRole,
    ) {
        let font = self.font_or_panic();
        // SAFETY: see `surface_fill_rect`.
        unsafe {
            super::text::draw_text_line(
                self.ctx,
                font,
                origin.x as f64,
                origin.y as f64,
                text,
                runs,
            )
        };
    }

    fn surface_text_x_at_index(
        &mut self,
        text: &str,
        _role: crate::FontRole,
        byte_idx: usize,
    ) -> f32 {
        super::text::text_x_at_index(self.font_or_panic(), text, byte_idx)
    }

    fn surface_text_index_at_x(&mut self, text: &str, _role: crate::FontRole, x: f32) -> usize {
        super::text::text_index_at_x(self.font_or_panic(), text, x)
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
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

    fn surface_push_clip(&mut self, rect: Rect) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_push_clip(self.ctx, rect) };
    }

    fn surface_pop_clip(&mut self) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { super::backend::ns_pop_clip(self.ctx) };
    }

    fn surface_blit_alpha(
        &mut self,
        dest: Rect,
        alpha: &[u8],
        width: u32,
        height: u32,
        color: Color,
    ) {
        // SAFETY: see `surface_fill_rect`.
        unsafe { blit_alpha(self.ctx, dest, alpha, width, height, color) };
    }

    /// Shapes in the adapter's own font's family; `role` is not
    /// consulted, as for every other text verb here.
    fn surface_glyph_alpha_sheet(
        &mut self,
        chars: &str,
        _role: crate::FontRole,
        cell: crate::primitive::Size,
    ) -> Option<AlphaSheet> {
        glyph_alpha_sheet(&self.font?.family_name(), chars, cell)
    }

    fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
        // Matches `MacBackend::draw_image`: no `NSImage` decoder wired up
        // yet.
        ImagePaintResult::Unsupported
    }
}

/// The terminal's shared pixel painter against a CoreGraphics bitmap, both
/// through `MacBackend`'s widget-contract paint and through [`CgSurface`]
/// directly — cell backgrounds, wide glyphs, overlays and clipping.
#[cfg(test)]
mod tests {
    use crate::event::{Rect as QRect, Viewport};
    use crate::macos::headless::BitmapSurface;
    use crate::macos::text::make_font;
    use crate::macos::MacBackend;
    use crate::primitives::terminal::{Terminal, TerminalCell, TerminalCursorShape};
    use crate::terminal_style::wide_glyph_x_scale;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;
    use core_text::font::CTFont;

    const W: u32 = 200;
    const H: u32 = 120;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    /// A line with a wide glyph, a combining mark and mixed bold/italic.
    const LINE: &str = "a日e\u{301}bc";

    fn line_runs() -> Vec<crate::TextRunStyle> {
        let fg = Color::rgb(230, 230, 230);
        vec![
            crate::TextRunStyle::new(0..1, fg),
            crate::TextRunStyle::new(1..4, Color::rgb(220, 60, 60)).with_bold(true),
            crate::TextRunStyle::new(4..7, fg).with_italic(true),
            crate::TextRunStyle::new(7..9, fg),
        ]
    }

    /// `surface_draw_text_line` paints exactly what the editor's per-run
    /// Core Text paint does: each run at its prefix's measured advance.
    #[test]
    fn draw_text_line_matches_the_editors_per_run_paint() {
        use crate::paint_surface::PaintSurface;
        let f = font();
        let runs = line_runs();
        let via_verb = BitmapSurface::new(W, 30);
        via_verb.fill(0.0, 0.0, 0.0, 0.0);
        let mut raw = super::CgSurface {
            ctx: via_verb.context_ptr(),
            font: Some(&f),
        };
        raw.surface_draw_text_line(
            crate::Point::new(4.0, 3.0),
            LINE,
            &runs,
            crate::FontRole::Editor,
        );

        let reference = BitmapSurface::new(W, 30);
        reference.fill(0.0, 0.0, 0.0, 0.0);
        for run in &runs {
            let (prefix_w, _) = crate::macos::text::measure_text(&f, &LINE[..run.range.start]);
            // SAFETY: the bitmap context lives for this whole test.
            unsafe {
                crate::macos::text::draw_text(
                    reference.context_ptr(),
                    &f,
                    &LINE[run.range.clone()],
                    4.0 + prefix_w,
                    3.0,
                    crate::macos::cg::color_to_cg(run.fg),
                )
            };
        }
        assert!(via_verb.bytes().iter().any(|&b| b != 0));
        assert_eq!(via_verb.bytes(), reference.bytes());
    }

    #[test]
    fn text_x_and_index_agree_with_core_text_advances() {
        use crate::paint_surface::PaintSurface;
        let f = font();
        let mut raw = super::CgSurface {
            ctx: std::ptr::null_mut(),
            font: Some(&f),
        };
        let role = crate::FontRole::Editor;
        for idx in [0usize, 1, 4, 7, 8, 9] {
            let expect = crate::macos::text::measure_text(&f, &LINE[..idx]).0 as f32;
            let x = raw.surface_text_x_at_index(LINE, role, idx);
            assert_eq!(x, expect, "x at byte {idx}");
            assert_eq!(
                raw.surface_text_index_at_x(LINE, role, x),
                idx,
                "index at x {x}"
            );
        }
        let narrow = raw.surface_text_x_at_index(LINE, role, 1);
        let wide = raw.surface_text_x_at_index(LINE, role, 4) - narrow;
        assert!(wide > narrow * 1.5, "wide {wide} narrow {narrow}");
        // Between `e` and its combining mark is not a boundary.
        let e_x = raw.surface_text_x_at_index(LINE, role, 4);
        let next = raw.surface_text_x_at_index(LINE, role, 7);
        assert_eq!(
            raw.surface_text_index_at_x(LINE, role, e_x + (next - e_x) * 0.4),
            4
        );
        assert_eq!(
            raw.surface_text_index_at_x(LINE, role, e_x + (next - e_x) * 0.6),
            7
        );
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

    fn paint(term: &Terminal) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            crate::DrawsPrimitives::draw_primitive(
                b,
                QRect::new(0.0, 0.0, W as f32, H as f32),
                term,
            );
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn cell_bg_paints_through_backend() {
        // One cell, magenta background, glyph 'A'. The fill_rect path
        // should colour the whole cell magenta before the glyph paints.
        let magenta = Color::rgb(200, 30, 200);
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('A', Color::rgb(255, 255, 255), magenta)]],
            scrollbar: None,
        };
        let s = paint(&term);
        // Probe a corner of the cell that the glyph "A" doesn't reach
        // (glyph sits roughly in the middle); top-left of the cell is
        // background.
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!((r, g, b), (magenta.r, magenta.g, magenta.b));
    }

    /// A double-width glyph (CJK) followed by vt100's blank continuation cell must
    /// have its background span both columns — the continuation cell's own
    /// (different) background must NOT paint over the second half of the wide glyph's
    /// cell. This test paints via `CgSurface` directly (rather than through
    /// `MacBackend`'s widget-contract paint) to control `char_width` explicitly instead
    /// of depending on Menlo's measured advance.
    #[test]
    fn wide_cell_background_spans_two_columns() {
        const CHAR_W: f32 = 10.0;
        const LINE_H: f32 = 20.0;

        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        // The wide glyph's foreground is set equal to its background
        // (magenta on magenta) so any antialiased glyph ink can't shift
        // the probed colour — same trick as the GTK twin test, and for
        // the same reason (font-fallback ink at the probe point would
        // otherwise vary by host).
        let row = vec![cell('日', magenta, magenta), cell(' ', magenta, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let theme = Theme::default();
        let f = font();
        let mut raw = super::CgSurface {
            ctx: surface.context_ptr(),
            font: Some(&f),
        };
        crate::primitives::terminal::paint(
            &term, &mut raw, &theme, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, None,
        );

        // Probe just past the first single-cell-width boundary, still
        // within the wide glyph's two-column span: must be magenta, not
        // cyan.
        let probe_x = (CHAR_W * 1.5) as u32;
        let (r, g, b, _) = surface.pixel(probe_x, 5);
        assert_eq!(
            (r, g, b),
            (magenta.r, magenta.g, magenta.b),
            "wide cell's background should span both columns, not be \
             overpainted by the continuation cell's background"
        );
    }

    /// Companion regression: ordinary narrow (single-width) cells must
    /// still advance by exactly `char_width` — the wide-cell fix must
    /// not widen unrelated cells. Mirrors `gtk::backend::tests`'s
    /// `narrow_cells_advance_by_single_char_width`.
    #[test]
    fn narrow_cells_advance_by_single_char_width() {
        const CHAR_W: f32 = 10.0;
        const LINE_H: f32 = 20.0;

        let magenta = Color::rgb(200, 30, 200);
        let cyan = Color::rgb(30, 200, 200);
        let white = Color::rgb(255, 255, 255);
        // 'B' (the probed cell) paints its glyph in its own background
        // colour so antialiased glyph ink can't corrupt the background
        // probe.
        let row = vec![cell('A', white, magenta), cell('B', cyan, cyan)];
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let theme = Theme::default();
        let f = font();
        let mut raw = super::CgSurface {
            ctx: surface.context_ptr(),
            font: Some(&f),
        };
        crate::primitives::terminal::paint(
            &term, &mut raw, &theme, 0.0, 0.0, W as f32, H as f32, LINE_H, CHAR_W, None,
        );

        // Just past the single-cell-width boundary: second cell's own
        // background (cyan) should already be showing.
        let probe_x = (CHAR_W * 1.5) as u32;
        let (r, g, b, _) = surface.pixel(probe_x, 5);
        assert_eq!(
            (r, g, b),
            (cyan.r, cyan.g, cyan.b),
            "narrow cells must still advance by exactly char_width"
        );
    }

    /// Mirrors the GTK twin — both backends share `wide_glyph_x_scale`,
    /// so a CJK glyph measuring 15px in an 18px (2 × 9px) box scales
    /// 1.2x on macOS too.
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

    /// Mirrors the GTK twin.
    #[test]
    fn exact_fit_wide_glyph_is_not_scaled() {
        assert_eq!(wide_glyph_x_scale(18.0, 18.0), 1.0);
    }

    /// Mirrors the GTK twin.
    #[test]
    fn over_wide_glyph_is_shrunk_into_box() {
        let scale = wide_glyph_x_scale(24.0, 18.0);
        assert!(
            scale < 1.0,
            "24px glyph in 18px box should shrink, got {scale}"
        );
        assert!((24.0 * scale - 18.0).abs() < 1e-9);
    }

    #[test]
    fn cursor_cell_inverts_fg_bg() {
        // Cursor flag flips the cell so bg is painted with the cell's fg.
        let fg = Color::rgb(10, 220, 30);
        let bg = Color::rgb(40, 40, 40);
        let mut c = cell('X', fg, bg);
        c.is_cursor = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let (r, g, b, _) = s.pixel(0, 0);
        // Cursor swap: bg now uses fg.
        assert_eq!((r, g, b), (fg.r, fg.g, fg.b));
    }

    #[test]
    fn find_active_cell_paints_orange() {
        let mut c = cell('z', Color::rgb(255, 255, 255), Color::rgb(20, 20, 20));
        c.is_find_active = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!((r, g, b), (255, 165, 0));
    }

    #[test]
    fn selected_cell_uses_theme_selection_bg() {
        let mut c = cell('x', Color::rgb(255, 255, 255), Color::rgb(10, 10, 10));
        c.selected = true;
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let s = paint(&term);
        let theme = Theme::default();
        let (r, g, b, _) = s.pixel(0, 0);
        assert_eq!(
            (r, g, b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
        );
    }

    #[test]
    fn cells_past_width_are_clipped() {
        // Build a row of 200 magenta cells. At a typical char_width of
        // ~8px, only ~25 fit in W=200. Past the clip we should still
        // see transparent black (surface initial fill).
        let magenta = Color::rgb(200, 30, 200);
        let row: Vec<_> = (0..200)
            .map(|_| cell(' ', Color::rgb(255, 255, 255), magenta))
            .collect();
        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![row],
            scrollbar: None,
        };
        let s = paint(&term);
        // Right-most pixel column: cells should NOT have painted there
        // because clipping kicks in at `x + char_width > x + W` — and
        // 200 / char_width < 200 cells fit.
        // Probe just inside the right edge: this should still be magenta
        // (one of the cells fits there).
        let inside = s.pixel(W / 2, 0);
        assert_eq!(
            (inside.0, inside.1, inside.2),
            (magenta.r, magenta.g, magenta.b)
        );
    }

    /// Rows past the pane bottom are clipped.
    ///
    /// A grid taller than the pane — exactly what an un-debounced
    /// interactive-resize frame produces — must stop at the pane bottom
    /// rather than bleed into whatever the host paints below.
    #[test]
    fn rows_past_the_pane_bottom_are_clipped() {
        let magenta = Color::rgb(200, 30, 200);
        // 60 rows at ~16pt line height is ~960pt of grid for a 60pt pane.
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

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let pane_h = 60.0_f32;
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            crate::DrawsPrimitives::draw_primitive(
                b,
                QRect::new(0.0, 0.0, W as f32, pane_h),
                &term,
            );
        });
        backend.end_frame();

        // Inside the pane: painted.
        let (r, g, b, _) = surface.pixel(4, 4);
        assert_eq!(
            (r, g, b),
            (magenta.r, magenta.g, magenta.b),
            "the pane's own rows should still paint",
        );

        // Well below the pane bottom: untouched (transparent black).
        let below = surface.pixel(4, H - 4);
        assert_eq!(
            below,
            (0, 0, 0, 0),
            "rows below the pane bottom must be clipped, not bled",
        );
    }

    /// `cargo test -p quadraui --no-default-features --features macos -- --ignored --nocapture macos::surface::tests::dump_smoke_ppm`
    ///
    /// Paints a sample terminal — three rows of mixed cells, a cursor
    /// cell, two find matches with one active, a selection run, plus a
    /// scrollbar — and writes `/tmp/quadraui_terminal.ppm`. Open in
    /// Preview to confirm:
    /// - Row 0 reads `quadraui terminal` in white on dark.
    /// - Row 1 has an inverted (white-on-green) cursor inside the
    ///   word `cursor`.
    /// - Row 2 shows the orange find-active match and dim find-match
    ///   highlights, plus a contiguous selection band using
    ///   `theme.selection_bg`.
    /// - Right edge has a scrollbar gutter + thumb (~50% down the
    ///   track, since `scroll_offset = 50` of 100 lines, 16 visible).
    #[test]
    #[ignore = "writes /tmp/quadraui_terminal.ppm — opt in with --ignored"]
    fn dump_smoke_ppm() {
        use crate::primitives::terminal::TerminalScrollbar;

        let white = Color::rgb(230, 230, 230);
        let dark = Color::rgb(20, 20, 30);
        let green = Color::rgb(60, 200, 90);

        fn row(s: &str, fg: Color, bg: Color) -> Vec<TerminalCell> {
            s.chars()
                .map(|ch| TerminalCell {
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
                })
                .collect()
        }

        let r0 = row("quadraui terminal", white, dark);
        let mut r1 = row("  cursor here", white, dark);
        // Inverted cursor on the 'c' of "cursor".
        r1[2].fg = green;
        r1[2].is_cursor = true;
        let mut r2 = row("  find foo bar foo qux", white, dark);
        // Active find on first "foo" (chars 7..10), dim match on second
        // "foo" (chars 15..18).
        for c in r2.iter_mut().take(10).skip(7) {
            c.is_find_active = true;
        }
        for c in r2.iter_mut().take(18).skip(15) {
            c.is_find_match = true;
        }
        // Selection on " bar " (chars 10..15).
        for c in r2.iter_mut().take(15).skip(10) {
            c.selected = true;
        }

        let term = Terminal {
            id: WidgetId::new("term"),
            cells: vec![r0, r1, r2],
            scrollbar: Some(TerminalScrollbar {
                total_lines: 100,
                visible_lines: 16,
                scroll_offset: 50,
                inverted: false,
                width: None,
            }),
        };
        let s = paint(&term);
        s.write_ppm_and_open("/tmp/quadraui_terminal.ppm");
    }
}
