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
//! `divider_bounds` and hit-tests it for drag operations via
//! [`SplitLayout::hit_test`] / [`SplitHit`]. When the user drags, the
//! app updates `ratio` on the primitive for the next frame. Clicks on
//! either pane resolve as `SplitHit::FirstPane` / `SplitHit::SecondPane`.

use crate::event::Rect;
use crate::primitives::split_tree::{SplitTree, SplitTreeMeasure};
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Declarative description of a split container.
///
/// # Examples
///
/// ```
/// use quadraui::{Rect, Split, SplitDirection, SplitHit, SplitMeasure, WidgetId};
///
/// let split = Split {
///     id: WidgetId::new("split:editor"),
///     direction: SplitDirection::Horizontal,
///     ratio: 0.3,
///     first_min: 0.0,
///     second_min: 0.0,
/// };
///
/// let bounds = Rect::new(0.0, 0.0, 100.0, 40.0);
/// let layout = split.layout(bounds, SplitMeasure::new(1.0));
///
/// assert_eq!(layout.hit_test(0.0, 0.0), SplitHit::FirstPane(split.id.clone()));
/// assert_eq!(layout.hit_test(99.0, 0.0), SplitHit::SecondPane(split.id));
/// ```
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
    /// space). This is a thin wrapper over
    /// [`SplitTree::layout`] — `Split` is exactly a two-leaf
    /// `SplitTree::Split`, so the two primitives share one relax
    /// formula and can never drift apart.
    pub fn layout(&self, bounds: Rect, measure: SplitMeasure) -> SplitLayout {
        let tree = SplitTree::split(
            self.direction,
            self.ratio,
            SplitTree::leaf(self.id),
            SplitTree::leaf(self.id),
        )
        .with_min_sizes(self.first_min, self.second_min);
        let tl = tree.layout(bounds, SplitTreeMeasure::new(measure.divider_thickness));

        let first_bounds = tl.leaves[0].1;
        let second_bounds = tl.leaves[1].1;
        let d = tl.dividers[0];
        let divider_bounds = match self.direction {
            SplitDirection::Horizontal => {
                Rect::new(d.position, d.cross_start, d.thickness, d.cross_size)
            }
            SplitDirection::Vertical => {
                Rect::new(d.cross_start, d.position, d.cross_size, d.thickness)
            }
        };

        let available = (d.axis_size - d.thickness).max(0.0);
        let resolved_ratio = if available > 0.0 {
            (d.position - d.axis_start) / available
        } else {
            self.ratio.clamp(0.0, 1.0)
        };

        // Hit regions: divider first (specificity), then panes.
        let hit_regions: Vec<(Rect, SplitHit)> = vec![
            (divider_bounds, SplitHit::Divider(self.id)),
            (first_bounds, SplitHit::FirstPane(self.id)),
            (second_bounds, SplitHit::SecondPane(self.id)),
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

// Unified paint implementation for split dividers, replacing
// three independent backend-specific versions (Cairo, Core Graphics,
// Direct2D). Each originally painted only `layout.divider_bounds` in
// `theme.separator` with that backend's drawing API.
//
// All three painted exactly one filled rectangle and nothing else
// (pane content is the host's job, not the rasteriser's).
//
// **Note: GTK alpha handling changed.** The old `gtk::draw_split`
// used `set_source` (drops alpha), while macOS's `CGContextSetRGBFillColor`
// and Windows's `ID2D1SolidColorBrush` honour real alpha. This code uses
// `surface_fill_rect`, which calls `gtk::set_source_rgba` on GTK, so GTK
// now respects alpha in translucent `theme.separator` values (which can
// occur when `Theme::from_vscode_json` parses an 8-digit `#rrggbbaa` VS Code
// `editorGroup.border` value). This brings GTK in line with macOS/Windows.
// The deprecated free-function shim keeps calling opaque `set_source` for
// backwards compatibility.
//
// Windows: `Backend::draw_split` passes `Theme::default()` since
// `WinBackend` does not carry a live theme to chrome rasterisers.
//
// Geometry (split layout, divider placement) was already unified in
// [`Split::layout`] before this change; only the *paint* half was
// triplicated. `paint` takes the resolved [`SplitLayout`], not the raw
// `Split` + bounds, so callers compute layout once and reuse it for both
// paint and hit-test.
//
// `#[allow(dead_code)]`: only called when a pixel backend is compiled in,
// exercised by each backend's `Backend::draw_split` and this module's
// `RecordingSurface` tests.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::SplitLayout;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a [`SplitLayout`]'s divider onto `surface` as a filled
    /// rectangle in `theme.separator`. `layout` must be the same
    /// [`SplitLayout`] the caller uses for hit-testing (typically
    /// `Backend::split_layout`'s return value, or the value this fn's
    /// own caller — `Backend::draw_split` — returns) so paint and
    /// hit-test can never disagree. Pane content is NOT painted —
    /// every backend leaves `first_bounds`/`second_bounds` to the host,
    /// same contract as before this migration.
    pub(crate) fn paint(layout: &SplitLayout, surface: &mut dyn PaintSurface, theme: &Theme) {
        surface.surface_fill_rect(layout.divider_bounds, theme.separator);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Rect as QRect, Viewport};
        use crate::primitives::split::{Split, SplitDirection, SplitMeasure};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every `surface_fill_rect` call — mirrors
        /// `primitives::split_tree`'s `RecordingSurface` test double,
        /// scoped to just the verb this primitive uses, so this test
        /// runs on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(QRect, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
                (0.0, 0.0)
            }
            fn surface_fill_rect(&mut self, rect: QRect, color: Color) {
                self.fills.push((rect, color));
            }
            /// Test-only recorder — `paint` never calls this verb
            /// (see this module's own doc for why no primitive here has been
            /// migrated onto it yet); this exists only so `RecordingSurface`
            /// satisfies the trait. Records into the same `fills` list as
            /// `surface_fill_rect` (radius dropped) — no test asserts on it
            /// today.
            fn surface_fill_rounded_rect(&mut self, rect: QRect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: QRect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: QRect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, _rect: QRect, _text: &str, _color: Color) {}
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: QRect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: QRect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn two_pane(direction: SplitDirection) -> Split {
            Split {
                id: WidgetId::new("s"),
                direction,
                ratio: 0.5,
                first_min: 0.0,
                second_min: 0.0,
            }
        }

        #[test]
        fn paints_exactly_the_divider_rect_in_theme_separator() {
            let split = two_pane(SplitDirection::Horizontal);
            let layout = split.layout(QRect::new(0.0, 0.0, 200.0, 100.0), SplitMeasure::new(4.0));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&layout, &mut surface, &theme);

            assert_eq!(surface.fills.len(), 1, "split paints chrome only");
            assert_eq!(surface.fills[0], (layout.divider_bounds, theme.separator));
        }

        #[test]
        fn vertical_direction_paints_the_same_divider_bounds() {
            let split = two_pane(SplitDirection::Vertical);
            let layout = split.layout(QRect::new(0.0, 0.0, 100.0, 200.0), SplitMeasure::new(4.0));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&layout, &mut surface, &theme);

            assert_eq!(surface.fills.len(), 1);
            assert_eq!(surface.fills[0].0, layout.divider_bounds);
        }

        #[test]
        fn non_zero_origin_divider_bounds_are_painted_absolute() {
            let split = two_pane(SplitDirection::Horizontal);
            let layout = split.layout(QRect::new(7.0, 13.0, 200.0, 100.0), SplitMeasure::new(4.0));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&layout, &mut surface, &theme);

            assert_eq!(surface.fills[0].0, layout.divider_bounds);
            assert!(surface.fills[0].0.x >= 7.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Split primitive tests ─────────────────────────────────────────

    #[test]
    fn split_layout_horizontal_even() {
        let s = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let bounds = Rect::new(0.0, 0.0, 202.0, 100.0);
        let layout = s.layout(bounds, SplitMeasure::new(2.0));
        // available = 200, first = 100, second = 100.
        assert_eq!(layout.first_bounds.width, 100.0);
        assert_eq!(layout.divider_bounds.x, 100.0);
        assert_eq!(layout.divider_bounds.width, 2.0);
        assert_eq!(layout.second_bounds.x, 102.0);
        assert_eq!(layout.second_bounds.width, 100.0);
    }

    #[test]
    fn split_layout_vertical_with_min() {
        let s = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Vertical,
            ratio: 0.1, // too small; clamped up to first_min
            first_min: 30.0,
            second_min: 20.0,
        };
        let bounds = Rect::new(0.0, 0.0, 100.0, 101.0);
        let layout = s.layout(bounds, SplitMeasure::new(1.0));
        // available = 100. Raw first = 10; clamped to first_min = 30.
        assert_eq!(layout.first_bounds.height, 30.0);
        assert_eq!(layout.second_bounds.height, 70.0);
        assert_eq!(layout.divider_bounds.y, 30.0);
    }

    #[test]
    fn split_hit_test_regions() {
        let s = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
        let layout = s.layout(bounds, SplitMeasure::new(2.0));
        // Click in first pane.
        match layout.hit_test(50.0, 50.0) {
            SplitHit::FirstPane(id) => assert_eq!(id.as_str(), "s"),
            _ => panic!(),
        }
        // Click on divider (x = 99.0..101.0).
        match layout.hit_test(99.5, 50.0) {
            SplitHit::Divider(id) => assert_eq!(id.as_str(), "s"),
            _ => panic!(),
        }
        // Click in second pane.
        match layout.hit_test(150.0, 50.0) {
            SplitHit::SecondPane(id) => assert_eq!(id.as_str(), "s"),
            _ => panic!(),
        }
    }

    #[test]
    fn split_layout_resolved_ratio() {
        let s = Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.0, // would collapse first pane
            first_min: 50.0,
            second_min: 0.0,
        };
        let bounds = Rect::new(0.0, 0.0, 201.0, 100.0);
        let layout = s.layout(bounds, SplitMeasure::new(1.0));
        // first_size clamped to 50; resolved_ratio = 50/200 = 0.25.
        assert!((layout.resolved_ratio - 0.25).abs() < 0.001);
    }
}
