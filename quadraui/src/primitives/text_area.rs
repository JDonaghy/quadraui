//! `TextArea` primitive: a standalone multi-line input, on the widget
//! contract (`Primitive` + `Interactive`).
//!
//! The standalone form of [`FieldKind::TextArea`]: it paints through the
//! same per-kind control painter a label-less `Form` row of that kind
//! uses, `visible_rows` rows tall, and edits through the shared
//! flat-value editing handler
//! ([`crate::behaviour::text_edit::handle_flat_edit_event`]) every other
//! text widget uses — Enter inserts a newline, Ctrl/Cmd+A/C/X
//! select-all/copy/cut, and a paste inserts.

use serde::{Deserialize, Serialize};

use crate::behaviour::text_edit::{
    handle_flat_edit_event, painted_flat_caret, FlatEditOutcome, FlatEditState,
};
use crate::event::Rect;
use crate::primitives::form::{BareFieldLayout, FieldKind, FormEvent};
use crate::types::WidgetId;
use crate::Handled;

fn default_rows() -> usize {
    3
}

/// A standalone multi-line input over a flat `value` (lines separated by
/// `'\n'`).
///
/// # Examples
///
/// ```
/// use quadraui::{TextArea, WidgetId};
///
/// let area = TextArea::new(WidgetId::new("notes"));
/// assert_eq!(area.visible_rows, 3);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TextArea {
    pub id: WidgetId,
    /// The text, lines separated by `'\n'`.
    #[serde(default)]
    pub value: String,
    /// Shown dimmed while the value is empty.
    #[serde(default)]
    pub placeholder: String,
    /// The caret as a byte offset into `value`; `None` puts it at the end.
    #[serde(default)]
    pub cursor: Option<usize>,
    /// Whether to paint the focused wash. The app owns it.
    #[serde(default)]
    pub has_focus: bool,
    /// Rows tall — the viewport height, independent of how many lines
    /// the value holds.
    #[serde(default = "default_rows")]
    pub visible_rows: usize,
}

impl TextArea {
    /// An empty, unfocused three-row text area.
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            value: String::new(),
            placeholder: String::new(),
            cursor: None,
            has_focus: false,
            visible_rows: default_rows(),
        }
    }

    /// The [`FieldKind`] this text area paints as, with the caret `state`
    /// has walked to.
    pub(crate) fn field_kind(&self, state: Option<&FlatEditState>) -> FieldKind {
        let (cursor, _) = painted_flat_caret(state, self.cursor, None);
        FieldKind::TextArea {
            value: self.value.clone(),
            placeholder: self.placeholder.clone(),
            cursor: cursor.or(Some(self.value.len())),
            visible_rows: self.visible_rows,
        }
    }
}

/// What a [`TextArea`]'s [`crate::interactive::Interactive::handle`]
/// resolves one gesture to. A copy reaches the host as
/// [`crate::UiEvent::TextCopied`] directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextAreaAction {
    /// The value changed; carries the whole new value.
    Changed(String),
}

impl crate::interactive::WidgetAction for TextAreaAction {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            TextAreaAction::Changed(value) => {
                crate::UiEvent::Form(id, FormEvent::TextInputChanged { id, value })
            }
        }
    }
}

impl crate::primitive::Primitive for TextArea {
    type Layout = BareFieldLayout;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// `visible_rows` rows tall; no natural width.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let rows = self.visible_rows.max(1) as f32;
        crate::NaturalSize::new(None, Some(rows * ctx.line_height.max(1.0)))
    }

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> BareFieldLayout {
        let area = match ctx.units() {
            crate::primitive::Units::Cells => crate::primitive::cell_grid_rect(rect),
            _ => rect,
        };
        BareFieldLayout { area, track: None }
    }

    fn paint(
        &self,
        layout: &BareFieldLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        crate::primitives::form::paint_bare_field_pixels(
            surface,
            layout,
            &self.field_kind(env.widget_state_as::<FlatEditState>()),
            false,
            self.has_focus,
            env.theme(),
        );
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &BareFieldLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        crate::primitives::form::paint_bare_field_cells(
            cells,
            layout,
            &self.field_kind(env.widget_state_as::<FlatEditState>()),
            false,
            self.has_focus,
            env.theme(),
        );
    }
}

impl crate::interactive::Interactive for TextArea {
    type State = FlatEditState;
    type Action = TextAreaAction;

    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    fn a11y_actions(&self, _state: &FlatEditState) -> Vec<crate::a11y::A11yAction> {
        vec![
            crate::a11y::A11yAction::Focus,
            crate::a11y::A11yAction::SetValue,
        ]
    }

    /// Every event goes through the shared flat-value editing handler; a
    /// press anywhere takes focus.
    fn handle(
        &self,
        state: &mut FlatEditState,
        event: &crate::interactive::RoutedEvent,
        _layout: &BareFieldLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TextAreaAction> {
        match handle_flat_edit_event(&self.value, self.cursor, None, state, event, &|_| None, cx) {
            FlatEditOutcome::Changed(value) => Handled::Event(TextAreaAction::Changed(value)),
            FlatEditOutcome::Consumed => Handled::Consumed,
            FlatEditOutcome::Ignored => Handled::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, NamedKey};
    use crate::interactive::{Interactive, KeyEvent, RoutedEvent};
    use crate::types::Modifiers;

    fn run(
        area: &TextArea,
        state: &mut FlatEditState,
        event: RoutedEvent,
    ) -> (Handled<TextAreaAction>, Vec<crate::UiEvent>) {
        let layout = BareFieldLayout {
            area: Rect::new(0.0, 0.0, 12.0, 3.0),
            track: None,
        };
        crate::compose::widget_host::with_cx(area.id, crate::primitive::Units::Cells, |cx| {
            area.handle(state, &event, &layout, cx)
        })
    }

    fn key(k: Key, modifiers: Modifiers) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: k,
            modifiers,
            repeat: false,
        })
    }

    #[test]
    fn enter_inserts_a_newline_at_the_caret() {
        let mut area = TextArea::new(WidgetId::new("ta"));
        area.value = "ab".into();
        area.cursor = Some(1);
        let mut state = FlatEditState::default();
        assert_eq!(
            run(
                &area,
                &mut state,
                key(Key::Named(NamedKey::Enter), Modifiers::default())
            )
            .0,
            Handled::Event(TextAreaAction::Changed("a\nb".into()))
        );
    }

    #[test]
    fn select_all_then_copy_emits_the_whole_value() {
        let mut area = TextArea::new(WidgetId::new("ta"));
        area.value = "one\ntwo".into();
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let mut state = FlatEditState::default();
        let _ = run(&area, &mut state, key(Key::Char('a'), ctrl));
        let (_, emitted) = run(&area, &mut state, key(Key::Char('c'), ctrl));
        assert_eq!(emitted, vec![crate::UiEvent::TextCopied("one\ntwo".into())]);
    }
}
