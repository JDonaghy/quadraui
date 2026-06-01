//! GTK shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard GTK event loop.

use std::cell::RefCell;

use crate::compose::app_shell::{AppShell, AppShellEvent, AppShellLayout};
use crate::compose::bottom_panel::{BottomPanelController, BottomPanelEvent};
use crate::event::Rect;
use crate::runner::{AppLogic, Reaction};
use crate::shell::{ShellApp, ShellConfig, ShellContext};
use crate::types::WidgetId;
use crate::{Backend, MouseButton, UiEvent};

struct ShellAdapter<A: ShellApp> {
    app: A,
    shell: AppShell,
    _last_layout: Option<AppShellLayout>,
    active_panel_id: Option<WidgetId>,
    /// Optional bottom-panel controller. Present when `ShellConfig.bottom_panel`
    /// was `Some`. Wrapped in `RefCell` because `render(&self)` must mutate it
    /// to cache hit regions and render tab content.
    bottom_panel: Option<RefCell<BottomPanelController>>,
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

/// Run a [`ShellApp`] with AppShell chrome on the GTK backend.
pub fn run_with_shell<A: ShellApp + 'static>(app: A, config: ShellConfig) {
    let mut shell = AppShell::new(config.panels, config.default_sidebar_width)
        .with_bottom_items(config.bottom_items)
        .with_min_width(config.min_sidebar_width)
        .with_max_width(config.max_sidebar_width)
        .with_position(config.position);

    if config.has_title_bar {
        shell = shell.with_title_bar(config.title_bar_height_lh);
    }
    if config.has_bottom_panel {
        shell = shell
            .with_bottom_panel(config.bottom_panel_height_lh)
            .with_bottom_panel_limits(
                config.min_bottom_panel_height_lh,
                config.max_bottom_panel_height_lh,
            );
    }
    if config.has_command_line {
        shell = shell.with_command_line();
    }
    if config.has_status_bar {
        shell = shell.with_status_bar();
    }

    // When a BottomPanelConfig is present, enable the bottom panel region
    // and create the controller. Initial height is set in setup() once we
    // have the backend's viewport + line_height.
    let bottom_panel = if let Some(bp_config) = config.bottom_panel {
        shell = shell
            .with_bottom_panel(10.0)
            .with_bottom_panel_limits(3.0, 40.0);
        Some(RefCell::new(BottomPanelController::new(bp_config)))
    } else {
        None
    };

    let active_panel_id = shell.active_panel_id().cloned();

    let adapter = ShellAdapter {
        app,
        shell,
        _last_layout: None,
        active_panel_id,
        bottom_panel,
    };

    super::run::run(adapter);
}
