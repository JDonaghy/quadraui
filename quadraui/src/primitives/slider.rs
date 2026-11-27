//! `Slider` primitive: a standalone numeric slider, on the widget
//! contract (`Primitive` + `Interactive`).
//!
//! A slider is the standalone form of [`FieldKind::Slider`]: it paints
//! through the same per-kind control painter a label-less `Form` row of
//! that kind uses, and its keyboard stepping and press-to-position math
//! are the functions a `Form` slider row uses too ([`step_for_key`],
//! [`value_at_fraction`]). The layout carries the track rect the painter
//! fills, so a press lands on the value painted under it. A press takes
//! the pointer and a drag keeps tracking until release. Every change
//! reaches the host as `UiEvent::Form(id, FormEvent::SliderChanged { .. })`,
//! the event a `Form` slider row already reports.

use serde::{Deserialize, Serialize};

use crate::event::{Key, NamedKey, Rect};
use crate::primitives::form::{BareFieldLayout, FieldKind, FormEvent};
use crate::types::WidgetId;
use crate::Handled;

fn default_step() -> f32 {
    1.0
}

/// A standalone numeric slider over `[min, max]`.
///
/// # Examples
///
/// ```
/// use quadraui::{Slider, WidgetId};
///
/// let slider = Slider::new(WidgetId::new("volume"), 5.0, 0.0, 10.0);
/// assert_eq!(slider.step, 1.0);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Slider {
    pub id: WidgetId,
    /// The current value.
    pub value: f32,
    /// The low end of the range.
    pub min: f32,
    /// The high end of the range.
    pub max: f32,
    /// How far one arrow key moves the value.
    #[serde(default = "default_step")]
    pub step: f32,
    /// A disabled slider paints dimmed and ignores every gesture.
    #[serde(default)]
    pub disabled: bool,
    /// Whether to paint the focused-row wash. The app owns it.
    #[serde(default)]
    pub focused: bool,
}

impl Slider {
    /// An enabled, unfocused slider at `value` over `[min, max]`, stepping
    /// by `1.0`.
    pub fn new(id: WidgetId, value: f32, min: f32, max: f32) -> Self {
        Self {
            id,
            value,
            min,
            max,
            step: default_step(),
            disabled: false,
            focused: false,
        }
    }

    /// The [`FieldKind`] this slider paints as.
    pub(crate) fn field_kind(&self) -> FieldKind {
        FieldKind::Slider {
            value: self.value,
            min: self.min,
            max: self.max,
            step: self.step,
        }
    }

    /// `value` as a change, or `None` when it is the current value.
    fn changed(&self, value: f32) -> Option<SliderAction> {
        (value != self.value).then_some(SliderAction::Changed(value))
    }

    /// The value a pointer at absolute `x` maps to across `track`.
    fn value_at(&self, track: Rect, x: f32) -> f32 {
        let fraction = if track.width > 0.0 {
            (x - track.x) / track.width
        } else {
            0.0
        };
        value_at_fraction(fraction, self.min, self.max)
    }
}

/// Clamps `raw` into `[min, max]` without [`f32::clamp`]'s panic on an
/// inverted range: `min`/`max` arrive from app or wire data, so a typo
/// inverting them degrades to a well-defined (if useless) clamp.
pub(crate) fn clamp_value(raw: f32, min: f32, max: f32) -> f32 {
    raw.max(min).min(max)
}

/// The value at `fraction` of the way along a `[min, max]` track,
/// `fraction` clamped to the track's two ends.
pub(crate) fn value_at_fraction(fraction: f32, min: f32, max: f32) -> f32 {
    clamp_value(min + fraction.clamp(0.0, 1.0) * (max - min), min, max)
}

/// The value one key moves a slider at `value` to: Left/Down and
/// Right/Up step by `step`, PageDown/PageUp by ten steps, Home/End jump
/// to the ends. `None` for any other key, or a move that leaves the
/// clamped value where it is. Shared by [`Slider`] and a `Form`'s
/// `Slider` row.
pub(crate) fn step_for_key(value: f32, min: f32, max: f32, step: f32, key: &Key) -> Option<f32> {
    let next = match key {
        Key::Named(NamedKey::Left) | Key::Named(NamedKey::Down) => value - step,
        Key::Named(NamedKey::Right) | Key::Named(NamedKey::Up) => value + step,
        Key::Named(NamedKey::PageDown) => value - step * PAGE_STEPS,
        Key::Named(NamedKey::PageUp) => value + step * PAGE_STEPS,
        Key::Named(NamedKey::Home) => min,
        Key::Named(NamedKey::End) => max,
        _ => return None,
    };
    let next = clamp_value(next, min, max);
    (next != value).then_some(next)
}

/// Steps one PageUp/PageDown moves a slider.
const PAGE_STEPS: f32 = 10.0;

/// [`crate::interactive::Interactive::State`] for [`Slider`]: whether a
/// press on the slider is still held, so pointer moves keep dragging the
/// value.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SliderState {
    /// A left press is dragging the value.
    pub dragging: bool,
}

/// What a [`Slider`]'s [`crate::interactive::Interactive::handle`]
/// resolves one gesture to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SliderAction {
    /// The value moved; carries the new value.
    Changed(f32),
}

impl crate::interactive::WidgetAction for SliderAction {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            SliderAction::Changed(value) => {
                crate::UiEvent::Form(id, FormEvent::SliderChanged { id, value })
            }
        }
    }
}

impl crate::primitive::Primitive for Slider {
    type Layout = BareFieldLayout;
    type Event = ();

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// One row tall; no natural width — the track fills whatever width
    /// the container gives it.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        crate::NaturalSize::new(None, Some(ctx.line_height.max(1.0)))
    }

    /// The whole rect, plus the track rect the painter fills and a press
    /// maps across.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> BareFieldLayout {
        match ctx.units() {
            #[cfg(feature = "tui")]
            crate::primitive::Units::Cells => {
                let area = crate::primitive::cell_grid_rect(rect);
                let (start, cells) = crate::primitives::form::cell_paint::bare_field_slider_track(
                    area.width as u16,
                    self.value,
                );
                BareFieldLayout {
                    area,
                    track: Some(Rect::new(
                        area.x + start as f32,
                        area.y,
                        cells as f32,
                        area.height.min(1.0),
                    )),
                }
            }
            _ => BareFieldLayout {
                area: rect,
                track: Some(crate::primitives::form::bare_pixel_slider_track(rect)),
            },
        }
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

impl crate::interactive::Interactive for Slider {
    type State = SliderState;
    type Action = SliderAction;

    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    fn a11y_actions(&self, _state: &SliderState) -> Vec<crate::a11y::A11yAction> {
        if self.disabled {
            Vec::new()
        } else {
            vec![
                crate::a11y::A11yAction::Focus,
                crate::a11y::A11yAction::Increment,
                crate::a11y::A11yAction::Decrement,
            ]
        }
    }

    /// The arrow, page and Home/End keys step the value; a left press on
    /// the track sets it to the value under the pointer and takes the
    /// pointer, so a drag keeps setting it until release.
    fn handle(
        &self,
        state: &mut SliderState,
        event: &crate::interactive::RoutedEvent,
        layout: &BareFieldLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<SliderAction> {
        use crate::a11y::A11yAction;
        use crate::interactive::{PointerKind, RoutedEvent};
        use crate::MouseButton;

        let report = |action: Option<SliderAction>| match action {
            Some(action) => Handled::Event(action),
            None => Handled::Consumed,
        };
        let track = layout.track.unwrap_or(layout.area);
        if self.disabled {
            return Handled::Ignored;
        }
        match event {
            RoutedEvent::Key(key) => {
                match step_for_key(self.value, self.min, self.max, self.step, &key.key) {
                    Some(value) => Handled::Event(SliderAction::Changed(value)),
                    None => Handled::Ignored,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => {
                cx.request_focus(None);
                cx.capture_pointer();
                state.dragging = true;
                report(self.changed(self.value_at(track, layout.area.x + local.x)))
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } if state.dragging => {
                report(self.changed(self.value_at(track, layout.area.x + local.x)))
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Up(MouseButton::Left),
                ..
            } if state.dragging => {
                state.dragging = false;
                cx.release_pointer();
                Handled::Consumed
            }
            RoutedEvent::Action(A11yAction::Increment) => report(
                self.changed(
                    step_for_key(
                        self.value,
                        self.min,
                        self.max,
                        self.step,
                        &Key::Named(NamedKey::Right),
                    )
                    .unwrap_or(self.value),
                ),
            ),
            RoutedEvent::Action(A11yAction::Decrement) => report(
                self.changed(
                    step_for_key(
                        self.value,
                        self.min,
                        self.max,
                        self.step,
                        &Key::Named(NamedKey::Left),
                    )
                    .unwrap_or(self.value),
                ),
            ),
            RoutedEvent::Action(A11yAction::Focus) => {
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
    use crate::interactive::{Interactive, KeyEvent, PointerKind, RoutedEvent};
    use crate::{MouseButton, Point};

    fn key(k: Key) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: k,
            modifiers: Default::default(),
            repeat: false,
        })
    }

    fn layout() -> BareFieldLayout {
        BareFieldLayout {
            area: Rect::new(0.0, 0.0, 20.0, 1.0),
            track: Some(Rect::new(2.0, 0.0, 10.0, 1.0)),
        }
    }

    fn handle(
        slider: &Slider,
        state: &mut SliderState,
        event: RoutedEvent,
    ) -> Handled<SliderAction> {
        crate::compose::widget_host::with_cx(slider.id, crate::primitive::Units::Cells, |cx| {
            slider.handle(state, &event, &layout(), cx)
        })
        .0
    }

    fn pointer(kind: PointerKind, x: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point::new(x, 0.0),
            mods: Default::default(),
        }
    }

    #[test]
    fn keys_step_page_and_jump_within_the_range() {
        let slider = Slider::new(WidgetId::new("s"), 5.0, 0.0, 100.0);
        let mut state = SliderState::default();
        for (k, want) in [
            (Key::Named(NamedKey::Right), 6.0),
            (Key::Named(NamedKey::Left), 4.0),
            (Key::Named(NamedKey::PageUp), 15.0),
            (Key::Named(NamedKey::PageDown), 0.0),
            (Key::Named(NamedKey::End), 100.0),
        ] {
            assert_eq!(
                handle(&slider, &mut state, key(k.clone())),
                Handled::Event(SliderAction::Changed(want)),
                "{k:?}"
            );
        }
        let at_min = Slider::new(WidgetId::new("s"), 0.0, 0.0, 100.0);
        assert_eq!(
            handle(&at_min, &mut state, key(Key::Named(NamedKey::Home))),
            Handled::Ignored
        );
    }

    #[test]
    fn a_press_sets_the_value_under_the_pointer_and_a_drag_follows_it() {
        let slider = Slider::new(WidgetId::new("s"), 0.0, 0.0, 10.0);
        let mut state = SliderState::default();
        assert_eq!(
            handle(
                &slider,
                &mut state,
                pointer(PointerKind::Down(MouseButton::Left), 7.0)
            ),
            Handled::Event(SliderAction::Changed(5.0))
        );
        assert!(state.dragging);
        assert_eq!(
            handle(&slider, &mut state, pointer(PointerKind::Move, 30.0)),
            Handled::Event(SliderAction::Changed(10.0))
        );
        let _ = handle(
            &slider,
            &mut state,
            pointer(PointerKind::Up(MouseButton::Left), 30.0),
        );
        assert!(!state.dragging);
        assert_eq!(
            handle(&slider, &mut state, pointer(PointerKind::Move, 2.0)),
            Handled::Ignored
        );
    }

    #[test]
    fn an_inverted_range_clamps_instead_of_panicking() {
        assert_eq!(clamp_value(5.0, 10.0, 0.0), 0.0);
        assert_eq!(value_at_fraction(2.0, 0.0, 10.0), 10.0);
    }
}
