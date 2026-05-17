//! Shell runner infrastructure: [`ShellApp`] trait + [`ShellConfig`].
//!
//! Apps that want an AppShell (activity bar + sidebar + main content)
//! implement [`ShellApp`] instead of [`AppLogic`](crate::AppLogic).
//! Per-backend `run_with_shell()` functions handle the full lifecycle:
//! window creation, event wiring, AppShell chrome rendering, and event
//! routing — the consumer renders only its own content.

use crate::compose::app_shell::{AppShellEvent, AppShellLayout, PanelDefinition, ShellPosition};
use crate::types::WidgetId;
use crate::{Backend, Reaction, UiEvent};

/// Configuration for creating an AppShell.
pub struct ShellConfig {
    pub panels: Vec<PanelDefinition>,
    pub bottom_items: Vec<PanelDefinition>,
    pub title: String,
    pub default_sidebar_width: f32,
    pub min_sidebar_width: f32,
    pub max_sidebar_width: f32,
    pub position: ShellPosition,
}

impl ShellConfig {
    pub fn new(title: impl Into<String>, panels: Vec<PanelDefinition>) -> Self {
        Self {
            panels,
            bottom_items: Vec::new(),
            title: title.into(),
            default_sidebar_width: 20.0,
            min_sidebar_width: 8.0,
            max_sidebar_width: 50.0,
            position: ShellPosition::Left,
        }
    }

    pub fn with_bottom_items(mut self, items: Vec<PanelDefinition>) -> Self {
        self.bottom_items = items;
        self
    }

    pub fn with_position(mut self, position: ShellPosition) -> Self {
        self.position = position;
        self
    }
}

/// Application trait for apps that use the AppShell chrome.
///
/// The shell handles: activity bar rendering + clicks, sidebar
/// header + divider, panel switching, and resize drag. The consumer
/// renders panel content and main-area content into the bounds the
/// shell provides via [`AppShellLayout`].
pub trait ShellApp {
    /// Render content into the shell's content zones. The shell has
    /// already drawn its chrome (activity bar, sidebar header, divider);
    /// the consumer draws sidebar panel content + main content here.
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout);

    /// Handle events the shell didn't consume. Activity bar clicks,
    /// divider drag, and hover are handled internally; only unhandled
    /// events reach this method.
    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction;

    /// Called once after the shell is built. Optional.
    fn setup(&mut self, _backend: &mut dyn Backend) {}

    /// Notified when a panel switch occurs (activity bar click or
    /// programmatic). Optional.
    fn on_shell_event(&mut self, _event: &AppShellEvent) {}

    /// Access the active panel ID (for content routing in render_content).
    /// Provided by the shell runner — consumers don't implement this.
    fn active_panel_id_hint(&self) -> Option<&WidgetId> {
        None
    }
}
