//! GTK rasteriser for [`crate::TabBar`].
//!
//! Paints the tab bar onto a [`Context`] using a [`pango::Layout`] for
//! text measurement. Computes per-tab Pango pixel widths internally
//! (Pango interior-mutability requires a single layout handle that
//! both measures and paints, so splitting the work across the call
//! boundary would force callers to thread the handle through twice).
//!
//! Returns a generic [`TabBarHits`] so callers can resolve clicks
//! using their own segment-id conventions; vimcode walks the result
//! to construct its app-specific `TabBarHitInfo`.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::{cairo_rgb, set_source};
use crate::primitives::tab_bar::{TabBar, TabBarHits};
use crate::theme::Theme;

/// Per-tab padding (left + right) inside the tab background fill.
const TAB_PAD: f64 = 14.0;
/// Gap between the tab label and the close glyph.
const TAB_INNER_GAP: f64 = 10.0;
/// Gap between adjacent tabs.
const TAB_OUTER_GAP: f64 = 1.0;

/// Draw a [`TabBar`] into `(0, y_offset, width, line_height * 1.6)`
/// on `cr`. Caller is responsible for setting the desired UI font on
/// `layout` *before* calling — the rasteriser uses
/// [`pango::Layout::font_description`] as the base font and toggles
/// to a Pango Italic variant for preview tabs.
///
/// `hovered_close_tab` is a per-frame interaction override: when
/// `Some(i)` the `i`-th tab gets a rounded hover background behind
/// its close glyph. The primitive itself carries no mouse state.
///
/// # Visual contract
///
/// - **Tab row height:** `(line_height * 1.6).ceil()` — vertical
///   padding so the tabs don't touch the cell above.
/// - **Active tab:** `theme.tab_active_bg` background, optional 2 px
///   accent line at the top edge in [`TabBar::active_accent`].
/// - **Dirty tab:** close glyph is `●` (in `theme.foreground`)
///   instead of `×`.
/// - **Preview tab:** italicised label.
/// - **Right segments:** painted in `tab_inactive_fg` (or
///   `tab_active_fg` when `seg.is_active`), no bold.
#[allow(clippy::too_many_arguments)]
pub fn draw_tab_bar(
    cr: &Context,
    layout: &pango::Layout,
    width: f64,
    line_height: f64,
    y_offset: f64,
    bar: &TabBar,
    theme: &Theme,
    hovered_close_tab: Option<usize>,
) -> TabBarHits {
    let tab_row_height = (line_height * 1.6).ceil();
    let text_y_offset = y_offset + (tab_row_height - line_height) / 2.0;

    // Tab bar background.
    set_source(cr, theme.tab_bar_bg);
    cr.rectangle(0.0, y_offset, width, tab_row_height);
    cr.fill().ok();

    layout.set_attributes(None);
    let saved_font = layout.font_description().unwrap_or_default();
    let normal_font = saved_font.clone();
    let mut italic_font = normal_font.clone();
    italic_font.set_style(pango::Style::Italic);

    // ── Right-segment Pango widths (no painting yet) ──────────────────
    let mut right_widths: Vec<f64> = Vec::with_capacity(bar.right_segments.len());
    for seg in &bar.right_segments {
        layout.set_font_description(Some(&normal_font));
        layout.set_text(&seg.text);
        let (w, _) = layout.pixel_size();
        right_widths.push(w as f64);
    }
    let reserved_px: f64 = right_widths.iter().sum();
    let effective_tab_area = (width - reserved_px).max(0.0);

    // ── Per-tab measurement ──────────────────────────────────────────
    let close_w = {
        layout.set_font_description(Some(&normal_font));
        layout.set_text("×");
        let (w, _) = layout.pixel_size();
        w as f64
    };

    // Pre-measure every tab's full slot width (label + padding +
    // close button + outer gap). Used both to compute the correct
    // scroll offset AND for the per-tab paint loop below.
    let tab_slot_widths: Vec<f64> = bar
        .tabs
        .iter()
        .map(|tab| {
            if tab.is_preview {
                layout.set_font_description(Some(&italic_font));
            } else {
                layout.set_font_description(Some(&normal_font));
            }
            layout.set_text(&tab.label);
            let (name_w, _) = layout.pixel_size();
            TAB_PAD + name_w as f64 + TAB_INNER_GAP + close_w + TAB_PAD + TAB_OUTER_GAP
        })
        .collect();

    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    let correct_scroll_offset = if let Some(active) = active_idx {
        TabBar::fit_active_scroll_offset(active, bar.tabs.len(), effective_tab_area as usize, |i| {
            tab_slot_widths[i] as usize
        })
    } else {
        bar.scroll_offset
    };

    // ── Tabs paint loop ──────────────────────────────────────────────
    let mut slot_positions: Vec<(f64, f64)> = Vec::with_capacity(bar.tabs.len());
    let mut close_bounds: Vec<Option<(f64, f64)>> = Vec::with_capacity(bar.tabs.len());
    for _ in 0..bar.scroll_offset.min(bar.tabs.len()) {
        slot_positions.push((0.0, 0.0));
        close_bounds.push(None);
    }

    let mut x = 0.0_f64;
    for (tab_idx, tab) in bar.tabs.iter().enumerate().skip(bar.scroll_offset) {
        if tab.is_preview {
            layout.set_font_description(Some(&italic_font));
        } else {
            layout.set_font_description(Some(&normal_font));
        }
        layout.set_text(&tab.label);
        let (tab_name_w, _) = layout.pixel_size();
        let tab_w = tab_name_w as f64;
        let tab_content_w = TAB_PAD + tab_w + TAB_INNER_GAP + close_w + TAB_PAD;
        let slot_w = tab_content_w + TAB_OUTER_GAP;
        if x + slot_w > effective_tab_area {
            break;
        }
        slot_positions.push((x, x + slot_w));
        // Close-button hit zone matches the rendered glyph's pad-extended
        // box (the rounded hover-bg rect). See `close_x` below.
        let close_x = x + TAB_PAD + tab_w + TAB_INNER_GAP;
        let close_pad = 2.0;
        close_bounds.push(Some((close_x - close_pad, close_x + close_w + close_pad)));

        // Tab background.
        let bg_col = if tab.is_active {
            theme.tab_active_bg
        } else {
            theme.tab_bar_bg
        };
        set_source(cr, bg_col);
        cr.rectangle(x, y_offset, tab_content_w, tab_row_height);
        cr.fill().ok();

        // Top accent line for active tab in focused group.
        if tab.is_active {
            if let Some(accent) = bar.active_accent {
                let (ar, ag, ab) = cairo_rgb(accent);
                cr.set_source_rgb(ar, ag, ab);
                cr.rectangle(x, y_offset, tab_content_w, 2.0);
                cr.fill().ok();
            }
        }

        // Tab text.
        let fg_col = match (tab.is_active, tab.is_preview) {
            (true, true) => theme.tab_preview_active_fg,
            (true, false) => theme.tab_active_fg,
            (false, true) => theme.tab_preview_inactive_fg,
            (false, false) => theme.tab_inactive_fg,
        };
        set_source(cr, fg_col);
        layout.set_font_description(Some(if tab.is_preview {
            &italic_font
        } else {
            &normal_font
        }));
        cr.move_to(x + TAB_PAD, text_y_offset);
        pcfn::show_layout(cr, layout);

        // Close (×) or dirty (●) glyph — with optional rounded hover bg.
        let close_x = x + TAB_PAD + tab_w + TAB_INNER_GAP;
        let is_close_hovered = hovered_close_tab == Some(tab_idx);
        if is_close_hovered {
            let pad = 2.0;
            let rx = close_x - pad;
            let ry = text_y_offset + pad;
            let rw = close_w + pad * 2.0;
            let rh = line_height - pad * 2.0;
            let (hr, hg, hb) = cairo_rgb(theme.foreground);
            cr.set_source_rgba(hr, hg, hb, 0.15);
            let radius = 3.0;
            cr.new_path();
            cr.arc(
                rx + rw - radius,
                ry + radius,
                radius,
                -std::f64::consts::FRAC_PI_2,
                0.0,
            );
            cr.arc(
                rx + rw - radius,
                ry + rh - radius,
                radius,
                0.0,
                std::f64::consts::FRAC_PI_2,
            );
            cr.arc(
                rx + radius,
                ry + rh - radius,
                radius,
                std::f64::consts::FRAC_PI_2,
                std::f64::consts::PI,
            );
            cr.arc(
                rx + radius,
                ry + radius,
                radius,
                std::f64::consts::PI,
                3.0 * std::f64::consts::FRAC_PI_2,
            );
            cr.close_path();
            cr.fill().ok();
        }
        let close_glyph = if tab.is_dirty && !is_close_hovered {
            "●"
        } else {
            "×"
        };
        let close_fg = if tab.is_dirty || is_close_hovered {
            theme.foreground
        } else if tab.is_active {
            theme.tab_inactive_fg
        } else {
            theme.separator
        };
        set_source(cr, close_fg);
        layout.set_font_description(Some(&normal_font));
        layout.set_text(close_glyph);
        cr.move_to(close_x, text_y_offset);
        pcfn::show_layout(cr, layout);

        x += slot_w;
    }

    // ── Right segments paint loop ────────────────────────────────────
    let right_base = width - reserved_px;
    let mut right_segment_bounds: Vec<(f64, f64)> = Vec::with_capacity(bar.right_segments.len());
    let mut sx = right_base;
    for (i, seg) in bar.right_segments.iter().enumerate() {
        let seg_w = right_widths[i];
        let fg_col = if seg.is_active {
            theme.tab_active_fg
        } else {
            theme.tab_inactive_fg
        };
        set_source(cr, fg_col);
        layout.set_font_description(Some(&normal_font));
        layout.set_text(&seg.text);
        cr.move_to(sx, text_y_offset);
        pcfn::show_layout(cr, layout);
        right_segment_bounds.push((sx, sx + seg_w));
        sx += seg_w;
    }

    // Sample measurement for char-col estimation. The 15-char string
    // mirrors what vimcode's pre-lift renderer used.
    layout.set_font_description(Some(&normal_font));
    layout.set_text("ABCDabcd0123.:_");
    let (sample_px, _) = layout.pixel_size();
    let char_w = (sample_px as f64 / 15.0).max(1.0);
    let available_cols = (effective_tab_area / char_w).floor().max(0.0) as usize;

    // Restore caller's font.
    layout.set_font_description(Some(&saved_font));

    TabBarHits {
        slot_positions,
        close_bounds,
        right_segment_bounds,
        available_cols,
        correct_scroll_offset,
    }
}
