//! macOS (Core Graphics + Core Text) rasteriser for
//! [`crate::primitives::diff_view::DiffView`].
//!
//! Port of [`crate::gtk::diff_view::draw_diff_view`] — same colour
//! mapping, same row-kind semantics, same [`DiffViewLayout`] return
//! value, so scroll clamping behaves identically on both backends.
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
use crate::primitives::diff_view::{DiffMode, DiffRowKind, DiffView, DiffViewLayout};
use crate::theme::Theme;
use crate::types::Color;

/// Divider width between the two panes, in points.
const DIVIDER_PX: f64 = 1.0;
/// Text inset from a pane's left edge.
const TEXT_PAD: f64 = 4.0;
/// Text inset used in unified mode (tighter, matching the GTK twin).
const UNIFIED_PAD: f64 = 2.0;

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
    if w <= 0.0 || h <= 0.0 || line_height <= 0.0 {
        return DiffViewLayout {
            visible_rows: 0,
            total_rows: view.total_rows(),
        };
    }

    let total_rows = view.total_rows();

    match view.mode {
        DiffMode::SideBySide => {
            draw_side_by_side(ctx, font, x, y, w, h, view, theme, line_height, total_rows)
        }
        DiffMode::Unified => draw_unified(ctx, font, x, y, w, h, view, theme, line_height),
    }
}

// ── Side-by-side ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
unsafe fn draw_side_by_side(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    view: &DiffView,
    theme: &Theme,
    line_height: f64,
    total_rows: usize,
) -> DiffViewLayout {
    let has_header = view.left_label.is_some() || view.right_label.is_some();
    let header_h = if has_header { line_height } else { 0.0 };

    let left_w = ((w - DIVIDER_PX) / 2.0).floor();
    let right_w = (w - DIVIDER_PX - left_w).max(0.0);
    let divider_x = x + left_w;

    // Total background.
    fill_rect(ctx, x, y, w, h, theme.background);

    if has_header {
        fill_rect(ctx, x, y, w, line_height, theme.header_bg);
        fill_rect(ctx, divider_x, y, DIVIDER_PX, line_height, theme.border_fg);

        if let Some(label) = &view.left_label {
            clipped_text(
                ctx,
                font,
                label,
                x + TEXT_PAD,
                y,
                x,
                y,
                left_w,
                line_height,
                theme.header_fg,
            );
        }
        if let Some(label) = &view.right_label {
            clipped_text(
                ctx,
                font,
                label,
                divider_x + DIVIDER_PX + TEXT_PAD,
                y,
                divider_x + DIVIDER_PX,
                y,
                right_w,
                line_height,
                theme.header_fg,
            );
        }
    }

    let content_y_start = y + header_h;
    let content_h = (h - header_h).max(0.0);
    let visible_rows = (content_h / line_height).floor() as usize;

    let all_rows: Vec<_> = view.hunks.iter().flat_map(|hk| hk.rows.iter()).collect();
    let start = view.scroll_offset.min(total_rows.saturating_sub(1));
    let end = (start + visible_rows).min(total_rows);

    for (row_idx, row) in all_rows
        .iter()
        .enumerate()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let row_y = content_y_start + (row_idx - start) as f64 * line_height;

        let (left_fg, left_bg, right_fg, right_bg) = row_colors(row.kind, theme);

        fill_rect(ctx, x, row_y, left_w, line_height, left_bg);
        fill_rect(
            ctx,
            divider_x + DIVIDER_PX,
            row_y,
            right_w,
            line_height,
            right_bg,
        );
        fill_rect(
            ctx,
            divider_x,
            row_y,
            DIVIDER_PX,
            line_height,
            theme.border_fg,
        );

        if let Some(text) = &row.left {
            clipped_text(
                ctx,
                font,
                text,
                x + TEXT_PAD,
                row_y,
                x,
                row_y,
                left_w,
                line_height,
                left_fg,
            );
        }
        if let Some(text) = &row.right {
            clipped_text(
                ctx,
                font,
                text,
                divider_x + DIVIDER_PX + TEXT_PAD,
                row_y,
                divider_x + DIVIDER_PX,
                row_y,
                right_w,
                line_height,
                right_fg,
            );
        }
    }

    DiffViewLayout {
        visible_rows,
        total_rows,
    }
}

// ── Unified ──────────────────────────────────────────────────────────────────

/// One rendered line in unified mode: either a synthesised `@@` header or
/// a real diff row.
enum UnifiedLine<'a> {
    Header(String),
    Content(&'a crate::primitives::diff_view::DiffRow),
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_unified(
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
    fill_rect(ctx, x, y, w, h, theme.background);

    let mut lines: Vec<UnifiedLine<'_>> = Vec::new();
    for hunk in &view.hunks {
        // `-n` counts lines sourced from the LEFT file, `+m` from the
        // RIGHT — these differ from `hunk.rows.len()` whenever a change
        // run produces padding rows. Identical to the GTK twin.
        let left_count = hunk.rows.iter().filter(|r| r.left.is_some()).count();
        let right_count = hunk.rows.iter().filter(|r| r.right.is_some()).count();
        lines.push(UnifiedLine::Header(format!(
            "@@ -{},{} +{},{} @@",
            hunk.left_start, left_count, hunk.right_start, right_count
        )));
        for row in &hunk.rows {
            lines.push(UnifiedLine::Content(row));
        }
    }

    let total_display = lines.len();
    let visible_rows = (h / line_height).floor() as usize;
    let start = view.scroll_offset.min(total_display.saturating_sub(1));
    let end = (start + visible_rows).min(total_display);

    for (i, line) in lines
        .iter()
        .enumerate()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let row_y = y + (i - start) as f64 * line_height;

        match line {
            UnifiedLine::Header(header_text) => {
                fill_rect(ctx, x, row_y, w, line_height, theme.background);
                clipped_text(
                    ctx,
                    font,
                    header_text,
                    x + UNIFIED_PAD,
                    row_y,
                    x,
                    row_y,
                    w,
                    line_height,
                    theme.accent_fg,
                );
            }
            UnifiedLine::Content(row) => {
                let (prefix, fg, bg) = match row.kind {
                    DiffRowKind::Same => (' ', theme.muted_fg, theme.background),
                    DiffRowKind::Removed | DiffRowKind::Changed => {
                        ('-', theme.git_deleted, theme.diff_removed_bg)
                    }
                    DiffRowKind::Added => ('+', theme.git_added, theme.diff_added_bg),
                };

                fill_rect(ctx, x, row_y, w, line_height, bg);

                let prefix_str = prefix.to_string();
                draw_text(
                    ctx,
                    font,
                    &prefix_str,
                    x + UNIFIED_PAD,
                    row_y,
                    color_to_cg(fg),
                );
                let (prefix_w, _) = measure_text(font, &prefix_str);

                let text = match row.kind {
                    DiffRowKind::Removed => row.left.as_deref().unwrap_or(""),
                    _ => row.right.as_deref().or(row.left.as_deref()).unwrap_or(""),
                };
                let text_x = x + UNIFIED_PAD + prefix_w + TEXT_PAD;
                clipped_text(
                    ctx,
                    font,
                    text,
                    text_x,
                    row_y,
                    text_x,
                    row_y,
                    (w - (text_x - x)).max(0.0),
                    line_height,
                    fg,
                );
            }
        }
    }

    // Return `total_display` (content rows + one `@@` header per hunk) so
    // callers clamp `scroll_offset` correctly in unified mode.
    DiffViewLayout {
        visible_rows,
        total_rows: total_display,
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

/// `(left_fg, left_bg, right_fg, right_bg)` for a row kind. Mirrors the
/// GTK twin's `row_colors_gtk` exactly.
fn row_colors(kind: DiffRowKind, theme: &Theme) -> (Color, Color, Color, Color) {
    match kind {
        DiffRowKind::Same => (
            theme.muted_fg,
            theme.background,
            theme.muted_fg,
            theme.background,
        ),
        DiffRowKind::Changed => (
            theme.git_deleted,
            theme.diff_removed_bg,
            theme.git_added,
            theme.diff_added_bg,
        ),
        DiffRowKind::Removed => (
            theme.git_deleted,
            theme.diff_removed_bg,
            theme.muted_fg,
            theme.diff_padding_bg,
        ),
        DiffRowKind::Added => (
            theme.muted_fg,
            theme.diff_padding_bg,
            theme.git_added,
            theme.diff_added_bg,
        ),
    }
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
    fn first_row_of(view: &DiffView, kind: DiffRowKind) -> usize {
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
        let changed = first_row_of(&view, DiffRowKind::Changed);
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
