//! macOS (Core Graphics + Core Text) rasteriser for
//! [`crate::primitives::diff_view::DiffView`].
//!
//! Painting moved to the shared
//! [`crate::primitives::diff_view::native_surface_paint::paint`] (#866,
//! `NativeSurface` Phase 2d slice 9/9) — see that fn's doc for the two
//! named divergences (row/header text vertical alignment; header-label
//! ellipsize vs. hard-clip) found while unifying
//! `gtk::diff_view::draw_diff_view`, `macos::diff_view::draw_diff_view`
//! and `win::diff_view::draw_diff_view` into one implementation. This
//! module now only carries [`RawMacDiffViewSurface`] and the deprecated
//! [`draw_diff_view`] compatibility shim over it, mirroring
//! `macos::status_bar::RawMacStatusBarSurface` (#860).
//!
//! Before #737 landed, `MacBackend::draw_diff_view` painted nothing and
//! returned `visible_rows: 0`, which silently pinned every host's scroll
//! clamp to zero (quadraui#484 §4) — the shared paint below inherits
//! that fix via [`DiffView::layout`].
//!
//! # Safety
//!
//! `unsafe` here is confined to [`RawMacDiffViewSurface`]'s trait impl,
//! which forwards to [`super::backend::ns_fill_rect`]/[`ns_push_clip`]/
//! [`ns_pop_clip`](super::backend::ns_pop_clip) and [`super::text::draw_text`] —
//! each requires a valid `CGContextRef` borrowed for the duration of the
//! call, the same contract [`RawMacDiffViewSurface`]'s constructor sites
//! (the deprecated [`draw_diff_view`] shim, and this module's own tests)
//! uphold.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::native_surface::NativeSurface;
use crate::primitives::diff_view::{DiffView, DiffViewLayout};
use crate::theme::Theme;

/// Minimal [`NativeSurface`] adapter over a bare `CGContextRef` + font,
/// used only by the deprecated [`draw_diff_view`] shim below — mirrors
/// `macos::status_bar::RawMacStatusBarSurface`'s identical pattern
/// (#860).
struct RawMacDiffViewSurface<'a> {
    ctx: CGContextRef,
    font: &'a CTFont,
}

impl NativeSurface for RawMacDiffViewSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawMacDiffViewSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawMacDiffViewSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawMacDiffViewSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawMacDiffViewSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawMacDiffViewSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let (w, h) = super::text::measure_text(self.font, text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's paint
        // pass — see this struct's construction site.
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
    }

    fn surface_stroke_rect(
        &mut self,
        _rect: crate::Rect,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("DiffView::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        // SAFETY: `self.ctx` is the caller-supplied context passed to
        // `draw_diff_view`, valid for the duration of the shim call.
        unsafe {
            super::text::draw_text(
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
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("DiffView::paint never strokes a line")
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
        unreachable!("DiffView::paint never draws an image")
    }
}

/// Deprecated free-function shim (#866, CLAUDE.md rule 8): reproduces
/// the pre-#866 signature exactly for any external caller that held a
/// direct `quadraui::macos::draw_diff_view` reference rather than going
/// through [`crate::Backend::draw_diff_view`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call (typical: the frame-scope pointer stashed on [`super::MacBackend`]).
/// Calling with a freed or null pointer is UB.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_diff_view` instead — this free function is a compatibility shim over the shared #866 implementation"
)]
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_diff_view(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    view: &DiffView,
    theme: &Theme,
    line_height: f64,
) -> DiffViewLayout {
    let mut surface = RawMacDiffViewSurface { ctx, font };
    crate::primitives::diff_view::native_surface_paint::paint(
        view,
        &mut surface,
        theme,
        crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32),
        line_height as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::diff_view::DiffMode;
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 400;
    const H: u32 = 200;

    /// Divider width in points — mirrors the shared
    /// `primitives::diff_view::DIFF_DIVIDER_W` value the geometry uses.
    const DIVIDER_PX: f64 = crate::primitives::diff_view::DIFF_DIVIDER_W as f64;

    fn sample_view(mode: DiffMode) -> DiffView {
        let left = "one\ntwo\nthree\n";
        let right = "one\nTWO\nthree\n";
        DiffView {
            id: WidgetId::new("diff"),
            left: left.into(),
            right: right.into(),
            left_label: None,
            right_label: None,
            hunks: crate::diff::compute_hunks(left, right),
            mode,
            editability: Default::default(),
            scroll_offset: 0,
            focused_pane: Default::default(),
            has_focus: false,
        }
    }

    /// Index of the first row of `kind` in the flattened row list, i.e.
    /// the screen row it lands on when `scroll_offset == 0`.
    fn first_row_of(view: &DiffView, kind: crate::primitives::diff_view::DiffRowKind) -> usize {
        view.hunks
            .iter()
            .flat_map(|h| h.rows.iter())
            .position(|r| r.kind == kind)
            .expect("fixture should contain the requested row kind")
    }

    /// Paint `view` via the real [`Backend::draw_diff_view`] trait
    /// method — which now routes through the shared `native_surface_paint::paint`
    /// — rather than the deprecated free-function shim, so these tests
    /// don't trip the `-D warnings`-denied `deprecated` lint (CLAUDE.md
    /// rule 3; mirrors `gtk::backend`'s identical test-migration note).
    fn paint_via_backend(view: &DiffView, rect: QRect) -> (BitmapSurface, DiffViewLayout, f64) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 12.0).expect("Menlo installed"));
        let lh = backend.line_height() as f64;
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let captured = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *captured.borrow_mut() = Some(b.draw_diff_view(rect, view));
        });
        backend.end_frame();
        (surface, captured.into_inner().expect("layout captured"), lh)
    }

    /// The degenerate impl this replaced returned `visible_rows: 0` and
    /// painted nothing, which pinned host scroll clamps to zero.
    #[test]
    fn visible_rows_is_not_stuck_at_zero() {
        let view = sample_view(DiffMode::SideBySide);
        let (_surface, layout, lh) =
            paint_via_backend(&view, QRect::new(0.0, 0.0, W as f32, H as f32));
        assert!(
            layout.visible_rows > 0,
            "visible_rows must reflect the real row capacity, not the old 0 stub",
        );
        assert_eq!(layout.visible_rows, (H as f64 / lh).floor() as usize);
        assert_eq!(layout.total_rows, view.total_rows());
    }

    #[test]
    fn changed_row_paints_removed_and_added_backgrounds() {
        let view = sample_view(DiffMode::SideBySide);
        let (surface, _layout, lh) =
            paint_via_backend(&view, QRect::new(0.0, 0.0, W as f32, H as f32));
        let theme = Theme::default();

        // No labels, so no header row offsets the content rows: screen
        // row N sits at `N * line_height`.
        let changed = first_row_of(&view, crate::primitives::diff_view::DiffRowKind::Changed);
        let row_y = (lh * (changed as f64 + 0.5)) as u32;
        let left_w = ((W as f64 - DIVIDER_PX) / 2.0).floor();

        // Probe the right end of the left pane, clear of glyphs.
        let (r, g, b, _) = surface.pixel(left_w as u32 - 4, row_y);
        assert_eq!(
            (r, g, b),
            (
                theme.diff_removed_bg.r,
                theme.diff_removed_bg.g,
                theme.diff_removed_bg.b
            ),
            "left pane of a Changed row should carry diff_removed_bg",
        );

        let (r, g, b, _) = surface.pixel(W - 4, row_y);
        assert_eq!(
            (r, g, b),
            (
                theme.diff_added_bg.r,
                theme.diff_added_bg.g,
                theme.diff_added_bg.b
            ),
            "right pane of a Changed row should carry diff_added_bg",
        );
    }

    #[test]
    fn divider_paints_border_colour() {
        let view = sample_view(DiffMode::SideBySide);
        let (surface, _layout, lh) =
            paint_via_backend(&view, QRect::new(0.0, 0.0, W as f32, H as f32));
        let theme = Theme::default();
        let left_w = ((W as f64 - DIVIDER_PX) / 2.0).floor();
        let (r, g, b, _) = surface.pixel(left_w as u32, (lh * 0.5) as u32);
        assert_eq!(
            (r, g, b),
            (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b),
            "the 1pt pane divider should be painted in border_fg",
        );
    }

    /// Non-zero-origin regression guard (LESSONS.md:159-181): the view
    /// must paint at the requested origin, and nothing above/left of it.
    #[test]
    fn paints_at_a_nonzero_origin_only() {
        let origin_x = 40.0_f32;
        let origin_y = 24.0_f32;
        let view = sample_view(DiffMode::SideBySide);
        let (surface, layout, lh) = paint_via_backend(
            &view,
            QRect::new(origin_x, origin_y, W as f32 - origin_x, H as f32 - origin_y),
        );
        let theme = Theme::default();

        assert_eq!(
            layout.visible_rows,
            ((H as f32 - origin_y) as f64 / lh).floor() as usize,
            "visible_rows must be derived from the rect actually handed in",
        );

        // The divider sits at origin_x + floor((w-1)/2), not at W/2.
        let w = (W as f32 - origin_x) as f64;
        let left_w = ((w - DIVIDER_PX) / 2.0).floor();
        let div_x = (origin_x as f64 + left_w) as u32;
        let probe_y = (origin_y as f64 + lh * 0.5) as u32;
        let (r, g, b, _) = surface.pixel(div_x, probe_y);
        assert_eq!(
            (r, g, b),
            (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b),
            "divider must follow the requested origin",
        );

        // Untouched white above and to the left.
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(div_x, origin_y as u32 - 4);
                (r, g, b)
            },
            (255, 255, 255),
            "nothing should paint above the requested origin",
        );
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(4, probe_y);
                (r, g, b)
            },
            (255, 255, 255),
            "nothing should paint left of the requested origin",
        );
    }

    #[test]
    fn unified_mode_counts_hunk_headers_in_total_rows() {
        let view = sample_view(DiffMode::Unified);
        let (_surface, layout, _lh) =
            paint_via_backend(&view, QRect::new(0.0, 0.0, W as f32, H as f32));
        let hunk_count = view.hunks.len();
        assert!(hunk_count > 0, "fixture should produce at least one hunk");
        assert_eq!(
            layout.total_rows,
            view.total_rows() + hunk_count,
            "unified mode reports content rows plus one @@ header per hunk",
        );
    }

    #[test]
    fn header_row_paints_labels_strip() {
        let mut view = sample_view(DiffMode::SideBySide);
        view.left_label = Some("a/main.rs".into());
        view.right_label = Some("b/main.rs".into());
        let (surface, layout, lh) =
            paint_via_backend(&view, QRect::new(0.0, 0.0, W as f32, H as f32));
        let theme = Theme::default();

        // Header strip occupies the first row; probe clear of glyphs.
        let left_w = ((W as f64 - DIVIDER_PX) / 2.0).floor();
        let (r, g, b, _) = surface.pixel(left_w as u32 - 4, (lh * 0.5) as u32);
        assert_eq!(
            (r, g, b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
        );
        // ...and the header steals one row from the content capacity.
        assert_eq!(layout.visible_rows, ((H as f64 - lh) / lh).floor() as usize,);
    }

    #[test]
    fn zero_sized_rect_returns_zero_visible_rows_without_painting() {
        let view = sample_view(DiffMode::SideBySide);
        let (surface, layout, _lh) = paint_via_backend(&view, QRect::new(0.0, 0.0, 0.0, 0.0));
        assert_eq!(layout.visible_rows, 0);
        assert_eq!(layout.total_rows, view.total_rows());
        assert_eq!(
            {
                let (r, g, b, _) = surface.pixel(1, 1);
                (r, g, b)
            },
            (255, 255, 255),
        );
    }
}
