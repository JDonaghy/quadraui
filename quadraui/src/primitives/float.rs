//! `Float` primitive: an anchored overlay surface above the main layout
//! — the supply-side primitive vimcode's "Extension UI Phase 3:
//! overlays/floats for Lua extensions" work needs so vimcode never has
//! to build floats itself (the Platform-Neutrality Rule).
//!
//! A `Float` is pure chrome: this primitive paints a background fill
//! and an optional border, and nothing else. The content a consumer
//! actually wants inside a float — which-key-style key hints,
//! flash-style jump labels, a treesitter-context-style sticky header, a
//! floating terminal — is any other quadraui primitive, painted by the
//! host into [`FloatLayout::content_bounds`] after
//! [`crate::Backend::draw_float`] returns. `Float` doesn't own that
//! content any more than [`crate::SidebarPanel`] owns the tree/terminal/
//! form a host draws into `SidebarPanelLayout::content_bounds`.
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
//! [`crate::ModalStack`] via [`crate::ModalStack::push_with_focusable`] (push
//! a non-focusable float with `push_with_focusable(id, bounds, false)`
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
//! `layout.content_bounds` afterwards, same as [`crate::SidebarPanel`].
//!
//! Setting [`Float::transparent`] skips the background fill entirely —
//! a pure border-only overlay through which whatever is already
//! painted underneath shows. On TUI this falls out for free: every
//! backend paints floats directly onto the shared cell buffer in
//! z-order, so not overwriting a cell leaves the layer below it
//! exactly as the host last drew it. On a pixel surface (GTK/macOS/
//! Win) skipping `surface_fill_rect` leaves the surface's existing
//! contents in place for the same reason.

use crate::event::{Point, Rect};
use crate::layout::{Anchor, ResolvedSide, Side};
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a float.
///
/// # Examples
///
/// ```
/// use quadraui::{Anchor, Float, FloatHit, FloatMeasure, Point, Rect, Side, WidgetId};
///
/// let anchor = Anchor::new(Rect::new(10.0, 10.0, 5.0, 1.0), Side::Bottom);
/// let float = Float::new(WidgetId::new("float:keyhints"), anchor);
///
/// let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
/// let layout = float.layout(viewport, FloatMeasure::new(20.0, 4.0));
///
/// assert_eq!(layout.hit_test(Point::new(15.0, 12.0)), FloatHit::Body);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// Background colour override. `None` = theme default. Ignored
    /// when [`Float::transparent`] is `true` — there is no fill to
    /// colour.
    #[serde(default)]
    pub bg: Option<Color>,
    /// Skip the background fill entirely, leaving a border-only (when
    /// `border` is set) overlay through which whatever is already
    /// painted underneath shows. `false` by default, matching
    /// [`Float::new`] — most floats (menus, popups, panels) want an
    /// opaque surface; a sticky header or inline hint banner that must
    /// not obscure the content behind it sets this to `true`. Takes
    /// priority over [`Float::bg`]: a transparent float paints no
    /// fill regardless of what `bg` is set to.
    #[serde(default)]
    pub transparent: bool,
}

fn default_border() -> bool {
    true
}

fn default_focusable() -> bool {
    true
}

impl Float {
    /// A focusable-by-default float with a border, at `anchor`, no
    /// colour override, opaque background — the common case for an
    /// interactive popup. Builder-free convenience matching
    /// `Tooltip::new`'s shape; set `focusable`/`border`/`bg`/
    /// `transparent` directly afterwards (every field is `pub`) rather
    /// than via a `with_*` chain — see `docs/PRIMITIVE_RULES.md`'s
    /// rule-8 history of why this crate stopped adding those.
    pub fn new(id: WidgetId, anchor: Anchor) -> Self {
        Self {
            id,
            anchor,
            focusable: true,
            border: true,
            bg: None,
            transparent: false,
        }
    }
}

impl Default for Float {
    /// A focusable, bordered float anchored to the origin with an empty
    /// id. See [`Float::new`] for a float with a real id/anchor.
    fn default() -> Self {
        Self::new(
            WidgetId::new(""),
            Anchor::new(Rect::default(), Side::default()),
        )
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
        self.layout_at(bounds, resolved_side)
    }

    /// The layout of a float whose box is already placed at `bounds`:
    /// `content_bounds` is `bounds` inset by the border, when there is one.
    fn layout_at(&self, bounds: Rect, resolved_side: ResolvedSide) -> FloatLayout {
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

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────
//
// `Float` reaches the frame type-erased as `Surface::Widget`. Its size is a
// `layout()` argument rather than a descriptor field, so `Primitive::layout`
// takes the rect the host already resolved through [`Float::layout`] as the
// float's final box. A float paints chrome only and reacts to no input
// itself — focus routing for a focusable float goes through
// [`crate::ModalStack`], and the content inside it is another widget — so
// it is `non_interactive!`.

impl crate::a11y::A11yPrimitive for Float {
    /// A generic group: the float is a container, and the content the host
    /// paints inside it carries its own semantics.
    fn a11y_info(&self) -> crate::a11y::A11yInfo {
        crate::a11y::A11yInfo::new()
            .with_role(crate::a11y::A11yRole::Group)
            .with_label("Floating panel")
    }
}

/// The [`ResolvedSide`] a preference names when no viewport flip applied —
/// what a layout built from an already-placed rect reports.
fn unflipped(side: Side) -> ResolvedSide {
    match side {
        Side::Top => ResolvedSide::Top,
        Side::Bottom => ResolvedSide::Bottom,
        Side::Left => ResolvedSide::Left,
        Side::Right => ResolvedSide::Right,
        Side::AtPoint => ResolvedSide::AtPoint,
    }
}

impl crate::primitive::Primitive for Float {
    type Layout = FloatLayout;
    type Event = FloatHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Float(self.clone())
    }

    /// `rect` is the float's final box — the `bounds` a host got from
    /// [`Float::layout`] — reported with the anchor's preferred side as
    /// the resolved one.
    fn layout(&self, _ctx: &crate::primitive::LayoutCtx, rect: Rect) -> FloatLayout {
        self.layout_at(rect, unflipped(self.anchor.preferred))
    }

    fn paint(
        &self,
        layout: &FloatLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        _style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            native_surface_paint::paint(self, layout, surface, theme);
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_float`] because the `Buffer` → `CellSurface`
    /// adapter is private to `crate::tui`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &FloatLayout,
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
        cell_paint::paint(self, layout, &mut surface, theme);
    }
}

crate::non_interactive!(Float);

/// The cell-grid painter for [`Float`], written against
/// [`crate::CellSurface`]: a filled background box and — when
/// `float.border` is set — a square-corner border (`┌─┐`/`│`/`└─┘`).
/// Content is the host's, painted into `layout.content_bounds`.
///
/// `float.transparent` skips the fill and paints the border glyphs over
/// each cell's existing background, so the layer below shows through.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{Float, FloatLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;

    /// Paint `float`'s chrome at `layout.bounds`, rounded to whole cells.
    pub(crate) fn paint(
        float: &Float,
        layout: &FloatLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let x = layout.bounds.x.round() as u16;
        let y = layout.bounds.y.round() as u16;
        let w = layout.bounds.width.round() as u16;
        let h = layout.bounds.height.round() as u16;
        if w == 0 || h == 0 {
            return;
        }

        let bg = float.bg.unwrap_or(theme.surface_bg);
        if !float.transparent {
            surface.fill(
                layout.bounds,
                CellStyle {
                    fg: Some(bg),
                    bg: Some(bg),
                    ..CellStyle::default()
                },
            );
        }

        if !float.border {
            return;
        }
        if w < 2 || h < 2 {
            // Too small for a full box — leave the flat fill in place
            // rather than a border that would overwrite all of it.
            return;
        }

        // Per cell rather than `CellSurface::draw_border`, which takes one
        // style for the whole border: a transparent float's border keeps
        // each underlying cell's own background.
        let put = |surface: &mut dyn CellSurface, cx: u16, cy: u16, ch: char| {
            let cell_bg = if float.transparent {
                surface.get(cx, cy).and_then(|(_, style)| style.bg)
            } else {
                Some(bg)
            };
            let style = CellStyle {
                fg: Some(theme.border_fg),
                bg: cell_bg,
                ..CellStyle::default()
            };
            surface.put_char(cx, cy, ch, style);
        };
        let (right, bottom) = (x + w - 1, y + h - 1);
        for cx in x..=right {
            let (top_ch, bottom_ch) = if cx == x {
                ('┌', '└')
            } else if cx == right {
                ('┐', '┘')
            } else {
                ('─', '─')
            };
            put(surface, cx, y, top_ch);
            put(surface, cx, bottom, bottom_ch);
        }
        for cy in (y + 1)..bottom {
            put(surface, x, cy, '│');
            put(surface, right, cy, '│');
        }
    }
}

// ── PaintSurface shared chrome paint (mirrors `primitives::panel` /
// `primitives::canvas` — GTK/macOS/Win share one implementation with
// zero backend-specific policy; TUI paints through `cell_paint` above
// because it has no pixel surface to route through `PaintSurface`) ─────
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
    /// [`crate::SidebarPanel`]'s own paint. Skips the fill entirely when
    /// `float.transparent` is set, leaving whatever the surface
    /// already holds in place underneath the border.
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
        if !float.transparent {
            let bg = float.bg.unwrap_or(theme.surface_bg);
            surface.surface_fill_rect(bounds, bg);
        }
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
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                _radius: f32,
                color: Color,
                width: f32,
            ) {
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

        #[test]
        fn paint_skips_fill_but_keeps_border_when_transparent() {
            let mut f = float(true);
            f.transparent = true;
            f.bg = Some(Color::rgb(1, 2, 3)); // ignored — transparent wins.
            let layout = f.layout(
                Rect::new(0.0, 0.0, 200.0, 100.0),
                FloatMeasure::new(20.0, 10.0),
            );
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&f, &layout, &mut surface, &theme);
            assert!(surface.fills.is_empty());
            assert_eq!(surface.strokes.len(), 1);
            assert_eq!(surface.strokes[0].0, layout.bounds);
        }

        #[test]
        fn paint_skips_fill_and_border_when_transparent_and_borderless() {
            let mut f = float(false);
            f.transparent = true;
            let layout = f.layout(
                Rect::new(0.0, 0.0, 200.0, 100.0),
                FloatMeasure::new(20.0, 10.0),
            );
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&f, &layout, &mut surface, &theme);
            assert!(surface.fills.is_empty());
            assert!(surface.strokes.is_empty());
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
        assert!(!f.transparent);
    }

    #[test]
    fn serde_round_trip() {
        let anchor = Anchor::new(Rect::new(1.0, 2.0, 3.0, 4.0), Side::Left).with_margin(2.0);
        let mut f = Float::new(WidgetId::new("hint"), anchor);
        f.focusable = false;
        f.bg = Some(Color::rgb(10, 20, 30));
        f.transparent = true;
        let json = serde_json::to_string(&f).unwrap();
        let back: Float = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn serde_defaults_focusable_true_border_true_and_transparent_false_when_omitted() {
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
        assert!(!f.transparent);
    }

    #[test]
    fn float_measure_serde_round_trip() {
        let m = FloatMeasure::new(20.0, 5.0);
        let json = serde_json::to_string(&m).unwrap();
        let back: FloatMeasure = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    // ── Widget contract (`Primitive` + `Interactive`) ─────────────────

    fn ctx() -> crate::primitive::LayoutCtx<'static> {
        crate::primitive::LayoutCtx::new(1.0, 1.0, false)
    }

    /// `Primitive::layout` keeps the host-placed rect as the box, insets
    /// the content by the border, and reports the anchor's preferred side.
    #[test]
    fn primitive_layout_takes_the_rect_as_the_box() {
        use crate::primitive::Primitive;
        let anchor = Anchor::new(Rect::new(0.0, 0.0, 1.0, 1.0), Side::Right);
        let f = Float::new(WidgetId::new("f"), anchor);
        let rect = Rect::new(4.0, 2.0, 10.0, 6.0);
        let layout = Primitive::layout(&f, &ctx(), rect);
        assert_eq!(layout.bounds, rect);
        assert_eq!(layout.content_bounds, Rect::new(5.0, 3.0, 8.0, 4.0));
        assert_eq!(layout.resolved_side, ResolvedSide::Right);
        assert_eq!(Primitive::id(&f), Some(&f.id));
    }

    /// Fed the bounds `Float::layout` resolved, `Primitive::layout`
    /// reproduces that layout whenever the preferred side was kept.
    #[test]
    fn primitive_layout_reproduces_an_unflipped_float_layout() {
        use crate::primitive::Primitive;
        let anchor = Anchor::new(Rect::new(40.0, 10.0, 10.0, 2.0), Side::Bottom).with_margin(1.0);
        let mut f = Float::new(WidgetId::new("f"), anchor);
        f.border = false;
        let resolved = f.layout(viewport(), FloatMeasure::new(20.0, 5.0));
        assert_eq!(Primitive::layout(&f, &ctx(), resolved.bounds), resolved);
    }

    /// A float takes no focus of its own: keyboard routing to a focusable
    /// float goes through `ModalStack`, not the widget.
    #[test]
    fn float_is_non_interactive() {
        use crate::interactive::{FocusSpec, Interactive};
        let f = Float::default();
        assert_eq!(Interactive::focus(&f), FocusSpec::None);
    }

    /// The a11y info matches the generic "Floating panel" group the
    /// accessibility tree already reports for a float.
    #[test]
    fn a11y_info_is_a_floating_panel_group() {
        use crate::a11y::{A11yInfo, A11yPrimitive, A11yRole};
        assert_eq!(
            Float::default().a11y_info(),
            A11yInfo::new()
                .with_role(A11yRole::Group)
                .with_label("Floating panel")
        );
    }

    #[test]
    fn primitive_capture_is_the_float_content() {
        use crate::primitive::Primitive;
        let f = Float::default();
        match Primitive::capture(&f) {
            crate::frame::OwnedContent::Float(captured) => assert_eq!(captured, f),
            other => panic!("expected OwnedContent::Float, got {other:?}"),
        }
    }

    // ── Cell-path paint (`cell_paint::paint`) ───────────────────────────

    #[cfg(feature = "tui")]
    mod cells {
        use super::super::cell_test_paint::draw_float;
        use super::*;
        use crate::event::Point;
        use crate::theme::Theme;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf.cell((x, y)).unwrap().symbol().chars().next().unwrap()
        }

        fn theme() -> Theme {
            Theme::default()
        }

        #[test]
        fn draws_bordered_box_at_resolved_bounds() {
            let mut buf = Buffer::empty(RRect::new(0, 0, 40, 20));
            let anchor = Anchor::new(Rect::new(5.0, 5.0, 4.0, 1.0), Side::Bottom);
            let float = Float::new(WidgetId::new("f"), anchor);
            let viewport = Rect::new(0.0, 0.0, 40.0, 20.0);
            let layout = float.layout(viewport, FloatMeasure::new(10.0, 4.0));
            draw_float(&mut buf, &float, &layout, &theme());

            let x = layout.bounds.x.round() as u16;
            let y = layout.bounds.y.round() as u16;
            let w = layout.bounds.width.round() as u16;
            let h = layout.bounds.height.round() as u16;
            assert_eq!(cell_char(&buf, x, y), '┌');
            assert_eq!(cell_char(&buf, x + w - 1, y), '┐');
            assert_eq!(cell_char(&buf, x, y + h - 1), '└');
            assert_eq!(cell_char(&buf, x + w - 1, y + h - 1), '┘');
            assert_eq!(cell_char(&buf, x, y + 1), '│');
        }

        #[test]
        fn borderless_float_paints_flat_fill_with_no_box_glyphs() {
            let mut buf = Buffer::empty(RRect::new(0, 0, 40, 20));
            let anchor = Anchor::new(Rect::new(5.0, 5.0, 4.0, 1.0), Side::Bottom);
            let mut float = Float::new(WidgetId::new("f"), anchor);
            float.border = false;
            let viewport = Rect::new(0.0, 0.0, 40.0, 20.0);
            let layout = float.layout(viewport, FloatMeasure::new(10.0, 4.0));
            draw_float(&mut buf, &float, &layout, &theme());

            let x = layout.bounds.x.round() as u16;
            let y = layout.bounds.y.round() as u16;
            assert_eq!(cell_char(&buf, x, y), ' ');
        }

        /// Paint/click round-trip (`docs/PRIMITIVE_RULES.md` rule 4): find
        /// the painted top-left border glyph, hit-test that exact cell
        /// against `layout.hit_test`, and assert both agree the box starts
        /// there.
        #[test]
        fn paint_and_hit_test_agree_on_the_box_bounds() {
            let mut buf = Buffer::empty(RRect::new(0, 0, 40, 20));
            let anchor = Anchor::new(Rect::new(5.0, 5.0, 4.0, 1.0), Side::Bottom);
            let float = Float::new(WidgetId::new("f"), anchor);
            let viewport = Rect::new(0.0, 0.0, 40.0, 20.0);
            let layout = float.layout(viewport, FloatMeasure::new(10.0, 4.0));
            draw_float(&mut buf, &float, &layout, &theme());

            // Find the painted top-left corner glyph by scanning the buffer —
            // not by re-deriving it from `layout` (that would just check
            // layout against itself).
            let mut found: Option<(u16, u16)> = None;
            for row in 0..20u16 {
                for col in 0..40u16 {
                    if cell_char(&buf, col, row) == '┌' {
                        found = Some((col, row));
                    }
                }
            }
            let (fx, fy) = found.expect("top-left border glyph must have painted");

            let p = Point::new(fx as f32, fy as f32);
            assert_eq!(
                layout.hit_test(p),
                crate::primitives::float::FloatHit::Body,
                "the painted top-left cell must hit-test as inside the float"
            );
            // One cell outside (up-left) must miss.
            let outside = Point::new(fx as f32 - 1.0, fy as f32 - 1.0);
            assert_eq!(
                layout.hit_test(outside),
                crate::primitives::float::FloatHit::Outside
            );
        }

        /// `Primitive::paint_cells` paints exactly what the TUI entry
        /// point paints.
        #[test]
        fn primitive_paint_cells_matches_draw_float() {
            use crate::primitive::{LayoutCtx, Primitive};
            let anchor = Anchor::new(Rect::new(5.0, 5.0, 4.0, 1.0), Side::Bottom);
            let float = Float::new(WidgetId::new("f"), anchor);
            let layout = float.layout(
                Rect::new(0.0, 0.0, 40.0, 20.0),
                FloatMeasure::new(10.0, 4.0),
            );
            let ctx = LayoutCtx::new(1.0, 1.0, false);
            let via_layout = Primitive::layout(&float, &ctx, layout.bounds);
            assert_eq!(via_layout, layout);

            let mut via_primitive = Buffer::empty(RRect::new(0, 0, 40, 20));
            Primitive::paint_cells(&float, &via_layout, &mut via_primitive, &theme());
            let mut direct = Buffer::empty(RRect::new(0, 0, 40, 20));
            draw_float(&mut direct, &float, &layout, &theme());
            assert_eq!(via_primitive, direct);
        }
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

    pub fn draw_float(buf: &mut Buffer, float: &Float, layout: &FloatLayout, theme: &Theme) {
        let mut sink = None;
        cell_paint::paint(float, layout, &mut surface(buf, &mut sink), theme);
    }
}
