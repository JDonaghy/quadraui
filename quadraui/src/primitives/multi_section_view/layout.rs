//! Layout for [`MultiSectionView`]: the chrome geometry (headers, aux
//! rows, bodies, scrollbars, dividers), the per-unit metrics and body
//! measurement it is resolved with, and the [`Primitive`] layout that adds
//! each visible body's own resolved layout on top.
//!
//! [`Primitive`]: crate::primitive::Primitive

use super::{
    AuxHit, Axis, DividerBounds, HeaderHit, MsvLayoutMetrics, MultiSectionView,
    MultiSectionViewHit, MultiSectionViewLayout, ScrollMode, ScrollbarHit, Section, SectionAux,
    SectionBody, SectionHeader, SectionLayout, SectionMeasure, SectionSize,
};
use crate::event::Rect;
use crate::types::Decoration;

impl MultiSectionView {
    /// Compute the full chrome layout for this view.
    ///
    /// `measure(section_idx) -> SectionMeasure` reports the body's
    /// natural content size (used by `SectionSize::Content` and
    /// `ContentClamped`) and the aux row size (used to subtract from
    /// the body area). The closure is called once per section.
    pub fn layout<F>(
        &self,
        bounds: Rect,
        metrics: MsvLayoutMetrics,
        measure: F,
    ) -> MultiSectionViewLayout
    where
        F: Fn(usize) -> SectionMeasure,
    {
        match self.axis {
            Axis::Vertical => self.layout_vertical(bounds, metrics, measure),
            Axis::Horizontal => {
                // Horizontal rasterisers not yet implemented; return an empty layout.
                MultiSectionViewLayout {
                    bounds,
                    axis: self.axis,
                    scroll_mode: self.scroll_mode,
                    sections: Vec::new(),
                    dividers: Vec::new(),
                    panel_scrollbar: None,
                    panel_scrollbar_thumb: None,
                    hit_regions: Vec::new(),
                }
            }
        }
    }

    fn layout_vertical<F>(
        &self,
        bounds: Rect,
        metrics: MsvLayoutMetrics,
        measure: F,
    ) -> MultiSectionViewLayout
    where
        F: Fn(usize) -> SectionMeasure,
    {
        let n = self.sections.len();
        if n == 0 {
            return MultiSectionViewLayout {
                bounds,
                axis: Axis::Vertical,
                scroll_mode: self.scroll_mode,
                sections: Vec::new(),
                dividers: Vec::new(),
                panel_scrollbar: None,
                panel_scrollbar_thumb: None,
                hit_regions: Vec::new(),
            };
        }

        // Pre-compute per-section measures in one pass; reused below.
        let measures: Vec<SectionMeasure> = (0..n).map(&measure).collect();

        // Container main-axis size after subtracting divider stripes
        // between non-collapsed sections. Dividers always sit between
        // sections (not above the first / below the last).
        let divider_count = if metrics.divider_size > 0.0 && n > 1 {
            (n - 1) as f32
        } else {
            0.0
        };
        let dividers_total = metrics.divider_size * divider_count;
        let usable_main = (bounds.height - dividers_total).max(0.0);

        // Per-section "must-have" main-axis cost: header + (aux if any).
        // This is paid even when collapsed for the header.
        let mut chrome_cost: Vec<f32> = vec![0.0; n];
        for i in 0..n {
            let aux = if self.sections[i].aux.is_some() {
                measures[i].aux_size
            } else {
                0.0
            };
            // Collapsed sections only show the header row.
            let chrome = if self.sections[i].collapsed {
                metrics.header_size
            } else {
                metrics.header_size + aux
            };
            chrome_cost[i] = chrome;
        }

        // ── Resolve per-section main-axis sizes ────────────────────
        let mut resolved = match self.scroll_mode {
            ScrollMode::WholePanel => {
                // Every section sized to chrome + content height.
                let mut sizes = Vec::with_capacity(n);
                for i in 0..n {
                    let body = if self.sections[i].collapsed {
                        0.0
                    } else {
                        measures[i].content_size
                    };
                    let total = chrome_cost[i] + body;
                    sizes.push(apply_min_max(&self.sections[i], total));
                }
                sizes
            }
            ScrollMode::PerSection => resolve_per_section_sizes(
                &self.sections,
                &measures,
                &chrome_cost,
                bounds.height,
                usable_main,
            ),
        };

        // Snap resolved sizes to integer cell multiples when the host
        // requests it (TUI). Without this, fractional section sizes
        // (e.g. 5.5 cells) cause paint to round to integer rows while
        // hit_test uses the raw fractional bounds — paint and click
        // disagree about which row is in which section. Distributes the
        // remainder one cell at a time so the sum still equals the
        // (integer) usable_main exactly.
        if metrics.cell_quantum > 0.0 && n > 0 {
            let q = metrics.cell_quantum;
            let target_total: f32 = resolved.iter().sum();
            let target_cells = (target_total / q).round() as i32;
            let mut snapped: Vec<i32> = resolved.iter().map(|s| (s / q).floor() as i32).collect();
            let mut remainder = target_cells - snapped.iter().sum::<i32>();
            // Order the indices by the size of the dropped fractional
            // part so the largest fractional shares get the spare
            // cells first — matches how layout managers usually break
            // ties on integer distribution.
            let mut order: Vec<(usize, f32)> = resolved
                .iter()
                .enumerate()
                .map(|(i, s)| (i, (s / q) - (s / q).floor()))
                .collect();
            order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let mut cursor = 0;
            while remainder > 0 && !order.is_empty() {
                let (idx, _) = order[cursor % order.len()];
                snapped[idx] += 1;
                remainder -= 1;
                cursor += 1;
            }
            // If we overshot (remainder negative), shave from the
            // smallest-fractional sections.
            if remainder < 0 {
                let mut order_rev = order;
                order_rev.reverse();
                let mut c = 0;
                while remainder < 0 && !order_rev.is_empty() {
                    let (idx, _) = order_rev[c % order_rev.len()];
                    if snapped[idx] > 1 {
                        snapped[idx] -= 1;
                        remainder += 1;
                    }
                    c += 1;
                }
            }
            resolved = snapped.into_iter().map(|s| (s as f32) * q).collect();
        }

        // ── Walk and emit per-section layouts + dividers ───────────
        let mut sections_out = Vec::with_capacity(n);
        let mut dividers = Vec::new();
        let mut hit_regions: Vec<(Rect, MultiSectionViewHit)> = Vec::new();

        let panel_sb_w = match self.scroll_mode {
            ScrollMode::WholePanel => {
                let total: f32 = resolved.iter().sum::<f32>() + dividers_total;
                if total > bounds.height {
                    metrics.scrollbar_size
                } else {
                    0.0
                }
            }
            ScrollMode::PerSection => 0.0,
        };

        // For WholePanel mode, sections stack at content size and the
        // viewport scrolls the whole panel. `panel_scroll` shifts every
        // section's y upward; the layout clamps the value internally to
        // `[0, total - viewport]` so sections never scroll past the
        // bottom even if the host stores an over-shot value (host's
        // stored scroll may exceed the clamp; layout only ever applies
        // the clamped value).
        let scroll_offset = match self.scroll_mode {
            ScrollMode::WholePanel => {
                let total: f32 = resolved.iter().sum::<f32>() + dividers_total;
                let max_scroll = (total - bounds.height).max(0.0);
                self.panel_scroll.clamp(0.0, max_scroll)
            }
            ScrollMode::PerSection => 0.0,
        };
        let mut y = bounds.y - scroll_offset;
        for i in 0..n {
            // For PerSection, each section is bounded by its resolved size.
            // For WholePanel, sections stack at content size and the panel
            // itself scrolls (content past bounds.height is clipped by the
            // backend; we still emit hit_regions / sections normally).
            let s_main = resolved[i];
            let s_top = y;

            let header_bounds = Rect::new(
                bounds.x,
                s_top,
                (bounds.width - panel_sb_w).max(0.0),
                metrics.header_size,
            );

            let mut content_top = s_top + metrics.header_size;

            let aux_bounds = if let Some(aux) = &self.sections[i].aux {
                if self.sections[i].collapsed {
                    None
                } else {
                    let aux_h = measures[i].aux_size;
                    let r = Rect::new(bounds.x, content_top, bounds.width, aux_h);
                    content_top += aux_h;
                    let _ = aux; // silences unused-binding warnings on cfg-narrow builds
                    Some(r)
                }
            } else {
                None
            };

            let collapsed = self.sections[i].collapsed;

            // Body and scrollbar split the remainder of this section's
            // main-axis budget.
            let remaining_main = (s_top + s_main - content_top).max(0.0);

            // Decide if a per-section scrollbar is reserved. PerSection
            // mode reserves one if body content overflows the body area;
            // WholePanel never reserves per-section scrollbars.
            let body_main = if collapsed { 0.0 } else { remaining_main };
            let needs_scrollbar = matches!(self.scroll_mode, ScrollMode::PerSection)
                && !collapsed
                && body_overflows(&self.sections[i], measures[i].content_size, body_main);
            let scrollbar_w = if needs_scrollbar {
                metrics.scrollbar_size
            } else {
                0.0
            };
            let body_w = (bounds.width - scrollbar_w - panel_sb_w).max(0.0);

            let body_bounds = Rect::new(bounds.x, content_top, body_w, body_main);
            let scrollbar_bounds = if needs_scrollbar {
                Some(Rect::new(
                    bounds.x + body_w,
                    content_top,
                    scrollbar_w,
                    body_main,
                ))
            } else {
                None
            };

            // Per-section thumb bounds — computed from the body's scroll state when
            // introspectable (Tree, List). For overflowing bodies without row-based
            // scroll the thumb bounds stay None; the hit-region emission below falls
            // back to a single combined-Thumb region covering the whole gutter.
            let thumb_bounds = match (scrollbar_bounds, body_scroll_state(&self.sections[i].body)) {
                (Some(sb), Some((scroll_rows, row_count))) if row_count > 0 => {
                    Some(compute_thumb_bounds(
                        sb,
                        scroll_rows,
                        row_count,
                        measures[i].content_size,
                        metrics.cell_quantum,
                    ))
                }
                _ => None,
            };

            sections_out.push(SectionLayout {
                section_idx: i,
                header_bounds,
                aux_bounds,
                body_bounds,
                scrollbar_bounds,
                thumb_bounds,
                collapsed,
                resolved_size: s_main,
            });

            // Header hit regions: actions first (right-to-left so the
            // last-declared action gets the rightmost slot). Then
            // chevron at the leading edge if shown. Title area covers
            // whatever remains.
            push_header_hits(&self.sections[i].header, header_bounds, i, &mut hit_regions);

            // Aux hit region: a single Input / Search / Custom rectangle,
            // or per-action regions for Toolbar.
            if let (Some(aux), Some(ar)) = (&self.sections[i].aux, aux_bounds) {
                if !collapsed {
                    push_aux_hits(aux, ar, i, &mut hit_regions);
                }
            }

            // Body hit region: one rectangle that the host re-tests
            // against the inner body's own hit_test.
            if !collapsed && body_bounds.height > 0.0 && body_bounds.width > 0.0 {
                hit_regions.push((body_bounds, MultiSectionViewHit::Body { section: i }));
            }

            // Scrollbar hit regions. When the body's scroll state is introspectable
            // (Tree, List), split the gutter into TrackBefore / Thumb / TrackAfter so
            // consumers can route page-up and page-down clicks without re-deriving
            // thumb geometry. For overflowing body types without row-based scroll, fall
            // back to a single combined-Thumb region. Hit-region order matters: Thumb
            // first (front-to-back hit_test), then the two tracks.
            if let Some(sb) = scrollbar_bounds {
                if let Some(thumb) = thumb_bounds {
                    hit_regions.push((
                        thumb,
                        MultiSectionViewHit::Scrollbar {
                            section: i,
                            kind: ScrollbarHit::Thumb,
                        },
                    ));
                    let above_h = (thumb.y - sb.y).max(0.0);
                    if above_h > 0.0 {
                        let above = Rect::new(sb.x, sb.y, sb.width, above_h);
                        hit_regions.push((
                            above,
                            MultiSectionViewHit::Scrollbar {
                                section: i,
                                kind: ScrollbarHit::TrackBefore,
                            },
                        ));
                    }
                    let below_y = thumb.y + thumb.height;
                    let below_h = (sb.y + sb.height - below_y).max(0.0);
                    if below_h > 0.0 {
                        let below = Rect::new(sb.x, below_y, sb.width, below_h);
                        hit_regions.push((
                            below,
                            MultiSectionViewHit::Scrollbar {
                                section: i,
                                kind: ScrollbarHit::TrackAfter,
                            },
                        ));
                    }
                } else {
                    hit_regions.push((
                        sb,
                        MultiSectionViewHit::Scrollbar {
                            section: i,
                            kind: ScrollbarHit::Thumb,
                        },
                    ));
                }
            }

            // Divider after this section (not after the last one).
            y += s_main;
            if metrics.divider_size > 0.0 && i + 1 < n && self.allow_resize {
                let d = Rect::new(bounds.x, y, bounds.width, metrics.divider_size);
                dividers.push(DividerBounds {
                    above: i,
                    below: i + 1,
                    bounds: d,
                });
                hit_regions.push((
                    d,
                    MultiSectionViewHit::Divider {
                        above: i,
                        below: i + 1,
                    },
                ));
                y += metrics.divider_size;
            }
        }

        // Panel-level scrollbar (WholePanel mode only). Thumb geometry uses the
        // same canonical fit_thumb helper as per-section scrollbars. The thumb rect
        // is published on the layout (panel_scrollbar_thumb) so callers can read it
        // instead of re-deriving it.
        let (panel_scrollbar, panel_scrollbar_thumb) = match self.scroll_mode {
            ScrollMode::WholePanel => {
                let total_content: f32 = resolved.iter().sum::<f32>() + dividers_total;
                if total_content > bounds.height {
                    let sb_w = metrics.scrollbar_size;
                    let r = Rect::new(
                        bounds.x + bounds.width - sb_w,
                        bounds.y,
                        sb_w,
                        bounds.height,
                    );
                    let min_thumb = panel_thumb_min(&metrics);
                    // Deliberately *not* snapped to `metrics.cell_quantum`
                    // (unlike `compute_thumb_bounds`'s per-section thumb):
                    // a whole-cell `quantize_thumb` pass rounds a
                    // near-full-track thumb *up* to fill the track
                    // exactly, zeroing `travel` and making the panel
                    // scrollbar undraggable under a 1-row overflow — see
                    // `tui_panel_drag_works_in_multi_tree_example_shape`.
                    // The paint side rounds this fractional rect to cells
                    // at render time instead (`tui::multi_section_view::
                    // paint_panel_scrollbar`), same as the per-section
                    // paint path's `.round()` fallback.
                    let (thumb_start, thumb_h) = crate::primitives::scrollbar::fit_thumb(
                        self.panel_scroll,
                        total_content,
                        bounds.height,
                        r.height,
                        min_thumb,
                    );
                    let thumb_y = r.y + thumb_start;
                    if thumb_y > r.y {
                        hit_regions.push((
                            Rect::new(r.x, r.y, r.width, thumb_y - r.y),
                            MultiSectionViewHit::PanelScrollbar {
                                kind: ScrollbarHit::TrackBefore,
                            },
                        ));
                    }
                    let thumb_rect = Rect::new(r.x, thumb_y, r.width, thumb_h);
                    hit_regions.push((
                        thumb_rect,
                        MultiSectionViewHit::PanelScrollbar {
                            kind: ScrollbarHit::Thumb,
                        },
                    ));
                    let thumb_bottom = thumb_y + thumb_h;
                    let track_bottom = r.y + r.height;
                    if thumb_bottom < track_bottom {
                        hit_regions.push((
                            Rect::new(r.x, thumb_bottom, r.width, track_bottom - thumb_bottom),
                            MultiSectionViewHit::PanelScrollbar {
                                kind: ScrollbarHit::TrackAfter,
                            },
                        ));
                    }
                    (Some(r), Some(thumb_rect))
                } else {
                    (None, None)
                }
            }
            ScrollMode::PerSection => (None, None),
        };

        MultiSectionViewLayout {
            bounds,
            axis: Axis::Vertical,
            scroll_mode: self.scroll_mode,
            sections: sections_out,
            dividers,
            panel_scrollbar,
            panel_scrollbar_thumb,
            hit_regions,
        }
    }

    /// Apply a divider drag (Q3 — Fixed-on-drag policy).
    ///
    /// `delta` is the signed main-axis distance the divider moved
    /// (positive = section `above` grew, section `below` shrank).
    /// Both adjacent sections become `SectionSize::Fixed(measured)`,
    /// honouring their `min_size`/`max_size`.
    ///
    /// Returns `true` if either section's size changed; `false` if
    /// `delta` was clamped to zero (already at the boundary).
    pub fn resize_divider(
        &mut self,
        above: usize,
        below: usize,
        current_above: f32,
        current_below: f32,
        delta: f32,
    ) -> bool {
        if above >= self.sections.len() || below >= self.sections.len() {
            return false;
        }
        // Clamp delta against both sections' min/max.
        let (lo_above, hi_above) = size_bounds(&self.sections[above]);
        let (lo_below, hi_below) = size_bounds(&self.sections[below]);
        let max_grow_above = hi_above - current_above;
        let max_shrink_above = current_above - lo_above;
        let max_grow_below = hi_below - current_below;
        let max_shrink_below = current_below - lo_below;
        let mut d = delta;
        if d > 0.0 {
            d = d.min(max_grow_above).min(max_shrink_below);
        } else if d < 0.0 {
            d = (-d).min(max_shrink_above).min(max_grow_below).neg();
        }
        if d == 0.0 {
            return false;
        }
        let new_above = (current_above + d).round() as u16;
        let new_below = (current_below - d).round() as u16;
        self.sections[above].size = SectionSize::Fixed(new_above);
        self.sections[below].size = SectionSize::Fixed(new_below);
        true
    }
}

// ── Internal helpers ───────────────────────────────────────────────────────

trait FloatNeg {
    fn neg(self) -> Self;
}
impl FloatNeg for f32 {
    fn neg(self) -> Self {
        -self
    }
}

fn apply_min_max(s: &Section, raw: f32) -> f32 {
    let mut v = raw;
    if let Some(min) = s.min_size {
        v = v.max(min as f32);
    }
    if let Some(max) = s.max_size {
        v = v.min(max as f32);
    }
    v
}

fn size_bounds(s: &Section) -> (f32, f32) {
    let lo = s.min_size.map(|m| m as f32).unwrap_or(0.0);
    let hi = s.max_size.map(|m| m as f32).unwrap_or(f32::INFINITY);
    (lo, hi)
}

/// Minimum thumb height for the panel-level scrollbar, in the same units
/// the rest of the layout uses (cells for TUI, pixels for GTK).
///
/// `cell_quantum > 0.0` flags TUI: the painter clamps thumb height to 1 cell
/// (`.max(1)` in `paint_panel_scrollbar`), so we mirror that here. GTK keeps
/// the historical 8-pixel floor (clamped up to `scrollbar_size` when the
/// gutter is wider).
///
/// Exposed `pub(crate)` so drag init can compute the same `thumb_h` the
/// layout did, keeping drag travel consistent with the painted thumb's track
/// distance.
pub(crate) fn panel_thumb_min(metrics: &MsvLayoutMetrics) -> f32 {
    if metrics.cell_quantum > 0.0 {
        metrics.cell_quantum
    } else {
        metrics.scrollbar_size.max(8.0)
    }
}

fn body_overflows(s: &Section, content: f32, body_main: f32) -> bool {
    // Empty/Custom never claim overflow — host paints whatever it wants.
    match &s.body {
        SectionBody::Empty(_) | SectionBody::Custom(_) => false,
        _ => content > body_main + 0.5, // tolerate sub-cell rounding
    }
}

/// Inner body's scroll state when introspectable. Returns
/// `(scroll_offset_rows, row_count)` for body types that carry a
/// row-based `scroll_offset` field. `None` for bodies without
/// row-based scroll (`Form`, `Terminal`, `MessageList`, `Text`,
/// `Empty`, `Custom`); the layout falls back to a single
/// combined-Thumb hit region for those.
///
/// Per #9: the primitive introspects the body enum directly rather
/// than threading a host-supplied scroll measure through every
/// `MultiSectionView::layout` call. The scroll state already lives
/// on the inner primitive — there's no second source of truth.
fn body_scroll_state(body: &SectionBody) -> Option<(usize, usize)> {
    match body {
        SectionBody::Tree(t) => Some((t.scroll_offset, t.rows.len())),
        SectionBody::List(l) => {
            let title_row = if l.title.is_some() { 1 } else { 0 };
            Some((l.scroll_offset, l.items.len() + title_row))
        }
        _ => None,
    }
}

/// Snap a [`crate::fit_thumb`] result to whole `cell_quantum` units so
/// paint (which rounds to integer cells in TUI) and hit-test (which
/// otherwise sees raw fractional bounds) agree exactly. A no-op when
/// `cell_quantum <= 0.0` (GTK's fractional-pixel bounds pass through
/// unchanged).
///
/// Used by [`compute_thumb_bounds`] (per-section scrollbars) only.
/// *Not* used for the panel-level scrollbar — see the "Deliberately
/// not snapped" note at that branch's `fit_thumb` call in
/// `MultiSectionView::layout_vertical` for why quantising a
/// near-full-track panel thumb would zero out drag `travel`. Panel
/// paint rounds its fractional rect to cells at render time instead
/// (`tui::multi_section_view::paint_panel_scrollbar`).
pub(crate) fn quantize_thumb(
    thumb_start: f32,
    thumb_len: f32,
    track_len: f32,
    cell_quantum: f32,
) -> (f32, f32) {
    if cell_quantum <= 0.0 {
        return (thumb_start, thumb_len);
    }
    let q = cell_quantum;
    let start_q = (thumb_start / q).floor() as i32;
    let end_q = ((thumb_start + thumb_len) / q).ceil() as i32;
    let max_end_q = (track_len / q).round() as i32;
    let end_q = end_q.min(max_end_q);
    let len_q = (end_q - start_q).max(1);
    let start_q = start_q.min(max_end_q - len_q).max(0);
    (start_q as f32 * q, len_q as f32 * q)
}

/// Compute thumb bounds for a per-section scrollbar.
///
/// Shares [`crate::fit_thumb`] with the panel-level scrollbar (see the
/// `fit_thumb` call in the `WholePanel` branch of
/// `MultiSectionView::layout_vertical`) so both kinds of scrollbar size
/// and position their thumb identically. Unlike the panel-level branch,
/// this additionally snaps to `cell_quantum` via [`quantize_thumb`] when
/// set, so paint and hit-test agree on integer cells (TUI) — per-section
/// thumbs don't have the panel scrollbar's near-full-track drag-`travel`
/// concern (see `quantize_thumb`'s doc), so there's no reason not to.
/// Falls through to fractional bounds when `cell_quantum == 0.0` (GTK).
fn compute_thumb_bounds(
    gutter: Rect,
    scroll_rows: usize,
    row_count: usize,
    content_size: f32,
    cell_quantum: f32,
) -> Rect {
    if row_count == 0 || gutter.height <= 0.0 || content_size <= 0.0 {
        return gutter;
    }
    // Convert row-indexed scroll into the same main-axis units as
    // content_size: row_main_unit = content_size / row_count.
    let row_main_unit = content_size / row_count as f32;
    let scroll_main = scroll_rows as f32 * row_main_unit;
    let visible = gutter.height; // viewport main-axis size = gutter height
    let track_len = gutter.height;
    let min_thumb_len = if cell_quantum > 0.0 {
        cell_quantum
    } else {
        1.0
    };
    let (thumb_start, thumb_len) = crate::primitives::scrollbar::fit_thumb(
        scroll_main,
        content_size,
        visible,
        track_len,
        min_thumb_len,
    );
    let (thumb_start, thumb_len) = quantize_thumb(thumb_start, thumb_len, track_len, cell_quantum);
    Rect::new(gutter.x, gutter.y + thumb_start, gutter.width, thumb_len)
}

/// Three-pass main-axis size resolution for `ScrollMode::PerSection`.
///
/// Pass 1 (Fixed): allocate `SectionSize::Fixed`, `Content` /
/// `ContentClamped`, and collapsed-section header heights.
/// Pass 2 (Percent): allocate `SectionSize::Percent` against the
/// *original* container size; if total overflows the remainder, scale
/// proportionally.
/// Pass 3 (Flex): distribute leftover space across `Weight`,
/// `EqualShare`. `Content` already consumed its content size in pass 1.
fn resolve_per_section_sizes(
    sections: &[Section],
    measures: &[SectionMeasure],
    chrome_cost: &[f32],
    container_main: f32,
    usable_main: f32,
) -> Vec<f32> {
    let n = sections.len();
    let mut out = vec![0.0_f32; n];

    // Pass 1: Fixed + Content (collapsed sections always reduce to chrome
    // cost only).
    let mut consumed = 0.0_f32;
    let mut pass1_done = vec![false; n];
    for i in 0..n {
        if sections[i].collapsed {
            // Collapsed: only the header (chrome_cost includes only
            // header for collapsed).
            out[i] = apply_min_max(&sections[i], chrome_cost[i]);
            consumed += out[i];
            pass1_done[i] = true;
            continue;
        }
        match sections[i].size {
            SectionSize::Fixed(rows) => {
                let v = apply_min_max(&sections[i], rows as f32);
                out[i] = v;
                consumed += v;
                pass1_done[i] = true;
            }
            SectionSize::Content => {
                let raw = chrome_cost[i] + measures[i].content_size;
                let v = apply_min_max(&sections[i], raw);
                out[i] = v;
                consumed += v;
                pass1_done[i] = true;
            }
            _ => {}
        }
    }

    // Pass 2: Percent (allocated against container_main).
    let mut percent_indices: Vec<usize> = Vec::new();
    let mut percent_raw: Vec<f32> = Vec::new();
    for i in 0..n {
        if pass1_done[i] {
            continue;
        }
        if let SectionSize::Percent(p) = sections[i].size {
            percent_indices.push(i);
            percent_raw.push(p.clamp(0.0, 1.0) * container_main);
        }
    }
    let percent_total: f32 = percent_raw.iter().sum();
    let percent_budget = (usable_main - consumed).max(0.0);
    let percent_scale = if percent_total > percent_budget && percent_total > 0.0 {
        percent_budget / percent_total
    } else {
        1.0
    };
    for (k, &i) in percent_indices.iter().enumerate() {
        let raw = percent_raw[k] * percent_scale;
        let v = apply_min_max(&sections[i], raw.max(chrome_cost[i]));
        out[i] = v;
        consumed += v;
        pass1_done[i] = true;
    }

    // Pass 3: Weight + EqualShare share remainder.
    let remainder = (usable_main - consumed).max(0.0);
    let mut flex_indices: Vec<usize> = Vec::new();
    let mut weights: Vec<f32> = Vec::new();
    for i in 0..n {
        if pass1_done[i] {
            continue;
        }
        match sections[i].size {
            SectionSize::Weight(w) => {
                flex_indices.push(i);
                weights.push(w.max(0.0));
            }
            SectionSize::EqualShare => {
                flex_indices.push(i);
                weights.push(1.0);
            }
            // Fixed/Content/Percent already handled above.
            _ => {}
        }
    }
    let weight_sum: f32 = weights.iter().sum();
    if !flex_indices.is_empty() && weight_sum > 0.0 {
        for (k, &i) in flex_indices.iter().enumerate() {
            let raw = remainder * (weights[k] / weight_sum);
            let v = apply_min_max(&sections[i], raw.max(chrome_cost[i]));
            out[i] = v;
        }
    } else {
        // No flex sections — leftover space is unused. Apps that want
        // to absorb it should add an `EqualShare` section.
        for &i in &flex_indices {
            out[i] = apply_min_max(&sections[i], chrome_cost[i]);
        }
    }

    out
}

fn push_header_hits(
    header: &SectionHeader,
    bounds: Rect,
    section: usize,
    out: &mut Vec<(Rect, MultiSectionViewHit)>,
) {
    // Estimated chevron width (1 unit). The exact glyph width is
    // backend-dependent; this is a hit-test approximation that matches
    // typical 1-cell or icon-width footprints.
    let chevron_w = if header.show_chevron { 2.0 } else { 0.0 };

    // Action buttons: right-to-left, 2 units each (icon + spacing).
    // Disabled actions don't reserve a hit region — clicks fall through
    // to TitleArea.
    let action_w = 2.0;
    let mut right_cursor = bounds.x + bounds.width;
    for action in header.actions.iter().rev() {
        if !action.enabled {
            continue;
        }
        let r = Rect::new(right_cursor - action_w, bounds.y, action_w, bounds.height);
        out.push((
            r,
            MultiSectionViewHit::Header {
                section,
                kind: HeaderHit::Action(action.id.clone()),
            },
        ));
        right_cursor -= action_w;
    }

    // Chevron region (leading edge).
    if header.show_chevron {
        let r = Rect::new(bounds.x, bounds.y, chevron_w, bounds.height);
        out.push((
            r,
            MultiSectionViewHit::Header {
                section,
                kind: HeaderHit::Chevron,
            },
        ));
    }

    // TitleArea covers the strip between chevron and the leftmost
    // enabled action button.
    let title_x = bounds.x + chevron_w;
    let title_w = (right_cursor - title_x).max(0.0);
    if title_w > 0.0 {
        let r = Rect::new(title_x, bounds.y, title_w, bounds.height);
        out.push((
            r,
            MultiSectionViewHit::Header {
                section,
                kind: HeaderHit::TitleArea,
            },
        ));
    }
}

fn push_aux_hits(
    aux: &SectionAux,
    bounds: Rect,
    section: usize,
    out: &mut Vec<(Rect, MultiSectionViewHit)>,
) {
    match aux {
        SectionAux::Input(_) | SectionAux::Search(_) => {
            out.push((
                bounds,
                MultiSectionViewHit::Aux {
                    section,
                    kind: AuxHit::Input,
                },
            ));
        }
        SectionAux::Toolbar(actions) => {
            let action_w = 2.0;
            let mut x = bounds.x;
            for a in actions {
                if !a.enabled {
                    x += action_w;
                    continue;
                }
                let r = Rect::new(x, bounds.y, action_w, bounds.height);
                out.push((
                    r,
                    MultiSectionViewHit::Aux {
                        section,
                        kind: AuxHit::Action(a.id.clone()),
                    },
                ));
                x += action_w;
            }
        }
        SectionAux::Custom(_) => {
            out.push((
                bounds,
                MultiSectionViewHit::Aux {
                    section,
                    kind: AuxHit::Custom,
                },
            ));
        }
    }
}

// ── Metrics and body measurement ───────────────────────────────────────────

/// The [`MsvLayoutMetrics`] a pixel surface lays a view out with at
/// `line_height`: a header `1.4 ×` the line height, an 8px scrollbar
/// gutter, a 1px divider when sections are resizable, and no cell
/// quantisation.
pub fn msv_metrics(line_height: f64, allow_resize: bool) -> MsvLayoutMetrics {
    MsvLayoutMetrics {
        header_size: (line_height * 1.4) as f32,
        divider_size: if allow_resize { 1.0 } else { 0.0 },
        scrollbar_size: 8.0,
        cell_quantum: 0.0,
    }
}

/// The [`MsvLayoutMetrics`] a cell grid lays a view out with: one cell
/// per header row, scrollbar column and divider (the divider only when
/// sections are resizable), and section sizes snapped to whole cells so
/// paint and hit-test agree on every row.
pub fn cell_msv_metrics(allow_resize: bool) -> MsvLayoutMetrics {
    MsvLayoutMetrics {
        header_size: 1.0,
        divider_size: if allow_resize { 1.0 } else { 0.0 },
        scrollbar_size: 1.0,
        cell_quantum: 1.0,
    }
}

/// One section's body content size and aux row size at `line_height` —
/// whole cells when `line_height` is `1.0`, pixels otherwise.
///
/// Tree header rows take `1.2 ×` the line height and every other tree,
/// list, form or empty-state row `1.4 ×`, each rounded; a list title, a
/// text line and each message line take one line height, and a message
/// takes one header line plus at least one text line. Terminal and
/// custom bodies report no natural size.
pub fn msv_body_measure(
    body: &SectionBody,
    aux: &Option<SectionAux>,
    line_height: f64,
) -> SectionMeasure {
    let item_h = (line_height * 1.4).round() as f32;
    let aux_size = if aux.is_some() { item_h } else { 0.0 };
    let content_size = match body {
        SectionBody::Tree(t) => {
            let header_h = (line_height * 1.2).round() as f32;
            t.rows
                .iter()
                .map(|row| {
                    if matches!(row.decoration, Decoration::Header) {
                        header_h
                    } else {
                        item_h
                    }
                })
                .sum()
        }
        SectionBody::List(l) => {
            let title_h = if l.title.is_some() {
                line_height as f32
            } else {
                0.0
            };
            title_h + l.items.len() as f32 * item_h
        }
        SectionBody::Form(f) => f.fields.len() as f32 * item_h,
        SectionBody::Chart(c) => {
            if matches!(c.kind, crate::primitives::chart::ChartKind::Sparkline) {
                line_height as f32
            } else {
                item_h * 8.0
            }
        }
        SectionBody::MessageList(m) => m
            .rows
            .iter()
            .map(|r| {
                let lines = r.text.lines().count().max(1) as f32;
                line_height as f32 + lines * line_height as f32
            })
            .sum(),
        SectionBody::Terminal(_) => 0.0,
        SectionBody::Text(lines) => lines.len() as f32 * line_height as f32,
        SectionBody::Empty(_) => item_h * 4.0,
        SectionBody::Custom(_) => 0.0,
    };
    SectionMeasure {
        content_size,
        aux_size,
    }
}

/// The chrome layout a pixel surface resolves for `view` in `bounds` at
/// `line_height` — [`msv_metrics`] plus [`msv_body_measure`]. A host
/// hit-testing a view it paints through a pixel backend calls this with
/// the backend's chrome line height, the same value the paint used.
pub fn msv_layout(
    view: &MultiSectionView,
    bounds: Rect,
    line_height: f64,
) -> MultiSectionViewLayout {
    let metrics = msv_metrics(line_height, view.allow_resize);
    view.layout(bounds, metrics, |i| {
        msv_body_measure(&view.sections[i].body, &view.sections[i].aux, line_height)
    })
}

/// The chrome layout a cell grid resolves for `view` in `bounds` —
/// [`cell_msv_metrics`] plus [`msv_body_measure`] at one cell per line.
pub fn cell_msv_layout(view: &MultiSectionView, bounds: Rect) -> MultiSectionViewLayout {
    let metrics = cell_msv_metrics(view.allow_resize);
    view.layout(bounds, metrics, |i| {
        msv_body_measure(&view.sections[i].body, &view.sections[i].aux, 1.0)
    })
}

// ── Contract layout ────────────────────────────────────────────────────────

#[cfg(feature = "tui")]
pub(crate) use contract_layout::clip_rows;
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use contract_layout::{MultiSectionViewPrimitiveLayout, SectionBodyLayout, SectionBodyView};

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
mod contract_layout {
    use super::{cell_msv_layout, msv_layout};
    use crate::event::Rect;
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::primitives::chart::ChartPrimitiveLayout;
    use crate::primitives::form::FormPrimitiveLayout;
    use crate::primitives::list::ListViewPrimitiveLayout;
    use crate::primitives::message_list::MessageListLayout;
    use crate::primitives::multi_section_view::{
        MultiSectionView, MultiSectionViewHit, MultiSectionViewLayout, SectionBody, SectionLayout,
    };
    use crate::primitives::terminal::TerminalLayout;
    use crate::primitives::tree::TreeViewPrimitiveLayout;

    /// [`Primitive::Layout`] for a [`MultiSectionView`]: the chrome layout
    /// every host already hit-tests against, plus the layout each visible
    /// body resolved through its own widget's [`Primitive::layout`].
    #[derive(Debug, Clone, PartialEq)]
    #[non_exhaustive]
    pub struct MultiSectionViewPrimitiveLayout {
        /// Header, aux, body, scrollbar and divider bounds, in the frame
        /// the view was laid out in.
        pub chrome: MultiSectionViewLayout,
        /// The unit the layout was resolved in.
        pub units: Units,
        /// One standard row in [`Self::units`]: the step a wheel notch and
        /// a panel arrow key scroll by.
        pub line_height: f32,
        /// One character advance in [`Self::units`], which places the
        /// caret under a click in an aux input.
        pub char_width: f32,
        /// One entry per section, indexed like
        /// [`MultiSectionView::sections`]: the body's own layout, or
        /// `None` when the section is collapsed, its body is off screen,
        /// or the host paints it ([`SectionBody::Custom`]).
        pub bodies: Vec<Option<SectionBodyView>>,
    }

    impl MultiSectionViewPrimitiveLayout {
        /// Which chrome region contains the absolute point `(x, y)` — see
        /// [`MultiSectionViewLayout::hit_test`].
        pub fn hit_test(&self, x: f32, y: f32) -> MultiSectionViewHit {
            self.chrome.hit_test(x, y)
        }

        /// The resolved chrome bounds of section `idx`.
        pub fn section(&self, idx: usize) -> Option<&SectionLayout> {
            self.chrome.sections.get(idx)
        }

        /// The resolved body of section `idx`, when it has one.
        pub fn body(&self, idx: usize) -> Option<&SectionBodyView> {
            self.bodies.get(idx).and_then(Option::as_ref)
        }
    }

    /// One visible section body: where its widget was laid out, what part
    /// of that is on screen, and the widget's own layout.
    #[derive(Debug, Clone, PartialEq)]
    #[non_exhaustive]
    pub struct SectionBodyView {
        /// The rect the body widget was laid out in, in absolute
        /// coordinates; a forwarded pointer event is made local to it.
        pub rect: Rect,
        /// The part of [`Self::rect`] inside the view's bounds — the clip
        /// the body paints inside.
        pub clip: Rect,
        /// The descriptor the body was laid out and painted from when it
        /// differs from the section's own: a tree or list whose scrollbar
        /// the section's gutter already draws, or one whose rows scroll
        /// past a clipped top edge on a cell grid. `None` paints the
        /// section's own body.
        pub body: Option<SectionBody>,
        /// The body widget's own resolved layout.
        pub layout: SectionBodyLayout,
    }

    /// A section body's own [`Primitive::Layout`], one variant per body
    /// widget; `Text` and `Empty` bodies carry the rect this module's own
    /// painters fill.
    #[derive(Debug, Clone, PartialEq)]
    #[non_exhaustive]
    pub enum SectionBodyLayout {
        Tree(TreeViewPrimitiveLayout),
        List(ListViewPrimitiveLayout),
        Form(FormPrimitiveLayout),
        Terminal(TerminalLayout),
        MessageList(MessageListLayout),
        Chart(ChartPrimitiveLayout),
        Text(Rect),
        Empty(Rect),
    }

    /// The part of `r` that lies between `top` and `bottom`, or `None`
    /// when none of it does.
    pub(crate) fn clip_rows(r: Rect, top: f32, bottom: f32) -> Option<Rect> {
        let r_bottom = r.y + r.height;
        if r.height <= 0.0 || r_bottom <= top || r.y >= bottom {
            return None;
        }
        let y = r.y.max(top);
        let h = r_bottom.min(bottom) - y;
        (h > 0.0).then(|| Rect::new(r.x, y, r.width, h))
    }

    impl MultiSectionView {
        /// The contract layout for `rect`: the chrome layout in `ctx`'s
        /// units, then every expanded, on-screen body laid out through its
        /// own widget.
        pub(crate) fn resolve_layout(
            &self,
            ctx: &LayoutCtx,
            rect: Rect,
        ) -> MultiSectionViewPrimitiveLayout {
            let units = ctx.units();
            let (chrome, line_height, char_width) = match units {
                Units::Cells => (cell_msv_layout(self, rect), 1.0, 1.0),
                _ => {
                    let lh = ctx.chrome_line_height();
                    (msv_layout(self, rect, lh as f64), lh, ctx.char_width)
                }
            };
            let bodies = chrome
                .sections
                .iter()
                .map(|s| self.body_view(ctx, units, &chrome, s))
                .collect();
            MultiSectionViewPrimitiveLayout {
                chrome,
                units,
                line_height,
                char_width,
                bodies,
            }
        }

        fn body_view(
            &self,
            ctx: &LayoutCtx,
            units: Units,
            chrome: &MultiSectionViewLayout,
            s: &SectionLayout,
        ) -> Option<SectionBodyView> {
            if s.collapsed || s.body_bounds.width <= 0.0 || s.body_bounds.height <= 0.0 {
                return None;
            }
            let section = &self.sections[s.section_idx];
            let top = chrome.bounds.y;
            let clip = clip_rows(s.body_bounds, top, top + chrome.bounds.height)?;
            // A cell grid cannot clip a body mid-row, so a body scrolled
            // past the top edge is laid out in its visible rows only, with
            // the hidden rows added to its own scroll offset.
            let (rect, skipped) = match units {
                Units::Cells => (clip, (clip.y - s.body_bounds.y).round().max(0.0) as usize),
                _ => (s.body_bounds, 0),
            };
            // The section's gutter draws the scrollbar for a tree or list
            // body, so the body widget does not draw a second one.
            let gutter = s.scrollbar_bounds.is_some();
            let adjusted = match &section.body {
                SectionBody::Tree(t) if (gutter && t.style.show_scrollbar) || skipped > 0 => {
                    let mut t = t.clone();
                    t.style.show_scrollbar &= !gutter;
                    t.scroll_offset = t.scroll_offset.saturating_add(skipped);
                    Some(SectionBody::Tree(t))
                }
                SectionBody::List(l) if (gutter && l.show_v_scrollbar) || skipped > 0 => {
                    let mut l = l.clone();
                    l.show_v_scrollbar &= !gutter;
                    l.scroll_offset = l.scroll_offset.saturating_add(skipped);
                    Some(SectionBody::List(l))
                }
                _ => None,
            };
            let body = adjusted.as_ref().unwrap_or(&section.body);
            let layout = match body {
                SectionBody::Tree(t) => SectionBodyLayout::Tree(Primitive::layout(t, ctx, rect)),
                SectionBody::List(l) => SectionBodyLayout::List(Primitive::layout(l, ctx, rect)),
                SectionBody::Form(f) => SectionBodyLayout::Form(Primitive::layout(f, ctx, rect)),
                SectionBody::Terminal(t) => {
                    SectionBodyLayout::Terminal(Primitive::layout(t, ctx, rect))
                }
                SectionBody::MessageList(m) => {
                    SectionBodyLayout::MessageList(Primitive::layout(m, ctx, rect))
                }
                SectionBody::Chart(c) => SectionBodyLayout::Chart(Primitive::layout(c, ctx, rect)),
                SectionBody::Text(_) => SectionBodyLayout::Text(rect),
                SectionBody::Empty(_) => SectionBodyLayout::Empty(rect),
                SectionBody::Custom(_) => return None,
            };
            Some(SectionBodyView {
                rect,
                clip,
                body: adjusted,
                layout,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::message_list::{MessageList, MessageRow};
    use crate::types::{Color, WidgetId};

    /// A message measures one header line plus at least one text line in
    /// both units, so an empty message reserves the same rows on a cell
    /// grid as on a pixel surface.
    #[test]
    fn an_empty_message_measures_two_lines_in_cells_and_pixels() {
        let body = SectionBody::MessageList(MessageList {
            id: WidgetId::new("msgs"),
            rows: vec![MessageRow::new("", Color::rgb(255, 255, 255), 0.0)],
            scroll_top: 0,
        });
        assert_eq!(msv_body_measure(&body, &None, 1.0).content_size, 2.0);
        assert_eq!(msv_body_measure(&body, &None, 20.0).content_size, 40.0);
    }

    #[test]
    fn message_list_measures_its_real_height() {
        let body = SectionBody::MessageList(MessageList {
            id: WidgetId::new("msgs"),
            rows: vec![MessageRow::new(
                "line one\nline two",
                Color::rgb(255, 255, 255),
                0.0,
            )],
            scroll_top: 0,
        });
        assert_eq!(
            msv_body_measure(&body, &None, 20.0).content_size,
            20.0 + 2.0 * 20.0
        );
    }
}
