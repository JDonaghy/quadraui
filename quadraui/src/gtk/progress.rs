//! GTK rasteriser for [`crate::ProgressBar`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::progress::native_surface_paint::paint`].
//! This module carries [`gtk_progress_layout`], the backend-specific
//! pure geometry computation (no painting involved).

use crate::primitives::layout_metrics::pixel_progress_layout;
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};

/// Compute the GTK pixel-unit layout for a [`ProgressBar`] without
/// painting. Shares its cancel-affordance width with
/// `mac_progress_layout` / `win_progress_layout` via
/// [`pixel_progress_layout`].
pub fn gtk_progress_layout(bar: &ProgressBar, x: f64, y: f64, w: f64, h: f64) -> ProgressBarLayout {
    pixel_progress_layout(bar, x as f32, y as f32, w as f32, h as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::progress::ProgressBarHit;
    use crate::types::WidgetId;

    /// `progress_layout` is documented **ABSOLUTE**:
    /// `fill_bounds` / `cancel_bounds` must start at the bar's own
    /// origin, not (0, 0) — the case that hides a LOCAL/ABSOLUTE mixup.
    fn round_trip_at(x: f64, y: f64) {
        let bar = ProgressBar {
            id: WidgetId::new("prog"),
            label: String::new(),
            value: Some(0.5),
            frame_idx: 0,
            cancellable: true,
            accent: None,
        };
        let layout = gtk_progress_layout(&bar, x, y, 100.0, 20.0);

        let fb = layout.fill_bounds.expect("determinate fill present");
        assert_eq!(fb.x as f64, x);
        assert_eq!(fb.y as f64, y);

        let cb = layout.cancel_bounds.expect("cancel bounds present");
        let ccx = cb.x + cb.width / 2.0;
        let ccy = cb.y + cb.height / 2.0;
        assert_eq!(
            layout.hit_test(ccx, ccy),
            ProgressBarHit::Cancel(bar.id.clone())
        );

        let bcx = fb.x + 1.0;
        assert_eq!(
            layout.hit_test(bcx, y as f32 + 1.0),
            ProgressBarHit::Body(bar.id)
        );
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Regression guard: layout must be correct at non-zero origins.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
