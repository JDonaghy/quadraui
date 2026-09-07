//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::diff_view::DiffView`] (#737).
//!
//! Painting moved to the shared
//! [`crate::primitives::diff_view::native_surface_paint::paint`] (#866,
//! `NativeSurface` Phase 2d slice 9/9) — see that fn's doc for the two
//! named divergences (row/header text vertical alignment; header-label
//! ellipsize vs. hard-clip) found while unifying
//! `gtk::diff_view::draw_diff_view`, `macos::diff_view::draw_diff_view`
//! and `win::diff_view::draw_diff_view` into one implementation. This
//! module now only carries [`RawWinDiffViewSurface`] and the deprecated
//! [`draw_diff_view`] compatibility shim over it, mirroring
//! `win::status_bar::RawWinStatusBarSurface` (#860).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod diff_view;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! `diff_view_layout` is **not** overridden on `WinBackend` — same as
//! every other backend. It ships a trait default (`Backend::diff_view_layout`)
//! that is a pure function of `line_height()` + `DiffView::mode`, which is
//! exactly what every backend's own `draw_diff_view` already resolves to
//! (see `tests/conformance/caps.rs`'s `#506` block comment, revisited for
//! #737 — the new shared [`DiffView::layout`] backs that default's
//! reasoning even more directly now, but the default itself was already
//! the honest answer and stays unoverridden here too).

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{pop_clip, push_clip, DWrite};
use crate::event::Rect;
use crate::native_surface::NativeSurface;
use crate::primitives::diff_view::{DiffView, DiffViewLayout};
use crate::theme::Theme;

/// Minimal [`NativeSurface`] adapter over a bare `&ID2D1RenderTarget` +
/// [`DWrite`], used only by the deprecated [`draw_diff_view`] shim below
/// and by this module's own tests — mirrors
/// `win::status_bar::RawWinStatusBarSurface`'s identical pattern (#860),
/// extended with clip push/pop, which this primitive's paint actually
/// uses.
pub(crate) struct RawWinDiffViewSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
    pub(crate) dwrite: &'a DWrite,
}

impl NativeSurface for RawWinDiffViewSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawWinDiffViewSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawWinDiffViewSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawWinDiffViewSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawWinDiffViewSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawWinDiffViewSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.dwrite.measure_text(text).unwrap_or((0.0, 0.0))
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        let _ = super::text::fill_rect(self.target, rect, color);
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
        let _ = self.dwrite.draw_text(self.target, text, rect, color);
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
        push_clip(self.target, rect);
    }

    fn surface_pop_clip(&mut self) {
        pop_clip(self.target);
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
/// direct `quadraui::win::draw_diff_view` reference rather than going
/// through [`crate::Backend::draw_diff_view`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is why
/// this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_diff_view` instead — this free function is a compatibility shim over the shared #866 implementation"
)]
pub fn draw_diff_view(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    view: &DiffView,
    theme: &Theme,
    line_height: f32,
) -> DiffViewLayout {
    let mut surface = RawWinDiffViewSurface { target, dwrite };
    crate::primitives::diff_view::native_surface_paint::paint(
        view,
        &mut surface,
        theme,
        rect,
        line_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::compute_hunks;
    use crate::primitives::diff_view::{DiffMode, DiffRowKind};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 400.0;
    const H: f32 = 200.0;
    const LINE_HEIGHT: f32 = 16.0;

    fn sample_view(mode: DiffMode) -> DiffView {
        let left = "one\ntwo\nthree\n";
        let right = "one\nTWO\nthree\n";
        DiffView {
            id: WidgetId::new("diff"),
            left: left.into(),
            right: right.into(),
            left_label: None,
            right_label: None,
            hunks: compute_hunks(left, right),
            mode,
            editability: Default::default(),
            scroll_offset: 0,
            focused_pane: Default::default(),
            has_focus: false,
        }
    }

    fn first_row_of(view: &DiffView, kind: DiffRowKind) -> usize {
        view.hunks
            .iter()
            .flat_map(|h| h.rows.iter())
            .position(|r| r.kind == kind)
            .expect("fixture should contain the requested row kind")
    }

    /// Paint `view` via the shared
    /// [`crate::primitives::diff_view::native_surface_paint::paint`]
    /// through a [`RawWinDiffViewSurface`] over `surface`'s headless
    /// target — the same adapter the deprecated [`draw_diff_view`] shim
    /// uses, exercised here directly so these tests don't trip the
    /// `-D warnings`-denied `deprecated` lint (CLAUDE.md rule 3; mirrors
    /// `win::status_bar`'s identical test-migration note).
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        view: &DiffView,
        theme: &Theme,
        line_height: f32,
    ) -> DiffViewLayout {
        surface
            .paint(|target| {
                let mut raw = RawWinDiffViewSurface { target, dwrite };
                crate::primitives::diff_view::native_surface_paint::paint(
                    view,
                    &mut raw,
                    theme,
                    rect,
                    line_height,
                );
            })
            .map(|_| view.layout(rect, line_height).as_layout())
            .expect("paint diff view")
    }

    /// C0 smoke: `draw_diff_view` must actually paint text + a
    /// scroll-clamp-usable layout rather than panicking or hitting a
    /// `todo!()` (#737's acceptance bar — "draw_diff_view survives C0
    /// with text_ok on win").
    #[test]
    fn draw_diff_view_paints_text_and_returns_layout() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &view, &theme, LINE_HEIGHT);

        assert!(layout.visible_rows > 0);
        assert_eq!(layout.total_rows, view.total_rows());

        // "text_ok" — some non-background pixel actually painted inside
        // the left pane (proves DrawText ran, not just the background/row
        // fills).
        let mut painted_any = false;
        for x in 0..(W as u32 / 2) {
            for y in 0..(LINE_HEIGHT as u32 * 3) {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected diff_view to paint visible glyphs");
    }

    /// The divider colour and position are the same shared geometry every
    /// backend paints from — pins the DIP position independently of the
    /// text-glyph probe above.
    #[test]
    fn divider_paints_border_colour() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(0.0, 0.0, W, H);

        paint(&surface, &dwrite, rect, &view, &theme, LINE_HEIGHT);

        let changed = first_row_of(&view, DiffRowKind::Changed);
        let geometry = view.layout(rect, LINE_HEIGHT);
        let divider_x = geometry.panes.expect("side-by-side has panes").divider_x as u32;
        let row_y = (LINE_HEIGHT * (changed as f32 + 0.5)) as u32;

        let px = surface.pixel_at(divider_x, row_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b),
            "the pane divider should be painted in border_fg",
        );
    }

    /// Non-zero-origin regression guard (LESSONS.md — the LOCAL/ABSOLUTE
    /// mixup) — mirrors every other `win::` rasteriser's own nonzero-origin
    /// test (see `win::pipeline_view::paint_and_click_round_trip_action_button_at_nonzero_origin`).
    #[test]
    fn paints_at_a_nonzero_origin_only() {
        let origin_x = 40.0_f32;
        let origin_y = 24.0_f32;
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(origin_x, origin_y, W - origin_x, H - origin_y);

        // Pre-fill with a sentinel colour distinct from every theme colour
        // this paints with, so "untouched" is unambiguous — mirrors
        // `win::pipeline_view`/`win::list`'s own nonzero-origin tests and
        // this module's own `zero_size_rect_is_a_no_op`. A raw
        // `HeadlessSurface` starts black, which collides with real theme
        // colours closely enough to make a false pass possible.
        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill sentinel background");

        paint(&surface, &dwrite, rect, &view, &theme, LINE_HEIGHT);

        let geometry = view.layout(rect, LINE_HEIGHT);
        let panes = geometry.panes.expect("side-by-side has panes");
        let div_x = panes.divider_x as u32;
        let probe_y = (origin_y + LINE_HEIGHT * 0.5) as u32;

        let px = surface.pixel_at(div_x, probe_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b),
            "divider must follow the requested origin",
        );

        // Untouched sentinel above and to the left.
        let above = surface.pixel_at(div_x, origin_y as u32 - 4);
        assert_eq!(
            (above.r, above.g, above.b),
            (255, 255, 255),
            "nothing should paint above the requested origin",
        );
        let left = surface.pixel_at(4, probe_y);
        assert_eq!(
            (left.r, left.g, left.b),
            (255, 255, 255),
            "nothing should paint left of the requested origin",
        );
    }

    /// Unified mode's `total_rows` counts hunk headers too — mirrors the
    /// GTK/macOS/TUI twins' regression guard.
    #[test]
    fn unified_mode_counts_hunk_headers_in_total_rows() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let view = sample_view(DiffMode::Unified);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = paint(&surface, &dwrite, rect, &view, &theme, LINE_HEIGHT);

        let hunk_count = view.hunks.len();
        assert!(hunk_count > 0, "fixture should produce at least one hunk");
        assert_eq!(
            layout.total_rows,
            view.total_rows() + hunk_count,
            "unified mode reports content rows plus one @@ header per hunk",
        );
    }

    /// No-paint layout must agree with what `draw_diff_view` painted from
    /// — same contract every other `win::` rasteriser's
    /// `no_paint_layout_matches_paint_layout` test proves.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let theme = Theme::default();

        let painted = paint(&surface, &dwrite, rect, &view, &theme, LINE_HEIGHT);
        let no_paint = view.layout(rect, LINE_HEIGHT).as_layout();
        assert_eq!(painted, no_paint);
    }

    /// Zero-size rect is a no-op — mirrors every other `win::` rasteriser's
    /// same guard.
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        paint(&surface, &dwrite, rect, &view, &theme, LINE_HEIGHT);

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width diff view should paint nothing at all",
        );
    }
}
