//! Markdown → [`StyledText`] adapter (first-cut implementation).
//!
//! Converts a subset of CommonMark to [`StyledText`] lines that the TUI and
//! GTK backends can render.  Supported features:
//!
//! | Syntax | Output |
//! |--------|--------|
//! | `# H1` / `## H2` / `### H3` | bold span + `line_scales[i]` = 2.0 / 1.5 / 1.2 |
//! | `**text**` / `__text__` | [`StyledSpan`] with `bold: true` |
//! | `*text*` / `_text_` | [`StyledSpan`] with `italic: true` |
//! | `` `text` `` | [`StyledSpan`] coloured with [`Theme::accent_fg`] |
//!
//! # Intentional deferrals (first-cut scope)
//!
//! The following are **consciously out of scope** for this first cut and will
//! be addressed in a follow-up issue:
//!
//! * **Bulleted / numbered lists** — currently pass through as literal `- ` /
//!   `1. ` plain text with no indent or styling.  This was the second motivating
//!   example in the original issue (coord-tui Review-findings panel).
//!
//! * **`code_blocks: Vec<CodeBlockRange>` / `CodeBlockRange` struct** — omitted
//!   from `RenderedMarkdown` intentionally.  The issue proposed this field so
//!   tree-sitter-capable callers (vimcode) could opt into per-language syntax
//!   highlighting.  Because `RenderedMarkdown` is library-constructed (consumers
//!   only read its fields), adding `code_blocks` later is a non-breaking additive
//!   change.  Fenced code blocks currently pass through as plain text.
//!
//! Everything else (links, blockquotes, tables, images) also passes through as
//! plain text.
//!
//! # Example
//!
//! ```rust
//! # use quadraui::compose::markdown::render_markdown_to_styled;
//! # use quadraui::Theme;
//! let md = "# Hello\n**bold** and *italic* and `code`";
//! let result = render_markdown_to_styled(md, &Theme::default());
//! assert_eq!(result.lines.len(), result.line_scales.len());
//! assert!(result.line_scales[0] > 1.0); // heading
//! ```

use crate::theme::Theme;
use crate::types::{StyledSpan, StyledText};

// ── Public output type ─────────────────────────────────────────────────────

/// Output of [`render_markdown_to_styled`].
///
/// All three `Vec`s are **always the same length** — one entry per rendered
/// line.  The invariant holds for all inputs including empty strings.
#[derive(Debug, Clone, Default)]
pub struct RenderedMarkdown {
    /// One [`StyledText`] per line, with inline formatting applied.
    pub lines: Vec<StyledText>,
    /// Plain text of each line (all span text concatenated, markdown syntax
    /// stripped).  Useful for hit-tests, search, and accessibility.
    pub line_text: Vec<String>,
    /// Per-line font-scale factor.  `1.0` for body text; `2.0` / `1.5` /
    /// `1.2` for H1 / H2 / H3 respectively.
    pub line_scales: Vec<f32>,
}

// ── Public entry point ─────────────────────────────────────────────────────

/// Convert a markdown `input` string to [`RenderedMarkdown`] using `theme`
/// for inline-code foreground colour.
///
/// Each `\n`-separated line in `input` produces exactly one entry in all
/// three output vectors.  The function is deterministic and allocation-light
/// (no regex, no external parser crate — pure Rust string scanning).
pub fn render_markdown_to_styled(input: &str, theme: &Theme) -> RenderedMarkdown {
    let mut result = RenderedMarkdown::default();

    for raw_line in input.lines() {
        let (plain, styled, scale) = render_line(raw_line, theme);
        result.lines.push(styled);
        result.line_text.push(plain);
        result.line_scales.push(scale);
    }

    result
}

// ── Per-line rendering ─────────────────────────────────────────────────────

/// Process one raw markdown line, returning `(plain_text, StyledText, scale)`.
fn render_line(line: &str, theme: &Theme) -> (String, StyledText, f32) {
    let (heading_level, content) = parse_heading_prefix(line);
    let scale = heading_scale(heading_level);
    // Headings pass `bold = true` as the base style so every span in the
    // heading is bold (inline **bold** inside a heading stays bold too).
    let spans = parse_inline(content, heading_level > 0, false, theme);
    let plain: String = spans.iter().map(|s| s.text.as_str()).collect();
    (plain, StyledText { spans }, scale)
}

/// Detect a heading prefix (`# `, `## `, `### `).
///
/// Returns `(level, content_after_prefix)`.  Level 0 means no heading.
fn parse_heading_prefix(line: &str) -> (u8, &str) {
    if let Some(rest) = line.strip_prefix("### ") {
        (3, rest)
    } else if let Some(rest) = line.strip_prefix("## ") {
        (2, rest)
    } else if let Some(rest) = line.strip_prefix("# ") {
        (1, rest)
    } else {
        (0, line)
    }
}

/// Map heading level to font-scale multiplier.
fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 2.0,
        2 => 1.5,
        3 => 1.2,
        _ => 1.0,
    }
}

// ── Inline span parser ─────────────────────────────────────────────────────

/// Parse inline markdown in `text` with the given base `bold`/`italic` flags,
/// returning a flat list of [`StyledSpan`]s.
///
/// Delimiters are matched greedily left-to-right with the following priority:
///
/// 1. `` ` ``…`` ` `` — inline code (no further parsing inside)
/// 2. `**` / `__` — bold (content is recursively parsed)
/// 3. `*` / `_` — italic (content is recursively parsed)
///
/// Unmatched delimiters are treated as literal plain text.
///
/// The function is not recursive-descent in the pathological sense: real
/// markdown is shallow (at most a few levels of nesting) and the recursion
/// only fires on matched delimiter pairs.
fn parse_inline(text: &str, bold: bool, italic: bool, theme: &Theme) -> Vec<StyledSpan> {
    let mut spans: Vec<StyledSpan> = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut pos = 0usize;
    let mut plain_start = 0usize;

    while pos < len {
        let b = bytes[pos];

        // ── Inline code: `...` ───────────────────────────────────────────
        if b == b'`' {
            let after = pos + 1;
            if let Some(close_rel) = text[after..].find('`') {
                // Flush plain text before this code span.
                if plain_start < pos {
                    spans.push(make_span(&text[plain_start..pos], bold, italic, None));
                }
                let code_text = &text[after..after + close_rel];
                // `accent_fg` is the closest Theme field to "code_fg".
                // It is a distinct light-blue that reads as "special" text,
                // consistent with how many dark editors colour inline code.
                spans.push(StyledSpan {
                    text: code_text.to_string(),
                    fg: Some(theme.accent_fg),
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                });
                pos = after + close_rel + 1;
                plain_start = pos;
                continue;
            }
            // No closing backtick — treat as plain text, advance past it.
            pos += 1;
            continue;
        }

        // ── Bold: **...** ────────────────────────────────────────────────
        if b == b'*' && pos + 1 < len && bytes[pos + 1] == b'*' {
            let after = pos + 2;
            if let Some(close_rel) = text[after..].find("**") {
                if plain_start < pos {
                    spans.push(make_span(&text[plain_start..pos], bold, italic, None));
                }
                let inner = &text[after..after + close_rel];
                spans.extend(parse_inline(inner, true, italic, theme));
                pos = after + close_rel + 2;
                plain_start = pos;
                continue;
            }
            // Unmatched ** — skip both characters as plain text.
            pos += 2;
            continue;
        }

        // ── Bold: __...__ ────────────────────────────────────────────────
        if b == b'_' && pos + 1 < len && bytes[pos + 1] == b'_' {
            let after = pos + 2;
            if let Some(close_rel) = text[after..].find("__") {
                if plain_start < pos {
                    spans.push(make_span(&text[plain_start..pos], bold, italic, None));
                }
                let inner = &text[after..after + close_rel];
                spans.extend(parse_inline(inner, true, italic, theme));
                pos = after + close_rel + 2;
                plain_start = pos;
                continue;
            }
            // Unmatched __ — skip both characters as plain text.
            pos += 2;
            continue;
        }

        // ── Italic: *...* ────────────────────────────────────────────────
        // This branch only fires when the `**` branch above did not match,
        // i.e. the current character is a lone `*`.
        if b == b'*' {
            let after = pos + 1;
            if let Some(close_rel) = text[after..].find('*') {
                if plain_start < pos {
                    spans.push(make_span(&text[plain_start..pos], bold, italic, None));
                }
                let inner = &text[after..after + close_rel];
                spans.extend(parse_inline(inner, bold, true, theme));
                pos = after + close_rel + 1;
                plain_start = pos;
                continue;
            }
            // Unmatched * — skip as plain text.
            pos += 1;
            continue;
        }

        // ── Italic: _..._ ────────────────────────────────────────────────
        // This branch only fires when the `__` branch above did not match.
        if b == b'_' {
            let after = pos + 1;
            if let Some(close_rel) = text[after..].find('_') {
                if plain_start < pos {
                    spans.push(make_span(&text[plain_start..pos], bold, italic, None));
                }
                let inner = &text[after..after + close_rel];
                spans.extend(parse_inline(inner, bold, true, theme));
                pos = after + close_rel + 1;
                plain_start = pos;
                continue;
            }
            // Unmatched _ — skip as plain text.
            pos += 1;
            continue;
        }

        pos += 1;
    }

    // Flush any remaining plain text.
    if plain_start < len {
        spans.push(make_span(&text[plain_start..], bold, italic, None));
    }

    spans
}

/// Construct a [`StyledSpan`] with the given style flags and optional
/// foreground colour.
fn make_span(text: &str, bold: bool, italic: bool, fg: Option<crate::types::Color>) -> StyledSpan {
    StyledSpan {
        text: text.to_string(),
        fg,
        bg: None,
        bold,
        italic,
        underline: false,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    // ── Vector-length invariant ────────────────────────────────────────

    #[test]
    fn output_vectors_are_length_aligned() {
        let inputs = &[
            "",
            "plain text",
            "# Heading",
            "line one\nline two\nline three",
            "**bold** and *italic* and `code`",
            "# H1\n## H2\n### H3\nbody",
        ];
        let theme = Theme::default();
        for input in inputs {
            let r = render_markdown_to_styled(input, &theme);
            assert_eq!(
                r.lines.len(),
                r.line_text.len(),
                "line_text length mismatch for input {input:?}"
            );
            assert_eq!(
                r.lines.len(),
                r.line_scales.len(),
                "line_scales length mismatch for input {input:?}"
            );
        }
    }

    // ── Headings ───────────────────────────────────────────────────────

    #[test]
    fn h1_produces_scale_2_and_bold_span() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("# Hello", &theme);
        assert_eq!(r.lines.len(), 1);
        assert!(
            (r.line_scales[0] - 2.0).abs() < f32::EPSILON,
            "H1 scale should be 2.0"
        );
        assert!(
            r.lines[0].spans.iter().any(|s| s.bold),
            "H1 should produce at least one bold span"
        );
        assert_eq!(r.line_text[0], "Hello");
    }

    #[test]
    fn h2_produces_scale_1_5_and_bold_span() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("## World", &theme);
        assert!(
            (r.line_scales[0] - 1.5).abs() < f32::EPSILON,
            "H2 scale should be 1.5"
        );
        assert!(r.lines[0].spans.iter().any(|s| s.bold));
    }

    #[test]
    fn h3_produces_scale_1_2_and_bold_span() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("### Section", &theme);
        assert!(
            (r.line_scales[0] - 1.2).abs() < f32::EPSILON,
            "H3 scale should be 1.2"
        );
        assert!(r.lines[0].spans.iter().any(|s| s.bold));
    }

    #[test]
    fn body_line_has_scale_1_0() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("Just text", &theme);
        assert!(
            (r.line_scales[0] - 1.0).abs() < f32::EPSILON,
            "body scale should be 1.0"
        );
    }

    #[test]
    fn heading_plain_text_is_content_without_hashes() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("# My Heading", &theme);
        assert_eq!(r.line_text[0], "My Heading");
    }

    // ── Bold ───────────────────────────────────────────────────────────

    #[test]
    fn double_asterisk_bold() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("**bold**", &theme);
        assert_eq!(r.lines.len(), 1);
        let bold_spans: Vec<_> = r.lines[0].spans.iter().filter(|s| s.bold).collect();
        assert!(!bold_spans.is_empty(), "expected at least one bold span");
        assert!(bold_spans.iter().any(|s| s.text == "bold"));
    }

    #[test]
    fn double_underscore_bold() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("__bold__", &theme);
        assert!(r.lines[0].spans.iter().any(|s| s.bold && s.text == "bold"));
    }

    // ── Italic ─────────────────────────────────────────────────────────

    #[test]
    fn single_asterisk_italic() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("*italic*", &theme);
        assert_eq!(r.lines.len(), 1);
        let italic_spans: Vec<_> = r.lines[0].spans.iter().filter(|s| s.italic).collect();
        assert!(
            !italic_spans.is_empty(),
            "expected at least one italic span"
        );
        assert!(italic_spans.iter().any(|s| s.text == "italic"));
    }

    #[test]
    fn single_underscore_italic() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("_italic_", &theme);
        assert!(r.lines[0]
            .spans
            .iter()
            .any(|s| s.italic && s.text == "italic"));
    }

    // ── Inline code ────────────────────────────────────────────────────

    #[test]
    fn backtick_code_span_uses_accent_fg() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("`code`", &theme);
        assert_eq!(r.lines.len(), 1);
        let code_spans: Vec<_> = r.lines[0]
            .spans
            .iter()
            .filter(|s| s.fg == Some(theme.accent_fg))
            .collect();
        assert!(!code_spans.is_empty(), "expected a code-colored span");
        assert!(code_spans.iter().any(|s| s.text == "code"));
    }

    #[test]
    fn backtick_code_is_not_bold_or_italic() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("`code`", &theme);
        let code_spans: Vec<_> = r.lines[0]
            .spans
            .iter()
            .filter(|s| s.fg == Some(theme.accent_fg))
            .collect();
        for s in &code_spans {
            assert!(!s.bold, "inline code should not be bold");
            assert!(!s.italic, "inline code should not be italic");
        }
    }

    // ── Mixed inline on one line ────────────────────────────────────────

    #[test]
    fn mixed_inline_styles_split_into_correct_spans() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("**bold** and *italic* and `code`", &theme);
        assert_eq!(r.lines.len(), 1);
        let spans = &r.lines[0].spans;

        // There must be a bold span containing "bold".
        assert!(
            spans.iter().any(|s| s.bold && s.text.contains("bold")),
            "missing bold span; spans: {spans:?}"
        );
        // There must be an italic span containing "italic".
        assert!(
            spans.iter().any(|s| s.italic && s.text.contains("italic")),
            "missing italic span; spans: {spans:?}"
        );
        // There must be a code-colored span containing "code".
        assert!(
            spans
                .iter()
                .any(|s| s.fg == Some(theme.accent_fg) && s.text.contains("code")),
            "missing code span; spans: {spans:?}"
        );
        // Plain text between the styled regions must be present too.
        assert!(
            spans.iter().any(|s| s.text.contains(" and ")),
            "expected plain ' and ' separator; spans: {spans:?}"
        );
        // The reconstructed plain text must match the styled text stripped of syntax.
        assert_eq!(r.line_text[0], "bold and italic and code");
    }

    #[test]
    fn bold_italic_combined_in_heading() {
        // Inside a heading, the base style is bold=true.  An extra * makes it
        // italic too — the span should carry both flags.
        let theme = Theme::default();
        let r = render_markdown_to_styled("# *emphasized heading*", &theme);
        // The span for "emphasized heading" should be both bold (from heading)
        // and italic (from *...*).
        assert!(
            r.lines[0].spans.iter().any(|s| s.bold && s.italic),
            "expected bold+italic span inside heading; spans: {:?}",
            r.lines[0].spans
        );
    }

    // ── Multi-line input ───────────────────────────────────────────────

    #[test]
    fn multi_line_all_three_heading_levels() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("# H1\n## H2\n### H3\nbody", &theme);
        assert_eq!(r.lines.len(), 4);
        assert!((r.line_scales[0] - 2.0).abs() < f32::EPSILON);
        assert!((r.line_scales[1] - 1.5).abs() < f32::EPSILON);
        assert!((r.line_scales[2] - 1.2).abs() < f32::EPSILON);
        assert!((r.line_scales[3] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn empty_input_produces_empty_vectors() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("", &theme);
        assert!(r.lines.is_empty());
        assert!(r.line_text.is_empty());
        assert!(r.line_scales.is_empty());
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("Hello, world!", &theme);
        assert_eq!(r.line_text[0], "Hello, world!");
        // No special styling expected on a plain line.
        assert_eq!(r.lines[0].spans.len(), 1);
        assert!(!r.lines[0].spans[0].bold);
        assert!(!r.lines[0].spans[0].italic);
        assert!(r.lines[0].spans[0].fg.is_none());
    }

    // ── Edge cases ─────────────────────────────────────────────────────

    #[test]
    fn unmatched_delimiter_treated_as_plain_text() {
        let theme = Theme::default();
        // A lone * with no closing * should pass through as plain text.
        let r = render_markdown_to_styled("price is $5*2", &theme);
        assert_eq!(r.line_text[0], "price is $5*2");
        // No italic spans expected.
        assert!(r.lines[0].spans.iter().all(|s| !s.italic));
    }

    #[test]
    fn heading_with_no_content_is_still_a_heading() {
        let theme = Theme::default();
        // "# " followed by nothing — should still produce scale 2.0.
        let r = render_markdown_to_styled("# ", &theme);
        assert!((r.line_scales[0] - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn hash_without_space_is_not_a_heading() {
        // "#title" without a space after # is not CommonMark heading syntax.
        let theme = Theme::default();
        let r = render_markdown_to_styled("#title", &theme);
        assert!(
            (r.line_scales[0] - 1.0).abs() < f32::EPSILON,
            "should be body, not heading"
        );
        assert_eq!(r.line_text[0], "#title");
    }
}
