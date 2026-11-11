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

    /// The divider's click/drag target rect, in the same backend-native
    /// coordinate units as the rest of this layout.
    ///
    /// On backends that snap the click target to an integer grid (TUI's
    /// `tui_split_layout`, via `snap_divider_hit_region_to_painted_cell`),
    /// this differs from `divider_bounds`, which always keeps the
    /// unsnapped float position for hosts positioning pane content.
    /// Unaffected backends (GTK/win/macos pixel layouts) return the same
    /// rect as `divider_bounds`. Hosts that hit-test the divider by hand
    /// instead of calling [`SplitLayout::hit_test`] — e.g. to cache it
    /// across frames for drag start — must use this, not `divider_bounds`,
    /// or they can click a cell the divider isn't painted at.
    ///
    /// `hit_regions`'s first entry is always the divider (`Split::layout`
    /// and every snapping variant push it first, for hit-test specificity
    /// over the panes it sits between).
    pub fn divider_hit_bounds(&self) -> Rect {
        self.hit_regions[0].0
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
        // Both leaves share `self.id` — this transient tree is never
        // inspected by id (only `leaves[0]`/`leaves[1]`/`dividers[0]`,
        // read positionally below), so `SplitTree::leaf_ids()`'s
        // identity invariant doesn't apply here. Don't "fix" this by
        // minting distinct ids; `Split` has exactly one id to give both.
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
    use crate::Rect;

    /// Visual width of the painted sash line, in the same units as
    /// `divider_bounds` — VS Code's single-pixel resize affordance.
    /// [`SplitLayout::divider_bounds`]/[`SplitLayout::divider_hit_bounds`]
    /// stay the full [`super::SplitMeasure::divider_thickness`]-wide band
    /// (so a drag target is wider than one pixel); only the painted line
    /// is thinned to this and centred within that band. Mirrors
    /// [`crate::primitives::split_tree::native_surface_paint::SASH_PAINT_WIDTH`]
    /// so the two primitives read as the same visual weight.
    pub(crate) const SASH_PAINT_WIDTH: f32 = 1.0;

    /// Paint a [`SplitLayout`]'s divider onto `surface` as a filled
    /// rectangle in `theme.separator`. `layout` must be the same
    /// [`SplitLayout`] the caller uses for hit-testing (typically
    /// `Backend::split_layout`'s return value, or the value this fn's
    /// own caller — `Backend::draw_split` — returns) so paint and
    /// hit-test can never disagree. Pane content is NOT painted —
    /// every backend leaves `first_bounds`/`second_bounds` to the host,
    /// same contract as before this migration. The painted rect is
    /// [`SASH_PAINT_WIDTH`] wide, centred within `layout.divider_bounds`
    /// — narrower than the hit-testable band so the sash reads as a thin
    /// line while the drag target stays wide (see [`sash_rect`]).
    pub(crate) fn paint(layout: &SplitLayout, surface: &mut dyn PaintSurface, theme: &Theme) {
        surface.surface_fill_rect(sash_rect(layout.divider_bounds), theme.separator);
    }

    /// Centre a [`SASH_PAINT_WIDTH`]-thick line inside `divider_bounds`
    /// along whichever axis is the divider's thickness. [`super::Split::layout`]
    /// always builds `divider_bounds` with `divider_thickness` on one
    /// axis and the full cross-axis span (container width/height) on the
    /// other, so the thickness axis is reliably the shorter of the two —
    /// no need to thread `SplitDirection` through just for this.
    fn sash_rect(divider_bounds: Rect) -> Rect {
        if divider_bounds.width <= divider_bounds.height {
            let width = SASH_PAINT_WIDTH.min(divider_bounds.width.max(0.0));
            let x = centered_pixel_aligned(divider_bounds.x, divider_bounds.width, width);
            Rect::new(x, divider_bounds.y, width, divider_bounds.height)
        } else {
            let height = SASH_PAINT_WIDTH.min(divider_bounds.height.max(0.0));
            let y = centered_pixel_aligned(divider_bounds.y, divider_bounds.height, height);
            Rect::new(divider_bounds.x, y, divider_bounds.width, height)
        }
    }

    /// Resolve the leading edge of a `width`-wide line centred inside
    /// `[band_start, band_start + band_len)`, snapped to the nearest
    /// whole device pixel so a 1px sash renders as a crisp line rather
    /// than a blurred, anti-aliased straddle across two pixels — then
    /// clamped back into the band so the snap can never push the line
    /// outside the hit-testable area it's meant to sit inside.
    fn centered_pixel_aligned(band_start: f32, band_len: f32, width: f32) -> f32 {
        let center = band_start + band_len / 2.0;
        let aligned = (center - width / 2.0).round();
        aligned.clamp(band_start, (band_start + band_len - width).max(band_start))
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
        fn paints_a_one_px_sash_centred_inside_the_wider_hit_band() {
            let split = two_pane(SplitDirection::Horizontal);
            let layout = split.layout(QRect::new(0.0, 0.0, 200.0, 100.0), SplitMeasure::new(4.0));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&layout, &mut surface, &theme);

            assert_eq!(surface.fills.len(), 1, "split paints chrome only");
            let (painted, color) = surface.fills[0];
            assert_eq!(color, theme.separator);
            assert_eq!(painted.width, SASH_PAINT_WIDTH, "sash paints 1px wide");
            assert_eq!(painted.height, layout.divider_bounds.height);
            // The painted sash is centred inside — and so fully contained
            // within — the wider (4px) hit-testable band.
            assert!(painted.x >= layout.divider_bounds.x);
            assert!(
                painted.x + painted.width <= layout.divider_bounds.x + layout.divider_bounds.width
            );
            assert_eq!(
                layout.divider_hit_bounds().width,
                4.0,
                "hit target stays the full divider thickness"
            );
        }

        #[test]
        fn vertical_direction_paints_a_one_px_horizontal_sash() {
            let split = two_pane(SplitDirection::Vertical);
            let layout = split.layout(QRect::new(0.0, 0.0, 100.0, 200.0), SplitMeasure::new(4.0));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&layout, &mut surface, &theme);

            assert_eq!(surface.fills.len(), 1);
            let painted = surface.fills[0].0;
            assert_eq!(painted.height, SASH_PAINT_WIDTH, "sash paints 1px tall");
            assert_eq!(painted.width, layout.divider_bounds.width);
            assert!(painted.y >= layout.divider_bounds.y);
            assert!(
                painted.y + painted.height
                    <= layout.divider_bounds.y + layout.divider_bounds.height
            );
            assert_eq!(
                layout.divider_hit_bounds().height,
                4.0,
                "hit target stays the full divider thickness"
            );
        }

        #[test]
        fn non_zero_origin_divider_bounds_are_painted_absolute() {
            let split = two_pane(SplitDirection::Horizontal);
            let layout = split.layout(QRect::new(7.0, 13.0, 200.0, 100.0), SplitMeasure::new(4.0));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&layout, &mut surface, &theme);

            let painted = surface.fills[0].0;
            assert!(painted.x >= layout.divider_bounds.x);
            assert!(
                painted.x + painted.width <= layout.divider_bounds.x + layout.divider_bounds.width
            );
            assert!(painted.x >= 7.0);
        }

        #[test]
        fn sash_stays_no_wider_than_a_tiny_divider_band() {
            // When `divider_thickness` is itself below `SASH_PAINT_WIDTH`
            // (degenerate, but `SplitMeasure` doesn't forbid it), the sash
            // must not paint wider than the band it sits inside.
            let split = two_pane(SplitDirection::Horizontal);
            let layout = split.layout(QRect::new(0.0, 0.0, 200.0, 100.0), SplitMeasure::new(0.5));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&layout, &mut surface, &theme);

            let painted = surface.fills[0].0;
            assert!(painted.width <= layout.divider_bounds.width + 1e-6);
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
