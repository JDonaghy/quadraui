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
