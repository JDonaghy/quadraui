//! macOS (Core Graphics) rasteriser for [`crate::SplitTree`].
//!
//! Port of [`crate::gtk::split_tree::draw_split_tree`]: paints only the
//! dividers as filled rectangles — leaf content is the app's
//! responsibility, painted into the rects [`SplitTreeLayout::leaves`]
//! returns. Divider thickness matches [`super::split`]'s 4 points, which
//! in turn matches GTK, so a `SplitTree` and a plain `Split` line up.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;

use crate::event::Rect as QRect;
use crate::primitives::split_tree::{SplitDirection, SplitTree, SplitTreeLayout, SplitTreeMeasure};
use crate::theme::Theme;
use crate::types::Color;

/// 4-point divider thickness, matching `super::split`'s `DIVIDER_PX` and
/// the GTK twin's `GTK_DIVIDER_PX`.
const MAC_DIVIDER_PX: f32 = 4.0;

/// Compute the macOS point-unit layout for a [`SplitTree`] without
/// painting. `x` / `y` are baked into the returned leaf and divider
/// geometry (absolute frame), matching [`super::split::mac_split_layout`]
/// and the GTK / TUI twins.
pub fn mac_split_tree_layout(tree: &SplitTree, x: f64, y: f64, w: f64, h: f64) -> SplitTreeLayout {
    let bounds = QRect::new(x as f32, y as f32, w as f32, h as f32);
    tree.layout(bounds, SplitTreeMeasure::new(MAC_DIVIDER_PX))
}

/// Draw a [`SplitTree`]'s dividers onto `ctx`. Returns the layout for
/// host click/drag dispatch. Leaf content is NOT painted.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of the
/// call (typical: the frame-scope pointer stashed on [`super::MacBackend`]).
/// Calling with a freed or null pointer is UB.
pub unsafe fn draw_split_tree(
    ctx: CGContextRef,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    tree: &SplitTree,
    theme: &Theme,
) -> SplitTreeLayout {
    let layout = mac_split_tree_layout(tree, x, y, w, h);

    for div in &layout.dividers {
        let (rx, ry, rw, rh) = match div.direction {
            SplitDirection::Horizontal => (
                div.position as f64,
                div.cross_start as f64,
                div.thickness as f64,
                div.cross_size as f64,
            ),
            SplitDirection::Vertical => (
                div.cross_start as f64,
                div.position as f64,
                div.cross_size as f64,
                div.thickness as f64,
            ),
        };
        fill_rect(ctx, rx, ry, rw, rh, theme.separator);
    }

    layout
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Point, Viewport};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 200;

    fn wid(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    fn two_pane() -> SplitTree {
        SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        )
    }

    fn nested() -> SplitTree {
        SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        )
    }

    /// Paint through the real `Backend::draw_split_tree` path.
    fn paint_via_backend(tree: &SplitTree, rect: QRect) -> (BitmapSurface, SplitTreeLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let captured = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *captured.borrow_mut() = Some(b.draw_split_tree(rect, tree));
        });
        backend.end_frame();
        (surface, captured.into_inner().expect("layout captured"))
    }

    #[test]
    fn divider_paints_separator_colour() {
        let (surface, layout) =
            paint_via_backend(&two_pane(), QRect::new(0.0, 0.0, W as f32, H as f32));
        let theme = Theme::default();
        assert_eq!(layout.dividers.len(), 1);
        let d = &layout.dividers[0];
        let px = (d.position + d.thickness / 2.0) as u32;
        let py = (d.cross_start + d.cross_size / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
            "divider band should carry the separator colour",
        );
    }

    #[test]
    fn leaf_areas_are_left_unpainted() {
        let (surface, _layout) =
            paint_via_backend(&two_pane(), QRect::new(0.0, 0.0, W as f32, H as f32));
        let (r, g, b, _) = surface.pixel(8, H / 2);
        assert_eq!(
            (r, g, b),
            (255, 255, 255),
            "split tree paints chrome only — leaf interiors stay untouched",
        );
    }

    #[test]
    fn nested_tree_paints_every_divider() {
        let (surface, layout) =
            paint_via_backend(&nested(), QRect::new(0.0, 0.0, W as f32, H as f32));
        let theme = Theme::default();
        assert_eq!(layout.dividers.len(), 2, "one divider per split node");
        for (i, d) in layout.dividers.iter().enumerate() {
            let (px, py) = match d.direction {
                SplitDirection::Horizontal => (
                    (d.position + d.thickness / 2.0) as u32,
                    (d.cross_start + d.cross_size / 2.0) as u32,
                ),
                SplitDirection::Vertical => (
                    (d.cross_start + d.cross_size / 2.0) as u32,
                    (d.position + d.thickness / 2.0) as u32,
                ),
            };
            let (r, g, b, _) = surface.pixel(px, py);
            assert_eq!(
                (r, g, b),
                (theme.separator.r, theme.separator.g, theme.separator.b),
                "divider {i} ({:?}) should be painted",
                d.direction,
            );
        }
    }

    /// Shared paint↔click round trip: click the exact pixel the
    /// rasteriser painted the divider at, and each leaf's centre, and
    /// prove `hit_test_divider` / `hit_test_leaf` resolve them.
    fn paint_click_round_trip_at(origin_x: f32, origin_y: f32) {
        let rect = QRect::new(origin_x, origin_y, W as f32 - origin_x, H as f32 - origin_y);
        let (surface, layout) = paint_via_backend(&two_pane(), rect);
        let theme = Theme::default();

        let d = &layout.dividers[0];
        let cx = d.position + d.thickness / 2.0;
        let cy = d.cross_start + d.cross_size / 2.0;

        // Paint side: the pixel is separator-coloured.
        let (r, g, b, _) = surface.pixel(cx as u32, cy as u32);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
            "divider pixel at origin ({origin_x}, {origin_y}) should be painted",
        );

        // Click side: the same point resolves to divider 0.
        assert_eq!(
            layout.hit_test_divider(Point::new(cx, cy), 2.0),
            Some(0),
            "divider hit-test must resolve the painted position at origin \
             ({origin_x}, {origin_y})",
        );

        // Both leaves resolve at their own centres.
        for (id, lr) in &layout.leaves {
            let hit =
                layout.hit_test_leaf(Point::new(lr.x + lr.width / 2.0, lr.y + lr.height / 2.0));
            assert_eq!(
                hit,
                Some(id),
                "leaf {id:?} should hit-test at its own centre (origin {origin_x}, {origin_y})",
            );
        }

        // And every leaf sits inside the requested bounds — the origin
        // was actually honoured rather than silently dropped.
        for (id, lr) in &layout.leaves {
            assert!(
                lr.x >= origin_x - 0.001 && lr.y >= origin_y - 0.001,
                "leaf {id:?} at ({}, {}) escaped the requested origin ({origin_x}, {origin_y})",
                lr.x,
                lr.y,
            );
        }
    }

    #[test]
    fn paint_click_round_trip() {
        paint_click_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (quadraui#494, LESSONS.md:159-181).
    #[test]
    fn paint_click_round_trip_at_nonzero_origin() {
        paint_click_round_trip_at(37.0, 21.0);
    }

    #[test]
    fn layout_twin_matches_the_painted_layout() {
        // `Backend::split_tree_layout` must be the no-paint twin of
        // `draw_split_tree` — same dividers, same leaves, same space.
        let rect = QRect::new(11.0, 7.0, 260.0, 150.0);
        let tree = nested();
        let (_surface, painted) = paint_via_backend(&tree, rect);

        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        let computed = backend.split_tree_layout(rect, &tree);

        assert_eq!(painted.bounds, computed.bounds);
        assert_eq!(painted.leaves, computed.leaves);
        assert_eq!(painted.dividers.len(), computed.dividers.len());
        for (a, b) in painted.dividers.iter().zip(computed.dividers.iter()) {
            assert_eq!(a.split_index, b.split_index);
            assert_eq!(a.direction, b.direction);
            assert!((a.position - b.position).abs() < 0.001);
            assert!((a.cross_start - b.cross_start).abs() < 0.001);
            assert!((a.cross_size - b.cross_size).abs() < 0.001);
            assert!((a.thickness - b.thickness).abs() < 0.001);
        }
    }

    #[test]
    fn divider_thickness_matches_the_plain_split_rasteriser() {
        let layout = mac_split_tree_layout(&two_pane(), 0.0, 0.0, 100.0, 50.0);
        assert_eq!(layout.dividers[0].thickness, MAC_DIVIDER_PX);
        // available = 96, 0.5 * 96 = 48 — identical to the GTK twin's
        // `layout_matches_split_layout_semantics`.
        assert!((layout.dividers[0].position - 48.0).abs() < 0.001);
    }
}
