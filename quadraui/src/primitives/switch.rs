//! `Switch` primitive: a standalone on/off toggle — layout, hit-test,
//! keyboard focus/activation, and its event, all on the widget contract
//! (`Primitive` + `Interactive`, below). `Switch` reaches a frame as a
//! type-erased `crate::frame::Surface::Widget`; there is no `Backend`
//! method or per-platform rasteriser file of its own.
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
//! [`crate::focus::FocusTree`] for the app-wide owner of "what has
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
    /// Whether the pointer currently sits over this switch. Set by the
    /// host from its own hover tracking (an [`crate::interaction::InteractionState`]
    /// keyed by [`Switch::id`], or any other source); read by this
    /// type's own shared pixel painter (`native_surface_paint::paint`)
    /// to tint the track toward [`crate::Theme::button_hover_bg`] — the
    /// same self-contained state convention [`Switch::focused`] already
    /// follows.
    #[serde(default)]
    pub hovered: bool,
    /// Whether the pointer currently holds this switch pressed. Same
    /// host-set convention as [`Switch::hovered`]; read by that same
    /// pixel painter to tint the track toward
    /// [`crate::Theme::button_pressed_bg`], which wins over
    /// [`Switch::hovered`] when both are set.
    #[serde(default)]
    pub pressed: bool,
}

impl Switch {
    /// An off, enabled, unfocused, unhovered, unpressed switch — set
    /// `value`/`disabled`/`focused`/`hovered`/`pressed` directly
    /// afterwards (every field is `pub`) rather than via a `with_*`
    /// chain — see `docs/PRIMITIVE_RULES.md`'s rule-8 history of why
    /// this crate stopped adding those.
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            value: false,
            disabled: false,
            focused: false,
            hovered: false,
            pressed: false,
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

impl crate::interactive::WidgetAction for SwitchEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            SwitchEvent::Toggled { value, .. } => {
                crate::UiEvent::SwitchToggled { widget: id, value }
            }
        }
    }
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────
//
// `Switch` reaches the frame type-erased as [`crate::frame::Surface::Widget`]:
// `layout` resolves the switch's own bounds — [`pixel_switch_layout`] on a
// pixel surface, [`cell_switch_layout`] on a cell grid, both of which are
// just [`Switch::layout`] itself since a switch has no unit-dependent
// geometry — `paint` runs the shared pixel painter in
// [`native_surface_paint`], `paint_cells` runs the cell painter in
// [`cell_paint`], and `handle` folds in [`Switch::handle_key`]/
// [`Switch::click`].

/// Resolve `switch`'s layout for a pixel surface — identical to
/// [`cell_switch_layout`] since [`Switch::layout`] has no unit-dependent
/// geometry of its own.
pub(crate) fn pixel_switch_layout(switch: &Switch, rect: Rect) -> SwitchLayout {
    switch.layout(rect)
}

/// Resolve `switch`'s layout for a cell grid — see [`pixel_switch_layout`].
pub(crate) fn cell_switch_layout(switch: &Switch, rect: Rect) -> SwitchLayout {
    switch.layout(rect)
}

impl crate::primitive::Primitive for Switch {
    type Layout = SwitchLayout;
    type Event = SwitchHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Switch(self.clone())
    }

    /// `[● ]`/`[ ●]` track: 4 cells on a cell grid, `2 * line_height` on
    /// a pixel surface — the same width [`crate::compose`]'s own
    /// `toggle_control_size` resolves for a `Switch` reached through a
    /// non-contract binding host, one row/line tall either way.
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

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> SwitchLayout {
        match ctx.units() {
            crate::primitive::Units::Cells => cell_switch_layout(self, rect),
            _ => pixel_switch_layout(self, rect),
        }
    }

    /// The shared pixel painter, against the already-resolved `layout`.
    fn paint(
        &self,
        layout: &SwitchLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        _style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let _ = native_surface_paint::paint(self, surface, theme, layout.bounds);
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme);
    }

    /// [`cell_paint::paint`] over `buf`, adapted through
    /// [`crate::tui::cell_surface::TuiCellSurface`] — the `Buffer` ->
    /// [`crate::CellSurface`] bridge every `tui` cell-grid painter builds
    /// for itself, `pub(crate)` rather than private to `crate::tui` for
    /// exactly this reach.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &SwitchLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        // The painter never moves the cursor, so the sink is never read.
        let mut cursor_sink = None;
        let mut surface = crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            &mut cursor_sink,
        );
        cell_paint::paint(self, layout, &mut surface, theme);
    }
}

impl crate::interactive::Interactive for Switch {
    type State = ();
    type Action = SwitchEvent;

    /// One focus stop per switch — the same self-contained convention
    /// `Switch::focused`'s own doc describes.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    fn a11y_actions(&self, _state: &()) -> Vec<crate::a11y::A11yAction> {
        if self.disabled {
            Vec::new()
        } else {
            vec![crate::a11y::A11yAction::Click]
        }
    }

    /// `Enter`/`Space` activates — note this does *not* call
    /// [`Switch::handle_key`], whose own `self.focused` gate is the
    /// legacy, non-contract convention a host sets by hand before
    /// calling it directly; the engine only ever routes a `Key` event to
    /// whichever widget its own `FocusTree` already names as focused
    /// (see `crate::runtime::route_widget_key`), so re-checking
    /// `self.focused` here would wrongly ignore the key on a descriptor
    /// a host rebuilds fresh every frame without separately mirroring
    /// that flag. A left click activates regardless of focus (see
    /// [`Switch::click`]), requesting focus first so a pointer
    /// activation leaves the switch focused the same way a keyboard
    /// activation requires it to already be.
    fn handle(
        &self,
        _state: &mut (),
        event: &crate::interactive::RoutedEvent,
        _layout: &SwitchLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<SwitchEvent> {
        use crate::interactive::{PointerKind, RoutedEvent};
        use crate::Handled;

        match event {
            RoutedEvent::Key(crate::interactive::KeyEvent {
                key: Key::Named(NamedKey::Enter) | Key::Char(' '),
                ..
            }) => match self.click() {
                Some(event) => Handled::Event(event),
                None => Handled::Ignored,
            },
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                ..
            } => match self.click() {
                Some(event) => {
                    cx.request_focus(None);
                    Handled::Event(event)
                }
                None => Handled::Ignored,
            },
            RoutedEvent::Action(crate::a11y::A11yAction::Click) => match self.click() {
                Some(event) => Handled::Event(event),
                None => Handled::Ignored,
            },
            _ => Handled::Ignored,
        }
    }
}

/// The cell-grid painter for [`Switch`], written against
/// [`crate::CellSurface`]. `Switch`'s own `paint_cells` adapts the raw
/// `Buffer` it's called with into that seam via
/// `crate::tui::cell_surface::TuiCellSurface`, the same bridge every
/// `tui` cell-grid painter builds for itself.
///
/// Paints a 4-cell `[● ]`/`[ ●]` track with the knob on the right when
/// `value` is `true`, on the left when `false`. The whole cell range is
/// one hit target, same as [`Switch::layout`].
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{Switch, SwitchLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;

    const TUI_SWITCH_WIDTH: u16 = 4;

    /// An opaque single-cell style — every glyph this painter writes
    /// sets both colours and no modifier.
    fn solid(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `switch` into `layout.bounds` on `surface`.
    ///
    /// `layout` must be the value [`super::cell_switch_layout`] returned
    /// for the same switch and rect, so paint and
    /// [`SwitchLayout::hit_test`] can never disagree.
    pub(crate) fn paint(
        switch: &Switch,
        layout: &SwitchLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let bounds = layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let y = bounds.y.round() as u16;
        let x0 = bounds.x.round() as u16;
        let w = bounds.width.round() as u16;
        let end = x0.saturating_add(w);

        let track_bg = if switch.disabled {
            theme.surface_bg
        } else if switch.value {
            theme.accent_bg
        } else {
            theme.surface_bg
        };
        let brk_fg = if switch.disabled {
            theme.muted_fg
        } else if switch.focused {
            theme.accent_fg
        } else {
            theme.border_fg
        };
        let knob_fg = if switch.disabled {
            theme.muted_fg
        } else {
            theme.foreground
        };

        let track_w = TUI_SWITCH_WIDTH.min(w);
        let knob_col = if track_w <= 2 {
            x0
        } else if switch.value {
            x0 + track_w - 2
        } else {
            x0 + 1
        };

        let mut col = x0;
        if col < end {
            surface.put_char(col, y, '[', solid(brk_fg, track_bg));
            col += 1;
        }
        while col < x0 + track_w.saturating_sub(1) && col < end {
            let ch = if col == knob_col { '●' } else { ' ' };
            let fg = if col == knob_col { knob_fg } else { track_bg };
            surface.put_char(col, y, ch, solid(fg, track_bg));
            col += 1;
        }
        if col < end {
            surface.put_char(col, y, ']', solid(brk_fg, track_bg));
            col += 1;
        }
        while col < end {
            surface.put_char(col, y, ' ', solid(knob_fg, theme.background));
            col += 1;
        }
    }
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
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a [`Switch`] into `rect` on `surface`, returning
    /// [`SwitchLayout`] for host click dispatch. Tints the track toward
    /// [`Theme::button_hover_bg`]/[`Theme::button_pressed_bg`] when
    /// [`Switch::hovered`]/[`Switch::pressed`] is set — the same
    /// [`PressableVisual`] priority (pressed beats hovered beats
    /// resting) [`crate::primitives::button::native_surface_paint::paint`]
    /// already resolves for [`crate::Button`].
    pub(crate) fn paint(
        switch: &Switch,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
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
            match PressableVisual::resolve(switch.pressed, false, switch.hovered) {
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
        fn resting_switch_paints_the_plain_track() {
            let switch = switch();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&switch, &mut surface, &theme, RECT);

            assert_eq!(track_fill(&surface), theme.surface_bg);
        }

        #[test]
        fn hovered_switch_tints_the_track_toward_button_hover_bg() {
            let mut switch = switch();
            switch.hovered = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&switch, &mut surface, &theme, RECT);

            assert_eq!(
                track_fill(&surface),
                theme.button_hover_bg(theme.surface_bg)
            );
        }

        #[test]
        fn pressed_switch_tints_the_track_toward_button_pressed_bg() {
            let mut switch = switch();
            switch.pressed = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&switch, &mut surface, &theme, RECT);

            assert_eq!(
                track_fill(&surface),
                theme.button_pressed_bg(theme.surface_bg)
            );
        }

        /// Pressed beats hovered — a switch can't be mid-press without
        /// also being hovered, and the deeper pressed tint must win.
        #[test]
        fn pressed_and_hovered_resolves_to_the_pressed_tint() {
            let mut switch = switch();
            switch.hovered = true;
            switch.pressed = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&switch, &mut surface, &theme, RECT);

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
            switch.hovered = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&switch, &mut surface, &theme, RECT);

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
    fn new_defaults_to_off_enabled_unfocused_and_untinted() {
        let s = switch();
        assert!(!s.value);
        assert!(!s.disabled);
        assert!(!s.focused);
        assert!(!s.hovered);
        assert!(!s.pressed);
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
        s.hovered = true;
        s.pressed = true;
        let json = serde_json::to_string(&s).unwrap();
        let back: Switch = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn serde_defaults_value_disabled_focused_hovered_pressed_to_false_when_omitted() {
        let json = r#"{"id":"settings:wrap"}"#;
        let s: Switch = serde_json::from_str(json).unwrap();
        assert!(!s.value);
        assert!(!s.disabled);
        assert!(!s.focused);
        assert!(!s.hovered);
        assert!(!s.pressed);
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent,
        WidgetAction,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::{Handled, MouseButton, Point};

    /// Minimal [`EventHost`] mock — see `crate::interactive`'s own test
    /// module for the shape this mirrors.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<crate::interactive::SubTarget>>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<crate::interactive::SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<crate::interactive::SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: crate::backend::PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn key_event(key: Key) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn down_event() -> RoutedEvent {
        RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(1.0, 0.0),
            mods: Modifiers::default(),
        }
    }

    #[test]
    fn declares_a_single_focus_stop() {
        assert_eq!(Switch::new(WidgetId::new("s")).focus(), FocusSpec::Stop);
    }

    #[test]
    fn primitive_layout_and_paint_cells_round_trip() {
        let mut s = Switch::new(WidgetId::new("settings:wrap"));
        s.value = true;
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let rect = Rect::new(0.0, 0.0, 4.0, 1.0);
        let layout = Primitive::layout(&s, &ctx, rect);
        assert_eq!(layout.bounds, rect);

        #[cfg(feature = "tui")]
        {
            let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 4, 1));
            Primitive::paint_cells(&s, &layout, &mut buf, &crate::theme::Theme::default());
            assert_eq!(buf[(1u16, 0u16)].symbol(), " ");
            assert_eq!(buf[(2u16, 0u16)].symbol(), "●");
        }
    }

    #[test]
    fn enabled_switch_advertises_a_click_action() {
        let s = Switch::new(WidgetId::new("s"));
        assert_eq!(s.a11y_actions(&()), vec![A11yAction::Click]);
    }

    #[test]
    fn disabled_switch_advertises_no_actions() {
        let mut s = Switch::new(WidgetId::new("s"));
        s.disabled = true;
        assert!(s.a11y_actions(&()).is_empty());
    }

    #[test]
    fn handle_enter_toggles() {
        let s = Switch::new(WidgetId::new("s"));
        let layout = SwitchLayout {
            bounds: Rect::new(0.0, 0.0, 4.0, 1.0),
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(s.id, &mut host);
        let outcome = s.handle(
            &mut (),
            &key_event(Key::Named(NamedKey::Enter)),
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(SwitchEvent::Toggled {
                id: s.id,
                value: true
            })
        );
    }

    #[test]
    fn handle_click_requests_focus_and_toggles() {
        let s = Switch::new(WidgetId::new("s"));
        let layout = SwitchLayout {
            bounds: Rect::new(0.0, 0.0, 4.0, 1.0),
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(s.id, &mut host);
        let outcome = s.handle(&mut (), &down_event(), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(SwitchEvent::Toggled {
                id: s.id,
                value: true
            })
        );
        assert_eq!(host.focus_requests, vec![None]);
    }

    /// Three-way event parity's in-crate leg: an [`A11yAction::Click`]
    /// routed through [`crate::a11y::route_widget_action`] (the
    /// assistive-technology path a screen reader's default action takes)
    /// produces the exact same [`crate::UiEvent`] a direct
    /// [`Interactive::handle`] call with the same `RoutedEvent::Action`
    /// does. The TUI mouse-click leg lives in
    /// `tests/widget_contract_switch.rs`, which drives the real
    /// `event -> handle -> render` path `TuiDriver` exercises.
    #[test]
    fn route_widget_action_click_matches_direct_interactive_handle() {
        let s = Switch::new(WidgetId::new("settings:wrap"));
        let id = s.id;
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(id, Rect::new(0.0, 0.0, 4.0, 1.0), Box::new(s.clone()));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let ctx = LayoutCtx::new(1.0, 1.0, false);

        let events =
            crate::a11y::route_widget_action(&tree, &mut states, &ctx, id, A11yAction::Click);

        let layout = SwitchLayout {
            bounds: Rect::new(0.0, 0.0, 4.0, 1.0),
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(id, &mut host);
        let direct = s.handle(
            &mut (),
            &RoutedEvent::Action(A11yAction::Click),
            &layout,
            &mut cx,
        );
        let expected = match direct {
            Handled::Event(ev) => vec![ev.into_ui_event(id)],
            _ => vec![],
        };
        assert_eq!(events, expected);
        assert_eq!(
            events,
            vec![SwitchEvent::Toggled { id, value: true }.into_ui_event(id)]
        );
    }
}
