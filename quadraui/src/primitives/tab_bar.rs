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
//! The primitive defines declarative state + events; backends resolve clicks
//! through [`crate::backend::tab_bar_hits_from_layout`].
//!
//! # Two painters, one primitive
//!
//! This module holds both of the primitive's rasterisers: the pixel one
//! ([`native_surface_paint::paint`], written against
//! [`crate::paint_surface::PaintSurface`]) and the cell-grid one
//! (`cell_paint::paint`, which `crate::tui` re-exports as its
//! `draw_tab_bar*` entry points). Neither resolves geometry — both paint
//! an already-resolved [`TabBarLayout`] verbatim — and keeping them
//! adjacent is what makes a divergence between terminal and pixel tab bars
//! a single-file diff.
//!
//! # `PaintSurface` migration status — painter lands, backends pending
//!
//! This module now has a shared [`native_surface_paint::paint`], the same
//! `&mut dyn PaintSurface`-generic shape [`crate::primitives::menu_bar`],
//! [`crate::primitives::toolbar`] and [`crate::primitives::activity_bar`]
//! already use. It paints background, active accent + chip, whole-tab
//! hover/pressed, a focus-visible ring, the label with ellipsis, an
//! optional leading icon, the close/dirty glyph, and scroll-arrow
//! chevrons — all from an already-resolved [`TabBarLayout`], the same
//! layout-in / paint-out split [`crate::primitives::toolbar::native_surface_paint::paint`]
//! takes.
//!
//! **`gtk::tab_bar` and `macos::tab_bar` both paint through it now;
//! `win::tab_bar` still carries its own full Direct2D paint loop**, and
//! the drift this table documents is between the shared module's own
//! coverage and that one remaining bespoke copy:
//!
//! | Behaviour | GTK + macOS (through this module) | Windows |
//! |---|---|---|
//! | Active-tab background | rounded, y-inset chip (radius 4, 4 px inset) | full-height square rect |
//! | Active-tab top accent | 1 px, on the chip's inset top edge | 2 px, on the strip's top edge |
//! | Close-button hover backdrop | rounded, `foreground` @ 15 % alpha | square, `tab_bar_bg.lighten(0.15)` |
//! | Preview-tab label | italic + `tab_preview_*_fg` | upright, **`tab_preview_*_fg` ignored** |
//! | [`TabFrame::Brackets`] framing | painted | **not painted** |
//!
//! Migrating `win::tab_bar` onto [`native_surface_paint::paint`] is a
//! separate, per-backend issue.
//!
//! ## The macOS `close_bounds` convention
//!
//! GTK, Windows and (now) macOS all resolve geometry through the shared
//! [`crate::primitives::layout_metrics::pixel_tab_bar_layout`] (macOS via
//! [`crate::primitives::layout_metrics::pixel_tab_bar_layout_fitting_boundary`]),
//! which sizes the close box as `tab_inner_gap + close_glyph_w` starting
//! one inner gap *before* the glyph. macOS's pre-migration bespoke
//! builders sized it differently — `close_glyph_w + 2 * CLOSE_PAD`
//! centred on the glyph — which is exactly the mismatch
//! [`native_surface_paint::paint`] sidesteps: it treats
//! [`VisibleTab::close_bounds`] as an opaque "glyph lives somewhere in
//! here" box and centres the glyph inside it, rather than assuming
//! either convention, so a caller on either one gets a correctly
//! centred glyph without this module having to pick a side.
//!
//! # Backend contract
//!
//! **`TabBar` has measurement-dependent state and a non-trivial backend
//! contract.** Skipping any step makes the active tab land off-screen
//! after layout changes (window resize, new file open, scroll-to).
//!
//! Per paint, the backend MUST:
//!
//! 1. **Measure each tab in its native unit.** Char counts for TUI, Pango
//!    pixel widths for GTK, DirectWrite for Win-GUI, Core Text for macOS.
//!    The measurement must include the tab's full visual width — label
//!    text *plus* any padding, close-button area, and inter-tab gap that
//!    the rendering will draw. Pre-compute into a `Vec<usize>` since
//!    you'll need it twice (once for the fit calculation, once for the
//!    paint loop).
//!
//! 2. **Compute the correct scroll offset** by calling
//!    [`TabBar::fit_active_scroll_offset`] with `(active_idx, tab_count,
//!    available_width, |i| measured[i])`. `available_width` and the
//!    measurer's return type must use the same unit.
//!
//! 3. **Write the result back to wherever the app stores `scroll_offset`.**
//!    The `bar.scroll_offset` field on the primitive itself is the *input*
//!    for this paint; the app holds the canonical value. Provide a setter
//!    that returns whether the value changed.
//!
//! 4. **If the offset changed, repaint with the corrected state.** This
//!    handles the case where last frame's offset was stale (window just
//!    resized, etc.). Two patterns work:
//!    - **TUI / Win-GUI** (loop-driven backends): the next loop iteration
//!      naturally redraws — set a "needs redraw" flag and continue.
//!    - **GTK / event-driven backends without mid-draw mutability**: do a
//!      *second paint inline within the same draw callback* (overdraw the
//!      same Cairo context). `idle_add` / queued draws are unreliable
//!      during continuous resize events.
//!
//! 5. **Use `bar.scroll_offset` (the input value) for the paint loop's
//!    starting tab index.** The corrected offset only matters for the
//!    next paint cycle; this paint shows what the engine state currently
//!    says.
//!
//! Skipping step 1 (using a generic char width estimate) under-estimates
//! per-tab width by ~4 cells in pixel-rendering backends — the active
//! tab gets clipped on the right edge.
//!
//! Skipping step 4 leaves the active tab off-screen until *some other*
//! event triggers a paint — what looks like a sticky bug to the user.
//!
//! See vimcode's `src/gtk/quadraui_gtk.rs::draw_tab_bar` and
//! `src/gtk/mod.rs::set_draw_func` for the GTK reference implementation,
//! and `src/tui_main/mod.rs` (post-`terminal.draw` block) for TUI.

use crate::event::Rect;
use crate::types::{Color, Icon, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::a11y::A11yAction;
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::event::{Key, MouseButton, NamedKey};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, WidgetAction,
};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::Handled;

/// Declarative description of a tab bar.
///
/// # Examples
///
/// ```
/// use quadraui::{SegmentMeasure, TabBar, TabBarHit, TabItem, TabMeasure, WidgetId};
///
/// let bar = TabBar {
///     id: WidgetId::new("tabbar:editor"),
///     tabs: vec![TabItem {
///         label: "main.rs".to_string(),
///         is_active: true,
///         ..Default::default()
///     }],
///     scroll_offset: 0,
///     right_segments: vec![],
///     active_accent: None,
///     show_tab_close: true,
///     compact: false,
/// };
///
/// let layout = bar.layout(
///     80.0,
///     1.0,
///     2.0,
///     |_| TabMeasure::new(20.0, 2.0),
///     |_| SegmentMeasure::new(0.0),
/// );
/// assert_eq!(layout.hit_test(1.0, 0.0), TabBarHit::Tab(0));
/// ```
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
    /// When `false`, per-tab close buttons (×/●) are suppressed — the
    /// measurer returns `close_width: 0.0` and rasterisers skip the glyph.
    /// Defaults to `true` for backward compatibility with file-tab bars.
    #[serde(default = "default_true")]
    pub show_tab_close: bool,
    /// When `true`, the GTK rasteriser uses minimal padding (2px) and
    /// zero gap between tabs. Intended for compact chrome like terminal
    /// toolbar tabs or bottom panel tab switchers. TUI ignores this flag —
    /// it never pads tabs, closable or not — but still separates adjacent
    /// labels with a column reserved for a glyph (see
    /// `tui::TAB_SEPARATOR_CHAR`) whenever a tab has no close button to
    /// provide that boundary instead.
    #[serde(default)]
    pub compact: bool,
}

fn default_true() -> bool {
    true
}

/// An empty bar with the same field values a deserialised `{}` gets, so a
/// struct literal can name only the fields it cares about and end in
/// `..Default::default()`.
impl Default for TabBar {
    fn default() -> Self {
        Self {
            id: WidgetId::new("tab-bar"),
            tabs: Vec::new(),
            scroll_offset: 0,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }
}

/// Optional per-tab icon drawn before the label — the coloured
/// language/file-type glyph VS Code shows on every tab (e.g. an orange
/// gear for `.toml`, a blue badge for `.ts`). Glyph-sourcing follows the
/// same convention as [`crate::primitives::activity_bar::ActivityItem::icon`]
/// (typically a single Nerd Font codepoint); `color` tints just the
/// glyph and is independent of the tab's active/inactive foreground, so
/// the icon keeps its identity colour even on an inactive tab.
///
/// # Why icons ride *beside* the bar, not inside [`TabItem`]
///
/// Icons are supplied as a **sidecar slice** parallel to [`TabBar::tabs`]
/// — see [`Backend::draw_tab_bar_icons`] — rather than as a
/// `TabItem::icon` field. `TabItem` is a plain (non-`#[non_exhaustive]`)
/// struct that both downstream consumers and this repo's sealed
/// acceptance slices build with **exhaustive literals**, so a new field
/// is a hard break for every one of them (CLAUDE.md rule 8 / the
/// `PRIMITIVE_RULES.md` rule-8 blast-radius table: *"new field on a
/// public struct a consumer constructs"* ⇒ breaking; *"new `Backend`
/// method, no default"* ⇒ not breaking, keep doing this). The sidecar
/// buys the same capability at zero migration cost: callers that want
/// icons pass a slice, callers that don't keep calling
/// [`Backend::draw_tab_bar`] unchanged.
///
/// [`Backend::draw_tab_bar_icons`]: crate::Backend::draw_tab_bar_icons
/// [`Backend::draw_tab_bar`]: crate::Backend::draw_tab_bar
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TabIcon {
    /// Icon glyph — usually a single Nerd Font character.
    pub glyph: String,
    /// Glyph colour, drawn regardless of the tab's active state.
    pub color: Color,
}

impl TabIcon {
    /// TUI column width this icon reserves: the glyph's display width
    /// (via [`crate::text_util::display_width`] — Nerd Font PUA icon
    /// glyphs measure 1 column, per `char_cell_width`'s convention) plus
    /// a 1-column gap before the label.
    ///
    /// GTK / macOS measure their own icon width in pixels (glyph metrics
    /// differ from the TUI cell-width model), so this helper is
    /// TUI-specific; see `gtk::tab_bar::draw_tab_bar_icons`'s own Pango
    /// measurement instead.
    pub fn cols(&self) -> u16 {
        crate::text_util::display_width(&self.glyph) as u16 + 1
    }

    /// Build a [`TabIcon`] from one of the bundled codicon set's names
    /// (`"file-code"`, `"markdown"`, …) instead of a raw glyph string —
    /// the same table [`crate::Icon::codicon`] resolves names through.
    /// `None` if `name` isn't in the crate's curated name table; see
    /// `codicon_names.rs`'s module doc for the full list.
    #[must_use]
    pub fn codicon(name: &str, color: Color) -> Option<Self> {
        let glyph = crate::codicon_names::codepoint(name)?;
        Some(Self {
            glyph: glyph.to_string(),
            color,
        })
    }
}

/// Accepts both `{"glyph": ..., "color": ...}` and `{"codicon": "...",
/// "color": ...}` — the same two-shape convention as [`Icon`]'s manual
/// `Deserialize` impl; see that type's doc for why `Codicon` is tried
/// first.
impl<'de> Deserialize<'de> for TabIcon {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Codicon { codicon: String, color: Color },
            Glyph { glyph: String, color: Color },
        }

        match Wire::deserialize(deserializer)? {
            Wire::Glyph { glyph, color } => Ok(TabIcon { glyph, color }),
            Wire::Codicon { codicon, color } => {
                let glyph = crate::codicon_names::codepoint(&codicon).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "unknown codicon name {codicon:?} — see codicon_names::NAMES"
                    ))
                })?;
                Ok(TabIcon {
                    glyph: glyph.to_string(),
                    color,
                })
            }
        }
    }
}

/// The icon for tab `idx` in an icon sidecar slice, or `None` when the
/// slot is empty **or the slice is shorter than the tab list**.
///
/// Every backend resolves icons through this one helper so the
/// "shorter-than-`tabs` slice means no icon" convention can't drift
/// between rasterisers: `&[]` is always a legal "no icons at all"
/// argument, and a caller that only decorates the first few tabs never
/// has to pad the slice with `None`s.
pub fn tab_icon_at(icons: &[Option<TabIcon>], idx: usize) -> Option<&TabIcon> {
    icons.get(idx).and_then(|slot| slot.as_ref())
}

/// TUI columns reserved for tab `idx`'s icon (glyph width + 1-column
/// gap), or `0` when that tab has no icon. Shared by the TUI
/// rasteriser's paint and measurement paths so the two can't disagree.
pub fn tab_icon_cols(icons: &[Option<TabIcon>], idx: usize) -> u16 {
    tab_icon_at(icons, idx).map_or(0, TabIcon::cols)
}

/// Compute the label text to paint inside a tab, after reserving the
/// leading icon slot and the trailing close-button / dirty-dot slot.
///
/// `tab_width` is the tab's full measured width — [`TabMeasure::total_width`]
/// for a layout built via [`TabBar::layout`]. `icon_width` is the width of
/// the leading icon slot (before the label; `0.0` when the tab has no
/// icon). `close_width` is the width of the trailing close-button /
/// dirty-dot slot (after the label; `0.0` when the tab has no close
/// button). Both are in the same unit as `tab_width` — char-cell counts
/// for TUI, Pango/DirectWrite/Core Text pixel widths for GTK/Win/macOS.
/// `measure` returns an arbitrary string's width in that same unit.
///
/// Delegates to [`crate::text_util::truncate_with_ellipsis`]: the label
/// comes back unchanged when it already fits in `tab_width - icon_width -
/// close_width`, and ellipsis-truncated otherwise. The returned string's
/// measured width never exceeds that budget, so a backend that paints it
/// flush against the icon/close slots can never overlap either one.
///
/// Every tab-bar backend calls this instead of re-deriving its own
/// truncation — see [`crate::tui::tab_bar`]'s painter for the reference
/// call, which derives `icon_width`/`close_width` from the same slot
/// widths that built the tab's [`TabMeasure`].
pub fn fit_tab_label<F>(
    label: &str,
    tab_width: f32,
    icon_width: f32,
    close_width: f32,
    measure: F,
) -> String
where
    F: Fn(&str) -> f32,
{
    let available = (tab_width - icon_width - close_width).max(0.0);
    crate::text_util::truncate_with_ellipsis(label, available, measure).0
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
    /// When `true`, this tab renders an individual `×` / `●` close button
    /// (subject to [`TabBar::show_tab_close`] also being `true`). When
    /// `false`, the backend omits the close button for this tab even when
    /// `show_tab_close` is set on the bar — no space is reserved and no
    /// glyph is rendered. Defaults to `true` for backward compatibility.
    #[serde(default = "default_true")]
    pub is_closable: bool,
}

impl Default for TabItem {
    /// Matches the per-field `#[serde(default...)]` values above —
    /// notably `is_closable: true`, not derived-`Default`'s `false`.
    fn default() -> Self {
        Self {
            label: String::new(),
            is_active: false,
            is_dirty: false,
            is_preview: false,
            is_closable: true,
        }
    }
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

    /// Keyboard navigation for [`TabChrome::focused_index`]: `Left`/
    /// `ArrowLeft` and `Right`/`ArrowRight` move the focused tab one
    /// position (clamped to `0..tab_count`, starting from tab `0` when
    /// nothing was focused yet); `Enter` and `Space`/`" "` activate the
    /// currently focused tab.
    ///
    /// A pure, static helper rather than a `&mut self` method — like
    /// [`Self::fit_active_scroll_offset`] — because the focused index
    /// lives in [`TabChrome`], not on `TabBar` itself (see that field's
    /// doc for why). The caller supplies the current focus (`None` if no
    /// tab is focused yet) and gets back either the new focus to store
    /// for next frame (`FocusMoved`) or the tab to activate
    /// (`TabActivated`); every other key returns `None` so the caller's
    /// own [`TabBarEvent::KeyPressed`] fallback still fires.
    ///
    /// `tab_count == 0` always returns `None` — there is nothing to
    /// focus or activate.
    pub fn handle_focus_key(
        focused: Option<usize>,
        tab_count: usize,
        key: &str,
    ) -> Option<TabBarEvent> {
        if tab_count == 0 {
            return None;
        }
        match key {
            "ArrowLeft" | "Left" => {
                let cur = focused.unwrap_or(0);
                Some(TabBarEvent::FocusMoved {
                    index: cur.saturating_sub(1),
                })
            }
            "ArrowRight" | "Right" => {
                let cur = focused.unwrap_or(0);
                Some(TabBarEvent::FocusMoved {
                    index: (cur + 1).min(tab_count - 1),
                })
            }
            "Enter" | "Space" | " " => focused.map(|index| TabBarEvent::TabActivated { index }),
            _ => None,
        }
    }
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Primitives return fully-resolved `Layout` structs; backends rasterise verbatim.
// A backend that fails to consume a field (e.g. doesn't iterate `visible_tabs`)
// produces visibly broken output on its own platform — not silent divergence on
// the next one. Tab-bar layout is the reference implementation of this pattern.
//
// All coordinates are in the backend's native unit (char cells for TUI,
// pixels for GTK / Win-GUI / macOS). The primitive is unit-agnostic: the
// caller supplies measurements and the same unit comes back in the
// returned `Rect`s.

/// Close-button glyph the cell-grid rasteriser (`cell_paint::paint`,
/// re-exported as `crate::tui::TAB_CLOSE_CHAR`) paints on each tab.
/// `×` (U+00D7 MULTIPLICATION SIGN) — narrower than
/// `✕` and present in every monospaced terminal font we've encountered.
pub const TAB_CLOSE_CHAR: char = '×';

/// Cell width reserved per tab for the close-button + trailing
/// separator. Apps that build a [`TabMeasure`] for cell-grid tab-bar
/// layout pass this as the `close_width` so the layout reserves the
/// right amount of trailing space.
pub const TAB_CLOSE_COLS: u16 = 2;

/// Separator glyph painted between adjacent tab labels that have no
/// close button to provide their own visual boundary (`show_tab_close:
/// false`, or a non-closable tab). Light vertical box-drawing rule —
/// renders in every monospaced terminal font and reads as a distinct
/// item boundary rather than running labels together.
pub const TAB_SEPARATOR_CHAR: char = '│';

/// Cell width reserved after a tab with no close button, for
/// [`TAB_SEPARATOR_CHAR`]. Apps that build a [`TabMeasure`] for a tab
/// with `close_width: 0.0` should add this to the tab's measured width so
/// the rasteriser has a cell to paint the separator into — with one
/// exception: a bracket-framed active tab (see [`TabChrome`]) also
/// measures `close_width: 0.0`, but its final column is the closing `]`,
/// not a separator, so it must not add this constant.
///
/// The rasteriser paints the separator right after the label (plus any
/// close glyph), not necessarily at the tab's last column — if the
/// measurer reserves more trailing width than this constant, the extra
/// slack lands as blank space between the separator and the next tab.
/// Every no-close tab gets one, including the last visible tab, so a bar
/// with no right segments ends on a trailing rule rather than a blank
/// edge; that's deliberate, matching how a close-button bar always ends
/// on its last `×`/`●` glyph rather than blank space.
pub const TAB_SEPARATOR_COLS: u16 = 1;

/// Per-tab measurement supplied by the backend's layout caller.
///
/// `total_width` is the tab's full visual width (label + padding + close
/// button + inter-tab gap). `close_width` is the width of the close-button
/// hit region; `0.0` means the tab has no close button (e.g. a pinned
/// tab). Absent [`Self::trailing_width`] (the default, `0.0`), the close
/// region sits flush against the tab's right edge — `total_width -
/// close_width .. total_width`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabMeasure {
    pub total_width: f32,
    pub close_width: f32,
    /// Width of chrome painted *after* the close-button hit region —
    /// e.g. a closing-bracket glyph requested via
    /// [`TabChrome::active_frame`] — that counts toward `total_width` but
    /// must be excluded from the close region so [`TabBarLayout`]'s
    /// `close_bounds` still lands on the glyph itself, not the chrome
    /// wrapping it. Defaults to `0.0` (see [`Self::new`]), reproducing
    /// the behaviour where the close region is flush against the
    /// tab's right edge.
    pub trailing_width: f32,
}

impl TabMeasure {
    /// `trailing_width` defaults to `0.0` — see the struct doc. Use
    /// [`Self::with_trailing`] to reserve chrome after the close region.
    pub fn new(total_width: f32, close_width: f32) -> Self {
        Self {
            total_width,
            close_width,
            trailing_width: 0.0,
        }
    }

    /// Reserve `trailing_width` of `total_width` for chrome painted after
    /// the close-button hit region (e.g. a closing-bracket glyph), so
    /// [`TabBarLayout`]'s `close_bounds` lands before it instead of
    /// overlapping it.
    pub fn with_trailing(mut self, trailing_width: f32) -> Self {
        self.trailing_width = trailing_width;
        self
    }
}

/// Per-segment measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentMeasure {
    pub width: f32,
}

impl SegmentMeasure {
    pub fn new(width: f32) -> Self {
        Self { width }
    }
}

/// Resolved position of one visible tab after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTab {
    /// Index into the original `TabBar.tabs` Vec.
    pub tab_idx: usize,
    /// Full tab rectangle (includes close-button area).
    pub bounds: Rect,
    /// Close-button sub-rectangle, if the tab has one.
    pub close_bounds: Option<Rect>,
}

/// Resolved position of one visible right-aligned segment after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleSegment {
    /// Index into the original `TabBar.right_segments` Vec.
    pub segment_idx: usize,
    pub bounds: Rect,
    /// `true` iff the segment has an `id` (is clickable).
    pub clickable: bool,
}

/// Classification of a hit-test result. Produced by
/// [`TabBarLayout::hit_test`]; backends translate native mouse events
/// into one of these variants.
///
/// Variant order in `hit_regions` is from most-specific to least: close
/// buttons before tab bodies, scroll arrows and segments are disjoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabBarHit {
    /// Click landed on a tab body (not its close button). Index is into
    /// `TabBar.tabs`.
    Tab(usize),
    /// Click landed on a tab's close button. Index is into `TabBar.tabs`.
    TabClose(usize),
    /// Click landed on the scroll-left affordance.
    ScrollLeft,
    /// Click landed on the scroll-right affordance.
    ScrollRight,
    /// Click landed on a right-aligned clickable segment.
    RightSegment(WidgetId),
    /// Click landed in dead space — no action.
    Empty,
}

/// Which tab-bar sub-part a [`crate::interactive::SubTarget`] names —
/// [`TabBarHit`] re-expressed as the borrow-less, widget-agnostic tag the
/// widget contract addresses sub-parts with.
///
/// [`TabBarHit`] itself can't be that tag: it carries a [`WidgetId`] for
/// a right segment (not expressible in a `SubTarget`'s `u64`) and has an
/// `Empty` variant, which is the *absence* of a sub-part rather than one
/// of them. This enum carries the segment's index instead, and has no
/// `Empty`: "no sub-part under the point" is
/// [`crate::primitive::PaintedWidgets::painted_hit`]'s `None`.
///
/// Round-trips through [`Self::sub_target`]/[`Self::from_sub_target`], so
/// a driver asks for `TabBarSubPart::TabClose(2)`'s painted rect and a
/// binding layer turns a hit back into a part it can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabBarSubPart {
    /// A tab body (not its close button). Index is into `TabBar.tabs`.
    Tab(usize),
    /// A tab's close button. Index is into `TabBar.tabs`.
    TabClose(usize),
    /// The scroll-left affordance.
    ScrollLeft,
    /// The scroll-right affordance.
    ScrollRight,
    /// A right-aligned clickable segment, by index into
    /// `TabBar.right_segments`.
    RightSegment(usize),
}

impl TabBarSubPart {
    /// This part's [`crate::interactive::SubTarget`] tag: an 8-bit kind
    /// in the high bits, the index in the low 32 — the same
    /// pack-a-pair-into-one-`u64` convention
    /// [`crate::interactive::FocusStop`]'s own cell-packing example uses.
    pub fn sub_target(self) -> crate::interactive::SubTarget {
        let (kind, idx) = match self {
            Self::Tab(i) => (1u64, i as u64),
            Self::TabClose(i) => (2, i as u64),
            Self::ScrollLeft => (3, 0),
            Self::ScrollRight => (4, 0),
            Self::RightSegment(i) => (5, i as u64),
        };
        crate::interactive::SubTarget((kind << 32) | (idx & 0xffff_ffff))
    }

    /// Decode a [`Self::sub_target`] tag back into a part, or `None` for
    /// a tag this widget never produced (another widget's sub-target).
    pub fn from_sub_target(sub: crate::interactive::SubTarget) -> Option<Self> {
        let idx = (sub.0 & 0xffff_ffff) as usize;
        match sub.0 >> 32 {
            1 => Some(Self::Tab(idx)),
            2 => Some(Self::TabClose(idx)),
            3 => Some(Self::ScrollLeft),
            4 => Some(Self::ScrollRight),
            5 => Some(Self::RightSegment(idx)),
            _ => None,
        }
    }
}

/// Every addressable sub-part of a resolved [`TabBarLayout`], paired with
/// the [`crate::interactive::SubTarget`] that names it, in the layout's
/// own bar-relative space.
///
/// Most-specific first: close buttons before the tab bodies that contain
/// them, so the first rect containing a point is the right answer — the
/// same ordering [`TabBarLayout::hit_test`] walks its own `hit_regions`
/// in. Only *clickable* right segments are listed; a decorative one
/// addresses nothing.
///
/// Shared by [`crate::interactive::Interactive::sub_regions`] (the
/// painted-layout record's source) and [`tab_part_center`]'s legacy
/// fallback, so both describe the same geometry.
pub fn tab_bar_sub_regions(layout: &TabBarLayout) -> Vec<(Rect, crate::interactive::SubTarget)> {
    let mut out = Vec::new();
    for vt in &layout.visible_tabs {
        if let Some(close) = vt.close_bounds {
            out.push((close, TabBarSubPart::TabClose(vt.tab_idx).sub_target()));
        }
    }
    for vt in &layout.visible_tabs {
        out.push((vt.bounds, TabBarSubPart::Tab(vt.tab_idx).sub_target()));
    }
    if let Some(r) = layout.scroll_left {
        out.push((r, TabBarSubPart::ScrollLeft.sub_target()));
    }
    if let Some(r) = layout.scroll_right {
        out.push((r, TabBarSubPart::ScrollRight.sub_target()));
    }
    for vs in &layout.visible_segments {
        if vs.clickable {
            out.push((
                vs.bounds,
                TabBarSubPart::RightSegment(vs.segment_idx).sub_target(),
            ));
        }
    }
    out
}

/// The absolute center of one tab-bar sub-part in the bar `bar` — the
/// point a click must land on to hit it.
///
/// Resolves against the frame's painted-layout record first
/// ([`crate::primitive::PaintedWidgets`]), which every bar painted
/// through [`crate::DrawsPrimitives::draw_dyn`] registers itself in. A
/// bar still painted by a legacy `Backend::draw_tab_bar*` rasteriser
/// isn't in that record, so `legacy` is the per-backend
/// `(bar rect, resolved layout)` cache those rasterisers fill — pass
/// `None` to query the record alone.
///
/// `None` when `bar` didn't paint this frame, or when the named part
/// isn't on screen (a tab scrolled out of view behind the bar's
/// `scroll_offset`, a tab that drew no close button, an absent scroll
/// chevron).
pub fn tab_part_center(
    bar: &WidgetId,
    part: TabBarSubPart,
    legacy: Option<(Rect, &TabBarLayout)>,
) -> Option<(f32, f32)> {
    let sub = part.sub_target();
    if let Some(center) = crate::primitive::painted_sub_center(bar, sub) {
        return Some(center);
    }
    let (rect, layout) = legacy?;
    let (part_rect, _) = tab_bar_sub_regions(layout)
        .into_iter()
        .find(|(_, s)| *s == sub)?;
    Some((
        rect.x + part_rect.x + part_rect.width / 2.0,
        rect.y + part_rect.y + part_rect.height / 2.0,
    ))
}

/// Fully-resolved tab-bar layout. Backends iterate `visible_tabs` /
/// `visible_segments` for painting; call [`Self::hit_test`] for clicks.
///
/// # Writing `resolved_scroll_offset` back
///
/// When a frame paints with a stale `TabBar.scroll_offset` (e.g. after a
/// window resize or a jump to a tab that wasn't previously visible), the
/// layout corrects it. The backend should write `resolved_scroll_offset`
/// back to the app's stored scroll state so the next frame starts
/// coherent. See `TabBar::layout` docs for the two-pass-paint pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct TabBarLayout {
    /// Total bar width in the measurer's unit (copied from input).
    pub bar_width: f32,
    /// Total bar height in the measurer's unit (copied from input).
    pub bar_height: f32,
    /// Tabs that made it onto the bar, left-to-right as drawn.
    pub visible_tabs: Vec<VisibleTab>,
    /// Right-aligned segments that fit, drawn left-to-right starting from
    /// their resolved left edge.
    pub visible_segments: Vec<VisibleSegment>,
    /// Left scroll-arrow rectangle, present iff `resolved_scroll_offset > 0`
    /// and `scroll_arrow_width > 0.0`.
    pub scroll_left: Option<Rect>,
    /// Right scroll-arrow rectangle, present iff tabs extend beyond the
    /// visible area and `scroll_arrow_width > 0.0`.
    pub scroll_right: Option<Rect>,
    /// Ordered hit-region list. `hit_test` walks this from the start and
    /// returns the first containing region. More-specific regions (close
    /// buttons) come before containing regions (tab bodies).
    pub hit_regions: Vec<(Rect, TabBarHit)>,
    /// Scroll offset actually used. May differ from `TabBar.scroll_offset`
    /// if the input was stale.
    pub resolved_scroll_offset: usize,
}

impl TabBarLayout {
    /// Test which clickable region (if any) contains point `(x, y)`.
    /// Returns `TabBarHit::Empty` when no region matches.
    pub fn hit_test(&self, x: f32, y: f32) -> TabBarHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        TabBarHit::Empty
    }

    /// Center point of tab `tab_idx`'s full bounds (label + close button),
    /// in this layout's own bar-relative coordinate space — the same
    /// space `visible_tabs[].bounds` uses (origin at the bar's top-left,
    /// *not* the target surface). `None` if `tab_idx` isn't in
    /// `visible_tabs`, i.e. it's scrolled out of view behind the bar's
    /// `scroll_offset`.
    ///
    /// Backends that cache a `TabBarLayout` per `WidgetId` at paint time
    /// add the cached bar rect's own `(x, y)` origin to this to get an
    /// absolute screen point — see `TuiDriver::tab_center` /
    /// `GtkDriver::tab_center`, which every tab bar needs
    /// since every tab paints the same label-independent chrome and
    /// `find()` can't disambiguate tab 3's target from tab 0's the way it
    /// can for uniquely-labeled text.
    pub fn tab_center(&self, tab_idx: usize) -> Option<(f32, f32)> {
        self.visible_tabs
            .iter()
            .find(|vt| vt.tab_idx == tab_idx)
            .map(|vt| {
                (
                    vt.bounds.x + vt.bounds.width / 2.0,
                    vt.bounds.y + vt.bounds.height / 2.0,
                )
            })
    }

    /// Center point of tab `tab_idx`'s close-button hit region, in the
    /// same bar-relative space as [`Self::tab_center`]. `None` when the
    /// tab is scrolled out of view (see [`Self::tab_center`]) *or* when
    /// it drew no close button this frame — `is_closable: false` on the
    /// tab, or `show_tab_close: false` on the bar.
    pub fn tab_close_center(&self, tab_idx: usize) -> Option<(f32, f32)> {
        self.visible_tabs
            .iter()
            .find(|vt| vt.tab_idx == tab_idx)
            .and_then(|vt| vt.close_bounds)
            .map(|cb| (cb.x + cb.width / 2.0, cb.y + cb.height / 2.0))
    }
}

/// Active-tab framing vocabulary: which decoration, if any, should
/// enclose the active tab's full content — label *and* close glyph.
///
/// The close region is always flush against the tab's right edge unless
/// [`TabMeasure::trailing_width`] reserves space for chrome painted
/// *after* the close glyph. `#[non_exhaustive]`: a future frame style
/// (e.g. a stroked border) is additive rather than breaking.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TabFrame {
    /// No enclosing decoration beyond the tab's ordinary active
    /// background.
    #[default]
    None,
    /// Encloses the active tab's full content — icon, label, and close
    /// glyph — in bracket framing. TUI paints literal `[` / `]` glyphs
    /// around the tab's content; GTK mirrors them with Pango-measured
    /// bracket glyphs so the two backends agree on what "enclosing" means.
    Brackets,
}

/// Per-bar chrome request: which active-tab frame a backend should
/// paint. Passed alongside a [`TabBar`] to
/// [`crate::Backend::draw_tab_bar_with_chrome`] /
/// [`crate::Backend::tab_bar_layout_with_chrome`].
///
/// A separate sidecar value rather than a field on [`TabBar`] itself,
/// to avoid requiring a new field on the plain struct `TabBar` which
/// is built with exhaustive literals throughout this repo. Threading
/// the vocabulary through a new value + new trait methods (both given
/// default bodies that ignore chrome and delegate to the plain
/// `draw_tab_bar` / `tab_bar_layout`) adds no required field and breaks
/// no existing `Backend` implementor or call site.
///
/// `#[non_exhaustive]`: a future chrome knob (e.g. a frame colour
/// override) is additive. Construct with [`TabChrome::new`] /
/// [`TabChrome::default`] and [`TabChrome::with_active_frame`]; the
/// field stays `pub` for reading.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TabChrome {
    /// Which frame to paint around the active tab. Defaults to
    /// [`TabFrame::None`].
    #[serde(default)]
    pub active_frame: TabFrame,
    /// Index into [`TabBar::tabs`] of the keyboard-focused tab, if any.
    ///
    /// `Some(i)` tells the rasteriser to paint a focus-visible treatment
    /// on tab `i` (reverse-video on TUI — see [`crate::tui::tab_bar`]).
    /// The caller is responsible for composing this with its own
    /// focus-visible state (e.g. [`crate::FocusTree::is_focus_visible`]):
    /// pass `None` whenever the bar has keyboard focus but focus isn't
    /// currently *visible* (the most recent interaction was a click), the
    /// same convention [`Toolbar::focused_index`] documents.
    ///
    /// An index `>= tabs.len()` paints nothing (no reverse-video, no
    /// panic) on TUI — see `tui::tab_bar::draw_tab_bar_with_chrome_*`,
    /// which only ever compares against an actually-visible tab's index.
    ///
    /// A sidecar field rather than one on [`TabBar`] itself, for the same
    /// reason [`Self::active_frame`] is: `TabBar` is built with
    /// exhaustive literals throughout this repo (including the sealed
    /// acceptance slices), so a new field there is breaking everywhere at
    /// once; a new [`TabChrome`] knob is additive.
    ///
    /// [`Toolbar::focused_index`]: crate::primitives::toolbar::Toolbar::focused_index
    #[serde(default)]
    pub focused_index: Option<usize>,
}

impl TabChrome {
    /// Chrome requesting the given active-tab frame.
    pub fn new(active_frame: TabFrame) -> Self {
        Self {
            active_frame,
            focused_index: None,
        }
    }

    /// Set the active-tab frame.
    pub fn with_active_frame(mut self, active_frame: TabFrame) -> Self {
        self.active_frame = active_frame;
        self
    }

    /// Set the keyboard-focused tab index. `None` paints no focus
    /// treatment at all.
    pub fn with_focused_index(mut self, focused_index: Option<usize>) -> Self {
        self.focused_index = focused_index;
        self
    }
}

/// Optional app-supplied override for the close and dirty (unsaved)
/// marks a tab bar paints.
///
/// A standalone, `#[non_exhaustive]` registry — not fields on
/// [`TabBar`]/[`TabItem`], which stay exhaustive — so a consumer that
/// wants its own iconography (e.g. a Nerd Font glyph) passes a
/// `TabBarGlyphs` to the rasteriser without the crate taking on a
/// breaking field addition. Leaving a field `None`, or passing no
/// `TabBarGlyphs` at all, keeps today's default (`×` for close, `●` for
/// dirty) byte-for-byte unchanged.
///
/// Each glyph is an [`Icon`], so a consumer can hand it a
/// Nerd Font form and a plain-Unicode fallback; the TUI rasteriser
/// paints a single terminal cell, resolves the glyph via
/// [`crate::Backend::nerd_fonts_enabled`], and only consumes the first
/// `char` — a multi-character override silently degrades to its
/// leading character. `Icon::color` is resolved but not painted on
/// TUI: the glyph cell always uses the surrounding theme foreground.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TabBarGlyphs {
    close: Option<Icon>,
    dirty: Option<Icon>,
}

impl TabBarGlyphs {
    /// No overrides — every tab paints today's default glyphs.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the close-button glyph (default `×`).
    #[must_use]
    pub fn with_close_glyph(mut self, glyph: impl Into<Icon>) -> Self {
        self.close = Some(glyph.into());
        self
    }

    /// Override the dirty (unsaved) mark (default `●`).
    #[must_use]
    pub fn with_dirty_glyph(mut self, glyph: impl Into<Icon>) -> Self {
        self.dirty = Some(glyph.into());
        self
    }

    /// The overridden close-button glyph, or `None` to keep the default.
    #[must_use]
    pub fn close_glyph(&self) -> Option<&Icon> {
        self.close.as_ref()
    }

    /// The overridden dirty-mark glyph, or `None` to keep the default.
    #[must_use]
    pub fn dirty_glyph(&self) -> Option<&Icon> {
        self.dirty.as_ref()
    }
}

/// Per-frame interaction-state output from a tab-bar rasteriser. All
/// positions are in target-surface coordinates.
///
/// Apps consume this to dispatch clicks. Tabs before the scroll offset
/// get sentinel entries so indices in `slot_positions` / `close_bounds`
/// line up with `bar.tabs`.
///
/// # Legacy `f64` coordinates
///
/// Every other hit/layout struct in this crate uses `f32`; this one uses
/// `f64` pairs and `available_cols: usize` in character columns (a
/// TUI-only concept even GTK/macOS fake by measuring a Pango sample
/// string), for compatibility with existing downstream code.
///
/// # Stability
///
/// This is the return type of `draw_tab_bar` / `tab_bar_layout` and their
/// icon/chrome twins, alongside [`TabBarLayout`] — f32,
/// `Rect`/[`TabBarHit`]-based, unit agnostic — which the `*_layout`-suffixed
/// methods and [`crate::primitives::layout_metrics::pixel_tab_bar_layout`]
/// return instead. Both return types coexist; a caller picks whichever
/// coordinate convention and unit fits — target-surface-absolute `f64`
/// pairs here, bar-relative `Rect`s there.
///
/// Removing this struct in favour of `TabBarLayout` alone needs two things
/// neither in-repo crate code can do on its own: the sealed acceptance
/// slice at `tests/acceptance/ms-11/wide_tab_labels.rs` calls `draw_tab_bar`
/// and reads `.slot_positions` straight off the return value, and a
/// `Backend` trait signature change can't touch that file; and the
/// `coord-tui` consumer calls `Backend::tab_bar_layout` /
/// `tab_bar_layout_with_chrome` and reads `.slot_positions` /
/// `.close_bounds` / `.correct_scroll_offset` directly in several of its
/// own click-dispatch paths, so dropping the type would stop that consumer
/// from compiling against this crate.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TabBarHits {
    /// `[(start_x, end_x)]` per tab index. Tabs before
    /// `bar.scroll_offset` have zero-width `(0.0, 0.0)` sentinels.
    pub slot_positions: Vec<(f64, f64)>,
    /// `[Some((start_x, end_x))]` for each visible tab's close-button
    /// hit zone, or `None` for tabs without a close button (and
    /// sentinels for tabs before the scroll offset). Indexed by tab
    /// index in `bar.tabs` so callers don't recompute close geometry
    /// — the rasteriser knows the exact placement and reports it.
    pub close_bounds: Vec<Option<(f64, f64)>>,
    /// `[(start_x, end_x)]` per right-segment index, in the order the
    /// segments were declared.
    pub right_segment_bounds: Vec<(f64, f64)>,
    /// Tab-bar content width in **character columns** (computed from a
    /// 15-char sample's Pango width). Useful for engines that decide
    /// per-tab budgets in cell units.
    pub available_cols: usize,
    /// Scroll offset that would make the active tab visible *given
    /// this frame's actual measurements*. Caller compares to
    /// `bar.scroll_offset` and triggers a repaint if they differ.
    pub correct_scroll_offset: usize,
}

impl TabBar {
    /// Compute the full rendering + hit-test layout for this tab bar.
    ///
    /// Per D6: layout decisions live here; backends consume the returned
    /// `TabBarLayout` verbatim (iterate `visible_tabs` /
    /// `visible_segments` for painting; call `hit_test` for clicks).
    /// Backends do not make their own decisions about overflow, scroll
    /// offset, segment drop, or close-button position.
    ///
    /// # Arguments
    ///
    /// - `bar_width`, `bar_height` — bar dimensions in the measurer's
    ///   unit.
    /// - `scroll_arrow_width` — reserved width for each scroll arrow
    ///   when tabs overflow. Pass `0.0` to disable scroll arrows; tabs
    ///   that don't fit are then simply clipped off the right without
    ///   any visual indicator.
    /// - `measure_tab(i)` — returns total + close widths for tab `i`.
    /// - `measure_segment(i)` — returns width for right-segment `i`.
    ///
    /// All arguments share the same unit; the primitive itself is
    /// unit-agnostic. For TUI pass char-cell counts (e.g.
    /// `measure_tab(i) = TabMeasure::new(label.chars().count() as f32,
    /// 1.0)`); for GTK / Win-GUI / macOS pass pixel widths from Pango /
    /// DirectWrite / Core Text.
    ///
    /// # Overflow policy (v1)
    ///
    /// - **Right segments:** kept together as one block. If the block
    ///   would literally not fit inside the bar (`total > bar_width`),
    ///   it's dropped entirely (all or nothing). Otherwise it renders,
    ///   even if it leaves little room for tabs — matches pre-D6
    ///   behaviour in vimcode's TUI / GTK / Win-GUI backends. Priority-
    ///   drop per-segment (like `StatusBar::fit_right_start`) is a
    ///   planned iteration — tab-bar segments tend to be either a small
    ///   action cluster or nothing, so per-segment priority ranks
    ///   aren't yet useful.
    /// - **Tabs:** when the full set doesn't fit, `scroll_offset` is
    ///   chosen to keep the active tab visible (delegates to
    ///   [`Self::fit_active_scroll_offset`]). Scroll arrows appear on
    ///   the sides that have hidden content.
    /// - **Close buttons:** always positioned at the right end of their
    ///   tab. Backends supply `close_width` per-tab; a value of `0.0`
    ///   suppresses the close button.
    ///
    /// # Two-pass-paint pattern (GTK / event-driven backends)
    ///
    /// If `resolved_scroll_offset != self.scroll_offset`, the current
    /// paint reflects the layout's correction; write
    /// `resolved_scroll_offset` back to the app's stored value and
    /// invalidate or repaint. GTK must do the second paint inline (see
    /// `PLAN.md` lesson on `idle_add_local_once` unreliability).
    pub fn layout<F1, F2>(
        &self,
        bar_width: f32,
        bar_height: f32,
        scroll_arrow_width: f32,
        measure_tab: F1,
        measure_segment: F2,
    ) -> TabBarLayout
    where
        F1: Fn(usize) -> TabMeasure,
        F2: Fn(usize) -> SegmentMeasure,
    {
        let mut visible_tabs: Vec<VisibleTab> = Vec::new();
        let mut visible_segments: Vec<VisibleSegment> = Vec::new();
        let mut hit_regions: Vec<(Rect, TabBarHit)> = Vec::new();

        if self.tabs.is_empty() && self.right_segments.is_empty() {
            return TabBarLayout {
                bar_width,
                bar_height,
                visible_tabs,
                visible_segments,
                scroll_left: None,
                scroll_right: None,
                hit_regions,
                resolved_scroll_offset: 0,
            };
        }

        // ── Right segments: render if they fit in the bar at all ──────
        let seg_widths: Vec<f32> = (0..self.right_segments.len())
            .map(|i| measure_segment(i).width)
            .collect();
        let total_seg_width: f32 = seg_widths.iter().sum();
        let segs_fit = !self.right_segments.is_empty() && total_seg_width <= bar_width;
        let right_area_width = if segs_fit { total_seg_width } else { 0.0 };

        // ── Tabs ───────────────────────────────────────────────────────
        let tab_measures: Vec<TabMeasure> = (0..self.tabs.len()).map(&measure_tab).collect();
        let total_tab_width: f32 = tab_measures.iter().map(|m| m.total_width).sum();
        let tab_area_no_scroll = (bar_width - right_area_width).max(0.0);
        let active_idx = self.tabs.iter().position(|t| t.is_active).unwrap_or(0);

        let (resolved_scroll_offset, tab_start_x, tab_end_x, needs_left, needs_right) =
            if self.tabs.is_empty() {
                (0usize, 0.0, tab_area_no_scroll, false, false)
            } else if total_tab_width <= tab_area_no_scroll + f32::EPSILON {
                // Everything fits — no scroll, no arrows.
                (0usize, 0.0, tab_area_no_scroll, false, false)
            } else if scroll_arrow_width <= 0.0 {
                // Scroll arrows disabled: the **caller** owns scroll
                // (e.g. vimcode's TUI computes a scroll offset via
                // `Engine::ensure_active_tab_visible` and stores it
                // on `bar.scroll_offset`). Honour that value so the
                // active tab actually appears, instead of clipping
                // from index 0 and dropping it. Clamp to a valid
                // index so callers can't push out-of-range values.
                let offset = self.scroll_offset.min(self.tabs.len().saturating_sub(1));
                (offset, 0.0, tab_area_no_scroll, false, false)
            } else {
                // Need scroll arrows. Reserve space for two; even if only one
                // ends up shown, the reserved width keeps `fit_active_scroll_offset`
                // honest.
                let tab_area_with_scroll = (tab_area_no_scroll - 2.0 * scroll_arrow_width).max(0.0);
                let avail_usize = tab_area_with_scroll as usize;
                let scroll_offset =
                    Self::fit_active_scroll_offset(active_idx, self.tabs.len(), avail_usize, |i| {
                        tab_measures[i].total_width.ceil() as usize
                    });
                let sum_from_offset: f32 = tab_measures[scroll_offset..]
                    .iter()
                    .map(|m| m.total_width)
                    .sum();
                let needs_right = sum_from_offset > tab_area_with_scroll + f32::EPSILON;
                let needs_left = scroll_offset > 0;
                let tab_start = scroll_arrow_width;
                (
                    scroll_offset,
                    tab_start,
                    tab_start + tab_area_with_scroll,
                    needs_left,
                    needs_right,
                )
            };

        // ── Left scroll arrow ──────────────────────────────────────────
        let scroll_left = if needs_left {
            let r = Rect::new(0.0, 0.0, scroll_arrow_width, bar_height);
            hit_regions.push((r, TabBarHit::ScrollLeft));
            Some(r)
        } else {
            None
        };

        // ── Visible tabs ───────────────────────────────────────────────
        let mut close_regions: Vec<(Rect, TabBarHit)> = Vec::new();
        let mut body_regions: Vec<(Rect, TabBarHit)> = Vec::new();
        let mut cursor_x = tab_start_x;

        for (i, tm) in tab_measures.iter().enumerate().skip(resolved_scroll_offset) {
            let tm = *tm;
            if cursor_x + tm.total_width > tab_end_x + f32::EPSILON {
                break;
            }
            let bounds = Rect::new(cursor_x, 0.0, tm.total_width, bar_height);
            let close_bounds =
                if tm.close_width > 0.0 && tm.close_width + tm.trailing_width <= tm.total_width {
                    Some(Rect::new(
                        cursor_x + tm.total_width - tm.trailing_width - tm.close_width,
                        0.0,
                        tm.close_width,
                        bar_height,
                    ))
                } else {
                    None
                };
            visible_tabs.push(VisibleTab {
                tab_idx: i,
                bounds,
                close_bounds,
            });
            if let Some(cb) = close_bounds {
                close_regions.push((cb, TabBarHit::TabClose(i)));
            }
            body_regions.push((bounds, TabBarHit::Tab(i)));
            cursor_x += tm.total_width;
        }

        // Close regions must come before body regions so `hit_test` returns
        // the more-specific close hit when the pointer is on the × glyph.
        hit_regions.extend(close_regions);
        hit_regions.extend(body_regions);

        // ── Right scroll arrow ─────────────────────────────────────────
        let scroll_right = if needs_right {
            let r = Rect::new(tab_end_x, 0.0, scroll_arrow_width, bar_height);
            hit_regions.push((r, TabBarHit::ScrollRight));
            Some(r)
        } else {
            None
        };

        // ── Right-aligned segments ─────────────────────────────────────
        if segs_fit {
            let mut seg_x = bar_width - right_area_width;
            for (i, seg) in self.right_segments.iter().enumerate() {
                let w = seg_widths[i];
                let bounds = Rect::new(seg_x, 0.0, w, bar_height);
                let clickable = seg.id.is_some();
                visible_segments.push(VisibleSegment {
                    segment_idx: i,
                    bounds,
                    clickable,
                });
                if let Some(id) = &seg.id {
                    hit_regions.push((bounds, TabBarHit::RightSegment(*id)));
                }
                seg_x += w;
            }
        }

        TabBarLayout {
            bar_width,
            bar_height,
            visible_tabs,
            visible_segments,
            scroll_left,
            scroll_right,
            hit_regions,
            resolved_scroll_offset,
        }
    }
}

/// Events a `TabBar` emits back to the app. Currently unused by vimcode
/// (click path goes through the engine's `TabBarClickTarget` enum), but
/// defined for plugin invariants §10 — plugin-declared tab bars will
/// consume events directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// Left/Right moved the keyboard-focused tab to this new index. The
    /// app is responsible for storing it and threading it back as
    /// [`TabChrome::focused_index`] (with focus-visible composition) on
    /// the next render — see [`TabBar::handle_focus_key`].
    FocusMoved { index: usize },
}

// Shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// Cairo / Core Graphics / Direct2D API — see the module doc's
// "`PaintSurface` migration status" section for scope and what still
// lives per-backend (bracket framing, and the macOS/GTK close-bounds
// convention difference this module sidesteps rather than resolves).
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{
        fit_tab_label, tab_icon_at, TabBar, TabBarGlyphs, TabBarLayout, TabChrome, TabFrame,
        TabIcon,
    };
    use crate::backend::PressableVisual;
    use crate::event::{Point, Rect};
    use crate::paint_surface::{PaintSurface, PathVerb};
    use crate::primitives::layout_metrics::{pixel, tab_close_hover_bg};
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::Color;

    /// Live, per-frame interaction state [`paint`] reads. [`super::TabItem`]
    /// carries no [`crate::types::WidgetId`] (every other
    /// `*_with_state` painter in this crate keys hover/press off an id),
    /// so hover/press/focus are reported by tab index instead — the same
    /// convention `gtk::tab_bar::draw_tab_bar_icons_with_chrome_and_glyphs`'s
    /// `hovered_tab`/`pressed_tab`/`hovered_close_tab` parameters already
    /// use. Bundled into one `Copy` struct, rather than four loose `Option<usize>`
    /// parameters, purely to keep [`paint`]'s own argument list short.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub(crate) struct TabBarPaintState {
        /// Index of the tab whose whole body (not just its close glyph)
        /// the pointer is over.
        pub(crate) hovered_tab: Option<usize>,
        /// Index of the tab whose whole body is currently pressed. Wins
        /// over `hovered_tab` when both name the same tab — the same
        /// pressed-over-hovered priority [`PressableVisual::resolve`]
        /// gives every other interactive painter in this crate.
        pub(crate) pressed_tab: Option<usize>,
        /// Index of the tab whose close glyph the pointer is over —
        /// tints just the glyph's own hover backdrop, independent of
        /// `hovered_tab`/`pressed_tab`.
        pub(crate) hovered_close_tab: Option<usize>,
        /// Index of the keyboard-focused tab, mirroring
        /// [`super::TabChrome::focused_index`].
        pub(crate) focused_index: Option<usize>,
    }

    /// Centre a chevron pointing left (`pointing_left: true`) or right
    /// inside `rect`, stroked in `color` — the scroll-arrow affordance
    /// for [`TabBarLayout::scroll_left`]/[`TabBarLayout::scroll_right`].
    /// No in-tree pixel backend paints a visible glyph here today (each
    /// reserves the hit-region geometry and clips instead — see
    /// `gtk::tab_bar`'s module doc), so this is new, not a port of an
    /// existing per-backend shape.
    fn paint_scroll_chevron(
        surface: &mut dyn PaintSurface,
        rect: Rect,
        pointing_left: bool,
        color: Color,
    ) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        let half = (rect.width.min(rect.height) * 0.25).max(1.0);
        let (tip_x, back_x) = if pointing_left {
            (cx - half, cx + half)
        } else {
            (cx + half, cx - half)
        };
        surface.surface_draw_path(
            &[
                PathVerb::MoveTo(Point::new(back_x, cy - half)),
                PathVerb::LineTo(Point::new(tip_x, cy)),
                PathVerb::LineTo(Point::new(back_x, cy + half)),
            ],
            color,
            1.5,
        );
    }

    /// Paint a [`TabBar`] at its caller-resolved `layout` onto `surface`.
    ///
    /// # Coordinate space
    ///
    /// `layout` (a [`TabBarLayout`]) is bar-relative, not surface-absolute
    /// — see [`TabBarLayout::tab_center`]'s own doc. This function paints
    /// every rect verbatim against that bar-relative origin. A caller
    /// whose bar does not start at the surface's own origin (e.g. a tab
    /// strip below a toolbar) must either translate `surface` to the
    /// bar's own top-left corner before calling (`PaintSurface` has no
    /// offset argument on any `surface_*` method, so this function cannot
    /// do that translation itself), or — if it instead bakes the offset
    /// into every `Rect` in `layout` directly, as `macos::tab_bar` does —
    /// pass that same `(x, y)` offset as `bar_x`/`bar_y` so the one rect
    /// this function derives itself, the background fill, lands in the
    /// same place.
    ///
    /// # Visual contract
    ///
    /// - **Background:** the full bar rect fills with `theme.tab_bar_bg`.
    /// - **Active tab:** an inset, rounded chip
    ///   (`theme.tab_active_bg`) rather than a full-height square block;
    ///   `bar.active_accent`, when set, paints a thin top-edge accent
    ///   line on the chip's own inset top edge. The chip (and, below, the
    ///   inactive-tab fill and the focus ring) spans the tab's full
    ///   `vt.bounds` width, including [`pixel::TAB_OUTER_GAP`]'s share of
    ///   it — [`TabBarLayout`] carries no per-tab gap to narrow by, so
    ///   (unlike `gtk::tab_bar`, which subtracts it) these come out up to
    ///   1px wider than the GTK reference.
    /// - **Whole-tab hover/pressed:** tints the tab's background toward
    ///   `theme.list_hover_bg`/`theme.list_pressed_bg` — pressed wins —
    ///   independent of the close glyph's own hover state below.
    /// - **Label:** ellipsis-fitted via [`fit_tab_label`] into the space
    ///   left after the leading icon (if any) and the close-button
    ///   region (if any); italic when [`super::TabItem::is_preview`].
    ///   The leading icon and label both start at
    ///   [`pixel::TAB_PAD_COMPACT`] from the tab's left edge when
    ///   [`TabBar::compact`] is set, [`Style::tab_padding_x`] otherwise —
    ///   the same compact/non-compact switch every backend's own
    ///   `pixel_tab_bar_layout` caller resolves to measure `vt.bounds`
    ///   in the first place.
    /// - **Icon:** [`tab_icon_at`]'s glyph, painted in its own `color`
    ///   before the label, independent of the tab's fg colour.
    /// - **Close/dirty glyph:** centred inside
    ///   [`super::VisibleTab::close_bounds`] (see the module doc for why this
    ///   is a deliberate, convention-agnostic centring rather than a
    ///   fixed leading-gap offset); dirty tabs paint the dirty glyph
    ///   unless the close glyph is hovered, in which case the close
    ///   glyph (and its rounded hover backdrop) takes over.
    /// - **Focus-visible ring:** a square stroke flush with
    ///   `state.focused_index`'s own slot bounds, in
    ///   `theme.focus_ring_color()`.
    /// - **Scroll arrows:** a chevron centred in
    ///   `layout.scroll_left`/`layout.scroll_right`, when present.
    ///
    /// `icons` follows [`tab_icon_at`]'s own convention: a slice shorter
    /// than `bar.tabs` is read as "no icon" for the missing tail, and
    /// `&[]` is always legal. `glyphs` overrides the close/dirty glyph
    /// (see [`TabBarGlyphs`]); `nerd_fonts_enabled` resolves which half
    /// of an override [`crate::types::Icon`] paints, mirroring
    /// [`crate::Backend::nerd_fonts_enabled`]. `chrome.active_frame` ==
    /// [`TabFrame::Brackets`] wraps the active tab's label (and close/dirty
    /// glyph, when present) in `[` … `]`, painted in the tab's own `fg`
    /// colour; [`TabBarLayout`]'s own `vt.bounds`/`close_bounds` already
    /// reserve the bracket glyphs' width (see
    /// [`crate::primitives::layout_metrics::pixel_tab_bar_layout`]'s doc),
    /// so this function only has to paint them, not re-measure the slot.
    /// `bar_x`/`bar_y` are the top-left this function's own background
    /// fill starts at — `(0.0, 0.0)` for a caller that translates
    /// `surface` to the bar's origin, or the same offset already baked
    /// into `layout`'s own rects otherwise (see "Coordinate space"
    /// above).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        bar: &TabBar,
        layout: &TabBarLayout,
        icons: &[Option<TabIcon>],
        glyphs: Option<&TabBarGlyphs>,
        nerd_fonts_enabled: bool,
        chrome: &TabChrome,
        state: TabBarPaintState,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        bar_x: f32,
        bar_y: f32,
    ) {
        if layout.bar_width <= 0.0 || layout.bar_height <= 0.0 {
            return;
        }

        let brackets = matches!(chrome.active_frame, TabFrame::Brackets);

        surface.surface_fill_rect(
            Rect::new(bar_x, bar_y, layout.bar_width, layout.bar_height),
            theme.tab_bar_bg,
        );

        let line_height = surface.surface_line_height();
        let close_glyph_default = crate::codicon::CLOSE.to_string();
        let dirty_glyph_default = crate::codicon::DIRTY.to_string();
        let close_glyph: &str = glyphs
            .and_then(TabBarGlyphs::close_glyph)
            .map(|icon| icon.resolve(nerd_fonts_enabled))
            .unwrap_or(&close_glyph_default);
        let dirty_glyph: &str = glyphs
            .and_then(TabBarGlyphs::dirty_glyph)
            .map(|icon| icon.resolve(nerd_fonts_enabled))
            .unwrap_or(&dirty_glyph_default);

        for vt in &layout.visible_tabs {
            let tab = &bar.tabs[vt.tab_idx];
            let bounds = vt.bounds;
            if bounds.width <= 0.0 || bounds.height <= 0.0 {
                continue;
            }

            let is_tab_pressed = state.pressed_tab == Some(vt.tab_idx);
            let is_tab_hovered = state.hovered_tab == Some(vt.tab_idx);
            let tint = |base: Color| -> Color {
                match PressableVisual::resolve(is_tab_pressed, false, is_tab_hovered) {
                    PressableVisual::PressedOrActive => theme.list_pressed_bg(base),
                    PressableVisual::Hovered => theme.list_hover_bg(base),
                    PressableVisual::Resting => base,
                }
            };

            let text_y = bounds.y + ((bounds.height - line_height) / 2.0).max(0.0);

            let tab_base_bg = if tab.is_active {
                tint(theme.tab_active_bg)
            } else {
                tint(theme.tab_bar_bg)
            };

            if tab.is_active {
                // Shrinks the inset (down to 0 at the limit) on a row only
                // just tall enough for `line_height` — a fixed
                // `TAB_CHIP_INSET_Y` on both edges there would leave the
                // chip shorter than the label, letting a descender ink
                // past its bottom edge.
                let max_chip_inset_y = ((bounds.height - line_height) / 2.0).max(0.0);
                let chip_inset_y = pixel::TAB_CHIP_INSET_Y.min(max_chip_inset_y);
                let chip = Rect::new(
                    bounds.x,
                    bounds.y + chip_inset_y,
                    bounds.width,
                    (bounds.height - 2.0 * chip_inset_y).max(0.0),
                );
                surface.surface_fill_rounded_rect(chip, pixel::TAB_CHIP_RADIUS, tab_base_bg);
                if let Some(accent) = bar.active_accent {
                    surface.surface_fill_rect(
                        Rect::new(chip.x, chip.y, chip.width, pixel::TAB_ACCENT_THICKNESS),
                        accent,
                    );
                }
            } else {
                surface.surface_fill_rect(bounds, tab_base_bg);
            }

            let fg = match (tab.is_active, tab.is_preview) {
                (true, true) => theme.tab_preview_active_fg,
                (true, false) => theme.tab_active_fg,
                (false, true) => theme.tab_preview_inactive_fg,
                (false, false) => theme.tab_inactive_fg,
            };

            // Matches the compact/non-compact `tab_pad` resolution every
            // caller of `pixel_tab_bar_layout` already uses to measure
            // `vt.bounds`/`vt.close_bounds` — reading `style.tab_padding_x`
            // unconditionally here would start content past the padding
            // the layout actually reserved whenever `bar.compact` is set.
            let tab_pad = if bar.compact {
                pixel::TAB_PAD_COMPACT
            } else {
                style.tab_padding_x
            };
            let mut cursor_x = bounds.x + tab_pad;

            // Opening bracket, when this tab is active and `chrome`
            // requests `TabFrame::Brackets` — painted in the tab's own
            // `fg` colour, immediately before the icon/label content it
            // encloses. `vt.bounds`/`vt.close_bounds` already reserved
            // this glyph's width (and the closing one below), so painting
            // it just advances `cursor_x` by its measured width.
            let is_bracket = brackets && tab.is_active;
            if is_bracket {
                let (bracket_w, _) = surface.surface_measure_text("[");
                surface.surface_draw_text_run(
                    Rect::new(cursor_x, text_y, bracket_w, line_height),
                    "[",
                    fg,
                );
                cursor_x += bracket_w;
            }

            if let Some(icon) = tab_icon_at(icons, vt.tab_idx) {
                let (icon_w, _) = surface.surface_measure_text(&icon.glyph);
                surface.surface_draw_icon_glyph(
                    Rect::new(cursor_x, text_y, icon_w, line_height),
                    &icon.glyph,
                    icon.color,
                );
                cursor_x += icon_w + pixel::TAB_ICON_GAP;
            }

            let label_right_edge = match vt.close_bounds {
                Some(cb) => cb.x,
                None => bounds.x + bounds.width - tab_pad,
            };
            let label_budget = (label_right_edge - cursor_x).max(0.0);
            let label = fit_tab_label(&tab.label, label_budget, 0.0, 0.0, |s| {
                surface.surface_measure_text(s).0
            });
            let (label_w, _) = surface.surface_measure_text(&label);
            surface.surface_draw_text_run_styled(
                Rect::new(cursor_x, text_y, label_w.min(label_budget), line_height),
                &label,
                fg,
                false,
                tab.is_preview,
                false,
                1.0,
            );

            // Closing bracket for a bracket-framed tab with no close
            // button — painted right after the label. When a close
            // button is present, the closing bracket instead paints
            // right after that glyph, inside the block below, since its
            // position depends on the glyph's own measured width.
            if is_bracket && vt.close_bounds.is_none() {
                let (bracket_w, _) = surface.surface_measure_text("]");
                surface.surface_draw_text_run(
                    Rect::new(
                        cursor_x + label_w.min(label_budget),
                        text_y,
                        bracket_w,
                        line_height,
                    ),
                    "]",
                    fg,
                );
            }

            if let Some(cb) = vt.close_bounds {
                let is_close_hovered = state.hovered_close_tab == Some(vt.tab_idx);
                if is_close_hovered {
                    let backdrop = Rect::new(
                        cb.x + pixel::TAB_CLOSE_HOVER_PAD,
                        text_y + pixel::TAB_CLOSE_HOVER_PAD,
                        (cb.width - 2.0 * pixel::TAB_CLOSE_HOVER_PAD).max(0.0),
                        (line_height - 2.0 * pixel::TAB_CLOSE_HOVER_PAD).max(0.0),
                    );
                    surface.surface_fill_rounded_rect(
                        backdrop,
                        pixel::TAB_CLOSE_HOVER_RADIUS,
                        tab_close_hover_bg(tab_base_bg, theme.foreground),
                    );
                }

                let glyph = if tab.is_dirty && !is_close_hovered {
                    dirty_glyph
                } else {
                    close_glyph
                };
                let close_fg = if tab.is_dirty || is_close_hovered {
                    theme.foreground
                } else if tab.is_active {
                    theme.tab_inactive_fg
                } else {
                    theme.separator
                };
                let (glyph_w, _) = surface.surface_measure_text(glyph);
                let glyph_x = cb.x + ((cb.width - glyph_w) / 2.0).max(0.0);
                surface.surface_draw_icon_glyph(
                    Rect::new(glyph_x, text_y, glyph_w, line_height),
                    glyph,
                    close_fg,
                );

                // Closing bracket, right after the close/dirty glyph —
                // `vt.bounds`' own trailing width already reserved room
                // for it beyond `cb`.
                if is_bracket {
                    let (bracket_w, _) = surface.surface_measure_text("]");
                    surface.surface_draw_text_run(
                        Rect::new(glyph_x + glyph_w, text_y, bracket_w, line_height),
                        "]",
                        fg,
                    );
                }
            }
        }

        if let Some(focus_idx) = state.focused_index {
            if let Some(vt) = layout
                .visible_tabs
                .iter()
                .find(|vt| vt.tab_idx == focus_idx)
            {
                let ring_width = style.focus_ring_width.max(1.0);
                let inset = ring_width / 2.0;
                surface.surface_stroke_rect(
                    Rect::new(
                        vt.bounds.x + inset,
                        vt.bounds.y + inset,
                        (vt.bounds.width - ring_width).max(0.0),
                        (vt.bounds.height - ring_width).max(0.0),
                    ),
                    theme.focus_ring_color(),
                    ring_width,
                );
            }
        }

        for vs in &layout.visible_segments {
            let seg = &bar.right_segments[vs.segment_idx];
            let fg = if seg.is_active {
                theme.tab_active_fg
            } else {
                theme.tab_inactive_fg
            };
            let text_y = vs.bounds.y + ((vs.bounds.height - line_height) / 2.0).max(0.0);
            surface.surface_draw_text_run(
                Rect::new(vs.bounds.x, text_y, vs.bounds.width, line_height),
                &seg.text,
                fg,
            );
        }

        if let Some(rect) = layout.scroll_left {
            paint_scroll_chevron(surface, rect, true, theme.tab_inactive_fg);
        }
        if let Some(rect) = layout.scroll_right {
            paint_scroll_chevron(surface, rect, false, theme.tab_inactive_fg);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::tab_bar::{SegmentMeasure, TabItem, TabMeasure};
        use crate::types::WidgetId;
        use crate::Image;

        /// Records every paint call this test suite inspects — enough to
        /// assert each state's own fill/stroke/text-run colour without a
        /// Cairo/Core Graphics/Direct2D host.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            rounded_fills: Vec<(Rect, f32, Color)>,
            strokes: Vec<(Rect, Color, f32)>,
            text_runs: Vec<(Rect, String, Color)>,
            paths: Vec<(Vec<PathVerb>, Color, f32)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(300.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                14.0
            }
            fn surface_char_width(&self) -> f32 {
                7.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 7.0, 12.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
                self.rounded_fills.push((rect, radius, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _sw: f32) {}
            fn surface_draw_path(&mut self, verbs: &[PathVerb], color: Color, stroke_width: f32) {
                self.paths.push((verbs.to_vec(), color, stroke_width));
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn tab(label: &str) -> TabItem {
            TabItem {
                label: label.to_string(),
                is_active: false,
                is_dirty: false,
                is_preview: false,
                is_closable: true,
            }
        }

        fn bar(tabs: Vec<TabItem>) -> TabBar {
            TabBar {
                id: WidgetId::new("tabbar:test"),
                tabs,
                scroll_offset: 0,
                right_segments: vec![],
                active_accent: None,
                show_tab_close: true,
                compact: false,
            }
        }

        /// Fixed-width measurer standing in for a real Pango/DirectWrite/
        /// Core Text measurement: each tab is `tab_w` surface-native
        /// units wide with a 10-unit close region.
        fn layout_for(
            bar: &TabBar,
            bar_width: f32,
            scroll_arrow_width: f32,
            tab_w: f32,
        ) -> TabBarLayout {
            bar.layout(
                bar_width,
                20.0,
                scroll_arrow_width,
                |_| TabMeasure::new(tab_w, if bar.show_tab_close { 10.0 } else { 0.0 }),
                |_| SegmentMeasure::new(0.0),
            )
        }

        #[test]
        fn active_tab_fills_a_rounded_chip_in_tab_active_bg() {
            let mut b = bar(vec![tab("main.rs")]);
            b.tabs[0].is_active = true;
            let layout = layout_for(&b, 100.0, 0.0, 60.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            assert!(
                surface
                    .rounded_fills
                    .iter()
                    .any(|(_, _, c)| *c == theme.tab_active_bg),
                "active tab must fill a rounded chip in tab_active_bg; rounded_fills were {:?}",
                surface.rounded_fills,
            );
        }

        /// A hovered (not pressed) inactive tab tints its background
        /// toward `theme.list_hover_bg`, not the plain resting
        /// `tab_bar_bg`.
        #[test]
        fn hovered_tab_tints_toward_list_hover_bg() {
            let b = bar(vec![tab("a"), tab("b")]);
            let layout = layout_for(&b, 100.0, 0.0, 20.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let state = TabBarPaintState {
                hovered_tab: Some(1),
                ..Default::default()
            };

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                state,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            let expected = theme.list_hover_bg(theme.tab_bar_bg);
            assert!(
                surface.fills.iter().any(|(_, c)| *c == expected),
                "hovered_tab must tint toward list_hover_bg; fills were {:?}",
                surface.fills,
            );
        }

        /// `pressed_tab` tints deeper than `hovered_tab` and wins when
        /// both name the same tab — the same pressed-over-hovered
        /// priority every other `_with_state` painter in this crate
        /// gives.
        #[test]
        fn pressed_tab_tints_deeper_than_hovered_and_wins() {
            let b = bar(vec![tab("a"), tab("b")]);
            let layout = layout_for(&b, 100.0, 0.0, 20.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let state = TabBarPaintState {
                hovered_tab: Some(1),
                pressed_tab: Some(1),
                ..Default::default()
            };

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                state,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            let expected = theme.list_pressed_bg(theme.tab_bar_bg);
            assert!(
                surface.fills.iter().any(|(_, c)| *c == expected),
                "pressed_tab must win over hovered_tab and tint via list_pressed_bg; \
                 fills were {:?}",
                surface.fills,
            );
        }

        /// `state.focused_index` strokes a square ring, flush with that
        /// tab's own slot bounds, in `theme.focus_ring_color()`.
        #[test]
        fn focused_index_strokes_a_ring_in_the_focus_ring_color() {
            let b = bar(vec![tab("a"), tab("b")]);
            let layout = layout_for(&b, 100.0, 0.0, 20.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let state = TabBarPaintState {
                focused_index: Some(1),
                ..Default::default()
            };

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                state,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            assert!(
                surface
                    .strokes
                    .iter()
                    .any(|(_, c, _)| *c == theme.focus_ring_color()),
                "focused_index must stroke a ring in the focus-ring colour; \
                 strokes were {:?}",
                surface.strokes,
            );
        }

        /// A dirty tab paints the dirty glyph (not the close glyph) as
        /// an icon-glyph text run, when its close glyph isn't hovered.
        #[test]
        fn dirty_tab_paints_the_dirty_glyph_not_the_close_glyph() {
            let mut b = bar(vec![tab("a")]);
            b.tabs[0].is_dirty = true;
            let layout = layout_for(&b, 100.0, 0.0, 60.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            let dirty = crate::codicon::DIRTY.to_string();
            let close = crate::codicon::CLOSE.to_string();
            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == &dirty),
                "a dirty tab must paint the dirty glyph; text runs were {:?}",
                surface.text_runs,
            );
            assert!(
                !surface.text_runs.iter().any(|(_, t, _)| t == &close),
                "a dirty tab's close glyph must not also paint; text runs were {:?}",
                surface.text_runs,
            );
        }

        /// Hovering a dirty tab's close glyph overrides the dirty glyph
        /// back to the close glyph and paints a rounded hover backdrop
        /// behind it.
        #[test]
        fn hovered_close_glyph_overrides_dirty_and_paints_a_rounded_backdrop() {
            let mut b = bar(vec![tab("a")]);
            b.tabs[0].is_dirty = true;
            let layout = layout_for(&b, 100.0, 0.0, 60.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let state = TabBarPaintState {
                hovered_close_tab: Some(0),
                ..Default::default()
            };

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                state,
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            let close = crate::codicon::CLOSE.to_string();
            let dirty = crate::codicon::DIRTY.to_string();
            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == &close),
                "a hovered close glyph must paint the close glyph, not the dirty \
                 glyph; text runs were {:?}",
                surface.text_runs,
            );
            assert!(!surface.text_runs.iter().any(|(_, t, _)| t == &dirty));
            let expected_backdrop = tab_close_hover_bg(theme.tab_bar_bg, theme.foreground);
            assert!(
                surface
                    .rounded_fills
                    .iter()
                    .any(|(_, _, c)| *c == expected_backdrop),
                "a hovered close glyph must paint a rounded backdrop blended \
                 from the tab's own resting background toward foreground; \
                 rounded_fills were {:?}",
                surface.rounded_fills,
            );
        }

        /// `chrome.active_frame == TabFrame::Brackets` wraps the active
        /// tab's label and close glyph in `[` … `]`, each painted as its
        /// own text run in the tab's own `fg` colour. The inactive tab
        /// gets no bracket glyphs at all.
        #[test]
        fn bracket_chrome_paints_opening_and_closing_glyphs_around_an_active_tab_with_close() {
            let mut b = bar(vec![tab("a"), tab("b")]);
            b.tabs[0].is_active = true;
            let layout = layout_for(&b, 200.0, 0.0, 60.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let chrome = TabChrome::new(TabFrame::Brackets);

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &chrome,
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            let open_brackets = surface
                .text_runs
                .iter()
                .filter(|(_, t, c)| t == "[" && *c == theme.tab_active_fg)
                .count();
            let close_brackets = surface
                .text_runs
                .iter()
                .filter(|(_, t, c)| t == "]" && *c == theme.tab_active_fg)
                .count();
            assert_eq!(
                open_brackets, 1,
                "the active tab must paint exactly one opening bracket; \
                 text runs were {:?}",
                surface.text_runs,
            );
            assert_eq!(
                close_brackets, 1,
                "the active tab must paint exactly one closing bracket; \
                 text runs were {:?}",
                surface.text_runs,
            );
        }

        /// Without `TabFrame::Brackets`, no tab paints `[`/`]` glyphs —
        /// the default chrome used by every other test in this module
        /// must stay bracket-free.
        #[test]
        fn plain_chrome_paints_no_bracket_glyphs() {
            let mut b = bar(vec![tab("a")]);
            b.tabs[0].is_active = true;
            let layout = layout_for(&b, 100.0, 0.0, 60.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            assert!(
                !surface
                    .text_runs
                    .iter()
                    .any(|(_, t, _)| t == "[" || t == "]"),
                "TabFrame::None must not paint any bracket glyph; text runs \
                 were {:?}",
                surface.text_runs,
            );
        }

        /// A bracket-framed tab with no close button at all still paints
        /// its closing bracket, right after the label rather than after a
        /// close glyph that doesn't exist.
        #[test]
        fn bracket_chrome_paints_closing_glyph_after_the_label_when_no_close_button() {
            let mut b = bar(vec![tab("a")]);
            b.tabs[0].is_active = true;
            b.show_tab_close = false;
            let layout = layout_for(&b, 100.0, 0.0, 60.0);
            assert!(
                layout.visible_tabs[0].close_bounds.is_none(),
                "a bar with no close button must resolve no close_bounds"
            );
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let chrome = TabChrome::new(TabFrame::Brackets);

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &chrome,
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == "]"),
                "a bracket-framed, non-closable tab must still paint its \
                 closing bracket; text runs were {:?}",
                surface.text_runs,
            );
        }

        /// When the strip overflows, `layout.scroll_left`/`scroll_right`
        /// each paint a chevron via `surface_draw_path` — the new
        /// scroll-arrow affordance no in-tree pixel backend painted
        /// before this shared painter existed.
        #[test]
        fn overflowing_strip_paints_both_scroll_chevrons() {
            let mut b = bar((0..10).map(|i| tab(&format!("t{i}"))).collect());
            b.tabs[5].is_active = true;
            let layout = layout_for(&b, 30.0, 2.0, 10.0);
            assert!(layout.scroll_left.is_some(), "expected a left scroll arrow");
            assert!(
                layout.scroll_right.is_some(),
                "expected a right scroll arrow"
            );
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            assert_eq!(
                surface.paths.len(),
                2,
                "both scroll arrows must paint a chevron path; paths were {:?}",
                surface.paths,
            );
        }

        /// A zero-size or NaN-sized layout (e.g. a collapsed split with
        /// no width, or a backend that fed through an unresolved
        /// measurement) paints nothing rather than panicking.
        #[test]
        fn zero_size_layout_paints_nothing() {
            let b = bar(vec![tab("a")]);
            let zero_layout = TabBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_tabs: vec![],
                visible_segments: vec![],
                scroll_left: None,
                scroll_right: None,
                hit_regions: vec![],
                resolved_scroll_offset: 0,
            };
            let nan_layout = TabBarLayout {
                bar_width: f32::NAN,
                ..zero_layout.clone()
            };
            let theme = Theme::default();

            for layout in [&zero_layout, &nan_layout] {
                let mut surface = RecordingSurface::default();
                paint(
                    &b,
                    layout,
                    &[],
                    None,
                    true,
                    &TabChrome::default(),
                    TabBarPaintState::default(),
                    &mut surface,
                    &theme,
                    &Style::default(),
                    0.0,
                    0.0,
                );

                assert!(surface.fills.is_empty());
                assert!(surface.rounded_fills.is_empty());
            }
        }

        /// A leading icon (from [`tab_icon_at`]) paints at the tab's own
        /// `tab_padding_x` offset, in the icon's own `color` —
        /// independent of the label's fg colour.
        #[test]
        fn leading_icon_paints_at_tab_padding_x_in_its_own_color() {
            let b = bar(vec![tab("main.rs")]);
            let layout = layout_for(&b, 100.0, 0.0, 60.0);
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();
            let icon_color = Color::rgba(10, 20, 30, 255);
            let icons = vec![Some(TabIcon {
                glyph: "I".to_string(),
                color: icon_color,
            })];

            paint(
                &b,
                &layout,
                &icons,
                None,
                true,
                &TabChrome::default(),
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &style,
                0.0,
                0.0,
            );

            let tab_x = layout.visible_tabs[0].bounds.x;
            assert!(
                surface.text_runs.iter().any(|(r, t, c)| t == "I"
                    && *c == icon_color
                    && r.x == tab_x + style.tab_padding_x),
                "the leading icon must paint at tab_padding_x in its own \
                 colour; text runs were {:?}",
                surface.text_runs,
            );
        }

        /// The label is ellipsis-fitted so it never overlaps
        /// [`super::VisibleTab::close_bounds`] — the label's painted
        /// rect ends at or before the close region's left edge.
        #[test]
        fn label_is_ellipsis_fitted_against_close_bounds() {
            let mut b = bar(vec![tab("a-very-long-tab-label-indeed")]);
            b.tabs[0].is_active = true;
            let layout = layout_for(&b, 100.0, 0.0, 40.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            let cb = layout.visible_tabs[0]
                .close_bounds
                .expect("tab is closable");
            let label_run = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t.starts_with('a'))
                .expect("label must paint a text run");
            assert!(
                label_run.0.x + label_run.0.width <= cb.x + 0.01,
                "label rect {:?} must not extend past close_bounds.x ({})",
                label_run.0,
                cb.x,
            );
            assert!(
                label_run.1.ends_with('…') || label_run.1.contains('…'),
                "a label this long against a 40-wide tab must be \
                 ellipsis-fitted; painted label was {:?}",
                label_run.1,
            );
        }

        /// `bar.active_accent`, when set, paints a
        /// [`pixel::TAB_ACCENT_THICKNESS`]-tall line on the active tab
        /// chip's own inset top edge, in the accent colour.
        #[test]
        fn active_accent_paints_a_top_edge_line_on_the_chip() {
            let mut b = bar(vec![tab("a")]);
            b.tabs[0].is_active = true;
            let accent = Color::rgba(200, 50, 50, 255);
            b.active_accent = Some(accent);
            let layout = layout_for(&b, 100.0, 0.0, 60.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            // The bar's 20.0 row height against `RecordingSurface`'s
            // 14.0 stub line height clamps the chip inset below the
            // fixed `TAB_CHIP_INSET_Y`, same as production's adaptive
            // inset in `paint`.
            let chip_inset_y = pixel::TAB_CHIP_INSET_Y.min(((20.0 - 14.0) / 2.0_f32).max(0.0));
            let chip_y = layout.visible_tabs[0].bounds.y + chip_inset_y;
            assert!(
                surface.fills.iter().any(|(r, c)| {
                    *c == accent && r.y == chip_y && r.height == pixel::TAB_ACCENT_THICKNESS
                }),
                "active_accent must paint a thin line on the chip's own \
                 inset top edge; fills were {:?}",
                surface.fills,
            );
        }

        /// The close glyph is painted centred inside
        /// [`super::VisibleTab::close_bounds`] — the convention-agnostic
        /// centring the module doc describes, not a fixed leading-gap
        /// offset.
        #[test]
        fn close_glyph_is_centred_inside_close_bounds() {
            let b = bar(vec![tab("a")]);
            let layout = layout_for(&b, 100.0, 0.0, 60.0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
                0.0,
                0.0,
            );

            let cb = layout.visible_tabs[0]
                .close_bounds
                .expect("tab is closable");
            let close = crate::codicon::CLOSE.to_string();
            let (glyph_w, _) = surface.surface_measure_text(&close);
            let expected_x = cb.x + ((cb.width - glyph_w) / 2.0).max(0.0);
            assert!(
                surface
                    .text_runs
                    .iter()
                    .any(|(r, t, _)| t == &close && (r.x - expected_x).abs() < 0.01),
                "the close glyph must be centred inside close_bounds \
                 ({:?}); text runs were {:?}",
                cb,
                surface.text_runs,
            );
        }

        /// [`TabBar::compact`] narrows the leading content offset to
        /// [`pixel::TAB_PAD_COMPACT`], matching the padding every
        /// `pixel_tab_bar_layout` caller already measured `vt.bounds`
        /// with for a compact bar — not `style.tab_padding_x`, which
        /// would start the icon/label past the space the layout
        /// actually reserved.
        #[test]
        fn compact_bar_starts_content_at_the_compact_pad_not_style_padding() {
            let mut b = bar(vec![tab("a")]);
            b.compact = true;
            let layout = layout_for(&b, 100.0, 0.0, 60.0);
            let theme = Theme::default();
            // A non-default, wider `tab_padding_x` — if the painter read
            // this unconditionally (ignoring `bar.compact`), content
            // would start well past `pixel::TAB_PAD_COMPACT`.
            let style = Style::default().with_tab_padding_x(20.0);
            let mut surface = RecordingSurface::default();

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                &TabChrome::default(),
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &style,
                0.0,
                0.0,
            );

            let tab_x = layout.visible_tabs[0].bounds.x;
            let label_run = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == "a")
                .expect("label must paint a text run");
            assert!(
                (label_run.0.x - (tab_x + pixel::TAB_PAD_COMPACT)).abs() < 0.01,
                "a compact bar must start the label at TAB_PAD_COMPACT \
                 ({}), not style.tab_padding_x (20.0); label rect was {:?}",
                pixel::TAB_PAD_COMPACT,
                label_run.0,
            );
        }
    }
}

// Shared cell-grid paint implementation — the whole-cell counterpart to
// [`native_surface_paint`], which paints the same primitive onto a pixel
// canvas through [`crate::paint_surface::PaintSurface`]. Both live in this
// module so a change to one is reviewed next to the other instead of
// drifting in a backend directory.
//
// Takes `&mut ratatui::buffer::Buffer` directly rather than a
// quadraui-local cell-surface abstraction, for the reason
// [`crate::primitive::Primitive::paint_cells`] documents at length: a
// terminal rasteriser picks individual glyphs (`×`, `│`, `[`, `]`) and
// reckons in `char_cell_width` columns, neither of which a pixel-shaped
// `PaintSurface` verb can express, so the cell path gets its own entry
// point instead of a lying `PaintSurface` impl.
//
// `pub` rather than `pub(crate)` because [`paint`] *is*
// `crate::tui::draw_tab_bar_icons_with_chrome_and_glyphs` — the `tui`
// module re-exports it under that name rather than wrapping it in a
// forwarder that would have to be kept in step with this signature.
// `#[doc(hidden)]` keeps the painter itself off the documented surface,
// the same way every `crate::tui::draw_*` rasteriser re-export is hidden.
#[cfg(feature = "tui")]
#[doc(hidden)]
pub mod cell_paint {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    #[allow(deprecated)]
    use super::TabBarHits;
    use super::{
        fit_tab_label, tab_icon_at, tab_icon_cols, TabBar, TabBarGlyphs, TabBarLayout, TabChrome,
        TabFrame, TabIcon, TAB_CLOSE_CHAR, TAB_SEPARATOR_CHAR,
    };
    use crate::text_util::char_cell_width;
    use crate::theme::Theme;
    use crate::tui::{
        ratatui_color, set_cell, set_cell_styled, set_cell_wide, set_cell_wide_styled,
    };
    use crate::types::Icon;

    /// Resolve a [`TabBarGlyphs`] slot (the close or dirty override) to the
    /// single `char` this rasteriser paints, falling back to `default` when
    /// the slot is unset. Shared so the close-glyph and dirty-glyph
    /// branches below don't repeat the same `Option` chain.
    fn tab_glyph_char(icon: Option<&Icon>, nerd_fonts_enabled: bool, default: char) -> char {
        icon.map(|icon| icon.resolve(nerd_fonts_enabled))
            .and_then(|s| s.chars().next())
            .unwrap_or(default)
    }

    /// Paint `bar` into `area` on `buf` from the already-resolved `layout`,
    /// and return its hit geometry.
    ///
    /// Per D6 this consumes a pre-computed [`TabBarLayout`] (built by the
    /// caller via [`TabBar::layout`] with its native cell-width measurer)
    /// and paints the resolved `visible_tabs` + `visible_segments`
    /// verbatim — it never recomputes widths. The returned
    /// `TabBarHits::available_cols` carries the tab-content width in cells
    /// so the caller can adjust the scroll offset for the next frame.
    ///
    /// # Visual contract
    ///
    /// - **Bar background:** filled with `theme.tab_bar_bg`.
    /// - **Active tab:** `tab_active_fg` + `tab_active_bg`. When
    ///   [`TabBar::active_accent`] is `Some`, the filename portion (chars
    ///   after the last `": "` in `tab.label`) gets a
    ///   [`Modifier::UNDERLINED`] with that accent colour.
    /// - **Dirty tab:** the close cell shows `●` in `theme.foreground`
    ///   instead of `×`.
    /// - **Preview tab:** `*_preview_*_fg` and [`Modifier::ITALIC`]; combines
    ///   with the underline accent when active.
    /// - **Right segments:** painted in `tab_inactive_fg` (or
    ///   `seg.is_active`'s `tab_active_fg`). Double-width glyphs (per
    ///   `unicode-width`) use the wide-cell helpers.
    /// - **Tab labels:** measured and painted in display columns, not
    ///   `char`s — a double-width glyph (per `unicode-width`) occupies two
    ///   columns and advances the cursor by 2, mirroring the right-segment
    ///   loop. The measure side (`TuiBackend::draw_tab_bar` /
    ///   `tab_bar_layout` in `crate::tui::backend`) must agree, or a tab is
    ///   measured narrower than it paints.
    /// - **Icons:** `icons` is a sidecar slice parallel to `bar.tabs` (see
    ///   [`crate::Backend::draw_tab_bar_icons`]), resolved through
    ///   [`tab_icon_at`] / [`tab_icon_cols`] so a short slice means "no
    ///   icon" rather than a panic. A decorated tab paints its glyph at the
    ///   tab's leading edge in [`TabIcon::color`] — independent of the tab's
    ///   active/inactive foreground, so the icon keeps its identity colour
    ///   on an inactive tab — followed by a 1-column gap before the label.
    ///   That reservation is [`tab_icon_cols`], exactly what the measure
    ///   side adds to the tab's measured width, so paint and measurement
    ///   cannot drift. Passing `&[]` paints no icons at all.
    /// - **Chrome:** when `chrome.active_frame` is [`TabFrame::Brackets`],
    ///   the active tab's full content — icon, label, and close glyph — is
    ///   enclosed in literal `[` / `]` glyphs; the caller's measurer must
    ///   have already reserved those columns in `layout`. See
    ///   [`TabChrome::focused_index`] for the keyboard-focus reverse-video
    ///   treatment.
    /// - **Glyph overrides:** `glyphs: None` paints the default `×`/`●`
    ///   marks. `Some` overrides only the marks it sets;
    ///   `nerd_fonts_enabled` selects which half of each override [`Icon`]
    ///   — `glyph` or `fallback` — is resolved (see
    ///   [`crate::Backend::nerd_fonts_enabled`]), and is ignored when
    ///   `glyphs` does not set the mark being drawn.
    ///
    /// Downstream: vimcode's `tab_hit_width` (`src/render.rs`) is
    /// deliberately pinned on a `.chars().count()` measure because it must
    /// agree with what this rasteriser paints; its own doc comment names a
    /// `display_width` measure here as the unblocker for switching. coord-tui
    /// consumes the same tab bar and should be checked for the equivalent
    /// assumption.
    #[allow(deprecated)] // returns the deprecated `TabBarHits`
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        buf: &mut Buffer,
        area: Rect,
        bar: &TabBar,
        icons: &[Option<TabIcon>],
        chrome: &TabChrome,
        layout: &TabBarLayout,
        theme: &Theme,
        glyphs: Option<&TabBarGlyphs>,
        nerd_fonts_enabled: bool,
    ) -> TabBarHits {
        if area.width == 0 || area.height == 0 {
            return TabBarHits::default();
        }

        let bar_bg = ratatui_color(theme.tab_bar_bg);
        let btn_fg = ratatui_color(theme.tab_inactive_fg);
        let btn_fg_active = ratatui_color(theme.tab_active_fg);
        let foreground = ratatui_color(theme.foreground);

        // Fill bar background.
        for x in area.x..area.x + area.width {
            set_cell(buf, x, area.y, ' ', bar_bg, bar_bg);
        }

        // Hit geometry comes straight from the shared `TabBarLayout` via the
        // same `tab_bar_hits_from_layout`/`shift_tab_bar_hits` pair the
        // no-paint `TuiBackend::tab_bar_layout_icons` twin
        // (`crate::tui::backend`) already uses — this function only
        // overrides `available_cols` with the tab-content width below and
        // keeps `correct_scroll_offset` as the caller's input (a cell-grid
        // char-based fit needs no correction).
        let mut hits = crate::backend::tab_bar_hits_from_layout(layout, bar);
        crate::backend::shift_tab_bar_hits(&mut hits, area.x as f64);

        // Tab-content width (engine feedback): bar minus reserved right area.
        let reserved: u16 = bar.right_segments.iter().map(|s| s.width_cells).sum();
        let tab_content_width = if area.width >= reserved {
            (area.width - reserved) as usize
        } else {
            area.width as usize
        };

        // ── Right-aligned segments (from layout) ───────────────────────────
        for vs in &layout.visible_segments {
            let seg = &bar.right_segments[vs.segment_idx];
            let fg = if seg.is_active { btn_fg_active } else { btn_fg };
            let bx = area.x + vs.bounds.x.round() as u16;
            let seg_end = bx + seg.width_cells;
            let mut cx = bx;
            for ch in seg.text.chars() {
                if cx >= seg_end {
                    break;
                }
                let w = char_cell_width(ch);
                if w == 2 {
                    if cx + 1 < seg_end + 1 {
                        set_cell_wide(buf, cx, area.y, ch, fg, bar_bg);
                        cx += 2;
                    } else {
                        cx += 1;
                    }
                } else {
                    set_cell(buf, cx, area.y, ch, fg, bar_bg);
                    cx += 1;
                }
            }
        }

        // ── Tabs (from layout) ─────────────────────────────────────────────
        let accent = bar.active_accent.map(ratatui_color);
        let active_fg = ratatui_color(theme.tab_active_fg);
        let active_bg = ratatui_color(theme.tab_active_bg);
        let preview_active_fg = ratatui_color(theme.tab_preview_active_fg);
        let inactive_fg = ratatui_color(theme.tab_inactive_fg);
        let preview_inactive_fg = ratatui_color(theme.tab_preview_inactive_fg);
        let separator = ratatui_color(theme.separator);

        for vt in &layout.visible_tabs {
            let tab = &bar.tabs[vt.tab_idx];
            let tab_x = area.x + vt.bounds.x.round() as u16;

            let (fg, bg) = match (tab.is_active, tab.is_preview) {
                (true, true) => (preview_active_fg, active_bg),
                (true, false) => (active_fg, active_bg),
                (false, true) => (preview_inactive_fg, bar_bg),
                (false, false) => (inactive_fg, bar_bg),
            };

            // Keyboard-focus treatment: swap fg/bg — reverse video — across
            // this tab's whole slot (icon, label, close/dirty glyph, bracket
            // framing, and the trailing separator/blank cell) when
            // `chrome.focused_index` names it. The caller composes this with
            // its own focus-visible state (see [`TabChrome::focused_index`]'s
            // doc): a click-suppressed focus passes `None` here, so no swap
            // happens and the tab paints exactly as it would unfocused.
            // `base_bg` keeps the tab's own pre-swap background available so
            // a glyph painted in a colour other than `fg` (the close/dirty
            // mark, the icon) can swap against it the same way the label
            // does, rather than keeping its own hue unswapped on an
            // already-inverted background.
            let is_focused = chrome.focused_index == Some(vt.tab_idx);
            let base_bg = bg;
            let (fg, bg) = if is_focused { (bg, fg) } else { (fg, bg) };
            let swap_against = |glyph_fg: ratatui::style::Color, base: ratatui::style::Color| {
                if is_focused {
                    (base, glyph_fg)
                } else {
                    (glyph_fg, base)
                }
            };
            let swap_against_base =
                |glyph_fg: ratatui::style::Color| swap_against(glyph_fg, base_bg);

            // When chrome requests bracket framing, the active tab's
            // full content — icon, label, close glyph — is enclosed in `[` /
            // `]`. `lead` shifts every subsequent paint position right by one
            // column to make room for the opening bracket; the closing
            // bracket is painted after the close glyph (or after the label,
            // for a bar with no close button) below.
            let bracket_active = matches!(chrome.active_frame, TabFrame::Brackets) && tab.is_active;
            let lead: u16 = if bracket_active { 1 } else { 0 };
            if bracket_active {
                set_cell(buf, tab_x, area.y, '[', fg, bg);
            }

            // Icon glyph, if this tab has one — painted before the label in
            // its own colour, independent of the tab's active/inactive fg.
            // `tab_icon_cols` (glyph width + 1-column gap) is exactly what
            // the measure side added to this tab's measured width, so the
            // label loop below can start right after it without re-deriving
            // the reservation.
            let icon_cols = tab_icon_cols(icons, vt.tab_idx);
            if let Some(icon) = tab_icon_at(icons, vt.tab_idx) {
                let (icon_fg, icon_bg) = swap_against_base(ratatui_color(icon.color));
                let icon_end = tab_x + lead + icon_cols;
                let mut cx = tab_x + lead;
                for ch in icon.glyph.chars() {
                    if cx >= icon_end {
                        break;
                    }
                    let w = char_cell_width(ch);
                    if w == 2 {
                        set_cell_wide(buf, cx, area.y, ch, icon_fg, icon_bg);
                        cx += 2;
                    } else {
                        set_cell(buf, cx, area.y, ch, icon_fg, icon_bg);
                        cx += 1;
                    }
                }
            }

            let mut modifier = Modifier::empty();
            if tab.is_preview {
                modifier |= Modifier::ITALIC;
            }
            if tab.is_active && accent.is_some() {
                modifier |= Modifier::UNDERLINED;
            }
            let prefix_mod = if tab.is_preview {
                Modifier::ITALIC
            } else {
                Modifier::empty()
            };

            // Layout carries total tab width; close_bounds (when present)
            // covers the close-glyph cells (already positioned before any
            // trailing chrome via `TabMeasure::trailing_width`). Label
            // occupies the cells between the leading bracket (if any) and
            // close_bounds.x.
            let tab_width = vt.bounds.width.round() as u16;
            // A bracket-framed tab with no close button still reserves its
            // last column for the closing bracket; `close_bounds` is `None`
            // in that case (no close button), so it isn't already excluded
            // from `tab_width` the way the close region is.
            let trailing_bracket_no_close: u16 = if bracket_active && vt.close_bounds.is_none() {
                1
            } else {
                0
            };
            let label_end = match vt.close_bounds {
                Some(cb) => area.x + cb.x.round() as u16,
                None => tab_x + tab_width - trailing_bracket_no_close,
            };
            let tab_end = tab_x + tab_width;

            // The label's cell budget is whatever sits between the leading
            // bracket/icon slot and `label_end` (the close glyph, trailing
            // bracket, or tab edge). A label that doesn't fit is
            // ellipsis-truncated through the shared [`fit_tab_label`] rather
            // than hard-clipped mid-glyph — the same `…`-costs-one-cell
            // convention `truncate_with_ellipsis` uses everywhere else in
            // the crate.
            let leading_slot = (lead + icon_cols) as f32;
            let trailing_slot = (tab_end.saturating_sub(label_end)) as f32;
            let fitted_label = fit_tab_label(
                &tab.label,
                tab_width as f32,
                leading_slot,
                trailing_slot,
                |s| crate::text_util::display_width(s) as f32,
            );

            // Filename (after the last ": ") carries the underline accent.
            // Computed off the fitted label so an ellipsis-truncated cut
            // before the ": " boundary falls back to treating the whole
            // visible text as "filename" rather than indexing stale offsets
            // from the untruncated original.
            let prefix_len = fitted_label.rfind(": ").map(|p| p + 2).unwrap_or(0);

            // Stride by display width, not by `char`: a double-width
            // glyph (CJK, many emoji) must land in two columns, mirroring the
            // right-segment loop above. The measure side computes
            // `label_end`'s budget with the same `display_width` function, so
            // the two sides agree.
            let mut x = tab_x + lead + icon_cols;
            for (ci, ch) in fitted_label.chars().enumerate() {
                if x >= label_end {
                    break;
                }
                let in_filename = ci >= prefix_len;
                let cell_mod = if in_filename { modifier } else { prefix_mod };
                let ul = if in_filename && tab.is_active {
                    accent
                } else {
                    None
                };
                let w = char_cell_width(ch);
                if w == 2 {
                    if x + 1 < label_end + 1 {
                        set_cell_wide_styled(buf, x, area.y, ch, fg, bg, cell_mod, ul);
                        x += 2;
                    } else {
                        // Not enough budget left for the glyph's second column
                        // — skip rather than paint half of a wide char.
                        x += 1;
                    }
                } else {
                    set_cell_styled(buf, x, area.y, ch, fg, bg, cell_mod, ul);
                    x += 1;
                }
            }

            // Close glyph: ● for dirty, × otherwise (or the app-supplied
            // override from `glyphs`).
            if vt.close_bounds.is_some() && x < tab_end {
                let (close_ch, close_fg) = if tab.is_dirty {
                    let ch = tab_glyph_char(
                        glyphs.and_then(TabBarGlyphs::dirty_glyph),
                        nerd_fonts_enabled,
                        '●',
                    );
                    (ch, foreground)
                } else {
                    let ch = tab_glyph_char(
                        glyphs.and_then(TabBarGlyphs::close_glyph),
                        nerd_fonts_enabled,
                        TAB_CLOSE_CHAR,
                    );
                    let fg = if tab.is_active { active_fg } else { separator };
                    (ch, fg)
                };
                let (close_fg, close_bg) = swap_against_base(close_fg);
                set_cell(buf, x, area.y, close_ch, close_fg, close_bg);
                x += 1;
            }
            // The closing bracket lands right after the close glyph
            // (or right after the label, for a bracket-framed tab with no
            // close button) — taking the place of the plain trailing
            // separator cell a non-bracket tab paints here.
            if bracket_active {
                if x < tab_end {
                    set_cell(buf, x, area.y, ']', fg, bg);
                }
            } else if x < tab_end {
                if vt.close_bounds.is_none() {
                    // No close button reserved for this tab — the measurer
                    // reserved this cell as a separator column instead, so
                    // paint a visible rule rather than blank space;
                    // otherwise adjacent labels read as one run of text with
                    // only bold-vs-not telling them apart.
                    let (sep_fg, sep_bg) = swap_against(separator, bar_bg);
                    set_cell(buf, x, area.y, TAB_SEPARATOR_CHAR, sep_fg, sep_bg);
                } else {
                    // Trailing separator space (within tab bounds, normally
                    // bar bg; tracks the already-swapped `bg` instead when
                    // focused, so a focused tab's reverse-video band reaches
                    // its right edge rather than stopping one column short).
                    let trailing_bg = if is_focused { bg } else { bar_bg };
                    set_cell(buf, x, area.y, ' ', trailing_bg, trailing_bg);
                }
            }
        }

        hits.available_cols = tab_content_width;
        // A cell-grid char-based fit is exact; no correction needed.
        hits.correct_scroll_offset = bar.scroll_offset;
        hits
    }
}

// ── `crate::primitive::Primitive` + `crate::interactive::Interactive` ──
//
// The backend-neutral widget-contract seam for `TabBar`: `layout` moves
// the measure-then-layout logic out of `Backend::tab_bar_layout*` (the
// TUI branch mirrors `TuiBackend::draw_tab_bar_icons`'s own char-cell
// measurer; the pixel branch is `layout_metrics::pixel_tab_bar_layout`
// fed by `LayoutCtx::text_width`, i.e. the calling backend's own
// `Backend::measure_text`, so the shared layout places tabs at the same
// glyph widths each pixel rasteriser paints them at).
// `paint`/`paint_cells` call the two
// painters this module already shares with every per-backend
// rasteriser — `native_surface_paint::paint` / `cell_paint::paint` —
// unchanged. `Interactive::handle` folds `TabBar::handle_focus_key`'s
// keyboard model in directly.
//
// Every `Backend::draw_tab_bar*`/`tab_bar_layout*` method and the four
// per-backend `tui|gtk|macos|win/tab_bar.rs` files still exist alongside
// this seam: `compose::WorkspaceController` paints its strip through
// `draw_dyn`, and the remaining callers stay on those rasterisers until
// each is migrated onto `Surface::Widget`/`draw_dyn` too.
//
// # Scope gaps against the full widget-contract recipe
//
// - **No icon/glyph/chrome sidecar.** `TabChrome`, `TabBarGlyphs` and the
//   `icons: &[Option<TabIcon>]` sidecar are backend-call parameters
//   today, not fields on `TabBar` (see that struct's own "why icons ride
//   beside the bar" doc) — `Primitive::layout`/`paint`/`paint_cells` only
//   ever see `&self`, so this seam always paints with
//   `TabChrome::default()`, no icons, and no glyph overrides. Turning
//   them into descriptor fields needs a new field on the plain,
//   exhaustively-constructed `TabBar` struct, which breaks every
//   existing literal — including `tests/acceptance/ms-11/*.rs`, the
//   sealed oracle slice a work leg cannot edit — so it stays out of this
//   slice.
// - **`focus()` is `FocusSpec::Stop`, not `Children`.** Matches
//   `ListView`/`CellGrid`'s own precedent for a composite widget with
//   internal keyboard navigation (one Tab stop; arrow keys move an
//   internal cursor) rather than one keyboard stop per tab. The pointer
//   side is separate and does name every part: `Interactive::sub_regions`
//   publishes each tab body, close button, scroll chevron and clickable
//   right segment as a `TabBarSubPart` tag, which is what the painted-
//   layout record hit-tests. `handle` still resolves a click through
//   `TabBarLayout::hit_test` against the `local` point, since the
//   runtime's own mouse routing
//   (`crate::runtime::route_widget_pointer_down`) delivers
//   `RoutedEvent::Pointer { sub: None, .. }`.
#[cfg(feature = "tui")]
fn tui_tab_bar_primitive_layout(bar: &TabBar, width: f32, height: f32) -> TabBarLayout {
    // Route through the exact resolver `crate::tui::backend::TuiBackend`'s
    // own `draw_tab_bar_icons`/`draw_tab_bar_icons_layout` call
    // (`crate::tui::tui_tab_bar_layout`) so this layout can never drift
    // from what TUI actually paints — see this section's own "No
    // icon/glyph/chrome sidecar" doc for why the icon sidecar here is
    // always empty.
    let area = ratatui::layout::Rect::new(0, 0, width as u16, height as u16);
    crate::tui::tui_tab_bar_layout(area, bar, &[])
}

/// Translate every `Rect` inside a bar-relative [`TabBarLayout`] by
/// `(dx, dy)` — [`native_surface_paint::paint`]'s `surface_*` calls paint
/// every tab/segment/scroll-arrow rect verbatim from `layout` (see that
/// function's own "Coordinate space" doc), and a generic
/// `&mut dyn PaintSurface` has no translate verb, so
/// [`Primitive::paint`](crate::primitive::Primitive::paint)'s pixel
/// branch bakes the widget rect's own `(x, y)` into a translated copy of
/// the computed `TabBarLayout` instead of passing the bar-relative one
/// straight through — the painter's own background fill then gets that
/// same offset via `bar_x`/`bar_y` so it lands in the same place as
/// every translated rect.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
fn translate_tab_bar_layout(mut layout: TabBarLayout, dx: f32, dy: f32) -> TabBarLayout {
    let shift = |r: Rect| Rect::new(r.x + dx, r.y + dy, r.width, r.height);
    for vt in &mut layout.visible_tabs {
        vt.bounds = shift(vt.bounds);
        vt.close_bounds = vt.close_bounds.map(shift);
    }
    for vs in &mut layout.visible_segments {
        vs.bounds = shift(vs.bounds);
    }
    layout.scroll_left = layout.scroll_left.map(shift);
    layout.scroll_right = layout.scroll_right.map(shift);
    for (r, _) in &mut layout.hit_regions {
        *r = shift(*r);
    }
    layout
}

/// [`crate::primitive::Primitive::Layout`] for [`TabBar`] — the
/// bar-relative [`TabBarLayout`] paired with the absolute area it was
/// laid out into and the Nerd-Font toggle resolved at layout time.
///
/// `layout`'s own `visible_tabs`/`visible_segments`/`hit_regions` stay
/// bar-relative (origin at this struct's own [`Self::area`] top-left) —
/// the same convention [`crate::interactive::RoutedEvent::Pointer`]'s own
/// `local` field uses, so [`Interactive::handle`] hit-tests against
/// `layout.layout` directly without subtracting [`Self::area`] first.
///
/// `#[non_exhaustive]`: backend-returned, never constructed by a
/// consumer via struct literal, same convention as [`TabBarLayout`]
/// itself.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TabBarPrimitiveLayout {
    /// Absolute area [`crate::primitive::Primitive::layout`] was called
    /// with.
    pub area: Rect,
    /// Nerd-Font toggle resolved from
    /// [`crate::primitive::LayoutCtx::nerd_fonts_enabled`] at layout
    /// time.
    pub nerd_fonts_enabled: bool,
    /// The bar-relative layout itself.
    pub layout: TabBarLayout,
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl crate::primitive::Primitive for TabBar {
    type Layout = TabBarPrimitiveLayout;
    type Event = TabBarEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Round-trips through [`crate::frame::OwnedContent::TabBarWidget`] —
    /// distinct from [`crate::frame::OwnedContent::TabBar`] (which
    /// [`crate::frame::capture_surface`] produces for the legacy
    /// [`crate::frame::Surface::TabBar`] instead) so a `TabBar` reached
    /// through [`crate::frame::Surface::Widget`] replays through
    /// [`crate::backend::DrawsPrimitives::draw_primitive`] and
    /// hit-tests as [`crate::frame::FrameZone::Widget`] after capture,
    /// exactly as it does live — see that variant's own doc.
    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::TabBarWidget(self.clone())
    }

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> Self::Layout {
        // Same TUI-vs-pixel-backend discriminator `ListView`/`DataTable`'s
        // own `Primitive::layout` use — see `LayoutCtx::units`'s own doc
        // for why `ctx.units()` is reliable even when both features are
        // compiled into the same binary.
        #[cfg(feature = "tui")]
        {
            if matches!(ctx.units(), crate::primitive::Units::Cells) {
                let layout = tui_tab_bar_primitive_layout(self, rect.width, rect.height);
                return TabBarPrimitiveLayout {
                    area: rect,
                    nerd_fonts_enabled: ctx.nerd_fonts_enabled,
                    layout,
                };
            }
        }
        // Tab labels, close glyphs and right segments are chrome —
        // painted in the backend's UI font by every
        // `draw_tab_bar*` rasteriser — so this measures them through the
        // calling backend's own `Backend::measure_text` for
        // `FontRole::Chrome`, the same font those rasterisers resolve.
        // Without a measurer in reach (`LayoutCtx::new`) it degrades to
        // the `ctx.char_width` approximation.
        //
        // Known gap: a preview tab (`TabItem::is_preview`) is rasterised
        // in an *italic* variant of the chrome font, and `FontRole` has
        // no italic member, so its label is measured here in the upright
        // face. Widths for a bar containing preview tabs can therefore
        // still drift slightly from what the pixel rasteriser paints.
        let measure = crate::primitives::layout_metrics::LayoutCtxMeasure {
            ctx,
            role: crate::FontRole::Chrome,
        };
        let (tab_pad, tab_inner_gap, tab_outer_gap) = if self.compact {
            (
                crate::primitives::layout_metrics::pixel::TAB_PAD_COMPACT,
                crate::primitives::layout_metrics::pixel::TAB_INNER_GAP_COMPACT,
                crate::primitives::layout_metrics::pixel::TAB_OUTER_GAP_COMPACT,
            )
        } else {
            (
                crate::primitives::layout_metrics::pixel::TAB_PAD,
                crate::primitives::layout_metrics::pixel::TAB_INNER_GAP,
                crate::primitives::layout_metrics::pixel::TAB_OUTER_GAP,
            )
        };
        let tab_name_widths: Vec<f32> = self
            .tabs
            .iter()
            .map(|t| crate::primitives::layout_metrics::TextMeasure::width_of(&measure, &t.label))
            .collect();
        // No icon sidecar reaches this far either (see this section's own
        // doc) — every tab's icon-extra width is `0.0`.
        let tab_icon_extras = vec![0.0f32; self.tabs.len()];
        let chrome = TabChrome::default();
        let (layout, _corrected_scroll_offset, _available_cols) =
            crate::primitives::layout_metrics::pixel_tab_bar_layout(
                self,
                rect.width,
                rect.height,
                tab_pad,
                tab_inner_gap,
                tab_outer_gap,
                &tab_name_widths,
                &tab_icon_extras,
                &chrome,
                &measure,
            );
        TabBarPrimitiveLayout {
            area: rect,
            nerd_fonts_enabled: ctx.nerd_fonts_enabled,
            layout,
        }
    }

    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            // See `translate_tab_bar_layout`'s own doc: bakes
            // `layout.area`'s own origin into a translated copy of the
            // bar-relative layout, then passes the same `(x, y)` as
            // `bar_x`/`bar_y` so the shared painter's own background-fill
            // rect lines up with it too.
            let translated =
                translate_tab_bar_layout(layout.layout.clone(), layout.area.x, layout.area.y);
            let state = native_surface_paint::TabBarPaintState::default();
            native_surface_paint::paint(
                self,
                &translated,
                &[],
                None,
                layout.nerd_fonts_enabled,
                &TabChrome::default(),
                state,
                surface,
                theme,
                style,
                layout.area.x,
                layout.area.y,
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// Delegates straight to [`cell_paint::paint`] — the exact function
    /// [`crate::tui::draw_tab_bar_icons_with_chrome_and_glyphs`] itself
    /// re-exports — with an absolute `area` derived from
    /// [`TabBarPrimitiveLayout::area`], so painting `TabBar` through
    /// [`crate::primitive::Primitive`] on TUI uses byte-for-byte the same
    /// painter every `Backend::draw_tab_bar*` entry point does.
    #[cfg(feature = "tui")]
    #[allow(deprecated)] // `cell_paint::paint` returns the deprecated `TabBarHits`
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        let area = crate::tui::backend::q_rect_to_ratatui(layout.area);
        let _ = cell_paint::paint(
            buf,
            area,
            self,
            &[],
            &TabChrome::default(),
            &layout.layout,
            theme,
            None,
            layout.nerd_fonts_enabled,
        );
    }
}

// ── Interactive: keyboard nav + mouse click ─────────────────────────
//
// `TabBar::handle_focus_key`'s model folds in directly: `state.focused_index`
// is the live counterpart of the caller-threaded `Option<usize>` that
// free function took, moved by Left/Right and activated by Enter/Space,
// exactly as that function's own doc describes.

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl WidgetAction for TabBarEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::TabBar(id, self)
    }
}

/// [`Interactive::State`] for [`TabBar`] — see this section's module doc
/// for why it carries only the keyboard-focused tab index,
/// [`TabChrome::focused_index`]'s live counterpart.
///
/// `#[non_exhaustive]`: a future field add here is additive, same
/// convention as [`TabBarLayout`].
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabBarState {
    /// The tab Left/Right has moved keyboard focus to. `None` until the
    /// first arrow press. The app threads this back onto the next
    /// frame's [`TabChrome::focused_index`] (with its own focus-visible
    /// composition — see that field's doc) to paint the reverse-video
    /// treatment.
    pub focused_index: Option<usize>,
}

/// Canonical key string for [`TabBarEvent::KeyPressed`] — the same
/// spelling [`NamedKey::name`] already promises, mirroring
/// `crate::primitives::list`'s own identically-named private helper so
/// every `KeyPressed` event in the crate agrees on one spelling.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
fn tab_bar_key_to_event_string(key: &Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Named(named) => named.name().into_owned(),
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl TabBar {
    /// Dispatch one keystroke against live [`TabBarState`] — the
    /// `Interactive::handle` counterpart of the static
    /// [`Self::handle_focus_key`] this crate's own backends called
    /// directly before this seam existed. Left/Right move
    /// `state.focused_index` (clamped into `0..tabs.len()`, starting
    /// from tab `0` when nothing was focused yet); Enter/Space activate
    /// the currently focused tab; every other key is reported via
    /// [`EventCx::emit`] as the raw [`TabBarEvent::KeyPressed`] an app
    /// may bind its own hotkeys to, then declared [`Handled::Ignored`]
    /// rather than claimed — see [`Interactive::handle`]'s own doc for
    /// why an unclaimed key must still bubble.
    fn handle_key_routed(
        &self,
        state: &mut TabBarState,
        key: &Key,
        modifiers: &Modifiers,
        cx: &mut EventCx,
    ) -> Handled<TabBarEvent> {
        if self.tabs.is_empty() {
            return Handled::Ignored;
        }
        match key {
            Key::Named(NamedKey::Left) => {
                let cur = state.focused_index.unwrap_or(0);
                let next = cur.saturating_sub(1);
                state.focused_index = Some(next);
                Handled::Event(TabBarEvent::FocusMoved { index: next })
            }
            Key::Named(NamedKey::Right) => {
                let cur = state.focused_index.unwrap_or(0);
                let next = (cur + 1).min(self.tabs.len() - 1);
                state.focused_index = Some(next);
                Handled::Event(TabBarEvent::FocusMoved { index: next })
            }
            Key::Named(NamedKey::Enter) | Key::Char(' ') => match state.focused_index {
                Some(index) => Handled::Event(TabBarEvent::TabActivated { index }),
                None => Handled::Ignored,
            },
            _ => {
                cx.emit(
                    TabBarEvent::KeyPressed {
                        key: tab_bar_key_to_event_string(key),
                        modifiers: *modifiers,
                    }
                    .into_ui_event(cx.id),
                );
                Handled::Ignored
            }
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl Interactive for TabBar {
    type State = TabBarState;
    type Action = TabBarEvent;

    /// One focus stop as a whole — see this section's parent doc's
    /// "`focus()` is `FocusSpec::Stop`, not `Children`" bullet.
    /// [`FocusSpec::None`] for an empty bar — there is no tab for
    /// keyboard focus to land on.
    fn focus(&self) -> FocusSpec {
        if self.tabs.is_empty() {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    /// Every tab body, close button, scroll chevron and clickable right
    /// segment this layout resolved — [`tab_bar_sub_regions`], which also
    /// backs [`tab_part_center`]'s legacy fallback.
    ///
    /// [`TabBarPrimitiveLayout::layout`] is already bar-relative (see
    /// that field's doc), which is exactly the widget-local space
    /// [`Interactive::sub_regions`] asks for — no translation needed
    /// here.
    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, crate::interactive::SubTarget)> {
        tab_bar_sub_regions(&layout.layout)
    }

    fn handle(
        &self,
        state: &mut TabBarState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<TabBarEvent> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => match layout.layout.hit_test(local.x, local.y) {
                TabBarHit::Tab(index) => {
                    cx.request_focus(None);
                    state.focused_index = Some(index);
                    Handled::Event(TabBarEvent::TabActivated { index })
                }
                TabBarHit::TabClose(index) => {
                    cx.request_focus(None);
                    Handled::Event(TabBarEvent::TabClosed { index })
                }
                TabBarHit::RightSegment(id) => {
                    cx.request_focus(None);
                    Handled::Event(TabBarEvent::ButtonClicked { id })
                }
                // No `TabBarEvent` exists yet for a scroll-chevron click
                // (see this section's parent doc) and empty space is
                // simply not actionable.
                TabBarHit::ScrollLeft | TabBarHit::ScrollRight | TabBarHit::Empty => {
                    Handled::Ignored
                }
            },
            RoutedEvent::Key(KeyEvent { key, modifiers, .. }) => {
                self.handle_key_routed(state, key, modifiers, cx)
            }
            // The assistive-technology default action activates whichever
            // tab keyboard focus already names, falling back to the
            // bar's own `is_active` tab when nothing has been
            // keyboard-focused yet — the same "describe what a real
            // keyboard/mouse gesture would do" contract
            // `a11y::action::route_action`'s own doc states.
            RoutedEvent::Action(A11yAction::Click) => {
                let index = state
                    .focused_index
                    .filter(|&i| i < self.tabs.len())
                    .or_else(|| self.tabs.iter().position(|t| t.is_active));
                match index {
                    Some(index) => Handled::Event(TabBarEvent::TabActivated { index }),
                    None => Handled::Ignored,
                }
            }
            _ => Handled::Ignored,
        }
    }
}

#[cfg(all(
    test,
    any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )
))]
mod interactive_tests {
    use super::*;
    use crate::event::{NamedKey, Point};
    use crate::interactive::EventHost;
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock — the same shape
    /// `crate::primitives::list`'s own module tests use.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<crate::interactive::SubTarget>>,
        emitted: Vec<crate::UiEvent>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<crate::interactive::SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<crate::interactive::SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, event: crate::UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn bar(labels: &[&str], active: usize) -> TabBar {
        TabBar {
            id: WidgetId::new("tabs:test"),
            tabs: labels
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: label.to_string(),
                    is_active: i == active,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }

    /// Whole-cell chrome metrics plus a 40×1 bar rect —
    /// [`LayoutCtx::units`] reports [`Units::Cells`], so
    /// [`Primitive::layout`] resolves its TUI branch.
    #[cfg(feature = "tui")]
    fn cell_regime() -> (LayoutCtx<'static>, Rect) {
        (
            LayoutCtx::new(1.0, 1.0, false),
            Rect::new(0.0, 0.0, 40.0, 1.0),
        )
    }

    /// Pixel chrome metrics plus a 400×24 bar rect — the regime every
    /// pixel backend (GTK, Win, macOS) lays out in. The rect is wide
    /// enough for the fixtures below to fit with
    /// [`crate::primitives::layout_metrics::pixel`]'s own tab padding and
    /// gaps, so the resolved layout has real tab and close-glyph bounds
    /// to hit-test against.
    fn pixel_regime() -> (LayoutCtx<'static>, Rect) {
        (
            LayoutCtx::new(24.0, 8.0, false),
            Rect::new(0.0, 0.0, 400.0, 24.0),
        )
    }

    /// Lay `bar` out once per unit regime this build can resolve: the
    /// pixel regime always (its `Primitive::layout` branch compiles for
    /// every backend feature), plus the whole-cell regime when `tui` is
    /// compiled in. Geometry-dependent tests assert against every entry
    /// so a hit-test holds on both branches rather than only on the one
    /// the active feature set happens to select.
    fn layout_regimes(bar: &TabBar) -> Vec<TabBarPrimitiveLayout> {
        let mut out = Vec::new();
        #[cfg(feature = "tui")]
        {
            let (ctx, rect) = cell_regime();
            out.push(Primitive::layout(bar, &ctx, rect));
        }
        let (ctx, rect) = pixel_regime();
        out.push(Primitive::layout(bar, &ctx, rect));
        out
    }

    /// One representative layout, for the tests that need *a* layout
    /// value but never read its geometry (keyboard and a11y dispatch
    /// ignore `layout` entirely) — the whole-cell one when `tui` is
    /// compiled in, so the cell-paint round-trip below gets a layout
    /// matching its own 40×1 buffer.
    fn layout_for(bar: &TabBar) -> TabBarPrimitiveLayout {
        layout_regimes(bar).swap_remove(0)
    }

    fn key(named: NamedKey) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// Per-label widths a fake [`LayoutCtx::with_measurer`] measurer
    /// reports, deliberately unrelated to [`pixel_regime`]'s
    /// `char_width: 8.0`: `alpha` (5 chars) measures *wider* than its
    /// approximation and `beta` (4 chars) *narrower*, the same
    /// both-directions error a real proportional chrome font produces.
    /// Anything else (the `×` close glyph) falls through to the same
    /// `char_width` the approximation uses, so only the label widths
    /// differ between the two layouts below.
    fn fake_text_width(text: &str) -> f32 {
        match text {
            "alpha" => 100.0,
            "beta" => 20.0,
            other => other.chars().count() as f32 * 8.0,
        }
    }

    /// Each visible tab's `(x, x + width)` span.
    fn tab_spans(layout: &TabBarPrimitiveLayout) -> Vec<(f32, f32)> {
        layout
            .layout
            .visible_tabs
            .iter()
            .map(|vt| (vt.bounds.x, vt.bounds.x + vt.bounds.width))
            .collect()
    }

    /// `Primitive::layout`'s pixel branch sizes tabs from
    /// [`LayoutCtx::text_width`] — the calling backend's real font
    /// measurement — not from `char_width * chars`.
    ///
    /// Expected spans come straight from `pixel_tab_bar_layout`'s own
    /// `TabMeasure::total` arithmetic at
    /// [`crate::primitives::layout_metrics::pixel`]'s non-compact
    /// metrics (`TAB_PAD` 14, `TAB_INNER_GAP` 10, `TAB_OUTER_GAP` 1) plus
    /// an 8px `×`: each tab's slot is `14 + name_w + (10 + 8) + 14 + 1`
    /// wide, so only `name_w` differs between the two layouts.
    #[test]
    fn primitive_layout_sizes_tabs_from_the_ctx_text_measurer() {
        let bar = bar(&["alpha", "beta"], 0);
        let (base, rect) = pixel_regime();
        let measurer = |text: &str, _role: crate::FontRole| fake_text_width(text);

        let measured = Primitive::layout(&bar, &base.with_measurer(&measurer), rect);
        assert_eq!(
            tab_spans(&measured),
            vec![(0.0, 147.0), (147.0, 214.0)],
            "tab widths must come from the ctx's text measurer (alpha 100px, beta 20px), \
             not from char_width * chars"
        );

        let approximated = Primitive::layout(&bar, &base, rect);
        assert_eq!(
            tab_spans(&approximated),
            vec![(0.0, 87.0), (87.0, 166.0)],
            "with no measurer attached the same layout falls back to char_width * chars \
             (alpha 40px, beta 32px)"
        );
    }

    #[test]
    fn empty_bar_has_no_focus_stop() {
        assert_eq!(bar(&[], 0).focus(), FocusSpec::None);
    }

    #[test]
    fn non_empty_bar_is_one_focus_stop() {
        assert_eq!(bar(&["a.rs"], 0).focus(), FocusSpec::Stop);
    }

    #[test]
    fn click_on_a_tab_body_activates_it_and_requests_focus() {
        let b = bar(&["main.rs", "lib.rs"], 0);
        for layout in layout_regimes(&b) {
            let mut state = TabBarState::default();
            let mut host = MockHost::default();
            let mut cx = EventCx::new(b.id, &mut host);

            // The first tab's own centre, read back from the resolved
            // layout rather than hardcoded, so the probe lands inside the
            // tab body in whichever unit regime laid it out.
            let (cx_pt, cy_pt) = layout
                .layout
                .tab_center(0)
                .expect("the first tab is visible on a bar this wide");
            let click = RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(cx_pt, cy_pt),
                mods: Modifiers::default(),
            };
            let outcome = b.handle(&mut state, &click, &layout, &mut cx);
            assert_eq!(
                outcome,
                Handled::Event(TabBarEvent::TabActivated { index: 0 })
            );
            assert_eq!(state.focused_index, Some(0));
            assert_eq!(host.focus_requests, vec![None]);
        }
    }

    #[test]
    fn click_on_a_close_glyph_closes_that_tab() {
        let b = bar(&["main.rs"], 0);
        for layout in layout_regimes(&b) {
            let (close_x, close_y) = layout
                .layout
                .tab_close_center(0)
                .expect("main.rs has a close button");
            let mut state = TabBarState::default();
            let mut host = MockHost::default();
            let mut cx = EventCx::new(b.id, &mut host);

            let click = RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(close_x, close_y),
                mods: Modifiers::default(),
            };
            let outcome = b.handle(&mut state, &click, &layout, &mut cx);
            assert_eq!(outcome, Handled::Event(TabBarEvent::TabClosed { index: 0 }));
        }
    }

    #[test]
    fn right_arrow_then_left_arrow_move_the_focused_index_and_clamp() {
        let b = bar(&["a", "b", "c"], 0);
        let layout = layout_for(&b);
        let mut state = TabBarState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(b.id, &mut host);

        let outcome = b.handle(&mut state, &key(NamedKey::Right), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(TabBarEvent::FocusMoved { index: 1 })
        );
        assert_eq!(state.focused_index, Some(1));

        let outcome = b.handle(&mut state, &key(NamedKey::Right), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(TabBarEvent::FocusMoved { index: 2 })
        );

        // Already at the last tab — Right clamps rather than panicking
        // or wrapping.
        let outcome = b.handle(&mut state, &key(NamedKey::Right), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(TabBarEvent::FocusMoved { index: 2 })
        );

        let outcome = b.handle(&mut state, &key(NamedKey::Left), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(TabBarEvent::FocusMoved { index: 1 })
        );
    }

    #[test]
    fn enter_with_no_focused_tab_is_ignored() {
        let b = bar(&["a", "b"], 0);
        let layout = layout_for(&b);
        let mut state = TabBarState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(b.id, &mut host);

        let outcome = b.handle(&mut state, &key(NamedKey::Enter), &layout, &mut cx);
        assert_eq!(outcome, Handled::Ignored);
    }

    #[test]
    fn enter_activates_the_focused_tab() {
        let b = bar(&["a", "b"], 0);
        let layout = layout_for(&b);
        let mut state = TabBarState {
            focused_index: Some(1),
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(b.id, &mut host);

        let outcome = b.handle(&mut state, &key(NamedKey::Enter), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(TabBarEvent::TabActivated { index: 1 })
        );
    }

    #[test]
    fn unbound_key_still_emits_key_pressed_and_bubbles() {
        let b = bar(&["a"], 0);
        let layout = layout_for(&b);
        let mut state = TabBarState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(b.id, &mut host);

        let outcome = b.handle(&mut state, &key(NamedKey::Escape), &layout, &mut cx);
        assert_eq!(outcome, Handled::Ignored);
        assert_eq!(
            host.emitted,
            vec![TabBarEvent::KeyPressed {
                key: "Escape".to_string(),
                modifiers: Modifiers::default(),
            }
            .into_ui_event(b.id)]
        );
    }

    #[test]
    fn a11y_click_activates_the_focused_tab_or_falls_back_to_is_active() {
        let b = bar(&["a", "b", "c"], 1);
        let layout = layout_for(&b);
        let mut host = MockHost::default();

        // No keyboard focus yet — falls back to whichever tab `is_active`.
        let mut state = TabBarState::default();
        let mut cx = EventCx::new(b.id, &mut host);
        let outcome = b.handle(
            &mut state,
            &RoutedEvent::Action(A11yAction::Click),
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(TabBarEvent::TabActivated { index: 1 })
        );

        // Keyboard focus set — wins over `is_active`.
        state.focused_index = Some(0);
        let mut cx = EventCx::new(b.id, &mut host);
        let outcome = b.handle(
            &mut state,
            &RoutedEvent::Action(A11yAction::Click),
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(TabBarEvent::TabActivated { index: 0 })
        );
    }

    /// Three-way event parity, the narrower (in-crate) half of this
    /// slice's parity test: an `A11yAction::Click` routed through
    /// [`crate::a11y::route_widget_action`] (the assistive-technology
    /// path a screen reader's default action takes) produces the exact
    /// same [`crate::UiEvent`] a direct [`Interactive::handle`] call with
    /// the same `RoutedEvent::Action` does — proving the generic
    /// `FrameTree`-based a11y routing seam and this widget's own
    /// `handle` dispatch agree by construction. The TUI mouse-click leg
    /// of the parity triangle lives in
    /// `tests/tab_bar_interactive_driver.rs`, which drives the real
    /// `event -> handle -> render` path `TuiDriver` exercises; this
    /// in-crate test is the one place that can reach
    /// `route_widget_action`, which (like `route_action`) is not
    /// re-exported past this crate's own private `a11y` module.
    #[test]
    fn route_widget_action_click_matches_direct_interactive_handle() {
        let b = bar(&["a", "b", "c"], 0);
        let id = b.id;
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(id, Rect::new(0.0, 0.0, 40.0, 1.0), Box::new(b.clone()));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let ctx = LayoutCtx::new(1.0, 1.0, false);

        let events =
            crate::a11y::route_widget_action(&tree, &mut states, &ctx, id, A11yAction::Click);

        let layout = layout_for(&b);
        let mut state = TabBarState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(id, &mut host);
        let direct = b.handle(
            &mut state,
            &RoutedEvent::Action(A11yAction::Click),
            &layout,
            &mut cx,
        );
        let expected = match direct {
            Handled::Event(ev) => vec![ev.into_ui_event(id)],
            _ => vec![],
        };
        assert_eq!(events, expected);
        assert_eq!(
            events,
            vec![TabBarEvent::TabActivated { index: 0 }.into_ui_event(id)]
        );
    }

    #[cfg(feature = "tui")]
    #[test]
    fn primitive_layout_round_trips_through_the_cell_paint_painter() {
        let b = bar(&["main.rs", "lib.rs"], 0);
        let layout = layout_for(&b);
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 1));
        Primitive::paint_cells(&b, &layout, &mut buf, &crate::theme::Theme::default());
        let painted: String = (0..10).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            painted.starts_with("main.rs"),
            "expected the first tab's label to paint at the bar's own origin, got {painted:?}"
        );
    }
}

#[cfg(test)]
mod hit_test_diff_tests {
    use super::*;

    fn make_diff_bar() -> TabBar {
        TabBar {
            id: WidgetId::new("tabs:group"),
            tabs: vec![
                TabItem {
                    label: "main.rs".into(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
                TabItem {
                    label: "lib.rs".into(),
                    is_active: false,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
            ],
            right_segments: vec![
                // change_label = "1 of 5", text = " 1 of 5" = 7 chars
                TabBarSegment {
                    id: None,
                    text: " 1 of 5".into(),
                    width_cells: 7,
                    is_active: false,
                },
                TabBarSegment {
                    id: Some(WidgetId::new("tab:diff_prev")),
                    text: " a".into(),
                    width_cells: 3,
                    is_active: false,
                },
                TabBarSegment {
                    id: Some(WidgetId::new("tab:diff_next")),
                    text: " b".into(),
                    width_cells: 3,
                    is_active: false,
                },
                TabBarSegment {
                    id: Some(WidgetId::new("tab:diff_toggle")),
                    text: " c".into(),
                    width_cells: 3,
                    is_active: false,
                },
                TabBarSegment {
                    id: Some(WidgetId::new("tab:split_right")),
                    text: " d".into(),
                    width_cells: 3,
                    is_active: false,
                },
                TabBarSegment {
                    id: Some(WidgetId::new("tab:split_down")),
                    text: " e ".into(),
                    width_cells: 3,
                    is_active: false,
                },
                TabBarSegment {
                    id: Some(WidgetId::new("tab:action_menu")),
                    text: " f ".into(),
                    width_cells: 3,
                    is_active: false,
                },
            ],
            active_accent: None,
            scroll_offset: 0,
            show_tab_close: true,
            compact: false,
        }
    }

    #[test]
    fn diff_buttons_resolve_to_correct_widget_ids() {
        let bar = make_diff_bar();
        let bar_width = 80.0_f32;
        let tab_widths = [9_usize, 8];
        let layout = bar.layout(
            bar_width,
            1.0,
            0.0,
            |i| TabMeasure::new(tab_widths[i] as f32, 2.0),
            |i| SegmentMeasure::new(bar.right_segments[i].width_cells as f32),
        );

        // right_area = 7 + 3*6 = 25; segs start at 80 - 25 = 55.
        // change_label: 55..62, diff_prev: 62..65, diff_next: 65..68,
        // diff_toggle: 68..71, split_right: 71..74, split_down: 74..77,
        // action_menu: 77..80.
        for (col, expected) in [
            (62, "tab:diff_prev"),
            (64, "tab:diff_prev"),
            (65, "tab:diff_next"),
            (67, "tab:diff_next"),
            (68, "tab:diff_toggle"),
            (70, "tab:diff_toggle"),
            (71, "tab:split_right"),
            (74, "tab:split_down"),
            (77, "tab:action_menu"),
        ] {
            let hit = layout.hit_test(col as f32, 0.0);
            match hit {
                TabBarHit::RightSegment(id) => {
                    assert_eq!(
                        id.as_str(),
                        expected,
                        "click at col {col} expected {expected}, got {id:?}"
                    );
                }
                other => {
                    panic!("click at col {col} expected RightSegment({expected}), got {other:?}")
                }
            }
        }

        // Click on change_label (col 56) should be Empty (no id).
        match layout.hit_test(56.0, 0.0) {
            TabBarHit::Empty => {}
            other => panic!("click at col 56 (change_label) expected Empty, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_bar_roundtrip_serde() {
        let bar = TabBar {
            id: WidgetId::new("group-0-tabs"),
            tabs: vec![
                TabItem {
                    label: " 1: main.rs ".to_string(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
                TabItem {
                    label: " 2: lib.rs ".to_string(),
                    is_active: false,
                    is_dirty: true,
                    is_preview: false,
                    is_closable: true,
                },
                TabItem {
                    label: " 3: render.rs ".to_string(),
                    is_active: false,
                    is_dirty: false,
                    is_preview: true,
                    is_closable: true,
                },
            ],
            scroll_offset: 0,
            right_segments: vec![
                TabBarSegment {
                    text: "2 of 5 ".to_string(),
                    width_cells: 7,
                    id: None,
                    is_active: false,
                },
                TabBarSegment {
                    text: " ← ".to_string(),
                    width_cells: 3,
                    id: Some(WidgetId::new("tab:diff_prev")),
                    is_active: false,
                },
                TabBarSegment {
                    text: " … ".to_string(),
                    width_cells: 3,
                    id: Some(WidgetId::new("tab:action_menu")),
                    is_active: false,
                },
            ],
            active_accent: Some(Color::rgb(100, 200, 255)),
            show_tab_close: true,
            compact: false,
        };
        let json = serde_json::to_string(&bar).unwrap();
        let back: TabBar = serde_json::from_str(&json).unwrap();
        assert_eq!(bar, back);
    }

    #[test]
    fn tab_bar_fit_active_scroll_offset() {
        // 10 tabs, each measures 100 units. Width 250 fits 2 full tabs.
        let measure = |_i: usize| 100usize;

        // Active 0 fits at offset 0.
        assert_eq!(TabBar::fit_active_scroll_offset(0, 10, 250, measure), 0);
        // Active 1 fits at offset 0 (tabs 0,1 fit in 250).
        assert_eq!(TabBar::fit_active_scroll_offset(1, 10, 250, measure), 0);
        // Active 2 doesn't fit at offset 0; walk back from 2 → tabs 1,2 fit in 250 → offset 1.
        assert_eq!(TabBar::fit_active_scroll_offset(2, 10, 250, measure), 1);
        // Active 9 (last) → tabs 8,9 fit → offset 8.
        assert_eq!(TabBar::fit_active_scroll_offset(9, 10, 250, measure), 8);

        // Variable widths: GTK-like scenario where each tab has ~50 units of
        // padding overhead the engine's char-based estimate would miss.
        let varied = [60, 80, 70, 90, 100, 50, 120, 75, 65, 85];
        let measure_varied = |i: usize| varied[i];
        // Width 200, active 9 (last). Walk back: 85+65=150, +75=225 > 200 → break.
        // best_offset stays at 8 (only tabs 8,9 fit).
        assert_eq!(
            TabBar::fit_active_scroll_offset(9, 10, 200, measure_varied),
            8
        );

        // Edge case: tab wider than width → keep that tab as the only visible one.
        // Active 5 (width 50) at width 30. Walk back from 5: tw=50 > 30 → break.
        // best_offset stays at 5 (initial). Renders tab 5 alone, clipped on right.
        assert_eq!(
            TabBar::fit_active_scroll_offset(5, 10, 30, measure_varied),
            5
        );

        // Edge cases: empty + out-of-bounds active.
        assert_eq!(TabBar::fit_active_scroll_offset(0, 0, 100, measure), 0);
        assert_eq!(TabBar::fit_active_scroll_offset(99, 5, 100, measure), 0);
    }

    // ── D6 layout API tests (per-primitive Layout + hit_test) ────────────
    //
    // These exercise the unit-agnostic contract: a TUI-style measurer
    // (char counts, integer-valued f32) and a pixel-style measurer
    // (fractional f32 from proportional-font metrics) must both produce
    // consistent layouts. The cross-backend correctness of this
    // abstraction is the whole point — see north-star goal in PLAN.md.

    fn make_tab(label: &str, is_active: bool) -> TabItem {
        TabItem {
            label: label.to_string(),
            is_active,
            is_dirty: false,
            is_preview: false,
            is_closable: true,
        }
    }

    fn make_bar(tabs: Vec<TabItem>) -> TabBar {
        TabBar {
            id: WidgetId::new("t"),
            tabs,
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }

    #[test]
    fn tab_bar_layout_empty() {
        let bar = make_bar(vec![]);
        let layout = bar.layout(
            100.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 0);
        assert_eq!(layout.visible_segments.len(), 0);
        assert!(layout.scroll_left.is_none());
        assert!(layout.scroll_right.is_none());
        assert_eq!(layout.hit_regions.len(), 0);
        assert_eq!(layout.resolved_scroll_offset, 0);
        assert_eq!(layout.hit_test(5.0, 1.0), TabBarHit::Empty);
    }

    #[test]
    fn tab_bar_layout_single_tab_fits() {
        let bar = make_bar(vec![make_tab("main.rs", true)]);
        // Width 20; tab is 10 wide; plenty of room; no scroll arrows needed.
        let layout = bar.layout(
            20.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 2.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 1);
        assert_eq!(layout.visible_tabs[0].tab_idx, 0);
        assert_eq!(layout.visible_tabs[0].bounds.x, 0.0);
        assert_eq!(layout.visible_tabs[0].bounds.width, 10.0);
        assert!(layout.visible_tabs[0].close_bounds.is_some());
        let cb = layout.visible_tabs[0].close_bounds.unwrap();
        assert_eq!(cb.x, 8.0); // 10 - 2
        assert_eq!(cb.width, 2.0);
        assert!(layout.scroll_left.is_none());
        assert!(layout.scroll_right.is_none());

        // Hit-test: click on tab body returns Tab(0).
        assert_eq!(layout.hit_test(5.0, 1.0), TabBarHit::Tab(0));
        // Click on close area returns TabClose(0), not Tab(0).
        assert_eq!(layout.hit_test(9.0, 1.0), TabBarHit::TabClose(0));
        // Click past the tab returns Empty.
        assert_eq!(layout.hit_test(15.0, 1.0), TabBarHit::Empty);
    }

    #[test]
    fn tab_bar_layout_tui_char_units() {
        // Classic TUI scenario: 3 tabs each 10 cells wide; bar is 30 cells.
        // All fit, no scroll.
        let bar = make_bar(vec![
            make_tab("a", true),
            make_tab("b", false),
            make_tab("c", false),
        ]);
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 3);
        assert_eq!(layout.visible_tabs[0].bounds.x, 0.0);
        assert_eq!(layout.visible_tabs[1].bounds.x, 10.0);
        assert_eq!(layout.visible_tabs[2].bounds.x, 20.0);
        assert!(layout.scroll_left.is_none());
        assert!(layout.scroll_right.is_none());
        assert_eq!(layout.resolved_scroll_offset, 0);
    }

    #[test]
    fn tab_bar_layout_pixel_units_fractional() {
        // Proves the unit-agnostic contract: fractional pixel widths (as
        // Pango would return) produce a consistent layout. Same 3 tabs but
        // each measured at 87.5 px; bar is 400 px.
        let bar = make_bar(vec![
            make_tab("a", true),
            make_tab("b", false),
            make_tab("c", false),
        ]);
        let layout = bar.layout(
            400.0,
            22.0,
            16.0,
            |_| TabMeasure::new(87.5, 18.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 3);
        assert_eq!(layout.visible_tabs[0].bounds.x, 0.0);
        assert_eq!(layout.visible_tabs[1].bounds.x, 87.5);
        assert_eq!(layout.visible_tabs[2].bounds.x, 175.0);
        assert_eq!(layout.visible_tabs[0].bounds.height, 22.0);
        let cb = layout.visible_tabs[0].close_bounds.unwrap();
        assert_eq!(cb.x, 87.5 - 18.0);
        assert_eq!(cb.width, 18.0);
    }

    #[test]
    fn tab_bar_layout_overflow_active_in_middle() {
        // 10 tabs × 10 cells; bar 30 cells → ~2 tabs fit after reserving
        // 2*2=4 cells for scroll arrows. Active is tab 5 (middle). Both
        // scroll arrows should appear.
        let bar = make_bar(
            (0..10)
                .map(|i| make_tab(&format!("t{i}"), i == 5))
                .collect(),
        );
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(layout.scroll_left.is_some());
        assert!(layout.scroll_right.is_some());
        // Active tab must be among visible.
        let visible_indices: Vec<usize> = layout.visible_tabs.iter().map(|v| v.tab_idx).collect();
        assert!(
            visible_indices.contains(&5),
            "active tab 5 not visible: {:?}",
            visible_indices
        );
        // Scroll offset > 0 (we walked back from the active tab).
        assert!(layout.resolved_scroll_offset > 0);
        // First visible tab starts after the left arrow.
        assert_eq!(layout.visible_tabs[0].bounds.x, 2.0);
    }

    #[test]
    fn tab_bar_layout_overflow_active_at_start() {
        // 10 tabs × 10; bar 30; active is tab 0. Only right arrow needed.
        let bar = make_bar(
            (0..10)
                .map(|i| make_tab(&format!("t{i}"), i == 0))
                .collect(),
        );
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(layout.scroll_left.is_none()); // offset 0
        assert!(layout.scroll_right.is_some()); // tabs to the right
        assert_eq!(layout.resolved_scroll_offset, 0);
    }

    #[test]
    fn tab_bar_layout_overflow_active_at_end() {
        // 10 tabs × 10; bar 30; active is tab 9 (last). Only left arrow.
        let bar = make_bar(
            (0..10)
                .map(|i| make_tab(&format!("t{i}"), i == 9))
                .collect(),
        );
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(layout.scroll_left.is_some());
        assert!(layout.scroll_right.is_none()); // nothing past active
        assert!(layout.resolved_scroll_offset > 0);
        // Active tab is among visible.
        assert!(layout.visible_tabs.iter().any(|v| v.tab_idx == 9));
    }

    #[test]
    fn tab_bar_layout_right_segments_fit() {
        let bar = TabBar {
            id: WidgetId::new("t"),
            tabs: vec![make_tab("a", true)],
            scroll_offset: 0,
            right_segments: vec![
                TabBarSegment {
                    text: " ← ".to_string(),
                    width_cells: 3,
                    id: Some(WidgetId::new("prev")),
                    is_active: false,
                },
                TabBarSegment {
                    text: " → ".to_string(),
                    width_cells: 3,
                    id: Some(WidgetId::new("next")),
                    is_active: false,
                },
            ],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        // Bar 100 wide. Tab 10 wide. Right segments: 6 wide total.
        let layout = bar.layout(
            100.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |i| {
                let w = [3.0, 3.0][i];
                SegmentMeasure::new(w)
            },
        );
        assert_eq!(layout.visible_segments.len(), 2);
        // Right segments start at bar_width - total_width = 100 - 6 = 94.
        assert_eq!(layout.visible_segments[0].bounds.x, 94.0);
        assert_eq!(layout.visible_segments[1].bounds.x, 97.0);
        // Hit-test on first segment.
        let hit = layout.hit_test(95.0, 1.0);
        match hit {
            TabBarHit::RightSegment(id) => assert_eq!(id.as_str(), "prev"),
            other => panic!("expected RightSegment(prev), got {other:?}"),
        }
    }

    #[test]
    fn tab_bar_layout_right_segments_dropped_when_too_wide() {
        let bar = TabBar {
            id: WidgetId::new("t"),
            tabs: vec![make_tab("a", true)],
            scroll_offset: 0,
            right_segments: vec![TabBarSegment {
                text: "many many pixels".to_string(),
                width_cells: 60,
                id: Some(WidgetId::new("huge")),
                is_active: false,
            }],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        };
        // Bar 50 wide. Segment is 60 wide; literally doesn't fit → drop.
        let layout = bar.layout(
            50.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(60.0),
        );
        assert_eq!(layout.visible_segments.len(), 0);
        // No hit-region for the dropped segment.
        for (_, hit) in &layout.hit_regions {
            assert!(!matches!(hit, TabBarHit::RightSegment(_)));
        }

        // But a segment that fits, even narrowly, renders. 60 ≤ 100 → keep.
        let bar2 = bar.clone();
        let layout2 = bar2.layout(
            100.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(60.0),
        );
        assert_eq!(layout2.visible_segments.len(), 1);
    }

    #[test]
    fn tab_bar_layout_stale_scroll_offset_corrected() {
        // App stored scroll_offset 0 but active is tab 9 — layout should
        // correct by returning a non-zero `resolved_scroll_offset`. This is
        // the "write this back to storage" signal for the two-pass-paint
        // pattern.
        let tabs: Vec<_> = (0..10)
            .map(|i| make_tab(&format!("t{i}"), i == 9))
            .collect();
        let mut bar = make_bar(tabs);
        bar.scroll_offset = 0; // stale
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(
            layout.resolved_scroll_offset > 0,
            "expected correction but got {}",
            layout.resolved_scroll_offset
        );
    }

    #[test]
    fn tab_bar_layout_no_arrows_honours_caller_scroll_offset() {
        // When `scroll_arrow_width = 0.0` the primitive defers scroll to the
        // caller — vimcode's TUI computes scroll via
        // `Engine::ensure_active_tab_visible` and writes it to
        // `bar.scroll_offset`. Regression for "newly-opened tabs invisible
        // in TUI when the bar overflows" — without this, the layout
        // collapsed to `resolved_scroll_offset = 0` and clipped the
        // active (rightmost) tab.
        let tabs: Vec<_> = (0..10)
            .map(|i| make_tab(&format!("t{i}"), i == 9))
            .collect();
        let mut bar = make_bar(tabs);
        // Caller has already computed: show tabs starting at 7 so 7,8,9 fit.
        bar.scroll_offset = 7;
        let layout = bar.layout(
            30.0,
            1.0,
            0.0, // no arrows
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.resolved_scroll_offset, 7);
        assert_eq!(layout.visible_tabs.len(), 3);
        assert_eq!(layout.visible_tabs[0].tab_idx, 7);
        assert_eq!(layout.visible_tabs[2].tab_idx, 9);
    }

    #[test]
    fn tab_bar_layout_no_arrows_clamps_oversize_scroll_offset() {
        // Caller-supplied scroll_offset is clamped to a valid index so a
        // stale value (e.g. tab count just shrank) can't push out of range.
        let tabs: Vec<_> = (0..3).map(|i| make_tab(&format!("t{i}"), false)).collect();
        let mut bar = make_bar(tabs);
        bar.scroll_offset = 99;
        let layout = bar.layout(
            30.0,
            1.0,
            0.0,
            |_| TabMeasure::new(50.0, 1.0), // each tab way oversized so overflow path fires
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.resolved_scroll_offset, 2); // tabs.len() - 1
    }

    #[test]
    fn tab_bar_layout_no_close_button_when_close_width_zero() {
        // A pinned / preview tab style — backend passes close_width = 0
        // to suppress the close button.
        let bar = make_bar(vec![make_tab("pinned.rs", true)]);
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 0.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert_eq!(layout.visible_tabs.len(), 1);
        assert!(layout.visible_tabs[0].close_bounds.is_none());
        // Hit-test anywhere on the tab returns Tab(0), never TabClose.
        assert_eq!(layout.hit_test(5.0, 0.5), TabBarHit::Tab(0));
        assert_eq!(layout.hit_test(9.5, 0.5), TabBarHit::Tab(0));
    }

    #[test]
    fn tab_bar_layout_hit_test_outside_bar() {
        let bar = make_bar(vec![make_tab("a", true)]);
        let layout = bar.layout(
            20.0,
            2.0,
            2.0,
            |_| TabMeasure::new(10.0, 2.0),
            |_| SegmentMeasure::new(0.0),
        );
        // Well past the bar.
        assert_eq!(layout.hit_test(100.0, 1.0), TabBarHit::Empty);
        // Below the bar.
        assert_eq!(layout.hit_test(5.0, 100.0), TabBarHit::Empty);
        // Negative coords (robust to weird backends).
        assert_eq!(layout.hit_test(-1.0, 1.0), TabBarHit::Empty);
    }

    #[test]
    fn tab_bar_layout_scroll_disabled_tabs_clip() {
        // scroll_arrow_width = 0.0 disables scroll arrows; tabs that don't
        // fit are silently clipped. Useful for backends that don't want
        // scroll affordances yet.
        let bar = make_bar(
            (0..10)
                .map(|i| make_tab(&format!("t{i}"), i == 0))
                .collect(),
        );
        let layout = bar.layout(
            30.0,
            1.0,
            0.0, // scroll disabled
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        // Exactly 3 tabs fit (no arrow reservation).
        assert_eq!(layout.visible_tabs.len(), 3);
        assert!(layout.scroll_left.is_none());
        assert!(layout.scroll_right.is_none());
    }

    #[test]
    fn tab_bar_layout_tab_center_and_tab_close_center() {
        let bar = make_bar(vec![
            make_tab("a", true),
            make_tab("b", false),
            make_tab("c", false),
        ]);
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 2.0),
            |_| SegmentMeasure::new(0.0),
        );
        // Tab 1 occupies x in [10, 20); center is (15, 0.5).
        assert_eq!(layout.tab_center(1), Some((15.0, 0.5)));
        // Its close button is the trailing 2 cells [18, 20); center (19, 0.5).
        assert_eq!(layout.tab_close_center(1), Some((19.0, 0.5)));
        // Out-of-range index — never a visible tab.
        assert_eq!(layout.tab_center(99), None);
        assert_eq!(layout.tab_close_center(99), None);
    }

    #[test]
    fn tab_bar_layout_tab_center_none_when_scrolled_out_of_view() {
        // Mirrors `tab_bar_layout_overflow_active_at_end`: tab 0 scrolls
        // out of view once tab 9 (active) forces the window rightward.
        let bar = make_bar(
            (0..10)
                .map(|i| make_tab(&format!("t{i}"), i == 9))
                .collect(),
        );
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 1.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(
            !layout.visible_tabs.iter().any(|v| v.tab_idx == 0),
            "tab 0 should have scrolled out of view"
        );
        assert_eq!(
            layout.tab_center(0),
            None,
            "tab_center must be None for a tab hidden behind scroll_offset"
        );
        assert_eq!(layout.tab_close_center(0), None);
    }

    #[test]
    fn tab_bar_layout_tab_close_center_none_when_not_closable() {
        // close_width = 0.0 (mirrors `tab_bar_layout_no_close_button_when_close_width_zero`)
        // models `is_closable: false` / `show_tab_close: false` — the tab
        // itself is still visible, but drew no close button this frame.
        let bar = make_bar(vec![make_tab("pinned.rs", true)]);
        let layout = bar.layout(
            30.0,
            1.0,
            2.0,
            |_| TabMeasure::new(10.0, 0.0),
            |_| SegmentMeasure::new(0.0),
        );
        assert!(layout.tab_center(0).is_some());
        assert_eq!(
            layout.tab_close_center(0),
            None,
            "tab_close_center must be None when the tab drew no close button"
        );
    }

    #[test]
    fn tab_bar_event_roundtrip_serde() {
        let events = vec![
            TabBarEvent::TabActivated { index: 2 },
            TabBarEvent::TabClosed { index: 0 },
            TabBarEvent::ButtonClicked {
                id: WidgetId::new("tab:split_right"),
            },
            TabBarEvent::KeyPressed {
                key: "F1".to_string(),
                modifiers: Modifiers::default(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: TabBarEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    #[test]
    fn tab_icon_codicon_resolves_a_known_name_to_its_bundled_glyph() {
        let icon = TabIcon::codicon("file-code", Color::rgb(222, 165, 132))
            .expect("\"file-code\" is a known codicon name");
        assert_eq!(icon.glyph, "\u{eae9}");
        assert_eq!(icon.color, Color::rgb(222, 165, 132));
    }

    #[test]
    fn tab_icon_codicon_rejects_an_unknown_name() {
        assert_eq!(
            TabIcon::codicon("not-a-real-codicon-name", Color::rgb(0, 0, 0)),
            None
        );
    }

    /// The wire shape a Python/JS app sends for a tab icon: `{"codicon":
    /// "...", "color": ...}` in place of a raw `glyph` string.
    #[test]
    fn tab_icon_deserializes_the_codicon_wire_shape() {
        let icon: TabIcon = serde_json::from_str(
            r#"{"codicon": "file-code", "color": {"r": 222, "g": 165, "b": 132, "a": 255}}"#,
        )
        .expect("codicon wire shape should deserialize");
        assert_eq!(
            icon,
            TabIcon::codicon("file-code", Color::rgb(222, 165, 132)).unwrap()
        );
    }

    /// The pre-existing `{"glyph": ..., "color": ...}` wire shape still
    /// deserializes unchanged.
    #[test]
    fn tab_icon_deserializes_the_glyph_wire_shape_unchanged() {
        let icon: TabIcon =
            serde_json::from_str(r#"{"glyph": "R", "color": {"r": 1, "g": 2, "b": 3, "a": 255}}"#)
                .expect("glyph wire shape should deserialize");
        assert_eq!(
            icon,
            TabIcon {
                glyph: "R".to_string(),
                color: Color::rgb(1, 2, 3),
            }
        );
    }

    #[test]
    fn tab_icon_deserialize_rejects_an_unknown_codicon_name() {
        let result: Result<TabIcon, _> = serde_json::from_str(
            r#"{"codicon": "not-a-real-codicon-name", "color": {"r": 0, "g": 0, "b": 0, "a": 255}}"#,
        );
        assert!(result.is_err());
    }

    // ── fit_tab_label ────────────────────────────────────────────────────

    /// Monospace measurer: one unit per `char`, like a TUI cell count.
    fn mono_measure(s: &str) -> f32 {
        s.chars().count() as f32
    }

    /// Proportional test measurer: a stand-in for Pango/DirectWrite/Core
    /// Text pixel widths — each `char` costs a different width, so a
    /// naive `char`-count budget would mis-measure it.
    fn proportional_measure(s: &str) -> f32 {
        s.chars()
            .map(|c| if "mw".contains(c) { 12.0 } else { 7.0 })
            .sum()
    }

    #[test]
    fn fit_tab_label_returns_the_label_unchanged_when_it_fits() {
        let fitted = fit_tab_label("main.rs", 20.0, 0.0, 2.0, mono_measure);
        assert_eq!(fitted, "main.rs");
    }

    #[test]
    fn fit_tab_label_truncates_with_ellipsis_on_a_monospace_measurer() {
        // tab_width 10, icon slot 0, close slot 2 -> label budget 8 cells.
        let fitted = fit_tab_label("a_very_long_filename.rs", 10.0, 0.0, 2.0, mono_measure);
        assert!(
            fitted.ends_with('…'),
            "overlong label must end in an ellipsis, got {fitted:?}"
        );
        assert!(
            mono_measure(&fitted) <= 10.0 - 2.0,
            "fitted label {fitted:?} must fit the budget excluding the close slot"
        );
    }

    #[test]
    fn fit_tab_label_truncates_with_ellipsis_on_a_proportional_measurer() {
        // Each tab reserves a leading icon slot (14 px) and a trailing
        // close slot (14 px); label budget is whatever's left of
        // `tab_width`.
        let fitted = fit_tab_label("settings.json", 80.0, 14.0, 14.0, proportional_measure);
        assert!(
            fitted.ends_with('…'),
            "overlong label must end in an ellipsis, got {fitted:?}"
        );
        let budget = 80.0 - 14.0 - 14.0;
        assert!(
            proportional_measure(&fitted) <= budget,
            "fitted label {fitted:?} (width {}) must not exceed the {budget} budget",
            proportional_measure(&fitted)
        );
    }

    #[test]
    fn fit_tab_label_never_overlaps_the_close_glyph_slot() {
        // The close slot occupies the tab's last 2 units; the fitted
        // label's measured width must never eat into it.
        for label in [
            "x",
            "main.rs",
            "a_very_long_filename_that_keeps_going.rs",
            "日本語のファイル名.rs",
        ] {
            let fitted = fit_tab_label(label, 12.0, 1.0, 2.0, mono_measure);
            let label_budget = 12.0 - 1.0 - 2.0;
            assert!(
                mono_measure(&fitted) <= label_budget,
                "label {label:?} fitted to {fitted:?} (width {}) overlaps the close slot (budget {label_budget})",
                mono_measure(&fitted)
            );
        }
    }

    #[test]
    fn fit_tab_label_budget_too_small_for_any_text_returns_empty() {
        // icon_width + close_width consume the whole tab; nothing left
        // even for a bare ellipsis.
        let fitted = fit_tab_label("main.rs", 4.0, 2.0, 2.0, mono_measure);
        assert_eq!(fitted, "");
    }

    // ── Keyboard-focusable seam ───────────────────────────────────────────

    #[test]
    fn handle_focus_key_left_right_move_and_clamp() {
        // Nothing focused yet: treated as if tab 0 were current, matching
        // `PipelineView::handle_key`'s identical `unwrap_or(0)` convention.
        assert_eq!(
            TabBar::handle_focus_key(None, 3, "ArrowRight"),
            Some(TabBarEvent::FocusMoved { index: 1 })
        );
        assert_eq!(
            TabBar::handle_focus_key(Some(0), 3, "Right"),
            Some(TabBarEvent::FocusMoved { index: 1 })
        );
        // Clamps at the last tab rather than wrapping or going out of range.
        assert_eq!(
            TabBar::handle_focus_key(Some(2), 3, "ArrowRight"),
            Some(TabBarEvent::FocusMoved { index: 2 })
        );
        // Clamps at 0 going left.
        assert_eq!(
            TabBar::handle_focus_key(Some(0), 3, "ArrowLeft"),
            Some(TabBarEvent::FocusMoved { index: 0 })
        );
        assert_eq!(
            TabBar::handle_focus_key(Some(2), 3, "Left"),
            Some(TabBarEvent::FocusMoved { index: 1 })
        );
    }

    #[test]
    fn handle_focus_key_enter_and_space_activate_the_focused_tab() {
        assert_eq!(
            TabBar::handle_focus_key(Some(1), 3, "Enter"),
            Some(TabBarEvent::TabActivated { index: 1 })
        );
        assert_eq!(
            TabBar::handle_focus_key(Some(1), 3, "Space"),
            Some(TabBarEvent::TabActivated { index: 1 })
        );
        assert_eq!(
            TabBar::handle_focus_key(Some(1), 3, " "),
            Some(TabBarEvent::TabActivated { index: 1 })
        );
        // No tab focused yet: Enter/Space have nothing to activate.
        assert_eq!(TabBar::handle_focus_key(None, 3, "Enter"), None);
    }

    #[test]
    fn handle_focus_key_ignores_unrelated_keys_and_empty_bars() {
        assert_eq!(TabBar::handle_focus_key(Some(0), 3, "Tab"), None);
        assert_eq!(TabBar::handle_focus_key(Some(0), 0, "ArrowRight"), None);
        assert_eq!(TabBar::handle_focus_key(Some(0), 0, "Enter"), None);
    }

    #[test]
    fn tab_chrome_with_focused_index_round_trips_through_builder_and_serde() {
        let chrome = TabChrome::new(TabFrame::None).with_focused_index(Some(2));
        assert_eq!(chrome.focused_index, Some(2));

        let json = serde_json::to_string(&chrome).unwrap();
        let back: TabChrome = serde_json::from_str(&json).unwrap();
        assert_eq!(chrome, back);

        // Omitting the field entirely (an older wire payload) decodes to
        // `None`, not a hard error — the additive-knob contract.
        let legacy: TabChrome = serde_json::from_str(r#"{"active_frame":"None"}"#).unwrap();
        assert_eq!(legacy.focused_index, None);
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[cfg(test)]
mod primitive_parity_tests {
    use super::*;
    use crate::backend::ImagePaintResult;
    use crate::event::{Point, Viewport};
    use crate::primitive::{LayoutCtx, Primitive};
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Image;
    use crate::PaintSurface;
    use crate::PathVerb;

    /// Records every verb call as a formatted line, in call order — a
    /// byte-for-byte equal log from two different call paths is strong
    /// evidence they painted the same thing. Mirrors
    /// `crate::primitives::list::primitive_parity_tests::LogSurface`.
    #[derive(Default)]
    struct LogSurface {
        calls: Vec<String>,
    }

    impl PaintSurface for LogSurface {
        fn surface_begin_frame(&mut self, _viewport: Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> Viewport {
            Viewport::new(400.0, 100.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            24.0
        }
        fn surface_char_width(&self) -> f32 {
            8.0
        }
        fn surface_measure_text(&self, text: &str) -> (f32, f32) {
            (text.chars().count() as f32 * 8.0, 14.0)
        }
        fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
            self.calls.push(format!("fill_rect {rect:?} {color:?}"));
        }
        fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
            self.calls
                .push(format!("fill_rounded_rect {rect:?} {radius} {color:?}"));
        }
        fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
            self.calls
                .push(format!("stroke_rect {rect:?} {color:?} {stroke_width}"));
        }
        fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
            self.calls
                .push(format!("draw_text_run {rect:?} {text:?} {color:?}"));
        }
        fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
            self.calls.push(format!(
                "draw_line {from:?} {to:?} {color:?} {stroke_width}"
            ));
        }
        fn surface_draw_path(&mut self, verbs: &[PathVerb], color: Color, stroke_width: f32) {
            self.calls
                .push(format!("draw_path {verbs:?} {color:?} {stroke_width}"));
        }
        fn surface_push_clip(&mut self, rect: Rect) {
            self.calls.push(format!("push_clip {rect:?}"));
        }
        fn surface_pop_clip(&mut self) {
            self.calls.push("pop_clip".to_string());
        }
        fn surface_draw_image(&mut self, rect: Rect, _image: &Image) -> ImagePaintResult {
            self.calls.push(format!("draw_image {rect:?}"));
            ImagePaintResult::Unsupported
        }
    }

    fn sample_bar() -> TabBar {
        TabBar {
            id: WidgetId::new("tabs:parity"),
            tabs: vec![
                TabItem {
                    label: "main.rs".into(),
                    is_active: true,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                },
                TabItem {
                    label: "lib.rs".into(),
                    is_active: false,
                    is_dirty: true,
                    is_preview: false,
                    is_closable: true,
                },
            ],
            right_segments: vec![],
            active_accent: None,
            scroll_offset: 0,
            show_tab_close: true,
            compact: false,
        }
    }

    /// The acceptance criterion: `TabBar`'s [`Primitive::layout`] +
    /// [`Primitive::paint`] must produce the exact same verb log as
    /// laying out via [`crate::primitives::layout_metrics::pixel_tab_bar_layout`]
    /// and calling [`native_surface_paint::paint`] directly with the
    /// equivalent, translated arguments — including, crucially, a `rect`
    /// whose `x` is **not** `0.0`, which is what pins the background
    /// fill at `layout.area`'s own origin rather than the surface's.
    #[test]
    fn tab_bar_primitive_paint_matches_native_surface_paint_at_a_nonzero_rect_x() {
        let bar = sample_bar();
        let rect = Rect::new(37.0, 5.0, 220.0, 24.0);
        let ctx = LayoutCtx::new(24.0, 8.0, false);
        let theme = Theme::default();
        let style = Style::default();

        let via_primitive_layout = Primitive::layout(&bar, &ctx, rect);
        let mut via_primitive_surface = LogSurface::default();
        Primitive::paint(
            &bar,
            &via_primitive_layout,
            &mut via_primitive_surface,
            &theme,
            &style,
        );

        let measure = crate::primitives::layout_metrics::CharWidthMeasure(ctx.char_width);
        let tab_name_widths: Vec<f32> = bar
            .tabs
            .iter()
            .map(|t| crate::primitives::layout_metrics::TextMeasure::width_of(&measure, &t.label))
            .collect();
        let tab_icon_extras = vec![0.0f32; bar.tabs.len()];
        let chrome = TabChrome::default();
        let (direct_layout, _corrected_scroll_offset, _available_cols) =
            crate::primitives::layout_metrics::pixel_tab_bar_layout(
                &bar,
                rect.width,
                rect.height,
                crate::primitives::layout_metrics::pixel::TAB_PAD,
                crate::primitives::layout_metrics::pixel::TAB_INNER_GAP,
                crate::primitives::layout_metrics::pixel::TAB_OUTER_GAP,
                &tab_name_widths,
                &tab_icon_extras,
                &chrome,
                &measure,
            );
        let translated = translate_tab_bar_layout(direct_layout, rect.x, rect.y);
        let mut direct_surface = LogSurface::default();
        native_surface_paint::paint(
            &bar,
            &translated,
            &[],
            None,
            ctx.nerd_fonts_enabled,
            &TabChrome::default(),
            native_surface_paint::TabBarPaintState::default(),
            &mut direct_surface,
            &theme,
            &style,
            rect.x,
            rect.y,
        );

        assert!(
            !via_primitive_surface.calls.is_empty(),
            "the bar must paint something for this comparison to be meaningful"
        );
        assert_eq!(via_primitive_surface.calls, direct_surface.calls);

        // Pin the exact bug this test guards: the background fill must
        // start at `rect.x`, not `0.0`.
        let bg_fill = via_primitive_surface
            .calls
            .iter()
            .find(|c| c.contains(&format!("{:?}", theme.tab_bar_bg)) && c.starts_with("fill_rect"))
            .expect("the bar must fill a background rect");
        assert!(
            bg_fill.contains(&format!("x: {:?}", rect.x)),
            "the background fill must start at rect.x ({}); call was {bg_fill:?}",
            rect.x,
        );
    }
}

/// GTK-backed proof that the shared `Primitive::layout` and the GTK
/// rasteriser agree on where each tab sits — the whole point of
/// `LayoutCtx::text_width`: both measure the same labels with the same
/// Pango font instead of one guessing from `char_width`.
#[cfg(all(test, feature = "gtk"))]
mod gtk_measure_parity_tests {
    use super::*;
    use crate::backend::{Backend, DrawsPrimitives};
    use crate::gtk::backend::GtkBackend;
    use crate::primitive::Primitive;
    use crate::style::Style;
    use pangocairo::cairo::{Context, Format, ImageSurface};
    use pangocairo::pango;

    /// Labels whose Pango widths diverge from `char_width * chars` in
    /// *both* directions, so the comparison below cannot pass on a
    /// scalar-corrected approximation.
    fn parity_bar() -> TabBar {
        TabBar {
            id: WidgetId::new("tabs:gtk-parity"),
            tabs: ["alpha", "beta", "gamma"]
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == 0,
                    ..Default::default()
                })
                .collect(),
            right_segments: vec![],
            active_accent: None,
            scroll_offset: 0,
            show_tab_close: true,
            compact: false,
        }
    }

    fn spans(tabs: &[VisibleTab]) -> Vec<(usize, f32, f32)> {
        tabs.iter()
            .map(|vt| (vt.tab_idx, vt.bounds.x, vt.bounds.x + vt.bounds.width))
            .collect()
    }

    /// `draw_primitive` → `TabBar::layout` must resolve the same per-tab
    /// x-ranges as `Backend::draw_tab_bar_layout` for the same bar and
    /// rect on a live Pango context.
    ///
    /// `tab_padding_x` is set to `pixel::TAB_PAD` because the two paths
    /// read that one metric from different places — the rasteriser from
    /// `Style::tab_padding_x`, the shared layout from the
    /// `layout_metrics::pixel` constant, since `LayoutCtx` carries no
    /// `Style` — and this test is about the glyph widths, not that gap.
    #[test]
    fn primitive_layout_tab_spans_match_the_gtk_rasteriser() {
        let bar = parity_bar();
        let rect = Rect::new(0.0, 0.0, 600.0, 35.0);

        let surface = ImageSurface::create(Format::ARgb32, 600, 35).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_ctx = pangocairo::functions::create_context(&cr);
        let pango_layout = pango::Layout::new(&pango_ctx);

        let mut backend = GtkBackend::new();
        // The measurer `draw_primitive` attaches reads this context, not
        // the per-frame one, so `Backend::measure_text` resolves real
        // Pango advances here instead of its pre-realise estimate.
        backend.set_pango_context(pango_ctx.clone());
        Backend::set_style(
            &mut backend,
            Style::default().with_tab_padding_x(crate::primitives::layout_metrics::pixel::TAB_PAD),
        );

        let (rasterised, widget) = backend.enter_frame_scope(&cr, &pango_layout, |b| {
            let rasterised = Backend::draw_tab_bar_layout(b, rect, &bar, None);
            let widget = b.draw_primitive(rect, &bar);
            (rasterised, widget)
        });

        assert_eq!(
            spans(&widget.layout.visible_tabs),
            spans(&rasterised.visible_tabs),
            "TabBar::layout through draw_primitive must place tabs where the GTK \
             rasteriser places them — both measure labels with Backend::measure_text"
        );
        assert_eq!(
            widget.layout.visible_tabs.len(),
            bar.tabs.len(),
            "all three tabs must fit in a 600px strip for the comparison to cover them"
        );
    }

    /// The parity above is not vacuous: the same layout built from a
    /// measurer-free `LayoutCtx` (the `char_width` approximation) does
    /// *not* match the rasteriser on these labels.
    #[test]
    fn the_char_width_approximation_disagrees_with_the_gtk_rasteriser() {
        let bar = parity_bar();
        let rect = Rect::new(0.0, 0.0, 600.0, 35.0);

        let surface = ImageSurface::create(Format::ARgb32, 600, 35).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_ctx = pangocairo::functions::create_context(&cr);
        let pango_layout = pango::Layout::new(&pango_ctx);

        let mut backend = GtkBackend::new();
        backend.set_pango_context(pango_ctx.clone());
        Backend::set_style(
            &mut backend,
            Style::default().with_tab_padding_x(crate::primitives::layout_metrics::pixel::TAB_PAD),
        );

        let (rasterised, approximated) = backend.enter_frame_scope(&cr, &pango_layout, |b| {
            let rasterised = Backend::draw_tab_bar_layout(b, rect, &bar, None);
            let ctx = crate::primitive::LayoutCtx::new(
                crate::primitive::LayoutCtx::resolve_row_height(
                    Backend::chrome_line_height(b),
                    &Backend::style(b),
                ),
                Backend::char_width(b),
                Backend::nerd_fonts_enabled(b),
            );
            let approximated = Primitive::layout(&bar, &ctx, rect);
            (rasterised, approximated)
        });

        assert_ne!(
            spans(&approximated.layout.visible_tabs),
            spans(&rasterised.visible_tabs),
            "if the char_width approximation already matched Pango on these labels, the \
             parity test above would prove nothing"
        );
    }
}
