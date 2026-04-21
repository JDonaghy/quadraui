//! `TabBar` primitive: a horizontal strip of tabs followed by right-aligned
//! action segments (e.g. split buttons, diff toolbar, overflow menu).
//!
//! Unlike `StatusBar`, a `TabBar` has tab-specific semantics — each tab
//! carries `is_active` / `is_dirty` / `is_preview` visual states and an
//! optional close button with its own click target. Apps render the close
//! button inline with the tab label.
//!
//! Right-aligned segments (`right_segments`) are generic clickable icon /
//! label slots for the buttons that live at the far right of the bar: split
//! controls, action menus, diff toolbars, etc. Non-clickable labels (e.g.
//! "2 of 5" in a diff toolbar) set `id = None`.
//!
//! Scope in A.6c: the primitive defines declarative state + events for
//! plugin readiness, but vimcode's click path still resolves through the
//! existing engine-side `TabBarClickTarget` enum. `TabBarEvent` exists
//! for a later stage where plugin-defined tab bars use event-driven clicks.

use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a tab bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabBar {
    pub id: WidgetId,
    pub tabs: Vec<TabItem>,
    /// Index of the first visible tab when tabs overflow. Tabs before this
    /// index are hidden; tabs after are clipped as needed.
    #[serde(default)]
    pub scroll_offset: usize,
    /// Right-aligned trailing segments, drawn in order from left to right
    /// starting at `bar_width - sum(widths)`. Use this slot for toolbar
    /// buttons and inline labels.
    #[serde(default)]
    pub right_segments: Vec<TabBarSegment>,
    /// Optional colour used to underline the active tab's filename portion.
    /// `None` = no underline accent (typical for inactive groups).
    #[serde(default)]
    pub active_accent: Option<Color>,
}

/// One tab in a `TabBar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabItem {
    /// Display label, e.g. `" 3: main.rs "`. Backends may underline a subset
    /// (the filename portion after the last `": "`) — they are responsible
    /// for locating the filename boundary from this string.
    pub label: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub is_dirty: bool,
    #[serde(default)]
    pub is_preview: bool,
}

/// One right-aligned segment in a `TabBar`. Either a clickable button
/// (with an `id`) or a non-interactive label (`id = None`).
///
/// `width_cells` is the pre-computed width in TUI character cells. The
/// adapter fills this in based on whether Nerd Font icons are enabled
/// (wide glyphs take 2 cells, ASCII fallbacks 1). GTK / Direct2D backends
/// use `width_cells` for click-region book-keeping in cell units; pixel
/// positioning is done by Pango measurement at draw time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabBarSegment {
    /// Text / icon glyph to render (e.g. `" … "`, `" ⇅ "`, `"2 of 5"`).
    pub text: String,
    pub width_cells: u16,
    /// `None` = non-interactive. `Some(id)` = clickable; backend emits
    /// `ButtonClicked { id }` when resolving a hit on this segment.
    #[serde(default)]
    pub id: Option<WidgetId>,
    /// Highlighted (toggled-on) state, e.g. diff-fold-toggle when folded.
    #[serde(default)]
    pub is_active: bool,
}

impl TabBar {
    /// Find the smallest scroll offset such that the tab at `active` is
    /// visible inside `available_width`, given a per-tab measurement
    /// function. Returns the index of the first tab to render.
    ///
    /// Generic over the unit system: `measure(i)` and `available_width`
    /// must use the same units. Each backend supplies its native measurer:
    ///
    /// - TUI passes char-cell counts.
    /// - GTK passes Pango pixel widths (label + tab padding + close button).
    /// - Win-GUI / macOS pass DirectWrite / Core Text pixel widths.
    ///
    /// This is the unit-agnostic counterpart to vimcode's
    /// `Engine::ensure_active_tab_visible` (which is hardcoded to char
    /// units suited for TUI). Backends with non-char rendering MUST use
    /// this helper instead of the engine algorithm — otherwise the
    /// engine's per-tab width estimate will mismatch actual rendering and
    /// the active tab can land off-screen.
    ///
    /// **Algorithm**: try offset 0 first (maximises visible tabs). If
    /// `active` doesn't fit there, walk backwards from `active`,
    /// accumulating widths, and return the smallest offset where it
    /// still fits. Mirrors the engine's algorithm bit-for-bit so the
    /// behavioural contract is identical across backends.
    pub fn fit_active_scroll_offset<F>(
        active: usize,
        tab_count: usize,
        available_width: usize,
        measure: F,
    ) -> usize
    where
        F: Fn(usize) -> usize,
    {
        if tab_count == 0 || active >= tab_count {
            return 0;
        }
        // How many fit starting from offset 0?
        let mut used = 0;
        let mut from_zero = 0;
        for i in 0..tab_count {
            let w = measure(i);
            if used + w > available_width {
                break;
            }
            used += w;
            from_zero += 1;
        }
        if active < from_zero {
            return 0;
        }
        // Walk backwards from active to find the smallest offset where
        // active still fits at the right edge.
        let mut used = 0;
        let mut best_offset = active;
        for i in (0..=active).rev() {
            let w = measure(i);
            if used + w > available_width {
                break;
            }
            used += w;
            best_offset = i;
        }
        best_offset
    }
}

/// Events a `TabBar` emits back to the app. Currently unused by vimcode
/// (click path goes through the engine's `TabBarClickTarget` enum), but
/// defined for plugin invariants §10 — plugin-declared tab bars will
/// consume events directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabBarEvent {
    /// User clicked a tab body (not its close button) — index is into
    /// `tabs`, matching the visible order (scroll_offset still applies).
    TabActivated { index: usize },
    /// User clicked a tab's close button.
    TabClosed { index: usize },
    /// User clicked a right-side segment with a non-`None` id.
    ButtonClicked { id: WidgetId },
    /// A key was pressed with the tab bar focused and the primitive didn't
    /// consume it. Currently unused by vimcode (tab bars don't take
    /// keyboard focus) but kept for shape parity.
    KeyPressed { key: String, modifiers: Modifiers },
}
