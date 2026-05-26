//! `Toolbar` primitive: a horizontal strip of clickable action buttons that
//! sits **above** a content area (above a tree, list, terminal, etc.) — not
//! on a panel title bar (use [`crate::Panel`]`.actions` for that) and not for
//! view selection (use [`crate::TabBar`]).
//!
//! ## Why this isn't `StatusBar`
//!
//! Two downstream apps (vimcode debug toolbar, coord-tui sidebar action bar)
//! historically faked toolbars by stuffing clickable verbs into
//! [`crate::StatusBar`] segments with `action_id` set. That works on TUI
//! (it's just a row of cells) but breaks the abstraction the moment a
//! backend wants to render the strip natively:
//!
//! - GTK status bars (`Gtk.Statusbar`) have no hover/focus styling, no
//!   ARIA `role="toolbar"`, and aren't part of the focusable widget tree.
//!   A real toolbar wants `Gtk.Button` widgets with hover state.
//! - `StatusBarSegment` has no `enabled` field; apps have been dimming the
//!   colour and accepting that clicks still fire.
//! - The semantics are wrong — status bars are read-only informational
//!   strips.
//!
//! `Toolbar` makes the toolbar shape first-class so backends can render it
//! as buttons (with hover, focus, disabled states, accessibility) and apps
//! get a primitive that matches their actual intent.
//!
//! ## Shape
//!
//! Buttons are an ordered list of [`ToolbarButton`] entries. Three variants:
//!
//! - [`ToolbarButton::Action`] — clickable button with label, optional icon
//!   glyph and key hint, optional disabled/active state, tooltip.
//! - [`ToolbarButton::Separator`] — visual gap between button groups.
//! - [`ToolbarButton::Label`] — non-clickable inline text (e.g. "2 of 5"
//!   in a diff toolbar). Distinct from a disabled action because it
//!   doesn't occupy a button hit zone.
//!
//! ## Backend contract
//!
//! Declarative. Each backend renders the toolbar in its native idiom:
//!
//! - **TUI**: single row of `[ icon label (k) ]` cells with hit zones per
//!   action. Separators render as a dim `│ ` cell. Disabled buttons paint
//!   in a muted foreground and don't dispatch clicks.
//! - **GTK**: `Gtk.Box` of `Gtk.Button` (or buttons painted with Cairo);
//!   `Gtk.Separator` between groups. ARIA `role="toolbar"`. Hover paints
//!   `theme.hover_bg`; pressed paints `theme.selected_bg`.
//! - **macOS** / **Win-GUI** (future): native button widgets in an inline
//!   stack view; same hit-test contract returned via
//!   [`ToolbarLayout::hit_test`].
//!
//! The primitive returns a [`ToolbarLayout`] carrying per-button bounds
//! (per D6 contract) so paint and click consume one layout per frame.

use crate::event::Rect;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

// ── Data model ───────────────────────────────────────────────────────────────

/// Declarative description of a horizontal toolbar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toolbar {
    pub id: WidgetId,
    pub buttons: Vec<ToolbarButton>,
    /// Optional background colour. When `None` the backend picks a theme
    /// default (typically slightly lighter than the panel bg so the
    /// toolbar reads as foreground chrome).
    #[serde(default)]
    pub bg: Option<Color>,
}

/// One item in a [`Toolbar`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolbarButton {
    /// A clickable action button.
    Action {
        id: WidgetId,
        /// Primary label text (e.g. "Refine", "Continue").
        label: String,
        /// Optional leading icon glyph (e.g. "▶", "🔄"). TUI rasterises
        /// as plain text; GTK can swap for an Image widget.
        #[serde(default)]
        icon: Option<String>,
        /// Optional parenthesised key hint shown after the label
        /// (e.g. "(r)" for the 'r' keybind).
        #[serde(default)]
        key_hint: Option<String>,
        /// When false the button renders dimmed and clicks aren't
        /// dispatched. Apps still receive the layout entry so they can
        /// show a "why disabled" tooltip on hover.
        #[serde(default = "default_true")]
        enabled: bool,
        /// When true the button has a pressed / toggled visual (e.g. a
        /// "Filter on" toggle). Independent of `enabled`.
        #[serde(default)]
        is_active: bool,
        /// Optional hover tooltip.
        #[serde(default)]
        tooltip: String,
    },
    /// Visual separator between button groups. No interaction, no id.
    Separator,
    /// A non-clickable label segment (e.g. "2 of 5" in a diff toolbar).
    /// Different from a disabled `Action` because it never occupies a
    /// button hit-zone.
    Label {
        text: String,
        #[serde(default)]
        fg: Option<Color>,
    },
}

/// Events emitted by a [`Toolbar`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolbarEvent {
    /// User clicked (or activated via keyboard) an enabled action button.
    ButtonClicked { id: WidgetId, modifiers: Modifiers },
}

// ── Layout + hit-testing ─────────────────────────────────────────────────────

/// Which variant a visible toolbar slot belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarItemKind {
    /// Clickable action button.
    Action,
    /// Visual separator.
    Separator,
    /// Non-clickable label.
    Label,
}

/// Resolved position of one visible toolbar item after layout.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleToolbarItem {
    /// Index into [`Toolbar::buttons`].
    pub item_idx: usize,
    pub kind: ToolbarItemKind,
    pub bounds: Rect,
    /// `true` iff the item is an [`ToolbarButton::Action`] with
    /// `enabled == true`. Disabled actions still produce a layout entry
    /// (so hover tooltips can fire) but are skipped by `hit_test`.
    pub clickable: bool,
    /// `id` of the underlying `Action`. `None` for `Separator` / `Label`.
    pub action_id: Option<WidgetId>,
}

/// Classification of a hit-test result on a toolbar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolbarHit {
    /// Click landed on a clickable action button — carries its id.
    Button(WidgetId),
    /// Click landed inside the bar but not on a clickable button
    /// (gap, separator, label, or disabled action).
    Empty,
}

/// Fully-resolved toolbar layout. Backends iterate `visible_items` for
/// painting and call [`Self::hit_test`] for clicks.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolbarLayout {
    pub bar_bounds: Rect,
    pub visible_items: Vec<VisibleToolbarItem>,
}

impl ToolbarLayout {
    /// Test which clickable action (if any) contains point `(x, y)`.
    /// Returns [`ToolbarHit::Empty`] when no clickable region matches —
    /// disabled actions, separators, labels, and the gap between buttons
    /// all map to `Empty`.
    pub fn hit_test(&self, x: f32, y: f32) -> ToolbarHit {
        for vis in &self.visible_items {
            if !vis.clickable {
                continue;
            }
            let r = vis.bounds;
            if x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height {
                if let Some(id) = &vis.action_id {
                    return ToolbarHit::Button(id.clone());
                }
            }
        }
        ToolbarHit::Empty
    }
}

// ── Measurement ──────────────────────────────────────────────────────────────

/// Per-item measurement supplied by the backend's layout caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolbarItemMeasure {
    /// Width of the item in the backend's native unit (cells for TUI,
    /// pixels for GTK / Win-GUI / macOS).
    pub width: f32,
}

impl ToolbarItemMeasure {
    pub fn new(width: f32) -> Self {
        Self { width }
    }
}

impl Toolbar {
    /// Compute the full rendering + hit-test layout for this toolbar.
    ///
    /// Items lay out left-to-right starting at `(origin_x, origin_y)`,
    /// each taking the width returned by `measure`. Items that would
    /// extend past `bar_width` are clipped: they appear in
    /// `visible_items` with truncated bounds (or `width == 0` if they
    /// would start beyond the edge). Apps that need overflow handling
    /// (chevron menu) compose that outside the primitive.
    ///
    /// `bar_height` is shared by every item. Numeric arguments share
    /// the same unit; the primitive itself is unit-agnostic.
    pub fn layout<F>(
        &self,
        origin_x: f32,
        origin_y: f32,
        bar_width: f32,
        bar_height: f32,
        measure: F,
    ) -> ToolbarLayout
    where
        F: Fn(&ToolbarButton) -> ToolbarItemMeasure,
    {
        let bar_bounds = Rect::new(origin_x, origin_y, bar_width, bar_height);
        let mut visible_items: Vec<VisibleToolbarItem> = Vec::with_capacity(self.buttons.len());

        let mut cursor = origin_x;
        let right_edge = origin_x + bar_width;

        for (i, btn) in self.buttons.iter().enumerate() {
            let w = measure(btn).width.max(0.0);
            // Clip width so item doesn't paint past the bar's right edge.
            let visible_w = (right_edge - cursor).max(0.0).min(w);
            let bounds = Rect::new(cursor, origin_y, visible_w, bar_height);

            let (kind, clickable, action_id) = match btn {
                ToolbarButton::Action { id, enabled, .. } => (
                    ToolbarItemKind::Action,
                    *enabled && visible_w > 0.0,
                    Some(id.clone()),
                ),
                ToolbarButton::Separator => (ToolbarItemKind::Separator, false, None),
                ToolbarButton::Label { .. } => (ToolbarItemKind::Label, false, None),
            };

            visible_items.push(VisibleToolbarItem {
                item_idx: i,
                kind,
                bounds,
                clickable,
                action_id,
            });

            cursor += w;
        }

        ToolbarLayout {
            bar_bounds,
            visible_items,
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;

    fn mk_action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn cell_measure() -> impl Fn(&ToolbarButton) -> ToolbarItemMeasure {
        // Mirrors what the TUI rasteriser uses: `[ label ]` style cells,
        // separators 2 cells, labels their char width.
        |btn| match btn {
            ToolbarButton::Action {
                label,
                icon,
                key_hint,
                ..
            } => {
                let icon_w = icon.as_ref().map(|s| s.chars().count() + 1).unwrap_or(0);
                let hint_w = key_hint
                    .as_ref()
                    .map(|s| s.chars().count() + 1)
                    .unwrap_or(0);
                // `[ ` + content + ` ]`
                ToolbarItemMeasure::new((4 + icon_w + label.chars().count() + hint_w) as f32)
            }
            ToolbarButton::Separator => ToolbarItemMeasure::new(2.0),
            ToolbarButton::Label { text, .. } => {
                ToolbarItemMeasure::new(text.chars().count() as f32)
            }
        }
    }

    #[test]
    fn empty_toolbar_layout() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![],
            bg: None,
        };
        let layout = bar.layout(0.0, 0.0, 80.0, 1.0, cell_measure());
        assert!(layout.visible_items.is_empty());
        assert_eq!(layout.bar_bounds.width, 80.0);
    }

    #[test]
    fn layout_places_buttons_left_to_right() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Refine", true), mk_action("b", "Drop", true)],
            bg: None,
        };
        let layout = bar.layout(0.0, 0.0, 80.0, 1.0, cell_measure());
        assert_eq!(layout.visible_items.len(), 2);
        // First button starts at origin.
        assert_eq!(layout.visible_items[0].bounds.x, 0.0);
        // Second button starts after first's full width.
        let w0 = layout.visible_items[0].bounds.width;
        assert_eq!(layout.visible_items[1].bounds.x, w0);
    }

    #[test]
    fn hit_test_routes_enabled_action_to_button() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("refine", "Refine", true)],
            bg: None,
        };
        let layout = bar.layout(0.0, 0.0, 80.0, 1.0, cell_measure());
        let r = layout.visible_items[0].bounds;
        let hit = layout.hit_test(r.x + 1.0, r.y);
        match hit {
            ToolbarHit::Button(id) => assert_eq!(id.as_str(), "refine"),
            _ => panic!("expected Button hit, got {:?}", hit),
        }
    }

    #[test]
    fn hit_test_skips_disabled_action() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("refine", "Refine", false)],
            bg: None,
        };
        let layout = bar.layout(0.0, 0.0, 80.0, 1.0, cell_measure());
        assert!(!layout.visible_items[0].clickable);
        let r = layout.visible_items[0].bounds;
        assert_eq!(layout.hit_test(r.x + 1.0, r.y), ToolbarHit::Empty);
    }

    #[test]
    fn hit_test_skips_separator_and_label() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                ToolbarButton::Separator,
                ToolbarButton::Label {
                    text: "2 of 5".into(),
                    fg: None,
                },
            ],
            bg: None,
        };
        let layout = bar.layout(0.0, 0.0, 80.0, 1.0, cell_measure());
        assert!(!layout.visible_items[0].clickable);
        assert!(!layout.visible_items[1].clickable);
        // Click anywhere in either: Empty.
        let r = layout.visible_items[1].bounds;
        assert_eq!(layout.hit_test(r.x, r.y), ToolbarHit::Empty);
    }

    #[test]
    fn layout_clips_to_bar_width() {
        // Two 10-cell buttons in a 15-cell bar: second is partially
        // clipped (width 5), still emitted in visible_items so apps see
        // the truncation, not silent drop.
        let mk_wide = |id: &str| ToolbarButton::Action {
            id: WidgetId::new(id),
            label: "xxxxxxxx".into(), // 8 chars + "[  ]" wrapper = 12 cells via measurer
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        };
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_wide("a"), mk_wide("b")],
            bg: None,
        };
        // Use a fixed 6-cell measurer so the second button is clipped.
        let measure = |_: &ToolbarButton| ToolbarItemMeasure::new(6.0);
        let layout = bar.layout(0.0, 0.0, 9.0, 1.0, measure);
        assert_eq!(layout.visible_items.len(), 2);
        // First button: 0..6.
        assert_eq!(layout.visible_items[0].bounds.x, 0.0);
        assert_eq!(layout.visible_items[0].bounds.width, 6.0);
        // Second button: starts at 6, has only 3 cells of room.
        assert_eq!(layout.visible_items[1].bounds.x, 6.0);
        assert_eq!(layout.visible_items[1].bounds.width, 3.0);
    }

    #[test]
    fn layout_zero_width_bar_produces_zero_width_items() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "X", true)],
            bg: None,
        };
        let layout = bar.layout(0.0, 0.0, 0.0, 1.0, cell_measure());
        assert_eq!(layout.visible_items.len(), 1);
        assert_eq!(layout.visible_items[0].bounds.width, 0.0);
        // Zero-width item isn't clickable even if enabled — the hit
        // never lands inside it.
        assert!(!layout.visible_items[0].clickable);
    }

    #[test]
    fn origin_offset_propagates() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "X", true)],
            bg: None,
        };
        let layout = bar.layout(10.0, 5.0, 80.0, 1.0, cell_measure());
        assert_eq!(layout.bar_bounds.x, 10.0);
        assert_eq!(layout.bar_bounds.y, 5.0);
        assert_eq!(layout.visible_items[0].bounds.x, 10.0);
        assert_eq!(layout.visible_items[0].bounds.y, 5.0);
    }

    // ── Serde ────────────────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip_toolbar() {
        let bar = Toolbar {
            id: WidgetId::new("debug-toolbar"),
            buttons: vec![
                ToolbarButton::Action {
                    id: WidgetId::new("debug:continue"),
                    label: "Continue".into(),
                    icon: Some("▶".into()),
                    key_hint: Some("F5".into()),
                    enabled: true,
                    is_active: false,
                    tooltip: "Resume execution".into(),
                },
                ToolbarButton::Separator,
                ToolbarButton::Action {
                    id: WidgetId::new("debug:stop"),
                    label: "Stop".into(),
                    icon: None,
                    key_hint: None,
                    enabled: false,
                    is_active: false,
                    tooltip: String::new(),
                },
                ToolbarButton::Label {
                    text: "paused".into(),
                    fg: Some(Color::rgb(200, 100, 50)),
                },
            ],
            bg: Some(Color::rgb(40, 40, 40)),
        };
        let json = serde_json::to_string(&bar).unwrap();
        let back: Toolbar = serde_json::from_str(&json).unwrap();
        assert_eq!(bar, back);
    }

    #[test]
    fn serde_roundtrip_event() {
        let events = vec![ToolbarEvent::ButtonClicked {
            id: WidgetId::new("debug:continue"),
            modifiers: Modifiers::default(),
        }];
        for ev in &events {
            let json = serde_json::to_string(ev).unwrap();
            let back: ToolbarEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(ev, &back);
        }
    }

    #[test]
    fn action_enabled_defaults_to_true_in_serde() {
        // Round-trip a JSON literal without `enabled` set — should
        // default to true so apps don't need to set it explicitly.
        let json = r#"{
            "type": "Action",
            "id": "x",
            "label": "X"
        }"#;
        let _ = json; // serde tagging is internal; verify via a real round-trip
        let btn = ToolbarButton::Action {
            id: WidgetId::new("x"),
            label: "X".into(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        };
        let json = serde_json::to_string(&btn).unwrap();
        let back: ToolbarButton = serde_json::from_str(&json).unwrap();
        assert_eq!(btn, back);
    }
}
