//! [`TextField`] — standalone single-line framed text field.
//!
//! The Finder-style search box shape: one bracketed row with an
//! optional leading icon and an optional clear (×) button, placeable
//! anywhere (a title-bar toolbar, above a list) rather than only as a
//! [`crate::Form`] row. Field semantics (`value`/`placeholder`/`cursor`/
//! `selection_anchor`/`has_focus`) match
//! [`crate::primitives::form::FieldKind::TextInput`] exactly, so a
//! consumer migrating a Form row to a standalone field changes no
//! editing logic.
//!
//! ## Editing — reuses [`EditOp`]/[`TextEditor`], no second editor
//!
//! [`TextField::apply_edit_op`] routes through
//! [`crate::primitives::text_input::apply_flat_edit_op`] — the same flat
//! `value` + byte-offset `cursor`/`selection_anchor` bridge
//! `FormController` uses for `FieldKind::TextInput`/`TextArea`/
//! `PasswordInput` — instead of re-deriving cursor/selection math here.
//!
//! ## Layout — horizontal scroll keeps the cursor visible
//!
//! Unlike `Form`'s `TextInput` row (which truncates at the control
//! column's width with no scroll), [`TextField::layout`] computes a
//! horizontal scroll offset, in characters, so the cursor never scrolls
//! out of view. [`Self::layout`]'s `rect` → [`TextFieldLayout`] is a
//! pure function of [`TextFieldMeasure`] (itself derived from
//! `Backend::char_width()`/`Backend::line_height()`) and this
//! primitive's own state — backend-neutral, per
//! `PRIMITIVE_RULES.md`'s "primitive-first" rule.
//!
//! ## Degrading under width pressure
//!
//! When `rect` is too narrow for everything requested, [`Self::layout`]
//! drops the clear button first, then the leading icon, before ever
//! shrinking the text column to zero — see [`Self::layout`]'s doc.

use serde::{Deserialize, Serialize};

use crate::event::{Point, Rect};
use crate::primitives::text_input::{apply_flat_edit_op, EditOp};
use crate::types::{Icon, WidgetId};

/// Standalone single-line framed text field. See the module doc.
///
/// `#[non_exhaustive]`, builder methods (`with_*`) plus `new`/`Default`,
/// same convention as [`crate::TextInput`] — see that type's doc for why
/// this shape never grows a `pub` field a consumer's exhaustive literal
/// could break on.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextField {
    pub id: WidgetId,
    /// Current text. Typing mutates it via [`Self::apply_edit_op`].
    #[serde(default)]
    pub value: String,
    /// Shown, dimmed, in place of an empty [`Self::value`].
    #[serde(default)]
    pub placeholder: String,
    /// Byte offset into [`Self::value`]. `Some` renders a caret there;
    /// `None` renders the field read-only (no caret).
    #[serde(default)]
    pub cursor: Option<usize>,
    /// Byte offset into [`Self::value`]. When `Some` and different from
    /// [`Self::cursor`], the range between them paints with a selection
    /// highlight.
    #[serde(default)]
    pub selection_anchor: Option<usize>,
    /// Whether the field has keyboard focus — same convention as
    /// [`crate::Button::focused`]: carries no "how did focus arrive"
    /// information.
    #[serde(default)]
    pub has_focus: bool,
    /// Icon painted inside the left bracket, before the text — e.g. a
    /// magnifier for a search field. `None` paints no icon.
    #[serde(default)]
    pub leading_icon: Option<Icon>,
    /// Whether a clear (×) button shows when [`Self::value`] is
    /// non-empty. Hit-tested as [`TextFieldHit::Clear`].
    #[serde(default)]
    pub clearable: bool,
}

impl TextField {
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            value: String::new(),
            placeholder: String::new(),
            cursor: None,
            selection_anchor: None,
            has_focus: false,
            leading_icon: None,
            clearable: false,
        }
    }

    /// Set [`Self::value`].
    #[must_use]
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    /// Set [`Self::placeholder`].
    #[must_use]
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set [`Self::cursor`].
    #[must_use]
    pub fn with_cursor(mut self, cursor: Option<usize>) -> Self {
        self.cursor = cursor;
        self
    }

    /// Set [`Self::selection_anchor`].
    #[must_use]
    pub fn with_selection_anchor(mut self, selection_anchor: Option<usize>) -> Self {
        self.selection_anchor = selection_anchor;
        self
    }

    /// Set [`Self::has_focus`].
    #[must_use]
    pub fn with_has_focus(mut self, has_focus: bool) -> Self {
        self.has_focus = has_focus;
        self
    }

    /// Set [`Self::leading_icon`].
    #[must_use]
    pub fn with_leading_icon(mut self, icon: Icon) -> Self {
        self.leading_icon = Some(icon);
        self
    }

    /// Set [`Self::clearable`].
    #[must_use]
    pub fn with_clearable(mut self, clearable: bool) -> Self {
        self.clearable = clearable;
        self
    }

    /// Apply one [`EditOp`] (insert/delete/move/select), mutating
    /// [`Self::value`]/[`Self::cursor`]/[`Self::selection_anchor`] in
    /// place via the shared flat-value bridge
    /// [`crate::primitives::text_input::apply_flat_edit_op`] — see the
    /// module doc's "Editing" section. Returns whether anything changed
    /// (same convention as [`crate::TextEditor::apply`]).
    pub fn apply_edit_op(&mut self, op: EditOp) -> bool {
        let cursor = self
            .cursor
            .unwrap_or(self.value.len())
            .min(self.value.len());
        let (new_value, new_cursor, new_anchor) =
            apply_flat_edit_op(&self.value, cursor, self.selection_anchor, op);
        let changed = new_value != self.value
            || Some(new_cursor) != self.cursor
            || new_anchor != self.selection_anchor;
        self.value = new_value;
        self.cursor = Some(new_cursor);
        self.selection_anchor = new_anchor;
        changed
    }

    /// Clear [`Self::value`] and reset the cursor/selection — what a
    /// click on [`TextFieldHit::Clear`] does.
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = Some(0);
        self.selection_anchor = None;
    }

    /// Compute layout for `rect`. `measure` supplies char width / row
    /// height in the backend's native units.
    ///
    /// Reserves one column each for the opening/closing bracket, one
    /// for the leading icon (when [`Self::leading_icon`] is `Some`), and
    /// one for the clear button (when [`Self::clearable`] and
    /// [`Self::value`] is non-empty). If `rect` can't fit everything
    /// requested plus at least one text column, the clear button is
    /// dropped first, then the icon — the text column is never dropped
    /// to zero while any room exists at all.
    ///
    /// [`TextFieldLayout::resolved_scroll_chars`] is the number of
    /// leading characters of [`Self::value`] scrolled out of view,
    /// chosen so [`Self::cursor`] always lands inside
    /// [`TextFieldLayout::text_rect`].
    pub fn layout(&self, rect: Rect, measure: TextFieldMeasure) -> TextFieldLayout {
        let char_w = measure.char_width.max(1.0);
        let row_h = measure.line_height.max(1.0);
        let total_cols = ((rect.width / char_w).floor().max(0.0)) as usize;

        let want_icon = self.leading_icon.is_some();
        let want_clear = self.clearable && !self.value.is_empty();

        let reserved =
            |icon: bool, clear: bool| -> usize { 2 + usize::from(icon) + usize::from(clear) };
        let mut show_icon = want_icon;
        let mut show_clear = want_clear;
        if show_clear && total_cols.saturating_sub(reserved(show_icon, show_clear)) < 1 {
            show_clear = false;
        }
        if show_icon && total_cols.saturating_sub(reserved(show_icon, show_clear)) < 1 {
            show_icon = false;
        }
        let text_cols = total_cols.saturating_sub(reserved(show_icon, show_clear));

        // Char index of the cursor (bytes → chars, clamped/snapped to a
        // char boundary so a bogus app-supplied offset can't panic).
        let cursor_byte = crate::text_util::snap_to_char_boundary(
            &self.value,
            self.cursor.unwrap_or(0).min(self.value.len()),
        );
        let cursor_char = self.value[..cursor_byte].chars().count();

        let scroll = if text_cols == 0 || cursor_char < text_cols {
            0
        } else {
            cursor_char + 1 - text_cols
        };

        let mut col = 1.0; // after the opening bracket
        let icon_rect = if show_icon {
            let r = Rect::new(rect.x + col * char_w, rect.y, char_w, row_h);
            col += 1.0;
            Some(r)
        } else {
            None
        };
        let text_rect = Rect::new(
            rect.x + col * char_w,
            rect.y,
            text_cols as f32 * char_w,
            row_h,
        );
        col += text_cols as f32;
        // The column after the clear button (if any) is the closing
        // bracket's own, not its own hit region, so `col` is never read
        // again past this point.
        let clear_rect = if show_clear {
            Some(Rect::new(rect.x + col * char_w, rect.y, char_w, row_h))
        } else {
            None
        };

        let cursor_bounds = self.cursor.and_then(|_| {
            if text_cols == 0 {
                return None;
            }
            let rel = cursor_char.checked_sub(scroll)?;
            if rel >= text_cols {
                return None;
            }
            Some(Rect::new(
                text_rect.x + rel as f32 * char_w,
                text_rect.y,
                char_w,
                row_h,
            ))
        });

        // Hit region for "text": everything between the brackets that
        // isn't the clear button — clicking the icon still focuses/
        // positions the caret, same as clicking the text itself.
        let text_hit_start = icon_rect.map_or(text_rect.x, |r| r.x);
        let text_hit_width = text_rect.x + text_rect.width - text_hit_start;
        let mut hit_regions = vec![(
            Rect::new(text_hit_start, rect.y, text_hit_width.max(0.0), row_h),
            TextFieldHit::Text,
        )];
        if let Some(r) = clear_rect {
            hit_regions.push((r, TextFieldHit::Clear));
        }

        TextFieldLayout {
            bounds: rect,
            icon_rect,
            text_rect,
            clear_rect,
            cursor_bounds,
            resolved_scroll_chars: scroll,
            hit_regions,
        }
    }
}

/// Same shape as [`TextField::new`] with an empty `WidgetId` — the
/// `new(required…)`/`with_*`/`Default` trio every exhaustive-literal-free
/// primitive in this crate uses.
impl Default for TextField {
    fn default() -> Self {
        Self::new(WidgetId::new(String::new()))
    }
}

/// Click-routing result from [`TextFieldLayout::hit_test`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFieldHit {
    /// Landed on the text column (or the icon, which focuses/positions
    /// the caret the same way clicking the text does).
    Text,
    /// Landed on the clear (×) button.
    Clear,
    /// Landed outside every region — typically outside `bounds`
    /// entirely, since [`TextField::layout`] covers everything between
    /// the brackets with [`Self::Text`]/[`Self::Clear`].
    Miss,
}

/// Fully-resolved layout from [`TextField::layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct TextFieldLayout {
    /// Outer bounds (matches the `rect` argument to `layout`).
    pub bounds: Rect,
    /// Leading icon's cell, when shown.
    pub icon_rect: Option<Rect>,
    /// Text column — the window into `value` currently visible, after
    /// [`Self::resolved_scroll_chars`] characters have scrolled off its
    /// left edge.
    pub text_rect: Rect,
    /// Clear (×) button's cell, when shown.
    pub clear_rect: Option<Rect>,
    /// Caret bounds when the cursor is inside the visible text window,
    /// otherwise `None`.
    pub cursor_bounds: Option<Rect>,
    /// Characters of `value` scrolled out of view on the left, chosen
    /// so the cursor stays inside [`Self::text_rect`]. Rasterisers slice
    /// `value.chars().skip(resolved_scroll_chars)` before painting.
    pub resolved_scroll_chars: usize,
    /// Per-region hit map for click routing.
    pub hit_regions: Vec<(Rect, TextFieldHit)>,
}

impl TextFieldLayout {
    /// Hit-test a click at surface-native coordinates `(x, y)`.
    ///
    /// Coordinate frame: **ABSOLUTE** — `bounds`/`text_rect`/
    /// `hit_regions` all carry `rect.x`/`rect.y` from the `rect` passed
    /// to [`TextField::layout`]. Returns [`TextFieldHit::Miss`] when
    /// `(x, y)` falls outside every region.
    pub fn hit_test(&self, x: f32, y: f32) -> TextFieldHit {
        for (rect, hit) in &self.hit_regions {
            if rect.contains(Point::new(x, y)) {
                return *hit;
            }
        }
        TextFieldHit::Miss
    }
}

/// Per-field measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextFieldMeasure {
    /// Width of one character column in surface-native units (TUI:
    /// `1.0` cells; GTK/macOS/Win: the monospace advance).
    pub char_width: f32,
    /// Height of the field's one row (TUI: `1.0` cells; GTK/macOS/Win:
    /// pixels).
    pub line_height: f32,
}

impl TextFieldMeasure {
    pub fn new(char_width: f32, line_height: f32) -> Self {
        Self {
            char_width,
            line_height,
        }
    }

    /// Build from the backend's own [`crate::backend::Metrics`]
    /// (`backend.measure()`), same convention as
    /// [`crate::TextInputMeasure::from_metrics`].
    pub fn from_metrics(m: &crate::backend::Metrics) -> Self {
        Self::new(m.char_width, m.line_height)
    }
}

/// Paint a [`TextField`] onto a pixel [`crate::paint_surface::PaintSurface`]
/// at its already-resolved `layout` — the GTK/macOS/Win sibling of
/// [`crate::tui::text_field::draw_text_field`]'s one-row
/// `[ icon value × ]` painter. A stroked box around the whole `rect`
/// stands in for the TUI brackets; inside it, an optional leading-icon
/// glyph, the value/placeholder/selection/caret run (through
/// [`crate::primitives::form::paint_text_run_with_caret`] — the same
/// routine `Form`'s own `TextInput` row paints with, so the two can
/// never carry separate copies of that math), and an optional clear
/// (×) button glyph.
///
/// [`TextFieldLayout::resolved_scroll_chars`] characters are scrolled
/// off `tf.value`'s left edge before painting, same as the TUI painter;
/// `tf.cursor`/`tf.selection_anchor` are re-based onto that scrolled
/// slice's own byte offsets since `paint_text_run_with_caret` has no
/// scroll concept of its own (`Form`'s row never scrolls, so it always
/// passes the unscrolled value).
// `#[allow(dead_code)]`: same reasoning as `primitives::text_input`'s own
// `native_surface_paint` module doc — a `--features win` build on a
// non-Windows host gates `WinBackend::draw_text_field`'s only call site
// behind `target_os = "windows"`, leaving no reachable caller here.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) fn paint_text_field_gui(
    surface: &mut dyn crate::paint_surface::PaintSurface,
    rect: Rect,
    layout: &TextFieldLayout,
    tf: &TextField,
    theme: &crate::theme::Theme,
    nerd_fonts_enabled: bool,
) {
    use crate::primitives::form::paint_text_run_with_caret;

    surface.surface_fill_rect(rect, theme.input_bg);

    let field_fg = theme.foreground;
    let dim_fg = theme.muted_fg;
    let border_fg = if tf.has_focus {
        theme.accent_fg
    } else {
        dim_fg
    };
    surface.surface_stroke_rect(rect, border_fg, 1.0);

    if let Some(icon_rect) = layout.icon_rect {
        if let Some(icon) = &tf.leading_icon {
            let glyph = icon.resolve(nerd_fonts_enabled);
            surface.surface_draw_text_run(icon_rect, glyph, dim_fg);
        }
    }

    let scroll_byte = tf
        .value
        .char_indices()
        .nth(layout.resolved_scroll_chars)
        .map(|(i, _)| i)
        .unwrap_or(tf.value.len());
    let scrolled_value = &tf.value[scroll_byte..];
    let cursor = tf.cursor.map(|c| c.saturating_sub(scroll_byte));
    let selection_anchor = tf.selection_anchor.map(|a| a.saturating_sub(scroll_byte));

    paint_text_run_with_caret(
        surface,
        rect,
        layout.text_rect.x,
        layout.text_rect.width,
        scrolled_value,
        &tf.placeholder,
        cursor,
        selection_anchor,
        field_fg,
        dim_fg,
        theme.selection_bg,
        theme.accent_fg,
        false,
        '\0',
    );

    if let Some(clear_rect) = layout.clear_rect {
        surface.surface_draw_text_run(clear_rect, "\u{00D7}", dim_fg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure() -> TextFieldMeasure {
        TextFieldMeasure::new(1.0, 1.0)
    }

    #[test]
    fn empty_field_shows_placeholder_bounds_with_no_icon_or_clear() {
        let field = TextField::new(WidgetId::new("f")).with_placeholder("search");
        let layout = field.layout(Rect::new(0.0, 0.0, 10.0, 1.0), measure());
        assert!(layout.icon_rect.is_none());
        assert!(layout.clear_rect.is_none());
        // 10 cols - 2 brackets = 8 text cols.
        assert_eq!(layout.text_rect.width, 8.0);
        assert_eq!(layout.resolved_scroll_chars, 0);
    }

    #[test]
    fn clearable_with_value_reserves_a_clear_column() {
        let field = TextField::new(WidgetId::new("f"))
            .with_value("hi")
            .with_clearable(true);
        let layout = field.layout(Rect::new(0.0, 0.0, 10.0, 1.0), measure());
        assert!(layout.clear_rect.is_some());
        // 10 - 2 brackets - 1 clear = 7 text cols.
        assert_eq!(layout.text_rect.width, 7.0);
    }

    #[test]
    fn narrow_rect_drops_clear_before_icon() {
        let icon = Icon::new("S", "S");
        let field = TextField::new(WidgetId::new("f"))
            .with_value("hi")
            .with_clearable(true)
            .with_leading_icon(icon);
        // 5 cols: 2 brackets + 1 icon + 1 clear leaves exactly 1 text
        // col — still fits both. Narrow it by one more to force a drop.
        let layout = field.layout(Rect::new(0.0, 0.0, 4.0, 1.0), measure());
        assert!(layout.clear_rect.is_none(), "clear should drop first");
        assert!(layout.icon_rect.is_some(), "icon should still show");
    }

    #[test]
    fn everything_drops_when_rect_is_too_narrow_even_for_icon() {
        let icon = Icon::new("S", "S");
        let field = TextField::new(WidgetId::new("f"))
            .with_value("hi")
            .with_clearable(true)
            .with_leading_icon(icon);
        let layout = field.layout(Rect::new(0.0, 0.0, 3.0, 1.0), measure());
        assert!(layout.clear_rect.is_none());
        assert!(layout.icon_rect.is_none());
        assert_eq!(layout.text_rect.width, 1.0);
    }

    #[test]
    fn cursor_past_the_visible_window_scrolls_into_view() {
        let field = TextField::new(WidgetId::new("f"))
            .with_value("0123456789")
            .with_cursor(Some(10))
            .with_has_focus(true);
        // 10 cols - 2 brackets = 8 text cols; cursor at char 10 needs
        // scroll = 10 + 1 - 8 = 3.
        let layout = field.layout(Rect::new(0.0, 0.0, 10.0, 1.0), measure());
        assert_eq!(layout.resolved_scroll_chars, 3);
        assert!(layout.cursor_bounds.is_some());
    }

    /// Non-zero-origin regression per `PRIMITIVE_RULES.md`'s coordinate-
    /// frame table: every rect in the layout must carry `rect.x`/
    /// `rect.y`, not just the width/height math above.
    #[test]
    fn layout_is_absolute_not_local() {
        let icon = Icon::new("S", "S");
        let field = TextField::new(WidgetId::new("f"))
            .with_value("hi")
            .with_clearable(true)
            .with_leading_icon(icon);
        let layout = field.layout(Rect::new(5.0, 2.0, 10.0, 1.0), measure());
        assert_eq!(layout.icon_rect.unwrap().x, 6.0);
        assert_eq!(layout.text_rect.x, 7.0);
        assert_eq!(layout.text_rect.y, 2.0);
        assert_eq!(layout.clear_rect.unwrap().y, 2.0);
        for (rect, _) in &layout.hit_regions {
            assert!(rect.x >= 5.0);
            assert!(rect.y >= 2.0);
        }
    }

    #[test]
    fn hit_test_routes_clear_and_text_and_misses_outside() {
        let field = TextField::new(WidgetId::new("f"))
            .with_value("hi")
            .with_clearable(true);
        let layout = field.layout(Rect::new(0.0, 0.0, 10.0, 1.0), measure());
        let clear = layout.clear_rect.unwrap();
        assert_eq!(
            layout.hit_test(clear.x + 0.1, clear.y + 0.1),
            TextFieldHit::Clear
        );
        assert_eq!(layout.hit_test(2.0, 0.0), TextFieldHit::Text);
        assert_eq!(layout.hit_test(-1.0, 0.0), TextFieldHit::Miss);
    }

    #[test]
    fn apply_edit_op_inserts_without_a_second_editor() {
        let mut field = TextField::new(WidgetId::new("f")).with_cursor(Some(0));
        let changed = field.apply_edit_op(EditOp::InsertChar('x'));
        assert!(changed);
        assert_eq!(field.value, "x");
        assert_eq!(field.cursor, Some(1));
    }

    #[test]
    fn clear_resets_value_and_cursor() {
        let mut field = TextField::new(WidgetId::new("f")).with_value("hi");
        field.clear();
        assert_eq!(field.value, "");
        assert_eq!(field.cursor, Some(0));
        assert_eq!(field.selection_anchor, None);
    }
}
