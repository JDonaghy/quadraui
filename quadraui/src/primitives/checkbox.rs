//! `Checkbox` primitive: a standalone boolean toggle, on the widget
//! contract (`Primitive` + `Interactive`).
//!
//! A checkbox is the standalone form of [`FieldKind::Toggle`]: it paints
//! through the same per-kind control painter a label-less `Form` row of
//! that kind uses (`form::cell_paint::paint_bare_field` on a cell grid,
//! `form`'s pixel `paint_bare_field` elsewhere), so the two never drift
//! apart. Enter, Space, a left press and the assistive-technology click
//! action all flip [`Checkbox::value`]; the new value reaches the host as
//! `UiEvent::Form(id, FormEvent::ToggleChanged { .. })`, the event a
//! `Form` toggle row already reports.

use serde::{Deserialize, Serialize};

use crate::event::{Key, NamedKey, Rect};
use crate::primitives::form::{BareFieldLayout, FieldKind, FormEvent};
use crate::types::WidgetId;
use crate::Handled;

/// A standalone boolean toggle.
///
/// # Examples
///
/// ```
/// use quadraui::{Checkbox, WidgetId};
///
/// let checkbox = Checkbox::new(WidgetId::new("settings:wrap"));
/// assert!(!checkbox.value);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Checkbox {
    pub id: WidgetId,
    /// Whether the box is checked.
    #[serde(default)]
    pub value: bool,
    /// A disabled checkbox paints dimmed and ignores every gesture.
    #[serde(default)]
    pub disabled: bool,
    /// Whether to paint the focused-row wash. The app owns it, the same
    /// as every other visible field of a controlled widget.
    #[serde(default)]
    pub focused: bool,
}

impl Checkbox {
    /// An unchecked, enabled, unfocused checkbox.
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            value: false,
            disabled: false,
            focused: false,
        }
    }

    /// The [`FieldKind`] this checkbox paints as.
    pub(crate) fn field_kind(&self) -> FieldKind {
        FieldKind::Toggle { value: self.value }
    }

    /// The flip a press or an activation key produces, or `None` when
    /// disabled.
    fn toggle(&self) -> Option<CheckboxAction> {
        (!self.disabled).then_some(CheckboxAction::Toggled(!self.value))
    }
}

/// Whether `key` activates a toggle: Enter or Space — shared by
/// [`Checkbox`] and a `Form`'s `Toggle` row.
pub(crate) fn activates(key: &Key) -> bool {
    matches!(key, Key::Named(NamedKey::Enter) | Key::Char(' '))
}

/// What a [`Checkbox`]'s [`crate::interactive::Interactive::handle`]
/// resolves one gesture to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckboxAction {
    /// The box flipped; carries the new value.
    Toggled(bool),
}

impl crate::interactive::WidgetAction for CheckboxAction {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            CheckboxAction::Toggled(value) => {
                crate::UiEvent::Form(id, FormEvent::ToggleChanged { id, value })
            }
        }
    }
}

impl crate::primitive::Primitive for Checkbox {
    type Layout = BareFieldLayout;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// The box plus the one-cell (or bare-inset) lead-in a label-less
    /// form row gives its control: 4 cells on a cell grid, two text rows'
    /// width on a pixel surface; one row tall either way.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        match ctx.units() {
            crate::primitive::Units::Cells => crate::NaturalSize::new(Some(4.0), Some(1.0)),
            _ => crate::NaturalSize::new(Some(2.0 * ctx.line_height), Some(ctx.line_height)),
        }
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

impl crate::interactive::Interactive for Checkbox {
    type State = ();
    type Action = CheckboxAction;

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

    /// Enter/Space, a left press and the assistive-technology click all
    /// flip the box; a press also takes focus.
    fn handle(
        &self,
        _state: &mut (),
        event: &crate::interactive::RoutedEvent,
        _layout: &BareFieldLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<CheckboxAction> {
        use crate::interactive::{PointerKind, RoutedEvent};

        let flip = |toggle: Option<CheckboxAction>| match toggle {
            Some(action) => Handled::Event(action),
            None => Handled::Ignored,
        };
        match event {
            RoutedEvent::Key(key) if activates(&key.key) => flip(self.toggle()),
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                ..
            } => {
                cx.request_focus(None);
                flip(self.toggle())
            }
            RoutedEvent::Action(crate::a11y::A11yAction::Click) => flip(self.toggle()),
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

    fn handle(checkbox: &Checkbox, event: RoutedEvent) -> Handled<CheckboxAction> {
        let layout = BareFieldLayout {
            area: Rect::new(0.0, 0.0, 4.0, 1.0),
            track: None,
        };
        crate::compose::widget_host::with_cx(checkbox.id, crate::primitive::Units::Cells, |cx| {
            checkbox.handle(&mut (), &event, &layout, cx)
        })
        .0
    }

    #[test]
    fn enter_and_space_flip_the_value() {
        let mut checkbox = Checkbox::new(WidgetId::new("c"));
        assert_eq!(
            handle(&checkbox, key(Key::Named(NamedKey::Enter))),
            Handled::Event(CheckboxAction::Toggled(true))
        );
        checkbox.value = true;
        assert_eq!(
            handle(&checkbox, key(Key::Char(' '))),
            Handled::Event(CheckboxAction::Toggled(false))
        );
        assert_eq!(handle(&checkbox, key(Key::Char('x'))), Handled::Ignored);
    }

    #[test]
    fn a_disabled_checkbox_ignores_every_gesture() {
        let mut checkbox = Checkbox::new(WidgetId::new("c"));
        checkbox.disabled = true;
        assert_eq!(
            handle(&checkbox, key(Key::Named(NamedKey::Enter))),
            Handled::Ignored
        );
        assert!(checkbox.a11y_actions(&()).is_empty());
    }

    #[test]
    fn toggled_reports_as_the_form_toggle_event() {
        use crate::interactive::WidgetAction;
        let id = WidgetId::new("c");
        assert_eq!(
            CheckboxAction::Toggled(true).into_ui_event(id),
            crate::UiEvent::Form(id, FormEvent::ToggleChanged { id, value: true })
        );
    }

    /// The assistive-technology leg of the group-F event parity
    /// (`tests/group_f_event_parity.rs`): a `Click` action routed through
    /// [`crate::a11y::route_widget_action`] reports the same event a
    /// mouse click on the box does.
    #[test]
    fn a_click_action_matches_the_parity_event() {
        let id = WidgetId::new("cb");
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(
            id,
            Rect::new(0.0, 0.0, 6.0, 1.0),
            Box::new(Checkbox::new(id)),
        );
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
                FormEvent::ToggleChanged { id, value: true }
            )]
        );
    }
}
