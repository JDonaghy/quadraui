//! GTK rasteriser for [`crate::TabBar`].
//!
//! Calls [`TabBar::layout`] with Pango pixel measurers to produce a
//! [`TabBarLayout`], then paints from the resolved `visible_tabs` and
//! `visible_segments`. Paint and hit-test consume one layout — no
//! independent geometry derivation.
//!
//! Returns a [`TabBarHits`] (converted from the layout, for callers that
//! resolve clicks using their own segment-id conventions) alongside the
//! [`TabBarLayout`] itself, whose bar-relative `visible_tabs` /
//! `close_bounds` geometry is what `GtkBackend::draw_tab_bar` caches per
//! `WidgetId` for `GtkDriver::tab_center`/`tab_close_center` (quadraui#594).

use gtk4::cairo::Context;
use gtk4::pango;

use super::{cairo_rgb, set_source};
use crate::backend::tab_bar_layout_to_hits;
use crate::primitives::tab_bar::{
    SegmentMeasure, TabBar, TabBarHits, TabBarLayout, TabChrome, TabFrame, TabMeasure,
};
use crate::theme::Theme;

/// Per-tab padding (left + right) inside the tab background fill.
const TAB_PAD: f64 = 14.0;
/// Gap between the tab label and the close glyph.
const TAB_INNER_GAP: f64 = 10.0;
/// Gap between adjacent tabs.
const TAB_OUTER_GAP: f64 = 1.0;
/// Gap between a tab's icon glyph (see [`crate::TabIcon`]) and its label.
pub(crate) const TAB_ICON_GAP: f64 = 6.0;
/// Height of the active-tab top-edge accent line — VS Code Dark Modern's
/// `tab.activeBorderTop` (#620).
const TAB_ACTIVE_BORDER_TOP_PX: f64 = 1.0;

/// The Nerd-Font-swapped variant of the tab bar's label font, used to
/// measure and paint [`crate::TabIcon`] glyphs (#620). Icon glyphs share
/// the label's size/weight/style but swap in an icon family, matching the
/// activity bar's glyph-sourcing convention — the UI font itself typically
/// isn't patched with icon codepoints.
///
/// Shared by [`draw_tab_bar_icons`] and `GtkBackend::tab_bar_layout_icons`
/// so the no-paint measurement can't drift from the painted glyph.
pub(crate) fn tab_icon_font(base: &pango::FontDescription) -> pango::FontDescription {
    let mut f = base.clone();
    f.set_family("Symbols Nerd Font, monospace");
    f
}

/// Per-tab extra width (px) reserved for an icon glyph + [`TAB_ICON_GAP`],
/// indexed like `bar.tabs`. `0.0` for every tab without an icon, so
/// icon-less tabs keep byte-identical width and hit-test geometry to the
/// pre-#620 rasteriser.
///
/// Mutates `pango_layout`'s font description + text as it measures; the
/// caller re-sets both before its next use (both call sites do).
pub(crate) fn tab_icon_extras(
    pango_layout: &pango::Layout,
    icon_font: &pango::FontDescription,
    tab_count: usize,
    icons: &[Option<crate::primitives::tab_bar::TabIcon>],
) -> Vec<f64> {
    (0..tab_count)
        .map(
            |i| match crate::primitives::tab_bar::tab_icon_at(icons, i) {
                Some(icon) => {
                    pango_layout.set_font_description(Some(icon_font));
                    pango_layout.set_text(&icon.glyph);
                    let (icon_w, _) = pango_layout.pixel_size();
                    icon_w as f64 + TAB_ICON_GAP
                }
                None => 0.0,
            },
        )
        .collect()
}

/// Draw a [`TabBar`] into `(x_offset, y_offset, width, row_height)` on `cr`.
/// Caller is responsible for setting the desired UI font on `layout`
/// *before* calling — the rasteriser uses
/// [`pango::Layout::font_description`] as the base font and toggles
/// to a Pango Italic variant for preview tabs.
///
/// `x_offset` is the left edge of the tab bar in Cairo surface
/// coordinates. Every background fill, tab rectangle, close-button
/// hover, and text `move_to` is offset by this value, so the rasteriser
/// paints into the correct column without the caller needing to
/// pre-translate the Cairo context.
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
/// Returns `(hits, layout)`. `hits` reports its regions in
/// **target-surface (absolute) coordinates** — the same coordinate space
/// as raw click `x` values. `x_offset` is applied internally, so callers
/// compare click positions directly against the returned
/// `slot_positions`, `close_bounds`, and `right_segment_bounds` without
/// any further adjustment. `layout` is the underlying [`TabBarLayout`]
/// this function computed to paint and derive `hits` from — its own
/// `visible_tabs`/`close_bounds` rects are **bar-relative** (origin at
/// `(0, 0)`, *not* `x_offset`/`y_offset`); `GtkBackend::draw_tab_bar`
/// caches it per `WidgetId` for `GtkDriver::tab_center`/`tab_close_center`
/// (quadraui#594), which add the bar's own screen-space origin back in.
///
/// # Visual contract
///
/// - **Tab row height:** caller-provided via `row_height`.
/// - **Active tab:** `theme.tab_active_bg` background, plus a 1 px accent
///   line at the top edge when [`TabBar::active_accent`] is `Some` — `None`
///   paints no accent at all, matching the TUI and macOS rasterisers.
/// - **Dirty tab:** close glyph is `●` (in `theme.foreground`)
///   instead of `×`.
/// - **Preview tab:** italicised label.
/// - **Right segments:** painted in `tab_inactive_fg` (or
///   `tab_active_fg` when `seg.is_active`), no bold.
#[allow(clippy::too_many_arguments)]
pub fn draw_tab_bar(
    cr: &Context,
    pango_layout: &pango::Layout,
    x_offset: f64,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    theme: &Theme,
    hovered_close_tab: Option<usize>,
) -> (TabBarHits, TabBarLayout) {
    draw_tab_bar_icons(
        cr,
        pango_layout,
        x_offset,
        width,
        line_height,
        y_offset,
        row_height,
        bar,
        &[],
        theme,
        hovered_close_tab,
    )
}

/// [`draw_tab_bar`] plus per-tab icon glyphs (#620).
///
/// `icons` is a sidecar slice parallel to `bar.tabs` (see
/// [`crate::Backend::draw_tab_bar_icons`]) — entry `i` decorates tab
/// `i`, a `None` or missing entry means "no icon", and `&[]` reproduces
/// [`draw_tab_bar`] pixel for pixel.
///
/// Each decorated tab reserves `pango(glyph) + TAB_ICON_GAP` extra
/// pixels ahead of its label and paints the glyph in
/// [`crate::TabIcon::color`], independent of the tab's active/inactive
/// foreground. The reservation feeds the same [`crate::TabMeasure`] the
/// layout is built from, so close-button bounds and slot positions stay
/// on the glyphs the user sees.
///
/// Icon glyphs share the label's size/weight/style but swap in a Nerd
/// Font family, matching the activity bar's glyph-sourcing convention —
/// the UI font itself typically isn't patched with icon codepoints.
#[allow(clippy::too_many_arguments)]
pub fn draw_tab_bar_icons(
    cr: &Context,
    pango_layout: &pango::Layout,
    x_offset: f64,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    icons: &[Option<crate::primitives::tab_bar::TabIcon>],
    theme: &Theme,
    hovered_close_tab: Option<usize>,
) -> (TabBarHits, TabBarLayout) {
    draw_tab_bar_icons_with_chrome(
        cr,
        pango_layout,
        x_offset,
        width,
        line_height,
        y_offset,
        row_height,
        bar,
        icons,
        &TabChrome::default(),
        theme,
        hovered_close_tab,
    )
}

/// [`draw_tab_bar`] with an explicit [`TabChrome`] request (#631).
///
/// `&[]` icons + [`TabChrome::default`] reproduces [`draw_tab_bar`] pixel
/// for pixel.
#[allow(clippy::too_many_arguments)]
pub fn draw_tab_bar_with_chrome(
    cr: &Context,
    pango_layout: &pango::Layout,
    x_offset: f64,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    chrome: &TabChrome,
    theme: &Theme,
    hovered_close_tab: Option<usize>,
) -> (TabBarHits, TabBarLayout) {
    draw_tab_bar_icons_with_chrome(
        cr,
        pango_layout,
        x_offset,
        width,
        line_height,
        y_offset,
        row_height,
        bar,
        &[],
        chrome,
        theme,
        hovered_close_tab,
    )
}

/// [`draw_tab_bar_icons`] plus an explicit [`TabChrome`] request (#631).
///
/// When `chrome.active_frame` is [`TabFrame::Brackets`], the active tab's
/// full content — icon, label, and close glyph — is enclosed in literal
/// `[` / `]` glyphs, measured via Pango like every other glyph this
/// rasteriser paints. Mirrors `tui::tab_bar::draw_tab_bar_icons_with_chrome`
/// so the two backends agree on what "enclosing" means (#631).
#[allow(clippy::too_many_arguments)]
pub fn draw_tab_bar_icons_with_chrome(
    cr: &Context,
    pango_layout: &pango::Layout,
    x_offset: f64,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    icons: &[Option<crate::primitives::tab_bar::TabIcon>],
    chrome: &TabChrome,
    theme: &Theme,
    hovered_close_tab: Option<usize>,
) -> (TabBarHits, TabBarLayout) {
    let tab_row_height = row_height;
    let text_y_offset = y_offset + (tab_row_height - line_height) / 2.0;

    let tab_pad = if bar.compact { 2.0 } else { TAB_PAD };
    let tab_inner_gap = if bar.compact { 4.0 } else { TAB_INNER_GAP };
    let tab_outer_gap = if bar.compact { 0.0 } else { TAB_OUTER_GAP };

    // Tab bar background.
    set_source(cr, theme.tab_bar_bg);
    cr.rectangle(x_offset, y_offset, width, tab_row_height);
    cr.fill().ok();

    pango_layout.set_attributes(None);
    let saved_font = pango_layout.font_description().unwrap_or_default();
    let normal_font = saved_font.clone();
    let mut italic_font = normal_font.clone();
    italic_font.set_style(pango::Style::Italic);

    // ── Pre-measure close glyph ─────────────────────────────────────
    // Measure the × glyph width once; individual tabs use it conditionally
    // based on `bar.show_tab_close && tab.is_closable`.
    let close_glyph_w = if bar.show_tab_close {
        pango_layout.set_font_description(Some(&normal_font));
        pango_layout.set_text("×");
        let (w, _) = pango_layout.pixel_size();
        w as f64
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

    let icon_font = tab_icon_font(&normal_font);
    let tab_icon_extras = tab_icon_extras(pango_layout, &icon_font, bar.tabs.len(), icons);

    // ── Pre-measure bracket glyphs (#631) ───────────────────────────
    // Only needed when chrome requests `TabFrame::Brackets`; measured via
    // Pango like every other glyph here so the reservation always matches
    // what actually paints, on any font.
    let brackets = matches!(chrome.active_frame, TabFrame::Brackets);
    let (bracket_open_w, bracket_close_w) = if brackets {
        pango_layout.set_font_description(Some(&normal_font));
        pango_layout.set_text("[");
        let (ow, _) = pango_layout.pixel_size();
        pango_layout.set_text("]");
        let (cw, _) = pango_layout.pixel_size();
        (ow as f64, cw as f64)
    } else {
        (0.0, 0.0)
    };

    let measure_tab = |i: usize| -> TabMeasure {
        let name_w = tab_name_widths[i] as f32;
        let icon_extra = tab_icon_extras[i] as f32;
        // Per-tab closability: only reserve space for the × glyph when both
        // `show_tab_close` (bar-level) and `is_closable` (tab-level) are set.
        let has_close = bar.show_tab_close && bar.tabs[i].is_closable;
        // #631: the active tab's bracket frame, if requested.
        let is_bracket = brackets && bar.tabs[i].is_active;
        let tab_close_extra = if has_close {
            tab_inner_gap + close_glyph_w
        } else {
            0.0
        };
        let bracket_extra = if is_bracket {
            bracket_open_w + bracket_close_w
        } else {
            0.0
        };
        let total = tab_pad as f32
            + bracket_extra as f32
            + icon_extra
            + name_w
            + tab_close_extra as f32
            + tab_pad as f32
            + tab_outer_gap as f32;
        if is_bracket && has_close {
            // The close region covers just the glyph (no trailing
            // padding/gap) so `close_bounds` lands on `×`, not the `]`
            // and margin that follow it.
            let close_w = (tab_inner_gap + close_glyph_w) as f32;
            let trailing_w = (bracket_close_w + tab_pad + tab_outer_gap) as f32;
            TabMeasure::new(total, close_w).with_trailing(trailing_w)
        } else if has_close {
            let close_w = (tab_inner_gap + close_glyph_w + tab_pad + tab_outer_gap) as f32;
            TabMeasure::new(total, close_w)
        } else {
            TabMeasure::new(total, 0.0)
        }
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
        cr.rectangle(x_offset + tab_x, y_offset, tab_visual_w, tab_row_height);
        cr.fill().ok();

        // Top accent line for the active tab — VS Code Dark Modern's
        // `tab.activeBorderTop` (#620). `bar.active_accent` is `None` = no
        // accent, matching the TUI (`tui/tab_bar.rs`) and macOS
        // (`macos/tab_bar.rs`) rasterisers and the field's own doc comment
        // ("`None` = no underline accent (typical for inactive groups)",
        // `primitives/tab_bar.rs`). Callers that want VS Code's focused-tab
        // top border opt in explicitly with
        // `active_accent: Some(theme.tab_active_border_top())`; a bar with no
        // notion of focus (bottom-panel tab strip, terminal toolbar,
        // unfocused splits) passes `None` and gets no strip, same as every
        // other backend.
        if tab.is_active {
            if let Some(accent) = bar.active_accent {
                set_source(cr, accent);
                cr.rectangle(
                    x_offset + tab_x,
                    y_offset,
                    tab_visual_w,
                    TAB_ACTIVE_BORDER_TOP_PX,
                );
                cr.fill().ok();
            }
        }

        // Tab text colour, computed early so the bracket frame (if any)
        // paints in the same colour as the label it encloses.
        let fg_col = match (tab.is_active, tab.is_preview) {
            (true, true) => theme.tab_preview_active_fg,
            (true, false) => theme.tab_active_fg,
            (false, true) => theme.tab_preview_inactive_fg,
            (false, false) => theme.tab_inactive_fg,
        };

        // #631: opening bracket, when this tab is active and chrome
        // requests `TabFrame::Brackets`. `content_pad` shifts every
        // subsequent paint position right by the bracket's width.
        let is_bracket = brackets && tab.is_active;
        let content_pad = tab_pad + if is_bracket { bracket_open_w } else { 0.0 };
        if is_bracket {
            set_source(cr, fg_col);
            pango_layout.set_font_description(Some(&normal_font));
            pango_layout.set_text("[");
            cr.move_to(x_offset + tab_x + tab_pad, text_y_offset);
            super::painted_text::show_layout(cr, pango_layout);
        }

        // Icon glyph, if this tab has one — painted before the label in
        // its own colour, independent of the tab's active/inactive fg.
        let icon_extra = tab_icon_extras[vt.tab_idx];
        if let Some(icon) = crate::primitives::tab_bar::tab_icon_at(icons, vt.tab_idx) {
            set_source(cr, icon.color);
            pango_layout.set_font_description(Some(&icon_font));
            pango_layout.set_text(&icon.glyph);
            cr.move_to(x_offset + tab_x + content_pad, text_y_offset);
            super::painted_text::show_layout(cr, pango_layout);
        }

        // Tab text.
        set_source(cr, fg_col);
        pango_layout.set_font_description(Some(if tab.is_preview {
            &italic_font
        } else {
            &normal_font
        }));
        pango_layout.set_text(&tab.label);
        cr.move_to(x_offset + tab_x + content_pad + icon_extra, text_y_offset);
        super::painted_text::show_layout(cr, pango_layout);

        // #631: closing bracket, for a bracket-framed tab with no close
        // button — painted right after the label. (When there *is* a
        // close button, the closing bracket paints after the glyph,
        // inside the block below, since its position depends on the
        // glyph's own measured width.)
        if is_bracket && !(bar.show_tab_close && tab.is_closable) {
            set_source(cr, fg_col);
            pango_layout.set_font_description(Some(&normal_font));
            pango_layout.set_text("]");
            cr.move_to(
                x_offset + tab_x + content_pad + icon_extra + tab_name_widths[vt.tab_idx],
                text_y_offset,
            );
            super::painted_text::show_layout(cr, pango_layout);
        }

        // Paint the close glyph only when both the bar-level flag and the
        // per-tab `is_closable` flag are set — matching the measurement above.
        if bar.show_tab_close && tab.is_closable {
            if let Some(cb) = vt.close_bounds {
                let close_x = cb.x as f64 + tab_inner_gap;
                let is_close_hovered = hovered_close_tab == Some(vt.tab_idx);

                // Rounded hover background behind close glyph.
                if is_close_hovered {
                    let pad = 2.0;
                    let rx = x_offset + close_x - pad;
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
                cr.move_to(x_offset + close_x, text_y_offset);
                super::painted_text::show_layout(cr, pango_layout);

                // #631: closing bracket, right after the glyph — `cb.x`
                // already stops short of it via
                // `TabMeasure::trailing_width`, so this lands exactly
                // where the reservation left room for it.
                if is_bracket {
                    set_source(cr, fg_col);
                    pango_layout.set_font_description(Some(&normal_font));
                    pango_layout.set_text("]");
                    cr.move_to(x_offset + close_x + close_glyph_w, text_y_offset);
                    super::painted_text::show_layout(cr, pango_layout);
                }
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
        cr.move_to(x_offset + vs.bounds.x as f64, text_y_offset);
        super::painted_text::show_layout(cr, pango_layout);
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
            .map(|i| {
                // Use per-tab close_extra to match the measurement in measure_tab.
                let has_close = bar.show_tab_close && bar.tabs[i].is_closable;
                let per_tab_close_extra = if has_close {
                    tab_inner_gap + close_glyph_w
                } else {
                    0.0
                };
                tab_name_widths[i]
                    + tab_icon_extras[i]
                    + tab_pad * 2.0
                    + per_tab_close_extra
                    + tab_outer_gap
            })
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
    // `tab_bar_layout_to_hits` yields bar-relative positions, but the
    // `TabBarHits` contract is target-surface coordinates (matching the TUI
    // rasteriser and the painted glyphs above, which all add `x_offset`).
    // Shift every hit range so consumers can hit-test against raw click x.
    // Shared with `Backend::tab_bar_layout` so the paint and no-paint
    // paths cannot drift apart again (issue #552).
    crate::backend::shift_tab_bar_hits(&mut hits, x_offset);
    hits.correct_scroll_offset = correct_scroll_offset;
    hits.available_cols = available_cols;
    (hits, layout)
}

// ── Tests ──────────────────────────────────────────────────────────────────
//
// Headless paint test: verify that `draw_tab_bar` respects `x_offset` and
// never paints tab chrome into columns left of that offset.
//
// Uses a Cairo `ImageSurface` (no display required) and reads back pixel
// data directly. The test is gated on the `gtk` feature so it only runs
// under `cargo test --features gtk`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tab_bar::{TabBar, TabBarSegment, TabItem};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    // Surface dimensions: wide enough to contain the tab bar at x_offset=50,
    // tall enough for a single row.
    const W: i32 = 300;
    const ROW_H: i32 = 24;
    const LINE_H: f64 = 14.0;
    const X_OFFSET: f64 = 50.0;

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    ///
    /// Cairo's `ARgb32` stores each pixel as four bytes in native
    /// (little-endian) byte order: [B, G, R, A].  The `stride` from
    /// [`ImageSurface::stride`] is in bytes and may include padding.
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        // ARgb32 byte layout on little-endian: B=off+0, G=off+1, R=off+2
        (data[off + 2], data[off + 1], data[off])
    }

    fn make_bar() -> TabBar {
        TabBar {
            id: WidgetId::new("test-tabs"),
            tabs: vec![TabItem {
                label: "main.rs".to_string(),
                is_active: true,
                is_dirty: false,
                is_preview: false,
                is_closable: true,
            }],
            scroll_offset: 0,
            right_segments: vec![TabBarSegment {
                text: " ⇅ ".to_string(),
                width_cells: 3,
                id: None,
                is_active: false,
            }],
            active_accent: None,
            // Disable close button so the test doesn't depend on
            // close-glyph measurement.
            show_tab_close: false,
            compact: false,
        }
    }

    /// Theme with a tab_bar_bg that is clearly distinct from white so we
    /// can distinguish "untouched background" from "painted tab chrome".
    fn make_theme() -> Theme {
        Theme {
            tab_bar_bg: Color::rgb(40, 44, 52),
            tab_active_bg: Color::rgb(60, 66, 80),
            ..Theme::default()
        }
    }

    /// Paint a tab bar into a fresh white surface at `x_offset = X_OFFSET`
    /// and return the surface.
    fn paint_at_offset() -> ImageSurface {
        let surface = ImageSurface::create(Format::ARgb32, W, ROW_H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            // Fill with white so any pixel left untouched is clearly white.
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let bar = make_bar();
            let theme = make_theme();
            draw_tab_bar(
                &cr,
                &pango_layout,
                X_OFFSET,
                (W as f64) - X_OFFSET,
                LINE_H,
                0.0,
                ROW_H as f64,
                &bar,
                &theme,
                None,
            );
        }
        surface
    }

    /// Columns 0..X_OFFSET-1 must be untouched white — the rasteriser must
    /// not paint any tab chrome left of `x_offset`.
    #[test]
    fn tab_bar_does_not_paint_before_x_offset() {
        let mut surface = paint_at_offset();
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let mid_y = ROW_H / 2;
        for x in 0..(X_OFFSET as i32) {
            let px = pixel(&data, stride, x, mid_y);
            assert_eq!(
                px,
                (255, 255, 255),
                "pixel at x={x} should be untouched white, got {px:?}"
            );
        }
    }

    /// Column X_OFFSET must show the tab bar background fill — confirming
    /// the rasteriser starts painting exactly at `x_offset`.
    #[test]
    fn tab_bar_starts_painting_at_x_offset() {
        let mut surface = paint_at_offset();
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let mid_y = ROW_H / 2;
        // x = X_OFFSET is the first pixel of the tab bar; it must differ
        // from the white fill we applied before painting.
        let px = pixel(&data, stride, X_OFFSET as i32, mid_y);
        assert_ne!(
            px,
            (255, 255, 255),
            "pixel at x={X_OFFSET} should be tab bar chrome, not white background"
        );
    }

    /// #620: a tab with an [`crate::primitives::tab_bar::TabIcon`] must
    /// reserve extra width over an otherwise-identical icon-less tab, and
    /// its close button must still resolve to a `TabClose` hit at the
    /// close bounds the layout reports (the icon reservation must not
    /// disturb close-glyph hit-test geometry).
    #[test]
    fn icon_reserves_extra_width_and_close_still_hit_tests() {
        use crate::primitives::tab_bar::{TabBarHit, TabIcon};

        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![TabItem {
                label: "main.rs".to_string(),
                is_active: true,
                ..Default::default()
            }],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };

        let surface = ImageSurface::create(Format::ARgb32, W, ROW_H).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);
        let theme = make_theme();

        // Same bar painted twice — only the icon sidecar differs, so any
        // geometry delta below is attributable to the icon reservation.
        let (_, no_icon_layout) = draw_tab_bar(
            &cr,
            &pango_layout,
            0.0,
            W as f64,
            LINE_H,
            0.0,
            ROW_H as f64,
            &bar,
            &theme,
            None,
        );

        let icons = vec![Some(TabIcon {
            glyph: "\u{f09b}".to_string(),
            color: Color::rgb(240, 150, 60),
        })];
        let (icon_hits, icon_layout) = draw_tab_bar_icons(
            &cr,
            &pango_layout,
            0.0,
            W as f64,
            LINE_H,
            0.0,
            ROW_H as f64,
            &bar,
            &icons,
            &theme,
            None,
        );

        let no_icon_w = no_icon_layout.visible_tabs[0].bounds.width;
        let icon_w = icon_layout.visible_tabs[0].bounds.width;
        assert!(
            icon_w > no_icon_w,
            "tab with an icon should reserve more width than an otherwise-identical \
             icon-less tab: {icon_w} vs {no_icon_w}"
        );

        let close_bounds = icon_layout.visible_tabs[0]
            .close_bounds
            .expect("icon tab should still have a close button reserved");
        let click_x = close_bounds.x + close_bounds.width / 2.0;
        let click_y = close_bounds.y + close_bounds.height / 2.0;
        match icon_layout.hit_test(click_x, click_y) {
            TabBarHit::TabClose(0) => {}
            other => panic!("expected TabClose(0) at close bounds, got {other:?}"),
        }
        assert!(
            icon_hits.close_bounds[0].is_some(),
            "TabBarHits should also report the close bounds for the icon tab"
        );
    }

    /// #620: when a bar opts in with `active_accent: Some(colour)`, the
    /// active tab's top row of pixels must be that colour — VS Code Dark
    /// Modern's `tab.activeBorderTop`. The row immediately below must NOT
    /// be that colour, confirming the accent is a thin top strip, not the
    /// whole active-tab background.
    #[test]
    fn active_tab_top_row_is_accent_when_opted_in() {
        let accent = Color::rgb(90, 170, 255);
        let surface = ImageSurface::create(Format::ARgb32, W, ROW_H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            let mut bar = make_bar();
            bar.active_accent = Some(accent);
            let theme = make_theme();
            draw_tab_bar(
                &cr,
                &pango_layout,
                0.0,
                W as f64,
                LINE_H,
                0.0,
                ROW_H as f64,
                &bar,
                &theme,
                None,
            );
        }
        let mut surface = surface;
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let top = pixel(&data, stride, 5, 0);
        assert_eq!(
            top,
            (accent.r, accent.g, accent.b),
            "top row of the active tab should be the opted-in active_accent colour"
        );

        let below = pixel(&data, stride, 5, 3);
        assert_ne!(
            below,
            (accent.r, accent.g, accent.b),
            "a few rows below the top accent should be the tab body, not the accent colour"
        );
    }

    /// #620 review follow-up: `active_accent: None` must paint no accent
    /// strip at all — the pre-#620 behaviour, and the contract the TUI
    /// (`tui::tab_bar::draw_tab_bar`) and macOS (`macos::tab_bar`)
    /// rasterisers already implement. Regressing this would mean every
    /// vimcode caller that passes `None` on purpose (bottom-panel tab
    /// strip, terminal toolbar, any unfocused split) starts painting a
    /// theme-default accent it never asked for.
    #[test]
    fn active_tab_top_row_has_no_accent_when_none() {
        let surface = ImageSurface::create(Format::ARgb32, W, ROW_H).expect("create ImageSurface");
        let theme = make_theme();
        {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            let bar = make_bar();
            assert_eq!(bar.active_accent, None, "test fixture must exercise None");
            draw_tab_bar(
                &cr,
                &pango_layout,
                0.0,
                W as f64,
                LINE_H,
                0.0,
                ROW_H as f64,
                &bar,
                &theme,
                None,
            );
        }
        let mut surface = surface;
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let border_top = theme.tab_active_border_top();
        let active_bg = theme.tab_active_bg;
        let top = pixel(&data, stride, 5, 0);
        assert_ne!(
            top,
            (border_top.r, border_top.g, border_top.b),
            "with active_accent: None, the top row must NOT be theme.tab_active_border_top()"
        );
        assert_eq!(
            top,
            (active_bg.r, active_bg.g, active_bg.b),
            "with active_accent: None, the top row should just be the active tab's \
             background — no accent strip painted over it"
        );
    }

    /// #631: `TabFrame::Brackets` must widen the active tab (room for `[`
    /// and `]`) and shift its close-button hit region right of the
    /// leading bracket — while `close_bounds`' own *width* stays exactly
    /// the glyph's, so a click still resolves to `TabClose`, not `Tab`.
    #[test]
    fn bracket_frame_widens_active_tab_and_close_bounds_excludes_the_bracket() {
        use crate::primitives::tab_bar::{TabBarHit, TabChrome, TabFrame};

        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![TabItem {
                label: "main.rs".to_string(),
                is_active: true,
                ..Default::default()
            }],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };

        let surface = ImageSurface::create(Format::ARgb32, W, ROW_H).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);
        let theme = make_theme();

        let (_, plain_layout) = draw_tab_bar(
            &cr,
            &pango_layout,
            0.0,
            W as f64,
            LINE_H,
            0.0,
            ROW_H as f64,
            &bar,
            &theme,
            None,
        );

        let chrome = TabChrome::new(TabFrame::Brackets);
        let (chrome_hits, chrome_layout) = draw_tab_bar_with_chrome(
            &cr,
            &pango_layout,
            0.0,
            W as f64,
            LINE_H,
            0.0,
            ROW_H as f64,
            &bar,
            &chrome,
            &theme,
            None,
        );

        let plain_w = plain_layout.visible_tabs[0].bounds.width;
        let chrome_w = chrome_layout.visible_tabs[0].bounds.width;
        assert!(
            chrome_w > plain_w,
            "bracket framing should widen the active tab: {chrome_w} vs {plain_w}"
        );

        let plain_close = plain_layout.visible_tabs[0]
            .close_bounds
            .expect("plain tab should still report close bounds");
        let chrome_close = chrome_layout.visible_tabs[0]
            .close_bounds
            .expect("bracket-framed tab should still report close bounds");
        // The plain close region bundles the trailing padding + outer gap
        // (it's flush against the tab's right edge); the bracket-framed
        // region excludes that trailing chrome via `TabMeasure::trailing_width`
        // instead, so it's narrower by exactly `TAB_PAD + TAB_OUTER_GAP`.
        assert!(
            (plain_close.width - chrome_close.width - (TAB_PAD as f32 + TAB_OUTER_GAP as f32))
                .abs()
                < 0.01,
            "bracket close region should be narrower than the plain one by exactly the \
             trailing pad + outer gap it no longer bundles: plain={}, chrome={}",
            plain_close.width,
            chrome_close.width
        );
        assert!(
            chrome_close.x > plain_close.x,
            "close region should shift right to make room for the leading bracket"
        );
        let chrome_tab_end =
            chrome_layout.visible_tabs[0].bounds.x + chrome_layout.visible_tabs[0].bounds.width;
        assert!(
            chrome_close.x + chrome_close.width < chrome_tab_end,
            "close bounds must stop before the tab's right edge, leaving room for ']'"
        );

        let click_x = chrome_close.x + chrome_close.width / 2.0;
        let click_y = chrome_close.y + chrome_close.height / 2.0;
        match chrome_layout.hit_test(click_x, click_y) {
            TabBarHit::TabClose(0) => {}
            other => panic!("expected TabClose(0) at close bounds, got {other:?}"),
        }

        assert_eq!(chrome_hits.close_bounds.len(), 1);
        assert!(chrome_hits.close_bounds[0].is_some());
    }
}
