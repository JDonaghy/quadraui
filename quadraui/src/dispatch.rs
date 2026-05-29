//! Cross-backend mouse + scroll dispatch.
//!
//! Backends call into these free functions with raw (platform-translated)
//! mouse and scroll events. Quadraui consults the [`ModalStack`] first,
//! decides which widget — if any — should receive the event, and returns
//! a `Vec<UiEvent>` the backend pushes onto its per-frame event queue.
//!
//! The key guarantee: **events landing inside an open modal cannot fall
//! through to widgets behind it**. This is the contract vimcode's
//! TUI `mouse.rs` enforces inline; centralising it here eliminates the
//! class of bug where a new modal is added to one backend but forgotten
//! in another (issue #192 is the motivating case).
//!
//! # What's here in the pilot
//!
//! [`dispatch_mouse_down`] only. Mouse-up, drag, and scroll dispatch
//! arrive in follow-up commits (per the B.4 event-routing plan).
//!
//! # What's explicitly not here
//!
//! - Inner hit refinement within a modal — `Palette`, `Dialog`, etc.
//!   have their own `*Layout::hit_test` for that. The dispatcher
//!   identifies the topmost modal under the cursor; the app still
//!   calls the primitive's hit test afterward if it needs an
//!   item-level target.
//! - Base-layer hit testing — the editor, sidebar, tabs, and so on.
//!   The pilot leaves base-layer events going through the backend's
//!   existing mouse handlers, which are already per-backend. Later
//!   commits can route them through here too.

use crate::event::{MouseButton, Point, ScrollDelta, UiEvent};
use crate::modal_stack::ModalStack;
use crate::primitives::palette::PaletteEvent;
use crate::primitives::scrollbar::ScrollAxis;
use crate::types::WidgetId;
use crate::Modifiers;

// ─── Drag state ─────────────────────────────────────────────────────────────

/// What's being dragged, if anything. Backends hold one [`DragState`]
/// (typically on the same struct that owns the [`ModalStack`]) and
/// update it from the click / drag / release handlers via
/// [`DragState::begin`] / [`DragState::end`]. The dispatch functions
/// below consult it to decide which primitive-specific event to emit.
#[derive(Debug, Clone, PartialEq)]
pub enum DragTarget {
    /// A vertical scrollbar drag. `track_start` and `track_length`
    /// are in the backend's native units (pixels for GTK; cells for
    /// TUI) and define the track region the thumb can traverse.
    /// `max_scroll` is supplied directly by the caller (typically
    /// `total - visible`) so the dispatcher doesn't need to know
    /// the unit system. The dispatcher maps a drag
    /// point's y to a scroll offset via linear interpolation:
    ///
    /// ```text
    /// rel    = (y - track_start) / track_length   (clamped 0..=1)
    /// offset = round(rel * max_scroll)
    /// ```
    ScrollbarY {
        /// Which widget's scrollbar is being dragged. Used to route
        /// the resulting event back to the right primitive.
        widget: WidgetId,
        /// Track top in the backend's native y-coordinate.
        track_start: f32,
        /// Track length in the backend's native units. Must be > 0.
        track_length: f32,
        /// Actual painted thumb length in the same units as
        /// `track_length`. The dispatcher uses this directly for
        /// `effective_track = track_length - thumb_length` — no
        /// recomputation from visible/total counts.
        thumb_length: f32,
        /// Maximum scroll offset the drag can produce. The caller
        /// supplies this directly (typically `total - visible`)
        /// so the dispatcher doesn't need to know the unit system.
        max_scroll: usize,
        /// Where on the thumb the cursor was at click-down time, as
        /// the cursor's offset from the thumb's top in the backend's
        /// native units. The dispatcher subtracts this from
        /// `position.y` before computing `rel`, so the cursor stays
        /// at the same relative spot on the thumb during the drag —
        /// the standard "thumb doesn't jump when you grab it" UX
        /// every native scrollbar provides. Set to `0.0` for clicks
        /// on the track *outside* the thumb (jump-to-position
        /// behavior — the thumb hops so its top lands at the cursor).
        grab_offset: f32,
        /// When true, offset 0 means "at the bottom" (scrollback
        /// style). The dispatcher flips the ratio so the thumb sits
        /// at the bottom when offset is 0 and at the top when fully
        /// scrolled back.
        inverted: bool,
    },
    /// A horizontal scrollbar drag. Same shape as [`DragTarget::ScrollbarY`]
    /// but operates on the x-axis: `track_start` is the leftmost x of
    /// the track, `track_length` is the track width, and the dispatcher
    /// reads `position.x` (not `position.y`) to compute the offset.
    ScrollbarX {
        widget: WidgetId,
        /// Track left in the backend's native x-coordinate.
        track_start: f32,
        /// Track length in the backend's native units. Must be > 0.
        track_length: f32,
        /// Actual painted thumb length. See
        /// [`DragTarget::ScrollbarY::thumb_length`].
        thumb_length: f32,
        /// Maximum scroll offset. See
        /// [`DragTarget::ScrollbarY::max_scroll`].
        max_scroll: usize,
        /// Cursor's x-offset from the thumb's left at click-down. See
        /// [`DragTarget::ScrollbarY`] for why this matters.
        grab_offset: f32,
        /// When true, offset 0 means "at the right edge". See
        /// [`DragTarget::ScrollbarY::inverted`].
        inverted: bool,
    },
    /// A text-selection drag. `region` is the id of the
    /// [`TextRegion`] where the drag started; `anchor` is the screen
    /// position at click-down. The anchor is fixed for the lifetime of
    /// the drag; [`dispatch_mouse_drag`] reads it to emit
    /// [`UiEvent::TextSelectionChanged`] with the current cursor as
    /// the `focus`.
    TextSelection {
        /// Which text region is being selected.
        region: WidgetId,
        /// Screen position where the drag started.
        anchor: Point,
    },
}

/// One drag in progress, or none. Backends hold one instance; call
/// [`Self::begin`] on mouse-down over a draggable region and
/// [`Self::end`] on mouse-up. The dispatch functions here read it to
/// decide whether a mouse-move should produce a drag-update event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DragState {
    current: Option<DragTarget>,
}

impl DragState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start tracking a drag. Overwrites any previous state —
    /// backends are expected to call [`Self::end`] on mouse-up before
    /// beginning the next drag, but overwriting is the safer default
    /// than panicking (spurious duplicate begins do happen in
    /// gesture-heavy paths).
    pub fn begin(&mut self, target: DragTarget) {
        self.current = Some(target);
    }

    /// Clear the drag. No-op if nothing is in progress.
    pub fn end(&mut self) {
        self.current = None;
    }

    pub fn is_active(&self) -> bool {
        self.current.is_some()
    }

    pub fn target(&self) -> Option<&DragTarget> {
        self.current.as_ref()
    }
}

// ─── Dispatch functions ─────────────────────────────────────────────────────

/// Translate a raw mouse-down event into a `Vec<UiEvent>`, consulting
/// the modal stack first.
///
/// # Returns
///
/// Three cases:
///
/// 1. **Click landed on an open modal.** Emits
///    `[UiEvent::MouseDown { widget: Some(id), .. }]`. The app's
///    dispatch matches on the widget id and routes to the modal's
///    inner hit-test if it needs finer resolution.
/// 2. **Click landed outside every open modal, but a modal is open.**
///    Emits `[UiEvent::MouseDown { widget: None, .. }, UiEvent::Palette(id, Closed)]`
///    where `id` is the topmost modal (the one the backdrop click
///    dismisses). This is the "click outside to close" convention every
///    desktop platform follows. **Base-layer widgets must not receive
///    the event** — the event vec doesn't include a second
///    `MouseDown` for a base widget, and the caller should consume on
///    the emission of `PaletteEvent::Closed`.
/// 3. **No modal open.** Emits `[UiEvent::MouseDown { widget: None, .. }]`
///    with no primitive event; the backend's existing base-layer
///    mouse handlers deal with it as they did before the pilot.
///
/// Callers that only care about case 1 (e.g. a GTK event handler that
/// wants to stop the drag from leaking to the editor) can check the
/// returned vec:
///
/// ```ignore
/// let events = dispatch_mouse_down(&stack, pos, button, mods);
/// if events.iter().any(|e| matches!(e, UiEvent::MouseDown { widget: Some(_), .. })
///     || matches!(e, UiEvent::Palette(_, PaletteEvent::Closed)))
/// {
///     // modal consumed the click — don't run base-layer dispatch
///     return;
/// }
/// ```
///
/// # Note on `PaletteEvent::Closed`
///
/// Today this dispatcher always emits [`PaletteEvent::Closed`] for
/// backdrop clicks regardless of which primitive type is topmost on
/// the stack. That's a pilot-scope simplification — the palette is
/// the only consumer wired up in commit 1. When a second modal type
/// needs the backdrop-dismiss behaviour (e.g. Dialog), we'll generalise
/// to a `ModalDismissed(WidgetId)` event or per-primitive variants.
pub fn dispatch_mouse_down(
    stack: &ModalStack,
    position: Point,
    button: MouseButton,
    modifiers: Modifiers,
) -> Vec<UiEvent> {
    // Case 1: click landed inside an open modal.
    if let Some(widget_id) = stack.hit_test(position) {
        return vec![UiEvent::MouseDown {
            widget: Some(widget_id.clone()),
            button,
            position,
            modifiers,
        }];
    }

    // Case 2: modal(s) open but click was outside them → dismiss topmost.
    if let Some(top) = stack.top() {
        return vec![
            UiEvent::MouseDown {
                widget: None,
                button,
                position,
                modifiers,
            },
            UiEvent::Palette(top.id.clone(), PaletteEvent::Closed),
        ];
    }

    // Case 3: no modals open. Event belongs to the base layer.
    vec![UiEvent::MouseDown {
        widget: None,
        button,
        position,
        modifiers,
    }]
}

/// Translate a mouse-move event. When no drag is in progress, emits a
/// plain [`UiEvent::MouseMoved`]. When a [`DragTarget::ScrollbarY`]
/// drag is active, additionally emits a generic
/// [`UiEvent::ScrollOffsetChanged { widget, new_offset }`] with the
/// derived scroll offset. The app's dispatch matches on the event
/// (and switches on `widget` to route to the right scroll-state
/// field) without needing the track geometry — this function owns
/// the translation.
///
/// # How the offset is computed
///
/// `ratio = ((point.y - track_start) / track_length).clamp(0, 1)`
/// `new_offset = round(ratio * max_scroll)` where `max_scroll` is
/// supplied directly by the `DragTarget`.
///
/// This mirrors the math TUI already uses in `mouse.rs`'s
/// `dragging_picker_sb` branch, extended to f32 so it works for
/// pixel-unit backends (GTK, macOS) as well as cell-unit backends
/// (TUI).
pub fn dispatch_mouse_drag(
    drag: &DragState,
    position: Point,
    buttons: crate::event::ButtonMask,
) -> Vec<UiEvent> {
    let mut events = vec![UiEvent::MouseMoved { position, buttons }];

    match drag.target() {
        Some(DragTarget::ScrollbarY {
            widget,
            track_start,
            track_length,
            thumb_length,
            max_scroll,
            grab_offset,
            inverted,
        }) if *track_length > 0.0 && *max_scroll > 0 => {
            let effective_track = (*track_length - *thumb_length).max(1.0);
            let rel = (position.y - *track_start - *grab_offset) / effective_track;
            let clamped = rel.clamp(0.0, 1.0);
            let clamped = if *inverted { 1.0 - clamped } else { clamped };
            let new_offset = (clamped * *max_scroll as f32).round() as usize;
            events.push(UiEvent::ScrollOffsetChanged {
                widget: widget.clone(),
                new_offset,
            });
        }
        Some(DragTarget::ScrollbarX {
            widget,
            track_start,
            track_length,
            thumb_length,
            max_scroll,
            grab_offset,
            inverted,
        }) if *track_length > 0.0 && *max_scroll > 0 => {
            let effective_track = (*track_length - *thumb_length).max(1.0);
            let rel = (position.x - *track_start - *grab_offset) / effective_track;
            let clamped = rel.clamp(0.0, 1.0);
            let clamped = if *inverted { 1.0 - clamped } else { clamped };
            let new_offset = (clamped * *max_scroll as f32).round() as usize;
            events.push(UiEvent::ScrollOffsetChanged {
                widget: widget.clone(),
                new_offset,
            });
        }
        Some(DragTarget::TextSelection { region, anchor }) => {
            events.push(UiEvent::TextSelectionChanged {
                region: region.clone(),
                anchor: *anchor,
                focus: position,
            });
        }
        _ => {}
    }

    events
}

/// Translate a mouse-up event. Always emits [`UiEvent::MouseUp`] and
/// clears any active drag state. If the click landed inside a modal,
/// the `MouseUp` carries the modal's `widget` id — matching
/// [`dispatch_mouse_down`]'s precedence — so apps can treat a drag
/// that crosses the modal boundary atomically.
pub fn dispatch_mouse_up(
    stack: &ModalStack,
    drag: &mut DragState,
    position: Point,
    button: MouseButton,
) -> Vec<UiEvent> {
    drag.end();
    let widget = stack.hit_test(position).cloned();
    vec![UiEvent::MouseUp {
        widget,
        button,
        position,
    }]
}

// ─── Text region ─────────────────────────────────────────────────────────────

/// A selectable text region registered during paint. Apps push one
/// entry per selectable text area each frame;
/// [`dispatch_click`] hit-tests the list to begin a
/// [`DragTarget::TextSelection`] drag when the user clicks inside.
///
/// Push in paint order (back-to-front). The dispatcher walks
/// last-to-first so the topmost-painted region wins on overlap.
/// Text regions take priority over [`ScrollSurface`] *bodies* (not
/// scrollbars) — a text region registered inside a scroll surface body
/// will capture clicks first.
///
/// # Relationship with `ScrollSurface`
///
/// A text region is typically co-located with a scroll surface: the
/// scroll surface owns the scrollbar; the text region owns the content
/// body. Register them both, and the precedence chain
/// (scrollbar thumb/track → text region → surface body → none) falls
/// out automatically.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRegion {
    pub id: WidgetId,
    pub bounds: crate::event::Rect,
}

/// Compute the screen-cell ranges covered by a line-wise text selection.
///
/// Given `anchor` and `focus` in screen coordinates (TUI: cells;
/// GTK/macOS: pixels) and the `bounds` of the [`TextRegion`], returns
/// a `Vec` of `(row, col_start, col_end)` tuples (half-open:
/// `col_start..col_end`) in document order (top-to-bottom).
///
/// **Line-wise stream semantics** (same as most terminal selections):
///
/// | Position | Range |
/// |---|---|
/// | First row | `anchor_col .. region_end_col` |
/// | Middle rows | `region_start_col .. region_end_col` |
/// | Last row | `region_start_col .. focus_col + 1` |
/// | Single row | `min_col .. max_col + 1` |
///
/// Coordinates are clamped to `bounds`. Returns an empty `Vec` when
/// `anchor == focus` (selection collapsed to a point — plain click).
pub fn text_selection_line_range(
    anchor: Point,
    focus: Point,
    bounds: crate::event::Rect,
) -> Vec<(u16, u16, u16)> {
    // Empty when anchor == focus (no movement → collapsed click).
    let ax = anchor.x.round() as i32;
    let ay = anchor.y.round() as i32;
    let fx = focus.x.round() as i32;
    let fy = focus.y.round() as i32;
    if ax == fx && ay == fy {
        return Vec::new();
    }

    // Put in document order (top-to-bottom, left-to-right).
    let (start, end) = if ay < fy || (ay == fy && ax <= fx) {
        (anchor, focus)
    } else {
        (focus, anchor)
    };

    let region_x = bounds.x.round() as u16;
    let region_end_x = (bounds.x + bounds.width).round() as u16;
    let region_y = bounds.y.round() as u16;
    let region_end_y = (bounds.y + bounds.height).round() as u16;

    // Clamp start/end rows and cols to region.
    let start_row = (start.y.round() as u16).clamp(region_y, region_end_y.saturating_sub(1));
    let end_row = (end.y.round() as u16).clamp(region_y, region_end_y.saturating_sub(1));
    let start_col = (start.x.round() as u16).clamp(region_x, region_end_x);
    // End col is inclusive → add 1 for the half-open range, then clamp.
    let end_col = ((end.x.round() as u16).saturating_add(1)).clamp(region_x, region_end_x);

    if start_row == end_row {
        if start_col >= end_col {
            return Vec::new();
        }
        return vec![(start_row, start_col, end_col)];
    }

    let mut ranges = Vec::with_capacity((end_row - start_row + 1) as usize);
    // First row: from start_col to region end.
    ranges.push((start_row, start_col, region_end_x));
    // Middle rows: full region width.
    for row in (start_row + 1)..end_row {
        ranges.push((row, region_x, region_end_x));
    }
    // Last row: from region start to end_col.
    ranges.push((end_row, region_x, end_col));
    ranges
}

// ─── Scroll dispatch ──────────────────────────────────────────────────────

/// A scrollable surface registered during paint. The consumer pushes
/// one entry per scrollable widget each frame; [`dispatch_scroll`]
/// hit-tests the list to route wheel events to the right widget.
///
/// Push in paint order (back-to-front). The dispatcher walks
/// last-to-first so the topmost-painted surface wins on overlap —
/// same semantics as [`ModalStack`].
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollSurface {
    pub id: WidgetId,
    pub bounds: crate::event::Rect,
    /// Optional scrollbar geometry. When present, [`dispatch_click`]
    /// can route thumb clicks (auto-start drag) and track clicks
    /// (page up/down) without per-backend code.
    pub scrollbar: Option<SurfaceScrollbar>,
}

/// Translate a raw scroll-wheel event into a `Vec<UiEvent>`, consulting
/// the modal stack first, then the registered scroll surfaces.
///
/// # Scroll delta sign convention
///
/// Positive `delta.y` = scroll content **up** (toward the top of the
/// document). Backends normalise their native direction before
/// constructing [`ScrollDelta`] — see [`ScrollDelta`] for the
/// canonical definition. Compose helpers ([`TreeController`],
/// [`SidebarSystem`]) consume the delta directly without per-call-site
/// negation.
///
/// # Returns
///
/// Three cases:
///
/// 1. **Scroll landed inside an open modal.** Emits
///    `[UiEvent::Scroll { widget: Some(modal_id), delta, position }]`.
///    The app routes to the modal's inner scroll handler.
/// 2. **Modal(s) open but scroll was outside them.** Emits nothing —
///    scroll behind an open modal is swallowed (you can't scroll the
///    editor while a dialog is up).
/// 3. **No modal open.** Hit-tests `scroll_surfaces` last-to-first
///    (topmost-painted wins). If a surface matches, emits
///    `[UiEvent::Scroll { widget: Some(surface_id), delta, position }]`.
///    If no surface matches, emits
///    `[UiEvent::Scroll { widget: None, delta, position }]` for
///    base-layer handling.
pub fn dispatch_scroll(
    stack: &ModalStack,
    scroll_surfaces: &[ScrollSurface],
    position: Point,
    delta: ScrollDelta,
) -> Vec<UiEvent> {
    // Case 1: scroll inside an open modal.
    if let Some(widget_id) = stack.hit_test(position) {
        return vec![UiEvent::Scroll {
            widget: Some(widget_id.clone()),
            delta,
            position,
        }];
    }

    // Case 2: modal(s) open but scroll was outside → swallow.
    if !stack.is_empty() {
        return Vec::new();
    }

    // Case 3: no modals. Hit-test scroll surfaces (last-to-first).
    for surface in scroll_surfaces.iter().rev() {
        if position.x >= surface.bounds.x
            && position.x < surface.bounds.x + surface.bounds.width
            && position.y >= surface.bounds.y
            && position.y < surface.bounds.y + surface.bounds.height
        {
            return vec![UiEvent::Scroll {
                widget: Some(surface.id.clone()),
                delta,
                position,
            }];
        }
    }

    // No surface matched — base-layer event.
    vec![UiEvent::Scroll {
        widget: None,
        delta,
        position,
    }]
}

// ─── Click dispatch with scrollbar awareness ──────────────────────────────

/// Scrollbar geometry for a [`ScrollSurface`]. Registered during paint
/// so [`dispatch_click`] can route scrollbar clicks (thumb drag, track
/// page) without per-backend code. Supports both vertical and
/// horizontal scrollbars via the `axis` field.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceScrollbar {
    /// Vertical or horizontal. Determines which coordinate axis
    /// `dispatch_click` uses for grab offset, track-click paging,
    /// and which `DragTarget` variant is created.
    pub axis: ScrollAxis,
    /// Full scrollbar track bounds (gutter area).
    pub track_bounds: crate::event::Rect,
    /// Current thumb bounds within the track.
    pub thumb_bounds: crate::event::Rect,
    /// Total number of scrollable items (lines, rows, cols, etc.).
    pub total_items: usize,
    /// Number of items visible in the viewport.
    pub visible_items: usize,
    /// Current scroll offset (index of the first visible item).
    pub scroll_offset: usize,
    /// When true, offset 0 means "at the bottom" (scrollback style).
    /// Propagated to the `DragTarget`'s `inverted` field on thumb
    /// click and used to flip track-click page direction.
    pub inverted: bool,
}

/// Translate a raw mouse-down event, consulting the modal stack first,
/// then the registered scroll surfaces (including their scrollbar
/// regions) and text regions. Supersedes [`dispatch_mouse_down`] for
/// consumers that register scroll surfaces and/or text regions.
///
/// # Returns
///
/// Priority order:
///
/// 1. **Click inside an open modal** → `MouseDown { widget: Some(modal_id) }`.
/// 2. **Click outside every modal, but modal open** → dismiss topmost
///    (same as [`dispatch_mouse_down`]).
/// 3. **Click on a scroll surface's scrollbar thumb** → starts a
///    [`DragTarget::ScrollbarY`] or [`DragTarget::ScrollbarX`] drag
///    (depending on `SurfaceScrollbar::axis`), emits
///    `MouseDown { widget: Some(surface_id) }`.
/// 4. **Click on a scroll surface's scrollbar track** (above or below
///    thumb) → emits `ScrollOffsetChanged { widget, new_offset }`
///    with the offset paged up or down by `visible_items`.
/// 5. **Click inside a registered [`TextRegion`]** → starts a
///    [`DragTarget::TextSelection`] drag, emits
///    `MouseDown { widget: Some(region_id) }`. Text regions take
///    priority over scroll surface bodies (not scrollbars).
/// 6. **Click on a scroll surface body** (not scrollbar, not text
///    region) → `MouseDown { widget: Some(surface_id) }`.
/// 7. **No surface or region matched** → `MouseDown { widget: None }`.
pub fn dispatch_click(
    stack: &ModalStack,
    scroll_surfaces: &[ScrollSurface],
    text_regions: &[TextRegion],
    drag: &mut DragState,
    position: Point,
    button: MouseButton,
    modifiers: Modifiers,
) -> Vec<UiEvent> {
    // Case 1: click inside an open modal.
    if let Some(widget_id) = stack.hit_test(position) {
        return vec![UiEvent::MouseDown {
            widget: Some(widget_id.clone()),
            button,
            position,
            modifiers,
        }];
    }

    // Case 2: modal open but click outside → dismiss topmost.
    if let Some(top) = stack.top() {
        return vec![
            UiEvent::MouseDown {
                widget: None,
                button,
                position,
                modifiers,
            },
            UiEvent::Palette(top.id.clone(), PaletteEvent::Closed),
        ];
    }

    // Case 3–5: no modals. Hit-test scroll surfaces (last-to-first).
    for surface in scroll_surfaces.iter().rev() {
        let in_bounds = position.x >= surface.bounds.x
            && position.x < surface.bounds.x + surface.bounds.width
            && position.y >= surface.bounds.y
            && position.y < surface.bounds.y + surface.bounds.height;
        if !in_bounds {
            continue;
        }

        // Check scrollbar regions first (if present).
        if let Some(ref sb) = surface.scrollbar {
            let in_track = position.x >= sb.track_bounds.x
                && position.x < sb.track_bounds.x + sb.track_bounds.width
                && position.y >= sb.track_bounds.y
                && position.y < sb.track_bounds.y + sb.track_bounds.height;

            if in_track {
                let in_thumb = position.x >= sb.thumb_bounds.x
                    && position.x < sb.thumb_bounds.x + sb.thumb_bounds.width
                    && position.y >= sb.thumb_bounds.y
                    && position.y < sb.thumb_bounds.y + sb.thumb_bounds.height;

                if in_thumb {
                    let max_scroll = sb.total_items.saturating_sub(sb.visible_items);
                    match sb.axis {
                        ScrollAxis::Vertical => {
                            let grab_offset = position.y - sb.thumb_bounds.y;
                            drag.begin(DragTarget::ScrollbarY {
                                widget: surface.id.clone(),
                                track_start: sb.track_bounds.y,
                                track_length: sb.track_bounds.height,
                                thumb_length: sb.thumb_bounds.height,
                                max_scroll,
                                grab_offset,
                                inverted: sb.inverted,
                            });
                        }
                        ScrollAxis::Horizontal => {
                            let grab_offset = position.x - sb.thumb_bounds.x;
                            drag.begin(DragTarget::ScrollbarX {
                                widget: surface.id.clone(),
                                track_start: sb.track_bounds.x,
                                track_length: sb.track_bounds.width,
                                thumb_length: sb.thumb_bounds.width,
                                max_scroll,
                                grab_offset,
                                inverted: sb.inverted,
                            });
                        }
                    }
                    return vec![UiEvent::MouseDown {
                        widget: Some(surface.id.clone()),
                        button,
                        position,
                        modifiers,
                    }];
                }

                // Track click → page forward or back. For vertical,
                // "before" = above thumb; for horizontal, "before" =
                // left of thumb. When inverted, the direction flips.
                let max_offset = sb.total_items.saturating_sub(sb.visible_items);
                let before_thumb = match sb.axis {
                    ScrollAxis::Vertical => position.y < sb.thumb_bounds.y,
                    ScrollAxis::Horizontal => position.x < sb.thumb_bounds.x,
                };
                let page_back = if sb.inverted {
                    !before_thumb
                } else {
                    before_thumb
                };
                let new_offset = if page_back {
                    sb.scroll_offset.saturating_sub(sb.visible_items)
                } else {
                    (sb.scroll_offset + sb.visible_items).min(max_offset)
                };
                return vec![UiEvent::ScrollOffsetChanged {
                    widget: surface.id.clone(),
                    new_offset,
                }];
            }
        }

        // Case 5: Check text regions before falling through to body click.
        // Walk last-to-first so the topmost-painted region wins on overlap.
        for tr in text_regions.iter().rev() {
            if tr.bounds.contains(position) {
                drag.begin(DragTarget::TextSelection {
                    region: tr.id.clone(),
                    anchor: position,
                });
                return vec![UiEvent::MouseDown {
                    widget: Some(tr.id.clone()),
                    button,
                    position,
                    modifiers,
                }];
            }
        }

        // Case 6: body click (not on scrollbar, not on text region).
        return vec![UiEvent::MouseDown {
            widget: Some(surface.id.clone()),
            button,
            position,
            modifiers,
        }];
    }

    // No scroll surface hit. Still check standalone text regions
    // (not nested inside any scroll surface).
    for tr in text_regions.iter().rev() {
        if tr.bounds.contains(position) {
            drag.begin(DragTarget::TextSelection {
                region: tr.id.clone(),
                anchor: position,
            });
            return vec![UiEvent::MouseDown {
                widget: Some(tr.id.clone()),
                button,
                position,
                modifiers,
            }];
        }
    }

    // Case 7: no surface or region matched.
    vec![UiEvent::MouseDown {
        widget: None,
        button,
        position,
        modifiers,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect;
    use crate::types::WidgetId;

    fn id(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    fn pt(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn click_inside_modal_emits_single_mousedown_with_widget() {
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(10.0, 10.0, 100.0, 100.0));
        let events = dispatch_mouse_down(
            &stack,
            pt(50.0, 50.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::MouseDown { widget, button, .. } => {
                assert_eq!(widget.as_ref().unwrap(), &id("palette"));
                assert_eq!(*button, MouseButton::Left);
            }
            _ => panic!("expected MouseDown, got {:?}", events[0]),
        }
    }

    #[test]
    fn click_outside_open_modal_dismisses_topmost() {
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(10.0, 10.0, 50.0, 50.0));
        // Click well outside the palette's bounds.
        let events = dispatch_mouse_down(
            &stack,
            pt(500.0, 500.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 2);
        // First event: MouseDown with widget None (backdrop click).
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown { widget: None, .. }
        ));
        // Second event: palette Closed.
        match &events[1] {
            UiEvent::Palette(wid, PaletteEvent::Closed) => {
                assert_eq!(wid, &id("palette"));
            }
            _ => panic!("expected Palette::Closed, got {:?}", events[1]),
        }
    }

    #[test]
    fn click_when_no_modal_open_emits_single_mousedown_with_no_widget() {
        let stack = ModalStack::new();
        let events = dispatch_mouse_down(
            &stack,
            pt(100.0, 100.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown { widget: None, .. }
        ));
    }

    #[test]
    fn stacked_modals_click_inside_top_targets_top() {
        // Palette open; then a dialog on top of it. Click in the
        // overlap region should target the dialog, not the palette.
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(0.0, 0.0, 200.0, 200.0));
        stack.push(id("dialog"), rect(50.0, 50.0, 100.0, 100.0));
        let events = dispatch_mouse_down(
            &stack,
            pt(100.0, 100.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        match &events[0] {
            UiEvent::MouseDown { widget, .. } => {
                assert_eq!(widget.as_ref().unwrap(), &id("dialog"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn stacked_modals_click_inside_lower_targets_lower() {
        // Click lands in palette's bounds but outside the dialog on top.
        // The lower-modal id is what should be reported — the click is
        // still inside a modal, so no backdrop-dismiss.
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(0.0, 0.0, 200.0, 200.0));
        stack.push(id("dialog"), rect(50.0, 50.0, 100.0, 100.0));
        let events = dispatch_mouse_down(
            &stack,
            pt(10.0, 10.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::MouseDown { widget, .. } => {
                assert_eq!(widget.as_ref().unwrap(), &id("palette"));
            }
            _ => panic!(),
        }
    }

    // ── Drag tests ────────────────────────────────────────────────────

    fn buttons_mask_left() -> crate::event::ButtonMask {
        crate::event::ButtonMask {
            left: true,
            ..Default::default()
        }
    }

    #[test]
    fn drag_state_begin_and_end() {
        let mut drag = DragState::new();
        assert!(!drag.is_active());
        drag.begin(DragTarget::ScrollbarY {
            widget: id("picker"),
            track_start: 100.0,
            track_length: 200.0,
            thumb_length: 16.0,
            max_scroll: 40,
            grab_offset: 0.0,
            inverted: false,
        });
        assert!(drag.is_active());
        match drag.target().unwrap() {
            DragTarget::ScrollbarY { widget, .. } => assert_eq!(widget, &id("picker")),
            DragTarget::ScrollbarX { .. } => panic!("expected ScrollbarY"),
            DragTarget::TextSelection { .. } => panic!("expected ScrollbarY"),
        }
        drag.end();
        assert!(!drag.is_active());
        assert!(drag.target().is_none());
    }

    #[test]
    fn dispatch_mouse_drag_without_drag_emits_only_moved() {
        let drag = DragState::new();
        let events = dispatch_mouse_drag(&drag, pt(50.0, 50.0), buttons_mask_left());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], UiEvent::MouseMoved { .. }));
    }

    #[test]
    fn dispatch_mouse_drag_with_scrollbar_emits_scroll_offset_changed() {
        // Track 80 units from y=100; 100 items, viewport shows 20.
        // thumb_ratio = 20/100 = 0.2 → thumb_length = 16
        // effective_track = 64; max_scroll = 80
        // Mouse at y=100+32 (halfway through effective_track) → offset 40.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("picker"),
            track_start: 100.0,
            track_length: 80.0,
            thumb_length: 16.0,
            max_scroll: 80,
            grab_offset: 0.0,
            inverted: false,
        });
        let events = dispatch_mouse_drag(&drag, pt(500.0, 132.0), buttons_mask_left());
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], UiEvent::MouseMoved { .. }));
        match &events[1] {
            UiEvent::ScrollOffsetChanged { widget, new_offset } => {
                assert_eq!(widget, &id("picker"));
                assert_eq!(*new_offset, 40);
            }
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_mouse_drag_clamps_above_and_below_track() {
        // Same geometry as above: max_scroll = 80, effective_track = 64.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("p"),
            track_start: 100.0,
            track_length: 80.0,
            thumb_length: 16.0,
            max_scroll: 80,
            grab_offset: 0.0,
            inverted: false,
        });
        // Above track: offset = 0.
        let events = dispatch_mouse_drag(&drag, pt(0.0, 50.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => {
                assert_eq!(*new_offset, 0);
            }
            _ => panic!(),
        }
        // Below effective track: clamped to max_scroll = 80.
        let events = dispatch_mouse_drag(&drag, pt(0.0, 500.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => {
                assert_eq!(*new_offset, 80);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn dispatch_mouse_drag_with_zero_track_does_not_crash_or_emit() {
        // Pathological input — no track. Should emit only MouseMoved.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("p"),
            track_start: 0.0,
            track_length: 0.0,
            thumb_length: 0.0,
            max_scroll: 90,
            grab_offset: 0.0,
            inverted: false,
        });
        let events = dispatch_mouse_drag(&drag, pt(0.0, 0.0), buttons_mask_left());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], UiEvent::MouseMoved { .. }));
    }

    #[test]
    fn dispatch_mouse_up_clears_drag_state() {
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("p"),
            track_start: 0.0,
            track_length: 10.0,
            thumb_length: 5.0,
            max_scroll: 10,
            grab_offset: 0.0,
            inverted: false,
        });
        let stack = ModalStack::new();
        let events = dispatch_mouse_up(&stack, &mut drag, pt(5.0, 5.0), MouseButton::Left);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], UiEvent::MouseUp { .. }));
        assert!(!drag.is_active());
    }

    #[test]
    fn dispatch_mouse_drag_with_grab_offset_keeps_thumb_relative_to_cursor() {
        // Same geometry as the basic test: track 80 from y=100, viewport 20,
        // total 100 → effective_track 64, max_scroll 80. With grab_offset=8
        // the cursor at y=140 sits 8 units below the thumb's top, so the
        // math sees effective y=132 → rel=32/64=0.5 → offset 40 (matches
        // the no-grab base test where cursor was at y=132 directly).
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("p"),
            track_start: 100.0,
            track_length: 80.0,
            thumb_length: 16.0,
            max_scroll: 80,
            grab_offset: 8.0,
            inverted: false,
        });
        let events = dispatch_mouse_drag(&drag, pt(500.0, 140.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => assert_eq!(*new_offset, 40),
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_mouse_drag_horizontal_scrollbar() {
        // Mirror of the basic Y-axis test on the X axis. Track 80 from x=100;
        // 100 cols total, viewport shows 20. thumb_length=16, effective=64,
        // max_scroll=80. Cursor at x=132 (halfway through effective) → 40.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarX {
            widget: id("editor:hsb"),
            track_start: 100.0,
            track_length: 80.0,
            thumb_length: 16.0,
            max_scroll: 80,
            grab_offset: 0.0,
            inverted: false,
        });
        let events = dispatch_mouse_drag(&drag, pt(132.0, 500.0), buttons_mask_left());
        assert_eq!(events.len(), 2);
        match &events[1] {
            UiEvent::ScrollOffsetChanged { widget, new_offset } => {
                assert_eq!(widget, &id("editor:hsb"));
                assert_eq!(*new_offset, 40);
            }
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_mouse_drag_horizontal_grab_offset_preserves_relative() {
        // Same geometry as horizontal_scrollbar test; with grab_offset=8 the
        // cursor at x=140 maps to effective x=132 → 40.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarX {
            widget: id("editor:hsb"),
            track_start: 100.0,
            track_length: 80.0,
            thumb_length: 16.0,
            max_scroll: 80,
            grab_offset: 8.0,
            inverted: false,
        });
        let events = dispatch_mouse_drag(&drag, pt(140.0, 500.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => assert_eq!(*new_offset, 40),
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    #[test]
    fn painted_thumb_length_controls_effective_track() {
        // Track 400px, 20 visible out of 4000 total.
        // Painted thumb = 20px → effective = 380; max_scroll = 3980.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarX {
            widget: id("editor:hsb"),
            track_start: 100.0,
            track_length: 400.0,
            thumb_length: 20.0,
            max_scroll: 3980,
            grab_offset: 0.0,
            inverted: false,
        });
        // Cursor at x = 100 + 380 (end of effective track) → ratio 1.0 → 3980
        let events = dispatch_mouse_drag(&drag, pt(480.0, 0.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => assert_eq!(*new_offset, 3980),
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
        // Halfway: x = 100 + 190 = 290 → ratio 0.5 → 1990
        let events = dispatch_mouse_drag(&drag, pt(290.0, 0.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => assert_eq!(*new_offset, 1990),
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    // ── Round-trip: fit_thumb ↔ dispatch agreement ─────────────────
    //
    // The rasteriser calls `fit_thumb` to position the thumb. The
    // dispatcher inverts that position back to a scroll offset.
    // These tests prove the two agree for every scroll offset
    // including 0 and max_scroll.

    fn assert_round_trip(
        label: &str,
        track_start: f32,
        track_length: f32,
        total: usize,
        visible: usize,
        min_thumb: f32,
        scroll: usize,
    ) {
        use crate::primitives::scrollbar::fit_thumb;
        let max_scroll = total.saturating_sub(visible);
        let (thumb_start, thumb_len) = fit_thumb(
            scroll as f32,
            total as f32,
            visible as f32,
            track_length,
            min_thumb,
        );
        let grab = thumb_len / 2.0;
        let cursor_x = track_start + thumb_start + grab;
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarX {
            widget: id("sb"),
            track_start,
            track_length,
            thumb_length: thumb_len,
            max_scroll,
            grab_offset: grab,
            inverted: false,
        });
        let events = dispatch_mouse_drag(&drag, pt(cursor_x, 0.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => {
                assert_eq!(
                    *new_offset, scroll,
                    "{label}: expected offset {scroll}, got {new_offset}"
                );
            }
            other => panic!("{label}: expected ScrollOffsetChanged, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_vimcode_numbers_at_max() {
        assert_round_trip("3609 max", 50.0, 800.0, 3609, 137, 20.0, 3609 - 137);
    }

    #[test]
    fn round_trip_vimcode_numbers_at_zero() {
        assert_round_trip("3609 zero", 50.0, 800.0, 3609, 137, 20.0, 0);
    }

    #[test]
    fn round_trip_vimcode_numbers_at_midpoint() {
        assert_round_trip("3609 mid", 50.0, 800.0, 3609, 137, 20.0, 1736);
    }

    #[test]
    fn round_trip_small_content_at_max() {
        assert_round_trip("small max", 0.0, 100.0, 200, 50, 10.0, 150);
    }

    #[test]
    fn round_trip_large_min_thumb_at_max() {
        // thumb dominates: min_thumb = 40 on 100px track → effective = 60
        assert_round_trip("big thumb max", 0.0, 100.0, 5000, 50, 40.0, 4950);
    }

    #[test]
    fn round_trip_cursor_past_track_end_reaches_max() {
        use crate::primitives::scrollbar::fit_thumb;
        let track_start = 50.0;
        let track_length = 800.0;
        let total = 3609_usize;
        let visible = 137_usize;
        let max_scroll = total - visible;
        let (_, thumb_len) = fit_thumb(0.0, total as f32, visible as f32, track_length, 20.0);
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarX {
            widget: id("sb"),
            track_start,
            track_length,
            thumb_length: thumb_len,
            max_scroll,
            grab_offset: 0.0,
            inverted: false,
        });
        // Cursor WAY past track end — must still clamp to max_scroll
        let events = dispatch_mouse_drag(
            &drag,
            pt(track_start + track_length + 200.0, 0.0),
            buttons_mask_left(),
        );
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => {
                assert_eq!(*new_offset, max_scroll);
            }
            other => panic!("expected ScrollOffsetChanged, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_mouse_up_carries_modal_widget_if_over() {
        let mut stack = ModalStack::new();
        stack.push(id("palette"), rect(0.0, 0.0, 100.0, 100.0));
        let mut drag = DragState::new();
        let events = dispatch_mouse_up(&stack, &mut drag, pt(50.0, 50.0), MouseButton::Left);
        match &events[0] {
            UiEvent::MouseUp { widget, .. } => {
                assert_eq!(widget.as_ref().unwrap(), &id("palette"));
            }
            _ => panic!(),
        }
    }

    // ── Scroll dispatch tests ─────────────────────────────────────────

    fn delta(dy: f32) -> ScrollDelta {
        ScrollDelta::new(0.0, dy)
    }

    #[test]
    fn scroll_inside_modal_routes_to_modal() {
        let mut stack = ModalStack::new();
        stack.push(id("picker"), rect(10.0, 10.0, 100.0, 100.0));
        let events = dispatch_scroll(&stack, &[], pt(50.0, 50.0), delta(-3.0));
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::Scroll {
                widget, delta: d, ..
            } => {
                assert_eq!(widget.as_ref().unwrap(), &id("picker"));
                assert_eq!(d.y, -3.0);
            }
            other => panic!("expected Scroll, got {:?}", other),
        }
    }

    #[test]
    fn scroll_outside_open_modal_is_swallowed() {
        let mut stack = ModalStack::new();
        stack.push(id("dialog"), rect(10.0, 10.0, 50.0, 50.0));
        let events = dispatch_scroll(&stack, &[], pt(500.0, 500.0), delta(-1.0));
        assert!(events.is_empty(), "scroll behind modal should be swallowed");
    }

    #[test]
    fn scroll_routes_to_topmost_surface() {
        let stack = ModalStack::new();
        let surfaces = vec![
            ScrollSurface {
                id: id("editor"),
                bounds: rect(0.0, 0.0, 200.0, 200.0),
                scrollbar: None,
            },
            ScrollSurface {
                id: id("sidebar"),
                bounds: rect(0.0, 0.0, 50.0, 200.0),
                scrollbar: None,
            },
        ];
        // Point (25, 100) is inside both surfaces. Sidebar was pushed
        // last (painted on top) → wins.
        let events = dispatch_scroll(&stack, &surfaces, pt(25.0, 100.0), delta(-1.0));
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::Scroll { widget, .. } => {
                assert_eq!(widget.as_ref().unwrap(), &id("sidebar"));
            }
            other => panic!("expected Scroll, got {:?}", other),
        }
    }

    #[test]
    fn scroll_on_non_overlapping_surface() {
        let stack = ModalStack::new();
        let surfaces = vec![
            ScrollSurface {
                id: id("editor"),
                bounds: rect(50.0, 0.0, 150.0, 200.0),
                scrollbar: None,
            },
            ScrollSurface {
                id: id("sidebar"),
                bounds: rect(0.0, 0.0, 50.0, 200.0),
                scrollbar: None,
            },
        ];
        // Point (100, 100) is inside editor only.
        let events = dispatch_scroll(&stack, &surfaces, pt(100.0, 100.0), delta(2.0));
        match &events[0] {
            UiEvent::Scroll { widget, .. } => {
                assert_eq!(widget.as_ref().unwrap(), &id("editor"));
            }
            other => panic!("expected Scroll, got {:?}", other),
        }
    }

    #[test]
    fn scroll_with_no_surfaces_emits_no_widget() {
        let stack = ModalStack::new();
        let events = dispatch_scroll(&stack, &[], pt(50.0, 50.0), delta(-1.0));
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::Scroll { widget, .. } => {
                assert!(widget.is_none());
            }
            other => panic!("expected Scroll, got {:?}", other),
        }
    }

    #[test]
    fn scroll_outside_all_surfaces_emits_no_widget() {
        let stack = ModalStack::new();
        let surfaces = vec![ScrollSurface {
            id: id("panel"),
            bounds: rect(0.0, 0.0, 50.0, 50.0),
            scrollbar: None,
        }];
        let events = dispatch_scroll(&stack, &surfaces, pt(100.0, 100.0), delta(-1.0));
        match &events[0] {
            UiEvent::Scroll { widget, .. } => {
                assert!(widget.is_none());
            }
            other => panic!("expected Scroll, got {:?}", other),
        }
    }

    // ── dispatch_click tests ──────────────────────────────────────────

    fn surface_with_scrollbar() -> ScrollSurface {
        ScrollSurface {
            id: id("log"),
            bounds: rect(0.0, 0.0, 40.0, 30.0),
            scrollbar: Some(SurfaceScrollbar {
                axis: ScrollAxis::Vertical,
                track_bounds: rect(39.0, 0.0, 1.0, 30.0),
                thumb_bounds: rect(39.0, 20.0, 1.0, 8.0),
                total_items: 100,
                visible_items: 30,
                scroll_offset: 60,
                inverted: false,
            }),
        }
    }

    #[test]
    fn click_thumb_starts_drag() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_scrollbar()];
        let mut drag = DragState::new();
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(39.5, 22.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown {
                widget: Some(w),
                ..
            } if w.as_str() == "log"
        ));
        assert!(drag.is_active());
        match drag.target().unwrap() {
            DragTarget::ScrollbarY {
                widget, max_scroll, ..
            } => {
                assert_eq!(widget.as_str(), "log");
                assert_eq!(*max_scroll, 70);
            }
            other => panic!("expected ScrollbarY, got {:?}", other),
        }
    }

    #[test]
    fn click_track_before_pages_up() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_scrollbar()];
        let mut drag = DragState::new();
        // Click above the thumb (thumb starts at y=20).
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(39.5, 5.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::ScrollOffsetChanged { widget, new_offset } => {
                assert_eq!(widget.as_str(), "log");
                // scroll_offset=60, page up by visible_items=30 → 30.
                assert_eq!(*new_offset, 30);
            }
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
        assert!(!drag.is_active());
    }

    #[test]
    fn click_track_after_pages_down() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_scrollbar()];
        let mut drag = DragState::new();
        // Click below the thumb (thumb ends at y=28).
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(39.5, 29.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::ScrollOffsetChanged { widget, new_offset } => {
                assert_eq!(widget.as_str(), "log");
                // scroll_offset=60, page down by 30, max=70 → 70.
                assert_eq!(*new_offset, 70);
            }
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    #[test]
    fn click_body_not_scrollbar_emits_mousedown() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_scrollbar()];
        let mut drag = DragState::new();
        // Click in the body area (not on the scrollbar gutter).
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(10.0, 10.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown {
                widget: Some(w),
                ..
            } if w.as_str() == "log"
        ));
        assert!(!drag.is_active());
    }

    #[test]
    fn click_no_surfaces_emits_no_widget() {
        let stack = ModalStack::new();
        let mut drag = DragState::new();
        let events = dispatch_click(
            &stack,
            &[],
            &[],
            &mut drag,
            pt(50.0, 50.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown { widget: None, .. }
        ));
    }

    #[test]
    fn click_modal_takes_priority_over_surfaces() {
        let mut stack = ModalStack::new();
        stack.push(id("dialog"), rect(0.0, 0.0, 100.0, 100.0));
        let surfaces = vec![surface_with_scrollbar()];
        let mut drag = DragState::new();
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(39.5, 22.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        // Should route to the modal, not the scrollbar.
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown {
                widget: Some(w),
                ..
            } if w.as_str() == "dialog"
        ));
        assert!(!drag.is_active());
    }

    // ── Inverted scrollbar tests ─────────────────────────────────────

    #[test]
    fn inverted_drag_flips_offset() {
        // Same geometry as the basic drag test: track 80 from y=100,
        // viewport 20, total 100. effective_track=64, max_scroll=80.
        // Normal: cursor at y=132 (halfway) → offset 40.
        // Inverted: same position → offset 80 - 40 = 40… wait:
        //   rel = 32/64 = 0.5, inverted → 1.0 - 0.5 = 0.5 → 40.
        // To see the flip, use the track top: y=100 → rel=0, inv→1.0 → 80.
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("term"),
            track_start: 100.0,
            track_length: 80.0,
            thumb_length: 16.0,
            max_scroll: 80,
            grab_offset: 0.0,
            inverted: true,
        });
        // Cursor at track top → normal offset 0, inverted offset 80.
        let events = dispatch_mouse_drag(&drag, pt(0.0, 100.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => assert_eq!(*new_offset, 80),
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
        // Cursor at track bottom → normal offset 80, inverted offset 0.
        let events = dispatch_mouse_drag(&drag, pt(0.0, 500.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => assert_eq!(*new_offset, 0),
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    #[test]
    fn inverted_drag_halfway_matches_non_inverted() {
        // At the exact midpoint, both normal and inverted produce the
        // same offset (symmetry check).
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: id("term"),
            track_start: 100.0,
            track_length: 80.0,
            thumb_length: 16.0,
            max_scroll: 80,
            grab_offset: 0.0,
            inverted: true,
        });
        let events = dispatch_mouse_drag(&drag, pt(0.0, 132.0), buttons_mask_left());
        match &events[1] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => assert_eq!(*new_offset, 40),
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    fn surface_with_inverted_scrollbar() -> ScrollSurface {
        ScrollSurface {
            id: id("term"),
            bounds: rect(0.0, 0.0, 40.0, 30.0),
            scrollbar: Some(SurfaceScrollbar {
                axis: ScrollAxis::Vertical,
                track_bounds: rect(39.0, 0.0, 1.0, 30.0),
                thumb_bounds: rect(39.0, 20.0, 1.0, 8.0),
                total_items: 100,
                visible_items: 30,
                scroll_offset: 60,
                inverted: true,
            }),
        }
    }

    #[test]
    fn inverted_thumb_click_propagates_flag() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_inverted_scrollbar()];
        let mut drag = DragState::new();
        dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(39.5, 22.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert!(drag.is_active());
        match drag.target().unwrap() {
            DragTarget::ScrollbarY { inverted, .. } => assert!(*inverted),
            DragTarget::ScrollbarX { .. } | DragTarget::TextSelection { .. } => {
                panic!("expected ScrollbarY")
            }
        }
    }

    #[test]
    fn inverted_track_click_flips_page_direction() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_inverted_scrollbar()];
        let mut drag = DragState::new();
        // Click ABOVE the thumb (y=5 < thumb y=20).
        // Normal: page up → offset 60 - 30 = 30.
        // Inverted: above thumb pages FORWARD → offset 60 + 30 = 70 (capped at 70).
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(39.5, 5.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        match &events[0] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => assert_eq!(*new_offset, 70),
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }

        // Click BELOW the thumb (y=29 > thumb bottom y=28).
        // Normal: page down → offset 60 + 30 = 70.
        // Inverted: below thumb pages BACK → offset 60 - 30 = 30.
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(39.5, 29.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        match &events[0] {
            UiEvent::ScrollOffsetChanged { new_offset, .. } => assert_eq!(*new_offset, 30),
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    // ── Horizontal scroll surface tests ──────────────────────────────

    fn surface_with_h_scrollbar() -> ScrollSurface {
        // Horizontal scrollbar at the bottom of a 400×300 surface.
        // Track spans x=0..400 at y=296, height=4.
        // Thumb at x=50..90 (40px wide).
        // 2000 total cols, 200 visible, offset 250.
        ScrollSurface {
            id: id("editor"),
            bounds: rect(0.0, 0.0, 400.0, 300.0),
            scrollbar: Some(SurfaceScrollbar {
                axis: ScrollAxis::Horizontal,
                track_bounds: rect(0.0, 296.0, 400.0, 4.0),
                thumb_bounds: rect(50.0, 296.0, 40.0, 4.0),
                total_items: 2000,
                visible_items: 200,
                scroll_offset: 250,
                inverted: false,
            }),
        }
    }

    #[test]
    fn h_scrollbar_thumb_click_starts_scrollbar_x_drag() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_h_scrollbar()];
        let mut drag = DragState::new();
        // Click on the thumb at x=60, y=297
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(60.0, 297.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], UiEvent::MouseDown { widget: Some(w), .. } if w.as_str() == "editor")
        );
        assert!(drag.is_active());
        match drag.target().unwrap() {
            DragTarget::ScrollbarX {
                widget,
                track_start,
                track_length,
                thumb_length,
                max_scroll,
                grab_offset,
                ..
            } => {
                assert_eq!(widget.as_str(), "editor");
                assert_eq!(*track_start, 0.0);
                assert_eq!(*track_length, 400.0);
                assert_eq!(*thumb_length, 40.0);
                assert_eq!(*max_scroll, 1800);
                assert!(*grab_offset > 9.0 && *grab_offset < 11.0); // 60.0 - 50.0 = 10.0
            }
            DragTarget::ScrollbarY { .. } | DragTarget::TextSelection { .. } => {
                panic!("expected ScrollbarX")
            }
        }
    }

    #[test]
    fn h_scrollbar_track_click_left_of_thumb_pages_back() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_h_scrollbar()];
        let mut drag = DragState::new();
        // Click left of thumb (x=10 < thumb x=50)
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(10.0, 297.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        match &events[0] {
            UiEvent::ScrollOffsetChanged { widget, new_offset } => {
                assert_eq!(widget.as_str(), "editor");
                // page back: 250 - 200 = 50
                assert_eq!(*new_offset, 50);
            }
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    #[test]
    fn h_scrollbar_track_click_right_of_thumb_pages_forward() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_h_scrollbar()];
        let mut drag = DragState::new();
        // Click right of thumb (x=200 > thumb right x=90)
        let events = dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(200.0, 297.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        match &events[0] {
            UiEvent::ScrollOffsetChanged { widget, new_offset } => {
                assert_eq!(widget.as_str(), "editor");
                // page forward: 250 + 200 = 450
                assert_eq!(*new_offset, 450);
            }
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    #[test]
    fn h_scrollbar_drag_produces_scroll_offset() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_h_scrollbar()];
        let mut drag = DragState::new();
        // Click thumb to start drag
        dispatch_click(
            &stack,
            &surfaces,
            &[],
            &mut drag,
            pt(60.0, 297.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert!(drag.is_active());
        // Drag cursor to x=370 (near track end)
        let events = dispatch_mouse_drag(&drag, pt(370.0, 297.0), buttons_mask_left());
        assert_eq!(events.len(), 2);
        match &events[1] {
            UiEvent::ScrollOffsetChanged { widget, new_offset } => {
                assert_eq!(widget.as_str(), "editor");
                assert_eq!(*new_offset, 1800); // max_scroll
            }
            other => panic!("expected ScrollOffsetChanged, got {:?}", other),
        }
    }

    // ── TextRegion / text-selection tests ─────────────────────────────

    #[test]
    fn click_text_region_starts_text_selection_drag() {
        let stack = ModalStack::new();
        let mut drag = DragState::new();
        let text_regions = vec![TextRegion {
            id: id("log:body"),
            bounds: rect(0.0, 0.0, 40.0, 30.0),
        }];
        let events = dispatch_click(
            &stack,
            &[],
            &text_regions,
            &mut drag,
            pt(10.0, 5.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown {
                widget: Some(w), ..
            } if w.as_str() == "log:body"
        ));
        assert!(drag.is_active());
        match drag.target().unwrap() {
            DragTarget::TextSelection { region, anchor } => {
                assert_eq!(region.as_str(), "log:body");
                assert_eq!(*anchor, pt(10.0, 5.0));
            }
            other => panic!("expected TextSelection, got {:?}", other),
        }
    }

    #[test]
    fn scrollbar_wins_over_text_region() {
        let stack = ModalStack::new();
        let surfaces = vec![surface_with_scrollbar()];
        let text_regions = vec![TextRegion {
            id: id("log:body"),
            // Covers the whole surface including the scrollbar column.
            bounds: rect(0.0, 0.0, 40.0, 30.0),
        }];
        let mut drag = DragState::new();
        // Click on scrollbar thumb (x=39.5, y=22) — scrollbar must win.
        dispatch_click(
            &stack,
            &surfaces,
            &text_regions,
            &mut drag,
            pt(39.5, 22.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert!(matches!(
            drag.target().unwrap(),
            DragTarget::ScrollbarY { .. }
        ));
    }

    #[test]
    fn text_region_wins_over_surface_body() {
        let stack = ModalStack::new();
        let surfaces = vec![ScrollSurface {
            id: id("log"),
            bounds: rect(0.0, 0.0, 40.0, 30.0),
            scrollbar: None,
        }];
        let text_regions = vec![TextRegion {
            id: id("log:body"),
            bounds: rect(0.0, 0.0, 40.0, 30.0),
        }];
        let mut drag = DragState::new();
        let events = dispatch_click(
            &stack,
            &surfaces,
            &text_regions,
            &mut drag,
            pt(10.0, 5.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown { widget: Some(w), .. } if w.as_str() == "log:body"
        ));
        assert!(matches!(
            drag.target().unwrap(),
            DragTarget::TextSelection { .. }
        ));
    }

    #[test]
    fn modal_wins_over_text_region() {
        let mut stack = ModalStack::new();
        stack.push(id("dialog"), rect(0.0, 0.0, 100.0, 100.0));
        let text_regions = vec![TextRegion {
            id: id("body"),
            bounds: rect(0.0, 0.0, 100.0, 100.0),
        }];
        let mut drag = DragState::new();
        let events = dispatch_click(
            &stack,
            &[],
            &text_regions,
            &mut drag,
            pt(50.0, 50.0),
            MouseButton::Left,
            Modifiers::default(),
        );
        // Modal wins — no TextSelection drag started.
        assert!(!drag.is_active());
        assert!(matches!(
            &events[0],
            UiEvent::MouseDown { widget: Some(w), .. } if w.as_str() == "dialog"
        ));
    }

    #[test]
    fn dispatch_mouse_drag_text_selection_emits_changed() {
        let mut drag = DragState::new();
        drag.begin(DragTarget::TextSelection {
            region: id("log:body"),
            anchor: pt(5.0, 2.0),
        });
        let events = dispatch_mouse_drag(&drag, pt(10.0, 5.0), buttons_mask_left());
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], UiEvent::MouseMoved { .. }));
        match &events[1] {
            UiEvent::TextSelectionChanged {
                region,
                anchor,
                focus,
            } => {
                assert_eq!(region.as_str(), "log:body");
                assert_eq!(*anchor, pt(5.0, 2.0));
                assert_eq!(*focus, pt(10.0, 5.0));
            }
            other => panic!("expected TextSelectionChanged, got {:?}", other),
        }
    }

    // ── text_selection_line_range tests ──────────────────────────────────

    #[test]
    fn line_range_empty_when_anchor_equals_focus() {
        let bounds = rect(0.0, 0.0, 40.0, 10.0);
        let ranges = text_selection_line_range(pt(5.0, 3.0), pt(5.0, 3.0), bounds);
        assert!(ranges.is_empty(), "collapsed selection must be empty");
    }

    #[test]
    fn line_range_single_row() {
        // Same row, anchor before focus.
        let bounds = rect(0.0, 0.0, 40.0, 10.0);
        let ranges = text_selection_line_range(pt(5.0, 3.0), pt(10.0, 3.0), bounds);
        // Half-open: 5..11 (focus_col 10 is inclusive → +1 = 11).
        assert_eq!(ranges, vec![(3, 5, 11)]);
    }

    #[test]
    fn line_range_single_row_reversed() {
        // Same row, focus before anchor — must swap to document order.
        let bounds = rect(0.0, 0.0, 40.0, 10.0);
        let ranges = text_selection_line_range(pt(10.0, 3.0), pt(5.0, 3.0), bounds);
        assert_eq!(ranges, vec![(3, 5, 11)]);
    }

    #[test]
    fn line_range_multi_row() {
        let bounds = rect(0.0, 0.0, 40.0, 10.0);
        // anchor at (2,1), focus at (5,3).
        let ranges = text_selection_line_range(pt(2.0, 1.0), pt(5.0, 3.0), bounds);
        // Row 1: 2..40 (start_col to region_end)
        // Row 2: 0..40 (full width)
        // Row 3: 0..6  (region_start to focus_col+1)
        assert_eq!(ranges, vec![(1, 2, 40), (2, 0, 40), (3, 0, 6)]);
    }

    #[test]
    fn line_range_multi_row_reversed() {
        let bounds = rect(0.0, 0.0, 40.0, 10.0);
        // Swap anchor and focus — must produce same result in document order.
        let ranges = text_selection_line_range(pt(5.0, 3.0), pt(2.0, 1.0), bounds);
        assert_eq!(ranges, vec![(1, 2, 40), (2, 0, 40), (3, 0, 6)]);
    }

    #[test]
    fn line_range_clamped_to_bounds() {
        // Region starts at x=5, y=2; anchor/focus way outside — must clamp.
        let bounds = rect(5.0, 2.0, 20.0, 5.0);
        let ranges = text_selection_line_range(pt(0.0, 0.0), pt(100.0, 100.0), bounds);
        // start row clamped to 2, end row clamped to 6 (2+5-1),
        // start_col clamped to 5, end_col clamped to 25.
        assert_eq!(
            ranges[0],
            (2, 5, 25),
            "first row: anchor clamped to region x"
        );
        assert_eq!(
            *ranges.last().unwrap(),
            (6, 5, 25),
            "last row: focus clamped to region end"
        );
        // All middle rows are full region width.
        for &(_, c_start, c_end) in &ranges[1..ranges.len() - 1] {
            assert_eq!(c_start, 5);
            assert_eq!(c_end, 25);
        }
    }
}
