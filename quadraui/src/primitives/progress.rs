//! `ProgressBar` primitive: a determinate or indeterminate progress
//! indicator with optional label and cancel button. Complements
//! [`Spinner`](super::spinner::Spinner) for operations where a fraction
//! is known (file transfers, download progress, multi-step installs).
//!
//! When `value` is `Some(f)` the bar renders a filled portion up to
//! `f` (clamped to 0.0..=1.0). When `value` is `None` the bar runs
//! indeterminate — backends render a sliding / pulsing fill pattern
//! using `frame_idx` the same way `Spinner` does.
//!
//! Optional cancel button: when `cancellable = true`, backends render
//! a trailing cancel affordance; clicks resolve as
//! [`ProgressBarHit::Cancel`].

use crate::event::Rect;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a progress bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressBar {
    pub id: WidgetId,
    /// Label rendered above or inline with the bar. Empty = bar only.
    #[serde(default)]
    pub label: String,
    /// `Some(f)` = determinate at fraction `f` (clamped `0.0..=1.0`);
    /// `None` = indeterminate, use `frame_idx` for animation.
    #[serde(default)]
    pub value: Option<f32>,
    /// Animation frame (same convention as `Spinner`) for indeterminate
    /// mode. Ignored when `value.is_some()`.
    #[serde(default)]
    pub frame_idx: usize,
    /// When true, a cancel affordance is drawn at the trailing edge
    /// (see [`ProgressBarHit::Cancel`]).
    #[serde(default)]
    pub cancellable: bool,
    /// Override the fill colour. `None` = theme default.
    #[serde(default)]
    pub accent: Option<Color>,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Measurement for a `ProgressBar`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressBarMeasure {
    /// Full width of the bar area.
    pub width: f32,
    /// Full height.
    pub height: f32,
    /// Width of the cancel affordance at the trailing edge (0 if not
    /// cancellable).
    pub cancel_width: f32,
}

impl ProgressBarMeasure {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            cancel_width: 0.0,
        }
    }
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressBarHit {
    /// Click landed on the cancel affordance.
    Cancel(WidgetId),
    /// Click landed on the bar body (not cancel).
    Body(WidgetId),
    /// Click landed outside the bar.
    Empty,
}

/// Fully-resolved progress-bar layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressBarLayout {
    pub bounds: Rect,
    /// Filled portion of the bar. For determinate bars, this is
    /// `bar_x..bar_x + value*bar_width`; for indeterminate, `None`
    /// (backend animates via `frame_idx`).
    pub fill_bounds: Option<Rect>,
    pub cancel_bounds: Option<Rect>,
    pub hit_regions: Vec<(Rect, ProgressBarHit)>,
}

impl ProgressBarLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> ProgressBarHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        ProgressBarHit::Empty
    }
}

impl ProgressBar {
    /// Compute layout + hit regions.
    ///
    /// # Arguments
    ///
    /// - `origin_x`, `origin_y` — top-left position.
    /// - `measure` — full width/height + cancel-affordance width.
    ///
    /// The fill portion's width is `(value.clamp(0, 1)) * (width - cancel_width)`
    /// so the bar body never overlaps the cancel affordance.
    pub fn layout(
        &self,
        origin_x: f32,
        origin_y: f32,
        measure: ProgressBarMeasure,
    ) -> ProgressBarLayout {
        let bounds = Rect::new(origin_x, origin_y, measure.width, measure.height);
        let cancel_width = if self.cancellable {
            measure.cancel_width
        } else {
            0.0
        };
        let bar_width = (measure.width - cancel_width).max(0.0);

        let fill_bounds = if let Some(frac) = self.value {
            let f = frac.clamp(0.0, 1.0);
            Some(Rect::new(origin_x, origin_y, bar_width * f, measure.height))
        } else {
            None
        };

        let cancel_bounds = if self.cancellable && cancel_width > 0.0 {
            Some(Rect::new(
                origin_x + bar_width,
                origin_y,
                cancel_width,
                measure.height,
            ))
        } else {
            None
        };

        let mut hit_regions: Vec<(Rect, ProgressBarHit)> = Vec::new();
        if let Some(cb) = cancel_bounds {
            hit_regions.push((cb, ProgressBarHit::Cancel(self.id.clone())));
        }
        // Body is the entire bar; cancel comes first so it wins on overlap.
        hit_regions.push((bounds, ProgressBarHit::Body(self.id.clone())));

        ProgressBarLayout {
            bounds,
            fill_bounds,
            cancel_bounds,
            hit_regions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bar_layout_determinate() {
        let p = ProgressBar {
            id: WidgetId::new("download"),
            label: "Downloading…".to_string(),
            value: Some(0.4),
            frame_idx: 0,
            cancellable: false,
            accent: None,
        };
        let layout = p.layout(0.0, 0.0, ProgressBarMeasure::new(200.0, 8.0));
        assert_eq!(layout.bounds.width, 200.0);
        let fill = layout.fill_bounds.unwrap();
        assert_eq!(fill.width, 80.0); // 0.4 * 200
        assert!(layout.cancel_bounds.is_none());
    }

    #[test]
    fn progress_bar_layout_indeterminate() {
        let p = ProgressBar {
            id: WidgetId::new("op"),
            label: String::new(),
            value: None,
            frame_idx: 5,
            cancellable: false,
            accent: None,
        };
        let layout = p.layout(0.0, 0.0, ProgressBarMeasure::new(100.0, 4.0));
        assert!(layout.fill_bounds.is_none());
    }

    #[test]
    fn progress_bar_layout_cancellable() {
        let p = ProgressBar {
            id: WidgetId::new("install"),
            label: "Installing…".to_string(),
            value: Some(0.5),
            frame_idx: 0,
            cancellable: true,
            accent: None,
        };
        let layout = p.layout(
            0.0,
            0.0,
            ProgressBarMeasure {
                width: 200.0,
                height: 8.0,
                cancel_width: 20.0,
            },
        );
        // Fill uses the bar area minus cancel width.
        let fill = layout.fill_bounds.unwrap();
        assert_eq!(fill.width, (200.0 - 20.0) * 0.5); // 90
        let cancel = layout.cancel_bounds.unwrap();
        assert_eq!(cancel.x, 180.0);
        assert_eq!(cancel.width, 20.0);
        // Click on cancel → Cancel(id).
        match layout.hit_test(190.0, 4.0) {
            ProgressBarHit::Cancel(id) => assert_eq!(id.as_str(), "install"),
            _ => panic!("expected Cancel hit"),
        }
        // Click on bar body (before cancel) → Body(id).
        match layout.hit_test(50.0, 4.0) {
            ProgressBarHit::Body(id) => assert_eq!(id.as_str(), "install"),
            _ => panic!("expected Body hit"),
        }
    }

    #[test]
    fn progress_bar_value_clamped() {
        let p = ProgressBar {
            id: WidgetId::new("overrun"),
            label: String::new(),
            value: Some(1.5), // > 1.0
            frame_idx: 0,
            cancellable: false,
            accent: None,
        };
        let layout = p.layout(0.0, 0.0, ProgressBarMeasure::new(100.0, 4.0));
        // Clamped to 1.0 → full width.
        assert_eq!(layout.fill_bounds.unwrap().width, 100.0);
    }
}
