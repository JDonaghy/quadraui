//! macOS (Core Graphics + Core Text) rasteriser for
//! [`crate::primitives::diff_view::DiffView`].
//!
//! Port of [`crate::gtk::diff_view::draw_diff_view`] — same colour
//! mapping, same row-kind semantics, same [`DiffViewLayout`] return
//! value, so scroll clamping behaves identically on both backends. Row/
//! pane geometry and the scroll-clamped visible-line window come from
//! [`DiffView::layout`] — the shared layout API lifted out of three
//! near-identical backend copies (issue #737). This module only converts
//! the resulting DIP-agnostic `f32` geometry to `f64` points and paints;
//! it does not re-derive positions.
//!
//! Before this landed, `MacBackend::draw_diff_view` painted nothing and
//! returned `visible_rows: 0`, which silently pinned every host's scroll
//! clamp to zero (quadraui#484 §4).
//!
//! # Side-by-side layout
//!
//! - Left pane width = `(w - 1) / 2` points (divider is 1 point wide).
//! - Right pane = remaining width.
//! - Per-row background fills cover the full pane width so padding rows
//!   stay visually distinct from content rows.
//!
//! # Unified layout
//!
//! - `@@ ... @@` hunk headers drawn in `theme.accent_fg`.
//! - `-` / `+` / ` ` prefix + full-width background fill per row.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::event::Rect as ERect;
use crate::primitives::diff_view::{
    row_colors, unified_hunk_header, unified_row_style, unified_row_text, DiffLineContent,
    DiffMode, DiffView, DiffViewGeometry, DiffViewLayout,
};
use crate::theme::Theme;
use crate::types::Color;

/// Text inset from a pane's left edge.
const TEXT_PAD: f64 = 4.0;
/// Text inset used in unified mode (tighter, matching the GTK twin).
const UNIFIED_PAD: f64 = 2.0;

/// Convert an `f32` DIP-agnostic rect from [`DiffView::layout`] to point
/// `f64`s.
fn pt_rect(r: ERect) -> (f64, f64, f64, f64) {
    (r.x as f64, r.y as f64, r.width as f64, r.height as f64)
}

/// Draw a [`DiffView`] into the region `(x, y, w, h)` on `ctx`.
///
/// `line_height` is the point height of one text row (supplied by the
/// backend from its current font metrics).
///
/// Returns [`DiffViewLayout`] for scroll clamping.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call (typical: the frame-scope pointer stashed on [`super::MacBackend`]).
/// Calling with a freed or null pointer is UB.
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
    let viewport = ERect::new(x as f32, y as f32, w as f32, h as f32);
    let geometry = view.layout(viewport, line_height as f32);

    if w <= 0.0 || h <= 0.0 || line_height <= 0.0 {
        return geometry.as_layout();
    }

    fill_rect(ctx, x, y, w, h, theme.background);

    match view.mode {
        DiffMode::SideBySide => draw_side_by_side(ctx, font, view, theme, &geometry),
        DiffMode::Unified => draw_unified(ctx, font, view, theme, &geometry),
    }

    geometry.as_layout()
}

// ── Side-by-side ─────────────────────────────────────────────────────────────

unsafe fn draw_side_by_side(
    ctx: CGContextRef,
    font: &CTFont,
    view: &DiffView,
    theme: &Theme,
    geometry: &DiffViewGeometry,
) {
    let flat = view.flat_rows();

    if let Some(header) = &geometry.header {
        let (lx, ly, lw, lh) = pt_rect(header.left);
        let (rx, _ry, rw, _rh) = pt_rect(header.right);
        let (dx, dy, dw, dh) = pt_rect(header.divider);

        fill_rect(ctx, lx, ly, lw + dw + rw, lh, theme.header_bg);
        fill_rect(ctx, dx, dy, dw, dh, theme.border_fg);

        if let Some(label) = &view.left_label {
            clipped_text(
                ctx,
                font,
                label,
                lx + TEXT_PAD,
                ly,
                lx,
                ly,
                lw,
                lh,
                theme.header_fg,
            );
        }
        if let Some(label) = &view.right_label {
            clipped_text(
                ctx,
                font,
                label,
                rx + TEXT_PAD,
                ly,
                rx,
                ly,
                rw,
                lh,
                theme.header_fg,
            );
        }
    }

    for line in &geometry.lines {
        let DiffLineContent::Row { row_idx } = line.content else {
            continue;
        };
        let row = flat[row_idx];
        let (left_fg, left_bg, right_fg, right_bg) = row_colors(row.kind, theme);

        let (lx, ly, lw, lh) = pt_rect(line.left.expect("side-by-side row has left bounds"));
        let (rx, ry, rw, rh) = pt_rect(line.right.expect("side-by-side row has right bounds"));
        let (dx, dy, dw, dh) = pt_rect(line.divider.expect("side-by-side row has divider bounds"));

        fill_rect(ctx, lx, ly, lw, lh, left_bg);
        fill_rect(ctx, rx, ry, rw, rh, right_bg);
        fill_rect(ctx, dx, dy, dw, dh, theme.border_fg);

        if let Some(text) = &row.left {
            clipped_text(ctx, font, text, lx + TEXT_PAD, ly, lx, ly, lw, lh, left_fg);
        }
        if let Some(text) = &row.right {
            clipped_text(ctx, font, text, rx + TEXT_PAD, ry, rx, ry, rw, rh, right_fg);
        }
    }
}

// ── Unified ──────────────────────────────────────────────────────────────────

unsafe fn draw_unified(
    ctx: CGContextRef,
    font: &CTFont,
    view: &DiffView,
    theme: &Theme,
    geometry: &DiffViewGeometry,
) {
    let flat = view.flat_rows();

    for line in &geometry.lines {
        let (rx, ry, rw, rh) = pt_rect(line.bounds);

        match line.content {
            DiffLineContent::UnifiedHeader { hunk_idx } => {
                let header_text = unified_hunk_header(&view.hunks[hunk_idx]);
                fill_rect(ctx, rx, ry, rw, rh, theme.background);
                clipped_text(
                    ctx,
                    font,
                    &header_text,
                    rx + UNIFIED_PAD,
                    ry,
                    rx,
                    ry,
                    rw,
                    rh,
                    theme.accent_fg,
                );
            }
            DiffLineContent::Row { row_idx } => {
                let row = flat[row_idx];
                let (prefix, fg, bg) = unified_row_style(row.kind, theme);

                fill_rect(ctx, rx, ry, rw, rh, bg);

                let prefix_str = prefix.to_string();
                draw_text(
                    ctx,
                    font,
                    &prefix_str,
                    rx + UNIFIED_PAD,
                    ry,
                    color_to_cg(fg),
                );
                let (prefix_w, _) = measure_text(font, &prefix_str);

                let text = unified_row_text(row);
                let text_x = rx + UNIFIED_PAD + prefix_w + TEXT_PAD;
                clipped_text(
                    ctx,
                    font,
                    text,
                    text_x,
                    ry,
                    text_x,
                    ry,
                    (rw - (text_x - rx)).max(0.0),
                    rh,
                    fg,
                );
            }
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Paint `text` at `(tx, ty)` clipped to `(cx, cy, cw, ch)`.
///
/// Core Text has no `EllipsizeMode`; clipping is the macOS equivalent of
/// the GTK twin's `cr.clip()` + `set_ellipsize` pair.
#[allow(clippy::too_many_arguments)]
unsafe fn clipped_text(
    ctx: CGContextRef,
    font: &CTFont,
    text: &str,
    tx: f64,
    ty: f64,
    cx: f64,
    cy: f64,
    cw: f64,
    ch: f64,
    color: Color,
) {
    if cw <= 0.0 || ch <= 0.0 || text.is_empty() {
        return;
    }
    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(cx, cy, cw, ch));
    draw_text(ctx, font, text, tx, ty, color_to_cg(color));
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
    if w <= 0.0 || h <= 0.0 {
        return;
    }
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
    use crate::event::{Rect as QRect, Viewport};
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
