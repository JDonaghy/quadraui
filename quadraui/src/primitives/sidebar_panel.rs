//! `SidebarPanel` primitive: a vertical container with an optional
//! [`Toolbar`] header at the top + a content region beneath that the
//! host paints into.
//!
//! Solves the "panel-with-toolbar" composition gap surfaced in #259.
//! Hosts that wanted a sidebar with a clickable action header used to
//! carve a top rect by hand, paint a `Toolbar` into it, remember to
//! always reserve the slot (so content didn't shift when the toolbar
//! came/went), and hit-test the bar manually before falling through
//! to content. That coordination — paint + hit-test + layout-reserve
//! — is the easy-to-get-wrong part this primitive owns.
//!
//! ## Why "SidebarPanel" and not "Sidebar"
//!
//! The compose layer already exports a `SidebarEvent` enum tied to
//! the multi-section [`crate::SidebarSystem`]. Adding a `Sidebar`
//! primitive with its own `SidebarEvent` would collide at the lib
//! re-export boundary. `SidebarPanel` reads correctly at the use
//! sites in claude-coordinator + vimcode (each panel IS a sidebar
//! panel) and keeps the names disjoint.
//!
//! ## Shape
//!
//! - `toolbar`:
//!   - `None`: no slot reserved. Content occupies the full rect.
//!   - `Some(bar)`: header slot reserved at `toolbar_height`. Reserved
//!     *even when `bar.buttons.is_empty()`*, so content below doesn't
//!     shift as toolbar items come and go (the actual bug coord-tui
//!     hit when toolbar appearance was state-dependent).
//! - `toolbar_height`: optional explicit height in native units. When
//!   `None`, backends pick an idiomatic default (`1.0` cells TUI,
//!   `line_height` GTK / macOS).
//!
//! ## Backend contract
//!
//! Each backend renders the toolbar into the header slot and **does
//! not paint the content region** — the content rect is returned in
//! `SidebarPanelLayout.content_bounds` for the host to paint into
//! (typically a tree, list, form, etc.). This is the same pattern
//! [`crate::Panel`] uses for its content region.
//!
//! Hit-test:
//! - Click inside the toolbar slot routes via the nested
//!   `ToolbarLayout::hit_test` to a [`SidebarPanelHit::ToolbarButton`]
//!   or, for misses inside the slot,
//!   [`SidebarPanelHit::ToolbarEmpty`].
//! - Click inside the content rect produces
//!   [`SidebarPanelHit::Content`] with content-local coordinates.
//! - Click outside both produces [`SidebarPanelHit::Empty`].

use crate::event::Rect;
use crate::primitives::toolbar::{Toolbar, ToolbarHit, ToolbarItemMeasure, ToolbarLayout};
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

// ── Data model ───────────────────────────────────────────────────────────────

/// Vertical container: optional header toolbar + content region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidebarPanel {
    pub id: WidgetId,
    /// Optional header toolbar. When `None` the slot is **not**
    /// reserved (content gets the full rect). When `Some`, the slot
    /// is always reserved at `toolbar_height` even if `buttons` is
    /// empty — so content below doesn't shift as buttons appear /
    /// disappear.
    #[serde(default)]
    pub toolbar: Option<Toolbar>,
    /// Reserved height for the toolbar slot in native units. When
    /// `None`, backends pick an idiomatic default (1 cell TUI,
    /// `line_height` GTK / macOS).
    #[serde(default)]
    pub toolbar_height: Option<f32>,
}

/// Events emitted by a [`SidebarPanel`].
///
/// Note the name: backends emit `ToolbarButtonClicked` (not just
/// `ButtonClicked`) so a host that listens to multiple event sources
/// doesn't accidentally collide with a content-area button event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SidebarPanelEvent {
    /// User clicked an enabled toolbar button in the header slot.
    ToolbarButtonClicked { id: WidgetId, modifiers: Modifiers },
    /// User clicked inside the content rect. `content_pos` is in
    /// **content-local coordinates** (relative to `content_bounds.x` /
    /// `content_bounds.y`) — hosts can pass it directly to their
    /// child widget's hit-test.
    ContentClicked {
        content_x: f32,
        content_y: f32,
        modifiers: Modifiers,
    },
}

// ── Layout + hit-testing ─────────────────────────────────────────────────────

/// Fully-resolved layout for a [`SidebarPanel`].
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarPanelLayout {
    /// Bounds of the entire panel as given to `layout()`.
    pub panel_bounds: Rect,
    /// Bounds of the toolbar slot. `None` when the panel has no
    /// toolbar (slot wasn't reserved). When `Some`, this rect is
    /// reserved even if the toolbar has no buttons.
    pub toolbar_bounds: Option<Rect>,
    /// Bounds of the content area. Always populated — equals the
    /// panel minus the toolbar slot.
    pub content_bounds: Rect,
    /// Resolved inner toolbar layout. `None` mirrors `toolbar_bounds`.
    pub toolbar_layout: Option<ToolbarLayout>,
}

/// Classification of a hit-test result on a [`SidebarPanel`].
#[derive(Debug, Clone, PartialEq)]
pub enum SidebarPanelHit {
    /// Click landed on a clickable toolbar button — carries its id.
    ToolbarButton(WidgetId),
    /// Click landed inside the toolbar slot but not on a clickable
    /// button (gap, separator, label, or disabled action).
    ToolbarEmpty,
    /// Click landed inside the content area. `(x, y)` are
    /// **content-local** — relative to `content_bounds.x` / `.y`.
    Content { x: f32, y: f32 },
    /// Click missed both the toolbar slot and the content area
    /// (outside the panel entirely).
    Empty,
}

impl SidebarPanelLayout {
    /// Test point `(x, y)` against the layout. Toolbar slot wins over
    /// the content area when bounds overlap (which they shouldn't —
    /// `layout()` carves disjoint rects — but the precedence is
    /// documented in case future relaxations of that invariant break
    /// it).
    pub fn hit_test(&self, x: f32, y: f32) -> SidebarPanelHit {
        if let (Some(tb), Some(tlayout)) = (self.toolbar_bounds, &self.toolbar_layout) {
            if contains(tb, x, y) {
                return match tlayout.hit_test(x, y) {
                    ToolbarHit::Button(id) => SidebarPanelHit::ToolbarButton(id),
                    ToolbarHit::Empty => SidebarPanelHit::ToolbarEmpty,
                };
            }
        }
        if contains(self.content_bounds, x, y) {
            return SidebarPanelHit::Content {
                x: x - self.content_bounds.x,
                y: y - self.content_bounds.y,
            };
        }
        SidebarPanelHit::Empty
    }
}

fn contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

// ── Measurement ──────────────────────────────────────────────────────────────

/// Caller-supplied measurements for computing a
/// [`SidebarPanelLayout`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarPanelMeasure {
    /// Default header height when `SidebarPanel.toolbar_height` is
    /// `None`. Use `1.0` for TUI cells, `line_height` for native.
    pub default_toolbar_height: f32,
    /// Per-toolbar-item width in native units — passed through to the
    /// nested `Toolbar::layout`. Backends supply the same measurer
    /// they would use for a standalone toolbar in the same rect.
    pub item_width: f32,
}

impl SidebarPanelMeasure {
    pub fn new(default_toolbar_height: f32, item_width: f32) -> Self {
        Self {
            default_toolbar_height,
            item_width,
        }
    }
}

impl SidebarPanel {
    /// Compute the full layout for this panel.
    ///
    /// `measure_item` is the per-toolbar-item width measurer. It
    /// receives each `ToolbarButton` and returns its width in the
    /// caller's native unit — TUI passes a cell-count measurer, GTK
    /// passes a Pango pixel measurer. When the panel has no toolbar
    /// the closure is never invoked.
    pub fn layout<F>(
        &self,
        bounds: Rect,
        measure: SidebarPanelMeasure,
        measure_item: F,
    ) -> SidebarPanelLayout
    where
        F: Fn(&crate::primitives::toolbar::ToolbarButton) -> ToolbarItemMeasure,
    {
        let toolbar_height = self
            .toolbar_height
            .unwrap_or(measure.default_toolbar_height);

        match &self.toolbar {
            Some(bar) => {
                // Reserve the slot even when `bar.buttons.is_empty()`
                // so content doesn't shift as toolbar items come and
                // go (the bug coord-tui hit twice).
                let slot_height = toolbar_height.min(bounds.height).max(0.0);
                let toolbar_bounds = Rect::new(bounds.x, bounds.y, bounds.width, slot_height);
                let content_y = bounds.y + slot_height;
                let content_h = (bounds.height - slot_height).max(0.0);
                let content_bounds = Rect::new(bounds.x, content_y, bounds.width, content_h);

                let toolbar_layout = bar.layout(
                    toolbar_bounds.x,
                    toolbar_bounds.y,
                    toolbar_bounds.width,
                    toolbar_bounds.height,
                    measure_item,
                );

                SidebarPanelLayout {
                    panel_bounds: bounds,
                    toolbar_bounds: Some(toolbar_bounds),
                    content_bounds,
                    toolbar_layout: Some(toolbar_layout),
                }
            }
            None => SidebarPanelLayout {
                panel_bounds: bounds,
                toolbar_bounds: None,
                content_bounds: bounds,
                toolbar_layout: None,
            },
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::ToolbarButton;
    use crate::types::WidgetId;

    fn mk_action(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn panel_with_toolbar() -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![mk_action("a", "Refine"), mk_action("b", "Drop")],
                bg: None,
            }),
            toolbar_height: None,
        }
    }

    fn measure() -> (
        SidebarPanelMeasure,
        impl Fn(&ToolbarButton) -> ToolbarItemMeasure,
    ) {
        let m = SidebarPanelMeasure::new(1.0, 8.0);
        (m, |_btn: &ToolbarButton| ToolbarItemMeasure::new(8.0))
    }

    #[test]
    fn no_toolbar_gives_full_rect_to_content() {
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: None,
            toolbar_height: None,
        };
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        assert!(layout.toolbar_bounds.is_none());
        assert!(layout.toolbar_layout.is_none());
        assert_eq!(layout.content_bounds, Rect::new(0.0, 0.0, 30.0, 10.0));
    }

    #[test]
    fn toolbar_reserves_top_slot() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        let tb = layout.toolbar_bounds.expect("toolbar slot reserved");
        assert_eq!(tb.x, 0.0);
        assert_eq!(tb.y, 0.0);
        assert_eq!(tb.width, 30.0);
        assert_eq!(tb.height, 1.0); // default_toolbar_height
                                    // Content starts below the slot.
        assert_eq!(layout.content_bounds.y, 1.0);
        assert_eq!(layout.content_bounds.height, 9.0);
    }

    #[test]
    fn empty_toolbar_still_reserves_slot() {
        // The whole point of Gap 2 is that content doesn't shift when
        // the toolbar's buttons vector is temporarily empty.
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![], // empty!
                bg: None,
            }),
            toolbar_height: None,
        };
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        assert!(layout.toolbar_bounds.is_some());
        // Slot still reserved — content offset by the toolbar height.
        assert_eq!(layout.content_bounds.y, 1.0);
        assert_eq!(layout.content_bounds.height, 9.0);
    }

    #[test]
    fn explicit_toolbar_height_overrides_default() {
        let mut panel = panel_with_toolbar();
        panel.toolbar_height = Some(3.0);
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        let tb = layout.toolbar_bounds.unwrap();
        assert_eq!(tb.height, 3.0);
        assert_eq!(layout.content_bounds.y, 3.0);
        assert_eq!(layout.content_bounds.height, 7.0);
    }

    #[test]
    fn hit_test_routes_toolbar_button_click() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        // First button is at x=0..8 in the toolbar slot.
        match layout.hit_test(2.0, 0.0) {
            SidebarPanelHit::ToolbarButton(id) => assert_eq!(id.as_str(), "a"),
            other => panic!("expected ToolbarButton hit, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_inside_toolbar_slot_off_button_is_toolbar_empty() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        // Past the second button's right edge (16) but still in the
        // toolbar slot (y < 1).
        let hit = layout.hit_test(20.0, 0.0);
        assert_eq!(hit, SidebarPanelHit::ToolbarEmpty);
    }

    #[test]
    fn hit_test_content_returns_content_local_coords() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(10.0, 5.0, 30.0, 10.0), m, mi);
        // Click at panel-absolute (12.0, 8.0): toolbar slot is rows
        // 5..6, so y=8 lands in content. Content origin is (10, 6).
        // Local: (12-10, 8-6) = (2.0, 2.0).
        match layout.hit_test(12.0, 8.0) {
            SidebarPanelHit::Content { x, y } => {
                assert_eq!(x, 2.0);
                assert_eq!(y, 2.0);
            }
            other => panic!("expected Content hit, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_outside_panel_is_empty() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        assert_eq!(layout.hit_test(100.0, 100.0), SidebarPanelHit::Empty);
    }

    #[test]
    fn toolbar_height_clamps_to_panel_height() {
        // If a host accidentally asks for a toolbar taller than the
        // panel, the slot must not overflow into negative-content
        // territory.
        let mut panel = panel_with_toolbar();
        panel.toolbar_height = Some(50.0);
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        let tb = layout.toolbar_bounds.unwrap();
        assert_eq!(tb.height, 10.0); // clamped to panel height
        assert_eq!(layout.content_bounds.height, 0.0);
    }

    #[test]
    fn serde_roundtrip() {
        let panel = panel_with_toolbar();
        let json = serde_json::to_string(&panel).unwrap();
        let back: SidebarPanel = serde_json::from_str(&json).unwrap();
        assert_eq!(panel, back);
    }

    #[test]
    fn serde_event_roundtrip() {
        let events = vec![
            SidebarPanelEvent::ToolbarButtonClicked {
                id: WidgetId::new("x"),
                modifiers: Modifiers::default(),
            },
            SidebarPanelEvent::ContentClicked {
                content_x: 1.5,
                content_y: 2.5,
                modifiers: Modifiers::default(),
            },
        ];
        for ev in &events {
            let json = serde_json::to_string(ev).unwrap();
            let back: SidebarPanelEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(ev, &back);
        }
    }
}
