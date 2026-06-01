//! Markdown → [`StyledText`] adapter.
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
//! | `- item` / `* item` | indent + `•` marker span in [`Theme::accent_fg`] |
//! | `1. item` | indent + `N.` marker span in [`Theme::accent_fg`] |
//! | `` ```lang … ``` `` | dim-bg spans; `code_blocks` side-channel populated |
//! | `[text](url)` | underlined [`Theme::link_fg`] span; `links` side-channel populated |
//! | `> text` | `│ ` rule in [`Theme::link_fg`] + parsed inline content |
//!
//! Emphasis (`*`/`_`) honours a CommonMark-style **flanking** rule, so
//! `snake_case` identifiers and whitespace-flanked operators are *not*
//! mistaken for emphasis (`foo_bar` and `a * b * c` render upright).  See
//! [`can_open`] / [`can_close`].
//!
//! A runnable demo lives in `examples/tui_markdown.rs` /
//! `examples/gtk_markdown.rs` — it renders a document through
//! [`render_markdown_to_styled`] into a `RichTextPopup`.
//!
//! # Intentional deferrals
//!
//! The following are **consciously out of scope** and tracked in a follow-up
//! issue:
//!
//! * **Nested lists** — single-level only.  A nested `- ` inside a list item
//!   passes through as plain text.
//! * **Tables** — largest effort, separate work.
//! * **Images** — TUI has no raster path; lowest priority.
//! * **Link text emphasis** — inline bold/italic inside `[text](url)` is not
//!   parsed; the link text renders as a single unstyled-then-underlined span.
//!
//! # Side-channels
//!
//! [`RenderedMarkdown`] carries two additive side-channels that **do not
//! affect the length-aligned vectors** (`lines` / `line_text` / `line_scales`):
//!
//! * `code_blocks: Vec<CodeBlockRange>` — fenced code block extents.
//!   Tree-sitter-capable callers opt into per-language highlighting using this.
//! * `links: Vec<(usize, Range<usize>, String)>` — `(line_idx, byte_range,
//!   url)` triples.  `byte_range` indexes into `line_text[line_idx]`.
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

use std::ops::Range;

use crate::theme::Theme;
use crate::types::{StyledSpan, StyledText};

// ── Public output types ────────────────────────────────────────────────────

/// A fenced code block encountered during rendering.
///
/// `fence_open` is the index (into `RenderedMarkdown::lines`) of the opening
/// `` ``` `` fence line.  Content lines span indices
/// `fence_open + 1 .. fence_close` (exclusive).  If the input ended without a
/// closing fence, `fence_close` is `None`.
#[derive(Debug, Clone)]
pub struct CodeBlockRange {
    /// Line index of the opening `` ``` `` fence.
    pub fence_open: usize,
    /// Line index of the closing `` ``` `` fence, or `None` when unclosed.
    pub fence_close: Option<usize>,
    /// Language hint from the opening fence (e.g. `"rust"`, `"python"`).
    pub lang: Option<String>,
}

/// Output of [`render_markdown_to_styled`].
///
/// All three primary `Vec`s are **always the same length** — one entry per
/// rendered line.  The invariant holds for all inputs including empty strings
/// and inputs that contain fenced code blocks.
///
/// `code_blocks` and `links` are **additive side-channels**: their lengths are
/// independent of the primary vectors.  Consumers that only need styled text
/// can ignore them.
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
    /// Fenced code blocks.  Each entry describes the opening and closing fence
    /// line indices plus the optional language tag.  Additive — does not affect
    /// the primary vector lengths.
    pub code_blocks: Vec<CodeBlockRange>,
    /// Links found in the input.  Each entry is `(line_idx, byte_range, url)`.
    /// `line_idx` indexes into the primary vectors.  `byte_range` is a byte
    /// range into `line_text[line_idx]`.  Additive.
    pub links: Vec<(usize, Range<usize>, String)>,
}

// ── Internal types ─────────────────────────────────────────────────────────

/// A link extracted during inline parsing.  Byte ranges into `line_text` are
/// resolved later by scanning the concatenated span text.
struct RawLink {
    text: String,
    url: String,
}

// ── Public entry point ─────────────────────────────────────────────────────

/// Convert a markdown `input` string to [`RenderedMarkdown`] using `theme`
/// for colours.
///
/// Each `\n`-separated line in `input` produces exactly one entry in all
/// three primary output vectors (`lines`, `line_text`, `line_scales`).  The
/// function is deterministic and allocation-light (no regex, no external
/// parser crate — pure Rust string scanning).
///
/// Fenced code blocks span multiple raw lines.  All fence and content lines
/// are included in the primary vectors (one entry per raw line), but no
/// inner emphasis is parsed for code block content.
pub fn render_markdown_to_styled(input: &str, theme: &Theme) -> RenderedMarkdown {
    let mut result = RenderedMarkdown::default();
    let mut in_code_block = false;
    let mut open_fence_idx: Option<usize> = None;
    let mut open_fence_lang: Option<String> = None;

    for raw_line in input.lines() {
        let line_idx = result.lines.len();

        if in_code_block {
            // Inside a fenced code block — check for closing fence first.
            if detect_fence(raw_line).is_some() {
                // Closing fence.
                result.code_blocks.push(CodeBlockRange {
                    fence_open: open_fence_idx.unwrap_or(line_idx),
                    fence_close: Some(line_idx),
                    lang: open_fence_lang.take(),
                });
                let (plain, styled) = render_fence_line(raw_line, theme);
                result.lines.push(styled);
                result.line_text.push(plain);
                result.line_scales.push(1.0);
                in_code_block = false;
                open_fence_idx = None;
            } else {
                // Code block content — no emphasis parsing.
                let (plain, styled) = render_code_content(raw_line, theme);
                result.lines.push(styled);
                result.line_text.push(plain);
                result.line_scales.push(1.0);
            }
        } else if let Some(lang) = detect_fence(raw_line) {
            // Opening fence.
            let (plain, styled) = render_fence_line(raw_line, theme);
            result.lines.push(styled);
            result.line_text.push(plain);
            result.line_scales.push(1.0);
            in_code_block = true;
            open_fence_idx = Some(line_idx);
            open_fence_lang = lang;
        } else {
            // Regular line — headings, lists, blockquotes, body, inline markup.
            let (plain, styled, scale, line_links) = render_regular_line(raw_line, theme);
            for (start, end, url) in line_links {
                result.links.push((line_idx, start..end, url));
            }
            result.lines.push(styled);
            result.line_text.push(plain);
            result.line_scales.push(scale);
        }
    }

    // An unclosed code block: record it with no closing fence.
    if in_code_block {
        if let Some(fence_open) = open_fence_idx {
            result.code_blocks.push(CodeBlockRange {
                fence_open,
                fence_close: None,
                lang: open_fence_lang,
            });
        }
    }

    result
}

// ── Fence detection ────────────────────────────────────────────────────────

/// Detect a fenced code block delimiter (`` ``` `` with optional language tag).
///
/// Returns `Some(lang)` where `lang` is the language string (possibly `None`
/// if the fence has no tag).  Returns `None` if this is not a fence line.
fn detect_fence(line: &str) -> Option<Option<String>> {
    let trimmed = line.trim_start();
    if let Some(after_ticks) = trimmed.strip_prefix("```") {
        let after = after_ticks.trim();
        if after.is_empty() {
            Some(None)
        } else {
            Some(Some(after.to_string()))
        }
    } else {
        None
    }
}

// ── Code block line rendering ──────────────────────────────────────────────

/// Render a fence delimiter line (`` ```lang `` or `` ``` ``).
///
/// Styled with [`Theme::muted_fg`] on a [`Theme::surface_bg`] tint to
/// visually distinguish it from body text.
fn render_fence_line(line: &str, theme: &Theme) -> (String, StyledText) {
    let plain = line.to_string();
    let styled = StyledText {
        spans: vec![StyledSpan {
            text: plain.clone(),
            fg: Some(theme.muted_fg),
            bg: Some(theme.surface_bg),
            bold: false,
            italic: false,
            underline: false,
        }],
    };
    (plain, styled)
}

/// Render one content line inside a fenced code block.
///
/// No emphasis parsing is done.  Text colour is [`Theme::foreground`] on a
/// [`Theme::surface_bg`] tint — matching the fence lines — so the whole block
/// reads as a contiguous dimmed-bg region.
fn render_code_content(line: &str, theme: &Theme) -> (String, StyledText) {
    let plain = line.to_string();
    let styled = StyledText {
        spans: vec![StyledSpan {
            text: plain.clone(),
            fg: Some(theme.foreground),
            bg: Some(theme.surface_bg),
            bold: false,
            italic: false,
            underline: false,
        }],
    };
    (plain, styled)
}

// ── Per-line rendering (regular lines) ────────────────────────────────────

/// Process one regular (non-code-block) markdown line.
///
/// Returns `(plain_text, StyledText, scale, link_entries)`.
/// `link_entries` are `(byte_start, byte_end, url)` triples into `plain_text`.
fn render_regular_line(
    line: &str,
    theme: &Theme,
) -> (String, StyledText, f32, Vec<(usize, usize, String)>) {
    // ── Blockquote ────────────────────────────────────────────────────
    if let Some(content) = parse_blockquote(line) {
        let (inner_spans, raw_links) = parse_inline(content, false, false, theme);
        let inner_plain: String = inner_spans.iter().map(|s| s.text.as_str()).collect();
        // "│ " — U+2502 (3 UTF-8 bytes) + space = 4 bytes.
        let prefix = "\u{2502} ";
        let prefix_len = prefix.len(); // = 4
        let plain = format!("{prefix}{inner_plain}");
        let links = resolve_links(&raw_links, &inner_plain, prefix_len);
        let bar_span = StyledSpan {
            text: prefix.to_string(),
            fg: Some(theme.link_fg),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        };
        let mut spans = vec![bar_span];
        spans.extend(inner_spans);
        return (plain, StyledText { spans }, 1.0, links);
    }

    // ── Bulleted list: "- item" / "* item" ───────────────────────────
    if let Some(content) = parse_bullet_item(line) {
        let (inner_spans, raw_links) = parse_inline(content, false, false, theme);
        let inner_plain: String = inner_spans.iter().map(|s| s.text.as_str()).collect();
        // "  • " — 2 spaces + U+2022 (3 bytes) + space = 6 bytes.
        let prefix = "  \u{2022} ";
        let prefix_len = prefix.len(); // = 6
        let plain = format!("{prefix}{inner_plain}");
        let links = resolve_links(&raw_links, &inner_plain, prefix_len);
        let indent_span = StyledSpan::plain("  ");
        let bullet_span = StyledSpan {
            text: "\u{2022} ".to_string(),
            fg: Some(theme.accent_fg),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        };
        let mut spans = vec![indent_span, bullet_span];
        spans.extend(inner_spans);
        return (plain, StyledText { spans }, 1.0, links);
    }

    // ── Numbered list: "1. item", "2. item", … ───────────────────────
    if let Some((num, content)) = parse_ordered_item(line) {
        let (inner_spans, raw_links) = parse_inline(content, false, false, theme);
        let inner_plain: String = inner_spans.iter().map(|s| s.text.as_str()).collect();
        let marker = format!("{num}. ");
        // "  " (2 bytes) + marker
        let prefix = format!("  {marker}");
        let prefix_len = prefix.len();
        let plain = format!("{prefix}{inner_plain}");
        let links = resolve_links(&raw_links, &inner_plain, prefix_len);
        let indent_span = StyledSpan::plain("  ");
        let marker_span = StyledSpan {
            text: marker,
            fg: Some(theme.accent_fg),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        };
        let mut spans = vec![indent_span, marker_span];
        spans.extend(inner_spans);
        return (plain, StyledText { spans }, 1.0, links);
    }

    // ── Heading / body ────────────────────────────────────────────────
    let (heading_level, content) = parse_heading_prefix(line);
    let scale = heading_scale(heading_level);
    let (spans, raw_links) = parse_inline(content, heading_level > 0, false, theme);
    let plain: String = spans.iter().map(|s| s.text.as_str()).collect();
    let links = resolve_links(&raw_links, &plain, 0);
    (plain, StyledText { spans }, scale, links)
}

// ── Block-level prefix detectors ───────────────────────────────────────────

/// Detect a blockquote prefix (`> ` or bare `>`).
///
/// Returns the content after the prefix, or `None` if not a blockquote.
fn parse_blockquote(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("> ") {
        Some(rest)
    } else if line.starts_with('>') && !line.starts_with(">>>") {
        // Bare ">" or ">text" (no space) — strip only the leading ">".
        Some(&line[1..])
    } else {
        None
    }
}

/// Detect a bullet list item (`- ` or `* ` prefix).
///
/// Returns the item content (after the prefix), or `None`.
fn parse_bullet_item(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("- ") {
        Some(rest)
    } else if let Some(rest) = line.strip_prefix("* ") {
        Some(rest)
    } else {
        None
    }
}

/// Detect an ordered list item (`N. ` prefix where N is one or more digits).
///
/// Returns `(number, item_content)`, or `None`.
fn parse_ordered_item(line: &str) -> Option<(u32, &str)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    if i >= bytes.len() || bytes[i] != b'.' {
        return None;
    }
    if i + 1 >= bytes.len() || bytes[i + 1] != b' ' {
        return None;
    }
    let num: u32 = line[..i].parse().ok()?;
    Some((num, &line[i + 2..]))
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

// ── Link side-channel helper ───────────────────────────────────────────────

/// Resolve [`RawLink`]s to `(byte_start, byte_end, url)` triples into a line's
/// plain text.
///
/// `inner_plain` is the plain text of the inline content (without any block
/// prefix).  `prefix_len` is the byte length of the block prefix prepended
/// before `inner_plain` in the final `plain` string (e.g. `"  • "` = 6 bytes).
///
/// Links are resolved left-to-right: each link text is searched for in
/// `inner_plain` starting from after the previous match, so duplicate link
/// texts resolve to distinct, non-overlapping positions.
fn resolve_links(
    raw_links: &[RawLink],
    inner_plain: &str,
    prefix_len: usize,
) -> Vec<(usize, usize, String)> {
    let mut result = Vec::new();
    let mut search_from = 0usize;
    for link in raw_links {
        if let Some(pos) = inner_plain[search_from..].find(&link.text) {
            let abs_start = prefix_len + search_from + pos;
            let abs_end = abs_start + link.text.len();
            result.push((abs_start, abs_end, link.url.clone()));
            search_from += pos + link.text.len();
        }
    }
    result
}

// ── Inline span parser ─────────────────────────────────────────────────────

/// Parse inline markdown in `text` with the given base `bold`/`italic` flags,
/// returning a flat list of [`StyledSpan`]s and any [`RawLink`]s found.
///
/// Delimiters are matched left-to-right with the following priority:
///
/// 1. `` ` ``…`` ` `` — inline code (no further parsing inside)
/// 2. `[text](url)` — link (no emphasis parsing inside the link text)
/// 3. `**` / `__` — bold (content is recursively parsed)
/// 4. `*` / `_` — italic (content is recursively parsed)
///
/// Emphasis (`*`/`_`) delimiters obey a CommonMark-style **flanking** rule so
/// that intraword and dangling delimiters are *not* mistaken for emphasis:
///
/// * `the foo_bar and baz_qux funcs` — the `_`s are intraword, so no emphasis
///   fires (critical for `snake_case` identifiers in review-findings bodies).
/// * `a * b * c` — the `*`s are surrounded by whitespace, so they cannot open
///   or close emphasis.
///
/// A delimiter is only treated as emphasis when it can *open* at its position
/// **and** a later delimiter that can *close* exists.  Otherwise it is emitted
/// as literal plain text.  See [`can_open`] / [`can_close`].
///
/// Unmatched or non-flanking delimiters are treated as literal plain text.
fn parse_inline(
    text: &str,
    bold: bool,
    italic: bool,
    theme: &Theme,
) -> (Vec<StyledSpan>, Vec<RawLink>) {
    let mut spans: Vec<StyledSpan> = Vec::new();
    let mut links: Vec<RawLink> = Vec::new();
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
                // Inline code keeps the surrounding `bold` (so a code span in a
                // heading stays bold) but is never italicised.
                spans.push(StyledSpan {
                    text: code_text.to_string(),
                    fg: Some(theme.accent_fg),
                    bg: None,
                    bold,
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

        // ── Link: [text](url) ────────────────────────────────────────────
        if b == b'[' {
            if let Some((link_text, url, after)) = parse_link(text, pos) {
                if plain_start < pos {
                    spans.push(make_span(&text[plain_start..pos], bold, italic, None));
                }
                spans.push(StyledSpan {
                    text: link_text.to_string(),
                    fg: Some(theme.link_fg),
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: true,
                });
                links.push(RawLink {
                    text: link_text.to_string(),
                    url: url.to_string(),
                });
                pos = after;
                plain_start = pos;
                continue;
            }
            // No valid link syntax — treat `[` as plain text.
            pos += 1;
            continue;
        }

        // ── Bold: **...** / __...__ ──────────────────────────────────────
        if (b == b'*' || b == b'_') && pos + 1 < len && bytes[pos + 1] == b {
            let run_end = pos + 2;
            if can_open(text, pos, run_end, b) {
                if let Some(close) = find_emphasis_close(text, run_end, b, 2) {
                    if plain_start < pos {
                        spans.push(make_span(&text[plain_start..pos], bold, italic, None));
                    }
                    let inner = &text[run_end..close];
                    let (inner_spans, inner_links) = parse_inline(inner, true, italic, theme);
                    spans.extend(inner_spans);
                    links.extend(inner_links);
                    pos = close + 2;
                    plain_start = pos;
                    continue;
                }
            }
            // Cannot open, or no valid close — emit both chars as plain text.
            pos += 2;
            continue;
        }

        // ── Italic: *...* / _..._ ────────────────────────────────────────
        // This branch only fires when the bold branch above did not match,
        // i.e. the current character is a lone `*` / `_`.
        if b == b'*' || b == b'_' {
            let run_end = pos + 1;
            if can_open(text, pos, run_end, b) {
                if let Some(close) = find_emphasis_close(text, run_end, b, 1) {
                    if plain_start < pos {
                        spans.push(make_span(&text[plain_start..pos], bold, italic, None));
                    }
                    let inner = &text[run_end..close];
                    let (inner_spans, inner_links) = parse_inline(inner, bold, true, theme);
                    spans.extend(inner_spans);
                    links.extend(inner_links);
                    pos = close + 1;
                    plain_start = pos;
                    continue;
                }
            }
            // Non-flanking or unmatched delimiter — leave as plain text.
            pos += 1;
            continue;
        }

        pos += 1;
    }

    // Flush any remaining plain text.
    if plain_start < len {
        spans.push(make_span(&text[plain_start..], bold, italic, None));
    }

    (spans, links)
}

// ── Link syntax parser ─────────────────────────────────────────────────────

/// Attempt to parse a Markdown link starting at `start` (which must be `[`).
///
/// Returns `(link_text, url, byte_after)` on success, or `None` if the
/// text at `start` does not form a valid `[text](url)` pattern.
///
/// Known limitation: URLs containing `)` are truncated at the first `)`.
/// This is a deliberate first-cut simplification.
fn parse_link(text: &str, start: usize) -> Option<(&str, &str, usize)> {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Scan forward to find the matching `]`, tracking bracket nesting.
    let mut i = start + 1;
    let mut depth = 1i32;
    while i < len {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b'\\' if i + 1 < len => {
                // Skip escaped character.
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 || i >= len {
        return None;
    }

    let close_bracket = i; // index of `]`
    let link_text = &text[start + 1..close_bracket];

    // Require `(` immediately after `]`.
    let after_bracket = close_bracket + 1;
    if after_bracket >= len || bytes[after_bracket] != b'(' {
        return None;
    }

    let url_start = after_bracket + 1;
    // Find the closing `)`.  Simple scan — does not handle nested parens.
    let url_end = text[url_start..].find(')')?;
    let url = &text[url_start..url_start + url_end];
    let after = url_start + url_end + 1;

    Some((link_text, url, after))
}

// ── Flanking rules (CommonMark-subset) ─────────────────────────────────────

/// The character immediately before byte index `pos` in `text` (None at start).
fn char_before(text: &str, pos: usize) -> Option<char> {
    text[..pos].chars().next_back()
}

/// The character immediately at/after byte index `pos` in `text` (None at end).
fn char_at(text: &str, pos: usize) -> Option<char> {
    text[pos..].chars().next()
}

/// A boundary char (None = start/end of string) counts as whitespace for the
/// purpose of flanking classification.
fn is_ws_boundary(c: Option<char>) -> bool {
    match c {
        None => true,
        Some(ch) => ch.is_whitespace(),
    }
}

/// ASCII/Unicode punctuation classification used by the flanking rules.
fn is_punct(c: Option<char>) -> bool {
    matches!(c, Some(ch) if ch.is_ascii_punctuation())
}

/// A delimiter run spanning bytes `start..end` is **left-flanking** if it is
/// not followed by whitespace, and either not followed by punctuation or both
/// preceded and (effectively) bounded by whitespace/punctuation.
fn is_left_flanking(text: &str, start: usize, end: usize) -> bool {
    let after = char_at(text, end);
    let before = char_before(text, start);
    if is_ws_boundary(after) {
        return false;
    }
    !is_punct(after) || is_ws_boundary(before) || is_punct(before)
}

/// A delimiter run spanning bytes `start..end` is **right-flanking** if it is
/// not preceded by whitespace, and either not preceded by punctuation or both
/// followed and (effectively) bounded by whitespace/punctuation.
fn is_right_flanking(text: &str, start: usize, end: usize) -> bool {
    let before = char_before(text, start);
    let after = char_at(text, end);
    if is_ws_boundary(before) {
        return false;
    }
    !is_punct(before) || is_ws_boundary(after) || is_punct(after)
}

/// Whether a delimiter run of `delim` (`b'*'` or `b'_'`) spanning bytes
/// `start..end` can *open* emphasis at its position.
///
/// `*` may open whenever it is left-flanking (intraword `*` is permitted by
/// CommonMark).  `_` additionally must not be intraword: it may only open when
/// it is left-flanking and either not right-flanking or preceded by
/// punctuation — this is what stops `foo_bar` from italicising.
fn can_open(text: &str, start: usize, end: usize, delim: u8) -> bool {
    let left = is_left_flanking(text, start, end);
    if delim == b'*' {
        left
    } else {
        left && (!is_right_flanking(text, start, end) || is_punct(char_before(text, start)))
    }
}

/// Whether a delimiter run of `delim` spanning bytes `start..end` can *close*
/// emphasis.  Mirror image of [`can_open`].
fn can_close(text: &str, start: usize, end: usize, delim: u8) -> bool {
    let right = is_right_flanking(text, start, end);
    if delim == b'*' {
        right
    } else {
        right && (!is_left_flanking(text, start, end) || is_punct(char_at(text, end)))
    }
}

/// Scan forward from byte index `from` for a delimiter run of `delim` whose
/// length is at least `run` and that satisfies [`can_close`].  Returns the byte
/// index of the start of the closing run (the last `run` delimiters of it), or
/// `None` if no valid closer exists.
fn find_emphasis_close(text: &str, from: usize, delim: u8, run: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = from;
    while i < len {
        if bytes[i] == delim {
            let mut j = i;
            while j < len && bytes[j] == delim {
                j += 1;
            }
            let this_run = j - i;
            // A non-empty span is required, so the closer must start after
            // `from`; `from == i` would mean empty content, skip it.
            if this_run >= run && i > from && can_close(text, i, j, delim) {
                return Some(j - run);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    None
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
            // New constructs must also maintain the invariant.
            "- bullet item",
            "1. numbered item",
            "> blockquote",
            "```rust\nfn main() {}\n```",
            "see [link](http://example.com) here",
            // Unclosed code block.
            "```\nunclosed",
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

        assert!(
            spans.iter().any(|s| s.bold && s.text.contains("bold")),
            "missing bold span; spans: {spans:?}"
        );
        assert!(
            spans.iter().any(|s| s.italic && s.text.contains("italic")),
            "missing italic span; spans: {spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.fg == Some(theme.accent_fg) && s.text.contains("code")),
            "missing code span; spans: {spans:?}"
        );
        assert!(
            spans.iter().any(|s| s.text.contains(" and ")),
            "expected plain ' and ' separator; spans: {spans:?}"
        );
        assert_eq!(r.line_text[0], "bold and italic and code");
    }

    #[test]
    fn bold_italic_combined_in_heading() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("# *emphasized heading*", &theme);
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
        assert_eq!(r.lines[0].spans.len(), 1);
        assert!(!r.lines[0].spans[0].bold);
        assert!(!r.lines[0].spans[0].italic);
        assert!(r.lines[0].spans[0].fg.is_none());
    }

    // ── Edge cases ─────────────────────────────────────────────────────

    #[test]
    fn unmatched_delimiter_treated_as_plain_text() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("price is $5*2", &theme);
        assert_eq!(r.line_text[0], "price is $5*2");
        assert!(r.lines[0].spans.iter().all(|s| !s.italic));
    }

    // ── Flanking / intraword emphasis guards ───────────────────────────

    #[test]
    fn intraword_underscores_do_not_italicise() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("the foo_bar and baz_qux funcs", &theme);
        assert!(
            r.lines[0].spans.iter().all(|s| !s.italic),
            "intraword underscores must not italicise; spans: {:?}",
            r.lines[0].spans
        );
        assert_eq!(r.line_text[0], "the foo_bar and baz_qux funcs");
    }

    #[test]
    fn whitespace_flanked_asterisks_do_not_italicise() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("a * b * c", &theme);
        assert!(
            r.lines[0].spans.iter().all(|s| !s.italic),
            "whitespace-flanked asterisks must not italicise; spans: {:?}",
            r.lines[0].spans
        );
        assert_eq!(r.line_text[0], "a * b * c");
    }

    #[test]
    fn single_snake_case_underscore_is_literal() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("render_markdown_to_styled does work", &theme);
        assert!(r.lines[0].spans.iter().all(|s| !s.italic));
        assert_eq!(r.line_text[0], "render_markdown_to_styled does work");
    }

    #[test]
    fn intraword_double_underscore_does_not_bold() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("foo__bar__baz", &theme);
        assert!(
            r.lines[0].spans.iter().all(|s| !s.bold),
            "intraword __ must not bold; spans: {:?}",
            r.lines[0].spans
        );
        assert_eq!(r.line_text[0], "foo__bar__baz");
    }

    #[test]
    fn well_formed_emphasis_still_works_after_flanking_guard() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("use *italic* and _also_ and **bold**", &theme);
        let spans = &r.lines[0].spans;
        assert!(spans.iter().any(|s| s.italic && s.text == "italic"));
        assert!(spans.iter().any(|s| s.italic && s.text == "also"));
        assert!(spans.iter().any(|s| s.bold && s.text == "bold"));
    }

    #[test]
    fn inline_code_in_heading_stays_bold() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("# the `fn` keyword", &theme);
        let code_span = r.lines[0]
            .spans
            .iter()
            .find(|s| s.fg == Some(theme.accent_fg))
            .expect("expected an inline-code span in the heading");
        assert!(code_span.bold, "inline code in a heading should be bold");
        assert!(!code_span.italic);
    }

    #[test]
    fn heading_with_no_content_is_still_a_heading() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("# ", &theme);
        assert!((r.line_scales[0] - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn hash_without_space_is_not_a_heading() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("#title", &theme);
        assert!(
            (r.line_scales[0] - 1.0).abs() < f32::EPSILON,
            "should be body, not heading"
        );
        assert_eq!(r.line_text[0], "#title");
    }

    // ── Bulleted lists ─────────────────────────────────────────────────

    #[test]
    fn dash_bullet_emits_bullet_glyph_and_accent() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("- hello", &theme);
        assert_eq!(r.lines.len(), 1);
        // plain text includes the "• " marker.
        assert!(
            r.line_text[0].contains('\u{2022}'),
            "plain text must contain bullet glyph; got: {:?}",
            r.line_text[0]
        );
        assert!(
            r.line_text[0].contains("hello"),
            "plain text must include the item content"
        );
        // The bullet span is coloured with accent_fg.
        assert!(
            r.lines[0]
                .spans
                .iter()
                .any(|s| s.fg == Some(theme.accent_fg) && s.text.contains('\u{2022}')),
            "expected an accent-coloured bullet span; spans: {:?}",
            r.lines[0].spans
        );
    }

    #[test]
    fn asterisk_bullet_emits_bullet_glyph() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("* world", &theme);
        assert!(r.line_text[0].contains('\u{2022}'));
        assert!(r.line_text[0].contains("world"));
    }

    #[test]
    fn bullet_item_inline_markup_is_parsed() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("- **bold** item", &theme);
        assert!(
            r.lines[0].spans.iter().any(|s| s.bold && s.text == "bold"),
            "inline bold inside bullet must be parsed; spans: {:?}",
            r.lines[0].spans
        );
        assert_eq!(r.line_text[0], "  \u{2022} bold item");
    }

    #[test]
    fn bullet_scale_is_1_0() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("- item", &theme);
        assert!((r.line_scales[0] - 1.0).abs() < f32::EPSILON);
    }

    // ── Numbered lists ─────────────────────────────────────────────────

    #[test]
    fn numbered_list_emits_number_and_accent() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("1. first", &theme);
        assert_eq!(r.lines.len(), 1);
        assert!(
            r.line_text[0].contains("1."),
            "plain text must contain the marker"
        );
        assert!(r.line_text[0].contains("first"));
        assert!(
            r.lines[0]
                .spans
                .iter()
                .any(|s| s.fg == Some(theme.accent_fg) && s.text.contains("1.")),
            "expected accent-coloured marker span; spans: {:?}",
            r.lines[0].spans
        );
    }

    #[test]
    fn numbered_list_multi_digit() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("10. tenth", &theme);
        assert!(r.line_text[0].contains("10."));
        assert!(r.line_text[0].contains("tenth"));
    }

    #[test]
    fn numbered_list_inline_markup_is_parsed() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("2. *italic* content", &theme);
        assert!(
            r.lines[0]
                .spans
                .iter()
                .any(|s| s.italic && s.text == "italic"),
            "inline italic inside numbered item must be parsed; spans: {:?}",
            r.lines[0].spans
        );
    }

    #[test]
    fn numbered_scale_is_1_0() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("3. item", &theme);
        assert!((r.line_scales[0] - 1.0).abs() < f32::EPSILON);
    }

    // ── Fenced code blocks ─────────────────────────────────────────────

    #[test]
    fn code_block_three_lines_produce_three_entries() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("```rust\nfn main() {}\n```", &theme);
        assert_eq!(r.lines.len(), 3, "fence-open, content, fence-close");
        // All three lines must have scale 1.0.
        for scale in &r.line_scales {
            assert!((scale - 1.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn code_block_fence_line_uses_muted_fg() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("```\ncode\n```", &theme);
        // First and last lines are fence lines — muted_fg.
        let fence_open = &r.lines[0].spans[0];
        assert_eq!(
            fence_open.fg,
            Some(theme.muted_fg),
            "fence line must use muted_fg"
        );
    }

    #[test]
    fn code_block_content_line_uses_foreground() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("```\ncontent\n```", &theme);
        let content = &r.lines[1].spans[0];
        assert_eq!(
            content.fg,
            Some(theme.foreground),
            "code content must use foreground fg"
        );
    }

    #[test]
    fn code_block_content_not_emphasis_parsed() {
        let theme = Theme::default();
        // **bold** inside a code block must NOT produce a bold span.
        let r = render_markdown_to_styled("```\n**not bold**\n```", &theme);
        assert!(
            r.lines[1].spans.iter().all(|s| !s.bold),
            "content inside code block must not be emphasis-parsed"
        );
        assert_eq!(r.line_text[1], "**not bold**");
    }

    #[test]
    fn code_block_range_side_channel_populated() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("```rust\nfn f() {}\n```", &theme);
        assert_eq!(r.code_blocks.len(), 1);
        let cb = &r.code_blocks[0];
        assert_eq!(cb.fence_open, 0);
        assert_eq!(cb.fence_close, Some(2));
        assert_eq!(cb.lang.as_deref(), Some("rust"));
    }

    #[test]
    fn code_block_no_lang_tag() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("```\ncode\n```", &theme);
        assert_eq!(r.code_blocks.len(), 1);
        assert!(r.code_blocks[0].lang.is_none());
    }

    #[test]
    fn unclosed_code_block_recorded_with_no_close() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("```\nline1\nline2", &theme);
        assert_eq!(r.lines.len(), 3);
        assert_eq!(r.code_blocks.len(), 1);
        assert_eq!(r.code_blocks[0].fence_open, 0);
        assert!(r.code_blocks[0].fence_close.is_none());
    }

    // ── Links ──────────────────────────────────────────────────────────

    #[test]
    fn link_emits_underlined_link_fg_span() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("[click here](http://example.com)", &theme);
        assert_eq!(r.lines.len(), 1);
        let link_span = r.lines[0]
            .spans
            .iter()
            .find(|s| s.text == "click here")
            .expect("expected a span with the link text");
        assert!(link_span.underline, "link span must be underlined");
        assert_eq!(
            link_span.fg,
            Some(theme.link_fg),
            "link span must use link_fg"
        );
    }

    #[test]
    fn link_plain_text_is_link_text_not_url() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("[label](http://example.com)", &theme);
        assert_eq!(r.line_text[0], "label");
    }

    #[test]
    fn link_side_channel_populated() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("[label](http://example.com)", &theme);
        assert_eq!(r.links.len(), 1);
        let (line_idx, ref range, ref url) = r.links[0];
        assert_eq!(line_idx, 0);
        assert_eq!(&r.line_text[0][range.clone()], "label");
        assert_eq!(url, "http://example.com");
    }

    #[test]
    fn link_inside_body_text_mixed() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("see [docs](http://docs.example.com) for more", &theme);
        assert_eq!(r.line_text[0], "see docs for more");
        assert_eq!(r.links.len(), 1);
        let (_, ref range, _) = r.links[0];
        assert_eq!(&r.line_text[0][range.clone()], "docs");
    }

    #[test]
    fn link_side_channel_byte_range_matches_plain_text() {
        // Multiple links on one line — ranges must point to the correct text.
        let theme = Theme::default();
        let r = render_markdown_to_styled("[alpha](http://a.com) and [beta](http://b.com)", &theme);
        assert_eq!(r.links.len(), 2);
        let (_, ref r0, ref u0) = r.links[0];
        let (_, ref r1, ref u1) = r.links[1];
        assert_eq!(&r.line_text[0][r0.clone()], "alpha");
        assert_eq!(u0, "http://a.com");
        assert_eq!(&r.line_text[0][r1.clone()], "beta");
        assert_eq!(u1, "http://b.com");
    }

    // ── Blockquotes ────────────────────────────────────────────────────

    #[test]
    fn blockquote_emits_bar_glyph_and_link_fg() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("> quoted text", &theme);
        assert_eq!(r.lines.len(), 1);
        // The bar span uses link_fg.
        assert!(
            r.lines[0]
                .spans
                .iter()
                .any(|s| s.fg == Some(theme.link_fg) && s.text.contains('\u{2502}')),
            "expected a link_fg bar span; spans: {:?}",
            r.lines[0].spans
        );
        // plain text includes the bar prefix.
        assert!(r.line_text[0].contains('\u{2502}'));
        assert!(r.line_text[0].contains("quoted text"));
    }

    #[test]
    fn blockquote_inline_markup_is_parsed() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("> **bold** inside quote", &theme);
        assert!(
            r.lines[0].spans.iter().any(|s| s.bold && s.text == "bold"),
            "inline bold inside blockquote must be parsed; spans: {:?}",
            r.lines[0].spans
        );
    }

    #[test]
    fn blockquote_scale_is_1_0() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("> quote", &theme);
        assert!((r.line_scales[0] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn blockquote_link_inside_quote_tracked() {
        let theme = Theme::default();
        let r = render_markdown_to_styled("> see [here](http://example.com)", &theme);
        assert_eq!(r.links.len(), 1);
        let (line_idx, ref range, ref url) = r.links[0];
        assert_eq!(line_idx, 0);
        assert_eq!(&r.line_text[0][range.clone()], "here");
        assert_eq!(url, "http://example.com");
    }
}
