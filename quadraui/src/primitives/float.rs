//! `Float` primitive: an anchored overlay surface above the main layout
//! — the supply-side primitive vimcode's "Extension UI Phase 3:
//! overlays/floats for Lua extensions" work needs so vimcode never has
//! to build floats itself (the Platform-Neutrality Rule).
//!
//! A `Float` is pure chrome, the same contract as [`crate::Panel`]:
//! this primitive paints a background fill and an optional border, and
//! nothing else. The content a consumer actually wants inside a float
//! — which-key-style key hints, flash-style jump labels, a
//! treesitter-context-style sticky header, a floating terminal — is any
//! other quadraui primitive, painted by the host into
//! [`FloatLayout::content_bounds`] after [`crate::Backend::draw_float`]
//! returns. `Float` doesn't own that content any more than `Panel`
//! owns the tree/terminal/form a host draws into `PanelLayout::content_bounds`.
//!
//! # Position: [`crate::layout::Anchor`]
//!
//! [`Float::layout`] positions the float with the shared [`Anchor`]
//! resolver (relative to a point, a widget's rect, or a cell) — the
//! same "preferred side, flip on overflow, pin to the viewport edge as
//! a last resort" rule every other anchored overlay in this crate
//! (`Tooltip`, `ContextMenu`, `Completions`, `RichTextPopup`) already
//! follows. Unlike those, `Float` does not take the `Anchor` as a
//! transient `layout()` argument alone — see "Serde-describable" below.
//!
//! # Stacking: [`crate::ModalStack`]
//!
//! A `Float` is not self-stacking — exactly like `Dialog`/`ContextMenu`/
//! `Palette`, the host pushes `float.id` + the resolved
//! [`FloatLayout::bounds`] onto its backend's [`crate::ModalStack`] when
//! the float opens, and pops it when the float closes. Pushing
//! registers the float for hit-testing (`ModalStack::hit_test` tags a
//! `MouseDown` landing inside it with `float.id`, so a host never writes
//! its own "is a float open" click guard — see
//! `examples/common/modal_occlusion_demo.rs` for the established
//! pattern, which `examples/common/float_app.rs` follows for floats).
//!
//! ## Focus vs. non-focus floats
//!
//! [`Float::focusable`] is the "a hint popup must not steal keys; an
//! interactive float must" distinction this primitive exists to carry.
//! It is **not** enforced by this primitive — a `Float` is plain data, same as every
//! other primitive here — it is carried through to
//! [`crate::ModalStack`] via [`crate::ModalStack::push_focusable`] (push
//! a non-focusable float with `push_focusable(id, bounds, false)`
//! instead of the focusable-by-default [`crate::ModalStack::push`]) and
//! read back via [`crate::ModalStack::top_focusable`]: the topmost
//! *focusable* entry, skipping any non-focusable float stacked above it
//! (a which-key hint that popped up on top of an already-open
//! interactive float must not steal that float's keyboard focus). A
//! host routes keyboard events by asking `top_focusable()` who should
//! receive them, falling through to the base layer when it returns
//! `None` — see `examples/common/float_app.rs`'s `handle` for the
//! worked pattern.
//!
//! # Serde-describable (`docs/UI_CRATE_DESIGN.md` §10)
//!
//! `Float` derives `Serialize`/`Deserialize` like every other
//! primitive, so vimcode can map an extension-declared float
//! (`anchor`, `focusable`, optional `bg`/`border`) straight onto this
//! struct and route its events by [`WidgetId`]. [`FloatMeasure`] also
//! derives `Serialize`/`Deserialize` so the float's `width`/`height`
//! travel the same way, even though it is a `layout()` argument rather
//! than a `Float` field — see [`FloatMeasure`]'s own doc for why. This
//! is why `Float` stores its `anchor` as a field (unlike
//! `Tooltip`/`ContextMenu`, which take an anchor rect as a `layout()`
//! argument the caller already has handy in Rust): a plugin-declared
//! float has no Rust call site computing that
//! argument each frame, so the anchor has to survive a JSON round-trip
//! on the struct itself. [`crate::layout::Anchor`]/[`crate::layout::Side`]/
//! [`crate::layout::ResolvedSide`] gained their own `Serialize`/
//! `Deserialize` derives in this same change for exactly this reason.
//!
//! # Backend contract
//!
//! **Chrome-only overlay.** [`crate::Backend::draw_float`] paints a
//! background fill (`float.bg`, or the theme default) and, when
//! `float.border` is set, a 1-unit border stroke — nothing else. No
//! text, no sub-widgets. The host paints its own content into
//! `layout.content_bounds` afterwards, same as `Panel`.

use crate::event::{Point, Rect};
use crate::layout::{Anchor, ResolvedSide};
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a float.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Float {
    pub id: WidgetId,
    /// Where to anchor the float — relative to a point, a cell, or
    /// another widget's rect. See [`Anchor`]'s own doc for the
    /// flip/clamp resolution [`Float::layout`] applies.
    pub anchor: Anchor,
    /// Whether this float takes keyboard focus while open. Defaults to
    /// `true` (matching [`Float::new`]) when omitted from a serialised
    /// descriptor — a transient hint popup must set this to `false`
    /// explicitly. See this module's "Focus vs. non-focus floats" doc
    /// section for how a host reads this back off [`crate::ModalStack`].
    #[serde(default = "default_focusable")]
    pub focusable: bool,
    /// Whether to paint a 1-unit border stroke around the float. `true`
    /// by default — most floats (menus, popups, panels) read as a
    /// distinct surface only with a border; a sticky header or inline
    /// hint banner that should blend into the main layout sets this to
    /// `false`.
    #[serde(default = "default_border")]
    pub border: bool,
    /// Background colour override. `None` = theme default.
    #[serde(default)]
    pub bg: Option<Color>,
}

fn default_border() -> bool {
    true
}

fn default_focusable() -> bool {
    true
}

impl Float {
    /// A focusable-by-default float with a border, at `anchor`, no
    /// colour override — the common case for an interactive popup.
    /// Builder-free convenience matching `Tooltip::new`'s shape; set
    /// `focusable`/`border`/`bg` directly afterwards (every field is
    /// `pub`) rather than via a `with_*` chain — see
    /// `docs/PRIMITIVE_RULES.md`'s rule-8 history of why this crate
    /// stopped adding those.
    pub fn new(id: WidgetId, anchor: Anchor) -> Self {
        Self {
            id,
            anchor,
            focusable: true,
            border: true,
            bg: None,
        }
    }
}

/// `Float::layout`'s caller-supplied size — the box the float reserves
/// on screen, border included (same "whole box, not just content"
/// contract as [`crate::TooltipMeasure`] — see that type's doc for why
/// a caller sizing for *N* content lines on TUI needs the border rows
/// budgeted in). Derives `Serialize`/`Deserialize` alongside [`Float`]
/// itself so an extension-declared float's size can travel the same
/// JSON round-trip as its other fields, even though it is a `layout()`
/// argument rather than a `Float` field.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FloatMeasure {
    pub width: f32,
    pub height: f32,
}

impl FloatMeasure {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// One cell/pixel of border inset reserved on every edge when
/// `float.border` is set — the same whole-row/whole-column convention
/// `Dialog`'s TUI chrome and every pixel backend's 1px stroke already
/// use (see [`crate::Dialog::measure_generic`]'s `border_chrome_inset`
/// doc for the TUI side of this).
const BORDER_INSET: f32 = 1.0;

/// Fully-resolved float layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatLayout {
    /// Full float box bounds, in ABSOLUTE (target-surface) coordinates
    /// — the same frame [`Anchor::resolve`] already returns.
    pub bounds: Rect,
    /// `bounds` inset by [`BORDER_INSET`] on every edge when
    /// `float.border` is set (0 inset otherwise). The host paints its
    /// actual content here — this primitive never does.
    pub content_bounds: Rect,
    /// Which side of the anchor the float actually resolved to (may
    /// differ from `float.anchor.preferred` if the preferred side
    /// overflowed the viewport).
    pub resolved_side: ResolvedSide,
}

/// Classification of a hit-test result. A `Float` has no sub-regions of
/// its own (content is the host's) — hits just report "on the float"
/// vs. "outside." In practice a host rarely calls this directly: a
/// `MouseDown` landing inside an open float's `bounds` already arrives
/// pre-tagged with `float.id` once the host has pushed it onto
/// [`crate::ModalStack`] (see this module's doc), so `FloatHit::Body`
/// is mostly useful for a host that wants to distinguish "inside the
/// float's own chrome" from "inside the content it drew" without a
/// second [`crate::ModalStack`] lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatHit {
    Body,
    Outside,
}

impl Float {
    /// Resolve the float's bounds against `viewport`, applying
    /// [`Anchor::resolve`]'s flip/clamp fallback at the viewport edge.
    pub fn layout(&self, viewport: Rect, measure: FloatMeasure) -> FloatLayout {
        let (origin, resolved_side) = self
            .anchor
            .resolve((measure.width, measure.height), viewport);
        let bounds = Rect::new(origin.x, origin.y, measure.width, measure.height);
        let inset = if self.border { BORDER_INSET } else { 0.0 };
        let content_bounds = Rect::new(
            bounds.x + inset,
            bounds.y + inset,
            (bounds.width - inset * 2.0).max(0.0),
            (bounds.height - inset * 2.0).max(0.0),
        );
        FloatLayout {
            bounds,
            content_bounds,
            resolved_side,
        }
    }
}

impl FloatLayout {
    pub fn hit_test(&self, p: Point) -> FloatHit {
        if self.bounds.contains(p) {
            FloatHit::Body
        } else {
            FloatHit::Outside
        }
    }
}

// ── PaintSurface shared chrome paint (mirrors `primitives::panel` /
// `primitives::canvas` — GTK/macOS/Win share one implementation with
// zero backend-specific policy; TUI has its own cell-based rasteriser
// in `tui::float` because it has no pixel surface to route through
// `PaintSurface`) ──────────────────────────────────────────────────────
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Float, FloatLayout};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a [`Float`]'s chrome (background fill + optional border
    /// stroke) at `layout.bounds`. Content is NOT painted — the host
    /// draws into `layout.content_bounds`, same contract as
    /// `primitives::panel::native_surface_paint::paint`.
    pub(crate) fn paint(
        float: &Float,
        layout: &FloatLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        let bounds = layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }
        let bg = float.bg.unwrap_or(theme.surface_bg);
        surface.surface_fill_rect(bounds, bg);
        if float.border {
            surface.surface_stroke_rect(bounds, theme.border_fg, 1.0);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::event::{Rect, Viewport};
        use crate::layout::{Anchor, Side};
        use crate::primitives::float::FloatMeasure;
        use crate::types::{Color, WidgetId};
        use crate::Image;

        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
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
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, width: f32) {
                self.strokes.push((rect, color, width));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::event::Point,
                _to: crate::event::Point,
                _color: Color,
                _width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_image(
                &mut self,
                _rect: Rect,
                _image: &Image,
            ) -> crate::backend::ImagePaintResult {
                crate::backend::ImagePaintResult::Unsupported
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
        }

        fn float(border: bool) -> Float {
            let mut f = Float::new(
                WidgetId::new("f"),
                Anchor::new(Rect::new(10.0, 10.0, 5.0, 1.0), Side::Bottom),
            );
            f.border = border;
            f
        }

        #[test]
        fn paint_fills_background_and_strokes_border() {
            let f = float(true);
            let layout = f.layout(
                Rect::new(0.0, 0.0, 200.0, 100.0),
                FloatMeasure::new(20.0, 10.0),
            );
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&f, &layout, &mut surface, &theme);
            assert_eq!(surface.fills.len(), 1);
            assert_eq!(surface.fills[0].0, layout.bounds);
            assert_eq!(surface.strokes.len(), 1);
            assert_eq!(surface.strokes[0].0, layout.bounds);
        }

        #[test]
        fn paint_skips_border_stroke_when_disabled() {
            let f = float(false);
            let layout = f.layout(
                Rect::new(0.0, 0.0, 200.0, 100.0),
                FloatMeasure::new(20.0, 10.0),
            );
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&f, &layout, &mut surface, &theme);
            assert_eq!(surface.fills.len(), 1);
            assert!(surface.strokes.is_empty());
        }

        #[test]
        fn paint_uses_bg_override_when_set() {
            let mut f = float(true);
            f.bg = Some(Color::rgb(1, 2, 3));
            let layout = f.layout(
                Rect::new(0.0, 0.0, 200.0, 100.0),
                FloatMeasure::new(20.0, 10.0),
            );
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&f, &layout, &mut surface, &theme);
            assert_eq!(surface.fills[0].1, Color::rgb(1, 2, 3));
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Side;

    fn viewport() -> Rect {
        Rect::new(0.0, 0.0, 100.0, 50.0)
    }

    #[test]
    fn layout_places_float_relative_to_anchor() {
        let anchor = Anchor::new(Rect::new(40.0, 10.0, 10.0, 2.0), Side::Bottom).with_margin(1.0);
        let f = Float::new(WidgetId::new("f"), anchor);
        let layout = f.layout(viewport(), FloatMeasure::new(20.0, 5.0));
        // Bottom placement: x centred on the anchor, y below it + margin.
        assert_eq!(layout.bounds.x, 35.0);
        assert_eq!(layout.bounds.y, 13.0);
        assert_eq!(layout.resolved_side, ResolvedSide::Bottom);
    }

    #[test]
    fn layout_clamps_to_viewport_edge_when_anchor_is_near_the_edge() {
        // Anchor near the bottom edge: Bottom overflows, flips to Top.
        let anchor = Anchor::new(Rect::new(40.0, 48.0, 10.0, 2.0), Side::Bottom).with_margin(1.0);
        let f = Float::new(WidgetId::new("f"), anchor);
        let layout = f.layout(viewport(), FloatMeasure::new(20.0, 5.0));
        assert_eq!(layout.resolved_side, ResolvedSide::Top);
        assert!(layout.bounds.y >= 0.0);
        assert!(layout.bounds.y + layout.bounds.height <= 50.0);
    }

    #[test]
    fn content_bounds_insets_by_border_width_when_bordered() {
        let anchor = Anchor::new(Rect::new(0.0, 0.0, 1.0, 1.0), Side::AtPoint);
        let f = Float::new(WidgetId::new("f"), anchor);
        let layout = f.layout(viewport(), FloatMeasure::new(10.0, 6.0));
        assert_eq!(layout.content_bounds.x, layout.bounds.x + BORDER_INSET);
        assert_eq!(layout.content_bounds.y, layout.bounds.y + BORDER_INSET);
        assert_eq!(layout.content_bounds.width, 10.0 - BORDER_INSET * 2.0);
        assert_eq!(layout.content_bounds.height, 6.0 - BORDER_INSET * 2.0);
    }

    #[test]
    fn content_bounds_fills_the_whole_box_when_borderless() {
        let anchor = Anchor::new(Rect::new(0.0, 0.0, 1.0, 1.0), Side::AtPoint);
        let mut f = Float::new(WidgetId::new("f"), anchor);
        f.border = false;
        let layout = f.layout(viewport(), FloatMeasure::new(10.0, 6.0));
        assert_eq!(layout.content_bounds, layout.bounds);
    }

    #[test]
    fn hit_test_inside_and_outside() {
        let anchor = Anchor::new(Rect::new(0.0, 0.0, 1.0, 1.0), Side::AtPoint);
        let f = Float::new(WidgetId::new("f"), anchor);
        let layout = f.layout(viewport(), FloatMeasure::new(10.0, 6.0));
        let inside = Point::new(layout.bounds.x + 1.0, layout.bounds.y + 1.0);
        let outside = Point::new(layout.bounds.x - 1.0, layout.bounds.y - 1.0);
        assert_eq!(layout.hit_test(inside), FloatHit::Body);
        assert_eq!(layout.hit_test(outside), FloatHit::Outside);
    }

    #[test]
    fn new_defaults_to_focusable_with_border_and_no_bg_override() {
        let anchor = Anchor::new(Rect::new(0.0, 0.0, 1.0, 1.0), Side::AtPoint);
        let f = Float::new(WidgetId::new("f"), anchor);
        assert!(f.focusable);
        assert!(f.border);
        assert!(f.bg.is_none());
    }

    #[test]
    fn serde_round_trip() {
        let anchor = Anchor::new(Rect::new(1.0, 2.0, 3.0, 4.0), Side::Left).with_margin(2.0);
        let mut f = Float::new(WidgetId::new("hint"), anchor);
        f.focusable = false;
        f.bg = Some(Color::rgb(10, 20, 30));
        let json = serde_json::to_string(&f).unwrap();
        let back: Float = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn serde_defaults_focusable_true_and_border_true_when_omitted() {
        // Backward/forward-compat: a serialised float that predates a
        // field addition (or a hand-written JSON descriptor from a Lua
        // extension that only sets `id`/`anchor`) must still deserialise,
        // and must agree with `Float::new`'s own defaults so a
        // JSON-described float and a Rust-constructed one behave alike.
        let json = r#"{"id":"f","anchor":{"rect":{"x":0.0,"y":0.0,"width":1.0,"height":1.0},"preferred":"Bottom","margin":0.0}}"#;
        let f: Float = serde_json::from_str(json).unwrap();
        assert!(f.focusable);
        assert!(f.border);
        assert!(f.bg.is_none());
    }

    #[test]
    fn float_measure_serde_round_trip() {
        let m = FloatMeasure::new(20.0, 5.0);
        let json = serde_json::to_string(&m).unwrap();
        let back: FloatMeasure = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }
}
