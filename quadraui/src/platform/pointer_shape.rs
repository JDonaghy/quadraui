// ─────────────────────────────────────────────────────────────────────
// PointerShape enum-walk scaffold
// ─────────────────────────────────────────────────────────────────────

/// Every [`crate::backend::ResizeEdge`] variant, in a stable order.
/// Backends building a `ResizeEdge` → native-surface-edge or →
/// cursor-name table (and the tests that exercise it exhaustively)
/// iterate this instead of hand-listing all 8 arms — see e.g.
/// `gtk::backend::resize_edge_to_surface_edge_maps_every_variant`.
/// `#[cfg(test)]`: every consumer today is a mapping-table *test* (a
/// production `set_cursor`/`begin_window_resize` match handles one
/// `ResizeEdge` at a time and never needs the full list) — see the
/// module-doc note on why every item here is gated.
#[cfg(all(
    test,
    any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
pub(crate) const ALL_RESIZE_EDGES: [crate::backend::ResizeEdge; 8] = [
    crate::backend::ResizeEdge::North,
    crate::backend::ResizeEdge::South,
    crate::backend::ResizeEdge::East,
    crate::backend::ResizeEdge::West,
    crate::backend::ResizeEdge::NorthEast,
    crate::backend::ResizeEdge::NorthWest,
    crate::backend::ResizeEdge::SouthEast,
    crate::backend::ResizeEdge::SouthWest,
];

/// Every [`crate::backend::PointerShape`] variant, in a stable order —
/// [`crate::backend::PointerShape::Default`], one
/// [`crate::backend::PointerShape::Resize`] per [`ALL_RESIZE_EDGES`] edge
/// (8), and one of each remaining non-exhaustive variant (`Text`,
/// `Pointer`, `Grab`, `Grabbing`, `Crosshair`, `Wait`, `NotAllowed`,
/// `ColResize`, `RowResize`) — 18 total. Each backend still owns its own
/// `PointerShape` → native cursor table (cursor names/objects are
/// inherently native — GTK's are CSS cursor-name strings, AppKit's are
/// `NSCursor` objects, and AppKit in particular has no public diagonal-
/// resize cursor, so backends may legitimately map more than one
/// `PointerShape` to the same native cursor) — this is the enum-walk
/// *scaffold* that was missing: a canonical list backends' own mapping
/// tests iterate against instead of hand-duplicating the variant list
/// per backend.
#[cfg(all(
    test,
    any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
pub(crate) fn all_pointer_shapes() -> [crate::backend::PointerShape; 18] {
    use crate::backend::PointerShape;
    [
        PointerShape::Default,
        PointerShape::Resize(ALL_RESIZE_EDGES[0]),
        PointerShape::Resize(ALL_RESIZE_EDGES[1]),
        PointerShape::Resize(ALL_RESIZE_EDGES[2]),
        PointerShape::Resize(ALL_RESIZE_EDGES[3]),
        PointerShape::Resize(ALL_RESIZE_EDGES[4]),
        PointerShape::Resize(ALL_RESIZE_EDGES[5]),
        PointerShape::Resize(ALL_RESIZE_EDGES[6]),
        PointerShape::Resize(ALL_RESIZE_EDGES[7]),
        PointerShape::Text,
        PointerShape::Pointer,
        PointerShape::Grab,
        PointerShape::Grabbing,
        PointerShape::Crosshair,
        PointerShape::Wait,
        PointerShape::NotAllowed,
        PointerShape::ColResize,
        PointerShape::RowResize,
    ]
}

#[cfg(all(
    test,
    any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
mod pointer_shape_scaffold_tests {
    use super::*;
    use crate::backend::{PointerShape, ResizeEdge};

    #[test]
    fn all_resize_edges_has_every_variant_exactly_once() {
        let mut seen: Vec<ResizeEdge> = ALL_RESIZE_EDGES.to_vec();
        seen.sort_by_key(|e| format!("{e:?}"));
        seen.dedup();
        assert_eq!(
            seen.len(),
            ALL_RESIZE_EDGES.len(),
            "ALL_RESIZE_EDGES must list every ResizeEdge variant exactly once"
        );
    }

    #[test]
    fn all_pointer_shapes_is_default_plus_one_resize_per_edge_plus_rest() {
        const REST: [PointerShape; 9] = [
            PointerShape::Text,
            PointerShape::Pointer,
            PointerShape::Grab,
            PointerShape::Grabbing,
            PointerShape::Crosshair,
            PointerShape::Wait,
            PointerShape::NotAllowed,
            PointerShape::ColResize,
            PointerShape::RowResize,
        ];
        let shapes = all_pointer_shapes();
        assert_eq!(shapes.len(), ALL_RESIZE_EDGES.len() + 1 + REST.len());
        assert_eq!(shapes[0], PointerShape::Default);
        for (shape, edge) in shapes[1..9].iter().zip(ALL_RESIZE_EDGES.iter()) {
            assert_eq!(*shape, PointerShape::Resize(*edge));
        }
        assert_eq!(&shapes[9..], &REST);
    }
}
