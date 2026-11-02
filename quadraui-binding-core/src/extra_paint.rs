//! The `ExtraPaint` trait and one impl per additive primitive — the
//! paint/hit-test logic for every primitive painted outside the six fixed
//! slots (`Button`/`TextDisplay`/`List`/`Tree`/`DataTable`/`TabBar`).
//! Registering a further additive primitive needs exactly one impl here
//! plus one entry in a binding's own decode registry (`EXTRA_PRIMITIVES` in
//! `quadraui-py`) — no enum variant, no central match arm.

use std::rc::Rc;

use quadraui::{
    ActivityBar, Backend, BoardModel, Chart, ComboBox, CommandCenter, CommandLine,
    CompletionItemMeasure, DatePicker, DiffView, Form, Image, InteractionState, MenuBar,
    MessageList, Minimap, MultiSectionView, NumberInput, Palette, PipelineView, Point, ProgressBar,
    RadioGroup, Rect, Scrollbar, Spinner, StatusBar, Switch, SwitchHit, TextInput, Toolbar,
    WidgetId,
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
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_form(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
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
    /// character column, so any click inside `area` counts as landing on
    /// the input (focus/caret placement stays coarse until a metrics-aware
    /// hit-test is threaded through).
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
        backend.draw_activity_bar(area, self, None);
    }

    fn id(&self) -> WidgetId {
        self.id
    }
}

impl ExtraPaint for Toolbar {
    fn paint(&self, backend: &mut dyn Backend, area: Rect) {
        backend.draw_toolbar_interactive(area, self, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
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
        backend.draw_status_bar_interactive(area, self, &InteractionState::default());
    }

    fn id(&self) -> WidgetId {
        self.id
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
        backend.draw_palette(area, self);
    }

    fn id(&self) -> WidgetId {
        self.id
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

/// The additive primitive kind names, in the same registration order
/// `quadraui-py`'s own `EXTRA_PRIMITIVES` decode table uses.
/// [`crate::render::draw_bands`]
/// iterates this (rather than the `extra` slot `HashMap` itself) so the
/// paint order a frame produces is deterministic, independent of the map's
/// iteration order.
pub const EXTRA_PRIMITIVE_KINDS: [&str; 25] = [
    "progress",
    "spinner",
    "image",
    "form",
    "text_input",
    "number_input",
    "combo_box",
    "date_picker",
    "radio_group",
    "switch",
    "chart",
    "message_list",
    "board",
    "pipeline_view",
    "minimap",
    "multi_section_view",
    "diff_view",
    "activity_bar",
    "toolbar",
    "menu_bar",
    "status_bar",
    "command_line",
    "command_center",
    "palette",
    "scrollbar",
];
