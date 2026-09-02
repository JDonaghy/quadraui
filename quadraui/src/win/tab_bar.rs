//! Direct2D / DirectWrite rasteriser for [`crate::TabBar`] (issue #25).
//!
//! Calls [`TabBar::layout`] (the D6 layout API) with DirectWrite pixel
//! measurers, then paints from the resolved `visible_tabs` /
//! `visible_segments`. Converts to [`TabBarHits`] via the shared
//! [`crate::backend::tab_bar_layout_to_hits`] / `shift_tab_bar_hits`
//! helpers, the same ones the TUI and GTK backends use — see
//! `Backend::tab_bar_layout`'s doc for why that shift matters (issue
//! #552).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod tab_bar;` and `backend.rs`'s module
//! docs. See `win::status_bar`'s module doc for why colours come from
//! `Theme::default()` rather than a live `WinBackend` theme field.
//!
//! Scope for #25: no [`crate::TabChrome`] / bracket-frame support (the
//! `Backend` trait gives `draw_tab_bar_with_chrome` /
//! `tab_bar_layout_with_chrome` default bodies that fall back to the
//! plain methods below, so this is not a compile-error gap — see those
//! methods' docs) and no italic preview-tab styling (would need a second
//! `IDWriteTextFormat`; deferred to a follow-up rather than widening this
//! issue).

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::backend::{shift_tab_bar_hits, tab_bar_layout_to_hits};
use crate::event::Rect;
use crate::theme::Theme;
use crate::{tab_icon_at, SegmentMeasure, TabBar, TabBarHits, TabBarLayout, TabIcon, TabMeasure};

/// Left+right padding (DIPs) inside a tab's background fill.
const TAB_PAD_DIP: f32 = 14.0;
/// Gap (DIPs) between a tab's label and its close glyph.
const TAB_INNER_GAP_DIP: f32 = 10.0;
/// Gap (DIPs) between adjacent tabs.
const TAB_OUTER_GAP_DIP: f32 = 1.0;
/// Gap (DIPs) between a tab's icon glyph ([`crate::TabIcon`]) and its label.
const TAB_ICON_GAP_DIP: f32 = 6.0;
/// Height (DIPs) of the active tab's top-edge accent line.
const TAB_ACTIVE_ACCENT_DIP: f32 = 2.0;

fn close_glyph_width(dwrite: &DWrite, bar: &TabBar) -> f32 {
    if bar.show_tab_close {
        dwrite.measure_text("×").map(|(w, _)| w).unwrap_or(0.0)
    } else {
        0.0
    }
}

fn icon_extra_width(dwrite: &DWrite, icons: &[Option<TabIcon>], i: usize) -> f32 {
    match tab_icon_at(icons, i) {
        Some(icon) => {
            let (w, _) = dwrite.measure_text(&icon.glyph).unwrap_or((0.0, 0.0));
            w + TAB_ICON_GAP_DIP
        }
        None => 0.0,
    }
}

/// Compute the [`TabBarLayout`] for `bar` against `rect`'s dimensions,
/// measuring every tab/segment via `dwrite`. Shared by every paint and
/// no-paint entry point in this module so they can never disagree on
/// geometry.
fn compute_layout(
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
) -> TabBarLayout {
    let close_w = close_glyph_width(dwrite, bar);
    let measure_tab = |i: usize| -> TabMeasure {
        let tab = &bar.tabs[i];
        let (name_w, _) = dwrite.measure_text(&tab.label).unwrap_or((0.0, 0.0));
        let icon_extra = icon_extra_width(dwrite, icons, i);
        let has_close = bar.show_tab_close && tab.is_closable;
        let close_extra = if has_close {
            TAB_INNER_GAP_DIP + close_w
        } else {
            0.0
        };
        let total =
            TAB_PAD_DIP + icon_extra + name_w + close_extra + TAB_PAD_DIP + TAB_OUTER_GAP_DIP;
        let close_region_w = if has_close {
            TAB_INNER_GAP_DIP + close_w + TAB_PAD_DIP + TAB_OUTER_GAP_DIP
        } else {
            0.0
        };
        TabMeasure::new(total, close_region_w)
    };
    let measure_segment = |i: usize| -> SegmentMeasure {
        let (w, _) = dwrite
            .measure_text(&bar.right_segments[i].text)
            .unwrap_or((0.0, 0.0));
        SegmentMeasure::new(w)
    };
    bar.layout(rect.width, rect.height, 0.0, measure_tab, measure_segment)
}

/// Recompute the scroll offset that would make the active tab visible
/// given this frame's actual DirectWrite measurements — the "engine
/// feedback" half of the two-pass-paint pattern
/// [`TabBar::layout`]'s doc describes (scroll arrows are disabled here,
/// via `scroll_arrow_width: 0.0`, so [`TabBar::layout`] itself just
/// honours `bar.scroll_offset` verbatim rather than correcting it).
fn correct_scroll_offset(
    dwrite: &DWrite,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    effective_tab_area: f32,
) -> usize {
    let close_w = close_glyph_width(dwrite, bar);
    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    match active_idx {
        Some(active) => {
            let widths: Vec<usize> = (0..bar.tabs.len())
                .map(|i| {
                    let (name_w, _) = dwrite
                        .measure_text(&bar.tabs[i].label)
                        .unwrap_or((0.0, 0.0));
                    let icon_extra = icon_extra_width(dwrite, icons, i);
                    let has_close = bar.show_tab_close && bar.tabs[i].is_closable;
                    let close_extra = if has_close {
                        TAB_INNER_GAP_DIP + close_w
                    } else {
                        0.0
                    };
                    (TAB_PAD_DIP * 2.0 + icon_extra + name_w + close_extra + TAB_OUTER_GAP_DIP)
                        .ceil() as usize
                })
                .collect();
            TabBar::fit_active_scroll_offset(
                active,
                bar.tabs.len(),
                effective_tab_area.max(0.0) as usize,
                |i| widths[i],
            )
        }
        None => bar.scroll_offset,
    }
}

fn hits_from_layout(
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    layout: &TabBarLayout,
) -> TabBarHits {
    let mut hits = tab_bar_layout_to_hits(layout, bar);
    shift_tab_bar_hits(&mut hits, rect.x as f64);
    let seg_reserved: f32 = layout
        .visible_segments
        .iter()
        .map(|vs| vs.bounds.width)
        .sum();
    hits.correct_scroll_offset =
        correct_scroll_offset(dwrite, bar, icons, rect.width - seg_reserved);
    hits
}

/// Compute a [`TabBar`]'s layout without painting, for a bar decorated
/// with per-tab icons (#620) — the no-paint twin of
/// [`draw_tab_bar_icons`]. `&[]` reproduces [`win_tab_bar_layout`].
pub fn win_tab_bar_layout_icons(
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
) -> TabBarHits {
    let layout = compute_layout(dwrite, rect, bar, icons);
    hits_from_layout(dwrite, rect, bar, icons, &layout)
}

/// Compute a [`TabBar`]'s layout without painting — the icon-less twin of
/// [`win_tab_bar_layout_icons`].
pub fn win_tab_bar_layout(dwrite: &DWrite, rect: Rect, bar: &TabBar) -> TabBarHits {
    win_tab_bar_layout_icons(dwrite, rect, bar, &[])
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
pub fn draw_tab_bar_icons(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    icons: &[Option<TabIcon>],
    hovered_close_tab: Option<usize>,
) -> TabBarHits {
    let theme = Theme::default();
    let _ = fill_rect(target, rect, theme.tab_bar_bg);

    let layout = compute_layout(dwrite, rect, bar, icons);
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

    hits_from_layout(dwrite, rect, bar, icons, &layout)
}

/// Draw a [`TabBar`] with no per-tab icons — [`draw_tab_bar_icons`] with
/// `icons: &[]`.
pub fn draw_tab_bar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &TabBar,
    hovered_close_tab: Option<usize>,
) -> TabBarHits {
    draw_tab_bar_icons(target, dwrite, rect, bar, &[], hovered_close_tab)
}

#[cfg(test)]
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
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
}
