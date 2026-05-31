//! Helpers for parsing Claude CLI / Anthropic API stream-json output into
//! [`crate::compose::chat_controller::ChatTurn`]s.
//!
//! # Claude CLI stream-json format
//!
//! Running `claude -p --output-format stream-json` emits NDJSON where each
//! line is a complete JSON object.  The relevant event types are:
//!
//! - `"assistant"` — an assistant reply.  The nested
//!   `message.content[*]` array may contain `"text"` items and / or
//!   `"tool_use"` items.  Tool-only turns are not user-visible in a
//!   chat transcript and are skipped.
//! - Other types (`"system"`, `"result"`, etc.) are not chat content and
//!   are also skipped.
//!
//! Inner whitespace (newlines, indentation) inside the assistant's text is
//! preserved, so numbered lists and multi-line answers render correctly in
//! [`crate::compose::ChatController`].
//!
//! # Usage
//!
//! ```rust
//! use quadraui::ai::parse_stream_json_turns;
//!
//! let lines: Vec<String> = vec![
//!     r#"{"type":"system","subtype":"init","session_id":"abc"}"#.into(),
//!     r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello!"}]}}"#.into(),
//!     r#"{"type":"result","subtype":"success","result":"Hello!"}"#.into(),
//! ];
//! let turns = parse_stream_json_turns(&lines);
//! assert_eq!(turns.len(), 1);
//! ```
//!
//! # Streaming (line-at-a-time)
//!
//! When an SSE drainer already processes lines one at a time, call
//! [`stream_json_turns_incremental`] per line and collect the
//! `Some(ChatTurn)` results:
//!
//! ```rust
//! use quadraui::ai::stream_json_turns_incremental;
//!
//! let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi!"}]}}"#;
//! let turn = stream_json_turns_incremental(line);
//! assert!(turn.is_some());
//! ```

use crate::compose::chat_controller::{ChatRole, ChatTurn};
use crate::types::StyledText;

/// Parse a slice of NDJSON lines from `claude --output-format stream-json`
/// (or the Anthropic streaming API) into assistant [`ChatTurn`]s.
///
/// Each line is parsed with [`stream_json_turns_incremental`].  Non-assistant
/// events, tool-only turns, and empty / whitespace-only text bodies are
/// silently dropped.  The caller merges the returned turns with their own
/// user turns and optional system header.
pub fn parse_stream_json_turns(lines: &[String]) -> Vec<ChatTurn> {
    lines
        .iter()
        .filter_map(|l| stream_json_turns_incremental(l.as_str()))
        .collect()
}

/// Feed one NDJSON line from the stream, returning an assistant [`ChatTurn`]
/// if the line represents a non-empty, non-tool-only assistant event.
///
/// Returns `None` for:
/// - Lines that are not valid JSON.
/// - Non-`"assistant"` event types (`"system"`, `"result"`, …).
/// - Turns whose `message.content` contains only `"tool_use"` items with no
///   `"text"` content (tool calls belong in a log view, not a chat).
/// - Turns whose combined text body is empty or whitespace-only.
///
/// Useful when wiring an SSE drainer that accumulates lines one at a time.
pub fn stream_json_turns_incremental(line: &str) -> Option<ChatTurn> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let content = v.get("message")?.get("content")?.as_array()?;
    let text: String = content
        .iter()
        .filter_map(|item| {
            if item.get("type")?.as_str()? == "text" {
                Some(item.get("text")?.as_str()?.to_string())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        return None;
    }
    Some(ChatTurn {
        role: ChatRole::Assistant,
        text: StyledText::plain(text),
        timestamp_unix: None,
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::chat_controller::ChatRole;

    // ── stream_json_turns_incremental ─────────────────────────────────────────

    #[test]
    fn incremental_parses_text_assistant_event() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello!"}]}}"#;
        let turn = stream_json_turns_incremental(line).expect("should parse");
        assert_eq!(turn.role, ChatRole::Assistant);
        let body: String = turn.text.spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(body, "Hello!");
    }

    #[test]
    fn incremental_skips_system_event() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
        assert!(stream_json_turns_incremental(line).is_none());
    }

    #[test]
    fn incremental_skips_result_event() {
        let line = r#"{"type":"result","subtype":"success","result":"ok"}"#;
        assert!(stream_json_turns_incremental(line).is_none());
    }

    #[test]
    fn incremental_skips_tool_only_turn() {
        // An assistant event whose content contains only tool_use, no text.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"bash","input":{}}]}}"#;
        assert!(stream_json_turns_incremental(line).is_none());
    }

    #[test]
    fn incremental_skips_whitespace_only_text() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"   \n  "}]}}"#;
        assert!(stream_json_turns_incremental(line).is_none());
    }

    #[test]
    fn incremental_preserves_inner_newlines() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"1. first\n2. second"}]}}"#;
        let turn = stream_json_turns_incremental(line).expect("should parse");
        let body: String = turn.text.spans.iter().map(|s| s.text.as_str()).collect();
        assert!(body.contains('\n'), "inner newlines should be preserved");
        assert!(body.contains("1. first"));
        assert!(body.contains("2. second"));
    }

    #[test]
    fn incremental_skips_malformed_json() {
        assert!(stream_json_turns_incremental("not json").is_none());
        assert!(stream_json_turns_incremental("").is_none());
        assert!(stream_json_turns_incremental("  ").is_none());
    }

    #[test]
    fn incremental_concatenates_multiple_text_spans() {
        // Multiple text items in content are joined.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"},{"type":"text","text":" world"}]}}"#;
        let turn = stream_json_turns_incremental(line).expect("should parse");
        let body: String = turn.text.spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(body, "Hello world");
    }

    #[test]
    fn incremental_skips_text_item_when_tool_use_precedes_and_no_text() {
        // Mixed content where the only text item is empty after trimming.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"view","input":{}},{"type":"text","text":""}]}}"#;
        assert!(stream_json_turns_incremental(line).is_none());
    }

    // ── parse_stream_json_turns ───────────────────────────────────────────────

    #[test]
    fn batch_parses_only_assistant_turns() {
        let lines = vec![
            r#"{"type":"system","subtype":"init","session_id":"s1"}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}}"#
                .to_string(),
            r#"{"type":"result","subtype":"success","result":"Hi"}"#.to_string(),
        ];
        let turns = parse_stream_json_turns(&lines);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].role, ChatRole::Assistant);
    }

    #[test]
    fn batch_returns_empty_when_no_assistant_events() {
        let lines = vec![
            r#"{"type":"system","subtype":"init"}"#.to_string(),
            r#"{"type":"result","subtype":"success"}"#.to_string(),
        ];
        assert!(parse_stream_json_turns(&lines).is_empty());
    }

    #[test]
    fn batch_returns_multiple_turns() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"First"}]}}"#
                .to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Second"}]}}"#
                .to_string(),
        ];
        let turns = parse_stream_json_turns(&lines);
        assert_eq!(turns.len(), 2);
    }
}
