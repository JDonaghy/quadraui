//! Direct2D / DirectWrite rasteriser for [`crate::TabBar`] (issue #25).
//!
//! Calls [`TabBar::layout`] (the D6 layout API) with DirectWrite pixel
//! measurers, then paints from the resolved `visible_tabs` /
//! `visible_segments`. Converts to [`TabBarHits`] via the shared
//! [`crate::backend::tab_bar_hits_from_layout`] / `shift_tab_bar_hits`
//! helpers, the same ones the TUI and GTK backends use — see
//! `Backend::tab_bar_layout`'s doc for why that shift matters (issue
//! #552).
//!
//! Issue #1078: only the actual Direct2D paint entry points
//! ([`draw_tab_bar`], [`draw_tab_bar_icons`], [`draw_tab_bar_layout`],
//! [`draw_tab_bar_icons_layout`], and their shared paint loop) are
//! `#[cfg(target_os = "windows")]`-gated. [`compute_layout`] and every
//! `win_tab_bar_*` no-paint fn below are pure geometry generic over
//! [`crate::primitives::layout_metrics::TextMeasure`] — no Direct2D/
//! DirectWrite type in their signature — so they compile and run
//! everywhere, including a plain `cargo test --features win` on Linux.
//! `super::mod`'s `mod tab_bar;` is no longer whole-module gated; see
//! `backend.rs`'s module docs. See `win::status_bar`'s module doc for why
//! colours come from `Theme::default()` rather than a live `WinBackend`
//! theme field.
//!
//! Scope for #25: no [`crate::TabChrome`] / bracket-frame support (the
//! `Backend` trait gives `draw_tab_bar_with_chrome` /
//! `tab_bar_layout_with_chrome` default bodies that fall back to the
//! plain methods below, so this is not a compile-error gap — see those
//! methods' docs) and no italic preview-tab styling (would need a second
//! `IDWriteTextFormat`; deferred to a follow-up rather than widening this
//! issue).
//!
//! Still a per-backend paint loop: **#1081 did NOT migrate `TabBar`.**
//! Unlike `win::menu_bar` / `win::toolbar` / `win::activity_bar`, which
//! `NativeSurface` Phase 4 slice 5/8 collapsed into
//! `primitives::<name>::native_surface_paint::paint`, every pixel
//! `paint_tab_bar_icons_from_layout` below draws is still Direct2D-
//! specific and still triplicated with `gtk::tab_bar` / `macos::tab_bar`
//! — the two scope gaps just above (no bracket chrome, no italic preview
//! styling; this module also ignores `theme.tab_preview_*_fg` entirely)
//! are three of the five live drifts between those copies. See
//! [`crate::primitives::tab_bar`]'s "`NativeSurface` migration status"
//! section for the full drift table, the macOS `close_bounds`-convention
//! prerequisite that blocks the move, and why #1081 stays open rather
//! than closing as complete.

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

#[cfg(target_os = "windows")]
use super::text::fill_rect;
#[cfg(target_os = "windows")]
use super::text::DWrite;
use crate::backend::{shift_tab_bar_hits, tab_bar_hits_from_layout};
use crate::event::Rect;
use crate::primitives::layout_metrics::TextMeasure;
#[cfg(target_os = "windows")]
use crate::theme::Theme;
// `TabBarHits` is `#[deprecated]` (issue #823) — this module still
// narrows its `TabBarLayout` down to that struct because the six
// `Backend` tab-bar methods `WinBackend` implements still return it, so
// the import needs the same allow every use site below does.
#[allow(deprecated)]
use crate::{tab_icon_at, TabBar, TabBarHits, TabBarLayout, TabChrome, TabIcon};

/// Left+right padding (DIPs) inside a tab's background fill.
const TAB_PAD_DIP: f32 = 14.0;
/// Gap (DIPs) between a tab's label and its close glyph.
const TAB_INNER_GAP_DIP: f32 = 10.0;
/// Gap (DIPs) between adjacent tabs.
const TAB_OUTER_GAP_DIP: f32 = 1.0;
/// Gap (DIPs) between a tab's icon glyph ([`crate::TabIcon`]) and its label.
const TAB_ICON_GAP_DIP: f32 = 6.0;
/// Height (DIPs) of the active tab's top-edge accent line. Only used by
/// the paint path (issue #1078).
#[cfg(target_os = "windows")]
const TAB_ACTIVE_ACCENT_DIP: f32 = 2.0;

/// Only used by the paint loop below (for hover-rect/close-glyph
/// positioning) — the shared [`pixel_layout`] re-measures the same glyph
/// itself via `measure`, so paint and layout can never disagree about its
/// width even though each measures it once.
#[cfg(target_os = "windows")]
fn close_glyph_width(measure: &dyn TextMeasure, bar: &TabBar) -> f32 {
    if bar.show_tab_close {
        measure.width_of("×")
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

/// Compute `(layout, corrected_scroll_offset, available_cols)` for `bar`
/// against `rect`'s dimensions, measuring every tab/segment via
/// `measure`. Shared by every paint and no-paint entry point in this
/// module so they can never disagree on geometry.
///
/// Issue #1080: this module's own `measure_tab`/`measure_segment`/
/// `correct_scroll_offset` composition used to duplicate exactly what
/// `gtk::tab_bar`/`macos::tab_bar` also each computed independently —
/// now a thin wrapper over the shared
/// [`crate::primitives::layout_metrics::pixel_tab_bar_layout`]. Windows
/// has no bracket-chrome support yet (see this module's "Scope" doc), so
/// `chrome` is always [`TabChrome::default`]. Pure geometry over
/// [`TextMeasure`] (issue #1078) — no Direct2D/DirectWrite type needed,
/// so [`super::backend::WinBackend`] can call this directly with a
/// [`super::backend`]-local nominal measurer when no live `DWrite`
/// handle exists yet, instead of carrying a separate duplicate.
fn pixel_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
) -> (TabBarLayout, usize, usize) {
    let tab_name_widths: Vec<f32> = bar
        .tabs
        .iter()
        .map(|t| measure.width_of(&t.label))
        .collect();
    let icon_extras: Vec<f32> = (0..bar.tabs.len())
        .map(|i| icon_extra_width(measure, icons, i))
        .collect();
    crate::primitives::layout_metrics::pixel_tab_bar_layout(
        bar,
        rect.width,
        rect.height,
        TAB_PAD_DIP,
        TAB_INNER_GAP_DIP,
        TAB_OUTER_GAP_DIP,
        &tab_name_widths,
        &icon_extras,
        &TabChrome::default(),
        measure,
    )
}

/// Compute the [`TabBarLayout`] half of [`pixel_layout`] — the icon-less
/// callers below only need the layout, not the scroll-offset/
/// available-cols engine feedback.
fn compute_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
) -> TabBarLayout {
    pixel_layout(measure, rect, bar, icons).0
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
/// with per-tab icons (#620) — the no-paint twin of
/// [`draw_tab_bar_icons`]. `&[]` reproduces [`win_tab_bar_layout`].
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub fn win_tab_bar_layout_icons(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
) -> TabBarHits {
    let (layout, corrected_scroll_offset, available_cols) = pixel_layout(measure, rect, bar, icons);
    hits_from_layout(rect, bar, &layout, corrected_scroll_offset, available_cols)
}

/// Compute a [`TabBar`]'s layout without painting — the icon-less twin of
/// [`win_tab_bar_layout_icons`].
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub fn win_tab_bar_layout(measure: &dyn TextMeasure, rect: Rect, bar: &TabBar) -> TabBarHits {
    win_tab_bar_layout_icons(measure, rect, bar, &[])
}

/// Compute a [`TabBar`]'s [`TabBarLayout`] without painting, for a bar
/// decorated with per-tab icons — issue #919's `TabBarLayout`-returning
/// counterpart to [`win_tab_bar_layout_icons`]. Unlike macOS's
/// `mac_tab_bar_native_layout`, no duplicated measurement code is
/// needed here: [`compute_layout`] already *is* the single measurement
/// path both [`draw_tab_bar_icons`] and [`win_tab_bar_layout_icons`]
/// narrow down to `TabBarHits` — this just returns it directly.
pub fn win_tab_bar_native_layout_icons(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
) -> TabBarLayout {
    compute_layout(measure, rect, bar, icons)
}

/// Compute a [`TabBar`]'s [`TabBarLayout`] without painting — the
/// icon-less twin of [`win_tab_bar_native_layout_icons`].
pub fn win_tab_bar_native_layout(
    measure: &dyn TextMeasure,
    rect: Rect,
    bar: &TabBar,
) -> TabBarLayout {
    win_tab_bar_native_layout_icons(measure, rect, bar, &[])
}

/// Draw a [`TabBar`] with per-tab icon glyphs (#620) into `rect` (DIPs)
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
/// - **Dirty tab:** close glyph is `●` instead of `×` (suppressed while
///   hovered, so the hover state always shows `×` to close).
/// - **Right segments:** painted in `tab_inactive_fg`, or `tab_active_fg`
///   when `seg.is_active`.
#[cfg(target_os = "windows")]
#[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
pub fn draw_tab_bar_icons(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    hovered_close_tab: Option<usize>,
) -> TabBarHits {
    let (layout, corrected_scroll_offset, available_cols) = pixel_layout(dwrite, rect, bar, icons);
    paint_tab_bar_icons_from_layout(target, dwrite, rect, bar, icons, hovered_close_tab, &layout);
    hits_from_layout(rect, bar, &layout, corrected_scroll_offset, available_cols)
}

/// Draw a [`TabBar`] with per-tab icon glyphs, returning [`TabBarLayout`]
/// instead of the deprecated [`TabBarHits`] (issue #919) — the
/// `TabBarLayout`-returning counterpart to [`draw_tab_bar_icons`] above.
/// Shares [`compute_layout`] and [`paint_tab_bar_icons_from_layout`] with
/// it, so the two can never paint different pixels — only the return
/// value differs.
#[cfg(target_os = "windows")]
pub fn draw_tab_bar_icons_layout(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    hovered_close_tab: Option<usize>,
) -> TabBarLayout {
    let layout = compute_layout(dwrite, rect, bar, icons);
    paint_tab_bar_icons_from_layout(target, dwrite, rect, bar, icons, hovered_close_tab, &layout);
    layout
}

/// Shared paint loop for [`draw_tab_bar_icons`] / [`draw_tab_bar_icons_layout`]
/// — paints `bar` from a pre-computed `layout` and returns nothing, so
/// both callers can hand back whichever return type (`TabBarHits` vs.
/// `TabBarLayout`) their contract needs.
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
) {
    let theme = Theme::default();
    let _ = fill_rect(target, rect, theme.tab_bar_bg);

    let close_w = close_glyph_width(dwrite, bar);

    for vt in &layout.visible_tabs {
        let tab = &bar.tabs[vt.tab_idx];
        let visual_w = (vt.bounds.width - TAB_OUTER_GAP_DIP).max(0.0);
        let tab_rect = Rect::new(
            rect.x + vt.bounds.x,
            rect.y + vt.bounds.y,
            visual_w,
            vt.bounds.height,
        );

        let bg = if tab.is_active {
            theme.tab_active_bg
        } else {
            theme.tab_bar_bg
        };
        let _ = fill_rect(target, tab_rect, bg);

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
        let mut cursor_x = tab_rect.x + TAB_PAD_DIP;

        if let Some(icon) = tab_icon_at(icons, vt.tab_idx) {
            let (iw, ih) = dwrite.measure_text(&icon.glyph).unwrap_or((0.0, 0.0));
            let icon_rect = Rect::new(cursor_x, tab_rect.y + (tab_rect.height - ih) / 2.0, iw, ih);
            let _ = dwrite.draw_text(target, &icon.glyph, icon_rect, icon.color);
            cursor_x += iw + TAB_ICON_GAP_DIP;
        }

        let (name_w, name_h) = dwrite.measure_text(&tab.label).unwrap_or((0.0, 0.0));
        let label_rect = Rect::new(
            cursor_x,
            tab_rect.y + (tab_rect.height - name_h) / 2.0,
            name_w,
            name_h,
        );
        let _ = dwrite.draw_text(target, &tab.label, label_rect, fg);

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
                let (cgw, cgh) = dwrite.measure_text(close_glyph).unwrap_or((0.0, 0.0));
                let close_rect = Rect::new(
                    close_x,
                    tab_rect.y + (tab_rect.height - cgh) / 2.0,
                    cgw,
                    cgh,
                );
                let _ = dwrite.draw_text(target, close_glyph, close_rect, close_fg);
            }
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
) -> TabBarHits {
    draw_tab_bar_icons(target, dwrite, rect, bar, &[], hovered_close_tab)
}

/// Draw a [`TabBar`] with no per-tab icons, returning [`TabBarLayout`]
/// instead of the deprecated [`TabBarHits`] (issue #919) —
/// [`draw_tab_bar_icons_layout`] with `icons: &[]`.
#[cfg(target_os = "windows")]
pub fn draw_tab_bar_layout(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    hovered_close_tab: Option<usize>,
) -> TabBarLayout {
    draw_tab_bar_icons_layout(target, dwrite, rect, bar, &[], hovered_close_tab)
}

// #1078: every test below paints through a real `DWrite`/`HeadlessSurface`
// (the module's paint fns are now the only Windows-only parts, but these
// specific tests all exercise them) — gated the same way the whole
// module used to be, rather than pretending they run on Linux. A
// cross-platform-safe pure-geometry test lives in `win::list`'s test mod
// instead (issue #1078's acceptance test), since `list_layout` is where
// the drift this issue fixes actually was.
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

        surface
            .paint(|target| {
                draw_tab_bar(target, &dwrite, rect, &bar, None);
            })
            .expect("paint tab bar");

        let layout = compute_layout(&dwrite, rect, &bar, &[]);
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
        let hits = win_tab_bar_layout(&dwrite, rect, &bar);
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

        let surface = HeadlessSurface::new((W + 5.0) as u32, H as u32).expect("create surface");
        let mut painted = None;
        surface
            .paint(|target| {
                painted = Some(draw_tab_bar(target, &dwrite, rect, &bar, None));
            })
            .expect("paint");
        let painted = painted.expect("draw_tab_bar ran");
        let no_paint = win_tab_bar_layout(&dwrite, rect, &bar);

        assert_eq!(painted, no_paint);
    }

    /// Issue #919: the new `TabBarLayout`-returning fns
    /// (`draw_tab_bar_layout` / `win_tab_bar_native_layout`) must agree
    /// with each other exactly as their `TabBarHits`-returning siblings
    /// do above — same bar, same rect, same measurer. Since both new
    /// fns are thin wrappers over the shared `compute_layout` this is
    /// mostly a wiring check, but it pins the paint path's `layout`
    /// (post-paint) against the no-paint path's independently-called
    /// `compute_layout` so the two can't silently diverge later.
    #[test]
    fn native_layout_no_paint_matches_native_layout_paint() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(5.0, 0.0, W, H);

        let surface = HeadlessSurface::new((W + 5.0) as u32, H as u32).expect("create surface");
        let mut painted = None;
        surface
            .paint(|target| {
                painted = Some(draw_tab_bar_layout(target, &dwrite, rect, &bar, None));
            })
            .expect("paint");
        let painted = painted.expect("draw_tab_bar_layout ran");
        let no_paint = win_tab_bar_native_layout(&dwrite, rect, &bar);

        assert_eq!(painted, no_paint);
    }

    /// The `TabBarLayout`-returning accessor and the deprecated
    /// `TabBarHits`-returning one must report the same geometry —
    /// converting the former through the shared `tab_bar_hits_from_layout`
    /// helper and comparing field-by-field against the latter's direct
    /// output. This is the "twin" pattern issue #919 asks every backend
    /// to cover; the other three each get their own version of this
    /// test (`gtk::testing::tab_bar_layout_twin_matches_painted_geometry_headless`,
    /// its TUI counterpart, and macOS's `native_layout_agrees_with_hits_layout`).
    #[test]
    #[allow(deprecated)] // exercises the deprecated `TabBarHits` — issue #823
    fn native_layout_agrees_with_hits_layout() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let bar = bar();
        let rect = Rect::new(0.0, 0.0, W, H);

        let hits = win_tab_bar_layout(&dwrite, rect, &bar);
        let native = win_tab_bar_native_layout(&dwrite, rect, &bar);
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
}
