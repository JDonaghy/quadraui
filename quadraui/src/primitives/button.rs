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
use crate::style::Style;
use crate::types::{Icon, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Visual emphasis variant. Purely descriptive — `Button` carries
/// no backend painting, so a rasteriser reads this field to choose its
/// own chrome (filled vs. outlined vs. icon-only).
///
/// `#[non_exhaustive]`: an additive variant here (as [`Self::Danger`]/
/// [`Self::Warning`] themselves are) would otherwise break a downstream
/// consumer's exhaustive `match`. Marking the enum `#[non_exhaustive]`
/// forecloses that break for the *next* variant — the same rule-8
/// preference order `docs/PRIMITIVE_RULES.md` states: a non-breaking
/// shape over a breaking one whenever one is available for free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ButtonVariant {
    /// The call-to-action for its surface (e.g. a dialog's "OK").
    #[default]
    Primary,
    /// A lower-emphasis action alongside a primary one (e.g. "Cancel").
    Secondary,
    /// Icon-only, no label chrome (e.g. a toolbar glyph button).
    Icon,
    /// A destructive action (e.g. "Delete"). Coloured through
    /// [`crate::Theme::button_danger_bg`] on a backend that fills a
    /// solid background, and through [`crate::Theme::error_fg`] as a
    /// plain foreground tint on TUI (see `crate::tui::button`'s own
    /// doc) — there is no dedicated `danger_fg` field, same "derive, don't
    /// add" rule [`crate::Theme::button_danger_bg`] itself follows.
    Danger,
    /// A cautionary action (e.g. "Overwrite"), one step down from
    /// [`Self::Danger`]. Coloured through
    /// [`crate::Theme::button_warning_bg`] /
    /// [`crate::Theme::warning_fg`] — see [`Self::Danger`]'s doc for the
    /// same shape.
    Warning,
}

/// Icon placement relative to [`Button::label`] when [`Button::icon`] is
/// set. No effect when `icon` is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ButtonIconPosition {
    /// Icon paints before the label (e.g. `[ ⚠ Delete ]`).
    #[default]
    Leading,
    /// Icon paints after the label (e.g. `[ Delete ⚠ ]`).
    Trailing,
}

/// `#[serde(default)]` value for [`Button::icon_gap`] — matches the gap
/// [`Button::new`] already sets, so a payload missing this field
/// deserializes to the same gap as `Button::default()`.
fn default_icon_gap() -> f32 {
    4.0
}

/// Fully-resolved, backend-neutral [`Button`] geometry: every backend's
/// rasteriser reads the same three numbers from here rather than each
/// hardcoding its own literal (or quietly drifting from the others).
///
/// **TUI story: cell-quantised / no-op**, same as the [`Style`] tokens it
/// is built from — see that struct's module doc. A terminal row is
/// always exactly one cell tall and has no sub-cell corner/padding to
/// refine, so `crate::tui::button` has no use for this struct.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ButtonMetrics {
    /// Resolved button height, in surface-native pixels — read straight
    /// from [`Style::control_height`], the same "standard single-line
    /// control" metric every other overlay sizes its own single-line
    /// chrome against.
    pub height: f32,
    /// Resolved horizontal inset, in surface-native pixels, between the
    /// button's own edge and its label/icon content — read straight
    /// from [`Style::padding`], the same inner-spacing token
    /// `Toast`/`Tooltip`/`ContextMenu`/`Palette`/`Dialog` already share.
    pub padding_x: f32,
    /// Resolved corner radius, in surface-native pixels — read straight
    /// from [`Style::corner_radius`], the same radius every other
    /// rounded-rect overlay rasterises its box with.
    pub corner_radius: f32,
}

impl ButtonMetrics {
    /// Resolve a [`Button`]'s geometry from `style`. Every number here
    /// is a direct read of an existing [`Style`] field — there is no
    /// button-specific token, so a host that retunes `Style` once gets
    /// a matching button for free on every backend.
    pub fn resolve(style: &Style) -> Self {
        Self {
            height: style.control_height,
            padding_x: style.padding,
            corner_radius: style.corner_radius,
        }
    }
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
    /// Hover tooltip text, shown via a host's own
    /// [`crate::compose::HoverTooltipTracker`] after
    /// [`crate::Style::tooltip_delay_ms`] of continuous pointer hover
    /// and hidden on pointer leave, a click, or a timeout. `None` (the
    /// default) means no tooltip — a host that wants one sets this
    /// field directly, the same way every other field here is set.
    /// Same "host decides when to show it, the primitive just carries
    /// the text" contract [`crate::primitives::activity_bar::ActivityItem::tooltip`]
    /// already documents.
    ///
    /// **TUI story: no-op.** [`Button`] has no hover UI on a terminal —
    /// `crate::tui::button`'s rasteriser never reads this field, the
    /// same as `ActivityItem::tooltip`'s own TUI story.
    #[serde(default)]
    pub tooltip: Option<String>,
    /// Optional icon painted alongside [`Button::label`], at
    /// [`Button::icon_position`]. `None` (the default) paints no icon —
    /// every existing construction site keeps compiling and painting
    /// exactly as before.
    ///
    /// **TUI story: glyph when Nerd Fonts are on, fallback otherwise.**
    /// `crate::tui::button` resolves [`Icon::glyph`] when the backend's
    /// `set_nerd_fonts(true)` is active and [`Icon::fallback`] otherwise
    /// — the same convention [`crate::ActivityBar`]'s own TUI rasteriser
    /// already follows.
    #[serde(default)]
    pub icon: Option<Icon>,
    /// Where [`Button::icon`] paints relative to [`Button::label`]. No
    /// effect while `icon` is `None`.
    #[serde(default)]
    pub icon_position: ButtonIconPosition,
    /// Gap, in surface-native pixels, between [`Button::icon`] and
    /// [`Button::label`]. No effect while `icon` is `None`.
    ///
    /// **TUI story: cell-quantised, floored to one cell.** A sub-cell
    /// gap has no grid column to paint into, and a gap of zero visible
    /// cells would read as a typo rather than an intentional tight
    /// pairing — `crate::tui::button` rounds this up to whole cells
    /// (`ceil(icon_gap / nominal_cell_px)`, the same nominal-cell-width
    /// fallback [`Style::padding`]'s own doc describes) and floors the
    /// result at one cell whenever an icon is actually painted.
    ///
    /// Default `4.0` — a tight but visually separated icon/label pairing.
    #[serde(default = "default_icon_gap")]
    pub icon_gap: f32,
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
            tooltip: None,
            icon: None,
            icon_position: ButtonIconPosition::Leading,
            icon_gap: default_icon_gap(),
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

/// Shared, kind-agnostic button description — the fields [`Button`]
/// itself carries beyond `id`/`focused` (which only make sense on a
/// standalone control), lifted out so [`crate::ToolbarButton::Action`],
/// [`crate::ButtonRowItem`], [`crate::DialogButton`] and
/// [`crate::ToastButton`] can each carry one through an optional `spec`
/// field without pulling in `Button`'s own id/focus-tracking concerns.
///
/// **Deprecate-first.** Every one of those four kinds keeps its own
/// legacy fields (icon string, tint, is_default/is_cancel, primary, …)
/// working exactly as before; `spec` is an *additional*, optional
/// override. A host that never sets it gets the same rendering those
/// kinds have always had. A host that does set it gets the full B3a
/// vocabulary — [`ButtonVariant::Danger`]/[`ButtonVariant::Warning`],
/// icon placement, and a tooltip — on whichever backend reads it.
///
/// `#[non_exhaustive]` for the same reason [`Button`] itself is: an
/// additive field here shouldn't force a downstream consumer's
/// exhaustive struct literal to grow in lockstep. Construct with
/// [`Self::new`] and set fields directly afterwards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ButtonSpec {
    pub label: String,
    #[serde(default)]
    pub variant: ButtonVariant,
    #[serde(default)]
    pub icon: Option<Icon>,
    #[serde(default)]
    pub icon_position: ButtonIconPosition,
    #[serde(default = "default_icon_gap")]
    pub icon_gap: f32,
    #[serde(default)]
    pub tooltip: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

impl ButtonSpec {
    /// A primary, enabled spec with `label` and every other field at
    /// [`Button::new`]'s own defaults — so a legacy-kind's derived spec
    /// and a host-authored one start from the identical baseline.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            variant: ButtonVariant::Primary,
            icon: None,
            icon_position: ButtonIconPosition::Leading,
            icon_gap: default_icon_gap(),
            tooltip: None,
            disabled: false,
        }
    }

    /// Materialise a paintable [`Button`] from this spec, for a host
    /// rasteriser that wants to paint a non-`Button` primitive's button
    /// (a [`crate::ToolbarButton::Action`], [`crate::ButtonRowItem`],
    /// [`crate::DialogButton`] or [`crate::ToastButton`] carrying a
    /// `spec` override) through the same bracket-chrome / icon / intent-
    /// colour path `crate::tui::button` already paints a standalone
    /// `Button` with, rather than duplicating that logic per kind.
    /// `id`/`focused` come from the host — a `ButtonSpec` carries
    /// neither, since those only make sense once attached to a specific
    /// widget instance.
    pub fn to_button(&self, id: WidgetId, focused: bool) -> Button {
        Button {
            id,
            label: self.label.clone(),
            variant: self.variant,
            disabled: self.disabled,
            focused,
            tooltip: self.tooltip.clone(),
            icon: self.icon.clone(),
            icon_position: self.icon_position,
            icon_gap: self.icon_gap,
        }
    }

    /// Minimum cell-unit width that paints this spec's resolved
    /// [`Button`] without clipping — the `[ ` / ` ]` bracket chrome
    /// (4 cells) plus [`Self::label`]'s own display width, plus a
    /// 1-cell icon glyph and its [`Self::icon_gap`]-derived gap
    /// (`ceil(icon_gap / NOMINAL_CELL_PX_WIDTH)`, floored at one cell)
    /// when [`Self::icon`] is set. [`ButtonVariant::Icon`] paints no
    /// bracket chrome, so its width is just the icon block.
    ///
    /// `crate::tui::button::draw_button` paints exactly this content —
    /// see that module's own doc for the chrome layout this mirrors. A
    /// caller that must size a button box *before* handing it to that
    /// rasteriser (e.g. [`crate::primitives::dialog::Dialog::measure_generic_inner`],
    /// `crate::tui::dialog::tui_dialog_layout`,
    /// `crate::compose::message_dialog::MessageDialogController`'s own
    /// layout) uses this instead of a fixed per-label estimate, so a
    /// `Danger`/`Warning` spec's icon and longer label never clip the
    /// closing bracket.
    pub fn min_cell_width(&self) -> f32 {
        use crate::primitives::layout_metrics::pixel::NOMINAL_CELL_PX_WIDTH;

        let label_w = crate::text_util::display_width(&self.label);
        let icon_block = if self.icon.is_some() {
            let gap_cells = if !self.icon_gap.is_finite() {
                1
            } else {
                ((self.icon_gap / NOMINAL_CELL_PX_WIDTH).ceil() as usize).max(1)
            };
            1 + gap_cells
        } else {
            0
        };
        if matches!(self.variant, ButtonVariant::Icon) {
            icon_block.max(1) as f32
        } else {
            (4 + icon_block + label_w) as f32
        }
    }
}

impl Default for ButtonSpec {
    /// A primary, enabled spec with an empty label. See [`Self::new`]
    /// for a spec with a real label.
    fn default() -> Self {
        Self::new("")
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
    use super::{Button, ButtonIconPosition, ButtonLayout, ButtonMetrics, ButtonVariant};
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

    /// Clamp [`Button::icon_gap`] (a surface-native pixel count) into a
    /// sane paint-time range, the GUI-pixel twin of `tui::button`'s own
    /// `icon_gap_cells` clamp. A non-finite gap (a stray `f32::NAN`/
    /// `f32::MAX`) would otherwise propagate into `content_w`/`x`
    /// unclamped: `f32::max` resolves `(rect.width - content_w) / 2.0`
    /// against a NaN `content_w` back to a finite value (`f32::max`
    /// returns whichever operand isn't NaN), but the second content run's
    /// `x += w + gap` has no such rescue and goes NaN outright, which a
    /// backend's own text-draw call (e.g. Cairo's `move_to`) then quietly
    /// no-ops for the rest of that frame. Clamped at `max_width` (the
    /// button's own paint width) for the same reason `icon_gap_cells`
    /// clamps at `max_cells`: an unvalidated huge gap can use at most as
    /// much room as the button actually has.
    fn sanitize_icon_gap(icon_gap: f32, max_width: f32) -> f32 {
        let max_width = max_width.max(0.0);
        if !icon_gap.is_finite() {
            return max_width;
        }
        icon_gap.clamp(0.0, max_width)
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

        let metrics = ButtonMetrics::resolve(style);

        let bg = if button.disabled {
            theme.surface_bg
        } else {
            match button.variant {
                ButtonVariant::Primary => theme.accent_bg,
                ButtonVariant::Secondary | ButtonVariant::Icon => theme.surface_bg,
                ButtonVariant::Danger => theme.button_danger_bg(),
                ButtonVariant::Warning => theme.button_warning_bg(),
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
        surface.surface_fill_rounded_rect(rect, metrics.corner_radius, bg);

        if !matches!(button.variant, ButtonVariant::Icon) {
            let border = if button.disabled {
                theme.muted_fg
            } else if button.focused {
                theme.accent_fg
            } else {
                theme.border_fg
            };
            surface.surface_stroke_rounded_rect(rect, metrics.corner_radius, border, 1.0);
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

        if !button.label.is_empty() || button.icon.is_some() {
            let fg = if button.disabled {
                theme.muted_fg
            } else {
                theme.foreground
            };

            // The icon glyph, measured against the surface's current
            // font, alongside its own paint width/height. GUI backends
            // always carry the bundled codicon font (see
            // `MacBackend`/`GtkBackend`/`WinBackend`'s own
            // `nerd_fonts_enabled` field doc), so `Icon::glyph` always
            // renders here — unlike TUI's `Icon::fallback` escape hatch
            // for a terminal with no such font installed.
            let icon_metrics = button.icon.as_ref().map(|icon| {
                let (w, h) = surface.surface_measure_text(&icon.glyph);
                (icon, w, h)
            });
            let (label_w, label_h) = if button.label.is_empty() {
                (0.0, 0.0)
            } else {
                surface.surface_measure_text(&button.label)
            };
            let icon_w = icon_metrics.as_ref().map_or(0.0, |(_, w, _)| *w);
            let icon_h = icon_metrics.as_ref().map_or(0.0, |(_, _, h)| *h);
            let gap = if icon_metrics.is_some() && !button.label.is_empty() {
                sanitize_icon_gap(button.icon_gap, rect.width)
            } else {
                0.0
            };
            let content_w = icon_w + gap + label_w;
            let content_h = icon_h.max(label_h);

            // Centre the icon+label block horizontally, but never closer
            // to `rect`'s own edge than `metrics.padding_x` —
            // `Style::padding`'s own resolved value, the same
            // inner-spacing token `Toast`/`Tooltip`/`ContextMenu`/
            // `Palette`/`Dialog` already read off `Style::padding`.
            let centered_x = rect.x + ((rect.width - content_w) / 2.0).max(metrics.padding_x);
            let text_y = rect.y + ((rect.height - content_h) / 2.0).max(0.0);
            let mut x = centered_x;

            // A disabled button dims its label to `theme.muted_fg` above;
            // an icon with its own explicit `Icon::color` must dim the
            // same way rather than keep painting at full saturation, or a
            // disabled control reads as enabled through its icon alone.
            let icon_color = if button.disabled {
                fg
            } else {
                icon_metrics
                    .as_ref()
                    .and_then(|(icon, _, _)| icon.color)
                    .unwrap_or(fg)
            };

            match (icon_metrics, button.icon_position) {
                (Some((icon, w, _)), ButtonIconPosition::Leading) => {
                    surface.surface_draw_text_run(
                        Rect::new(x, text_y, w, content_h),
                        &icon.glyph,
                        icon_color,
                    );
                    x += w + gap;
                    if !button.label.is_empty() {
                        surface.surface_draw_text_run(
                            Rect::new(x, text_y, label_w, content_h),
                            &button.label,
                            fg,
                        );
                    }
                }
                (Some((icon, w, _)), ButtonIconPosition::Trailing) => {
                    if !button.label.is_empty() {
                        surface.surface_draw_text_run(
                            Rect::new(x, text_y, label_w, content_h),
                            &button.label,
                            fg,
                        );
                        x += label_w + gap;
                    }
                    surface.surface_draw_text_run(
                        Rect::new(x, text_y, w, content_h),
                        &icon.glyph,
                        icon_color,
                    );
                }
                (None, _) => {
                    surface.surface_draw_text_run(
                        Rect::new(x, text_y, label_w, content_h),
                        &button.label,
                        fg,
                    );
                }
            }
        }

        layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Viewport};
        use crate::types::{Color, Icon, WidgetId};
        use crate::Image;

        /// Records every fill/stroke call — enough to assert the resting
        /// fill colour and the focus ring are (or aren't) painted,
        /// without a Cairo/Core Graphics/Direct2D host.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, f32, Color)>,
            strokes: Vec<(Rect, f32, Color, f32)>,
            /// Every `surface_draw_text_run` call, in paint order —
            /// `(rect, text, color)`.
            text_runs: Vec<(Rect, String, Color)>,
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
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
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

        #[test]
        fn danger_variant_fills_with_the_theme_danger_colour() {
            let mut button = Button::new(WidgetId::new("b"), "Delete");
            button.variant = ButtonVariant::Danger;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (_, _, fill) = surface.fills.first().expect("button must paint a fill");
            assert_eq!(*fill, theme.button_danger_bg());
            assert_ne!(
                *fill, theme.accent_bg,
                "a Danger button must not fall back to the Primary fill"
            );
        }

        #[test]
        fn warning_variant_fills_with_the_theme_warning_colour() {
            let mut button = Button::new(WidgetId::new("b"), "Overwrite");
            button.variant = ButtonVariant::Warning;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (_, _, fill) = surface.fills.first().expect("button must paint a fill");
            assert_eq!(*fill, theme.button_warning_bg());
        }

        /// An enabled button paints an `Icon::color`-carrying icon at
        /// that colour, not the plain foreground.
        #[test]
        fn enabled_button_paints_icon_at_its_own_color() {
            let mut button = Button::new(WidgetId::new("b"), "Save");
            let icon_color = Color::rgb(255, 0, 0);
            button.icon = Some(Icon::new("S", "S").with_color(icon_color));
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (_, _, color) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "S")
                .expect("icon glyph must paint a text run");
            assert_eq!(*color, icon_color);
        }

        /// A disabled button dims its label to `theme.muted_fg` — an
        /// icon carrying its own explicit `Icon::color` must dim the
        /// same way rather than keep painting at full saturation, or a
        /// disabled control reads as enabled through its icon alone.
        #[test]
        fn disabled_button_dims_a_coloured_icon_to_muted_fg() {
            let mut button = Button::new(WidgetId::new("b"), "Save");
            let icon_color = Color::rgb(255, 0, 0);
            button.icon = Some(Icon::new("S", "S").with_color(icon_color));
            button.disabled = true;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (_, _, color) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "S")
                .expect("icon glyph must paint a text run");
            assert_eq!(
                *color, theme.muted_fg,
                "a disabled icon must dim to theme.muted_fg like the label, not keep its own color"
            );
            assert_ne!(
                *color, icon_color,
                "a disabled icon must not keep painting at its own explicit color"
            );
        }

        /// A non-finite `icon_gap` (e.g. `f32::NAN`) must not propagate
        /// into the label's paint `x` — `sanitize_icon_gap` clamps it to
        /// a finite value before it reaches the centring arithmetic.
        #[test]
        fn non_finite_icon_gap_does_not_produce_a_nan_label_x() {
            let mut button = Button::new(WidgetId::new("b"), "Save");
            button.icon = Some(Icon::new("S", "S"));
            button.icon_gap = f32::NAN;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (_, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "S")
                .expect("icon glyph must paint a text run");
            let (label_rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "Save")
                .expect("label must paint a text run");
            assert!(
                label_rect.x.is_finite(),
                "a non-finite icon_gap must not make the label's paint x NaN, got {}",
                label_rect.x,
            );
        }

        /// A huge, finite `icon_gap` (e.g. `f32::MAX`) must stay finite
        /// through the centring arithmetic — `sanitize_icon_gap` clamps
        /// it at `rect.width` rather than letting it pass through
        /// unbounded, the same "clamp the unvalidated literal" guard
        /// `tui::button::icon_gap_cells` already has for its own
        /// cell-space twin.
        #[test]
        fn huge_icon_gap_is_clamped_instead_of_producing_a_nonfinite_label_x() {
            let mut button = Button::new(WidgetId::new("b"), "Save");
            button.icon = Some(Icon::new("S", "S"));
            button.icon_gap = f32::MAX;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (label_rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "Save")
                .expect("label must paint a text run");
            assert!(
                label_rect.x.is_finite(),
                "a huge icon_gap must not make the label's paint x non-finite, got {}",
                label_rect.x,
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
        b.tooltip = Some("Save the current file".to_string());
        let json = serde_json::to_string(&b).unwrap();
        let back: Button = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn new_has_no_tooltip_by_default() {
        assert_eq!(button().tooltip, None);
    }

    /// A JSON payload serialized before [`Button::tooltip`] existed has
    /// no such key; `#[serde(default)]` must deserialize it as `None`
    /// rather than erroring, so a consumer's persisted descriptor from
    /// an earlier `quadraui` version keeps loading.
    #[test]
    fn deserializing_without_tooltip_defaults_to_none() {
        let json = r#"{"id": "dialog:ok", "label": "OK"}"#;
        let b: Button = serde_json::from_str(json).expect("deserialize without tooltip");
        assert_eq!(b.tooltip, None);
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

    #[test]
    fn new_has_no_icon_and_default_leading_gap_of_four() {
        let b = button();
        assert_eq!(b.icon, None);
        assert_eq!(b.icon_position, ButtonIconPosition::Leading);
        assert_eq!(b.icon_gap, 4.0);
    }

    /// A JSON payload serialized before `icon`/`icon_position`/`icon_gap`
    /// existed has none of those keys; `#[serde(default)]` must deserialize
    /// every one of them at the same defaults `Button::new` sets, so a
    /// consumer's persisted descriptor from an earlier `quadraui` version
    /// keeps loading.
    #[test]
    fn deserializing_without_icon_fields_defaults_to_new() {
        let json = r#"{"id": "dialog:ok", "label": "OK"}"#;
        let b: Button = serde_json::from_str(json).expect("deserialize without icon fields");
        assert_eq!(b.icon, None);
        assert_eq!(b.icon_position, ButtonIconPosition::Leading);
        assert_eq!(b.icon_gap, 4.0);
    }

    #[test]
    fn icon_round_trips_through_serde() {
        let mut b = button();
        b.icon = Some(crate::types::Icon::new("\u{f071}", "!"));
        b.icon_position = ButtonIconPosition::Trailing;
        b.icon_gap = 6.0;
        let json = serde_json::to_string(&b).unwrap();
        let back: Button = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn danger_and_warning_variants_round_trip_through_serde() {
        for variant in [ButtonVariant::Danger, ButtonVariant::Warning] {
            let mut b = button();
            b.variant = variant;
            let json = serde_json::to_string(&b).unwrap();
            let back: Button = serde_json::from_str(&json).unwrap();
            assert_eq!(b, back);
        }
    }

    #[test]
    fn resolved_metrics_reads_height_padding_and_radius_straight_from_style() {
        let style = Style::default()
            .with_control_height(32.0)
            .with_padding(10.0)
            .with_corner_radius(9.0);

        let metrics = ButtonMetrics::resolve(&style);

        assert_eq!(metrics.height, 32.0);
        assert_eq!(metrics.padding_x, 10.0);
        assert_eq!(metrics.corner_radius, 9.0);
    }

    #[test]
    fn resolved_metrics_matches_style_default() {
        let style = Style::default();
        let metrics = ButtonMetrics::resolve(&style);
        assert_eq!(metrics.height, style.control_height);
        assert_eq!(metrics.padding_x, style.padding);
        assert_eq!(metrics.corner_radius, style.corner_radius);
    }

    #[test]
    fn button_spec_new_matches_button_new_defaults() {
        let spec = ButtonSpec::new("OK");
        assert_eq!(spec.label, "OK");
        assert_eq!(spec.variant, ButtonVariant::Primary);
        assert_eq!(spec.icon, None);
        assert_eq!(spec.icon_position, ButtonIconPosition::Leading);
        assert_eq!(spec.icon_gap, 4.0);
        assert_eq!(spec.tooltip, None);
        assert!(!spec.disabled);
    }

    #[test]
    fn button_spec_default_is_an_empty_label_primary_spec() {
        assert_eq!(ButtonSpec::default(), ButtonSpec::new(""));
    }

    #[test]
    fn to_button_carries_every_field_and_the_given_id_and_focus() {
        let mut spec = ButtonSpec::new("Delete");
        spec.variant = ButtonVariant::Danger;
        spec.icon = Some(crate::types::Icon::new("\u{f071}", "!"));
        spec.icon_position = ButtonIconPosition::Trailing;
        spec.icon_gap = 6.0;
        spec.tooltip = Some("Permanently delete".to_string());
        spec.disabled = true;

        let id = WidgetId::new("dialog:delete");
        let button = spec.to_button(id.clone(), true);

        assert_eq!(button.id, id);
        assert_eq!(button.label, "Delete");
        assert_eq!(button.variant, ButtonVariant::Danger);
        assert_eq!(button.icon, spec.icon);
        assert_eq!(button.icon_position, ButtonIconPosition::Trailing);
        assert_eq!(button.icon_gap, 6.0);
        assert_eq!(button.tooltip, spec.tooltip);
        assert!(button.disabled);
        assert!(button.focused);
    }

    #[test]
    fn min_cell_width_is_label_plus_four_cells_of_chrome() {
        let spec = ButtonSpec::new("Move to Trash");
        // "[ " + "Move to Trash" (13) + " ]" = 4 + 13.
        assert_eq!(spec.min_cell_width(), 17.0);
    }

    #[test]
    fn min_cell_width_adds_a_one_cell_icon_plus_its_gap() {
        let mut spec = ButtonSpec::new("Move to Trash");
        spec.icon = Some(crate::types::Icon::new("\u{f071}", "!"));
        // A 1-cell icon glyph plus a ≥1-cell gap, on top of the plain
        // label's own `min_cell_width`.
        let plain = ButtonSpec::new("Move to Trash").min_cell_width();
        assert!(spec.min_cell_width() > plain);
    }

    #[test]
    fn min_cell_width_floors_the_icon_gap_at_one_cell() {
        let mut narrow_gap = ButtonSpec::new("Go");
        narrow_gap.icon = Some(crate::types::Icon::new("\u{f061}", ">"));
        narrow_gap.icon_gap = 0.0;
        let mut wide_gap = ButtonSpec::new("Go");
        wide_gap.icon = Some(crate::types::Icon::new("\u{f061}", ">"));
        wide_gap.icon_gap = 64.0;

        // 4 chrome + 1 icon + 1 floored gap + 2 label chars.
        assert_eq!(narrow_gap.min_cell_width(), 8.0);
        assert!(wide_gap.min_cell_width() > narrow_gap.min_cell_width());
    }

    #[test]
    fn button_spec_serde_round_trip() {
        let mut spec = ButtonSpec::new("Overwrite");
        spec.variant = ButtonVariant::Warning;
        spec.icon = Some(crate::types::Icon::new("W", "W"));
        let json = serde_json::to_string(&spec).unwrap();
        let back: ButtonSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    /// A JSON payload serialized before every optional `ButtonSpec` field
    /// existed has only `label`; `#[serde(default)]` must deserialize the
    /// rest at [`ButtonSpec::new`]'s own defaults.
    #[test]
    fn button_spec_deserializes_with_defaults_when_only_label_is_present() {
        let json = r#"{"label": "OK"}"#;
        let spec: ButtonSpec = serde_json::from_str(json).expect("deserialize label-only spec");
        assert_eq!(spec, ButtonSpec::new("OK"));
    }
}
