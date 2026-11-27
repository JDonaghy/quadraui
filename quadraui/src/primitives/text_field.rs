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
use crate::Handled;

/// Standalone single-line framed text field. See the module doc.
///
/// `#[non_exhaustive]`, builder methods (`with_*`) plus `new`/`Default`,
/// same convention as [`crate::TextInput`] — see that type's doc for why
/// this shape never grows a `pub` field a consumer's exhaustive literal
/// could break on.
///
/// # Examples
///
/// ```
/// use quadraui::{Rect, TextField, TextFieldHit, TextFieldMeasure, WidgetId};
///
/// let field = TextField::new(WidgetId::new("search"))
///     .with_value("query")
///     .with_placeholder("Search")
///     .with_cursor(Some(5))
///     .with_has_focus(true)
///     .with_clearable(true);
///
/// let rect = Rect::new(0.0, 0.0, 20.0, 1.0);
/// let layout = field.layout(rect, TextFieldMeasure::new(1.0, 1.0));
///
/// assert!(layout.cursor_bounds.is_some());
/// let clear = layout.clear_rect.expect("non-empty clearable value shows ×");
/// assert_eq!(layout.hit_test(clear.x, clear.y), TextFieldHit::Clear);
/// ```
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

// ── Cell-grid layout + paint ───────────────────────────────────────────

/// Resolve this field's layout in **whole cell** units — the one
/// cell-grid entry point [`crate::primitive::Primitive::layout`] and
/// [`crate::tui::tui_text_field_layout`] share, so the widget contract
/// and the legacy `Backend::draw_text_field` path can never disagree
/// about where the icon, text column or clear button sit.
pub fn cell_layout(tf: &TextField, rect: Rect) -> TextFieldLayout {
    tf.layout(rect, TextFieldMeasure::new(1.0, 1.0))
}

/// `TextField`'s cell-grid rasteriser, written against
/// [`crate::CellSurface`]'s verbs — the cell-grid twin of
/// [`paint_text_field_gui`].
///
/// One row: `[` + optional leading-icon glyph + text + optional `×`
/// clear button + `]`, with a block caret (reverse video on the cursor's
/// own cell). The brackets tint [`crate::Theme::accent_fg`] while
/// focused and [`crate::Theme::muted_fg`] otherwise.
///
/// [`put_text_row`] is the "paint the text between the brackets" half,
/// shared with the `Form` cell painter's own `TextInput` row, so
/// the two can never drift apart on cursor/selection math. The field's
/// brackets/icon/clear button paint separately from `Form`'s row because
/// only `TextField` scrolls — `Form`'s row truncates instead.
/// `#[cfg(feature = "tui")]`: `crate::tui`'s own delegate is this
/// painter's only caller, so a pixel-only or featureless build would
/// compile it unused — and CI's `-D warnings` legs turn that into a hard
/// error.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{TextField, TextFieldLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::event::Rect;
    use crate::theme::Theme;
    use crate::types::Color;

    /// Paint `tf` at the already-resolved `layout` onto `surface`.
    pub(crate) fn paint(
        tf: &TextField,
        layout: &TextFieldLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        nerd_fonts_enabled: bool,
    ) {
        let rect = layout.bounds;
        if rect.width < 1.0 || rect.height < 1.0 {
            return;
        }
        let row_bg = theme.input_bg;
        let field_fg = theme.foreground;
        let dim_fg = theme.muted_fg;
        let bracket_fg = if tf.has_focus {
            theme.accent_fg
        } else {
            dim_fg
        };
        let style = |fg: Color| CellStyle {
            fg: Some(fg),
            bg: Some(row_bg),
            ..CellStyle::default()
        };

        let y = rect.y as u16;
        let x0 = rect.x as u16;
        let x1 = x0 + (rect.width as u16).saturating_sub(1);

        // Clear this single row first — the same "paint the background,
        // then overlay content" order every cell rasteriser uses. Only
        // `rect.y`'s row, never the full `rect` height: a caller that
        // lays this field out taller than one cell must not get extra
        // background rows below the bracketed line.
        let row_rect = Rect::new(rect.x, rect.y, rect.width, 1.0);
        surface.fill(row_rect, style(field_fg));

        surface.put_char(x0, y, '[', style(bracket_fg));
        if x1 > x0 {
            surface.put_char(x1, y, ']', style(bracket_fg));
        }

        if let Some(icon_rect) = layout.icon_rect {
            let glyph = tf
                .leading_icon
                .as_ref()
                .and_then(|icon| icon.resolve(nerd_fonts_enabled).chars().next())
                .unwrap_or(' ');
            surface.put_char(icon_rect.x as u16, y, glyph, style(dim_fg));
        }

        let text_col = layout.text_rect.x as u16;
        let text_cols = layout.text_rect.width as usize;
        put_text_row(
            surface,
            y,
            text_col,
            text_col + text_cols as u16,
            text_cols,
            layout.resolved_scroll_chars,
            &tf.value,
            &tf.placeholder,
            tf.cursor,
            tf.selection_anchor,
            field_fg,
            dim_fg,
            row_bg,
        );

        if let Some(clear_rect) = layout.clear_rect {
            surface.put_char(clear_rect.x as u16, y, '\u{00D7}', style(dim_fg));
        }
    }

    /// Paint the inside-the-brackets text content: `value` (or dimmed
    /// `placeholder` when empty) starting at column `text_col`, clipped
    /// to `max_cols`, with a selection highlight between the byte
    /// offsets `cursor`/`selection_anchor` bracket and a reverse-video
    /// block caret at `cursor`.
    ///
    /// `skip_chars` is how many leading characters of the shown text are
    /// scrolled out of view before painting starts — `0` for `Form`'s
    /// row (which has no scroll, only truncation),
    /// [`TextFieldLayout::resolved_scroll_chars`] for `TextField`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn put_text_row(
        surface: &mut dyn CellSurface,
        y: u16,
        text_col: u16,
        right_bound: u16,
        max_cols: usize,
        skip_chars: usize,
        value: &str,
        placeholder: &str,
        cursor: Option<usize>,
        selection_anchor: Option<usize>,
        field_fg: Color,
        dim_fg: Color,
        row_bg: Color,
    ) {
        let shown = if value.is_empty() { placeholder } else { value };
        let input_fg = if value.is_empty() { dim_fg } else { field_fg };
        let visible_chars = shown.chars().count().saturating_sub(skip_chars);
        let desired = max_cols.min(visible_chars);

        let (sel_lo, sel_hi) = if value.is_empty() {
            (0, 0)
        } else {
            match (cursor, selection_anchor) {
                (Some(c), Some(a)) if c != a => (c.min(a), c.max(a)),
                _ => (0, 0),
            }
        };

        let mut byte = 0usize;
        let mut chars = shown.chars();
        for _ in 0..skip_chars {
            match chars.next() {
                Some(ch) => byte += ch.len_utf8(),
                None => break,
            }
        }
        for (i, ch) in chars.take(desired).enumerate() {
            let col = text_col + i as u16;
            if col >= right_bound {
                break;
            }
            let in_selection = sel_hi > sel_lo && byte >= sel_lo && byte < sel_hi;
            let (ch_fg, ch_bg) = if in_selection {
                (row_bg, input_fg)
            } else {
                (input_fg, row_bg)
            };
            surface.put_char(
                col,
                y,
                ch,
                CellStyle {
                    fg: Some(ch_fg),
                    bg: Some(ch_bg),
                    ..CellStyle::default()
                },
            );
            byte += ch.len_utf8();
        }

        // The caret itself is bound only by `right_bound` (the same bound
        // the text loop above uses), not by `desired` — a cursor sitting
        // just past the last visible character (typing at the end of a
        // field that exactly fills its column budget) still needs a
        // paintable cell there, even though the text loop above painted
        // nothing that far right.
        if let Some(cur) = cursor {
            if !value.is_empty() {
                let mut byte2 = 0usize;
                let mut char_idx = 0usize;
                for ch in shown.chars() {
                    if byte2 >= cur {
                        break;
                    }
                    byte2 += ch.len_utf8();
                    char_idx += 1;
                }
                if char_idx >= skip_chars {
                    // Bound `rel` by the same text-column budget the
                    // visible text loop above uses (`max_cols`). `Form`'s
                    // row (which always passes `skip_chars = 0`)
                    // truncates rather than scrolls, so a cursor sitting
                    // past the truncated window would otherwise paint the
                    // caret outside the field's brackets; `TextField`'s
                    // own scrolling layout already guarantees
                    // `rel <= max_cols` here, so the clamp is a no-op for
                    // it.
                    let rel = (char_idx - skip_chars).min(max_cols);
                    let cursor_col = text_col + rel as u16;
                    if cursor_col < right_bound {
                        let ch = shown.chars().nth(char_idx).unwrap_or(' ');
                        surface.put_char(
                            cursor_col,
                            y,
                            ch,
                            CellStyle {
                                fg: Some(row_bg),
                                bg: Some(field_fg),
                                ..CellStyle::default()
                            },
                        );
                    }
                }
            }
        }
    }
}

// ── Widget contract ────────────────────────────────────────────────────

/// [`crate::primitive::Primitive::Layout`] for [`TextField`]: the
/// absolute area the field was laid out into, the resolved
/// [`TextFieldLayout`], the character advance the laying-out backend
/// measured with, and whether that backend has a Nerd Font registered.
///
/// `char_width` is carried because [`crate::interactive::Interactive`]
/// gets no [`crate::primitive::LayoutCtx`] of its own and resolving a
/// click's character offset needs the same advance the layout positioned
/// glyphs with; `nerd_fonts` because
/// [`crate::primitive::Primitive::paint_cells`] takes none either and a
/// [`crate::types::Icon`] resolves a different glyph with and without
/// one.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct TextFieldPrimitiveLayout {
    /// The absolute rect the field was laid out into.
    pub area: Rect,
    /// The resolved field layout, in absolute coordinates.
    pub layout: TextFieldLayout,
    /// One character's advance, in the laying-out backend's units.
    pub char_width: f32,
    /// Whether the laying-out backend has a Nerd Font icon face.
    pub nerd_fonts: bool,
}

impl TextFieldPrimitiveLayout {
    /// Classify an absolute point against this field — see
    /// [`TextFieldLayout::hit_test`].
    pub fn hit_test(&self, x: f32, y: f32) -> TextFieldHit {
        self.layout.hit_test(x, y)
    }

    /// The character offset (not byte offset) an absolute `x` resolves
    /// to inside the visible text window, rounded to the nearest
    /// boundary so a click on a glyph's right half puts the caret after
    /// it.
    pub fn char_offset_at(&self, x: f32) -> usize {
        let char_w = self.char_width.max(1.0);
        let offset = ((x - self.layout.text_rect.x) / char_w).round().max(0.0) as usize;
        self.layout.resolved_scroll_chars + offset
    }
}

impl crate::primitive::Primitive for TextField {
    type Layout = TextFieldPrimitiveLayout;
    type Event = TextFieldHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::TextField(self.clone())
    }

    /// One row tall, and as wide as the value (or placeholder) plus the
    /// two brackets and whatever icon/clear columns this field requests.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let char_w = ctx.char_width.max(1.0);
        let shown = if self.value.is_empty() {
            self.placeholder.as_str()
        } else {
            self.value.as_str()
        };
        let chrome = 2
            + usize::from(self.leading_icon.is_some())
            + usize::from(self.clearable && !self.value.is_empty());
        let cols = shown.chars().count() + chrome;
        crate::NaturalSize::new(
            Some((cols as f32 * char_w).min(avail.width)),
            Some(ctx.line_height.max(1.0)),
        )
    }

    /// `rect` is the field's own one-row box. On a cell grid the layout
    /// comes from [`cell_layout`]; on a pixel surface from
    /// [`TextField::layout`] with the backend's own advance and line
    /// height.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> TextFieldPrimitiveLayout {
        let rect = match ctx.units() {
            crate::primitive::Units::Cells => crate::primitive::cell_grid_rect(rect),
            _ => rect,
        };
        let (layout, char_width) = match ctx.units() {
            crate::primitive::Units::Cells => (cell_layout(self, rect), 1.0),
            _ => (
                self.layout(rect, TextFieldMeasure::new(ctx.char_width, ctx.line_height)),
                ctx.char_width.max(1.0),
            ),
        };
        TextFieldPrimitiveLayout {
            area: rect,
            layout,
            char_width,
            nerd_fonts: ctx.nerd_fonts_enabled,
        }
    }

    /// The shared pixel painter — the same one every pixel backend's
    /// `draw_text_field` routes through.
    fn paint(
        &self,
        layout: &TextFieldPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let painted = self.with_painted_caret(env.widget_state_as::<TextFieldState>());
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            paint_text_field_gui(
                surface,
                layout.area,
                &layout.layout,
                &painted,
                theme,
                layout.nerd_fonts,
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (&painted, layout, surface, theme);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_text_field`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`. That
    /// entry point re-resolves the layout from the same area through the
    /// same [`cell_layout`] this `Primitive` laid out with, so the glyphs
    /// land exactly where `layout` says they do.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &TextFieldPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let painted = self.with_painted_caret(env.widget_state_as::<TextFieldState>());
        cell_paint::paint(&painted, &layout.layout, cells, theme, layout.nerd_fonts);
    }
}

/// [`crate::interactive::Interactive::State`] for [`TextField`]: the
/// shared flat-value editing state (caret, selection anchor and
/// drag-select flag, as byte offsets into [`TextField::value`]).
///
/// Same division of labour as [`crate::primitives::text_input::TextInputState`]
/// — see that type's doc for why the caret is interaction state rather
/// than something read back off the descriptor.
pub type TextFieldState = crate::behaviour::text_edit::FlatEditState;

/// What a [`TextField`]'s own [`crate::interactive::Interactive::handle`]
/// resolves one gesture to. A copy reaches the host as
/// [`crate::UiEvent::TextCopied`] directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextFieldAction {
    /// The value changed — a keystroke, a cut, or a click on the clear
    /// (×) button. Carries the whole new value, the shape
    /// [`crate::primitives::form::FormEvent::TextInputChanged`] already
    /// names.
    Changed(String),
    /// Enter committed the field's current value.
    Committed(String),
}

impl crate::interactive::WidgetAction for TextFieldAction {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            TextFieldAction::Changed(value) => crate::UiEvent::Form(
                id,
                crate::primitives::form::FormEvent::TextInputChanged { id, value },
            ),
            TextFieldAction::Committed(value) => crate::UiEvent::Form(
                id,
                crate::primitives::form::FormEvent::TextInputCommitted { id, value },
            ),
        }
    }
}

impl TextField {
    /// This field with the caret and selection its own editing has
    /// walked to, for a painter — the descriptor's own when `state` has
    /// not moved the caret.
    fn with_painted_caret(&self, state: Option<&TextFieldState>) -> std::borrow::Cow<'_, Self> {
        let (cursor, anchor) = crate::behaviour::text_edit::painted_flat_caret(
            state,
            self.cursor,
            self.selection_anchor,
        );
        if cursor == self.cursor && anchor == self.selection_anchor {
            std::borrow::Cow::Borrowed(self)
        } else {
            let mut painted = self.clone();
            painted.cursor = cursor;
            painted.selection_anchor = anchor;
            std::borrow::Cow::Owned(painted)
        }
    }
}

impl crate::interactive::Interactive for TextField {
    type State = TextFieldState;
    type Action = TextFieldAction;

    /// One focus stop as a whole — a single-line field routes every
    /// keystroke itself.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    /// The clear (×) button first, then the text column — most specific
    /// first, translated into the widget-local space this method answers
    /// in.
    fn sub_regions(
        &self,
        layout: &TextFieldPrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        let (ox, oy) = (layout.area.x, layout.area.y);
        let local = |r: Rect| Rect::new(r.x - ox, r.y - oy, r.width, r.height);
        let mut out = Vec::new();
        if let Some(clear) = layout.layout.clear_rect {
            out.push((local(clear), crate::interactive::SubTarget(SUB_CLEAR)));
        }
        out.push((
            local(layout.layout.text_rect),
            crate::interactive::SubTarget(SUB_TEXT),
        ));
        out
    }

    /// A screen reader can focus the field and replace its whole value.
    fn a11y_actions(&self, _state: &TextFieldState) -> Vec<crate::a11y::A11yAction> {
        vec![
            crate::a11y::A11yAction::Focus,
            crate::a11y::A11yAction::SetValue,
        ]
    }

    /// Enter commits and a press on the clear button empties the field;
    /// every other event goes through the shared flat-value editing
    /// handler, so this field, [`crate::CommandLine`], the palette query
    /// and `Form`'s own text row can never disagree about what a key, a
    /// click or a drag means.
    fn handle(
        &self,
        state: &mut TextFieldState,
        event: &crate::interactive::RoutedEvent,
        layout: &TextFieldPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TextFieldAction> {
        use crate::behaviour::text_edit::{col_to_byte, handle_flat_edit_event, FlatEditOutcome};
        use crate::interactive::{PointerKind, RoutedEvent};

        match event {
            // Single-line field: Enter commits instead of splitting the
            // value.
            RoutedEvent::Key(key)
                if matches!(
                    key.key,
                    crate::event::Key::Named(crate::event::NamedKey::Enter)
                ) =>
            {
                return Handled::Event(TextFieldAction::Committed(self.value.clone()));
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                local,
                ..
            } if layout.hit_test(layout.area.x + local.x, layout.area.y + local.y)
                == TextFieldHit::Clear =>
            {
                cx.request_focus(None);
                *state = TextFieldState {
                    cursor: Some(0),
                    ..TextFieldState::default()
                };
                return if self.value.is_empty() {
                    Handled::Consumed
                } else {
                    Handled::Event(TextFieldAction::Changed(String::new()))
                };
            }
            _ => {}
        }
        let offset_at = |local: crate::event::Point| {
            let (x, y) = (layout.area.x + local.x, layout.area.y + local.y);
            (layout.hit_test(x, y) == TextFieldHit::Text)
                .then(|| col_to_byte(&self.value, layout.char_offset_at(x)))
        };
        match handle_flat_edit_event(
            &self.value,
            self.cursor,
            self.selection_anchor,
            state,
            event,
            &offset_at,
            cx,
        ) {
            FlatEditOutcome::Changed(value) => Handled::Event(TextFieldAction::Changed(value)),
            FlatEditOutcome::Consumed => Handled::Consumed,
            FlatEditOutcome::Ignored => Handled::Ignored,
        }
    }
}

/// [`crate::interactive::SubTarget`] for the clear (×) button — see
/// [`crate::interactive::Interactive::sub_regions`].
const SUB_CLEAR: u64 = 0;
/// [`crate::interactive::SubTarget`] for the text column.
const SUB_TEXT: u64 = 1;

/// Routes a headless `TuiDriver` test can't reach: the
/// assistive-technology action path, a host-driven focus transition, the
/// widget-local `sub_regions` translation, and word motion.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::event::{Key, MouseButton, NamedKey, Point};
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::types::Modifiers;
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock — the same shape every other widget
    /// module's own `Interactive` tests use.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
        emitted: Vec<crate::UiEvent>,
        redraws: usize,
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
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {
            self.redraws += 1;
        }
        fn emit(&mut self, event: crate::UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn field(value: &str) -> TextField {
        TextField::new(WidgetId::new("tf"))
            .with_value(value)
            .with_has_focus(true)
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 14.0, 1.0)
    }

    fn layout_of(tf: &TextField) -> TextFieldPrimitiveLayout {
        Primitive::layout(tf, &LayoutCtx::new(1.0, 1.0, false), area())
    }

    fn key(key: Key, modifiers: Modifiers) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key,
            modifiers,
            repeat: false,
        })
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    fn click(x: f32, mods: Modifiers) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(x, 0.0),
            mods,
        }
    }

    #[test]
    fn a_text_field_is_one_focus_stop() {
        assert_eq!(Interactive::focus(&field("a")), FocusSpec::Stop);
    }

    #[test]
    fn sub_regions_name_the_clear_button_before_the_text_column() {
        let tf = field("abc").with_clearable(true);
        let layout = layout_of(&tf);
        let regions = Interactive::sub_regions(&tf, &layout);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].1, SubTarget(0));
        assert_eq!(regions[1].1, SubTarget(1));
        // Local space: the text column starts one bracket column in,
        // never at the absolute origin.
        assert_eq!(regions[1].0.x, 1.0);
    }

    #[test]
    fn enter_commits_the_current_value_without_inserting_a_newline() {
        let tf = field("query");
        let layout = layout_of(&tf);
        let mut host = MockHost::default();
        let mut state = TextFieldState::default();
        let mut cx = EventCx::new(tf.id, &mut host);
        let handled = Interactive::handle(
            &tf,
            &mut state,
            &key(Key::Named(NamedKey::Enter), Modifiers::default()),
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(TextFieldAction::Committed("query".into()))
        );
    }

    #[test]
    fn ctrl_x_emits_the_copy_alongside_the_shortened_value() {
        let tf = field("query");
        let layout = layout_of(&tf);
        let mut host = MockHost::default();
        let mut state = TextFieldState::default();
        let mut cx = EventCx::new(tf.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &tf,
                &mut state,
                &key(Key::Char('a'), ctrl()),
                &layout,
                &mut cx,
            ),
            Handled::Consumed
        );
        let handled = Interactive::handle(
            &tf,
            &mut state,
            &key(Key::Char('x'), ctrl()),
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(TextFieldAction::Changed(String::new()))
        );
        assert_eq!(
            host.emitted,
            vec![crate::UiEvent::TextCopied("query".into())]
        );
    }

    #[test]
    fn ctrl_left_moves_the_caret_a_whole_word() {
        let tf = field("foo_bar baz");
        let layout = layout_of(&tf);
        let mut host = MockHost::default();
        let mut state = TextFieldState::default();
        let mut cx = EventCx::new(tf.id, &mut host);
        let handled = Interactive::handle(
            &tf,
            &mut state,
            &key(Key::Named(NamedKey::Left), ctrl()),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(state.cursor, Some(8));
        assert_eq!(host.redraws, 1);
    }

    #[test]
    fn a_click_on_the_clear_button_empties_the_field() {
        let tf = field("abc").with_clearable(true);
        let layout = layout_of(&tf);
        let clear = layout
            .layout
            .clear_rect
            .expect("clearable, non-empty value");
        let mut host = MockHost::default();
        let mut state = TextFieldState::default();
        let mut cx = EventCx::new(tf.id, &mut host);
        let handled = Interactive::handle(
            &tf,
            &mut state,
            &click(clear.x + 0.5, Modifiers::default()),
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(TextFieldAction::Changed(String::new()))
        );
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn a_shift_click_extends_the_selection_to_the_clicked_character() {
        let tf = field("alpha");
        let layout = layout_of(&tf);
        let text_x = layout.layout.text_rect.x;
        let mut host = MockHost::default();
        let mut state = TextFieldState::default();
        let mut cx = EventCx::new(tf.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &tf,
                &mut state,
                &click(text_x + 1.0, Modifiers::default()),
                &layout,
                &mut cx,
            ),
            Handled::Consumed
        );
        assert_eq!(state.cursor, Some(1));
        assert_eq!(
            Interactive::handle(
                &tf,
                &mut state,
                &click(
                    text_x + 4.0,
                    Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                ),
                &layout,
                &mut cx,
            ),
            Handled::Consumed
        );
        assert_eq!(state.cursor, Some(4));
        assert_eq!(state.anchor, Some(1));
    }

    #[test]
    fn an_a11y_focus_action_requests_focus_on_the_field_itself() {
        let tf = field("a");
        let layout = layout_of(&tf);
        let mut host = MockHost::default();
        let mut state = TextFieldState::default();
        let mut cx = EventCx::new(tf.id, &mut host);
        let handled = Interactive::handle(
            &tf,
            &mut state,
            &RoutedEvent::Action(A11yAction::Focus),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn focus_in_seeds_the_caret_from_the_descriptor_and_focus_out_releases_it() {
        let tf = field("alpha").with_cursor(Some(2));
        let layout = layout_of(&tf);
        let mut host = MockHost::default();
        let mut state = TextFieldState {
            cursor: Some(5),
            anchor: Some(4),
            ..TextFieldState::default()
        };
        let mut cx = EventCx::new(tf.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &tf,
                &mut state,
                &RoutedEvent::FocusIn(None),
                &layout,
                &mut cx
            ),
            Handled::Consumed
        );
        assert_eq!(state.cursor, Some(2));
        assert_eq!(state.anchor, None);

        assert_eq!(
            Interactive::handle(&tf, &mut state, &RoutedEvent::FocusOut, &layout, &mut cx),
            Handled::Consumed
        );
        assert_eq!(state.cursor, None);
    }

    #[test]
    fn ctrl_z_is_left_to_the_app_instead_of_typing_a_z() {
        let tf = field("alpha");
        let layout = layout_of(&tf);
        let mut host = MockHost::default();
        let mut state = TextFieldState::default();
        let mut cx = EventCx::new(tf.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &tf,
                &mut state,
                &key(Key::Char('z'), ctrl()),
                &layout,
                &mut cx,
            ),
            Handled::Ignored
        );
    }

    #[test]
    fn a_press_then_a_drag_selects_the_dragged_over_text() {
        let tf = field("alpha");
        let layout = layout_of(&tf);
        let text_x = layout.layout.text_rect.x;
        let mut host = MockHost::default();
        let mut state = TextFieldState::default();
        let mut cx = EventCx::new(tf.id, &mut host);
        let _ = Interactive::handle(
            &tf,
            &mut state,
            &click(text_x + 1.0, Modifiers::default()),
            &layout,
            &mut cx,
        );
        let drag = RoutedEvent::Pointer {
            kind: PointerKind::Move,
            sub: None,
            local: Point::new(text_x + 4.0, 0.0),
            mods: Modifiers::default(),
        };
        assert_eq!(
            Interactive::handle(&tf, &mut state, &drag, &layout, &mut cx),
            Handled::Consumed
        );
        assert_eq!((state.cursor, state.anchor), (Some(4), Some(1)));

        let release = RoutedEvent::Pointer {
            kind: PointerKind::Up(MouseButton::Left),
            sub: None,
            local: Point::new(text_x + 4.0, 0.0),
            mods: Modifiers::default(),
        };
        let _ = Interactive::handle(&tf, &mut state, &release, &layout, &mut cx);
        let after = RoutedEvent::Pointer {
            kind: PointerKind::Move,
            sub: None,
            local: Point::new(text_x + 2.0, 0.0),
            mods: Modifiers::default(),
        };
        assert_eq!(
            Interactive::handle(&tf, &mut state, &after, &layout, &mut cx),
            Handled::Ignored
        );
        assert_eq!(state.cursor, Some(4));
    }

    #[test]
    fn a_pixel_layout_resolves_a_click_with_the_backends_char_width() {
        let tf = field("alpha");
        let layout = Primitive::layout(
            &tf,
            &LayoutCtx::new(16.0, 8.0, false),
            Rect::new(0.0, 0.0, 160.0, 20.0),
        );
        assert_eq!(layout.char_width, 8.0);
        assert_eq!(layout.char_offset_at(layout.layout.text_rect.x + 24.0), 3);
    }

    #[test]
    fn measure_reports_one_row_and_the_value_plus_its_chrome_columns() {
        let tf = field("abc").with_clearable(true);
        let size = Primitive::measure(
            &tf,
            &LayoutCtx::new(1.0, 1.0, false),
            crate::primitive::Size::new(40.0, 40.0),
        );
        // Two brackets + one clear column + three value columns.
        assert_eq!(size.width, Some(6.0));
        assert_eq!(size.height, Some(1.0));
    }
}

/// Cell-grid paint tests for [`cell_paint`], driven through
/// [`crate::tui::draw_text_field`] — the delegate that owns the
/// `Buffer` → [`crate::CellSurface`] adapter the painter needs.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::cell_test_paint::draw_text_field;
    use crate::primitives::text_field::TextField;
    use crate::theme::Theme;
    use crate::types::{Color, Icon, WidgetId};

    fn theme() -> Theme {
        Theme::default()
    }

    /// The ratatui colour `crate::tui`'s own cell writers land for a
    /// quadraui [`Color`] — spelled here because that crate-private
    /// conversion isn't reachable from a widget module.
    fn qc(c: Color) -> ratatui::style::Color {
        ratatui::style::Color::Rgb(c.r, c.g, c.b)
    }

    /// Paint/click round-trip (`PRIMITIVE_RULES.md` rule 4): paint, find
    /// the painted glyphs, hit_test those exact coordinates, assert
    /// paint and click identify the same region.
    #[test]
    fn paint_click_round_trip_text_and_clear() {
        let tf = TextField::new(WidgetId::new("search"))
            .with_value("abc")
            .with_clearable(true)
            .with_leading_icon(Icon::new("S", "S"))
            .with_has_focus(true);
        let area = Rect::new(2, 1, 12, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let layout = draw_text_field(&mut buf, area, &tf, &theme(), false);

        // Clear glyph painted where the layout says it should be.
        let clear_rect = layout.clear_rect.expect("clearable + non-empty value");
        let cell = &buf[(clear_rect.x as u16, clear_rect.y as u16)];
        assert_eq!(cell.symbol(), "\u{00D7}");
        assert_eq!(
            layout.hit_test(clear_rect.x + 0.5, clear_rect.y + 0.5),
            super::TextFieldHit::Clear
        );

        // Text glyph painted where the layout says it should be.
        let text_rect = layout.text_rect;
        let cell = &buf[(text_rect.x as u16, text_rect.y as u16)];
        assert_eq!(cell.symbol(), "a");
        assert_eq!(
            layout.hit_test(text_rect.x + 0.5, text_rect.y + 0.5),
            super::TextFieldHit::Text
        );

        // Brackets at the row's own edges.
        assert_eq!(buf[(area.x, area.y)].symbol(), "[");
        assert_eq!(buf[(area.x + area.width - 1, area.y)].symbol(), "]");
    }

    #[test]
    fn paint_shows_the_selection_the_fields_own_editing_made() {
        use crate::primitive::{LayoutCtx, PaintEnv, Primitive};
        let tf = TextField::new(WidgetId::new("tf"))
            .with_value("alpha")
            .with_has_focus(true);
        let area = crate::event::Rect::new(0.0, 0.0, 12.0, 1.0);
        let layout = Primitive::layout(&tf, &LayoutCtx::new(1.0, 1.0, false), area);
        let text_x = layout.layout.text_rect.x as u16;
        let theme = theme();
        let style = crate::Style::default();
        let paint = |state: Option<&super::TextFieldState>| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
            {
                let mut cells = crate::tui::BufferCellSurface::new(&mut buf);
                let env = PaintEnv::new(&theme, &style)
                    .with_widget_state(state.map(|s| s as &dyn std::any::Any));
                Primitive::paint_cells(&tf, &layout, &mut cells, &env);
            }
            buf
        };
        let plain = paint(None);
        assert_eq!(plain[(text_x + 1, 0)].bg, qc(theme.input_bg));

        let selected = super::TextFieldState {
            cursor: Some(3),
            anchor: Some(1),
            ..Default::default()
        };
        let buf = paint(Some(&selected));
        assert_ne!(buf[(text_x + 1, 0)].bg, qc(theme.input_bg));
        assert_ne!(buf[(text_x + 2, 0)].bg, qc(theme.input_bg));
        assert_eq!(buf[(text_x + 4, 0)].bg, qc(theme.input_bg));
    }

    #[test]
    fn placeholder_renders_dimmed_when_value_is_empty() {
        let tf = TextField::new(WidgetId::new("search")).with_placeholder("Search");
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        draw_text_field(&mut buf, area, &tf, &theme(), false);
        let cell = &buf[(1, 0)];
        assert_eq!(cell.symbol(), "S");
        assert_eq!(cell.fg, qc(theme().muted_fg));
    }

    #[test]
    fn narrow_rect_drops_clear_then_icon_when_painting() {
        let tf = TextField::new(WidgetId::new("search"))
            .with_value("hi")
            .with_clearable(true)
            .with_leading_icon(Icon::new("S", "S"));
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        let layout = draw_text_field(&mut buf, area, &tf, &theme(), false);
        assert!(layout.clear_rect.is_none());
        assert!(layout.icon_rect.is_none());
        // Only the brackets + one text column fit; the painted glyph at
        // the sole text column is the value's first (and only visible)
        // character.
        assert_eq!(buf[(1, 0)].symbol(), "h");
    }

    #[test]
    fn cursor_scrolled_into_view_paints_in_the_last_text_column() {
        let tf = TextField::new(WidgetId::new("search"))
            .with_value("0123456789")
            .with_cursor(Some(10))
            .with_has_focus(true);
        let area = Rect::new(0, 0, 10, 1); // 8 text cols after brackets
        let mut buf = Buffer::empty(area);
        let layout = draw_text_field(&mut buf, area, &tf, &theme(), false);
        assert_eq!(layout.resolved_scroll_chars, 3);
        // Visible window is "3456789" clipped to 8 cols -> "3456789" (7
        // chars, cursor sits one past the last char at the 8th column).
        assert_eq!(buf[(1, 0)].symbol(), "3");
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

/// Cell-painting entry points used by this module's own `#[cfg(test)]`
/// rasteriser tests: they build the `Buffer` -> [`crate::CellSurface`]
/// adapter (private to `crate::tui`) the same way
/// [`crate::primitive::Primitive::paint_cells`] does, so a test can
/// paint into a bare ratatui buffer without a backend.
#[cfg(all(test, feature = "tui"))]
mod cell_test_paint {
    use super::*;
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;

    fn surface<'a>(
        buf: &'a mut Buffer,
        sink: &'a mut Option<(u16, u16, crate::cell_surface::CursorShape)>,
    ) -> impl crate::CellSurface + 'a {
        crate::tui::cell_surface::TuiCellSurface::over(
            buf,
            crate::backend::ColorDepth::TrueColor,
            sink,
        )
    }

    pub fn draw_text_field(
        buf: &mut Buffer,
        rect: ratatui::layout::Rect,
        tf: &TextField,
        theme: &Theme,
        nerd_fonts_enabled: bool,
    ) -> TextFieldLayout {
        let area = Rect::new(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        );
        let layout = cell_layout(tf, area);
        let mut sink = None;
        cell_paint::paint(
            tf,
            &layout,
            &mut surface(buf, &mut sink),
            theme,
            nerd_fonts_enabled,
        );
        layout
    }
}
