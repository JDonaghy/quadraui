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
use crate::types::WidgetId;
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

// ── NativeSurface paint (#864, Phase 2d slice 7/9 of the NativeSurface
// milestone, child of #811) ────────────────────────────────────────────
//
// Before this, `gtk::draw_split` (Cairo), `macos::split::draw_split`
// (Core Graphics) and `win::split::draw_split` (Direct2D) each
// independently painted the same divider-only chrome with their own
// drawing API (quadraui#785 child #811, `docs/SMELL_AUDIT_2026-07.md`
// §5). `paint` below is the one shared implementation, written against
// [`crate::native_surface::NativeSurface`] (#807, Phase 1) instead of
// any one backend's drawing API.
//
// Re-verified while migrating, per this issue's "re-verify before you
// implement": all three deleted copies painted exactly one filled
// rectangle — `layout.divider_bounds` in the opaque `theme.separator`
// colour — and nothing else (pane content is explicitly the host's
// job, not the rasteriser's, on every backend). GTK used `set_source`
// (opaque, no alpha channel); macOS's `CGContextSetRGBFillColor` and
// Windows's `ID2D1SolidColorBrush` both pass a real alpha channel, but
// since `theme.separator` is always fully opaque (`Color::rgb`, not
// `rgba`) the three are pixel-identical. **No divergence found** — same
// conclusion as `primitives::split_tree`'s identical migration (#863,
// slice 6/9), which shares this exact divider-fill shape.
//
// One difference worth naming, though it isn't a paint divergence: the
// pre-migration `win::split::draw_split` always painted with
// `Theme::default()` rather than any live theme (`WinBackend` doesn't
// carry one through to chrome rasterisers yet — see that module's old
// "# Theme" doc section, mirrored now in `win::split`'s module doc).
// `Backend::draw_split`'s Windows arm preserves that by passing
// `Theme::default()` to this fn explicitly, same as `draw_split_tree`'s
// migration did.
//
// Unlike `draw_scrollbar`/`draw_panel`, geometry (pane rects + divider
// placement) was ALREADY the single shared [`Split::layout`] before
// this issue — only the *paint* half (filling `divider_bounds`) was
// triplicated. `paint` therefore takes an already-resolved
// [`SplitLayout`] (the same convention as
// `primitives::split_tree::native_surface_paint::paint`), not the raw
// `Split` + bounds — callers compute the layout once and reuse it for
// both paint and hit-test.
//
// `#[allow(dead_code)]`: see `primitives::form`'s identical note (#808)
// — only *called* once a real pixel backend is compiled in, exercised
// by each backend's own `Backend::draw_split` call site plus this
// module's own `RecordingSurface` tests on every leg that enables one
// of the three cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::SplitLayout;
    use crate::native_surface::NativeSurface;
    use crate::theme::Theme;

    /// Paint a [`SplitLayout`]'s divider onto `surface` as a filled
    /// rectangle in `theme.separator`. `layout` must be the same
    /// [`SplitLayout`] the caller uses for hit-testing (typically
    /// `Backend::split_layout`'s return value, or the value this fn's
    /// own caller — `Backend::draw_split` — returns) so paint and
    /// hit-test can never disagree. Pane content is NOT painted —
    /// every backend leaves `first_bounds`/`second_bounds` to the host,
    /// same contract as before this migration.
    pub(crate) fn paint(layout: &SplitLayout, surface: &mut dyn NativeSurface, theme: &Theme) {
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

        impl NativeSurface for RecordingSurface {
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
            fn surface_stroke_rect(&mut self, _rect: QRect, _color: Color, _stroke_width: f32) {}
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
