//! Direct2D / DirectWrite rasteriser for [`crate::TabBar`].
//!
//! [`pixel_layout`] measures and positions tabs by hand with DirectWrite
//! pixel measurers — like `macos::tab_bar`'s `mac_tab_bar_native_layout_icons`,
//! not the shared `TabBar::layout` D6 API — so a tab whose full label
//! would overflow the strip gets it ellipsis-fitted through
//! [`crate::primitives::tab_bar::fit_tab_label`] into whatever space is
//! actually left, instead of being dropped from the strip outright. Paint
//! loops consume the resolved `visible_tabs` / `visible_segments` plus the
//! per-tab label strings the fit produced ([`PixelLayout::fitted_labels`]),
//! so they never re-decide a fit the layout already made.
//! Converts to [`TabBarHits`] via the shared
//! [`crate::backend::tab_bar_hits_from_layout`] / `shift_tab_bar_hits`
//! helpers, the same ones the TUI and GTK backends use — see
//! `Backend::tab_bar_layout`'s doc for why that shift matters.
//!
//! Only the actual Direct2D paint entry points ([`draw_tab_bar`],
//! [`draw_tab_bar_icons`], [`draw_tab_bar_layout`],
//! [`draw_tab_bar_icons_layout`], and their shared paint loop) are
//! `#[cfg(target_os = "windows")]`-gated. [`compute_layout`] and every
//! `win_tab_bar_*` no-paint fn below are pure geometry generic over
//! [`crate::primitives::layout_metrics::TextMeasure`] — no Direct2D/
//! DirectWrite type in their signature — so they compile and run
//! everywhere, including a plain `cargo test --features win` on Linux.
//!
//! Takes the live theme as a `&Theme` parameter, matching every other
//! Win-GUI rasteriser's convention — the caller
//! ([`crate::win::WinBackend::draw_tab_bar_icons`] /
//! `draw_tab_bar_icons_layout`) passes `&self.current_theme`, the same
//! field `Backend::set_theme` writes. Likewise takes `&Style` for
//! [`Style::tab_padding_x`] — the left+right inset inside a tab's
//! background fill that the icon/label/close glyph sit inside of — via
//! `&self.current_style`, the same field `Backend::set_current_style`
//! writes.
//!
//! Scope: no [`crate::TabChrome`] bracket-frame support (the `Backend`
//! trait's `draw_tab_bar_with_chrome` / `tab_bar_layout_with_chrome`
//! default bodies fall back to the plain methods below for
//! [`crate::TabChrome::active_frame`], so an unhonoured frame request is
//! not a compile-error gap — see those methods' docs) and no italic
//! preview-tab styling (would need a second `IDWriteTextFormat`; deferred
//! to a follow-up rather than widening this issue). [`crate::TabChrome::focused_index`]
//! *is* honoured — see "Focus-visible ring" below.
//!
//! # Focus-visible ring
//!
//! When [`crate::TabChrome::focused_index`] names a currently-visible
//! tab, [`draw_tab_bar_icons_with_glyphs`] strokes a square ring in
//! [`Theme::focus_ring_color`] at [`Style::focus_ring_width`] flush with
//! that tab's own slot bounds, via [`super::text::stroke_rect`] (which
//! insets the stroke by half its width itself, so the geometry handed to
//! it is the slot's raw content rect) — the same convention
//! `macos::tab_bar`'s and `gtk::tab_bar`'s twin painters use. The caller
//! composes focus-visible suppression into `focused_index` itself
//! (`None` after a click); this module paints unconditionally on
//! whatever index it is given.
//!
//! Still a per-backend paint loop. Unlike `win::menu_bar` /
//! `win::toolbar` / `win::activity_bar`, which were collapsed into
//! `primitives::<name>::native_surface_paint::paint`, every pixel
//! `paint_tab_bar_icons_from_layout` below draws is still Direct2D-
//! specific and still triplicated with `gtk::tab_bar` / `macos::tab_bar`
//! — the two scope gaps just above (no bracket chrome, no italic preview
//! styling; this module also ignores `theme.tab_preview_*_fg` entirely)
//! are three of the five live drifts between those copies. See
//! [`crate::primitives::tab_bar`]'s "`PaintSurface` migration status"
//! section for the full drift table and the macOS `close_bounds`-convention
//! prerequisite that blocks the move.

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

#[cfg(target_os = "windows")]
use super::text::DWrite;
#[cfg(target_os = "windows")]
use super::text::{fill_rect, stroke_rect};
use crate::backend::{shift_tab_bar_hits, tab_bar_hits_from_layout};
use crate::event::Rect;
use crate::primitives::layout_metrics::TextMeasure;
#[cfg(target_os = "windows")]
use crate::primitives::tab_bar::TabBarGlyphs;
use crate::style::Style;
#[cfg(target_os = "windows")]
use crate::theme::Theme;
// `TabBarHits` is `#[deprecated]` — this module still
// narrows its `TabBarLayout` down to that struct because the six
// `Backend` tab-bar methods `WinBackend` implements still return it, so
// the import needs the same allow every use site below does.
#[allow(deprecated)]
use crate::{tab_icon_at, TabBar, TabBarHits, TabBarLayout, TabIcon};
// Only the `#[cfg(all(test, target_os = "windows"))]` module below needs
// `TabChrome` — `pixel_layout` has no `TabChrome` parameter (win has no
// bracket-chrome support; see this module's "Scope" doc).
#[cfg(all(test, target_os = "windows"))]
use crate::TabChrome;

/// Gap (DIPs) between a tab's label and its close glyph.
const TAB_INNER_GAP_DIP: f32 = 10.0;
/// Gap (DIPs) between adjacent tabs.
const TAB_OUTER_GAP_DIP: f32 = 1.0;
/// Gap (DIPs) between a tab's icon glyph ([`crate::TabIcon`]) and its label.
const TAB_ICON_GAP_DIP: f32 = 6.0;
/// Height (DIPs) of the active tab's top-edge accent line. Only used by
/// the paint path.
#[cfg(target_os = "windows")]
const TAB_ACTIVE_ACCENT_DIP: f32 = 2.0;

/// The codicon close glyph's measured width, or `0.0` when the bar paints
/// no close button at all. Used by both [`pixel_layout`] (to size the
/// close-button reservation the label-fit budget below is derived from)
/// and the paint loop (for hover-rect/close-glyph positioning) — one
/// function means layout and paint can never disagree about this width.
fn close_glyph_width(measure: &dyn TextMeasure, bar: &TabBar) -> f32 {
    if bar.show_tab_close {
        // The codicon close glyph — `measure` (a live `&DWrite`)
        // always has the bundled codicon fallback baked into its text
        // format, see `DWrite::new`'s doc.
        measure.width_of(&crate::codicon::CLOSE.to_string())
    } else {
        0.0
    }
}

fn icon_extra_width(measure: &dyn TextMeasure, icons: &[Option<TabIcon>], i: usize) -> f32 {
    match tab_icon_at(icons, i) {
        Some(icon) => measure.width_of(&icon.glyph) + TAB_ICON_GAP_DIP,
        None => 0.0,
    }
}

/// Resolve a [`TabBarGlyphs`] slot (the close or dirty override) to the
/// text run this rasteriser paints via `dwrite.draw_text`, falling back to
/// `default` when the slot is unset. Unlike the TUI backend's per-cell
/// `tab_glyph_char`, this keeps the resolved glyph as a whole `String` —
/// Direct2D paints it as one text run, not one terminal cell.
#[cfg(target_os = "windows")]
fn tab_glyph_text(
    icon: Option<&crate::types::Icon>,
    nerd_fonts_enabled: bool,
    default: char,
) -> String {
    icon.map(|icon| icon.resolve(nerd_fonts_enabled).to_string())
        .unwrap_or_else(|| default.to_string())
}

/// `(leading, trailing)` space reserved around a tab's label — `leading`
/// is the tab's left padding plus any icon reservation, `trailing` is the
/// close-button reservation (when the tab has one) plus the tab's right
/// padding. [`pixel_layout`]'s layout loop derives every tab's label
/// budget through this one function, so the icon side and the close side
/// of a tab can only ever be sized together.
fn tab_label_budget(tab_pad: f32, icon_extra: f32, close_extra: f32) -> (f32, f32) {
    (tab_pad + icon_extra, close_extra + tab_pad)
}

/// Per-tab close-button reservation: [`TAB_INNER_GAP_DIP`] plus the close
/// glyph's own width (`close_glyph_w`, from [`close_glyph_width`]) when
/// `bar` shows a close button for this tab, `0.0` otherwise.
fn close_extra_for(bar: &TabBar, tab_idx: usize, close_glyph_w: f32) -> f32 {
    if bar.show_tab_close && bar.tabs[tab_idx].is_closable {
        TAB_INNER_GAP_DIP + close_glyph_w
    } else {
        0.0
    }
}

/// [`pixel_layout`]'s result.
///
/// `fitted_labels` is parallel to `layout.visible_tabs` (same length, same
/// order) and holds the exact string each visible tab is painted with —
/// already ellipsis-fitted where the full label would not fit its slot.
/// It is a layout *output* rather than something the paint loop re-derives
/// from `VisibleTab::bounds`, because re-deriving means recomputing
/// `bounds.width - TAB_OUTER_GAP_DIP - leading - trailing` in `f32`: that
/// round trip is not exact, so a label the layout fit whole can come back
/// one ulp over budget and lose a character to a spurious ellipsis.
struct PixelLayout {
    layout: TabBarLayout,
    /// `#[cfg_attr(not(target_os = "windows"), allow(dead_code))]`: the
    /// only consumer is [`paint_tab_bar_icons_from_layout`], which is
    /// Direct2D-only and therefore `cfg(target_os = "windows")`. The field
    /// itself stays unconditional so the layout computation that fills it
    /// — and its tests — compile and run on every host.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    fitted_labels: Vec<String>,
    corrected_scroll_offset: usize,
    available_cols: usize,
}

/// Compute the [`PixelLayout`] for `bar` against `rect`'s dimensions,
/// measuring every tab/segment via `measure`. Shared by every paint and
/// no-paint entry point in this module so they can never disagree on
/// geometry.
///
/// Pure geometry over [`TextMeasure`] — no Direct2D/DirectWrite type
/// needed, so [`super::backend::WinBackend`] can call this directly with a
/// [`super::backend`]-local nominal measurer when no live `DWrite` handle
/// exists yet. Windows has no bracket-chrome support (see this module's
/// "Scope" doc), so there is no `TabChrome` parameter to thread through.
///
/// A tab whose full (untruncated) width would overflow the strip gets its
/// label ellipsis-fitted into whatever space is actually left, via the
/// shared [`crate::primitives::tab_bar::fit_tab_label`], instead of being
/// dropped from the strip outright — mirroring `macos::tab_bar`'s
/// `mac_tab_bar_native_layout_icons`. The close-button hit region is
/// positioned off the fitted label's own measured width, so it always
/// lands fully inside the tab's (possibly shrunk) slot.
///
/// `style.tab_padding_x` sets the left+right inset (DIPs) inside a tab's
/// background fill that [`paint_tab_bar_icons_from_layout`] paints the
/// icon/label/close glyph inside of — both reach into `style` for the
/// same value, so paint and layout can never disagree about the inset.
fn pixel_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    style: &Style,
) -> PixelLayout {
    let width = rect.width;
    let height = rect.height;
    let tab_pad = style.tab_padding_x;
    let close_glyph_w = close_glyph_width(measure, bar);

    let tab_name_widths: Vec<f32> = bar
        .tabs
        .iter()
        .map(|t| measure.width_of(&t.label))
        .collect();
    let icon_extras: Vec<f32> = (0..bar.tabs.len())
        .map(|i| icon_extra_width(measure, icons, i))
        .collect();

    // ── Right-aligned segments: reserved before tabs get their budget ──
    let seg_widths: Vec<f32> = bar
        .right_segments
        .iter()
        .map(|s| measure.width_of(&s.text))
        .collect();
    let total_seg_width: f32 = seg_widths.iter().sum();
    let segs_fit = !bar.right_segments.is_empty() && total_seg_width <= width;
    let right_area_width = if segs_fit { total_seg_width } else { 0.0 };
    // The area tabs budget against for scroll-offset/`available_cols`
    // engine feedback — always reserves the segments' full width,
    // regardless of `segs_fit`, matching the shared
    // `primitives::layout_metrics::pixel_tab_bar_layout`'s convention.
    let effective_tab_area = (width - total_seg_width).max(0.0);
    // The area the visible-tabs loop below actually paints into — only
    // excludes the segments' width when they fit (and will be painted).
    let tab_area_no_scroll = (width - right_area_width).max(0.0);

    // ── Per-tab full (untruncated) slot widths — scroll-offset input ───
    let full_slot_widths: Vec<f32> = (0..bar.tabs.len())
        .map(|i| {
            let close_extra = close_extra_for(bar, i, close_glyph_w);
            let (leading, trailing) = tab_label_budget(tab_pad, icon_extras[i], close_extra);
            leading + tab_name_widths[i] + trailing + TAB_OUTER_GAP_DIP
        })
        .collect();
    let total_tab_width: f32 = full_slot_widths.iter().sum();

    // Win has no scroll arrows (always `scroll_arrow_width: 0.0`), so once
    // everything no longer fits the caller owns scroll — honour
    // `bar.scroll_offset` as-is (clamped), matching
    // `TabBar::layout`'s own "caller owns scroll" branch.
    let resolved_scroll_offset = if total_tab_width <= tab_area_no_scroll + f32::EPSILON {
        0
    } else {
        bar.scroll_offset.min(bar.tabs.len().saturating_sub(1))
    };

    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    let corrected_scroll_offset = match active_idx {
        Some(active) => TabBar::fit_active_scroll_offset(
            active,
            bar.tabs.len(),
            effective_tab_area as usize,
            |i| full_slot_widths[i].ceil() as usize,
        ),
        None => bar.scroll_offset,
    };

    // ── Visible tabs ─────────────────────────────────────────────────
    let mut visible_tabs: Vec<crate::VisibleTab> = Vec::new();
    let mut fitted_labels: Vec<String> = Vec::new();
    let mut close_regions: Vec<(Rect, crate::TabBarHit)> = Vec::new();
    let mut body_regions: Vec<(Rect, crate::TabBarHit)> = Vec::new();
    let mut x = 0.0_f32;
    for (i, tab) in bar.tabs.iter().enumerate().skip(resolved_scroll_offset) {
        let name_w = tab_name_widths[i];
        let icon_extra = icon_extras[i];
        let close_extra = close_extra_for(bar, i, close_glyph_w);
        let (leading, trailing) = tab_label_budget(tab_pad, icon_extra, close_extra);
        let full_content_w = leading + name_w + trailing;
        let full_slot_w = full_content_w + TAB_OUTER_GAP_DIP;
        let remaining = (tab_area_no_scroll - x).max(0.0);

        let (label_used, name_w_used, content_w, truncated) =
            if full_slot_w <= remaining + f32::EPSILON {
                (tab.label.clone(), name_w, full_content_w, false)
            } else {
                let truncated_budget = (remaining - TAB_OUTER_GAP_DIP).max(0.0);
                if leading + trailing > truncated_budget {
                    // Not even room for this tab's own padding/close slot —
                    // nothing left on the strip to show it in.
                    break;
                }
                let fitted = crate::primitives::tab_bar::fit_tab_label(
                    &tab.label,
                    truncated_budget,
                    leading,
                    trailing,
                    |s| measure.width_of(s),
                );
                let fitted_w = measure.width_of(&fitted);
                (fitted, fitted_w, leading + fitted_w + trailing, true)
            };
        let slot_w = content_w + TAB_OUTER_GAP_DIP;
        let bounds = Rect::new(x, 0.0, slot_w, height);

        let has_close = bar.show_tab_close && tab.is_closable;
        let close_bounds = if has_close {
            let close_w = TAB_INNER_GAP_DIP + close_glyph_w;
            let cb = Rect::new(x + leading + name_w_used, 0.0, close_w, height);
            close_regions.push((cb, crate::TabBarHit::TabClose(i)));
            Some(cb)
        } else {
            None
        };

        body_regions.push((bounds, crate::TabBarHit::Tab(i)));
        visible_tabs.push(crate::VisibleTab {
            tab_idx: i,
            bounds,
            close_bounds,
        });
        fitted_labels.push(label_used);

        x += slot_w;
        if truncated {
            // This tab already consumed every point of slack the strip had
            // left, and the minimum width any further tab could claim is
            // its own padding alone — more than the at-most-one-glyph-
            // plus-ellipsis slack a truncated fit can leave behind. So no
            // tab after this one can fit either.
            break;
        }
    }

    // Close regions before body regions, matching `TabBar::layout`'s own
    // hit-region ordering, so `hit_test` returns the more-specific close
    // hit when the pointer is on the × glyph.
    let mut hit_regions: Vec<(Rect, crate::TabBarHit)> =
        Vec::with_capacity(close_regions.len() + body_regions.len());
    hit_regions.extend(close_regions);
    hit_regions.extend(body_regions);

    // ── Right-aligned segments ───────────────────────────────────────
    let mut visible_segments: Vec<crate::VisibleSegment> = Vec::with_capacity(seg_widths.len());
    if segs_fit {
        let mut seg_x = width - right_area_width;
        for (i, seg) in bar.right_segments.iter().enumerate() {
            let w = seg_widths[i];
            let bounds = Rect::new(seg_x, 0.0, w, height);
            let clickable = seg.id.is_some();
            if let Some(id) = &seg.id {
                hit_regions.push((bounds, crate::TabBarHit::RightSegment(*id)));
            }
            visible_segments.push(crate::VisibleSegment {
                segment_idx: i,
                bounds,
                clickable,
            });
            seg_x += w;
        }
    }

    let sample_w =
        measure.width_of(crate::primitives::layout_metrics::pixel::TAB_CELL_WIDTH_SAMPLE);
    let char_w = (sample_w
        / crate::primitives::layout_metrics::pixel::TAB_CELL_WIDTH_SAMPLE
            .chars()
            .count() as f32)
        .max(1.0);
    let available_cols = (effective_tab_area / char_w).floor().max(0.0) as usize;

    let layout = TabBarLayout {
        bar_width: width,
        bar_height: height,
        visible_tabs,
        visible_segments,
        scroll_left: None,
        scroll_right: None,
        hit_regions,
        resolved_scroll_offset,
    };
    PixelLayout {
        layout,
        fitted_labels,
        corrected_scroll_offset,
        available_cols,
    }
}

/// Compute the [`TabBarLayout`] half of [`pixel_layout`] — the icon-less
/// callers below only need the layout, not the fitted labels or the
/// scroll-offset/available-cols engine feedback.
fn compute_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    style: &Style,
) -> TabBarLayout {
    pixel_layout(measure, rect, bar, icons, style).layout
}

#[allow(deprecated)] // builds the deprecated `TabBarHits` — issue #823
fn hits_from_layout(
    rect: Rect,
    bar: &TabBar,
    layout: &TabBarLayout,
    corrected_scroll_offset: usize,
    available_cols: usize,
) -> TabBarHits {
    let mut hits = tab_bar_hits_from_layout(layout, bar);
    shift_tab_bar_hits(&mut hits, rect.x as f64);
    hits.correct_scroll_offset = corrected_scroll_offset;
    hits.available_cols = available_cols;
    hits
}

/// Compute a [`TabBar`]'s layout without painting, for a bar decorated
/// with per-tab icons — the no-paint twin of
/// [`draw_tab_bar_icons`]. `&[]` reproduces [`win_tab_bar_layout`].
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub fn win_tab_bar_layout_icons(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    style: &Style,
) -> TabBarHits {
    let PixelLayout {
        layout,
        corrected_scroll_offset,
        available_cols,
        ..
    } = pixel_layout(measure, rect, bar, icons, style);
    hits_from_layout(rect, bar, &layout, corrected_scroll_offset, available_cols)
}

/// Compute a [`TabBar`]'s layout without painting — the icon-less twin of
/// [`win_tab_bar_layout_icons`].
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub fn win_tab_bar_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    style: &Style,
) -> TabBarHits {
    win_tab_bar_layout_icons(measure, rect, bar, &[], style)
}

/// Compute a [`TabBar`]'s [`TabBarLayout`] without painting, for a bar
/// decorated with per-tab icons. Unlike macOS's `mac_tab_bar_native_layout`,
/// no duplicated measurement code is needed here: [`compute_layout`] already
/// *is* the single measurement path both [`draw_tab_bar_icons`] and
/// [`win_tab_bar_layout_icons`] narrow down to `TabBarHits` — this just
/// returns it directly.
pub fn win_tab_bar_native_layout_icons(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    style: &Style,
) -> TabBarLayout {
    compute_layout(measure, rect, bar, icons, style)
}

/// Compute a [`TabBar`]'s [`TabBarLayout`] without painting — the
/// icon-less twin of [`win_tab_bar_native_layout_icons`].
pub fn win_tab_bar_native_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    style: &Style,
) -> TabBarLayout {
    win_tab_bar_native_layout_icons(measure, rect, bar, &[], style)
}

/// Draw a [`TabBar`] with per-tab icon glyphs into `rect` (DIPs)
/// on `target`. Returns [`TabBarHits`] in **target-surface (absolute)**
/// coordinates, matching [`crate::Backend::draw_tab_bar_icons`]'s
/// contract.
///
/// `icons` is a sidecar slice parallel to `bar.tabs` — see
/// [`crate::tab_icon_at`]. `hovered_close_tab` paints a lightened
/// background behind the hovered tab's close glyph.
///
/// # Visual contract
///
/// - **Active tab:** `theme.tab_active_bg` background, plus a
///   [`TAB_ACTIVE_ACCENT_DIP`]-tall top-edge accent line when
///   [`TabBar::active_accent`] is `Some` (`None` paints nothing, matching
///   every other backend).
/// - **Dirty tab:** close glyph is a codicon filled circle instead of
///   the codicon close mark (suppressed while hovered, so the hover
///   state always shows the close mark to close).
/// - **Right segments:** painted in `tab_inactive_fg`, or `tab_active_fg`
///   when `seg.is_active`.
#[cfg(target_os = "windows")]
#[allow(deprecated, clippy::too_many_arguments)] // returns the deprecated `TabBarHits` — issue #823
pub fn draw_tab_bar_icons(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    hovered_close_tab: Option<usize>,
    theme: &Theme,
    style: &Style,
) -> TabBarHits {
    draw_tab_bar_icons_with_glyphs(
        target,
        dwrite,
        rect,
        bar,
        icons,
        hovered_close_tab,
        theme,
        style,
        None,
        false,
        None,
        None,
        None,
    )
}

/// [`draw_tab_bar_icons`], but honouring an optional
/// [`TabBarGlyphs`] override for the close/dirty mark — `glyphs: None`
/// reproduces [`draw_tab_bar_icons`] exactly. `hovered_tab`/`pressed_tab`
/// name the whole tab (not its close glyph) the mouse is over/held down
/// on — `None`/`None` paints every tab's resting background, same as
/// [`draw_tab_bar_icons`]. `focused_index` names the tab the
/// keyboard-focus ring paints on — see this module's "Focus-visible
/// ring" doc for the geometry — with `None` painting no ring at all,
/// same as [`draw_tab_bar_icons`].
///
/// Caveat: the close-glyph hover rect and hit zone
/// ([`crate::TabBarHits::close_bounds`]) are both sized from
/// [`close_glyph_width`], which measures the default codicon close glyph
/// — not the override. A visually wide `with_close_glyph`/
/// `with_dirty_glyph` override can overhang the tab and paint outside the
/// rect the hover/hit geometry reserved for it.
#[cfg(target_os = "windows")]
#[allow(deprecated, clippy::too_many_arguments)] // returns the deprecated `TabBarHits` — issue #823
pub fn draw_tab_bar_icons_with_glyphs(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    hovered_close_tab: Option<usize>,
    theme: &Theme,
    style: &Style,
    glyphs: Option<&TabBarGlyphs>,
    nerd_fonts_enabled: bool,
    hovered_tab: Option<usize>,
    pressed_tab: Option<usize>,
    focused_index: Option<usize>,
) -> TabBarHits {
    let PixelLayout {
        layout,
        fitted_labels,
        corrected_scroll_offset,
        available_cols,
    } = pixel_layout(dwrite, rect, bar, icons, style);
    paint_tab_bar_icons_from_layout(
        target,
        dwrite,
        rect,
        bar,
        icons,
        hovered_close_tab,
        &layout,
        &fitted_labels,
        theme,
        style,
        glyphs,
        nerd_fonts_enabled,
        hovered_tab,
        pressed_tab,
        focused_index,
    );
    hits_from_layout(rect, bar, &layout, corrected_scroll_offset, available_cols)
}

/// Draw a [`TabBar`] with per-tab icon glyphs, returning [`TabBarLayout`]
/// instead of the deprecated [`TabBarHits`] — the
/// `TabBarLayout`-returning counterpart to [`draw_tab_bar_icons`] above.
/// Shares [`pixel_layout`] and [`paint_tab_bar_icons_from_layout`] with
/// it, so the two can never paint different pixels — only the return
/// value differs.
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
pub fn draw_tab_bar_icons_layout(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    hovered_close_tab: Option<usize>,
    theme: &Theme,
    style: &Style,
) -> TabBarLayout {
    draw_tab_bar_icons_layout_with_state(
        target,
        dwrite,
        rect,
        bar,
        icons,
        hovered_close_tab,
        theme,
        style,
        None,
        None,
        None,
    )
}

/// [`draw_tab_bar_icons_layout`], plus `hovered_tab`/`pressed_tab`
/// indices naming the whole tab (not its close glyph) the mouse is
/// over/held down on, and `focused_index` naming the tab the
/// keyboard-focus ring paints on — the `TabBarLayout`-returning twin of
/// [`draw_tab_bar_icons_with_glyphs`]'s own `hovered_tab`/`pressed_tab`/
/// `focused_index` triple. Not re-exported from `super::mod` (unlike
/// [`draw_tab_bar_icons_layout`]): [`crate::win::WinBackend`] is the only
/// caller, reached through
/// [`crate::Backend::draw_tab_bar_layout_with_state`] (`focused_index:
/// None`) and [`crate::Backend::draw_tab_bar_with_chrome_layout`]
/// (`focused_index: chrome.focused_index`).
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_tab_bar_icons_layout_with_state(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    hovered_close_tab: Option<usize>,
    theme: &Theme,
    style: &Style,
    hovered_tab: Option<usize>,
    pressed_tab: Option<usize>,
    focused_index: Option<usize>,
) -> TabBarLayout {
    let PixelLayout {
        layout,
        fitted_labels,
        ..
    } = pixel_layout(dwrite, rect, bar, icons, style);
    paint_tab_bar_icons_from_layout(
        target,
        dwrite,
        rect,
        bar,
        icons,
        hovered_close_tab,
        &layout,
        &fitted_labels,
        theme,
        style,
        None,
        false,
        hovered_tab,
        pressed_tab,
        focused_index,
    );
    layout
}

/// Shared paint loop for [`draw_tab_bar_icons`] / [`draw_tab_bar_icons_layout`]
/// — paints `bar` from a pre-computed `layout` and returns nothing, so
/// both callers can hand back whichever return type (`TabBarHits` vs.
/// `TabBarLayout`) their contract needs.
///
/// `fitted_labels` is [`PixelLayout::fitted_labels`] — the string to paint
/// for each entry of `layout.visible_tabs`, in the same order. Taking it
/// as an argument rather than re-deriving it from `VisibleTab::bounds` is
/// what keeps the painted label and the close-button hit box the layout
/// positioned around it in exact agreement.
///
/// `hovered_tab`/`pressed_tab` name the whole tab (not its close glyph,
/// which `hovered_close_tab` already covers) the mouse is over/held down
/// on — tinted via `Theme::list_hover_bg`/`list_pressed_bg`, the same
/// "blend the row's own resting background toward `foreground`"
/// derivation `menu_item_hover_bg`/`menu_item_pressed_bg` already reuse
/// for a popup-menu row. Pressed wins over hovered, matching every other
/// `_with_state` painter in this crate's sibling backends.
///
/// `focused_index` names the tab the keyboard-focus ring paints on — see
/// this module's "Focus-visible ring" doc — painted in a pass after
/// every tab's own background/label/close glyph.
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
fn paint_tab_bar_icons_from_layout(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    hovered_close_tab: Option<usize>,
    layout: &TabBarLayout,
    fitted_labels: &[String],
    theme: &Theme,
    style: &Style,
    glyphs: Option<&TabBarGlyphs>,
    nerd_fonts_enabled: bool,
    hovered_tab: Option<usize>,
    pressed_tab: Option<usize>,
    focused_index: Option<usize>,
) {
    let _ = fill_rect(target, rect, theme.tab_bar_bg);

    let close_w = close_glyph_width(dwrite, bar);

    for (slot, vt) in layout.visible_tabs.iter().enumerate() {
        let tab = &bar.tabs[vt.tab_idx];
        let visual_w = (vt.bounds.width - TAB_OUTER_GAP_DIP).max(0.0);
        let tab_rect = Rect::new(
            rect.x + vt.bounds.x,
            rect.y + vt.bounds.y,
            visual_w,
            vt.bounds.height,
        );

        let is_tab_pressed = pressed_tab == Some(vt.tab_idx);
        let is_tab_hovered = hovered_tab == Some(vt.tab_idx);
        let tint = |base: crate::types::Color| -> crate::types::Color {
            if is_tab_pressed {
                theme.list_pressed_bg(base)
            } else if is_tab_hovered {
                theme.list_hover_bg(base)
            } else {
                base
            }
        };

        let bg = if tab.is_active {
            theme.tab_active_bg
        } else {
            theme.tab_bar_bg
        };
        let _ = fill_rect(target, tab_rect, tint(bg));

        if tab.is_active {
            if let Some(accent) = bar.active_accent {
                let accent_rect = Rect::new(
                    tab_rect.x,
                    tab_rect.y,
                    tab_rect.width,
                    TAB_ACTIVE_ACCENT_DIP,
                );
                let _ = fill_rect(target, accent_rect, accent);
            }
        }

        let fg = if tab.is_active {
            theme.tab_active_fg
        } else {
            theme.tab_inactive_fg
        };
        let mut cursor_x = tab_rect.x + style.tab_padding_x;

        if let Some(icon) = tab_icon_at(icons, vt.tab_idx) {
            let (iw, ih) = dwrite.measure_text(&icon.glyph).unwrap_or((0.0, 0.0));
            let icon_rect = Rect::new(cursor_x, tab_rect.y + (tab_rect.height - ih) / 2.0, iw, ih);
            let _ = dwrite.draw_text(target, &icon.glyph, icon_rect, icon.color);
            cursor_x += iw + TAB_ICON_GAP_DIP;
        }

        // The label the layout fitted to this slot — already
        // ellipsis-truncated by `pixel_layout` when the full label would
        // have overflowed the strip, and the same string the close-button
        // hit box was positioned off.
        let fitted_label: &str = fitted_labels
            .get(slot)
            .map(String::as_str)
            .unwrap_or(&tab.label);

        let (name_w, name_h) = dwrite.measure_text(fitted_label).unwrap_or((0.0, 0.0));
        let label_rect = Rect::new(
            cursor_x,
            tab_rect.y + (tab_rect.height - name_h) / 2.0,
            name_w,
            name_h,
        );
        let _ = dwrite.draw_text(target, fitted_label, label_rect, fg);

        if bar.show_tab_close && tab.is_closable {
            if let Some(cb) = vt.close_bounds {
                let close_x = rect.x + cb.x + TAB_INNER_GAP_DIP;
                let is_close_hovered = hovered_close_tab == Some(vt.tab_idx);

                if is_close_hovered {
                    let hover_bg = theme.tab_bar_bg.lighten(0.15);
                    let hover_rect = Rect::new(
                        close_x - 2.0,
                        tab_rect.y + 2.0,
                        close_w + 4.0,
                        (tab_rect.height - 4.0).max(0.0),
                    );
                    let _ = fill_rect(target, hover_rect, hover_bg);
                }

                // Codicon glyphs by default — `dwrite` always has the
                // bundled codicon fallback baked into its text format, see
                // `DWrite::new`'s doc. An app-supplied `glyphs` override
                // replaces either mark.
                let close_glyph = if tab.is_dirty && !is_close_hovered {
                    tab_glyph_text(
                        glyphs.and_then(TabBarGlyphs::dirty_glyph),
                        nerd_fonts_enabled,
                        crate::codicon::DIRTY,
                    )
                } else {
                    tab_glyph_text(
                        glyphs.and_then(TabBarGlyphs::close_glyph),
                        nerd_fonts_enabled,
                        crate::codicon::CLOSE,
                    )
                };
                let close_fg = if tab.is_dirty || is_close_hovered {
                    theme.foreground
                } else if tab.is_active {
                    theme.tab_inactive_fg
                } else {
                    theme.separator
                };
                let (cgw, cgh) = dwrite.measure_text(&close_glyph).unwrap_or((0.0, 0.0));
                let close_rect = Rect::new(
                    close_x,
                    tab_rect.y + (tab_rect.height - cgh) / 2.0,
                    cgw,
                    cgh,
                );
                let _ = dwrite.draw_text(target, &close_glyph, close_rect, close_fg);
            }
        }
    }

    // ── Keyboard-focus ring pass ─────────────────────────────────────
    // Painted after every tab's own background/label/close glyph, not
    // inside the loop above: slots are contiguous (only
    // `TAB_OUTER_GAP_DIP` of bar background between them), so a ring
    // stroked immediately after its own tab would have its trailing
    // edge painted over by the next tab's opaque background fill.
    // `focused_index` already carries the caller's own focus-visible
    // suppression (see `crate::TabChrome::focused_index`'s doc), so
    // `None` here paints nothing.
    //
    // `stroke_rect` insets its own geometry by half of `stroke_width`
    // (see that fn's doc), so the rect handed to it here is the tab's
    // raw visual slot — not pre-inset the way `macos::tab_bar`/
    // `gtk::tab_bar`'s twin painters need, since Cairo/CoreGraphics
    // centre a stroke on the path verbatim. Strokes square (not the
    // active tab's rounded chip — this rasteriser paints tabs with
    // square corners throughout), since keyboard focus can land on an
    // inactive tab too.
    if let Some(focus_idx) = focused_index {
        if let Some(vt) = layout
            .visible_tabs
            .iter()
            .find(|vt| vt.tab_idx == focus_idx)
        {
            let visual_w = (vt.bounds.width - TAB_OUTER_GAP_DIP).max(0.0);
            let ring_rect = Rect::new(
                rect.x + vt.bounds.x,
                rect.y + vt.bounds.y,
                visual_w,
                vt.bounds.height,
            );
            let _ = stroke_rect(
                target,
                ring_rect,
                theme.focus_ring_color(),
                style.focus_ring_width,
            );
        }
    }

    for vs in &layout.visible_segments {
        let seg = &bar.right_segments[vs.segment_idx];
        let fg = if seg.is_active {
            theme.tab_active_fg
        } else {
            theme.tab_inactive_fg
        };
        let (seg_w, seg_h) = dwrite.measure_text(&seg.text).unwrap_or((0.0, 0.0));
        let seg_rect = Rect::new(
            rect.x + vs.bounds.x,
            rect.y + vs.bounds.y + (vs.bounds.height - seg_h) / 2.0,
            seg_w,
            seg_h,
        );
        let _ = dwrite.draw_text(target, &seg.text, seg_rect, fg);
    }
}

/// Draw a [`TabBar`] with no per-tab icons — [`draw_tab_bar_icons`] with
/// `icons: &[]`.
#[cfg(target_os = "windows")]
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub fn draw_tab_bar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    hovered_close_tab: Option<usize>,
    theme: &Theme,
    style: &Style,
) -> TabBarHits {
    draw_tab_bar_icons(
        target,
        dwrite,
        rect,
        bar,
        &[],
        hovered_close_tab,
        theme,
        style,
    )
}

/// Draw a [`TabBar`] with no per-tab icons, returning [`TabBarLayout`]
/// instead of the deprecated [`TabBarHits`] —
/// [`draw_tab_bar_icons_layout`] with `icons: &[]`.
#[cfg(target_os = "windows")]
pub fn draw_tab_bar_layout(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    hovered_close_tab: Option<usize>,
    theme: &Theme,
    style: &Style,
) -> TabBarLayout {
    draw_tab_bar_icons_layout(
        target,
        dwrite,
        rect,
        bar,
        &[],
        hovered_close_tab,
        theme,
        style,
    )
}

/// Portable tests for the measurement/placement half of this module.
/// [`pixel_layout`] is pure geometry over [`TextMeasure`], so these run on
/// every host — unlike `mod tests` below, which needs a live
/// `DWrite`/`HeadlessSurface`.
#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::primitives::tab_bar::TabItem;
    use crate::types::WidgetId;

    /// DIPs per character for [`Mono`]. Deliberately fractional: a whole
    /// number would let an `f32` budget round trip through
    /// add-then-subtract exactly and hide an off-by-one-ulp fit.
    const CHAR_W: f32 = 7.3;

    /// Monospace stand-in for `DWrite` — every `char` is [`CHAR_W`] wide.
    struct Mono;

    impl TextMeasure for Mono {
        fn width_of(&self, text: &str) -> f32 {
            text.chars().count() as f32 * CHAR_W
        }
    }

    fn bar_with(labels: &[&str]) -> TabBar {
        TabBar {
            id: WidgetId::new("tabbar:test"),
            tabs: labels
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == 0,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }

    /// Full (untruncated) slot width of a closable tab whose label is
    /// `label`, measured the way [`pixel_layout`] measures it.
    fn full_slot_width(label: &str, style: &Style) -> f32 {
        let close_extra = TAB_INNER_GAP_DIP + Mono.width_of(&crate::codicon::CLOSE.to_string());
        let (leading, trailing) = tab_label_budget(style.tab_padding_x, 0.0, close_extra);
        leading + Mono.width_of(label) + trailing + TAB_OUTER_GAP_DIP
    }

    fn layout_of(bar: &TabBar, width: f32, style: &Style) -> PixelLayout {
        pixel_layout(&Mono, Rect::new(0.0, 0.0, width, 30.0), bar, &[], style)
    }

    /// A label too wide for the strip keeps its tab on the strip, fitted
    /// with an ellipsis, instead of the tab being dropped outright.
    #[test]
    fn overlong_label_is_ellipsis_fitted_rather_than_dropping_its_tab() {
        let style = Style::default();
        let bar = bar_with(&["a_very_long_filename_for_this_single_tab.rs"]);
        let strip_w = full_slot_width(&bar.tabs[0].label, &style) / 3.0;

        let resolved = layout_of(&bar, strip_w, &style);

        assert_eq!(
            resolved.layout.visible_tabs.len(),
            1,
            "the narrow strip must still show the tab, not drop it"
        );
        let fitted = &resolved.fitted_labels[0];
        assert!(
            fitted.ends_with('…'),
            "fitted label {fitted:?} must end in an ellipsis"
        );
        assert!(
            fitted.chars().count() < bar.tabs[0].label.chars().count(),
            "fitted label {fitted:?} must be shorter than the original {:?}",
            bar.tabs[0].label
        );
        let bounds = resolved.layout.visible_tabs[0].bounds;
        assert!(
            bounds.x + bounds.width <= strip_w + f32::EPSILON,
            "fitted tab {bounds:?} must stay inside the {strip_w}-wide strip"
        );
    }

    /// The close-button hit box of a fitted tab abuts the *fitted* label
    /// and stays inside the strip — so a click on the painted × resolves
    /// to [`crate::TabBarHit::TabClose`] rather than to the tab body.
    #[test]
    fn close_hit_box_of_a_fitted_tab_abuts_the_fitted_label_inside_the_strip() {
        let style = Style::default();
        let bar = bar_with(&["another_quite_long_tab_label_here.rs"]);
        let strip_w = full_slot_width(&bar.tabs[0].label, &style) / 2.0;

        let resolved = layout_of(&bar, strip_w, &style);

        let vt = &resolved.layout.visible_tabs[0];
        let close = vt.close_bounds.expect("a closable tab reports a close box");
        let close_extra = TAB_INNER_GAP_DIP + Mono.width_of(&crate::codicon::CLOSE.to_string());
        let (leading, _) = tab_label_budget(style.tab_padding_x, 0.0, close_extra);
        let expected_x = vt.bounds.x + leading + Mono.width_of(&resolved.fitted_labels[0]);
        assert!(
            (close.x - expected_x).abs() < 0.01,
            "close box x {} must sit right after the fitted label (expected {expected_x})",
            close.x
        );
        assert!(
            close.x + close.width <= strip_w + f32::EPSILON,
            "close box {close:?} must stay inside the {strip_w}-wide strip"
        );
        assert_eq!(
            resolved.layout.hit_test(close.x + close.width / 2.0, 1.0),
            crate::TabBarHit::TabClose(0),
            "the fitted tab's close box must still hit-test as its close button"
        );
    }

    /// A label the strip has exact room for keeps every character — the
    /// ellipsis fit only engages on genuine overflow, never on an `f32`
    /// rounding wobble in the budget.
    #[test]
    fn label_that_exactly_fills_the_strip_keeps_every_character() {
        let style = Style::default();
        let bar = bar_with(&["exactly_this_wide.rs"]);
        let strip_w = full_slot_width(&bar.tabs[0].label, &style);

        let resolved = layout_of(&bar, strip_w, &style);

        assert_eq!(
            resolved.fitted_labels,
            vec![bar.tabs[0].label.clone()],
            "an exactly-fitting label must not be truncated"
        );
    }

    /// Tabs that fit are painted whole; the first one that overflows is
    /// ellipsis-fitted into the slack that is left.
    #[test]
    fn tabs_that_fit_stay_whole_and_only_the_overflowing_one_is_fitted() {
        let style = Style::default();
        let bar = bar_with(&["a.rs", "bbbbbbbbbbbbbbbb.rs", "c.rs"]);
        let strip_w =
            full_slot_width("a.rs", &style) + full_slot_width("bbbbbbbbbbbbbbbb.rs", &style) / 2.0;

        let resolved = layout_of(&bar, strip_w, &style);

        assert_eq!(
            resolved.fitted_labels.len(),
            2,
            "the third tab has no room left at all: {:?}",
            resolved.fitted_labels
        );
        assert_eq!(
            resolved.fitted_labels[0], "a.rs",
            "a tab that fits must keep its whole label"
        );
        assert!(
            resolved.fitted_labels[1].ends_with('…'),
            "the overflowing tab {:?} must be ellipsis-fitted",
            resolved.fitted_labels[1]
        );
    }

    /// `style.tab_padding_x` is the only source of a tab's label inset, so
    /// a wider padding leaves less room for the label and bites into the
    /// fitted string rather than overflowing the slot.
    #[test]
    fn wider_tab_padding_shortens_the_fitted_label() {
        let narrow = Style::default().with_tab_padding_x(4.0);
        let wide = Style::default().with_tab_padding_x(24.0);
        let bar = bar_with(&["a_moderately_long_tab_label.rs"]);
        let strip_w = full_slot_width(&bar.tabs[0].label, &narrow) / 2.0;

        let narrow_fit = layout_of(&bar, strip_w, &narrow).fitted_labels[0].clone();
        let wide_fit = layout_of(&bar, strip_w, &wide).fitted_labels[0].clone();

        assert!(
            wide_fit.chars().count() < narrow_fit.chars().count(),
            "wider padding must shorten the fitted label \
             (narrow {narrow_fit:?}, wide {wide_fit:?})"
        );
    }
}

// Tests below paint through a real `DWrite`/`HeadlessSurface`, which only
// exists on Windows. The portable measurement/placement half is covered by
// `mod layout_tests` above.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::primitives::tab_bar::{TabBarHit, TabBarSegment, TabItem};
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 300.0;
    const H: f32 = 30.0;

    fn bar() -> TabBar {
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
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
            ],
            scroll_offset: 0,
            right_segments: vec![TabBarSegment {
                text: " ⇅ ".into(),
                width_cells: 3,
                id: Some(WidgetId::new("tab:split")),
                is_active: false,
            }],
            active_accent: Some(Color::rgb(80, 140, 255)),
            show_tab_close: true,
            compact: false,
        }
    }

    /// Paint↔click round trip: the active tab's background must be
    /// painted at its own bounds, and a click at the centre of each
    /// visible tab (per the independently-computed `TabBarLayout`) must
    /// `hit_test` back to that tab.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);
        let style = Style::default();

        surface
            .paint(|target| {
                draw_tab_bar(target, &dwrite, rect, &bar, None, &Theme::default(), &style);
            })
            .expect("paint tab bar");

        let layout = compute_layout(&dwrite, rect, &bar, &[], &style);
        assert_eq!(layout.visible_tabs.len(), 2, "both tabs should fit");

        for vt in &layout.visible_tabs {
            let cx = vt.bounds.x + vt.bounds.width / 2.0;
            let cy = vt.bounds.y + vt.bounds.height / 2.0;
            assert_eq!(
                layout.hit_test(cx, cy),
                TabBarHit::Tab(vt.tab_idx),
                "tab {} centre should hit-test back to itself",
                vt.tab_idx,
            );
        }

        // Active tab (index 0) is painted with `theme.tab_active_bg`,
        // distinct from the bar's own `tab_bar_bg` — sample just inside
        // its left padding, clear of the accent line and any glyph.
        let theme = Theme::default();
        let active_bounds = layout.visible_tabs[0].bounds;
        let sample_x = (active_bounds.x + 2.0) as u32;
        let sample_y = (active_bounds.y + active_bounds.height - 4.0) as u32;
        let px = surface.pixel_at(sample_x, sample_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.tab_active_bg.r,
                theme.tab_active_bg.g,
                theme.tab_active_bg.b
            ),
            "active tab should paint its background at its own bounds"
        );

        // Right segment's `TabBarHits.right_segment_bounds[0]` must agree
        // (in absolute coordinates) with where the layout's own
        // `visible_segments[0]` says it painted.
        let hits = win_tab_bar_layout(&dwrite, rect, &bar, &style);
        let vs = &layout.visible_segments[0];
        assert_eq!(
            hits.right_segment_bounds[0],
            (vs.bounds.x as f64, (vs.bounds.x + vs.bounds.width) as f64),
            "TabBarHits right-segment bounds must be absolute (rect.x == 0 here) \
             and agree with the layout's own visible_segments"
        );
    }

    /// The no-paint layout (`win_tab_bar_layout`) must agree byte-for-byte
    /// with what `draw_tab_bar` painted — same bar, same rect, same
    /// measurer.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(5.0, 0.0, W, H);
        let style = Style::default();

        let surface = HeadlessSurface::new((W + 5.0) as u32, H as u32).expect("create surface");
        let mut painted = None;
        surface
            .paint(|target| {
                painted = Some(draw_tab_bar(
                    target,
                    &dwrite,
                    rect,
                    &bar,
                    None,
                    &Theme::default(),
                    &style,
                ));
            })
            .expect("paint");
        let painted = painted.expect("draw_tab_bar ran");
        let no_paint = win_tab_bar_layout(&dwrite, rect, &bar, &style);

        assert_eq!(painted, no_paint);
    }

    /// The new `TabBarLayout`-returning fns (`draw_tab_bar_layout` /
    /// `win_tab_bar_native_layout`) must agree with each other exactly as their
    /// `TabBarHits`-returning siblings do — same bar, same rect, same measurer.
    /// Since both new fns are thin wrappers over the shared `compute_layout`
    /// this is mostly a wiring check, but it pins the paint path's `layout`
    /// (post-paint) against the no-paint path's independently-called
    /// `compute_layout` so the two can't silently diverge later.
    #[test]
    fn native_layout_no_paint_matches_native_layout_paint() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(5.0, 0.0, W, H);
        let style = Style::default();

        let surface = HeadlessSurface::new((W + 5.0) as u32, H as u32).expect("create surface");
        let mut painted = None;
        surface
            .paint(|target| {
                painted = Some(draw_tab_bar_layout(
                    target,
                    &dwrite,
                    rect,
                    &bar,
                    None,
                    &Theme::default(),
                    &style,
                ));
            })
            .expect("paint");
        let painted = painted.expect("draw_tab_bar_layout ran");
        let no_paint = win_tab_bar_native_layout(&dwrite, rect, &bar, &style);

        assert_eq!(painted, no_paint);
    }

    /// The `TabBarLayout`-returning accessor and the deprecated
    /// `TabBarHits`-returning one must report the same geometry — converting the
    /// former through the shared `tab_bar_hits_from_layout` helper and comparing
    /// field-by-field against the latter's direct output. This is the "twin"
    /// pattern: the other backends each get their own version of this test
    /// (`gtk::testing::tab_bar_layout_twin_matches_painted_geometry_headless`,
    /// its TUI counterpart, and macOS's `native_layout_agrees_with_hits_layout`).
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn native_layout_agrees_with_hits_layout() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);
        let style = Style::default();

        let hits = win_tab_bar_layout(&dwrite, rect, &bar, &style);
        let native = win_tab_bar_native_layout(&dwrite, rect, &bar, &style);
        let mut native_as_hits = tab_bar_hits_from_layout(&native, &bar);
        shift_tab_bar_hits(&mut native_as_hits, rect.x as f64);

        assert_eq!(
            hits.slot_positions, native_as_hits.slot_positions,
            "tab slots must agree between the two accessors"
        );
        assert_eq!(
            hits.close_bounds, native_as_hits.close_bounds,
            "close-button spans must agree too"
        );
        assert_eq!(
            hits.right_segment_bounds, native_as_hits.right_segment_bounds,
            "right-segment spans must agree too"
        );
    }

    /// An app-supplied [`TabBarGlyphs`] override swaps the close/dirty
    /// mark the Windows rasteriser paints; with `glyphs: None`,
    /// [`draw_tab_bar_icons_with_glyphs`] paints the same codicon default
    /// as [`draw_tab_bar_icons`].
    #[test]
    fn close_dirty_glyph_override_paints_app_glyph_default_paints_codicon() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut dirty_bar = bar();
        dirty_bar.tabs[0].is_dirty = true;
        let rect = Rect::new(0.0, 0.0, W, H);
        let style = Style::default();

        let glyphs = TabBarGlyphs::new()
            .with_close_glyph("x")
            .with_dirty_glyph("*");

        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let previous = crate::testing::install_text_run_sink();
        surface
            .paint(|target| {
                draw_tab_bar_icons_with_glyphs(
                    target,
                    &dwrite,
                    rect,
                    &dirty_bar,
                    &[],
                    None,
                    &Theme::default(),
                    &style,
                    Some(&glyphs),
                    true,
                    None,
                    None,
                    None,
                );
            })
            .expect("paint with glyph override");
        let runs = crate::testing::take_text_run_sink(previous);
        assert!(
            runs.iter().any(|r| r.text == "*"),
            "expected the overridden dirty glyph among painted runs, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
        assert!(!runs.iter().any(|r| r.text.contains(crate::codicon::DIRTY)));
        // Tab 1 (`lib.rs`) is closable and not dirty, so the close-glyph
        // override paints there too.
        assert!(
            runs.iter().any(|r| r.text == "x"),
            "expected the overridden close glyph among painted runs, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
        assert!(!runs.iter().any(|r| r.text.contains(crate::codicon::CLOSE)));

        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let previous = crate::testing::install_text_run_sink();
        surface
            .paint(|target| {
                draw_tab_bar_icons_with_glyphs(
                    target,
                    &dwrite,
                    rect,
                    &dirty_bar,
                    &[],
                    None,
                    &Theme::default(),
                    &style,
                    None,
                    true,
                    None,
                    None,
                    None,
                );
            })
            .expect("paint with no override");
        let runs = crate::testing::take_text_run_sink(previous);
        assert!(
            runs.iter().any(|r| r.text.contains(crate::codicon::DIRTY)),
            "expected the default codicon dirty glyph among painted runs, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
        assert!(
            runs.iter().any(|r| r.text.contains(crate::codicon::CLOSE)),
            "expected the default codicon close glyph among painted runs, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
    }

    /// Drives the override through [`crate::backend::Backend`]'s
    /// `draw_tab_bar_with_chrome_and_glyphs` method on
    /// [`crate::win::backend::WinBackend`] — the actual deliverable this
    /// module's free functions are wired behind — rather than calling
    /// [`draw_tab_bar_icons_with_glyphs`] (the free fn) directly, the way
    /// the test above does.
    #[test]
    fn backend_trait_close_dirty_glyph_override_paints_app_glyph_default_paints_codicon() {
        use crate::backend::Backend;
        use crate::win::backend::WinBackend;

        let mut dirty_bar = bar();
        dirty_bar.tabs[0].is_dirty = true;
        let rect = Rect::new(0.0, 0.0, W, H);
        let chrome = TabChrome::default();

        let paint = |glyphs: Option<&TabBarGlyphs>| {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let mut backend = WinBackend::new();
            backend
                .attach_headless(surface.target().clone(), W as u32, H as u32)
                .expect("attach headless surface");
            backend.set_painted_text_recording(true);
            backend.begin_frame(crate::event::Viewport::new(W, H, 1.0));
            #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
            let _ = backend
                .draw_tab_bar_with_chrome_and_glyphs(rect, &dirty_bar, None, &chrome, glyphs);
            backend.end_frame();
            backend.text_runs().to_vec()
        };

        let glyphs = TabBarGlyphs::new()
            .with_close_glyph("x")
            .with_dirty_glyph("*");
        let overridden = paint(Some(&glyphs));
        assert!(
            overridden.iter().any(|r| r.text == "*"),
            "expected the overridden dirty glyph among painted runs, got {:?}",
            overridden
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            overridden.iter().any(|r| r.text == "x"),
            "expected the overridden close glyph among painted runs, got {:?}",
            overridden
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(!overridden
            .iter()
            .any(|r| r.text.contains(crate::codicon::DIRTY)));
        assert!(!overridden
            .iter()
            .any(|r| r.text.contains(crate::codicon::CLOSE)));

        let default = paint(None);
        assert!(
            default
                .iter()
                .any(|r| r.text.contains(crate::codicon::DIRTY)),
            "expected the default codicon dirty glyph among painted runs, got {:?}",
            default.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
        assert!(
            default
                .iter()
                .any(|r| r.text.contains(crate::codicon::CLOSE)),
            "expected the default codicon close glyph among painted runs, got {:?}",
            default.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
    }

    /// `Style::tab_padding_x` sets the label's leading inset: with no icon
    /// decorating the tab, the label text run's painted `x` origin sits
    /// exactly `slot_x + tab_padding_x` from the tab's own slot edge, both
    /// at the default style and at a distinctly wider one — proving the
    /// token actually drives the paint rather than the assertion holding
    /// by coincidence at the default. Drives the real `Backend::draw_tab_bar`
    /// entry point on `WinBackend` — the same choke point
    /// `backend_trait_close_dirty_glyph_override_paints_app_glyph_default_paints_codicon`
    /// exercises above — via the painted-text-run recorder, rather than
    /// calling [`draw_tab_bar_icons_with_glyphs`] directly.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn tab_padding_x_sets_the_labels_leading_inset() {
        use crate::backend::Backend;
        use crate::win::backend::WinBackend;

        let mut single = bar();
        single.tabs.truncate(1);
        single.tabs[0].is_closable = false;
        single.show_tab_close = false;
        single.right_segments.clear();

        let label_x_for_style = |style: Style| -> (f32, f32) {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let mut backend = WinBackend::new();
            backend
                .attach_headless(surface.target().clone(), W as u32, H as u32)
                .expect("attach headless surface");
            backend.set_current_style(style);
            backend.set_painted_text_recording(true);
            backend.begin_frame(crate::event::Viewport::new(W, H, 1.0));
            let hits = backend.draw_tab_bar(Rect::new(0.0, 0.0, W, H), &single, None);
            backend.end_frame();
            let slot_x = hits.slot_positions[0].0 as f32;
            let runs = backend.text_runs().to_vec();
            let label_run = runs
                .iter()
                .find(|r| r.text == "main.rs")
                .expect("label must have painted a text run");
            (label_run.bounds.x, slot_x)
        };

        let default_style = Style::default();
        let (default_label_x, default_slot_x) = label_x_for_style(default_style);
        assert!(
            (default_label_x - (default_slot_x + default_style.tab_padding_x)).abs() < 0.01,
            "label x {default_label_x} should sit tab_padding_x={} in from slot edge {default_slot_x}",
            default_style.tab_padding_x,
        );

        // A distinctly different inset must move the label by exactly the
        // difference.
        let wide_style = Style::default().with_tab_padding_x(26.0);
        let (wide_label_x, wide_slot_x) = label_x_for_style(wide_style);
        assert!(
            (wide_label_x - (wide_slot_x + wide_style.tab_padding_x)).abs() < 0.01,
            "label x {wide_label_x} should sit tab_padding_x={} in from slot edge {wide_slot_x}",
            wide_style.tab_padding_x,
        );
        assert!(
            (wide_label_x - default_label_x - 16.0).abs() < 0.01,
            "widening tab_padding_x by 16.0 (10.0 -> 26.0) should shift the label \
             by exactly 16.0 (default {default_label_x}, wide {wide_label_x})",
        );
    }

    /// A label wider than the tab strip it has to fit in is ellipsis-fitted
    /// through the shared `crate::primitives::tab_bar::fit_tab_label`
    /// instead of being dropped from the strip. Drives the real
    /// `Backend::draw_tab_bar` entry point on `WinBackend`, matching the
    /// macOS sibling test `overlong_label_in_a_narrow_strip_ends_in_ellipsis_and_close_glyph_stays_fully_visible`.
    /// The close glyph's hit box — and the glyph itself — must still land
    /// fully inside the narrow strip.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn overlong_label_in_a_narrow_strip_ends_in_ellipsis_and_close_glyph_stays_fully_visible() {
        use crate::backend::Backend;
        use crate::win::backend::WinBackend;

        const NARROW_W: f32 = 110.0;

        let mut single = bar();
        single.tabs.truncate(1);
        single.tabs[0].label = "a_very_long_filename_for_this_single_tab.rs".into();
        single.right_segments.clear();

        let surface = HeadlessSurface::new(NARROW_W as u32, H as u32).expect("create surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), NARROW_W as u32, H as u32)
            .expect("attach headless surface");
        backend.set_painted_text_recording(true);
        backend.begin_frame(crate::event::Viewport::new(NARROW_W, H, 1.0));
        let hits = backend.draw_tab_bar(Rect::new(0.0, 0.0, NARROW_W, H), &single, None);
        backend.end_frame();
        let runs = backend.text_runs().to_vec();

        assert_eq!(
            hits.slot_positions.len(),
            1,
            "the narrow strip must still show the tab (ellipsis-fitted), not drop it"
        );

        let label_run = runs
            .iter()
            .find(|r| !r.text.contains(crate::codicon::CLOSE) && r.text.contains('…'))
            .unwrap_or_else(|| {
                panic!(
                    "expected an ellipsis-truncated label run among painted runs, got {:?}",
                    runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
                )
            });
        assert!(
            label_run.text.ends_with('…'),
            "label run {:?} must end in an ellipsis",
            label_run.text
        );
        assert!(
            (label_run.text.chars().count() as f64) < single.tabs[0].label.chars().count() as f64,
            "the painted label {:?} must be shorter than the original {:?}",
            label_run.text,
            single.tabs[0].label,
        );

        let (close_lo, close_hi) =
            hits.close_bounds[0].expect("a closable tab must still report a close-button hit box");
        assert!(
            close_hi <= NARROW_W as f64,
            "close hit box [{close_lo}, {close_hi}] must stay inside the {NARROW_W}-wide strip"
        );

        let close_run = runs
            .iter()
            .find(|r| r.text.contains(crate::codicon::CLOSE))
            .expect("the close glyph must still be painted");
        assert!(
            close_run.bounds.x >= 0.0 && close_run.bounds.x + close_run.bounds.width <= NARROW_W,
            "close glyph run {close_run:?} must be fully visible inside the {NARROW_W}-wide strip"
        );
    }

    /// `TabChrome::focused_index` must paint a focus-visible ring on the
    /// named tab — present after keyboard navigation (`Some(i)`), absent
    /// after a click (`None`, the caller's own focus-visible suppression
    /// convention documented on
    /// [`crate::primitives::tab_bar::TabChrome::focused_index`]). Mirrors
    /// `macos::tab_bar`'s/`gtk::tab_bar`'s twin tests: probes all four
    /// edges of the ring rect (the focused tab's own slot bounds —
    /// [`super::stroke_rect`] insets by half `style.focus_ring_width`
    /// itself, see this module's "Focus-visible ring" doc), so a
    /// regression that drops an edge fails here. Unlike the mac/gtk
    /// fixtures, win paints a tab's background across the full row
    /// height with no chip margin (see `paint_tab_bar_icons_from_layout`'s
    /// `tab_rect`), so the click-suppressed probes all fall on tab 0's
    /// own `tab_active_bg`, not a separate bar background. Drives the
    /// real `Backend::draw_tab_bar_with_chrome_and_glyphs` entry point on
    /// `WinBackend`.
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` half of the paint fn's return
    fn focused_index_paints_a_ring_after_keyboard_nav_and_none_after_a_click() {
        use crate::backend::Backend;
        use crate::win::backend::WinBackend;

        let bar_fixture = bar();
        let theme = Theme::default();
        let style = Style::default().with_focus_ring_width(4.0);
        let ring_color = theme.focus_ring_color();
        let active_bg = theme.tab_active_bg;
        let rect = Rect::new(0.0, 0.0, W, H);

        let paint = |focused_index: Option<usize>| -> (HeadlessSurface, TabBarHits) {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let mut backend = WinBackend::new();
            backend
                .attach_headless(surface.target().clone(), W as u32, H as u32)
                .expect("attach headless surface");
            backend.set_current_style(style);
            backend.begin_frame(crate::event::Viewport::new(W, H, 1.0));
            let chrome = TabChrome::default().with_focused_index(focused_index);
            let hits = backend.draw_tab_bar_with_chrome_and_glyphs(
                rect,
                &bar_fixture,
                None,
                &chrome,
                None,
            );
            backend.end_frame();
            (surface, hits)
        };

        // Keyboard focus on tab 0 ("main.rs", the active and only visible
        // tab at this index): all four edges of the inset ring rect must
        // carry `theme.focus_ring_color()`.
        let (focused_surface, focused_hits) = paint(Some(0));
        let (slot_x, slot_end) = focused_hits.slot_positions[0];
        let tab_content_w = slot_end - slot_x - TAB_OUTER_GAP_DIP as f64;
        let ring_width = style.focus_ring_width as f64;
        let inset = (ring_width / 2.0)
            .min(tab_content_w / 2.0)
            .min(H as f64 / 2.0)
            .max(0.0);
        let left = (slot_x + inset).round() as u32;
        let right = (slot_x + tab_content_w - inset).round() as u32;
        let top = inset.round() as u32;
        let bottom = (H as f64 - inset).round() as u32;
        let mid_y = (H / 2.0) as u32;
        // A column inside the tab's trailing padding — right of the
        // "main.rs" label's own ink — rather than the tab's horizontal
        // centre, which the painted label occupies.
        let label_clear_x = (slot_x + tab_content_w - 5.0).round() as u32;

        let probes = [
            ("left", left, mid_y),
            ("right", right, mid_y),
            ("top", label_clear_x, top),
            ("bottom", label_clear_x, bottom),
        ];

        for (name, px, py) in probes {
            let got = focused_surface.pixel_at(px, py);
            assert_eq!(
                (got.r, got.g, got.b),
                (ring_color.r, ring_color.g, ring_color.b),
                "expected the focus ring colour at the {name} edge ({px}, {py}) \
                 when TabChrome::focused_index names tab 0",
            );
        }

        // Click suppresses focus-visible: `focused_index: None` must
        // paint whatever sits under each edge absent a ring — tab 0's own
        // `tab_active_bg`, which fills the whole row height here.
        let (clicked_surface, _clicked_hits) = paint(None);
        for (name, px, py) in probes {
            let got = clicked_surface.pixel_at(px, py);
            assert_eq!(
                (got.r, got.g, got.b),
                (active_bg.r, active_bg.g, active_bg.b),
                "expected no focus ring at the {name} edge ({px}, {py}) when \
                 TabChrome::focused_index is None (click-suppressed)",
            );
        }
    }

    /// [`crate::Backend::draw_tab_bar_with_chrome`] and
    /// [`crate::Backend::draw_tab_bar_with_chrome_layout`] are the entry
    /// points the gallery and `quadraui-binding-core`'s render path
    /// actually call — not just the `_and_glyphs` spelling the test
    /// above exercises. Both must paint `chrome.focused_index`'s ring
    /// too.
    #[test]
    #[allow(deprecated)] // `draw_tab_bar_with_chrome` returns the deprecated `TabBarHits`
    fn draw_tab_bar_with_chrome_and_its_layout_twin_both_paint_the_focus_ring() {
        use crate::backend::Backend;
        use crate::win::backend::WinBackend;

        let bar_fixture = bar();
        let theme = Theme::default();
        let style = Style::default();
        let ring_color = theme.focus_ring_color();
        let chrome = TabChrome::default().with_focused_index(Some(0));
        let rect = Rect::new(0.0, 0.0, W, H);

        // `Style::focus_ring_width` default (2.0, inset by half): sample
        // at the ring's vertical centreline on the left edge of tab 0's
        // slot.
        let ring_width = style.focus_ring_width as f64;
        let probe_x = (ring_width / 2.0).round() as u32;
        let probe_y = (H / 2.0) as u32;

        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W as u32, H as u32)
            .expect("attach headless surface");
        backend.begin_frame(crate::event::Viewport::new(W, H, 1.0));
        let _ = backend.draw_tab_bar_with_chrome(rect, &bar_fixture, None, &chrome);
        backend.end_frame();
        let got = surface.pixel_at(probe_x, probe_y);
        assert_eq!(
            (got.r, got.g, got.b),
            (ring_color.r, ring_color.g, ring_color.b),
            "draw_tab_bar_with_chrome must paint chrome.focused_index's ring"
        );

        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W as u32, H as u32)
            .expect("attach headless surface");
        backend.begin_frame(crate::event::Viewport::new(W, H, 1.0));
        let _ = backend.draw_tab_bar_with_chrome_layout(rect, &bar_fixture, None, &chrome);
        backend.end_frame();
        let got = surface.pixel_at(probe_x, probe_y);
        assert_eq!(
            (got.r, got.g, got.b),
            (ring_color.r, ring_color.g, ring_color.b),
            "draw_tab_bar_with_chrome_layout must paint chrome.focused_index's ring"
        );
    }
}
