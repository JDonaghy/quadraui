//! `TreeView` primitive: hierarchical rows with expand/collapse, optional
//! icons, styled text, badges, and keyboard-driven selection.
//!
//! Trees are pre-flattened by the app: each `TreeRow` carries its
//! `TreePath`, visual `indent`, and an `is_expanded` flag (`None` for
//! leaves). Backends iterate `rows` in order; the primitive does not store
//! tree structure of its own. This keeps the data model plain and
//! plugin-friendly while letting apps control exactly which rows are
//! visible at any given frame.
//!
//! # Backend contract
//!
//! **Purely declarative** — render `rows[scroll_offset..]` until the
//! viewport is full. Click → row index → emit `TreeEvent::RowActivated`
//! with the row's `path`. Keyboard navigation (`j`/`k`/`h`/`l`/`Enter`)
//! emits the corresponding event; the *app* updates `selected_path` and
//! `scroll_offset` for the next frame.
//!
//! No measurement-dependent state — backends pick a uniform row height
//! (often `line_height` for leaves, `line_height * 1.4` for branches in
//! GUI backends, exactly `1` cell for TUI). Per-backend row cadence is
//! allowed; the primitive only constrains data shape.
//! [`TreeStyle::row_height`](crate::types::TreeStyle::row_height) lets a
//! host pin that cadence to a fixed value instead of letting it float
//! with the editor font — see #623.
//!
//! Apps that need "scroll selection into view" do it themselves by
//! adjusting `scroll_offset` based on the selected row's flat index and
//! the viewport row count.

use crate::event::Rect;
use crate::primitives::scrollbar::Scrollbar;
use crate::types::{
    Badge, Decoration, Icon, Modifiers, SelectionMode, StyledText, TreePath, TreeStyle, WidgetId,
};
use serde::{Deserialize, Serialize};

/// Declarative description of a `TreeView` widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeView {
    pub id: WidgetId,
    /// Pre-flattened, pre-expanded rows in visual order.
    pub rows: Vec<TreeRow>,
    pub selection_mode: SelectionMode,
    pub selected_path: Option<TreePath>,
    /// How many rows have been scrolled past (app-owned in v1; primitive-owned
    /// scroll state with `ScrollState::id(widget_id)` is a later stage per
    /// `docs/UI_CRATE_DESIGN.md` §3.1).
    #[serde(default)]
    pub scroll_offset: usize,
    pub style: TreeStyle,
    #[serde(default)]
    pub has_focus: bool,
}

/// One visible row in a `TreeView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeRow {
    pub path: TreePath,
    /// Visual indent level in `style.indent` units. Usually equals
    /// `path.len() - 1` but apps may flatten (e.g. show a child as indent 0
    /// when rendering a subtree in isolation).
    pub indent: u16,
    pub icon: Option<Icon>,
    pub text: StyledText,
    /// Right-aligned status indicator (e.g. git status letter, item count).
    pub badge: Option<Badge>,
    /// `None` marks a leaf; `Some(true)` marks an expanded branch;
    /// `Some(false)` marks a collapsed branch.
    pub is_expanded: Option<bool>,
    #[serde(default)]
    pub decoration: Decoration,
    /// When `Some`, backends render an inline text input in place of
    /// `text` and `badge`. The row's indent, icon, and chevron are
    /// still rendered normally.
    #[serde(default)]
    pub edit: Option<TreeRowEditState>,
}

/// Inline editing state for a tree row. When present on a `TreeRow`,
/// backends render a text input in place of the normal row label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeRowEditState {
    pub text: String,
    /// Cursor position as a byte offset into `text`.
    pub cursor: usize,
    /// Selection anchor as a byte offset. When `Some(n)` and `n != cursor`,
    /// the range between anchor and cursor is selected.
    pub selection_anchor: Option<usize>,
    /// Shown in muted style when `text` is empty (e.g. "New file name...").
    #[serde(default)]
    pub placeholder: Option<String>,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6 in `docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §9: primitives return
// fully-resolved `Layout` structs; backends rasterise verbatim. Third
// primitive to gain the new shape after `TabBar` and `StatusBar`. TreeView
// is purely vertical — rows stack from `scroll_offset` until the viewport
// fills. Sub-row layout (chevron / icon / text / badge positions within a
// row) is still backend-owned in v1 because each backend has native
// conventions for those elements (see the A.1c lesson in PLAN.md: "When
// porting a primitive's draw function to a new backend, match the new
// backend's pre-migration row cadence, not the other backend's").

/// Per-row measurement supplied by the backend.
///
/// `height` is the row's height in the backend's native unit — 1 cell for
/// TUI, `line_height` or `line_height * 1.4` for GTK (leaves vs branches),
/// similar for other native backends.
///
/// `chevron_end_x` — when `Some(w)` and the row has `is_expanded.is_some()`,
/// [`TreeView::layout`] splits the row's hit region into a
/// [`TreeViewHit::Chevron`] zone for `x ∈ [0, w)` and a
/// [`TreeViewHit::Row`] zone for the remainder. Backends set this to the
/// x coordinate (in tree-local units) where the painted chevron ends.
/// `None` means no chevron split (leaf rows, or `show_chevrons = false`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeRowMeasure {
    pub height: f32,
    pub chevron_end_x: Option<f32>,
}

impl TreeRowMeasure {
    pub fn new(height: f32) -> Self {
        Self {
            height,
            chevron_end_x: None,
        }
    }

    /// Convenience constructor: row with an explicit chevron boundary.
    pub fn with_chevron(height: f32, chevron_end_x: f32) -> Self {
        Self {
            height,
            chevron_end_x: Some(chevron_end_x),
        }
    }
}

/// Resolved position of one visible tree row after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTreeRow {
    /// Index into the original `TreeView.rows` Vec (absolute, not visible).
    pub row_idx: usize,
    /// Full row bounds. `height` is clipped to the viewport if the row
    /// would extend past the bottom edge.
    pub bounds: Rect,
}

/// Classification of a hit-test result.
///
/// When [`TreeRowMeasure::chevron_end_x`] is set for a branch row the layout
/// emits two adjacent hit regions for that row: a [`Chevron`] region on the
/// left (covering the painted expand/collapse glyph) and a [`Row`] region for
/// the remainder. Backends that do not distinguish the two leave
/// `chevron_end_x` as `None`; clicking anywhere on the row returns `Row`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeViewHit {
    /// Click landed on the body portion of a row.
    /// Carries the `row_idx` into `TreeView.rows`.
    Row(usize),
    /// Click landed on the expand/collapse chevron of a branch row.
    /// Carries the same `row_idx` as the companion `Row` region.
    Chevron(usize),
    /// Click landed in the viewport's empty region (below the last row).
    Empty,
}

/// Fully-resolved tree-view layout. Backends iterate `visible_rows` for
/// painting and call [`Self::hit_test`] for clicks.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeViewLayout {
    /// Viewport width in the measurer's unit.
    pub viewport_width: f32,
    /// Viewport height in the measurer's unit.
    pub viewport_height: f32,
    /// Rows that are at least partially visible, top to bottom.
    pub visible_rows: Vec<VisibleTreeRow>,
    /// Ordered hit-region list. One region per visible row.
    pub hit_regions: Vec<(Rect, TreeViewHit)>,
    /// Scroll offset actually used. Clamped to `[0, rows.len())` so the
    /// backend never iterates past the end of the row slice.
    pub resolved_scroll_offset: usize,
}

impl TreeViewLayout {
    /// Test which row (if any) contains point `(x, y)`. Returns
    /// `TreeViewHit::Empty` when the point is below the last visible row.
    pub fn hit_test(&self, x: f32, y: f32) -> TreeViewHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        TreeViewHit::Empty
    }
}

impl TreeView {
    /// Compute the full rendering + hit-test layout for this tree.
    ///
    /// Per D6: layout decisions live here; backends consume the returned
    /// `TreeViewLayout` verbatim — iterate `visible_rows` for painting;
    /// call `hit_test` for clicks. Sub-row elements (chevron, icon, text,
    /// badge) are still backend-owned in v1 because their positions
    /// depend heavily on native conventions (TUI char cells vs GTK Pango
    /// pixel metrics).
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — available area in the
    ///   measurer's unit.
    /// - `measure_row(i)` — height for row `i` (index into `self.rows`).
    ///   Receives the row index (not the row itself) so backends can
    ///   vary height by decoration, indent, or other row state they know
    ///   about via their copy of `self.rows`.
    ///
    /// # Row clipping
    ///
    /// The last visible row's `bounds.height` is clipped to whatever
    /// fits inside the viewport. Backends that want to skip partially-
    /// visible rows can check `row.bounds.height < measure_row(row.row_idx).height`.
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        measure_row: F,
    ) -> TreeViewLayout
    where
        F: Fn(usize) -> TreeRowMeasure,
    {
        let mut visible_rows: Vec<VisibleTreeRow> = Vec::new();
        let mut hit_regions: Vec<(Rect, TreeViewHit)> = Vec::new();

        // Clamp scroll_offset to a valid starting index; we still report
        // the clamped value so the app can write it back and self-correct.
        let resolved_scroll_offset =
            crate::primitives::scrollbar::clamp_scroll_offset(self.scroll_offset, self.rows.len());

        let mut y = 0.0_f32;
        for i in resolved_scroll_offset..self.rows.len() {
            if y >= viewport_height {
                break;
            }
            let m = measure_row(i);
            // Clip the last row's height to fit inside the viewport.
            let remaining = viewport_height - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, viewport_width, height);
            visible_rows.push(VisibleTreeRow { row_idx: i, bounds });
            // Split the hit region into Chevron + Row when the backend
            // supplied a chevron boundary for this branch row.
            let row = &self.rows[i];
            if let (Some(_), Some(chev_x)) = (row.is_expanded, m.chevron_end_x) {
                let chev_x = chev_x.clamp(0.0, viewport_width);
                if chev_x > 0.0 {
                    hit_regions.push((Rect::new(0.0, y, chev_x, height), TreeViewHit::Chevron(i)));
                }
                let body_w = (viewport_width - chev_x).max(0.0);
                if body_w > 0.0 {
                    hit_regions.push((Rect::new(chev_x, y, body_w, height), TreeViewHit::Row(i)));
                }
            } else {
                hit_regions.push((bounds, TreeViewHit::Row(i)));
            }
            y += m.height;
        }

        TreeViewLayout {
            viewport_width,
            viewport_height,
            visible_rows,
            hit_regions,
            resolved_scroll_offset,
        }
    }

    /// Vertical scrollbar geometry for this tree rendered into `area`, or
    /// `None` when the tree is empty or all rows fit in the viewport
    /// (#1043 — before this method, an overflowing tree had no
    /// host-facing way to ask for a scroll affordance short of
    /// hand-rolling one per backend, and [`crate::Backend::draw_tree`]
    /// painted none at all).
    ///
    /// This is the single source of truth shared by any rasteriser that
    /// paints a tree scrollbar and consumers that hit-test the resolved
    /// `track`/`thumb_start`/`thumb_len` to implement thumb dragging —
    /// mirrors [`ListView::vscrollbar`](crate::primitives::list::ListView::vscrollbar).
    /// Reach it backend-agnostically via
    /// [`crate::Backend::tree_vscrollbar`].
    ///
    /// Unlike `ListView`, `TreeView` rows can have mixed heights on pixel
    /// backends (`Decoration::Header` rows paint shorter than others —
    /// see [`crate::primitives::layout_metrics::tree_layout`]). This
    /// method still sizes the thumb against a uniform `row_height`: a
    /// pixel-exact extent would require summing every row's real
    /// measured height every frame just to position a thumb, and the
    /// thumb only needs to be proportional, not a promise of pixel-exact
    /// travel. Unlike `ListView` (whose items really do paint at exactly
    /// `line_height`, with no multiplier), a `TreeView`'s non-header rows
    /// paint at `line_height * 1.4` (or [`crate::types::TreeStyle::row_height`]
    /// when the host set one) — so callers must pass that pitch, not raw
    /// `line_height`, or the computed "does this overflow" answer and the
    /// thumb geometry won't match what [`crate::primitives::layout_metrics::tree_layout`]
    /// (and therefore `draw_tree`) actually paints (#1043).
    ///
    /// # Arguments
    ///
    /// - `area` — the tree surface rect, in surface-native units (TUI
    ///   cells, GTK / macOS / Windows pixels).
    /// - `row_height` — height of one (non-header) row: `1.0` on TUI
    ///   (which always uses 1 cell/row, ignoring `TreeStyle::row_height`
    ///   — see `tui_tree_layout`'s doc), or
    ///   [`crate::primitives::layout_metrics::tree_row_pitch`]'s result on
    ///   pixel backends. The scrollbar occupies the rightmost column of
    ///   `area`, and `row_height` also serves as both the column width
    ///   and the minimum thumb length (same convention as
    ///   `ListView::vscrollbar`).
    pub fn vscrollbar(&self, area: Rect, row_height: f32) -> Option<Scrollbar> {
        if row_height <= 0.0 {
            return None;
        }
        let total = self.rows.len() as f32;
        if total == 0.0 {
            return None;
        }
        let visible = (area.height / row_height).floor();
        if total <= visible {
            return None;
        }
        let track = Rect::new(
            area.x + area.width - row_height,
            area.y,
            row_height,
            area.height,
        );
        Some(Scrollbar::vertical(
            self.id.clone(),
            track,
            self.scroll_offset as f32,
            total,
            visible,
            row_height,
        ))
    }
}

// ── NativeSurface paint (#1075, NativeSurface Phase 4 slice 2/8) ───────────
//
// Before this, `gtk::draw_tree` (Cairo), `macos::tree::draw_tree` (Core
// Graphics) and `win::tree::draw_tree` (Direct2D) each independently
// painted the same row/chevron/icon/badge/scrollbar content with their
// own drawing API. `paint` below is the one shared implementation,
// written against [`crate::native_surface::NativeSurface`] instead of
// any one backend's API — see `crate::primitives::list::native_surface_paint`
// for the same pattern applied one primitive earlier.
//
// What this migration fixes (issue #1075's named drift, plus two
// smaller ones surfaced while unifying — reported here rather than
// silently assumed, per this issue's "re-verify before you implement"):
//
// - **Missing vertical scrollbar.** `Backend::tree_vscrollbar` already
//   returns real track/thumb geometry (#1043), but none of the three
//   `draw_tree`s ever painted it. Fixed via
//   [`crate::primitives::scrollbar::native_surface_paint::paint`].
// - **Win never painted `TreeRow::edit`** (inline rename) at all —
//   rows mid-rename rendered their stale label instead. macOS painted
//   a reduced fallback (plain text, no caret/selection — see its
//   pre-migration module doc, "Scope omissions"); GTK alone had the
//   full caret + selection-highlight + placeholder treatment. `paint`
//   below carries GTK's full version for all three backends, rather
//   than picking the lowest common denominator — a caret-less rename
//   box is a real usability gap, not a style choice worth preserving.
// - **Error/Warning decoration colour**: `win::tree` alone mapped
//   `Decoration::Error`/`Warning` to `theme.error_fg`/`warning_fg`;
//   GTK/macOS's `def_fg` match only special-cased `Muted`, falling
//   through to plain `foreground` for Error/Warning rows — an
//   inconsistency with `ListView`, whose per-item decoration → fg
//   mapping already colours all four decorations identically on every
//   backend (see `gtk::list::draw_list`'s doc). `paint` adopts Win's
//   fuller mapping for all three, matching `ListView`'s convention.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{TreeRowEditState, TreeView, TreeViewLayout};
    use crate::native_surface::NativeSurface;
    use crate::text_util::{safe_prefix, snap_to_char_boundary};
    use crate::theme::Theme;
    use crate::types::Decoration;
    use crate::Rect;

    /// Paint a [`TreeView`]'s content — background, rows
    /// (selection/decoration/chevron/icon/badge/text or inline-edit),
    /// and vertical scrollbar — onto `surface` at `area`.
    ///
    /// `tree_layout` must be the same [`TreeViewLayout`] the caller uses
    /// for hit-testing (typically `gtk_tree_layout`/`mac_tree_layout`/
    /// `win_tree_layout`'s return value) so paint and hit-test can never
    /// disagree. `line_height` is one text row's height in `surface`'s
    /// native units; row pitch (header vs. leaf/branch) is derived from
    /// it via [`crate::primitives::layout_metrics::tree_row_pitch`], the
    /// same formula `tree_layout` itself used. `nerd_fonts_enabled`
    /// selects `Icon::glyph` vs. `Icon::fallback`.
    pub(crate) fn paint(
        tree: &TreeView,
        area: Rect,
        tree_layout: &TreeViewLayout,
        line_height: f32,
        nerd_fonts_enabled: bool,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
    ) {
        if area.width <= 0.0 || area.height <= 0.0 {
            return;
        }

        surface.surface_push_clip(area);
        surface.surface_fill_rect(area, theme.tab_bar_bg);

        let indent_px = (line_height * 0.9).round();
        let header_height = (line_height * 1.2).round();
        let item_height =
            crate::primitives::layout_metrics::tree_row_pitch(tree, line_height as f64) as f32;

        for vis_row in &tree_layout.visible_rows {
            let row = &tree.rows[vis_row.row_idx];
            let row_x = area.x + vis_row.bounds.x;
            let row_y = area.y + vis_row.bounds.y;
            let row_w = vis_row.bounds.width;
            let row_h = vis_row.bounds.height;

            // Skip rows the layout clipped to a partial height — painting
            // them produces a compressed background band at the section
            // boundary (matches GTK/macOS's pre-migration behaviour; Win's
            // pre-migration `draw_tree` did not skip these, an oversight
            // fixed by this unification rather than a deliberate
            // per-backend choice — nothing documented it as one).
            let is_header = matches!(row.decoration, Decoration::Header);
            let full_h = if is_header {
                header_height
            } else {
                item_height
            };
            if row_h < full_h - 0.5 {
                continue;
            }

            let path_selected = tree.selected_path.as_ref().is_some_and(|p| p == &row.path);
            let is_selected = tree.has_focus && path_selected;
            let is_inactive_selected = !tree.has_focus && path_selected;

            let (def_fg, row_bg) = if is_selected {
                (theme.header_fg, theme.selected_bg)
            } else if is_inactive_selected {
                (theme.foreground, theme.inactive_selected_bg)
            } else if is_header {
                (theme.header_fg, theme.header_bg)
            } else {
                match row.decoration {
                    Decoration::Muted => (theme.muted_fg, theme.tab_bar_bg),
                    Decoration::Error => (theme.error_fg, theme.tab_bar_bg),
                    Decoration::Warning => (theme.warning_fg, theme.tab_bar_bg),
                    _ => (theme.foreground, theme.tab_bar_bg),
                }
            };

            let row_rect = Rect::new(row_x, row_y, row_w, row_h);
            surface.surface_fill_rect(row_rect, row_bg);

            let mut cursor_x = row_x + 2.0 + row.indent as f32 * indent_px;

            if let Some(expanded) = row.is_expanded {
                if tree.style.show_chevrons {
                    let chevron = if expanded {
                        &tree.style.chevron_expanded
                    } else {
                        &tree.style.chevron_collapsed
                    };
                    let (cw, ch) = surface.surface_measure_text(chevron);
                    let cy = row_y + (row_h - ch) / 2.0;
                    surface.surface_draw_text_run(Rect::new(cursor_x, cy, cw, ch), chevron, def_fg);
                    cursor_x += cw + 4.0;
                }
            } else {
                cursor_x += line_height * 0.8;
            }

            if let Some(ref icon) = row.icon {
                let glyph = if nerd_fonts_enabled {
                    icon.glyph.as_str()
                } else {
                    icon.fallback.as_str()
                };
                let icon_fg = icon.color.unwrap_or(def_fg);
                let (iw, ih) = surface.surface_measure_text(glyph);
                let iy = row_y + (row_h - ih) / 2.0;
                surface.surface_draw_text_run(Rect::new(cursor_x, iy, iw, ih), glyph, icon_fg);
                cursor_x += iw + 6.0;
            }

            if let Some(ref edit) = row.edit {
                paint_edit_input(
                    surface,
                    cursor_x,
                    row_y,
                    row_h,
                    row_x + row_w,
                    edit,
                    def_fg,
                    theme.selection_bg,
                    theme.muted_fg,
                );
                continue;
            }

            let badge_info = row.badge.as_ref().map(|badge| {
                let (bw, _) = surface.surface_measure_text(&badge.text);
                let bfg = badge.fg.unwrap_or(theme.muted_fg);
                let bbg = badge.bg.unwrap_or(row_bg);
                (badge.text.clone(), bw, bfg, bbg)
            });
            let badge_reserve = badge_info
                .as_ref()
                .map(|(_, bw, ..)| *bw + 8.0)
                .unwrap_or(0.0);
            let text_right_limit = row_x + row_w - badge_reserve - 4.0;
            let text_start_x = cursor_x;

            // #1183: hard-clip label painting to `text_right_limit` so a
            // long span can never bleed into the badge's reserved gap.
            // Before this, only a span's *background fill* was clamped
            // (`clipped_w` below) — the glyph run itself was always drawn
            // at its full measured width via `surface_draw_text_run_styled`,
            // so an over-long label's last glyphs painted straight through
            // (and often past) the badge with no gap, mirroring the TUI
            // bug this issue reports. `NativeSurface` has no ellipsize verb
            // (see `diff_view::native_surface_paint`'s identical note), so
            // GUI backends hard-clip rather than ellipsize — TUI is the
            // only backend that can cheaply measure+truncate a column
            // budget and paint a literal `…` glyph.
            let clip_w = (text_right_limit - cursor_x).max(0.0);
            surface.surface_push_clip(Rect::new(cursor_x, row_y, clip_w, row_h));
            for span in &row.text.spans {
                if cursor_x >= text_right_limit {
                    break;
                }
                let span_fg = if let Some(c) = span.fg {
                    c
                } else if matches!(row.decoration, Decoration::Muted) {
                    theme.muted_fg
                } else {
                    def_fg
                };
                let (sw, sh) = surface.surface_measure_text_styled(&span.text, span.bold);
                if let Some(sbg) = span.bg {
                    let clipped_w = sw.min((text_right_limit - cursor_x).max(0.0));
                    surface.surface_fill_rect(Rect::new(cursor_x, row_y, clipped_w, row_h), sbg);
                }
                let sy = row_y + (row_h - sh) / 2.0;
                surface.surface_draw_text_run_styled(
                    Rect::new(cursor_x, sy, sw, sh),
                    &span.text,
                    span_fg,
                    span.bold,
                    false,
                    false,
                    1.0,
                );
                cursor_x += sw;
            }
            surface.surface_pop_clip();

            if let Some((btext, bw, bfg, bbg)) = badge_info {
                let bx = row_x + row_w - bw - 4.0;
                // #1183: gate on `text_right_limit > text_start_x` (is
                // there room, in principle, to reserve the badge + gap at
                // all?) rather than the old `bx > cursor_x` — `cursor_x`
                // reflects the label's *unclamped* measured width, so a
                // label just barely longer than the row used to hide the
                // badge outright instead of clamping the label and still
                // showing it, the mirror image of the overwrite bug this
                // issue reports.
                if text_right_limit > text_start_x {
                    if bbg != row_bg {
                        surface.surface_fill_rect(Rect::new(bx - 2.0, row_y, bw + 4.0, row_h), bbg);
                    }
                    let (_, bh) = surface.surface_measure_text(&btext);
                    let by = row_y + (row_h - bh) / 2.0;
                    surface.surface_draw_text_run(Rect::new(bx, by, bw, bh), &btext, bfg);
                }
            }
        }

        // ── Vertical scrollbar (#1075 fix: previously never painted) ───
        if let Some(vsb) = tree.vscrollbar(area, item_height) {
            crate::primitives::scrollbar::native_surface_paint::paint(&vsb, surface, theme);
        }

        surface.surface_pop_clip();
    }

    /// Paint an inline-rename [`TreeRowEditState`]: placeholder (when
    /// `edit.text` is empty), selection highlight, text, and a thin
    /// vertical caret bar. Ported from GTK's original
    /// `paint_edit_input_gtk` (the only one of the three pre-migration
    /// rasterisers with the full treatment — see this module's doc).
    #[allow(clippy::too_many_arguments)]
    fn paint_edit_input(
        surface: &mut dyn NativeSurface,
        text_x: f32,
        row_y: f32,
        row_h: f32,
        right_edge: f32,
        edit: &TreeRowEditState,
        fg: crate::types::Color,
        sel_bg: crate::types::Color,
        dim: crate::types::Color,
    ) {
        let text_w = right_edge - text_x - 4.0;
        if text_w <= 0.0 {
            return;
        }

        if edit.text.is_empty() {
            if let Some(ref ph) = edit.placeholder {
                let (_, th) = surface.surface_measure_text(ph);
                surface.surface_draw_text_run(
                    Rect::new(text_x, row_y + (row_h - th) / 2.0, text_w, th),
                    ph,
                    dim,
                );
            }
            // Caret at position 0.
            surface.surface_fill_rect(Rect::new(text_x, row_y + 3.0, 1.5, row_h - 6.0), fg);
            return;
        }

        // Selection highlight.
        if let Some(anchor) = edit.selection_anchor {
            if anchor != edit.cursor {
                let lo = snap_to_char_boundary(&edit.text, anchor.min(edit.cursor));
                let hi = snap_to_char_boundary(&edit.text, anchor.max(edit.cursor));
                let prefix = &edit.text[..lo];
                let sel_text = &edit.text[lo..hi];
                let (prefix_w, _) = surface.surface_measure_text(prefix);
                let (sel_w, _) = surface.surface_measure_text(sel_text);
                surface.surface_fill_rect(
                    Rect::new(text_x + prefix_w, row_y + 2.0, sel_w, row_h - 4.0),
                    sel_bg,
                );
            }
        }

        // Text.
        let (_, th) = surface.surface_measure_text(&edit.text);
        surface.surface_draw_text_run(
            Rect::new(text_x, row_y + (row_h - th) / 2.0, text_w, th),
            &edit.text,
            fg,
        );

        // Thin vertical caret bar.
        let cursor_prefix = safe_prefix(&edit.text, edit.cursor);
        let (cx_off, _) = surface.surface_measure_text(cursor_prefix);
        let caret_x = text_x + cx_off;
        surface.surface_fill_rect(Rect::new(caret_x, row_y + 3.0, 1.5, row_h - 6.0), fg);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Viewport};
        use crate::primitives::tree::TreeRow;
        use crate::types::{Badge, Color, SelectionMode, StyledText, TreeStyle, WidgetId};
        use crate::Image;

        /// Records every fill + text-run call — mirrors
        /// `primitives::list::native_surface_paint`'s `RecordingSurface`
        /// test double, scoped to the verbs this primitive uses, so this
        /// test runs on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            texts: Vec<(Rect, String, Color)>,
            clips: Vec<Rect>,
        }

        impl NativeSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.texts.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _sw: f32) {}
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clips.push(rect);
            }
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn leaf(idx: u16, label: &str) -> TreeRow {
            TreeRow {
                path: vec![idx],
                indent: 0,
                icon: None,
                text: StyledText::plain(label.to_string()),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            }
        }

        fn make_tree(rows: Vec<TreeRow>) -> TreeView {
            TreeView {
                id: WidgetId::new("t"),
                rows,
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: TreeStyle::default(),
                has_focus: true,
            }
        }

        const LINE_HEIGHT: f32 = 16.0;
        const AREA: Rect = Rect::new(0.0, 0.0, 100.0, 80.0);

        /// #1075 regression: before this migration, none of the three
        /// backends' `draw_tree` painted a vertical scrollbar even
        /// though `Backend::tree_vscrollbar` (#1043) already exposed
        /// real geometry for hit-testing. Paints through the shared fn
        /// and asserts a fill lands at the exact track rect
        /// `TreeView::vscrollbar` resolves.
        #[test]
        fn paints_vertical_scrollbar_track_when_overflowing() {
            let tree = make_tree((0..50).map(|i| leaf(i, &format!("row{i}"))).collect());
            let layout =
                crate::primitives::layout_metrics::tree_layout(&tree, AREA, LINE_HEIGHT as f64);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
            );

            let item_height =
                crate::primitives::layout_metrics::tree_row_pitch(&tree, LINE_HEIGHT as f64) as f32;
            let expected = tree
                .vscrollbar(AREA, item_height)
                .expect("50 rows in an 80px/22px viewport must need a v-scrollbar");
            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, _)| (r.x - expected.track.x).abs() < 0.01
                        && (r.y - expected.track.y).abs() < 0.01
                        && (r.width - expected.track.width).abs() < 0.01
                        && (r.height - expected.track.height).abs() < 0.01),
                "expected a fill at the v-scrollbar track {:?}, got fills: {:?}",
                expected.track,
                surface.fills
            );
        }

        /// #1075 regression: `win::tree::draw_tree` never painted
        /// `TreeRow::edit` at all (see this module's doc). This test
        /// paints a row mid-rename through the shared fn and asserts a
        /// caret bar (a thin, distinctly-sized fill) shows up — it would
        /// have failed against the pre-migration Win rasteriser, which
        /// rendered the row's stale label instead of any edit-state
        /// paint at all.
        #[test]
        fn paints_caret_for_row_being_edited() {
            let mut rows = vec![leaf(0, "alpha"), leaf(1, "old-name"), leaf(2, "gamma")];
            rows[1].edit = Some(TreeRowEditState {
                text: "new-name".into(),
                cursor: 3,
                selection_anchor: None,
                placeholder: None,
            });
            let tree = make_tree(rows);
            let layout =
                crate::primitives::layout_metrics::tree_layout(&tree, AREA, LINE_HEIGHT as f64);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
            );

            let row1 = &layout.visible_rows[1];
            let caret_fill = surface.fills.iter().find(|(r, _)| {
                (r.width - 1.5).abs() < 0.01
                    && r.y >= row1.bounds.y
                    && r.y < row1.bounds.y + row1.bounds.height
            });
            assert!(
                caret_fill.is_some(),
                "expected a 1.5px-wide caret fill inside row 1's bounds {:?}, got fills: {:?}",
                row1.bounds,
                surface.fills
            );
        }

        /// Selection highlight inside an edited row paints a fill in
        /// `theme.selection_bg` sized to the selected substring, distinct
        /// from the caret fill.
        #[test]
        fn paints_selection_highlight_for_edited_row_with_selection() {
            let mut rows = vec![leaf(0, "old-name")];
            rows[0].edit = Some(TreeRowEditState {
                text: "new-name".into(),
                cursor: 3,
                selection_anchor: Some(0),
                placeholder: None,
            });
            let tree = make_tree(rows);
            let layout =
                crate::primitives::layout_metrics::tree_layout(&tree, AREA, LINE_HEIGHT as f64);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
            );

            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, c)| *c == theme.selection_bg && r.width > 1.5),
                "expected a selection-highlight fill in theme.selection_bg wider than \
                 the caret bar, got fills: {:?}",
                surface.fills
            );
        }

        #[test]
        fn selected_row_paints_selected_bg() {
            let mut tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
            tree.selected_path = Some(vec![1]);
            tree.has_focus = true;
            let layout =
                crate::primitives::layout_metrics::tree_layout(&tree, AREA, LINE_HEIGHT as f64);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
            );

            let row1 = &layout.visible_rows[1];
            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, c)| (r.x - row1.bounds.x).abs() < 0.01
                        && (r.y - row1.bounds.y).abs() < 0.01
                        && *c == theme.selected_bg),
                "selected row must paint theme.selected_bg at its own bounds"
            );
        }

        /// Error/Warning decoration rows paint their dedicated fg colour
        /// (adopted from Win's pre-migration mapping — see this module's
        /// doc for why GTK/macOS's narrower `Muted`-only mapping wasn't
        /// preserved instead).
        #[test]
        fn error_and_warning_decoration_use_dedicated_fg() {
            let mut err_row = leaf(0, "boom");
            err_row.decoration = Decoration::Error;
            let mut warn_row = leaf(1, "careful");
            warn_row.decoration = Decoration::Warning;
            let tree = make_tree(vec![err_row, warn_row]);
            let layout =
                crate::primitives::layout_metrics::tree_layout(&tree, AREA, LINE_HEIGHT as f64);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
            );

            assert!(surface
                .texts
                .iter()
                .any(|(_, t, c)| t == "boom" && *c == theme.error_fg));
            assert!(surface
                .texts
                .iter()
                .any(|(_, t, c)| t == "careful" && *c == theme.warning_fg));
        }

        /// #1183 parity check: GUI backends (GTK/Win/macOS all share this
        /// `paint` fn) must clamp an over-long label before a right-aligned
        /// badge the same way TUI does — a gap between the label's clip
        /// boundary and the badge, never the badge painted flush against
        /// (or on top of) the label. `NativeSurface` has no ellipsize verb
        /// (see `paint`'s inline #1183 note), so this asserts the hard-clip
        /// boundary sits strictly left of the badge's start, not a literal
        /// `…` glyph — that part of the fix is TUI-only.
        #[test]
        fn long_label_with_badge_clips_before_badge_leaving_a_gap() {
            let mut row = leaf(0, "BACKEND_SPECULATIVE_EXECUTION_MODULE");
            row.badge = Some(Badge::plain("U"));
            let tree = make_tree(vec![row]);
            let layout =
                crate::primitives::layout_metrics::tree_layout(&tree, AREA, LINE_HEIGHT as f64);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
            );

            let (badge_bw, _) = surface.surface_measure_text("U");
            let badge_start_x = AREA.x + AREA.width - badge_bw - 4.0;

            // `paint` also pushes one whole-`area` clip bracket at the top
            // of the function (unrelated to this row's label); the label
            // clip is the narrower, row-scoped one, so pick the tightest
            // match rather than the first.
            let label_clip = surface
                .clips
                .iter()
                .filter(|r| (r.y - layout.visible_rows[0].bounds.y).abs() < 0.01)
                .min_by(|a, b| a.width.partial_cmp(&b.width).unwrap())
                .expect("expected a clip pushed around the row's label paint");

            assert!(
                label_clip.x + label_clip.width < badge_start_x,
                "label clip right edge ({}) must sit strictly left of the badge's \
                 start ({badge_start_x}), leaving a gap — got clip {:?}",
                label_clip.x + label_clip.width,
                label_clip
            );

            // The badge itself must still be painted (there was room for it).
            assert!(
                surface.texts.iter().any(|(_, t, _)| t == "U"),
                "badge text should still paint when the label is clamped, got texts: {:?}",
                surface.texts
            );
        }
    }
}

/// Events a `TreeView` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeEvent {
    /// Single-click (or Enter on the keyboard) on a row.
    RowClicked {
        path: TreePath,
        modifiers: Modifiers,
    },
    /// Double-click on a row (typically "open" / "activate").
    RowDoubleClicked { path: TreePath },
    /// The chevron was clicked, or Space/arrow-keys expanded/collapsed a branch.
    RowToggleExpand { path: TreePath },
    /// Keyboard selection moved to a new row.
    SelectionChanged { path: TreePath },
    /// A key was pressed while the tree had focus and the primitive did not
    /// consume it. App may interpret it (e.g. `s` stages a file).
    KeyPressed { key: String, modifiers: Modifiers },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_view_roundtrip_serde() {
        let tree = TreeView {
            id: WidgetId::new("sc"),
            rows: vec![TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("Staged Changes"),
                badge: Some(Badge::plain("3")),
                is_expanded: Some(true),
                decoration: Decoration::Normal,
                edit: None,
            }],
            selection_mode: SelectionMode::Single,
            selected_path: Some(vec![0]),
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: true,
        };
        let json = serde_json::to_string(&tree).unwrap();
        let back: TreeView = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
    }

    /// #623: `TreeStyle::row_height` is additive, not a required field.
    ///
    /// A `TreeStyle` serialised before #623 (or hand-written by a Lua
    /// plugin, per the `types.rs` design invariant that every primitive
    /// is plugin-constructible from JSON) carries no `row_height` key at
    /// all. `#[serde(default)]` must decode that as `None` — i.e. "keep
    /// deriving the row pitch from `line_height`" — rather than failing
    /// the whole `TreeView` deserialisation with a missing-field error.
    #[test]
    fn tree_style_row_height_defaults_to_none_when_absent_from_json() {
        let legacy = r#"{
            "indent": 2,
            "show_chevrons": true,
            "chevron_expanded": "▾",
            "chevron_collapsed": "▸"
        }"#;
        let style: TreeStyle =
            serde_json::from_str(legacy).expect("pre-#623 TreeStyle must decode");
        assert_eq!(
            style.row_height, None,
            "absent row_height means 'derive from line_height', not a decode error"
        );
        assert_eq!(style, TreeStyle::default());
    }

    /// #623: an explicit override survives a serde round-trip, so a host
    /// that pins the row pitch keeps it across a save/restore of its UI
    /// state (and across the plugin JSON boundary).
    #[test]
    fn tree_style_row_height_override_roundtrips_serde() {
        let style = TreeStyle {
            row_height: Some(22),
            ..TreeStyle::default()
        };
        let json = serde_json::to_string(&style).unwrap();
        let back: TreeStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.row_height, Some(22));
        assert_eq!(style, back);
    }

    // ── D6 TreeView layout API tests ──────────────────────────────────

    fn make_tree_row(path: &[u16], indent: u16, label: &str) -> TreeRow {
        TreeRow {
            path: path.to_vec(),
            indent,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn make_tree(rows: Vec<TreeRow>, scroll: usize) -> TreeView {
        TreeView {
            id: WidgetId::new("t"),
            rows,
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: scroll,
            style: TreeStyle::default(),
            has_focus: true,
        }
    }

    #[test]
    fn tree_view_layout_empty() {
        let tree = make_tree(vec![], 0);
        let layout = tree.layout(40.0, 20.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.visible_rows.len(), 0);
        assert_eq!(layout.hit_regions.len(), 0);
        assert_eq!(layout.resolved_scroll_offset, 0);
        assert_eq!(layout.hit_test(5.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_all_rows_fit() {
        let tree = make_tree(
            (0..3)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].bounds.y, 0.0);
        assert_eq!(layout.visible_rows[1].bounds.y, 1.0);
        assert_eq!(layout.visible_rows[2].bounds.y, 2.0);
        // Hit-test each row by y coord.
        assert_eq!(layout.hit_test(10.0, 0.5), TreeViewHit::Row(0));
        assert_eq!(layout.hit_test(10.0, 1.5), TreeViewHit::Row(1));
        assert_eq!(layout.hit_test(10.0, 2.5), TreeViewHit::Row(2));
        // Below last row → Empty.
        assert_eq!(layout.hit_test(10.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_scroll_offset_applies() {
        let tree = make_tree(
            (0..5)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            2, // skip first 2
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].row_idx, 2);
        assert_eq!(layout.visible_rows[1].row_idx, 3);
        assert_eq!(layout.visible_rows[2].row_idx, 4);
    }

    #[test]
    fn tree_view_layout_viewport_overflow_clips() {
        // 10 rows of height 2.0 each; viewport 5.0 tall → only 3 rows
        // fit (one partially clipped).
        let tree = make_tree(
            (0..10)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(40.0, 5.0, |_| TreeRowMeasure::new(2.0));
        // Rows 0 (y=0..2), 1 (y=2..4), 2 (y=4..5 clipped) — three visible.
        assert_eq!(layout.visible_rows.len(), 3);
        // Last row clipped to height 1.0 (remaining = 5 - 4 = 1).
        assert_eq!(layout.visible_rows[2].bounds.height, 1.0);
        // A click below all visible rows returns Empty.
        assert_eq!(layout.hit_test(10.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_varying_row_heights() {
        // Branches (is_expanded != None) get height 1.4 * base; leaves get
        // 1.0. Proves the measurer can consult row state.
        let mut rows = vec![
            make_tree_row(&[0], 0, "branch"),
            make_tree_row(&[0, 0], 1, "leaf0"),
            make_tree_row(&[0, 1], 1, "leaf1"),
        ];
        rows[0].is_expanded = Some(true);
        let tree = make_tree(rows.clone(), 0);
        let layout = tree.layout(40.0, 10.0, |i| {
            let h = if rows[i].is_expanded.is_some() {
                1.4
            } else {
                1.0
            };
            TreeRowMeasure::new(h)
        });
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].bounds.height, 1.4);
        assert_eq!(layout.visible_rows[1].bounds.y, 1.4);
        assert_eq!(layout.visible_rows[1].bounds.height, 1.0);
        assert!((layout.visible_rows[2].bounds.y - 2.4).abs() < 0.001);
    }

    #[test]
    fn tree_view_layout_pixel_units_fractional() {
        // GTK-style: line_height = 18.5 px leaf, 25.9 px branch. Proves
        // fractional row heights flow through correctly.
        let tree = make_tree(
            (0..5)
                .map(|i| make_tree_row(&[i], 0, &format!("r{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(300.0, 60.0, |_| TreeRowMeasure::new(18.5));
        // 3 full rows fit (55.5), 4th starts at y=55.5 and gets clipped to 4.5 px.
        assert_eq!(layout.visible_rows.len(), 4);
        assert!((layout.visible_rows[3].bounds.height - 4.5).abs() < 0.001);
    }

    #[test]
    fn tree_view_layout_scroll_offset_clamped() {
        // scroll_offset beyond rows.len() — resolved to rows.len()-1 so
        // the single remaining row is visible.
        let tree = make_tree(
            (0..3)
                .map(|i| make_tree_row(&[i], 0, &format!("r{i}")))
                .collect(),
            99,
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.visible_rows.len(), 1);
        assert_eq!(layout.visible_rows[0].row_idx, 2);
    }

    // ── #1043 TreeView::vscrollbar ──────────────────────────────────────

    mod vscrollbar_tests {
        use super::*;

        /// Build a tree with `n_rows` leaf rows, scrolled to `scroll_offset`.
        fn vtree(n_rows: usize, scroll_offset: usize) -> TreeView {
            make_tree(
                (0..n_rows)
                    .map(|i| make_tree_row(&[i as u16], 0, &format!("row{i}")))
                    .collect(),
                scroll_offset,
            )
        }

        #[test]
        fn none_when_tree_is_empty() {
            let t = vtree(0, 0);
            assert!(t.vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0).is_none());
        }

        #[test]
        fn none_when_rows_fit_in_viewport() {
            // 5 rows, 10-row viewport — all fit, no scrollbar.
            let t = vtree(5, 0);
            assert!(t.vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0).is_none());
        }

        #[test]
        fn none_when_rows_exactly_fill_viewport() {
            // 10 rows, 10-row viewport — total == visible, no scrollbar.
            let t = vtree(10, 0);
            assert!(t.vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0).is_none());
        }

        #[test]
        fn none_when_row_height_is_zero_or_negative() {
            let t = vtree(50, 0);
            assert!(t.vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 0.0).is_none());
            assert!(t
                .vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), -1.0)
                .is_none());
        }

        #[test]
        fn track_spans_rightmost_column() {
            let t = vtree(20, 0);
            let sb = t
                .vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
                .expect("overflow should yield a scrollbar");
            // Rightmost column (x=19), full height, 1 unit wide.
            assert_eq!(sb.track.x, 19.0);
            assert_eq!(sb.track.y, 0.0);
            assert_eq!(sb.track.width, 1.0);
            assert_eq!(sb.track.height, 10.0);
            // Thumb starts at the top when scroll_offset == 0.
            assert_eq!(sb.thumb_start, 0.0);
            assert!(sb.thumb_len > 0.0 && sb.thumb_len < sb.track.height);
        }

        #[test]
        fn thumb_advances_with_scroll_offset() {
            let t = vtree(20, 10);
            let sb = t
                .vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
                .expect("overflow should yield a scrollbar");
            assert!(
                sb.thumb_start > 0.0,
                "thumb should have travelled from the top once scrolled"
            );
        }

        #[test]
        fn pixel_units_row_height_is_track_width_and_min_thumb() {
            // GTK-style: row_height = 18.5px. Track column width and
            // minimum thumb length both derive from it, same convention
            // as `ListView::vscrollbar`.
            let t = vtree(50, 0);
            let sb = t
                .vscrollbar(Rect::new(0.0, 0.0, 300.0, 200.0), 18.5)
                .expect("overflow should yield a scrollbar");
            assert_eq!(sb.track.width, 18.5);
            assert!(sb.thumb_len >= 18.5);
        }
    }
}
