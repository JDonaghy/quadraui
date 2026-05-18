//! GTK rasteriser for [`crate::TabBar`].
//!
//! Calls [`TabBar::layout`] with Pango pixel measurers to produce a
//! [`TabBarLayout`], then paints from the resolved `visible_tabs` and
//! `visible_segments`. Paint and hit-test consume one layout — no
//! independent geometry derivation.
//!
//! Returns a [`TabBarHits`] (converted from the layout) so callers can
//! resolve clicks using their own segment-id conventions.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::{cairo_rgb, set_source};
use crate::backend::tab_bar_layout_to_hits;
use crate::primitives::tab_bar::{SegmentMeasure, TabBar, TabBarHits, TabMeasure};
use crate::theme::Theme;

/// Per-tab padding (left + right) inside the tab background fill.
const TAB_PAD: f64 = 14.0;
/// Gap between the tab label and the close glyph.
const TAB_INNER_GAP: f64 = 10.0;
/// Gap between adjacent tabs.
const TAB_OUTER_GAP: f64 = 1.0;

/// Draw a [`TabBar`] into `(0, y_offset, width, row_height)` on `cr`.
/// Caller is responsible for setting the desired UI font on `layout`
/// *before* calling — the rasteriser uses
/// [`pango::Layout::font_description`] as the base font and toggles
/// to a Pango Italic variant for preview tabs.
///
/// `row_height` controls the tab bar's vertical extent. Callers that
/// want padded file-tab spacing pass `(line_height * 1.6).ceil()`;
/// callers that want compact bars (terminal toolbar, bottom panel tab
/// switcher) pass `line_height` directly.
///
/// `hovered_close_tab` is a per-frame interaction override: when
/// `Some(i)` the `i`-th tab gets a rounded hover background behind
/// its close glyph. The primitive itself carries no mouse state.
///
/// # Visual contract
///
/// - **Tab row height:** caller-provided via `row_height`.
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
    pango_layout: &pango::Layout,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    theme: &Theme,
    hovered_close_tab: Option<usize>,
) -> TabBarHits {
    let tab_row_height = row_height;
    let text_y_offset = y_offset + (tab_row_height - line_height) / 2.0;

    let tab_pad = if bar.compact { 2.0 } else { TAB_PAD };
    let tab_inner_gap = if bar.compact { 4.0 } else { TAB_INNER_GAP };
    let tab_outer_gap = if bar.compact { 0.0 } else { TAB_OUTER_GAP };

    // Tab bar background.
    set_source(cr, theme.tab_bar_bg);
    cr.rectangle(0.0, y_offset, width, tab_row_height);
    cr.fill().ok();

    pango_layout.set_attributes(None);
    let saved_font = pango_layout.font_description().unwrap_or_default();
    let normal_font = saved_font.clone();
    let mut italic_font = normal_font.clone();
    italic_font.set_style(pango::Style::Italic);

    // ── Pre-measure close glyph ─────────────────────────────────────
    let close_glyph_w = if bar.show_tab_close {
        pango_layout.set_font_description(Some(&normal_font));
        pango_layout.set_text("×");
        let (w, _) = pango_layout.pixel_size();
        w as f64
    } else {
        0.0
    };

    let close_extra = if bar.show_tab_close {
        tab_inner_gap + close_glyph_w
    } else {
        0.0
    };

    // ── Pre-measure tabs → TabMeasure ───────────────────────────────
    let tab_name_widths: Vec<f64> = bar
        .tabs
        .iter()
        .map(|tab| {
            if tab.is_preview {
                pango_layout.set_font_description(Some(&italic_font));
            } else {
                pango_layout.set_font_description(Some(&normal_font));
            }
            pango_layout.set_text(&tab.label);
            let (name_w, _) = pango_layout.pixel_size();
            name_w as f64
        })
        .collect();

    let measure_tab = |i: usize| -> TabMeasure {
        let name_w = tab_name_widths[i] as f32;
        let total =
            tab_pad as f32 + name_w + close_extra as f32 + tab_pad as f32 + tab_outer_gap as f32;
        let close_w = if bar.show_tab_close {
            (tab_inner_gap + close_glyph_w + tab_pad + tab_outer_gap) as f32
        } else {
            0.0
        };
        TabMeasure::new(total, close_w)
    };

    let measure_segment = |i: usize| -> SegmentMeasure {
        pango_layout.set_font_description(Some(&normal_font));
        pango_layout.set_text(&bar.right_segments[i].text);
        let (w, _) = pango_layout.pixel_size();
        SegmentMeasure::new(w as f32)
    };

    // ── Compute layout — single source of truth ─────────────────────
    let layout = bar.layout(
        width as f32,
        row_height as f32,
        0.0, // no scroll arrows in GTK
        measure_tab,
        measure_segment,
    );

    // ── Paint tabs from layout ──────────────────────────────────────
    for vt in &layout.visible_tabs {
        let tab = &bar.tabs[vt.tab_idx];
        let tab_x = vt.bounds.x as f64;
        let tab_visual_w = vt.bounds.width as f64 - tab_outer_gap;

        // Tab background.
        let bg_col = if tab.is_active {
            theme.tab_active_bg
        } else {
            theme.tab_bar_bg
        };
        set_source(cr, bg_col);
        cr.rectangle(tab_x, y_offset, tab_visual_w, tab_row_height);
        cr.fill().ok();

        // Top accent line for active tab in focused group.
        if tab.is_active {
            if let Some(accent) = bar.active_accent {
                let (ar, ag, ab) = cairo_rgb(accent);
                cr.set_source_rgb(ar, ag, ab);
                cr.rectangle(tab_x, y_offset, tab_visual_w, 2.0);
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
        pango_layout.set_font_description(Some(if tab.is_preview {
            &italic_font
        } else {
            &normal_font
        }));
        pango_layout.set_text(&tab.label);
        cr.move_to(tab_x + tab_pad, text_y_offset);
        pcfn::show_layout(cr, pango_layout);

        if bar.show_tab_close {
            if let Some(cb) = vt.close_bounds {
                let close_x = cb.x as f64 + tab_inner_gap;
                let is_close_hovered = hovered_close_tab == Some(vt.tab_idx);

                // Rounded hover background behind close glyph.
                if is_close_hovered {
                    let pad = 2.0;
                    let rx = close_x - pad;
                    let ry = text_y_offset + pad;
                    let rw = close_glyph_w + pad * 2.0;
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
                pango_layout.set_font_description(Some(&normal_font));
                pango_layout.set_text(close_glyph);
                cr.move_to(close_x, text_y_offset);
                pcfn::show_layout(cr, pango_layout);
            }
        }
    }

    // ── Right segments from layout ──────────────────────────────────
    for vs in &layout.visible_segments {
        let seg = &bar.right_segments[vs.segment_idx];
        let fg_col = if seg.is_active {
            theme.tab_active_fg
        } else {
            theme.tab_inactive_fg
        };
        set_source(cr, fg_col);
        pango_layout.set_font_description(Some(&normal_font));
        pango_layout.set_text(&seg.text);
        cr.move_to(vs.bounds.x as f64, text_y_offset);
        pcfn::show_layout(cr, pango_layout);
    }

    // ── Correct scroll offset (engine feedback) ─────────────────────
    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    let seg_widths: Vec<f64> = bar
        .right_segments
        .iter()
        .map(|seg| {
            pango_layout.set_font_description(Some(&normal_font));
            pango_layout.set_text(&seg.text);
            let (w, _) = pango_layout.pixel_size();
            w as f64
        })
        .collect();
    let reserved_px: f64 = seg_widths.iter().sum();
    let effective_tab_area = (width - reserved_px).max(0.0);

    let correct_scroll_offset = if let Some(active) = active_idx {
        let tab_slot_widths: Vec<f64> = (0..bar.tabs.len())
            .map(|i| tab_name_widths[i] + tab_pad * 2.0 + close_extra + tab_outer_gap)
            .collect();
        TabBar::fit_active_scroll_offset(active, bar.tabs.len(), effective_tab_area as usize, |i| {
            tab_slot_widths[i] as usize
        })
    } else {
        bar.scroll_offset
    };

    // ── Sample measurement for char-col estimation ──────────────────
    pango_layout.set_font_description(Some(&normal_font));
    pango_layout.set_text("ABCDabcd0123.:_");
    let (sample_px, _) = pango_layout.pixel_size();
    let char_w = (sample_px as f64 / 15.0).max(1.0);
    let available_cols = (effective_tab_area / char_w).floor().max(0.0) as usize;

    // Restore caller's font.
    pango_layout.set_font_description(Some(&saved_font));

    let mut hits = tab_bar_layout_to_hits(&layout, bar);
    hits.correct_scroll_offset = correct_scroll_offset;
    hits.available_cols = available_cols;
    hits
}
