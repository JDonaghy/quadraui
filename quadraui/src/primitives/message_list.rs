//! `MessageList` primitive: a scrollable list of styled lines used by
//! chat-style panels (vimcode's AI assistant sidebar).
//!
//! Each row carries its own foreground colour and a small left-indent
//! offset so role labels (`You:` / `AI:`) line up flush-left while
//! content lines indent. The panel background is supplied by the
//! caller — message bodies share one fill. Per-message bg highlighting
//! could be added later as an optional `bg_override` field; the current
//! shape mirrors what both vimcode rasterisers emit today.
//!
//! Wrapping happens at the call site (vimcode's adapter splits message
//! content into wrap-width chunks before pushing rows) — the primitive
//! is data-only.

use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// A single row in a [`MessageList`].
///
/// `indent` is in surface units — TUI cells or GTK pixels — so the
/// caller picks the unit appropriate for its rasteriser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRow {
    pub text: String,
    pub fg: Color,
    /// Left-indent offset in surface units (cells / pixels).
    #[serde(default)]
    pub indent: f32,
}

impl MessageRow {
    pub fn new(text: impl Into<String>, fg: Color, indent: f32) -> Self {
        Self {
            text: text.into(),
            fg,
            indent,
        }
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
