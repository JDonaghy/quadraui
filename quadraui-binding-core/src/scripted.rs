//! [`ScriptedPrimitive`]: a [`quadraui::primitive::Primitive`] backed by a
//! script-side widget definition — `measure`/`paint`/`on_event`/`a11y`
//! callbacks a binding forwards into its host language's script runtime,
//! instead of Rust code — so a script-authored widget is drawn through
//! [`quadraui::backend::DrawsPrimitives::draw_primitive`], the exact seam a
//! hand-written Rust [`quadraui::primitive::Primitive`] impl already uses,
//! on every backend.
//!
//! # Display-list reuse — no new op set
//!
//! [`ScriptWidget::paint`] answers with the same [`quadraui::DrawOp`]/
//! [`quadraui::CanvasRegion`] vocabulary a [`quadraui::Canvas`] descriptor
//! already carries. [`ScriptedPrimitive`] builds a `Canvas` from that
//! answer and paints it through the identical code every built-in `Canvas`
//! leaf already paints through:
//! [`quadraui::primitives::canvas::native_surface_paint::paint`] on a
//! pixel backend's [`quadraui::PaintSurface`], [`quadraui::tui::draw_canvas`]
//! on TUI's cell buffer. Region hit-testing reuses
//! [`quadraui::CanvasLayout::region_hit_test`] the same way.
//!
//! # Caching — one FFI round trip per widget per frame at most
//!
//! A script call crosses a real FFI boundary in a real binding, so
//! [`ScriptedPrimitive`] never calls back into the script more than once
//! per frame per cache key. [`quadraui::primitive::Primitive::layout`]
//! caches `measure`'s answer keyed on `(ctx, rect)`;
//! [`quadraui::primitive::Primitive::paint`]/`paint_cells` cache `paint`'s
//! answer keyed on `(props, rect, theme)` — both skip the script call
//! entirely when a later frame's key matches the cached one.
//! [`ScriptedPrimitive::call_count`] reports the running total, for a test
//! to assert a repeated frame with an unchanged key made no further calls.
//!
//! # Why no central engine/render dispatch hook
//!
//! [`ScriptedPrimitive`] implements [`crate::extra_paint::ExtraPaint`]
//! directly (see the `impl` below), the existing additive-primitive seam
//! [`crate::render::RenderFrame::extra`] already paints/hit-tests through
//! generically — [`crate::extra_paint`]'s own module doc: "no enum
//! variant, no central match arm". That covers paint and whole-area click
//! routing for free; nothing in `render.rs`/`engine.rs` needs to change.
//! Resolving a click to a *specific* [`quadraui::CanvasRegion`] (rather
//! than "hit this widget at all") goes through
//! [`ScriptedPrimitive::hit_test`]/[`ScriptedPrimitive::click_region`]
//! directly — the same scope a real `Canvas` leaf's
//! own `HitDetail::CanvasRegion` plumbing needed its own dedicated
//! `hit.rs`/`semantic.rs` change for, left to a follow-up issue rather than
//! bundled into this one.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use quadraui::primitive::{LayoutCtx, Primitive};
use quadraui::primitives::canvas::native_surface_paint;
use quadraui::{
    A11yChild, A11yInfo, A11yPrimitive, Backend, Canvas, CanvasEvent, CanvasHit, CanvasLayout,
    CanvasRegion, DrawOp, DrawsPrimitives, Key, Modifiers, MouseButton, NaturalSize, PaintSurface,
    Point, Rect, Style, Theme, WidgetId,
};

use crate::extra_paint::ExtraPaint;

/// Script-side widget-definition data, round-tripped unchanged through
/// every [`ScriptWidget`] callback that takes it. `serde_json::Value`
/// matches the wire shape every other descriptor this crate decodes
/// already uses (see `crate::wire`) — a binding passes its script's props
/// dict straight through rather than this crate inventing its own typed
/// props shape.
pub type ScriptedProps = serde_json::Value;

/// Script-side widget state threaded through [`ScriptWidget::on_event`] —
/// same shape/rationale as [`ScriptedProps`], kept as a separate type alias
/// since a binding's props (what the script declared) and state (what the
/// script is tracking internally, e.g. a drag-in-progress) are logically
/// distinct even though both are just a `serde_json::Value` today.
pub type ScriptedState = serde_json::Value;

/// Available space [`ScriptWidget::measure`] sizes into — the layout
/// solver's counterpart of a leaf's intrinsic-size pass. `None` on an axis
/// means unconstrained on that axis, the same convention
/// [`quadraui::NaturalSize`]'s own doc describes for "no cheap
/// measurement".
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScriptedConstraints {
    pub max_width: Option<f32>,
    pub max_height: Option<f32>,
}

/// Size [`ScriptWidget::measure`] answers with.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScriptedSize {
    pub width: f32,
    pub height: f32,
}

/// Display list + regions [`ScriptWidget::paint`] answers with — reuses
/// [`DrawOp`]/[`CanvasRegion`] verbatim; see module doc "Display-list
/// reuse".
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptedPaint {
    pub ops: Vec<DrawOp>,
    pub regions: Vec<CanvasRegion>,
}

impl ScriptedPaint {
    /// Builds the [`Canvas`] [`ScriptedPrimitive`] actually paints through
    /// — `id`/`focusable` come from the primitive itself, not the script's
    /// per-frame answer, since neither varies frame to frame the way
    /// `ops`/`regions` do.
    fn as_canvas(&self, id: WidgetId, focusable: bool) -> Canvas {
        Canvas {
            id,
            ops: self.ops.clone(),
            regions: self.regions.clone(),
            focusable,
        }
    }
}

/// Accessibility node [`ScriptWidget::a11y`] answers with — reuses
/// [`A11yInfo`]/[`A11yChild`] verbatim (the `role`/`name`/`state` shape
/// every other [`A11yPrimitive`] impl in this workspace already reports
/// through) rather than a parallel role/name/state vocabulary.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptedA11y {
    pub info: A11yInfo,
    pub children: Vec<A11yChild>,
}

/// A pointer/key input routed to [`ScriptWidget::on_event`] — resolved by
/// a host from a real `quadraui::UiEvent` plus [`ScriptedPrimitive`]'s own
/// region hit-testing ([`ScriptedPrimitive::hit_test`]), the "host derives
/// these" contract [`quadraui::primitive::Primitive::Event`]'s own doc
/// describes. Wraps [`CanvasEvent`] for the pointer cases rather than
/// re-declaring `RegionClicked`/`RegionDoubleClicked`/`RegionHovered` —
/// the same region-click vocabulary a real `Canvas` leaf's
/// `UiEvent::Canvas` already carries.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptedInputEvent {
    /// A declared region was clicked, double-clicked, or changed hover
    /// state.
    Canvas(CanvasEvent),
    /// A key was pressed while this widget held focus.
    Key(Key),
}

/// The script-side callback boundary [`ScriptedPrimitive`] wraps. A real
/// binding implements this once per host language — a Python/JS object
/// with `measure`/`paint`/`on_event`/`a11y` methods forwarding into the
/// script runtime; this crate's own tests implement it directly in Rust
/// ("the way a binding would" — see the `tests` module below) to exercise
/// the whole seam with no actual FFI boundary in the way.
pub trait ScriptWidget {
    /// Sizes this widget for the layout solver.
    fn measure(&self, ctx: &LayoutCtx, constraints: ScriptedConstraints) -> ScriptedSize;

    /// Builds this frame's display list and hit-testable regions.
    fn paint(&self, ctx: &LayoutCtx, props: &ScriptedProps, rect: Rect) -> ScriptedPaint;

    /// Handles a region click/double-click or a focused key press.
    /// Returns whether the script consumed it.
    fn on_event(
        &self,
        event: &ScriptedInputEvent,
        props: &ScriptedProps,
        state: &ScriptedState,
    ) -> bool;

    /// Optional accessibility node. `None` (the default) reports nothing —
    /// correct for a widget with no accessible surface of its own.
    fn a11y(&self, props: &ScriptedProps) -> Option<ScriptedA11y> {
        let _ = props;
        None
    }
}

/// The fully-resolved layout [`ScriptedPrimitive`] produces — just the
/// absolute area and the script's measured size, per
/// [`quadraui::primitive::Primitive`]'s own
/// "layout bakes in what paint needs" design (module doc, "Design"
/// section). Region hit-testing lives on [`ScriptedPrimitive`] itself
/// instead of here, since it depends on `paint`'s answer, not `measure`'s
/// — see [`ScriptedPrimitive::hit_test`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptedLayout {
    pub rect: Rect,
    pub size: ScriptedSize,
}

#[derive(Debug, Clone, PartialEq)]
struct MeasureCacheKey {
    ctx: LayoutCtx,
    rect: Rect,
}

#[derive(Debug, Clone, PartialEq)]
struct PaintCacheKey {
    props: ScriptedProps,
    rect: Rect,
    theme: Theme,
}

struct PaintCacheEntry {
    key: PaintCacheKey,
    paint: ScriptedPaint,
    /// Absolute layout the cached `paint` was resolved against — needed
    /// by [`ScriptedPrimitive::hit_test`]/[`ScriptedPrimitive::canvas`] to
    /// rebuild the same [`CanvasLayout`] `paint`/`paint_cells` last used,
    /// without re-deriving it from `key.rect` (which is LOCAL-origin-free,
    /// same value, but spelling it out once here avoids two sources of
    /// truth for "what layout did we last paint into").
    layout: CanvasLayout,
}

/// A [`quadraui::primitive::Primitive`] backed by a [`ScriptWidget`]. See
/// the module doc for the full contract.
pub struct ScriptedPrimitive<W: ScriptWidget> {
    id: WidgetId,
    widget: W,
    focusable: bool,
    props: RefCell<ScriptedProps>,
    state: RefCell<ScriptedState>,
    measure_cache: RefCell<Option<(MeasureCacheKey, ScriptedSize)>>,
    paint_cache: RefCell<Option<PaintCacheEntry>>,
    a11y_cache: RefCell<Option<(ScriptedProps, Option<ScriptedA11y>)>>,
    /// Total number of calls made into [`Self::widget`] across
    /// `measure`/`paint`/`on_event`/`a11y` — see module doc "Caching".
    call_count: AtomicU64,
}

impl<W: ScriptWidget> ScriptedPrimitive<W> {
    /// Builds a primitive around `widget`, with no props/state set yet
    /// (both start as `serde_json::Value::Null`) and `focusable` matching
    /// [`quadraui::Canvas::focusable`]'s own "declarative flag the app
    /// reads back" contract — [`ScriptedPrimitive`] never computes a
    /// tab-stop list itself.
    pub fn new(id: WidgetId, widget: W, focusable: bool) -> Self {
        ScriptedPrimitive {
            id,
            widget,
            focusable,
            props: RefCell::new(serde_json::Value::Null),
            state: RefCell::new(serde_json::Value::Null),
            measure_cache: RefCell::new(None),
            paint_cache: RefCell::new(None),
            a11y_cache: RefCell::new(None),
            call_count: AtomicU64::new(0),
        }
    }

    /// Updates the props a host re-renders this widget with each frame.
    /// Does not itself call into the script — a later `layout`/`paint`
    /// call does that, and only on a cache miss (module doc, "Caching").
    pub fn set_props(&self, props: ScriptedProps) {
        *self.props.borrow_mut() = props;
    }

    /// Updates the widget state [`ScriptWidget::on_event`] reads back.
    pub fn set_state(&self, state: ScriptedState) {
        *self.state.borrow_mut() = state;
    }

    /// Total script calls made so far — see module doc "Caching".
    pub fn call_count(&self) -> u64 {
        self.call_count.load(Ordering::Relaxed)
    }

    fn record_call(&self) {
        self.call_count.fetch_add(1, Ordering::Relaxed);
    }

    /// The [`Canvas`] + [`CanvasLayout`] `paint`/`paint_cells` last
    /// resolved — `None` until the first `paint`/`paint_cells` call, since
    /// the display list (and so the regions a click can resolve against)
    /// doesn't exist before that.
    fn canvas(&self) -> Option<(Canvas, CanvasLayout)> {
        self.paint_cache
            .borrow()
            .as_ref()
            .map(|entry| (entry.paint.as_canvas(self.id, self.focusable), entry.layout))
    }

    /// Resolves `point` (ABSOLUTE — same frame as the `rect` last passed
    /// to [`Primitive::layout`]/[`Primitive::paint`]) against this
    /// widget's last-painted [`Canvas::regions`], via
    /// [`CanvasLayout::region_hit_test`] — the identical resolution a real
    /// `Canvas` leaf gives. [`CanvasHit::Outside`] before any paint has
    /// happened yet (no regions exist to hit).
    pub fn hit_test(&self, point: Point) -> CanvasHit {
        match self.canvas() {
            Some((canvas, layout)) => layout.region_hit_test(&canvas, point.x, point.y),
            None => CanvasHit::Outside,
        }
    }

    /// Routes a resolved region click to [`ScriptWidget::on_event`],
    /// always a real script call (never cached — each dispatched event is
    /// its own round trip, unlike the per-frame display list). Returns
    /// whether the script consumed it.
    pub fn click_region(
        &self,
        region: WidgetId,
        button: MouseButton,
        modifiers: Modifiers,
        local: Point,
        double: bool,
    ) -> bool {
        let event = if double {
            ScriptedInputEvent::Canvas(CanvasEvent::RegionDoubleClicked {
                region,
                x: local.x,
                y: local.y,
            })
        } else {
            ScriptedInputEvent::Canvas(CanvasEvent::RegionClicked {
                region,
                button,
                modifiers,
                x: local.x,
                y: local.y,
            })
        };
        self.record_call();
        self.widget
            .on_event(&event, &self.props.borrow(), &self.state.borrow())
    }

    /// Routes a focused key press to [`ScriptWidget::on_event`]. Always a
    /// real script call, same reasoning as [`Self::click_region`].
    pub fn key_pressed(&self, key: Key) -> bool {
        let event = ScriptedInputEvent::Key(key);
        self.record_call();
        self.widget
            .on_event(&event, &self.props.borrow(), &self.state.borrow())
    }

    /// The script's accessibility node for the current props, calling
    /// [`ScriptWidget::a11y`] only when `props` has changed since the last
    /// call (module doc, "Caching").
    fn resolve_a11y(&self) -> Option<ScriptedA11y> {
        let props = self.props.borrow().clone();
        if let Some((cached_props, cached)) = self.a11y_cache.borrow().as_ref() {
            if *cached_props == props {
                return cached.clone();
            }
        }
        self.record_call();
        let result = self.widget.a11y(&props);
        *self.a11y_cache.borrow_mut() = Some((props, result.clone()));
        result
    }

    /// `measure`'s answer for `(ctx, rect)`, calling
    /// [`ScriptWidget::measure`] only on a cache miss.
    fn resolve_size(&self, ctx: &LayoutCtx, rect: Rect) -> ScriptedSize {
        let key = MeasureCacheKey { ctx: *ctx, rect };
        if let Some((cached_key, cached_size)) = self.measure_cache.borrow().as_ref() {
            if *cached_key == key {
                return *cached_size;
            }
        }
        let constraints = ScriptedConstraints {
            max_width: Some(rect.width),
            max_height: Some(rect.height),
        };
        self.record_call();
        let size = self.widget.measure(ctx, constraints);
        *self.measure_cache.borrow_mut() = Some((key, size));
        size
    }

    /// `paint`'s answer for `(props, rect, theme)`, calling
    /// [`ScriptWidget::paint`] only on a cache miss, and returns the
    /// [`Canvas`]/[`CanvasLayout`] pair the pixel/TUI paint paths both
    /// replay unchanged.
    fn resolve_paint(&self, ctx: &LayoutCtx, rect: Rect, theme: &Theme) -> (Canvas, CanvasLayout) {
        let props = self.props.borrow().clone();
        let key = PaintCacheKey {
            props,
            rect,
            theme: *theme,
        };
        {
            let cache = self.paint_cache.borrow();
            if let Some(entry) = cache.as_ref() {
                if entry.key == key {
                    return (entry.paint.as_canvas(self.id, self.focusable), entry.layout);
                }
            }
        }
        self.record_call();
        let paint = self.widget.paint(ctx, &key.props, rect);
        let canvas = paint.as_canvas(self.id, self.focusable);
        let layout = canvas.layout(rect);
        *self.paint_cache.borrow_mut() = Some(PaintCacheEntry { key, paint, layout });
        (canvas, layout)
    }
}

impl<W: ScriptWidget> A11yPrimitive for ScriptedPrimitive<W> {
    fn a11y_info(&self) -> A11yInfo {
        self.resolve_a11y().map(|a| a.info).unwrap_or_default()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.resolve_a11y().map(|a| a.children).unwrap_or_default()
    }
}

impl<W: ScriptWidget> Primitive for ScriptedPrimitive<W> {
    type Layout = ScriptedLayout;
    type Event = ScriptedInputEvent;

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ScriptedLayout {
        let size = self.resolve_size(ctx, rect);
        ScriptedLayout { rect, size }
    }

    fn paint(
        &self,
        layout: &ScriptedLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        _style: &Style,
    ) {
        let ctx = LayoutCtx {
            line_height: surface.surface_line_height(),
            char_width: surface.surface_char_width(),
            nerd_fonts_enabled: false,
        };
        let (canvas, canvas_layout) = self.resolve_paint(&ctx, layout.rect, theme);
        native_surface_paint::paint(&canvas, &canvas_layout, surface);
    }

    // No `#[cfg(feature = "tui")]` here: unlike `quadraui`'s own
    // `Primitive::paint_cells` (cfg'd because `quadraui` builds with or
    // without its `tui` cargo feature), this crate's own
    // `Cargo.toml` turns `quadraui`'s `tui` feature on unconditionally, so
    // the trait method this overrides always exists when this crate
    // compiles at all.
    fn paint_cells(
        &self,
        layout: &ScriptedLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &Theme,
    ) {
        let ctx = LayoutCtx {
            line_height: 1.0,
            char_width: 1.0,
            nerd_fonts_enabled: false,
        };
        let (canvas, _) = self.resolve_paint(&ctx, layout.rect, theme);
        let area = q_rect_to_ratatui(layout.rect);
        quadraui::tui::draw_canvas(buf, area, &canvas, theme);
    }
}

/// Converts a `quadraui::Rect` (f32, native-unit) into the
/// `ratatui::layout::Rect` (u16 cells) [`quadraui::tui::draw_canvas`]
/// takes — same rounding [`quadraui::tui::backend`]'s own (crate-private)
/// conversion uses, reproduced here since that helper isn't part of this
/// crate's public surface.
fn q_rect_to_ratatui(r: Rect) -> ratatui::layout::Rect {
    let x = r.x.max(0.0).round() as u16;
    let y = r.y.max(0.0).round() as u16;
    let w = r.width.max(0.0).round() as u16;
    let h = r.height.max(0.0).round() as u16;
    ratatui::layout::Rect::new(x, y, w, h)
}

/// Lets [`ScriptedPrimitive`] sit directly in [`crate::render::RenderFrame::extra`]
/// — the existing additive-primitive seam — with no central match arm (see
/// module doc, "Why no central engine/render dispatch hook").
impl<W: ScriptWidget + 'static> ExtraPaint for ScriptedPrimitive<W> {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let _ = backend.draw_primitive(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-widget bounds check — `true` whenever the point lands
    /// anywhere inside the widget's own body, including outside every
    /// declared region (same posture [`CanvasHit::Inside`] already has for
    /// a real `Canvas` leaf). Resolving a click to a *specific* region is
    /// [`Self::hit_test`]'s job, not this trait's `bool`-only answer.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        let _ = area;
        ScriptedPrimitive::hit_test(self, point) != CanvasHit::Outside
    }

    fn tab_stop(&self) -> bool {
        self.focusable
    }

    /// Calls [`ScriptWidget::measure`] directly with no size constraint on
    /// either axis — unlike [`Primitive::layout`], not routed through
    /// [`Self::resolve_size`]'s per-frame `(ctx, rect)` cache, since there
    /// is no real `rect` here to key on (`crate::render::layout_container_rects`
    /// calls this once, before any rect is resolved, to decide how much
    /// space to give this leaf in the first place) and, unlike a per-frame
    /// paint, this is not called every frame.
    fn natural_size(&self, backend: &mut dyn Backend) -> NaturalSize {
        let ctx = LayoutCtx {
            line_height: backend.line_height(),
            char_width: backend.char_width(),
            nerd_fonts_enabled: backend.nerd_fonts_enabled(),
        };
        self.record_call();
        let size = self.widget.measure(&ctx, ScriptedConstraints::default());
        NaturalSize::new(Some(size.width), Some(size.height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::event::Viewport;
    use quadraui::{A11yRole, Color, FontRole};

    /// A scripted widget built directly in Rust test code, the way a
    /// binding would build one from its script runtime — a single
    /// coloured swatch with one clickable region covering its right half,
    /// and an `A11yRole::Button` accessibility node.
    struct Swatch;

    impl ScriptWidget for Swatch {
        fn measure(&self, _ctx: &LayoutCtx, constraints: ScriptedConstraints) -> ScriptedSize {
            ScriptedSize {
                width: constraints.max_width.unwrap_or(10.0),
                height: constraints.max_height.unwrap_or(4.0),
            }
        }

        fn paint(&self, _ctx: &LayoutCtx, props: &ScriptedProps, rect: Rect) -> ScriptedPaint {
            let color = props.get("color").and_then(|v| v.as_str()).unwrap_or("red");
            let fill = match color {
                "blue" => Color::rgb(0, 0, 200),
                _ => Color::rgb(200, 0, 0),
            };
            ScriptedPaint {
                ops: vec![DrawOp::Rect {
                    rect: Rect::new(0.0, 0.0, rect.width, rect.height),
                    color: fill,
                }],
                regions: vec![CanvasRegion {
                    id: WidgetId::new("swatch:button"),
                    rect: Rect::new(rect.width / 2.0, 0.0, rect.width / 2.0, rect.height),
                    cursor: None,
                }],
            }
        }

        fn on_event(
            &self,
            event: &ScriptedInputEvent,
            _props: &ScriptedProps,
            _state: &ScriptedState,
        ) -> bool {
            matches!(
                event,
                ScriptedInputEvent::Canvas(CanvasEvent::RegionClicked { .. })
            )
        }

        fn a11y(&self, _props: &ScriptedProps) -> Option<ScriptedA11y> {
            Some(ScriptedA11y {
                info: A11yInfo::new()
                    .with_role(A11yRole::Button)
                    .with_label("Swatch"),
                children: Vec::new(),
            })
        }
    }

    fn ctx() -> LayoutCtx {
        LayoutCtx {
            line_height: 16.0,
            char_width: 8.0,
            nerd_fonts_enabled: false,
        }
    }

    #[derive(Default)]
    struct RecordingSurface {
        fills: Vec<(Rect, Color)>,
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
        fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
        fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
        fn surface_stroke_rounded_rect(
            &mut self,
            _rect: Rect,
            _radius: f32,
            _color: Color,
            _stroke_width: f32,
        ) {
        }
        fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
        fn surface_draw_text_run_with_role(
            &mut self,
            _rect: Rect,
            _text: &str,
            _color: Color,
            _role: FontRole,
            _italic: bool,
        ) {
        }
        fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _width: f32) {}
        fn surface_push_clip(&mut self, _rect: Rect) {}
        fn surface_pop_clip(&mut self) {}
        fn surface_draw_image(
            &mut self,
            _rect: Rect,
            _image: &quadraui::Image,
        ) -> quadraui::backend::ImagePaintResult {
            quadraui::backend::ImagePaintResult::Unsupported
        }
    }

    fn swatch() -> ScriptedPrimitive<Swatch> {
        let primitive = ScriptedPrimitive::new(WidgetId::new("swatch"), Swatch, true);
        primitive.set_props(serde_json::json!({ "color": "blue" }));
        primitive
    }

    #[test]
    fn paints_the_scripts_ops_onto_a_paint_surface() {
        let primitive = swatch();
        let rect = Rect::new(0.0, 0.0, 10.0, 4.0);
        let layout = primitive.layout(&ctx(), rect);
        let mut surface = RecordingSurface::default();
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &Theme::default(),
            &Style::default(),
        );

        assert_eq!(surface.fills.len(), 1);
        assert_eq!(surface.fills[0].0, rect);
        assert_eq!(surface.fills[0].1, Color::rgb(0, 0, 200));
    }

    #[test]
    fn paints_the_scripts_ops_onto_a_tui_cell_buffer() {
        let primitive = swatch();
        let rect = Rect::new(0.0, 0.0, 10.0, 4.0);
        let layout = primitive.layout(&ctx(), rect);
        let area = ratatui::layout::Rect::new(0, 0, 10, 4);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        primitive.paint_cells(&layout, &mut buf, &Theme::default());

        assert_ne!(buf[(0, 0)].symbol().chars().next().unwrap_or(' '), ' ');
    }

    #[test]
    fn a_region_click_routes_to_on_event() {
        let primitive = swatch();
        let rect = Rect::new(0.0, 0.0, 10.0, 4.0);
        let mut surface = RecordingSurface::default();
        let layout = primitive.layout(&ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &Theme::default(),
            &Style::default(),
        );

        // The region covers the right half: `(8.0, 2.0)` lands inside it,
        // `(1.0, 2.0)` doesn't.
        assert_eq!(
            primitive.hit_test(Point::new(8.0, 2.0)),
            CanvasHit::Region {
                id: WidgetId::new("swatch:button")
            }
        );
        assert_eq!(primitive.hit_test(Point::new(1.0, 2.0)), CanvasHit::Inside);

        let handled = primitive.click_region(
            WidgetId::new("swatch:button"),
            MouseButton::Left,
            Modifiers::default(),
            Point::new(8.0, 2.0),
            false,
        );
        assert!(handled);
    }

    #[test]
    fn reports_the_scripts_a11y_node() {
        let primitive = swatch();
        assert_eq!(primitive.a11y_info().a11y_role, Some(A11yRole::Button));
        assert_eq!(primitive.a11y_info().a11y_label, Some("Swatch".to_string()));
        assert!(primitive.a11y_children().is_empty());
    }

    #[test]
    fn unchanged_props_make_zero_script_calls_on_a_second_frame() {
        let primitive = swatch();
        let rect = Rect::new(0.0, 0.0, 10.0, 4.0);
        let mut surface = RecordingSurface::default();

        let layout = primitive.layout(&ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &Theme::default(),
            &Style::default(),
        );
        let _ = primitive.a11y_info();
        let after_first_frame = primitive.call_count();
        assert!(after_first_frame > 0, "first frame should call the script");

        // Second frame: identical ctx/rect/theme/props.
        let layout = primitive.layout(&ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &Theme::default(),
            &Style::default(),
        );
        let _ = primitive.a11y_info();

        assert_eq!(
            primitive.call_count(),
            after_first_frame,
            "an unchanged frame must make zero further script calls"
        );
    }

    #[test]
    fn changed_props_call_the_script_again() {
        let primitive = swatch();
        let rect = Rect::new(0.0, 0.0, 10.0, 4.0);
        let mut surface = RecordingSurface::default();

        let layout = primitive.layout(&ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &Theme::default(),
            &Style::default(),
        );
        let after_first_frame = primitive.call_count();

        primitive.set_props(serde_json::json!({ "color": "red" }));
        let layout = primitive.layout(&ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &Theme::default(),
            &Style::default(),
        );

        assert!(primitive.call_count() > after_first_frame);
        assert_eq!(surface.fills.last().unwrap().1, Color::rgb(200, 0, 0));
    }
}
