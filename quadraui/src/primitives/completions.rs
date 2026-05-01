//! `Completions` primitive: an LSP-style autocomplete popup anchored to
//! the cursor position. Similar to `ListView` but tailored for
//! completion semantics — per-item `kind` icon (function / variable /
//! snippet / etc.), right-aligned type detail, optional
//! documentation preview.
//!
//! Unlike `Palette`, the query isn't inside the primitive — it's
//! implicit in the cursor position. The app filters its source against
//! the current word-under-cursor and passes the visible `items`.
//!
//! # Backend contract
//!
//! **Declarative + anchor-positioned.** Apps pass the cursor position
//! and the primitive's `layout()` picks a position that keeps the
//! popup inside the viewport (prefer below the cursor, flip to above
//! if it would overflow). Click on item → `ItemActivated { idx }`;
//! keyboard navigation emits the respective events; Escape / Enter /
//! Tab follow their typical IDE semantics.

use crate::event::Rect;
use crate::types::{Icon, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a completion popup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completions {
    pub id: WidgetId,
    pub items: Vec<CompletionItem>,
    pub selected_idx: usize,
    #[serde(default)]
    pub scroll_offset: usize,
    /// When true, the primitive is "live" and receiving focus for
    /// keyboard nav. When false it's suppressed (rendered but
    /// non-interactive) — apps set this based on their own state.
    #[serde(default)]
    pub has_focus: bool,
}

/// One completion candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    /// Display label (what the user sees in the popup). Usually the
    /// same as the insertion text, but LSP can differ (e.g. label
    /// `"map(..)"` inserts `"map"`).
    pub label: StyledText,
    /// Right-aligned type / detail text, e.g. `"fn(T) -> U"` or
    /// `"crate::foo"`.
    #[serde(default)]
    pub detail: Option<StyledText>,
    /// Optional documentation snippet shown in a preview panel. The
    /// primitive doesn't render this — apps display it separately when
    /// they care.
    #[serde(default)]
    pub documentation: Option<StyledText>,
    /// Kind icon (LSP CompletionItemKind mapping).
    #[serde(default)]
    pub kind: CompletionKind,
    /// Icon override. Backends default to a theme-defined glyph per
    /// `kind` if this is `None`.
    #[serde(default)]
    pub icon: Option<Icon>,
}

/// Kind of completion — drives the default icon + colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompletionKind {
    #[default]
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

/// Events a `Completions` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionsEvent {
    /// Selection moved (keyboard or mouse hover).
    SelectionChanged { idx: usize },
    /// User confirmed the selected item (Enter, Tab, or click).
    ItemActivated { idx: usize },
    /// Popup dismissed (Escape, click outside, typed a character that
    /// invalidates the list).
    Cancelled,
    /// Key passed through — app may use it to filter further or trigger
    /// signature-help.
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Per-item measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompletionItemMeasure {
    pub height: f32,
}

impl CompletionItemMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }
}

/// Resolved position of one visible completion item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleCompletion {
    pub item_idx: usize,
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionsHit {
    Item(usize),
    /// Click inside the popup but not on an item (border padding).
    Inert,
    /// Click outside the popup — typically dismisses.
    Empty,
}

/// Resolved placement direction for the popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionsPlacement {
    /// Popup rendered below the cursor anchor.
    Below,
    /// Popup rendered above the cursor anchor (flipped to avoid
    /// overflow).
    Above,
}

/// Fully-resolved completions-popup layout.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionsLayout {
    pub bounds: Rect,
    pub placement: CompletionsPlacement,
    pub visible_items: Vec<VisibleCompletion>,
    pub hit_regions: Vec<(Rect, CompletionsHit)>,
    pub resolved_scroll_offset: usize,
}

impl CompletionsLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> CompletionsHit {
        let inside = x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height;
        if !inside {
            return CompletionsHit::Empty;
        }
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        CompletionsHit::Inert
    }
}

impl Completions {
    /// Compute popup placement + per-item bounds.
    ///
    /// # Arguments
    ///
    /// - `cursor_x`, `cursor_y` — caret position in viewport coords.
    /// - `line_height` — height of the line containing the cursor
    ///   (used for the gap between cursor and popup).
    /// - `viewport` — parent surface bounds.
    /// - `popup_width`, `max_popup_height` — size constraints for the
    ///   popup.
    /// - `measure_item(i)` — height for item `i`.
    ///
    /// # Placement
    ///
    /// Prefer below the cursor; flip above if below would overflow.
    /// Shift left if the popup would extend past the right edge.
    #[allow(clippy::too_many_arguments)]
    pub fn layout<F>(
        &self,
        cursor_x: f32,
        cursor_y: f32,
        line_height: f32,
        viewport: Rect,
        popup_width: f32,
        max_popup_height: f32,
        measure_item: F,
    ) -> CompletionsLayout
    where
        F: Fn(usize) -> CompletionItemMeasure,
    {
        // Clamp scroll offset.
        let resolved_scroll_offset = if self.items.is_empty() {
            0
        } else {
            self.scroll_offset.min(self.items.len() - 1)
        };

        // Determine the popup's height (bounded by max and total content).
        let total_content: f32 = (resolved_scroll_offset..self.items.len())
            .map(|i| measure_item(i).height)
            .sum();
        let desired_height = total_content.min(max_popup_height);

        // Vertical placement: below cursor if it fits, else above.
        let below_y = cursor_y + line_height;
        let above_y = cursor_y - desired_height;
        let below_fits = below_y + desired_height <= viewport.y + viewport.height;
        let above_fits = above_y >= viewport.y;
        let (y, placement) = if below_fits {
            (below_y, CompletionsPlacement::Below)
        } else if above_fits {
            (above_y, CompletionsPlacement::Above)
        } else {
            // Neither fits cleanly: pick the side with more room and
            // clip.
            let below_room = viewport.y + viewport.height - below_y;
            let above_room = cursor_y - viewport.y;
            if below_room >= above_room {
                (below_y, CompletionsPlacement::Below)
            } else {
                (
                    (cursor_y - desired_height).max(viewport.y),
                    CompletionsPlacement::Above,
                )
            }
        };

        // Horizontal: prefer cursor_x, shift left if overflow.
        let x = if cursor_x + popup_width > viewport.x + viewport.width {
            (viewport.x + viewport.width - popup_width).max(viewport.x)
        } else {
            cursor_x.max(viewport.x)
        };

        // Clip popup to the viewport.
        let clipped_h = (viewport.y + viewport.height - y)
            .min(desired_height)
            .max(0.0);
        let bounds = Rect::new(x, y, popup_width, clipped_h);

        let mut visible_items: Vec<VisibleCompletion> = Vec::new();
        let mut hit_regions: Vec<(Rect, CompletionsHit)> = Vec::new();

        let mut cursor_row = y;
        for i in resolved_scroll_offset..self.items.len() {
            if cursor_row >= y + clipped_h {
                break;
            }
            let m = measure_item(i);
            let remaining = y + clipped_h - cursor_row;
            let draw_h = m.height.min(remaining).max(0.0);
            if draw_h <= 0.0 {
                break;
            }
            let item_bounds = Rect::new(x, cursor_row, popup_width, draw_h);
            visible_items.push(VisibleCompletion {
                item_idx: i,
                bounds: item_bounds,
            });
            hit_regions.push((item_bounds, CompletionsHit::Item(i)));
            cursor_row += m.height;
        }

        CompletionsLayout {
            bounds,
            placement,
            visible_items,
            hit_regions,
            resolved_scroll_offset,
        }
    }
}
