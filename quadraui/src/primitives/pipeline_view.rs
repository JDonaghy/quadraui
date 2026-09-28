//! `PipelineView` primitive: a horizontal row of stage boxes connected by
//! arrows, each with a label, status indicator, and optional action button.
//!
//! Useful for CI/CD pipelines, multi-step wizards, deployment workflows, and
//! any sequential process where stages have discrete pass/fail status.
//!
//! ## Layout model
//!
//! ```text
//! ╭──────────╮      ╭──────────╮      ╭──────────╮
//! │  ✓ Build │ ───▶ │  ● Test  │ ───▶ │  Deploy  │
//! │          │      │  [Retry] │      │  [Go]    │
//! ╰──────────╯      ╰──────────╯      ╰──────────╯
//! ```
//!
//! Stage boxes have equal width (computed from the widest label + padding).
//! Arrow connectors sit between adjacent boxes. Click routing distinguishes
//! action-button vs. stage-body clicks. Keyboard focus moves with Left/Right;
//! Enter fires the focused stage's action if present.
//!
//! ## Event routing
//!
//! `PipelineView` owns layout and hit-testing; backends own painting.
//! After each paint the backend returns a [`PipelineViewLayout`] which the
//! host holds. On mouse events the host calls
//! [`PipelineViewLayout::hit_test`] to translate `(x, y)` into an
//! `Option<PipelineEvent>`. Keyboard events are handled by the host via
//! [`PipelineView::handle_key`].

use crate::event::Rect;
use crate::theme::Theme;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

// ── Data model ───────────────────────────────────────────────────────────────

/// Status of a single pipeline stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageStatus {
    /// Waiting to run — rendered dim.
    Pending,
    /// Currently executing — rendered with accent colour (optional spinner).
    Active,
    /// Completed successfully — rendered with green checkmark (✓).
    Done,
    /// Execution failed — rendered with red X (✗).
    Failed,
    /// Intentionally bypassed — rendered with strikethrough / grey dash (─).
    Skipped,
    /// Previously completed (or failed), but an upstream stage has since been
    /// re-run — the result is against an older revision and should not
    /// be trusted. Distinct from Pending ("never run") and Done ("trustworthy").
    /// Rendered dim with an `↻` icon to suggest re-running.
    Stale,
}

/// The `status → glyph` table for a stage's status icon — shared by every
/// backend's rasteriser (gtk, macos, tui, win). Issue #713's primitive-first
/// rule forbids a backend from carrying its own copy of this match; a
/// second/third/fourth `status_icon_text`-shaped function is exactly the
/// duplication that rule exists to stop.
///
/// Every arm is a single-codepoint glyph, so callers needing a `char`
/// (e.g. `tui::pipeline_view`, which paints one cell at a time) can take
/// `.chars().next()` safely.
pub fn status_glyph(status: &StageStatus) -> &'static str {
    match status {
        StageStatus::Done => "✓",
        StageStatus::Active => "●",
        StageStatus::Failed => "✗",
        StageStatus::Pending => "·",
        StageStatus::Skipped => "─",
        StageStatus::Stale => "↻",
    }
}

/// The `status → colour` table for a stage — shared by every backend's
/// rasteriser (#713). Used for **both** the status icon fill and the stage
/// box border: the two mappings were identical in all three pre-existing
/// copies (gtk, macos, tui), so this single table covers both call sites
/// rather than shipping as two near-duplicate tables that could drift.
pub fn status_color(status: &StageStatus, theme: &Theme) -> Color {
    match status {
        StageStatus::Done => theme.git_added,
        StageStatus::Active => theme.accent_bg,
        StageStatus::Failed => theme.error_fg,
        StageStatus::Pending | StageStatus::Skipped | StageStatus::Stale => theme.muted_fg,
    }
}

/// A single stage in a [`PipelineView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStage {
    /// Display label for this stage (e.g. "Build", "Test", "Deploy").
    pub label: String,
    /// Execution status controlling colour and icon.
    pub status: StageStatus,
    /// Optional action button text shown at the bottom of the box
    /// (e.g. "Go", "Retry", "Skip"). `None` = no button rendered.
    #[serde(default)]
    pub action: Option<String>,
}

/// Declarative description of a horizontal pipeline widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineView {
    pub id: WidgetId,
    /// Ordered list of stages rendered left-to-right.
    pub stages: Vec<PipelineStage>,
    /// Index of the keyboard-focused stage, if any.
    #[serde(default)]
    pub focused_stage: Option<usize>,
}

/// Events emitted by a [`PipelineView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineEvent {
    /// User clicked (or pressed Enter on) the action button of a stage.
    StageAction { index: usize },
    /// User clicked the body of a stage box (not the action button).
    StageSelected { index: usize },
    /// Key pressed while the widget has focus but not consumed by nav.
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── Layout + hit-testing ─────────────────────────────────────────────────────

/// Resolved layout for a single stage (coordinates in backend-native units).
#[derive(Debug, Clone, PartialEq)]
pub struct StageBounds {
    /// Index into [`PipelineView::stages`].
    pub index: usize,
    /// Full stage-box bounds.
    pub box_bounds: Rect,
    /// Bounds of the status icon area (top half of box).
    pub icon_bounds: Rect,
    /// Bounds of the label area.
    pub label_bounds: Rect,
    /// Bounds of the action button, if this stage has one.
    pub action_bounds: Option<Rect>,
    /// Bounds of the arrow connector leading **to** the *next* stage.
    /// `None` for the last stage.
    pub arrow_bounds: Option<Rect>,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineHit {
    /// Click landed on a stage's action button.
    Action(usize),
    /// Click landed on a stage body (not the action button).
    Body(usize),
    /// Click missed all interactive regions.
    Empty,
}

/// Fully-resolved pipeline layout.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineViewLayout {
    /// Overall bounding rect of the entire widget.
    pub bounds: Rect,
    /// Per-stage resolved positions.
    pub stages: Vec<StageBounds>,
    /// Uniform stage-box width (all stages share this).
    pub stage_width: f32,
    /// Uniform stage-box height.
    pub stage_height: f32,
    /// Width of each arrow connector between boxes.
    pub arrow_width: f32,
}

impl PipelineViewLayout {
    /// Hit-test a click at `(x, y)`. Returns [`PipelineHit::Action`] if the
    /// click falls inside a stage's action-button bounds, [`PipelineHit::Body`]
    /// for the rest of a stage box, or [`PipelineHit::Empty`] otherwise.
    ///
    /// Action bounds are checked first so they win over the stage body on any
    /// overlap.
    pub fn hit_test(&self, x: f32, y: f32) -> PipelineHit {
        for sb in &self.stages {
            // Action button takes priority.
            if let Some(ab) = sb.action_bounds {
                if contains(ab, x, y) {
                    return PipelineHit::Action(sb.index);
                }
            }
            if contains(sb.box_bounds, x, y) {
                return PipelineHit::Body(sb.index);
            }
        }
        PipelineHit::Empty
    }
}

fn contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

// ── Measurement ──────────────────────────────────────────────────────────────

/// Caller-supplied measurements for computing a [`PipelineViewLayout`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PipelineViewMeasure {
    /// Width of the whole widget in backend-native units.
    pub width: f32,
    /// Height of the whole widget.
    pub height: f32,
    /// Width of each arrow connector between boxes.
    pub arrow_width: f32,
    /// Height reserved for the action button at the bottom of each box.
    /// `0` if no stages have actions.
    pub action_height: f32,
}

impl PipelineViewMeasure {
    pub fn new(width: f32, height: f32, arrow_width: f32, action_height: f32) -> Self {
        Self {
            width,
            height,
            arrow_width,
            action_height,
        }
    }
}

impl PipelineView {
    /// Compute the layout for this pipeline.
    ///
    /// Stage width is allocated uniformly: each box gets an equal share of
    /// `measure.width` after subtracting the `(n-1) * arrow_width` connectors.
    /// Callers supply `measure.width` from their known widget rect.
    ///
    /// `origin_x`, `origin_y` are the top-left of the widget in backend-native
    /// coordinates.
    pub fn layout(
        &self,
        origin_x: f32,
        origin_y: f32,
        measure: PipelineViewMeasure,
    ) -> PipelineViewLayout {
        let n = self.stages.len();
        let bounds = Rect::new(origin_x, origin_y, measure.width, measure.height);

        if n == 0 {
            return PipelineViewLayout {
                bounds,
                stages: vec![],
                stage_width: 0.0,
                stage_height: measure.height,
                arrow_width: measure.arrow_width,
            };
        }

        let arrow_total = measure.arrow_width * (n as f32 - 1.0).max(0.0);
        let stage_width = ((measure.width - arrow_total) / n as f32).max(1.0);
        let stage_height = measure.height;

        let mut stages = Vec::with_capacity(n);
        let mut x = origin_x;

        for (i, _stage) in self.stages.iter().enumerate() {
            let box_bounds = Rect::new(x, origin_y, stage_width, stage_height);

            // Icon occupies the top 40% of the box (min 1 unit).
            let icon_h = (stage_height * 0.4).max(1.0);
            let icon_bounds = Rect::new(x, origin_y, stage_width, icon_h);

            // Label sits below the icon.
            let label_y = origin_y + icon_h;
            let label_h = if measure.action_height > 0.0 {
                (stage_height - icon_h - measure.action_height).max(0.0)
            } else {
                (stage_height - icon_h).max(0.0)
            };
            let label_bounds = Rect::new(x, label_y, stage_width, label_h);

            // Action button at the bottom of the box.
            let action_bounds = if self.stages[i].action.is_some() && measure.action_height > 0.0 {
                let ay = origin_y + stage_height - measure.action_height;
                Some(Rect::new(x, ay, stage_width, measure.action_height))
            } else {
                None
            };

            // Arrow connector to the right (not on the last stage).
            let arrow_bounds = if i + 1 < n {
                Some(Rect::new(
                    x + stage_width,
                    origin_y,
                    measure.arrow_width,
                    stage_height,
                ))
            } else {
                None
            };

            stages.push(StageBounds {
                index: i,
                box_bounds,
                icon_bounds,
                label_bounds,
                action_bounds,
                arrow_bounds,
            });

            x += stage_width + measure.arrow_width;
        }

        PipelineViewLayout {
            bounds,
            stages,
            stage_width,
            stage_height,
            arrow_width: measure.arrow_width,
        }
    }

    /// Handle a keyboard event on the focused pipeline.
    ///
    /// - `ArrowLeft` / `ArrowRight` move `focused_stage`.
    /// - `Enter` fires [`PipelineEvent::StageAction`] for the focused stage if
    ///   it has an action, or [`PipelineEvent::StageSelected`] otherwise.
    ///
    /// Returns `Some(event)` if the key was consumed, `None` if the caller
    /// should handle it.
    pub fn handle_key(&mut self, key: &str, modifiers: Modifiers) -> Option<PipelineEvent> {
        let n = self.stages.len();
        if n == 0 {
            return None;
        }
        match key {
            "ArrowLeft" | "Left" => {
                let cur = self.focused_stage.unwrap_or(0);
                self.focused_stage = Some(cur.saturating_sub(1));
                None
            }
            "ArrowRight" | "Right" => {
                let cur = self.focused_stage.unwrap_or(0);
                self.focused_stage = Some((cur + 1).min(n - 1));
                None
            }
            "Enter" => {
                if let Some(idx) = self.focused_stage {
                    if idx < n {
                        if self.stages[idx].action.is_some() {
                            return Some(PipelineEvent::StageAction { index: idx });
                        } else {
                            return Some(PipelineEvent::StageSelected { index: idx });
                        }
                    }
                }
                None
            }
            _ => Some(PipelineEvent::KeyPressed {
                key: key.to_string(),
                modifiers,
            }),
        }
    }
}

// ── NativeSurface paint (shared gtk/macos/win implementation, issue #1085,
// NativeSurface Phase 4 8/8) ────────────────────────────────────────────
//
// Before this, `gtk::pipeline_view::draw_pipeline_view` (Cairo + Pango),
// `macos::pipeline_view::draw_pipeline_view` (Core Graphics + Core Text)
// and `win::pipeline_view::draw_pipeline_view` (Direct2D + DirectWrite)
// each independently painted the same stage/arrow geometry (already
// unified by [`PipelineView::layout`]/[`pixel_pipeline_view_layout`])
// with their own drawing API. `paint` below is the one shared
// implementation, written against [`crate::native_surface::NativeSurface`]
// (#807, Phase 1) instead of any one backend's drawing API — same shape
// as `diff_view`'s #866 migration and `board`'s #1085 slice above.
//
// ## Divergences found (re-verified, reported here rather than silently
// resolved — CLAUDE.md's "re-verify before you implement" + this issue's
// acceptance bar)
//
// 1. **Status icon vertical position.** `gtk::pipeline_view` and
//    `win::pipeline_view` both centred the icon glyph within the top
//    third of the box (`icon_h = bh / 3.0`, then `by + icon_h / 2.0 -
//    ih / 2.0`). `macos::pipeline_view` used `by + bh / 5.0`, ignoring
//    the glyph's own measured height entirely. `paint` uses the 2-of-3
//    majority (gtk/win) formula on every backend — macOS gains real
//    vertical centring it never had.
// 2. **Label vertical position.** `gtk::pipeline_view` and
//    `win::pipeline_view` both centred the label using its own measured
//    height (`by + bh / 2.0 - lh / 2.0`). `macos::pipeline_view` used a
//    hardcoded `by + bh / 2.0 - 8.0` offset, never consulting the
//    measured height. `paint` uses the 2-of-3 majority formula — macOS's
//    label centring no longer drifts for fonts taller/shorter than the
//    8px the hardcoded offset assumed.
// 3. **Label overflow handling.** `gtk::pipeline_view` ellipsized an
//    overflowing label with Pango's `EllipsizeMode::End`.
//    `win::pipeline_view` clamped the *drawn* width to
//    `bb.width - 2 * PIPELINE_H_PAD` and let `DWrite::draw_text`'s
//    `D2D1_DRAW_TEXT_OPTIONS_CLIP` cut it off. `macos::pipeline_view` did
//    neither — an overlong label could paint past the box into the next
//    stage or the arrow connector, uncropped. `NativeSurface` has no
//    ellipsize verb (see `diff_view`'s #866 "Header-label overflow
//    handling" and `board`'s #1085 divergence 1/2 for the identical
//    tradeoff), so `paint` clamps the label's drawn width like
//    `win::pipeline_view` did and additionally hard-clips the whole box
//    (icon + label + action) to `box_bounds` — macOS's overflow bleed is
//    fixed, and GTK's ellipsis becomes a hard clip.
// 4. **Action-button tint background.** `gtk::pipeline_view` painted a
//    translucent `theme.accent_bg` tint (Cairo `set_source_rgba` at
//    `0.15` alpha, inset 1px) behind the button label.
//    `win::pipeline_view` approximated the same tint with a CPU-side
//    [`crate::types::Color::blend`] against `theme.surface_bg` — exactly
//    the "`Color::blend` instead of a real alpha composite" smell
//    [`crate::native_surface::NativeSurface::surface_fill_rect_alpha`]'s
//    own doc names as the reason that verb exists.
//    `macos::pipeline_view` painted no tint at all. `paint` uses
//    [`crate::native_surface::NativeSurface::surface_fill_rect_alpha`] to
//    fill the full `action_bounds` with a real alpha composite over
//    `theme.accent_bg` at the same `0.15` alpha every backend already
//    agreed on — macOS gains the tint it was missing, and Windows's CPU
//    blend becomes a real composite.
// 5. **Action-button label vertical position.** `gtk::pipeline_view` and
//    `win::pipeline_view` both centred the button label vertically within
//    `action_bounds` (`ab.y + ab.height / 2.0 - bh2 / 2.0`).
//    `macos::pipeline_view` drew it flush to `ab.y` (top-aligned,
//    ignoring the button's own height). `paint` uses the 2-of-3 majority
//    formula — macOS's button label is now vertically centred like its
//    siblings.
// 6. **Focus indicator / arrow-head shape.** `gtk::pipeline_view` and
//    `macos::pipeline_view` filled a solid triangle path for both the
//    `▼` focus indicator and the arrow connector's head.
//    `win::pipeline_view` drew each as two stroked line segments instead
//    — `win::text` exposes no filled-arbitrary-path primitive (only
//    rects, rounded rects, lines, and circles). `NativeSurface` has the
//    identical gap (`surface_draw_line`/`surface_fill_rect`/
//    `surface_fill_rounded_rect`, no fill-path verb), so `paint` adopts
//    Windows's two-line chevron on every backend. GTK/macOS's solid
//    triangles become open chevrons — a visible but small shape change at
//    this glyph-scale size.
// 7. **Stage-box border shape.** `gtk::pipeline_view` and
//    `macos::pipeline_view` stroked a *rounded*-rect border
//    (`pixel::CORNER_RADIUS`); `win::pipeline_view` could only stroke a
//    straight rectangle (no rounded-stroke primitive — see divergence 6's
//    same underlying gap). `NativeSurface::surface_stroke_rect` is
//    axis-aligned only, so `paint` strokes every stage-box border as a
//    straight rectangle — mirrors `board`'s #1085 divergence 3 exactly.
//    GTK/macOS boxes lose their rounded-pill corners; `pixel::CORNER_RADIUS`
//    is no longer consumed by any rasteriser.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{status_color, status_glyph, PipelineView, PipelineViewLayout};
    use crate::event::{Point, Rect};
    use crate::native_surface::NativeSurface;
    use crate::primitives::layout_metrics::{pixel, pixel_pipeline_view_layout};
    use crate::theme::Theme;

    /// Alpha applied to the action button's tint background — the value
    /// every pre-port backend already agreed on (GTK's `set_source_rgba`
    /// alpha, Windows's `Color::blend` factor). See divergence 4 above.
    const ACTION_TINT_ALPHA: f32 = 0.15;

    /// Paint a [`PipelineView`] into `rect` on `surface`, returning
    /// [`PipelineViewLayout`] for host click dispatch — same contract as
    /// every deleted per-backend `draw_pipeline_view`.
    pub(crate) fn paint(
        view: &PipelineView,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
        rect: Rect,
    ) -> PipelineViewLayout {
        let layout = pixel_pipeline_view_layout(view, rect.x, rect.y, rect.width, rect.height);

        if rect.width <= 0.0 || rect.height <= 0.0 {
            return layout;
        }

        for sb in &layout.stages {
            let stage = &view.stages[sb.index];
            let is_focused = view.focused_stage == Some(sb.index);
            let bb = sb.box_bounds;

            if bb.width <= 0.0 || bb.height <= 0.0 {
                continue;
            }

            // ── Box fill + border (straight rect on every backend — see
            // divergence 7 above) ───────────────────────────────────────
            surface.surface_fill_rect(bb, theme.surface_bg);
            let border_color = status_color(&stage.status, theme);
            surface.surface_stroke_rect(bb, border_color, pixel::PIPELINE_BORDER_WIDTH as f32);

            // ── Focus indicator (▼ chevron above the box — divergence 6) ──
            if is_focused {
                let ind_x = bb.x + bb.width / 2.0;
                let tri_tip_y = bb.y - 1.0;
                let tri_base_y = bb.y - pixel::PIPELINE_FOCUS_INDICATOR_H + 1.0;
                let half_w = 5.0;
                surface.surface_draw_line(
                    Point::new(ind_x - half_w, tri_base_y),
                    Point::new(ind_x, tri_tip_y),
                    theme.muted_fg,
                    1.5,
                );
                surface.surface_draw_line(
                    Point::new(ind_x, tri_tip_y),
                    Point::new(ind_x + half_w, tri_base_y),
                    theme.muted_fg,
                    1.5,
                );
            }

            // Icon + label + action are all clipped to the box — see
            // divergence 3 above for why every backend now hard-clips.
            surface.surface_push_clip(bb);

            // ── Status icon (top third of box — divergence 1) ────────────
            let icon_text = status_glyph(&stage.status);
            let icon_color = status_color(&stage.status, theme);
            let (iw, ih) = surface.surface_measure_text(icon_text);
            let icon_h = bb.height / 3.0;
            let icon_cx = bb.x + bb.width / 2.0 - iw / 2.0;
            let icon_cy = bb.y + icon_h / 2.0 - ih / 2.0;
            surface.surface_draw_text_run(
                Rect::new(icon_cx, icon_cy, iw.max(1.0), ih.max(1.0)),
                icon_text,
                icon_color,
            );

            // ── Label (centred, clamped + hard-clipped — divergences 2/3) ──
            if !stage.label.is_empty() {
                let (lw, lh) = surface.surface_measure_text(&stage.label);
                let avail_w = (bb.width - 2.0 * pixel::PIPELINE_H_PAD as f32).max(0.0);
                let draw_w = lw.min(avail_w).max(1.0);
                let label_cx = bb.x + bb.width / 2.0 - draw_w / 2.0;
                let label_cy = bb.y + bb.height / 2.0 - lh / 2.0;
                surface.surface_draw_text_run(
                    Rect::new(label_cx, label_cy, draw_w, lh.max(1.0)),
                    &stage.label,
                    theme.foreground,
                );
            }

            // ── Action button (bottom strip — divergences 4/5) ────────────
            if let (Some(ab), Some(action_text)) = (sb.action_bounds, &stage.action) {
                let btn_label = format!("[{}]", action_text);

                surface.surface_fill_rect_alpha(ab, theme.accent_bg, ACTION_TINT_ALPHA);

                let (bw2, bh2) = surface.surface_measure_text(&btn_label);
                let btn_cx = ab.x + ab.width / 2.0 - bw2 / 2.0;
                let btn_cy = ab.y + ab.height / 2.0 - bh2 / 2.0;
                surface.surface_draw_text_run(
                    Rect::new(btn_cx, btn_cy, bw2.max(1.0), bh2.max(1.0)),
                    &btn_label,
                    theme.accent_bg,
                );
            }

            surface.surface_pop_clip();

            // ── Arrow connector (line + chevron head — divergence 6) ──────
            if let Some(arrow) = sb.arrow_bounds {
                let ax = arrow.x;
                let mid_y = arrow.y + arrow.height / 2.0;
                let aw = arrow.width;

                surface.surface_draw_line(
                    Point::new(ax, mid_y),
                    Point::new(ax + aw - 6.0, mid_y),
                    theme.muted_fg,
                    1.0,
                );

                let tip_x = ax + aw - 1.0;
                let tail_x = ax + aw - 7.0;
                let half_h = 4.0;
                surface.surface_draw_line(
                    Point::new(tail_x, mid_y - half_h),
                    Point::new(tip_x, mid_y),
                    theme.muted_fg,
                    1.0,
                );
                surface.surface_draw_line(
                    Point::new(tip_x, mid_y),
                    Point::new(tail_x, mid_y + half_h),
                    theme.muted_fg,
                    1.0,
                );
            }
        }

        layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::pipeline_view::{PipelineStage, StageStatus};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every drawing verb `paint` issues — mirrors
        /// `primitives::diff_view`'s own `RecordingSurface` (#810/#865/#866).
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
            lines: Vec<(Point, Point, Color)>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
        }

        impl NativeSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(300.0, 80.0, 1.0)
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
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, _stroke_width: f32) {
                self.strokes.push((rect, color));
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                from: crate::Point,
                to: crate::Point,
                color: Color,
                _stroke_width: f32,
            ) {
                self.lines.push((from, to, color));
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clip_pushes.push(rect);
            }
            fn surface_pop_clip(&mut self) {
                self.clip_pops += 1;
            }
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn make_view() -> PipelineView {
            PipelineView {
                id: WidgetId::new("pipe"),
                stages: vec![
                    PipelineStage {
                        label: "Build".into(),
                        status: StageStatus::Done,
                        action: None,
                    },
                    PipelineStage {
                        label: "Test".into(),
                        status: StageStatus::Active,
                        action: Some("Retry".into()),
                    },
                ],
                focused_stage: Some(0),
            }
        }

        #[test]
        fn zero_size_rect_paints_nothing() {
            let view = make_view();
            let mut surface = RecordingSurface::default();
            paint(
                &view,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 0.0, 0.0),
            );
            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
        }

        /// Regression for divergence 7: the stage-box border is a
        /// straight `surface_stroke_rect`, never a rounded fill/stroke.
        #[test]
        fn stage_box_border_is_a_straight_rect() {
            let view = make_view();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &view,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 300.0, 80.0),
            );
            let bb = layout.stages[0].box_bounds;
            assert!(surface.strokes.iter().any(|(r, _)| *r == bb));
        }

        /// Regression for divergence 6: the focused stage's indicator is
        /// two line strokes (a chevron), not a filled path.
        #[test]
        fn focused_stage_paints_a_two_line_chevron() {
            let view = make_view();
            let mut surface = RecordingSurface::default();
            paint(
                &view,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 300.0, 80.0),
            );
            assert_eq!(
                surface.lines.len(),
                2 + 3,
                "1 focus chevron (2 lines) + 1 arrow connector (3 lines)"
            );
        }

        /// Regression for divergence 3: icon + label + action are all
        /// clipped to the stage box (one push/pop bracket per stage).
        #[test]
        fn each_stage_clips_its_content_to_the_box() {
            let view = make_view();
            let mut surface = RecordingSurface::default();
            paint(
                &view,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 300.0, 80.0),
            );
            assert_eq!(surface.clip_pushes.len(), surface.clip_pops);
            assert_eq!(surface.clip_pushes.len(), view.stages.len());
        }

        /// Regression for divergence 4: the action button paints a
        /// translucent `accent_bg` tint (a real alpha composite, via the
        /// default `surface_fill_rect_alpha` forwarding to
        /// `surface_fill_rect` with `color.with_alpha`), covering the
        /// whole `action_bounds` — every backend gets it now, including
        /// the one (macOS) that previously painted none at all.
        #[test]
        fn action_button_paints_a_translucent_tint_over_the_full_bounds() {
            let view = make_view();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &view,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 300.0, 80.0),
            );
            let ab = layout.stages[1]
                .action_bounds
                .expect("stage 1 has an action button");
            let tint = surface
                .fills
                .iter()
                .find(|(r, _)| *r == ab)
                .map(|(_, c)| *c)
                .expect("action tint fill over the full action_bounds");
            assert_eq!(tint.r, theme.accent_bg.r);
            assert_eq!(tint.g, theme.accent_bg.g);
            assert_eq!(tint.b, theme.accent_bg.b);
            assert!(
                tint.a < theme.accent_bg.a,
                "tint alpha ({}) must be lower than accent_bg's own opaque alpha ({})",
                tint.a,
                theme.accent_bg.a,
            );
        }

        /// Regression for divergences 1/2: icon and label both use the
        /// "top-third centred" / "measured-height centred" majority
        /// formula (gtk+win), not macOS's pre-#1085 hardcoded offsets.
        #[test]
        fn icon_and_label_are_centred_using_their_measured_size() {
            let view = make_view();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &view,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 300.0, 80.0),
            );
            let bb = layout.stages[0].box_bounds;
            let (_, icon_text, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == status_glyph(&StageStatus::Done))
                .expect("icon text painted");
            assert_eq!(icon_text, status_glyph(&StageStatus::Done));
            let (label_rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == "Build")
                .expect("label text painted");
            let (lw, lh) = surface.surface_measure_text("Build");
            assert!((label_rect.width - lw).abs() < 0.01);
            let expected_cy = bb.y + bb.height / 2.0 - lh / 2.0;
            assert!((label_rect.y - expected_cy).abs() < 0.01);
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;

    fn make_pipeline() -> PipelineView {
        PipelineView {
            id: WidgetId::new("pipe"),
            stages: vec![
                PipelineStage {
                    label: "Build".into(),
                    status: StageStatus::Done,
                    action: None,
                },
                PipelineStage {
                    label: "Test".into(),
                    status: StageStatus::Active,
                    action: Some("Retry".into()),
                },
                PipelineStage {
                    label: "Deploy".into(),
                    status: StageStatus::Pending,
                    action: Some("Go".into()),
                },
            ],
            focused_stage: None,
        }
    }

    fn measure() -> PipelineViewMeasure {
        // 300 wide, 4 arrow units each side, action area 10 units tall.
        PipelineViewMeasure::new(300.0, 60.0, 4.0, 10.0)
    }

    // ── Construction ────────────────────────────────────────────────────

    #[test]
    fn construction_and_stage_count() {
        let p = make_pipeline();
        assert_eq!(p.stages.len(), 3);
        assert_eq!(p.stages[0].label, "Build");
        assert_eq!(p.stages[1].status, StageStatus::Active);
        assert!(p.stages[2].action.is_some());
    }

    #[test]
    fn stage_mutation() {
        let mut p = make_pipeline();
        p.stages[0].status = StageStatus::Failed;
        assert_eq!(p.stages[0].status, StageStatus::Failed);
        p.stages.push(PipelineStage {
            label: "Notify".into(),
            status: StageStatus::Pending,
            action: None,
        });
        assert_eq!(p.stages.len(), 4);
    }

    // ── Layout ──────────────────────────────────────────────────────────

    #[test]
    fn layout_equal_width_boxes() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // 3 stages, 2 arrows × 4.0 = 8.0; (300 - 8) / 3 ≈ 97.33
        let expected_w = (300.0 - 8.0) / 3.0;
        assert!(
            (layout.stage_width - expected_w).abs() < 0.01,
            "stage_width = {}, expected ~{}",
            layout.stage_width,
            expected_w
        );
        for sb in &layout.stages {
            assert!(
                (sb.box_bounds.width - expected_w).abs() < 0.01,
                "stage {} box width = {}, expected {}",
                sb.index,
                sb.box_bounds.width,
                expected_w
            );
        }
    }

    #[test]
    fn layout_origin_offset() {
        let p = make_pipeline();
        let layout = p.layout(10.0, 5.0, measure());
        assert_eq!(layout.bounds.x, 10.0);
        assert_eq!(layout.bounds.y, 5.0);
        assert_eq!(layout.stages[0].box_bounds.x, 10.0);
        assert_eq!(layout.stages[0].box_bounds.y, 5.0);
    }

    #[test]
    fn layout_arrow_between_stages() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // Arrow after stage 0.
        let arrow = layout.stages[0].arrow_bounds.expect("arrow after stage 0");
        assert_eq!(arrow.x, layout.stages[0].box_bounds.x + layout.stage_width);
        assert_eq!(arrow.width, 4.0);
        // No arrow after last stage.
        assert!(layout.stages[2].arrow_bounds.is_none());
    }

    #[test]
    fn layout_action_bounds_present_when_action_set() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // Stage 0 has no action.
        assert!(layout.stages[0].action_bounds.is_none());
        // Stage 1 has "Retry".
        let ab = layout.stages[1]
            .action_bounds
            .expect("action bounds for stage 1");
        assert_eq!(ab.height, 10.0);
        // Stage 2 has "Go".
        assert!(layout.stages[2].action_bounds.is_some());
    }

    #[test]
    fn layout_empty_pipeline_is_safe() {
        let p = PipelineView {
            id: WidgetId::new("empty"),
            stages: vec![],
            focused_stage: None,
        };
        let layout = p.layout(0.0, 0.0, PipelineViewMeasure::new(300.0, 60.0, 4.0, 10.0));
        assert_eq!(layout.stages.len(), 0);
        assert_eq!(layout.stage_width, 0.0);
    }

    // ── Hit-testing ─────────────────────────────────────────────────────

    #[test]
    fn hit_test_action_button_returns_action_event() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // Stage 1 has action bounds.
        let ab = layout.stages[1].action_bounds.unwrap();
        let cx = ab.x + ab.width / 2.0;
        let cy = ab.y + ab.height / 2.0;
        assert_eq!(layout.hit_test(cx, cy), PipelineHit::Action(1));
    }

    #[test]
    fn hit_test_stage_body_returns_body_event() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        let bb = layout.stages[0].box_bounds;
        // Click in the top half of stage 0 (no action there).
        let cx = bb.x + bb.width / 2.0;
        let cy = bb.y + bb.height / 4.0;
        assert_eq!(layout.hit_test(cx, cy), PipelineHit::Body(0));
    }

    #[test]
    fn hit_test_miss_returns_empty() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        // Click past the right edge.
        assert_eq!(layout.hit_test(500.0, 30.0), PipelineHit::Empty);
        // Click above the widget.
        assert_eq!(layout.hit_test(50.0, -1.0), PipelineHit::Empty);
    }

    #[test]
    fn hit_test_arrow_region_returns_empty() {
        let p = make_pipeline();
        let layout = p.layout(0.0, 0.0, measure());
        let arrow = layout.stages[0].arrow_bounds.unwrap();
        // Arrow connector is non-interactive — no PipelineHit returned for it.
        let hit = layout.hit_test(arrow.x + arrow.width / 2.0, arrow.y + arrow.height / 2.0);
        assert_eq!(hit, PipelineHit::Empty);
    }

    // ── Keyboard navigation ─────────────────────────────────────────────

    #[test]
    fn keyboard_right_moves_focus() {
        let mut p = make_pipeline();
        p.focused_stage = Some(0);
        p.handle_key("Right", Modifiers::default());
        assert_eq!(p.focused_stage, Some(1));
        p.handle_key("ArrowRight", Modifiers::default());
        assert_eq!(p.focused_stage, Some(2));
    }

    #[test]
    fn keyboard_right_clamps_at_last() {
        let mut p = make_pipeline();
        p.focused_stage = Some(2); // last
        p.handle_key("Right", Modifiers::default());
        assert_eq!(p.focused_stage, Some(2));
    }

    #[test]
    fn keyboard_left_moves_focus() {
        let mut p = make_pipeline();
        p.focused_stage = Some(2);
        p.handle_key("Left", Modifiers::default());
        assert_eq!(p.focused_stage, Some(1));
        p.handle_key("ArrowLeft", Modifiers::default());
        assert_eq!(p.focused_stage, Some(0));
    }

    #[test]
    fn keyboard_left_clamps_at_zero() {
        let mut p = make_pipeline();
        p.focused_stage = Some(0);
        p.handle_key("Left", Modifiers::default());
        assert_eq!(p.focused_stage, Some(0));
    }

    #[test]
    fn keyboard_enter_fires_action_when_present() {
        let mut p = make_pipeline();
        p.focused_stage = Some(1); // has "Retry" action
        let event = p.handle_key("Enter", Modifiers::default());
        assert_eq!(event, Some(PipelineEvent::StageAction { index: 1 }));
    }

    #[test]
    fn keyboard_enter_fires_selected_when_no_action() {
        let mut p = make_pipeline();
        p.focused_stage = Some(0); // no action
        let event = p.handle_key("Enter", Modifiers::default());
        assert_eq!(event, Some(PipelineEvent::StageSelected { index: 0 }));
    }

    #[test]
    fn keyboard_enter_no_focus_is_noop() {
        let mut p = make_pipeline();
        p.focused_stage = None;
        let event = p.handle_key("Enter", Modifiers::default());
        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_unknown_key_passes_through() {
        let mut p = make_pipeline();
        p.focused_stage = Some(0);
        let event = p.handle_key("Escape", Modifiers::default());
        assert!(matches!(event, Some(PipelineEvent::KeyPressed { .. })));
    }

    // ── Serde ────────────────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip() {
        let p = make_pipeline();
        let json = serde_json::to_string(&p).unwrap();
        let back: PipelineView = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn event_serde_roundtrip() {
        let events = vec![
            PipelineEvent::StageAction { index: 2 },
            PipelineEvent::StageSelected { index: 0 },
            PipelineEvent::KeyPressed {
                key: "Escape".into(),
                modifiers: Modifiers::default(),
            },
        ];
        for e in &events {
            let json = serde_json::to_string(e).unwrap();
            let back: PipelineEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(e, &back);
        }
    }
}
