//! `CommandLine` primitive: a single-line input/output surface for editor
//! command prompts (`:`, `/`, `?`) and transient messages.
//!
//! Display-only �� the engine handles keystroke input and updates
//! `text` / `cursor_offset` each frame. Both TUI and GTK rasterisers
//! draw text with an optional insert cursor; alignment can be flipped
//! for right-aligned count displays.
//!
//! [`CommandLineLayout`] (issue #705) closes the character-offset hit-test
//! gap that made the command line mouse-selectable on TUI (via a
//! terminal-only inverted-cell read-back trick) and structurally unable to
//! be on GTK. `CommandLine::layout` resolves click/selection geometry the
//! same way every other primitive does — see [`CommandLineLayout::hit_test`]
//! and [`CommandLineLayout::selection_bounds`].

use crate::event::Rect;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Declarative description of a command line surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandLine {
    pub id: WidgetId,
    /// Full display text (includes prompt character if any, e.g. `:wq`).
    pub text: String,
    /// Byte offset within `text` at which to draw the insert cursor.
    /// `None` suppresses the cursor (message-display mode).
    #[serde(default)]
    pub cursor_offset: Option<usize>,
    /// When `true`, right-align the text (used for count/match displays).
    #[serde(default)]
    pub right_align: bool,
}

/// Backend-supplied character metrics for [`CommandLine::layout`]. TUI
/// passes `1.0` (one cell per character); GTK/macOS pass the monospace
/// advance width of the active font, same convention as
/// [`crate::primitives::text_input::TextInputMeasure`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandLineMeasure {
    pub char_width: f32,
}

impl CommandLineMeasure {
    pub fn new(char_width: f32) -> Self {
        Self { char_width }
    }
}

/// Fully-resolved layout for a `CommandLine` (issue #705).
///
/// Gives every backend the character-offset hit test the TUI rasteriser
/// used to get "for free" by reading back inverted terminal cells after
/// painting — a trick with no pixel-backend equivalent, which is exactly
/// why the GTK command line was structurally unable to support mouse
/// selection. [`Self::hit_test`] maps a click x-coordinate to a **byte
/// offset** into [`CommandLine::text`] (matching [`CommandLine::cursor_offset`]'s
/// contract, so the result can be fed straight back into the primitive),
/// and [`Self::selection_bounds`] turns a `(start, end)` byte-offset pair
/// into a paintable rect so a host can render a drag-selection highlight
/// without re-deriving glyph metrics.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommandLineLayout {
    /// Bounds this layout was computed for (matches the `rect` argument).
    pub bounds: Rect,
    /// x at which the first character is painted — `bounds.x` when
    /// left-aligned, shifted right when `right_align` pushes (short) text
    /// toward the far edge.
    pub text_origin_x: f32,
    /// Column width in surface-native units (TUI: 1.0 cells; GTK/macOS:
    /// the backend's monospace char width).
    pub char_width: f32,
    /// Byte offset of each character column in `text`, plus a trailing
    /// entry for `text.len()` — i.e. `len() == text.chars().count() + 1`.
    /// Lets `hit_test` / `selection_bounds` map columns to byte offsets
    /// (and back) without re-walking `text` on every call or storing a
    /// second copy of it.
    char_byte_offsets: Vec<usize>,
}

impl CommandLineLayout {
    /// Map an absolute x-coordinate to a byte offset into the source
    /// `text`, clamped to `[0, text.len()]`. Points left of the first
    /// character return `0`; points at or right of the last column return
    /// `text.len()`.
    ///
    /// Coordinate frame: **ABSOLUTE** — `x` is compared directly against
    /// `text_origin_x`, which already carries `bounds.x` and any
    /// right-align shift (issue #505 convention, matching
    /// [`crate::primitives::text_input::TextInputLayout`]).
    pub fn hit_test(&self, x: f32) -> usize {
        let last = self.char_byte_offsets.last().copied().unwrap_or(0);
        if self.char_width <= 0.0 || self.char_byte_offsets.len() <= 1 {
            return last;
        }
        if x <= self.text_origin_x {
            return self.char_byte_offsets[0];
        }
        let col = ((x - self.text_origin_x) / self.char_width).floor() as usize;
        let max_col = self.char_byte_offsets.len() - 1;
        self.char_byte_offsets[col.min(max_col)]
    }

    /// Rect covering the single character column at `byte_offset` — its
    /// left edge to the next character's left edge. A `byte_offset` that
    /// doesn't land exactly on a known column (e.g. stale, or mid-char) is
    /// snapped to the column it falls within.
    pub fn char_bounds(&self, byte_offset: usize) -> Rect {
        let col = self.column_for_byte_offset(byte_offset);
        Rect::new(
            self.text_origin_x + col as f32 * self.char_width,
            self.bounds.y,
            self.char_width,
            self.bounds.height,
        )
    }

    /// Rect spanning the selection `[start, end)` (byte offsets, either
    /// order — mirrors the drag-selection state a host tracks, which can
    /// run in either direction). A host paints this as a highlight behind
    /// the selected text; this is the "enough geometry for a selection
    /// range to be painted back" piece of #705 — it replaces the
    /// TUI-only inverted-cell pass without quadraui needing to own
    /// selection *state* (that stays host-side, same as today).
    ///
    /// Returns `None` for an empty/zero-width selection.
    pub fn selection_bounds(&self, sel: (usize, usize)) -> Option<Rect> {
        let (lo, hi) = if sel.0 <= sel.1 { sel } else { (sel.1, sel.0) };
        if lo == hi {
            return None;
        }
        let lo_col = self.column_for_byte_offset(lo);
        let hi_col = self.column_for_byte_offset(hi);
        if hi_col <= lo_col {
            return None;
        }
        Some(Rect::new(
            self.text_origin_x + lo_col as f32 * self.char_width,
            self.bounds.y,
            (hi_col - lo_col) as f32 * self.char_width,
            self.bounds.height,
        ))
    }

    /// Column index whose character starts at or covers `byte_offset`.
    fn column_for_byte_offset(&self, byte_offset: usize) -> usize {
        match self.char_byte_offsets.binary_search(&byte_offset) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        }
    }
}

impl CommandLine {
    /// Compute the click/selection geometry for painting or hit-testing
    /// this command line in `rect`, using `measure`'s column width.
    pub fn layout(&self, rect: Rect, measure: CommandLineMeasure) -> CommandLineLayout {
        let char_width = measure.char_width.max(0.0);
        let mut char_byte_offsets: Vec<usize> = self.text.char_indices().map(|(b, _)| b).collect();
        char_byte_offsets.push(self.text.len());
        let n_chars = char_byte_offsets.len().saturating_sub(1);

        let text_origin_x = if self.right_align && char_width > 0.0 {
            let text_w = n_chars as f32 * char_width;
            (rect.x + rect.width - text_w).max(rect.x)
        } else {
            rect.x
        };

        CommandLineLayout {
            bounds: rect,
            text_origin_x,
            char_width,
            char_byte_offsets,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_serde() {
        let cmd = CommandLine {
            id: "cmd".into(),
            text: ":wq".into(),
            cursor_offset: Some(3),
            right_align: false,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: CommandLine = serde_json::from_str(&json).unwrap();
        assert_eq!(back.text, ":wq");
        assert_eq!(back.cursor_offset, Some(3));
    }

    #[test]
    fn defaults_via_serde() {
        let json = r#"{"id":"cmd","text":"hello"}"#;
        let cmd: CommandLine = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.cursor_offset, None);
        assert!(!cmd.right_align);
    }

    // ── CommandLineLayout (issue #705) ──────────────────────────────────

    fn cmd(text: &str, right_align: bool) -> CommandLine {
        CommandLine {
            id: WidgetId::new("cmd"),
            text: text.into(),
            cursor_offset: None,
            right_align,
        }
    }

    #[test]
    fn hit_test_maps_x_to_byte_offset_left_aligned() {
        let c = cmd(":wq", false);
        let layout = c.layout(Rect::new(0.0, 0.0, 20.0, 1.0), CommandLineMeasure::new(1.0));
        assert_eq!(layout.hit_test(0.0), 0); // before ':'
        assert_eq!(layout.hit_test(1.5), 1); // inside 'w'
        assert_eq!(layout.hit_test(100.0), 3); // past the end -> text.len()
    }

    #[test]
    fn hit_test_at_nonzero_origin() {
        // #505: a LOCAL/ABSOLUTE mixup is invisible at rect.x == 0, so this
        // primitive-level test uses a nonzero origin too.
        let c = cmd(":wq", false);
        let layout = c.layout(
            Rect::new(10.0, 5.0, 20.0, 1.0),
            CommandLineMeasure::new(1.0),
        );
        // x < text_origin_x (10.0) clamps to the first column.
        assert_eq!(layout.hit_test(3.0), 0);
        assert_eq!(layout.hit_test(10.0), 0);
        assert_eq!(layout.hit_test(11.5), 1);
    }

    #[test]
    fn hit_test_accounts_for_multibyte_chars() {
        // ":éditer" — 'é' is 2 bytes; byte offsets after it must skip a byte,
        // not walk one-per-column, or this reproduces the #503 class of bug.
        let c = cmd(":éditer", false);
        let layout = c.layout(Rect::new(0.0, 0.0, 20.0, 1.0), CommandLineMeasure::new(1.0));
        // Columns: ':' (0) 'é' (1) 'd' (2) 'i' (3) ...
        assert_eq!(layout.hit_test(0.5), 0);
        assert_eq!(layout.hit_test(1.5), 1); // start of 'é', byte offset 1
        assert_eq!(layout.hit_test(2.5), 3); // start of 'd' -> byte offset 3 (post 2-byte 'é')
    }

    #[test]
    fn hit_test_right_aligned_shifts_text_origin() {
        let c = cmd("3/17", true);
        let layout = c.layout(Rect::new(0.0, 0.0, 10.0, 1.0), CommandLineMeasure::new(1.0));
        // 4 chars in a 10-wide rect -> text starts at x = 6.
        assert_eq!(layout.text_origin_x, 6.0);
        assert_eq!(layout.hit_test(0.0), 0); // left of text -> first column
        assert_eq!(layout.hit_test(6.5), 0);
        assert_eq!(layout.hit_test(7.5), 1);
    }

    #[test]
    fn hit_test_empty_text_always_zero() {
        let c = cmd("", false);
        let layout = c.layout(Rect::new(0.0, 0.0, 10.0, 1.0), CommandLineMeasure::new(1.0));
        assert_eq!(layout.hit_test(0.0), 0);
        assert_eq!(layout.hit_test(9.0), 0);
    }

    #[test]
    fn selection_bounds_spans_the_selected_columns() {
        let c = cmd(":wq!", false);
        let layout = c.layout(Rect::new(0.0, 0.0, 20.0, 1.0), CommandLineMeasure::new(1.0));
        // Select ":wq" -> byte offsets 0..3.
        let r = layout.selection_bounds((0, 3)).unwrap();
        assert_eq!((r.x, r.width), (0.0, 3.0));
    }

    #[test]
    fn selection_bounds_order_independent() {
        let c = cmd(":wq!", false);
        let layout = c.layout(Rect::new(0.0, 0.0, 20.0, 1.0), CommandLineMeasure::new(1.0));
        assert_eq!(
            layout.selection_bounds((3, 0)),
            layout.selection_bounds((0, 3))
        );
    }

    #[test]
    fn selection_bounds_empty_range_is_none() {
        let c = cmd(":wq!", false);
        let layout = c.layout(Rect::new(0.0, 0.0, 20.0, 1.0), CommandLineMeasure::new(1.0));
        assert!(layout.selection_bounds((2, 2)).is_none());
    }

    #[test]
    fn selection_bounds_at_nonzero_origin() {
        let c = cmd(":wq!", false);
        let layout = c.layout(
            Rect::new(10.0, 5.0, 20.0, 1.0),
            CommandLineMeasure::new(1.0),
        );
        let r = layout.selection_bounds((0, 3)).unwrap();
        assert_eq!((r.x, r.y, r.width), (10.0, 5.0, 3.0));
    }

    #[test]
    fn char_bounds_snaps_mid_char_offset_to_its_column() {
        let c = cmd(":éditer", false);
        let layout = c.layout(Rect::new(0.0, 0.0, 20.0, 1.0), CommandLineMeasure::new(1.0));
        // Byte 2 sits inside 'é' (bytes 1..3); should snap to column 1.
        let snapped = layout.char_bounds(2);
        let exact = layout.char_bounds(1);
        assert_eq!(snapped, exact);
    }

    #[test]
    fn roundtrip_hit_test_then_selection_bounds() {
        // A host drags from x=1 to x=3 over ":wq!" and should get back a
        // selection rect covering exactly the "wq" columns.
        let c = cmd(":wq!", false);
        let layout = c.layout(Rect::new(0.0, 0.0, 20.0, 1.0), CommandLineMeasure::new(1.0));
        let start = layout.hit_test(1.0);
        let end = layout.hit_test(3.0);
        let r = layout.selection_bounds((start, end)).unwrap();
        assert_eq!((r.x, r.width), (1.0, 2.0));
    }
}
