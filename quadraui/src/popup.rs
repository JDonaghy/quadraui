//! Widget-owned popups: the [`Popup`] a widget describes from
//! [`crate::interactive::Interactive::popups`], the [`OpenPopup`] the
//! runtime resolves it into, and the per-frame paint queue that puts every
//! open popup above the rest of the frame.
//!
//! # How a popup lives
//!
//! A popup has no state of its own. Whether it is open is something the
//! owning widget already knows: a descriptor field the app sets (a
//! `ComboBox`'s `open`), or the owner's persistent
//! [`crate::interactive::Interactive::State`]. Each frame the owner
//! answers [`crate::interactive::Interactive::popups`] from that, and the
//! answer *is* the set of open popups. Closing one is answering without
//! it.
//!
//! The runtime does three things with that answer:
//!
//! - **Paint.** [`crate::DrawsPrimitives::draw_dyn`] asks every
//!   identified widget it paints for its popups and queues them; the
//!   runner paints the queue through [`paint_open_popups`] at the end of
//!   the frame, so popups land above everything the frame painted,
//!   lowest [`Popup::z`] first. While a modal is open
//!   ([`crate::ModalStack`]) no popup paints: the modal is the top
//!   layer, and routing ignores popups until it closes.
//! - **Hit-test.** The routing engine resolves the same popups into the
//!   event's [`crate::frame_tree::FrameTree`]
//!   ([`crate::frame_tree::FrameTree::push_popup`]), and
//!   [`crate::frame_tree::FrameTree::hit`] answers a popup before any
//!   node. An event over a popup reaches the *owner* as
//!   [`crate::interactive::RoutedEvent::PopupEvent`], resolved against the
//!   popup's content (popup-local coordinates, the content's own
//!   sub-target).
//! - **Dismiss.** A press outside every popup and outside its owner, or
//!   Escape, reaches the owner as
//!   [`crate::interactive::RoutedEvent::PopupDismissed`] when the popup's
//!   [`PopupDismiss`] allows it. The owner reacts the way it reacts to
//!   any other close request: it emits the event that makes the app clear
//!   the descriptor field, or clears its own state.
//!
//! The content widget only paints and names its sub-parts
//! ([`crate::interactive::Interactive::sub_regions`]); its own `handle` is
//! never called. The owner holds the behaviour, so one widget's popup
//! logic stays in one `handle`.

use std::cell::RefCell;

use crate::event::Rect;
use crate::primitive::{DynWidget, Size};
use crate::types::WidgetId;

/// Which of a widget's popups an event or a dismissal is about — chosen
/// by the owner in [`Popup::new`] and handed back verbatim in
/// [`crate::interactive::RoutedEvent::PopupEvent`]/
/// [`crate::interactive::RoutedEvent::PopupDismissed`]. A widget with one
/// popup uses any constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PopupKey(pub u64);

/// Where a popup sits relative to its [`Popup::anchor`].
///
/// [`Popup::resolve`] flips to the opposite side when the preferred one
/// would leave the viewport and the opposite side fits, then clamps the
/// result into the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PopupSide {
    /// Left-aligned with the anchor, directly under it — a dropdown.
    #[default]
    Below,
    /// Left-aligned with the anchor, directly over it.
    Above,
    /// Top-aligned with the anchor, to its right — a submenu.
    After,
    /// Top-aligned with the anchor, to its left.
    Before,
    /// Covering the anchor's top-left corner.
    Over,
}

/// Which gestures close a popup on the runtime's behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PopupDismiss {
    /// A press outside the popup and outside its owner.
    pub outside_click: bool,
    /// The Escape key, whichever widget holds focus.
    pub escape: bool,
}

impl PopupDismiss {
    /// Dismissed by both an outside press and Escape — the default.
    pub const ALL: Self = Self {
        outside_click: true,
        escape: true,
    };
    /// Never dismissed by the runtime: the owner closes it itself.
    pub const NONE: Self = Self {
        outside_click: false,
        escape: false,
    };
}

impl Default for PopupDismiss {
    fn default() -> Self {
        Self::ALL
    }
}

/// Why the runtime dismissed a popup — carried by
/// [`crate::interactive::RoutedEvent::PopupDismissed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DismissReason {
    /// A press landed outside the popup and outside its owner.
    OutsideClick,
    /// Escape was pressed.
    Escape,
}

/// One popup a widget owns, as its
/// [`crate::interactive::Interactive::popups`] describes it.
///
/// `anchor` is in the **owner's local space** (the same space as
/// [`crate::interactive::Interactive::sub_regions`]), so an owner derives
/// it from its own layout without knowing where it was painted.
///
/// ```
/// use quadraui::popup::{Popup, PopupKey, PopupSide};
/// use quadraui::primitive::Size;
/// use quadraui::{Rect, StatusBar, WidgetId};
///
/// let content = StatusBar {
///     id: WidgetId::new("dropdown:content"),
///     left_segments: Vec::new(),
///     right_segments: Vec::new(),
/// };
/// let popup = Popup::new(
///     PopupKey(0),
///     Rect::new(0.0, 0.0, 20.0, 1.0),
///     Size::new(20.0, 5.0),
///     content,
/// )
/// .side(PopupSide::Below);
///
/// // Owner painted at (10, 3) in an 80×24 viewport: the popup drops
/// // directly below the anchor.
/// let rect = popup.resolve(
///     Rect::new(10.0, 3.0, 20.0, 1.0),
///     Rect::new(0.0, 0.0, 80.0, 24.0),
/// );
/// assert_eq!(rect, Rect::new(10.0, 4.0, 20.0, 5.0));
/// ```
#[non_exhaustive]
pub struct Popup {
    /// Which of the owner's popups this is.
    pub key: PopupKey,
    /// The owner-local rect the popup attaches to.
    pub anchor: Rect,
    /// The size the popup asks for; [`Self::resolve`] shrinks it to fit
    /// the viewport.
    pub size: Size,
    /// Which side of `anchor` the popup prefers.
    pub side: PopupSide,
    /// Stacking order among open popups: higher paints later and is
    /// hit-tested first. Ties keep the owners' paint order.
    pub z: i32,
    /// Which gestures dismiss it on the runtime's behalf.
    pub dismiss: PopupDismiss,
    /// What the popup shows. Painted through
    /// [`crate::DrawsPrimitives::draw_dyn`] in the popup's resolved rect;
    /// its [`crate::interactive::Interactive::sub_regions`] name the
    /// sub-target a [`crate::interactive::RoutedEvent::PopupEvent`]
    /// carries.
    pub content: Box<dyn DynWidget>,
}

impl std::fmt::Debug for Popup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Popup")
            .field("key", &self.key)
            .field("anchor", &self.anchor)
            .field("size", &self.size)
            .field("side", &self.side)
            .field("z", &self.z)
            .field("dismiss", &self.dismiss)
            .field("content", &self.content.id())
            .finish()
    }
}

impl Popup {
    /// A popup showing `content` at `size`, below `anchor`, at `z` 0,
    /// dismissed by an outside press or Escape.
    pub fn new(key: PopupKey, anchor: Rect, size: Size, content: impl DynWidget) -> Self {
        Self {
            key,
            anchor,
            size,
            side: PopupSide::default(),
            z: 0,
            dismiss: PopupDismiss::default(),
            content: Box::new(content),
        }
    }

    /// This popup with its preferred side replaced.
    pub fn side(mut self, side: PopupSide) -> Self {
        self.side = side;
        self
    }

    /// This popup with its stacking order replaced.
    pub fn z(mut self, z: i32) -> Self {
        self.z = z;
        self
    }

    /// This popup with its dismissal rules replaced.
    pub fn dismiss(mut self, dismiss: PopupDismiss) -> Self {
        self.dismiss = dismiss;
        self
    }

    /// The absolute rect this popup occupies when its owner is painted at
    /// `owner_rect` inside `viewport`.
    ///
    /// The preferred side flips when it would cross the viewport edge and
    /// the opposite side fits; the result is then clamped into the
    /// viewport and shrunk to it. An empty `viewport` (no frame begun
    /// yet) skips both, placing the popup exactly where it asked.
    pub fn resolve(&self, owner_rect: Rect, viewport: Rect) -> Rect {
        let anchor = Rect::new(
            owner_rect.x + self.anchor.x,
            owner_rect.y + self.anchor.y,
            self.anchor.width,
            self.anchor.height,
        );
        let (w, h) = (self.size.width, self.size.height);
        let bounded = viewport.width > 0.0 && viewport.height > 0.0;
        let right = viewport.x + viewport.width;
        let bottom = viewport.y + viewport.height;
        let below = anchor.y + anchor.height;
        let after = anchor.x + anchor.width;
        let (x, y) = match self.side {
            PopupSide::Below => {
                let flip = bounded && below + h > bottom && anchor.y - h >= viewport.y;
                (anchor.x, if flip { anchor.y - h } else { below })
            }
            PopupSide::Above => {
                let flip = bounded && anchor.y - h < viewport.y && below + h <= bottom;
                (anchor.x, if flip { below } else { anchor.y - h })
            }
            PopupSide::After => {
                let flip = bounded && after + w > right && anchor.x - w >= viewport.x;
                (if flip { anchor.x - w } else { after }, anchor.y)
            }
            PopupSide::Before => {
                let flip = bounded && anchor.x - w < viewport.x && after + w <= right;
                (if flip { after } else { anchor.x - w }, anchor.y)
            }
            PopupSide::Over => (anchor.x, anchor.y),
        };
        if !bounded {
            return Rect::new(x, y, w, h);
        }
        let w = w.min(viewport.width);
        let h = h.min(viewport.height);
        let x = x.min(right - w).max(viewport.x);
        let y = y.min(bottom - h).max(viewport.y);
        Rect::new(x, y, w, h)
    }
}

/// A [`Popup`] resolved for one frame: its owner, its absolute rect and
/// the content to paint there.
#[non_exhaustive]
pub struct OpenPopup {
    /// The widget whose [`crate::interactive::Interactive::popups`]
    /// produced this popup, and which receives its events.
    pub owner: WidgetId,
    /// The owner-chosen key — see [`PopupKey`].
    pub key: PopupKey,
    /// The absolute rect the popup occupies.
    pub rect: Rect,
    /// Stacking order — see [`Popup::z`].
    pub z: i32,
    /// Which gestures dismiss it — see [`Popup::dismiss`].
    pub dismiss: PopupDismiss,
    /// What the popup shows — see [`Popup::content`].
    pub content: Box<dyn DynWidget>,
}

impl std::fmt::Debug for OpenPopup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenPopup")
            .field("owner", &self.owner)
            .field("key", &self.key)
            .field("rect", &self.rect)
            .field("z", &self.z)
            .field("dismiss", &self.dismiss)
            .finish_non_exhaustive()
    }
}

impl OpenPopup {
    /// Resolve `popup`, owned by `owner` painted at `owner_rect`, inside
    /// `viewport` — see [`Popup::resolve`].
    pub fn resolve(owner: WidgetId, owner_rect: Rect, popup: Popup, viewport: Rect) -> Self {
        let rect = popup.resolve(owner_rect, viewport);
        Self {
            owner,
            key: popup.key,
            rect,
            z: popup.z,
            dismiss: popup.dismiss,
            content: popup.content,
        }
    }
}

/// Insert `popup` into `stack`, kept in paint order: ascending
/// [`OpenPopup::z`], with ties after every popup already there. A popup
/// with the same owner and key as one already in `stack` replaces it, so
/// a widget painted twice in one frame still shows each popup once.
pub(crate) fn insert_by_z(stack: &mut Vec<OpenPopup>, popup: OpenPopup) {
    stack.retain(|p| !(p.owner == popup.owner && p.key == popup.key));
    let at = stack.partition_point(|p| p.z <= popup.z);
    stack.insert(at, popup);
}

/// One popup [`paint_open_popups`] painted this frame — see
/// [`painted_popups`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintedPopup {
    /// The owning widget.
    pub owner: WidgetId,
    /// The owner-chosen key.
    pub key: PopupKey,
    /// The absolute rect it painted into.
    pub rect: Rect,
    /// Its stacking order.
    pub z: i32,
}

/// The per-thread popup queue and paint record. Thread-local for the
/// same reason as [`crate::primitive::with_painted_widgets`]'s store: it
/// is filled from [`crate::DrawsPrimitives::draw_dyn`], which reaches the
/// backend only through `Backend` methods, and each backend paints on the
/// thread that owns its event loop.
#[derive(Default)]
struct PopupFrame {
    pending: Vec<OpenPopup>,
    painted: Vec<PaintedPopup>,
    /// Set while [`paint_open_popups`] paints popup content, whose own
    /// popups [`with_nested_popups`] has already resolved.
    painting: bool,
}

thread_local! {
    static POPUP_FRAME: RefCell<PopupFrame> = RefCell::new(PopupFrame::default());
}

/// Drop the previous frame's queued and painted popups. Called from the
/// shared begin-frame step every backend runs.
pub fn clear_popup_frame() {
    POPUP_FRAME.with(|f| *f.borrow_mut() = PopupFrame::default());
}

/// Queue `popup` for [`paint_open_popups`].
pub(crate) fn queue_popup(popup: OpenPopup) {
    POPUP_FRAME.with(|f| {
        let mut frame = f.borrow_mut();
        if !frame.painting {
            insert_by_z(&mut frame.pending, popup);
        }
    });
}

/// Every popup [`paint_open_popups`] painted this frame, in paint order
/// (back to front) — the record a driver reads to click inside one.
pub fn painted_popups() -> Vec<PaintedPopup> {
    POPUP_FRAME.with(|f| f.borrow().painted.clone())
}

/// How many levels of popups-opened-by-popups [`with_nested_popups`]
/// follows before it stops — a bound on a content widget that keeps
/// answering [`crate::interactive::Interactive::popups`] with itself.
const MAX_POPUP_DEPTH: usize = 8;

/// `roots` plus every popup their content opens, level by level, in
/// paint order — the one answer both paint and hit-testing use.
///
/// A popup opened by a popup's content (a submenu) belongs to the root
/// popup's owner: the owner built that content, so it receives the
/// nested popup's events and dismissals under the key the content chose,
/// and picks content keys that do not collide with its own. A nested
/// popup is placed against its parent's rect and never stacks below its
/// parent: its z is raised to the parent's when lower.
pub(crate) fn with_nested_popups<B: crate::Backend + ?Sized>(
    backend: &B,
    roots: Vec<OpenPopup>,
) -> Vec<OpenPopup> {
    let viewport = crate::backend::viewport_rect(backend.viewport());
    let mut levels = vec![roots];
    while levels.len() < MAX_POPUP_DEPTH {
        let mut next = Vec::new();
        for parent in levels.last().into_iter().flatten() {
            let content = parent.content.as_ref();
            if content.id().is_none() || !content.opens_popups() {
                continue;
            }
            let layout = crate::backend::layout_like_paint(backend, content, parent.rect);
            let popups = crate::widget_state::with_widget_state_ref(content.id(), |state| {
                content.popups(state, layout.as_ref())
            });
            for popup in popups {
                let mut open = OpenPopup::resolve(parent.owner, parent.rect, popup, viewport);
                open.z = open.z.max(parent.z);
                next.push(open);
            }
        }
        if next.is_empty() {
            break;
        }
        levels.push(next);
    }
    let mut stack = Vec::new();
    for popup in levels.into_iter().flatten() {
        insert_by_z(&mut stack, popup);
    }
    stack
}

/// Paint every popup queued this frame, and every popup their content
/// opens, above everything already painted, in [`with_nested_popups`]'s
/// order.
///
/// Each runner calls this once per frame inside the frame scope, after
/// the app's `render` and the runner's own focus ring and selection
/// highlight. Nothing paints while the backend's
/// [`crate::ModalStack`] holds an entry: a modal sits above every
/// widget's popups, and routing ignores popups for as long as one is
/// open, so the popups stay hidden until it closes.
pub fn paint_open_popups(backend: &mut dyn crate::Backend) {
    use crate::DrawsPrimitives;
    let queued = POPUP_FRAME.with(|f| std::mem::take(&mut f.borrow_mut().pending));
    let modal_open = backend
        .modal_stack_handle()
        .try_borrow()
        .map_or(true, |stack| !stack.is_empty());
    if queued.is_empty() || modal_open {
        return;
    }
    let stack = with_nested_popups(&*backend, queued);
    POPUP_FRAME.with(|f| f.borrow_mut().painting = true);
    for popup in &stack {
        let _ = backend.draw_dyn(popup.rect, popup.content.as_ref(), &());
    }
    POPUP_FRAME.with(|f| {
        let mut frame = f.borrow_mut();
        frame.painting = false;
        frame.painted.extend(stack.iter().map(|popup| PaintedPopup {
            owner: popup.owner,
            key: popup.key,
            rect: popup.rect,
            z: popup.z,
        }));
    });
}

/// [`paint_open_popups`] for a runner that must not unwind out of its
/// paint callback: a panic while painting popup content is reported once
/// and the frame finishes without the remaining popups.
#[cfg(any(all(feature = "macos", target_os = "macos"), feature = "win"))]
pub(crate) fn paint_open_popups_caught(backend: &mut dyn crate::Backend) {
    let painted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        paint_open_popups(backend);
    }));
    if let Err(payload) = painted {
        POPUP_FRAME.with(|f| f.borrow_mut().painting = false);
        crate::desktop::report_caught_panic_once("popup::paint_open_popups", payload.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::{LayoutCtx, Primitive};

    #[derive(Debug, Default)]
    struct Blank;

    impl crate::a11y::A11yPrimitive for Blank {
        fn a11y_info(&self) -> crate::a11y::A11yInfo {
            crate::a11y::A11yInfo::default()
        }
    }

    impl Primitive for Blank {
        type Layout = Rect;
        type Event = ();

        fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
            rect
        }

        fn paint(
            &self,
            _layout: &Rect,
            _surface: &mut dyn crate::paint_surface::PaintSurface,
            _env: &crate::PaintEnv,
        ) {
        }
    }

    crate::non_interactive!(Blank);

    fn popup(side: PopupSide, w: f32, h: f32) -> Popup {
        Popup::new(
            PopupKey(0),
            Rect::new(0.0, 0.0, 10.0, 1.0),
            Size::new(w, h),
            Blank,
        )
        .side(side)
    }

    const VIEWPORT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 80.0,
        height: 24.0,
    };

    #[test]
    fn below_drops_under_the_anchor_when_it_fits() {
        let rect =
            popup(PopupSide::Below, 10.0, 5.0).resolve(Rect::new(4.0, 2.0, 10.0, 1.0), VIEWPORT);
        assert_eq!(rect, Rect::new(4.0, 3.0, 10.0, 5.0));
    }

    #[test]
    fn below_flips_above_the_anchor_near_the_bottom_edge() {
        let rect =
            popup(PopupSide::Below, 10.0, 5.0).resolve(Rect::new(4.0, 22.0, 10.0, 1.0), VIEWPORT);
        assert_eq!(rect, Rect::new(4.0, 17.0, 10.0, 5.0));
    }

    #[test]
    fn after_flips_to_the_left_near_the_right_edge() {
        let rect =
            popup(PopupSide::After, 20.0, 3.0).resolve(Rect::new(65.0, 2.0, 10.0, 1.0), VIEWPORT);
        assert_eq!(rect, Rect::new(45.0, 2.0, 20.0, 3.0));
    }

    #[test]
    fn an_oversized_popup_is_clamped_into_the_viewport() {
        let rect =
            popup(PopupSide::Below, 200.0, 50.0).resolve(Rect::new(4.0, 2.0, 10.0, 1.0), VIEWPORT);
        assert_eq!(rect, VIEWPORT);
    }

    #[test]
    fn an_empty_viewport_places_the_popup_exactly_where_it_asked() {
        let rect = popup(PopupSide::Below, 10.0, 5.0)
            .resolve(Rect::new(4.0, 30.0, 10.0, 1.0), Rect::default());
        assert_eq!(rect, Rect::new(4.0, 31.0, 10.0, 5.0));
    }

    fn open(owner: &str, key: u64, z: i32) -> OpenPopup {
        OpenPopup::resolve(
            WidgetId::new(owner),
            Rect::new(0.0, 0.0, 10.0, 1.0),
            Popup::new(PopupKey(key), Rect::default(), Size::new(1.0, 1.0), Blank).z(z),
            VIEWPORT,
        )
    }

    #[test]
    fn insert_by_z_keeps_ascending_z_and_arrival_order_among_ties() {
        let mut stack = Vec::new();
        insert_by_z(&mut stack, open("a", 0, 1));
        insert_by_z(&mut stack, open("b", 0, 0));
        insert_by_z(&mut stack, open("c", 0, 1));
        insert_by_z(&mut stack, open("d", 0, -1));
        let order: Vec<_> = stack.iter().map(|p| p.owner).collect();
        assert_eq!(
            order,
            ["d", "b", "a", "c"].map(WidgetId::new).to_vec(),
            "lowest z first, ties in arrival order"
        );
    }

    #[test]
    fn insert_by_z_replaces_a_popup_with_the_same_owner_and_key() {
        let mut stack = Vec::new();
        insert_by_z(&mut stack, open("a", 0, 0));
        insert_by_z(&mut stack, open("a", 1, 0));
        insert_by_z(&mut stack, open("a", 0, 5));
        assert_eq!(stack.len(), 2);
        assert_eq!(stack[1].key, PopupKey(0));
        assert_eq!(stack[1].z, 5);
    }
}
