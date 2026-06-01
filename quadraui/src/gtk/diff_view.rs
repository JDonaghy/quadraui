//! GTK rasteriser for [`crate::primitives::diff_view::DiffView`].
//!
//! Paints a two-pane (side-by-side) or single-column (unified) diff onto a
//! [`gtk4::cairo::Context`] using a [`pango::Layout`] for text rendering.
//!
//! The visual contract mirrors the TUI rasteriser — same colour mapping,
//! same row-kind semantics, same `DiffViewLayout` return value.
//!
//! # Side-by-side layout
//!
//! - Left pane width = `(w - 1) / 2` pixels (divider is 1 px wide).
//! - Right pane = remaining width.
//! - Per-row background fills cover the full pane width so padding rows are
//!   visually distinct from content rows.
//!
//! # Unified layout
//!
//! - `@@ ... @@` hunk headers drawn in `theme.accent_fg`.
//! - `-`/`+`/` ` prefix + full-width background fill per row.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::{cairo_rgb, set_source};
use crate::primitives::diff_view::{DiffMode, DiffRowKind, DiffView, DiffViewLayout};
use crate::theme::Theme;

/// Draw a [`DiffView`] into the region `(x, y, w, h)` on `cr`.
///
/// `line_height` is the pixel height of one text row (supplied by the
/// backend from its current font metrics).
///
/// Returns [`DiffViewLayout`] for scroll clamping.
#[allow(clippy::too_many_arguments)]
pub fn draw_diff_view(
    cr: &Context,
    layout: &pango::Layout,
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
            draw_side_by_side(cr, layout, x, y, w, h, view, theme, line_height, total_rows)
        }
        DiffMode::Unified => draw_unified(cr, layout, x, y, w, h, view, theme, line_height),
    }
}

// ── Side-by-side ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_side_by_side(
    cr: &Context,
    pango_layout: &pango::Layout,
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

    let divider_px = 1.0_f64;
    let left_w = ((w - divider_px) / 2.0).floor();
    let right_w = (w - divider_px - left_w).max(0.0);
    let divider_x = x + left_w;

    // Fill total background.
    set_source(cr, theme.background);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    // Draw header row.
    if has_header {
        let hdr_bg = cairo_rgb(theme.header_bg);
        let hdr_fg = cairo_rgb(theme.header_fg);

        cr.set_source_rgb(hdr_bg.0, hdr_bg.1, hdr_bg.2);
        cr.rectangle(x, y, w, line_height);
        cr.fill().ok();

        // Divider in header.
        let div_bg = cairo_rgb(theme.border_fg);
        cr.set_source_rgb(div_bg.0, div_bg.1, div_bg.2);
        cr.rectangle(divider_x, y, divider_px, line_height);
        cr.fill().ok();

        cr.set_source_rgb(hdr_fg.0, hdr_fg.1, hdr_fg.2);
        if let Some(label) = &view.left_label {
            pango_layout.set_text(label);
            pango_layout.set_width((left_w * pango::SCALE as f64) as i32);
            pango_layout.set_ellipsize(pango::EllipsizeMode::End);
            let (_, text_h) = pango_layout.pixel_size();
            cr.move_to(x + 4.0, y + (line_height - text_h as f64) / 2.0);
            pcfn::show_layout(cr, pango_layout);
        }
        if let Some(label) = &view.right_label {
            pango_layout.set_text(label);
            pango_layout.set_width((right_w * pango::SCALE as f64) as i32);
            pango_layout.set_ellipsize(pango::EllipsizeMode::End);
            let (_, text_h) = pango_layout.pixel_size();
            cr.move_to(
                divider_x + divider_px + 4.0,
                y + (line_height - text_h as f64) / 2.0,
            );
            pcfn::show_layout(cr, pango_layout);
        }
        // Reset ellipsize for content rows.
        pango_layout.set_ellipsize(pango::EllipsizeMode::None);
        pango_layout.set_width(-1);
    }

    let content_y_start = y + header_h;
    let content_h = (h - header_h).max(0.0);
    let visible_rows = (content_h / line_height).floor() as usize;

    let all_rows: Vec<_> = view.hunks.iter().flat_map(|h| h.rows.iter()).collect();
    let start = view.scroll_offset.min(total_rows.saturating_sub(1));
    let end = (start + visible_rows).min(total_rows);

    for (row_idx, row) in all_rows.iter().enumerate().skip(start).take(end - start) {
        let row_y = content_y_start + (row_idx - start) as f64 * line_height;

        let (left_fg, left_bg, right_fg, right_bg) = row_colors_gtk(row.kind, theme);

        // Left pane background.
        cr.set_source_rgb(left_bg.0, left_bg.1, left_bg.2);
        cr.rectangle(x, row_y, left_w, line_height);
        cr.fill().ok();

        // Right pane background.
        cr.set_source_rgb(right_bg.0, right_bg.1, right_bg.2);
        cr.rectangle(divider_x + divider_px, row_y, right_w, line_height);
        cr.fill().ok();

        // Divider.
        let div_c = cairo_rgb(theme.border_fg);
        cr.set_source_rgb(div_c.0, div_c.1, div_c.2);
        cr.rectangle(divider_x, row_y, divider_px, line_height);
        cr.fill().ok();

        // Left text.
        if let Some(text) = &row.left {
            cr.set_source_rgb(left_fg.0, left_fg.1, left_fg.2);
            pango_layout.set_text(text);
            let (_, text_h) = pango_layout.pixel_size();
            // Clip to left pane. NOTE: cr.clip() clears the current path
            // (including the current point), so move_to MUST come after clip.
            cr.save().ok();
            cr.rectangle(x, row_y, left_w, line_height);
            cr.clip();
            cr.move_to(x + 4.0, row_y + (line_height - text_h as f64) / 2.0);
            pcfn::show_layout(cr, pango_layout);
            cr.restore().ok();
        }

        // Right text.
        if let Some(text) = &row.right {
            cr.set_source_rgb(right_fg.0, right_fg.1, right_fg.2);
            pango_layout.set_text(text);
            let (_, text_h) = pango_layout.pixel_size();
            cr.save().ok();
            cr.rectangle(divider_x + divider_px, row_y, right_w, line_height);
            cr.clip();
            cr.move_to(
                divider_x + divider_px + 4.0,
                row_y + (line_height - text_h as f64) / 2.0,
            );
            pcfn::show_layout(cr, pango_layout);
            cr.restore().ok();
        }
    }

    DiffViewLayout {
        visible_rows,
        total_rows,
    }
}

// ── Unified ───────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_unified(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    view: &DiffView,
    theme: &Theme,
    line_height: f64,
) -> DiffViewLayout {
    // Fill background.
    set_source(cr, theme.background);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    #[derive(Clone)]
    enum UnifiedLine<'a> {
        Header(String),
        Content(&'a crate::primitives::diff_view::DiffRow),
    }

    let mut lines: Vec<UnifiedLine<'_>> = Vec::new();
    for hunk in &view.hunks {
        let header = format!(
            "@@ -{},{} +{},{} @@",
            hunk.left_start,
            hunk.rows.len(),
            hunk.right_start,
            hunk.rows.len()
        );
        lines.push(UnifiedLine::Header(header));
        for row in &hunk.rows {
            lines.push(UnifiedLine::Content(row));
        }
    }

    let total_display = lines.len();
    let visible_rows = (h / line_height).floor() as usize;
    let start = view.scroll_offset.min(total_display.saturating_sub(1));
    let end = (start + visible_rows).min(total_display);

    for (i, line) in lines.iter().enumerate().skip(start).take(end - start) {
        let row_y = y + (i - start) as f64 * line_height;

        match line {
            UnifiedLine::Header(header_text) => {
                let fg = cairo_rgb(theme.accent_fg);
                let bg = cairo_rgb(theme.background);
                cr.set_source_rgb(bg.0, bg.1, bg.2);
                cr.rectangle(x, row_y, w, line_height);
                cr.fill().ok();
                cr.set_source_rgb(fg.0, fg.1, fg.2);
                pango_layout.set_text(header_text);
                let (_, text_h) = pango_layout.pixel_size();
                cr.save().ok();
                cr.rectangle(x, row_y, w, line_height);
                cr.clip();
                cr.move_to(x + 2.0, row_y + (line_height - text_h as f64) / 2.0);
                pcfn::show_layout(cr, pango_layout);
                cr.restore().ok();
            }
            UnifiedLine::Content(row) => {
                let (prefix, fg, bg) = match row.kind {
                    DiffRowKind::Same => {
                        (' ', cairo_rgb(theme.muted_fg), cairo_rgb(theme.background))
                    }
                    DiffRowKind::Removed | DiffRowKind::Changed => (
                        '-',
                        cairo_rgb(theme.git_deleted),
                        cairo_rgb(theme.diff_removed_bg),
                    ),
                    DiffRowKind::Added => (
                        '+',
                        cairo_rgb(theme.git_added),
                        cairo_rgb(theme.diff_added_bg),
                    ),
                };

                // Row background.
                cr.set_source_rgb(bg.0, bg.1, bg.2);
                cr.rectangle(x, row_y, w, line_height);
                cr.fill().ok();

                cr.set_source_rgb(fg.0, fg.1, fg.2);

                // Prefix character.
                let prefix_str = prefix.to_string();
                pango_layout.set_text(&prefix_str);
                let (pw, text_h) = pango_layout.pixel_size();
                cr.move_to(x + 2.0, row_y + (line_height - text_h as f64) / 2.0);
                pcfn::show_layout(cr, pango_layout);

                // Content text.
                let text = match row.kind {
                    DiffRowKind::Removed => row.left.as_deref().unwrap_or(""),
                    _ => row.right.as_deref().or(row.left.as_deref()).unwrap_or(""),
                };
                pango_layout.set_text(text);
                let (_, text_h2) = pango_layout.pixel_size();
                let text_x = x + 2.0 + pw as f64 + 4.0;
                cr.save().ok();
                cr.rectangle(text_x, row_y, (w - (text_x - x)).max(0.0), line_height);
                cr.clip();
                cr.move_to(text_x, row_y + (line_height - text_h2 as f64) / 2.0);
                pcfn::show_layout(cr, pango_layout);
                cr.restore().ok();
            }
        }
    }

    // Return total_display (content rows + one @@ header per hunk) so
    // callers can clamp scroll_offset correctly in unified mode.
    DiffViewLayout {
        visible_rows,
        total_rows: total_display,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Cairo RGB triple.
type Rgb = (f64, f64, f64);

/// Returns `(left_fg, left_bg, right_fg, right_bg)` for a given row kind.
fn row_colors_gtk(kind: DiffRowKind, theme: &Theme) -> (Rgb, Rgb, Rgb, Rgb) {
    match kind {
        DiffRowKind::Same => (
            cairo_rgb(theme.muted_fg),
            cairo_rgb(theme.background),
            cairo_rgb(theme.muted_fg),
            cairo_rgb(theme.background),
        ),
        DiffRowKind::Changed => (
            cairo_rgb(theme.git_deleted),
            cairo_rgb(theme.diff_removed_bg),
            cairo_rgb(theme.git_added),
            cairo_rgb(theme.diff_added_bg),
        ),
        DiffRowKind::Removed => (
            cairo_rgb(theme.git_deleted),
            cairo_rgb(theme.diff_removed_bg),
            cairo_rgb(theme.muted_fg),
            cairo_rgb(theme.diff_padding_bg),
        ),
        DiffRowKind::Added => (
            cairo_rgb(theme.muted_fg),
            cairo_rgb(theme.diff_padding_bg),
            cairo_rgb(theme.git_added),
            cairo_rgb(theme.diff_added_bg),
        ),
    }
}
