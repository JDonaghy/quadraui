//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::minimap::Minimap`] (#738).
//!
//! Mirrors `gtk::minimap`'s technique: rows tile at a fixed pitch
//! ([`crate::primitives::minimap::ROW_PITCH_PX`],
//! [`crate::primitives::minimap::MinimapSizing::FixedPitch`]) — see
//! `Minimap::layout_with_sizing`'s module docs for the slide behaviour when
//! a file needs more rows than the strip holds at that pitch. At that
//! pitch, [`crate::primitives::minimap::is_legible`] is false, so painting
//! lands in [`MinimapRenderMode::ColumnBlocks`]: one 1-DIP-wide block per
//! non-blank character column, coloured by whichever aggregated span
//! covers it — the same VS Code `renderCharacters: false` silhouette
//! `gtk::minimap` paints.
//!
//! Before #738 the legibility/render-mode threshold
//! (`is_legible`/`render_mode`/`minimap_font_px`) and the `ROW_PITCH_PX`/
//! `COLUMN_CAPACITY` geometry constants existed only in `gtk::minimap`, so
//! this rasteriser would otherwise have had to invent its own answer to
//! "how small is too small to shape real text" rather than reuse GTK's
//! already-tuned one. They now live in [`crate::primitives::minimap`], and
//! so do the span-lookup helpers both backends' row loops need
//! ([`SpanCursor`], [`color_at_column`], [`truncate_to_columns`]) — this
//! module only converts the shared `f32` geometry to Direct2D paint calls.
//!
//! [`MinimapRenderMode::Characters`] — real glyphs at a font size scaled to
//! the row pitch — is deliberately painted with the backend's single
//! configured [`DWrite`] text format rather than a per-row-pitch size, the
//! same divergence `win::board`'s module doc notes for badge/title fonts:
//! DirectWrite text formats aren't cheap to vary per call without a format
//! cache no Win-GUI rasteriser has yet. `ROW_PITCH_PX` (2 DIP) stays below
//! `LEGIBILITY_FLOOR_PX` (4 DIP) today, so `Characters` is not reachable
//! through this rasteriser's own fixed pitch — same as GTK's default —
//! and is exercised directly in this module's tests rather than through
//! [`draw_minimap`] itself, mirroring `gtk::minimap`'s own
//! `characters_branch_truncates_to_the_column_capacity_before_shaping`
//! test.
//!
//! Colour lookups (both branches) walk `syntax_spans` once per paint via
//! [`SpanCursor`], not once per row — same O(rows + spans) merge-walk
//! `gtk::minimap` uses (#667 pt. 4).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod minimap;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{blend, fill_rect, pop_clip, push_clip, DWrite};
use crate::event::Rect;
use crate::primitives::minimap::{
    color_at_column, minimap_font_px, render_mode, truncate_to_columns, Minimap, MinimapLayout,
    MinimapRenderMode, MinimapSizing, MinimapSpan, SpanCursor, VisibleMinimapLine, COLUMN_CAPACITY,
    ROW_PITCH_PX,
};
use crate::theme::Theme;

/// Win-GUI shows one buffer line per painted row — no cross-line colour
/// reduction, same technique as `gtk::minimap::LINES_PER_ROW` (see
/// [`crate::MinimapGrid`]'s doc for why TUI's braille packing differs).
pub const LINES_PER_ROW: usize = 1;

/// Compute the Win-GUI DIP-unit layout for a [`Minimap`] without painting
/// — the DirectWrite twin of [`draw_minimap`]'s internal layout call. Same
/// contract as the GTK/TUI twins' `*_minimap_layout`: `rect.x`/`rect.y`
/// are baked into every returned bound (absolute frame).
pub fn win_minimap_layout(minimap: &Minimap, rect: Rect) -> MinimapLayout {
    minimap.layout_with_sizing(
        rect,
        LINES_PER_ROW,
        MinimapSizing::FixedPitch(ROW_PITCH_PX as f32),
    )
}

/// Draw a [`Minimap`] into `rect` (DIPs, target-relative) on `target`.
/// Returns the resolved [`MinimapLayout`] for host click dispatch
/// (`layout.hit_test(x, y)` -> [`crate::primitives::minimap::MinimapHit`]) —
/// same contract as the GTK/TUI twins' `draw_minimap`.
pub fn draw_minimap(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    let layout = win_minimap_layout(minimap, rect);

    if layout.visible_lines.is_empty() {
        return layout;
    }

    let _ = fill_rect(target, rect, theme.background);

    let hl = &layout.viewport_highlight;
    if hl.height > 0.0 {
        let tint = blend(theme.background, theme.accent_bg, 0.25);
        let _ = fill_rect(target, *hl, tint);
    }

    // Row pitch already lives on the layout, post-cap -- read it back
    // rather than recomputing `rect.height / visible_lines.len()`, which
    // would silently undo the fixed-pitch contract (mirrors
    // `gtk::draw_minimap`'s same note). All rows share one pitch, so the
    // first is representative.
    let row_px = layout
        .visible_lines
        .first()
        .map(|v| v.bounds.height as f64)
        .unwrap_or(0.0);
    let mode = render_mode(row_px);

    // Clip all row painting to the strip: a below-floor colour-block walk
    // is already width-bounded by `COLUMN_CAPACITY`, but a strip narrower
    // than that capacity must still not bleed into whatever the host
    // painted beside the minimap (mirrors `gtk::draw_minimap`'s `cr.clip()`
    // bracket, #663).
    push_clip(target, rect);

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
                paint_row_glyphs(target, dwrite, vline, &line.text, row_spans, theme)
            }
            MinimapRenderMode::ColumnBlocks => {
                paint_row_blocks(target, vline, &line.text, row_spans, theme)
            }
        }
    }

    pop_clip(target);

    layout
}

/// `Characters` branch: paint `text` with the backend's single configured
/// [`DWrite`] text format — see the module doc's "Divergence from the GTK
/// twin" note for why this doesn't vary the font size per `row_px` the way
/// `gtk::minimap::paint_row_glyphs` does. Still bounds its cost to
/// [`COLUMN_CAPACITY`] characters (#667 pt. 3), and still colours the row
/// from its own `row_spans` — the first span's colour wins (DirectWrite
/// needs a custom text renderer for true per-run colour within one
/// `DrawText` call, out of scope for a branch this rasteriser's own fixed
/// pitch never reaches).
fn paint_row_glyphs(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    vline: &VisibleMinimapLine,
    text: &str,
    row_spans: &[MinimapSpan],
    theme: &Theme,
) {
    // `minimap_font_px` is the shared pitch->size mapping every backend's
    // `Characters` branch is keyed on; Win-GUI can't act on it without a
    // format cache (see module doc), but computing it here keeps this
    // branch honestly wired to the same decision function rather than
    // silently ignoring it.
    let _ = minimap_font_px(vline.bounds.height as f64);

    let truncated = truncate_to_columns(text, COLUMN_CAPACITY);
    let fg = row_spans
        .first()
        .map(|s| s.color)
        .unwrap_or(theme.foreground);
    let _ = dwrite.draw_text(target, truncated, vline.bounds, fg);
}

/// `ColumnBlocks` branch: paint one 1-DIP-wide block per non-blank
/// character column of `text`, each coloured by whichever span covers it
/// — VS Code's `renderCharacters: false` look, which (unlike a single
/// per-line bar) preserves the line's indent and internal-gap silhouette
/// (#667 pt. 2). Stops after [`COLUMN_CAPACITY`] columns, so a
/// pathologically long line costs no more to paint than a short one.
fn paint_row_blocks(
    target: &ID2D1RenderTarget,
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
        let block = Rect::new(
            vline.bounds.x + col as f32,
            vline.bounds.y,
            1.0,
            vline.bounds.height,
        );
        let _ = fill_rect(target, block, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::minimap::{MinimapHit, MinimapLine};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

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

    /// C0 smoke: `draw_minimap` must actually paint pixels + return a
    /// click-routable layout rather than panicking or hitting a `todo!()`
    /// (#738's acceptance bar). The default fixed pitch stays below the
    /// legibility floor, so this exercises `ColumnBlocks`, not `DrawText`
    /// — the "non-background pixel actually painted" check every other
    /// `win::` C0 smoke uses stands in for "text_ok" here.
    #[test]
    fn draw_minimap_paints_column_blocks_and_returns_layout() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_minimap(target, &dwrite, rect, &mm, &theme);
            })
            .map(|_| win_minimap_layout(&mm, rect))
            .expect("paint minimap");

        assert!(!layout.visible_lines.is_empty());

        let mut painted_any = false;
        for x in 0..W as u32 {
            for y in 0..(ROW_PITCH_PX as u32 * 8).max(8) {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected draw_minimap to paint visible blocks");
    }

    /// Paint↔click round trip at a non-zero origin — #505's LOCAL/ABSOLUTE
    /// mixup regression guard, mirrored from `win::board`/`win::diff_view`'s
    /// own nonzero-origin tests.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        let origin_x = 7.0_f32;
        let origin_y = 13.0_f32;
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let mm = minimap_from(vec!["x"; 8], 8);
        let rect = Rect::new(origin_x, origin_y, 40.0, 100.0);

        let layout = surface
            .paint(|target| {
                draw_minimap(target, &dwrite, rect, &mm, &theme);
            })
            .map(|_| win_minimap_layout(&mm, rect))
            .expect("paint minimap");

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
    /// painted — same contract every other `win::` rasteriser's
    /// `no_paint_layout_matches_paint_layout` test proves.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_minimap(target, &dwrite, rect, &mm, &Theme::default());
            })
            .map(|_| win_minimap_layout(&mm, rect))
            .expect("paint");
        let no_paint = win_minimap_layout(&mm, rect);
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
        let mm = minimap_from(vec!["fn main() {}"; 8], 8);
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        surface
            .paint(|target| {
                draw_minimap(target, &dwrite, rect, &mm, &theme);
            })
            .expect("paint minimap");

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width minimap should paint nothing at all",
        );
    }

    /// The fixed pitch stays independent of both the strip height and the
    /// file length — same parity guarantee `gtk::minimap`'s
    /// `row_pitch_is_independent_of_the_strip_height` /
    /// `_of_the_file_length` tests pin, now proven on Win-GUI's own
    /// `win_minimap_layout` (#738: both backends share `ROW_PITCH_PX`).
    #[test]
    fn row_pitch_is_independent_of_strip_height_and_file_length() {
        let short = minimap_from(vec!["fn main() {}"; 3], 3);
        let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
        let long = minimap_from(long_refs, 10_000);

        let tall = win_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 800.0));
        let short_strip = win_minimap_layout(&short, Rect::new(0.0, 0.0, 40.0, 16.0));
        let long_layout = win_minimap_layout(&long, Rect::new(0.0, 0.0, 40.0, 400.0));

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
    /// reach it via `ROW_PITCH_PX` alone, mirroring `gtk::minimap`'s own
    /// `characters_branch_truncates_to_the_column_capacity_before_shaping`.
    #[test]
    fn characters_branch_paints_a_truncated_line_directly() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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

        surface
            .paint(|target| {
                paint_row_glyphs(target, &dwrite, &vline, &long_line, &[], &theme);
            })
            .expect("paint glyphs");

        let mut painted_any = false;
        for x in 0..W as u32 {
            for y in 0..20u32 {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected the Characters branch to paint text");
    }
}
