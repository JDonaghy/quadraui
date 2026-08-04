//! GTK shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard GTK event loop.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch). This
//! module only wires the adapter into the GTK event loop.

use std::cell::RefCell;

use crate::compose::app_shell::AppShell;
use crate::compose::bottom_panel::BottomPanelController;
use crate::shell::{ShellApp, ShellConfig};
use crate::shell_adapter::ShellAdapter;

/// Assemble the [`AppShell`] + [`ShellAdapter`] stack for a [`ShellApp`].
///
/// This is the single source of truth for shell construction — both the
/// live runner ([`run_with_shell`]) and the headless test driver
/// ([`crate::gtk::testing::driver_with_shell`]) call this so they cannot
/// drift apart as [`ShellConfig`] grows. Mirrors
/// [`crate::tui::shell_runner::build_shell_adapter`] line for line; GTK has
/// no backend-specific wiring beyond what [`ShellAdapter`] already
/// encapsulates (editor font, bottom-panel init), so the two are identical
/// today.
pub(crate) fn build_shell_adapter<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
) -> ShellAdapter<A> {
    let editor_font = config.editor_font.clone();
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
        // Enable the panel with a generous initial height; setup() will
        // recalculate from height_fraction once the backend is ready.
        shell = shell
            .with_bottom_panel(10.0)
            .with_bottom_panel_limits(3.0, 40.0);
        Some(RefCell::new(BottomPanelController::new(bp_config)))
    } else {
        None
    };

    let active_panel_id = shell.active_panel_id().cloned();

    ShellAdapter::new(app, shell, active_panel_id, bottom_panel, editor_font)
}

/// Run a [`ShellApp`] with AppShell chrome on the GTK backend.
pub fn run_with_shell<A: ShellApp + 'static>(app: A, config: ShellConfig) {
    let adapter = build_shell_adapter(app, config);
    super::run::run(adapter);
}
