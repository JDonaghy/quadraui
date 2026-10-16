//! `ComboBox` primitive: a free-text field backed by a fixed list of
//! options, with a type-ahead filtered dropdown — layout, hit-test,
//! keyboard handling and validation/commit/cancel events. Shared logic
//! only; no backend painter lives here (see `docs/PRIMITIVE_RULES.md`'s
//! cross-backend commitment — a rasteriser follows in a later,
//! backend-scoped issue).
//!
//! Additive alongside [`crate::TextInput`], which is untouched — a
//! `ComboBox` isn't a `TextInput` with a dropdown bolted on, it's its
//! own primitive with its own commit model.
//!
//! # Filtering and placement
//!
//! [`ComboBox::filtered_indices`] is a self-contained, case-insensitive
//! substring match over [`ComboBox::options`] (no shared filter utility
//! exists elsewhere in-crate to call into). [`ComboBox::layout`] reuses
//! [`crate::Completions`]'s placement algorithm directly: it builds a
//! transient [`crate::Completions`] value out of the filtered options
//! and delegates to [`crate::Completions::layout`] for the prefer-below
//! / flip-above / shift-left / clip-to-viewport popup math, embedding
//! the result as [`ComboBoxLayout::dropdown`].
//!
//! # Editing model
//!
//! [`ComboBox::text`] is the live edit buffer (what the field displays
//! while typing); [`ComboBox::committed`] is the last confirmed value.
//! [`ComboBox::handle_key`]'s `Enter` commits — either the currently
//! highlighted dropdown option (when [`ComboBox::open`] and
//! [`ComboBox::highlighted`] is `Some`) or the typed `text` verbatim —
//! and the host applies [`ComboBoxEvent::Committed`] by setting both
//! `committed` and `text` to the event's `value` and closing the
//! dropdown. `Escape` cancels: the host applies
//! [`ComboBoxEvent::Cancelled`] by resetting `text` back to `committed`
//! and closing the dropdown. Typed characters
//! ([`ComboBoxEvent::TextChanged`]) are expected to also open the
//! dropdown — the host sets `open = true` alongside updating `text`.
//! `Up`/`Down` move [`ComboBox::highlighted`] through the *filtered*
//! list with wraparound, mirroring [`crate::RadioGroup`]'s cursor
//! movement.
//!
//! # Focus and activation
//!
//! Like [`crate::Switch`], a `ComboBox` tracks its own
//! [`ComboBox::focused`] flag; [`ComboBox::handle_key`] is gated on
//! `focused` and not [`ComboBox::disabled`]. [`ComboBox::click`] (a
//! dropdown-item activation) is gated on `disabled` alone, matching
//! [`crate::Switch::click`]'s "a pointer click doesn't require prior
//! keyboard focus" contract.

use crate::event::{Key, NamedKey, Point, Rect};
use crate::primitives::completions::{
    CompletionItem, CompletionItemMeasure, CompletionKind, Completions, CompletionsHit,
    CompletionsLayout,
};
use crate::types::{Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of an editable combo box: free text plus a
/// filtered dropdown of fixed options.
///
/// # Examples
///
/// ```
/// use quadraui::{ComboBox, ComboBoxEvent, Key, Modifiers, WidgetId};
///
/// let mut combo = ComboBox::new(
///     WidgetId::new("form:fruit"),
///     vec!["Apple".into(), "Apricot".into(), "Banana".into()],
/// );
/// combo.focused = true;
///
/// let event = combo.handle_key(&Key::Char('a'), &Modifiers::default());
/// assert_eq!(
///     event,
///     Some(ComboBoxEvent::TextChanged { id: combo.id.clone(), text: "a".into() })
/// );
///
/// combo.text = "ap".into();
/// assert_eq!(combo.filtered_indices(), vec![0, 1]);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ComboBox {
    pub id: WidgetId,
    /// Fixed candidate list [`ComboBox::filtered_indices`] matches
    /// against.
    pub options: Vec<String>,
    /// Live edit buffer — what the field currently displays.
    #[serde(default)]
    pub text: String,
    /// Last confirmed value. Only changes via
    /// [`ComboBoxEvent::Committed`] — the host applies it the same way
    /// every other primitive's events are applied.
    #[serde(default)]
    pub committed: String,
    /// Whether the filtered dropdown is currently shown.
    #[serde(default)]
    pub open: bool,
    /// Cursor index into the *filtered* list (from
    /// [`ComboBox::filtered_indices`]), not the raw `options` list.
    #[serde(default)]
    pub highlighted: Option<usize>,
    /// When `true`, every method below returns `None` — same
    /// "disabled means inert, not hidden" contract every other
    /// primitive's `disabled` flag follows.
    #[serde(default)]
    pub disabled: bool,
    /// Whether this field currently holds keyboard focus. Set by the
    /// host's focus-traversal logic; read by
    /// [`ComboBox::handle_key`] to gate typed/arrow-key handling.
    #[serde(default)]
    pub focused: bool,
}

impl ComboBox {
    /// A combo box over `options`, with an empty text buffer, closed
    /// dropdown, enabled and unfocused. Set `text`/`open`/`disabled`/
    /// `focused` directly afterwards (every field is `pub`) rather than
    /// via a `with_*` chain — see `docs/PRIMITIVE_RULES.md`'s rule-8
    /// history of why this crate stopped adding those.
    pub fn new(id: WidgetId, options: Vec<String>) -> Self {
        Self {
            id,
            options,
            text: String::new(),
            committed: String::new(),
            open: false,
            highlighted: None,
            disabled: false,
            focused: false,
        }
    }

    /// Indices into [`ComboBox::options`] that case-insensitively
    /// contain [`ComboBox::text`] as a substring. Empty `text` matches
    /// every option.
    pub fn filtered_indices(&self) -> Vec<usize> {
        if self.text.is_empty() {
            return (0..self.options.len()).collect();
        }
        let needle = self.text.to_lowercase();
        self.options
            .iter()
            .enumerate()
            .filter(|(_, option)| option.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect()
    }

    /// Resolve the field's own bounds plus, when `open`, the dropdown
    /// popup's layout — built by handing the filtered options to a
    /// transient [`Completions`] value and delegating to its
    /// placement algorithm (prefer below the field, flip above on
    /// overflow, shift left on overflow, clip to viewport).
    pub fn layout<F>(
        &self,
        field_bounds: Rect,
        viewport: Rect,
        popup_width: f32,
        max_popup_height: f32,
        measure_item: F,
    ) -> ComboBoxLayout
    where
        F: Fn(usize) -> CompletionItemMeasure,
    {
        let filtered = self.filtered_indices();
        let dropdown = if self.open && !filtered.is_empty() {
            let completions = Completions {
                id: self.id.clone(),
                items: filtered
                    .iter()
                    .map(|&i| CompletionItem {
                        label: StyledText::plain(self.options[i].clone()),
                        detail: None,
                        documentation: None,
                        kind: CompletionKind::Text,
                        icon: None,
                    })
                    .collect(),
                selected_idx: self.highlighted.unwrap_or(0),
                scroll_offset: 0,
                has_focus: self.focused,
            };
            Some(completions.layout(
                field_bounds.x,
                field_bounds.y,
                field_bounds.height,
                viewport,
                popup_width,
                max_popup_height,
                measure_item,
            ))
        } else {
            None
        };
        ComboBoxLayout {
            field_bounds,
            dropdown,
        }
    }

    /// Move [`ComboBox::highlighted`] by `delta` through the filtered
    /// list with wraparound, mirroring
    /// [`crate::RadioGroup`]'s cursor movement: `None` seeds at index 0
    /// moving forward or the last index moving backward, so the first
    /// step lands correctly.
    fn move_highlight(&self, delta: isize) -> Option<usize> {
        let len = self.filtered_indices().len();
        if len == 0 {
            return None;
        }
        let next = match self.highlighted {
            None if delta > 0 => 0,
            None => len - 1,
            Some(i) => (i as isize + delta).rem_euclid(len as isize) as usize,
        };
        Some(next)
    }

    /// `Up`/`Down` move the highlighted dropdown cursor; `Enter`
    /// commits; `Escape` cancels an open edit; digits/letters and
    /// `Backspace` edit `text`. Gated on `focused` and not `disabled`
    /// — any other key is unhandled (`None`).
    pub fn handle_key(&self, key: &Key, _modifiers: &Modifiers) -> Option<ComboBoxEvent> {
        if self.disabled || !self.focused {
            return None;
        }
        match key {
            Key::Named(NamedKey::Down) => {
                let index = self.move_highlight(1)?;
                Some(ComboBoxEvent::HighlightChanged {
                    id: self.id.clone(),
                    index,
                })
            }
            Key::Named(NamedKey::Up) => {
                let index = self.move_highlight(-1)?;
                Some(ComboBoxEvent::HighlightChanged {
                    id: self.id.clone(),
                    index,
                })
            }
            Key::Named(NamedKey::Enter) => Some(self.commit()),
            Key::Named(NamedKey::Escape) => self.open.then(|| ComboBoxEvent::Cancelled {
                id: self.id.clone(),
            }),
            Key::Named(NamedKey::Backspace) => {
                let mut text = self.text.clone();
                text.pop();
                Some(ComboBoxEvent::TextChanged {
                    id: self.id.clone(),
                    text,
                })
            }
            Key::Char(c) => {
                let mut text = self.text.clone();
                text.push(*c);
                Some(ComboBoxEvent::TextChanged {
                    id: self.id.clone(),
                    text,
                })
            }
            _ => None,
        }
    }

    /// Resolve the committed value: the highlighted filtered option
    /// when the dropdown is open and something is highlighted,
    /// otherwise the typed text verbatim.
    fn commit(&self) -> ComboBoxEvent {
        let filtered = self.filtered_indices();
        let value = match self.highlighted.and_then(|i| filtered.get(i)) {
            Some(&option_idx) if self.open => self.options[option_idx].clone(),
            _ => self.text.clone(),
        };
        ComboBoxEvent::Committed {
            id: self.id.clone(),
            value,
        }
    }

    /// Pointer activation dispatcher: the host calls this after
    /// resolving a `MouseDown`/`MouseUp` position through
    /// [`ComboBoxLayout::hit_test`]. A hit on the field itself or
    /// outside the control produces no event — focusing the field is
    /// the host's job, same as elsewhere in this crate.
    pub fn click(&self, hit: ComboBoxHit) -> Option<ComboBoxEvent> {
        if self.disabled {
            return None;
        }
        match hit {
            ComboBoxHit::DropdownItem(index) => {
                let filtered = self.filtered_indices();
                let option_idx = *filtered.get(index)?;
                Some(ComboBoxEvent::Committed {
                    id: self.id.clone(),
                    value: self.options[option_idx].clone(),
                })
            }
            ComboBoxHit::Field | ComboBoxHit::Outside => None,
        }
    }
}

impl Default for ComboBox {
    /// An empty combo box with no options. See [`ComboBox::new`] for one
    /// with a real id/options list.
    fn default() -> Self {
        Self::new(WidgetId::new(""), Vec::new())
    }
}

/// Fully-resolved combo-box layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ComboBoxLayout {
    pub field_bounds: Rect,
    /// `None` when the dropdown is closed or has no matches.
    pub dropdown: Option<CompletionsLayout>,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComboBoxHit {
    Field,
    /// Index into the *filtered* list, not the raw `options` list.
    DropdownItem(usize),
    Outside,
}

impl ComboBoxLayout {
    pub fn hit_test(&self, p: Point) -> ComboBoxHit {
        if let Some(dropdown) = &self.dropdown {
            if let CompletionsHit::Item(idx) = dropdown.hit_test(p.x, p.y) {
                return ComboBoxHit::DropdownItem(idx);
            }
        }
        if self.field_bounds.contains(p) {
            ComboBoxHit::Field
        } else {
            ComboBoxHit::Outside
        }
    }
}

/// Emitted by [`ComboBox`]'s methods. `TextChanged` carries the new
/// edit-buffer text (host also opens the dropdown in response);
/// `HighlightChanged` carries the new filtered-list cursor index;
/// `Committed` carries the resolved final value; `Cancelled` carries
/// only the id — the host reverts `text` to `committed` and closes the
/// dropdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComboBoxEvent {
    TextChanged { id: WidgetId, text: String },
    HighlightChanged { id: WidgetId, index: usize },
    Committed { id: WidgetId, value: String },
    Cancelled { id: WidgetId },
}

/// Shared-paint executor for [`ComboBox`] — a rasteriser calls into
/// this (never re-deriving geometry or re-filtering `options` itself)
/// instead of walking the fields directly, matching every other
/// primitive's `native_surface_paint` module (see
/// `docs/PRIMITIVE_RULES.md`'s "Primitive-first" rule). Gated to the
/// backends that actually paint pixels — the TUI backend renders
/// `ComboBox` through its own cell-grid path instead.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ComboBox, ComboBoxLayout};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a [`ComboBox`] onto `surface`: a bordered text field
    /// showing [`ComboBox::text`], plus — when `layout.dropdown` is
    /// `Some` — the filtered options popup, highlighting whichever
    /// filtered-list index [`ComboBox::highlighted`] names. Item
    /// labels aren't carried on [`crate::primitives::completions::VisibleCompletion`],
    /// so each visible item's `item_idx` (an index into the *filtered*
    /// list, per [`ComboBox::filtered_indices`]'s own contract) is
    /// mapped back through `combo_box.options` here rather than in the
    /// primitive's own layout code, mirroring how [`ComboBox::commit`]
    /// already does that same index chase.
    pub(crate) fn paint(
        combo_box: &ComboBox,
        layout: &ComboBoxLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        let field = layout.field_bounds;
        if field.width > 0.0 && field.height > 0.0 {
            let border = if combo_box.disabled {
                theme.border_fg
            } else if combo_box.focused {
                theme.accent_fg
            } else {
                theme.border_fg
            };
            let fg = if combo_box.disabled {
                theme.muted_fg
            } else {
                theme.surface_fg
            };
            surface.surface_fill_rect(field, theme.input_bg);
            surface.surface_stroke_rect(field, border, 1.0);
            if !combo_box.text.is_empty() {
                surface.surface_draw_text_run(field, &combo_box.text, fg);
            }
        }

        let Some(dropdown) = &layout.dropdown else {
            return;
        };
        if dropdown.bounds.width <= 0.0 || dropdown.bounds.height <= 0.0 {
            return;
        }
        surface.surface_fill_rect(dropdown.bounds, theme.completion_bg);
        surface.surface_stroke_rect(dropdown.bounds, theme.completion_border, 1.0);

        let filtered = combo_box.filtered_indices();
        for visible in &dropdown.visible_items {
            if combo_box.highlighted == Some(visible.item_idx) {
                surface.surface_fill_rect(visible.bounds, theme.completion_selected_bg);
            }
            if let Some(&option_idx) = filtered.get(visible.item_idx) {
                surface.surface_draw_text_run(
                    visible.bounds,
                    &combo_box.options[option_idx],
                    theme.completion_fg,
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Rect, Viewport};
        use crate::primitives::completions::CompletionItemMeasure;
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every surface verb `ComboBox`'s paint uses — mirrors
        /// `primitives::command_line`'s identical test double, so this
        /// test runs on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color)>,
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
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, _stroke_width: f32) {
                self.strokes.push((rect, color));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: Point,
                _to: Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn options() -> Vec<String> {
            vec!["Apple".into(), "Apricot".into(), "Banana".into()]
        }

        fn field_bounds() -> Rect {
            Rect::new(0.0, 0.0, 20.0, 1.0)
        }

        fn viewport() -> Rect {
            Rect::new(0.0, 0.0, 80.0, 24.0)
        }

        #[test]
        fn paints_field_background_border_and_text() {
            let mut c = ComboBox::new(WidgetId::new("form:fruit"), options());
            c.text = "Cher".into();
            let layout = c.layout(field_bounds(), viewport(), 20.0, 10.0, |_| {
                CompletionItemMeasure::new(1.0)
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);

            assert_eq!(surface.fills[0], (field_bounds(), theme.input_bg));
            assert!(surface.strokes.contains(&(field_bounds(), theme.border_fg)));
            assert_eq!(surface.text_runs[0].1, "Cher");
        }

        #[test]
        fn closed_dropdown_paints_no_popup_chrome() {
            let c = ComboBox::new(WidgetId::new("form:fruit"), options());
            let layout = c.layout(field_bounds(), viewport(), 20.0, 10.0, |_| {
                CompletionItemMeasure::new(1.0)
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);

            assert!(!surface
                .fills
                .iter()
                .any(|(_, color)| *color == theme.completion_bg));
        }

        #[test]
        fn open_dropdown_paints_every_filtered_option_label() {
            let mut c = ComboBox::new(WidgetId::new("form:fruit"), options());
            c.open = true;
            c.text = "ap".into(); // filtered: Apple, Apricot
            let layout = c.layout(field_bounds(), viewport(), 20.0, 10.0, |_| {
                CompletionItemMeasure::new(1.0)
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);

            assert!(surface
                .fills
                .iter()
                .any(|(_, color)| *color == theme.completion_bg));
            let labels: Vec<&str> = surface
                .text_runs
                .iter()
                .map(|(_, text, _)| text.as_str())
                .collect();
            assert!(labels.contains(&"Apple"));
            assert!(labels.contains(&"Apricot"));
            assert!(!labels.contains(&"Banana"));
        }

        #[test]
        fn highlighted_item_gets_a_selection_fill() {
            let mut c = ComboBox::new(WidgetId::new("form:fruit"), options());
            c.open = true;
            c.text = "ap".into(); // filtered: [0 Apple, 1 Apricot]
            c.highlighted = Some(1);
            let layout = c.layout(field_bounds(), viewport(), 20.0, 10.0, |_| {
                CompletionItemMeasure::new(1.0)
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);

            let highlighted_bounds = layout.dropdown.as_ref().unwrap().visible_items[1].bounds;
            assert!(surface
                .fills
                .contains(&(highlighted_bounds, theme.completion_selected_bg)));
            let unhighlighted_bounds = layout.dropdown.as_ref().unwrap().visible_items[0].bounds;
            assert!(!surface
                .fills
                .contains(&(unhighlighted_bounds, theme.completion_selected_bg)));
        }

        #[test]
        fn zero_size_field_paints_nothing_for_the_field() {
            let c = ComboBox::new(WidgetId::new("form:fruit"), options());
            let layout = c.layout(
                Rect::new(0.0, 0.0, 0.0, 0.0),
                viewport(),
                20.0,
                10.0,
                |_| CompletionItemMeasure::new(1.0),
            );
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&c, &layout, &mut surface, &theme);
            assert!(surface.fills.is_empty());
            assert!(surface.strokes.is_empty());
            assert!(surface.text_runs.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Vec<String> {
        vec!["Apple".into(), "Apricot".into(), "Banana".into()]
    }

    fn combo() -> ComboBox {
        ComboBox::new(WidgetId::new("form:fruit"), options())
    }

    #[test]
    fn new_starts_empty_closed_enabled_unfocused() {
        let c = combo();
        assert_eq!(c.text, "");
        assert_eq!(c.committed, "");
        assert!(!c.open);
        assert_eq!(c.highlighted, None);
        assert!(!c.disabled);
        assert!(!c.focused);
    }

    #[test]
    fn filtered_indices_matches_case_insensitive_substring() {
        let mut c = combo();
        assert_eq!(c.filtered_indices(), vec![0, 1, 2]);

        c.text = "ap".into();
        assert_eq!(c.filtered_indices(), vec![0, 1]);

        c.text = "AP".into();
        assert_eq!(c.filtered_indices(), vec![0, 1]);

        c.text = "nana".into();
        assert_eq!(c.filtered_indices(), vec![2]);

        c.text = "zzz".into();
        assert_eq!(c.filtered_indices(), Vec::<usize>::new());
    }

    #[test]
    fn typed_chars_accumulate_in_text_when_focused() {
        let mut c = combo();
        c.focused = true;

        let e1 = c.handle_key(&Key::Char('a'), &Modifiers::default());
        assert_eq!(
            e1,
            Some(ComboBoxEvent::TextChanged {
                id: c.id.clone(),
                text: "a".into()
            })
        );
        c.text = "a".into();

        let e2 = c.handle_key(&Key::Char('p'), &Modifiers::default());
        assert_eq!(
            e2,
            Some(ComboBoxEvent::TextChanged {
                id: c.id.clone(),
                text: "ap".into()
            })
        );
    }

    #[test]
    fn backspace_pops_the_last_character() {
        let mut c = combo();
        c.focused = true;
        c.text = "ap".into();
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Backspace), &Modifiers::default()),
            Some(ComboBoxEvent::TextChanged {
                id: c.id.clone(),
                text: "a".into()
            })
        );
    }

    #[test]
    fn down_and_up_move_the_highlight_through_the_filtered_list_with_wraparound() {
        let mut c = combo();
        c.focused = true;
        c.text = "ap".into(); // filtered: [0, 1]

        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 0
            })
        );
        c.highlighted = Some(0);

        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 1
            })
        );
        c.highlighted = Some(1);

        // Wraps back to 0.
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 0
            })
        );

        c.highlighted = Some(0);
        // Up from 0 wraps to the last filtered index.
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            Some(ComboBoxEvent::HighlightChanged {
                id: c.id.clone(),
                index: 1
            })
        );
    }

    #[test]
    fn navigation_is_inert_when_the_filtered_list_is_empty() {
        let mut c = combo();
        c.focused = true;
        c.text = "zzz".into();
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn enter_commits_the_highlighted_option_when_open() {
        let mut c = combo();
        c.focused = true;
        c.open = true;
        c.text = "ap".into();
        c.highlighted = Some(1); // "Apricot"
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            Some(ComboBoxEvent::Committed {
                id: c.id.clone(),
                value: "Apricot".into()
            })
        );
    }

    #[test]
    fn enter_commits_typed_text_verbatim_when_nothing_is_highlighted() {
        let mut c = combo();
        c.focused = true;
        c.text = "Cherry".into();
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            Some(ComboBoxEvent::Committed {
                id: c.id.clone(),
                value: "Cherry".into()
            })
        );
    }

    #[test]
    fn escape_cancels_an_open_edit_and_is_inert_otherwise() {
        let mut c = combo();
        c.focused = true;
        c.open = true;
        c.text = "ap".into();
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Escape), &Modifiers::default()),
            Some(ComboBoxEvent::Cancelled { id: c.id.clone() })
        );

        c.open = false;
        assert_eq!(
            c.handle_key(&Key::Named(NamedKey::Escape), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_not_focused_or_disabled() {
        let c = combo();
        assert_eq!(c.handle_key(&Key::Char('a'), &Modifiers::default()), None);

        let mut c = combo();
        c.focused = true;
        c.disabled = true;
        assert_eq!(c.handle_key(&Key::Char('a'), &Modifiers::default()), None);
    }

    #[test]
    fn click_on_dropdown_item_commits_without_requiring_focus() {
        let mut c = combo();
        c.text = "ap".into(); // filtered: [0, 1]
        assert_eq!(
            c.click(ComboBoxHit::DropdownItem(1)),
            Some(ComboBoxEvent::Committed {
                id: c.id.clone(),
                value: "Apricot".into()
            })
        );
        assert_eq!(c.click(ComboBoxHit::Field), None);
        assert_eq!(c.click(ComboBoxHit::Outside), None);
    }

    #[test]
    fn click_is_inert_when_disabled() {
        let mut c = combo();
        c.disabled = true;
        assert_eq!(c.click(ComboBoxHit::DropdownItem(0)), None);
    }

    #[test]
    fn layout_has_no_dropdown_when_closed_and_a_dropdown_when_open() {
        let c = combo();
        let field_bounds = Rect::new(0.0, 0.0, 20.0, 1.0);
        let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);

        let closed = c.layout(field_bounds, viewport, 20.0, 10.0, |_| {
            CompletionItemMeasure::new(1.0)
        });
        assert!(closed.dropdown.is_none());
        assert_eq!(closed.hit_test(Point::new(5.0, 0.0)), ComboBoxHit::Field);

        let mut open = c;
        open.open = true;
        let layout = open.layout(field_bounds, viewport, 20.0, 10.0, |_| {
            CompletionItemMeasure::new(1.0)
        });
        assert!(layout.dropdown.is_some());
        // The dropdown is placed just below the field (field height 1.0).
        assert_eq!(
            layout.hit_test(Point::new(1.0, 1.0)),
            ComboBoxHit::DropdownItem(0)
        );
    }

    #[test]
    fn layout_has_no_dropdown_when_open_but_no_matches() {
        let mut c = combo();
        c.open = true;
        c.text = "zzz".into();
        let field_bounds = Rect::new(0.0, 0.0, 20.0, 1.0);
        let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
        let layout = c.layout(field_bounds, viewport, 20.0, 10.0, |_| {
            CompletionItemMeasure::new(1.0)
        });
        assert!(layout.dropdown.is_none());
    }

    #[test]
    fn serde_round_trip() {
        let mut c = combo();
        c.text = "ap".into();
        c.open = true;
        c.highlighted = Some(1);
        c.focused = true;
        let json = serde_json::to_string(&c).unwrap();
        let back: ComboBox = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn serde_defaults_optional_fields_when_omitted() {
        let json = r#"{"id":"form:fruit","options":["Apple","Banana"]}"#;
        let c: ComboBox = serde_json::from_str(json).unwrap();
        assert_eq!(c.text, "");
        assert_eq!(c.committed, "");
        assert!(!c.open);
        assert_eq!(c.highlighted, None);
        assert!(!c.disabled);
        assert!(!c.focused);
    }
}
