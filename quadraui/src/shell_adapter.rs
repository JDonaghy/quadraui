//! Shared [`ShellAdapter`] that wraps a [`ShellApp`] in an [`AppShell`]
//! and implements [`AppLogic`] against it.
//!
//! Both the TUI and GTK shell runners instantiate this struct and pass it to
//! their respective `run()` entry point. All runtime logic — bottom-panel
//! height initialisation, panel-changed routing, resize fraction recalc, and
//! bottom-panel click dispatch — lives here once, avoiding per-backend drift.
//!
//! Backend-specific differences (line-height units, pixel vs cell sizes) are
//! encapsulated in [`crate::Backend`] method calls; the adapter only uses the
//! trait surface.
//!
//! # Note on code sharing
//!
//! Previously the TUI and GTK runners each embedded a byte-for-byte copy of
//! `ShellAdapter` and its `AppLogic` impl. This module is the single canonical
//! location; the per-backend `run_with_shell` functions merely construct the
//! adapter and forward it to their native runner.

use std::cell::RefCell;

use crate::compose::app_shell::{AppShell, AppShellEvent, AppShellLayout};
use crate::compose::bottom_panel::{BottomPanelController, BottomPanelEvent};
use crate::event::{Point, Rect};
use crate::runner::{AppLogic, Reaction};
use crate::shell::{ShellApp, ShellContext};
use crate::types::WidgetId;
use crate::{ActivityBarEvent, Backend, MouseButton, UiEvent};

/// Adapts a [`ShellApp`] into an [`AppLogic`] by composing it with an
/// [`AppShell`] and an optional [`BottomPanelController`].
///
/// The struct and its fields are `pub(crate)` so only the in-crate shell
/// runners can construct it. Downstream consumers drive the shell through
/// [`crate::tui::shell_runner::run_with_shell`] /
/// [`crate::gtk::shell_runner::run_with_shell`] + [`ShellApp`] callbacks —
/// they never touch the adapter directly.
pub(crate) struct ShellAdapter<A: ShellApp> {
    pub(crate) app: A,
    pub(crate) shell: AppShell,
    pub(crate) _last_layout: Option<AppShellLayout>,
    pub(crate) active_panel_id: Option<WidgetId>,
    /// Optional bottom-panel controller. Present when `ShellConfig.bottom_panel`
    /// was `Some`. Wrapped in `RefCell` because `render(&self)` must mutate it
    /// to cache hit regions and render tab content.
    pub(crate) bottom_panel: Option<RefCell<BottomPanelController>>,
    /// Editor font override from `ShellConfig::with_editor_font`
    /// (family, size_pt), applied once in `setup()` via
    /// `Backend::set_editor_font` (#422). `None` leaves the backend's own
    /// default font in place.
    pub(crate) editor_font: Option<(String, f32)>,
}

impl<A: ShellApp> ShellAdapter<A> {
    /// Construct a new [`ShellAdapter`] from its parts. Only called by the
    /// in-crate TUI / GTK shell runners; downstream consumers should use
    /// `run_with_shell` rather than building an adapter directly.
    pub(crate) fn new(
        app: A,
        shell: AppShell,
        active_panel_id: Option<WidgetId>,
        bottom_panel: Option<RefCell<BottomPanelController>>,
        editor_font: Option<(String, f32)>,
    ) -> Self {
        Self {
            app,
            shell,
            _last_layout: None,
            active_panel_id,
            bottom_panel,
            editor_font,
        }
    }
}

impl<A: ShellApp> AppLogic for ShellAdapter<A> {
    type AreaId = ();

    fn setup(&mut self, backend: &mut dyn Backend) {
        // Apply the configured editor font (#422) before anything else so
        // it's in place for the app's own `setup()` and the very first
        // frame. `Backend::set_editor_font` no-ops on backends without a
        // font concept (TUI).
        if let Some((family, size_pt)) = &self.editor_font {
            backend.set_editor_font(family, *size_pt);
        }

        // Initialise bottom panel height from height_fraction on first setup.
        if let Some(ref ctrl_cell) = self.bottom_panel {
            let ctrl = ctrl_cell.borrow();
            let viewport = backend.viewport();
            let lh = backend.line_height().max(1.0);
            let panel_lh = (viewport.height / lh * ctrl.height_fraction).clamp(3.0, 30.0);
            drop(ctrl);
            self.shell.set_bottom_panel_height(panel_lh);
        }
        self.app.setup(backend);
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let layout = self.shell.render(backend, area);

        if let Some(ref ctrl_cell) = self.bottom_panel {
            let maximised = ctrl_cell.borrow().maximised;

            if maximised {
                // Expand panel to cover main content area too.
                let main = layout.main_content_bounds;
                let bp_h = layout.bottom_panel_bounds.map(|r| r.height).unwrap_or(0.0);
                let full_panel = Rect::new(main.x, main.y, main.width, main.height + bp_h);
                ctrl_cell.borrow_mut().render(backend, full_panel);

                // Pass a layout with zeroed main area so app skips rendering it.
                let maximised_layout = AppShellLayout {
                    main_content_bounds: Rect::new(
                        main.x,
                        main.y + main.height + bp_h,
                        main.width,
                        0.0,
                    ),
                    bottom_panel_bounds: Some(full_panel),
                    ..layout
                };
                self.app.render_content(backend, &maximised_layout);
            } else if let Some(panel_bounds) = layout.bottom_panel_bounds {
                ctrl_cell.borrow_mut().render(backend, panel_bounds);
                self.app.render_content(backend, &layout);
            } else {
                self.app.render_content(backend, &layout);
            }
        } else {
            self.app.render_content(backend, &layout);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let viewport = backend.viewport();
        let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);

        // On viewport resize, recalculate the bottom panel height from fraction.
        if matches!(event, UiEvent::WindowResized { .. }) {
            if let Some(ref ctrl_cell) = self.bottom_panel {
                let height_fraction = ctrl_cell.borrow().height_fraction;
                let lh = backend.line_height().max(1.0);
                let panel_lh = (viewport.height / lh * height_fraction).clamp(3.0, 30.0);
                self.shell.set_bottom_panel_height(panel_lh);
            }
        }

        // An app-level modal (MenuSystem dropdown, Palette, Dialog, ...) can
        // visually overlap shell chrome (activity bar, sidebar, dividers,
        // bottom-panel grip). `AppShell::handle` below — and the
        // bottom-panel tab-strip click check further down — both hit-test
        // purely on screen position, so an open modal that overlaps chrome
        // would otherwise have its `MouseDown` / `MouseUp` silently
        // swallowed by the chrome (issue #411). Consult the backend's
        // `ModalStack` directly — the same source `dispatch_click` /
        // `dispatch_mouse_down` (`crate::dispatch`) used to tag the event's
        // `widget` field — and route straight to the app when the position
        // lands inside an open modal. This upholds the modal-stack contract
        // ("events landing inside an open modal cannot fall through to the
        // base layer", see `crate::dispatch` module docs) for shell chrome
        // too, not just base-layer widgets.
        if let Some(position) = mouse_event_position(&event) {
            if backend.modal_stack_mut().hit_test(position).is_some() {
                let layout = self.shell.layout(area, backend.line_height());
                let ctx = ShellContext::new(
                    self.active_panel_id.as_ref(),
                    self.shell.sidebar_visible(),
                    &layout,
                );
                return self.app.handle(event, backend, &ctx);
            }
        }

        let shell_ev = self.shell.handle(&event, backend, area);
        match &shell_ev {
            AppShellEvent::PanelChanged { panel_id } => {
                self.active_panel_id = Some(panel_id.clone());
                self.app.on_shell_event(&shell_ev);
                return Reaction::Redraw;
            }
            AppShellEvent::SidebarHidden => {
                self.app.on_shell_event(&shell_ev);
                return Reaction::Redraw;
            }
            AppShellEvent::SidebarResized { .. } => {
                self.app.on_shell_event(&shell_ev);
                return Reaction::Redraw;
            }
            AppShellEvent::BottomPanelResized { new_height } => {
                // Forward as BottomPanelEvent::Resized to app when a controller is present.
                // Also update `height_fraction` so a subsequent WindowResized doesn't snap
                // the panel back to its initial proportion: the drag is the new intent.
                if let Some(ref ctrl_cell) = self.bottom_panel {
                    let new_fraction = resized_to_fraction(*new_height, viewport.height);
                    ctrl_cell.borrow_mut().height_fraction = new_fraction;
                    let ev = BottomPanelEvent::Resized(*new_height);
                    self.app.on_bottom_panel_event(&ev);
                }
                self.app.on_shell_event(&shell_ev);
                return Reaction::Redraw;
            }
            AppShellEvent::BottomPanelHidden => {
                self.app.on_shell_event(&shell_ev);
                return Reaction::Redraw;
            }
            AppShellEvent::BottomItemClicked { .. } => {
                self.app.on_shell_event(&shell_ev);
                return Reaction::Redraw;
            }
            AppShellEvent::Consumed => return Reaction::Redraw,
            AppShellEvent::Ignored => {}
        }

        // ── Built-in activity-bar keyboard navigation (#409) ──────────────
        //
        // `AppShell::build_activity_bar()` (compose/app_shell.rs) already
        // implements the full keyboard-cursor API (select_next/prev,
        // activate_selected, ...) and the backends already translate a
        // `KeyPressed` into `UiEvent::ActivityBar(id, KeyPressed { key, .. })`
        // once `activity_keyboard_focused()` is true — see
        // `TuiBackend::apply_dispatch`. But `ShellApp` consumers had no path
        // to reach any of it (the raw `AppLogic` pattern in
        // `examples/common/shell_app.rs` owns `AppShell` directly and wires
        // this itself). Mirror that pattern here, once, for every `ShellApp`
        // consumer: intercept the synthesized `ActivityBar` event before it
        // would otherwise reach `ShellApp::handle` (which never sees this
        // event type — there is nothing for a consumer to opt into or
        // conflict with) and drive `AppShell` directly. Resulting
        // `AppShellEvent`s are reported through the existing
        // `on_shell_event` notification, exactly like a mouse-driven panel
        // switch.
        if let UiEvent::ActivityBar(_, ActivityBarEvent::KeyPressed { ref key, .. }) = event {
            return match key.as_str() {
                "j" | "Down" => {
                    self.shell.activity_select_next();
                    Reaction::Redraw
                }
                "k" | "Up" => {
                    self.shell.activity_select_prev();
                    Reaction::Redraw
                }
                "l" | "Enter" | " " | "Space" => {
                    if let Some(ev) = self.shell.activity_activate_selected() {
                        self.shell.set_activity_keyboard_focused(false);
                        if let AppShellEvent::PanelChanged { ref panel_id } = ev {
                            self.active_panel_id = Some(panel_id.clone());
                        }
                        self.app.on_shell_event(&ev);
                    }
                    Reaction::Redraw
                }
                "Escape" | "h" | "Left" => {
                    self.shell.set_activity_keyboard_focused(false);
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            };
        }

        // If the shell didn't consume the event, check the bottom panel tab strip.
        if let Some(ref ctrl_cell) = self.bottom_panel {
            if let UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } = &event
            {
                let bp_ev = ctrl_cell.borrow_mut().handle_click(position.x, position.y);
                if let Some(bp_ev) = bp_ev {
                    // Auto-hide the panel once its last tab is closed.
                    if matches!(bp_ev, BottomPanelEvent::TabClosed(_))
                        && ctrl_cell.borrow().tabs().is_empty()
                    {
                        self.shell.hide_bottom_panel();
                    }
                    self.app.on_bottom_panel_event(&bp_ev);
                    return Reaction::Redraw;
                }
            }
        }

        let layout = self.shell.layout(area, backend.line_height());
        let ctx = ShellContext::new(
            self.active_panel_id.as_ref(),
            self.shell.sidebar_visible(),
            &layout,
        );
        let mut reaction = self.app.handle(event, backend, &ctx);

        // The app may have called `ctx.request_activity_keyboard_focus()`
        // (e.g. in response to its own `Tab` / `Ctrl+W` binding) to enter
        // the keyboard-cursor mode the block above then drives.
        if ctx.take_activity_focus_requested() {
            self.shell.set_activity_keyboard_focused(true);
            self.shell.activity_set_cursor(0);
            if reaction == Reaction::Continue {
                reaction = Reaction::Redraw;
            }
        }

        reaction
    }

    fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
        self.app.tick(backend)
    }
}

/// Extract the screen position from a `MouseDown` / `MouseUp` event, or
/// `None` for any other event variant.
///
/// Used to consult the backend's [`crate::modal_stack::ModalStack`] before
/// letting shell chrome (`AppShell::handle`) hit-test the same position —
/// only these two variants are position-gated by chrome logic today.
fn mouse_event_position(event: &UiEvent) -> Option<Point> {
    match event {
        UiEvent::MouseDown { position, .. } => Some(*position),
        UiEvent::MouseUp { position, .. } => Some(*position),
        _ => None,
    }
}

/// Recover the bottom-panel `height_fraction` after a drag-resize.
///
/// `new_height` arrives in the backend's native units (cells for TUI, pixels
/// for GTK) from `AppShellEvent::BottomPanelResized`; `viewport_height` is
/// in the same native units (from `Backend::viewport()`). Dividing yields the
/// proportion of the viewport the panel now occupies, clamped to `[0, 1]`.
///
/// Centralised so the in-line `handle()` branch and the regression test
/// stay in lockstep: a missed update here is what made the panel snap back
/// to the initial 30 % on the next `WindowResized`.
fn resized_to_fraction(new_height: f32, viewport_height: f32) -> f32 {
    let h = viewport_height.max(1.0);
    (new_height / h).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resized_to_fraction_recovers_proportion() {
        // Drag the panel to 18 cells in a 60-cell viewport → 30 %.
        assert!((resized_to_fraction(18.0, 60.0) - 0.30).abs() < 1e-5);
        // Drag to 36 cells in the same viewport → 60 %.
        assert!((resized_to_fraction(36.0, 60.0) - 0.60).abs() < 1e-5);
    }

    #[test]
    fn resized_to_fraction_clamps_negative_to_zero() {
        // Negative heights would otherwise propagate a bogus fraction
        // into the next viewport recalc.
        assert_eq!(resized_to_fraction(-5.0, 60.0), 0.0);
    }

    #[test]
    fn resized_to_fraction_clamps_over_full_viewport_to_one() {
        // If the drag math overshoots the viewport, cap at 100 %.
        assert_eq!(resized_to_fraction(120.0, 60.0), 1.0);
    }

    #[test]
    fn resized_to_fraction_handles_zero_viewport_safely() {
        // A zero-height viewport (pre-init / minimised) must not divide
        // by zero — the clamp keeps the output finite.
        let f = resized_to_fraction(10.0, 0.0);
        assert!(f.is_finite());
    }

    /// Regression for the bug: dragging the panel to a new height must
    /// update `height_fraction` so a subsequent `WindowResized` reuses
    /// the dragged proportion instead of snapping back to the initial
    /// 30 %. This exercises the same arithmetic the `handle()` branch
    /// uses for `AppShellEvent::BottomPanelResized`.
    #[test]
    fn drag_then_window_resize_preserves_user_intent() {
        // Initial state: viewport 100 cells tall, fraction 0.30
        // → panel 30 cells.
        let mut height_fraction = 0.30_f32;
        let viewport_h_initial = 100.0_f32;
        let initial_panel = viewport_h_initial * height_fraction;
        assert!((initial_panel - 30.0).abs() < 1e-4);

        // User drags to 60 cells. The adapter updates the fraction.
        let dragged_native = 60.0_f32;
        height_fraction = resized_to_fraction(dragged_native, viewport_h_initial);
        assert!((height_fraction - 0.60).abs() < 1e-5);

        // Window then resizes to 50 cells tall. The adapter recalculates
        // panel height from the (now-updated) fraction: 50 * 0.6 = 30 cells.
        let viewport_h_resized = 50.0_f32;
        let recalculated_panel = viewport_h_resized * height_fraction;
        assert!(
            (recalculated_panel - 30.0).abs() < 1e-4,
            "post-resize panel height ({}) should reflect dragged proportion (0.60), \
             not initial 0.30 (which would give {})",
            recalculated_panel,
            viewport_h_resized * 0.30,
        );
    }
}
