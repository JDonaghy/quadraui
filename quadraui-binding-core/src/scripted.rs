//! [`ScriptedPrimitive`]: a widget backed by a script-side widget
//! definition — `measure`/`paint`/`on_event`/`a11y` callbacks a binding
//! forwards into its host language's script runtime, instead of Rust code.
//! It implements [`quadraui::primitive::Primitive`] and
//! [`quadraui::Interactive`], so it is an ordinary
//! [`quadraui::primitive::DynWidget`]: it decodes as a generic
//! [`crate::layout::LeafDescriptor::Widget`] leaf, paints through
//! [`quadraui::DrawsPrimitives::draw_dyn`] and receives input through the
//! engine's generic widget routing, exactly like a built-in kind.
//!
//! # Display-list reuse — no new op set
//!
//! [`ScriptWidget::paint`] answers with the same [`quadraui::DrawOp`]/
//! [`quadraui::CanvasRegion`] vocabulary a [`quadraui::Canvas`] descriptor
//! already carries. [`ScriptedPrimitive`] builds a `Canvas` from that
//! answer and paints it through the identical code every built-in `Canvas`
//! leaf paints through:
//! [`quadraui::primitives::canvas::native_surface_paint::paint`] on a
//! pixel backend's [`quadraui::PaintSurface`], `Canvas`'s own
//! `Primitive::paint_cells` on TUI's cell grid. A press is resolved to a
//! region by `Canvas`'s own [`quadraui::Interactive::handle`] the same way,
//! so a script-defined widget reports the same
//! [`quadraui::UiEvent::Canvas`] a `Canvas` leaf does.
//!
//! # Caching — one FFI round trip per widget per frame at most
//!
//! A script call crosses a real FFI boundary in a real binding, so
//! [`ScriptedPrimitive`] never calls back into the script more than once
//! per frame per cache key. [`quadraui::primitive::Primitive::layout`]
//! caches `measure`'s answer keyed on `(ctx, rect)`;
//! [`quadraui::primitive::Primitive::paint`]/`paint_cells` cache `paint`'s
//! answer keyed on `(props, rect, theme)`;
//! [`quadraui::primitive::Primitive::measure`] caches `measure`'s
//! unconstrained answer keyed on `ctx` alone (it runs once per
//! layout/paint pass, before any rect exists to key on) — all three skip
//! the script call entirely when a later frame's key matches the cached
//! one. [`ScriptedPrimitive::call_count`] reports the running total, for a
//! test to assert a repeated frame with an unchanged key made no further
//! calls.
//!
//! # Splicing into a decoded tree without a `Deserialize` impl
//!
//! A real [`ScriptWidget`] wraps a live host-language callback object —
//! there is no sensible `Deserialize` for that, so a `"scripted_widget"`
//! leaf can't build one from its payload the way a built-in kind does.
//! [`register_scripted_widget`] is the side channel instead: a binding
//! builds its [`ScriptedPrimitive<W>`] once (wrapping its own live
//! callback object) and registers it under its own id. The
//! `"scripted_widget"` kind itself is registered in core's runtime kind
//! registry ([`quadraui::register_widget_kind`]); its decoder reads the
//! payload's `"id"`, looks the registration up, applies that frame's
//! `"props"` and hands back a handle to the SAME widget — never a fresh
//! instance, so its caches and the callback object it wraps survive every
//! frame's decode.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use quadraui::primitive::{DynWidget, LayoutCtx, Primitive, Size};
use quadraui::primitives::canvas::native_surface_paint;
use quadraui::{
    A11yChild, A11yInfo, A11yPrimitive, Canvas, CanvasEvent, CanvasHit, CanvasLayout, CanvasRegion,
    CanvasState, DrawOp, EventCx, FocusSpec, Handled, Interactive, Key, KeyEvent, NaturalSize,
    PaintSurface, Point, PointerKind, Rect, RoutedEvent, Theme, WidgetId,
};

/// The widget kind a `"scripted_widget"` layout-tree leaf names. Registered
/// in core's runtime kind registry by [`ensure_scripted_widget_kind`].
pub const SCRIPTED_WIDGET_KIND: &str = "scripted_widget";

// ─── The scripted-widget registry ──────────────────────────────────────────
//
// A side channel from a binding's own callback-object wrapping (see module
// doc, "Splicing into a decoded tree without a `Deserialize` impl") to the
// `"scripted_widget"` kind's decoder. `thread_local!`, not a
// `static`-plus-`Mutex`: a `ScriptedPrimitive<W>` is neither `Send` nor
// `Sync` (it holds its callback object and caches behind `Rc`/`RefCell`),
// and a binding's render pass — the only code that registers or decodes
// one — runs on a single OS thread in every binding (the GTK/TUI event
// loop's own thread; a Python render callback runs under the GIL, which
// serializes it onto whichever thread called in).

/// Applies one frame's props to a registered widget and hands back a handle
/// to it.
type ScriptedFactory = Rc<dyn Fn(ScriptedProps) -> Box<dyn DynWidget>>;

thread_local! {
    static SCRIPTED_WIDGETS: RefCell<HashMap<String, ScriptedFactory>> =
        RefCell::new(HashMap::new());
}

/// Registers the `"scripted_widget"` kind in core's runtime kind registry,
/// once per process. Every binding-core entry point that decodes or names
/// leaf kinds calls this first, so the kind is always decodable and never
/// claimable by a host's own [`crate::register_widget_kind`].
pub fn ensure_scripted_widget_kind() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        #[cfg(feature = "schema")]
        let registered = quadraui::register_widget_kind_with_schema(
            SCRIPTED_WIDGET_KIND,
            decode_scripted_widget,
            scripted_widget_schema,
        );
        #[cfg(not(feature = "schema"))]
        let registered =
            quadraui::register_widget_kind(SCRIPTED_WIDGET_KIND, decode_scripted_widget);
        if let Err(err) = registered {
            panic!("{SCRIPTED_WIDGET_KIND:?} must be free in the kind registry: {err}");
        }
    });
}

/// The `"scripted_widget"` kind's decoder: looks the payload's `"id"` up in
/// this thread's registrations and returns the registered widget with
/// `"props"` applied.
fn decode_scripted_widget(
    _kind: &str,
    value: serde_json::Value,
) -> Result<Box<dyn DynWidget>, serde_json::Error> {
    let descriptor: ScriptedWidgetDescriptor = serde_json::from_value(value)?;
    let factory = SCRIPTED_WIDGETS
        .with(|registry| registry.borrow().get(&descriptor.id).cloned())
        .ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom(format!(
                "no scripted widget is registered for id {:?} — the binding must call \
                 quadraui_binding_core::register_scripted_widget before decoding a tree \
                 containing it",
                descriptor.id
            ))
        })?;
    Ok(factory(descriptor.props))
}

#[cfg(feature = "schema")]
fn scripted_widget_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(ScriptedWidgetDescriptor))
        .expect("a derived schema serializes")
}

/// Registers (replacing any previous registration under the same id)
/// `widget` under its own [`WidgetId`], for a `"scripted_widget"` leaf whose
/// payload names that id to decode into — on this and every later decode,
/// until [`unregister_scripted_widget`] or another registration under the
/// same id replaces it. Call this on the same thread the decode itself runs
/// on — see this module's "The scripted-widget registry" note.
pub fn register_scripted_widget<W: ScriptWidget + 'static>(widget: ScriptedPrimitive<W>) {
    ensure_scripted_widget_kind();
    let id = widget.inner.id.as_str().to_string();
    let factory: ScriptedFactory = Rc::new(move |props| {
        widget.set_props(props);
        Box::new(widget.clone())
    });
    SCRIPTED_WIDGETS.with(|registry| {
        registry.borrow_mut().insert(id, factory);
    });
}

/// Drops `id`'s registration, if any — a binding calls this once the
/// script-authored widget it wraps is torn down, so a later
/// `"scripted_widget"` leaf that still names `id` fails decode with a clear
/// "not registered" [`crate::DecodeError`] instead of resurrecting a stale,
/// logically-dead widget.
pub fn unregister_scripted_widget(id: &str) {
    SCRIPTED_WIDGETS.with(|registry| {
        registry.borrow_mut().remove(id);
    });
}

/// The payload of a `"scripted_widget"` layout-tree leaf: the id the widget
/// was registered under through [`register_scripted_widget`] and this
/// frame's `"props"`, defaulting to `Value::Null` when absent. The
/// `measure`/`paint`/`on_event`/`a11y` callbacks live on the registered
/// [`ScriptWidget`] object, not here (see this module's doc, "Splicing into
/// a decoded tree without a `Deserialize` impl").
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScriptedWidgetDescriptor {
    pub id: String,
    #[serde(default)]
    pub props: ScriptedProps,
}

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
/// distinct even though both are just a `serde_json::Value`.
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

/// A press/key input routed to [`ScriptWidget::on_event`] by
/// [`ScriptedPrimitive`]'s [`Interactive::handle`]. Wraps [`CanvasEvent`]
/// for the pointer cases rather than re-declaring
/// `RegionClicked`/`RegionDoubleClicked` — the same region-click
/// vocabulary a real `Canvas` leaf's `UiEvent::Canvas` already carries.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptedInputEvent {
    /// A declared region was clicked or double-clicked.
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
/// [`quadraui::primitive::Primitive`]'s own "layout bakes in what paint
/// needs" design. Region hit-testing lives on [`ScriptedPrimitive`] itself
/// instead of here, since it depends on `paint`'s answer, not `measure`'s
/// — see [`ScriptedPrimitive::hit_test`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptedLayout {
    pub rect: Rect,
    pub size: ScriptedSize,
}

#[derive(Debug, Clone, PartialEq)]
struct MeasureCacheKey {
    ctx: LayoutCtx<'static>,
    rect: Rect,
}

/// `ctx`'s three scalar metrics with no text measurer attached — the
/// `'static` form a cache key can own. [`LayoutCtx`]'s own `PartialEq`
/// compares exactly those metrics and ignores the measurer, so a key
/// built this way still matches the live ctx it came from.
///
/// That measurer-blindness is also the cache's one staleness window: a
/// script widget that sizes text through [`LayoutCtx::text_width`] keeps
/// hitting this cache across a font change that leaves the three scalars
/// untouched. No scripted widget measures text.
fn cache_key_ctx(ctx: &LayoutCtx) -> LayoutCtx<'static> {
    LayoutCtx::new(ctx.line_height, ctx.char_width, ctx.nerd_fonts_enabled)
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
    /// Absolute layout the cached `paint` was resolved against, so
    /// [`ScriptedPrimitive::hit_test`] resolves against the same
    /// [`CanvasLayout`] `paint`/`paint_cells` last used.
    layout: CanvasLayout,
}

/// The shared body of a [`ScriptedPrimitive`]: every clone of one handle
/// reads and writes the same props, state and caches.
struct ScriptedInner<W> {
    id: WidgetId,
    widget: W,
    focusable: bool,
    props: RefCell<ScriptedProps>,
    state: RefCell<ScriptedState>,
    measure_cache: RefCell<Option<(MeasureCacheKey, ScriptedSize)>>,
    paint_cache: RefCell<Option<PaintCacheEntry>>,
    natural_size_cache: RefCell<Option<(LayoutCtx<'static>, ScriptedSize)>>,
    a11y_cache: RefCell<Option<(ScriptedProps, Option<ScriptedA11y>)>>,
    /// Total number of calls made into [`Self::widget`] across
    /// `measure`/`paint`/`on_event`/`a11y` — see module doc "Caching".
    call_count: AtomicU64,
}

/// A widget backed by a [`ScriptWidget`]. See the module doc for the full
/// contract. Cloning yields another handle to the same widget: props,
/// script state, caches and the call count are shared.
pub struct ScriptedPrimitive<W: ScriptWidget> {
    inner: Rc<ScriptedInner<W>>,
}

impl<W: ScriptWidget> Clone for ScriptedPrimitive<W> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<W: ScriptWidget> ScriptedPrimitive<W> {
    /// Builds a primitive around `widget`, with no props/state set yet
    /// (both start as `serde_json::Value::Null`) and `focusable` matching
    /// [`quadraui::Canvas::focusable`]'s own "declarative flag" contract:
    /// a focusable scripted widget is one tab stop.
    pub fn new(id: WidgetId, widget: W, focusable: bool) -> Self {
        ScriptedPrimitive {
            inner: Rc::new(ScriptedInner {
                id,
                widget,
                focusable,
                props: RefCell::new(serde_json::Value::Null),
                state: RefCell::new(serde_json::Value::Null),
                measure_cache: RefCell::new(None),
                paint_cache: RefCell::new(None),
                natural_size_cache: RefCell::new(None),
                a11y_cache: RefCell::new(None),
                call_count: AtomicU64::new(0),
            }),
        }
    }

    /// Updates the props a host re-renders this widget with each frame.
    /// Does not itself call into the script — a later `layout`/`paint`
    /// call does that, and only on a cache miss (module doc, "Caching").
    pub fn set_props(&self, props: ScriptedProps) {
        *self.inner.props.borrow_mut() = props;
    }

    /// The props last set via [`Self::set_props`] (`Value::Null` until the
    /// first call) — read back by a host that needs to confirm what a
    /// wire decode applied.
    pub fn props(&self) -> ScriptedProps {
        self.inner.props.borrow().clone()
    }

    /// Updates the widget state [`ScriptWidget::on_event`] reads back.
    pub fn set_state(&self, state: ScriptedState) {
        *self.inner.state.borrow_mut() = state;
    }

    /// Total script calls made so far — see module doc "Caching".
    pub fn call_count(&self) -> u64 {
        self.inner.call_count.load(Ordering::Relaxed)
    }

    fn record_call(&self) {
        self.inner.call_count.fetch_add(1, Ordering::Relaxed);
    }

    /// The [`Canvas`] + [`CanvasLayout`] `paint`/`paint_cells` last
    /// resolved — `None` until the first `paint`/`paint_cells` call, since
    /// the display list (and so the regions a click can resolve against)
    /// doesn't exist before that.
    fn canvas(&self) -> Option<(Canvas, CanvasLayout)> {
        self.inner.paint_cache.borrow().as_ref().map(|entry| {
            (
                entry.paint.as_canvas(self.inner.id, self.inner.focusable),
                entry.layout,
            )
        })
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

    /// Hands `event` to [`ScriptWidget::on_event`] — always a real script
    /// call, never cached: each dispatched event is its own round trip,
    /// unlike the per-frame display list. Returns whether the script
    /// consumed it.
    fn deliver(&self, event: ScriptedInputEvent) -> bool {
        self.record_call();
        self.inner.widget.on_event(
            &event,
            &self.inner.props.borrow(),
            &self.inner.state.borrow(),
        )
    }

    /// The script's accessibility node for the current props, calling
    /// [`ScriptWidget::a11y`] only when `props` has changed since the last
    /// call (module doc, "Caching").
    fn resolve_a11y(&self) -> Option<ScriptedA11y> {
        let props = self.inner.props.borrow().clone();
        if let Some((cached_props, cached)) = self.inner.a11y_cache.borrow().as_ref() {
            if *cached_props == props {
                return cached.clone();
            }
        }
        self.record_call();
        let result = self.inner.widget.a11y(&props);
        *self.inner.a11y_cache.borrow_mut() = Some((props, result.clone()));
        result
    }

    /// `measure`'s answer for `(ctx, rect)`, calling
    /// [`ScriptWidget::measure`] only on a cache miss.
    fn resolve_size(&self, ctx: &LayoutCtx, rect: Rect) -> ScriptedSize {
        let key = MeasureCacheKey {
            ctx: cache_key_ctx(ctx),
            rect,
        };
        if let Some((cached_key, cached_size)) = self.inner.measure_cache.borrow().as_ref() {
            if *cached_key == key {
                return *cached_size;
            }
        }
        let constraints = ScriptedConstraints {
            max_width: Some(rect.width),
            max_height: Some(rect.height),
        };
        self.record_call();
        let size = self.inner.widget.measure(ctx, constraints);
        *self.inner.measure_cache.borrow_mut() = Some((key, size));
        size
    }

    /// [`Primitive::measure`]'s answer for `ctx`, calling
    /// [`ScriptWidget::measure`] unconstrained only on a cache miss. Keyed
    /// on `ctx` alone — the layout solver measures a content-sized leaf once
    /// per layout/paint pass, before any rect is resolved, so this answer
    /// is cached exactly like `paint`'s is, in a cache separate from
    /// [`Self::resolve_size`]'s `(ctx, rect)` one.
    fn resolve_natural_size(&self, ctx: &LayoutCtx) -> ScriptedSize {
        if let Some((cached_ctx, cached_size)) = self.inner.natural_size_cache.borrow().as_ref() {
            if *cached_ctx == *ctx {
                return *cached_size;
            }
        }
        self.record_call();
        let size = self
            .inner
            .widget
            .measure(ctx, ScriptedConstraints::default());
        *self.inner.natural_size_cache.borrow_mut() = Some((cache_key_ctx(ctx), size));
        size
    }

    /// `paint`'s answer for `(props, rect, theme)`, calling
    /// [`ScriptWidget::paint`] only on a cache miss, and returns the
    /// [`Canvas`]/[`CanvasLayout`] pair the pixel/TUI paint paths both
    /// replay unchanged.
    fn resolve_paint(&self, ctx: &LayoutCtx, rect: Rect, theme: &Theme) -> (Canvas, CanvasLayout) {
        let props = self.inner.props.borrow().clone();
        let key = PaintCacheKey {
            props,
            rect,
            theme: *theme,
        };
        {
            let cache = self.inner.paint_cache.borrow();
            if let Some(entry) = cache.as_ref() {
                if entry.key == key {
                    return (
                        entry.paint.as_canvas(self.inner.id, self.inner.focusable),
                        entry.layout,
                    );
                }
            }
        }
        self.record_call();
        let paint = self.inner.widget.paint(ctx, &key.props, rect);
        let canvas = paint.as_canvas(self.inner.id, self.inner.focusable);
        let layout = canvas.layout(rect);
        *self.inner.paint_cache.borrow_mut() = Some(PaintCacheEntry { key, paint, layout });
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

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.inner.id)
    }

    /// The script's unconstrained size, through
    /// [`Self::resolve_natural_size`]'s `ctx`-keyed cache, so an unchanged
    /// `ctx` never re-enters the script.
    fn measure(&self, ctx: &LayoutCtx, _avail: Size) -> NaturalSize {
        let size = self.resolve_natural_size(ctx);
        NaturalSize::new(Some(size.width), Some(size.height))
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ScriptedLayout {
        let size = self.resolve_size(ctx, rect);
        ScriptedLayout { rect, size }
    }

    fn paint(
        &self,
        layout: &ScriptedLayout,
        surface: &mut dyn PaintSurface,
        env: &quadraui::PaintEnv,
    ) {
        let theme = env.theme();
        let style = env.style();
        let ctx = LayoutCtx::new(
            LayoutCtx::resolve_row_height(surface.surface_line_height(), style),
            surface.surface_char_width(),
            false,
        );
        let (canvas, resolved) = self.resolve_paint(&ctx, layout.rect, theme);
        native_surface_paint::paint(&canvas, &resolved, surface);
    }

    // No `#[cfg(feature = "tui")]` here: unlike `quadraui`'s own
    // `Primitive::paint_cells` (cfg'd because `quadraui` builds with or
    // without its `tui` cargo feature), this crate's own `Cargo.toml` turns
    // `quadraui`'s `tui` feature on unconditionally, so the trait method
    // this overrides always exists when this crate compiles at all.
    fn paint_cells(
        &self,
        layout: &ScriptedLayout,
        cells: &mut dyn quadraui::CellSurface,
        env: &quadraui::PaintEnv,
    ) {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let (canvas, resolved) = self.resolve_paint(&ctx, layout.rect, env.theme());
        canvas.paint_cells(&resolved, cells, env);
    }
}

impl<W: ScriptWidget + 'static> Interactive for ScriptedPrimitive<W> {
    /// The painted canvas's own gesture state.
    type State = CanvasState;
    /// The region event the painted canvas reports, delivered to the app
    /// as the same [`quadraui::UiEvent::Canvas`] a `Canvas` leaf produces.
    type Action = CanvasEvent;

    /// Every move inside the widget reaches [`Self::handle`], so the
    /// hovered region follows the pointer.
    const RECEIVES_HOVER_MOVES: bool = true;

    /// One tab stop when built `focusable`; otherwise none.
    fn focus(&self) -> FocusSpec {
        if self.inner.focusable {
            FocusSpec::Stop
        } else {
            FocusSpec::None
        }
    }

    /// - **Press** / **double-click** resolves through the last-painted
    ///   [`Canvas`]'s own [`Interactive::handle`]: on a declared region it
    ///   reports [`CanvasEvent::RegionClicked`] /
    ///   [`CanvasEvent::RegionDoubleClicked`] and hands that same event to
    ///   [`ScriptWidget::on_event`]; on bare canvas it is ignored and never
    ///   reaches the script.
    /// - **Hover** resolves through the same canvas, which reports
    ///   [`CanvasEvent::RegionHovered`] whenever the region under the
    ///   pointer changes. Hover never reaches the script.
    /// - **Key** (routed only while this widget holds focus) goes to
    ///   [`ScriptWidget::on_event`] and is otherwise left to travel on, so
    ///   the app still sees the key press.
    ///
    /// Whenever the script reports an event consumed, the host repaints.
    fn handle(
        &self,
        state: &mut CanvasState,
        event: &RoutedEvent,
        layout: &ScriptedLayout,
        cx: &mut EventCx,
    ) -> Handled<CanvasEvent> {
        match event {
            RoutedEvent::Pointer { kind, .. } => {
                let Some((canvas, _)) = self.canvas() else {
                    return Handled::Ignored;
                };
                let canvas_layout = canvas.layout(layout.rect);
                let handled = Interactive::handle(&canvas, state, event, &canvas_layout, cx);
                let press = matches!(kind, PointerKind::Down(_) | PointerKind::DoubleClick(_));
                if let (true, Handled::Event(region_event)) = (press, &handled) {
                    if self.deliver(ScriptedInputEvent::Canvas(region_event.clone())) {
                        cx.request_redraw();
                    }
                }
                handled
            }
            RoutedEvent::Key(KeyEvent { key, .. }) => {
                if self.deliver(ScriptedInputEvent::Key(key.clone())) {
                    cx.request_redraw();
                }
                Handled::Ignored
            }
            _ => Handled::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::event::Viewport;
    use quadraui::{A11yRole, Color, EventHost, FontRole, Modifiers, MouseButton, Style};

    /// Records the repaint request a widget makes through its [`EventCx`].
    #[derive(Default)]
    struct TestHost {
        redraw: bool,
    }

    impl EventHost for TestHost {
        fn is_focused(
            &self,
            _id: &WidgetId,
            _sub: Option<quadraui::interactive::SubTarget>,
        ) -> bool {
            true
        }
        fn request_focus(
            &mut self,
            _id: &WidgetId,
            _sub: Option<quadraui::interactive::SubTarget>,
        ) {
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: quadraui::PointerShape) {}
        fn request_redraw(&mut self) {
            self.redraw = true;
        }
        fn emit(&mut self, _event: quadraui::UiEvent) {}
        fn units(&self) -> quadraui::primitive::Units {
            quadraui::primitive::Units::Pixels
        }
    }

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

    fn ctx() -> LayoutCtx<'static> {
        LayoutCtx::new(16.0, 8.0, false)
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
        let layout = Primitive::layout(&primitive, &ctx(), rect);
        let mut surface = RecordingSurface::default();
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &quadraui::PaintEnv::new(&Theme::default(), &Style::default()),
        );

        assert_eq!(surface.fills.len(), 1);
        assert_eq!(surface.fills[0].0, rect);
        assert_eq!(surface.fills[0].1, Color::rgb(0, 0, 200));
    }

    #[test]
    fn paints_the_scripts_ops_onto_a_tui_cell_buffer() {
        let primitive = swatch();
        let rect = Rect::new(0.0, 0.0, 10.0, 4.0);
        let layout = Primitive::layout(&primitive, &ctx(), rect);
        let area = ratatui::layout::Rect::new(0, 0, 10, 4);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        primitive.paint_cells(
            &layout,
            &mut quadraui::tui::BufferCellSurface::new(&mut buf),
            &quadraui::PaintEnv::new(&Theme::default(), &quadraui::Style::default()),
        );

        assert_eq!(buf[(0, 0)].bg, ratatui::style::Color::Rgb(0, 0, 200));
    }

    #[test]
    fn a_region_click_routes_to_on_event() {
        let primitive = swatch();
        let rect = Rect::new(0.0, 0.0, 10.0, 4.0);
        let mut surface = RecordingSurface::default();
        let layout = Primitive::layout(&primitive, &ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &quadraui::PaintEnv::new(&Theme::default(), &Style::default()),
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

        let mut host = TestHost::default();
        let mut cx = EventCx::new(WidgetId::new("swatch"), &mut host);
        let mut state = CanvasState::default();
        let handled = Interactive::handle(
            &primitive,
            &mut state,
            &RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(8.0, 2.0),
                mods: Modifiers::default(),
            },
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(CanvasEvent::RegionClicked {
                region: WidgetId::new("swatch:button"),
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
                x: 8.0,
                y: 2.0,
            })
        );
        assert!(
            host.redraw,
            "the script consumed the click, so the host repaints"
        );
    }

    /// A press on bare canvas, outside every declared region, is ignored
    /// and never reaches the script.
    #[test]
    fn a_press_outside_every_region_never_reaches_the_script() {
        let primitive = swatch();
        let rect = Rect::new(0.0, 0.0, 10.0, 4.0);
        let layout = Primitive::layout(&primitive, &ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut RecordingSurface::default(),
            &quadraui::PaintEnv::new(&Theme::default(), &Style::default()),
        );
        let before = primitive.call_count();
        let mut host = TestHost::default();
        let mut cx = EventCx::new(WidgetId::new("swatch"), &mut host);
        let handled = Interactive::handle(
            &primitive,
            &mut CanvasState::default(),
            &RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(1.0, 2.0),
                mods: Modifiers::default(),
            },
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Ignored);
        assert_eq!(primitive.call_count(), before);
    }

    /// The `"scripted_widget"` kind decodes through core's generic
    /// `decode_widget`, into a handle to the very widget the binding
    /// registered, with that frame's props applied.
    #[test]
    fn a_registered_scripted_widget_decodes_through_the_core_kind_registry() {
        let primitive = ScriptedPrimitive::new(WidgetId::new("decoded-swatch"), Swatch, true);
        register_scripted_widget(primitive.clone());
        let decoded = quadraui::decode_widget(
            SCRIPTED_WIDGET_KIND,
            serde_json::json!({ "id": "decoded-swatch", "props": { "color": "blue" } }),
        )
        .expect("a registered scripted widget decodes");
        let decoded = decoded
            .as_any()
            .downcast_ref::<ScriptedPrimitive<Swatch>>()
            .expect("the decode hands back the registered widget type");
        assert_eq!(primitive.props(), serde_json::json!({ "color": "blue" }));
        assert!(Rc::ptr_eq(&decoded.inner, &primitive.inner));
        unregister_scripted_widget("decoded-swatch");
        assert!(quadraui::decode_widget(
            SCRIPTED_WIDGET_KIND,
            serde_json::json!({ "id": "decoded-swatch" }),
        )
        .is_err());
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

        let layout = Primitive::layout(&primitive, &ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &quadraui::PaintEnv::new(&Theme::default(), &Style::default()),
        );
        let _ = primitive.a11y_info();
        let _ = Primitive::measure(&primitive, &ctx(), Size::new(10.0, 4.0));
        let after_first_frame = primitive.call_count();
        assert!(after_first_frame > 0, "first frame should call the script");

        // Second frame: identical ctx/rect/theme/props.
        let layout = Primitive::layout(&primitive, &ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &quadraui::PaintEnv::new(&Theme::default(), &Style::default()),
        );
        let _ = primitive.a11y_info();
        let _ = Primitive::measure(&primitive, &ctx(), Size::new(10.0, 4.0));

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

        let layout = Primitive::layout(&primitive, &ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &quadraui::PaintEnv::new(&Theme::default(), &Style::default()),
        );
        let after_first_frame = primitive.call_count();

        primitive.set_props(serde_json::json!({ "color": "red" }));
        let layout = Primitive::layout(&primitive, &ctx(), rect);
        Primitive::paint(
            &primitive,
            &layout,
            &mut surface,
            &quadraui::PaintEnv::new(&Theme::default(), &Style::default()),
        );

        assert!(primitive.call_count() > after_first_frame);
        assert_eq!(surface.fills.last().unwrap().1, Color::rgb(200, 0, 0));
    }
}
