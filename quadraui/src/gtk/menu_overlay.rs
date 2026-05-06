//! GTK overlay helper for [`MenuSystem`] dropdowns.
//!
//! Encapsulates the `DrawingArea` boilerplate needed when a GTK app
//! renders the menu bar in a separate titlebar DA and the dropdown
//! popup in a full-window overlay DA. The helper handles the negative-y
//! coordinate transform (the bar sits above the overlay), DA property
//! setup (`set_focusable(false)`, `set_can_target` toggling), and
//! draw / click / motion wiring — eliminating the two bug classes that
//! arise from hand-rolling this (~120 lines of per-app code):
//!
//! 1. **Coordinate mismatch** — render applies the negative-y offset
//!    but click/motion handlers forget it, so hit-testing is wrong.
//! 2. **Missing `set_focusable(false)`** — GTK steals Left/Right arrow
//!    keys for focus navigation instead of routing them to MenuSystem.
//!
//! # Usage
//!
//! ```ignore
//! let overlay = MenuOverlay::new();
//! overlay.connect(menu_system, backend, bar_rect_cell, "Sans 11", |ev| {
//!     match ev {
//!         MenuEvent::Activated(id) => { /* dispatch action */ }
//!         MenuEvent::Ignored => {}
//!         _ => { /* trigger redraw */ }
//!     }
//! });
//! window_overlay.add_overlay(overlay.drawing_area());
//!
//! // On MenuRedraw:
//! overlay.sync(menu_system.borrow().is_open());
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{pango, DrawingArea, EventControllerMotion, GestureClick};
use pangocairo::functions as pcfn;

use super::backend::GtkBackend;
use crate::backend::Backend;
use crate::compose::menu_system::{MenuEvent, MenuSystem};
use crate::event::{ButtonMask, MouseButton, Point, Rect, UiEvent, Viewport};
use crate::types::Modifiers;

/// Overlay `DrawingArea` wired to render and dispatch events for a
/// [`MenuSystem`] dropdown popup.
pub struct MenuOverlay {
    da: DrawingArea,
}

impl Default for MenuOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl MenuOverlay {
    /// Create a correctly-configured overlay `DrawingArea`. The DA
    /// starts with `can_target(false)` (events pass through to the
    /// content below) and `focusable(false)` (GTK won't steal arrow
    /// keys for focus navigation). Call [`Self::connect`] to wire
    /// draw / click / motion handlers, then add the DA to your
    /// `gtk4::Overlay` via [`Self::drawing_area`].
    pub fn new() -> Self {
        let da = DrawingArea::new();
        da.set_hexpand(true);
        da.set_vexpand(true);
        da.set_can_target(false);
        da.set_focusable(false);
        Self { da }
    }

    /// The underlying `DrawingArea`. Pass to
    /// `gtk4::Overlay::add_overlay`.
    pub fn drawing_area(&self) -> &DrawingArea {
        &self.da
    }

    /// Translate a bar_rect from the menu bar DA's local coordinates
    /// into the overlay DA's coordinate space. The menu bar sits above
    /// the overlay (it's a GTK titlebar), so the bar's y-origin becomes
    /// negative in overlay space.
    pub fn bar_rect_in_overlay(bar_rect: Rect) -> Rect {
        Rect::new(
            bar_rect.x,
            -bar_rect.height,
            bar_rect.width,
            bar_rect.height,
        )
    }

    /// Toggle event capture and queue a redraw. Call on every
    /// `MenuEvent::StateChanged` / `MenuEvent::Consumed`.
    ///
    /// When the menu is open, `can_target(true)` lets the overlay
    /// intercept clicks (so they hit the dropdown, not the content
    /// behind it). When closed, `can_target(false)` makes the overlay
    /// transparent to events.
    ///
    /// Optionally pass the menu bar's `DrawingArea` to queue its
    /// redraw too (so the bar's highlight updates in sync).
    pub fn sync(&self, is_open: bool, bar_da: Option<&DrawingArea>) {
        self.da.set_can_target(is_open);
        self.da.queue_draw();
        if let Some(bar) = bar_da {
            bar.queue_draw();
        }
    }

    /// Wire draw, click, and motion handlers that delegate to
    /// [`MenuSystem`]. Call once during setup.
    ///
    /// - `menu_system` — shared `MenuSystem` state (the same instance
    ///   the menu bar DA's click handler and the keyboard handler use).
    /// - `backend` — shared `GtkBackend`.
    /// - `bar_rect` — the menu bar's rect in its own local coordinates,
    ///   updated each frame by the menu bar DA's draw function. The
    ///   helper applies the negative-y transform internally.
    /// - `font` — Pango font description string (e.g. `"Sans 11"`).
    ///   Used to measure line_height and char_width for the overlay's
    ///   independent draw callback.
    /// - `on_event` — callback invoked with every `MenuEvent` returned
    ///   by `MenuSystem::handle`. Typical dispatch: `Activated(id)` →
    ///   send to your action handler; `StateChanged` / `Consumed` →
    ///   call [`Self::sync`]; `Ignored` → no-op.
    pub fn connect(
        &self,
        menu_system: Rc<RefCell<MenuSystem>>,
        backend: Rc<RefCell<GtkBackend>>,
        bar_rect: Rc<Cell<Rect>>,
        font: &str,
        on_event: impl Fn(MenuEvent) + 'static + Clone,
    ) {
        self.wire_draw(menu_system.clone(), backend.clone(), bar_rect.clone(), font);
        self.wire_click(
            menu_system.clone(),
            backend.clone(),
            bar_rect.clone(),
            on_event.clone(),
        );
        self.wire_motion(menu_system, backend, bar_rect, on_event);
    }

    fn wire_draw(
        &self,
        menu_system: Rc<RefCell<MenuSystem>>,
        backend: Rc<RefCell<GtkBackend>>,
        bar_rect: Rc<Cell<Rect>>,
        font: &str,
    ) {
        let font_str = font.to_string();
        self.da.set_draw_func(move |_da, cr, w, h| {
            if !menu_system.borrow().is_open() {
                return;
            }
            let pango_ctx = pcfn::create_context(cr);
            let pango_layout = pango::Layout::new(&pango_ctx);
            let font_desc = pango::FontDescription::from_string(&font_str);
            pango_layout.set_font_description(Some(&font_desc));
            let metrics = pango_ctx.metrics(Some(&font_desc), None);
            let lh = (metrics.ascent() + metrics.descent()) as f64 / pango::SCALE as f64;
            pango_layout.set_text("M");
            let cw = pango_layout.pixel_size().0 as f64;

            let overlay_rect = Self::bar_rect_in_overlay(bar_rect.get());

            // Clear the overlay surface so old content doesn't bleed
            // through if the layout shifts between frames.
            cr.set_operator(gtk4::cairo::Operator::Clear);
            cr.paint().ok();
            cr.set_operator(gtk4::cairo::Operator::Over);

            let mut b = backend.borrow_mut();
            b.begin_frame(Viewport::new(w as f32, h as f32, 1.0));
            b.enter_frame_scope(cr, &pango_layout, |b| {
                b.set_current_line_height(lh);
                b.set_current_char_width(cw);
                menu_system.borrow().render(b, overlay_rect);
            });
        });
    }

    fn wire_click(
        &self,
        menu_system: Rc<RefCell<MenuSystem>>,
        backend: Rc<RefCell<GtkBackend>>,
        bar_rect: Rc<Cell<Rect>>,
        on_event: impl Fn(MenuEvent) + 'static,
    ) {
        let gesture = GestureClick::new();
        gesture.set_button(1);
        gesture.connect_pressed(move |_, _, x, y| {
            let overlay_rect = Self::bar_rect_in_overlay(bar_rect.get());
            let ev = UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(x as f32, y as f32),
                modifiers: Modifiers::default(),
            };
            let menu_event =
                menu_system
                    .borrow_mut()
                    .handle(&ev, &mut *backend.borrow_mut(), overlay_rect);
            on_event(menu_event);
        });
        self.da.add_controller(gesture);
    }

    fn wire_motion(
        &self,
        menu_system: Rc<RefCell<MenuSystem>>,
        backend: Rc<RefCell<GtkBackend>>,
        bar_rect: Rc<Cell<Rect>>,
        on_event: impl Fn(MenuEvent) + 'static,
    ) {
        let motion = EventControllerMotion::new();
        motion.connect_motion(move |_, x, y| {
            if !menu_system.borrow().is_open() {
                return;
            }
            let overlay_rect = Self::bar_rect_in_overlay(bar_rect.get());
            let ev = UiEvent::MouseMoved {
                position: Point::new(x as f32, y as f32),
                buttons: ButtonMask::default(),
            };
            let menu_event =
                menu_system
                    .borrow_mut()
                    .handle(&ev, &mut *backend.borrow_mut(), overlay_rect);
            on_event(menu_event);
        });
        self.da.add_controller(motion);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_rect_in_overlay_applies_negative_y() {
        let stored = Rect::new(0.0, 0.0, 800.0, 24.0);
        let transformed = MenuOverlay::bar_rect_in_overlay(stored);
        assert_eq!(transformed.x, 0.0);
        assert_eq!(transformed.y, -24.0);
        assert_eq!(transformed.width, 800.0);
        assert_eq!(transformed.height, 24.0);
    }

    #[test]
    fn bar_rect_in_overlay_preserves_x_offset() {
        let stored = Rect::new(10.0, 0.0, 780.0, 20.0);
        let transformed = MenuOverlay::bar_rect_in_overlay(stored);
        assert_eq!(transformed.x, 10.0);
        assert_eq!(transformed.y, -20.0);
        assert_eq!(transformed.width, 780.0);
        assert_eq!(transformed.height, 20.0);
    }
}
