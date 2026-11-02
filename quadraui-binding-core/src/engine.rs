//! The `event -> hit -> semantic` orchestration every binding's own
//! [`quadraui::AppLogic::handle`] would otherwise have to re-pour:
//! menu-bar/context-menu routing, divider/scrollbar drag arming and
//! translation, click-to-focus, focus-scoped key routing, and the raw ->
//! semantic event substitution [`crate::semantic::semantic_event_for_hit`]
//! performs. [`InteractionEngine`] owns every piece of state that
//! orchestration touches; a binding's own `AppLogic` impl holds one, feeds
//! it the frame's painted hits after each render, and forwards `handle` to
//! it with an [`EventSink`] that bridges back into its own user-code
//! callback — the one binding-specific step this engine cannot do itself.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use quadraui::compose::{
    ContextMenuController, ContextMenuOutcome, MenuDef, MenuEvent, MenuSystem,
};
use quadraui::primitives::scrollbar::{scroll_by_clamped, ThumbDrag};
use quadraui::{wheel_row_delta, Backend, EditOp, MenuBar, Modifiers, Rect, UiEvent, WidgetId};

use crate::drag::{find_split_layout, tree_scroll_state, ActiveDrag, ScrollbarDrag};
use crate::extra_paint::extra_as;
use crate::hit::{event_position, hit_test_painted, set_event_widget, HitDetail, PaintedHit};
use crate::semantic::{focus_scoped_key_event, semantic_event_for_hit};
use crate::text_input::{text_input_insert_event, TextCursorState};

/// The one binding-specific step [`InteractionEngine::handle`] cannot do
/// itself: handing a resolved [`UiEvent`] (and, for a pointer event that
/// landed on a specific sub-area, its [`HitDetail`]) to the binding's own
/// user-code callback, and reporting back the [`quadraui::Reaction`] that
/// callback produced. A binding implements this once, around its own
/// handler-invocation machinery (Python's `Py<PyAny>::call1`, a JS
/// function value, ...) — everything upstream of this call is identical
/// for every binding and lives in [`InteractionEngine::handle`] instead.
pub trait EventSink {
    fn dispatch(&mut self, event: &UiEvent, hit_detail: Option<&HitDetail>) -> quadraui::Reaction;
}

/// Owns every piece of state the `event -> hit -> semantic` orchestration
/// needs across frames: the current frame's painted hits, an armed
/// divider/scrollbar drag, per-widget text-input cursor cache, and the
/// menu-bar/context-menu controllers. A binding's own `AppLogic` impl holds
/// exactly one of these and never needs its own copy of any of this state.
#[derive(Default)]
pub struct InteractionEngine {
    /// Every primitive the binding's most recent [`quadraui::AppLogic::render`]
    /// call painted, in paint order, as a [`PaintedHit`] carrying its id and
    /// the hit-testable layout its backend `draw_*` call returned. A binding
    /// rebuilds this from scratch via [`InteractionEngine::set_hits`] at the
    /// start of every `render` call; [`InteractionEngine::handle`] reads it
    /// to translate an incoming pointer event's screen coordinates into the
    /// `WidgetId` (and finer [`HitDetail`]) it landed on.
    hit_map: RefCell<Vec<PaintedHit>>,
    /// Armed by a `MouseDown` landing on a split divider (per [`Self::hit_map`]'s
    /// [`PaintedHit::Split`] hit-testing) and cleared on the matching
    /// `MouseUp`. While armed, [`InteractionEngine::handle`] turns every
    /// `MouseMoved` into a synthesized `UiEvent::SplitDividerDragged` instead
    /// of forwarding the raw move, so the binding's own event handler never
    /// has to recompute split geometry itself to support divider dragging.
    active_drag: RefCell<Option<ActiveDrag>>,
    /// Armed by a `MouseDown` landing on `HitDetail::TreeScrollbarThumb`
    /// (per [`Self::hit_map`]'s [`PaintedHit::Tree`] hit-testing) and cleared on
    /// the matching `MouseUp`. While armed, [`InteractionEngine::handle`]
    /// turns every `MouseMoved` into a synthesized `UiEvent::ScrollOffsetChanged`
    /// instead of forwarding the raw move, mirroring [`Self::active_drag`]'s
    /// divider-drag handling for the tree's own scrollbar thumb.
    scrollbar_drag: RefCell<Option<ScrollbarDrag>>,
    /// Per-`TextInput` cursor/selection state, keyed by `WidgetId`, that
    /// survives across keystrokes — see [`crate::text_input::apply_text_input_edit`]'s
    /// own doc for why a cache is needed at all.
    text_cursors: RefCell<HashMap<WidgetId, TextCursorState>>,
    /// Owns the in-window dropdown behind a `"menu_bar"` extra: the
    /// open/close/keyboard-nav/submenu state machine that turns a click on
    /// a top-level item into an open dropdown and a selection into
    /// `UiEvent::MenuActivated`.
    menu_system: RefCell<MenuSystem>,
    /// The absolute bounds of the `"menu_bar"` extra [`InteractionEngine::sync_menu_bar`]
    /// most recently painted, read by [`InteractionEngine::handle`] to call
    /// [`MenuSystem::handle`] with the same `bar_rect` the paint pass used.
    /// `None` whenever no `"menu_bar"` extra was present that frame.
    menu_bar_rect: RefCell<Option<Rect>>,
    /// Owns the state behind a binding's own `show_context_menu` surface:
    /// which popup (if any) is open and routes its mouse/keyboard navigation
    /// and dismissal.
    ctx_menu: ContextMenuController,
}

impl InteractionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// The current frame's painted hits, for a binding that needs direct
    /// read access (e.g. to resolve a tree's scrollbar state for its own
    /// tests or diagnostics) beyond what [`InteractionEngine::handle`] and
    /// [`InteractionEngine::tab_stops`] already expose.
    pub fn hit_map(&self) -> Ref<'_, Vec<PaintedHit>> {
        self.hit_map.borrow()
    }

    /// Replaces this frame's painted hits. Call once per render, after the
    /// binding's own paint pass (`draw_bands`/`paint_layout`) has fully
    /// populated `hits`.
    pub fn set_hits(&self, hits: Vec<PaintedHit>) {
        *self.hit_map.borrow_mut() = hits;
    }

    /// Mutable access to the context-menu controller, for a binding's own
    /// `show_context_menu`-style surface to open a popup on.
    pub fn ctx_menu_mut(&mut self) -> &mut ContextMenuController {
        &mut self.ctx_menu
    }

    /// Paints the open context-menu popup (if any) on top of whatever this
    /// frame already painted. Call once per render, after the binding's own
    /// paint pass and [`InteractionEngine::sync_menu_bar`].
    pub fn render_ctx_menu(&self, backend: &mut dyn Backend) {
        self.ctx_menu.render(backend);
    }

    /// Derives this frame's tab stops directly from the hit map
    /// [`InteractionEngine::set_hits`] last recorded, in paint order.
    /// `Split` carries no `WidgetId` and is not a focusable leaf, so it is
    /// skipped; `Extra` is a tab stop only when its descriptor opts in via
    /// [`crate::extra_paint::ExtraPaint::tab_stop`] (currently `Switch` and
    /// `TextInput`); every other variant yields its id and bounding `Rect`.
    pub fn tab_stops(&self) -> Vec<(WidgetId, Rect)> {
        self.hit_map
            .borrow()
            .iter()
            .filter_map(|hit| match hit {
                PaintedHit::Button { id, layout } => Some((*id, layout.bounds)),
                PaintedHit::Tree { id, area, .. }
                | PaintedHit::List { id, area, .. }
                | PaintedHit::DataTable { id, area, .. }
                | PaintedHit::TabBar { id, area, .. } => Some((*id, *area)),
                PaintedHit::Split { .. } => None,
                PaintedHit::Extra { id, area, extra } => extra.tab_stop().then_some((*id, *area)),
            })
            .collect()
    }

    /// Rebuilds [`Self::menu_system`]'s menu list from this frame's `"menu_bar"`
    /// extra (if any) and paints its dropdown/submenu popups on top of the
    /// plain bar `hits` already recorded, then remembers the bar's absolute
    /// bounds in [`Self::menu_bar_rect`] for [`InteractionEngine::handle`] to
    /// reuse. A no-op (and clears `menu_bar_rect`) when no `"menu_bar"`
    /// extra was painted this frame. Call once per render, after `hits` is
    /// fully built — the extras loop in `draw_bands`/`paint_leaf` already
    /// painted the plain, non-interactive bar via `MenuBar`'s own
    /// `ExtraPaint::paint`; this repaints it with `menu_system`'s own
    /// open/focused state (and the dropdown, when one is open) layered on
    /// top, the same "paint after everything else" contract
    /// `ContextMenuController::render` documents for itself. Only the
    /// *first* `"menu_bar"` extra painted this frame is wired up this
    /// way — a second one in the same frame still paints (via `paint_leaf`'s
    /// normal `Extra` path) but is not interactive.
    ///
    /// `menu_system.render` draws from `MenuSystem`'s own open/focused/id
    /// state (see `MenuSystem::build_menu_bar`), not the descriptor's
    /// `open_item`/`focused_item`/`id` fields — those are overridden, so a
    /// binding that sets them declaratively (e.g. to render a pressed
    /// visual) has no effect once the bar is interactive.
    pub fn sync_menu_bar(&self, backend: &mut dyn Backend, hits: &[PaintedHit]) {
        let found = hits.iter().find_map(|hit| match hit {
            PaintedHit::Extra { area, extra, .. } => {
                extra_as::<MenuBar>(extra).map(|menu_bar| (*area, menu_bar))
            }
            _ => None,
        });
        let Some((area, menu_bar)) = found else {
            // The `"menu_bar"` extra is gone from this frame (the app
            // stopped rendering one, or swapped it for something else).
            // Close any dropdown first so the `"menu-system-dropdown"`
            // entry `MenuSystem::open_menu` pushed onto the modal stack
            // gets popped — otherwise it leaks, and the dropdown would
            // silently reappear (with nothing to hit-test it) if the
            // extra ever comes back.
            if self.menu_system.borrow().is_open() {
                self.menu_system.borrow_mut().close(backend);
            }
            *self.menu_bar_rect.borrow_mut() = None;
            return;
        };
        let menus: Vec<MenuDef> = menu_bar
            .items
            .iter()
            .map(|item| MenuDef {
                id: item.id,
                label: item.label.clone(),
                disabled: item.disabled,
                items: item.submenu.clone().unwrap_or_default(),
            })
            .collect();
        let mut menu_system = self.menu_system.borrow_mut();
        menu_system.set_menus(menus);
        menu_system.render(backend, area);
        *self.menu_bar_rect.borrow_mut() = Some(area);
    }

    /// The full `event -> hit -> semantic` orchestration: menu-bar/
    /// context-menu routing, divider/scrollbar drag arming and translation,
    /// click-to-focus, focus-scoped key/paste routing, and the raw ->
    /// semantic event substitution, ending in exactly one call to `sink`
    /// (two, for a `MouseDown`/`DoubleClick` that also changes focus) with
    /// the fully-resolved event. Mirrors a binding's own `AppLogic::handle`
    /// body one-for-one; the binding itself need only check whether its own
    /// handler is registered before calling this (so that an app with no
    /// handler at all skips every bit of state this engine would otherwise
    /// touch) and supply `sink`.
    pub fn handle(
        &mut self,
        mut event: UiEvent,
        backend: &mut dyn Backend,
        sink: &mut dyn EventSink,
    ) -> quadraui::Reaction {
        use quadraui::Reaction;

        // A `MouseUp` always ends an armed divider/scrollbar drag, even
        // when the menu-bar/context-menu routing just below reports the
        // same event `Consumed` (e.g. a dropdown-item click, or the
        // outside-click that dismisses an open dropdown/popup) and returns
        // before reaching the `MouseUp` check that would otherwise sit
        // after that routing — leaving a drag armed across the dismissal
        // otherwise.
        if matches!(event, UiEvent::MouseUp { .. }) {
            *self.active_drag.borrow_mut() = None;
            *self.scrollbar_drag.borrow_mut() = None;
        }

        // A `"menu_bar"` extra's dropdown and a context-menu popup both own
        // a full modal-input state machine that must see every event before
        // the rest of this function's own routing — matching
        // `MenuSystem::handle`/`ContextMenuController::handle`'s own doc
        // ("call this before other UI routing"). Each reports `Ignored`
        // immediately when nothing of theirs is open/relevant, so this is
        // safe to run unconditionally even for an app with neither a menu
        // bar nor an open context menu.
        let mut menu_reaction = Reaction::Continue;
        if let Some(bar_rect) = *self.menu_bar_rect.borrow() {
            let was_open = self.menu_system.borrow().is_open();
            let menu_event = self
                .menu_system
                .borrow_mut()
                .handle(&event, backend, bar_rect);
            match menu_event {
                MenuEvent::Activated(id) => {
                    // Activation already closed the dropdown in
                    // `menu_system`'s own state; force the repaint that
                    // takes it off screen regardless of what `sink`
                    // itself returns (mirrors `focus_reaction.merge`
                    // below).
                    return Reaction::Redraw
                        .merge(sink.dispatch(&UiEvent::MenuActivated(id), None));
                }
                // Navigation/highlight only — nothing for the binding's own
                // handler to react to, just a repaint so the new
                // open/selected state shows. Exception: a `MouseMoved` with
                // no dropdown open before or after this call only updated
                // which top-level label is hover-highlighted — it isn't
                // "consuming" the move in any sense a divider/scrollbar drag
                // or the handler itself would care about, so let it keep
                // flowing through the rest of this function instead of
                // swallowing it here; this call's own repaint need folds
                // into whatever `Reaction` that routing ends up producing.
                MenuEvent::StateChanged | MenuEvent::Consumed => {
                    let hover_only_on_closed_bar = !was_open
                        && !self.menu_system.borrow().is_open()
                        && matches!(event, UiEvent::MouseMoved { .. });
                    if hover_only_on_closed_bar {
                        menu_reaction = Reaction::Redraw;
                    } else {
                        return Reaction::Redraw;
                    }
                }
                MenuEvent::Ignored => {}
            }
        }
        match self.ctx_menu.handle(&event, backend) {
            ContextMenuOutcome::Event(ev) => {
                // Same reasoning as the `MenuEvent::Activated` arm above:
                // activation/dismissal already closed the popup in
                // `ctx_menu`'s own state, so force the repaint regardless
                // of what `sink` returns.
                return Reaction::Redraw.merge(sink.dispatch(&ev, None));
            }
            ContextMenuOutcome::Consumed => return Reaction::Redraw,
            ContextMenuOutcome::Ignored => {}
        }

        // A move while a divider drag is armed becomes the drag itself —
        // checked before the hit-test below, since `MouseMoved` has no
        // `widget` field for `event_position`/`set_event_widget` to touch.
        // A move while a tree scrollbar thumb drag is armed instead becomes
        // a `ScrollOffsetChanged`, mirroring the divider case; the two
        // drags are mutually exclusive, so only one of these branches ever
        // fires for a given move.
        if let UiEvent::MouseMoved { position, .. } = &event {
            if let Some(drag) = self.active_drag.borrow().as_ref() {
                let new_ratio = drag.ratio_at(*position);
                event = UiEvent::SplitDividerDragged {
                    tree: drag.tree,
                    split_index: 0,
                    new_ratio,
                };
            } else if let Some(drag) = self.scrollbar_drag.borrow().as_ref() {
                if let Some(new_offset) = drag.drag.offset_at(position.y) {
                    let new_offset = new_offset.round() as usize;
                    event = UiEvent::ScrollOffsetChanged {
                        widget: drag.widget,
                        new_offset,
                    };
                }
            }
        }

        // A plain `KeyPressed` (Tab/Shift+Tab already claimed upstream by
        // the binding's own `preprocess_event` once `tab_stops` is
        // non-empty) is routed to whichever leaf currently holds focus,
        // wrapped into that leaf's own `KeyPressed` variant so the binding's
        // own handler sees a focus-scoped event instead of a backend-global
        // one.
        if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
            if let Some(focused) = backend.focus_manager().focused().copied() {
                event = focus_scoped_key_event(
                    focused,
                    key,
                    *modifiers,
                    &self.hit_map.borrow(),
                    &self.text_cursors,
                );
            }
        }

        // IME/compose and clipboard-paste text has no `KeyPressed` of its
        // own to route through `focus_scoped_key_event` above — resolve it
        // the same way, directly against whichever `TextInput` extra
        // currently holds focus.
        if let UiEvent::CharTyped(ch) = &event {
            if let Some(focused) = backend.focus_manager().focused().copied() {
                if let Some(form_event) = text_input_insert_event(
                    focused,
                    &self.hit_map.borrow(),
                    EditOp::InsertChar(*ch),
                    &self.text_cursors,
                ) {
                    event = form_event;
                }
            }
        }
        if let UiEvent::ClipboardPaste(text) = &event {
            if let Some(focused) = backend.focus_manager().focused().copied() {
                if let Some(form_event) = text_input_insert_event(
                    focused,
                    &self.hit_map.borrow(),
                    EditOp::InsertText(text.clone()),
                    &self.text_cursors,
                ) {
                    event = form_event;
                }
            }
        }

        let hit = event_position(&event)
            .and_then(|position| hit_test_painted(&self.hit_map.borrow(), position));
        if let Some((id, _)) = hit {
            set_event_widget(&mut event, id);
        }

        // A wheel `Scroll` landing anywhere over a tree (its rows, its
        // chevrons, or its scrollbar) replaces the raw event outright with
        // the `ScrollOffsetChanged` it resolves to — never dispatched
        // alongside it — mirroring the `MouseDown`/`DoubleClick` semantic
        // replacement below, including suppressing the `"hit"` attachment
        // via `scroll_translated`, below. A tree with nothing to scroll
        // (all rows fit the viewport) has no scrollbar, so
        // `tree_scroll_state` returns `None` and the raw `Scroll` passes
        // through untouched.
        let mut scroll_translated = false;
        if let UiEvent::Scroll { delta, .. } = &event {
            if let Some((id, detail)) = &hit {
                let over_tree = matches!(
                    detail,
                    HitDetail::TreeRow { .. }
                        | HitDetail::TreeChevron { .. }
                        | HitDetail::TreeScrollbarThumb
                        | HitDetail::TreeScrollbarTrackBefore
                        | HitDetail::TreeScrollbarTrackAfter
                );
                if over_tree {
                    if let Some((_, total_rows, viewport_rows, scroll_offset)) =
                        tree_scroll_state(&self.hit_map.borrow(), *id)
                    {
                        let max_offset = total_rows.saturating_sub(viewport_rows);
                        let rows = wheel_row_delta(delta.y, 1.0);
                        let new_offset = scroll_by_clamped(scroll_offset, rows, max_offset);
                        event = UiEvent::ScrollOffsetChanged {
                            widget: *id,
                            new_offset,
                        };
                        scroll_translated = true;
                    }
                }
            }
        }

        // A `MouseDown` (or a bare `DoubleClick` pushed without the
        // `MouseDown`/`MouseUp` pair that precedes one on a real backend —
        // a host synthesizing clicks, e.g. a test, does this) landing on a
        // focusable painted leaf moves keyboard focus there first — matching
        // every native backend's click-to-focus convention — and, if that
        // actually changed the focused widget, dispatches a `FocusChanged`
        // event through `sink` ahead of the click event itself. Its
        // `Reaction` is combined with the main dispatch's below (via
        // `Reaction::merge`) rather than returned on its own, so a
        // non-`Continue` result from the `FocusChanged` dispatch (e.g. a
        // `Redraw`) never suppresses the click event that follows it.
        let mut focus_reaction = Reaction::Continue;
        if let Some((id, _)) = &hit {
            if matches!(
                event,
                UiEvent::MouseDown { .. } | UiEvent::DoubleClick { .. }
            ) {
                let is_focusable = self.tab_stops().iter().any(|(tid, _)| tid == id);
                if is_focusable && backend.set_focus(Some(*id)) {
                    focus_reaction = sink.dispatch(&UiEvent::FocusChanged(Some(*id)), None);
                    if matches!(focus_reaction, Reaction::Exit) {
                        return focus_reaction;
                    }
                }
            }
        }

        // A `MouseDown`/`DoubleClick` either arms a divider drag or, for a
        // tab/tree-row/tree-chevron/list-item/data-table-row/data-table-header
        // hit, is replaced outright by the semantic event it stands for — in
        // which case the raw `MouseDown`/`DoubleClick` (and its `"hit"` dict)
        // never reaches `sink` at all.
        let mut attach_hit = hit.is_some() && !scroll_translated;
        if let Some((id, detail)) = &hit {
            let modifiers_and_double_click = match &event {
                UiEvent::MouseDown { modifiers, .. } => Some((*modifiers, false)),
                UiEvent::DoubleClick { .. } => Some((Modifiers::default(), true)),
                _ => None,
            };
            if let Some((modifiers, double_click)) = modifiers_and_double_click {
                if !double_click && matches!(detail, HitDetail::SplitDivider) {
                    if let UiEvent::MouseDown { position, .. } = &event {
                        if let Some(layout) =
                            find_split_layout(&self.hit_map.borrow(), *position, *id)
                        {
                            *self.active_drag.borrow_mut() = Some(ActiveDrag { tree: *id, layout });
                        }
                    }
                } else if !double_click && matches!(detail, HitDetail::TreeScrollbarThumb) {
                    // A press on the thumb itself arms a drag (consumed by
                    // the `MouseMoved` branch above on later moves) rather
                    // than scrolling immediately — matching
                    // `compose::TreeController::click_scrollbar`'s
                    // `PressZone::Thumb` arm. The raw `MouseDown` (and its
                    // `"hit"` dict) stops here either way, so this never
                    // falls through to row selection.
                    if let UiEvent::MouseDown { position, .. } = &event {
                        if let Some((scrollbar, total_rows, viewport_rows, scroll_offset)) =
                            tree_scroll_state(&self.hit_map.borrow(), *id)
                        {
                            let max_offset = total_rows.saturating_sub(viewport_rows);
                            if max_offset > 0 {
                                *self.scrollbar_drag.borrow_mut() = Some(ScrollbarDrag {
                                    widget: *id,
                                    drag: ThumbDrag::begin(
                                        position.y,
                                        scroll_offset as f32,
                                        scrollbar.travel(),
                                        max_offset as f32,
                                    ),
                                });
                            }
                        }
                    }
                    attach_hit = false;
                } else if !double_click
                    && matches!(
                        detail,
                        HitDetail::TreeScrollbarTrackBefore | HitDetail::TreeScrollbarTrackAfter
                    )
                {
                    // A press on the track pages by one viewport, matching
                    // `click_scrollbar`'s `PressZone::Before`/`After` arms
                    // (`page_scroll`). Never falls through to row selection.
                    if let Some((_, total_rows, viewport_rows, scroll_offset)) =
                        tree_scroll_state(&self.hit_map.borrow(), *id)
                    {
                        let max_offset = total_rows.saturating_sub(viewport_rows);
                        let page = if matches!(detail, HitDetail::TreeScrollbarTrackBefore) {
                            -(viewport_rows as isize)
                        } else {
                            viewport_rows as isize
                        };
                        let new_offset = scroll_by_clamped(scroll_offset, page, max_offset);
                        event = UiEvent::ScrollOffsetChanged {
                            widget: *id,
                            new_offset,
                        };
                    }
                    attach_hit = false;
                } else if let Some(semantic) = semantic_event_for_hit(
                    *id,
                    detail,
                    modifiers,
                    double_click,
                    &self.hit_map.borrow(),
                ) {
                    event = semantic;
                    attach_hit = false;
                }
            }
        }

        let hit_detail = if attach_hit {
            hit.as_ref().map(|(_, detail)| detail)
        } else {
            None
        };
        let event_reaction = sink.dispatch(&event, hit_detail);
        menu_reaction.merge(focus_reaction).merge(event_reaction)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use quadraui::tui::testing::TuiDriver;
    use quadraui::{
        AppLogic, Backend, Button, Modifiers, MouseButton, Point, Reaction, Rect, UiEvent, WidgetId,
    };

    use super::{EventSink, InteractionEngine};
    use crate::hit::HitDetail;
    use crate::render::{draw_bands, RenderRefs};

    /// Records every event [`InteractionEngine::handle`] dispatches, in
    /// order, so a test can assert on the orchestration's decisions (e.g.
    /// click-to-focus sending a `FocusChanged` ahead of the click itself)
    /// without any host-language callback in scope.
    #[derive(Default)]
    struct RecordingSink {
        dispatched: Vec<(UiEvent, Option<HitDetail>)>,
    }

    impl EventSink for RecordingSink {
        fn dispatch(&mut self, event: &UiEvent, hit_detail: Option<&HitDetail>) -> Reaction {
            self.dispatched.push((event.clone(), hit_detail.cloned()));
            Reaction::Continue
        }
    }

    /// A minimal [`AppLogic`] whose `render`/`handle` go through
    /// [`InteractionEngine`] exactly the way a real binding's own thin
    /// `AppLogic` leaf would, with no host-language FFI type anywhere in
    /// this file.
    struct MinimalLogic {
        button: Button,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for MinimalLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: Some(&self.button),
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                },
                &mut hits,
            );
            self.engine.set_hits(hits);
        }

        fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// A `MouseDown` on the button must round-trip through
    /// [`InteractionEngine::handle`] as click-to-focus (a `FocusChanged`
    /// dispatch) followed by the click itself with its hit attached — the
    /// same two-dispatch sequence a real binding's `AppLogic::handle` leaf
    /// produces, proving the orchestration itself (not just paint/hit-test)
    /// is usable with no host-language binding in scope.
    #[test]
    fn click_on_a_button_focuses_it_then_dispatches_the_click() {
        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 20, 5);

        let button_bounds = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    crate::hit::PaintedHit::Button { layout, .. } => Some(layout.bounds),
                    _ => None,
                })
                .expect("button should have painted a hit")
        };
        let click_point = Point::new(
            button_bounds.x + button_bounds.width / 2.0,
            button_bounds.y + button_bounds.height / 2.0,
        );

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: click_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert_eq!(
            dispatched.len(),
            2,
            "expected a focus dispatch and the click itself"
        );
        assert!(
            matches!(dispatched[0].0, UiEvent::FocusChanged(Some(id)) if id == WidgetId::new("go")),
            "click-to-focus must dispatch FocusChanged first: {:?}",
            dispatched[0]
        );
        assert!(
            matches!(dispatched[1].0, UiEvent::MouseDown { .. }),
            "the raw click must still follow, with its hit attached: {:?}",
            dispatched[1]
        );
        assert!(
            dispatched[1].1.is_some(),
            "a button has no semantic click substitution, so the raw click keeps its hit"
        );
    }
}
