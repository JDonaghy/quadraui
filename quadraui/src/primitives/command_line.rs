//! `CommandLine` primitive: a single-line input/output surface for editor
//! command prompts (`:`, `/`, `?`) and transient messages.
//!
//! Display-only �� the engine handles keystroke input and updates
//! `text` / `cursor_offset` each frame. Both TUI and GTK rasterisers
//! draw text with an optional insert cursor; alignment can be flipped
//! for right-aligned count displays.

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
}
