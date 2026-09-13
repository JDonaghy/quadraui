//! Core Graphics / Core Text rasteriser for [`crate::primitives::minimap::Minimap`]
//! (#961).
//!
//! Mirrors `gtk::minimap`/`win::minimap`'s technique: rows tile at a fixed
//! pitch ([`crate::primitives::minimap::ROW_PITCH_PX`],
//! [`crate::primitives::minimap::MinimapSizing::FixedPitch`]) — see
//! `Minimap::layout_with_sizing`'s module docs for the slide behaviour when
//! a file needs more rows than the strip holds at that pitch. At that
//! pitch, [`crate::primitives::minimap::is_legible`] is false, so painting
//! lands in [`MinimapRenderMode::ColumnBlocks`]: one 1pt-wide block per
//! non-blank character column, coloured by whichever aggregated span
//! covers it — the same VS Code `renderCharacters: false` silhouette
//! `gtk::minimap`/`win::minimap` paint.
//!
//! #382 scoped macOS's Core Graphics/Core Text paint calls out of scope,
//! and #738 lifted the shared legibility/render-mode thresholds and
//! geometry constants into [`crate::primitives::minimap`] specifically so
//! this rasteriser (like `win::minimap` before it) can consume them
//! without re-deriving anything. Only the actual paint calls — fill
//! rects, `CTLine` glyph runs via [`super::text::draw_text`], and the clip
//! bracket — are backend-specific here (#961).
//!
//! [`MinimapRenderMode::Characters`] — real glyphs — is painted with the
//! backend's single configured [`CTFont`] rather than a per-row-pitch
//! size, the same divergence `win::minimap`'s module doc notes for its
//! own `DWrite` text format: a per-pitch `CTFont` needs a
//! `clone_with_font_size` call (and cache) this rasteriser doesn't bother
//! with because `ROW_PITCH_PX` (2 pt) stays below `LEGIBILITY_FLOOR_PX`
//! (4 pt) today, so `Characters` is not reachable through this
//! rasteriser's own fixed pitch — same as GTK's and Win-GUI's defaults —
//! and is exercised directly in this module's tests rather than through
//! [`draw_minimap`] itself, mirroring `gtk::minimap`'s own
//! `characters_branch_truncates_to_the_column_capacity_before_shaping`
//! test.
//!
//! Colour lookups (both branches) walk `syntax_spans` once per paint via
//! [`SpanCursor`], not once per row — same O(rows + spans) merge-walk
//! `gtk::minimap`/`win::minimap` use (#667 pt. 4).

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::draw_text;
use crate::event::Rect;
use crate::primitives::minimap::{
    color_at_column, minimap_font_px, render_mode, truncate_to_columns, Minimap, MinimapLayout,
    MinimapRenderMode, MinimapSizing, MinimapSpan, SpanCursor, VisibleMinimapLine, COLUMN_CAPACITY,
    ROW_PITCH_PX,
};
use crate::theme::Theme;
use crate::types::Color;

/// Same grouping factor as GTK/Win-GUI — see `gtk::minimap::LINES_PER_ROW`'s
/// doc. macOS shows one buffer line per painted row, no cross-line colour
/// reduction (see [`crate::MinimapGrid`]'s doc for why TUI's braille
/// packing differs).
pub const LINES_PER_ROW: usize = 1;

/// Compute the macOS point-unit layout for a [`Minimap`] without painting —
/// the same [`Minimap::layout_with_sizing`] call GTK/Win-GUI make.
pub fn mac_minimap_layout(minimap: &Minimap, rect: Rect) -> MinimapLayout {
    minimap.layout_with_sizing(
        rect,
        LINES_PER_ROW,
        MinimapSizing::FixedPitch(ROW_PITCH_PX as f32),
    )
}

/// Draw a [`Minimap`] into `rect` (points, target-relative) on `ctx`.
/// Returns the resolved [`MinimapLayout`] for host click dispatch
/// (`layout.hit_test(x, y)` -> [`crate::primitives::minimap::MinimapHit`]) —
/// same contract as the GTK/Win-GUI/TUI twins' `draw_minimap`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of this
/// call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
pub unsafe fn draw_minimap(
    ctx: CGContextRef,
    font: &CTFont,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    let layout = mac_minimap_layout(minimap, rect);

    if rect.width <= 0.0 || rect.height <= 0.0 || layout.visible_lines.is_empty() {
        return layout;
    }

    fill_rect(
        ctx,
        rect.x as f64,
        rect.y as f64,
        rect.width as f64,
        rect.height as f64,
        theme.background,
    );

    let hl = &layout.viewport_highlight;
    if hl.height > 0.0 {
        // Same translucent-overlay technique as
        // `editor::draw_visual_selection`'s `fill_rect_alpha`: paint
        // `theme.accent_bg` at partial alpha over the already-painted
        // background rather than pre-blending the two colours, letting
        // Core Graphics' normal (source-over) compositing do the mixing —
        // mirrors the `blend(theme.background, theme.accent_bg, 0.25)`
        // call `gtk::minimap`/`win::minimap` make.
        fill_rect_alpha(
            ctx,
            hl.x as f64,
            hl.y as f64,
            hl.width as f64,
            hl.height as f64,
            theme.accent_bg,
            0.25,
        );
    }

    // Row pitch already lives on the layout, post-cap -- read it back
    // rather than recomputing `rect.height / visible_lines.len()`, which
    // would silently undo the fixed-pitch contract (mirrors
    // `gtk::draw_minimap`'s/`win::draw_minimap`'s same note). All rows
    // share one pitch, so the first is representative.
    let row_px = layout
        .visible_lines
        .first()
        .map(|v| v.bounds.height as f64)
        .unwrap_or(0.0);
    let mode = render_mode(row_px);

    // Clip all row painting to the strip: a below-floor colour-block walk
    // is already width-bounded by `COLUMN_CAPACITY`, but a strip narrower
    // than that capacity must still not bleed into whatever the host
    // painted beside the minimap (mirrors `gtk::draw_minimap`'s
    // `cr.clip()`/`win::draw_minimap`'s clip bracket, #663).
    CGContextSaveGState(ctx);
    CGContextClipToRect(
        ctx,
        CGRect::new_xywh(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        ),
    );

    // One merge-walk over `syntax_spans` for the whole paint, not one
    // linear rescan per row (#667 pt. 4) -- `visible_lines` is always
    // ascending in `start_line_idx`, matching `SpanCursor`'s own
    // non-decreasing-query requirement.
    let mut spans = SpanCursor::new(&minimap.syntax_spans);

    for vline in &layout.visible_lines {
        let Some(line) = minimap.lines.get(vline.start_line_idx) else {
            continue;
        };
        let row_spans = spans.row_spans(vline.start_line_idx);
        match mode {
            MinimapRenderMode::Characters => {
                paint_row_glyphs(ctx, font, vline, &line.text, row_spans, theme)
            }
            MinimapRenderMode::ColumnBlocks => {
                paint_row_blocks(ctx, vline, &line.text, row_spans, theme)
            }
        }
    }

    CGContextRestoreGState(ctx);

    layout
}

/// `Characters` branch: paint `text` with the backend's single configured
/// [`CTFont`] — see the module doc's divergence note for why this doesn't
/// vary the font size per `row_px` the way `gtk::minimap::paint_row_glyphs`
/// does. Still bounds its cost to [`COLUMN_CAPACITY`] characters (#667 pt.
/// 3), and still colours the row from its own `row_spans` — the first
/// span's colour wins (Core Text needs a per-run colour attribute for true
/// mixed-colour text within one `CTLine`, out of scope for a branch this
/// rasteriser's own fixed pitch never reaches).
fn paint_row_glyphs(
    ctx: CGContextRef,
    font: &CTFont,
    vline: &VisibleMinimapLine,
    text: &str,
    row_spans: &[MinimapSpan],
    theme: &Theme,
) {
    // `minimap_font_px` is the shared pitch->size mapping every backend's
    // `Characters` branch is keyed on; this rasteriser can't act on it
    // without a font-size cache (see module doc), but computing it here
    // keeps this branch honestly wired to the same decision function
    // rather than silently ignoring it.
    let _ = minimap_font_px(vline.bounds.height as f64);

    let truncated = truncate_to_columns(text, COLUMN_CAPACITY);
    let fg = row_spans
        .first()
        .map(|s| s.color)
        .unwrap_or(theme.foreground);
    // SAFETY: `ctx` is the same valid, live `CGContextRef` `draw_minimap`
    // was called with, still in scope for the duration of this call.
    unsafe {
        draw_text(
            ctx,
            font,
            truncated,
            vline.bounds.x as f64,
            vline.bounds.y as f64,
            color_to_cg(fg),
        );
    }
}

/// `ColumnBlocks` branch: paint one 1pt-wide block per non-blank character
/// column of `text`, each coloured by whichever span covers it — VS
/// Code's `renderCharacters: false` look, which (unlike a single per-line
/// bar) preserves the line's indent and internal-gap silhouette (#667 pt.
/// 2). Stops after [`COLUMN_CAPACITY`] columns, so a pathologically long
/// line costs no more to paint than a short one.
fn paint_row_blocks(
    ctx: CGContextRef,
    vline: &VisibleMinimapLine,
    text: &str,
    row_spans: &[MinimapSpan],
    theme: &Theme,
) {
    for (col, ch) in text.chars().enumerate().take(COLUMN_CAPACITY) {
        if ch.is_whitespace() {
            continue;
        }
        let color = color_at_column(row_spans, col, theme.foreground);
        // SAFETY: `ctx` is the same valid, live `CGContextRef` `draw_minimap`
        // was called with, still in scope for the duration of this call.
        unsafe {
            fill_rect(
                ctx,
                vline.bounds.x as f64 + col as f64,
                vline.bounds.y as f64,
                1.0,
                vline.bounds.height as f64,
                color,
            );
        }
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

/// # Safety
///
/// `ctx` must be a valid, live `CGContextRef` for the duration of this call.
unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

/// Like [`fill_rect`], but the alpha channel is `alpha` instead of the
/// colour's own — mirrors `editor::fill_rect_alpha`'s same technique for
/// the viewport-highlight overlay.
///
/// # Safety
///
/// `ctx` must be a valid, live `CGContextRef` for the duration of this call.
unsafe fn fill_rect_alpha(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color, alpha: f64) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (r, g, b, _) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, alpha);
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
    use super::*;
    use crate::primitives::minimap::{MinimapHit, MinimapLine};
    use crate::types::WidgetId;

    const W: f32 = 200.0;
    const H: f32 = 200.0;

    fn minimap_from(lines: Vec<&str>, total_buffer_lines: usize) -> Minimap {
        Minimap {
            id: WidgetId::new("mm"),
            lines: lines
                .into_iter()
                .enumerate()
                .map(|(i, t)| MinimapLine {
                    text: t.into(),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: 0,
            total_buffer_lines,
        }
    }

    fn test_font() -> CTFont {
        make_font("Menlo", 10.0).expect("Menlo should be installed on every macOS host")
    }

    /// C0 smoke: `draw_minimap` must actually paint pixels + return a
    /// click-routable layout rather than doing nothing (#961's acceptance
    /// bar). The default fixed pitch stays below the legibility floor, so
    /// this exercises `ColumnBlocks`, not glyph shaping — the
    /// "non-background pixel actually painted" check every other `macos::`
    /// C0 smoke uses stands in for "text_ok" here.
    #[test]
    fn draw_minimap_paints_column_blocks_and_returns_layout() {
        let surface = BitmapSurface::new(W as u32, H as u32);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let font = test_font();
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = unsafe { draw_minimap(surface.context_ptr(), &font, rect, &mm, &theme) };

        assert!(!layout.visible_lines.is_empty());

        let mut painted_any = false;
        for x in 0..W as u32 {
            for y in 0..(ROW_PITCH_PX as u32 * 8).max(8) {
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected draw_minimap to paint visible blocks");
    }

    /// Paint↔click round trip at a non-zero origin — #505's LOCAL/ABSOLUTE
    /// mixup regression guard, mirrored from `macos::board`/`win::minimap`'s
    /// own nonzero-origin tests.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        let origin_x = 7.0_f32;
        let origin_y = 13.0_f32;
        let surface = BitmapSurface::new(W as u32, H as u32);
        let font = test_font();
        let theme = Theme::default();
        let mm = minimap_from(vec!["x"; 8], 8);
        let rect = Rect::new(origin_x, origin_y, 40.0, 100.0);

        let layout = unsafe { draw_minimap(surface.context_ptr(), &font, rect, &mm, &theme) };

        assert_eq!(
            layout.hit_test(origin_x + 20.0, origin_y + 50.0),
            MinimapHit::Seek { fraction: 0.5 }
        );
        assert_eq!(
            layout.hit_test(origin_x + 20.0, origin_y),
            MinimapHit::Seek { fraction: 0.0 }
        );
    }

    /// No-paint layout must agree byte-for-byte with what `draw_minimap`
    /// painted — same contract every other backend's
    /// `no_paint_layout_matches_paint_layout` test proves.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, W, H);
        let font = test_font();
        let surface = BitmapSurface::new(W as u32, H as u32);

        let painted =
            unsafe { draw_minimap(surface.context_ptr(), &font, rect, &mm, &Theme::default()) };
        let no_paint = mac_minimap_layout(&mm, rect);
        assert_eq!(painted, no_paint);
    }

    /// Zero-size rect is a no-op — mirrors every other backend's same
    /// guard.
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let surface = BitmapSurface::new(W as u32, H as u32);
        let font = test_font();
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface.fill(1.0, 1.0, 1.0, 1.0);

        unsafe {
            draw_minimap(surface.context_ptr(), &font, rect, &mm, &theme);
        }

        let (r, g, b, _) = surface.pixel(1, 1);
        assert_eq!(
            (r, g, b),
            (255, 255, 255),
            "a zero-width minimap should paint nothing at all",
        );
    }

    /// The fixed pitch stays independent of both the strip height and the
    /// file length — same parity guarantee `gtk::minimap`'s/`win::minimap`'s
    /// `row_pitch_is_independent_of_the_strip_height` /
    /// `_of_the_file_length` tests pin, now proven on macOS's own
    /// `mac_minimap_layout`.
    #[test]
    fn row_pitch_is_independent_of_strip_height_and_file_length() {
        let short = minimap_from(vec!["fn main() {}"; 3], 3);
        let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
        let long = minimap_from(long_refs, 10_000);

        let tall = mac_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 800.0));
        let short_strip = mac_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 16.0));
        let long_layout = mac_minimap_layout(&long, Rect::new(0.0, 0.0, 40.0, 400.0));

        assert_eq!(tall.visible_lines[0].bounds.height as f64, ROW_PITCH_PX);
        assert_eq!(
            short_strip.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
        assert_eq!(
            long_layout.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
    }

    /// The `Characters` branch is unreachable through the default fixed
    /// pitch today (it stays below `LEGIBILITY_FLOOR_PX`), but it must
    /// still bound its cost to `COLUMN_CAPACITY` characters and still
    /// paint something — exercised directly since `draw_minimap` can't
    /// reach it via `ROW_PITCH_PX` alone, mirroring `gtk::minimap`'s /
    /// `win::minimap`'s own equivalent test.
    #[test]
    fn characters_branch_paints_a_truncated_line_directly() {
        let surface = BitmapSurface::new(W as u32, H as u32);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let font = test_font();
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let long_line = "y".repeat(10_000);
        let vline = VisibleMinimapLine {
            start_line_idx: 0,
            bounds: Rect::new(0.0, 0.0, W, 20.0),
        };

        paint_row_glyphs(
            surface.context_ptr(),
            &font,
            &vline,
            &long_line,
            &[],
            &theme,
        );

        let mut painted_any = false;
        for x in 0..W as u32 {
            for y in 0..20u32 {
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected the Characters branch to paint text");
    }
}
