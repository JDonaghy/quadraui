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
//! **No backend calls it yet.** `gtk::tab_bar`, `macos::tab_bar` and
//! `win::tab_bar` each still carry their own full Cairo / Core Graphics /
//! Direct2D paint loop, and the three copies have already drifted:
//!
//! | Behaviour | GTK | macOS | Windows |
//! |---|---|---|---|
//! | Active-tab background | rounded, y-inset chip (radius 4, 4 px inset) | rounded, y-inset chip (radius 4, 4 px inset) | full-height square rect |
//! | Active-tab top accent | 1 px, on the chip's inset top edge | 1 px, on the chip's inset top edge | 2 px, on the strip's top edge |
//! | Close-button hover backdrop | rounded, `foreground` @ 15 % alpha | **none painted** | square, `tab_bar_bg.lighten(0.15)` |
//! | Preview-tab label | italic + `tab_preview_*_fg` | upright + `tab_preview_*_fg` | upright, **`tab_preview_*_fg` ignored** |
//! | [`TabFrame::Brackets`] framing | painted | **not painted** | **not painted** |
//!
//! Migrating each backend onto [`native_surface_paint::paint`] is a
//! separate, per-backend issue — this module only adds the shared
//! painter itself.
//!
//! ## Why each backend still carries its own loop
//!
//! GTK and Windows resolve geometry through the shared
//! [`crate::primitives::layout_metrics::pixel_tab_bar_layout`],
//! but macOS still resolves it in two bespoke, macOS-only builders
//! that use a **different `close_bounds` convention**:
//! macOS sizes the close box as `close_glyph_w + 2 * CLOSE_PAD` centred
//! on the glyph, while `pixel_tab_bar_layout` sizes it as
//! `tab_inner_gap + close_glyph_w` starting one inner gap *before* the
//! glyph. [`native_surface_paint::paint`] sidesteps the mismatch by
//! treating [`VisibleTab::close_bounds`] as an opaque "glyph lives
//! somewhere in here" box and centring the glyph inside it, rather than
//! assuming either backend's leading-gap convention — so a caller on
//! either convention gets a correctly centred glyph without this module
//! having to pick a side.
//!
//! Unifying the two backends' close-button **hit** geometry so macOS also
//! resolves through `pixel_tab_bar_layout` remains a separate effort from
//! wiring each backend onto this paint module — see
//! [`native_surface_paint`] for what the shared painter does and does
//! not yet cover (no [`TabFrame::Brackets`] framing).
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
    /// focus-visible state (e.g. [`crate::compose::focus_ring::FocusRing`]):
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
/// This is the stable return type of `draw_tab_bar` / `tab_bar_layout`
/// and their icon/chrome twins, not a transitional shape scheduled for
/// removal. [`TabBarLayout`] — f32, `Rect`/[`TabBarHit`]-based, unit
/// agnostic — is a parallel addition the `*_layout`-suffixed methods and
/// [`crate::primitives::layout_metrics::pixel_tab_bar_layout`] return, not
/// a migration target for this one: `vimcode` depends directly on this
/// struct's exact `f64` field shapes, and two of the four in-tree
/// rasterisers construct it without ever computing an intermediate
/// `TabBarLayout`. Both return types coexist; a caller picks whichever
/// coordinate convention and unit fits — target-surface-absolute `f64`
/// pairs here, bar-relative `Rect`s there.
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
    use super::{fit_tab_label, tab_icon_at, TabBar, TabBarGlyphs, TabBarLayout, TabIcon};
    use crate::backend::PressableVisual;
    use crate::event::{Point, Rect};
    use crate::paint_surface::{PaintSurface, PathVerb};
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::Color;

    /// Vertical margin (surface-native units) above and below the
    /// active-tab chip fill — mirrors `gtk::tab_bar`'s pre-migration
    /// `TAB_CHIP_INSET_Y`.
    const TAB_CHIP_INSET_Y: f32 = 4.0;
    /// Corner radius of the active-tab chip — mirrors `gtk::tab_bar`'s
    /// pre-migration `TAB_CHIP_RADIUS`.
    const TAB_CHIP_RADIUS: f32 = 4.0;
    /// Height of the active-tab top-edge accent line — mirrors
    /// `gtk::tab_bar`'s pre-migration `TAB_ACTIVE_BORDER_TOP_PX`.
    const TAB_ACTIVE_BORDER_TOP: f32 = 1.0;
    /// Gap between a tab's leading icon glyph and its label — mirrors
    /// `gtk::tab_bar`'s own `TAB_ICON_GAP`.
    const TAB_ICON_GAP: f32 = 6.0;
    /// Padding, on every side, of the close-button hover backdrop
    /// inside [`super::VisibleTab::close_bounds`].
    const CLOSE_HOVER_PAD: f32 = 2.0;
    /// Corner radius of the close-button hover backdrop.
    const CLOSE_HOVER_RADIUS: f32 = 3.0;
    /// Alpha (0.0..=1.0) of the close-button hover backdrop, painted in
    /// `theme.foreground` — mirrors `gtk::tab_bar`'s pre-migration
    /// `0.15` constant.
    const CLOSE_HOVER_ALPHA: f32 = 0.15;

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
    /// # Visual contract
    ///
    /// - **Background:** the full bar rect fills with `theme.tab_bar_bg`.
    /// - **Active tab:** an inset, rounded chip
    ///   (`theme.tab_active_bg`) rather than a full-height square block;
    ///   `bar.active_accent`, when set, paints a thin top-edge accent
    ///   line on the chip's own inset top edge.
    /// - **Whole-tab hover/pressed:** tints the tab's background toward
    ///   `theme.list_hover_bg`/`theme.list_pressed_bg` — pressed wins —
    ///   independent of the close glyph's own hover state below.
    /// - **Label:** ellipsis-fitted via [`fit_tab_label`] into the space
    ///   left after the leading icon (if any) and the close-button
    ///   region (if any); italic when [`super::TabItem::is_preview`].
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
    /// [`crate::Backend::nerd_fonts_enabled`]. `TabFrame::Brackets`
    /// framing is not yet painted here — see the module doc.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        bar: &TabBar,
        layout: &TabBarLayout,
        icons: &[Option<TabIcon>],
        glyphs: Option<&TabBarGlyphs>,
        nerd_fonts_enabled: bool,
        state: TabBarPaintState,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
    ) {
        if layout.bar_width <= 0.0 || layout.bar_height <= 0.0 {
            return;
        }

        surface.surface_fill_rect(
            Rect::new(0.0, 0.0, layout.bar_width, layout.bar_height),
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

            if tab.is_active {
                let chip = Rect::new(
                    bounds.x,
                    bounds.y + TAB_CHIP_INSET_Y,
                    bounds.width,
                    (bounds.height - 2.0 * TAB_CHIP_INSET_Y).max(0.0),
                );
                surface.surface_fill_rounded_rect(chip, TAB_CHIP_RADIUS, tint(theme.tab_active_bg));
                if let Some(accent) = bar.active_accent {
                    surface.surface_fill_rect(
                        Rect::new(chip.x, chip.y, chip.width, TAB_ACTIVE_BORDER_TOP),
                        accent,
                    );
                }
            } else {
                surface.surface_fill_rect(bounds, tint(theme.tab_bar_bg));
            }

            let fg = match (tab.is_active, tab.is_preview) {
                (true, true) => theme.tab_preview_active_fg,
                (true, false) => theme.tab_active_fg,
                (false, true) => theme.tab_preview_inactive_fg,
                (false, false) => theme.tab_inactive_fg,
            };

            let mut cursor_x = bounds.x + style.tab_padding_x;

            if let Some(icon) = tab_icon_at(icons, vt.tab_idx) {
                let (icon_w, _) = surface.surface_measure_text(&icon.glyph);
                surface.surface_draw_icon_glyph(
                    Rect::new(cursor_x, text_y, icon_w, line_height),
                    &icon.glyph,
                    icon.color,
                );
                cursor_x += icon_w + TAB_ICON_GAP;
            }

            let label_right_edge = match vt.close_bounds {
                Some(cb) => cb.x,
                None => bounds.x + bounds.width - style.tab_padding_x,
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

            if let Some(cb) = vt.close_bounds {
                let is_close_hovered = state.hovered_close_tab == Some(vt.tab_idx);
                if is_close_hovered {
                    let backdrop = Rect::new(
                        cb.x + CLOSE_HOVER_PAD,
                        text_y + CLOSE_HOVER_PAD,
                        (cb.width - 2.0 * CLOSE_HOVER_PAD).max(0.0),
                        (line_height - 2.0 * CLOSE_HOVER_PAD).max(0.0),
                    );
                    surface.surface_fill_rounded_rect(
                        backdrop,
                        CLOSE_HOVER_RADIUS,
                        theme.foreground.with_alpha(CLOSE_HOVER_ALPHA as f64),
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
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
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
                state,
                &mut surface,
                &theme,
                &Style::default(),
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
                state,
                &mut surface,
                &theme,
                &Style::default(),
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
                state,
                &mut surface,
                &theme,
                &Style::default(),
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
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
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
                state,
                &mut surface,
                &theme,
                &Style::default(),
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
            assert!(
                surface
                    .rounded_fills
                    .iter()
                    .any(|(_, _, c)| *c == theme.foreground.with_alpha(0.15)),
                "a hovered close glyph must paint a rounded backdrop in \
                 foreground @ 15% alpha; rounded_fills were {:?}",
                surface.rounded_fills,
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
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
            );

            assert_eq!(
                surface.paths.len(),
                2,
                "both scroll arrows must paint a chevron path; paths were {:?}",
                surface.paths,
            );
        }

        /// A zero-size layout (e.g. a collapsed split with no width)
        /// paints nothing rather than panicking on a negative/NaN rect.
        #[test]
        fn zero_size_layout_paints_nothing() {
            let b = bar(vec![tab("a")]);
            let layout = TabBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_tabs: vec![],
                visible_segments: vec![],
                scroll_left: None,
                scroll_right: None,
                hit_regions: vec![],
                resolved_scroll_offset: 0,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &b,
                &layout,
                &[],
                None,
                true,
                TabBarPaintState::default(),
                &mut surface,
                &theme,
                &Style::default(),
            );

            assert!(surface.fills.is_empty());
            assert!(surface.rounded_fills.is_empty());
        }
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
