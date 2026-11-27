//! `PasswordInput` primitive: a standalone masked single-line input, on
//! the widget contract (`Primitive` + `Interactive`).
//!
//! The standalone form of [`FieldKind::PasswordInput`]: it paints through
//! the same per-kind control painter a label-less `Form` row of that kind
//! uses (one mask glyph per character, never the plaintext), and edits
//! through the shared flat-value editing handler
//! ([`crate::behaviour::text_edit::handle_flat_edit_event`]) every other
//! text widget uses. Enter commits. Copy and cut never take the value
//! out: Ctrl/Cmd+C and Ctrl/Cmd+X are swallowed. Paste inserts.

use serde::{Deserialize, Serialize};

use crate::behaviour::text_edit::{
    handle_flat_edit_event, painted_flat_caret, EditIntent, FlatEditOutcome, FlatEditState,
};
use crate::event::{Key, NamedKey, Rect};
use crate::primitives::form::{BareFieldLayout, FieldKind, FormEvent};
use crate::types::WidgetId;
use crate::Handled;

fn default_mask() -> char {
    '•'
}

/// A standalone masked single-line input. The value is held in
/// plaintext; only painting masks it.
///
/// # Examples
///
/// ```
/// use quadraui::{PasswordInput, WidgetId};
///
/// let input = PasswordInput::new(WidgetId::new("login:password"));
/// assert_eq!(input.mask_char, '•');
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct PasswordInput {
    pub id: WidgetId,
    /// The plaintext value.
    #[serde(default)]
    pub value: String,
    /// Shown dimmed while the value is empty.
    #[serde(default)]
    pub placeholder: String,
    /// The caret as a byte offset into `value`; `None` puts it at the end.
    #[serde(default)]
    pub cursor: Option<usize>,
    /// Whether to paint the focused wash and caret. The app owns it.
    #[serde(default)]
    pub has_focus: bool,
    /// The glyph painted for each character of the value.
    #[serde(default = "default_mask")]
    pub mask_char: char,
}

impl PasswordInput {
    /// An empty, unfocused input masking with `•`.
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            value: String::new(),
            placeholder: String::new(),
            cursor: None,
            has_focus: false,
            mask_char: default_mask(),
        }
    }

    /// The [`FieldKind`] this input paints as, with the caret `state` has
    /// walked to.
    pub(crate) fn field_kind(&self, state: Option<&FlatEditState>) -> FieldKind {
        let (cursor, _) = painted_flat_caret(state, self.cursor, None);
        FieldKind::PasswordInput {
            value: self.value.clone(),
            placeholder: self.placeholder.clone(),
            cursor: cursor.or(Some(self.value.len())),
            mask_char: self.mask_char,
        }
    }
}

/// What a [`PasswordInput`]'s
/// [`crate::interactive::Interactive::handle`] resolves one gesture to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordInputAction {
    /// The value changed; carries the whole new value.
    Changed(String),
    /// Enter committed the current value.
    Committed(String),
}

impl crate::interactive::WidgetAction for PasswordInputAction {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            PasswordInputAction::Changed(value) => {
                crate::UiEvent::Form(id, FormEvent::TextInputChanged { id, value })
            }
            PasswordInputAction::Committed(value) => {
                crate::UiEvent::Form(id, FormEvent::TextInputCommitted { id, value })
            }
        }
    }
}

impl crate::primitive::Primitive for PasswordInput {
    type Layout = BareFieldLayout;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// One row tall; no natural width — the box fills the width it is
    /// given.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        crate::NaturalSize::new(None, Some(ctx.line_height.max(1.0)))
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

impl crate::interactive::Interactive for PasswordInput {
    type State = FlatEditState;
    type Action = PasswordInputAction;

    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    fn a11y_actions(&self, _state: &FlatEditState) -> Vec<crate::a11y::A11yAction> {
        vec![
            crate::a11y::A11yAction::Focus,
            crate::a11y::A11yAction::SetValue,
        ]
    }

    /// Enter commits; Ctrl/Cmd+C and Ctrl/Cmd+X are swallowed so the
    /// value never reaches the clipboard; everything else goes through
    /// the shared flat-value editing handler. A press anywhere takes
    /// focus.
    fn handle(
        &self,
        state: &mut FlatEditState,
        event: &crate::interactive::RoutedEvent,
        _layout: &BareFieldLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<PasswordInputAction> {
        use crate::interactive::RoutedEvent;

        // One line by construction: a pasted or IME-committed line break
        // is dropped rather than splitting the value.
        let single_line;
        let event = match event {
            RoutedEvent::Text(text) if text.contains(['\n', '\r']) => {
                single_line = RoutedEvent::Text(text.replace(['\n', '\r'], ""));
                &single_line
            }
            _ => event,
        };
        if let RoutedEvent::Key(key) = event {
            if matches!(key.key, Key::Named(NamedKey::Enter)) {
                return Handled::Event(PasswordInputAction::Committed(self.value.clone()));
            }
            if matches!(
                EditIntent::from_key(&key.key, key.modifiers),
                Some(EditIntent::Copy | EditIntent::Cut)
            ) {
                return Handled::Consumed;
            }
        }
        match handle_flat_edit_event(&self.value, self.cursor, None, state, event, &|_| None, cx) {
            FlatEditOutcome::Changed(value) => Handled::Event(PasswordInputAction::Changed(value)),
            FlatEditOutcome::Consumed => Handled::Consumed,
            FlatEditOutcome::Ignored => Handled::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::{Interactive, KeyEvent, RoutedEvent};
    use crate::types::Modifiers;

    fn run(
        input: &PasswordInput,
        state: &mut FlatEditState,
        event: RoutedEvent,
    ) -> (Handled<PasswordInputAction>, Vec<crate::UiEvent>) {
        let layout = BareFieldLayout {
            area: Rect::new(0.0, 0.0, 12.0, 1.0),
            track: None,
        };
        crate::compose::widget_host::with_cx(input.id, crate::primitive::Units::Cells, |cx| {
            input.handle(state, &event, &layout, cx)
        })
    }

    fn key(k: Key, modifiers: Modifiers) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: k,
            modifiers,
            repeat: false,
        })
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    fn input(value: &str) -> PasswordInput {
        let mut input = PasswordInput::new(WidgetId::new("pw"));
        input.value = value.into();
        input
    }

    #[test]
    fn typing_and_paste_edit_the_value_and_enter_commits_it() {
        let pw = input("hi");
        let mut state = FlatEditState::default();
        assert_eq!(
            run(&pw, &mut state, key(Key::Char('!'), Modifiers::default())).0,
            Handled::Event(PasswordInputAction::Changed("hi!".into()))
        );
        assert_eq!(
            run(&pw, &mut state, RoutedEvent::Text("xy".into())).0,
            Handled::Event(PasswordInputAction::Changed("hixy".into()))
        );
        assert_eq!(
            run(
                &pw,
                &mut state,
                key(Key::Named(NamedKey::Enter), Modifiers::default())
            )
            .0,
            Handled::Event(PasswordInputAction::Committed("hi".into()))
        );
    }

    #[test]
    fn copy_and_cut_never_take_the_value_out() {
        let pw = input("secret");
        let mut state = FlatEditState::default();
        let _ = run(&pw, &mut state, key(Key::Char('a'), ctrl()));
        for chord in ['c', 'x'] {
            let (handled, emitted) = run(&pw, &mut state, key(Key::Char(chord), ctrl()));
            assert_eq!(handled, Handled::Consumed, "{chord}");
            assert!(emitted.is_empty(), "{chord}: {emitted:?}");
        }
    }

    #[test]
    fn a_pasted_line_break_is_dropped_from_the_single_line_value() {
        let mut input = PasswordInput::new(WidgetId::new("pw"));
        input.value = "hi".into();
        let mut state = FlatEditState::default();
        let (handled, _) = run(&input, &mut state, RoutedEvent::Text("yo\r\nthere".into()));
        assert_eq!(
            handled,
            Handled::Event(PasswordInputAction::Changed("hiyothere".into()))
        );
    }
}
