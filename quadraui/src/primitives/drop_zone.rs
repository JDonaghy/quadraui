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
