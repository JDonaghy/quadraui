//! `ActivityBar` primitive: a vertical strip of icon buttons (VSCode-style
//! left rail). Items are split into a top group (rendered from the top)
//! and an optional bottom group (pinned to the bottom of the available
//! area). Each item is a single clickable icon with active / keyboard-
//! selection visual states and an optional tooltip.
//!
//! Typical vimcode layout:
//! * Top: hamburger / menu, explorer, search, debug, git, extensions, AI,
//!   dynamically-registered extension panels
//! * Bottom: settings (gear)
//!
//! Click resolution is per-backend — TUI computes from cell-row arithmetic,
//! GTK from pixel-row arithmetic. The primitive itself carries no layout
//! calculation; it's a declarative list.
//!
//! # Backend contract
//!
//! **Declarative + per-frame interaction state passed alongside.** Render
//! the `top_items` from the top of the strip, then the `bottom_items`
//! pinned to the bottom. Click on item → emit
//! `ActivityBarEvent::ItemClicked { id }`.
//!
//! Hover state (which item the mouse is currently over for tooltip
//! affordance) is **per-frame, backend-owned** — the primitive does NOT
//! carry it. Backends pass `hovered_idx: Option<usize>` to their own
//! `draw_activity_bar` function. Same pattern as `TabBar`'s
//! `hovered_close_tab`. Rule: **state that's only knowable by the
//! backend (cursor position, focus-within, scroll momentum) lives
//! beside the primitive, not inside it.**

use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of an activity bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityBar {
    pub id: WidgetId,
    /// Top items rendered starting at the top edge, one row per item.
    pub top_items: Vec<ActivityItem>,
    /// Bottom-pinned items rendered from the bottom edge upward.
    /// Rendered only if there's room after `top_items`.
    #[serde(default)]
    pub bottom_items: Vec<ActivityItem>,
    /// Colour of the left-edge accent bar on active items.
    /// `None` = no accent rendering.
    #[serde(default)]
    pub active_accent: Option<Color>,
    /// Background colour for keyboard-selected items (arrow-nav highlight).
    /// `None` = backends fall back to their own default.
    #[serde(default)]
    pub selection_bg: Option<Color>,
}

/// One icon entry in an `ActivityBar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityItem {
    /// Opaque widget id for click routing. The adapter picks a meaningful
    /// namespaced string (e.g. `"activity:explorer"`, `"activity:ext:foo"`).
    pub id: WidgetId,
    /// Glyph to render — single character for the TUI cell-based layout;
    /// GTK backend can render wider strings when font supports them.
    pub icon: String,
    /// Hover tooltip text. TUI ignores (no hover UI); GTK uses as a native
    /// `set_tooltip_text`.
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub is_active: bool,
    /// Keyboard-focused selection highlight (used by TUI when the activity
    /// bar has `toolbar_focused`). GTK rarely sets this — native buttons
    /// manage their own focus rings.
    #[serde(default)]
    pub is_keyboard_selected: bool,
}

/// Events an `ActivityBar` emits back to the app. Currently unused by
/// vimcode (click path dispatches by row arithmetic + engine-side
/// `SidebarPanel` enum), but defined for plugin invariants §10.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityBarEvent {
    /// An item was clicked (or activated via Enter while keyboard-focused).
    ItemClicked { id: WidgetId },
    /// A key was pressed with the activity bar focused and the primitive
    /// didn't consume it.
    KeyPressed { key: String, modifiers: Modifiers },
}
