//! GTK shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard GTK event loop.

use crate::compose::app_shell::{AppShell, AppShellEvent, AppShellLayout};
use crate::event::Rect;
use crate::runner::{AppLogic, Reaction};
use crate::shell::{ShellApp, ShellConfig};
use crate::types::WidgetId;
use crate::{Backend, UiEvent};

struct ShellAdapter<A: ShellApp> {
    app: A,
    shell: AppShell,
    _last_layout: Option<AppShellLayout>,
    active_panel_id: Option<WidgetId>,
}

impl<A: ShellApp> AppLogic for ShellAdapter<A> {
    type AreaId = ();

    fn setup(&mut self, backend: &mut dyn Backend) {
        self.app.setup(backend);
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        let layout = self.shell.render(backend, area);
        self.app.render_content(backend, &layout);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let viewport = backend.viewport();
        let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);

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
            AppShellEvent::BottomItemClicked { .. } => {
                self.app.on_shell_event(&shell_ev);
                return Reaction::Redraw;
            }
            AppShellEvent::Consumed => return Reaction::Redraw,
            AppShellEvent::Ignored => {}
        }

        self.app.handle(event, backend)
    }
}

/// Run a [`ShellApp`] with AppShell chrome on the GTK backend.
pub fn run_with_shell<A: ShellApp + 'static>(app: A, config: ShellConfig) {
    let shell = AppShell::new(config.panels, config.default_sidebar_width)
        .with_bottom_items(config.bottom_items)
        .with_min_width(config.min_sidebar_width)
        .with_max_width(config.max_sidebar_width)
        .with_position(config.position);

    let active_panel_id = shell.active_panel_id().cloned();

    let adapter = ShellAdapter {
        app,
        shell,
        _last_layout: None,
        active_panel_id,
    };

    super::run::run(adapter);
}
