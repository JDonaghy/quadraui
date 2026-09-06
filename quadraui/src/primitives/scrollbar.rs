//! `Scrollbar` primitive: a thin track-and-thumb indicator showing the
//! visible window's position within a larger scrollable region.
//!
//! Used by editor viewports (vertical line scroll, horizontal column
//! scroll), tab-group viewports, and any panel with overflowing content.
//! The primitive is intentionally data-only: it carries pre-computed
//! `thumb_start` + `thumb_len` positions along the track, in the same
//! units as the rasteriser's surface (cells for TUI, pixels for GTK).
//!
//! ## Math
//!
//! Two backends in this crate compute thumb geometry slightly
//! differently — the TUI vertical scrollbar uses
//! `thumb_start = floor(scroll/total * track_len)` (cell precision,
//! offset proportional to scroll/total), while GTK's overlay uses
//! `thumb_start = (scroll/(total-visible)) * (track_len-thumb_len)` with
//! a 20-pixel minimum thumb. Both shapes are valid; this crate doesn't
//! force one over the other.
//!
//! [`fit_thumb`] offers a single canonical helper that some backends
//! consume directly. Backends with subtly different conventions may
//! ignore the helper and supply their own pre-computed geometry to
//! [`Scrollbar`]; the rasteriser only paints, never measures.

use crate::event::Rect;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Orientation of a scrollbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScrollAxis {
    Vertical,
    Horizontal,
}

/// Declarative description of a scrollbar.
///
/// Coordinate units in `track`, `thumb_start`, and `thumb_len` are
/// surface-native (TUI cells, GTK pixels). `thumb_start` is an offset
/// from the track's leading edge along `axis`; `thumb_len` is the
/// thumb's length along `axis`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scrollbar {
    pub id: WidgetId,
    pub axis: ScrollAxis,
    pub track: Rect,
    pub thumb_start: f32,
    pub thumb_len: f32,
    /// Cursor is hovering over the scrollbar — rasterisers may brighten
    /// the thumb. Default `false`.
    #[serde(default)]
    pub hovered: bool,
    /// User is actively dragging the thumb — rasterisers may apply an
    /// even brighter / fully opaque highlight. Default `false`.
    #[serde(default)]
    pub dragging: bool,
}

impl Scrollbar {
    /// Build a vertical scrollbar with thumb geometry computed via
    /// [`fit_thumb`].
    pub fn vertical(
        id: impl Into<WidgetId>,
        track: Rect,
        scroll: f32,
        total: f32,
        visible: f32,
        min_thumb_len: f32,
    ) -> Self {
        let (thumb_start, thumb_len) =
            fit_thumb(scroll, total, visible, track.height, min_thumb_len);
        Self {
            id: id.into(),
            axis: ScrollAxis::Vertical,
            track,
            thumb_start,
            thumb_len,
            hovered: false,
            dragging: false,
        }
    }

    /// Build a horizontal scrollbar with thumb geometry computed via
    /// [`fit_thumb`].
    pub fn horizontal(
        id: impl Into<WidgetId>,
        track: Rect,
        scroll: f32,
        total: f32,
        visible: f32,
        min_thumb_len: f32,
    ) -> Self {
        let (thumb_start, thumb_len) =
            fit_thumb(scroll, total, visible, track.width, min_thumb_len);
        Self {
            id: id.into(),
            axis: ScrollAxis::Horizontal,
            track,
            thumb_start,
            thumb_len,
            hovered: false,
            dragging: false,
        }
    }
}

/// Canonical scrollbar thumb-fitting math.
///
/// Sizes the thumb proportional to `visible / total`, clamped to
/// `[min_thumb_len, track_len]`. Positions the thumb so it travels the
/// remaining `track_len - thumb_len` linearly with `scroll` over its
/// available range `(total - visible)`. When `total <= visible` the
/// scrollbar has no work to do; both outputs are zero.
///
/// All values are in the same surface units. Backends that need cell-
/// precise rounding (e.g. TUI) typically `.floor()` / `.ceil()` the
/// returned values themselves.
pub fn fit_thumb(
    scroll: f32,
    total: f32,
    visible: f32,
    track_len: f32,
    min_thumb_len: f32,
) -> (f32, f32) {
    if total <= 0.0 || track_len <= 0.0 || visible <= 0.0 || total <= visible {
        return (0.0, 0.0);
    }
    let raw_len = (visible / total) * track_len;
    let thumb_len = raw_len.max(min_thumb_len).min(track_len);
    let scroll_range = (total - visible).max(1.0);
    let travel = (track_len - thumb_len).max(0.0);
    let thumb_start = (scroll / scroll_range).clamp(0.0, 1.0) * travel;
    (thumb_start, thumb_len)
}

/// Clamp a (possibly stale) `scroll_offset` into a valid starting index
/// for `content_len` items.
///
/// Returns `0` when there is nothing to scroll to (`content_len == 0`),
/// otherwise `scroll_offset.min(content_len - 1)` — pinning the start so
/// the last item is always reachable instead of scrolling clean past the
/// end into an empty view. This is the "resolved scroll offset" formula
/// `TreeView`, `ListView`, `Palette`, `Completions`, `Form`, and
/// `TextDisplay` layouts each re-derived independently before #508.
///
/// Callers that render a fixed number of rows per frame (rather than
/// filling a variable-height viewport item-by-item) usually want
/// [`visible_window`] instead, which layers the end-of-window clamp on
/// top of this.
pub fn clamp_scroll_offset(scroll_offset: usize, content_len: usize) -> usize {
    if content_len == 0 {
        0
    } else {
        scroll_offset.min(content_len - 1)
    }
}

/// Visible window `[start, end)` into a `content_len`-item list, given a
/// (possibly stale) `scroll_offset` and how many rows fit in the
/// viewport (`viewport_rows` — typically `(body_len / row_step).floor()`,
/// computed by the caller since the step size is surface- and
/// primitive-specific).
///
/// `start` is [`clamp_scroll_offset`]; `end` is `start + viewport_rows`
/// clamped to `content_len`. Returns `(0, 0)` when `content_len == 0`.
pub fn visible_window(
    scroll_offset: usize,
    content_len: usize,
    viewport_rows: usize,
) -> (usize, usize) {
    if content_len == 0 {
        return (0, 0);
    }
    let start = clamp_scroll_offset(scroll_offset, content_len);
    let end = (start + viewport_rows).min(content_len);
    (start, end)
}

// ── NativeSurface paint (#811, Phase 2d of the NativeSurface milestone) ────
//
// Before this, `gtk::draw_scrollbar` (Cairo), `macos::scrollbar::draw_scrollbar`
// (Core Graphics) and `win::scrollbar::draw_scrollbar` (Direct2D) each
// independently painted the same overlay track+thumb geometry with their
// own drawing API (quadraui#785 child #811, `docs/SMELL_AUDIT_2026-07.md`
// §5). `paint` below is the one shared implementation, written against
// [`crate::native_surface::NativeSurface`] (#807, Phase 1) instead of any
// one backend's drawing API.
//
// Unlike `primitives::chart`'s divergence-heavy unification (#810), the
// three deleted copies here were already near-identical: same
// track/thumb alpha thresholds, same axis-based rect placement. The one
// divergence this issue names as "known, owned elsewhere" —
// `win::scrollbar` pre-blending its fill against `theme.background`
// instead of a real alpha blend (quadraui#791) — had **already been
// fixed**, independently of this issue, before this migration started:
// the pre-deletion `win/scrollbar.rs` module doc documented the fix
// landing under #791 directly (a real translucent `ID2D1SolidColorBrush`
// fill, matching GTK's `cr.set_source_rgba` and macOS's
// `CGContextSetRGBFillColor` with a real alpha channel). Re-verified
// while migrating, per this issue's "re-verify before you implement" —
// reported here rather than silently assumed.
//
// What #791 did *not* reach, because the code didn't exist yet:
// `NativeSurface::surface_fill_rect`'s own GTK implementation (added
// later, by the #808/#810 migrations that gave `primitives::{chart,form,
// terminal,text_display}` a shared paint path) called
// `crate::gtk::set_source` — `cr.set_source_rgb`, which drops `Color::a`
// outright. Routing `Scrollbar`'s paint through `surface_fill_rect`
// unchanged would have *reintroduced* an opaque-fill regression on GTK
// specifically, on the one primitive whose entire visual identity is a
// translucent overlay. Fixed at the source instead of worked around
// here: `GtkBackend::surface_fill_rect` now calls the new
// `crate::gtk::set_source_rgba` (see that fn's doc for why every
// existing `surface_fill_rect` caller is unaffected — they all already
// pass opaque colours).
//
// `#[allow(dead_code)]`: see `primitives::form`'s identical note (#808)
// — only *called* once a real pixel backend is compiled in, exercised by
// each backend's own `Backend::draw_scrollbar`/`draw_terminal` call
// sites plus this module's own `RecordingSurface` tests on every leg
// that enables one of the three cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ScrollAxis, Scrollbar};
    use crate::native_surface::NativeSurface;
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Rect;

    /// `color` with its alpha channel replaced by `alpha` (`0.0`-`1.0`).
    fn with_alpha(color: Color, alpha: f32) -> Color {
        Color::rgba(
            color.r,
            color.g,
            color.b,
            (255.0 * alpha.clamp(0.0, 1.0)).round() as u8,
        )
    }

    /// Paint a [`Scrollbar`] onto `surface`: a translucent track with a
    /// brighter translucent thumb on top, both bumping alpha on
    /// hover/drag. See this module's doc for the one known divergence
    /// (quadraui#791) re-verified (already fixed) while unifying three
    /// per-backend copies into this one.
    pub(crate) fn paint(scrollbar: &Scrollbar, surface: &mut dyn NativeSurface, theme: &Theme) {
        let track = scrollbar.track;
        if track.width <= 0.0 || track.height <= 0.0 {
            return;
        }

        let track_alpha = if scrollbar.hovered || scrollbar.dragging {
            0.35
        } else {
            0.20
        };
        let thumb_alpha = if scrollbar.dragging {
            0.85
        } else if scrollbar.hovered {
            0.70
        } else {
            0.50
        };

        surface.surface_fill_rect(track, with_alpha(theme.scrollbar_track, track_alpha));

        let thumb_rect = match scrollbar.axis {
            ScrollAxis::Vertical => Rect::new(
                track.x,
                track.y + scrollbar.thumb_start,
                track.width,
                scrollbar.thumb_len,
            ),
            ScrollAxis::Horizontal => Rect::new(
                track.x + scrollbar.thumb_start,
                track.y,
                scrollbar.thumb_len,
                track.height,
            ),
        };
        surface.surface_fill_rect(thumb_rect, with_alpha(theme.scrollbar_thumb, thumb_alpha));
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::types::WidgetId;
        use crate::Image;

        /// Records every `surface_fill_rect` call — mirrors
        /// `primitives::chart`'s `RecordingSurface` test double, scoped
        /// to just the verb this primitive uses, so this test runs on
        /// any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
        }

        impl NativeSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
                (0.0, 0.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn vertical_bar(scroll: f32, total: f32, visible: f32) -> Scrollbar {
            Scrollbar::vertical(
                WidgetId::new("sb"),
                Rect::new(0.0, 0.0, 8.0, 200.0),
                scroll,
                total,
                visible,
                20.0,
            )
        }

        /// Regression for quadraui#791, ported to the shared paint path:
        /// track/thumb must carry real alpha (`Color::a < 255`), never a
        /// fully-opaque colour pre-blended against `theme.background`.
        /// This is also the test that catches the GTK-specific
        /// `surface_fill_rect` alpha-dropping bug this same issue fixed
        /// (see this module's doc) — RED against a `paint` that routed
        /// through the pre-fix `surface_fill_rect`/`set_source`.
        #[test]
        fn track_and_thumb_paint_with_real_alpha_not_opaque() {
            let sb = vertical_bar(0.0, 200.0, 50.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&sb, &mut surface, &theme);

            assert_eq!(
                surface.fills.len(),
                2,
                "expected exactly a track fill and a thumb fill"
            );
            for (_, color) in &surface.fills {
                assert!(
                    color.a < 255,
                    "scrollbar fill must carry real alpha, got opaque a={}",
                    color.a
                );
            }
        }

        #[test]
        fn dragging_increases_thumb_alpha() {
            let mut sb = vertical_bar(0.0, 200.0, 50.0);
            let theme = Theme::default();

            let mut normal = RecordingSurface::default();
            paint(&sb, &mut normal, &theme);
            let normal_thumb_alpha = normal.fills[1].1.a;

            sb.dragging = true;
            let mut dragging = RecordingSurface::default();
            paint(&sb, &mut dragging, &theme);
            let dragging_thumb_alpha = dragging.fills[1].1.a;

            assert!(
                dragging_thumb_alpha > normal_thumb_alpha,
                "dragging should raise thumb alpha: normal={normal_thumb_alpha}, dragging={dragging_thumb_alpha}"
            );
        }

        #[test]
        fn zero_size_track_paints_nothing() {
            let sb = Scrollbar::vertical(
                WidgetId::new("sb"),
                Rect::new(0.0, 0.0, 0.0, 0.0),
                0.0,
                200.0,
                50.0,
                20.0,
            );
            let mut surface = RecordingSurface::default();
            paint(&sb, &mut surface, &Theme::default());
            assert!(surface.fills.is_empty());
        }

        #[test]
        fn horizontal_thumb_rect_uses_width_axis() {
            let track = Rect::new(0.0, 50.0, 100.0, 8.0);
            let sb = Scrollbar::horizontal(WidgetId::new("h"), track, 0.0, 200.0, 40.0, 10.0);
            let mut surface = RecordingSurface::default();
            paint(&sb, &mut surface, &Theme::default());
            let (thumb_rect, _) = surface.fills[1];
            assert_eq!(thumb_rect.y, track.y);
            assert_eq!(thumb_rect.height, track.height);
            assert!(thumb_rect.width > 0.0 && thumb_rect.width <= track.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_thumb_zero_range_returns_zero() {
        assert_eq!(fit_thumb(0.0, 0.0, 100.0, 200.0, 1.0), (0.0, 0.0));
        assert_eq!(fit_thumb(0.0, 100.0, 100.0, 200.0, 1.0), (0.0, 0.0));
        assert_eq!(fit_thumb(0.0, 100.0, 200.0, 200.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn fit_thumb_proportional_size() {
        // 20% of total visible → 20% of track length.
        let (start, len) = fit_thumb(0.0, 100.0, 20.0, 200.0, 1.0);
        assert_eq!(start, 0.0);
        assert!((len - 40.0).abs() < 0.01);
    }

    #[test]
    fn fit_thumb_min_length_applied() {
        let (_start, len) = fit_thumb(0.0, 1000.0, 1.0, 100.0, 10.0);
        assert!(len >= 10.0);
    }

    #[test]
    fn fit_thumb_clamped_to_track() {
        let (_start, len) = fit_thumb(0.0, 100.0, 50.0, 30.0, 100.0);
        assert!(len <= 30.0);
    }

    #[test]
    fn fit_thumb_full_scroll_aligns_to_track_end() {
        // scroll = total - visible should put thumb at the end.
        let (start, len) = fit_thumb(80.0, 100.0, 20.0, 200.0, 1.0);
        assert!((start + len - 200.0).abs() < 0.01);
    }

    #[test]
    fn fit_thumb_clamps_overscroll() {
        // scroll past max should still place thumb at track end.
        let (start, len) = fit_thumb(500.0, 100.0, 20.0, 200.0, 1.0);
        assert!((start + len - 200.0).abs() < 0.01);
    }

    #[test]
    fn fit_thumb_horizontal_uses_width_units() {
        let (start, len) = fit_thumb(50.0, 200.0, 100.0, 400.0, 1.0);
        assert!((len - 200.0).abs() < 0.01);
        assert!(start >= 0.0 && start + len <= 400.0);
    }

    #[test]
    fn vertical_constructor_uses_track_height() {
        let track = Rect::new(10.0, 20.0, 8.0, 100.0);
        let sb = Scrollbar::vertical("v", track, 0.0, 200.0, 50.0, 5.0);
        assert!(matches!(sb.axis, ScrollAxis::Vertical));
        assert_eq!(sb.track.height, 100.0);
        assert!((sb.thumb_len - 25.0).abs() < 0.01);
    }

    #[test]
    fn horizontal_constructor_uses_track_width() {
        let track = Rect::new(0.0, 0.0, 100.0, 4.0);
        let sb = Scrollbar::horizontal("h", track, 0.0, 200.0, 100.0, 5.0);
        assert!(matches!(sb.axis, ScrollAxis::Horizontal));
        assert_eq!(sb.track.width, 100.0);
        assert!((sb.thumb_len - 50.0).abs() < 0.01);
    }

    #[test]
    fn clamp_scroll_offset_empty_content_returns_zero() {
        assert_eq!(clamp_scroll_offset(0, 0), 0);
        assert_eq!(clamp_scroll_offset(50, 0), 0);
    }

    #[test]
    fn clamp_scroll_offset_in_range_is_unchanged() {
        assert_eq!(clamp_scroll_offset(0, 10), 0);
        assert_eq!(clamp_scroll_offset(5, 10), 5);
        assert_eq!(clamp_scroll_offset(9, 10), 9);
    }

    #[test]
    fn clamp_scroll_offset_pins_to_last_item() {
        // Scrolling past the end still resolves to the last valid index
        // rather than an empty view.
        assert_eq!(clamp_scroll_offset(10, 10), 9);
        assert_eq!(clamp_scroll_offset(1000, 10), 9);
    }

    #[test]
    fn clamp_scroll_offset_single_item() {
        assert_eq!(clamp_scroll_offset(0, 1), 0);
        assert_eq!(clamp_scroll_offset(50, 1), 0);
    }

    #[test]
    fn visible_window_empty_content_is_zero_zero() {
        assert_eq!(visible_window(0, 0, 5), (0, 0));
        assert_eq!(visible_window(50, 0, 5), (0, 0));
    }

    #[test]
    fn visible_window_fits_entirely_in_viewport() {
        // 5 items, 10-row viewport — the whole list is visible from 0.
        assert_eq!(visible_window(0, 5, 10), (0, 5));
    }

    #[test]
    fn visible_window_slices_a_middle_page() {
        assert_eq!(visible_window(3, 20, 5), (3, 8));
    }

    #[test]
    fn visible_window_clamps_end_to_content_len() {
        assert_eq!(visible_window(18, 20, 5), (18, 20));
    }

    #[test]
    fn visible_window_overscroll_still_shows_last_item() {
        // Scrolled well past the end: pins to the last item rather than
        // returning an empty window.
        assert_eq!(visible_window(1000, 20, 5), (19, 20));
    }

    #[test]
    fn visible_window_zero_viewport_rows_is_empty() {
        assert_eq!(visible_window(3, 20, 0), (3, 3));
    }
}
