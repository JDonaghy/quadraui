//! GTK rasteriser for [`crate::Minimap`]: fixed row pitch, per-column
//! colour blocks (#667; supersedes the file-length-dependent font scaling
//! from #382).
//!
//! Rows tile at a fixed [`ROW_PITCH_PX`], independent of the file's
//! length ([`crate::MinimapSizing::FixedPitch`]) — see
//! `Minimap::layout`'s module docs for the slide behaviour when a file
//! needs more rows than the strip holds at that pitch. At that pitch,
//! [`is_legible`] is false, so painting lands in
//! [`MinimapRenderMode::ColumnBlocks`]: one 1px-wide block per non-blank
//! character column, coloured by whichever aggregated span covers it,
//! rather than one bar per line — this is what preserves the indent
//! silhouette VS Code's `renderCharacters: false` mode reads as (#667).
//! [`MinimapRenderMode::Characters`] — real glyphs painted through Pango
//! at a scaled-down **absolute** size (`FontDescription::set_absolute_size`)
//! — is kept for pitches at or above
//! [`crate::primitives::minimap::LEGIBILITY_FLOOR_PX`] (not reached
//! by [`ROW_PITCH_PX`] today, but still a real, tested code path: nothing
//! stops a future caller from requesting a taller fixed pitch). The mode
//! switch ([`render_mode`]) is a pure function of the row pitch so it's
//! deterministic and directly testable without a live surface.
//!
//! Both branches bound their per-row cost to [`COLUMN_CAPACITY`] columns:
//! `ColumnBlocks`' walk stops there, and `Characters` truncates the text
//! it hands to Pango there too, so a pathologically long line costs no
//! more to paint or shape than a short one (#667 pt. 3).
//!
//! Colour lookups (both branches) walk `syntax_spans` once per paint via
//! [`SpanCursor`], not once per row — `aggregate_spans` documents its
//! output as sorted by `(line_idx, start_col)`, and `visible_lines` is
//! itself always ascending in `start_line_idx`, so a single merge-walk
//! is O(rows + spans) total rather than the old O(rows * spans) rescan
//! (#667 pt. 4).
//!
//! #738: the legibility/render-mode thresholds (`is_legible`,
//! `render_mode`, `minimap_font_px`), the shared `ROW_PITCH_PX` /
//! `COLUMN_CAPACITY` geometry constants, and the span-lookup helpers
//! (`SpanCursor`, `color_at_column`, `truncate_to_columns`) used to be
//! defined in this module. They now live in
//! [`crate::primitives::minimap`] so `win::minimap` (and, eventually, a
//! macOS rasteriser) consume the exact same decision logic instead of
//! growing an independent interpretation of either threshold — this
//! module only imports them.
//!
//! # Glyph atlas (#1035)
//!
//! `render_mode`/`LEGIBILITY_FLOOR_PX` say a *shaped* font at
//! `ROW_PITCH_PX` is unreadable — true, and still what [`draw_minimap`]
//! (this module's public, uncached entry point, kept for source
//! compatibility) does: it stays on `render_mode`'s pitch gate, so it
//! still always lands in [`MinimapRenderMode::ColumnBlocks`], exactly as
//! before #1035. [`draw_minimap_cached`] — what [`crate::gtk::backend::GtkBackend::draw_minimap`]
//! actually calls — takes a different path: it never shapes at the
//! target pitch at all. It builds a [`MinimapCharAtlas`] once per
//! `(font family, scale)` (cached in a [`MinimapAtlasCache`] owned by
//! `GtkBackend`, matching #1014's `image_cache` pattern) by shaping each
//! ASCII character once, *large*, via [`render_char_sample_sheet`], then
//! box-filter-downsampling every cell to a device-pixel tile — the
//! technique [`crate::primitives::minimap`]'s "Character glyph atlas"
//! section docs describe. Painting a row then blits pre-downsampled
//! alpha tiles ([`paint_row_atlas`]/`blit_alpha_tile`) instead of asking
//! Pango to shape a 2px font — no per-frame shaping, and legible glyph
//! *shapes* at a pitch the shaper alone could never produce.

use gtk4::cairo::{Context, Format, ImageSurface};
use gtk4::pango;

use super::{cairo_rgb, set_source};
use crate::event::Rect as QRect;
use crate::primitives::minimap::{
    color_at_column, minimap_font_px, render_mode, truncate_to_columns, Minimap, MinimapAtlasCache,
    MinimapCharAtlas, MinimapLayout, MinimapRenderMode, MinimapSizing, MinimapSpan, SpanCursor,
    VisibleMinimapLine, ATLAS_FIRST_CHAR, ATLAS_LAST_CHAR, COLUMN_CAPACITY, ROW_PITCH_PX,
};
use crate::theme::Theme;

/// GTK shows one buffer line per painted row — no cross-line colour
/// reduction (see [`crate::MinimapGrid`]'s doc for why TUI differs).
pub const LINES_PER_ROW: usize = 1;

/// Compute the GTK pixel-unit layout for a [`Minimap`] without painting.
pub fn gtk_minimap_layout(minimap: &Minimap, x: f64, y: f64, w: f64, h: f64) -> MinimapLayout {
    minimap.layout_with_sizing(
        QRect::new(x as f32, y as f32, w as f32, h as f32),
        LINES_PER_ROW,
        MinimapSizing::FixedPitch(ROW_PITCH_PX as f32),
    )
}

/// Shared skeleton for [`draw_minimap`]/[`draw_minimap_cached`]: computes
/// the layout, paints the background and viewport highlight, clips to
/// the strip, then calls `paint_row` once per visible row via one
/// [`SpanCursor`] merge-walk (#667 pt. 4). Returns early (no clip, no
/// callback invocations) when the layout has nothing to paint.
///
/// `paint_row` gets each row's own pitch (`vline.bounds.height`, post-cap
/// — see [`draw_minimap`]'s old comment on why this beats recomputing `h
/// / visible_lines.len()`), even though every row shares one pitch under
/// [`MinimapSizing::FixedPitch`], so a caller never has to special-case
/// "read it off the first row instead".
#[allow(clippy::too_many_arguments)]
fn paint_minimap_rows(
    cr: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    minimap: &Minimap,
    theme: &Theme,
    mut paint_row: impl FnMut(&Context, &VisibleMinimapLine, f64, &str, &[MinimapSpan]),
) -> MinimapLayout {
    let layout = gtk_minimap_layout(minimap, x, y, w, h);

    if layout.visible_lines.is_empty() {
        return layout;
    }

    set_source(cr, theme.background);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    let hl = &layout.viewport_highlight;
    if hl.height > 0.0 {
        let (r, g, b) = cairo_rgb(theme.accent_bg);
        cr.set_source_rgba(r, g, b, 0.25);
        cr.rectangle(hl.x as f64, hl.y as f64, hl.width as f64, hl.height as f64);
        cr.fill().ok();
    }

    // Clip all row painting to the strip: a legible pitch can still shape
    // a line longer than `w`, and below-floor colour blocks are already
    // width-bounded but glyphs are not -- without this, text bled across
    // whatever the host painted beside the minimap (#663).
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();

    // One merge-walk over `syntax_spans` for the whole paint, not one
    // linear rescan per row (#667 pt. 4) -- `visible_lines` is always
    // ascending in `start_line_idx` (see `Minimap::layout`), matching
    // `SpanCursor`'s own non-decreasing-query requirement.
    let mut spans = SpanCursor::new(&minimap.syntax_spans);

    for vline in &layout.visible_lines {
        let Some(line) = minimap.lines.get(vline.start_line_idx) else {
            continue;
        };
        let row_spans = spans.row_spans(vline.start_line_idx);
        paint_row(cr, vline, vline.bounds.height as f64, &line.text, row_spans);
    }

    cr.restore().ok();

    layout
}

/// Draw a [`Minimap`] onto `cr`. Returns the layout for host click
/// dispatch (`layout.hit_test(x, y)` -> [`crate::MinimapHit`]).
///
/// This is the pre-#1035 shim, kept exactly as it always behaved (still
/// gated on [`render_mode`]'s legibility floor, so it still always lands
/// in [`MinimapRenderMode::ColumnBlocks`] at the default `ROW_PITCH_PX`)
/// — for source compatibility with any caller reaching this free
/// function directly rather than through [`crate::backend::Backend::draw_minimap`],
/// mirroring `gtk::image::draw_image`'s relationship to
/// `draw_image_cached` (#1014). [`crate::gtk::backend::GtkBackend::draw_minimap`]
/// — the path every real app takes — calls [`draw_minimap_cached`]
/// instead, which is where #1035's glyph atlas actually lives. See the
/// module docs' "Glyph atlas" section.
#[allow(clippy::too_many_arguments)]
pub fn draw_minimap(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    let saved_font = pango_layout.font_description();

    let layout = paint_minimap_rows(
        cr,
        x,
        y,
        w,
        h,
        minimap,
        theme,
        |cr, vline, row_px, text, row_spans| match render_mode(row_px) {
            MinimapRenderMode::Characters => paint_row_glyphs(
                cr,
                pango_layout,
                saved_font.as_ref(),
                vline,
                row_px,
                text,
                row_spans,
                theme,
            ),
            MinimapRenderMode::ColumnBlocks => paint_row_blocks(cr, vline, text, row_spans, theme),
        },
    );

    pango_layout.set_attributes(None);
    pango_layout.set_font_description(saved_font.as_ref());
    // Reset the ellipsize/width state `paint_row_glyphs` sets so it
    // can't leak onto whatever the shared layout paints next.
    pango_layout.set_width(-1);
    pango_layout.set_ellipsize(pango::EllipsizeMode::None);

    layout
}

/// Draw a [`Minimap`] using a cached [`MinimapCharAtlas`] for real
/// character shapes (#1035), instead of [`draw_minimap`]'s legibility-gated
/// font scaling. `atlas_cache` should be a field the caller keeps alive
/// across frames — [`crate::gtk::backend::GtkBackend`] owns one, the same
/// way it owns `image_cache` for #1014. `dpi_scale` sizes each glyph tile
/// to real device pixels (issue's "Scale / HiDPI" section): a `2` DIP
/// row pitch at `dpi_scale` 2.0 gets `4` real device pixels of glyph
/// detail, not `2`.
///
/// Never shapes at the target pitch: the atlas build (large, once per
/// `(font family, scale)`) happens inside `atlas_cache.get_or_build`
/// only on a cache miss; every paint after that just blits pre-downsampled
/// alpha tiles. Falls back to [`paint_row_blocks`] only if the atlas
/// itself degrades to [`MinimapCharAtlas::filled`] returning a zero-sized
/// tile (never happens in practice — `filled` always clamps to at least
/// `1x1` — kept as a defensive no-op-safe branch rather than an
/// `unwrap`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_minimap_cached(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    minimap: &Minimap,
    theme: &Theme,
    atlas_cache: &mut MinimapAtlasCache,
    dpi_scale: f64,
) -> MinimapLayout {
    let family = pango_layout
        .font_description()
        .and_then(|f| f.family())
        .map(|g| g.to_string())
        .unwrap_or_else(|| "Monospace".to_string());
    let scale = dpi_scale.max(1.0);
    // One column is 1 DIP wide (see `paint_row_blocks`'s own "1px-wide
    // block per column"), one row is `ROW_PITCH_PX` DIPs tall -- convert
    // both to real device pixels via `scale` (issue's HiDPI note: `m =
    // round(scale * 2)` in VS Code's own terms, keeping the *logical*
    // pitch unchanged while the atlas gets real device-pixel detail).
    let tile_w = (scale).round().max(1.0) as usize;
    let tile_h = (ROW_PITCH_PX * scale).round().max(1.0) as usize;

    let atlas = atlas_cache.get_or_build(&family, scale as f32, || {
        build_char_atlas(&family, tile_w, tile_h)
    });

    paint_minimap_rows(
        cr,
        x,
        y,
        w,
        h,
        minimap,
        theme,
        |cr, vline, _row_px, text, row_spans| {
            if atlas.tile_w() == 0 || atlas.tile_h() == 0 {
                paint_row_blocks(cr, vline, text, row_spans, theme);
            } else {
                paint_row_atlas(cr, atlas, scale, vline, text, row_spans, theme);
            }
        },
    )
}

/// Number of ASCII code points [`render_char_sample_sheet`] shapes —
/// see [`crate::primitives::minimap::ATLAS_CHAR_COUNT`].
const CHAR_SAMPLE_CELL_W: f64 = 10.0;
const CHAR_SAMPLE_CELL_H: f64 = 16.0;

/// Build a [`MinimapCharAtlas`] for `family` at `tile_w x tile_h`,
/// falling back to [`MinimapCharAtlas::filled`] (a safe, visually-inert
/// degraded result — see that constructor's docs) if the sample-sheet
/// render itself fails (e.g. surface allocation).
fn build_char_atlas(family: &str, tile_w: usize, tile_h: usize) -> MinimapCharAtlas {
    match render_char_sample_sheet(family) {
        Some(alpha) => MinimapCharAtlas::from_alpha_sheet(
            &alpha,
            (CHAR_SAMPLE_CELL_W as usize) * (crate::primitives::minimap::ATLAS_CHAR_COUNT),
            CHAR_SAMPLE_CELL_H as usize,
            CHAR_SAMPLE_CELL_W as usize,
            tile_w,
            tile_h,
        ),
        None => MinimapCharAtlas::filled(tile_w, tile_h),
    }
}

/// Shape every sampled ASCII character (see
/// [`crate::primitives::minimap::ATLAS_FIRST_CHAR`]/`ATLAS_LAST_CHAR`)
/// once, at `CHAR_SAMPLE_CELL_H` px bold, into one sample sheet —
/// mirrors VS Code's `MinimapCharRenderer.createSampleData` (96 cells,
/// 10px advance, 16px tall, bold 16px font). Returns a tightly-packed,
/// row-major *coverage* buffer (`sheet_w * CHAR_SAMPLE_CELL_H` bytes,
/// `0` transparent .. `255` opaque): text is painted solid white onto a
/// fully transparent background, so the surface's own alpha channel
/// *is* anti-aliasing coverage directly — no premultiplied-colour
/// division needed the way a canvas `getImageData` readback (VS Code's
/// own situation) would.
fn render_char_sample_sheet(family: &str) -> Option<Vec<u8>> {
    let char_count = crate::primitives::minimap::ATLAS_CHAR_COUNT;
    let cell_w = CHAR_SAMPLE_CELL_W as i32;
    let sheet_h = CHAR_SAMPLE_CELL_H as i32;
    let sheet_w = cell_w * char_count as i32;

    let surface = ImageSurface::create(Format::ARgb32, sheet_w, sheet_h).ok()?;
    let cr = Context::new(&surface).ok()?;
    let pango_layout = pangocairo::functions::create_layout(&cr);

    let mut font = pango::FontDescription::new();
    font.set_family(family);
    font.set_weight(pango::Weight::Bold);
    font.set_absolute_size(CHAR_SAMPLE_CELL_H * 0.85 * pango::SCALE as f64);
    pango_layout.set_font_description(Some(&font));

    cr.set_source_rgb(1.0, 1.0, 1.0);
    for (i, code) in (ATLAS_FIRST_CHAR..=ATLAS_LAST_CHAR).enumerate() {
        let ch = char::from_u32(code)?;
        pango_layout.set_text(&ch.to_string());
        cr.move_to(i as f64 * CHAR_SAMPLE_CELL_W, 0.0);
        super::painted_text::show_layout(&cr, &pango_layout);
    }
    surface.flush();

    let stride = surface.stride() as usize;
    let mut surface = surface;
    let data = surface.data().ok()?;
    let (sheet_w, sheet_h) = (sheet_w as usize, sheet_h as usize);
    let mut alpha = vec![0u8; sheet_w * sheet_h];
    for row in 0..sheet_h {
        for col in 0..sheet_w {
            // Cairo's native `ARgb32` byte order is [B, G, R, A]
            // (little-endian) -- see `gtk::minimap::tests::pixel`'s own
            // comment for the same layout read from the colour side.
            alpha[row * sheet_w + col] = data[row * stride + col * 4 + 3];
        }
    }
    Some(alpha)
}

/// Paint one row using `atlas`'s pre-downsampled alpha tiles: blit a
/// tile per non-blank character, tinted by whichever span covers it — no
/// shaping, matching [`paint_row_blocks`]'s own column-capacity bound
/// (#667 pt. 3) and whitespace skip, but painting real glyph shapes
/// instead of solid blocks.
#[allow(clippy::too_many_arguments)]
fn paint_row_atlas(
    cr: &Context,
    atlas: &MinimapCharAtlas,
    dpi_scale: f64,
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
        blit_alpha_tile(
            cr,
            atlas.tile(ch),
            atlas.tile_w(),
            atlas.tile_h(),
            vline.bounds.x as f64 + col as f64,
            vline.bounds.y as f64,
            dpi_scale,
            color,
        );
    }
}

/// Blit one `tile_w x tile_h` alpha tile (device pixels) at logical
/// position `(x, y)`, tinted by `color`. Builds a small `A8` surface
/// from `tile` and paints `color` through it via `mask_surface` — a pure
/// alpha blend, not a shaped glyph run. The `cr.scale(1.0 / dpi_scale,
/// ..)` maps the tile's device-pixel resolution back down to logical
/// units, so a `tile_w x tile_h` device-pixel tile always occupies
/// exactly one column x [`ROW_PITCH_PX`] logical units on screen,
/// regardless of `dpi_scale`.
#[allow(clippy::too_many_arguments)]
fn blit_alpha_tile(
    cr: &Context,
    tile: &[u8],
    tile_w: usize,
    tile_h: usize,
    x: f64,
    y: f64,
    dpi_scale: f64,
    color: crate::types::Color,
) {
    if tile_w == 0 || tile_h == 0 {
        return;
    }
    let Ok(mut surface) = ImageSurface::create(Format::A8, tile_w as i32, tile_h as i32) else {
        return;
    };
    {
        let stride = surface.stride() as usize;
        let Ok(mut data) = surface.data() else {
            return;
        };
        for row in 0..tile_h {
            let src = &tile[row * tile_w..(row + 1) * tile_w];
            let dst_off = row * stride;
            data[dst_off..dst_off + tile_w].copy_from_slice(src);
        }
    }
    surface.flush();

    let (r, g, b) = cairo_rgb(color);
    cr.save().ok();
    cr.translate(x, y);
    let inv_scale = 1.0 / dpi_scale.max(f64::MIN_POSITIVE);
    cr.scale(inv_scale, inv_scale);
    cr.set_source_rgb(r, g, b);
    cr.mask_surface(&surface, 0.0, 0.0).ok();
    cr.restore().ok();
}

#[allow(clippy::too_many_arguments)]
fn paint_row_glyphs(
    cr: &Context,
    pango_layout: &pango::Layout,
    saved_font: Option<&pango::FontDescription>,
    vline: &VisibleMinimapLine,
    row_px: f64,
    text: &str,
    row_spans: &[MinimapSpan],
    theme: &Theme,
) {
    let mut font = saved_font.cloned().unwrap_or_default();
    font.set_absolute_size(minimap_font_px(row_px) * pango::SCALE as f64);
    pango_layout.set_font_description(Some(&font));
    // Bound the shaped run to the row's own width and ellipsize rather
    // than shaping (and then relying on the Cairo clip to hide) glyphs
    // that will never be visible -- keeps shaping cheap even for a very
    // long line; the `cr.clip()` in `draw_minimap` is the hard guarantee.
    pango_layout.set_width((vline.bounds.width * pango::SCALE as f32).round() as i32);
    pango_layout.set_ellipsize(pango::EllipsizeMode::End);

    // Never shape more than the strip's column capacity worth of a line
    // (#667 pt. 3) -- a pathologically long line must cost no more to
    // shape than a short one, mirroring `paint_row_blocks`' own walk cap.
    let truncated = truncate_to_columns(text, COLUMN_CAPACITY);
    pango_layout.set_text(truncated);

    if row_spans.is_empty() {
        pango_layout.set_attributes(None);
    } else {
        let to_u16 = |c: u8| -> u16 { ((c as u16) << 8) | c as u16 };
        let attrs = pango::AttrList::new();
        for span in row_spans {
            let (start, end) = char_range_to_byte_range(truncated, span.start_col, span.end_col);
            let mut a = pango::AttrColor::new_foreground(
                to_u16(span.color.r),
                to_u16(span.color.g),
                to_u16(span.color.b),
            );
            a.set_start_index(start);
            a.set_end_index(end);
            attrs.insert(a);
        }
        pango_layout.set_attributes(Some(&attrs));
    }

    set_source(cr, theme.foreground);
    cr.move_to(vline.bounds.x as f64, vline.bounds.y as f64);
    super::painted_text::show_layout(cr, pango_layout);
}

/// Paint one 1px-wide block per non-blank character column of `text`,
/// each coloured by whichever span covers it — VS Code's
/// `renderCharacters: false` look, which (unlike a single per-line bar)
/// preserves the line's indent and internal-gap silhouette (#667 pt. 2).
/// Stops after [`COLUMN_CAPACITY`] columns, so a pathologically long
/// line costs no more to paint than a short one.
fn paint_row_blocks(
    cr: &Context,
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
        set_source(cr, color);
        cr.rectangle(
            vline.bounds.x as f64 + col as f64,
            vline.bounds.y as f64,
            1.0,
            vline.bounds.height as f64,
        );
        cr.fill().ok();
    }
}

/// Convert a `[start_col, end_col)` character range into UTF-8 byte
/// offsets Pango attributes need. Out-of-range columns clamp to the
/// text's own char count rather than panicking.
fn char_range_to_byte_range(text: &str, start_col: usize, end_col: usize) -> (u32, u32) {
    let mut start_byte = text.len();
    let mut end_byte = text.len();
    let mut char_idx = 0usize;
    for (byte_idx, _) in text.char_indices() {
        if char_idx == start_col {
            start_byte = start_byte.min(byte_idx);
        }
        if char_idx == end_col {
            end_byte = byte_idx;
        }
        char_idx += 1;
    }
    if start_col == 0 {
        start_byte = 0;
    }
    if end_col >= char_idx {
        end_byte = text.len();
    }
    (
        start_byte.min(text.len()) as u32,
        end_byte.min(text.len()) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::minimap::{MinimapHit, MinimapLine};
    use crate::types::{Color, WidgetId};
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

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

    fn surface_and_layout() -> (CairoContext, pango::Layout) {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let layout = pangocairo::functions::create_layout(&cr);
        (cr, layout)
    }

    // ── fixed pitch, not file-length-dependent (#667) ────────────────

    #[test]
    fn row_pitch_is_independent_of_the_strip_height() {
        // Inverts the old #382/#663 contract on purpose: the same
        // buffer painted into strips of two very different heights must
        // now resolve to the *same* row pitch. See `Minimap::layout`'s
        // module docs for why (`MinimapSizing::FixedPitch`).
        let lines: Vec<&str> = vec!["fn main() {}"; 8];
        let mm = minimap_from(lines, 8);

        let tall = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 800.0);
        let short = gtk_minimap_layout(&mm, 0.0, 0.0, 40.0, 16.0);

        let tall_px = tall.visible_lines[0].bounds.height as f64;
        let short_px = short.visible_lines[0].bounds.height as f64;
        assert_eq!(tall_px, ROW_PITCH_PX);
        assert_eq!(short_px, ROW_PITCH_PX);
    }

    #[test]
    fn row_pitch_is_independent_of_the_file_length() {
        // The other half of the same guarantee: a 3-line file and a
        // 10,000-line file painted into the *same* strip must also
        // resolve to the same row pitch.
        let short_mm = minimap_from(vec!["fn main() {}"; 3], 3);
        let long_lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let long_refs: Vec<&str> = long_lines.iter().map(String::as_str).collect();
        let long_mm = minimap_from(long_refs, 10_000);

        let short_layout = gtk_minimap_layout(&short_mm, 0.0, 0.0, 40.0, 400.0);
        let long_layout = gtk_minimap_layout(&long_mm, 0.0, 0.0, 40.0, 400.0);

        assert_eq!(
            short_layout.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
        assert_eq!(
            long_layout.visible_lines[0].bounds.height as f64,
            ROW_PITCH_PX
        );
    }

    #[test]
    fn short_file_in_a_tall_strip_does_not_stretch() {
        // A 3-line file at the fixed pitch occupies 6px of a 200px
        // strip -- it must not stretch to fill it (the pre-#667 `Fill`
        // behaviour, now GTK-inapplicable).
        let mm = minimap_from(vec!["//! Placeholder module"; 3], 3);
        let layout = gtk_minimap_layout(&mm, 0.0, 0.0, 200.0, 200.0);
        assert_eq!(layout.visible_lines.len(), 3);
        let last = layout.visible_lines.last().unwrap();
        assert!(
            (last.bounds.y + last.bounds.height) < 200.0,
            "a short file must not stretch its rows to fill the whole strip"
        );
    }

    // ── legibility floor ─────────────────────────────────────────────
    //
    // `is_legible`/`render_mode`'s own threshold behaviour is covered by
    // `primitives::minimap::tests::legibility_floor_switches_render_mode_on_both_sides`
    // (#738) — this module keeps only the paint-level regression that a
    // real Cairo/Pango pass actually respects that threshold.

    #[test]
    fn default_fixed_pitch_stays_below_the_floor_and_never_shapes_text() {
        // ROW_PITCH_PX (2px) is below LEGIBILITY_FLOOR_PX (4px), so the
        // default GTK minimap always lands in ColumnBlocks -- and,
        // unlike pre-#667, that no longer changes with strip height.
        let mm = minimap_from(vec!["fn main() {}"; 4], 4);
        let (cr, layout) = surface_and_layout();

        let _ = draw_minimap(&cr, &layout, 0.0, 0.0, 40.0, 4.0, &mm, &Theme::default());
        assert_eq!(layout.text(), "");

        let _ = draw_minimap(&cr, &layout, 0.0, 0.0, 40.0, 400.0, &mm, &Theme::default());
        assert_eq!(
            layout.text(),
            "",
            "the fixed pitch stays below the legibility floor regardless of strip height"
        );
    }

    #[test]
    fn characters_branch_truncates_to_the_column_capacity_before_shaping() {
        // The Characters branch is unreachable through the default fixed
        // pitch today, but it must still bound its shaping cost (#667 pt.
        // 3) -- exercised directly since `draw_minimap` can't reach it
        // via `ROW_PITCH_PX` alone.
        let long_line = "y".repeat(10_000);
        let (cr, pango_layout) = surface_and_layout();
        let vline = VisibleMinimapLine {
            start_line_idx: 0,
            bounds: QRect::new(0.0, 0.0, 400.0, 20.0),
        };
        paint_row_glyphs(
            &cr,
            &pango_layout,
            None,
            &vline,
            20.0,
            &long_line,
            &[],
            &Theme::default(),
        );
        assert_eq!(
            pango_layout.text().chars().count(),
            COLUMN_CAPACITY,
            "must shape no more than COLUMN_CAPACITY characters even for a 10,000-char line"
        );
    }

    // ── glyph atlas (#1035) ─────────────────────────────────────────────

    /// Paint `mm` through [`draw_minimap_cached`] into a fresh surface and
    /// return the raw pixel bytes -- used to compare two paints for
    /// pixel-level identity/difference.
    fn paint_cached_pixels(mm: &Minimap, w: i32, h: i32) -> Vec<u8> {
        let mut surface = ImageSurface::create(Format::ARgb32, w, h).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            let mut cache = MinimapAtlasCache::new();
            draw_minimap_cached(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                w as f64,
                h as f64,
                mm,
                &Theme::default(),
                &mut cache,
                1.0,
            );
        }
        surface.flush();
        let pixels = surface.data().expect("surface data").to_vec();
        pixels
    }

    #[test]
    fn atlas_mode_differentiates_content_column_blocks_would_make_identical() {
        // #1035's own acceptance bar: two rows of equal length, both
        // entirely non-blank, must paint *different* alpha patterns
        // under real character shapes -- `ColumnBlocks` would paint an
        // identical run of solid columns for both (every column non-blank
        // either way), which is exactly the information this issue is
        // about not throwing away.
        let code_line = "fn main() {";
        let comment_line = "///////////";
        assert_eq!(
            code_line.chars().count(),
            comment_line.chars().count(),
            "both lines must be the same length for this to be a fair comparison"
        );

        let code_pixels = paint_cached_pixels(&minimap_from(vec![code_line], 1), 40, 4);
        let comment_pixels = paint_cached_pixels(&minimap_from(vec![comment_line], 1), 40, 4);

        assert_ne!(
            code_pixels, comment_pixels,
            "Characters-mode (atlas) rows for different same-length, all-non-blank \
             content must paint different pixels"
        );
    }

    #[test]
    fn atlas_mode_paints_something_for_a_non_blank_line() {
        // A baseline sanity check alongside the differentiation test
        // above: a non-blank line must actually paint ink, not silently
        // no-op into an all-background surface (which would make the
        // "differentiates content" test above meaningless if both inputs
        // painted nothing).
        let bg = ImageSurface::create(Format::ARgb32, 40, 4)
            .and_then(|mut s| {
                let cr = CairoContext::new(&s)?;
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.paint()?;
                drop(cr); // release the surface borrow before `s.data()`
                s.flush();
                Ok(s.data().expect("surface data").to_vec())
            })
            .expect("blank surface");

        let painted = paint_cached_pixels(&minimap_from(vec!["fn main() {"], 1), 40, 4);
        assert_ne!(
            painted, bg,
            "a non-blank line must paint something other than a uniform background"
        );
    }

    #[test]
    fn atlas_mode_falls_back_to_column_blocks_visuals_when_atlas_is_degenerate() {
        // `draw_minimap_cached`'s defensive branch for a zero-sized atlas
        // (never hit via the real `build_char_atlas` path, since
        // `MinimapCharAtlas::filled`/`from_alpha_sheet` both clamp
        // tile_w/tile_h to at least 1 -- see `draw_minimap_cached`'s own
        // doc) still needs to paint *something* sane rather than nothing.
        // Exercise `paint_row_blocks` directly (the same function the
        // fallback branch calls) to confirm that half of the contract
        // independently of atlas construction.
        let mm = minimap_from(vec!["    x  y"], 1);
        let theme = Theme {
            background: Color::rgb(10, 10, 10),
            foreground: Color::rgb(200, 200, 200),
            ..Theme::default()
        };
        let mut surface = ImageSurface::create(Format::ARgb32, 20, 4).expect("create surface");
        let layout = {
            let cr = CairoContext::new(&surface).expect("Context::new");
            let vline = layout_first_vline(&mm, 20.0, 4.0);
            let row_spans: &[MinimapSpan] = &[];
            paint_row_blocks(&cr, &vline, &mm.lines[0].text, row_spans, &theme);
            vline
        };
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);
        assert_eq!(pixel(&data, stride, layout.bounds.x as i32 + 4, 0), fg);
    }

    fn layout_first_vline(mm: &Minimap, w: f64, h: f64) -> VisibleMinimapLine {
        gtk_minimap_layout(mm, 0.0, 0.0, w, h).visible_lines[0]
    }

    // ── per-column colour blocks (#667 pt. 2) ─────────────────────────

    #[test]
    fn column_blocks_skip_indentation_and_internal_gaps() {
        // 4-space indent, "x", 2-space internal gap, "y": the walk must
        // paint no block over the indent or the gap -- only over the two
        // non-blank columns, preserving the indent silhouette.
        let mm = minimap_from(vec!["    x  y"], 1);
        let theme = Theme {
            background: Color::rgb(10, 10, 10),
            foreground: Color::rgb(200, 200, 200),
            ..Theme::default()
        };

        let mut surface = ImageSurface::create(Format::ARgb32, 20, 4).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_minimap(&cr, &pango_layout, 0.0, 0.0, 20.0, 4.0, &mm, &theme);
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let bg = (theme.background.r, theme.background.g, theme.background.b);
        let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);

        for col in 0..4 {
            assert_eq!(
                pixel(&data, stride, col, 0),
                bg,
                "indent column {col} must stay blank"
            );
        }
        assert_eq!(
            pixel(&data, stride, 4, 0),
            fg,
            "'x' column must paint a block"
        );
        for col in 5..7 {
            assert_eq!(
                pixel(&data, stride, col, 0),
                bg,
                "gap column {col} must stay blank"
            );
        }
        assert_eq!(
            pixel(&data, stride, 7, 0),
            fg,
            "'y' column must paint a block"
        );
    }

    #[test]
    fn column_blocks_stop_at_the_column_capacity() {
        let long_line = "x".repeat(10_000);
        let mm = minimap_from(vec![&long_line], 1);
        let theme = Theme {
            background: Color::rgb(1, 1, 1),
            foreground: Color::rgb(250, 250, 250),
            ..Theme::default()
        };
        let strip_w = (COLUMN_CAPACITY + 50) as i32;

        let mut surface = ImageSurface::create(Format::ARgb32, strip_w, 4).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_minimap(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                strip_w as f64,
                4.0,
                &mm,
                &theme,
            );
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);
        let bg = (theme.background.r, theme.background.g, theme.background.b);
        assert_eq!(
            pixel(&data, stride, (COLUMN_CAPACITY - 1) as i32, 0),
            fg,
            "the last in-capacity column must still paint"
        );
        assert_eq!(
            pixel(&data, stride, COLUMN_CAPACITY as i32, 0),
            bg,
            "the walk must stop at COLUMN_CAPACITY and paint no further"
        );
    }

    // `color_at_column`/`SpanCursor`'s own behaviour is covered by
    // `primitives::minimap::tests::color_at_column_*` /
    // `span_cursor_matches_a_full_linear_scan_per_row` (#738) — nothing
    // GTK-specific left to re-test here.

    // ── paint/click round trip ────────────────────────────────────────

    /// Shared body for `paint_and_click_round_trip_returns_seek_for_the_clicked_fraction`
    /// — `minimap_layout` is documented **ABSOLUTE**
    /// (`docs/PRIMITIVE_RULES.md` "Coordinate frames for `*_layout`
    /// methods", issue #505), so `hit_test` must be called with
    /// coordinates shifted by the same `x`/`y` origin the strip was
    /// painted at.
    fn paint_and_click_round_trip_at(x: f64, y: f64) {
        let mm = minimap_from(vec!["x"; 8], 8);
        let (cr, pango_layout) = surface_and_layout();
        let layout = draw_minimap(
            &cr,
            &pango_layout,
            x,
            y,
            40.0,
            100.0,
            &mm,
            &Theme::default(),
        );
        assert_eq!(
            layout.hit_test((x + 20.0) as f32, (y + 50.0) as f32),
            MinimapHit::Seek { fraction: 0.5 }
        );
        assert_eq!(
            layout.hit_test((x + 20.0) as f32, y as f32),
            MinimapHit::Seek { fraction: 0.0 }
        );
    }

    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction() {
        paint_and_click_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends"): `(x, y) = (0, 0)` is exactly the case where a
    /// LOCAL/ABSOLUTE mixup in `gtk_minimap_layout` would be invisible.
    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction_at_nonzero_origin() {
        paint_and_click_round_trip_at(7.0, 13.0);
    }

    #[test]
    fn char_range_to_byte_range_handles_multibyte_text() {
        // "café" — 'é' is 2 bytes (bytes 3-4), so the byte length (5)
        // diverges from the char count (4).
        let (start, end) = char_range_to_byte_range("café", 3, 4);
        assert_eq!(start, 3);
        assert_eq!(end, 5);
    }

    #[test]
    fn empty_minimap_is_a_no_op() {
        let mm = minimap_from(vec![], 0);
        let (cr, pango_layout) = surface_and_layout();
        let layout = draw_minimap(
            &cr,
            &pango_layout,
            0.0,
            0.0,
            40.0,
            100.0,
            &mm,
            &Theme::default(),
        );
        assert!(layout.visible_lines.is_empty());
    }

    // ── strip clipping (#663) ───────────────────────────────────────────

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    ///
    /// Cairo's `ARgb32` stores each pixel as four bytes in native
    /// (little-endian) byte order: [B, G, R, A].
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    #[test]
    fn wide_line_blocks_never_paint_right_of_bounds() {
        // Pre-#663 there was no `cr.clip()` and no Pango layout width, so
        // a line wider than the strip painted straight across whatever
        // sat beside it (in vimcode's case, the neighbouring editor
        // pane). Paint a pathologically long line into a strip embedded
        // in a wider white canvas and assert nothing right of the strip
        // is touched.
        const STRIP_W: i32 = 40;
        const CANVAS_W: i32 = 200;
        const H: i32 = 40;

        let long_line = "x".repeat(500);
        let mm = minimap_from(vec![&long_line], 1);

        let mut surface =
            ImageSurface::create(Format::ARgb32, CANVAS_W, H).expect("create surface");
        {
            let cr = CairoContext::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0); // white sentinel
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let theme = Theme {
                background: Color::rgb(20, 20, 20),
                foreground: Color::rgb(255, 255, 255),
                ..Theme::default()
            };
            // The fixed pitch (ROW_PITCH_PX, 2px) is below the
            // legibility floor, so this exercises the ColumnBlocks
            // branch -- the column-capacity walk on its own already
            // bounds how far right it paints, but the clip is still the
            // hard guarantee this test checks.
            draw_minimap(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                STRIP_W as f64,
                H as f64,
                &mm,
                &theme,
            );
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        for y in 0..H {
            for x in STRIP_W..CANVAS_W {
                assert_eq!(
                    pixel(&data, stride, x, y),
                    (255, 255, 255),
                    "pixel ({x},{y}) is right of bounds.x + bounds.width and must be untouched"
                );
            }
        }
    }
}
