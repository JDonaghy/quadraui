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
///
/// # Examples
///
/// ```
/// use quadraui::{CommandLine, CommandLineMeasure, Rect, WidgetId};
///
/// let cmd = CommandLine {
///     id: WidgetId::new("cmdline:editor"),
///     text: ":wq".to_string(),
///     cursor_offset: Some(3),
///     right_align: false,
/// };
///
/// let rect = Rect::new(0.0, 23.0, 80.0, 1.0);
/// let layout = cmd.layout(rect, CommandLineMeasure::new(1.0));
///
/// assert_eq!(layout.hit_test(0.0), 0);
/// assert_eq!(layout.hit_test(100.0), cmd.text.len());
/// ```
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

    /// Build from the backend's own [`crate::backend::Metrics`]
    /// (`backend.measure()`) instead of reading `backend.char_width()`
    /// by hand (quadraui#817).
    pub fn from_metrics(m: &crate::backend::Metrics) -> Self {
        Self::new(m.char_width)
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

/// Shared [`PaintSurface`](crate::paint_surface::PaintSurface)-backed
/// paint for [`CommandLine`] (`PaintSurface` Phase 4 6/8, #1083) —
/// replaces the three per-backend copies `gtk::command_line::draw_command_line_selection`,
/// `macos::command_line::draw_command_line` and
/// `win::command_line::draw_command_line` used to carry independently.
///
/// # Divergences the three pre-#1083 copies had, closed here
///
/// - **Selection highlight.** Only the GTK copy
///   (`gtk::command_line::draw_command_line_selection`, #1001) ever
///   painted [`CommandLineLayout::selection_bounds`]'s rect — macOS's and
///   Windows's `Backend::draw_command_line_selection` both silently
///   ignored the `selection` argument and forwarded to the plain
///   `draw_command_line`, each with a doc comment admitting "no visual
///   highlight yet ... issue #1001 scoped the paint work to GTK/Cairo and
///   TUI/ratatui." [`paint`] below always paints the highlight (a no-op
///   when `selection` is `None` or empty), so all three backends now
///   agree.
/// - **Insert-cursor x-position.** GTK measured a real glyph-prefix width
///   against its own paint `pango::Layout` and anchored at `x +
///   prefix_width`, ignoring any right-align shift — a bug the macOS
///   module doc calls out explicitly ("only ever visible for a
///   right-aligned command line that also carries a cursor ... the bug
///   has never bitten"). macOS instead anchored at `text_x +
///   prefix_width` (right-align-correct); Windows already used the
///   shared [`CommandLineLayout::char_bounds`] fixed-advance column,
///   independent of any per-backend glyph measurement. [`paint`] adopts
///   Windows's approach uniformly (`layout.char_bounds(offset)`) — it
///   agrees with every existing left-aligned call site (the only shape
///   any live caller uses today) and is simply correct for the
///   right-aligned case none of them exercised.
/// - **Zero-size guard.** macOS and Windows both short-circuited a
///   non-positive `rect.width`/`rect.height` to a no-paint layout; GTK
///   had no equivalent guard (relying on Cairo's own no-op fill/clip for
///   a degenerate rect). [`paint`] applies the guard uniformly, matching
///   `primitives::status_bar::native_surface_paint::paint`'s precedent
///   for the same shape (#860).
/// - **Clipping.** Only macOS clipped painting to `rect` before this;
///   [`paint`] pushes/pops a clip on every backend, so an over-long or
///   right-aligned string that overflows the bar can no longer bleed
///   past its edges on GTK/Windows.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{CommandLine, CommandLineLayout, CommandLineMeasure};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Insert-cursor width in surface-native units (px/DIP/pt) — matches
    /// every pre-#1083 per-backend copy's 2-unit bar
    /// (`gtk::command_line`'s hardcoded `2.0`, `macos::command_line::CURSOR_W`,
    /// `win::command_line::CURSOR_W_DIP`).
    const CURSOR_WIDTH: f32 = 2.0;

    /// Paint a [`CommandLine`] into `rect` on `surface`, highlighting
    /// `selection` (a `(start, end)` byte-offset pair, either order) if
    /// given, and return the resolved [`CommandLineLayout`] — same
    /// contract as [`crate::Backend::draw_command_line_selection`]. A
    /// `None` (or empty/zero-width) `selection` paints identically to
    /// [`crate::Backend::draw_command_line`].
    ///
    /// A non-positive `rect.width`/`rect.height` short-circuits to the
    /// no-paint layout without touching `surface` at all — see this
    /// module's doc, "Zero-size guard".
    pub(crate) fn paint(
        cmd: &CommandLine,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
        char_width: f32,
        selection: Option<(usize, usize)>,
    ) -> CommandLineLayout {
        let layout = cmd.layout(rect, CommandLineMeasure::new(char_width));

        if rect.width <= 0.0 || rect.height <= 0.0 {
            return layout;
        }

        surface.surface_push_clip(rect);
        surface.surface_fill_rect(rect, theme.command_line_bg);

        if let Some(sel) = selection {
            if let Some(r) = layout.selection_bounds(sel) {
                surface.surface_fill_rect_alpha(r, theme.selection, theme.selection_alpha);
            }
        }

        if !cmd.text.is_empty() {
            // `layout.text_origin_x` already carries `rect.x` and any
            // right-align shift (issue #505 convention: absolute, not
            // rect-local) — paint starting there, bounded to the
            // remainder of the bar so an over-long or right-aligned
            // string can't paint past `rect`'s far edge (the clip above
            // additionally stops it bleeding past every edge).
            let text_rect = Rect::new(
                layout.text_origin_x,
                rect.y,
                (rect.x + rect.width - layout.text_origin_x).max(0.0),
                rect.height,
            );
            surface.surface_draw_text_run(text_rect, &cmd.text, theme.command_line_fg);

            if let Some(offset) = cmd.cursor_offset {
                let col_rect = layout.char_bounds(offset);
                let cursor_rect = Rect::new(col_rect.x, col_rect.y, CURSOR_WIDTH, col_rect.height);
                surface.surface_fill_rect(cursor_rect, theme.cursor);
            }
        }

        surface.surface_pop_clip();
        layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every surface verb this primitive's paint uses —
        /// mirrors `primitives::status_bar`'s identical test double, so
        /// this test runs on any host without Cairo/Core Graphics/
        /// Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> crate::Viewport {
                crate::Viewport::new(200.0, 100.0, 1.0)
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
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clip_pushes.push(rect);
            }
            fn surface_pop_clip(&mut self) {
                self.clip_pops += 1;
            }
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn sample(text: &str, cursor: Option<usize>, right_align: bool) -> CommandLine {
            CommandLine {
                id: WidgetId::new("cmdline"),
                text: text.into(),
                cursor_offset: cursor,
                right_align,
            }
        }

        #[test]
        fn fills_background_then_text_then_cursor() {
            let cmd = sample(":wq", Some(1), false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 100.0, 20.0);
            let layout = paint(&cmd, &mut surface, &theme, rect, 8.0, None);

            assert_eq!(surface.fills[0], (rect, theme.command_line_bg));
            assert_eq!(surface.text_runs.len(), 1);
            assert_eq!(surface.text_runs[0].1, ":wq");
            assert_eq!(surface.text_runs[0].2, theme.command_line_fg);

            // Cursor rect is the second fill, at char_bounds(1).
            let cursor_rect = layout.char_bounds(1);
            assert_eq!(surface.fills[1].0.x, cursor_rect.x);
            assert_eq!(surface.fills[1].1, theme.cursor);
        }

        #[test]
        fn no_selection_paints_no_highlight() {
            let cmd = sample(":wq", None, false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &cmd,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 20.0),
                8.0,
                None,
            );
            // Only the background fill — no selection, no cursor.
            assert_eq!(surface.fills.len(), 1);
        }

        /// Closes the divergence documented on [`paint`]'s own doc: only
        /// `gtk::command_line` used to paint a selection highlight —
        /// macOS and Windows both dropped the `selection` argument
        /// entirely. This is the RED-before-the-port case: on the old
        /// `macos::command_line::draw_command_line`/`win::command_line::draw_command_line`
        /// (which never took a `selection` argument at all), there was no
        /// way to even ask for this, let alone assert it painted.
        #[test]
        fn selection_paints_highlight_before_text() {
            let cmd = sample(":wq!", None, false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 100.0, 20.0);
            let layout = paint(&cmd, &mut surface, &theme, rect, 8.0, Some((0, 3)));

            let sel_rect = layout.selection_bounds((0, 3)).unwrap();
            // Highlight fill (index 1) comes after the background (index
            // 0) and before the text run.
            assert_eq!(surface.fills.len(), 2);
            assert_eq!(surface.fills[1].0, sel_rect);
            assert_eq!(
                surface.fills[1].1,
                theme.selection.with_alpha(theme.selection_alpha as f64)
            );
        }

        #[test]
        fn empty_selection_range_paints_no_highlight() {
            let cmd = sample(":wq!", None, false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &cmd,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 20.0),
                8.0,
                Some((2, 2)),
            );
            assert_eq!(surface.fills.len(), 1);
        }

        /// Closes the other documented divergence: GTK anchored the
        /// cursor at a real glyph-prefix width ignoring right-align;
        /// this shared `paint` anchors at the shared
        /// `CommandLineLayout::char_bounds`, which *does* account for
        /// the right-align shift.
        #[test]
        fn cursor_respects_right_align_shift() {
            let cmd = sample("3/17", Some(2), true);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
            let layout = paint(&cmd, &mut surface, &theme, rect, 1.0, None);

            let cursor_rect = layout.char_bounds(2);
            assert!(
                cursor_rect.x > rect.x,
                "right-aligned text's cursor must shift right of the bar origin"
            );
            let cursor_fill = surface
                .fills
                .iter()
                .find(|(r, c)| *c == theme.cursor && r.x == cursor_rect.x)
                .expect("cursor should paint at char_bounds(2)");
            let _ = cursor_fill;
        }

        #[test]
        fn zero_size_rect_paints_nothing() {
            let cmd = sample(":wq", Some(1), false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &cmd,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 0.0, 20.0),
                8.0,
                Some((0, 1)),
            );
            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
            assert!(surface.clip_pushes.is_empty());
            assert_eq!(surface.clip_pops, 0);
        }

        #[test]
        fn clip_pushed_and_popped_once_around_paint() {
            let cmd = sample(":wq", None, false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(3.0, 4.0, 50.0, 20.0);
            paint(&cmd, &mut surface, &theme, rect, 8.0, None);
            assert_eq!(surface.clip_pushes, vec![rect]);
            assert_eq!(surface.clip_pops, 1);
        }

        /// Multibyte regression (#503's class of bug): a `cursor_offset`
        /// landing mid-character must not panic — `char_bounds` snaps via
        /// `column_for_byte_offset`, not a raw string slice.
        #[test]
        fn multibyte_cursor_offset_does_not_panic() {
            let text = ":éditer";
            assert!(!text.is_char_boundary(2));
            let cmd = sample(text, Some(2), false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &cmd,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 20.0),
                8.0,
                None,
            );
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
