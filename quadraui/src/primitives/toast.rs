//! `Toast` primitive: a transient corner notification with optional
//! severity tint and optional action button. Used for "File saved",
//! "LSP disconnected", "3 errors in src/foo.rs", etc.
//!
//! Toasts are ephemeral — the app owns their lifecycle (show, auto-dismiss
//! after a duration, manual dismiss) and passes the primitive the current
//! set of visible toasts each frame. The primitive itself does not tick
//! time or auto-dismiss; those are app concerns.
//!
//! # Backend contract
//!
//! **Declarative + overlay.** Render toasts stacked in the configured
//! `corner`, with each toast a box of (title, body, optional action
//! button). Clicks resolve via [`ToastStackLayout::hit_test`] /
//! [`ToastHit`]: the action button hits `ToastHit::Action`; the
//! dismiss affordance hits `ToastHit::Dismiss`. Toast boxes don't take
//! keyboard focus — they're strictly a notification surface.
//!
//! Stacking direction: bottom-corner toasts grow upward (newest nearest
//! the corner); top-corner toasts grow downward. `Toast::layout()`
//! handles this based on `corner`.

use crate::event::Rect;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a toast stack for one corner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastStack {
    pub id: WidgetId,
    /// Which corner of the viewport the stack occupies.
    pub corner: ToastCorner,
    /// Toasts in temporal order — oldest first. Visual order depends on
    /// `corner` (bottom corners stack upward, top corners stack downward).
    pub toasts: Vec<ToastItem>,
}

/// Corner placement for a `ToastStack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToastCorner {
    #[default]
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

/// One toast notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastItem {
    pub id: WidgetId,
    pub title: String,
    /// Body text. Can be empty for minimal "File saved" style toasts.
    #[serde(default)]
    pub body: String,
    /// Visual severity — backends tint the box accordingly.
    #[serde(default)]
    pub severity: ToastSeverity,
    /// Optional action button. `None` = no action shown; just the
    /// dismiss affordance is clickable.
    #[serde(default)]
    pub action: Option<ToastAction>,
    /// Override severity's default tint. Most toasts use `None` and let
    /// the theme decide.
    #[serde(default)]
    pub accent: Option<Color>,
}

/// Severity level of a `ToastItem`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToastSeverity {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

/// Action button on a toast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastAction {
    pub id: WidgetId,
    pub label: String,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// First new B.3 primitive on D6. Toasts stack in a corner with uniform
// spacing; per-toast sizes are backend-supplied (a "body"-less toast is
// shorter than one with a multi-line body).

/// Per-toast measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToastMeasure {
    /// Full width of the toast box in the backend's unit.
    pub width: f32,
    /// Full height of the toast box.
    pub height: f32,
    /// Width of the dismiss affordance at the trailing edge. `0.0` if
    /// no dismiss UI is drawn.
    pub dismiss_width: f32,
    /// Width of the action button (at the trailing edge, before
    /// dismiss). `0.0` if the toast has no action.
    pub action_width: f32,
}

impl ToastMeasure {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            dismiss_width: 0.0,
            action_width: 0.0,
        }
    }
}

/// Resolved position of one visible toast after layout.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleToast {
    /// Index into `ToastStack.toasts`.
    pub toast_idx: usize,
    pub id: WidgetId,
    /// Full toast box bounds.
    pub bounds: Rect,
    /// Dismiss affordance (if present).
    pub dismiss_bounds: Option<Rect>,
    /// Action button (if present).
    pub action_bounds: Option<Rect>,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastHit {
    /// Click landed on a toast's action button.
    Action(WidgetId),
    /// Click landed on a toast's dismiss affordance.
    Dismiss(WidgetId),
    /// Click landed on a toast's body (not action or dismiss).
    Body(WidgetId),
    /// Click landed outside any toast.
    Empty,
}

/// Fully-resolved toast-stack layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ToastStackLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub visible_toasts: Vec<VisibleToast>,
    pub hit_regions: Vec<(Rect, ToastHit)>,
}

/// Translate a `Rect` by `(dx, dy)`, keeping its size.
fn shift_rect(r: Rect, dx: f32, dy: f32) -> Rect {
    Rect::new(r.x + dx, r.y + dy, r.width, r.height)
}

impl ToastStackLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> ToastHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        ToastHit::Empty
    }
}

impl ToastStack {
    /// Compute the rendering + hit-test layout for the stack.
    ///
    /// # Arguments
    ///
    /// - `origin_x`, `origin_y` — the top-left corner of the app's
    ///   overlay area, in the backend's absolute (screen/buffer) frame.
    ///   Returned `bounds` / `hit_regions` are absolute — callers pass
    ///   `hit_test` raw click coordinates in that same frame, matching
    ///   the `MenuBar`/`Panel` convention (unlike `TreeView`'s
    ///   viewport-local frame). Pass `(0.0, 0.0)` for an overlay
    ///   anchored at the buffer/window origin.
    /// - `viewport_width`, `viewport_height` — the app's overlay area
    ///   size. Toasts are positioned relative to this.
    /// - `margin` — spacing between the stack and the viewport edges.
    /// - `gap` — vertical gap between consecutive toasts.
    /// - `measure_toast(i)` — per-toast width/height/sub-region widths.
    ///
    /// # Stacking direction
    ///
    /// - `BottomRight` / `BottomLeft`: newest toast is nearest the
    ///   corner; older toasts stack upward.
    /// - `TopRight` / `TopLeft`: newest toast is nearest the corner;
    ///   older toasts stack downward.
    ///
    /// Toasts are iterated oldest-first (matching `self.toasts` order);
    /// the layout positions them in reverse of that for bottom corners
    /// so the newest stays pinned.
    #[allow(clippy::too_many_arguments)]
    pub fn layout<F>(
        &self,
        origin_x: f32,
        origin_y: f32,
        viewport_width: f32,
        viewport_height: f32,
        margin: f32,
        gap: f32,
        measure_toast: F,
    ) -> ToastStackLayout
    where
        F: Fn(usize) -> ToastMeasure,
    {
        let mut visible_toasts: Vec<VisibleToast> = Vec::new();
        let mut hit_regions: Vec<(Rect, ToastHit)> = Vec::new();

        if self.toasts.is_empty() {
            return ToastStackLayout {
                viewport_width,
                viewport_height,
                visible_toasts,
                hit_regions,
            };
        }

        let is_right = matches!(
            self.corner,
            ToastCorner::BottomRight | ToastCorner::TopRight
        );
        let is_bottom = matches!(
            self.corner,
            ToastCorner::BottomRight | ToastCorner::BottomLeft
        );

        // Iteration order: bottom corners show newest nearest the corner,
        // so we iterate newest-first and stack upward from the bottom.
        // Top corners show newest nearest the corner (top edge) and
        // stack downward.
        let ordered: Vec<(usize, ToastMeasure)> = if is_bottom {
            // Newest (highest index) nearest bottom — iterate in reverse.
            (0..self.toasts.len())
                .rev()
                .map(|i| (i, measure_toast(i)))
                .collect()
        } else {
            (0..self.toasts.len())
                .map(|i| (i, measure_toast(i)))
                .collect()
        };

        // Starting y: bottom edge - margin for bottom corners; margin for top.
        let mut y_cursor = if is_bottom {
            viewport_height - margin
        } else {
            margin
        };

        for (i, m) in ordered {
            if m.width <= 0.0 || m.height <= 0.0 {
                continue;
            }
            let x = if is_right {
                (viewport_width - margin - m.width).max(0.0)
            } else {
                margin
            };
            let y = if is_bottom {
                (y_cursor - m.height).max(0.0)
            } else {
                y_cursor
            };

            // Skip if the toast would render off-screen.
            if (is_bottom && y >= y_cursor) || (!is_bottom && y + m.height > viewport_height) {
                break;
            }

            let bounds = Rect::new(x, y, m.width, m.height);

            // Sub-regions at the trailing edge (right edge of the toast,
            // regardless of corner side).
            let dismiss_bounds = if m.dismiss_width > 0.0 {
                Some(Rect::new(
                    bounds.x + bounds.width - m.dismiss_width,
                    bounds.y,
                    m.dismiss_width,
                    bounds.height,
                ))
            } else {
                None
            };
            let action_bounds = if m.action_width > 0.0 {
                let offset_from_right = m.dismiss_width + m.action_width;
                Some(Rect::new(
                    bounds.x + bounds.width - offset_from_right,
                    bounds.y,
                    m.action_width,
                    bounds.height,
                ))
            } else {
                None
            };

            let toast_id = self.toasts[i].id.clone();
            visible_toasts.push(VisibleToast {
                toast_idx: i,
                id: toast_id.clone(),
                bounds,
                dismiss_bounds,
                action_bounds,
            });

            // Register hit regions in specificity order: dismiss, action, body.
            if let Some(db) = dismiss_bounds {
                hit_regions.push((db, ToastHit::Dismiss(toast_id.clone())));
            }
            if let Some(ab) = action_bounds {
                // Action carries the action's id (not the toast's) so the
                // app can dispatch the intended action directly from the
                // hit result.
                if let Some(act) = &self.toasts[i].action {
                    hit_regions.push((ab, ToastHit::Action(act.id.clone())));
                }
            }
            hit_regions.push((bounds, ToastHit::Body(toast_id)));

            // Advance the cursor for the next toast.
            if is_bottom {
                y_cursor = y - gap;
                if y_cursor <= 0.0 {
                    break;
                }
            } else {
                y_cursor = y + m.height + gap;
                if y_cursor >= viewport_height {
                    break;
                }
            }
        }

        // Shift from the viewport-local frame computed above into the
        // caller's absolute frame. Matches `MenuBar::layout` /
        // `Panel::layout`'s convention (bounds already carry the origin,
        // so paint loops use them verbatim and hosts `hit_test` with raw
        // click coordinates) rather than `TreeView`'s local-frame
        // convention. Before this, `ToastStack::layout` had no origin
        // parameter at all, so every backend's `*_toast_stack_layout`
        // silently dropped `rect.x` / `rect.y` — invisible at the origin
        // (every prior test) and a real drift for any non-zero-origin
        // overlay (quadraui#494 / LESSONS.md "Layout helpers must return
        // coords in the same frame across backends").
        if origin_x != 0.0 || origin_y != 0.0 {
            for vt in &mut visible_toasts {
                vt.bounds = shift_rect(vt.bounds, origin_x, origin_y);
                vt.dismiss_bounds = vt.dismiss_bounds.map(|r| shift_rect(r, origin_x, origin_y));
                vt.action_bounds = vt.action_bounds.map(|r| shift_rect(r, origin_x, origin_y));
            }
            for (rect, _) in &mut hit_regions {
                *rect = shift_rect(*rect, origin_x, origin_y);
            }
        }

        ToastStackLayout {
            viewport_width,
            viewport_height,
            visible_toasts,
            hit_regions,
        }
    }
}

// ── NativeSurface paint (#861, Phase 2d slice 4/9 of the NativeSurface
// milestone) ────────────────────────────────────────────────────────────
//
// Before this, `gtk::toast::draw_toast_stack` (Cairo/Pango),
// `macos::toast::draw_toast_stack` (Core Graphics/Core Text) and
// `win::toast::draw_toast_stack` (Direct2D/DirectWrite) each independently
// painted the same toast box (background tint, title/body text, dismiss
// glyph, action label) with their own drawing API (quadraui#785 child
// #811, `docs/SMELL_AUDIT_2026-07.md` §5). `paint` below is the one
// shared implementation, written against
// [`crate::native_surface::NativeSurface`] (#807, Phase 1) instead of any
// one backend's drawing API — matching the pattern #811 (scrollbar), #859
// (panel) and #860 (status_bar) already established.
//
// Each backend's own no-paint `*_toast_stack_layout` twin (`gtk_toast_stack_layout`,
// `mac_toast_stack_layout`, `win_toast_stack_layout`) is untouched by this
// migration — it's layout-only, consumed by `Backend::toast_stack_layout`
// for hit-testing without a repaint, and out of this issue's scope (see
// this primitive's own D6 layout API section above). The margin/gap/
// dismiss-width/action-padding/box-width constants below are `paint`'s own
// copy, matching every pre-#861 per-backend constant's value exactly (all
// three agreed already), same as `status_bar::native_surface_paint::MIN_GAP`'s
// independent copy of `MIN_GAP_PX`.
//
// `severity_bg`'s colour formula (the `Success`/`Warning` hardcoded RGB,
// `Info`/`Error` theme fields) is unchanged — lifting those into `Theme`
// is quadraui#815's job, not this one.
//
// # Divergences found — reported, not silently resolved
//
// 1. **Win never took a live theme.** `win::toast::draw_toast_stack` had
//    no `theme: &Theme` parameter at all — `paint_toast` built
//    `Theme::default()` internally on every call, regardless of
//    `WinBackend::set_theme`. Quadraui#789 ("win rasterisers paint with
//    the live theme, not `Theme::default()`") fixed this same class of
//    bug for six other Win-GUI rasterisers (`activity_bar`, `menu_bar`,
//    `context_menu`, `completions`, `find_replace`, `status_bar`) but
//    toast was not in that list — re-verified against #789's own diff
//    while migrating (per this issue's "re-verify before you implement"),
//    not assumed. The shared `paint` requires a `theme: &Theme` parameter
//    (matching gtk/macos, which always had one), so `WinBackend::draw_toast_stack`
//    now passes `&self.current_theme` — fixing the gap as a side effect of
//    the unification rather than leaving it for a future issue.
//
// 2. **Dismiss/action glyph centring.** `gtk::toast::paint_toast` and
//    `macos::toast::paint_toast` both centre the dismiss `×` and the
//    action-button label horizontally within their reserved sub-region
//    (`dismiss_bounds`/`action_bounds`), using the glyph/label's own
//    measured width. `win::toast::paint_toast` drew both directly into
//    the full-height `db`/`ab` rect via `DWrite::draw_text`, which uses
//    DirectWrite's default (leading/near) alignment — flush against the
//    sub-region's own left edge, not centred. The shared `paint` adopts
//    the 2-of-3 (gtk/macos) centred shape uniformly, which visibly shifts
//    Win's dismiss `×` and action label rightward to the centre of their
//    reserved column. Reported here per this issue's instructions, rather
//    than silently picking one.
//
// `#[allow(dead_code)]`: see `primitives::scrollbar`'s identical note
// (#811) — only *called* once a real pixel backend is compiled in,
// exercised by each backend's own `Backend::draw_toast_stack` call site
// plus this module's own `RecordingSurface` tests on every leg that
// enables one of the three cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{
        ToastItem, ToastMeasure, ToastSeverity, ToastStack, ToastStackLayout, VisibleToast,
    };
    use crate::event::Rect;
    use crate::native_surface::NativeSurface;
    use crate::theme::Theme;
    use crate::types::Color;

    const TOAST_WIDTH: f32 = 320.0;
    const TOAST_MARGIN: f32 = 12.0;
    const TOAST_GAP: f32 = 8.0;
    const DISMISS_WIDTH: f32 = 28.0;
    const ACTION_PADDING: f32 = 16.0;
    const TOAST_PADDING: f32 = 8.0;

    /// Severity → fallback background tint, used when `ToastItem::accent`
    /// is `None`. Duplicated verbatim across `tui::toast` (out of scope
    /// for this `NativeSurface` migration — see the module doc's TUI
    /// note in `native_surface.rs`) — lifting these hardcoded colours
    /// into `Theme` is quadraui#815's job, not this one.
    fn severity_bg(severity: ToastSeverity, theme: &Theme) -> Color {
        match severity {
            ToastSeverity::Info => theme.surface_bg,
            ToastSeverity::Success => Color::rgb(30, 80, 30),
            ToastSeverity::Warning => Color::rgb(100, 80, 20),
            ToastSeverity::Error => theme.error_fg,
        }
    }

    /// Compute a [`ToastStack`]'s layout and paint it onto `surface` in
    /// one pass, returning the resolved [`ToastStackLayout`] for the
    /// caller's click dispatch — same contract as
    /// [`crate::Backend::draw_toast_stack`]. `line_height` is the
    /// caller's current text-row height (surface-native units); action
    /// label width is measured directly against `surface`, so a no-paint
    /// hit-test caller (each backend's own `*_toast_stack_layout`) must
    /// keep using its own equivalent measurement to agree with what this
    /// painted.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        stack: &ToastStack,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
        origin_x: f32,
        origin_y: f32,
        viewport_width: f32,
        viewport_height: f32,
        line_height: f32,
    ) -> ToastStackLayout {
        let layout = stack.layout(
            origin_x,
            origin_y,
            viewport_width,
            viewport_height,
            TOAST_MARGIN,
            TOAST_GAP,
            |i| {
                let toast = &stack.toasts[i];
                let h = if toast.body.is_empty() {
                    line_height + TOAST_PADDING * 2.0
                } else {
                    line_height * 2.0 + TOAST_PADDING * 2.0
                };
                let action_w = toast
                    .action
                    .as_ref()
                    .map(|a| {
                        let (w, _) = surface.surface_measure_text(&a.label);
                        w + ACTION_PADDING
                    })
                    .unwrap_or(0.0);
                ToastMeasure {
                    width: TOAST_WIDTH.min((viewport_width - TOAST_MARGIN * 2.0).max(0.0)),
                    height: h,
                    dismiss_width: DISMISS_WIDTH,
                    action_width: action_w,
                }
            },
        );

        for vt in &layout.visible_toasts {
            let toast = &stack.toasts[vt.toast_idx];
            paint_toast(surface, theme, vt, toast, line_height);
        }

        layout
    }

    /// Paint one resolved toast box: background tint, title, optional
    /// body (second line), dismiss `×` and optional action label — both
    /// of the latter centred horizontally within their own reserved
    /// sub-region, at the same vertical position as the title (see this
    /// module's doc, divergence 2).
    fn paint_toast(
        surface: &mut dyn NativeSurface,
        theme: &Theme,
        vt: &VisibleToast,
        toast: &ToastItem,
        line_height: f32,
    ) {
        let bg_color = toast
            .accent
            .unwrap_or_else(|| severity_bg(toast.severity, theme));
        surface.surface_fill_rect(vt.bounds, bg_color);

        let title_rect = Rect::new(
            vt.bounds.x + TOAST_PADDING,
            vt.bounds.y + TOAST_PADDING,
            (vt.bounds.width - TOAST_PADDING * 2.0).max(0.0),
            line_height,
        );
        surface.surface_draw_text_run(title_rect, &toast.title, theme.foreground);

        if !toast.body.is_empty() {
            let body_rect = Rect::new(
                title_rect.x,
                title_rect.y + line_height,
                title_rect.width,
                line_height,
            );
            surface.surface_draw_text_run(body_rect, &toast.body, theme.foreground);
        }

        if let Some(db) = vt.dismiss_bounds {
            let (tw, _) = surface.surface_measure_text("×");
            let rect = Rect::new(
                db.x + (db.width - tw) / 2.0,
                vt.bounds.y + TOAST_PADDING,
                db.width,
                db.height,
            );
            surface.surface_draw_text_run(rect, "×", theme.foreground);
        }

        if let (Some(ab), Some(action)) = (vt.action_bounds, &toast.action) {
            let (tw, _) = surface.surface_measure_text(&action.label);
            let rect = Rect::new(
                ab.x + (ab.width - tw) / 2.0,
                vt.bounds.y + TOAST_PADDING,
                ab.width,
                ab.height,
            );
            surface.surface_draw_text_run(rect, &action.label, theme.accent_fg);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::toast::{ToastAction, ToastCorner};
        use crate::types::WidgetId;
        use crate::Image;

        /// Records every surface verb this primitive's paint uses —
        /// mirrors `primitives::status_bar`'s `RecordingSurface` test
        /// double, so this test runs on any host without Cairo/Core
        /// Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl NativeSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(400.0, 300.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
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

        fn toast(id: &str, title: &str) -> ToastItem {
            ToastItem {
                id: WidgetId::new(id),
                title: title.into(),
                body: String::new(),
                severity: ToastSeverity::Info,
                action: None,
                accent: None,
            }
        }

        fn stack_br(toasts: Vec<ToastItem>) -> ToastStack {
            ToastStack {
                id: WidgetId::new("toasts"),
                corner: ToastCorner::BottomRight,
                toasts,
            }
        }

        #[test]
        fn empty_stack_paints_nothing() {
            let stack = stack_br(vec![]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(&stack, &mut surface, &theme, 0.0, 0.0, 400.0, 300.0, 16.0);
            assert!(layout.visible_toasts.is_empty());
            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
        }

        /// Background fill uses `accent` when present, else the
        /// severity's fallback tint.
        #[test]
        fn accent_overrides_severity_tint() {
            let accent = Color::rgb(10, 20, 30);
            let mut t = toast("t1", "Hello");
            t.accent = Some(accent);
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&stack, &mut surface, &theme, 0.0, 0.0, 400.0, 300.0, 16.0);
            assert_eq!(surface.fills[0].1, accent);
        }

        #[test]
        fn severity_tint_used_when_no_accent() {
            let mut t = toast("t1", "Hello");
            t.severity = ToastSeverity::Error;
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&stack, &mut surface, &theme, 0.0, 0.0, 400.0, 300.0, 16.0);
            assert_eq!(surface.fills[0].1, theme.error_fg);
        }

        /// Regression for this module's divergence 2: the dismiss glyph
        /// and action label must be painted centred within their own
        /// reserved sub-region, not flush against its left edge — the
        /// shape `win::toast` alone lacked pre-#861.
        #[test]
        fn dismiss_and_action_are_centred_in_their_sub_region() {
            let mut t = toast("t1", "Build failed");
            t.action = Some(ToastAction {
                id: WidgetId::new("open_log"),
                label: "Open log".into(),
            });
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(&stack, &mut surface, &theme, 0.0, 0.0, 400.0, 300.0, 16.0);
            let vt = &layout.visible_toasts[0];
            let db = vt.dismiss_bounds.expect("dismiss bounds present");
            let ab = vt.action_bounds.expect("action bounds present");

            let (dismiss_rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "×")
                .expect("dismiss glyph painted");
            let dismiss_w = 8.0; // RecordingSurface: 1 char * 8.0
            assert!((dismiss_rect.x - (db.x + (db.width - dismiss_w) / 2.0)).abs() < 0.01);

            let (action_rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "Open log")
                .expect("action label painted");
            let action_w = "Open log".chars().count() as f32 * 8.0;
            assert!((action_rect.x - (ab.x + (ab.width - action_w) / 2.0)).abs() < 0.01);
        }

        #[test]
        fn body_line_painted_below_title() {
            let mut t = toast("t1", "Title");
            t.body = "Body text".into();
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&stack, &mut surface, &theme, 0.0, 0.0, 400.0, 300.0, 16.0);
            let title_run = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "Title")
                .expect("title painted");
            let body_run = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "Body text")
                .expect("body painted");
            assert_eq!(body_run.0.y, title_run.0.y + 16.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Toast primitive tests (D6 shape, new B.3 primitive) ───────────

    fn make_toast(id: &str, title: &str) -> ToastItem {
        ToastItem {
            id: WidgetId::new(id),
            title: title.to_string(),
            body: String::new(),
            severity: ToastSeverity::Info,
            action: None,
            accent: None,
        }
    }

    fn make_toast_stack(corner: ToastCorner, toasts: Vec<ToastItem>) -> ToastStack {
        ToastStack {
            id: WidgetId::new("toasts"),
            corner,
            toasts,
        }
    }

    #[test]
    fn toast_layout_empty() {
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![]);
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| {
            ToastMeasure::new(300.0, 64.0)
        });
        assert_eq!(layout.visible_toasts.len(), 0);
        assert_eq!(layout.hit_test(100.0, 100.0), ToastHit::Empty);
    }

    #[test]
    fn toast_layout_bottom_right_newest_at_bottom() {
        let stack = make_toast_stack(
            ToastCorner::BottomRight,
            vec![
                make_toast("first", "First"),
                make_toast("second", "Second"),
                make_toast("third", "Third"),
            ],
        );
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| {
            ToastMeasure::new(300.0, 64.0)
        });
        assert_eq!(layout.visible_toasts.len(), 3);
        // Newest (idx=2, "third") pinned at the bottom.
        let newest = &layout.visible_toasts[0];
        assert_eq!(newest.toast_idx, 2);
        assert_eq!(newest.id.as_str(), "third");
        // Newest bottom = viewport_height (600) - margin (16) - toast_height (64) = 520
        assert_eq!(newest.bounds.y, 520.0);
        // Right-aligned: x = 800 - 16 - 300 = 484
        assert_eq!(newest.bounds.x, 484.0);
        // Second-newest above with gap.
        assert_eq!(layout.visible_toasts[1].id.as_str(), "second");
        assert_eq!(layout.visible_toasts[1].bounds.y, 520.0 - 8.0 - 64.0);
    }

    #[test]
    fn toast_layout_top_left_newest_at_top() {
        let stack = make_toast_stack(
            ToastCorner::TopLeft,
            vec![make_toast("a", "A"), make_toast("b", "B")],
        );
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 10.0, 5.0, |_| {
            ToastMeasure::new(200.0, 50.0)
        });
        assert_eq!(layout.visible_toasts.len(), 2);
        // Iteration is oldest-first for top corners.
        let first = &layout.visible_toasts[0];
        assert_eq!(first.id.as_str(), "a");
        assert_eq!(first.bounds.x, 10.0);
        assert_eq!(first.bounds.y, 10.0);
        let second = &layout.visible_toasts[1];
        assert_eq!(second.bounds.y, 10.0 + 50.0 + 5.0);
    }

    #[test]
    fn toast_layout_action_and_dismiss_regions() {
        let mut toast = make_toast("t1", "Build failed");
        toast.action = Some(ToastAction {
            id: WidgetId::new("open_log"),
            label: "Open log".to_string(),
        });
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![toast]);
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| ToastMeasure {
            width: 300.0,
            height: 64.0,
            dismiss_width: 24.0,
            action_width: 80.0,
        });
        let v = &layout.visible_toasts[0];
        assert!(v.dismiss_bounds.is_some());
        assert!(v.action_bounds.is_some());
        let db = v.dismiss_bounds.unwrap();
        let ab = v.action_bounds.unwrap();
        // Dismiss at trailing edge.
        assert_eq!(db.x + db.width, v.bounds.x + v.bounds.width);
        // Action left of dismiss.
        assert_eq!(ab.x + ab.width, db.x);

        // Hit-test on dismiss.
        match layout.hit_test(db.x + 5.0, db.y + 10.0) {
            ToastHit::Dismiss(id) => assert_eq!(id.as_str(), "t1"),
            _ => panic!("expected Dismiss hit"),
        }
        // Hit-test on action.
        match layout.hit_test(ab.x + 5.0, ab.y + 10.0) {
            ToastHit::Action(id) => assert_eq!(id.as_str(), "open_log"),
            _ => panic!("expected Action hit"),
        }
        // Hit-test on body (left part of toast, not on action/dismiss).
        match layout.hit_test(v.bounds.x + 5.0, v.bounds.y + 10.0) {
            ToastHit::Body(id) => assert_eq!(id.as_str(), "t1"),
            _ => panic!("expected Body hit"),
        }
    }

    #[test]
    fn toast_layout_stack_clips_when_out_of_room() {
        // 5 toasts of 64px each, but viewport only has 200 px from margin
        // to top. Should render as many as fit.
        let stack = make_toast_stack(
            ToastCorner::BottomRight,
            (0..5)
                .map(|i| make_toast(&format!("t{i}"), &format!("T{i}")))
                .collect(),
        );
        let layout = stack.layout(0.0, 0.0, 800.0, 200.0, 10.0, 8.0, |_| {
            ToastMeasure::new(300.0, 64.0)
        });
        // Bottom stack. Newest at y = 200 - 10 - 64 = 126. Each subsequent
        // goes up 64+8=72. Next: 126-72=54. Next: 54-72=-18 (would be off-top).
        // So only 2-3 fit. Specifically we break when y_cursor <= 0.
        assert!(layout.visible_toasts.len() >= 2);
        assert!(layout.visible_toasts.len() <= 3);
    }

    /// Non-zero-origin regression guard (quadraui#494 / LESSONS.md):
    /// `ToastStack::layout` previously had no origin parameter at all,
    /// so it could only ever be called with an implicit `(0, 0)`
    /// origin — a shape no `*_toast_stack_layout` backend wrapper could
    /// correct for. Confirms every returned bound (toast, dismiss,
    /// action, hit region) shifts rigidly by `(origin_x, origin_y)`
    /// relative to the origin-`(0, 0)` layout for the same stack.
    #[test]
    fn toast_layout_nonzero_origin_shifts_every_bound() {
        let mut toast = make_toast("t1", "Build failed");
        toast.action = Some(ToastAction {
            id: WidgetId::new("open_log"),
            label: "Open log".to_string(),
        });
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![toast]);
        let measure = |_: usize| ToastMeasure {
            width: 300.0,
            height: 64.0,
            dismiss_width: 24.0,
            action_width: 80.0,
        };
        let origin = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, measure);
        let shifted = stack.layout(7.0, 13.0, 800.0, 600.0, 16.0, 8.0, measure);

        let o = &origin.visible_toasts[0];
        let s = &shifted.visible_toasts[0];
        assert_eq!(s.bounds.x, o.bounds.x + 7.0);
        assert_eq!(s.bounds.y, o.bounds.y + 13.0);
        assert_eq!(s.bounds.width, o.bounds.width);
        assert_eq!(
            s.dismiss_bounds.unwrap().x,
            o.dismiss_bounds.unwrap().x + 7.0
        );
        assert_eq!(
            s.action_bounds.unwrap().y,
            o.action_bounds.unwrap().y + 13.0
        );

        // Round trip: an absolute hit against the shifted layout must
        // resolve the same way the origin layout resolves its local hit.
        let db = s.dismiss_bounds.unwrap();
        assert_eq!(
            shifted.hit_test(db.x + 5.0, db.y + 10.0),
            ToastHit::Dismiss(WidgetId::new("t1")),
        );
    }
}
