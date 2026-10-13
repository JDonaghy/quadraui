//! `RadioGroup` primitive: a standalone set of mutually-exclusive
//! options — layout, hit-test, keyboard focus/activation (arrow-key
//! wraparound cursor), and its event. Shared logic only; no backend
//! painter lives here (see `docs/PRIMITIVE_RULES.md`'s cross-backend
//! commitment — a rasteriser follows in a later, backend-scoped issue).
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
//! [`crate::focus::FocusManager`] for the app-wide owner of "what has
//! focus").

use crate::event::{Key, NamedKey, Point, Rect};
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// A single option within a [`RadioGroup`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadioOption {
    pub id: WidgetId,
    pub label: String,
    #[serde(default)]
    pub disabled: bool,
}

impl RadioOption {
    pub fn new(id: WidgetId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            disabled: false,
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

/// Emitted by [`RadioGroup::handle_key`]/[`RadioGroup::select`] when the
/// selected option changes. Carries the new index so a host doesn't need
/// to re-read [`RadioGroup::selected`] after applying the event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RadioGroupEvent {
    SelectionChanged { index: usize },
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
}
