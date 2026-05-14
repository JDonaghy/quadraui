//! High-level helpers that compose multiple primitives into reusable
//! interaction patterns.
//!
//! The `primitives` module provides stateless descriptors (MenuBar,
//! ContextMenu, Form, etc.). This module provides **controllers** that
//! own the interaction state machine for common compositions — so apps
//! define structure + handle semantic events, without reimplementing
//! open/close/navigate/hover logic.
//!
//! - [`FocusGroup`] — Tab/Shift+Tab cycling through N regions by index.
//! - [`FocusRing`] — Tab/Shift+Tab cycling through widget IDs.
//! - [`MenuSystem`] — MenuBar + ContextMenu dropdown composition.
//! - [`SidebarSystem`] — MSV + TreeView sidebar panel composition.
//! - [`FormController`] — single Form with built-in scrollbar + event dispatch.
//! - [`TreeController`] — single keyboard-navigable TreeView + scrollbar.
//! - [`AppShell`] — ActivityBar + sidebar panel container composition.

pub mod app_shell;
pub mod focus_group;
pub mod focus_ring;
pub mod form_controller;
pub mod menu_system;
pub mod sidebar_system;
pub mod status_bar_interaction;
pub mod tree_controller;

pub use app_shell::{AppShell, AppShellEvent, AppShellLayout, PanelDefinition, ShellPosition};
pub use focus_group::FocusGroup;
pub use focus_ring::FocusRing;
pub use form_controller::{FormController, FormControllerEvent};
pub use menu_system::{MenuDef, MenuEvent, MenuSystem};
pub use sidebar_system::{
    NavigationMode, SectionKind, SidebarEvent, SidebarSectionDef, SidebarSystem,
};
pub use status_bar_interaction::{StatusBarAction, StatusBarInteraction};
pub use tree_controller::{TreeController, TreeControllerEvent};
