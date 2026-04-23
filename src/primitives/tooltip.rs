//! `Tooltip` primitive: a short text popup anchored to an element.
//! Used for hover-hint text (activity bar items, status segments,
//! truncated tab labels, LSP hover results that are small enough to
//! inline rather than a full docblock).
//!
//! A `Tooltip` is paired with an **anchor** rectangle — the element it
//! describes. The layout method picks a position (`placement`) near the
//! anchor that keeps the tooltip inside the viewport.
//!
//! # Backend contract
//!
//! **Declarative + placement.** Apps decide when a tooltip shows
//! (hover delay, keyboard focus, etc.) and pass the current anchor +
//! content. The primitive's `layout()` chooses x/y based on preferred
//! placement, adjusting if it would overflow the viewport. Backends
//! render a box with the content at the resolved position.

use crate::event::Rect;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a tooltip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tooltip {
    pub id: WidgetId,
    /// Tooltip text. Single-line for most cases; backends wrap if
    /// `max_width` is narrower than the natural width.
    pub text: String,
    /// Preferred placement relative to the anchor.
    #[serde(default)]
    pub placement: TooltipPlacement,
    /// Override background colour. `None` = theme default.
    #[serde(default)]
    pub bg: Option<Color>,
    /// Override foreground colour.
    #[serde(default)]
    pub fg: Option<Color>,
}

/// Preferred placement of a `Tooltip` relative to its anchor.
///
/// The layout method falls back to the opposite side if the preferred
/// placement would overflow the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TooltipPlacement {
    /// Above the anchor, left-aligned.
    Top,
    /// Below the anchor, left-aligned.
    #[default]
    Bottom,
    /// Left of the anchor, vertically centered.
    Left,
    /// Right of the anchor, vertically centered.
    Right,
}

/// Events a `Tooltip` emits. Tooltips are non-interactive; events exist
/// for parity with other primitives but rarely fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TooltipEvent {
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Measurement for a `Tooltip` — the content's natural size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TooltipMeasure {
    pub width: f32,
    pub height: f32,
}

impl TooltipMeasure {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// Resolved placement — what the layout actually chose (may differ
/// from `tooltip.placement` if the preferred direction overflowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedPlacement {
    Top,
    Bottom,
    Left,
    Right,
}

/// Classification of a hit-test result on a tooltip. Tooltips are
/// non-interactive, so hits just report "on tooltip" vs "outside."
/// Apps that want to pin the tooltip on click use this as the signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TooltipHit {
    Body(WidgetId),
    Empty,
}

/// Fully-resolved tooltip layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TooltipLayout {
    pub bounds: Rect,
    pub resolved_placement: ResolvedPlacement,
}

impl TooltipLayout {
    pub fn hit_test(&self, x: f32, y: f32, id: &WidgetId) -> TooltipHit {
        if x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height
        {
            TooltipHit::Body(id.clone())
        } else {
            TooltipHit::Empty
        }
    }
}

impl Tooltip {
    /// Compute tooltip placement.
    ///
    /// # Arguments
    ///
    /// - `anchor` — bounds of the element being described.
    /// - `viewport` — bounds of the parent surface; tooltip is clamped
    ///   to stay inside these.
    /// - `measure` — content width/height.
    /// - `margin` — gap between the anchor and the tooltip along the
    ///   placement axis.
    ///
    /// # Placement fallback
    ///
    /// The preferred placement is tried first. If it would push the
    /// tooltip past a viewport edge, the opposite side is tried. If
    /// both fail (unusual — anchor in the middle of a tiny viewport),
    /// the tooltip is pinned to the viewport edge on the preferred side.
    pub fn layout(
        &self,
        anchor: Rect,
        viewport: Rect,
        measure: TooltipMeasure,
        margin: f32,
    ) -> TooltipLayout {
        let vw = measure.width;
        let vh = measure.height;

        // Compute preferred x/y for each possible placement.
        let candidate = |p: TooltipPlacement| -> (f32, f32) {
            match p {
                TooltipPlacement::Top => {
                    (anchor.x + (anchor.width - vw) * 0.5, anchor.y - margin - vh)
                }
                TooltipPlacement::Bottom => (
                    anchor.x + (anchor.width - vw) * 0.5,
                    anchor.y + anchor.height + margin,
                ),
                TooltipPlacement::Left => (
                    anchor.x - margin - vw,
                    anchor.y + (anchor.height - vh) * 0.5,
                ),
                TooltipPlacement::Right => (
                    anchor.x + anchor.width + margin,
                    anchor.y + (anchor.height - vh) * 0.5,
                ),
            }
        };

        let fits = |x: f32, y: f32| -> bool {
            x >= viewport.x
                && x + vw <= viewport.x + viewport.width
                && y >= viewport.y
                && y + vh <= viewport.y + viewport.height
        };

        // Try preferred, then opposite, then clamp.
        let opposite = match self.placement {
            TooltipPlacement::Top => TooltipPlacement::Bottom,
            TooltipPlacement::Bottom => TooltipPlacement::Top,
            TooltipPlacement::Left => TooltipPlacement::Right,
            TooltipPlacement::Right => TooltipPlacement::Left,
        };

        let (x, y, resolved) = {
            let (px, py) = candidate(self.placement);
            if fits(px, py) {
                (px, py, self.placement)
            } else {
                let (ox, oy) = candidate(opposite);
                if fits(ox, oy) {
                    (ox, oy, opposite)
                } else {
                    // Fall back to preferred, clamped to viewport.
                    let cx = px.clamp(viewport.x, viewport.x + viewport.width - vw);
                    let cy = py.clamp(viewport.y, viewport.y + viewport.height - vh);
                    (cx, cy, self.placement)
                }
            }
        };

        let resolved_placement = match resolved {
            TooltipPlacement::Top => ResolvedPlacement::Top,
            TooltipPlacement::Bottom => ResolvedPlacement::Bottom,
            TooltipPlacement::Left => ResolvedPlacement::Left,
            TooltipPlacement::Right => ResolvedPlacement::Right,
        };

        TooltipLayout {
            bounds: Rect::new(x, y, vw, vh),
            resolved_placement,
        }
    }
}
