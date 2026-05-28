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
//! - [`FolderPickerController`] — cross-backend directory-browsing modal.
//! - [`ChatController`] — chat overlay with scrollable transcript + multi-line
//!   input + status strip.

pub mod app_shell;
pub mod chat_controller;
pub mod focus_group;
pub mod focus_ring;
pub mod folder_picker;
pub mod form_controller;
pub mod menu_system;
pub mod sidebar_system;
pub mod status_bar_interaction;
pub mod toolbar_hover_tracker;
pub mod tree_controller;

pub use app_shell::{AppShell, AppShellEvent, AppShellLayout, PanelDefinition, ShellPosition};
pub use chat_controller::{ChatController, ChatControllerEvent, ChatRole, ChatTurn};
pub use focus_group::FocusGroup;
pub use focus_ring::FocusRing;
pub use folder_picker::{FolderPickerController, FolderPickerEvent, PALETTE_CHROME_ROWS};
pub use form_controller::{FormController, FormControllerEvent};
pub use menu_system::{MenuDef, MenuEvent, MenuSystem};
pub use sidebar_system::{
    NavigationMode, SectionKind, SidebarEvent, SidebarSectionDef, SidebarSystem,
};
pub use status_bar_interaction::{StatusBarAction, StatusBarInteraction};
pub use toolbar_hover_tracker::ToolbarHoverTracker;
pub use tree_controller::{TreeController, TreeControllerEvent};
