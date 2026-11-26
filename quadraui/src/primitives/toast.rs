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
//! `corner`, VS Code-style: title/body wrap across the box's
//! full width, growing its height up to a cap ([`MAX_BODY_LINES`])
//! before ellipsizing; the dismiss `×` sits alone near the top-right;
//! any [`Toast::actions`] sit on their own row at the bottom-right,
//! never inline with the title. Clicks resolve via
//! [`ToastStackLayout::hit_test`] / [`ToastHit`]: an action button hits
//! `ToastHit::Action`; the dismiss affordance hits `ToastHit::Dismiss`.
//!
//! Toast boxes take keyboard focus only when an app explicitly hands it
//! to them, via [`crate::compose::ToastStackController`] —
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
//! holding [`ToastButton`]s. Single-action counterparts were deprecated
//! and removed once known consumers migrated; see `CHANGELOG.md`'s
//! `Removed` entry.

use crate::event::{Key, NamedKey, Rect};
use crate::types::{Color, Modifiers, WidgetId};
use crate::Handled;
use serde::{Deserialize, Serialize};

/// Declarative description of a toast stack for one corner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToastOverlay {
    pub id: WidgetId,
    /// Which corner of the viewport the stack occupies.
    pub corner: ToastCorner,
    /// Toasts in temporal order — oldest first. Visual order depends on
    /// `corner` (bottom corners stack upward, top corners stack downward).
    pub toasts: Vec<Toast>,
    /// Which control (if any) currently has keyboard focus.
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Toast {
    pub id: WidgetId,
    pub title: String,
    /// Body text. Can be empty for minimal "File saved" style toasts.
    #[serde(default)]
    pub body: String,
    /// Visual severity — backends tint the box accordingly.
    #[serde(default)]
    pub severity: ToastSeverity,
    /// Ordered action buttons. Rendered on their own row at the bottom-right
    /// of the toast box, never inline with the title. Empty = no action row;
    /// just the dismiss affordance is clickable. At most one entry should set
    /// [`ToastButton::primary`] — see that field's doc.
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
///
/// Not `Eq`: [`Self::spec`] carries a [`crate::ButtonSpec`], which has an
/// `f32` (`icon_gap`) with no total order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToastButton {
    pub id: WidgetId,
    pub label: String,
    /// Styled with the theme's accent (filled background) instead of the
    /// plain secondary-button look. VS Code convention: at most
    /// one action per toast is primary — the "do the thing" button (e.g.
    /// "Install"), with the rest ("Don't ask again", …) staying
    /// secondary. Backends don't enforce the "at most one" rule; a
    /// caller that sets it on more than one action just gets more than
    /// one accent-filled button.
    #[serde(default)]
    pub primary: bool,
    /// Optional [`crate::ButtonSpec`] override, carrying the B3a
    /// vocabulary (variant intent, icon, a tooltip) this button's own
    /// legacy `primary` field can't express. `None` (the default) renders
    /// the plain filled-chip/plain look; see [`Self::resolved_button_spec`].
    #[serde(default)]
    pub spec: Option<crate::primitives::button::ButtonSpec>,
}

impl ToastButton {
    /// This button's explicit [`Self::spec`] override when set, otherwise
    /// one derived from its legacy `primary` field (primary → `Primary`
    /// variant, otherwise `Secondary`).
    pub fn resolved_button_spec(&self) -> crate::primitives::button::ButtonSpec {
        if let Some(spec) = &self.spec {
            return spec.clone();
        }
        let mut derived = crate::primitives::button::ButtonSpec::new(self.label.clone());
        derived.variant = if self.primary {
            crate::primitives::button::ButtonVariant::Primary
        } else {
            crate::primitives::button::ButtonVariant::Secondary
        };
        derived
    }
}

/// Keyboard-focus target within a [`ToastOverlay`] — set
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

// ── Text wrapping ───────────────────────────────────────────────────────────
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
                // forever trying (and failing) to fit it whole. Delegates
                // to the shared ellipsis-fitting search so a budget too
                // narrow even for "x…" correctly yields a bare "…" (or
                // nothing) instead of overflowing by one character.
                let (piece, _) =
                    crate::text_util::truncate_with_ellipsis(word, max_width, width_of);
                line = piece;
                idx += 1;
            }
            break;
        }
        lines.push(line);
    }

    // Words remain past `max_lines`: ellipsize the last line so the cut
    // is visible rather than silently dropped. If even a bare "…" doesn't
    // fit `max_width` — unreachable for the cell-unit TUI measurer, but
    // reachable for a pixel measurer with a very small budget — there is
    // no in-budget way to show the cut, so the line is left empty rather
    // than pushing a guaranteed-over-budget "…", matching
    // `truncate_with_ellipsis`'s own contract for the same degenerate case.
    if idx < words.len() {
        if let Some(last) = lines.last_mut() {
            if width_of("…") <= max_width {
                while !last.is_empty() && width_of(&format!("{last}…")) > max_width {
                    last.pop();
                }
                last.push('…');
            } else {
                last.clear();
            }
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
    fn single_word_truncated_at_width_one_never_overflows_budget() {
        // At max_width=1 even "x…" (2 cells) doesn't fit — the shared
        // ellipsis-fitting search must fall back to a bare "…", never an
        // over-budget "x…".
        let lines = wrap_text_lines("abc", 1.0, 1, &chars);
        assert_eq!(lines, vec!["…".to_string()]);
    }

    #[test]
    fn empty_text_yields_no_lines() {
        assert!(wrap_text_lines("", 20.0, 3, &chars).is_empty());
    }

    /// A pixel-unit measurer (unlike the cell-unit TUI one, where the
    /// ellipsis always costs exactly 1 cell) can make even a bare "…"
    /// wider than `max_width`. The cap-past-`max_lines` path must not
    /// respond by pushing a guaranteed-over-budget "…" — it leaves the
    /// line empty, the same degenerate answer `truncate_with_ellipsis`
    /// gives when nothing, not even the ellipsis, fits.
    #[test]
    fn sub_ellipsis_width_budget_never_overflows() {
        // Each char (including '…') costs 3 width units; a budget of 2
        // can't afford even one.
        let three_per_char = |s: &str| 3.0 * s.chars().count() as f32;
        let lines = wrap_text_lines("ab cd", 2.0, 1, &three_per_char);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0], "",
            "no content fits the budget, including the ellipsis itself"
        );
        assert!(three_per_char(&lines[0]) <= 2.0);
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

/// Compute a toast's dismiss + action-button rects, toast-local (relative to
/// the toast box's own top-left) — shared by every backend's measure closure,
/// so the `×` affordance's top-right position and the action row's
/// right-aligned, bottom-edge placement are computed identically everywhere,
/// whether the caller's unit is pixels or terminal cells.
///
/// VS Code shape: `×` sits alone near the top-right, clear of the title
/// row below it in height so it never overlaps a wrapped multi-line
/// title/body; actions sit on their own row at the bottom-right,
/// right-to-left (`action_widths[0]` ends up leftmost in the row),
/// never inline with the title. `action_widths` is each action's
/// already-measured, already-padded button width, in [`Toast::actions`]
/// order; the returned `Vec` is aligned 1:1 with it. Both rects are
/// clamped to stay non-negative even if the box is smaller than the sum
/// of its own padding/button widths (a degenerate but non-panicking box,
/// matching this module's existing `.max(0.0)`-everywhere convention).
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
    /// Action-button bounds, one per [`Toast::actions`] entry, same order.
    /// Empty if the toast has no actions.
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

impl ToastOverlay {
    /// Compute the rendering + hit-test layout for the stack.
    ///
    /// # Arguments
    ///
    /// - `origin_x`, `origin_y` — the top-left corner of the app's
    ///   overlay area, in the backend's absolute (screen/buffer) frame.
    ///   Returned `bounds` / `hit_regions` are absolute — callers pass
    ///   `hit_test` raw click coordinates in that same frame, matching
    ///   the `MenuBar`/`SidebarPanel` convention (unlike `TreeView`'s
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
            let dismiss_bounds = m.dismiss_rect.map(|r| shift_rect(r, bounds.x, bounds.y));
            let action_rects: Vec<Rect> = m
                .action_rects
                .iter()
                .map(|r| shift_rect(*r, bounds.x, bounds.y))
                .collect();

            let toast_id = self.toasts[i].id;
            visible_toasts.push(VisibleToast {
                toast_idx: i,
                id: toast_id,
                bounds,
                dismiss_bounds,
                action_rects: action_rects.clone(),
            });

            // Register hit regions in specificity order: dismiss, actions, body.
            if let Some(db) = dismiss_bounds {
                hit_regions.push((db, ToastHit::Dismiss(toast_id)));
            }
            // Action hit regions carry the action's own id (not the
            // toast's) so the app can dispatch the intended action
            // directly from the hit result. Zipped by position with
            // `Toast::actions`, matching `action_rects`'s documented
            // 1:1 order.
            for (ab, act) in action_rects.iter().zip(self.toasts[i].actions.iter()) {
                hit_regions.push((*ab, ToastHit::Action(act.id)));
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

        // Shift from the viewport-local frame into the caller's absolute
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

// ── Cell-grid measurement ───────────────────────────────────────────────────

/// Max toast width on a cell grid: a fixed `40` was too narrow for a
/// title with an action button (e.g. "Install Markdown Language
/// Server?" + "Install"), which got cut off mid-word. The width is a
/// per-content floor/ceiling — see [`cell_toast_width`] — with this as
/// the ceiling.
const CELL_TOAST_MAX_WIDTH: f32 = 60.0;
/// Width floor: keeps a short "Saved"-style toast from shrinking below a
/// sensible box, and leaves room for the 2-cell border + dismiss glyph.
const CELL_TOAST_MIN_WIDTH: f32 = 12.0;
const CELL_TOAST_MARGIN: f32 = 1.0;
const CELL_TOAST_GAP: f32 = 1.0;
const CELL_DISMISS_WIDTH: f32 = 3.0;
const CELL_ACTION_PADDING: f32 = 2.0;
/// Horizontal gap (cells) between two adjacent action buttons on the
/// button row.
const CELL_ACTION_GAP: f32 = 1.0;
/// Rows reserved for the top+bottom border.
const CELL_BORDER_ROWS: f32 = 2.0;
/// Columns reserved for the left+right border. Drawn in the 1-cell
/// padding columns every toast already reserved before text/dismiss, so
/// this doesn't change the box's overall width formula.
const CELL_BORDER_COLS: f32 = 2.0;
/// Toast-local inset from the box edge for the dismiss/action rows —
/// keeps both clear of the 1-cell border.
const CELL_INSET: f32 = 1.0;

/// Per-action button width in cells: label length plus
/// [`CELL_ACTION_PADDING`] breathing room either side.
fn cell_action_width(action: &ToastButton) -> f32 {
    action.label.chars().count() as f32 + CELL_ACTION_PADDING
}

/// Resolve a toast's box width in cells from its content: wide enough for
/// the whole title plus its dismiss button, *and* wide enough for the
/// action row (if any) to fit without its buttons overlapping — clamped
/// to a sensible [`CELL_TOAST_MAX_WIDTH`] ceiling and
/// [`CELL_TOAST_MIN_WIDTH`] floor, and never wider than the viewport
/// allows. The body doesn't drive width — it wraps instead (see
/// [`cell_body_lines`]) — so a very long body alone doesn't blow the box
/// out sideways.
fn cell_toast_width(toast: &Toast, viewport_width: f32) -> f32 {
    let title_needed = toast.title.chars().count() as f32 + CELL_BORDER_COLS + CELL_DISMISS_WIDTH;
    let action_widths: Vec<f32> = toast.actions.iter().map(cell_action_width).collect();
    let actions_needed = if action_widths.is_empty() {
        0.0
    } else {
        CELL_BORDER_COLS
            + action_widths.iter().sum::<f32>()
            + CELL_ACTION_GAP * (action_widths.len() as f32 - 1.0)
    };
    title_needed
        .max(actions_needed)
        .clamp(CELL_TOAST_MIN_WIDTH, CELL_TOAST_MAX_WIDTH)
        .min((viewport_width - CELL_TOAST_MARGIN * 2.0).max(0.0))
}

/// Word-wrap `toast`'s body to fit a box of `width` columns (already
/// resolved by [`cell_toast_width`]), capped at [`MAX_BODY_LINES`] —
/// shared by the measure closure (for height) and the painter (for the
/// actual text), so they always agree.
pub(crate) fn cell_body_lines(toast: &Toast, width: f32) -> Vec<String> {
    if toast.body.is_empty() {
        return Vec::new();
    }
    let avail = (width - CELL_BORDER_COLS).max(0.0);
    wrap_text_lines(&toast.body, avail, MAX_BODY_LINES, &|s| {
        s.chars().count() as f32
    })
}

/// A toast's box height in cells: title row + wrapped body rows + one
/// button row (only when the toast has actions) + the two border rows.
fn cell_toast_height(toast: &Toast, width: f32) -> f32 {
    let body_lines = cell_body_lines(toast, width).len();
    let button_row = if toast.actions.is_empty() { 0.0 } else { 1.0 };
    1.0 + body_lines as f32 + button_row + CELL_BORDER_ROWS
}

/// Compute a [`ToastOverlay`]'s cell-unit layout — the cell-grid twin of
/// [`crate::primitives::layout_metrics::pixel_toast_stack_layout`].
///
/// `area`'s origin is baked into the returned bounds (absolute buffer
/// coordinates, matching `MenuBar::cell_layout`)
/// — hosts call `layout.hit_test(x, y)` with raw click coordinates, no
/// localisation needed.
pub(crate) fn cell_toast_stack_layout(stack: &ToastOverlay, area: Rect) -> ToastStackLayout {
    let viewport_width = area.width;
    stack.layout(
        area.x,
        area.y,
        viewport_width,
        area.height,
        CELL_TOAST_MARGIN,
        CELL_TOAST_GAP,
        |i| {
            let toast = &stack.toasts[i];
            let width = cell_toast_width(toast, viewport_width);
            let height = cell_toast_height(toast, width);
            let action_widths: Vec<f32> = toast.actions.iter().map(cell_action_width).collect();
            let (dismiss_rect, action_rects) = toast_button_rects(
                width,
                height,
                CELL_INSET,
                CELL_DISMISS_WIDTH,
                1.0,
                &action_widths,
                1.0,
                CELL_ACTION_GAP,
            );
            ToastMeasure {
                width,
                height,
                dismiss_rect,
                action_rects,
            }
        },
    )
}

// ── Cell-grid painter ───────────────────────────────────────────────────────

/// The cell-grid painter for [`ToastOverlay`], written against
/// [`crate::CellSurface`].
///
/// One call paints the whole stack: each resolved box gets a severity (or
/// [`Toast::accent`]) background wash, a `┌─┐└─┘│` border, a title
/// truncated clear of the dismiss `×`, up to [`MAX_BODY_LINES`] wrapped
/// body rows, the `×` itself near the top-right, and the action row at
/// the bottom-right. [`ToastOverlay::focus`], when it names a control on
/// a box, inverts that control into `theme.link_fg`.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{
        cell_body_lines, severity_bg_cells, truncate_line, Toast, ToastFocus, ToastFocusTarget,
        ToastStackLayout, VisibleToast, CELL_BORDER_COLS,
    };
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Rect;

    /// A foreground/background pair with no modifier bits — the only
    /// [`CellStyle`] shape this painter builds.
    fn ink(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint every visible toast in `layout`, clipped to `clip` (the
    /// overlay area the host handed the stack — a box that would reach
    /// past it is cut off rather than drawn outside).
    pub(crate) fn paint(
        stack: &super::ToastOverlay,
        layout: &ToastStackLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        clip: Rect,
    ) {
        for vt in &layout.visible_toasts {
            let toast = &stack.toasts[vt.toast_idx];
            let focus = stack.focus.as_ref().filter(|f| f.toast_id == toast.id);
            paint_toast(surface, clip, vt, toast, theme, focus);
        }
    }

    fn paint_toast(
        surface: &mut dyn CellSurface,
        clip: Rect,
        vt: &VisibleToast,
        toast: &Toast,
        theme: &Theme,
        focus: Option<&ToastFocus>,
    ) {
        let bg = toast
            .accent
            .unwrap_or_else(|| severity_bg_cells(toast.severity, theme));
        let fg = theme.foreground;
        let border_fg = theme.border_fg;

        let bx = vt.bounds.x.round() as u16;
        let by = vt.bounds.y.round() as u16;
        let bw = vt.bounds.width.round() as u16;
        let bh = vt.bounds.height.round() as u16;

        let clip_x1 = (clip.x + clip.width).round() as u16;
        let clip_y1 = (clip.y + clip.height).round() as u16;
        let in_clip = |x: u16, y: u16| x < clip_x1 && y < clip_y1;

        let body_ink = ink(fg, bg);
        let chrome = ink(border_fg, bg);

        // Fill background.
        for dy in 0..bh {
            for dx in 0..bw {
                let (x, y) = (bx + dx, by + dy);
                if in_clip(x, y) {
                    surface.put_char(x, y, ' ', body_ink);
                }
            }
        }

        // Border drawn with box-drawing characters on top/bottom rows;
        // left/right columns reuse the 1-cell padding every toast already
        // reserves before text/dismiss, so this doesn't change the box's
        // width. Painted cell by cell rather than through
        // `CellSurface::draw_border` so each glyph is dropped
        // individually when the box reaches past `clip`.
        let bottom_y = by + bh.saturating_sub(1);
        if bh >= 2 && bw >= 2 {
            for dx in 0..bw {
                let x = bx + dx;
                let top_ch = if dx == 0 {
                    '┌'
                } else if dx == bw - 1 {
                    '┐'
                } else {
                    '─'
                };
                if in_clip(x, by) {
                    surface.put_char(x, by, top_ch, chrome);
                }
                let bottom_ch = if dx == 0 {
                    '└'
                } else if dx == bw - 1 {
                    '┘'
                } else {
                    '─'
                };
                if in_clip(x, bottom_y) {
                    surface.put_char(x, bottom_y, bottom_ch, chrome);
                }
            }
            for dy in 1..bh.saturating_sub(1) {
                let y = by + dy;
                if in_clip(bx, y) {
                    surface.put_char(bx, y, '│', chrome);
                }
                if in_clip(bx + bw - 1, y) {
                    surface.put_char(bx + bw - 1, y, '│', chrome);
                }
            }
        }

        // Title on the first interior row (below the top border),
        // left-aligned with 1-cell padding, truncated/ellipsized to the
        // space left of the dismiss button — never drawn under it. The
        // action row has its own line at the bottom, so the title only
        // needs to dodge dismiss.
        let title_y = by + 1;
        let text_end = vt
            .dismiss_bounds
            .map(|db| db.x.round() as u16)
            .unwrap_or(bx + bw.saturating_sub(1));
        let dismiss_w = vt.dismiss_bounds.map(|d| d.width).unwrap_or(0.0);
        let title_avail = (vt.bounds.width - CELL_BORDER_COLS - dismiss_w).max(0.0);
        let title_line = truncate_line(&toast.title, title_avail, &|s| s.chars().count() as f32);
        let title_budget = text_end
            .saturating_sub(bx + 1)
            .min(clip_x1.saturating_sub(bx + 1));
        if in_clip(bx + 1, title_y) {
            surface.put_str_clipped(bx + 1, title_y, &title_line, title_budget, body_ink);
        }

        // Body wraps across up to `MAX_BODY_LINES` rows below the title.
        // Body rows have no action/dismiss to avoid, so they use the full
        // interior width.
        if !toast.body.is_empty() {
            let body_text_end = bx + bw.saturating_sub(1);
            let body_budget = body_text_end
                .saturating_sub(bx + 1)
                .min(clip_x1.saturating_sub(bx + 1));
            for (i, line) in cell_body_lines(toast, vt.bounds.width).iter().enumerate() {
                let body_y = title_y + 1 + i as u16;
                if in_clip(bx + 1, body_y) {
                    surface.put_str_clipped(bx + 1, body_y, line, body_budget, body_ink);
                }
            }
        }

        // Dismiss × near the top-right: a single-row square near the
        // title row.
        let dismiss_focused = matches!(focus, Some(f) if f.target == ToastFocusTarget::Dismiss);
        if let Some(db) = vt.dismiss_bounds {
            let dx = db.x.round() as u16 + (db.width as u16).saturating_sub(1) / 2;
            let dy = db.y.round() as u16;
            if in_clip(dx, dy) {
                // Visible focus indicator — invert fg/bg on the dismiss
                // cell when it's the controller's focus target.
                let style = if dismiss_focused {
                    ink(bg, theme.link_fg)
                } else {
                    body_ink
                };
                surface.put_char(dx, dy, '×', style);
            }
        }

        // Action buttons on their own row at the bottom-right.
        for (i, (ab, action)) in vt.action_rects.iter().zip(toast.actions.iter()).enumerate() {
            let ay = ab.y.round() as u16;
            let ax0 = ab.x.round() as u16;
            let aw = ab.width.round() as u16;
            let action_focused =
                matches!(focus, Some(f) if f.target == ToastFocusTarget::Action(i));
            let (mut row_bg, mut row_fg) = if action.primary {
                (theme.accent_bg, theme.foreground)
            } else {
                (bg, theme.link_fg)
            };
            // An explicit `spec` override's `variant` tints the
            // foreground in the intent colour and clears any `accent_bg`
            // fill `primary` would otherwise set — no background fill of
            // its own on a cell grid, the same "no-op background"
            // convention `crate::tui::button` uses for a standalone
            // `Danger`/`Warning` `Button`, regardless of the legacy
            // `primary` flag. A `spec: None` action keeps its plain
            // primary/secondary look, unaffected. Only `variant` is
            // honoured here today — `icon`/`icon_position`/`icon_gap`/
            // `tooltip`/`disabled` on the spec have no cell-grid toast
            // effect yet.
            if let Some(spec) = &action.spec {
                match spec.variant {
                    crate::primitives::button::ButtonVariant::Danger => {
                        row_fg = theme.error_fg;
                        row_bg = bg;
                    }
                    crate::primitives::button::ButtonVariant::Warning => {
                        row_fg = theme.warning_fg;
                        row_bg = bg;
                    }
                    _ => {}
                }
            }
            if action_focused {
                // Visible focus indicator — invert to a `theme.link_fg`
                // filled row, same treatment as dismiss.
                row_fg = row_bg;
                row_bg = theme.link_fg;
            }
            let face = ink(row_fg, row_bg);
            // Fill the button's own row so a primary action reads as a
            // filled "chip".
            for dx in 0..aw {
                let x = ax0 + dx;
                if in_clip(x, ay) {
                    surface.put_char(x, ay, ' ', face);
                }
            }
            let label_start = ax0 + (aw.saturating_sub(action.label.chars().count() as u16)) / 2;
            let budget = (ax0 + aw)
                .saturating_sub(label_start)
                .min(clip_x1.saturating_sub(label_start));
            if in_clip(label_start, ay) {
                surface.put_str_clipped(label_start, ay, &action.label, budget, face);
            }
        }
    }
}

/// Severity → fallback background tint on a cell grid, used when
/// [`Toast::accent`] is `None`.
///
/// `Info` stays on the neutral `theme.surface_bg` panel colour rather
/// than `Theme::info_bg`'s accent tint — see
/// `native_surface_paint::severity_bg`'s own doc for why.
#[cfg(feature = "tui")]
pub(crate) fn severity_bg_cells(severity: ToastSeverity, theme: &crate::theme::Theme) -> Color {
    match severity {
        ToastSeverity::Info => theme.surface_bg,
        ToastSeverity::Success => theme.success_bg(),
        ToastSeverity::Warning => theme.warning_bg(),
        ToastSeverity::Error => theme.danger_bg(),
    }
}

// ── PaintSurface paint ────────────────────────────────────────────────
//
// This is the one shared implementation of toast painting, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// drawing API.
//
// Each backend's own no-paint `*_toast_stack_layout` twin is untouched —
// it's layout-only, consumed by `Backend::toast_stack_layout` for
// hit-testing without a repaint. The margin/gap/dismiss-width/
// action-padding/box-width constants below match every pre-migration
// per-backend constant's value exactly.
//
// `severity_bg` resolves every severity through a dedicated `Theme`
// accessor (`success_bg`/`warning_bg`/`danger_bg`/`info_bg`) rather than a
// mix of hardcoded RGB literals and raw foreground fields, so a themed
// app's success/warning/danger/info colours stay consistent across every
// primitive that needs one of these roles, not just toasts.
//
// # Divergences found — reported, not silently resolved
//
// 1. **Win never took a live theme.** `win::toast::draw_toast_stack` had
//    no `theme: &Theme` parameter — it built `Theme::default()` internally
//    on every call. The shared `paint` requires a `theme: &Theme`
//    parameter (matching gtk/macos), so `WinBackend::draw_toast_stack`
//    now passes `&self.current_theme`.
//
// 2. **Dismiss/action glyph centring.** `gtk::toast::paint_toast` and
//    `macos::toast::paint_toast` centre the dismiss `×` and action-button
//    label horizontally within their reserved sub-region. `win::toast::
//    paint_toast` drew both directly via `DWrite::draw_text`, using
//    DirectWrite's default (leading/near) alignment — flush against the
//    sub-region's left edge. The shared `paint` adopts the 2-of-3
//    (gtk/macos) centred shape uniformly.
//
// 3. **Body-line vertical offset.** `gtk::toast::paint_toast` and
//    `macos::toast::paint_toast` positioned the body line at the title's
//    own *measured* pixel height. `win::toast::paint_toast` used the
//    nominal `line_height` argument instead. The shared `paint` adopts
//    the 2-of-3 (gtk/macos) measured-height shape uniformly.
//
// 4. **Toast-width clamp floor.** `gtk::toast::gtk_toast_stack_layout`
//    and `macos::toast::macos_toast_stack_layout` computed
//    `TOAST_WIDTH.min(viewport_width - TOAST_MARGIN * 2.0)` with no
//    floor. Only `win::toast::win_toast_stack_layout` clamped the
//    subtrahend to `.max(0.0)`. The shared `paint` adopts Win's
//    (safer) clamped form for all three.
//
// `#[allow(dead_code)]`: only called once a real pixel backend is
// compiled in, exercised by each backend's own `Backend::draw_toast_stack`
// call site plus this module's own `RecordingSurface` tests.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{
        shift_rect, truncate_line, wrap_text_lines, Toast, ToastFocus, ToastFocusTarget,
        ToastOverlay, ToastSeverity, ToastStackLayout, VisibleToast, MAX_BODY_LINES,
    };
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::primitives::layout_metrics::pixel::TOAST_PADDING;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};

    /// Severity → fallback background tint, used when `Toast::accent` is
    /// `None`. Duplicated across `tui::toast` (out of scope for the
    /// `PaintSurface` migration) — both read the same `Theme` fields/
    /// accessors, so they stay in sync without a shared function.
    ///
    /// `Info` stays on the neutral `theme.surface_bg` panel colour
    /// rather than `Theme::info_bg`'s accent tint: `Info` is
    /// `ToastSeverity::default()`, so every toast shown without an
    /// explicit severity goes through this arm, and VS Code's own info
    /// notification background is the neutral notification surface, not
    /// an accent-tinted one.
    fn severity_bg(severity: ToastSeverity, theme: &Theme) -> Color {
        match severity {
            ToastSeverity::Info => theme.surface_bg,
            ToastSeverity::Success => theme.success_bg(),
            ToastSeverity::Warning => theme.warning_bg(),
            ToastSeverity::Error => theme.danger_bg(),
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
        paint_with_x_offset(
            stack,
            surface,
            theme,
            style,
            origin_x,
            origin_y,
            viewport_width,
            viewport_height,
            line_height,
            |_| 0.0,
        )
    }

    /// Same contract as [`paint`], but shifts each toast box horizontally
    /// by `x_offset_for(id)` points before painting — the hook a backend's
    /// entrance-slide animation uses to paint a toast mid-flight without
    /// perturbing the returned [`ToastStackLayout`], which stays at the
    /// toast's resting position so hit-testing is unaffected by the
    /// in-flight visual offset.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_with_x_offset<F>(
        stack: &ToastOverlay,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        origin_x: f32,
        origin_y: f32,
        viewport_width: f32,
        viewport_height: f32,
        line_height: f32,
        x_offset_for: F,
    ) -> ToastStackLayout
    where
        F: Fn(&WidgetId) -> f32,
    {
        // The box/dismiss/action geometry comes from
        // `pixel_toast_stack_layout`, the crate's one pixel-unit toast
        // measurement — the same function every no-paint hit-test caller
        // resolves its own layout through, so paint and hit-test can't
        // drift apart.
        let layout = {
            let measure = SurfaceMeasure(&*surface);
            crate::primitives::layout_metrics::pixel_toast_stack_layout(
                stack,
                &measure,
                origin_x,
                origin_y,
                viewport_width,
                viewport_height,
                line_height,
            )
        };
        paint_layout(
            stack,
            &layout,
            surface,
            theme,
            style,
            line_height,
            x_offset_for,
        );
        layout
    }

    /// A live [`PaintSurface`]'s own text measurement, as the
    /// [`crate::primitives::layout_metrics::TextMeasure`]
    /// [`crate::primitives::layout_metrics::pixel_toast_stack_layout`]
    /// takes.
    struct SurfaceMeasure<'a>(&'a dyn PaintSurface);

    impl crate::primitives::layout_metrics::TextMeasure for SurfaceMeasure<'_> {
        fn width_of(&self, text: &str) -> f32 {
            self.0.surface_measure_text(text).0
        }
    }

    /// Paint an already-resolved [`ToastStackLayout`] — the half of
    /// [`paint_with_x_offset`] that rasterises, split out so
    /// [`crate::primitive::Primitive::paint`] can render the layout its
    /// own `layout()` call already produced instead of measuring the
    /// stack a second time.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_layout<F>(
        stack: &ToastOverlay,
        layout: &ToastStackLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        line_height: f32,
        x_offset_for: F,
    ) where
        F: Fn(&WidgetId) -> f32,
    {
        for vt in &layout.visible_toasts {
            let toast = &stack.toasts[vt.toast_idx];
            let focus = stack.focus.as_ref().filter(|f| f.toast_id == toast.id);
            let dx = x_offset_for(&vt.id);
            if dx == 0.0 {
                surface.surface_note_overlay(vt.bounds);
                paint_toast(surface, theme, style, vt, toast, line_height, focus);
            } else {
                let shifted = VisibleToast {
                    toast_idx: vt.toast_idx,
                    id: vt.id,
                    bounds: shift_rect(vt.bounds, dx, 0.0),
                    dismiss_bounds: vt.dismiss_bounds.map(|r| shift_rect(r, dx, 0.0)),
                    action_rects: vt
                        .action_rects
                        .iter()
                        .map(|r| shift_rect(*r, dx, 0.0))
                        .collect(),
                };
                surface.surface_note_overlay(shifted.bounds);
                paint_toast(surface, theme, style, &shifted, toast, line_height, focus);
            }
        }
    }

    /// Paint one resolved toast box: background tint, a theme border,
    /// title, optional body, a top-right dismiss `×`, and on their own row
    /// at the bottom-right (never inline with the title) zero or more action
    /// buttons: the one marked [`crate::primitives::toast::ToastButton::
    /// primary`] filled with `theme.accent_bg`/`theme.foreground` (matching
    /// `tui::toolbar`'s focused-action pairing), the rest plain text in
    /// `theme.link_fg` (a secondary/ghost-button look). `focus`, when it
    /// names a control on this toast, gets a `theme.link_fg` focus-ring
    /// stroke (every backend must draw one).
    ///
    /// The title is truncated/ellipsized to the space left of the dismiss
    /// button — never drawn under it. The body wraps across up to
    /// [`MAX_BODY_LINES`] lines at the box's full width, offset below the
    /// title by the title's own *measured* height, not the nominal
    /// `line_height`, each subsequent line advancing by `line_height`.
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
            // Offset by the title's own measured pixel height, not the
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
            let is_focused = matches!(focus, Some(f) if f.target == ToastFocusTarget::Action(i));

            // An explicit `spec` override paints through the shared
            // `Button` fill/border/icon/intent-colour path instead of
            // this action's own filled-chip/plain-text look, so e.g. a
            // `Danger` spec reads in `theme.button_danger_bg()` rather
            // than the plain `accent_bg`/`link_fg` pair below —
            // `button::native_surface_paint::paint` also draws its own
            // focus ring, so `is_focused` reaches it instead of the
            // `surface_stroke_rect` call this arm uses otherwise. A
            // `spec: None` action keeps painting through the plain
            // filled-chip/plain-text path below, unchanged.
            if let Some(spec) = &action.spec {
                let button = spec.to_button(action.id, is_focused);
                crate::primitives::button::native_surface_paint::paint(
                    &button, surface, theme, *ab, style, false, false,
                );
                continue;
            }

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
            if is_focused {
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
            /// Records `surface_stroke_rect` calls so focus-ring tests can
            /// confirm one was drawn and where.
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
            assert_eq!(surface.fills[0].1, theme.danger_bg());
        }

        /// Regression: the dismiss glyph and action label must be painted
        #[test]
        fn dismiss_and_action_are_centred_in_their_sub_region() {
            let mut t = toast("t1", "Build failed");
            t.actions = vec![ToastButton {
                spec: None,
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

        /// Acceptance: two actions sit on their own row at the bottom-right,
        #[test]
        fn multiple_actions_are_right_aligned_on_their_own_row() {
            let mut t = toast("t1", "Install Markdown Language Server?");
            t.actions = vec![
                ToastButton {
                    spec: None,
                    id: WidgetId::new("install"),
                    label: "Install".into(),
                    primary: true,
                },
                ToastButton {
                    spec: None,
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
                    spec: None,
                    id: WidgetId::new("install"),
                    label: "Install".into(),
                    primary: true,
                },
                ToastButton {
                    spec: None,
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

        /// A `Primary` spec on a `ToastButton` paints through
        /// `button::native_surface_paint::paint` —
        /// `theme.accent_bg` via the shared `Button` fill path, with its
        /// bracket-less chrome (border + fill), not just the plain
        /// `surface_fill_rect` this module's own `primary` legacy field
        /// paints without `spec`.
        #[test]
        fn primary_spec_action_paints_through_the_shared_button_path() {
            let mut t = toast("t1", "Install?");
            let spec = crate::primitives::button::ButtonSpec::new("Install");
            t.actions = vec![ToastButton {
                spec: Some(spec),
                id: WidgetId::new("install"),
                label: "Install".into(),
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
            let install_bounds = layout.visible_toasts[0].action_rects[0];

            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, c)| *r == install_bounds && *c == theme.accent_bg),
                "a Primary spec must still fill theme.accent_bg through the \
                 shared Button path; fills were {:?}",
                surface.fills,
            );
            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == "Install"),
                "the button label must still be painted through the shared \
                 Button path; text runs were {:?}",
                surface.text_runs,
            );
        }

        /// A `Danger` spec on a `ToastButton` paints
        /// `theme.button_danger_bg()`, not `theme.accent_bg` — the
        /// vocabulary the legacy `primary` flag alone can't express.
        #[test]
        fn danger_spec_action_fills_the_shared_button_danger_colour() {
            let mut t = toast("t1", "Delete file?");
            let mut spec = crate::primitives::button::ButtonSpec::new("Delete");
            spec.variant = crate::primitives::button::ButtonVariant::Danger;
            t.actions = vec![ToastButton {
                spec: Some(spec),
                id: WidgetId::new("delete"),
                label: "Delete".into(),
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
            let delete_bounds = layout.visible_toasts[0].action_rects[0];

            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, c)| *r == delete_bounds && *c == theme.button_danger_bg()),
                "a Danger spec must fill the shared Button's danger colour; \
                 fills were {:?}",
                surface.fills,
            );
        }

        /// A focused dismiss button gets a `theme.link_fg` stroke
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

        /// A focused action button gets a `theme.link_fg` stroke
        /// around its own bounds — a different toast/action combination
        /// than the dismiss ring, confirming the focus target is looked
        /// up per-toast (`ToastFocus::toast_id`), not just "the first
        /// toast".
        #[test]
        fn focused_action_gets_a_focus_ring() {
            let t1 = toast("t1", "Saved");
            let mut t2 = toast("t2", "Install?");
            t2.actions = vec![ToastButton {
                spec: None,
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

        /// Regression: the body line must be offset by the title's own
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

        /// Acceptance regression: a toast with a long title, an action
        #[test]
        fn long_title_and_body_stay_inside_bounds_and_dont_overlap_action() {
            let mut t = toast("t1", "Install Markdown Language Server?");
            t.actions = vec![ToastButton {
                spec: None,
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

            // The title run must end at or before the dismiss
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

// ── Widget contract ─────────────────────────────────────────────────────────

/// [`ToastOverlay`]'s fully-resolved widget-contract layout: the absolute
/// overlay area the stack was laid out into, plus the
/// [`ToastStackLayout`] resolved inside it.
///
/// `layout`'s rects are **absolute** — [`ToastOverlay::layout`] bakes
/// `area`'s origin into every bound, the same frame
/// [`ToastStackLayout::hit_test`] has always taken — so [`Self::hit_test`]
/// takes absolute coordinates too.
/// [`crate::interactive::Interactive::sub_regions`], which must answer in
/// widget-local space, subtracts `area`'s origin itself.
///
/// `#[non_exhaustive]`: produced by
/// [`Primitive::layout`](crate::primitive::Primitive::layout), never built
/// by a consumer via struct literal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ToastOverlayPrimitiveLayout {
    /// The rect [`Primitive::layout`](crate::primitive::Primitive::layout)
    /// received — the app's overlay area, which the stack anchors its
    /// corner to.
    pub area: Rect,
    /// The stack's own resolved layout, in absolute coordinates.
    pub layout: ToastStackLayout,
}

impl ToastOverlayPrimitiveLayout {
    /// Classify an absolute point against the stack — see
    /// [`ToastStackLayout::hit_test`].
    pub fn hit_test(&self, x: f32, y: f32) -> ToastHit {
        self.layout.hit_test(x, y)
    }
}

/// [`crate::interactive::Interactive::State`] for [`ToastOverlay`]: which
/// control keyboard focus has reached.
///
/// A stack starts unfocused and only becomes focused when a host asks it
/// to (a click on a toast, or a `FocusIn` from the host's own "focus
/// notifications" command) — toasts never steal focus on their own, the
/// same non-modal contract this module's doc describes.
///
/// `#[non_exhaustive]`: a future field add here is additive.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToastOverlayState {
    /// Which toast and control currently hold focus, or `None` while the
    /// stack is unfocused. A host paints the ring by threading this back
    /// onto the next frame's [`ToastOverlay::focus`].
    pub focus: Option<ToastFocus>,
}

/// What a [`ToastOverlay`]'s own
/// [`crate::interactive::Interactive::handle`] resolves one gesture to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastOverlayAction {
    /// An action button was activated — by click, or by Enter on the
    /// keyboard-focused action. Carries that [`ToastButton::id`],
    /// matching [`ToastHit::Action`]'s shape so a host dispatches
    /// identically whether the action was reached by mouse or keyboard.
    ActionActivated(WidgetId),
    /// A toast was dismissed — its `×` was clicked, Enter activated the
    /// `×` holding focus, or Escape dismissed whichever box held focus.
    /// Carries that [`Toast::id`], matching [`ToastHit::Dismiss`]'s shape.
    /// The host removes that toast from the next frame's descriptor; this
    /// widget never mutates the stack it was handed.
    Dismissed(WidgetId),
    /// Escape cleared focus with no toast left to dismiss — reachable
    /// when the box holding focus already vanished from the descriptor
    /// by some other path (an expiry timer, a click elsewhere).
    FocusReturned,
}

impl crate::interactive::WidgetAction for ToastOverlayAction {
    /// An activated action rides [`crate::UiEvent::ToastActionClicked`]
    /// under that action's own id — the variant every in-canvas toast
    /// action click already arrives as.
    ///
    /// A dismissal has no dedicated `UiEvent` of its own, so it rides the
    /// same variant under the **toast's** id instead: a host already
    /// holds the toast list it is about to filter, so matching the id
    /// against it is how it tells the two apart, and nothing is lost in
    /// the mapping. [`Self::FocusReturned`] reports the overlay's own id
    /// for the same reason — it names the stack, not a button.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            ToastOverlayAction::ActionActivated(action) => {
                crate::UiEvent::ToastActionClicked(action)
            }
            ToastOverlayAction::Dismissed(toast) => crate::UiEvent::ToastActionClicked(toast),
            ToastOverlayAction::FocusReturned => crate::UiEvent::ToastActionClicked(id),
        }
    }
}

/// Pack a `(toast index, control index)` pair into one
/// [`crate::interactive::SubTarget`] — control `0` is the dismiss `×`,
/// control `i + 1` is [`Toast::actions`]`[i]`, the same ordering
/// [`ToastFocusTarget`] itself uses.
fn pack_sub(toast_idx: usize, control_idx: usize) -> crate::interactive::SubTarget {
    crate::interactive::SubTarget(((toast_idx as u64) << 32) | control_idx as u64)
}

/// Inverse of [`pack_sub`].
fn unpack_sub(sub: crate::interactive::SubTarget) -> (usize, usize) {
    ((sub.0 >> 32) as usize, (sub.0 & 0xffff_ffff) as usize)
}

/// A [`ToastFocusTarget`] as its control index — see [`pack_sub`].
pub(crate) fn target_to_control_index(target: ToastFocusTarget) -> usize {
    match target {
        ToastFocusTarget::Dismiss => 0,
        ToastFocusTarget::Action(i) => i + 1,
    }
}

/// A control index as its [`ToastFocusTarget`] — see [`pack_sub`].
pub(crate) fn control_index_to_target(index: usize) -> ToastFocusTarget {
    if index == 0 {
        ToastFocusTarget::Dismiss
    } else {
        ToastFocusTarget::Action(index - 1)
    }
}

impl ToastOverlay {
    /// Dispatch one keystroke against a live focus cursor — the whole
    /// keyboard model for a focused stack, shared by
    /// [`crate::interactive::Interactive::handle`] and
    /// [`crate::compose::ToastStackController::handle_event`] so both
    /// agree on every key.
    ///
    /// Tab/Shift-Tab and Left/Right cycle the controls of whichever box
    /// holds focus (dismiss `×` first, then each action, wrapping at
    /// either end); Up/Down move between toasts in [`Self::toasts`]
    /// order, clamping at either end and resetting the in-toast cursor to
    /// the dismiss button; Enter activates the control holding focus;
    /// Escape dismisses that box *and* clears focus. Every other key, and
    /// every key while `focus` is `None`, is [`Handled::Ignored`] — the
    /// stack never claims a key it isn't the target of.
    ///
    /// A `focus` naming a toast absent from the descriptor is cleared
    /// first, so a host that keeps feeding keys after an expiry never
    /// gets an action for a box that has already gone.
    pub(crate) fn handle_focus_key(
        &self,
        focus: &mut Option<ToastFocus>,
        key: &Key,
        modifiers: &Modifiers,
    ) -> Handled<ToastOverlayAction> {
        if let Some(f) = focus.as_ref() {
            if !self.toasts.iter().any(|t| t.id == f.toast_id) {
                *focus = None;
            }
        }
        let Some(current) = focus.clone() else {
            return Handled::Ignored;
        };
        let Some(toast_idx) = self.toasts.iter().position(|t| t.id == current.toast_id) else {
            *focus = None;
            return Handled::Ignored;
        };
        let control_count = 1 + self.toasts[toast_idx].actions.len();
        let cur = target_to_control_index(current.target);
        let set_control = |focus: &mut Option<ToastFocus>, index: usize| {
            *focus = Some(ToastFocus {
                toast_id: current.toast_id,
                target: control_index_to_target(index),
            });
        };

        match key {
            Key::Named(NamedKey::Escape) => {
                *focus = None;
                Handled::Event(ToastOverlayAction::Dismissed(current.toast_id))
            }
            Key::Named(NamedKey::Enter) => match current.target {
                ToastFocusTarget::Dismiss => {
                    Handled::Event(ToastOverlayAction::Dismissed(current.toast_id))
                }
                ToastFocusTarget::Action(i) => match self.toasts[toast_idx].actions.get(i) {
                    Some(a) => Handled::Event(ToastOverlayAction::ActionActivated(a.id)),
                    None => Handled::Consumed,
                },
            },
            Key::Named(NamedKey::Left) => {
                set_control(focus, crate::focus::cycle_index(cur, -1, control_count));
                Handled::Consumed
            }
            Key::Named(NamedKey::Right) => {
                set_control(focus, crate::focus::cycle_index(cur, 1, control_count));
                Handled::Consumed
            }
            Key::Named(NamedKey::Tab) => {
                let delta = if modifiers.shift { -1 } else { 1 };
                set_control(focus, crate::focus::cycle_index(cur, delta, control_count));
                Handled::Consumed
            }
            Key::Named(NamedKey::BackTab) => {
                set_control(focus, crate::focus::cycle_index(cur, -1, control_count));
                Handled::Consumed
            }
            Key::Named(NamedKey::Up) => {
                if toast_idx > 0 {
                    self.focus_toast_at(focus, toast_idx - 1);
                }
                Handled::Consumed
            }
            Key::Named(NamedKey::Down) => {
                if toast_idx + 1 < self.toasts.len() {
                    self.focus_toast_at(focus, toast_idx + 1);
                }
                Handled::Consumed
            }
            _ => Handled::Ignored,
        }
    }

    /// Move `focus` to the toast at `idx`, seeded on its dismiss `×`.
    pub(crate) fn focus_toast_at(&self, focus: &mut Option<ToastFocus>, idx: usize) {
        if let Some(t) = self.toasts.get(idx) {
            *focus = Some(ToastFocus {
                toast_id: t.id,
                target: ToastFocusTarget::Dismiss,
            });
        }
    }

    /// Resolve whichever control `focus` names to the action it would
    /// produce — what Enter and an assistive-technology `Click` both do.
    fn activate_focused(&self, focus: &Option<ToastFocus>) -> Option<ToastOverlayAction> {
        let f = focus.as_ref()?;
        let toast = self.toasts.iter().find(|t| t.id == f.toast_id)?;
        match f.target {
            ToastFocusTarget::Dismiss => Some(ToastOverlayAction::Dismissed(toast.id)),
            ToastFocusTarget::Action(i) => toast
                .actions
                .get(i)
                .map(|a| ToastOverlayAction::ActionActivated(a.id)),
        }
    }
}

impl crate::primitive::Primitive for ToastOverlay {
    type Layout = ToastOverlayPrimitiveLayout;
    type Event = ToastHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Toast(self.clone())
    }

    /// `rect` is the app's overlay area — the stack anchors
    /// [`Self::corner`] to it and sizes each box from its own content.
    ///
    /// On a cell grid that is [`cell_toast_stack_layout`]; on a pixel
    /// surface it is
    /// [`crate::primitives::layout_metrics::pixel_toast_stack_layout`],
    /// measuring action labels through the calling backend's real
    /// [`crate::Backend::measure_text`].
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> ToastOverlayPrimitiveLayout {
        let layout = match ctx.units() {
            crate::primitive::Units::Cells => cell_toast_stack_layout(self, rect),
            _ => {
                let measure = crate::primitives::layout_metrics::LayoutCtxMeasure {
                    ctx,
                    role: crate::FontRole::Chrome,
                };
                crate::primitives::layout_metrics::pixel_toast_stack_layout(
                    self,
                    &measure,
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    ctx.line_height,
                )
            }
        };
        ToastOverlayPrimitiveLayout { area: rect, layout }
    }

    /// The shared pixel painter over the already-resolved layout, with
    /// the surface's own line height as the row pitch — the same metric
    /// every pixel backend's `draw_toast_stack` passes it.
    fn paint(
        &self,
        layout: &ToastOverlayPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let line_height = surface.surface_line_height();
            native_surface_paint::paint_layout(
                self,
                &layout.layout,
                surface,
                theme,
                style,
                line_height,
                |_| 0.0,
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_toast_stack`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`. That
    /// delegate re-resolves the same cell layout from
    /// [`ToastOverlayPrimitiveLayout::area`] that this layout already
    /// holds, so paint and hit-test stay in agreement.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ToastOverlayPrimitiveLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        // The painter never moves the cursor, so the sink is never read.
        let mut cursor_sink = None;
        let mut surface = crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            &mut cursor_sink,
        );
        cell_paint::paint(self, &layout.layout, &mut surface, theme, layout.area);
    }
}

impl crate::interactive::Interactive for ToastOverlay {
    type State = ToastOverlayState;
    type Action = ToastOverlayAction;

    /// One focus stop per control of every toast — dismiss `×` first,
    /// then each [`Toast::actions`] entry, toast by toast — so a host
    /// that hands the stack keyboard focus can walk straight to a
    /// control. [`crate::interactive::FocusSpec::None`] for an empty
    /// stack: there is nothing to focus.
    fn focus(&self) -> crate::interactive::FocusSpec {
        if self.toasts.is_empty() {
            return crate::interactive::FocusSpec::None;
        }
        crate::interactive::FocusSpec::Children(
            self.toasts
                .iter()
                .enumerate()
                .flat_map(|(ti, toast)| {
                    (0..1 + toast.actions.len()).map(move |ci| crate::interactive::FocusStop {
                        sub: pack_sub(ti, ci),
                    })
                })
                .collect(),
        )
    }

    /// Each toast's dismiss `×` and action buttons, most-specific first
    /// — translated out of [`ToastStackLayout`]'s absolute frame into the
    /// widget-local space this method is contracted to answer in. The
    /// toast body itself is not an addressable sub-part: it carries no
    /// focus stop and no action.
    fn sub_regions(
        &self,
        layout: &ToastOverlayPrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        let (ox, oy) = (layout.area.x, layout.area.y);
        let local = |r: Rect| Rect::new(r.x - ox, r.y - oy, r.width, r.height);
        let mut out = Vec::new();
        for vt in &layout.layout.visible_toasts {
            if let Some(db) = vt.dismiss_bounds {
                out.push((local(db), pack_sub(vt.toast_idx, 0)));
            }
            for (i, ab) in vt.action_rects.iter().enumerate() {
                out.push((local(*ab), pack_sub(vt.toast_idx, i + 1)));
            }
        }
        out
    }

    /// `Click` activates whichever control keyboard focus names, so a
    /// screen reader can dismiss a toast or run its action.
    fn a11y_actions(&self, state: &ToastOverlayState) -> Vec<crate::a11y::A11yAction> {
        if state.focus.is_some() {
            vec![crate::a11y::A11yAction::Click]
        } else {
            Vec::new()
        }
    }

    /// A click on a toast's `×` dismisses it, a click on an action button
    /// activates that action, and a click on a toast body focuses that
    /// toast's `×` (the same seed [`crate::compose::ToastStackController::give_focus`]
    /// uses) without producing an action. Keys run the shared keyboard
    /// model — see [`ToastOverlay::handle_focus_key`]. A click that
    /// lands on no toast at all is [`Handled::Ignored`]: the stack is
    /// non-modal and never swallows input aimed at the app behind it.
    fn handle(
        &self,
        state: &mut ToastOverlayState,
        event: &crate::interactive::RoutedEvent,
        layout: &ToastOverlayPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<ToastOverlayAction> {
        use crate::interactive::{KeyEvent, PointerKind, RoutedEvent};

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                local,
                ..
            } => {
                let (x, y) = (layout.area.x + local.x, layout.area.y + local.y);
                match layout.hit_test(x, y) {
                    ToastHit::Dismiss(toast_id) => {
                        state.focus = None;
                        Handled::Event(ToastOverlayAction::Dismissed(toast_id))
                    }
                    ToastHit::Action(action_id) => {
                        if let Some((ti, ci)) = self.find_action(action_id) {
                            state.focus = Some(ToastFocus {
                                toast_id: self.toasts[ti].id,
                                target: control_index_to_target(ci + 1),
                            });
                            cx.request_focus(Some(pack_sub(ti, ci + 1)));
                        }
                        Handled::Event(ToastOverlayAction::ActionActivated(action_id))
                    }
                    ToastHit::Body(toast_id) => {
                        if let Some(ti) = self.toasts.iter().position(|t| t.id == toast_id) {
                            self.focus_toast_at(&mut state.focus, ti);
                            cx.request_focus(Some(pack_sub(ti, 0)));
                        }
                        Handled::Consumed
                    }
                    ToastHit::Empty => Handled::Ignored,
                }
            }
            RoutedEvent::Key(KeyEvent { key, modifiers, .. }) => {
                self.handle_focus_key(&mut state.focus, key, modifiers)
            }
            RoutedEvent::Action(crate::a11y::A11yAction::Click) => {
                match self.activate_focused(&state.focus) {
                    Some(action) => Handled::Event(action),
                    None => Handled::Ignored,
                }
            }
            RoutedEvent::FocusIn(Some(sub)) => {
                let (ti, ci) = unpack_sub(*sub);
                match self.toasts.get(ti) {
                    Some(toast) if ci <= toast.actions.len() => {
                        state.focus = Some(ToastFocus {
                            toast_id: toast.id,
                            target: control_index_to_target(ci),
                        });
                        Handled::Consumed
                    }
                    _ => Handled::Ignored,
                }
            }
            RoutedEvent::FocusOut => {
                state.focus = None;
                Handled::Consumed
            }
            _ => Handled::Ignored,
        }
    }
}

impl ToastOverlay {
    /// Locate an action button by its own id, as a
    /// `(toast index, action index)` pair.
    fn find_action(&self, action_id: WidgetId) -> Option<(usize, usize)> {
        self.toasts.iter().enumerate().find_map(|(ti, toast)| {
            toast
                .actions
                .iter()
                .position(|a| a.id == action_id)
                .map(|ai| (ti, ai))
        })
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
            spec: None,
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
        // not inline with it.
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

    /// A toast with two actions produces two, non-overlapping
    /// `action_rects` entries, aligned 1:1 with `Toast::actions`.
    #[test]
    fn toast_layout_multiple_action_regions() {
        let mut toast = make_toast("t1", "Install Markdown Language Server?");
        toast.actions = vec![
            ToastButton {
                spec: None,
                id: WidgetId::new("install"),
                label: "Install".to_string(),
                primary: true,
            },
            ToastButton {
                spec: None,
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

    /// Non-zero-origin regression guard: `ToastOverlay::layout` previously
    #[test]
    fn toast_layout_nonzero_origin_shifts_every_bound() {
        let mut toast = make_toast("t1", "Build failed");
        toast.actions = vec![ToastButton {
            spec: None,
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

    // ── B3b: ButtonSpec seam ─────────────────────────────────────────────

    #[test]
    fn primary_action_without_spec_derives_a_primary_spec() {
        let action = ToastButton {
            id: WidgetId::new("toast:install"),
            label: "Install".to_string(),
            primary: true,
            spec: None,
        };
        let spec = action.resolved_button_spec();
        assert_eq!(spec.label, "Install");
        assert_eq!(
            spec.variant,
            crate::primitives::button::ButtonVariant::Primary
        );
    }

    #[test]
    fn non_primary_action_without_spec_derives_a_secondary_spec() {
        let action = ToastButton {
            id: WidgetId::new("toast:dismiss"),
            label: "Don't ask again".to_string(),
            primary: false,
            spec: None,
        };
        assert_eq!(
            action.resolved_button_spec().variant,
            crate::primitives::button::ButtonVariant::Secondary
        );
    }

    #[test]
    fn action_with_explicit_spec_overrides_the_legacy_fields() {
        let mut explicit = crate::primitives::button::ButtonSpec::new("Report issue");
        explicit.variant = crate::primitives::button::ButtonVariant::Warning;
        let action = ToastButton {
            id: WidgetId::new("toast:report"),
            label: "Report".to_string(),
            primary: false,
            spec: Some(explicit.clone()),
        };
        assert_eq!(action.resolved_button_spec(), explicit);
    }
}

/// The cell painter's own paint/click round-trip coverage, driven
/// through `crate::tui::draw_toast_stack` — the delegate that wraps a
/// ratatui `Buffer` in the `CellSurface` [`cell_paint::paint`] takes.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use super::cell_test_paint::draw_toast_stack;
    use super::{
        Toast, ToastButton, ToastCorner, ToastFocus, ToastFocusTarget, ToastHit, ToastOverlay,
        ToastSeverity, CELL_BORDER_COLS, CELL_DISMISS_WIDTH, CELL_TOAST_MAX_WIDTH,
        CELL_TOAST_MIN_WIDTH,
    };
    use crate::theme::Theme;
    use crate::tui::ratatui_color;
    use crate::types::WidgetId;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn info_toast(id: &str, title: &str) -> Toast {
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

    /// Shared body for `single_toast_paint_and_click_round_trip`, run at
    /// both the origin and a non-zero `area` origin. `tui_toast_stack_layout`
    /// bakes `area.x`/`area.y` straight into the returned bounds (absolute
    /// frame, matching `MenuBar::cell_layout`), so both the
    /// painted glyph position AND the `hit_test` coordinates must shift
    /// together with the origin.
    fn single_toast_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 60, 20);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 60, origin_y + 20));
        let stack = stack_br(vec![info_toast("t1", "Hello world")]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        assert_eq!(layout.visible_toasts.len(), 1);
        let vt = &layout.visible_toasts[0];

        // Title should be painted inside the toast bounds, on the first
        // interior row below the top border.
        let tx = vt.bounds.x.round() as u16 + 1;
        let ty = vt.bounds.y.round() as u16 + 1;
        assert_eq!(cell_char(&buf, tx, ty), 'H');

        // Hit-test on the title text → Body.
        let hit = layout.hit_test(tx as f32 + 0.5, ty as f32 + 0.5);
        assert_eq!(hit, ToastHit::Body(WidgetId::new("t1")));
    }

    #[test]
    fn single_toast_paint_and_click_round_trip() {
        single_toast_paint_and_click_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard: same round trip,
    /// painted at a shifted `area` origin.
    #[test]
    fn single_toast_paint_and_click_round_trip_at_nonzero_origin() {
        single_toast_paint_and_click_round_trip_at(7, 13);
    }

    /// Shared body for `dismiss_glyph_paint_and_click_round_trip` —
    /// see [`single_toast_paint_and_click_round_trip_at`] for the
    /// coordinate-frame rationale.
    fn dismiss_glyph_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 60, 20);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 60, origin_y + 20));
        let stack = stack_br(vec![info_toast("t1", "Test")]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        let db = vt.dismiss_bounds.expect("dismiss bounds present");
        let dx = db.x.round() as u16 + (db.width as u16).saturating_sub(1) / 2;
        // Dismiss glyph paints on its own inset row near the top-right
        let dy = db.y.round() as u16;
        assert_eq!(cell_char(&buf, dx, dy), '×');

        let hit = layout.hit_test(dx as f32 + 0.5, dy as f32 + 0.5);
        assert_eq!(hit, ToastHit::Dismiss(WidgetId::new("t1")));
    }

    #[test]
    fn dismiss_glyph_paint_and_click_round_trip() {
        dismiss_glyph_paint_and_click_round_trip_at(0, 0);
    }

    #[test]
    fn dismiss_glyph_paint_and_click_round_trip_at_nonzero_origin() {
        dismiss_glyph_paint_and_click_round_trip_at(7, 13);
    }

    /// Shared body for `action_button_paint_and_click_round_trip` —
    /// see [`single_toast_paint_and_click_round_trip_at`] for the
    /// coordinate-frame rationale.
    fn action_button_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 60, 20);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 60, origin_y + 20));
        let mut toast = info_toast("t1", "Error occurred");
        toast.actions = vec![ToastButton {
            spec: None,
            id: WidgetId::new("retry"),
            label: "Retry".into(),
            primary: false,
        }];
        let stack = stack_br(vec![toast]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        assert_eq!(vt.action_rects.len(), 1);
        let ab = vt.action_rects[0];
        let ax0 = ab.x.round() as u16;
        let aw = ab.width.round() as u16;
        let ax = ax0 + (aw.saturating_sub("Retry".chars().count() as u16)) / 2;
        // Action label paints on its own row at the bottom-right.
        let ay = ab.y.round() as u16;
        assert_eq!(cell_char(&buf, ax, ay), 'R');

        let hit = layout.hit_test(ax as f32 + 0.5, ay as f32 + 0.5);
        assert_eq!(hit, ToastHit::Action(WidgetId::new("retry")));
    }

    #[test]
    fn action_button_paint_and_click_round_trip() {
        action_button_paint_and_click_round_trip_at(0, 0);
    }

    #[test]
    fn action_button_paint_and_click_round_trip_at_nonzero_origin() {
        action_button_paint_and_click_round_trip_at(7, 13);
    }

    #[test]
    fn body_text_paints_on_second_row() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let mut toast = info_toast("t1", "Title");
        toast.body = "Body text here".into();
        let stack = stack_br(vec![toast]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        let bx = vt.bounds.x.round() as u16 + 1;
        // Body paints below the title row, which itself sits below the
        // top border: bounds.y (border) + 1 (title) + 1 (body).
        let body_y = vt.bounds.y.round() as u16 + 2;
        assert_eq!(cell_char(&buf, bx, body_y), 'B');
    }

    /// Acceptance regression: the box widens to fit the whole title
    #[test]
    fn long_title_with_action_widens_box_and_avoids_overlap() {
        let area = Rect::new(0, 0, 90, 20);
        let mut buf = Buffer::empty(area);
        let mut toast = info_toast("t1", "Install Markdown Language Server?");
        toast.actions = vec![ToastButton {
            spec: None,
            id: WidgetId::new("install"),
            label: "Install".into(),
            primary: true,
        }];
        toast.body = "N: don't ask again · :ExtInstall markdown-language-server".into();
        let stack = stack_br(vec![toast.clone()]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        assert_eq!(vt.action_rects.len(), 1);
        let ab = vt.action_rects[0];
        let db = vt.dismiss_bounds.expect("dismiss bounds present");

        // Box widened to fit the whole title + dismiss, up to the
        // configured max.
        let title_len = toast.title.chars().count() as f32;
        let expected_width = (title_len + CELL_BORDER_COLS + CELL_DISMISS_WIDTH)
            .clamp(CELL_TOAST_MIN_WIDTH, CELL_TOAST_MAX_WIDTH);
        assert_eq!(vt.bounds.width, expected_width);

        // The whole title painted, ending strictly before the dismiss
        // column — never overlapping it.
        let bx = vt.bounds.x.round() as u16;
        let ty = vt.bounds.y.round() as u16 + 1;
        let dx = db.x.round() as u16;
        let painted_title: String = (bx + 1..dx).map(|x| cell_char(&buf, x, ty)).collect();
        assert_eq!(painted_title.trim_end(), toast.title);

        // The action row is on its own line, strictly below the title
        // row and the dismiss row — never inline with either.
        assert!(ab.y > db.y);
        assert!(ab.y > vt.bounds.y + 1.0);

        // Border corners are visible.
        let by = vt.bounds.y.round() as u16;
        let bottom_y = by + vt.bounds.height.round() as u16 - 1;
        assert_eq!(cell_char(&buf, bx, by), '┌');
        assert_eq!(cell_char(&buf, bx, bottom_y), '└');

        // Body wrapped onto a row inside the box, above the bottom border.
        let body_y1 = ty + 1;
        assert_ne!(cell_char(&buf, bx + 1, body_y1), ' ');
        assert!(
            bottom_y > body_y1,
            "box should have room for the wrapped body before its border"
        );
    }

    /// Two actions produce two distinct, side-by-side button
    /// rects on the same row, with the primary action's row filled with
    /// `theme.accent_bg`.
    #[test]
    fn two_actions_paint_side_by_side_with_primary_filled() {
        let area = Rect::new(0, 0, 90, 20);
        let mut buf = Buffer::empty(area);
        let mut toast = info_toast("t1", "Install?");
        toast.actions = vec![
            ToastButton {
                spec: None,
                id: WidgetId::new("install"),
                label: "Install".into(),
                primary: true,
            },
            ToastButton {
                spec: None,
                id: WidgetId::new("skip"),
                label: "Skip".into(),
                primary: false,
            },
        ];
        let stack = stack_br(vec![toast]);
        let theme = Theme::default();
        let layout = draw_toast_stack(&mut buf, area, &stack, &theme);

        let vt = &layout.visible_toasts[0];
        assert_eq!(vt.action_rects.len(), 2);
        let install = vt.action_rects[0];
        let skip = vt.action_rects[1];
        assert_eq!(install.y, skip.y);
        assert!(install.x + install.width <= skip.x);

        // Primary's row is filled with `theme.accent_bg`.
        let ax = install.x.round() as u16;
        let ay = install.y.round() as u16;
        let cell = &buf[(ax, ay)];
        assert_eq!(cell.bg, ratatui_color(theme.accent_bg));
    }

    /// A `ToastButton` with an explicit `Danger` spec tints its label
    /// `theme.error_fg` — no background fill of its own, same "intent
    /// via foreground only" convention `crate::tui::button` uses on TUI.
    /// A `spec: None` action (every pre-existing construction site) keeps
    /// its plain primary/secondary look, unaffected.
    #[test]
    fn danger_spec_action_tints_error_fg_with_no_background_fill() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let mut toast = info_toast("t1", "Delete this file?");
        let mut spec = crate::primitives::button::ButtonSpec::new("Delete");
        spec.variant = crate::primitives::button::ButtonVariant::Danger;
        toast.actions = vec![ToastButton {
            spec: Some(spec),
            id: WidgetId::new("delete"),
            label: "Delete".into(),
            primary: false,
        }];
        let stack = stack_br(vec![toast]);
        let theme = Theme::default();
        let layout = draw_toast_stack(&mut buf, area, &stack, &theme);

        let action = layout.visible_toasts[0].action_rects[0];
        let ax = action.x.round() as u16;
        let ay = action.y.round() as u16;
        let cell = &buf[(ax, ay)];
        assert_eq!(cell.fg, ratatui_color(theme.error_fg));
        assert_ne!(
            cell.bg,
            ratatui_color(theme.accent_bg),
            "Danger must not fall back to the primary accent fill"
        );
    }

    /// A `Danger`/`Warning` spec clears `accent_bg` even when the
    /// legacy `primary` flag is also set — "no background fill of its
    /// own" holds regardless of `primary`, not only when it's `false`.
    #[test]
    fn danger_spec_action_clears_accent_bg_even_when_primary_is_set() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let mut toast = info_toast("t1", "Delete this file?");
        let mut spec = crate::primitives::button::ButtonSpec::new("Delete");
        spec.variant = crate::primitives::button::ButtonVariant::Danger;
        toast.actions = vec![ToastButton {
            spec: Some(spec),
            id: WidgetId::new("delete"),
            label: "Delete".into(),
            primary: true,
        }];
        let stack = stack_br(vec![toast]);
        let theme = Theme::default();
        let layout = draw_toast_stack(&mut buf, area, &stack, &theme);

        let action = layout.visible_toasts[0].action_rects[0];
        let ax = action.x.round() as u16;
        let ay = action.y.round() as u16;
        let cell = &buf[(ax, ay)];
        assert_eq!(cell.fg, ratatui_color(theme.error_fg));
        assert_ne!(
            cell.bg,
            ratatui_color(theme.accent_bg),
            "Danger must not fall back to the primary accent fill, even \
             when the legacy `primary` flag is also set"
        );
    }

    /// The controller's `ToastFocus` gets a visible focus
    /// indicator (inverted colours) on the dismiss cell.
    #[test]
    fn focused_dismiss_paints_inverted() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let mut stack = stack_br(vec![info_toast("t1", "Saved")]);
        stack.focus = Some(ToastFocus {
            toast_id: WidgetId::new("t1"),
            target: ToastFocusTarget::Dismiss,
        });
        let theme = Theme::default();
        let layout = draw_toast_stack(&mut buf, area, &stack, &theme);
        let db = layout.visible_toasts[0]
            .dismiss_bounds
            .expect("dismiss bounds present");
        let dx = db.x.round() as u16 + (db.width as u16).saturating_sub(1) / 2;
        let dy = db.y.round() as u16;
        let cell = &buf[(dx, dy)];
        assert_eq!(cell.bg, ratatui_color(theme.link_fg));
    }

    #[test]
    fn multiple_toasts_stack_upward_from_bottom() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let stack = stack_br(vec![
            info_toast("first", "First"),
            info_toast("second", "Second"),
        ]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        assert_eq!(layout.visible_toasts.len(), 2);
        // Newest (second) is nearest the bottom corner.
        assert_eq!(layout.visible_toasts[0].id.as_str(), "second");
        assert_eq!(layout.visible_toasts[1].id.as_str(), "first");
        // Second toast should be above the first.
        assert!(layout.visible_toasts[1].bounds.y < layout.visible_toasts[0].bounds.y);
    }

    #[test]
    fn empty_stack_paints_nothing() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let stack = stack_br(vec![]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());
        assert!(layout.visible_toasts.is_empty());
        assert_eq!(layout.hit_test(30.0, 10.0), ToastHit::Empty);
    }

    #[test]
    fn outside_toast_returns_empty() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let stack = stack_br(vec![info_toast("t1", "Test")]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());
        // Top-left corner is far from bottom-right toast.
        assert_eq!(layout.hit_test(0.0, 0.0), ToastHit::Empty);
    }
}

/// The `Interactive` half's own unit coverage, for the paths a headless
/// `TuiDriver` test cannot reach: the assistive-technology action route,
/// a host-driven `FocusIn`/`FocusOut`, and the stale-focus resync.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::event::{MouseButton, Point};
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock — the same shape
    /// `crate::primitives::tab_bar`'s own module tests use.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn toast_with(id: &str, actions: &[&str]) -> Toast {
        Toast {
            id: WidgetId::new(id),
            title: id.into(),
            body: String::new(),
            severity: ToastSeverity::Info,
            actions: actions
                .iter()
                .map(|a| ToastButton {
                    id: WidgetId::new(*a),
                    label: (*a).into(),
                    primary: false,
                    spec: None,
                })
                .collect(),
            accent: None,
        }
    }

    fn stack() -> ToastOverlay {
        ToastOverlay {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: vec![toast_with("older", &[]), toast_with("newest", &["undo"])],
            focus: None,
        }
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 60.0, 16.0)
    }

    fn layout_of(s: &ToastOverlay) -> ToastOverlayPrimitiveLayout {
        Primitive::layout(s, &LayoutCtx::new(1.0, 1.0, false), area())
    }

    fn run(
        s: &ToastOverlay,
        state: &mut ToastOverlayState,
        event: RoutedEvent,
    ) -> (Handled<ToastOverlayAction>, MockHost) {
        let layout = layout_of(s);
        let mut host = MockHost::default();
        let outcome = {
            let mut cx = EventCx::new(s.id, &mut host);
            s.handle(state, &event, &layout, &mut cx)
        };
        (outcome, host)
    }

    #[test]
    fn focus_declares_one_stop_per_control() {
        let s = stack();
        match s.focus() {
            // `older` has only its dismiss `×`; `newest` has `×` plus one
            // action.
            FocusSpec::Children(stops) => {
                assert_eq!(stops.len(), 3);
                assert_eq!(stops[0].sub, pack_sub(0, 0));
                assert_eq!(stops[1].sub, pack_sub(1, 0));
                assert_eq!(stops[2].sub, pack_sub(1, 1));
            }
            other => panic!("expected per-control stops, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_stack_takes_no_focus() {
        let mut s = stack();
        s.toasts.clear();
        assert_eq!(s.focus(), FocusSpec::None);
    }

    #[test]
    fn sub_regions_are_reported_in_widget_local_space() {
        let s = stack();
        let layout = layout_of(&s);
        let regions = s.sub_regions(&layout);
        // Two dismiss glyphs plus the one action button.
        assert_eq!(regions.len(), 3);
        let newest = layout
            .layout
            .visible_toasts
            .iter()
            .find(|vt| vt.id == WidgetId::new("newest"))
            .expect("newest toast laid out");
        let dismiss = newest.dismiss_bounds.expect("dismiss bounds");
        let (rect, sub) = regions
            .iter()
            .find(|(_, sub)| *sub == pack_sub(newest.toast_idx, 0))
            .expect("a region for the newest toast's dismiss");
        assert_eq!(*sub, pack_sub(newest.toast_idx, 0));
        assert_eq!(rect.x, dismiss.x - layout.area.x);
        assert_eq!(rect.y, dismiss.y - layout.area.y);
    }

    #[test]
    fn a11y_click_activates_whichever_control_holds_focus() {
        let mut s = stack();
        s.focus = Some(ToastFocus {
            toast_id: WidgetId::new("newest"),
            target: ToastFocusTarget::Action(0),
        });
        let mut state = ToastOverlayState {
            focus: s.focus.clone(),
        };
        let (outcome, _) = run(&s, &mut state, RoutedEvent::Action(A11yAction::Click));
        assert_eq!(
            outcome,
            Handled::Event(ToastOverlayAction::ActionActivated(WidgetId::new("undo")))
        );
    }

    #[test]
    fn a11y_click_with_nothing_focused_is_ignored() {
        let s = stack();
        let mut state = ToastOverlayState::default();
        let (outcome, _) = run(&s, &mut state, RoutedEvent::Action(A11yAction::Click));
        assert_eq!(outcome, Handled::Ignored);
        assert!(s.a11y_actions(&state).is_empty());
    }

    #[test]
    fn a_key_for_a_toast_that_left_the_descriptor_clears_focus_instead_of_acting() {
        let s = stack();
        let mut focus = Some(ToastFocus {
            toast_id: WidgetId::new("expired"),
            target: ToastFocusTarget::Dismiss,
        });
        let outcome = s.handle_focus_key(
            &mut focus,
            &Key::Named(NamedKey::Enter),
            &Modifiers::default(),
        );
        assert_eq!(outcome, Handled::Ignored);
        assert_eq!(focus, None);
    }

    #[test]
    fn focus_in_and_focus_out_track_the_hosts_focus_cursor() {
        let s = stack();
        let mut state = ToastOverlayState::default();
        let (outcome, _) = run(&s, &mut state, RoutedEvent::FocusIn(Some(pack_sub(1, 1))));
        assert_eq!(outcome, Handled::Consumed);
        assert_eq!(
            state.focus,
            Some(ToastFocus {
                toast_id: WidgetId::new("newest"),
                target: ToastFocusTarget::Action(0),
            })
        );

        let (outcome, _) = run(&s, &mut state, RoutedEvent::FocusOut);
        assert_eq!(outcome, Handled::Consumed);
        assert_eq!(state.focus, None);
    }

    #[test]
    fn a_focus_in_naming_no_control_is_ignored() {
        let s = stack();
        let mut state = ToastOverlayState::default();
        let (outcome, _) = run(&s, &mut state, RoutedEvent::FocusIn(Some(pack_sub(0, 4))));
        assert_eq!(outcome, Handled::Ignored);
        assert_eq!(state.focus, None);
    }

    #[test]
    fn a_dismiss_click_reports_the_toasts_own_id() {
        let s = stack();
        let layout = layout_of(&s);
        let newest = layout
            .layout
            .visible_toasts
            .iter()
            .find(|vt| vt.id == WidgetId::new("newest"))
            .expect("newest toast laid out");
        let dismiss = newest.dismiss_bounds.expect("dismiss bounds");
        let mut state = ToastOverlayState::default();
        let (outcome, _) = run(
            &s,
            &mut state,
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(dismiss.x - layout.area.x, dismiss.y - layout.area.y),
                mods: Modifiers::default(),
            },
        );
        assert_eq!(
            outcome,
            Handled::Event(ToastOverlayAction::Dismissed(WidgetId::new("newest")))
        );
    }

    #[test]
    fn a_click_on_no_toast_at_all_bubbles() {
        let s = stack();
        let mut state = ToastOverlayState::default();
        let (outcome, _) = run(
            &s,
            &mut state,
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(0.0, 0.0),
                mods: Modifiers::default(),
            },
        );
        assert_eq!(outcome, Handled::Ignored);
    }

    #[test]
    fn a_body_click_parks_focus_on_that_toasts_dismiss_glyph() {
        let s = stack();
        let layout = layout_of(&s);
        let newest = layout
            .layout
            .visible_toasts
            .iter()
            .find(|vt| vt.id == WidgetId::new("newest"))
            .expect("newest toast laid out");
        let mut state = ToastOverlayState::default();
        let (outcome, host) = run(
            &s,
            &mut state,
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(
                    newest.bounds.x - layout.area.x,
                    newest.bounds.y - layout.area.y,
                ),
                mods: Modifiers::default(),
            },
        );
        assert_eq!(outcome, Handled::Consumed);
        assert_eq!(
            state.focus,
            Some(ToastFocus {
                toast_id: WidgetId::new("newest"),
                target: ToastFocusTarget::Dismiss,
            })
        );
        assert_eq!(
            host.focus_requests,
            vec![Some(pack_sub(newest.toast_idx, 0))]
        );
    }

    #[test]
    fn the_cell_layout_and_the_widget_contract_layout_agree() {
        let s = stack();
        assert_eq!(layout_of(&s).layout, cell_toast_stack_layout(&s, area()));
    }

    #[test]
    fn a_key_with_no_focus_is_never_claimed() {
        let s = stack();
        let mut state = ToastOverlayState::default();
        let (outcome, _) = run(
            &s,
            &mut state,
            RoutedEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Tab),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
        );
        assert_eq!(outcome, Handled::Ignored);
    }
}

/// Cell-painting entry points used by this module's own `#[cfg(test)]`
/// rasteriser tests: they build the `Buffer` -> [`crate::CellSurface`]
/// adapter (private to `crate::tui`) the same way
/// [`crate::primitive::Primitive::paint_cells`] does, so a test can
/// paint into a bare ratatui buffer without a backend.
#[cfg(all(test, feature = "tui"))]
mod cell_test_paint {
    use super::*;
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;

    fn surface<'a>(
        buf: &'a mut Buffer,
        sink: &'a mut Option<(u16, u16, crate::cell_surface::CursorShape)>,
    ) -> impl crate::CellSurface + 'a {
        crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            sink,
        )
    }

    pub fn draw_toast_stack(
        buf: &mut Buffer,
        area: ratatui::layout::Rect,
        stack: &ToastOverlay,
        theme: &Theme,
    ) -> ToastStackLayout {
        let clip = Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        );
        let layout = cell_toast_stack_layout(stack, clip);
        let mut sink = None;
        cell_paint::paint(stack, &layout, &mut surface(buf, &mut sink), theme, clip);
        layout
    }
}
