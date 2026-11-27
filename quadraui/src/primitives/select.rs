//! `Select` primitive: a standalone single-choice option picker, on the
//! widget contract (`Primitive` + `Interactive`).
//!
//! A select is the standalone form of [`FieldKind::Dropdown`]: it paints
//! through the same per-kind control painter a label-less `Form` row of
//! that kind uses, and Up/Down move and commit the choice in one step
//! with no separate open step — the [`step_for_key`] a `Form` dropdown
//! row uses too. A left press advances to the next option. Every change
//! reaches the host as `UiEvent::Form(id, FormEvent::DropdownChanged { .. })`,
//! the event a `Form` dropdown row already reports.

use serde::{Deserialize, Serialize};

use crate::event::{Key, NamedKey, Rect};
use crate::primitives::form::{BareFieldLayout, FieldKind, FormEvent};
use crate::types::{StyledText, WidgetId};
use crate::Handled;

/// A standalone single-choice option picker.
///
/// # Examples
///
/// ```
/// use quadraui::{Select, WidgetId};
///
/// let select = Select::new(WidgetId::new("theme"), vec!["Light".into(), "Dark".into()]);
/// assert_eq!(select.selected_idx, 0);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Select {
    pub id: WidgetId,
    /// The options, in order.
    pub options: Vec<String>,
    /// The chosen option's index into `options`.
    #[serde(default)]
    pub selected_idx: usize,
    /// A disabled select paints dimmed and ignores every gesture.
    #[serde(default)]
    pub disabled: bool,
    /// Whether to paint the focused-row wash. The app owns it.
    #[serde(default)]
    pub focused: bool,
}

impl Select {
    /// An enabled, unfocused select over `options` with the first chosen.
    pub fn new(id: WidgetId, options: Vec<String>) -> Self {
        Self {
            id,
            options,
            selected_idx: 0,
            disabled: false,
            focused: false,
        }
    }

    /// The [`FieldKind`] this select paints as.
    pub(crate) fn field_kind(&self) -> FieldKind {
        FieldKind::Dropdown {
            options: self
                .options
                .iter()
                .map(|o| StyledText::plain(o.clone()))
                .collect(),
            selected_idx: self.selected_idx,
        }
    }

    /// `idx` as a change, or `None` when disabled, empty or already
    /// chosen.
    fn choose(&self, idx: Option<usize>) -> Handled<SelectAction> {
        match idx {
            Some(idx) if !self.disabled && idx != self.selected_idx => {
                Handled::Event(SelectAction::Changed(idx))
            }
            _ => Handled::Ignored,
        }
    }
}

/// The option one key moves a choice at `selected` among `len` options
/// to: Up the previous, Down the next, clamped (never wrapping). `None`
/// for any other key, an empty list, or a move already at that end.
/// Shared by [`Select`] and a `Form`'s `Dropdown` row.
pub(crate) fn step_for_key(selected: usize, len: usize, key: &Key) -> Option<usize> {
    let last = len.checked_sub(1)?;
    let next = match key {
        Key::Named(NamedKey::Up) => selected.saturating_sub(1),
        Key::Named(NamedKey::Down) => (selected + 1).min(last),
        _ => return None,
    }
    .min(last);
    (next != selected).then_some(next)
}

/// What a [`Select`]'s [`crate::interactive::Interactive::handle`]
/// resolves one gesture to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectAction {
    /// The choice moved; carries the new index into `options`.
    Changed(usize),
}

impl crate::interactive::WidgetAction for SelectAction {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            SelectAction::Changed(selected_idx) => {
                crate::UiEvent::Form(id, FormEvent::DropdownChanged { id, selected_idx })
            }
        }
    }
}

impl crate::primitive::Primitive for Select {
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
            &self.field_kind(),
            self.disabled,
            self.focused,
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
            &self.field_kind(),
            self.disabled,
            self.focused,
            env.theme(),
        );
    }
}

impl crate::interactive::Interactive for Select {
    type State = ();
    type Action = SelectAction;

    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    fn a11y_actions(&self, _state: &()) -> Vec<crate::a11y::A11yAction> {
        if self.disabled {
            Vec::new()
        } else {
            vec![
                crate::a11y::A11yAction::Focus,
                crate::a11y::A11yAction::Click,
            ]
        }
    }

    /// Up/Down move and commit the choice; a left press (or the
    /// assistive-technology click) advances to the next option and takes
    /// focus. Enter and Space stay the app's, since there is no open step
    /// for them to perform.
    fn handle(
        &self,
        _state: &mut (),
        event: &crate::interactive::RoutedEvent,
        _layout: &BareFieldLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<SelectAction> {
        use crate::interactive::{PointerKind, RoutedEvent};

        let next = || {
            step_for_key(
                self.selected_idx,
                self.options.len(),
                &Key::Named(NamedKey::Down),
            )
        };
        match event {
            RoutedEvent::Key(key) => self.choose(step_for_key(
                self.selected_idx,
                self.options.len(),
                &key.key,
            )),
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                ..
            } => {
                cx.request_focus(None);
                match self.choose(next()) {
                    Handled::Ignored => Handled::Consumed,
                    other => other,
                }
            }
            RoutedEvent::Action(crate::a11y::A11yAction::Click) => self.choose(next()),
            RoutedEvent::Action(crate::a11y::A11yAction::Focus) => {
                cx.request_focus(None);
                Handled::Consumed
            }
            _ => Handled::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::{Interactive, KeyEvent, RoutedEvent};

    fn key(k: Key) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: k,
            modifiers: Default::default(),
            repeat: false,
        })
    }

    fn handle(select: &Select, event: RoutedEvent) -> Handled<SelectAction> {
        let layout = BareFieldLayout {
            area: Rect::new(0.0, 0.0, 10.0, 1.0),
            track: None,
        };
        crate::compose::widget_host::with_cx(select.id, crate::primitive::Units::Cells, |cx| {
            select.handle(&mut (), &event, &layout, cx)
        })
        .0
    }

    fn three() -> Select {
        Select::new(WidgetId::new("s"), vec!["a".into(), "b".into(), "c".into()])
    }

    #[test]
    fn up_and_down_move_and_commit_clamped_at_the_ends() {
        let mut select = three();
        assert_eq!(
            handle(&select, key(Key::Named(NamedKey::Down))),
            Handled::Event(SelectAction::Changed(1))
        );
        assert_eq!(
            handle(&select, key(Key::Named(NamedKey::Up))),
            Handled::Ignored
        );
        select.selected_idx = 2;
        assert_eq!(
            handle(&select, key(Key::Named(NamedKey::Down))),
            Handled::Ignored
        );
        assert_eq!(
            handle(&select, key(Key::Named(NamedKey::Enter))),
            Handled::Ignored
        );
    }

    #[test]
    fn an_empty_or_disabled_select_never_changes() {
        let empty = Select::new(WidgetId::new("s"), Vec::new());
        assert_eq!(
            handle(&empty, key(Key::Named(NamedKey::Down))),
            Handled::Ignored
        );
        let mut disabled = three();
        disabled.disabled = true;
        assert_eq!(
            handle(&disabled, key(Key::Named(NamedKey::Down))),
            Handled::Ignored
        );
    }

    /// The assistive-technology leg of the group-F event parity
    /// (`tests/group_f_event_parity.rs`): a `Click` action routed through
    /// [`crate::a11y::route_widget_action`] reports the same event a
    /// mouse click on the field does.
    #[test]
    fn a_click_action_matches_the_parity_event() {
        let id = WidgetId::new("sel");
        let select = Select::new(id, vec!["One".into(), "Two".into(), "Three".into()]);
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(id, Rect::new(0.0, 0.0, 16.0, 1.0), Box::new(select));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let ctx = crate::primitive::LayoutCtx::new(1.0, 1.0, false);
        let events = crate::a11y::route_widget_action(
            &tree,
            &mut states,
            &ctx,
            id,
            crate::a11y::A11yAction::Click,
        );
        assert_eq!(
            events,
            vec![crate::UiEvent::Form(
                id,
                crate::FormEvent::DropdownChanged {
                    id,
                    selected_idx: 1
                }
            )]
        );
    }
}
