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
//! - [`TreeController`] — single keyboard-navigable TreeView + scrollbar.

pub mod focus_group;
pub mod focus_ring;
pub mod menu_system;
pub mod sidebar_system;
pub mod status_bar_interaction;
pub mod tree_controller;

pub use focus_group::FocusGroup;
pub use focus_ring::FocusRing;
pub use menu_system::{MenuDef, MenuEvent, MenuSystem};
pub use sidebar_system::{NavigationMode, SidebarEvent, SidebarSectionDef, SidebarSystem};
pub use status_bar_interaction::{StatusBarAction, StatusBarInteraction};
pub use tree_controller::{TreeController, TreeControllerEvent};
