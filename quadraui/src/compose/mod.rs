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
//! - [`FilePickerController`] — cross-backend file open/save modal.
//! - [`MessageDialogController`] — cross-backend message/alert-box driver.
//! - [`FormDialogController`] — a [`crate::primitives::dialog::Dialog`]
//!   hosting a multi-field [`crate::primitives::form::Form`]
//!   (`DialogInput::Form`), with Tab/Shift+Tab handed off between the
//!   form's fields and the dialog's buttons.
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
//!   paint a body that borrows app state.
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
//!   [`ListHoverTracker`]/[`TreeHoverTracker`]'s plain "what's hovered"
//!   answer.
//! - [`Handled`] — the shared "did you handle it" return type new
//!   `handle_*` controller methods converge on: a semantic event to
//!   route, a state-only consumption, or "not mine".
//!
//! [`crate::ScrollView`] is a `primitives` type, not a controller here —
//! it is the generic seam for a host that scrolls content it lays out
//! itself. [`FormController`], [`TreeController`], and [`ChatController`]
//! keep their own row-indexed scroll state unchanged; a future controller
//! for scrolling arbitrary child content (a help panel, a form section)
//! composes [`crate::ScrollView`] the same way these compose their own
//! primitive instead of duplicating offset/thumb math.

mod activity_bar_controller;
pub mod app_shell;
pub mod bottom_panel;
pub mod chat_controller;
pub mod context_menu_controller;
mod divider_drag;
pub mod dual_mode_palette;
pub mod file_picker;
pub mod focus_group;
pub mod focus_ring;
pub mod folder_picker;
pub mod form_controller;
pub mod form_dialog;
mod handled;
pub mod help_layer;
pub mod hover_tooltip_tracker;
pub mod markdown;
pub mod menu_system;
pub mod message_dialog;
pub mod notification;
mod path_picker;
mod shell_layout;
mod sidebar_edit;
pub mod sidebar_panel_body;
mod sidebar_scroll;
pub mod sidebar_system;
pub mod status_bar_interaction;
mod tab_drag;
pub mod tab_group;
pub mod toast_stack;
pub mod toolbar_hover_tracker;
pub mod tree_controller;
pub mod workspace;

pub use app_shell::{
    AppShell, AppShellEvent, AppShellLayout, BottomBand, PanelDefinition, ShellPosition,
};
pub use bottom_panel::{
    BackendWidget, BottomPanelConfig, BottomPanelController, BottomPanelEvent, BottomPanelLayout,
    BottomPanelTab,
};
pub use chat_controller::{ChatController, ChatControllerEvent, ChatRole, ChatTurn};
pub use context_menu_controller::{ContextMenuController, ContextMenuOutcome};
pub use dual_mode_palette::{DualModePaletteController, DualModePaletteEvent};
pub use file_picker::{FilePickerController, FilePickerEvent, FilePickerMode};
pub use focus_group::FocusGroup;
pub use focus_ring::FocusRing;
pub use folder_picker::{FolderPickerController, FolderPickerEvent, PALETTE_CHROME_ROWS};
pub use form_controller::{FormController, FormControllerEvent};
pub use form_dialog::{FormDialogController, FormDialogEvent};
pub use handled::Handled;
pub use help_layer::{
    filter_help_actions, help_actions_to_palette_items, HelpAction, HelpNote,
    HelpOverlayController, HelpOverlayEvent, HelpRegistry, ViewHelp,
};
pub use hover_tooltip_tracker::HoverTooltipTracker;
pub use markdown::{render_markdown_to_styled_wrapped, CodeBlockRange, RenderedMarkdown};
pub use menu_system::{MenuDef, MenuEvent, MenuSystem};
pub use message_dialog::MessageDialogController;
// `MessageDialogEvent` is `#[deprecated]` (use `MessageDialogController::handle_event`'s
// `Handled<MessageDialogChoice>` instead). Allow it here since `pub use` of a
// deprecated item is itself a use site.
#[allow(deprecated)]
pub use message_dialog::MessageDialogEvent;
pub use notification::notify_or_toast;
pub use sidebar_panel_body::{SidebarPanelBody, SidebarPanelBodyLayout, SidebarPanelChrome};
pub use sidebar_system::{
    NavigationMode, SectionKind, SidebarEvent, SidebarSectionDef, SidebarSystem,
};
// `StatusBarAction` is `#[deprecated]` (use `StatusBarInteraction::handle_event`'s
// `Handled<WidgetId>` instead). Allow it here since `pub use` of a deprecated
// item is itself a use site.
#[allow(deprecated)]
pub use status_bar_interaction::StatusBarAction;
pub use status_bar_interaction::StatusBarInteraction;
pub use tab_drag::{resolve_tab_drop, TabDragSource, TabDropInstruction};
pub use tab_group::{
    GroupLayout, Pane, PaneDragRect, PaneTab, TabGroupController, TabGroupEvent, TabGroupLayout,
};
pub use toast_stack::{ToastStackAction, ToastStackController};
// `ToastStackEvent` is `#[deprecated]` (use `ToastStackController::handle_event`'s
// `Handled<ToastStackAction>` instead). Allow it here since `pub use` of a
// deprecated item is itself a use site.
#[allow(deprecated)]
pub use toast_stack::ToastStackEvent;
pub use toolbar_hover_tracker::{ListHoverTracker, ToolbarHoverTracker, TreeHoverTracker};
pub use tree_controller::{Edge, TreeController, TreeControllerEvent};
pub use workspace::{WorkspaceController, WorkspaceDoc, WorkspaceEvent, WorkspaceLayout};
