//! The `ExtraPaint` trait, one impl per additive primitive, and the
//! `extra_primitives!` table decoding each one — the paint/hit-test and
//! wire-decode logic for every primitive painted outside the six fixed
//! slots (`Button`/`TextDisplay`/`List`/`Tree`/`DataTable`/`TabBar`).
//! Registering a further additive primitive needs exactly one impl here
//! plus one appended line in the `extra_primitives!` table at the bottom
//! of this file — no enum variant, no central match arm, and no second
//! decode registry in a binding to keep in lockstep with it.

use std::rc::Rc;

use quadraui::{
    ActivityBar, Backend, BoardModel, Canvas, Chart, ComboBox, CommandCenter, CommandLine,
    CompletionItemMeasure, DatePicker, DiffView, Editor, FindReplacePanel, Form, Image,
    InteractionState, MenuBar, MessageList, Minimap, MultiSectionView, NumberInput, NumberInputHit,
    Palette, PipelineView, Point, ProgressBar, RadioGroup, Rect, ScrollAxis, ScrollView, Scrollbar,
    Spinner, StatusBar, StatusBarHit, Switch, SwitchHit, Terminal, TextInput, TextRegion, Toolbar,
    ToolbarButton, WidgetId,
};

/// Paints one additive primitive descriptor painted outside the six fixed
/// slots above. Implemented once per primitive type (see the `impl`s just
/// below) so that registering a kind in a binding's own `EXTRA_PRIMITIVES`
/// decode table never needs a central match arm.
///
/// [`ExtraPaint::id`] is mechanical (`self.id`) on every impl and backs the
/// [`crate::hit::PaintedHit::Extra`] entry pushed for this descriptor.
/// [`ExtraPaint::hit_test`] and [`ExtraPaint::tab_stop`] default to "not
/// interactive", which most additive primitives keep; [`quadraui::Switch`]
/// and [`quadraui::TextInput`] override both to
/// route pointer and keyboard focus through their own `layout`/`hit_test`.
/// This trait only gets a hit/focus to the binding's own event handler with
/// `widget` set — the per-kind semantic event synthesis that turns it into a
/// `FormEvent` (toggling the switch's value, editing the text input, ...)
/// lives in `semantic_event_for_hit`, `focus_scoped_key_event`,
/// `text_input_key_event` and `text_input_insert_event`, below.
pub trait ExtraPaint: std::any::Any {
    fn paint(&self, backend: &mut dyn Backend, area: Rect);

    /// The widget id this descriptor paints under, used to populate
    /// [`crate::hit::PaintedHit::Extra`] without threading a leaf-tree
    /// address through.
    fn id(&self) -> WidgetId;

    /// Whether `point` lands on an interactive region of this descriptor,
    /// given the ABSOLUTE `area` it was painted into (the same frame
    /// `point` itself is in — no translation, matching `Button`/`Split`'s
    /// own layout convention). `false` by default: most additive
    /// primitives are pure display and a miss here lets
    /// [`crate::hit::hit_test_painted`] fall through to whatever painted
    /// behind them.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        let _ = (area, point);
        false
    }

    /// Whether this descriptor should be included in the binding's own tab-stop list.
    /// `false` by default, matching [`ExtraPaint::hit_test`]'s "not
    /// interactive unless overridden" default.
    fn tab_stop(&self) -> bool {
        false
    }

    /// Paints with real hover/pressed state, returning the absolute bounds
    /// and action id of each clickable child this descriptor just painted —
    /// a chrome bar's button ([`quadraui::Toolbar`]), segment
    /// ([`quadraui::StatusBar`]), or item ([`quadraui::ActivityBar`]).
    /// `crate::render::draw_bands`/`paint_leaf` call this (not
    /// [`ExtraPaint::paint`] directly) so the returned regions can be
    /// recorded into [`crate::hit::PaintedHit::Extra::chrome_hits`] and
    /// hit-tested ahead of [`ExtraPaint::hit_test`] on a later click —
    /// `hit_test`'s plain `bool` has no way to say *which* child a point
    /// landed on.
    ///
    /// Default: ignores `interaction` and forwards to [`ExtraPaint::paint`]
    /// with no clickable children — correct for every extra except
    /// `Toolbar`, `StatusBar` and `ActivityBar`, each of which overrides
    /// this to report its own per-child click targets.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        let _ = interaction;
        self.paint(backend, area);
        Vec::new()
    }
}

/// Recovers the concrete descriptor (`Switch`, `TextInput`, ...) kept alive
/// behind the `Rc<dyn ExtraPaint>` in a [`crate::hit::PaintedHit::Extra`] entry, via
/// trait-upcasting coercion to [`ExtraPaint`]'s `std::any::Any` supertrait.
/// Used by [`crate::semantic::semantic_event_for_hit`] and the binding's own
/// event handler to route a hit/key to the matched extra's own semantics
/// (`Switch::click`, `TextEditor::apply`, ...) instead of leaving every
/// extra paint-only.
pub fn extra_as<T: 'static>(extra: &Rc<dyn ExtraPaint>) -> Option<&T> {
    (extra.as_ref() as &dyn std::any::Any).downcast_ref::<T>()
}

impl ExtraPaint for ProgressBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_progress(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Spinner {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_spinner(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Image {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_image(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Form {
    /// The bare, non-interactive paint this trait's own `paint` always
    /// does for every extra — `crate::engine::InteractionEngine::sync_forms`
    /// repaints on top of it with the real [`quadraui::compose::FormController`]
    /// state (scroll offset, in-progress edits, ...) once wired, the same
    /// "paint plain first, then repaint interactive on top" sequencing
    /// `crate::engine::InteractionEngine::sync_menu_bar` already uses for
    /// `"menu_bar"`.
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_form(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check, like [`TextInput::hit_test`] above — a
    /// click's precise field-level routing is
    /// `crate::engine::InteractionEngine::handle`'s own `FormController`
    /// dispatch, which resolves it via `FormController`'s own internal
    /// layout rather than this trait's `bool`-only answer.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for TextInput {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_text_input(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check: [`ExtraPaint::hit_test`] has no
    /// backend-metrics parameter to resolve a click to a specific
    /// character column, so this only ever decides whether a click landed
    /// on the input at all — `InteractionEngine::handle`'s own
    /// `HitDetail::ExtraBody` arm has `Backend::measure()` in hand at the
    /// click site and threads it through `text_input_click_event` for the
    /// metrics-aware part (resolving which character column, and placing
    /// the caret there).
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for RadioGroup {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_radio_group(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check: [`ExtraPaint::hit_test`]'s own signature
    /// has no parameter for the per-option `measure` closure
    /// [`quadraui::RadioGroup::layout`] needs (unlike
    /// [`NumberInput::hit_test`]'s fixed stepper column). That alone
    /// doesn't rule out resolving a click to a specific option —
    /// `InteractionEngine::handle`'s own click site has a real `backend`
    /// (and so real metrics) in hand, the same way it does for
    /// [`TextInput::hit_test`]'s equivalent limitation — it just isn't
    /// wired there yet, so a click still only focuses the whole group;
    /// `Up`/`Down`/`Left`/`Right` move the selection once focused via
    /// [`ExtraPaint::tab_stop`].
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for Switch {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_switch(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    fn hit_test(&self, area: Rect, point: Point) -> bool {
        self.layout(area).hit_test(point) == SwitchHit::Body
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for NumberInput {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let layout = self.layout(area);
        backend.draw_number_input(self, &layout);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Precise, not whole-area, unlike [`TextInput::hit_test`] above:
    /// [`quadraui::NumberInput::layout`] needs no backend metrics (its
    /// stepper column is a fixed `STEPPER_WIDTH`), so the real
    /// increment/decrement/text-area regions are resolvable here, which is
    /// what lets a stepper click be routed precisely (see
    /// `crate::engine::InteractionEngine::handle`'s own `NumberInput`
    /// click arm) instead of only coarse click-to-focus.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        self.layout(area).hit_test(point) != NumberInputHit::Outside
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for ComboBox {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        // Standalone (non-`Form`-embedded) display: the field itself only
        // ever needs its own row, so `field_bounds` is the top row of
        // `area` while the full `area` is passed as the dropdown's
        // viewport -- otherwise `Completions::layout`'s placement math has
        // no room above or below the field to open the dropdown into and
        // always clips it to zero height. A single-row item height is
        // assumed since no measurement pass runs outside a real UI loop.
        // The dropdown itself only paints when `self.open` is true with a
        // non-empty filtered list, so a closed/default combo box renders
        // just its field regardless of these choices. "Row" is one line
        // scaled by `line_height`, the backend-native unit (cells on TUI,
        // pixels on native), so the field and its popup rows size correctly
        // on every backend instead of collapsing to one pixel on native.
        let line_height = backend.line_height();
        let field_bounds = Rect::new(area.x, area.y, area.width.max(line_height), line_height);
        let max_popup_height = (area.height - line_height).max(line_height);
        let layout = self.layout(
            field_bounds,
            area,
            area.width.max(line_height),
            max_popup_height,
            |_| CompletionItemMeasure::new(line_height),
        );
        backend.draw_combo_box(self, &layout);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check, same limitation [`RadioGroup::hit_test`]
    /// documents: resolving a click to the field or a specific filtered
    /// dropdown item needs the backend's `line_height` [`ExtraPaint::paint`]
    /// uses to build [`quadraui::ComboBox::layout`]'s `field_bounds`/
    /// `measure_item`, which this trait's `hit_test` has no parameter for
    /// — `InteractionEngine::handle` has that `backend` in hand at the
    /// click site, same as it does for `TextInput`, so this is not wired
    /// yet rather than impossible. Typing, `Up`/`Down`/`Enter`/`Escape`
    /// still work once focused via [`ExtraPaint::tab_stop`].
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for DatePicker {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        // Standalone (non-`Form`-embedded) display: derive a 7-column,
        // 6-row month grid plus a one-row header (and a one-row time field
        // when `self.time` is set), clipped to the given area. The header
        // and time rows are one line each, scaled by `line_height` (the
        // backend-native unit: cells on TUI, pixels on native) so they size
        // correctly on every backend instead of collapsing to one pixel on
        // native; the grid rows are then whatever is left over divided by
        // six, floored at one line, so a date picker painted into a band
        // shorter than 7 lines deliberately overflows the band rather than
        // shrinking its cells below a usable size.
        let line_height = backend.line_height();
        let header_height = line_height;
        let time_height = if self.time.is_some() {
            line_height
        } else {
            0.0
        };
        let cell_width = (area.width / 7.0).max(line_height);
        let cell_height = ((area.height - header_height - time_height) / 6.0).max(line_height);
        let layout = self.layout(
            area.x,
            area.y,
            cell_width,
            cell_height,
            header_height,
            time_height,
        );
        backend.draw_date_picker(self, &layout);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check, same limitation [`RadioGroup::hit_test`]
    /// documents: resolving a click to a prev/next control or a specific
    /// day cell needs the backend's `line_height` [`ExtraPaint::paint`]
    /// uses to derive the grid's `cell_width`/`cell_height`, which this
    /// trait's `hit_test` has no parameter for — not wired yet rather than
    /// impossible, same caveat. `Left`/`Right`/`Up`/`Down`/`PageUp`/
    /// `PageDown`/`Enter` navigation still works once focused via
    /// [`ExtraPaint::tab_stop`].
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for Chart {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        // Display-only: no hover/crosshair state is tracked by this
        // binding, so both are `None` and the returned `ChartLayout` (for
        // click/nearest-point routing) is discarded.
        backend.draw_chart(area, self, None, None);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for MessageList {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_message_list(area, self);
        // Register a selectable text region every frame so drag-select and
        // Ctrl-C copy work against the transcript. TUI ignores `lines` (it
        // reads the live cell buffer); pixel backends extract from it, so
        // `lines` must be the rows actually painted into `area` this frame:
        // `rows[scroll_top..]`, clipped to how many whole rows fit in
        // `area.height` at this frame's line height — matching
        // `Backend::draw_message_list`'s own row walk (TUI's
        // `draw_message_list`, GTK's `draw_message_list` free function) so
        // row `i` of `bounds` always lines up with `lines[i]`. Each row is
        // already one display row (wrapping happens at the call site, not
        // in this primitive), so unlike `TextDisplay` there's no wrap/
        // timestamp misalignment to worry about here.
        let line_height = backend.line_height();
        let visible_rows = if line_height > 0.0 {
            (area.height / line_height).floor() as usize
        } else {
            0
        };
        let lines = self
            .rows
            .iter()
            .skip(self.scroll_top)
            .take(visible_rows)
            .map(|row| row.text.clone())
            .collect();
        backend.register_text_region(TextRegion {
            id: self.id,
            bounds: area,
            lines,
        });
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for BoardModel {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_board(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for PipelineView {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_pipeline_view(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Minimap {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_minimap(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for MultiSectionView {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_multi_section_view(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for DiffView {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_diff_view(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for ActivityBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        self.paint_chrome(backend, area, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// [`quadraui::Backend::draw_activity_bar`] takes a positional
    /// `hovered_idx: Option<usize>` — a flat index into
    /// [`quadraui::ActivityBarLayout::visible_items`], not a [`WidgetId`] —
    /// so `interaction.hovered()` is first resolved against a no-paint
    /// [`quadraui::Backend::activity_bar_layout`] call to recover that
    /// index before the real paint. Row spans
    /// ([`quadraui::ActivityBarRowHit::y_start`]/`y_end`) are relative to
    /// `area`'s own top edge, so they're shifted by `area.x`/`area.y`
    /// before being recorded — [`crate::hit::PaintedHit::Extra::chrome_hits`]
    /// is always in the ABSOLUTE frame [`ExtraPaint::hit_test`] uses.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        let hovered_idx = interaction.hovered().and_then(|hovered_id| {
            backend
                .activity_bar_layout(area, self)
                .iter()
                .position(|row| row.id == *hovered_id)
        });
        // `ActivityBarRowHit` carries only `y_start`/`y_end` — no x/width of
        // its own — so `area.width` is the only width the `Backend` trait
        // exposes for this row; it matches what `draw_bands` actually fills
        // the row's background with (a full-band-width horizontal strip in
        // the fixed-slot layout), not a narrower vertical-strip assumption.
        backend
            .draw_activity_bar(area, self, hovered_idx)
            .into_iter()
            .map(|row| {
                (
                    Rect::new(
                        area.x,
                        area.y + row.y_start,
                        area.width,
                        row.y_end - row.y_start,
                    ),
                    row.id,
                )
            })
            .collect()
    }
}

impl ExtraPaint for Toolbar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        self.paint_chrome(backend, area, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// [`quadraui::Backend::draw_toolbar_interactive`]'s own
    /// [`quadraui::ToolbarLayout::visible_items`] is already in the
    /// ABSOLUTE frame [`ExtraPaint::hit_test`] uses, so no translation is
    /// needed — unlike [`ActivityBar::paint_chrome`]'s bar-relative row
    /// spans. Disabled actions and non-action items (separators, labels)
    /// have no `action_id` or aren't `clickable`, so they never become a
    /// hit region, matching [`quadraui::ToolbarLayout::hit_test`]'s own
    /// skip rule.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        backend
            .draw_toolbar_interactive(area, self, interaction)
            .visible_items
            .into_iter()
            .filter(|item| item.clickable)
            .filter_map(|item| item.action_id.map(|id| (item.bounds, id)))
            .collect()
    }

    /// A tab stop whenever at least one button could ever be
    /// [`Self::activate_focused`]'s target — matching `Switch`'s own
    /// "interactive, so focusable" rule. A toolbar with no enabled
    /// `Action` (all separators/labels, or every action disabled) is
    /// skipped, the same way an empty `List`/`Tree` still counts (unlike
    /// those, nothing here could ever be activated, so there is no reason
    /// to spend a tab stop on it).
    fn tab_stop(&self) -> bool {
        self.buttons
            .iter()
            .any(|button| matches!(button, ToolbarButton::Action { enabled: true, .. }))
    }
}

impl ExtraPaint for MenuBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_menu_bar(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for StatusBar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        self.paint_chrome(backend, area, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Unlike [`Toolbar::paint_chrome`], [`quadraui::StatusBarLayout::hit_regions`]
    /// are bar-LOCAL (relative to `area`'s own origin — see
    /// [`quadraui::Backend::draw_status_bar_interactive`]'s own doc), so
    /// each region is shifted by `area.x`/`area.y` before being recorded.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        backend
            .draw_status_bar_interactive(area, self, interaction)
            .hit_regions
            .into_iter()
            .filter_map(|(rect, hit)| match hit {
                StatusBarHit::Segment(id) => Some((
                    Rect::new(area.x + rect.x, area.y + rect.y, rect.width, rect.height),
                    id,
                )),
                StatusBarHit::Empty => None,
            })
            .collect()
    }
}

impl ExtraPaint for CommandLine {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_command_line(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for CommandCenter {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_command_center(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Palette {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        self.paint_chrome(backend, area, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// [`quadraui::Backend::draw_palette_with_state`] only tints the
    /// pinned create-action row — the one row in a `Palette` with a
    /// stable id, `palette.id` itself (see that method's own doc for
    /// why [`quadraui::PaletteItem`] rows stay out of `InteractionState`) —
    /// so the one chrome hit recorded here is that row, when present.
    /// Its bounds come from a no-paint [`quadraui::Backend::palette_layout`]
    /// call — the same layout `draw_palette_with_state` paints from —
    /// shifted into the ABSOLUTE frame [`ExtraPaint::hit_test`] uses,
    /// mirroring [`ActivityBar::paint_chrome`]'s own translation.
    fn paint_chrome(
        &self,
        backend: &mut dyn Backend,
        area: Rect,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        backend.draw_palette_with_state(area, self, interaction);
        backend
            .palette_layout(area, self)
            .create_bounds
            .map(|b| {
                vec![(
                    Rect::new(area.x + b.x, area.y + b.y, b.width, b.height),
                    self.id,
                )]
            })
            .unwrap_or_default()
    }
}

impl ExtraPaint for Scrollbar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_scrollbar(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Editor {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let _ = backend.draw_editor(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check, same limitation [`TextInput::hit_test`]
    /// documents above: this trait has no backend-metrics parameter to
    /// resolve a click to a buffer line/column via
    /// [`quadraui::Editor::layout`]'s own `cell_width`/`line_height`
    /// arguments, so any click inside `area` counts as landing on the
    /// editor. A host that wants `Editor::layout(..).hit_test(..)`'s
    /// buffer-position/gutter/scrollbar detail re-derives it from the
    /// enriched `MouseDown`'s position plus its own metrics.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for Terminal {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_terminal(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check — same shape as [`Editor::hit_test`]:
    /// resolving a click to a specific cell is [`quadraui::Terminal`]'s own
    /// `layout(..).hit_test(..)`, which a host calls itself once it has the
    /// enriched `MouseDown` position, since mouse/keyboard interaction is
    /// deliberately outside this paint-snapshot primitive (see the module
    /// doc on `quadraui::Terminal`). This still gives click-to-focus so the
    /// app can route subsequent key presses to the live PTY.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for Canvas {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let _ = backend.draw_canvas(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Matches [`quadraui::CanvasLayout::hit_test`]'s own "inside these
    /// bounds at all" answer (see [`quadraui::CanvasHit`]'s doc) — `Canvas`
    /// has no sub-regions of its own, so there is nothing finer to resolve
    /// here.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }
}

impl ExtraPaint for ScrollView {
    /// `ScrollView` carries no backend painter of its own (module doc: "no
    /// backend painter lives here" — a host clips/paints its own content
    /// inside the viewport this descriptor describes). The one thing this
    /// leaf still paints is the scrollbar indicator itself, via
    /// [`quadraui::ScrollView::scrollbar`], in a fixed `line_height`-wide
    /// strip along the scrolling axis's trailing edge — the same
    /// cell-width convention `Tree`'s/`List`'s own TUI vertical-scrollbar
    /// track uses. [`quadraui::ScrollView::scrollbar`] returns `None` when
    /// there's nothing to scroll, so a `ScrollView` whose content fits its
    /// viewport paints nothing at all.
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        let line_height = backend.line_height();
        let track = match self.axis {
            ScrollAxis::Vertical => Rect::new(
                area.x + area.width - line_height,
                area.y,
                line_height,
                area.height,
            ),
            ScrollAxis::Horizontal => Rect::new(
                area.x,
                area.y + area.height - line_height,
                area.width,
                line_height,
            ),
        };
        if let Some(scrollbar) = self.scrollbar(track, line_height) {
            backend.draw_scrollbar(track, &scrollbar);
        }
    }

    fn id(&self) -> WidgetId {
        self.id
    }

    /// Whole-area bounds check: a wheel `Scroll` or click landing anywhere
    /// inside the viewport counts as targeting this `ScrollView`, so the
    /// enriched event's `widget` resolves to it — a host applies
    /// `ScrollView::handle_wheel`/`handle_key` itself against the position/
    /// key it already receives on that enriched event.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

impl ExtraPaint for FindReplacePanel {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_find_replace(area, self);
    }

    /// `FindReplacePanel` carries no `id` field of its own — it is a
    /// singleton overlay anchored to the active editor group (module doc:
    /// "the inline find/replace panel that anchors at top-right of the
    /// active editor group"), so only one instance is ever live at a time,
    /// the same way the `"find_replace"` slot in a binding's own
    /// `EXTRA_PRIMITIVES` decode table admits only one active value. A
    /// fixed sentinel id is therefore enough to address it in
    /// [`crate::hit::PaintedHit::Extra`] without adding a field to the
    /// descriptor itself.
    fn id(&self) -> WidgetId {
        WidgetId::new("find_replace")
    }

    /// Whole-area bounds check, same limitation [`Editor::hit_test`]
    /// documents: resolving a click to one of [`quadraui::FindReplacePanel::hit_regions`]'s
    /// char-cell targets needs `content_origin`/`char_width`/`line_height`
    /// that this trait has no parameter for — [`quadraui::FindReplacePanel::hit_test`]
    /// is still available to a host that has those metrics in hand.
    fn hit_test(&self, area: Rect, point: Point) -> bool {
        area.contains(point)
    }

    fn tab_stop(&self) -> bool {
        true
    }
}

/// Counts its `literal` arguments at compile time — the array-length
/// companion `extra_primitives!` needs so [`EXTRA_PRIMITIVE_KINDS`]'s
/// size tracks the table below with no hand-maintained length to drift.
macro_rules! extra_primitive_count {
    () => (0usize);
    ($head:literal $(, $tail:literal)* $(,)?) => (1usize + extra_primitive_count!($($tail),*));
}

/// Generates [`EXTRA_PRIMITIVE_KINDS`] and [`decode_extra_primitive`] from
/// one `"kind" => Type` table — the single source of truth both the kind
/// list and its decode dispatch share, so a further additive primitive
/// needs exactly one appended line here (plus that type's own
/// [`ExtraPaint`] impl above) and never a second table to keep in
/// lockstep with it.
macro_rules! extra_primitives {
    ($( $kind:literal => $ty:ty ),+ $(,)?) => {
        /// The additive primitive kind names, in [`decode_extra_primitive`]'s
        /// dispatch order. [`crate::render::draw_bands`] iterates this
        /// (rather than the `extra` slot `HashMap` itself) so the paint
        /// order a frame produces is deterministic, independent of the
        /// map's iteration order.
        pub const EXTRA_PRIMITIVE_KINDS: [&str; extra_primitive_count!($($kind),+)] =
            [$($kind),+];

        /// Decodes one additive primitive descriptor, dispatched by `kind`
        /// through the same table [`EXTRA_PRIMITIVE_KINDS`] names — the
        /// core counterpart of a binding's own `EXTRA_PRIMITIVES` decode
        /// registry (`quadraui-py`'s `insert_extra`/`insert_progress`/...
        /// and its layout-tree leaf/render-frame decode all route through
        /// this). Returns a [`crate::DecodeError`] naming every registered
        /// kind when `kind` isn't one of them.
        pub fn decode_extra_primitive<'de, D: serde::Deserializer<'de>>(
            what: &str,
            kind: &str,
            deserializer: D,
        ) -> Result<Rc<dyn ExtraPaint>, crate::DecodeError> {
            match kind {
                $(
                    $kind => Ok(Rc::new(crate::decode_value::<$ty, D>(what, deserializer)?)
                        as Rc<dyn ExtraPaint>),
                )+
                _ => Err(crate::DecodeError(format!(
                    "{what}: unknown additive primitive kind {kind:?} (expected one of {:?})",
                    EXTRA_PRIMITIVE_KINDS
                ))),
            }
        }
    };
}

extra_primitives! {
    "progress" => ProgressBar,
    "spinner" => Spinner,
    "image" => Image,
    "form" => Form,
    "text_input" => TextInput,
    "number_input" => NumberInput,
    "combo_box" => ComboBox,
    "date_picker" => DatePicker,
    "radio_group" => RadioGroup,
    "switch" => Switch,
    "chart" => Chart,
    "message_list" => MessageList,
    "board" => BoardModel,
    "pipeline_view" => PipelineView,
    "minimap" => Minimap,
    "multi_section_view" => MultiSectionView,
    "diff_view" => DiffView,
    "activity_bar" => ActivityBar,
    "toolbar" => Toolbar,
    "menu_bar" => MenuBar,
    "status_bar" => StatusBar,
    "command_line" => CommandLine,
    "command_center" => CommandCenter,
    "palette" => Palette,
    "scrollbar" => Scrollbar,
    "editor" => Editor,
    "terminal" => Terminal,
    "canvas" => Canvas,
    "scroll_view" => ScrollView,
    "find_replace" => FindReplacePanel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit::PaintedHit;
    use crate::layout::{LayoutLeaf, LeafDescriptor};
    use crate::render::paint_leaf;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{AppLogic, PaletteItem, Reaction, UiEvent};
    use std::cell::RefCell;

    /// Records every event an [`EventSink`] receives, in order, for a
    /// chrome-leaf test to assert against without any host-language
    /// callback in scope.
    #[derive(Default)]
    struct RecordingSink {
        dispatched: Vec<UiEvent>,
    }

    impl crate::engine::EventSink for RecordingSink {
        fn dispatch(
            &mut self,
            event: &UiEvent,
            _hit_detail: Option<&crate::hit::HitDetail>,
        ) -> Reaction {
            self.dispatched.push(event.clone());
            Reaction::Continue
        }
    }

    fn palette(create_label: Option<&str>) -> Palette {
        Palette {
            id: WidgetId::new("pal"),
            title: "Commands".into(),
            query: String::new(),
            query_cursor: 0,
            items: vec![PaletteItem {
                text: quadraui::StyledText::plain("open file"),
                detail: None,
                icon: None,
                match_positions: vec![],
                depth: 0,
                expandable: false,
                expanded: false,
            }],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 0,
            has_focus: true,
            show_query: true,
            create_label: create_label.map(|s| s.to_string()),
            preview: None,
            mode: Default::default(),
        }
    }

    struct PaletteChromeApp {
        palette: Palette,
        hits: RefCell<Vec<PaintedHit>>,
    }

    impl AppLogic for PaletteChromeApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let leaf = LayoutLeaf {
                id: "pal-leaf".into(),
                descriptor: LeafDescriptor::Extra(Rc::new(self.palette.clone())),
                scroll_px: None,
            };
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut hits = Vec::new();
            let chrome = |_id| InteractionState::default();
            paint_leaf(backend, area, &leaf, &mut hits, &chrome);
            *self.hits.borrow_mut() = hits;
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    fn chrome_hits_for(create_label: Option<&str>) -> Vec<(Rect, WidgetId)> {
        let app = PaletteChromeApp {
            palette: palette(create_label),
            hits: RefCell::new(Vec::new()),
        };
        let mut driver = TuiDriver::new(app, 40, 20);
        driver.render();
        let found = driver.app().hits.borrow().iter().find_map(|hit| match hit {
            PaintedHit::Extra { chrome_hits, .. } => Some(chrome_hits.clone()),
            _ => None,
        });
        found.expect("a Palette extra was painted")
    }

    /// With no pinned create row, there is nothing to register as a
    /// chrome hit — `chrome_hover`/`chrome_pressed` (and so
    /// `draw_palette_with_state`'s hover tint) never fire for a plain
    /// palette.
    #[test]
    fn no_create_label_means_no_chrome_hits() {
        assert_eq!(chrome_hits_for(None), Vec::new());
    }

    /// A pinned create row is recorded as one chrome hit keyed by
    /// `palette.id` — the same id [`Backend::draw_palette_with_state`]
    /// checks `interaction.is_hovered`/`is_pressed` against, so a
    /// py/node app that sets `create_label` gets hover for free the
    /// moment `InteractionEngine::handle`'s generic `chrome_hover_hit`
    /// bookkeeping (keyed off this exact `chrome_hits` entry) sees a
    /// `MouseMoved` land inside it.
    #[test]
    fn create_label_registers_one_chrome_hit_keyed_by_palette_id() {
        let hits = chrome_hits_for(Some("Create 'foo'"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, WidgetId::new("pal"));
    }

    /// A `Switch` placed directly as a `LayoutNode::Leaf` (not through the
    /// flat-slot `extra` map) paints via [`crate::render::paint_layout`] and
    /// a `MouseDown` landing on it round-trips through
    /// [`crate::engine::InteractionEngine::handle`] into the same
    /// `UiEvent::Form(.., FormEvent::ToggleChanged)` a slot-painted switch
    /// dispatches — proving the additive-primitive leaf path
    /// (`crate::layout::LeafDescriptor::Extra`, wired generically for every
    /// `EXTRA_PRIMITIVE_KINDS` entry) carries an interactive form control's
    /// semantic event all the way to the handler, not just its paint.
    #[test]
    fn switch_leaf_click_toggles_through_the_interaction_engine() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode, Spacing};
        use crate::render::paint_layout;
        use quadraui::{FormEvent, Modifiers, MouseButton};

        struct SwitchLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for SwitchLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let tree = LayoutNode::Row(LayoutContainer {
                    children: vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "sw:1".into(),
                        descriptor: LeafDescriptor::Extra(Rc::new(Switch::new(WidgetId::new(
                            "sw:1",
                        )))),
                        scroll_px: None,
                    })],
                    spacing: Spacing::default(),
                });
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits, &|id| {
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

        let app = SwitchLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        let switch_area = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Extra { id, area, .. } if *id == WidgetId::new("sw:1") => {
                        Some(*area)
                    }
                    _ => None,
                })
                .expect("switch leaf should have painted an Extra hit")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(switch_area.x, switch_area.y),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(id, FormEvent::ToggleChanged { value: true, .. })
                    if *id == WidgetId::new("sw:1")
            )),
            "expected a ToggleChanged event for the inline switch leaf, got {dispatched:?}"
        );
    }

    /// A `Toolbar` placed directly as a `LayoutNode::Leaf` paints through
    /// [`crate::render::paint_layout`]'s `Extra` arm, which calls
    /// [`ExtraPaint::paint_chrome`] (not just [`ExtraPaint::paint`]) and
    /// records its clickable buttons into [`crate::hit::PaintedHit::Extra::chrome_hits`]
    /// — exactly the same chrome-click path [`crate::render::draw_bands`]
    /// uses for a flat-slot `"toolbar"`. A `MouseDown` on the recorded
    /// button rect round-trips through [`crate::engine::InteractionEngine::handle`]'s
    /// `HitDetail::ExtraChrome` arm into the same
    /// `UiEvent::Toolbar(.., ToolbarEvent::ButtonClicked)` a slot-painted
    /// toolbar dispatches, proving the chrome-bar leaf path carries its
    /// semantic click all the way to the handler, not just its paint.
    #[test]
    fn toolbar_leaf_click_emits_button_clicked_through_the_interaction_engine() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode, Spacing};
        use crate::render::paint_layout;
        use quadraui::{Modifiers, MouseButton, ToolbarEvent};

        struct ToolbarLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for ToolbarLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let toolbar = Toolbar::new(WidgetId::new("toolbar:main")).with_buttons(vec![
                    ToolbarButton::Action {
                        id: WidgetId::new("toolbar:save"),
                        label: "Save".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                        spec: None,
                    },
                ]);
                let tree = LayoutNode::Row(LayoutContainer {
                    children: vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "toolbar:main".into(),
                        descriptor: LeafDescriptor::Extra(Rc::new(toolbar)),
                        scroll_px: None,
                    })],
                    spacing: Spacing::default(),
                });
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits, &|id| {
                    self.engine.chrome_interaction(id)
                });
                crate::host::render_overlays(&self.engine, backend, hits);
            }

            fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = ToolbarLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        let button_point = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Extra {
                        id, chrome_hits, ..
                    } if *id == WidgetId::new("toolbar:main") => chrome_hits
                        .first()
                        .map(|(rect, _)| Point::new(rect.x, rect.y)),
                    _ => None,
                })
                .expect("toolbar leaf should have painted a chrome hit for its button")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: button_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Toolbar(id, ToolbarEvent::ButtonClicked { id: button_id, .. })
                    if *id == WidgetId::new("toolbar:main")
                        && *button_id == WidgetId::new("toolbar:save")
            )),
            "expected a ButtonClicked event for the inline toolbar leaf, got {dispatched:?}"
        );
    }

    /// A `MenuBar` placed directly as a `LayoutNode::Leaf` is synced into
    /// [`crate::engine::InteractionEngine`]'s own `MenuSystem` by
    /// [`crate::host::render_overlays`]'s `sync_menu_bar` call — the same
    /// helper every binding's render path (flat-slot or render-callback)
    /// runs after painting, so a leaf-painted bar gets the identical
    /// dropdown/keyboard-navigation state a slot-painted one does.
    /// `Alt+F` opens the one top-level menu this test builds, and `Enter`
    /// activates its sole leaf item, round-tripping through
    /// [`crate::engine::InteractionEngine::handle`] into the same
    /// `UiEvent::MenuActivated` a slot-painted menu bar dispatches.
    #[test]
    fn menu_bar_leaf_activation_reaches_the_interaction_engine() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode, Spacing};
        use crate::render::paint_layout;
        use quadraui::{ContextMenuItem, Key, Modifiers, NamedKey, StyledText};

        struct MenuBarLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for MenuBarLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let menu_bar = MenuBar {
                    id: WidgetId::new("menu_bar:main"),
                    items: vec![quadraui::MenuBarItem {
                        id: WidgetId::new("menu:file"),
                        label: "&File".into(),
                        disabled: false,
                        submenu: Some(vec![ContextMenuItem {
                            id: Some(WidgetId::new("menu:file:save")),
                            label: StyledText::plain("Save"),
                            ..Default::default()
                        }]),
                    }],
                    open_item: None,
                    focused_item: None,
                };
                let tree = LayoutNode::Row(LayoutContainer {
                    children: vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "menu_bar:main".into(),
                        descriptor: LeafDescriptor::Extra(Rc::new(menu_bar)),
                        scroll_px: None,
                    })],
                    spacing: Spacing::default(),
                });
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits, &|id| {
                    self.engine.chrome_interaction(id)
                });
                crate::host::render_overlays(&self.engine, backend, hits);
            }

            fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = MenuBarLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Char('f'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        });
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::MenuActivated(id) if *id == WidgetId::new("menu:file:save")
            )),
            "expected a MenuActivated event for the inline menu_bar leaf, got {dispatched:?}"
        );
    }
}
