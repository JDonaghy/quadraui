//! Proof that `Primitive` is genuinely public API, and that
//! `DrawsPrimitives::draw_primitive` honours its contract.
//!
//! This file is a separate compilation unit, so a widget it declares is a
//! widget *outside* the crate: if `Swatch` below implements
//! [`quadraui::Primitive`] and every pixel backend paints it with no change
//! to `Backend`/`gtk`/`macos`/`win`, the seam does what it claims. Sibling
//! file to `paint_surface_public_api.rs`, one level up: `PaintSurface` is
//! the unsealed paint seam, `Primitive` is the `layout()` + `paint()` pair
//! built on top of it.
//!
//! Three things are asserted:
//!
//! 1. `draw_primitive` is reachable through `&mut dyn Backend` — the shape
//!    every `compose::*` controller's `render` is handed — even though it is
//!    generic over `Primitive` and so cannot sit on `Backend` itself.
//! 2. The [`quadraui::LayoutCtx`] it builds carries the calling backend's
//!    own `chrome_line_height` / `char_width` / `nerd_fonts_enabled`.
//! 3. It paints exactly when [`quadraui::Backend::paint_surface`] answers
//!    `Some` (GTK, Win-GUI), and still returns the resolved layout when it
//!    answers `None` (TUI, for a `Primitive` impl with no
//!    [`quadraui::primitive::Primitive::paint_cells`] override), so a host
//!    can hit-test against geometry it cannot paint.
//! 4. On TUI, a `Primitive` impl that *does* override `paint_cells` gets
//!    painted too, through [`quadraui::Backend::cell_surface`] — the same
//!    `&mut dyn Backend` seam, not a concrete `&mut TuiBackend`.
//!
//! `Swatch::paint` records its call rather than issuing a paint verb: a live
//! Cairo/Direct2D render target is needed to exercise the verbs themselves.
//!
//! [`Counter`] carries the whole contract from outside the crate:
//! `Primitive` with both `paint` and `paint_cells`, and `Interactive` with a
//! focus stop, a `State` and an `Action`. Through public API only it lays
//! out, paints in cells on TUI (a `TuiDriver` app pushing it as
//! `Surface::Widget`) and in pixels on GTK (read back from a Cairo image) and
//! Win, takes focus from Tab, handles a key and a click, and answers
//! `route_widget_action` with the same event the click produces. The
//! binding half, a runtime-registered kind reached through binding-core's
//! wire decode, is `quadraui-binding-core/tests/out_of_crate_widget.rs`.

#![cfg(any(feature = "tui", feature = "gtk", feature = "win"))]

use std::cell::RefCell;

use quadraui::primitive::{LayoutCtx, Primitive};
use quadraui::{A11yInfo, A11yPrimitive, Backend, DrawsPrimitives, FontRole, PaintSurface, Rect};

/// [`Swatch`]'s fully-resolved layout: the absolute area it was laid out
/// into, plus the metrics `draw_primitive` measured off the backend for it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SwatchLayout {
    area: Rect,
    /// The metrics half of the ctx `draw_primitive` built, rebuilt with
    /// [`LayoutCtx::new`] so this layout owns a `'static` value. The
    /// measurer half is recorded separately in [`Self::measured_label`],
    /// since a `Primitive::Layout` cannot borrow from its ctx.
    ctx: LayoutCtx<'static>,
    /// `ctx.text_width(LABEL, FontRole::Chrome)` as the ctx answered it
    /// — the calling backend's own `measure_text` when `draw_primitive`
    /// attached one.
    measured_label: f32,
}

/// Text [`Swatch::layout`] measures through its ctx, so a test can
/// compare the answer against the calling backend's own
/// [`quadraui::Backend::measure_text`].
const LABEL: &str = "alpha beta";

/// A filled square, built against the public `Primitive` seam only.
struct Swatch {
    /// One entry per [`Primitive::paint`] call, so a test can tell whether
    /// the backend ran the paint half at all.
    painted: RefCell<Vec<Rect>>,
}

impl Swatch {
    fn new() -> Self {
        Self {
            painted: RefCell::new(Vec::new()),
        }
    }
}

impl A11yPrimitive for Swatch {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::default()
    }
}

impl Primitive for Swatch {
    type Layout = SwatchLayout;
    type Event = ();

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> SwatchLayout {
        SwatchLayout {
            area: rect,
            ctx: LayoutCtx::new(ctx.line_height, ctx.char_width, ctx.nerd_fonts_enabled),
            measured_label: ctx.text_width(LABEL, FontRole::Chrome),
        }
    }

    fn paint(
        &self,
        layout: &SwatchLayout,
        _surface: &mut dyn PaintSurface,
        _env: &quadraui::PaintEnv,
    ) {
        self.painted.borrow_mut().push(layout.area);
    }
}

// `Swatch` implements `quadraui::Interactive` via `quadraui::non_interactive!`
// — proof the macro, like `Primitive` itself, is genuinely usable from
// outside the crate.
quadraui::non_interactive!(Swatch);

/// Draw a `Swatch` through `&mut dyn Backend`, returning the layout
/// `draw_primitive` handed back, the areas `paint` was called with, and
/// the metrics plus [`LABEL`] width read off the same backend for
/// comparison.
///
/// `measured.line_height` runs the same backend-measured
/// `chrome_line_height` through [`LayoutCtx::resolve_row_height`] that
/// `draw_primitive` itself applies before building the `LayoutCtx` a
/// `Primitive::layout` call receives, so this matches `draw_primitive`'s
/// real row-pitch resolution rather than the raw, unfloored font metric.
fn draw_through_the_seam(
    backend: &mut dyn Backend,
    area: Rect,
) -> (SwatchLayout, Vec<Rect>, LayoutCtx<'static>, f32) {
    let style = backend.style();
    let measured = LayoutCtx::new(
        LayoutCtx::resolve_row_height(backend.chrome_line_height(), &style),
        backend.char_width(),
        backend.nerd_fonts_enabled(),
    );
    let measured_label = backend.measure_text(LABEL, FontRole::Chrome).0;
    let swatch = Swatch::new();
    let layout = backend.draw_primitive(area, &swatch);
    (
        layout,
        swatch.painted.into_inner(),
        measured,
        measured_label,
    )
}

/// Every backend resolves the layout the same way; only the paint half
/// depends on `Backend::paint_surface`.
fn assert_layout_is_resolved(
    layout: SwatchLayout,
    area: Rect,
    measured: LayoutCtx<'static>,
    measured_label: f32,
) {
    assert_eq!(
        layout.area, area,
        "draw_primitive must pass the caller's rect to Primitive::layout and hand the \
         resolved layout straight back"
    );
    assert_eq!(
        layout.ctx, measured,
        "LayoutCtx must carry the calling backend's own chrome_line_height / char_width / \
         nerd_fonts_enabled, so a Primitive impl never has to reach for &dyn Backend itself"
    );
    assert_eq!(
        layout.measured_label, measured_label,
        "LayoutCtx::text_width must answer the calling backend's own Backend::measure_text \
         for the label, not a char_width approximation of it"
    );
}

#[cfg(feature = "tui")]
#[test]
fn tui_draw_primitive_resolves_layout_without_painting() {
    use quadraui::tui::backend::TuiBackend;

    let mut backend = TuiBackend::new();
    let area = Rect::new(1.0, 2.0, 10.0, 4.0);
    let (layout, painted, measured, measured_label) = draw_through_the_seam(&mut backend, area);

    assert_layout_is_resolved(layout, area, measured, measured_label);
    assert!(
        painted.is_empty(),
        "TuiBackend answers None from Backend::paint_surface, so draw_primitive has no \
         surface to paint onto and must skip Primitive::paint — the layout it still returns \
         is what lets a TUI host reserve space for, and hit-test, a GUI-only Primitive; \
         painted areas were {painted:?}"
    );
}

/// A primitive with a real [`Primitive::paint_cells`] impl, unlike
/// [`Swatch`] (which takes the no-op default) — proves
/// `draw_primitive`'s TUI cell path is reachable through `&mut dyn
/// Backend`, not only a concrete `&mut TuiBackend`.
#[cfg(feature = "tui")]
struct CellSwatch;

#[cfg(feature = "tui")]
impl A11yPrimitive for CellSwatch {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::default()
    }
}

#[cfg(feature = "tui")]
impl Primitive for CellSwatch {
    type Layout = Rect;
    type Event = ();

    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
        rect
    }

    fn paint(&self, _layout: &Rect, _surface: &mut dyn PaintSurface, _env: &quadraui::PaintEnv) {}

    fn paint_cells(
        &self,
        layout: &Rect,
        cells: &mut dyn quadraui::CellSurface,
        _env: &quadraui::PaintEnv,
    ) {
        cells.put_char(
            layout.x as u16,
            layout.y as u16,
            'X',
            quadraui::CellSurfaceStyle::default(),
        );
    }
}

/// [`CellSwatch::paint_cells`] must run when `draw_primitive` is called
/// through `&mut dyn Backend` against a real `TuiBackend` — the shape
/// every `compose::*` controller's `render` is handed
/// ([`Backend::cell_surface`] is what makes this reachable).
#[cfg(feature = "tui")]
#[test]
fn tui_draw_primitive_paints_cells_through_dyn_backend() {
    use quadraui::tui::backend::TuiBackend;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut backend = TuiBackend::new();
    let area = Rect::new(1.0, 2.0, 10.0, 4.0);
    let mut terminal = Terminal::new(TestBackend::new(20, 10)).expect("test terminal");
    terminal
        .draw(|frame| {
            backend.enter_frame_scope(frame, |b: &mut TuiBackend| {
                let b: &mut dyn Backend = b;
                b.draw_primitive(area, &CellSwatch);
            });
        })
        .expect("draw");

    let cell = terminal
        .backend()
        .buffer()
        .cell((1, 2))
        .expect("cell in bounds");
    assert_eq!(
        cell.symbol(),
        "X",
        "CellSwatch::paint_cells must have painted through draw_primitive's &mut dyn \
         Backend path, not been silently skipped the way the no-op default is"
    );
}

/// `draw_primitive` now clips its [`quadraui::Backend::paint_surface`]
/// path to `rect` via a real
/// `PaintSurface::surface_push_clip`/`surface_pop_clip` pair, which on
/// `GtkBackend` requires a live Cairo context — same as every other
/// `GtkBackend::draw_*` method, each of which panics if called outside
/// `GtkBackend::enter_frame_scope`. An off-screen `ImageSurface` + a
/// throwaway `pango::Layout` give this test that live context without a
/// real window.
#[cfg(feature = "gtk")]
#[test]
fn gtk_draw_primitive_paints_the_resolved_layout() {
    use gtk4::cairo::{Context as CairoContext, Format, ImageSurface};
    use quadraui::gtk::backend::GtkBackend;

    let surface = ImageSurface::create(Format::ARgb32, 200, 80).expect("create ImageSurface");
    let cr = CairoContext::new(&surface).expect("Context::new");
    let pango_layout = pangocairo::functions::create_layout(&cr);

    let mut backend = GtkBackend::new();
    let area = Rect::new(4.0, 8.0, 120.0, 40.0);
    let (layout, painted, measured, measured_label) =
        backend.enter_frame_scope(&cr, &pango_layout, |b| {
            let b: &mut dyn Backend = b;
            draw_through_the_seam(b, area)
        });

    assert_layout_is_resolved(layout, area, measured, measured_label);
    assert_eq!(
        painted,
        vec![area],
        "GtkBackend implements PaintSurface, so draw_primitive must call Primitive::paint \
         exactly once with the layout it just resolved"
    );
}

#[cfg(feature = "win")]
#[test]
fn win_draw_primitive_paints_the_resolved_layout() {
    use quadraui::win::backend::WinBackend;

    let mut backend = WinBackend::new();
    let area = Rect::new(4.0, 8.0, 120.0, 40.0);
    let (layout, painted, measured, measured_label) = draw_through_the_seam(&mut backend, area);

    assert_layout_is_resolved(layout, area, measured, measured_label);
    assert_eq!(
        painted,
        vec![area],
        "WinBackend implements PaintSurface, so draw_primitive must call Primitive::paint \
         exactly once with the layout it just resolved"
    );
}

/// A widget outside the crate reads hover and press from the
/// [`quadraui::PaintEnv`] it is painted with, through public API only.
struct HoverSwatch {
    id: quadraui::WidgetId,
    painted: RefCell<Vec<quadraui::Color>>,
}

impl A11yPrimitive for HoverSwatch {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::default()
    }
}

impl Primitive for HoverSwatch {
    type Layout = Rect;
    type Event = ();

    fn id(&self) -> Option<&quadraui::WidgetId> {
        Some(&self.id)
    }

    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
        rect
    }

    fn paint(&self, layout: &Rect, surface: &mut dyn PaintSurface, env: &quadraui::PaintEnv) {
        let theme = env.theme();
        let color = if env.is_pressed(&self.id) {
            theme.accent_bg
        } else if env.is_hovered(&self.id) {
            theme.hover_bg
        } else {
            theme.background
        };
        surface.surface_fill_rect(*layout, color);
        self.painted.borrow_mut().push(color);
    }
}

#[test]
fn an_external_primitive_reads_hover_and_press_from_its_paint_env() {
    use quadraui::{InteractionState, PaintEnv, Style, Theme};

    struct Sink;
    impl PaintSurface for Sink {
        fn surface_begin_frame(&mut self, _v: quadraui::Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> quadraui::Viewport {
            quadraui::Viewport::new(0.0, 0.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            16.0
        }
        fn surface_char_width(&self) -> f32 {
            8.0
        }
        fn surface_measure_text(&self, _t: &str) -> (f32, f32) {
            (0.0, 0.0)
        }
        fn surface_fill_rect(&mut self, _r: Rect, _c: quadraui::Color) {}
        fn surface_fill_rounded_rect(&mut self, _r: Rect, _radius: f32, _c: quadraui::Color) {}
        fn surface_stroke_rect(&mut self, _r: Rect, _c: quadraui::Color, _w: f32) {}
        fn surface_draw_text_run(&mut self, _r: Rect, _t: &str, _c: quadraui::Color) {}
        fn surface_draw_line(
            &mut self,
            _from: quadraui::Point,
            _to: quadraui::Point,
            _c: quadraui::Color,
            _w: f32,
        ) {
        }
        fn surface_push_clip(&mut self, _r: Rect) {}
        fn surface_pop_clip(&mut self) {}
        fn surface_draw_image(
            &mut self,
            _r: Rect,
            _i: &quadraui::Image,
        ) -> quadraui::backend::ImagePaintResult {
            quadraui::backend::ImagePaintResult::Unsupported
        }
    }

    let id = quadraui::WidgetId::new("hover-swatch");
    let swatch = HoverSwatch {
        id,
        painted: RefCell::new(Vec::new()),
    };
    let (theme, style) = (Theme::default(), Style::default());
    let rect = Rect::new(0.0, 0.0, 4.0, 4.0);
    let rest = InteractionState::new();
    let hover = InteractionState::from_parts(Some(id), None);
    let press = InteractionState::from_parts(Some(id), Some(id));
    for state in [&rest, &hover, &press] {
        let env = PaintEnv::new(&theme, &style).with_interaction(state);
        swatch.paint(&rect, &mut Sink, &env);
    }
    assert_eq!(
        *swatch.painted.borrow(),
        vec![theme.background, theme.hover_bg, theme.accent_bg]
    );
}

/// What [`Counter`] emits, as the payload of a [`quadraui::UiEvent::User`]:
/// a widget outside the crate has no `UiEvent` variant of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CounterBumped {
    id: quadraui::WidgetId,
    /// The count the widget showed when it was bumped.
    from: u32,
}

/// [`Counter`]'s semantic action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CounterAction {
    Bumped { from: u32 },
}

impl quadraui::WidgetAction for CounterAction {
    fn into_ui_event(self, id: quadraui::WidgetId) -> quadraui::UiEvent {
        let CounterAction::Bumped { from } = self;
        quadraui::UiEvent::User(quadraui::UserPayload::new(CounterBumped { id, from }))
    }
}

/// [`Counter`]'s gesture state: how many bumps `handle` has produced.
#[derive(Debug, Default)]
struct CounterState {
    bumps: u32,
}

/// A focusable counter built against the public contract only. The count
/// it shows is descriptor data (controlled mode): the app owns it and
/// increments it on each [`CounterBumped`].
#[derive(Debug, Clone)]
struct Counter {
    id: quadraui::WidgetId,
    count: u32,
}

/// The label [`Counter`] paints: `[n]`.
fn counter_label(count: u32) -> String {
    format!("[{count}]")
}

impl A11yPrimitive for Counter {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::default()
    }
}

impl Primitive for Counter {
    type Layout = Rect;
    type Event = CounterAction;

    fn id(&self) -> Option<&quadraui::WidgetId> {
        Some(&self.id)
    }

    fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
        rect
    }

    fn paint(&self, layout: &Rect, surface: &mut dyn PaintSurface, env: &quadraui::PaintEnv) {
        let theme = env.theme();
        surface.surface_fill_rect(*layout, theme.accent_bg);
        surface.surface_draw_text_run(*layout, &counter_label(self.count), theme.accent_fg);
    }

    fn paint_cells(
        &self,
        layout: &Rect,
        cells: &mut dyn quadraui::CellSurface,
        env: &quadraui::PaintEnv,
    ) {
        let theme = env.theme();
        let style = quadraui::CellSurfaceStyle {
            fg: Some(theme.accent_fg),
            bg: Some(theme.accent_bg),
            bold: true,
            ..Default::default()
        };
        cells.fill(*layout, style);
        cells.put_str_clipped(
            layout.x as u16,
            layout.y as u16,
            &counter_label(self.count),
            layout.width as u16,
            style,
        );
    }
}

impl quadraui::Interactive for Counter {
    type State = CounterState;
    type Action = CounterAction;

    fn focus(&self) -> quadraui::FocusSpec {
        quadraui::FocusSpec::Stop
    }

    fn handle(
        &self,
        state: &mut CounterState,
        event: &quadraui::RoutedEvent,
        _layout: &Rect,
        cx: &mut quadraui::EventCx,
    ) -> quadraui::Handled<CounterAction> {
        use quadraui::{Key, NamedKey, PointerKind, RoutedEvent};
        let bump = match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(quadraui::MouseButton::Left),
                ..
            } => {
                cx.request_focus(None);
                true
            }
            RoutedEvent::Key(key) => {
                matches!(key.key, Key::Named(NamedKey::Enter) | Key::Char('+'))
            }
            RoutedEvent::Action(quadraui::A11yAction::Click) => true,
            _ => false,
        };
        if !bump {
            return quadraui::Handled::Ignored;
        }
        state.bumps += 1;
        quadraui::Handled::Event(CounterAction::Bumped { from: self.count })
    }
}

/// Every [`CounterBumped`] payload in `events`, in order.
fn counter_bumps(events: &[quadraui::UiEvent]) -> Vec<CounterBumped> {
    events
        .iter()
        .filter_map(|event| match event {
            quadraui::UiEvent::User(payload) => payload.downcast_ref::<CounterBumped>().copied(),
            _ => None,
        })
        .collect()
}

#[cfg(feature = "tui")]
mod counter_tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{
        AppLogic, FrameTree, NamedKey, Reaction, ScreenLayout, Surface, UiEvent, WidgetId,
    };

    const AREA: Rect = Rect::new(2.0, 1.0, 6.0, 1.0);

    fn counter_id() -> WidgetId {
        WidgetId::new("outside:counter")
    }

    /// An app that owns the count and draws the counter as a
    /// `Surface::Widget`, the way any in-crate widget is drawn.
    struct CounterApp {
        count: u32,
        events: Vec<UiEvent>,
    }

    impl CounterApp {
        fn new() -> Self {
            Self {
                count: 0,
                events: Vec::new(),
            }
        }

        fn counter(&self) -> Counter {
            Counter {
                id: counter_id(),
                count: self.count,
            }
        }
    }

    impl AppLogic for CounterApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut frame = ScreenLayout::new();
            frame.push(Surface::Widget {
                rect: AREA,
                widget: Box::new(self.counter()),
            });
            frame.draw(backend);
        }

        fn frame_tree(&self, _area: ()) -> FrameTree {
            let mut tree = FrameTree::new();
            tree.push(counter_id(), AREA, Box::new(self.counter()));
            tree
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            vec![(counter_id(), AREA)]
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            if let [bump] = counter_bumps(std::slice::from_ref(&event)).as_slice() {
                assert_eq!(
                    bump.from, self.count,
                    "the counter reports the count it showed"
                );
                self.count += 1;
            }
            self.events.push(event);
            Reaction::Redraw
        }
    }

    #[test]
    fn an_outside_widget_paints_its_cells_through_surface_widget() {
        let driver = TuiDriver::new(CounterApp::new(), 12, 3);
        assert_eq!(
            driver.find_bounds("[0]").map(|r| (r.x, r.y)),
            Some((AREA.x, AREA.y)),
            "{}",
            driver.screen()
        );
        let style = driver
            .style_at(AREA.x as u16, AREA.y as u16)
            .expect("the counter's first cell is on screen");
        assert!(
            style
                .modifiers
                .contains(quadraui::tui::testing::Modifier::BOLD),
            "paint_cells' style reaches the terminal buffer: {style:?}"
        );
        let blank = driver
            .style_at((AREA.x + AREA.width - 1.0) as u16, AREA.y as u16)
            .expect("the counter's last cell is on screen");
        assert_eq!(
            blank.bg, style.bg,
            "paint_cells fills the whole rect, not only the label"
        );
    }

    #[test]
    fn tab_focuses_an_outside_widget_and_its_keys_reach_handle() {
        let mut driver = TuiDriver::new(CounterApp::new(), 12, 3);

        driver.press_named(NamedKey::Tab);
        assert!(
            driver
                .app()
                .events
                .contains(&UiEvent::FocusChanged(Some(counter_id()))),
            "Tab gives the outside widget's FocusSpec::Stop focus: {:?}",
            driver.app().events
        );
        driver.press_named(NamedKey::Enter);
        driver.press(quadraui::Key::Char('+'));
        driver.press(quadraui::Key::Char('x'));

        assert_eq!(
            counter_bumps(&driver.app().events),
            vec![
                CounterBumped {
                    id: counter_id(),
                    from: 0
                },
                CounterBumped {
                    id: counter_id(),
                    from: 1
                },
            ]
        );
        assert!(driver.screen_contains("[2]"), "{}", driver.screen());
    }

    #[test]
    fn a_click_on_an_outside_widget_focuses_it_and_reaches_handle() {
        let mut driver = TuiDriver::new(CounterApp::new(), 12, 3);
        driver.set_double_click_folding(false);

        driver.click(AREA.x + 1.0, AREA.y);
        assert_eq!(
            counter_bumps(&driver.app().events),
            vec![CounterBumped {
                id: counter_id(),
                from: 0
            }]
        );
        assert!(driver.screen_contains("[1]"), "{}", driver.screen());

        driver.press_named(NamedKey::Enter);
        assert_eq!(
            counter_bumps(&driver.app().events).len(),
            2,
            "the click focused the counter, so Enter reaches its handle with no Tab first"
        );
    }

    /// The assistive-technology path reaches the same `handle` and
    /// produces the same event a click does.
    #[test]
    fn an_a11y_click_on_an_outside_widget_matches_a_mouse_click() {
        let mut driver = TuiDriver::new(CounterApp::new(), 12, 3);
        driver.set_double_click_folding(false);
        driver.click(AREA.x + 1.0, AREA.y);
        let clicked = counter_bumps(&driver.app().events);

        let app = CounterApp::new();
        let tree = app.frame_tree(());
        let mut states = quadraui::WidgetStateStore::new();
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let routed = quadraui::route_widget_action(
            &tree,
            &mut states,
            &ctx,
            counter_id(),
            quadraui::A11yAction::Click,
        );

        assert_eq!(counter_bumps(&routed), clicked);
        assert_eq!(routed.len(), 1, "one action, one event: {routed:?}");
    }
}

/// Paints a [`Counter`] through `draw_dyn` on a real Cairo image and reads
/// the pixels back: the fill lands inside the rect and nothing outside it.
#[cfg(feature = "gtk")]
#[test]
fn gtk_paints_an_outside_widget_in_pixels_through_draw_dyn() {
    use gtk4::cairo::{Context as CairoContext, Format, ImageSurface};
    use quadraui::gtk::backend::GtkBackend;

    let mut surface = ImageSurface::create(Format::ARgb32, 120, 60).expect("create ImageSurface");
    let area = Rect::new(10.0, 10.0, 60.0, 30.0);
    let fill = {
        let cr = CairoContext::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);
        let mut backend = GtkBackend::new();
        let counter = Counter {
            id: quadraui::WidgetId::new("outside:counter"),
            count: 7,
        };
        backend.enter_frame_scope(&cr, &pango_layout, |b| {
            let b: &mut dyn Backend = b;
            let layout = b.draw_dyn(area, &counter, &());
            assert_eq!(layout.downcast_ref::<Rect>(), Some(&area));
            b.theme().accent_bg
        })
    };
    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data().expect("image data");
    // Cairo's ARGB32 is native-endian premultiplied; the fill is opaque.
    let pixel = |x: usize, y: usize| {
        let i = y * stride + x * 4;
        let word = u32::from_ne_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        (
            (word >> 16) as u8,
            (word >> 8) as u8,
            word as u8,
            (word >> 24) as u8,
        )
    };
    assert_eq!(
        pixel(area.x as usize + 1, (area.y + area.height) as usize - 2),
        (fill.r, fill.g, fill.b, 255),
        "the counter's fill is painted inside its rect"
    );
    assert_eq!(pixel(2, 2).3, 0, "nothing is painted outside the rect");
}

#[cfg(feature = "win")]
#[test]
fn win_paints_an_outside_widget_through_draw_dyn() {
    use quadraui::win::backend::WinBackend;

    let mut backend = WinBackend::new();
    let area = Rect::new(4.0, 8.0, 120.0, 40.0);
    let counter = Counter {
        id: quadraui::WidgetId::new("outside:counter"),
        count: 3,
    };
    let b: &mut dyn Backend = &mut backend;
    assert!(b.paint_surface().is_some(), "WinBackend paints in pixels");
    let layout = b.draw_dyn(area, &counter, &());
    assert_eq!(layout.downcast_ref::<Rect>(), Some(&area));
}
