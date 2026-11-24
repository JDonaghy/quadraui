//! `CommandLine` primitive: a single-line input/output surface for editor
//! command prompts (`:`, `/`, `?`) and transient messages.
//!
//! Display-only �� the engine handles keystroke input and updates
//! `text` / `cursor_offset` each frame. Both TUI and GTK rasterisers
//! draw text with an optional insert cursor; alignment can be flipped
//! for right-aligned count displays.
//!
//! [`CommandLineLayout`] closes the character-offset hit-test
//! gap that made the command line mouse-selectable on TUI (via a
//! terminal-only inverted-cell read-back trick) and structurally unable to
//! be on GTK. `CommandLine::layout` resolves click/selection geometry the
//! same way every other primitive does — see [`CommandLineLayout::hit_test`]
//! and [`CommandLineLayout::selection_bounds`].

use crate::event::Rect;
use crate::types::WidgetId;
use crate::Handled;
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
    /// by hand.
    pub fn from_metrics(m: &crate::backend::Metrics) -> Self {
        Self::new(m.char_width)
    }
}

/// Fully-resolved layout for a `CommandLine`.
///
/// Provides character-offset hit testing and selection bounds for all backends.
/// [`Self::hit_test`] maps a click x-coordinate to a **byte offset** into
/// [`CommandLine::text`] (matching [`CommandLine::cursor_offset`]'s contract),
/// and [`Self::selection_bounds`] turns a `(start, end)` byte-offset pair into a
/// paintable rect so a host can render a drag-selection highlight.
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
    /// Map an absolute x-coordinate to a byte offset into the source `text`,
    /// clamped to `[0, text.len()]`. Points left of the first character return `0`;
    /// points at or right of the last column return `text.len()`.
    ///
    /// Coordinate frame: **ABSOLUTE** — `x` is compared directly against
    /// `text_origin_x`, which already carries `bounds.x` and any right-align shift.
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

    /// Rect spanning the selection `[start, end)` (byte offsets, either order).
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

/// Shared [`PaintSurface`](crate::paint_surface::PaintSurface)-backed paint
/// for [`CommandLine`]. Unifies selection highlight rendering across backends:
/// all backends paint [`CommandLineLayout::selection_bounds`]'s rect,
/// clipped to the command bar bounds, with insert-cursor positioning via
/// [`CommandLineLayout::char_bounds`], and a 1px `border_fg` frame around
/// the bar (every GUI single-line input's shared convention).
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

    /// Insert-cursor width in surface-native units (px/DIP/pt): 2.0.
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

        surface.surface_fill_rect(rect, theme.command_line_bg);
        // 1px frame, matching `text_input`/`form`'s bordered single-line
        // input convention — TUI keeps its own borderless bottom-row
        // rasteriser (`tui::command_line`), this module is GUI-only.
        surface.surface_stroke_rect(rect, theme.border_fg, 1.0);

        surface.surface_push_clip(rect);

        if let Some(sel) = selection {
            if let Some(r) = layout.selection_bounds(sel) {
                surface.surface_fill_rect_alpha(r, theme.selection, theme.selection_alpha);
            }
        }

        if !cmd.text.is_empty() {
            // `layout.text_origin_x` is an absolute coordinate carrying `rect.x` and any
            // right-align shift; paint starting there, bounded to the remainder of the bar.
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
            strokes: Vec<(Rect, Color, f32)>,
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
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
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
        fn paints_a_1px_border_frame() {
            let cmd = sample(":wq", None, false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 100.0, 20.0);
            paint(&cmd, &mut surface, &theme, rect, 8.0, None);

            assert_eq!(
                surface.strokes,
                vec![(rect, theme.border_fg, 1.0)],
                "command line should frame its full bar in a 1px border"
            );
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

        /// Selection highlight painting: unifies behavior across backends.
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

        /// A `cursor_offset` landing mid-character must not panic: `char_bounds`
        /// snaps via `column_for_byte_offset`, not a raw string slice.
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

// ── Cell-grid layout + paint ───────────────────────────────────────────

/// Resolve this bar's layout in **whole cell** units — the one cell-grid
/// entry point [`crate::primitive::Primitive::layout`] and
/// [`crate::tui::command_line::tui_command_line_layout`] share, so the widget contract
/// and the legacy `Backend::draw_command_line` path can never disagree
/// about which column a character sits in.
pub fn cell_layout(cmd: &CommandLine, rect: Rect) -> CommandLineLayout {
    cmd.layout(rect, CommandLineMeasure::new(1.0))
}

/// `CommandLine`'s cell-grid rasteriser, written against
/// [`crate::CellSurface`]'s verbs — the cell-grid twin of
/// [`native_surface_paint::paint`].
///
/// One borderless row filled with [`crate::Theme::command_line_bg`], the
/// text painted in [`crate::Theme::command_line_fg`] (pushed to the far
/// edge when [`CommandLine::right_align`]), the cell columns a selection
/// covers tinted [`crate::Theme::selection`], and a block cursor at
/// [`CommandLine::cursor_offset`] made by swapping that cell's own
/// foreground and background.
///
/// The selection is a parameter rather than a second entry point: one
/// painter, driven by whatever selection state its caller holds.
/// `#[cfg(feature = "tui")]`: `crate::tui`'s own delegate is this
/// painter's only caller, so a pixel-only or featureless build would
/// compile it unused — and CI's `-D warnings` legs turn that into a hard
/// error.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{CommandLine, CommandLineLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::text_util::safe_prefix;
    use crate::theme::Theme;

    /// Paint `cmd` at the already-resolved `layout` onto `surface`,
    /// highlighting the columns `selection` (a `(start, end)` byte-offset
    /// pair, either order) covers.
    pub(crate) fn paint(
        cmd: &CommandLine,
        layout: &CommandLineLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        selection: Option<(usize, usize)>,
    ) {
        let area = layout.bounds;
        if area.width < 1.0 || area.height < 1.0 {
            return;
        }
        let fg = theme.command_line_fg;
        let bg = theme.command_line_bg;
        let sel_bg = theme.selection;

        // Absolute (area-relative-origin) column range covered by the
        // selection, if any — `char_width` is `1.0` here (one cell per
        // character), so `CommandLineLayout::selection_bounds`'s rect
        // `x`/`width` are already whole cell columns.
        let sel_cols = selection.and_then(|sel| {
            layout.selection_bounds(sel).map(|r| {
                let start = r.x.round() as u16;
                let end = (r.x + r.width).round() as u16;
                (start, end)
            })
        });
        let style_for = |x: u16| CellStyle {
            fg: Some(fg),
            bg: Some(match sel_cols {
                Some((start, end)) if x >= start && x < end => sel_bg,
                _ => bg,
            }),
            ..CellStyle::default()
        };

        let x0 = area.x as u16;
        let row_y = area.y as u16;
        let cols = area.width as u16;
        for x in x0..x0 + cols {
            surface.put_char(x, row_y, ' ', style_for(x));
        }

        if cmd.text.is_empty() {
            return;
        }

        let chars: Vec<char> = cmd.text.chars().collect();
        let start_x = if cmd.right_align {
            let len = chars.len() as u16;
            if len > cols {
                // Too wide to right-align: the legacy rasteriser paints
                // nothing rather than cropping a right-aligned run.
                return;
            }
            x0 + cols - len
        } else {
            x0
        };
        for (x, &ch) in (start_x..).zip(chars.iter()) {
            if x >= x0 + cols {
                break;
            }
            surface.put_char(x, row_y, ch, style_for(x));
        }

        // Block cursor: swap the cell's own colours, so the glyph already
        // painted there stays legible in reverse video.
        if let Some(offset) = cmd.cursor_offset {
            let cursor_col = safe_prefix(&cmd.text, offset).chars().count() as u16;
            let cx = x0 + cursor_col.min(cols.saturating_sub(1));
            if cx < x0 + cols {
                let fallback = CellStyle {
                    fg: Some(fg),
                    bg: Some(bg),
                    ..CellStyle::default()
                };
                let (ch, under) = surface.get(cx, row_y).unwrap_or((' ', fallback));
                surface.put_char(
                    cx,
                    row_y,
                    ch,
                    CellStyle {
                        fg: under.bg.or(Some(bg)),
                        bg: under.fg.or(Some(fg)),
                        ..under
                    },
                );
            }
        }
    }
}

// ── Widget contract ────────────────────────────────────────────────────

/// [`crate::primitive::Primitive::Layout`] for [`CommandLine`]: the
/// absolute area the bar was laid out into, the resolved
/// [`CommandLineLayout`], and the selection the paint path highlights.
///
/// # Why `selection` is always `None` here
///
/// A selection range is host state ([`CommandLine`] itself carries no
/// field for one — see that type's doc), and
/// [`crate::primitive::Primitive::layout`] sees only the descriptor. A
/// host that tracks a drag-selection paints it by resolving
/// [`CommandLine::layout`] itself and calling the selection-aware painter
/// ([`crate::Backend::draw_command_line_selection`], which is the same
/// painter with the range filled in). The field exists on this layout so
/// that painter has exactly one source for the range, rather than a
/// second draw entry point.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct CommandLinePrimitiveLayout {
    /// The absolute rect the bar was laid out into.
    pub area: Rect,
    /// The resolved bar layout, in absolute coordinates.
    pub layout: CommandLineLayout,
    /// The selection to highlight, as a `(start, end)` byte-offset pair.
    pub selection: Option<(usize, usize)>,
}

impl CommandLinePrimitiveLayout {
    /// The byte offset into [`CommandLine::text`] an absolute `x`
    /// resolves to — see [`CommandLineLayout::hit_test`].
    pub fn hit_test(&self, x: f32) -> usize {
        self.layout.hit_test(x)
    }
}

impl crate::primitive::Primitive for CommandLine {
    type Layout = CommandLinePrimitiveLayout;
    type Event = usize;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::CommandLine(self.clone())
    }

    /// Exactly one row tall, as wide as the text — a command bar
    /// normally stretches, so the width is advisory only.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let width = self.text.chars().count() as f32 * ctx.char_width.max(1.0);
        crate::NaturalSize::new(Some(width.min(avail.width)), Some(ctx.line_height.max(1.0)))
    }

    /// `rect` is the bar's own one-row strip. On a cell grid the layout
    /// comes from [`cell_layout`]; on a pixel surface from
    /// [`CommandLine::layout`] with the backend's own monospace advance.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> CommandLinePrimitiveLayout {
        let layout = match ctx.units() {
            crate::primitive::Units::Cells => cell_layout(self, rect),
            _ => self.layout(rect, CommandLineMeasure::new(ctx.char_width)),
        };
        CommandLinePrimitiveLayout {
            area: rect,
            layout,
            selection: None,
        }
    }

    /// The shared pixel painter — the same one every pixel backend's
    /// `draw_command_line`/`draw_command_line_selection` routes through,
    /// with the selection taken from the layout rather than a separate
    /// draw call.
    fn paint(
        &self,
        layout: &CommandLinePrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        _style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            native_surface_paint::paint(
                self,
                surface,
                theme,
                layout.area,
                layout.layout.char_width,
                layout.selection,
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::command_line::draw_command_line_selection`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`. That
    /// entry point re-resolves the layout from the same area through the
    /// same [`cell_layout`] this `Primitive` laid out with, so the glyphs
    /// land exactly where `layout` says they do.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &CommandLinePrimitiveLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        let area = layout.area;
        crate::tui::command_line::draw_command_line_selection(
            buf,
            ratatui::layout::Rect::new(
                area.x as u16,
                area.y as u16,
                area.width as u16,
                area.height as u16,
            ),
            self,
            theme,
            layout.selection,
        );
    }
}

/// [`crate::interactive::Interactive::State`] for [`CommandLine`]: the
/// caret and selection anchor this widget's own key handling moves, as
/// byte offsets into [`CommandLine::text`].
///
/// Same division of labour as
/// [`crate::primitives::text_input::TextInputState`] — see that type's
/// doc for why the caret is interaction state rather than something read
/// back off the descriptor.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommandLineState {
    /// The caret byte offset this widget's own editing has moved to, or
    /// `None` while it still follows [`CommandLine::cursor_offset`].
    pub cursor: Option<usize>,
    /// The selection's fixed end, as a byte offset.
    pub anchor: Option<usize>,
}

/// What a [`CommandLine`]'s own
/// [`crate::interactive::Interactive::handle`] resolves one gesture to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandLineAction {
    /// The text changed — a keystroke or a cut. Carries the whole new
    /// text, the shape
    /// [`crate::primitives::form::FormEvent::TextInputChanged`] already
    /// names.
    Changed(String),
    /// Enter submitted the command.
    Submitted(String),
    /// Escape abandoned the command.
    Cancelled,
    /// Ctrl-C (or the copy half of Ctrl-X) put the selection on the
    /// clipboard.
    Copied(String),
}

impl crate::interactive::WidgetAction for CommandLineAction {
    /// Edits and submission ride [`crate::UiEvent::Form`]'s
    /// `TextInputChanged`/`TextInputCommitted` — the crate's one "a text
    /// field was edited / answered" channel. Escape has no such variant,
    /// so it reports as the unconsumed-key passthrough
    /// [`crate::primitives::form::FormEvent::KeyPressed`], which is what
    /// a `Form` does with a key it declines.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        use crate::primitives::form::FormEvent;
        match self {
            CommandLineAction::Changed(value) => {
                crate::UiEvent::Form(id, FormEvent::TextInputChanged { id, value })
            }
            CommandLineAction::Submitted(value) => {
                crate::UiEvent::Form(id, FormEvent::TextInputCommitted { id, value })
            }
            CommandLineAction::Cancelled => crate::UiEvent::Form(
                id,
                FormEvent::KeyPressed {
                    key: "Escape".into(),
                    modifiers: crate::types::Modifiers::default(),
                },
            ),
            CommandLineAction::Copied(text) => crate::UiEvent::TextCopied(text),
        }
    }
}

impl CommandLine {
    /// The caret one `handle` call starts from: whatever this widget's
    /// own state has walked to, else [`Self::cursor_offset`], else the
    /// end of the text — snapped to a char boundary either way.
    fn effective_cursor(&self, state: &CommandLineState) -> usize {
        crate::behaviour::text_edit::effective_flat_cursor(
            &self.text,
            state.cursor,
            self.cursor_offset,
        )
    }

    /// Apply one [`crate::EditOp`] against this bar's text and the
    /// caret/anchor in `state`, writing the resulting caret back.
    ///
    /// Returns the new text when it changed, `None` when the op only
    /// moved the caret or the selection.
    fn apply_op(&self, state: &mut CommandLineState, op: crate::EditOp) -> Option<String> {
        let (text, cursor, anchor) = crate::behaviour::text_edit::apply_flat_edit_op(
            &self.text,
            self.effective_cursor(state),
            state.anchor,
            op,
        );
        state.cursor = Some(cursor);
        state.anchor = anchor;
        (text != self.text).then_some(text)
    }

    /// The selected text, as the shared flat-value selection span
    /// resolves it against this bar's text and `state`'s caret/anchor.
    fn selection_text(&self, state: &CommandLineState) -> Option<String> {
        crate::behaviour::text_edit::flat_selected_text(
            &self.text,
            self.effective_cursor(state),
            state.anchor,
        )
    }

    /// Apply `op` and report it: a text change as
    /// [`CommandLineAction::Changed`], a caret-only move as
    /// [`Handled::Consumed`] plus a redraw request.
    fn commit(
        &self,
        state: &mut CommandLineState,
        op: crate::EditOp,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<CommandLineAction> {
        match self.apply_op(state, op) {
            Some(text) => Handled::Event(CommandLineAction::Changed(text)),
            None => {
                cx.request_redraw();
                Handled::Consumed
            }
        }
    }

    /// One keystroke, with Enter/Escape claimed ahead of the shared chord
    /// vocabulary (which maps Enter to a newline) and the Ctrl-chorded
    /// clipboard / select-all keys resolved by
    /// [`EditIntent::from_key`](crate::behaviour::text_edit::EditIntent::from_key).
    fn handle_key(
        &self,
        state: &mut CommandLineState,
        key: &crate::event::Key,
        modifiers: crate::types::Modifiers,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<CommandLineAction> {
        use crate::behaviour::text_edit::EditIntent;
        use crate::event::{Key, NamedKey};
        use crate::EditOp;

        match key {
            // A command bar is one line: Enter submits rather than
            // splitting the text, so the newline `EditOp::from_key`
            // would resolve never reaches the buffer.
            Key::Named(NamedKey::Enter) => {
                return Handled::Event(CommandLineAction::Submitted(self.text.clone()));
            }
            Key::Named(NamedKey::Escape) => return Handled::Event(CommandLineAction::Cancelled),
            _ => {}
        }
        match EditIntent::from_key(key, modifiers) {
            Some(EditIntent::SelectAll) => self.commit(state, EditOp::SelectAll, cx),
            Some(EditIntent::Copy) => match self.selection_text(state) {
                Some(text) => Handled::Event(CommandLineAction::Copied(text)),
                None => Handled::Consumed,
            },
            Some(EditIntent::Cut) => match self.selection_text(state) {
                Some(text) => {
                    cx.emit(crate::UiEvent::TextCopied(text));
                    self.commit(state, EditOp::DeleteSelection, cx)
                }
                None => Handled::Consumed,
            },
            // Paste is the host's: the clipboard text arrives as
            // `UiEvent::ClipboardPaste`, which a host feeds back in as
            // `RoutedEvent::Text`.
            Some(EditIntent::Paste) => Handled::Ignored,
            Some(EditIntent::Op(op)) => self.commit(state, op, cx),
            None => Handled::Ignored,
        }
    }
}

impl crate::interactive::Interactive for CommandLine {
    type State = CommandLineState;
    type Action = CommandLineAction;

    /// One focus stop as a whole — the bar routes every keystroke
    /// itself.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    /// A screen reader can focus the bar and replace its whole text.
    fn a11y_actions(&self, _state: &CommandLineState) -> Vec<crate::a11y::A11yAction> {
        vec![
            crate::a11y::A11yAction::Focus,
            crate::a11y::A11yAction::SetValue,
        ]
    }

    /// Keystrokes go through [`crate::EditOp::from_key`] and the shared
    /// flat-value bridge, so this bar, [`crate::TextField`] and
    /// [`crate::TextEditor`] can never disagree about what a key means.
    /// Enter submits, Escape cancels, Ctrl-A/C/X select-all/copy/cut, a
    /// click parks the caret at the clicked character and a shift-click
    /// (or a drag) extends the selection to it.
    fn handle(
        &self,
        state: &mut CommandLineState,
        event: &crate::interactive::RoutedEvent,
        layout: &CommandLinePrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<CommandLineAction> {
        use crate::interactive::{PointerKind, RoutedEvent};

        match event {
            RoutedEvent::Key(key) => self.handle_key(state, &key.key, key.modifiers, cx),
            RoutedEvent::Text(text) => {
                self.commit(state, crate::EditOp::InsertText(text.clone()), cx)
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                local,
                mods,
                ..
            } => {
                cx.request_focus(None);
                cx.capture_pointer();
                let offset = layout.hit_test(layout.area.x + local.x);
                self.move_caret_to(state, offset, mods.shift, cx)
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } if cx.is_focused(None) => {
                // A drag with the button still down extends the
                // selection; the engine only routes a move here while
                // this widget holds the pointer capture taken above.
                let offset = layout.hit_test(layout.area.x + local.x);
                self.move_caret_to(state, offset, true, cx)
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Up(crate::event::MouseButton::Left),
                ..
            } => {
                cx.release_pointer();
                Handled::Consumed
            }
            RoutedEvent::Action(crate::a11y::A11yAction::Focus) => {
                cx.request_focus(None);
                Handled::Consumed
            }
            RoutedEvent::FocusIn(_) => {
                // Re-seed from the descriptor: the host may have moved
                // the caret while this bar was unfocused.
                state.cursor = Some(self.cursor_offset.unwrap_or(self.text.len()));
                state.anchor = None;
                Handled::Consumed
            }
            RoutedEvent::FocusOut => {
                state.cursor = None;
                state.anchor = None;
                Handled::Consumed
            }
            _ => Handled::Ignored,
        }
    }
}

impl CommandLine {
    /// Park the caret at byte offset `offset`, extending the selection
    /// when `extend`.
    fn move_caret_to(
        &self,
        state: &mut CommandLineState,
        offset: usize,
        extend: bool,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<CommandLineAction> {
        let (_, col) = crate::behaviour::text_edit::byte_offset_to_line_col(&self.text, offset);
        self.commit(
            state,
            crate::EditOp::SetCursor {
                line: 0,
                col,
                extend,
            },
            cx,
        )
    }
}

/// Routes a headless `TuiDriver` test can't reach: the
/// assistive-technology action path, a host-driven focus transition, and
/// the drag that extends a selection.
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
    /// module's own `Interactive` tests use. `focused` answers
    /// [`EventHost::is_focused`] so the drag arm, which only runs while
    /// the bar holds focus, is reachable.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
        emitted: Vec<crate::UiEvent>,
        captures: usize,
        releases: usize,
        focused: bool,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            self.focused
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {
            self.captures += 1;
        }
        fn release_pointer(&mut self) {
            self.releases += 1;
        }
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, event: crate::UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn bar(text: &str) -> CommandLine {
        CommandLine {
            id: WidgetId::new("cmdline"),
            text: text.into(),
            cursor_offset: Some(text.len()),
            right_align: false,
        }
    }

    fn area() -> Rect {
        Rect::new(0.0, 23.0, 40.0, 1.0)
    }

    fn layout_of(cmd: &CommandLine) -> CommandLinePrimitiveLayout {
        Primitive::layout(cmd, &LayoutCtx::new(1.0, 1.0, false), area())
    }

    fn key(key: Key, modifiers: Modifiers) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key,
            modifiers,
            repeat: false,
        })
    }

    fn pointer(kind: PointerKind, x: f32, mods: Modifiers) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point::new(x, 0.0),
            mods,
        }
    }

    #[test]
    fn a_command_line_is_one_focus_stop() {
        assert_eq!(Interactive::focus(&bar(":wq")), FocusSpec::Stop);
    }

    #[test]
    fn enter_submits_and_escape_cancels() {
        let cmd = bar(":wq");
        let layout = layout_of(&cmd);
        let mut host = MockHost::default();
        let mut state = CommandLineState::default();
        let mut cx = EventCx::new(cmd.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &cmd,
                &mut state,
                &key(Key::Named(NamedKey::Enter), Modifiers::default()),
                &layout,
                &mut cx
            ),
            Handled::Event(CommandLineAction::Submitted(":wq".into()))
        );
        assert_eq!(
            Interactive::handle(
                &cmd,
                &mut state,
                &key(Key::Named(NamedKey::Escape), Modifiers::default()),
                &layout,
                &mut cx
            ),
            Handled::Event(CommandLineAction::Cancelled)
        );
    }

    #[test]
    fn typing_appends_at_the_caret() {
        let cmd = bar(":w");
        let layout = layout_of(&cmd);
        let mut host = MockHost::default();
        let mut state = CommandLineState::default();
        let mut cx = EventCx::new(cmd.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &cmd,
                &mut state,
                &key(Key::Char('q'), Modifiers::default()),
                &layout,
                &mut cx
            ),
            Handled::Event(CommandLineAction::Changed(":wq".into()))
        );
    }

    #[test]
    fn a_drag_extends_the_selection_from_the_press_position() {
        let cmd = bar(":wqall");
        let layout = layout_of(&cmd);
        let mut host = MockHost {
            focused: true,
            ..Default::default()
        };
        let mut state = CommandLineState::default();
        let mut cx = EventCx::new(cmd.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &cmd,
                &mut state,
                &pointer(
                    PointerKind::Down(MouseButton::Left),
                    1.0,
                    Modifiers::default(),
                ),
                &layout,
                &mut cx,
            ),
            Handled::Consumed
        );
        assert_eq!(state.cursor, Some(1));
        assert_eq!(
            Interactive::handle(
                &cmd,
                &mut state,
                &pointer(PointerKind::Move, 4.0, Modifiers::default()),
                &layout,
                &mut cx,
            ),
            Handled::Consumed
        );
        assert_eq!(state.cursor, Some(4));
        assert_eq!(state.anchor, Some(1));
        assert_eq!(
            Interactive::handle(
                &cmd,
                &mut state,
                &pointer(
                    PointerKind::Up(MouseButton::Left),
                    4.0,
                    Modifiers::default(),
                ),
                &layout,
                &mut cx,
            ),
            Handled::Consumed
        );
        assert_eq!((host.captures, host.releases), (1, 1));
    }

    #[test]
    fn ctrl_x_emits_the_copy_alongside_the_shortened_text() {
        let cmd = bar(":wq");
        let layout = layout_of(&cmd);
        let mut host = MockHost::default();
        let mut state = CommandLineState::default();
        let mut cx = EventCx::new(cmd.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &cmd,
                &mut state,
                &key(
                    Key::Char('a'),
                    Modifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                &layout,
                &mut cx,
            ),
            Handled::Consumed
        );
        let handled = Interactive::handle(
            &cmd,
            &mut state,
            &key(
                Key::Char('x'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(CommandLineAction::Changed(String::new()))
        );
        assert_eq!(host.emitted, vec![crate::UiEvent::TextCopied(":wq".into())]);
    }

    #[test]
    fn an_a11y_focus_action_requests_focus_on_the_bar_itself() {
        let cmd = bar(":wq");
        let layout = layout_of(&cmd);
        let mut host = MockHost::default();
        let mut state = CommandLineState::default();
        let mut cx = EventCx::new(cmd.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &cmd,
                &mut state,
                &RoutedEvent::Action(A11yAction::Focus),
                &layout,
                &mut cx
            ),
            Handled::Consumed
        );
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn focus_in_seeds_the_caret_from_the_descriptor_and_focus_out_releases_it() {
        let mut cmd = bar(":wq");
        cmd.cursor_offset = Some(1);
        let layout = layout_of(&cmd);
        let mut host = MockHost::default();
        let mut state = CommandLineState {
            cursor: Some(3),
            anchor: Some(2),
        };
        let mut cx = EventCx::new(cmd.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &cmd,
                &mut state,
                &RoutedEvent::FocusIn(None),
                &layout,
                &mut cx
            ),
            Handled::Consumed
        );
        assert_eq!(state.cursor, Some(1));
        assert_eq!(state.anchor, None);
        assert_eq!(
            Interactive::handle(&cmd, &mut state, &RoutedEvent::FocusOut, &layout, &mut cx),
            Handled::Consumed
        );
        assert_eq!(state.cursor, None);
    }

    #[test]
    fn the_contract_layout_carries_no_selection_of_its_own() {
        let cmd = bar(":wq");
        assert_eq!(layout_of(&cmd).selection, None);
    }

    #[test]
    fn a_pixel_layout_hit_tests_with_the_backends_char_width() {
        let cmd = bar(":wq");
        let layout = Primitive::layout(
            &cmd,
            &LayoutCtx::new(16.0, 8.0, false),
            Rect::new(0.0, 0.0, 320.0, 20.0),
        );
        assert_eq!(layout.layout.char_width, 8.0);
        assert_eq!(layout.hit_test(8.0), 1);
    }

    #[test]
    fn measure_reports_one_row_and_the_texts_own_width() {
        let cmd = bar(":wq");
        let size = Primitive::measure(
            &cmd,
            &LayoutCtx::new(1.0, 1.0, false),
            crate::primitive::Size::new(40.0, 40.0),
        );
        assert_eq!(size.width, Some(3.0));
        assert_eq!(size.height, Some(1.0));
    }
}

/// Cell-grid paint tests for [`cell_paint`], driven through
/// [`crate::tui::command_line::draw_command_line`] — the delegate that
/// owns the `Buffer` → [`crate::CellSurface`] adapter the painter needs.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use crate::primitives::command_line::CommandLine;
    use crate::theme::Theme;
    use crate::tui::command_line::{draw_command_line, draw_command_line_selection};
    use crate::types::{Color, WidgetId};

    /// The ratatui colour `crate::tui`'s own cell writers land for a
    /// quadraui [`Color`] — spelled here because that crate-private
    /// conversion isn't reachable from a widget module.
    fn ratatui_color(c: Color) -> ratatui::style::Color {
        ratatui::style::Color::Rgb(c.r, c.g, c.b)
    }

    /// `cursor_offset` is a host-supplied byte offset with no guarantee it
    /// lands on a char boundary; it must be validated to avoid slicing into
    /// a multibyte character.
    #[test]
    fn draw_command_line_with_multibyte_cursor_does_not_panic() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let theme = Theme::default();

        // ":éditer" — byte 2 sits inside the 2-byte 'é' (starts at byte 1).
        let text = ":éditer";
        assert!(!text.is_char_boundary(2));
        let cmd = CommandLine {
            id: WidgetId::new("cmdline"),
            text: text.into(),
            cursor_offset: Some(2),
            right_align: false,
        };

        // Must not panic.
        draw_command_line(&mut buf, Rect::new(0, 0, 20, 1), &cmd, &theme);
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    /// Paint and click round-trip: paint via the real `draw_command_line`
    /// rasteriser, read back the actual painted cell character from the
    /// `Buffer`, then `hit_test` that exact painted coordinate and assert it
    /// resolves to that character's byte offset. This verifies that
    /// `draw_command_line`'s glyph placement stays in sync with
    /// `CommandLine::layout`'s column formula.
    ///
    /// Tested at a nonzero origin (`area.x == 5`) to verify that `hit_test`
    /// correctly ignores clicks left of the bar and maps columns starting at
    /// `area.x`, not `0`.
    #[test]
    fn tui_command_line_paint_and_click_round_trip_at_nonzero_origin() {
        let area = Rect::new(5, 3, 20, 1);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let cmd = CommandLine {
            id: WidgetId::new("cmdline"),
            text: ":wq".into(),
            cursor_offset: None,
            right_align: false,
        };

        let layout = draw_command_line(&mut buf, area, &cmd, &theme);

        // Column 0 (':') is actually painted at the absolute origin.
        assert_eq!(cell_char(&buf, 5, 3), ':');
        // Column 1 ('w') is actually painted one cell to the right.
        assert_eq!(cell_char(&buf, 6, 3), 'w');

        // hit_test at the real painted position of 'w' (a click lands
        // somewhere inside the cell, not necessarily at its left edge)
        // must resolve to 'w's byte offset (1) in ":wq".
        assert_eq!(layout.hit_test(6.5), 1);

        // A click left of the bar clamps to the first column's byte offset.
        assert_eq!(layout.hit_test(0.0), 0);
        // Column 0 starts at x == area.x == 5.
        assert_eq!(layout.hit_test(5.0), 0);
    }

    /// `draw_command_line_selection` highlights exactly the selected
    /// cells with `theme.selection` as background, leaves the rest at
    /// `theme.command_line_bg`, and `draw_command_line` (no selection
    /// argument) stays unaffected.
    #[test]
    fn tui_command_line_selection_highlights_only_selected_cells() {
        let area = Rect::new(5, 3, 20, 1);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let cmd = CommandLine {
            id: WidgetId::new("cmdline"),
            text: ":wq!".into(),
            cursor_offset: None,
            right_align: false,
        };

        // Select ":w" -> byte offsets 0..2 -> columns 0,1 -> cells x=5,6.
        let layout = draw_command_line_selection(&mut buf, area, &cmd, &theme, Some((0, 2)));
        assert_eq!(cell_char(&buf, 5, 3), ':');
        assert_eq!(cell_char(&buf, 6, 3), 'w');

        let sel_bg = ratatui_color(theme.selection);
        let plain_bg = ratatui_color(theme.command_line_bg);
        assert_eq!(
            buf[(5, 3)].bg,
            sel_bg,
            "selected column 0 should carry the selection bg"
        );
        assert_eq!(
            buf[(6, 3)].bg,
            sel_bg,
            "selected column 1 should carry the selection bg"
        );
        assert_eq!(
            buf[(7, 3)].bg,
            plain_bg,
            "unselected column 2 ('q') should keep the plain command-line bg"
        );
        assert_eq!(
            buf[(8, 3)].bg,
            plain_bg,
            "unselected column 3 ('!') should keep the plain command-line bg"
        );

        // Sanity: the returned layout's own geometry agrees.
        let r = layout.selection_bounds((0, 2)).unwrap();
        assert_eq!((r.x, r.width), (5.0, 2.0));

        // `draw_command_line` (no selection) must remain unaffected.
        let mut buf2 = Buffer::empty(area);
        draw_command_line(&mut buf2, area, &cmd, &theme);
        assert_eq!(
            buf2[(5, 3)].bg,
            plain_bg,
            "draw_command_line (no selection arg) must not highlight anything"
        );
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

    // ── CommandLineLayout ───────────────────────────────────────────────

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
        // A LOCAL/ABSOLUTE mixup is invisible at rect.x == 0, so this
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
        // ":éditer" — 'é' is 2 bytes; byte offsets after it must skip a byte per
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
