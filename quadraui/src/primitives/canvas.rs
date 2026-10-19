//! `Canvas` primitive: app-defined drawing.
//!
//! Every other primitive in this crate is a closed shape — an app that
//! wants a diagram, a custom gauge, a sparkline with bespoke decoration,
//! or any pixel the 48 other shipped primitives don't already draw has no way
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
//! | [`DrawOp::Polygon`] / [`DrawOp::Ellipse`] | **braille** — filled via the same even-odd scanline fill every pixel backend uses ([`polygon_fill_spans`]), just quantised to dot rows instead of pixel rows. `Ellipse` is first approximated as an N-gon ([`ellipse_polygon`]) and then fills identically to `Polygon`. |
//! | [`DrawOp::Arc`] / [`DrawOp::Bezier`] | **braille** — both sample down to a polyline ([`arc_polyline`] / [`bezier_polyline`]) and stroke it exactly like [`DrawOp::Path`]; `stroke_width` is ignored for the same reason `Line`/`Path`'s is. |
//! | [`DrawOp::DashedLine`] | **braille** — split into alternating dash/gap sub-segments ([`dash_segments`]) and each dash strokes like [`DrawOp::Line`]; the gaps are simply not drawn, so the dash pattern survives the degrade (unlike `stroke_width`, which still has no sub-cell width to vary). |
//! | [`DrawOp::StrokeRect`] | **braille** — four [`DrawOp::Line`]-style segments around the rect's perimeter; `stroke_width` is ignored, same as every other stroke op. |
//! | [`DrawOp::StyledTextRun`] | **cell-quantised** — identical degrade to [`DrawOp::TextRun`] (position snaps to the nearest whole cell, background always paints `theme.background`); `role`, `italic` and `size` are dropped entirely, since a terminal cell grid has exactly one font and one cell size by definition (see `crate::font_role`'s module doc: "TUI is deliberately absent from this picture"). |
//!
//! No op is **N/A** — every one produces a real, typed answer the app
//! can see without branching on which backend is live, which is the
//! whole point of D-014's three-tier story.
//!
//! # Alpha and per-op font size
//!
//! There is no separate per-op alpha field: every `DrawOp` that carries a
//! [`Color`] already gets translucency for free through `color.a`
//! (`Color::with_alpha`) — every pixel backend's `surface_fill_rect`
//! (and friends) already honours it with a real native alpha blend, the
//! same way [`crate::paint_surface::PaintSurface::surface_fill_rect_alpha`]'s
//! own doc describes; TUI's braille/cell degrade has no alpha channel to
//! vary (a cell or dot is either lit or not), so it treats any `color.a`
//! as fully opaque, which is the coarsest-but-still-correct answer D-014
//! asks for.
//!
//! [`DrawOp::StyledTextRun::size`] is the one deliberately inert field in
//! this module: every backend accepts it but ignores it today (documented
//! on the field itself), since no `PaintSurface` verb yet takes a
//! per-call font size — this is an honest placeholder for that verb
//! landing in a later, backend-specific issue, not a silent no-op a
//! caller could mistake for working.
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
    /// Fill a closed polygon through `points`, in order, using the
    /// even-odd rule (the same rule [`polygon_fill_spans`] implements).
    /// Unlike [`DrawOp::Path`], `points` is always treated as closed —
    /// there is no `closed` flag, since an *open* filled shape has no
    /// well-defined interior.
    Polygon { points: Vec<Point>, color: Color },
    /// Fill an axis-aligned ellipse centred on `center`.
    Ellipse {
        center: Point,
        radius_x: f32,
        radius_y: f32,
        color: Color,
    },
    /// Stroke a circular arc centred on `center`, from `start_angle` to
    /// `end_angle` (radians, `0.0` pointing along `+x`, increasing
    /// clockwise in screen coordinates — same convention as
    /// [`std::f32::consts::TAU`]'s usual `cos`/`sin` sweep). `end_angle`
    /// may be less than `start_angle` to sweep the other way.
    Arc {
        center: Point,
        radius: f32,
        start_angle: f32,
        end_angle: f32,
        color: Color,
        stroke_width: f32,
    },
    /// Stroke a cubic Bézier curve from `from` to `to`, shaped by control
    /// points `control1`/`control2` — the same four-point contract as
    /// SVG's `C` path command.
    Bezier {
        from: Point,
        control1: Point,
        control2: Point,
        to: Point,
        color: Color,
        stroke_width: f32,
    },
    /// Stroke a single line segment as alternating `dash_len`-long drawn
    /// segments and `gap_len`-long gaps, starting with a drawn segment at
    /// `from`. A non-finite or non-positive `dash_len`/`gap_len` falls
    /// back to a single solid segment, same as a plain [`DrawOp::Line`]
    /// (see [`dash_segments`]).
    DashedLine {
        from: Point,
        to: Point,
        color: Color,
        stroke_width: f32,
        dash_len: f32,
        gap_len: f32,
    },
    /// Stroke the outline of a rect without filling it — the outline
    /// twin of [`DrawOp::Rect`], matching
    /// [`crate::paint_surface::PaintSurface::surface_stroke_rect`]'s own
    /// inset-stroke convention on every pixel backend.
    StrokeRect {
        rect: Rect,
        color: Color,
        stroke_width: f32,
    },
    /// [`DrawOp::TextRun`] with an explicit font role, italic flag, and
    /// (currently inert — see module doc) font size.
    StyledTextRun {
        rect: Rect,
        text: String,
        color: Color,
        role: CanvasFontRole,
        italic: bool,
        /// Per-op font size override. `None` means "this surface's
        /// current size", matching [`DrawOp::TextRun`]. **Not yet wired
        /// to any backend** — every rasteriser accepts and ignores
        /// `Some(_)` today (module doc's "Alpha and per-op font size"
        /// section); this field exists so the descriptor shape doesn't
        /// need another breaking change once a backend grows the verb.
        #[serde(default)]
        size: Option<f32>,
    },
}

/// Serde-friendly mirror of [`crate::FontRole`] for
/// [`DrawOp::StyledTextRun`] — `FontRole` itself has no
/// `Serialize`/`Deserialize` derive (see `crate::font_role`'s module
/// doc), and `Canvas`/`DrawOp` are descriptor types that must round-trip
/// through serde (`PRIMITIVE_RULES.md` rule 8's descriptor-shape
/// guidance). [`Self::to_font_role`] is the only place this maps back to
/// the real enum, kept private to this module since nothing outside the
/// native-paint seam needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanvasFontRole {
    Chrome,
    Editor,
}

impl CanvasFontRole {
    /// Map to the real [`crate::FontRole`] a `PaintSurface` call expects.
    ///
    /// Only called from `native_surface_paint::paint`'s `StyledTextRun`
    /// arm, which is itself compiled in only once a real pixel backend is
    /// enabled (see that module's own `#[allow(dead_code)]` just above) —
    /// a `tui`-only build never calls this method.
    #[allow(dead_code)]
    pub(crate) fn to_font_role(self) -> crate::FontRole {
        match self {
            CanvasFontRole::Chrome => crate::FontRole::Chrome,
            CanvasFontRole::Editor => crate::FontRole::Editor,
        }
    }
}

/// Fill spans for a closed polygon using the even-odd rule, scanning in
/// `row_height`-tall horizontal bands from the polygon's own `y` bounds.
/// Pure geometry, shared by every pixel backend (through
/// [`native_surface_paint::paint`]) and the TUI degrade (`crate::tui::canvas`),
/// since neither [`crate::paint_surface::PaintSurface`] nor the TUI
/// braille grid has a native "fill this polygon" primitive — both lower
/// it to a list of filled rects instead.
///
/// Returns an empty `Vec` for degenerate input (`points.len() < 3`,
/// non-finite/non-positive `row_height`, or a polygon with no vertical
/// extent) rather than panicking — `points` is reachable from
/// `Deserialize`d app data, so it can never be assumed well-formed.
pub(crate) fn polygon_fill_spans(points: &[Point], row_height: f32) -> Vec<Rect> {
    if points.len() < 3 || !row_height.is_finite() || row_height <= 0.0 {
        return Vec::new();
    }
    let min_y = points.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
    let max_y = points.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
    if !min_y.is_finite() || !max_y.is_finite() || max_y <= min_y {
        return Vec::new();
    }

    let mut spans = Vec::new();
    let mut y = min_y;
    while y < max_y {
        let scan_y = y + row_height / 2.0;
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..points.len() {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            let (lo, hi) = if a.y <= b.y { (a, b) } else { (b, a) };
            if scan_y >= lo.y && scan_y < hi.y && hi.y > lo.y {
                let t = (scan_y - lo.y) / (hi.y - lo.y);
                xs.push(lo.x + t * (hi.x - lo.x));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks_exact(2) {
            let (x0, x1) = (pair[0], pair[1]);
            if x1 > x0 {
                spans.push(Rect::new(x0, y, x1 - x0, row_height));
            }
        }
        y += row_height;
    }
    spans
}

/// Approximate an ellipse as a `segments`-sided polygon (`segments`
/// clamped to at least `3`), suitable for [`polygon_fill_spans`].
pub(crate) fn ellipse_polygon(
    center: Point,
    radius_x: f32,
    radius_y: f32,
    segments: usize,
) -> Vec<Point> {
    let n = segments.max(3);
    (0..n)
        .map(|i| {
            let theta = (i as f32) * std::f32::consts::TAU / (n as f32);
            Point::new(
                center.x + radius_x * theta.cos(),
                center.y + radius_y * theta.sin(),
            )
        })
        .collect()
}

/// Sample a circular arc into a polyline of `segments.max(1) + 1` points
/// (inclusive of both endpoints), suitable for stroking as a sequence of
/// line segments — see [`DrawOp::Arc`]'s own doc for the angle convention.
pub(crate) fn arc_polyline(
    center: Point,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
    segments: usize,
) -> Vec<Point> {
    let n = segments.max(1);
    (0..=n)
        .map(|i| {
            let t = (i as f32) / (n as f32);
            let theta = start_angle + t * (end_angle - start_angle);
            Point::new(
                center.x + radius * theta.cos(),
                center.y + radius * theta.sin(),
            )
        })
        .collect()
}

/// Sample a cubic Bézier curve into a polyline of `segments.max(1) + 1`
/// points (inclusive of both endpoints) via the standard cubic Bézier
/// formula, suitable for stroking as a sequence of line segments.
pub(crate) fn bezier_polyline(
    from: Point,
    control1: Point,
    control2: Point,
    to: Point,
    segments: usize,
) -> Vec<Point> {
    let n = segments.max(1);
    (0..=n)
        .map(|i| {
            let t = (i as f32) / (n as f32);
            let mt = 1.0 - t;
            let x = mt * mt * mt * from.x
                + 3.0 * mt * mt * t * control1.x
                + 3.0 * mt * t * t * control2.x
                + t * t * t * to.x;
            let y = mt * mt * mt * from.y
                + 3.0 * mt * mt * t * control1.y
                + 3.0 * mt * t * t * control2.y
                + t * t * t * to.y;
            Point::new(x, y)
        })
        .collect()
}

/// Split the line segment `from..to` into alternating drawn
/// dash/undrawn-gap sub-segments, each `dash_len`/`gap_len` long,
/// starting with a drawn dash at `from`. Falls back to a single solid
/// `from..to` segment (never an infinite loop, never empty) when
/// `dash_len`/`gap_len` is non-finite or non-positive, or when `from` and
/// `to` are the same point.
pub(crate) fn dash_segments(
    from: Point,
    to: Point,
    dash_len: f32,
    gap_len: f32,
) -> Vec<(Point, Point)> {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let total = (dx * dx + dy * dy).sqrt();
    if !total.is_finite() || total <= 0.0 {
        return Vec::new();
    }
    if !dash_len.is_finite() || !gap_len.is_finite() || dash_len <= 0.0 || gap_len < 0.0 {
        return vec![(from, to)];
    }
    let ux = dx / total;
    let uy = dy / total;
    let mut segments = Vec::new();
    let mut pos = 0.0_f32;
    let mut drawing = true;
    while pos < total {
        let step = if drawing { dash_len } else { gap_len };
        let next = (pos + step).min(total);
        if drawing {
            let p0 = Point::new(from.x + ux * pos, from.y + uy * pos);
            let p1 = Point::new(from.x + ux * next, from.y + uy * next);
            segments.push((p0, p1));
        }
        if step <= 0.0 {
            // Zero-length gap: avoid spinning forever on `gap_len == 0.0`.
            drawing = true;
            pos = next.max(pos + f32::EPSILON);
            continue;
        }
        pos = next;
        drawing = !drawing;
    }
    segments
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
    use super::{
        arc_polyline, bezier_polyline, dash_segments, ellipse_polygon, polygon_fill_spans, Canvas,
        CanvasLayout, DrawOp,
    };
    use crate::event::{Point, Rect};
    use crate::paint_surface::PaintSurface;

    /// Scanline band height for lowering [`DrawOp::Polygon`]/
    /// [`DrawOp::Ellipse`] into
    /// [`crate::paint_surface::PaintSurface::surface_fill_rect`] calls —
    /// one device pixel, since every pixel backend's native resolution
    /// is fine enough that this reads as a solid fill (unlike
    /// `tui::canvas`'s dot-row height, which is driven by the braille
    /// grid instead).
    const PIXEL_POLYGON_ROW_HEIGHT: f32 = 1.0;
    /// Segment counts for sampling the curve-shaped ops down to
    /// polylines, chosen generously since a pixel surface's native
    /// resolution hides faceting at normal canvas sizes.
    const ELLIPSE_SEGMENTS: usize = 64;
    const ARC_SEGMENTS: usize = 48;
    const BEZIER_SEGMENTS: usize = 24;

    fn translate_rect(r: Rect, dx: f32, dy: f32) -> Rect {
        Rect::new(r.x + dx, r.y + dy, r.width, r.height)
    }

    fn translate_point(p: Point, dx: f32, dy: f32) -> Point {
        Point::new(p.x + dx, p.y + dy)
    }

    fn translate_points(points: &[Point], dx: f32, dy: f32) -> Vec<Point> {
        points.iter().map(|p| translate_point(*p, dx, dy)).collect()
    }

    fn stroke_polyline(
        surface: &mut dyn PaintSurface,
        points: &[Point],
        color: crate::types::Color,
        stroke_width: f32,
    ) {
        for pair in points.windows(2) {
            surface.surface_draw_line(pair[0], pair[1], color, stroke_width);
        }
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
                DrawOp::Polygon { points, color } => {
                    let translated = translate_points(points, dx, dy);
                    for span in polygon_fill_spans(&translated, PIXEL_POLYGON_ROW_HEIGHT) {
                        surface.surface_fill_rect(span, *color);
                    }
                }
                DrawOp::Ellipse {
                    center,
                    radius_x,
                    radius_y,
                    color,
                } => {
                    let polygon = ellipse_polygon(
                        translate_point(*center, dx, dy),
                        *radius_x,
                        *radius_y,
                        ELLIPSE_SEGMENTS,
                    );
                    for span in polygon_fill_spans(&polygon, PIXEL_POLYGON_ROW_HEIGHT) {
                        surface.surface_fill_rect(span, *color);
                    }
                }
                DrawOp::Arc {
                    center,
                    radius,
                    start_angle,
                    end_angle,
                    color,
                    stroke_width,
                } => {
                    let polyline = arc_polyline(
                        translate_point(*center, dx, dy),
                        *radius,
                        *start_angle,
                        *end_angle,
                        ARC_SEGMENTS,
                    );
                    stroke_polyline(surface, &polyline, *color, *stroke_width);
                }
                DrawOp::Bezier {
                    from,
                    control1,
                    control2,
                    to,
                    color,
                    stroke_width,
                } => {
                    let polyline = bezier_polyline(
                        translate_point(*from, dx, dy),
                        translate_point(*control1, dx, dy),
                        translate_point(*control2, dx, dy),
                        translate_point(*to, dx, dy),
                        BEZIER_SEGMENTS,
                    );
                    stroke_polyline(surface, &polyline, *color, *stroke_width);
                }
                DrawOp::DashedLine {
                    from,
                    to,
                    color,
                    stroke_width,
                    dash_len,
                    gap_len,
                } => {
                    for (seg_from, seg_to) in dash_segments(
                        translate_point(*from, dx, dy),
                        translate_point(*to, dx, dy),
                        *dash_len,
                        *gap_len,
                    ) {
                        surface.surface_draw_line(seg_from, seg_to, *color, *stroke_width);
                    }
                }
                DrawOp::StrokeRect {
                    rect,
                    color,
                    stroke_width,
                } => {
                    surface.surface_stroke_rect(
                        translate_rect(*rect, dx, dy),
                        *color,
                        *stroke_width,
                    );
                }
                DrawOp::StyledTextRun {
                    rect,
                    text,
                    color,
                    role,
                    italic,
                    size: _,
                } => {
                    // `size` is a documented no-op today (module doc,
                    // "Alpha and per-op font size") — no `PaintSurface`
                    // verb yet accepts a per-call font size.
                    surface.surface_draw_text_run_with_role(
                        translate_rect(*rect, dx, dy),
                        text,
                        *color,
                        role.to_font_role(),
                        *italic,
                    );
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
        use super::super::CanvasFontRole;
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
            text_runs_with_role: Vec<(Rect, String, Color, crate::FontRole, bool)>,
            images: Vec<Rect>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
            strokes: Vec<(Rect, Color, f32)>,
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
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_text_run_with_role(
                &mut self,
                rect: Rect,
                text: &str,
                color: Color,
                role: crate::FontRole,
                italic: bool,
            ) {
                self.text_runs_with_role
                    .push((rect, text.to_string(), color, role, italic));
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

        #[test]
        fn polygon_op_fills_via_scanline_spans() {
            let c = canvas(vec![DrawOp::Polygon {
                points: vec![
                    Point::new(0.0, 0.0),
                    Point::new(10.0, 0.0),
                    Point::new(10.0, 10.0),
                    Point::new(0.0, 10.0),
                ],
                color: Color::rgb(5, 6, 7),
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert!(!surface.fills.is_empty());
            assert!(surface
                .fills
                .iter()
                .all(|(_, color)| *color == Color::rgb(5, 6, 7)));
        }

        #[test]
        fn ellipse_op_fills_via_polygon_approximation() {
            let c = canvas(vec![DrawOp::Ellipse {
                center: Point::new(10.0, 10.0),
                radius_x: 5.0,
                radius_y: 3.0,
                color: Color::rgb(8, 9, 10),
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert!(!surface.fills.is_empty());
        }

        #[test]
        fn arc_op_strokes_a_sampled_polyline() {
            let c = canvas(vec![DrawOp::Arc {
                center: Point::new(10.0, 10.0),
                radius: 5.0,
                start_angle: 0.0,
                end_angle: std::f32::consts::PI,
                color: Color::rgb(1, 2, 3),
                stroke_width: 1.0,
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert!(!surface.lines.is_empty());
        }

        #[test]
        fn bezier_op_strokes_a_sampled_polyline() {
            let c = canvas(vec![DrawOp::Bezier {
                from: Point::new(0.0, 0.0),
                control1: Point::new(0.0, 10.0),
                control2: Point::new(10.0, 10.0),
                to: Point::new(10.0, 0.0),
                color: Color::rgb(4, 5, 6),
                stroke_width: 1.0,
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert!(!surface.lines.is_empty());
        }

        #[test]
        fn dashed_line_op_strokes_alternating_segments_not_a_solid_line() {
            let c = canvas(vec![DrawOp::DashedLine {
                from: Point::new(0.0, 0.0),
                to: Point::new(20.0, 0.0),
                color: Color::rgb(1, 1, 1),
                stroke_width: 1.0,
                dash_len: 4.0,
                gap_len: 2.0,
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert!(surface.lines.len() > 1);
        }

        #[test]
        fn stroke_rect_op_forwards_to_surface_stroke_rect() {
            let c = canvas(vec![DrawOp::StrokeRect {
                rect: Rect::new(1.0, 1.0, 8.0, 8.0),
                color: Color::rgb(2, 3, 4),
                stroke_width: 2.0,
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(
                surface.strokes,
                vec![(Rect::new(1.0, 1.0, 8.0, 8.0), Color::rgb(2, 3, 4), 2.0)]
            );
        }

        #[test]
        fn styled_text_run_op_forwards_role_and_italic_size_is_ignored() {
            let c = canvas(vec![DrawOp::StyledTextRun {
                rect: Rect::new(0.0, 0.0, 40.0, 16.0),
                text: "styled".into(),
                color: Color::rgb(7, 7, 7),
                role: CanvasFontRole::Editor,
                italic: true,
                size: Some(24.0),
            }]);
            let layout = c.layout(Rect::new(0.0, 0.0, 50.0, 50.0));
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface);
            assert_eq!(surface.text_runs_with_role.len(), 1);
            let (_, text, _, role, italic) = &surface.text_runs_with_role[0];
            assert_eq!(text, "styled");
            assert_eq!(*role, crate::FontRole::Editor);
            assert!(*italic);
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

    fn round_trip(op: DrawOp) {
        let json = serde_json::to_string(&op).expect("serialize");
        let back: DrawOp = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(op, back);
    }

    #[test]
    fn new_draw_op_variants_round_trip_through_serde() {
        round_trip(DrawOp::Polygon {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(1.0, 0.0),
                Point::new(0.0, 1.0),
            ],
            color: Color::rgb(1, 2, 3),
        });
        round_trip(DrawOp::Ellipse {
            center: Point::new(5.0, 5.0),
            radius_x: 2.0,
            radius_y: 3.0,
            color: Color::rgb(1, 2, 3),
        });
        round_trip(DrawOp::Arc {
            center: Point::new(5.0, 5.0),
            radius: 2.0,
            start_angle: 0.0,
            end_angle: 1.0,
            color: Color::rgb(1, 2, 3),
            stroke_width: 1.0,
        });
        round_trip(DrawOp::Bezier {
            from: Point::new(0.0, 0.0),
            control1: Point::new(1.0, 1.0),
            control2: Point::new(2.0, 1.0),
            to: Point::new(3.0, 0.0),
            color: Color::rgb(1, 2, 3),
            stroke_width: 1.0,
        });
        round_trip(DrawOp::DashedLine {
            from: Point::new(0.0, 0.0),
            to: Point::new(10.0, 0.0),
            color: Color::rgb(1, 2, 3),
            stroke_width: 1.0,
            dash_len: 2.0,
            gap_len: 1.0,
        });
        round_trip(DrawOp::StrokeRect {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            color: Color::rgb(1, 2, 3),
            stroke_width: 1.0,
        });
        round_trip(DrawOp::StyledTextRun {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            text: "hi".into(),
            color: Color::rgb(1, 2, 3),
            role: CanvasFontRole::Chrome,
            italic: false,
            size: None,
        });
        round_trip(DrawOp::StyledTextRun {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            text: "hi".into(),
            color: Color::rgb(1, 2, 3),
            role: CanvasFontRole::Editor,
            italic: true,
            size: Some(18.0),
        });
    }

    #[test]
    fn styled_text_run_size_defaults_to_none_when_field_is_absent_from_json() {
        // Older-shaped JSON (pre-dating the `size` field) must still
        // deserialize via `#[serde(default)]` rather than erroring —
        // the additive-field contract the module doc promises.
        let json = r#"{"StyledTextRun":{"rect":{"x":0.0,"y":0.0,"width":1.0,"height":1.0},"text":"x","color":{"r":1,"g":2,"b":3,"a":255},"role":"Chrome","italic":false}}"#;
        let op: DrawOp = serde_json::from_str(json).expect("deserialize without `size`");
        match op {
            DrawOp::StyledTextRun { size, .. } => assert_eq!(size, None),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn polygon_fill_spans_rejects_degenerate_input() {
        assert!(polygon_fill_spans(&[], 1.0).is_empty());
        assert!(polygon_fill_spans(&[Point::new(0.0, 0.0), Point::new(1.0, 1.0)], 1.0).is_empty());
        let triangle = [
            Point::new(0.0, 0.0),
            Point::new(4.0, 0.0),
            Point::new(2.0, 4.0),
        ];
        assert!(polygon_fill_spans(&triangle, 0.0).is_empty());
        assert!(polygon_fill_spans(&triangle, f32::NAN).is_empty());
    }

    #[test]
    fn polygon_fill_spans_fills_a_square() {
        let square = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ];
        let spans = polygon_fill_spans(&square, 2.0);
        assert!(!spans.is_empty());
        for span in &spans {
            assert!(span.x >= 0.0 && span.x + span.width <= 10.0 + f32::EPSILON);
        }
    }

    #[test]
    fn ellipse_polygon_has_at_least_three_points() {
        assert_eq!(ellipse_polygon(Point::new(0.0, 0.0), 1.0, 1.0, 0).len(), 3);
        assert_eq!(
            ellipse_polygon(Point::new(0.0, 0.0), 1.0, 1.0, 16).len(),
            16
        );
    }

    #[test]
    fn arc_polyline_includes_both_endpoints() {
        let points = arc_polyline(Point::new(0.0, 0.0), 1.0, 0.0, std::f32::consts::PI, 8);
        assert_eq!(points.len(), 9);
        assert!((points[0].x - 1.0).abs() < 1e-5);
        assert!((points[8].x - (-1.0)).abs() < 1e-4);
    }

    #[test]
    fn bezier_polyline_starts_and_ends_at_the_control_points() {
        let points = bezier_polyline(
            Point::new(0.0, 0.0),
            Point::new(0.0, 5.0),
            Point::new(5.0, 5.0),
            Point::new(5.0, 0.0),
            10,
        );
        assert_eq!(points.len(), 11);
        assert_eq!(points[0], Point::new(0.0, 0.0));
        assert_eq!(points[10], Point::new(5.0, 0.0));
    }

    #[test]
    fn dash_segments_rejects_degenerate_and_falls_back_to_solid() {
        let from = Point::new(0.0, 0.0);
        let to = Point::new(10.0, 0.0);
        assert!(dash_segments(from, from, 2.0, 1.0).is_empty());
        assert_eq!(dash_segments(from, to, 0.0, 1.0), vec![(from, to)]);
        assert_eq!(dash_segments(from, to, f32::NAN, 1.0), vec![(from, to)]);
    }

    #[test]
    fn dash_segments_alternates_dash_and_gap() {
        let from = Point::new(0.0, 0.0);
        let to = Point::new(10.0, 0.0);
        let segments = dash_segments(from, to, 2.0, 1.0);
        // 10 units: dash 0-2, gap 2-3, dash 3-5, gap 5-6, dash 6-8, gap
        // 8-9, dash 9-10 -> 4 drawn dashes.
        assert_eq!(segments.len(), 4);
        for (a, b) in &segments {
            assert!(b.x > a.x);
        }
    }

    #[test]
    fn dash_segments_never_loops_forever_on_a_zero_length_gap() {
        let from = Point::new(0.0, 0.0);
        let to = Point::new(10.0, 0.0);
        // A zero gap must still terminate — regression guard for the
        // `step <= 0.0` branch in `dash_segments`.
        let segments = dash_segments(from, to, 2.0, 0.0);
        assert!(!segments.is_empty());
    }

    #[test]
    fn canvas_font_role_maps_to_the_real_font_role() {
        assert_eq!(
            CanvasFontRole::Chrome.to_font_role(),
            crate::FontRole::Chrome
        );
        assert_eq!(
            CanvasFontRole::Editor.to_font_role(),
            crate::FontRole::Editor
        );
    }
}
