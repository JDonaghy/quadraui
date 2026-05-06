//! High-level helpers that compose multiple primitives into reusable
//! interaction patterns.
//!
//! The `primitives` module provides stateless descriptors (MenuBar,
//! ContextMenu, Form, etc.). This module provides **controllers** that
//! own the interaction state machine for common compositions — so apps
//! define structure + handle semantic events, without reimplementing
//! open/close/navigate/hover logic.
//!
//! - [`FocusRing`] — Tab/Shift+Tab cycling through widget IDs.
//! - [`MenuSystem`] — MenuBar + ContextMenu dropdown composition.
//! - [`SidebarSystem`] — MSV + TreeView sidebar panel composition.

pub mod focus_ring;
pub mod menu_system;
pub mod sidebar_system;

pub use focus_ring::FocusRing;
pub use menu_system::{MenuDef, MenuEvent, MenuSystem};
pub use sidebar_system::{SidebarEvent, SidebarSectionDef, SidebarSystem};
