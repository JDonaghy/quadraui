//! macOS rasteriser for [`crate::TabBar`].
//!
//! Mirrors [`crate::gtk::tab_bar::draw_tab_bar`]: measures tab widths
//! via Core Text, lays out left-to-right with active-tab highlighting,
//! close glyphs (× or ● for dirty), and right-aligned segments.
//! Returns a [`TabBarHits`] carrying per-tab + per-segment screen
//! bounds for the caller's click dispatch.
//!
//! ## Scope omissions (follow-up after the #38 chrome batch)
//!
//! - **Italic preview tabs** — needs an italic-variant `CTFont` via
//!   `CTFontCreateCopyWithSymbolicTraits`. Same dependency as bold
//!   support in [`super::status_bar`]; pairs naturally with that
//!   future change. Until it lands, preview tabs render in the
//!   active font.
//! - **Rounded close-button hover background** — the GTK rasteriser
//!   draws a 3 px rounded rect under the close glyph on hover. macOS
//!   uses a simpler approach for #38: tint the close glyph itself
//!   (`theme.foreground`) when hovered. Visual parity with GTK is
//!   tracked separately.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
// `TabBarHits` is `#[deprecated]` (issue #823) — `mac_tab_bar_layout`
// still constructs it directly rather than narrowing it down from a
// `TabBarLayout`, per #504's original audit — so the import needs the
// same allow every use site below does. Issue #919 added
// `mac_tab_bar_native_layout`, a *second*, independently-computed
// function that builds the real `TabBarLayout` `Backend::resolve_tab_bar_layout`
// exposes; see that function's doc for why it duplicates rather than
// shares `mac_tab_bar_layout`'s measurement code.
use crate::event::Rect;
use crate::primitives::tab_bar::{
    tab_icon_at, TabBarHit, TabBarLayout, TabIcon, VisibleSegment, VisibleTab,
};
#[allow(deprecated)]
use crate::primitives::tab_bar::{TabBar, TabBarHits};
use crate::theme::Theme;
use crate::types::Color;

/// Per-tab horizontal padding (left + right) inside the tab background fill.
const TAB_PAD: f64 = 14.0;
/// Gap between the tab label and the close glyph.
const TAB_INNER_GAP: f64 = 10.0;
/// Gap between adjacent tabs.
const TAB_OUTER_GAP: f64 = 1.0;
/// Top-edge accent strip height for the active tab (when `active_accent`
/// is set).
const ACCENT_HEIGHT: f64 = 2.0;
/// 15-char sample used to estimate cell width for `available_cols`.
/// Same string the GTK rasteriser uses, so app-level cell budgets
/// remain comparable between backends.
const CELL_WIDTH_SAMPLE: &str = "ABCDabcd0123.:_";
/// Slack added either side of the close glyph when building its hit box.
const CLOSE_PAD: f64 = 2.0;
/// Gap between a tab's icon glyph (see [`TabIcon`]) and its label (#926).
/// Same value as [`crate::gtk::tab_bar`]'s `TAB_ICON_GAP`, so a decorated
/// tab is the same shape on both pixel backends.
const TAB_ICON_GAP: f64 = 6.0;

/// Per-tab extra width (points) reserved for an icon glyph plus
/// [`TAB_ICON_GAP`], indexed like `bar.tabs`. `0.0` for every tab without
/// an icon, so an icon-less tab keeps byte-identical width and hit-test
/// geometry to the pre-#926 rasteriser (and `&[]` — what [`draw_tab_bar`]
/// and the other icon-less wrappers pass — reproduces it exactly for
/// every tab).
///
/// This is the CoreText icon-width pass #926 was opened for. Every
/// geometry consumer goes through it — [`mac_tab_bar_layout_icons`],
/// [`mac_tab_bar_native_layout_icons`] and [`draw_tab_bar_icons`]'s paint
/// loop — which is what keeps a tab's close-button hit box on the glyph
/// it draws once an icon shifts the label right.
///
/// # Font choice — the current font, not a swapped-in icon family
///
/// Unlike GTK (`gtk::tab_bar::tab_icon_font`, which replaces the family
/// with `Symbols Nerd Font`), macOS measures and paints the glyph with
/// whatever font [`crate::Backend::set_current_font`] installed. That is
/// this backend's established convention for every other glyph it draws —
/// see [`super::activity_bar::draw_activity_bar`]'s doc, which makes the
/// same choice for [`crate::Icon`] — and it is what keeps measure and
/// paint using one font by construction: a family swap here would need
/// the same swap in the paint loop or the reservation would not match the
/// ink. A caller whose font carries no icon codepoints gets tofu of the
/// measured width, not misplaced hit boxes.
pub(crate) fn mac_tab_icon_extras(
    font: &CTFont,
    tab_count: usize,
    icons: &[Option<TabIcon>],
) -> Vec<f64> {
    (0..tab_count)
        .map(|i| match tab_icon_at(icons, i) {
            Some(icon) => measure_text(font, &icon.glyph).0 + TAB_ICON_GAP,
            None => 0.0,
        })
        .collect()
}

/// Compute the [`TabBarHits`] [`draw_tab_bar`] would produce for `bar` at
/// `width`, without painting.
///
/// This is the single measurement path: [`draw_tab_bar`] calls it too and
/// paints from its output, so the no-paint twin backing
/// [`crate::Backend::tab_bar_layout`] cannot drift from what was painted.
/// Since #926 both spellings go through [`mac_tab_bar_layout_icons`] with
/// an empty icon sidecar, which is equivalent point for point — pass a
/// real sidecar to that function to decorate tabs.
///
/// # Coordinate space — bar-relative here; callers shift to absolute
///
/// This function itself returns **bar-relative** `x` (slots start at
/// `0.0`) and is never handed `rect.x` — same shape as
/// [`mac_tab_bar_native_layout_icons`] below. The
/// [`crate::Backend::tab_bar_layout`] doc pins the `TabBarHits` *contract*
/// at target-surface (absolute) coordinates though, so every caller that
/// owes it — [`crate::macos::MacBackend::draw_tab_bar_icons`]'s paint path
/// and [`crate::macos::MacBackend::tab_bar_layout_icons`]'s no-paint twin —
/// runs this function's output through
/// [`crate::backend::shift_tab_bar_hits`] with `rect.x` (issue #552 /
/// #934), the same TUI / GTK convention. `MacBackend::draw_tab_bar_icons`
/// additionally wraps its CGContext in `CGContextTranslateCTM(ctx, rect.x,
/// rect.y)` before calling [`draw_tab_bar_icons`] below, so the ink lands
/// at the shifted hits' position too — mirroring
/// `GtkBackend::draw_activity_bar`'s `cr.translate` and this crate's own
/// `MacBackend::draw_activity_bar`. What this function guarantees is the
/// invariant that is actually load-bearing: `tab_bar_layout` returns
/// exactly what `draw_tab_bar` painted, once both are shifted the same way.
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub fn mac_tab_bar_layout(font: &CTFont, width: f64, bar: &TabBar) -> TabBarHits {
    mac_tab_bar_layout_icons(font, width, bar, &[])
}

/// [`mac_tab_bar_layout`] with a per-tab icon sidecar (#926).
///
/// `icons` is parallel to `bar.tabs` (see
/// [`crate::Backend::tab_bar_layout_icons`]) — entry `i` decorates tab
/// `i`, a `None` or missing entry means "no icon", and `&[]` reproduces
/// [`mac_tab_bar_layout`] point for point. Each decorated tab reserves
/// [`mac_tab_icon_extras`]' width ahead of its label, which shifts that
/// tab's label *and* its close-glyph hit box right by the same amount —
/// the reason the reservation has to live here, in the one measurement
/// path [`draw_tab_bar_icons`] paints from, rather than in the paint loop
/// alone.
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub fn mac_tab_bar_layout_icons(
    font: &CTFont,
    width: f64,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
) -> TabBarHits {
    let tab_pad = if bar.compact { 2.0 } else { TAB_PAD };
    let tab_inner_gap = if bar.compact { 4.0 } else { TAB_INNER_GAP };
    let tab_outer_gap = if bar.compact { 0.0 } else { TAB_OUTER_GAP };

    // ── Right-segment widths (reserved before tabs get their budget) ──
    let right_widths: Vec<f64> = bar
        .right_segments
        .iter()
        .map(|seg| measure_text(font, &seg.text).0)
        .collect();
    let reserved_px: f64 = right_widths.iter().sum();
    let effective_tab_area = (width - reserved_px).max(0.0);

    // Close-glyph width measured once — individual tabs use it
    // conditionally based on `bar.show_tab_close && tab.is_closable`. The
    // `●` dirty variant is the same width in Menlo and most monospace
    // fonts.
    let close_w = if bar.show_tab_close {
        measure_text(font, "×").0
    } else {
        0.0
    };
    let close_extra_for = |tab_idx: usize| -> f64 {
        if bar.show_tab_close && bar.tabs[tab_idx].is_closable {
            tab_inner_gap + close_w
        } else {
            0.0
        }
    };

    // #926's CoreText icon-width pass. Measured once here and reused by
    // every width/x expression below, so a decorated tab's label, close
    // box and slot edge all shift by the same amount.
    let icon_extras = mac_tab_icon_extras(font, bar.tabs.len(), icons);

    // Pre-measure every tab's full slot width — used for scroll-offset
    // resolution.
    let tab_slot_widths: Vec<f64> = bar
        .tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            let (name_w, _) = measure_text(font, &tab.label);
            tab_pad + icon_extras[i] + name_w + close_extra_for(i) + tab_pad + tab_outer_gap
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

    // ── Slot geometry ────────────────────────────────────────────────
    let mut slot_positions: Vec<(f64, f64)> = Vec::with_capacity(bar.tabs.len());
    let mut close_bounds: Vec<Option<(f64, f64)>> = Vec::with_capacity(bar.tabs.len());
    for _ in 0..bar.scroll_offset.min(bar.tabs.len()) {
        slot_positions.push((0.0, 0.0));
        close_bounds.push(None);
    }

    let mut x = 0.0_f64;
    for (tab_idx, tab) in bar.tabs.iter().enumerate().skip(bar.scroll_offset) {
        let (tab_name_w, _) = measure_text(font, &tab.label);
        let icon_extra = icon_extras[tab_idx];
        let tab_content_w = tab_pad + icon_extra + tab_name_w + close_extra_for(tab_idx) + tab_pad;
        let slot_w = tab_content_w + tab_outer_gap;
        if x + slot_w > effective_tab_area {
            break;
        }
        slot_positions.push((x, x + slot_w));

        if bar.show_tab_close && tab.is_closable {
            let close_x = x + tab_pad + icon_extra + tab_name_w + tab_inner_gap;
            close_bounds.push(Some((close_x - CLOSE_PAD, close_x + close_w + CLOSE_PAD)));
        } else {
            close_bounds.push(None);
        }

        x += slot_w;
    }

    // ── Right segments ───────────────────────────────────────────────
    let mut right_segment_bounds: Vec<(f64, f64)> = Vec::with_capacity(right_widths.len());
    let mut sx = width - reserved_px;
    for seg_w in &right_widths {
        right_segment_bounds.push((sx, sx + seg_w));
        sx += seg_w;
    }

    // Cell-width estimation: 15-char sample width / 15 → average glyph
    // advance. Matches the GTK convention so app-level char-col math
    // doesn't diverge across backends.
    let (sample_px, _) = measure_text(font, CELL_WIDTH_SAMPLE);
    let char_w = (sample_px / CELL_WIDTH_SAMPLE.chars().count() as f64).max(1.0);
    let available_cols = (effective_tab_area / char_w).floor().max(0.0) as usize;

    TabBarHits {
        slot_positions,
        close_bounds,
        right_segment_bounds,
        available_cols,
        correct_scroll_offset,
    }
}

/// Compute the [`TabBarLayout`] [`draw_tab_bar`] paints, without
/// painting — issue #919's `TabBarLayout`-returning counterpart to
/// [`mac_tab_bar_layout`], backing [`crate::Backend::resolve_tab_bar_layout`].
///
/// # Why this duplicates `mac_tab_bar_layout` instead of sharing it
///
/// Every other backend derives its `TabBarHits` by calling
/// [`crate::TabBar::layout`] (the shared D6 layout API) and narrowing the
/// resulting `TabBarLayout` down via
/// [`crate::backend::tab_bar_hits_from_layout`]. macOS never adopted that
/// path (#504's audit) — `mac_tab_bar_layout` above measures and
/// positions tabs by hand, one `f64` tuple at a time, with no
/// intermediate `TabBarLayout` to source native `Rect` coordinates from.
/// Retrofitting `mac_tab_bar_layout` onto the shared layout API is real,
/// independent rasteriser work (out of scope for this additive-only
/// issue — see its "Scope" section), so this function instead mirrors
/// `mac_tab_bar_layout`'s measurement expressions line-for-line and
/// builds a `TabBarLayout` directly. `native_layout_agrees_with_hits_layout`
/// (below) pins the two against each other so they cannot silently
/// drift apart.
///
/// # Coordinate space — bar-relative, matching `mac_tab_bar_layout`
///
/// Same x-axis convention as `mac_tab_bar_layout` (see its doc): macOS
/// paints tabs from `x = 0`, never shifted by `rect.x`, so both this
/// and that function are already bar-relative in x — no `TabBarHits`
/// absolute-coordinate divergence to account for. `height` (the caller's
/// `rect.height`) becomes every tab's `bounds.height`, and every
/// `bounds.y` is `0.0` — [`TabBarLayout`]'s own documented convention
/// (origin at the bar's top-left).
pub fn mac_tab_bar_native_layout(
    font: &CTFont,
    width: f64,
    height: f64,
    bar: &TabBar,
) -> TabBarLayout {
    mac_tab_bar_native_layout_icons(font, width, height, bar, &[])
}

/// [`mac_tab_bar_native_layout`] with a per-tab icon sidecar (#926) — the
/// `TabBarLayout`-returning twin of [`mac_tab_bar_layout_icons`], backing
/// [`crate::Backend::resolve_tab_bar_layout_icons`].
///
/// `icons` follows the same convention as [`mac_tab_bar_layout_icons`]'
/// (`&[]` reproduces the icon-less geometry point for point), and both
/// functions source their reservation from the same
/// [`mac_tab_icon_extras`] pass, so `native_layout_agrees_with_hits_layout`
/// keeps holding with icons present.
pub fn mac_tab_bar_native_layout_icons(
    font: &CTFont,
    width: f64,
    height: f64,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
) -> TabBarLayout {
    let tab_pad = if bar.compact { 2.0 } else { TAB_PAD };
    let tab_inner_gap = if bar.compact { 4.0 } else { TAB_INNER_GAP };
    let tab_outer_gap = if bar.compact { 0.0 } else { TAB_OUTER_GAP };

    // ── Right-segment widths (reserved before tabs get their budget) ──
    let right_widths: Vec<f64> = bar
        .right_segments
        .iter()
        .map(|seg| measure_text(font, &seg.text).0)
        .collect();
    let reserved_px: f64 = right_widths.iter().sum();
    let effective_tab_area = (width - reserved_px).max(0.0);

    let close_w = if bar.show_tab_close {
        measure_text(font, "×").0
    } else {
        0.0
    };
    let close_extra_for = |tab_idx: usize| -> f64 {
        if bar.show_tab_close && bar.tabs[tab_idx].is_closable {
            tab_inner_gap + close_w
        } else {
            0.0
        }
    };

    // #926's icon-width pass — same call, same values as
    // `mac_tab_bar_layout_icons`'.
    let icon_extras = mac_tab_icon_extras(font, bar.tabs.len(), icons);

    // Pre-measure every tab's full slot width — used for scroll-offset
    // resolution, exactly as `mac_tab_bar_layout` does.
    let tab_slot_widths: Vec<f64> = bar
        .tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            let (name_w, _) = measure_text(font, &tab.label);
            tab_pad + icon_extras[i] + name_w + close_extra_for(i) + tab_pad + tab_outer_gap
        })
        .collect();

    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    let resolved_scroll_offset = if let Some(active) = active_idx {
        TabBar::fit_active_scroll_offset(active, bar.tabs.len(), effective_tab_area as usize, |i| {
            tab_slot_widths[i] as usize
        })
    } else {
        bar.scroll_offset
    };

    // ── Visible tabs ─────────────────────────────────────────────────
    // Painted from `bar.scroll_offset` (the caller's value), not the
    // freshly-resolved `resolved_scroll_offset` above — exactly what
    // `mac_tab_bar_layout` does: the "engine feedback" correction is
    // reported back for the *next* frame's caller to apply, not applied
    // to this one. Unlike `TabBarHits`, there is no `(0.0, 0.0)`
    // sentinel padding to keep vector indices aligned — `visible_tabs`
    // is sparse and carries its own `tab_idx`, matching every other
    // backend's `TabBarLayout`.
    let mut visible_tabs: Vec<VisibleTab> = Vec::new();
    let mut close_regions: Vec<(Rect, TabBarHit)> = Vec::new();
    let mut body_regions: Vec<(Rect, TabBarHit)> = Vec::new();

    let mut x = 0.0_f64;
    for (tab_idx, tab) in bar.tabs.iter().enumerate().skip(bar.scroll_offset) {
        let (tab_name_w, _) = measure_text(font, &tab.label);
        let icon_extra = icon_extras[tab_idx];
        let tab_content_w = tab_pad + icon_extra + tab_name_w + close_extra_for(tab_idx) + tab_pad;
        let slot_w = tab_content_w + tab_outer_gap;
        if x + slot_w > effective_tab_area {
            break;
        }
        let bounds = Rect::new(x as f32, 0.0, slot_w as f32, height as f32);

        let close_bounds = if bar.show_tab_close && tab.is_closable {
            let close_x = x + tab_pad + icon_extra + tab_name_w + tab_inner_gap;
            let cb = Rect::new(
                (close_x - CLOSE_PAD) as f32,
                0.0,
                (close_w + 2.0 * CLOSE_PAD) as f32,
                height as f32,
            );
            close_regions.push((cb, TabBarHit::TabClose(tab_idx)));
            Some(cb)
        } else {
            None
        };

        body_regions.push((bounds, TabBarHit::Tab(tab_idx)));
        visible_tabs.push(VisibleTab {
            tab_idx,
            bounds,
            close_bounds,
        });

        x += slot_w;
    }

    // Close regions before body regions — matches `TabBar::layout`'s own
    // hit-region ordering (close-before-body) so `hit_test` returns the
    // more-specific close hit when the pointer is on the × glyph.
    let mut hit_regions: Vec<(Rect, TabBarHit)> =
        Vec::with_capacity(close_regions.len() + body_regions.len());
    hit_regions.extend(close_regions);
    hit_regions.extend(body_regions);

    // ── Right segments ───────────────────────────────────────────────
    let mut visible_segments: Vec<VisibleSegment> = Vec::with_capacity(right_widths.len());
    let mut sx = width - reserved_px;
    for (segment_idx, seg_w) in right_widths.iter().enumerate() {
        let bounds = Rect::new(sx as f32, 0.0, *seg_w as f32, height as f32);
        let seg = &bar.right_segments[segment_idx];
        let clickable = seg.id.is_some();
        if let Some(id) = &seg.id {
            hit_regions.push((bounds, TabBarHit::RightSegment(id.clone())));
        }
        visible_segments.push(VisibleSegment {
            segment_idx,
            bounds,
            clickable,
        });
        sx += seg_w;
    }

    TabBarLayout {
        bar_width: width as f32,
        bar_height: height as f32,
        visible_tabs,
        visible_segments,
        scroll_left: None,
        scroll_right: None,
        hit_regions,
        resolved_scroll_offset,
    }
}

/// Paint `bar` into the rect `(0, y_offset, width, row_height)` on
/// `ctx`. `line_height` is the *text* line height; `row_height` may be
/// larger (callers pad file-tab bars). Returns per-tab + per-segment
/// hit bounds plus the resolved scroll offset.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]). Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub unsafe fn draw_tab_bar(
    ctx: CGContextRef,
    font: &CTFont,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    theme: &Theme,
    hovered_close_tab: Option<usize>,
) -> TabBarHits {
    draw_tab_bar_icons(
        ctx,
        font,
        width,
        line_height,
        y_offset,
        row_height,
        bar,
        theme,
        hovered_close_tab,
        &[],
    )
}

/// [`draw_tab_bar`] plus per-tab icon glyphs (#926, closing the #620
/// follow-up gap on macOS).
///
/// `icons` is a sidecar slice parallel to `bar.tabs` (see
/// [`crate::Backend::draw_tab_bar_icons`]) — entry `i` decorates tab `i`,
/// a `None` or missing entry means "no icon", and `&[]` reproduces
/// [`draw_tab_bar`] pixel for pixel. The glyph paints at the tab's leading
/// edge, just inside the tab padding, in its own [`TabIcon::color`] (an
/// icon keeps its language/identity colour rather than inheriting the
/// tab's active/inactive foreground, matching the TUI and GTK
/// rasterisers); the label and close glyph shift right by exactly the
/// width [`mac_tab_icon_extras`] reserved for it, which is why the
/// returned close-button hit boxes still land on the × the user sees.
///
/// # Coordinate space — paints bar-relative; caller supplies the origin
///
/// Like [`mac_tab_bar_layout_icons`] above, this paints into
/// `(0, y_offset, width, row_height)` in `ctx`'s *current* coordinate
/// space and returns bar-relative `TabBarHits`. It does not take an
/// `x_offset` — [`crate::macos::MacBackend::draw_tab_bar_icons`] instead
/// wraps this call in `CGContextTranslateCTM(ctx, rect.x, rect.y)` (issue
/// #934) so the ink lands at the bar's real screen position, and
/// separately shifts the returned hits by `rect.x` via
/// [`crate::backend::shift_tab_bar_hits`] so they still describe exactly
/// what was painted. Mirrors `MacBackend::draw_activity_bar`'s identical
/// CTM-translate treatment of the (also bar-relative)
/// `super::activity_bar::draw_activity_bar`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call (typical: the frame-scope pointer stashed on [`super::MacBackend`]).
/// Calling with a freed or null pointer is UB.
#[allow(clippy::too_many_arguments)]
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub unsafe fn draw_tab_bar_icons(
    ctx: CGContextRef,
    font: &CTFont,
    width: f64,
    line_height: f64,
    y_offset: f64,
    row_height: f64,
    bar: &TabBar,
    theme: &Theme,
    hovered_close_tab: Option<usize>,
    icons: &[Option<TabIcon>],
) -> TabBarHits {
    let text_y_offset = y_offset + (row_height - line_height) / 2.0;
    let tab_pad = if bar.compact { 2.0 } else { TAB_PAD };
    let tab_outer_gap = if bar.compact { 0.0 } else { TAB_OUTER_GAP };

    // Single source of truth for geometry — `Backend::tab_bar_layout_icons`
    // calls the same function with the same sidecar, so the no-paint twin
    // can never drift from what this loop actually paints.
    let hits = mac_tab_bar_layout_icons(font, width, bar, icons);
    // Re-derived rather than threaded out of `mac_tab_bar_layout_icons`:
    // the same pure function of (font, icons), so the label x below is
    // offset by exactly the width the hit boxes were built around.
    let icon_extras = mac_tab_icon_extras(font, bar.tabs.len(), icons);

    CGContextSaveGState(ctx);

    // Tab-bar background.
    fill_rect(ctx, 0.0, y_offset, width, row_height, theme.tab_bar_bg);

    // ── Tabs paint loop ──────────────────────────────────────────────
    // `slot_positions` is padded with `(0.0, 0.0)` for the scrolled-past
    // tabs, so start after them and stop where the layout stopped.
    for (tab_idx, tab) in bar
        .tabs
        .iter()
        .enumerate()
        .skip(bar.scroll_offset)
        .take(hits.slot_positions.len().saturating_sub(bar.scroll_offset))
    {
        let (slot_x, slot_end) = hits.slot_positions[tab_idx];
        let tab_content_w = slot_end - slot_x - tab_outer_gap;

        // Tab background.
        let bg_col = if tab.is_active {
            theme.tab_active_bg
        } else {
            theme.tab_bar_bg
        };
        fill_rect(ctx, slot_x, y_offset, tab_content_w, row_height, bg_col);

        // Top accent line for the active tab.
        if tab.is_active {
            if let Some(accent) = bar.active_accent {
                fill_rect(ctx, slot_x, y_offset, tab_content_w, ACCENT_HEIGHT, accent);
            }
        }

        // Icon glyph (#926), at the tab's leading edge in its own colour.
        // `icon_extras[tab_idx]` is the glyph's measured width plus
        // `TAB_ICON_GAP`, so the label below starts one gap past the ink.
        let icon_extra = icon_extras[tab_idx];
        if let Some(icon) = tab_icon_at(icons, tab_idx) {
            draw_text(
                ctx,
                font,
                &icon.glyph,
                slot_x + tab_pad,
                text_y_offset,
                color_to_cg(icon.color),
            );
        }

        // Tab label.
        let fg_col = match (tab.is_active, tab.is_preview) {
            (true, true) => theme.tab_preview_active_fg,
            (true, false) => theme.tab_active_fg,
            (false, true) => theme.tab_preview_inactive_fg,
            (false, false) => theme.tab_inactive_fg,
        };
        draw_text(
            ctx,
            font,
            &tab.label,
            slot_x + tab_pad + icon_extra,
            text_y_offset,
            color_to_cg(fg_col),
        );

        // Close glyph (× or ● for dirty), tinted on hover. Painted at the
        // exact x the hit box was built around, so a click that lands in
        // `close_bounds` lands on the glyph the user saw.
        if let Some((close_lo, _)) = hits.close_bounds[tab_idx] {
            let close_x = close_lo + CLOSE_PAD;
            let is_close_hovered = hovered_close_tab == Some(tab_idx);
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
            draw_text(
                ctx,
                font,
                close_glyph,
                close_x,
                text_y_offset,
                color_to_cg(close_fg),
            );
        }
    }

    // ── Right segments paint loop ────────────────────────────────────
    for (i, seg) in bar.right_segments.iter().enumerate() {
        let (sx, _) = hits.right_segment_bounds[i];
        let fg_col = if seg.is_active {
            theme.tab_active_fg
        } else {
            theme.tab_inactive_fg
        };
        draw_text(ctx, font, &seg.text, sx, text_y_offset, color_to_cg(fg_col));
    }

    CGContextRestoreGState(ctx);

    hits
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
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
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
    use crate::primitives::tab_bar::{TabBar, TabBarSegment, TabIcon, TabItem};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Backend;

    const W: u32 = 480;
    const H: u32 = 28;
    const FONT_SIZE: f64 = 14.0;

    fn font() -> CTFont {
        make_font("Menlo", FONT_SIZE).expect("Menlo installed")
    }

    /// A backend whose *editor* and *chrome* fonts are both this
    /// module's `font()`.
    ///
    /// Issue #1003 moved the tab bar onto the chrome font
    /// (`ChromePrimitive::TabBar`): `draw_tab_bar`/`draw_tab_bar_icons`
    /// and their no-paint `tab_bar_layout*` twins now measure and paint
    /// through `MacBackend::chrome_font`, which `MacBackend::new` seeds
    /// with the ~11pt **system UI** font. A harness that set only the
    /// editor font would therefore paint at a size no assertion here
    /// knows about, while the `mac_tab_bar_layout*`/`mac_tab_icon_extras`
    /// expectations several tests compute directly still used `font()` —
    /// the two would disagree about every slot boundary and glyph
    /// position. Setting both keeps every assertion in this module about
    /// tab-bar geometry at a known `FONT_SIZE`, rather than about
    /// whichever metrics the system UI font happens to have on the
    /// running host. Same fix, same reason, as
    /// `macos::status_bar`'s `backend_with_test_fonts` (#963).
    fn backend_with_test_fonts() -> MacBackend {
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_chrome_font(font());
        backend
    }

    fn sample_bar() -> TabBar {
        TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![
                TabItem {
                    label: "main.rs".into(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
                TabItem {
                    label: "lib.rs".into(),
                    is_active: false,
                    is_dirty: true,
                    is_preview: false,
                    is_closable: true,
                },
            ],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: Some(Color::rgb(80, 140, 255)),
            show_tab_close: true,
            compact: false,
        }
    }

    /// Drive a paint through the full `MacBackend::draw_tab_bar` path
    /// and return `(surface, hits)` for inspection. Mirrors the
    /// status_bar harness so future chrome tests follow the same
    /// shape.
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn paint_via_backend(
        bar: &TabBar,
        hovered_close: Option<usize>,
    ) -> (BitmapSurface, TabBarHits) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(0.0, 0.0, W as f32, H as f32), bar, hovered_close);
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        (surface, hits.into_inner().unwrap())
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn active_tab_paints_active_bg() {
        // The active tab's bg differs from `tab_bar_bg`. Probe just
        // above the bottom edge near the left of the active tab's
        // slot (past the leading padding, before the label glyphs).
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();

        let (start, end) = hits.slot_positions[0];
        assert!(end > start, "active tab slot must have non-zero width");

        // Probe near the bottom-left of the slot — past the 14px
        // padding, below the accent strip, but well outside any glyph
        // pixels. y = row_height - 2 stays inside the painted row.
        let probe_x = (start + 2.0) as u32;
        let probe_y = H - 2;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        let expected = theme.tab_active_bg;
        assert_eq!(
            (r, g, b),
            (expected.r, expected.g, expected.b),
            "active tab bg at ({}, {}) should be tab_active_bg",
            probe_x,
            probe_y,
        );
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn active_accent_paints_at_top_of_active_tab() {
        // 2-px accent strip at y_offset for the active tab.
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);

        let (start, _) = hits.slot_positions[0];
        // Top scanline (y=0) inside the active slot — past leading
        // padding so we don't overlap the second tab.
        let probe_x = (start + 4.0) as u32;
        let (r, g, b, _) = surface.pixel(probe_x, 0);
        let accent = bar.active_accent.unwrap();
        assert_eq!(
            (r, g, b),
            (accent.r, accent.g, accent.b),
            "accent strip at top edge should match TabBar.active_accent",
        );
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn close_bounds_round_trip_via_hits_struct() {
        // Round-trip: paint, then sample a coordinate inside the
        // reported close-bounds and assert the hits struct's bounds
        // contain it. This is the "paint and click agree on close
        // button location" gate.
        let bar = sample_bar();
        let (_surface, hits) = paint_via_backend(&bar, None);

        let close = hits.close_bounds[0].expect("active tab has close bounds");
        let mid_x = (close.0 + close.1) / 2.0;
        assert!(
            mid_x >= close.0 && mid_x < close.1,
            "midpoint must be inside close bounds [{}, {})",
            close.0,
            close.1,
        );
        // The reported bounds must sit inside the tab slot — a paint
        // shift on the close glyph would land outside the slot and
        // catch the drift here.
        let (slot_start, slot_end) = hits.slot_positions[0];
        assert!(
            close.0 >= slot_start && close.1 <= slot_end,
            "close bounds [{}, {}) must be inside tab slot [{}, {})",
            close.0,
            close.1,
            slot_start,
            slot_end,
        );
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn dirty_tab_uses_filled_circle_glyph() {
        // `is_dirty` swaps the close glyph from `×` to `●`. We can't
        // easily compare glyph shape pixel-by-pixel, but a row-wise
        // ink ratio differs noticeably: `●` is mostly filled, `×` is
        // two thin diagonals. Compare the dirty tab's close column
        // against the active tab's: dirty should be visibly denser.
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);

        let active_close = hits.close_bounds[0].unwrap();
        let dirty_close = hits.close_bounds[1].unwrap();

        // Count non-bg pixels in a 1-column strip across the line
        // height for each close glyph. The dirty (●) column should
        // have more inked pixels than the × column.
        fn ink_density(surface: &BitmapSurface, x: u32, bg: Color) -> u32 {
            (0..H)
                .filter(|&y| {
                    let (r, g, b, _) = surface.pixel(x, y);
                    !(r == bg.r && g == bg.g && b == bg.b)
                })
                .count() as u32
        }
        let theme = Theme::default();
        let active_ink = ink_density(
            &surface,
            ((active_close.0 + active_close.1) / 2.0) as u32,
            theme.tab_active_bg,
        );
        let dirty_ink = ink_density(
            &surface,
            ((dirty_close.0 + dirty_close.1) / 2.0) as u32,
            theme.tab_bar_bg,
        );
        assert!(
            dirty_ink > active_ink,
            "dirty `●` glyph should ink more rows ({}) than active `×` ({})",
            dirty_ink,
            active_ink,
        );
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn non_closable_tab_has_no_close_bounds_even_when_bar_show_close_is_true() {
        // Regression: `bar.show_tab_close = true` is a bar-level flag, but
        // individual tabs may opt out via `is_closable = false`. The macOS
        // rasteriser must consult both — otherwise non-closable tabs render
        // a phantom × that triggers no action when clicked.
        // Use identical labels so the *only* width difference between the
        // two slots is the close-glyph reservation, isolating the regression.
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: vec![
                TabItem {
                    label: "tab.rs".into(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
                TabItem {
                    label: "tab.rs".into(),
                    is_active: false,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: false,
                },
            ],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let (_surface, hits) = paint_via_backend(&bar, None);
        assert!(
            hits.close_bounds[0].is_some(),
            "closable tab must have close bounds",
        );
        assert!(
            hits.close_bounds[1].is_none(),
            "non-closable tab must have no close bounds even when bar.show_tab_close is true",
        );
        // The non-closable tab's slot must also be narrower (no close-glyph
        // reservation), so the two slot widths differ in proportion to the
        // close glyph + inner gap.
        let (s0, e0) = hits.slot_positions[0];
        let (s1, e1) = hits.slot_positions[1];
        let closable_slot = e0 - s0;
        let pinned_slot = e1 - s1;
        assert!(
            closable_slot > pinned_slot,
            "closable tab slot ({closable_slot}) should reserve more width than pinned slot ({pinned_slot})",
        );
    }

    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn empty_bar_paints_only_tab_bar_bg() {
        let bar = TabBar {
            id: WidgetId::new("empty"),
            tabs: vec![],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();

        // Whole row should be tab_bar_bg.
        let (r, g, b, _) = surface.pixel(W / 2, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b)
        );
        assert!(hits.slot_positions.is_empty());
        assert!(hits.close_bounds.is_empty());
    }

    /// `Backend::tab_bar_layout` must return exactly what
    /// `draw_tab_bar` painted — both route through `mac_tab_bar_layout`,
    /// and this pins that (quadraui#484). See that function's docs for
    /// the known bar-relative-vs-absolute divergence (#552); what is
    /// guaranteed here is that layout == paint.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn layout_twin_matches_the_painted_hits() {
        let bar = sample_bar();
        let (_surface, painted) = paint_via_backend(&bar, None);

        let backend = backend_with_test_fonts();
        let computed = backend.tab_bar_layout(QRect::new(0.0, 0.0, W as f32, H as f32), &bar);

        assert_eq!(painted.slot_positions, computed.slot_positions);
        assert_eq!(painted.close_bounds, computed.close_bounds);
        assert_eq!(painted.right_segment_bounds, computed.right_segment_bounds);
        assert_eq!(painted.available_cols, computed.available_cols);
        assert_eq!(
            painted.correct_scroll_offset,
            computed.correct_scroll_offset
        );
    }

    /// Issue #919: `mac_tab_bar_native_layout` — the new `TabBarLayout`-
    /// returning function backing `Backend::resolve_tab_bar_layout` —
    /// must report exactly the same geometry `mac_tab_bar_layout`
    /// (the deprecated `TabBarHits`-returning one) does, since the two
    /// duplicate rather than share their measurement code (see
    /// `mac_tab_bar_native_layout`'s doc for why). Converting the new
    /// function's `TabBarLayout` through the shared
    /// `tab_bar_hits_from_layout` helper and comparing field-by-field
    /// against the old function's direct output pins the two together.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn native_layout_agrees_with_hits_layout() {
        let bar = sample_bar();
        let f = font();

        let hits = mac_tab_bar_layout(&f, W as f64, &bar);
        let native = mac_tab_bar_native_layout(&f, W as f64, H as f64, &bar);
        let native_as_hits = crate::backend::tab_bar_hits_from_layout(&native, &bar);

        assert_eq!(
            hits.slot_positions, native_as_hits.slot_positions,
            "tab slots must agree between the two independently-computed layouts"
        );
        assert_eq!(
            hits.close_bounds, native_as_hits.close_bounds,
            "close-button spans must agree too"
        );
        assert_eq!(
            hits.right_segment_bounds, native_as_hits.right_segment_bounds,
            "right-segment spans must agree too"
        );
        assert_eq!(
            hits.correct_scroll_offset, native.resolved_scroll_offset,
            "the \"fit active tab\" scroll correction must agree"
        );
    }

    /// A click at the centre of a reported close box lands on the close
    /// glyph the rasteriser actually drew — the paint↔click round trip
    /// the shared `mac_tab_bar_layout` exists to guarantee.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn close_box_centre_is_where_the_glyph_was_painted() {
        let bar = sample_bar();
        let (surface, hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        let (lo, hi) = hits.close_bounds[0].expect("tab 0 is closable");
        let bg = theme.tab_active_bg;

        // Somewhere in the close box's x-span, some row differs from the
        // tab background: that is the glyph.
        let inked = (lo.ceil() as u32..hi.floor() as u32).any(|x| {
            (0..H).any(|y| {
                let (r, g, b, _) = surface.pixel(x.min(W - 1), y);
                (r, g, b) != (bg.r, bg.g, bg.b)
            })
        });
        assert!(
            inked,
            "close box [{lo}, {hi}] contains no painted glyph — hits and paint have drifted",
        );
    }

    /// A helper for the #926 icon tests: drive a paint through the full
    /// `MacBackend::draw_tab_bar_icons` path with a real sidecar and
    /// return `(surface, hits)`. Deliberately the icon-carrying twin of
    /// [`paint_via_backend`], so a test can compare the two outputs and
    /// attribute every delta to the icon reservation.
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn paint_icons_via_backend(
        bar: &TabBar,
        icons: &[Option<TabIcon>],
        hovered_close: Option<usize>,
    ) -> (BitmapSurface, TabBarHits) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar_icons(
                QRect::new(0.0, 0.0, W as f32, H as f32),
                bar,
                icons,
                hovered_close,
            );
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        (surface, hits.into_inner().unwrap())
    }

    /// An ASCII icon on tab 0 only. ASCII on purpose — a Nerd Font
    /// codepoint measures as tofu (or nothing) under Menlo, which would
    /// make `icon_extra` uninformative; `examples/common/tab_icons_demo.rs`
    /// makes the same choice for the same reason. Tab 1 is left `None` so
    /// every test below also covers the mixed-sidecar case: a decorated
    /// tab must not change an undecorated one's *width*, only its origin.
    fn sample_icons() -> Vec<Option<TabIcon>> {
        vec![
            Some(TabIcon {
                glyph: "R".into(),
                color: Color::rgb(220, 120, 60),
            }),
            None,
        ]
    }

    /// Issue #931: `draw_tab_bar_icons` used to abort the whole process
    /// (via a `debug_assert!` that unwound across the AppKit `drawRect:`
    /// frame, which AppKit can't catch) on the first frame that painted a
    /// **non-empty** icon sidecar — exactly the input a default
    /// `use_nerd_fonts = true` app hits on frame one. Reaching the
    /// assertions below at all is that regression test; #926 then made the
    /// sidecar actually *do* something, which the width assertions pin.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn draw_tab_bar_icons_with_non_empty_sidecar_does_not_abort() {
        let bar = sample_bar();
        let icons = sample_icons();

        let (_surface, hits) = paint_icons_via_backend(&bar, &icons, None);
        let (_icon_less_surface, icon_less_hits) = paint_via_backend(&bar, None);

        // #926: the decorated tab is wider than its icon-less self by
        // exactly the CoreText-measured glyph width plus `TAB_ICON_GAP`.
        let expected_extra = mac_tab_icon_extras(&font(), bar.tabs.len(), &icons)[0];
        assert!(
            expected_extra > TAB_ICON_GAP,
            "an ASCII icon glyph must measure non-zero under Menlo (got \
             {expected_extra} including a {TAB_ICON_GAP}pt gap)",
        );
        let (lo, hi) = hits.slot_positions[0];
        let (plain_lo, plain_hi) = icon_less_hits.slot_positions[0];
        assert_eq!(lo, plain_lo, "the first tab still starts at the bar origin");
        assert!(
            ((hi - lo) - ((plain_hi - plain_lo) + expected_extra)).abs() < 0.01,
            "decorated slot width {} should be the icon-less width {} plus the \
             reservation {expected_extra}",
            hi - lo,
            plain_hi - plain_lo,
        );

        // Tab 1 carries no icon, so its own width is unchanged — it is
        // only pushed right by tab 0's reservation.
        let (t1_lo, t1_hi) = hits.slot_positions[1];
        let (p1_lo, p1_hi) = icon_less_hits.slot_positions[1];
        assert!(
            ((t1_hi - t1_lo) - (p1_hi - p1_lo)).abs() < 0.01,
            "an undecorated tab must keep its icon-less width",
        );
        assert!(
            ((t1_lo - p1_lo) - expected_extra).abs() < 0.01,
            "an undecorated tab shifts right by exactly the preceding tab's \
             icon reservation",
        );
    }

    /// Same #931 non-abort guarantee for the layout-only twins
    /// (`draw_tab_bar_icons_layout`, `tab_bar_layout_icons`,
    /// `resolve_tab_bar_layout_icons`) — none of these paint, but all four
    /// shared the same `debug_assert!` guard before the fix — plus #926's
    /// requirement that all four agree with the *painted* geometry once a
    /// sidecar is present, not merely with each other.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn layout_only_icon_twins_agree_with_the_icon_paint() {
        let bar = sample_bar();
        let icons = sample_icons();
        let rect = QRect::new(0.0, 0.0, W as f32, H as f32);
        let (_surface, painted) = paint_icons_via_backend(&bar, &icons, None);

        let mut backend = backend_with_test_fonts();

        // `tab_bar_layout_icons` is the no-paint twin: it must report
        // exactly what `draw_tab_bar_icons` painted (the load-bearing
        // macOS invariant), and must differ from the icon-less twin.
        let icon_hits = backend.tab_bar_layout_icons(rect, &bar, &icons);
        assert_eq!(icon_hits.slot_positions, painted.slot_positions);
        assert_eq!(icon_hits.close_bounds, painted.close_bounds);
        let plain_hits = backend.tab_bar_layout(rect, &bar);
        assert_ne!(
            icon_hits.slot_positions, plain_hits.slot_positions,
            "#926: a non-empty sidecar must change the reported geometry",
        );

        // `resolve_tab_bar_layout_icons` is the `TabBarLayout` spelling of
        // the same measurement — independently computed (see
        // `mac_tab_bar_native_layout_icons`' doc), so pin it against the
        // painted hits through the shared converter.
        let icon_layout = backend.resolve_tab_bar_layout_icons(rect, &bar, &icons);
        let as_hits = crate::backend::tab_bar_hits_from_layout(&icon_layout, &bar);
        assert_eq!(
            as_hits.slot_positions, painted.slot_positions,
            "the two independently-computed icon layouts must agree",
        );
        assert_eq!(as_hits.close_bounds, painted.close_bounds);

        // And the painting `TabBarLayout` variant must match it too.
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let drawn = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *drawn.borrow_mut() = Some(b.draw_tab_bar_icons_layout(rect, &bar, &icons, None));
        });
        backend.end_frame();
        assert_eq!(
            drawn.into_inner().unwrap().visible_tabs,
            icon_layout.visible_tabs,
        );
    }

    /// Issue #926's headline guarantee: with an icon present, a click at
    /// the centre of the reported close box still lands on the `×` the
    /// rasteriser drew. This is precisely what a *paint-only* icon fix
    /// would break — the glyph shifts right while the hit box stays put —
    /// and why the reservation has to live in the shared measurement pass.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn close_box_still_lands_on_the_close_glyph_with_an_icon_present() {
        let bar = sample_bar();
        let icons = sample_icons();
        let (surface, hits) = paint_icons_via_backend(&bar, &icons, None);
        let (_plain_surface, plain_hits) = paint_via_backend(&bar, None);
        let theme = Theme::default();

        let (lo, hi) = hits.close_bounds[0].expect("tab 0 is closable");
        let (plain_lo, _) = plain_hits.close_bounds[0].expect("tab 0 is closable");
        assert!(
            lo > plain_lo,
            "the icon must push the close box right (icon {lo} vs icon-less {plain_lo})",
        );

        // The close box must still sit inside its own slot…
        let (slot_lo, slot_hi) = hits.slot_positions[0];
        assert!(
            lo >= slot_lo && hi <= slot_hi,
            "close box [{lo}, {hi}) must stay inside tab slot [{slot_lo}, {slot_hi})",
        );

        // …and there must be ink in it: the `×` the user clicks.
        let bg = theme.tab_active_bg;
        let inked = (lo.ceil() as u32..hi.floor() as u32).any(|x| {
            (0..H).any(|y| {
                let (r, g, b, _) = surface.pixel(x.min(W - 1), y);
                (r, g, b) != (bg.r, bg.g, bg.b)
            })
        });
        assert!(
            inked,
            "close box [{lo}, {hi}) contains no painted glyph — the icon \
             reservation moved the hit box off the × it draws",
        );
    }

    /// The icon glyph paints at the tab's leading edge, ahead of the
    /// label, in its own [`TabIcon::color`] rather than the tab foreground
    /// (#926) — the "identity colour survives an inactive tab" half of the
    /// `TabIcon` contract the TUI and GTK rasterisers already honour.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn icon_glyph_paints_before_the_label_in_its_own_colour() {
        let bar = sample_bar();
        let icons = sample_icons();
        let icon_color = icons[0].as_ref().unwrap().color;
        let (surface, hits) = paint_icons_via_backend(&bar, &icons, None);

        let (slot_lo, _) = hits.slot_positions[0];
        let icon_extra = mac_tab_icon_extras(&font(), bar.tabs.len(), &icons)[0];
        // The glyph occupies `[slot_lo + TAB_PAD, slot_lo + TAB_PAD +
        // icon_extra - TAB_ICON_GAP)`; the gap after it is bare
        // background, which is what separates icon ink from label ink.
        let glyph_lo = slot_lo + TAB_PAD;
        let glyph_hi = glyph_lo + icon_extra - TAB_ICON_GAP;
        assert!(glyph_hi > glyph_lo, "icon span must be non-empty");

        // Some pixel in the glyph span leans towards the icon colour.
        // Exact equality would be wrong to assert — CoreText antialiases
        // the glyph against `tab_active_bg` — so look for a pixel closer
        // to the icon colour than to the background.
        let bg = Theme::default().tab_active_bg;
        let dist = |a: (u8, u8, u8), c: Color| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
            d(a.0, c.r) + d(a.1, c.g) + d(a.2, c.b)
        };
        let found = (glyph_lo.floor() as u32..glyph_hi.ceil() as u32).any(|x| {
            (0..H).any(|y| {
                let (r, g, b, _) = surface.pixel(x.min(W - 1), y);
                dist((r, g, b), icon_color) < dist((r, g, b), bg)
            })
        });
        assert!(
            found,
            "no pixel in the icon span [{glyph_lo}, {glyph_hi}) leans towards \
             TabIcon::color {icon_color:?} — the glyph was not painted, or was \
             painted in the tab foreground",
        );
    }

    /// An empty sidecar must reproduce the icon-less rasteriser *pixel for
    /// pixel* — `draw_tab_bar` is now literally `draw_tab_bar_icons(..,
    /// &[])`, and this is what keeps #926 a behaviour change only for
    /// callers that actually pass icons.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn empty_sidecar_is_pixel_identical_to_the_icon_less_paint() {
        let bar = sample_bar();
        let (icon_surface, icon_hits) = paint_icons_via_backend(&bar, &[], None);
        let (plain_surface, plain_hits) = paint_via_backend(&bar, None);

        assert_eq!(icon_hits.slot_positions, plain_hits.slot_positions);
        assert_eq!(icon_hits.close_bounds, plain_hits.close_bounds);
        assert_eq!(icon_hits.available_cols, plain_hits.available_cols);
        for y in 0..H {
            for x in 0..W {
                assert_eq!(
                    icon_surface.pixel(x, y),
                    plain_surface.pixel(x, y),
                    "pixel ({x}, {y}) differs with an empty sidecar",
                );
            }
        }
    }

    /// A sidecar shorter (or longer) than `bar.tabs` must not panic —
    /// `tab_icon_at` treats a missing entry as "no icon", the convention
    /// every backend's sidecar handling shares.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn short_and_overlong_sidecars_are_tolerated() {
        let bar = sample_bar();
        let f = font();

        // One entry for a two-tab bar: tab 1 behaves as `None`.
        let short = vec![Some(TabIcon {
            glyph: "R".into(),
            color: Color::rgb(220, 120, 60),
        })];
        let hits = mac_tab_bar_layout_icons(&f, W as f64, &bar, &short);
        assert_eq!(hits.slot_positions.len(), bar.tabs.len());
        assert_eq!(
            hits.slot_positions[0],
            mac_tab_bar_layout_icons(&f, W as f64, &bar, &sample_icons()).slot_positions[0],
            "a short sidecar decorates the entries it does have",
        );

        // Three entries for a two-tab bar: the extra is ignored.
        let long = vec![
            None,
            None,
            Some(TabIcon {
                glyph: "R".into(),
                color: Color::rgb(220, 120, 60),
            }),
        ];
        let long_hits = mac_tab_bar_layout_icons(&f, W as f64, &bar, &long);
        let plain = mac_tab_bar_layout(&f, W as f64, &bar);
        assert_eq!(
            long_hits.slot_positions, plain.slot_positions,
            "an overlong sidecar whose in-range entries are all None must \
             reproduce the icon-less geometry",
        );
    }

    /// The scroll-offset correction (`fit_active_scroll_offset`) has to see
    /// the icon reservation too — a decorated tab is wider, so fewer tabs
    /// fit and the offset that keeps the active tab visible can differ.
    /// Pins that `mac_tab_bar_layout_icons` feeds the *widened* slot widths
    /// into that correction rather than the icon-less ones.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn icon_reservation_feeds_the_scroll_offset_correction() {
        // Wide labels + the narrow test bar so the active tab can't fit
        // without scrolling, and every tab decorated so the reservation is
        // what tips it over.
        let bar = TabBar {
            id: WidgetId::new("tabs"),
            tabs: (0..6)
                .map(|i| TabItem {
                    label: format!(" a_rather_long_file_name_{i}.rs "),
                    is_active: i == 5,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let icons: Vec<Option<TabIcon>> = (0..6)
            .map(|_| {
                Some(TabIcon {
                    glyph: "RR".into(),
                    color: Color::rgb(220, 120, 60),
                })
            })
            .collect();
        let f = font();

        let plain = mac_tab_bar_layout(&f, W as f64, &bar);
        let decorated = mac_tab_bar_layout_icons(&f, W as f64, &bar, &icons);
        // Guard the premise: the reservation must actually have widened
        // the slots this correction is computed from, otherwise the
        // assertions below would hold vacuously.
        // `bar.scroll_offset` is 0, so slot 0 is the first *painted* tab.
        let slot_w = |h: &TabBarHits| {
            let (lo, hi) = h.slot_positions[0];
            hi - lo
        };
        assert!(
            slot_w(&decorated) > slot_w(&plain),
            "decorated slots must be wider than icon-less ones \
             ({} vs {})",
            slot_w(&decorated),
            slot_w(&plain),
        );
        assert!(
            decorated.correct_scroll_offset >= plain.correct_scroll_offset,
            "wider tabs can only need the same or a later scroll offset \
             (icon-less {}, decorated {})",
            plain.correct_scroll_offset,
            decorated.correct_scroll_offset,
        );
        // Fewer decorated tabs fit in the same bar, so the visible slot
        // count can only shrink or hold.
        let visible = |h: &TabBarHits| h.slot_positions.len();
        assert!(
            visible(&decorated) <= visible(&plain),
            "decorated tabs are wider, so no more of them can fit \
             (icon-less {}, decorated {})",
            visible(&plain),
            visible(&decorated),
        );
    }

    /// `mac_tab_bar_native_layout_icons` and `mac_tab_bar_layout_icons`
    /// duplicate their measurement code (see the former's doc), so #919's
    /// agreement test has to hold with a sidecar present too — #926's
    /// reservation is applied in two places and this is what stops the two
    /// from drifting.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn native_layout_agrees_with_hits_layout_with_icons() {
        let bar = sample_bar();
        let icons = sample_icons();
        let f = font();

        let hits = mac_tab_bar_layout_icons(&f, W as f64, &bar, &icons);
        let native = mac_tab_bar_native_layout_icons(&f, W as f64, H as f64, &bar, &icons);
        let native_as_hits = crate::backend::tab_bar_hits_from_layout(&native, &bar);

        assert_eq!(hits.slot_positions, native_as_hits.slot_positions);
        assert_eq!(hits.close_bounds, native_as_hits.close_bounds);
        assert_eq!(
            hits.right_segment_bounds,
            native_as_hits.right_segment_bounds
        );
        assert_eq!(hits.correct_scroll_offset, native.resolved_scroll_offset);
    }

    /// Issue #934 RED-verify: before the fix, `MacBackend::draw_tab_bar`
    /// forwarded `rect.width` / `rect.y` into a rasteriser that always
    /// painted its background flush against the CGContext's absolute
    /// `x = 0`, ignoring `rect.x` entirely — colliding with whatever sits
    /// to the left of the tab bar's real position (a sidebar, on the
    /// reported bug). Painting at a `rect.x` that models a sidebar's
    /// right edge must leave that sidebar column untouched and start the
    /// tab bar's background exactly at `rect.x`.
    ///
    /// Uses an empty bar (no tabs) so the whole strip is uniformly
    /// `tab_bar_bg` — same reasoning as [`empty_bar_paints_only_tab_bar_bg`]
    /// — so a probe anywhere in the strip can't land on an active tab's
    /// differently-coloured slot instead.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn tab_bar_paints_at_rect_x_not_at_window_origin() {
        const SIDEBAR_W: f32 = 120.0;
        let canvas_w = SIDEBAR_W as u32 + W;
        let surface = BitmapSurface::new(canvas_w, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let bar = TabBar {
            id: WidgetId::new("empty"),
            tabs: vec![],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(canvas_w as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let _ = b.draw_tab_bar(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar, None);
        });
        backend.end_frame();

        // Left of the sidebar boundary must stay exactly as the surface
        // was initialised — fully transparent — never the tab bar's
        // background fill.
        let (_, _, _, a) = surface.pixel(4, H / 2);
        assert_eq!(
            a, 0,
            "sidebar column (x=4) must stay untouched by the tab bar fill",
        );

        // At (and past) the sidebar's right edge: tab bar background.
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(SIDEBAR_W as u32 + 4, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "tab bar background should start at rect.x, not the window origin",
        );
    }

    /// Companion to [`tab_bar_paints_at_rect_x_not_at_window_origin`]:
    /// `Backend::tab_bar_layout`/`draw_tab_bar` document absolute
    /// (target-surface) coordinates for `TabBarHits` — the first tab's
    /// slot must start at or after `rect.x`, never at bar-relative `0.0`.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn tab_bar_hits_are_absolute_at_nonzero_rect_x() {
        const SIDEBAR_W: f32 = 120.0;
        let canvas_w = SIDEBAR_W as u32 + W;
        let surface = BitmapSurface::new(canvas_w, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let bar = sample_bar();
        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(canvas_w as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar, None);
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        let hits = hits.into_inner().unwrap();

        let (first_start, _) = hits.slot_positions[0];
        assert!(
            first_start >= SIDEBAR_W as f64,
            "first tab slot should start at/after rect.x={SIDEBAR_W}, got {first_start}",
        );
    }

    /// Same invariant as [`layout_twin_matches_the_painted_hits`] above,
    /// pinned at a non-zero `rect.x` — the case that #934 regressed on
    /// before this fix (paint and no-paint must still agree once both are
    /// shifted to the absolute contract `Backend::tab_bar_layout`
    /// documents).
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn layout_twin_matches_the_painted_hits_at_nonzero_origin() {
        const SIDEBAR_W: f32 = 120.0;
        let canvas_w = SIDEBAR_W as u32 + W;
        let surface = BitmapSurface::new(canvas_w, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let bar = sample_bar();
        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(canvas_w as f32, H as f32, 1.0));
        let painted = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar, None);
            *painted.borrow_mut() = Some(h);
        });
        backend.end_frame();
        let painted = painted.into_inner().unwrap();

        let computed = backend.tab_bar_layout(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar);

        assert_eq!(painted.slot_positions, computed.slot_positions);
        assert_eq!(painted.close_bounds, computed.close_bounds);
        assert_eq!(painted.right_segment_bounds, computed.right_segment_bounds);
        // Non-blocking review note (#934 iteration 1): the zero-origin
        // sibling `layout_twin_matches_the_painted_hits` also checks
        // `available_cols`/`correct_scroll_offset` equality — neither
        // depends on `rect.x` (both are pure functions of `rect.width` and
        // the bar's own tab widths), but asserting them here too keeps
        // this non-zero-origin case exercising the same fields the
        // zero-origin case does, rather than a strict subset of them.
        assert_eq!(painted.available_cols, computed.available_cols);
        assert_eq!(
            painted.correct_scroll_offset,
            computed.correct_scroll_offset
        );
        // Sanity: the agreement isn't just "both zero" — the geometry
        // actually moved with the origin.
        assert!(painted.slot_positions[0].0 >= SIDEBAR_W as f64);
    }

    /// Issue #934 review follow-up: the reported "editor toolbar" symptom
    /// (three controls — split / actions / overflow — floating at the
    /// wrong position) is not `crate::primitives::toolbar::Toolbar` (that
    /// rasteriser, audited separately, already bakes `rect.x`/`rect.y`
    /// absolutely and was never broken). It is `TabBar::right_segments` —
    /// see `crate::primitives::tab_bar`'s module doc ("split buttons, diff
    /// toolbar, overflow menu") — which paints through the exact same
    /// `super::draw_tab_bar_icons` call, inside the exact same
    /// `CGContextTranslateCTM(ctx, rect.x, 0.0)` wrap, that
    /// [`tab_bar_paints_at_rect_x_not_at_window_origin`] proved fixes the
    /// tabs. No test before this one exercised a non-empty
    /// `right_segments` at a non-zero `rect.x`, so the fix's coverage of
    /// this specific reported symptom was accidental, not verified.
    ///
    /// RED-verified by construction: pre-fix,
    /// `MacBackend::draw_tab_bar`/`tab_bar_layout` never shifted
    /// `TabBarHits` by `rect.x` at all (`shift_tab_bar_hits` didn't exist
    /// yet), so `hits.right_segment_bounds[0]` would equal the bar-relative
    /// `(rel_start, rel_end)` computed below, not `(rel_start + rect.x,
    /// rel_end + rect.x)` — the `assert_eq!` fails pre-fix by exactly
    /// `SIDEBAR_W`.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn right_segments_are_shifted_by_rect_x_like_the_tabs() {
        const SIDEBAR_W: f32 = 120.0;
        let canvas_w = SIDEBAR_W as u32 + W;
        let surface = BitmapSurface::new(canvas_w, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut bar = sample_bar();
        bar.right_segments = vec![TabBarSegment {
            text: "\u{22ef}".into(), // "⋯" overflow glyph
            width_cells: 3,
            id: Some(WidgetId::new("tb:overflow")),
            is_active: false,
        }];

        // Bar-relative geometry `draw_tab_bar_icons` computes internally,
        // before any `rect.x` shift — the pre-fix behaviour.
        let bar_relative = mac_tab_bar_layout(&font(), W as f64, &bar);
        assert_eq!(
            bar_relative.right_segment_bounds.len(),
            1,
            "fixture bar should have exactly one right segment",
        );
        let (rel_start, rel_end) = bar_relative.right_segment_bounds[0];

        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(canvas_w as f32, H as f32, 1.0));
        let hits = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let h = b.draw_tab_bar(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar, None);
            *hits.borrow_mut() = Some(h);
        });
        backend.end_frame();
        let hits = hits.into_inner().unwrap();

        assert_eq!(hits.right_segment_bounds.len(), 1);
        assert_eq!(
            hits.right_segment_bounds[0],
            (rel_start + SIDEBAR_W as f64, rel_end + SIDEBAR_W as f64),
            "right segment bounds should be the bar-relative geometry shifted by rect.x={SIDEBAR_W}",
        );

        // `tab_bar_layout`'s no-paint twin must agree, same invariant as
        // `layout_twin_matches_the_painted_hits_at_nonzero_origin` above.
        let computed = backend.tab_bar_layout(QRect::new(SIDEBAR_W, 0.0, W as f32, H as f32), &bar);
        assert_eq!(hits.right_segment_bounds, computed.right_segment_bounds);
    }
}
