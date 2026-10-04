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
//!
//! # Adoption status (#825)
//!
//! Kept public without a compose-layer consumer today (unlike
//! [`Spinner`](super::spinner::Spinner), which
//! [`crate::compose::chat_controller::ChatController`] already
//! constructs). Demoting it would mean stripping `draw_progress` /
//! `progress_layout` from `Backend` across all five implementations
//! (win/gtk/tui/macos/testing) — deleting complete, working,
//! cross-backend rasteriser parity that rule 1 of the portability
//! commitment asks every primitive to have, for a type paired with an
//! adopted sibling (same shape family, same `frame_idx` convention,
//! same `examples/common/indicators_app.rs` demo across all four
//! backends, same `conformance`/`ms-11` acceptance coverage). Recorded
//! here, not silently ignored — see issue #825.

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

// ── PaintSurface paint (shared gtk/macos/win implementation, issue #1085,
// PaintSurface Phase 4 8/8) ────────────────────────────────────────────
//
// Before this, `gtk::progress::draw_progress` (Cairo + Pango),
// `macos::progress::draw_progress` (Core Graphics + Core Text) and
// `win::progress::draw_progress` (Direct2D + DirectWrite) each
// independently painted the same track/fill/label/cancel geometry
// (already unified by [`ProgressBar::layout`]/[`pixel_progress_layout`])
// with their own drawing API. `paint` below is the one shared
// implementation, written against [`crate::paint_surface::PaintSurface`]
// (#807, Phase 1) instead of any one backend's drawing API — same shape
// as `diff_view`'s #866 migration and this issue's `board`/`pipeline_view`
// slices above.
//
// ## Divergence found (re-verified, reported here rather than silently
// resolved — CLAUDE.md's "re-verify before you implement" + this issue's
// acceptance bar)
//
// **Theme ignored on Windows.** `gtk::progress::draw_progress` and
// `macos::progress::draw_progress` both take a `theme: &Theme` parameter
// and paint the host's actual active theme. `win::progress::draw_progress`
// took no theme parameter at all — it called `Theme::default()`
// internally on every paint, so a Win-GUI host running any theme other
// than the default painted every progress bar in the wrong colours (see
// that module's former doc, "`WinBackend` does not yet carry a live
// `Theme`"). `paint` below takes `theme: &Theme` like its GTK/macOS
// twins, and `WinBackend::draw_progress` now passes `self.current_theme`
// — the same live theme every other Win-GUI rasteriser already uses.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ProgressBar, ProgressBarLayout};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::primitives::layout_metrics::{pixel, pixel_progress_layout};
    use crate::theme::Theme;

    /// Paint a [`ProgressBar`] into `rect` on `surface`, returning
    /// [`ProgressBarLayout`] for host click dispatch — same contract as
    /// every deleted per-backend `draw_progress`.
    pub(crate) fn paint(
        bar: &ProgressBar,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
    ) -> ProgressBarLayout {
        let layout = pixel_progress_layout(bar, rect.x, rect.y, rect.width, rect.height);

        // Track background.
        surface.surface_fill_rect(rect, theme.surface_bg);

        let fill_color = bar.accent.unwrap_or(theme.accent_bg);
        if let Some(fb) = layout.fill_bounds {
            surface.surface_fill_rect(fb, fill_color);
        } else {
            // Indeterminate pulse — same cadence on every backend.
            let bar_w = if bar.cancellable {
                (rect.width - pixel::PROGRESS_CANCEL_WIDTH).max(0.0)
            } else {
                rect.width
            };
            if bar_w > 0.0 {
                let pulse_w = pixel::PROGRESS_PULSE_WIDTH.min(bar_w);
                let pos = (bar.frame_idx as f32 * 4.0) % bar_w;
                let w = pulse_w.min(bar_w - pos);
                if w > 0.0 {
                    surface.surface_fill_rect(
                        Rect::new(rect.x + pos, rect.y, w, rect.height),
                        fill_color,
                    );
                }
            }
        }

        // Label.
        if !bar.label.is_empty() {
            surface.surface_draw_text_run(
                Rect::new(
                    rect.x + 4.0,
                    rect.y,
                    (rect.width - 4.0).max(0.0),
                    rect.height,
                ),
                &bar.label,
                theme.foreground,
            );
        }

        // Cancel `×` affordance.
        if let Some(cb) = layout.cancel_bounds {
            let (tw, _) = surface.surface_measure_text("\u{d7}");
            let glyph_x = cb.x + ((cb.width - tw) / 2.0).max(0.0);
            surface.surface_draw_text_run(
                Rect::new(
                    glyph_x,
                    cb.y,
                    (cb.x + cb.width - glyph_x).max(1.0),
                    cb.height,
                ),
                "\u{d7}",
                theme.foreground,
            );
        }

        layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every drawing verb `paint` issues — mirrors
        /// `primitives::diff_view`'s own `RecordingSurface` (#810/#865/#866).
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 20.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
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

        fn bar(value: Option<f32>, cancellable: bool) -> ProgressBar {
            ProgressBar {
                id: WidgetId::new("p"),
                label: String::new(),
                value,
                frame_idx: 0,
                cancellable,
                accent: None,
            }
        }

        #[test]
        fn determinate_fill_uses_the_caller_supplied_theme() {
            let bar = bar(Some(0.5), false);
            let custom_accent = Color::rgb(12, 200, 250);
            let theme = Theme {
                accent_bg: custom_accent,
                ..Theme::default()
            };
            let mut surface = RecordingSurface::default();
            let layout = paint(&bar, &mut surface, &theme, Rect::new(0.0, 0.0, 200.0, 20.0));
            let fb = layout.fill_bounds.expect("determinate fill present");
            assert!(surface
                .fills
                .iter()
                .any(|(r, c)| *r == fb && *c == custom_accent));
        }

        #[test]
        fn label_and_cancel_glyph_are_drawn_as_text_runs() {
            let mut bar = bar(Some(0.5), true);
            bar.label = "Uploading".into();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&bar, &mut surface, &theme, Rect::new(0.0, 0.0, 200.0, 20.0));
            assert!(surface.text_runs.iter().any(|(_, t, _)| t == "Uploading"));
            assert!(surface.text_runs.iter().any(|(_, t, _)| t == "\u{d7}"));
        }

        #[test]
        fn indeterminate_pulse_moves_with_frame_idx() {
            let theme = Theme::default();
            let mut bar0 = bar(None, false);
            bar0.frame_idx = 0;
            let mut surface0 = RecordingSurface::default();
            paint(
                &bar0,
                &mut surface0,
                &theme,
                Rect::new(0.0, 0.0, 200.0, 20.0),
            );

            let mut bar1 = bar(None, false);
            bar1.frame_idx = 5;
            let mut surface1 = RecordingSurface::default();
            paint(
                &bar1,
                &mut surface1,
                &theme,
                Rect::new(0.0, 0.0, 200.0, 20.0),
            );

            let pulse0 = surface0.fills.last().expect("pulse fill").0;
            let pulse1 = surface1.fills.last().expect("pulse fill").0;
            assert_ne!(pulse0.x, pulse1.x);
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
