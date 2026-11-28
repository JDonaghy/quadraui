//! `Button` primitive: a standalone, clickable action control — primary,
//! secondary, and icon variants, with keyboard focus/activation, plus
//! its disabled state and event. Layout/hit-test/keyboard logic, and
//! every rasteriser (the `cell_paint` cell-grid painter and the
//! `native_surface_paint` pixel painter), live here, reached through
//! [`crate::primitive::Primitive::paint_cells`] /
//! [`crate::primitive::Primitive::paint`] rather than a per-backend
//! `Backend` method.
//!
//! Today `Form`'s `Button`/`ButtonRow` field kinds cover the same shape
//! embedded inside a form. `Button` is additive: a standalone control
//! for a host that wants a button outside a form (a toolbar action, a
//! dialog's own footer, a palette entry) without pulling in the whole
//! `Form` machinery. `Form`'s own field kinds are
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
//! [`crate::focus::FocusTree`] for the app-wide owner of "what has
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// from [`Style::button_radius`], the button-specific radius token
    /// (narrower than [`Style::corner_radius`], which every other
    /// rounded-rect overlay rasterises its own box with).
    pub corner_radius: f32,
}

impl ButtonMetrics {
    /// Resolve a [`Button`]'s geometry from `style`. Every number here
    /// is a direct read of an existing [`Style`] field — `corner_radius`
    /// from [`Style::button_radius`] specifically, not
    /// [`Style::corner_radius`] — so a host that retunes `Style` once
    /// gets a matching button for free on every backend.
    pub fn resolve(style: &Style) -> Self {
        Self {
            height: style.control_height,
            padding_x: style.padding,
            corner_radius: style.button_radius,
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// **TUI story: shown by a host's own hover driving, not by
    /// `crate::tui::button`'s rasteriser directly.** `crate::tui::button`
    /// itself never reads this field — the same as `ActivityItem::tooltip`'s
    /// own TUI story — but a host built on `quadraui-binding-core`'s
    /// `BindingHost::drive_hover_tooltip` paints a real hovered tooltip on
    /// the TUI backend from it, for a `Button` reached either through
    /// `BindingHost::button` or through a layout-tree leaf painted by
    /// `paint_leaf`/`paint_layout`.
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
    /// `variant`/`disabled`/`focused` directly afterwards (every field is `pub`) rather than via a
    /// `with_*` chain — see `docs/PRIMITIVE_RULES.md`'s rule-8 history
    /// of why this crate stopped adding those.
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
        ButtonLayout {
            bounds,
            nerd_fonts_enabled: false,
        }
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

    /// Derive a [`ButtonSpec`] from the plain `label`/`icon`/`tooltip`/
    /// `disabled` fields [`crate::ToolbarButton::Action`],
    /// [`crate::DialogButton`], [`crate::ToastButton`] and
    /// [`crate::ButtonRowItem`] each carry alongside their optional
    /// `spec` override — the single derivation those four kinds' own
    /// `resolved_button_spec` share. `variant` starts at [`Self::new`]'s
    /// own `ButtonVariant::Primary` default; a caller that derives a
    /// variant from its own legacy field (e.g. `DialogButton::is_default`,
    /// `ToastButton::primary`) sets `.variant` on the result afterwards.
    pub fn from_legacy(
        label: impl Into<String>,
        icon: Option<Icon>,
        tooltip: Option<String>,
        disabled: bool,
    ) -> Self {
        let mut spec = Self::new(label);
        spec.icon = icon;
        spec.tooltip = tooltip;
        spec.disabled = disabled;
        spec
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
    /// `crate::compose::message_dialog::MessageDialog`'s own
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
    /// Whether the backend has a Nerd Font icon face registered — see
    /// [`crate::primitive::LayoutCtx::nerd_fonts_enabled`]. Baked in at
    /// layout time (by [`crate::primitive::Primitive::layout`]) because
    /// [`crate::primitive::Primitive::paint_cells`] has no `LayoutCtx`
    /// of its own to read it from — the same seam
    /// [`crate::primitives::tab_bar::TabBarPrimitiveLayout::nerd_fonts_enabled`]/
    /// [`crate::primitives::activity_bar::ActivityBarPrimitiveLayout::nerd_fonts`]
    /// carry it through for the same reason. [`Button::layout`], which
    /// has no `LayoutCtx` to consult, leaves it `false`.
    pub nerd_fonts_enabled: bool,
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

impl crate::interactive::WidgetAction for ButtonEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            ButtonEvent::Activated { .. } => crate::UiEvent::ButtonActivated { widget: id },
        }
    }
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────
//
// `Button` reaches the frame type-erased as [`crate::frame::Surface::Widget`]:
// `layout` resolves the button's own bounds plus the backend's
// `nerd_fonts_enabled` flag (baked in since [`Primitive::paint_cells`]
// has no `LayoutCtx` of its own — see [`ButtonLayout::nerd_fonts_enabled`]'s
// doc) — [`pixel_button_layout`] on a pixel surface,
// [`cell_button_layout`] on a cell grid — `paint` runs the shared pixel
// painter in [`native_surface_paint`], `paint_cells` runs the cell
// painter in [`cell_paint`], and `handle` folds in
// [`Button::handle_key`]/[`Button::click`].

/// Resolve `button`'s layout for a pixel surface — bounds plus
/// `nerd_fonts_enabled` from `ctx`.
pub(crate) fn pixel_button_layout(
    button: &Button,
    ctx: &crate::primitive::LayoutCtx,
    rect: Rect,
) -> ButtonLayout {
    let mut layout = button.layout(rect);
    layout.nerd_fonts_enabled = ctx.nerd_fonts_enabled;
    layout
}

/// Resolve `button`'s layout for a cell grid — see [`pixel_button_layout`].
///
/// Quantises `rect` to whole cells first, the same rounding a cell-grid
/// backend's own `Rect` -> native-grid conversion already applies before
/// it ever paints a cell — see [`cell_paint::paint`]'s own `x0`/`w`
/// rounding. Without this, [`ButtonLayout::hit_test`] would compare a
/// pointer position against the raw, possibly-fractional `rect` a
/// container handed this leaf (e.g. a ratio-split
/// [`crate::SplitTree`] pane), one cell wider than what actually painted
/// — mis-claiming the adjacent divider's own cell as this button's body.
pub(crate) fn cell_button_layout(
    button: &Button,
    ctx: &crate::primitive::LayoutCtx,
    rect: Rect,
) -> ButtonLayout {
    let mut layout = button.layout(crate::primitive::cell_grid_rect(rect));
    layout.nerd_fonts_enabled = ctx.nerd_fonts_enabled;
    layout
}

impl crate::primitive::Primitive for Button {
    type Layout = ButtonLayout;
    type Event = ButtonHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Button(self.clone())
    }

    /// `[ Label ]` bracket chrome: [`Self::min_cell_width`]-shaped
    /// chrome plus [`Self::label`]'s own display width on a cell grid,
    /// [`ButtonMetrics::padding_x`] on both sides plus the measured
    /// label width on a pixel surface — a label-driven intrinsic width
    /// no fixed-size container can supply on its own, same reasoning
    /// [`crate::primitives::toolbar::measure_button`] already applies
    /// to a `ToolbarButton::Action`.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let spec = ButtonSpec {
            label: self.label.clone(),
            variant: self.variant,
            icon: self.icon.clone(),
            icon_position: self.icon_position,
            icon_gap: self.icon_gap,
            tooltip: None,
            disabled: self.disabled,
        };
        match ctx.units() {
            crate::primitive::Units::Cells => {
                crate::NaturalSize::new(Some(spec.min_cell_width()), Some(1.0))
            }
            _ => {
                let metrics = ButtonMetrics::resolve(ctx.style());
                let label_w = ctx.text_width(&self.label, self.font_role());
                let icon_w = if self.icon.is_some() {
                    metrics.height + self.icon_gap
                } else {
                    0.0
                };
                let width = if matches!(self.variant, ButtonVariant::Icon) {
                    icon_w.max(metrics.height)
                } else {
                    metrics.padding_x * 2.0 + label_w + icon_w
                };
                crate::NaturalSize::new(Some(width), Some(metrics.height))
            }
        }
    }

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> ButtonLayout {
        match ctx.units() {
            crate::primitive::Units::Cells => cell_button_layout(self, ctx, rect),
            _ => pixel_button_layout(self, ctx, rect),
        }
    }

    /// The shared pixel painter, against the already-resolved `layout`'s
    /// bounds, tinted by whether `env` reports this button hovered or
    /// pressed.
    fn paint(
        &self,
        layout: &ButtonLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let style = env.style();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let _ = native_surface_paint::paint(
                self,
                surface,
                theme,
                layout.bounds,
                style,
                env.is_hovered(&self.id),
                env.is_pressed(&self.id),
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme, style);
    }

    /// [`cell_paint::paint`] over `buf`, adapted through
    /// [`crate::tui::cell_surface::TuiCellSurface`] — the `Buffer` ->
    /// [`crate::CellSurface`] bridge every `tui` cell-grid painter builds
    /// for itself, `pub(crate)` rather than private to `crate::tui` for
    /// exactly this reach. Hover and press come from `env`, the same as
    /// [`Primitive::paint`] above.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ButtonLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        cell_paint::paint(
            self,
            layout,
            cells,
            theme,
            env.is_hovered(&self.id),
            env.is_pressed(&self.id),
        );
    }
}

impl crate::interactive::Interactive for Button {
    type State = ();
    type Action = ButtonEvent;

    /// One focus stop per button — the same self-contained convention
    /// `Button::focused`'s own doc describes.
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
    /// [`Button::handle_key`], whose own `self.focused` gate is the
    /// legacy, non-contract convention a host sets by hand before
    /// calling it directly; the engine only ever routes a `Key` event to
    /// whichever widget its own `FocusTree` already names as focused
    /// (see `crate::runtime::route_widget_key`), so re-checking
    /// `self.focused` here would wrongly ignore the key on a descriptor
    /// a host rebuilds fresh every frame without separately mirroring
    /// that flag. A left click activates regardless of focus (see
    /// [`Button::click`]), requesting focus first so a pointer
    /// activation leaves the button focused the same way a keyboard
    /// activation requires it to already be.
    fn handle(
        &self,
        _state: &mut (),
        event: &crate::interactive::RoutedEvent,
        _layout: &ButtonLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<ButtonEvent> {
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
                kind: PointerKind::Down(crate::event::MouseButton::Left),
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

/// The cell-grid painter for [`Button`], written against
/// [`crate::CellSurface`] — moved from `crate::tui::button`, which is
/// now a thin `Buffer` -> [`crate::CellSurface`] adapter.
///
/// Paints `[ Label ]` bracket chrome for [`ButtonVariant::Primary`] /
/// [`ButtonVariant::Secondary`] / [`ButtonVariant::Danger`] /
/// [`ButtonVariant::Warning`], and the bare label for
/// [`ButtonVariant::Icon`]. [`ButtonLayout::nerd_fonts_enabled`] (baked
/// in at layout time) picks [`crate::Icon::glyph`] vs.
/// [`crate::Icon::fallback`] for [`Button::icon`].
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{Button, ButtonIconPosition, ButtonLayout, ButtonVariant};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::primitives::layout_metrics::pixel::NOMINAL_CELL_PX_WIDTH;
    use crate::theme::Theme;
    use crate::types::Color;

    fn solid(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Quantise [`Button::icon_gap`] to whole TUI cells — see
    /// `crate::tui::button`'s historical doc for why a zero-cell gap
    /// isn't honoured. Clamped at `max_cells` so an unvalidated,
    /// arbitrarily large `icon_gap` can neither overflow the `as usize`
    /// cast nor allocate past what the paint loop could ever use.
    fn icon_gap_cells(icon_gap: f32, max_cells: usize) -> usize {
        if !icon_gap.is_finite() {
            return max_cells.max(1);
        }
        ((icon_gap / NOMINAL_CELL_PX_WIDTH).ceil() as usize).clamp(1, max_cells.max(1))
    }

    /// Paint `button` into `layout.bounds` on `surface`, tinting the
    /// background via `Theme::button_pressed_bg`/`Theme::button_hover_bg`
    /// when `is_hovered`/`is_pressed` is set — pressed takes priority
    /// over hover, same as the GTK/macOS/Win `native_surface_paint`
    /// path.
    ///
    /// `layout` must be the value [`super::cell_button_layout`] returned
    /// for the same button and rect, so paint and
    /// [`ButtonLayout::hit_test`] can never disagree.
    pub(crate) fn paint(
        button: &Button,
        layout: &ButtonLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        is_hovered: bool,
        is_pressed: bool,
    ) {
        let bounds = layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let x0 = bounds.x.round() as u16;
        let w = bounds.width.round() as u16;
        let end = x0.saturating_add(w);
        let area_height = bounds.height.round() as u16;
        // Centre the label row so a focus ring's top/bottom border
        // (painted on the cell's edge rows) never overwrites it.
        let y = bounds.y.round() as u16 + area_height.saturating_sub(1) / 2;

        let intent_fg = match button.variant {
            ButtonVariant::Danger => Some(theme.error_fg),
            ButtonVariant::Warning => Some(theme.warning_fg),
            ButtonVariant::Primary | ButtonVariant::Secondary | ButtonVariant::Icon => None,
        };

        let bg = if button.disabled {
            theme.surface_bg
        } else {
            match button.variant {
                ButtonVariant::Primary => theme.accent_bg,
                ButtonVariant::Secondary | ButtonVariant::Danger | ButtonVariant::Warning => {
                    theme.surface_bg
                }
                ButtonVariant::Icon => theme.background,
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
        let fg = if button.disabled {
            theme.muted_fg
        } else if let Some(intent) = intent_fg {
            intent
        } else {
            theme.foreground
        };
        let brk_fg = if button.disabled {
            theme.muted_fg
        } else if button.focused {
            theme.accent_fg
        } else if let Some(intent) = intent_fg {
            intent
        } else {
            theme.border_fg
        };

        let is_icon = matches!(button.variant, ButtonVariant::Icon);
        let icon_ch: Option<char> = button.icon.as_ref().map(|icon| {
            icon.resolve(layout.nerd_fonts_enabled)
                .chars()
                .next()
                .unwrap_or(' ')
        });
        let inner = match icon_ch {
            None => button.label.clone(),
            Some(ch) => {
                let gap = " ".repeat(icon_gap_cells(button.icon_gap, w as usize));
                match button.icon_position {
                    ButtonIconPosition::Leading => format!("{ch}{gap}{}", button.label),
                    ButtonIconPosition::Trailing => format!("{}{gap}{ch}", button.label),
                }
            }
        };
        let text: String = if is_icon {
            inner
        } else {
            format!("[ {inner} ]")
        };
        let last_idx = text.chars().count().saturating_sub(1);

        let mut col = x0;
        for (i, ch) in text.chars().enumerate() {
            if col >= end {
                break;
            }
            let is_bracket = !is_icon && (i == 0 || i == last_idx);
            let ch_fg = if is_bracket { brk_fg } else { fg };
            surface.put_char(col, y, ch, solid(ch_fg, bg));
            col += 1;
        }
        while col < end {
            surface.put_char(col, y, ' ', solid(fg, bg));
            col += 1;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::event::{Point, Rect as QRect};
        use crate::primitive::LayoutCtx;
        use crate::primitives::button::{cell_button_layout, ButtonHit};
        use crate::tui::cell_surface::TuiCellSurface;
        use crate::tui::ratatui_color;
        use crate::types::{Icon, WidgetId};
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        /// Must clamp against the button's own paint width rather than
        /// growing the gap string without bound — an unvalidated
        /// `f32::MAX`/non-finite `icon_gap` must neither panic nor
        /// allocate anywhere near that size.
        #[test]
        fn icon_gap_cells_clamps_non_finite_and_huge_gaps() {
            assert_eq!(icon_gap_cells(f32::MAX, 10), 10);
            assert_eq!(icon_gap_cells(f32::INFINITY, 10), 10);
            assert_eq!(icon_gap_cells(f32::NAN, 10), 10);
        }

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf[(x, y)].symbol().chars().next().unwrap_or(' ')
        }

        fn cell_bg(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
            buf[(x, y)]
                .style()
                .bg
                .unwrap_or(ratatui::style::Color::Reset)
        }

        fn cell_fg(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
            buf[(x, y)]
                .style()
                .fg
                .unwrap_or(ratatui::style::Color::Reset)
        }

        fn mk_button(label: &str) -> Button {
            Button::new(WidgetId::new("dialog:ok"), label)
        }

        /// Lay out and paint `button` into `area` on a fresh [`Buffer`],
        /// `is_hovered`/`is_pressed` both `false` — the same resting
        /// paint a [`Button`] gets through the contract path when nothing
        /// is hovered or pressed.
        fn paint_button(area: RRect, button: &Button, theme: &Theme) -> (Buffer, ButtonLayout) {
            paint_interactive(area, button, theme, false, false)
        }

        /// [`paint`], threading `is_hovered`/`is_pressed` through to
        /// [`super::paint`] directly — exercises the shared painter's own
        /// tint logic without a [`crate::PaintEnv`].
        fn paint_interactive(
            area: RRect,
            button: &Button,
            theme: &Theme,
            is_hovered: bool,
            is_pressed: bool,
        ) -> (Buffer, ButtonLayout) {
            let mut buf = Buffer::empty(area);
            let layout = cell_button_layout(
                button,
                &LayoutCtx::new(1.0, 1.0, false),
                QRect::new(
                    area.x as f32,
                    area.y as f32,
                    area.width as f32,
                    area.height as f32,
                ),
            );
            let mut cursor_sink = None;
            let mut surface = TuiCellSurface::over(
                &mut buf,
                crate::backend::ColorDepth::TrueColor,
                &mut cursor_sink,
            );
            super::paint(button, &layout, &mut surface, theme, is_hovered, is_pressed);
            (buf, layout)
        }

        // Parametrized over the area's origin — LESSONS.md "Layout helpers
        // must return coords in the same frame across backends".
        // `cell_button_layout` bakes `area.x`/`area.y` straight into
        // `Button::layout`'s returned bounds (absolute frame), so the
        // regression guard is a full paint+hit_test round trip re-run at a
        // non-zero origin.
        fn primary_round_trip_at(origin_x: u16, origin_y: u16) {
            let area = RRect::new(origin_x, origin_y, 8, 1);
            let button = mk_button("OK");
            let (buf, layout) = paint_button(area, &button, &Theme::default());

            assert_eq!(cell_char(&buf, origin_x, origin_y), '[');
            assert_eq!(cell_char(&buf, origin_x + 2, origin_y), 'O');
            assert_eq!(cell_char(&buf, origin_x + 3, origin_y), 'K');
            assert_eq!(cell_char(&buf, origin_x + 5, origin_y), ']');

            let hit = layout.hit_test(Point::new(origin_x as f32 + 1.0, origin_y as f32));
            assert_eq!(hit, ButtonHit::Body);
            let outside = layout.hit_test(Point::new(origin_x as f32 + 50.0, origin_y as f32));
            assert_eq!(outside, ButtonHit::Outside);
        }

        #[test]
        fn primary_paint_and_hit_round_trip() {
            primary_round_trip_at(0, 0);
        }

        #[test]
        fn primary_paint_and_hit_round_trip_at_nonzero_origin() {
            primary_round_trip_at(7, 13);
        }

        #[test]
        fn icon_variant_paints_bare_label_without_brackets() {
            let area = RRect::new(0, 0, 4, 1);
            let mut button = mk_button("X");
            button.variant = ButtonVariant::Icon;
            let (buf, _) = paint_button(area, &button, &Theme::default());
            assert_eq!(cell_char(&buf, 0, 0), 'X');
            assert_eq!(cell_char(&buf, 1, 0), ' ');
        }

        #[test]
        fn disabled_button_still_paints_but_dims() {
            let area = RRect::new(0, 0, 8, 1);
            let mut button = mk_button("OK");
            button.disabled = true;
            let (buf, _) = paint_button(area, &button, &Theme::default());
            assert_eq!(cell_char(&buf, 2, 0), 'O');
        }

        /// A taller cell centres the label (keeping it off the focus ring's
        /// edge rows) while the hit rect still covers the whole cell.
        #[test]
        fn taller_area_centres_the_label_and_keeps_whole_cell_hit_bounds() {
            let area = RRect::new(0, 0, 8, 3);
            let (buf, layout) = paint_button(area, &mk_button("OK"), &Theme::default());
            assert_eq!(cell_char(&buf, 0, 1), '[', "label paints on the middle row");
            assert_eq!(cell_char(&buf, 0, 0), ' ');
            assert_eq!(cell_char(&buf, 0, 2), ' ');
            assert_eq!(layout.bounds.height.round() as u16, area.height);
            assert_eq!(layout.hit_test(Point::new(0.0, 0.0)), ButtonHit::Body);
        }

        #[test]
        fn zero_size_is_a_no_op() {
            let (buf, _) =
                paint_button(RRect::new(0, 0, 0, 0), &mk_button("OK"), &Theme::default());
            assert_eq!(buf.area, RRect::new(0, 0, 0, 0));
        }

        #[test]
        fn hovered_background_differs_from_resting() {
            let area = RRect::new(0, 0, 8, 1);
            let theme = Theme::default();
            let button = mk_button("OK");

            let (resting_buf, _) = paint_interactive(area, &button, &theme, false, false);
            let (hovered_buf, _) = paint_interactive(area, &button, &theme, true, false);

            assert_ne!(
                cell_bg(&resting_buf, 3, 0),
                cell_bg(&hovered_buf, 3, 0),
                "hovering should repaint the button background"
            );
        }

        #[test]
        fn pressed_background_differs_from_hovered_and_resting() {
            let area = RRect::new(0, 0, 8, 1);
            let theme = Theme::default();
            let button = mk_button("OK");

            let (resting_buf, _) = paint_interactive(area, &button, &theme, false, false);
            let (hovered_buf, _) = paint_interactive(area, &button, &theme, true, false);
            let (pressed_buf, _) = paint_interactive(area, &button, &theme, true, true);

            assert_ne!(cell_bg(&pressed_buf, 3, 0), cell_bg(&resting_buf, 3, 0));
            assert_ne!(cell_bg(&pressed_buf, 3, 0), cell_bg(&hovered_buf, 3, 0));
        }

        #[test]
        fn disabled_button_ignores_hover_and_pressed() {
            let area = RRect::new(0, 0, 8, 1);
            let theme = Theme::default();
            let mut button = mk_button("OK");
            button.disabled = true;

            let (resting_buf, _) = paint_interactive(area, &button, &theme, false, false);
            let (hovered_buf, _) = paint_interactive(area, &button, &theme, true, true);

            assert_eq!(
                cell_bg(&resting_buf, 3, 0),
                cell_bg(&hovered_buf, 3, 0),
                "a disabled button must not paint hover/pressed tints"
            );
        }

        /// The contract painter takes hover and press from its
        /// [`crate::PaintEnv`], addressed by the button's own id; state
        /// naming another widget leaves the button at rest.
        #[test]
        fn contract_paint_tints_from_the_env_interaction() {
            use crate::interaction::InteractionState;
            use crate::primitive::{PaintEnv, Primitive};

            let area = RRect::new(0, 0, 8, 1);
            let theme = Theme::default();
            let style = crate::Style::default();
            let button = mk_button("OK");
            let paint_with = |interaction: &InteractionState| {
                let mut buf = Buffer::empty(area);
                let layout = cell_button_layout(
                    &button,
                    &LayoutCtx::new(1.0, 1.0, false),
                    QRect::new(0.0, 0.0, 8.0, 1.0),
                );
                let env = PaintEnv::new(&theme, &style).with_interaction(interaction);
                Primitive::paint_cells(
                    &button,
                    &layout,
                    &mut crate::tui::BufferCellSurface::new(&mut buf),
                    &env,
                );
                cell_bg(&buf, 3, 0)
            };

            let resting = paint_with(&InteractionState::new());
            let hovered = paint_with(&InteractionState::from_parts(Some(button.id), None));
            let pressed = paint_with(&InteractionState::from_parts(
                Some(button.id),
                Some(button.id),
            ));
            let elsewhere = paint_with(&InteractionState::from_parts(
                Some(WidgetId::new("dialog:cancel")),
                None,
            ));

            assert_ne!(hovered, resting, "a hovered env must tint the button");
            assert_ne!(
                pressed, hovered,
                "a pressed env must paint the pressed tint"
            );
            assert_eq!(elsewhere, resting, "hover on another widget must not tint");
        }

        /// `Danger`/`Warning` paint intent entirely through the foreground
        /// colour on TUI — both the bracket chrome and the label text — and
        /// leave the background untouched (same `surface_bg` as `Secondary`),
        /// unlike `Primary`'s solid `accent_bg` fill.
        #[test]
        fn danger_variant_paints_error_fg_on_brackets_and_label_with_no_bg_fill() {
            let area = RRect::new(0, 0, 10, 1);
            let theme = Theme::default();
            let mut button = mk_button("Delete");
            button.variant = ButtonVariant::Danger;

            let (buf, _) = paint_button(area, &button, &theme);

            let bracket_fg = cell_fg(&buf, 0, 0);
            let label_fg = cell_fg(&buf, 2, 0);
            assert_eq!(bracket_fg, ratatui_color(theme.error_fg));
            assert_eq!(label_fg, ratatui_color(theme.error_fg));
            assert_eq!(
                cell_bg(&buf, 0, 0),
                ratatui_color(theme.surface_bg),
                "Danger must not fill a solid background on TUI"
            );
        }

        #[test]
        fn warning_variant_paints_warning_fg_on_brackets_and_label() {
            let area = RRect::new(0, 0, 10, 1);
            let theme = Theme::default();
            let mut button = mk_button("Overwrite");
            button.variant = ButtonVariant::Warning;

            let (buf, _) = paint_button(area, &button, &theme);

            assert_eq!(cell_fg(&buf, 0, 0), ratatui_color(theme.warning_fg));
            assert_eq!(cell_fg(&buf, 2, 0), ratatui_color(theme.warning_fg));
        }

        /// A focused `Danger` button must stay visually distinct from an
        /// unfocused one: the bracket chrome switches to `accent_fg` (the
        /// same focus cue `Primary`/`Secondary` already paint) while the
        /// label keeps `error_fg` either way.
        #[test]
        fn focused_danger_button_bracket_differs_from_unfocused_but_label_keeps_intent_colour() {
            let area = RRect::new(0, 0, 10, 1);
            let theme = Theme::default();
            let mut button = mk_button("Delete");
            button.variant = ButtonVariant::Danger;

            let (unfocused_buf, _) = paint_button(area, &button, &theme);
            let unfocused_bracket_fg = cell_fg(&unfocused_buf, 0, 0);
            let unfocused_label_fg = cell_fg(&unfocused_buf, 2, 0);

            button.focused = true;
            let (focused_buf, _) = paint_button(area, &button, &theme);
            let focused_bracket_fg = cell_fg(&focused_buf, 0, 0);
            let focused_label_fg = cell_fg(&focused_buf, 2, 0);

            assert_ne!(
                focused_bracket_fg, unfocused_bracket_fg,
                "a focused Danger button's bracket chrome must look different \
                 from an unfocused one"
            );
            assert_eq!(
                focused_bracket_fg,
                ratatui_color(theme.accent_fg),
                "focus wins on the bracket chrome ahead of the intent colour"
            );
            assert_eq!(
                unfocused_label_fg,
                ratatui_color(theme.error_fg),
                "the label keeps the intent colour while unfocused"
            );
            assert_eq!(
                focused_label_fg,
                ratatui_color(theme.error_fg),
                "the label keeps the intent colour while focused too"
            );
        }

        #[test]
        fn disabled_danger_button_paints_muted_fg_not_the_intent_colour() {
            let area = RRect::new(0, 0, 10, 1);
            let theme = Theme::default();
            let mut button = mk_button("Delete");
            button.variant = ButtonVariant::Danger;
            button.disabled = true;

            let (buf, _) = paint_button(area, &button, &theme);

            assert_eq!(cell_fg(&buf, 2, 0), ratatui_color(theme.muted_fg));
        }

        /// `[ ⚠ Delete ]`: a Danger button with a leading icon, painted with
        /// Nerd Fonts on (glyph) and off (fallback).
        #[test]
        fn danger_button_with_icon_paints_glyph_or_fallback_per_nerd_fonts_flag() {
            let area = RRect::new(0, 0, 14, 1);
            let theme = Theme::default();
            let mut button = mk_button("Delete");
            button.variant = ButtonVariant::Danger;
            button.icon = Some(Icon::new("\u{f071}", "!"));

            let row_text = |nerd_fonts_enabled: bool| -> String {
                let layout = cell_button_layout(
                    &button,
                    &LayoutCtx::new(1.0, 1.0, nerd_fonts_enabled),
                    QRect::new(0.0, 0.0, area.width as f32, area.height as f32),
                );
                let mut buf = Buffer::empty(area);
                let mut cursor_sink = None;
                let mut surface = TuiCellSurface::over(
                    &mut buf,
                    crate::backend::ColorDepth::TrueColor,
                    &mut cursor_sink,
                );
                paint(&button, &layout, &mut surface, &theme, false, false);
                (area.x..area.x + area.width)
                    .map(|x| cell_char(&buf, x, area.y))
                    .collect()
            };

            let with_nerd_fonts = row_text(true);
            assert!(
                with_nerd_fonts.contains('\u{f071}'),
                "nerd_fonts_enabled: true should paint the glyph half of the icon, got {with_nerd_fonts:?}"
            );
            assert!(with_nerd_fonts.contains("Delete"));

            let without_nerd_fonts = row_text(false);
            assert!(
                without_nerd_fonts.contains('!'),
                "nerd_fonts_enabled: false should paint the fallback half of the icon, got {without_nerd_fonts:?}"
            );
            assert!(!without_nerd_fonts.contains('\u{f071}'));
        }

        /// A huge/non-finite `icon_gap` must neither panic nor allocate
        /// anywhere near its raw size — the unit-level clamp is covered
        /// directly by [`icon_gap_cells_clamps_non_finite_and_huge_gaps`];
        /// this is the black-box "painting one doesn't blow up" check.
        #[test]
        fn huge_icon_gap_is_clamped_instead_of_overflowing_or_panicking() {
            let area = RRect::new(0, 0, 14, 1);
            let mut button = mk_button("Save");
            button.icon = Some(Icon::new("S", "S"));
            button.icon_gap = f32::MAX;

            let _ = paint_button(area, &button, &Theme::default());
        }

        #[test]
        fn trailing_icon_paints_after_the_label() {
            let area = RRect::new(0, 0, 14, 1);
            let mut button = mk_button("Save");
            button.icon = Some(Icon::new("S", "S"));
            button.icon_position = ButtonIconPosition::Trailing;

            let (buf, _) = paint_button(area, &button, &Theme::default());
            let row: String = (area.x..area.x + area.width)
                .map(|x| cell_char(&buf, x, area.y))
                .collect();

            let save_idx = row.find("Save").expect("label must paint");
            let icon_idx = row.rfind('S').expect("icon glyph must paint");
            assert!(
                icon_idx > save_idx,
                "a Trailing icon must paint after the label, got {row:?}"
            );
        }

        #[test]
        fn no_icon_paints_the_label_alone() {
            let area = RRect::new(0, 0, 8, 1);
            let button = mk_button("OK");
            assert_eq!(button.icon, None);

            let (buf, _) = paint_button(area, &button, &Theme::default());
            assert_eq!(cell_char(&buf, 0, 0), '[');
            assert_eq!(cell_char(&buf, 2, 0), 'O');
        }
    }
}

/// `paint` is the shared [`crate::paint_surface::PaintSurface`]
/// implementation of button painting (fill/border/label), used by
/// backends that have no dedicated native rasteriser for `Button` —
/// the same minimal-chrome fallback [`crate::primitives::progress`]
/// already uses for `ProgressBar`.
/// Outset gap between a primitive's own edge and its focus-visible
/// ring, in surface-native units (GTK/macOS points, Win-GUI DIPs).
pub(crate) const FOCUS_RING_GAP: f32 = 2.0;

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Button, ButtonIconPosition, ButtonLayout, ButtonMetrics, ButtonVariant};
    use crate::backend::PressableVisual;
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;

    use super::FOCUS_RING_GAP;

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
    /// a caller whose own fill/border also radiuses at
    /// `style.corner_radius` — [`TextInput`] via `crate::primitives::text_input`,
    /// every per-backend rasteriser listed below — gets a matching ring
    /// for free without a separate radius argument to drift), and the
    /// stroke width [`Style::focus_ring_width`] configures. This
    /// module's own [`paint`] below computes its ring through here but
    /// then overwrites [`FocusRingGeometry::corner_radius`] with its own
    /// [`ButtonMetrics::corner_radius`] (`Style::button_radius`, not
    /// `style.corner_radius`) before stroking, since `Button`'s body
    /// radiuses narrower than the shared overlay default. A per-backend
    /// `TextInput` rasteriser that adds a ring stroke should call
    /// through here too, rather than re-deriving the outset gap
    /// independently, so widening `Style::focus_ring_width` widens every
    /// primitive's ring identically instead of drifting through
    /// hand-tuned literals.
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
    /// [`crate::primitive::Primitive::paint`] calls this with the
    /// hover/press state its [`crate::PaintEnv`] reports, which tints `bg` via
    /// [`Theme::button_pressed_bg`]/[`Theme::button_hover_bg`] (pressed
    /// takes priority over hover, matching every other
    /// host-set-field-driven rasteriser in this crate — a button can't
    /// be mid-press without also being hovered). A caller painting a
    /// [`ButtonSpec`]-backed action (`crate::Toolbar`, `crate::Dialog`,
    /// `crate::Toast`, `crate::ButtonRow`) passes its own resolved
    /// hover/pressed state directly instead, since those kinds track it
    /// index- or id-keyed rather than on a materialised [`Button`].
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
        } else {
            match PressableVisual::resolve(is_pressed, false, is_hovered) {
                PressableVisual::PressedOrActive => theme.button_pressed_bg(bg),
                PressableVisual::Hovered => theme.button_hover_bg(bg),
                PressableVisual::Resting => bg,
            }
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
        // `crate::FocusTree::is_focus_visible`.
        if button.focused && !button.disabled {
            let mut ring = focus_ring_geometry(rect, style);
            // `focus_ring_geometry` reads `style.corner_radius` by
            // default (the radius every other rounded-rect overlay's
            // ring shares), but a `Button`'s own body paints at
            // `metrics.corner_radius` (`Style::button_radius`) above —
            // overridden here so the ring traces the same corner the
            // fill/border just drew, rather than a wider `corner_radius`
            // ring sitting visibly outside a tighter body.
            ring.corner_radius = metrics.corner_radius;
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

        /// The ring traces the button's own body radius
        /// (`Style::button_radius`), not the wider `Style::corner_radius`
        /// every other overlay's ring shares — otherwise a radius-6
        /// ring would sit visibly outside a radius-2 body.
        #[test]
        fn focused_button_ring_radius_matches_button_radius_not_corner_radius() {
            let mut button = Button::new(WidgetId::new("b"), "OK");
            button.focused = true;
            let theme = Theme::default();
            let style = Style::default()
                .with_button_radius(2.0)
                .with_corner_radius(6.0);
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let body = surface
                .fills
                .iter()
                .find(|(_, _, c)| *c == theme.accent_bg)
                .expect("the button body must fill theme.accent_bg");
            assert_eq!(body.1, 2.0, "the body must fill at button_radius");

            let ring = surface
                .strokes
                .iter()
                .find(|(_, _, color, _)| *color == theme.focus_ring_color())
                .expect("a focused button must paint a ring stroke");
            assert_eq!(
                ring.1, 2.0,
                "the ring must stroke at button_radius, not corner_radius"
            );
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

        #[test]
        fn secondary_variant_fills_surface_bg() {
            let mut button = Button::new(WidgetId::new("b"), "Cancel");
            button.variant = ButtonVariant::Secondary;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (_, _, fill) = surface.fills.first().expect("button must paint a fill");
            assert_eq!(*fill, theme.surface_bg);
        }

        #[test]
        fn disabled_button_fills_surface_bg_regardless_of_variant() {
            let mut button = Button::new(WidgetId::new("b"), "OK");
            button.disabled = true;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (_, _, fill) = surface.fills.first().expect("button must paint a fill");
            assert_eq!(*fill, theme.surface_bg);
        }

        /// `is_hovered: true` tints the fill toward `Theme::button_hover_bg`
        /// — this painter's own hover story, which
        /// [`Primitive::paint`] reaches by passing its env's hover state
        /// straight through.
        #[test]
        fn hover_state_changes_the_fill_colour() {
            let button = Button::new(WidgetId::new("b"), "OK");
            let theme = Theme::default();
            let style = Style::default();

            let mut resting = RecordingSurface::default();
            paint(&button, &mut resting, &theme, RECT, &style, false, false);
            let mut hovered = RecordingSurface::default();
            paint(&button, &mut hovered, &theme, RECT, &style, true, false);

            assert_ne!(
                resting.fills.first().map(|(_, _, c)| *c),
                hovered.fills.first().map(|(_, _, c)| *c),
                "hovering a button must repaint its fill",
            );
        }

        /// `is_pressed: true` wins over `is_hovered: true` — the same
        /// priority [`crate::backend::PressableVisual::resolve`] documents.
        #[test]
        fn pressed_state_wins_over_hover_and_changes_the_fill_colour() {
            let button = Button::new(WidgetId::new("b"), "OK");
            let theme = Theme::default();
            let style = Style::default();

            let mut hovered = RecordingSurface::default();
            paint(&button, &mut hovered, &theme, RECT, &style, true, false);
            let mut pressed = RecordingSurface::default();
            paint(&button, &mut pressed, &theme, RECT, &style, true, true);

            assert_ne!(
                hovered.fills.first().map(|(_, _, c)| *c),
                pressed.fills.first().map(|(_, _, c)| *c),
                "a pressed button must paint Theme::button_pressed_bg, not stay at the hover tint",
            );
        }

        /// The contract painter reads hover and press from its
        /// [`crate::PaintEnv`], addressed by the button's own id.
        #[test]
        fn contract_paint_tints_from_the_env_interaction() {
            use crate::interaction::InteractionState;
            use crate::primitive::{PaintEnv, Primitive};

            let button = Button::new(WidgetId::new("b"), "OK");
            let theme = Theme::default();
            let style = Style::default();
            let fill_with = |interaction: &InteractionState| {
                let env = PaintEnv::new(&theme, &style).with_interaction(interaction);
                let mut surface = RecordingSurface::default();
                Primitive::paint(&button, &button.layout(RECT), &mut surface, &env);
                surface.fills.first().map(|(_, _, c)| *c)
            };

            let resting = fill_with(&InteractionState::new());
            let hovered = fill_with(&InteractionState::from_parts(Some(button.id), None));
            let pressed = fill_with(&InteractionState::from_parts(
                Some(button.id),
                Some(button.id),
            ));

            let base = resting.expect("button must paint a fill");
            assert_eq!(hovered, Some(theme.button_hover_bg(base)));
            assert_eq!(pressed, Some(theme.button_pressed_bg(base)));
        }

        /// A `Leading` icon paints before the label, `icon_gap` clear of
        /// it.
        #[test]
        fn leading_icon_paints_before_the_label_icon_gap_apart() {
            let mut button = Button::new(WidgetId::new("b"), "Save");
            button.icon = Some(Icon::new("S", "S"));
            button.icon_position = ButtonIconPosition::Leading;
            button.icon_gap = 10.0;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (icon_rect, _, _) = surface
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
                icon_rect.x < label_rect.x,
                "a Leading icon must paint before the label: icon.x={}, label.x={}",
                icon_rect.x,
                label_rect.x,
            );
            let gap = label_rect.x - (icon_rect.x + icon_rect.width);
            assert!(
                (gap - button.icon_gap).abs() < 0.5,
                "expected icon_gap ({}) clear of the icon, got {gap}",
                button.icon_gap,
            );
        }

        /// `Trailing` flips the order: the label paints first, the icon
        /// after it.
        #[test]
        fn trailing_icon_paints_after_the_label() {
            let mut button = Button::new(WidgetId::new("b"), "Save");
            button.icon = Some(Icon::new("S", "S"));
            button.icon_position = ButtonIconPosition::Trailing;
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (icon_rect, _, _) = surface
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
                label_rect.x < icon_rect.x,
                "a Trailing icon must paint after the label: label.x={}, icon.x={}",
                label_rect.x,
                icon_rect.x,
            );
        }

        /// The label is inset from `rect`'s own edge by at least
        /// `Style::default().padding` — `ButtonMetrics::padding_x`'s own
        /// source — not flush against it.
        #[test]
        fn label_is_inset_by_at_least_the_style_padding() {
            let button = Button::new(WidgetId::new("b"), "OK");
            let theme = Theme::default();
            let style = Style::default();
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, RECT, &style, false, false);

            let (label_rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "OK")
                .expect("label must paint a text run");
            assert!(
                label_rect.x - RECT.x >= style.padding - 0.5,
                "expected the label inset by at least style.padding ({}), got {}",
                style.padding,
                label_rect.x - RECT.x,
            );
        }

        /// At `RECT`'s own width, plain centring alone already clears
        /// `style.padding` for a two-character label, so the test above
        /// never actually exercises the `.max(metrics.padding_x)` clamp
        /// in this painter's centring arithmetic. This test narrows the
        /// rect to exactly `label_w + style.padding` wide — `label_w`
        /// from `RecordingSurface::surface_measure_text`'s own `8.0`-per-
        /// char formula — where plain centring would put the label at
        /// half of `style.padding`, below the floor, so the assertion
        /// only holds if the clamp binds.
        #[test]
        fn label_clamps_to_style_padding_when_plain_centering_would_not() {
            let button = Button::new(WidgetId::new("b"), "OK");
            let theme = Theme::default();
            let style = Style::default();
            let label_w = 2.0 * 8.0; // "OK": 2 chars at 8.0px/char.
            let rect = Rect::new(RECT.x, RECT.y, label_w + style.padding, RECT.height);
            let mut surface = RecordingSurface::default();

            paint(&button, &mut surface, &theme, rect, &style, false, false);

            let (label_rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, text, _)| text == "OK")
                .expect("label must paint a text run");
            assert!(
                label_rect.x - rect.x >= style.padding - 0.5,
                "expected the clamp to hold the label at style.padding ({}), got {}",
                style.padding,
                label_rect.x - rect.x,
            );
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
            .with_button_radius(9.0);

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
        assert_eq!(metrics.corner_radius, style.button_radius);
    }

    /// A button's corner radius tracks [`Style::button_radius`], not
    /// [`Style::corner_radius`] — the two are independent tokens, so
    /// retuning one leaves the other's consumer unchanged.
    #[test]
    fn resolved_metrics_corner_radius_is_independent_of_style_corner_radius() {
        let style = Style::default().with_corner_radius(20.0);
        let metrics = ButtonMetrics::resolve(&style);
        assert_eq!(metrics.corner_radius, Style::default().button_radius);
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

    /// `from_legacy` carries `icon`/`tooltip`/`disabled` straight through
    /// and leaves `variant` at [`ButtonSpec::new`]'s own `Primary` default
    /// — a caller that derives a variant from its own legacy field sets
    /// it afterwards.
    #[test]
    fn from_legacy_carries_icon_tooltip_and_disabled_with_primary_variant() {
        let icon = crate::types::Icon::new("W", "W");
        let spec = ButtonSpec::from_legacy(
            "Refresh",
            Some(icon.clone()),
            Some("Reload the tree".to_string()),
            true,
        );

        assert_eq!(spec.label, "Refresh");
        assert_eq!(spec.icon, Some(icon));
        assert_eq!(spec.tooltip, Some("Reload the tree".to_string()));
        assert!(spec.disabled);
        assert_eq!(spec.variant, ButtonVariant::Primary);
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
        assert_eq!(
            Button::new(WidgetId::new("b"), "OK").focus(),
            FocusSpec::Stop
        );
    }

    #[test]
    fn primitive_layout_and_paint_cells_round_trip() {
        let b = Button::new(WidgetId::new("dialog:ok"), "OK");
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let rect = Rect::new(0.0, 0.0, 6.0, 1.0);
        let layout = Primitive::layout(&b, &ctx, rect);
        assert_eq!(layout.bounds, rect);

        #[cfg(feature = "tui")]
        {
            let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 6, 1));
            Primitive::paint_cells(
                &b,
                &layout,
                &mut crate::tui::BufferCellSurface::new(&mut buf),
                &crate::PaintEnv::new(&crate::theme::Theme::default(), &crate::Style::default()),
            );
            let row: String = (0..6).map(|x| buf[(x, 0)].symbol().to_string()).collect();
            assert!(row.contains('O'));
        }
    }

    #[test]
    fn enabled_button_advertises_a_click_action() {
        let b = Button::new(WidgetId::new("b"), "OK");
        assert_eq!(b.a11y_actions(&()), vec![A11yAction::Click]);
    }

    #[test]
    fn disabled_button_advertises_no_actions() {
        let mut b = Button::new(WidgetId::new("b"), "OK");
        b.disabled = true;
        assert!(b.a11y_actions(&()).is_empty());
    }

    #[test]
    fn handle_enter_activates() {
        let b = Button::new(WidgetId::new("b"), "OK");
        let layout = ButtonLayout {
            bounds: Rect::new(0.0, 0.0, 6.0, 1.0),
            nerd_fonts_enabled: false,
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(b.id, &mut host);
        let outcome = b.handle(
            &mut (),
            &key_event(Key::Named(NamedKey::Enter)),
            &layout,
            &mut cx,
        );
        assert_eq!(outcome, Handled::Event(ButtonEvent::Activated { id: b.id }));
    }

    #[test]
    fn handle_click_requests_focus_and_activates() {
        let b = Button::new(WidgetId::new("b"), "OK");
        let layout = ButtonLayout {
            bounds: Rect::new(0.0, 0.0, 6.0, 1.0),
            nerd_fonts_enabled: false,
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(b.id, &mut host);
        let outcome = b.handle(&mut (), &down_event(), &layout, &mut cx);
        assert_eq!(outcome, Handled::Event(ButtonEvent::Activated { id: b.id }));
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn disabled_button_ignores_activation_events() {
        let mut b = Button::new(WidgetId::new("b"), "OK");
        b.disabled = true;
        let layout = ButtonLayout {
            bounds: Rect::new(0.0, 0.0, 6.0, 1.0),
            nerd_fonts_enabled: false,
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(b.id, &mut host);
        let outcome = b.handle(
            &mut (),
            &key_event(Key::Named(NamedKey::Enter)),
            &layout,
            &mut cx,
        );
        assert_eq!(outcome, Handled::Ignored);
    }

    /// Three-way event parity's in-crate leg: an [`A11yAction::Click`]
    /// routed through [`crate::a11y::route_widget_action`] produces the
    /// exact same [`crate::UiEvent`] a direct [`Interactive::handle`]
    /// call with the same `RoutedEvent::Action` does. The TUI
    /// mouse-click leg lives in `tests/widget_contract_button.rs`.
    #[test]
    fn route_widget_action_click_matches_direct_interactive_handle() {
        let b = Button::new(WidgetId::new("dialog:ok"), "OK");
        let id = b.id;
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(id, Rect::new(0.0, 0.0, 6.0, 1.0), Box::new(b.clone()));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let ctx = LayoutCtx::new(1.0, 1.0, false);

        let events =
            crate::a11y::route_widget_action(&tree, &mut states, &ctx, id, A11yAction::Click);

        let layout = ButtonLayout {
            bounds: Rect::new(0.0, 0.0, 6.0, 1.0),
            nerd_fonts_enabled: false,
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(id, &mut host);
        let direct = b.handle(
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
            vec![ButtonEvent::Activated { id }.into_ui_event(id)]
        );
    }
}
