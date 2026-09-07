//! Drop-zone computation for tab drag-and-drop.
//!
//! Given a set of group rectangles (each with per-tab slot positions)
//! and a cursor position, [`compute_drop_zone`] determines where a
//! dragged tab would land: center of a group, an edge split, or a
//! reorder position within the tab bar.
//!
//! [`DropOverlay`] translates a [`DropZone`] result into visual
//! overlay geometry (highlight rect, insertion bar, ghost label
//! position) that backends render on top of the normal frame.

use crate::event::Rect;
use serde::{Deserialize, Serialize};

/// A group's bounds and per-tab slot positions, supplied by the
/// consumer's layout system. `tab_slots` contains `(start_x, end_x)`
/// pairs in the same coordinate system as `bounds`.
#[derive(Debug, Clone)]
pub struct DropGroupRect {
    pub bounds: Rect,
    pub tab_slots: Vec<(f32, f32)>,
}

/// Cardinal direction for a split drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DropEdge {
    Left,
    Right,
    Top,
    Bottom,
}

/// What kind of drop zone the cursor is over.
#[derive(Debug, Clone, PartialEq)]
pub enum DropZoneKind {
    /// Drop onto the center — tab joins this group.
    Center,
    /// Drop on an edge — split the group in this direction.
    Split(DropEdge),
    /// Drop between tabs — reorder within the tab bar.
    /// The `usize` is the insertion index (0 = before first tab,
    /// `tab_count` = after last tab).
    TabReorder(usize),
}

/// Result of [`compute_drop_zone`].
#[derive(Debug, Clone, PartialEq)]
pub struct DropZone {
    pub kind: DropZoneKind,
    /// Which group (index into the `groups` slice) this targets.
    pub group_idx: usize,
}

/// Compute which drop zone a cursor position falls into.
///
/// `tab_bar_height` is the height of the tab bar region at the top of
/// each group. Cursor positions within the tab bar region produce
/// `TabReorder`; positions in the content area below produce `Center`
/// or `Split` depending on edge proximity.
///
/// Returns `None` if the cursor is outside all groups.
pub fn compute_drop_zone(
    cursor_x: f32,
    cursor_y: f32,
    groups: &[DropGroupRect],
    tab_bar_height: f32,
) -> Option<DropZone> {
    for (gi, group) in groups.iter().enumerate() {
        let b = &group.bounds;
        if cursor_x < b.x
            || cursor_x >= b.x + b.width
            || cursor_y < b.y
            || cursor_y >= b.y + b.height
        {
            continue;
        }

        let in_tab_bar = cursor_y < b.y + tab_bar_height && tab_bar_height > 0.0;

        if in_tab_bar && !group.tab_slots.is_empty() {
            let insert_idx = tab_reorder_index(cursor_x, &group.tab_slots);
            return Some(DropZone {
                kind: DropZoneKind::TabReorder(insert_idx),
                group_idx: gi,
            });
        }

        let content_y = b.y + tab_bar_height;
        let content_h = (b.height - tab_bar_height).max(0.0);
        if content_h <= 0.0 {
            return Some(DropZone {
                kind: DropZoneKind::Center,
                group_idx: gi,
            });
        }

        let edge_w = edge_zone_size(b.width);
        let edge_h = edge_zone_size(content_h);

        let rel_x = cursor_x - b.x;
        let rel_y = cursor_y - content_y;

        if rel_x < edge_w {
            return Some(DropZone {
                kind: DropZoneKind::Split(DropEdge::Left),
                group_idx: gi,
            });
        }
        if rel_x >= b.width - edge_w {
            return Some(DropZone {
                kind: DropZoneKind::Split(DropEdge::Right),
                group_idx: gi,
            });
        }
        if rel_y < edge_h {
            return Some(DropZone {
                kind: DropZoneKind::Split(DropEdge::Top),
                group_idx: gi,
            });
        }
        if rel_y >= content_h - edge_h {
            return Some(DropZone {
                kind: DropZoneKind::Split(DropEdge::Bottom),
                group_idx: gi,
            });
        }

        return Some(DropZone {
            kind: DropZoneKind::Center,
            group_idx: gi,
        });
    }
    None
}

/// Hit-test classification for a cursor position against a set of
/// [`DropGroupRect`]s (quadraui#818).
#[derive(Debug, Clone, PartialEq)]
pub enum DropZoneHit {
    /// Cursor is over a live drop zone.
    Zone(DropZone),
    /// Cursor is outside every group.
    Empty,
}

/// Hit-test a cursor position against `groups`, following this
/// primitive's `Empty`-variant convention on top of [`compute_drop_zone`]
/// (quadraui#818). Identical arguments and behaviour to
/// [`compute_drop_zone`] — this only wraps its `Option` result in a
/// [`DropZoneHit`] so `DropZone`, like every other primitive, has a
/// `hit_test` entry point with a named miss case instead of `None`.
pub fn drop_zone_hit_test(
    cursor_x: f32,
    cursor_y: f32,
    groups: &[DropGroupRect],
    tab_bar_height: f32,
) -> DropZoneHit {
    match compute_drop_zone(cursor_x, cursor_y, groups, tab_bar_height) {
        Some(zone) => DropZoneHit::Zone(zone),
        None => DropZoneHit::Empty,
    }
}

/// Compute the insertion index from cursor x and tab slot positions.
/// Finds the midpoint of each tab; cursor left of midpoint inserts
/// before, right inserts after.
fn tab_reorder_index(cursor_x: f32, slots: &[(f32, f32)]) -> usize {
    for (i, (start, end)) in slots.iter().enumerate() {
        let mid = (*start + *end) / 2.0;
        if cursor_x < mid {
            return i;
        }
    }
    slots.len()
}

/// Edge zone size: 20% of dimension, clamped to [3, dimension/2].
fn edge_zone_size(dimension: f32) -> f32 {
    (dimension * 0.2).clamp(3.0, dimension / 2.0)
}

// ── Drop overlay ────────────────────────────────────────────────────────────

/// Visual overlay components for rendering a drop zone indicator.
/// Backends draw these on top of the normal frame during a tab drag.
#[derive(Debug, Clone, PartialEq)]
pub struct DropOverlay {
    /// Tinted rectangle highlighting the target zone.
    pub highlight: Option<Rect>,
    /// Vertical or horizontal insertion bar for tab reorder.
    pub insertion_bar: Option<Rect>,
    /// Ghost label position: `(x, y)` near the cursor.
    pub ghost_position: Option<(f32, f32)>,
}

impl DropOverlay {
    /// Alpha applied to the highlight tint. Shared by every backend
    /// rasteriser (`gtk::drop_overlay`, `macos::drop_overlay`,
    /// `win::drop_overlay`) so they agree on a value by construction
    /// rather than by three separately-copied literals — see
    /// `PRIMITIVE_RULES.md`'s primitive-first rule (#713).
    pub const HIGHLIGHT_ALPHA: f32 = 0.15;

    /// Minimum insertion-bar thickness (px on GTK/macOS, DIPs on
    /// Win-GUI), applied when [`DropOverlay::insertion_bar`]'s computed
    /// width/height would otherwise be thinner (e.g. a zero-width bar
    /// from [`drop_zone_overlay`]).
    pub const MIN_BAR_THICKNESS: f32 = 2.0;
}

/// Compute overlay geometry for a drop zone.
///
/// `bar_height` is the thickness of the insertion bar (typically 2–3
/// pixels in GTK, 1 cell in TUI). `ghost_offset` is how far the ghost
/// label floats from the cursor (typically one line height).
pub fn drop_zone_overlay(
    zone: &DropZone,
    groups: &[DropGroupRect],
    cursor_x: f32,
    cursor_y: f32,
    tab_bar_height: f32,
    bar_thickness: f32,
    ghost_offset: f32,
) -> DropOverlay {
    let b = groups[zone.group_idx].bounds;
    let content_y = b.y + tab_bar_height;
    let content_h = (b.height - tab_bar_height).max(0.0);

    match &zone.kind {
        DropZoneKind::Center => DropOverlay {
            highlight: Some(Rect::new(b.x, content_y, b.width, content_h)),
            insertion_bar: None,
            ghost_position: Some((cursor_x + ghost_offset, cursor_y)),
        },
        DropZoneKind::Split(dir) => {
            let half_w = b.width / 2.0;
            let half_h = content_h / 2.0;
            let highlight = match dir {
                DropEdge::Left => Rect::new(b.x, content_y, half_w, content_h),
                DropEdge::Right => Rect::new(b.x + half_w, content_y, half_w, content_h),
                DropEdge::Top => Rect::new(b.x, content_y, b.width, half_h),
                DropEdge::Bottom => Rect::new(b.x, content_y + half_h, b.width, half_h),
            };
            DropOverlay {
                highlight: Some(highlight),
                insertion_bar: None,
                ghost_position: Some((cursor_x + ghost_offset, cursor_y)),
            }
        }
        DropZoneKind::TabReorder(idx) => {
            let slots = &groups[zone.group_idx].tab_slots;
            let bar_x = if *idx == 0 {
                slots.first().map_or(b.x, |(s, _)| *s)
            } else if *idx >= slots.len() {
                slots.last().map_or(b.x, |(_, e)| *e)
            } else {
                let (_, prev_end) = slots[*idx - 1];
                let (next_start, _) = slots[*idx];
                (prev_end + next_start) / 2.0
            };
            DropOverlay {
                highlight: None,
                insertion_bar: Some(Rect::new(
                    bar_x - bar_thickness / 2.0,
                    b.y,
                    bar_thickness,
                    tab_bar_height,
                )),
                ghost_position: Some((cursor_x + ghost_offset, cursor_y)),
            }
        }
    }
}

// ── NativeSurface paint (#865, Phase 2d slice 8/9 of the NativeSurface
// milestone, #811 / #785) ───────────────────────────────────────────────
//
// Before this, `gtk::draw_drop_overlay` (Cairo), `macos::drop_overlay::
// draw_drop_overlay` (Core Graphics) and `win::drop_overlay::
// draw_drop_overlay` (Direct2D) each independently painted the same
// highlight-rect + insertion-bar geometry with their own drawing API.
// `paint` below is the one shared implementation, written against
// [`crate::native_surface::NativeSurface`] (#807, Phase 1) instead of
// any one backend's drawing API — same shape as `scrollbar`'s #811
// slice 1/9 migration (commit `2a47547`).
//
// ## The one divergence this migration found (re-verified, reported here
// rather than silently resolved — CLAUDE.md's "re-verify before you
// implement")
//
// GTK's `cr.set_source_rgba` and macOS's `CGContextSetRGBFillColor` both
// painted the highlight tint as a *real* alpha-blended fill straight onto
// whatever the frame already had underneath. Windows's pre-migration
// `win::drop_overlay::draw_drop_overlay` did not: its render target is
// created with `D2D1_ALPHA_MODE_IGNORE`/`UNKNOWN`, so that module's
// author CPU-premixed the tint against `theme.background` via
// `win::text::blend` before an opaque fill — a real behavioural
// difference from GTK/macOS (a halo of `theme.background` rather than a
// blend of *actual* content) whenever a drop overlay paints over
// anything other than bare theme background (an editor, a terminal, a
// panel header).
//
// That is the same class of bug quadraui#791 fixed for
// `win::scrollbar::draw_scrollbar` — but unlike scrollbar's migration
// (slice 1/9), where the win side had *already* been fixed independently
// before the migration started, drop_overlay's win-side premix was still
// live going into this slice. `NativeSurface::surface_fill_rect` itself
// — the shared verb this `paint` fn calls — has painted a real alpha
// blend on every one of the three pixel backends since #791 (Windows)
// and slice 1/9 (GTK's `cr.set_source_rgba` fix; macOS's
// `CGContextSetRGBFillColor` already did). Routing this primitive's
// paint through the (already-correct) shared verb therefore *fixes*
// Windows's drop-overlay highlight to match GTK/macOS's real-blend
// behaviour, rather than reproducing the old CPU-premix — the opposite
// direction of slice 1/9's GTK fix, landing at the same shared verb. See
// `win::drop_overlay`'s module doc for how the deprecated shim's test
// suite pins the corrected behaviour.
//
// `#[allow(dead_code)]`: see `primitives::scrollbar`'s identical note —
// only *called* once a real pixel backend is compiled in, exercised by
// each backend's own `Backend::draw_drop_overlay` call sites plus this
// module's own `RecordingSurface` tests on every leg that enables one of
// the three cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::DropOverlay;
    use crate::native_surface::NativeSurface;
    use crate::theme::Theme;
    use crate::types::Color;

    /// `color` with its alpha channel replaced by `alpha` (`0.0`-`1.0`).
    fn with_alpha(color: Color, alpha: f32) -> Color {
        Color::rgba(
            color.r,
            color.g,
            color.b,
            (255.0 * alpha.clamp(0.0, 1.0)).round() as u8,
        )
    }

    /// Paint a [`DropOverlay`] onto `surface`: a translucent highlight
    /// rect at [`DropOverlay::HIGHLIGHT_ALPHA`] and/or a solid insertion
    /// bar widened to at least [`DropOverlay::MIN_BAR_THICKNESS`], both
    /// in `theme.accent_fg`. `ghost_position` is not painted here — no
    /// backend renders a ghost label (see the per-backend module docs),
    /// so callers that want one draw it themselves on top.
    pub(crate) fn paint(overlay: &DropOverlay, surface: &mut dyn NativeSurface, theme: &Theme) {
        if let Some(h) = overlay.highlight {
            if h.width > 0.0 && h.height > 0.0 {
                surface.surface_fill_rect(
                    h,
                    with_alpha(theme.accent_fg, DropOverlay::HIGHLIGHT_ALPHA),
                );
            }
        }

        if let Some(bar) = overlay.insertion_bar {
            if bar.height > 0.0 {
                let widened = crate::event::Rect::new(
                    bar.x,
                    bar.y,
                    bar.width.max(DropOverlay::MIN_BAR_THICKNESS),
                    bar.height,
                );
                surface.surface_fill_rect(widened, theme.accent_fg);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Rect, Viewport};
        use crate::Image;

        /// Records every `surface_fill_rect` call — mirrors
        /// `primitives::scrollbar`'s `RecordingSurface` test double,
        /// scoped to just the verb this primitive uses, so this test
        /// runs on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
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
            fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
                (0.0, 0.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        /// Regression pinning this migration's found divergence: the
        /// highlight must carry real alpha (`Color::a < 255`), matching
        /// GTK/macOS's pre-migration behaviour and *correcting*
        /// Windows's pre-migration CPU-premix (see this module's doc).
        #[test]
        fn highlight_paints_with_real_alpha_not_opaque() {
            let overlay = DropOverlay {
                highlight: Some(Rect::new(10.0, 10.0, 50.0, 30.0)),
                insertion_bar: None,
                ghost_position: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &theme);

            assert_eq!(
                surface.fills.len(),
                1,
                "expected exactly the highlight fill"
            );
            let (rect, color) = surface.fills[0];
            assert_eq!(rect, Rect::new(10.0, 10.0, 50.0, 30.0));
            assert!(
                color.a < 255,
                "highlight fill must carry real alpha, got opaque a={}",
                color.a
            );
        }

        #[test]
        fn insertion_bar_paints_opaque() {
            let overlay = DropOverlay {
                highlight: None,
                insertion_bar: Some(Rect::new(40.0, 0.0, 2.0, 20.0)),
                ghost_position: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &theme);

            assert_eq!(surface.fills.len(), 1, "expected exactly the bar fill");
            let (_, color) = surface.fills[0];
            assert_eq!(
                color, theme.accent_fg,
                "insertion bar is solid theme.accent_fg"
            );
        }

        #[test]
        fn insertion_bar_widens_to_min_thickness() {
            let overlay = DropOverlay {
                highlight: None,
                insertion_bar: Some(Rect::new(40.0, 0.0, 0.0, 20.0)),
                ghost_position: None,
            };
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &Theme::default());

            let (rect, _) = surface.fills[0];
            assert_eq!(rect.width, DropOverlay::MIN_BAR_THICKNESS);
        }

        #[test]
        fn zero_size_highlight_paints_nothing() {
            let overlay = DropOverlay {
                highlight: Some(Rect::new(10.0, 10.0, 0.0, 0.0)),
                insertion_bar: None,
                ghost_position: None,
            };
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &Theme::default());
            assert!(surface.fills.is_empty());
        }

        #[test]
        fn empty_overlay_paints_nothing() {
            let overlay = DropOverlay {
                highlight: None,
                insertion_bar: None,
                ghost_position: None,
            };
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &Theme::default());
            assert!(surface.fills.is_empty());
        }

        #[test]
        fn both_highlight_and_bar_paint_two_fills() {
            let overlay = DropOverlay {
                highlight: Some(Rect::new(0.0, 0.0, 20.0, 20.0)),
                insertion_bar: Some(Rect::new(40.0, 0.0, 2.0, 20.0)),
                ghost_position: None,
            };
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &Theme::default());
            assert_eq!(surface.fills.len(), 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(x: f32, y: f32, w: f32, h: f32, slots: &[(f32, f32)]) -> DropGroupRect {
        DropGroupRect {
            bounds: Rect::new(x, y, w, h),
            tab_slots: slots.to_vec(),
        }
    }

    #[test]
    fn center_drop() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = compute_drop_zone(50.0, 50.0, &groups, 20.0).unwrap();
        assert_eq!(zone.group_idx, 0);
        assert_eq!(zone.kind, DropZoneKind::Center);
    }

    #[test]
    fn left_edge_split() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = compute_drop_zone(5.0, 50.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::Split(DropEdge::Left));
    }

    #[test]
    fn right_edge_split() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = compute_drop_zone(95.0, 50.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::Split(DropEdge::Right));
    }

    #[test]
    fn top_edge_split() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        // tab_bar_height=20, content starts at y=20, edge_h = 60*0.2 = 12
        let zone = compute_drop_zone(50.0, 25.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::Split(DropEdge::Top));
    }

    #[test]
    fn bottom_edge_split() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = compute_drop_zone(50.0, 75.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::Split(DropEdge::Bottom));
    }

    #[test]
    fn tab_reorder_before_first() {
        let slots = vec![(10.0, 40.0), (40.0, 70.0)];
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &slots)];
        // Cursor at x=15, left of first tab's midpoint (25)
        let zone = compute_drop_zone(15.0, 5.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::TabReorder(0));
    }

    #[test]
    fn tab_reorder_between_tabs() {
        let slots = vec![(10.0, 40.0), (40.0, 70.0)];
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &slots)];
        // Cursor at x=35, right of first midpoint (25), left of second (55)
        let zone = compute_drop_zone(35.0, 5.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::TabReorder(1));
    }

    #[test]
    fn tab_reorder_after_last() {
        let slots = vec![(10.0, 40.0), (40.0, 70.0)];
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &slots)];
        // Cursor at x=60, right of second midpoint (55)
        let zone = compute_drop_zone(60.0, 5.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::TabReorder(2));
    }

    #[test]
    fn cursor_outside_all_groups() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        assert!(compute_drop_zone(200.0, 200.0, &groups, 20.0).is_none());
    }

    // ── #818: hit_test ───────────────────────────────────────────────────

    #[test]
    fn hit_test_returns_zone_for_center_drop() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        match drop_zone_hit_test(50.0, 50.0, &groups, 20.0) {
            DropZoneHit::Zone(zone) => {
                assert_eq!(zone.kind, DropZoneKind::Center);
                assert_eq!(zone.group_idx, 0);
            }
            DropZoneHit::Empty => panic!("expected a zone hit"),
        }
    }

    #[test]
    fn hit_test_outside_all_groups_is_empty() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        assert_eq!(
            drop_zone_hit_test(200.0, 200.0, &groups, 20.0),
            DropZoneHit::Empty
        );
    }

    #[test]
    fn multiple_groups_selects_correct() {
        let groups = vec![
            group(0.0, 0.0, 50.0, 80.0, &[]),
            group(50.0, 0.0, 50.0, 80.0, &[]),
        ];
        let zone = compute_drop_zone(75.0, 50.0, &groups, 20.0).unwrap();
        assert_eq!(zone.group_idx, 1);
    }

    #[test]
    fn overlay_center_covers_content_area() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = DropZone {
            kind: DropZoneKind::Center,
            group_idx: 0,
        };
        let ov = drop_zone_overlay(&zone, &groups, 50.0, 50.0, 20.0, 2.0, 10.0);
        assert_eq!(ov.highlight, Some(Rect::new(0.0, 20.0, 100.0, 60.0)));
        assert!(ov.insertion_bar.is_none());
    }

    #[test]
    fn overlay_split_left_covers_left_half() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = DropZone {
            kind: DropZoneKind::Split(DropEdge::Left),
            group_idx: 0,
        };
        let ov = drop_zone_overlay(&zone, &groups, 5.0, 50.0, 20.0, 2.0, 10.0);
        assert_eq!(ov.highlight, Some(Rect::new(0.0, 20.0, 50.0, 60.0)));
    }

    #[test]
    fn overlay_tab_reorder_insertion_bar() {
        let slots = vec![(10.0, 40.0), (40.0, 70.0)];
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &slots)];
        let zone = DropZone {
            kind: DropZoneKind::TabReorder(1),
            group_idx: 0,
        };
        let ov = drop_zone_overlay(&zone, &groups, 35.0, 5.0, 20.0, 2.0, 10.0);
        assert!(ov.highlight.is_none());
        let bar = ov.insertion_bar.unwrap();
        assert_eq!(bar.x, 39.0); // midpoint of prev_end(40) and next_start(40) = 40, minus half thickness
        assert_eq!(bar.width, 2.0);
        assert_eq!(bar.height, 20.0);
    }
}
