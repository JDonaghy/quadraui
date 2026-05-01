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
}
