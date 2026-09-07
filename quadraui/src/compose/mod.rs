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
//! - [`DualModePaletteController`] — Palette that toggles between text-input
//!   mode (free-text confirm) and list/search mode (item selection).
//! - [`ChatController`] — chat overlay with scrollable transcript + multi-line
//!   input + status strip.
//! - [`TabGroupController`] — TabBar + Split + DropZone + FocusGroup wired into
//!   editor-group-style tabbed split panes.
//! - [`HelpRegistry`] / [`HelpOverlayController`] — per-view help registry +
//!   `?`-triggered cheatsheet overlay, plus helpers to feed registered
//!   actions into the command palette.
//! - [`WorkspaceController`] — open-N-view-one document set (ordered
//!   opaque ids, one active) rendered through the `TabBar` primitive.
//!   Unlike [`TabGroupController`] it owns no content, so the host can
//!   paint a body that borrows app state (#596).
//!
//! # Adopt-or-demote pass (#825, before the `v0.1.0` tag)
//!
//! `KeyMap`/`KeyContext` (#473) had zero constructors anywhere —
//! neither consumer, nor this crate's own examples/tests beyond its
//! own unit-test module — so it was demoted out of this module to
//! `examples/common/key_map.rs` as a copy-paste recipe rather than
//! frozen as public API by the upcoming tag. See that file's module
//! doc for the full disposition. The other #825 candidates stay here,
//! each for a stated reason: [`FocusRing`] is the prerequisite shape
//! for #788's focus manager; [`FolderPickerController`] and
//! [`TabGroupController`] already carry full TUI+GTK demo + driver-test
//! coverage and a documented migration target (`FolderPickerController`'s
//! own module doc names the vimcode call site it was extracted from);
//! [`BottomPanelController`] is already load-bearing — it's what
//! `ShellConfig::bottom_panel` / `AppShell::with_bottom_panel` construct
//! (`shell.rs`/`shell_adapter.rs`), not a standalone unused type.

pub mod app_shell;
pub mod bottom_panel;
pub mod chat_controller;
pub mod dual_mode_palette;
pub mod focus_group;
pub mod focus_ring;
pub mod folder_picker;
pub mod form_controller;
pub mod help_layer;
pub mod markdown;
pub mod menu_system;
pub mod sidebar_system;
pub mod status_bar_interaction;
pub mod tab_group;
pub mod toolbar_hover_tracker;
pub mod tree_controller;
pub mod workspace;

pub use app_shell::{AppShell, AppShellEvent, AppShellLayout, PanelDefinition, ShellPosition};
pub use bottom_panel::{
    BackendWidget, BottomPanelConfig, BottomPanelController, BottomPanelEvent, BottomPanelLayout,
    BottomPanelTab,
};
pub use chat_controller::{ChatController, ChatControllerEvent, ChatRole, ChatTurn};
pub use dual_mode_palette::{DualModePaletteController, DualModePaletteEvent};
pub use focus_group::FocusGroup;
pub use focus_ring::FocusRing;
pub use folder_picker::{FolderPickerController, FolderPickerEvent, PALETTE_CHROME_ROWS};
pub use form_controller::{FormController, FormControllerEvent};
pub use help_layer::{
    filter_help_actions, help_actions_to_palette_items, HelpAction, HelpNote,
    HelpOverlayController, HelpOverlayEvent, HelpRegistry, ViewHelp,
};
pub use markdown::{render_markdown_to_styled_wrapped, CodeBlockRange, RenderedMarkdown};
pub use menu_system::{MenuDef, MenuEvent, MenuSystem};
pub use sidebar_system::{
    NavigationMode, SectionKind, SidebarEvent, SidebarSectionDef, SidebarSystem,
};
pub use status_bar_interaction::{StatusBarAction, StatusBarInteraction};
pub use tab_group::{
    GroupLayout, Pane, PaneDragRect, PaneTab, TabGroupController, TabGroupEvent, TabGroupLayout,
};
pub use toolbar_hover_tracker::ToolbarHoverTracker;
pub use tree_controller::{TreeController, TreeControllerEvent};
pub use workspace::{WorkspaceController, WorkspaceDoc, WorkspaceEvent, WorkspaceLayout};
