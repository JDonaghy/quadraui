//! Optional flex/grid layout module, built on [`taffy`] (quadraui#1103).
//!
//! Every consumer of this crate ends up hand-computing [`Rect`]s for
//! anything beyond a fixed split — stacking a toolbar over a body over a
//! status bar, wrapping a row of buttons, sizing a grid of cards. This
//! module is an *optional* `layout` wrapper over [`taffy`] (the same
//! flex/grid engine behind GPUI, Dioxus and Bevy) that does that
//! arithmetic once, generically, and hands back plain [`Rect`]s in the
//! same ABSOLUTE (target-surface) coordinate convention the rest of the
//! crate uses (see `crate::layout`'s module doc for that convention).
//!
//! This module is deliberately **unit-agnostic**: [`Rect`] is `f32`
//! everywhere in quadraui, and so is every taffy coordinate — a TUI app
//! can hand this module a viewport measured in terminal cells and get
//! back cell-sized `Rect`s; a GTK/macOS/Win app hands it logical pixels
//! and gets pixel `Rect`s back. Nothing here assumes a particular
//! backend, and nothing in `Backend` needs to change for it to work —
//! this module only ever *produces* `Rect`s for a caller to pass into
//! the same `draw_*`/`layout` calls it already makes.
//!
//! ## What this is not
//!
//! This is not a replacement for any primitive's own `layout()` —
//! [`DataTable`](crate::primitives::data_table), [`SplitTree`] and
//! friends keep computing their own internal row/column geometry the
//! way they always have. [`FlexLayout`] is for the *app-level*
//! arrangement an `AppLogic::render` does by hand today: "status bar on
//! top, two side-by-side panels below it, each with 8px padding".
//!
//! ## Usage
//!
//! ```
//! use quadraui::event::Rect;
//! use quadraui::flex::{percent, FlexDirection, FlexLayout, Size, Style};
//!
//! let mut flex = FlexLayout::new();
//! let left = flex
//!     .add_leaf(Style {
//!         size: Size { width: percent(0.5), height: percent(1.0) },
//!         ..Default::default()
//!     })
//!     .unwrap();
//! let right = flex
//!     .add_leaf(Style {
//!         size: Size { width: percent(0.5), height: percent(1.0) },
//!         ..Default::default()
//!     })
//!     .unwrap();
//! let root = flex
//!     .add_container(
//!         Style {
//!             flex_direction: FlexDirection::Row,
//!             size: Size { width: percent(1.0), height: percent(1.0) },
//!             ..Default::default()
//!         },
//!         &[left, right],
//!     )
//!     .unwrap();
//!
//! let rects = flex.compute(root, Rect::new(0.0, 0.0, 100.0, 40.0)).unwrap();
//! assert_eq!(rects[&left], Rect::new(0.0, 0.0, 50.0, 40.0));
//! assert_eq!(rects[&right], Rect::new(50.0, 0.0, 50.0, 40.0));
//! ```

use std::collections::HashMap;
use std::fmt;

use crate::event::Rect;

// ── Re-exported taffy style vocabulary ──────────────────────────────────
//
// Re-exported rather than wrapped: `Style` is a plain-data struct (no
// closures, nothing taffy-internal leaks through it), and wrapping each
// field in a quadraui-specific enum would just be relabelling taffy's
// own CSS-flex/grid vocabulary for no behavioural gain. `Rect<T>` is
// re-exported as [`Edges`] (not `Rect`) so it never collides with
// `crate::event::Rect`, the concrete `f32` pixel/cell rectangle this
// module's own [`FlexLayout::compute`] returns.
pub use taffy::geometry::{Line, Rect as Edges, Size};
pub use taffy::style::{
    AlignContent, AlignItems, AlignSelf, Dimension, Display, FlexDirection, FlexWrap, GridAutoFlow,
    GridPlacement, GridTemplateComponent, JustifyContent, JustifyItems, JustifySelf,
    LengthPercentage, LengthPercentageAuto, Overflow, Position, Style, TrackSizingFunction,
};
pub use taffy::style_helpers::{
    auto, evenly_sized_tracks, fit_content, fr, length, max_content, min_content, minmax, percent,
    repeat, span, zero,
};

/// Opaque handle to a node inside a [`FlexLayout`] tree.
///
/// Returned by [`FlexLayout::add_leaf`]/[`FlexLayout::add_container`] and
/// used to look a computed [`Rect`] up in [`FlexLayout::compute`]'s
/// result map, or to pass as a child of another container. Cheap to
/// copy; carries no reference into the tree, so it stays valid across
/// `&mut FlexLayout` calls (unlike holding a borrow would).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(taffy::NodeId);

/// Error from a [`FlexLayout`] operation.
///
/// Thin wrapper over [`taffy::TaffyError`] — kept as quadraui's own type
/// (rather than re-exporting taffy's directly) so this module's public
/// API never forces a caller to add `taffy` as its own direct
/// dependency just to name the error type; see this crate's
/// `[dependencies]` comment on the `layout` feature for why `taffy`
/// itself stays an optional, feature-gated dependency.
#[derive(Debug)]
pub struct FlexLayoutError(taffy::TaffyError);

impl fmt::Display for FlexLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for FlexLayoutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<taffy::TaffyError> for FlexLayoutError {
    fn from(err: taffy::TaffyError) -> Self {
        Self(err)
    }
}

/// `Result` alias for [`FlexLayout`] operations.
pub type Result<T> = std::result::Result<T, FlexLayoutError>;

/// A flex/grid layout tree, computing [`Rect`]s from a [`taffy::Style`]
/// tree (quadraui#1103).
///
/// Build a tree with [`add_leaf`](Self::add_leaf) (no children — a
/// button, a label, anything whose own size and position is fully
/// determined by its parent's flex/grid rules) and
/// [`add_container`](Self::add_container) (lays its children out per
/// its own `Style`), then call [`compute`](Self::compute) with the
/// outer bounds to get back a [`NodeId`] → [`Rect`] map in ABSOLUTE
/// coordinates — ready to hand straight to any primitive's own
/// `layout(bounds, ...)` or `draw_*` call.
///
/// One `FlexLayout` is a disposable, per-frame computation, matching
/// this crate's immediate-mode model: build the tree, call `compute`
/// once, read the `Rect`s, drop it. It is not meant to be retained
/// across frames (unlike a persistent GUI toolkit's layout tree) —
/// rebuilding a small tree of `Style`s each frame is the same cost
/// immediate-mode UI already pays for everything else it describes as
/// plain data.
pub struct FlexLayout {
    tree: taffy::TaffyTree<()>,
    // `taffy::TaffyTree`'s own `TaffyResult`-returning accessors
    // (`style`/`layout`/`parent`/`new_with_children`, ...) index their
    // backing slotmap directly and *panic* on a `NodeId` from a
    // different `TaffyTree`, despite the `Result` in their signature —
    // see quadraui#1103's PR for the upstream behaviour this was
    // checked against. Tracking every id this `FlexLayout` has actually
    // handed out lets `add_container`/`compute` reject a foreign
    // `NodeId` with a clean [`FlexLayoutError`] *before* it reaches
    // `taffy`, which is the "not a panic" guarantee this module's docs
    // promise.
    known: std::collections::HashSet<taffy::NodeId>,
}

impl Default for FlexLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl FlexLayout {
    /// Create an empty layout tree.
    pub fn new() -> Self {
        Self {
            tree: taffy::TaffyTree::new(),
            known: std::collections::HashSet::new(),
        }
    }

    /// Add a leaf node (no children) with the given style.
    pub fn add_leaf(&mut self, style: Style) -> Result<NodeId> {
        let id = self.tree.new_leaf(style)?;
        self.known.insert(id);
        Ok(NodeId(id))
    }

    /// Add a container node, laying out `children` per `style`'s own
    /// flex/grid rules.
    ///
    /// `children` must already have been returned by this same
    /// [`FlexLayout`] (an earlier `add_leaf`/`add_container` call) —
    /// mixing `NodeId`s from two different `FlexLayout`s is a logic
    /// error, rejected here with a [`FlexLayoutError`] before it ever
    /// reaches the underlying `taffy` tree.
    pub fn add_container(&mut self, style: Style, children: &[NodeId]) -> Result<NodeId> {
        for child in children {
            if !self.known.contains(&child.0) {
                return Err(taffy::TaffyError::InvalidChildNode(child.0).into());
            }
        }
        let kids: Vec<taffy::NodeId> = children.iter().map(|n| n.0).collect();
        let id = self.tree.new_with_children(style, &kids)?;
        self.known.insert(id);
        Ok(NodeId(id))
    }

    /// Compute layout for the subtree rooted at `root`, constrained to
    /// `available` (both its size *and* its origin — the returned
    /// `Rect`s are offset by `available.x`/`available.y`, so passing the
    /// `Rect` of e.g. a panel body already placed elsewhere on screen
    /// yields ABSOLUTE, not panel-relative, coordinates).
    ///
    /// Returns every node in `root`'s subtree (including `root` itself)
    /// mapped to its computed `Rect`.
    pub fn compute(&mut self, root: NodeId, available: Rect) -> Result<HashMap<NodeId, Rect>> {
        if !self.known.contains(&root.0) {
            return Err(taffy::TaffyError::InvalidInputNode(root.0).into());
        }
        let available_space = taffy::Size {
            width: taffy::AvailableSpace::Definite(available.width),
            height: taffy::AvailableSpace::Definite(available.height),
        };
        self.tree.compute_layout(root.0, available_space)?;

        let mut out = HashMap::new();
        self.collect(root, available.x, available.y, &mut out)?;
        Ok(out)
    }

    /// Recursively convert each node's taffy-relative `location` (always
    /// relative to its own parent) into an ABSOLUTE `Rect`, by threading
    /// the running parent offset down the tree.
    fn collect(
        &self,
        node: NodeId,
        parent_x: f32,
        parent_y: f32,
        out: &mut HashMap<NodeId, Rect>,
    ) -> Result<()> {
        let layout = self.tree.layout(node.0)?;
        let x = parent_x + layout.location.x;
        let y = parent_y + layout.location.y;
        out.insert(node, Rect::new(x, y, layout.size.width, layout.size.height));

        for child in self.tree.children(node.0)? {
            self.collect(NodeId(child), x, y, out)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_of_two_equal_leaves_splits_available_width() {
        let mut flex = FlexLayout::new();
        let left = flex
            .add_leaf(Style {
                size: Size {
                    width: percent(0.5),
                    height: percent(1.0),
                },
                ..Default::default()
            })
            .unwrap();
        let right = flex
            .add_leaf(Style {
                size: Size {
                    width: percent(0.5),
                    height: percent(1.0),
                },
                ..Default::default()
            })
            .unwrap();
        let root = flex
            .add_container(
                Style {
                    flex_direction: FlexDirection::Row,
                    size: Size {
                        width: percent(1.0),
                        height: percent(1.0),
                    },
                    ..Default::default()
                },
                &[left, right],
            )
            .unwrap();

        let rects = flex
            .compute(root, Rect::new(0.0, 0.0, 100.0, 40.0))
            .unwrap();
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[&left], Rect::new(0.0, 0.0, 50.0, 40.0));
        assert_eq!(rects[&right], Rect::new(50.0, 0.0, 50.0, 40.0));
        assert_eq!(rects[&root], Rect::new(0.0, 0.0, 100.0, 40.0));
    }

    #[test]
    fn column_stack_offsets_are_absolute_not_panel_relative() {
        // Offsetting `available`'s origin (as if this were a panel body
        // already placed elsewhere on screen) must thread through to
        // every child `Rect` — the whole point of returning ABSOLUTE
        // coordinates, matching `crate::layout`'s convention.
        let mut flex = FlexLayout::new();
        let top = flex
            .add_leaf(Style {
                size: Size {
                    width: percent(1.0),
                    height: length(10.0),
                },
                ..Default::default()
            })
            .unwrap();
        let bottom = flex
            .add_leaf(Style {
                size: Size {
                    width: percent(1.0),
                    height: length(10.0),
                },
                ..Default::default()
            })
            .unwrap();
        let root = flex
            .add_container(
                Style {
                    flex_direction: FlexDirection::Column,
                    size: Size {
                        width: percent(1.0),
                        height: length(20.0),
                    },
                    ..Default::default()
                },
                &[top, bottom],
            )
            .unwrap();

        let rects = flex.compute(root, Rect::new(5.0, 7.0, 30.0, 20.0)).unwrap();
        assert_eq!(rects[&top], Rect::new(5.0, 7.0, 30.0, 10.0));
        assert_eq!(rects[&bottom], Rect::new(5.0, 17.0, 30.0, 10.0));
    }

    #[test]
    fn grid_two_by_two_divides_both_axes() {
        let mut flex = FlexLayout::new();
        let cells: Vec<NodeId> = (0..4)
            .map(|_| {
                flex.add_leaf(Style {
                    ..Default::default()
                })
                .unwrap()
            })
            .collect();
        let root = flex
            .add_container(
                Style {
                    display: Display::Grid,
                    size: Size {
                        width: length(100.0),
                        height: length(100.0),
                    },
                    grid_template_columns: vec![fr(1.0), fr(1.0)],
                    grid_template_rows: vec![fr(1.0), fr(1.0)],
                    ..Default::default()
                },
                &cells,
            )
            .unwrap();

        let rects = flex
            .compute(root, Rect::new(0.0, 0.0, 100.0, 100.0))
            .unwrap();
        // Every cell should be 50x50, and the four cells should tile the
        // 100x100 grid without overlap: four distinct origins.
        let mut origins: Vec<(f32, f32)> = cells
            .iter()
            .map(|c| {
                let r = rects[c];
                assert_eq!((r.width, r.height), (50.0, 50.0));
                (r.x, r.y)
            })
            .collect();
        origins.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(
            origins,
            vec![(0.0, 0.0), (0.0, 50.0), (50.0, 0.0), (50.0, 50.0)]
        );
    }

    #[test]
    fn unknown_node_id_from_another_tree_errors_not_panics() {
        let mut a = FlexLayout::new();
        let leaf_a = a.add_leaf(Style::default()).unwrap();
        let root_a = a.add_container(Style::default(), &[leaf_a]).unwrap();

        let mut b = FlexLayout::new();
        // `leaf_a` belongs to `a`'s tree, not `b`'s: both `add_container`
        // and `compute` must reject it cleanly rather than panicking
        // (see `FlexLayout::known`'s doc for why `taffy` itself can't be
        // trusted to do this).
        assert!(b.add_container(Style::default(), &[leaf_a]).is_err());
        assert!(b.compute(leaf_a, Rect::new(0.0, 0.0, 10.0, 10.0)).is_err());

        // `a`'s own tree is unaffected.
        let rects = a.compute(root_a, Rect::new(0.0, 0.0, 10.0, 10.0)).unwrap();
        assert!(rects.contains_key(&root_a));
    }
}
