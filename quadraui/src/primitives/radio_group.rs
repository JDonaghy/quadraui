//! `RadioGroup` primitive: a standalone set of mutually-exclusive
//! options — layout, hit-test, keyboard focus/activation (arrow-key
//! wraparound cursor), and its event, all on the widget contract
//! (`Primitive` + `Interactive`, below). `RadioGroup` reaches a frame
//! as a type-erased `crate::frame::Surface::Widget`; there is no
//! `Backend` method or per-platform rasteriser file of its own.
//!
//! Today `Form`'s `Radio`/`RadioGroup` field kind covers the same shape
//! embedded inside a form. `RadioGroup` is additive: a standalone
//! control for a host that wants a radio group outside a form (a
//! settings panel, a toolbar dropdown's contents) without pulling in
//! the whole `Form`/`FormController` machinery. `Form`'s own field
//! kinds are untouched.
//!
//! # Focus and activation
//!
//! Unlike [`crate::Switch`] or [`crate::Button`] (one focusable target
//! each), a `RadioGroup` holds a *cursor* over several options:
//! [`RadioGroup::selected`] is both "which option has focus" and "which
//! option is chosen" — the single-selection-is-the-cursor convention
//! `<input type="radio">` groups use. [`RadioGroup::handle_key`] moves
//! the cursor with `Up`/`Left` (previous) and `Down`/`Right` (next),
//! wrapping at either end exactly like [`crate::compose::focus_group`]'s
//! index-cycling (`None` seeds at the first/last option depending on
//! direction; `Some(i)` steps by `rem_euclid`), and skips over
//! `disabled` options — bounded to one full lap so an all-disabled group
//! can't spin forever. [`RadioGroup::select`] does the same move-and-fire
//! for a pointer click on a specific option, gated on that option's own
//! `disabled` flag alone (a click doesn't require the group to already
//! have focus). Both return the same [`RadioGroupEvent`] so a host wires
//! one event type regardless of input modality.
//!
//! Routing app-wide Tab into/out of the group is the host's job — same
//! division of responsibility as every other primitive here: once the
//! group has focus, arrow keys move the cursor *within* it; Tab leaves
//! it to the host's focus-traversal logic (see
//! [`crate::focus::FocusTree`] for the app-wide owner of "what has
//! focus").

use crate::event::{Key, NamedKey, Point, Rect};
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// A single option within a [`RadioGroup`].
///
/// `#[non_exhaustive]`: built via [`RadioOption::new`] plus direct field
/// assignment (every field is `pub`), not an exhaustive struct literal —
/// the same convention [`RadioGroup`] and [`crate::Switch`] already use,
/// so a future field addition here stays additive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RadioOption {
    pub id: WidgetId,
    pub label: String,
    #[serde(default)]
    pub disabled: bool,
    /// Whether the pointer currently sits over this option. Set by the
    /// host from its own hover tracking (an
    /// [`crate::interaction::InteractionState`] keyed by
    /// [`RadioOption::id`], or any other source); read by this type's
    /// own shared pixel painter (`native_surface_paint::paint`) to tint
    /// this option's marker toward [`crate::Theme::button_hover_bg`] —
    /// the same host-set convention [`RadioGroup::focused`] already
    /// follows.
    #[serde(default)]
    pub hovered: bool,
    /// Whether the pointer currently holds this option pressed. Same
    /// host-set convention as [`RadioOption::hovered`]; read by that
    /// same pixel painter to tint the marker toward
    /// [`crate::Theme::button_pressed_bg`], which wins over
    /// [`RadioOption::hovered`] when both are set.
    #[serde(default)]
    pub pressed: bool,
}

impl RadioOption {
    pub fn new(id: WidgetId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            disabled: false,
            hovered: false,
            pressed: false,
        }
    }
}

/// Declarative description of a standalone radio group.
///
/// # Examples
///
/// ```
/// use quadraui::{
///     Key, Modifiers, NamedKey, Point, Rect, RadioGroup, RadioGroupEvent, RadioGroupHit,
///     RadioOption, RadioOptionMeasure, WidgetId,
/// };
///
/// let mut group = RadioGroup::new(
///     WidgetId::new("settings:theme"),
///     vec![
///         RadioOption::new(WidgetId::new("settings:theme:light"), "Light"),
///         RadioOption::new(WidgetId::new("settings:theme:dark"), "Dark"),
///     ],
/// );
/// group.focused = true;
///
/// let layout = group.layout(Rect::new(0.0, 0.0, 10.0, 2.0), |_option| RadioOptionMeasure {
///     width: 10.0,
///     height: 1.0,
/// });
/// assert_eq!(layout.hit_test(Point::new(1.0, 1.0)), RadioGroupHit::Option(1));
///
/// let event = group.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default());
/// assert_eq!(event, Some(RadioGroupEvent::SelectionChanged { index: 0 }));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RadioGroup {
    pub id: WidgetId,
    pub options: Vec<RadioOption>,
    /// Index into [`RadioGroup::options`] that is both the current
    /// selection and the keyboard cursor. `None` means nothing is
    /// selected yet (the state before a user has made a first choice).
    #[serde(default)]
    pub selected: Option<usize>,
    /// Whether this group currently holds keyboard focus. Set by the
    /// host's focus-traversal logic; read by [`RadioGroup::handle_key`]
    /// to gate cursor movement.
    #[serde(default)]
    pub focused: bool,
}

impl RadioGroup {
    /// A group over `options` with no selection and unfocused — set
    /// `selected`/`focused` directly afterwards (every field is `pub`)
    /// rather than via a `with_*` chain — see
    /// `docs/PRIMITIVE_RULES.md`'s rule-8 history of why this crate
    /// stopped adding those.
    pub fn new(id: WidgetId, options: Vec<RadioOption>) -> Self {
        Self {
            id,
            options,
            selected: None,
            focused: false,
        }
    }

    /// Resolve each option's hit-testable bounds, stacking them
    /// vertically from `bounds`'s top-left using `measure` to size each
    /// row — the same measure-closure convention [`crate::Toolbar`] and
    /// [`crate::Form`] already use so a host supplies real text/icon
    /// metrics without `RadioGroup` depending on any backend.
    pub fn layout<F>(&self, bounds: Rect, measure: F) -> RadioGroupLayout
    where
        F: Fn(&RadioOption) -> RadioOptionMeasure,
    {
        let mut items = Vec::with_capacity(self.options.len());
        let mut y = bounds.y;
        for option in &self.options {
            let size = measure(option);
            let height = size.height.min(bounds.y + bounds.height - y).max(0.0);
            items.push(Rect::new(bounds.x, y, bounds.width.min(size.width), height));
            y += size.height;
        }
        RadioGroupLayout { bounds, items }
    }

    /// `Up`/`Left` moves the cursor to the previous enabled option,
    /// `Down`/`Right` to the next, wrapping at either end. Any other key
    /// is unhandled (`None`) — the same "return `None` for a key this
    /// control doesn't own" contract [`crate::Toolbar`] and
    /// [`crate::PipelineView`] already follow.
    pub fn handle_key(&self, key: &Key, _modifiers: &Modifiers) -> Option<RadioGroupEvent> {
        if !self.focused {
            return None;
        }
        let delta = match key {
            Key::Named(NamedKey::Up) | Key::Named(NamedKey::Left) => -1,
            Key::Named(NamedKey::Down) | Key::Named(NamedKey::Right) => 1,
            _ => return None,
        };
        self.move_selection(delta)
            .map(|index| RadioGroupEvent::SelectionChanged { index })
    }

    /// Pointer selection: selects `index` when it names an enabled
    /// option, regardless of [`RadioGroup::focused`] (a click doesn't
    /// require prior keyboard focus the way arrow keys do). The host
    /// calls this after confirming a `MouseDown`/`MouseUp` landed on
    /// [`RadioGroupHit::Option`] via [`RadioGroupLayout::hit_test`].
    pub fn select(&self, index: usize) -> Option<RadioGroupEvent> {
        let option = self.options.get(index)?;
        if option.disabled {
            return None;
        }
        Some(RadioGroupEvent::SelectionChanged { index })
    }

    /// Step the cursor by `delta` (`-1` or `1`), wrapping at either end
    /// and skipping `disabled` options, mirroring
    /// [`crate::compose::focus_group::FocusGroup::cycle`]'s index
    /// semantics exactly: `None` seeds at index `0` for a forward step
    /// or the last index for a backward step, then every step is
    /// `(i + delta).rem_euclid(len)`. The skip loop is bounded to one
    /// full lap (`self.options.len()` iterations) so a group where every
    /// option is disabled returns `None` instead of spinning forever.
    fn move_selection(&self, delta: isize) -> Option<usize> {
        let len = self.options.len();
        if len == 0 {
            return None;
        }
        let len_i = len as isize;
        let mut idx = match self.selected {
            None if delta >= 0 => -1,
            None => 0,
            Some(i) => i as isize,
        };
        for _ in 0..len {
            idx = (idx + delta).rem_euclid(len_i);
            let candidate = idx as usize;
            if !self.options[candidate].disabled {
                return Some(candidate);
            }
        }
        None
    }
}

impl Default for RadioGroup {
    /// An empty, unselected, unfocused group with no options. See
    /// [`RadioGroup::new`] for a group with real options.
    fn default() -> Self {
        Self::new(WidgetId::new(""), Vec::new())
    }
}

/// Fully-resolved radio group layout: `items[i]` is the hit-testable
/// bounds for `options[i]`.
#[derive(Debug, Clone, PartialEq)]
pub struct RadioGroupLayout {
    pub bounds: Rect,
    pub items: Vec<Rect>,
}

/// Size a host measures for one option's row, fed into
/// [`RadioGroup::layout`] via its `measure` closure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RadioOptionMeasure {
    pub width: f32,
    pub height: f32,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioGroupHit {
    Option(usize),
    Outside,
}

impl RadioGroupLayout {
    pub fn hit_test(&self, p: Point) -> RadioGroupHit {
        for (index, item) in self.items.iter().enumerate() {
            if item.contains(p) {
                return RadioGroupHit::Option(index);
            }
        }
        RadioGroupHit::Outside
    }
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────
//
// `RadioGroup` reaches the frame type-erased as
// [`crate::frame::Surface::Widget`]: `layout` resolves each option's own
// row — [`pixel_radio_group_layout`] on a pixel surface,
// [`cell_radio_group_layout`] on a cell grid — `paint` runs the shared
// pixel painter in [`native_surface_paint`], `paint_cells` runs the cell
// painter in [`cell_paint`], and `handle` folds in
// [`RadioGroup::handle_key`]/[`RadioGroup::select`] plus the per-option
// keyboard focus [`crate::interactive::FocusSpec::Children`] declares.

/// Resolve `group`'s layout for a pixel surface: one row per option,
/// `row_height` tall (the caller's resolved chrome line height).
pub(crate) fn pixel_radio_group_layout(
    group: &RadioGroup,
    row_height: f32,
    rect: Rect,
) -> RadioGroupLayout {
    group.layout(rect, |_option: &RadioOption| RadioOptionMeasure {
        width: rect.width,
        height: row_height,
    })
}

/// Resolve `group`'s layout for a cell grid: one whole-cell row per
/// option — the cell-grid twin of [`pixel_radio_group_layout`].
pub(crate) fn cell_radio_group_layout(group: &RadioGroup, rect: Rect) -> RadioGroupLayout {
    group.layout(rect, |_option: &RadioOption| RadioOptionMeasure {
        width: rect.width,
        height: 1.0,
    })
}

impl crate::primitive::Primitive for RadioGroup {
    type Layout = RadioGroupLayout;
    type Event = RadioGroupHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::RadioGroup(self.clone())
    }

    /// One row per option, `row_height` tall, as wide as a selection
    /// glyph plus the widest option label — the chrome's own `("(x) "`
    /// 4-character marker on a cell grid (matching [`cell_paint::paint`]'s
    /// `"({glyph}) {label}"` text), or the circular marker plus its 4px
    /// gap on a pixel surface (matching [`native_surface_paint::paint_resolved`]'s
    /// `marker_size + 4.0` label offset), approximated here as
    /// `1.5 * line_height` the same way [`crate::compose`]'s
    /// `toggle_control_size` approximates a toggle's own chrome.
    /// `NaturalSize::NONE` for an empty group: there is nothing to size.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        if self.options.is_empty() {
            return crate::NaturalSize::NONE;
        }
        let (row_height, glyph_width) = match ctx.units() {
            crate::primitive::Units::Cells => (1.0, 4.0),
            _ => (ctx.line_height, 1.5 * ctx.line_height),
        };
        let widest_label = self
            .options
            .iter()
            .map(|opt| ctx.text_width(&opt.label, crate::FontRole::Chrome))
            .fold(0.0_f32, f32::max);
        crate::NaturalSize::new(
            Some(glyph_width + widest_label),
            Some(self.options.len() as f32 * row_height),
        )
    }

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> RadioGroupLayout {
        match ctx.units() {
            crate::primitive::Units::Cells => cell_radio_group_layout(self, rect),
            _ => pixel_radio_group_layout(self, ctx.line_height, rect),
        }
    }

    /// The shared pixel painter, against the already-resolved `layout`.
    fn paint(
        &self,
        layout: &RadioGroupLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        native_surface_paint::paint_resolved(self, layout, surface, theme);
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
        layout: &RadioGroupLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        cell_paint::paint(self, layout, cells, theme);
    }
}

/// [`RadioGroup`]'s interaction state — which option's keyboard focus
/// stop is currently live, so an assistive-technology `Click` (which
/// carries no sub-target of its own) knows which option to activate.
/// Kept here rather than on the geometry/selection-only descriptor,
/// same convention [`crate::primitives::scrollbar::ScrollbarState`]
/// follows.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RadioGroupState {
    focused_option: Option<usize>,
}

impl crate::interactive::Interactive for RadioGroup {
    type State = RadioGroupState;
    type Action = RadioGroupEvent;

    /// One focus stop per option — walked by the host's tab order, and
    /// addressed back to [`crate::interactive::RoutedEvent::FocusIn`]
    /// below by its index packed into
    /// [`crate::interactive::SubTarget`]. `FocusSpec::None` for an
    /// empty group: there is nothing for keyboard focus to land on.
    fn focus(&self) -> crate::interactive::FocusSpec {
        if self.options.is_empty() {
            crate::interactive::FocusSpec::None
        } else {
            crate::interactive::FocusSpec::Children(
                (0..self.options.len())
                    .map(|i| crate::interactive::FocusStop {
                        sub: crate::interactive::SubTarget(i as u64),
                    })
                    .collect(),
            )
        }
    }

    /// Every option's own row, most-specific order doesn't matter here
    /// since rows never overlap — translated out of `layout`'s absolute
    /// frame into the widget-local space this method is contracted to
    /// answer in.
    fn sub_regions(&self, layout: &RadioGroupLayout) -> Vec<(Rect, crate::interactive::SubTarget)> {
        let (ox, oy) = (layout.bounds.x, layout.bounds.y);
        layout
            .items
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    Rect::new(r.x - ox, r.y - oy, r.width, r.height),
                    crate::interactive::SubTarget(i as u64),
                )
            })
            .collect()
    }

    /// `Click` selects whichever option currently holds keyboard focus.
    fn a11y_actions(&self, _state: &RadioGroupState) -> Vec<crate::a11y::A11yAction> {
        if self.options.is_empty() {
            Vec::new()
        } else {
            vec![crate::a11y::A11yAction::Click]
        }
    }

    /// Arrow keys move the cursor (see [`RadioGroup::handle_key`] —
    /// note this does *not* re-check `self.focused`, the legacy,
    /// non-contract convention a host sets by hand: the engine only
    /// ever routes a `Key` event to whichever widget its own
    /// `FocusTree` already names as focused, so a key reaching here has
    /// already been confirmed focused); a left click selects whichever
    /// option it landed on (see [`RadioGroup::select`]), regardless of
    /// prior focus.
    fn handle(
        &self,
        state: &mut RadioGroupState,
        event: &crate::interactive::RoutedEvent,
        layout: &RadioGroupLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<RadioGroupEvent> {
        use crate::interactive::{KeyEvent, PointerKind, RoutedEvent};
        use crate::Handled;

        match event {
            RoutedEvent::Key(KeyEvent { key, .. }) => {
                let delta = match key {
                    Key::Named(NamedKey::Up) | Key::Named(NamedKey::Left) => -1,
                    Key::Named(NamedKey::Down) | Key::Named(NamedKey::Right) => 1,
                    _ => return Handled::Ignored,
                };
                match self.move_selection(delta) {
                    Some(index) => {
                        state.focused_option = Some(index);
                        cx.request_focus(Some(crate::interactive::SubTarget(index as u64)));
                        Handled::Event(RadioGroupEvent::SelectionChanged { index })
                    }
                    None => Handled::Ignored,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => {
                let p = Point::new(layout.bounds.x + local.x, layout.bounds.y + local.y);
                match layout.hit_test(p) {
                    RadioGroupHit::Option(index) => match self.select(index) {
                        Some(event) => {
                            state.focused_option = Some(index);
                            cx.request_focus(Some(crate::interactive::SubTarget(index as u64)));
                            Handled::Event(event)
                        }
                        None => Handled::Ignored,
                    },
                    RadioGroupHit::Outside => Handled::Ignored,
                }
            }
            RoutedEvent::Action(crate::a11y::A11yAction::Click) => {
                match state
                    .focused_option
                    .or(self.selected)
                    .and_then(|i| self.select(i))
                {
                    Some(event) => Handled::Event(event),
                    None => Handled::Ignored,
                }
            }
            RoutedEvent::FocusIn(Some(sub)) => {
                let idx = sub.0 as usize;
                if idx < self.options.len() {
                    state.focused_option = Some(idx);
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            _ => Handled::Ignored,
        }
    }
}

/// The cell-grid painter for [`RadioGroup`], written against
/// [`crate::CellSurface`]. `RadioGroup`'s own `paint_cells` adapts the
/// raw `Buffer` it's called with into that seam via
/// `crate::tui::cell_surface::TuiCellSurface`, the same bridge every
/// `tui` cell-grid painter builds for itself.
///
/// Stacks each [`RadioOption`] one per row, painting `(•) Label` for the
/// selected option and `( ) Label` for the rest.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{RadioGroup, RadioGroupLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;

    fn solid(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `group` into `layout.items` on `surface`.
    ///
    /// `layout` must be the value [`super::cell_radio_group_layout`]
    /// returned for the same group and rect, so paint and
    /// [`RadioGroupLayout::hit_test`] can never disagree.
    pub(crate) fn paint(
        group: &RadioGroup,
        layout: &RadioGroupLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        for (index, option) in group.options.iter().enumerate() {
            let Some(row) = layout.items.get(index) else {
                continue;
            };
            if row.height <= 0.0 || row.width <= 0.0 {
                continue;
            }
            let y = row.y.round() as u16;
            let x0 = row.x.round() as u16;
            let w = row.width.round() as u16;
            let end = x0.saturating_add(w);
            let is_selected = group.selected == Some(index);

            let bg = if is_selected {
                theme.selected_bg
            } else {
                theme.background
            };
            let brk_fg = if option.disabled {
                theme.muted_fg
            } else if is_selected && group.focused {
                theme.accent_fg
            } else {
                theme.border_fg
            };
            let label_fg = if option.disabled {
                theme.muted_fg
            } else {
                theme.foreground
            };

            let glyph = if is_selected { '•' } else { ' ' };
            let text = format!("({glyph}) {}", option.label);
            let last_bracket_idx = 2usize; // '(', glyph, ')' — label starts after

            let mut col = x0;
            for (i, ch) in text.chars().enumerate() {
                if col >= end {
                    break;
                }
                let fg = if i <= last_bracket_idx {
                    brk_fg
                } else {
                    label_fg
                };
                surface.put_char(col, y, ch, solid(fg, bg));
                col += 1;
            }
            while col < end {
                surface.put_char(col, y, ' ', solid(label_fg, bg));
                col += 1;
            }
        }
    }
}

/// `paint` is the shared [`crate::paint_surface::PaintSurface`]
/// implementation of radio-group painting (per-option marker/label),
/// used by backends that have no dedicated native rasteriser for
/// `RadioGroup` — the same minimal-chrome fallback
/// [`crate::primitives::progress`] already uses for `ProgressBar`.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{RadioGroup, RadioGroupLayout, RadioOption, RadioOptionMeasure};
    use crate::backend::PressableVisual;
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a [`RadioGroup`] into `rect` on `surface`, stacking each
    /// option's row using `row_height`, and returning
    /// [`RadioGroupLayout`] for host click dispatch. Tints each
    /// enabled option's marker toward
    /// [`Theme::button_hover_bg`]/[`Theme::button_pressed_bg`] when
    /// that option's [`RadioOption::hovered`]/[`RadioOption::pressed`]
    /// is set — the same [`PressableVisual`] priority (pressed beats
    /// hovered beats resting)
    /// [`crate::primitives::button::native_surface_paint::paint`]
    /// already resolves for [`crate::Button`].
    pub(crate) fn paint(
        group: &RadioGroup,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
        row_height: f32,
    ) -> RadioGroupLayout {
        let layout = group.layout(rect, |_option: &RadioOption| RadioOptionMeasure {
            width: rect.width,
            height: row_height,
        });
        paint_resolved(group, &layout, surface, theme);
        layout
    }

    /// The same paint as [`paint`], against an already-resolved
    /// `layout` — what
    /// [`crate::primitive::Primitive::paint`] calls, since the widget
    /// contract hands a painter the layout rather than a rect.
    pub(crate) fn paint_resolved(
        group: &RadioGroup,
        layout: &RadioGroupLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        for (index, option) in group.options.iter().enumerate() {
            let Some(item) = layout.items.get(index) else {
                continue;
            };
            if item.width <= 0.0 || item.height <= 0.0 {
                continue;
            }

            let selected = group.selected == Some(index);
            let resting_marker = if selected {
                theme.accent_fg
            } else {
                theme.border_fg
            };
            let marker_color = if option.disabled {
                theme.muted_fg
            } else {
                match PressableVisual::resolve(option.pressed, false, option.hovered) {
                    PressableVisual::PressedOrActive => theme.button_pressed_bg(resting_marker),
                    PressableVisual::Hovered => theme.button_hover_bg(resting_marker),
                    PressableVisual::Resting => resting_marker,
                }
            };
            let marker_size = item.height.min(item.width).max(0.0);
            let marker_rect = Rect::new(item.x, item.y, marker_size, marker_size);
            if selected {
                surface.surface_fill_rounded_rect(marker_rect, marker_size / 2.0, marker_color);
            } else {
                surface.surface_stroke_rounded_rect(
                    marker_rect,
                    marker_size / 2.0,
                    marker_color,
                    1.0,
                );
            }

            if !option.label.is_empty() {
                let label_x = item.x + marker_size + 4.0;
                let label_rect = Rect::new(
                    label_x,
                    item.y,
                    (item.x + item.width - label_x).max(0.0),
                    item.height,
                );
                let fg = if option.disabled {
                    theme.muted_fg
                } else {
                    theme.foreground
                };
                surface.surface_draw_text_run(label_rect, &option.label, fg);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Viewport};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every fill/stroke call — enough to assert each
        /// option marker's resting vs. hovered vs. pressed colour
        /// without a Cairo/Core Graphics/Direct2D host.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, f32, Color)>,
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
            fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
                self.fills.push((rect, radius, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
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

        const RECT: Rect = Rect::new(0.0, 0.0, 30.0, 3.0);
        const ROW_HEIGHT: f32 = 1.0;

        fn options() -> Vec<RadioOption> {
            vec![
                RadioOption::new(WidgetId::new("theme:light"), "Light"),
                RadioOption::new(WidgetId::new("theme:dark"), "Dark"),
            ]
        }

        fn group() -> RadioGroup {
            RadioGroup::new(WidgetId::new("settings:theme"), options())
        }

        /// The first option's marker stroke colour (unselected options
        /// stroke, not fill).
        fn first_marker_stroke(surface: &RecordingSurface) -> Color {
            surface
                .strokes
                .first()
                .expect("an unselected option must stroke its marker")
                .2
        }

        #[test]
        fn resting_option_strokes_the_plain_marker() {
            let group = group();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&group, &mut surface, &theme, RECT, ROW_HEIGHT);

            assert_eq!(first_marker_stroke(&surface), theme.border_fg);
        }

        #[test]
        fn hovered_option_tints_its_marker_toward_button_hover_bg() {
            let mut group = group();
            group.options[0].hovered = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&group, &mut surface, &theme, RECT, ROW_HEIGHT);

            assert_eq!(
                first_marker_stroke(&surface),
                theme.button_hover_bg(theme.border_fg)
            );
        }

        #[test]
        fn pressed_option_tints_its_marker_toward_button_pressed_bg() {
            let mut group = group();
            group.options[0].pressed = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&group, &mut surface, &theme, RECT, ROW_HEIGHT);

            assert_eq!(
                first_marker_stroke(&surface),
                theme.button_pressed_bg(theme.border_fg)
            );
        }

        /// A disabled option must not tint even while reported hovered.
        #[test]
        fn disabled_option_does_not_tint_when_hovered() {
            let mut group = group();
            group.options[0].disabled = true;
            group.options[0].hovered = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&group, &mut surface, &theme, RECT, ROW_HEIGHT);

            assert_eq!(first_marker_stroke(&surface), theme.muted_fg);
        }

        /// Hovering one option must not tint a different option's
        /// marker.
        #[test]
        fn hovering_one_option_does_not_tint_another() {
            let mut group = group();
            group.options[1].hovered = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(&group, &mut surface, &theme, RECT, ROW_HEIGHT);

            assert_eq!(first_marker_stroke(&surface), theme.border_fg);
        }
    }
}

/// Emitted by [`RadioGroup::handle_key`]/[`RadioGroup::select`] when the
/// selected option changes. Carries the new index so a host doesn't need
/// to re-read [`RadioGroup::selected`] after applying the event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RadioGroupEvent {
    SelectionChanged { index: usize },
}

impl crate::interactive::WidgetAction for RadioGroupEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            RadioGroupEvent::SelectionChanged { index } => {
                crate::UiEvent::RadioGroupSelectionChanged { widget: id, index }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Vec<RadioOption> {
        vec![
            RadioOption::new(WidgetId::new("theme:light"), "Light"),
            RadioOption::new(WidgetId::new("theme:dark"), "Dark"),
            RadioOption::new(WidgetId::new("theme:auto"), "Auto"),
        ]
    }

    fn group() -> RadioGroup {
        RadioGroup::new(WidgetId::new("settings:theme"), options())
    }

    fn measure(_option: &RadioOption) -> RadioOptionMeasure {
        RadioOptionMeasure {
            width: 10.0,
            height: 1.0,
        }
    }

    #[test]
    fn new_defaults_to_unselected_and_unfocused() {
        let g = group();
        assert_eq!(g.selected, None);
        assert!(!g.focused);
        assert_eq!(g.options.len(), 3);
    }

    #[test]
    fn layout_stacks_options_vertically_and_hit_tests_by_index() {
        let g = group();
        let layout = g.layout(Rect::new(0.0, 0.0, 10.0, 3.0), measure);
        assert_eq!(layout.items.len(), 3);
        assert_eq!(
            layout.hit_test(Point::new(1.0, 0.0)),
            RadioGroupHit::Option(0)
        );
        assert_eq!(
            layout.hit_test(Point::new(1.0, 1.0)),
            RadioGroupHit::Option(1)
        );
        assert_eq!(
            layout.hit_test(Point::new(1.0, 2.0)),
            RadioGroupHit::Option(2)
        );
        assert_eq!(
            layout.hit_test(Point::new(1.0, 5.0)),
            RadioGroupHit::Outside
        );
    }

    #[test]
    fn handle_key_down_seeds_at_first_option_when_unselected() {
        let mut g = group();
        g.focused = true;
        let event = g.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default());
        assert_eq!(event, Some(RadioGroupEvent::SelectionChanged { index: 0 }));
    }

    #[test]
    fn handle_key_up_seeds_at_last_option_when_unselected() {
        let mut g = group();
        g.focused = true;
        let event = g.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default());
        assert_eq!(event, Some(RadioGroupEvent::SelectionChanged { index: 2 }));
    }

    #[test]
    fn handle_key_down_wraps_from_last_to_first() {
        let mut g = group();
        g.focused = true;
        g.selected = Some(2);
        let event = g.handle_key(&Key::Named(NamedKey::Right), &Modifiers::default());
        assert_eq!(event, Some(RadioGroupEvent::SelectionChanged { index: 0 }));
    }

    #[test]
    fn handle_key_up_wraps_from_first_to_last() {
        let mut g = group();
        g.focused = true;
        g.selected = Some(0);
        let event = g.handle_key(&Key::Named(NamedKey::Left), &Modifiers::default());
        assert_eq!(event, Some(RadioGroupEvent::SelectionChanged { index: 2 }));
    }

    #[test]
    fn handle_key_skips_disabled_options() {
        let mut g = group();
        g.focused = true;
        g.options[1].disabled = true;
        g.selected = Some(0);
        let event = g.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default());
        assert_eq!(event, Some(RadioGroupEvent::SelectionChanged { index: 2 }));
    }

    #[test]
    fn handle_key_returns_none_when_all_options_disabled() {
        let mut g = group();
        g.focused = true;
        for option in &mut g.options {
            option.disabled = true;
        }
        g.selected = Some(0);
        assert_eq!(
            g.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_ignores_unrelated_keys() {
        let mut g = group();
        g.focused = true;
        assert_eq!(
            g.handle_key(&Key::Named(NamedKey::Tab), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_not_focused() {
        let g = group();
        assert_eq!(
            g.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn select_fires_for_enabled_option_without_requiring_focus() {
        let g = group();
        assert_eq!(
            g.select(1),
            Some(RadioGroupEvent::SelectionChanged { index: 1 })
        );
    }

    #[test]
    fn select_is_inert_for_disabled_option() {
        let mut g = group();
        g.options[1].disabled = true;
        assert_eq!(g.select(1), None);
    }

    #[test]
    fn select_is_inert_for_out_of_range_index() {
        let g = group();
        assert_eq!(g.select(99), None);
    }

    #[test]
    fn serde_round_trip() {
        let mut g = group();
        g.selected = Some(1);
        g.focused = true;
        let json = serde_json::to_string(&g).unwrap();
        let back: RadioGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn serde_defaults_selected_and_focused_when_omitted() {
        let json = r#"{"id":"settings:theme","options":[]}"#;
        let g: RadioGroup = serde_json::from_str(json).unwrap();
        assert_eq!(g.selected, None);
        assert!(!g.focused);
    }

    #[test]
    fn option_new_defaults_to_enabled_and_untinted() {
        let option = RadioOption::new(WidgetId::new("theme:light"), "Light");
        assert!(!option.disabled);
        assert!(!option.hovered);
        assert!(!option.pressed);
    }

    #[test]
    fn option_serde_round_trip() {
        let mut option = RadioOption::new(WidgetId::new("theme:light"), "Light");
        option.hovered = true;
        option.pressed = true;
        let json = serde_json::to_string(&option).unwrap();
        let back: RadioOption = serde_json::from_str(&json).unwrap();
        assert_eq!(option, back);
    }

    #[test]
    fn option_serde_defaults_hovered_and_pressed_when_omitted() {
        let json = r#"{"id":"theme:light","label":"Light"}"#;
        let option: RadioOption = serde_json::from_str(json).unwrap();
        assert!(!option.hovered);
        assert!(!option.pressed);
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, FocusStop, Interactive, KeyEvent, PointerKind, RoutedEvent,
        SubTarget, WidgetAction,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::{Handled, MouseButton};

    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
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

    fn options() -> Vec<RadioOption> {
        vec![
            RadioOption::new(WidgetId::new("theme:light"), "Light"),
            RadioOption::new(WidgetId::new("theme:dark"), "Dark"),
        ]
    }

    fn group() -> RadioGroup {
        RadioGroup::new(WidgetId::new("settings:theme"), options())
    }

    fn layout_for(g: &RadioGroup, rect: Rect) -> RadioGroupLayout {
        cell_radio_group_layout(g, rect)
    }

    #[test]
    fn declares_one_focus_stop_per_option() {
        let g = group();
        assert_eq!(
            g.focus(),
            FocusSpec::Children(vec![
                FocusStop { sub: SubTarget(0) },
                FocusStop { sub: SubTarget(1) },
            ])
        );
    }

    #[test]
    fn empty_group_declares_no_focus_stops() {
        assert_eq!(RadioGroup::default().focus(), FocusSpec::None);
    }

    #[test]
    fn handle_arrow_key_selects_and_requests_focus_on_that_option() {
        let g = group();
        let layout = layout_for(&g, Rect::new(0.0, 0.0, 10.0, 2.0));
        let mut state = RadioGroupState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(g.id, &mut host);
        let outcome = g.handle(
            &mut state,
            &RoutedEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(RadioGroupEvent::SelectionChanged { index: 0 })
        );
        assert_eq!(state.focused_option, Some(0));
        assert_eq!(host.focus_requests, vec![Some(SubTarget(0))]);
    }

    #[test]
    fn handle_click_on_a_row_selects_it() {
        let g = group();
        let layout = layout_for(&g, Rect::new(0.0, 0.0, 10.0, 2.0));
        let mut state = RadioGroupState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(g.id, &mut host);
        let outcome = g.handle(
            &mut state,
            &RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(1.0, 1.0),
                mods: Modifiers::default(),
            },
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(RadioGroupEvent::SelectionChanged { index: 1 })
        );
        assert_eq!(state.focused_option, Some(1));
    }

    #[test]
    fn a11y_click_action_selects_the_currently_focused_option() {
        let g = group();
        let layout = layout_for(&g, Rect::new(0.0, 0.0, 10.0, 2.0));
        let mut state = RadioGroupState {
            focused_option: Some(1),
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(g.id, &mut host);
        let outcome = g.handle(
            &mut state,
            &RoutedEvent::Action(A11yAction::Click),
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(RadioGroupEvent::SelectionChanged { index: 1 })
        );
    }

    #[test]
    fn focus_in_records_the_focused_option() {
        let g = group();
        let layout = layout_for(&g, Rect::new(0.0, 0.0, 10.0, 2.0));
        let mut state = RadioGroupState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(g.id, &mut host);
        let outcome = g.handle(
            &mut state,
            &RoutedEvent::FocusIn(Some(SubTarget(1))),
            &layout,
            &mut cx,
        );
        assert_eq!(outcome, Handled::Consumed);
        assert_eq!(state.focused_option, Some(1));
    }

    #[test]
    fn primitive_layout_and_paint_cells_round_trip() {
        let mut g = group();
        g.selected = Some(1);
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let rect = Rect::new(0.0, 0.0, 10.0, 2.0);
        let layout = Primitive::layout(&g, &ctx, rect);
        assert_eq!(layout.items.len(), 2);

        #[cfg(feature = "tui")]
        {
            let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 10, 2));
            Primitive::paint_cells(
                &g,
                &layout,
                &mut crate::tui::BufferCellSurface::new(&mut buf),
                &crate::PaintEnv::new(&crate::theme::Theme::default(), &crate::Style::default()),
            );
            let row1: String = (0..10).map(|x| buf[(x, 1)].symbol()).collect();
            assert!(row1.starts_with("(\u{2022}) Dark") || row1.starts_with("(•) Dark"));
        }
    }

    /// `measure`'s reported width must fit the widest label without
    /// truncation on a cell grid: `cell_paint::paint` always writes the
    /// 4-char `"(x) "` chrome before the label, so under-reporting even
    /// one cell of that chrome clips the longer of two differently-sized
    /// labels.
    #[test]
    fn measure_reports_the_widest_labels_chrome_plus_width_on_cells() {
        let g = RadioGroup::new(
            WidgetId::new("settings:theme"),
            vec![
                RadioOption::new(WidgetId::new("theme:a"), "Ab"),
                RadioOption::new(WidgetId::new("theme:b"), "C"),
            ],
        );
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let natural = Primitive::measure(&g, &ctx, crate::primitive::Size::new(0.0, 0.0));
        assert_eq!(
            natural.width,
            Some(4.0 + 2.0),
            "4-cell chrome plus the widest label (\"Ab\", 2 chars)"
        );
        assert_eq!(natural.height, Some(2.0), "one row per option");
    }

    /// An empty group has nothing to size.
    #[test]
    fn measure_reports_no_natural_size_for_an_empty_group() {
        let g = RadioGroup::default();
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let natural = Primitive::measure(&g, &ctx, crate::primitive::Size::new(0.0, 0.0));
        assert_eq!(natural, crate::NaturalSize::NONE);
    }

    /// Three-way event parity's in-crate leg: an [`A11yAction::Click`]
    /// routed through [`crate::a11y::route_widget_action`] produces the
    /// exact same [`crate::UiEvent`] a direct [`Interactive::handle`]
    /// call with the same `RoutedEvent::Action` does. The TUI
    /// mouse-click leg lives in `tests/widget_contract_radio_group.rs`.
    #[test]
    fn route_widget_action_click_matches_direct_interactive_handle() {
        let g = group();
        let id = g.id;
        let rect = Rect::new(0.0, 0.0, 10.0, 2.0);
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(id, rect, Box::new(g.clone()));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let ctx = LayoutCtx::new(1.0, 1.0, false);

        // Focus the second option first, through the same `FocusIn` path
        // the engine's own click/tab routing would drive, so the `Click`
        // action below has a target to resolve against.
        let layout = layout_for(&g, rect);
        {
            let mut host = MockHost::default();
            let mut cx = EventCx::new(id, &mut host);
            let state = states
                .state_for(id, &g)
                .downcast_mut::<RadioGroupState>()
                .unwrap();
            let _ = g.handle(
                state,
                &RoutedEvent::FocusIn(Some(SubTarget(1))),
                &layout,
                &mut cx,
            );
        }

        let events =
            crate::a11y::route_widget_action(&tree, &mut states, &ctx, id, A11yAction::Click);

        let mut state = RadioGroupState {
            focused_option: Some(1),
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(id, &mut host);
        let direct = g.handle(
            &mut state,
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
            vec![RadioGroupEvent::SelectionChanged { index: 1 }.into_ui_event(id)]
        );
    }
}
