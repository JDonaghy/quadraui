//! Shared primitive types used across widgets: colours, icons, styled text,
//! widget identifiers, modifier flags, and tree paths.
//!
//! Design invariants (from `docs/UI_CRATE_DESIGN.md` §10, plugin-driven UI):
//! - Every type is owned (no `&'static` or borrowed data in public API).
//! - Every type is `Serialize + Deserialize` so Lua plugins can produce them
//!   via JSON → Rust struct conversion.
//! - No Rust closures cross the Rust/plugin boundary; events are plain data.

use serde::{Deserialize, Serialize};

/// RGBA colour, 0-255 each channel.
///
/// Matches the shape of `vimcode::Color` conceptually. Adapter code in apps
/// (e.g. vimcode) maps app-specific colour types to this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parse `"#rrggbb"` or `"#rrggbbaa"`. Returns `None` on malformed input.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        let (r, g, b, a) = match s.len() {
            6 => (
                u8::from_str_radix(&s[0..2], 16).ok()?,
                u8::from_str_radix(&s[2..4], 16).ok()?,
                u8::from_str_radix(&s[4..6], 16).ok()?,
                255,
            ),
            8 => (
                u8::from_str_radix(&s[0..2], 16).ok()?,
                u8::from_str_radix(&s[2..4], 16).ok()?,
                u8::from_str_radix(&s[4..6], 16).ok()?,
                u8::from_str_radix(&s[6..8], 16).ok()?,
            ),
            _ => return None,
        };
        Some(Self { r, g, b, a })
    }
}

/// A contiguous run of text sharing a single style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledSpan {
    pub text: String,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
}

impl StyledSpan {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }

    pub fn with_fg(text: impl Into<String>, fg: Color) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

/// A sequence of styled spans rendered inline on one line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledText {
    pub spans: Vec<StyledSpan>,
}

impl StyledText {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            spans: vec![StyledSpan::plain(text)],
        }
    }

    pub fn colored(text: impl Into<String>, fg: Color) -> Self {
        Self {
            spans: vec![StyledSpan::with_fg(text, fg)],
        }
    }

    pub fn visible_width(&self) -> usize {
        self.spans.iter().map(|s| s.text.chars().count()).sum()
    }
}

/// Icon reference. Each app supplies both the glyph (for icon fonts) and
/// an ASCII/Unicode fallback (for TUI or when the icon font is unavailable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Icon {
    /// Nerd Font / icon-font glyph string.
    pub glyph: String,
    /// ASCII or basic-Unicode fallback used when `glyph` cannot render.
    pub fallback: String,
}

impl Icon {
    pub fn new(glyph: impl Into<String>, fallback: impl Into<String>) -> Self {
        Self {
            glyph: glyph.into(),
            fallback: fallback.into(),
        }
    }
}

/// Stable identifier for a widget across frames.
///
/// Owned `String` so plugins can generate IDs at runtime. Apps should
/// namespace IDs (e.g. `"plugin:my-ext:main-form"`) to avoid collisions
/// with core widget IDs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WidgetId(pub String);

impl WidgetId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for WidgetId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for WidgetId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Keyboard modifier state carried alongside input events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Modifiers {
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub cmd: bool,
}

/// Path to a row in a tree-structured widget.
///
/// `vec![]` is the root; `vec![3, 1]` is the second child of the fourth
/// top-level row. Indices are `u16` to fit comfortably in small fixed-size
/// arrays while still accommodating trees with thousands of siblings.
pub type TreePath = Vec<u16>;

/// Selection behaviour for row-oriented widgets (`TreeView`, `ListView`,
/// `DataTable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SelectionMode {
    /// Rows cannot be selected.
    None,
    /// At most one row is selected at a time.
    #[default]
    Single,
    /// Multiple rows may be selected (shift/ctrl click).
    Multi,
}

/// Optional visual decoration applied to a row beyond its text colour.
///
/// Backends map each variant to an appropriate visual treatment. For row
/// widgets like `TreeView`, `Header` typically gets a distinct background
/// (section-header styling); `Muted` dims the foreground; `Error` /
/// `Warning` override the foreground with theme error/warning colours;
/// `Modified` implies italic but otherwise normal colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Decoration {
    /// Rendered with default foreground and background.
    #[default]
    Normal,
    /// Section-header styling: distinct (typically darker or accent) background,
    /// often with bold text. Use for rows that group others rather than
    /// represent a leaf entity. Row tree-hierarchy status (`is_expanded`
    /// Some vs. None) is orthogonal: not all branches are headers, and a
    /// leaf row can be a header.
    Header,
    /// Dimmed (e.g. gitignored file, stale git log entry).
    Muted,
    /// Red / error-toned (e.g. lint error, merge conflict).
    Error,
    /// Amber / warning-toned (e.g. unsaved changes badge).
    Warning,
    /// Italicised, typically unchanged foreground colour.
    Modified,
}

/// Right-aligned status indicator on a row: short text plus optional
/// foreground/background colouring. Used by `TreeView` / `ListView` /
/// `DataTable` for git-status letters, item counts, conflict markers, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Badge {
    pub text: String,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}

impl Badge {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            fg: None,
            bg: None,
        }
    }

    pub fn colored(text: impl Into<String>, fg: Color) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bg: None,
        }
    }
}

/// Visual style configuration for a `TreeView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeStyle {
    /// Cells / pixels per visual indent level.
    pub indent: u16,
    /// Draw expand/collapse chevrons on branches.
    pub show_chevrons: bool,
    /// Chevron drawn for an expanded branch.
    pub chevron_expanded: String,
    /// Chevron drawn for a collapsed branch.
    pub chevron_collapsed: String,
}

impl Default for TreeStyle {
    fn default() -> Self {
        Self {
            indent: 2,
            show_chevrons: true,
            chevron_expanded: "▾".into(),
            chevron_collapsed: "▸".into(),
        }
    }
}
