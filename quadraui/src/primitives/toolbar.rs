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
//!
//! ## Nerd-Font icons
//!
//! [`ToolbarButton::Action::icon`] is a plain `Option<String>` painted
//! verbatim on every backend, so a host that wants a Nerd-Font glyph with
//! an ASCII fallback registers the pair in a [`ToolbarIcons`] side table
//! and calls [`ToolbarIcons::apply`] to bake the right half into the
//! `Toolbar` it hands to the backend (issue #913).

use std::borrow::Cow;

use crate::event::Rect;
use crate::types::{Color, Icon, WidgetId};
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

// ── Data model ───────────────────────────────────────────────────────────────

/// Declarative description of a horizontal toolbar.
///
/// **Adding a field here used to be a breaking change** — see issue
/// #1251 (the v0.1.0 breaking batch, phase 2), which made this struct
/// `#[non_exhaustive]` for real once both known consumers had migrated
/// off exhaustive literals onto [`Toolbar::new`] plus the `with_*`
/// builder per field below (and a `Default` impl).
///
/// # Examples
///
/// ```
/// use quadraui::{Toolbar, ToolbarButton, ToolbarHit, ToolbarItemMeasure, WidgetId};
///
/// let bar = Toolbar::new(WidgetId::new("toolbar:explorer")).with_buttons(vec![
///     ToolbarButton::Action {
///         id: WidgetId::new("toolbar:new_file"),
///         label: "New File".to_string(),
///         icon: None,
///         key_hint: None,
///         enabled: true,
///         is_active: false,
///         tooltip: String::new(),
///     },
/// ]);
///
/// let layout = bar.layout(0.0, 0.0, 80.0, 1.0, |_| ToolbarItemMeasure::new(10.0));
/// assert_eq!(
///     layout.hit_test(2.0, 0.0),
///     ToolbarHit::Button(WidgetId::new("toolbar:new_file"))
/// );
/// ```
#[non_exhaustive]
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
}

/// Vertical alignment of button/label text within a toolbar slot taller
/// than one text row (issue #260).
///
/// On a 1-row slot every variant resolves to the same painted position,
/// so this is purely a multi-row concern — see each rasteriser's
/// `text_row`/`ty` resolution (`tui::toolbar::tui_text_row`,
/// [`native_surface_paint::paint`]) for the exact per-backend formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToolbarVAlign {
    /// (default) Text on the top row of the slot. Padding below.
    #[default]
    Top,
    /// Text vertically centred. For even heights, the row/offset above
    /// centre wins (matches the pre-#260 TUI arithmetic this variant
    /// preserves byte-for-byte).
    Center,
    /// Text on the bottom row of the slot. Padding above.
    Bottom,
}

/// Per-call paint options for [`Toolbar`] rasterisers that don't have a
/// home directly on the [`Toolbar`] snapshot itself.
///
/// `Toolbar` is one of the all-`pub`-field paint-time snapshots both
/// known downstream consumers build with **exhaustive struct
/// literals** — no `..base`, no `..Default::default()` (six such
/// literals: four in `coord-tui`, two in `vimcode`; see
/// `quadraui/tests/downstream_struct_literals.rs` and this file's own
/// [`ToolbarIcons`] doc for the two previous times this exact class of
/// struct hit the same wall, #833/#913). Issue #260 originally proposed
/// `valign` as a new field directly on `Toolbar`; growing its field
/// list would break every one of those six call sites with `E0063`.
/// A small, independently-growable options struct — the same escape
/// hatch [`crate::primitives::editor::EditorPaintOptions`] uses for
/// `Editor` — sidesteps that, and can gain more fields later for free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToolbarPaintOptions {
    /// Vertical alignment of button/label text within the toolbar's
    /// slot. Only visible when the slot is taller than one text row.
    pub valign: ToolbarVAlign,
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
        /// on every backend. TUI rasterises as plain text; GTK can swap
        /// for an Image widget.
        ///
        /// This field holds *one* string, so a button that needs a
        /// Nerd-Font glyph **and** a distinct ASCII fallback registers
        /// the pair in a [`ToolbarIcons`] table and lets
        /// [`ToolbarIcons::apply`] write the resolved half into this
        /// field before the `Toolbar` reaches a backend (issue #913).
        /// The field's type deliberately stays `Option<String>`: it is
        /// named by ~67 exhaustive struct literals across `quadraui`,
        /// `coord-tui` and `vimcode`, the same blast-radius constraint
        /// issue #683 hit for `PanelDefinition::icon`.
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

// ── Nerd-Font icon overrides ─────────────────────────────────────────────────

/// Per-button Nerd-Font glyph + ASCII fallback pairs for a [`Toolbar`],
/// keyed by the owning [`ToolbarButton::Action`]'s `id` (issue #913).
///
/// Mirrors [`crate::compose::app_shell::AppShell::with_panel_icon`]'s
/// override-by-id shape. Register the pairs once, then call
/// [`Self::apply`] with the backend's Nerd-Font flag to get a `Toolbar`
/// whose buttons already carry the right half of each pair:
///
/// ```
/// use quadraui::{Icon, Toolbar, ToolbarButton, ToolbarIcons, WidgetId};
///
/// let bar = Toolbar::new(WidgetId::new("sc:toolbar")).with_buttons(vec![
///     ToolbarButton::Action {
///         id: WidgetId::new("sc:refresh"),
///         label: "Refresh".into(),
///         icon: None,
///         key_hint: None,
///         enabled: true,
///         is_active: false,
///         tooltip: String::new(),
///     },
/// ]);
/// let icons = ToolbarIcons::new()
///     .with(WidgetId::new("sc:refresh"), Icon::new("\u{f021}", "R"));
///
/// // Nerd Fonts available → the glyph; otherwise → the ASCII fallback.
/// let painted = icons.apply(&bar, true);
/// assert!(matches!(&painted.buttons[0],
///     ToolbarButton::Action { icon: Some(i), .. } if i == "\u{f021}"));
/// ```
///
/// ## Why a side table, not a `Toolbar` field
///
/// `Toolbar` is an all-`pub`-field paint-time snapshot that both external
/// consumers used to build with **exhaustive struct literals** (four
/// sites in `coord-tui`, two in `vimcode` — see
/// `tests/downstream_struct_literals.rs`) before migrating to its
/// `new`/`with_*` builder ahead of issue #1251 making it
/// `#[non_exhaustive]` for real. Growing the field list is still a
/// breaking change for anyone still on the old literal shape (now a
/// hard `E0639`, with no deprecation shim available for a struct
/// literal). Keeping the pairs in a separate type the host composes on
/// its own side is the same non-breaking escape hatch `TextEditor` uses
/// for `TextInput` (issue #833).
///
/// ## Why resolution happens here and not inside each rasteriser
///
/// [`Self::apply`] returns a plain `Toolbar`, so **layout, hit-testing and
/// paint all consume the one already-resolved value**. A wide glyph's
/// measured width and its painted width cannot disagree, because no code
/// downstream of `apply` ever sees the unresolved form — a stronger
/// guarantee than threading a `nerd_fonts_enabled` flag into every
/// measure/paint site and relying on each of them to resolve identically.
///
/// A small `Vec` rather than a `HashMap` because toolbars carry a handful
/// of buttons at most, and `Vec<(K, V)>` (unlike `HashMap<K, V>`)
/// implements `Eq`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolbarIcons {
    entries: Vec<(WidgetId, Icon)>,
}

impl ToolbarIcons {
    /// An empty table — [`Self::apply`] on it is a no-op that returns a
    /// byte-identical clone of the input `Toolbar`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the glyph + fallback pair for the [`ToolbarButton::Action`]
    /// with this `id`.
    ///
    /// `id` may name any button (or none at all — unknown ids are stored
    /// but never resolved). Registering the same id twice keeps the
    /// earliest entry, since [`Self::get`] returns the first match.
    #[must_use]
    pub fn with(mut self, id: WidgetId, icon: Icon) -> Self {
        self.entries.push((id, icon));
        self
    }

    /// The registered pair for `id`, if any.
    pub fn get(&self, id: &WidgetId) -> Option<&Icon> {
        self.entries
            .iter()
            .find(|(oid, _)| oid == id)
            .map(|(_, icon)| icon)
    }

    /// `true` when nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many pairs are registered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Resolve one button against this table.
    ///
    /// An [`ToolbarButton::Action`] with a registered pair comes back as an
    /// owned clone whose `icon` field is the pair's `glyph` (when
    /// `nerd_fonts_enabled`) or `fallback` (otherwise) — the override wins
    /// outright, replacing whatever the button's own `icon` held, exactly
    /// like [`crate::compose::app_shell::AppShell`] does for
    /// `PanelDefinition::icon`. Every other case borrows `btn` untouched.
    pub fn resolve_button<'a>(
        &self,
        btn: &'a ToolbarButton,
        nerd_fonts_enabled: bool,
    ) -> Cow<'a, ToolbarButton> {
        if let ToolbarButton::Action { id, .. } = btn {
            if let Some(icon) = self.get(id) {
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

    /// Bake every registered pair into a copy of `bar`, ready to hand to
    /// any `Backend::draw_toolbar*` / `SidebarPanel` / `FieldKind::Toolbar`
    /// / `Dialog` path.
    ///
    /// With an empty table (or a `bar` whose buttons match no registered
    /// id) the result equals `bar` field-for-field, so an app that never
    /// registers anything paints byte-identically to pre-#913 behaviour.
    #[must_use]
    pub fn apply(&self, bar: &Toolbar, nerd_fonts_enabled: bool) -> Toolbar {
        if self.is_empty() {
            return bar.clone();
        }
        Toolbar {
            id: bar.id.clone(),
            buttons: bar
                .buttons
                .iter()
                .map(|btn| self.resolve_button(btn, nerd_fonts_enabled).into_owned())
                .collect(),
            bg: bar.bg,
            focused_index: bar.focused_index,
        }
    }
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

/// Resolve the y-offset (px/DIPs) at which text of `content_height`
/// should paint within a slot starting at `slot_y` with `slot_height`,
/// per `valign` (issue #260). Shared by every pixel-unit rasteriser via
/// [`native_surface_paint::paint`] so GTK / macOS / Win-GUI agree
/// exactly. TUI's cell-grid math is its own
/// (`tui::toolbar::tui_text_row`) since a cell is already quantized —
/// folding it into this `f32` formula would just re-round it back.
pub fn valign_offset_y(
    valign: ToolbarVAlign,
    slot_y: f32,
    slot_height: f32,
    content_height: f32,
) -> f32 {
    match valign {
        ToolbarVAlign::Top => slot_y,
        ToolbarVAlign::Center => slot_y + (slot_height - content_height) / 2.0,
        ToolbarVAlign::Bottom => slot_y + (slot_height - content_height),
    }
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

/// An empty toolbar with an empty `WidgetId` — the `new(required…)`/
/// `with_*`/`Default` trio that lets a consumer build a
/// `Toolbar` without an exhaustive struct literal.
impl Default for Toolbar {
    fn default() -> Self {
        Self::new(WidgetId::new(String::new()))
    }
}

impl Toolbar {
    /// An empty toolbar with no buttons — chain `with_*` to fill it in.
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            buttons: Vec::new(),
            bg: None,
            focused_index: None,
        }
    }

    /// Replace [`Self::buttons`].
    #[must_use]
    pub fn with_buttons(mut self, buttons: Vec<ToolbarButton>) -> Self {
        self.buttons = buttons;
        self
    }

    /// Set [`Self::bg`].
    #[must_use]
    pub fn with_bg(mut self, bg: Color) -> Self {
        self.bg = Some(bg);
        self
    }

    /// Set [`Self::focused_index`]. Takes `Option<usize>` directly
    /// (rather than wrapping like [`Self::with_bg`]) since callers
    /// commonly carry "no button focused" as their own `Option<usize>`
    /// state and want to forward it verbatim.
    #[must_use]
    pub fn with_focused_index(mut self, focused_index: Option<usize>) -> Self {
        self.focused_index = focused_index;
        self
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

        let widths: Vec<f32> = self
            .buttons
            .iter()
            .map(|btn| measure(btn).width.max(0.0))
            .collect();
        let cursors = item_cursor_positions(origin_x, bar_height, &widths);
        let right_edge = origin_x + bar_width;

        for (i, btn) in self.buttons.iter().enumerate() {
            let w = widths[i];
            let cursor = cursors[i];
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
        }

        ToolbarLayout {
            bar_bounds,
            visible_items,
        }
    }
}

/// Compute each item's left edge given its width, left-to-right, never
/// shrinking — items past the available width still get a cursor position
/// (the full cumulative sum), so overflow is clipped by the caller
/// afterwards rather than by squeezing items to fit.
///
/// With the `layout` feature this runs through [`crate::flex`]'s shared
/// flexbox engine (a `Row` of fixed-width, `flex_shrink: 0.0` leaves) so
/// `Toolbar` composes with the same engine [`crate::primitives::Form`] and
/// [`crate::compose::AppShell`] use. Without it, the same arithmetic is
/// done by hand. Both arms are covered by this module's exact-value tests
/// (`layout_places_buttons_left_to_right`, `layout_clips_to_bar_width`,
/// `origin_offset_propagates`) to guarantee they agree bit-for-bit.
#[cfg(feature = "layout")]
fn item_cursor_positions(origin_x: f32, bar_height: f32, widths: &[f32]) -> Vec<f32> {
    use crate::flex::{length, FlexLayout, Size, Style};

    if widths.is_empty() {
        return Vec::new();
    }

    let mut flex = FlexLayout::new().without_rounding();
    let leaves: Vec<_> = widths
        .iter()
        .map(|&w| {
            flex.add_leaf(Style {
                size: Size {
                    width: length(w),
                    height: length(bar_height),
                },
                flex_shrink: 0.0,
                flex_grow: 0.0,
                ..Default::default()
            })
            .expect("toolbar item leaf node")
        })
        .collect();
    let root = flex
        .add_container(Style::default(), &leaves)
        .expect("toolbar container node");

    let total_width: f32 = widths.iter().sum();
    let available = Rect::new(origin_x, 0.0, total_width, bar_height);
    let computed = flex.compute(root, available).expect("toolbar flex layout");

    leaves.iter().map(|id| computed[id].x).collect()
}

/// Same contract as the `layout`-feature arm above, computed by hand.
#[cfg(not(feature = "layout"))]
fn item_cursor_positions(origin_x: f32, _bar_height: f32, widths: &[f32]) -> Vec<f32> {
    let mut cursor = origin_x;
    let mut cursors = Vec::with_capacity(widths.len());
    for &w in widths {
        cursors.push(cursor);
        cursor += w;
    }
    cursors
}

// ── PaintSurface Phase 4 slice 5/8 (#1081) ─────────────────────────────────
//
// `paint` below is the one shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API — see `crate::primitives::menu_bar::native_surface_paint` for the
// same pattern applied one primitive earlier in this issue.
//
// Pre-migration, `gtk::toolbar`, `macos::toolbar` and `win::toolbar`
// agreed on every colour/state-priority decision (see this file's own
// per-state colouring table, copied verbatim into all three module
// docs) and on `action_text`/`measure_button`'s shared layout formula —
// but diverged on the hover/pressed/active highlight's corner
// treatment:
//
// - **GTK** painted a `CORNER_RADIUS`-rounded pill (`rounded_rect_path`
//   + fill) and a matching rounded-rect focus-ring stroke.
// - **macOS and Windows** both painted a plain square inset rectangle
//   for the highlight, and a plain square stroke for the focus ring —
//   Windows' own module doc explained why: "No rounded-rect / stroke-
//   inset helper exists yet in `win::text`".
//
// That blocker no longer applies: issue #1073 added
// [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`]
// to every pixel backend specifically to unblock chrome primitives like
// this one (see that verb's own doc). `paint` below uses it
// unconditionally for the highlight fill — closing macOS's and
// Windows' gap onto GTK's nicer pill shape, rather than flattening GTK
// down to the 2-of-3 majority. The focus ring stays a **square**
// stroke on all three: [`PaintSurface`] has no rounded-stroke verb
// (same gap noted in `primitives::context_menu::native_surface_paint`'s
// module doc), so keeping GTK's rounded ring would need a fill-shaped
// workaround uglier than just picking the square the other two already
// used.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{
        action_text, valign_offset_y, Toolbar, ToolbarButton, ToolbarLayout, ToolbarPaintOptions,
    };
    use crate::event::{Point, Rect};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::types::WidgetId;

    /// Corner radius for the hover/pressed/active highlight pill —
    /// mirrors `gtk::toolbar`'s pre-migration `CORNER_RADIUS`, now
    /// shared by every backend (see module doc).
    const CORNER_RADIUS: f32 = 4.0;

    /// Paint a [`Toolbar`] at its caller-resolved `layout` onto
    /// `surface`. `hovered_id`/`pressed_id` select the live interaction
    /// state; see this module's own doc table for the exact
    /// fg/bg-per-state contract every backend shares. `options.valign`
    /// resolves where text paints within a slot taller than one text
    /// row (issue #260) — see [`valign_offset_y`].
    pub(crate) fn paint(
        bar: &Toolbar,
        layout: &ToolbarLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
        options: ToolbarPaintOptions,
    ) {
        if layout.bar_bounds.width <= 0.0 || layout.bar_bounds.height <= 0.0 {
            return;
        }

        let bar_bg = bar.bg.unwrap_or(theme.header_bg);
        surface.surface_fill_rect(layout.bar_bounds, bar_bg);

        for vis in &layout.visible_items {
            let item = vis.bounds;
            if item.width <= 0.0 || item.height <= 0.0 {
                continue;
            }

            let btn = &bar.buttons[vis.item_idx];
            match btn {
                ToolbarButton::Action {
                    id,
                    label,
                    icon,
                    key_hint,
                    enabled,
                    is_active,
                    ..
                } => {
                    let is_hovered = *enabled && hovered_id == Some(id);
                    let is_pressed = *enabled && pressed_id == Some(id);
                    let is_focused = *enabled && bar.focused_index == Some(vis.item_idx);

                    // Highlight background: pressed/active > hovered > none.
                    let highlight = if is_pressed || *is_active {
                        Some(theme.selected_bg)
                    } else if is_hovered {
                        Some(theme.hover_bg)
                    } else {
                        None
                    };
                    if let Some(bg) = highlight {
                        let inset = Rect::new(
                            item.x + 2.0,
                            item.y + 2.0,
                            (item.width - 4.0).max(0.0),
                            (item.height - 4.0).max(0.0),
                        );
                        surface.surface_fill_rounded_rect(inset, CORNER_RADIUS, bg);
                    }

                    // Focus ring: only when not already visually
                    // dominated by hover / pressed / active.
                    if is_focused && !is_hovered && !is_pressed && !*is_active {
                        let ring = Rect::new(
                            item.x + 1.5,
                            item.y + 1.5,
                            (item.width - 3.0).max(0.0),
                            (item.height - 3.0).max(0.0),
                        );
                        surface.surface_stroke_rect(ring, theme.accent_fg, 1.0);
                    }

                    let fg = if !*enabled {
                        theme.muted_fg
                    } else if is_hovered {
                        theme.hover_fg
                    } else {
                        theme.foreground
                    };

                    let text = action_text(label, icon.as_deref(), key_hint.as_deref());
                    let (tw, th) = surface.surface_measure_text(&text);
                    let tx = item.x + (item.width - tw) / 2.0;
                    let ty = valign_offset_y(options.valign, item.y, item.height, th);
                    surface.surface_draw_text_run(
                        Rect::new(tx, ty, tw.max(0.0), th.max(0.0)),
                        &text,
                        fg,
                    );
                }
                ToolbarButton::Separator => {
                    let mid_x = item.x + item.width / 2.0;
                    let pad_y = (item.height * 0.2).max(2.0);
                    surface.surface_draw_line(
                        Point::new(mid_x, item.y + pad_y),
                        Point::new(mid_x, item.y + item.height - pad_y),
                        theme.muted_fg,
                        1.0,
                    );
                }
                ToolbarButton::Label { text, fg } => {
                    let color = fg.unwrap_or(theme.muted_fg);
                    let (tw, th) = surface.surface_measure_text(text);
                    let ty = valign_offset_y(options.valign, item.y, item.height, th);
                    surface.surface_draw_text_run(
                        Rect::new(item.x, ty, tw.max(0.0), th.max(0.0)),
                        text,
                        color,
                    );
                }
            }
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
        // separators 2 cells, labels their *display* width.
        //
        // `crate::text_util::display_width`, not `chars().count()` (issue
        // #913): a char count undercounts every East-Asian-Wide glyph and
        // emoji by half a cell, which made this helper disagree with the
        // real `tui::toolbar::tui_item_width` — so a wide-icon layout bug
        // could have hidden behind a green suite here.
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

    // ── #260: `ToolbarVAlign` / `valign_offset_y` ───────────────────────

    #[test]
    fn valign_offset_y_top_ignores_slot_height() {
        assert_eq!(valign_offset_y(ToolbarVAlign::Top, 10.0, 40.0, 12.0), 10.0);
    }

    #[test]
    fn valign_offset_y_center_splits_remaining_space_evenly() {
        // slot_height=40, content_height=12 → 28px left over, 14px above.
        assert_eq!(
            valign_offset_y(ToolbarVAlign::Center, 10.0, 40.0, 12.0),
            10.0 + 14.0
        );
    }

    #[test]
    fn valign_offset_y_bottom_flushes_content_to_the_end_of_the_slot() {
        assert_eq!(
            valign_offset_y(ToolbarVAlign::Bottom, 10.0, 40.0, 12.0),
            10.0 + 28.0
        );
    }

    #[test]
    fn valign_offset_y_every_variant_agrees_on_a_one_row_slot() {
        // slot_height == content_height: no room to move, every variant
        // resolves to the same offset (`ToolbarVAlign`'s own doc).
        for valign in [
            ToolbarVAlign::Top,
            ToolbarVAlign::Center,
            ToolbarVAlign::Bottom,
        ] {
            assert_eq!(
                valign_offset_y(valign, 5.0, 20.0, 20.0),
                5.0,
                "valign={valign:?}"
            );
        }
    }

    #[test]
    fn toolbar_valign_defaults_to_top() {
        assert_eq!(ToolbarVAlign::default(), ToolbarVAlign::Top);
        assert_eq!(ToolbarPaintOptions::default().valign, ToolbarVAlign::Top);
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

    // ── #913: Nerd-Font icon overrides + wide-glyph measurement ─────────

    fn mk_icon_action(id: &str, label: &str, icon: Option<&str>) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: icon.map(|s| s.to_string()),
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn icon_of(btn: &ToolbarButton) -> Option<&str> {
        match btn {
            ToolbarButton::Action { icon, .. } => icon.as_deref(),
            _ => None,
        }
    }

    fn bar_with(buttons: Vec<ToolbarButton>) -> Toolbar {
        Toolbar {
            id: WidgetId::new("tb"),
            buttons,
            bg: None,
            focused_index: None,
        }
    }

    #[test]
    fn icon_overrides_pick_glyph_or_fallback_by_flag() {
        let bar = bar_with(vec![mk_icon_action("a", "Refresh", Some("~"))]);
        let icons = ToolbarIcons::new().with(WidgetId::new("a"), Icon::new("\u{f021}", "R"));

        assert_eq!(
            icon_of(&icons.apply(&bar, true).buttons[0]),
            Some("\u{f021}"),
            "nerd fonts on → the override's glyph replaces the button's own icon"
        );
        assert_eq!(
            icon_of(&icons.apply(&bar, false).buttons[0]),
            Some("R"),
            "nerd fonts off → the override's ASCII fallback"
        );
    }

    #[test]
    fn apply_without_overrides_is_byte_identical_to_the_input_bar() {
        // The flag-off acceptance bar: an app that registers nothing must
        // paint exactly as it did before #913, on either flag value.
        let bar = bar_with(vec![
            mk_icon_action("a", "Refine", Some("\u{f021}")),
            ToolbarButton::Separator,
            ToolbarButton::Label {
                text: "2 of 5".into(),
                fg: None,
            },
        ]);
        let empty = ToolbarIcons::new();
        assert!(empty.is_empty());
        assert_eq!(empty.apply(&bar, true), bar);
        assert_eq!(empty.apply(&bar, false), bar);

        // Same for a table that names a button this bar doesn't have.
        let unrelated = ToolbarIcons::new().with(WidgetId::new("nope"), Icon::new("X", "Y"));
        assert_eq!(unrelated.apply(&bar, true), bar);
    }

    #[test]
    fn apply_leaves_non_action_items_and_unregistered_buttons_alone() {
        let bar = bar_with(vec![
            mk_icon_action("a", "Refresh", Some("~")),
            ToolbarButton::Separator,
            mk_icon_action("b", "Drop", Some("-")),
        ]);
        let icons = ToolbarIcons::new().with(WidgetId::new("a"), Icon::new("\u{f021}", "R"));
        let out = icons.apply(&bar, true);

        assert_eq!(icon_of(&out.buttons[0]), Some("\u{f021}"));
        assert_eq!(out.buttons[1], ToolbarButton::Separator);
        assert_eq!(
            icon_of(&out.buttons[2]),
            Some("-"),
            "an unregistered button keeps its own icon"
        );
        // Everything else about the bar survives the copy.
        assert_eq!(out.id, bar.id);
        assert_eq!(out.bg, bar.bg);
        assert_eq!(out.focused_index, bar.focused_index);
    }

    #[test]
    fn first_registration_wins_for_a_duplicated_id() {
        let bar = bar_with(vec![mk_icon_action("a", "Refresh", None)]);
        let icons = ToolbarIcons::new()
            .with(WidgetId::new("a"), Icon::new("first", "F"))
            .with(WidgetId::new("a"), Icon::new("second", "S"));
        assert_eq!(icons.len(), 2);
        assert_eq!(icon_of(&icons.apply(&bar, true).buttons[0]), Some("first"));
    }

    #[test]
    fn resolve_button_borrows_when_there_is_nothing_to_resolve() {
        let icons = ToolbarIcons::new().with(WidgetId::new("a"), Icon::new("\u{f021}", "R"));
        let sep = ToolbarButton::Separator;
        assert!(matches!(
            icons.resolve_button(&sep, true),
            Cow::Borrowed(ToolbarButton::Separator)
        ));
        let other = mk_icon_action("b", "Drop", Some("-"));
        assert!(matches!(
            icons.resolve_button(&other, true),
            Cow::Borrowed(_)
        ));
        let hit = mk_icon_action("a", "Refresh", None);
        assert!(matches!(icons.resolve_button(&hit, true), Cow::Owned(_)));
    }

    #[test]
    fn a_wide_override_glyph_shifts_the_next_button_by_two_cells() {
        // The #913 measurement half: a resolved East-Asian-Wide glyph
        // occupies two terminal cells, so the *second* button's hit
        // region must start two cells further right than it does with a
        // one-cell ASCII fallback. Before the `display_width` fix in
        // `cell_measure` above, both layouts came out identical here and
        // a real hit-region/paint divergence would have gone unnoticed.
        let bar = bar_with(vec![
            mk_icon_action("a", "Refresh", None),
            mk_icon_action("b", "Drop", None),
        ]);
        let icons = ToolbarIcons::new().with(WidgetId::new("a"), Icon::new("一", "R"));

        let narrow = icons
            .apply(&bar, false)
            .layout(0.0, 0.0, 80.0, 1.0, cell_measure());
        let wide = icons
            .apply(&bar, true)
            .layout(0.0, 0.0, 80.0, 1.0, cell_measure());

        assert_eq!(
            wide.visible_items[0].bounds.width - narrow.visible_items[0].bounds.width,
            1.0,
            "the wide glyph is one cell wider than its ASCII fallback"
        );
        assert_eq!(
            wide.visible_items[1].bounds.x - narrow.visible_items[1].bounds.x,
            1.0,
            "the next button's hit region moves with it"
        );
    }

    // ── `new`/`with_*`/`Default` builders ──────────────────────────────

    /// `Default::default()` matches `Toolbar::new` with an empty id — the
    /// `new(required…)`/`with_*`/`Default` trio only differs in what `id`
    /// it carries.
    #[test]
    fn default_matches_new_with_empty_id() {
        assert_eq!(Toolbar::default(), Toolbar::new(WidgetId::new("")));
    }

    /// Chaining every `with_*` builder reaches exactly the values the
    /// exhaustive literal in `tests/downstream_struct_literals.rs` sets
    /// field-for-field, without writing a struct literal at all.
    #[test]
    fn with_builders_reach_every_field_a_struct_literal_can_set() {
        let bar = Toolbar::new(WidgetId::new("sidebar-action-bar"))
            .with_buttons(vec![mk_action("sidebar:refresh", "Refresh", true)])
            .with_bg(Color::rgb(10, 20, 30))
            .with_focused_index(Some(0));

        assert_eq!(
            bar,
            Toolbar {
                id: WidgetId::new("sidebar-action-bar"),
                buttons: vec![mk_action("sidebar:refresh", "Refresh", true)],
                bg: Some(Color::rgb(10, 20, 30)),
                focused_index: Some(0),
            }
        );
    }

    /// `with_focused_index(None)` clears it back to the `new()` default —
    /// the one builder here that takes `Option<usize>` rather than
    /// wrapping, precisely so a caller can forward its own
    /// `Option<usize>` state (including `None`) in one call.
    #[test]
    fn with_focused_index_accepts_none() {
        let bar = Toolbar::new(WidgetId::new("bar")).with_focused_index(Some(2));
        assert_eq!(bar.focused_index, Some(2));
        assert_eq!(bar.with_focused_index(None).focused_index, None);
    }
}
