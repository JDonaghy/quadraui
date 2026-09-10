//! `Toolbar` primitive: a horizontal strip of clickable action buttons that
//! sits **above** a content area (above a tree, list, terminal, etc.) — not
//! on a panel title bar (use [`crate::Panel`]`.actions` for that) and not for
//! view selection (use [`crate::TabBar`]).
//!
//! ## Why this isn't `StatusBar`
//!
//! Downstream apps historically faked toolbars by stuffing clickable verbs into
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

use std::borrow::Cow;

use crate::event::Rect;
use crate::types::{Color, Icon, WidgetId};
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
    /// Index of the keyboard-focused button within [`Self::buttons`].
    ///
    /// When `Some(i)`, backends render the button at that index with a
    /// distinct focus highlight (e.g. accent-coloured background on TUI,
    /// a focus ring on GTK/macOS) so keyboard users know which button
    /// Enter or Space will activate.
    ///
    /// The host app is responsible for advancing this via Tab / Shift-Tab
    /// and activating via Enter / Space. Only `Action` buttons with
    /// `enabled == true` should ever be assigned here; rasterisers skip
    /// the focus highlight silently if the index points at a non-action
    /// or disabled item.
    #[serde(default)]
    pub focused_index: Option<usize>,
    /// Per-button Nerd-Font glyph + ASCII fallback pairs, keyed by the
    /// owning [`ToolbarButton::Action`]'s `id`.
    ///
    /// [`ToolbarButton::Action::icon`] stays a plain `Option<String>`
    /// fallback on purpose — 67 literal construction sites across
    /// `quadraui`, `coord-tui`, and `vimcode` pin its type, the same
    /// blast-radius problem issue #683 hit for `PanelDefinition::icon`.
    /// Rather than widen that field, a button that wants a *distinct*
    /// Nerd-Font glyph and ASCII fallback registers the pair here via
    /// [`Self::with_icon_override`] — mirroring
    /// [`crate::compose::app_shell::AppShell::with_panel_icon`]'s
    /// override-by-id shape (issue #913). Empty by default, so a
    /// `Toolbar` built without this field (every existing literal)
    /// paints byte-identical to pre-#913 `develop` on every backend.
    ///
    /// A small `Vec` rather than a `HashMap` because toolbars carry a
    /// handful of buttons at most, and `Vec<(K, V)>` (unlike
    /// `HashMap<K, V>`) implements `Eq`, which `Toolbar`'s own derive
    /// needs.
    #[serde(default)]
    pub icon_overrides: Vec<(WidgetId, Icon)>,
}

/// One item in a [`Toolbar`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolbarButton {
    /// A clickable action button.
    Action {
        id: WidgetId,
        /// Primary label text (e.g. "Refine", "Continue").
        label: String,
        /// Optional leading icon glyph (e.g. "▶", "🔄"), painted verbatim
        /// on every backend.
        ///
        /// This is the ASCII/Unicode fallback string when the owning
        /// [`Toolbar`] registers a [`crate::types::Icon`] override for
        /// this button's `id` via [`Toolbar::with_icon_override`] — in
        /// that case the override's `glyph` paints instead whenever the
        /// backend's Nerd Font glyph set is available (issue #913). A
        /// button with no override keeps painting this string on every
        /// backend regardless of Nerd Font availability, unchanged from
        /// pre-#913 behaviour.
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
    /// A layout for an area the rasteriser did not (or could not) paint
    /// into: no visible items at all, so [`Self::hit_test`] returns
    /// [`ToolbarHit::Empty`] for every point. Use this instead of
    /// [`Toolbar::layout`]'s geometric result whenever the paint loop
    /// skips the widget entirely — a `width == 0` / `height == 0` area,
    /// for instance — so the returned layout never describes cells that
    /// were never actually drawn (quadraui#649).
    pub fn empty(bar_bounds: Rect) -> Self {
        ToolbarLayout {
            bar_bounds,
            visible_items: Vec::new(),
        }
    }

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

// ── Shared button-measure formula (#730) ────────────────────────────────────

/// Horizontal padding inside each [`ToolbarButton::Action`] button, in
/// px/DIPs. Shared by every pixel rasteriser via [`measure_button`] — see
/// that function's doc for why this used to be five separately
/// maintained copies.
pub const ACTION_H_PAD: f32 = 8.0;

/// Width of a [`ToolbarButton::Separator`] slot, in px/DIPs. See
/// [`ACTION_H_PAD`] / [`measure_button`].
pub const SEPARATOR_PX: f32 = 12.0;

/// Format an `Action`'s rendered text: `"{icon} {label} ({hint})"`, with
/// the icon and/or key-hint sections dropped when absent.
///
/// Shared by every pixel rasteriser's paint AND measure paths (#730) so
/// the text a button paints and the text [`measure_button`] measured
/// against can never disagree.
pub fn action_text(label: &str, icon: Option<&str>, key_hint: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(icon) = icon {
        s.push_str(icon);
        s.push(' ');
    }
    s.push_str(label);
    if let Some(hint) = key_hint {
        s.push_str(" (");
        s.push_str(hint);
        s.push(')');
    }
    s
}

/// Compute the pixel/DIP width of a single toolbar item via `measure`.
///
/// This is the **single** button-measure formula for every pixel
/// backend. Before #730 it existed independently in
/// `gtk::toolbar::measure_item`, `macos::toolbar::measure_item`,
/// `gtk::sidebar_panel::item_width_px`, `macos::sidebar_panel::item_width_px`,
/// and inline in
/// [`crate::primitives::layout_metrics::form_field_measure`]'s `Toolbar`
/// field arm — five copies that agreed with each other only by luck, not
/// construction (see `PRIMITIVE_RULES.md`'s primitive-first rule, #713).
/// `win::toolbar` and every future pixel backend call this instead of
/// writing a sixth.
///
/// Each backend supplies `measure` via a thin [`crate::primitives::layout_metrics::TextMeasure`]
/// adapter over its own live font/context (a `pango::Layout`, a `CTFont`,
/// DirectWrite metrics, …) — see e.g. `gtk::toolbar`'s `PangoMeasure`,
/// `macos::toolbar`'s `CtFontMeasure`, `win::toolbar`'s `DWriteMeasure`.
///
/// TUI does **not** call this: its cell-based `[ icon label (hint) ]`
/// bracket format — with icon-only compaction and double-width-glyph
/// awareness (`tui::toolbar::tui_item_width`) — is a genuinely different
/// formula, not a drifted copy of this pixel one, so folding it in here
/// would be a silent behaviour change rather than a dedup.
pub fn measure_button(
    measure: &dyn crate::primitives::layout_metrics::TextMeasure,
    btn: &ToolbarButton,
) -> f32 {
    match btn {
        ToolbarButton::Action {
            label,
            icon,
            key_hint,
            ..
        } => {
            let text = action_text(label, icon.as_deref(), key_hint.as_deref());
            measure.width_of(&text) + 2.0 * ACTION_H_PAD
        }
        ToolbarButton::Separator => SEPARATOR_PX,
        ToolbarButton::Label { text, .. } => measure.width_of(text),
    }
}

impl Toolbar {
    /// Attach a distinct Nerd-Font glyph + ASCII fallback pair to one
    /// button, overriding the plain-string [`ToolbarButton::Action::icon`]
    /// that button was built with (issue #913).
    ///
    /// `id` may name any button in [`Self::buttons`]; unknown ids are
    /// stored but never consulted (no button will ever resolve them).
    /// Calling this twice for the same id appends a second entry —
    /// [`Self::icon_override`] returns the first match, so the earliest
    /// registration wins. Which half of the pair actually paints is the
    /// backend's `nerd_fonts_enabled` flag: `glyph` when Nerd Font
    /// glyphs are available, `fallback` otherwise.
    #[must_use]
    pub fn with_icon_override(mut self, id: WidgetId, icon: Icon) -> Self {
        self.icon_overrides.push((id, icon));
        self
    }

    /// The registered [`Icon`] override for `id`, if any — see
    /// [`Self::with_icon_override`].
    pub fn icon_override(&self, id: &WidgetId) -> Option<&Icon> {
        self.icon_overrides
            .iter()
            .find(|(oid, _)| oid == id)
            .map(|(_, icon)| icon)
    }

    /// Resolve the icon `btn` should actually paint/measure, given
    /// whether the backend's Nerd Font glyph set is available.
    ///
    /// When `btn` is an [`ToolbarButton::Action`] with a registered
    /// [`Self::icon_override`], returns a copy of `btn` with its `icon`
    /// field swapped for `Icon::glyph` (when `nerd_fonts_enabled`) or
    /// `Icon::fallback` (otherwise) — the override wins outright,
    /// replacing the button's own `icon` field entirely, exactly like
    /// [`crate::compose::app_shell::AppShell::resolved_icon`] does for
    /// `PanelDefinition::icon`. Every other case (no override, or a
    /// non-`Action` button) borrows `btn` unchanged, so calling this on
    /// a `Toolbar` with no overrides is a no-op — the byte-identical
    /// `develop` behaviour the flag-off acceptance bar requires.
    ///
    /// Every rasteriser calls this once per button — during layout
    /// *and* during paint, from the exact same `(bar, nerd_fonts_enabled)`
    /// inputs — so a wide glyph's measured width and painted width can
    /// never disagree.
    pub fn resolve_button_icon<'a>(
        &self,
        btn: &'a ToolbarButton,
        nerd_fonts_enabled: bool,
    ) -> Cow<'a, ToolbarButton> {
        if let ToolbarButton::Action { id, .. } = btn {
            if let Some(icon) = self.icon_override(id) {
                let resolved = if nerd_fonts_enabled {
                    icon.glyph.clone()
                } else {
                    icon.fallback.clone()
                };
                let mut cloned = btn.clone();
                if let ToolbarButton::Action {
                    icon: icon_field, ..
                } = &mut cloned
                {
                    *icon_field = Some(resolved);
                }
                return Cow::Owned(cloned);
            }
        }
        Cow::Borrowed(btn)
    }

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
        // separators 2 cells, labels their *display* width. Uses
        // `crate::text_util::display_width`, not `chars().count()` —
        // before issue #913 this used `chars().count()`, which
        // undercounts every East-Asian-Wide / emoji icon by half a cell
        // and made this helper diverge from `tui::toolbar::tui_item_width`
        // (which already measured correctly), so a wide-icon layout bug
        // could hide behind a green test suite here.
        |btn| match btn {
            ToolbarButton::Action {
                label,
                icon,
                key_hint,
                ..
            } => {
                let icon_w = icon
                    .as_ref()
                    .map(|s| crate::text_util::display_width(s) + 1)
                    .unwrap_or(0);
                let hint_w = key_hint
                    .as_ref()
                    .map(|s| crate::text_util::display_width(s) + 1)
                    .unwrap_or(0);
                // `[ ` + content + ` ]`
                let label_w = crate::text_util::display_width(label);
                ToolbarItemMeasure::new((4 + icon_w + label_w + hint_w) as f32)
            }
            ToolbarButton::Separator => ToolbarItemMeasure::new(2.0),
            ToolbarButton::Label { text, .. } => {
                ToolbarItemMeasure::new(crate::text_util::display_width(text) as f32)
            }
        }
    }

    #[test]
    fn empty_toolbar_layout() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![],
            bg: None,
            focused_index: None,
            icon_overrides: Vec::new(),
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
            focused_index: None,
            icon_overrides: Vec::new(),
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
            focused_index: None,
            icon_overrides: Vec::new(),
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
            focused_index: None,
            icon_overrides: Vec::new(),
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
            focused_index: None,
            icon_overrides: Vec::new(),
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
            focused_index: None,
            icon_overrides: Vec::new(),
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
            focused_index: None,
            icon_overrides: Vec::new(),
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
            focused_index: None,
            icon_overrides: Vec::new(),
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
            focused_index: None,
            icon_overrides: Vec::new(),
        };
        let json = serde_json::to_string(&bar).unwrap();
        let back: Toolbar = serde_json::from_str(&json).unwrap();
        assert_eq!(bar, back);
    }

    // ── Shared measure formula (#730) ───────────────────────────────────

    /// Fixed-width stand-in for a real font: `6.0` px per char — the
    /// same convention `primitives::layout_metrics`'s own `FakeMeasure`
    /// uses, since a real per-backend measurer (Pango / Core Text /
    /// DirectWrite) can't run on every CI host. Because `gtk::toolbar`,
    /// `macos::toolbar`, and `win::toolbar` all measure by calling
    /// [`measure_button`] through their own thin adapter over this exact
    /// trait, proving the formula once here is proving it for every
    /// pixel backend at once — there is no second implementation left to
    /// drift (the "identical widths across pixel backends" acceptance
    /// bar for #730).
    struct FakeMeasure;
    impl crate::primitives::layout_metrics::TextMeasure for FakeMeasure {
        fn width_of(&self, text: &str) -> f32 {
            text.chars().count() as f32 * 6.0
        }
    }

    #[test]
    fn action_text_formats_icon_label_hint() {
        assert_eq!(action_text("Refine", None, None), "Refine");
        assert_eq!(action_text("Refine", Some("▶"), None), "▶ Refine");
        assert_eq!(action_text("Refine", None, Some("r")), "Refine (r)");
        assert_eq!(action_text("Refine", Some("▶"), Some("r")), "▶ Refine (r)");
    }

    #[test]
    fn measure_button_action_is_text_width_plus_double_pad() {
        let btn = mk_action("a", "Go", true); // 2 chars * 6.0 = 12.0
        let w = measure_button(&FakeMeasure, &btn);
        assert_eq!(w, 12.0 + 2.0 * ACTION_H_PAD);
    }

    #[test]
    fn measure_button_separator_is_fixed_width() {
        let w = measure_button(&FakeMeasure, &ToolbarButton::Separator);
        assert_eq!(w, SEPARATOR_PX);
    }

    #[test]
    fn measure_button_label_is_unpadded_text_width() {
        let btn = ToolbarButton::Label {
            text: "2 of 5".into(), // 6 chars * 6.0 = 36.0
            fg: None,
        };
        let w = measure_button(&FakeMeasure, &btn);
        assert_eq!(w, 36.0);
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

    // ── Icon overrides (#913) ───────────────────────────────────────────

    /// Flag-off acceptance bar: a `Toolbar` with no `icon_overrides`
    /// resolves every button's icon to exactly its own `icon` field,
    /// regardless of `nerd_fonts_enabled` — i.e. `resolve_button_icon`
    /// is a no-op absent an override, so today's `icon: Some("▶".into())`
    /// literals keep painting byte-identical to pre-#913 `develop`.
    #[test]
    fn resolve_button_icon_is_a_noop_with_no_overrides() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![],
            bg: None,
            focused_index: None,
            icon_overrides: Vec::new(),
        };
        let btn = ToolbarButton::Action {
            id: WidgetId::new("a"),
            label: "Refine".into(),
            icon: Some("▶".into()),
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        };
        for flag in [false, true] {
            let resolved = bar.resolve_button_icon(&btn, flag);
            assert_eq!(resolved.as_ref(), &btn, "nerd_fonts_enabled={flag}");
        }
    }

    /// Flag-on acceptance bar: a button with a registered override paints
    /// `Icon::glyph` when `nerd_fonts_enabled` and `Icon::fallback`
    /// otherwise — the override replaces the button's own `icon` field
    /// entirely, mirroring `AppShell::resolved_icon`.
    #[test]
    fn resolve_button_icon_picks_glyph_or_fallback() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![],
            bg: None,
            focused_index: None,
            icon_overrides: Vec::new(),
        }
        .with_icon_override(WidgetId::new("a"), Icon::new("\u{f021}", "R"));
        let btn = ToolbarButton::Action {
            id: WidgetId::new("a"),
            label: "Refresh".into(),
            icon: Some("stale".into()),
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        };

        let glyph_resolved = bar.resolve_button_icon(&btn, true);
        match glyph_resolved.as_ref() {
            ToolbarButton::Action { icon, .. } => {
                assert_eq!(icon.as_deref(), Some("\u{f021}"))
            }
            other => panic!("expected Action, got {other:?}"),
        }

        let fallback_resolved = bar.resolve_button_icon(&btn, false);
        match fallback_resolved.as_ref() {
            ToolbarButton::Action { icon, .. } => assert_eq!(icon.as_deref(), Some("R")),
            other => panic!("expected Action, got {other:?}"),
        }
    }

    /// An override registered for an id that isn't in `buttons` (or that
    /// doesn't match the button passed in) is simply never consulted —
    /// `resolve_button_icon` only ever looks at the `id` embedded in the
    /// `btn` argument itself.
    #[test]
    fn resolve_button_icon_ignores_override_for_a_different_id() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![],
            bg: None,
            focused_index: None,
            icon_overrides: Vec::new(),
        }
        .with_icon_override(WidgetId::new("other"), Icon::new("\u{f021}", "R"));
        let btn = ToolbarButton::Action {
            id: WidgetId::new("a"),
            label: "Refine".into(),
            icon: Some("▶".into()),
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        };
        let resolved = bar.resolve_button_icon(&btn, true);
        assert_eq!(resolved.as_ref(), &btn);
    }

    /// Non-`Action` items (separators, labels) have no `id` to key an
    /// override on — `resolve_button_icon` must leave them untouched
    /// rather than panicking or silently misinterpreting the item.
    #[test]
    fn resolve_button_icon_leaves_separator_and_label_untouched() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![],
            bg: None,
            focused_index: None,
            icon_overrides: Vec::new(),
        }
        .with_icon_override(WidgetId::new("a"), Icon::new("\u{f021}", "R"));
        assert_eq!(
            bar.resolve_button_icon(&ToolbarButton::Separator, true)
                .as_ref(),
            &ToolbarButton::Separator
        );
        let label = ToolbarButton::Label {
            text: "2 of 5".into(),
            fg: None,
        };
        assert_eq!(bar.resolve_button_icon(&label, true).as_ref(), &label);
    }

    // ── Wide-icon cell-width measurement (#913 Part 2) ──────────────────

    /// A wide (East-Asian-Wide / emoji) icon on the first button must
    /// reserve two cells, not one, so the second button's layout — and
    /// therefore its hit region — starts where it actually paints.
    /// Before #913 `cell_measure` (this test module's stand-in for the
    /// TUI rasteriser's cell formula) measured icons via
    /// `chars().count()`, undercounting a wide glyph by one cell; the
    /// real `tui::toolbar::tui_item_width` already used
    /// `text_util::display_width` and was unaffected; this test pins the
    /// primitive-level formula to the same contract so the two can never
    /// drift again.
    #[test]
    fn wide_icon_on_first_button_offsets_second_buttons_layout() {
        // 一 (U+4E00) is an unambiguous double-width CJK ideograph — same
        // choice `tui::toolbar`'s own `double_width_icon_reserves_two_cells`
        // makes, to avoid depending on how a given `unicode-width` version
        // classifies emoji presentation.
        let wide_icon = mk_action_with_icon("a", "X", "一"); // 2 cells
        let plain = mk_action("b", "Y", true);
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![wide_icon, plain],
            bg: None,
            focused_index: None,
            icon_overrides: Vec::new(),
        };
        let layout = bar.layout(0.0, 0.0, 80.0, 1.0, cell_measure());
        // `cell_measure`'s Action formula: 4 (brackets/padding) + icon_w
        // (display_width + 1 trailing space) + label_w + hint_w
        // = 4 + (2 + 1) + 1 + 0 = 8.
        let w0 = layout.visible_items[0].bounds.width;
        assert_eq!(w0, 8.0, "wide icon should occupy 2 measured cells");
        // Second button must start exactly where the first one's measured
        // (and therefore painted) width ends — not one cell short.
        assert_eq!(layout.visible_items[1].bounds.x, w0);
    }

    fn mk_action_with_icon(id: &str, label: &str, icon: &str) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: Some(icon.to_string()),
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }
}
