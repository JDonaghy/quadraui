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
//!
//! # Backend contract
//!
//! **`TabBar` has measurement-dependent state and a non-trivial backend
//! contract.** Skipping any step makes the active tab land off-screen
//! after layout changes (window resize, new file open, scroll-to). This
//! is the bug class we hit hardest in vimcode (issue #158, 5 commits to
//! find the right architecture).
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
    /// When `false`, per-tab close buttons (×/●) are suppressed — the
    /// measurer returns `close_width: 0.0` and rasterisers skip the glyph.
    /// Defaults to `true` for backward compatibility with file-tab bars.
    #[serde(default = "default_true")]
    pub show_tab_close: bool,
    /// When `true`, the GTK rasteriser uses minimal padding (2px) and
    /// zero gap between tabs. Intended for compact chrome like terminal
    /// toolbar tabs or bottom panel tab switchers. TUI is unaffected
    /// (it already renders labels with no padding).
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
/// # Why icons ride *beside* the bar, not inside [`TabItem`] (#620)
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    ///
    /// Added under #620 so a caller can write
    /// `TabItem { label, ..Default::default() }` instead of listing
    /// every field; that also means any *future* field addition here
    /// costs `..Default::default()` callers nothing.
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
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6 in `docs/BACKEND_TRAIT_PROPOSAL.md` §9: primitives return
// fully-resolved `Layout` structs; backends rasterise verbatim. A backend
// that fails to consume a field (e.g. doesn't iterate `visible_tabs`)
// produces visibly broken output on its own platform — not silent
// divergence on the next one. Tab-bar layout is the reference
// implementation of this pattern (closes #179).
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
    /// wrapping it. `0.0` (the default via [`Self::new`]) reproduces the
    /// pre-#631 behaviour where the close region is flush against the
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
    /// overlapping it. Added for #631 ([`TabChrome::active_frame`]).
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
    /// `GtkDriver::tab_center` (quadraui#594), which every tab bar needs
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

/// Active-tab framing vocabulary (#631): which decoration, if any, should
/// enclose the active tab's full content — label *and* close glyph.
///
/// Before this existed, "enclosing" decoration was whatever a backend's
/// active-tab background fill happened to cover; a consumer that wanted an
/// explicit bracket-style frame (`[title ×]`) had no declarative way to ask
/// for it, because [`TabBar::layout`]'s close region is always flush
/// against the tab's right edge — there's no way for chrome painted
/// *after* the close glyph to exist without [`TabMeasure::trailing_width`]
/// telling `close_bounds` to stop short of it. `coord-tui`'s
/// `doc_tab_label` baked `[`/`]` directly into `TabItem::label` and found
/// the close glyph by scanning the string for exactly this reason (see
/// that function's doc comment).
///
/// `#[non_exhaustive]`: brand new this PR, so marking it costs no consumer
/// anything today, and a future frame style (e.g. a stroked border) is
/// additive rather than breaking.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TabFrame {
    /// No enclosing decoration beyond the tab's ordinary active
    /// background — the pre-#631 behaviour on every backend.
    #[default]
    None,
    /// Encloses the active tab's full content — icon, label, and close
    /// glyph — in bracket framing. TUI paints literal `[` / `]` glyphs
    /// around the tab's content; GTK mirrors them with Pango-measured
    /// bracket glyphs so the two backends agree on what "enclosing" means.
    Brackets,
}

/// Per-bar chrome request (#631): which active-tab frame a backend should
/// paint. Passed alongside a [`TabBar`] to
/// [`crate::Backend::draw_tab_bar_with_chrome`] /
/// [`crate::Backend::tab_bar_layout_with_chrome`].
///
/// A separate sidecar value rather than a field on [`TabBar`] itself, for
/// the same reason [`crate::TooltipChrome`] sits beside [`crate::Tooltip`]
/// instead of inside it: `TabBar` is a plain, non-`#[non_exhaustive]`
/// struct built with exhaustive literals throughout this repo and by
/// downstream consumers, so a new required field would be a hard break for
/// every one of them (`PRIMITIVE_RULES.md` rule 8). Threading the
/// vocabulary through a new value + new trait methods (both given default
/// bodies that ignore chrome and delegate to the plain `draw_tab_bar` /
/// `tab_bar_layout`) means #631 adds no required field and breaks no
/// existing `Backend` implementor or call site.
///
/// `#[non_exhaustive]`: brand new this PR, so marking it costs no consumer
/// anything today, and a future chrome knob (e.g. a frame colour override)
/// is additive. Construct with [`TabChrome::new`] /
/// [`TabChrome::default`] and [`TabChrome::with_active_frame`]; the field
/// stays `pub` for reading.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TabChrome {
    /// Which frame to paint around the active tab. Defaults to
    /// [`TabFrame::None`] — no change from pre-#631 behaviour.
    #[serde(default)]
    pub active_frame: TabFrame,
}

impl TabChrome {
    /// Chrome requesting the given active-tab frame.
    pub fn new(active_frame: TabFrame) -> Self {
        Self { active_frame }
    }

    /// Set the active-tab frame.
    pub fn with_active_frame(mut self, active_frame: TabFrame) -> Self {
        self.active_frame = active_frame;
        self
    }
}

/// Per-frame interaction-state output from a tab-bar rasteriser. All
/// positions are in target-surface coordinates.
///
/// Apps consume this to dispatch clicks. Tabs before the
/// scroll offset get sentinel entries so indices in `slot_positions`
/// / `close_bounds` line up with `bar.tabs`.
///
/// # Legacy `f64` coordinates (issue #504)
///
/// Every other hit/layout struct in this crate (`TabBarLayout`,
/// `ActivityBarRowHit`, `StatusBarLayout`, …) uses `f32` — the crate's
/// native-unit convention (`Point`, `Rect`). This one still uses `f64`
/// pairs and `available_cols: usize` in **character columns** (a TUI-only
/// concept even GTK/macOS fake by measuring a Pango sample string),
/// because it predates that convention and [`TabBarLayout`] — the
/// intended f32-native replacement — didn't exist yet.
///
/// It stays this way rather than being fixed in place because `vimcode`
/// (`src/core/engine/mod.rs`, `src/core/engine/terminal_ops.rs`,
/// `src/gtk/mod.rs`) destructures these fields directly as `f64`, and
/// CLAUDE.md's downstream-consumers policy forbids a hard break — a type
/// change here needs the same two-PR deprecate-then-remove protocol as
/// [`crate::backend::EditorPaintResult::cursor_position`], except across
/// six `Backend` trait methods (`draw_tab_bar`, `draw_tab_bar_icons`,
/// `draw_tab_bar_with_chrome`, `tab_bar_layout`, `tab_bar_layout_icons`,
/// `tab_bar_layout_with_chrome`) and four backends, two of which
/// (`macos::tab_bar`, `win::tab_bar`) construct this struct directly with
/// no intermediate `TabBarLayout` to source native `f32` coordinates
/// from. That is real, separate follow-up work, not something this PR's
/// pass over `EditorPaintResult`/`ActivityBarRowHit` could fold in.
///
/// The one piece of this struct's *legacy-ness* this crate can and does
/// fix without breaking anyone: the converter that constructs it,
/// [`crate::backend::tab_bar_layout_to_hits`], is deprecated in favour of
/// [`crate::backend::tab_bar_hits_from_layout`] (same body, new name, zero
/// remaining in-repo callers of the old one).
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
                    hit_regions.push((bounds, TabBarHit::RightSegment(id.clone())));
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

    // ─── quadraui#594 ────────────────────────────────────────────────────

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
}
