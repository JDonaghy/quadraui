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

use quadraui::backend::MessageDialogOptions;
use quadraui::compose::{
    ContextMenuController, ContextMenuOutcome, FormController, FormControllerEvent, MenuDef,
    MenuEvent, MenuSystem, MessageDialogController, MessageDialogEvent,
};
use quadraui::primitives::scrollbar::{scroll_by_clamped, ThumbDrag};
use quadraui::{
    wheel_row_delta, Backend, CompletionItemMeasure, Completions, CompletionsHit,
    CompletionsLayout, ContextMenu, ContextMenuItem, ContextMenuPlacement, DataTableEvent, EditOp,
    Float, FloatLayout, FloatMeasure, Form, InteractionState, Key, MenuBar, MessageDialogButton,
    Modifiers, NamedKey, Point, Rect, RichTextPopup, RichTextPopupHit, RichTextPopupLayout,
    RichTextPopupMeasure, TextDisplayEvent, TextInput, Toast, ToastCorner, ToastHit, ToastOverlay,
    ToastStackLayout, Tooltip, TooltipLayout, TooltipMeasure, UiEvent, WidgetId,
};

use crate::drag::{
    find_data_table_layout, find_split_layout, find_split_tree_layout, text_display_scroll_state,
    tree_scroll_state, ActiveDrag, DataTableColumnDrag, ScrollbarDrag,
};
use crate::extra_paint::extra_as;
use crate::hit::{
    chrome_hover_hit, event_position, hit_test_painted, set_event_widget, HitDetail, PaintedHit,
};
use crate::semantic::{focus_scoped_key_event, number_input_click_event, semantic_event_for_hit};
use crate::text_input::{text_input_click_event, text_input_insert_event, TextEditorCache};

/// `backend.viewport()`, as a `(0, 0)`-origin [`Rect`] — the shape every
/// anchored-overlay layout call here (`Tooltip`/`Completions`/`Float`/
/// `RichTextPopup`/`ToastOverlay`) takes as its own viewport-clamp
/// argument.
fn viewport_rect(backend: &dyn Backend) -> Rect {
    let viewport = backend.viewport();
    Rect::new(0.0, 0.0, viewport.width, viewport.height)
}

/// `Reaction::Continue`, unless a tooltip auto-dismissed earlier in the
/// same [`InteractionEngine::handle`] call, in which case the redraw that
/// takes it off screen still has to happen — the same reasoning
/// `InteractionEngine::handle`'s own `tooltip_dismissed` local documents,
/// applied to an early `return` that would otherwise skip past the final
/// `Reaction` merge entirely.
fn continue_or_tooltip_redraw(tooltip_dismissed: bool) -> quadraui::Reaction {
    if tooltip_dismissed {
        quadraui::Reaction::Redraw
    } else {
        quadraui::Reaction::Continue
    }
}

/// The one binding-specific step [`InteractionEngine::handle`] cannot do
/// itself: handing a resolved [`UiEvent`] (and, for a pointer event that
/// landed on a specific sub-area, its [`HitDetail`]) to the binding's own
/// user-code callback, and reporting back the [`quadraui::Reaction`] that
/// callback produced. A binding implements this once, around its own
/// handler-invocation machinery (a Python callable, a JS function value,
/// ...) — everything upstream of this call is identical for every binding
/// and lives in [`InteractionEngine::handle`] instead.
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
    /// Armed by a `MouseDown` landing on a split divider — a two-pane
    /// `Split`'s (per [`Self::hit_map`]'s [`PaintedHit::Split`]
    /// hit-testing) or one of an N-way `SplitTree`'s (per
    /// [`PaintedHit::SplitTree`]) — and cleared on the matching `MouseUp`.
    /// While armed, [`InteractionEngine::handle`] turns every `MouseMoved`
    /// into a synthesized `UiEvent::SplitDividerDragged` instead of
    /// forwarding the raw move, so the binding's own event handler never
    /// has to recompute split geometry itself to support divider dragging.
    active_drag: RefCell<Option<ActiveDrag>>,
    /// Armed by a `MouseDown` landing on `HitDetail::TreeScrollbarThumb`
    /// (per [`Self::hit_map`]'s [`PaintedHit::Tree`] hit-testing) and cleared on
    /// the matching `MouseUp`. While armed, [`InteractionEngine::handle`]
    /// turns every `MouseMoved` into a synthesized `UiEvent::ScrollOffsetChanged`
    /// instead of forwarding the raw move, mirroring [`Self::active_drag`]'s
    /// divider-drag handling for the tree's own scrollbar thumb.
    scrollbar_drag: RefCell<Option<ScrollbarDrag>>,
    /// Armed by a `MouseDown` landing on `HitDetail::DataTableHeaderDivider`
    /// (per [`Self::hit_map`]'s [`PaintedHit::DataTable`] hit-testing) and
    /// cleared on the matching `MouseUp`. While armed,
    /// [`InteractionEngine::handle`] turns every `MouseMoved` into a
    /// synthesized `UiEvent::DataTable(.., ColumnResized)` instead of
    /// forwarding the raw move, mirroring [`Self::active_drag`]'s divider
    /// handling for a `DataTable`'s own column dividers.
    column_drag: RefCell<Option<DataTableColumnDrag>>,
    /// Per-`TextInput` [`quadraui::TextEditor`] session, keyed by
    /// `WidgetId`, that survives across keystrokes — see
    /// [`crate::text_input::apply_text_input_edit`]'s own doc for why a
    /// cache is needed at all, including its own `undo_stack`.
    text_editors: TextEditorCache,
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
    /// Owns the state behind a binding's own `show_message_dialog` surface:
    /// the open in-canvas [`Dialog`][quadraui::Dialog] (if any), tracked on
    /// the backend's [`quadraui::ModalStack`] for the same hit-test
    /// precedence [`Self::ctx_menu`] already has.
    dialog: Option<MessageDialogController>,
    /// Per-bar hovered child, keyed by the chrome-bar extra's own id
    /// (`Toolbar`'s button, `StatusBar`'s segment, `ActivityBar`'s item) —
    /// updated on every `MouseMoved` via [`chrome_hover_hit`] and read back
    /// by [`Self::chrome_interaction`] so `render` paints real hover state
    /// instead of `InteractionState::default()`. A bar absent from this
    /// map has nothing hovered.
    chrome_hover: RefCell<HashMap<WidgetId, WidgetId>>,
    /// Per-bar pressed child, mirroring [`Self::chrome_hover`]. Set on a
    /// `MouseDown` landing on a chrome-bar child and cleared unconditionally
    /// on every `MouseUp`, same as [`Self::active_drag`]/[`Self::scrollbar_drag`]/
    /// [`Self::column_drag`].
    chrome_pressed: RefCell<HashMap<WidgetId, WidgetId>>,
    /// One [`FormController`] per `"form"` extra painted this (or a prior)
    /// frame, keyed by the extra's own id — rebuilt from [`Self::sync_forms`]
    /// every render so a submit/button click, toggle, or keystroke inside
    /// it resolves through the SAME field-level routing a `Form`-embedding
    /// host would get from driving `FormController` directly, instead of
    /// staying paint-only. An entry survives across frames (unlike
    /// [`Self::menu_system`]'s single instance) since, unlike `"menu_bar"`/
    /// `"find_replace"`, nothing about `Form` makes it a singleton overlay —
    /// a frame can legitimately paint more than one, each under its own id.
    form_controllers: RefCell<HashMap<WidgetId, FormController>>,
    /// The absolute bounds [`Self::sync_forms`] most recently painted each
    /// [`Self::form_controllers`] entry into, read by
    /// [`InteractionEngine::handle`] so a routed event's position
    /// resolves against the same `rect` the paint pass used (`FormController::handle`
    /// does its own position-relative hit-testing internally, unlike this
    /// engine's `hit_map`-based primitives).
    form_rects: RefCell<HashMap<WidgetId, Rect>>,
    /// This frame's `"form"` extra ids, in the paint order
    /// [`Self::sync_forms`] saw them — the `HashMap`s above have none, so
    /// a `MouseDown` claim loop iterating `form_rects` directly would
    /// resolve a same-frame rect overlap (which should never happen in
    /// practice; [`Self::sync_forms`]'s own doc notes it) against a
    /// nondeterministic order across runs. [`InteractionEngine::handle`]
    /// walks this instead, so the invariant "at most one claims a given
    /// click" is reproducible rather than merely usually true.
    form_order: RefCell<Vec<WidgetId>>,
    /// Owns the state behind a binding's own `show_toast`/`dismiss_toast`
    /// surface: the toasts currently showing, oldest first, and which
    /// corner they stack in. Non-modal (see [`quadraui::primitives::toast`]'s
    /// module doc) — unlike [`Self::dialog`], this never claims the
    /// [`quadraui::ModalStack`] or blocks input on its own; [`Self::handle`]
    /// only intercepts a click that lands on a toast's own dismiss/action
    /// region.
    toasts: Vec<Toast>,
    /// Which corner [`Self::toasts`] stacks in — set by the most recent
    /// [`Self::show_toast`] call.
    toast_corner: ToastCorner,
    /// Owns the state behind a binding's own `show_tooltip`/`hide_tooltip`
    /// surface: the open [`Tooltip`] (if any) and the anchor rect it was
    /// shown against. Non-interactive (see [`Self::render_tooltip`]'s doc)
    /// — [`Self::handle`] only ever clears it as a side effect of the next
    /// click/key, never routes a hit against it.
    tooltip: Option<(Tooltip, Rect)>,
    /// Owns the state behind a binding's own `show_completions`/
    /// `dismiss_completions` surface: the open [`Completions`] popup (if
    /// any) and the cursor position it was anchored to.
    completions: Option<(Completions, Point)>,
    /// Owns the state behind a binding's own `show_float`/`dismiss_float`
    /// surface: the open [`Float`] (if any) and the caller-supplied
    /// [`FloatMeasure`] its layout was resolved from. Pushed onto the
    /// backend's [`quadraui::ModalStack`] while open, mirroring
    /// [`Self::dialog`] — see [`quadraui::primitives::float`]'s own
    /// "Stacking" doc section for why a `Float` needs this even though it
    /// is otherwise non-blocking chrome.
    float: Option<(Float, FloatMeasure)>,
    /// Owns the state behind a binding's own `show_rich_text_popup`/
    /// `dismiss_rich_text_popup` surface: the open [`RichTextPopup`] (if
    /// any) and the anchor position it was shown against.
    rich_text_popup: Option<(RichTextPopup, Point)>,
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
    ///
    /// Also prunes [`Self::text_editors`] down to just the ids still
    /// painted as a `"text_input"` extra this frame — mirroring
    /// [`Self::sync_forms`]'s own "painted this frame" retain for
    /// [`Self::form_controllers`]. Without this, a `TextInput` that stops
    /// rendering (its host removes it, navigates away, ...) leaves its
    /// cached [`quadraui::TextEditor`] — buffer, cursor, and up to
    /// `UNDO_HISTORY_LIMIT` full undo snapshots — in the map forever.
    pub fn set_hits(&self, hits: Vec<PaintedHit>) {
        let live_text_inputs: Vec<WidgetId> = hits
            .iter()
            .filter_map(|hit| match hit {
                PaintedHit::Extra { id, extra, .. } if extra_as::<TextInput>(extra).is_some() => {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        self.text_editors
            .borrow_mut()
            .retain(|id, _| live_text_inputs.contains(id));
        *self.hit_map.borrow_mut() = hits;
    }

    /// Mutable access to the context-menu controller, for a binding's own
    /// `show_context_menu`-style surface to open a popup on.
    pub fn ctx_menu_mut(&mut self) -> &mut ContextMenuController {
        &mut self.ctx_menu
    }

    /// Builds an anchored [`ContextMenu`] from `items` and opens it at `at`,
    /// seeding its keyboard selection on the first selectable item — the
    /// full "build the popup, open it" sequence behind a binding's own
    /// `show_context_menu` surface, so that surface only has to decode its
    /// own wire format into `items` before calling this.
    pub fn open_context_menu(
        &mut self,
        items: Vec<ContextMenuItem>,
        at: Point,
        backend: &mut dyn Backend,
    ) {
        let mut menu = ContextMenu {
            id: WidgetId::new("qui-context-menu"),
            items,
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        };
        menu.selected_idx = menu.first_selectable();
        self.ctx_menu.open(menu, at, backend);
    }

    /// Paints the open context-menu popup (if any) on top of whatever this
    /// frame already painted. Call once per render, after the binding's own
    /// paint pass and [`InteractionEngine::sync_menu_bar`].
    pub fn render_ctx_menu(&self, backend: &mut dyn Backend) {
        self.ctx_menu.render(backend);
    }

    /// Whether a message dialog opened via [`Self::open_message_dialog`]
    /// is currently open.
    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// Opens an in-canvas message/alert/confirm dialog built from `opts`,
    /// pushing its bounds onto the backend's [`quadraui::ModalStack`] (via
    /// [`quadraui::Backend::modal_stack_handle`]) so other modal-aware code
    /// gets the same hit-test precedence an open context menu already has.
    /// Replaces (and pops) any dialog already open. Also closes an open
    /// `"menu_bar"` dropdown or context-menu popup, if either is open: the
    /// dialog paints on top of both (`render_dialog` runs after
    /// `sync_menu_bar`/`render_ctx_menu`), and [`Self::handle`] now routes
    /// the dialog ahead of both, so a popup left open underneath would be
    /// unreachable input it still holds precedence over. Call from a
    /// binding's own `show_message_dialog` surface, mirroring
    /// [`Self::ctx_menu_mut`]'s `.open(...)` call.
    pub fn open_message_dialog(&mut self, opts: MessageDialogOptions, backend: &mut dyn Backend) {
        self.close_message_dialog(backend);
        if self.menu_system.borrow().is_open() {
            self.menu_system.borrow_mut().close(backend);
        }
        if self.ctx_menu.is_open() {
            self.ctx_menu.close(backend);
        }
        let controller = MessageDialogController::new(opts);
        let id = controller.dialog().id;
        let bounds = controller.bounds(backend);
        backend.modal_stack_handle().borrow_mut().push(id, bounds);
        self.dialog = Some(controller);
    }

    /// Closes the open message dialog (if any), popping its
    /// [`quadraui::ModalStack`] entry. Returns whether a dialog was open to
    /// close. Exposed so a binding's own `show_message_dialog` surface can
    /// offer a programmatic dismiss alongside input-driven resolution
    /// (Enter/Escape/click) — without this, a dialog with no buttons at
    /// all, or one the app wants to tear down from outside input, could
    /// never be cleared. A no-op (returns `false`) if nothing is open.
    pub fn close_message_dialog(&mut self, backend: &mut dyn Backend) -> bool {
        match self.dialog.take() {
            Some(open) => {
                backend
                    .modal_stack_handle()
                    .borrow_mut()
                    .pop(&open.dialog().id);
                true
            }
            None => false,
        }
    }

    /// Paints the open message dialog (if any) on top of whatever this
    /// frame already painted. Call once per render, after the binding's own
    /// paint pass — same contract as [`Self::render_ctx_menu`].
    pub fn render_dialog(&self, backend: &mut dyn Backend) {
        if let Some(dialog) = &self.dialog {
            dialog.render(backend);
        }
    }

    // ── Toast ────────────────────────────────────────────────────────

    /// Shows `toast` in `corner`, stacking it among whatever toasts are
    /// already showing. Replaces (keeps the position of, but refreshes
    /// the content of) any existing toast with the same
    /// [`quadraui::types::WidgetId`]. `corner` applies to the whole
    /// stack, so showing a second toast in a different corner moves
    /// every toast — including ones already showing — to the new
    /// corner; apps that want toasts in two corners at once need two
    /// separate `WidgetId`-disjoint calls per frame, same caveat
    /// [`quadraui::ToastOverlay`]'s own single-`corner` field carries.
    pub fn show_toast(&mut self, toast: Toast, corner: ToastCorner) {
        self.toasts.retain(|t| t.id != toast.id);
        self.toasts.push(toast);
        self.toast_corner = corner;
    }

    /// Removes the toast with this id, if showing. Returns whether one
    /// was removed. Exposed so a binding's own `dismiss_toast` surface
    /// can close a toast programmatically (e.g. after its own auto-expire
    /// timer fires) in addition to the dismiss `×`
    /// [`Self::handle`] already routes.
    pub fn dismiss_toast(&mut self, id: WidgetId) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|t| t.id != id);
        self.toasts.len() != before
    }

    fn toast_overlay(&self) -> ToastOverlay {
        ToastOverlay {
            id: WidgetId::new("qui-toast-stack"),
            corner: self.toast_corner,
            toasts: self.toasts.clone(),
            focus: None,
        }
    }

    /// Paints every showing toast, stacked in [`Self::toast_corner`], on
    /// top of whatever this frame already painted. Call once per render,
    /// after every other `render_*` on this engine — a toast notification
    /// reads as "always on top" (VS Code convention), so it paints over
    /// an open dialog/popup/float too. A no-op when no toast is showing.
    pub fn render_toast(&self, backend: &mut dyn Backend) {
        if self.toasts.is_empty() {
            return;
        }
        let overlay = self.toast_overlay();
        backend.draw_toast_overlay(viewport_rect(backend), &overlay);
    }

    /// Recomputes the same [`quadraui::ToastStackLayout`]
    /// [`Self::render_toast`] just painted, for [`Self::handle`]'s click
    /// routing — the "one layout fn, two callers" pattern
    /// [`MessageDialogController::layout`] already follows for `Dialog`.
    fn toast_layout(&self, backend: &dyn Backend) -> ToastStackLayout {
        backend.toast_stack_layout(viewport_rect(backend), &self.toast_overlay())
    }

    // ── Tooltip ──────────────────────────────────────────────────────

    /// Shows `tooltip` anchored against `anchor` (the rect of whatever
    /// widget it describes, in the backend's ABSOLUTE frame). Replaces
    /// any tooltip already showing.
    pub fn show_tooltip(&mut self, tooltip: Tooltip, anchor: Rect) {
        self.tooltip = Some((tooltip, anchor));
    }

    /// Hides the open tooltip (if any). Returns whether one was open.
    /// [`Self::handle`] already does this automatically on the next
    /// click/key — see that method's doc — so apps only need this for an
    /// explicit "hide now" (e.g. the hovered widget itself changed).
    pub fn hide_tooltip(&mut self) -> bool {
        self.tooltip.take().is_some()
    }

    fn tooltip_layout(
        &self,
        backend: &dyn Backend,
        tooltip: &Tooltip,
        anchor: Rect,
    ) -> TooltipLayout {
        let m = backend.measure();
        let n_lines = tooltip.text.lines().count().max(1);
        let longest = tooltip.text.lines().map(str::len).max().unwrap_or(0);
        let measure = TooltipMeasure::new(
            m.char_width * (longest as f32 + 4.0),
            m.line_height * (n_lines as f32 + 2.0),
        );
        tooltip.layout(anchor, viewport_rect(backend), measure, m.line_height)
    }

    /// Paints the open tooltip (if any) on top of whatever this frame
    /// already painted. Tooltips are non-interactive
    /// ([`quadraui::Backend::draw_tooltip`]'s own doc — "no hit data
    /// returned"), so unlike every other `render_*` here there is
    /// nothing for [`Self::handle`] to hit-test against it; the only
    /// input routing it gets is the auto-dismiss [`Self::handle`]
    /// performs on the next click/key.
    pub fn render_tooltip(&self, backend: &mut dyn Backend) {
        if let Some((tooltip, anchor)) = &self.tooltip {
            let layout = self.tooltip_layout(backend, tooltip, *anchor);
            backend.draw_tooltip(tooltip, &layout);
        }
    }

    // ── Completions ──────────────────────────────────────────────────

    /// Shows `completions` anchored at `cursor` (the caret position in
    /// the backend's ABSOLUTE frame). Replaces any popup already open.
    pub fn show_completions(&mut self, completions: Completions, cursor: Point) {
        self.completions = Some((completions, cursor));
    }

    /// Closes the open completions popup (if any) without reporting a
    /// selection. Returns whether one was open.
    pub fn dismiss_completions(&mut self) -> bool {
        self.completions.take().is_some()
    }

    fn completions_layout(
        &self,
        backend: &dyn Backend,
        completions: &Completions,
        cursor: Point,
    ) -> CompletionsLayout {
        let m = backend.measure();
        completions.layout(
            cursor.x,
            cursor.y,
            m.line_height,
            viewport_rect(backend),
            m.char_width * 30.0,
            m.line_height * 8.0,
            |_| CompletionItemMeasure::new(m.line_height),
        )
    }

    /// Paints the open completions popup (if any) on top of whatever
    /// this frame already painted. Call once per render, before
    /// [`Self::render_dialog`] (which [`Self::handle`] checks before
    /// this popup's own hit-test): an app that somehow has both open at
    /// once gets the dialog on top, both visually and for input —
    /// "hit precedence agrees with paint order" either way, same
    /// invariant [`Self::dialog`]'s own doc calls out.
    pub fn render_completions(&self, backend: &mut dyn Backend) {
        if let Some((completions, cursor)) = &self.completions {
            let layout = self.completions_layout(backend, completions, *cursor);
            backend.draw_completions(completions, &layout);
        }
    }

    // ── Float ────────────────────────────────────────────────────────

    /// Opens `float` at `measure`'s size, pushing its resolved bounds
    /// onto the backend's [`quadraui::ModalStack`] via
    /// [`quadraui::ModalStack::push_float`] so other modal-aware code
    /// gets the same hit-test precedence [`Self::dialog`] already has.
    /// Replaces (and pops) any float already open. **Content is not
    /// this call's concern** — same as [`quadraui::Backend::draw_float`]
    /// itself, the host paints whatever belongs in
    /// [`quadraui::FloatLayout::content_bounds`] through its own normal
    /// render path after this call's `render_float` runs.
    pub fn show_float(&mut self, float: Float, measure: FloatMeasure, backend: &mut dyn Backend) {
        self.close_float(backend);
        let layout = float.layout(viewport_rect(backend), measure);
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push_float(&float, layout.bounds);
        self.float = Some((float, measure));
    }

    /// Closes the open float (if any), popping its
    /// [`quadraui::ModalStack`] entry. Returns whether one was open.
    pub fn dismiss_float(&mut self, backend: &mut dyn Backend) -> bool {
        self.close_float(backend)
    }

    fn close_float(&mut self, backend: &mut dyn Backend) -> bool {
        match self.float.take() {
            Some((float, _)) => {
                backend.modal_stack_handle().borrow_mut().pop(&float.id);
                true
            }
            None => false,
        }
    }

    fn float_layout(
        &self,
        backend: &dyn Backend,
        float: &Float,
        measure: FloatMeasure,
    ) -> FloatLayout {
        float.layout(viewport_rect(backend), measure)
    }

    /// Paints the open float's chrome (if any) on top of whatever this
    /// frame already painted. Call once per render. Paints chrome only —
    /// see [`Self::show_float`]'s doc — the host paints its own content
    /// into the returned bounds' `content_bounds` separately.
    pub fn render_float(&self, backend: &mut dyn Backend) {
        if let Some((float, measure)) = &self.float {
            let layout = self.float_layout(backend, float, *measure);
            backend.draw_float(float, &layout);
        }
    }

    // ── RichTextPopup ────────────────────────────────────────────────

    /// Shows `popup` anchored at `anchor` (typically the top-left of the
    /// editor cell it describes, in the backend's ABSOLUTE frame).
    /// Replaces any popup already open.
    pub fn show_rich_text_popup(&mut self, popup: RichTextPopup, anchor: Point) {
        self.rich_text_popup = Some((popup, anchor));
    }

    /// Closes the open rich-text popup (if any) without reporting a link
    /// click. Returns whether one was open.
    pub fn dismiss_rich_text_popup(&mut self) -> bool {
        self.rich_text_popup.take().is_some()
    }

    fn rich_text_popup_layout(
        &self,
        backend: &dyn Backend,
        popup: &RichTextPopup,
        anchor: Point,
    ) -> RichTextPopupLayout {
        let m = backend.measure();
        let measure = RichTextPopupMeasure::new(m.char_width * 30.0, m.line_height)
            .with_scale_rows(backend.scales_text_rows());
        popup.layout(
            anchor.x,
            anchor.y,
            viewport_rect(backend),
            measure,
            |_, start, end| m.char_width * (end - start) as f32,
        )
    }

    /// Paints the open rich-text popup (if any) on top of whatever this
    /// frame already painted. Call once per render.
    pub fn render_rich_text_popup(&self, backend: &mut dyn Backend) {
        if let Some((popup, anchor)) = &self.rich_text_popup {
            let layout = self.rich_text_popup_layout(backend, popup, *anchor);
            backend.draw_rich_text_popup(popup, &layout);
        }
    }

    /// Derives this frame's tab stops directly from the hit map
    /// [`InteractionEngine::set_hits`] last recorded, in paint order.
    /// `Split`/`SplitTree`/`Panel`/`SidebarPanel` are chrome around other
    /// content, not focusable leaves themselves, so they are skipped (their
    /// own action/toolbar buttons aren't recorded as separate `PaintedHit`
    /// entries — see `crate::hit::HitDetail::PanelAction` et al. — so there
    /// is nothing yet for a future keyboard-nav pass to pick up here
    /// either); `Extra` is a tab stop only when its descriptor opts in via
    /// [`crate::extra_paint::ExtraPaint::tab_stop`] (currently `Switch` and
    /// `TextInput`); `TextDisplay` is always one (scroll/copy need
    /// keyboard focus the same way a `Tree`/`List` does); every other
    /// variant yields its id and bounding `Rect`.
    pub fn tab_stops(&self) -> Vec<(WidgetId, Rect)> {
        self.hit_map
            .borrow()
            .iter()
            .filter_map(|hit| match hit {
                PaintedHit::Button { id, layout } => Some((*id, layout.bounds)),
                PaintedHit::Tree { id, area, .. }
                | PaintedHit::List { id, area, .. }
                | PaintedHit::DataTable { id, area, .. }
                | PaintedHit::TabBar { id, area, .. }
                | PaintedHit::TextDisplay { id, area, .. } => Some((*id, *area)),
                PaintedHit::Split { .. }
                | PaintedHit::SplitTree { .. }
                | PaintedHit::Panel { .. }
                | PaintedHit::SidebarPanel { .. } => None,
                PaintedHit::Extra {
                    id, area, extra, ..
                } => extra.tab_stop().then_some((*id, *area)),
            })
            .collect()
    }

    /// The hover/pressed [`InteractionState`] for the chrome-bar extra
    /// painted under id `bar_id` this frame, read from [`Self::chrome_hover`]/
    /// [`Self::chrome_pressed`] — call from `render`, once per chrome-bar
    /// extra, right before repainting it, so it shows real hover/pressed
    /// state instead of [`InteractionState::default()`].
    pub fn chrome_interaction(&self, bar_id: WidgetId) -> InteractionState {
        InteractionState::from_parts(
            self.chrome_hover.borrow().get(&bar_id).copied(),
            self.chrome_pressed.borrow().get(&bar_id).copied(),
        )
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

    /// Repaints every `"form"` extra painted this frame through its own
    /// cached [`FormController`] (creating one the first time an id is
    /// seen), so the form shows [`FormController`]'s own richer,
    /// stateful rendering (its live scroll offset, in-progress text/date
    /// edit overlay, ...) instead of just [`quadraui::Form`]'s own
    /// plain, non-interactive [`crate::extra_paint::ExtraPaint::paint`] —
    /// the same "paint after everything else, from the stateful
    /// controller instead of the bare descriptor" pattern
    /// [`Self::sync_menu_bar`] already uses for `"menu_bar"`. Call once
    /// per render, after the extras loop in `draw_bands`/`paint_leaf`
    /// already painted the plain bar. Unlike `sync_menu_bar`'s
    /// single `"menu_bar"` instance, every `"form"` extra painted this
    /// frame gets its own entry, keyed by its own id — a `"form"` is not
    /// a singleton overlay the way `"menu_bar"`/`"find_replace"` are.
    /// A cached entry whose id wasn't painted this frame is dropped, so a
    /// form that stops rendering doesn't keep answering events forever.
    ///
    /// Seeds a newly-created controller's own `scroll_offset` from the
    /// descriptor's `Form.scroll_offset` the first time its id is seen, so
    /// a host that paints a non-zero `scroll_offset` on insert (the
    /// contract the plain, non-controller `ExtraPaint::paint` path already
    /// honours) doesn't silently lose it to `FormController::new`'s `0`
    /// the moment the form gets wired through this cache. Only on
    /// creation, not every frame: from then on `FormController` owns
    /// `scroll_offset` itself (wheel/thumb-drag advance it internally, the
    /// same "host doesn't have to echo it back" contract
    /// `compose::form_controller`'s own module doc describes for the
    /// cursor) — reseeding it from the descriptor on every frame would
    /// snap a live scroll straight back to whatever stale value a host
    /// that never echoes it keeps sending.
    ///
    /// `has_focus`, by contrast, IS reconciled every frame: the
    /// descriptor's own `Form.has_focus` OR'd with whether `backend`'s
    /// focus manager currently names this form. Unlike `scroll_offset`,
    /// engine-level focus is already an authoritative value this engine
    /// tracks continuously, so ORing it in is never destructive of a
    /// legitimate internal source of truth the way reseeding
    /// `scroll_offset` would be — and it's what makes the focused-row
    /// highlight track engine focus in both directions: appearing the
    /// very next render after a click moves focus onto the form (not just
    /// after the first keystroke, the only other site that calls
    /// `set_has_focus`), and clearing once focus moves elsewhere even if
    /// the host keeps echoing a stale `focused_field`/`has_focus: true`.
    pub fn sync_forms(&self, backend: &mut dyn Backend, hits: &[PaintedHit]) {
        let mut seen = Vec::new();
        for hit in hits {
            let PaintedHit::Extra {
                id, area, extra, ..
            } = hit
            else {
                continue;
            };
            let Some(form) = extra_as::<Form>(extra) else {
                continue;
            };
            seen.push(*id);
            let engine_focused = backend.focus_manager().focused() == Some(id);
            let mut controllers = self.form_controllers.borrow_mut();
            let is_new = !controllers.contains_key(id);
            let controller = controllers
                .entry(*id)
                .or_insert_with(|| FormController::new(id.as_str().to_string()));
            controller.set_form(form.clone());
            if is_new {
                controller.set_scroll_offset(form.scroll_offset);
            }
            controller.set_has_focus(form.has_focus || engine_focused);
            controller.render_and_cache(backend, *area);
            self.form_rects.borrow_mut().insert(*id, *area);
        }
        self.form_controllers
            .borrow_mut()
            .retain(|id, _| seen.contains(id));
        self.form_rects
            .borrow_mut()
            .retain(|id, _| seen.contains(id));
        *self.form_order.borrow_mut() = seen;
    }

    /// Repaints every standalone `"text_input"` extra this frame whose
    /// cached [`quadraui::TextEditor`] (seeded by [`text_input_click_event`]/
    /// [`text_input_insert_event`]) disagrees with the descriptor's own
    /// `cursor_line`/`cursor_col` — the same "paint after everything else,
    /// from the richer live state" overlay [`Self::sync_forms`] uses for
    /// `"form"`. Without this, `text_input_click_event`'s `SetCursor`
    /// moves the caret only *logically*: the next keystroke starts from
    /// the clicked position, but nothing ever paints it there, since
    /// [`crate::extra_paint::ExtraPaint::paint`] for `TextInput` draws the
    /// raw descriptor — whose `cursor_col` is normally `0`, because
    /// `FormEvent::TextInputChanged`/`Committed` carry no cursor for a
    /// host to echo back onto it. Call once per render, after the extras
    /// loop already painted the plain input and after
    /// [`Self::sync_forms`] (order between the two doesn't matter — they
    /// never touch the same extra kind).
    pub fn sync_text_input_carets(&self, backend: &mut dyn Backend, hits: &[PaintedHit]) {
        let editors = self.text_editors.borrow();
        for hit in hits {
            let PaintedHit::Extra {
                id, area, extra, ..
            } = hit
            else {
                continue;
            };
            let Some(text_input) = extra_as::<TextInput>(extra) else {
                continue;
            };
            let Some(editor) = editors.get(id) else {
                continue;
            };
            let live = (editor.input.cursor_line, editor.input.cursor_col);
            if live == (text_input.cursor_line, text_input.cursor_col) {
                continue;
            }
            let mut repainted = text_input.clone();
            repainted.cursor_line = live.0;
            repainted.cursor_col = live.1;
            backend.draw_text_input(*area, &repainted);
        }
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

        // Set when a tooltip auto-dismisses below — merged into the final
        // `Reaction` (and threaded through `continue_or_tooltip_redraw`
        // for the handful of early `return`s further down whose own
        // outcome is "nothing changed") so the redraw that takes the
        // tooltip off screen always happens, regardless of which path
        // through this function the event takes.
        let mut tooltip_dismissed = false;

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
            *self.column_drag.borrow_mut() = None;
            self.chrome_pressed.borrow_mut().clear();
            // Mirrors the resets above for a `"form"` extra's own
            // scrollbar-thumb drag: `FormController::handle`'s `MouseUp`
            // arm clears `scroll_drag` unconditionally (it doesn't check
            // position), so trying every cached controller here releases
            // whichever one (if any) is mid-drag instead of leaving its
            // thumb rendered pressed forever (`build_scrollbar_lh`'s
            // `dragging` flag never clears otherwise).
            for id in self.form_order.borrow().iter() {
                let rect = self.form_rects.borrow().get(id).copied();
                if let Some(rect) = rect {
                    if let Some(controller) = self.form_controllers.borrow_mut().get_mut(id) {
                        controller.handle(&event, backend, rect);
                    }
                }
            }
        }

        // A tooltip is non-interactive chrome, not a modal-input state
        // machine — it never claims a click/key for itself, so this is a
        // side effect rather than an early return. Clearing it here (on
        // every `MouseDown`/`KeyPressed`, before anything else below gets
        // a chance to consume the event) is what "route dismissal" means
        // for a primitive with [`Self::render_tooltip`]'s "no hit data
        // returned" contract: the next interaction anywhere always hides
        // it, same as a native tooltip's hover-out behaviour.
        if matches!(
            event,
            UiEvent::MouseDown { .. } | UiEvent::KeyPressed { .. }
        ) && self.hide_tooltip()
        {
            // Not returned directly — `tooltip_dismissed` is merged into
            // the final `Reaction` below, so a click that both dismisses
            // the tooltip and goes on to do something else (e.g. focus a
            // button) still redraws for the dismissal even when nothing
            // else would have.
            tooltip_dismissed = true;
        }

        // An open toast stack is non-modal (unlike every overlay below)
        // but still paints on top of everything else this frame
        // (`render_toast` runs last of all `render_*` on this engine —
        // see its own doc), so a click on its dismiss `×`/action region
        // must claim the event before any of the modal overlays below
        // get a chance to, same "hit precedence agrees with paint order"
        // invariant `Self::dialog`'s own doc calls out. A click that
        // misses every toast (`ToastHit::Empty`) falls through
        // unconsumed to the modal/content routing below, since a toast
        // in a screen corner essentially never overlaps anything else
        // painted there.
        if !self.toasts.is_empty() {
            if let UiEvent::MouseDown { position, .. } = &event {
                match self.toast_layout(backend).hit_test(position.x, position.y) {
                    ToastHit::Dismiss(id) => {
                        self.dismiss_toast(id);
                        return Reaction::Redraw;
                    }
                    ToastHit::Action(action_id) => {
                        return Reaction::Redraw
                            .merge(sink.dispatch(&UiEvent::ToastActionClicked(action_id), None));
                    }
                    ToastHit::Body(_) => return continue_or_tooltip_redraw(tooltip_dismissed),
                    ToastHit::Empty => {}
                }
            }
        }

        // An open message dialog is modal and painted on top of everything
        // else this frame (`render_dialog` runs after `sync_menu_bar`/
        // `render_ctx_menu`), so it must see every event before the
        // menu-bar/context-menu routing just below — hit precedence has to
        // agree with paint order in both directions. `open_message_dialog`
        // closes any open dropdown/popup when the dialog opens, so this and
        // the menu-bar/context-menu checks below are never simultaneously
        // "open" in practice; the ordering here is what makes that
        // invariant hold even if that changes.
        // `MessageDialogController::handle` reports `Ignored` immediately
        // when nothing is open, so this is safe to run unconditionally.
        if let Some(dialog) = self.dialog.as_mut() {
            match dialog.handle(&event, &*backend) {
                MessageDialogEvent::Resolved(choice) => {
                    // The dialog is closing regardless of what `sink`
                    // itself returns — force the repaint that takes it off
                    // screen, same as the `ContextMenuOutcome::Event` arm
                    // below.
                    self.close_message_dialog(backend);
                    return Reaction::Redraw
                        .merge(sink.dispatch(&UiEvent::MessageDialogResolved(choice), None));
                }
                MessageDialogEvent::Consumed => return Reaction::Redraw,
                MessageDialogEvent::Ignored => {}
            }
        }

        // An open completions popup claims Escape (dismiss) and a
        // `MouseDown` landing anywhere — on an item (reports the
        // activation), elsewhere inside the popup (swallowed — it's
        // still "on the popup"), or outside it (dismisses, same as a
        // context-menu's click-outside). Checked after the dialog (so an
        // open dialog keeps its own higher hit precedence if an app ever
        // has both open) but before the menu-bar/context-menu routing
        // below, matching [`Self::render_completions`]'s "before
        // `Self::render_dialog`" paint-order doc.
        if let Some((completions, cursor)) = self.completions.clone() {
            match &event {
                UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Escape),
                    ..
                } => {
                    self.dismiss_completions();
                    return Reaction::Redraw;
                }
                UiEvent::MouseDown { position, .. } => {
                    let layout = self.completions_layout(backend, &completions, cursor);
                    match layout.hit_test(position.x, position.y) {
                        CompletionsHit::Item(idx) => {
                            self.dismiss_completions();
                            return Reaction::Redraw.merge(sink.dispatch(
                                &UiEvent::CompletionItemActivated(completions.id, idx),
                                None,
                            ));
                        }
                        CompletionsHit::Inert => {
                            return continue_or_tooltip_redraw(tooltip_dismissed)
                        }
                        CompletionsHit::Empty => {
                            self.dismiss_completions();
                            return Reaction::Redraw;
                        }
                    }
                }
                _ => {}
            }
        }

        // An open rich-text popup claims Escape (dismiss) and a
        // `MouseDown` the same way completions does above: a link hit
        // reports the click, any other in-bounds hit is swallowed, and a
        // miss dismisses the popup.
        if let Some((popup, anchor)) = self.rich_text_popup.clone() {
            match &event {
                UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Escape),
                    ..
                } => {
                    self.dismiss_rich_text_popup();
                    return Reaction::Redraw;
                }
                UiEvent::MouseDown { position, .. } => {
                    let layout = self.rich_text_popup_layout(backend, &popup, anchor);
                    match layout.hit_test(position.x, position.y) {
                        RichTextPopupHit::Link(idx) => {
                            let url = popup
                                .links
                                .get(idx)
                                .map(|l| l.url.clone())
                                .unwrap_or_default();
                            return Reaction::Redraw.merge(
                                sink.dispatch(&UiEvent::RichTextLinkClicked(popup.id, url), None),
                            );
                        }
                        RichTextPopupHit::Outside => {
                            self.dismiss_rich_text_popup();
                            return Reaction::Redraw;
                        }
                        RichTextPopupHit::Char(..)
                        | RichTextPopupHit::ScrollbarTrack
                        | RichTextPopupHit::ScrollbarThumb
                        | RichTextPopupHit::Body => {
                            return continue_or_tooltip_redraw(tooltip_dismissed)
                        }
                    }
                }
                _ => {}
            }
        }

        // An open float claims Escape (dismiss) only when it is
        // focusable — a non-focusable hint float never steals keys, same
        // distinction [`quadraui::ModalStack::top_focusable`]'s own doc
        // draws. It does NOT claim mouse clicks here: its content is
        // painted by the host, not this engine (see `Self::show_float`'s
        // doc), so a click anywhere near it has to keep flowing through
        // the normal hit-map routing below for that content to ever
        // receive it.
        if let Some((float, _)) = &self.float {
            if float.focusable
                && matches!(
                    &event,
                    UiEvent::KeyPressed {
                        key: Key::Named(NamedKey::Escape),
                        ..
                    }
                )
            {
                self.dismiss_float(backend);
                return Reaction::Redraw;
            }
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

        // A `KeyPressed`/`CharTyped`/`ClipboardPaste` landing on a `"form"`
        // extra that currently holds engine-level focus routes through
        // that `Form`'s own cached `FormController` — `FormController::handle`'s
        // own `has_focus` gate (see its module doc) is what makes this
        // safe to try unconditionally even when no `"form"` extra is
        // focused (or none exists this frame): `form_rects`/`form_controllers`
        // then has no entry for the focused id, or `set_has_focus(true)`
        // simply enables a gate nothing else reads. Only `FormAction`
        // replaces `event`; `ScrollChanged`/`Consumed`/`Ignored` never
        // arise for these three event kinds in `FormController::handle_inner`'s
        // own match, so there is nothing else to react to here. Tab/
        // Shift+Tab field cycling inside a form is NOT reachable this way —
        // `quadraui::runtime::preprocess_event` claims `Tab`/`BackTab`
        // unconditionally for the app-level tab order whenever `tab_stops()`
        // is non-empty, before an event ever reaches here.
        if matches!(
            event,
            UiEvent::KeyPressed { .. } | UiEvent::CharTyped(_) | UiEvent::ClipboardPaste(_)
        ) {
            if let Some(focused) = backend.focus_manager().focused().copied() {
                if let Some(rect) = self.form_rects.borrow().get(&focused).copied() {
                    if let Some(controller) = self.form_controllers.borrow_mut().get_mut(&focused) {
                        controller.set_has_focus(true);
                        if let FormControllerEvent::FormAction(form_event) =
                            controller.handle(&event, backend, rect)
                        {
                            event = UiEvent::Form(focused, form_event);
                        }
                    }
                }
            }
        }

        // A `MouseDown` lands on at most one `"form"` extra's own `rect`
        // (`FormController::handle`'s own `click_inner` returns `Ignored`,
        // with no mutation, for every other one — see its doc), so trying
        // every cached controller, in `form_order`'s deterministic paint
        // order, and stopping at the first non-`Ignored` result is safe
        // regardless of how many `"form"` extras are
        // painted this frame. This runs ahead of (and independently of)
        // the generic hit-test/click-to-focus pipeline below, since
        // `FormController` resolves which FIELD within the form a click
        // landed on using its own internal layout — the generic pipeline
        // only ever sees the form AS A WHOLE (one `WidgetId`, via
        // `ExtraPaint::hit_test`'s whole-area check).
        if matches!(event, UiEvent::MouseDown { .. }) {
            let claim = self
                .form_order
                .borrow()
                .iter()
                .filter_map(|id| self.form_rects.borrow().get(id).map(|rect| (*id, *rect)))
                .collect::<Vec<_>>()
                .into_iter()
                .find_map(|(id, rect)| {
                    let outcome = self
                        .form_controllers
                        .borrow_mut()
                        .get_mut(&id)
                        .map(|controller| controller.handle(&event, backend, rect))?;
                    (!matches!(outcome, FormControllerEvent::Ignored)).then_some((id, outcome))
                });
            if let Some((id, outcome)) = claim {
                // Mirrors the generic click-to-focus block below: a click
                // a `FormController` just claimed also moves engine-level
                // focus to the form's own id, so the `KeyPressed`/
                // `CharTyped`/`ClipboardPaste` routing above can find it
                // on the very next keystroke.
                let mut focus_reaction = Reaction::Continue;
                if backend.set_focus(Some(id)) {
                    focus_reaction = sink.dispatch(&UiEvent::FocusChanged(Some(id)), None);
                    if matches!(focus_reaction, Reaction::Exit) {
                        return focus_reaction;
                    }
                }
                return match outcome {
                    FormControllerEvent::FormAction(form_event) => focus_reaction.merge(
                        Reaction::Redraw.merge(sink.dispatch(&UiEvent::Form(id, form_event), None)),
                    ),
                    FormControllerEvent::ScrollChanged | FormControllerEvent::Consumed => {
                        focus_reaction.merge(Reaction::Redraw)
                    }
                    FormControllerEvent::Ignored => unreachable!("filtered out above"),
                };
            }
        }

        // A move with the left button held continues whichever `"form"`
        // extra's own scrollbar-thumb drag (if any) the `MouseDown` claim
        // above armed — `FormController::handle`'s `MouseMoved` arm
        // no-ops (`Ignored`) for every controller with no `scroll_drag` of
        // its own, so trying all of them here is safe regardless of which
        // one (if any) is actually dragging. Unlike the generic
        // divider/scrollbar drags above, `FormController`'s own
        // `scroll_offset` is internal state the host never needs to
        // mirror (same contract as its text cursor), so this never swaps
        // `event` into a semantic substitute — it just needs to run so the
        // redraw after a `Consumed` result shows the new offset.
        if matches!(
            event,
            UiEvent::MouseMoved {
                buttons: quadraui::ButtonMask { left: true, .. },
                ..
            }
        ) {
            let mut consumed = false;
            for id in self.form_order.borrow().iter() {
                let rect = self.form_rects.borrow().get(id).copied();
                if let Some(rect) = rect {
                    if let Some(controller) = self.form_controllers.borrow_mut().get_mut(id) {
                        if matches!(
                            controller.handle(&event, backend, rect),
                            FormControllerEvent::Consumed | FormControllerEvent::ScrollChanged
                        ) {
                            consumed = true;
                        }
                    }
                }
            }
            if consumed {
                return Reaction::Redraw;
            }
        }

        // A wheel `Scroll` landing inside a `"form"` extra's own rect
        // scrolls it directly through the cached `FormController` — unlike
        // the tree/text-display wheel-scroll translation below, this never
        // swaps `event` into a semantic substitute (the host never needs
        // to mirror `FormController`'s internal `scroll_offset`, same as
        // the drag-continuation case just above), so a hit is consumed
        // outright here rather than falling through to the generic
        // hit-test/semantic pipeline.
        if let UiEvent::Scroll { position, .. } = &event {
            let claim = self
                .form_order
                .borrow()
                .iter()
                .filter_map(|id| self.form_rects.borrow().get(id).map(|rect| (*id, *rect)))
                .collect::<Vec<_>>()
                .into_iter()
                .find(|(_, rect)| rect.contains(*position));
            if let Some((id, rect)) = claim {
                if let Some(controller) = self.form_controllers.borrow_mut().get_mut(&id) {
                    if matches!(
                        controller.handle(&event, backend, rect),
                        FormControllerEvent::Consumed | FormControllerEvent::ScrollChanged
                    ) {
                        return Reaction::Redraw;
                    }
                }
            }
        }

        // Chrome-bar (`Toolbar`/`StatusBar`/`ActivityBar`) hover bookkeeping,
        // read back by `chrome_interaction` so `render` paints real hover
        // state. Handled directly via `chrome_hover_hit` rather than the
        // `hit`/`hit_test_painted` computed below, since `event_position`
        // never resolves a position for `MouseMoved` (enriching its own
        // `widget` field isn't this module's job) — so `hit_test_painted`
        // never runs for it either. A hover transition folds a `Redraw`
        // into `menu_reaction` (merged into the final `Reaction` below) —
        // otherwise a slot-based `insert_*` host with no render-callback
        // promotion (unlike `quadraui-py`'s `reaction_for_callback`) would
        // record the new hovered child and never actually repaint it.
        if let UiEvent::MouseMoved { position, .. } = &event {
            let chrome_hit = chrome_hover_hit(&self.hit_map.borrow(), *position);
            let mut hover = self.chrome_hover.borrow_mut();
            let before = hover.clone();
            hover.retain(|bar, _| chrome_hit.map(|(hit_bar, _)| hit_bar) == Some(*bar));
            if let Some((bar, action)) = chrome_hit {
                hover.insert(bar, action);
            }
            if *hover != before {
                menu_reaction = menu_reaction.merge(Reaction::Redraw);
            }
        }

        // A move while a divider drag is armed becomes the drag itself —
        // checked before the hit-test below, since `MouseMoved` has no
        // `widget` field for `event_position`/`set_event_widget` to touch.
        // A move while a tree scrollbar thumb drag is armed instead becomes
        // a `ScrollOffsetChanged`, and a move while a `DataTable` column
        // divider drag is armed becomes a `ColumnResized`, mirroring the
        // divider case; all three drags are mutually exclusive, so only one
        // of these branches ever fires for a given move.
        if let UiEvent::MouseMoved { position, .. } = &event {
            if let Some(drag) = self.active_drag.borrow().as_ref() {
                if let Some(new_ratio) = drag.ratio_at(*position) {
                    event = UiEvent::SplitDividerDragged {
                        tree: drag.tree(),
                        split_index: drag.split_index(),
                        new_ratio,
                    };
                }
            } else if let Some(drag) = self.scrollbar_drag.borrow().as_ref() {
                if let Some(new_offset) = drag.drag.offset_at(position.y) {
                    let new_offset = new_offset.round() as usize;
                    event = UiEvent::ScrollOffsetChanged {
                        widget: drag.widget,
                        new_offset,
                    };
                }
            } else if let Some(drag) = self.column_drag.borrow().as_ref() {
                event = UiEvent::DataTable(
                    drag.widget,
                    DataTableEvent::ColumnResized {
                        col: drag.col,
                        width: drag.width_at(*position),
                    },
                );
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
                    &self.text_editors,
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
                    &self.text_editors,
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
                    &self.text_editors,
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

        // A `MouseDown` landing on a chrome-bar child presses it, mirroring
        // `Self::chrome_hover`'s bookkeeping above — cleared unconditionally
        // on the next `MouseUp` (top of this function), regardless of
        // where that `MouseUp` lands.
        if let (UiEvent::MouseDown { .. }, Some((action, HitDetail::ExtraChrome { bar }))) =
            (&event, &hit)
        {
            self.chrome_pressed.borrow_mut().insert(*bar, *action);
        }

        // A wheel `Scroll` landing anywhere over a tree (its rows, its
        // chevrons, or its scrollbar) replaces the raw event outright with
        // the `ScrollOffsetChanged` it resolves to — never dispatched
        // alongside it — mirroring the `MouseDown`/`DoubleClick` semantic
        // replacement below, including suppressing the `"hit"` attachment
        // via `scroll_translated`, below. A tree with nothing to scroll
        // (all rows fit the viewport) has no scrollbar, so
        // `tree_scroll_state` returns `None` and the raw `Scroll` passes
        // through untouched. A `TextDisplay` scrolls the same way, but
        // into its own `TextDisplayEvent::Scrolled` — the primitive
        // already has a typed scroll event, unlike `Tree`, so there is no
        // need for the generic `ScrollOffsetChanged` substitute.
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
                let over_text_display = matches!(
                    detail,
                    HitDetail::TextDisplayLine { .. }
                        | HitDetail::TextDisplayScrollbarThumb
                        | HitDetail::TextDisplayScrollbarTrackBefore
                        | HitDetail::TextDisplayScrollbarTrackAfter
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
                } else if over_text_display {
                    if let Some((total_lines, viewport_lines, scroll_offset)) =
                        text_display_scroll_state(&self.hit_map.borrow(), *id)
                    {
                        let max_offset = total_lines.saturating_sub(viewport_lines);
                        let rows = wheel_row_delta(delta.y, 1.0);
                        let new_offset = scroll_by_clamped(scroll_offset, rows, max_offset);
                        event =
                            UiEvent::TextDisplay(*id, TextDisplayEvent::Scrolled { new_offset });
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
                // A non-double click landing on ANY extra's body — not just
                // `TextInput`/`NumberInput` — gets `HitDetail::ExtraBody`
                // (`hit_test_painted`'s own `Extra` arm doesn't distinguish
                // by kind), so this is the first arm of the same `if`/
                // `else if` chain the other `detail` variants use below,
                // with its own `semantic_event_for_hit` fallback folded in
                // rather than left to the chain's final arm — a `Switch`
                // body click (which `text_input_click_event`/
                // `number_input_click_event` both no-op on, since neither
                // finds the kind it looks for) still resolves through
                // `semantic_event_for_hit`'s own `ExtraBody` arm this way.
                // Folding every `ExtraBody` outcome into one arm up front
                // makes it structurally impossible for two of them to fire
                // for the same click as a future kind is wired in here.
                // Must run before `semantic_event_for_hit` is reached,
                // since `TextInput`'s caret placement is a side effect on
                // `text_editors` only (a `SetCursor` never changes the
                // buffer, so there is no event to swap in; see
                // `text_input_click_event`'s own doc) and `NumberInput`'s
                // stepper click DOES swap `event` outright.
                if !double_click && matches!(detail, HitDetail::ExtraBody) {
                    if let Some(position) = event_position(&event) {
                        text_input_click_event(
                            &self.hit_map.borrow(),
                            *id,
                            position,
                            quadraui::TextInputMeasure::from_metrics(&backend.measure()),
                            &self.text_editors,
                        );
                        if let Some(number_input_event) =
                            number_input_click_event(&self.hit_map.borrow(), *id, position)
                        {
                            event = number_input_event;
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
                } else if !double_click && matches!(detail, HitDetail::SplitDivider) {
                    if let UiEvent::MouseDown { position, .. } = &event {
                        if let Some(layout) =
                            find_split_layout(&self.hit_map.borrow(), *position, *id)
                        {
                            *self.active_drag.borrow_mut() =
                                Some(ActiveDrag::Split { tree: *id, layout });
                        }
                    }
                } else if !double_click && matches!(detail, HitDetail::SplitTreeDivider { .. }) {
                    // Mirrors the `SplitDivider` arm above: arms a drag
                    // (consumed by the `MouseMoved` branch above on later
                    // moves) for the N-way `SplitTree` case instead of the
                    // two-pane `Split` one, keyed by the dragged divider's
                    // own `split_index` rather than always `0`.
                    if let HitDetail::SplitTreeDivider { split_index } = detail {
                        if let Some(layout) = find_split_tree_layout(&self.hit_map.borrow(), *id) {
                            *self.active_drag.borrow_mut() = Some(ActiveDrag::SplitTree {
                                tree: *id,
                                split_index: *split_index,
                                layout,
                            });
                        }
                    }
                } else if !double_click
                    && matches!(detail, HitDetail::DataTableHeaderDivider { .. })
                {
                    // Mirrors the `SplitDivider` arm above: arms the drag
                    // (consumed by the `MouseMoved` branch above on later
                    // moves) but leaves the raw `MouseDown` (and its
                    // `"hit"` dict) to reach `sink` unchanged below, same as
                    // a split-divider press.
                    if let HitDetail::DataTableHeaderDivider { col } = detail {
                        if let Some((area, layout)) =
                            find_data_table_layout(&self.hit_map.borrow(), *id)
                        {
                            *self.column_drag.borrow_mut() = Some(DataTableColumnDrag {
                                widget: *id,
                                col: *col,
                                area,
                                layout,
                            });
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
        let tooltip_reaction = if tooltip_dismissed {
            Reaction::Redraw
        } else {
            Reaction::Continue
        };
        tooltip_reaction
            .merge(menu_reaction)
            .merge(focus_reaction)
            .merge(event_reaction)
    }
}

/// Builds a [`MessageDialogOptions`] from a binding's own decoded
/// `title`/`body`/`buttons`/`severity` arguments, validating `buttons` the
/// way every binding's own `show_message_dialog`-style surface needs to:
/// non-empty (a dialog with no buttons can never be resolved by Enter or
/// Escape) and no button setting `tint` (accepted for shape compatibility
/// with the in-canvas `Dialog` primitive's own `buttons` field, but not yet
/// honored by [`MessageDialogController`]). `Err` carries the message a
/// binding's own "invalid input" error wraps.
pub fn message_dialog_options_from_buttons(
    title: String,
    body: String,
    buttons: Vec<quadraui::DialogButton>,
    severity: Option<quadraui::DialogSeverity>,
) -> Result<MessageDialogOptions, String> {
    if buttons.is_empty() {
        return Err(
            "buttons must not be empty — a dialog with no buttons can never be resolved by \
             Enter or Escape"
                .to_string(),
        );
    }
    if let Some(tinted) = buttons.iter().find(|b| b.tint.is_some()) {
        return Err(format!(
            "button {:?} sets tint, which this surface does not honor yet — omit tint or \
             leave it unset",
            tinted.id.as_str()
        ));
    }
    Ok(MessageDialogOptions {
        title,
        body,
        buttons: buttons
            .into_iter()
            .map(|b| MessageDialogButton {
                id: b.id,
                label: b.label,
                is_default: b.is_default,
                is_cancel: b.is_cancel,
            })
            .collect(),
        severity,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use quadraui::tui::testing::TuiDriver;
    use quadraui::{
        AppLogic, Backend, Button, DataTable, Modifiers, MouseButton, Point, Reaction, Rect,
        UiEvent, WidgetId,
    };

    use super::{EventSink, InteractionEngine};
    use crate::drag::find_split_tree_layout;
    use crate::hit::HitDetail;
    use crate::layout::{LayoutLeaf, LayoutNode, LayoutPanel, LayoutSplitTree, LeafDescriptor};
    use crate::render::{draw_bands, paint_layout, RenderRefs};

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
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.render_dialog(backend);
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

    /// Mirrors [`MinimalLogic`], but paints one `"form"` extra through
    /// [`InteractionEngine::sync_forms`] instead of a plain `Button`, so a
    /// test can drive `"form"`-specific routing (scroll, drag, click)
    /// without any host-language FFI type in scope.
    struct FormLogic {
        form: quadraui::Form,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for FormLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let mut extra: HashMap<String, std::rc::Rc<dyn crate::extra_paint::ExtraPaint>> =
                HashMap::new();
            extra.insert(
                "form".to_string(),
                std::rc::Rc::new(self.form.clone())
                    as std::rc::Rc<dyn crate::extra_paint::ExtraPaint>,
            );
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
            );
            self.engine.sync_forms(backend, &hits);
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

    /// A wheel `Scroll` landing inside a `"form"` extra's own rect must
    /// advance its cached [`quadraui::compose::FormController`]'s own
    /// `scroll_offset` — regression coverage for the `MouseUp`/`MouseMoved`/
    /// `Scroll` routing [`InteractionEngine::handle`] added alongside the
    /// `MouseDown` claim loop above it.
    #[test]
    fn wheel_scroll_over_a_form_advances_its_scroll_offset() {
        use quadraui::{FieldKind, FormField, ScrollDelta, StyledText};

        let fields = (0..10)
            .map(|i| FormField {
                id: WidgetId::new(format!("f{i}")),
                label: StyledText::plain(format!("Field{i}")),
                kind: FieldKind::Label,
                hint: StyledText::plain(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
            })
            .collect();
        let form = quadraui::Form {
            id: WidgetId::new("settings"),
            fields,
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
        };

        let logic = FormLogic {
            form,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta { x: 0.0, y: -1.0 },
            position: Point::new(1.0, 1.0),
        });

        let (_backend, app) = driver.parts_mut();
        let offset = app
            .engine
            .form_controllers
            .borrow()
            .get(&WidgetId::new("settings"))
            .map(|fc| fc.scroll_offset());
        assert_eq!(offset, Some(1), "wheel scroll should advance scroll_offset");
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

    /// A message dialog opened via [`InteractionEngine::open_message_dialog`]
    /// intercepts an Enter keypress before any of the rest of
    /// [`InteractionEngine::handle`]'s own routing runs (the painted
    /// `Button` behind it never sees the key), dispatches
    /// `UiEvent::MessageDialogResolved` with the default button's id, and
    /// pops its [`quadraui::ModalStack`] entry on resolution — the same
    /// "owns every event while open, closes cleanly on resolve" contract
    /// this module's own context-menu coverage already proves for
    /// `ctx_menu`. The click path (hit-testing the dialog's own button
    /// layout) is covered black-box, through the real binding boundary, by
    /// `quadraui-py`'s `show_message_dialog_then_clicking_a_button_emits_message_dialog_resolved`.
    #[test]
    fn open_message_dialog_then_pressing_enter_resolves_the_default_button_and_pops_the_modal_stack(
    ) {
        use quadraui::backend::{MessageDialogButton, MessageDialogOptions};
        use quadraui::{Key, NamedKey};

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        let opts = MessageDialogOptions {
            title: "Unsaved changes".to_string(),
            body: "Save before closing?".to_string(),
            buttons: vec![
                MessageDialogButton {
                    id: WidgetId::new("save"),
                    label: "Save".to_string(),
                    is_default: true,
                    is_cancel: false,
                },
                MessageDialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".to_string(),
                    is_default: false,
                    is_cancel: true,
                },
            ],
            severity: None,
        };
        {
            let (backend, app) = driver.parts_mut();
            app.engine.open_message_dialog(opts, backend);
            assert!(
                backend.modal_stack_handle().borrow().top().is_some(),
                "opening the dialog must push a ModalStack entry"
            );
        }
        driver.render();

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert_eq!(
                dispatched.len(),
                1,
                "resolving the dialog dispatches exactly one event: {:?}",
                dispatched
            );
            assert!(
                matches!(
                    dispatched[0].0,
                    UiEvent::MessageDialogResolved(id) if id == WidgetId::new("save")
                ),
                "expected MessageDialogResolved(save): {:?}",
                dispatched[0]
            );
        }

        let (backend, app) = driver.parts_mut();
        assert!(
            !app.engine.dialog_open(),
            "the dialog must close once resolved"
        );
        assert!(
            backend.modal_stack_handle().borrow().top().is_none(),
            "resolving the dialog must pop its ModalStack entry"
        );
    }

    /// Opening a message dialog while a context-menu popup is already
    /// open closes the popup: paint order puts the dialog on top
    /// (`render_dialog` runs after `render_ctx_menu`), and [`Self::handle`]
    /// now checks the dialog first, so a popup left open underneath would
    /// be unreachable input it still held precedence over. Pins the fix
    /// for the "both stay open, the popup keeps event priority" defect.
    #[test]
    fn open_message_dialog_closes_an_open_context_menu_popup() {
        use quadraui::backend::{MessageDialogButton, MessageDialogOptions};
        use quadraui::{ContextMenu, ContextMenuItem, Key, NamedKey};

        let logic = MinimalLogic {
            button: Button::new(WidgetId::new("go"), "Go"),
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 20);

        {
            let (backend, app) = driver.parts_mut();
            let menu = ContextMenu {
                id: WidgetId::new("popup"),
                items: vec![ContextMenuItem {
                    id: Some(WidgetId::new("copy")),
                    label: quadraui::types::StyledText::plain("Copy"),
                    ..Default::default()
                }],
                selected_idx: 0,
                bg: None,
                placement: quadraui::ContextMenuPlacement::AnchorPoint,
            };
            app.engine
                .ctx_menu_mut()
                .open(menu, Point::new(1.0, 1.0), backend);
            assert!(
                app.engine.ctx_menu_mut().is_open(),
                "the popup must be open before the dialog opens"
            );
        }

        let opts = MessageDialogOptions {
            title: "Heads up".to_string(),
            body: "Something happened.".to_string(),
            buttons: vec![MessageDialogButton {
                id: WidgetId::new("ok"),
                label: "OK".to_string(),
                is_default: true,
                is_cancel: false,
            }],
            severity: None,
        };
        {
            let (backend, app) = driver.parts_mut();
            app.engine.open_message_dialog(opts, backend);
            assert!(
                !app.engine.ctx_menu_mut().is_open(),
                "opening the dialog must close the popup painted underneath it"
            );
        }
        driver.render();

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert_eq!(
            dispatched.len(),
            1,
            "Enter must resolve the dialog, not reach the already-closed popup: {:?}",
            dispatched
        );
        assert!(
            matches!(
                dispatched[0].0,
                UiEvent::MessageDialogResolved(id) if id == WidgetId::new("ok")
            ),
            "expected MessageDialogResolved(ok), not a context-menu event: {:?}",
            dispatched[0]
        );
    }

    /// A minimal [`AppLogic`] painting a single `DataTable`, for the
    /// header-click/divider-drag/keyboard-nav coverage below — mirrors
    /// [`MinimalLogic`], swapping the button band for a data-table one.
    struct DataTableLogic {
        data_table: DataTable,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for DataTableLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: None,
                    list: None,
                    tree: None,
                    data_table: Some(&self.data_table),
                    tab_bar: None,
                    extra: &extra,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
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

    /// Builds a two-fixed-column, three-row `DataTable` and drives it
    /// through a [`TuiDriver`], for the header-click/divider-drag/
    /// keyboard-nav tests below to aim a click or drag at exact geometry
    /// (read back via [`data_table_geometry`]) rather than hardcoding
    /// coordinates.
    fn data_table_driver(selected_idx: Option<usize>) -> TuiDriver<DataTableLogic> {
        use quadraui::{Column, ColumnWidth, DataRow, StyledText};

        let data_table = DataTable {
            id: WidgetId::new("table"),
            columns: vec![
                Column {
                    title: "Name".to_string(),
                    width: ColumnWidth::Fixed(8.0),
                    align: Default::default(),
                },
                Column {
                    title: "Age".to_string(),
                    width: ColumnWidth::Fixed(8.0),
                    align: Default::default(),
                },
            ],
            rows: vec![
                DataRow {
                    cells: vec![StyledText::plain("Alice"), StyledText::plain("30")],
                    decoration: Default::default(),
                },
                DataRow {
                    cells: vec![StyledText::plain("Bob"), StyledText::plain("40")],
                    decoration: Default::default(),
                },
                DataRow {
                    cells: vec![StyledText::plain("Cleo"), StyledText::plain("50")],
                    decoration: Default::default(),
                },
            ],
            selected_idx,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
        };
        let logic = DataTableLogic {
            data_table,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 24, 8)
    }

    /// The painted `DataTable`'s own area/layout, read back from the hit
    /// map the same way [`click_on_a_button_focuses_it_then_dispatches_the_click`]
    /// reads a button's bounds — so a test aims at exact geometry instead
    /// of hardcoding coordinates.
    fn data_table_geometry(
        driver: &TuiDriver<DataTableLogic>,
    ) -> (Rect, quadraui::DataTableLayout) {
        let hits = driver.app().engine.hit_map();
        hits.iter()
            .find_map(|hit| match hit {
                crate::hit::PaintedHit::DataTable { area, layout, .. } => {
                    Some((*area, layout.clone()))
                }
                _ => None,
            })
            .expect("data table should have painted a hit")
    }

    /// Clicking a column header must emit `DataTableEvent::HeaderClicked`
    /// (the sort request a Python app toggles into ascending/descending),
    /// never a raw `MouseDown` with a `data_table_header` hit dict left for
    /// the handler to re-interpret itself.
    #[test]
    fn clicking_a_data_table_header_emits_header_clicked() {
        use quadraui::DataTableEvent;

        let mut driver = data_table_driver(None);
        let (area, layout) = data_table_geometry(&driver);
        let header_point = Point::new(
            area.x + layout.columns[0].x + layout.columns[0].width / 2.0,
            area.y + layout.header_height / 2.0,
        );

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: header_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::HeaderClicked { col: 0 })
                    if *id == WidgetId::new("table")
            )),
            "expected a HeaderClicked sort request: {:?}",
            dispatched
        );
    }

    /// Pressing down on a header divider and dragging right must emit
    /// `DataTableEvent::ColumnResized` for the column to its left, growing
    /// its width as the pointer moves — the column-resize drag state
    /// mirroring `ActiveDrag`'s split-divider handling.
    #[test]
    fn dragging_a_header_divider_emits_column_resized() {
        use quadraui::DataTableEvent;

        let mut driver = data_table_driver(None);
        let (area, layout) = data_table_geometry(&driver);
        let original_width = layout.columns[0].width;
        let divider_y = area.y + layout.header_height / 2.0;
        let divider_point = Point::new(area.x + layout.columns[0].x + original_width, divider_y);

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: divider_point,
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(divider_point.x + 5.0, divider_y),
            buttons: quadraui::ButtonMask {
                left: true,
                right: false,
                middle: false,
            },
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        let resized = dispatched.iter().find_map(|(event, _)| match event {
            UiEvent::DataTable(id, DataTableEvent::ColumnResized { col, width })
                if *id == WidgetId::new("table") =>
            {
                Some((*col, *width))
            }
            _ => None,
        });
        let (col, width) = resized
            .unwrap_or_else(|| panic!("expected a ColumnResized event, got: {:?}", dispatched));
        assert_eq!(col, 0, "the divider drags the column to its left");
        assert!(
            width > original_width,
            "dragging right must grow the column: original={original_width}, new={width}"
        );
    }

    /// Down-arrow navigation on a focused `DataTable` must move the row
    /// selection by one (`DataTableEvent::RowSelected`), not fall through
    /// as a widget-less raw `UiEvent::KeyPressed` the handler has no way to
    /// route back to the table that actually has focus.
    #[test]
    fn down_arrow_on_a_focused_data_table_selects_the_next_row() {
        use quadraui::{DataTableEvent, Key, NamedKey};

        let mut driver = data_table_driver(Some(0));
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("table"))),
                "the data table is a tab stop and must accept focus"
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::RowSelected { idx: 1 })
                    if *id == WidgetId::new("table")
            )),
            "expected RowSelected {{ idx: 1 }}: {:?}",
            dispatched
        );
    }

    /// Enter on a focused `DataTable` with a selected row must activate
    /// that row (`DataTableEvent::RowActivated`) — the keyboard equivalent
    /// of a pointer double-click, so an app wired to `RowActivated` for
    /// "open" also works from the keyboard, not just a mouse.
    #[test]
    fn enter_on_a_focused_data_table_activates_the_selected_row() {
        use quadraui::{DataTableEvent, Key, NamedKey};

        let mut driver = data_table_driver(Some(0));
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("table"))),
                "the data table is a tab stop and must accept focus"
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::RowActivated { idx: 0 })
                    if *id == WidgetId::new("table")
            )),
            "expected RowActivated {{ idx: 0 }}: {:?}",
            dispatched
        );
    }

    /// Enter on a focused `DataTable` with nothing selected must be a
    /// no-op — there is no row to activate, so it must not fall through
    /// to a raw `UiEvent::KeyPressed` the handler has no table to route
    /// it back to, and must not activate row 0 by default the way
    /// Up/Down's `RowSelected` navigation does.
    #[test]
    fn enter_on_a_focused_data_table_with_no_selection_is_a_no_op() {
        use quadraui::{DataTableEvent, Key, NamedKey};

        let mut driver = data_table_driver(None);
        driver.render();
        {
            let (backend, _app) = driver.parts_mut();
            assert!(
                backend.set_focus(Some(WidgetId::new("table"))),
                "the data table is a tab stop and must accept focus"
            );
        }

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            !dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(_, DataTableEvent::RowActivated { .. })
            )),
            "no selection means no row to activate: {:?}",
            dispatched
        );
    }

    /// A single click on a `DataTable` row must emit
    /// `DataTableEvent::RowSelected`, not `RowActivated` — the "select"
    /// event, so a row body click never opens anything on its own.
    #[test]
    fn single_click_on_a_data_table_row_emits_row_selected() {
        use quadraui::DataTableEvent;

        let mut driver = data_table_driver(None);
        driver.render();
        let (area, layout) = data_table_geometry(&driver);
        let row_point = Point::new(area.x + 2.0, area.y + layout.header_height + 0.5);

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: row_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::RowSelected { idx: 0 })
                    if *id == WidgetId::new("table")
            )),
            "expected RowSelected {{ idx: 0 }}: {:?}",
            dispatched
        );
        assert!(
            !dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(_, DataTableEvent::RowActivated { .. })
            )),
            "a single click must not emit RowActivated: {:?}",
            dispatched
        );
    }

    /// A double-click on a `DataTable` row must emit the distinct
    /// `DataTableEvent::RowDoubleClicked`, not `RowSelected`/`RowActivated`
    /// — the "open" event a file-manager-style app should listen for
    /// instead of opening on every single click.
    #[test]
    fn double_click_on_a_data_table_row_emits_row_double_clicked() {
        use quadraui::DataTableEvent;

        let mut driver = data_table_driver(None);
        driver.render();
        let (area, layout) = data_table_geometry(&driver);
        let row_point = Point::new(area.x + 2.0, area.y + layout.header_height + 0.5);

        driver.dispatch(UiEvent::DoubleClick {
            widget: None,
            position: row_point,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::DataTable(id, DataTableEvent::RowDoubleClicked { idx: 0 })
                    if *id == WidgetId::new("table")
            )),
            "expected RowDoubleClicked {{ idx: 0 }}: {:?}",
            dispatched
        );
    }

    /// A minimal [`AppLogic`] painting a single `TextDisplay`, for the
    /// titled-display hit-test/wheel-scroll coverage below — mirrors
    /// [`DataTableLogic`], swapping the data-table band for a text-display
    /// one.
    struct TextDisplayLogic {
        text_display: quadraui::TextDisplay,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for TextDisplayLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let extra = HashMap::new();
            draw_bands(
                backend,
                RenderRefs {
                    button: None,
                    text_display: Some(&self.text_display),
                    list: None,
                    tree: None,
                    data_table: None,
                    tab_bar: None,
                    extra: &extra,
                },
                &mut hits,
                &|id| self.engine.chrome_interaction(id),
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

    /// Builds a titled, 5-line `TextDisplay` (more lines than the fixed
    /// 3-line band can show once the title row eats into it) and drives it
    /// through a [`TuiDriver`] — the regression shape for the body-offset
    /// bug: every backend's `text_display_layout` lays the body out from
    /// `0.0` and leaves the title row to be painted separately above it
    /// (see [`crate::hit::PaintedHit::TextDisplay`]'s own doc), so a hit
    /// test that only subtracts `area`'s origin (not also the title row's
    /// height) misreads which body line a click/scroll landed on.
    fn titled_text_display_driver() -> TuiDriver<TextDisplayLogic> {
        use quadraui::{StyledSpan, StyledText, TextDisplay, TextDisplayLine};

        let text_display = TextDisplay {
            id: WidgetId::new("log"),
            lines: (0..5)
                .map(|i| TextDisplayLine {
                    spans: vec![StyledSpan::plain(format!("line {i}"))],
                    decoration: Default::default(),
                    timestamp: None,
                })
                .collect(),
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: Some(StyledText::plain("Logs")),
            show_scrollbar: false,
        };
        let logic = TextDisplayLogic {
            text_display,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        TuiDriver::new(logic, 40, 10)
    }

    /// The painted `TextDisplay`'s own area/body-offset/viewport-line-count,
    /// read back the same way [`data_table_geometry`] reads a table's — so
    /// a test aims at exact geometry instead of hardcoding how many rows
    /// the fixed-slot band leaves for the body.
    fn text_display_geometry(driver: &TuiDriver<TextDisplayLogic>) -> (Rect, f32, usize, usize) {
        let hits = driver.app().engine.hit_map();
        hits.iter()
            .find_map(|hit| match hit {
                crate::hit::PaintedHit::TextDisplay {
                    area,
                    body_offset,
                    total_lines,
                    viewport_lines,
                    ..
                } => Some((*area, *body_offset, *total_lines, *viewport_lines)),
                _ => None,
            })
            .expect("text display should have painted a hit")
    }

    /// A click on the title row of a titled `TextDisplay` must NOT resolve
    /// to `TextDisplayLine { line: 0 }` — the body-local layout's line 0
    /// sits one row below the title, so a hit at the title's own screen
    /// position has to miss the layout entirely (translated to a negative
    /// local y) rather than silently reading as the first body line.
    #[test]
    fn click_on_titled_text_display_title_row_does_not_resolve_as_body_line_zero() {
        let mut driver = titled_text_display_driver();
        let (area, body_offset, _, _) = text_display_geometry(&driver);
        assert!(
            body_offset > 0.0,
            "a titled display must report a body offset"
        );
        let title_point = Point::new(area.x + 1.0, area.y);

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: title_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        let hit_body_line_zero = dispatched
            .iter()
            .any(|(_, hit)| matches!(hit, Some(HitDetail::TextDisplayLine { line: 0 })));
        assert!(
            !hit_body_line_zero,
            "a click on the title row must not be misread as body line 0: {:?}",
            dispatched
        );
    }

    /// A wheel notch over the bottom-most visible body row of a titled
    /// `TextDisplay` must emit `TextDisplayEvent::Scrolled`. Before the
    /// body-offset fix, that row's screen position translated to a local y
    /// past every `hit_regions` entry (the layout only covers
    /// `viewport_lines` rows starting at `0.0`), so the hit test missed
    /// entirely and the wheel event fell through untranslated.
    #[test]
    fn wheel_scroll_over_titled_text_display_bottom_row_emits_scrolled() {
        use quadraui::TextDisplayEvent;

        let mut driver = titled_text_display_driver();
        let (area, body_offset, total_lines, viewport_lines) = text_display_geometry(&driver);
        assert!(
            total_lines > viewport_lines,
            "the display must have more lines than fit, or there's nothing to scroll"
        );
        let bottom_row_point = Point::new(
            area.x + 1.0,
            area.y + body_offset + (viewport_lines as f32 - 0.5),
        );

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: quadraui::ScrollDelta::new(0.0, -1.0),
            position: bottom_row_point,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::TextDisplay(id, TextDisplayEvent::Scrolled { new_offset: 1 })
                    if *id == WidgetId::new("log")
            )),
            "expected TextDisplay(Scrolled {{ new_offset: 1 }}): {:?}",
            dispatched
        );
    }

    /// A minimal [`AppLogic`] painting a single `quadraui::SplitTree` node
    /// (via [`paint_layout`], not [`draw_bands`] — `SplitTree` is a
    /// `LayoutNode` shape, not one of the six fixed slots), for the
    /// divider-drag coverage below.
    struct SplitTreeLogic {
        tree: quadraui::SplitTree,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for SplitTreeLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::SplitTree(LayoutSplitTree {
                id: "tree".to_string(),
                tree: self.tree.clone(),
                leaves: vec![
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "left".to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new("left-btn"),
                            "L",
                        )),
                    }),
                    LayoutNode::Leaf(LayoutLeaf {
                        id: "right".to_string(),
                        descriptor: LeafDescriptor::Button(Button::new(
                            WidgetId::new("right-btn"),
                            "R",
                        )),
                    }),
                ],
            });
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
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

    /// Dragging a `SplitTree` divider must turn the following `MouseMoved`
    /// into `UiEvent::SplitDividerDragged` with the resolved ratio — the
    /// N-way generalisation of [`dragging_a_header_divider_emits_column_resized`]'s
    /// coverage, exercising [`crate::drag::ActiveDrag::SplitTree`] instead
    /// of the two-pane [`crate::drag::ActiveDrag::Split`] case.
    #[test]
    fn dragging_a_split_tree_divider_changes_the_ratio() {
        use quadraui::SplitDirection;

        // Ratio 0.6 (not the more obvious 0.5) so the resolved divider
        // `position` has a fractional part under `0.5` — `first_w` then
        // rounds DOWN to the Button leaf's own painted width, so the leaf
        // doesn't also claim the divider's painted cell the way it would
        // at a dead-even tie (`width * 0.5` on an even width rounds UP,
        // per `f32::round`'s ties-away-from-zero rule, which is a
        // pre-existing, separate leaf/divider hit-box overlap this test
        // isn't about).
        let tree = quadraui::SplitTree::split(
            SplitDirection::Horizontal,
            0.6,
            quadraui::SplitTree::leaf(WidgetId::new("left-btn")),
            quadraui::SplitTree::leaf(WidgetId::new("right-btn")),
        );
        let logic = SplitTreeLogic {
            tree,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        // The module's usual 40-cell-wide viewport — deliberately even, so
        // the divider's resolved `position` is fractional and the click
        // below exercises the truncate-to-painted-cell path
        // `hit_test_painted`'s `PaintedHit::SplitTree` arm needs (an odd
        // width like 41 lands the divider on an exact integer and would
        // hide a regression there).
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        // Read the divider's own painted geometry back off the hit map
        // (the `data_table_geometry` pattern) instead of hand-deriving the
        // click column, so the test can't drift from what was actually
        // painted.
        let layout = {
            let hits = driver.app().engine.hit_map();
            find_split_tree_layout(&hits, WidgetId::new("tree"))
                .expect("split tree should have painted a hit")
        };
        let divider = &layout.dividers[0];
        assert!(
            divider.position.fract() > 0.0,
            "expected a fractional divider position on an even-width viewport, got {}",
            divider.position
        );
        let divider_cell = divider.cell_position() as f32;

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(divider_cell, 0.0),
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseMoved {
            position: Point::new(divider_cell + 10.0, 0.0),
            buttons: quadraui::ButtonMask {
                left: true,
                right: false,
                middle: false,
            },
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        let dragged = dispatched.iter().find_map(|(event, _)| match event {
            UiEvent::SplitDividerDragged {
                tree,
                split_index,
                new_ratio,
            } if *tree == WidgetId::new("tree") => Some((*split_index, *new_ratio)),
            _ => None,
        });
        let (split_index, new_ratio) = dragged
            .unwrap_or_else(|| panic!("expected a SplitDividerDragged event: {:?}", dispatched));
        assert_eq!(split_index, 0);
        // ratio = (cursor_axis - axis_start) / axis_size, the same
        // `dispatch_mouse_moved` conversion `drag.rs`'s `ActiveDrag::SplitTree::ratio_at`
        // now mirrors: (19 + 10 - 0) / 40 == 0.725.
        let expected_ratio = (divider_cell + 10.0 - divider.axis_start) / divider.axis_size;
        assert!(
            (new_ratio - expected_ratio).abs() < 0.001,
            "expected a ratio around {expected_ratio}, got {new_ratio}"
        );
    }

    /// A minimal [`AppLogic`] painting a single `quadraui::Panel` node, for
    /// the title-bar-action-click coverage below.
    struct PanelLogic {
        panel: quadraui::Panel,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for PanelLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = LayoutNode::Panel(LayoutPanel {
                panel: self.panel.clone(),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "body".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(
                        WidgetId::new("body-btn"),
                        "Body",
                    )),
                })),
            });
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
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

    /// Clicking a `Panel` title-bar action button must set the raw
    /// `MouseDown`'s `widget` to the action's own id — exercising
    /// `crate::render::paint_layout`'s `LayoutNode::Panel` arm, which
    /// records the panel's chrome layout as a [`crate::hit::PaintedHit::Panel`]
    /// so this hit resolves instead of being dropped, same as every other
    /// primitive's own hit-test coverage in this module.
    #[test]
    fn clicking_a_panel_action_button_sets_the_click_widget_to_its_id() {
        use quadraui::{Panel, PanelAction, StyledText};

        let panel = Panel {
            id: WidgetId::new("outer"),
            title: Some(StyledText::plain("Explorer")),
            actions: vec![PanelAction {
                id: WidgetId::new("close"),
                icon: "x".to_string(),
                tooltip: "Close".to_string(),
                is_active: false,
            }],
            accent: None,
            collapsed: false,
        };
        let logic = PanelLogic {
            panel,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        // The lone action button is right-aligned in the title bar — TUI's
        // own 3-cell action width (`TUI_ACTION_BUTTON_WIDTH` in
        // `quadraui::tui::panel`) puts it at [40.0 - 3.0, 40.0) ==
        // [37.0, 40.0).
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(38.0, 0.0),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::MouseDown { widget: Some(id), .. } if *id == WidgetId::new("close")
            )),
            "expected a MouseDown with widget=close: {:?}",
            dispatched
        );
    }

    /// Clicking a `Panel`'s title bar away from its action buttons (the
    /// collapse affordance — `PanelHit::TitleBar` is the only collapse hit
    /// the primitive exposes) must set the raw `MouseDown`'s `widget` to
    /// the panel's own id.
    #[test]
    fn clicking_a_panel_title_bar_sets_the_click_widget_to_the_panel_id() {
        use quadraui::{Panel, StyledText};

        let panel = Panel {
            id: WidgetId::new("outer"),
            title: Some(StyledText::plain("Explorer")),
            actions: vec![],
            accent: None,
            collapsed: false,
        };
        let logic = PanelLogic {
            panel,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        // Row 0 away from any action button (there are none here) is the
        // title bar itself.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(1.0, 0.0),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::MouseDown { widget: Some(id), .. } if *id == WidgetId::new("outer")
            )),
            "expected a MouseDown with widget=outer: {:?}",
            dispatched
        );
    }

    /// A minimal [`AppLogic`] painting a single `quadraui::SidebarPanel`
    /// node with a toolbar, for the toolbar-button-click coverage below.
    struct SidebarPanelLogic {
        sidebar_panel: quadraui::SidebarPanel,
        engine: InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for SidebarPanelLogic {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut hits = Vec::new();
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let node = crate::layout::LayoutNode::SidebarPanel(crate::layout::LayoutSidebarPanel {
                sidebar_panel: self.sidebar_panel.clone(),
                content: Box::new(LayoutNode::Leaf(LayoutLeaf {
                    id: "body".to_string(),
                    descriptor: LeafDescriptor::Button(Button::new(
                        WidgetId::new("body-btn"),
                        "Body",
                    )),
                })),
            });
            paint_layout(backend, area, &node, &mut hits, &|id| {
                self.engine.chrome_interaction(id)
            });
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

    /// Clicking a `SidebarPanel`'s toolbar button must set the raw
    /// `MouseDown`'s `widget` to the button's own id — exercising
    /// `crate::render::paint_layout`'s `LayoutNode::SidebarPanel` arm,
    /// which records the panel's chrome layout as a
    /// [`crate::hit::PaintedHit::SidebarPanel`] so this hit resolves
    /// instead of being dropped.
    #[test]
    fn clicking_a_sidebar_panel_toolbar_button_sets_the_click_widget_to_its_id() {
        use quadraui::{SidebarPanel, Toolbar, ToolbarButton};

        let sidebar_panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar::new(WidgetId::new("sb:toolbar")).with_buttons(vec![
                ToolbarButton::Action {
                    id: WidgetId::new("refine"),
                    label: "Refine".to_string(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
            ])),
            toolbar_height: None,
        };
        let logic = SidebarPanelLogic {
            sidebar_panel,
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(logic, 40, 10);
        driver.render();

        // The lone button starts at the toolbar's own x=0, row 0.
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(1.0, 0.0),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, _)| matches!(
                event,
                UiEvent::MouseDown { widget: Some(id), .. } if *id == WidgetId::new("refine")
            )),
            "expected a MouseDown with widget=refine: {:?}",
            dispatched
        );
    }
}
