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

    // Configure the height unconditionally and let `has_title_bar` control
    // only initial visibility. Gating the whole call on `has_title_bar`
    // (as before) silently discarded `title_bar_height_lh` whenever the
    // bar started hidden — exactly the case where it matters later: a
    // consumer that reveals the bar at runtime via `set_title_bar_visible`
    // got the `AppShell` struct default (1.5 line-heights) instead of the
    // height it configured (#547).
    shell = shell.with_title_bar(config.title_bar_height_lh);
    shell.set_title_bar_visible(config.has_title_bar);

    // Bottom panel does NOT share this defect, despite the parallel shape:
    // `AppShell` has no runtime setter that flips `has_bottom_panel` alone
    // the way `set_title_bar_visible` flips `has_title_bar`. The only way
    // to enable it is `with_bottom_panel(height)`, which sets the flag and
    // height together — `show_bottom_panel`/`hide_bottom_panel`/
    // `toggle_bottom_panel` only touch the independent `bottom_panel_visible`
    // flag and are no-ops while `has_bottom_panel` is false. So gating this
    // call on `config.has_bottom_panel` cannot later reveal a stale height.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::app_shell::AppShellLayout;
    use crate::event::Rect;
    use crate::shell::ShellContext;
    use crate::{Backend, Reaction, UiEvent};

    struct NoopApp;

    impl ShellApp for NoopApp {
        fn render_content(&self, _backend: &mut dyn Backend, _layout: &AppShellLayout) {}

        fn handle(
            &mut self,
            _event: UiEvent,
            _backend: &mut dyn Backend,
            _ctx: &ShellContext,
        ) -> Reaction {
            Reaction::Continue
        }
    }

    /// #547: mirrors `tui::shell_runner`'s regression test — see there for
    /// the full explanation. `build_shell_adapter` is identical across the
    /// two backends, so the defect (and the fix) is too.
    #[test]
    fn title_bar_height_lh_survives_start_hidden_then_shown() {
        let mut config = ShellConfig::new("t", Vec::new());
        config.has_title_bar = false;
        config.title_bar_height_lh = 1.0;

        let mut adapter = build_shell_adapter(NoopApp, config);
        assert!(!adapter.shell.title_bar_visible());

        adapter.shell.set_title_bar_visible(true);
        let layout = adapter.shell.layout(Rect::new(0.0, 0.0, 80.0, 24.0), 1.0);
        let tb = layout.title_bar_bounds.expect("row now reserved");
        assert_eq!(
            tb.height, 1.0,
            "configured height (1.0 lh) must be honoured, not the 1.5 lh AppShell default"
        );
    }

    /// Companion case: `has_title_bar: true` consumers are unaffected —
    /// same height, visible from the first frame, no behaviour change.
    #[test]
    fn title_bar_height_lh_honoured_when_started_visible() {
        let mut config = ShellConfig::new("t", Vec::new());
        config.has_title_bar = true;
        config.title_bar_height_lh = 1.0;

        let adapter = build_shell_adapter(NoopApp, config);
        assert!(adapter.shell.title_bar_visible());

        let layout = adapter.shell.layout(Rect::new(0.0, 0.0, 80.0, 24.0), 1.0);
        let tb = layout
            .title_bar_bounds
            .expect("row reserved from construction");
        assert_eq!(tb.height, 1.0);
    }
}
