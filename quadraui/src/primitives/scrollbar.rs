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
//! [`fit_thumb`] is the canonical thumb-sizing/positioning formula —
//! every scrollbar geometry type (`Scrollbar`, `PaletteScrollbar`,
//! `PopupScrollbar`, `TerminalScrollbar`, `dispatch::SurfaceScrollbar`)
//! computes its thumb through it, directly or via a thin per-primitive
//! adapter (e.g. `multi_section_view::compute_thumb_bounds` converts a
//! row-indexed scroll position into `fit_thumb`'s continuous units, then
//! [`quantize_thumb`](crate::primitives::multi_section_view::quantize_thumb)
//! snaps the result to whole cells for TUI).
//!
//! A caller may choose its own `min_thumb_len` value (cells vs. pixels,
//! 1 vs. 8, etc.) — that is a per-surface *policy knob* `fit_thumb`
//! takes as a parameter.

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
///
/// # Examples
///
/// ```
/// use quadraui::{PressZone, Rect, Scrollbar};
///
/// let track = Rect::new(79.0, 0.0, 1.0, 24.0);
/// // 100 total lines, 24 visible, scrolled to line 10.
/// let bar = Scrollbar::vertical("scrollbar:editor", track, 10.0, 100.0, 24.0, 2.0);
///
/// assert_eq!(bar.press_zone(0.0), PressZone::Before);
/// // thumb_len = (24.0 / 100.0) * 24.0 = 5.76, so travel is the track
/// // length minus that, not a tautological "it's non-negative" check.
/// assert_eq!(bar.travel(), track.height - bar.thumb_len);
/// ```
///
/// Not `#[non_exhaustive]`: the oracle acceptance suite constructs this
/// struct with an exhaustive literal, and that suite is sealed, so no
/// in-tree caller can migrate off the literal to keep pace with a future
/// field addition. [`Default`] is implemented regardless — it is
/// additive and doesn't require the exhaustiveness trade-off.
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

impl Default for Scrollbar {
    /// A vertical scrollbar with an empty track and no thumb. See
    /// [`Scrollbar::vertical`] / [`Scrollbar::horizontal`] for a
    /// scrollbar with real thumb geometry.
    fn default() -> Self {
        Self {
            id: WidgetId::new(""),
            axis: ScrollAxis::Vertical,
            track: Rect::default(),
            thumb_start: 0.0,
            thumb_len: 0.0,
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

/// Where a click landed relative to a [`Scrollbar`]'s painted thumb,
/// along its axis. Returned by [`Scrollbar::press_zone`] so callers can
/// route a scrollbar click into "start a thumb drag" (on the thumb) vs.
/// "page before/after" (on the bare track) without re-deriving
/// `thumb_top`/`thumb_bottom` arithmetic themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressZone {
    /// Before the thumb (page up / left).
    Before,
    /// On the thumb itself — start a drag.
    Thumb,
    /// After the thumb (page down / right).
    After,
}

impl Scrollbar {
    /// Classify a click at `pos` — the coordinate along this scrollbar's
    /// [`ScrollAxis`] (`y` for [`ScrollAxis::Vertical`], `x` for
    /// [`ScrollAxis::Horizontal`]), in the same units as [`Self::track`] —
    /// against the painted thumb.
    pub fn press_zone(&self, pos: f32) -> PressZone {
        let thumb_start = match self.axis {
            ScrollAxis::Vertical => self.track.y + self.thumb_start,
            ScrollAxis::Horizontal => self.track.x + self.thumb_start,
        };
        let thumb_end = thumb_start + self.thumb_len;
        if pos < thumb_start {
            PressZone::Before
        } else if pos < thumb_end {
            PressZone::Thumb
        } else {
            PressZone::After
        }
    }

    /// How far the thumb can travel along the track: `track_len -
    /// thumb_len`, clamped to `>= 0.0`. `track_len` is `track.height`
    /// for a vertical scrollbar, `track.width` for horizontal.
    ///
    /// Feeds [`ThumbDrag::begin`]'s `travel` parameter.
    pub fn travel(&self) -> f32 {
        let track_len = match self.axis {
            ScrollAxis::Vertical => self.track.height,
            ScrollAxis::Horizontal => self.track.width,
        };
        (track_len - self.thumb_len).max(0.0)
    }
}

/// State for an in-progress scrollbar thumb drag.
///
/// Shared by every scroll-owning controller. Offsets are `f32` so this
/// type serves both discrete, row-indexed scroll state (`usize`, round
/// [`Self::offset_at`]'s result) and continuous pixel-based scroll state
/// without duplicating the struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThumbDrag {
    origin_pos: f32,
    origin_offset: f32,
    travel: f32,
    max_offset: f32,
}

impl ThumbDrag {
    /// Start tracking a drag from a thumb click.
    ///
    /// - `origin_pos` — cursor coordinate (along the scroll axis) at
    ///   mouse-down.
    /// - `origin_offset` — the scroll offset at mouse-down.
    /// - `travel` — track pixels/cells the thumb can move over, e.g.
    ///   [`Scrollbar::travel`].
    /// - `max_offset` — the largest reachable scroll offset.
    pub fn begin(origin_pos: f32, origin_offset: f32, travel: f32, max_offset: f32) -> Self {
        Self {
            origin_pos,
            origin_offset,
            travel,
            max_offset,
        }
    }

    /// Resolve the scroll offset for cursor coordinate `pos`, clamped
    /// to `[0, max_offset]`.
    ///
    /// Returns `None` when the drag has no usable range (`travel <=
    /// 0.0` or `max_offset <= 0.0`) — callers should treat that as "no
    /// change" (e.g. return their own `Ignored` event variant).
    ///
    /// The result is not rounded: continuous (pixel) callers use it
    /// directly, discrete (row-indexed) callers round it to `usize`
    /// themselves. Clamping before rounding gives the same integer
    /// result as the reverse order, since `max_offset` is always an
    /// exact integer value for discrete callers.
    pub fn offset_at(&self, pos: f32) -> Option<f32> {
        if self.travel <= 0.0 || self.max_offset <= 0.0 {
            return None;
        }
        let delta = pos - self.origin_pos;
        let scaled = delta / self.travel * self.max_offset;
        Some((self.origin_offset + scaled).clamp(0.0, self.max_offset))
    }

    /// The `max_offset` this drag was started with. Useful for callers
    /// that need to compare a resolved [`Self::offset_at`] result
    /// against the drag's own range after the drag reference has gone
    /// out of scope (e.g. to recompute a "stuck to end" flag).
    pub fn max_offset(&self) -> f32 {
        self.max_offset
    }
}

/// Clamp `current + delta` into `[0, max]`.
///
/// The core arithmetic behind every `scroll_by` / `page_scroll` operation.
/// `max` is typically `content_len.saturating_sub(viewport_rows)`.
pub fn scroll_by_clamped(current: usize, delta: isize, max: usize) -> usize {
    let cur = current as isize;
    let max = max as isize;
    (cur + delta).clamp(0, max) as usize
}

/// Clamp a (possibly stale) `scroll_offset` into a valid starting index
/// for `content_len` items.
///
/// Returns `0` when there is nothing to scroll to (`content_len == 0`),
/// otherwise `scroll_offset.min(content_len - 1)` — pinning the start so
/// the last item is always reachable instead of scrolling past the end
/// into an empty view.
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

// This is the one shared implementation of scrollbar painting, written
// against [`crate::paint_surface::PaintSurface`] instead of any one
// backend's drawing API.
//
// Track and thumb must carry real alpha (`Color::a < 255`), never a
// fully-opaque colour pre-blended against `theme.background`.
//
// `#[allow(dead_code)]`: only called when a real pixel backend is
// compiled in, exercised by each backend's own `Backend::draw_scrollbar`
// call site plus tests.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ScrollAxis, Scrollbar};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::Rect;

    /// Paint a [`Scrollbar`] onto `surface`: a translucent track with a
    /// brighter, higher-contrast translucent thumb on top (pill-shaped,
    /// rounded to half its shorter side), both bumping alpha on
    /// hover/drag. See this module's doc for the one known divergence
    /// re-verified (already fixed) while unifying three
    /// per-backend copies into this one.
    pub(crate) fn paint(scrollbar: &Scrollbar, surface: &mut dyn PaintSurface, theme: &Theme) {
        let track = scrollbar.track;
        if track.width <= 0.0 || track.height <= 0.0 {
            return;
        }

        let track_alpha = if scrollbar.hovered || scrollbar.dragging {
            0.35
        } else {
            0.20
        };
        // Thumb alpha floors at 0.65 even at rest — below that the thumb
        // reads as barely narrower than the track instead of a distinct
        // control, at the visual weight VS Code's default theme carries.
        let thumb_alpha = if scrollbar.dragging {
            0.90
        } else if scrollbar.hovered {
            0.80
        } else {
            0.65
        };

        surface.surface_fill_rect(track, theme.scrollbar_track.with_alpha(track_alpha));

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
        let thumb_radius = thumb_rect.width.min(thumb_rect.height) / 2.0;
        surface.surface_fill_rounded_rect(
            thumb_rect,
            thumb_radius,
            theme.scrollbar_thumb.with_alpha(thumb_alpha),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every `surface_fill_rect` / `surface_fill_rounded_rect`
        /// call — mirrors `primitives::chart`'s `RecordingSurface` test
        /// double, scoped to just the verbs this primitive uses, so this
        /// test runs on any host without Cairo/Core Graphics/Direct2D.
        ///
        /// `fills` holds every fill regardless of verb, `(rect, color)`,
        /// in call order — `paint`'s track-then-thumb order means index 1
        /// is always the thumb. `rounded_radii` records just the radius
        /// passed to `surface_fill_rounded_rect` calls, in the same
        /// relative order, so a test can assert the thumb painted with a
        /// real corner radius without caring about its exact pixel value.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            rounded_radii: Vec<f32>,
        }

        impl PaintSurface for RecordingSurface {
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
            fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
                self.fills.push((rect, color));
                self.rounded_radii.push(radius);
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
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

        /// Track and thumb must carry real alpha (`Color::a < 255`), not a
        /// fully-opaque colour pre-blended against `theme.background`.
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

        /// The thumb must read as a distinct control against its track even
        /// at rest, not only on hover/drag — regression coverage for the
        /// too-translucent-to-read-as-14px-wide complaint: resting alpha
        /// floors at 0.65 (`166/255`) and the thumb paints with a real
        /// corner radius rather than a sharp-cornered `surface_fill_rect`.
        #[test]
        fn resting_thumb_has_high_contrast_rounded_fill() {
            let sb = vertical_bar(0.0, 200.0, 50.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&sb, &mut surface, &theme);

            let (thumb_rect, thumb_color) = surface.fills[1];
            assert!(
                thumb_color.a >= 166,
                "resting thumb alpha should floor at ~0.65, got a={}",
                thumb_color.a
            );
            assert_eq!(
                surface.rounded_radii.len(),
                1,
                "thumb should paint via surface_fill_rounded_rect, not a sharp-cornered fill"
            );
            assert!(
                surface.rounded_radii[0] > 0.0,
                "thumb corner radius should be positive"
            );
            assert!(
                surface.rounded_radii[0] <= thumb_rect.width.min(thumb_rect.height) / 2.0 + 0.01
            );
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

    // ── press_zone / travel ─────────────────────────────────────────

    #[test]
    fn press_zone_before_on_after_thumb() {
        // track 0..200, thumb spans [50, 90).
        let sb = Scrollbar::vertical("v", Rect::new(0.0, 0.0, 8.0, 200.0), 40.0, 200.0, 40.0, 1.0);
        assert_eq!(sb.thumb_start, 40.0);
        assert!((sb.thumb_len - 40.0).abs() < 0.01);
        assert_eq!(sb.press_zone(10.0), PressZone::Before);
        assert_eq!(sb.press_zone(sb.thumb_start), PressZone::Thumb);
        assert_eq!(
            sb.press_zone(sb.thumb_start + sb.thumb_len - 1.0),
            PressZone::Thumb
        );
        assert_eq!(
            sb.press_zone(sb.thumb_start + sb.thumb_len),
            PressZone::After
        );
    }

    #[test]
    fn press_zone_uses_x_axis_for_horizontal() {
        let track = Rect::new(0.0, 50.0, 100.0, 8.0);
        let sb = Scrollbar::horizontal("h", track, 0.0, 200.0, 40.0, 10.0);
        assert_eq!(sb.press_zone(0.0), PressZone::Thumb);
        assert_eq!(
            sb.press_zone(track.x + sb.thumb_len + 1.0),
            PressZone::After
        );
    }

    #[test]
    fn travel_is_track_len_minus_thumb_len() {
        let sb = Scrollbar::vertical("v", Rect::new(0.0, 0.0, 8.0, 200.0), 0.0, 200.0, 40.0, 1.0);
        assert!((sb.travel() - (200.0 - sb.thumb_len)).abs() < 0.01);
    }

    #[test]
    fn travel_never_negative() {
        // min_thumb_len larger than track clamps thumb_len to track_len.
        let sb = Scrollbar::vertical(
            "v",
            Rect::new(0.0, 0.0, 8.0, 50.0),
            0.0,
            200.0,
            40.0,
            1000.0,
        );
        assert_eq!(sb.travel(), 0.0);
    }

    // ── ThumbDrag ────────────────────────────────────────────────────

    #[test]
    fn thumb_drag_no_movement_returns_origin_offset() {
        let drag = ThumbDrag::begin(100.0, 5.0, 80.0, 20.0);
        assert_eq!(drag.offset_at(100.0), Some(5.0));
    }

    #[test]
    fn thumb_drag_moves_proportionally() {
        // travel=80 maps to max_offset=20 units, so moving 40 (half the
        // travel) should move the offset by 10 (half of max_offset).
        let drag = ThumbDrag::begin(0.0, 0.0, 80.0, 20.0);
        assert_eq!(drag.offset_at(40.0), Some(10.0));
    }

    #[test]
    fn thumb_drag_clamps_to_range() {
        let drag = ThumbDrag::begin(0.0, 0.0, 80.0, 20.0);
        assert_eq!(drag.offset_at(-1000.0), Some(0.0));
        assert_eq!(drag.offset_at(1000.0), Some(20.0));
    }

    #[test]
    fn thumb_drag_zero_travel_or_max_offset_is_none() {
        assert_eq!(ThumbDrag::begin(0.0, 0.0, 0.0, 20.0).offset_at(50.0), None);
        assert_eq!(ThumbDrag::begin(0.0, 0.0, 80.0, 0.0).offset_at(50.0), None);
    }

    // ── scroll_by_clamped ────────────────────────────────────────────

    #[test]
    fn scroll_by_clamped_moves_within_range() {
        assert_eq!(scroll_by_clamped(5, 3, 20), 8);
        assert_eq!(scroll_by_clamped(5, -3, 20), 2);
    }

    #[test]
    fn scroll_by_clamped_clamps_to_bounds() {
        assert_eq!(scroll_by_clamped(5, -100, 20), 0);
        assert_eq!(scroll_by_clamped(5, 100, 20), 20);
    }
}
