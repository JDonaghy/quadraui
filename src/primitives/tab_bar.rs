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
/// hit region at the right end of the tab; `0.0` means the tab has no
/// close button (e.g. a pinned tab).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabMeasure {
    pub total_width: f32,
    pub close_width: f32,
}

impl TabMeasure {
    pub fn new(total_width: f32, close_width: f32) -> Self {
        Self {
            total_width,
            close_width,
        }
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
}

/// Per-frame interaction-state output from a tab-bar rasteriser. All
/// positions are in target-surface coordinates.
///
/// Apps consume this to dispatch clicks. Tabs before the
/// scroll offset get sentinel entries so indices in `slot_positions`
/// / `close_bounds` line up with `bar.tabs`.
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
            let close_bounds = if tm.close_width > 0.0 && tm.close_width <= tm.total_width {
                Some(Rect::new(
                    cursor_x + tm.total_width - tm.close_width,
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
                },
                TabItem {
                    label: "lib.rs".into(),
                    is_active: false,
                    is_dirty: false,
                    is_preview: false,
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
                other => panic!("click at col {col} expected RightSegment({expected}), got {other:?}"),
            }
        }

        // Click on change_label (col 56) should be Empty (no id).
        match layout.hit_test(56.0, 0.0) {
            TabBarHit::Empty => {}
            other => panic!("click at col 56 (change_label) expected Empty, got {other:?}"),
        }
    }
}
