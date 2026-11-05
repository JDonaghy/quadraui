//! `Button` primitive: a standalone, clickable action control — primary,
//! secondary, and icon variants, with keyboard focus/activation, plus
//! its disabled state and event. Layout/hit-test/keyboard logic lives
//! here; backend rasterisers live in `tui::button` and inline in each
//! platform's `backend.rs` (see `Backend::draw_button`).
//!
//! Today `Form`'s `Button`/`ButtonRow` field kinds cover the same shape
//! embedded inside a form. `Button` is additive: a standalone control
//! for a host that wants a button outside a form (a toolbar action, a
//! dialog's own footer, a palette entry) without pulling in the whole
//! `Form`/`FormController` machinery. `Form`'s own field kinds are
//! untouched.
//!
//! # Focus and activation
//!
//! A `Button` tracks its own [`Button::focused`] flag, the same
//! self-contained pattern [`crate::PipelineView`] and [`crate::Toolbar`]
//! already use for single-widget keyboard state — there is exactly one
//! focusable target per `Button`. [`Button::handle_key`] activates on
//! `Enter` or `Space` while focused and not [`Button::disabled`];
//! [`Button::click`] does the same for a pointer click, gated on
//! `disabled` alone. Both return the same [`ButtonEvent`] so a host
//! wires one event type regardless of input modality.
//!
//! Routing app-wide Tab into `Button::focused` is the host's job — same
//! division of responsibility as every other primitive here: this type
//! owns what happens once it has focus, not how focus arrives (see
//! [`crate::focus::FocusManager`] for the app-wide owner of "what has
//! focus").

use crate::backend::PointerShape;
use crate::event::{Key, NamedKey, Point, Rect};
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Visual emphasis variant. Purely descriptive — `Button` carries
/// no backend painting, so a rasteriser reads this field to choose its
/// own chrome (filled vs. outlined vs. icon-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ButtonVariant {
    /// The call-to-action for its surface (e.g. a dialog's "OK").
    #[default]
    Primary,
    /// A lower-emphasis action alongside a primary one (e.g. "Cancel").
    Secondary,
    /// Icon-only, no label chrome (e.g. a toolbar glyph button).
    Icon,
}

/// Declarative description of a standalone button.
///
/// # Examples
///
/// ```
/// use quadraui::{Button, ButtonEvent, ButtonHit, Key, Modifiers, NamedKey, Point, Rect, WidgetId};
///
/// let mut button = Button::new(WidgetId::new("dialog:ok"), "OK");
/// button.focused = true;
///
/// let layout = button.layout(Rect::new(0.0, 0.0, 6.0, 1.0));
/// assert_eq!(layout.hit_test(Point::new(1.0, 0.0)), ButtonHit::Body);
///
/// let event = button.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default());
/// assert_eq!(event, Some(ButtonEvent::Activated { id: button.id.clone() }));
/// ```
///
/// `#[non_exhaustive]` rejects a struct literal from outside this crate —
/// including a literal naming every field with a trailing
/// `..Default::default()` — which is exactly what makes a future field
/// addition non-breaking for a downstream consumer:
///
/// ```compile_fail,E0639
/// use quadraui::{Button, ButtonVariant, WidgetId};
///
/// let _ = Button {
///     id: WidgetId::new("dialog:ok"),
///     label: "OK".to_string(),
///     variant: ButtonVariant::default(),
///     disabled: false,
///     focused: false,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Button {
    pub id: WidgetId,
    /// Visible label. Ignored (but still present) for
    /// [`ButtonVariant::Icon`] — a host typically pairs an icon button
    /// with an [`crate::Icon`] carried alongside the button itself rather
    /// than through it, same as [`crate::Toolbar`]'s items.
    pub label: String,
    #[serde(default)]
    pub variant: ButtonVariant,
    /// When `true`, [`Button::handle_key`] and [`Button::click`] both
    /// return `None` — same "disabled means inert, not hidden" contract
    /// every other primitive's `disabled` flag follows.
    #[serde(default)]
    pub disabled: bool,
    /// Whether this button currently holds keyboard focus. Set by the
    /// host's focus-traversal logic; read by [`Button::handle_key`] to
    /// gate activation.
    #[serde(default)]
    pub focused: bool,
}

impl Button {
    /// A primary, enabled, unfocused button with `label`. Set
    /// `variant`/`disabled`/`focused` directly afterwards (every field
    /// is `pub`) rather than via a `with_*` chain — see
    /// `docs/PRIMITIVE_RULES.md`'s rule-8 history of why this crate
    /// stopped adding those.
    pub fn new(id: WidgetId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            variant: ButtonVariant::Primary,
            disabled: false,
            focused: false,
        }
    }

    /// Resolve the button's hit-testable bounds. A `Button` has no
    /// internal sub-regions — the whole box is one hit target.
    pub fn layout(&self, bounds: Rect) -> ButtonLayout {
        ButtonLayout { bounds }
    }

    /// `Enter`/`Space` activates when focused and enabled. Any other
    /// key is unhandled (`None`) — the same "return `None` for a key
    /// this control doesn't own" contract [`crate::Toolbar`] and
    /// [`crate::PipelineView`] already follow.
    pub fn handle_key(&self, key: &Key, _modifiers: &Modifiers) -> Option<ButtonEvent> {
        if self.disabled || !self.focused {
            return None;
        }
        match key {
            Key::Named(NamedKey::Enter) | Key::Char(' ') => {
                Some(ButtonEvent::Activated { id: self.id })
            }
            _ => None,
        }
    }

    /// Pointer activation: fires when enabled, regardless of
    /// [`Button::focused`] (a click doesn't require prior keyboard
    /// focus the way `Enter`/`Space` does). The host calls this after
    /// confirming a `MouseDown`/`MouseUp` landed on [`ButtonHit::Body`]
    /// via [`ButtonLayout::hit_test`].
    pub fn click(&self) -> Option<ButtonEvent> {
        if self.disabled {
            return None;
        }
        Some(ButtonEvent::Activated { id: self.id })
    }
}

impl Default for Button {
    /// A primary, enabled, unfocused button with an empty id and label.
    /// See [`Button::new`] for a button with a real id/label.
    fn default() -> Self {
        Self::new(WidgetId::new(""), "")
    }
}

/// Fully-resolved button layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ButtonLayout {
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonHit {
    Body,
    Outside,
}

impl ButtonLayout {
    pub fn hit_test(&self, p: Point) -> ButtonHit {
        if self.bounds.contains(p) {
            ButtonHit::Body
        } else {
            ButtonHit::Outside
        }
    }

    /// Pointer glyph a backend should request while the mouse sits at `p` —
    /// a hand over the clickable body, the default arrow outside it.
    ///
    /// Always treats the body as clickable, because `ButtonLayout` carries
    /// only [`Self::bounds`] and has no view of [`Button::disabled`]. A
    /// caller that knows which [`Button`] produced this layout should use
    /// [`Self::pointer_shape_for`] instead, which returns the default arrow
    /// over a disabled button's body rather than a hand over dead space.
    pub fn pointer_shape(&self, p: Point) -> PointerShape {
        match self.hit_test(p) {
            ButtonHit::Body => PointerShape::Pointer,
            ButtonHit::Outside => PointerShape::Default,
        }
    }

    /// [`Self::pointer_shape`], but honest about a disabled button: the
    /// body of a disabled `button` isn't clickable (see [`Button::click`]),
    /// so it gets the default arrow instead of a hand, matching what native
    /// toolkits show over dead controls.
    pub fn pointer_shape_for(&self, button: &Button, p: Point) -> PointerShape {
        if button.disabled {
            return PointerShape::Default;
        }
        self.pointer_shape(p)
    }
}

/// Emitted by [`Button::handle_key`]/[`Button::click`] when the button
/// activates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonEvent {
    Activated { id: WidgetId },
}

/// `paint` is the shared [`crate::paint_surface::PaintSurface`]
/// implementation of button painting (fill/border/label), used by
/// backends that have no dedicated native rasteriser for `Button` —
/// the same minimal-chrome fallback [`crate::primitives::progress`]
/// already uses for `ProgressBar`.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Button, ButtonLayout, ButtonVariant};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;

    /// Outset gap between a primitive's own edge and its focus-visible
    /// ring, in surface-native units (GTK/macOS points, Win-GUI DIPs).
    const FOCUS_RING_GAP: f32 = 2.0;

    /// Resolved pixel geometry for a primitive's focus-visible ring:
    /// the outset rect to stroke, the corner radius to stroke it with,
    /// and the stroke width.
    pub(crate) struct FocusRingGeometry {
        pub(crate) rect: Rect,
        pub(crate) corner_radius: f32,
        pub(crate) width: f32,
    }

    /// Shared geometry for a focus-visible ring drawn around `bounds`:
    /// an outset stroke rect ([`FOCUS_RING_GAP`] clear of `bounds` on
    /// every side), `style.corner_radius` (read from `style` itself, so
    /// there is no separate radius argument for a call site to drift
    /// from the primitive's own fill/border radius), and the stroke
    /// width [`Style::focus_ring_width`] configures. This module's own
    /// [`paint`] below computes its ring through here; a per-backend
    /// `TextInput` rasteriser that adds a ring stroke should too, rather
    /// than re-deriving the outset gap independently, so widening
    /// `Style::focus_ring_width` widens every primitive's ring
    /// identically instead of drifting through hand-tuned literals.
    ///
    /// Distinct from [`crate::backend::Backend::draw_focus_ring`], which
    /// every backend already implements by stroking the bare rect with
    /// `theme.accent_fg` — no outset gap, square corners, and not
    /// `Theme::focus_ring_color`. The two conventions aren't unified
    /// here; a caller choosing between them gets a differently-shaped
    /// ring either way.
    ///
    /// Note for a rasteriser that calls this with `TextInput`'s own
    /// bounds: `TextInput`'s border is always stroked square
    /// (`surface_stroke_rect`, no radius), so a ring stroked at
    /// `style.corner_radius` > 0 traces a rounded outline around a
    /// square input unless the call site strokes the ring square too.
    pub(crate) fn focus_ring_geometry(bounds: Rect, style: &Style) -> FocusRingGeometry {
        FocusRingGeometry {
            rect: Rect::new(
                bounds.x - FOCUS_RING_GAP,
                bounds.y - FOCUS_RING_GAP,
                bounds.width + FOCUS_RING_GAP * 2.0,
                bounds.height + FOCUS_RING_GAP * 2.0,
            ),
            corner_radius: style.corner_radius,
            width: style.focus_ring_width,
        }
    }

    /// Paint a [`Button`] into `rect` on `surface`, returning
    /// [`ButtonLayout`] for host click dispatch. `style` supplies the
    /// corner radius the fill/border paint with.
    ///
    /// `is_hovered` / `is_pressed` are `false` for the plain, non-state
    /// `Backend::draw_button` call sites — identical paint to before
    /// hover support existed. `Backend::draw_button_with_state`
    /// implementations pass the resolved
    /// `interaction.is_hovered(&button.id)` /
    /// `interaction.is_pressed(&button.id)` instead, which tints `bg`
    /// via [`Theme::button_pressed_bg`] (pressed takes priority over
    /// hover, matching every other `InteractionState`-driven
    /// rasteriser in this crate — a button can't be mid-press without
    /// also being hovered).
    pub(crate) fn paint(
        button: &Button,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
        style: &Style,
        is_hovered: bool,
        is_pressed: bool,
    ) -> ButtonLayout {
        let layout = button.layout(rect);
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return layout;
        }

        let bg = if button.disabled {
            theme.surface_bg
        } else {
            match button.variant {
                ButtonVariant::Primary => theme.accent_bg,
                ButtonVariant::Secondary | ButtonVariant::Icon => theme.surface_bg,
            }
        };
        let bg = if button.disabled {
            bg
        } else if is_pressed {
            theme.button_pressed_bg(bg)
        } else if is_hovered {
            theme.button_hover_bg(bg)
        } else {
            bg
        };
        surface.surface_fill_rounded_rect(rect, style.corner_radius, bg);

        if !matches!(button.variant, ButtonVariant::Icon) {
            let border = if button.disabled {
                theme.muted_fg
            } else if button.focused {
                theme.accent_fg
            } else {
                theme.border_fg
            };
            surface.surface_stroke_rounded_rect(rect, style.corner_radius, border, 1.0);
        }

        // Focus-visible ring: a second, outset stroke in the theme's
        // ring colour, independent of the border tint above — the same
        // `Style::focus_ring_width` every `Backend::draw_focus_ring`
        // caller reads, so a consumer that widens the ring widens this
        // one too. Suppressed when disabled (nothing to focus) and left
        // to the caller otherwise: `Button::focused` carries no "how did
        // focus arrive" information, so a host that wants focus-visible
        // suppression after a mouse click (VS Code's convention) clears
        // `focused` itself on click rather than on Tab — see
        // `crate::compose::FocusRing::is_focus_visible`.
        if button.focused && !button.disabled {
            let ring = focus_ring_geometry(rect, style);
            surface.surface_stroke_rounded_rect(
                ring.rect,
                ring.corner_radius,
                theme.focus_ring_color(),
                ring.width,
            );
        }

        if !button.label.is_empty() {
            let fg = if button.disabled {
                theme.muted_fg
            } else {
                theme.foreground
            };
            surface.surface_draw_text_run(rect, &button.label, fg);
        }

        layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Viewport};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every stroke call — enough to assert the focus ring
        /// is (or isn't) painted, without a Cairo/Core Graphics/Direct2D
        /// host.
        #[derive(Default)]
        struct RecordingSurface {
            strokes: Vec<(Rect, f32, Color, f32)>,
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
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, 0.0, color, stroke_width));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                radius: f32,
                color: Color,
                stroke_width: f32,
            ) {
                self.strokes.push((rect, radius, color, stroke_width));
            }
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _sw: f32) {}
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        const RECT: Rect = Rect::new(10.0, 10.0, 60.0, 20.0);

        #[test]
        fn focus_ring_geometry_outsets_by_the_shared_gap() {
            let bounds = Rect::new(10.0, 10.0, 60.0, 20.0);
            let style = Style::default()
                .with_corner_radius(4.0)
                .with_focus_ring_width(3.0);

            let ring = focus_ring_geometry(bounds, &style);

            assert_eq!(ring.rect, Rect::new(8.0, 8.0, 64.0, 24.0));
            assert_eq!(ring.corner_radius, 4.0);
            assert_eq!(ring.width, 3.0);
        }

        #[test]
        fn focus_ring_geometry_reads_corner_radius_from_style_not_a_separate_argument() {
            let bounds = Rect::new(10.0, 10.0, 60.0, 20.0);
            let style = Style::default().with_corner_radius(9.0);

            let ring = focus_ring_geometry(bounds, &style);

            // There's no independent radius argument to drift from
            // `style.corner_radius` — the helper always reads it from
            // `style` itself.
            assert_eq!(ring.corner_radius, 9.0);
        }

        #[test]
        fn focus_ring_geometry_reads_width_from_style_not_a_fixed_literal() {
            let bounds = Rect::new(0.0, 0.0, 10.0, 10.0);
            let narrow = focus_ring_geometry(bounds, &Style::default().with_focus_ring_width(1.0));
            let wide = focus_ring_geometry(bounds, &Style::default().with_focus_ring_width(8.0));

            assert_eq!(narrow.width, 1.0);
            assert_eq!(wide.width, 8.0);
            // Only the stroke width changes — the outset gap is shared,
            // not derived from `Style::focus_ring_width` itself.
            assert_eq!(narrow.rect, wide.rect);
        }

        #[test]
        fn focused_button_paints_a_ring_stroke_in_the_theme_ring_colour() {
            let mut button = Button::new(WidgetId::new("b"), "OK");
            button.focused = true;
            let theme = Theme::default();
            let style = Style::default().with_focus_ring_width(3.0);
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let ring = surface
                .strokes
                .iter()
                .find(|(_, _, _, sw)| *sw == 3.0)
                .expect("a focused button must paint a ring stroke at the style's width");
            assert_eq!(ring.2, theme.focus_ring_color());
            // The ring surrounds the button rather than tracing it exactly.
            assert!(ring.0.x < RECT.x && ring.0.y < RECT.y);
            assert!(ring.0.width > RECT.width && ring.0.height > RECT.height);
        }

        #[test]
        fn unfocused_button_paints_no_ring_stroke() {
            let button = Button::new(WidgetId::new("b"), "OK");
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            assert!(
                surface
                    .strokes
                    .iter()
                    .all(|(_, _, color, _)| *color != theme.focus_ring_color()),
                "an unfocused button must not paint the focus ring colour"
            );
        }

        #[test]
        fn disabled_focused_button_paints_no_ring_stroke() {
            let mut button = Button::new(WidgetId::new("b"), "OK");
            button.focused = true;
            button.disabled = true;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            assert!(
                surface
                    .strokes
                    .iter()
                    .all(|(_, _, color, _)| *color != theme.focus_ring_color()),
                "a disabled button must not paint a focus ring even while `focused`"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button() -> Button {
        Button::new(WidgetId::new("dialog:ok"), "OK")
    }

    #[test]
    fn new_defaults_to_primary_enabled_and_unfocused() {
        let b = button();
        assert_eq!(b.variant, ButtonVariant::Primary);
        assert!(!b.disabled);
        assert!(!b.focused);
        assert_eq!(b.label, "OK");
    }

    #[test]
    fn hit_test_inside_and_outside() {
        let b = button();
        let layout = b.layout(Rect::new(2.0, 2.0, 6.0, 1.0));
        assert_eq!(layout.hit_test(Point::new(3.0, 2.0)), ButtonHit::Body);
        assert_eq!(layout.hit_test(Point::new(0.0, 0.0)), ButtonHit::Outside);
    }

    #[test]
    fn handle_key_activates_on_enter_when_focused_and_enabled() {
        let mut b = button();
        b.focused = true;
        let event = b.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default());
        assert_eq!(event, Some(ButtonEvent::Activated { id: b.id.clone() }));
    }

    #[test]
    fn handle_key_activates_on_space() {
        let mut b = button();
        b.focused = true;
        let event = b.handle_key(&Key::Char(' '), &Modifiers::default());
        assert_eq!(event, Some(ButtonEvent::Activated { id: b.id.clone() }));
    }

    #[test]
    fn handle_key_ignores_unrelated_keys() {
        let mut b = button();
        b.focused = true;
        assert_eq!(
            b.handle_key(&Key::Named(NamedKey::Tab), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_not_focused() {
        let b = button();
        assert_eq!(
            b.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_disabled() {
        let mut b = button();
        b.focused = true;
        b.disabled = true;
        assert_eq!(
            b.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn click_activates_without_requiring_focus() {
        let b = button();
        assert_eq!(b.click(), Some(ButtonEvent::Activated { id: b.id.clone() }));
    }

    #[test]
    fn click_is_inert_when_disabled() {
        let mut b = button();
        b.disabled = true;
        assert_eq!(b.click(), None);
    }

    #[test]
    fn serde_round_trip() {
        let mut b = button();
        b.variant = ButtonVariant::Secondary;
        b.focused = true;
        let json = serde_json::to_string(&b).unwrap();
        let back: Button = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn pointer_shape_is_pointer_over_body_and_default_outside() {
        let b = button();
        let layout = b.layout(Rect::new(2.0, 2.0, 6.0, 1.0));
        assert_eq!(
            layout.pointer_shape(Point::new(3.0, 2.0)),
            PointerShape::Pointer
        );
        assert_eq!(
            layout.pointer_shape(Point::new(0.0, 0.0)),
            PointerShape::Default
        );
        // Just outside the bounds' right edge (bounds span x in [2, 8)):
        // the half-open `Rect::contains` boundary, not just a point far
        // away from the whole widget.
        assert_eq!(
            layout.pointer_shape(Point::new(8.0, 2.0)),
            PointerShape::Default
        );
    }

    #[test]
    fn pointer_shape_for_is_default_over_a_disabled_button_body() {
        let mut b = button();
        let layout = b.layout(Rect::new(2.0, 2.0, 6.0, 1.0));
        let inside = Point::new(3.0, 2.0);

        // Enabled: same as the bare `pointer_shape` — a hand over the body.
        assert_eq!(layout.pointer_shape_for(&b, inside), PointerShape::Pointer);

        // Disabled: the body isn't clickable, so no hand over it.
        b.disabled = true;
        assert_eq!(layout.pointer_shape_for(&b, inside), PointerShape::Default);
    }

    #[test]
    fn serde_defaults_variant_primary_and_flags_false_when_omitted() {
        let json = r#"{"id":"dialog:ok","label":"OK"}"#;
        let b: Button = serde_json::from_str(json).unwrap();
        assert_eq!(b.variant, ButtonVariant::Primary);
        assert!(!b.disabled);
        assert!(!b.focused);
    }
}
