//! `Toast` primitive: a transient corner notification with optional
//! severity tint and zero or more action buttons. Used for "File saved",
//! "LSP disconnected", "3 errors in src/foo.rs", "Install Markdown
//! Language Server?", etc.
//!
//! Toasts are ephemeral — the app owns their lifecycle (show, auto-dismiss
//! after a duration, manual dismiss) and passes the primitive the current
//! set of visible toasts each frame. The primitive itself does not tick
//! time or auto-dismiss; those are app concerns.
//!
//! # Backend contract
//!
//! **Declarative + overlay.** Render toasts stacked in the configured
//! `corner`, VS Code-style (#1185): title/body wrap across the box's
//! full width, growing its height up to a cap ([`MAX_BODY_LINES`])
//! before ellipsizing; the dismiss `×` sits alone near the top-right;
//! any [`Toast::actions`] sit on their own row at the bottom-right,
//! never inline with the title. Clicks resolve via
//! [`ToastStackLayout::hit_test`] / [`ToastHit`]: an action button hits
//! `ToastHit::Action`; the dismiss affordance hits `ToastHit::Dismiss`.
//!
//! Toast boxes take keyboard focus only when an app explicitly hands it
//! to them, via [`crate::compose::ToastStackController`] (#1185) —
//! they're otherwise strictly a passive notification surface, and never
//! steal focus or block input on their own (unlike a modal
//! [`crate::primitives::dialog::Dialog`]). `ToastOverlay::focus`, set from
//! the controller, is what makes a backend paint a focus ring at all.
//!
//! Stacking direction: bottom-corner toasts grow upward (newest nearest
//! the corner); top-corner toasts grow downward. `ToastOverlay::layout()`
//! handles this based on `corner`.
//!
//! # Naming
//!
//! [`ToastOverlay`] (a corner's worth of toasts) holds [`Toast`]s, each
//! holding [`ToastButton`]s. Their pre-#1185 single-action counterparts —
//! `ToastStack`, `ToastItem`, `ToastAction` — were deprecated alongside
//! these shapes and removed outright in issue #1251, once both known
//! consumers had migrated (zero remaining uses); see `CHANGELOG.md`'s
//! `Removed` entry.

use std::collections::HashMap;
use std::time::Instant;

use crate::event::Rect;
use crate::transition::{Easing, Transition, CHROME_TRANSITION_DURATION};
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a toast stack for one corner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastOverlay {
    pub id: WidgetId,
    /// Which corner of the viewport the stack occupies.
    pub corner: ToastCorner,
    /// Toasts in temporal order — oldest first. Visual order depends on
    /// `corner` (bottom corners stack upward, top corners stack downward).
    pub toasts: Vec<Toast>,
    /// Which control (if any) currently has keyboard focus (#1185).
    /// `None` — the common case, since toasts are non-modal and never
    /// steal focus on their own — paints with no focus ring. Set this
    /// from [`crate::compose::ToastStackController::focus`] before
    /// calling [`crate::Backend::draw_toast_overlay`] to make the
    /// controller's cursor visible; the primitive itself never mutates
    /// this field (declarative, like every other field here).
    #[serde(default)]
    pub focus: Option<ToastFocus>,
}

/// Corner placement for a `ToastOverlay`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToastCorner {
    #[default]
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

/// One toast notification.
///
/// # Examples
///
/// ```
/// use quadraui::{Toast, ToastHit, ToastMeasure, ToastOverlay, ToastSeverity, WidgetId};
///
/// let toast = Toast {
///     id: WidgetId::new("toast:saved"),
///     title: "File saved".to_string(),
///     body: String::new(),
///     severity: ToastSeverity::Success,
///     actions: vec![],
///     accent: None,
/// };
/// let overlay = ToastOverlay {
///     id: WidgetId::new("toasts:bottom_right"),
///     corner: Default::default(),
///     toasts: vec![toast.clone()],
///     focus: None,
/// };
///
/// let layout = overlay.layout(0.0, 0.0, 80.0, 24.0, 1.0, 1.0, |_| {
///     ToastMeasure::new(30.0, 3.0)
/// });
///
/// assert_eq!(layout.visible_toasts.len(), 1);
/// assert_eq!(layout.hit_test(60.0, 21.0), ToastHit::Body(toast.id));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toast {
    pub id: WidgetId,
    pub title: String,
    /// Body text. Can be empty for minimal "File saved" style toasts.
    #[serde(default)]
    pub body: String,
    /// Visual severity — backends tint the box accordingly.
    #[serde(default)]
    pub severity: ToastSeverity,
    /// Ordered action buttons (#1185 — was `ToastItem::action: Option<ToastAction>`, at
    /// most one). Rendered on their own row at the bottom-right of the
    /// toast box, never inline with the title. Empty = no action row;
    /// just the dismiss affordance is clickable. At most one entry
    /// should set [`ToastButton::primary`] — see that field's doc.
    #[serde(default)]
    pub actions: Vec<ToastButton>,
    /// Override severity's default tint. Most toasts use `None` and let
    /// the theme decide.
    #[serde(default)]
    pub accent: Option<Color>,
}

/// Severity level of a `Toast`.
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
pub struct ToastButton {
    pub id: WidgetId,
    pub label: String,
    /// Styled with the theme's accent (filled background) instead of the
    /// plain secondary-button look (#1185). VS Code convention: at most
    /// one action per toast is primary — the "do the thing" button (e.g.
    /// "Install"), with the rest ("Don't ask again", …) staying
    /// secondary. Backends don't enforce the "at most one" rule; a
    /// caller that sets it on more than one action just gets more than
    /// one accent-filled button.
    #[serde(default)]
    pub primary: bool,
}

/// Keyboard-focus target within a [`ToastOverlay`] (#1185) — set
/// [`ToastOverlay::focus`] to one of these so every backend's rasteriser
/// draws a visible focus ring around it. Produced by
/// [`crate::compose::ToastStackController`]; the primitive layer only
/// consumes it for painting, never computes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastFocus {
    /// Which toast (by [`Toast::id`]) currently has keyboard focus.
    pub toast_id: WidgetId,
    /// Which control within that toast is focused.
    pub target: ToastFocusTarget,
}

/// Which control within a focused toast has keyboard focus — see
/// [`ToastFocus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToastFocusTarget {
    /// The dismiss `×` affordance.
    Dismiss,
    /// One of the toast's action buttons, by index into
    /// [`Toast::actions`].
    Action(usize),
}

// ── Text wrapping (#1182) ───────────────────────────────────────────────────
//
// Shared between every backend's toast rasteriser: the GUI paint
// (`native_surface_paint::paint`), its no-paint layout twin
// (`crate::primitives::layout_metrics::pixel_toast_stack_layout`), and
// `tui::toast`. Before this, none of them wrapped or truncated title/body
// text at all — a long title ran straight through the action button's
// reserved column, and a long body was drawn as one line that simply
// extended past the toast box (and, on GUI, past the window). Unit-
// agnostic (`width_of` is pixel width on GUI surfaces, character count on
// TUI cells) so one implementation covers both.

/// Max number of wrapped lines a toast body grows to before its last line
/// is ellipsized. Shared by the paint/layout pair on every pixel backend
/// and by `tui::toast`, so a no-paint layout call always predicts the same
/// box height the matching paint call actually drew.
pub(crate) const MAX_BODY_LINES: usize = 3;

/// Greedy word-wrap `text` into lines that each measure `<= max_width`
/// under `width_of`. Caps output at `max_lines`; if words remain past
/// that cap, the last line is trimmed and suffixed with `…` so the cut is
/// visible rather than silently dropped. A single word wider than
/// `max_width` on its own (a long URL/identifier with no spaces) is
/// hard-truncated with `…` instead of overflowing or looping forever
/// trying to fit it.
///
/// `max_lines == 1` is how callers get "truncate/ellipsize, never wrap"
/// behaviour for a title; `max_lines == `[`MAX_BODY_LINES`] is how the
/// body grows the toast's height instead.
pub(crate) fn wrap_text_lines(
    text: &str,
    max_width: f32,
    max_lines: usize,
    width_of: &dyn Fn(&str) -> f32,
) -> Vec<String> {
    if text.is_empty() || max_lines == 0 {
        return Vec::new();
    }
    if max_width <= 0.0 {
        return vec![text.to_string()];
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<String> = Vec::new();
    let mut idx = 0;
    while idx < words.len() && lines.len() < max_lines {
        let mut line = String::new();
        loop {
            if idx >= words.len() {
                break;
            }
            let word = words[idx];
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if width_of(&candidate) <= max_width {
                line = candidate;
                idx += 1;
                continue;
            }
            if line.is_empty() {
                // A single word wider than max_width on its own:
                // hard-truncate it with an ellipsis rather than looping
                // forever trying (and failing) to fit it whole.
                let mut piece = String::new();
                for ch in word.chars() {
                    let next = format!("{piece}{ch}…");
                    if width_of(&next) > max_width && !piece.is_empty() {
                        break;
                    }
                    piece.push(ch);
                }
                piece.push('…');
                line = piece;
                idx += 1;
            }
            break;
        }
        lines.push(line);
    }

    // Words remain past `max_lines`: ellipsize the last line so the cut
    // is visible rather than silently dropped.
    if idx < words.len() {
        if let Some(last) = lines.last_mut() {
            while !last.is_empty() && width_of(&format!("{last}…")) > max_width {
                last.pop();
            }
            last.push('…');
        }
    }

    lines
}

/// Truncate `text` to a single line that fits `max_width`, ellipsizing if
/// it doesn't — the title's shape (never wraps, unlike the body).
///
/// `#[cfg_attr(not(any(<every backend feature>)), allow(dead_code))]`: the
/// only callers are *rasterisers* — `tui::toast` and
/// `native_surface_paint::paint` (gtk/win/macos) — unlike its sibling
/// [`wrap_text_lines`], which `primitives::layout_metrics` also calls from
/// the ungated no-paint layout path (the title is always one line, so it
/// can't change a toast's height and layout never needs to truncate it).
/// A featureless `cargo check -p quadraui` therefore compiles no caller at
/// all. Kept compiled (rather than `#[cfg]`-ed out) on every leg so the
/// `wrap_tests` unit tests below cover it with no features enabled too.
#[cfg_attr(
    not(any(
        feature = "tui",
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )),
    allow(dead_code)
)]
pub(crate) fn truncate_line(text: &str, max_width: f32, width_of: &dyn Fn(&str) -> f32) -> String {
    wrap_text_lines(text, max_width, 1, width_of)
        .into_iter()
        .next()
        .unwrap_or_default()
}

#[cfg(test)]
mod wrap_tests {
    use super::{truncate_line, wrap_text_lines};

    /// Character-count "pixel" metric — simplest deterministic `width_of`
    /// for these unit tests (matches how `tui::toast` measures).
    fn chars(s: &str) -> f32 {
        s.chars().count() as f32
    }

    #[test]
    fn short_text_is_one_line_unchanged() {
        let lines = wrap_text_lines("Hello", 20.0, 3, &chars);
        assert_eq!(lines, vec!["Hello".to_string()]);
    }

    #[test]
    fn wraps_on_word_boundaries() {
        let lines = wrap_text_lines("one two three four", 9.0, 3, &chars);
        // "one two" = 7 chars fits in 9; adding "three" (13) doesn't.
        assert_eq!(lines[0], "one two");
        for line in &lines {
            assert!(chars(line) <= 9.0, "line {line:?} exceeds max_width");
        }
    }

    #[test]
    fn ellipsizes_last_line_when_content_overflows_max_lines() {
        let lines = wrap_text_lines("a b c d e f g h", 3.0, 2, &chars);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'));
        for line in &lines {
            assert!(chars(line) <= 3.0, "line {line:?} exceeds max_width");
        }
    }

    #[test]
    fn truncate_line_never_wraps() {
        let line = truncate_line("Install Markdown Language Server?", 10.0, &chars);
        assert!(chars(&line) <= 10.0);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn single_word_wider_than_max_width_is_hard_truncated() {
        let lines = wrap_text_lines("supercalifragilisticexpialidocious", 5.0, 1, &chars);
        assert_eq!(lines.len(), 1);
        assert!(
            chars(&lines[0]) <= 5.0,
            "line {:?} exceeds max_width",
            lines[0]
        );
        assert!(lines[0].ends_with('…'));
    }

    #[test]
    fn empty_text_yields_no_lines() {
        assert!(wrap_text_lines("", 20.0, 3, &chars).is_empty());
    }
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// First new B.3 primitive on D6. Toasts stack in a corner with uniform
// spacing; per-toast sizes are backend-supplied (a "body"-less toast is
// shorter than one with a multi-line body).

/// Per-toast measurement supplied by the backend.
///
/// `dismiss_rect` / `action_rects` are **toast-local** (relative to the
/// toast box's own top-left corner, not the viewport) — [`ToastOverlay::layout`]
/// translates them into absolute bounds itself, the same way it already
/// positions `width`/`height` into the stack. This lets each backend's
/// measure closure own its own padding/row-placement geometry (VS
/// Code-style: dismiss top-right, actions on their own row at the
/// bottom-right — see [`toast_button_rects`], the shared helper every
/// in-tree measure closure uses to compute both) without
/// `ToastOverlay::layout` itself needing to know any padding constant.
#[derive(Debug, Clone, PartialEq)]
pub struct ToastMeasure {
    /// Full width of the toast box in the backend's unit.
    pub width: f32,
    /// Full height of the toast box.
    pub height: f32,
    /// Toast-local dismiss-affordance rect. `None` if no dismiss UI is
    /// drawn.
    pub dismiss_rect: Option<Rect>,
    /// Toast-local action-button rects, one per [`Toast::actions`]
    /// entry, same order. Empty if the toast has no actions.
    pub action_rects: Vec<Rect>,
}

impl ToastMeasure {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            dismiss_rect: None,
            action_rects: Vec::new(),
        }
    }
}

/// Compute a toast's dismiss + action-button rects, toast-local
/// (relative to the toast box's own top-left) — shared by every
/// backend's measure closure (#1185: `native_surface_paint::paint`,
/// `tui::toast`, `layout_metrics::pixel_toast_stack_layout`) so the `×`
/// affordance's top-right position and the action row's right-aligned,
/// bottom-edge placement are computed identically everywhere, whether
/// the caller's unit is pixels or terminal cells.
///
/// VS Code shape: `×` sits alone near the top-right, clear of the title
/// row below it in height so it never overlaps a wrapped multi-line
/// title/body; actions sit on their own row at the bottom-right,
/// right-to-left (`action_widths[0]` ends up leftmost in the row),
/// never inline with the title. `action_widths` is each action's
/// already-measured, already-padded button width, in
/// [`Toast::actions`] order; the returned `Vec` is aligned 1:1 with
/// it. Both rects are clamped to stay non-negative even if the box is
/// smaller than the sum of its own padding/button widths (a degenerate
/// but non-panicking box, matching this module's existing
/// `.max(0.0)`-everywhere convention).
#[allow(clippy::too_many_arguments)]
pub(crate) fn toast_button_rects(
    width: f32,
    height: f32,
    padding: f32,
    dismiss_width: f32,
    dismiss_height: f32,
    action_widths: &[f32],
    action_height: f32,
    action_gap: f32,
) -> (Option<Rect>, Vec<Rect>) {
    let dismiss_rect = if dismiss_width > 0.0 && dismiss_height > 0.0 {
        Some(Rect::new(
            (width - padding - dismiss_width).max(0.0),
            padding,
            dismiss_width,
            dismiss_height,
        ))
    } else {
        None
    };

    let mut action_rects: Vec<Rect> = Vec::with_capacity(action_widths.len());
    if !action_widths.is_empty() && action_height > 0.0 {
        let row_y = (height - padding - action_height).max(0.0);
        let mut x_cursor = width - padding;
        for w in action_widths.iter().rev() {
            x_cursor -= w;
            action_rects.push(Rect::new(x_cursor.max(0.0), row_y, *w, action_height));
            x_cursor -= action_gap;
        }
        action_rects.reverse();
    }
    (dismiss_rect, action_rects)
}

/// Resolved position of one visible toast after layout.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleToast {
    /// Index into `ToastOverlay.toasts`.
    pub toast_idx: usize,
    pub id: WidgetId,
    /// Full toast box bounds.
    pub bounds: Rect,
    /// Dismiss affordance (if present).
    pub dismiss_bounds: Option<Rect>,
    /// Action-button bounds, one per [`Toast::actions`] entry, same
    /// order. Empty if the toast has no actions.
    ///
    /// The pre-#1185 single-action mirror field, `action_bounds`
    /// (always equal to `action_rects.first().copied()`), was removed
    /// in issue #1109 (zero uses in coord-tui's `main` and vimcode's
    /// `develop`).
    pub action_rects: Vec<Rect>,
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

/// Drives toast slide-in/slide-out via one [`Transition`] per toast,
/// keyed by [`WidgetId`].
///
/// Toast/[`ToastOverlay`] stay declarative (`PartialEq`, `Eq`,
/// `Serialize`) on purpose — see this module's top doc — so the
/// slide progress for each toast lives here instead, owned by
/// whatever controller owns the overlay's lifecycle
/// (`crate::compose::ToastStackController` today). Call [`Self::show_at`]
/// when a toast first appears and [`Self::dismiss_at`] when it starts
/// leaving (before actually removing it from
/// [`ToastOverlay::toasts`] — the toast needs to still be in the
/// declarative list for its slide-out to paint at all), then pass
/// `self` to [`ToastOverlay::layout_with_motion`] instead of
/// [`ToastOverlay::layout`].
///
/// A toast with no tracked transition reports [`Self::progress`] as
/// `1.0` (fully shown) — the TUI convention: never call
/// `show_at`/`dismiss_at` at all, and every toast paints at its
/// settled position immediately, with [`Self::is_animating`] always
/// `false`. GTK/macOS/Win call `show_at`/`dismiss_at` to get the
/// slide.
#[derive(Debug, Clone, Default)]
pub struct ToastMotion {
    transitions: HashMap<WidgetId, Transition>,
}

impl ToastMotion {
    /// An empty tracker — every toast reports `progress == 1.0` (fully
    /// shown, no animation) until [`Self::show_at`]/[`Self::dismiss_at`]
    /// is called for its id.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or retarget, if already mid-transition) `id`'s slide
    /// toward fully shown (`1.0`), anchored at `now`.
    pub fn show_at(&mut self, id: WidgetId, now: Instant) {
        self.retarget_at(id, 1.0, now);
    }

    /// Start (or retarget) `id`'s slide toward fully hidden (`0.0`),
    /// anchored at `now`. The caller is responsible for actually
    /// removing the toast from [`ToastOverlay::toasts`] once
    /// [`Self::progress`] reaches `0.0` (or [`Self::is_animating`]
    /// settles) — this type only drives the number, not the
    /// overlay's declarative contents.
    pub fn dismiss_at(&mut self, id: WidgetId, now: Instant) {
        self.retarget_at(id, 0.0, now);
    }

    fn retarget_at(&mut self, id: WidgetId, to: f32, now: Instant) {
        match self.transitions.get_mut(&id) {
            Some(existing) => existing.retarget_at(now, to),
            None => {
                let from = 1.0 - to;
                self.transitions.insert(
                    id,
                    Transition::start_at(
                        now,
                        from,
                        to,
                        CHROME_TRANSITION_DURATION,
                        Easing::EaseOut,
                    ),
                );
            }
        }
    }

    /// Slide progress for `id` at `now` — `0.0` fully hidden (off the
    /// nearest viewport edge), `1.0` fully shown. `1.0` for any id with
    /// no tracked transition (see this type's doc for why that's the
    /// TUI-friendly default).
    pub fn progress(&self, id: &WidgetId, now: Instant) -> f32 {
        self.transitions
            .get(id)
            .map(|t| t.value_at(now))
            .unwrap_or(1.0)
    }

    /// Whether any tracked toast is still mid-slide at `now` — the
    /// signal for the `Reaction::RedrawAfter` chained-rearm pattern
    /// (`crate::runner::chrome_transition_reaction`), same shape as
    /// [`crate::InteractionState::is_animating`].
    pub fn is_animating(&self, now: Instant) -> bool {
        self.transitions.values().any(|t| !t.is_done_at(now))
    }

    /// Drop every settled transition — bounds memory growth for an app
    /// that shows many short-lived toasts over a long session. Safe to
    /// call every frame; a toast mid-slide is left untouched.
    pub fn prune(&mut self, now: Instant) {
        self.transitions.retain(|_, t| !t.is_done_at(now));
    }

    /// Whether no toast has a tracked transition at all. Mostly useful
    /// for tests asserting [`Self::prune`] actually dropped something,
    /// since a settled and an untracked toast otherwise look identical
    /// through [`Self::progress`] (both report `1.0`/`0.0` at rest).
    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty()
    }
}

impl ToastOverlay {
    /// [`Self::layout`] plus per-toast slide-in/slide-out from `motion`.
    /// Every box ([`VisibleToast::bounds`],
    /// `dismiss_bounds`, `action_rects`) and every
    /// [`ToastStackLayout::hit_regions`] entry is shifted vertically by
    /// `(1.0 - motion.progress(id, now)) * box_height`, toward the
    /// viewport edge nearest the stack's corner (bottom corners rise up
    /// from below; top corners drop down from above) — the direction a
    /// VS Code-style toast actually slides.
    ///
    /// At `progress == 1.0` for every visible toast (nothing currently
    /// animating, or a caller that never calls
    /// [`ToastMotion::show_at`]/[`ToastMotion::dismiss_at`] at all — the
    /// TUI convention) this returns exactly what [`Self::layout`] would
    /// have, box for box; see `tui_snaps_to_the_final_layout_with_no_motion_calls`.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_with_motion<F>(
        &self,
        origin_x: f32,
        origin_y: f32,
        viewport_width: f32,
        viewport_height: f32,
        margin: f32,
        gap: f32,
        motion: &ToastMotion,
        now: Instant,
        measure_toast: F,
    ) -> ToastStackLayout
    where
        F: Fn(usize) -> ToastMeasure,
    {
        let mut layout = self.layout(
            origin_x,
            origin_y,
            viewport_width,
            viewport_height,
            margin,
            gap,
            measure_toast,
        );

        let is_bottom = matches!(
            self.corner,
            ToastCorner::BottomRight | ToastCorner::BottomLeft
        );

        for vt in &mut layout.visible_toasts {
            let progress = motion.progress(&vt.id, now);
            let hidden_offset = (1.0 - progress) * vt.bounds.height;
            let dy = if is_bottom {
                hidden_offset
            } else {
                -hidden_offset
            };
            if dy == 0.0 {
                continue;
            }
            vt.bounds = shift_rect(vt.bounds, 0.0, dy);
            vt.dismiss_bounds = vt.dismiss_bounds.map(|r| shift_rect(r, 0.0, dy));
            vt.action_rects = vt
                .action_rects
                .iter()
                .map(|r| shift_rect(*r, 0.0, dy))
                .collect();
        }

        // Hit regions are rebuilt from the now-shifted `visible_toasts`
        // rather than shifted in place, mirroring `Self::layout`'s own
        // dismiss/action/body ordering exactly (specificity order:
        // dismiss, then actions, then body).
        let mut hit_regions: Vec<(Rect, ToastHit)> = Vec::new();
        for vt in &layout.visible_toasts {
            if let Some(db) = vt.dismiss_bounds {
                hit_regions.push((db, ToastHit::Dismiss(vt.id.clone())));
            }
            if let Some(toast) = self.toasts.get(vt.toast_idx) {
                for (ab, act) in vt.action_rects.iter().zip(toast.actions.iter()) {
                    hit_regions.push((*ab, ToastHit::Action(act.id.clone())));
                }
            }
            hit_regions.push((vt.bounds, ToastHit::Body(vt.id.clone())));
        }
        layout.hit_regions = hit_regions;

        layout
    }

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

            // Sub-regions come pre-computed toast-local by the measure
            // closure (#1185 — dismiss near the top-right, actions on
            // their own row at the bottom-right; see `toast_button_rects`)
            // — this loop only translates them into the stack's absolute
            // frame, same as `bounds` itself.
            let dismiss_bounds = m.dismiss_rect.map(|r| shift_rect(r, bounds.x, bounds.y));
            let action_rects: Vec<Rect> = m
                .action_rects
                .iter()
                .map(|r| shift_rect(*r, bounds.x, bounds.y))
                .collect();

            let toast_id = self.toasts[i].id.clone();
            visible_toasts.push(VisibleToast {
                toast_idx: i,
                id: toast_id.clone(),
                bounds,
                dismiss_bounds,
                action_rects: action_rects.clone(),
            });

            // Register hit regions in specificity order: dismiss, actions, body.
            if let Some(db) = dismiss_bounds {
                hit_regions.push((db, ToastHit::Dismiss(toast_id.clone())));
            }
            // Action hit regions carry the action's own id (not the
            // toast's) so the app can dispatch the intended action
            // directly from the hit result. Zipped by position with
            // `Toast::actions`, matching `action_rects`'s documented
            // 1:1 order.
            for (ab, act) in action_rects.iter().zip(self.toasts[i].actions.iter()) {
                hit_regions.push((*ab, ToastHit::Action(act.id.clone())));
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
        // convention. Before this, `ToastOverlay::layout` had no origin
        // parameter at all, so every backend's `*_toast_stack_layout`
        // silently dropped `rect.x` / `rect.y` — invisible at the origin
        // (every prior test) and a real drift for any non-zero-origin
        // overlay (quadraui#494 / LESSONS.md "Layout helpers must return
        // coords in the same frame across backends").
        if origin_x != 0.0 || origin_y != 0.0 {
            for vt in &mut visible_toasts {
                vt.bounds = shift_rect(vt.bounds, origin_x, origin_y);
                vt.dismiss_bounds = vt.dismiss_bounds.map(|r| shift_rect(r, origin_x, origin_y));
                vt.action_rects = vt
                    .action_rects
                    .iter()
                    .map(|r| shift_rect(*r, origin_x, origin_y))
                    .collect();
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

// ── PaintSurface paint (#861, Phase 2d slice 4/9 of the PaintSurface
// milestone) ────────────────────────────────────────────────────────────
//
// Before this, `gtk::toast::draw_toast_stack` (Cairo/Pango),
// `macos::toast::draw_toast_stack` (Core Graphics/Core Text) and
// `win::toast::draw_toast_stack` (Direct2D/DirectWrite) each independently
// painted the same toast box (background tint, title/body text, dismiss
// glyph, action label) with their own drawing API (quadraui#785 child
// #811, `docs/SMELL_AUDIT_2026-07.md` §5). `paint` below is the one
// shared implementation, written against
// [`crate::paint_surface::PaintSurface`] (#807, Phase 1) instead of any
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
// 3. **Body-line vertical offset.** `gtk::toast::paint_toast` and
//    `macos::toast::paint_toast` both positioned the body (second) line
//    at the title's own *measured* pixel height — gtk via
//    `pango_layout.pixel_size().1`, macos via `measure_text(font,
//    &toast.title)` — and neither ever used the `line_height` parameter
//    for this offset (gtk's was even named `_line_height`, unused).
//    `win::toast::paint_toast` was the outlier, using the nominal
//    `line_height` argument instead. The shared `paint_toast` adopts the
//    2-of-3 (gtk/macos) measured-height shape uniformly — calling
//    `surface.surface_measure_text(&toast.title)` and using its height
//    for the body offset — which visibly moves Win's body line whenever
//    a title's real rendered height differs from its nominal
//    `line_height`. Reported here per this issue's instructions, rather
//    than silently picking one.
//
// 4. **Toast-width clamp floor.** `gtk::toast::gtk_toast_stack_layout`
//    and `macos::toast::macos_toast_stack_layout` computed
//    `TOAST_WIDTH.min(viewport_width - TOAST_MARGIN * 2.0)` with no
//    floor — a viewport narrower than `TOAST_MARGIN * 2` (24px) could
//    drive the resolved width negative. Only
//    `win::toast::win_toast_stack_layout` already clamped the subtrahend
//    to `.max(0.0)`. The shared `paint` adopts Win's (safer) clamped
//    form for all three below. This only changes anything for a
//    degenerate viewport under 24px wide, and is strictly protective
//    (a `ToastMeasure` can never receive a negative width), but is
//    named here for the same transparency reason as 1–3.
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
        toast_button_rects, truncate_line, wrap_text_lines, Toast, ToastFocus, ToastFocusTarget,
        ToastMeasure, ToastOverlay, ToastSeverity, ToastStackLayout, VisibleToast, MAX_BODY_LINES,
    };
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::Color;

    const TOAST_WIDTH: f32 = 320.0;
    const TOAST_MARGIN: f32 = 12.0;
    const TOAST_GAP: f32 = 8.0;
    const DISMISS_WIDTH: f32 = 28.0;
    const ACTION_PADDING: f32 = 16.0;
    /// Horizontal gap between two adjacent action buttons on the button
    /// row (#1185).
    const ACTION_GAP: f32 = 8.0;
    const TOAST_PADDING: f32 = 8.0;

    /// Severity → fallback background tint, used when `Toast::accent`
    /// is `None`. Duplicated verbatim across `tui::toast` (out of scope
    /// for this `PaintSurface` migration — see the module doc's TUI
    /// note in `paint_surface.rs`) — lifting these hardcoded colours
    /// into `Theme` is quadraui#815's job, not this one.
    fn severity_bg(severity: ToastSeverity, theme: &Theme) -> Color {
        match severity {
            ToastSeverity::Info => theme.surface_bg,
            ToastSeverity::Success => Color::rgb(30, 80, 30),
            ToastSeverity::Warning => Color::rgb(100, 80, 20),
            ToastSeverity::Error => theme.error_fg,
        }
    }

    /// Compute a [`ToastOverlay`]'s layout and paint it onto `surface` in
    /// one pass, returning the resolved [`ToastStackLayout`] for the
    /// caller's click dispatch — same contract as
    /// [`crate::Backend::draw_toast_stack`]. `line_height` is the
    /// caller's current text-row height (surface-native units); action
    /// label width is measured directly against `surface`, so a no-paint
    /// hit-test caller (each backend's own `*_toast_stack_layout`) must
    /// keep using its own equivalent measurement to agree with what this
    /// painted. `style` supplies the corner radius/border
    /// width/shadow elevation every toast box paints with — callers pass
    /// `&self.style()`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        stack: &ToastOverlay,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
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
                let width = TOAST_WIDTH.min((viewport_width - TOAST_MARGIN * 2.0).max(0.0));
                let body_avail = (width - TOAST_PADDING * 2.0).max(0.0);
                let body_lines = if toast.body.is_empty() {
                    0
                } else {
                    wrap_text_lines(&toast.body, body_avail, MAX_BODY_LINES, &|s| {
                        surface.surface_measure_text(s).0
                    })
                    .len()
                    .max(1)
                };
                // Button row (#1185): a separate row below the title/body,
                // added only when the toast has actions — never inline
                // with the title, unlike the pre-#1185 trailing-column
                // reservation.
                let has_actions = !toast.actions.is_empty();
                let button_row_h = if has_actions {
                    TOAST_PADDING + line_height
                } else {
                    0.0
                };
                let h = line_height
                    + TOAST_PADDING * 2.0
                    + body_lines as f32 * line_height
                    + button_row_h;
                let action_widths: Vec<f32> = toast
                    .actions
                    .iter()
                    .map(|a| surface.surface_measure_text(&a.label).0 + ACTION_PADDING)
                    .collect();
                let (dismiss_rect, action_rects) = toast_button_rects(
                    width,
                    h,
                    TOAST_PADDING,
                    DISMISS_WIDTH,
                    // Height is a single text row, not `DISMISS_WIDTH`
                    // (a wide *click-target*, not a tall one) — a short,
                    // body-less, action-less toast is barely taller than
                    // one row plus padding, and a square dismiss target
                    // that tall would reach past the button row (#1185).
                    line_height,
                    &action_widths,
                    line_height,
                    ACTION_GAP,
                );
                ToastMeasure {
                    width,
                    height: h,
                    dismiss_rect,
                    action_rects,
                }
            },
        );

        for vt in &layout.visible_toasts {
            let toast = &stack.toasts[vt.toast_idx];
            let focus = stack.focus.as_ref().filter(|f| f.toast_id == toast.id);
            paint_toast(surface, theme, style, vt, toast, line_height, focus);
        }

        layout
    }

    /// Paint one resolved toast box: background tint, a theme border
    /// (#1182 — previously absent on every backend, which left an Info
    /// toast's `surface_bg` fill blending into a light-theme window),
    /// title, optional body, a top-right dismiss `×`, and — on their own
    /// row at the bottom-right, never inline with the title (#1185) —
    /// zero or more action buttons: the one marked
    /// [`crate::primitives::toast::ToastButton::primary`] filled with
    /// `theme.accent_bg`/`theme.foreground` (matching
    /// `tui::toolbar`'s focused-action pairing), the rest plain text in
    /// `theme.link_fg` (a secondary/ghost-button look, matching that
    /// field's existing "focused-popup border" role — see
    /// `crate::theme::Theme::link_fg`'s doc). `focus`, when it names a
    /// control on this toast, gets a `theme.link_fg` focus-ring stroke
    /// (#1185 — every backend must draw one; see
    /// [`crate::compose::ToastStackController`]).
    ///
    /// The title is truncated/ellipsized to the space left of the
    /// dismiss button — never drawn under it (#1182/#1185: the action
    /// row moved off the title line entirely, so the title only needs
    /// to dodge `×` now). The body wraps across up to [`MAX_BODY_LINES`]
    /// lines at the box's full width, offset below the title by the
    /// title's own *measured* height, not the nominal `line_height`
    /// (divergence 3), each subsequent line advancing by `line_height`.
    fn paint_toast(
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        vt: &VisibleToast,
        toast: &Toast,
        line_height: f32,
        focus: Option<&ToastFocus>,
    ) {
        let bg_color = toast
            .accent
            .unwrap_or_else(|| severity_bg(toast.severity, theme));
        // VS-Code-style elevated card: a shadow behind, then a rounded
        // fill and a rounded border.
        surface.surface_draw_shadow(
            vt.bounds,
            style.corner_radius,
            style.shadow_elevation,
            Color::rgba(0, 0, 0, 100),
        );
        surface.surface_fill_rounded_rect(vt.bounds, style.corner_radius, bg_color);
        surface.surface_stroke_rounded_rect(
            vt.bounds,
            style.corner_radius,
            theme.border_fg,
            style.border_width,
        );

        let dismiss_w = vt.dismiss_bounds.map(|d| d.width).unwrap_or(0.0);
        let title_avail_w = (vt.bounds.width - TOAST_PADDING * 2.0 - dismiss_w).max(0.0);
        let title_line = truncate_line(&toast.title, title_avail_w, &|s| {
            surface.surface_measure_text(s).0
        });

        let title_rect = Rect::new(
            vt.bounds.x + TOAST_PADDING,
            vt.bounds.y + TOAST_PADDING,
            title_avail_w,
            line_height,
        );
        surface.surface_draw_text_run(title_rect, &title_line, theme.foreground);

        if !toast.body.is_empty() {
            // Offset by the title's own measured pixel height (matching
            // gtk/macos's pre-#861 `pixel_size()`/`measure_text` calls),
            // not the nominal `line_height` — see this module's doc,
            // divergence 3. Only `win::toast` used `line_height` here
            // pre-migration.
            let (_, title_h) = surface.surface_measure_text(&toast.title);
            let body_avail_w = (vt.bounds.width - TOAST_PADDING * 2.0).max(0.0);
            let body_lines = wrap_text_lines(&toast.body, body_avail_w, MAX_BODY_LINES, &|s| {
                surface.surface_measure_text(s).0
            });
            for (i, line) in body_lines.iter().enumerate() {
                let body_rect = Rect::new(
                    title_rect.x,
                    title_rect.y + title_h + i as f32 * line_height,
                    body_avail_w,
                    line_height,
                );
                surface.surface_draw_text_run(body_rect, line, theme.foreground);
            }
        }

        if let Some(db) = vt.dismiss_bounds {
            let (tw, _) = surface.surface_measure_text("×");
            let rect = Rect::new(db.x + (db.width - tw) / 2.0, db.y, db.width, db.height);
            surface.surface_draw_text_run(rect, "×", theme.foreground);
            if matches!(focus, Some(f) if f.target == ToastFocusTarget::Dismiss) {
                surface.surface_stroke_rect(db, theme.link_fg, 2.0);
            }
        }

        for (i, (ab, action)) in vt.action_rects.iter().zip(toast.actions.iter()).enumerate() {
            if action.primary {
                surface.surface_fill_rect(*ab, theme.accent_bg);
            }
            let (tw, _) = surface.surface_measure_text(&action.label);
            let fg = if action.primary {
                theme.foreground
            } else {
                theme.link_fg
            };
            let rect = Rect::new(ab.x + (ab.width - tw) / 2.0, ab.y, ab.width, ab.height);
            surface.surface_draw_text_run(rect, &action.label, fg);
            if matches!(focus, Some(f) if f.target == ToastFocusTarget::Action(i)) {
                surface.surface_stroke_rect(*ab, theme.link_fg, 2.0);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::toast::{ToastButton, ToastCorner, ToastFocusTarget};
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
            /// #1185: records `surface_stroke_rect` calls (previously a
            /// no-op recorder — nothing asserted on strokes before the
            /// focus-ring requirement existed) so focus-ring tests can
            /// confirm one was drawn, and where.
            strokes: Vec<(Rect, Color, f32)>,
            /// Overrides `surface_measure_text`'s returned height when
            /// set — lets a test make the "measured text height" and
            /// "nominal `line_height` passed into `paint`" conventions
            /// disagree, so it can tell which one `paint_toast` actually
            /// uses (regression for this module's divergence 3). `None`
            /// falls back to the fixed `16.0` every other test relies on.
            measured_text_height: Option<f32>,
        }

        impl PaintSurface for RecordingSurface {
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
                (
                    text.chars().count() as f32 * 8.0,
                    self.measured_text_height.unwrap_or(16.0),
                )
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            /// `paint_toast` now paints its box through this verb
            /// (radius dropped — same `fills` list `surface_fill_rect`
            /// above records into, so every existing `fills[..]`
            /// assertion still sees the toast's background fill here).
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            /// `paint_toast`'s border, radius dropped — same
            /// `strokes` list `surface_stroke_rect` above records into.
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                _radius: f32,
                color: Color,
                stroke_width: f32,
            ) {
                self.strokes.push((rect, color, stroke_width));
            }
            /// Deliberately does **not** forward to
            /// `PaintSurface`'s default (which would recurse into
            /// `surface_fill_rounded_rect` three times and pollute
            /// `fills` with shadow layers ahead of the real background
            /// fill every `fills[0]`-indexing test here relies on) — a
            /// no-op recorder, same posture as `surface_draw_line` below.
            fn surface_draw_shadow(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _elevation: u8,
                _color: Color,
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

        fn toast(id: &str, title: &str) -> Toast {
            Toast {
                id: WidgetId::new(id),
                title: title.into(),
                body: String::new(),
                severity: ToastSeverity::Info,
                actions: Vec::new(),
                accent: None,
            }
        }

        fn stack_br(toasts: Vec<Toast>) -> ToastOverlay {
            ToastOverlay {
                id: WidgetId::new("toasts"),
                corner: ToastCorner::BottomRight,
                toasts,
                focus: None,
            }
        }

        #[test]
        fn empty_stack_paints_nothing() {
            let stack = stack_br(vec![]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
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
            paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
            assert_eq!(surface.fills[0].1, accent);
        }

        #[test]
        fn severity_tint_used_when_no_accent() {
            let mut t = toast("t1", "Hello");
            t.severity = ToastSeverity::Error;
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
            assert_eq!(surface.fills[0].1, theme.error_fg);
        }

        /// Regression for this module's divergence 2: the dismiss glyph
        /// and action label must be painted centred within their own
        /// reserved sub-region, not flush against its left edge — the
        /// shape `win::toast` alone lacked pre-#861.
        #[test]
        fn dismiss_and_action_are_centred_in_their_sub_region() {
            let mut t = toast("t1", "Build failed");
            t.actions = vec![ToastButton {
                id: WidgetId::new("open_log"),
                label: "Open log".into(),
                primary: false,
            }];
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
            let vt = &layout.visible_toasts[0];
            let db = vt.dismiss_bounds.expect("dismiss bounds present");
            let ab = vt.action_rects[0];

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

        /// #1185 acceptance: two actions sit on their own row at the
        /// bottom-right, right-to-left ordering (`actions[0]` ends up
        /// leftmost), strictly below the dismiss `×` — never inline
        /// with the title — and don't overlap each other.
        #[test]
        fn multiple_actions_are_right_aligned_on_their_own_row() {
            let mut t = toast("t1", "Install Markdown Language Server?");
            t.actions = vec![
                ToastButton {
                    id: WidgetId::new("install"),
                    label: "Install".into(),
                    primary: true,
                },
                ToastButton {
                    id: WidgetId::new("dont-ask"),
                    label: "Don't ask again".into(),
                    primary: false,
                },
            ];
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
            let vt = &layout.visible_toasts[0];
            let db = vt.dismiss_bounds.expect("dismiss bounds present");
            assert_eq!(vt.action_rects.len(), 2);
            let install = vt.action_rects[0];
            let dont_ask = vt.action_rects[1];

            // Same row, both below the dismiss button.
            assert_eq!(install.y, dont_ask.y);
            assert!(install.y > db.y + db.height - 0.01);

            // `install` (first action) sits left of `dont_ask`, no overlap.
            assert!(install.x + install.width <= dont_ask.x + 0.01);

            // Row is right-aligned: the rightmost button's edge is at the
            // toast's own trailing-edge padding.
            assert!(
                (dont_ask.x + dont_ask.width - (vt.bounds.x + vt.bounds.width - 8.0)).abs() < 0.01
            );
        }

        /// The action marked `primary` gets a `theme.accent_bg` fill
        /// (VS Code's "do the thing" button); a non-primary action gets
        /// no fill of its own.
        #[test]
        fn primary_action_gets_accent_fill() {
            let mut t = toast("t1", "Install?");
            t.actions = vec![
                ToastButton {
                    id: WidgetId::new("install"),
                    label: "Install".into(),
                    primary: true,
                },
                ToastButton {
                    id: WidgetId::new("skip"),
                    label: "Skip".into(),
                    primary: false,
                },
            ];
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
            let vt = &layout.visible_toasts[0];
            let install_bounds = vt.action_rects[0];
            let skip_bounds = vt.action_rects[1];

            assert!(surface
                .fills
                .iter()
                .any(|(r, c)| *r == install_bounds && *c == theme.accent_bg));
            assert!(!surface.fills.iter().any(|(r, _)| *r == skip_bounds));
        }

        /// #1185: a focused dismiss button gets a `theme.link_fg` stroke
        /// around its own bounds.
        #[test]
        fn focused_dismiss_gets_a_focus_ring() {
            let t = toast("t1", "Saved");
            let mut stack = stack_br(vec![t]);
            stack.focus = Some(ToastFocus {
                toast_id: WidgetId::new("t1"),
                target: ToastFocusTarget::Dismiss,
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
            let db = layout.visible_toasts[0]
                .dismiss_bounds
                .expect("dismiss bounds present");
            assert!(surface
                .strokes
                .iter()
                .any(|(r, c, w)| *r == db && *c == theme.link_fg && *w > 1.0));
        }

        /// #1185: a focused action button gets a `theme.link_fg` stroke
        /// around its own bounds — a different toast/action combination
        /// than the dismiss ring, confirming the focus target is looked
        /// up per-toast (`ToastFocus::toast_id`), not just "the first
        /// toast".
        #[test]
        fn focused_action_gets_a_focus_ring() {
            let t1 = toast("t1", "Saved");
            let mut t2 = toast("t2", "Install?");
            t2.actions = vec![ToastButton {
                id: WidgetId::new("install"),
                label: "Install".into(),
                primary: true,
            }];
            let mut stack = stack_br(vec![t1, t2]);
            stack.focus = Some(ToastFocus {
                toast_id: WidgetId::new("t2"),
                target: ToastFocusTarget::Action(0),
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
            let t2_visible = layout
                .visible_toasts
                .iter()
                .find(|vt| vt.id == WidgetId::new("t2"))
                .expect("t2 visible");
            let ab = t2_visible.action_rects[0];
            assert!(surface
                .strokes
                .iter()
                .any(|(r, c, w)| *r == ab && *c == theme.link_fg && *w > 1.0));

            // The unfocused toast's own dismiss must not get a ring.
            let t1_visible = layout
                .visible_toasts
                .iter()
                .find(|vt| vt.id == WidgetId::new("t1"))
                .expect("t1 visible");
            let t1_db = t1_visible.dismiss_bounds.expect("dismiss bounds present");
            assert!(!surface
                .strokes
                .iter()
                .any(|(r, c, w)| *r == t1_db && *c == theme.link_fg && *w > 1.0));
        }

        #[test]
        fn body_line_painted_below_title() {
            let mut t = toast("t1", "Title");
            t.body = "Body text".into();
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
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

        /// Regression for this module's divergence 3: the body line must
        /// be offset by the title's own *measured* pixel height, not the
        /// nominal `line_height` passed into `paint` — matching gtk/
        /// macos's pre-#861 `pixel_size()`/`measure_text` calls, not
        /// win's pre-#861 `line_height` arithmetic. Sets the surface's
        /// measured title height (20.0) to disagree with `line_height`
        /// (16.0) so the two conventions can't coincidentally agree.
        #[test]
        fn body_offset_uses_measured_title_height_not_nominal_line_height() {
            let mut t = toast("t1", "Title");
            t.body = "Body text".into();
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface {
                measured_text_height: Some(20.0),
                ..Default::default()
            };
            paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );
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
            assert_eq!(body_run.0.y, title_run.0.y + 20.0);
        }

        /// Acceptance regression for #1182: a toast with a long title, an
        /// action button and a long body must not overlap the title with
        /// the action, and every painted text run must stay inside the
        /// toast's own bounds (no run extending past the box, let alone
        /// the viewport). The pre-fix behaviour drew the full,
        /// untruncated title straight through the action column and the
        /// full body as one unwrapped line past the box's right edge.
        #[test]
        fn long_title_and_body_stay_inside_bounds_and_dont_overlap_action() {
            let mut t = toast("t1", "Install Markdown Language Server?");
            t.actions = vec![ToastButton {
                id: WidgetId::new("install"),
                label: "Install".into(),
                primary: true,
            }];
            t.body =
                "N: don't ask again · :ExtInstall markdown-language-server for full details".into();
            let stack = stack_br(vec![t]);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &stack,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
                400.0,
                300.0,
                16.0,
            );

            let vt = &layout.visible_toasts[0];
            let ab = vt.action_rects[0];
            let db = vt.dismiss_bounds.expect("dismiss bounds present");

            // Every text run must stay within the toast's own bounds,
            // both horizontally (x + measured width) and vertically
            // (y + line height, allowing for wrapped body lines).
            for (rect, text, _) in &surface.text_runs {
                if text == "×" {
                    continue;
                }
                let (w, _) = surface.surface_measure_text(text);
                assert!(
                    rect.x >= vt.bounds.x - 0.01,
                    "{text:?} starts left of the toast bounds"
                );
                assert!(
                    rect.x + w <= vt.bounds.x + vt.bounds.width + 0.01,
                    "{text:?} (rect {rect:?}, measured width {w}) overflows the toast's right edge"
                );
                assert!(
                    rect.y + 16.0 <= vt.bounds.y + vt.bounds.height + 0.01,
                    "{text:?} overflows the toast's bottom edge"
                );
            }

            // #1185: the title run must end at or before the dismiss
            // button's left edge (never drawn under `×`) — the action
            // row moved off the title line entirely, so it no longer
            // constrains the title at all, unlike pre-#1185.
            let title_run = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text.starts_with("Install Markdown"))
                .expect("truncated title painted");
            let (title_w, _) = surface.surface_measure_text(&title_run.1);
            assert!(
                title_run.0.x + title_w <= db.x + 0.01,
                "title run {:?} overlaps the dismiss button at {:?}",
                title_run,
                db
            );

            // The action row sits on its own line, strictly below both
            // the title row and the dismiss button — never inline with
            // either.
            assert!(
                ab.y > db.y + db.height - 0.01,
                "action row overlaps dismiss's row"
            );

            // The long body needed more than one line, so the box grew
            // taller than the single-body-line default, and the button
            // row adds further height on top of that.
            let single_line_height = 16.0 + 8.0 * 2.0 + 16.0; // line_height + 2*padding + 1 body line
            assert!(
                vt.bounds.height > single_line_height,
                "box should have grown to fit the wrapped body: height={}",
                vt.bounds.height
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Toast primitive tests (D6 shape, new B.3 primitive) ───────────

    fn make_toast(id: &str, title: &str) -> Toast {
        Toast {
            id: WidgetId::new(id),
            title: title.to_string(),
            body: String::new(),
            severity: ToastSeverity::Info,
            actions: Vec::new(),
            accent: None,
        }
    }

    fn make_toast_stack(corner: ToastCorner, toasts: Vec<Toast>) -> ToastOverlay {
        ToastOverlay {
            id: WidgetId::new("toasts"),
            corner,
            toasts,
            focus: None,
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
        toast.actions = vec![ToastButton {
            id: WidgetId::new("open_log"),
            label: "Open log".to_string(),
            primary: false,
        }];
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![toast]);
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| ToastMeasure {
            width: 300.0,
            height: 64.0,
            dismiss_rect: Some(Rect::new(300.0 - 24.0, 0.0, 24.0, 24.0)),
            action_rects: vec![Rect::new(300.0 - 80.0, 64.0 - 20.0, 80.0, 20.0)],
        });
        let v = &layout.visible_toasts[0];
        assert!(v.dismiss_bounds.is_some());
        assert_eq!(v.action_rects.len(), 1);
        let db = v.dismiss_bounds.unwrap();
        let ab = v.action_rects[0];
        // Dismiss at the trailing (top) edge.
        assert_eq!(db.x + db.width, v.bounds.x + v.bounds.width);
        // Action row sits on its own, lower row — below the dismiss row,
        // not inline with it (#1185).
        assert!(ab.y >= db.y + db.height);

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

    /// #1185: a toast with two actions produces two, non-overlapping
    /// `action_rects` entries, aligned 1:1 with `Toast::actions`.
    #[test]
    fn toast_layout_multiple_action_regions() {
        let mut toast = make_toast("t1", "Install Markdown Language Server?");
        toast.actions = vec![
            ToastButton {
                id: WidgetId::new("install"),
                label: "Install".to_string(),
                primary: true,
            },
            ToastButton {
                id: WidgetId::new("dont-ask"),
                label: "Don't ask again".to_string(),
                primary: false,
            },
        ];
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![toast]);
        let layout = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| ToastMeasure {
            width: 300.0,
            height: 80.0,
            dismiss_rect: Some(Rect::new(276.0, 0.0, 24.0, 24.0)),
            action_rects: vec![
                Rect::new(120.0, 60.0, 70.0, 20.0),
                Rect::new(198.0, 60.0, 90.0, 20.0),
            ],
        });
        let v = &layout.visible_toasts[0];
        assert_eq!(v.action_rects.len(), 2);
        let install = v.action_rects[0];
        let dont_ask = v.action_rects[1];
        // No overlap between the two buttons.
        assert!(install.x + install.width <= dont_ask.x);

        match layout.hit_test(install.x + 1.0, install.y + 1.0) {
            ToastHit::Action(id) => assert_eq!(id.as_str(), "install"),
            other => panic!("expected Action(install), got {other:?}"),
        }
        match layout.hit_test(dont_ask.x + 1.0, dont_ask.y + 1.0) {
            ToastHit::Action(id) => assert_eq!(id.as_str(), "dont-ask"),
            other => panic!("expected Action(dont-ask), got {other:?}"),
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
    /// `ToastOverlay::layout` previously had no origin parameter at all,
    /// so it could only ever be called with an implicit `(0, 0)`
    /// origin — a shape no `*_toast_stack_layout` backend wrapper could
    /// correct for. Confirms every returned bound (toast, dismiss,
    /// action, hit region) shifts rigidly by `(origin_x, origin_y)`
    /// relative to the origin-`(0, 0)` layout for the same stack.
    #[test]
    fn toast_layout_nonzero_origin_shifts_every_bound() {
        let mut toast = make_toast("t1", "Build failed");
        toast.actions = vec![ToastButton {
            id: WidgetId::new("open_log"),
            label: "Open log".to_string(),
            primary: false,
        }];
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![toast]);
        let measure = |_: usize| ToastMeasure {
            width: 300.0,
            height: 64.0,
            dismiss_rect: Some(Rect::new(276.0, 0.0, 24.0, 24.0)),
            action_rects: vec![Rect::new(220.0, 44.0, 80.0, 20.0)],
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
        assert_eq!(s.action_rects[0].y, o.action_rects[0].y + 13.0);

        // Round trip: an absolute hit against the shifted layout must
        // resolve the same way the origin layout resolves its local hit.
        let db = s.dismiss_bounds.unwrap();
        assert_eq!(
            shifted.hit_test(db.x + 5.0, db.y + 10.0),
            ToastHit::Dismiss(WidgetId::new("t1")),
        );
    }

    // ── ToastMotion / layout_with_motion ──────────────────────────────

    #[test]
    fn tui_snaps_to_the_final_layout_with_no_motion_calls() {
        let stack = make_toast_stack(
            ToastCorner::BottomRight,
            vec![make_toast("a", "A"), make_toast("b", "B")],
        );
        let motion = ToastMotion::new();
        let now = Instant::now();
        let plain = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| {
            ToastMeasure::new(300.0, 64.0)
        });
        let animated =
            stack.layout_with_motion(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, &motion, now, |_| {
                ToastMeasure::new(300.0, 64.0)
            });
        assert_eq!(plain, animated);
        assert!(!motion.is_animating(now));
    }

    #[test]
    fn show_at_slides_a_bottom_corner_toast_up_from_below() {
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![make_toast("a", "A")]);
        let mut motion = ToastMotion::new();
        let t0 = Instant::now();
        motion.show_at(WidgetId::new("a"), t0);

        let settled = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| {
            ToastMeasure::new(300.0, 64.0)
        });
        let at_start =
            stack.layout_with_motion(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, &motion, t0, |_| {
                ToastMeasure::new(300.0, 64.0)
            });
        let settled_y = settled.visible_toasts[0].bounds.y;
        let start_y = at_start.visible_toasts[0].bounds.y;
        assert!(
            start_y > settled_y,
            "a bottom-corner toast should start *below* its resting position \
             (larger y), got start_y={start_y} settled_y={settled_y}"
        );
        assert_eq!(start_y - settled_y, settled.visible_toasts[0].bounds.height);

        let at_end = stack.layout_with_motion(
            0.0,
            0.0,
            800.0,
            600.0,
            16.0,
            8.0,
            &motion,
            t0 + CHROME_TRANSITION_DURATION,
            |_| ToastMeasure::new(300.0, 64.0),
        );
        assert_eq!(at_end, settled, "fully shown should match the plain layout");
    }

    #[test]
    fn show_at_slides_a_top_corner_toast_down_from_above() {
        let stack = make_toast_stack(ToastCorner::TopLeft, vec![make_toast("a", "A")]);
        let mut motion = ToastMotion::new();
        let t0 = Instant::now();
        motion.show_at(WidgetId::new("a"), t0);

        let settled = stack.layout(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, |_| {
            ToastMeasure::new(300.0, 64.0)
        });
        let at_start =
            stack.layout_with_motion(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, &motion, t0, |_| {
                ToastMeasure::new(300.0, 64.0)
            });
        let settled_y = settled.visible_toasts[0].bounds.y;
        let start_y = at_start.visible_toasts[0].bounds.y;
        assert!(
            start_y < settled_y,
            "a top-corner toast should start *above* its resting position \
             (smaller y), got start_y={start_y} settled_y={settled_y}"
        );
    }

    #[test]
    fn dismiss_at_slides_back_out_and_hit_regions_follow_the_shifted_bounds() {
        let stack = make_toast_stack(ToastCorner::BottomRight, vec![make_toast("a", "A")]);
        let mut motion = ToastMotion::new();
        let t0 = Instant::now();
        // Already shown, then start dismissing.
        motion.show_at(WidgetId::new("a"), t0);
        let fully_in = t0 + CHROME_TRANSITION_DURATION;
        motion.dismiss_at(WidgetId::new("a"), fully_in);
        assert!(motion.is_animating(fully_in));

        let mid = fully_in + CHROME_TRANSITION_DURATION / 2;
        let layout =
            stack.layout_with_motion(0.0, 0.0, 800.0, 600.0, 16.0, 8.0, &motion, mid, |_| {
                ToastMeasure::new(300.0, 64.0)
            });
        let vt = &layout.visible_toasts[0];
        // Hit-testing the body at its *current* (shifted) bounds must
        // resolve, proving `hit_regions` tracked the slide rather than
        // staying pinned to the settled position.
        let cx = vt.bounds.x + vt.bounds.width / 2.0;
        let cy = vt.bounds.y + vt.bounds.height / 2.0;
        assert_eq!(layout.hit_test(cx, cy), ToastHit::Body(WidgetId::new("a")));

        let fully_out = fully_in + CHROME_TRANSITION_DURATION;
        assert!(!motion.is_animating(fully_out));
        assert_eq!(motion.progress(&WidgetId::new("a"), fully_out), 0.0);
    }

    #[test]
    fn prune_drops_settled_transitions_but_keeps_in_flight_ones() {
        let mut motion = ToastMotion::new();
        let t0 = Instant::now();
        motion.show_at(WidgetId::new("settled"), t0);

        let settled_at = t0 + CHROME_TRANSITION_DURATION;
        assert!(!motion.is_animating(settled_at));
        motion.prune(settled_at);
        assert!(
            motion.is_empty(),
            "a fully-settled transition should be pruned away"
        );

        motion.show_at(WidgetId::new("mid-flight"), settled_at);
        let mid = settled_at + CHROME_TRANSITION_DURATION / 2;
        motion.prune(mid);
        assert!(
            !motion.is_empty(),
            "a still-animating transition must survive pruning"
        );
        assert!(motion.is_animating(mid));
    }
}
