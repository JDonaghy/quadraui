//! `Switch` primitive: a standalone on/off toggle — layout, hit-test,
//! keyboard focus/activation, and its event. Layout/hit-test/keyboard
//! logic lives here; backend rasterisers live in `tui::switch` and
//! inline in each platform's `backend.rs` (see `Backend::draw_switch`).
//!
//! Today `Form`'s `Toggle`/`ToggleGroup` field kinds cover the same
//! on/off shape embedded inside a form. `Switch` is additive: a
//! standalone control for a host that wants a toggle outside a form
//! (a settings panel, a toolbar, a status strip) without pulling in the
//! whole `Form`/`FormController` machinery. `Form`'s own field kinds
//! are untouched.
//!
//! # Focus and activation
//!
//! A `Switch` tracks its own [`Switch::focused`] flag, the same
//! self-contained pattern [`crate::PipelineView`] and [`crate::Toolbar`]
//! already use for single-widget keyboard state — there is exactly one
//! focusable target per `Switch`, so (unlike [`crate::RadioGroup`]'s
//! multi-item cursor) no wraparound index is needed. [`Switch::handle_key`]
//! activates (flips [`Switch::value`]) on `Enter` or `Space` while
//! focused and not [`Switch::disabled`]; [`Switch::click`] does the same
//! for a pointer click, gated on `disabled` alone (a mouse click doesn't
//! require prior keyboard focus). Both return the same [`SwitchEvent`]
//! so a host wires one event type regardless of input modality.
//!
//! Routing app-wide Tab into `Switch::focused` is the host's job —
//! same division of responsibility as every other primitive here: this
//! type owns what happens once it has focus, not how focus arrives (see
//! [`crate::focus::FocusManager`] for the app-wide owner of "what has
//! focus").

use crate::event::{Key, NamedKey, Point, Rect};
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a standalone on/off switch.
///
/// # Examples
///
/// ```
/// use quadraui::{Key, Modifiers, NamedKey, Point, Rect, Switch, SwitchEvent, SwitchHit, WidgetId};
///
/// let mut switch = Switch::new(WidgetId::new("settings:wrap"));
/// switch.focused = true;
///
/// let layout = switch.layout(Rect::new(0.0, 0.0, 8.0, 1.0));
/// assert_eq!(layout.hit_test(Point::new(1.0, 0.0)), SwitchHit::Body);
///
/// let event = switch.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default());
/// assert_eq!(
///     event,
///     Some(SwitchEvent::Toggled { id: switch.id.clone(), value: true })
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Switch {
    pub id: WidgetId,
    /// Current on/off state.
    #[serde(default)]
    pub value: bool,
    /// When `true`, [`Switch::handle_key`] and [`Switch::click`] both
    /// return `None` — same "disabled means inert, not hidden" contract
    /// every other primitive's `disabled` flag follows.
    #[serde(default)]
    pub disabled: bool,
    /// Whether this switch currently holds keyboard focus. Set by the
    /// host's focus-traversal logic; read by [`Switch::handle_key`] to
    /// gate activation.
    #[serde(default)]
    pub focused: bool,
}

impl Switch {
    /// An off, enabled, unfocused switch — set `value`/`disabled`/
    /// `focused` directly afterwards (every field is `pub`) rather than
    /// via a `with_*` chain — see `docs/PRIMITIVE_RULES.md`'s rule-8
    /// history of why this crate stopped adding those.
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            value: false,
            disabled: false,
            focused: false,
        }
    }

    /// Resolve the switch's hit-testable bounds. A `Switch` has no
    /// internal sub-regions (unlike [`crate::RadioGroup`]'s per-option
    /// list) — the whole box is one hit target.
    pub fn layout(&self, bounds: Rect) -> SwitchLayout {
        SwitchLayout { bounds }
    }

    /// `Enter`/`Space` flips [`Switch::value`] when focused and enabled.
    /// Any other key is unhandled (`None`) — the same "return `None` for
    /// a key this control doesn't own" contract [`crate::Toolbar`] and
    /// [`crate::PipelineView`] already follow.
    pub fn handle_key(&self, key: &Key, _modifiers: &Modifiers) -> Option<SwitchEvent> {
        if self.disabled || !self.focused {
            return None;
        }
        match key {
            Key::Named(NamedKey::Enter) | Key::Char(' ') => Some(SwitchEvent::Toggled {
                id: self.id,
                value: !self.value,
            }),
            _ => None,
        }
    }

    /// Pointer activation: flips [`Switch::value`] when enabled,
    /// regardless of [`Switch::focused`] (a click doesn't require prior
    /// keyboard focus the way `Enter`/`Space` does). The host calls this
    /// after confirming a `MouseDown`/`MouseUp` landed on
    /// [`SwitchHit::Body`] via [`SwitchLayout::hit_test`].
    pub fn click(&self) -> Option<SwitchEvent> {
        if self.disabled {
            return None;
        }
        Some(SwitchEvent::Toggled {
            id: self.id,
            value: !self.value,
        })
    }
}

impl Default for Switch {
    /// An off, enabled, unfocused switch with an empty id. See
    /// [`Switch::new`] for a switch with a real id.
    fn default() -> Self {
        Self::new(WidgetId::new(""))
    }
}

/// Fully-resolved switch layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwitchLayout {
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchHit {
    Body,
    Outside,
}

impl SwitchLayout {
    pub fn hit_test(&self, p: Point) -> SwitchHit {
        if self.bounds.contains(p) {
            SwitchHit::Body
        } else {
            SwitchHit::Outside
        }
    }
}

/// Emitted by [`Switch::handle_key`]/[`Switch::click`] when the switch's
/// value flips. Carries the *new* value so a host doesn't need to
/// re-read `Switch::value` after applying the event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchEvent {
    Toggled { id: WidgetId, value: bool },
}

/// `paint` is the shared [`crate::paint_surface::PaintSurface`]
/// implementation of switch painting (track/thumb), used by backends
/// that have no dedicated native rasteriser for `Switch` — the same
/// minimal-chrome fallback [`crate::primitives::progress`] already
/// uses for `ProgressBar`. Chrome-only (no label of its own), so the
/// caller is expected to also register a click zone — see
/// [`crate::Backend::draw_focus_ring`]'s doc for that pattern.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Switch, SwitchLayout};
    use crate::backend::PressableVisual;
    use crate::event::Rect;
    use crate::interaction::InteractionState;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a [`Switch`] into `rect` on `surface`, returning
    /// [`SwitchLayout`] for host click dispatch. Identical to calling
    /// [`paint_with_state`] with an empty [`InteractionState`] — no
    /// hover/pressed tint — which is what every existing call site
    /// (none of which track hover yet) keeps getting.
    pub(crate) fn paint(
        switch: &Switch,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
    ) -> SwitchLayout {
        paint_with_state(switch, surface, theme, rect, &InteractionState::new())
    }

    /// [`paint`], additionally tinting the track toward
    /// [`Theme::button_hover_bg`]/[`Theme::button_pressed_bg`] when
    /// `interaction` reports [`Switch::id`] as hovered/pressed — the
    /// same [`PressableVisual`] priority (pressed beats hovered beats
    /// resting) [`crate::primitives::button::native_surface_paint::paint`]
    /// already resolves for [`crate::Button`]. No backend calls this
    /// yet (see [`crate::Backend::draw_switch_with_state`]); it exists
    /// so a backend that starts resolving [`InteractionState`] for
    /// `Switch` has one paint call site ready.
    pub(crate) fn paint_with_state(
        switch: &Switch,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
        interaction: &InteractionState,
    ) -> SwitchLayout {
        let layout = switch.layout(rect);
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return layout;
        }

        let resting_track = if switch.value {
            theme.accent_bg
        } else {
            theme.surface_bg
        };
        let track_color = if switch.disabled {
            theme.surface_bg
        } else {
            let is_hovered = interaction.is_hovered(&switch.id);
            let is_pressed = interaction.is_pressed(&switch.id);
            match PressableVisual::resolve(is_pressed, false, is_hovered) {
                PressableVisual::PressedOrActive => theme.button_pressed_bg(resting_track),
                PressableVisual::Hovered => theme.button_hover_bg(resting_track),
                PressableVisual::Resting => resting_track,
            }
        };
        let radius = (rect.height / 2.0).max(0.0);
        surface.surface_fill_rounded_rect(rect, radius, track_color);

        let border = if switch.disabled {
            theme.muted_fg
        } else if switch.focused {
            theme.accent_fg
        } else {
            theme.border_fg
        };
        surface.surface_stroke_rounded_rect(rect, radius, border, 1.0);

        // Thumb: a filled circle at the track's "on" or "off" end.
        let thumb_diameter = rect.height.max(0.0);
        let thumb_x = if switch.value {
            rect.x + (rect.width - thumb_diameter).max(0.0)
        } else {
            rect.x
        };
        let thumb_color = if switch.disabled {
            theme.muted_fg
        } else {
            theme.foreground
        };
        let thumb_rect = Rect::new(thumb_x, rect.y, thumb_diameter, thumb_diameter);
        surface.surface_fill_rounded_rect(thumb_rect, thumb_diameter / 2.0, thumb_color);

        layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Viewport};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every fill/stroke call — enough to assert the
        /// track's resting vs. hovered vs. pressed fill colour without
        /// a Cairo/Core Graphics/Direct2D host.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, f32, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
                self.fills.push((rect, radius, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _sw: f32) {}
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        const RECT: Rect = Rect::new(0.0, 0.0, 20.0, 10.0);

        fn switch() -> Switch {
            Switch::new(WidgetId::new("settings:wrap"))
        }

        /// The track's first (bottom-most, since it's pushed first)
        /// fill colour — the track, not the thumb (pushed second).
        fn track_fill(surface: &RecordingSurface) -> Color {
            surface.fills.first().expect("track must paint a fill").2
        }

        #[test]
        fn paint_matches_paint_with_state_against_an_empty_interaction() {
            let switch = switch();
            let theme = Theme::default();
            let mut via_paint = RecordingSurface::default();
            let mut via_with_state = RecordingSurface::default();

            paint(&switch, &mut via_paint, &theme, RECT);
            paint_with_state(
                &switch,
                &mut via_with_state,
                &theme,
                RECT,
                &InteractionState::new(),
            );

            assert_eq!(via_paint.fills, via_with_state.fills);
        }

        #[test]
        fn hovered_switch_tints_the_track_toward_button_hover_bg() {
            let switch = switch();
            let theme = Theme::default();
            let mut interaction = InteractionState::new();
            interaction.set_hovered(Some(switch.id));
            let mut surface = RecordingSurface::default();

            paint_with_state(&switch, &mut surface, &theme, RECT, &interaction);

            assert_eq!(
                track_fill(&surface),
                theme.button_hover_bg(theme.surface_bg)
            );
        }

        #[test]
        fn pressed_switch_tints_the_track_toward_button_pressed_bg() {
            let switch = switch();
            let theme = Theme::default();
            let mut interaction = InteractionState::new();
            interaction.set_pressed(Some(switch.id));
            let mut surface = RecordingSurface::default();

            paint_with_state(&switch, &mut surface, &theme, RECT, &interaction);

            assert_eq!(
                track_fill(&surface),
                theme.button_pressed_bg(theme.surface_bg)
            );
        }

        /// Pressed beats hovered — a switch can't be mid-press without
        /// also being hovered, and the deeper pressed tint must win.
        #[test]
        fn pressed_and_hovered_resolves_to_the_pressed_tint() {
            let switch = switch();
            let theme = Theme::default();
            let mut interaction = InteractionState::new();
            interaction.set_hovered(Some(switch.id));
            interaction.set_pressed(Some(switch.id));
            let mut surface = RecordingSurface::default();

            paint_with_state(&switch, &mut surface, &theme, RECT, &interaction);

            assert_eq!(
                track_fill(&surface),
                theme.button_pressed_bg(theme.surface_bg)
            );
        }

        /// A disabled switch must not tint even while reported hovered
        /// — "disabled means inert" extends to the hover/pressed tint
        /// too, not just click/key dispatch.
        #[test]
        fn disabled_switch_does_not_tint_when_hovered() {
            let mut switch = switch();
            switch.disabled = true;
            let theme = Theme::default();
            let mut interaction = InteractionState::new();
            interaction.set_hovered(Some(switch.id));
            let mut surface = RecordingSurface::default();

            paint_with_state(&switch, &mut surface, &theme, RECT, &interaction);

            assert_eq!(track_fill(&surface), theme.surface_bg);
        }

        /// Interaction state keyed by a different widget id must not
        /// bleed into this switch's tint.
        #[test]
        fn unrelated_hovered_id_does_not_tint_the_track() {
            let switch = switch();
            let theme = Theme::default();
            let mut interaction = InteractionState::new();
            interaction.set_hovered(Some(WidgetId::new("some:other:widget")));
            let mut surface = RecordingSurface::default();

            paint_with_state(&switch, &mut surface, &theme, RECT, &interaction);

            assert_eq!(track_fill(&surface), theme.surface_bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switch() -> Switch {
        Switch::new(WidgetId::new("settings:wrap"))
    }

    #[test]
    fn new_defaults_to_off_enabled_and_unfocused() {
        let s = switch();
        assert!(!s.value);
        assert!(!s.disabled);
        assert!(!s.focused);
    }

    #[test]
    fn hit_test_inside_and_outside() {
        let s = switch();
        let layout = s.layout(Rect::new(2.0, 2.0, 6.0, 1.0));
        assert_eq!(layout.hit_test(Point::new(3.0, 2.0)), SwitchHit::Body);
        assert_eq!(layout.hit_test(Point::new(0.0, 0.0)), SwitchHit::Outside);
    }

    #[test]
    fn handle_key_toggles_on_enter_when_focused_and_enabled() {
        let mut s = switch();
        s.focused = true;
        let event = s.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default());
        assert_eq!(
            event,
            Some(SwitchEvent::Toggled {
                id: s.id.clone(),
                value: true
            })
        );
    }

    #[test]
    fn handle_key_toggles_on_space() {
        let mut s = switch();
        s.focused = true;
        s.value = true;
        let event = s.handle_key(&Key::Char(' '), &Modifiers::default());
        assert_eq!(
            event,
            Some(SwitchEvent::Toggled {
                id: s.id.clone(),
                value: false
            })
        );
    }

    #[test]
    fn handle_key_ignores_unrelated_keys() {
        let mut s = switch();
        s.focused = true;
        assert_eq!(
            s.handle_key(&Key::Named(NamedKey::Tab), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_not_focused() {
        let s = switch();
        assert_eq!(
            s.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_disabled() {
        let mut s = switch();
        s.focused = true;
        s.disabled = true;
        assert_eq!(
            s.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn click_toggles_without_requiring_focus() {
        let s = switch();
        assert_eq!(
            s.click(),
            Some(SwitchEvent::Toggled {
                id: s.id.clone(),
                value: true
            })
        );
    }

    #[test]
    fn click_is_inert_when_disabled() {
        let mut s = switch();
        s.disabled = true;
        assert_eq!(s.click(), None);
    }

    #[test]
    fn serde_round_trip() {
        let mut s = switch();
        s.value = true;
        s.focused = true;
        let json = serde_json::to_string(&s).unwrap();
        let back: Switch = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn serde_defaults_value_disabled_focused_to_false_when_omitted() {
        let json = r#"{"id":"settings:wrap"}"#;
        let s: Switch = serde_json::from_str(json).unwrap();
        assert!(!s.value);
        assert!(!s.disabled);
        assert!(!s.focused);
    }
}
