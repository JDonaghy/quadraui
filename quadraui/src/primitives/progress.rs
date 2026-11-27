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
//! Kept public as a cross-backend primitive with complete rasteriser
//! parity across all five implementations (win/gtk/tui/macos/testing),
//! paired with [`Spinner`](super::spinner::Spinner) as a sibling in the
//! same shape family.

use crate::event::Rect;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a progress bar.
///
/// # Examples
///
/// ```
/// use quadraui::{ProgressBar, ProgressBarMeasure, WidgetId};
///
/// let bar = ProgressBar {
///     id: WidgetId::new("progress:download"),
///     label: "Downloading…".to_string(),
///     value: Some(0.5),
///     frame_idx: 0,
///     cancellable: false,
///     accent: None,
/// };
///
/// let layout = bar.layout(0.0, 0.0, ProgressBarMeasure::new(100.0, 1.0));
/// let fill = layout.fill_bounds.expect("determinate bar has a fill rect");
/// assert_eq!(fill.width, 50.0);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
            hit_regions.push((cb, ProgressBarHit::Cancel(self.id)));
        }
        // Body is the entire bar; cancel comes first so it wins on overlap.
        hit_regions.push((bounds, ProgressBarHit::Body(self.id)));

        ProgressBarLayout {
            bounds,
            fill_bounds,
            cancel_bounds,
            hit_regions,
        }
    }
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────
//
// `ProgressBar` reaches the frame type-erased as
// [`crate::frame::Surface::Widget`]: `layout` resolves the bar's own
// geometry in the host's units — [`pixel_progress_layout`] on a pixel
// surface, [`cell_progress_layout`] on a cell grid — `paint` runs the
// shared pixel painter in [`native_surface_paint`], and `paint_cells`
// runs the cell painter in [`cell_paint`].
//
// A bar reacts to no input of its own. Even the cancel affordance is
// the *host's* affordance: [`ProgressBarLayout::hit_test`] classifies
// the click and the host decides what cancelling means (there is no
// "cancel" state on the descriptor for a `handle` to flip), so the
// widget is [`crate::non_interactive!`].

/// Columns [`cell_paint`] reserves for the cancel affordance on a cell
/// grid: one blank gutter column, the `×` glyph, one trailing column.
/// The cell-grid twin of [`pixel::PROGRESS_CANCEL_WIDTH`].
///
/// [`pixel::PROGRESS_CANCEL_WIDTH`]: crate::primitives::layout_metrics::pixel::PROGRESS_CANCEL_WIDTH
pub(crate) const CELL_CANCEL_WIDTH: f32 = 3.0;

/// Cells [`cell_paint`] fills for the indeterminate pulse block. Gated
/// with its only reader, the cell painter.
#[cfg(feature = "tui")]
pub(crate) const CELL_PULSE_WIDTH: usize = 3;

/// Resolve `bar`'s layout for a cell grid: `rect` in whole terminal
/// columns/rows, with [`CELL_CANCEL_WIDTH`] reserved for the cancel
/// affordance and the height clamped to the single row the cell painter
/// draws.
///
/// The cell-grid twin of
/// [`crate::primitives::layout_metrics::pixel_progress_layout`]; both
/// report bounds in the absolute frame `rect` is expressed in, so a
/// [`ProgressBarLayout::hit_test`] against either takes the same
/// coordinates the host painted at.
pub(crate) fn cell_progress_layout(bar: &ProgressBar, rect: Rect) -> ProgressBarLayout {
    bar.layout(
        rect.x,
        rect.y,
        ProgressBarMeasure {
            width: rect.width,
            height: rect.height.min(1.0),
            cancel_width: CELL_CANCEL_WIDTH,
        },
    )
}

impl crate::primitive::Primitive for ProgressBar {
    type Layout = ProgressBarLayout;
    type Event = ProgressBarHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Progress(self.clone())
    }

    /// One row tall, stretching to whatever width the container gives
    /// it: a progress bar's length *is* the measurement it communicates,
    /// so there is no intrinsic width to report.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        match ctx.units() {
            crate::primitive::Units::Cells => crate::NaturalSize::new(None, Some(1.0)),
            _ => crate::NaturalSize::new(None, Some(ctx.line_height)),
        }
    }

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> ProgressBarLayout {
        match ctx.units() {
            crate::primitive::Units::Cells => cell_progress_layout(self, rect),
            _ => crate::primitives::layout_metrics::pixel_progress_layout(
                self,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
            ),
        }
    }

    /// The shared pixel painter, against the already-resolved `layout`.
    fn paint(
        &self,
        layout: &ProgressBarLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let style = env.style();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        native_surface_paint::paint_resolved(self, layout, surface, theme, style);
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme, style);
    }

    /// [`cell_paint::paint`] into the layout's bounds.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ProgressBarLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        cell_paint::paint(self, layout, cells, env.theme());
    }
}

crate::non_interactive!(ProgressBar);

/// The cell-grid painter for [`ProgressBar`], written against
/// [`crate::CellSurface`].
///
/// One row: a `░` track across the bar body, a `█` run for the filled
/// portion (determinate) or a [`CELL_PULSE_WIDTH`]-cell block sliding
/// with `frame_idx` (indeterminate), the label overlaid from the second
/// column, and a `×` in the cancel gutter when
/// [`ProgressBarLayout::cancel_bounds`] is set.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{ProgressBar, ProgressBarLayout, CELL_PULSE_WIDTH};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;

    /// An opaque single-cell style — every glyph this painter writes
    /// sets both colours and no modifier.
    fn solid(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `bar` into `layout.bounds` on `surface`.
    ///
    /// `layout` must be the value
    /// [`super::cell_progress_layout`] returned for the same bar and
    /// rect, so paint and [`ProgressBarLayout::hit_test`] can never
    /// disagree. A layout with no painted row (zero width or height) is
    /// a no-op.
    pub(crate) fn paint(
        bar: &ProgressBar,
        layout: &ProgressBarLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        if layout.bounds.width < 1.0 || layout.bounds.height < 1.0 {
            return;
        }

        let y = layout.bounds.y.round() as u16;
        let bx = layout.bounds.x.round() as u16;
        let bw = layout.bounds.width.round() as u16;

        let fill_color = bar.accent.unwrap_or(theme.accent_bg);
        let track_bg = theme.surface_bg;
        let fg = theme.foreground;

        // Track background, stopping short of the cancel gutter.
        let bar_end = match layout.cancel_bounds {
            Some(cb) => cb.x.round() as u16,
            None => bx + bw,
        };
        for dx in bx..bar_end {
            surface.put_char(dx, y, '░', solid(fg, track_bg));
        }

        if let Some(fb) = layout.fill_bounds {
            let fill_end = (fb.x + fb.width).round() as u16;
            for dx in bx..fill_end.min(bar_end) {
                surface.put_char(dx, y, '█', solid(fill_color, fill_color));
            }
        } else {
            // Indeterminate: a short block sliding with `frame_idx`.
            let track_w = bar_end.saturating_sub(bx) as usize;
            if track_w > 0 {
                let pulse_w = CELL_PULSE_WIDTH.min(track_w);
                let pos = bar.frame_idx % track_w;
                for i in 0..pulse_w {
                    let dx = bx + ((pos + i) % track_w) as u16;
                    if dx < bar_end {
                        surface.put_char(dx, y, '█', solid(fill_color, fill_color));
                    }
                }
            }
        }

        // Label overlay, left-aligned with one cell of padding. Each
        // glyph keeps whatever background the track/fill left under it,
        // so the label reads against both halves of a partly-filled bar.
        //
        // The loops above always write an opaque background across the
        // whole bar first, so `track_bg` is only a fallback for a cell
        // whose recovered background is not a concrete colour. Painting
        // the label without the track would therefore flatten the
        // label's background to `track_bg` rather than leave it alone.
        if !bar.label.is_empty() {
            for (col, ch) in (bx + 1..).zip(bar.label.chars()) {
                if col >= bar_end {
                    break;
                }
                let under = surface
                    .get(col, y)
                    .and_then(|(_, style)| style.bg)
                    .unwrap_or(track_bg);
                surface.put_char(col, y, ch, solid(fg, under));
            }
        }

        // Cancel `×` affordance, centred in its gutter.
        if let Some(cb) = layout.cancel_bounds {
            let cx = cb.x.round() as u16 + 1;
            if cx < bx + bw {
                surface.put_char(cx, y, '×', solid(fg, track_bg));
            }
        }
    }
}

// `paint` is the shared [`crate::paint_surface::PaintSurface`]
// implementation of progress bar painting (track/fill/label/cancel),
// replacing independent Cairo, Core Graphics, and Direct2D
// implementations. It takes `theme: &Theme` so all backends
// (including Windows) paint in the host's active theme.
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
    use crate::style::Style;
    use crate::theme::Theme;

    /// Paint a [`ProgressBar`] into `rect` on `surface`, returning
    /// [`ProgressBarLayout`] for host click dispatch — same contract as
    /// every deleted per-backend `draw_progress`. `style` supplies the
    /// corner radius the track/fill paint with — no
    /// border/shadow: VS Code's own progress indicator is a flat bar
    /// with no outline or elevation, unlike the card-shaped overlays
    /// this issue also touches. Callers pass `&self.style()`.
    pub(crate) fn paint(
        bar: &ProgressBar,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
        style: &Style,
    ) -> ProgressBarLayout {
        let layout = pixel_progress_layout(bar, rect.x, rect.y, rect.width, rect.height);
        paint_resolved(bar, &layout, surface, theme, style);
        layout
    }

    /// The same paint as [`paint`], against an already-resolved
    /// `layout` — what
    /// [`crate::primitive::Primitive::paint`] calls, since the widget
    /// contract hands a painter the layout rather than a rect.
    ///
    /// `layout.bounds` is the bar's own rect: both
    /// [`pixel_progress_layout`] and
    /// [`super::cell_progress_layout`] bake the origin they were laid
    /// out at into it.
    pub(crate) fn paint_resolved(
        bar: &ProgressBar,
        layout: &ProgressBarLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
    ) {
        let rect = layout.bounds;

        // Track background.
        surface.surface_fill_rounded_rect(rect, style.corner_radius, theme.surface_bg);

        let fill_color = bar.accent.unwrap_or(theme.accent_bg);
        if let Some(fb) = layout.fill_bounds {
            surface.surface_fill_rounded_rect(fb, style.corner_radius, fill_color);
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
                    surface.surface_fill_rounded_rect(
                        Rect::new(rect.x + pos, rect.y, w, rect.height),
                        style.corner_radius,
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
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every drawing verb `paint` issues — mirrors
        /// `primitives::diff_view`'s own `RecordingSurface`.
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
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
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
            let layout = paint(
                &bar,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 200.0, 20.0),
                &Style::default(),
            );
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
            paint(
                &bar,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 200.0, 20.0),
                &Style::default(),
            );
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
                &Style::default(),
            );

            let mut bar1 = bar(None, false);
            bar1.frame_idx = 5;
            let mut surface1 = RecordingSurface::default();
            paint(
                &bar1,
                &mut surface1,
                &theme,
                Rect::new(0.0, 0.0, 200.0, 20.0),
                &Style::default(),
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

/// Cell-grid paint and hit-test round trips through the widget's own
/// cell painter onto a plain buffer.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use crate::primitives::progress::{
        cell_paint, cell_progress_layout, ProgressBar, ProgressBarLayout,
    };
    use crate::theme::Theme;
    use crate::tui::BufferCellSurface;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// The cell layout for `area`, painted onto `buf` through the
    /// widget's own cell painter.
    fn draw_progress(
        buf: &mut Buffer,
        area: Rect,
        bar: &ProgressBar,
        theme: &Theme,
    ) -> ProgressBarLayout {
        let layout = cell_progress_layout(
            bar,
            crate::event::Rect::new(
                area.x as f32,
                area.y as f32,
                area.width as f32,
                area.height as f32,
            ),
        );
        cell_paint::paint(bar, &layout, &mut BufferCellSurface::new(buf), theme);
        layout
    }
    use crate::primitives::progress::ProgressBarHit;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn det_bar(value: f32) -> ProgressBar {
        ProgressBar {
            id: WidgetId::new("prog"),
            label: String::new(),
            value: Some(value),
            frame_idx: 0,
            cancellable: false,
            accent: None,
        }
    }

    // Shared bodies parametrized over the area's origin — LESSONS.md
    // "Layout helpers must return coords in the same frame across
    // backends". `tui_progress_layout` bakes `area.x`/
    // `area.y` straight into `bar.layout`'s returned bounds (absolute
    // frame), so the regression guard is a full paint+hit_test round
    // trip re-run at a non-zero origin, not a relative-coords check.

    fn determinate_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = det_bar(0.5);
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());

        // Fill should cover ~10 cells (50% of 20) starting at the area's origin.
        let fb = layout.fill_bounds.expect("fill bounds present");
        let fill_end = (fb.x + fb.width).round() as u16;
        assert_eq!(fill_end, origin_x + 10);
        assert_eq!(cell_char(&buf, origin_x, origin_y), '█');
        assert_eq!(cell_char(&buf, origin_x + 9, origin_y), '█');
        assert_eq!(cell_char(&buf, origin_x + 10, origin_y), '░');

        let hit = layout.hit_test(origin_x as f32 + 5.0, origin_y as f32 + 0.5);
        assert_eq!(hit, ProgressBarHit::Body(WidgetId::new("prog")));
    }

    #[test]
    fn determinate_fill_paint_and_click_round_trip() {
        determinate_round_trip_at(0, 0);
    }

    /// The same fill+hit_test round trip must hold when the bar isn't
    /// painted at the screen origin.
    #[test]
    fn determinate_fill_paint_and_click_round_trip_at_nonzero_origin() {
        determinate_round_trip_at(7, 13);
    }

    fn cancel_button_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 20, 1);
        let mut buf = Buffer::empty(area);
        let mut bar = det_bar(0.5);
        bar.cancellable = true;
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());

        let cb = layout.cancel_bounds.expect("cancel bounds present");
        let cx = cb.x.round() as u16 + 1;
        assert_eq!(cell_char(&buf, cx, origin_y), '×');

        let hit = layout.hit_test(cx as f32 + 0.5, origin_y as f32 + 0.5);
        assert_eq!(hit, ProgressBarHit::Cancel(WidgetId::new("prog")));
    }

    #[test]
    fn cancel_button_paint_and_click_round_trip() {
        cancel_button_round_trip_at(0, 0);
    }

    /// Non-zero-origin round-trip invariant: the same fill+hit_test must hold at any origin.
    #[test]
    fn cancel_button_paint_and_click_round_trip_at_nonzero_origin() {
        cancel_button_round_trip_at(7, 13);
    }

    #[test]
    fn indeterminate_paints_pulse() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = ProgressBar {
            id: WidgetId::new("prog"),
            label: String::new(),
            value: None,
            frame_idx: 5,
            cancellable: false,
            accent: None,
        };
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());

        assert!(layout.fill_bounds.is_none());
        // Pulse block should be at frame_idx % width.
        assert_eq!(cell_char(&buf, 5, 0), '█');
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let area = Rect::new(0, 0, 0, 0);
        let bar = det_bar(0.5);
        let _layout = draw_progress(&mut buf, area, &bar, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn outside_hit_returns_empty() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let bar = det_bar(0.5);
        let layout = draw_progress(&mut buf, area, &bar, &Theme::default());
        assert_eq!(layout.hit_test(25.0, 0.5), ProgressBarHit::Empty);
    }
}
