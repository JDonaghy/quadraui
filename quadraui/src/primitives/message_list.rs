//! `MessageList` primitive: a scrollable list of styled lines used by
//! chat-style panels (e.g. an AI assistant sidebar).
//!
//! Each row carries its own foreground colour and a small left-indent
//! offset so role labels (`You:` / `AI:`) line up flush-left while
//! content lines indent. The panel background is supplied by the
//! caller — message bodies share one fill. Per-message bg highlighting
//! could be added later as an optional `bg_override` field; the current
//! shape mirrors what both rasterisers emit today.
//!
//! Wrapping happens at the call site (a host's adapter splits message
//! content into wrap-width chunks before pushing rows) — the primitive
//! is data-only.
//!
//! # Styled rows
//!
//! Rows produced by the chat-controller's markdown path carry a non-empty
//! `spans` vector and a `scale` factor.  Backends that support rich text
//! (GTK via Pango attributes, TUI via ratatui modifiers) use those fields;
//! backends that don't fall back to `text` + `fg`.  The invariant is:
//!
//! * `spans.is_empty()` → render exactly as before (unchanged output on
//!   every backend).
//! * `spans` non-empty → render each span in its own fg / bold / italic;
//!   apply `scale` (GTK `AttrFloat::new_scale`, TUI ignores it).

use crate::event::Rect;
use crate::types::{Color, StyledSpan, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// A single row in a [`MessageList`].
///
/// `indent` is in surface units — TUI cells or GTK pixels — so the
/// caller picks the unit appropriate for its rasteriser.
///
/// When `spans` is **empty** the row is rendered with the flat `text` +
/// `fg` path — output is byte-for-byte identical to pre-styled-row
/// behaviour, preserving existing callers.  When `spans` is non-empty
/// the rasteriser applies per-span fg/bold/italic and the per-row `scale`
/// multiplier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRow {
    pub text: String,
    pub fg: Color,
    /// Left-indent offset in surface units (cells / pixels).
    #[serde(default)]
    pub indent: f32,
    /// Per-span rich styling.  Empty for flat rows (the `new` path).
    /// Non-empty for styled rows produced by the markdown transcript path.
    #[serde(default)]
    pub spans: Vec<StyledSpan>,
    /// Font-scale multiplier (`1.0` = body, `2.0`/`1.5`/`1.2` for H1/H2/H3).
    /// GTK applies this via `pango::AttrFloat::new_scale`; TUI ignores it
    /// (terminal cells have no variable character size).
    #[serde(default = "MessageRow::default_scale")]
    pub scale: f32,
}

impl MessageRow {
    /// Construct a flat row. `spans` is empty and `scale` is `1.0`.
    /// Output on every backend is **identical** to the pre-styled-row
    /// behaviour — this is the safe "no change" path for existing callers.
    pub fn new(text: impl Into<String>, fg: Color, indent: f32) -> Self {
        Self {
            text: text.into(),
            fg,
            indent,
            spans: Vec::new(),
            scale: 1.0,
        }
    }

    /// Construct a styled row from a [`StyledText`].
    ///
    /// `text` is set to the concatenation of all span texts (the plain-text
    /// fallback used by backends that don't read `spans`).  `fg` is the
    /// fallback foreground applied to spans whose `fg` is `None`.
    pub fn styled(styled: StyledText, fg: Color, indent: f32, scale: f32) -> Self {
        let text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
        Self {
            text,
            fg,
            indent,
            spans: styled.spans,
            scale,
        }
    }

    fn default_scale() -> f32 {
        1.0
    }
}

/// Declarative description of a scrollable styled-row list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageList {
    pub id: WidgetId,
    pub rows: Vec<MessageRow>,
    /// Index of the first row to draw at the top of the visible area.
    /// Backends clamp this to `rows.len() - visible_rows` so overscroll
    /// at the end pins the last message instead of leaving blank space.
    #[serde(default)]
    pub scroll_top: usize,
}

/// Backend-supplied row-height measurement for [`MessageList::hit_test`].
/// Both rasterisers (`tui::draw_message_list`, `gtk::draw_message_list`)
/// paint every row at a uniform `line_height` — see their module docs —
/// so this mirrors the one number each already threads through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MessageListMeasure {
    pub line_height: f32,
}

impl MessageListMeasure {
    pub fn new(line_height: f32) -> Self {
        Self { line_height }
    }

    /// Build from the backend's own [`crate::backend::Metrics`]
    /// (`backend.measure()`), matching the other `*Measure::from_metrics`
    /// constructors (quadraui#817).
    pub fn from_metrics(m: &crate::backend::Metrics) -> Self {
        Self::new(m.line_height)
    }
}

/// Hit-test classification for a click against a [`MessageList`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageListHit {
    /// Click landed on a row. Index into [`MessageList::rows`].
    Row(usize),
    /// Click missed every row — outside `rect`, or in the blank tail
    /// below the last painted row.
    Empty,
}

impl MessageList {
    /// Hit-test a click at surface-native `(x, y)` against this list,
    /// painted into `rect` with `measure.line_height`-tall uniform rows
    /// starting at [`Self::scroll_top`] (quadraui#818) — the same walk
    /// `tui::draw_message_list` / `gtk::draw_message_list` use to
    /// position each row.
    ///
    /// Coordinate frame: **ABSOLUTE** — `(x, y)` is compared directly
    /// against `rect`, matching `text_input_layout`'s convention.
    pub fn hit_test(
        &self,
        rect: Rect,
        measure: MessageListMeasure,
        x: f32,
        y: f32,
    ) -> MessageListHit {
        if measure.line_height <= 0.0 {
            return MessageListHit::Empty;
        }
        if x < rect.x || x >= rect.x + rect.width || y < rect.y || y >= rect.y + rect.height {
            return MessageListHit::Empty;
        }
        let row_offset = ((y - rect.y) / measure.line_height) as usize;
        let idx = self.scroll_top + row_offset;
        if idx < self.rows.len() {
            MessageListHit::Row(idx)
        } else {
            MessageListHit::Empty
        }
    }
}

// ── PaintSurface paint (issue #1084, PaintSurface Phase 4 7/8) ───────────
//
// `paint` below is shared by the **macOS and Windows** rasterisers only —
// `gtk::message_list::draw_message_list` is **not** migrated and stays a
// full, bespoke Cairo + Pango implementation. This mirrors the exception
// `crate::primitives::rich_text_popup::native_surface_paint` already
// documents and for the same underlying reason:
//
// [`gtk::message_list::draw_message_list`](crate::gtk::message_list::draw_message_list)
// renders each row's `spans` as **one** Pango call carrying a byte-ranged
// `AttrList` (per-span fg/bold/italic/underline, plus a whole-row
// `AttrFloat::new_scale` for markdown heading rows) — real shaped-line
// glyph positions, immune to the "measure each span, advance x by its
// width" drift that issue #214 fixed in `rich_text_popup`. `PaintSurface`
// has no "shape one line with N attribute ranges" verb, only single-
// style runs (`surface_draw_text_run(_styled)`), so migrating GTK onto
// `paint` below would mean reintroducing that previously-fixed bug class.
// Per this issue's "do not tranche silently" instruction: this is that
// call, made explicitly.
//
// macOS and Windows never had that problem — both already painted
// per-span with manual x-advance (the same shape `paint` below takes) —
// so consolidating *their* two copies carries no such risk, and closes a
// real, pre-existing gap between them:
//
// - **Spans ignored entirely.** `macos::message_list::draw_message_list`
//   never read `row.spans` at all — every row painted flat `row.text` +
//   `row.fg`, even when the caller supplied rich per-span styling. This
//   was not a documented scope omission on that module (unlike, say,
//   `win::message_list`'s italic/underline/scale note below) — it simply
//   never implemented the styled path `win::message_list` (#30) already
//   had. `paint` reads spans on both backends uniformly now: this is the
//   RED-before-the-port case — see `macos::message_list`'s
//   `styled_row_paints_per_span_colour` test, which has no pre-#1084
//   equivalent because there was no way to even ask for it.
// - **Per-span bold.** Carried via [`PaintSurface::surface_draw_text_run_styled`]
//   on both backends now. Win's `D2dSurface` honours it for real
//   (`DWrite::draw_text_styled`, matching this module's pre-#1084
//   behaviour exactly); macOS's `CgSurface` takes that verb's *default*,
//   which drops style entirely — inert on macOS (no visual regression,
//   no new bold either), the same posture `crate::primitives::status_bar`
//   and `crate::primitives::rich_text_popup` already document for the
//   identical default.
//
// Not carried over on either backend, matching pre-#1084 `win::message_list`'s
// own documented gap:
//
// - **Per-span italic / underline.** Passed through to
//   `surface_draw_text_run_styled`, but dropped by both backends' current
//   adapters (`D2dSurface` explicitly ignores them; `CgSurface` takes the
//   style-dropping default) — no Direct2D italic text format or underline
//   attribute is wired up today, matching `win::message_list`'s pre-#1084
//   module doc verbatim. Not a new gap introduced by this migration.
// - **`MessageRow::scale`** (markdown heading rows). `PaintSurface` has
//   no font-*size* verb (`surface_draw_text_run_styled`'s `scale_x` is a
//   horizontal *stretch* of the same-size glyph, for the wide-CJK-glyph
//   fix — see that verb's own doc — not a point-size change), so there is
//   no way to honour it through this trait, matching
//   `crate::primitives::rich_text_popup::native_surface_paint`'s
//   identical gap for its own per-line font scale. Every row paints at
//   its regular glyph size on both backends, same as every pre-#1084
//   macOS row (which ignored `scale` along with the rest of the styled
//   path) and every pre-#1084 Windows row (which never read `scale` at
//   all — see that module's old doc).
//
// **Zero-size guard**, closed uniformly like every other
// `native_surface_paint::paint` in this crate: pre-#1084, only
// `macos::message_list::draw_message_list` short-circuited a non-positive
// `w`; `win::message_list::draw_message_list` had no `w` parameter to
// guard at all. `paint` below takes no `w` — every original caller's `w`
// was used solely for that early-return, never to clip or size anything —
// so each backend's thin wrapper keeps its own pre-paint check instead
// (see `macos::message_list::draw_message_list`'s `if w <= 0.0` guard);
// `paint` itself only needs `line_height > 0.0`, matching Windows's
// pre-#1084 guard exactly.
#[cfg(any(feature = "win", all(feature = "macos", target_os = "macos")))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::MessageList;
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;

    /// Paint a [`MessageList`] into `rect` on `surface` — see this
    /// module's doc for the full per-backend divergence survey this
    /// closes/preserves. `rect.width` is unused (see the doc's "Zero-size
    /// guard" note); only `rect.x`, `rect.y`, and `rect.height` (via
    /// `rect.y + rect.height` as the bottom clip) matter.
    ///
    /// A `line_height <= 0.0` short-circuits to a no-paint call without
    /// touching `surface` at all.
    pub(crate) fn paint(
        list: &MessageList,
        surface: &mut dyn PaintSurface,
        rect: Rect,
        line_height: f32,
    ) {
        if line_height <= 0.0 {
            return;
        }
        let max_y = rect.y + rect.height;

        for (i, row) in list.rows.iter().skip(list.scroll_top).enumerate() {
            let ry = rect.y + i as f32 * line_height;
            if ry + line_height > max_y {
                break;
            }

            if !row.spans.is_empty() {
                // ── Styled path ─────────────────────────────────────────
                let mut cursor_x = rect.x + row.indent;
                for span in &row.spans {
                    let span_fg = span.fg.unwrap_or(row.fg);
                    let (sw, sh) = surface.surface_measure_text_styled(&span.text, span.bold);
                    let sy = ry + (line_height - sh) / 2.0;
                    surface.surface_draw_text_run_styled(
                        Rect::new(cursor_x, sy, sw.max(1.0), sh.max(1.0)),
                        &span.text,
                        span_fg,
                        span.bold,
                        span.italic,
                        span.underline,
                        1.0,
                    );
                    cursor_x += sw;
                }
            } else {
                // ── Flat path (unchanged from before spans were added) ───
                let (sw, sh) = surface.surface_measure_text(&row.text);
                let sy = ry + (line_height - sh) / 2.0;
                surface.surface_draw_text_run(
                    Rect::new(rect.x + row.indent, sy, sw.max(1.0), sh.max(1.0)),
                    &row.text,
                    row.fg,
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::primitives::message_list::MessageRow;
        use crate::types::{Color, StyledSpan, WidgetId};
        use crate::Image;

        /// Records every surface verb this primitive's paint uses —
        /// mirrors `primitives::command_line`'s identical test double, so
        /// this test runs on any host without Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            text_runs: Vec<(Rect, String, Color)>,
            styled_runs: Vec<(Rect, String, Color, bool, bool, bool)>,
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
            fn surface_measure_text_styled(&self, text: &str, _bold: bool) -> (f32, f32) {
                self.surface_measure_text(text)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            #[allow(clippy::too_many_arguments)]
            fn surface_draw_text_run_styled(
                &mut self,
                rect: Rect,
                text: &str,
                color: Color,
                bold: bool,
                italic: bool,
                underline: bool,
                _scale_x: f32,
            ) {
                self.styled_runs
                    .push((rect, text.to_string(), color, bold, italic, underline));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
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

        fn sample_list() -> MessageList {
            MessageList {
                id: WidgetId::new("ml"),
                rows: vec![
                    MessageRow::new("You:", Color::rgb(255, 220, 0), 0.0),
                    MessageRow::new("hi there", Color::rgb(220, 220, 220), 8.0),
                ],
                scroll_top: 0,
            }
        }

        #[test]
        fn flat_rows_paint_via_plain_text_run() {
            let list = sample_list();
            let mut surface = RecordingSurface::default();
            paint(&list, &mut surface, Rect::new(0.0, 0.0, 200.0, 100.0), 16.0);
            assert_eq!(surface.text_runs.len(), 2);
            assert!(surface.styled_runs.is_empty());
            assert_eq!(surface.text_runs[0].1, "You:");
            assert_eq!(surface.text_runs[0].2, Color::rgb(255, 220, 0));
        }

        /// The RED-before-the-port case named in this module's doc: prior
        /// to #1084, `macos::message_list::draw_message_list` had no way
        /// to paint a styled row at all — it never read `row.spans`. This
        /// asserts the shared `paint` now does, on any backend.
        #[test]
        fn styled_rows_paint_per_span_via_styled_run() {
            let mut list = sample_list();
            list.rows = vec![MessageRow {
                text: "bold text".into(),
                fg: Color::rgb(220, 220, 220),
                indent: 0.0,
                spans: vec![
                    StyledSpan {
                        text: "bold".into(),
                        fg: Some(Color::rgb(255, 0, 0)),
                        bg: None,
                        bold: true,
                        italic: false,
                        underline: false,
                    },
                    StyledSpan::plain(" text"),
                ],
                scale: 1.0,
            }];
            let mut surface = RecordingSurface::default();
            paint(&list, &mut surface, Rect::new(0.0, 0.0, 200.0, 100.0), 16.0);
            assert!(surface.text_runs.is_empty());
            assert_eq!(surface.styled_runs.len(), 2);
            assert_eq!(surface.styled_runs[0].1, "bold");
            assert_eq!(surface.styled_runs[0].2, Color::rgb(255, 0, 0));
            assert!(surface.styled_runs[0].3, "first span should paint bold");
            // Second span falls back to the row's own fg.
            assert_eq!(surface.styled_runs[1].2, Color::rgb(220, 220, 220));
            assert!(!surface.styled_runs[1].3);
        }

        #[test]
        fn scroll_top_skips_leading_rows() {
            let mut list = sample_list();
            list.scroll_top = 1;
            let mut surface = RecordingSurface::default();
            paint(&list, &mut surface, Rect::new(0.0, 0.0, 200.0, 100.0), 16.0);
            assert_eq!(surface.text_runs.len(), 1);
            assert_eq!(surface.text_runs[0].1, "hi there");
        }

        #[test]
        fn rows_past_bottom_edge_are_not_painted() {
            let mut list = sample_list();
            for i in 0..20 {
                list.rows.push(MessageRow::new(
                    format!("row {i}"),
                    Color::rgb(200, 200, 200),
                    0.0,
                ));
            }
            let mut surface = RecordingSurface::default();
            // Only 3 rows worth of height available.
            paint(&list, &mut surface, Rect::new(0.0, 0.0, 200.0, 48.0), 16.0);
            assert_eq!(surface.text_runs.len(), 3);
        }

        #[test]
        fn zero_line_height_paints_nothing() {
            let list = sample_list();
            let mut surface = RecordingSurface::default();
            paint(&list, &mut surface, Rect::new(0.0, 0.0, 200.0, 100.0), 0.0);
            assert!(surface.text_runs.is_empty());
            assert!(surface.styled_runs.is_empty());
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_has_empty_spans_and_unit_scale() {
        let row = MessageRow::new("hello", Color::rgb(200, 200, 200), 2.0);
        assert!(
            row.spans.is_empty(),
            "MessageRow::new must produce empty spans"
        );
        assert!(
            (row.scale - 1.0).abs() < f32::EPSILON,
            "MessageRow::new must set scale to 1.0"
        );
        assert_eq!(row.text, "hello");
        assert_eq!(row.indent, 2.0);
    }

    #[test]
    fn styled_carries_spans_and_scale() {
        let spans = vec![
            StyledSpan {
                text: "bold".to_string(),
                fg: Some(Color::rgb(255, 255, 0)),
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            },
            StyledSpan::plain(" text"),
        ];
        let styled = StyledText {
            spans: spans.clone(),
        };
        let row = MessageRow::styled(styled, Color::rgb(200, 200, 200), 2.0, 1.5);

        assert_eq!(row.spans, spans);
        assert!((row.scale - 1.5).abs() < f32::EPSILON);
        assert_eq!(row.text, "bold text");
        assert_eq!(row.indent, 2.0);
    }

    #[test]
    fn styled_with_empty_styled_text_produces_empty_spans_and_empty_text() {
        let row = MessageRow::styled(StyledText::default(), Color::rgb(100, 100, 100), 0.0, 1.0);
        assert!(row.spans.is_empty());
        assert_eq!(row.text, "");
    }

    #[test]
    fn new_and_equivalent_styled_plain_rows_have_same_text() {
        // A styled row built from a plain StyledText must have the same
        // text field as the corresponding flat row — ensuring callers that
        // read `row.text` on the plain path see the same value.
        let plain = StyledText::plain("same content");
        let flat = MessageRow::new("same content", Color::rgb(200, 200, 200), 2.0);
        let rich = MessageRow::styled(plain, Color::rgb(200, 200, 200), 2.0, 1.0);
        assert_eq!(flat.text, rich.text);
        assert_eq!(flat.fg, rich.fg);
        assert_eq!(flat.indent, rich.indent);
        assert_eq!(flat.scale, rich.scale);
        // The styled constructor always populates spans (even if just plain).
        assert_eq!(rich.spans.len(), 1);
        assert!(flat.spans.is_empty());
    }

    #[test]
    fn serde_round_trip_flat_row() {
        let row = MessageRow::new("flat", Color::rgb(1, 2, 3), 0.0);
        let json = serde_json::to_string(&row).unwrap();
        let decoded: MessageRow = serde_json::from_str(&json).unwrap();
        assert_eq!(row, decoded);
    }

    #[test]
    fn serde_round_trip_styled_row() {
        let styled = StyledText {
            spans: vec![StyledSpan {
                text: "hi".to_string(),
                fg: Some(Color::rgb(255, 0, 0)),
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            }],
        };
        let row = MessageRow::styled(styled, Color::rgb(200, 200, 200), 2.0, 1.5);
        let json = serde_json::to_string(&row).unwrap();
        let decoded: MessageRow = serde_json::from_str(&json).unwrap();
        assert_eq!(row, decoded);
    }

    #[test]
    fn serde_legacy_json_without_spans_defaults_to_empty() {
        // Deserialise a JSON object that has no "spans" or "scale" fields —
        // this is the pre-styled-row wire format.  Both fields must default
        // gracefully so old serialised data remains loadable.
        let legacy = r#"{"text":"legacy","fg":{"r":100,"g":100,"b":100,"a":255},"indent":0.0}"#;
        let row: MessageRow = serde_json::from_str(legacy).unwrap();
        assert!(row.spans.is_empty(), "spans must default to empty");
        assert!(
            (row.scale - 1.0).abs() < f32::EPSILON,
            "scale must default to 1.0"
        );
        assert_eq!(row.text, "legacy");
    }

    // ── #818: hit_test ───────────────────────────────────────────────────

    fn list_with_rows(n: usize, scroll_top: usize) -> MessageList {
        MessageList {
            id: WidgetId::new("ml"),
            rows: (0..n)
                .map(|i| MessageRow::new(format!("row{i}"), Color::rgb(0, 0, 0), 0.0))
                .collect(),
            scroll_top,
        }
    }

    #[test]
    fn hit_test_returns_row_index_at_scroll_offset() {
        let list = list_with_rows(10, 2);
        let rect = Rect::new(0.0, 0.0, 40.0, 5.0);
        let measure = MessageListMeasure::new(1.0);
        // Row 1 of the viewport (y in [1,2)) maps to rows[scroll_top + 1] = rows[3].
        assert_eq!(
            list.hit_test(rect, measure, 5.0, 1.5),
            MessageListHit::Row(3)
        );
    }

    #[test]
    fn hit_test_below_last_row_is_empty() {
        let list = list_with_rows(2, 0);
        let rect = Rect::new(0.0, 0.0, 40.0, 5.0);
        let measure = MessageListMeasure::new(1.0);
        // Row index 3 (y in [3,4)) has no backing message row.
        assert_eq!(
            list.hit_test(rect, measure, 5.0, 3.5),
            MessageListHit::Empty
        );
    }

    #[test]
    fn hit_test_outside_rect_is_empty() {
        let list = list_with_rows(5, 0);
        let rect = Rect::new(10.0, 10.0, 40.0, 5.0);
        let measure = MessageListMeasure::new(1.0);
        assert_eq!(
            list.hit_test(rect, measure, 0.0, 0.0),
            MessageListHit::Empty
        );
    }

    #[test]
    fn hit_test_zero_line_height_is_empty() {
        let list = list_with_rows(5, 0);
        let rect = Rect::new(0.0, 0.0, 40.0, 5.0);
        let measure = MessageListMeasure::new(0.0);
        assert_eq!(
            list.hit_test(rect, measure, 1.0, 1.0),
            MessageListHit::Empty
        );
    }
}
