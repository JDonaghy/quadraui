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
use crate::event::Rect;
use crate::runner::{AppLogic, Reaction};
use crate::shell::{ShellApp, ShellContext};
use crate::types::WidgetId;
use crate::{Backend, MouseButton, UiEvent};

/// Adapts a [`ShellApp`] into an [`AppLogic`] by composing it with an
/// [`AppShell`] and an optional [`BottomPanelController`].
pub struct ShellAdapter<A: ShellApp> {
    pub app: A,
    pub shell: AppShell,
    pub _last_layout: Option<AppShellLayout>,
    pub active_panel_id: Option<WidgetId>,
    /// Optional bottom-panel controller. Present when `ShellConfig.bottom_panel`
    /// was `Some`. Wrapped in `RefCell` because `render(&self)` must mutate it
    /// to cache hit regions and render tab content.
    pub bottom_panel: Option<RefCell<BottomPanelController>>,
}

impl<A: ShellApp> AppLogic for ShellAdapter<A> {
    type AreaId = ();

    fn setup(&mut self, backend: &mut dyn Backend) {
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
                if let Some(ref _ctrl_cell) = self.bottom_panel {
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
        let ctx = ShellContext {
            active_panel_id: self.active_panel_id.as_ref(),
            sidebar_visible: self.shell.sidebar_visible(),
            layout: &layout,
        };
        self.app.handle(event, backend, &ctx)
    }

    fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
        self.app.tick(backend)
    }
}
