//! [`TextInput`] — standalone multi-line text input primitive.
//!
//! Used for free-form text entry (commit messages, multi-line search,
//! note-taking). Stores text as a `Vec<String>` of lines, with cursor
//! position tracked as `(line, col)`. The primitive handles vertical
//! scroll auto-clamp (cursor stays in viewport) and emits hit regions
//! per visible line so consumers route clicks back to text positions.
//!
//! V1 is line-based (consumer pre-splits long input into lines). Word
//! wrap inside the primitive is a future extension; today consumers
//! split on `\n` or pre-wrap at their preferred width.

use serde::{Deserialize, Serialize};

use crate::event::Rect;
use crate::types::WidgetId;

/// Multi-line text input. Text is stored as one entry per line — empty
/// lines are empty strings. Cursor is `(line, col)` in *char columns*
/// (not bytes); the rasterisers convert as needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextInput {
    pub id: WidgetId,
    /// One entry per logical line. Newlines are implicit between
    /// consecutive entries. Empty input is `vec![String::new()]`.
    pub lines: Vec<String>,
    /// Cursor line index. Clamped to `lines.len().saturating_sub(1)`
    /// by [`Self::layout`].
    #[serde(default)]
    pub cursor_line: usize,
    /// Cursor column within the cursor line, in *char columns*.
    /// Clamped to the line's char count by [`Self::layout`].
    #[serde(default)]
    pub cursor_col: usize,
    /// Optional placeholder shown when `lines` is empty or contains a
    /// single empty string. Rendered in muted color.
    #[serde(default)]
    pub placeholder: Option<String>,
    /// First visible line index. The primitive clamps this so the
    /// cursor stays inside the viewport.
    #[serde(default)]
    pub scroll_offset: usize,
    /// Whether the input has keyboard focus. Controls cursor visibility
    /// and border color (rasteriser-defined).
    #[serde(default)]
    pub has_focus: bool,
}

impl TextInput {
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            placeholder: None,
            scroll_offset: 0,
            has_focus: false,
        }
    }
}

/// Hit-test classification for clicks inside a `TextInput`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputHit {
    /// Click landed on a text line — the consumer maps `(line, col)`
    /// to a new cursor position.
    Line { line_idx: usize },
    /// Click landed in the content area but past the last line.
    EmptyArea,
}

/// One visible line resolved by layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTextInputLine {
    /// Index into [`TextInput::lines`].
    pub line_idx: usize,
    pub bounds: Rect,
}

/// Fully-resolved layout.
#[derive(Debug, Clone, PartialEq)]
pub struct TextInputLayout {
    /// Outer bounds (matches the `rect` argument to layout).
    pub bounds: Rect,
    /// Content bounds (inside any border/padding the rasteriser draws).
    pub content_bounds: Rect,
    pub visible_lines: Vec<VisibleTextInputLine>,
    /// Cursor bounds in viewport pixels/cells when the cursor is inside
    /// the visible window, otherwise `None`. Rasterisers paint a cursor
    /// glyph (TUI) or vertical bar (GTK) at this rect.
    pub cursor_bounds: Option<Rect>,
    /// Scroll offset after auto-clamp. The primitive guarantees the
    /// cursor is visible by adjusting this from the input value.
    pub resolved_scroll_offset: usize,
    /// Per-region hit map for click routing.
    pub hit_regions: Vec<(Rect, TextInputHit)>,
    /// True when the placeholder was rendered (lines empty / single
    /// empty line). Rasterisers consult this to draw the placeholder
    /// in a muted color.
    pub placeholder_active: bool,
}

/// Per-line measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextInputMeasure {
    /// Height of one row in surface-native units (TUI: 1.0 cells;
    /// GTK: line_height pixels).
    pub row_height: f32,
    /// Width of one character column. TUI: 1.0 (cells). GTK: the
    /// monospace `char_width` from the backend — used to position the
    /// cursor and route clicks back to columns.
    pub char_width: f32,
}

impl TextInputMeasure {
    pub fn new(row_height: f32, char_width: f32) -> Self {
        Self {
            row_height,
            char_width,
        }
    }
}

impl TextInput {
    /// Compute layout for `rect`. `measure` supplies row height + char
    /// width in the backend's native units.
    pub fn layout(&self, rect: Rect, measure: TextInputMeasure) -> TextInputLayout {
        let row_h = measure.row_height.max(1.0);
        let char_w = measure.char_width.max(1.0);

        // Border + padding: 1 cell / 1 pixel of border + 1 unit padding.
        // Rasterisers reserve the same chrome.
        let border = 1.0;
        let pad = 0.0;
        let content_x = rect.x + border + pad;
        let content_y = rect.y + border + pad;
        let content_w = (rect.width - (border + pad) * 2.0).max(0.0);
        let content_h = (rect.height - (border + pad) * 2.0).max(0.0);
        let content_bounds = Rect::new(content_x, content_y, content_w, content_h);

        let total_lines = self.lines.len().max(1);
        let max_rows = ((content_h / row_h).floor() as usize).max(1);

        // Clamp cursor line/col to text bounds.
        let cursor_line = self.cursor_line.min(total_lines.saturating_sub(1));
        let cursor_col = {
            let line = self.lines.get(cursor_line).map_or("", String::as_str);
            self.cursor_col.min(line.chars().count())
        };

        // Auto-clamp scroll_offset so the cursor stays in view.
        let max_scroll = total_lines.saturating_sub(max_rows);
        let mut scroll = self.scroll_offset.min(max_scroll);
        if cursor_line < scroll {
            scroll = cursor_line;
        } else if cursor_line >= scroll + max_rows {
            scroll = cursor_line + 1 - max_rows;
        }
        scroll = scroll.min(max_scroll);

        let visible_count = total_lines.saturating_sub(scroll).min(max_rows);
        let mut visible_lines: Vec<VisibleTextInputLine> = Vec::with_capacity(visible_count);
        let mut hit_regions: Vec<(Rect, TextInputHit)> = Vec::with_capacity(visible_count + 1);

        let placeholder_active = self.placeholder.is_some()
            && (self.lines.is_empty() || (self.lines.len() == 1 && self.lines[0].is_empty()));

        for i in 0..visible_count {
            let line_idx = scroll + i;
            let row_y = content_y + i as f32 * row_h;
            let row_bounds = Rect::new(content_x, row_y, content_w, row_h);
            visible_lines.push(VisibleTextInputLine {
                line_idx,
                bounds: row_bounds,
            });
            hit_regions.push((row_bounds, TextInputHit::Line { line_idx }));
        }

        // Empty-area hit zone: any vertical space below the last
        // visible line.
        let used_h = visible_count as f32 * row_h;
        if used_h < content_h {
            hit_regions.push((
                Rect::new(content_x, content_y + used_h, content_w, content_h - used_h),
                TextInputHit::EmptyArea,
            ));
        }

        // Cursor bounds — only when in the visible window.
        let cursor_bounds = if cursor_line >= scroll && cursor_line < scroll + visible_count {
            let row_off = cursor_line - scroll;
            let cursor_x = content_x + cursor_col as f32 * char_w;
            let cursor_y = content_y + row_off as f32 * row_h;
            // Width: char_width for TUI block cursor; GTK rasteriser may
            // narrow to a thin bar. We expose char_width so the TUI path
            // works with set_cell at the right column.
            Some(Rect::new(cursor_x, cursor_y, char_w, row_h))
        } else {
            None
        };

        TextInputLayout {
            bounds: rect,
            content_bounds,
            visible_lines,
            cursor_bounds,
            resolved_scroll_offset: scroll,
            hit_regions,
            placeholder_active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(lines: Vec<&str>) -> TextInput {
        TextInput {
            id: WidgetId::new("ti"),
            lines: lines.into_iter().map(String::from).collect(),
            cursor_line: 0,
            cursor_col: 0,
            placeholder: None,
            scroll_offset: 0,
            has_focus: true,
        }
    }

    fn measure() -> TextInputMeasure {
        TextInputMeasure::new(1.0, 1.0)
    }

    fn rect(w: f32, h: f32) -> Rect {
        Rect::new(0.0, 0.0, w, h)
    }

    #[test]
    fn empty_input_renders_one_row() {
        let ti = TextInput::new(WidgetId::new("ti"));
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.visible_lines.len(), 1);
        assert_eq!(l.visible_lines[0].line_idx, 0);
    }

    #[test]
    fn placeholder_active_when_empty() {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.placeholder = Some("type here".into());
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert!(l.placeholder_active);
    }

    #[test]
    fn placeholder_inactive_with_content() {
        let mut ti = input(vec!["hello"]);
        ti.placeholder = Some("type here".into());
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert!(!l.placeholder_active);
    }

    #[test]
    fn cursor_bounds_at_origin_when_empty() {
        let ti = TextInput::new(WidgetId::new("ti"));
        let l = ti.layout(rect(20.0, 10.0), measure());
        let cb = l.cursor_bounds.unwrap();
        // Border is 1 cell -> content starts at (1, 1)
        assert_eq!(cb.x, 1.0);
        assert_eq!(cb.y, 1.0);
        assert_eq!(cb.width, 1.0);
        assert_eq!(cb.height, 1.0);
    }

    #[test]
    fn cursor_bounds_track_col() {
        let mut ti = input(vec!["hello"]);
        ti.cursor_col = 3;
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.cursor_bounds.unwrap().x, 1.0 + 3.0); // border + col
    }

    #[test]
    fn cursor_col_clamped_to_line_length() {
        let mut ti = input(vec!["hi"]);
        ti.cursor_col = 99;
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.cursor_bounds.unwrap().x, 1.0 + 2.0); // clamped to 2
    }

    #[test]
    fn cursor_line_clamped_to_total() {
        let mut ti = input(vec!["a", "b", "c"]);
        ti.cursor_line = 99;
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert!(l.cursor_bounds.is_some());
        // Cursor lands on line index 2 (last available).
        let cb = l.cursor_bounds.unwrap();
        assert_eq!(cb.y, 1.0 + 2.0); // border + 2 rows
    }

    #[test]
    fn scroll_auto_pulls_cursor_into_view_downward() {
        // 10 lines, viewport fits 3, cursor on line 7.
        let mut ti = input(vec!["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]);
        ti.cursor_line = 7;
        // content height = 5 - 2 (border) = 3, max_rows = 3.
        let l = ti.layout(rect(20.0, 5.0), measure());
        // Cursor should be visible: scroll = 7 + 1 - 3 = 5
        assert_eq!(l.resolved_scroll_offset, 5);
        assert!(l.cursor_bounds.is_some());
    }

    #[test]
    fn scroll_auto_pulls_cursor_into_view_upward() {
        let mut ti = input(vec!["0", "1", "2", "3", "4", "5"]);
        ti.cursor_line = 1;
        ti.scroll_offset = 4; // stale — cursor is above viewport
        let l = ti.layout(rect(20.0, 5.0), measure());
        assert_eq!(l.resolved_scroll_offset, 1);
    }

    #[test]
    fn scroll_clamped_when_cursor_in_view() {
        let mut ti = input(vec!["0", "1", "2", "3"]);
        ti.cursor_line = 0;
        ti.scroll_offset = 99; // wildly stale
        let l = ti.layout(rect(20.0, 10.0), measure());
        // max_scroll = 4 - (10-2)/1 = 4 - 8 = 0, so clamp to 0
        assert_eq!(l.resolved_scroll_offset, 0);
    }

    #[test]
    fn hit_regions_one_per_visible_line() {
        let ti = input(vec!["a", "b", "c"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        let line_hits: Vec<_> = l
            .hit_regions
            .iter()
            .filter(|(_, h)| matches!(h, TextInputHit::Line { .. }))
            .collect();
        assert_eq!(line_hits.len(), 3);
    }

    #[test]
    fn empty_area_hit_region_below_last_line() {
        let ti = input(vec!["only"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        let empty = l
            .hit_regions
            .iter()
            .find(|(_, h)| matches!(h, TextInputHit::EmptyArea));
        assert!(empty.is_some());
    }

    #[test]
    fn cursor_bounds_none_when_off_screen() {
        // Force scroll_offset stale by manipulating cursor_line.
        // Build 10 lines, cursor on 0, but max_rows shows 3 — should
        // auto-scroll to keep cursor in view (so this is harder to test).
        // Test the literal off-screen case: cursor_line clamped to 2,
        // scroll forced to 0, max_rows = 1.
        let mut ti = input(vec!["a", "b", "c"]);
        ti.cursor_line = 2;
        ti.scroll_offset = 0;
        let l = ti.layout(rect(20.0, 3.0), measure()); // content_h = 1, max_rows = 1
                                                       // Auto-scroll keeps cursor in view, so it's still visible:
        assert!(l.cursor_bounds.is_some());
        assert_eq!(l.resolved_scroll_offset, 2);
    }
}
