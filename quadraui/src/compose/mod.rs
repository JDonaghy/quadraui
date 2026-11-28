//! High-level helpers that compose multiple primitives into reusable
//! interaction patterns.
//!
//! The `primitives` module provides stateless descriptors (MenuBar,
//! ContextMenu, Form, etc.). This module provides **controllers** that
//! own the interaction state machine for common compositions — so apps
//! define structure + handle semantic events, without reimplementing
//! open/close/navigate/hover logic.
//!
//! - [`AppMenu`] — MenuBar + ContextMenu dropdown cascade.
//! - [`Sidebar`] — a stack of collapsible tree and form sections.
//! - [`FormController`] — single Form with built-in scrollbar + event dispatch.
//! - [`TreeTableController`] — hierarchical model (lazy children,
//!   sibling-scoped sort) flattened into a `TreeTable`, over
//!   [`crate::behaviour::nav::TreeNavCore`].
//! - [`ShellView`] — ActivityBar + sidebar panel container composition.
//! - [`PathPicker`] — cross-backend filesystem picker modal:
//!   [`FolderPicker`] browses directories, [`FilePicker`] opens or saves
//!   files.
//! - [`MessageDialog`] — cross-backend message/alert box.
//! - [`FormDialog`] — a [`crate::primitives::dialog::Dialog`]
//!   hosting a multi-field [`crate::primitives::form::Form`]
//!   (`DialogInput::Form`), with Tab/Shift+Tab handed off between the
//!   form's fields and the dialog's buttons.
//! - [`DualModePalette`] — Palette that toggles between text-input
//!   mode (free-text confirm) and list/search mode (item selection).
//! - [`Chat`] — chat overlay with scrollable transcript + multi-line
//!   input + status strip.
//! - [`TabGroup`] — editor-group-style tabbed split panes: a `TabBar` per
//!   pane, `SplitTree` dividers and a `DropOverlay` for tab drags.
//! - [`HelpRegistry`] / [`HelpOverlay`] — per-view help registry +
//!   `?`-triggered cheatsheet overlay, plus helpers to feed registered
//!   actions into the command palette.
//! - [`Workspace`] — open-N-view-one document set (ordered opaque ids,
//!   one active, an optional preview) painted through a `TabBar` child;
//!   the host paints the body, which can borrow app state.
//! - [`notify_or_toast`] — send a [`crate::backend::Notification`]
//!   natively where the backend supports it, degrade to the in-canvas
//!   `Toast` primitive where it doesn't.
//! - [`SidebarPanelBody`] — background fill + optional header/search
//!   chrome + body widget + scrollbar-gutter reservation for a sidebar
//!   panel, composed from existing `Backend` methods.
//! - [`ToastStackController`] — keyboard-focus cursor for the non-modal
//!   [`crate::primitives::toast::ToastOverlay`]: Tab/Shift+Tab/Left/Right
//!   cycle a focused toast's buttons, Up/Down move between toasts, Enter
//!   activates, Escape dismisses.
//! - [`HoverTooltipTracker`] — delay + auto-dismiss state machine for a
//!   hover-driven tooltip, sitting on top of [`ToolbarHoverTracker`]/
//!   [`ListHoverTracker`]'s plain "what's hovered"
//!   answer.
//! - [`Handled`] — the shared "did you handle it" return type new
//!   `handle_*` controller methods converge on: a semantic event to
//!   route, a state-only consumption, or "not mine".
//!
//! [`crate::ScrollView`] is a `primitives` type, not a controller here —
//! it is the generic seam for a host that scrolls content it lays out
//! itself. [`FormController`] and [`Chat`] keep their own
//! row-indexed scroll state unchanged; a future controller
//! for scrolling arbitrary child content (a help panel, a form section)
//! composes [`crate::ScrollView`] the same way these compose their own
//! primitive instead of duplicating offset/thumb math.

pub mod app_menu;
pub mod app_shell;
pub mod bottom_panel;
pub mod chat;
pub mod context_menu_controller;
pub mod dual_mode_palette;
pub mod file_picker;
pub mod folder_picker;
pub mod form_controller;
pub mod form_dialog;
mod handled;
pub mod help_layer;
pub mod hover_tooltip_tracker;
pub mod markdown;
pub mod message_dialog;
pub mod notification;
mod palette_host;
mod path_picker;
mod shell_layout;
pub mod sidebar;
pub mod sidebar_panel_body;
pub mod status_bar_interaction;
mod tab_drag;
pub mod tab_group;
pub mod toast_stack;
pub mod toolbar_hover_tracker;
// Drives `TreeTable`'s `Interactive` impl, which exists only where a
// backend does.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub mod tree_table_controller;
pub(crate) mod widget_host;
pub use widget_host::{drive_ui_event, routed_event, Driven};
pub mod workspace;

pub use app_shell::{
    BottomBand, PanelDefinition, ShellDrag, ShellEvent, ShellLayout, ShellPosition, ShellRegion,
    ShellView, ShellViewLayout, ShellViewState,
};
pub use bottom_panel::{
    BackendWidget, BottomPanel, BottomPanelConfig, BottomPanelEvent, BottomPanelItem,
    BottomPanelPrimitiveLayout, BottomPanelState, BottomPanelTab,
};
pub use chat::{Chat, ChatEvent, ChatLayout, ChatRole, ChatState, ChatTurn, ChatZones};
pub use context_menu_controller::{ContextMenuController, ContextMenuOutcome};
pub use dual_mode_palette::{DualModePalette, DualModePaletteAction, DualModePaletteState};
pub use file_picker::{FileKind, FilePicker, FilePickerMode};
pub use folder_picker::{FolderKind, FolderPicker};
pub use form_controller::{FormController, FormControllerAction};
// `FormControllerEvent` is `#[deprecated]` (use `FormController::handle_event`/
// `handle_cached_event`'s `Handled<FormControllerAction>` instead). Allow it
// here since `pub use` of a deprecated item is itself a use site.
pub use app_menu::{AppMenu, AppMenuEvent, AppMenuLayout, AppMenuState, AppMenuView, MenuDef};
#[allow(deprecated)]
pub use form_controller::FormControllerEvent;
pub use form_dialog::{FormDialog, FormDialogAction, FormDialogLayout, FormDialogState};
pub use handled::Handled;
pub use help_layer::{
    filter_help_actions, help_actions_to_palette_items, HelpAction, HelpNote, HelpOverlay,
    HelpOverlayAction, HelpOverlayLayout, HelpOverlayState, HelpRegistry, ViewHelp,
};
pub use hover_tooltip_tracker::HoverTooltipTracker;
pub use markdown::{render_markdown_to_styled_wrapped, CodeBlockRange, RenderedMarkdown};
pub use message_dialog::{MessageDialog, MessageDialogState};
pub use notification::notify_or_toast;
pub use path_picker::{
    walk_entries, PathPicker, PathPickerAction, PathPickerEditState, PathPickerState, PickerKind,
    WalkOptions, DEFAULT_IGNORE_DIRS, DEFAULT_MAX_DEPTH, PALETTE_CHROME_ROWS,
};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use sidebar::SidebarLayout;
pub use sidebar::{
    NavigationMode, SectionKind, Sidebar, SidebarAction, SidebarBody, SidebarSection,
    SidebarSectionDef, SidebarState,
};
pub use sidebar_panel_body::{SidebarPanelBody, SidebarPanelBodyLayout, SidebarPanelChrome};
// `StatusBarAction` is `#[deprecated]` (use `StatusBarInteraction::handle_event`'s
// `Handled<WidgetId>` instead). Allow it here since `pub use` of a deprecated
// item is itself a use site.
#[allow(deprecated)]
pub use status_bar_interaction::StatusBarAction;
pub use status_bar_interaction::StatusBarInteraction;
pub use tab_drag::{resolve_tab_drop, TabDragSource, TabDropInstruction};
pub use tab_group::{
    GroupLayout, TabGroup, TabGroupEvent, TabGroupPane, TabGroupPaneLayout,
    TabGroupPrimitiveLayout, TabGroupState, TabGroupSubPart, TabGroupTab,
};
pub use toast_stack::{ToastStackAction, ToastStackController};
// `ToastStackEvent` is `#[deprecated]` (use `ToastStackController::handle_event`'s
// `Handled<ToastStackAction>` instead). Allow it here since `pub use` of a
// deprecated item is itself a use site.
pub use crate::behaviour::nav::Edge;
#[allow(deprecated)]
pub use toast_stack::ToastStackEvent;
pub use toolbar_hover_tracker::{ListHoverTracker, ToolbarHoverTracker};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use tree_table_controller::{
    NodeId, TreeTableController, TreeTableControllerEvent, TreeTableModel, TreeTableModelLayout,
    TreeTableModelState, TreeTableNodeData,
};
pub use workspace::{
    Workspace, WorkspaceDoc, WorkspaceEvent, WorkspacePrimitiveLayout, WorkspaceState,
};
