//! GTK rasteriser for [`crate::SplitTree`].
//!
//! Paints only the dividers as filled rectangles — leaf content is the
//! app's responsibility, painted into the rects `SplitTreeLayout::leaves`
//! returns. Mirrors [`super::split::draw_split`]'s divider chrome and
//! divider thickness (4px).

use gtk4::cairo::Context;

use super::set_source;
use crate::event::Rect;
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout, SplitTreeMeasure};
use crate::theme::Theme;

const GTK_DIVIDER_PX: f32 = 4.0;

/// Compute the GTK pixel-unit layout for a [`SplitTree`] without
/// painting.
pub fn gtk_split_tree_layout(tree: &SplitTree, x: f64, y: f64, w: f64, h: f64) -> SplitTreeLayout {
    let bounds = Rect::new(x as f32, y as f32, w as f32, h as f32);
    tree.layout(bounds, SplitTreeMeasure::new(GTK_DIVIDER_PX))
}

/// Draw a [`SplitTree`]'s dividers onto `cr`. Returns the layout for
/// host click/drag dispatch. Leaf content is NOT painted.
#[allow(clippy::too_many_arguments)]
pub fn draw_split_tree(
    cr: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    tree: &SplitTree,
    theme: &Theme,
) -> SplitTreeLayout {
    let layout = gtk_split_tree_layout(tree, x, y, w, h);

    set_source(cr, theme.separator);
    for div in &layout.dividers {
        let (rx, ry, rw, rh) = match div.direction {
            crate::primitives::split_tree::SplitDirection::Horizontal => (
                div.position as f64,
                div.cross_start as f64,
                div.thickness as f64,
                div.cross_size as f64,
            ),
            crate::primitives::split_tree::SplitDirection::Vertical => (
                div.cross_start as f64,
                div.position as f64,
                div.cross_size as f64,
                div.thickness as f64,
            ),
        };
        cr.rectangle(rx, ry, rw, rh);
    }
    cr.fill().ok();

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split_tree::SplitDirection;
    use crate::types::WidgetId;

    fn wid(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    #[test]
    fn layout_matches_split_layout_semantics() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        let layout = gtk_split_tree_layout(&tree, 0.0, 0.0, 100.0, 50.0);
        assert_eq!(layout.leaves.len(), 2);
        assert_eq!(layout.dividers.len(), 1);
        assert_eq!(layout.dividers[0].thickness, GTK_DIVIDER_PX);
        // available = 96, 0.5*96 = 48.
        assert!((layout.dividers[0].position - 48.0).abs() < 0.001);
    }

    #[test]
    fn nested_tree_produces_all_dividers() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        let layout = gtk_split_tree_layout(&tree, 0.0, 0.0, 400.0, 300.0);
        assert_eq!(layout.leaves.len(), 3);
        assert_eq!(layout.dividers.len(), 2);
        assert_eq!(layout.dividers[0].split_index, 0);
        assert_eq!(layout.dividers[1].split_index, 1);
    }
}
