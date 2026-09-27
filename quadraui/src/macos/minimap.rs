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
//! [`MinimapRenderMode::Characters`] is unreachable through
//! [`draw_minimap`]'s own fixed pitch ([`MinimapScale::One`]:
//! `ROW_PITCH_PX` (2 pt) stays below `LEGIBILITY_FLOOR_PX` (4 pt)) — that
//! branch is exercised directly in this module's tests instead, mirroring
//! `gtk::minimap`'s own
//! `characters_branch_truncates_to_the_column_capacity_before_shaping`
//! test. At [`MinimapScale::Two`] (issue #1143) the row pitch (4 pt)
//! clears the floor, and [`draw_minimap_cached`] — what
//! [`super::backend::MacBackend::draw_minimap`] actually calls — paints
//! real glyph *shapes* via a downsampled Core Text atlas (see the "Glyph
//! atlas" section below), not the backend's single editor-size `CTFont`.
//!
//! Colour lookups (both branches) walk `syntax_spans` once per paint via
//! [`SpanCursor`], not once per row — same O(rows + spans) merge-walk
//! `gtk::minimap`/`win::minimap` use (#667 pt. 4).
//!
//! # Glyph atlas (#1153)
//!
//! Mirrors `gtk::minimap`'s #1035 atlas technique: [`MinimapCharAtlas`]/
//! [`MinimapAtlasCache`] are backend-agnostic (they live in
//! [`crate::primitives::minimap`] already, built for GTK's Pango sample
//! sheet), so this rasteriser reuses them unchanged rather than growing a
//! parallel Core Text-flavoured atlas type. [`build_char_atlas`] shapes
//! every sampled ASCII character once, *large* and bold, into one sample
//! sheet ([`render_char_sample_sheet`], via [`super::text::draw_text`]
//! onto an offscreen [`BitmapSurface`]) and hands the alpha bytes to
//! [`MinimapCharAtlas::from_alpha_sheet`], which box-filter-downsamples
//! each cell to the target device-pixel tile — exactly the pipeline
//! [`crate::primitives::minimap`]'s "Character glyph atlas" section docs
//! describe. [`draw_minimap_cached`] then blits each row's tiles
//! ([`paint_row_atlas`]/`blit_alpha_tile`) instead of shaping a run at the
//! target pitch with the backend's single [`CTFont`] — the bug this
//! section fixes (#1153): that full-size shape doesn't fit its row band
//! at all, and stacks illegibly across neighbouring rows.
//!
//! Unlike GTK's `mask_surface` single-call blit, this rasteriser paints
//! each tile pixel-by-pixel via `CGContextFillRect` (see
//! [`blit_alpha_tile`]'s doc) rather than constructing a `CGImage` alpha
//! mask — tile sizes are bounded by [`MinimapScale::cell_w_px`]/
//! `row_pitch_px` times the DPI scale, so this stays cheap without the
//! extra `CGImageMaskCreate` FFI surface.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;
use core_text::font_descriptor::kCTFontBoldTrait;

use super::headless::BitmapSurface;
use super::text::{draw_text, make_font};
use crate::event::Rect;
use crate::primitives::layout_metrics::pixel_minimap_layout_scaled;
use crate::primitives::minimap::{
    color_at_column, minimap_font_px, render_mode, truncate_to_columns, Minimap, MinimapAtlasCache,
    MinimapCharAtlas, MinimapLayout, MinimapRenderMode, MinimapScale, MinimapSpan, SpanCursor,
    VisibleMinimapLine, ATLAS_CHAR_COUNT, ATLAS_FIRST_CHAR, ATLAS_LAST_CHAR, COLUMN_CAPACITY,
};
use crate::theme::Theme;
use crate::types::Color;

/// Compute the macOS point-unit layout for a [`Minimap`] without painting, at
/// [`MinimapScale::One`] — the pre-#1143 fixed pitch. Kept exactly as-is for
/// source compatibility with any caller reaching this free function
/// directly; every real paint path goes through
/// [`MacBackend::minimap_layout`]'s [`mac_minimap_layout_scaled`] instead, so
/// it can honor [`crate::backend::Backend::minimap_scale`] (issue #1143).
///
/// [`MacBackend::minimap_layout`]: crate::macos::MacBackend
pub fn mac_minimap_layout(minimap: &Minimap, rect: Rect) -> MinimapLayout {
    mac_minimap_layout_scaled(minimap, rect, MinimapScale::One)
}

/// [`mac_minimap_layout`], but at an explicit [`MinimapScale`] (issue
/// #1143) — what [`MacBackend::minimap_layout`] actually calls. Shares
/// its `lines_per_row`/sizing formula with `gtk_minimap_layout_scaled` /
/// `win_minimap_layout_scaled` via [`pixel_minimap_layout_scaled`] (issue
/// #1079).
///
/// [`MacBackend::minimap_layout`]: crate::macos::MacBackend
pub(crate) fn mac_minimap_layout_scaled(
    minimap: &Minimap,
    rect: Rect,
    scale: MinimapScale,
) -> MinimapLayout {
    pixel_minimap_layout_scaled(minimap, rect, scale)
}

/// Draw a [`Minimap`] into `rect` (points, target-relative) on `ctx`, at
/// [`MinimapScale::One`] — the pre-#1143 fixed pitch. Kept exactly as-is
/// for source compatibility with any caller reaching this free function
/// directly (mirrors [`crate::gtk::minimap::draw_minimap`]'s own pre-#1035
/// shim doc); every real paint path goes through
/// [`MacBackend::draw_minimap`]'s [`draw_minimap_cached`] instead, so it
/// can honor [`crate::backend::Backend::minimap_scale`] (issue #1143) and
/// paint through the glyph atlas (issue #1153). Returns the resolved
/// [`MinimapLayout`] for host click dispatch (`layout.hit_test(x, y)` ->
/// [`crate::primitives::minimap::MinimapHit`]) — same contract as the
/// GTK/Win-GUI/TUI twins' `draw_minimap`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of this
/// call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
///
/// [`MacBackend::draw_minimap`]: crate::macos::MacBackend
pub unsafe fn draw_minimap(
    ctx: CGContextRef,
    font: &CTFont,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    draw_minimap_scaled(ctx, font, rect, minimap, theme, MinimapScale::One)
}

/// [`draw_minimap`], but at an explicit [`MinimapScale`] (issue #1143).
/// Kept exactly as it always behaved — still shapes the `Characters`
/// branch with the backend's single configured, editor-size [`CTFont`]
/// via [`paint_row_glyphs`] rather than the downsampled atlas — for
/// source compatibility with any caller reaching this free function
/// directly rather than through [`crate::backend::Backend::draw_minimap`],
/// mirroring [`crate::gtk::minimap::draw_minimap`]'s relationship to
/// `draw_minimap_cached` (#1014's split, reused by #1035). At
/// [`MinimapScale::Two`] this is exactly issue #1153's bug: the row pitch
/// clears [`crate::primitives::minimap::LEGIBILITY_FLOOR_PX`], so
/// `render_mode` resolves to [`MinimapRenderMode::Characters`], but the
/// editor-size glyph run doesn't fit the row band and stacks illegibly
/// across neighbouring rows. [`MacBackend::draw_minimap`] — the path
/// every real app takes — calls [`draw_minimap_cached`] instead, which is
/// where the fix (the module doc's "Glyph atlas" section) actually
/// lives.
///
/// # Safety
///
/// Same contract as [`draw_minimap`].
///
/// [`MacBackend::draw_minimap`]: crate::macos::MacBackend
pub(crate) unsafe fn draw_minimap_scaled(
    ctx: CGContextRef,
    font: &CTFont,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
    scale: MinimapScale,
) -> MinimapLayout {
    paint_minimap_rows(
        ctx,
        rect,
        minimap,
        theme,
        scale,
        |ctx, vline, row_px, cell_w, text, row_spans| match render_mode(row_px) {
            MinimapRenderMode::Characters => {
                paint_row_glyphs(ctx, font, vline, text, row_spans, theme)
            }
            MinimapRenderMode::ColumnBlocks => {
                paint_row_blocks(ctx, vline, text, row_spans, theme, cell_w)
            }
        },
    )
}

/// Draw a [`Minimap`] using a cached [`MinimapCharAtlas`] for real
/// character shapes (issue #1153), instead of [`draw_minimap_scaled`]'s
/// editor-size-font shim. `atlas_cache` should be a field the caller
/// keeps alive across frames — [`super::backend::MacBackend`] owns one,
/// the same way [`crate::gtk::backend::GtkBackend`] owns its own
/// (#1035). `dpi_scale` sizes each glyph tile to real device pixels
/// (`self.viewport.scale` on `MacBackend` — Core Graphics' own DPI ratio,
/// same role GTK's `dpi_scale` field plays): a `scale` cell at
/// `dpi_scale` 2.0 gets twice the real device pixels of glyph detail a
/// `dpi_scale` 1.0 cell of the same logical size does. See the module
/// doc's "Glyph atlas" section for the pipeline.
///
/// Never shapes at the target pitch: the atlas build (large, once per
/// `(font family, scale)`) happens inside `atlas_cache.get_or_build` only
/// on a cache miss; every paint after that just blits pre-downsampled
/// alpha tiles. Falls back to [`paint_row_blocks`] only if the atlas
/// itself degrades to [`MinimapCharAtlas::filled`] returning a zero-sized
/// tile (never happens in practice — same defensive, no-op-safe branch
/// [`crate::gtk::minimap::draw_minimap_cached`] takes).
///
/// # Safety
///
/// Same contract as [`draw_minimap`].
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn draw_minimap_cached(
    ctx: CGContextRef,
    font: &CTFont,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
    scale: MinimapScale,
    atlas_cache: &mut MinimapAtlasCache,
    dpi_scale: f64,
) -> MinimapLayout {
    let family = font.family_name();
    let dpi_scale = dpi_scale.max(1.0);
    let tile_w = (scale.cell_w_px() * dpi_scale).round().max(1.0) as usize;
    let tile_h = (scale.row_pitch_px() * dpi_scale).round().max(1.0) as usize;

    // `scale` changes the atlas's tile dimensions just as much as
    // `family`/`dpi_scale` do, so it must be part of the cache key too —
    // folded into the family string rather than growing
    // `MinimapAtlasCache::get_or_build`'s own `(String, f32)` key shape
    // (mirrors `gtk::minimap::draw_minimap_cached`'s identical `family_key`
    // trick).
    let family_key = format!("{family}#{scale:?}");
    let atlas = atlas_cache.get_or_build(&family_key, dpi_scale as f32, || {
        build_char_atlas(&family, tile_w, tile_h)
    });

    paint_minimap_rows(
        ctx,
        rect,
        minimap,
        theme,
        scale,
        |ctx, vline, _row_px, cell_w, text, row_spans| {
            if atlas.tile_w() == 0 || atlas.tile_h() == 0 {
                paint_row_blocks(ctx, vline, text, row_spans, theme, cell_w);
            } else {
                paint_row_atlas(ctx, atlas, dpi_scale, vline, text, row_spans, theme, cell_w);
            }
        },
    )
}

/// Shared skeleton for [`draw_minimap_scaled`]/[`draw_minimap_cached`]:
/// computes the layout, paints the background and viewport highlight,
/// clips to the strip, then calls `paint_row` once per visible row via
/// one [`SpanCursor`] merge-walk (#667 pt. 4). Returns early (no clip, no
/// callback invocations) when the layout has nothing to paint — mirrors
/// `gtk::minimap::paint_minimap_rows`.
///
/// `paint_row` gets each row's own pitch (`vline.bounds.height`, post-cap
/// — see the old `draw_minimap_scaled` comment on why this beats
/// recomputing `h / visible_lines.len()`), even though every row shares
/// one pitch under [`crate::primitives::minimap::MinimapSizing::FixedPitch`],
/// plus `scale`'s own [`MinimapScale::cell_w_px`] (issue #1143), so a
/// row's per-column paint walk steps by the right cell width without
/// re-deriving it from `scale` itself.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of this
/// call — same contract as [`draw_minimap`].
unsafe fn paint_minimap_rows(
    ctx: CGContextRef,
    rect: Rect,
    minimap: &Minimap,
    theme: &Theme,
    scale: MinimapScale,
    mut paint_row: impl FnMut(CGContextRef, &VisibleMinimapLine, f64, f64, &str, &[MinimapSpan]),
) -> MinimapLayout {
    let layout = mac_minimap_layout_scaled(minimap, rect, scale);

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
        paint_row(
            ctx,
            vline,
            vline.bounds.height as f64,
            scale.cell_w_px(),
            &line.text,
            row_spans,
        );
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

/// `ColumnBlocks` branch: paint one `cell_w`-wide block per non-blank
/// character column of `text`, each coloured by whichever span covers it —
/// VS Code's `renderCharacters: false` look, which (unlike a single
/// per-line bar) preserves the line's indent and internal-gap silhouette
/// (#667 pt. 2). Stops after [`COLUMN_CAPACITY`] columns, so a
/// pathologically long line costs no more to paint than a short one.
/// `cell_w` is [`MinimapScale::cell_w_px`] (issue #1143) — `1.0` at the
/// pre-#1143 default [`MinimapScale::One`], matching this function's
/// original hardcoded block width byte for byte.
fn paint_row_blocks(
    ctx: CGContextRef,
    vline: &VisibleMinimapLine,
    text: &str,
    row_spans: &[MinimapSpan],
    theme: &Theme,
    cell_w: f64,
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
                vline.bounds.x as f64 + col as f64 * cell_w,
                vline.bounds.y as f64,
                cell_w,
                vline.bounds.height as f64,
                color,
            );
        }
    }
}

/// Advance width (pt) of one cell in [`render_char_sample_sheet`]'s
/// sample sheet — see [`ATLAS_CHAR_COUNT`] for the number of cells the
/// sheet holds. Matches `gtk::minimap::CHAR_SAMPLE_CELL_W` byte for byte
/// (VS Code's own `createSampleData` advance).
const CHAR_SAMPLE_CELL_W: f64 = 10.0;
/// Height (pt) of one cell in [`render_char_sample_sheet`]'s sample sheet
/// (and of the sheet itself). Matches `gtk::minimap::CHAR_SAMPLE_CELL_H`.
const CHAR_SAMPLE_CELL_H: f64 = 16.0;

/// Build a [`MinimapCharAtlas`] for `family` at `tile_w x tile_h`,
/// falling back to [`MinimapCharAtlas::filled`] (a safe, visually-inert
/// degraded result — see that constructor's docs) if the sample-sheet
/// render itself fails (e.g. surface allocation, or the family not
/// resolving to a real font).
fn build_char_atlas(family: &str, tile_w: usize, tile_h: usize) -> MinimapCharAtlas {
    match render_char_sample_sheet(family) {
        Some(alpha) => MinimapCharAtlas::from_alpha_sheet(
            &alpha,
            (CHAR_SAMPLE_CELL_W as usize) * ATLAS_CHAR_COUNT,
            CHAR_SAMPLE_CELL_H as usize,
            CHAR_SAMPLE_CELL_W as usize,
            tile_w,
            tile_h,
        ),
        None => MinimapCharAtlas::filled(tile_w, tile_h),
    }
}

/// Shape every sampled ASCII character (see [`ATLAS_FIRST_CHAR`]/
/// [`ATLAS_LAST_CHAR`]) once, at `CHAR_SAMPLE_CELL_H` pt bold, into one
/// sample sheet — mirrors VS Code's `MinimapCharRenderer.createSampleData`
/// (96 cells, 10pt advance, 16pt tall, bold 16pt font) and
/// `gtk::minimap::render_char_sample_sheet`'s identical technique.
/// Returns a tightly-packed, row-major *coverage* buffer (`sheet_w *
/// CHAR_SAMPLE_CELL_H` bytes, `0` transparent .. `255` opaque).
///
/// Paints white text onto [`BitmapSurface`]'s zero-initialised
/// (transparent) background, premultiplied — so for a fully-white,
/// fully-opaque source pixel the R/G/B channels equal the alpha channel
/// directly, and for a partially-covered (anti-aliased) pixel they equal
/// the coverage fraction times 255 either way — reading the R channel
/// back is exactly the coverage byte [`MinimapCharAtlas::from_alpha_sheet`]
/// wants, no separate alpha-channel readback or premultiplied-colour
/// division needed (same shortcut `gtk::minimap`'s own sample-sheet
/// reader takes from Cairo's native byte order, just off a different
/// channel).
fn render_char_sample_sheet(family: &str) -> Option<Vec<u8>> {
    let cell_w = CHAR_SAMPLE_CELL_W as u32;
    let sheet_h = CHAR_SAMPLE_CELL_H as u32;
    let sheet_w = cell_w * ATLAS_CHAR_COUNT as u32;

    let base_font = make_font(family, CHAR_SAMPLE_CELL_H * 0.85)?;
    // Bold, matching VS Code's/GTK's sample-sheet weight — a downsampled
    // regular-weight glyph loses too much ink to read back as a shape at
    // all. Not every family has a bold face Core Text can synthesize;
    // fall back to the regular weight rather than failing the whole
    // atlas build.
    let font = base_font
        .clone_with_symbolic_traits(kCTFontBoldTrait, kCTFontBoldTrait)
        .unwrap_or(base_font);

    let surface = BitmapSurface::new(sheet_w, sheet_h);

    // Suppress `draw_text`'s own built-in recording
    // (`macos::text::record_if_active`) for the duration of this
    // sample-sheet build. This paints into a private, throwaway
    // offscreen surface, never the real widget -- but `record_if_active`
    // reads from a **thread-local** sink (`crate::testing::text_run_sink_active`),
    // not one scoped to this `CGContextRef`, so without this, a
    // `MacDriver`-based test that happens to take an atlas cache miss
    // during a recorded frame would get these `ATLAS_CHAR_COUNT` sample
    // glyphs' bounds appended to `MacBackend::painted_text`, polluting
    // `MacDriver::find`/`screen_contains` with meaningless bounds.
    // `install_text_run_sink` swaps in a fresh scratch sink (recording
    // still nominally "active" so `record_if_active` still measures and
    // pushes into it) and `take_text_run_sink` below discards that scratch
    // vec and restores whatever sink (`None` or a live outer recording)
    // was active before -- same hazard `gtk::minimap::render_char_sample_sheet`
    // documents hitting for its own sample sheet (quadraui#1035 review),
    // just worked around differently since macOS's `draw_text` has no
    // separate non-recording twin to call instead.
    let prev_sink = crate::testing::install_text_run_sink();

    // ASCII code points 0x20..=0x7E always fit `u8` and are always valid
    // `char`s, so this cast is infallible -- deliberately avoiding a
    // `char::from_u32(code)?` early return here, which would skip
    // restoring `prev_sink` below.
    for (i, code) in (ATLAS_FIRST_CHAR..=ATLAS_LAST_CHAR).enumerate() {
        let ch = code as u8 as char;
        // SAFETY: `surface.context_ptr()` is a valid, live `CGContextRef`
        // for the surface's lifetime, which outlives this whole loop.
        unsafe {
            draw_text(
                surface.context_ptr(),
                &font,
                &ch.to_string(),
                i as f64 * CHAR_SAMPLE_CELL_W,
                0.0,
                (1.0, 1.0, 1.0, 1.0),
            );
        }
    }

    let _ = crate::testing::take_text_run_sink(prev_sink);

    let mut alpha = vec![0u8; (sheet_w * sheet_h) as usize];
    for y in 0..sheet_h {
        for x in 0..sheet_w {
            let (r, _g, _b, _a) = surface.pixel(x, y);
            alpha[(y * sheet_w + x) as usize] = r;
        }
    }
    Some(alpha)
}

/// Paint one row using `atlas`'s pre-downsampled alpha tiles: blit a tile
/// per non-blank character, tinted by whichever span covers it — no
/// shaping, matching [`paint_row_blocks`]'s own column-capacity bound
/// (#667 pt. 3) and whitespace skip, but painting real glyph shapes
/// instead of solid blocks. `cell_w` (issue #1143's
/// [`MinimapScale::cell_w_px`]) is the logical column advance — `col as
/// f64 * cell_w`, not a bare `col as f64` — so a wider
/// [`MinimapScale::Two`] cell doesn't overlap its neighbour.
#[allow(clippy::too_many_arguments)]
fn paint_row_atlas(
    ctx: CGContextRef,
    atlas: &MinimapCharAtlas,
    dpi_scale: f64,
    vline: &VisibleMinimapLine,
    text: &str,
    row_spans: &[MinimapSpan],
    theme: &Theme,
    cell_w: f64,
) {
    for (col, ch) in text.chars().enumerate().take(COLUMN_CAPACITY) {
        if ch.is_whitespace() {
            continue;
        }
        let color = color_at_column(row_spans, col, theme.foreground);
        // SAFETY: `ctx` is the same valid, live `CGContextRef` `draw_minimap`
        // was called with, still in scope for the duration of this call.
        unsafe {
            blit_alpha_tile(
                ctx,
                atlas.tile(ch),
                atlas.tile_w(),
                atlas.tile_h(),
                vline.bounds.x as f64 + col as f64 * cell_w,
                vline.bounds.y as f64,
                dpi_scale,
                color,
            );
        }
    }
}

/// Blit one `tile_w x tile_h` alpha tile (device pixels) at logical
/// position `(x, y)`, tinted by `color`. Unlike `gtk::minimap`'s
/// `blit_alpha_tile` (a single Cairo `mask_surface` call over a small
/// `ImageSurface`), this paints the tile pixel-by-pixel via
/// `CGContextFillRect` at `1 / dpi_scale`-pt device-pixel granularity —
/// avoiding the extra `CGImageMaskCreate`/`CGDataProviderCreateWithData`
/// FFI surface `core-graphics` doesn't expose safely, at the cost of up
/// to `tile_w * tile_h` fill calls per character. Tile sizes are bounded
/// by [`MinimapScale::cell_w_px`]/`row_pitch_px` times `dpi_scale` (a few
/// pixels wide/tall even at HiDPI), so this stays cheap.
///
/// # Safety
///
/// `ctx` must be a valid, live `CGContextRef` for the duration of this
/// call.
#[allow(clippy::too_many_arguments)]
unsafe fn blit_alpha_tile(
    ctx: CGContextRef,
    tile: &[u8],
    tile_w: usize,
    tile_h: usize,
    x: f64,
    y: f64,
    dpi_scale: f64,
    color: Color,
) {
    if tile_w == 0 || tile_h == 0 {
        return;
    }
    let (r, g, b, _) = color_to_cg(color);
    let px = 1.0 / dpi_scale.max(f64::MIN_POSITIVE);
    for row in 0..tile_h {
        for col in 0..tile_w {
            let coverage = tile[row * tile_w + col];
            if coverage == 0 {
                continue;
            }
            CGContextSetRGBFillColor(ctx, r, g, b, coverage as f64 / 255.0);
            CGContextFillRect(
                ctx,
                CGRect::new_xywh(x + col as f64 * px, y + row as f64 * px, px, px),
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
    use super::*;
    use crate::primitives::minimap::{MinimapHit, MinimapLine, ROW_PITCH_PX};
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

    /// #1143: `MinimapScale::Two`'s row pitch (4pt) clears
    /// `LEGIBILITY_FLOOR_PX`, so unlike the `MinimapScale::One` default
    /// (which never reaches it) `draw_minimap_scaled` must actually take
    /// the `Characters` branch and paint real Core Text glyphs at this
    /// scale, with a row pitch matching `MinimapScale::Two::row_pitch_px`.
    #[test]
    fn scale_two_reaches_the_characters_branch_with_a_taller_row_pitch() {
        let surface = BitmapSurface::new(W as u32, H as u32);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let font = test_font();
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };
        let mm = minimap_from(vec!["fn main() {}"; 4], 4);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = unsafe {
            draw_minimap_scaled(
                surface.context_ptr(),
                &font,
                rect,
                &mm,
                &theme,
                MinimapScale::Two,
            )
        };

        assert_eq!(
            layout.visible_lines[0].bounds.height,
            MinimapScale::Two.row_pitch_px() as f32
        );

        let mut painted_any = false;
        for x in 0..W as u32 {
            for y in 0..(MinimapScale::Two.row_pitch_px() as u32 * 4).max(8) {
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(
            painted_any,
            "expected draw_minimap_scaled at MinimapScale::Two to paint visible text"
        );
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

    // ── glyph atlas (#1153) ──────────────────────────────────────────────

    /// Regression guard for #1153: at [`MinimapScale::Two`], the pre-fix
    /// `Characters` branch (`paint_row_glyphs`, still what
    /// [`draw_minimap_scaled`] uses) shapes with the backend's single
    /// full editor-size [`CTFont`] — nowhere near small enough to fit a
    /// 4pt row band, so it stacks illegibly across neighbouring rows.
    /// [`draw_minimap_cached`] — what `MacBackend::draw_minimap` actually
    /// calls — must instead keep every row's ink inside its own row
    /// band. Row 1 here is blank, so *any* ink in its band can only be
    /// row 0's glyph bleeding past its own row.
    #[test]
    fn atlas_mode_keeps_each_row_within_its_own_band() {
        let code_line = "fn main()";
        let mm = minimap_from(vec![code_line, ""], 2);
        let scale = MinimapScale::Two;
        let cell_w = scale.cell_w_px() as u32;
        let row_h = scale.row_pitch_px() as u32;
        let w = (code_line.chars().count() as u32) * cell_w;
        let h = row_h * 2;

        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };

        let surface = BitmapSurface::new(w, h);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let font = test_font();
        let mut cache = MinimapAtlasCache::new();
        let rect = Rect::new(0.0, 0.0, w as f32, h as f32);

        let layout = unsafe {
            draw_minimap_cached(
                surface.context_ptr(),
                &font,
                rect,
                &mm,
                &theme,
                scale,
                &mut cache,
                1.0,
            )
        };
        assert_eq!(
            layout.visible_lines.len(),
            2,
            "expected exactly 2 visible rows"
        );

        // Row 1 (blank) must stay fully background across its whole band
        // -- any ink there is row 0's glyph tiles bleeding past their own
        // row, exactly the #1153 bug.
        for y in row_h..h {
            for x in 0..w {
                let (r, g, b, _) = surface.pixel(x, y);
                assert_eq!(
                    (r, g, b),
                    (255, 255, 255),
                    "row 1 (blank) must stay fully background at ({x}, {y}) -- \
                     any ink here means row 0 bled past its own row band"
                );
            }
        }

        // Sanity: row 0 must have actually painted *something*, or the
        // containment check above would be vacuous.
        let mut painted_row0 = false;
        for y in 0..row_h {
            for x in 0..w {
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) != (255, 255, 255) {
                    painted_row0 = true;
                }
            }
        }
        assert!(painted_row0, "expected row 0 to paint something");
    }

    /// #1143's own acceptance bar, now proven for the fixed atlas path:
    /// at [`MinimapScale::Two`], a row of real code paints *shapes* --
    /// non-uniform pixel values across a character's own cell, not a
    /// solid, uniformly-coloured block -- mirroring
    /// `gtk::minimap::scale_two_atlas_mode_paints_a_shape_and_leaves_blank_columns_empty`.
    #[test]
    fn atlas_mode_paints_a_shape_not_a_solid_block() {
        let line = "fn main()";
        let mm = minimap_from(vec![line], 1);
        let scale = MinimapScale::Two;
        let cell_w = scale.cell_w_px() as u32;
        let row_h = scale.row_pitch_px() as u32;
        let w = line.chars().count() as u32 * cell_w;

        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(0, 0, 0),
            ..Theme::default()
        };

        let surface = BitmapSurface::new(w, row_h);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let font = test_font();
        let mut cache = MinimapAtlasCache::new();
        let rect = Rect::new(0.0, 0.0, w as f32, row_h as f32);

        unsafe {
            draw_minimap_cached(
                surface.context_ptr(),
                &font,
                rect,
                &mm,
                &theme,
                scale,
                &mut cache,
                1.0,
            );
        }

        // The first character's own cell ('f', column 0) must show a
        // shape -- at least two pixels inside the cell with different
        // colours -- rather than a solid, uniformly-filled block.
        let mut cell_pixels = Vec::new();
        for dx in 0..cell_w {
            for y in 0..row_h {
                cell_pixels.push(surface.pixel(dx, y));
            }
        }
        let distinct: std::collections::HashSet<_> = cell_pixels.iter().collect();
        assert!(
            distinct.len() > 1,
            "expected the 'f' cell to show a shape (multiple distinct pixel values), got {cell_pixels:?}"
        );
    }

    /// Regression guard for the #1035 review finding, ported to macOS
    /// (#1153): the sample sheet [`render_char_sample_sheet`] shapes each
    /// ASCII glyph into is a private, throwaway surface (never real
    /// screen content), but `crate::testing`'s text-run sink is a
    /// **thread-local**, not scoped to any particular `CGContextRef` --
    /// so if this function ever stopped pausing/discarding recording
    /// around its `draw_text` calls, every atlas cache miss during a
    /// `MacDriver`-based test (guaranteed on a fresh backend's first
    /// paint) would silently append all `ATLAS_CHAR_COUNT` sample
    /// glyphs' bogus bounds to the shared sink that backs
    /// `MacDriver::find`/`screen_contains`.
    #[test]
    fn render_char_sample_sheet_does_not_pollute_the_text_run_sink() {
        let previous = crate::testing::install_text_run_sink();
        let _ = render_char_sample_sheet("Menlo");
        let recorded = crate::testing::take_text_run_sink(previous);
        assert!(
            recorded.is_empty(),
            "render_char_sample_sheet must never record into the text-run sink, got {recorded:?}"
        );
    }
}
