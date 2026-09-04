//! GTK rasteriser for [`crate::primitives::diff_view::DiffView`].
//!
//! Paints a two-pane (side-by-side) or single-column (unified) diff onto a
//! [`gtk4::cairo::Context`] using a [`pango::Layout`] for text rendering.
//! Row/pane geometry and the scroll-clamped visible-line window come from
//! [`DiffView::layout`] — the shared layout API lifted out of three
//! near-identical backend copies (issue #737). This module only converts
//! the resulting DIP-agnostic `f32` geometry to pixel `f64`s and paints;
//! it does not re-derive positions.
//!
//! The visual contract mirrors the TUI rasteriser — same colour mapping
//! (via [`crate::primitives::diff_view::row_colors`] /
//! [`crate::primitives::diff_view::unified_row_style`], also shared, not a
//! third copy of the colour table), same row-kind semantics, same
//! `DiffViewLayout` return value.
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

use super::{cairo_rgb, set_source};
use crate::event::Rect as ERect;
use crate::primitives::diff_view::{
    row_colors, unified_hunk_header, unified_row_style, unified_row_text, DiffLineContent,
    DiffMode, DiffView, DiffViewGeometry, DiffViewLayout,
};
use crate::theme::Theme;

/// Convert an `f32` DIP-agnostic rect from [`DiffView::layout`] to pixel
/// `f64`s.
fn px_rect(r: ERect) -> (f64, f64, f64, f64) {
    (r.x as f64, r.y as f64, r.width as f64, r.height as f64)
}

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
    let viewport = ERect::new(x as f32, y as f32, w as f32, h as f32);
    let geometry = view.layout(viewport, line_height as f32);

    if w <= 0.0 || h <= 0.0 || line_height <= 0.0 {
        return geometry.as_layout();
    }

    // Total background.
    set_source(cr, theme.background);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    match view.mode {
        DiffMode::SideBySide => draw_side_by_side(cr, layout, view, theme, &geometry),
        DiffMode::Unified => draw_unified(cr, layout, view, theme, &geometry),
    }

    geometry.as_layout()
}

// ── Side-by-side ─────────────────────────────────────────────────────────────

fn draw_side_by_side(
    cr: &Context,
    pango_layout: &pango::Layout,
    view: &DiffView,
    theme: &Theme,
    geometry: &DiffViewGeometry,
) {
    let flat = view.flat_rows();

    if let Some(header) = &geometry.header {
        let hdr_bg = cairo_rgb(theme.header_bg);
        let hdr_fg = cairo_rgb(theme.header_fg);
        let (lx, ly, lw, lh) = px_rect(header.left);
        let (rx, _ry, rw, _rh) = px_rect(header.right);
        let (dx, dy, dw, dh) = px_rect(header.divider);

        cr.set_source_rgb(hdr_bg.0, hdr_bg.1, hdr_bg.2);
        cr.rectangle(lx, ly, lw + dw + rw, lh);
        cr.fill().ok();

        let div_bg = cairo_rgb(theme.border_fg);
        cr.set_source_rgb(div_bg.0, div_bg.1, div_bg.2);
        cr.rectangle(dx, dy, dw, dh);
        cr.fill().ok();

        cr.set_source_rgb(hdr_fg.0, hdr_fg.1, hdr_fg.2);
        if let Some(label) = &view.left_label {
            pango_layout.set_text(label);
            pango_layout.set_width((lw * pango::SCALE as f64) as i32);
            pango_layout.set_ellipsize(pango::EllipsizeMode::End);
            let (_, text_h) = pango_layout.pixel_size();
            cr.move_to(lx + 4.0, ly + (lh - text_h as f64) / 2.0);
            super::painted_text::show_layout(cr, pango_layout);
        }
        if let Some(label) = &view.right_label {
            pango_layout.set_text(label);
            pango_layout.set_width((rw * pango::SCALE as f64) as i32);
            pango_layout.set_ellipsize(pango::EllipsizeMode::End);
            let (_, text_h) = pango_layout.pixel_size();
            cr.move_to(rx + 4.0, ly + (lh - text_h as f64) / 2.0);
            super::painted_text::show_layout(cr, pango_layout);
        }
        // Reset ellipsize for content rows.
        pango_layout.set_ellipsize(pango::EllipsizeMode::None);
        pango_layout.set_width(-1);
    }

    for line in &geometry.lines {
        let DiffLineContent::Row { row_idx } = line.content else {
            continue;
        };
        let row = flat[row_idx];
        let (left_fg, left_bg, right_fg, right_bg) = row_colors(row.kind, theme);
        let (left_fg, left_bg, right_fg, right_bg) = (
            cairo_rgb(left_fg),
            cairo_rgb(left_bg),
            cairo_rgb(right_fg),
            cairo_rgb(right_bg),
        );

        let (lx, ly, lw, lh) = px_rect(line.left.expect("side-by-side row has left bounds"));
        let (rx, ry, rw, rh) = px_rect(line.right.expect("side-by-side row has right bounds"));
        let (dx, dy, dw, dh) = px_rect(line.divider.expect("side-by-side row has divider bounds"));

        // Left pane background.
        cr.set_source_rgb(left_bg.0, left_bg.1, left_bg.2);
        cr.rectangle(lx, ly, lw, lh);
        cr.fill().ok();

        // Right pane background.
        cr.set_source_rgb(right_bg.0, right_bg.1, right_bg.2);
        cr.rectangle(rx, ry, rw, rh);
        cr.fill().ok();

        // Divider.
        let div_c = cairo_rgb(theme.border_fg);
        cr.set_source_rgb(div_c.0, div_c.1, div_c.2);
        cr.rectangle(dx, dy, dw, dh);
        cr.fill().ok();

        // Left text.
        if let Some(text) = &row.left {
            cr.set_source_rgb(left_fg.0, left_fg.1, left_fg.2);
            pango_layout.set_text(text);
            let (_, text_h) = pango_layout.pixel_size();
            // Clip to left pane. NOTE: cr.clip() clears the current path
            // (including the current point), so move_to MUST come after clip.
            cr.save().ok();
            cr.rectangle(lx, ly, lw, lh);
            cr.clip();
            cr.move_to(lx + 4.0, ly + (lh - text_h as f64) / 2.0);
            super::painted_text::show_layout(cr, pango_layout);
            cr.restore().ok();
        }

        // Right text.
        if let Some(text) = &row.right {
            cr.set_source_rgb(right_fg.0, right_fg.1, right_fg.2);
            pango_layout.set_text(text);
            let (_, text_h) = pango_layout.pixel_size();
            cr.save().ok();
            cr.rectangle(rx, ry, rw, rh);
            cr.clip();
            cr.move_to(rx + 4.0, ry + (rh - text_h as f64) / 2.0);
            super::painted_text::show_layout(cr, pango_layout);
            cr.restore().ok();
        }
    }
}

// ── Unified ───────────────────────────────────────────────────────────────────

fn draw_unified(
    cr: &Context,
    pango_layout: &pango::Layout,
    view: &DiffView,
    theme: &Theme,
    geometry: &DiffViewGeometry,
) {
    let flat = view.flat_rows();

    for line in &geometry.lines {
        let (rx, ry, rw, rh) = px_rect(line.bounds);

        match line.content {
            DiffLineContent::UnifiedHeader { hunk_idx } => {
                let header_text = unified_hunk_header(&view.hunks[hunk_idx]);
                let fg = cairo_rgb(theme.accent_fg);
                let bg = cairo_rgb(theme.background);
                cr.set_source_rgb(bg.0, bg.1, bg.2);
                cr.rectangle(rx, ry, rw, rh);
                cr.fill().ok();
                cr.set_source_rgb(fg.0, fg.1, fg.2);
                pango_layout.set_text(&header_text);
                let (_, text_h) = pango_layout.pixel_size();
                cr.save().ok();
                cr.rectangle(rx, ry, rw, rh);
                cr.clip();
                cr.move_to(rx + 2.0, ry + (rh - text_h as f64) / 2.0);
                super::painted_text::show_layout(cr, pango_layout);
                cr.restore().ok();
            }
            DiffLineContent::Row { row_idx } => {
                let row = flat[row_idx];
                let (prefix, fg, bg) = unified_row_style(row.kind, theme);
                let (fg, bg) = (cairo_rgb(fg), cairo_rgb(bg));

                // Row background.
                cr.set_source_rgb(bg.0, bg.1, bg.2);
                cr.rectangle(rx, ry, rw, rh);
                cr.fill().ok();

                cr.set_source_rgb(fg.0, fg.1, fg.2);

                // Prefix character.
                let prefix_str = prefix.to_string();
                pango_layout.set_text(&prefix_str);
                let (pw, text_h) = pango_layout.pixel_size();
                cr.move_to(rx + 2.0, ry + (rh - text_h as f64) / 2.0);
                super::painted_text::show_layout(cr, pango_layout);

                // Content text.
                let text = unified_row_text(row);
                pango_layout.set_text(text);
                let (_, text_h2) = pango_layout.pixel_size();
                let text_x = rx + 2.0 + pw as f64 + 4.0;
                cr.save().ok();
                cr.rectangle(text_x, ry, (rw - (text_x - rx)).max(0.0), rh);
                cr.clip();
                cr.move_to(text_x, ry + (rh - text_h2 as f64) / 2.0);
                super::painted_text::show_layout(cr, pango_layout);
                cr.restore().ok();
            }
        }
    }
}
