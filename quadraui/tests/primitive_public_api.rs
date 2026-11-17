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
//!    answers `None` (TUI), so a host can hit-test against geometry it
//!    cannot paint.
//!
//! `Swatch::paint` records its call rather than issuing a paint verb: a live
//! Cairo/Direct2D render target is needed to exercise the verbs themselves,
//! and pixel output for a real `Primitive` impl is covered by
//! `src/primitives/list.rs`'s `ListView` parity tests.

#![cfg(any(feature = "tui", feature = "gtk", feature = "win"))]

use std::cell::RefCell;

use quadraui::primitive::{LayoutCtx, Primitive};
use quadraui::{
    A11yInfo, A11yPrimitive, Backend, DrawsPrimitives, PaintSurface, Rect, Style, Theme,
};

/// [`Swatch`]'s fully-resolved layout: the absolute area it was laid out
/// into, plus the metrics `draw_primitive` measured off the backend for it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SwatchLayout {
    area: Rect,
    ctx: LayoutCtx,
}

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
            ctx: *ctx,
        }
    }

    fn paint(
        &self,
        layout: &SwatchLayout,
        _surface: &mut dyn PaintSurface,
        _theme: &Theme,
        _style: &Style,
    ) {
        self.painted.borrow_mut().push(layout.area);
    }
}

/// Draw a `Swatch` through `&mut dyn Backend`, returning the layout
/// `draw_primitive` handed back, the areas `paint` was called with, and the
/// metrics read off the same backend for comparison.
fn draw_through_the_seam(
    backend: &mut dyn Backend,
    area: Rect,
) -> (SwatchLayout, Vec<Rect>, LayoutCtx) {
    let measured = LayoutCtx {
        line_height: backend.chrome_line_height(),
        char_width: backend.char_width(),
        nerd_fonts_enabled: backend.nerd_fonts_enabled(),
    };
    let swatch = Swatch::new();
    let layout = backend.draw_primitive(area, &swatch);
    (layout, swatch.painted.into_inner(), measured)
}

/// Every backend resolves the layout the same way; only the paint half
/// depends on `Backend::paint_surface`.
fn assert_layout_is_resolved(layout: SwatchLayout, area: Rect, measured: LayoutCtx) {
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
}

#[cfg(feature = "tui")]
#[test]
fn tui_draw_primitive_resolves_layout_without_painting() {
    use quadraui::tui::backend::TuiBackend;

    let mut backend = TuiBackend::new();
    let area = Rect::new(1.0, 2.0, 10.0, 4.0);
    let (layout, painted, measured) = draw_through_the_seam(&mut backend, area);

    assert_layout_is_resolved(layout, area, measured);
    assert!(
        painted.is_empty(),
        "TuiBackend answers None from Backend::paint_surface, so draw_primitive has no \
         surface to paint onto and must skip Primitive::paint — the layout it still returns \
         is what lets a TUI host reserve space for, and hit-test, a GUI-only Primitive; \
         painted areas were {painted:?}"
    );
}

#[cfg(feature = "gtk")]
#[test]
fn gtk_draw_primitive_paints_the_resolved_layout() {
    use quadraui::gtk::backend::GtkBackend;

    let mut backend = GtkBackend::new();
    let area = Rect::new(4.0, 8.0, 120.0, 40.0);
    let (layout, painted, measured) = draw_through_the_seam(&mut backend, area);

    assert_layout_is_resolved(layout, area, measured);
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
    let (layout, painted, measured) = draw_through_the_seam(&mut backend, area);

    assert_layout_is_resolved(layout, area, measured);
    assert_eq!(
        painted,
        vec![area],
        "WinBackend implements PaintSurface, so draw_primitive must call Primitive::paint \
         exactly once with the layout it just resolved"
    );
}
