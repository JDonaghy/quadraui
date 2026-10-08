//! `Canvas` primitive: app-defined drawing.
//!
//! Every other primitive in this crate is a closed shape — an app that
//! wants a diagram, a custom gauge, a sparkline with bespoke decoration,
//! or any pixel the 40 other shipped primitives don't already draw has no way
//! to put it on screen. `Canvas` is the escape hatch: a declarative list
//! of [`DrawOp`]s (rect, rounded rect, line, path, text run, image,
//! push/pop clip) that every backend paints through the same seam —
//! [`crate::paint_surface::PaintSurface`] for the three pixel backends,
//! and a dedicated degrade rasteriser (`crate::tui::canvas`) for TUI.
//!
//! # Coordinate frame
//!
//! `DrawOp` coordinates are **LOCAL** to the canvas: `(0, 0)` is the
//! top-left corner of whatever `rect` [`Backend::draw_canvas`]
//! (`crate::Backend`) was given, in that backend's own native units —
//! the same "native units" contract every other `Rect`/`Point` in this
//! crate already carries (see [`crate::event::Point`]'s own doc): pixels
//! on GTK/macOS/Win, cells on TUI. An app does not need to know which
//! backend is live to build a `Canvas` — it authors ops against
//! whatever unit the active backend's `canvas_layout`/`draw_canvas`
//! rect is already expressed in, exactly as it would size a `Terminal`
//! or `Editor` rect.
//!
//! # TUI story: degrade (design decision D-014)
//!
//! `Canvas` is not GUI-only — `docs/decisions/DECISIONS.md` D-014 records
//! the decision explicitly: TUI rasterises the same ops into cells
//! rather than reporting `Unsupported`, using the sub-cell braille
//! packing `tui::braille` already provides for [`crate::Chart`] and
//! [`crate::Minimap`]. The degrade is not uniform across every op kind —
//! each op picks the coarsest fidelity that still answers the "did the
//! user get the same information" bar D-014 sets, not a lower one:
//!
//! | Op | TUI degrade |
//! |---|---|
//! | [`DrawOp::Rect`] / [`DrawOp::RoundedRect`] / [`DrawOp::Line`] / [`DrawOp::Path`] | **braille** — sub-cell dots, same packing as `Chart`'s line charts. `RoundedRect`'s `radius` is dropped (no sub-cell arc); `Line`/`Path`'s `stroke_width` is ignored entirely (a braille dot has no width to vary); the fill itself still renders. |
//! | [`DrawOp::TextRun`] / [`DrawOp::Image`] | **cell-quantised** — text can't live inside a dot cell, so position snaps to the nearest whole cell. `Image` paints [`crate::Image::fallback_text`] the same way [`crate::Backend::draw_image`] already does for TUI, since there is still no pixel grid to decode bytes onto. A `TextRun`'s cell background always paints `theme.background`, never the colour of a shape underneath it — unlike a pixel backend, where `TextRun` paints transparently over whatever was already there. |
//! | [`DrawOp::PushClip`] / [`DrawOp::PopClip`] | **cell-quantised** — the clip rect's edges round outward to whole cells; shape ops inside it still paint at full braille resolution, just bounded to those cells. A clipped `TextRun` is shifted to start at the clip edge rather than trimmed, so the characters that would have fallen outside the clip are not dropped — they are shown at the clip edge instead of hidden. |
//!
//! No op is **N/A** — every one produces a real, typed answer the app
//! can see without branching on which backend is live, which is the
//! whole point of D-014's three-tier story.
//!
//! # What this is not
//!
//! `Canvas` carries no retained scene graph, no hit-testing of individual
//! ops, and no animation/timing. [`CanvasLayout::hit_test`] only answers
//! "did this click land inside the canvas's own bounds at all" — same
//! shape as [`crate::ImageHit`]'s `Image`/`Empty`. An app that needs to
//! know *which* op a click landed on keeps that mapping itself (it
//! authored the ops, so it already knows their coordinates) and keys off
//! [`CanvasHit::Inside`] only to decide whether to bother.

use crate::event::{Point, Rect};
use crate::primitives::image::Image;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of an app-drawn canvas: an ordered list of
/// [`DrawOp`]s, painted in order (later ops paint over earlier ones,
/// same "last writer wins" rule every backend's own native 2D API
/// already applies to overlapping draws).
///
/// # Examples
///
/// ```
/// use quadraui::{Canvas, CanvasHit, Color, DrawOp, Rect, WidgetId};
///
/// let canvas = Canvas {
///     id: WidgetId::new("canvas:sparkline"),
///     ops: vec![DrawOp::Rect {
///         rect: Rect::new(0.0, 0.0, 10.0, 4.0),
///         color: Color::rgb(0, 200, 0),
///     }],
/// };
///
/// let layout = canvas.layout(Rect::new(0.0, 0.0, 10.0, 4.0));
/// assert_eq!(layout.hit_test(5.0, 2.0), CanvasHit::Inside);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    pub id: WidgetId,
    pub ops: Vec<DrawOp>,
}

/// One drawing instruction. Coordinates are LOCAL to the canvas — see
/// the module doc's "Coordinate frame" section.
///
/// `#[non_exhaustive]`: per `PRIMITIVE_RULES.md` rule 8, a downstream
/// `match` needs a wildcard arm so a later op kind stays additive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DrawOp {
    /// Fill an axis-aligned rect with a solid colour.
    Rect { rect: Rect, color: Color },
    /// Fill a rect with rounded corners. `radius` is clamped the same
    /// way [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`]
    /// clamps it (half the shorter side, floored at `0.0`) — see that
    /// method's own doc. TUI drops the rounding (see module doc's
    /// degrade table) but still fills the rect.
    RoundedRect {
        rect: Rect,
        radius: f32,
        color: Color,
    },
    /// Stroke a single line segment.
    Line {
        from: Point,
        to: Point,
        color: Color,
        stroke_width: f32,
    },
    /// Stroke a polyline through `points`, in order. `closed` adds a
    /// final segment back from the last point to the first (so a
    /// triangle is three points with `closed: true`, not four with the
    /// first point repeated).
    Path {
        points: Vec<Point>,
        color: Color,
        stroke_width: f32,
        closed: bool,
    },
    /// Paint `text` at `rect`'s top-left corner in `color`, using the
    /// surface's current font. Single line — same contract as
    /// [`crate::paint_surface::PaintSurface::surface_draw_text_run`].
    TextRun {
        rect: Rect,
        text: String,
        color: Color,
    },
    /// Paint an image into `rect`, honouring `image.fit`. See
    /// [`crate::Image`] for the decode contract; TUI paints
    /// `image.fallback_text` instead (module doc's degrade table).
    Image { rect: Rect, image: Image },
    /// Push an axis-aligned clip rect. Should be balanced by a matching
    /// [`DrawOp::PopClip`] later in `ops`; every rasteriser degrades a
    /// malformed (unbalanced) push/pop list the same way, by construction
    /// rather than passing it through to the backend: an extra `PopClip`
    /// with no open `PushClip` is dropped instead of forwarded, and any
    /// `PushClip` still open once `ops` runs out is closed automatically
    /// before painting anything else. Either way the imbalance is
    /// contained to this `Canvas` and never reaches a pixel backend's own
    /// save/restore stack, where an unmatched pop (e.g. GTK's bare
    /// `cr.restore()`) would otherwise corrupt every draw painted after it
    /// for the rest of the frame.
    PushClip { rect: Rect },
    /// Pop the clip most recently pushed by [`DrawOp::PushClip`].
    PopClip,
}

/// Resolved geometry for a [`Canvas`] — just its own bounds, since
/// `DrawOp` coordinates are already LOCAL to them (module doc). A pure,
/// backend-independent function of the `rect` passed to
/// [`Canvas::layout`] — every backend's `Backend::canvas_layout` takes
/// the trait's default body for exactly this reason (no backend-specific
/// metric exists to add).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasLayout {
    pub bounds: Rect,
}

/// Hit-test classification for a click against a [`Canvas`]. `Canvas`
/// has no sub-regions of its own — see the module doc's "What this is
/// not" section — so this only answers "inside the canvas's own bounds
/// at all", the same shape as [`crate::ImageHit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasHit {
    Inside,
    Outside,
}

impl Canvas {
    /// Compute this canvas's layout within `bounds`. Pure geometry — see
    /// [`CanvasLayout`]'s own doc for why there is nothing more to
    /// resolve.
    pub fn layout(&self, bounds: Rect) -> CanvasLayout {
        CanvasLayout { bounds }
    }
}

impl CanvasLayout {
    /// Hit-test a click at `(x, y)` against [`Self::bounds`].
    ///
    /// Coordinate frame: **ABSOLUTE** — same frame as [`Self::bounds`],
    /// which already carries the `bounds` rect passed to
    /// [`Canvas::layout`].
    pub fn hit_test(&self, x: f32, y: f32) -> CanvasHit {
        let b = self.bounds;
        if x >= b.x && x < b.x + b.width && y >= b.y && y < b.y + b.height {
            CanvasHit::Inside
        } else {
            CanvasHit::Outside
        }
    }
}

// ── PaintSurface paint ──────────────────────────────────────────────────
//
// Every pixel backend (GTK/macOS/Win) paints a `Canvas` identically: walk
// `ops` in order, translate each op's LOCAL coordinates by the layout's
// own origin, and call the matching `PaintSurface` verb. There is no
// backend-specific behaviour here at all — unlike `Panel`/`Toast`, which
// resolve theme colours and chrome metrics, `Canvas`'s ops already carry
// every colour/geometry value the app wants, so this is a pure
// passthrough rather than a shared *policy*. `Path` has no dedicated
// `PaintSurface` verb (the trait's verbs don't include one, by design),
// so it lowers to one `surface_draw_line` call per segment —
// indistinguishable on screen from a real path stroke for the
// polylines `Canvas` targets, and avoids growing `PaintSurface` for a
// shape `surface_draw_line` already composes.
//
// `#[allow(dead_code)]`: see `primitives::panel`'s identical note
// — only *called* once a real pixel backend is compiled in, exercised by
// each backend's own `Backend::draw_canvas` call site plus this module's
// own `RecordingSurface` tests on every leg that enables one of the
// three cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Canvas, CanvasLayout, DrawOp};
    use crate::event::{Point, Rect};
    use crate::paint_surface::PaintSurface;

    fn translate_rect(r: Rect, dx: f32, dy: f32) -> Rect {
        Rect::new(r.x + dx, r.y + dy, r.width, r.height)
    }

    fn translate_point(p: Point, dx: f32, dy: f32) -> Point {
        Point::new(p.x + dx, p.y + dy)
    }

    /// Paint `canvas`'s `ops` onto `surface`, translated into
    /// `layout.bounds`'s own origin. See this module's own doc for why
    /// every pixel backend shares this one implementation.
    pub(crate) fn paint(canvas: &Canvas, layout: &CanvasLayout, surface: &mut dyn PaintSurface) {
        let dx = layout.bounds.x;
        let dy = layout.bounds.y;
        // Tracks how many `PushClip`s are currently open so a malformed
        // `ops` list (app data is `Deserialize`, so this is one bad field
        // away) can never hand `surface` an unbalanced push/pop pair. Every
        // pixel backend's `surface_pop_clip` assumes a matching prior
        // `surface_push_clip` ([`crate::paint_surface::PaintSurface`]'s own
        // doc) — GTK's is a bare `cr.restore()` with no matching `save()`,
        // which leaves the whole `cairo::Context` in a permanent error
        // state for the rest of the frame, not just this canvas. A stray
        // `PopClip` with no open push is dropped instead of forwarded, and
        // any push still open when `ops` runs out is popped here before
        // returning, matching `tui::canvas`'s own `clip_per_op` guard
        // (`stack.len() > 1`) so both rasterisers degrade a malformed op
        // list the same way.
        let mut clip_depth: u32 = 0;
        for op in &canvas.ops {
            match op {
                DrawOp::Rect { rect, color } => {
                    surface.surface_fill_rect(translate_rect(*rect, dx, dy), *color);
                }
                DrawOp::RoundedRect {
                    rect,
                    radius,
                    color,
                } => {
                    surface.surface_fill_rounded_rect(
                        translate_rect(*rect, dx, dy),
                        *radius,
                        *color,
                    );
                }
                DrawOp::Line {
                    from,
                    to,
                    color,
                    stroke_width,
                } => {
                    surface.surface_draw_line(
                        translate_point(*from, dx, dy),
                        translate_point(*to, dx, dy),
                        *color,
                        *stroke_width,
                    );
                }
                DrawOp::Path {
                    points,
                    color,
                    stroke_width,
                    closed,
                } => {
                    for pair in points.windows(2) {
                        surface.surface_draw_line(
                            translate_point(pair[0], dx, dy),
                            translate_point(pair[1], dx, dy),
                            *color,
                            *stroke_width,
                        );
                    }
                    if *closed && points.len() > 1 {
                        let first = points[0];
                        let last = points[points.len() - 1];
                        surface.surface_draw_line(
                            translate_point(last, dx, dy),
                            translate_point(first, dx, dy),
                            *color,
                            *stroke_width,
                        );
                    }
                }
                DrawOp::TextRun { rect, text, color } => {
                    surface.surface_draw_text_run(translate_rect(*rect, dx, dy), text, *color);
                }
                DrawOp::Image { rect, image } => {
                    let _ = surface.surface_draw_image(translate_rect(*rect, dx, dy), image);
                }
                DrawOp::PushClip { rect } => {
                    surface.surface_push_clip(translate_rect(*rect, dx, dy));
                    clip_depth += 1;
                }
                DrawOp::PopClip => {
                    if clip_depth > 0 {
                        surface.surface_pop_clip();
                        clip_depth -= 1;
                    }
                }
            }
        }
        // Close out any push left open by a malformed `ops` list so the
        // imbalance can't leak past this canvas into the rest of the frame.
        for _ in 0..clip_depth {
            surface.surface_pop_clip();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::image::{Image, ImageFit, ImageSource};
        use crate::types::{Color, WidgetId};

        /// Records every surface verb `paint` uses — mirrors
        /// `primitives::panel`'s `RecordingSurface` test double, so this
        /// test runs on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            rounded_fills: Vec<(Rect, f32, Color)>,
            lines: Vec<(Point, Point, Color, f32)>,
            text_runs: Vec<(Rect, String, Color)>,
            images: Vec<Rect>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
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
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
                self.rounded_fills.push((rect, radius, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                from: Point,
                to: Point,
                color: Color,
                stroke_width: f32,
            ) {
                self.lines.push((from, to, color, stroke_width));
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clip_pushes.push(rect);
            }
            fn surface_pop_clip(&mut self) {
                self.clip_pops += 1;
            }
            fn surface_draw_image(&mut self, rect: Rect, _image: &Image) -> ImagePaintResult {
                self.images.push(rect);
                ImagePaintResult::Painted
            }
        }

        fn canvas(ops: Vec<DrawOp>) -> Canvas {
            Canvas {
                id: WidgetId::new("canvas"),
                ops,
            }
        }

        #[test]
        fn rect_op_translates_by_the_layout_origin() {
            let c = canvas(vec![DrawOp::Rect {
                rect: Rect::new(2.0, 3.0, 10.0, 5.0),
                color: Color::rgb(255, 0, 0),
            }]);
            let layout = c.layout(Rect::new(7.0, 11.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(
                surface.fills,
                vec![(Rect::new(9.0, 14.0, 10.0, 5.0), Color::rgb(255, 0, 0))]
            );
        }

        #[test]
        fn rounded_rect_op_forwards_radius() {
            let c = canvas(vec![DrawOp::RoundedRect {
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                radius: 4.0,
                color: Color::rgb(0, 255, 0),
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(surface.rounded_fills.len(), 1);
            assert_eq!(surface.rounded_fills[0].1, 4.0);
        }

        #[test]
        fn line_op_translates_both_endpoints() {
            let c = canvas(vec![DrawOp::Line {
                from: Point::new(0.0, 0.0),
                to: Point::new(10.0, 10.0),
                color: Color::rgb(0, 0, 255),
                stroke_width: 2.0,
            }]);
            let layout = c.layout(Rect::new(5.0, 5.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(
                surface.lines,
                vec![(
                    Point::new(5.0, 5.0),
                    Point::new(15.0, 15.0),
                    Color::rgb(0, 0, 255),
                    2.0
                )]
            );
        }

        #[test]
        fn open_path_draws_one_line_per_consecutive_pair() {
            let c = canvas(vec![DrawOp::Path {
                points: vec![
                    Point::new(0.0, 0.0),
                    Point::new(5.0, 0.0),
                    Point::new(5.0, 5.0),
                ],
                color: Color::rgb(1, 2, 3),
                stroke_width: 1.0,
                closed: false,
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            // Open path: n points -> n-1 segments, no closing segment back
            // to the first point.
            assert_eq!(surface.lines.len(), 2);
        }

        #[test]
        fn closed_path_adds_the_final_segment_back_to_the_first_point() {
            let c = canvas(vec![DrawOp::Path {
                points: vec![
                    Point::new(0.0, 0.0),
                    Point::new(5.0, 0.0),
                    Point::new(5.0, 5.0),
                ],
                color: Color::rgb(1, 2, 3),
                stroke_width: 1.0,
                closed: true,
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            // Closed path: n points -> n segments (one extra, back to start).
            assert_eq!(surface.lines.len(), 3);
            let (from, to, ..) = surface.lines[2];
            assert_eq!(from, Point::new(5.0, 5.0));
            assert_eq!(to, Point::new(0.0, 0.0));
        }

        #[test]
        fn text_run_op_translates_rect_and_keeps_the_text() {
            let c = canvas(vec![DrawOp::TextRun {
                rect: Rect::new(1.0, 1.0, 40.0, 16.0),
                text: "hello canvas".into(),
                color: Color::rgb(10, 20, 30),
            }]);
            let layout = c.layout(Rect::new(100.0, 200.0, 300.0, 300.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(surface.text_runs.len(), 1);
            assert_eq!(surface.text_runs[0].0, Rect::new(101.0, 201.0, 40.0, 16.0));
            assert_eq!(surface.text_runs[0].1, "hello canvas");
        }

        #[test]
        fn image_op_paints_through_the_surface() {
            let c = canvas(vec![DrawOp::Image {
                rect: Rect::new(0.0, 0.0, 32.0, 32.0),
                image: Image {
                    id: WidgetId::new("img"),
                    source: ImageSource::Path("/tmp/x.png".into()),
                    intrinsic_size: Some((32, 32)),
                    fit: ImageFit::Contain,
                    fallback_text: "[X]".into(),
                },
            }]);
            let layout = c.layout(Rect::new(10.0, 10.0, 100.0, 100.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(surface.images, vec![Rect::new(10.0, 10.0, 32.0, 32.0)]);
        }

        #[test]
        fn push_and_pop_clip_both_reach_the_surface() {
            let c = canvas(vec![
                DrawOp::PushClip {
                    rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                },
                DrawOp::Rect {
                    rect: Rect::new(0.0, 0.0, 5.0, 5.0),
                    color: Color::rgb(9, 9, 9),
                },
                DrawOp::PopClip,
            ]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(surface.clip_pushes.len(), 1);
            assert_eq!(surface.clip_pops, 1);
        }

        #[test]
        fn extra_pop_clip_with_no_open_push_is_dropped_not_forwarded() {
            // A `PopClip` with nothing open on the stack must never reach
            // the surface: on GTK that would be a bare `cr.restore()` with
            // no matching `cr.save()`, which corrupts the whole Cairo
            // context for the rest of the frame.
            let c = canvas(vec![
                DrawOp::PopClip,
                DrawOp::Rect {
                    rect: Rect::new(0.0, 0.0, 5.0, 5.0),
                    color: Color::rgb(9, 9, 9),
                },
            ]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(surface.clip_pops, 0);
            // The rest of the op list still paints normally.
            assert_eq!(surface.fills.len(), 1);
        }

        #[test]
        fn unclosed_push_clip_is_popped_once_ops_run_out() {
            // Two opens, one explicit close: the still-open push must be
            // closed automatically rather than left on the surface's clip
            // stack for whatever paints after this canvas.
            let c = canvas(vec![
                DrawOp::PushClip {
                    rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                },
                DrawOp::PushClip {
                    rect: Rect::new(1.0, 1.0, 5.0, 5.0),
                },
                DrawOp::PopClip,
            ]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(surface.clip_pushes.len(), 2);
            // One explicit pop plus one synthesised to close the leftover
            // push: every push this canvas made is balanced by a pop.
            assert_eq!(surface.clip_pops, 2);
        }

        #[test]
        fn ops_paint_in_declared_order() {
            // "Last writer wins" (module doc): two overlapping rects at
            // the same spot must be recorded in the order they were
            // declared, not reordered — a consumer diffing paint order
            // against expectation relies on this.
            let c = canvas(vec![
                DrawOp::Rect {
                    rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                    color: Color::rgb(1, 1, 1),
                },
                DrawOp::Rect {
                    rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                    color: Color::rgb(2, 2, 2),
                },
            ]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(surface.fills[0].1, Color::rgb(1, 1, 1));
            assert_eq!(surface.fills[1].1, Color::rgb(2, 2, 2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_returns_bounds_unchanged() {
        let c = Canvas {
            id: WidgetId::new("c"),
            ops: vec![],
        };
        let bounds = Rect::new(3.0, 4.0, 20.0, 10.0);
        let layout = c.layout(bounds);
        assert_eq!(layout.bounds, bounds);
    }

    #[test]
    fn hit_test_inside_bounds_is_inside() {
        let c = Canvas {
            id: WidgetId::new("c"),
            ops: vec![],
        };
        let layout = c.layout(Rect::new(5.0, 5.0, 10.0, 10.0));
        assert_eq!(layout.hit_test(7.0, 7.0), CanvasHit::Inside);
    }

    #[test]
    fn hit_test_outside_bounds_is_outside() {
        let c = Canvas {
            id: WidgetId::new("c"),
            ops: vec![],
        };
        let layout = c.layout(Rect::new(5.0, 5.0, 10.0, 10.0));
        assert_eq!(layout.hit_test(0.0, 0.0), CanvasHit::Outside);
    }

    #[test]
    fn hit_test_respects_a_non_zero_origin() {
        // LESSONS.md regression shape: (0, 0) is exactly the
        // case where a frame mixup is invisible.
        let c = Canvas {
            id: WidgetId::new("c"),
            ops: vec![],
        };
        let layout = c.layout(Rect::new(100.0, 200.0, 10.0, 10.0));
        assert_eq!(layout.hit_test(105.0, 205.0), CanvasHit::Inside);
        assert_eq!(layout.hit_test(5.0, 5.0), CanvasHit::Outside);
    }

    #[test]
    fn non_exhaustive_draw_op_round_trips_through_serde() {
        let op = DrawOp::Rect {
            rect: Rect::new(1.0, 2.0, 3.0, 4.0),
            color: Color::rgb(10, 20, 30),
        };
        let json = serde_json::to_string(&op).expect("serialize");
        let back: DrawOp = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(op, back);
    }
}
