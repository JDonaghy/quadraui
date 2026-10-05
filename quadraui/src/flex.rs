//! Optional flex/grid layout module, built on [`taffy`].
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
//! [`DataTable`](crate::primitives::data_table), [`SplitTree`](crate::SplitTree) and
//! friends keep computing their own internal row/column geometry the
//! way they always have. [`FlexLayout`] is for the *app-level*
//! arrangement an `AppLogic::render` does by hand today: "status bar on
//! top, two side-by-side panels below it, each with 8px padding".
//!
//! This also does not support content-based sizing. [`FlexLayout::compute`]
//! calls taffy's layout pass with no measure function, so a leaf styled
//! with [`min_content`], [`max_content`], [`fit_content`], or any other
//! size that depends on the leaf's own rendered content resolves that axis
//! to `0.0` rather than erroring. Only styles whose sizes are fully
//! determined by fixed lengths, percentages, or the parent's own flex/grid
//! distribution are supported today.
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

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Process-unique id minted once per [`FlexLayout`], so a [`NodeId`] it
/// hands out can never be confused with a same-numbered node from a
/// different `FlexLayout`.
static NEXT_TREE_ID: AtomicU64 = AtomicU64::new(0);

/// Opaque handle to a node inside a [`FlexLayout`] tree.
///
/// Returned by [`FlexLayout::add_leaf`]/[`FlexLayout::add_container`] and
/// passed back in to look a computed [`Rect`] up in [`FlexLayout::compute`]'s
/// result map, or to pass as a child of another container. Cheap to
/// copy; carries no reference into the tree, so it stays valid across
/// `&mut FlexLayout` calls (unlike holding a borrow would).
///
/// Carries its owning [`FlexLayout`]'s tree id alongside the underlying
/// `taffy::NodeId`: a bare `taffy::NodeId` is a slotmap key, unique only
/// within the tree that issued it, so two freshly-built trees hand out
/// identical values for the same insertion order. Without the tree id, a
/// `NodeId` from one `FlexLayout` could be mistaken for a node in a
/// different one once both trees have grown to the same size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(u64, taffy::NodeId);

/// Error from a [`FlexLayout`] operation.
#[derive(Debug)]
pub enum FlexLayoutError {
    /// A [`NodeId`] passed to a [`FlexLayout`] method belongs to a
    /// different `FlexLayout`'s tree.
    ForeignNode(NodeId),
    /// A [`NodeId`] was passed to [`FlexLayout::add_container`] as a child
    /// a second time — a node has exactly one parent, so reusing it as a
    /// child of another container would silently move it rather than
    /// giving it two.
    AlreadyParented(NodeId),
    /// The underlying `taffy` tree rejected the operation.
    Taffy(taffy::TaffyError),
}

impl fmt::Display for FlexLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForeignNode(node) => {
                write!(f, "{node:?} belongs to a different FlexLayout's tree")
            }
            Self::AlreadyParented(node) => {
                write!(f, "{node:?} is already a child of another container")
            }
            Self::Taffy(err) => fmt::Display::fmt(err, f),
        }
    }
}

impl std::error::Error for FlexLayoutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Taffy(err) => Some(err),
            Self::ForeignNode(_) | Self::AlreadyParented(_) => None,
        }
    }
}

impl From<taffy::TaffyError> for FlexLayoutError {
    fn from(err: taffy::TaffyError) -> Self {
        Self::Taffy(err)
    }
}

/// `Result` alias for [`FlexLayout`] operations.
///
/// Named `FlexResult` rather than `Result` so `use quadraui::flex::*`
/// alongside this module's style-helper glob doesn't shadow
/// `std::result::Result` for the caller.
pub type FlexResult<T> = std::result::Result<T, FlexLayoutError>;

/// A flex/grid layout tree, computing [`Rect`]s from a [`taffy::Style`]
/// tree.
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
    id: u64,
    tree: taffy::TaffyTree<()>,
    // Every child `NodeId` that has already been attached to a container,
    // so a second `add_container` call reusing the same child is rejected
    // instead of silently giving it a second parent (taffy only ever
    // records the most recent one).
    parented: HashSet<taffy::NodeId>,
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
            id: NEXT_TREE_ID.fetch_add(1, Ordering::Relaxed),
            tree: taffy::TaffyTree::new(),
            parented: HashSet::new(),
        }
    }

    /// Add a leaf node (no children) with the given style.
    pub fn add_leaf(&mut self, style: Style) -> FlexResult<NodeId> {
        let id = self.tree.new_leaf(style)?;
        Ok(NodeId(self.id, id))
    }

    /// Add a container node, laying out `children` per `style`'s own
    /// flex/grid rules.
    ///
    /// `children` must already have been returned by this same
    /// [`FlexLayout`] (an earlier `add_leaf`/`add_container` call) and
    /// must not already be a child of another container — both are
    /// logic errors, rejected here with a [`FlexLayoutError`] before
    /// they ever reach the underlying `taffy` tree.
    pub fn add_container(&mut self, style: Style, children: &[NodeId]) -> FlexResult<NodeId> {
        for child in children {
            if child.0 != self.id {
                return Err(FlexLayoutError::ForeignNode(*child));
            }
            if self.parented.contains(&child.1) {
                return Err(FlexLayoutError::AlreadyParented(*child));
            }
        }
        let kids: Vec<taffy::NodeId> = children.iter().map(|n| n.1).collect();
        let id = self.tree.new_with_children(style, &kids)?;
        for child in children {
            self.parented.insert(child.1);
        }
        Ok(NodeId(self.id, id))
    }

    /// Compute layout for the subtree rooted at `root`, constrained to
    /// `available` (both its size *and* its origin — the returned
    /// `Rect`s are offset by `available.x`/`available.y`, so passing the
    /// `Rect` of e.g. a panel body already placed elsewhere on screen
    /// yields ABSOLUTE, not panel-relative, coordinates).
    ///
    /// Returns every node in `root`'s subtree (including `root` itself)
    /// mapped to its computed `Rect`.
    pub fn compute(&mut self, root: NodeId, available: Rect) -> FlexResult<HashMap<NodeId, Rect>> {
        if root.0 != self.id {
            return Err(FlexLayoutError::ForeignNode(root));
        }
        let available_space = taffy::Size {
            width: taffy::AvailableSpace::Definite(available.width),
            height: taffy::AvailableSpace::Definite(available.height),
        };
        self.tree.compute_layout(root.1, available_space)?;

        let mut out = HashMap::new();
        self.collect(root, available.x, available.y, &mut out)?;
        Ok(out)
    }

    /// Converts every node's taffy-relative `location` (always relative
    /// to its own parent) into an ABSOLUTE `Rect`, by threading the
    /// running parent offset down the tree.
    ///
    /// Iterative (an explicit work-stack, not recursion): the tree this
    /// walks is rebuilt from an app's model every frame, so its depth is
    /// data-driven rather than bounded by this module — a deep model
    /// should be slow, not a stack overflow.
    fn collect(
        &self,
        root: NodeId,
        origin_x: f32,
        origin_y: f32,
        out: &mut HashMap<NodeId, Rect>,
    ) -> FlexResult<()> {
        let mut stack = vec![(root, origin_x, origin_y)];
        while let Some((node, parent_x, parent_y)) = stack.pop() {
            let layout = self.tree.layout(node.1)?;
            let x = parent_x + layout.location.x;
            let y = parent_y + layout.location.y;
            out.insert(node, Rect::new(x, y, layout.size.width, layout.size.height));

            for child in self.tree.children(node.1)? {
                stack.push((NodeId(self.id, child), x, y));
            }
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
    fn nested_three_level_tree_threads_absolute_offsets_correctly() {
        // Root -> child alone can't distinguish a correctly-threaded
        // offset from one applied twice (or not at all) — that bug class
        // only shows up once a node's parent is itself not the root, so
        // this nests leaf -> inner -> outer and gives `compute` a
        // non-zero `available` origin on top.
        let mut flex = FlexLayout::new();
        let spacer = flex
            .add_leaf(Style {
                size: Size {
                    width: percent(1.0),
                    height: length(5.0),
                },
                ..Default::default()
            })
            .unwrap();
        let leaf_one = flex
            .add_leaf(Style {
                size: Size {
                    width: percent(1.0),
                    height: length(10.0),
                },
                ..Default::default()
            })
            .unwrap();
        let leaf_two = flex
            .add_leaf(Style {
                size: Size {
                    width: percent(1.0),
                    height: length(10.0),
                },
                ..Default::default()
            })
            .unwrap();
        let inner = flex
            .add_container(
                Style {
                    flex_direction: FlexDirection::Column,
                    size: Size {
                        width: percent(1.0),
                        height: length(20.0),
                    },
                    ..Default::default()
                },
                &[leaf_one, leaf_two],
            )
            .unwrap();
        let outer = flex
            .add_container(
                Style {
                    flex_direction: FlexDirection::Column,
                    size: Size {
                        width: percent(1.0),
                        height: length(25.0),
                    },
                    ..Default::default()
                },
                &[spacer, inner],
            )
            .unwrap();

        let rects = flex
            .compute(outer, Rect::new(5.0, 7.0, 30.0, 25.0))
            .unwrap();
        assert_eq!(rects[&spacer], Rect::new(5.0, 7.0, 30.0, 5.0));
        assert_eq!(rects[&inner], Rect::new(5.0, 12.0, 30.0, 20.0));
        assert_eq!(rects[&leaf_one], Rect::new(5.0, 12.0, 30.0, 10.0));
        assert_eq!(rects[&leaf_two], Rect::new(5.0, 22.0, 30.0, 10.0));
    }

    #[test]
    fn grid_two_by_two_divides_both_axes() {
        let mut flex = FlexLayout::new();
        let cells: Vec<NodeId> = (0..4)
            .map(|_| flex.add_leaf(Style::default()).unwrap())
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
        // Give `b` a node of its own first: `b`'s first-inserted node
        // reuses the same bare `taffy::NodeId` slotmap key as `a`'s, so
        // this is the case a check based on the raw `taffy::NodeId` alone
        // (with no tree id attached) cannot distinguish from a node `b`
        // actually owns. `leaf_a` still belongs to `a`'s tree, not `b`'s,
        // so both calls below must reject it cleanly.
        let _ = b.add_leaf(Style::default()).unwrap();
        assert!(b.add_container(Style::default(), &[leaf_a]).is_err());
        assert!(b.compute(leaf_a, Rect::new(0.0, 0.0, 10.0, 10.0)).is_err());

        // `a`'s own tree is unaffected.
        let rects = a.compute(root_a, Rect::new(0.0, 0.0, 10.0, 10.0)).unwrap();
        assert!(rects.contains_key(&root_a));
    }

    #[test]
    fn reparenting_a_node_errors_not_silently_moves_it() {
        let mut flex = FlexLayout::new();
        let leaf = flex.add_leaf(Style::default()).unwrap();
        let _first_parent = flex.add_container(Style::default(), &[leaf]).unwrap();

        // `leaf` already has a parent; attaching it to a second container
        // must error rather than silently moving it there.
        assert!(matches!(
            flex.add_container(Style::default(), &[leaf]),
            Err(FlexLayoutError::AlreadyParented(node)) if node == leaf
        ));
    }
}
