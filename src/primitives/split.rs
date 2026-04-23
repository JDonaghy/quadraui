//! `Split` primitive: a two-pane container with a draggable divider.
//! Used for editor-and-sidebar layouts, diff views, horizontal +
//! vertical window splits, and anywhere a resizable boundary between
//! two regions is needed.
//!
//! Like `Panel`, `Split` describes the frame (divider position + pane
//! rectangles) but doesn't hold pane content. Apps draw their content
//! into `first_bounds` and `second_bounds`.
//!
//! # Backend contract
//!
//! **Declarative + draggable.** The backend renders the divider at
//! `divider_bounds` and hit-tests it for drag operations. When the
//! user drags, the backend emits `DividerDragged { new_ratio }`; the
//! app updates `ratio` on the primitive for the next frame. Clicks on
//! either pane emit `PaneClicked { idx }`.

use crate::event::Rect;
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a split container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Split {
    pub id: WidgetId,
    pub direction: SplitDirection,
    /// Divider position as a fraction of the container's cross-axis
    /// length (0.0..=1.0). Clamped to a sensible range in `layout()`
    /// to keep both panes visible.
    pub ratio: f32,
    /// Minimum size of the first pane in the backend's native unit.
    /// `0.0` = no minimum.
    #[serde(default)]
    pub first_min: f32,
    /// Minimum size of the second pane. `0.0` = no minimum.
    #[serde(default)]
    pub second_min: f32,
}

/// Orientation of a `Split`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    /// Divider runs vertically; panes are side-by-side (first = left,
    /// second = right).
    Horizontal,
    /// Divider runs horizontally; panes are stacked (first = top,
    /// second = bottom).
    Vertical,
}

/// Events a `Split` emits back to the app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SplitEvent {
    /// User dragged the divider. `new_ratio` is the app-clamped,
    /// ready-to-store value.
    DividerDragged { new_ratio: f32 },
    /// User double-clicked the divider (apps typically reset to 0.5 or
    /// invoke a smart layout).
    DividerDoubleClicked,
    /// User clicked inside one of the panes. `idx` is 0 = first, 1 = second.
    PaneClicked { idx: u8 },
    /// Key pressed while the split had focus.
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Divider dimensions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitMeasure {
    /// Thickness of the divider along the cross-axis (e.g. 1 char cell
    /// in TUI, 4–6 px in GTK).
    pub divider_thickness: f32,
}

impl SplitMeasure {
    pub fn new(divider_thickness: f32) -> Self {
        Self { divider_thickness }
    }
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitHit {
    /// Click landed on the first pane.
    FirstPane(WidgetId),
    /// Click landed on the second pane.
    SecondPane(WidgetId),
    /// Click landed on the divider (start of a drag operation).
    Divider(WidgetId),
    /// Click landed outside the split.
    Outside,
}

/// Fully-resolved split layout.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitLayout {
    pub bounds: Rect,
    pub first_bounds: Rect,
    pub divider_bounds: Rect,
    pub second_bounds: Rect,
    pub hit_regions: Vec<(Rect, SplitHit)>,
    /// Ratio actually used (may differ from input if clamped by the
    /// min-size constraints). Apps should write this back so the next
    /// frame starts coherent.
    pub resolved_ratio: f32,
}

impl SplitLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> SplitHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        SplitHit::Outside
    }
}

impl Split {
    /// Compute pane + divider bounds.
    ///
    /// # Arguments
    ///
    /// - `bounds` — container region.
    /// - `measure` — divider thickness.
    ///
    /// # Ratio clamping
    ///
    /// The input `ratio` is clamped so both panes honour their
    /// respective `first_min` / `second_min`. If the container is too
    /// small to satisfy both minimums, the minimums are relaxed
    /// proportionally (first_min and second_min split the available
    /// space).
    pub fn layout(&self, bounds: Rect, measure: SplitMeasure) -> SplitLayout {
        let (total, cross_start) = match self.direction {
            SplitDirection::Horizontal => (bounds.width, bounds.x),
            SplitDirection::Vertical => (bounds.height, bounds.y),
        };
        let available = (total - measure.divider_thickness).max(0.0);

        // Clamp ratio to keep both panes above their minimums.
        let clamped = self.ratio.clamp(0.0, 1.0);
        let first_size_raw = available * clamped;

        let (first_size, second_size) = if self.first_min + self.second_min <= available {
            let fs = first_size_raw.max(self.first_min);
            let fs = fs.min(available - self.second_min);
            (fs, available - fs)
        } else if available > 0.0 {
            // Minimums don't fit — split proportionally to the mins.
            let total_min = self.first_min + self.second_min;
            let f = available * (self.first_min / total_min);
            (f, available - f)
        } else {
            (0.0, 0.0)
        };

        let resolved_ratio = if available > 0.0 {
            first_size / available
        } else {
            clamped
        };

        let (first_bounds, divider_bounds, second_bounds) = match self.direction {
            SplitDirection::Horizontal => (
                Rect::new(cross_start, bounds.y, first_size, bounds.height),
                Rect::new(
                    cross_start + first_size,
                    bounds.y,
                    measure.divider_thickness,
                    bounds.height,
                ),
                Rect::new(
                    cross_start + first_size + measure.divider_thickness,
                    bounds.y,
                    second_size,
                    bounds.height,
                ),
            ),
            SplitDirection::Vertical => (
                Rect::new(bounds.x, cross_start, bounds.width, first_size),
                Rect::new(
                    bounds.x,
                    cross_start + first_size,
                    bounds.width,
                    measure.divider_thickness,
                ),
                Rect::new(
                    bounds.x,
                    cross_start + first_size + measure.divider_thickness,
                    bounds.width,
                    second_size,
                ),
            ),
        };

        // Hit regions: divider first (specificity), then panes.
        let hit_regions: Vec<(Rect, SplitHit)> = vec![
            (divider_bounds, SplitHit::Divider(self.id.clone())),
            (first_bounds, SplitHit::FirstPane(self.id.clone())),
            (second_bounds, SplitHit::SecondPane(self.id.clone())),
        ];

        SplitLayout {
            bounds,
            first_bounds,
            divider_bounds,
            second_bounds,
            hit_regions,
            resolved_ratio,
        }
    }
}
