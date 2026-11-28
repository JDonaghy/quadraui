//! Hit testing against a frame's painted primitives: [`PaintedHit`] records
//! what was painted where, [`hit_test_painted`] turns a pointer position
//! into the id and fine-grained [`HitDetail`] it landed on, and
//! [`event_position`]/[`set_event_widget`] read and write the one `widget`
//! field a [`quadraui::UiEvent`] this module enriches carries.

use std::collections::HashMap;
use std::rc::Rc;

use quadraui::{
    Point, PressZone, Rect, ScrollAxis, Scrollbar, SplitHit, SplitLayout, SplitTreeLayout,
    UiEvent, WidgetId,
};

use quadraui::primitive::DynWidget;

use crate::wire::DragRole;

/// One primitive this app painted during the current frame: its id and the
/// hit-testable layout its paint resolved for `area`. Recorded into the binding's own hit-map cache by `draw_bands` /
/// `paint_layout` / `paint_leaf`, in paint order, and consulted by
/// [`hit_test_painted`] to translate an incoming pointer event's screen
/// position into the primitive (and sub-region) under it.
pub enum PaintedHit {
    /// `Split`'s layout is already in the ABSOLUTE frame, and its own
    /// `WidgetId` only matters once a hit is found — carried by
    /// `SplitHit`'s own variants, not stored here.
    Split { layout: SplitLayout },
    /// A [`quadraui::SplitTree`] node's layout, in the ABSOLUTE frame
    /// (matching [`Self::Split`]). `id` is the tree's own host-supplied id
    /// — unlike `SplitHit`, [`SplitTreeLayout::hit_test_divider`] only
    /// resolves to a `split_index`, not an id, so it's carried alongside
    /// the layout here instead. Divider-only: a hit anywhere else in the
    /// tree's bounds (a leaf's content, or the gap between leaves) is left
    /// to whatever [`crate::layout::LayoutNode::SplitTree`]'s own
    /// per-leaf content paints underneath, which `crate::render::paint_layout`
    /// pushes AFTER this entry so it wins the reverse-paint-order check
    /// first.
    SplitTree {
        id: WidgetId,
        layout: SplitTreeLayout,
    },
    /// One [`crate::layout::LeafDescriptor::Widget`], painted by
    /// [`crate::render::paint_leaf`]'s `Widget` arm. `area` is the
    /// ABSOLUTE rect it was painted into, hit-tested as a whole
    /// ([`HitDetail::WidgetBody`] on any `area.contains(point)`): the
    /// widget resolves its own sub-regions in [`DynWidget::handle`].
    /// `widget` is an [`Rc`] clone of the leaf's own descriptor, kept alive
    /// past this frame's render so a later event can resolve
    /// `DynWidget::handle` against it; its layout is recomputed against
    /// `area` at event time, which reproduces exactly what
    /// [`quadraui::DrawsPrimitives::draw_dyn`] painted this frame.
    Widget {
        id: WidgetId,
        area: Rect,
        widget: Rc<dyn DynWidget>,
    },
    /// A [`crate::layout::LayoutNode::Scroll`]'s own viewport, in the
    /// ABSOLUTE frame (matching [`Self::Split`]) — `viewport` already
    /// excludes the scrollbar gutter `crate::render::paint_layout`'s
    /// `LayoutNode::Scroll` arm reserves. Pushed BEFORE that arm recurses
    /// into its (culled, offset) content, same ordering as
    /// [`Self::Split`]'s own divider hit — a descendant content hit
    /// (pushed after, so checked first in reverse order) wins over this
    /// one. Content scrolled out of view never gets its own
    /// [`PaintedHit`] at all (culled before painting), so there is nothing
    /// for this entry to mask; it exists so a click or wheel landing on
    /// empty space inside the viewport (nothing painted under the pointer)
    /// still resolves to the `Scroll` itself, via [`HitDetail::ScrollBody`].
    /// `axis`/`content_len`/`offset` are the same values
    /// [`crate::render::paint_scroll`] fed its own [`quadraui::ScrollView`]
    /// this frame — read back by the engine's wheel/page-key/drag handling
    /// to resolve a new offset without re-measuring the content. `scrollbar`
    /// is `None` when the content fits without scrolling; its `track` is
    /// an absolute rect, outside `viewport` (reserved alongside it, not
    /// inside it), and is narrowed by [`clip_hits_to_viewport`] — a
    /// `Scroll` nested inside another `Scroll`'s viewport needs its own
    /// track clipped to the OUTER viewport too. `children` is one [`ScrollChildHit`] per direct
    /// [`crate::layout::LayoutNode::Leaf`] child when this scroll's
    /// content is a `Column`/`Row` matching its own axis — read by the
    /// engine to scroll a child that just received focus fully into view.
    /// Empty for `Atomic` content or a non-`Leaf` direct child (a nested
    /// `Row`/`Column`/`Split`/…), where that scroll-into-view isn't
    /// implemented yet.
    Scroll {
        id: WidgetId,
        viewport: Rect,
        axis: ScrollAxis,
        content_len: f32,
        offset: f32,
        scrollbar: Option<Scrollbar>,
        children: Vec<ScrollChildHit>,
    },
}

/// One [`PaintedHit::Scroll::children`] entry: `id` is the direct
/// [`crate::layout::LayoutNode::Leaf`] child's own id; `start`/`len` are
/// its main-axis span within the scroll's content, content-relative — NOT
/// yet offset by [`PaintedHit::Scroll::offset`], and not clamped to
/// `viewport` the way the child's own painted rect may be, since the
/// engine's scroll-into-view math needs the un-clamped span. Mirrors
/// [`crate::render::ScrollChildSlot`]'s own `start`/`len` fields, which
/// this struct is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollChildHit {
    pub id: WidgetId,
    pub start: f32,
    pub len: f32,
}

/// Intersects `hits[start..]` — one [`crate::render::paint_scroll`] call's
/// own descendant content, painted inside `viewport` between its
/// `Backend::push_clip(viewport)`/`pop_clip` pair — with `viewport`
/// itself, dropping an entry whose bounds don't overlap it at all.
///
/// Without this, a descendant's own recorded bounds can reach past
/// `viewport` on a backend whose `Rect` carries a negative-origin main
/// axis correctly (every non-cell-grid one — see
/// [`crate::render::scroll_content_slots`]'s own `cell_grid` doc, which
/// already clips a cell-grid backend's bounds before they're ever
/// painted, making this pass a no-op there), or on any backend whose
/// `ScrollSlots::Atomic` content is sized to the scroll's full
/// `content_len` rather than clamped to one viewport's worth (the common
/// case — see `scroll_content_slots`'s own doc). Checked in reverse paint
/// order by [`hit_test_painted`], a hit whose bounds leak past its own
/// scroll's viewport can otherwise mask a sibling painted earlier at that
/// same screen position (e.g. a button above a scroll whose own
/// oversized content reaches back up over it) — this keeps every
/// descendant's bounds inside the viewport it was actually painted into,
/// so a point outside it never resolves to that scroll's content and a
/// sibling painted before the scroll keeps its own clicks.
///
/// Only the hit kinds whose containment test lives in a plain field this
/// module can reach are narrowed, via [`primary_bounds_mut`]:
/// [`PaintedHit::Widget`]'s `area` and a nested [`PaintedHit::Scroll`]'s
/// own `viewport`. A nested scroll's `scrollbar.track` (checked by
/// [`hit_test_painted`] ahead of its viewport) is narrowed alongside it,
/// directly, never dropping the surrounding `Scroll` entry even when the
/// track itself ends up with no overlap. The chrome-layout kinds
/// (`Split`/`SplitTree`) carry several independent
/// sub-region rects apiece with no single outer bounds to narrow, and
/// aren't realistic `Scroll` content — left as-is.
pub(crate) fn clip_hits_to_viewport(hits: &mut Vec<PaintedHit>, start: usize, viewport: Rect) {
    let mut i = start;
    while i < hits.len() {
        let Some(bounds) = primary_bounds_mut(&mut hits[i]).map(|r| *r) else {
            i += 1;
            continue;
        };
        let Some(clipped) = intersect_rect(bounds, viewport) else {
            hits.remove(i);
            continue;
        };
        *primary_bounds_mut(&mut hits[i]).expect("checked above") = clipped;
        if let PaintedHit::Scroll {
            scrollbar: Some(scrollbar),
            ..
        } = &mut hits[i]
        {
            scrollbar.track = clip_or_empty(scrollbar.track, viewport);
        }
        i += 1;
    }
}

/// The hit kind's own containment-test rect — [`PaintedHit::Widget`]'s
/// `area` and a nested
/// [`PaintedHit::Scroll`]'s own `viewport` — or `None` for a chrome-layout
/// kind with no single outer bounds. The one place [`clip_hits_to_viewport`]
/// reaches into a [`PaintedHit`] by mutable reference, so narrowing one
/// kind's field and narrowing a different kind's field can't drift apart
/// into two independently-maintained match arms.
fn primary_bounds_mut(hit: &mut PaintedHit) -> Option<&mut Rect> {
    match hit {
        PaintedHit::Widget { area, .. } => Some(area),
        PaintedHit::Scroll { viewport, .. } => Some(viewport),
        PaintedHit::Split { .. } | PaintedHit::SplitTree { .. } => None,
    }
}

/// `rect` intersected with `viewport`, or a zero-area rect at `rect`'s own
/// origin when they don't overlap at all — for a hit's own secondary
/// sub-rect (a scrollbar's `track`) where "no
/// longer inside the viewport" should make just that sub-rect
/// unreachable, never drop the [`PaintedHit`] entry it's attached to (its
/// own primary bounds, narrowed separately by [`clip_hits_to_viewport`],
/// already decides that).
fn clip_or_empty(rect: Rect, viewport: Rect) -> Rect {
    intersect_rect(rect, viewport).unwrap_or(Rect::new(rect.x, rect.y, 0.0, 0.0))
}

/// The overlapping region of `a` and `b`, or `None` when they don't
/// overlap at all (zero-width/height counts as no overlap, matching
/// [`Rect::contains`]'s own half-open convention).
fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    if x1 > x0 && y1 > y0 {
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    } else {
        None
    }
}

/// The sub-region of a primitive an incoming pointer event landed on,
/// attached to the enriched event's `"hit"` key (alongside `widget`, the
/// primitive's own id) so the binding's own event handler can react to
/// "row 3" or "the divider" without re-deriving hit geometry itself.
///
/// `PartialEq`/`Eq` let [`InteractionEngine::handle`][crate::engine::InteractionEngine::handle]
/// compare a `MouseMoved`'s new hit against its previous one to decide
/// whether to synthesize `MouseEntered`/`MouseLeft` — two hits with the
/// same `WidgetId` but a different detail (e.g. a split's pane vs. its
/// divider) still count as a hover change.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HitDetail {
    SplitFirstPane,
    SplitSecondPane,
    SplitDivider,
    /// Landed on a [`quadraui::SplitTree`] divider — `split_index` is the
    /// pre-order index of the `SplitTree::Split` node being resized (see
    /// [`quadraui::SplitTreeDivider::split_index`]). Arms a drag in the
    /// binding's own event handler, mirroring `SplitDivider`'s
    /// [`crate::drag::ActiveDrag`] handling.
    SplitTreeDivider {
        split_index: usize,
    },
    /// Landed on a [`crate::layout::LeafDescriptor::Widget`]'s whole
    /// painted area. Carries no sub-region detail: the widget resolves its
    /// own sub-regions in [`DynWidget::handle`], which
    /// [`crate::engine::InteractionEngine::handle`] routes the event to.
    WidgetBody,
    /// Landed inside a [`crate::layout::LayoutNode::Scroll`]'s viewport but
    /// on no more specific descendant hit — e.g. empty space between
    /// stacked content items. Carries no sub-region detail, mirroring
    /// [`Self::WidgetBody`]; the binding's own wheel routing uses this to
    /// resolve which `Scroll` a wheel event targets.
    ScrollBody,
    /// Landed before a `Scroll`'s own scrollbar thumb (page-up/left zone).
    ScrollScrollbarTrackBefore,
    /// Landed on a `Scroll`'s own scrollbar thumb; arms a drag.
    ScrollScrollbarThumb,
    /// Landed after a `Scroll`'s own scrollbar thumb (page-down/right zone).
    ScrollScrollbarTrackAfter,
}

/// Hit-tests `point` (in the backend's native absolute coordinate space,
/// same as `UiEvent::MouseDown`'s `position`) against every primitive
/// painted this frame, returning the id and fine-grained detail of
/// whichever one claims it.
///
/// Checked in reverse paint order — later-painted primitives sit on top of
/// earlier ones — and a miss against one candidate (e.g. outside its own
/// paint `area`) falls through to the
/// next rather than stopping, so a click that misses the topmost primitive
/// can still land on one painted earlier.
pub fn hit_test_painted(hit_map: &[PaintedHit], point: Point) -> Option<(WidgetId, HitDetail)> {
    for hit in hit_map.iter().rev() {
        match hit {
            PaintedHit::Split { layout } => match layout.hit_test(point.x, point.y) {
                SplitHit::FirstPane(id) => return Some((id, HitDetail::SplitFirstPane)),
                SplitHit::SecondPane(id) => return Some((id, HitDetail::SplitSecondPane)),
                SplitHit::Divider(id) => return Some((id, HitDetail::SplitDivider)),
                SplitHit::Outside => {}
            },
            PaintedHit::SplitTree { id, layout } => {
                // A TUI mouse event's `point` is already quantized to whole
                // cells (`x = column as f32`), but `SplitTreeDivider::position`
                // is the unquantized float the layout resolved — e.g. ratio
                // 0.5 on a 40-column pane gives `position = 19.5`, which the
                // TUI rasteriser paints at column 19 (`cell_position`
                // truncates). A zero-tolerance band test on the raw float
                // position misses that painted cell for every fractional
                // position, i.e. half of all width/ratio combinations. `1.0`
                // is the minimum tolerance that provably covers the
                // truncation (`position - floor(position) < 1.0` always), and
                // matches the tolerance other backends' own divider hit-tests
                // use (`win::split_tree`, `gtk::split_tree`); `examples/common/split_tree_app.rs`
                // and `macos::split_tree` go further and use `2.0` for a
                // thicker visual grab target.
                if let Some(split_index) = layout.hit_test_divider(point, 1.0) {
                    return Some((*id, HitDetail::SplitTreeDivider { split_index }));
                }
            }
            PaintedHit::Widget { id, area, .. } => {
                if area.contains(point) {
                    return Some((*id, HitDetail::WidgetBody));
                }
            }
            PaintedHit::Scroll {
                id,
                viewport,
                scrollbar,
                ..
            } => {
                if let Some(scrollbar) = scrollbar {
                    if scrollbar.track.contains(point) {
                        let pos = match scrollbar.axis {
                            ScrollAxis::Vertical => point.y,
                            ScrollAxis::Horizontal => point.x,
                        };
                        return Some((
                            *id,
                            match scrollbar.press_zone(pos) {
                                PressZone::Before => HitDetail::ScrollScrollbarTrackBefore,
                                PressZone::Thumb => HitDetail::ScrollScrollbarThumb,
                                PressZone::After => HitDetail::ScrollScrollbarTrackAfter,
                            },
                        ));
                    }
                }
                if viewport.contains(point) {
                    return Some((*id, HitDetail::ScrollBody));
                }
            }
        }
    }
    None
}

/// Converts [`crate::wire::DecodeDiagnostics::drag_roles`]'s own
/// decode-time, `String`-keyed side table (one entry per wire leaf whose
/// dict set `"draggable"`/`"drop_target"`) into the [`WidgetId`]-keyed
/// shape [`crate::engine::InteractionEngine::set_drag_roles`] retains —
/// the same `String` → `WidgetId::new` wrapping a [`crate::layout::LayoutLeaf::id`]
/// gets at paint time (see `crate::render::paint_leaf`), so a leaf id the
/// decode recorded a [`DragRole`] against resolves to the identical
/// [`WidgetId`] a later [`hit_test_painted`] call returns for that same
/// leaf's [`PaintedHit`]. Pure: reads `roles`, builds a fresh map, touches
/// neither `roles` nor any [`PaintedHit`].
pub fn drag_roles_by_widget_id(roles: &HashMap<String, DragRole>) -> HashMap<WidgetId, DragRole> {
    roles
        .iter()
        .map(|(id, role)| (WidgetId::new(id.clone()), *role))
        .collect()
}

/// The screen position to hit-test against [`PaintedHit`]s, for the one
/// `event` variants whose own `widget` field this module enriches. `None`
/// for every other variant (e.g. `MouseMoved`, `MouseEntered`, key events),
/// which the binding's own event handler forwards to user code exactly as
/// received.
pub fn event_position(event: &UiEvent) -> Option<Point> {
    match event {
        UiEvent::MouseDown { position, .. }
        | UiEvent::MouseUp { position, .. }
        | UiEvent::DoubleClick { position, .. }
        | UiEvent::Scroll { position, .. } => Some(*position),
        _ => None,
    }
}

/// Sets `event`'s own `widget` field to `id`, for whichever of the four
/// variants [`event_position`] matched. A no-op for every other variant.
pub fn set_event_widget(event: &mut UiEvent, id: WidgetId) {
    match event {
        UiEvent::MouseDown { widget, .. }
        | UiEvent::MouseUp { widget, .. }
        | UiEvent::DoubleClick { widget, .. }
        | UiEvent::Scroll { widget, .. } => *widget = Some(id),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::Button;

    /// A widget hit whose `area` reaches well past `viewport` on every
    /// side, standing in for a scroll's oversized atomic content.
    fn oversized_widget_hit(viewport: Rect) -> PaintedHit {
        let id = WidgetId::new("log");
        PaintedHit::Widget {
            id,
            area: Rect::new(
                viewport.x,
                viewport.y - 5.0,
                viewport.width,
                viewport.height + 10.0,
            ),
            widget: Rc::new(Button::new(id, "Log")),
        }
    }

    /// [`clip_hits_to_viewport`]'s whole acceptance scenario: a button
    /// painted above a scroll, whose own descendant content's `area`
    /// leaked up past the scroll's `viewport` and over the button's own
    /// row. A click on the button's row still resolves to the button —
    /// not the leaked content — and a point well above either never
    /// resolves to anything.
    #[test]
    fn clip_hits_to_viewport_keeps_a_sibling_painted_before_the_scroll() {
        let viewport = Rect::new(0.0, 1.0, 20.0, 9.0);
        let button_id = WidgetId::new("above");
        let button = PaintedHit::Widget {
            id: button_id,
            area: Rect::new(0.0, 0.0, 20.0, 1.0),
            widget: Rc::new(Button::new(button_id, "Above")),
        };
        let scroll = PaintedHit::Scroll {
            id: WidgetId::new("scroll"),
            viewport,
            axis: ScrollAxis::Vertical,
            content_len: 0.0,
            offset: 0.0,
            scrollbar: None,
            children: Vec::new(),
        };
        let mut hits = vec![button, scroll];
        let hits_before = hits.len();
        hits.push(oversized_widget_hit(viewport));
        clip_hits_to_viewport(&mut hits, hits_before, viewport);

        // The button's own row (y=0) sits outside the scroll's
        // `viewport` (which starts at y=1) and inside the leaked hit's
        // own pre-clip bounds, so an unclipped hit map would resolve this
        // click to the scroll's content instead of the button.
        assert_eq!(
            hit_test_painted(&hits, Point { x: 0.0, y: 0.0 }),
            Some((WidgetId::new("above"), HitDetail::WidgetBody))
        );

        // Still resolves to the content for a point genuinely inside the
        // viewport -- narrowing a hit's bounds doesn't drop it outright.
        assert_eq!(
            hit_test_painted(&hits, Point { x: 0.0, y: 5.0 }),
            Some((WidgetId::new("log"), HitDetail::WidgetBody))
        );
    }

    /// A hit whose bounds don't overlap `viewport` at all is dropped
    /// outright, not just narrowed to an empty rect that would never
    /// match any point anyway — so a caller iterating `hits` directly
    /// (rather than through [`hit_test_painted`]) never sees a
    /// zero-sized leftover.
    #[test]
    fn clip_hits_to_viewport_drops_a_hit_with_no_overlap() {
        let viewport = Rect::new(0.0, 10.0, 20.0, 5.0);
        let button_id = WidgetId::new("far-above");
        let mut hits = vec![PaintedHit::Widget {
            id: button_id,
            area: Rect::new(0.0, 0.0, 20.0, 1.0),
            widget: Rc::new(Button::new(button_id, "Above")),
        }];
        clip_hits_to_viewport(&mut hits, 0, viewport);
        assert!(hits.is_empty());
    }

    /// [`PaintedHit::Scroll`]'s own `scrollbar.track` is narrowed — a nested
    /// `Scroll` painted inside an outer one, straddling the OUTER
    /// viewport's own edge, must not keep a clickable track reaching past
    /// it.
    #[test]
    fn clip_hits_to_viewport_narrows_a_scrolls_scrollbar_track() {
        let viewport = Rect::new(0.0, 0.0, 20.0, 10.0);
        let track = Rect::new(19.0, 0.0, 1.0, 24.0);
        let scrollbar = Some(Scrollbar::vertical("track", track, 0.0, 100.0, 24.0, 1.0));
        let mut hits = vec![PaintedHit::Scroll {
            id: WidgetId::new("hit-test:scroll"),
            viewport: Rect::new(0.0, 0.0, 20.0, 24.0),
            axis: ScrollAxis::Vertical,
            content_len: 100.0,
            offset: 0.0,
            scrollbar,
            children: Vec::new(),
        }];
        clip_hits_to_viewport(&mut hits, 0, viewport);

        let PaintedHit::Scroll {
            scrollbar: Some(scrollbar),
            ..
        } = &hits[0]
        else {
            panic!("still a Scroll hit with a scrollbar");
        };
        // The track's own pre-clip span
        // (`y` in `[0, 24)`) reached past `viewport`'s bottom edge (`y` in
        // `[0, 10)`) -- narrowed, a point past the viewport's own edge
        // doesn't resolve to it.
        assert!(!scrollbar.track.contains(Point { x: 19.0, y: 20.0 }));
        assert!(scrollbar.track.contains(Point { x: 19.0, y: 5.0 }));
    }

    /// [`drag_roles_by_widget_id`]'s whole contract: every `String` key
    /// becomes the identical [`WidgetId`] a [`PaintedHit`] for that same
    /// leaf id would carry, each mapped to its own unchanged [`DragRole`],
    /// with none dropped and none merged into another entry.
    #[test]
    fn drag_roles_by_widget_id_rekeys_every_entry_to_the_matching_widget_id() {
        let mut roles = HashMap::new();
        roles.insert(
            "drag1".to_string(),
            DragRole {
                draggable: true,
                drop_target: false,
            },
        );
        roles.insert(
            "drop1".to_string(),
            DragRole {
                draggable: false,
                drop_target: true,
            },
        );

        let by_widget_id = drag_roles_by_widget_id(&roles);

        assert_eq!(by_widget_id.len(), 2);
        assert_eq!(
            by_widget_id.get(&WidgetId::new("drag1")),
            Some(&DragRole {
                draggable: true,
                drop_target: false,
            })
        );
        assert_eq!(
            by_widget_id.get(&WidgetId::new("drop1")),
            Some(&DragRole {
                draggable: false,
                drop_target: true,
            })
        );
    }

    /// An empty decode-time table rekeys to an empty `WidgetId`-keyed one
    /// — no entry is invented out of nowhere.
    #[test]
    fn drag_roles_by_widget_id_of_an_empty_table_is_empty() {
        assert!(drag_roles_by_widget_id(&HashMap::new()).is_empty());
    }
}
